//! Strategy performance, forecast calibration, fill P&L, utilisation
//! history, and walk-forward optimization entry points. Split out of
//! `commands.rs`.

use crate::commands::{effective_venue_fee_bps, UTILISATION_DEFAULT_WINDOW_MS};
use crate::state::AppEngineState;
use arbitrageur_execution::virtual_ledger::StrategyMetrics;
use arbitrageur_math::calibration::WfoOptimizationResult;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;

pub fn get_strategy_performance_impl(
    strategy_id: Option<String>,
    state: &AppEngineState,
) -> Result<Vec<StrategyMetrics>, String> {
    let ledger = state.virtual_ledger.read();
    let strat_ids = match strategy_id {
        Some(s) => vec![s],
        None => vec![
            "spatial_spread".to_string(),
            "stat_arb".to_string(),
            "funding_rate".to_string(),
            "prediction_parity".to_string(),
        ],
    };

    // Environment-scoped: metrics only aggregate lots opened under the
    // ACTIVE environment — live and testnet history never blend.
    let env = state.env().as_str();
    let metrics = strat_ids
        .into_iter()
        .map(|id| ledger.compute_strategy_metrics(&id, env))
        .collect();

    Ok(metrics)
}

/// One scored cut of the prediction log: a (strategy, kind) slice's
/// calibration statistics. `kind` preserves event semantics — only
/// net-P&L kinds are scored, so a reversion forecast can never inflate a
/// profit-probability report.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationCutDto {
    pub strategy_id: String,
    pub kind: String,
    pub report: arbitrageur_math::forecast_calibration::CalibrationReport,
}

/// Forecast-calibration report over the durable prediction log.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationReportDto {
    /// Predictions scored against a reconciled realized outcome.
    pub scored: usize,
    /// Predictions still waiting on a reconciled close (open or
    /// unrealized positions) — shown so thin evidence is visible.
    pub unresolved: usize,
    /// Aggregate calibration over every scored net-P&L prediction.
    pub overall: arbitrageur_math::forecast_calibration::CalibrationReport,
    /// Per (strategy, kind) cuts for net-P&L-scored predictions.
    pub cuts: Vec<CalibrationCutDto>,
    /// Count of logged-but-never-P&L-scored kinds (reversion, persist,
    /// heuristic) — surfaced so the audit trail is visible.
    pub audit_only: usize,
}

/// Score the durable prediction log. `kind`/`strategy_id`/`live_only`
/// filter the rows; only kinds whose event is realized net P&L are
/// scored — the rest count toward `audit_only`.
pub fn get_calibration_report_impl(
    strategy_id: Option<String>,
    live_only: bool,
    state: &AppEngineState,
) -> Result<CalibrationReportDto, String> {
    let rows = state
        .state_store
        .list_signal_predictions(
            None,
            strategy_id.as_deref(),
            live_only.then_some(true),
            false,
        )
        .map_err(|e| e.to_string())?;

    let mut scored_pairs: Vec<(f64, bool)> = Vec::new();
    let mut unresolved = 0usize;
    let mut audit_only = 0usize;
    let mut by_cut: std::collections::BTreeMap<(String, String), Vec<(f64, bool)>> =
        std::collections::BTreeMap::new();

    for r in &rows {
        let scorable = crate::state_store::NET_PNL_PREDICTION_KINDS.contains(&r.kind.as_str());
        match (scorable, r.realized_net_bps) {
            (true, Some(net_bps)) => {
                let pair = (r.predicted_p, net_bps > 0.0);
                scored_pairs.push(pair);
                by_cut
                    .entry((r.strategy_id.clone(), r.kind.clone()))
                    .or_default()
                    .push(pair);
            }
            (true, None) => unresolved += 1,
            (false, _) => audit_only += 1,
        }
    }

    let cuts = by_cut
        .into_iter()
        .map(|((strategy_id, kind), pairs)| CalibrationCutDto {
            strategy_id,
            kind,
            report: arbitrageur_math::forecast_calibration::calibration_report(&pairs),
        })
        .collect();

    Ok(CalibrationReportDto {
        scored: scored_pairs.len(),
        unresolved,
        overall: arbitrageur_math::forecast_calibration::calibration_report(&scored_pairs),
        cuts,
        audit_only,
    })
}

/// Attributed realized PnL for one strategy, derived from durable signal-leg
/// fills (fees included). Closed root families owned by `strategy_id` sum
/// their root + `flat-*` child fills via side-signed cash flows; families
/// that cannot be valued (no strategy tag, or no priced fills) are excluded
/// from the sums and reported as `unattributed_closed_families`.
/// Environment-scoped: only signals recorded under the ACTIVE environment
/// aggregate — live and testnet fills never blend. Legacy `unknown` rows
/// (pre-v32, un-attributable) are excluded from both environments' sums
/// rather than guessed into one.
pub fn get_strategy_fill_pnl_impl(
    strategy_id: &str,
    state: &AppEngineState,
) -> Result<crate::attribution::StrategyFillPnl, String> {
    use crate::state_store::LegSide;
    let want = crate::attribution::attribute_strategy_id(strategy_id);
    let env = state.env().as_str();
    let signals = state
        .state_store
        .list_signals(None, None)
        .map_err(|e| e.to_string())?;
    let signals: Vec<_> = signals
        .into_iter()
        .filter(|s| s.environment == env)
        .collect();
    let mut net_usd = 0.0_f64;
    let mut notional = 0.0_f64;
    let mut closed = 0_usize;
    let mut open = 0_usize;
    let mut unattributed = 0_usize;
    for root in signals
        .iter()
        .filter(|s| !s.id.starts_with("flat-") && !s.id.starts_with("trim-"))
    {
        let owned = crate::attribution::attribute_strategy_id(&root.strategy_id) == want;
        let is_closed = root.status == crate::state_store::SignalStatus::Closed;
        if !owned {
            continue;
        }
        if !is_closed {
            if root.status.counts_as_open() {
                open += 1;
            }
            continue;
        }
        let mut fills = Vec::new();
        for sig in std::iter::once(root).chain(
            signals
                .iter()
                .filter(|s| crate::state_store::is_flatten_child_id(&s.id, &root.id)),
        ) {
            for leg in &sig.legs {
                fills.push(crate::attribution::LegFill {
                    is_short: leg.side == LegSide::Short,
                    qty: leg.filled_qty,
                    price: leg.fill_price,
                    fee_usd: leg.fee_usd,
                });
            }
        }
        match crate::attribution::family_realized_pnl(&fills) {
            Some(pnl) => {
                closed += 1;
                net_usd += pnl.net_usd;
                notional += pnl.notional_usd;
            }
            None => {
                unattributed += 1;
            }
        }
    }
    let is_live = state.is_live();
    if want == crate::attribution::UNATTRIBUTED_STRATEGY_ID {
        // Quarantine query: report only the quarantined families.
        return Ok(crate::attribution::StrategyFillPnl {
            strategy_id: want,
            net_profit_usd: 0.0,
            net_profit_bps: None,
            closed_families: 0,
            open_families: 0,
            unattributed_closed_families: unattributed,
            unattributed_log_entries: state
                .execution_logs
                .read()
                .iter()
                .filter(|e| {
                    e.is_live == is_live
                        && crate::attribution::attribute_strategy_id(&e.strategy_id)
                            == crate::attribution::UNATTRIBUTED_STRATEGY_ID
                })
                .count(),
        });
    }
    Ok(crate::attribution::StrategyFillPnl {
        strategy_id: want,
        net_profit_usd: (net_usd * 100.0).round() / 100.0,
        net_profit_bps: if notional > 0.0 {
            Some(((net_usd / notional * 10_000.0) * 10.0).round() / 10.0)
        } else {
            None
        },
        closed_families: closed,
        open_families: open,
        unattributed_closed_families: unattributed,
        unattributed_log_entries: state
            .execution_logs
            .read()
            .iter()
            .filter(|e| {
                e.is_live == is_live
                    && crate::attribution::attribute_strategy_id(&e.strategy_id)
                        == crate::attribution::UNATTRIBUTED_STRATEGY_ID
            })
            .count(),
    })
}

/// Time-weighted average utilisation for one venue over a trailing window
/// (default 24h) from the durable margin-snapshot log. `partial` is true
/// when snapshots cover less than the full window — the UI labels the
/// shorter window honestly instead of stretching it.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UtilisationHistoryDto {
    pub venue: String,
    pub avg_rate_pct: Option<f64>,
    pub covered_ms: i64,
    pub window_ms: i64,
    pub partial: bool,
    pub point_count: usize,
}

pub fn get_utilisation_history_impl(
    venue: &str,
    window_hours: Option<u64>,
    state: &AppEngineState,
) -> Result<UtilisationHistoryDto, String> {
    let default_hours = (UTILISATION_DEFAULT_WINDOW_MS / 3_600_000).max(1) as u64;
    let hours = window_hours.unwrap_or(default_hours).clamp(1, 720);
    let window_ms: i64 = (hours as i64).saturating_mul(3_600_000);
    let now_ms = Utc::now().timestamp_millis();
    let since_ms = now_ms.saturating_sub(window_ms);
    let rows = state
        .state_store
        .list_margin_snapshots(venue, since_ms, state.env().as_str())
        .map_err(|e| e.to_string())?;
    let points: Vec<(i64, f64)> = rows.iter().map(|r| (r.ts_ms, r.rate_pct)).collect();
    let point_count = points.len();
    let avg = crate::attribution::time_weighted_avg(&points, now_ms, window_ms);
    Ok(UtilisationHistoryDto {
        venue: venue.trim().to_lowercase(),
        avg_rate_pct: avg.map(|a| (a.avg_rate_pct * 100.0).round() / 100.0),
        covered_ms: avg.map(|a| a.covered_ms).unwrap_or(0),
        window_ms,
        partial: avg.map(|a| a.covered_ms < window_ms).unwrap_or(true),
        point_count,
    })
}

pub async fn run_wfo_calibration_impl(
    symbol: &str,
    state: &AppEngineState,
) -> Result<WfoOptimizationResult, String> {
    let sym = crate::state_store::canonical_base(symbol);
    if sym.is_empty() {
        return Err("invalid symbol".into());
    }

    // Rebuild the executable cross-venue edge series from recorded book
    // samples: at each timestamp, buy the cheapest ask and sell the richest
    // bid across *different* venues — the same edge the spread radar trades.
    // Under `separate` USD-quote mode the requested pair's own series is the
    // honest input — merging sibling quotes would calibrate on instruments
    // the user chose to keep distinct.
    let now_ms = chrono::Utc::now().timestamp_millis();
    let samples = state
        .state_store
        .spread_samples_mode(
            symbol.trim(),
            state.usd_merge_quotes(),
            0,
            now_ms,
            state.env().as_str(),
        )
        .map_err(|e| e.to_string())?;

    let mut groups: std::collections::BTreeMap<i64, Vec<&crate::state_store::SpreadSample>> =
        std::collections::BTreeMap::new();
    for s in &samples {
        groups.entry(s.ts_ms).or_default().push(s);
    }
    let history: Vec<arbitrageur_math::QuoteObservation> = groups
        .iter()
        .filter_map(|(ts, venues)| {
            if venues.len() < 2 {
                return None;
            }
            let mut best_edge = f64::NEG_INFINITY;
            let mut ref_px = 0.0_f64;
            let mut best_depth = 1.0_f64;
            for buy in venues.iter() {
                for sell in venues.iter() {
                    if buy.venue_id.eq_ignore_ascii_case(&sell.venue_id) {
                        continue;
                    }
                    if !(buy.best_ask.is_finite()
                        && buy.best_ask > 0.0
                        && sell.best_bid.is_finite()
                        && sell.best_bid > 0.0)
                    {
                        continue;
                    }
                    let edge = sell.best_bid - buy.best_ask;
                    if edge > best_edge {
                        best_edge = edge;
                        ref_px = buy.best_ask;
                        // Binding constraint: thinner of the two sides the
                        // trade would actually take — buy venue's ask depth,
                        // sell venue's bid depth. Missing depth (pre-v28
                        // rows) falls back to neutral 1.0.
                        best_depth = match (buy.ask_depth, sell.bid_depth) {
                            (Some(a), Some(b)) => a.min(b),
                            (Some(a), None) => a,
                            (None, Some(b)) => b,
                            (None, None) => 1.0,
                        };
                    }
                }
            }
            if !best_edge.is_finite() || ref_px <= 0.0 {
                return None;
            }
            Some(arbitrageur_math::QuoteObservation::new(
                *ts as u64,
                best_edge / ref_px * 10_000.0,
                best_depth,
            ))
        })
        .collect();

    if history.len() < 40 {
        return Err(format!(
            "WFO needs at least 40 multi-venue spread observations; {sym} has {} \
             (spread sampling must record the symbol on 2+ venues)",
            history.len()
        ));
    }

    // Friction: sum of the two cheapest admitted venues' effective taker
    // fees (one per leg) plus a small exit-slippage allowance.
    let admitted = state.get_venues();
    let mut fees: Vec<f64> = admitted
        .iter()
        .map(|(vid, _, _)| effective_venue_fee_bps(state, vid))
        .filter(|f| f.is_finite() && *f >= 0.0)
        .collect();
    fees.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let friction_bps = match fees.len() {
        0 => 10.0,
        1 => fees[0] * 2.0,
        _ => fees[0] + fees[1],
    } + 4.0;

    // Target: the configured radar expected-profit floor when the monitor is
    // up; otherwise break-even.
    let target_profit_bps = state
        .monitor
        .lock()
        .as_ref()
        .map(|e| e.config().min_expected_profit_bps)
        .unwrap_or(0.0)
        .max(0.0);

    let candidate_windows: &[usize] = &[10, 20, 30, 50, 100, 200];
    Ok(arbitrageur_math::run_walk_forward_optimization(
        &history,
        candidate_windows,
        target_profit_bps,
        friction_bps,
    ))
}

#[tauri::command]
pub async fn get_strategy_performance(
    strategy_id: Option<String>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<StrategyMetrics>, String> {
    get_strategy_performance_impl(strategy_id, &state)
}

#[tauri::command]
pub async fn get_strategy_fill_pnl(
    strategy_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<crate::attribution::StrategyFillPnl, String> {
    get_strategy_fill_pnl_impl(&strategy_id, &state)
}

#[tauri::command]
pub async fn get_calibration_report(
    strategy_id: Option<String>,
    live_only: bool,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<CalibrationReportDto, String> {
    get_calibration_report_impl(strategy_id, live_only, &state)
}

#[tauri::command]
pub async fn get_utilisation_history(
    venue: String,
    window_hours: Option<u64>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<UtilisationHistoryDto, String> {
    get_utilisation_history_impl(&venue, window_hours, &state)
}

#[tauri::command]
pub async fn run_wfo_calibration(
    symbol: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<WfoOptimizationResult, String> {
    run_wfo_calibration_impl(&symbol, &state).await
}

// ---------------------------------------------------------------------------
