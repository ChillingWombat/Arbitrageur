//! Pre-Settlement Funding Epoch Countdown and Cubic Variance Collapse Predictor.
//!
//! Models the convergence of perpetual contract funding rates over their settlement
//! windows (e.g. 8-hour epochs on Binance, OKX, Deribit, Lighter; 1-hour on Hyperliquid).
//!
//! Key Mathematical Formulations:
//! 1. **Time-Weighted Average Price (TWAP) Realization**:
//!    $$F_T = \tau \bar{P}_{[0, t]} + (1 - \tau) \bar{P}_{[t, T]}$$
//! 2. **Cubic Variance Collapse**:
//!    $$\text{Var}(F_T \mid \mathcal{F}_t) = \frac{\sigma_P^2 T}{3} (1 - \tau)^3$$
//!    By mid-epoch ($\tau = 0.50$), $87.5\%$ of future variance is eliminated.
//!    By $\tau = 0.75$, $98.4\%$ is eliminated, rendering the upcoming payout virtually fixed.
//! 3. **Settlement Sniping & Avoidance Timing**:
//!    Determines optimal entry/exit acceleration relative to the settlement timestamp.

use serde::{Deserialize, Serialize};

/// Result of pre-settlement funding epoch countdown forecast.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FundingEpochForecast {
    /// Normalized epoch progress $\tau \in [0.0, 1.0]$.
    pub tau_progress: f64,
    /// Seconds remaining until the next funding settlement timestamp.
    pub seconds_to_settlement: f64,
    /// Expected final settled funding rate for the epoch (as a fraction, e.g. 0.0002 = 2 bps).
    pub predicted_settled_rate: f64,
    /// Fraction of initial uncertainty remaining: $(1 - \tau)^3 \in [0.0, 1.0]$.
    pub remaining_uncertainty_ratio: f64,
    /// True if $\tau \ge 0.73$ (uncertainty ratio $< 2.0\%$), indicating near-deterministic payout.
    pub is_rate_locked_in: bool,
    /// True if remaining stochastic variance could reverse the sign of the funding payment.
    pub is_sign_flip_risk: bool,
    /// Annualized percentage rate (APR %) implied by the predicted settlement.
    pub predicted_apr_pct: f64,
}

/// Pre-settlement funding epoch predictor.
#[derive(Debug, Clone)]
pub struct FundingEpochPredictor {
    /// Standard epoch length in seconds (e.g. 28,800 for 8h, 3,600 for 1h).
    pub epoch_duration_secs: f64,
}

impl Default for FundingEpochPredictor {
    fn default() -> Self {
        Self {
            epoch_duration_secs: 28800.0, // 8 Hours
        }
    }
}

impl FundingEpochPredictor {
    pub fn new(epoch_duration_secs: f64) -> Self {
        Self {
            epoch_duration_secs: epoch_duration_secs.max(60.0),
        }
    }

    /// Creates an 8-hour epoch predictor (Binance, OKX, Lighter, Deribit standard).
    pub fn for_8h_epoch() -> Self {
        Self::new(28800.0)
    }

    /// Creates a 1-hour epoch predictor (Hyperliquid standard).
    pub fn for_1h_epoch() -> Self {
        Self::new(3600.0)
    }

    /// Evaluates the funding forecast given timestamps and premium observations.
    ///
    /// * `epoch_start_ms`: Millisecond timestamp when the current epoch began.
    /// * `epoch_end_ms`: Millisecond timestamp when settlement occurs.
    /// * `current_time_ms`: Current wall-clock time in milliseconds.
    /// * `realized_twap`: Average premium index realized from `epoch_start` to `current_time`.
    /// * `current_premium`: Instantaneous spot-perp basis / premium rate right now.
    /// * `historical_sigma_premium`: Trailing standard deviation of hourly premium fluctuations.
    pub fn evaluate_forecast(
        &self,
        epoch_start_ms: i64,
        epoch_end_ms: i64,
        current_time_ms: i64,
        realized_twap: f64,
        current_premium: f64,
        historical_sigma_premium: f64,
    ) -> Option<FundingEpochForecast> {
        let total_duration_ms = (epoch_end_ms - epoch_start_ms) as f64;
        if total_duration_ms <= 0.0 || current_time_ms < epoch_start_ms {
            return None;
        }

        let elapsed_ms = (current_time_ms - epoch_start_ms) as f64;
        let tau = (elapsed_ms / total_duration_ms).clamp(0.0, 1.0);
        let seconds_to_settle = ((epoch_end_ms - current_time_ms).max(0) as f64) / 1000.0;

        // Blended forecast: realized portion + remaining portion expected at current premium
        let predicted_rate = tau * realized_twap + (1.0 - tau) * current_premium;

        // Cubic uncertainty collapse: Var(F_T | F_t) propto (1 - tau)^3
        let remaining_uncertainty = (1.0 - tau).powi(3);

        // Near-certainty threshold: uncertainty below 2% of original variance
        let is_locked_in = remaining_uncertainty < 0.02 || tau >= 0.73;

        // Standard deviation of remaining TWAP estimate
        let remaining_std = historical_sigma_premium * remaining_uncertainty.sqrt();
        // Sign flip risk: if 2 * remaining_std exceeds the predicted magnitude
        let is_sign_flip_risk = predicted_rate.abs() < 2.0 * remaining_std;

        // Annualized APR: (Rate per epoch) * (epochs per year) * 100%
        let epochs_per_year = (8760.0 * 3600.0) / self.epoch_duration_secs;
        let predicted_apr = predicted_rate * epochs_per_year * 100.0;

        Some(FundingEpochForecast {
            tau_progress: tau,
            seconds_to_settlement: seconds_to_settle,
            predicted_settled_rate: predicted_rate,
            remaining_uncertainty_ratio: remaining_uncertainty,
            is_rate_locked_in: is_locked_in,
            is_sign_flip_risk,
            predicted_apr_pct: predicted_apr,
        })
    }

    /// Evaluates whether an impending settlement should accelerate an exit or postpone it.
    ///
    /// * `net_position_side_positive`: True if our position is in the direction that RECEIVES funding.
    /// * `seconds_to_settlement`: Time until settlement in seconds.
    /// * `predicted_settled_rate`: Expected funding rate fraction.
    pub fn settlement_timing_recommendation(
        &self,
        receives_funding: bool,
        seconds_to_settlement: f64,
        predicted_settled_rate: f64,
    ) -> SettlementTimingAdvice {
        if seconds_to_settlement <= 0.0 {
            return SettlementTimingAdvice::Neutral;
        }

        if receives_funding && predicted_settled_rate > 0.00005 {
            // Due to receive a significant funding payment (>= 0.5 bps) within 5 minutes
            if seconds_to_settlement <= 300.0 {
                return SettlementTimingAdvice::PostponeExitUntilSettled {
                    seconds_to_wait: seconds_to_settlement + 15.0, // wait until 15s after settle
                    expected_payout_rate: predicted_settled_rate,
                };
            }
        } else if !receives_funding && predicted_settled_rate > 0.00005 {
            // Due to PAY a funding debit within 3 minutes
            if seconds_to_settlement <= 180.0 {
                return SettlementTimingAdvice::AccelerateExitBeforeSettlement {
                    seconds_remaining: seconds_to_settlement,
                    avoided_debit_rate: predicted_settled_rate,
                };
            }
        }

        SettlementTimingAdvice::Neutral
    }
}

/// Operational recommendation regarding funding settlement timing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SettlementTimingAdvice {
    /// No urgency or settlement distortion.
    Neutral,
    /// Position is due to receive an imminent positive funding payout: postpone planned rotation/flatten
    /// until after settlement confirms on-chain/on-exchange.
    PostponeExitUntilSettled {
        seconds_to_wait: f64,
        expected_payout_rate: f64,
    },
    /// Position is due to pay an impending funding debit: accelerate exit before settlement to avoid the debit.
    AccelerateExitBeforeSettlement {
        seconds_remaining: f64,
        avoided_debit_rate: f64,
    },
}

/// Configuration for the Funding Epoch Harvest Optimizer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpochHarvestConfig {
    pub basis_volatility_annual: f64,
    pub crowd_impact_coeff: f64,
    pub crowd_alpha: f64,
    pub basis_revert_kappa: f64,
    pub risk_aversion: f64,
}

impl Default for EpochHarvestConfig {
    fn default() -> Self {
        Self {
            basis_volatility_annual: 0.085,
            crowd_impact_coeff: 1.2e-4,
            crowd_alpha: 0.65,
            basis_revert_kappa: 0.08,
            risk_aversion: 1e-4,
        }
    }
}

/// Operational decision output from pre-epoch harvest evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarvestScheduleDecision {
    pub should_enter_now: bool,
    pub optimal_countdown_sec: f64,
    pub post_epoch_exit_sec: f64,
    pub expected_net_edge_usd: f64,
    pub is_too_late_for_entry: bool,
}

/// Pre-Epoch Funding Settlement Jump Optimizer.
///
/// Models pre-epoch crowding price impact and basis volatility to find the closed-form
/// optimal entry countdown Delta t_enter* and post-settlement fast-exit horizon tau_exit*.
#[derive(Debug, Clone)]
pub struct FundingEpochHarvestOptimizer {
    pub config: EpochHarvestConfig,
}

impl FundingEpochHarvestOptimizer {
    pub fn new(config: EpochHarvestConfig) -> Self {
        Self { config }
    }

    /// Computes optimal entry countdown Delta t_enter* (in seconds):
    /// Delta t* = [ (2 * alpha * eta) / (gamma * sigma_B^2 * S) ]^(1 / (alpha + 1))
    pub fn compute_optimal_entry_countdown(&self, spot_price: f64) -> f64 {
        let alpha = self.config.crowd_alpha;
        let eta = self.config.crowd_impact_coeff;
        let sigma_b_per_sec = self.config.basis_volatility_annual / (365.25 * 86400.0_f64).sqrt();
        let sigma_b_sq = sigma_b_per_sec * sigma_b_per_sec;
        let gamma = self.config.risk_aversion;

        let numerator = 2.0 * alpha * eta;
        let denominator = gamma * sigma_b_sq * spot_price.max(1.0);

        if denominator <= 1e-15 {
            return 180.0; // Safe default ~3 minutes
        }

        let tau_star = (numerator / denominator).powf(1.0 / (alpha + 1.0));
        tau_star.clamp(45.0, 360.0) // Bounded between 45s and 6 minutes
    }

    /// Computes post-settlement fast exit horizon tau_exit* (in seconds)
    /// to exit before basis mean-reversion snapback erodes more than 25% of the collected funding dividend:
    /// tau_exit = (1 / kappa) * ln( B_0 / (B_0 - max_decay) )
    pub fn compute_optimal_exit_horizon(
        &self,
        expected_funding_usd: f64,
        basis_premium_usd: f64,
    ) -> f64 {
        let kappa = self.config.basis_revert_kappa.max(1e-4);
        let max_allowable_decay = 0.25 * expected_funding_usd;

        if basis_premium_usd <= max_allowable_decay {
            return 25.0; // Safe default
        }

        let ratio = basis_premium_usd / (basis_premium_usd - max_allowable_decay).max(1e-4);
        let tau_exit = (1.0 / kappa) * ratio.ln();
        tau_exit.clamp(10.0, 45.0)
    }

    /// Evaluates current market state and generates execution triggers
    pub fn evaluate_epoch_window(
        &self,
        seconds_to_epoch: f64,
        spot_price: f64,
        predicted_funding_rate_8h: f64,
        position_notional_usd: f64,
        roundtrip_fees_usd: f64,
    ) -> HarvestScheduleDecision {
        let optimal_countdown = self.compute_optimal_entry_countdown(spot_price);
        let funding_payment_usd = position_notional_usd * predicted_funding_rate_8h;
        let net_expected_edge = funding_payment_usd - roundtrip_fees_usd;

        let exit_horizon =
            self.compute_optimal_exit_horizon(funding_payment_usd, 0.50 * funding_payment_usd);

        if net_expected_edge <= 0.0 {
            return HarvestScheduleDecision {
                should_enter_now: false,
                optimal_countdown_sec: optimal_countdown,
                post_epoch_exit_sec: exit_horizon,
                expected_net_edge_usd: net_expected_edge,
                is_too_late_for_entry: false,
            };
        }

        let is_too_late = seconds_to_epoch < 30.0;
        let is_in_entry_window = seconds_to_epoch <= (optimal_countdown + 30.0) && !is_too_late;

        HarvestScheduleDecision {
            should_enter_now: is_in_entry_window,
            optimal_countdown_sec: optimal_countdown,
            post_epoch_exit_sec: exit_horizon,
            expected_net_edge_usd: net_expected_edge,
            is_too_late_for_entry: is_too_late,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cubic_variance_collapse() {
        let pred = FundingEpochPredictor::for_8h_epoch();
        let start = 1_000_000;
        let end = start + 28_800 * 1000;

        // 1. At start (tau = 0.0)
        let f0 = pred
            .evaluate_forecast(start, end, start, 0.0001, 0.0001, 0.0002)
            .unwrap();
        assert!((f0.tau_progress - 0.0).abs() < 1e-4);
        assert!((f0.remaining_uncertainty_ratio - 1.0).abs() < 1e-4);
        assert!(!f0.is_rate_locked_in);

        // 2. At midpoint (tau = 0.50, 4 hours elapsed)
        let mid = start + 14_400 * 1000;
        let f_mid = pred
            .evaluate_forecast(start, end, mid, 0.0002, 0.0001, 0.0002)
            .unwrap();
        assert!((f_mid.tau_progress - 0.5).abs() < 1e-4);
        // (1 - 0.5)^3 = 0.125 (12.5% uncertainty remaining)
        assert!((f_mid.remaining_uncertainty_ratio - 0.125).abs() < 1e-4);
        assert!(!f_mid.is_rate_locked_in);

        // 3. At 6 hours (tau = 0.75)
        let t_75 = start + 21_600 * 1000;
        let f_75 = pred
            .evaluate_forecast(start, end, t_75, 0.0002, 0.0002, 0.0002)
            .unwrap();
        assert!((f_75.tau_progress - 0.75).abs() < 1e-4);
        // (1 - 0.75)^3 = 0.015625 (< 2% uncertainty remaining!)
        assert!(f_75.remaining_uncertainty_ratio < 0.02);
        assert!(f_75.is_rate_locked_in);
    }

    #[test]
    fn test_settlement_timing_advice() {
        let pred = FundingEpochPredictor::for_8h_epoch();

        // 60 seconds before settlement, receiving funding -> postpone exit
        let advice = pred.settlement_timing_recommendation(true, 60.0, 0.0003);
        match advice {
            SettlementTimingAdvice::PostponeExitUntilSettled {
                seconds_to_wait, ..
            } => {
                assert!(seconds_to_wait > 60.0);
            }
            _ => panic!("Expected PostponeExitUntilSettled"),
        }

        // 90 seconds before settlement, paying funding -> accelerate exit
        let advice_pay = pred.settlement_timing_recommendation(false, 90.0, 0.0003);
        match advice_pay {
            SettlementTimingAdvice::AccelerateExitBeforeSettlement {
                seconds_remaining, ..
            } => {
                assert_eq!(seconds_remaining, 90.0);
            }
            _ => panic!("Expected AccelerateExitBeforeSettlement"),
        }
    }

    #[test]
    fn test_optimal_entry_countdown() {
        let optimizer = FundingEpochHarvestOptimizer::new(EpochHarvestConfig::default());
        let tau_btc = optimizer.compute_optimal_entry_countdown(60_000.0);
        assert!((45.0..=360.0).contains(&tau_btc));

        let tau_eth = optimizer.compute_optimal_entry_countdown(3_000.0);
        assert!((45.0..=360.0).contains(&tau_eth));
        assert!(tau_eth >= tau_btc);
    }

    #[test]
    fn test_epoch_window_evaluation() {
        let optimizer = FundingEpochHarvestOptimizer::new(EpochHarvestConfig::default());

        // 1. Uneconomic edge: funding does not cover fees
        let dec_uneconomic =
            optimizer.evaluate_epoch_window(120.0, 60_000.0, 0.00005, 10_000.0, 20.0);
        assert!(!dec_uneconomic.should_enter_now);
        assert!(dec_uneconomic.expected_net_edge_usd < 0.0);
        assert!(!dec_uneconomic.is_too_late_for_entry);

        // 2. Too close to settlement (< 30s) -> adverse selection front-running hazard
        let dec_too_late = optimizer.evaluate_epoch_window(15.0, 60_000.0, 0.0005, 10_000.0, 1.0);
        assert!(!dec_too_late.should_enter_now);
        assert!(dec_too_late.is_too_late_for_entry);

        // 3. Perfect entry window: 150s remaining, lucrative funding
        let dec_enter = optimizer.evaluate_epoch_window(150.0, 60_000.0, 0.0005, 10_000.0, 1.0);
        assert!(dec_enter.should_enter_now);
        assert!(!dec_enter.is_too_late_for_entry);
        assert!(dec_enter.expected_net_edge_usd > 0.0);
        assert!(dec_enter.post_epoch_exit_sec >= 10.0 && dec_enter.post_epoch_exit_sec <= 45.0);
    }
}
