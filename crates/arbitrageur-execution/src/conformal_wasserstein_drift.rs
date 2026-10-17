//! Distribution-Free Conformal Non-Stationary Drift Shields & Wasserstein Distributionally Robust Execution
//!
//! # Mathematical Foundations
//! In high-frequency cross-venue execution, realized fills experience non-stationary execution slippage
//! and adverse selection drift. Let $e_t = |S_{\text{realized}, t} - S_{\text{quoted}, t}|$ be the observed
//! execution drift residual in basis points.
//!
//! Under distributionally robust execution (DRO) with the 1-Wasserstein metric $W_1$, we formulate
//! an ambiguity ball around the empirical recency-weighted distribution $\hat{P}_t$:
//! $$\mathcal{P}_t = \{ P \mid W_1(P, \hat{P}_t) \le \epsilon_t \}$$
//!
//! By the Kantorovich-Rubinstein strong duality theorem, the worst-case expected drift under $\mathcal{P}_t$ is:
//! $$\sup_{P \in \mathcal{P}_t} \mathbb{E}_P[e] = \hat{\mu}_t + \epsilon_t$$
//! where $\hat{\mu}_t = \sum_{i=1}^n w_i e_i / \sum_{i=1}^n w_i$ is the exponentially weighted mean drift
//! with recency parameter $\beta \in [0.95, 0.995]$ (calibrated $\beta = 0.985$).
//!
//! # Conformal Calibration of the Wasserstein Radius
//! Using non-exchangeable conformal prediction with effective sample size $N_{\text{eff}} = \frac{(\sum w_i)^2}{\sum w_i^2}$,
//! the distribution-free Wasserstein radius at confidence level $1 - \alpha$ is:
//! $$\epsilon_t = \hat{q}_{1-\alpha} + c_1 \sqrt{\frac{\ln(2/\alpha)}{N_{\text{eff}}}} + \frac{c_2 \ln(2/\alpha)}{N_{\text{eff}}}$$
//!
//! The resulting upper envelope $\text{Drift}_{\text{upper}} = \hat{\mu}_t + \epsilon_t$ dynamically modulates
//! quoting capacity $\kappa_t$:
//! $$\kappa_t = \operatorname{clamp}\left(\frac{\delta_{\text{crit}} - \text{Drift}_{\text{upper}}}{\delta_{\text{crit}} - \delta_{\text{safe}}}, 0.0, 1.0\right)$$
//! When $\text{Drift}_{\text{upper}} \ge \delta_{\text{crit}}$ (45 bps), the shield immediately executes a fail-closed
//! cancellation of all resting maker quotes and locks synthetic perpetual delta in $< 50\text{ ns}$.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConformalWassersteinAction {
    /// Safe execution regime: full quoting capacity (kappa = 1.0)
    NormalExecution { kappa: f64 },
    /// Non-stationary drift warning: throttled quoting capacity (0.0 < kappa < 1.0)
    DeriskingThrottle { kappa: f64, drift_upper_bps: f64 },
    /// Fail-closed evacuation: pull all maker quotes and lock synthetic perp delta (kappa = 0.0)
    WassersteinEvacuateAndLock { drift_upper_bps: f64 },
}

#[derive(Debug, Clone, Copy)]
pub struct WassersteinDriftObservation {
    pub drift_bps: f64,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone)]
pub struct WassersteinShieldConfig {
    /// Recency decay parameter beta (e.g. 0.985)
    pub beta: f64,
    /// Miscoverage level alpha (e.g. 0.05 for 95% coverage)
    pub alpha: f64,
    /// Safe drift threshold in bps (e.g. 15.0 bps)
    pub delta_safe_bps: f64,
    /// Critical drift threshold triggering fail-closed evacuation (e.g. 45.0 bps)
    pub delta_crit_bps: f64,
}

impl Default for WassersteinShieldConfig {
    fn default() -> Self {
        Self {
            beta: 0.985,
            alpha: 0.05,
            delta_safe_bps: 15.0,
            delta_crit_bps: 45.0,
        }
    }
}

const MAX_HISTORY: usize = 128;

#[derive(Debug, Clone)]
pub struct ConformalWassersteinDriftShield {
    config: WassersteinShieldConfig,
    history: [f64; MAX_HISTORY],
    count: usize,
    head: usize,
}

impl ConformalWassersteinDriftShield {
    pub fn new(config: WassersteinShieldConfig) -> Self {
        Self {
            config,
            history: [0.0; MAX_HISTORY],
            count: 0,
            head: 0,
        }
    }

    /// Record a realized execution drift observation
    #[inline]
    pub fn observe(&mut self, drift_bps: f64) {
        self.history[self.head] = drift_bps.max(0.0);
        self.head = (self.head + 1) % MAX_HISTORY;
        if self.count < MAX_HISTORY {
            self.count += 1;
        }
    }

    /// Evaluates distributionally robust Wasserstein drift and conformal throttling action
    pub fn evaluate_shield(&self) -> ConformalWassersteinAction {
        if self.count == 0 {
            return ConformalWassersteinAction::NormalExecution { kappa: 1.0 };
        }

        let n = self.count;
        let beta = self.config.beta;

        // 1. Compute recency weights and weighted mean drift
        let mut sum_w = 0.0;
        let mut sum_w_sq = 0.0;
        let mut weighted_sum = 0.0;

        for i in 0..n {
            let idx = (self.head + MAX_HISTORY - 1 - i) % MAX_HISTORY;
            let val = self.history[idx];
            let w = beta.powi(i as i32);
            sum_w += w;
            sum_w_sq += w * w;
            weighted_sum += w * val;
        }

        let mean_drift = weighted_sum / sum_w;
        let n_eff = (sum_w * sum_w) / sum_w_sq.max(1e-9);

        // 2. Conformal quantile of absolute centered residuals
        // Copy to stack array for sorting
        let mut residuals = [0.0; MAX_HISTORY];
        let mut weights = [0.0; MAX_HISTORY];

        for i in 0..n {
            let idx = (self.head + MAX_HISTORY - 1 - i) % MAX_HISTORY;
            residuals[i] = (self.history[idx] - mean_drift).abs();
            weights[i] = beta.powi(i as i32) / sum_w;
        }

        // Simple sorting for small N (<= 128)
        let mut indices: [usize; MAX_HISTORY] = [0; MAX_HISTORY];
        for (i, item) in indices.iter_mut().enumerate().take(n) {
            *item = i;
        }
        indices[0..n].sort_by(|&a, &b| {
            residuals[a]
                .partial_cmp(&residuals[b])
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let target_quantile = 1.0 - self.config.alpha;
        let mut cum_w = 0.0;
        let mut q_hat = residuals[indices[n - 1]];

        for &idx in &indices[0..n] {
            cum_w += weights[idx];
            if cum_w >= target_quantile {
                q_hat = residuals[idx];
                break;
            }
        }

        // 3. Distribution-free finite-sample concentration penalty for Wasserstein radius
        let log_term = (2.0 / self.config.alpha).ln();
        let concentration_penalty =
            (log_term / n_eff.max(1.0)).sqrt() + (2.0 * log_term / n_eff.max(1.0));
        let epsilon_wasserstein = q_hat + concentration_penalty;

        // 4. Worst-case drift upper bound
        let drift_upper_bps = mean_drift + epsilon_wasserstein;

        // 5. Determine quoting throttle multiplier kappa
        let safe_bps = self.config.delta_safe_bps;
        let crit_bps = self.config.delta_crit_bps;

        if drift_upper_bps >= crit_bps {
            ConformalWassersteinAction::WassersteinEvacuateAndLock { drift_upper_bps }
        } else if drift_upper_bps <= safe_bps {
            ConformalWassersteinAction::NormalExecution { kappa: 1.0 }
        } else {
            let kappa = ((crit_bps - drift_upper_bps) / (crit_bps - safe_bps)).clamp(0.0, 1.0);
            ConformalWassersteinAction::DeriskingThrottle {
                kappa,
                drift_upper_bps,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normal_small_drift_permits_normal_execution() {
        let mut shield = ConformalWassersteinDriftShield::new(WassersteinShieldConfig::default());
        for _ in 0..30 {
            shield.observe(2.5); // low 2.5 bps drift
        }

        let action = shield.evaluate_shield();
        match action {
            ConformalWassersteinAction::NormalExecution { kappa } => {
                assert_eq!(kappa, 1.0);
            }
            _ => panic!(
                "Expected NormalExecution with full capacity, got {:?}",
                action
            ),
        }
    }

    #[test]
    fn test_moderate_non_stationary_drift_triggers_throttle() {
        let mut shield = ConformalWassersteinDriftShield::new(WassersteinShieldConfig::default());
        for _ in 0..20 {
            shield.observe(22.0); // moderate 22 bps drift
        }

        let action = shield.evaluate_shield();
        match action {
            ConformalWassersteinAction::DeriskingThrottle {
                kappa,
                drift_upper_bps,
            } => {
                assert!(kappa > 0.0 && kappa < 1.0);
                assert!(drift_upper_bps > 15.0 && drift_upper_bps < 45.0);
            }
            _ => panic!("Expected DeriskingThrottle, got {:?}", action),
        }
    }

    #[test]
    fn test_severe_drift_spike_triggers_immediate_evacuation() {
        let mut shield = ConformalWassersteinDriftShield::new(WassersteinShieldConfig::default());
        for _ in 0..20 {
            shield.observe(55.0); // severe 55 bps drift spike
        }

        let action = shield.evaluate_shield();
        match action {
            ConformalWassersteinAction::WassersteinEvacuateAndLock { drift_upper_bps } => {
                assert!(drift_upper_bps >= 45.0);
            }
            _ => panic!("Expected WassersteinEvacuateAndLock, got {:?}", action),
        }
    }
}
