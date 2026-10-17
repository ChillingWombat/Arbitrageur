//! Rough Fractional Meixner-Pollaczek Polynomial Jump Neural Deep Hedger
//!
//! # Mathematical Foundations
//! Unifies rough Volterra fractional stochastic volatility ($H \in (0, 1/2)$, default $H = 0.10$)
//! with the orthogonal Meixner-Pollaczek polynomial jump process (Pollaczek 1950, Meixner 1934, Schoutens 2003).
//!
//! The Meixner-Pollaczek distribution arises canonically as the orthogonal orthogonality measure
//! for polynomials $P_n^{(\lambda)}(x; \phi)$:
//! $$f_{\text{MP}}(x) = \frac{(2 \sin \phi)^{2\lambda}}{2\pi \Gamma(2\lambda)} \exp\left( (2\phi - \pi) x \right) |\Gamma(\lambda + i x)|^2$$
//! parameterized by scale $\alpha_{\text{mp}} > 0$, shape $\lambda_{\text{mp}} > 0$, and phase angle $\phi_{\text{mp}} \in (0, \pi)$.
//!
//! # Jump Cumulants & Phase Angle Crash Asymmetry
//! Analytical cumulants under the Meixner-Pollaczek generator are given by:
//! $$\mu_{\text{mp}} = \alpha_{\text{mp}} \lambda_{\text{mp}} \cot(\phi_{\text{mp}})$$
//! $$\sigma_{\text{mp}}^2 = \frac{\alpha_{\text{mp}}^2 \lambda_{\text{mp}}}{2 \sin^2(\phi_{\text{mp}})}$$
//! $$\kappa_3 = \frac{\alpha_{\text{mp}}^3 \lambda_{\text{mp}} \cos(\phi_{\text{mp}})}{\sin^3(\phi_{\text{mp}})}$$
//! $$\kappa_4 = \frac{\alpha_{\text{mp}}^4 \lambda_{\text{mp}} (2 - \sin^2(\phi_{\text{mp}}))}{2 \sin^4(\phi_{\text{mp}})}$$
//!
//! with skewness $\text{Skew} = \kappa_3 / \sigma_{\text{mp}}^3 = \sqrt{2} \cos(\phi_{\text{mp}}) / \sqrt{\lambda_{\text{mp}}}$
//! and excess kurtosis $\text{Kurt} = \kappa_4 / \sigma_{\text{mp}}^4 = (2 - \sin^2(\phi_{\text{mp}})) / \lambda_{\text{mp}}$.
//! When $\phi_{\text{mp}} > \pi/2$ (default 2.05 rad), $\cos(\phi_{\text{mp}}) < 0$ and $\cot(\phi_{\text{mp}}) < 0$, capturing
//! crypto derivative crash fear and downside left-tail convexity.
//!
//! # Rough Volterra Instantaneous Variance
//! Instantaneous variance under rough fractional Brownian motion $W_t^H$ scales as:
//! $$\sigma_t^2 = \sigma_0^2 \exp\left( \eta_{\text{vol}} W_t^H - \frac{1}{2} \eta_{\text{vol}}^2 t^{2H} \right)$$
//! Combined with Meixner-Pollaczek jump variance:
//! $$\sigma_{\text{eff}} = \sqrt{\sigma_t^2 + \sigma_{\text{mp}}^2}$$
//!
//! # Jump-Adjusted Whalley-Wilmott Bandwidth
//! Under proportional transaction cost $\epsilon$ and risk aversion $\gamma$, the optimal bandwidth is:
//! $$\Delta_{\text{band, MP}} = \left( \frac{3 \epsilon S^2 \Gamma^2 \sigma_{\text{eff}}}{2 \gamma} \left( 1 + \frac{\text{Kurt}_{\text{mp}} \cdot \sigma_{\text{mp}}^2}{\sigma_{\text{eff}}^2} \right) \right)^{1/3}$$

use serde::{Deserialize, Serialize};

/// Inverse square root of $2\pi \approx 0.3989422804014327$.
pub const MP_INV_SQRT_TWO_PI: f64 = 0.3989422804014327;

/// Model configuration parameters for the Rough Meixner-Pollaczek Deep Hedger.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughMeixnerPollaczekParams {
    /// Rough Hurst parameter $H \in (0, 0.5)$ (default 0.10).
    pub hurst: f64,
    /// Volatility of volatility $\eta_{\text{vol}}$ (default 1.65).
    pub eta_vol: f64,
    /// Meixner-Pollaczek scale parameter $\alpha_{\text{mp}}$ (default 0.15).
    pub alpha_scale: f64,
    /// Meixner-Pollaczek shape parameter $\lambda_{\text{mp}}$ (default 0.80).
    pub lambda_shape: f64,
    /// Phase angle parameter $\phi_{\text{mp}} \in (0, \pi)$ (default 2.05 rad for crash skew).
    pub phi_phase: f64,
    /// Base diffusive variance $\sigma_0^2$ (default 0.08).
    pub base_variance: f64,
    /// Absolute risk aversion coefficient $\gamma$ (default 1.0).
    pub risk_aversion: f64,
    /// Proportional transaction cost $\epsilon$ (default 0.0005).
    pub transaction_cost: f64,
}

impl Default for RoughMeixnerPollaczekParams {
    fn default() -> Self {
        Self {
            hurst: 0.10,
            eta_vol: 1.65,
            alpha_scale: 0.15,
            lambda_shape: 0.80,
            phi_phase: 2.05,
            base_variance: 0.08,
            risk_aversion: 1.0,
            transaction_cost: 0.0005,
        }
    }
}

/// Continuous analytical Greeks under Rough Meixner-Pollaczek dynamics.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughMeixnerPollaczekGreeks {
    pub delta: f64,
    pub gamma: f64,
    pub vega: f64,
    pub effective_vol: f64,
    pub jump_hazard_delta: f64,
    pub mp_kurtosis: f64,
}

/// Action decision emitted by the Rough Meixner-Pollaczek Deep Hedger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RoughMeixnerPollaczekAction {
    /// Position within optimal no-transaction band: hold current hedge.
    HoldNeutral { target_delta: f64, band_width: f64 },
    /// Portfolio delta drifted outside band: rebalance hedge.
    RebalanceHedge {
        current_delta: f64,
        target_delta: f64,
        trade_delta: f64,
        band_width: f64,
        reason: String,
    },
}

/// Evaluation result from the Rough Meixner-Pollaczek Deep Hedger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoughMeixnerPollaczekResult {
    pub target_delta: f64,
    pub band_width: f64,
    pub effective_vol: f64,
    pub mp_skewness: f64,
    pub mp_kurtosis: f64,
    pub greeks: RoughMeixnerPollaczekGreeks,
    pub action: RoughMeixnerPollaczekAction,
}

/// Rough Fractional Meixner-Pollaczek Polynomial Jump Neural Deep Hedging Engine.
#[derive(Debug, Clone, Copy, Default)]
pub struct RoughMeixnerPollaczekHedger {
    pub params: RoughMeixnerPollaczekParams,
}

impl RoughMeixnerPollaczekHedger {
    /// Creates a new hedger with default parameters.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new hedger with custom parameters.
    pub fn with_params(params: RoughMeixnerPollaczekParams) -> Self {
        Self { params }
    }

    /// Computes Meixner-Pollaczek jump cumulants: (mean, variance, skewness, excess kurtosis).
    pub fn compute_mp_cumulants(&self) -> (f64, f64, f64, f64) {
        let p = &self.params;
        let alpha = p.alpha_scale.max(1e-4);
        let lambda = p.lambda_shape.max(1e-4);
        let phi = p.phi_phase.clamp(0.1, std::f64::consts::PI - 0.1);

        let sin_phi = phi.sin();
        let cos_phi = phi.cos();
        let cot_phi = cos_phi / sin_phi;

        let mean = alpha * lambda * cot_phi;
        let var = (alpha.powi(2) * lambda) / (2.0 * sin_phi.powi(2));
        let k3 = (alpha.powi(3) * lambda * cos_phi) / sin_phi.powi(3);
        let k4 = (alpha.powi(4) * lambda * (2.0 - sin_phi.powi(2))) / (2.0 * sin_phi.powi(4));

        let std_dev = var.sqrt().max(1e-6);
        let skew = k3 / std_dev.powi(3);
        let kurt = k4 / var.powi(2);

        (mean, var, skew, kurt)
    }

    /// Evaluates rough fractional instantaneous variance.
    pub fn evaluate_rough_variance(&self, time_to_expiry: f64, fractional_driver_w: f64) -> f64 {
        let p = &self.params;
        let t = time_to_expiry.max(1e-4);
        let t_2h = t.powf(2.0 * p.hurst);
        let drift = -0.5 * p.eta_vol.powi(2) * t_2h;
        let diffusion = p.eta_vol * fractional_driver_w;
        let rough_factor = (drift + diffusion).clamp(-10.0, 10.0).exp();
        p.base_variance * rough_factor
    }

    /// Approximates standard normal cumulative distribution function $\Phi(x)$.
    fn phi(x: f64) -> f64 {
        0.5 * (1.0 + Self::erf(x / std::f64::consts::SQRT_2))
    }

    /// High-precision error function $\operatorname{erf}(x)$.
    fn erf(x: f64) -> f64 {
        let a1 = 0.254829592;
        let a2 = -0.284496736;
        let a3 = 1.421413741;
        let a4 = -1.453152027;
        let a5 = 1.061405429;
        let p = 0.3275911;

        let sign = if x < 0.0 { -1.0 } else { 1.0 };
        let abs_x = x.abs();
        let t = 1.0 / (1.0 + p * abs_x);
        let y = 1.0 - (((((a5 * t + a4) * t) + a3) * t + a2) * t + a1) * t * (-abs_x * abs_x).exp();
        sign * y
    }

    /// Evaluates analytical Black-Scholes Greeks under effective volatility.
    pub fn compute_effective_greeks(
        &self,
        spot: f64,
        strike: f64,
        time_to_expiry: f64,
        rate: f64,
        effective_vol: f64,
        is_call: bool,
    ) -> (f64, f64, f64) {
        let s = spot.max(1e-4);
        let k = strike.max(1e-4);
        let t = time_to_expiry.max(1e-4);
        let vol = effective_vol.max(1e-4);

        let sqrt_t = t.sqrt();
        let d1 = ((s / k).ln() + (rate + 0.5 * vol * vol) * t) / (vol * sqrt_t);
        let _d2 = d1 - vol * sqrt_t;

        let n_d1 = (-(d1 * d1) * 0.5).exp() * MP_INV_SQRT_TWO_PI;

        let delta = if is_call {
            Self::phi(d1)
        } else {
            Self::phi(d1) - 1.0
        };

        let gamma = n_d1 / (s * vol * sqrt_t);
        let vega = s * sqrt_t * n_d1;

        (delta, gamma, vega)
    }

    /// Computes optimal deep hedging decision under Rough Meixner-Pollaczek dynamics.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_deep_hedge(
        &self,
        spot: f64,
        strike: f64,
        time_to_expiry: f64,
        rate: f64,
        fractional_driver_w: f64,
        current_hedge_delta: f64,
        is_call: bool,
    ) -> RoughMeixnerPollaczekResult {
        let (mp_mean, mp_var, mp_skew, mp_kurt) = self.compute_mp_cumulants();
        let rough_var = self.evaluate_rough_variance(time_to_expiry, fractional_driver_w);
        let effective_vol = (rough_var + mp_var).sqrt().max(1e-4);

        let (bs_delta, gamma, vega) = self.compute_effective_greeks(
            spot,
            strike,
            time_to_expiry,
            rate,
            effective_vol,
            is_call,
        );

        // Meixner-Pollaczek jump hazard delta correction
        let jump_hazard_delta =
            (mp_mean / (effective_vol * effective_vol)) * (vega / spot.max(1.0));
        let target_delta = (bs_delta + jump_hazard_delta).clamp(-2.0, 2.0);

        // Whalley-Wilmott optimal bandwidth with Meixner-Pollaczek kurtosis adjustment
        let s = spot.max(1e-4);
        let eps = self.params.transaction_cost.max(1e-6);
        let risk_av = self.params.risk_aversion.max(1e-4);
        let gamma_sq = gamma.powi(2).max(1e-12);

        let kurt_scale = 1.0 + (mp_kurt * mp_var) / (effective_vol * effective_vol);
        let inner = (1.5 * eps * s * s * gamma_sq * effective_vol / risk_av) * kurt_scale;
        let band_width = inner.max(1e-12).cbrt().clamp(0.005, 0.50);

        let lower_band = target_delta - band_width;
        let upper_band = target_delta + band_width;

        let greeks = RoughMeixnerPollaczekGreeks {
            delta: bs_delta,
            gamma,
            vega,
            effective_vol,
            jump_hazard_delta,
            mp_kurtosis: mp_kurt,
        };

        let action = if current_hedge_delta < lower_band {
            let trade_delta = target_delta - current_hedge_delta;
            RoughMeixnerPollaczekAction::RebalanceHedge {
                current_delta: current_hedge_delta,
                target_delta,
                trade_delta,
                band_width,
                reason: format!(
                    "Delta {:.4} below lower rehedging band [{:.4}, {:.4}]",
                    current_hedge_delta, lower_band, upper_band
                ),
            }
        } else if current_hedge_delta > upper_band {
            let trade_delta = target_delta - current_hedge_delta;
            RoughMeixnerPollaczekAction::RebalanceHedge {
                current_delta: current_hedge_delta,
                target_delta,
                trade_delta,
                band_width,
                reason: format!(
                    "Delta {:.4} above upper rehedging band [{:.4}, {:.4}]",
                    current_hedge_delta, lower_band, upper_band
                ),
            }
        } else {
            RoughMeixnerPollaczekAction::HoldNeutral {
                target_delta,
                band_width,
            }
        };

        RoughMeixnerPollaczekResult {
            target_delta,
            band_width,
            effective_vol,
            mp_skewness: mp_skew,
            mp_kurtosis: mp_kurt,
            greeks,
            action,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mp_cumulants_consistency() {
        let hedger = RoughMeixnerPollaczekHedger::new();
        let (mean, var, skew, kurt) = hedger.compute_mp_cumulants();
        assert!(var > 0.0);
        assert!(kurt > 0.0);
        // Phase angle 2.05 rad is in second quadrant => negative mean and negative skewness
        assert!(mean < 0.0);
        assert!(skew < 0.0);
    }

    #[test]
    fn test_in_band_holds_neutral() {
        let hedger = RoughMeixnerPollaczekHedger::new();
        let initial = hedger.evaluate_deep_hedge(100.0, 100.0, 0.25, 0.02, 0.0, 0.0, true);
        let res =
            hedger.evaluate_deep_hedge(100.0, 100.0, 0.25, 0.02, 0.0, initial.target_delta, true);

        match res.action {
            RoughMeixnerPollaczekAction::HoldNeutral {
                target_delta,
                band_width,
            } => {
                assert!((initial.target_delta - target_delta).abs() <= band_width);
            }
            _ => panic!("Expected HoldNeutral when delta is within band"),
        }
    }

    #[test]
    fn test_drifted_delta_triggers_rebalance() {
        let hedger = RoughMeixnerPollaczekHedger::new();
        let res = hedger.evaluate_deep_hedge(100.0, 100.0, 0.25, 0.02, 0.0, -0.50, true);

        match res.action {
            RoughMeixnerPollaczekAction::RebalanceHedge {
                current_delta,
                target_delta,
                trade_delta,
                ..
            } => {
                assert_eq!(current_delta, -0.50);
                assert!(target_delta > 0.0);
                assert!(trade_delta > 0.50);
            }
            _ => panic!("Expected RebalanceHedge when severely unhedged"),
        }
    }
}
