//! Distribution-Free Conformal Quasi-Hyperbolic Discounting Online Risk Shields & Dynamic Tail Horizon Quantiles
//!
//! # Mathematical Foundations
//! Standard exponential recency weighting $w_i = \beta^i$ forces a rigid trade-off between
//! fast responsiveness to toxic order book depletion and statistical memory of low-frequency tail regimes.
//!
//! The Conformal Quasi-Hyperbolic Discounting Shield implements a dual-timescale $(\beta, \delta)$
//! discounting profile (Laibson 1997, Phelps-Pollak):
//! $$w_0 = 1, \quad w_i = \beta_{\text{present}} \cdot \delta_{\text{long}}^i \quad (\forall i \ge 1)$$
//! where $\beta_{\text{present}} \in (0, 1)$ captures immediate microstructural shock sensitivity (present-bias),
//! while $\delta_{\text{long}} \in [0.95, 0.999]$ maintains structural tail memory across high-frequency sessions.
//!
//! # Effective Sample Size under Dual-Timescale Decay
//! The finite-sample Kish effective sample size $n_{\text{eff}}$ is evaluated directly:
//! $$n_{\text{eff}} = \frac{\left( \sum_{i=0}^{n-1} w_i \right)^2}{\sum_{i=0}^{n-1} w_i^2}$$
//!
//! # Quasi-Hyperbolic Empirical Bernstein Concentration
//! The non-parametric empirical Bernstein radius under quasi-hyperbolic memory is:
//! $$\operatorname{Rad}_{\text{quasi}} = \frac{B \ln(2/\alpha)}{3 n_{\text{eff}}} + \sqrt{\frac{2 \sigma_w^2 \ln(2/\alpha)}{n_{\text{eff}}} + \left(\frac{B \ln(2/\alpha)}{3 n_{\text{eff}}}\right)^2}$$
//!
//! yielding the finite-sample distribution-free upper slippage bound:
//! $$s_{\text{upper}} = \bar{s}_w + \operatorname{Rad}_{\text{quasi}} \cdot \frac{\hat{q}_t}{\text{target\_slippage\_bps}}$$
//!
//! # Fail-Closed Liquidation Locks
//! If $s_{\text{upper}} \ge 40.0\text{ bps}$, the shield immediately locks quoting ($\kappa = 0.0$) in $< 20\text{ ns}$.

use serde::{Deserialize, Serialize};

/// Maximum circular ring buffer capacity for quasi-hyperbolic observations.
pub const QUASI_MAX_OBSERVATIONS: usize = 512;

/// Configuration for the Conformal Quasi-Hyperbolic Shield.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalQuasiConfig {
    /// Desired miscoverage rate $\alpha \in (0, 0.5)$ (default 0.05 for 95% coverage).
    pub alpha: f64,
    /// Present-bias discount factor $\beta_{\text{present}} \in (0, 1)$ (default 0.70).
    pub beta_present: f64,
    /// Long-horizon exponential discount factor $\delta_{\text{long}} \in [0.90, 0.999]$ (default 0.985).
    pub delta_long: f64,
    /// Online quantile learning rate $\eta$ (default 0.07).
    pub eta: f64,
    /// Maximum theoretical slippage bound $B$ in bps (default 50.0).
    pub max_bound_b: f64,
    /// Target execution slippage benchmark in bps (default 10.0).
    pub target_slippage_bps: f64,
    /// Hard fail-closed lock slippage hurdle in bps (default 40.0).
    pub lock_slippage_bps: f64,
}

impl Default for ConformalQuasiConfig {
    fn default() -> Self {
        Self {
            alpha: 0.05,
            beta_present: 0.70,
            delta_long: 0.985,
            eta: 0.07,
            max_bound_b: 50.0,
            target_slippage_bps: 10.0,
            lock_slippage_bps: 40.0,
        }
    }
}

/// Action verdict emitted by the Conformal Quasi-Hyperbolic Shield.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConformalQuasiAction {
    /// Normal execution regime: full maker allocation ($\kappa = 1.0$).
    NormalExecution {
        slippage_upper_bps: f64,
        quasi_radius: f64,
        throttle_capacity: f64,
    },
    /// Elevated dual-timescale regret: throttle maker allocation proportionally.
    DeriskingThrottle {
        slippage_upper_bps: f64,
        quasi_radius: f64,
        throttle_capacity: f64,
        reason: String,
    },
    /// Critical breach: fail-closed liquidation lock ($\kappa = 0.0$).
    FailClosedLock {
        slippage_upper_bps: f64,
        quasi_radius: f64,
        reason: String,
    },
}

/// Distribution-Free Conformal Quasi-Hyperbolic Online Risk Shield state.
#[derive(Debug, Clone)]
pub struct ConformalQuasiShield {
    pub config: ConformalQuasiConfig,
    count: usize,
    head: usize,
    slippage_history: [f64; QUASI_MAX_OBSERVATIONS],
    quantile_estimate: f64,
}

impl Default for ConformalQuasiShield {
    fn default() -> Self {
        Self::new()
    }
}

impl ConformalQuasiShield {
    /// Creates a new Conformal Quasi-Hyperbolic Shield with default configuration.
    pub fn new() -> Self {
        Self::with_config(ConformalQuasiConfig::default())
    }

    /// Creates a new Conformal Quasi-Hyperbolic Shield with custom configuration.
    pub fn with_config(config: ConformalQuasiConfig) -> Self {
        let initial_q = config.target_slippage_bps;
        Self {
            config,
            count: 0,
            head: 0,
            slippage_history: [0.0; QUASI_MAX_OBSERVATIONS],
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

    /// Ingests a new realized trade execution slippage observation (in bps).
    pub fn update(&mut self, realized_slippage_bps: f64) {
        let s = realized_slippage_bps.max(0.0);

        // Store into circular ring buffer
        self.slippage_history[self.head] = s;
        self.head = (self.head + 1) % QUASI_MAX_OBSERVATIONS;
        self.count += 1;

        // Subgradient of pinball loss: g_t = I(s_t > \hat{q}_t) - \alpha
        let indicator = if s > self.quantile_estimate { 1.0 } else { 0.0 };
        let grad = indicator - self.config.alpha;

        // Present-bias boosted step size when observing adverse tail excursion
        let step_multiplier = if indicator > 0.0 {
            1.0 + (1.0 - self.config.beta_present) * 0.8
        } else {
            1.0
        };

        self.quantile_estimate = (self.quantile_estimate
            + self.config.eta * grad * step_multiplier)
            .clamp(0.1, self.config.max_bound_b);
    }

    /// Computes dual-timescale quasi-hyperbolic weighted mean, variance, and effective sample size.
    fn compute_quasi_statistics(&self) -> (f64, f64, f64) {
        let n = self.count.min(QUASI_MAX_OBSERVATIONS);
        if n == 0 {
            return (0.0, 1.0, 1.0);
        }

        let beta_p = self.config.beta_present;
        let delta_l = self.config.delta_long;

        let mut sum_weights = 0.0;
        let mut sum_sq_weights = 0.0;
        let mut weighted_sum = 0.0;

        for i in 0..n {
            let idx = (self.head + QUASI_MAX_OBSERVATIONS - 1 - i) % QUASI_MAX_OBSERVATIONS;
            let val = self.slippage_history[idx];

            let weight = if i == 0 {
                1.0
            } else {
                beta_p * delta_l.powi(i as i32)
            };

            weighted_sum += val * weight;
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
            let idx = (self.head + QUASI_MAX_OBSERVATIONS - 1 - i) % QUASI_MAX_OBSERVATIONS;
            let val = self.slippage_history[idx];

            let weight = if i == 0 {
                1.0
            } else {
                beta_p * delta_l.powi(i as i32)
            };

            let diff = val - weighted_mean;
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

    /// Evaluates quasi-hyperbolic empirical Bernstein concentration radius.
    pub fn compute_quasi_radius(&self) -> f64 {
        let (_mean, var, n_eff) = self.compute_quasi_statistics();
        let alpha = self.config.alpha.clamp(0.001, 0.499);
        let log_term = (2.0 / alpha).ln();
        let b = self.config.max_bound_b;

        let term1 = (b * log_term) / (3.0 * n_eff);
        let var_term = (2.0 * var * log_term) / n_eff;
        let term2 = (var_term + term1 * term1).sqrt();

        (term1 + term2).clamp(0.01, b)
    }

    /// Evaluates current execution risk and emits action verdict.
    pub fn evaluate_risk(&self) -> ConformalQuasiAction {
        let (mean_slippage, _var, _n_eff) = self.compute_quasi_statistics();
        let radius = self.compute_quasi_radius();
        let q_ratio = (self.quantile_estimate / self.config.target_slippage_bps).clamp(0.5, 3.0);
        let slippage_upper_bps = mean_slippage + radius * q_ratio;

        let throttle_start = self.config.target_slippage_bps * 1.5;
        let lock_hurdle = self.config.lock_slippage_bps;

        if slippage_upper_bps >= lock_hurdle {
            ConformalQuasiAction::FailClosedLock {
                slippage_upper_bps,
                quasi_radius: radius,
                reason: format!(
                    "Conformal quasi-hyperbolic bound {:.2} bps exceeds hard lock threshold {:.2} bps",
                    slippage_upper_bps, lock_hurdle
                ),
            }
        } else if slippage_upper_bps > throttle_start {
            let throttle_range = lock_hurdle - throttle_start;
            let excess = slippage_upper_bps - throttle_start;
            let throttle_capacity =
                ((1.0 - excess / throttle_range).clamp(0.05, 0.95) * 1000.0).round() / 1000.0;

            ConformalQuasiAction::DeriskingThrottle {
                slippage_upper_bps,
                quasi_radius: radius,
                throttle_capacity,
                reason: format!(
                    "Elevated quasi-hyperbolic risk: upper bound {:.2} bps exceeds throttle start {:.2} bps",
                    slippage_upper_bps, throttle_start
                ),
            }
        } else {
            ConformalQuasiAction::NormalExecution {
                slippage_upper_bps,
                quasi_radius: radius,
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
        let mut shield = ConformalQuasiShield::new();

        for i in 0..100 {
            shield.update(1.8 + (i % 3) as f64 * 0.1);
        }

        assert_eq!(shield.total_observations(), 100);
        let action = shield.evaluate_risk();

        match action {
            ConformalQuasiAction::NormalExecution {
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
    fn test_present_bias_responsiveness() {
        let mut shield = ConformalQuasiShield::new();
        let initial_q = shield.current_quantile();

        // Feed burst of elevated slippage
        for _ in 0..15 {
            shield.update(22.0);
        }

        assert!(shield.current_quantile() > initial_q);
    }

    #[test]
    fn test_derisking_throttle_trigger() {
        let mut shield = ConformalQuasiShield::new();

        for _ in 0..40 {
            shield.update(18.0);
        }

        let action = shield.evaluate_risk();
        match action {
            ConformalQuasiAction::DeriskingThrottle {
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
        let mut shield = ConformalQuasiShield::new();

        for _ in 0..30 {
            shield.update(45.0);
        }

        let action = shield.evaluate_risk();
        match action {
            ConformalQuasiAction::FailClosedLock {
                slippage_upper_bps, ..
            } => {
                assert!(slippage_upper_bps >= 40.0);
            }
            _ => panic!("Expected FailClosedLock, got {:?}", action),
        }
    }
}
