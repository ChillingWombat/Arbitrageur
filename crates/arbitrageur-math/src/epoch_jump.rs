//! Continuous Intraday Funding Jump Risk & Cross-Epoch Arbitrage with Fast Basis Harvest Reversal Guard.
//!
//! Provides production mathematical formulations for:
//! 1. **Compound Poisson Jump-Diffusion for Pre-Epoch Funding Premium**:
//!    $$dF_t = \kappa (\bar{F} - F_t) dt + \sigma_F dW_t + J_t dN_t$$
//!    where $N_t$ is an inhomogeneous Poisson process with settlement-surging jump intensity:
//!    $$\lambda_J(t) = \lambda_0 + \lambda_{\text{epoch}} \exp\left( -\frac{(T_{\text{epoch}} - t)^+}{\tau_{\text{jump}}} \right)$$
//!    and jump magnitude $J \sim \mathcal{N}(\mu_J, \sigma_J^2)$.
//! 2. **Jump-Risk-Adjusted Optimal Pre-Epoch Entry Timing $\Delta t_{\text{enter}}^*$**:
//!    $$\Delta t_{\text{enter}}^* = \left( \frac{2 \alpha \eta}{\gamma \cdot (\sigma_B^2 + \lambda_J(t)(\mu_J^2 + \sigma_J^2)) \cdot S} \right)^{\frac{1}{\alpha + 1}}$$
//!    balancing pre-epoch basis compression against sudden liquidation co-jumps.
//! 3. **Fast Basis Harvest Reversal Guard & Stopping Time $\tau_{\text{exit}}^* \le 35\text{s}$**:
//!    Tracks post-settlement basis reversion velocity $v_{\text{revert}}(t) = \frac{dB}{dt}$ and triggers immediate
//!    market/IOC liquidation when basis erosion exceeds the 25% threshold or when elapsed time hits the 35s ceiling.

use serde::{Deserialize, Serialize};

/// Compound Poisson jump-diffusion parameters for funding rate processes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FundingJumpDiffusionParams {
    /// Mean reversion rate $\kappa > 0$.
    pub kappa: f64,
    /// Long-run equilibrium funding rate $\bar{F}$.
    pub theta_mean: f64,
    /// Continuous diffusion volatility $\sigma_F$.
    pub sigma_diffusion: f64,
    /// Base background jump intensity $\lambda_0$ (jumps per second).
    pub lambda_base: f64,
    /// Pre-epoch jump intensity surge coefficient $\lambda_{\text{epoch}}$.
    pub lambda_epoch_surge: f64,
    /// Pre-epoch surge decay time scale $\tau_{\text{jump}}$ in seconds.
    pub tau_jump_sec: f64,
    /// Mean jump magnitude $\mu_J$.
    pub mu_jump: f64,
    /// Jump magnitude volatility $\sigma_J$.
    pub sigma_jump: f64,
}

impl Default for FundingJumpDiffusionParams {
    fn default() -> Self {
        Self {
            kappa: 0.15,
            theta_mean: 0.0001, // 1 bp per 8h
            sigma_diffusion: 0.00005,
            lambda_base: 0.0001,        // ~0.36 jumps/hour background
            lambda_epoch_surge: 0.0025, // ~9 jumps/hour right before epoch
            tau_jump_sec: 180.0,        // surges in final 3 minutes
            mu_jump: 0.00005,           // 0.5 bps
            sigma_jump: 0.00010,        // 1.0 bps
        }
    }
}

/// Pre-epoch entry evaluation verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreEpochEntryVerdict {
    /// Whether current time is inside the optimal pre-epoch entry execution window.
    pub should_enter_now: bool,
    /// Optimal countdown time in seconds prior to settlement $\Delta t_{\text{enter}}^*$.
    pub optimal_countdown_sec: f64,
    /// Current instantaneous jump intensity $\lambda_J(t)$ in jumps per hour.
    pub jump_intensity_per_hour: f64,
    /// Jump-adjusted total annualized basis volatility.
    pub total_basis_vol_annual: f64,
    /// Expected net edge after roundtrip fees and jump risk penalty (USD).
    pub expected_net_pnl_usd: f64,
    /// True if jump hazard is elevated ($\lambda_J > 3 \times \lambda_0$).
    pub is_jump_hazard_elevated: bool,
    /// True if remaining time until epoch is too short for safe execution ($< 25\text{s}$).
    pub is_too_close_to_epoch: bool,
}

/// Pre-Epoch Funding Settlement Jump Optimizer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreEpochEntryOptimizer {
    pub params: FundingJumpDiffusionParams,
    /// Market impact coefficient $\eta$.
    pub eta_impact: f64,
    /// Impact concavity $\alpha \in (0, 1]$.
    pub alpha_concavity: f64,
    /// Risk aversion coefficient $\gamma$.
    pub gamma_risk_aversion: f64,
    /// Base continuous basis volatility (annualized).
    pub base_basis_vol_annual: f64,
}

impl Default for PreEpochEntryOptimizer {
    fn default() -> Self {
        Self {
            params: FundingJumpDiffusionParams::default(),
            eta_impact: 1.2e-4,
            alpha_concavity: 0.65,
            gamma_risk_aversion: 1e-4,
            base_basis_vol_annual: 0.085,
        }
    }
}

impl PreEpochEntryOptimizer {
    pub fn new(
        params: FundingJumpDiffusionParams,
        eta_impact: f64,
        alpha_concavity: f64,
        gamma_risk_aversion: f64,
        base_basis_vol_annual: f64,
    ) -> Self {
        Self {
            params,
            eta_impact: eta_impact.max(1e-7),
            alpha_concavity: alpha_concavity.clamp(0.10, 1.0),
            gamma_risk_aversion: gamma_risk_aversion.max(1e-7),
            base_basis_vol_annual: base_basis_vol_annual.max(0.001),
        }
    }

    /// Evaluates instantaneous jump intensity $\lambda_J(t)$:
    /// $$\lambda_J(t) = \lambda_0 + \lambda_{\text{epoch}} \exp\left(-\frac{\max(0, T_{\text{epoch}} - t)}{\tau_{\text{jump}}}\right)$$
    pub fn instantaneous_jump_intensity(&self, seconds_to_epoch: f64) -> f64 {
        let t_rem = seconds_to_epoch.max(0.0);
        let surge =
            self.params.lambda_epoch_surge * (-t_rem / self.params.tau_jump_sec.max(1.0)).exp();
        self.params.lambda_base + surge
    }

    /// Computes jump-adjusted total basis variance per second:
    /// $$\sigma_{\text{eff}}^2 = \sigma_{B, \text{sec}}^2 + \lambda_J (\mu_J^2 + \sigma_J^2)$$
    pub fn jump_adjusted_variance_per_sec(&self, seconds_to_epoch: f64) -> f64 {
        let sigma_b_sec = self.base_basis_vol_annual / (365.25 * 86400.0_f64).sqrt();
        let cont_var = sigma_b_sec * sigma_b_sec;

        let lambda_j = self.instantaneous_jump_intensity(seconds_to_epoch);
        let jump_second_moment = self.params.mu_jump * self.params.mu_jump
            + self.params.sigma_jump * self.params.sigma_jump;

        cont_var + lambda_j * jump_second_moment
    }

    /// Computes closed-form jump-adjusted optimal pre-epoch entry countdown $\Delta t_{\text{enter}}^*$:
    /// $$\Delta t_{\text{enter}}^* = \left( \frac{2 \alpha \eta}{\gamma \cdot \sigma_{\text{eff}}^2 \cdot S} \right)^{\frac{1}{\alpha + 1}}$$
    pub fn compute_optimal_entry_countdown(&self, spot_price: f64, seconds_to_epoch: f64) -> f64 {
        let alpha = self.alpha_concavity;
        let eta = self.eta_impact;
        let gamma = self.gamma_risk_aversion;
        let var_eff = self.jump_adjusted_variance_per_sec(seconds_to_epoch);
        let s = spot_price.max(1.0);

        let numerator = 2.0 * alpha * eta;
        let denominator = gamma * var_eff * s;

        if denominator <= 1e-18 {
            return 180.0;
        }

        let tau_star = (numerator / denominator).powf(1.0 / (alpha + 1.0));
        // Bound between 25 seconds and 360 seconds (6 minutes)
        tau_star.clamp(25.0, 360.0)
    }

    /// Evaluates pre-epoch harvest window for execution timing.
    pub fn evaluate_entry_window(
        &self,
        seconds_to_epoch: f64,
        spot_price: f64,
        predicted_settled_rate: f64,
        position_notional_usd: f64,
        roundtrip_fees_usd: f64,
    ) -> PreEpochEntryVerdict {
        let optimal_countdown = self.compute_optimal_entry_countdown(spot_price, seconds_to_epoch);
        let lambda_j = self.instantaneous_jump_intensity(seconds_to_epoch);
        let lambda_j_hourly = lambda_j * 3600.0;

        let total_var_sec = self.jump_adjusted_variance_per_sec(seconds_to_epoch);
        let total_vol_annual = (total_var_sec * 365.25 * 86400.0).sqrt();

        let funding_income_usd = position_notional_usd * predicted_settled_rate;
        // Jump risk penalty = lambda_j * E[|J|] * notional * dt
        let dt_dwell = seconds_to_epoch.clamp(10.0, 120.0);
        let jump_penalty_usd = lambda_j
            * (self.params.mu_jump.abs() + self.params.sigma_jump)
            * position_notional_usd
            * dt_dwell;
        let net_pnl_usd = funding_income_usd - roundtrip_fees_usd - jump_penalty_usd;

        let is_too_close = seconds_to_epoch < 25.0;
        let is_in_entry_window =
            seconds_to_epoch <= (optimal_countdown + 20.0) && !is_too_close && net_pnl_usd > 0.0;
        let is_jump_hazard_elevated = lambda_j > (self.params.lambda_base * 3.0);

        PreEpochEntryVerdict {
            should_enter_now: is_in_entry_window,
            optimal_countdown_sec: optimal_countdown,
            jump_intensity_per_hour: lambda_j_hourly,
            total_basis_vol_annual: total_vol_annual,
            expected_net_pnl_usd: net_pnl_usd,
            is_jump_hazard_elevated,
            is_too_close_to_epoch: is_too_close,
        }
    }
}

/// Action recommended by the Fast Basis Harvest Reversal Guard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FastBasisReversalAction {
    /// Basis has broken the retention barrier or velocity threshold: exit immediately via Market/IOC.
    UrgentExitMarketIOC {
        reason: String,
        retained_dividend_fraction: f64,
        reversion_velocity_bps_sec: f64,
    },
    /// Basis remains healthy and stable: continue holding the cash-and-carry position.
    HoldCarryPosition {
        current_basis_bps: f64,
        elapsed_post_epoch_sec: f64,
    },
    /// Prior to epoch settlement: waiting for funding harvest.
    PreEpochPending { seconds_to_settlement: f64 },
}

/// Configuration for the Fast Basis Harvest Reversal Guard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FastBasisReversalConfig {
    /// Maximum post-epoch stopping time $\tau_{\text{exit}}^* \le 35\text{s}$.
    pub max_post_epoch_dwell_sec: f64,
    /// Allowable fraction of funding dividend that basis erosion may consume before exit (typically 0.25 = 25%).
    pub max_dividend_decay_fraction: f64,
    /// Critical negative basis reversion velocity in bps/sec (e.g. -1.5 bps/sec) triggering urgent exit.
    pub critical_reversion_velocity_bps_sec: f64,
    /// Minimum time window for velocity derivative calculation in seconds.
    pub velocity_smoothing_window_sec: f64,
}

impl Default for FastBasisReversalConfig {
    fn default() -> Self {
        Self {
            max_post_epoch_dwell_sec: 35.0, // Strictly <= 35 seconds
            max_dividend_decay_fraction: 0.25,
            critical_reversion_velocity_bps_sec: -1.50,
            velocity_smoothing_window_sec: 2.0,
        }
    }
}

/// Fast Basis Harvest Reversal Guard.
///
/// Guards against rapid post-settlement basis snapback where crowd unhedging erodes
/// the harvested funding dividend within 35 seconds of epoch settlement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FastBasisHarvestReversalGuard {
    pub config: FastBasisReversalConfig,
    /// Recorded epoch settlement timestamp in milliseconds.
    pub epoch_settlement_ms: Option<i64>,
    /// Basis at settlement timestamp: $B_0 = P_{\text{perp}}(T_{\text{epoch}}) - P_{\text{spot}}(T_{\text{epoch}})$.
    pub settlement_basis_usd: Option<f64>,
    /// Harvested funding dividend per unit: $F_{\text{settled}} \cdot S$.
    pub harvested_funding_dividend_usd: Option<f64>,
    /// Recent basis observations: `(timestamp_ms, basis_usd)`.
    pub recent_observations: Vec<(i64, f64)>,
}

impl FastBasisHarvestReversalGuard {
    pub fn new(config: FastBasisReversalConfig) -> Self {
        Self {
            config,
            epoch_settlement_ms: None,
            settlement_basis_usd: None,
            harvested_funding_dividend_usd: None,
            recent_observations: Vec::with_capacity(64),
        }
    }

    /// Arm the guard with the epoch settlement timestamp and baseline values.
    pub fn arm_at_settlement(
        &mut self,
        epoch_settlement_ms: i64,
        settlement_basis_usd: f64,
        harvested_funding_dividend_usd: f64,
    ) {
        self.epoch_settlement_ms = Some(epoch_settlement_ms);
        self.settlement_basis_usd = Some(settlement_basis_usd);
        self.harvested_funding_dividend_usd = Some(harvested_funding_dividend_usd.max(1e-6));
        self.recent_observations.clear();
        self.recent_observations
            .push((epoch_settlement_ms, settlement_basis_usd));
    }

    /// Records a new post-settlement price basis observation.
    pub fn record_observation(&mut self, timestamp_ms: i64, basis_usd: f64) {
        self.recent_observations.push((timestamp_ms, basis_usd));
        if self.recent_observations.len() > 100 {
            self.recent_observations.remove(0);
        }
    }

    /// Computes instantaneous post-settlement basis reversion velocity $v_{\text{revert}} = \frac{dB}{dt}$ in bps/sec.
    pub fn compute_reversion_velocity_bps_sec(&self, spot_price: f64) -> f64 {
        if self.recent_observations.len() < 2 || spot_price <= 0.0 {
            return 0.0;
        }

        let n = self.recent_observations.len();
        let (t_latest, b_latest) = self.recent_observations[n - 1];

        // Find reference observation roughly `velocity_smoothing_window_sec` ago
        let target_t = t_latest - (self.config.velocity_smoothing_window_sec * 1000.0) as i64;
        let mut ref_obs = self.recent_observations[0];

        for obs in self.recent_observations.iter().rev() {
            if obs.0 <= target_t {
                ref_obs = *obs;
                break;
            }
        }

        let dt_sec = ((t_latest - ref_obs.0) as f64) / 1000.0;
        if dt_sec <= 0.05 {
            return 0.0;
        }

        let delta_b_usd = b_latest - ref_obs.1;
        let delta_b_bps = (delta_b_usd / spot_price) * 10_000.0;

        delta_b_bps / dt_sec
    }

    /// Evaluates post-settlement reversal protection rules and returns the operational action.
    pub fn evaluate_reversal_guard(
        &mut self,
        current_time_ms: i64,
        current_basis_usd: f64,
        spot_price: f64,
    ) -> FastBasisReversalAction {
        let settle_ms = match self.epoch_settlement_ms {
            Some(t) => t,
            None => {
                return FastBasisReversalAction::PreEpochPending {
                    seconds_to_settlement: 0.0,
                }
            }
        };

        if current_time_ms < settle_ms {
            let sec_to_settle = ((settle_ms - current_time_ms) as f64) / 1000.0;
            return FastBasisReversalAction::PreEpochPending {
                seconds_to_settlement: sec_to_settle,
            };
        }

        self.record_observation(current_time_ms, current_basis_usd);

        let elapsed_sec = ((current_time_ms - settle_ms) as f64) / 1000.0;
        let b0 = self.settlement_basis_usd.unwrap_or(current_basis_usd);
        let dividend = self.harvested_funding_dividend_usd.unwrap_or(1.0);

        let current_basis_bps = (current_basis_usd / spot_price.max(1.0)) * 10_000.0;
        let basis_decay_usd = (b0 - current_basis_usd).max(0.0);
        let decay_fraction = basis_decay_usd / dividend;
        let retained_fraction = (1.0 - decay_fraction).clamp(0.0, 1.0);

        let velocity_bps_sec = self.compute_reversion_velocity_bps_sec(spot_price);

        // Check 1: Maximum allowable post-epoch dwell time reached (strictly <= 35 seconds)
        if elapsed_sec >= self.config.max_post_epoch_dwell_sec {
            return FastBasisReversalAction::UrgentExitMarketIOC {
                reason: format!(
                    "Post-epoch maximum dwell time ({:.1}s >= {:.1}s) reached",
                    elapsed_sec, self.config.max_post_epoch_dwell_sec
                ),
                retained_dividend_fraction: retained_fraction,
                reversion_velocity_bps_sec: velocity_bps_sec,
            };
        }

        // Check 2: Basis decay exceeded maximum allowable dividend degradation
        if decay_fraction >= self.config.max_dividend_decay_fraction {
            return FastBasisReversalAction::UrgentExitMarketIOC {
                reason: format!(
                    "Basis decay ({:.1}%) exceeded maximum allowable threshold ({:.1}%)",
                    decay_fraction * 100.0,
                    self.config.max_dividend_decay_fraction * 100.0
                ),
                retained_dividend_fraction: retained_fraction,
                reversion_velocity_bps_sec: velocity_bps_sec,
            };
        }

        // Check 3: Critical negative basis reversion velocity (violent snapback)
        if velocity_bps_sec <= self.config.critical_reversion_velocity_bps_sec {
            return FastBasisReversalAction::UrgentExitMarketIOC {
                reason: format!(
                    "Basis reversion velocity ({:.2} bps/s) breached critical threshold ({:.2} bps/s)",
                    velocity_bps_sec, self.config.critical_reversion_velocity_bps_sec
                ),
                retained_dividend_fraction: retained_fraction,
                reversion_velocity_bps_sec: velocity_bps_sec,
            };
        }

        FastBasisReversalAction::HoldCarryPosition {
            current_basis_bps,
            elapsed_post_epoch_sec: elapsed_sec,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jump_intensity_pre_epoch_surge() {
        let optimizer = PreEpochEntryOptimizer::default();
        let lambda_far = optimizer.instantaneous_jump_intensity(3600.0); // 1h before
        let lambda_near = optimizer.instantaneous_jump_intensity(30.0); // 30s before

        assert!(
            lambda_near > lambda_far * 10.0,
            "Jump intensity should surge near epoch"
        );
        let vol_far = optimizer.jump_adjusted_variance_per_sec(3600.0);
        let vol_near = optimizer.jump_adjusted_variance_per_sec(30.0);
        assert!(vol_near > vol_far, "Variance must incorporate jump surges");
    }

    #[test]
    fn test_optimal_entry_countdown_bounds() {
        let optimizer = PreEpochEntryOptimizer::default();
        let tau_star = optimizer.compute_optimal_entry_countdown(60_000.0, 120.0);
        assert!(
            (25.0..=360.0).contains(&tau_star),
            "Optimal countdown {} must be clamped to safe operational limits",
            tau_star
        );

        let verdict = optimizer.evaluate_entry_window(
            90.0, 60_000.0, 0.0003, // 3 bps funding
            100_000.0, 15.0,
        );
        assert!(verdict.expected_net_pnl_usd > 0.0);
    }

    #[test]
    fn test_fast_basis_reversal_guard_stopping_time() {
        let config = FastBasisReversalConfig {
            max_post_epoch_dwell_sec: 35.0,
            max_dividend_decay_fraction: 0.25,
            critical_reversion_velocity_bps_sec: -1.5,
            velocity_smoothing_window_sec: 2.0,
        };
        let mut guard = FastBasisHarvestReversalGuard::new(config);

        let settle_t = 1_000_000_i64;
        let b0 = 10.0; // $10 basis premium
        let dividend = 12.0; // $12 collected funding

        guard.arm_at_settlement(settle_t, b0, dividend);

        // Pre-epoch test
        let pre_verdict = guard.evaluate_reversal_guard(settle_t - 5000, 10.0, 60_000.0);
        match pre_verdict {
            FastBasisReversalAction::PreEpochPending {
                seconds_to_settlement,
            } => {
                assert_eq!(seconds_to_settlement, 5.0);
            }
            _ => panic!("Expected pre-epoch pending"),
        }

        // Post-epoch normal hold (5 seconds post-epoch, basis intact)
        let hold_verdict = guard.evaluate_reversal_guard(settle_t + 5000, 9.5, 60_000.0);
        match hold_verdict {
            FastBasisReversalAction::HoldCarryPosition { .. } => {}
            _ => panic!("Expected hold"),
        }

        // Post-epoch decay breach: basis drops from $10 to $6 (decay = $4 > 0.25 * $12 = $3)
        let breach_verdict = guard.evaluate_reversal_guard(settle_t + 12000, 6.0, 60_000.0);
        match breach_verdict {
            FastBasisReversalAction::UrgentExitMarketIOC { reason, .. } => {
                assert!(reason.contains("Basis decay"));
            }
            _ => panic!("Expected urgent exit on decay breach"),
        }

        // Dwell time timeout check (> 35s)
        let mut guard2 = FastBasisHarvestReversalGuard::new(FastBasisReversalConfig::default());
        guard2.arm_at_settlement(settle_t, b0, dividend);
        let timeout_verdict = guard2.evaluate_reversal_guard(settle_t + 36000, 9.8, 60_000.0);
        match timeout_verdict {
            FastBasisReversalAction::UrgentExitMarketIOC { reason, .. } => {
                assert!(reason.contains("Post-epoch maximum dwell time"));
            }
            _ => panic!("Expected urgent exit on 35s dwell time limit"),
        }
    }
}
