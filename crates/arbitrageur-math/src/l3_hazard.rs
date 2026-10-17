//! Level-3 (L3) Market By Order (MBO) Liquidity Replenishment & Queue Drain Hazard Forecaster.
//!
//! Provides:
//! - Online tracking of order-by-order point process intensities: aggressive sweep matching (lambda_sweep),
//!   resting order cancellations (mu_cancel), and passive liquidity replenishment (lambda_add).
//! - Continuous queue survival function S(t | V_ahead) and instantaneous level depletion velocity mu_drain(t).
//! - Closed-form calculation of Pick-Off Hazard Index Pi_pickoff = P(tau_drain <= tau_cancel + delta).
//! - Real-time evacuation alerts (EvacuatePickoff) to withdraw resting maker quotes before toxic sweeps hit.

use arbitrageur_core::types::Decimal;
use serde::{Deserialize, Serialize};

/// Error function approximation (Abramowitz and Stegun 7.1.26).
fn erf_approx(x: f64) -> f64 {
    let a1 = 0.254829592;
    let a2 = -0.284496736;
    let a3 = 1.421413741;
    let a4 = -1.453152027;
    let a5 = 1.061405429;
    let p = 0.3275911;

    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let abs_x = x.abs();

    let t = 1.0 / (1.0 + p * abs_x);
    let y = 1.0 - (((((a5 * t + a4) * t) + a3) * t + a2) * t + a1) * t * (-abs_x * abs_x).exp();

    sign * y
}

/// Standard normal CDF Phi(z).
fn normal_cdf(z: f64) -> f64 {
    0.5 * (1.0 + erf_approx(z / std::f64::consts::SQRT_2))
}

/// Decision output from the Queue Drain Hazard Forecaster.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum QueueHazardDecision {
    /// Queue is stable; maintain seniority at the resting price level.
    SafeHold {
        volume_ahead: Decimal,
        tau_drain_sec: f64,
        hazard_index: f64,
    },
    /// Queue is too deep or stagnant; consider cancelling and re-pegging closer to touch.
    RepegPassive {
        reason: String,
        volume_ahead: Decimal,
    },
    /// Imminent drain or toxic sweep detected; withdraw resting order immediately to prevent pick-off.
    EvacuatePickoff {
        reason: String,
        tau_drain_sec: f64,
        hazard_index: f64,
    },
}

/// Detailed queue telemetry and hazard metrics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueDrainHazardMetrics {
    pub volume_ahead: Decimal,
    pub total_level_volume: Decimal,
    pub drain_velocity: f64,
    pub tau_drain_sec: f64,
    pub hazard_index: f64,
    pub survival_prob: f64,
}

/// Configuration parameters for the Queue Drain Hazard Forecaster.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct QueueDrainHazardConfig {
    /// Expected round-trip cancellation latency to the exchange matching engine (seconds, e.g. 0.035 = 35ms).
    pub cancel_latency_sec: f64,
    /// Critical pick-off hazard index threshold (0.0 to 1.0) triggering evacuation (e.g. 0.70).
    pub critical_hazard_threshold: f64,
    /// Extra safety margin added to cancel latency (seconds, e.g. 0.050 = 50ms).
    pub safety_margin_sec: f64,
    /// Maximum allowable volume ahead ratio before recommending passive re-pegging (e.g. 0.85).
    pub max_volume_ahead_ratio: f64,
    /// Ratio of drain velocity to replenishment speed triggering evacuation alert (e.g. 3.5).
    pub drain_to_replenish_ratio: f64,
    /// Minimum buffer volume ahead in base units before critical hazard logic activates.
    pub min_buffer_volume: f64,
}

impl Default for QueueDrainHazardConfig {
    fn default() -> Self {
        Self {
            cancel_latency_sec: 0.035, // 35ms
            critical_hazard_threshold: 0.70,
            safety_margin_sec: 0.050, // 50ms
            max_volume_ahead_ratio: 0.85,
            drain_to_replenish_ratio: 3.5,
            min_buffer_volume: 0.05,
        }
    }
}

/// High-Frequency Level-3 Market By Order (MBO) Liquidity Replenishment & Queue Drain Hazard Forecaster.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct QueueDrainHazardForecaster {
    pub config: QueueDrainHazardConfig,
    /// Estimated aggressive trade sweep intensity (trades / sec).
    pub trade_intensity_lambda: f64,
    /// Estimated cancellation intensity of resting limit orders (cancels / sec).
    pub cancel_intensity_mu: f64,
    /// Estimated replenishment arrival rate of new limit orders (adds / sec).
    pub replenish_rate_lambda_add: f64,
    /// Rolling mean size of executed aggressive sweeps.
    pub mean_trade_size: f64,
    /// Rolling mean size of cancelled resting limit orders.
    pub mean_cancel_size: f64,
    /// Rolling mean size of added limit orders.
    pub mean_add_size: f64,
    /// Diffusion volatility of the queue depletion process.
    pub diffusion_sigma: f64,
}

impl Default for QueueDrainHazardForecaster {
    fn default() -> Self {
        Self::new(QueueDrainHazardConfig::default())
    }
}

impl QueueDrainHazardForecaster {
    pub fn new(config: QueueDrainHazardConfig) -> Self {
        Self {
            config,
            trade_intensity_lambda: 1.0,
            cancel_intensity_mu: 2.0,
            replenish_rate_lambda_add: 2.5,
            mean_trade_size: 0.5,
            mean_cancel_size: 0.5,
            mean_add_size: 0.5,
            diffusion_sigma: 1.0,
        }
    }

    /// Records an executed aggressive trade sweep to update intensity.
    pub fn record_trade(&mut self, size: f64, dt_sec: f64) {
        let alpha = (dt_sec / 5.0).clamp(0.01, 0.50);
        let inst_rate = if dt_sec > 1e-4 { 1.0 / dt_sec } else { 10.0 };
        self.trade_intensity_lambda =
            (1.0 - alpha) * self.trade_intensity_lambda + alpha * inst_rate;
        self.mean_trade_size = (1.0 - alpha) * self.mean_trade_size + alpha * size.max(0.001);
    }

    /// Records a limit order cancellation to update intensity.
    pub fn record_cancel(&mut self, size: f64, dt_sec: f64) {
        let alpha = (dt_sec / 5.0).clamp(0.01, 0.50);
        let inst_rate = if dt_sec > 1e-4 { 1.0 / dt_sec } else { 10.0 };
        self.cancel_intensity_mu = (1.0 - alpha) * self.cancel_intensity_mu + alpha * inst_rate;
        self.mean_cancel_size = (1.0 - alpha) * self.mean_cancel_size + alpha * size.max(0.001);
    }

    /// Records a new limit order replenishment add to update intensity.
    pub fn record_add(&mut self, size: f64, dt_sec: f64) {
        let alpha = (dt_sec / 5.0).clamp(0.01, 0.50);
        let inst_rate = if dt_sec > 1e-4 { 1.0 / dt_sec } else { 10.0 };
        self.replenish_rate_lambda_add =
            (1.0 - alpha) * self.replenish_rate_lambda_add + alpha * inst_rate;
        self.mean_add_size = (1.0 - alpha) * self.mean_add_size + alpha * size.max(0.001);
    }

    /// Computes instantaneous depletion velocity mu_drain = lambda_m * v_m + mu_c * v_c * (V_ahead / V_total).
    pub fn compute_drain_velocity(&self, volume_ahead_f64: f64, total_level_f64: f64) -> f64 {
        let sweep_vel = self.trade_intensity_lambda * self.mean_trade_size;
        let cancel_ratio = if total_level_f64 > 1e-6 {
            (volume_ahead_f64 / total_level_f64).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let cancel_vel = self.cancel_intensity_mu * self.mean_cancel_size * cancel_ratio;
        (sweep_vel + cancel_vel).max(1e-4)
    }

    /// Evaluates real-time queue hazard for a resting maker order.
    pub fn evaluate_hazard(
        &self,
        volume_ahead: Decimal,
        total_level_volume: Decimal,
        is_toxic_flow: bool,
    ) -> (QueueHazardDecision, QueueDrainHazardMetrics) {
        let v_ahead = volume_ahead.to_string().parse::<f64>().unwrap_or(0.0);
        let v_total = total_level_volume.to_string().parse::<f64>().unwrap_or(1.0);

        let mu_drain = self.compute_drain_velocity(v_ahead, v_total);
        let tau_drain = if mu_drain > 1e-6 {
            v_ahead / mu_drain
        } else {
            3600.0
        };

        let tau_cancel = self.config.cancel_latency_sec;
        let tau_crit = tau_cancel + self.config.safety_margin_sec;

        // Diffusion standard deviation over cancellation horizon: sigma * sqrt(tau_cancel)
        let sigma = self.diffusion_sigma.max(0.05);
        let sigma_tau = sigma * tau_cancel.max(1e-4).sqrt();

        // Pick-off hazard index: Pi_pickoff = 1 - Phi((V_ahead - mu_drain * tau_cancel) / (sigma * sqrt(tau)))
        let z_pickoff = (v_ahead - mu_drain * tau_cancel) / sigma_tau;
        let hazard_index = (1.0 - normal_cdf(z_pickoff)).clamp(0.0, 1.0);

        // Queue survival probability: S(t) = exp(-mu_drain / V_ahead * tau_cancel)
        let survival_prob = if v_ahead > 1e-4 {
            (-mu_drain * tau_cancel / v_ahead).exp().clamp(0.0, 1.0)
        } else {
            0.0
        };

        let metrics = QueueDrainHazardMetrics {
            volume_ahead,
            total_level_volume,
            drain_velocity: mu_drain,
            tau_drain_sec: tau_drain,
            hazard_index,
            survival_prob,
        };

        // Decision Policy:
        // 1. If toxic flow is detected and volume ahead is low: Evacuate immediately
        if is_toxic_flow && v_ahead <= self.config.min_buffer_volume {
            return (
                QueueHazardDecision::EvacuatePickoff {
                    reason: "TOXIC_SWEEP_IMMINENT_LOW_BUFFER".to_string(),
                    tau_drain_sec: tau_drain,
                    hazard_index: 1.0,
                },
                metrics,
            );
        }

        // 2. Imminent pick-off hazard: tau_drain <= tau_crit or hazard_index >= threshold
        if (tau_drain <= tau_crit && v_ahead <= self.config.min_buffer_volume * 2.0)
            || hazard_index >= self.config.critical_hazard_threshold
        {
            return (
                QueueHazardDecision::EvacuatePickoff {
                    reason: format!(
                        "PICKOFF_HAZARD_SPIKE: Pi={:.2} >= {:.2} or tau_drain={:.3}s <= {:.3}s",
                        hazard_index, self.config.critical_hazard_threshold, tau_drain, tau_crit
                    ),
                    tau_drain_sec: tau_drain,
                    hazard_index,
                },
                metrics,
            );
        }

        // 3. Drain velocity outstrips replenishment speed by hurdle
        let replenish_speed = (self.replenish_rate_lambda_add * self.mean_add_size).max(1e-4);
        if mu_drain >= self.config.drain_to_replenish_ratio * replenish_speed
            && v_ahead <= self.config.min_buffer_volume * 1.5
        {
            return (
                QueueHazardDecision::EvacuatePickoff {
                    reason: format!(
                        "DRAIN_EXCEEDS_REPLENISHMENT: mu_drain={:.2} >= {:.1}x replenish={:.2}",
                        mu_drain, self.config.drain_to_replenish_ratio, replenish_speed
                    ),
                    tau_drain_sec: tau_drain,
                    hazard_index,
                },
                metrics,
            );
        }

        // 4. Buried too deep in an oversized queue: Repeg passive
        if v_total > 1.0 && (v_ahead / v_total) >= self.config.max_volume_ahead_ratio {
            return (
                QueueHazardDecision::RepegPassive {
                    reason: format!(
                        "BURIED_IN_QUEUE_DEPTH: share={:.1}% >= {:.1}%",
                        (v_ahead / v_total) * 100.0,
                        self.config.max_volume_ahead_ratio * 100.0
                    ),
                    volume_ahead,
                },
                metrics,
            );
        }

        // 5. Normal safe queue position
        (
            QueueHazardDecision::SafeHold {
                volume_ahead,
                tau_drain_sec: tau_drain,
                hazard_index,
            },
            metrics,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrageur_core::types::dec;

    #[test]
    fn test_queue_drain_hazard_safe_hold() {
        let forecaster = QueueDrainHazardForecaster::default();
        // Generous volume ahead (10.0), total level volume (20.0), non-toxic
        let vol_ahead = dec!(10.0);
        let vol_total = dec!(20.0);

        let (decision, metrics) = forecaster.evaluate_hazard(vol_ahead, vol_total, false);
        assert!(
            metrics.hazard_index < 0.20,
            "Hazard index should be low for deep buffer: {}",
            metrics.hazard_index
        );
        assert!(
            metrics.tau_drain_sec > 1.0,
            "Drain time should be > 1s: {}",
            metrics.tau_drain_sec
        );
        assert!(
            metrics.survival_prob > 0.90,
            "Survival prob high: {}",
            metrics.survival_prob
        );
        match decision {
            QueueHazardDecision::SafeHold { .. } => {}
            other => panic!("Expected SafeHold, got {:?}", other),
        }
    }

    #[test]
    fn test_queue_drain_hazard_evacuate_on_thinning_volume() {
        let forecaster = QueueDrainHazardForecaster {
            trade_intensity_lambda: 50.0, // Rapid aggressive sweeps
            mean_trade_size: 1.0,         // 50 base units / sec
            diffusion_sigma: 0.5,
            ..Default::default()
        };

        // Thin volume ahead: 0.02 base units (< min_buffer_volume 0.05)
        let vol_ahead = dec!(0.02);
        let vol_total = dec!(0.10);

        let (decision, metrics) = forecaster.evaluate_hazard(vol_ahead, vol_total, false);
        assert!(
            metrics.hazard_index >= 0.70,
            "Hazard index spikes: {}",
            metrics.hazard_index
        );
        assert!(
            metrics.tau_drain_sec < 0.01,
            "Expected drain time sub-10ms: {}",
            metrics.tau_drain_sec
        );

        match decision {
            QueueHazardDecision::EvacuatePickoff { reason, .. } => {
                assert!(
                    reason.contains("PICKOFF_HAZARD_SPIKE")
                        || reason.contains("DRAIN_EXCEEDS_REPLENISHMENT")
                );
            }
            other => panic!("Expected EvacuatePickoff, got {:?}", other),
        }
    }

    #[test]
    fn test_queue_drain_hazard_evacuate_on_toxic_flow() {
        let forecaster = QueueDrainHazardForecaster::default();
        let vol_ahead = dec!(0.03); // <= min_buffer_volume (0.05)
        let vol_total = dec!(0.50);

        let (decision, _) = forecaster.evaluate_hazard(vol_ahead, vol_total, true);
        match decision {
            QueueHazardDecision::EvacuatePickoff { reason, .. } => {
                assert_eq!(reason, "TOXIC_SWEEP_IMMINENT_LOW_BUFFER");
            }
            other => panic!("Expected EvacuatePickoff on toxic flow, got {:?}", other),
        }
    }

    #[test]
    fn test_queue_drain_hazard_repeg_when_buried() {
        let forecaster = QueueDrainHazardForecaster::default();
        // Order is at 95% of queue depth (buried)
        let vol_ahead = dec!(95.0);
        let vol_total = dec!(100.0);

        let (decision, _) = forecaster.evaluate_hazard(vol_ahead, vol_total, false);
        match decision {
            QueueHazardDecision::RepegPassive { reason, .. } => {
                assert!(reason.contains("BURIED_IN_QUEUE_DEPTH"));
            }
            other => panic!("Expected RepegPassive, got {:?}", other),
        }
    }

    #[test]
    fn test_intensity_updates() {
        let mut forecaster = QueueDrainHazardForecaster::default();
        let initial_trade_rate = forecaster.trade_intensity_lambda;
        let initial_trade_size = forecaster.mean_trade_size;

        // Feed aggressive burst of trades: 50 trades arriving with dt=0.01s, size=2.5
        for _ in 0..50 {
            forecaster.record_trade(2.5, 0.01);
        }

        assert!(
            forecaster.trade_intensity_lambda > initial_trade_rate,
            "Trade intensity accelerated"
        );
        assert!(
            forecaster.mean_trade_size > initial_trade_size,
            "Mean trade size increased"
        );
    }
}
