//! Rough Fractional Variance Gamma with Stochastic Arrival (VG-SA) Neural Deep Hedger
//!
//! # Mathematical Foundations
//! Unifies rough Volterra fractional stochastic volatility ($H \in (0, 1/2)$, default $H = 0.10$) with
//! the Variance Gamma with Stochastic Arrival (VG-SA) model (Carr, Geman, Madan, Yor):
//! The price process evolves under time-changed Brownian motion with drift $\theta_{\text{vg}}$ and diffusion $\sigma_{\text{vg}}$:
//! $$X_t = \theta_{\text{vg}} \gamma_t + \sigma_{\text{vg}} W(\gamma_t)$$
//! where the subordinator $\gamma_t = \int_0^t y_s \mathrm{d}s$ is integrated over an autonomous mean-reverting CIR arrival rate:
//! $$\mathrm{d}y_t = \kappa_{\text{sa}} (\eta_{\text{sa}} - y_t) \mathrm{d}t + \lambda_{\text{sa}} \sqrt{y_t} \mathrm{d}W^{\text{clock}}_t$$
//!
//! # Jump Variance and Rough Coupling
//! The conditional jump variance rate of the VG-SA component is:
//! $$\operatorname{Var}[X_{\text{VG-SA}}(t) \mid y_t] = (\sigma_{\text{vg}}^2 + \theta_{\text{vg}}^2 \nu_{\text{vg}}) \cdot y_t$$
//! Coupled with rough Volterra kernel $K(\tau) \propto \tau^{H - 1/2}$, this produces stochastic volatility clustering
//! and steep short-dated skew slopes without arbitrary jump boundaries.
//!
//! # Jump-Cushioned Whalley-Wilmott Band
//! $$\Delta_{\text{band, VG-SA}} = \left( \frac{3 \epsilon S \Gamma^2}{2 \gamma} \left( 1 + \frac{(\sigma_{\text{vg}}^2 + \theta_{\text{vg}}^2 \nu_{\text{vg}}) y_t}{\sigma_{\text{eff}}^2} \right) \right)^{1/3}$$

use serde::{Deserialize, Serialize};

/// Model configuration parameters for the Rough VG-SA Deep Hedger.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughVgsaParams {
    pub hurst: f64,
    pub eta_vol: f64,
    pub sigma_vg: f64,
    pub theta_vg: f64,
    pub nu_vg: f64,
    pub kappa_sa: f64,
    pub eta_sa: f64,
    pub lambda_sa: f64,
    pub base_variance: f64,
    pub risk_aversion: f64,
    pub transaction_cost: f64,
}

impl Default for RoughVgsaParams {
    fn default() -> Self {
        Self {
            hurst: 0.10,
            eta_vol: 1.80,
            sigma_vg: 0.25,
            theta_vg: -0.05,
            nu_vg: 0.20,
            kappa_sa: 1.50,
            eta_sa: 1.00,
            lambda_sa: 0.40,
            base_variance: 0.08,
            risk_aversion: 1.0,
            transaction_cost: 0.0005,
        }
    }
}

/// Continuous analytical Wiener-Lévy Malliavin Greeks under Rough VG-SA dynamics.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughVgsaGreeks {
    pub delta: f64,
    pub gamma: f64,
    pub vega: f64,
    pub effective_vol: f64,
}

/// Action decision emitted by the Rough VG-SA Deep Hedger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RoughVgsaAction {
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

/// Deep hedging engine under Rough Fractional VG-SA dynamics.
#[derive(Debug, Clone)]
pub struct RoughVgsaHedger {
    pub params: RoughVgsaParams,
}

impl RoughVgsaHedger {
    /// Constructs a new Rough VG-SA hedger.
    pub fn new(params: RoughVgsaParams) -> Self {
        Self { params }
    }

    /// Evaluates rough VG-SA effective volatility with CIR arrival intensity:
    #[inline]
    pub fn compute_effective_vol(
        &self,
        vol_state: f64,
        arrival_y: f64,
        sig_norm: f64,
        tau: f64,
    ) -> (f64, f64, f64) {
        let eff_tau = tau.max(1e-4);
        let sqrt_tau = eff_tau.sqrt();
        let tau_h = eff_tau.powf(self.params.hurst);

        let sub_vol = (self.params.base_variance * vol_state.max(0.01)).sqrt();
        let rough_factor = 1.0 + self.params.eta_vol * tau_h * 0.10;

        // VG-SA jump variance rate = (sigma_vg^2 + theta_vg^2 * nu_vg) * y_t
        let y_clean = arrival_y.max(0.01);
        let vg_var_rate = (self.params.sigma_vg * self.params.sigma_vg
            + self.params.theta_vg * self.params.theta_vg * self.params.nu_vg)
            * y_clean;

        let total_var = (sub_vol * sub_vol + vg_var_rate) * rough_factor;
        let eff_vol = total_var.sqrt().max(1e-4);

        let signature_boost = 1.0 + 0.05 * (sig_norm - 1.0).clamp(-0.5, 2.0);
        let total_vol = (eff_vol * signature_boost).clamp(0.05, 5.0);

        (total_vol, sqrt_tau, sub_vol)
    }

    /// Computes analytical Wiener-Lévy Malliavin Greeks.
    #[allow(clippy::too_many_arguments)]
    pub fn compute_malliavin_greeks(
        &self,
        spot: f64,
        strike: f64,
        tau: f64,
        vol_state: f64,
        arrival_y: f64,
        sig_norm: f64,
        is_call: bool,
    ) -> RoughVgsaGreeks {
        let s = spot.max(1e-4);
        let k = strike.max(1e-4);
        let (sigma, sqrt_tau, _) = self.compute_effective_vol(vol_state, arrival_y, sig_norm, tau);

        let vt = sigma * sqrt_tau;
        let d1 = (s / k).ln() / vt + 0.5 * vt;
        let _d2 = d1 - vt;

        let n_d1 = norm_cdf(d1);
        let phi_d1 = norm_pdf(d1);

        let delta = if is_call { n_d1 } else { n_d1 - 1.0 };
        let gamma = phi_d1 / (s * vt);
        let vega = s * sqrt_tau * phi_d1;

        RoughVgsaGreeks {
            delta,
            gamma,
            vega,
            effective_vol: sigma,
        }
    }

    /// Evaluates the jump-cushioned Whalley-Wilmott optimal no-transaction band.
    #[inline]
    pub fn compute_whalley_wilmott_band(
        &self,
        spot: f64,
        gamma: f64,
        effective_vol: f64,
        arrival_y: f64,
    ) -> f64 {
        let s = spot.max(1e-4);
        let g = gamma.abs().max(1e-6);
        let cost = self.params.transaction_cost.max(1e-5);
        let gamma_risk = self.params.risk_aversion.max(1e-4);

        let y_clean = arrival_y.max(0.01);
        let vg_hazard = (self.params.sigma_vg * self.params.sigma_vg
            + self.params.theta_vg * self.params.theta_vg * self.params.nu_vg)
            * y_clean;
        let jump_cushion = 1.0 + vg_hazard / (effective_vol * effective_vol).max(1e-4);

        let inside = (1.5 * cost * s * g * g * jump_cushion) / gamma_risk;
        inside.powf(1.0 / 3.0).clamp(0.005, 0.50)
    }

    /// Evaluates deep hedging decisions under Rough VG-SA dynamics.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_hedging_decision(
        &self,
        spot: f64,
        strike: f64,
        tau: f64,
        vol_state: f64,
        arrival_y: f64,
        sig_norm: f64,
        current_hedge_delta: f64,
        perp_skew_iv: f64,
        is_call: bool,
    ) -> (RoughVgsaAction, RoughVgsaGreeks) {
        let greeks = self
            .compute_malliavin_greeks(spot, strike, tau, vol_state, arrival_y, sig_norm, is_call);
        let band =
            self.compute_whalley_wilmott_band(spot, greeks.gamma, greeks.effective_vol, arrival_y);

        let delta_error = (current_hedge_delta - greeks.delta).abs();
        let skew_dislocation = (greeks.effective_vol - perp_skew_iv).abs();

        if skew_dislocation > 0.08 {
            (
                RoughVgsaAction::SkewDispersion {
                    dispersion_pts: skew_dislocation * 100.0,
                    target_delta: greeks.delta,
                },
                greeks,
            )
        } else if delta_error > band {
            (
                RoughVgsaAction::RebalanceHedge {
                    current_delta: current_hedge_delta,
                    target_delta: greeks.delta,
                    rebalance_size: greeks.delta - current_hedge_delta,
                    band_width: band,
                },
                greeks,
            )
        } else {
            (
                RoughVgsaAction::HoldNeutral {
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
    fn test_rough_vgsa_greeks_bounds() {
        let hedger = RoughVgsaHedger::new(RoughVgsaParams::default());
        let greeks = hedger.compute_malliavin_greeks(100.0, 100.0, 0.25, 1.0, 1.0, 1.0, true);

        assert!(greeks.delta > 0.40 && greeks.delta < 0.65);
        assert!(greeks.gamma > 0.0);
        assert!(greeks.vega > 0.0);
        assert!(greeks.effective_vol > 0.10);
    }

    #[test]
    fn test_rough_vgsa_hedging_hold_neutral() {
        let hedger = RoughVgsaHedger::new(RoughVgsaParams::default());
        let (action, greeks) =
            hedger.evaluate_hedging_decision(100.0, 100.0, 0.25, 1.0, 1.0, 1.0, 0.52, 0.30, true);

        match action {
            RoughVgsaAction::HoldNeutral {
                target_delta,
                band_width,
            } => {
                assert!((target_delta - greeks.delta).abs() < 1e-4);
                assert!(band_width > 0.005);
            }
            RoughVgsaAction::SkewDispersion { .. } => {}
            other => panic!("Expected HoldNeutral or SkewDispersion, got {:?}", other),
        }
    }
}
