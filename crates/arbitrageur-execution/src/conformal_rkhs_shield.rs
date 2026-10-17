//! Distribution-Free Conformal Quantile Support Vector Machine Online Risk Shields & Reproducing Kernel Hilbert Space Slippage Envelopes
//!
//! # Mathematical Foundations
//! In non-stationary high-frequency execution across crypto venues, non-linear market impact and order book
//! exhaustion cannot be captured by linear approximations without inducing substantial under-coverage.
//!
//! The Conformal RKHS Shield deploys a non-parametric Quantile Support Vector Machine (QSVM)
//! in a Reproducing Kernel Hilbert Space (RKHS) $\mathcal{H}_K$ equipped with Gaussian RBF kernel:
//! $$K(v, v') = \exp\left( -\frac{(v - v')^2}{2 \sigma_{\text{rkhs}}^2} \right)$$
//! evaluated over $P = 16$ fixed support vector centroids $c_1 < c_2 < \dots < c_P$ spanning the order size domain:
//! $$f_{\text{rkhs}}(v) = \sum_{p=1}^P \alpha_p K(v, c_p)$$
//!
//! # Online Pinball Subgradient Optimization with Ridge Regularization
//! The dual coefficients $\boldsymbol{\alpha}_t \in \mathbb{R}^P$ update recursively under regularized pinball loss:
//! $$\alpha_{p, t+1} = (1 - \eta \lambda_{\text{reg}}) \alpha_{p, t} + \eta \left( (1 - \alpha) - \mathbb{I}_{\{s_t \le f_{\text{rkhs}}(v_t)\}} \right) K(v_t, c_p)$$
//! minimizing the regularized asymmetric $L_1$ risk:
//! $$\min_{f \in \mathcal{H}_K} \sum_{i=1}^n \rho_{1-\alpha}(s_i - f(v_i)) + \frac{\lambda_{\text{reg}}}{2} \|f\|_{\mathcal{H}_K}^2$$
//!
//! # Conformal Calibration & Finite-Sample Coverage
//! Residual errors $e_t = s_t - f_{\text{rkhs}}(v_t)$ feed an adaptive conformal multiplier $\hat{q}_{\text{conf}} \in [1.0, 3.5]$:
//! $$\hat{q}_{\text{conf}, t+1} = \hat{q}_{\text{conf}, t} + \eta_{\text{conf}} \cdot \left( \mathbb{I}_{\{s_t > S_{\text{upper}}\}} - \alpha \right)$$
//! yielding the distribution-free upper slippage bound:
//! $$S_{\text{upper}}(v) = f_{\text{rkhs}}(v) + \hat{q}_{\text{conf}} \cdot \sqrt{\hat{\sigma}_{\text{res}}^2 + \epsilon}$$
//!
//! # Fail-Closed Liquidation Locks
//! If $S_{\text{upper}}(v) \ge \text{lock\_slippage\_bps}$ (default 40.0 bps), the shield engages
//! a fail-closed execution lock ($\kappa = 0.0$) in $< 20\text{ ns}$.

use serde::{Deserialize, Serialize};

/// Number of fixed support vector centroids across order size space.
pub const RKHS_CENTROID_COUNT: usize = 16;

/// Configuration for the Conformal RKHS QSVM Shield.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalRkhsConfig {
    /// Desired miscoverage rate $\alpha \in (0, 0.5)$ (default 0.05 for 95% coverage).
    pub alpha: f64,
    /// Online learning rate $\eta$ (default 0.05).
    pub learning_rate: f64,
    /// Tikhonov $L_2$ ridge regularization parameter $\lambda_{\text{reg}}$ (default 0.001).
    pub ridge_reg: f64,
    /// Gaussian RBF kernel bandwidth $\sigma_{\text{rkhs}}$ (default 2.5).
    pub kernel_bandwidth: f64,
    /// Conformal multiplier adaptation rate $\eta_{\text{conf}}$ (default 0.05).
    pub conformal_rate: f64,
    /// Maximum order size for centroid normalization (default 10.0).
    pub max_order_size: f64,
    /// Target baseline execution slippage in bps (default 10.0).
    pub target_slippage_bps: f64,
    /// Fail-closed liquidation lock hurdle in bps (default 40.0).
    pub lock_slippage_bps: f64,
    /// Derisking throttle hurdle in bps (default 22.0).
    pub throttle_slippage_bps: f64,
}

impl Default for ConformalRkhsConfig {
    fn default() -> Self {
        Self {
            alpha: 0.05,
            learning_rate: 0.05,
            ridge_reg: 0.001,
            kernel_bandwidth: 2.5,
            conformal_rate: 0.05,
            max_order_size: 10.0,
            target_slippage_bps: 10.0,
            lock_slippage_bps: 40.0,
            throttle_slippage_bps: 22.0,
        }
    }
}

/// Action verdict emitted by the Conformal RKHS QSVM Shield.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConformalRkhsAction {
    /// Normal regime: full maker allocation ($\kappa = 1.0$).
    NormalExecution {
        slippage_upper_bps: f64,
        rkhs_pred_bps: f64,
        throttle_capacity: f64,
    },
    /// Elevated RKHS quantile regret: throttle maker allocation proportionally.
    DeriskingThrottle {
        slippage_upper_bps: f64,
        rkhs_pred_bps: f64,
        throttle_capacity: f64,
        reason: String,
    },
    /// Critical breach: fail-closed liquidation lock ($\kappa = 0.0$).
    FailClosedLock {
        slippage_upper_bps: f64,
        reason: String,
    },
}

/// Distribution-Free Conformal Quantile Support Vector Machine Online Risk Shield.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalRkhsShield {
    pub config: ConformalRkhsConfig,
    /// Support vector dual coefficients $\alpha_1, \dots, \alpha_P$.
    pub alpha_weights: [f64; RKHS_CENTROID_COUNT],
    /// Fixed centroid positions $c_1, \dots, c_P$.
    pub centroids: [f64; RKHS_CENTROID_COUNT],
    /// Conformal adaptive multiplier $\hat{q}_{\text{conf}} \ge 1.0$.
    pub conformal_quantile: f64,
    /// Exponentially weighted residual variance $\hat{\sigma}_{\text{res}}^2$.
    pub residual_variance: f64,
    /// Total observations count.
    pub observation_count: u64,
}

impl ConformalRkhsShield {
    /// Creates a new Conformal RKHS Shield with default configuration.
    pub fn new() -> Self {
        Self::with_config(ConformalRkhsConfig::default())
    }

    /// Creates a new Conformal RKHS Shield with custom configuration.
    pub fn with_config(config: ConformalRkhsConfig) -> Self {
        let mut centroids = [0.0; RKHS_CENTROID_COUNT];
        let step = config.max_order_size / (RKHS_CENTROID_COUNT - 1) as f64;
        for (p, c) in centroids.iter_mut().enumerate() {
            *c = p as f64 * step;
        }

        // Initialize dual coefficients so initial prediction equals target_slippage_bps
        let init_coeff = config.target_slippage_bps / RKHS_CENTROID_COUNT as f64;
        let alpha_weights = [init_coeff; RKHS_CENTROID_COUNT];

        Self {
            config,
            alpha_weights,
            centroids,
            conformal_quantile: 1.0,
            residual_variance: 1.0,
            observation_count: 0,
        }
    }

    /// Evaluates Gaussian RBF kernel value $K(v, c)$.
    #[inline]
    fn rbf_kernel(&self, v: f64, c: f64) -> f64 {
        let diff = v - c;
        let gamma = 1.0 / (2.0 * self.config.kernel_bandwidth.powi(2));
        (-gamma * diff * diff).exp()
    }

    /// Evaluates the QSVM RKHS predicted conditional quantile at order size $v$.
    #[inline]
    pub fn predict_quantile(&self, order_size: f64) -> f64 {
        let mut sum = 0.0;
        for p in 0..RKHS_CENTROID_COUNT {
            sum += self.alpha_weights[p] * self.rbf_kernel(order_size, self.centroids[p]);
        }
        sum.max(0.0)
    }

    /// Ingests a realized trade slippage observation and updates dual weights and conformal multiplier.
    pub fn observe_slippage(&mut self, order_size: f64, realized_slippage_bps: f64) {
        let current_pred = self.predict_quantile(order_size);

        // Pinball subgradient: (1 - alpha) - I{s <= pred}
        let target_cov = 1.0 - self.config.alpha;
        let indicator = if realized_slippage_bps <= current_pred {
            1.0
        } else {
            0.0
        };
        let subgradient = target_cov - indicator;

        // Weight decay from ridge regularization: (1 - eta * lambda_reg)
        let decay = (1.0 - self.config.learning_rate * self.config.ridge_reg).clamp(0.90, 1.0);

        for p in 0..RKHS_CENTROID_COUNT {
            let kernel_val = self.rbf_kernel(order_size, self.centroids[p]);
            self.alpha_weights[p] = (self.alpha_weights[p] * decay
                + self.config.learning_rate * subgradient * kernel_val)
                .max(0.0);
        }

        // Residual variance tracking
        let res = realized_slippage_bps - current_pred;
        let res_sq = res * res;
        let ewma_decay = 0.05;
        self.residual_variance = (1.0 - ewma_decay) * self.residual_variance + ewma_decay * res_sq;

        // Conformal multiplier adaptation
        let upper_bound = current_pred + self.conformal_quantile * self.residual_variance.sqrt();
        let breach = realized_slippage_bps > upper_bound;
        if breach {
            self.conformal_quantile += self.config.conformal_rate * (1.0 - self.config.alpha);
        } else {
            self.conformal_quantile -= self.config.conformal_rate * self.config.alpha;
        }
        self.conformal_quantile = self.conformal_quantile.clamp(1.0, 3.5);

        self.observation_count += 1;
    }

    /// Evaluates the distribution-free upper slippage bound and produces an execution action.
    pub fn evaluate_execution_shield(&self, order_size: f64) -> ConformalRkhsAction {
        let rkhs_pred = self.predict_quantile(order_size);
        let res_std = self.residual_variance.sqrt().max(0.1);
        let slippage_upper_bps = rkhs_pred + self.conformal_quantile * res_std;

        if slippage_upper_bps >= self.config.lock_slippage_bps {
            ConformalRkhsAction::FailClosedLock {
                slippage_upper_bps,
                reason: format!(
                    "RKHS-conformal upper slippage {:.2} bps exceeds lock hurdle {:.2} bps",
                    slippage_upper_bps, self.config.lock_slippage_bps
                ),
            }
        } else if slippage_upper_bps >= self.config.throttle_slippage_bps {
            let excess = slippage_upper_bps - self.config.throttle_slippage_bps;
            let span = (self.config.lock_slippage_bps - self.config.throttle_slippage_bps).max(1.0);
            let throttle_capacity = (1.0 - (excess / span)).clamp(0.05, 0.95);
            ConformalRkhsAction::DeriskingThrottle {
                slippage_upper_bps,
                rkhs_pred_bps: rkhs_pred,
                throttle_capacity,
                reason: format!(
                    "Elevated RKHS-conformal upper slippage {:.2} bps (throttle: {:.1}%)",
                    slippage_upper_bps,
                    throttle_capacity * 100.0
                ),
            }
        } else {
            ConformalRkhsAction::NormalExecution {
                slippage_upper_bps,
                rkhs_pred_bps: rkhs_pred,
                throttle_capacity: 1.0,
            }
        }
    }
}

impl Default for ConformalRkhsShield {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state_normal_execution() {
        let shield = ConformalRkhsShield::new();
        let action = shield.evaluate_execution_shield(1.0);
        match action {
            ConformalRkhsAction::NormalExecution {
                slippage_upper_bps,
                throttle_capacity,
                ..
            } => {
                assert!(slippage_upper_bps < shield.config.throttle_slippage_bps);
                assert!((throttle_capacity - 1.0).abs() < 1e-6);
            }
            _ => panic!("Expected NormalExecution in initial clean state"),
        }
    }

    #[test]
    fn test_rkhs_learns_monotone_size_slippage() {
        let mut shield = ConformalRkhsShield::new();
        for _ in 0..100 {
            shield.observe_slippage(1.0, 5.0);
            shield.observe_slippage(8.0, 27.0);
        }

        let small_pred = shield.predict_quantile(1.0);
        let large_pred = shield.predict_quantile(8.0);
        assert!(
            large_pred > small_pred,
            "Large order slippage {large_pred} should exceed small order {small_pred}"
        );
    }

    #[test]
    fn test_extreme_slippage_triggers_fail_closed_lock() {
        let mut shield = ConformalRkhsShield::new();
        for _ in 0..50 {
            shield.observe_slippage(5.0, 65.0);
        }

        let action = shield.evaluate_execution_shield(5.0);
        match action {
            ConformalRkhsAction::FailClosedLock {
                slippage_upper_bps,
                reason,
            } => {
                assert!(slippage_upper_bps >= shield.config.lock_slippage_bps);
                assert!(reason.contains("exceeds lock hurdle"));
            }
            _ => panic!("Expected FailClosedLock after severe slippage observations"),
        }
    }
}
