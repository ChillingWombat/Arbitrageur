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
        let h = [1.0, x];

        // 3. Measurement prediction and residual:
        // y_hat = H * state = alpha + beta * x
        let y_hat = self.state[0] * h[0] + self.state[1] * h[1];
        let residual = y - y_hat;
        self.latest_residual = residual;

        // 4. Innovation variance:
        // S = H * P * H^T + R
        // H * P = [h0*P00 + h1*P10, h0*P01 + h1*P11]
        let hp0 = h[0] * self.p[0][0] + h[1] * self.p[1][0];
        let hp1 = h[0] * self.p[0][1] + h[1] * self.p[1][1];
        let s = hp0 * h[0] + hp1 * h[1] + self.r;
        self.latest_variance = s;

        if s.abs() < 1e-15 {
            return (self.state[1], residual);
        }

        // 5. Kalman gain:
        // K = P * H^T / S
        // P * H^T = [P00*h0 + P01*h1, P10*h0 + P11*h1]
        let pht0 = self.p[0][0] * h[0] + self.p[0][1] * h[1];
        let pht1 = self.p[1][0] * h[0] + self.p[1][1] * h[1];
        let k0 = pht0 / s;
        let k1 = pht1 / s;

        // 6. State update:
        // state = state + K * residual
        self.state[0] += k0 * residual;
        self.state[1] += k1 * residual;

        // 7. Covariance update:
        // P = (I - K * H) * P
        let kh00 = k0 * h[0];
        let kh01 = k0 * h[1];
        let kh10 = k1 * h[0];
        let kh11 = k1 * h[1];

        let i_minus_kh00 = 1.0 - kh00;
        let i_minus_kh01 = -kh01;
        let i_minus_kh10 = -kh10;
        let i_minus_kh11 = 1.0 - kh11;

        let new_p00 = i_minus_kh00 * self.p[0][0] + i_minus_kh01 * self.p[1][0];
        let new_p01 = i_minus_kh00 * self.p[0][1] + i_minus_kh01 * self.p[1][1];
        let new_p10 = i_minus_kh10 * self.p[0][0] + i_minus_kh11 * self.p[1][0];
        let new_p11 = i_minus_kh10 * self.p[0][1] + i_minus_kh11 * self.p[1][1];

        let sym_p01 = (new_p01 + new_p10) * 0.5;
        self.p = [
            [new_p00.max(1e-12), sym_p01],
            [sym_p01, new_p11.max(1e-12)],
        ];

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
