//! Causal Double Machine Learning (DML) & Neyman-Orthogonal Price Impact Estimator (Tranche 75).
//!
//!Naive ordinary least squares (OLS) estimation of temporary and permanent market impact
//! $\Delta P_t = \theta Q_t + \varepsilon_t$ suffers from severe **simultaneity / endogeneity bias**:
//! institutional execution algorithms submit larger child orders $Q_t$ precisely when directional
//! alpha and exogenous order flow imbalance $X_t$ are strong ($\operatorname{Cov}(Q_t, g_0(X_t)) > 0$),
//! conflating exogenous price drift with self-inflicted execution impact.
//!
//! Implements Chernozhukov-Chetverikov-Demirer-Duflo-Hansen-Newey-Robins **Double/Debiased Machine
//! Learning (DML)** in Robinson's partially linear structural model:
//! $$\begin{aligned}
//! \Delta P_t &= \theta_0 \, Q_t + g_0(X_t) + U_t, \quad \mathbb{E}[U_t \mid X_t, Q_t] = 0 \\
//! Q_t &= m_0(X_t) + V_t, \quad \mathbb{E}[V_t \mid X_t] = 0
//! \end{aligned}$$
//!
//! Using 2-fold cross-fitting (Part A and Part B) to eliminate overfitting bias, we residualize both
//! the price change $\tilde{Y}_t = \Delta P_t - \hat{\ell}^{(-k)}(X_t)$ and our own trade size
//! $\tilde{V}_t = Q_t - \hat{m}^{(-k)}(X_t)$, and compute the Neyman-orthogonal causal impact
//! parameter:
//! $$\hat{\theta}_{\text{DML}} = \frac{\sum_{t=1}^n \tilde{V}_t \tilde{Y}_t}{\sum_{t=1}^n \tilde{V}_t^2}$$
//!
//! Satisfies the Neyman orthogonality condition $\left. \partial_\eta \mathbb{E}[\psi(W; \theta_0, \eta)] \right|_{\eta = \eta_0} = 0$,
//! guaranteeing $\sqrt{n}$-consistency even when nuisance estimators converge at slower rates $o(n^{-1/4})$.

use serde::{Deserialize, Serialize};

/// Number of microstructural confounding features $d$ in $X_t$
/// (e.g., `[exogenous_ofi, book_imbalance, realized_volatility, spread_bps]`).
pub const DML_CONFOUNDER_DIM: usize = 4;

/// Single execution observation $(X_t, Q_t, \Delta P_t)$ for Causal DML calibration.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CausalImpactObservation {
    /// Microstructural confounding state vector $X_t \in \mathbb{R}^4$.
    pub confounders: [f64; DML_CONFOUNDER_DIM],
    /// Signed child order trade size $Q_t$ (in base units or normalized USD notional).
    pub signed_order_size: f64,
    /// Realized price excursion $\Delta P_t$ (in basis points).
    pub price_change_bps: f64,
}

/// Calibrated Causal Price Impact Result with heteroskedasticity-robust (Eicker-Huber-White) inference.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CausalImpactEstimate {
    /// Naive biased OLS impact slope $\hat{\theta}_{\text{OLS}}$ (bps per unit size).
    pub naive_ols_impact_bps_per_unit: f64,
    /// Debiased 2-fold cross-fitted Causal DML impact slope $\hat{\theta}_{\text{DML}}$ (bps per unit size).
    pub causal_dml_impact_bps_per_unit: f64,
    /// Estimated simultaneity / endogeneity bias $\hat{\theta}_{\text{OLS}} - \hat{\theta}_{\text{DML}}$.
    pub endogeneity_bias_bps_per_unit: f64,
    /// Heteroskedasticity-robust standard error $\hat{\sigma}_{\theta}$ of $\hat{\theta}_{\text{DML}}$.
    pub robust_std_error: f64,
    /// Upper $95\%$ conservative causal impact bound $\hat{\theta}_{\text{DML}} + 1.96 \hat{\sigma}_{\theta}$.
    pub conservative_upper_impact_bps_per_unit: f64,
    /// Number of cross-fitted observations used.
    pub sample_count: usize,
}

/// Ridge-regularized linear + quadratic basis nuisance learner for $m_0(X)$ and $\ell_0(X)$.
/// Maps $X \in \mathbb{R}^4$ to a 9-dimensional feature map $\phi(X) = [1, X_1, \dots, X_4, X_1^2, \dots, X_4^2]$.
const BASIS_DIM: usize = 1 + 2 * DML_CONFOUNDER_DIM;

fn expand_basis(x: &[f64; DML_CONFOUNDER_DIM]) -> [f64; BASIS_DIM] {
    let mut phi = [0.0; BASIS_DIM];
    phi[0] = 1.0;
    for i in 0..DML_CONFOUNDER_DIM {
        phi[1 + i] = x[i];
        phi[1 + DML_CONFOUNDER_DIM + i] = x[i] * x[i];
    }
    phi
}

/// Fits a Ridge-regularized nuisance regressor $\hat{w} = (\Phi^T \Phi + \lambda I)^{-1} \Phi^T y$
/// via Gauss-Jordan elimination with partial pivoting.
fn fit_ridge_nuisance(
    samples: &[CausalImpactObservation],
    target_fn: impl Fn(&CausalImpactObservation) -> f64,
    ridge_lambda: f64,
) -> [f64; BASIS_DIM] {
    let mut a = [[0.0; BASIS_DIM]; BASIS_DIM];
    let mut b = [0.0; BASIS_DIM];

    for obs in samples {
        let phi = expand_basis(&obs.confounders);
        let y = target_fn(obs);
        for i in 0..BASIS_DIM {
            b[i] += phi[i] * y;
            for j in 0..BASIS_DIM {
                a[i][j] += phi[i] * phi[j];
            }
        }
    }

    // Add Ridge penalty to non-intercept terms
    for (i, row) in a.iter_mut().enumerate() {
        row[i] += if i == 0 { 1e-8 } else { ridge_lambda.max(1e-6) };
    }

    solve_linear_system_9x9(a, b)
}

fn predict_nuisance(weights: &[f64; BASIS_DIM], x: &[f64; DML_CONFOUNDER_DIM]) -> f64 {
    let phi = expand_basis(x);
    let mut sum = 0.0;
    for i in 0..BASIS_DIM {
        sum += weights[i] * phi[i];
    }
    sum
}

#[allow(clippy::needless_range_loop)]
fn solve_linear_system_9x9(
    mut a: [[f64; BASIS_DIM]; BASIS_DIM],
    mut b: [f64; BASIS_DIM],
) -> [f64; BASIS_DIM] {
    for col in 0..BASIS_DIM {
        let mut pivot_row = col;
        let mut max_val = a[col][col].abs();
        for (r, row) in a.iter().enumerate().skip(col + 1) {
            if row[col].abs() > max_val {
                max_val = row[col].abs();
                pivot_row = r;
            }
        }
        if max_val < 1e-12 {
            continue;
        }
        if pivot_row != col {
            a.swap(col, pivot_row);
            b.swap(col, pivot_row);
        }
        let pivot = a[col][col];
        for c in col..BASIS_DIM {
            a[col][c] /= pivot;
        }
        b[col] /= pivot;

        for r in 0..BASIS_DIM {
            if r != col {
                let factor = a[r][col];
                if factor.abs() > 1e-15 {
                    for c in col..BASIS_DIM {
                        a[r][c] -= factor * a[col][c];
                    }
                    b[r] -= factor * b[col];
                }
            }
        }
    }
    b
}

/// Online 2-Fold Cross-Fitted Causal DML Impact Estimator & Optimal Almgren-Chriss Child Sizer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CausalDmlImpactEngine {
    pub observations: Vec<CausalImpactObservation>,
    pub max_window_size: usize,
    pub ridge_lambda: f64,
    pub fallback_impact_bps_per_unit: f64,
}

impl CausalDmlImpactEngine {
    pub fn new(
        max_window_size: usize,
        ridge_lambda: f64,
        fallback_impact_bps_per_unit: f64,
    ) -> Self {
        Self {
            observations: Vec::with_capacity(max_window_size.max(16)),
            max_window_size: max_window_size.max(16),
            ridge_lambda: ridge_lambda.max(1e-4),
            fallback_impact_bps_per_unit: fallback_impact_bps_per_unit.max(0.01),
        }
    }

    pub fn record_observation(&mut self, obs: CausalImpactObservation) {
        if !obs.signed_order_size.is_finite()
            || !obs.price_change_bps.is_finite()
            || !obs.confounders.iter().all(|v| v.is_finite())
        {
            return;
        }
        if self.observations.len() >= self.max_window_size {
            self.observations.remove(0);
        }
        self.observations.push(obs);
    }

    /// Computes the 2-fold cross-fitted Neyman-orthogonal Causal DML price impact estimate.
    pub fn estimate_causal_impact(&self) -> Option<CausalImpactEstimate> {
        let n = self.observations.len();
        if n < 16 {
            return None;
        }

        // 1. Naive OLS estimate: \sum Q_t \Delta P_t / \sum Q_t^2
        let mut q_dy_sum = 0.0;
        let mut q_sq_sum = 0.0;
        for obs in &self.observations {
            q_dy_sum += obs.signed_order_size * obs.price_change_bps;
            q_sq_sum += obs.signed_order_size * obs.signed_order_size;
        }
        if q_sq_sum < 1e-12 {
            return None;
        }
        let naive_ols = q_dy_sum / q_sq_sum;

        // 2. 2-Fold Cross-Fitting: split into Fold A (even indices) and Fold B (odd indices)
        let mut fold_a = Vec::with_capacity(n / 2 + 1);
        let mut fold_b = Vec::with_capacity(n / 2 + 1);
        for (idx, &obs) in self.observations.iter().enumerate() {
            if idx % 2 == 0 {
                fold_a.push(obs);
            } else {
                fold_b.push(obs);
            }
        }

        // Fit nuisance models on Fold A to predict residuals on Fold B, and vice versa
        let w_m_a = fit_ridge_nuisance(&fold_a, |o| o.signed_order_size, self.ridge_lambda);
        let w_l_a = fit_ridge_nuisance(&fold_a, |o| o.price_change_bps, self.ridge_lambda);

        let w_m_b = fit_ridge_nuisance(&fold_b, |o| o.signed_order_size, self.ridge_lambda);
        let w_l_b = fit_ridge_nuisance(&fold_b, |o| o.price_change_bps, self.ridge_lambda);

        let mut v_tilde_vec = Vec::with_capacity(n);
        let mut y_tilde_vec = Vec::with_capacity(n);
        let mut vt_yt_sum = 0.0;
        let mut vt_sq_sum = 0.0;

        for (idx, obs) in self.observations.iter().enumerate() {
            let (m_hat, l_hat) = if idx % 2 == 0 {
                // Evaluated using Fold B nuisance models
                (
                    predict_nuisance(&w_m_b, &obs.confounders),
                    predict_nuisance(&w_l_b, &obs.confounders),
                )
            } else {
                // Evaluated using Fold A nuisance models
                (
                    predict_nuisance(&w_m_a, &obs.confounders),
                    predict_nuisance(&w_l_a, &obs.confounders),
                )
            };

            let v_tilde = obs.signed_order_size - m_hat;
            let y_tilde = obs.price_change_bps - l_hat;
            vt_yt_sum += v_tilde * y_tilde;
            vt_sq_sum += v_tilde * v_tilde;
            v_tilde_vec.push(v_tilde);
            y_tilde_vec.push(y_tilde);
        }

        if vt_sq_sum < 1e-12 {
            return None;
        }

        let theta_dml = (vt_yt_sum / vt_sq_sum).max(0.0);

        // 3. Eicker-Huber-White heteroskedasticity-robust variance estimator:
        // \hat{\sigma}_\theta^2 = \frac{\sum_{t=1}^n \tilde{V}_t^2 (\tilde{Y}_t - \hat{\theta}_{\text{DML}} \tilde{V}_t)^2}{(\sum_{t=1}^n \tilde{V}_t^2)^2}
        let mut meat_sum = 0.0;
        for i in 0..n {
            let vt = v_tilde_vec[i];
            let residual = y_tilde_vec[i] - theta_dml * vt;
            meat_sum += (vt * residual) * (vt * residual);
        }
        let robust_se = (meat_sum.sqrt()) / vt_sq_sum;
        let conservative_upper = theta_dml + 1.96 * robust_se;

        Some(CausalImpactEstimate {
            naive_ols_impact_bps_per_unit: naive_ols,
            causal_dml_impact_bps_per_unit: theta_dml,
            endogeneity_bias_bps_per_unit: naive_ols - theta_dml,
            robust_std_error: robust_se,
            conservative_upper_impact_bps_per_unit: conservative_upper,
            sample_count: n,
        })
    }

    /// Computes the maximum child order size $Q^*$ whose conservative causal price impact
    /// does not exceed `max_allowed_impact_bps` (fail-closed fallback if uncalibrated).
    pub fn recommend_max_child_size(&self, max_allowed_impact_bps: f64) -> f64 {
        if max_allowed_impact_bps <= 0.0 || !max_allowed_impact_bps.is_finite() {
            return 0.0;
        }
        let effective_slope = self
            .estimate_causal_impact()
            .map(|est| {
                est.conservative_upper_impact_bps_per_unit
                    .max(self.fallback_impact_bps_per_unit * 0.25)
            })
            .unwrap_or(self.fallback_impact_bps_per_unit);

        max_allowed_impact_bps / effective_slope.max(1e-6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_causal_dml_removes_simultaneity_endogeneity_bias() {
        // True structural data-generating process:
        // Confounder X_0 = exogenous OFI in [-2, 2]
        // Algo submits larger orders when exogenous OFI is high:
        //   Q_t = 3.0 * X_0 + V_t
        // True price change has true causal impact \theta_0 = 1.50 bps/unit PLUS direct OFI drift 6.0 * X_0:
        //   \Delta P_t = 1.50 * Q_t + 6.0 * X_0 + U_t
        // Notice: Naive OLS attributes the 6.0 * X_0 drift to Q_t, severely overestimating \hat{\theta}_{OLS} ~ 3.3+!
        // Causal DML must recover \hat{\theta}_{DML} \approx 1.50!
        let mut engine = CausalDmlImpactEngine::new(200, 1e-3, 2.5);

        for i in 0..120 {
            let t = i as f64;
            let x0 = (t * 0.37).sin() * 1.5;
            let x1 = (t * 0.73).cos() * 0.8;
            let x2 = (t * 0.19).sin() * 0.5;
            let x3 = (t * 0.51).cos() * 0.4;

            // Independent treatment excitation V_t (orthogonal to X)
            let v_t = (t * 1.91).sin() * 1.2 + (t * 2.83).cos() * 0.7;
            // Independent price noise U_t
            let u_t = (t * 3.17).cos() * 0.05;

            let q_t = 3.0 * x0 + 1.2 * (x1 * x1) + v_t;
            let true_theta = 1.50;
            let dp_t = true_theta * q_t + 6.0 * x0 + 2.4 * (x1 * x1) + u_t;

            engine.record_observation(CausalImpactObservation {
                confounders: [x0, x1, x2, x3],
                signed_order_size: q_t,
                price_change_bps: dp_t,
            });
        }

        let est = engine
            .estimate_causal_impact()
            .expect("Must produce Causal DML estimate");

        // Naive OLS is heavily upward biased (> 3.0 bps/unit) due to endogeneity with X_0
        assert!(
            est.naive_ols_impact_bps_per_unit > 3.0,
            "Expected upward-biased OLS > 3.0, got {}",
            est.naive_ols_impact_bps_per_unit
        );

        // Causal DML eliminates the confounder bias and recovers true_theta = 1.50 within 0.08!
        assert!(
            (est.causal_dml_impact_bps_per_unit - 1.50).abs() < 0.08,
            "Expected debiased Causal DML near 1.50, got {}",
            est.causal_dml_impact_bps_per_unit
        );
        assert!(est.endogeneity_bias_bps_per_unit > 1.5);
    }

    #[test]
    fn test_uncalibrated_engine_uses_fail_closed_fallback() {
        let engine = CausalDmlImpactEngine::new(64, 1e-2, 4.0);
        // With 0 samples, recommend_max_child_size uses fallback_impact_bps_per_unit = 4.0
        let max_q = engine.recommend_max_child_size(10.0);
        assert!((max_q - 2.5).abs() < 1e-9);
    }
}
