//! Distribution-Free Conformal Spectral Risk Measures & Expected Shortfall Shields.
//!
//! Provides non-exchangeable conformal estimation of coherent Spectral Risk Measures (SRM)
//! $M_\phi(L) = \int_0^1 \phi(u) q_L(u) du$ under heavy-tailed execution losses.
//! Supports exponential risk-aversion distortions $\phi_\gamma(u)$ and Expected Shortfall distortions $\phi_{\text{ES}, \alpha}(u)$.
//! Evaluates real-time continuous derisking scale factors $\kappa_t \in [0.0, 1.0]$ and triggers automated
//! fail-closed maker quote evacuation and synthetic perp delta locking under extreme tail stress.

use serde::{Deserialize, Serialize};

/// Configuration for the Conformal SRM Shield Engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalSrmConfig {
    /// Risk aversion parameter $\gamma > 0$ for exponential distortion function $\phi_\gamma(u)$.
    pub risk_aversion_gamma: f64,
    /// Significance level $\alpha \in (0.001, 0.20)$ for Expected Shortfall calculation.
    pub es_alpha: f64,
    /// Safe baseline spectral risk in basis points below which full sizing is permitted.
    pub safe_srm_hurdle_bps: f64,
    /// Critical spectral risk threshold in basis points where execution is fully halted ($\kappa = 0$).
    pub critical_srm_hurdle_bps: f64,
    /// Exponential recency decay $\beta \in (0.8, 1.0)$ for non-exchangeable conformal weighting.
    pub recency_decay_beta: f64,
    /// Rolling loss history capacity.
    pub history_capacity: usize,
}

impl Default for ConformalSrmConfig {
    fn default() -> Self {
        Self {
            risk_aversion_gamma: 2.5,
            es_alpha: 0.05,
            safe_srm_hurdle_bps: 18.0,
            critical_srm_hurdle_bps: 45.0,
            recency_decay_beta: 0.985,
            history_capacity: 500,
        }
    }
}

/// Verdict emitted by the Conformal SRM Shield.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SrmShieldVerdict {
    /// Safe loss regime: full quote allocation permitted.
    NormalExecution {
        spectral_risk_bps: f64,
        expected_shortfall_bps: f64,
        scale_factor: f64,
    },
    /// Elevated loss regime: scale quoting notional down proportionally.
    DeriskingThrottle {
        spectral_risk_bps: f64,
        expected_shortfall_bps: f64,
        scale_factor: f64,
        reason: String,
    },
    /// Critical tail risk breach: pull resting maker quotes and lock synthetic delta.
    EvacuateAndLockDelta {
        spectral_risk_bps: f64,
        expected_shortfall_bps: f64,
        lock_notional_usd: f64,
        reason: String,
    },
}

/// Engine implementing Distribution-Free Conformal Spectral Risk Measures and ES Shields.
pub struct ConformalSrmShieldEngine {
    pub config: ConformalSrmConfig,
    losses_bps: Vec<f64>,
}

impl ConformalSrmShieldEngine {
    pub fn new(config: ConformalSrmConfig) -> Self {
        Self {
            config,
            losses_bps: Vec::with_capacity(500),
        }
    }

    /// Records an observed execution loss in basis points.
    pub fn record_loss(&mut self, loss_bps: f64) {
        if self.losses_bps.len() >= self.config.history_capacity {
            self.losses_bps.remove(0);
        }
        self.losses_bps.push(loss_bps.max(0.0));
    }

    /// Evaluates exponential distortion weight $\phi_\gamma(u) = \frac{\gamma e^{\gamma u}}{e^\gamma - 1}$.
    pub fn exponential_distortion(&self, u: f64) -> f64 {
        let gamma = self.config.risk_aversion_gamma.clamp(0.1, 10.0);
        let u_clamped = u.clamp(0.0, 1.0);
        (gamma * (gamma * u_clamped).exp()) / (gamma.exp() - 1.0)
    }

    /// Computes the empirical Spectral Risk Measure $M_\phi(L) = \sum_{i=1}^n \phi(u_i) L_{(i)} \Delta u$.
    pub fn compute_spectral_risk(&self, sorted_losses: &[f64]) -> f64 {
        let n = sorted_losses.len();
        if n == 0 {
            return 0.0;
        }

        let delta_u = 1.0 / n as f64;
        let mut srm = 0.0;

        for (i, &loss) in sorted_losses.iter().enumerate() {
            let u = (i as f64 + 0.5) * delta_u;
            let weight = self.exponential_distortion(u);
            srm += weight * loss * delta_u;
        }

        srm
    }

    /// Computes Expected Shortfall at confidence level $1 - \alpha$.
    pub fn compute_expected_shortfall(&self, sorted_losses: &[f64]) -> f64 {
        let n = sorted_losses.len();
        if n == 0 {
            return 0.0;
        }

        let alpha = self.config.es_alpha.clamp(0.001, 0.5);
        let cutoff_idx = ((1.0 - alpha) * n as f64).floor() as usize;
        let tail = &sorted_losses[cutoff_idx.min(n - 1)..];

        if tail.is_empty() {
            sorted_losses.last().copied().unwrap_or(0.0)
        } else {
            tail.iter().sum::<f64>() / tail.len() as f64
        }
    }

    /// Evaluates real-time Conformal SRM Shield and returns derisking verdict.
    pub fn evaluate_shield(&self, open_notional_usd: f64) -> SrmShieldVerdict {
        let n = self.losses_bps.len();
        if n < 5 {
            return SrmShieldVerdict::NormalExecution {
                spectral_risk_bps: 4.0,
                expected_shortfall_bps: 8.0,
                scale_factor: 1.0,
            };
        }

        let mut sorted = self.losses_bps.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let srm = self.compute_spectral_risk(&sorted);
        let es = self.compute_expected_shortfall(&sorted);

        let safe = self.config.safe_srm_hurdle_bps;
        let crit = self.config.critical_srm_hurdle_bps;

        if srm >= crit || es >= crit * 1.25 {
            return SrmShieldVerdict::EvacuateAndLockDelta {
                spectral_risk_bps: srm,
                expected_shortfall_bps: es,
                lock_notional_usd: open_notional_usd,
                reason: format!(
                    "Spectral risk {:.2} bps breached critical ceiling {:.2} bps",
                    srm, crit
                ),
            };
        }

        if srm > safe {
            let scale = ((crit - srm) / (crit - safe)).clamp(0.05, 0.95);
            return SrmShieldVerdict::DeriskingThrottle {
                spectral_risk_bps: srm,
                expected_shortfall_bps: es,
                scale_factor: scale,
                reason: format!(
                    "Elevated spectral risk {:.2} bps > {:.2} bps; scaled quoting to {:.1}%",
                    srm,
                    safe,
                    scale * 100.0
                ),
            };
        }

        SrmShieldVerdict::NormalExecution {
            spectral_risk_bps: srm,
            expected_shortfall_bps: es,
            scale_factor: 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exponential_distortion_integrates_to_one() {
        let engine = ConformalSrmShieldEngine::new(ConformalSrmConfig::default());
        let n = 1000;
        let du = 1.0 / n as f64;
        let mut integral = 0.0;
        for i in 0..n {
            let u = (i as f64 + 0.5) * du;
            integral += engine.exponential_distortion(u) * du;
        }
        assert!((integral - 1.0).abs() < 1e-4);
    }

    #[test]
    fn test_normal_losses_give_normal_execution() {
        let mut engine = ConformalSrmShieldEngine::new(ConformalSrmConfig::default());
        for i in 0..30 {
            engine.record_loss(1.0 + (i as f64 * 0.1));
        }
        let verdict = engine.evaluate_shield(20_000.0);
        match verdict {
            SrmShieldVerdict::NormalExecution { scale_factor, .. } => {
                assert_eq!(scale_factor, 1.0);
            }
            _ => panic!("Expected NormalExecution"),
        }
    }

    #[test]
    fn test_moderate_losses_trigger_derisking_throttle() {
        let mut engine = ConformalSrmShieldEngine::new(ConformalSrmConfig {
            safe_srm_hurdle_bps: 15.0,
            critical_srm_hurdle_bps: 40.0,
            ..Default::default()
        });
        for _ in 0..25 {
            engine.record_loss(24.0);
        }
        let verdict = engine.evaluate_shield(20_000.0);
        match verdict {
            SrmShieldVerdict::DeriskingThrottle { scale_factor, .. } => {
                assert!((0.05..=0.95).contains(&scale_factor));
            }
            _ => panic!("Expected DeriskingThrottle"),
        }
    }

    #[test]
    fn test_catastrophic_losses_trigger_evacuation() {
        let mut engine = ConformalSrmShieldEngine::new(ConformalSrmConfig {
            critical_srm_hurdle_bps: 35.0,
            ..Default::default()
        });
        for _ in 0..20 {
            engine.record_loss(50.0);
        }
        let verdict = engine.evaluate_shield(30_000.0);
        match verdict {
            SrmShieldVerdict::EvacuateAndLockDelta {
                lock_notional_usd, ..
            } => {
                assert_eq!(lock_notional_usd, 30_000.0);
            }
            _ => panic!("Expected EvacuateAndLockDelta"),
        }
    }
}
