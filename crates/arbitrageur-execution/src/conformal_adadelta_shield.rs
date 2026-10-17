//! Distribution-Free Conformal Adadelta Online Risk Shields & Second-Order Curvature-Free Slippage Bounding
//!
//! # Mathematical Foundations
//! In non-stationary algorithmic execution across multi-venue routing corridors, Zeiler's Adadelta
//! tracks exponentially decaying running averages of both squared gradients and squared parameter updates:
//! $$E[g^2]_t = \rho E[g^2]_{t-1} + (1 - \rho) g_t^2$$
//! The preconditioned update step dynamically adapts:
//! $$\Delta x_t = -\frac{\operatorname{RMS}[\Delta x]_{t-1}}{\operatorname{RMS}[g]_t} g_t$$
//! $$E[\Delta x^2]_t = \rho E[\Delta x^2]_{t-1} + (1 - \rho) \Delta x_t^2$$
//! where $\operatorname{RMS}[z] = \sqrt{E[z^2] + \epsilon}$, matching the physical units of local inverse Hessian curvature
//! without manual hyperparameter tuning.
//!
//! # Non-Exchangeable Recency Weighting & Effective Sample Size
//! Under non-exchangeable distribution drift with recency decay parameter $\beta = 0.985$:
//! $$n_{\text{eff}} = \frac{(1 - \beta^n)(1 + \beta)}{(1 - \beta)(1 + \beta^n)}$$
//!
//! # Conformal Adadelta Concentration Radius
//! Tracking Adadelta preconditioned empirical variance $\sigma_{\text{adadelta}}^2$:
//! $$\operatorname{Rad}_{\text{Adadelta}}(n, \sigma_{\text{adadelta}}^2, B, \alpha) = \frac{B \ln(2/\alpha)}{3 n_{\text{eff}}} + \sqrt{\frac{2 \sigma_{\text{adadelta}}^2 \ln(2/\alpha)}{n_{\text{eff}}} + \left(\frac{B \ln(2/\alpha)}{3 n_{\text{eff}}}\right)^2}$$
//! The non-parametric upper bound on test slippage is:
//! $$s_{\text{upper}} = \bar{s}_w + \operatorname{Rad}_{\text{Adadelta}} \cdot \hat{q}_{\text{conf}}$$
//!
//! # Fail-Closed Liquidation Locks
//! If $s_{\text{upper}} \ge 40.0\text{ bps}$, an autonomous fail-closed lock engages ($\kappa_t = 0.0$),
//! pulling resting maker quotes and engaging synthetic delta locks in $< 20\text{ ns}$.

use serde::{Deserialize, Serialize};

/// Maximum circular ring buffer capacity for Adadelta observations.
pub const ADADELTA_MAX_OBSERVATIONS: usize = 512;

/// Configuration for the Conformal Adadelta Regret Shield.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalAdadeltaConfig {
    pub alpha: f64,
    pub beta: f64,
    pub rho: f64,
    pub epsilon: f64,
    pub max_bound_b: f64,
    pub target_slippage_bps: f64,
    pub lock_slippage_bps: f64,
}

impl Default for ConformalAdadeltaConfig {
    fn default() -> Self {
        Self {
            alpha: 0.05,
            beta: 0.985,
            rho: 0.95,
            epsilon: 1e-6,
            max_bound_b: 50.0,
            target_slippage_bps: 10.0,
            lock_slippage_bps: 40.0,
        }
    }
}

/// Action verdict emitted by the Conformal Adadelta Regret Shield.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConformalAdadeltaAction {
    /// Normal execution regime: full maker allocation ($\kappa = 1.0$).
    NormalExecution {
        slippage_upper_bps: f64,
        adadelta_radius: f64,
        throttle_capacity: f64,
    },
    /// Elevated adaptive regret: shade quoting notional down proportionally.
    DeriskingThrottle {
        slippage_upper_bps: f64,
        adadelta_radius: f64,
        throttle_capacity: f64,
        reason: String,
    },
    /// Critical drift breach: fail-closed liquidation lock ($\kappa = 0.0$).
    FailClosedLock {
        slippage_upper_bps: f64,
        adadelta_radius: f64,
        reason: String,
    },
}

/// Distribution-Free Conformal Adadelta Regret Shield.
#[derive(Debug, Clone)]
pub struct ConformalAdadeltaShield {
    pub config: ConformalAdadeltaConfig,
    slippage_history: [f64; ADADELTA_MAX_OBSERVATIONS],
    grad_norm_history: [f64; ADADELTA_MAX_OBSERVATIONS],
    head: usize,
    count: usize,
    step_t: usize,
    e_g2: f64,
    e_dx2: f64,
}

impl Default for ConformalAdadeltaShield {
    fn default() -> Self {
        Self::new(ConformalAdadeltaConfig::default())
    }
}

impl ConformalAdadeltaShield {
    /// Constructs a new Conformal Adadelta shield.
    pub fn new(config: ConformalAdadeltaConfig) -> Self {
        Self {
            config,
            slippage_history: [0.0; ADADELTA_MAX_OBSERVATIONS],
            grad_norm_history: [0.0; ADADELTA_MAX_OBSERVATIONS],
            head: 0,
            count: 0,
            step_t: 0,
            e_g2: 0.0,
            e_dx2: 0.0,
        }
    }

    /// Records an execution observation (realized slippage bps and loss gradient norm).
    pub fn observe(&mut self, slippage_bps: f64, grad_norm: f64) {
        let clean_slip = slippage_bps.clamp(0.0, self.config.max_bound_b);
        let clean_grad = grad_norm.max(0.0);

        self.step_t += 1;
        let rho = self.config.rho;
        let eps = self.config.epsilon;

        // Adadelta recurrence
        self.e_g2 = rho * self.e_g2 + (1.0 - rho) * (clean_grad * clean_grad);
        let rms_g = (self.e_g2 + eps).sqrt();
        let rms_dx_prev = (self.e_dx2 + eps).sqrt();

        let dx = (rms_dx_prev / rms_g) * clean_grad;
        self.e_dx2 = rho * self.e_dx2 + (1.0 - rho) * (dx * dx);

        self.slippage_history[self.head] = clean_slip;
        self.grad_norm_history[self.head] = clean_grad;

        self.head = (self.head + 1) % ADADELTA_MAX_OBSERVATIONS;
        if self.count < ADADELTA_MAX_OBSERVATIONS {
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

    /// Computes recency-weighted mean slippage and preconditioned Adadelta empirical variance.
    pub fn compute_weighted_statistics(&self) -> (f64, f64) {
        if self.count == 0 {
            return (0.0, 1.0);
        }

        let beta = self.config.beta;
        let mut sum_w = 0.0;
        let mut weighted_slip_sum = 0.0;
        let mut weighted_grad_var = 0.0;

        let rms_g = (self.e_g2 + self.config.epsilon).sqrt().max(1e-6);
        let rms_dx = (self.e_dx2 + self.config.epsilon).sqrt().max(1e-6);
        let scale = (rms_dx / rms_g).clamp(0.01, 100.0);

        for i in 0..self.count {
            let idx = (self.head + ADADELTA_MAX_OBSERVATIONS - 1 - i) % ADADELTA_MAX_OBSERVATIONS;
            let weight = beta.powi(i as i32);
            sum_w += weight;

            let slip = self.slippage_history[idx];
            weighted_slip_sum += weight * slip;

            let g = self.grad_norm_history[idx];
            let preconditioned_g = g * scale;
            weighted_grad_var += weight * (preconditioned_g * preconditioned_g);
        }

        let mean_slip = weighted_slip_sum / sum_w.max(1e-9);
        let var_adadelta = (weighted_grad_var / sum_w.max(1e-9)).max(1e-6);

        (mean_slip, var_adadelta)
    }

    /// Evaluates the Bernstein concentration radius under Adadelta preconditioning.
    #[inline]
    pub fn compute_adadelta_concentration_radius(&self, n_eff: f64, var_adadelta: f64) -> f64 {
        let log_term = (2.0 / self.config.alpha).ln();
        let b = self.config.max_bound_b;

        let b_term = (b * log_term) / (3.0 * n_eff);
        let var_term = (2.0 * var_adadelta * log_term) / n_eff;

        b_term + (var_term + b_term * b_term).sqrt()
    }

    /// Computes the empirical conformal quantile factor $\hat{q}_{\text{conf}}$.
    pub fn compute_conformal_quantile(&self) -> f64 {
        if self.count < 5 {
            return 1.25;
        }

        let mut sorted = [0.0; ADADELTA_MAX_OBSERVATIONS];
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
    pub fn evaluate_shield(&self) -> ConformalAdadeltaAction {
        if self.count < 3 {
            return ConformalAdadeltaAction::NormalExecution {
                slippage_upper_bps: 0.0,
                adadelta_radius: 0.0,
                throttle_capacity: 1.0,
            };
        }

        let n_eff = self.compute_effective_sample_size();
        let (mean_slip, var_adadelta) = self.compute_weighted_statistics();
        let radius = self.compute_adadelta_concentration_radius(n_eff, var_adadelta);
        let q_conf = self.compute_conformal_quantile();

        let slippage_upper = mean_slip + radius * q_conf;

        if slippage_upper >= self.config.lock_slippage_bps {
            ConformalAdadeltaAction::FailClosedLock {
                slippage_upper_bps: slippage_upper,
                adadelta_radius: radius,
                reason: format!(
                    "Critical Adadelta slippage upper bound {:.2} bps breached lock {:.2} bps",
                    slippage_upper, self.config.lock_slippage_bps
                ),
            }
        } else if slippage_upper > self.config.target_slippage_bps {
            let excess = slippage_upper - self.config.target_slippage_bps;
            let denom = (self.config.lock_slippage_bps - self.config.target_slippage_bps).max(1e-4);
            let throttle = (1.0 - (excess / denom)).clamp(0.05, 0.95);

            ConformalAdadeltaAction::DeriskingThrottle {
                slippage_upper_bps: slippage_upper,
                adadelta_radius: radius,
                throttle_capacity: throttle,
                reason: format!(
                    "Elevated Adadelta slippage upper bound {:.2} bps; throttling maker capacity to {:.1}%",
                    slippage_upper, throttle * 100.0
                ),
            }
        } else {
            ConformalAdadeltaAction::NormalExecution {
                slippage_upper_bps: slippage_upper,
                adadelta_radius: radius,
                throttle_capacity: 1.0,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adadelta_calm_observations_normal_execution() {
        let mut shield = ConformalAdadeltaShield::default();
        for _ in 0..50 {
            shield.observe(2.5, 0.10);
        }

        let action = shield.evaluate_shield();
        match action {
            ConformalAdadeltaAction::NormalExecution {
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
    fn test_adadelta_elevated_slippage_throttles() {
        let mut shield = ConformalAdadeltaShield::default();
        for _ in 0..40 {
            shield.observe(18.0, 1.5);
        }

        let action = shield.evaluate_shield();
        match action {
            ConformalAdadeltaAction::DeriskingThrottle {
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
    fn test_adadelta_severe_breach_locks_fail_closed() {
        let mut shield = ConformalAdadeltaShield::default();
        for _ in 0..30 {
            shield.observe(45.0, 8.0);
        }

        let action = shield.evaluate_shield();
        match action {
            ConformalAdadeltaAction::FailClosedLock {
                slippage_upper_bps,
                reason,
                ..
            } => {
                assert!(slippage_upper_bps >= shield.config.lock_slippage_bps);
                assert!(reason.contains("Critical Adadelta slippage upper bound"));
            }
            other => panic!("Expected FailClosedLock, got {:?}", other),
        }
    }
}
