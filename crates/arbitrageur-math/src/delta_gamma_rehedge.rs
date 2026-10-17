//! Dynamic Options Delta-Gamma Rehedging under Toxic Adverse Selection & Jump Regimes.
//!
//! Formulates:
//! 1. Non-linear Whalley-Wilmott-Bouchaud power-law market impact ($I(v) = \eta |v|^\alpha$)
//!    coupled with Poisson jump-diffusion ($dN_t \sim \text{Poisson}(\lambda_J)$):
//!    $$\Delta S_{\text{rehedge}}^* = S \cdot \left(\frac{3 (\alpha + 1) \eta S^{\alpha - 1} |\Gamma|^{\alpha - 1}}{2 \gamma |\Gamma| \sigma^2}\right)^{\frac{1}{3 + \alpha}} \left(1 + \frac{\lambda_J \mathbb{E}[J^2]}{\sigma^2}\right)^{-1/6}$$
//! 2. Cross-venue perpetual synthetic options replication under continuous asymmetric funding carry $\phi_{\text{perp}}$:
//!    $$\Delta_{\text{synth}}^* = e^{-\phi_{\text{perp}} \tau} \Phi(d_1)$$
//!    $$C_{\text{synth}} = e^{-\phi_{\text{perp}} \tau} S \Phi(d_1) - K e^{-r \tau} \Phi(d_2)$$

use crate::black_scholes::standard_normal_cdf;
use serde::{Deserialize, Serialize};

/// Parameters for non-linear power-law impact and jump-diffusion delta-gamma hedging.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PowerLawHedgingConfig {
    /// Power-law market impact exponent $\alpha \in [0.4, 1.0]$. Bouchaud square-root law $\approx 0.5$.
    pub impact_exponent_alpha: f64,
    /// Permanent/temporary market impact scale factor $\eta$.
    pub impact_scale_eta: f64,
    /// Absolute risk aversion coefficient $\gamma_{\text{risk}}$.
    pub risk_aversion: f64,
    /// Annualized asset volatility $\sigma$.
    pub asset_volatility: f64,
    /// Poisson jump arrival intensity $\lambda_J$ (jumps per year).
    pub jump_intensity: f64,
    /// Expected squared jump magnitude $\mathbb{E}[J^2]$.
    pub expected_jump_size_sq: f64,
    /// Continuous annualized perpetual funding carry rate $\phi_{\text{perp}}$ (e.g. +0.10 for +10% APR).
    pub perpetual_funding_carry: f64,
    /// Risk-free rate $r$.
    pub risk_free_rate: f64,
}

impl Default for PowerLawHedgingConfig {
    fn default() -> Self {
        Self {
            impact_exponent_alpha: 0.5,    // Bouchaud square-root law
            impact_scale_eta: 0.0002,      // 2 bps impact coefficient
            risk_aversion: 0.0001,         // Institutional risk aversion
            asset_volatility: 0.65,        // 65% annualized crypto IV
            jump_intensity: 4.0,           // 4 major jump events / year
            expected_jump_size_sq: 0.0025, // 5% jump std dev squared
            perpetual_funding_carry: 0.08, // +8% APR funding carry
            risk_free_rate: 0.04,          // 4% USD risk-free rate
        }
    }
}

/// Actionable rehedge recommendation from the non-linear delta-gamma rehedging engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RehedgeRecommendation {
    /// Portfolio delta and gamma curvature are within the optimal no-transaction bandwidth.
    MaintainHedge {
        current_spot: f64,
        bandwidth_usd: f64,
        jump_compression_ratio: f64,
    },
    /// Portfolio delta drift has breached the optimal bandwidth; rebalance perpetual future.
    RebalancePerpetualDelta {
        underlying_asset: String,
        rebalance_qty: f64,
        target_delta: f64,
        bandwidth_usd: f64,
        recommended_perp_venue: String,
        reason: String,
    },
}

/// Analytical results of the non-linear rehedging bandwidth calculation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BichuchPowerLawBandwidth {
    pub unadjusted_bandwidth_usd: f64,
    pub jump_compression_multiplier: f64,
    pub optimal_bandwidth_usd: f64,
    pub bandwidth_pct_spot: f64,
}

/// Synthetic perpetual option replication metrics.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SyntheticOptionReplication {
    pub synthetic_delta: f64,
    pub synthetic_price: f64,
    pub funding_drag_usd: f64,
}

/// Non-linear Whalley-Wilmott-Bouchaud Delta-Gamma Rehedger.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct NonlinearDeltaGammaRehedger {
    pub config: PowerLawHedgingConfig,
}

impl NonlinearDeltaGammaRehedger {
    pub fn new(config: PowerLawHedgingConfig) -> Self {
        Self { config }
    }

    /// Computes the closed-form optimal rehedging bandwidth:
    /// $$\Delta S_{\text{rehedge}}^* = S \cdot \left(\frac{3 (\alpha + 1) \eta}{2 \gamma_{\text{risk}} S^2 |\Gamma| \sigma^2}\right)^{\frac{1}{3 + \alpha}} \left(1 + \frac{\lambda_J \mathbb{E}[J^2]}{\sigma^2}\right)^{-1/6}$$
    pub fn compute_optimal_bandwidth(&self, spot: f64, net_gamma: f64) -> BichuchPowerLawBandwidth {
        let s = spot.max(1.0);
        let gamma_abs = net_gamma.abs().max(1e-8);
        let alpha = self.config.impact_exponent_alpha.clamp(0.1, 2.0);
        let eta = self.config.impact_scale_eta.max(1e-6);
        let gamma_risk = self.config.risk_aversion.max(1e-6);
        let sigma = self.config.asset_volatility.max(0.05);
        let sigma_sq = sigma * sigma;

        // Dollar gamma = S^2 * |Gamma|
        let dollar_gamma = s * s * gamma_abs;
        let numerator = 3.0 * (alpha + 1.0) * eta;
        let denominator = 2.0 * gamma_risk * dollar_gamma * sigma_sq;

        let base_ratio = (numerator / denominator).max(1e-12);
        let exponent = 1.0 / (3.0 + alpha);
        let unadjusted_pct = base_ratio.powf(exponent);

        // Jump compression factor: (1 + lambda_J * E[J^2] / sigma^2)^(-1/6)
        let jump_var_ratio =
            (self.config.jump_intensity * self.config.expected_jump_size_sq) / sigma_sq;
        let jump_compression = (1.0 + jump_var_ratio).powf(-1.0 / 6.0);

        let final_pct = (unadjusted_pct * jump_compression).clamp(0.0001, 0.25);
        let final_bandwidth_usd = s * final_pct;
        let unadjusted_bandwidth_usd = s * unadjusted_pct;

        BichuchPowerLawBandwidth {
            unadjusted_bandwidth_usd,
            jump_compression_multiplier: jump_compression,
            optimal_bandwidth_usd: final_bandwidth_usd,
            bandwidth_pct_spot: final_pct,
        }
    }

    /// Computes synthetic perpetual option replication delta and price under continuous funding carry:
    /// $$\Delta_{\text{synth}}^* = e^{-\phi_{\text{perp}} \tau} \Phi(d_1)$$
    /// $$C_{\text{synth}} = e^{-\phi_{\text{perp}} \tau} S \Phi(d_1) - K e^{-r \tau} \Phi(d_2)$$
    pub fn compute_synthetic_replication(
        &self,
        spot: f64,
        strike: f64,
        tau_years: f64,
        is_call: bool,
    ) -> SyntheticOptionReplication {
        let s = spot.max(1.0);
        let k = strike.max(1.0);
        let tau = tau_years.max(1e-4);
        let phi = self.config.perpetual_funding_carry;
        let r = self.config.risk_free_rate;
        let sigma = self.config.asset_volatility.max(0.05);

        let sigma_sqrt_tau = sigma * tau.sqrt();
        let d1 = ((s / k).ln() + (r - phi + 0.5 * sigma * sigma) * tau) / sigma_sqrt_tau;
        let d2 = d1 - sigma_sqrt_tau;

        let carry_discount = (-phi * tau).exp();
        let rf_discount = (-r * tau).exp();

        let nd1 = standard_normal_cdf(d1);
        let nd2 = standard_normal_cdf(d2);

        if is_call {
            let delta = carry_discount * nd1;
            let price = (carry_discount * s * nd1 - rf_discount * k * nd2).max(0.0);
            let funding_drag = s * (1.0 - carry_discount) * nd1;
            SyntheticOptionReplication {
                synthetic_delta: delta,
                synthetic_price: price,
                funding_drag_usd: funding_drag,
            }
        } else {
            let n_neg_d1 = standard_normal_cdf(-d1);
            let n_neg_d2 = standard_normal_cdf(-d2);
            let delta = -carry_discount * n_neg_d1;
            let price = (rf_discount * k * n_neg_d2 - carry_discount * s * n_neg_d1).max(0.0);
            let funding_drag = s * (1.0 - carry_discount) * n_neg_d1;
            SyntheticOptionReplication {
                synthetic_delta: delta,
                synthetic_price: price,
                funding_drag_usd: funding_drag,
            }
        }
    }

    /// Evaluates live portfolio rehedging triggers and calculates the exact perpetual rebalance order.
    pub fn evaluate_portfolio_rehedge(
        &self,
        underlying_asset: &str,
        spot: f64,
        last_hedged_spot: f64,
        net_delta: f64,
        net_gamma: f64,
        candidate_perp_venues: &[(&str, f64)], // (venue_name, fee_bps)
    ) -> RehedgeRecommendation {
        let bandwidth = self.compute_optimal_bandwidth(spot, net_gamma);
        let spot_drift = (spot - last_hedged_spot).abs();

        // Calculate delta drift tolerance based on gamma: Delta drift = Gamma * spot_drift
        let delta_drift_tolerance = (net_gamma.abs() * bandwidth.optimal_bandwidth_usd).max(0.05);

        if spot_drift > bandwidth.optimal_bandwidth_usd || net_delta.abs() > delta_drift_tolerance {
            // Target delta: rehedge to boundary rather than 0 to minimize execution fees, bounded by current delta
            let target_delta =
                net_delta.signum() * (delta_drift_tolerance * 0.5).min(net_delta.abs() * 0.5);
            let rebalance_qty = -(net_delta - target_delta);

            // Select optimal venue with lowest trading friction
            let best_venue = candidate_perp_venues
                .iter()
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|v| v.0.to_string())
                .unwrap_or_else(|| "hyperliquid".to_string());

            RehedgeRecommendation::RebalancePerpetualDelta {
                underlying_asset: underlying_asset.to_string(),
                rebalance_qty,
                target_delta,
                bandwidth_usd: bandwidth.optimal_bandwidth_usd,
                recommended_perp_venue: best_venue,
                reason: format!(
                    "Spot drift ${:.2} exceeded optimal bandwidth ${:.2} (net delta: {:.4}, net gamma: {:.6})",
                    spot_drift, bandwidth.optimal_bandwidth_usd, net_delta, net_gamma
                ),
            }
        } else {
            RehedgeRecommendation::MaintainHedge {
                current_spot: spot,
                bandwidth_usd: bandwidth.optimal_bandwidth_usd,
                jump_compression_ratio: bandwidth.jump_compression_multiplier,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_power_law_bandwidth_jump_compression() {
        let mut cfg = PowerLawHedgingConfig {
            jump_intensity: 0.0, // No jumps
            ..Default::default()
        };
        let rehedger_no_jump = NonlinearDeltaGammaRehedger::new(cfg);
        let bw_no_jump = rehedger_no_jump.compute_optimal_bandwidth(60000.0, 0.0005);

        cfg.jump_intensity = 10.0; // Severe jump regime
        let rehedger_jump = NonlinearDeltaGammaRehedger::new(cfg);
        let bw_jump = rehedger_jump.compute_optimal_bandwidth(60000.0, 0.0005);

        assert!(
            bw_jump.optimal_bandwidth_usd < bw_no_jump.optimal_bandwidth_usd,
            "Jump diffusion must compress the optimal rehedging bandwidth"
        );
        assert!(
            bw_jump.jump_compression_multiplier < 1.0,
            "Jump compression multiplier must be < 1.0"
        );
    }

    #[test]
    fn test_synthetic_option_replication_funding_carry() {
        let mut cfg = PowerLawHedgingConfig {
            perpetual_funding_carry: 0.0, // Zero funding carry
            ..Default::default()
        };
        let rehedger_zero_carry = NonlinearDeltaGammaRehedger::new(cfg);
        let rep_zero = rehedger_zero_carry.compute_synthetic_replication(3000.0, 3000.0, 0.1, true);

        cfg.perpetual_funding_carry = 0.20; // 20% annualized funding carry
        let rehedger_high_carry = NonlinearDeltaGammaRehedger::new(cfg);
        let rep_high = rehedger_high_carry.compute_synthetic_replication(3000.0, 3000.0, 0.1, true);

        assert!(
            rep_high.synthetic_delta < rep_zero.synthetic_delta,
            "Positive funding carry must discount synthetic delta"
        );
        assert!(rep_high.funding_drag_usd > 0.0);
    }

    #[test]
    fn test_evaluate_portfolio_rehedge_trigger() {
        let rehedger = NonlinearDeltaGammaRehedger::default();
        let venues = [("hyperliquid", 1.5), ("binance", 4.0)];

        // Small drift within bandwidth
        let res_calm =
            rehedger.evaluate_portfolio_rehedge("BTC", 60000.0, 60050.0, 0.01, 0.0002, &venues);
        assert!(matches!(
            res_calm,
            RehedgeRecommendation::MaintainHedge { .. }
        ));

        // Large spot jump ($3,000 drift) with large net delta
        let res_jump =
            rehedger.evaluate_portfolio_rehedge("BTC", 60000.0, 63000.0, 0.35, 0.0005, &venues);
        if let RehedgeRecommendation::RebalancePerpetualDelta {
            rebalance_qty,
            recommended_perp_venue,
            ..
        } = res_jump
        {
            assert!(rebalance_qty < 0.0, "Long delta requires selling perp");
            assert_eq!(recommended_perp_venue, "hyperliquid");
        } else {
            panic!("Expected RebalancePerpetualDelta recommendation");
        }
    }
}
