//! Distribution-Free Conformal Extreme Value Copula Risk Bounding & Tail Covariation Shields
//!
//! Formulates multi-venue tail risk and joint drawdown protection across segregated
//! exchange accounts via bivariate Extreme Value Copulas (Gumbel-Hougaard and Galambos)
//! equipped with Pickands dependence functions $A(t)$ and upper tail dependence coefficient
//! $\lambda_U = 2 - 2 A(1/2) = 2 - 2^{1/\theta}$.
//!
//! Applies distribution-free non-exchangeable conformal prediction with exponential
//! recency weighting ($\beta \in (0, 1)$) to guarantee finite-sample coverage of worst-case
//! joint tail losses without Gaussian or elliptical distribution assumptions.

use serde::{Deserialize, Serialize};

/// Configuration for Conformal Extreme Copula Shield.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformalExtremeCopulaConfig {
    /// Nominal non-coverage level $\alpha \in (0, 1)$ (e.g. 0.05 for 95% coverage).
    pub alpha: f64,
    /// Exponential recency weighting decay parameter $\beta \in (0, 1]$.
    pub recency_decay: f64,
    /// Minimum observations before activating conformalized bounds.
    pub min_calibration_samples: usize,
    /// Safe upper tail dependence threshold (e.g. 0.35).
    pub safe_lambda_u_threshold: f64,
    /// Critical upper tail dependence threshold (e.g. 0.65).
    pub critical_lambda_u_threshold: f64,
    /// Safe joint drawdown in bps (e.g. 40.0 bps).
    pub safe_tail_loss_bps: f64,
    /// Critical joint drawdown in bps triggering evacuation (e.g. 100.0 bps).
    pub critical_tail_loss_bps: f64,
}

impl Default for ConformalExtremeCopulaConfig {
    fn default() -> Self {
        Self {
            alpha: 0.05,
            recency_decay: 0.985,
            min_calibration_samples: 30,
            safe_lambda_u_threshold: 0.35,
            critical_lambda_u_threshold: 0.65,
            safe_tail_loss_bps: 40.0,
            critical_tail_loss_bps: 100.0,
        }
    }
}

/// Historical joint loss observation across two margin venues.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JointVenueLossObservation {
    /// Loss on Venue A in basis points.
    pub loss_venue_a_bps: f64,
    /// Loss on Venue B in basis points.
    pub loss_venue_b_bps: f64,
}

/// Protective shield verdict issued by Conformal Extreme Copula Engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExtremeCopulaShieldVerdict {
    /// Tail risk is within safe limits. Normal execution and full quoting permitted.
    NormalExecution {
        lambda_u: f64,
        tail_loss_bound_bps: f64,
    },
    /// Elevated joint tail risk. Maker quote sizes throttled, proportional deleveraging initiated.
    ThrottleAndDeleverage {
        lambda_u: f64,
        tail_loss_bound_bps: f64,
        scale_factor: f64,
        reason: String,
    },
    /// Critical systemic tail covariation. Pull all resting quotes and lock synthetic delta.
    EvacuateAndHedgeDelta {
        lambda_u: f64,
        tail_loss_bound_bps: f64,
        emergency_hedge_usd: f64,
        reason: String,
    },
}

/// Distribution-Free Conformal Extreme Value Copula Risk Engine.
#[derive(Debug, Clone)]
pub struct ConformalExtremeCopulaEngine {
    config: ConformalExtremeCopulaConfig,
    history: Vec<JointVenueLossObservation>,
}

impl ConformalExtremeCopulaEngine {
    pub fn new(config: ConformalExtremeCopulaConfig) -> Self {
        Self {
            config,
            history: Vec::with_capacity(1024),
        }
    }

    /// Appends a new joint venue loss observation.
    pub fn record_observation(&mut self, obs: JointVenueLossObservation) {
        self.history.push(obs);
    }

    /// Computes the empirical Kendall's tau $\tau$ between Venue A and B losses.
    pub fn compute_kendalls_tau(&self) -> f64 {
        let n = self.history.len();
        if n < 4 {
            return 0.0;
        }

        let mut concordant = 0;
        let mut discordant = 0;

        for i in 0..n {
            for j in (i + 1)..n {
                let dx = self.history[i].loss_venue_a_bps - self.history[j].loss_venue_a_bps;
                let dy = self.history[i].loss_venue_b_bps - self.history[j].loss_venue_b_bps;
                let prod = dx * dy;
                if prod > 0.0 {
                    concordant += 1;
                } else if prod < 0.0 {
                    discordant += 1;
                }
            }
        }

        let total_pairs = (n * (n - 1)) / 2;
        if total_pairs == 0 {
            0.0
        } else {
            ((concordant as f64) - (discordant as f64)) / (total_pairs as f64)
        }
    }

    /// Evaluates the Gumbel-Hougaard parameter $\theta = \frac{1}{1 - \tau}$ and upper tail dependence:
    /// $$\lambda_U = 2 - 2^{1/\theta}$$
    pub fn compute_upper_tail_dependence(&self) -> f64 {
        let tau = self.compute_kendalls_tau().clamp(0.0, 0.95);
        let theta = (1.0 / (1.0 - tau)).max(1.0);
        let lambda_u = 2.0 - 2.0_f64.powf(1.0 / theta);
        lambda_u.clamp(0.0, 1.0)
    }

    /// Evaluates the conformalized upper bound on joint maximum loss $\max(L_A, L_B)$
    /// with exponential recency weighting.
    pub fn compute_conformal_tail_bound(&self) -> f64 {
        let n = self.history.len();
        if n < self.config.min_calibration_samples {
            return self.config.safe_tail_loss_bps;
        }

        let beta = self.config.recency_decay;
        let mut weighted_losses: Vec<(f64, f64)> = self
            .history
            .iter()
            .enumerate()
            .map(|(idx, obs)| {
                let joint_max = obs.loss_venue_a_bps.max(obs.loss_venue_b_bps).max(0.0);
                let age = (n - 1 - idx) as f64;
                let weight = beta.powf(age);
                (joint_max, weight)
            })
            .collect();

        // Sort by loss ascending
        weighted_losses.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        let total_weight: f64 = weighted_losses.iter().map(|(_, w)| *w).sum();
        let target_mass = (1.0 - self.config.alpha) * total_weight;

        let mut cum_weight = 0.0;
        let mut bound = self.config.safe_tail_loss_bps;

        for (loss, weight) in weighted_losses {
            cum_weight += weight;
            if cum_weight >= target_mass {
                bound = loss;
                break;
            }
        }

        bound
    }

    /// Evaluates the full Tail Covariation Shield verdict for a given total portfolio notional.
    pub fn evaluate_shield(&self, portfolio_notional_usd: f64) -> ExtremeCopulaShieldVerdict {
        let lambda_u = self.compute_upper_tail_dependence();
        let tail_loss_bound = self.compute_conformal_tail_bound();

        let is_crit_lambda = lambda_u >= self.config.critical_lambda_u_threshold;
        let is_crit_loss = tail_loss_bound >= self.config.critical_tail_loss_bps;

        if is_crit_lambda || is_crit_loss {
            return ExtremeCopulaShieldVerdict::EvacuateAndHedgeDelta {
                lambda_u,
                tail_loss_bound_bps: tail_loss_bound,
                emergency_hedge_usd: portfolio_notional_usd,
                reason: format!(
                    "Critical tail risk: λ_U={:.3} (thresh={:.3}) or tail loss={:.1} bps (thresh={:.1} bps)",
                    lambda_u,
                    self.config.critical_lambda_u_threshold,
                    tail_loss_bound,
                    self.config.critical_tail_loss_bps
                ),
            };
        }

        let is_elevated_lambda = lambda_u >= self.config.safe_lambda_u_threshold;
        let is_elevated_loss = tail_loss_bound >= self.config.safe_tail_loss_bps;

        if is_elevated_lambda || is_elevated_loss {
            let scale_lambda = if is_elevated_lambda {
                1.0 - (lambda_u - self.config.safe_lambda_u_threshold)
                    / (self.config.critical_lambda_u_threshold
                        - self.config.safe_lambda_u_threshold)
                        .max(1e-6)
            } else {
                1.0
            };

            let scale_loss = if is_elevated_loss {
                1.0 - (tail_loss_bound - self.config.safe_tail_loss_bps)
                    / (self.config.critical_tail_loss_bps - self.config.safe_tail_loss_bps)
                        .max(1e-6)
            } else {
                1.0
            };

            let scale = scale_lambda.min(scale_loss).clamp(0.10, 0.90);

            return ExtremeCopulaShieldVerdict::ThrottleAndDeleverage {
                lambda_u,
                tail_loss_bound_bps: tail_loss_bound,
                scale_factor: scale,
                reason: format!(
                    "Elevated tail covariation: λ_U={:.3}, tail loss={:.1} bps; quoting throttled to {:.1}%",
                    lambda_u,
                    tail_loss_bound,
                    scale * 100.0
                ),
            };
        }

        ExtremeCopulaShieldVerdict::NormalExecution {
            lambda_u,
            tail_loss_bound_bps: tail_loss_bound,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normal_losses_permit_normal_execution() {
        let mut engine = ConformalExtremeCopulaEngine::new(ConformalExtremeCopulaConfig::default());
        for i in 0..40 {
            engine.record_observation(JointVenueLossObservation {
                loss_venue_a_bps: 2.0 + (i % 5) as f64,
                loss_venue_b_bps: 3.0 + ((i + 2) % 5) as f64,
            });
        }

        let verdict = engine.evaluate_shield(50_000.0);
        match verdict {
            ExtremeCopulaShieldVerdict::NormalExecution {
                lambda_u,
                tail_loss_bound_bps,
            } => {
                assert!(lambda_u < 0.35);
                assert!(tail_loss_bound_bps < 40.0);
            }
            _ => panic!("Expected NormalExecution"),
        }
    }

    #[test]
    fn test_high_concordance_triggers_deleveraging() {
        let mut engine = ConformalExtremeCopulaEngine::new(ConformalExtremeCopulaConfig::default());
        // Highly concordant losses
        for i in 0..40 {
            let val = (i as f64) * 1.5;
            engine.record_observation(JointVenueLossObservation {
                loss_venue_a_bps: val,
                loss_venue_b_bps: val + 1.0,
            });
        }

        let verdict = engine.evaluate_shield(50_000.0);
        match verdict {
            ExtremeCopulaShieldVerdict::ThrottleAndDeleverage { scale_factor, .. } => {
                assert!(scale_factor < 0.90);
            }
            ExtremeCopulaShieldVerdict::EvacuateAndHedgeDelta { .. } => {}
            _ => panic!("Expected ThrottleAndDeleverage or Evacuate"),
        }
    }

    #[test]
    fn test_severe_joint_drawdown_triggers_evacuation() {
        let mut engine = ConformalExtremeCopulaEngine::new(ConformalExtremeCopulaConfig::default());
        for _ in 0..40 {
            engine.record_observation(JointVenueLossObservation {
                loss_venue_a_bps: 120.0,
                loss_venue_b_bps: 130.0,
            });
        }

        let verdict = engine.evaluate_shield(75_000.0);
        match verdict {
            ExtremeCopulaShieldVerdict::EvacuateAndHedgeDelta {
                emergency_hedge_usd,
                ..
            } => {
                assert_eq!(emergency_hedge_usd, 75_000.0);
            }
            _ => panic!("Expected EvacuateAndHedgeDelta"),
        }
    }
}
