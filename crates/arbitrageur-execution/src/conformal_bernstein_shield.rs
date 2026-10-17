//! Distribution-Free Conformal Empirical Bernstein Bounds & Sub-Gamma Worst-Case Slippage Shields.
//!
//! Models execution loss tails via variance-sensitive empirical Bernstein concentration bounds
//! under non-exchangeable exponential recency weighting $\beta = 0.985$:
//! $$B_{1-\alpha}(L) = \bar{L}_t + \sqrt{\frac{2 V_t \ln(2/\alpha)}{N_{\text{eff}}}} + \frac{3 c \ln(2/\alpha)}{N_{\text{eff}}}$$
//! where $V_t$ is the exponentially weighted empirical variance and $N_{\text{eff}} = \frac{(\sum w_i)^2}{\sum w_i^2}$
//! is the effective historical sample size.
//! Enforces sub-gamma tail concentration envelopes and dynamically modulates maker quoting scale factors $\kappa_t$,
//! triggering microsecond fail-closed maker order pulling and synthetic perpetual delta locking when the
//! empirical Bernstein bound breaches critical risk hurdles.

use serde::{Deserialize, Serialize};

/// Configuration for the Conformal Empirical Bernstein Shield.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalBernsteinConfig {
    /// Significance level $\alpha \in (0.001, 0.15)$ (default 0.05 for 95% coverage).
    pub alpha: f64,
    /// Exponential recency decay parameter $\beta \in (0.80, 0.999)$ (default 0.985).
    pub recency_decay_beta: f64,
    /// Maximum loss bound scale parameter $c$ in basis points (default 100.0).
    pub max_loss_scale_c_bps: f64,
    /// Safe baseline Bernstein bound hurdle in basis points below which 100% sizing is permitted.
    pub safe_hurdle_bps: f64,
    /// Critical Bernstein bound threshold in basis points above which execution halts ($\kappa = 0$).
    pub critical_hurdle_bps: f64,
    /// Rolling loss history capacity.
    pub history_capacity: usize,
}

impl Default for ConformalBernsteinConfig {
    fn default() -> Self {
        Self {
            alpha: 0.05,
            recency_decay_beta: 0.985,
            max_loss_scale_c_bps: 100.0,
            safe_hurdle_bps: 15.0,
            critical_hurdle_bps: 45.0,
            history_capacity: 500,
        }
    }
}

/// Verdict emitted by the Conformal Empirical Bernstein Shield.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BernsteinShieldVerdict {
    /// Safe loss regime: full quote allocation permitted ($\kappa = 1.0$).
    NormalExecution {
        empirical_mean_bps: f64,
        empirical_variance_bps2: f64,
        bernstein_bound_bps: f64,
        scale_factor: f64,
    },
    /// Elevated loss variance or mean: scale quoting notional down proportionally.
    DeriskingThrottle {
        empirical_mean_bps: f64,
        empirical_variance_bps2: f64,
        bernstein_bound_bps: f64,
        scale_factor: f64,
        reason: String,
    },
    /// Critical tail risk breach: pull resting maker quotes and lock synthetic delta.
    EvacuateAndLockDelta {
        empirical_mean_bps: f64,
        empirical_variance_bps2: f64,
        bernstein_bound_bps: f64,
        lock_notional_usd: f64,
        reason: String,
    },
}

/// Engine implementing Distribution-Free Conformal Empirical Bernstein Shields.
pub struct ConformalBernsteinShieldEngine {
    pub config: ConformalBernsteinConfig,
    losses_bps: Vec<f64>,
}

impl ConformalBernsteinShieldEngine {
    pub fn new(config: ConformalBernsteinConfig) -> Self {
        Self {
            config,
            losses_bps: Vec::with_capacity(500),
        }
    }

    /// Records an observed execution loss in basis points.
    pub fn record_loss(&mut self, loss_bps: f64) {
        if self.losses_bps.len() >= self.config.history_capacity {
            self.losses_bps.remove(0);
        }
        self.losses_bps.push(loss_bps.max(0.0));
    }

    /// Evaluates weighted empirical mean $\bar{L}_t$, empirical variance $V_t$, and effective sample size $N_{\text{eff}}$.
    pub fn compute_weighted_moments(&self) -> (f64, f64, f64) {
        let n = self.losses_bps.len();
        if n == 0 {
            return (0.0, 0.0, 1.0);
        }

        let beta = self.config.recency_decay_beta.clamp(0.5, 0.999);
        let mut sum_w = 0.0;
        let mut sum_w_sq = 0.0;
        let mut sum_w_loss = 0.0;

        for (i, &loss) in self.losses_bps.iter().enumerate() {
            let w = beta.powi((n - 1 - i) as i32);
            sum_w += w;
            sum_w_sq += w * w;
            sum_w_loss += w * loss;
        }

        let mean = sum_w_loss / sum_w;

        let mut sum_w_var = 0.0;
        for (i, &loss) in self.losses_bps.iter().enumerate() {
            let w = beta.powi((n - 1 - i) as i32);
            let diff = loss - mean;
            sum_w_var += w * diff * diff;
        }

        let variance = (sum_w_var / sum_w).max(0.0);
        let n_eff = (sum_w * sum_w / sum_w_sq).max(1.0);

        (mean, variance, n_eff)
    }

    /// Evaluates the empirical Bernstein concentration upper bound $B_{1-\alpha}(L)$.
    pub fn compute_bernstein_bound(&self) -> (f64, f64, f64) {
        let (mean, variance, n_eff) = self.compute_weighted_moments();
        let alpha = self.config.alpha.clamp(1e-4, 0.5);
        let log_term = (2.0 / alpha).ln();
        let c = self.config.max_loss_scale_c_bps.max(1.0);

        let variance_term = (2.0 * variance * log_term / n_eff).sqrt();
        let scale_term = (3.0 * c * log_term) / n_eff;

        let bernstein_bound = mean + variance_term + scale_term;
        (mean, variance, bernstein_bound)
    }

    /// Evaluates the shield verdict and continuous derisking multiplier $\kappa_t$.
    pub fn evaluate_shield(&self, current_exposure_usd: f64) -> BernsteinShieldVerdict {
        if self.losses_bps.is_empty() {
            return BernsteinShieldVerdict::NormalExecution {
                empirical_mean_bps: 0.0,
                empirical_variance_bps2: 0.0,
                bernstein_bound_bps: 0.0,
                scale_factor: 1.0,
            };
        }

        let (mean, variance, bound) = self.compute_bernstein_bound();

        if bound >= self.config.critical_hurdle_bps {
            BernsteinShieldVerdict::EvacuateAndLockDelta {
                empirical_mean_bps: mean,
                empirical_variance_bps2: variance,
                bernstein_bound_bps: bound,
                lock_notional_usd: current_exposure_usd,
                reason: format!(
                    "Critical Bernstein bound breach: {:.2} bps >= {:.2} bps (mean={:.2}, var={:.2}). Evacuating quotes & locking delta.",
                    bound, self.config.critical_hurdle_bps, mean, variance
                ),
            }
        } else if bound > self.config.safe_hurdle_bps {
            let denominator = self.config.critical_hurdle_bps - self.config.safe_hurdle_bps;
            let scale_factor =
                ((self.config.critical_hurdle_bps - bound) / denominator).clamp(0.0, 1.0);

            BernsteinShieldVerdict::DeriskingThrottle {
                empirical_mean_bps: mean,
                empirical_variance_bps2: variance,
                bernstein_bound_bps: bound,
                scale_factor,
                reason: format!(
                    "Elevated Bernstein risk: {:.2} bps > safe hurdle {:.2} bps. Scaling quotes to {:.1}%.",
                    bound, self.config.safe_hurdle_bps, scale_factor * 100.0
                ),
            }
        } else {
            BernsteinShieldVerdict::NormalExecution {
                empirical_mean_bps: mean,
                empirical_variance_bps2: variance,
                bernstein_bound_bps: bound,
                scale_factor: 1.0,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normal_losses_give_normal_execution() {
        let mut engine = ConformalBernsteinShieldEngine::new(ConformalBernsteinConfig {
            safe_hurdle_bps: 20.0,
            critical_hurdle_bps: 50.0,
            ..Default::default()
        });

        // 100 observations of low losses (2.0 bps)
        for _ in 0..100 {
            engine.record_loss(2.0);
        }

        let verdict = engine.evaluate_shield(100_000.0);
        match verdict {
            BernsteinShieldVerdict::NormalExecution {
                scale_factor,
                bernstein_bound_bps,
                ..
            } => {
                assert_eq!(scale_factor, 1.0);
                assert!(bernstein_bound_bps < 20.0);
            }
            _ => panic!("Expected NormalExecution for calm loss stream"),
        }
    }

    #[test]
    fn test_moderate_variance_triggers_derisking_throttle() {
        let mut engine = ConformalBernsteinShieldEngine::new(ConformalBernsteinConfig {
            safe_hurdle_bps: 15.0,
            critical_hurdle_bps: 45.0,
            ..Default::default()
        });

        // Alternating moderate and elevated losses
        for i in 0..80 {
            let loss = if i % 2 == 0 { 10.0 } else { 22.0 };
            engine.record_loss(loss);
        }

        let verdict = engine.evaluate_shield(200_000.0);
        match verdict {
            BernsteinShieldVerdict::DeriskingThrottle {
                scale_factor,
                bernstein_bound_bps,
                ..
            } => {
                assert!(bernstein_bound_bps > 15.0 && bernstein_bound_bps < 45.0);
                assert!(scale_factor > 0.0 && scale_factor < 1.0);
            }
            _ => panic!("Expected DeriskingThrottle for elevated variance"),
        }
    }

    #[test]
    fn test_severe_losses_trigger_evacuation_and_lock() {
        let mut engine = ConformalBernsteinShieldEngine::new(ConformalBernsteinConfig {
            safe_hurdle_bps: 15.0,
            critical_hurdle_bps: 45.0,
            ..Default::default()
        });

        // Severe loss shocks (55.0 bps)
        for _ in 0..60 {
            engine.record_loss(55.0);
        }

        let verdict = engine.evaluate_shield(500_000.0);
        match verdict {
            BernsteinShieldVerdict::EvacuateAndLockDelta {
                lock_notional_usd,
                bernstein_bound_bps,
                ..
            } => {
                assert_eq!(lock_notional_usd, 500_000.0);
                assert!(bernstein_bound_bps >= 45.0);
            }
            _ => panic!("Expected EvacuateAndLockDelta for severe loss breach"),
        }
    }

    #[test]
    fn test_bernstein_bound_monotonicity_in_variance() {
        let mut engine_low =
            ConformalBernsteinShieldEngine::new(ConformalBernsteinConfig::default());
        let mut engine_high =
            ConformalBernsteinShieldEngine::new(ConformalBernsteinConfig::default());

        for _ in 0..100 {
            engine_low.record_loss(10.0);
        }
        for i in 0..100 {
            let l = if i % 2 == 0 { 0.0 } else { 20.0 };
            engine_high.record_loss(l);
        }

        let (_, var_low, bound_low) = engine_low.compute_bernstein_bound();
        let (_, var_high, bound_high) = engine_high.compute_bernstein_bound();

        assert!(var_high > var_low);
        assert!(bound_high > bound_low);
    }
}
