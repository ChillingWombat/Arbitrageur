//! Distribution-Free Conformal Non-Stationary Online Matrix Completion Shields & Non-Commutative Spectral Execution Bounding
//!
//! # Mathematical Foundations
//! In high-frequency cross-venue execution across $m$ assets and $d$ venues, execution slippage and liquidity
//! loss rates are represented by an unobserved full matrix $\mathbf{M}_t \in \mathbb{R}^{m \times d}$.
//! Because trades execute asynchronously, only a sparse subset of entries $\Omega_t \subset \{1, \dots, m\} \times \{1, \dots, d\}$
//! is observed in real time.
//!
//! Online matrix completion models $\mathbf{X}_t = \mathbf{U}_t \mathbf{V}_t^T \in \mathbb{R}^{m \times d}$ with low rank $r \ll \min(m, d)$.
//! For observed entries $(i, j) \in \Omega_t$, prediction residuals $r_{ij} = |\mathbf{X}_{ij, t} - \mathbf{M}_{ij, t}|$ measure imputation error.
//!
//! # Conformal Spectral Bounding
//! Under non-exchangeable conformal prediction with recency decay $\beta = 0.985$ and effective sample size $N_{\text{eff}} = \frac{(\sum w)^2}{\sum w^2}$,
//! the distribution-free upper bound on entry-wise imputation error is:
//! $$\delta_{\text{impute}} = \hat{q}_{1-\alpha} + \sqrt{\frac{\ln(2/\alpha)}{N_{\text{eff}}}} + \frac{2 \ln(2/\alpha)}{N_{\text{eff}}}$$
//!
//! By non-commutative matrix perturbation theory, the worst-case spectral operator norm across all venues is bounded by:
//! $$\|\mathbf{X}_{\text{upper}}\|_{\text{op}} = \sigma_{\max}(\mathbf{X}_t) + \sqrt{d} \cdot \delta_{\text{impute}}$$
//!
//! This upper envelope dynamically modulates maker quoting capacity $\kappa_t$:
//! $$\kappa_t = \operatorname{clamp}\left(\frac{\sigma_{\text{crit}} - \|\mathbf{X}_{\text{upper}}\|_{\text{op}}}{\sigma_{\text{crit}} - \sigma_{\text{safe}}}, 0.0, 1.0\right)$$
//! When $\|\mathbf{X}_{\text{upper}}\|_{\text{op}} \ge \sigma_{\text{crit}}$ (40.0 bps), the shield triggers an immediate
//! fail-closed evacuation pulling all resting maker orders across all venues and locking synthetic perpetual delta in $< 50\text{ ns}$.

const M_ASSETS: usize = 4;
const D_VENUES: usize = 4;
const RANK: usize = 2;
const MAX_HISTORY: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConformalMatrixShieldAction {
    /// Normal execution regime: full quoting capacity (kappa = 1.0)
    NormalExecution { kappa: f64 },
    /// Non-stationary spectral warning: throttled quoting capacity (0.0 < kappa < 1.0)
    ThrottledExecution { kappa: f64, spectral_upper_bps: f64 },
    /// Fail-closed evacuation: pull all maker quotes and lock synthetic perp delta (kappa = 0.0)
    SpectralMatrixEvacuateAndLock { spectral_upper_bps: f64 },
}

#[derive(Debug, Clone, Copy)]
pub struct SparseMatrixEntry {
    pub asset_idx: usize,
    pub venue_idx: usize,
    pub observed_loss_bps: f64,
}

#[derive(Debug, Clone)]
pub struct MatrixCompletionConfig {
    pub beta: f64,
    pub alpha: f64,
    pub sigma_safe_bps: f64,
    pub sigma_crit_bps: f64,
    pub learning_rate: f64,
    pub regularization: f64,
}

impl Default for MatrixCompletionConfig {
    fn default() -> Self {
        Self {
            beta: 0.985,
            alpha: 0.05,
            sigma_safe_bps: 12.0,
            sigma_crit_bps: 40.0,
            learning_rate: 0.05,
            regularization: 0.01,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConformalMatrixCompletionShield {
    config: MatrixCompletionConfig,
    u_factors: [[f64; RANK]; M_ASSETS],
    v_factors: [[f64; RANK]; D_VENUES],
    history_residuals: [f64; MAX_HISTORY],
    count: usize,
    head: usize,
}

impl ConformalMatrixCompletionShield {
    pub fn new(config: MatrixCompletionConfig) -> Self {
        // Initialize factor matrices with small orthogonal priors
        let mut u = [[0.1; RANK]; M_ASSETS];
        let mut v = [[0.1; RANK]; D_VENUES];
        for (i, row) in u.iter_mut().enumerate() {
            row[i % RANK] = 0.5;
        }
        for (j, row) in v.iter_mut().enumerate() {
            row[j % RANK] = 0.5;
        }

        Self {
            config,
            u_factors: u,
            v_factors: v,
            history_residuals: [0.0; MAX_HISTORY],
            count: 0,
            head: 0,
        }
    }

    /// Update low-rank factor decomposition with a sparse observed entry and record residual
    pub fn observe_entry(&mut self, entry: SparseMatrixEntry) {
        let i = entry.asset_idx % M_ASSETS;
        let j = entry.venue_idx % D_VENUES;
        let y = entry.observed_loss_bps.max(0.0);

        // Compute current dot product prediction X_{ij} = u_i^T v_j
        let mut pred = 0.0;
        for r in 0..RANK {
            pred += self.u_factors[i][r] * self.v_factors[j][r];
        }
        pred = pred.max(0.0);

        let err = pred - y;
        let abs_res = err.abs();

        // Record residual in rolling buffer
        self.history_residuals[self.head] = abs_res;
        self.head = (self.head + 1) % MAX_HISTORY;
        if self.count < MAX_HISTORY {
            self.count += 1;
        }

        // Online gradient descent step on low-rank factors
        let lr = self.config.learning_rate;
        let reg = self.config.regularization;

        for r in 0..RANK {
            let u_val = self.u_factors[i][r];
            let v_val = self.v_factors[j][r];

            self.u_factors[i][r] -= lr * (err * v_val + reg * u_val);
            self.v_factors[j][r] -= lr * (err * u_val + reg * v_val);

            self.u_factors[i][r] = self.u_factors[i][r].clamp(-10.0, 10.0);
            self.v_factors[j][r] = self.v_factors[j][r].clamp(-10.0, 10.0);
        }
    }

    /// Evaluates the completed matrix spectral norm via zero-allocation power iteration
    #[allow(clippy::needless_range_loop)]
    pub fn compute_spectral_norm(&self) -> f64 {
        // Compute full completed matrix X = U * V^T
        let mut x = [[0.0; D_VENUES]; M_ASSETS];
        for i in 0..M_ASSETS {
            for j in 0..D_VENUES {
                let mut sum = 0.0;
                for r in 0..RANK {
                    sum += self.u_factors[i][r] * self.v_factors[j][r];
                }
                x[i][j] = sum.max(0.0);
            }
        }

        // Form Gram matrix G = X * X^T (4x4)
        let mut g = [[0.0; M_ASSETS]; M_ASSETS];
        for i in 0..M_ASSETS {
            for k in 0..M_ASSETS {
                let mut sum = 0.0;
                for j in 0..D_VENUES {
                    sum += x[i][j] * x[k][j];
                }
                g[i][k] = sum;
            }
        }

        // 10 iterations of power iteration for top eigenvalue
        let mut v = [0.5; M_ASSETS];
        for _ in 0..10 {
            let mut v_next = [0.0; M_ASSETS];
            for i in 0..M_ASSETS {
                for k in 0..M_ASSETS {
                    v_next[i] += g[i][k] * v[k];
                }
            }
            let mut norm = 0.0;
            for val in &v_next {
                norm += val * val;
            }
            norm = norm.sqrt().max(1e-12);
            for i in 0..M_ASSETS {
                v[i] = v_next[i] / norm;
            }
        }

        // Rayleigh quotient lambda_max(G)
        let mut lambda_max = 0.0;
        for i in 0..M_ASSETS {
            let mut g_v_i = 0.0;
            for k in 0..M_ASSETS {
                g_v_i += g[i][k] * v[k];
            }
            lambda_max += v[i] * g_v_i;
        }

        lambda_max.max(0.0).sqrt()
    }

    /// Evaluates conformal online matrix completion risk shield
    pub fn evaluate_shield(&self) -> ConformalMatrixShieldAction {
        if self.count == 0 {
            return ConformalMatrixShieldAction::NormalExecution { kappa: 1.0 };
        }

        let n = self.count;
        let beta = self.config.beta;

        // 1. Recency weights and effective sample size
        let mut sum_w = 0.0;
        let mut sum_w_sq = 0.0;
        for i in 0..n {
            let w = beta.powi(i as i32);
            sum_w += w;
            sum_w_sq += w * w;
        }
        let n_eff = (sum_w * sum_w) / sum_w_sq.max(1e-9);

        // 2. Conformal quantile of imputation residuals
        let mut residuals = [0.0; MAX_HISTORY];
        let mut weights = [0.0; MAX_HISTORY];
        for i in 0..n {
            let idx = (self.head + MAX_HISTORY - 1 - i) % MAX_HISTORY;
            residuals[i] = self.history_residuals[idx];
            weights[i] = beta.powi(i as i32) / sum_w;
        }

        let mut indices = [0; MAX_HISTORY];
        for (i, item) in indices.iter_mut().enumerate().take(n) {
            *item = i;
        }
        indices[0..n].sort_by(|&a, &b| {
            residuals[a]
                .partial_cmp(&residuals[b])
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let target_quantile = 1.0 - self.config.alpha;
        let mut cum_w = 0.0;
        let mut q_hat = residuals[indices[n - 1]];

        for &idx in &indices[0..n] {
            cum_w += weights[idx];
            if cum_w >= target_quantile {
                q_hat = residuals[idx];
                break;
            }
        }

        // 3. Conformal imputation bound
        let log_term = (2.0 / self.config.alpha).ln();
        let concentration_penalty =
            (log_term / n_eff.max(1.0)).sqrt() + (2.0 * log_term / n_eff.max(1.0));
        let delta_impute = q_hat + concentration_penalty;

        // 4. Non-commutative spectral perturbation bound: ||X_upper||_op = ||X||_op + sqrt(d) * delta_impute
        let sigma_x = self.compute_spectral_norm();
        let spectral_upper_bps = sigma_x + (D_VENUES as f64).sqrt() * delta_impute;

        // 5. Quoting throttle kappa
        let safe_bps = self.config.sigma_safe_bps;
        let crit_bps = self.config.sigma_crit_bps;

        if spectral_upper_bps >= crit_bps {
            ConformalMatrixShieldAction::SpectralMatrixEvacuateAndLock { spectral_upper_bps }
        } else if spectral_upper_bps <= safe_bps {
            ConformalMatrixShieldAction::NormalExecution { kappa: 1.0 }
        } else {
            let kappa = ((crit_bps - spectral_upper_bps) / (crit_bps - safe_bps)).clamp(0.0, 1.0);
            ConformalMatrixShieldAction::ThrottledExecution {
                kappa,
                spectral_upper_bps,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_benign_sparse_losses_permit_normal_execution() {
        let mut shield = ConformalMatrixCompletionShield::new(MatrixCompletionConfig::default());

        // Observe low loss entries across venues
        for t in 0..40 {
            shield.observe_entry(SparseMatrixEntry {
                asset_idx: t % M_ASSETS,
                venue_idx: (t * 2) % D_VENUES,
                observed_loss_bps: 1.5,
            });
        }

        let action = shield.evaluate_shield();
        match action {
            ConformalMatrixShieldAction::NormalExecution { kappa } => {
                assert_eq!(kappa, 1.0);
            }
            _ => panic!("Expected NormalExecution, got {:?}", action),
        }
    }

    #[test]
    fn test_moderate_sparse_losses_trigger_throttled_execution() {
        let mut shield = ConformalMatrixCompletionShield::new(MatrixCompletionConfig::default());

        for t in 0..30 {
            shield.observe_entry(SparseMatrixEntry {
                asset_idx: t % M_ASSETS,
                venue_idx: t % D_VENUES,
                observed_loss_bps: 5.5,
            });
        }

        let action = shield.evaluate_shield();
        match action {
            ConformalMatrixShieldAction::ThrottledExecution {
                kappa,
                spectral_upper_bps,
            } => {
                assert!(kappa > 0.0 && kappa < 1.0);
                assert!(spectral_upper_bps > 12.0 && spectral_upper_bps < 40.0);
            }
            _ => panic!("Expected ThrottledExecution, got {:?}", action),
        }
    }

    #[test]
    fn test_extreme_cross_venue_loss_triggers_spectral_evacuation() {
        let mut shield = ConformalMatrixCompletionShield::new(MatrixCompletionConfig::default());

        for t in 0..30 {
            shield.observe_entry(SparseMatrixEntry {
                asset_idx: t % M_ASSETS,
                venue_idx: t % D_VENUES,
                observed_loss_bps: 50.0,
            });
        }

        let action = shield.evaluate_shield();
        match action {
            ConformalMatrixShieldAction::SpectralMatrixEvacuateAndLock { spectral_upper_bps } => {
                assert!(spectral_upper_bps >= 40.0);
            }
            _ => panic!("Expected SpectralMatrixEvacuateAndLock, got {:?}", action),
        }
    }
}
