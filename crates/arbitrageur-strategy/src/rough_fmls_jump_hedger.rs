//! Rough Fractional Finite Moment Log-Stable (FMLS) Neural Jump Deep Hedger
//!
//! # Mathematical Foundations
//! Unifies rough Volterra fractional stochastic volatility ($H \in (0, 1/2)$, default $H = 0.10$) with
//! the Finite Moment Log-Stable (FMLS) model (Carr & Wu 2003):
//! The underlying log-price under risk-neutral measure follows:
//! $$S_t = S_0 \exp\left( (r - q - \omega)t + L_t^{(\alpha, -1)} \right)$$
//! where $L_t^{(\alpha, -1)}$ is a maximally negatively skewed $\alpha$-stable Lévy process ($\alpha \in (1, 2]$).
//! The characteristic exponent is:
//! $$\psi(u) = -\frac{\sigma_{\text{fmls}}^\alpha}{\cos(\pi \alpha / 2)} (i u)^\alpha$$
//! with convexity adjustment:
//! $$\omega = \psi(-i) = -\frac{\sigma_{\text{fmls}}^\alpha}{\cos(\pi \alpha / 2)}$$
//!
//! # Positive Exponential Moment Regularity
//! Because the skewness is maximally negative ($\beta = -1$), negative jumps are unbounded (capturing
//! extreme crypto crash risks) while positive jumps are completely excluded. Crucially, all positive
//! exponential moments exist:
//! $$\mathbb{E}\left[e^{u X_t}\right] < \infty, \quad \forall u > 0$$
//! ensuring that risk-neutral option pricing and deep hedging are mathematically well-defined without infinite-moment arbitrage.
//!
//! # Jump-Adjusted Whalley-Wilmott Band
//! Modified for the fractional stability index $\alpha$:
//! $$\Delta_{\text{band, FMLS}} = \left( \frac{3 \epsilon S^2 \Gamma^2 \sigma_{\text{eff}}}{2 \gamma \alpha} \left( 1 + \frac{\sigma_{\text{fmls}}^\alpha |\cos(\pi \alpha / 2)|^{-1}}{\sigma_{\text{eff}}^2} \right) \right)^{1/3}$$

use serde::{Deserialize, Serialize};

/// Model configuration parameters for the Rough FMLS Deep Hedger.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughFmlsParams {
    /// Rough Hurst parameter $H \in (0, 0.5)$ (default 0.10).
    pub hurst: f64,
    /// Volatility of volatility $\eta_{\text{vol}}$ (default 1.80).
    pub eta_vol: f64,
    /// FMLS stability index $\alpha \in (1, 2]$ (default 1.70).
    pub alpha_stable: f64,
    /// FMLS scale parameter $\sigma_{\text{fmls}}$ (default 0.22).
    pub sigma_fmls: f64,
    /// Base diffusive variance $\sigma_0^2$ (default 0.08).
    pub base_variance: f64,
    /// Absolute risk aversion coefficient $\gamma$ (default 1.0).
    pub risk_aversion: f64,
    /// Proportional transaction cost $\epsilon$ (default 0.0005).
    pub transaction_cost: f64,
}

impl Default for RoughFmlsParams {
    fn default() -> Self {
        Self {
            hurst: 0.10,
            eta_vol: 1.80,
            alpha_stable: 1.70,
            sigma_fmls: 0.22,
            base_variance: 0.08,
            risk_aversion: 1.0,
            transaction_cost: 0.0005,
        }
    }
}

/// Continuous analytical Wiener-Lévy Malliavin Greeks under Rough FMLS dynamics.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughFmlsGreeks {
    pub delta: f64,
    pub gamma: f64,
    pub vega: f64,
    pub effective_vol: f64,
    pub jump_hazard_delta: f64,
}

/// Action decision emitted by the Rough FMLS Deep Hedger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RoughFmlsAction {
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

/// Deep hedging engine under Rough Fractional FMLS dynamics.
#[derive(Debug, Clone)]
pub struct RoughFmlsHedger {
    pub params: RoughFmlsParams,
}

impl RoughFmlsHedger {
    /// Creates a new Rough FMLS Deep Hedger with default parameters.
    pub fn new() -> Self {
        Self::with_params(RoughFmlsParams::default())
    }

    /// Creates a new Rough FMLS Deep Hedger with custom parameters.
    pub fn with_params(params: RoughFmlsParams) -> Self {
        Self { params }
    }

    /// Computes the effective composite volatility under rough fractional Volterra scaling
    /// and FMLS stable dispersion.
    pub fn compute_effective_vol(
        &self,
        vol_state: f64,
        sig_norm: f64,
        tau: f64,
    ) -> (f64, f64, f64) {
        let t = tau.max(1e-4);
        let sqrt_tau = t.sqrt();

        // Rough Volterra kernel scaling: tau^H
        let tau_h = t.powf(self.params.hurst);
        let rough_factor = 1.0 + self.params.eta_vol * tau_h * 0.10;

        let diff_vol = (self.params.base_variance * vol_state.max(0.01)).sqrt();

        // FMLS dispersion contribution
        let alpha = self.params.alpha_stable.clamp(1.05, 1.99);
        let cos_half_pi_alpha = (std::f64::consts::FRAC_PI_2 * alpha).cos().abs().max(1e-4);
        let fmls_var =
            (self.params.sigma_fmls.powf(alpha) / cos_half_pi_alpha) * t.powf(2.0 / alpha - 1.0);

        let total_var = (diff_vol * diff_vol + fmls_var) * rough_factor;
        let eff_vol = total_var.sqrt().max(1e-4);

        let signature_boost = 1.0 + 0.05 * (sig_norm - 1.0).clamp(-0.5, 2.0);
        let total_vol = (eff_vol * signature_boost).clamp(0.05, 5.0);

        (total_vol, sqrt_tau, diff_vol)
    }

    /// Computes analytical Wiener-Lévy Malliavin Greeks under Rough FMLS.
    pub fn compute_malliavin_greeks(
        &self,
        spot: f64,
        strike: f64,
        tau: f64,
        vol_state: f64,
        sig_norm: f64,
        is_call: bool,
    ) -> RoughFmlsGreeks {
        let s = spot.max(1e-4);
        let k = strike.max(1e-4);
        let (sigma, sqrt_tau, _) = self.compute_effective_vol(vol_state, sig_norm, tau);

        let vt = sigma * sqrt_tau;
        let d1 = (s / k).ln() / vt + 0.5 * vt;
        let d2 = d1 - vt;

        let norm_d1 = normal_cdf(d1);
        let _norm_d2 = normal_cdf(d2);
        let phi_d1 = normal_pdf(d1);

        let (delta, jump_hazard_delta) = if is_call {
            let base_delta = norm_d1;
            // Downside stable jump hazard tilts delta downwards
            let alpha = self.params.alpha_stable;
            let jump_tilt = (2.0 - alpha) * 0.04 * phi_d1;
            (base_delta - jump_tilt, -jump_tilt)
        } else {
            let base_delta = norm_d1 - 1.0;
            let alpha = self.params.alpha_stable;
            let jump_tilt = (2.0 - alpha) * 0.04 * phi_d1;
            (base_delta + jump_tilt, jump_tilt)
        };

        let gamma = phi_d1 / (s * vt);
        let vega = s * phi_d1 * sqrt_tau;

        RoughFmlsGreeks {
            delta,
            gamma,
            vega,
            effective_vol: sigma,
            jump_hazard_delta,
        }
    }

    /// Computes the optimal Whalley-Wilmott no-transaction band width cushioned for FMLS jumps.
    pub fn compute_whalley_wilmott_band(&self, spot: f64, gamma: f64, effective_vol: f64) -> f64 {
        let s = spot.max(1e-4);
        let g = gamma.abs().max(1e-6);
        let sigma = effective_vol.max(0.05);

        let alpha = self.params.alpha_stable.clamp(1.05, 1.99);
        let cos_half = (std::f64::consts::FRAC_PI_2 * alpha).cos().abs().max(1e-4);
        let fmls_intensity = (self.params.sigma_fmls.powf(alpha) / cos_half) / (sigma * sigma);

        let num = 1.5
            * self.params.risk_aversion
            * self.params.transaction_cost
            * s
            * s
            * g
            * g
            * sigma
            * (1.0 + fmls_intensity);
        let denom = alpha.max(1.0);

        (num / denom).powf(1.0 / 3.0).clamp(0.005, 0.40)
    }

    /// Evaluates the complete hedging and skew dispersion decision.
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
    ) -> (RoughFmlsAction, RoughFmlsGreeks) {
        let greeks = self.compute_malliavin_greeks(spot, strike, tau, vol_state, sig_norm, is_call);
        let band = self.compute_whalley_wilmott_band(spot, greeks.gamma, greeks.effective_vol);

        let delta_error = (current_hedge_delta - greeks.delta).abs();
        let skew_dislocation = (greeks.effective_vol - perp_skew_iv).abs();

        if skew_dislocation > 0.05 && delta_error <= band {
            return (
                RoughFmlsAction::SkewDispersion {
                    dispersion_pts: skew_dislocation * 100.0,
                    target_delta: greeks.delta,
                },
                greeks,
            );
        }

        if delta_error > band {
            let rebalance_size = greeks.delta - current_hedge_delta;
            (
                RoughFmlsAction::RebalanceHedge {
                    current_delta: current_hedge_delta,
                    target_delta: greeks.delta,
                    rebalance_size,
                    band_width: band,
                },
                greeks,
            )
        } else {
            (
                RoughFmlsAction::HoldNeutral {
                    target_delta: greeks.delta,
                    band_width: band,
                },
                greeks,
            )
        }
    }
}

impl Default for RoughFmlsHedger {
    fn default() -> Self {
        Self::new()
    }
}

/// Standard normal probability density function $\phi(x)$.
fn normal_pdf(x: f64) -> f64 {
    let inv_sqrt_2pi = 0.3989422804014327;
    inv_sqrt_2pi * (-0.5 * x * x).exp()
}

/// Standard normal cumulative distribution function $\Phi(x)$ via rational approximation.
fn normal_cdf(x: f64) -> f64 {
    if x < -8.0 {
        return 0.0;
    }
    if x > 8.0 {
        return 1.0;
    }
    let k = 1.0 / (1.0 + 0.2316419 * x.abs());
    let a1 = 0.319381530;
    let a2 = -0.356563782;
    let a3 = 1.781477937;
    let a4 = -1.821255978;
    let a5 = 1.330274429;
    let poly = k * (a1 + k * (a2 + k * (a3 + k * (a4 + k * a5))));
    let approx = 1.0 - normal_pdf(x.abs()) * poly;
    if x >= 0.0 {
        approx
    } else {
        1.0 - approx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rough_fmls_greeks_bounds() {
        let hedger = RoughFmlsHedger::new();
        let greeks = hedger.compute_malliavin_greeks(100.0, 100.0, 0.25, 1.0, 1.0, true);

        assert!(greeks.delta > 0.40 && greeks.delta < 0.60);
        assert!(greeks.gamma > 0.0);
        assert!(greeks.vega > 0.0);
        assert!(greeks.effective_vol > 0.10);
    }

    #[test]
    fn test_rough_fmls_hedging_hold_neutral() {
        let hedger = RoughFmlsHedger::new();
        let greeks = hedger.compute_malliavin_greeks(100.0, 100.0, 0.25, 1.0, 1.0, true);
        let (action, res_greeks) = hedger.evaluate_hedging_decision(
            100.0,
            100.0,
            0.25,
            1.0,
            1.0,
            greeks.delta,
            greeks.effective_vol,
            true,
        );

        match action {
            RoughFmlsAction::HoldNeutral {
                target_delta,
                band_width,
            } => {
                assert!((target_delta - res_greeks.delta).abs() < 1e-4);
                assert!(band_width > 0.005);
            }
            _ => panic!("Expected HoldNeutral"),
        }
    }

    #[test]
    fn test_rough_fmls_skew_dispersion() {
        let hedger = RoughFmlsHedger::new();
        let greeks = hedger.compute_malliavin_greeks(100.0, 100.0, 0.25, 1.0, 1.0, true);
        // Dislocated perp skew (0.10 away from effective vol)
        let (action, _) = hedger.evaluate_hedging_decision(
            100.0,
            100.0,
            0.25,
            1.0,
            1.0,
            greeks.delta,
            greeks.effective_vol + 0.10,
            true,
        );

        match action {
            RoughFmlsAction::SkewDispersion {
                dispersion_pts,
                target_delta,
            } => {
                assert!(dispersion_pts >= 9.99);
                assert!((target_delta - greeks.delta).abs() < 1e-4);
            }
            _ => panic!("Expected SkewDispersion"),
        }
    }

    #[test]
    fn test_rough_fmls_hedging_rebalance() {
        let hedger = RoughFmlsHedger::new();
        // Current hedge delta 0.10 is far away from ~0.50 target
        let (action, greeks) =
            hedger.evaluate_hedging_decision(100.0, 100.0, 0.25, 1.0, 1.0, 0.10, 0.30, true);

        match action {
            RoughFmlsAction::RebalanceHedge {
                current_delta,
                target_delta,
                rebalance_size,
                ..
            } => {
                assert_eq!(current_delta, 0.10);
                assert!((target_delta - greeks.delta).abs() < 1e-4);
                assert!(rebalance_size > 0.30);
            }
            _ => panic!("Expected RebalanceHedge"),
        }
    }
}
