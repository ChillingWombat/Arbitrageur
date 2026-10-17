//! Multi-Asset Non-Linear Cross-Impact Simulation & Matrix Bouchaud Propagators with Cross-Exchange Decay Dynamics.
//!
//! Provides production mathematical formulations for:
//! 1. **Coupled Multi-Asset Transient Impact Kernel Matrix**:
//!    $$\mathbf{\Psi}_{ij}(t - s) = \mathbf{\Gamma}_{ij} \left(1 + \frac{t - s}{\tau_{0, j}}\right)^{-\alpha_j}$$
//!    where $\mathbf{\Gamma} \in \mathbb{R}^{K \times K}$ is the cross-impact intensity matrix, $\tau_{0, j} > 0$ is
//!    the microstructural time scale, and $\alpha_j \in (0, 1)$ is the power-law decay exponent.
//! 2. **Gatheral-Schied-Sircar (GSS) No-Dynamic-Arbitrage Theorem**:
//!    Prevents price manipulation (round-trip trajectory $\oint \dot{\mathbf{x}}(t) dt = 0$ yielding positive profit)
//!    by enforcing that the kernel matrix $\mathbf{\Psi}(\tau)$ is strictly positive definite.
//!    Performs spectral projection onto the positive semi-definite cone $\mathcal{S}_+^K$ and Ledoit-Wolf-style
//!    shrinkage regularization when the condition number $\kappa(\mathbf{\Gamma}) > 25.0$.
//! 3. **Discrete Fredholm Integral Inversion for Optimal Multi-Asset Execution Trajectories**:
//!    Solves the coupled Fredholm integral equation of the first kind:
//!    $$\min_{\mathbf{q}} \frac{1}{2} \mathbf{q}^T \mathbf{M} \mathbf{q} \quad \text{s.t.} \quad \mathbf{E}^T \mathbf{q} = \mathbf{Q}$$
//!    yielding the closed-form optimal trajectory:
//!    $$\mathbf{q}^* = \mathbf{M}^{-1} \mathbf{E} \left(\mathbf{E}^T \mathbf{M}^{-1} \mathbf{E}\right)^{-1} \mathbf{Q}$$
//!    which exhibits the empirical U-shaped trading velocity profile across correlated assets without book exhaustion.

#![allow(clippy::needless_range_loop)]

use serde::{Deserialize, Serialize};

/// Gatheral-Schied-Sircar No-Price-Manipulation Status Report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GssArbitrageStatus {
    /// True if the cross-impact matrix is strictly free of price manipulation.
    pub is_manipulation_free: bool,
    /// Spectral condition number $\kappa(\mathbf{\Gamma}) = \lambda_{\max} / \lambda_{\min}$.
    pub condition_number: f64,
    /// Minimum eigenvalue of the symmetric part $\frac{1}{2}(\mathbf{\Gamma} + \mathbf{\Gamma}^T)$.
    pub min_eigenvalue: f64,
    /// Maximum eigenvalue of the symmetric part.
    pub max_eigenvalue: f64,
    /// Whether spectral shrinkage regularization was applied.
    pub shrinkage_applied: bool,
}

/// Asset-specific Bouchaud power-law decay parameterization.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AssetDecayParam {
    /// Power-law decay exponent $\alpha \in (0.1, 0.95)$, typically ~0.50.
    pub alpha_decay: f64,
    /// Regularization time scale $\tau_0 > 0$ in seconds.
    pub tau_0_sec: f64,
    /// Temporary instantaneous impact coefficient $\eta > 0$.
    pub eta_temp: f64,
}

impl Default for AssetDecayParam {
    fn default() -> Self {
        Self {
            alpha_decay: 0.50,
            tau_0_sec: 0.10,
            eta_temp: 0.02,
        }
    }
}

/// Configuration for the Matrix Bouchaud Propagator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatrixPropagatorConfig {
    /// Maximum allowable condition number before shrinkage regularization is applied.
    pub max_condition_number: f64,
    /// Minimum strictly positive eigenvalue floor $\epsilon_{\min}$.
    pub min_eigenvalue_floor: f64,
    /// Shrinkage intensity factor $\lambda_{\text{shrink}} \in [0.0, 1.0]$.
    pub shrinkage_intensity: f64,
}

impl Default for MatrixPropagatorConfig {
    fn default() -> Self {
        Self {
            max_condition_number: 25.0,
            min_eigenvalue_floor: 1e-4,
            shrinkage_intensity: 0.15,
        }
    }
}

/// Coupled Multi-Asset Matrix Bouchaud Transient Impact Propagator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatrixBouchaudPropagator {
    pub num_assets: usize,
    pub asset_symbols: Vec<String>,
    pub decay_params: Vec<AssetDecayParam>,
    pub gamma_matrix: Vec<Vec<f64>>,
    pub config: MatrixPropagatorConfig,
}

impl MatrixBouchaudPropagator {
    /// Creates a new Matrix Bouchaud Propagator with GSS no-arbitrage enforcement.
    pub fn new(
        asset_symbols: Vec<String>,
        decay_params: Vec<AssetDecayParam>,
        raw_gamma_matrix: Vec<Vec<f64>>,
        config: MatrixPropagatorConfig,
    ) -> Option<Self> {
        let k = asset_symbols.len();
        if k == 0
            || decay_params.len() != k
            || raw_gamma_matrix.len() != k
            || raw_gamma_matrix.iter().any(|r| r.len() != k)
        {
            return None;
        }

        // Symmetrize and regularize raw gamma matrix to guarantee GSS no-arbitrage
        let regularized_gamma = Self::enforce_gss_non_arbitrage(&raw_gamma_matrix, &config);

        Some(Self {
            num_assets: k,
            asset_symbols,
            decay_params,
            gamma_matrix: regularized_gamma,
            config,
        })
    }

    /// Creates a default diagonal propagator where assets have independent impact.
    pub fn default_diagonal(asset_symbols: Vec<String>) -> Self {
        let k = asset_symbols.len();
        let mut gamma = vec![vec![0.0; k]; k];
        for (i, row) in gamma.iter_mut().enumerate().take(k) {
            row[i] = 1.0;
        }
        let decay_params = vec![AssetDecayParam::default(); k];
        Self {
            num_assets: k,
            asset_symbols,
            decay_params,
            gamma_matrix: gamma,
            config: MatrixPropagatorConfig::default(),
        }
    }

    /// Evaluates the transient cross-impact kernel matrix $\mathbf{\Psi}(\tau) \in \mathbb{R}^{K \times K}$:
    /// $$\Psi_{ij}(\tau) = \Gamma_{ij} \left(1 + \frac{\tau}{\tau_{0, j}}\right)^{-\alpha_j}$$
    pub fn evaluate_kernel_matrix(&self, tau_sec: f64) -> Vec<Vec<f64>> {
        let k = self.num_assets;
        let mut psi = vec![vec![0.0; k]; k];
        let t = tau_sec.max(0.0);

        for i in 0..k {
            for j in 0..k {
                let p = &self.decay_params[j];
                let decay_factor = (1.0 + t / p.tau_0_sec.max(1e-4)).powf(-p.alpha_decay);
                psi[i][j] = self.gamma_matrix[i][j] * decay_factor;
            }
        }

        psi
    }

    /// Computes cumulative price displacement vector $\mathbf{D}(t) \in \mathbb{R}^K$ from trade history:
    /// $$D_i(t) = \sum_{j=1}^K \sum_{m} \Psi_{ij}(t - t_{j, m}) q_{j, m}$$
    pub fn compute_cumulative_displacement(
        &self,
        past_trades_by_asset: &[Vec<(f64, f64)>], // (timestamp_sec, signed_quantity)
        current_time_sec: f64,
    ) -> Vec<f64> {
        if past_trades_by_asset.len() != self.num_assets {
            return vec![0.0; self.num_assets];
        }

        let mut displacements = vec![0.0; self.num_assets];

        for (j, trades) in past_trades_by_asset.iter().enumerate() {
            let p = &self.decay_params[j];
            for &(t_m, q_m) in trades {
                if current_time_sec >= t_m {
                    let tau = current_time_sec - t_m;
                    let decay = (1.0 + tau / p.tau_0_sec.max(1e-4)).powf(-p.alpha_decay);
                    for i in 0..self.num_assets {
                        displacements[i] += self.gamma_matrix[i][j] * decay * q_m;
                    }
                }
            }
        }

        displacements
    }

    /// Evaluates Gatheral-Schied-Sircar (GSS) no-dynamic-arbitrage criteria on a matrix.
    pub fn check_gss_status(
        gamma: &[Vec<f64>],
        config: &MatrixPropagatorConfig,
    ) -> GssArbitrageStatus {
        let k = gamma.len();
        if k == 0 {
            return GssArbitrageStatus {
                is_manipulation_free: true,
                condition_number: 1.0,
                min_eigenvalue: 1.0,
                max_eigenvalue: 1.0,
                shrinkage_applied: false,
            };
        }

        // Symmetrize: S = 0.5 * (Gamma + Gamma^T)
        let mut s = vec![vec![0.0; k]; k];
        for i in 0..k {
            for j in 0..k {
                s[i][j] = 0.5 * (gamma[i][j] + gamma[j][i]);
            }
        }

        let eigenvalues = Self::compute_symmetric_eigenvalues(&s);
        let min_eig = eigenvalues.iter().copied().fold(f64::INFINITY, f64::min);
        let max_eig = eigenvalues
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);

        let condition_number = if min_eig > 1e-12 {
            max_eig / min_eig
        } else {
            f64::INFINITY
        };

        let is_manipulation_free = min_eig >= config.min_eigenvalue_floor
            && condition_number <= config.max_condition_number;

        GssArbitrageStatus {
            is_manipulation_free,
            condition_number,
            min_eigenvalue: min_eig,
            max_eigenvalue: max_eig,
            shrinkage_applied: condition_number > config.max_condition_number
                || min_eig < config.min_eigenvalue_floor,
        }
    }

    /// Enforces GSS no-arbitrage via symmetrization, eigenvalue floor, and condition number shrinkage.
    fn enforce_gss_non_arbitrage(
        raw_gamma: &[Vec<f64>],
        config: &MatrixPropagatorConfig,
    ) -> Vec<Vec<f64>> {
        let k = raw_gamma.len();
        let mut s = vec![vec![0.0; k]; k];
        for i in 0..k {
            for j in 0..k {
                s[i][j] = 0.5 * (raw_gamma[i][j] + raw_gamma[j][i]);
            }
        }

        let (eigenvalues, eigenvectors) = Self::jacobi_eigen_decomposition(&s);

        // Floor eigenvalues to min_eigenvalue_floor
        let mut floored_eigs = vec![0.0; k];
        for (i, &eig) in eigenvalues.iter().enumerate().take(k) {
            floored_eigs[i] = eig.max(config.min_eigenvalue_floor);
        }

        // Reconstruct matrix: Gamma_proj = V * diag(floored_eigs) * V^T
        let mut proj = vec![vec![0.0; k]; k];
        for i in 0..k {
            for j in 0..k {
                let mut sum = 0.0;
                for l in 0..k {
                    sum += eigenvectors[i][l] * floored_eigs[l] * eigenvectors[j][l];
                }
                proj[i][j] = sum;
            }
        }

        // Check condition number
        let min_e = floored_eigs.iter().copied().fold(f64::INFINITY, f64::min);
        let max_e = floored_eigs
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let cond = if min_e > 1e-12 {
            max_e / min_e
        } else {
            f64::INFINITY
        };

        if cond > config.max_condition_number {
            // Apply Ledoit-Wolf-style shrinkage toward diagonal
            let lambda = config.shrinkage_intensity.clamp(0.01, 0.99);
            let mut reg = vec![vec![0.0; k]; k];
            for i in 0..k {
                for j in 0..k {
                    if i == j {
                        reg[i][j] = (1.0 - lambda) * proj[i][j] + lambda * proj[i][i];
                    } else {
                        reg[i][j] = (1.0 - lambda) * proj[i][j];
                    }
                }
            }
            reg
        } else {
            proj
        }
    }

    /// Computes eigenvalues of a real symmetric matrix via Jacobi rotations.
    fn compute_symmetric_eigenvalues(matrix: &[Vec<f64>]) -> Vec<f64> {
        let (eigs, _) = Self::jacobi_eigen_decomposition(matrix);
        eigs
    }

    /// Jacobi eigenvalue algorithm for real symmetric matrices.
    /// Returns (eigenvalues, eigenvectors).
    fn jacobi_eigen_decomposition(matrix: &[Vec<f64>]) -> (Vec<f64>, Vec<Vec<f64>>) {
        let n = matrix.len();
        let mut a = matrix.to_owned();
        let mut v = vec![vec![0.0; n]; n];
        for (i, row) in v.iter_mut().enumerate().take(n) {
            row[i] = 1.0;
        }

        let max_iterations = 100;
        let tol = 1e-10;

        for _ in 0..max_iterations {
            // Find off-diagonal element of maximum magnitude
            let mut max_val = 0.0;
            let mut p = 0;
            let mut q = 1;

            for i in 0..n {
                for j in (i + 1)..n {
                    let abs_val = a[i][j].abs();
                    if abs_val > max_val {
                        max_val = abs_val;
                        p = i;
                        q = j;
                    }
                }
            }

            if max_val < tol {
                break;
            }

            // Compute Jacobi rotation angle
            let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
            let t = if theta >= 0.0 {
                1.0 / (theta + (theta * theta + 1.0).sqrt())
            } else {
                -1.0 / (-theta + (theta * theta + 1.0).sqrt())
            };
            let c = 1.0 / (t * t + 1.0).sqrt();
            let s = t * c;

            let tau = s / (1.0 + c);
            let a_pq = a[p][q];

            a[p][q] = 0.0;
            a[q][p] = 0.0;
            a[p][p] -= t * a_pq;
            a[q][q] += t * a_pq;

            for r in 0..n {
                if r != p && r != q {
                    let a_rp = a[r][p];
                    let a_rq = a[r][q];
                    a[r][p] = a_rp - s * (a_rq + tau * a_rp);
                    a[p][r] = a[r][p];
                    a[r][q] = a_rq + s * (a_rp - tau * a_rq);
                    a[q][r] = a[r][q];
                }
            }

            for r in 0..n {
                let v_rp = v[r][p];
                let v_rq = v[r][q];
                v[r][p] = c * v_rp - s * v_rq;
                v[r][q] = s * v_rp + c * v_rq;
            }
        }

        let eigenvalues = (0..n).map(|i| a[i][i]).collect();
        (eigenvalues, v)
    }
}

/// Discrete multi-asset execution schedule computed from Fredholm integral inversion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MultiAssetExecutionSchedule {
    /// Number of time steps $N$.
    pub num_steps: usize,
    /// Number of assets $K$.
    pub num_assets: usize,
    /// Time slice timestamps relative to start (seconds).
    pub slice_timestamps_sec: Vec<f64>,
    /// Execution quantities per slice: `slices[step_idx][asset_idx]`.
    pub execution_slices: Vec<Vec<f64>>,
    /// Total expected market impact slippage cost across all assets (USD or quote units).
    pub expected_total_impact_cost: f64,
    /// Vector of target quantities $\mathbf{Q}$.
    pub target_quantities: Vec<f64>,
}

/// Discrete Fredholm Integral Inversion Engine for Multi-Asset Optimal Trajectories.
#[derive(Debug, Clone)]
pub struct FredholmOptimalExecutionEngine {
    pub propagator: MatrixBouchaudPropagator,
}

impl FredholmOptimalExecutionEngine {
    pub fn new(propagator: MatrixBouchaudPropagator) -> Self {
        Self { propagator }
    }

    /// Solves the multi-asset Fredholm integral equation of the first kind:
    /// $$\min_{\mathbf{q}} \frac{1}{2} \mathbf{q}^T \mathbf{M} \mathbf{q} \quad \text{s.t.} \quad \mathbf{E}^T \mathbf{q} = \mathbf{Q}$$
    ///
    /// * `target_portfolio`: Target quantity to execute per asset $\mathbf{Q} \in \mathbb{R}^K$.
    /// * `total_duration_sec`: Execution window $T$ in seconds.
    /// * `num_steps`: Number of execution slices $N \ge 2$.
    pub fn compute_optimal_schedule(
        &self,
        target_portfolio: &[f64],
        total_duration_sec: f64,
        num_steps: usize,
    ) -> Option<MultiAssetExecutionSchedule> {
        let k = self.propagator.num_assets;
        let n = num_steps.max(2);
        if target_portfolio.len() != k || total_duration_sec <= 0.0 {
            return None;
        }

        let dt = total_duration_sec / n as f64;
        let total_dim = k * n;

        // Build Block-Toeplitz Matrix M \in \mathbb{R}^{KN \times KN}
        // Block (n_i, n_j) has size K \times K:
        // M_{(n_i, n_j)} = 0.5 * [ \Psi(|n_i - n_j| dt) + \Psi^T(|n_i - n_j| dt) ] + \delta_{ij} H_temp
        let mut m_matrix = vec![vec![0.0; total_dim]; total_dim];

        for step_i in 0..n {
            for step_j in 0..n {
                let tau = ((step_i as isize - step_j as isize).abs() as f64) * dt;
                let psi = self.propagator.evaluate_kernel_matrix(tau);

                let row_offset = step_i * k;
                let col_offset = step_j * k;

                for a_i in 0..k {
                    for a_j in 0..k {
                        let symmetric_impact = 0.5 * (psi[a_i][a_j] + psi[a_j][a_i]);
                        m_matrix[row_offset + a_i][col_offset + a_j] = symmetric_impact;
                    }
                }

                // Add temporary instantaneous impact to diagonal blocks
                if step_i == step_j {
                    for a_i in 0..k {
                        let eta = self.propagator.decay_params[a_i].eta_temp;
                        m_matrix[row_offset + a_i][col_offset + a_i] += eta;
                    }
                }
            }
        }

        // Invert M matrix: M_inv \in \mathbb{R}^{KN \times KN}
        let m_inv = Self::invert_symmetric_positive_definite(&m_matrix)?;

        // Compute C = E^T M^{-1} E \in \mathbb{R}^{K \times K}
        // Since E is block of K x K identities: (E^T M^{-1} E)_{a, b} = \sum_{step_i} \sum_{step_j} M_inv[step_i * k + a][step_j * k + b]
        let mut c_matrix = vec![vec![0.0; k]; k];
        for a in 0..k {
            for b in 0..k {
                let mut sum = 0.0;
                for step_i in 0..n {
                    for step_j in 0..n {
                        sum += m_inv[step_i * k + a][step_j * k + b];
                    }
                }
                c_matrix[a][b] = sum;
            }
        }

        // Invert C matrix: C_inv \in \mathbb{R}^{K \times K}
        let c_inv = Self::invert_symmetric_positive_definite(&c_matrix)?;

        // Compute lambda = C_inv * Q \in \mathbb{R}^K
        let mut lambda = vec![0.0; k];
        for i in 0..k {
            for j in 0..k {
                lambda[i] += c_inv[i][j] * target_portfolio[j];
            }
        }

        // Compute q^* = M^{-1} * E * lambda \in \mathbb{R}^{KN}
        // For element (step_i * k + a): q^*_idx = \sum_{b=0}^{k-1} [ \sum_{step_j=0}^{n-1} M_inv[step_i * k + a][step_j * k + b] ] * lambda[b]
        let mut q_star = vec![0.0; total_dim];
        for step_i in 0..n {
            for a in 0..k {
                let idx = step_i * k + a;
                let mut sum = 0.0;
                for b in 0..k {
                    let mut m_inv_block_sum = 0.0;
                    for step_j in 0..n {
                        m_inv_block_sum += m_inv[idx][step_j * k + b];
                    }
                    sum += m_inv_block_sum * lambda[b];
                }
                q_star[idx] = sum;
            }
        }

        // Calculate expected impact cost: 0.5 * q^T * M * q
        let mut expected_cost = 0.0;
        for i in 0..total_dim {
            let mut row_sum = 0.0;
            for j in 0..total_dim {
                row_sum += m_matrix[i][j] * q_star[j];
            }
            expected_cost += 0.5 * q_star[i] * row_sum;
        }

        // Reshape q_star into execution_slices[step_idx][asset_idx]
        let mut slices = vec![vec![0.0; k]; n];
        let mut timestamps = Vec::with_capacity(n);

        for (step_i, slice) in slices.iter_mut().enumerate().take(n) {
            timestamps.push((step_i as f64 + 0.5) * dt);
            for a in 0..k {
                slice[a] = q_star[step_i * k + a];
            }
        }

        Some(MultiAssetExecutionSchedule {
            num_steps: n,
            num_assets: k,
            slice_timestamps_sec: timestamps,
            execution_slices: slices,
            expected_total_impact_cost: expected_cost,
            target_quantities: target_portfolio.to_vec(),
        })
    }

    /// Solves system A * X = I using Cholesky factorization with positive-definite fallback.
    fn invert_symmetric_positive_definite(matrix: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
        let n = matrix.len();
        // Compute Cholesky L * L^T = A
        let mut l = vec![vec![0.0; n]; n];

        for i in 0..n {
            for j in 0..=i {
                let mut sum = 0.0;
                for k in 0..j {
                    sum += l[i][k] * l[j][k];
                }

                if i == j {
                    let val = matrix[i][i] - sum;
                    if val <= 1e-12 {
                        // Matrix is singular or ill-conditioned; use Gauss-Jordan with diagonal loading
                        return Self::invert_gauss_jordan_regularized(matrix);
                    }
                    l[i][j] = val.sqrt();
                } else {
                    l[i][j] = (matrix[i][j] - sum) / l[j][j].max(1e-12);
                }
            }
        }

        // Invert L (lower triangular)
        let mut l_inv = vec![vec![0.0; n]; n];
        for i in 0..n {
            l_inv[i][i] = 1.0 / l[i][i];
            for j in 0..i {
                let mut sum = 0.0;
                for k in j..i {
                    sum += l[i][k] * l_inv[k][j];
                }
                l_inv[i][j] = -sum / l[i][i];
            }
        }

        // A^{-1} = L^{-T} * L^{-1}
        let mut a_inv = vec![vec![0.0; n]; n];
        for i in 0..n {
            for j in 0..n {
                let mut sum = 0.0;
                for k in 0..n {
                    sum += l_inv[k][i] * l_inv[k][j];
                }
                a_inv[i][j] = sum;
            }
        }

        Some(a_inv)
    }

    /// Regularized Gauss-Jordan inversion with partial pivoting and Tikhonov regularizer.
    fn invert_gauss_jordan_regularized(matrix: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
        let n = matrix.len();
        let mut aug = vec![vec![0.0; 2 * n]; n];
        let lambda_ridge = 1e-6;

        for i in 0..n {
            for j in 0..n {
                aug[i][j] = matrix[i][j] + if i == j { lambda_ridge } else { 0.0 };
            }
            aug[i][n + i] = 1.0;
        }

        for i in 0..n {
            // Find pivot
            let mut pivot_row = i;
            let mut max_val = aug[i][i].abs();
            for k in (i + 1)..n {
                if aug[k][i].abs() > max_val {
                    max_val = aug[k][i].abs();
                    pivot_row = k;
                }
            }

            if max_val < 1e-12 {
                return None;
            }

            if pivot_row != i {
                aug.swap(i, pivot_row);
            }

            let pivot = aug[i][i];
            for j in 0..(2 * n) {
                aug[i][j] /= pivot;
            }

            for k in 0..n {
                if k != i {
                    let factor = aug[k][i];
                    for j in 0..(2 * n) {
                        aug[k][j] -= factor * aug[i][j];
                    }
                }
            }
        }

        let mut inv = vec![vec![0.0; n]; n];
        for i in 0..n {
            for j in 0..n {
                inv[i][j] = aug[i][n + j];
            }
        }

        Some(inv)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gss_no_arbitrage_validation() {
        let config = MatrixPropagatorConfig::default();
        // Positive definite symmetric matrix
        let valid_gamma = vec![vec![1.0, 0.4], vec![0.4, 1.0]];
        let status = MatrixBouchaudPropagator::check_gss_status(&valid_gamma, &config);
        assert!(status.is_manipulation_free);
        assert!(status.min_eigenvalue > 0.0);
        assert!(status.condition_number < config.max_condition_number);
    }

    #[test]
    fn test_gss_asymmetric_symmetrization_and_projection() {
        let config = MatrixPropagatorConfig::default();
        // Asymmetric raw matrix with negative cross-terms
        let asymmetric_gamma = vec![vec![1.0, 0.8], vec![0.1, 0.5]];
        let symbols = vec!["BTC".to_string(), "ETH".to_string()];
        let params = vec![AssetDecayParam::default(), AssetDecayParam::default()];

        let propagator = MatrixBouchaudPropagator::new(symbols, params, asymmetric_gamma, config)
            .expect("Should initialize propagator");

        let status = MatrixBouchaudPropagator::check_gss_status(
            &propagator.gamma_matrix,
            &propagator.config,
        );
        assert!(status.is_manipulation_free);
        assert!(status.min_eigenvalue >= propagator.config.min_eigenvalue_floor);
    }

    #[test]
    fn test_fredholm_optimal_trajectory_conservation() {
        let symbols = vec!["BTC".to_string(), "ETH".to_string()];
        let params = vec![
            AssetDecayParam {
                alpha_decay: 0.50,
                tau_0_sec: 0.10,
                eta_temp: 0.01,
            },
            AssetDecayParam {
                alpha_decay: 0.55,
                tau_0_sec: 0.12,
                eta_temp: 0.015,
            },
        ];
        let gamma = vec![vec![1.0, 0.35], vec![0.35, 1.2]];

        let propagator = MatrixBouchaudPropagator::new(
            symbols,
            params,
            gamma,
            MatrixPropagatorConfig::default(),
        )
        .expect("Propagator creation");

        let engine = FredholmOptimalExecutionEngine::new(propagator);
        let target = vec![10.0, 50.0]; // Execute 10 BTC, 50 ETH
        let schedule = engine
            .compute_optimal_schedule(&target, 60.0, 6)
            .expect("Schedule computation");

        assert_eq!(schedule.num_steps, 6);
        assert_eq!(schedule.num_assets, 2);

        // Sum across time steps should match target quantities
        let mut total_executed = [0.0; 2];
        for slice in &schedule.execution_slices {
            total_executed[0] += slice[0];
            total_executed[1] += slice[1];
        }

        assert!(
            (total_executed[0] - 10.0).abs() < 1e-4,
            "BTC quantity conservation: {}",
            total_executed[0]
        );
        assert!(
            (total_executed[1] - 50.0).abs() < 1e-4,
            "ETH quantity conservation: {}",
            total_executed[1]
        );

        // Verify U-shaped trajectory: first and last steps have higher execution intensity than middle
        let first_slice_btc = schedule.execution_slices[0][0];
        let mid_slice_btc = schedule.execution_slices[2][0];
        let last_slice_btc = schedule.execution_slices[5][0];

        assert!(
            first_slice_btc > mid_slice_btc && last_slice_btc > mid_slice_btc,
            "Trajectory must be U-shaped: first={}, mid={}, last={}",
            first_slice_btc,
            mid_slice_btc,
            last_slice_btc
        );
        assert!(schedule.expected_total_impact_cost > 0.0);
    }
}
