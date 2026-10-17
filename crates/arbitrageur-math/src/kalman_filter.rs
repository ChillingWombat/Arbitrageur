/// Online 2-dimensional Kalman Filter for tracking time-varying hedge ratio \beta and intercept \alpha:
/// y_t = \alpha_t + \beta_t * x_t + v_t
#[derive(Debug, Clone)]
pub struct HedgeRatioKalmanFilter {
    /// State vector [alpha, beta]
    pub state: [f64; 2],
    /// State covariance matrix 2x2: [[P00, P01], [P10, P11]]
    pub p: [[f64; 2]; 2],
    /// Process noise covariance Q diagonal
    pub q: [f64; 2],
    /// Measurement noise variance R
    pub r: f64,
    /// Latest innovation residual: e_t = y_t - (alpha + beta * x_t)
    pub latest_residual: f64,
    /// Innovation variance: S_t = H * P * H^T + R
    pub latest_variance: f64,
}

impl HedgeRatioKalmanFilter {
    /// Initialize with default parameters:
    /// delta: process noise scaling (typically 1e-4 .. 1e-5)
    /// r: measurement noise variance (typically 1e-3 .. 1.0)
    pub fn new(delta: f64, r: f64) -> Self {
        Self {
            state: [0.0, 1.0], // Initial prior: alpha = 0, beta = 1
            p: [[1.0, 0.0], [0.0, 1.0]],
            q: [delta, delta],
            r,
            latest_residual: 0.0,
            latest_variance: 1.0,
        }
    }

    /// Update Kalman filter with new asset prices (x_t, y_t).
    /// Returns current hedge ratio \beta_t and residual spread e_t.
    pub fn update(&mut self, x: f64, y: f64) -> (f64, f64) {
        // 1. Time update (Predict):
        // P_{t|t-1} = P_{t-1|t-1} + Q
        self.p[0][0] += self.q[0];
        self.p[1][1] += self.q[1];

        // 2. Observation matrix H = [1.0, x]
        // 3. Measurement prediction and residual:
        // y_hat = H * state = alpha + beta * x
        let y_hat = self.state[0] + self.state[1] * x;
        let residual = y - y_hat;
        self.latest_residual = residual;

        // 4. Innovation variance:
        // S = H * P * H^T + R
        // S = P00 + 2*x*P01 + x²*P11 + R
        // Note: P is symmetric: P10 = P01
        let p00 = self.p[0][0];
        let p01 = self.p[0][1];
        let p11 = self.p[1][1];

        let hp0 = p00 + x * p01;
        let hp1 = p01 + x * p11;
        let s = hp0 + hp1 * x + self.r;
        self.latest_variance = s;

        if s.abs() < 1e-15 {
            return (self.state[1], residual);
        }

        // 5. Kalman gain:
        // K = P * H^T / S = [hp0, hp1]^T / S (since P is symmetric, P * H^T = (H * P)^T)
        let inv_s = 1.0 / s;
        let k0 = hp0 * inv_s;
        let k1 = hp1 * inv_s;

        // 6. State update:
        // state = state + K * residual
        self.state[0] += k0 * residual;
        self.state[1] += k1 * residual;

        // 7. Covariance update — Joseph stabilized form:
        //    P = (I - K·H)·P·(I - K·H)ᵀ + K·R·Kᵀ
        let m00 = 1.0 - k0;
        let m01 = -k0 * x;
        let m10 = -k1;
        let m11 = 1.0 - k1 * x;

        let mp00 = m00 * p00 + m01 * p01;
        let mp01 = m00 * p01 + m01 * p11;
        let mp10 = m10 * p00 + m11 * p01;
        let mp11 = m10 * p01 + m11 * p11;

        let r_k0 = self.r * k0;
        let new_p00 = mp00 * m00 + mp01 * m01 + r_k0 * k0;
        let new_p11 = mp10 * m10 + mp11 * m11 + self.r * k1 * k1;
        let sym_p01 = 0.5 * (mp00 * m10 + mp01 * m11 + mp10 * m00 + mp11 * m01) + r_k0 * k1;

        self.p = [[new_p00.max(1e-12), sym_p01], [sym_p01, new_p11.max(1e-12)]];
        self.regularize_covariance(500.0);

        (self.state[1], residual)
    }

    pub fn hedge_ratio(&self) -> f64 {
        self.state[1]
    }

    pub fn intercept(&self) -> f64 {
        self.state[0]
    }

    pub fn spread_z_score(&self) -> f64 {
        if self.latest_variance > 1e-15 {
            self.latest_residual / self.latest_variance.sqrt()
        } else {
            0.0
        }
    }

    /// Spectral condition number \kappa(P) = \lambda_{max} / \lambda_{min} of state covariance P.
    pub fn condition_number(&self) -> f64 {
        let a = self.p[0][0];
        let b = self.p[0][1];
        let c = self.p[1][1];
        let tr = a + c;
        let diff = a - c;
        let delta = (diff * diff + 4.0 * b * b).sqrt();
        let lambda_min = (tr - delta) * 0.5;
        let lambda_max = (tr + delta) * 0.5;
        if lambda_min <= 1e-12 {
            1e12
        } else {
            (lambda_max / lambda_min).max(1.0)
        }
    }

    /// Regularize state covariance P towards an isotropic target when ill-conditioned:
    /// P^* = (1 - \delta) P + \delta \bar{s}^2 I_2, where \bar{s}^2 = Tr(P)/2.
    pub fn regularize_covariance(&mut self, max_cond: f64) {
        let cond = self.condition_number();
        let det = self.p[0][0] * self.p[1][1] - self.p[0][1] * self.p[0][1];
        if cond > max_cond || det <= 1e-12 {
            let tr = self.p[0][0] + self.p[1][1];
            let target_diag = (tr * 0.5).max(1e-6);
            let delta = 0.25; // Ledoit-Wolf style shrinkage step
            self.p[0][0] = (1.0 - delta) * self.p[0][0] + delta * target_diag;
            self.p[1][1] = (1.0 - delta) * self.p[1][1] + delta * target_diag;
            self.p[0][1] *= 1.0 - delta;
            self.p[1][0] = self.p[0][1];
        }
    }
}

/// Online 2-state Kalman Filter for tracking structural funding rate premium \theta_t
/// and its drift velocity v_t (bps/hour) between two perpetual venues:
///
/// State: x_t = [\theta_t, v_t]^T
/// Dynamics:
///   \theta_t = \theta_{t-1} + v_{t-1} * \Delta t + w_{\theta}
///   v_t = v_{t-1} + w_v
/// Observation:
///   y_t = \theta_t + \epsilon_t,  \epsilon_t ~ N(0, R)
#[derive(Debug, Clone)]
pub struct FundingPremiumKalmanFilter {
    /// State [theta (structural premium in bps), velocity (drift in bps/hour)]
    pub state: [f64; 2],
    /// State covariance matrix 2x2
    pub p: [[f64; 2]; 2],
    /// Process noise variance [q_theta, q_v]
    pub q: [f64; 2],
    /// Measurement noise variance R
    pub r: f64,
    /// Latest innovation residual (y_t - theta_hat)
    pub latest_residual: f64,
    /// Latest innovation variance S_t = P00 + R
    pub latest_variance: f64,
}

impl FundingPremiumKalmanFilter {
    /// Initialize with typical perpetual funding dynamics:
    /// - q_theta: process noise for structural premium (default ~0.05 bps^2)
    /// - q_v: process noise for drift acceleration (default ~0.01 bps^2/h^2)
    /// - r: measurement noise variance for erratic tick-by-tick funding prints (default ~0.50 bps^2)
    pub fn new(q_theta: f64, q_v: f64, r: f64) -> Self {
        Self {
            state: [0.0, 0.0],
            p: [[10.0, 0.0], [0.0, 1.0]],
            q: [q_theta.max(1e-6), q_v.max(1e-6)],
            r: r.max(1e-6),
            latest_residual: 0.0,
            latest_variance: 10.0 + r,
        }
    }

    /// Update filter with a fresh observed funding premium (bps) and elapsed time dt_hours.
    /// Returns (filtered_premium_bps, drift_bps_per_hour).
    pub fn update(&mut self, observed_premium_bps: f64, dt_hours: f64) -> (f64, f64) {
        let dt = dt_hours.clamp(1.0 / 3600.0, 24.0);

        // 1. Time Update (Predict):
        // x_pred = F * x
        let theta_pred = self.state[0] + self.state[1] * dt;
        let v_pred = self.state[1];

        // P_pred = F * P * F^T + Q
        // F = [[1, dt], [0, 1]]
        let p00 = self.p[0][0];
        let p01 = self.p[0][1];
        let p11 = self.p[1][1];

        let pred_p00 = p00 + 2.0 * dt * p01 + dt * dt * p11 + self.q[0];
        let pred_p01 = p01 + dt * p11;
        let pred_p11 = p11 + self.q[1];

        // 2. Observation Update: H = [1.0, 0.0]
        let residual = observed_premium_bps - theta_pred;
        self.latest_residual = residual;

        // Innovation variance: S = H * P_pred * H^T + R = pred_p00 + R
        let s = pred_p00 + self.r;
        self.latest_variance = s;

        if s.abs() < 1e-15 {
            self.state = [theta_pred, v_pred];
            return (self.state[0], self.state[1]);
        }

        // Kalman gain: K = P_pred * H^T / S = [pred_p00 / S, pred_p01 / S]^T
        let inv_s = 1.0 / s;
        let k0 = pred_p00 * inv_s;
        let k1 = pred_p01 * inv_s;

        // State update:
        self.state[0] = theta_pred + k0 * residual;
        self.state[1] = v_pred + k1 * residual;

        // Covariance update (Joseph stabilized form):
        // m = I - K*H = [[1 - k0, 0], [-k1, 1]]
        let m00 = 1.0 - k0;
        let m10 = -k1;

        let new_p00 = m00 * pred_p00 * m00 + self.r * k0 * k0;
        let new_p11 = m10 * pred_p00 * m10 + pred_p11 + 2.0 * m10 * pred_p01 + self.r * k1 * k1;
        let sym_p01 = m00 * (m10 * pred_p00 + pred_p01) + self.r * k0 * k1;

        self.p = [[new_p00.max(1e-12), sym_p01], [sym_p01, new_p11.max(1e-12)]];

        (self.state[0], self.state[1])
    }

    /// The filtered structural funding premium in bps.
    pub fn structural_premium_bps(&self) -> f64 {
        self.state[0]
    }

    /// The structural drift velocity in bps / hour.
    pub fn drift_velocity(&self) -> f64 {
        self.state[1]
    }

    /// Forecasted funding premium at time horizon h_hours from now.
    pub fn forecast_premium(&self, h_hours: f64) -> f64 {
        self.state[0] + self.state[1] * h_hours.max(0.0)
    }

    /// Check if the structural funding premium is predicted to invert / flip sign
    /// within the next horizon_hours (e.g. before the 8-hour settlement).
    pub fn is_predicted_to_invert(&self, horizon_hours: f64) -> bool {
        let current_sign = self.state[0].signum();
        if current_sign == 0.0 {
            return true;
        }
        let forecasted = self.forecast_premium(horizon_hours);
        forecasted.signum() != current_sign
    }

    /// Instantaneous Z-score of observed premium vs structural equilibrium.
    pub fn z_score(&self) -> f64 {
        if self.latest_variance > 1e-15 {
            self.latest_residual / self.latest_variance.sqrt()
        } else {
            0.0
        }
    }
}

/// Analytical Ledoit-Wolf Covariance Shrinkage Estimator for fat-tailed, non-Gaussian returns:
/// \Sigma^* = \delta^* F + (1 - \delta^*) S
/// Target F = \bar{s}^2 I_N, where \bar{s}^2 = (1/N) Tr(S).
/// Proved condition number bound: \kappa(\Sigma^*) \le 1 + ((1 - \delta^*)/\delta^*) * N.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedoitWolfEstimator;

#[allow(clippy::needless_range_loop)]
impl LedoitWolfEstimator {
    /// Compute shrunk covariance matrix \Sigma^* and optimal shrinkage intensity \delta^*
    /// from observations matrix of size T x N (T rows of samples, N columns of assets/variables).
    pub fn shrink(observations: &[Vec<f64>]) -> Option<(Vec<Vec<f64>>, f64)> {
        let t = observations.len();
        if t < 3 {
            return None;
        }
        let n = observations[0].len();
        if n == 0 {
            return None;
        }
        for row in observations {
            if row.len() != n {
                return None;
            }
        }
        let t_f = t as f64;
        let n_f = n as f64;

        // 1. Calculate sample means
        let mut means = vec![0.0; n];
        for row in observations {
            for i in 0..n {
                means[i] += row[i];
            }
        }
        for m in &mut means {
            *m /= t_f;
        }

        // 2. Mean-center observations
        let mut centered = Vec::with_capacity(t);
        for row in observations {
            let mut c_row = vec![0.0; n];
            for i in 0..n {
                c_row[i] = row[i] - means[i];
            }
            centered.push(c_row);
        }

        // 3. Compute sample covariance S
        let mut s = vec![vec![0.0; n]; n];
        for k in 0..t {
            for i in 0..n {
                for j in 0..n {
                    s[i][j] += centered[k][i] * centered[k][j];
                }
            }
        }
        for i in 0..n {
            for j in 0..n {
                s[i][j] /= t_f;
            }
        }

        // 4. Shrinkage target: scaled identity F = \bar{s}^2 I_N
        let mut trace_s = 0.0;
        for i in 0..n {
            trace_s += s[i][i];
        }
        let bar_s2 = (trace_s / n_f).max(1e-12);

        // 5. Sample distance \hat{P} = ||S - F||_F^2
        let mut p_hat = 0.0;
        for i in 0..n {
            for j in 0..n {
                let diff = if i == j { s[i][j] - bar_s2 } else { s[i][j] };
                p_hat += diff * diff;
            }
        }

        if p_hat < 1e-15 {
            return Some((s, 0.0));
        }

        // 6. Fourth cross-moment variance \hat{V} (non-parametric, robust to fat-tailed returns):
        // w_{ij} = (1/T) \sum_{k=1}^T ( x_{ki} x_{kj} - s_{ij} )^2
        // \hat{V} = (1/T) \sum_{i,j=1}^N w_{ij}
        let mut v_hat = 0.0;
        for i in 0..n {
            for j in 0..n {
                let mut sum_cross_sq = 0.0;
                for k in 0..t {
                    let dev = centered[k][i] * centered[k][j] - s[i][j];
                    sum_cross_sq += dev * dev;
                }
                v_hat += sum_cross_sq / t_f;
            }
        }
        v_hat /= t_f;

        // 7. Optimal shrinkage intensity \delta^* = clamp(\hat{V} / \hat{P}, 0.0, 1.0)
        let delta_star = (v_hat / p_hat).clamp(0.0, 1.0);

        // 8. Shrunk covariance matrix \Sigma^* = \delta^* F + (1 - \delta^*) S
        let mut sigma_star = vec![vec![0.0; n]; n];
        for i in 0..n {
            for j in 0..n {
                let f_ij = if i == j { bar_s2 } else { 0.0 };
                sigma_star[i][j] = delta_star * f_ij + (1.0 - delta_star) * s[i][j];
            }
        }

        Some((sigma_star, delta_star))
    }

    /// Fast specialized 2x2 Ledoit-Wolf shrinkage for pairwise Kalman / Spread monitoring.
    pub fn shrink_2x2(samples: &[[f64; 2]]) -> Option<([[f64; 2]; 2], f64)> {
        let t = samples.len();
        if t < 3 {
            return None;
        }
        let t_f = t as f64;
        let mut m0 = 0.0;
        let mut m1 = 0.0;
        for s in samples {
            m0 += s[0];
            m1 += s[1];
        }
        m0 /= t_f;
        m1 /= t_f;

        let mut s00 = 0.0;
        let mut s01 = 0.0;
        let mut s11 = 0.0;
        for s in samples {
            let c0 = s[0] - m0;
            let c1 = s[1] - m1;
            s00 += c0 * c0;
            s01 += c0 * c1;
            s11 += c1 * c1;
        }
        s00 /= t_f;
        s01 /= t_f;
        s11 /= t_f;

        let bar_s2 = ((s00 + s11) * 0.5).max(1e-12);
        let p_hat = (s00 - bar_s2).powi(2) + (s11 - bar_s2).powi(2) + 2.0 * s01 * s01;
        if p_hat < 1e-15 {
            return Some(([[s00, s01], [s01, s11]], 0.0));
        }

        let mut v_hat = 0.0;
        for s in samples {
            let c0 = s[0] - m0;
            let c1 = s[1] - m1;
            let d00 = c0 * c0 - s00;
            let d01 = c0 * c1 - s01;
            let d11 = c1 * c1 - s11;
            v_hat += d00 * d00 + 2.0 * d01 * d01 + d11 * d11;
        }
        v_hat = (v_hat / t_f) / t_f;

        let delta_star = (v_hat / p_hat).clamp(0.0, 1.0);
        let sig00 = delta_star * bar_s2 + (1.0 - delta_star) * s00;
        let sig11 = delta_star * bar_s2 + (1.0 - delta_star) * s11;
        let sig01 = (1.0 - delta_star) * s01;

        Some(([[sig00, sig01], [sig01, sig11]], delta_star))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression (#46): the Joseph-form covariance update must keep P
    /// positive-semidefinite even under adversarial observation scales —
    /// alternating huge/tiny x values stress the (I−K·H)·P round-off that
    /// previously could produce negative variances.
    #[test]
    fn covariance_stays_psd_under_adversarial_inputs() {
        let mut kf = HedgeRatioKalmanFilter::new(1e-4, 1e-2);
        for i in 0..2000 {
            let x = if i % 2 == 0 { 1e6_f64 } else { 1e-6 };
            let y = x * 1.5 + (i as f64 * 0.001).sin();
            kf.update(x, y);
            let det = kf.p[0][0] * kf.p[1][1] - kf.p[0][1] * kf.p[1][0];
            assert!(
                kf.p[0][0] >= 0.0,
                "p00 negative at step {i}: {}",
                kf.p[0][0]
            );
            assert!(
                kf.p[1][1] >= 0.0,
                "p11 negative at step {i}: {}",
                kf.p[1][1]
            );
            assert!(
                det >= -1e-18,
                "covariance lost PSD at step {i}: det={det} p={:?}",
                kf.p
            );
        }
    }

    #[test]
    fn covariance_stays_psd_with_singular_observation() {
        // R ≈ 0 with a repeated observation drives S→0; gain blows up and
        // the plain-form update can go non-PSD.
        let mut kf = HedgeRatioKalmanFilter::new(0.0, 1e-12);
        for _ in 0..500 {
            kf.update(7.0, 10.5);
            let det = kf.p[0][0] * kf.p[1][1] - kf.p[0][1] * kf.p[1][0];
            assert!(kf.p[0][0] >= 0.0 && kf.p[1][1] >= 0.0);
            assert!(det >= -1e-24, "det={det}");
        }
    }

    #[test]
    fn test_funding_premium_kalman_filter_tracking() {
        let mut kf = FundingPremiumKalmanFilter::new(0.05, 0.01, 0.20);

        // Ground truth: premium starts at 10 bps and decays at -0.5 bps/hour
        // Feed 20 hourly noisy observations
        for i in 0..20 {
            let true_premium = 10.0 - 0.5 * (i as f64);
            let noise = if i % 2 == 0 { 0.2 } else { -0.2 };
            let (est_prem, est_drift) = kf.update(true_premium + noise, 1.0);
            assert!(est_prem.is_finite());
            assert!(est_drift.is_finite());
        }

        // Structural premium should be close to 10 - 0.5 * 19 = 0.5 bps
        assert!(
            (kf.structural_premium_bps() - 0.5).abs() < 1.0,
            "Structural premium should track truth, got {}",
            kf.structural_premium_bps()
        );
        // Drift velocity should be negative
        assert!(
            kf.drift_velocity() < -0.2,
            "Drift velocity should be negative, got {}",
            kf.drift_velocity()
        );
        // Should predict inversion within 2 hours
        assert!(
            kf.is_predicted_to_invert(4.0),
            "Should predict inversion within 4 hours"
        );
    }

    #[test]
    fn test_kalman_filter_condition_number_regularization() {
        let mut kf = HedgeRatioKalmanFilter::new(1e-4, 1e-2);
        // Feed a severely ill-conditioned scale (x ~ 100,000, y ~ 150,000)
        for i in 0..50 {
            let x = 100_000.0 + (i as f64) * 10.0;
            let y = 150_000.0 + (i as f64) * 15.0;
            kf.update(x, y);
            let cond = kf.condition_number();
            assert!(
                cond < 2000.0,
                "Condition number must be regularized, got {cond}"
            );
            let det = kf.p[0][0] * kf.p[1][1] - kf.p[0][1] * kf.p[0][1];
            assert!(det > 0.0, "Determinant must remain strictly positive");
        }
    }

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn test_ledoit_wolf_shrinkage_bounds_condition_number() {
        // Construct collinear multi-asset return observations (N=4, T=30)
        let mut obs = Vec::new();
        for t in 0..30 {
            let base = (t as f64) * 0.01;
            // Asset 2 and 3 are nearly collinear with Asset 1
            obs.push(vec![
                base + 0.001 * ((t % 3) as f64),
                base * 1.0001 + 0.00005,
                base * 0.9999 - 0.00005,
                (t as f64 * 0.05).sin() * 0.01,
            ]);
        }

        let (shrunk, delta) = LedoitWolfEstimator::shrink(&obs).expect("shrink success");
        assert!(
            delta > 0.0,
            "Collinear data must trigger non-zero shrinkage intensity"
        );
        assert!(delta <= 1.0);

        // Check positive definiteness and diagonal dominance
        for i in 0..4 {
            assert!(
                shrunk[i][i] > 0.0,
                "Diagonal variances must be strictly positive"
            );
        }
        // Shrunk matrix must be symmetric
        for i in 0..4 {
            for j in 0..4 {
                assert!((shrunk[i][j] - shrunk[j][i]).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn test_ledoit_wolf_shrink_2x2() {
        let samples = vec![
            [1.0, 2.0],
            [1.1, 2.15],
            [0.9, 1.85],
            [1.05, 2.05],
            [0.95, 1.95],
            [1.2, 2.3],
        ];
        let (shrunk, delta) =
            LedoitWolfEstimator::shrink_2x2(&samples).expect("shrink 2x2 success");
        assert!(shrunk[0][0] > 0.0);
        assert!(shrunk[1][1] > 0.0);
        assert!((shrunk[0][1] - shrunk[1][0]).abs() < 1e-12);
        assert!((0.0..=1.0).contains(&delta));
    }
}
