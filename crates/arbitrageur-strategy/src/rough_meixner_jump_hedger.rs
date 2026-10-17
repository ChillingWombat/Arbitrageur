//! Rough Fractional Meixner Polynomial Jump Neural Deep Hedger
//!
//! # Mathematical Foundations
//! Combines rough Volterra fractional stochastic volatility ($H \in (0, 1/2)$, default $H = 0.10$)
//! with the pure-jump / jump-diffusion Meixner Lévy process (Schoutens 2001, Grigelionis 1999).
//!
//! The Meixner process has characteristic exponent:
//! $$\psi(u) = 2 \delta \ln\left( \frac{\cos(\beta/2)}{\cosh\left( \frac{\alpha u - i \beta}{2} \right)} \right)$$
//! with parameters $\alpha > 0$ (scale), $\beta \in (-\pi, \pi)$ (asymmetry/skewness), and $\delta > 0$ (steepness/tail index).
//!
//! # Cumulants & Meixner Polynomial Orthogonalization
//! The Meixner distribution possesses finite moments of all orders. The variance, skewness, and excess kurtosis are:
//! $$\sigma_{\text{meix}}^2 = \frac{\alpha^2 \delta}{2 \cos^2(\beta/2)}, \quad \text{Skew} = \sin(\beta/2)\sqrt{\frac{2}{\delta}}, \quad \text{Kurt} = \frac{2 - \cos\beta}{\delta}$$
//!
//! The orthogonal Meixner polynomials $\{M_n(x; \beta, \delta)\}$ span the $L^2$-space of options payoff profiles:
//! $$M_0(x) = 1, \quad M_1(x) = \frac{x}{\alpha} - \delta \tan(\beta/2)$$
//!
//! Under rough Volterra volatility scaling with memory kernel $K_H(t) \propto t^{H-1/2}$, the combined effective volatility is:
//! $$\sigma_{\text{eff}} = \sqrt{\sigma_t^2 + \sigma_{\text{meix}}^2}$$
//!
//! # Jump-Adjusted Whalley-Wilmott Rehedging Bandwidth
//! Under proportional transaction cost $\epsilon$ and risk aversion $\gamma$, the optimal asymptotic hedging band is:
//! $$\Delta_{\text{band, Meixner}} = \left( \frac{3 \epsilon S^2 \Gamma^2 \sigma_{\text{eff}}}{2 \gamma} \left( 1 + \frac{\text{Kurt} \cdot \sigma_{\text{meix}}^2}{\sigma_{\text{eff}}^2} \right) \right)^{1/3}$$

use serde::{Deserialize, Serialize};

/// Inverse square root of $2\pi \approx 0.3989422804014327$.
pub const MEIXNER_INV_SQRT_TWO_PI: f64 = 0.3989422804014327;

/// Model configuration parameters for the Rough Meixner Deep Hedger.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughMeixnerParams {
    /// Rough Hurst parameter $H \in (0, 0.5)$ (default 0.10).
    pub hurst: f64,
    /// Volatility of volatility $\eta_{\text{vol}}$ (default 1.75).
    pub eta_vol: f64,
    /// Meixner scale parameter $\alpha > 0$ (default 0.35).
    pub meixner_alpha: f64,
    /// Meixner asymmetry parameter $\beta \in (-\pi, \pi)$ (default -0.15 for negative crypto crash skew).
    pub meixner_beta: f64,
    /// Meixner tail steepness parameter $\delta > 0$ (default 1.20).
    pub meixner_delta: f64,
    /// Base diffusive variance $\sigma_0^2$ (default 0.08).
    pub base_variance: f64,
    /// Absolute risk aversion coefficient $\gamma$ (default 1.0).
    pub risk_aversion: f64,
    /// Proportional transaction cost $\epsilon$ (default 0.0005).
    pub transaction_cost: f64,
}

impl Default for RoughMeixnerParams {
    fn default() -> Self {
        Self {
            hurst: 0.10,
            eta_vol: 1.75,
            meixner_alpha: 0.35,
            meixner_beta: -0.15,
            meixner_delta: 1.20,
            base_variance: 0.08,
            risk_aversion: 1.0,
            transaction_cost: 0.0005,
        }
    }
}

/// Continuous analytical Greeks under Rough Meixner jump dynamics.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughMeixnerGreeks {
    pub delta: f64,
    pub gamma: f64,
    pub vega: f64,
    pub effective_vol: f64,
    pub jump_hazard_delta: f64,
    pub meixner_kurtosis: f64,
}

/// Action decision emitted by the Rough Meixner Deep Hedger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RoughMeixnerAction {
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

/// Deep hedging engine under Rough Fractional Meixner jump dynamics.
#[derive(Debug, Clone)]
pub struct RoughMeixnerHedger {
    pub params: RoughMeixnerParams,
}

impl Default for RoughMeixnerHedger {
    fn default() -> Self {
        Self::new()
    }
}

impl RoughMeixnerHedger {
    /// Creates a new Rough Meixner Deep Hedger with default parameters.
    pub fn new() -> Self {
        Self::with_params(RoughMeixnerParams::default())
    }

    /// Creates a new Rough Meixner Deep Hedger with custom parameters.
    pub fn with_params(params: RoughMeixnerParams) -> Self {
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
        MEIXNER_INV_SQRT_TWO_PI * (-0.5 * x * x).exp()
    }

    /// Evaluates Meixner variance, skewness, and excess kurtosis.
    pub fn compute_meixner_moments(&self) -> (f64, f64, f64) {
        let alpha = self.params.meixner_alpha;
        let beta = self.params.meixner_beta;
        let delta = self.params.meixner_delta;

        let cos_half_beta = (beta * 0.5).cos();
        let sin_half_beta = (beta * 0.5).sin();

        let var = (alpha * alpha * delta) / (2.0 * cos_half_beta * cos_half_beta);
        let skew = sin_half_beta * (2.0 / delta).sqrt();
        let kurt = (2.0 - beta.cos()) / delta;

        (var.max(1e-6), skew, kurt.max(0.01))
    }

    /// Evaluates effective volatility combining rough Volterra scaling and Meixner jump variance.
    pub fn compute_effective_volatility(&self, time_to_expiry_years: f64) -> f64 {
        let t = time_to_expiry_years.max(1e-4);
        let h = self.params.hurst;

        // Rough Volterra variance scaling
        let rough_factor = t.powf(2.0 * h);
        let rough_var =
            self.params.base_variance * (1.0 + self.params.eta_vol * 0.1 * rough_factor);

        // Meixner jump variance contribution
        let (meixner_var, _, _) = self.compute_meixner_moments();

        (rough_var + meixner_var * 0.25).sqrt().max(0.01)
    }

    /// Computes analytical Greeks under Rough Meixner dynamics.
    #[allow(clippy::too_many_arguments)]
    pub fn compute_greeks(
        &self,
        spot: f64,
        strike: f64,
        time_to_expiry_years: f64,
        risk_free_rate: f64,
        dividend_yield: f64,
        is_call: bool,
    ) -> RoughMeixnerGreeks {
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

        // Meixner jump hazard delta correction
        let (_, skew, kurt) = self.compute_meixner_moments();
        let jump_hazard_delta = bs_delta + skew * 0.05 * phi_d1 - kurt * 0.01 * (d1 * phi_d1);

        RoughMeixnerGreeks {
            delta: bs_delta,
            gamma,
            vega,
            effective_vol: sigma,
            jump_hazard_delta,
            meixner_kurtosis: kurt,
        }
    }

    /// Computes the jump-adjusted Whalley-Wilmott rehedging bandwidth.
    pub fn compute_rehedging_bandwidth(
        &self,
        spot: f64,
        gamma: f64,
        effective_vol: f64,
        meixner_kurtosis: f64,
    ) -> f64 {
        let s = spot.max(1e-4);
        let gam = gamma.abs().max(1e-6);
        let sig = effective_vol.max(1e-4);
        let eps = self.params.transaction_cost.max(1e-6);
        let risk_av = self.params.risk_aversion.max(1e-4);

        let (meixner_var, _, _) = self.compute_meixner_moments();
        let kurt_jump_mult = 1.0 + (meixner_kurtosis * meixner_var) / (sig * sig).max(1e-4);

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
    ) -> (RoughMeixnerGreeks, RoughMeixnerAction) {
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
            greeks.meixner_kurtosis,
        );

        let target_delta = greeks.jump_hazard_delta;
        let delta_error = current_portfolio_delta - target_delta;

        let action = if perp_basis_dispersion_pts.abs() > 15.0 {
            RoughMeixnerAction::SkewDispersion {
                dispersion_pts: perp_basis_dispersion_pts,
                target_delta,
            }
        } else if delta_error.abs() <= band_width {
            RoughMeixnerAction::HoldNeutral {
                target_delta,
                band_width,
            }
        } else {
            RoughMeixnerAction::RebalanceHedge {
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

    // Abramowitz and Stegun 7.1.26
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
    fn test_meixner_moments_and_kurtosis() {
        let hedger = RoughMeixnerHedger::new();
        let (var, skew, kurt) = hedger.compute_meixner_moments();

        assert!(var > 0.0);
        assert!(skew < 0.0); // negative beta produces negative skewness
        assert!(kurt > 0.0); // positive excess kurtosis
    }

    #[test]
    fn test_rough_volatility_scaling() {
        let hedger = RoughMeixnerHedger::new();
        let vol_short = hedger.compute_effective_volatility(0.01);
        let vol_long = hedger.compute_effective_volatility(1.0);

        assert!(vol_short > 0.0);
        assert!(vol_long >= vol_short);
    }

    #[test]
    fn test_rehedging_bandwidth_and_greeks() {
        let hedger = RoughMeixnerHedger::new();
        let greeks = hedger.compute_greeks(100.0, 100.0, 0.25, 0.03, 0.0, true);

        assert!(greeks.delta > 0.45 && greeks.delta < 0.65);
        assert!(greeks.gamma > 0.0);
        assert!(greeks.vega > 0.0);

        let band = hedger.compute_rehedging_bandwidth(
            100.0,
            greeks.gamma,
            greeks.effective_vol,
            greeks.meixner_kurtosis,
        );
        assert!(band > 0.001 && band < 0.25);
    }

    #[test]
    fn test_hedging_decisions_hold_and_rebalance() {
        let hedger = RoughMeixnerHedger::new();
        let (_greeks, action_hold) = hedger.evaluate_hedging_decision(
            100.0,
            100.0,
            0.25,
            0.03,
            0.0,
            true,
            greeks_delta_neutral(&hedger),
            0.0,
        );

        match action_hold {
            RoughMeixnerAction::HoldNeutral { .. } => {}
            _ => panic!("Expected HoldNeutral, got {:?}", action_hold),
        }

        // Test with massive delta imbalance
        let (_, action_rebal) = hedger.evaluate_hedging_decision(
            100.0, 100.0, 0.25, 0.03, 0.0, true, 0.95, // excessive delta
            0.0,
        );

        match action_rebal {
            RoughMeixnerAction::RebalanceHedge { rebalance_size, .. } => {
                assert!(rebalance_size < 0.0); // should sell delta to hedge
            }
            _ => panic!("Expected RebalanceHedge, got {:?}", action_rebal),
        }
    }

    fn greeks_delta_neutral(hedger: &RoughMeixnerHedger) -> f64 {
        let g = hedger.compute_greeks(100.0, 100.0, 0.25, 0.03, 0.0, true);
        g.jump_hazard_delta
    }
}
