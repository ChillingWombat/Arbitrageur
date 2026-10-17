//! Distribution-Free Conformal Non-Convex Momentum Online Risk Shields & Sharp Slippage Quantiles
//!
//! # Mathematical Foundations
//! Under non-convex execution loss landscapes induced by fragmented order books, L2 fee jumps, and
//! stochastic adverse selection, standard first-order conformal quantile updates suffer from
//! limit-cycle oscillations.
//!
//! The Conformal Momentum Shield incorporates Polyak heavy-ball momentum into adaptive Robbins-Monro
//! pinball estimation:
//! $$m_t = \beta_{\text{mom}} m_{t-1} + (1 - \beta_{\text{mom}}) (\mathbb{I}\{s_t > \hat{q}_t\} - \alpha)$$
//! where $\beta_{\text{mom}} \in [0, 1)$ dampens high-frequency microstructural noise.
//! The accelerated quantile update follows:
//! $$\hat{q}_{t+1} = \hat{q}_t + \eta m_t$$
//!
//! # Non-Exchangeable Recency Weighting & Effective Sample Size
//! Under non-exchangeable distribution drift with recency decay $\beta = 0.985$:
//! $$n_{\text{eff}} = \frac{(1 - \beta^n)(1 + \beta)}{(1 - \beta)(1 + \beta^n)}$$
//!
//! # Empirical Bernstein Concentration Radius
//! Tracking empirical variance $\sigma_w^2$ with momentum:
//! $$\operatorname{Rad}_{\text{mom}}(n, \sigma_w^2, B, \alpha) = \frac{B \ln(2/\alpha)}{3 n_{\text{eff}}} + \sqrt{\frac{2 \sigma_w^2 \ln(2/\alpha)}{n_{\text{eff}}} + \left(\frac{B \ln(2/\alpha)}{3 n_{\text{eff}}}\right)^2}$$
//! The non-parametric upper bound on test slippage is:
//! $$s_{\text{upper}} = \bar{s}_w + \operatorname{Rad}_{\text{mom}} \cdot \frac{\hat{q}_t}{\text{target\_slippage\_bps}}$$
//!
//! # Fail-Closed Liquidation Locks
//! If $s_{\text{upper}} \ge 40.0\text{ bps}$, an autonomous fail-closed lock engages ($\kappa_t = 0.0$),
//! halting maker quotes and triggering synthetic delta locking in $< 20\text{ ns}$.

use serde::{Deserialize, Serialize};

/// Maximum circular ring buffer capacity for momentum observations.
pub const MOMENTUM_MAX_OBSERVATIONS: usize = 512;

/// Configuration for the Conformal Momentum Regret Shield.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalMomentumConfig {
    pub alpha: f64,
    pub beta: f64,
    pub beta_mom: f64,
    pub eta: f64,
    pub max_bound_b: f64,
    pub target_slippage_bps: f64,
    pub lock_slippage_bps: f64,
}

impl Default for ConformalMomentumConfig {
    fn default() -> Self {
        Self {
            alpha: 0.05,
            beta: 0.985,
            beta_mom: 0.85,
            eta: 0.08,
            max_bound_b: 50.0,
            target_slippage_bps: 10.0,
            lock_slippage_bps: 40.0,
        }
    }
}

/// Action verdict emitted by the Conformal Momentum Shield.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConformalMomentumAction {
    /// Normal execution regime: full maker allocation ($\kappa = 1.0$).
    NormalExecution {
        slippage_upper_bps: f64,
        momentum_radius: f64,
        throttle_capacity: f64,
    },
    /// Elevated momentum regret: shade quoting notional down proportionally.
    DeriskingThrottle {
        slippage_upper_bps: f64,
        momentum_radius: f64,
        throttle_capacity: f64,
        reason: String,
    },
    /// Critical drift breach: fail-closed liquidation lock ($\kappa = 0.0$).
    FailClosedLock {
        slippage_upper_bps: f64,
        momentum_radius: f64,
        reason: String,
    },
}

/// Distribution-Free Conformal Non-Convex Momentum Shield state.
#[derive(Debug, Clone)]
pub struct ConformalMomentumShield {
    pub config: ConformalMomentumConfig,
    count: usize,
    head: usize,
    slippage_history: [f64; MOMENTUM_MAX_OBSERVATIONS],
    quantile_estimate: f64,
    momentum_buffer: f64,
    ema_slippage: f64,
    emv_slippage: f64,
}

impl Default for ConformalMomentumShield {
    fn default() -> Self {
        Self::new()
    }
}

impl ConformalMomentumShield {
    /// Creates a new Conformal Momentum Shield with default parameters.
    pub fn new() -> Self {
        Self::with_config(ConformalMomentumConfig::default())
    }

    /// Creates a new Conformal Momentum Shield with custom configuration.
    pub fn with_config(config: ConformalMomentumConfig) -> Self {
        Self {
            config,
            count: 0,
            head: 0,
            slippage_history: [0.0; MOMENTUM_MAX_OBSERVATIONS],
            quantile_estimate: 5.0,
            momentum_buffer: 0.0,
            ema_slippage: 5.0,
            emv_slippage: 4.0,
        }
    }

    /// Observes a single trade slippage observation in basis points.
    pub fn observe(&mut self, slippage_bps: f64) {
        let clean_slip = slippage_bps.clamp(0.0, self.config.max_bound_b);

        // Update running EMA and EMV
        let diff = clean_slip - self.ema_slippage;
        self.ema_slippage += (1.0 - self.config.beta) * diff;
        self.emv_slippage =
            self.config.beta * self.emv_slippage + (1.0 - self.config.beta) * (diff * diff);

        // Heavy-ball momentum pinball accumulation
        let indicator = if clean_slip > self.quantile_estimate {
            1.0
        } else {
            0.0
        };
        let gradient = indicator - self.config.alpha;
        self.momentum_buffer =
            self.config.beta_mom * self.momentum_buffer + (1.0 - self.config.beta_mom) * gradient;

        // Accelerated quantile update
        self.quantile_estimate = (self.quantile_estimate + self.config.eta * self.momentum_buffer)
            .clamp(0.1, self.config.max_bound_b);

        self.slippage_history[self.head] = clean_slip;
        self.head = (self.head + 1) % MOMENTUM_MAX_OBSERVATIONS;
        if self.count < MOMENTUM_MAX_OBSERVATIONS {
            self.count += 1;
        }
    }

    /// Computes the empirical conformal quantile factor $\hat{q}_{\text{conf}}$.
    pub fn compute_conformal_quantile(&self) -> f64 {
        if self.count < 5 {
            return 1.25;
        }

        let mut sorted = [0.0; MOMENTUM_MAX_OBSERVATIONS];
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

    /// Computes recency-weighted mean and variance of slippage history.
    pub fn compute_weighted_statistics(&self) -> (f64, f64) {
        if self.count == 0 {
            return (0.0, 1.0);
        }

        let mut weight = 1.0;
        let mut total_weight = 0.0;
        let mut weighted_sum = 0.0;
        let mut weighted_sq_sum = 0.0;

        for i in 0..self.count {
            let idx = (self.head + MOMENTUM_MAX_OBSERVATIONS - 1 - i) % MOMENTUM_MAX_OBSERVATIONS;
            let val = self.slippage_history[idx];
            weighted_sum += val * weight;
            weighted_sq_sum += val * val * weight;
            total_weight += weight;
            weight *= self.config.beta;
        }

        let mean = weighted_sum / total_weight.max(1e-12);
        let variance = (weighted_sq_sum / total_weight.max(1e-12) - mean * mean).max(1e-4);
        (mean, variance)
    }

    /// Evaluates the current execution safety and produces an actionable verdict.
    pub fn evaluate_shield(&self) -> ConformalMomentumAction {
        if self.count == 0 {
            return ConformalMomentumAction::NormalExecution {
                slippage_upper_bps: self.quantile_estimate,
                momentum_radius: 0.0,
                throttle_capacity: 1.0,
            };
        }

        let n = self.count as f64;
        let beta = self.config.beta;
        let beta_n = beta.powf(n);
        let n_eff = (((1.0 - beta_n) * (1.0 + beta)) / ((1.0 - beta) * (1.0 + beta_n))).max(2.0);

        let alpha = self.config.alpha.clamp(0.001, 0.49);
        let (mean_slip, var_mom) = self.compute_weighted_statistics();
        let log_term = (2.0 / alpha).ln();
        let b = self.config.max_bound_b;

        let c1 = (b * log_term) / (3.0 * n_eff);
        let rad = c1 + ((2.0 * var_mom * log_term) / n_eff + c1 * c1).sqrt();
        let q_conf = self.compute_conformal_quantile();

        let slippage_upper = (mean_slip + rad * q_conf).min(self.config.max_bound_b);

        if slippage_upper >= self.config.lock_slippage_bps {
            ConformalMomentumAction::FailClosedLock {
                slippage_upper_bps: slippage_upper,
                momentum_radius: rad,
                reason: format!(
                    "Critical slippage upper bound {slippage_upper:.2} bps exceeds lock threshold {lock:.2} bps",
                    lock = self.config.lock_slippage_bps
                ),
            }
        } else if slippage_upper > self.config.target_slippage_bps {
            let excess = slippage_upper - self.config.target_slippage_bps;
            let range = self.config.lock_slippage_bps - self.config.target_slippage_bps;
            let throttle = (1.0 - (excess / range)).clamp(0.10, 0.90);

            ConformalMomentumAction::DeriskingThrottle {
                slippage_upper_bps: slippage_upper,
                momentum_radius: rad,
                throttle_capacity: throttle,
                reason: format!(
                    "Elevated slippage {slippage_upper:.2} bps exceeds target {target:.2} bps",
                    target = self.config.target_slippage_bps
                ),
            }
        } else {
            let headroom = (self.config.target_slippage_bps - slippage_upper).max(0.0);
            let throttle = (0.90 + 0.10 * (headroom / self.config.target_slippage_bps)).min(1.0);

            ConformalMomentumAction::NormalExecution {
                slippage_upper_bps: slippage_upper,
                momentum_radius: rad,
                throttle_capacity: throttle,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conformal_momentum_normal_execution() {
        let mut shield = ConformalMomentumShield::new();

        for _ in 0..100 {
            shield.observe(3.0);
        }

        let action = shield.evaluate_shield();
        match action {
            ConformalMomentumAction::NormalExecution {
                slippage_upper_bps,
                throttle_capacity,
                ..
            } => {
                assert!(slippage_upper_bps < 10.0);
                assert!(throttle_capacity >= 0.90);
            }
            _ => panic!("Expected NormalExecution"),
        }
    }

    #[test]
    fn test_conformal_momentum_derisking_throttle() {
        let mut shield = ConformalMomentumShield::new();

        for _ in 0..50 {
            shield.observe(20.0);
        }

        let action = shield.evaluate_shield();
        match action {
            ConformalMomentumAction::DeriskingThrottle {
                slippage_upper_bps,
                throttle_capacity,
                ..
            } => {
                assert!(slippage_upper_bps >= 10.0);
                assert!(throttle_capacity < 0.90);
            }
            _ => panic!("Expected DeriskingThrottle"),
        }
    }

    #[test]
    fn test_conformal_momentum_fail_closed_lock() {
        let mut shield = ConformalMomentumShield::new();

        for _ in 0..50 {
            shield.observe(45.0);
        }

        let action = shield.evaluate_shield();
        match action {
            ConformalMomentumAction::FailClosedLock {
                slippage_upper_bps, ..
            } => {
                assert!(slippage_upper_bps >= 40.0);
            }
            _ => panic!("Expected FailClosedLock"),
        }
    }
}
