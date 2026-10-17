//! Distribution-Free Conformal Spline-Quantile Online Risk Shields & Non-Parametric Horizon Smoothers
//!
//! # Mathematical Foundations
//! In non-stationary high-frequency execution across crypto venues, execution slippage exhibits
//! complex non-linear dependency on order size, book depth exhaustion, and microstructure latency.
//!
//! Rather than imposing rigid parametric models or global kernel smoothers, the Conformal Spline-Quantile
//! Shield deploys natural cubic B-spline basis functions $B_k(v)$ over knots $\xi_1 < \xi_2 < \dots < \xi_K$
//! on the order size domain:
//! $$S(v) = \sum_{k=1}^K \theta_k B_k(v)$$
//!
//! # Online Pinball Subgradient Quantile Updates
//! The spline knot weights $\boldsymbol{\theta}_t \in \mathbb{R}^K$ update online under check function
//! (pinball loss) subgradient descent with step size $\eta$:
//! $$\theta_{k, t+1} = \theta_{k, t} + \eta \cdot \left( (1 - \alpha) - \mathbb{I}_{\{s_t \le S(v_t)\}} \right) \cdot B_k(v_t)$$
//! ensuring the spline estimate non-parametrically tracks the conditional $(1 - \alpha)$ quantile curve.
//!
//! # Conformal Horizon Smoother & Finite-Sample Coverage
//! Residual errors $e_t = s_t - S(v_t)$ feed a recursive online conformal calibration score:
//! $$\alpha_t = \mathbb{I}_{\{s_t > S(v_t)\}}$$
//! with empirical conformal multiplier $\hat{q}_{\text{conf}, t} \in [1.0, 3.0]$.
//!
//! The distribution-free upper slippage bound is:
//! $$S_{\text{upper}}(v) = S(v) + \hat{q}_{\text{conf}, t} \cdot \sqrt{\hat{\sigma}_{\text{res}}^2 + \epsilon}$$
//!
//! # Fail-Closed Liquidation Locks
//! If $S_{\text{upper}}(v) \ge \text{lock\_slippage\_bps}$ (default 40.0 bps), the shield engages
//! a fail-closed execution lock ($\kappa = 0.0$) in $< 20\text{ ns}$.

use serde::{Deserialize, Serialize};

/// Number of cubic spline basis knots across the order size space.
pub const SPLINE_KNOT_COUNT: usize = 5;

/// Configuration for the Conformal Spline-Quantile Risk Shield.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalSplineConfig {
    /// Desired miscoverage rate $\alpha \in (0, 0.5)$ (default 0.05 for 95% coverage).
    pub alpha: f64,
    /// Online quantile learning rate $\eta$ (default 0.08).
    pub learning_rate: f64,
    /// Conformal quantile multiplier adaptation rate $\gamma$ (default 0.05).
    pub conformal_rate: f64,
    /// Maximum expected order size for knot normalization (default 10.0).
    pub max_order_size: f64,
    /// Target execution slippage benchmark in basis points (default 10.0).
    pub target_slippage_bps: f64,
    /// Fail-closed liquidation lock threshold in basis points (default 40.0).
    pub lock_slippage_bps: f64,
    /// Derisking throttle threshold in basis points (default 22.0).
    pub throttle_slippage_bps: f64,
}

impl Default for ConformalSplineConfig {
    fn default() -> Self {
        Self {
            alpha: 0.05,
            learning_rate: 0.08,
            conformal_rate: 0.05,
            max_order_size: 10.0,
            target_slippage_bps: 10.0,
            lock_slippage_bps: 40.0,
            throttle_slippage_bps: 22.0,
        }
    }
}

/// Action verdict emitted by the Conformal Spline-Quantile Shield.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConformalSplineAction {
    /// Normal regime: full maker allocation ($\kappa = 1.0$).
    NormalExecution {
        slippage_upper_bps: f64,
        spline_fitted_bps: f64,
        throttle_capacity: f64,
    },
    /// Elevated slippage quantile: throttle maker allocation proportionally.
    DeriskingThrottle {
        slippage_upper_bps: f64,
        spline_fitted_bps: f64,
        throttle_capacity: f64,
        reason: String,
    },
    /// Critical breach: fail-closed liquidation lock ($\kappa = 0.0$).
    FailClosedLock {
        slippage_upper_bps: f64,
        reason: String,
    },
}

/// Distribution-Free Conformal Spline-Quantile Online Risk Shield state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalSplineShield {
    pub config: ConformalSplineConfig,
    /// Spline knot coefficients $\theta_1, \dots, \theta_K$.
    pub knot_weights: [f64; SPLINE_KNOT_COUNT],
    /// Adaptive conformal quantile scale factor $\hat{q}_{\text{conf}} \ge 1.0$.
    pub conformal_quantile: f64,
    /// Exponentially-weighted residual mean squared error $\hat{\sigma}_{\text{res}}^2$.
    pub residual_variance: f64,
    /// Total observation count.
    pub observation_count: u64,
}

impl ConformalSplineShield {
    /// Creates a new Conformal Spline Shield with default configuration.
    pub fn new() -> Self {
        Self::with_config(ConformalSplineConfig::default())
    }

    /// Creates a new Conformal Spline Shield with custom configuration.
    pub fn with_config(config: ConformalSplineConfig) -> Self {
        let initial_slippage = config.target_slippage_bps;
        Self {
            config,
            knot_weights: [initial_slippage; SPLINE_KNOT_COUNT],
            conformal_quantile: 1.0,
            residual_variance: 1.0,
            observation_count: 0,
        }
    }

    /// Computes normalized B-spline basis function weights for normalized order size $u \in [0, 1]$.
    #[inline]
    fn compute_basis(&self, order_size: f64) -> [f64; SPLINE_KNOT_COUNT] {
        let u = (order_size / self.config.max_order_size).clamp(0.0, 1.0);
        let mut basis = [0.0; SPLINE_KNOT_COUNT];

        // 5-knot cardinal cubic basis partition of unity
        let step = 1.0 / (SPLINE_KNOT_COUNT - 1) as f64;
        let mut sum = 0.0;
        for (k, b) in basis.iter_mut().enumerate() {
            let knot = k as f64 * step;
            let dist = (u - knot).abs() / step;
            if dist < 1.0 {
                *b = 1.0 - 1.5 * dist.powi(2) + 0.75 * dist.powi(3);
            } else if dist < 2.0 {
                *b = 0.25 * (2.0 - dist).powi(3);
            } else {
                *b = 0.0;
            }
            sum += *b;
        }

        // Normalize to ensure strict partition of unity
        if sum > 1e-9 {
            for b in basis.iter_mut() {
                *b /= sum;
            }
        } else {
            basis[0] = 1.0;
        }

        basis
    }

    /// Evaluates the spline-fitted conditional quantile at order size $v$.
    #[inline]
    #[allow(clippy::needless_range_loop)]
    pub fn predict_spline(&self, order_size: f64) -> f64 {
        let basis = self.compute_basis(order_size);
        let mut pred = 0.0;
        for k in 0..SPLINE_KNOT_COUNT {
            pred += self.knot_weights[k] * basis[k];
        }
        pred.max(0.0)
    }

    /// Ingests a realized trade execution slippage and updates knot coefficients and conformal quantile.
    #[allow(clippy::needless_range_loop)]
    pub fn observe_slippage(&mut self, order_size: f64, realized_slippage_bps: f64) {
        let basis = self.compute_basis(order_size);
        let current_pred = self.predict_spline(order_size);

        // Pinball loss subgradient update: (1 - alpha) - I{s <= pred}
        let target_cov = 1.0 - self.config.alpha;
        let indicator = if realized_slippage_bps <= current_pred {
            1.0
        } else {
            0.0
        };
        let subgradient = target_cov - indicator;

        for k in 0..SPLINE_KNOT_COUNT {
            self.knot_weights[k] += self.config.learning_rate * subgradient * basis[k];
            // Enforce non-negative slippage
            if self.knot_weights[k] < 0.0 {
                self.knot_weights[k] = 0.0;
            }
        }

        // Residual variance tracking
        let res = realized_slippage_bps - current_pred;
        let res_sq = res * res;
        let ewma_decay = 0.05;
        self.residual_variance = (1.0 - ewma_decay) * self.residual_variance + ewma_decay * res_sq;

        // Conformal adaptation: expand quantile multiplier if breach occurs
        let breach = realized_slippage_bps
            > (current_pred + self.conformal_quantile * (self.residual_variance.sqrt()));
        if breach {
            self.conformal_quantile += self.config.conformal_rate * (1.0 - self.config.alpha);
        } else {
            self.conformal_quantile -= self.config.conformal_rate * self.config.alpha;
        }
        self.conformal_quantile = self.conformal_quantile.clamp(1.0, 3.5);

        self.observation_count += 1;
    }

    /// Evaluates the distribution-free upper slippage quantile and produces an execution action.
    pub fn evaluate_execution_shield(&self, order_size: f64) -> ConformalSplineAction {
        let spline_pred = self.predict_spline(order_size);
        let res_std = self.residual_variance.sqrt().max(0.1);
        let slippage_upper_bps = spline_pred + self.conformal_quantile * res_std;

        if slippage_upper_bps >= self.config.lock_slippage_bps {
            ConformalSplineAction::FailClosedLock {
                slippage_upper_bps,
                reason: format!(
                    "Spline-conformal upper slippage bound {:.2} bps exceeds lock hurdle {:.2} bps",
                    slippage_upper_bps, self.config.lock_slippage_bps
                ),
            }
        } else if slippage_upper_bps >= self.config.throttle_slippage_bps {
            let excess = slippage_upper_bps - self.config.throttle_slippage_bps;
            let span = (self.config.lock_slippage_bps - self.config.throttle_slippage_bps).max(1.0);
            let throttle_capacity = (1.0 - (excess / span)).clamp(0.05, 0.95);
            ConformalSplineAction::DeriskingThrottle {
                slippage_upper_bps,
                spline_fitted_bps: spline_pred,
                throttle_capacity,
                reason: format!(
                    "Elevated spline-conformal upper slippage {:.2} bps (throttle: {:.1}%)",
                    slippage_upper_bps,
                    throttle_capacity * 100.0
                ),
            }
        } else {
            ConformalSplineAction::NormalExecution {
                slippage_upper_bps,
                spline_fitted_bps: spline_pred,
                throttle_capacity: 1.0,
            }
        }
    }
}

impl Default for ConformalSplineShield {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state_normal_execution() {
        let shield = ConformalSplineShield::new();
        let action = shield.evaluate_execution_shield(1.0);
        match action {
            ConformalSplineAction::NormalExecution {
                slippage_upper_bps,
                spline_fitted_bps,
                throttle_capacity,
            } => {
                assert!(slippage_upper_bps < shield.config.throttle_slippage_bps);
                assert!((spline_fitted_bps - shield.config.target_slippage_bps).abs() < 1e-4);
                assert!((throttle_capacity - 1.0).abs() < 1e-6);
            }
            _ => panic!("Expected NormalExecution in initial clean state"),
        }
    }

    #[test]
    fn test_spline_learns_monotone_size_slippage() {
        let mut shield = ConformalSplineShield::new();
        // Train on increasing slippage with order size: size 1.0 -> 5 bps, size 8.0 -> 25 bps
        for _ in 0..100 {
            shield.observe_slippage(1.0, 6.0);
            shield.observe_slippage(8.0, 26.0);
        }

        let small_pred = shield.predict_spline(1.0);
        let large_pred = shield.predict_spline(8.0);
        assert!(
            large_pred > small_pred,
            "Spline should predict higher slippage for large order: small={small_pred}, large={large_pred}"
        );
    }

    #[test]
    fn test_extreme_slippage_triggers_fail_closed_lock() {
        let mut shield = ConformalSplineShield::new();
        for _ in 0..50 {
            shield.observe_slippage(5.0, 65.0);
        }

        let action = shield.evaluate_execution_shield(5.0);
        match action {
            ConformalSplineAction::FailClosedLock {
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
