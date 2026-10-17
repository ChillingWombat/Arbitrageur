//! Endogenous Volatility Jump Protections and Microstructure Jump Separation.
//!
//! Provides non-parametric separation of continuous diffusion from discontinuous jump shocks:
//! 1. **Barndorff-Nielsen & Shephard (2004) Bipower Variation (BV)**: Consistent estimator of
//!    integrated variance asymptotically robust against finite-activity jumps.
//! 2. **Relative Jump Ratio ($RJ_t$)**: Fraction of realized quadratic variation driven by jumps:
//!    $$RJ_t = \frac{\text{RV}_t - \text{BV}_t}{\text{RV}_t} \in [0, 1]$$
//! 3. **Mancini (2009) Threshold Jump Separation**:
//!    $$\Delta J_t = \Delta X_t \cdot \mathbf{1}_{\{|\Delta X_t| > \alpha \cdot \text{BV}_t^{0.5} \cdot \Delta t^\varpi\}}$$
//! 4. **Asymmetric Volatility Circuit Breaker**: Instantly commands maker quote cancellation and
//!    dynamic spread widening during directional liquidation cascades.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::f64::consts::PI;

/// Asymmetric action commanded by jump circuit breaker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum JumpAction {
    /// Severe negative jump (long liquidation cascade): pull resting bids, widen ask spread.
    PullBidsAndWidenAsks {
        jump_return_bps: f64,
        relative_jump_ratio: f64,
        hurdle_expansion_bps: f64,
    },
    /// Severe positive jump (short squeeze): pull resting asks, widen bid spread.
    PullAsksAndWidenBids {
        jump_return_bps: f64,
        relative_jump_ratio: f64,
        hurdle_expansion_bps: f64,
    },
    /// Elevated jump activity without strong single-tick directional breach: widen both sides.
    DefensiveSpreadWidening {
        relative_jump_ratio: f64,
        hurdle_expansion_bps: f64,
    },
    /// Normal continuous diffusion regime.
    NormalDiffusion,
}

/// Comprehensive volatility jump analysis report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JumpAssessment {
    pub is_jump: bool,
    pub latest_return_bps: f64,
    pub realized_variance: f64,
    pub bipower_variation: f64,
    pub relative_jump_ratio: f64,
    pub continuous_variance: f64,
    pub jump_variance: f64,
    pub dynamic_threshold_bps: f64,
    pub z_jump_stat: f64,
    pub action: JumpAction,
}

/// Online Mancini (2009) & Barndorff-Nielsen & Shephard Bipower Variation Jump Detector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManciniBipowerJumpDetector {
    pub window_size: usize,
    price_history: VecDeque<f64>,
    pub alpha_multiplier: f64,
    pub varpi_exponent: f64,
    pub jump_ratio_trigger: f64,
    pub base_widening_multiplier: f64,
}

impl Default for ManciniBipowerJumpDetector {
    fn default() -> Self {
        Self {
            window_size: 60,
            price_history: VecDeque::with_capacity(64),
            alpha_multiplier: 3.5,
            varpi_exponent: 0.49,
            jump_ratio_trigger: 0.35,
            base_widening_multiplier: 3.0,
        }
    }
}

impl ManciniBipowerJumpDetector {
    pub fn new(
        window_size: usize,
        alpha_multiplier: f64,
        varpi_exponent: f64,
        jump_ratio_trigger: f64,
    ) -> Self {
        let win = window_size.max(20);
        Self {
            window_size: win,
            price_history: VecDeque::with_capacity(win + 4),
            alpha_multiplier: alpha_multiplier.clamp(1.5, 6.0),
            varpi_exponent: varpi_exponent.clamp(0.1, 0.5),
            jump_ratio_trigger: jump_ratio_trigger.clamp(0.10, 0.90),
            base_widening_multiplier: 3.0,
        }
    }

    /// Resets historical observations.
    pub fn reset(&mut self) {
        self.price_history.clear();
    }

    /// Ingests a new price tick and evaluates jump indicators.
    ///
    /// # Arguments
    /// - `price`: Current mid or mark price.
    /// - `dt_sec`: Time delta from previous observation in seconds.
    pub fn ingest_tick(&mut self, price: f64, dt_sec: f64) -> JumpAssessment {
        if !price.is_finite() || price <= 0.0 {
            return self.empty_assessment(JumpAction::NormalDiffusion);
        }

        self.price_history.push_back(price);
        if self.price_history.len() > self.window_size {
            self.price_history.pop_front();
        }

        let n = self.price_history.len();
        if n < 6 || dt_sec <= 0.0 {
            return self.empty_assessment(JumpAction::NormalDiffusion);
        }

        // 1. Compute log returns: r_i = ln(P_i / P_{i-1})
        let mut returns = Vec::with_capacity(n - 1);
        for i in 1..n {
            let r = (self.price_history[i] / self.price_history[i - 1]).ln();
            returns.push(r);
        }

        let m = returns.len();
        let latest_return = *returns.last().unwrap_or(&0.0);
        let latest_return_bps = latest_return * 10000.0;

        // 2. Realized Quadratic Variation: RV = \sum r_i^2
        let rv: f64 = returns.iter().map(|&r| r * r).sum();

        // 3. Barndorff-Nielsen & Shephard Bipower Variation:
        // BV = (pi / 2) * (M / (M - 1)) * \sum_{i=2}^M |r_i| |r_{i-1}|
        let mut bv_inner = 0.0;
        for i in 1..m {
            bv_inner += returns[i].abs() * returns[i - 1].abs();
        }
        let bv = (PI / 2.0) * (m as f64 / (m as f64 - 1.0)) * bv_inner;

        // 4. Relative Jump Ratio: RJ = max(0, (RV - BV) / RV)
        let rj = if rv > 1e-12 {
            ((rv - bv) / rv).clamp(0.0, 1.0)
        } else {
            0.0
        };

        // 5. Tri-power Quarticity for asymptotic test statistic:
        // TP = mu_{4/3}^-3 * (M^2 / (M - 2)) * \sum_{i=3}^M |r_i|^{4/3} |r_{i-1}|^{4/3} |r_{i-2}|^{4/3}
        // with mu_{4/3} \approx 0.83086 => mu_{4/3}^-3 \approx 1.745
        let mut tp_inner = 0.0;
        for i in 2..m {
            tp_inner += returns[i].abs().powf(4.0 / 3.0)
                * returns[i - 1].abs().powf(4.0 / 3.0)
                * returns[i - 2].abs().powf(4.0 / 3.0);
        }
        let tp = 1.745 * ((m * m) as f64 / ((m - 2) as f64)) * tp_inner;

        let z_stat = if m >= 10 && bv > 1e-12 && rv > 1e-12 {
            let denom_var = 0.609066 * (1.0 / m as f64) * (tp / (bv * bv)).max(1.0);
            (rj / denom_var.sqrt().max(1e-6)).max(0.0)
        } else {
            0.0
        };

        // 6. Mancini Threshold: \theta = \alpha * sqrt(BV) * dt^\varpi
        let theta = self.alpha_multiplier * bv.sqrt() * dt_sec.powf(self.varpi_exponent);
        let theta_bps = theta * 10000.0;

        // 7. Threshold Jump Decomposition: separate continuous vs jump components
        let mut cont_var = 0.0;
        let mut jump_var = 0.0;
        for &r in &returns {
            if r.abs() <= theta {
                cont_var += r * r;
            } else {
                jump_var += r * r;
            }
        }

        // 8. Single-tick jump breach check
        let is_tick_jump = latest_return.abs() > theta && rj > self.jump_ratio_trigger;

        // 9. Command Asymmetric Circuit Action
        let action = if is_tick_jump {
            let expansion = (latest_return_bps.abs() * self.base_widening_multiplier * rj).max(5.0);
            if latest_return < 0.0 {
                JumpAction::PullBidsAndWidenAsks {
                    jump_return_bps: latest_return_bps,
                    relative_jump_ratio: rj,
                    hurdle_expansion_bps: expansion,
                }
            } else {
                JumpAction::PullAsksAndWidenBids {
                    jump_return_bps: latest_return_bps,
                    relative_jump_ratio: rj,
                    hurdle_expansion_bps: expansion,
                }
            }
        } else if rj > self.jump_ratio_trigger && rj > 0.45 {
            let expansion = (rj * 15.0).max(3.0);
            JumpAction::DefensiveSpreadWidening {
                relative_jump_ratio: rj,
                hurdle_expansion_bps: expansion,
            }
        } else {
            JumpAction::NormalDiffusion
        };

        JumpAssessment {
            is_jump: is_tick_jump,
            latest_return_bps,
            realized_variance: rv,
            bipower_variation: bv,
            relative_jump_ratio: rj,
            continuous_variance: cont_var,
            jump_variance: jump_var,
            dynamic_threshold_bps: theta_bps,
            z_jump_stat: z_stat,
            action,
        }
    }

    fn empty_assessment(&self, action: JumpAction) -> JumpAssessment {
        JumpAssessment {
            is_jump: false,
            latest_return_bps: 0.0,
            realized_variance: 0.0,
            bipower_variation: 0.0,
            relative_jump_ratio: 0.0,
            continuous_variance: 0.0,
            jump_variance: 0.0,
            dynamic_threshold_bps: 0.0,
            z_jump_stat: 0.0,
            action,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bipower_variation_continuous_vs_jump() {
        let mut detector = ManciniBipowerJumpDetector::new(50, 3.0, 0.49, 0.30);

        // 1. Simulate smooth continuous random walk with small increments (Gaussian diffusion)
        let mut px = 100.0;
        for i in 0..40 {
            let step = if i % 2 == 0 { 0.05 } else { -0.04 };
            px += step;
            detector.ingest_tick(px, 1.0);
        }

        let normal_report = detector.ingest_tick(px + 0.02, 1.0);
        assert!(
            !normal_report.is_jump,
            "small diffusion step should not trigger jump"
        );
        assert!(normal_report.bipower_variation > 0.0);
        assert_eq!(normal_report.action, JumpAction::NormalDiffusion);

        // 2. Sudden massive downward flash crash jump: price drops by 5% in a single tick
        let crash_px = px * 0.95;
        let crash_report = detector.ingest_tick(crash_px, 1.0);

        assert!(crash_report.is_jump, "5% crash should trigger jump");
        assert!(
            crash_report.relative_jump_ratio > 0.30,
            "jump ratio should be high"
        );
        assert!(crash_report.latest_return_bps < -400.0);

        match crash_report.action {
            JumpAction::PullBidsAndWidenAsks {
                jump_return_bps,
                hurdle_expansion_bps,
                ..
            } => {
                assert!(jump_return_bps < 0.0);
                assert!(hurdle_expansion_bps > 10.0);
            }
            _ => panic!("Expected PullBidsAndWidenAsks action for downward crash"),
        }
    }

    #[test]
    fn test_upward_squeeze_jump_action() {
        let mut detector = ManciniBipowerJumpDetector::new(40, 3.0, 0.49, 0.30);

        let mut px = 50000.0;
        for _ in 0..30 {
            px += 5.0;
            detector.ingest_tick(px, 1.0);
        }

        // Sudden upward pump / short squeeze (+4% in one tick)
        let pump_px = px * 1.04;
        let pump_report = detector.ingest_tick(pump_px, 1.0);

        assert!(pump_report.is_jump);
        assert!(pump_report.latest_return_bps > 350.0);

        match pump_report.action {
            JumpAction::PullAsksAndWidenBids {
                jump_return_bps,
                hurdle_expansion_bps,
                ..
            } => {
                assert!(jump_return_bps > 0.0);
                assert!(hurdle_expansion_bps > 10.0);
            }
            _ => panic!("Expected PullAsksAndWidenBids action for short squeeze"),
        }
    }
}
