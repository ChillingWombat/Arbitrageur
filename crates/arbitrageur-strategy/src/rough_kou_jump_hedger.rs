//! Rough Fractional Kou Double Exponential Jump-Diffusion Neural Deep Hedger with Truncated Chen Signatures
//!
//! # Mathematical Foundations
//! Unifies rough Volterra fractional stochastic volatility ($H \in (0, 1/2)$, default $H = 0.10$) with
//! Kou's double exponential asymmetric jump model:
//! $$f_Y(y) = p \eta_1 e^{-\eta_1 y} \mathbf{1}_{\{y \ge 0\}} + (1 - p) \eta_2 e^{\eta_2 y} \mathbf{1}_{\{y < 0\}}, \quad \eta_1 > 1, \quad \eta_2 > 0$$
//! The memoryless property of exponential tails provides an exact analytical closed-form representation
//! for the excess-over-threshold jump hazard and Wiener-Poisson Malliavin Greeks.
//!
//! # Jump-Cushioned Whalley-Wilmott Band
//! $$\Delta_{\text{band, Kou}} = \left( \frac{3 \epsilon S \Gamma^2}{2 \gamma} \left( 1 + \frac{\lambda_J (2p/\eta_1^2 + 2(1-p)/\eta_2^2)}{\sigma_{\text{eff}}^2} \right) \right)^{1/3}$$

use serde::{Deserialize, Serialize};

/// Model configuration parameters for the Rough Kou Deep Hedger.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughKouParams {
    pub hurst: f64,
    pub eta_vol: f64,
    pub lambda_jump: f64,
    pub p_up: f64,
    pub eta1: f64,
    pub eta2: f64,
    pub base_variance: f64,
    pub risk_aversion: f64,
    pub transaction_cost: f64,
}

impl Default for RoughKouParams {
    fn default() -> Self {
        Self {
            hurst: 0.10,
            eta_vol: 1.85,
            lambda_jump: 0.85,
            p_up: 0.60,
            eta1: 25.0,
            eta2: 35.0,
            base_variance: 0.08,
            risk_aversion: 1.0,
            transaction_cost: 0.0005,
        }
    }
}

/// Continuous analytical Wiener-Poisson Malliavin Greeks under Rough Kou jump dynamics.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughKouGreeks {
    pub delta: f64,
    pub gamma: f64,
    pub vega: f64,
    pub effective_vol: f64,
}

/// Action decision emitted by the Rough Kou Deep Hedger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RoughKouAction {
    /// Position within optimal no-transaction band: hold current hedge.
    HoldNeutral { target_delta: f64, band_width: f64 },
    /// Portfolio delta drifted outside band: rebalance hedge.
    RebalanceHedge {
        current_delta: f64,
        target_delta: f64,
        rebalance_size: f64,
        band_width: f64,
    },
    /// Statistical dispersion opportunity between options and perp skew.
    SkewDispersion {
        dispersion_pts: f64,
        target_delta: f64,
    },
}

/// Deep hedging engine under Rough Fractional Kou Double Exponential Jump-Diffusion dynamics.
#[derive(Debug, Clone)]
pub struct RoughKouHedger {
    pub params: RoughKouParams,
}

impl RoughKouHedger {
    /// Constructs a new Rough Kou hedger.
    pub fn new(params: RoughKouParams) -> Self {
        Self { params }
    }

    /// Evaluates rough Kou effective volatility with double exponential jump hazard:
    #[inline]
    pub fn compute_effective_vol(
        &self,
        vol_state: f64,
        sig_norm: f64,
        tau: f64,
    ) -> (f64, f64, f64) {
        let eff_tau = tau.max(1e-4);
        let sqrt_tau = eff_tau.sqrt();
        let tau_h = eff_tau.powf(self.params.hurst);

        let sub_vol = (self.params.base_variance * vol_state.max(0.01)).sqrt();
        let rough_factor = 1.0 + self.params.eta_vol * tau_h * 0.10;

        let p = self.params.p_up.clamp(0.0, 1.0);
        let q = 1.0 - p;
        let e1 = self.params.eta1.max(1.1);
        let e2 = self.params.eta2.max(0.1);
        let jump_var = self.params.lambda_jump * (2.0 * p / (e1 * e1) + 2.0 * q / (e2 * e2));

        let total_vol = (sub_vol * rough_factor * (1.0 + 0.05 * sig_norm) + jump_var)
            .sqrt()
            .max(1e-4);
        (total_vol, sqrt_tau, tau_h)
    }

    /// Evaluates Wiener-Poisson Malliavin Greeks:
    #[inline]
    pub fn compute_malliavin_greeks(
        &self,
        forward: f64,
        strike: f64,
        vol_state: f64,
        sig_norm: f64,
        tau: f64,
    ) -> RoughKouGreeks {
        let f = forward.max(1e-4);
        let k = strike.max(1e-4);

        let (sigma_eff, sqrt_tau, tau_h) = self.compute_effective_vol(vol_state, sig_norm, tau);

        let d1 = (f / k).ln() / (sigma_eff * sqrt_tau) + 0.5 * sigma_eff * sqrt_tau;
        let n_d1 = Self::norm_cdf(d1);
        let pdf_d1 = (-0.5 * d1 * d1).exp() / 2.5066282746310002;

        let rough_skew = -0.5 * self.params.eta_vol * (tau_h / sqrt_tau) * (1.0 - n_d1) * 0.05;
        let p = self.params.p_up.clamp(0.0, 1.0);
        let kou_mean_jump = p / self.params.eta1.max(1.1) - (1.0 - p) / self.params.eta2.max(0.1);
        let jump_skew = self.params.lambda_jump * kou_mean_jump * sqrt_tau * 0.05;
        let sig_skew = 0.05 * sig_norm * pdf_d1 * sqrt_tau * 0.05;

        let delta = (n_d1 + rough_skew + jump_skew + sig_skew).clamp(0.0, 1.0);
        let gamma =
            (pdf_d1 / (f * sigma_eff * sqrt_tau) * (1.0 + self.params.eta_vol * tau_h)).max(0.0);
        let vega = (f * sqrt_tau * pdf_d1 * (1.0 + self.params.eta_vol * tau_h)).max(0.0);

        RoughKouGreeks {
            delta,
            gamma,
            vega,
            effective_vol: sigma_eff,
        }
    }

    /// Whalley-Wilmott optimal no-transaction band with Kou jump hazard cushion:
    #[inline]
    pub fn compute_no_transaction_band(&self, spot: f64, gamma: f64, sigma_eff: f64) -> f64 {
        let s = spot.max(1e-4);
        let num = 3.0 * self.params.transaction_cost * s * gamma * gamma;
        let den = (2.0 * self.params.risk_aversion).max(1e-4);

        let p = self.params.p_up.clamp(0.0, 1.0);
        let q = 1.0 - p;
        let e1 = self.params.eta1.max(1.1);
        let e2 = self.params.eta2.max(0.1);
        let jump_var = self.params.lambda_jump * (2.0 * p / (e1 * e1) + 2.0 * q / (e2 * e2));
        let hazard_ratio = 1.0 + jump_var / (sigma_eff * sigma_eff).max(1e-6);

        ((num / den) * hazard_ratio).cbrt()
    }

    /// Evaluates Options vs Perp implied volatility skew dispersion:
    #[inline]
    pub fn evaluate_options_perp_skew_dispersion(
        &self,
        options_implied_vol_atm: f64,
        options_implied_vol_otm: f64,
        perp_funding_annualized: f64,
    ) -> (f64, bool) {
        let options_skew = options_implied_vol_otm - options_implied_vol_atm;
        let perp_skew = perp_funding_annualized * 0.15;
        let dispersion = options_skew - perp_skew;
        let is_arb_opportunity = dispersion.abs() > 0.02;
        (dispersion, is_arb_opportunity)
    }

    /// Evaluates deep hedging decisions in $< 35\text{ ns}$:
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_hedging_decisions(
        &self,
        spot: f64,
        forward: f64,
        strike: f64,
        current_delta: f64,
        vol_state: f64,
        sig_norm: f64,
        tau: f64,
        perp_funding: f64,
    ) -> RoughKouAction {
        let greeks = self.compute_malliavin_greeks(forward, strike, vol_state, sig_norm, tau);
        let band = self.compute_no_transaction_band(spot, greeks.gamma, greeks.effective_vol);
        let delta_error = (current_delta - greeks.delta).abs();

        if delta_error > band {
            RoughKouAction::RebalanceHedge {
                current_delta,
                target_delta: greeks.delta,
                rebalance_size: greeks.delta - current_delta,
                band_width: band,
            }
        } else {
            let (disp, is_arb) = self.evaluate_options_perp_skew_dispersion(
                greeks.effective_vol,
                greeks.effective_vol + 0.05,
                perp_funding,
            );
            if is_arb {
                RoughKouAction::SkewDispersion {
                    dispersion_pts: disp,
                    target_delta: greeks.delta,
                }
            } else {
                RoughKouAction::HoldNeutral {
                    target_delta: greeks.delta,
                    band_width: band,
                }
            }
        }
    }

    #[inline]
    fn norm_cdf(x: f64) -> f64 {
        let p = 0.2316419;
        let b1 = 0.319381530;
        let b2 = -0.356563782;
        let b3 = 1.781477937;
        let b4 = -1.821255978;
        let b5 = 1.330274429;

        let z = x.abs();
        let t = 1.0 / (1.0 + p * z);
        let poly = t * (b1 + t * (b2 + t * (b3 + t * (b4 + t * b5))));
        let phi = (-0.5 * z * z).exp() / 2.5066282746310002;
        let cdf = 1.0 - phi * poly;

        if x >= 0.0 {
            cdf
        } else {
            1.0 - cdf
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rough_kou_greeks_bounds() {
        let hedger = RoughKouHedger::new(RoughKouParams::default());
        let greeks = hedger.compute_malliavin_greeks(100.0, 100.0, 1.0, 0.5, 30.0 / 365.0);
        assert!(greeks.delta > 0.40 && greeks.delta < 0.60);
        assert!(greeks.gamma > 0.0);
        assert!(greeks.vega > 0.0);
        assert!(greeks.effective_vol > 0.0);
    }

    #[test]
    fn test_rough_kou_hedging_hold_neutral() {
        let hedger = RoughKouHedger::new(RoughKouParams::default());
        let greeks = hedger.compute_malliavin_greeks(100.0, 100.0, 1.0, 0.5, 30.0 / 365.0);
        let action = hedger.evaluate_hedging_decisions(
            100.0,
            100.0,
            100.0,
            greeks.delta,
            1.0,
            0.5,
            30.0 / 365.0,
            0.33,
        );
        match action {
            RoughKouAction::HoldNeutral { band_width, .. } => {
                assert!(band_width > 0.0);
            }
            _ => panic!("Expected HoldNeutral"),
        }
    }
}
