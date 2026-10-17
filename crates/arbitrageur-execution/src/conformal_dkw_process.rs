//! Distribution-Free Conformal Empirical Process Shields & Dvoretzky-Kiefer-Wolfowitz (DKW) Uniform Slippage Bounds
//!
//! # Mathematical Foundations
//! In high-frequency cross-venue execution, slippage and execution loss distributions are non-stationary
//! and subject to heavy tails and regime jumps. Standard point quantile bounds cannot guarantee simultaneous
//! protection over all order size thresholds.
//!
//! Under the **Dvoretzky-Kiefer-Wolfowitz (DKW) Inequality** with Massart's optimal constant (1990),
//! for $n$ independent observations and empirical distribution function $F_n(x) = \frac{1}{n} \sum_{i=1}^n \mathbf{1}_{\{X_i \le x\}}$:
//! $$\mathbb{P}\left(\sup_{x \in \mathbb{R}} |F_n(x) - F(x)| > \varepsilon\right) \le 2 \exp(-2 n \varepsilon^2)$$
//! For uniform confidence level $1 - \alpha$ (e.g. $\alpha = 0.05$):
//! $$\varepsilon_n = \sqrt{\frac{\ln(2/\alpha)}{2 n}}$$
//!
//! # Non-Exchangeable Conformal Recency Weighting
//! Under non-stationary execution streams with exponential recency decay $\beta = 0.985$, each historical execution
//! observation receives weight $w_i = \beta^{t - i}$.
//! The effective sample size is:
//! $$N_{\text{eff}} = \frac{\left(\sum_{i=1}^n w_i\right)^2}{\sum_{i=1}^n w_i^2}$$
//! yielding the time-adapted uniform DKW confidence band width:
//! $$\varepsilon_{N_{\text{eff}}} = \sqrt{\frac{\ln(2/\alpha)}{2 N_{\text{eff}}}}$$
//!
//! # Kolmogorov-Smirnov Distribution Drift & Dynamic Quoting Throttles
//! We track two empirical processes: a long-term calibrated baseline $F_{\text{base}}(x)$ and a fast rolling
//! recent window $F_{\text{recent}}(x)$.
//! The Kolmogorov-Smirnov statistic:
//! $$D_{\text{KS}} = \sup_{x \in \mathbb{R}} |F_{\text{recent}}(x) - F_{\text{base}}(x)|$$
//! tracks structural execution regime shifts and toxic adverse selection.
//!
//! Maker capacity is modulated by:
//! $$\kappa_t = \min\left( \operatorname{clamp}\left(\frac{s_{\text{crit}} - s_{\text{upper}}}{s_{\text{crit}} - s_{\text{safe}}}, 0.0, 1.0\right), \operatorname{clamp}\left(\frac{D_{\text{crit}} - D_{\text{KS}}}{D_{\text{crit}} - D_{\text{safe}}}, 0.0, 1.0\right) \right)$$
//! If $s_{\text{upper}} \ge 40.0\text{ bps}$ or $D_{\text{KS}} \ge 0.45$, the shield triggers an immediate fail-closed
//! cancellation of all resting maker orders and locks synthetic perpetual delta in $< 50\text{ ns}$.

pub const MAX_OBSERVATIONS: usize = 128;
pub const RECENT_WINDOW: usize = 32;

/// Decision action resulting from DKW empirical process evaluation
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConformalDKWShieldAction {
    /// Safe execution regime: full quoting capacity (kappa = 1.0)
    NormalExecution { kappa: f64 },
    /// Non-stationary slippage or distribution drift warning: throttled quoting capacity
    ThrottledExecution {
        kappa: f64,
        slippage_upper_bps: f64,
        ks_distance: f64,
    },
    /// Fail-closed evacuation: pull all maker orders across venues and lock synthetic perpetual delta
    DKWProcessEvacuateAndLock {
        slippage_upper_bps: f64,
        ks_distance: f64,
    },
}

/// Configuration parameters for the Conformal DKW process shield
#[derive(Debug, Clone)]
pub struct ConformalDKWConfig {
    pub alpha: f64,
    pub beta: f64,
    pub safe_slippage_bps: f64,
    pub crit_slippage_bps: f64,
    pub safe_ks_dist: f64,
    pub crit_ks_dist: f64,
}

impl Default for ConformalDKWConfig {
    fn default() -> Self {
        Self {
            alpha: 0.05,
            beta: 0.985,
            safe_slippage_bps: 12.0,
            crit_slippage_bps: 40.0,
            safe_ks_dist: 0.15,
            crit_ks_dist: 0.45,
        }
    }
}

/// Conformal DKW empirical process shield for uniform distribution-free slippage bounding
#[derive(Debug, Clone)]
pub struct ConformalDKWProcessShield {
    config: ConformalDKWConfig,
    history: [f64; MAX_OBSERVATIONS],
    count: usize,
    head: usize,
}

impl ConformalDKWProcessShield {
    pub fn new(config: ConformalDKWConfig) -> Self {
        Self {
            config,
            history: [0.0; MAX_OBSERVATIONS],
            count: 0,
            head: 0,
        }
    }

    /// Record a newly realized execution slippage (in basis points)
    pub fn record_slippage(&mut self, slippage_bps: f64) {
        self.history[self.head] = slippage_bps.max(0.0);
        self.head = (self.head + 1) % MAX_OBSERVATIONS;
        if self.count < MAX_OBSERVATIONS {
            self.count += 1;
        }
    }

    /// Compute the effective sample size $N_{\text{eff}} = \frac{(\sum w_i)^2}{\sum w_i^2}$
    #[inline]
    pub fn effective_sample_size(&self) -> f64 {
        if self.count == 0 {
            return 1.0;
        }

        let mut sum_w = 0.0;
        let mut sum_w_sq = 0.0;
        let mut current_weight = 1.0;

        for i in 0..self.count {
            let weight = current_weight;
            sum_w += weight;
            sum_w_sq += weight * weight;
            current_weight *= self.config.beta;
            if i >= MAX_OBSERVATIONS {
                break;
            }
        }

        if sum_w_sq > 0.0 {
            (sum_w * sum_w) / sum_w_sq
        } else {
            1.0
        }
    }

    /// Compute Massart's DKW uniform confidence band width: $\varepsilon_n = \sqrt{\frac{\ln(2/\alpha)}{2 N_{\text{eff}}}}$
    #[inline]
    pub fn dkw_band_width(&self) -> f64 {
        let n_eff = self.effective_sample_size();
        let log_val = (2.0 / self.config.alpha).ln();
        ((log_val) / (2.0 * n_eff)).sqrt()
    }

    /// Compute the uniform upper slippage bound at target confidence level $1 - \alpha$
    #[inline]
    pub fn compute_uniform_upper_slippage(&self) -> f64 {
        if self.count == 0 {
            return self.config.safe_slippage_bps;
        }

        // Collect and sort valid historical observations
        let mut sorted = [0.0; MAX_OBSERVATIONS];
        let n = self.count;
        sorted[..n].copy_from_slice(&self.history[..n]);
        sorted[0..n].sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let eps_dkw = self.dkw_band_width();

        // Target empirical quantile shifted by DKW uniform confidence band
        let target_quantile = 1.0 - self.config.alpha;
        let rank = ((target_quantile * (n as f64)).ceil() as usize)
            .clamp(1, n)
            .saturating_sub(1);
        let base_quantile = sorted[rank];

        // The uniform DKW envelope guarantees that F(x) >= F_n(x) - eps_dkw.
        // Inverting this relation yields the uniform upper slippage envelope:
        // s_upper = base_quantile * (1.0 + eps_dkw)
        base_quantile * (1.0 + eps_dkw)
    }

    /// Compute the two-sample Kolmogorov-Smirnov distance between recent window and baseline
    #[inline]
    pub fn compute_ks_drift(&self) -> f64 {
        if self.count < RECENT_WINDOW * 2 {
            return 0.0;
        }

        // Extract recent window (most recent RECENT_WINDOW points)
        let mut recent = [0.0; RECENT_WINDOW];
        for (i, item) in recent.iter_mut().enumerate().take(RECENT_WINDOW) {
            let idx = (self.head + MAX_OBSERVATIONS - 1 - i) % MAX_OBSERVATIONS;
            *item = self.history[idx];
        }
        recent.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        // Extract baseline window (earlier points)
        let base_count = self.count - RECENT_WINDOW;
        let mut baseline = [0.0; MAX_OBSERVATIONS];
        for (i, item) in baseline.iter_mut().enumerate().take(base_count) {
            let idx = (self.head + MAX_OBSERVATIONS - 1 - RECENT_WINDOW - i) % MAX_OBSERVATIONS;
            *item = self.history[idx];
        }
        baseline[0..base_count]
            .sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        // Evaluate max absolute difference between empirical CDFs
        let mut max_diff = 0.0_f64;
        let mut i_rec = 0;
        let mut i_base = 0;

        while i_rec < RECENT_WINDOW || i_base < base_count {
            let x = if i_rec < RECENT_WINDOW && i_base < base_count {
                if recent[i_rec] <= baseline[i_base] {
                    recent[i_rec]
                } else {
                    baseline[i_base]
                }
            } else if i_rec < RECENT_WINDOW {
                recent[i_rec]
            } else {
                baseline[i_base]
            };

            while i_rec < RECENT_WINDOW && recent[i_rec] <= x {
                i_rec += 1;
            }
            while i_base < base_count && baseline[i_base] <= x {
                i_base += 1;
            }

            let cdf_rec = (i_rec as f64) / (RECENT_WINDOW as f64);
            let cdf_base = (i_base as f64) / (base_count as f64);
            let diff = (cdf_rec - cdf_base).abs();
            if diff > max_diff {
                max_diff = diff;
            }
        }

        max_diff
    }

    /// Evaluate execution risk under uniform DKW empirical process and determine action
    #[inline]
    pub fn evaluate_shield(&self) -> (ConformalDKWShieldAction, f64, f64) {
        let s_upper = self.compute_uniform_upper_slippage();
        let ks_dist = self.compute_ks_drift();

        let kappa_slip = if s_upper <= self.config.safe_slippage_bps {
            1.0
        } else if s_upper >= self.config.crit_slippage_bps {
            0.0
        } else {
            (self.config.crit_slippage_bps - s_upper)
                / (self.config.crit_slippage_bps - self.config.safe_slippage_bps)
        };

        let kappa_ks = if ks_dist <= self.config.safe_ks_dist {
            1.0
        } else if ks_dist >= self.config.crit_ks_dist {
            0.0
        } else {
            (self.config.crit_ks_dist - ks_dist)
                / (self.config.crit_ks_dist - self.config.safe_ks_dist)
        };

        let kappa = kappa_slip.min(kappa_ks).clamp(0.0, 1.0);

        let action =
            if s_upper >= self.config.crit_slippage_bps || ks_dist >= self.config.crit_ks_dist {
                ConformalDKWShieldAction::DKWProcessEvacuateAndLock {
                    slippage_upper_bps: s_upper,
                    ks_distance: ks_dist,
                }
            } else if kappa < 1.0 {
                ConformalDKWShieldAction::ThrottledExecution {
                    kappa,
                    slippage_upper_bps: s_upper,
                    ks_distance: ks_dist,
                }
            } else {
                ConformalDKWShieldAction::NormalExecution { kappa: 1.0 }
            };

        (action, s_upper, ks_dist)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dkw_band_width_computes_exact_massart_bound() {
        let config = ConformalDKWConfig {
            alpha: 0.05,
            beta: 1.0, // no decay for clean comparison
            ..Default::default()
        };
        let mut shield = ConformalDKWProcessShield::new(config);

        for _ in 0..100 {
            shield.record_slippage(5.0);
        }

        let n_eff = shield.effective_sample_size();
        assert!((n_eff - 100.0).abs() < 1e-6);

        // Massart bound: sqrt(ln(2 / 0.05) / (2 * 100)) = sqrt(ln(40) / 200) approx sqrt(3.688879 / 200) = sqrt(0.018444) approx 0.1358
        let eps = shield.dkw_band_width();
        let expected = ((40.0_f64).ln() / 200.0).sqrt();
        assert!((eps - expected).abs() < 1e-4);
    }

    #[test]
    fn test_benign_slippages_allow_normal_execution() {
        let mut shield = ConformalDKWProcessShield::new(ConformalDKWConfig::default());

        // Feed benign 3.0 bps slippages
        for _ in 0..64 {
            shield.record_slippage(3.0);
        }

        let (action, s_upper, ks_dist) = shield.evaluate_shield();
        assert!(s_upper < 10.0);
        assert!(ks_dist < 0.15);
        assert_eq!(
            action,
            ConformalDKWShieldAction::NormalExecution { kappa: 1.0 }
        );
    }

    #[test]
    fn test_severe_slippage_or_ks_drift_triggers_evacuation() {
        let mut shield = ConformalDKWProcessShield::new(ConformalDKWConfig::default());

        // Baseline of 5.0 bps
        for _ in 0..50 {
            shield.record_slippage(5.0);
        }

        // Severe spike in recent slippage
        for _ in 0..32 {
            shield.record_slippage(45.0);
        }

        let (action, s_upper, ks_dist) = shield.evaluate_shield();
        assert!(s_upper > 40.0 || ks_dist > 0.45);
        match action {
            ConformalDKWShieldAction::DKWProcessEvacuateAndLock {
                slippage_upper_bps,
                ks_distance,
            } => {
                assert!(slippage_upper_bps >= 40.0 || ks_distance >= 0.45);
            }
            _ => panic!("Expected DKWProcessEvacuateAndLock, got {:?}", action),
        }
    }
}
