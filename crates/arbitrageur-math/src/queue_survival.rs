//! Cont-de Larrard FIFO Queue Survival Probability and Cancel-Replace Thresholds.
//!
//! Models the depletion of limit order queues under competing trade matching (market orders)
//! and limit order cancellations, formulating:
//! 1. Cont-de Larrard first-passage fill probability P_{fill}(\tau | q_{pos}, \mu_Q, \sigma_Q)
//! 2. Queue imbalance survival degradation: P_{surv}(\tau | q_{pos}, I_K)
//! 3. Closed-form cubic-root cancel-replace threshold \Delta P_{replace}^*(q_{pos})

use crate::black_scholes::standard_normal_cdf;
use serde::{Deserialize, Serialize};

/// FIFO Queue survival and depletion estimator.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct QueueSurvivalEstimator {
    /// Net drift rate of queue depletion (e.g. trade match rate \lambda_{trades} - quote addition rate \lambda_{adds})
    pub depletion_drift_rate: f64,
    /// Diffusion volatility of queue size process \sigma_Q
    pub queue_volatility: f64,
    /// Adverse imbalance sensitivity exponent \alpha
    pub imbalance_alpha: f64,
}

impl Default for QueueSurvivalEstimator {
    fn default() -> Self {
        Self {
            depletion_drift_rate: 1.5,
            queue_volatility: 2.0,
            imbalance_alpha: 1.2,
        }
    }
}

impl QueueSurvivalEstimator {
    pub fn new(depletion_drift_rate: f64, queue_volatility: f64, imbalance_alpha: f64) -> Self {
        Self {
            depletion_drift_rate: depletion_drift_rate.max(1e-4),
            queue_volatility: queue_volatility.max(1e-4),
            imbalance_alpha: imbalance_alpha.clamp(0.1, 5.0),
        }
    }

    /// Computes the Cont-de Larrard first-passage fill probability P_{fill}(\tau | q_{pos})
    /// of a resting limit order at queue position q_{pos} within time horizon \tau:
    /// P_{fill}(\tau | q_{pos}) = \Phi( \frac{\mu \tau - q_{pos}}{\sigma \sqrt{\tau}} )
    ///                          - \exp( \frac{2 \mu q_{pos}}{\sigma^2} ) * \Phi( \frac{-\mu \tau - q_{pos}}{\sigma \sqrt{\tau}} )
    pub fn fill_probability(&self, horizon_sec: f64, queue_pos: f64) -> f64 {
        if queue_pos <= 0.0 {
            return 1.0;
        }
        if horizon_sec <= 1e-6 {
            return 0.0;
        }

        let tau = horizon_sec;
        let mu = self.depletion_drift_rate;
        let sigma = self.queue_volatility;
        let sigma_sqrt_tau = sigma * tau.sqrt();

        if sigma_sqrt_tau <= 1e-9 {
            return if mu * tau >= queue_pos { 1.0 } else { 0.0 };
        }

        let term1_arg = (mu * tau - queue_pos) / sigma_sqrt_tau;
        let term2_arg = (-mu * tau - queue_pos) / sigma_sqrt_tau;

        let phi1 = standard_normal_cdf(term1_arg);
        let phi2 = standard_normal_cdf(term2_arg);

        // Exponential term: exp( 2 * mu * queue_pos / sigma^2 )
        let exp_exponent = (2.0 * mu * queue_pos) / (sigma * sigma);
        let exp_factor = if exp_exponent > 50.0 {
            0.0
        } else {
            exp_exponent.exp()
        };

        let raw_p = phi1 - exp_factor * phi2;
        raw_p.clamp(0.0, 1.0)
    }

    /// Computes expected time-to-depletion in seconds: \tau_{deplete} = q_{ahead} / \mu
    pub fn expected_depletion_time(&self, queue_pos: f64) -> f64 {
        if queue_pos <= 0.0 {
            0.0
        } else {
            queue_pos / self.depletion_drift_rate
        }
    }

    /// Computes queue survival probability incorporating multi-level order book imbalance:
    /// P_{surv}(\tau | q_{pos}, I_K) = P_{fill}(\tau | q_{pos}) * \exp( -\alpha \frac{q_{pos}}{Q_{total}} (1 + \max(0, -I_K)) )
    /// where I_K is in [-1.0, 1.0]. Negative I_K (adverse pressure on resting side) accelerates cancellation hazards.
    pub fn survival_probability_with_imbalance(
        &self,
        horizon_sec: f64,
        queue_pos: f64,
        total_queue_depth: f64,
        imbalance: f64,
    ) -> f64 {
        let base_p = self.fill_probability(horizon_sec, queue_pos);
        if base_p <= 1e-9 || total_queue_depth <= 1e-6 {
            return base_p;
        }

        let relative_pos = (queue_pos / total_queue_depth).clamp(0.0, 1.0);
        let adverse_pressure = 1.0 + (-imbalance).max(0.0);
        let penalty = (-self.imbalance_alpha * relative_pos * adverse_pressure).exp();

        (base_p * penalty).clamp(0.0, 1.0)
    }

    /// Computes closed-form cubic-root cancel-replace threshold \Delta P_{replace}^*(q_{pos}):
    /// \Delta P_{replace}^* = [ \frac{3 \sigma_P^2 (C_{cancel} + \omega \cdot \frac{q_{pos}}{Q_{total}})}{2 (\lambda_{trades} + \lambda_{cancels})} ]^{1/3} + 0.5 \cdot \text{TickSize}
    ///
    /// When market price shifts away by more than \Delta P_{replace}^*, the order should be canceled
    /// and replaced closer to the touch. Deep queue positions have higher seniority cost, lowering the hurdle to replace.
    pub fn cancel_replace_price_threshold(
        price_volatility_bps: f64,
        cancellation_cost_bps: f64,
        seniority_weight: f64,
        queue_pos: f64,
        total_queue_depth: f64,
        event_arrival_rate: f64,
        tick_size_bps: f64,
    ) -> f64 {
        let rel_pos = if total_queue_depth > 1e-6 {
            (queue_pos / total_queue_depth).clamp(0.0, 1.0)
        } else {
            1.0
        };

        let effective_cost = cancellation_cost_bps.max(0.1) + seniority_weight.max(0.0) * rel_pos;
        let sigma_sq = price_volatility_bps.max(0.1).powi(2);
        let lambda = event_arrival_rate.max(0.1);

        let radicand = (3.0 * sigma_sq * effective_cost) / (2.0 * lambda);
        radicand.cbrt() + 0.5 * tick_size_bps.max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_survival_front_of_queue_instant_fill() {
        let estimator = QueueSurvivalEstimator::default();
        let p = estimator.fill_probability(1.0, 0.0);
        assert_eq!(p, 1.0);
    }

    #[test]
    fn test_queue_survival_monotonic_in_time_and_depth() {
        let estimator = QueueSurvivalEstimator::new(2.0, 1.5, 1.0);
        let p_short = estimator.fill_probability(0.5, 5.0);
        let p_long = estimator.fill_probability(5.0, 5.0);
        assert!(
            p_long > p_short,
            "Longer horizon should have higher fill probability"
        );

        let p_deep = estimator.fill_probability(5.0, 15.0);
        assert!(
            p_long > p_deep,
            "Shallower queue position should have higher fill probability"
        );
    }

    #[test]
    fn test_survival_with_adverse_imbalance() {
        let estimator = QueueSurvivalEstimator::new(2.0, 1.5, 1.2);
        // Neutral imbalance (0.0) vs adverse imbalance (-0.8)
        let p_neutral = estimator.survival_probability_with_imbalance(2.0, 3.0, 10.0, 0.0);
        let p_adverse = estimator.survival_probability_with_imbalance(2.0, 3.0, 10.0, -0.8);

        assert!(
            p_adverse < p_neutral,
            "Adverse imbalance should penalize queue survival"
        );
    }

    #[test]
    fn test_cancel_replace_price_threshold_cubic_root() {
        let thresh_front = QueueSurvivalEstimator::cancel_replace_price_threshold(
            15.0, // sigma = 15 bps
            2.0,  // C_cancel = 2 bps
            5.0,  // omega = 5 bps
            1.0,  // queue_pos = 1 (front)
            100.0, 10.0, // lambda = 10 events/sec
            0.5,  // tick = 0.5 bps
        );

        let thresh_back = QueueSurvivalEstimator::cancel_replace_price_threshold(
            15.0, 2.0, 5.0, 95.0, // queue_pos = 95 (back)
            100.0, 10.0, 0.5,
        );

        assert!(thresh_back > thresh_front);
        assert!(thresh_front > 0.5);
    }
}
