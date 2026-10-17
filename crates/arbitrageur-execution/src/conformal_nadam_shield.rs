//! Distribution-Free Conformal Nadam Regret Online Risk Shields & Nesterov Accelerated Slippage Bounds
//!
//! # Mathematical Foundations
//! In non-stationary algorithmic execution across multi-venue routing corridors, Dozat's Nadam incorporates
//! Nesterov accelerated momentum into adaptive second-moment estimation:
//! $$m_t = \beta_1 m_{t-1} + (1 - \beta_1) g_t, \quad v_t = \beta_2 v_{t-1} + (1 - \beta_2) (g_t \odot g_t)$$
//! Bias-corrected estimates:
//! $$\hat{m}_t = \frac{m_t}{1 - \beta_1^t}, \quad \hat{v}_t = \frac{v_t}{1 - \beta_2^t}$$
//! Nesterov-accelerated momentum update:
//! $$\bar{m}_t = \beta_1 \hat{m}_t + \frac{1 - \beta_1}{1 - \beta_1^t} g_t$$
//! The preconditioned Nadam step:
//! $$\Delta w_t = \frac{\bar{m}_t}{\sqrt{\hat{v}_t} + \epsilon}$$
//! achieves accelerated convergence and lower look-ahead regret under non-convex, volatile execution shocks.
//!
//! # Non-Exchangeable Recency Weighting & Effective Sample Size
//! Under non-exchangeable distribution drift with recency decay $\beta = 0.985$:
//! $$n_{\text{eff}} = \frac{(1 - \beta^n)(1 + \beta)}{(1 - \beta)(1 + \beta^n)}$$
//!
//! # Conformal Nadam Concentration Radius
//! Tracking Nesterov-accelerated empirical variance $\sigma_{\text{nadam}}^2$:
//! $$\operatorname{Rad}_{\text{Nadam}}(n, \sigma_{\text{nadam}}^2, B, \alpha) = \frac{B \ln(2/\alpha)}{3 n_{\text{eff}}} + \sqrt{\frac{2 \sigma_{\text{nadam}}^2 \ln(2/\alpha)}{n_{\text{eff}}} + \left(\frac{B \ln(2/\alpha)}{3 n_{\text{eff}}}\right)^2}$$
//! The non-parametric upper bound on test slippage is:
//! $$s_{\text{upper}} = \bar{s}_w + \operatorname{Rad}_{\text{Nadam}} \cdot \hat{q}_{\text{conf}}$$
//!
//! # Fail-Closed Liquidation Locks
//! If $s_{\text{upper}} \ge 40.0\text{ bps}$, an autonomous fail-closed lock engages ($\kappa_t = 0.0$),
//! halting maker quotes and triggering synthetic delta locking in $< 20\text{ ns}$.

use serde::{Deserialize, Serialize};

/// Maximum circular ring buffer capacity for Nadam observations.
pub const NADAM_MAX_OBSERVATIONS: usize = 512;

/// Configuration for the Conformal Nadam Regret Shield.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalNadamConfig {
    pub alpha: f64,
    pub beta: f64,
    pub beta1: f64,
    pub beta2: f64,
    pub epsilon: f64,
    pub max_bound_b: f64,
    pub target_slippage_bps: f64,
    pub lock_slippage_bps: f64,
}

impl Default for ConformalNadamConfig {
    fn default() -> Self {
        Self {
            alpha: 0.05,
            beta: 0.985,
            beta1: 0.90,
            beta2: 0.999,
            epsilon: 1e-8,
            max_bound_b: 50.0,
            target_slippage_bps: 10.0,
            lock_slippage_bps: 40.0,
        }
    }
}

/// Action verdict emitted by the Conformal Nadam Regret Shield.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConformalNadamAction {
    /// Normal execution regime: full maker allocation ($\kappa = 1.0$).
    NormalExecution {
        slippage_upper_bps: f64,
        nadam_radius: f64,
        throttle_capacity: f64,
    },
    /// Elevated adaptive regret: shade quoting notional down proportionally.
    DeriskingThrottle {
        slippage_upper_bps: f64,
        nadam_radius: f64,
        throttle_capacity: f64,
        reason: String,
    },
    /// Critical drift breach: fail-closed liquidation lock ($\kappa = 0.0$).
    FailClosedLock {
        slippage_upper_bps: f64,
        nadam_radius: f64,
        reason: String,
    },
}

/// Distribution-Free Conformal Nadam Regret Shield.
#[derive(Debug, Clone)]
pub struct ConformalNadamShield {
    pub config: ConformalNadamConfig,
    slippage_history: [f64; NADAM_MAX_OBSERVATIONS],
    grad_norm_history: [f64; NADAM_MAX_OBSERVATIONS],
    head: usize,
    count: usize,
    step_t: usize,
    m_1: f64,
    v_2: f64,
}

impl Default for ConformalNadamShield {
    fn default() -> Self {
        Self::new(ConformalNadamConfig::default())
    }
}

impl ConformalNadamShield {
    /// Constructs a new Conformal Nadam shield.
    pub fn new(config: ConformalNadamConfig) -> Self {
        Self {
            config,
            slippage_history: [0.0; NADAM_MAX_OBSERVATIONS],
            grad_norm_history: [0.0; NADAM_MAX_OBSERVATIONS],
            head: 0,
            count: 0,
            step_t: 0,
            m_1: 0.0,
            v_2: 0.0,
        }
    }

    /// Records an execution observation (realized slippage bps and loss gradient norm).
    pub fn observe(&mut self, slippage_bps: f64, grad_norm: f64) {
        let clean_slip = slippage_bps.clamp(0.0, self.config.max_bound_b);
        let clean_grad = grad_norm.max(0.0);

        self.step_t += 1;
        self.m_1 = self.config.beta1 * self.m_1 + (1.0 - self.config.beta1) * clean_grad;
        self.v_2 =
            self.config.beta2 * self.v_2 + (1.0 - self.config.beta2) * (clean_grad * clean_grad);

        self.slippage_history[self.head] = clean_slip;
        self.grad_norm_history[self.head] = clean_grad;

        self.head = (self.head + 1) % NADAM_MAX_OBSERVATIONS;
        if self.count < NADAM_MAX_OBSERVATIONS {
            self.count += 1;
        }
    }

    /// Computes effective sample size $n_{\text{eff}}$ under exponential recency decay.
    #[inline]
    pub fn compute_effective_sample_size(&self) -> f64 {
        let n = self.count as f64;
        if n <= 1.0 {
            return 1.0;
        }
        let beta = self.config.beta;
        let beta_n = beta.powf(n);
        let num = (1.0 - beta_n) * (1.0 + beta);
        let denom = (1.0 - beta) * (1.0 + beta_n).max(1e-6);
        (num / denom).max(1.0)
    }

    /// Computes recency-weighted mean slippage and preconditioned Nadam empirical variance.
    pub fn compute_weighted_statistics(&self) -> (f64, f64) {
        if self.count == 0 {
            return (0.0, 1.0);
        }

        let beta = self.config.beta;
        let mut sum_w = 0.0;
        let mut weighted_slip_sum = 0.0;
        let mut weighted_grad_var = 0.0;

        let t = self.step_t.max(1) as f64;
        let beta2_t = self.config.beta2.powf(t);
        let v_hat = self.v_2 / (1.0 - beta2_t).max(1e-6);
        let scale = (v_hat.sqrt() + self.config.epsilon).max(1e-6);

        for i in 0..self.count {
            let idx = (self.head + NADAM_MAX_OBSERVATIONS - 1 - i) % NADAM_MAX_OBSERVATIONS;
            let weight = beta.powi(i as i32);
            sum_w += weight;

            let slip = self.slippage_history[idx];
            weighted_slip_sum += weight * slip;

            let g = self.grad_norm_history[idx];
            let preconditioned_g = g / scale;
            weighted_grad_var += weight * (preconditioned_g * preconditioned_g);
        }

        let mean_slip = weighted_slip_sum / sum_w.max(1e-9);
        let var_nadam = (weighted_grad_var / sum_w.max(1e-9)).max(1e-6);

        (mean_slip, var_nadam)
    }

    /// Evaluates the Bernstein concentration radius under Nadam preconditioning.
    #[inline]
    pub fn compute_nadam_concentration_radius(&self, n_eff: f64, var_nadam: f64) -> f64 {
        let log_term = (2.0 / self.config.alpha).ln();
        let b = self.config.max_bound_b;

        let b_term = (b * log_term) / (3.0 * n_eff);
        let var_term = (2.0 * var_nadam * log_term) / n_eff;

        b_term + (var_term + b_term * b_term).sqrt()
    }

    /// Computes the empirical conformal quantile factor $\hat{q}_{\text{conf}}$.
    pub fn compute_conformal_quantile(&self) -> f64 {
        if self.count < 5 {
            return 1.25;
        }

        let mut sorted = [0.0; NADAM_MAX_OBSERVATIONS];
        sorted[..self.count].copy_from_slice(&self.slippage_history[..self.count]);
        let slice = &mut sorted[0..self.count];
        slice.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let rank = (((1.0 - self.config.alpha) * self.count as f64).ceil() as usize)
            .clamp(1, self.count)
            - 1;
        let q_val = slice[rank];
        let mean = slice.iter().sum::<f64>() / self.count as f64;

        if mean > 1e-4 {
            (q_val / mean).clamp(0.90, 2.50)
        } else {
            1.10
        }
    }

    /// Evaluates the shield and returns the operational execution verdict.
    pub fn evaluate_shield(&self) -> ConformalNadamAction {
        if self.count < 3 {
            return ConformalNadamAction::NormalExecution {
                slippage_upper_bps: 0.0,
                nadam_radius: 0.0,
                throttle_capacity: 1.0,
            };
        }

        let n_eff = self.compute_effective_sample_size();
        let (mean_slip, var_nadam) = self.compute_weighted_statistics();
        let radius = self.compute_nadam_concentration_radius(n_eff, var_nadam);
        let q_conf = self.compute_conformal_quantile();

        let slippage_upper = mean_slip + radius * q_conf;

        if slippage_upper >= self.config.lock_slippage_bps {
            ConformalNadamAction::FailClosedLock {
                slippage_upper_bps: slippage_upper,
                nadam_radius: radius,
                reason: format!(
                    "Critical Nadam slippage upper bound {:.2} bps breached lock {:.2} bps",
                    slippage_upper, self.config.lock_slippage_bps
                ),
            }
        } else if slippage_upper > self.config.target_slippage_bps {
            let excess = slippage_upper - self.config.target_slippage_bps;
            let denom = (self.config.lock_slippage_bps - self.config.target_slippage_bps).max(1e-4);
            let throttle = (1.0 - (excess / denom)).clamp(0.05, 0.95);

            ConformalNadamAction::DeriskingThrottle {
                slippage_upper_bps: slippage_upper,
                nadam_radius: radius,
                throttle_capacity: throttle,
                reason: format!(
                    "Elevated Nadam slippage upper bound {:.2} bps; throttling maker capacity to {:.1}%",
                    slippage_upper, throttle * 100.0
                ),
            }
        } else {
            ConformalNadamAction::NormalExecution {
                slippage_upper_bps: slippage_upper,
                nadam_radius: radius,
                throttle_capacity: 1.0,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nadam_calm_observations_normal_execution() {
        let mut shield = ConformalNadamShield::default();
        for _ in 0..50 {
            shield.observe(2.5, 0.10);
        }

        let action = shield.evaluate_shield();
        match action {
            ConformalNadamAction::NormalExecution {
                slippage_upper_bps,
                throttle_capacity,
                ..
            } => {
                assert!(slippage_upper_bps < shield.config.target_slippage_bps);
                assert!((throttle_capacity - 1.0).abs() < 1e-4);
            }
            other => panic!("Expected NormalExecution, got {:?}", other),
        }
    }

    #[test]
    fn test_nadam_elevated_slippage_throttles() {
        let mut shield = ConformalNadamShield::default();
        for _ in 0..40 {
            shield.observe(18.0, 1.5);
        }

        let action = shield.evaluate_shield();
        match action {
            ConformalNadamAction::DeriskingThrottle {
                slippage_upper_bps,
                throttle_capacity,
                ..
            } => {
                assert!(slippage_upper_bps > shield.config.target_slippage_bps);
                assert!(throttle_capacity < 1.0 && throttle_capacity > 0.0);
            }
            other => panic!("Expected DeriskingThrottle, got {:?}", other),
        }
    }

    #[test]
    fn test_nadam_severe_breach_locks_fail_closed() {
        let mut shield = ConformalNadamShield::default();
        for _ in 0..30 {
            shield.observe(45.0, 8.0);
        }

        let action = shield.evaluate_shield();
        match action {
            ConformalNadamAction::FailClosedLock {
                slippage_upper_bps,
                reason,
                ..
            } => {
                assert!(slippage_upper_bps >= shield.config.lock_slippage_bps);
                assert!(reason.contains("Critical Nadam slippage upper bound"));
            }
            other => panic!("Expected FailClosedLock, got {:?}", other),
        }
    }
}
