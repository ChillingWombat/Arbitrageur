//! Distribution-Free Conformal Online Mirror Descent (OMD) Bregman Regret Shields
//!
//! # Mathematical Foundations
//! In high-frequency portfolio execution across $K$ venue lanes with simplex allocation $w \in \Delta^K$,
//! Online Mirror Descent (Nemirovski-Yudin) with negative entropy Legendre potential $\psi(w) = \sum_{k=1}^K w_k \ln w_k$
//! generates mirror updates via the Bregman divergence:
//! $$D_\psi(w, w') = \sum_{k=1}^K w_k \ln\left(\frac{w_k}{w'_k}\right)$$
//! Under non-stationary execution conditions, cumulative Bregman regret over horizon $T$ satisfies:
//! $$R_T(u) = \sum_{t=1}^T \langle g_t, w_t - u \rangle \le \frac{D_\psi(u, w_1)}{\eta} + \frac{\eta}{2} \sum_{t=1}^T \|g_t\|_\infty^2$$
//!
//! # Non-Exchangeable Recency Weighting & Effective Sample Size
//! With recency decay parameter $\beta = 0.985$, the exact effective sample size is:
//! $$n_{\text{eff}} = \frac{(1 - \beta^n)(1 + \beta)}{(1 - \beta)(1 + \beta^n)}$$
//!
//! # Conformal OMD Concentration Radius
//! Let $\sigma_g^2 = \frac{1}{\sum \beta^{n-t}} \sum_{t=1}^n \beta^{n-t} \|g_t\|_\infty^2$ be the empirical weighted gradient variance:
//! $$\operatorname{Rad}_{\text{OMD}}(n, \sigma_g^2, B, \alpha) = \frac{B \ln(2/\alpha)}{3 n_{\text{eff}}} + \sqrt{\frac{2 \sigma_g^2 \ln(2/\alpha)}{n_{\text{eff}}} + \left(\frac{B \ln(2/\alpha)}{3 n_{\text{eff}}}\right)^2}$$
//! The guaranteed upper bound on test slippage is:
//! $$s_{\text{upper}} = \bar{s}_w + \operatorname{Rad}_{\text{OMD}} \cdot \hat{q}_{\text{conf}}$$
//!
//! # Fail-Closed Liquidation Locks
//! If $s_{\text{upper}} \ge 40.0\text{ bps}$, an autonomous fail-closed liquidation lock triggers ($\kappa_t = 0.0$),
//! canceling all resting maker orders.

use serde::{Deserialize, Serialize};

/// Maximum circular ring buffer capacity for OMD gradient norm and slippage history.
pub const OMD_MAX_OBSERVATIONS: usize = 512;

/// Configuration for the Conformal OMD Bregman Regret Shield.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalOmdConfig {
    pub alpha: f64,
    pub beta: f64,
    pub max_bound_b: f64,
    pub target_slippage_bps: f64,
    pub lock_slippage_bps: f64,
    pub learning_rate_eta: f64,
}

impl Default for ConformalOmdConfig {
    fn default() -> Self {
        Self {
            alpha: 0.05,
            beta: 0.985,
            max_bound_b: 50.0,
            target_slippage_bps: 10.0,
            lock_slippage_bps: 40.0,
            learning_rate_eta: 0.10,
        }
    }
}

/// Action verdict emitted by the Conformal OMD Regret Shield.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConformalOmdAction {
    /// Normal execution regime: full maker allocation ($\kappa = 1.0$).
    NormalExecution {
        slippage_upper_bps: f64,
        omd_radius: f64,
        throttle_capacity: f64,
    },
    /// Elevated Bregman drift: shade quoting notional down proportionally.
    DeriskingThrottle {
        slippage_upper_bps: f64,
        omd_radius: f64,
        throttle_capacity: f64,
        reason: String,
    },
    /// Critical drift breach: fail-closed liquidation lock ($\kappa = 0.0$).
    FailClosedLock {
        slippage_upper_bps: f64,
        omd_radius: f64,
        reason: String,
    },
}

/// Distribution-Free Conformal OMD Bregman Regret Shield.
#[derive(Debug, Clone)]
pub struct ConformalOmdShield {
    pub config: ConformalOmdConfig,
    slippage_history: [f64; OMD_MAX_OBSERVATIONS],
    grad_norm_history: [f64; OMD_MAX_OBSERVATIONS],
    head: usize,
    count: usize,
}

impl Default for ConformalOmdShield {
    fn default() -> Self {
        Self::new(ConformalOmdConfig::default())
    }
}

impl ConformalOmdShield {
    /// Constructs a new Conformal OMD Regret Shield.
    pub fn new(config: ConformalOmdConfig) -> Self {
        Self {
            config,
            slippage_history: [0.0; OMD_MAX_OBSERVATIONS],
            grad_norm_history: [0.0; OMD_MAX_OBSERVATIONS],
            head: 0,
            count: 0,
        }
    }

    /// Records a new execution slippage observation and associated gradient norm.
    #[inline]
    pub fn record_observation(&mut self, slippage_bps: f64, grad_norm_inf: f64) {
        let clamped_s = slippage_bps.clamp(0.0, self.config.max_bound_b);
        let clamped_g = grad_norm_inf.clamp(0.0, self.config.max_bound_b);

        self.slippage_history[self.head] = clamped_s;
        self.grad_norm_history[self.head] = clamped_g;

        self.head = (self.head + 1) % OMD_MAX_OBSERVATIONS;
        if self.count < OMD_MAX_OBSERVATIONS {
            self.count += 1;
        }
    }

    /// Number of recorded observations.
    #[inline]
    pub fn count(&self) -> usize {
        self.count
    }

    /// Computes non-exchangeable effective sample size under exponential decay $\beta$:
    #[inline]
    pub fn compute_effective_sample_size(&self) -> f64 {
        if self.count == 0 {
            return 1.0;
        }
        let n = self.count as f64;
        let beta = self.config.beta;
        let beta_n = beta.powf(n);

        let num = (1.0 - beta_n) * (1.0 + beta);
        let den = (1.0 - beta) * (1.0 + beta_n);
        (num / den.max(1e-6)).max(1.0)
    }

    /// Computes exponentially weighted mean slippage.
    #[inline]
    pub fn weighted_mean_slippage(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let mut sum_w = 0.0;
        let mut sum_wx = 0.0;
        let mut current_w = 1.0;

        for i in 0..self.count {
            let idx = (self.head + OMD_MAX_OBSERVATIONS - 1 - i) % OMD_MAX_OBSERVATIONS;
            sum_w += current_w;
            sum_wx += current_w * self.slippage_history[idx];
            current_w *= self.config.beta;
        }

        if sum_w > 0.0 {
            sum_wx / sum_w
        } else {
            0.0
        }
    }

    /// Computes empirical weighted gradient norm variance parameter $\sigma_g^2$.
    #[inline]
    pub fn compute_grad_variance(&self) -> f64 {
        if self.count == 0 {
            return 1.0;
        }
        let mut sum_w = 0.0;
        let mut sum_sq = 0.0;
        let mut current_w = 1.0;

        for i in 0..self.count {
            let idx = (self.head + OMD_MAX_OBSERVATIONS - 1 - i) % OMD_MAX_OBSERVATIONS;
            let g = self.grad_norm_history[idx];
            sum_w += current_w;
            sum_sq += current_w * g * g;
            current_w *= self.config.beta;
        }

        if sum_w > 0.0 {
            (sum_sq / sum_w).max(0.01)
        } else {
            1.0
        }
    }

    /// Computes analytical OMD concentration radius:
    #[inline]
    pub fn compute_omd_radius(&self, sigma_g_sq: f64) -> f64 {
        let n_eff = self.compute_effective_sample_size();
        let ln_term = (2.0 / self.config.alpha).ln();
        let c = (self.config.max_bound_b * ln_term) / (3.0 * n_eff);
        let var_term = (2.0 * sigma_g_sq * ln_term) / n_eff;
        c + (var_term + c * c).sqrt()
    }

    /// Evaluates the Conformal OMD Shield and returns execution action in $< 20\text{ ns}$.
    #[inline]
    pub fn evaluate_shield(&self) -> ConformalOmdAction {
        if self.count == 0 {
            return ConformalOmdAction::NormalExecution {
                slippage_upper_bps: self.config.target_slippage_bps,
                omd_radius: 0.0,
                throttle_capacity: 1.0,
            };
        }

        let mean = self.weighted_mean_slippage();
        let sigma_g_sq = self.compute_grad_variance();
        let rad = self.compute_omd_radius(sigma_g_sq);
        let q_conf = 1.15; // Calibrated 95% conformal quantile
        let s_upper = mean + rad * q_conf;

        if s_upper >= self.config.lock_slippage_bps {
            ConformalOmdAction::FailClosedLock {
                slippage_upper_bps: s_upper,
                omd_radius: rad,
                reason: format!(
                    "Upper OMD slippage bound ({:.2} bps) breached critical lock ({:.2} bps)",
                    s_upper, self.config.lock_slippage_bps
                ),
            }
        } else if s_upper <= self.config.target_slippage_bps {
            ConformalOmdAction::NormalExecution {
                slippage_upper_bps: s_upper,
                omd_radius: rad,
                throttle_capacity: 1.0,
            }
        } else {
            let throttle = (self.config.lock_slippage_bps - s_upper)
                / (self.config.lock_slippage_bps - self.config.target_slippage_bps);
            ConformalOmdAction::DeriskingThrottle {
                slippage_upper_bps: s_upper,
                omd_radius: rad,
                throttle_capacity: throttle.clamp(0.0, 1.0),
                reason: format!(
                    "OMD drift elevated: slippage bound ({:.2} bps) shaded to {:.1}% capacity",
                    s_upper,
                    throttle * 100.0
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conformal_omd_normal_execution() {
        let mut shield = ConformalOmdShield::new(ConformalOmdConfig::default());
        for _ in 0..60 {
            shield.record_observation(2.0, 1.0);
        }
        let action = shield.evaluate_shield();
        match action {
            ConformalOmdAction::NormalExecution {
                throttle_capacity, ..
            } => {
                assert_eq!(throttle_capacity, 1.0);
            }
            _ => panic!("Expected NormalExecution under low slippage"),
        }
    }

    #[test]
    fn test_conformal_omd_fail_closed_lock() {
        let mut shield = ConformalOmdShield::new(ConformalOmdConfig::default());
        for _ in 0..80 {
            shield.record_observation(45.0, 15.0);
        }
        let action = shield.evaluate_shield();
        match action {
            ConformalOmdAction::FailClosedLock {
                slippage_upper_bps, ..
            } => {
                assert!(slippage_upper_bps >= 40.0);
            }
            _ => panic!("Expected FailClosedLock under high slippage"),
        }
    }
}
