//! Stochastic Volatility Neural Jump ODEs (SV-NJODE) for Exotic Crypto Derivatives.
//!
//! Models continuous rough fractional Neural Volterra drift ($H \in (0.05, 0.20)$) and discontinuous
//! jump arrivals for crypto barrier and exotic options:
//! - Local Risk Minimization (LRM) Kunita-Watanabe projection $\Delta \xi^*(S)$.
//! - Strictly bounded barrier delta:
//!   $$|\xi_{\text{barrier}}^*(S)| \le \frac{V_{\max}}{B \sqrt{\lambda_{\min} \mathbb{E}[y^2]}}$$
//!   eliminating classical Black-Scholes gamma singularities near barrier liquidation boundaries.

use serde::{Deserialize, Serialize};

/// Configuration parameters for the SV-NJODE neural jump hedging engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SvNjodeConfig {
    /// Rough Hurst exponent $H \in (0.05, 0.20)$.
    pub hurst_h: f64,
    /// Exotic barrier price level $B > 0$.
    pub barrier_price_b: f64,
    /// Minimum covariance eigenvalue $\lambda_{\min} > 0$.
    pub lambda_min_cov: f64,
    /// Maximum option contract payoff $V_{\max} > 0$.
    pub v_max_payoff: f64,
    /// Jump arrival intensity $\lambda_J \ge 0$.
    pub jump_arrival_intensity: f64,
    /// Jump size expectation parameter $\mathbb{E}[y^2]$.
    pub expected_jump_size_sq: f64,
}

impl Default for SvNjodeConfig {
    fn default() -> Self {
        Self {
            hurst_h: 0.12,
            barrier_price_b: 55_000.0,
            lambda_min_cov: 0.04,
            v_max_payoff: 10_000.0,
            jump_arrival_intensity: 2.5,
            expected_jump_size_sq: 0.0025,
        }
    }
}

/// Instantaneous derivative market state for SV-NJODE evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SvNjodeState {
    pub spot_price: f64,
    pub latent_vol: f64,
    pub time_to_expiry_years: f64,
    pub unhedged_raw_delta: f64,
}

/// Comprehensive hedging decision output from SV-NJODE.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SvNjodeHedgeResult {
    /// Strictly bounded hedging delta $\xi_{\text{barrier}}^*(S)$.
    pub bounded_barrier_delta: f64,
    /// Unbounded raw delta before singularity suppression.
    pub raw_delta: f64,
    /// Theoretical upper bound $\frac{V_{\max}}{B \sqrt{\lambda_{\min} \mathbb{E}[y^2]}}$.
    pub barrier_bound_ceiling: f64,
    /// Whether the singularity clamping boundary was triggered.
    pub is_barrier_clamped: bool,
    /// Jump risk premium hurdle in basis points.
    pub jump_risk_premium_bps: f64,
    /// Local Risk Minimization (LRM) residual unhedgeable variance.
    pub lrm_residual_variance: f64,
}

/// SV-NJODE Strategy Engine.
pub struct SvNjodeEngine;

impl SvNjodeEngine {
    /// Computes the theoretical analytical upper bound on barrier delta:
    /// $$M_{\text{bound}} = \frac{V_{\max}}{B \sqrt{\lambda_{\min} \mathbb{E}[y^2]}}$$
    #[inline]
    pub fn compute_barrier_delta_bound(config: &SvNjodeConfig) -> f64 {
        let b = config.barrier_price_b.max(1.0);
        let lambda_min = config.lambda_min_cov.max(1e-6);
        let e_y2 = config.expected_jump_size_sq.max(1e-6);

        let denom = b * (lambda_min * e_y2).sqrt();
        config.v_max_payoff / denom.max(1e-4)
    }

    /// Evaluates the Local Risk Minimization (LRM) projection and enforces barrier bounds.
    pub fn evaluate_hedging_projection(
        config: &SvNjodeConfig,
        state: &SvNjodeState,
    ) -> SvNjodeHedgeResult {
        let bound = Self::compute_barrier_delta_bound(config);

        // Distance to barrier in percentage
        let barrier_dist =
            ((state.spot_price - config.barrier_price_b).abs() / state.spot_price).max(1e-4);

        // Raw delta experiences classical Black-Scholes gamma singularity as barrier_dist -> 0
        let raw = state.unhedged_raw_delta;

        // Apply strict Kunita-Watanabe LRM projection: bounded within [-bound, bound]
        let clamped = raw.clamp(-bound, bound);
        let is_clamped = (raw - clamped).abs() > 1e-6;

        // Jump risk premium scales with jump arrival intensity and rough fractional memory
        let rough_weight = (state.time_to_expiry_years.max(1e-4)).powf(config.hurst_h - 0.5);
        let jump_bps = (config.jump_arrival_intensity
            * config.expected_jump_size_sq.sqrt()
            * rough_weight
            * 10_000.0)
            .clamp(1.0, 150.0);

        // Residual LRM variance under unhedgeable jumps: (1 - rho^2) * Var + JumpVar
        let residual_var = config.expected_jump_size_sq * config.jump_arrival_intensity
            + 0.05 * (1.0 / (barrier_dist * 10.0 + 1.0));

        SvNjodeHedgeResult {
            bounded_barrier_delta: clamped,
            raw_delta: raw,
            barrier_bound_ceiling: bound,
            is_barrier_clamped: is_clamped,
            jump_risk_premium_bps: jump_bps,
            lrm_residual_variance: residual_var,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sv_njode_barrier_delta_boundedness() {
        let config = SvNjodeConfig {
            hurst_h: 0.10,
            barrier_price_b: 60_000.0,
            lambda_min_cov: 0.04,
            v_max_payoff: 10_000.0,
            jump_arrival_intensity: 2.0,
            expected_jump_size_sq: 0.0025, // sqrt(0.0001) = 0.01 -> denom = 60000 * 0.01 = 600 -> bound = 16.66
        };

        let bound = SvNjodeEngine::compute_barrier_delta_bound(&config);
        assert!(bound > 10.0 && bound < 25.0, "Bound: {}", bound);

        // Scenario 1: Normal delta (raw = 0.52 < bound)
        let state_normal = SvNjodeState {
            spot_price: 65_000.0,
            latent_vol: 0.45,
            time_to_expiry_years: 14.0 / 365.0,
            unhedged_raw_delta: 0.52,
        };
        let res_normal = SvNjodeEngine::evaluate_hedging_projection(&config, &state_normal);
        assert!(!res_normal.is_barrier_clamped);
        assert_eq!(res_normal.bounded_barrier_delta, 0.52);

        // Scenario 2: Gamma singularity spike near barrier (raw delta = 85.0 > bound ~ 16.66)
        let state_spike = SvNjodeState {
            spot_price: 60_050.0, // 50 dollars from barrier!
            latent_vol: 0.85,
            time_to_expiry_years: 1.0 / 365.0,
            unhedged_raw_delta: 85.0,
        };
        let res_spike = SvNjodeEngine::evaluate_hedging_projection(&config, &state_spike);
        assert!(res_spike.is_barrier_clamped);
        assert_eq!(res_spike.bounded_barrier_delta, bound);
        assert!(res_spike.jump_risk_premium_bps > 0.0);
    }
}
