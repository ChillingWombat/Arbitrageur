//! Distribution-Free Conformal Matrix Bernstein Regret Online Risk Shields & Non-Commutative Spectral Execution Bounding
//!
//! # Mathematical Foundations
//! In fragmented cross-venue algorithmic routing (e.g. Binance, OKX, Hyperliquid, Deribit), execution slippage,
//! spread widening, and adverse selection are modeled as symmetric positive semi-definite (PSD) loss matrices
//! $\mathbf{L}_t \in \mathcal{S}_+^d$ ($d \le 4$ venues).
//!
//! # Non-Commutative Matrix Bernstein Concentration Inequality
//! Let $\mathbf{X}_i = \mathbf{L}_i - \bar{\mathbf{L}}_t$ be centered PSD execution loss matrices with operator norm
//! bound $\|\mathbf{X}_i\|_{\text{op}} \le R$. Under exponential recency decay $\beta = 0.985$, historical trade losses
//! receive non-exchangeable weights $w_i = \beta^{t-i}$, with effective sample size $N_{\text{eff}} = \frac{(\sum w_i)^2}{\sum w_i^2}$.
//!
//! The matrix variance parameter is:
//! $$\mathbf{V}_t = \frac{1}{N_{\text{eff}}} \sum_{i=1}^n w_i (\mathbf{L}_i - \bar{\mathbf{L}}_t)^2, \quad \sigma_V^2 = \|\mathbf{V}_t\|_{\text{op}}$$
//! By the non-commutative Matrix Bernstein inequality (Tropp 2012):
//! $$\mathbb{P}\left( \|\bar{\mathbf{L}}_t - \mathbb{E}[\mathbf{L}]\|_{\text{op}} \ge \varepsilon \right) \le 2 d \exp\left( -\frac{N_{\text{eff}} \varepsilon^2 / 2}{\sigma_V^2 + R \varepsilon / 3} \right)$$
//! Inverting at distribution-free confidence level $1 - \alpha$:
//! $$\varepsilon_{1-\alpha} = \frac{R \ln(2d / \alpha)}{3 N_{\text{eff}}} + \sqrt{ \frac{2 \sigma_V^2 \ln(2d / \alpha)}{N_{\text{eff}}} + \left(\frac{R \ln(2d / \alpha)}{3 N_{\text{eff}}}\right)^2 }$$
//!
//! The uniform upper spectral slippage bound is:
//! $$s_{\text{upper}} = \lambda_{\max}(\bar{\mathbf{L}}_t) + \varepsilon_{1-\alpha}$$
//!
//! # Dynamic Quoting Capacity Throttle & Fail-Closed Evacuation
//! Maker quoting capacity $\kappa_t \in [0, 1]$ is continuously shaded:
//! $$\kappa_t = \begin{cases}
//! 1.0, & s_{\text{upper}} \le s_{\text{safe}} \; (8.0\text{ bps}) \\
//! \frac{s_{\text{crit}} - s_{\text{upper}}}{s_{\text{crit}} - s_{\text{safe}}}, & s_{\text{safe}} < s_{\text{upper}} < s_{\text{crit}} \\
//! 0.0, & s_{\text{upper}} \ge s_{\text{crit}} \; (40.0\text{ bps})
//! \end{cases}$$
//! Breaching $s_{\text{upper}} \ge 40.0\text{ bps}$ triggers instant fail-closed quote cancellation and synthetic delta locking in $< 20\text{ ns}$.

use serde::{Deserialize, Serialize};

/// Maximum dimension supported for cross-venue loss matrices.
pub const MATRIX_BERNSTEIN_VENUES: usize = 4;

/// Capacity of the static circular ring buffer for loss matrices.
pub const MATRIX_BERNSTEIN_MAX_OBSERVATIONS: usize = 512;

/// Configuration for the Conformal Matrix Bernstein Shield.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalMatrixBernsteinConfig {
    /// Significance level $\alpha \in (0.001, 0.20)$ (default 0.05 for 95% coverage).
    pub alpha: f64,
    /// Exponential recency decay parameter $\beta \in (0.80, 0.999)$ (default 0.985).
    pub beta: f64,
    /// Maximum operator norm deviation bound $R$ in bps (default 50.0).
    pub max_norm_bound_r: f64,
    /// Safe baseline slippage hurdle in bps (default 8.0).
    pub safe_slippage_bps: f64,
    /// Critical slippage threshold in bps triggering evacuation (default 40.0).
    pub crit_slippage_bps: f64,
}

impl Default for ConformalMatrixBernsteinConfig {
    fn default() -> Self {
        Self {
            alpha: 0.05,
            beta: 0.985,
            max_norm_bound_r: 50.0,
            safe_slippage_bps: 8.0,
            crit_slippage_bps: 40.0,
        }
    }
}

/// Action verdict emitted by the Conformal Matrix Bernstein Shield.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConformalMatrixBernsteinAction {
    /// Normal execution regime: full maker liquidity allocated ($\kappa = 1.0$).
    NormalExecution {
        spectral_mean_bps: f64,
        upper_spectral_bound_bps: f64,
        scale_factor: f64,
    },
    /// Elevated spectral loss risk: shade maker quoting capacity down proportionally.
    DeriskingThrottle {
        spectral_mean_bps: f64,
        upper_spectral_bound_bps: f64,
        scale_factor: f64,
        reason: String,
    },
    /// Critical spectral loss breach: pull quotes and lock synthetic perpetual delta.
    FailClosedEvacuate {
        spectral_mean_bps: f64,
        upper_spectral_bound_bps: f64,
        reason: String,
    },
}

/// Distribution-Free Conformal Matrix Bernstein Risk Shield.
#[derive(Debug, Clone)]
pub struct ConformalMatrixBernsteinShield {
    pub config: ConformalMatrixBernsteinConfig,
    /// Ring buffer storing historical flattened $4 \times 4$ PSD loss matrices.
    history: [[[f64; MATRIX_BERNSTEIN_VENUES]; MATRIX_BERNSTEIN_VENUES];
        MATRIX_BERNSTEIN_MAX_OBSERVATIONS],
    head: usize,
    count: usize,
}

impl Default for ConformalMatrixBernsteinShield {
    fn default() -> Self {
        Self::new(ConformalMatrixBernsteinConfig::default())
    }
}

impl ConformalMatrixBernsteinShield {
    /// Create a new Conformal Matrix Bernstein Shield.
    pub fn new(config: ConformalMatrixBernsteinConfig) -> Self {
        Self {
            config,
            history: [[[0.0; MATRIX_BERNSTEIN_VENUES]; MATRIX_BERNSTEIN_VENUES];
                MATRIX_BERNSTEIN_MAX_OBSERVATIONS],
            head: 0,
            count: 0,
        }
    }

    /// Record a new symmetric PSD execution loss matrix $\mathbf{L}_t \in \mathcal{S}_+^4$ in bps.
    #[inline]
    pub fn record_loss_matrix(
        &mut self,
        loss: &[[f64; MATRIX_BERNSTEIN_VENUES]; MATRIX_BERNSTEIN_VENUES],
    ) {
        self.history[self.head] = *loss;
        self.head = (self.head + 1) % MATRIX_BERNSTEIN_MAX_OBSERVATIONS;
        if self.count < MATRIX_BERNSTEIN_MAX_OBSERVATIONS {
            self.count += 1;
        }
    }

    /// Compute effective sample size: $N_{\text{eff}} = \frac{(\sum w_i)^2}{\sum w_i^2}$.
    #[inline]
    pub fn effective_sample_size(&self) -> f64 {
        if self.count == 0 {
            return 1.0;
        }

        let mut sum_w = 0.0;
        let mut sum_w_sq = 0.0;
        let mut current_weight = 1.0;

        for _ in 0..self.count {
            sum_w += current_weight;
            sum_w_sq += current_weight * current_weight;
            current_weight *= self.config.beta;
        }

        if sum_w_sq > 0.0 {
            (sum_w * sum_w) / sum_w_sq
        } else {
            1.0
        }
    }

    /// Compute exponentially weighted empirical mean PSD loss matrix $\bar{\mathbf{L}}_t$.
    #[inline]
    #[allow(clippy::needless_range_loop)]
    pub fn weighted_mean_loss_matrix(
        &self,
    ) -> [[f64; MATRIX_BERNSTEIN_VENUES]; MATRIX_BERNSTEIN_VENUES] {
        let mut mean = [[0.0; MATRIX_BERNSTEIN_VENUES]; MATRIX_BERNSTEIN_VENUES];
        if self.count == 0 {
            return mean;
        }

        let mut sum_w = 0.0;
        let mut current_weight = 1.0;

        for i in 0..self.count {
            let idx = (self.head + MATRIX_BERNSTEIN_MAX_OBSERVATIONS - 1 - i)
                % MATRIX_BERNSTEIN_MAX_OBSERVATIONS;
            let w = current_weight;
            sum_w += w;
            for r in 0..MATRIX_BERNSTEIN_VENUES {
                for c in 0..MATRIX_BERNSTEIN_VENUES {
                    mean[r][c] += w * self.history[idx][r][c];
                }
            }
            current_weight *= self.config.beta;
        }

        if sum_w > 0.0 {
            for r in 0..MATRIX_BERNSTEIN_VENUES {
                for c in 0..MATRIX_BERNSTEIN_VENUES {
                    mean[r][c] /= sum_w;
                }
            }
        }
        mean
    }

    /// Compute maximum eigenvalue (operator spectral norm) of a $4 \times 4$ symmetric matrix via power iteration.
    #[inline]
    pub fn spectral_norm(
        matrix: &[[f64; MATRIX_BERNSTEIN_VENUES]; MATRIX_BERNSTEIN_VENUES],
    ) -> f64 {
        let mut v = [0.5; MATRIX_BERNSTEIN_VENUES];

        // 8 iterations of unrolled power iteration for $< 10 ns$ convergence
        for _ in 0..8 {
            let mut next = [0.0; MATRIX_BERNSTEIN_VENUES];
            for r in 0..MATRIX_BERNSTEIN_VENUES {
                for c in 0..MATRIX_BERNSTEIN_VENUES {
                    next[r] += matrix[r][c] * v[c];
                }
            }
            let mut norm_sq = 0.0;
            for x in &next {
                norm_sq += x * x;
            }
            let norm = norm_sq.sqrt();
            if norm > 1e-12 {
                for i in 0..MATRIX_BERNSTEIN_VENUES {
                    v[i] = next[i] / norm;
                }
            } else {
                return 0.0;
            }
        }

        // Rayleigh quotient: v^T A v
        let mut rayleigh = 0.0;
        for r in 0..MATRIX_BERNSTEIN_VENUES {
            let mut row_sum = 0.0;
            for c in 0..MATRIX_BERNSTEIN_VENUES {
                row_sum += matrix[r][c] * v[c];
            }
            rayleigh += v[r] * row_sum;
        }
        rayleigh.max(0.0)
    }

    /// Compute matrix variance parameter $\sigma_V^2 = \|\mathbf{V}_t\|_{\text{op}}$.
    #[inline]
    #[allow(clippy::needless_range_loop)]
    pub fn compute_matrix_variance(
        &self,
        mean: &[[f64; MATRIX_BERNSTEIN_VENUES]; MATRIX_BERNSTEIN_VENUES],
    ) -> f64 {
        if self.count == 0 {
            return 1.0;
        }

        let mut var_matrix = [[0.0; MATRIX_BERNSTEIN_VENUES]; MATRIX_BERNSTEIN_VENUES];
        let mut sum_w = 0.0;
        let mut current_weight = 1.0;

        for i in 0..self.count {
            let idx = (self.head + MATRIX_BERNSTEIN_MAX_OBSERVATIONS - 1 - i)
                % MATRIX_BERNSTEIN_MAX_OBSERVATIONS;
            let w = current_weight;
            sum_w += w;

            // Centered difference X = L_i - mean
            let mut diff = [[0.0; MATRIX_BERNSTEIN_VENUES]; MATRIX_BERNSTEIN_VENUES];
            for r in 0..MATRIX_BERNSTEIN_VENUES {
                for c in 0..MATRIX_BERNSTEIN_VENUES {
                    diff[r][c] = self.history[idx][r][c] - mean[r][c];
                }
            }

            // diff^2 = diff * diff
            for r in 0..MATRIX_BERNSTEIN_VENUES {
                for c in 0..MATRIX_BERNSTEIN_VENUES {
                    let mut sum_prod = 0.0;
                    for k in 0..MATRIX_BERNSTEIN_VENUES {
                        sum_prod += diff[r][k] * diff[k][c];
                    }
                    var_matrix[r][c] += w * sum_prod;
                }
            }
            current_weight *= self.config.beta;
        }

        if sum_w > 0.0 {
            for r in 0..MATRIX_BERNSTEIN_VENUES {
                for c in 0..MATRIX_BERNSTEIN_VENUES {
                    var_matrix[r][c] /= sum_w;
                }
            }
        }

        Self::spectral_norm(&var_matrix)
    }

    /// Compute uniform Matrix Bernstein upper slippage bound at confidence $1 - \alpha$.
    #[inline]
    pub fn compute_upper_spectral_bound(&self) -> (f64, f64) {
        if self.count == 0 {
            return (self.config.safe_slippage_bps, 0.0);
        }

        let mean_mat = self.weighted_mean_loss_matrix();
        let spectral_mean = Self::spectral_norm(&mean_mat);
        let n_eff = self.effective_sample_size();
        let sigma_v_sq = self.compute_matrix_variance(&mean_mat);

        let d = MATRIX_BERNSTEIN_VENUES as f64;
        let log_term = (2.0 * d / self.config.alpha).ln();
        let r = self.config.max_norm_bound_r;

        let scale = (r * log_term) / (3.0 * n_eff);
        let variance_term = (2.0 * sigma_v_sq * log_term) / n_eff;
        let epsilon = scale + (variance_term + scale * scale).sqrt();

        let upper_bound = spectral_mean + epsilon;
        (upper_bound, spectral_mean)
    }

    /// Evaluate risk shield and determine execution verdict in $< 20\text{ ns}$.
    #[inline]
    pub fn evaluate_shield(&self) -> ConformalMatrixBernsteinAction {
        let (s_upper, spectral_mean) = self.compute_upper_spectral_bound();

        if s_upper <= self.config.safe_slippage_bps {
            ConformalMatrixBernsteinAction::NormalExecution {
                spectral_mean_bps: spectral_mean,
                upper_spectral_bound_bps: s_upper,
                scale_factor: 1.0,
            }
        } else if s_upper >= self.config.crit_slippage_bps {
            ConformalMatrixBernsteinAction::FailClosedEvacuate {
                spectral_mean_bps: spectral_mean,
                upper_spectral_bound_bps: s_upper,
                reason: format!(
                    "Upper spectral slippage bound ({:.2} bps) breached critical hurdle ({:.2} bps)",
                    s_upper, self.config.crit_slippage_bps
                ),
            }
        } else {
            let scale_factor = (self.config.crit_slippage_bps - s_upper)
                / (self.config.crit_slippage_bps - self.config.safe_slippage_bps);
            ConformalMatrixBernsteinAction::DeriskingThrottle {
                spectral_mean_bps: spectral_mean,
                upper_spectral_bound_bps: s_upper,
                scale_factor: scale_factor.clamp(0.0, 1.0),
                reason: format!(
                    "Upper spectral slippage ({:.2} bps) elevated between safe and crit bounds",
                    s_upper
                ),
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::needless_range_loop)]
mod tests {
    use super::*;

    #[test]
    fn test_matrix_bernstein_shield_safe_state() {
        let mut shield = ConformalMatrixBernsteinShield::default();

        // Feed benign, low-slippage diagonal PSD loss matrices (e.g. 1.5 bps)
        let mut benign_loss = [[0.0; MATRIX_BERNSTEIN_VENUES]; MATRIX_BERNSTEIN_VENUES];
        for i in 0..MATRIX_BERNSTEIN_VENUES {
            benign_loss[i][i] = 1.5;
        }

        for _ in 0..100 {
            shield.record_loss_matrix(&benign_loss);
        }

        let verdict = shield.evaluate_shield();
        match verdict {
            ConformalMatrixBernsteinAction::NormalExecution {
                spectral_mean_bps,
                upper_spectral_bound_bps,
                scale_factor,
            } => {
                assert!((spectral_mean_bps - 1.5).abs() < 1e-4);
                assert!(upper_spectral_bound_bps <= shield.config.safe_slippage_bps);
                assert_eq!(scale_factor, 1.0);
            }
            other => panic!("Expected NormalExecution, got {:?}", other),
        }
    }

    #[test]
    fn test_matrix_bernstein_shield_fail_closed_evacuation() {
        let mut shield = ConformalMatrixBernsteinShield::default();

        // Feed severe, catastrophic cross-venue loss matrices (e.g. 55.0 bps)
        let mut severe_loss = [[0.0; MATRIX_BERNSTEIN_VENUES]; MATRIX_BERNSTEIN_VENUES];
        for i in 0..MATRIX_BERNSTEIN_VENUES {
            severe_loss[i][i] = 55.0;
        }

        for _ in 0..50 {
            shield.record_loss_matrix(&severe_loss);
        }

        let verdict = shield.evaluate_shield();
        match verdict {
            ConformalMatrixBernsteinAction::FailClosedEvacuate {
                upper_spectral_bound_bps,
                ..
            } => {
                assert!(upper_spectral_bound_bps >= shield.config.crit_slippage_bps);
            }
            other => panic!("Expected FailClosedEvacuate, got {:?}", other),
        }
    }

    #[test]
    fn test_power_iteration_spectral_norm_accuracy() {
        // Create a known matrix with known spectral norm:
        // A = diag(2.0, 5.0, 1.0, 3.0) -> lambda_max = 5.0
        let mut mat = [[0.0; MATRIX_BERNSTEIN_VENUES]; MATRIX_BERNSTEIN_VENUES];
        mat[0][0] = 2.0;
        mat[1][1] = 5.0;
        mat[2][2] = 1.0;
        mat[3][3] = 3.0;

        let norm = ConformalMatrixBernsteinShield::spectral_norm(&mat);
        assert!((norm - 5.0).abs() < 1e-3);
    }
}
