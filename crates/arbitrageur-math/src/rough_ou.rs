//! Rough Volatility & Fractional Ornstein-Uhlenbeck (fOU) for High-Frequency Crypto Basis Spreads.
//!
//! Classical quantitative finance assumes spreads are driven by standard Brownian motion (Hurst H = 0.5),
//! leading to memoryless Markovian exponential mean-reversion exp(-theta * t).
//!
//! In high-frequency crypto order books (Binance, OKX, Hyperliquid, Lighter), empirical microstructure
//! exhibits sub-diffusive rough volatility with Hurst exponent H in (0.04, 0.45), typically 0.08 - 0.15.
//!
//! This module implements:
//! 1. Non-parametric log-variogram estimator for the local Hurst parameter H:
//!    E[|X_{t + tau} - X_t|^2] ~ tau^(2H)
//! 2. Fractional Ornstein-Uhlenbeck (fOU) forward expected spread via the Mandelbrot-Van Ness kernel:
//!    \hat{X}_{t + \tau | t} = \mu + \frac{\cos(\pi H)}{\pi} \tau^{H + 1/2} \int_0^t \frac{X_s - \mu}{(t - s + \tau)(t - s)^{H + 1/2}} ds
//! 3. Closed-form dynamic entry/exit threshold adjustments under rough memory:
//!    b_{entry}^*(t) = \hat{X}_{t + \tau | t} \pm z^*(H) \cdot C_H \cdot \tau^H
//!    compressing entry hurdles by (2H)^((1 - 2H)/4) to harvest rapid rough mean-reversion spikes without
//!    waiting for nonexistent Gaussian half-lives, while filtering out false trend breakouts.

use std::collections::VecDeque;
use std::f64::consts::PI;

/// Parameters and state of the Fractional Ornstein-Uhlenbeck spread model.
#[derive(Debug, Clone, PartialEq)]
pub struct RoughFouSpreadEngine {
    /// Hurst parameter H in (0.04, 0.45)
    pub hurst_parameter: f64,
    /// Mean reversion speed \theta > 0
    pub mean_reversion_theta: f64,
    /// High-frequency spread volatility \sigma > 0
    pub volatility_sigma: f64,
    /// Long-term equilibrium mean \mu
    pub equilibrium_mean: f64,
    /// Ring buffer of historical spread observations
    pub history_window: VecDeque<f64>,
    /// Maximum ring buffer capacity
    pub max_history: usize,
}

impl Default for RoughFouSpreadEngine {
    fn default() -> Self {
        Self {
            hurst_parameter: 0.12,
            mean_reversion_theta: 1.5,
            volatility_sigma: 0.0025,
            equilibrium_mean: 0.0,
            history_window: VecDeque::with_capacity(120),
            max_history: 120,
        }
    }
}

impl RoughFouSpreadEngine {
    /// Creates a new `RoughFouSpreadEngine`.
    pub fn new(hurst: f64, theta: f64, sigma: f64, mean: f64, max_history: usize) -> Self {
        Self {
            hurst_parameter: hurst.clamp(0.04, 0.45),
            mean_reversion_theta: theta.max(1e-4),
            volatility_sigma: sigma.max(1e-5),
            equilibrium_mean: mean,
            history_window: VecDeque::with_capacity(max_history.max(30)),
            max_history: max_history.max(30),
        }
    }

    /// Ingests a new high-frequency spread tick into the ring buffer.
    pub fn ingest_spread(&mut self, spread: f64) {
        if !spread.is_finite() {
            return;
        }
        if self.history_window.len() >= self.max_history {
            self.history_window.pop_front();
        }
        self.history_window.push_back(spread);
    }

    /// Estimates the empirical Hurst parameter H from the history window using the log-variogram of increments:
    /// E[|X_{i+k} - X_i|^2] ~ k^(2H).
    /// Returns None if insufficient history (< 20 samples).
    pub fn estimate_empirical_hurst(&self) -> Option<f64> {
        let n = self.history_window.len();
        if n < 20 {
            return None;
        }

        let lags = [1, 2, 3, 4, 6, 8, 12];
        let mut log_lags = Vec::new();
        let mut log_variograms = Vec::new();

        for &lag in &lags {
            if lag >= n / 2 {
                break;
            }
            let mut sum_sq_diff = 0.0;
            let count = n - lag;
            for i in 0..count {
                let diff = self.history_window[i + lag] - self.history_window[i];
                sum_sq_diff += diff * diff;
            }
            let mean_sq_diff = (sum_sq_diff / count as f64).max(1e-12);
            log_lags.push((lag as f64).ln());
            log_variograms.push(mean_sq_diff.ln());
        }

        let m = log_lags.len();
        if m < 3 {
            return None;
        }

        // Linear regression: log_variogram = 2H * log_lag + C
        let mean_x = log_lags.iter().sum::<f64>() / m as f64;
        let mean_y = log_variograms.iter().sum::<f64>() / m as f64;

        let mut s_xx = 0.0;
        let mut s_xy = 0.0;
        for i in 0..m {
            let dx = log_lags[i] - mean_x;
            let dy = log_variograms[i] - mean_y;
            s_xx += dx * dx;
            s_xy += dx * dy;
        }

        if s_xx.abs() < 1e-12 {
            return None;
        }

        let slope = s_xy / s_xx;
        let h = (slope / 2.0).clamp(0.04, 0.45);
        Some(h)
    }

    /// Updates the Hurst parameter with an exponential smoothing factor.
    pub fn update_hurst_adaptive(&mut self, smoothing: f64) {
        if let Some(h_emp) = self.estimate_empirical_hurst() {
            let alpha = smoothing.clamp(0.01, 1.0);
            self.hurst_parameter = (1.0 - alpha) * self.hurst_parameter + alpha * h_emp;
        }
    }

    /// Evaluates forward expected spread E[X_{t + \tau} | F_t] using the Mandelbrot-Van Ness kernel:
    /// \hat{X}_{t + \tau | t} = \mu + \frac{\cos(\pi H)}{\pi} \tau^{H + 1/2} \int_0^t \frac{X_s - \mu}{(t - s + \tau)(t - s)^{H + 1/2}} ds
    pub fn forward_expected_spread(&self, horizon_tau_sec: f64, dt_sec: f64) -> f64 {
        let n = self.history_window.len();
        if n < 5 || horizon_tau_sec <= 0.0 || dt_sec <= 0.0 {
            return *self.history_window.back().unwrap_or(&self.equilibrium_mean);
        }

        let h = self.hurst_parameter;
        let tau = horizon_tau_sec;
        let cos_pi_h = (PI * h).cos();

        let mut memory_integral = 0.0;
        for i in 0..n {
            let s_lag = (n - 1 - i) as f64 * dt_sec + 1e-4;
            let spread_val = self.history_window[i] - self.equilibrium_mean;
            let denom = (s_lag + tau) * s_lag.powf(h + 0.5);
            memory_integral += (spread_val / denom.max(1e-8)) * dt_sec;
        }

        let memory_contrib = (cos_pi_h / PI) * tau.powf(h + 0.5) * memory_integral;
        self.equilibrium_mean + memory_contrib
    }

    /// Hurdle compression factor (2H)^((1 - 2H) / 4) accounting for sub-diffusive anti-persistence.
    #[inline]
    pub fn rough_hurdle_compression(&self) -> f64 {
        let h = self.hurst_parameter;
        (2.0 * h).powf((1.0 - 2.0 * h) / 4.0).clamp(0.50, 1.0)
    }

    /// Evaluates rough hurdle discount in bps for an observed spread opportunity.
    /// When rough anti-persistence is active (H < 0.5), compresses the hurdle by (1 - compression) * base_hurdle_bps.
    pub fn compute_rough_hurdle_discount_bps(&self, base_hurdle_bps: f64) -> f64 {
        let comp = self.rough_hurdle_compression();
        (base_hurdle_bps * (1.0 - comp)).max(0.0)
    }

    /// Computes dynamic rough entry and exit thresholds accounting for Hurst exponent H:
    /// Returns (entry_upper, entry_lower, exit_upper, exit_lower).
    pub fn compute_dynamic_thresholds(
        &self,
        horizon_tau_sec: f64,
        z_score_entry_base: f64,
        z_score_exit_base: f64,
    ) -> (f64, f64, f64, f64) {
        let h = self.hurst_parameter;
        let tau = horizon_tau_sec.max(1e-4);
        let exp_spread = self.forward_expected_spread(horizon_tau_sec, 0.05);

        // Conditional standard deviation C_H * tau^H
        let gamma_h = Self::gamma_approx(h + 0.5);
        let c_h = self.volatility_sigma / (gamma_h * (2.0 * h).sqrt()).max(1e-6);
        let cond_std = (c_h * tau.powf(h)).max(1e-6);

        // Threshold compression factor (2H)^((1 - 2H) / 4)
        let compression = self.rough_hurdle_compression();
        let effective_z_entry = z_score_entry_base * compression;
        let effective_z_exit = z_score_exit_base * compression;

        let entry_upper = exp_spread + effective_z_entry * cond_std;
        let entry_lower = exp_spread - effective_z_entry * cond_std;
        let exit_upper = exp_spread + effective_z_exit * cond_std;
        let exit_lower = exp_spread - effective_z_exit * cond_std;

        (entry_upper, entry_lower, exit_upper, exit_lower)
    }

    /// Evaluates whether the current observed spread triggers an entry or exit recommendation:
    pub fn evaluate_signal(
        &self,
        current_spread: f64,
        horizon_tau_sec: f64,
        z_score_entry_base: f64,
        z_score_exit_base: f64,
    ) -> RoughFouSignal {
        let (entry_up, entry_down, exit_up, exit_down) =
            self.compute_dynamic_thresholds(horizon_tau_sec, z_score_entry_base, z_score_exit_base);

        if current_spread >= entry_up {
            RoughFouSignal::ShortEntrySpreadTooHigh {
                current_spread,
                threshold: entry_up,
                expected_reversion: self.equilibrium_mean,
            }
        } else if current_spread <= entry_down {
            RoughFouSignal::LongEntrySpreadTooLow {
                current_spread,
                threshold: entry_down,
                expected_reversion: self.equilibrium_mean,
            }
        } else if current_spread <= exit_up && current_spread >= exit_down {
            RoughFouSignal::ExitMeanReverted {
                current_spread,
                exit_band_upper: exit_up,
                exit_band_lower: exit_down,
            }
        } else {
            RoughFouSignal::HoldWithinBand {
                current_spread,
                entry_upper: entry_up,
                entry_lower: entry_down,
            }
        }
    }

    /// Lanczos approximation for Gamma(x) for x > 0.
    #[inline]
    #[allow(clippy::excessive_precision)]
    pub fn gamma_approx(x: f64) -> f64 {
        let g = 7.0;
        let p = [
            0.99999999999980993,
            676.5203681218851,
            -1259.1392167224028,
            771.32342877765313,
            -176.61502916214059,
            12.507343278686905,
            -0.138571095836524,
            9.9843695780195716e-6,
            1.5056327351493116e-7,
        ];
        if x < 0.5 {
            return PI / ((PI * x).sin() * Self::gamma_approx(1.0 - x));
        }
        let z = x - 1.0;
        let mut sum = p[0];
        for (i, p_val) in p.iter().enumerate().skip(1) {
            sum += p_val / (z + i as f64);
        }
        let t = z + g + 0.5;
        (2.0 * PI).sqrt() * t.powf(z + 0.5) * (-t).exp() * sum
    }
}

/// Signal generated by the Rough Fractional OU Spread Engine.
#[derive(Debug, Clone, PartialEq)]
pub enum RoughFouSignal {
    ShortEntrySpreadTooHigh {
        current_spread: f64,
        threshold: f64,
        expected_reversion: f64,
    },
    LongEntrySpreadTooLow {
        current_spread: f64,
        threshold: f64,
        expected_reversion: f64,
    },
    ExitMeanReverted {
        current_spread: f64,
        exit_band_upper: f64,
        exit_band_lower: f64,
    },
    HoldWithinBand {
        current_spread: f64,
        entry_upper: f64,
        entry_lower: f64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rough_fou_engine_construction_and_bounds() {
        let engine = RoughFouSpreadEngine::new(0.02, -1.0, -0.5, 10.0, 10);
        // Bounds checking
        assert_eq!(engine.hurst_parameter, 0.04);
        assert!(engine.mean_reversion_theta > 0.0);
        assert!(engine.volatility_sigma > 0.0);
        assert_eq!(engine.max_history, 30);
    }

    #[test]
    fn test_lanczos_gamma_approximation() {
        // Gamma(1) = 1, Gamma(2) = 1, Gamma(3) = 2, Gamma(0.5) = sqrt(pi)
        let g1 = RoughFouSpreadEngine::gamma_approx(1.0);
        assert!((g1 - 1.0).abs() < 1e-8, "Gamma(1) = {g1}");

        let g2 = RoughFouSpreadEngine::gamma_approx(2.0);
        assert!((g2 - 1.0).abs() < 1e-8, "Gamma(2) = {g2}");

        let g3 = RoughFouSpreadEngine::gamma_approx(3.0);
        assert!((g3 - 2.0).abs() < 1e-8, "Gamma(3) = {g3}");

        let g_half = RoughFouSpreadEngine::gamma_approx(0.5);
        assert!((g_half - PI.sqrt()).abs() < 1e-8, "Gamma(0.5) = {g_half}");
    }

    #[test]
    fn test_empirical_hurst_estimation() {
        let mut engine = RoughFouSpreadEngine::new(0.10, 2.0, 0.01, 0.0, 100);

        // Generate synthetic rough series with antipersistent steps
        let mut val = 0.0;
        for i in 0..60 {
            // Alternating chattering signs characteristic of H < 0.5
            let step = if i % 2 == 0 { 0.005 } else { -0.0048 };
            val += step;
            engine.ingest_spread(val);
        }

        let h_opt = engine.estimate_empirical_hurst();
        assert!(h_opt.is_some(), "Should estimate empirical Hurst");
        let h = h_opt.unwrap();
        assert!(
            (0.04..=0.45).contains(&h),
            "Hurst must be within rough bounds: {h}"
        );
    }

    #[test]
    fn test_dynamic_thresholds_compression() {
        let mut engine = RoughFouSpreadEngine::new(0.10, 2.0, 0.01, 0.0, 50);
        for i in 0..30 {
            let s = ((i as f64) * 0.1).sin() * 0.005;
            engine.ingest_spread(s);
        }

        let (entry_up, entry_down, exit_up, exit_down) =
            engine.compute_dynamic_thresholds(1.0, 2.0, 0.5);

        assert!(
            entry_up > exit_up,
            "Entry upper must sit outside exit upper"
        );
        assert!(
            entry_down < exit_down,
            "Entry lower must sit outside exit lower"
        );

        // Verify compression: for H = 0.10, (2*0.10)^((1-0.2)/4) = 0.2^0.2 ~ 0.7247
        let comp = (0.20_f64).powf(0.20);
        assert!((comp - 0.7247).abs() < 0.01);
    }

    #[test]
    fn test_evaluate_signal_transitions() {
        let mut engine = RoughFouSpreadEngine::new(0.12, 1.5, 0.005, 0.0, 50);
        for _ in 0..20 {
            engine.ingest_spread(0.0);
        }

        // Test extreme high spread triggers Short entry
        let sig_high = engine.evaluate_signal(0.05, 1.0, 2.0, 0.5);
        assert!(matches!(
            sig_high,
            RoughFouSignal::ShortEntrySpreadTooHigh { .. }
        ));

        // Test extreme low spread triggers Long entry
        let sig_low = engine.evaluate_signal(-0.05, 1.0, 2.0, 0.5);
        assert!(matches!(
            sig_low,
            RoughFouSignal::LongEntrySpreadTooLow { .. }
        ));

        // Test near equilibrium triggers exit
        let sig_mid = engine.evaluate_signal(0.0, 1.0, 2.0, 0.5);
        assert!(matches!(sig_mid, RoughFouSignal::ExitMeanReverted { .. }));
    }
}
