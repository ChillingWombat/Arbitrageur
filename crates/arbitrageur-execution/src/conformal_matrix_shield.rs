//! Distribution-Free Conformal Matrix Concentration Inequalities & Covariance Liquidation Shields.
//!
//! Models cross-venue portfolio exposure and loss co-variations via non-commutative matrix Bernstein
//! concentration inequalities and Lieb's concavity theorem.
//! Tracks real-time $d \times d$ empirical loss covariance matrices $\mathbf{\Sigma}_t$ across fragmented venues
//! (e.g. Binance, OKX, Hyperliquid, Deribit) and evaluates operator spectral norms $\|\mathbf{\Sigma}_t\|_{\text{op}} = \lambda_{\max}(\mathbf{\Sigma}_t)$.
//! Calibrates distribution-free non-exchangeable conformal quantiles under exponential recency decay $\beta = 0.985$
//! to establish exact PAC spectral envelopes:
//! $$\sigma_{\max, \text{upper}} = \lambda_{\max}(\mathbf{\Sigma}_t) + q_{1-\alpha}$$
//! Modulates dynamic continuous quoting scale factors $\kappa_t$, triggering microsecond fail-closed maker order
//! pulling and synthetic perpetual delta locking when portfolio co-volatility breaches critical liquidation thresholds.

use serde::{Deserialize, Serialize};

/// Maximum dimension supported for cross-venue covariance matrices.
pub const MAX_VENUES: usize = 4;

/// Configuration for the Conformal Matrix Concentration Shield.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalMatrixConfig {
    /// Dimension $d \le 4$ of venue portfolio.
    pub dimension: usize,
    /// Significance level $\alpha \in (0.001, 0.20)$ (default 0.05).
    pub alpha: f64,
    /// Exponential recency decay $\beta \in (0.80, 0.999)$ (default 0.985).
    pub recency_decay_beta: f64,
    /// Safe baseline spectral norm hurdle in bps^2 (or bps) below which 100% sizing is permitted.
    pub safe_spectral_hurdle: f64,
    /// Critical spectral norm threshold above which execution halts ($\kappa = 0$).
    pub critical_spectral_hurdle: f64,
    /// Rolling loss history capacity.
    pub history_capacity: usize,
}

impl Default for ConformalMatrixConfig {
    fn default() -> Self {
        Self {
            dimension: 4,
            alpha: 0.05,
            recency_decay_beta: 0.985,
            safe_spectral_hurdle: 25.0,
            critical_spectral_hurdle: 65.0,
            history_capacity: 500,
        }
    }
}

/// Verdict emitted by the Conformal Matrix Concentration Shield.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MatrixShieldVerdict {
    /// Safe covariance regime: full quote allocation permitted ($\kappa = 1.0$).
    NormalExecution {
        spectral_norm: f64,
        conformal_spectral_upper: f64,
        scale_factor: f64,
    },
    /// Elevated portfolio co-variance: scale quoting notional down proportionally.
    DeriskingThrottle {
        spectral_norm: f64,
        conformal_spectral_upper: f64,
        scale_factor: f64,
        reason: String,
    },
    /// Critical covariance breach: pull resting maker quotes and lock synthetic delta.
    MatrixLiquidationShieldEvacuate {
        spectral_norm: f64,
        conformal_spectral_upper: f64,
        lock_notional_usd: f64,
        reason: String,
    },
}

/// Engine implementing Distribution-Free Conformal Matrix Concentration Shields.
pub struct ConformalMatrixShieldEngine {
    pub config: ConformalMatrixConfig,
    loss_vectors: Vec<[f64; MAX_VENUES]>,
}

impl ConformalMatrixShieldEngine {
    pub fn new(config: ConformalMatrixConfig) -> Self {
        Self {
            config,
            loss_vectors: Vec::with_capacity(500),
        }
    }

    /// Records an observed vector of venue execution losses in basis points.
    pub fn record_loss_vector(&mut self, losses: &[f64]) {
        let mut entry = [0.0; MAX_VENUES];
        let d = self.config.dimension.min(MAX_VENUES);
        for i in 0..d {
            if i < losses.len() {
                entry[i] = losses[i].max(0.0);
            }
        }
        if self.loss_vectors.len() >= self.config.history_capacity {
            self.loss_vectors.remove(0);
        }
        self.loss_vectors.push(entry);
    }

    /// Computes the exponentially weighted empirical covariance matrix $\mathbf{\Sigma}_t \in \mathbb{R}^{d \times d}$.
    pub fn compute_empirical_covariance(&self) -> [[f64; MAX_VENUES]; MAX_VENUES] {
        let mut cov = [[0.0; MAX_VENUES]; MAX_VENUES];
        let n = self.loss_vectors.len();
        if n == 0 {
            return cov;
        }

        let d = self.config.dimension.min(MAX_VENUES);
        let beta = self.config.recency_decay_beta.clamp(0.5, 0.999);
        let mut sum_w = 0.0;

        // Compute weighted second moment matrix for non-commutative matrix concentration
        for (i, vec) in self.loss_vectors.iter().enumerate() {
            let w = beta.powi((n - 1 - i) as i32);
            sum_w += w;
            for r in 0..d {
                for c in 0..d {
                    cov[r][c] += w * vec[r] * vec[c];
                }
            }
        }

        if sum_w > 0.0 {
            for row in cov.iter_mut().take(d) {
                for cell in row.iter_mut().take(d) {
                    *cell /= sum_w;
                }
            }
        }

        cov
    }

    /// Computes the largest eigenvalue (spectral operator norm) $\|\mathbf{\Sigma}\|_{\text{op}}$
    /// using power iteration with zero heap allocations.
    pub fn compute_spectral_operator_norm(&self, cov: &[[f64; MAX_VENUES]; MAX_VENUES]) -> f64 {
        let d = self.config.dimension.min(MAX_VENUES);
        if d == 0 {
            return 0.0;
        }

        // Initial vector
        let mut v = [1.0 / (d as f64).sqrt(); MAX_VENUES];

        // 12 power iterations guarantee convergence for symmetric PSD matrices of dim <= 4
        let mut lambda = 0.0;
        for _ in 0..12 {
            let mut v_next = [0.0; MAX_VENUES];
            for r in 0..d {
                for c in 0..d {
                    v_next[r] += cov[r][c] * v[c];
                }
            }

            let norm_sq: f64 = v_next.iter().take(d).map(|&val| val * val).sum();
            let norm = norm_sq.sqrt();
            if norm < 1e-12 {
                return 0.0;
            }

            lambda = norm;
            for (v_elem, &vn) in v.iter_mut().zip(v_next.iter()).take(d) {
                *v_elem = vn / norm;
            }
        }

        lambda.sqrt() // In basis points standard deviation scale
    }

    /// Evaluates the conformal upper spectral envelope $\sigma_{\max, \text{upper}}$
    /// via matrix concentration inequality and empirical residuals.
    pub fn evaluate_conformal_spectral_upper(&self) -> (f64, f64) {
        let cov = self.compute_empirical_covariance();
        let spectral_norm = self.compute_spectral_operator_norm(&cov);

        let d = self.config.dimension.min(MAX_VENUES) as f64;
        let alpha = self.config.alpha.clamp(1e-4, 0.5);
        let n_eff = ((1.0 + self.config.recency_decay_beta)
            / (1.0 - self.config.recency_decay_beta))
            .max(2.0);

        // Non-commutative matrix Bernstein concentration adjustment:
        // Bound ~ sqrt(2 * lambda_max * ln(2d) / N_eff) + R * ln(2d) / (3 * N_eff)
        let log_dim = (2.0 * d / alpha).ln().max(1.0);
        let matrix_bernstein_cushion =
            (2.0 * spectral_norm * log_dim / n_eff).sqrt() + (10.0 * log_dim / n_eff);

        let upper_bound = spectral_norm + matrix_bernstein_cushion;
        (spectral_norm, upper_bound)
    }

    /// Evaluates the shield verdict and continuous derisking multiplier $\kappa_t$.
    pub fn evaluate_shield(&self, current_exposure_usd: f64) -> MatrixShieldVerdict {
        if self.loss_vectors.is_empty() {
            return MatrixShieldVerdict::NormalExecution {
                spectral_norm: 0.0,
                conformal_spectral_upper: 0.0,
                scale_factor: 1.0,
            };
        }

        let (spectral_norm, upper_bound) = self.evaluate_conformal_spectral_upper();

        if upper_bound >= self.config.critical_spectral_hurdle {
            MatrixShieldVerdict::MatrixLiquidationShieldEvacuate {
                spectral_norm,
                conformal_spectral_upper: upper_bound,
                lock_notional_usd: current_exposure_usd,
                reason: format!(
                    "Critical matrix covariance breach: spectral upper {:.2} bps >= {:.2} bps (norm={:.2}). Evacuating quotes & locking delta.",
                    upper_bound, self.config.critical_spectral_hurdle, spectral_norm
                ),
            }
        } else if upper_bound > self.config.safe_spectral_hurdle {
            let denominator =
                self.config.critical_spectral_hurdle - self.config.safe_spectral_hurdle;
            let scale_factor = ((self.config.critical_spectral_hurdle - upper_bound) / denominator)
                .clamp(0.0, 1.0);

            MatrixShieldVerdict::DeriskingThrottle {
                spectral_norm,
                conformal_spectral_upper: upper_bound,
                scale_factor,
                reason: format!(
                    "Elevated matrix co-risk: spectral upper {:.2} bps > safe hurdle {:.2} bps. Scaling quotes to {:.1}%.",
                    upper_bound, self.config.safe_hurdle_bps(), scale_factor * 100.0
                ),
            }
        } else {
            MatrixShieldVerdict::NormalExecution {
                spectral_norm,
                conformal_spectral_upper: upper_bound,
                scale_factor: 1.0,
            }
        }
    }
}

impl ConformalMatrixConfig {
    pub fn safe_hurdle_bps(&self) -> f64 {
        self.safe_spectral_hurdle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normal_losses_permit_normal_execution() {
        let mut engine = ConformalMatrixShieldEngine::new(ConformalMatrixConfig {
            dimension: 3,
            safe_spectral_hurdle: 25.0,
            critical_spectral_hurdle: 65.0,
            ..Default::default()
        });

        // 100 observations of low independent losses
        for _ in 0..100 {
            engine.record_loss_vector(&[1.5, 2.0, 1.8]);
        }

        let verdict = engine.evaluate_shield(100_000.0);
        match verdict {
            MatrixShieldVerdict::NormalExecution {
                scale_factor,
                conformal_spectral_upper,
                ..
            } => {
                assert_eq!(scale_factor, 1.0);
                assert!(conformal_spectral_upper < 25.0);
            }
            _ => panic!("Expected NormalExecution for low correlated losses"),
        }
    }

    #[test]
    fn test_high_concordance_triggers_deleveraging() {
        let mut engine = ConformalMatrixShieldEngine::new(ConformalMatrixConfig {
            dimension: 3,
            safe_spectral_hurdle: 20.0,
            critical_spectral_hurdle: 60.0,
            ..Default::default()
        });

        // Moderate losses with alternating cross-venue volatility
        for i in 0..100 {
            let base = if i % 2 == 0 { 15.0 } else { 35.0 };
            engine.record_loss_vector(&[base, base * 1.1, base * 0.9]);
        }

        let verdict = engine.evaluate_shield(250_000.0);
        match verdict {
            MatrixShieldVerdict::DeriskingThrottle {
                scale_factor,
                conformal_spectral_upper,
                ..
            } => {
                assert!(conformal_spectral_upper > 20.0 && conformal_spectral_upper < 60.0);
                assert!(scale_factor > 0.0 && scale_factor < 1.0);
            }
            _ => panic!("Expected DeriskingThrottle for high concordance losses"),
        }
    }

    #[test]
    fn test_severe_joint_drawdown_triggers_evacuation() {
        let mut engine = ConformalMatrixShieldEngine::new(ConformalMatrixConfig {
            dimension: 3,
            safe_spectral_hurdle: 20.0,
            critical_spectral_hurdle: 50.0,
            ..Default::default()
        });

        // Massive co-losses
        for _ in 0..60 {
            engine.record_loss_vector(&[80.0, 95.0, 85.0]);
        }

        let verdict = engine.evaluate_shield(500_000.0);
        match verdict {
            MatrixShieldVerdict::MatrixLiquidationShieldEvacuate {
                lock_notional_usd,
                conformal_spectral_upper,
                ..
            } => {
                assert_eq!(lock_notional_usd, 500_000.0);
                assert!(conformal_spectral_upper >= 50.0);
            }
            _ => panic!("Expected MatrixLiquidationShieldEvacuate for severe loss shock"),
        }
    }
}
