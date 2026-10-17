//! Rough Fractional Tempered Hermite-Bessel Jump Neural Deep Hedger
//!
//! # Mathematical Foundations
//! Unifies rough Volterra fractional stochastic volatility ($H \in (0, 1/2)$, default $H = 0.10$)
//! with the Tempered Hermite-Bessel (THB) Lévy jump process (Barndorff-Nielsen & Shiryaev 2010, Yor 2001).
//!
//! The Tempered Hermite-Bessel jump density incorporates modified Bessel weighting with Hermite polynomial
//! perturbation to capture multi-scale jump clustering and extreme crypto crash fear:
//! $$\nu_{\text{THB}}(dx) = \frac{C_{\text{thb}}}{|x|^{1 + 2\nu}} K_\nu(\lambda_{\text{jump}} |x|) \exp(-\theta_{\text{thb}} x) \left( 1 + \beta_1 H_1(x) \right) dx$$
//!
//! # Jump Cumulants & Asymmetric Tail Structure
//! Analytical cumulants under the tempered Bessel-Hermite kernel are given by:
//! $$\mu_{\text{thb}} = -\theta_{\text{thb}} \cdot \frac{C_{\text{thb}}}{\lambda_{\text{jump}}}$$
//! $$\sigma_{\text{thb}}^2 = C_{\text{thb}} \frac{2\nu + 1}{\lambda_{\text{jump}}^2} (1 + \theta_{\text{thb}}^2)$$
//! $$\kappa_3 = -2 \theta_{\text{thb}} \cdot \frac{C_{\text{thb}}}{\lambda_{\text{jump}}^3} (3 + \beta_1)$$
//! $$\kappa_4 = 6 \cdot \frac{C_{\text{thb}}}{\lambda_{\text{jump}}^4} (1 + 2\theta_{\text{thb}}^2 + \beta_1)$$
//!
//! with skewness $\text{Skew} = \kappa_3 / \sigma_{\text{thb}}^3$ and excess kurtosis $\text{Kurt} = \kappa_4 / \sigma_{\text{thb}}^4$.
//!
//! # Rough Volterra Instantaneous Variance
//! Instantaneous variance under fractional Brownian motion $W_t^H$ scales as:
//! $$\sigma_t^2 = \sigma_0^2 \exp\left( \eta_{\text{vol}} W_t^H - \frac{1}{2} \eta_{\text{vol}}^2 t^{2H} \right)$$
//! Combined with Tempered Hermite-Bessel jump variance:
//! $$\sigma_{\text{eff}} = \sqrt{\sigma_t^2 + \sigma_{\text{thb}}^2}$$
//!
//! # Jump-Adjusted Whalley-Wilmott Bandwidth
//! Under proportional transaction cost $\epsilon$ and risk aversion $\gamma$, the optimal bandwidth is:
//! $$\Delta_{\text{band, THB}} = \left( \frac{3 \epsilon S^2 \Gamma^2 \sigma_{\text{eff}}}{2 \gamma} \left( 1 + \frac{\text{Kurt}_{\text{thb}} \cdot \sigma_{\text{thb}}^2}{\sigma_{\text{eff}}^2} \right) \right)^{1/3}$$

use serde::{Deserialize, Serialize};

/// Inverse square root of $2\pi \approx 0.3989422804014327$.
pub const THB_INV_SQRT_TWO_PI: f64 = 0.3989422804014327;

/// Model configuration parameters for the Rough Tempered Hermite-Bessel Deep Hedger.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughHermiteBesselParams {
    /// Rough Hurst parameter $H \in (0, 0.5)$ (default 0.10).
    pub hurst: f64,
    /// Volatility of volatility $\eta_{\text{vol}}$ (default 1.70).
    pub eta_vol: f64,
    /// Overall jump intensity $C_{\text{thb}}$ (default 0.40).
    pub c_thb: f64,
    /// Bessel order parameter $\nu \ge 0$ (default 0.50).
    pub nu_bessel: f64,
    /// Jump tempering rate $\lambda_{\text{jump}}$ (default 6.0).
    pub lambda_jump: f64,
    /// Crash skew tilt parameter $\theta_{\text{thb}}$ (default 0.85).
    pub theta_tilt: f64,
    /// First Hermite polynomial perturbation weight $\beta_1$ (default 0.35).
    pub beta_hermite: f64,
    /// Base diffusive variance $\sigma_0^2$ (default 0.08).
    pub base_variance: f64,
    /// Absolute risk aversion coefficient $\gamma$ (default 1.0).
    pub risk_aversion: f64,
    /// Proportional transaction cost $\epsilon$ (default 0.0005).
    pub transaction_cost: f64,
}

impl Default for RoughHermiteBesselParams {
    fn default() -> Self {
        Self {
            hurst: 0.10,
            eta_vol: 1.70,
            c_thb: 0.40,
            nu_bessel: 0.50,
            lambda_jump: 6.0,
            theta_tilt: 0.85,
            beta_hermite: 0.35,
            base_variance: 0.08,
            risk_aversion: 1.0,
            transaction_cost: 0.0005,
        }
    }
}

/// Continuous analytical Greeks under Rough Tempered Hermite-Bessel dynamics.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughHermiteBesselGreeks {
    pub delta: f64,
    pub gamma: f64,
    pub vega: f64,
    pub effective_vol: f64,
    pub jump_hazard_delta: f64,
    pub thb_kurtosis: f64,
}

/// Action decision emitted by the Rough Tempered Hermite-Bessel Deep Hedger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RoughHermiteBesselAction {
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

/// Evaluation result from the Rough Tempered Hermite-Bessel Deep Hedger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoughHermiteBesselResult {
    pub target_delta: f64,
    pub band_width: f64,
    pub effective_vol: f64,
    pub thb_skewness: f64,
    pub thb_kurtosis: f64,
    pub greeks: RoughHermiteBesselGreeks,
    pub action: RoughHermiteBesselAction,
}

/// Rough Fractional Tempered Hermite-Bessel Jump Neural Deep Hedging Engine.
#[derive(Debug, Clone, Copy, Default)]
pub struct RoughHermiteBesselHedger {
    pub params: RoughHermiteBesselParams,
}

impl RoughHermiteBesselHedger {
    /// Creates a new hedger with default parameters.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new hedger with custom parameters.
    pub fn with_params(params: RoughHermiteBesselParams) -> Self {
        Self { params }
    }

    /// Computes Tempered Hermite-Bessel jump cumulants: (mean, variance, skewness, excess kurtosis).
    pub fn compute_thb_cumulants(&self) -> (f64, f64, f64, f64) {
        let p = &self.params;
        let c = p.c_thb.max(1e-4);
        let lam = p.lambda_jump.max(1e-4);
        let nu = p.nu_bessel.max(0.0);
        let theta = p.theta_tilt;
        let beta = p.beta_hermite;

        let mean = -theta * (c / lam);
        let var = c * (2.0 * nu + 1.0) * (1.0 + theta * theta) / (lam * lam);
        let k3 = -2.0 * theta * (c / lam.powi(3)) * (3.0 + beta);
        let k4 = 6.0 * (c / lam.powi(4)) * (1.0 + 2.0 * theta * theta + beta);

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

        let n_d1 = (-(d1 * d1) * 0.5).exp() * THB_INV_SQRT_TWO_PI;

        let delta = if is_call {
            Self::phi(d1)
        } else {
            Self::phi(d1) - 1.0
        };

        let gamma = n_d1 / (s * vol * sqrt_t);
        let vega = s * sqrt_t * n_d1;

        (delta, gamma, vega)
    }

    /// Computes optimal deep hedging decision under Rough Tempered Hermite-Bessel dynamics.
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
    ) -> RoughHermiteBesselResult {
        let (thb_mean, thb_var, thb_skew, thb_kurt) = self.compute_thb_cumulants();
        let rough_var = self.evaluate_rough_variance(time_to_expiry, fractional_driver_w);
        let effective_vol = (rough_var + thb_var).sqrt().max(1e-4);

        let (bs_delta, gamma, vega) = self.compute_effective_greeks(
            spot,
            strike,
            time_to_expiry,
            rate,
            effective_vol,
            is_call,
        );

        // Tempered Hermite-Bessel jump hazard delta correction
        let jump_hazard_delta =
            (thb_mean / (effective_vol * effective_vol)) * (vega / spot.max(1.0));
        let target_delta = (bs_delta + jump_hazard_delta).clamp(-2.0, 2.0);

        // Whalley-Wilmott optimal bandwidth with Tempered Hermite-Bessel kurtosis adjustment
        let s = spot.max(1e-4);
        let eps = self.params.transaction_cost.max(1e-6);
        let risk_av = self.params.risk_aversion.max(1e-4);
        let gamma_sq = gamma.powi(2).max(1e-12);

        let kurt_scale = 1.0 + (thb_kurt * thb_var) / (effective_vol * effective_vol);
        let inner = (1.5 * eps * s * s * gamma_sq * effective_vol / risk_av) * kurt_scale;
        let band_width = inner.max(1e-12).cbrt().clamp(0.005, 0.50);

        let lower_band = target_delta - band_width;
        let upper_band = target_delta + band_width;

        let greeks = RoughHermiteBesselGreeks {
            delta: bs_delta,
            gamma,
            vega,
            effective_vol,
            jump_hazard_delta,
            thb_kurtosis: thb_kurt,
        };

        let action = if current_hedge_delta < lower_band {
            let trade_delta = target_delta - current_hedge_delta;
            RoughHermiteBesselAction::RebalanceHedge {
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
            RoughHermiteBesselAction::RebalanceHedge {
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
            RoughHermiteBesselAction::HoldNeutral {
                target_delta,
                band_width,
            }
        };

        RoughHermiteBesselResult {
            target_delta,
            band_width,
            effective_vol,
            thb_skewness: thb_skew,
            thb_kurtosis: thb_kurt,
            greeks,
            action,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thb_cumulants_consistency() {
        let hedger = RoughHermiteBesselHedger::new();
        let (mean, var, skew, kurt) = hedger.compute_thb_cumulants();
        assert!(var > 0.0);
        assert!(kurt > 0.0);
        assert!(mean < 0.0);
        assert!(skew < 0.0);
    }

    #[test]
    fn test_in_band_holds_neutral() {
        let hedger = RoughHermiteBesselHedger::new();
        let initial = hedger.evaluate_deep_hedge(100.0, 100.0, 0.25, 0.02, 0.0, 0.0, true);
        let res =
            hedger.evaluate_deep_hedge(100.0, 100.0, 0.25, 0.02, 0.0, initial.target_delta, true);

        match res.action {
            RoughHermiteBesselAction::HoldNeutral {
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
        let hedger = RoughHermiteBesselHedger::new();
        let res = hedger.evaluate_deep_hedge(100.0, 100.0, 0.25, 0.02, 0.0, -0.50, true);

        match res.action {
            RoughHermiteBesselAction::RebalanceHedge {
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
