//! Adaptive Optimal Liquidation under Stochastic Order Book Depth & Liquidity Resilience.
//!
//! Extends the Almgren-Chriss / Gatheral transient impact framework to incorporate:
//! 1. Stochastic instantaneous order book depth \Lambda_t and depth sweeps.
//! 2. Exponential order book liquidity resilience \beta_t (replenishment rate of resting limit orders).
//! 3. Dynamic pacing decisions that accelerate execution into liquidity surges and throttle or
//!    pause execution during order book depth vacuums, preventing market sweeps across empty price levels.
//!
//! Optimal liquidation velocity follows:
//! v^*(\tau, q, \Lambda) = q \cdot \left[ \frac{\kappa_{eff} \cosh(\kappa_{eff} \tau)}{\sinh(\kappa_{eff} \tau)} \right] \cdot \left(\frac{\Lambda_t}{\bar{\Lambda}}\right)^\alpha
//! where \kappa_{eff} = \sqrt{\lambda_{risk} \sigma^2 \bar{\Lambda}} and \alpha \approx 0.75.

use serde::{Deserialize, Serialize};

/// Adaptive optimal liquidation executor under stochastic order book depth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StochasticDepthLiquidationExecutor {
    /// Risk aversion coefficient \lambda_{risk} > 0
    pub risk_aversion_lambda: f64,
    /// Price volatility \sigma > 0
    pub price_volatility_sigma: f64,
    /// Baseline order book depth \bar{\Lambda} in base asset units per price level
    pub base_depth_lambda_bar: f64,
    /// Order book liquidity resilience rate \beta > 0 (1/sec)
    pub resilience_beta: f64,
    /// Depth sensitivity exponent \alpha in [0.5, 1.0], default 0.75
    pub depth_sensitivity_alpha: f64,
    /// Minimum depth ratio before triggering vacuum throttling (default 0.75)
    pub vacuum_throttle_threshold: f64,
    /// Critical depth ratio below which execution pauses for replenishment (default 0.25)
    pub critical_pause_threshold: f64,
}

impl Default for StochasticDepthLiquidationExecutor {
    fn default() -> Self {
        Self {
            risk_aversion_lambda: 1e-4,
            price_volatility_sigma: 0.005,
            base_depth_lambda_bar: 50.0,
            resilience_beta: 0.25, // Replenishment half-life ~ 2.77s
            depth_sensitivity_alpha: 0.75,
            vacuum_throttle_threshold: 0.75,
            critical_pause_threshold: 0.25,
        }
    }
}

impl StochasticDepthLiquidationExecutor {
    /// Creates a new `StochasticDepthLiquidationExecutor`.
    pub fn new(risk_aversion: f64, volatility: f64, base_depth: f64, resilience_beta: f64) -> Self {
        Self {
            risk_aversion_lambda: risk_aversion.max(1e-7),
            price_volatility_sigma: volatility.max(1e-5),
            base_depth_lambda_bar: base_depth.max(1e-3),
            resilience_beta: resilience_beta.max(1e-3),
            depth_sensitivity_alpha: 0.75,
            vacuum_throttle_threshold: 0.75,
            critical_pause_threshold: 0.25,
        }
    }

    /// Replenishment half-life \tau_{replenish} = ln(2) / \beta (in seconds).
    pub fn replenishment_half_life_sec(&self) -> f64 {
        std::f64::consts::LN_2 / self.resilience_beta
    }

    /// Computes the effective Almgren-Chriss decay rate \kappa_{eff} = \sqrt{\lambda \sigma^2 \bar{\Lambda}}.
    pub fn effective_decay_rate(&self) -> f64 {
        (self.risk_aversion_lambda
            * self.price_volatility_sigma.powi(2)
            * self.base_depth_lambda_bar)
            .sqrt()
            .max(1e-5)
    }

    /// Evaluates the unmodulated baseline Almgren-Chriss liquidation pacing v_{base}^*(\tau, q).
    pub fn base_liquidation_pacing(&self, remaining_qty: f64, remaining_time_sec: f64) -> f64 {
        if remaining_qty <= 0.0 || remaining_time_sec <= 1e-4 {
            return 0.0;
        }

        let tau = remaining_time_sec;
        let kappa = self.effective_decay_rate();
        let sinh_val = (kappa * tau).sinh();

        if sinh_val.abs() > 1e-6 {
            remaining_qty * (kappa * (kappa * tau).cosh() / sinh_val)
        } else {
            remaining_qty / tau
        }
    }

    /// Computes optimal instantaneous liquidation velocity v^*(\tau, q, \Lambda) given observed depth \Lambda_t.
    pub fn compute_optimal_velocity(
        &self,
        remaining_qty: f64,
        remaining_time_sec: f64,
        instantaneous_depth: f64,
    ) -> f64 {
        if remaining_qty <= 0.0 || remaining_time_sec <= 1e-4 {
            return 0.0;
        }

        let base_pace = self.base_liquidation_pacing(remaining_qty, remaining_time_sec);
        let depth_ratio = (instantaneous_depth / self.base_depth_lambda_bar).max(1e-4);

        if depth_ratio < self.critical_pause_threshold {
            return 0.0;
        }

        let depth_multiplier = depth_ratio.powf(self.depth_sensitivity_alpha);
        base_pace * depth_multiplier
    }

    /// Evaluates execution pacing decision based on observed instantaneous book depth:
    pub fn evaluate_pacing_decision(
        &self,
        remaining_qty: f64,
        remaining_time_sec: f64,
        instantaneous_depth: f64,
    ) -> ExecutionPacingDecision {
        if remaining_qty <= 0.0 {
            return ExecutionPacingDecision::Completed;
        }
        if remaining_time_sec <= 1e-4 {
            return ExecutionPacingDecision::ExecuteUrgentFinal { remaining_qty };
        }

        let depth_ratio = instantaneous_depth / self.base_depth_lambda_bar;
        let half_life = self.replenishment_half_life_sec();

        if depth_ratio < self.critical_pause_threshold {
            // Critical vacuum: book depth is hollowed out; pause to allow \beta replenishment
            ExecutionPacingDecision::PauseReplenishmentWait {
                observed_depth: instantaneous_depth,
                depth_ratio,
                recommended_wait_sec: (half_life * 1.5).clamp(0.10, 5.0),
            }
        } else if depth_ratio < self.vacuum_throttle_threshold {
            // Thin book: throttle velocity
            let vel = self.compute_optimal_velocity(
                remaining_qty,
                remaining_time_sec,
                instantaneous_depth,
            );
            ExecutionPacingDecision::ThrottleVacuum {
                throttled_velocity: vel,
                depth_ratio,
                slice_qty: (vel * half_life.min(remaining_time_sec)).min(remaining_qty),
            }
        } else if depth_ratio > 1.35 {
            // Liquidity surge: book is substantially deeper than baseline; accelerate execution
            let vel = self.compute_optimal_velocity(
                remaining_qty,
                remaining_time_sec,
                instantaneous_depth,
            );
            ExecutionPacingDecision::ExecuteSurge {
                accelerated_velocity: vel,
                depth_ratio,
                slice_qty: (vel * 1.0).min(remaining_qty),
            }
        } else {
            // Normal depth
            let vel = self.compute_optimal_velocity(
                remaining_qty,
                remaining_time_sec,
                instantaneous_depth,
            );
            ExecutionPacingDecision::ExecuteNormal {
                velocity: vel,
                depth_ratio,
                slice_qty: (vel * 1.0).min(remaining_qty),
            }
        }
    }
}

/// Dynamic pacing decision for residual liquidation and partial-fill unwinding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExecutionPacingDecision {
    /// Liquidity surge: instantaneous depth is high, safely accelerate execution
    ExecuteSurge {
        accelerated_velocity: f64,
        depth_ratio: f64,
        slice_qty: f64,
    },
    /// Normal book conditions: steady-state Almgren-Chriss pacing
    ExecuteNormal {
        velocity: f64,
        depth_ratio: f64,
        slice_qty: f64,
    },
    /// Book is thin: throttle velocity and slice smaller clips
    ThrottleVacuum {
        throttled_velocity: f64,
        depth_ratio: f64,
        slice_qty: f64,
    },
    /// Critical liquidity vacuum: pause and allow book resilience \beta to restore depth
    PauseReplenishmentWait {
        observed_depth: f64,
        depth_ratio: f64,
        recommended_wait_sec: f64,
    },
    /// Time horizon exhausted: must sweep final remnant
    ExecuteUrgentFinal { remaining_qty: f64 },
    /// Position already flat
    Completed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stochastic_depth_executor_defaults_and_half_life() {
        let exec = StochasticDepthLiquidationExecutor::default();
        let hl = exec.replenishment_half_life_sec();
        // ln(2) / 0.25 ~ 2.7725s
        assert!((hl - 2.7725).abs() < 1e-3, "Half life was {hl}");
        assert!(exec.effective_decay_rate() > 0.0);
    }

    #[test]
    fn test_liquidation_velocity_depth_scaling() {
        let exec = StochasticDepthLiquidationExecutor::new(1e-4, 0.01, 100.0, 0.5);

        let v_norm = exec.compute_optimal_velocity(10.0, 30.0, 100.0);
        let v_deep = exec.compute_optimal_velocity(10.0, 30.0, 200.0);
        let v_thin = exec.compute_optimal_velocity(10.0, 30.0, 50.0);
        let v_hollow = exec.compute_optimal_velocity(10.0, 30.0, 10.0); // < critical pause

        assert!(v_deep > v_norm, "Deep book must yield higher velocity");
        assert!(
            v_norm > v_thin,
            "Normal book must yield higher velocity than thin"
        );
        assert_eq!(v_hollow, 0.0, "Critical hollow book must pause (v = 0)");
    }

    #[test]
    fn test_evaluate_pacing_decision_states() {
        let exec = StochasticDepthLiquidationExecutor::new(1e-4, 0.01, 100.0, 0.5);

        // 1. Surge
        let dec_surge = exec.evaluate_pacing_decision(10.0, 30.0, 150.0);
        assert!(matches!(
            dec_surge,
            ExecutionPacingDecision::ExecuteSurge { .. }
        ));

        // 2. Normal
        let dec_norm = exec.evaluate_pacing_decision(10.0, 30.0, 100.0);
        assert!(matches!(
            dec_norm,
            ExecutionPacingDecision::ExecuteNormal { .. }
        ));

        // 3. Throttle
        let dec_throttle = exec.evaluate_pacing_decision(10.0, 30.0, 50.0);
        assert!(matches!(
            dec_throttle,
            ExecutionPacingDecision::ThrottleVacuum { .. }
        ));

        // 4. Critical Pause
        let dec_pause = exec.evaluate_pacing_decision(10.0, 30.0, 15.0);
        assert!(matches!(
            dec_pause,
            ExecutionPacingDecision::PauseReplenishmentWait { .. }
        ));

        // 5. Final Urgent
        let dec_final = exec.evaluate_pacing_decision(10.0, 0.0, 100.0);
        assert!(matches!(
            dec_final,
            ExecutionPacingDecision::ExecuteUrgentFinal { .. }
        ));

        // 6. Completed
        let dec_flat = exec.evaluate_pacing_decision(0.0, 30.0, 100.0);
        assert!(matches!(dec_flat, ExecutionPacingDecision::Completed));
    }
}
