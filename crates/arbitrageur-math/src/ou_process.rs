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
    /// R^2 goodness of fit — `Some` only when produced by `fit`; callers
    /// constructing params directly (no regression was run) set `None`,
    /// and quality gates treat `None` as unfit.
    pub r_squared: Option<f64>,
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

        let m = (n - 1) as f64;
        for i in 0..(n - 1) {
            let x = series[i];
            let y = series[i + 1];
            sum_x += x;
            sum_y += y;
        }

        // OLS: y = a*x + b
        // Numerically stable two-pass de-meaning (avoids catastrophic cancellation)
        let mean_x = sum_x / m;
        let mean_y = sum_y / m;
        let mut s_xx = 0.0;
        let mut s_xy = 0.0;
        for i in 0..(n - 1) {
            let dx = series[i] - mean_x;
            let dy = series[i + 1] - mean_y;
            s_xx += dx * dx;
            s_xy += dx * dy;
        }

        if s_xx.abs() < 1e-15 {
            return None;
        }

        let a = s_xy / s_xx;
        let b = mean_y - a * mean_x;

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
        // R² is only meaningful when the series has variance to explain;
        // a near-constant series yields None rather than a fake 0.0 score.
        let r_squared = if tss > 1e-15 {
            Some((1.0 - rss / tss).max(0.0))
        } else {
            None
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

    /// Fit Ornstein-Uhlenbeck parameters to series with microstructure noise filtering.
    ///
    /// When observed tick prices Y_k = X_k + \eta_k contain bid-ask bounce noise \eta_k ~ i.i.d.(0, \sigma_\eta^2),
    /// standard AR(1) OLS suffers from severe errors-in-variables attenuation bias:
    ///   a_OLS = Cov(Y_k, Y_{k-1}) / Var(Y_k) = a * Var(X) / (Var(X) + Var(\eta)) < a
    /// which causes theta -> infinity and half-life -> 0 as sampling interval shrinks.
    ///
    /// By using the discrete ARMA(1, 1) autocovariance ratio:
    ///   \gamma_1 = Cov(Y_k, Y_{k-1}) = a * Var(X)
    ///   \gamma_2 = Cov(Y_k, Y_{k-2}) = a^2 * Var(X)
    ///   => a = \gamma_2 / \gamma_1
    /// we eliminate the noise variance Var(\eta) from the estimator of a = exp(-\theta * dt).
    ///
    /// Returns `(OuModelParams, noise_variance)`.
    pub fn fit_with_noise(series: &[f64], dt: f64) -> Option<(Self, f64)> {
        let n = series.len();
        if n < 15 || dt <= 0.0 {
            return None;
        }

        let mut sum_y = 0.0;
        for &val in series {
            if !val.is_finite() {
                return None;
            }
            sum_y += val;
        }
        let mu = sum_y / (n as f64);

        // Demeaned sample autocovariances
        let mut gamma0 = 0.0;
        for &val in series {
            let diff = val - mu;
            gamma0 += diff * diff;
        }
        gamma0 /= n as f64;

        let mut gamma1 = 0.0;
        for i in 0..(n - 1) {
            gamma1 += (series[i] - mu) * (series[i + 1] - mu);
        }
        gamma1 /= (n - 1) as f64;

        let mut gamma2 = 0.0;
        for i in 0..(n - 2) {
            gamma2 += (series[i] - mu) * (series[i + 2] - mu);
        }
        gamma2 /= (n - 2) as f64;

        // If autocovariances are nonpositive, mean-reversion decay signal is weak/absent
        if gamma1 <= 1e-12 || gamma2 <= 1e-12 {
            return Self::fit(series, dt).map(|p| (p, 0.0));
        }

        let a = gamma2 / gamma1;
        if a <= 0.0 || a >= 0.9999 {
            return Self::fit(series, dt).map(|p| (p, 0.0));
        }

        let theta = -a.ln() / dt;
        let half_life = LN_2 / theta;

        // Stationary variance of underlying continuous OU process X
        let var_x = (gamma1 / a).max(1e-12);
        let noise_var = (gamma0 - var_x).max(0.0);
        let stationary_std = var_x.sqrt();
        let sigma = (2.0 * theta * var_x).sqrt();

        // Conditional prediction R^2
        let mut rss = 0.0;
        let mut tss = 0.0;
        for i in 0..(n - 1) {
            let pred = series[i] * a + mu * (1.0 - a);
            let err = series[i + 1] - pred;
            rss += err * err;
            let d = series[i + 1] - mu;
            tss += d * d;
        }
        let r_squared = if tss > 1e-15 {
            Some((1.0 - rss / tss).max(0.0))
        } else {
            None
        };

        Some((
            OuModelParams {
                theta,
                mu,
                sigma,
                half_life,
                stationary_std,
                r_squared,
            },
            noise_var,
        ))
    }

    /// Fit Ornstein-Uhlenbeck process parameters to irregular time series observations
    /// (timestamps, values) where inter-arrival times dt_i = timestamps[i] - timestamps[i-1] vary.
    ///
    /// Uses continuous-time Maximum Likelihood Estimation via 1D Profile Likelihood
    /// search over theta. For each candidate theta, mu*(theta) and sigma^2*(theta) are
    /// solved in closed form via exact transition density weighted least squares.
    pub fn fit_irregular(timestamps: &[f64], values: &[f64]) -> Option<Self> {
        let n = values.len();
        if n < 15 || timestamps.len() != n {
            return None;
        }

        // Validate strictly positive and finite time increments
        for i in 0..(n - 1) {
            let dt = timestamps[i + 1] - timestamps[i];
            if !dt.is_finite() || dt <= 0.0 || !values[i].is_finite() {
                return None;
            }
        }
        if !values[n - 1].is_finite() {
            return None;
        }

        let total_time = timestamps[n - 1] - timestamps[0];
        if total_time <= 0.0 {
            return None;
        }

        // Bounded search interval for mean reversion speed theta
        let theta_min = (LN_2 / (10.0 * total_time)).max(1e-6);
        let theta_max = (LN_2 / 0.5).min(1e4);

        const INV_PHI: f64 = 0.6180339887498949;
        let mut a = theta_min;
        let mut b = theta_max;
        let mut x1 = b - INV_PHI * (b - a);
        let mut x2 = a + INV_PHI * (b - a);

        let mut f1 = Self::eval_profile_nll(timestamps, values, x1);
        let mut f2 = Self::eval_profile_nll(timestamps, values, x2);

        for _ in 0..40 {
            if f1 < f2 {
                b = x2;
                x2 = x1;
                f2 = f1;
                x1 = b - INV_PHI * (b - a);
                f1 = Self::eval_profile_nll(timestamps, values, x1);
            } else {
                a = x1;
                x1 = x2;
                f1 = f2;
                x2 = a + INV_PHI * (b - a);
                f2 = Self::eval_profile_nll(timestamps, values, x2);
            }
        }

        let best_theta = if f1 < f2 { x1 } else { x2 };
        let (mu, sigma_sq) = Self::closed_form_mu_sigma(timestamps, values, best_theta)?;
        if !mu.is_finite() || !sigma_sq.is_finite() || sigma_sq <= 0.0 {
            return None;
        }

        let sigma = sigma_sq.sqrt();
        let half_life = LN_2 / best_theta;
        let stationary_std = sigma / (2.0 * best_theta).sqrt();

        // Calculate R^2 goodness of fit on one-step ahead conditional predictions
        let mut rss = 0.0;
        let mut sum_y = 0.0;
        for i in 0..(n - 1) {
            let dt = timestamps[i + 1] - timestamps[i];
            let decay = (-best_theta * dt).exp();
            let pred = values[i] * decay + mu * (1.0 - decay);
            let err = values[i + 1] - pred;
            rss += err * err;
            sum_y += values[i + 1];
        }
        let m = (n - 1) as f64;
        let y_mean = sum_y / m;
        let mut tss = 0.0;
        for val in &values[1..n] {
            let d = *val - y_mean;
            tss += d * d;
        }
        let r_squared = if tss > 1e-15 {
            Some((1.0 - rss / tss).max(0.0))
        } else {
            None
        };

        Some(OuModelParams {
            theta: best_theta,
            mu,
            sigma,
            half_life,
            stationary_std,
            r_squared,
        })
    }

    fn closed_form_mu_sigma(t: &[f64], x: &[f64], theta: f64) -> Option<(f64, f64)> {
        let n = x.len() - 1;
        let mut sum_w_num = 0.0;
        let mut sum_w_den = 0.0;

        for i in 0..n {
            let dt = t[i + 1] - t[i];
            if dt <= 0.0 {
                return None;
            }
            let a_i = (-theta * dt).exp();
            let v_tilde = (1.0 - a_i * a_i) / (2.0 * theta);
            if v_tilde <= 0.0 {
                return None;
            }
            let u_i = 1.0 - a_i;

            sum_w_num += (u_i * (x[i + 1] - a_i * x[i])) / v_tilde;
            sum_w_den += (u_i * u_i) / v_tilde;
        }

        if sum_w_den.abs() < 1e-15 {
            return None;
        }
        let mu = sum_w_num / sum_w_den;

        let mut sse = 0.0;
        for i in 0..n {
            let dt = t[i + 1] - t[i];
            let a_i = (-theta * dt).exp();
            let v_tilde = (1.0 - a_i * a_i) / (2.0 * theta);
            let pred = mu + (x[i] - mu) * a_i;
            let res = x[i + 1] - pred;
            sse += (res * res) / v_tilde;
        }

        let sigma_sq = sse / (n as f64);
        Some((mu, sigma_sq))
    }

    fn eval_profile_nll(t: &[f64], x: &[f64], theta: f64) -> f64 {
        let (mu, sigma_sq) = match Self::closed_form_mu_sigma(t, x, theta) {
            Some(res) => res,
            None => return 1e12,
        };
        if sigma_sq <= 0.0 || !sigma_sq.is_finite() || !mu.is_finite() {
            return 1e12;
        }

        let n = (x.len() - 1) as f64;
        let mut sum_log_v = 0.0;
        for i in 0..(x.len() - 1) {
            let dt = t[i + 1] - t[i];
            let a_i = (-theta * dt).exp();
            let v_tilde = (1.0 - a_i * a_i) / (2.0 * theta);
            if v_tilde <= 0.0 {
                return 1e12;
            }
            sum_log_v += v_tilde.ln();
        }

        0.5 * n * (2.0 * std::f64::consts::PI).ln()
            + 0.5 * n
            + 0.5 * n * sigma_sq.ln()
            + 0.5 * sum_log_v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ou_fit_synthetic_series() {
        // Generate discrete synthetic OU series: dX = 2.0 * (10.0 - X) dt + 1.0 dW
        let dt: f64 = 0.05;
        let theta_true: f64 = 2.0;
        let mu_true: f64 = 10.0;
        let sigma_true: f64 = 1.0;

        let mut series = Vec::with_capacity(500);
        let mut x: f64 = 10.0;
        series.push(x);

        let decay = (-theta_true * dt).exp();
        let var =
            (sigma_true * sigma_true / (2.0 * theta_true)) * (1.0 - (-2.0 * theta_true * dt).exp());
        let std = var.sqrt();

        // Deterministic LCG + Box-Muller for reproducible pseudo-normal noise
        let mut seed: u64 = 12345;
        for _ in 1..500 {
            seed = (seed.wrapping_mul(1103515245).wrapping_add(12345)) & 0x7fff_ffff;
            let u1 = (seed as f64 + 1.0) / 2147483649.0;
            seed = (seed.wrapping_mul(1103515245).wrapping_add(12345)) & 0x7fff_ffff;
            let u2 = (seed as f64 + 1.0) / 2147483649.0;
            let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();

            let mean = x * decay + mu_true * (1.0 - decay);
            x = mean + z * std;
            series.push(x);
        }

        let params = OuModelParams::fit(&series, dt).expect("fit should succeed");
        assert!(
            params.theta > 0.5 && params.theta < 5.0,
            "theta {}",
            params.theta
        );
        assert!((params.mu - mu_true).abs() < 2.0, "mu {}", params.mu);
        assert!(
            params.sigma > 0.2 && params.sigma < 3.0,
            "sigma {}",
            params.sigma
        );
        assert!(params.half_life > 0.0);
    }

    #[test]
    fn test_ou_fit_irregular_timestamps() {
        // Test non-uniform timestamps
        let mut timestamps = Vec::new();
        let mut values = Vec::new();
        let mut t: f64 = 0.0;
        let mut x: f64 = 5.0;
        let theta_true: f64 = 1.5;
        let mu_true: f64 = 5.0;
        let sigma_true: f64 = 0.8;

        timestamps.push(t);
        values.push(x);

        for i in 1..200 {
            // Irregular interval between 0.02 and 0.25 seconds
            let dt: f64 = 0.02 + ((i as f64 * std::f64::consts::E).cos().abs()) * 0.23;
            t += dt;
            let z = ((i as f64 * std::f64::consts::PI).sin()).clamp(-2.0, 2.0);
            let decay = (-theta_true * dt).exp();
            let mean = x * decay + mu_true * (1.0 - decay);
            let var = (sigma_true * sigma_true / (2.0 * theta_true))
                * (1.0 - (-2.0 * theta_true * dt).exp());
            x = mean + z * var.sqrt();

            timestamps.push(t);
            values.push(x);
        }

        let params = OuModelParams::fit_irregular(&timestamps, &values)
            .expect("fit_irregular should succeed");
        assert!(
            params.theta > 0.2 && params.theta < 5.0,
            "theta {}",
            params.theta
        );
        assert!((params.mu - mu_true).abs() < 1.5, "mu {}", params.mu);
        assert!(params.half_life > 0.0);
        assert!(params.stationary_std > 0.0);
    }

    #[test]
    fn test_ou_fit_irregular_rejects_bad_inputs() {
        assert!(OuModelParams::fit_irregular(&[], &[]).is_none());
        assert!(OuModelParams::fit_irregular(&[1.0; 20], &[1.0; 20]).is_none()); // dt = 0
        let ts = (0..20).map(|i| i as f64).collect::<Vec<_>>();
        let mut vals = vec![1.0; 20];
        vals[5] = f64::NAN;
        assert!(OuModelParams::fit_irregular(&ts, &vals).is_none());
    }

    #[test]
    fn test_ou_fit_with_noise_filters_microstructure_bounce() {
        let mut series = Vec::new();
        let dt: f64 = 0.05;
        let mut x: f64 = 10.0;
        let theta_true: f64 = 2.0;
        let mu_true: f64 = 10.0;
        let sigma_true: f64 = 1.0;
        let noise_std: f64 = 0.25;

        let decay = (-theta_true * dt).exp();
        let step_var =
            (sigma_true * sigma_true / (2.0 * theta_true)) * (1.0 - (-2.0 * theta_true * dt).exp());
        let step_std = step_var.sqrt();

        let lcg = |mut seed: u64| -> (u64, f64) {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let val = ((seed >> 33) as f64 / 2147483648.0) * 2.0 - 1.0;
            (seed, val)
        };

        let mut seed = 42_u64;
        for _ in 0..1000 {
            let (s1, z1) = lcg(seed);
            let (s2, z2) = lcg(s1);
            seed = s2;
            x = x * decay + mu_true * (1.0 - decay) + z1 * step_std;
            series.push(x + z2 * noise_std);
        }

        let (params, noise_var) =
            OuModelParams::fit_with_noise(&series, dt).expect("fit_with_noise should succeed");

        assert!(
            params.theta > 0.5 && params.theta < 6.0,
            "fitted theta: {}",
            params.theta
        );
        assert!(
            (params.mu - mu_true).abs() < 1.0,
            "fitted mu: {}",
            params.mu
        );
        assert!(params.half_life > 0.0);
        assert!(noise_var >= 0.0, "noise var: {}", noise_var);
    }
}
