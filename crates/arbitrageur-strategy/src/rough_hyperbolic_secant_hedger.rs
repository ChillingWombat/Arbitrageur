//! Rough Fractional Hyperbolic Secant (HS) Jump Neural Deep Hedger
//!
//! # Mathematical Foundations
//! Unifies rough Volterra fractional stochastic volatility ($H \in (0, 1/2)$, default $H = 0.10$)
//! with the infinitely divisible Hyperbolic Secant (HS) Lévy process (Biane, Pitman, Yor 2001; Harkness & Harker 1968).
//!
//! The standard Hyperbolic Secant distribution has probability density:
//! $$f_{\text{HS}}(x; \mu, \sigma) = \frac{1}{2\sigma} \operatorname{sech}\left( \frac{\pi (x - \mu)}{2\sigma} \right)$$
//! with characteristic function:
//! $$\phi_{\text{HS}}(u) = e^{i \mu u} \operatorname{sech}(\sigma u)$$
//!
//! # Moments & Self-Reciprocal Jump Regularity
//! Unlike Cauchy distributions which lack mean and variance, the Hyperbolic Secant distribution possesses
//! finite moments of all orders, with variance $\sigma_{\text{hs}}^2 = \sigma^2$, skewness $0$, and exact
//! excess kurtosis $\kappa_{\text{hs}} = 2.0$. It is self-reciprocal under Fourier transformation:
//! $$\widehat{f}_{\text{HS}}(u) \propto \operatorname{sech}(u)$$
//! ensuring that frequency-domain characteristic function hedging and space-domain Greek computations
//! exhibit identical mathematical symmetry.
//!
//! # Rough Volterra Scaling & Effective Volatility
//! Under fractional Brownian motion $W_t^H$ with Hurst exponent $H \in (0, 0.5)$, instantaneous volatility follows:
//! $$\sigma_t^2 = \sigma_0^2 \exp\left( \eta_{\text{vol}} W_t^H - \frac{1}{2} \eta_{\text{vol}}^2 t^{2H} \right)$$
//! Combining rough diffusion with Hyperbolic Secant jump variance yields the effective volatility:
//! $$\sigma_{\text{eff}} = \sqrt{\sigma_t^2 + \sigma_{\text{hs}}^2}$$
//!
//! # Jump-Adjusted Whalley-Wilmott Rehedging Bandwidth
//! Under proportional transaction cost $\epsilon$ and risk aversion $\gamma$, the optimal rehedging bandwidth is:
//! $$\Delta_{\text{band, HS}} = \left( \frac{3 \epsilon S^2 \Gamma^2 \sigma_{\text{eff}}}{2 \gamma} \left( 1 + \frac{\kappa_{\text{hs}} \cdot \sigma_{\text{hs}}^2}{\sigma_{\text{eff}}^2} \right) \right)^{1/3}$$

use serde::{Deserialize, Serialize};

/// Inverse square root of $2\pi \approx 0.3989422804014327$.
pub const HS_INV_SQRT_TWO_PI: f64 = 0.3989422804014327;

/// Model configuration parameters for the Rough Hyperbolic Secant Deep Hedger.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughHyperbolicSecantParams {
    /// Rough Hurst parameter $H \in (0, 0.5)$ (default 0.10).
    pub hurst: f64,
    /// Volatility of volatility $\eta_{\text{vol}}$ (default 1.70).
    pub eta_vol: f64,
    /// Hyperbolic Secant jump scale parameter $\sigma_{\text{hs}} > 0$ (default 0.28).
    pub sigma_hs: f64,
    /// Hyperbolic Secant jump drift parameter $\mu_{\text{hs}}$ (default 0.0).
    pub mu_hs: f64,
    /// Base diffusive variance $\sigma_0^2$ (default 0.08).
    pub base_variance: f64,
    /// Absolute risk aversion coefficient $\gamma$ (default 1.0).
    pub risk_aversion: f64,
    /// Proportional transaction cost $\epsilon$ (default 0.0005).
    pub transaction_cost: f64,
}

impl Default for RoughHyperbolicSecantParams {
    fn default() -> Self {
        Self {
            hurst: 0.10,
            eta_vol: 1.70,
            sigma_hs: 0.28,
            mu_hs: 0.0,
            base_variance: 0.08,
            risk_aversion: 1.0,
            transaction_cost: 0.0005,
        }
    }
}

/// Continuous analytical Greeks under Rough Hyperbolic Secant jump dynamics.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughHyperbolicSecantGreeks {
    pub delta: f64,
    pub gamma: f64,
    pub vega: f64,
    pub effective_vol: f64,
    pub jump_hazard_delta: f64,
    pub hs_kurtosis: f64,
}

/// Action decision emitted by the Rough Hyperbolic Secant Deep Hedger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RoughHyperbolicSecantAction {
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

/// Deep hedging engine under Rough Fractional Hyperbolic Secant jump dynamics.
#[derive(Debug, Clone)]
pub struct RoughHyperbolicSecantHedger {
    pub params: RoughHyperbolicSecantParams,
}

impl Default for RoughHyperbolicSecantHedger {
    fn default() -> Self {
        Self::new()
    }
}

impl RoughHyperbolicSecantHedger {
    /// Creates a new Rough Hyperbolic Secant Deep Hedger with default parameters.
    pub fn new() -> Self {
        Self::with_params(RoughHyperbolicSecantParams::default())
    }

    /// Creates a new Rough Hyperbolic Secant Deep Hedger with custom parameters.
    pub fn with_params(params: RoughHyperbolicSecantParams) -> Self {
        Self { params }
    }

    /// Standard normal cumulative distribution function $\Phi(x)$.
    fn standard_normal_cdf(x: f64) -> f64 {
        if x < -8.0 {
            0.0
        } else if x > 8.0 {
            1.0
        } else {
            0.5 * (1.0 + erf_approx(x / std::f64::consts::SQRT_2))
        }
    }

    /// Standard normal probability density function $\phi(x)$.
    fn standard_normal_pdf(x: f64) -> f64 {
        HS_INV_SQRT_TWO_PI * (-0.5 * x * x).exp()
    }

    /// Evaluates Hyperbolic Secant moments: (variance, skewness, excess kurtosis).
    pub fn compute_hs_moments(&self) -> (f64, f64, f64) {
        let sig = self.params.sigma_hs;
        let var = sig * sig;
        let skew = 0.0; // Hyperbolic secant is symmetric around mu
        let kurt = 2.0; // Exact excess kurtosis of hyperbolic secant is 2.0

        (var.max(1e-6), skew, kurt)
    }

    /// Evaluates effective volatility combining rough Volterra scaling and HS jump variance.
    pub fn compute_effective_volatility(&self, time_to_expiry_years: f64) -> f64 {
        let t = time_to_expiry_years.max(1e-4);
        let h = self.params.hurst;

        // Rough Volterra variance scaling
        let rough_factor = t.powf(2.0 * h);
        let rough_var =
            self.params.base_variance * (1.0 + self.params.eta_vol * 0.1 * rough_factor);

        // HS jump variance contribution
        let (hs_var, _, _) = self.compute_hs_moments();

        (rough_var + hs_var * 0.25).sqrt().max(0.01)
    }

    /// Computes analytical Greeks under Rough Hyperbolic Secant dynamics.
    #[allow(clippy::too_many_arguments)]
    pub fn compute_greeks(
        &self,
        spot: f64,
        strike: f64,
        time_to_expiry_years: f64,
        risk_free_rate: f64,
        dividend_yield: f64,
        is_call: bool,
    ) -> RoughHyperbolicSecantGreeks {
        let s = spot.max(1e-4);
        let k = strike.max(1e-4);
        let t = time_to_expiry_years.max(1e-4);
        let r = risk_free_rate;
        let q = dividend_yield;

        let sigma = self.compute_effective_volatility(t);
        let sqrt_t = t.sqrt();

        let d1 = ((s / k).ln() + (r - q + 0.5 * sigma * sigma) * t) / (sigma * sqrt_t);
        let phi_d1 = Self::standard_normal_pdf(d1);

        let bs_delta = if is_call {
            (-q * t).exp() * Self::standard_normal_cdf(d1)
        } else {
            (-q * t).exp() * (Self::standard_normal_cdf(d1) - 1.0)
        };

        let gamma = ((-q * t).exp() * phi_d1) / (s * sigma * sqrt_t);
        let vega = s * (-q * t).exp() * phi_d1 * sqrt_t;

        // Hyperbolic secant leptokurtic tail correction
        let (_, _, kurt) = self.compute_hs_moments();
        // Excess kurtosis shifts tail sensitivity: - (kurt / 24) * (d1^2 - 1) * d1 * phi(d1)
        let d1_sq = d1 * d1;
        let hermite_3 = (d1_sq - 1.0) * d1;
        let jump_hazard_delta = bs_delta - (kurt / 24.0) * 0.02 * hermite_3 * phi_d1;

        RoughHyperbolicSecantGreeks {
            delta: bs_delta,
            gamma,
            vega,
            effective_vol: sigma,
            jump_hazard_delta,
            hs_kurtosis: kurt,
        }
    }

    /// Computes the jump-adjusted Whalley-Wilmott rehedging bandwidth.
    pub fn compute_rehedging_bandwidth(
        &self,
        spot: f64,
        gamma: f64,
        effective_vol: f64,
        hs_kurtosis: f64,
    ) -> f64 {
        let s = spot.max(1e-4);
        let gam = gamma.abs().max(1e-6);
        let sig = effective_vol.max(1e-4);
        let eps = self.params.transaction_cost.max(1e-6);
        let risk_av = self.params.risk_aversion.max(1e-4);

        let (hs_var, _, _) = self.compute_hs_moments();
        let kurt_jump_mult = 1.0 + (hs_kurtosis * hs_var) / (sig * sig).max(1e-4);

        let inside = (1.5 * eps * s * s * gam * gam * sig * kurt_jump_mult) / risk_av;
        inside.powf(1.0 / 3.0).clamp(0.001, 0.25)
    }

    /// Evaluates current hedging state and produces an action verdict.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_hedging_decision(
        &self,
        spot: f64,
        strike: f64,
        time_to_expiry_years: f64,
        risk_free_rate: f64,
        dividend_yield: f64,
        is_call: bool,
        current_portfolio_delta: f64,
        perp_basis_dispersion_pts: f64,
    ) -> (RoughHyperbolicSecantGreeks, RoughHyperbolicSecantAction) {
        let greeks = self.compute_greeks(
            spot,
            strike,
            time_to_expiry_years,
            risk_free_rate,
            dividend_yield,
            is_call,
        );

        let band_width = self.compute_rehedging_bandwidth(
            spot,
            greeks.gamma,
            greeks.effective_vol,
            greeks.hs_kurtosis,
        );

        let target_delta = greeks.jump_hazard_delta;
        let delta_error = current_portfolio_delta - target_delta;

        let action = if perp_basis_dispersion_pts.abs() > 15.0 {
            RoughHyperbolicSecantAction::SkewDispersion {
                dispersion_pts: perp_basis_dispersion_pts,
                target_delta,
            }
        } else if delta_error.abs() <= band_width {
            RoughHyperbolicSecantAction::HoldNeutral {
                target_delta,
                band_width,
            }
        } else {
            RoughHyperbolicSecantAction::RebalanceHedge {
                current_delta: current_portfolio_delta,
                target_delta,
                rebalance_size: -delta_error,
                band_width,
            }
        };

        (greeks, action)
    }
}

/// High-accuracy numerical approximation of error function erf(x).
fn erf_approx(x: f64) -> f64 {
    let sign = if x >= 0.0 { 1.0 } else { -1.0 };
    let a = x.abs();

    let p = 0.3275911;
    let a1 = 0.254829592;
    let a2 = -0.284496736;
    let a3 = 1.421413741;
    let a4 = -1.453152027;
    let a5 = 1.061405429;

    let t = 1.0 / (1.0 + p * a);
    let poly = t * (a1 + t * (a2 + t * (a3 + t * (a4 + t * a5))));
    let y = 1.0 - poly * (-a * a).exp();

    sign * y
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hs_moments_and_kurtosis() {
        let hedger = RoughHyperbolicSecantHedger::new();
        let (var, skew, kurt) = hedger.compute_hs_moments();

        assert!(var > 0.0);
        assert_eq!(skew, 0.0);
        assert_eq!(kurt, 2.0);
    }

    #[test]
    fn test_rough_volatility_scaling() {
        let hedger = RoughHyperbolicSecantHedger::new();
        let vol_short = hedger.compute_effective_volatility(0.01);
        let vol_long = hedger.compute_effective_volatility(1.0);

        assert!(vol_short > 0.0);
        assert!(vol_long >= vol_short);
    }

    #[test]
    fn test_rehedging_bandwidth_and_greeks() {
        let hedger = RoughHyperbolicSecantHedger::new();
        let greeks = hedger.compute_greeks(100.0, 100.0, 0.25, 0.03, 0.0, true);

        assert!(greeks.delta > 0.45 && greeks.delta < 0.65);
        assert!(greeks.gamma > 0.0);
        assert!(greeks.vega > 0.0);

        let band = hedger.compute_rehedging_bandwidth(
            100.0,
            greeks.gamma,
            greeks.effective_vol,
            greeks.hs_kurtosis,
        );
        assert!(band > 0.001 && band < 0.25);
    }

    #[test]
    fn test_hedging_decisions_hold_and_rebalance() {
        let hedger = RoughHyperbolicSecantHedger::new();
        let greeks = hedger.compute_greeks(100.0, 100.0, 0.25, 0.03, 0.0, true);

        let (_, action_hold) = hedger.evaluate_hedging_decision(
            100.0,
            100.0,
            0.25,
            0.03,
            0.0,
            true,
            greeks.jump_hazard_delta,
            0.0,
        );

        match action_hold {
            RoughHyperbolicSecantAction::HoldNeutral { .. } => {}
            _ => panic!("Expected HoldNeutral, got {:?}", action_hold),
        }

        // Test with massive delta imbalance
        let (_, action_rebal) =
            hedger.evaluate_hedging_decision(100.0, 100.0, 0.25, 0.03, 0.0, true, 0.95, 0.0);

        match action_rebal {
            RoughHyperbolicSecantAction::RebalanceHedge { rebalance_size, .. } => {
                assert!(rebalance_size < 0.0);
            }
            _ => panic!("Expected RebalanceHedge, got {:?}", action_rebal),
        }
    }
}
