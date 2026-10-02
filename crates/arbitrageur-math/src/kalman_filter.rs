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
}
