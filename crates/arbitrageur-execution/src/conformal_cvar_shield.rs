//! Distribution-Free Conformal Risk-Sensitive Reinforcement Learning & Extreme Drawdown Shield.
//!
//! Provides non-asymptotic distribution-free conformal calibration of Conditional Value-at-Risk ($\text{CVaR}_\alpha$)
//! to dynamically shield order execution, scale down quote sizes, or trigger emergency halts during extreme tail slippage events.

use serde::{Deserialize, Serialize};

/// Configuration parameters for the Conformal CVaR Shield.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalCvarConfig {
    /// Tail significance level $\alpha \in (0, 0.5)$ (e.g. 0.05 for 95% CVaR).
    pub alpha: f64,
    /// Maximum tolerable CVaR in basis points (e.g. 35.0 bps).
    pub max_tolerable_cvar_bps: f64,
    /// Minimum calibration observations before trusting conformal bounds.
    pub min_calibration_samples: usize,
    /// Rolling window capacity for recent loss observations.
    pub window_size: usize,
}

impl Default for ConformalCvarConfig {
    fn default() -> Self {
        Self {
            alpha: 0.05,
            max_tolerable_cvar_bps: 35.0,
            min_calibration_samples: 40,
            window_size: 200,
        }
    }
}

/// Action verdict produced by the Conformal CVaR Shield.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ShieldVerdict {
    /// Risk is well within tolerance; full capital sizing allowed.
    AllowNormalExecution { scale: f64, estimated_cvar_bps: f64 },
    /// Conformal CVaR exceeds baseline tolerance; size quadratically throttled.
    ThrottledExecution {
        scale: f64,
        estimated_cvar_bps: f64,
        reason: String,
    },
    /// Extreme tail risk detected; trading halted or positions evacuated.
    HaltOrEvacuate {
        estimated_cvar_bps: f64,
        reason: String,
    },
}

/// Real-time Conformal CVaR Shield Engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalCvarShieldEngine {
    pub config: ConformalCvarConfig,
    pub loss_history: Vec<f64>,
}

impl ConformalCvarShieldEngine {
    pub fn new(config: ConformalCvarConfig) -> Self {
        Self {
            config,
            loss_history: Vec::with_capacity(256),
        }
    }

    /// Appends an empirical execution loss or adverse slippage observation in basis points.
    pub fn record_loss_bps(&mut self, loss_bps: f64) {
        if self.loss_history.len() >= self.config.window_size {
            self.loss_history.remove(0);
        }
        self.loss_history.push(loss_bps);
    }

    /// Evaluates non-parametric conformal Value-at-Risk $\widehat{\text{VaR}}_\alpha$.
    pub fn compute_conformal_var(&self) -> Option<f64> {
        let n = self.loss_history.len();
        if n < self.config.min_calibration_samples {
            return None;
        }

        let mut sorted = self.loss_history.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        // Conformal quantile index: ceil((n + 1) * (1 - alpha)) - 1
        let rank = (((n + 1) as f64 * (1.0 - self.config.alpha)).ceil() as usize).min(n);
        let idx = if rank == 0 { 0 } else { rank - 1 };

        Some(sorted[idx])
    }

    /// Evaluates non-parametric conformal Conditional Value-at-Risk $\widehat{\text{CVaR}}_\alpha$.
    pub fn compute_conformal_cvar(&self) -> Option<f64> {
        let var = self.compute_conformal_var()?;
        let mut sorted = self.loss_history.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        // Gather all tail losses >= var
        let tail_losses: Vec<f64> = sorted.into_iter().filter(|&l| l >= var).collect();
        if tail_losses.is_empty() {
            return Some(var);
        }

        let sum: f64 = tail_losses.iter().sum();
        let cvar = sum / tail_losses.len() as f64;
        Some(cvar.max(var))
    }

    /// Evaluates the risk shielding action based on conformal bounds.
    pub fn evaluate_shield(&self) -> ShieldVerdict {
        let cvar = match self.compute_conformal_cvar() {
            Some(val) => val,
            None => {
                // Fail-closed during calibration warmup
                return ShieldVerdict::ThrottledExecution {
                    scale: 0.5,
                    estimated_cvar_bps: 0.0,
                    reason: format!(
                        "Warmup: insufficient calibration samples ({}/{})",
                        self.loss_history.len(),
                        self.config.min_calibration_samples
                    ),
                };
            }
        };

        let max_tol = self.config.max_tolerable_cvar_bps;

        if cvar <= max_tol {
            ShieldVerdict::AllowNormalExecution {
                scale: 1.0,
                estimated_cvar_bps: cvar,
            }
        } else if cvar <= max_tol * 2.0 {
            // Quadratic risk throttle: scale = (max_tol / cvar)^2
            let ratio = max_tol / cvar;
            let scale = (ratio * ratio).clamp(0.1, 0.95);
            ShieldVerdict::ThrottledExecution {
                scale,
                estimated_cvar_bps: cvar,
                reason: format!(
                    "Conformal CVaR {:.1} bps exceeds tolerance {:.1} bps; throttled to {:.1}%",
                    cvar,
                    max_tol,
                    scale * 100.0
                ),
            }
        } else {
            ShieldVerdict::HaltOrEvacuate {
                estimated_cvar_bps: cvar,
                reason: format!(
                    "Extreme Drawdown: Conformal CVaR {:.1} bps exceeds critical threshold {:.1} bps (> 2x limit)",
                    cvar,
                    max_tol * 2.0
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conformal_cvar_coverage_and_scaling() {
        let config = ConformalCvarConfig {
            alpha: 0.05,
            max_tolerable_cvar_bps: 30.0,
            min_calibration_samples: 40,
            window_size: 100,
        };

        let mut engine = ConformalCvarShieldEngine::new(config);

        // Before sufficient calibration samples, must fail-closed with warmup throttle
        for _ in 0..10 {
            engine.record_loss_bps(5.0);
        }
        match engine.evaluate_shield() {
            ShieldVerdict::ThrottledExecution { scale, reason, .. } => {
                assert_eq!(scale, 0.5);
                assert!(reason.contains("Warmup"));
            }
            _ => panic!("Must throttle during warmup"),
        }

        // Fill up to 50 calm observations (mean loss ~10 bps, max ~20 bps)
        for i in 11..=50 {
            engine.record_loss_bps(5.0 + (i as f64 % 15.0));
        }

        match engine.evaluate_shield() {
            ShieldVerdict::AllowNormalExecution {
                scale,
                estimated_cvar_bps,
            } => {
                assert_eq!(scale, 1.0);
                assert!(estimated_cvar_bps < 30.0);
            }
            _ => panic!("Should allow normal execution under calm losses"),
        }

        // Inject moderate losses to cause throttling (losses between 30 and 45 bps)
        for _ in 0..15 {
            engine.record_loss_bps(42.0);
        }

        match engine.evaluate_shield() {
            ShieldVerdict::ThrottledExecution {
                scale,
                estimated_cvar_bps,
                ..
            } => {
                assert!((0.1..1.0).contains(&scale));
                assert!(estimated_cvar_bps > 30.0);
            }
            _ => panic!("Must throttle when CVaR breaches 30.0 bps"),
        }
    }

    #[test]
    fn test_extreme_drawdown_evacuation() {
        let config = ConformalCvarConfig {
            alpha: 0.05,
            max_tolerable_cvar_bps: 25.0,
            min_calibration_samples: 40,
            window_size: 100,
        };

        let mut engine = ConformalCvarShieldEngine::new(config);
        for _ in 0..40 {
            engine.record_loss_bps(10.0);
        }
        // Inject extreme flash crash losses exceeding 2x limit (limit is 25.0, 2x is 50.0)
        for _ in 0..10 {
            engine.record_loss_bps(75.0);
        }

        match engine.evaluate_shield() {
            ShieldVerdict::HaltOrEvacuate {
                estimated_cvar_bps,
                reason,
            } => {
                assert!(estimated_cvar_bps >= 50.0);
                assert!(reason.contains("Extreme Drawdown"));
            }
            _ => panic!("Must trigger HaltOrEvacuate during extreme tail loss shock"),
        }
    }
}
