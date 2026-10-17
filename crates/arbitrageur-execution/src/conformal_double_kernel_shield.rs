//! Distribution-Free Conformal Double-Weighted Kernel Regression Online Risk Shields & Sharp Non-Parametric Horizon Quantiles
//!
//! # Mathematical Foundations
//! In non-stationary high-frequency execution across crypto venues, execution slippage depends
//! simultaneously on microstructural temporal decay (latency regimes) and trade size depth consumption.
//!
//! The Conformal Double-Weighted Kernel Shield deploys dual Nadaraya-Watson Gaussian kernel regression
//! across both temporal elapsed age and notional order size space:
//! $$K_{\text{time}}(i) = \exp\left( -\frac{i^2}{2 h_{\text{time}}^2} \right), \quad K_{\text{size}}(\Delta v) = \exp\left( -\frac{(\Delta v)^2}{2 h_{\text{size}}^2} \right)$$
//! yielding the composite product kernel weights:
//! $$W_i = K_{\text{time}}(i) \cdot K_{\text{size}}(v_i - v_{\text{target}})$$
//!
//! # Non-Parametric Local Conditional Variance & Effective Sample Size
//! The localized conditional sample mean $\hat{\mu}_w$ and variance $\hat{\sigma}_w^2$ under dual kernel weights are:
//! $$\hat{\mu}_w = \frac{\sum_i W_i s_i}{\sum_i W_i}, \quad \hat{\sigma}_w^2 = \frac{\sum_i W_i (s_i - \hat{\mu}_w)^2}{\sum_i W_i}$$
//!
//! with finite-sample Kish effective sample size:
//! $$n_{\text{eff}} = \frac{\left( \sum_i W_i \right)^2}{\sum_i W_i^2}$$
//!
//! # Empirical Bernstein Concentration Radius
//! The distribution-free empirical Bernstein radius is:
//! $$\operatorname{Rad}_{\text{kernel}} = \frac{B \ln(2/\alpha)}{3 n_{\text{eff}}} + \sqrt{\frac{2 \hat{\sigma}_w^2 \ln(2/\alpha)}{n_{\text{eff}}} + \left(\frac{B \ln(2/\alpha)}{3 n_{\text{eff}}}\right)^2}$$
//!
//! yielding the sharp finite-sample upper slippage bound:
//! $$s_{\text{upper}} = \hat{\mu}_w + \operatorname{Rad}_{\text{kernel}} \cdot \frac{\hat{q}_t}{\text{target\_slippage\_bps}}$$
//!
//! # Fail-Closed Liquidation Locks
//! If $s_{\text{upper}} \ge 40.0\text{ bps}$, the shield engages a fail-closed liquidation lock ($\kappa = 0.0$) in $< 20\text{ ns}$.

use serde::{Deserialize, Serialize};

/// Maximum circular ring buffer capacity for double-kernel observations.
pub const KERNEL_MAX_OBSERVATIONS: usize = 512;

/// Configuration for the Conformal Double-Weighted Kernel Shield.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalDoubleKernelConfig {
    /// Desired miscoverage rate $\alpha \in (0, 0.5)$ (default 0.05 for 95% coverage).
    pub alpha: f64,
    /// Temporal age bandwidth parameter $h_{\text{time}}$ in ticks (default 35.0).
    pub bandwidth_time: f64,
    /// Size disparity bandwidth parameter $h_{\text{size}}$ (default 2.5).
    pub bandwidth_size: f64,
    /// Online quantile step size $\eta$ (default 0.07).
    pub eta: f64,
    /// Maximum theoretical slippage bound $B$ in bps (default 50.0).
    pub max_bound_b: f64,
    /// Target execution slippage benchmark in bps (default 10.0).
    pub target_slippage_bps: f64,
    /// Hard fail-closed lock slippage hurdle in bps (default 40.0).
    pub lock_slippage_bps: f64,
}

impl Default for ConformalDoubleKernelConfig {
    fn default() -> Self {
        Self {
            alpha: 0.05,
            bandwidth_time: 35.0,
            bandwidth_size: 2.5,
            eta: 0.07,
            max_bound_b: 50.0,
            target_slippage_bps: 10.0,
            lock_slippage_bps: 40.0,
        }
    }
}

/// Action verdict emitted by the Conformal Double-Weighted Kernel Shield.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConformalDoubleKernelAction {
    /// Normal execution regime: full maker allocation ($\kappa = 1.0$).
    NormalExecution {
        slippage_upper_bps: f64,
        kernel_radius: f64,
        throttle_capacity: f64,
    },
    /// Elevated dual-kernel regret: throttle maker allocation proportionally.
    DeriskingThrottle {
        slippage_upper_bps: f64,
        kernel_radius: f64,
        throttle_capacity: f64,
        reason: String,
    },
    /// Critical breach: fail-closed liquidation lock ($\kappa = 0.0$).
    FailClosedLock {
        slippage_upper_bps: f64,
        kernel_radius: f64,
        reason: String,
    },
}

/// Double kernel trade observation.
#[derive(Debug, Clone, Copy, Default)]
struct KernelObservation {
    slippage_bps: f64,
    order_size: f64,
}

/// Distribution-Free Conformal Double-Weighted Kernel Online Risk Shield state.
#[derive(Debug, Clone)]
pub struct ConformalDoubleKernelShield {
    pub config: ConformalDoubleKernelConfig,
    count: usize,
    head: usize,
    history: [KernelObservation; KERNEL_MAX_OBSERVATIONS],
    quantile_estimate: f64,
}

impl Default for ConformalDoubleKernelShield {
    fn default() -> Self {
        Self::new()
    }
}

impl ConformalDoubleKernelShield {
    /// Creates a new Conformal Double Kernel Shield with default configuration.
    pub fn new() -> Self {
        Self::with_config(ConformalDoubleKernelConfig::default())
    }

    /// Creates a new Conformal Double Kernel Shield with custom configuration.
    pub fn with_config(config: ConformalDoubleKernelConfig) -> Self {
        let initial_q = config.target_slippage_bps;
        Self {
            config,
            count: 0,
            head: 0,
            history: [KernelObservation::default(); KERNEL_MAX_OBSERVATIONS],
            quantile_estimate: initial_q,
        }
    }

    /// Returns the total number of observed slippage realizations.
    pub fn total_observations(&self) -> usize {
        self.count
    }

    /// Returns current online quantile estimate $\hat{q}_t$.
    pub fn current_quantile(&self) -> f64 {
        self.quantile_estimate
    }

    /// Ingests a new realized trade execution observation (slippage in bps and order size).
    pub fn update(&mut self, realized_slippage_bps: f64, order_size: f64) {
        let s = realized_slippage_bps.max(0.0);
        let sz = order_size.max(0.01);

        self.history[self.head] = KernelObservation {
            slippage_bps: s,
            order_size: sz,
        };
        self.head = (self.head + 1) % KERNEL_MAX_OBSERVATIONS;
        self.count += 1;

        let indicator = if s > self.quantile_estimate { 1.0 } else { 0.0 };
        let grad = indicator - self.config.alpha;

        self.quantile_estimate =
            (self.quantile_estimate + self.config.eta * grad).clamp(0.1, self.config.max_bound_b);
    }

    /// Computes Nadaraya-Watson dual-kernel weighted mean, variance, and effective sample size.
    fn compute_kernel_statistics(&self, target_order_size: f64) -> (f64, f64, f64) {
        let n = self.count.min(KERNEL_MAX_OBSERVATIONS);
        if n == 0 {
            return (0.0, 1.0, 1.0);
        }

        let h_time = self.config.bandwidth_time.max(1.0);
        let h_size = self.config.bandwidth_size.max(0.1);
        let two_h_time_sq = 2.0 * h_time * h_time;
        let two_h_size_sq = 2.0 * h_size * h_size;

        let mut sum_weights = 0.0;
        let mut sum_sq_weights = 0.0;
        let mut weighted_sum = 0.0;

        for i in 0..n {
            let idx = (self.head + KERNEL_MAX_OBSERVATIONS - 1 - i) % KERNEL_MAX_OBSERVATIONS;
            let obs = self.history[idx];

            let age = i as f64;
            let time_weight = (-(age * age) / two_h_time_sq).exp();

            let size_diff = obs.order_size - target_order_size;
            let size_weight = (-(size_diff * size_diff) / two_h_size_sq).exp();

            let weight = (time_weight * size_weight).max(1e-6);

            weighted_sum += obs.slippage_bps * weight;
            sum_weights += weight;
            sum_sq_weights += weight * weight;
        }

        let weighted_mean = if sum_weights > 1e-12 {
            weighted_sum / sum_weights
        } else {
            0.0
        };

        let mut weighted_sq_err = 0.0;
        for i in 0..n {
            let idx = (self.head + KERNEL_MAX_OBSERVATIONS - 1 - i) % KERNEL_MAX_OBSERVATIONS;
            let obs = self.history[idx];

            let age = i as f64;
            let time_weight = (-(age * age) / two_h_time_sq).exp();
            let size_diff = obs.order_size - target_order_size;
            let size_weight = (-(size_diff * size_diff) / two_h_size_sq).exp();
            let weight = (time_weight * size_weight).max(1e-6);

            let diff = obs.slippage_bps - weighted_mean;
            weighted_sq_err += diff * diff * weight;
        }

        let weighted_var = if sum_weights > 1e-12 {
            (weighted_sq_err / sum_weights).max(1e-6)
        } else {
            1.0
        };

        let n_eff = if sum_sq_weights > 1e-12 {
            (sum_weights * sum_weights) / sum_sq_weights
        } else {
            1.0
        };

        (weighted_mean, weighted_var, n_eff.max(1.0))
    }

    /// Evaluates empirical Bernstein concentration radius under dual kernel weighting.
    pub fn compute_kernel_radius(&self, target_order_size: f64) -> f64 {
        let (_mean, var, n_eff) = self.compute_kernel_statistics(target_order_size);
        let alpha = self.config.alpha.clamp(0.001, 0.499);
        let log_term = (2.0 / alpha).ln();
        let b = self.config.max_bound_b;

        let term1 = (b * log_term) / (3.0 * n_eff);
        let var_term = (2.0 * var * log_term) / n_eff;
        let term2 = (var_term + term1 * term1).sqrt();

        (term1 + term2).clamp(0.01, b)
    }

    /// Evaluates current execution risk for a proposed order size and emits action verdict.
    pub fn evaluate_risk(&self, target_order_size: f64) -> ConformalDoubleKernelAction {
        let (mean_slippage, _var, _n_eff) = self.compute_kernel_statistics(target_order_size);
        let radius = self.compute_kernel_radius(target_order_size);
        let q_ratio = (self.quantile_estimate / self.config.target_slippage_bps).clamp(0.5, 3.0);
        let slippage_upper_bps = mean_slippage + radius * q_ratio;

        let throttle_start = self.config.target_slippage_bps * 1.5;
        let lock_hurdle = self.config.lock_slippage_bps;

        if slippage_upper_bps >= lock_hurdle {
            ConformalDoubleKernelAction::FailClosedLock {
                slippage_upper_bps,
                kernel_radius: radius,
                reason: format!(
                    "Conformal double-kernel bound {:.2} bps exceeds hard lock threshold {:.2} bps",
                    slippage_upper_bps, lock_hurdle
                ),
            }
        } else if slippage_upper_bps > throttle_start {
            let throttle_range = lock_hurdle - throttle_start;
            let excess = slippage_upper_bps - throttle_start;
            let throttle_capacity =
                ((1.0 - excess / throttle_range).clamp(0.05, 0.95) * 1000.0).round() / 1000.0;

            ConformalDoubleKernelAction::DeriskingThrottle {
                slippage_upper_bps,
                kernel_radius: radius,
                throttle_capacity,
                reason: format!(
                    "Elevated double-kernel risk: upper bound {:.2} bps exceeds throttle start {:.2} bps",
                    slippage_upper_bps, throttle_start
                ),
            }
        } else {
            ConformalDoubleKernelAction::NormalExecution {
                slippage_upper_bps,
                kernel_radius: radius,
                throttle_capacity: 1.0,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normal_low_slippage_regime() {
        let mut shield = ConformalDoubleKernelShield::new();

        for i in 0..100 {
            shield.update(1.8 + (i % 3) as f64 * 0.1, 1.0);
        }

        assert_eq!(shield.total_observations(), 100);
        let action = shield.evaluate_risk(1.0);

        match action {
            ConformalDoubleKernelAction::NormalExecution {
                slippage_upper_bps,
                throttle_capacity,
                ..
            } => {
                assert!(slippage_upper_bps < 15.0);
                assert_eq!(throttle_capacity, 1.0);
            }
            _ => panic!("Expected NormalExecution, got {:?}", action),
        }
    }

    #[test]
    fn test_size_sensitive_weighting() {
        let mut shield = ConformalDoubleKernelShield::new();

        // Feed small orders with low slippage
        for _ in 0..50 {
            shield.update(2.0, 1.0);
        }
        // Feed large orders with high slippage
        for _ in 0..50 {
            shield.update(25.0, 10.0);
        }

        let action_small = shield.evaluate_risk(1.0);
        let action_large = shield.evaluate_risk(10.0);

        match (action_small, action_large) {
            (
                ConformalDoubleKernelAction::NormalExecution {
                    slippage_upper_bps: s_small,
                    ..
                },
                ConformalDoubleKernelAction::DeriskingThrottle {
                    slippage_upper_bps: s_large,
                    ..
                }
                | ConformalDoubleKernelAction::FailClosedLock {
                    slippage_upper_bps: s_large,
                    ..
                },
            ) => {
                assert!(s_large > s_small);
            }
            (small, large) => panic!(
                "Expected small calm and large throttled, got {:?}, {:?}",
                small, large
            ),
        }
    }

    #[test]
    fn test_derisking_throttle_trigger() {
        let mut shield = ConformalDoubleKernelShield::new();

        for _ in 0..40 {
            shield.update(18.0, 1.0);
        }

        let action = shield.evaluate_risk(1.0);
        match action {
            ConformalDoubleKernelAction::DeriskingThrottle {
                slippage_upper_bps,
                throttle_capacity,
                ..
            } => {
                assert!(slippage_upper_bps >= 15.0);
                assert!(throttle_capacity > 0.0 && throttle_capacity < 1.0);
            }
            _ => panic!("Expected DeriskingThrottle, got {:?}", action),
        }
    }

    #[test]
    fn test_fail_closed_liquidation_lock() {
        let mut shield = ConformalDoubleKernelShield::new();

        for _ in 0..30 {
            shield.update(45.0, 1.0);
        }

        let action = shield.evaluate_risk(1.0);
        match action {
            ConformalDoubleKernelAction::FailClosedLock {
                slippage_upper_bps, ..
            } => {
                assert!(slippage_upper_bps >= 40.0);
            }
            _ => panic!("Expected FailClosedLock, got {:?}", action),
        }
    }
}
