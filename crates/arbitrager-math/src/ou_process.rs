use std::f64::consts::LN_2;

/// Parameters of an Ornstein-Uhlenbeck continuous mean-reverting process:
/// dX_t = \theta (\mu - X_t) dt + \sigma dW_t
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OuModelParams {
    /// Mean reversion speed (rate) \theta > 0
    pub theta: f64,
    /// Long-term equilibrium mean \mu
    pub mu: f64,
    /// Volatility parameter \sigma > 0
    pub sigma: f64,
    /// Half-life of mean reversion = ln(2) / \theta
    pub half_life: f64,
    /// Stationary standard deviation = \sigma / \sqrt{2 \theta}
    pub stationary_std: f64,
    /// R^2 goodness of fit
    pub r_squared: f64,
}

impl OuModelParams {
    /// Fit Ornstein-Uhlenbeck process parameters to time series `series` sampled at interval `dt`.
    /// Uses exact Ordinary Least Squares / Maximum Likelihood Estimation (MLE) on discrete AR(1):
    /// X_k = a X_{k-1} + b + \epsilon_k
    /// where:
    ///   a = exp(-\theta dt)
    ///   b = \mu (1 - a)
    ///   \sigma_\epsilon^2 = \frac{\sigma^2}{2\theta} (1 - a^2)
    pub fn fit(series: &[f64], dt: f64) -> Option<Self> {
        let n = series.len();
        if n < 10 || dt <= 0.0 {
            return None;
        }

        let mut sum_x = 0.0;
        let mut sum_y = 0.0;
        let mut sum_xx = 0.0;
        let mut sum_xy = 0.0;

        let m = (n - 1) as f64;
        for i in 0..(n - 1) {
            let x = series[i];
            let y = series[i + 1];
            sum_x += x;
            sum_y += y;
            sum_xx += x * x;
            sum_xy += x * y;
        }

        // OLS: y = a*x + b
        let denominator = m * sum_xx - sum_x * sum_x;
        if denominator.abs() < 1e-15 {
            return None;
        }

        let a = (m * sum_xy - sum_x * sum_y) / denominator;
        let b = (sum_y - a * sum_x) / m;

        // For mean reversion, 0 < a < 1
        if a <= 0.0 || a >= 1.0 {
            return None;
        }

        let theta = -a.ln() / dt;
        let mu = b / (1.0 - a);

        // Residual sum of squares
        let mut rss = 0.0;
        for i in 0..(n - 1) {
            let pred = a * series[i] + b;
            let err = series[i + 1] - pred;
            rss += err * err;
        }
        let var_eps = rss / m;

        let sigma = (var_eps * 2.0 * theta / (1.0 - a * a)).sqrt();
        let half_life = LN_2 / theta;
        let stationary_std = sigma / (2.0 * theta).sqrt();

        // R^2 calculation
        let y_mean = sum_y / m;
        let mut tss = 0.0;
        for val in &series[1..n] {
            let d = *val - y_mean;
            tss += d * d;
        }
        let r_squared = if tss > 1e-15 {
            (1.0 - rss / tss).max(0.0)
        } else {
            0.0
        };

        Some(OuModelParams {
            theta,
            mu,
            sigma,
            half_life,
            stationary_std,
            r_squared,
        })
    }

    /// Calculate conditional expectation E[X_{t+\Delta t} | X_t = x].
    pub fn conditional_mean(&self, x_t: f64, delta_t: f64) -> f64 {
        let decay = (-self.theta * delta_t).exp();
        x_t * decay + self.mu * (1.0 - decay)
    }

    /// Calculate conditional variance Var(X_{t+\Delta t} | X_t = x).
    pub fn conditional_variance(&self, delta_t: f64) -> f64 {
        let decay = (-2.0 * self.theta * delta_t).exp();
        (self.sigma * self.sigma / (2.0 * self.theta)) * (1.0 - decay)
    }
}
