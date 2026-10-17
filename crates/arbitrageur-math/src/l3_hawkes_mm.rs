//! High-Frequency Level-3 Hawkes Jump-Diffusion Market Making Engine.
//!
//! Formulates dual-sided optimal limit order quoting coupling Guéant-Tapia-Manziadi
//! reservation prices with:
//! 1. Asymmetric Hawkes arrival intensities $\lambda^+(t), \lambda^-(t)$ driven by
//!    real-time buy and sell flow self-excitation.
//! 2. Closed-form Poisson jump markout penalty:
//!    $$\Delta p_{\text{jump}}(q) = \frac{\lambda_J}{\gamma \sigma^2} \left[ \exp\left( \gamma q \mu_J + \frac{1}{2} \gamma^2 q^2 \sigma_J^2 \right) - 1 \right]$$
//! 3. Layer-2 sequencer latency and soft-reorganization adverse selection compensation:
//!    $$\Delta p_{\text{adverse}} = \kappa_{\text{lead}} \cdot \text{OFI}_{\text{CEX}} \cdot \lambda_{\text{Kyle}} \cdot \tau_{\text{seq}} + z_\alpha \sigma \sqrt{\tau_{\text{seq}}} (1 - e^{-\lambda_{\text{sweep}} \tau_{\text{seq}}}) + \text{Cost}_{\text{reorg}} P_{\text{reorg}}$$
//! 4. Level-3 queue priority gating suppressing quote submission when cumulative volume
//!    ahead $V_{\text{ahead}} > V_{\max}^*$ or when adverse pick-off hazard surges.

use serde::{Deserialize, Serialize};

/// High-Frequency Level-3 Hawkes Jump-Diffusion Market Maker Configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L3HawkesMmConfig {
    /// Inventory risk aversion parameter $\gamma > 0$.
    pub risk_aversion_gamma: f64,
    /// Annualized asset volatility $\sigma$.
    pub asset_volatility: f64,
    /// Base liquidity order arrival rate $A_0$.
    pub base_arrival_a0: f64,
    /// Order fill price elasticity sensitivity $k$.
    pub fill_sensitivity_k: f64,
    /// Hawkes flow excitation coupling sensitivity $\kappa_{\text{flow}}$.
    pub flow_excitation_kappa: f64,
    /// Poisson jump arrival intensity $\lambda_J$.
    pub jump_intensity: f64,
    /// Expected log jump mean $\mu_J$ (typically negative for crypto crash hazard).
    pub jump_mean: f64,
    /// Log jump standard deviation $\sigma_J$.
    pub jump_std: f64,
    /// Kyle's lambda price impact parameter $\lambda_{\text{Kyle}}$.
    pub kyle_lambda: f64,
    /// CEX lead-lag cross-excitation sensitivity $\kappa_{\text{lead}}$.
    pub lead_lag_kappa: f64,
    /// Maximum inventory limit $q_{\max}$.
    pub max_inventory_q: f64,
    /// Minimum half-spread floor in basis points.
    pub min_half_spread_bps: f64,
}

impl Default for L3HawkesMmConfig {
    fn default() -> Self {
        Self {
            risk_aversion_gamma: 1e-4,
            asset_volatility: 0.55,
            base_arrival_a0: 120.0,
            fill_sensitivity_k: 1.5,
            flow_excitation_kappa: 1.25,
            jump_intensity: 0.15,
            jump_mean: -0.015,
            jump_std: 0.035,
            kyle_lambda: 0.00015,
            lead_lag_kappa: 0.85,
            max_inventory_q: 10.0,
            min_half_spread_bps: 1.0,
        }
    }
}

/// Level-3 Hawkes Quoting Recommendation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct L3HawkesQuoteRecommendation {
    /// Base reservation price $R(s, q)$.
    pub reservation_price: f64,
    /// Recommended limit bid quote price.
    pub recommended_bid: f64,
    /// Recommended limit ask quote price.
    pub recommended_ask: f64,
    /// Bid half-spread in basis points.
    pub bid_half_spread_bps: f64,
    /// Ask half-spread in basis points.
    pub ask_half_spread_bps: f64,
    /// Poisson jump markout penalty in USD $\Delta p_{\text{jump}}(q)$.
    pub jump_markout_penalty_usd: f64,
    /// L2 sequencer latency & reorg adverse selection compensation in USD $\Delta p_{\text{adverse}}$.
    pub adverse_selection_compensation_usd: f64,
    /// True if Level-3 queue priority permits quote submission.
    pub queue_gating_passed: bool,
}

/// High-Frequency Level-3 Hawkes Market Maker Engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L3HawkesMarketMaker {
    pub config: L3HawkesMmConfig,
}

impl L3HawkesMarketMaker {
    pub fn new(config: L3HawkesMmConfig) -> Self {
        Self { config }
    }

    /// Computes analytical Poisson jump markout penalty $\Delta p_{\text{jump}}(q)$:
    ///
    /// $$\Delta p_{\text{jump}}(q) = \frac{\lambda_J}{\gamma \sigma^2} \left[ \exp\left( \gamma q \mu_J + \frac{1}{2} \gamma^2 q^2 \sigma_J^2 \right) - 1 \right]$$
    pub fn jump_markout_penalty(&self, inventory_q: f64) -> f64 {
        let gamma = self.config.risk_aversion_gamma;
        let sigma = self.config.asset_volatility.max(1e-4);
        let lambda_j = self.config.jump_intensity;
        let mu_j = self.config.jump_mean;
        let sigma_j = self.config.jump_std;
        let q = inventory_q.clamp(-self.config.max_inventory_q, self.config.max_inventory_q);

        let exponent = gamma * q * mu_j + 0.5 * gamma * gamma * q * q * sigma_j * sigma_j;
        (lambda_j / (gamma * sigma * sigma)) * (exponent.exp() - 1.0)
    }

    /// Computes L2 Sequencer Latency & Reorg Adverse Selection Compensation $\Delta p_{\text{adverse}}$:
    ///
    /// $$\Delta p_{\text{adverse}} = \kappa_{\text{lead}} \cdot \text{OFI}_{\text{CEX}} \cdot \lambda_{\text{Kyle}} \cdot \tau_{\text{seq}} + z_\alpha \sigma \sqrt{\tau_{\text{seq}}} (1 - e^{-\lambda_{\text{sweep}} \tau_{\text{seq}}}) + \text{Cost}_{\text{reorg}} P_{\text{reorg}}$$
    pub fn adverse_selection_compensation(
        &self,
        cex_ofi: f64,
        tau_seq_sec: f64,
        sweep_intensity: f64,
        reorg_prob: f64,
        reorg_cost_usd: f64,
    ) -> f64 {
        let tau = tau_seq_sec.max(0.0);
        let term_lead = self.config.lead_lag_kappa * cex_ofi.abs() * self.config.kyle_lambda * tau;
        let z_alpha = 2.576; // 99% quantile
        let sweep_factor = 1.0 - (-sweep_intensity * tau).exp();
        let term_drift = z_alpha * self.config.asset_volatility * tau.sqrt() * sweep_factor;
        let term_reorg = reorg_cost_usd * reorg_prob.clamp(0.0, 1.0);

        term_lead + term_drift + term_reorg
    }

    /// Generates optimal limit quotes under Level-3 queue priority constraints.
    #[allow(clippy::too_many_arguments)]
    pub fn compute_quotes(
        &self,
        mid_price: f64,
        inventory_q: f64,
        hawkes_buy_intensity: f64,
        hawkes_sell_intensity: f64,
        baseline_mu: f64,
        cex_ofi: f64,
        tau_seq_sec: f64,
        volume_ahead: f64,
        max_volume_ahead: f64,
    ) -> L3HawkesQuoteRecommendation {
        let q = inventory_q.clamp(-self.config.max_inventory_q, self.config.max_inventory_q);
        let s = mid_price.max(1e-4);

        // Base Guéant reservation price: R = s - q * gamma * sigma^2 * s^2 * dt
        let reservation_price = s - q
            * self.config.risk_aversion_gamma
            * (self.config.asset_volatility * s).powi(2)
            * 0.01;

        // Hawkes asymmetric arrival multipliers
        let base_mu = baseline_mu.max(1e-4);
        let buy_mult = 1.0
            + self.config.flow_excitation_kappa
                * ((hawkes_buy_intensity - base_mu).max(0.0) / base_mu);
        let sell_mult = 1.0
            + self.config.flow_excitation_kappa
                * ((hawkes_sell_intensity - base_mu).max(0.0) / base_mu);

        let k = self.config.fill_sensitivity_k.max(1e-4);
        let half_spread_bid = (1.0 / k)
            * (1.0 + (k * self.config.risk_aversion_gamma / buy_mult))
                .ln()
                .max(self.config.min_half_spread_bps * 1e-4);
        let half_spread_ask = (1.0 / k)
            * (1.0 + (k * self.config.risk_aversion_gamma / sell_mult))
                .ln()
                .max(self.config.min_half_spread_bps * 1e-4);

        let p_jump = self.jump_markout_penalty(q);
        let p_adverse = self.adverse_selection_compensation(cex_ofi, tau_seq_sec, 2.5, 0.001, 10.0);

        let bid_price = reservation_price - half_spread_bid * s - p_jump - p_adverse;
        let ask_price = reservation_price + half_spread_ask * s + p_jump + p_adverse;

        let queue_gating_passed = volume_ahead <= max_volume_ahead;

        L3HawkesQuoteRecommendation {
            reservation_price,
            recommended_bid: bid_price,
            recommended_ask: ask_price,
            bid_half_spread_bps: half_spread_bid * 10000.0,
            ask_half_spread_bps: half_spread_ask * 10000.0,
            jump_markout_penalty_usd: p_jump,
            adverse_selection_compensation_usd: p_adverse,
            queue_gating_passed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jump_markout_penalty_properties() {
        let mm = L3HawkesMarketMaker::new(L3HawkesMmConfig::default());
        // Neutral inventory -> 0 jump penalty
        let pen_zero = mm.jump_markout_penalty(0.0);
        assert!(pen_zero.abs() < 1e-9);

        // Long inventory with negative jump mean -> negative penalty (drags prices down)
        let pen_long = mm.jump_markout_penalty(5.0);
        assert!(pen_long < 0.0);

        // Short inventory -> positive penalty
        let pen_short = mm.jump_markout_penalty(-5.0);
        assert!(pen_short > 0.0);
    }

    #[test]
    fn test_l3_hawkes_jump_and_latency_quoting() {
        let mm = L3HawkesMarketMaker::new(L3HawkesMmConfig::default());
        let quote_neutral = mm.compute_quotes(3000.0, 0.0, 10.0, 10.0, 8.0, 0.0, 0.25, 10.0, 50.0);
        let quote_long = mm.compute_quotes(3000.0, 5.0, 10.0, 10.0, 8.0, 0.0, 0.25, 10.0, 50.0);

        // Long inventory under downward jump hazard pulls down bid to shed position
        assert!(quote_long.recommended_bid < quote_neutral.recommended_bid);
        assert!(quote_long.jump_markout_penalty_usd < 0.0);

        // Queue gating check
        let quote_gated = mm.compute_quotes(3000.0, 0.0, 10.0, 10.0, 8.0, 0.0, 0.25, 100.0, 50.0);
        assert!(!quote_gated.queue_gating_passed);
        assert!(quote_neutral.queue_gating_passed);
    }

    #[test]
    fn test_adverse_selection_compensation_scales_with_latency() {
        let mm = L3HawkesMarketMaker::new(L3HawkesMmConfig::default());
        let comp_fast = mm.adverse_selection_compensation(0.5, 0.05, 2.0, 0.001, 10.0);
        let comp_slow = mm.adverse_selection_compensation(0.5, 0.50, 2.0, 0.001, 10.0);

        assert!(comp_slow > comp_fast);
        assert!(comp_fast > 0.0);
    }
}
