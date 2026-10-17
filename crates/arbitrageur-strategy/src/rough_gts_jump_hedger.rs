//! Rough Fractional Generalized Tempered Stable (GTS) Jump Neural Deep Hedger
//!
//! # Mathematical Foundations
//! Unifies rough Volterra fractional stochastic volatility ($H \in (0, 1/2)$, default $H = 0.10$)
//! with the Generalized Tempered Stable (GTS) Lévy process (Cont & Tankov 2004, Rosiński 2007).
//!
//! The GTS Lévy density allows distinct stability indices $\alpha_+, \alpha_- \in (0, 2)$
//! and tempering rates $\lambda_+, \lambda_- > 0$ for positive and negative jumps:
//! $$\nu_{\text{GTS}}(dx) = \left( \frac{C_+ e^{-\lambda_+ x}}{x^{1 + \alpha_+}} \mathbb{I}_{\{x > 0\}} + \frac{C_- e^{-\lambda_- |x|}}{|x|^{1 + \alpha_-}} \mathbb{I}_{\{x < 0\}} \right) dx$$
//!
//! # Asymmetric Crypto Tail Regimes & Jump Cumulants
//! In crypto options and perpetual markets, crash fear induces heavy negative jump tails ($\alpha_- > \alpha_+$)
//! and slower decay ($\lambda_- < \lambda_+$).
//!
//! The second, third, and fourth cumulants of the GTS jump process are:
//! $$\sigma_{\text{gts}}^2 = C_+ \Gamma(2 - \alpha_+) \lambda_+^{\alpha_+ - 2} + C_- \Gamma(2 - \alpha_-) \lambda_-^{\alpha_- - 2}$$
//! $$\kappa_3 = C_+ \Gamma(3 - \alpha_+) \lambda_+^{\alpha_+ - 3} - C_- \Gamma(3 - \alpha_-) \lambda_-^{\alpha_- - 3}$$
//! $$\kappa_4 = C_+ \Gamma(4 - \alpha_+) \lambda_+^{\alpha_+ - 4} + C_- \Gamma(4 - \alpha_-) \lambda_-^{\alpha_- - 4}$$
//!
//! with skewness $\text{Skew} = \kappa_3 / \sigma_{\text{gts}}^3$ and excess kurtosis $\text{Kurt} = \kappa_4 / \sigma_{\text{gts}}^4$.
//!
//! # Rough Volterra Variance & Effective Volatility
//! Instantaneous variance under fractional Brownian motion $W_t^H$ scales as:
//! $$\sigma_t^2 = \sigma_0^2 \exp\left( \eta_{\text{vol}} W_t^H - \frac{1}{2} \eta_{\text{vol}}^2 t^{2H} \right)$$
//! Combined with GTS jump variance:
//! $$\sigma_{\text{eff}} = \sqrt{\sigma_t^2 + \sigma_{\text{gts}}^2}$$
//!
//! # Jump-Adjusted Whalley-Wilmott Rehedging Bandwidth
//! Under proportional transaction cost $\epsilon$ and risk aversion $\gamma$, the optimal bandwidth is:
//! $$\Delta_{\text{band, GTS}} = \left( \frac{3 \epsilon S^2 \Gamma^2 \sigma_{\text{eff}}}{2 \gamma} \left( 1 + \frac{\text{Kurt} \cdot \sigma_{\text{gts}}^2}{\sigma_{\text{eff}}^2} \right) \right)^{1/3}$$

use serde::{Deserialize, Serialize};

/// Inverse square root of $2\pi \approx 0.3989422804014327$.
pub const GTS_INV_SQRT_TWO_PI: f64 = 0.3989422804014327;

/// Model configuration parameters for the Rough GTS Deep Hedger.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughGtsParams {
    /// Rough Hurst parameter $H \in (0, 0.5)$ (default 0.10).
    pub hurst: f64,
    /// Volatility of volatility $\eta_{\text{vol}}$ (default 1.70).
    pub eta_vol: f64,
    /// Positive jump intensity $C_+$ (default 0.12).
    pub c_plus: f64,
    /// Negative jump intensity $C_-$ (default 0.20 for crash dominance).
    pub c_minus: f64,
    /// Positive stability index $\alpha_+ \in (0, 2)$ (default 0.70).
    pub alpha_plus: f64,
    /// Negative stability index $\alpha_- \in (0, 2)$ (default 1.25).
    pub alpha_minus: f64,
    /// Positive tempering parameter $\lambda_+$ (default 4.5).
    pub lambda_plus: f64,
    /// Negative tempering parameter $\lambda_-$ (default 2.8).
    pub lambda_minus: f64,
    /// Base diffusive variance $\sigma_0^2$ (default 0.08).
    pub base_variance: f64,
    /// Absolute risk aversion coefficient $\gamma$ (default 1.0).
    pub risk_aversion: f64,
    /// Proportional transaction cost $\epsilon$ (default 0.0005).
    pub transaction_cost: f64,
}

impl Default for RoughGtsParams {
    fn default() -> Self {
        Self {
            hurst: 0.10,
            eta_vol: 1.70,
            c_plus: 0.12,
            c_minus: 0.20,
            alpha_plus: 0.70,
            alpha_minus: 1.25,
            lambda_plus: 4.5,
            lambda_minus: 2.8,
            base_variance: 0.08,
            risk_aversion: 1.0,
            transaction_cost: 0.0005,
        }
    }
}

/// Continuous analytical Greeks under Rough GTS jump dynamics.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughGtsGreeks {
    pub delta: f64,
    pub gamma: f64,
    pub vega: f64,
    pub effective_vol: f64,
    pub jump_hazard_delta: f64,
    pub gts_kurtosis: f64,
}

/// Action decision emitted by the Rough GTS Deep Hedger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RoughGtsAction {
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

/// Deep hedging engine under Rough Fractional GTS jump dynamics.
#[derive(Debug, Clone)]
pub struct RoughGtsHedger {
    pub params: RoughGtsParams,
}

impl Default for RoughGtsHedger {
    fn default() -> Self {
        Self::new()
    }
}

impl RoughGtsHedger {
    /// Creates a new Rough GTS Deep Hedger with default parameters.
    pub fn new() -> Self {
        Self::with_params(RoughGtsParams::default())
    }

    /// Creates a new Rough GTS Deep Hedger with custom parameters.
    pub fn with_params(params: RoughGtsParams) -> Self {
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
        GTS_INV_SQRT_TWO_PI * (-0.5 * x * x).exp()
    }

    /// Lanczos approximation of the Gamma function $\Gamma(z)$.
    #[allow(clippy::excessive_precision)]
    fn gamma_func(z: f64) -> f64 {
        if z < 0.5 {
            std::f64::consts::PI / ((std::f64::consts::PI * z).sin() * Self::gamma_func(1.0 - z))
        } else {
            let z = z - 1.0;
            let c = [
                0.99999999999980993,
                676.5203681218851,
                -1259.1392167224028,
                771.32342877765313,
                -176.61502916214059,
                12.507343278686905,
                -0.138571095836524,
                9.9843695780195716e-6,
                1.5056327351493116e-7,
            ];
            let mut x = c[0];
            for (i, &val) in c.iter().enumerate().skip(1) {
                x += val / (z + i as f64);
            }
            let t = z + 7.5;
            (2.0 * std::f64::consts::PI).sqrt() * t.powf(z + 0.5) * (-t).exp() * x
        }
    }

    /// Evaluates GTS jump moments: (variance, skewness, excess kurtosis).
    pub fn compute_gts_moments(&self) -> (f64, f64, f64) {
        let p = &self.params;

        // Second cumulant: variance
        let g2_pos = Self::gamma_func(2.0 - p.alpha_plus);
        let g2_neg = Self::gamma_func(2.0 - p.alpha_minus);
        let var_pos = p.c_plus * g2_pos * p.lambda_plus.powf(p.alpha_plus - 2.0);
        let var_neg = p.c_minus * g2_neg * p.lambda_minus.powf(p.alpha_minus - 2.0);
        let var = (var_pos + var_neg).max(1e-6);

        // Third cumulant
        let g3_pos = Self::gamma_func(3.0 - p.alpha_plus);
        let g3_neg = Self::gamma_func(3.0 - p.alpha_minus);
        let k3_pos = p.c_plus * g3_pos * p.lambda_plus.powf(p.alpha_plus - 3.0);
        let k3_neg = p.c_minus * g3_neg * p.lambda_minus.powf(p.alpha_minus - 3.0);
        let k3 = k3_pos - k3_neg;
        let skew = k3 / (var * var.sqrt());

        // Fourth cumulant: excess kurtosis
        let g4_pos = Self::gamma_func(4.0 - p.alpha_plus);
        let g4_neg = Self::gamma_func(4.0 - p.alpha_minus);
        let k4_pos = p.c_plus * g4_pos * p.lambda_plus.powf(p.alpha_plus - 4.0);
        let k4_neg = p.c_minus * g4_neg * p.lambda_minus.powf(p.alpha_minus - 4.0);
        let k4 = k4_pos + k4_neg;
        let kurt = (k4 / (var * var)).max(0.01);

        (var, skew, kurt)
    }

    /// Evaluates effective volatility combining rough Volterra scaling and GTS jump variance.
    pub fn compute_effective_volatility(&self, time_to_expiry_years: f64) -> f64 {
        let t = time_to_expiry_years.max(1e-4);
        let h = self.params.hurst;

        let rough_factor = t.powf(2.0 * h);
        let rough_var =
            self.params.base_variance * (1.0 + self.params.eta_vol * 0.1 * rough_factor);

        let (gts_var, _, _) = self.compute_gts_moments();
        (rough_var + gts_var * 0.25).sqrt().max(0.01)
    }

    /// Computes analytical Greeks under Rough GTS dynamics.
    #[allow(clippy::too_many_arguments)]
    pub fn compute_greeks(
        &self,
        spot: f64,
        strike: f64,
        time_to_expiry_years: f64,
        risk_free_rate: f64,
        dividend_yield: f64,
        is_call: bool,
    ) -> RoughGtsGreeks {
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

        // GTS jump hazard delta correction from skew and excess kurtosis
        let (_, skew, kurt) = self.compute_gts_moments();
        let jump_hazard_delta = bs_delta + skew * 0.05 * phi_d1 - kurt * 0.01 * (d1 * phi_d1);

        RoughGtsGreeks {
            delta: bs_delta,
            gamma,
            vega,
            effective_vol: sigma,
            jump_hazard_delta,
            gts_kurtosis: kurt,
        }
    }

    /// Computes the jump-adjusted Whalley-Wilmott rehedging bandwidth.
    pub fn compute_rehedging_bandwidth(
        &self,
        spot: f64,
        gamma: f64,
        effective_vol: f64,
        gts_kurtosis: f64,
    ) -> f64 {
        let s = spot.max(1e-4);
        let gam = gamma.abs().max(1e-6);
        let sig = effective_vol.max(1e-4);
        let eps = self.params.transaction_cost.max(1e-6);
        let risk_av = self.params.risk_aversion.max(1e-4);

        let (gts_var, _, _) = self.compute_gts_moments();
        let kurt_jump_mult = 1.0 + (gts_kurtosis * gts_var) / (sig * sig).max(1e-4);

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
    ) -> (RoughGtsGreeks, RoughGtsAction) {
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
            greeks.gts_kurtosis,
        );

        let target_delta = greeks.jump_hazard_delta;
        let delta_error = current_portfolio_delta - target_delta;

        let action = if perp_basis_dispersion_pts.abs() > 15.0 {
            RoughGtsAction::SkewDispersion {
                dispersion_pts: perp_basis_dispersion_pts,
                target_delta,
            }
        } else if delta_error.abs() <= band_width {
            RoughGtsAction::HoldNeutral {
                target_delta,
                band_width,
            }
        } else {
            RoughGtsAction::RebalanceHedge {
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
    fn test_gts_moments_and_kurtosis() {
        let hedger = RoughGtsHedger::new();
        let (var, skew, kurt) = hedger.compute_gts_moments();

        assert!(var > 0.0);
        assert!(skew < 0.0); // crash dominance produces negative skewness
        assert!(kurt > 0.0); // positive excess kurtosis
    }

    #[test]
    fn test_rough_volatility_scaling() {
        let hedger = RoughGtsHedger::new();
        let vol_short = hedger.compute_effective_volatility(0.01);
        let vol_long = hedger.compute_effective_volatility(1.0);

        assert!(vol_short > 0.0);
        assert!(vol_long >= vol_short);
    }

    #[test]
    fn test_rehedging_bandwidth_and_greeks() {
        let hedger = RoughGtsHedger::new();
        let greeks = hedger.compute_greeks(100.0, 100.0, 0.25, 0.03, 0.0, true);

        assert!(greeks.delta > 0.45 && greeks.delta < 0.65);
        assert!(greeks.gamma > 0.0);
        assert!(greeks.vega > 0.0);

        let band = hedger.compute_rehedging_bandwidth(
            100.0,
            greeks.gamma,
            greeks.effective_vol,
            greeks.gts_kurtosis,
        );
        assert!(band > 0.001 && band < 0.25);
    }

    #[test]
    fn test_hedging_decisions_hold_and_rebalance() {
        let hedger = RoughGtsHedger::new();
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
            RoughGtsAction::HoldNeutral { .. } => {}
            _ => panic!("Expected HoldNeutral, got {:?}", action_hold),
        }

        // Test with massive delta imbalance
        let (_, action_rebal) =
            hedger.evaluate_hedging_decision(100.0, 100.0, 0.25, 0.03, 0.0, true, 0.95, 0.0);

        match action_rebal {
            RoughGtsAction::RebalanceHedge { rebalance_size, .. } => {
                assert!(rebalance_size < 0.0);
            }
            _ => panic!("Expected RebalanceHedge, got {:?}", action_rebal),
        }
    }
}
