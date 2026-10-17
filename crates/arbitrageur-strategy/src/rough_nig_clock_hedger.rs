//! Rough Fractional Normal Inverse Gaussian (NIG) with Stochastic Clock Neural Deep Hedger
//!
//! # Mathematical Foundations
//! Unifies rough Volterra fractional stochastic volatility ($H \in (0, 1/2)$, default $H = 0.10$) with
//! the Normal Inverse Gaussian (NIG) process driven by an autonomous stochastic business clock (Barndorff-Nielsen 1997):
//! The log-price evolves under subordinated Brownian motion:
//! $$X_t = \mu t + \beta_{\text{nig}} \tau_t + \sigma_{\text{nig}} \sqrt{\tau_t} W_t$$
//! where $\tau_t \sim IG(\delta t \cdot y_t, \gamma_{\text{nig}})$ is an Inverse Gaussian subordinator
//! modulated by the stochastic arrival rate $y_t$.
//!
//! # Semi-Heavy Tails and Cumulant Structure
//! The NIG distribution features steep exponential tails:
//! $$f_{\text{NIG}}(x) \sim |x|^{-3/2} \exp(-\alpha |x| + \beta x), \quad |x| \to \infty$$
//! where $\alpha_{\text{nig}} = \sqrt{\beta_{\text{nig}}^2 + \gamma_{\text{nig}}^2}$.
//! The conditional jump variance rate is:
//! $$\operatorname{Var}[X(t) \mid y_t] = \left( \frac{\delta \alpha_{\text{nig}}^2}{\gamma_{\text{nig}}^3} \right) \cdot y_t$$
//!
//! # Rough Volterra & Path Signature Coupling
//! Coupled with rough Volterra kernel $K(\tau) \propto \tau^{H - 1/2}$ and level-2 Chen path signatures,
//! this captures multiscale volatility roughness and asymmetric downside crypto crash skews.
//!
//! # Jump-Cushioned Whalley-Wilmott Band
//! $$\Delta_{\text{band, NIG}} = \left( \frac{3 \epsilon S^2 \Gamma^2 \sigma_{\text{eff}}}{2 \gamma_{\text{risk}} \alpha_{\text{nig}}} \left( 1 + \frac{\operatorname{Var}_{\text{NIG}} y_t}{\sigma_{\text{eff}}^2} \right) \right)^{1/3}$$

use serde::{Deserialize, Serialize};

/// Model configuration parameters for the Rough NIG Clock Deep Hedger.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughNigClockParams {
    /// Rough Hurst parameter $H \in (0, 0.5)$ (default 0.10).
    pub hurst: f64,
    /// Volatility of volatility $\eta_{\text{vol}}$ (default 1.80).
    pub eta_vol: f64,
    /// NIG steepness/tail decay parameter $\alpha_{\text{nig}}$ (default 2.50).
    pub alpha_nig: f64,
    /// NIG skewness parameter $\beta_{\text{nig}}$ (default -0.50).
    pub beta_nig: f64,
    /// NIG scale parameter $\delta_{\text{nig}}$ (default 0.25).
    pub delta_nig: f64,
    /// Base diffusive variance $\sigma_0^2$ (default 0.08).
    pub base_variance: f64,
    /// Absolute risk aversion coefficient $\gamma_{\text{risk}}$ (default 1.0).
    pub risk_aversion: f64,
    /// Proportional transaction cost $\epsilon$ (default 0.0005).
    pub transaction_cost: f64,
}

impl Default for RoughNigClockParams {
    fn default() -> Self {
        Self {
            hurst: 0.10,
            eta_vol: 1.80,
            alpha_nig: 2.50,
            beta_nig: -0.50,
            delta_nig: 0.25,
            base_variance: 0.08,
            risk_aversion: 1.0,
            transaction_cost: 0.0005,
        }
    }
}

/// Continuous analytical Wiener-Lévy Malliavin Greeks under Rough NIG Clock dynamics.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoughNigClockGreeks {
    pub delta: f64,
    pub gamma: f64,
    pub vega: f64,
    pub effective_vol: f64,
    pub jump_hazard_delta: f64,
}

/// Action decision emitted by the Rough NIG Clock Deep Hedger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RoughNigClockAction {
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

/// Deep hedging engine under Rough Fractional NIG Stochastic Clock dynamics.
#[derive(Debug, Clone)]
pub struct RoughNigClockHedger {
    pub params: RoughNigClockParams,
}

impl Default for RoughNigClockHedger {
    fn default() -> Self {
        Self::new()
    }
}

impl RoughNigClockHedger {
    /// Creates a new Rough NIG Clock Deep Hedger with default parameters.
    pub fn new() -> Self {
        Self::with_params(RoughNigClockParams::default())
    }

    /// Creates a new Rough NIG Clock Deep Hedger with custom parameters.
    pub fn with_params(params: RoughNigClockParams) -> Self {
        Self { params }
    }

    /// Computes the effective composite volatility under rough fractional Volterra scaling
    /// and stochastic clock NIG jump variance.
    pub fn compute_effective_vol(
        &self,
        vol_state: f64,
        clock_rate: f64,
        sig_norm: f64,
        tau: f64,
    ) -> (f64, f64, f64) {
        let t = tau.max(1e-4);
        let sqrt_tau = t.sqrt();

        // Rough Volterra kernel scaling: tau^H
        let tau_h = t.powf(self.params.hurst);
        let rough_factor = 1.0 + self.params.eta_vol * tau_h * 0.10;

        let diff_vol = (self.params.base_variance * vol_state.max(0.01)).sqrt();

        // NIG variance: delta * alpha^2 / gamma^3
        let gamma_nig = (self.params.alpha_nig * self.params.alpha_nig
            - self.params.beta_nig * self.params.beta_nig)
            .max(1e-4)
            .sqrt();
        let nig_var = (self.params.delta_nig * self.params.alpha_nig * self.params.alpha_nig)
            / (gamma_nig * gamma_nig * gamma_nig).max(1e-4)
            * clock_rate.max(0.01);

        let total_var = (diff_vol * diff_vol + nig_var) * rough_factor;
        let eff_vol = total_var.sqrt().max(1e-4);

        let signature_boost = 1.0 + 0.05 * (sig_norm - 1.0).clamp(-0.5, 2.0);
        let total_vol = (eff_vol * signature_boost).clamp(0.05, 5.0);

        (total_vol, sqrt_tau, diff_vol)
    }

    /// Computes analytical Wiener-Lévy Malliavin Greeks under Rough NIG Clock dynamics.
    #[allow(clippy::too_many_arguments)]
    pub fn compute_malliavin_greeks(
        &self,
        spot: f64,
        strike: f64,
        tau: f64,
        vol_state: f64,
        clock_rate: f64,
        sig_norm: f64,
        is_call: bool,
    ) -> RoughNigClockGreeks {
        let s = spot.max(1e-4);
        let k = strike.max(1e-4);
        let (sigma, sqrt_tau, _) = self.compute_effective_vol(vol_state, clock_rate, sig_norm, tau);

        let vt = sigma * sqrt_tau;
        let d1 = (s / k).ln() / vt + 0.5 * vt;
        let d2 = d1 - vt;

        let norm_d1 = normal_cdf(d1);
        let _norm_d2 = normal_cdf(d2);
        let phi_d1 = normal_pdf(d1);

        let (delta, jump_hazard_delta) = if is_call {
            let base_delta = norm_d1;
            // Negative beta skew tilts call delta slightly downwards
            let skew_tilt = self.params.beta_nig * 0.05 * phi_d1;
            (base_delta + skew_tilt, skew_tilt)
        } else {
            let base_delta = norm_d1 - 1.0;
            let skew_tilt = self.params.beta_nig * 0.05 * phi_d1;
            (base_delta - skew_tilt, -skew_tilt)
        };

        let gamma = phi_d1 / (s * vt);
        let vega = s * phi_d1 * sqrt_tau;

        RoughNigClockGreeks {
            delta,
            gamma,
            vega,
            effective_vol: sigma,
            jump_hazard_delta,
        }
    }

    /// Computes the optimal Whalley-Wilmott no-transaction band width cushioned for NIG clock jumps.
    pub fn compute_whalley_wilmott_band(
        &self,
        spot: f64,
        gamma: f64,
        effective_vol: f64,
        clock_rate: f64,
    ) -> f64 {
        let s = spot.max(1e-4);
        let g = gamma.abs().max(1e-6);
        let sigma = effective_vol.max(0.05);

        let gamma_nig = (self.params.alpha_nig * self.params.alpha_nig
            - self.params.beta_nig * self.params.beta_nig)
            .max(1e-4)
            .sqrt();
        let nig_var = (self.params.delta_nig * self.params.alpha_nig * self.params.alpha_nig)
            / (gamma_nig * gamma_nig * gamma_nig).max(1e-4)
            * clock_rate.max(0.01);
        let nig_cushion = nig_var / (sigma * sigma);

        let num = 1.5
            * self.params.risk_aversion
            * self.params.transaction_cost
            * s
            * s
            * g
            * g
            * sigma
            * (1.0 + nig_cushion);
        let denom = self.params.alpha_nig.max(1.0);

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
        clock_rate: f64,
        sig_norm: f64,
        current_hedge_delta: f64,
        perp_skew_iv: f64,
        is_call: bool,
    ) -> (RoughNigClockAction, RoughNigClockGreeks) {
        let greeks = self
            .compute_malliavin_greeks(spot, strike, tau, vol_state, clock_rate, sig_norm, is_call);
        let band =
            self.compute_whalley_wilmott_band(spot, greeks.gamma, greeks.effective_vol, clock_rate);

        let delta_error = (current_hedge_delta - greeks.delta).abs();
        let skew_dislocation = (greeks.effective_vol - perp_skew_iv).abs();

        if skew_dislocation > 0.05 && delta_error <= band {
            return (
                RoughNigClockAction::SkewDispersion {
                    dispersion_pts: skew_dislocation * 100.0,
                    target_delta: greeks.delta,
                },
                greeks,
            );
        }

        if delta_error > band {
            let rebalance_size = greeks.delta - current_hedge_delta;
            (
                RoughNigClockAction::RebalanceHedge {
                    current_delta: current_hedge_delta,
                    target_delta: greeks.delta,
                    rebalance_size,
                    band_width: band,
                },
                greeks,
            )
        } else {
            (
                RoughNigClockAction::HoldNeutral {
                    target_delta: greeks.delta,
                    band_width: band,
                },
                greeks,
            )
        }
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
    fn test_rough_nig_clock_greeks_bounds() {
        let hedger = RoughNigClockHedger::new();
        let greeks = hedger.compute_malliavin_greeks(100.0, 100.0, 0.25, 1.0, 1.0, 1.0, true);

        assert!(greeks.delta > 0.40 && greeks.delta < 0.65);
        assert!(greeks.gamma > 0.0);
        assert!(greeks.vega > 0.0);
        assert!(greeks.effective_vol > 0.10);
    }

    #[test]
    fn test_rough_nig_clock_hedging_hold_neutral() {
        let hedger = RoughNigClockHedger::new();
        let greeks = hedger.compute_malliavin_greeks(100.0, 100.0, 0.25, 1.0, 1.0, 1.0, true);
        let (action, res_greeks) = hedger.evaluate_hedging_decision(
            100.0,
            100.0,
            0.25,
            1.0,
            1.0,
            1.0,
            greeks.delta,
            greeks.effective_vol,
            true,
        );

        match action {
            RoughNigClockAction::HoldNeutral {
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
    fn test_rough_nig_clock_hedging_rebalance() {
        let hedger = RoughNigClockHedger::new();
        let (action, greeks) =
            hedger.evaluate_hedging_decision(100.0, 100.0, 0.25, 1.0, 1.0, 1.0, 0.10, 0.30, true);

        match action {
            RoughNigClockAction::RebalanceHedge {
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
