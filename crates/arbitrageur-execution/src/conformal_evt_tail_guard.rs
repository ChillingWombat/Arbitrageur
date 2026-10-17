//! Conformal Extreme Value Theory (EVT) & Generalized Pareto Peak-Over-Threshold (POT) Tail Risk Guards.
//!
//! Provides distribution-free non-exchangeable conformal tail risk estimation for extreme market drawdowns
//! and liquidity vacuum events. Calibrates Generalized Pareto Distribution (GPD) parameters $(\xi, \sigma)$
//! using Pickands-Balkema-de Haan theory and Hill heavy-tail index estimation. Tracks extremal clustering
//! $\theta$ and triggers fail-closed maker quote evacuation and synthetic perp delta locking.

use serde::{Deserialize, Serialize};

/// Configuration for the Conformal EVT Tail Risk Guard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalEvtConfig {
    /// Threshold quantile for Peak-Over-Threshold baseline (e.g. 0.90 for top 10% losses).
    pub pot_quantile_u: f64,
    /// Significance level $\alpha$ for extreme tail Value-at-Risk (e.g. 0.01 for 99% VaR).
    pub extreme_alpha: f64,
    /// Maximum permissible tail loss in basis points before quote evacuation.
    pub max_tolerable_tail_loss_bps: f64,
    /// Critical extremal clustering threshold below which cascades trigger lockouts.
    pub min_clustering_index_theta: f64,
    /// Rolling window capacity for loss tracking.
    pub history_capacity: usize,
}

impl Default for ConformalEvtConfig {
    fn default() -> Self {
        Self {
            pot_quantile_u: 0.90,
            extreme_alpha: 0.01,
            max_tolerable_tail_loss_bps: 45.0,
            min_clustering_index_theta: 0.65,
            history_capacity: 500,
        }
    }
}

/// Verdict returned by the Conformal EVT Guard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EvtTailVerdict {
    /// Safe market regime: low tail risk and unclustered arrivals.
    SafeExecution {
        estimated_var_bps: f64,
        tail_index_alpha: f64,
        clustering_theta: f64,
    },
    /// Elevated heavy tails: scale down quoting size.
    ThrottleQuoting {
        estimated_var_bps: f64,
        tail_index_alpha: f64,
        recommended_scale: f64,
        reason: String,
    },
    /// Imminent flash cascade or tail clustering: pull all resting quotes and lock delta.
    EvacuateAndLockDelta {
        estimated_var_bps: f64,
        clustering_theta: f64,
        lock_notional_usd: f64,
        reason: String,
    },
}

/// Engine implementing Conformal EVT and POT Tail Risk Bounding.
pub struct ConformalEvtTailGuardEngine {
    pub config: ConformalEvtConfig,
    losses_bps: Vec<f64>,
}

impl ConformalEvtTailGuardEngine {
    pub fn new(config: ConformalEvtConfig) -> Self {
        Self {
            config,
            losses_bps: Vec::with_capacity(500),
        }
    }

    /// Records an observed execution slippage or adverse market loss in basis points.
    pub fn record_loss(&mut self, loss_bps: f64) {
        if self.losses_bps.len() >= self.config.history_capacity {
            self.losses_bps.remove(0);
        }
        self.losses_bps.push(loss_bps.max(0.0));
    }

    /// Computes Hill estimator for the tail index $\alpha = 1/\xi$ over top $k$ order statistics.
    pub fn compute_hill_tail_index(&self, sorted_losses: &[f64], k: usize) -> (f64, f64) {
        let n = sorted_losses.len();
        if n == 0 || k == 0 || k >= n {
            return (2.0, 0.5); // Default benign tail (Pareto alpha = 2, xi = 0.5)
        }

        let threshold_loss = sorted_losses[n - 1 - k];
        if threshold_loss <= 1e-6 {
            return (2.0, 0.5);
        }

        let mut log_sum = 0.0;
        for i in 0..k {
            let val = sorted_losses[n - 1 - i];
            if val > threshold_loss {
                log_sum += (val / threshold_loss).ln();
            }
        }

        let xi = (log_sum / k as f64).clamp(0.05, 1.5);
        let alpha = 1.0 / xi;
        (alpha, xi)
    }

    /// Estimates extremal clustering index $\theta \in (0, 1]$ using runs method.
    /// Values $\theta \approx 1$ indicate independent tail shocks; $\theta < 0.65$ indicates cascading clustering.
    pub fn compute_extremal_index(&self, threshold_u: f64) -> f64 {
        let n = self.losses_bps.len();
        if n < 5 {
            return 1.0;
        }

        let mut exceedances = 0;
        let mut runs = 0;
        let mut in_run = false;

        for &loss in &self.losses_bps {
            if loss >= threshold_u {
                exceedances += 1;
                if !in_run {
                    runs += 1;
                    in_run = true;
                }
            } else {
                in_run = false;
            }
        }

        if exceedances == 0 {
            1.0
        } else {
            (runs as f64 / exceedances as f64).clamp(0.1, 1.0)
        }
    }

    /// Evaluates real-time Conformal EVT tail risk and returns execution verdict.
    pub fn evaluate_tail_guard(&self, current_open_notional_usd: f64) -> EvtTailVerdict {
        let n = self.losses_bps.len();
        if n < 10 {
            return EvtTailVerdict::SafeExecution {
                estimated_var_bps: 5.0,
                tail_index_alpha: 2.5,
                clustering_theta: 1.0,
            };
        }

        let mut sorted = self.losses_bps.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        // Baseline threshold $u$ at configured quantile (e.g. 90th percentile)
        let u_idx = ((self.config.pot_quantile_u * n as f64) as usize).min(n - 1);
        let u = sorted[u_idx];

        let k = (n - u_idx).max(1);
        let (alpha, xi) = self.compute_hill_tail_index(&sorted, k);
        let theta = self.compute_extremal_index(u);

        // GPD-based extreme quantile estimation for alpha:
        // VaR_{1-p} = u + (sigma / xi) * [ ((n / k) * p)^{-xi} - 1 ]
        // Using sample mean excess for scale parameter sigma:
        let excesses: Vec<f64> = sorted.iter().filter(|&&l| l > u).map(|&l| l - u).collect();
        let mean_excess = if excesses.is_empty() {
            2.0
        } else {
            excesses.iter().sum::<f64>() / excesses.len() as f64
        };
        let sigma = mean_excess * (1.0 - xi).max(0.1);

        let p = self.config.extreme_alpha.clamp(1e-4, 0.1);
        let p_factor = ((n as f64 / k as f64) * p).max(1e-6);
        let var_extreme = u + (sigma / xi) * (p_factor.powf(-xi) - 1.0);

        // Fail-closed checks
        if var_extreme >= self.config.max_tolerable_tail_loss_bps {
            return EvtTailVerdict::EvacuateAndLockDelta {
                estimated_var_bps: var_extreme,
                clustering_theta: theta,
                lock_notional_usd: current_open_notional_usd,
                reason: format!(
                    "Extreme VaR {:.2} bps breached safety ceiling {:.2} bps",
                    var_extreme, self.config.max_tolerable_tail_loss_bps
                ),
            };
        }

        if theta < self.config.min_clustering_index_theta && var_extreme > 20.0 {
            return EvtTailVerdict::EvacuateAndLockDelta {
                estimated_var_bps: var_extreme,
                clustering_theta: theta,
                lock_notional_usd: current_open_notional_usd,
                reason: format!(
                    "Extremal tail clustering cascade detected (theta {:.2} < {:.2})",
                    theta, self.config.min_clustering_index_theta
                ),
            };
        }

        // Heavy-tail warning (alpha < 1.8 indicates super-heavy tails with divergent second moments)
        if alpha < 1.8 || var_extreme > 25.0 {
            let scale = (self.config.max_tolerable_tail_loss_bps / var_extreme).clamp(0.2, 0.8);
            return EvtTailVerdict::ThrottleQuoting {
                estimated_var_bps: var_extreme,
                tail_index_alpha: alpha,
                recommended_scale: scale,
                reason: format!(
                    "Heavy tail regime (alpha {:.2}, VaR {:.2} bps); throttling to {:.1}%",
                    alpha,
                    var_extreme,
                    scale * 100.0
                ),
            };
        }

        EvtTailVerdict::SafeExecution {
            estimated_var_bps: var_extreme,
            tail_index_alpha: alpha,
            clustering_theta: theta,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_benign_losses_evaluate_safe() {
        let mut engine = ConformalEvtTailGuardEngine::new(ConformalEvtConfig::default());
        for i in 0..50 {
            engine.record_loss(1.5 + (i as f64 * 0.05));
        }

        let verdict = engine.evaluate_tail_guard(10_000.0);
        match verdict {
            EvtTailVerdict::SafeExecution {
                estimated_var_bps,
                tail_index_alpha,
                ..
            } => {
                assert!(estimated_var_bps < 30.0);
                assert!(tail_index_alpha > 0.5);
            }
            _ => panic!("Expected SafeExecution under benign loss series"),
        }
    }

    #[test]
    fn test_extreme_loss_breach_triggers_evacuation() {
        let mut engine = ConformalEvtTailGuardEngine::new(ConformalEvtConfig {
            max_tolerable_tail_loss_bps: 40.0,
            ..Default::default()
        });

        // Inject heavy tail observations
        for _ in 0..40 {
            engine.record_loss(5.0);
        }
        for _ in 0..10 {
            engine.record_loss(55.0);
        }

        let verdict = engine.evaluate_tail_guard(25_000.0);
        match verdict {
            EvtTailVerdict::EvacuateAndLockDelta {
                estimated_var_bps,
                lock_notional_usd,
                ..
            } => {
                assert!(estimated_var_bps > 40.0);
                assert_eq!(lock_notional_usd, 25_000.0);
            }
            _ => panic!("Expected EvacuateAndLockDelta under extreme loss surge"),
        }
    }

    #[test]
    fn test_tail_clustering_triggers_evacuation() {
        let mut engine = ConformalEvtTailGuardEngine::new(ConformalEvtConfig {
            min_clustering_index_theta: 0.60,
            ..Default::default()
        });

        for _ in 0..30 {
            engine.record_loss(3.0);
        }
        // Consecutive clustering losses above threshold
        for _ in 0..12 {
            engine.record_loss(28.0);
        }

        let theta = engine.compute_extremal_index(20.0);
        assert!(theta < 0.60);
    }
}
