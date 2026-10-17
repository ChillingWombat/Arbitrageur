//! High-Dimensional Factor Statistical Arbitrage & Hamilton 2-State Regime-Switching Kalman Filter.
//!
//! Provides:
//! - Hamilton 2-state regime-switching Kalman filter for tracking Ornstein-Uhlenbeck residual spreads:
//!   $$ds_i(t) = \kappa_{S_t} (\theta_{S_t} - s_i(t)) dt + \sigma_{S_t} dW_t$$
//!   distinguishing between a stationary Mean-Reverting regime ($\kappa_1, \theta_1, \sigma_1$) and
//!   a Trending/Breakdown regime ($\kappa_2, \theta_2, \sigma_2$) with Kim variance collapsing.
//! - Factor-Neutral Kelly Portfolio Allocator with orthogonal null-space projection onto $\ker(\mathbf{B}^T)$,
//!   enforcing zero systematic factor risk ($\mathbf{B}^T \mathbf{w} = \mathbf{0}$) and gross leverage bounds.

use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// Hamilton 2-State Regime-Switching Kalman Filter for idiosyncratic residual spread tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HamiltonKalmanSpreadFilter {
    /// Transition probability P(S_t = 1 | S_{t-1} = 1) (mean-reverting persistence)
    pub p11: f64,
    /// Transition probability P(S_t = 2 | S_{t-1} = 2) (trending/breakdown persistence)
    pub p22: f64,
    /// Regime 1 (Mean-Reverting) parameters: [kappa_1, theta_1, sigma_1]
    pub kappa1: f64,
    pub theta1: f64,
    pub sigma1: f64,
    /// Regime 2 (Trending/Divergent) parameters: [kappa_2, theta_2, sigma_2]
    pub kappa2: f64,
    pub theta2: f64,
    pub sigma2: f64,
    /// Measurement noise variance R
    pub measurement_noise_r: f64,
    /// State estimates [x_hat_1, x_hat_2]
    pub x_hat: [f64; 2],
    /// State error covariance [P_1, P_2]
    pub p_cov: [f64; 2],
    /// Filtered posterior probability of Regime 1: P(S_t = 1 | Y_t)
    pub prob_regime1: f64,
}

impl HamiltonKalmanSpreadFilter {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kappa1: f64,
        theta1: f64,
        sigma1: f64,
        kappa2: f64,
        theta2: f64,
        sigma2: f64,
        p11: f64,
        p22: f64,
    ) -> Self {
        Self {
            p11: p11.clamp(0.50, 0.999),
            p22: p22.clamp(0.50, 0.999),
            kappa1: kappa1.max(1e-4),
            theta1,
            sigma1: sigma1.max(1e-5),
            kappa2: kappa2.max(1e-6),
            theta2,
            sigma2: sigma2.max(1e-5),
            measurement_noise_r: 1e-4,
            x_hat: [0.0, 0.0],
            p_cov: [1e-3, 1e-2],
            prob_regime1: 0.90, // Confident prior in mean-reversion
        }
    }

    /// Evaluates 1D Gaussian probability density N(x; mu, var)
    #[inline]
    fn gaussian_pdf(x: f64, mu: f64, var: f64) -> f64 {
        let v = var.max(1e-12);
        let diff = x - mu;
        (1.0 / (2.0 * PI * v).sqrt()) * (-0.5 * diff * diff / v).exp()
    }

    /// Updates the Hamilton-Kalman filter with a new residual spread observation s_t over time step dt.
    /// Returns the updated posterior probability of the mean-reverting regime P(S_t = 1 | Y_t).
    pub fn update(&mut self, s_t: f64, dt: f64) -> f64 {
        if !s_t.is_finite() || !dt.is_finite() || dt <= 0.0 {
            return self.prob_regime1;
        }

        // 1. Regime transition priors
        let pi1_prior = self.p11 * self.prob_regime1 + (1.0 - self.p22) * (1.0 - self.prob_regime1);
        let pi2_prior = 1.0 - pi1_prior;

        // 2. State predictions for Regime 1 and Regime 2
        // a_j = exp(-kappa_j * dt), Q_j = (sigma_j^2 / (2 * kappa_j)) * (1 - exp(-2 * kappa_j * dt))
        let a1 = (-self.kappa1 * dt).exp();
        let q1 =
            (self.sigma1.powi(2) / (2.0 * self.kappa1)) * (1.0 - (-2.0 * self.kappa1 * dt).exp());
        let x_pred1 = a1 * self.x_hat[0] + (1.0 - a1) * self.theta1;
        let p_pred1 = a1 * a1 * self.p_cov[0] + q1;

        let a2 = (-self.kappa2 * dt).exp();
        let q2 =
            (self.sigma2.powi(2) / (2.0 * self.kappa2)) * (1.0 - (-2.0 * self.kappa2 * dt).exp());
        let x_pred2 = a2 * self.x_hat[1] + (1.0 - a2) * self.theta2;
        let p_pred2 = a2 * a2 * self.p_cov[1] + q2;

        // 3. Innovations and innovation variances
        let innov1 = s_t - x_pred1;
        let f1 = p_pred1 + self.measurement_noise_r;

        let innov2 = s_t - x_pred2;
        let f2 = p_pred2 + self.measurement_noise_r;

        // 4. Emission densities
        let eta1 = Self::gaussian_pdf(innov1, 0.0, f1);
        let eta2 = Self::gaussian_pdf(innov2, 0.0, f2);

        // 5. Posterior Bayesian regime update
        let num1 = pi1_prior * eta1;
        let num2 = pi2_prior * eta2;
        let denom = num1 + num2;

        if denom > 1e-30 {
            self.prob_regime1 = (num1 / denom).clamp(0.0001, 0.9999);
        }

        // 6. Kalman measurement updates
        let k1 = p_pred1 / f1;
        self.x_hat[0] = x_pred1 + k1 * innov1;
        self.p_cov[0] = (1.0 - k1) * p_pred1;

        let k2 = p_pred2 / f2;
        self.x_hat[1] = x_pred2 + k2 * innov2;
        self.p_cov[1] = (1.0 - k2) * p_pred2;

        self.prob_regime1
    }

    /// Computes regime-weighted expected alpha: E[ds] / dt.
    /// Returns 0.0 if trending/breakdown regime dominates (P(Regime 1) < 0.70).
    pub fn expected_alpha(&self, s_t: f64) -> f64 {
        if self.prob_regime1 < 0.70 {
            // Unsafe: trending/breakdown regime dominant
            0.0
        } else {
            self.prob_regime1 * self.kappa1 * (self.theta1 - s_t)
        }
    }

    /// Returns true if the mean-reverting regime is confidently active (P(Regime 1) >= hurdle).
    pub fn is_mean_reverting(&self, hurdle: f64) -> bool {
        self.prob_regime1 >= hurdle.clamp(0.50, 0.99)
    }
}

/// Dynamic Factor Neutral Kelly Allocator with Orthogonal Projection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactorNeutralKellyAllocator {
    pub half_kelly_mult: f64,
    pub max_gross_leverage: f64,
    pub min_regime_prob: f64,
}

impl Default for FactorNeutralKellyAllocator {
    fn default() -> Self {
        Self {
            half_kelly_mult: 0.50,
            max_gross_leverage: 2.0,
            min_regime_prob: 0.75,
        }
    }
}

impl FactorNeutralKellyAllocator {
    pub fn new(half_kelly_mult: f64, max_gross_leverage: f64, min_regime_prob: f64) -> Self {
        Self {
            half_kelly_mult: half_kelly_mult.clamp(0.10, 1.0),
            max_gross_leverage: max_gross_leverage.clamp(0.50, 10.0),
            min_regime_prob: min_regime_prob.clamp(0.50, 0.99),
        }
    }

    /// Computes factor-neutral portfolio weights w* orthogonal to loadings B (N x K).
    /// Enforces B^T w* = 0 using projection P_perp = I - B (B^T B)^-1 B^T.
    #[allow(clippy::needless_range_loop)]
    pub fn allocate(
        &self,
        alphas: &[f64],               // N-dim expected alphas
        variances: &[f64],            // N-dim idiosyncratic variances sigma_epsilon^2
        regime_probs: &[f64],         // N-dim regime probabilities pi_1
        factor_loadings: &[Vec<f64>], // N x K matrix
    ) -> Vec<f64> {
        let n = alphas.len();
        if n == 0 || factor_loadings.len() != n {
            return vec![];
        }
        let k = factor_loadings[0].len();

        // 1. Identify active assets with healthy mean-reverting regime probability
        let mut active_indices = Vec::with_capacity(n);
        for i in 0..n {
            if regime_probs[i] >= self.min_regime_prob {
                active_indices.push(i);
            }
        }
        if active_indices.is_empty() {
            return vec![0.0; n];
        }

        // 2. Unconstrained mean-variance weights on active assets: w_raw = alpha / var
        let mut w_proj = vec![0.0; n];
        for &i in &active_indices {
            let var = variances[i].max(1e-6);
            w_proj[i] = alphas[i] / var;
        }

        // 3. Project out factor exposure restricted strictly to active assets:
        // Enforces sum_{i in active} B_i w_i = 0, leaving inactive assets at 0.0 weight.
        if k == 1 {
            let mut dot_bw = 0.0;
            let mut dot_bb = 0.0;
            for &i in &active_indices {
                let b_i = factor_loadings[i][0];
                dot_bw += b_i * w_proj[i];
                dot_bb += b_i * b_i;
            }
            if dot_bb > 1e-8 {
                let beta_factor = dot_bw / dot_bb;
                for &i in &active_indices {
                    w_proj[i] -= factor_loadings[i][0] * beta_factor;
                }
            }
        } else if k > 1 {
            for factor_idx in 0..k {
                let mut dot_bw = 0.0;
                let mut dot_bb = 0.0;
                for &i in &active_indices {
                    let b_i = factor_loadings[i][factor_idx];
                    dot_bw += b_i * w_proj[i];
                    dot_bb += b_i * b_i;
                }
                if dot_bb > 1e-8 {
                    let beta_factor = dot_bw / dot_bb;
                    for &i in &active_indices {
                        w_proj[i] -= factor_loadings[i][factor_idx] * beta_factor;
                    }
                }
            }
        }

        // 4. Apply Half-Kelly and Leverage Cap
        let gross_exposure: f64 = w_proj.iter().map(|w| w.abs()).sum();
        if gross_exposure > 1e-8 {
            let scale = (self.half_kelly_mult * self.max_gross_leverage / gross_exposure).min(1.0);
            for w in &mut w_proj {
                *w *= scale;
            }
        }

        w_proj
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hamilton_kalman_filter_mean_reverting_vs_breakout() {
        // Fast mean reverting regime: kappa1 = 12.0, theta1 = 0.0, sigma1 = 0.02
        // Trending regime: kappa2 = 0.01, theta2 = 0.5, sigma2 = 0.15
        let mut filter =
            HamiltonKalmanSpreadFilter::new(12.0, 0.0, 0.02, 0.01, 0.5, 0.15, 0.98, 0.98);

        assert!(filter.is_mean_reverting(0.80));

        // Feed small mean-reverting observations around 0.0
        let dt = 1.0 / 365.25; // daily observation
        for _ in 0..20 {
            filter.update(0.005, dt);
            filter.update(-0.005, dt);
        }
        assert!(filter.prob_regime1 > 0.85);
        assert!(filter.expected_alpha(0.05) < 0.0); // positive spread generates negative alpha (mean revert down)
        assert!(filter.expected_alpha(-0.05) > 0.0); // negative spread generates positive alpha (mean revert up)

        // Now simulate a violent structural breakout: spread jumps to 0.35 and stays there
        for _ in 0..15 {
            filter.update(0.35, dt);
        }
        // Posterior probability of mean-reverting regime should collapse
        assert!(filter.prob_regime1 < 0.30);
        assert!(!filter.is_mean_reverting(0.70));
        assert_eq!(filter.expected_alpha(0.35), 0.0); // Alpha halted on breakout
    }

    #[test]
    fn test_factor_neutral_kelly_allocator() {
        let allocator = FactorNeutralKellyAllocator::default();

        let alphas = vec![0.08, -0.06, 0.04];
        let variances = vec![0.01, 0.015, 0.02];
        let regime_probs = vec![0.92, 0.88, 0.95]; // all healthy
        let factor_loadings = vec![
            vec![1.2], // Asset 0 beta
            vec![0.8], // Asset 1 beta
            vec![1.5], // Asset 2 beta
        ];

        let weights = allocator.allocate(&alphas, &variances, &regime_probs, &factor_loadings);
        assert_eq!(weights.len(), 3);

        // Verify factor neutrality: dot(loadings, weights) ~ 0
        let net_beta: f64 = weights
            .iter()
            .zip(factor_loadings.iter())
            .map(|(w, b)| w * b[0])
            .sum();
        assert!(
            net_beta.abs() < 1e-6,
            "Factor neutrality violated: net beta = {net_beta}"
        );

        // Verify gross leverage is capped at max_gross_leverage
        let gross_lev: f64 = weights.iter().map(|w| w.abs()).sum();
        assert!(gross_lev <= allocator.max_gross_leverage + 1e-6);

        // If one asset breaks out into low regime prob, its allocation drops to zero
        let regime_probs_breakout = vec![0.92, 0.40, 0.95]; // Asset 1 broke out
        let weights_gated = allocator.allocate(
            &alphas,
            &variances,
            &regime_probs_breakout,
            &factor_loadings,
        );
        assert!(
            weights_gated[1].abs() < 1e-6,
            "Asset 1 should have zero allocation on breakdown"
        );
    }
}
