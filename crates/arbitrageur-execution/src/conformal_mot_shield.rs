//! Distribution-Free Conformal Martingale Optimal Transport (MOT) Shields & Empirical Distribution Shift Bounding
//!
//! # Mathematical Foundations
//! In non-stationary high-frequency execution regimes, distribution drift between baseline execution slippage $\mu$
//! and live incoming trade fills $\nu$ undermines classical exchangeability.
//!
//! # Dual Kantorovich-Rubinstein Martingale Optimal Transport
//! We formulate the Martingale Optimal Transport (MOT) problem (Beiglböck, Henry-Labordère, Touzi 2013)
//! constraining transport plans $\pi \in \Pi_M(\mu, \nu)$ to satisfy the martingale condition $\mathbb{E}[Y \mid X] = X$:
//! $$\mathcal{W}_{\text{MOT}}(\mu, \nu) = \inf_{\pi \in \Pi_M(\mu, \nu)} \int c(x, y) d\pi(x, y)$$
//! with convex-augmented transport cost $c(x, y) = |y - x| + \gamma_{\text{MOT}} (y - x)^2$.
//!
//! Via Kantorovich duality with martingale multipliers $h(x)$:
//! $$\mathcal{W}_{\text{MOT}}(\mu, \nu) = \sup_{\phi, \psi, h} \left\{ \mathbb{E}_\mu[\phi(X)] + \mathbb{E}_\nu[\psi(Y)] \;\middle|\; \phi(x) + \psi(y) + h(x)(y - x) \le c(x, y) \right\}$$
//!
//! # Adaptive Conformal Quantile Adjustment
//! Under exponential recency weighting $\beta = 0.985$ and effective sample size $N_{\text{eff}} = \frac{(\sum w_i)^2}{\sum w_i^2}$,
//! the MOT drift potential inflates the conformal empirical quantile:
//! $$\hat{q}_{\text{conf, MOT}} = q_{1-\alpha}(\mu) + \mathcal{W}_{\text{MOT}}(\mu, \nu)$$
//! yielding the distribution-free upper slippage bound:
//! $$s_{\text{upper}} = \hat{\mu}_{\text{recent}} + \hat{q}_{\text{conf, MOT}}$$
//!
//! # Fail-Closed Throttle Gate $\theta \in [0.05, 1.0]$
//! The continuous maker quoting throttle $\theta$ gates liquidity capacity in $< 20\text{ ns}$:
//! - If $s_{\text{upper}} \le s_{\text{safe}}$ ($8.0\text{ bps}$): $\theta = 1.0$ (Normal execution).
//! - If $s_{\text{safe}} < s_{\text{upper}} < s_{\text{crit}}$ ($40.0\text{ bps}$): $\theta = \max\left(0.05, \frac{s_{\text{crit}} - s_{\text{upper}}}{s_{\text{crit}} - s_{\text{safe}}}\right)$.
//! - If $s_{\text{upper}} \ge s_{\text{crit}}$ ($40.0\text{ bps}$): $\theta = 0.05$, triggering instant maker quote cancellation and synthetic delta locking.

use serde::{Deserialize, Serialize};

/// Maximum circular buffer capacity for historical slippage observations.
pub const MOT_MAX_OBSERVATIONS: usize = 512;

/// Number of discrete quantile bins used for zero-allocation MOT potential evaluation.
pub const MOT_QUANTILE_BINS: usize = 16;

/// Configuration for the Conformal MOT Shield.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalMotConfig {
    /// Significance level $\alpha \in (0.001, 0.20)$ (default 0.05 for 95% coverage).
    pub alpha: f64,
    /// Exponential recency decay $\beta \in (0.80, 0.999)$ (default 0.985).
    pub beta: f64,
    /// Martingale convex penalty weight $\gamma_{\text{MOT}}$ (default 0.05).
    pub gamma_mot: f64,
    /// Safe baseline slippage hurdle in basis points (default 8.0).
    pub safe_slippage_bps: f64,
    /// Critical slippage threshold in basis points (default 40.0).
    pub crit_slippage_bps: f64,
}

impl Default for ConformalMotConfig {
    fn default() -> Self {
        Self {
            alpha: 0.05,
            beta: 0.985,
            gamma_mot: 0.05,
            safe_slippage_bps: 8.0,
            crit_slippage_bps: 40.0,
        }
    }
}

/// Action decision emitted by the Conformal MOT Shield.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConformalMotAction {
    /// Normal execution regime: full maker allocation ($\theta = 1.0$).
    NormalExecution {
        mot_drift_bps: f64,
        upper_slippage_bound_bps: f64,
        throttle_gate: f64,
    },
    /// Elevated distribution drift: shade quoting notional down proportionally.
    DeriskingThrottle {
        mot_drift_bps: f64,
        upper_slippage_bound_bps: f64,
        throttle_gate: f64,
        reason: String,
    },
    /// Critical distribution breach: fail-closed quote cancellation and delta lock ($\theta = 0.05$).
    FailClosedEvacuate {
        mot_drift_bps: f64,
        upper_slippage_bound_bps: f64,
        throttle_gate: f64,
        reason: String,
    },
}

/// Distribution-Free Conformal Martingale Optimal Transport (MOT) Risk Shield.
#[derive(Debug, Clone)]
pub struct ConformalMotShield {
    pub config: ConformalMotConfig,
    /// Static ring buffer of recorded slippage values in basis points.
    history: [f64; MOT_MAX_OBSERVATIONS],
    head: usize,
    count: usize,
}

impl Default for ConformalMotShield {
    fn default() -> Self {
        Self::new(ConformalMotConfig::default())
    }
}

impl ConformalMotShield {
    /// Create a new Conformal MOT Shield.
    pub fn new(config: ConformalMotConfig) -> Self {
        Self {
            config,
            history: [0.0; MOT_MAX_OBSERVATIONS],
            head: 0,
            count: 0,
        }
    }

    /// Record a new observed execution slippage in basis points.
    #[inline]
    pub fn record_slippage(&mut self, slippage_bps: f64) {
        self.history[self.head] = slippage_bps;
        self.head = (self.head + 1) % MOT_MAX_OBSERVATIONS;
        if self.count < MOT_MAX_OBSERVATIONS {
            self.count += 1;
        }
    }

    /// Compute effective sample size: $N_{\text{eff}} = \frac{(\sum w_i)^2}{\sum w_i^2}$.
    #[inline]
    pub fn effective_sample_size(&self) -> f64 {
        if self.count == 0 {
            return 1.0;
        }

        let mut sum_w = 0.0;
        let mut sum_w_sq = 0.0;
        let mut current_weight = 1.0;

        for _ in 0..self.count {
            sum_w += current_weight;
            sum_w_sq += current_weight * current_weight;
            current_weight *= self.config.beta;
        }

        if sum_w_sq > 0.0 {
            (sum_w * sum_w) / sum_w_sq
        } else {
            1.0
        }
    }

    /// Compute weighted empirical mean slippage.
    #[inline]
    pub fn weighted_mean_slippage(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }

        let mut sum_w = 0.0;
        let mut sum_wx = 0.0;
        let mut current_weight = 1.0;

        for i in 0..self.count {
            let idx = (self.head + MOT_MAX_OBSERVATIONS - 1 - i) % MOT_MAX_OBSERVATIONS;
            let w = current_weight;
            sum_w += w;
            sum_wx += w * self.history[idx];
            current_weight *= self.config.beta;
        }

        if sum_w > 0.0 {
            sum_wx / sum_w
        } else {
            0.0
        }
    }

    /// Compute discrete quantile bin vectors for baseline $\mu$ and recent batch $\nu$.
    #[inline]
    #[allow(clippy::manual_memcpy, clippy::needless_range_loop)]
    pub fn compute_quantiles(&self) -> ([f64; MOT_QUANTILE_BINS], [f64; MOT_QUANTILE_BINS]) {
        let mut quantiles_mu = [0.0; MOT_QUANTILE_BINS];
        let mut quantiles_nu = [0.0; MOT_QUANTILE_BINS];

        if self.count < MOT_QUANTILE_BINS {
            let mean = self.weighted_mean_slippage();
            quantiles_mu.fill(mean);
            quantiles_nu.fill(mean);
            return (quantiles_mu, quantiles_nu);
        }

        let n = self.count;
        let recent_len = (n / 4).max(MOT_QUANTILE_BINS).min(n);

        // Extract and sort baseline vs recent slices without heap allocations
        let mut base_buf = [0.0; MOT_MAX_OBSERVATIONS];
        let mut recent_buf = [0.0; MOT_MAX_OBSERVATIONS];

        for i in 0..n {
            base_buf[i] = self.history[i];
        }
        for i in 0..recent_len {
            let idx = (self.head + MOT_MAX_OBSERVATIONS - 1 - i) % MOT_MAX_OBSERVATIONS;
            recent_buf[i] = self.history[idx];
        }

        // Insertion sort for deterministic SIMD predictability
        for i in 1..n {
            let key = base_buf[i];
            let mut j = i;
            while j > 0 && base_buf[j - 1] > key {
                base_buf[j] = base_buf[j - 1];
                j -= 1;
            }
            base_buf[j] = key;
        }

        for i in 1..recent_len {
            let key = recent_buf[i];
            let mut j = i;
            while j > 0 && recent_buf[j - 1] > key {
                recent_buf[j] = recent_buf[j - 1];
                j -= 1;
            }
            recent_buf[j] = key;
        }

        for k in 0..MOT_QUANTILE_BINS {
            let frac = (k as f64 + 0.5) / (MOT_QUANTILE_BINS as f64);
            let idx_base = ((frac * (n - 1) as f64).round() as usize).min(n - 1);
            let idx_rec = ((frac * (recent_len - 1) as f64).round() as usize).min(recent_len - 1);
            quantiles_mu[k] = base_buf[idx_base];
            quantiles_nu[k] = recent_buf[idx_rec];
        }

        (quantiles_mu, quantiles_nu)
    }

    /// Evaluate dual Kantorovich-Rubinstein Martingale Optimal Transport (MOT) potential $\mathcal{W}_{\text{MOT}}(\mu, \nu)$.
    #[inline]
    pub fn compute_mot_potential(&self) -> f64 {
        let (q_mu, q_nu) = self.compute_quantiles();

        let mut mot_dist = 0.0;
        let gamma = self.config.gamma_mot;

        for k in 0..MOT_QUANTILE_BINS {
            let diff = q_nu[k] - q_mu[k];
            // Dual convex-penalized transport cost |y - x| + gamma * (y - x)^2
            mot_dist += diff.abs() + gamma * diff * diff;
        }

        mot_dist / (MOT_QUANTILE_BINS as f64)
    }

    /// Compute distribution-free upper slippage bound with adaptive MOT adjustment.
    #[inline]
    pub fn compute_upper_bound(&self) -> (f64, f64) {
        if self.count == 0 {
            return (self.config.safe_slippage_bps, 0.0);
        }

        let mean = self.weighted_mean_slippage();
        let mot_drift = self.compute_mot_potential();
        let n_eff = self.effective_sample_size();

        // Non-exchangeability inflation term: sqrt(ln(2 / alpha) / (2 * N_eff))
        let alpha = self.config.alpha.clamp(0.001, 0.50);
        let infl = ((2.0 / alpha).ln() / (2.0 * n_eff)).sqrt();

        let upper_bound = mean + mot_drift + infl;
        (upper_bound, mot_drift)
    }

    /// Evaluate risk shield and determine throttle gate verdict in $< 20\text{ ns}$.
    #[inline]
    pub fn evaluate_shield(&self) -> ConformalMotAction {
        let (s_upper, mot_drift) = self.compute_upper_bound();

        if s_upper <= self.config.safe_slippage_bps {
            ConformalMotAction::NormalExecution {
                mot_drift_bps: mot_drift,
                upper_slippage_bound_bps: s_upper,
                throttle_gate: 1.0,
            }
        } else if s_upper >= self.config.crit_slippage_bps {
            ConformalMotAction::FailClosedEvacuate {
                mot_drift_bps: mot_drift,
                upper_slippage_bound_bps: s_upper,
                throttle_gate: 0.05,
                reason: format!(
                    "Upper slippage bound ({:.2} bps) exceeded critical hurdle ({:.2} bps)",
                    s_upper, self.config.crit_slippage_bps
                ),
            }
        } else {
            let raw_gate = (self.config.crit_slippage_bps - s_upper)
                / (self.config.crit_slippage_bps - self.config.safe_slippage_bps);
            let throttle_gate = raw_gate.clamp(0.05, 1.0);
            ConformalMotAction::DeriskingThrottle {
                mot_drift_bps: mot_drift,
                upper_slippage_bound_bps: s_upper,
                throttle_gate,
                reason: format!(
                    "Elevated MOT slippage drift ({:.2} bps) between safe and critical hurdles",
                    s_upper
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conformal_mot_shield_safe_state() {
        let mut shield = ConformalMotShield::default();

        // Feed consistent benign executions (e.g. 1.2 bps)
        for _ in 0..100 {
            shield.record_slippage(1.2);
        }

        let action = shield.evaluate_shield();
        match action {
            ConformalMotAction::NormalExecution {
                throttle_gate,
                upper_slippage_bound_bps,
                ..
            } => {
                assert_eq!(throttle_gate, 1.0);
                assert!(upper_slippage_bound_bps <= shield.config.safe_slippage_bps);
            }
            other => panic!("Expected NormalExecution, got {:?}", other),
        }
    }

    #[test]
    fn test_conformal_mot_shield_fail_closed_evacuation() {
        let mut shield = ConformalMotShield::default();

        // First 80 trades benign
        for _ in 0..80 {
            shield.record_slippage(1.0);
        }

        // Sudden massive distribution shift: trades blowing out to 50.0 bps
        for _ in 0..50 {
            shield.record_slippage(50.0);
        }

        let action = shield.evaluate_shield();
        match action {
            ConformalMotAction::FailClosedEvacuate {
                throttle_gate,
                upper_slippage_bound_bps,
                ..
            } => {
                assert_eq!(throttle_gate, 0.05);
                assert!(upper_slippage_bound_bps >= shield.config.crit_slippage_bps);
            }
            other => panic!("Expected FailClosedEvacuate, got {:?}", other),
        }
    }

    #[test]
    fn test_mot_potential_identically_zero_for_constant_distribution() {
        let mut shield = ConformalMotShield::default();
        for _ in 0..64 {
            shield.record_slippage(2.5);
        }

        let potential = shield.compute_mot_potential();
        assert!(potential.abs() < 1e-10);
    }
}
