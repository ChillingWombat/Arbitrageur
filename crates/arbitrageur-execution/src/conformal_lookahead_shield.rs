//! Distribution-Free Conformal Lookahead Online Risk Shields & Multi-Step Slippage Horizons
//!
//! # Mathematical Foundations
//! When executing staggered multi-leg arbitrage packages or child order trajectories, market impact
//! and adverse selection accumulate non-linearly across lookahead steps $h \in \{1, \dots, H\}$:
//! $$S_h = \sum_{j=1}^h s_j$$
//! Under non-stationary execution dynamics, standard single-step conformal bounds underestimate the joint
//! probability of tail excursions across the sequential trajectory.
//!
//! # Multi-Horizon Conformal Calibration
//! The Conformal Lookahead Shield maintains simultaneous distribution-free non-conformity quantiles
//! across all lookahead horizons $h \in \{1, \dots, H\}$. To preserve family-wise coverage without conservative
//! Bonferroni degradation, we introduce horizon-discounted coverage levels:
//! $$\alpha_h = \alpha \cdot \gamma^{h-1}$$
//! where $\gamma \in (0, 1]$ is the temporal lookahead discount factor.
//!
//! # Adaptive Robbins-Monro Pinball Updates
//! For each lookahead horizon $h$, the non-conformity quantile $\hat{q}^{(h)}_t$ is updated adaptively online:
//! $$\hat{q}^{(h)}_{t+1} = \hat{q}^{(h)}_t + \eta \left( \mathbb{I}\{S_{h, t} > \hat{q}^{(h)}_t\} - \alpha_h \right)$$
//!
//! # Empirical Bernstein Concentration Radius
//! Non-exchangeable recency weighting ($\beta = 0.985$) provides an effective sample size:
//! $$n_{\text{eff}} = \frac{(1 - \beta^n)(1 + \beta)}{(1 - \beta)(1 + \beta^n)}$$
//! with empirical Bernstein concentration radius bounding cumulative lookahead slippage:
//! $$\operatorname{Rad}_{\text{lookahead}}(n, \sigma_h^2, B, \alpha_h) = \frac{B \ln(2/\alpha_h)}{3 n_{\text{eff}}} + \sqrt{\frac{2 \sigma_h^2 \ln(2/\alpha_h)}{n_{\text{eff}}} + \left(\frac{B \ln(2/\alpha_h)}{3 n_{\text{eff}}}\right)^2}$$
//!
//! # Autonomous Horizon Compression & Abort
//! If cumulative slippage at horizon $h$ breaches the conservative conformal corridor:
//! - Mild breach ($S_h \le \text{lock\_slippage\_bps}$): dynamic horizon compression ($\kappa_t < 1.0$), reducing
//!   staggered execution steps to $h - 1$.
//! - Severe breach ($S_h > \text{lock\_slippage\_bps}$): immediate fail-closed abort ($\kappa_t = 0.0$),
//!   halting maker orders and initiating synthetic delta locks.

use serde::{Deserialize, Serialize};

/// Maximum lookahead horizon steps supported.
pub const LOOKAHEAD_MAX_HORIZON: usize = 8;

/// Maximum circular ring buffer capacity for lookahead history.
pub const LOOKAHEAD_BUFFER_CAPACITY: usize = 512;

/// Configuration for the Conformal Lookahead Shield.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalLookaheadConfig {
    /// Nominal miscoverage rate $\alpha \in (0, 0.5)$ (default 0.05).
    pub alpha: f64,
    /// Horizon discount factor $\gamma \in (0, 1]$ (default 0.95).
    pub gamma: f64,
    /// Non-exchangeable recency weighting decay (default 0.985).
    pub beta: f64,
    /// Robbins-Monro online step size $\eta$ (default 0.05).
    pub eta: f64,
    /// Maximum slippage bound $B$ in basis points (default 60.0).
    pub max_bound_b: f64,
    /// Target cumulative slippage threshold in basis points (default 15.0).
    pub target_slippage_bps: f64,
    /// Fail-closed lock threshold in basis points (default 45.0).
    pub lock_slippage_bps: f64,
    /// Number of active lookahead horizon steps (default 4).
    pub horizon_steps: usize,
}

impl Default for ConformalLookaheadConfig {
    fn default() -> Self {
        Self {
            alpha: 0.05,
            gamma: 0.95,
            beta: 0.985,
            eta: 0.05,
            max_bound_b: 60.0,
            target_slippage_bps: 15.0,
            lock_slippage_bps: 45.0,
            horizon_steps: 4,
        }
    }
}

/// Action verdict emitted by the Conformal Lookahead Shield.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConformalLookaheadAction {
    /// Safe execution regime across all lookahead horizons ($\kappa = 1.0$).
    NormalExecution {
        max_projected_slip_bps: f64,
        safe_horizon_steps: usize,
        throttle_capacity: f64,
    },
    /// Elevated cumulative slippage at horizon $h$: compress execution horizon to safe steps.
    CompressHorizon {
        breached_horizon_step: usize,
        safe_horizon_steps: usize,
        max_projected_slip_bps: f64,
        throttle_capacity: f64,
        reason: String,
    },
    /// Critical slippage excursion: fail-closed abort ($\kappa = 0.0$).
    FailClosedAbort {
        breached_horizon_step: usize,
        observed_slip_bps: f64,
        quantile_bound_bps: f64,
        reason: String,
    },
}

/// Distribution-Free Conformal Lookahead Shield state.
#[derive(Debug, Clone)]
pub struct ConformalLookaheadShield {
    pub config: ConformalLookaheadConfig,
    count: usize,
    head: usize,
    /// Circular buffer storing cumulative slippage trajectories: [buffer_idx][horizon_idx].
    history: [[f64; LOOKAHEAD_MAX_HORIZON]; LOOKAHEAD_BUFFER_CAPACITY],
    /// Running Robbins-Monro adaptive quantile estimates for each horizon.
    quantiles: [f64; LOOKAHEAD_MAX_HORIZON],
    /// Running exponential moving average of cumulative slippage per horizon.
    ema_slippage: [f64; LOOKAHEAD_MAX_HORIZON],
    /// Running exponential moving variance per horizon.
    emv_slippage: [f64; LOOKAHEAD_MAX_HORIZON],
    /// Consecutive breach counter for early warning.
    consecutive_breaches: usize,
}

impl Default for ConformalLookaheadShield {
    fn default() -> Self {
        Self::new()
    }
}

impl ConformalLookaheadShield {
    /// Creates a new Conformal Lookahead Shield with default parameters.
    pub fn new() -> Self {
        Self::with_config(ConformalLookaheadConfig::default())
    }

    /// Creates a new Conformal Lookahead Shield with custom configuration.
    pub fn with_config(config: ConformalLookaheadConfig) -> Self {
        let mut quantiles = [0.0; LOOKAHEAD_MAX_HORIZON];
        let mut ema_slippage = [0.0; LOOKAHEAD_MAX_HORIZON];
        let mut emv_slippage = [0.0; LOOKAHEAD_MAX_HORIZON];

        let h_steps = config.horizon_steps.clamp(1, LOOKAHEAD_MAX_HORIZON);
        for h in 0..h_steps {
            // Initial quantile scale proportional to lookahead step
            let step_scale = (h + 1) as f64;
            quantiles[h] = config.target_slippage_bps * (0.8 + 0.2 * step_scale);
            ema_slippage[h] = config.target_slippage_bps * 0.5 * step_scale;
            emv_slippage[h] = 4.0 * step_scale;
        }

        Self {
            config,
            count: 0,
            head: 0,
            history: [[0.0; LOOKAHEAD_MAX_HORIZON]; LOOKAHEAD_BUFFER_CAPACITY],
            quantiles,
            ema_slippage,
            emv_slippage,
            consecutive_breaches: 0,
        }
    }

    /// Observes a completed multi-step execution trajectory with cumulative slippages at each step.
    ///
    /// # Arguments
    /// * `trajectory_slips` - Slice of cumulative slippages in basis points for steps $1, \dots, H$.
    pub fn observe_trajectory(&mut self, trajectory_slips: &[f64]) {
        let h_steps = self
            .config
            .horizon_steps
            .min(LOOKAHEAD_MAX_HORIZON)
            .min(trajectory_slips.len());
        if h_steps == 0 {
            return;
        }

        let mut row = [0.0; LOOKAHEAD_MAX_HORIZON];
        for (h, &s_raw) in trajectory_slips.iter().take(h_steps).enumerate() {
            let s = s_raw.clamp(0.0, self.config.max_bound_b);
            row[h] = s;

            // Update EMA and EMV with non-exchangeable decay beta
            let diff = s - self.ema_slippage[h];
            self.ema_slippage[h] += (1.0 - self.config.beta) * diff;
            self.emv_slippage[h] =
                self.config.beta * self.emv_slippage[h] + (1.0 - self.config.beta) * (diff * diff);

            // Horizon-discounted alpha
            let alpha_h = (self.config.alpha * self.config.gamma.powi(h as i32)).clamp(0.001, 0.49);

            // Robbins-Monro pinball update
            let indicator = if s > self.quantiles[h] { 1.0 } else { 0.0 };
            let step = self.config.eta * (indicator - alpha_h);
            self.quantiles[h] = (self.quantiles[h] + step).clamp(0.1, self.config.max_bound_b);
        }

        self.history[self.head] = row;
        self.head = (self.head + 1) % LOOKAHEAD_BUFFER_CAPACITY;
        if self.count < LOOKAHEAD_BUFFER_CAPACITY {
            self.count += 1;
        }
    }

    /// Evaluates multi-step lookahead safety against proposed or observed cumulative slippages.
    ///
    /// # Arguments
    /// * `projected_slips` - Projected or in-flight cumulative slippage per horizon step.
    pub fn evaluate_lookahead(&mut self, projected_slips: &[f64]) -> ConformalLookaheadAction {
        let h_steps = self
            .config
            .horizon_steps
            .min(LOOKAHEAD_MAX_HORIZON)
            .min(projected_slips.len());
        if h_steps == 0 {
            return ConformalLookaheadAction::NormalExecution {
                max_projected_slip_bps: 0.0,
                safe_horizon_steps: self.config.horizon_steps,
                throttle_capacity: 1.0,
            };
        }

        // Effective sample size under non-exchangeable decay
        let n = self.count.max(1) as f64;
        let beta = self.config.beta;
        let beta_n = beta.powf(n);
        let n_eff = (((1.0 - beta_n) * (1.0 + beta)) / ((1.0 - beta) * (1.0 + beta_n))).max(2.0);

        let mut max_projected = 0.0;
        let mut first_breached_step = None;
        let mut critical_breach = None;

        for (h, &s) in projected_slips.iter().take(h_steps).enumerate() {
            if s > max_projected {
                max_projected = s;
            }

            let alpha_h = (self.config.alpha * self.config.gamma.powi(h as i32)).clamp(0.001, 0.49);
            let log_term = (2.0 / alpha_h).ln();
            let var = self.emv_slippage[h].max(1e-4);
            let b = self.config.max_bound_b;

            let c1 = (b * log_term) / (3.0 * n_eff);
            let rad = c1 + ((2.0 * var * log_term) / n_eff + c1 * c1).sqrt();

            let bound = (self.quantiles[h] + rad).min(self.config.max_bound_b);

            if s >= self.config.lock_slippage_bps {
                critical_breach = Some((h + 1, s, bound));
                break;
            } else if s > bound && first_breached_step.is_none() {
                first_breached_step = Some((h + 1, s, bound));
            }
        }

        if let Some((breached_step, obs_slip, bound)) = critical_breach {
            self.consecutive_breaches += 1;
            return ConformalLookaheadAction::FailClosedAbort {
                breached_horizon_step: breached_step,
                observed_slip_bps: obs_slip,
                quantile_bound_bps: bound,
                reason: format!(
                    "Critical slippage excursion at lookahead step {breached_step}: {obs_slip:.2} bps exceeds bound {bound:.2} bps"
                ),
            };
        }

        if let Some((breached_step, obs_slip, bound)) = first_breached_step {
            self.consecutive_breaches += 1;
            let safe_steps = (breached_step.saturating_sub(1)).max(1);
            let throttle = (bound / obs_slip.max(1e-4)).clamp(0.20, 0.85);

            return ConformalLookaheadAction::CompressHorizon {
                breached_horizon_step: breached_step,
                safe_horizon_steps: safe_steps,
                max_projected_slip_bps: max_projected,
                throttle_capacity: throttle,
                reason: format!(
                    "Elevated cumulative slippage at step {breached_step}: {obs_slip:.2} bps > {bound:.2} bps, compressing horizon to {safe_steps} steps"
                ),
            };
        }

        self.consecutive_breaches = 0;
        let headroom = (self.config.lock_slippage_bps - max_projected).max(0.0);
        let throttle = (headroom / self.config.lock_slippage_bps).clamp(0.50, 1.0);

        ConformalLookaheadAction::NormalExecution {
            max_projected_slip_bps: max_projected,
            safe_horizon_steps: h_steps,
            throttle_capacity: throttle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conformal_lookahead_normal_execution() {
        let mut shield = ConformalLookaheadShield::new();

        // Feed normal low-slippage trajectories
        for _ in 0..100 {
            shield.observe_trajectory(&[2.0, 4.0, 6.0, 8.0]);
        }

        let action = shield.evaluate_lookahead(&[2.5, 4.5, 6.5, 8.5]);
        match action {
            ConformalLookaheadAction::NormalExecution {
                max_projected_slip_bps,
                safe_horizon_steps,
                throttle_capacity,
            } => {
                assert_eq!(max_projected_slip_bps, 8.5);
                assert_eq!(safe_horizon_steps, 4);
                assert!(throttle_capacity >= 0.70);
            }
            _ => panic!("Expected NormalExecution"),
        }
    }

    #[test]
    fn test_conformal_lookahead_horizon_compression() {
        let mut shield = ConformalLookaheadShield::new();

        for _ in 0..50 {
            shield.observe_trajectory(&[2.0, 4.0, 6.0, 8.0]);
        }

        // Mild breach at step 4 (cumulative 35 bps exceeds bound ~29.5 bps, below 45 bps lock)
        let action = shield.evaluate_lookahead(&[2.0, 4.0, 6.0, 35.0]);
        match action {
            ConformalLookaheadAction::CompressHorizon {
                breached_horizon_step,
                safe_horizon_steps,
                throttle_capacity,
                ..
            } => {
                assert_eq!(breached_horizon_step, 4);
                assert_eq!(safe_horizon_steps, 3);
                assert!(throttle_capacity < 1.0);
            }
            _ => panic!("Expected CompressHorizon"),
        }
    }

    #[test]
    fn test_conformal_lookahead_fail_closed_abort() {
        let mut shield = ConformalLookaheadShield::new();

        for _ in 0..50 {
            shield.observe_trajectory(&[2.0, 4.0, 6.0, 8.0]);
        }

        // Severe excursion breaching lock threshold (50 bps > 45 bps)
        let action = shield.evaluate_lookahead(&[5.0, 15.0, 52.0, 58.0]);
        match action {
            ConformalLookaheadAction::FailClosedAbort {
                breached_horizon_step,
                observed_slip_bps,
                ..
            } => {
                assert_eq!(breached_horizon_step, 3);
                assert_eq!(observed_slip_bps, 52.0);
            }
            _ => panic!("Expected FailClosedAbort"),
        }
    }
}
