//! Rough Fractional Bilateral Gamma Jump Neural Deep Hedger
//!
//! # Mathematical Foundations
//! Unifies rough Volterra fractional stochastic volatility ($H \in (0, 1/2)$, default $H = 0.10$)
//! with the Bilateral Gamma (BG) Lévy process (Küchler & Tappe 2008, Eberlein & Madan 2009).
//!
//! The Bilateral Gamma process is defined as the difference of two independent Gamma processes:
//! $$X_t = \Gamma_t^{(+)}(c_+, \lambda_+) - \Gamma_t^{(-)}(c_-, \lambda_-)$$
//! where $\Gamma_t^{(\pm)} \sim \operatorname{Gamma}(c_\pm t, \lambda_\pm)$.
//! Decoupled positive and negative jump frequencies ($c_+, c_- > 0$) and decay parameters ($\lambda_+, \lambda_- > 0$)
//! cleanly capture asymmetric crash fear and upside momentum in crypto derivative markets.
//!
//! # Jump Cumulants & Asymmetric Tail Structure
//! The cumulants of the Bilateral Gamma process are:
//! $$\mu_{\text{bg}} = \frac{c_+}{\lambda_+} - \frac{c_-}{\lambda_-}$$
//! $$\sigma_{\text{bg}}^2 = \frac{c_+}{\lambda_+^2} + \frac{c_-}{\lambda_-^2}$$
//! $$\kappa_3 = 2 \left( \frac{c_+}{\lambda_+^3} - \frac{c_-}{\lambda_-^3} \right)$$
//! $$\kappa_4 = 6 \left( \frac{c_+}{\lambda_+^4} + \frac{c_-}{\lambda_-^4} \right)$$
//!
//! with skewness $\text{Skew} = \kappa_3 / \sigma_{\text{bg}}^3$ and excess kurtosis $\text{Kurt} = \kappa_4 / \sigma_{\text{bg}}^4$.
//!
//! # Rough Volterra Variance & Hybrid Volatility
//! Instantaneous variance under rough fractional Brownian motion $W_t^H$ scales as:
//! $$\sigma_t^2 = \sigma_0^2 \exp\left( \eta_{\text{vol}} W_t^H - \frac{1}{2} \eta_{\text{vol}}^2 t^{2H} \right)$$
//! Combined with Bilateral Gamma jump variance:
//! $$\sigma_{\text{eff}} = \sqrt{\sigma_t^2 + \sigma_{\text{bg}}^2}$$
//!
//! # Jump-Adjusted Whalley-Wilmott Rehedging Bandwidth
//! Under proportional transaction cost $\epsilon$ and absolute risk aversion $\gamma$, the optimal bandwidth is:
//! $$\Delta_{\text{band, BG}} = \left( \frac{3 \epsilon S^2 \Gamma^2 \sigma_{\text{eff}}}{2 \gamma} \left( 1 + \frac{\text{Kurt} \cdot \sigma_{\text{bg}}^2}{\sigma_{\text{eff}}^2} \right) \right)^{1/3}$$

use serde::{Deserialize, Serialize};

/// Inverse square root of $2\pi \approx 0.3989422804014327$.
pub const BG_INV_SQRT_TWO_PI: f64 = 0.3989422804014327;

/// Model configuration parameters for the Rough Bilateral Gamma Deep Hedger.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughBilateralGammaParams {
    /// Rough Hurst parameter $H \in (0, 0.5)$ (default 0.10).
    pub hurst: f64,
    /// Volatility of volatility $\eta_{\text{vol}}$ (default 1.65).
    pub eta_vol: f64,
    /// Positive jump intensity $c_+$ (default 1.5).
    pub c_plus: f64,
    /// Negative jump intensity $c_-$ (default 2.5 for crash asymmetry).
    pub c_minus: f64,
    /// Positive decay parameter $\lambda_+$ (default 12.0).
    pub lambda_plus: f64,
    /// Negative decay parameter $\lambda_-$ (default 8.0).
    pub lambda_minus: f64,
    /// Base diffusive variance $\sigma_0^2$ (default 0.08).
    pub base_variance: f64,
    /// Absolute risk aversion coefficient $\gamma$ (default 1.0).
    pub risk_aversion: f64,
    /// Proportional transaction cost $\epsilon$ (default 0.0005).
    pub transaction_cost: f64,
}

impl Default for RoughBilateralGammaParams {
    fn default() -> Self {
        Self {
            hurst: 0.10,
            eta_vol: 1.65,
            c_plus: 1.5,
            c_minus: 2.5,
            lambda_plus: 12.0,
            lambda_minus: 8.0,
            base_variance: 0.08,
            risk_aversion: 1.0,
            transaction_cost: 0.0005,
        }
    }
}

/// Continuous analytical Greeks under Rough Bilateral Gamma jump dynamics.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughBilateralGammaGreeks {
    pub delta: f64,
    pub gamma: f64,
    pub vega: f64,
    pub effective_vol: f64,
    pub jump_hazard_delta: f64,
    pub bg_kurtosis: f64,
}

/// Action decision emitted by the Rough Bilateral Gamma Deep Hedger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RoughBilateralGammaAction {
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

/// Evaluation result from the Rough Bilateral Gamma Deep Hedger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoughBilateralGammaResult {
    pub target_delta: f64,
    pub band_width: f64,
    pub effective_vol: f64,
    pub bg_skewness: f64,
    pub bg_kurtosis: f64,
    pub greeks: RoughBilateralGammaGreeks,
    pub action: RoughBilateralGammaAction,
}

/// Rough Fractional Bilateral Gamma Jump Neural Deep Hedging Engine.
#[derive(Debug, Clone, Copy, Default)]
pub struct RoughBilateralGammaHedger {
    pub params: RoughBilateralGammaParams,
}

impl RoughBilateralGammaHedger {
    /// Creates a new hedger with default parameters.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new hedger with custom parameters.
    pub fn with_params(params: RoughBilateralGammaParams) -> Self {
        Self { params }
    }

    /// Computes Bilateral Gamma jump cumulants: (mean, variance, skewness, excess kurtosis).
    pub fn compute_bg_cumulants(&self) -> (f64, f64, f64, f64) {
        let p = &self.params;
        let c_p = p.c_plus.max(1e-4);
        let c_m = p.c_minus.max(1e-4);
        let l_p = p.lambda_plus.max(1e-4);
        let l_m = p.lambda_minus.max(1e-4);

        let mean = (c_p / l_p) - (c_m / l_m);
        let var = (c_p / (l_p * l_p)) + (c_m / (l_m * l_m));
        let k3 = 2.0 * ((c_p / l_p.powi(3)) - (c_m / l_m.powi(3)));
        let k4 = 6.0 * ((c_p / l_p.powi(4)) + (c_m / l_m.powi(4)));

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

    /// High-precision error function $\operatorname{erf}(x)$ (maximum error $< 1.5 \times 10^{-7}$).
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

        let n_d1 = (-(d1 * d1) * 0.5).exp() * BG_INV_SQRT_TWO_PI;

        let delta = if is_call {
            Self::phi(d1)
        } else {
            Self::phi(d1) - 1.0
        };

        let gamma = n_d1 / (s * vol * sqrt_t);
        let vega = s * sqrt_t * n_d1;

        (delta, gamma, vega)
    }

    /// Computes optimal deep hedging decision under Rough Bilateral Gamma dynamics.
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
    ) -> RoughBilateralGammaResult {
        let (bg_mean, bg_var, bg_skew, bg_kurt) = self.compute_bg_cumulants();
        let rough_var = self.evaluate_rough_variance(time_to_expiry, fractional_driver_w);
        let effective_vol = (rough_var + bg_var).sqrt().max(1e-4);

        let (bs_delta, gamma, vega) = self.compute_effective_greeks(
            spot,
            strike,
            time_to_expiry,
            rate,
            effective_vol,
            is_call,
        );

        // Bilateral Gamma jump hazard delta correction
        let jump_hazard_delta =
            (bg_mean / (effective_vol * effective_vol)) * (vega / spot.max(1.0));
        let target_delta = (bs_delta + jump_hazard_delta).clamp(-2.0, 2.0);

        // Whalley-Wilmott optimal bandwidth with Bilateral Gamma kurtosis adjustment
        let s = spot.max(1e-4);
        let eps = self.params.transaction_cost.max(1e-6);
        let risk_av = self.params.risk_aversion.max(1e-4);
        let gamma_sq = gamma.powi(2).max(1e-12);

        let kurt_scale = 1.0 + (bg_kurt * bg_var) / (effective_vol * effective_vol);
        let inner = (1.5 * eps * s * s * gamma_sq * effective_vol / risk_av) * kurt_scale;
        let band_width = inner.max(1e-12).cbrt().clamp(0.005, 0.50);

        let lower_band = target_delta - band_width;
        let upper_band = target_delta + band_width;

        let greeks = RoughBilateralGammaGreeks {
            delta: bs_delta,
            gamma,
            vega,
            effective_vol,
            jump_hazard_delta,
            bg_kurtosis: bg_kurt,
        };

        let action = if current_hedge_delta < lower_band {
            let trade_delta = target_delta - current_hedge_delta;
            RoughBilateralGammaAction::RebalanceHedge {
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
            RoughBilateralGammaAction::RebalanceHedge {
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
            RoughBilateralGammaAction::HoldNeutral {
                target_delta,
                band_width,
            }
        };

        RoughBilateralGammaResult {
            target_delta,
            band_width,
            effective_vol,
            bg_skewness: bg_skew,
            bg_kurtosis: bg_kurt,
            greeks,
            action,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bg_cumulants_consistency() {
        let hedger = RoughBilateralGammaHedger::new();
        let (mean, var, skew, kurt) = hedger.compute_bg_cumulants();
        assert!(var > 0.0);
        assert!(kurt > 0.0);
        // Crash asymmetry: c_minus / lambda_minus > c_plus / lambda_plus => negative mean drift
        assert!(mean < 0.0);
        assert!(skew < 0.0);
    }

    #[test]
    fn test_in_band_holds_neutral() {
        let hedger = RoughBilateralGammaHedger::new();
        // First compute target delta to place current delta directly on target
        let initial = hedger.evaluate_deep_hedge(100.0, 100.0, 0.25, 0.02, 0.0, 0.0, true);
        let res =
            hedger.evaluate_deep_hedge(100.0, 100.0, 0.25, 0.02, 0.0, initial.target_delta, true);

        match res.action {
            RoughBilateralGammaAction::HoldNeutral {
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
        let hedger = RoughBilateralGammaHedger::new();
        let res = hedger.evaluate_deep_hedge(
            100.0, 100.0, 0.25, 0.02, 0.0,
            -0.50, // severely drifted hedge relative to target call delta
            true,
        );

        match res.action {
            RoughBilateralGammaAction::RebalanceHedge {
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
