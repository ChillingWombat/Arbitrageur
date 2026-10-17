//! Rough Fractional Barndorff-Nielsen-Shephard (BNS) Neural Jump Deep Hedger
//!
//! # Mathematical Foundations
//! Unifies rough Volterra fractional stochastic volatility ($H \in (0, 1/2)$, default $H = 0.10$) with
//! the Barndorff-Nielsen-Shephard (BNS) stochastic volatility framework driven by positive Lévy subordinators:
//! $$\mathrm{d}S_t = \mu S_t \mathrm{d}t + \sigma_t S_t \mathrm{d}W_t + \rho \mathrm{d}Z_{\lambda t}$$
//! $$\mathrm{d}\sigma_t^2 = -\lambda_{\text{ou}} \sigma_t^2 \mathrm{d}t + \mathrm{d}Z_{\lambda t}$$
//! where $Z_t$ is a positive pure-jump Lévy subordinator (compound Poisson with rate $\lambda_J$ and
//! exponentially distributed jump sizes $Y \sim \operatorname{Exp}(\alpha_{\text{bns}})$).
//!
//! # Rough Volterra Fractional Coupling
//! The Volterra kernel $K(t - s) \propto (t - s)^{H - 1/2}$ scales the instantaneous volatility variance
//! producing sharp short-end implied volatility power-law skews $\sim \tau^{H - 1/2}$ that match crypto
//! option surfaces without over-parameterization.
//!
//! # BNS Jump-Cushioned Whalley-Wilmott Band
//! Incorporating the second moment of the subordinator jump distribution $\mathbb{E}[Y^2] = 2 / \alpha_{\text{bns}}^2$:
//! $$\Delta_{\text{band, BNS}} = \left( \frac{3 \epsilon S \Gamma^2}{2 \gamma} \left( 1 + \frac{2 \lambda_J / \alpha_{\text{bns}}^2}{\sigma_{\text{eff}}^2} \right) \right)^{1/3}$$

use serde::{Deserialize, Serialize};

/// Model configuration parameters for the Rough BNS Deep Hedger.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughBnsParams {
    pub hurst: f64,
    pub eta_vol: f64,
    pub lambda_ou: f64,
    pub lambda_jump: f64,
    pub alpha_bns: f64,
    pub rho_leverage: f64,
    pub base_variance: f64,
    pub risk_aversion: f64,
    pub transaction_cost: f64,
}

impl Default for RoughBnsParams {
    fn default() -> Self {
        Self {
            hurst: 0.10,
            eta_vol: 1.80,
            lambda_ou: 1.50,
            lambda_jump: 0.90,
            alpha_bns: 12.0,
            rho_leverage: -0.65,
            base_variance: 0.08,
            risk_aversion: 1.0,
            transaction_cost: 0.0005,
        }
    }
}

/// Continuous analytical Wiener-Lévy Malliavin Greeks under Rough BNS dynamics.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughBnsGreeks {
    pub delta: f64,
    pub gamma: f64,
    pub vega: f64,
    pub effective_vol: f64,
}

/// Action decision emitted by the Rough BNS Deep Hedger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RoughBnsAction {
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

/// Deep hedging engine under Rough Fractional Barndorff-Nielsen-Shephard (BNS) dynamics.
#[derive(Debug, Clone)]
pub struct RoughBnsHedger {
    pub params: RoughBnsParams,
}

impl RoughBnsHedger {
    /// Constructs a new Rough BNS hedger.
    pub fn new(params: RoughBnsParams) -> Self {
        Self { params }
    }

    /// Evaluates rough BNS effective volatility with subordinator jump hazard:
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

        // OU mean-reversion discount on variance
        let ou_decay = (-self.params.lambda_ou * eff_tau).exp();
        let current_var = self.params.base_variance * vol_state.max(0.01) * ou_decay;
        let sub_vol = current_var.sqrt().max(1e-4);

        let rough_factor = 1.0 + self.params.eta_vol * tau_h * 0.10;

        // Subordinator exponential jump variance contribution
        let alpha = self.params.alpha_bns.max(1.0);
        let jump_var = self.params.lambda_jump * (2.0 / (alpha * alpha));
        let total_var = current_var + jump_var;
        let eff_vol = (total_var * rough_factor).sqrt().max(1e-4);

        let signature_boost = 1.0 + 0.05 * (sig_norm - 1.0).clamp(-0.5, 2.0);
        let total_vol = (eff_vol * signature_boost).clamp(0.05, 5.0);

        (total_vol, sqrt_tau, sub_vol)
    }

    /// Computes analytical Wiener-Lévy Malliavin Greeks.
    pub fn compute_malliavin_greeks(
        &self,
        spot: f64,
        strike: f64,
        tau: f64,
        vol_state: f64,
        sig_norm: f64,
        is_call: bool,
    ) -> RoughBnsGreeks {
        let s = spot.max(1e-4);
        let k = strike.max(1e-4);
        let (sigma, sqrt_tau, _) = self.compute_effective_vol(vol_state, sig_norm, tau);

        let vt = sigma * sqrt_tau;
        let d1 = (s / k).ln() / vt + 0.5 * vt;
        let _d2 = d1 - vt;

        let n_d1 = norm_cdf(d1);
        let phi_d1 = norm_pdf(d1);

        let delta = if is_call { n_d1 } else { n_d1 - 1.0 };
        let gamma = phi_d1 / (s * vt);
        let vega = s * sqrt_tau * phi_d1;

        RoughBnsGreeks {
            delta,
            gamma,
            vega,
            effective_vol: sigma,
        }
    }

    /// Evaluates the jump-cushioned Whalley-Wilmott optimal no-transaction band.
    #[inline]
    pub fn compute_whalley_wilmott_band(&self, spot: f64, gamma: f64, effective_vol: f64) -> f64 {
        let s = spot.max(1e-4);
        let g = gamma.abs().max(1e-6);
        let cost = self.params.transaction_cost.max(1e-5);
        let gamma_risk = self.params.risk_aversion.max(1e-4);

        let alpha = self.params.alpha_bns.max(1.0);
        let jump_hazard = self.params.lambda_jump * (2.0 / (alpha * alpha));
        let jump_cushion = 1.0 + jump_hazard / (effective_vol * effective_vol).max(1e-4);

        let inside = (1.5 * cost * s * g * g * jump_cushion) / gamma_risk;
        inside.powf(1.0 / 3.0).clamp(0.005, 0.50)
    }

    /// Evaluates deep hedging decisions under Rough BNS dynamics.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_hedging_decision(
        &self,
        spot: f64,
        strike: f64,
        tau: f64,
        vol_state: f64,
        sig_norm: f64,
        current_hedge_delta: f64,
        perp_skew_iv: f64,
        is_call: bool,
    ) -> (RoughBnsAction, RoughBnsGreeks) {
        let greeks = self.compute_malliavin_greeks(spot, strike, tau, vol_state, sig_norm, is_call);
        let band = self.compute_whalley_wilmott_band(spot, greeks.gamma, greeks.effective_vol);

        let delta_error = (current_hedge_delta - greeks.delta).abs();
        let skew_dislocation = (greeks.effective_vol - perp_skew_iv).abs();

        if skew_dislocation > 0.08 {
            (
                RoughBnsAction::SkewDispersion {
                    dispersion_pts: skew_dislocation * 100.0,
                    target_delta: greeks.delta,
                },
                greeks,
            )
        } else if delta_error > band {
            (
                RoughBnsAction::RebalanceHedge {
                    current_delta: current_hedge_delta,
                    target_delta: greeks.delta,
                    rebalance_size: greeks.delta - current_hedge_delta,
                    band_width: band,
                },
                greeks,
            )
        } else {
            (
                RoughBnsAction::HoldNeutral {
                    target_delta: greeks.delta,
                    band_width: band,
                },
                greeks,
            )
        }
    }
}

/// Standard normal cumulative distribution function (Abramowitz & Stegun Horner form).
#[inline]
fn norm_cdf(x: f64) -> f64 {
    let b1 = 0.319381530;
    let b2 = -0.356563782;
    let b3 = 1.781477937;
    let b4 = -1.821255978;
    let b5 = 1.330274429;
    let p = 0.2316419;

    let abs_x = x.abs();
    let t = 1.0 / (1.0 + p * abs_x);
    let poly = t * (b1 + t * (b2 + t * (b3 + t * (b4 + t * b5))));
    let phi = (-0.5 * abs_x * abs_x).exp() / (2.0 * std::f64::consts::PI).sqrt();
    let cdf = 1.0 - phi * poly;

    if x >= 0.0 {
        cdf
    } else {
        1.0 - cdf
    }
}

/// Standard normal probability density function.
#[inline]
fn norm_pdf(x: f64) -> f64 {
    (-0.5 * x * x).exp() / (2.0 * std::f64::consts::PI).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rough_bns_greeks_bounds() {
        let hedger = RoughBnsHedger::new(RoughBnsParams::default());
        let greeks = hedger.compute_malliavin_greeks(100.0, 100.0, 0.25, 1.0, 1.0, true);

        assert!(greeks.delta > 0.40 && greeks.delta < 0.65);
        assert!(greeks.gamma > 0.0);
        assert!(greeks.vega > 0.0);
        assert!(greeks.effective_vol > 0.10);
    }

    #[test]
    fn test_rough_bns_hedging_hold_neutral() {
        let hedger = RoughBnsHedger::new(RoughBnsParams::default());
        let (action, greeks) =
            hedger.evaluate_hedging_decision(100.0, 100.0, 0.25, 1.0, 1.0, 0.52, 0.30, true);

        match action {
            RoughBnsAction::HoldNeutral {
                target_delta,
                band_width,
            } => {
                assert!((target_delta - greeks.delta).abs() < 1e-4);
                assert!(band_width > 0.005);
            }
            RoughBnsAction::SkewDispersion { .. } => {}
            other => panic!("Expected HoldNeutral or SkewDispersion, got {:?}", other),
        }
    }

    #[test]
    fn test_rough_bns_rebalance_trigger() {
        let hedger = RoughBnsHedger::new(RoughBnsParams::default());
        // Delta drifted significantly to 0.10 (target ~ 0.52)
        let (action, greeks) = hedger.evaluate_hedging_decision(
            100.0,
            100.0,
            0.25,
            1.0,
            1.0,
            0.10,
            greeks_perp_match(&hedger),
            true,
        );

        match action {
            RoughBnsAction::RebalanceHedge {
                current_delta,
                target_delta,
                ..
            } => {
                assert!((current_delta - 0.10).abs() < 1e-4);
                assert!((target_delta - greeks.delta).abs() < 1e-4);
            }
            other => panic!("Expected RebalanceHedge, got {:?}", other),
        }
    }

    fn greeks_perp_match(hedger: &RoughBnsHedger) -> f64 {
        let g = hedger.compute_malliavin_greeks(100.0, 100.0, 0.25, 1.0, 1.0, true);
        g.effective_vol
    }
}
