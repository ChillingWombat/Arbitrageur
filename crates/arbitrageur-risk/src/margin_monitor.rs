use arbitrageur_core::position::{AccountSnapshot, Position};
use arbitrageur_core::types::{dec, Decimal, VenueId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, warn};

/// Health status of an account or position margin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarginHealthStatus {
    /// Funded account with comfortable margin.
    Healthy,
    Warning,
    Critical,
    Breached,
    /// Unfunded account: zero equity, no open positions, no margin locked.
    /// Nothing is at risk, so this is informational — never critical.
    Empty,
    /// Liquidation/margin data missing — safety cannot be verified.
    /// Never reported as Healthy: an unverifiable position is a risk the
    /// monitor cannot rule out.
    Unknown,
}

impl MarginHealthStatus {
    pub fn severity(&self) -> u8 {
        match self {
            MarginHealthStatus::Breached => 5,
            MarginHealthStatus::Critical => 4,
            MarginHealthStatus::Warning => 3,
            MarginHealthStatus::Unknown => 2,
            MarginHealthStatus::Healthy => 1,
            MarginHealthStatus::Empty => 0,
        }
    }
}

/// Margin monitor configuration.
#[derive(Debug, Clone)]
pub struct MarginMonitorConfig {
    /// Warning threshold for margin utilization percentage (e.g. 70%)
    pub warning_margin_usage_pct: Decimal,
    /// Critical threshold for margin utilization percentage (e.g. 85%)
    pub critical_margin_usage_pct: Decimal,
    /// Minimum allowed liquidation buffer distance (e.g. 15%)
    pub min_liquidation_distance_pct: Decimal,
}

impl Default for MarginMonitorConfig {
    fn default() -> Self {
        Self {
            warning_margin_usage_pct: dec!(70.0),
            critical_margin_usage_pct: dec!(85.0),
            min_liquidation_distance_pct: dec!(15.0),
        }
    }
}

/// Portfolio-level summary of lower- and upper-tail copula stress.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TailCopulaStressReport {
    pub stressed_venues: Vec<String>,
    pub lower_tail_lambda: f64,
    pub upper_tail_lambda: f64,
    pub max_required_buffer_pct: Decimal,
}

pub struct MarginMonitor {
    config: MarginMonitorConfig,
}

impl MarginMonitor {
    pub fn new(config: MarginMonitorConfig) -> Self {
        Self { config }
    }

    /// Assess overall health of an account snapshot.
    pub fn evaluate_account(&self, account: &AccountSnapshot) -> MarginHealthStatus {
        if account.equity <= dec!(0) {
            // Unfunded and flat: nothing at risk — informational, not a breach.
            // Zero/negative equity WITH positions or locked margin stays Breached.
            if account.positions.is_empty() && account.maintenance_margin <= dec!(0) {
                return MarginHealthStatus::Empty;
            }
            return MarginHealthStatus::Breached;
        }

        let usage = account.margin_usage_pct;
        if usage >= self.config.critical_margin_usage_pct {
            debug!(
                "[{}] CRITICAL margin utilization: {}% (limit: {}%)",
                account.venue, usage, self.config.critical_margin_usage_pct
            );
            MarginHealthStatus::Critical
        } else if usage >= self.config.warning_margin_usage_pct {
            debug!(
                "[{}] High margin utilization: {}% (warning: {}%)",
                account.venue, usage, self.config.warning_margin_usage_pct
            );
            MarginHealthStatus::Warning
        } else {
            MarginHealthStatus::Healthy
        }
    }

    /// Assess position liquidation proximity.
    pub fn evaluate_position(&self, pos: &Position) -> MarginHealthStatus {
        if let Some(distance) = pos.liquidation_distance_pct() {
            let distance_pct = distance * dec!(100.0);
            if distance_pct <= dec!(5.0) {
                warn!(
                    "[{}] Position {} distance to liquidation is dangerously low: {:.2}%",
                    pos.venue, pos.symbol, distance_pct
                );
                MarginHealthStatus::Critical
            } else if distance_pct <= self.config.min_liquidation_distance_pct {
                warn!(
                    "[{}] Position {} liquidation buffer below safe threshold: {:.2}%",
                    pos.venue, pos.symbol, distance_pct
                );
                MarginHealthStatus::Warning
            } else {
                MarginHealthStatus::Healthy
            }
        } else {
            // No liquidation price from the venue — the distance to a
            // liquidation cannot be computed, so the position's safety is
            // unverified. Fail closed: never claim Healthy.
            MarginHealthStatus::Unknown
        }
    }

    /// Portfolio-margin liquidation distance: the fraction of account equity
    /// that can evaporate before equity reaches the maintenance requirement
    /// and the venue liquidates. Venues like Deribit never emit per-position
    /// liq prices — liquidation strikes at the account level — so this is
    /// the honest distance for books that hold them. `None` when equity is
    /// non-positive (already at/inside the boundary) or the venue reported
    /// no maintenance figure at all.
    pub fn account_liquidation_distance(account: &AccountSnapshot) -> Option<Decimal> {
        if account.equity <= dec!(0) || account.maintenance_margin <= dec!(0) {
            return None;
        }
        Some((account.equity - account.maintenance_margin) / account.equity)
    }

    /// Evaluates segregated vulnerability across multiple venue accounts,
    /// identifying isolated margin distress that global averaging masks.
    pub fn evaluate_segregated_vulnerability(
        &self,
        accounts: &[AccountSnapshot],
        volatile_collateral_haircut_pct: Option<Decimal>,
    ) -> SegregatedVulnerabilityReport {
        let haircut = volatile_collateral_haircut_pct.unwrap_or(dec!(0.20)); // default 20% haircut
        let mut worst_status = MarginHealthStatus::Empty;
        let mut most_vulnerable_venue = None;
        let mut min_liquidation_distance: Option<Decimal> = None;
        let mut vulnerable_venues = Vec::new();
        let mut max_venue_usage_pct = dec!(0);
        let mut sum_equity = dec!(0);
        let mut sum_maintenance = dec!(0);
        let mut stressed_equity_sum = dec!(0);

        for acc in accounts {
            sum_equity += acc.equity;
            sum_maintenance += acc.maintenance_margin;

            let status = self.evaluate_account(acc);
            if status.severity() > worst_status.severity() {
                worst_status = status;
                most_vulnerable_venue = Some(acc.venue.clone());
            }

            let usage = acc.margin_usage_pct;
            if usage > max_venue_usage_pct {
                max_venue_usage_pct = usage;
                if most_vulnerable_venue.is_none() || status.severity() >= worst_status.severity() {
                    most_vulnerable_venue = Some(acc.venue.clone());
                }
            }

            if let Some(distance) = Self::account_liquidation_distance(acc) {
                if min_liquidation_distance.is_none_or(|d| distance < d) {
                    min_liquidation_distance = Some(distance);
                    most_vulnerable_venue = Some(acc.venue.clone());
                }
            }

            if (status == MarginHealthStatus::Warning
                || status == MarginHealthStatus::Critical
                || status == MarginHealthStatus::Breached)
                && !vulnerable_venues.contains(&acc.venue)
            {
                vulnerable_venues.push(acc.venue.clone());
            }

            // Calculate stressed equity
            if acc.equity <= dec!(0) {
                stressed_equity_sum += acc.equity;
            } else if acc.balances.is_empty() {
                stressed_equity_sum += acc.equity * (dec!(1) - haircut);
            } else {
                let mut stable_usd = dec!(0);
                for b in &acc.balances {
                    let cur = b.currency.to_uppercase();
                    let is_stable = cur == "USDC"
                        || cur == "USDT"
                        || cur == "USD"
                        || cur == "DAI"
                        || cur == "USDE"
                        || cur == "FDUSD"
                        || cur == "PYUSD";
                    if is_stable {
                        stable_usd += b.total;
                    }
                }
                let effective_stable_usd = stable_usd.min(acc.equity);
                let volatile_usd = (acc.equity - effective_stable_usd).max(dec!(0));
                let acc_stressed_eq = effective_stable_usd + volatile_usd * (dec!(1) - haircut);
                stressed_equity_sum += acc_stressed_eq;
            }
        }

        let global_aggregate_usage_pct = if sum_equity > dec!(0) {
            (sum_maintenance / sum_equity) * dec!(100)
        } else {
            dec!(0)
        };

        let is_segregated_insolvency_masked = global_aggregate_usage_pct
            < self.config.warning_margin_usage_pct
            && (max_venue_usage_pct >= self.config.critical_margin_usage_pct
                || worst_status == MarginHealthStatus::Critical
                || worst_status == MarginHealthStatus::Breached);

        SegregatedVulnerabilityReport {
            worst_status,
            most_vulnerable_venue,
            min_liquidation_distance,
            vulnerable_venues,
            global_aggregate_usage_pct,
            max_venue_usage_pct,
            stressed_equity_sum,
            is_segregated_insolvency_masked,
        }
    }

    /// Evaluates joint tail copula liquidation stress across segregated accounts.
    /// Incorporates heavy-tailed Expected Shortfall (ES_0.999) under GPD and
    /// joint lower- and upper-tail dependence (Clayton lambda_L and Gumbel
    /// lambda_U) over transfer latency tau.
    pub fn evaluate_tail_copula_safety(
        &self,
        accounts: &[AccountSnapshot],
        copula_model: Option<&arbitrageur_math::tail_copula::CrossMarginTailEvtCopula>,
        unwind_latency_sec: f64,
    ) -> Vec<(
        VenueId,
        arbitrageur_math::tail_copula::AccountTailSafetyReport,
    )> {
        let default_model = arbitrageur_math::tail_copula::CrossMarginTailEvtCopula::default();
        let model = copula_model.unwrap_or(&default_model);

        accounts
            .iter()
            .map(|acc| {
                let notional_sum: f64 = acc
                    .positions
                    .iter()
                    .map(|p| {
                        (p.quantity.abs() * p.entry_price)
                            .to_string()
                            .parse::<f64>()
                            .unwrap_or(0.0)
                    })
                    .sum();
                let mm = acc
                    .maintenance_margin
                    .to_string()
                    .parse::<f64>()
                    .unwrap_or(0.0);
                let notional = if notional_sum > 0.0 {
                    notional_sum
                } else if mm > 0.0 {
                    mm / 0.05
                } else {
                    0.0
                };
                let equity_usd = acc.equity.to_string().parse::<f64>().unwrap_or(0.0);
                let mut report =
                    model.evaluate_account_safety(equity_usd, notional, unwind_latency_sec);
                if notional > 0.0 {
                    let multiplier = Self::joint_tail_stress_multiplier(model);
                    let tau_sqrt = (unwind_latency_sec / 3600.0).max(1e-4).sqrt();
                    let es_reserve = report.expected_shortfall_999 * tau_sqrt;
                    let joint_tail_penalty =
                        model.tail_scale_sigma * tau_sqrt * multiplier;
                    let required_buffer =
                        model.maintenance_margin_ratio + es_reserve + joint_tail_penalty;

                    report.required_margin_buffer = required_buffer;
                    report.joint_tail_penalty = joint_tail_penalty;
                    report.is_safe =
                        report.current_liquidation_distance >= report.required_margin_buffer;
                    report.warning_message = (!report.is_safe).then(|| {
                        format!(
                            "Margin distance {:.2}% below required joint EVT copula buffer {:.2}% (ES_0.999={:.2}%, lambda_L={:.2}, lambda_U={:.2}, joint multiplier={:.2}). Trigger defensive rebalancing or trim.",
                            report.current_liquidation_distance * 100.0,
                            report.required_margin_buffer * 100.0,
                            report.expected_shortfall_999 * 100.0,
                            model.clayton_lower_tail_dependence(),
                            model.gumbel_upper_tail_dependence(),
                            multiplier
                        )
                    });
                }
                (acc.venue.clone(), report)
            })
            .collect()
    }

    fn joint_tail_stress_multiplier(
        model: &arbitrageur_math::tail_copula::CrossMarginTailEvtCopula,
    ) -> f64 {
        let lower = model.clayton_lower_tail_dependence();
        let upper = model.gumbel_upper_tail_dependence();
        1.0 + lower + upper
    }

    /// Summarises copula stress from position-level liquidation distances.
    /// Missing liquidation prices are treated as stressed because their safety
    /// cannot be verified.
    pub fn evaluate_tail_copula_report(&self, positions: &[Position]) -> TailCopulaStressReport {
        let model = arbitrageur_math::tail_copula::CrossMarginTailEvtCopula::default();
        let lower_tail_lambda = model.clayton_lower_tail_dependence();
        let upper_tail_lambda = model.gumbel_upper_tail_dependence();
        let multiplier = Self::joint_tail_stress_multiplier(&model);
        let tau_sqrt = 1e-4_f64.sqrt();
        let required_buffer = model.maintenance_margin_ratio
            + model.expected_shortfall_gpd(0.999) * tau_sqrt
            + model.tail_scale_sigma * tau_sqrt * multiplier;

        let mut venue_distances: HashMap<String, Option<f64>> = HashMap::new();
        for position in positions {
            let distance = position.liquidation_distance_pct().and_then(|value| {
                value
                    .to_string()
                    .parse::<f64>()
                    .ok()
                    .filter(|distance| distance.is_finite())
            });
            let entry = venue_distances
                .entry(position.venue.0.clone())
                .or_insert(None);
            *entry = match (*entry, distance) {
                (Some(current), Some(candidate)) => Some(current.min(candidate)),
                (None, Some(candidate)) => Some(candidate),
                (current, None) => current,
            };
        }

        let mut stressed_venues: Vec<String> = venue_distances
            .into_iter()
            .filter_map(|(venue, distance)| {
                distance
                    .map(|distance| distance < required_buffer)
                    .unwrap_or(true)
                    .then_some(venue)
            })
            .collect();
        stressed_venues.sort();

        TailCopulaStressReport {
            stressed_venues,
            lower_tail_lambda,
            upper_tail_lambda,
            max_required_buffer_pct: crate::delta_guard::f64_to_dec_opt(required_buffer * 100.0)
                .unwrap_or(Decimal::ZERO),
        }
    }

    /// Returns true if any active segregated venue violates the dynamic tail copula buffer.
    pub fn is_any_venue_tail_stressed(
        &self,
        accounts: &[AccountSnapshot],
        copula_model: Option<&arbitrageur_math::tail_copula::CrossMarginTailEvtCopula>,
        unwind_latency_sec: f64,
    ) -> bool {
        self.evaluate_tail_copula_safety(accounts, copula_model, unwind_latency_sec)
            .into_iter()
            .any(|(_, report)| !report.is_safe)
    }

    /// Evaluates multi-dimensional Brownian distance to insolvency across segregated accounts.
    /// Maps AccountSnapshot instances into SegregatedVenueMarginState, applying asset covariance
    /// if provided or default 60% annualized volatility over the operational transfer latency window.
    pub fn evaluate_unified_distance_to_insolvency(
        &self,
        accounts: &[AccountSnapshot],
        engine: Option<&arbitrageur_math::unified_margin_warning::UnifiedMarginWarningEngine>,
        asset_covariance: Option<&[Vec<f64>]>,
    ) -> arbitrageur_math::unified_margin_warning::UnifiedMarginWarningReport {
        let default_engine =
            arbitrageur_math::unified_margin_warning::UnifiedMarginWarningEngine::default();
        let eng = engine.unwrap_or(&default_engine);

        let states: Vec<arbitrageur_math::unified_margin_warning::SegregatedVenueMarginState> =
            accounts
                .iter()
                .map(|acc| {
                    let equity_usd = acc.equity.to_string().parse::<f64>().unwrap_or(0.0);
                    let mm_usd = acc
                        .maintenance_margin
                        .to_string()
                        .parse::<f64>()
                        .unwrap_or(0.0);
                    let notionals: Vec<f64> = acc
                        .positions
                        .iter()
                        .map(|p| {
                            let px = if p.entry_price > dec!(0) {
                                p.entry_price
                            } else {
                                p.mark_price
                            };
                            (p.quantity.abs() * px)
                                .to_string()
                                .parse::<f64>()
                                .unwrap_or(0.0)
                        })
                        .collect();
                    arbitrageur_math::unified_margin_warning::SegregatedVenueMarginState::new(
                        acc.venue.as_str(),
                        equity_usd,
                        mm_usd,
                        notionals,
                        0.0,
                    )
                })
                .collect();

        let empty_cov: [Vec<f64>; 0] = [];
        let cov = asset_covariance.unwrap_or(&empty_cov);
        eng.evaluate_portfolio(&states, cov)
    }

    /// Evaluates multi-venue signed directional stress matrix across segregated accounts.
    ///
    /// Unlike unsigned haircuts or portfolio-aggregated checks, this accurately models the
    /// cross-venue liquidation cascade trap: where a delta-neutral position (e.g. Long Hyperliquid, Short Lighter)
    /// causes catastrophic mark-to-market losses on the isolated long account under market drops,
    /// triggering exchange liquidation and leaving the short leg nakedly exposed.
    pub fn evaluate_signed_directional_stress_matrix(
        &self,
        accounts: &[AccountSnapshot],
        shocks: Option<&[f64]>,
    ) -> SignedDirectionalStressReport {
        let rebalancer =
            arbitrageur_math::collateral_barrier::CrossVenueCollateralBarrierRebalancer::default();
        let default_shocks = [-0.35, -0.25, -0.15, 0.15, 0.25, 0.35];
        let eval_shocks = shocks.unwrap_or(&default_shocks);

        let mut margin_states = Vec::with_capacity(accounts.len());
        let mut venue_map = std::collections::HashMap::new();

        for acc in accounts {
            let equity_usd = acc.equity.to_string().parse::<f64>().unwrap_or(0.0);
            let mm_usd = acc
                .maintenance_margin
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0);

            let mut notional_sum = 0.0;
            let mut signed_delta_sum = 0.0;

            for p in &acc.positions {
                let qty: f64 = p.quantity.abs().to_string().parse().unwrap_or(0.0);
                let px: f64 = p.entry_price.to_string().parse().unwrap_or(0.0);
                let notional = qty * px;
                notional_sum += notional;

                let sign = match p.side {
                    arbitrageur_core::types::Side::Buy => 1.0,
                    arbitrageur_core::types::Side::Sell => -1.0,
                };
                signed_delta_sum += sign * notional;
            }

            let notional = if notional_sum > 0.0 {
                notional_sum
            } else if mm_usd > 0.0 {
                mm_usd / 0.05
            } else {
                0.0
            };

            let mmr_rate = if notional > 0.0 {
                (mm_usd / notional).max(0.01)
            } else {
                0.05
            };

            let state = arbitrageur_math::collateral_barrier::VenueAccountMarginState::new(
                acc.venue.0.clone(),
                equity_usd,
                notional,
                mmr_rate,
                500.0,
                signed_delta_sum,
            );
            margin_states.push(state);
            venue_map.insert(acc.venue.0.clone(), acc.venue.clone());
        }

        let matrix = rebalancer.evaluate_stress_matrix(&margin_states, Some(eval_shocks));

        let mut venue_stressed_barriers: std::collections::HashMap<VenueId, Vec<(f64, f64)>> =
            std::collections::HashMap::new();
        let mut worst_barrier = f64::INFINITY;
        let mut worst_shock = 0.0;
        let mut worst_venue = None;
        let mut has_breach = false;

        for (shock, venues) in matrix {
            for (v_id, barrier) in venues {
                if let Some(v) = venue_map.get(&v_id) {
                    venue_stressed_barriers
                        .entry(v.clone())
                        .or_default()
                        .push((shock, barrier));

                    if barrier <= 0.0 {
                        has_breach = true;
                    }
                    if barrier < worst_barrier {
                        worst_barrier = barrier;
                        worst_shock = shock;
                        worst_venue = Some(v.clone());
                    }
                }
            }
        }

        SignedDirectionalStressReport {
            worst_shock_pct: worst_shock,
            most_vulnerable_venue: worst_venue,
            has_stress_liquidation_breach: has_breach,
            venue_stressed_barriers,
        }
    }

    /// Returns true if any active segregated venue violates the dynamic liquidation barrier under stress.
    pub fn is_any_venue_liquidation_stressed(
        &self,
        accounts: &[AccountSnapshot],
        shocks: Option<&[f64]>,
    ) -> bool {
        self.evaluate_signed_directional_stress_matrix(accounts, shocks)
            .has_stress_liquidation_breach
    }

    /// Evaluates multi-Greek macro crash scenarios using `MultiGreekCoVaRStressEngine`.
    pub fn evaluate_macro_stress_scenarios(
        &self,
        spot_price: f64,
        portfolio_delta: f64,
        portfolio_gamma: f64,
        portfolio_vega: f64,
        portfolio_theta: f64,
        portfolio_rho: f64,
    ) -> Vec<arbitrageur_math::portfolio_greeks_opt::MacroStressResult> {
        arbitrageur_math::portfolio_greeks_opt::MultiGreekCoVaRStressEngine::run_standard_macro_suite(
            spot_price,
            portfolio_delta,
            portfolio_gamma,
            portfolio_vega,
            portfolio_theta,
            portfolio_rho,
        )
    }

    /// Evaluates multi-venue portfolio margin headroom under the worst macro stress scenario.
    /// If stressed margin utilization exceeds 85%, flags an automated rebalance advisory.
    #[allow(clippy::too_many_arguments)]
    pub fn check_macro_stress_margin_headroom(
        &self,
        accounts: &[AccountSnapshot],
        spot_price: f64,
        portfolio_delta: f64,
        portfolio_gamma: f64,
        portfolio_vega: f64,
        portfolio_theta: f64,
        portfolio_rho: f64,
    ) -> MacroStressHeadroomReport {
        let stress_results = self.evaluate_macro_stress_scenarios(
            spot_price,
            portfolio_delta,
            portfolio_gamma,
            portfolio_vega,
            portfolio_theta,
            portfolio_rho,
        );

        let worst_pnl = stress_results
            .iter()
            .map(|r| r.total_stressed_pnl)
            .fold(0.0_f64, |acc, x| acc.min(x));

        let worst_scenario = stress_results
            .iter()
            .min_by(|a, b| {
                a.total_stressed_pnl
                    .partial_cmp(&b.total_stressed_pnl)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|r| r.scenario_name.clone())
            .unwrap_or_else(|| "Unknown".to_string());

        let total_equity: f64 = accounts
            .iter()
            .map(|a| a.equity.to_string().parse::<f64>().unwrap_or(0.0))
            .sum();

        let total_mmr: f64 = accounts
            .iter()
            .map(|a| {
                a.maintenance_margin
                    .to_string()
                    .parse::<f64>()
                    .unwrap_or(0.0)
            })
            .sum();

        let stressed_equity = (total_equity + worst_pnl).max(0.0);
        let stressed_utilization_pct = if stressed_equity > 0.0 {
            (total_mmr / stressed_equity) * 100.0
        } else if total_mmr > 0.0 {
            100.0
        } else {
            0.0
        };

        let is_rebalance_advisory_required = stressed_utilization_pct >= 85.0;

        MacroStressHeadroomReport {
            worst_scenario_name: worst_scenario,
            worst_scenario_pnl_usd: worst_pnl,
            baseline_equity_usd: total_equity,
            stressed_equity_usd: stressed_equity,
            total_maintenance_margin_usd: total_mmr,
            stressed_margin_utilization_pct: stressed_utilization_pct,
            is_rebalance_advisory_required,
            scenario_results: stress_results,
        }
    }
}

/// Portfolio margin headroom assessment under macro stress testing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MacroStressHeadroomReport {
    pub worst_scenario_name: String,
    pub worst_scenario_pnl_usd: f64,
    pub baseline_equity_usd: f64,
    pub stressed_equity_usd: f64,
    pub total_maintenance_margin_usd: f64,
    pub stressed_margin_utilization_pct: f64,
    pub is_rebalance_advisory_required: bool,
    pub scenario_results: Vec<arbitrageur_math::portfolio_greeks_opt::MacroStressResult>,
}

/// Result of multi-venue signed directional stress testing.
#[derive(Debug, Clone, PartialEq)]
pub struct SignedDirectionalStressReport {
    /// The market shock percentage that caused the worst liquidation barrier deficit
    pub worst_shock_pct: f64,
    /// The venue that is most vulnerable under the worst shock
    pub most_vulnerable_venue: Option<VenueId>,
    /// Whether any venue breached its continuous liquidation barrier (B_liq <= 0) under any evaluated shock
    pub has_stress_liquidation_breach: bool,
    /// Stressed liquidation barriers per venue across evaluated shocks: (shock_pct, barrier_usd)
    pub venue_stressed_barriers: std::collections::HashMap<VenueId, Vec<(f64, f64)>>,
}

/// Segregated venue vulnerability assessment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegregatedVulnerabilityReport {
    /// Overall segregated portfolio health (worst across all segregated venues).
    pub worst_status: MarginHealthStatus,
    /// Identifier of the most vulnerable venue (lowest liquidation distance or highest usage).
    pub most_vulnerable_venue: Option<VenueId>,
    /// Minimum liquidation distance observed across all segregated venues.
    pub min_liquidation_distance: Option<Decimal>,
    /// Venues that are in Warning, Critical, or Breached state.
    pub vulnerable_venues: Vec<VenueId>,
    /// Global aggregate margin usage percentage: sum(maintenance) / sum(equity) * 100.
    pub global_aggregate_usage_pct: Decimal,
    /// Maximum margin usage percentage observed in any single venue.
    pub max_venue_usage_pct: Decimal,
    /// Stressed equity sum after applying collateral haircuts.
    pub stressed_equity_sum: Decimal,
    /// Whether any segregated venue is critically vulnerable even if global aggregate usage appears safe.
    pub is_segregated_insolvency_masked: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrageur_core::types::VenueId;

    fn empty_account() -> AccountSnapshot {
        AccountSnapshot {
            venue: VenueId("hyperliquid".into()),
            equity: dec!(0),
            free_margin: dec!(0),
            maintenance_margin: dec!(0),
            margin_usage_pct: dec!(0),
            balances: vec![],
            positions: vec![],
            timestamp: chrono::Utc::now(),
        }
    }

    #[test]
    fn account_liq_distance_uses_equity_vs_maintenance() {
        let mut acc = empty_account();
        acc.equity = dec!(1000);
        acc.maintenance_margin = dec!(200);
        // Liquidation when equity falls to maintenance: (1000-200)/1000 = 80%.
        assert_eq!(
            MarginMonitor::account_liquidation_distance(&acc),
            Some(dec!(0.8))
        );
        // No maintenance figure → cannot compute.
        acc.maintenance_margin = dec!(0);
        assert_eq!(MarginMonitor::account_liquidation_distance(&acc), None);
        // Equity already at/under the boundary → no distance to report.
        acc.equity = dec!(0);
        acc.maintenance_margin = dec!(200);
        assert_eq!(MarginMonitor::account_liquidation_distance(&acc), None);
    }

    #[test]
    fn unfunded_flat_account_is_empty_not_breached() {
        let m = MarginMonitor::new(MarginMonitorConfig::default());
        assert_eq!(
            m.evaluate_account(&empty_account()),
            MarginHealthStatus::Empty
        );
    }

    #[test]
    fn zero_equity_with_positions_stays_breached() {
        use arbitrageur_core::position::Position;
        use arbitrageur_core::types::Side;
        let m = MarginMonitor::new(MarginMonitorConfig::default());
        let mut acc = empty_account();
        acc.positions.push(Position {
            venue: VenueId("hyperliquid".into()),
            symbol: "BTC-USDC".into(),
            side: Side::Buy,
            quantity: dec!(1),
            entry_price: dec!(60000),
            mark_price: dec!(60000),
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: None,
            leverage: dec!(5),
            initial_margin: dec!(12000),
            maintenance_margin: dec!(6000),
            updated_at: chrono::Utc::now(),
        });
        assert_eq!(m.evaluate_account(&acc), MarginHealthStatus::Breached);
    }

    /// Regression (#46): a position with no liquidation price cannot be
    /// verified safe — it must never report Healthy.
    #[test]
    fn position_without_liquidation_price_is_unknown_not_healthy() {
        use arbitrageur_core::position::Position;
        use arbitrageur_core::types::Side;
        let m = MarginMonitor::new(MarginMonitorConfig::default());
        let pos = Position {
            venue: VenueId("hyperliquid".into()),
            symbol: "BTC-USDC".into(),
            side: Side::Buy,
            quantity: dec!(1),
            entry_price: dec!(60000),
            mark_price: dec!(60000),
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: None,
            leverage: dec!(5),
            initial_margin: dec!(12000),
            maintenance_margin: dec!(6000),
            updated_at: chrono::Utc::now(),
        };
        assert_eq!(m.evaluate_position(&pos), MarginHealthStatus::Unknown);

        // With a liquidation price far away, the same position is Healthy.
        let mut pos = pos;
        pos.liquidation_price = Some(dec!(30000));
        assert_eq!(m.evaluate_position(&pos), MarginHealthStatus::Healthy);
    }

    #[test]
    fn test_segregated_vulnerability_detects_masked_insolvency() {
        let monitor = MarginMonitor::new(MarginMonitorConfig::default());

        // Account 1 (Hyperliquid): $90k equity, $9k maintenance -> 10% usage (Safe)
        let mut acc1 = empty_account();
        acc1.venue = VenueId("hyperliquid".into());
        acc1.equity = dec!(90000);
        acc1.maintenance_margin = dec!(9000);
        acc1.margin_usage_pct = dec!(10.0);

        // Account 2 (Lighter): $10k equity, $9.5k maintenance -> 95% usage (Critical)
        let mut acc2 = empty_account();
        acc2.venue = VenueId("lighter".into());
        acc2.equity = dec!(10000);
        acc2.maintenance_margin = dec!(9500);
        acc2.margin_usage_pct = dec!(95.0);

        let report = monitor.evaluate_segregated_vulnerability(&[acc1, acc2], None);

        // Global usage is (9000 + 9500) / (90000 + 10000) = 18.5%
        assert_eq!(report.global_aggregate_usage_pct, dec!(18.5));
        assert_eq!(report.max_venue_usage_pct, dec!(95.0));
        assert_eq!(report.worst_status, MarginHealthStatus::Critical);
        assert_eq!(
            report.most_vulnerable_venue,
            Some(VenueId("lighter".into()))
        );
        assert!(report.is_segregated_insolvency_masked);
        assert_eq!(report.vulnerable_venues, vec![VenueId("lighter".into())]);
    }

    #[test]
    fn test_segregated_vulnerability_haircut_reduces_stressed_equity() {
        use arbitrageur_core::position::Balance;
        let monitor = MarginMonitor::new(MarginMonitorConfig::default());

        let mut acc = empty_account();
        acc.equity = dec!(20000);
        acc.balances = vec![
            Balance {
                venue: VenueId("hyperliquid".into()),
                currency: "USDC".into(),
                total: dec!(10000),
                free: dec!(10000),
                locked: dec!(0),
            },
            Balance {
                venue: VenueId("hyperliquid".into()),
                currency: "ETH".into(),
                total: dec!(10000),
                free: dec!(10000),
                locked: dec!(0),
            },
        ];

        // 20% haircut on volatile ETH ($10k -> $8k), 0% on USDC ($10k)
        let report = monitor.evaluate_segregated_vulnerability(&[acc], Some(dec!(0.20)));
        assert_eq!(report.stressed_equity_sum, dec!(18000));
    }

    #[test]
    fn test_segregated_vulnerability_with_raw_token_units() {
        use arbitrageur_core::position::Balance;
        let monitor = MarginMonitor::new(MarginMonitorConfig::default());

        let mut acc = empty_account();
        // $50,000 equity, held as 5,000 USDC and 0.75 BTC (raw token count, not USD)
        acc.equity = dec!(50000);
        acc.balances = vec![
            Balance {
                venue: VenueId("binance".into()),
                currency: "USDT".into(),
                total: dec!(5000),
                free: dec!(5000),
                locked: dec!(0),
            },
            Balance {
                venue: VenueId("binance".into()),
                currency: "BTC".into(),
                total: dec!(0.75), // Raw coin amount, not USD
                free: dec!(0.75),
                locked: dec!(0),
            },
        ];

        // 20% haircut on the volatile collateral ($45,000 * 0.80 = $36,000) + $5,000 stable = $41,000
        let report = monitor.evaluate_segregated_vulnerability(&[acc], Some(dec!(0.20)));
        assert_eq!(report.stressed_equity_sum, dec!(41000));
    }

    #[test]
    fn test_evaluate_tail_copula_safety() {
        let monitor = MarginMonitor::new(MarginMonitorConfig::default());

        // Safe account: $2,500 equity on $10,000 notional (25% equity vs 5% MMR)
        let mut safe_acc = empty_account();
        safe_acc.venue = VenueId("hyperliquid".into());
        safe_acc.equity = dec!(2500);
        safe_acc.maintenance_margin = dec!(500);
        safe_acc.positions = vec![Position {
            venue: VenueId("hyperliquid".into()),
            symbol: "ETH-PERP".into(),
            side: arbitrageur_core::types::Side::Buy,
            quantity: dec!(5.0),
            entry_price: dec!(2000.0),
            mark_price: dec!(2000.0),
            liquidation_price: Some(dec!(1600.0)),
            leverage: dec!(10),
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            initial_margin: dec!(1000),
            maintenance_margin: dec!(500),
            updated_at: chrono::Utc::now(),
        }];

        // Stressed account: $550 equity on $10,000 notional (5.5% equity vs 5% MMR -> 0.5% distance)
        let mut unsafe_acc = empty_account();
        unsafe_acc.venue = VenueId("lighter".into());
        unsafe_acc.equity = dec!(550);
        unsafe_acc.maintenance_margin = dec!(500);
        unsafe_acc.positions = vec![Position {
            venue: VenueId("lighter".into()),
            symbol: "ETH-PERP".into(),
            side: arbitrageur_core::types::Side::Sell,
            quantity: dec!(5.0),
            entry_price: dec!(2000.0),
            mark_price: dec!(2000.0),
            liquidation_price: Some(dec!(2050.0)),
            leverage: dec!(20),
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            initial_margin: dec!(500),
            maintenance_margin: dec!(500),
            updated_at: chrono::Utc::now(),
        }];

        let reports =
            monitor.evaluate_tail_copula_safety(&[safe_acc, unsafe_acc.clone()], None, 300.0);
        assert_eq!(reports.len(), 2);
        assert!(reports[0].1.is_safe, "Hyperliquid should be safe");
        assert!(
            !reports[1].1.is_safe,
            "Lighter should fail EVT copula buffer"
        );
        assert!(reports[1].1.warning_message.is_some());

        let low_upper = arbitrageur_math::tail_copula::CrossMarginTailEvtCopula::new(
            0.05, 0.025, 0.36, 0.015, 0.05, 1.5, 1.0,
        );
        let high_upper = arbitrageur_math::tail_copula::CrossMarginTailEvtCopula::new(
            0.05, 0.025, 0.36, 0.015, 0.05, 1.5, 4.0,
        );
        let low_report =
            monitor.evaluate_tail_copula_safety(&[unsafe_acc.clone()], Some(&low_upper), 300.0);
        let high_report =
            monitor.evaluate_tail_copula_safety(&[unsafe_acc], Some(&high_upper), 300.0);
        assert!(
            high_report[0].1.required_margin_buffer > low_report[0].1.required_margin_buffer,
            "Gumbel upper-tail dependence must increase the required buffer"
        );
    }

    #[test]
    fn test_tail_copula_stress_report_exposes_both_tails() {
        let monitor = MarginMonitor::new(MarginMonitorConfig::default());
        let positions = vec![Position {
            venue: VenueId("lighter".into()),
            symbol: "ETH-PERP".into(),
            side: arbitrageur_core::types::Side::Sell,
            quantity: dec!(5.0),
            entry_price: dec!(2000.0),
            mark_price: dec!(2000.0),
            liquidation_price: Some(dec!(2050.0)),
            leverage: dec!(20),
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            initial_margin: dec!(500),
            maintenance_margin: dec!(500),
            updated_at: chrono::Utc::now(),
        }];

        let report = monitor.evaluate_tail_copula_report(&positions);
        assert_eq!(report.stressed_venues, vec!["lighter".to_string()]);
        assert!(report.lower_tail_lambda > 0.0);
        assert!(report.upper_tail_lambda > 0.0);
        assert!(report.max_required_buffer_pct > dec!(5.0));
    }

    #[test]
    fn test_evaluate_unified_distance_to_insolvency() {
        use arbitrageur_math::unified_margin_warning::MarginDefenseStage;
        let monitor = MarginMonitor::new(MarginMonitorConfig::default());

        // Safe account: $5,000 equity, $500 maintenance margin, $5,000 notional (10x leverage)
        let mut safe_acc = empty_account();
        safe_acc.venue = VenueId("hyperliquid".into());
        safe_acc.equity = dec!(5000);
        safe_acc.maintenance_margin = dec!(500);
        safe_acc.positions = vec![Position {
            venue: VenueId("hyperliquid".into()),
            symbol: "ETH-PERP".into(),
            side: arbitrageur_core::types::Side::Buy,
            quantity: dec!(2.5),
            entry_price: dec!(2000.0),
            mark_price: dec!(2000.0),
            liquidation_price: Some(dec!(1000.0)),
            leverage: dec!(10),
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            initial_margin: dec!(500),
            maintenance_margin: dec!(500),
            updated_at: chrono::Utc::now(),
        }];

        // Stressed account: $520 equity, $500 maintenance margin, $10,000 notional ($20 buffer)
        let mut stressed_acc = empty_account();
        stressed_acc.venue = VenueId("lighter".into());
        stressed_acc.equity = dec!(520);
        stressed_acc.maintenance_margin = dec!(500);
        stressed_acc.positions = vec![Position {
            venue: VenueId("lighter".into()),
            symbol: "ETH-PERP".into(),
            side: arbitrageur_core::types::Side::Sell,
            quantity: dec!(5.0),
            entry_price: dec!(2000.0),
            mark_price: dec!(2000.0),
            liquidation_price: Some(dec!(2020.0)),
            leverage: dec!(20),
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            initial_margin: dec!(500),
            maintenance_margin: dec!(500),
            updated_at: chrono::Utc::now(),
        }];

        let report =
            monitor.evaluate_unified_distance_to_insolvency(&[safe_acc, stressed_acc], None, None);

        assert!(
            !report.is_portfolio_safe,
            "Portfolio should not be safe with stressed Lighter"
        );
        assert_eq!(report.worst_venue, "lighter");
        assert!(
            report.overall_stage == MarginDefenseStage::RebalanceRequired
                || report.overall_stage == MarginDefenseStage::EmergencyDeleveraging
        );
        assert!(report.total_top_up_needed_usd > 0.0);
    }

    #[test]
    fn test_signed_directional_stress_matrix_delta_neutral_cascade() {
        let monitor = MarginMonitor::new(MarginMonitorConfig::default());

        // Account A (Hyperliquid): Long 5.0 BTC at $65,000 = $325,000 notional. Equity = $35,000. MMR = $16,250.
        let mut long_acc = empty_account();
        long_acc.venue = VenueId("hyperliquid".into());
        long_acc.equity = dec!(35000);
        long_acc.maintenance_margin = dec!(16250);
        long_acc.positions = vec![Position {
            venue: VenueId("hyperliquid".into()),
            symbol: "BTC-PERP".into(),
            side: arbitrageur_core::types::Side::Buy,
            quantity: dec!(5.0),
            entry_price: dec!(65000.0),
            mark_price: dec!(65000.0),
            liquidation_price: Some(dec!(61500.0)),
            leverage: dec!(10),
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            initial_margin: dec!(32500),
            maintenance_margin: dec!(16250),
            updated_at: chrono::Utc::now(),
        }];

        // Account B (Lighter): Short 5.0 BTC at $65,000 = $325,000 notional. Equity = $35,000. MMR = $16,250.
        let mut short_acc = empty_account();
        short_acc.venue = VenueId("lighter".into());
        short_acc.equity = dec!(35000);
        short_acc.maintenance_margin = dec!(16250);
        short_acc.positions = vec![Position {
            venue: VenueId("lighter".into()),
            symbol: "BTC-PERP".into(),
            side: arbitrageur_core::types::Side::Sell,
            quantity: dec!(5.0),
            entry_price: dec!(65000.0),
            mark_price: dec!(65000.0),
            liquidation_price: Some(dec!(68500.0)),
            leverage: dec!(10),
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            initial_margin: dec!(32500),
            maintenance_margin: dec!(16250),
            updated_at: chrono::Utc::now(),
        }];

        let report =
            monitor.evaluate_signed_directional_stress_matrix(&[long_acc, short_acc], None);

        // Portfolio is delta neutral in aggregate, but isolated segregated long leg breaches MMR on crash!
        assert!(
            report.has_stress_liquidation_breach,
            "Segregated long account must breach liquidation barrier under crash shock"
        );
        assert_eq!(
            report.most_vulnerable_venue,
            Some(VenueId("hyperliquid".into())),
            "Hyperliquid long account must be identified as most vulnerable"
        );
        assert!(
            report.worst_shock_pct < 0.0,
            "Worst shock should be negative crash"
        );
    }

    #[test]
    fn test_macro_stress_margin_headroom_evaluation() {
        let monitor = MarginMonitor::new(MarginMonitorConfig::default());
        let mut acc = empty_account();
        acc.equity = dec!(20000);
        acc.maintenance_margin = dec!(10000);

        let report = monitor.check_macro_stress_margin_headroom(
            &[acc],
            60000.0, // spot price
            1.0,     // long 1.0 delta => -25% drop causes -$15,000 loss
            0.0,     // gamma
            50.0,    // vega
            -10.0,   // theta
            0.0,     // rho
        );

        assert_eq!(report.baseline_equity_usd, 20000.0);
        assert!(report.worst_scenario_pnl_usd < -10000.0);
        assert!(report.stressed_equity_usd < 10000.0);
        assert!(report.stressed_margin_utilization_pct > 85.0);
        assert!(
            report.is_rebalance_advisory_required,
            "Rebalance advisory must trigger when stressed utilization exceeds 85%"
        );
    }
}
