//! Regime-switching and sequential change-point filters for funding rates
//! and spread processes.
//!
//! Includes:
//! 1. `CusumChangeDetector`: Minimax-optimal sequential change-point detection
//!    (Page 1954, Basseville & Nikiforov 1993) for detecting sudden sign-flips
//!    and carry collapse with minimal latency.
//! 2. `HamiltonMarkovFilter`: 2-state Markov regime-switching filter (Hamilton 1989)
//!    for estimating the posterior probability of being in a persistent healthy
//!    carry regime vs a turbulent/collapsing regime.

use std::f64::consts::PI;

/// Two-sided / downward cumulative sum (CUSUM) change-point detector.
///
/// Detects downward shifts from a target mean $\mu_0$ (healthy positive carry)
/// to an alternative mean $\mu_1 \le 0$ (collapse/inversion).
#[derive(Debug, Clone, PartialEq)]
pub struct CusumChangeDetector {
    /// Reference slack value k = (mu_healthy + mu_collapse) / 2.
    slack_k: f64,
    /// Decision boundary threshold h = threshold_mult * sigma.
    threshold_h: f64,
    /// Current accumulated downward CUSUM statistic S_t^-.
    statistic: f64,
}

impl CusumChangeDetector {
    /// Creates a new CUSUM detector.
    ///
    /// # Arguments
    /// - `mu_healthy`: Expected mean under healthy regime (e.g. +0.0004 per 8h, ~43% APR).
    /// - `mu_collapse`: Expected mean under inverted/collapsing regime (e.g. -0.0001 per 8h).
    /// - `sigma`: Standard deviation of the series.
    /// - `threshold_mult`: Multiplier on sigma for alarm threshold (typically 3.0 to 5.0).
    pub fn new(mu_healthy: f64, mu_collapse: f64, sigma: f64, threshold_mult: f64) -> Self {
        let sig = if sigma.is_finite() && sigma > 1e-12 {
            sigma
        } else {
            1e-4
        };
        let k = (mu_healthy + mu_collapse) / 2.0;
        let h = (threshold_mult.max(1.0)) * sig;
        Self {
            slack_k: k,
            threshold_h: h,
            statistic: 0.0,
        }
    }

    /// Default constructor tuned for annualized crypto funding rates (in 8h fractional units).
    ///
    /// Healthy target: +15% APR (~0.000137 per 8h)
    /// Collapse target: 0.0% APR
    pub fn default_funding_detector() -> Self {
        // 15% APR / 365 / 3 = 0.000137 per 8h
        let mu_h = 0.000137;
        let mu_c = 0.0;
        let sigma = 0.0002;
        Self::new(mu_h, mu_c, sigma, 4.0)
    }

    /// Ingests a new observation. Returns `true` if a downward regime shift is detected.
    pub fn update(&mut self, observation: f64) -> bool {
        self.update_scaled(observation, 1.0)
    }

    /// Ingests a new observation scaled by time step duration `dt_units` (in units of calibrated periods, e.g. 8h).
    pub fn update_scaled(&mut self, observation: f64, dt_units: f64) -> bool {
        if !observation.is_finite() || !dt_units.is_finite() || dt_units <= 0.0 {
            return false;
        }

        // S_t^- = max(0, S_{t-1}^- + (k - x_t) * dt)
        // Downward deviation (x_t < k) increments the statistic scaled by time elapsed.
        let inc = (self.slack_k - observation) * dt_units;
        self.statistic = (self.statistic + inc).max(0.0);

        self.statistic >= self.threshold_h
    }

    /// Resets the accumulated CUSUM statistic to zero.
    pub fn reset(&mut self) {
        self.statistic = 0.0;
    }

    /// Returns the current accumulated CUSUM statistic.
    pub fn current_statistic(&self) -> f64 {
        self.statistic
    }

    /// Returns the decision boundary threshold.
    pub fn threshold(&self) -> f64 {
        self.threshold_h
    }
}

/// Recursive 2-state Hamilton Markov regime-switching filter.
///
/// Evaluates whether the observed series is currently in:
/// - State 1: Healthy carry regime ($\mu_1 > 0$, $\sigma_1$)
/// - State 2: Collapse/turbulence regime ($\mu_2 \le 0$, $\sigma_2 > \sigma_1$)
#[derive(Debug, Clone, PartialEq)]
pub struct HamiltonMarkovFilter {
    /// Transition probability P(S_t = 1 | S_{t-1} = 1).
    pub p11: f64,
    /// Transition probability P(S_t = 2 | S_{t-1} = 2).
    pub p22: f64,
    /// State 1 (healthy) mean.
    pub mu1: f64,
    /// State 1 volatility.
    pub sigma1: f64,
    /// State 2 (collapse) mean.
    pub mu2: f64,
    /// State 2 volatility.
    pub sigma2: f64,
    /// Posterior probability of State 1 given observations through step t.
    pub prob_healthy_state: f64,
}

impl HamiltonMarkovFilter {
    /// Creates a new 2-state Hamilton filter.
    pub fn new(p11: f64, p22: f64, mu1: f64, sigma1: f64, mu2: f64, sigma2: f64) -> Self {
        Self {
            p11: p11.clamp(0.5, 0.999),
            p22: p22.clamp(0.5, 0.999),
            mu1,
            sigma1: sigma1.max(1e-6),
            mu2,
            sigma2: sigma2.max(1e-6),
            prob_healthy_state: 0.90, // Start with confident healthy prior
        }
    }

    /// Default constructor tuned for perpetual funding rates.
    pub fn default_funding_filter() -> Self {
        Self::new(
            0.98,    // Persistent healthy state
            0.90,    // Sticky collapse state
            0.0003,  // Healthy mean ~33% APR
            0.00015, // Healthy sigma
            -0.0001, // Collapse mean -11% APR
            0.0005,  // Elevated collapse volatility
        )
    }

    /// Gaussian density evaluation: N(x; mu, sigma^2).
    fn normal_density(x: f64, mu: f64, sigma: f64) -> f64 {
        let diff = (x - mu) / sigma;
        let norm = 1.0 / (sigma * (2.0 * PI).sqrt());
        norm * (-0.5 * diff * diff).exp()
    }

    /// Updates the filter with a new observation and returns posterior P(S_t = 1 | Y_t).
    pub fn update(&mut self, observation: f64) -> f64 {
        if !observation.is_finite() {
            return self.prob_healthy_state;
        }

        // 1. Prediction step: prior probabilities for step t
        // π_{1, t|t-1} = p11 * π_{1, t-1} + (1 - p22) * π_{2, t-1}
        let pi1_prior =
            self.p11 * self.prob_healthy_state + (1.0 - self.p22) * (1.0 - self.prob_healthy_state);
        let pi2_prior = 1.0 - pi1_prior;

        // 2. Emission likelihoods
        let f1 = Self::normal_density(observation, self.mu1, self.sigma1);
        let f2 = Self::normal_density(observation, self.mu2, self.sigma2);

        // 3. Bayes update
        let num1 = pi1_prior * f1;
        let num2 = pi2_prior * f2;
        let denom = num1 + num2;

        if denom > 1e-30 {
            self.prob_healthy_state = (num1 / denom).clamp(0.0001, 0.9999);
        }

        self.prob_healthy_state
    }

    /// Returns the posterior probability of being in the healthy carry regime.
    pub fn prob_healthy(&self) -> f64 {
        self.prob_healthy_state
    }

    /// Checks if the healthy regime probability meets or exceeds a safety threshold.
    pub fn is_healthy(&self, min_probability: f64) -> bool {
        self.prob_healthy_state >= min_probability
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cusum_stays_zero_under_steady_positive_carry() {
        let mut detector = CusumChangeDetector::default_funding_detector();
        for _ in 0..100 {
            let alarm = detector.update(0.0004); // Healthy positive carry
            assert!(!alarm);
            assert_eq!(detector.current_statistic(), 0.0);
        }
    }

    #[test]
    fn cusum_triggers_alarm_on_sudden_collapse() {
        let mut detector = CusumChangeDetector::default_funding_detector();
        // Warm up with positive prints
        detector.update(0.0003);
        detector.update(0.0003);

        // Sudden negative flip
        let mut triggered = false;
        for _ in 0..10 {
            if detector.update(-0.0008) {
                triggered = true;
                break;
            }
        }
        assert!(
            triggered,
            "CUSUM should trigger alarm on sudden negative flip"
        );
    }

    #[test]
    fn hamilton_filter_identifies_carry_regime() {
        let mut filter = HamiltonMarkovFilter::default_funding_filter();
        for _ in 0..20 {
            filter.update(0.00035);
        }
        assert!(filter.prob_healthy() > 0.95);
        assert!(filter.is_healthy(0.80));
    }

    #[test]
    fn hamilton_filter_detects_regime_break() {
        let mut filter = HamiltonMarkovFilter::default_funding_filter();
        for _ in 0..20 {
            filter.update(0.00035);
        }
        assert!(filter.is_healthy(0.80));

        // Regime shift into negative volatile prints
        for _ in 0..5 {
            filter.update(-0.0005);
        }
        assert!(filter.prob_healthy() < 0.20);
        assert!(!filter.is_healthy(0.80));
    }

    #[test]
    fn non_finite_inputs_do_not_panic() {
        let mut cusum = CusumChangeDetector::default_funding_detector();
        assert!(!cusum.update(f64::NAN));
        assert!(!cusum.update(f64::INFINITY));

        let mut hamilton = HamiltonMarkovFilter::default_funding_filter();
        let prev = hamilton.prob_healthy();
        assert_eq!(hamilton.update(f64::NAN), prev);
    }
}
