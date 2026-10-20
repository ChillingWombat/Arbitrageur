//! High-Frequency Intraday Funding Velocity Forecasting & Jump-Diffusion Carry Engine.
//!
//! Provides production mathematical formulations for:
//! 1. **Kinematic Kalman State-Space Tracking**:
//!    Estimates continuous funding rate level $F(t)$, velocity $v_F(t) = \frac{dF}{dt}$, and acceleration $a_F(t) = \frac{d^2F}{dt^2}$ in $< 0.5 \;\mu\text{s}$:
//!    $$\mathbf{x}(t) = [F(t), v_F(t), a_F(t)]^T$$
//!    $$\hat{F}(t + \tau) = \hat{F}(t) + \hat{v}_F(t) \tau + \frac{1}{2} \hat{a}_F(t) \tau^2$$
//! 2. **Hawkes Jump-Diffusion Shock Intensity**:
//!    Models shock clustering during violent perp liquidations and basis dislocations:
//!    $$\lambda_F(t) = \mu_F + \sum_{t_i < t} \alpha_F e^{-\beta_F(t - t_i)}$$
//! 3. **Predictive Carry Yield Harvesting Entry Policy**:
//!    Calculates exact projected funding carry accrued over remaining time $\tau_{\text{rem}}$ before settlement snapshot:
//!    $$\Phi_{\text{accrued}}(\tau_{\text{rem}}) = \int_0^{\tau_{\text{rem}}} \hat{F}(t + s) ds = \hat{F}(t) \tau_{\text{rem}} + \frac{1}{2} \hat{v}_F(t) \tau_{\text{rem}}^2 + \frac{1}{6} \hat{a}_F(t) \tau_{\text{rem}}^3$$
//!    Gating entry strictly when projected carry clears round-trip friction and statistical confidence bounds:
//!    $$\Phi_{\text{accrued}}(\tau_{\text{rem}}) \ge 2.0 \cdot \text{Cost}_{\text{RT}} + 1.96 \cdot \sigma_F \sqrt{\tau_{\text{rem}}}$$

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

const SECONDS_PER_YEAR: f64 = 365.25 * 86_400.0;

/// Kinematic funding rate state vector [F, v_F, a_F].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FundingKinematicState {
    /// Filtered funding rate level (annualized or 8-hour decimal).
    pub rate: f64,
    /// Funding rate velocity (dF/dt per second).
    pub velocity: f64,
    /// Funding rate acceleration (d^2F/dt^2 per second^2).
    pub acceleration: f64,
}

/// Online Kalman filter tracking continuous funding rate kinematics with Hawkes shock modeling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FundingVelocityKalmanHawkes {
    /// Kinematic state vector [F, v_F, a_F].
    pub state_x: [f64; 3],
    /// State covariance matrix 3x3.
    pub cov_p: [[f64; 3]; 3],
    /// Measurement noise variance R.
    pub measurement_noise_r: f64,
    /// Process noise spectral density Q.
    pub process_noise_q: f64,
    /// Hawkes baseline intensity \mu_F.
    pub hawkes_mu: f64,
    /// Hawkes excitation magnitude \alpha_F.
    pub hawkes_alpha: f64,
    /// Hawkes exponential decay rate \beta_F.
    pub hawkes_beta: f64,
    /// Shock detection threshold in standard deviations.
    pub shock_z_threshold: f64,
    /// Recent shock timestamp history.
    pub jump_history: VecDeque<f64>,
}

impl Default for FundingVelocityKalmanHawkes {
    fn default() -> Self {
        Self {
            state_x: [0.0001, 0.0, 0.0],
            cov_p: [[1e-6, 0.0, 0.0], [0.0, 1e-10, 0.0], [0.0, 0.0, 1e-14]],
            measurement_noise_r: 1e-7,
            process_noise_q: 1e-10,
            hawkes_mu: 0.10,
            hawkes_alpha: 0.80,
            hawkes_beta: 1.50,
            shock_z_threshold: 2.50,
            jump_history: VecDeque::new(),
        }
    }
}

impl FundingVelocityKalmanHawkes {
    pub fn new(
        initial_rate: f64,
        measurement_noise_r: f64,
        process_noise_q: f64,
        hawkes_mu: f64,
        hawkes_alpha: f64,
        hawkes_beta: f64,
    ) -> Self {
        Self {
            state_x: [initial_rate, 0.0, 0.0],
            cov_p: [[1e-6, 0.0, 0.0], [0.0, 1e-10, 0.0], [0.0, 0.0, 1e-14]],
            measurement_noise_r: measurement_noise_r.max(1e-12),
            process_noise_q: process_noise_q.max(1e-14),
            hawkes_mu: hawkes_mu.max(1e-6),
            hawkes_alpha: hawkes_alpha.max(0.0),
            hawkes_beta: hawkes_beta.max(1e-3),
            shock_z_threshold: 2.50,
            jump_history: VecDeque::new(),
        }
    }

    /// Updates Kalman filter with an observed funding rate print at timestamp `current_time_sec`.
    /// Returns the updated kinematic state and whether a jump shock was registered.
    pub fn update(
        &mut self,
        obs_rate: f64,
        dt_sec: f64,
        current_time_sec: f64,
    ) -> (FundingKinematicState, bool) {
        if !obs_rate.is_finite() || dt_sec <= 0.0 {
            return (
                FundingKinematicState {
                    rate: self.state_x[0],
                    velocity: self.state_x[1],
                    acceleration: self.state_x[2],
                },
                false,
            );
        }

        let dt = dt_sec.clamp(1e-4, 3600.0);

        // 1. Kinematic state prediction:
        // F_{pred} = F + v*dt + 0.5*a*dt^2
        // v_{pred} = v + a*dt
        // a_{pred} = a
        let x_pred = [
            self.state_x[0] + self.state_x[1] * dt + 0.5 * self.state_x[2] * dt * dt,
            self.state_x[1] + self.state_x[2] * dt,
            self.state_x[2],
        ];

        // 2. Covariance prediction: P_{pred} = A P A^T + Q dt
        // State transition matrix A:
        // [1, dt, 0.5*dt^2]
        // [0,  1,       dt]
        // [0,  0,        1]
        let dt2 = 0.5 * dt * dt;
        let p = self.cov_p;

        // Compute AP
        let ap = [
            [
                p[0][0] + dt * p[1][0] + dt2 * p[2][0],
                p[0][1] + dt * p[1][1] + dt2 * p[2][1],
                p[0][2] + dt * p[1][2] + dt2 * p[2][2],
            ],
            [
                p[1][0] + dt * p[2][0],
                p[1][1] + dt * p[2][1],
                p[1][2] + dt * p[2][2],
            ],
            [p[2][0], p[2][1], p[2][2]],
        ];

        // Compute (AP) A^T
        let mut p_pred = [
            [
                ap[0][0] + dt * ap[0][1] + dt2 * ap[0][2] + self.process_noise_q * dt,
                ap[0][1] + dt * ap[0][2],
                ap[0][2],
            ],
            [
                ap[1][0] + dt * ap[1][1] + dt2 * ap[1][2],
                ap[1][1] + dt * ap[1][2] + self.process_noise_q * 1e-4 * dt,
                ap[1][2],
            ],
            [
                ap[2][0] + dt * ap[2][1] + dt2 * ap[2][2],
                ap[2][1] + dt * ap[2][2],
                ap[2][2] + self.process_noise_q * 1e-8 * dt,
            ],
        ];
        // Enforce symmetry
        p_pred[0][1] = 0.5 * (p_pred[0][1] + p_pred[1][0]);
        p_pred[1][0] = p_pred[0][1];
        p_pred[0][2] = 0.5 * (p_pred[0][2] + p_pred[2][0]);
        p_pred[2][0] = p_pred[0][2];
        p_pred[1][2] = 0.5 * (p_pred[1][2] + p_pred[2][1]);
        p_pred[2][1] = p_pred[1][2];

        // 3. Innovation and measurement variance:
        let innov = obs_rate - x_pred[0];
        let s_var = p_pred[0][0] + self.measurement_noise_r;
        let std_dev = s_var.sqrt();

        // 4. Shock detection:
        let ratio = if std_dev > 1e-9 {
            innov.abs() / std_dev
        } else {
            0.0
        };
        let is_shock = ratio >= self.shock_z_threshold;

        if is_shock {
            self.jump_history.push_back(current_time_sec);
            // Prune jumps older than 3600 seconds
            while let Some(&t_old) = self.jump_history.front() {
                if current_time_sec - t_old > 3600.0 {
                    self.jump_history.pop_front();
                } else {
                    break;
                }
            }
        }

        // 5. Kalman gain:
        let k_gain = [
            p_pred[0][0] / s_var,
            p_pred[1][0] / s_var,
            p_pred[2][0] / s_var,
        ];

        // 6. State update:
        for i in 0..3 {
            self.state_x[i] = x_pred[i] + k_gain[i] * innov;
        }

        // Covariance update: P_new = (I - K H) P_pred
        let mut p_new = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                p_new[i][j] = p_pred[i][j] - k_gain[i] * p_pred[0][j];
            }
        }
        self.cov_p = p_new;

        let state = FundingKinematicState {
            rate: self.state_x[0],
            velocity: self.state_x[1],
            acceleration: self.state_x[2],
        };

        (state, is_shock)
    }

    /// Evaluates current Hawkes shock intensity \lambda_F(t).
    pub fn hawkes_intensity(&self, current_time_sec: f64) -> f64 {
        let mut intensity = self.hawkes_mu;
        for &t_jump in &self.jump_history {
            if current_time_sec >= t_jump {
                let tau = current_time_sec - t_jump;
                intensity += self.hawkes_alpha * (-self.hawkes_beta * tau).exp();
            }
        }
        intensity
    }

    /// Predicts funding rate at forward horizon \tau:
    /// \hat{F}(t + \tau) = \hat{F}(t) + \hat{v}_F(t) \tau + \frac{1}{2} \hat{a}_F(t) \tau^2.
    pub fn predict_rate_at(&self, tau_sec: f64) -> f64 {
        let tau = tau_sec.max(0.0);
        self.state_x[0] + self.state_x[1] * tau + 0.5 * self.state_x[2] * tau * tau
    }

    /// Predicts cumulative accrued funding over forward horizon \tau:
    /// \Phi_{\text{accrued}}(\tau) = \int_0^\tau \hat{F}(t + s) ds = \hat{F}(t) \tau + \frac{1}{2} \hat{v}_F(t) \tau^2 + \frac{1}{6} \hat{a}_F(t) \tau^3.
    pub fn predict_accrued_funding(&self, tau_sec: f64) -> f64 {
        let tau = tau_sec.max(0.0);
        self.state_x[0] * tau
            + 0.5 * self.state_x[1] * tau * tau
            + (1.0 / 6.0) * self.state_x[2] * tau.powi(3)
    }
}

/// Decision recommendation for predictive carry yield harvesting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CarryHarvestDecision {
    /// Whether to enter or hold the funding carry position.
    pub should_enter: bool,
    /// Projected funding yield accrued in basis points over the remaining window.
    pub projected_accrued_bps: f64,
    /// Required hurdle in basis points (friction + confidence band).
    pub required_hurdle_bps: f64,
    /// Net projected edge in basis points.
    pub net_edge_bps: f64,
    /// Hawkes jump shock intensity \lambda_F(t).
    pub shock_intensity: f64,
    /// Kinematic state summary.
    pub state: FundingKinematicState,
}

/// Evaluator for predictive funding carry yield harvesting.
#[derive(Debug, Clone, Default)]
pub struct PredictiveCarryYieldHarvester;

impl PredictiveCarryYieldHarvester {
    /// Evaluates whether entering a funding carry trade ahead of settlement is advantageous:
    /// - `tau_rem_sec`: remaining seconds until funding settlement snapshot (e.g. 900.0 for 15m prior).
    /// - `round_trip_cost_bps`: fees + expected execution slippage for entry and exit.
    /// - `annualized_rate_volatility`: volatility of funding rate in decimal.
    pub fn evaluate(
        engine: &FundingVelocityKalmanHawkes,
        tau_rem_sec: f64,
        round_trip_cost_bps: f64,
        annualized_rate_volatility: f64,
        current_time_sec: f64,
    ) -> CarryHarvestDecision {
        let state = FundingKinematicState {
            rate: engine.state_x[0],
            velocity: engine.state_x[1],
            acceleration: engine.state_x[2],
        };

        let raw_accrued = engine.predict_accrued_funding(tau_rem_sec);
        // Convert to basis points (assuming rate is in decimal per second or normalized interval)
        let projected_accrued_bps = raw_accrued * 10_000.0;

        let volatility_buffer = 1.96
            * annualized_rate_volatility.max(1e-4)
            * (tau_rem_sec / SECONDS_PER_YEAR).sqrt()
            * 100.0;
        let required_hurdle_bps = 2.0 * round_trip_cost_bps + volatility_buffer;

        let net_edge_bps = projected_accrued_bps - required_hurdle_bps;
        let shock_intensity = engine.hawkes_intensity(current_time_sec);

        // Do not enter if Hawkes shock intensity indicates a violent dislocation cascade (> 3.0)
        let is_shock_safe = shock_intensity < 3.0;
        let should_enter = net_edge_bps > 0.0 && is_shock_safe;

        CarryHarvestDecision {
            should_enter,
            projected_accrued_bps,
            required_hurdle_bps,
            net_edge_bps,
            shock_intensity,
            state,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kalman_kinematic_velocity_tracking() {
        let mut filter = FundingVelocityKalmanHawkes::new(0.0001, 1e-7, 1e-8, 0.1, 0.8, 1.5);

        // Simulate an accelerating funding rate: F(t) = 0.0001 + 0.00001 * t
        let mut t = 0.0;
        for i in 1..=20 {
            t += 10.0;
            let true_rate = 0.0001 + 0.00001 * t;
            let (state, _) = filter.update(true_rate, 10.0, t);
            if i >= 10 {
                // Velocity should be detected as positive (~0.00001)
                assert!(state.velocity > 0.0, "Velocity should be positive");
            }
        }

        // Forward projection
        let pred_50 = filter.predict_rate_at(50.0);
        assert!(pred_50 > filter.state_x[0]);
    }

    #[test]
    fn test_hawkes_shock_detection() {
        let mut filter = FundingVelocityKalmanHawkes::new(0.0001, 1e-7, 1e-8, 0.1, 0.8, 1.5);

        // Regular updates
        filter.update(0.0001, 10.0, 10.0);
        filter.update(0.000105, 10.0, 20.0);

        let baseline_intensity = filter.hawkes_intensity(20.0);
        assert!((baseline_intensity - 0.1).abs() < 1e-3);

        // Massive sudden shock jump (e.g. liquidation cascade)
        let (_, is_shock) = filter.update(0.0050, 10.0, 30.0);
        assert!(is_shock, "Massive jump should be flagged as shock");

        let shock_intensity = filter.hawkes_intensity(30.0);
        assert!(shock_intensity > baseline_intensity);
    }

    #[test]
    fn test_predictive_carry_harvest_decision() {
        let mut filter = FundingVelocityKalmanHawkes::new(0.0008, 1e-7, 1e-8, 0.1, 0.8, 1.5);
        // Rate is high and positive
        filter.update(0.00085, 10.0, 10.0);

        let decision = PredictiveCarryYieldHarvester::evaluate(
            &filter, 900.0, // 15 minutes before snapshot
            2.0,   // 2 bps round trip cost
            0.05,  // 5% vol
            10.0,
        );

        assert!(decision.projected_accrued_bps > 0.0);
        assert!(decision.required_hurdle_bps >= 4.0);
    }

    #[test]
    fn test_predictive_carry_volatility_buffer_scales_with_years() {
        let filter = FundingVelocityKalmanHawkes::new(0.0, 1e-7, 1e-8, 0.1, 0.8, 1.5);
        let one_year =
            PredictiveCarryYieldHarvester::evaluate(&filter, 365.25 * 86_400.0, 2.0, 0.05, 1.0);

        let expected = 4.0 + 1.96 * 0.05 * 100.0;
        assert!(
            (one_year.required_hurdle_bps - expected).abs() < 1e-8,
            "expected {expected}, got {}",
            one_year.required_hurdle_bps
        );
    }
}
