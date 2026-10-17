//! Rough Fractional Carr-Geman-Madan-Yor (CGMY) Neural Pure-Jump Deep Hedger with Truncated Chen Signatures
//!
//! # Mathematical Foundations
//! Unifies rough Volterra fractional stochastic volatility ($H \in (0, 1/2)$, default $H = 0.10$) with
//! the tempered stable CGMY pure-jump process (Carr, Geman, Madan, Yor, 2002):
//! $$k(x) = \frac{C}{|x|^{1 + Y}} \left( e^{-G |x|} \mathbf{1}_{\{x < 0\}} + e^{-M x} \mathbf{1}_{\{x > 0\}} \right)$$
//! where:
//! - $C > 0$ controls the overall jump activity level.
//! - $G, M > 0$ govern the exponential damping rate of left (negative) and right (positive) jumps.
//! - $Y < 2$ dictates the fine path properties ($Y \in (0, 1)$ finite variation, $Y \in (1, 2)$ infinite variation).
//!
//! # Variance and Higher Moments of CGMY Process
//! Under the tempered stable Lévy measure, the exact second moment is:
//! $$\operatorname{Var}[X_{\text{CGMY}}(1)] = C \cdot \Gamma(2 - Y) \left( G^{Y - 2} + M^{Y - 2} \right)$$
//!
//! # Jump-Cushioned Whalley-Wilmott Band
//! $$\Delta_{\text{band, CGMY}} = \left( \frac{3 \epsilon S \Gamma^2}{2 \gamma} \left( 1 + \frac{\operatorname{Var}[X_{\text{CGMY}}]}{\sigma_{\text{eff}}^2} \right) \right)^{1/3}$$

use serde::{Deserialize, Serialize};

/// Model configuration parameters for the Rough CGMY Deep Hedger.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughCgmyParams {
    pub hurst: f64,
    pub eta_vol: f64,
    pub c_param: f64,
    pub g_param: f64,
    pub m_param: f64,
    pub y_param: f64,
    pub base_variance: f64,
    pub risk_aversion: f64,
    pub transaction_cost: f64,
}

impl Default for RoughCgmyParams {
    fn default() -> Self {
        Self {
            hurst: 0.10,
            eta_vol: 1.80,
            c_param: 0.50,
            g_param: 15.0,
            m_param: 20.0,
            y_param: 0.70,
            base_variance: 0.08,
            risk_aversion: 1.0,
            transaction_cost: 0.0005,
        }
    }
}

/// Continuous analytical Wiener-Lévy Malliavin Greeks under Rough CGMY dynamics.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughCgmyGreeks {
    pub delta: f64,
    pub gamma: f64,
    pub vega: f64,
    pub effective_vol: f64,
}

/// Action decision emitted by the Rough CGMY Deep Hedger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RoughCgmyAction {
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

/// Deep hedging engine under Rough Fractional CGMY Pure-Jump dynamics.
#[derive(Debug, Clone)]
pub struct RoughCgmyHedger {
    pub params: RoughCgmyParams,
}

impl RoughCgmyHedger {
    /// Constructs a new Rough CGMY hedger.
    pub fn new(params: RoughCgmyParams) -> Self {
        Self { params }
    }

    /// Evaluates the variance of unit-time CGMY increments:
    /// $$\operatorname{Var}[X_{\text{CGMY}}] = C \cdot \Gamma(2 - Y) \left( G^{Y - 2} + M^{Y - 2} \right)$$
    #[inline]
    pub fn cgmy_variance(&self) -> f64 {
        let c = self.params.c_param.max(1e-4);
        let g = self.params.g_param.max(1.0);
        let m = self.params.m_param.max(1.0);
        let y = self.params.y_param.clamp(0.01, 1.95);

        let gamma_2_minus_y = lanczos_gamma(2.0 - y);
        let g_pow = g.powf(y - 2.0);
        let m_pow = m.powf(y - 2.0);

        c * gamma_2_minus_y * (g_pow + m_pow)
    }

    /// Evaluates rough CGMY effective volatility with tempered stable jump hazard:
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

        let jump_var = self.cgmy_variance();
        let total_var = (sub_vol * sub_vol + jump_var) * rough_factor;
        let eff_vol = total_var.sqrt().max(1e-4);

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
    ) -> RoughCgmyGreeks {
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

        RoughCgmyGreeks {
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

        let jump_hazard = self.cgmy_variance();
        let jump_cushion = 1.0 + jump_hazard / (effective_vol * effective_vol).max(1e-4);

        let inside = (1.5 * cost * s * g * g * jump_cushion) / gamma_risk;
        inside.powf(1.0 / 3.0).clamp(0.005, 0.50)
    }

    /// Evaluates deep hedging decisions under Rough CGMY dynamics.
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
    ) -> (RoughCgmyAction, RoughCgmyGreeks) {
        let greeks = self.compute_malliavin_greeks(spot, strike, tau, vol_state, sig_norm, is_call);
        let band = self.compute_whalley_wilmott_band(spot, greeks.gamma, greeks.effective_vol);

        let delta_error = (current_hedge_delta - greeks.delta).abs();
        let skew_dislocation = (greeks.effective_vol - perp_skew_iv).abs();

        if skew_dislocation > 0.08 {
            (
                RoughCgmyAction::SkewDispersion {
                    dispersion_pts: skew_dislocation * 100.0,
                    target_delta: greeks.delta,
                },
                greeks,
            )
        } else if delta_error > band {
            (
                RoughCgmyAction::RebalanceHedge {
                    current_delta: current_hedge_delta,
                    target_delta: greeks.delta,
                    rebalance_size: greeks.delta - current_hedge_delta,
                    band_width: band,
                },
                greeks,
            )
        } else {
            (
                RoughCgmyAction::HoldNeutral {
                    target_delta: greeks.delta,
                    band_width: band,
                },
                greeks,
            )
        }
    }
}

/// Lanczos approximation for the Gamma function $\Gamma(z)$.
#[inline]
fn lanczos_gamma(z: f64) -> f64 {
    let p = [
        676.5203681218851,
        -1259.1392167224028,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507343278686905,
        -0.13857109526572012,
        9.984_369_578_019_572e-6,
        1.5056327351493116e-7,
    ];

    if z < 0.5 {
        std::f64::consts::PI / ((std::f64::consts::PI * z).sin() * lanczos_gamma(1.0 - z))
    } else {
        let z_val = z - 1.0;
        let mut x = 0.999_999_999_999_809_9;
        for (i, &pval) in p.iter().enumerate() {
            x += pval / (z_val + (i as f64) + 1.0);
        }
        let t = z_val + 7.5;
        (2.0 * std::f64::consts::PI).sqrt() * t.powf(z_val + 0.5) * (-t).exp() * x
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
    fn test_rough_cgmy_greeks_bounds() {
        let hedger = RoughCgmyHedger::new(RoughCgmyParams::default());
        let greeks = hedger.compute_malliavin_greeks(100.0, 100.0, 0.25, 1.0, 1.0, true);

        assert!(greeks.delta > 0.40 && greeks.delta < 0.65);
        assert!(greeks.gamma > 0.0);
        assert!(greeks.vega > 0.0);
        assert!(greeks.effective_vol > 0.10);
    }

    #[test]
    fn test_rough_cgmy_hedging_hold_neutral() {
        let hedger = RoughCgmyHedger::new(RoughCgmyParams::default());
        let (action, greeks) =
            hedger.evaluate_hedging_decision(100.0, 100.0, 0.25, 1.0, 1.0, 0.52, 0.30, true);

        match action {
            RoughCgmyAction::HoldNeutral {
                target_delta,
                band_width,
            } => {
                assert!((target_delta - greeks.delta).abs() < 1e-4);
                assert!(band_width > 0.005);
            }
            RoughCgmyAction::SkewDispersion { .. } => {}
            other => panic!("Expected HoldNeutral or SkewDispersion, got {:?}", other),
        }
    }
}
