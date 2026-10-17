//! Deep Order Book Microstructure & Queue Priority Estimation.
//!
//! Formulates:
//! 1. Continuous-time queue depletion hazard rate:
//!    $$\lambda_{\text{queue}}(t) = \frac{\lambda_{\text{trade}} \bar{v} + \mu_{\text{cancel}} \bar{v} (V_{\text{ahead}} / Q)}{V_{\text{ahead}}}$$
//! 2. Closed-form fill probability surface:
//!    $$P_{\text{fill}}(t, k, d, s) = F_{\text{fp}}(t \mid k) \exp\left(-\lambda_{\text{fade}} \frac{k}{\mu_Q(s, d)}\right)$$
//! 3. Pick-Off Hazard Index:
//!    $$\Pi_{\text{pickoff}}^*(k) = \frac{\lambda_{\text{sweep}} \cdot (1 - S_{\text{queue}})}{\lambda_{\text{replenish}} \cdot S_{\text{queue}} + \epsilon} \cdot \left(1 + \frac{\max(0, \Delta P_{\text{adverse}})}{\text{Tick}}\right)$$
//!    triggering proactive quote withdrawal when $\Pi_{\text{pickoff}}^*(k) \ge 0.70$.

use crate::black_scholes::standard_normal_cdf;
use serde::{Deserialize, Serialize};

/// Configuration parameters for Queue Priority Hazard Modeling.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct QueueHazardConfig {
    /// Intensity of trade arrivals hitting the queue level ($\lambda_{\text{trade}}$ events/sec).
    pub trade_arrival_rate: f64,
    /// Cancellation intensity at the queue level ($\mu_{\text{cancel}}$ events/sec).
    pub cancellation_rate: f64,
    /// Average size per trade/cancel event ($\bar{v}$).
    pub mean_event_volume: f64,
    /// Quote fade intensity ($\lambda_{\text{fade}}$ 1/sec), representing arrival rate of adverse market sweeps.
    pub quote_fade_rate: f64,
    /// Market sweep arrival intensity ($\lambda_{\text{sweep}}$ events/sec).
    pub sweep_arrival_rate: f64,
    /// Replenishment arrival intensity ($\lambda_{\text{replenish}}$ events/sec).
    pub replenish_arrival_rate: f64,
    /// Queue size diffusion volatility ($\sigma_Q$).
    pub queue_diffusion_vol: f64,
    /// Tick size in basis points.
    pub tick_size_bps: f64,
    /// Threshold for Pick-Off Hazard Index triggering quote withdrawal ($\Pi_{\text{pickoff}}^* \ge 0.70$).
    pub pickoff_hazard_threshold: f64,
    /// Seniority threshold below which stagnant orders are evaluated for cancel-replace (e.g. 0.20).
    pub stagnation_seniority_threshold: f64,
}

impl Default for QueueHazardConfig {
    fn default() -> Self {
        Self {
            trade_arrival_rate: 3.5,
            cancellation_rate: 2.0,
            mean_event_volume: 1.0,
            quote_fade_rate: 0.15,
            sweep_arrival_rate: 1.2,
            replenish_arrival_rate: 2.5,
            queue_diffusion_vol: 2.0,
            tick_size_bps: 0.5,
            pickoff_hazard_threshold: 0.70,
            stagnation_seniority_threshold: 0.20,
        }
    }
}

/// Action recommendation returned by the Queue Priority Hazard Engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum QueueHazardRecommendation {
    /// Maintain resting limit order — favorable seniority and low toxic hazard.
    MaintainRestingQuote {
        seniority_ratio: f64,
        fill_probability: f64,
        pickoff_hazard: f64,
    },
    /// Proactively withdraw quote immediately to prevent pick-off by informed sweepers.
    ProactiveQuoteWithdrawal {
        reason: String,
        pickoff_hazard: f64,
        seniority_ratio: f64,
    },
    /// Cancel stagnant quote and replace closer to the new inside touch.
    CancelAndReplace {
        reason: String,
        seniority_ratio: f64,
        recommended_drift_bps: f64,
    },
}

/// Snapshot of queue position and microstructure probabilities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueuePositionSnapshot {
    pub initial_queue_depth: f64,
    pub volume_ahead: f64,
    pub total_level_depth: f64,
    pub seniority_ratio: f64,
    pub depletion_hazard_rate: f64,
    pub fill_probability_5s: f64,
    pub fill_probability_30s: f64,
    pub expected_time_to_fill_sec: f64,
    pub pickoff_hazard_index: f64,
}

/// Continuous-time Queue Priority and Microstructure Hazard Engine.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct QueuePriorityHazardEngine {
    pub config: QueueHazardConfig,
}

impl QueuePriorityHazardEngine {
    pub fn new(config: QueueHazardConfig) -> Self {
        Self { config }
    }

    /// Computes continuous-time queue depletion hazard rate:
    /// $$\lambda_{\text{queue}}(t) = \frac{\lambda_{\text{trade}} \bar{v} + \mu_{\text{cancel}} \bar{v} (V_{\text{ahead}} / Q)}{V_{\text{ahead}}}$$
    pub fn compute_queue_depletion_hazard(&self, volume_ahead: f64, total_depth: f64) -> f64 {
        if volume_ahead <= 1e-6 {
            return 100.0; // Top of queue: instantaneous depletion hazard
        }
        let total = total_depth.max(volume_ahead);
        let cancel_ahead_prob = (volume_ahead / total).clamp(0.0, 1.0);
        let effective_rate = self.config.trade_arrival_rate * self.config.mean_event_volume
            + self.config.cancellation_rate * self.config.mean_event_volume * cancel_ahead_prob;

        (effective_rate / volume_ahead).max(0.0)
    }

    /// Computes queue seniority fraction $S_{\text{queue}} \in [0.0, 1.0]$:
    /// $$S_{\text{queue}} = 1 - \frac{V_{\text{ahead}}}{Q}$$
    pub fn compute_seniority_ratio(volume_ahead: f64, total_depth: f64) -> f64 {
        if total_depth <= 1e-6 {
            return 1.0;
        }
        (1.0 - (volume_ahead / total_depth)).clamp(0.0, 1.0)
    }

    /// Evaluates the closed-form fill probability surface:
    /// $$P_{\text{fill}}(t, k, d, s) = F_{\text{fp}}(t \mid k) \exp\left(-\lambda_{\text{fade}} \frac{k}{\mu_Q(s, d)}\right)$$
    /// where $F_{\text{fp}}(t \mid k)$ is the Cont-de Larrard first-passage CDF:
    /// $$F_{\text{fp}}(t \mid k) = \Phi\left(\frac{\mu_Q t - k}{\sigma_Q \sqrt{t}}\right) - \exp\left(\frac{2 \mu_Q k}{\sigma_Q^2}\right) \Phi\left(\frac{-\mu_Q t - k}{\sigma_Q \sqrt{t}}\right)$$
    pub fn compute_fill_probability_surface(
        &self,
        horizon_sec: f64,
        volume_ahead: f64,
        _total_depth: f64,
    ) -> f64 {
        if volume_ahead <= 0.0 {
            return 1.0;
        }
        if horizon_sec <= 1e-6 {
            return 0.0;
        }

        let mu_q = (self.config.trade_arrival_rate + self.config.cancellation_rate * 0.5)
            * self.config.mean_event_volume;
        let sigma_q = self.config.queue_diffusion_vol.max(0.1);
        let t = horizon_sec;
        let k = volume_ahead;

        let sigma_sqrt_t = sigma_q * t.sqrt();
        let term1 = (mu_q * t - k) / sigma_sqrt_t;
        let term2 = (-mu_q * t - k) / sigma_sqrt_t;

        let phi1 = standard_normal_cdf(term1);
        let phi2 = standard_normal_cdf(term2);

        let exp_pow = (2.0 * mu_q * k) / (sigma_q * sigma_q);
        let exp_factor = if exp_pow > 50.0 { 0.0 } else { exp_pow.exp() };

        let raw_fp = (phi1 - exp_factor * phi2).clamp(0.0, 1.0);

        // Quote fade discount: exp(-lambda_fade * k / mu_Q)
        let fade_exponent = -self.config.quote_fade_rate * (k / mu_q.max(0.01));
        let fade_factor = fade_exponent.exp().clamp(0.0, 1.0);

        (raw_fp * fade_factor).clamp(0.0, 1.0)
    }

    /// Evaluates the Pick-Off Hazard Index:
    /// $$\Pi_{\text{pickoff}}^*(k) = \frac{\lambda_{\text{sweep}} \cdot (1 - S_{\text{queue}})}{\lambda_{\text{replenish}} \cdot S_{\text{queue}} + \epsilon} \cdot \left(\frac{\max(0, \Delta P_{\text{adverse}})}{\text{Tick}}\right)$$
    /// When adverse price drift is 0, pickoff hazard is 0; as adverse drift grows against low seniority,
    /// the index escalates sharply.
    pub fn compute_pickoff_hazard_index(
        &self,
        volume_ahead: f64,
        total_depth: f64,
        adverse_price_drift_bps: f64,
    ) -> f64 {
        let seniority = Self::compute_seniority_ratio(volume_ahead, total_depth);
        let eps = 1e-4;

        let sweep_to_replenish = (self.config.sweep_arrival_rate * (1.0 - seniority + 0.1))
            / (self.config.replenish_arrival_rate * (seniority + 0.1) + eps);

        let tick = self.config.tick_size_bps.max(0.1);
        let adverse_drift = adverse_price_drift_bps.max(0.0);
        let drift_multiplier = adverse_drift / tick;

        let raw_hazard = sweep_to_replenish * drift_multiplier;
        raw_hazard.clamp(0.0, 5.0)
    }

    /// Comprehensive evaluation of resting quote priority returning actionable verdict.
    pub fn evaluate_resting_quote(
        &self,
        volume_ahead: f64,
        total_depth: f64,
        dwell_time_sec: f64,
        adverse_price_drift_bps: f64,
    ) -> (QueueHazardRecommendation, QueuePositionSnapshot) {
        let seniority = Self::compute_seniority_ratio(volume_ahead, total_depth);
        let dep_hazard = self.compute_queue_depletion_hazard(volume_ahead, total_depth);
        let p_fill_5s = self.compute_fill_probability_surface(5.0, volume_ahead, total_depth);
        let p_fill_30s = self.compute_fill_probability_surface(30.0, volume_ahead, total_depth);

        let mu_q = (self.config.trade_arrival_rate + self.config.cancellation_rate * 0.5)
            * self.config.mean_event_volume;
        let exp_time = if volume_ahead <= 0.0 {
            0.0
        } else {
            volume_ahead / mu_q.max(0.01)
        };

        let pickoff_hazard =
            self.compute_pickoff_hazard_index(volume_ahead, total_depth, adverse_price_drift_bps);

        let snapshot = QueuePositionSnapshot {
            initial_queue_depth: total_depth,
            volume_ahead,
            total_level_depth: total_depth,
            seniority_ratio: seniority,
            depletion_hazard_rate: dep_hazard,
            fill_probability_5s: p_fill_5s,
            fill_probability_30s: p_fill_30s,
            expected_time_to_fill_sec: exp_time,
            pickoff_hazard_index: pickoff_hazard,
        };

        // Decision rule 1: Pick-Off Hazard Index breach
        if pickoff_hazard >= self.config.pickoff_hazard_threshold {
            let rec = QueueHazardRecommendation::ProactiveQuoteWithdrawal {
                reason: format!(
                    "Pick-off hazard index breached: {:.2} >= {:.2} (adverse drift: {:.2} bps)",
                    pickoff_hazard, self.config.pickoff_hazard_threshold, adverse_price_drift_bps
                ),
                pickoff_hazard,
                seniority_ratio: seniority,
            };
            return (rec, snapshot);
        }

        // Decision rule 2: Queue Stagnation & Stale Touch
        if seniority < self.config.stagnation_seniority_threshold
            && dwell_time_sec > (2.0 * exp_time).max(10.0)
        {
            let rec = QueueHazardRecommendation::CancelAndReplace {
                reason: format!(
                    "Queue priority stagnation: seniority {:.2} < {:.2} after {:.1}s dwell",
                    seniority, self.config.stagnation_seniority_threshold, dwell_time_sec
                ),
                seniority_ratio: seniority,
                recommended_drift_bps: self.config.tick_size_bps,
            };
            return (rec, snapshot);
        }

        // Default: Maintain resting order
        (
            QueueHazardRecommendation::MaintainRestingQuote {
                seniority_ratio: seniority,
                fill_probability: p_fill_5s,
                pickoff_hazard,
            },
            snapshot,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_priority_top_of_queue_instant_seniority() {
        let engine = QueuePriorityHazardEngine::default();
        let seniority = QueuePriorityHazardEngine::compute_seniority_ratio(0.0, 100.0);
        assert_eq!(seniority, 1.0);

        let p_fill = engine.compute_fill_probability_surface(5.0, 0.0, 100.0);
        assert_eq!(p_fill, 1.0);

        let (rec, snap) = engine.evaluate_resting_quote(0.0, 100.0, 2.0, 0.0);
        assert!(matches!(
            rec,
            QueueHazardRecommendation::MaintainRestingQuote { .. }
        ));
        assert_eq!(snap.seniority_ratio, 1.0);
    }

    #[test]
    fn test_pickoff_hazard_withdrawal_on_adverse_jump() {
        let engine = QueuePriorityHazardEngine::default();
        // Resting order with low seniority and severe adverse drift of 3.0 bps (6 ticks)
        let hazard = engine.compute_pickoff_hazard_index(80.0, 100.0, 3.0);
        assert!(
            hazard >= 0.70,
            "Adverse drift on low-seniority quote must trigger elevated pickoff hazard: {hazard}"
        );

        let (rec, snap) = engine.evaluate_resting_quote(80.0, 100.0, 5.0, 3.0);
        assert!(matches!(
            rec,
            QueueHazardRecommendation::ProactiveQuoteWithdrawal { .. }
        ));
        assert!(snap.pickoff_hazard_index >= 0.70);
    }

    #[test]
    fn test_stagnation_cancel_replace_trigger() {
        let engine = QueuePriorityHazardEngine::default();
        // Buried order with seniority 0.10, dwell time 45s (well beyond 2 * exp_time) and 0 adverse drift
        let (rec, snap) = engine.evaluate_resting_quote(90.0, 100.0, 45.0, 0.0);
        assert!(matches!(
            rec,
            QueueHazardRecommendation::CancelAndReplace { .. }
        ));
        assert!((snap.seniority_ratio - 0.10).abs() < 1e-6);
    }

    #[test]
    fn test_fill_probability_surface_decay_with_fade() {
        let engine = QueuePriorityHazardEngine::default();
        let p_short = engine.compute_fill_probability_surface(2.0, 20.0, 50.0);
        let p_long = engine.compute_fill_probability_surface(20.0, 20.0, 50.0);
        assert!(p_long > p_short);
    }
}
