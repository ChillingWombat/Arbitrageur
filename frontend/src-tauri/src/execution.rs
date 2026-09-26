//! Live execution core for Simple Spread (spatial) plus the adjacent
//! execution domains: cross-ticker/prediction dispatch, candidate
//! arbitration, circuit-breaker controls, killswitch, symbol flattening,
//! passive unwind, and liquidation-trim/convergence exits.
//!
//! Split out of `commands.rs`. Shared helpers that also serve the scanner
//! and funding domains are reached via `crate::commands`; sizing helpers
//! used by flatten paths come from `crate::triangle`.

use crate::commands::{
    compute_route_economics, effective_venue_fee_bps, fetch_multi_market_quotes_impl,
    live_entry_gate, maybe_record_margin_snapshots, record_execution_failure, record_execution_log,
    refresh_equity_expectations_after_fills, reject_if_strategy_filtered,
    resolve_effective_leverage, snapshot_venue_equity, strategy_slippage_bps,
    validate_adapter_book, validate_market_symbol, ConvergenceCheckDto, MarginHealthDto,
    NetDeltaDto, RiskOverviewDto, SimStepResultDto, TickerMarketQuoteDto,
};
use crate::num::dec_opt_f64;
use crate::state::{AppEngineState, ExecutionLogEntry};
use crate::triangle::{flatten_step_qty, sweep_flatten_qty};
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::position::AccountSnapshot;
use arbitrageur_core::quote::OrderBook;
use arbitrageur_core::types::{dec, Decimal, Side, VenueId};
use arbitrageur_execution::coordinator::ArbitrageExecutionResult;
use arbitrageur_execution::passive_unwind::{
    PassiveBookRefresh, PassiveOutcome, PassiveUnwindConfig,
};
use arbitrageur_execution::smart_routing::SmartOrderRouter;
use arbitrageur_execution::virtual_ledger::CycleStatus;
use arbitrageur_risk::margin_monitor::{MarginHealthStatus, MarginMonitor, MarginMonitorConfig};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::State;

#[allow(clippy::too_many_arguments)]
/// Resolve account attribution for a two-leg signal. An explicit strategy
/// route wins; otherwise the first enabled route covering the venue pair is
/// used, falling back to each venue's default account. Accounts may be absent
/// on venues that have no configured profiles.
/// Accounts attributed to a signal's legs, resolved from an explicit or
/// eligible strategy route, else each venue's default account.
pub(crate) struct SignalAccounts {
    pub(crate) route_id: Option<String>,
    pub(crate) long: Option<String>,
    pub(crate) short: Option<String>,
}

pub(crate) fn resolve_signal_accounts(
    state: &AppEngineState,
    strategy_id: &str,
    venue_a: &str,
    venue_b: &str,
    route_id: Option<&str>,
) -> Result<SignalAccounts, String> {
    if let Some(rid) = route_id {
        let route = state
            .state_store
            .list_strategy_routes(None)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|r| r.id == rid)
            .ok_or_else(|| format!("strategy route {rid} does not exist"))?;
        if !route.enabled {
            return Err(format!("strategy route {rid} is disabled"));
        }
        if !route.strategy_id.eq_ignore_ascii_case(strategy_id) {
            return Err(format!(
                "route {rid} belongs to strategy {}, not {strategy_id}",
                route.strategy_id
            ));
        }
        let covers = (route.account_a_venue.eq_ignore_ascii_case(venue_a)
            && route.account_b_venue.eq_ignore_ascii_case(venue_b))
            || (route.account_a_venue.eq_ignore_ascii_case(venue_b)
                && route.account_b_venue.eq_ignore_ascii_case(venue_a));
        if !covers {
            return Err(format!(
                "route {rid} does not cover venues {venue_a}/{venue_b}"
            ));
        }
        let (long, short) = if route.account_a_venue.eq_ignore_ascii_case(venue_a) {
            (route.account_a_id, route.account_b_id)
        } else {
            (route.account_b_id, route.account_a_id)
        };
        return Ok(SignalAccounts {
            route_id: Some(route.id),
            long: Some(long),
            short: Some(short),
        });
    }
    if let Ok(routes) = state.state_store.eligible_strategy_routes(
        strategy_id,
        venue_a,
        venue_b,
        state.env().as_str(),
    ) {
        if let Some(r) = routes.into_iter().next() {
            let (long, short) = if r.account_a_venue.eq_ignore_ascii_case(venue_a) {
                (r.account_a_id, r.account_b_id)
            } else {
                (r.account_b_id, r.account_a_id)
            };
            return Ok(SignalAccounts {
                route_id: Some(r.id),
                long: Some(long),
                short: Some(short),
            });
        }
    }
    Ok(SignalAccounts {
        route_id: None,
        long: state
            .state_store
            .default_account(venue_a, state.env().as_str())
            .ok()
            .flatten()
            .map(|a| a.id),
        short: state
            .state_store
            .default_account(venue_b, state.env().as_str())
            .ok()
            .flatten()
            .map(|a| a.id),
    })
}

/// Persist actual leg fills and derive the per-signal balance status.
/// A signal is `Balanced` only when its own legs net to zero delta — aggregate
/// netting across unrelated signals can never mark it balanced.
pub(crate) fn reconcile_signal_legs(
    state: &AppEngineState,
    signal_id: &str,
    fills: [(
        crate::state_store::LegSide,
        &arbitrageur_core::order::OrderResponse,
    ); 2],
) -> Result<crate::state_store::SignalStatus, String> {
    use crate::state_store::SignalStatus;
    for (side, leg) in fills {
        let filled = leg
            .filled_qty
            .to_string()
            .parse::<f64>()
            .map_err(|_| "unparseable filled quantity".to_string())?;
        let fee = leg
            .fee
            .map(|f| {
                f.to_string()
                    .parse::<f64>()
                    .map_err(|_| "unparseable fee".to_string())
            })
            .transpose()?
            .unwrap_or(0.0);
        // Persist accepted order identity even on a zero-fill (working) leg —
        // the reconciliation supervisor needs it to reach terminal state.
        if !leg.order_id.is_empty() {
            let cid = (!leg.client_order_id.is_empty()).then_some(leg.client_order_id.as_str());
            if let Err(e) =
                state
                    .state_store
                    .record_leg_order_id(signal_id, side, leg.order_id.as_str(), cid)
            {
                tracing::error!("persist venue order id failed for {signal_id} {side:?}: {e}");
            }
        }
        if filled > 0.0 {
            let px = leg
                .avg_fill_price
                .and_then(|p| p.to_string().parse::<f64>().ok())
                .filter(|p| *p > 0.0)
                .ok_or_else(|| format!("leg {side:?} filled but reports no positive price"))?;
            state
                .state_store
                .apply_leg_fill(
                    signal_id,
                    side,
                    filled,
                    px,
                    fee,
                    Some(leg.order_id.as_str()),
                )
                .map_err(|e| e.to_string())?;
        }
    }
    // Delta reads in shared units (leg qty ÷ its persisted unit_scale).
    // The balanced bound is the intended-delta magnitude: per-venue wire
    // grids quantize the legs independently, so a scaled pair's matched
    // fills can differ in shared units by up to that construction
    // asymmetry — in either direction, since a coordinator trim leaves
    // delta at exactly 0. Anything beyond the bound is real one-sided
    // exposure (partial, overfill) and still flags Imbalanced.
    let delta = state
        .state_store
        .signal_delta(signal_id)
        .map_err(|e| e.to_string())?;
    let intended = state
        .state_store
        .signal_intended_delta(signal_id)
        .map_err(|e| e.to_string())?;
    let eps = 1e-6 * (1.0 + intended.abs());
    Ok(
        if delta.is_finite() && intended.is_finite() && delta.abs() <= intended.abs() + eps {
            SignalStatus::Balanced
        } else {
            SignalStatus::Imbalanced
        },
    )
}

/// Persist the atomic truth of a coordinator unwind: the filled entry leg's
/// actual fill on the root signal, plus the compensation unwind order as a
/// `flat-` close child with its own leg, order identity, fill, and terminal
/// status. Without this the unwind exists only as raw JSON inside the
/// parent's failure note — the opportunity's atomic transaction trail and
/// per-signal cash-flow reads lose two real venue orders.
pub(crate) fn persist_unwind_leg_records(
    state: &AppEngineState,
    signal_id: &str,
    side: crate::state_store::LegSide,
    entry_fill: &arbitrageur_core::order::OrderResponse,
    unwind_resp: &arbitrageur_core::order::OrderResponse,
) {
    let to_f64 = |d: rust_decimal::Decimal| d.to_string().parse::<f64>().unwrap_or(0.0);
    // 1) The entry leg's actual fill — monotonic apply can't regress truth.
    if !entry_fill.order_id.is_empty() {
        let cid =
            (!entry_fill.client_order_id.is_empty()).then_some(entry_fill.client_order_id.as_str());
        if let Err(e) = state.state_store.record_leg_order_id(
            signal_id,
            side,
            entry_fill.order_id.as_str(),
            cid,
        ) {
            tracing::error!("persist unwound entry leg order id failed for {signal_id}: {e}");
        }
    }
    let entry_qty = to_f64(entry_fill.filled_qty);
    if entry_qty > 0.0 {
        if let Some(px) = entry_fill
            .avg_fill_price
            .and_then(|p| p.to_string().parse::<f64>().ok())
            .filter(|p| *p > 0.0)
        {
            let fee = entry_fill
                .fee
                .map(|f| f.to_string().parse::<f64>().unwrap_or(0.0))
                .unwrap_or(0.0);
            if let Err(e) = state.state_store.apply_leg_fill(
                signal_id,
                side,
                entry_qty,
                px,
                fee,
                Some(entry_fill.order_id.as_str()),
            ) {
                tracing::error!("persist unwound entry leg fill failed for {signal_id}: {e}");
            }
        } else {
            tracing::warn!(
                "unwound entry leg {signal_id} reports fill without a positive price — left unpriced"
            );
        }
    }

    // 2) The compensation unwind as a `flat-` close child. Close-family
    // legs store the ORDER direction as the leg side (buy→Long, sell→Short
    // — `opposite` of the position being closed): that convention is what
    // lets family net-position math read the child fill as offsetting.
    let close_side = match side {
        crate::state_store::LegSide::Long => crate::state_store::LegSide::Short,
        crate::state_store::LegSide::Short => crate::state_store::LegSide::Long,
    };
    let root = match state.state_store.get_signal(signal_id) {
        Ok(Some(s)) => s,
        Ok(None) => {
            tracing::error!("unwind persist: root signal {signal_id} missing");
            return;
        }
        Err(e) => {
            tracing::error!("unwind persist: read root signal {signal_id} failed: {e}");
            return;
        }
    };
    let parent_leg = root.legs.iter().find(|l| l.side == side);
    let close_id = format!("flat-{signal_id}-{:04x}", rand::random::<u16>());
    let unwind_qty = to_f64(unwind_resp.filled_qty);
    let new_leg = crate::state_store::NewSignalLeg {
        account_id: parent_leg.and_then(|l| l.account_id.clone()),
        venue: parent_leg
            .map(|l| l.venue.clone())
            .unwrap_or_else(|| unwind_resp.venue.0.clone()),
        side: close_side,
        symbol: Some(
            parent_leg
                .map(|l| l.symbol.clone())
                .unwrap_or_else(|| unwind_resp.symbol.clone()),
        ),
        intended_qty: if unwind_qty > 0.0 {
            unwind_qty
        } else {
            entry_qty
        },
        intended_price: None,
        unit_scale: parent_leg.map(|l| l.unit_scale),
    };
    if let Err(e) = state.state_store.record_signal_execution(
        &crate::state_store::NewSignalExecution {
            id: close_id.clone(),
            strategy_id: root.strategy_id.clone(),
            symbol: root.symbol.clone(),
            route_id: root.route_id.clone(),
            expected_net_usd: None,
            note: Some(format!("coordinator unwind of {signal_id}")),
            legs: vec![new_leg],
        },
        state.env().as_str(),
    ) {
        tracing::error!("persist unwind child {close_id} failed: {e}");
        return;
    }
    if !unwind_resp.order_id.is_empty() {
        let cid = (!unwind_resp.client_order_id.is_empty())
            .then_some(unwind_resp.client_order_id.as_str());
        if let Err(e) = state.state_store.record_leg_order_id(
            &close_id,
            close_side,
            unwind_resp.order_id.as_str(),
            cid,
        ) {
            tracing::error!("persist unwind order id failed for {close_id}: {e}");
        }
    }
    let unwind_px = unwind_resp
        .avg_fill_price
        .and_then(|p| p.to_string().parse::<f64>().ok())
        .filter(|p| *p > 0.0);
    if let Some(px) = unwind_px.filter(|_| unwind_qty > 0.0) {
        let fee = unwind_resp
            .fee
            .map(|f| f.to_string().parse::<f64>().unwrap_or(0.0))
            .unwrap_or(0.0);
        if let Err(e) = state.state_store.apply_leg_fill(
            &close_id,
            close_side,
            unwind_qty,
            px,
            fee,
            Some(unwind_resp.order_id.as_str()),
        ) {
            tracing::error!("persist unwind leg fill failed for {close_id}: {e}");
            return;
        }
        let _ = state.state_store.update_signal_status(
            &close_id,
            crate::state_store::SignalStatus::Closed,
            None,
        );
    } else {
        // Unwind response without a verifiable fill is not a close —
        // leave the child failed rather than claiming flatness.
        let _ = state.state_store.update_signal_status(
            &close_id,
            crate::state_store::SignalStatus::Failed,
            Some("unwind response carried no verifiable fill"),
        );
    }
}

/// One-shot repair for signals recorded before unwind atomic records were
/// persisted: `LegAFailedUnwound`/`LegBFailedUnwound` failures kept their
/// real venue orders only as Debug text inside the signal note. Parses the
/// note back into `OrderResponse`s and replays them through
/// `persist_unwind_leg_records`, so the ledger carries the same rows the
/// live path now writes — the opportunity's atomic trail and its cash-flow
/// totals become truthful for dispatches made on earlier builds.
/// Idempotent: a `flat-{id}-` child or an entry leg already showing the
/// fill means the records exist (persisted live or by an earlier boot).
pub(crate) fn backfill_unwound_atomic_trails(state: &AppEngineState) {
    let signals = match state.state_store.list_signals(None, None) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("unwound-trail backfill: list signals failed: {e}");
            return;
        }
    };
    for sig in &signals {
        let Some(note) = sig.note.as_deref() else {
            continue;
        };
        let (side, fill_marker) = if note.starts_with("LegBFailedUnwound {") {
            (
                crate::state_store::LegSide::Long,
                "leg_a_fill: OrderResponse {",
            )
        } else if note.starts_with("LegAFailedUnwound {") {
            (
                crate::state_store::LegSide::Short,
                "leg_b_fill: OrderResponse {",
            )
        } else {
            continue;
        };
        let prefix = format!("flat-{}-", sig.id);
        let kids: Vec<_> = signals
            .iter()
            .filter(|s| s.id.starts_with(&prefix))
            .collect();
        if !kids.is_empty() {
            // Children already exist — but ones written before the
            // order-direction convention was fixed store the closed
            // POSITION side. Flip those legs to `close_side` (the unwind
            // order's direction) once; a correct row never matches.
            let close_side = match side {
                crate::state_store::LegSide::Long => crate::state_store::LegSide::Short,
                crate::state_store::LegSide::Short => crate::state_store::LegSide::Long,
            };
            for child in &kids {
                if child.legs.iter().any(|l| l.side == side) {
                    match state.state_store.repair_leg_side(&child.id, close_side) {
                        Ok(n) if n > 0 => tracing::info!(
                            "unwound-trail backfill: corrected leg side on {}",
                            child.id
                        ),
                        Ok(_) => {}
                        Err(e) => tracing::warn!(
                            "unwound-trail backfill: side repair failed on {}: {e}",
                            child.id
                        ),
                    }
                }
            }
            continue;
        }
        match (
            parse_debug_order_response(note, fill_marker),
            parse_debug_order_response(note, "unwind_resp: OrderResponse {"),
        ) {
            (Some(entry), Some(unwind)) => {
                persist_unwind_leg_records(state, &sig.id, side, &entry, &unwind);
                tracing::info!("unwound-trail backfill: repaired {}", sig.id);
            }
            _ => tracing::warn!(
                "unwound-trail backfill: note for {} did not parse — atomic trail stays note-only",
                sig.id
            ),
        }
    }
}

/// Extract one `OrderResponse { ... }` block from a coordinator failure
/// note's Debug text and rebuild the struct. Field scans are tolerant of
/// the embedded `raw_response` JSON — its keys are camelCase (`"orderId"`),
/// which can never collide with the snake_case Debug fields (`order_id:`).
fn parse_debug_order_response(
    note: &str,
    marker: &str,
) -> Option<arbitrageur_core::order::OrderResponse> {
    let start = note.find(marker)? + marker.len();
    let rest = &note[start..];
    // The block ends at the next sibling field of the failure variant.
    let end = [", unwind_resp:", ", error_a:", ", error_b:", ", error:"]
        .iter()
        .filter_map(|m| rest.find(m))
        .min()
        .unwrap_or(rest.len());
    let block = &rest[..end];

    let order_id = debug_str_field(block, "order_id")?.to_string();
    if order_id.is_empty() {
        return None;
    }
    let client_order_id = debug_str_field(block, "client_order_id")
        .unwrap_or_default()
        .to_string();
    let venue = {
        let i = block.find("venue: VenueId(\"")? + "venue: VenueId(\"".len();
        let end = block[i..].find('"')? + i;
        arbitrageur_core::types::VenueId(block[i..end].to_string())
    };
    let symbol = debug_str_field(block, "symbol")?.to_string();
    let status_word = debug_word_field(block, "status")?;
    let status = match status_word {
        "New" => arbitrageur_core::types::OrderStatus::New,
        "PartiallyFilled" => arbitrageur_core::types::OrderStatus::PartiallyFilled,
        "Filled" => arbitrageur_core::types::OrderStatus::Filled,
        "Cancelled" => arbitrageur_core::types::OrderStatus::Cancelled,
        "Rejected" => arbitrageur_core::types::OrderStatus::Rejected,
        "Expired" => arbitrageur_core::types::OrderStatus::Expired,
        _ => return None,
    };
    let filled_qty = debug_dec_field(block, "filled_qty")?;
    let original_qty = debug_dec_field(block, "original_qty").unwrap_or(filled_qty);
    let remaining_qty =
        debug_dec_field(block, "remaining_qty").unwrap_or_else(|| original_qty - filled_qty);
    let avg_fill_price = debug_opt_dec_field(block, "avg_fill_price");
    let fee = debug_opt_dec_field(block, "fee");
    let fee_currency = debug_str_field(block, "fee_currency")
        .unwrap_or("USD")
        .to_string();
    let timestamp = debug_word_field(block, "timestamp")
        .and_then(|w| chrono::DateTime::parse_from_rfc3339(w).ok())
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);
    Some(arbitrageur_core::order::OrderResponse {
        order_id,
        client_order_id,
        venue,
        symbol,
        status,
        original_qty,
        filled_qty,
        remaining_qty,
        avg_fill_price,
        fee,
        fee_currency,
        raw_response: None,
        timestamp,
    })
}

/// `key: "value"` inside a Debug struct dump. Requires a non-word char (or
/// start) before `key` so `order_id` can't match inside `client_order_id`.
fn debug_str_field<'a>(block: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("{key}: \"");
    let mut from = 0;
    loop {
        let i = block[from..].find(&pat)? + from;
        let boundary = i == 0
            || !block.as_bytes()[i - 1].is_ascii_alphanumeric() && block.as_bytes()[i - 1] != b'_';
        if boundary {
            let rest = &block[i + pat.len()..];
            let bytes = rest.as_bytes();
            let mut j = 0;
            while j < bytes.len() {
                match bytes[j] {
                    b'\\' => j += 2,
                    b'"' => break,
                    _ => j += 1,
                }
            }
            return Some(&rest[..j.min(bytes.len())]);
        }
        from = i + pat.len();
    }
}

/// `key: 123.45` — a bare Debug scalar, ended by `,`/` `/`}`.
fn debug_dec_field(block: &str, key: &str) -> Option<rust_decimal::Decimal> {
    debug_word_field(block, key)?.parse().ok()
}

/// `key: Some(123.45)` or `key: None`.
fn debug_opt_dec_field(block: &str, key: &str) -> Option<rust_decimal::Decimal> {
    let pat = format!("{key}: ");
    let i = block.find(&pat)? + pat.len();
    let rest = &block[i..];
    if rest.starts_with("None") {
        return None;
    }
    let inner = rest.strip_prefix("Some(")?;
    let end = inner.find(')')?;
    inner[..end].parse().ok()
}

/// `key: token` — a bare word/number/timestamp, ended by `,`/` `/`}`.
/// Same boundary rule as `debug_str_field`.
fn debug_word_field<'a>(block: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("{key}: ");
    let mut from = 0;
    loop {
        let i = block[from..].find(&pat)? + from;
        let boundary = i == 0
            || !block.as_bytes()[i - 1].is_ascii_alphanumeric() && block.as_bytes()[i - 1] != b'_';
        if boundary {
            let rest = &block[i + pat.len()..];
            let end = rest.find([',', ' ', '}']).unwrap_or(rest.len());
            return Some(&rest[..end]);
        }
        from = i + pat.len();
    }
}

#[allow(clippy::too_many_arguments)]
/// Test-facing wrapper over `execute_spatial_arbitrage_inner`.
#[cfg(test)]
pub async fn execute_spatial_arbitrage_impl(
    symbol: &str,
    buy_venue: &str,
    sell_venue: &str,
    order_qty: f64,
    direction: Option<String>,
    trigger_spread_bps: Option<f64>,
    baseline_mean_spread_bps: Option<f64>,
    position_step: Option<u32>,
    max_units: Option<u32>,
    nominal_hurdle_bps: Option<f64>,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    execute_spatial_arbitrage_inner(
        symbol,
        buy_venue,
        sell_venue,
        order_qty,
        direction,
        trigger_spread_bps,
        baseline_mean_spread_bps,
        position_step,
        max_units,
        nominal_hurdle_bps,
        None,
        "futures",
        None,
        None,
        state,
    )
    .await
}

/// Realized round-trip P&L of a filled-then-unwound leg: entry and unwind
/// fees plus the price move across the two fills. `entry_side` is the
/// filled leg's side — a buy unwound by a sell gains (unwind − entry) per
/// unit; a sell unwound by a buy gains (entry − unwind). Missing average
/// prices degrade to fee-only rather than inventing a price move.
pub(crate) fn unwind_round_trip_pnl(
    entry: &arbitrageur_core::order::OrderResponse,
    unwind: &arbitrageur_core::order::OrderResponse,
    entry_side: Side,
) -> Decimal {
    let fees = entry.fee.unwrap_or_default() + unwind.fee.unwrap_or_default();
    let qty = entry.filled_qty;
    let px_pnl = match (entry.avg_fill_price, unwind.avg_fill_price) {
        (Some(e), Some(u)) => match entry_side {
            Side::Buy => (u - e) * qty,
            Side::Sell => (e - u) * qty,
        },
        _ => dec!(0),
    };
    px_pnl - fees
}

/// Forecast attached to an entry for the durable calibration log.
/// `kind` fixes which event `win_probability` describes — only "net_win"
/// rows score against realized net P&L at close; "reversion" rows persist
/// for audit but are never scored against P&L (different event).
#[derive(Debug, Clone, Copy)]
pub(crate) struct EntryPrediction {
    pub kind: &'static str,
    pub win_probability: f64,
    /// Expected net bps under the same forecast, when produced.
    pub expected_net_bps: Option<f64>,
}

/// Cross-instrument leg resolution for tracked-group rows (index perp ↔
/// ETF perp). `symbol` remains the group's numéraire key — the ledger and
/// signal namespace stay group-level; every venue-facing concern (book
/// fetch, position check, leverage, order placement) uses the leg's own
/// instrument.
///
/// Quantities: `order_qty` is in GROUP units — the same unit the monitor's
/// converted books express. Each leg's wire quantity is
/// `order_qty × scale` (leg units per group unit), snapped on its own
/// venue's grid; the matched hedge can therefore differ between legs by
/// at most one venue step in notional.
#[derive(Debug, Clone)]
pub(crate) struct PairLegs {
    /// Hub/lane key the leg's RAW book persists under (the member's own
    /// watchlist key — "QQQ" under merge, "QQQ-USD" under separate).
    pub buy_key: String,
    pub sell_key: String,
    /// Venue-facing canonical symbol the adapter fetches and orders by.
    pub buy_symbol: String,
    pub sell_symbol: String,
    /// Group-unit price per leg-unit price — the learned conversion the
    /// monitor used to build the row (numéraire leg = 1.0). A QQQ leg on a
    /// NAS100 row carries ~40; converted books multiply price by scale
    /// and divide size by scale.
    pub buy_scale: f64,
    pub sell_scale: f64,
}

/// Scale a book into group units: price × scale, size ÷ scale. Linear —
/// top-of-book order and depth ordering are preserved.
fn group_scaled_book(book: &OrderBook, scale: Decimal) -> OrderBook {
    let mut b = book.clone();
    for l in b.bids.iter_mut().chain(b.asks.iter_mut()) {
        l.price *= scale;
        l.size /= scale;
    }
    b
}

/// Snap a two-leg order quantity onto BOTH venues' wire grids. Each adapter
/// independently normalizes to its own lot/contract/scale grid inside
/// `place_order`, so an un-snapped quantity can fill different sizes on the
/// two legs — a naked residual by construction. Trade the finer common
/// size; refuse when the grids are irreconcilable rather than desync legs.
pub(crate) async fn common_wire_qty(
    symbol: &str,
    requested: Decimal,
    buy_adapter: &Arc<dyn ExchangeAdapter>,
    sell_adapter: &Arc<dyn ExchangeAdapter>,
) -> Result<Decimal, String> {
    let buy_vid = buy_adapter.venue_id();
    let sell_vid = sell_adapter.venue_id();
    let buy_wire = buy_adapter
        .wire_base_qty(symbol, requested)
        .await
        .map_err(|e| format!("{buy_vid} cannot size {symbol} on its wire grid: {e}"))?;
    let sell_wire = sell_adapter
        .wire_base_qty(symbol, requested)
        .await
        .map_err(|e| format!("{sell_vid} cannot size {symbol} on its wire grid: {e}"))?;
    let common = buy_wire.min(sell_wire);
    if common <= dec!(0) {
        return Err(format!(
            "{symbol} order quantity {requested} snaps to zero on {buy_vid}/{sell_vid} wire grids"
        ));
    }
    let buy_check = buy_adapter
        .wire_base_qty(symbol, common)
        .await
        .map_err(|e| format!("{buy_vid} cannot size snapped quantity {common}: {e}"))?;
    let sell_check = sell_adapter
        .wire_base_qty(symbol, common)
        .await
        .map_err(|e| format!("{sell_vid} cannot size snapped quantity {common}: {e}"))?;
    if buy_check != common || sell_check != common {
        return Err(format!(
            "Irreconcilable {symbol} wire grids: {buy_vid} -> {buy_check}, {sell_vid} -> {sell_check} for qty {common}"
        ));
    }
    if common != requested {
        tracing::info!(
            symbol,
            venue_buy = %buy_vid,
            venue_sell = %sell_vid,
            requested = %requested,
            snapped_qty = %common,
            "order quantity snapped to common venue wire grid"
        );
    }
    Ok(common)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_spatial_arbitrage_inner(
    symbol: &str,
    buy_venue: &str,
    sell_venue: &str,
    order_qty: f64,
    direction: Option<String>,
    trigger_spread_bps: Option<f64>,
    baseline_mean_spread_bps: Option<f64>,
    position_step: Option<u32>,
    max_units: Option<u32>,
    nominal_hurdle_bps: Option<f64>,
    route_id: Option<&str>,
    strategy_id: &str,
    prediction: Option<EntryPrediction>,
    pair_legs: Option<&PairLegs>,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    if !order_qty.is_finite() || order_qty <= 0.0 {
        return Err("order_qty must be a finite positive number".into());
    }
    // Cross-instrument legs resolve once up front: `*_hubkey` for book
    // lookup, `*_order_sym` for adapter calls, scales for shared-unit
    // economics. Validation of scale sanity is mandatory — a corrupt
    // conversion silently misprices every leg.
    let legs = match pair_legs {
        Some(l) => {
            let ok = |s: f64| s.is_finite() && s > 0.0;
            if !(ok(l.buy_scale) && ok(l.sell_scale)) {
                return Err("cross-instrument leg scales must be finite and positive".into());
            }
            if l.buy_symbol.trim().is_empty() || l.sell_symbol.trim().is_empty() {
                return Err("cross-instrument legs need explicit venue symbols".into());
            }
            Some(l)
        }
        None => None,
    };
    let buy_hubkey = legs.map(|l| l.buy_key.as_str()).unwrap_or(symbol);
    let sell_hubkey = legs.map(|l| l.sell_key.as_str()).unwrap_or(symbol);
    let buy_order_sym = legs.map(|l| l.buy_symbol.as_str()).unwrap_or(symbol);
    let sell_order_sym = legs.map(|l| l.sell_symbol.as_str()).unwrap_or(symbol);
    let scale_buy = legs
        .and_then(|l| Decimal::from_f64_retain(l.buy_scale))
        .map(|d| d.round_dp(9))
        .unwrap_or(dec!(1));
    let scale_sell = legs
        .and_then(|l| Decimal::from_f64_retain(l.sell_scale))
        .map(|d| d.round_dp(9))
        .unwrap_or(dec!(1));
    if buy_venue.trim().is_empty() || sell_venue.trim().is_empty() {
        return Err("Buy and sell venues must both be specified".into());
    }
    if buy_venue.eq_ignore_ascii_case(sell_venue) {
        return Err("Buy and sell venues must be distinct".into());
    }
    validate_market_symbol(symbol)?;
    if let Some(h) = nominal_hurdle_bps {
        if !h.is_finite() || h < 0.0 {
            return Err("nominal_hurdle_bps must be finite and nonnegative".into());
        }
    }

    // Authoritative grid-cap gate. Effective cap = caller-supplied max_units,
    // else the ticker config's persisted max_accumulated_units (the modal's
    // Execution & Sizing section) when present. Open units are derived from
    // the durable signal ledger — matched (balanced) pairs on the symbol —
    // never a cached counter, so restarts and partial histories cannot drift
    // the cap. An entry whose declared step exceeds the cap, or a ticker
    // already holding `cap` open pairs, is rejected before any orderbook
    // work. in_flight/imbalanced/unresolved signals don't count here — the
    // durable blocking gate below hard-refuses those symbols outright.
    let configured_max = max_units.or_else(|| {
        state
            .strategy_configs
            .read()
            .get(symbol)
            .map(|c| c.max_accumulated_units)
            .filter(|m| *m >= 1)
    });
    if let Some(max) = configured_max {
        if max < 1 {
            return Err("max_units must be at least 1".into());
        }
        if let Some(step) = position_step {
            if step < 1 {
                return Err("position_step must be at least 1".into());
            }
            if step > max {
                return Err(format!(
                    "Grid cap reached for {symbol}: position_step {step} exceeds max_units {max} — entry rejected"
                ));
            }
        }
        let open_units = state
            .state_store
            .open_balanced_signal_count(symbol, Some(state.env().as_str()))
            .map_err(|e| e.to_string())?;
        if open_units >= i64::from(max) {
            return Err(format!(
                "Grid cap reached for {symbol}: {open_units}/{max} open pairs — entry rejected"
            ));
        }
    }

    let _execution_guard = state.execution_lock.lock().await;

    if let Err(reason) = state.circuit_breaker.is_trading_allowed() {
        return Err(format!(
            "Execution halted by Risk Circuit Breaker: {}",
            reason
        ));
    }
    if *state.is_paused.read() {
        return Err(
            "Trading engine is PAUSED. Positions are preserved, but new executions are suspended."
                .into(),
        );
    }
    let is_live = state.is_live();
    if is_live {
        live_entry_gate(state, &[buy_venue, sell_venue])?;
    }
    // Stablecoin depeg gate — applies to every environment, not just live:
    // a paper-mode spread measured against a depegged quote is the same
    // phantom edge. Stable-vs-USD books are exempt (they ARE the depeg
    // arbitrage — the base-is-family check lives inside the gate).
    for (v, q, k) in [
        (
            buy_venue,
            state.book_quote(buy_venue, buy_hubkey),
            buy_hubkey,
        ),
        (
            sell_venue,
            state.book_quote(sell_venue, sell_hubkey),
            sell_hubkey,
        ),
    ] {
        state
            .stablecoin_gate(k, &q)
            .map_err(|e| format!("{v}: {e}"))?;
    }
    let _ = trigger_spread_bps; // caller-supplied spread is never trusted for eligibility

    // Durable per-ticker gate: an in-flight, imbalanced, or unresolved signal
    // for this symbol blocks any further order until its own legs reconcile
    // as delta-neutral. Survives restarts via the SQLite store.
    if let Some(blocker) = state
        .state_store
        .blocking_signals(symbol, Some(state.env().as_str()))
        .map_err(|e| e.to_string())?
        .into_iter()
        .next()
    {
        return Err(format!(
            "Symbol {} is blocked by signal {} (status {}): awaiting leg reconciliation/delta-neutral confirmation",
            symbol,
            blocker.id,
            blocker.status.as_str()
        ));
    }
    // Durable exits-only flag — an unreadable flag blocks rather than
    // silently permitting an entry the operator marked reduce-only. The
    // flag namespace is the calling strategy's own id: a spot reduce-only
    // must not read the futures flag (and vice versa).
    if state
        .state_store
        .ticker_reduce_only(symbol, strategy_id, state.env().as_str())
        .map_err(|e| e.to_string())?
    {
        return Err(format!(
            "Symbol {symbol} is reduce-only for {strategy_id} — exits only, entry refused"
        ));
    }

    state.ensure_market(symbol);

    let buy_adapter = state
        .get_adapter(buy_venue)
        .ok_or_else(|| format!("Buy venue '{}' is not admitted for execution", buy_venue))?;
    let sell_adapter = state
        .get_adapter(sell_venue)
        .ok_or_else(|| format!("Sell venue '{}' is not admitted for execution", sell_venue))?;
    let buy_vid = buy_adapter.venue_id();
    let sell_vid = sell_adapter.venue_id();
    if crate::state::canonical_venue_id(buy_venue) == crate::state::canonical_venue_id(sell_venue) {
        return Err("Buy and sell venues must resolve to distinct canonical venues".into());
    }

    // Cash-session preflight: equity-rail legs (Alpaca shares, Ondo
    // tokenized perps) refuse IOC outside US market hours — dispatching
    // then is a guaranteed leg failure plus an unnecessary unwind of the
    // other leg. The adapter classifies the leg's own order symbol, so
    // 24/7 crypto on the same rails is unaffected.
    if !arbitrageur_math::session::us_cash_open(Utc::now().timestamp_millis())
        && (buy_adapter.cash_session_bound(buy_order_sym)
            || sell_adapter.cash_session_bound(sell_order_sym))
    {
        return Err(format!(
            "cash-equity leg outside US market hours — {buy_venue}/{sell_venue} would reject IOC outright"
        ));
    }

    // Order books must come from the hub cache (max age 2s) or a fresh fetch
    // through the admitted read-only adapter; ticker data is not a substitute.
    // Cached or fetched, a book must carry the exact requested venue and symbol
    // and pass execution validation before it is trusted or cached.
    let book_buy_raw = match state.market_data_hub.get_valid_orderbook(
        &buy_vid,
        buy_hubkey,
        Some(std::time::Duration::from_secs(2)),
    ) {
        Some(b) => b,
        None => {
            let b = buy_adapter
                .fetch_orderbook(buy_order_sym)
                .await
                .map_err(|e| {
                    format!(
                        "No fresh {} orderbook for {}: {}",
                        buy_venue, buy_order_sym, e
                    )
                })?;
            validate_adapter_book(&b, &buy_vid, buy_order_sym)
                .map_err(|e| format!("Buy-side order book rejected: {}", e))?;
            state.market_data_hub.update_orderbook(b.clone());
            b
        }
    };
    let book_sell_raw = match state.market_data_hub.get_valid_orderbook(
        &sell_vid,
        sell_hubkey,
        Some(std::time::Duration::from_secs(2)),
    ) {
        Some(b) => b,
        None => {
            let b = sell_adapter
                .fetch_orderbook(sell_order_sym)
                .await
                .map_err(|e| {
                    format!(
                        "No fresh {} orderbook for {}: {}",
                        sell_venue, sell_order_sym, e
                    )
                })?;
            validate_adapter_book(&b, &sell_vid, sell_order_sym)
                .map_err(|e| format!("Sell-side order book rejected: {}", e))?;
            state.market_data_hub.update_orderbook(b.clone());
            b
        }
    };
    validate_adapter_book(&book_buy_raw, &buy_vid, buy_order_sym)
        .map_err(|e| format!("Buy-side order book rejected: {}", e))?;
    validate_adapter_book(&book_sell_raw, &sell_vid, sell_order_sym)
        .map_err(|e| format!("Sell-side order book rejected: {}", e))?;
    // Shared-unit economics run on group-converted books (identity at
    // scale 1 for same-instrument pairs); venue-facing work — IOC bounds,
    // order symbols, signal-leg intended prices — reads the raw books.
    let book_buy = group_scaled_book(&book_buy_raw, scale_buy);
    let book_sell = group_scaled_book(&book_sell_raw, scale_sell);

    let qty_dec = Decimal::from_str_exact(&order_qty.to_string())
        .ok()
        .filter(|q| *q > dec!(0))
        .ok_or_else(|| "Invalid order quantity".to_string())?;

    // Leg quantities: `qty_dec` stays in group units for the shared-unit
    // economics below; `qty_buy_leg`/`qty_sell_leg` are the native-unit
    // wire quantities each venue actually receives.
    let (qty_dec, qty_buy_leg, qty_sell_leg) = match legs {
        Some(l) => {
            let req_b = qty_dec * scale_buy;
            let req_s = qty_dec * scale_sell;
            let wb = buy_adapter
                .wire_base_qty(&l.buy_symbol, req_b)
                .await
                .map_err(|e| {
                    format!(
                        "{buy_vid} cannot size {} on its wire grid: {e}",
                        l.buy_symbol
                    )
                })?;
            let ws = sell_adapter
                .wire_base_qty(&l.sell_symbol, req_s)
                .await
                .map_err(|e| {
                    format!(
                        "{sell_vid} cannot size {} on its wire grid: {e}",
                        l.sell_symbol
                    )
                })?;
            if wb <= dec!(0) || ws <= dec!(0) {
                return Err(format!(
                    "{symbol} leg quantity snaps to zero on {buy_vid}/{sell_vid} wire grids"
                ));
            }
            // Snapping can only shrink a leg — the effective group size is
            // the smaller of the two snapped legs in group units.
            let eff = (wb / scale_buy).min(ws / scale_sell);
            let qb = buy_adapter
                .wire_base_qty(&l.buy_symbol, eff * scale_buy)
                .await
                .map_err(|e| format!("{buy_vid} wire re-snap failed: {e}"))?;
            let qs = sell_adapter
                .wire_base_qty(&l.sell_symbol, eff * scale_sell)
                .await
                .map_err(|e| format!("{sell_vid} wire re-snap failed: {e}"))?;
            if qb <= dec!(0) || qs <= dec!(0) {
                return Err(format!(
                    "{symbol} leg quantity re-snaps to zero on {buy_vid}/{sell_vid} wire grids"
                ));
            }
            (eff, qb, qs)
        }
        None => {
            let q = common_wire_qty(symbol, qty_dec, &buy_adapter, &sell_adapter).await?;
            (q, q, q)
        }
    };
    let order_qty: f64 = qty_dec
        .to_string()
        .parse()
        .map_err(|_| "Snapped order quantity is not finite".to_string())?;

    // Slippage tolerance is the calling strategy's own parameter —
    // spot reads `spot_max_slippage_bps`, not the futures gate.
    let max_slippage_bps = strategy_slippage_bps(state, strategy_id);
    let min_depth_ratio = state.settings.read().min_depth_ratio;
    let max_slip_dec = Decimal::from_f64_retain(max_slippage_bps)
        .ok_or_else(|| "Invalid max slippage setting".to_string())?;
    let min_depth_dec = Decimal::from_f64_retain(min_depth_ratio)
        .ok_or_else(|| "Invalid min depth ratio setting".to_string())?;

    // Preflight must see exactly what the bounded IOC orders can reach:
    // retain only asks at or below the buy limit and bids at or above the
    // sell limit before evaluating slippage, depth ratio, and VWAP.
    let router = SmartOrderRouter::new(max_slip_dec);
    let buy_ref = book_buy
        .best_ask_price()
        .ok_or_else(|| format!("No asks on buy-side book for {}", symbol))?;
    let sell_ref = book_sell
        .best_bid_price()
        .ok_or_else(|| format!("No bids on sell-side book for {}", symbol))?;
    // Leg-native references for order bounds and signal intent — the raw
    // instrument's own top-of-book, not the group-converted price.
    let buy_ref_leg = book_buy_raw
        .best_ask_price()
        .ok_or_else(|| format!("No asks on buy-side book for {}", buy_order_sym))?;
    let sell_ref_leg = book_sell_raw
        .best_bid_price()
        .ok_or_else(|| format!("No bids on sell-side book for {}", sell_order_sym))?;
    let buy_limit = router.calculate_bounded_price(Side::Buy, buy_ref);
    let sell_limit = router.calculate_bounded_price(Side::Sell, sell_ref);

    let mut book_buy_exec = book_buy.clone();
    book_buy_exec.asks.retain(|l| l.price <= buy_limit);
    let mut book_sell_exec = book_sell.clone();
    book_sell_exec.bids.retain(|l| l.price >= sell_limit);

    arbitrageur_risk::safety_guards::verify_pre_execution_slippage(
        &book_buy_exec,
        Side::Buy,
        qty_dec,
        max_slip_dec,
    )
    .map_err(|v| {
        format!(
            "Buy-side slippage check failed on {}: VWAP {} vs top {} ({} bps, limit {} bps)",
            buy_venue, v.effective_vwap, v.top_price, v.slippage_bps, v.max_allowed_bps
        )
    })?;
    arbitrageur_risk::safety_guards::verify_pre_execution_slippage(
        &book_sell_exec,
        Side::Sell,
        qty_dec,
        max_slip_dec,
    )
    .map_err(|v| {
        format!(
            "Sell-side slippage check failed on {}: VWAP {} vs top {} ({} bps, limit {} bps)",
            sell_venue, v.effective_vwap, v.top_price, v.slippage_bps, v.max_allowed_bps
        )
    })?;
    arbitrageur_risk::safety_guards::verify_pre_execution_slippage(
        &book_buy,
        Side::Sell,
        qty_dec,
        max_slip_dec,
    )
    .map_err(|v| {
        format!(
            "Exit slippage check failed on {}: VWAP {} vs top {} ({} bps, limit {} bps)",
            buy_venue, v.effective_vwap, v.top_price, v.slippage_bps, v.max_allowed_bps
        )
    })?;
    arbitrageur_risk::safety_guards::verify_pre_execution_slippage(
        &book_sell,
        Side::Buy,
        qty_dec,
        max_slip_dec,
    )
    .map_err(|v| {
        format!(
            "Exit slippage check failed on {}: VWAP {} vs top {} ({} bps, limit {} bps)",
            sell_venue, v.effective_vwap, v.top_price, v.slippage_bps, v.max_allowed_bps
        )
    })?;

    let buy_depth: Decimal = book_buy_exec.asks.iter().map(|l| l.size).sum();
    let sell_depth: Decimal = book_sell_exec.bids.iter().map(|l| l.size).sum();
    let required_depth = qty_dec * min_depth_dec;
    if buy_depth < required_depth || sell_depth < required_depth {
        return Err(format!(
            "Insufficient order book depth ratio: required {} per leg ({}x order), buy side offers {}, sell side offers {}",
            required_depth, min_depth_dec, buy_depth, sell_depth
        ));
    }

    let (buy_vwap, buy_remaining, _) = book_buy_exec
        .buy_execution_vwap(qty_dec)
        .ok_or_else(|| format!("No executable buy depth on {}", buy_venue))?;
    let (sell_vwap, sell_remaining, _) = book_sell_exec
        .sell_execution_vwap(qty_dec)
        .ok_or_else(|| format!("No executable sell depth on {}", sell_venue))?;
    if buy_remaining > dec!(0) || sell_remaining > dec!(0) {
        return Err("Requested quantity exceeds executable book depth within IOC limits".into());
    }

    // Entry profit floor: each strategy reads its own configured hurdle —
    // spot entries must clear `spot_min_net_spread_bps` (the page's Min Net
    // param); the futures grid keeps its `target_profit_bps`.
    let target_profit_bps = match strategy_id {
        "spot" => state
            .strategy_params
            .read()
            .get("spot")
            .map(|p| p.spot_min_net_spread_bps)
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or_else(|| {
                crate::state::StrategyParamsDto::defaults_for("spot").spot_min_net_spread_bps
            }),
        _ => state
            .strategy_params
            .read()
            .get("futures")
            .map(|p| p.target_profit_bps)
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(1.0),
    };
    let econ = compute_route_economics(
        &book_buy,
        &book_sell,
        qty_dec,
        effective_venue_fee_bps(state, buy_vid.as_str()),
        effective_venue_fee_bps(state, sell_vid.as_str()),
        target_profit_bps,
    )
    .ok_or_else(|| {
        format!(
            "Route economics unavailable on {}/{}: the full quantity cannot be priced on all four legs",
            buy_venue, sell_venue
        )
    })?;
    // The profit floor is the configured target — no caller-provided
    // override: a negative floor over IPC would let any client push a live
    // order through a genuinely bad route.
    let profit_floor_bps = target_profit_bps;
    // Reversion entries (radar) forecast profit from the spread returning
    // to its band, not from the static top-of-book edge: on a pair whose
    // mean edge is biased, a negative static edge can still be a positive-
    // expectation reversion entry. When a baseline mean accompanies a
    // radar dispatch, the profit floor applies to the deviation net of
    // route friction (`expected_net − mu`) — the same convention the
    // monitor gates on. The static-edge plausibility ceiling below still
    // runs: a wild top-of-book gap is a symbol collision under either
    // model.
    let dev_net_bps = baseline_mean_spread_bps
        .filter(|m| strategy_id == "radar" && m.is_finite())
        .map(|mu| econ.expected_net_bps - mu);
    let effective_net_bps = dev_net_bps.unwrap_or(econ.expected_net_bps);
    if effective_net_bps < profit_floor_bps {
        return Err(format!(
            "Expected net spread {} BPS after route friction is below the {} BPS target",
            effective_net_bps, profit_floor_bps
        ));
    }
    // Toxic-fill ceiling: a net edge beyond the configured divergence cap is
    // not alpha — it's a symbol collision or a corrupt quote (observed in
    // production: SPX500 quoting ~147M bps across HL/Vanta). Real cross-venue
    // dislocations are a fraction of this; refuse outright.
    let max_net_bps = state.settings.read().cb_max_spread_divergence_bps;
    if max_net_bps.is_finite() && max_net_bps > 0.0 && econ.expected_net_bps > max_net_bps {
        return Err(format!(
            "Expected net spread {} BPS exceeds the {} BPS plausibility ceiling — likely a mismatched symbol or corrupt quote",
            econ.expected_net_bps, max_net_bps
        ));
    }
    // The durable expectation is the model the entry actually traded on:
    // static net edge for spatial paths, deviation-net for radar rows.
    let net_bps_f = effective_net_bps;
    let expected_net_usd = net_bps_f / 10_000.0
        * (((buy_vwap + sell_vwap) / dec!(2)) * qty_dec)
            .to_string()
            .parse::<f64>()
            .unwrap_or(0.0);

    state
        .delta_guard
        .read()
        .check_proposed_pair(
            symbol,
            Side::Buy,
            qty_dec,
            buy_vwap,
            symbol,
            Side::Sell,
            qty_dec,
            sell_vwap,
        )
        .map_err(|(asset, exposure)| {
            format!(
                "Delta neutrality breach on {}: proposed net exposure ${} exceeds limit",
                asset, exposure
            )
        })?;

    let margin_monitor = margin_monitor_from_settings(state);
    // Both environments read real venue accounts — the adapter is the
    // environment-scoped one (production or venue testnet).
    let acc_buy = buy_adapter
        .fetch_account()
        .await
        .map_err(|e| format!("Failed to fetch account on {}: {}", buy_venue, e))?;
    let acc_sell = sell_adapter
        .fetch_account()
        .await
        .map_err(|e| format!("Failed to fetch account on {}: {}", sell_venue, e))?;
    for (vname, acc) in [(buy_venue, &acc_buy), (sell_venue, &acc_sell)] {
        match margin_monitor.evaluate_account(acc) {
            MarginHealthStatus::Empty => {
                return Err(format!("Account {} has no funds; execution refused", vname));
            }
            MarginHealthStatus::Critical | MarginHealthStatus::Breached => {
                return Err(format!(
                    "Account {} margin status is critical/breached; execution refused",
                    vname
                ));
            }
            _ => {}
        }
    }
    // Effective order-time leverage: the strategy chain (strategy params,
    // then the per-symbol legacy fallback, then 5) lowered via min() by
    // each leg account's cap and the venue cap. Feeds both the IM
    // pre-checks below and the set_leverage calls after the books load.
    // Spot trades at 1x — no leverage negotiation applies.
    let is_spot = strategy_id == "spot";
    let (target_leverage, _account_cap) = if is_spot {
        (1u32, None)
    } else {
        resolve_effective_leverage(
            state,
            "futures",
            strategy_id,
            symbol,
            buy_venue,
            sell_venue,
            route_id,
        )?
    };
    if is_spot {
        // Spot cannot margin or short: the buy leg draws quote cash, the
        // sell leg draws base inventory. `AccountSnapshot::get_balance` is
        // venue-truthful free balance for both; a missing row reads as zero
        // — refusing here is strictly better than a mid-cycle venue reject.
        let (base_ccy, quote_ccy) = symbol
            .split_once('-')
            .ok_or_else(|| format!("Spot symbol {symbol} is not canonical BASE-QUOTE"))?;
        let buy_notional = buy_vwap * qty_dec;
        let buy_free = acc_buy
            .get_balance(quote_ccy)
            .map(|b| b.free)
            .unwrap_or(dec!(0));
        if buy_free < buy_notional {
            return Err(format!(
                "Insufficient free {quote_ccy} on {buy_venue} for the spot buy leg: required {buy_notional}, available {buy_free}"
            ));
        }
        let sell_free = acc_sell
            .get_balance(base_ccy)
            .map(|b| b.free)
            .unwrap_or(dec!(0));
        if sell_free < qty_dec {
            return Err(format!(
                "Insufficient free {base_ccy} inventory on {sell_venue} for the spot sell leg: required {qty_dec}, available {sell_free}"
            ));
        }
    } else {
        let leverage = Decimal::from(target_leverage);
        let im_buy = (buy_vwap * qty_dec) / leverage;
        if im_buy > acc_buy.free_margin {
            return Err(format!(
                "Insufficient free margin on {}: required {} > available {}",
                buy_venue, im_buy, acc_buy.free_margin
            ));
        }
        let im_sell = (sell_vwap * qty_dec) / leverage;
        if im_sell > acc_sell.free_margin {
            return Err(format!(
                "Insufficient free margin on {}: required {} > available {}",
                sell_venue, im_sell, acc_sell.free_margin
            ));
        }
        // Adapter-normalized position check — `get_position` compares raw
        // venue symbol spellings, which miss scaled/native mappings (e.g.
        // Kraken `PF_XBTUSD` vs engine `BTC-USD*`). `account_position_for`
        // resolves through `normalize_account_position`; Err means the
        // mapping is ambiguous — refuse rather than stack onto unmeasured
        // exposure. Spot skips this: base holdings ARE the sell-side
        // inventory, not unhedged exposure to refuse.
        for (vname, adapter, acc, leg_sym) in [
            (buy_venue, &buy_adapter, &acc_buy, buy_order_sym),
            (sell_venue, &sell_adapter, &acc_sell, sell_order_sym),
        ] {
            match adapter.account_position_for(acc, leg_sym) {
                Ok(Some(_)) => {
                    return Err(format!(
                        "An open position already exists for {} on {}; closing lifecycle is not implemented",
                        leg_sym, vname
                    ));
                }
                Ok(None) => {}
                Err(e) => {
                    return Err(format!(
                        "Cannot verify {vname} position for {leg_sym} ({e}); refusing entry"
                    ));
                }
            }
        }
    }

    if let Err(reason) = state.circuit_breaker.is_trading_allowed() {
        return Err(format!(
            "Execution halted by Risk Circuit Breaker: {}",
            reason
        ));
    }
    if *state.is_paused.read() {
        return Err(
            "Trading engine is PAUSED. Positions are preserved, but new executions are suspended."
                .into(),
        );
    }

    // Leverage set is best-effort in both environments: venues may reject
    // no-op changes or cap at account level. A rejected change is NOT fatal
    // (the order submits at existing leverage) but must be visible — an
    // operator reading "5x" while the venue kept 20x changes margin math.
    // Spot has no leverage concept — skip the calls entirely.
    if !is_spot {
        for (vname, adapter, leg_sym) in [
            (buy_venue, &buy_adapter, buy_order_sym),
            (sell_venue, &sell_adapter, sell_order_sym),
        ] {
            if let Err(e) = adapter.set_leverage(leg_sym, target_leverage).await {
                tracing::warn!(
                    "set_leverage {target_leverage}x on {vname} {leg_sym} failed: {e} — order will submit at the venue's existing leverage"
                );
            }
        }
    }

    let (buy_exec, sell_exec): (
        Arc<dyn arbitrageur_core::exchange::ExchangeAdapter>,
        Arc<dyn arbitrageur_core::exchange::ExchangeAdapter>,
    ) = (buy_adapter.clone(), sell_adapter.clone());

    let order_buy = router.build_ioc_order(
        buy_exec.venue_id(),
        buy_order_sym,
        Side::Buy,
        buy_ref_leg,
        qty_buy_leg,
    );
    let order_sell = router.build_ioc_order(
        sell_exec.venue_id(),
        sell_order_sym,
        Side::Sell,
        sell_ref_leg,
        qty_sell_leg,
    );

    // Pre-trade equity snapshot of the two traded venues — the baseline the
    // post-fill attribution refresh measures own-fill deltas against.
    // Keys are `{env}-{venue}` matching the balances poll's namespacing.
    let mut equity_before_by_venue = HashMap::new();
    let env_str = state.env().as_str();
    for (vid, acc) in [(buy_vid.as_str(), &acc_buy), (sell_vid.as_str(), &acc_sell)] {
        let mut acc_usd = (*acc).clone();
        state.normalize_account_usd(vid, &mut acc_usd);
        if let Ok(f) = acc_usd.equity.to_string().parse::<f64>() {
            if f.is_finite() {
                equity_before_by_venue.insert(format!("{env_str}-{vid}"), f);
            }
        }
    }

    // Persist the selected candidate as in_flight BEFORE dispatch: if the
    // process dies mid-execution the symbol stays blocked on restart until
    // the signal's own legs reconcile.
    let accounts = resolve_signal_accounts(
        state,
        strategy_id,
        buy_vid.as_str(),
        sell_vid.as_str(),
        route_id,
    )?;
    let signal_id = format!(
        "sig-{}-{:08x}",
        Utc::now().timestamp_millis(),
        rand::random::<u32>()
    );
    state
        .state_store
        .record_signal_execution(
            &crate::state_store::NewSignalExecution {
                id: signal_id.clone(),
                strategy_id: strategy_id.to_string(),
                symbol: symbol.to_string(),
                route_id: accounts.route_id,
                expected_net_usd: Some(expected_net_usd),
                note: None,
                legs: vec![
                    crate::state_store::NewSignalLeg {
                        account_id: accounts.long,
                        venue: buy_vid.to_string(),
                        side: crate::state_store::LegSide::Long,
                        // Cross-instrument legs persist their own venue
                        // symbol + leg-unit qty so flatten/reconciliation
                        // address the real instrument, never the group key.
                        symbol: legs.map(|l| l.buy_symbol.clone()),
                        intended_qty: qty_buy_leg.to_string().parse::<f64>().unwrap_or(order_qty),
                        intended_price: buy_ref_leg.to_string().parse::<f64>().ok(),
                        // The conversion the entry was sized under —
                        // delta/exposure reads divide it back out to
                        // compare legs in shared units.
                        unit_scale: legs.map(|l| l.buy_scale),
                    },
                    crate::state_store::NewSignalLeg {
                        account_id: accounts.short,
                        venue: sell_vid.to_string(),
                        side: crate::state_store::LegSide::Short,
                        symbol: legs.map(|l| l.sell_symbol.clone()),
                        intended_qty: qty_sell_leg.to_string().parse::<f64>().unwrap_or(order_qty),
                        intended_price: sell_ref_leg.to_string().parse::<f64>().ok(),
                        unit_scale: legs.map(|l| l.sell_scale),
                    },
                ],
            },
            state.env().as_str(),
        )
        .map_err(|e| format!("Failed to persist in-flight signal record: {e}"))?;

    // Calibration log: persist the forecast BEFORE dispatch so the row
    // provably predates the outcome it will be scored against. A failed
    // write is non-fatal — calibration is diagnostic, not gating.
    if let Some(pred) = prediction {
        if let Err(e) = state.state_store.record_signal_prediction(
            &signal_id,
            pred.kind,
            pred.win_probability,
            pred.expected_net_bps,
            strategy_id,
            symbol,
            &format!("{buy_vid}/{sell_vid}"),
            is_live,
        ) {
            tracing::warn!("persist signal prediction failed for {signal_id}: {e}");
        }
    }

    let started = std::time::Instant::now();
    // Leg-unit → group-unit weights for the coordinator's matched-fill
    // math: partial fills compare and trim in group units so a QQQ leg's
    // ~40 units hedge 1 group unit, not 1 index unit. None = same-unit
    // instruments (historical behavior).
    let leg_unit_weights = legs.map(|_| (dec!(1) / scale_buy, dec!(1) / scale_sell));
    let res = state
        .coordinator
        .execute_two_leg_scaled(
            &buy_exec,
            order_buy,
            &sell_exec,
            order_sell,
            leg_unit_weights,
        )
        .await;
    let latency_us = started.elapsed().as_micros() as u64;

    // Own fills move the expected-equity baselines forward so the next
    // attribution poll does not misread trade PnL as a manual flow. Runs in
    // both environments (testnet venue equity moves exactly like live).
    refresh_equity_expectations_after_fills(state, &equity_before_by_venue).await;

    // Per-attempt realized P&L comes from the coordinator's verified fill
    // accounting (fees + slippage included) — identical semantics in both
    // environments. The testnet and live accumulators stay separate so a
    // demo session can never masquerade as real-money performance.
    // PartialPairKept carries the closed trim's real P&L; unwound variants
    // carry a real round-trip cost — neither may fall into the 0.0 arm or
    // the accumulators and execution log understate losses.
    let pnl_f = match &res {
        ArbitrageExecutionResult::Success {
            realized_profit, ..
        }
        | ArbitrageExecutionResult::PartialPairKept {
            realized_profit, ..
        } => realized_profit.to_string().parse::<f64>().unwrap_or(0.0),
        ArbitrageExecutionResult::LegBFailedUnwound {
            leg_a_fill,
            unwind_resp,
            ..
        } => unwind_round_trip_pnl(leg_a_fill, unwind_resp, Side::Buy)
            .to_string()
            .parse::<f64>()
            .unwrap_or(0.0),
        ArbitrageExecutionResult::LegAFailedUnwound {
            leg_b_fill,
            unwind_resp,
            ..
        } => unwind_round_trip_pnl(leg_b_fill, unwind_resp, Side::Sell)
            .to_string()
            .parse::<f64>()
            .unwrap_or(0.0),
        _ => 0.0,
    };
    let tot_f = accumulate_realized_profit(state, is_live, pnl_f);
    let (attempt_pnl_f, total_f) = (pnl_f, tot_f);

    let cycle = state.next_sim_step();

    // Partial-fill trim attribution: the coordinator trimmed the larger leg
    // down to min(fill_a, fill_b) via the unwind path on the excess only and
    // kept the matched portion open as a delta-neutral pair. The excess was
    // already unwound on the paper venues (paper equity reflects it), so the
    // ledger cycle and the signal fills below must cover exactly
    // `matched_qty` — attributing the raw fills would reconcile Imbalanced
    // and strand the slot. A trim whose residual unwind failed never reaches
    // this arm: the coordinator reports UnresolvedExposure and trips.
    let mut open_qty = order_qty;
    let mut trim_note = String::new();
    // The trim slice's total contribution to this cycle (excess leg price
    // delta + trim fees + the excess share of entry fees) — recorded on the
    // lot via `lot.trim_usd.{id}` so `close_flattened_spatial_lot` folds it
    // into the durable realized PnL/bps and the prediction resolution.
    let mut entry_trim_usd: Option<f64> = None;
    let res = match res {
        ArbitrageExecutionResult::PartialPairKept {
            leg_a_fill,
            leg_b_fill,
            matched_qty,
            unwind_a,
            unwind_b,
            realized_profit,
        } => {
            let matched_f = dec_to_f64(matched_qty, "trimmed matched quantity")?;
            if matched_f <= 0.0 {
                return Err("trimmed matched quantity is not positive".to_string());
            }
            open_qty = matched_f;
            let describe_trim = |o: &Option<Box<arbitrageur_core::order::OrderResponse>>| {
                o.as_ref().map_or_else(
                    || "no trim (leg equals matched quantity)".to_string(),
                    |r| format!("trimmed excess via order {}", r.order_id),
                )
            };
            trim_note = format!(
                "Partial fills trimmed to matched {matched_qty} (A: {}; B: {}); ",
                describe_trim(&unwind_a),
                describe_trim(&unwind_b),
            );
            // Coordinator semantics: realized = trim_pnl − entry_fees −
            // trim_fee, so realized + entry_fees isolates the trim slice
            // (price delta − trim fee) that belongs on the lot record.
            let trim_slice = realized_profit
                + leg_a_fill.fee.unwrap_or_default()
                + leg_b_fill.fee.unwrap_or_default();
            entry_trim_usd = dec_to_f64(trim_slice, "entry trim realized").ok();
            // `matched_qty` is in group units (leg-unit × weight); each
            // leg's durable fill must read its own venue units.
            let mut a = leg_a_fill;
            a.filled_qty = matched_qty * scale_buy;
            a.remaining_qty = dec!(0);
            a.original_qty = matched_qty * scale_buy;
            let mut b = leg_b_fill;
            b.filled_qty = matched_qty * scale_sell;
            b.remaining_qty = dec!(0);
            b.original_qty = matched_qty * scale_sell;
            // Realized cash P&L stays fee outflow only (entry + trim fees are
            // already in paper equity); the matched pair itself is unrealized.
            ArbitrageExecutionResult::Success {
                realized_profit: -(a.fee.unwrap_or_default() + b.fee.unwrap_or_default()),
                leg_a: a,
                leg_b: b,
            }
        }
        other => other,
    };

    match res {
        ArbitrageExecutionResult::Success {
            leg_a,
            leg_b,
            realized_profit: _,
        } => {
            let fees_dec = leg_a.fee.unwrap_or_default() + leg_b.fee.unwrap_or_default();
            let fee_f = fees_dec.to_string().parse::<f64>().unwrap_or(0.0);
            // Lot bookkeeping is per-leg: fills and prices stay in each
            // instrument's own units. The converted VWAPs are group-unit
            // prices — dividing by the leg scale restores leg units, and
            // each leg's persisted qty is its native filled quantity.
            let buy_px_f = leg_a
                .avg_fill_price
                .unwrap_or(buy_vwap / scale_buy)
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0);
            let sell_px_f = leg_b
                .avg_fill_price
                .unwrap_or(sell_vwap / scale_sell)
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0);
            let long_lot_qty = leg_a
                .filled_qty
                .to_string()
                .parse::<f64>()
                .unwrap_or(open_qty);
            let short_lot_qty = leg_b
                .filled_qty
                .to_string()
                .parse::<f64>()
                .unwrap_or(open_qty);
            let expected_capture_f = expected_net_usd;

            let cycle_id = format!("spatial-{}", cycle);
            {
                let mut ledger = state.virtual_ledger.write();
                ledger.open_cycle(
                    cycle_id.clone(),
                    crate::attribution::ledger_strategy_id(strategy_id),
                    symbol.to_string(),
                    buy_venue.to_string(),
                    sell_venue.to_string(),
                    net_bps_f,
                    long_lot_qty,
                    buy_px_f,
                    short_lot_qty,
                    sell_px_f,
                    fee_f,
                    state.env().as_str().to_string(),
                );
                // Durable lot (schema v30): without persistence a restart
                // loses close-matching, realized PnL, and prediction
                // resolution for this position. Fail-closed like the
                // signal-execution persist — an unaccountable position
                // must not be entered.
                if let Some(lot) = ledger.lots.get(&cycle_id) {
                    state
                        .state_store
                        .upsert_cycle_lot(lot)
                        .map_err(|e| format!("Failed to persist cycle lot: {e}"))?;
                }
                // Entry-time partial-pair trim: its realized slice already
                // debited the counter via the coordinator's realized_profit;
                // write the lot's trim mark so `close_flattened_spatial_lot`
                // folds it into the durable realized PnL (same convention as
                // `realize_liq_trim` — the close only prices the remaining
                // matched qty and would otherwise drop the trim outcome).
                if let Some(usd) = entry_trim_usd.filter(|u| u.is_finite() && *u != 0.0) {
                    let key = format!("lot.trim_usd.{cycle_id}");
                    let prior: f64 = state
                        .state_store
                        .kv_get(&key)
                        .ok()
                        .flatten()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0.0);
                    if let Err(e) = state.state_store.kv_set(&key, &(prior + usd).to_string()) {
                        tracing::warn!("persist entry-trim lot mark {key} failed: {e}");
                    }
                }
            }

            // Persist actual leg fills, then reconcile per-signal balance:
            // balanced only when THIS signal's own legs net to zero delta.
            let signal_status = reconcile_signal_legs(
                state,
                &signal_id,
                [
                    (crate::state_store::LegSide::Long, &leg_a),
                    (crate::state_store::LegSide::Short, &leg_b),
                ],
            )?;
            state
                .state_store
                .update_signal_status(&signal_id, signal_status, None)
                .map_err(|e| e.to_string())?;

            // Position-aware exit support: persist the entry edge baseline
            // for "futures" opens so the convergence check can later decide
            // converge/stop/timeout unwinds through the flatten path.
            // Best-effort AFTER fills are durable: a baseline-write failure
            // must never rewrite an executed open as failed; the slot then
            // stays under manual-flatten control and the check reports
            // Unwatched instead of inventing a baseline.
            let mut baseline_note = String::new();
            if strategy_id == "futures" {
                let row = crate::state_store::SpatialEntryBaseline {
                    signal_id: signal_id.clone(),
                    symbol: symbol.to_string(),
                    buy_venue: buy_vid.to_string(),
                    sell_venue: sell_vid.to_string(),
                    entry_net_bps: net_bps_f,
                    baseline_bps: baseline_mean_spread_bps,
                    entry_ts_ms: Utc::now().timestamp_millis(),
                };
                if let Err(e) = state.state_store.record_spatial_baseline(&row) {
                    baseline_note = format!(
                        " Entry baseline not persisted ({e}); convergence auto-check unavailable, manual flatten only."
                    );
                }
            }

            record_execution_log(state, ExecutionLogEntry {
                id: format!("exec-spatial-{}", cycle),
                timestamp: Utc::now().to_rfc3339(),
                trade_type: "Spatial".into(),
                strategy_id: crate::attribution::attribute_strategy_id(strategy_id),
                symbol: symbol.to_string(),
                venue_a: buy_venue.to_string(),
                venue_b: sell_venue.to_string(),
                leg_a_status: format!("{:?}", leg_a.status),
                leg_b_status: format!("{:?}", leg_b.status),
                success: true,
                gross_profit: 0.0,
                fees: fee_f,
                net_profit: attempt_pnl_f,
                latency_us,
                note: format!(
                    "{trim_note}{} position opened; expected convergence capture ${:.2} is unrealized{baseline_note}",
                    if is_live { "Live" } else { "Testnet" },
                    expected_capture_f
                ),
                direction: direction
                    .clone()
                    .or_else(|| Some("Divergence Entry".into())),
                trigger_spread_bps: Some(net_bps_f),
                baseline_mean_spread_bps,
                position_step: position_step.or(Some(1)),
                max_units: max_units.or(Some(3)),
                nominal_hurdle_bps,
                is_live,
            });

            maybe_record_margin_snapshots(state, Utc::now().timestamp_millis()).await;

            Ok(SimStepResultDto {
                cycle,
                opportunity_found: true,
                execution_success: true,
                trade_type: "Spatial Arb".into(),
                direction: direction.unwrap_or_else(|| "BuyASellB".into()),
                venue_a: buy_venue.to_string(),
                venue_b: sell_venue.to_string(),
                spread_bps: net_bps_f,
                realized_profit_usd: attempt_pnl_f,
                total_cumulative_pnl_usd: total_f,
                message: format!(
                    "{trim_note}{} position opened on {}/{}: realized P&L ${:.2}; expected convergence capture ${:.2} is unrealized",
                    if is_live { "Live" } else { "Testnet" },
                    buy_venue,
                    sell_venue,
                    attempt_pnl_f,
                    expected_capture_f
                ),
            })
        }
        ArbitrageExecutionResult::UnresolvedExposure {
            leg_a_fill,
            leg_b_fill,
            error_a,
            error_b,
        } => {
            // Persist whatever partial fill data exists, then mark the signal
            // unresolved — the ticker stays blocked until manual resolution.
            for (side, fill) in [
                (crate::state_store::LegSide::Long, &leg_a_fill),
                (crate::state_store::LegSide::Short, &leg_b_fill),
            ] {
                if let Some(f) = fill {
                    let filled = f.filled_qty.to_string().parse::<f64>().unwrap_or(0.0);
                    let px = f
                        .avg_fill_price
                        .and_then(|p| p.to_string().parse::<f64>().ok())
                        .unwrap_or(0.0);
                    let fee = f
                        .fee
                        .map(|v| v.to_string().parse::<f64>().unwrap_or(0.0))
                        .unwrap_or(0.0);
                    // Keep the venue order id reachable even when the leg was
                    // accepted but unfilled — reconciliation polls it to
                    // terminal state instead of losing the exposure.
                    if !f.order_id.is_empty() {
                        let cid =
                            (!f.client_order_id.is_empty()).then_some(f.client_order_id.as_str());
                        if let Err(e) = state.state_store.record_leg_order_id(
                            &signal_id,
                            side,
                            f.order_id.as_str(),
                            cid,
                        ) {
                            // A lost order id orphans a live venue order —
                            // louder than the surrounding `let _` style.
                            tracing::error!(
                                "persist venue order id failed for {signal_id} {side:?}: {e}"
                            );
                        }
                    }
                    if filled > 0.0 && px > 0.0 {
                        if let Err(e) = state.state_store.apply_leg_fill(
                            &signal_id,
                            side,
                            filled,
                            px,
                            fee,
                            Some(f.order_id.as_str()),
                        ) {
                            tracing::error!(
                                "persist leg fill failed for {signal_id} {side:?}: {e}"
                            );
                        }
                    }
                }
            }
            let _ = state.state_store.update_signal_status(
                &signal_id,
                crate::state_store::SignalStatus::Unresolved,
                Some(&format!(
                    "unresolved exposure — leg A: {error_a}; leg B: {error_b}"
                )),
            );
            if !state.circuit_breaker.is_tripped() {
                state
                    .circuit_breaker
                    .trip("Unresolved exposure after failed unwind");
            }
            let describe = |f: &Option<arbitrageur_core::order::OrderResponse>| {
                f.as_ref()
                    .map(|r| format!("{:?} filled {}/{}", r.status, r.filled_qty, r.original_qty))
                    .unwrap_or_else(|| "no fill".to_string())
            };
            record_execution_log(state, ExecutionLogEntry {
                id: format!("exec-spatial-{}", cycle),
                timestamp: Utc::now().to_rfc3339(),
                trade_type: "Spatial".into(),
                strategy_id: crate::attribution::attribute_strategy_id(strategy_id),
                symbol: symbol.to_string(),
                venue_a: buy_venue.to_string(),
                venue_b: sell_venue.to_string(),
                leg_a_status: describe(&leg_a_fill),
                leg_b_status: describe(&leg_b_fill),
                success: false,
                gross_profit: 0.0,
                fees: 0.0,
                net_profit: attempt_pnl_f,
                latency_us,
                note: format!(
                    "UNRESOLVED EXPOSURE: leg A [{}]; leg B [{}]; realized P&L to date ${:.2}; open exposure not marked",
                    error_a, error_b, attempt_pnl_f
                ),
                direction: direction.clone().or_else(|| Some("Divergence Entry".into())),
                trigger_spread_bps: Some(net_bps_f),
                baseline_mean_spread_bps,
                position_step: position_step.or(Some(1)),
                max_units: max_units.or(Some(3)),
                nominal_hurdle_bps,
                is_live,
            });
            Err(format!(
                "UNRESOLVED EXPOSURE: leg A [{}]; leg B [{}]",
                error_a, error_b
            ))
        }
        other => {
            let (status_a, status_b) = match &other {
                ArbitrageExecutionResult::LegBFailedUnwound { .. } => ("UNWOUND", "REJECTED"),
                ArbitrageExecutionResult::LegAFailedUnwound { .. } => ("REJECTED", "UNWOUND"),
                ArbitrageExecutionResult::HaltedByRisk(_) => ("HALTED", "HALTED"),
                _ => ("REJECTED", "REJECTED"),
            };
            // Persist the true atomic trail BEFORE the failed mark: the
            // filled leg's entry fill plus its compensation unwind are real
            // venue orders — as leg rows, not only raw JSON in the note.
            match &other {
                ArbitrageExecutionResult::LegBFailedUnwound {
                    leg_a_fill,
                    unwind_resp,
                    ..
                } => persist_unwind_leg_records(
                    state,
                    &signal_id,
                    crate::state_store::LegSide::Long,
                    leg_a_fill,
                    unwind_resp,
                ),
                ArbitrageExecutionResult::LegAFailedUnwound {
                    leg_b_fill,
                    unwind_resp,
                    ..
                } => persist_unwind_leg_records(
                    state,
                    &signal_id,
                    crate::state_store::LegSide::Short,
                    leg_b_fill,
                    unwind_resp,
                ),
                _ => {}
            }
            // Unwound legs restored flatness → `failed` (not blocking). Any
            // other terminal failure is also non-blocking once recorded.
            let _ = state.state_store.update_signal_status(
                &signal_id,
                crate::state_store::SignalStatus::Failed,
                Some(&format!("{other:?}")),
            );
            record_execution_log(
                state,
                ExecutionLogEntry {
                    id: format!("exec-spatial-{}", cycle),
                    timestamp: Utc::now().to_rfc3339(),
                    trade_type: "Spatial".into(),
                    strategy_id: crate::attribution::attribute_strategy_id(strategy_id),
                    symbol: symbol.to_string(),
                    venue_a: buy_venue.to_string(),
                    venue_b: sell_venue.to_string(),
                    leg_a_status: status_a.into(),
                    leg_b_status: status_b.into(),
                    success: false,
                    gross_profit: 0.0,
                    fees: 0.0,
                    net_profit: attempt_pnl_f,
                    latency_us,
                    note: format!("Atomic trade failed: {:?}", other),
                    direction: direction
                        .clone()
                        .or_else(|| Some("Divergence Entry".into())),
                    trigger_spread_bps: Some(net_bps_f),
                    baseline_mean_spread_bps,
                    position_step: position_step.or(Some(1)),
                    max_units: max_units.or(Some(3)),
                    nominal_hurdle_bps,
                    is_live,
                },
            );

            Ok(SimStepResultDto {
                cycle,
                opportunity_found: true,
                execution_success: false,
                trade_type: "Spatial Arb".into(),
                direction: direction.unwrap_or_else(|| "BuyASellB".into()),
                venue_a: buy_venue.to_string(),
                venue_b: sell_venue.to_string(),
                spread_bps: net_bps_f,
                realized_profit_usd: attempt_pnl_f,
                total_cumulative_pnl_usd: total_f,
                message: format!("Execution failed/unwound: {:?}", other),
            })
        }
    }
}

/* ------------------------------------------------------------------ */
/* Same-time candidate arbitration                                      */
/* ------------------------------------------------------------------ */

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpatialCandidateInput {
    pub symbol: String,
    pub buy_venue: String,
    pub sell_venue: String,
    pub order_qty: f64,
    /// Caller's estimated net USD — used only for ranking candidates in the
    /// same batch; the executor recomputes economics before dispatch.
    pub expected_net_usd: Option<f64>,
    /// Explicit strategy-level route override (account A ↔ account B).
    pub route_id: Option<String>,
    pub direction: Option<String>,
    /// Grid-cap context threaded through to the execution gate: the entry's
    /// 1-based step and the derived per-ticker max. Both default to None so
    /// older callers keep working (gate skipped when max_units is None).
    #[serde(default)]
    pub position_step: Option<u32>,
    #[serde(default)]
    pub max_units: Option<u32>,
    /// Scan-time reversion probability for this candidate — persisted to
    /// the calibration log when the candidate dispatches. Scan computes
    /// reversion semantics only, so the recorded kind is "reversion".
    #[serde(default)]
    pub predicted_win_probability: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateOutcomeDto {
    pub symbol: String,
    pub buy_venue: String,
    pub sell_venue: String,
    pub expected_net_usd: Option<f64>,
    pub selected: bool,
    pub reason: Option<String>,
    pub result: Option<SimStepResultDto>,
}

/// Submit a synchronized batch of candidates: per symbol, at most ONE order
/// is dispatched — the candidate with the highest expected net USD. Symbols
/// already blocked by an in-flight/unreconciled signal reject all candidates.
pub async fn execute_spatial_candidates_impl(
    candidates: Vec<SpatialCandidateInput>,
    auto: bool,
    state: &AppEngineState,
) -> Result<Vec<CandidateOutcomeDto>, String> {
    // Same autonomous-entry refusal as the funding/triangle paths: batch
    // dispatch must not become the live-autonomy loophole.
    if auto && state.is_live() {
        return Err(
            "autonomous spatial entries are testnet-only — run a manual Execute for live".into(),
        );
    }
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let mut by_symbol: std::collections::HashMap<String, Vec<SpatialCandidateInput>> =
        std::collections::HashMap::new();
    for c in candidates {
        let key = c.symbol.trim().to_lowercase();
        by_symbol.entry(key).or_default().push(c);
    }

    let mut outcomes = Vec::new();
    for (symbol_key, mut cands) in by_symbol {
        let outcome_for =
            |c: &SpatialCandidateInput,
             selected: bool,
             reason: Option<String>,
             result: Option<SimStepResultDto>| CandidateOutcomeDto {
                symbol: c.symbol.clone(),
                buy_venue: c.buy_venue.clone(),
                sell_venue: c.sell_venue.clone(),
                expected_net_usd: c.expected_net_usd,
                selected,
                reason,
                result,
            };

        if state
            .state_store
            .ticker_blocked(&symbol_key, Some(state.env().as_str()))
            .map_err(|e| e.to_string())?
        {
            for c in &cands {
                outcomes.push(outcome_for(
                    c,
                    false,
                    Some("ticker blocked by in-flight/unreconciled signal".into()),
                    None,
                ));
            }
            continue;
        }
        if state
            .state_store
            .ticker_reduce_only(&symbol_key, "futures", state.env().as_str())
            .map_err(|e| e.to_string())?
        {
            for c in &cands {
                outcomes.push(outcome_for(
                    c,
                    false,
                    Some("ticker is reduce-only — exits only, entry refused".into()),
                    None,
                ));
            }
            continue;
        }

        // Bot targeting honors the active strategy's auto-filters before any
        // economics ranking: filtered tickers lose with an explicit reason.
        // The execution-relevant scope is the candidate's own venue pair.
        let base = crate::monitor::canonical_base(&symbol_key);
        let mut auto_block: Option<String> = None;
        for c in &cands {
            let mut scope = vec![c.buy_venue.clone(), c.sell_venue.clone()];
            scope.sort();
            scope.dedup();
            if let Err(reason) = reject_if_strategy_filtered(state, "futures", &base, &scope).await
            {
                auto_block = Some(reason);
                break;
            }
        }
        if let Some(reason) = auto_block {
            for c in &cands {
                outcomes.push(outcome_for(c, false, Some(reason.clone()), None));
            }
            continue;
        }

        // Highest expected net USD wins; missing estimates rank last.
        cands.sort_by(|a, b| {
            b.expected_net_usd
                .unwrap_or(f64::NEG_INFINITY)
                .total_cmp(&a.expected_net_usd.unwrap_or(f64::NEG_INFINITY))
        });
        let winner = cands.remove(0);
        for loser in &cands {
            outcomes.push(outcome_for(
                loser,
                false,
                Some("not selected: lower expected net USD than winning candidate".into()),
                None,
            ));
        }

        let res = execute_spatial_arbitrage_inner(
            &winner.symbol,
            &winner.buy_venue,
            &winner.sell_venue,
            winner.order_qty,
            winner.direction.clone(),
            None,
            None,
            winner.position_step,
            winner.max_units,
            None,
            winner.route_id.as_deref(),
            "futures",
            winner.predicted_win_probability.map(|p| EntryPrediction {
                kind: "reversion",
                win_probability: p,
                expected_net_bps: None,
            }),
            None,
            state,
        )
        .await;
        match res {
            Ok(dto) => outcomes.push(outcome_for(&winner, true, None, Some(dto))),
            Err(e) => outcomes.push(outcome_for(&winner, true, Some(e), None)),
        }
    }
    Ok(outcomes)
}

/// Durable signal execution record for the frontend (symbol filter optional).
#[tauri::command]
pub async fn list_signal_executions(
    symbol: Option<String>,
    status: Option<String>,
    state: tauri::State<'_, Arc<AppEngineState>>,
) -> Result<Vec<crate::state_store::SignalExecution>, String> {
    let st = match status.as_deref() {
        Some(s) => Some(
            crate::state_store::SignalStatus::from_status(s)
                .ok_or_else(|| format!("unknown signal status '{s}'"))?,
        ),
        None => None,
    };
    state
        .state_store
        .list_signals(symbol.as_deref(), st)
        .map_err(|e| e.to_string())
}

/// One atomic venue order of a traded opportunity — a persisted signal leg
/// with its dispatch role derived from the owning signal's id (`entry` for
/// the root signal, `flatten`/`trim` for close-family children).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AtomicTransactionDto {
    pub leg_id: String,
    pub signal_id: String,
    pub role: &'static str,
    pub venue: String,
    pub account_id: Option<String>,
    /// Position side persisted on the leg row.
    pub side: &'static str,
    /// Order side — derived from role+side (entry longs buy, close longs sell).
    pub order_side: &'static str,
    /// Venue-facing instrument this leg actually ordered.
    pub symbol: String,
    pub intended_qty: f64,
    pub filled_qty: f64,
    pub intended_price: Option<f64>,
    pub fill_price: Option<f64>,
    pub fee_usd: f64,
    /// Leg venue-units per unit of the parent symbol (tracked-group scale).
    pub unit_scale: f64,
    pub venue_order_id: Option<String>,
    pub client_order_id: Option<String>,
    /// Aggregate fill state of this order: none/partial/filled.
    pub fill_state: &'static str,
    pub created_at: String,
    pub updated_at: String,
}

/// One traded opportunity: the root dispatch plus every close-family child
/// dispatch (flatten/trim) and the matched virtual-ledger cycle lot when one
/// exists. Realized PnL comes from the closed lot (authoritative); for
/// terminal families without a lot the persisted leg cash-flow stands in —
/// open positions report `null` rather than a fabricated mark.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TradedOpportunityDto {
    pub signal_id: String,
    pub strategy_id: String,
    pub symbol: String,
    pub environment: String,
    pub status: &'static str,
    pub route_id: Option<String>,
    pub expected_net_usd: Option<f64>,
    pub note: Option<String>,
    pub venue_long: Option<String>,
    pub venue_short: Option<String>,
    pub created_at: String,
    pub closed_at: Option<String>,
    pub lot_id: Option<String>,
    pub lot_status: Option<&'static str>,
    pub entry_spread_bps: Option<f64>,
    pub close_spread_bps: Option<f64>,
    pub realized_pnl_usd: Option<f64>,
    pub realized_pnl_bps: Option<f64>,
    /// Signed cash-flow across all family fills (sells − buys − fees).
    pub leg_net_flow_usd: f64,
    pub total_fees_usd: f64,
    pub child_count: usize,
    pub transactions: Vec<AtomicTransactionDto>,
}

/// All opportunities actually dispatched by the system — the durable signal
/// ledger, environment-scoped like every other exposure read (`unknown`
/// stays visible: unattributable exposure must never hide). Radar rows that
/// never dispatched produce no signal record and correctly never appear.
#[tauri::command]
pub async fn list_traded_opportunities(
    state: tauri::State<'_, Arc<AppEngineState>>,
) -> Result<Vec<TradedOpportunityDto>, String> {
    let env = state.env().as_str().to_string();
    let signals = state
        .state_store
        .list_signals(None, None)
        .map_err(|e| e.to_string())?;
    let lots = state.virtual_ledger.read().lots.clone();
    Ok(build_traded_opportunities(&env, &signals, &lots))
}

/// Group durable signals into traded opportunities for one environment.
/// Pure over the persisted rows — unit-testable without engine state.
pub(crate) fn build_traded_opportunities(
    env: &str,
    signals: &[crate::state_store::SignalExecution],
    lots: &std::collections::HashMap<
        String,
        arbitrageur_execution::virtual_ledger::ArbitrageCycleLot,
    >,
) -> Vec<TradedOpportunityDto> {
    let scoped: Vec<&crate::state_store::SignalExecution> = signals
        .iter()
        .filter(|s| s.environment == "unknown" || s.environment == env)
        .collect();
    let roots: Vec<&crate::state_store::SignalExecution> = scoped
        .iter()
        .copied()
        .filter(|s| !s.id.starts_with("flat-") && !s.id.starts_with("trim-"))
        .collect();
    let children_of = |root_id: &str| -> Vec<&crate::state_store::SignalExecution> {
        let flat_prefix = format!("flat-{root_id}-");
        let trim_prefix = format!("trim-{root_id}-");
        let mut kids: Vec<&crate::state_store::SignalExecution> = scoped
            .iter()
            .copied()
            .filter(|s| s.id.starts_with(&flat_prefix) || s.id.starts_with(&trim_prefix))
            .collect();
        kids.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        kids
    };
    let role_of = |sig_id: &str| -> &'static str {
        if sig_id.starts_with("flat-") {
            "flatten"
        } else if sig_id.starts_with("trim-") {
            "trim"
        } else {
            "entry"
        }
    };
    let mut out = Vec::with_capacity(roots.len());
    for root in roots {
        let children = children_of(&root.id);
        let mut txs: Vec<AtomicTransactionDto> = Vec::new();
        let mut net_flow = 0.0f64;
        let mut fees = 0.0f64;
        let mut buy_filled = false;
        let mut sell_filled = false;
        for sig in std::iter::once(root).chain(children.iter().copied()) {
            let role = role_of(&sig.id);
            for leg in &sig.legs {
                // Leg rows persist the ORDER direction as the leg side on
                // every record type: entry `long` bought, close-family
                // `long` bought back the short it closed (`opposite` of the
                // position). Verified against venue truth — a flat- child
                // stored `long` filled as a buy.
                let order_side = match leg.side {
                    crate::state_store::LegSide::Long => "buy",
                    crate::state_store::LegSide::Short => "sell",
                };
                let filled = if leg.filled_qty.is_finite() {
                    leg.filled_qty
                } else {
                    0.0
                };
                let fee = if leg.fee_usd.is_finite() {
                    leg.fee_usd.max(0.0)
                } else {
                    0.0
                };
                let intended = if leg.intended_qty.is_finite() {
                    leg.intended_qty
                } else {
                    0.0
                };
                if let Some(px) = leg.fill_price.filter(|p| p.is_finite() && *p > 0.0) {
                    if filled > 0.0 {
                        if order_side == "sell" {
                            sell_filled = true;
                        } else {
                            buy_filled = true;
                        }
                    }
                    let sign = if order_side == "sell" { 1.0 } else { -1.0 };
                    net_flow += sign * filled * px;
                }
                fees += fee;
                let fill_state = if filled <= 0.0 {
                    "no_fill"
                } else if filled + 1e-9 < intended {
                    "partial"
                } else {
                    "filled"
                };
                txs.push(AtomicTransactionDto {
                    leg_id: leg.id.clone(),
                    signal_id: sig.id.clone(),
                    role,
                    venue: leg.venue.clone(),
                    account_id: leg.account_id.clone(),
                    side: leg.side.as_str(),
                    order_side,
                    symbol: leg.symbol.clone(),
                    intended_qty: intended,
                    filled_qty: filled,
                    intended_price: leg.intended_price,
                    fill_price: leg.fill_price,
                    fee_usd: fee,
                    unit_scale: if leg.unit_scale.is_finite() && leg.unit_scale > 0.0 {
                        leg.unit_scale
                    } else {
                        1.0
                    },
                    venue_order_id: leg.venue_order_id.clone(),
                    client_order_id: leg.client_order_id.clone(),
                    fill_state,
                    created_at: leg.created_at.clone(),
                    updated_at: leg.updated_at.clone(),
                });
            }
        }
        net_flow -= fees;
        txs.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then(a.leg_id.cmp(&b.leg_id))
        });

        // Cycle-lot join: writers name the lot "{kind}-{root_signal_id}" —
        // ends_with covers spatial/funding/rehydrated without a prefix list
        // that could drift when a new strategy starts opening lots.
        let lot = lots.values().find(|l| l.id.ends_with(&root.id));
        let terminal = matches!(
            root.status,
            crate::state_store::SignalStatus::Closed | crate::state_store::SignalStatus::Failed
        );
        let lot_status = lot.map(|l| match l.status {
            arbitrageur_execution::virtual_ledger::CycleStatus::Open => "open",
            arbitrageur_execution::virtual_ledger::CycleStatus::Closed => "closed",
            arbitrageur_execution::virtual_ledger::CycleStatus::StoppedOut => "stopped_out",
            arbitrageur_execution::virtual_ledger::CycleStatus::TimedOut => "timed_out",
        });
        // Cash-flow realized P&L is only meaningful for a family that
        // round-tripped: fills on both order sides close the loop. A
        // single-direction family (e.g. an `extclose-` buy-back whose
        // entry never passed through the ledger) shows gross flow, not
        // profit — attribute nothing rather than misstate it. No fills at
        // all is provably $0.00.
        let realized = lot
            .and_then(|l| l.realized_pnl_usd)
            .or(if terminal && lot.is_none() {
                if !buy_filled && !sell_filled {
                    Some(0.0)
                } else if buy_filled && sell_filled {
                    Some(net_flow)
                } else {
                    None
                }
            } else {
                None
            });
        let realized_bps = lot.and_then(|l| l.realized_pnl_bps);
        let venue_long = lot.map(|l| l.venue_long.clone()).or_else(|| {
            root.legs
                .iter()
                .find(|l| l.side == crate::state_store::LegSide::Long)
                .map(|l| l.venue.clone())
        });
        let venue_short = lot.map(|l| l.venue_short.clone()).or_else(|| {
            root.legs
                .iter()
                .find(|l| l.side == crate::state_store::LegSide::Short)
                .map(|l| l.venue.clone())
        });
        out.push(TradedOpportunityDto {
            signal_id: root.id.clone(),
            strategy_id: root.strategy_id.clone(),
            symbol: root.symbol.clone(),
            environment: root.environment.clone(),
            status: root.status.as_str(),
            route_id: root.route_id.clone(),
            expected_net_usd: root.expected_net_usd,
            note: root.note.clone(),
            venue_long,
            venue_short,
            created_at: root.created_at.clone(),
            closed_at: root.closed_at.clone(),
            lot_id: lot.map(|l| l.id.clone()),
            lot_status,
            entry_spread_bps: lot.map(|l| l.entry_spread_bps),
            close_spread_bps: lot.and_then(|l| l.close_spread_bps),
            realized_pnl_usd: realized,
            realized_pnl_bps: realized_bps,
            leg_net_flow_usd: net_flow,
            total_fees_usd: fees,
            child_count: children.len(),
            transactions: txs,
        });
    }
    out.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then(b.signal_id.cmp(&a.signal_id))
    });
    out
}

/// Per-ticker execution/position state: whether the symbol is blocked, its
/// blocking signals, and the net filled position across all signals.
#[tauri::command]
pub async fn get_ticker_position_state(
    symbol: String,
    state: tauri::State<'_, Arc<AppEngineState>>,
) -> Result<serde_json::Value, String> {
    let blocking = state
        .state_store
        .blocking_signals(&symbol, Some(state.env().as_str()))
        .map_err(|e| e.to_string())?;
    let net = state
        .state_store
        .ticker_net_position(&symbol, Some(state.env().as_str()))
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "symbol": symbol,
        "blocked": !blocking.is_empty(),
        "blockingSignalIds": blocking.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
        "netPositionQty": net,
    }))
}

/// Persist the exits-only flag for (current environment, strategy, symbol).
/// Entry gates across every dispatch path consult this — a manual click can
/// never bypass it the way the old client-side localStorage flag could.
#[tauri::command]
pub async fn set_ticker_reduce_only(
    symbol: String,
    strategy_id: String,
    enabled: bool,
    state: tauri::State<'_, Arc<AppEngineState>>,
) -> Result<(), String> {
    let sid = crate::state_store::normalize_strategy_id(&strategy_id).map_err(|e| e.to_string())?;
    state
        .state_store
        .set_ticker_reduce_only(
            &symbol,
            &sid,
            state.env().as_str(),
            enabled,
            chrono::Utc::now().timestamp_millis(),
        )
        .map_err(|e| e.to_string())
}

/// All exits-only (strategy, symbol) rows for the current environment.
#[tauri::command]
pub async fn list_reduce_only_tickers(
    state: tauri::State<'_, Arc<AppEngineState>>,
) -> Result<Vec<serde_json::Value>, String> {
    let rows = state
        .state_store
        .list_reduce_only_tickers(state.env().as_str())
        .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|(strategy_id, symbol)| {
            serde_json::json!({"strategyId": strategy_id, "symbol": symbol})
        })
        .collect())
}

/// Per-strategy capability descriptor: whether scanning and execution are
/// wired end-to-end. Execute controls and auto-loops must gate on this —
/// a permanently-stubbed path must never render an armed Execute button.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyCapabilityDto {
    pub strategy_id: String,
    pub scan_ready: bool,
    pub execute_ready: bool,
    pub reason: Option<String>,
}

pub fn get_strategy_capabilities_impl() -> Vec<StrategyCapabilityDto> {
    let cap = |id: &str, scan: bool, exec: bool, reason: Option<&str>| StrategyCapabilityDto {
        strategy_id: id.into(),
        scan_ready: scan,
        execute_ready: exec,
        reason: reason.map(|r| r.to_string()),
    };
    vec![
        cap("futures", true, true, None),
        cap("radar", true, true, None),
        cap("funding", true, true, None),
        cap("triangle", true, true, None),
        // Spot rides the shared atomic two-leg path with spot semantics:
        // venue-listed spot markets only, quote-cash + base-inventory
        // preflights in place of margin/leverage, and the same durable
        // signal ledger, slippage and hurdle gates as every other strategy.
        cap("spot", true, true, None),
        cap(
            "prediction",
            false,
            false,
            Some("Prediction venues are scaffolds: no market data or order execution path"),
        ),
        cap(
            "stat_arb",
            true,
            false,
            Some("Research lab — no order dispatch"),
        ),
        cap(
            "external",
            false,
            false,
            Some("External positions are imported, not executed"),
        ),
    ]
}

#[tauri::command]
pub async fn get_strategy_capabilities(
    state: tauri::State<'_, Arc<AppEngineState>>,
) -> Result<Vec<StrategyCapabilityDto>, String> {
    let _ = state;
    Ok(get_strategy_capabilities_impl())
}

/// Aggregated current open positions across signal executions, grouped by
/// strategy + symbol + venue set. `unrealizedPnlUsd` is mark-to-market only
/// when every filled leg has an entry price and a fresh venue book; otherwise
/// `null` — never fabricated. `strategyId` scopes to one strategy.
#[tauri::command]
pub async fn get_open_positions(
    strategy_id: Option<String>,
    state: tauri::State<'_, Arc<AppEngineState>>,
) -> Result<Vec<crate::state_store::OpenPositionDto>, String> {
    let sid = match strategy_id.as_deref() {
        Some(raw) => {
            Some(crate::state_store::normalize_strategy_id(raw).map_err(|e| e.to_string())?)
        }
        None => None,
    };
    let mut signals = state
        .state_store
        .list_signals(None, None)
        .map_err(|e| e.to_string())?;
    if let Some(sid) = sid {
        signals.retain(|s| s.strategy_id.eq_ignore_ascii_case(&sid));
    }
    let hub = &state.market_data_hub;
    let mut positions = crate::state_store::aggregate_open_positions(&signals, &|venue, symbol| {
        hub.get_valid_orderbook(&VenueId::from(venue), symbol, None)
            .and_then(|b| b.mid_price())
            .and_then(|m| dec_to_f64(m, "mark mid").ok())
    });
    // An unreadable flag fails closed: refuse the whole read rather than
    // badge a reduce-only ticker as freely tradable.
    for p in &mut positions {
        p.reduce_only = state
            .state_store
            .ticker_reduce_only(&p.symbol, &p.strategy_id, state.env().as_str())
            .map_err(|e| e.to_string())?;
    }
    Ok(positions)
}

#[tauri::command]
pub async fn execute_spatial_candidates(
    candidates: Vec<SpatialCandidateInput>,
    auto: Option<bool>,
    state: tauri::State<'_, Arc<AppEngineState>>,
) -> Result<Vec<CandidateOutcomeDto>, String> {
    execute_spatial_candidates_impl(candidates, auto.unwrap_or(false), &state).await
}

pub async fn execute_cross_ticker_arbitrage_impl(
    long_symbol: &str,
    short_symbol: &str,
    long_venue: &str,
    short_venue: &str,
    order_qty: f64,
    net_spread_bps: Option<f64>,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    if let Err(reason) = state.circuit_breaker.is_trading_allowed() {
        return Err(format!(
            "Execution halted by Risk Circuit Breaker: {}",
            reason
        ));
    }
    if *state.is_paused.read() {
        return Err(
            "Trading engine is PAUSED. Positions are preserved, but new executions are suspended."
                .into(),
        );
    }
    let _ = (
        long_symbol,
        short_symbol,
        long_venue,
        short_venue,
        order_qty,
        net_spread_bps,
    );
    Err("Native 'cross-ticker' execution is disabled until authentic order submission and reconciliation are implemented".into())
}

pub async fn execute_prediction_arbitrage_impl(
    market_title: &str,
    buy_yes_venue: &str,
    buy_no_venue: &str,
    quantity: f64,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    if let Err(reason) = state.circuit_breaker.is_trading_allowed() {
        return Err(format!(
            "Execution halted by Risk Circuit Breaker: {}",
            reason
        ));
    }
    if *state.is_paused.read() {
        return Err(
            "Trading engine is PAUSED. Positions are preserved, but new executions are suspended."
                .into(),
        );
    }
    let _ = (market_title, buy_yes_venue, buy_no_venue, quantity);
    Err("Native 'prediction' execution is disabled until authentic order submission and reconciliation are implemented".into())
}

/// Live account read with last-known-good fallback.
/// Ok((snapshot, None)) = fresh read (cache refreshed).
/// Ok((snapshot, Some(age_ms))) = read failed, serving cached figures so a
/// connectivity blip never reads as a loss of funds.
/// Err = read failed with nothing cached (venue never read successfully).
pub(crate) async fn live_account_or_cached(
    state: &AppEngineState,
    vid: &str,
    adapter: &Arc<dyn ExchangeAdapter>,
) -> Result<(AccountSnapshot, Option<i64>), String> {
    // Freshness window: independent pollers (venue balances, risk overview,
    // external positions, reconciler) each firing fetch_account would
    // multiply a venue's account-endpoint rate — Orderly's /v1/positions
    // 429s at just a few req/3s. Within the window the last snapshot is
    // served as fresh (it IS fresh — `Some(age)` is reserved for reads that
    // actually failed, which callers surface as STALE/ERROR).
    const ACCOUNT_FRESH_MS: i64 = 2_500;
    // Cache/singleflight keys are env-namespaced — a live snapshot must
    // never be served to a testnet read after an environment switch.
    let key = format!("{}-{vid}", state.env().as_str());
    let now_ms = Utc::now().timestamp_millis();
    if let Some((acc, ts)) = state.last_good_accounts.lock().get(&key).cloned() {
        if now_ms.saturating_sub(ts) < ACCOUNT_FRESH_MS {
            return Ok((acc, None));
        }
    }
    // Uncredentialed venues (vault locked or never provisioned) cannot
    // answer an account read — serve the stale snapshot and skip the wire
    // call entirely. Without this gate every poller's tick burns a doomed
    // request per venue, and the failures count against the venue's
    // rate-limit backoff budget, starving its public market-data lanes.
    if !adapter.has_live_credentials() {
        if let Some((acc, ts)) = state.last_good_accounts.lock().get(&key).cloned() {
            return Ok((acc, Some(now_ms.saturating_sub(ts))));
        }
        return Err(format!("{vid}: no live credentials loaded"));
    }
    // Singleflight: concurrent stale reads share one in-flight fetch.
    let lock = state
        .account_fetch_locks
        .lock()
        .entry(key.clone())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _permit = lock.lock().await;
    // A waiter may find a fresh snapshot the leader just wrote.
    let now_ms = Utc::now().timestamp_millis();
    if let Some((acc, ts)) = state.last_good_accounts.lock().get(&key).cloned() {
        let age = now_ms.saturating_sub(ts);
        if age < ACCOUNT_FRESH_MS {
            tracing::debug!("{vid}: account read coalesced with in-flight fetch ({age}ms old)");
            return Ok((acc, None));
        }
    }
    match adapter.fetch_account().await {
        Ok(mut acc) => {
            acc.venue = vid.into();
            let now_ms = Utc::now().timestamp_millis();
            state
                .last_good_accounts
                .lock()
                .insert(key.clone(), (acc.clone(), now_ms));
            Ok((acc, None))
        }
        Err(e) => {
            let cached = state.last_good_accounts.lock().get(&key).cloned();
            match cached {
                Some((acc, ts)) => {
                    let age = Utc::now().timestamp_millis().saturating_sub(ts);
                    Ok((acc, Some(age)))
                }
                None => Err(e.to_string()),
            }
        }
    }
}

/// Feed the drawdown breaker from one venue's freshly-observed USD-basis
/// equity. The breaker watches the portfolio total for the ACTIVE
/// environment — `breaker_venue_equity` holds each venue's last-seen
/// value keyed `{env}-{vid}`, so any lane that reads an account (risk
/// overview, margin poller) keeps the total current even while nobody
/// polls the UI. A testnet<->live switch is a different account universe,
/// never a trading loss: the baseline re-anchors instead of tripping.
/// On a genuine universe switch the map's stale-env entries are dropped
/// so the old env's equity can't ghost the new env's total.
pub(crate) fn feed_breaker_equity_usd(state: &AppEngineState, vid: &str, equity_usd: Decimal) {
    let env_str = state.env().as_str();
    let prefix = format!("{env_str}-");
    let key = format!("{prefix}{vid}");
    let is_live = state.is_live();
    let total: Decimal = {
        let mut map = state.breaker_venue_equity.lock();
        map.insert(key, equity_usd);
        map.retain(|k, _| k.starts_with(&prefix));
        map.values().sum()
    };
    let mut universe = state.last_risk_live_universe.write();
    if *universe != Some(is_live) {
        state.circuit_breaker.reanchor_baseline(total);
        *universe = Some(is_live);
    } else {
        state.circuit_breaker.update_equity(total);
    }
}

/// Batch variant for lanes that already hold the full account list —
/// same per-venue bookkeeping, one total push at the end.
pub(crate) fn feed_breaker_equity(state: &AppEngineState, accounts: &[AccountSnapshot]) {
    let env_str = state.env().as_str();
    let prefix = format!("{env_str}-");
    let is_live = state.is_live();
    let total: Decimal = {
        let mut map = state.breaker_venue_equity.lock();
        for acc in accounts {
            map.insert(format!("{prefix}{}", acc.venue), acc.equity);
        }
        map.retain(|k, _| k.starts_with(&prefix));
        map.values().sum()
    };
    let mut universe = state.last_risk_live_universe.write();
    if *universe != Some(is_live) {
        state.circuit_breaker.reanchor_baseline(total);
        *universe = Some(is_live);
    } else {
        state.circuit_breaker.update_equity(total);
    }
}

/// Grace period before a stale venue escalates to ERROR (ms).
pub(crate) fn stale_grace_ms(state: &AppEngineState) -> i64 {
    (state.settings.read().balance_stale_warn_secs as i64).clamp(30, 3600) * 1000
}

/// Build the margin monitor from the operator's Safety settings so the
/// risk snapshot, venue balances, and pre-trade gates share one threshold set.
pub(crate) fn margin_monitor_from_settings(state: &AppEngineState) -> MarginMonitor {
    let s = state.settings.read();
    let cfg = MarginMonitorConfig {
        warning_margin_usage_pct: Decimal::from_f64_retain(s.margin_warning_usage_pct)
            .unwrap_or(dec!(70.0)),
        critical_margin_usage_pct: Decimal::from_f64_retain(s.margin_critical_usage_pct)
            .unwrap_or(dec!(85.0)),
        min_liquidation_distance_pct: Decimal::from_f64_retain(
            s.margin_min_liquidation_distance_pct,
        )
        .unwrap_or(dec!(15.0)),
    };
    MarginMonitor::new(cfg)
}

pub async fn get_risk_overview_impl(state: &AppEngineState) -> Result<RiskOverviewDto, String> {
    let cb = &state.circuit_breaker;
    let monitor = margin_monitor_from_settings(state);

    // Risk telemetry reads the active environment's admitted adapters so
    // the header reconciles with the Capital Allocation gadget — production
    // in Live, venue demo endpoints in Testnet. A venue whose read fails
    // keeps contributing its last-known-good figures: a connectivity blip
    // is not a loss of funds and must not move the portfolio total or trip
    // the drawdown breaker.
    let mut accounts = Vec::new();
    let manual_locked = {
        let mut vault = state.vault.write();
        vault.is_initialized() && !vault.is_unlocked() && !vault.is_auto_locked()
    };
    // The locked pause is a steady state, not an event — warn at most once
    // per 5 min instead of per venue per poll. Read failures keep per-call
    // visibility since each is a distinct anomaly.
    let locked_warn_due = || {
        let mut t = state.risk_locked_warn_at.lock();
        match *t {
            Some(prev) if prev.elapsed() < std::time::Duration::from_secs(300) => false,
            _ => {
                *t = Some(std::time::Instant::now());
                true
            }
        }
    };
    for (vid, _name, adapter) in state.get_venues() {
        match live_account_or_cached(state, vid, &adapter).await {
            Ok((mut acc, stale_age)) => {
                if let Some(age) = stale_age {
                    if manual_locked {
                        if locked_warn_due() {
                            tracing::warn!(
                                "risk overview: reads paused (vault locked), showing last-known figures ({}s old for {vid})",
                                age / 1000
                            );
                        }
                    } else {
                        tracing::warn!(
                            "risk overview: {vid} read failed, using last-known figures ({}s old)",
                            age / 1000
                        );
                    }
                }
                // USD-quote policy: a venue settling in USDT reports equity
                // in USDT — convert to the USD basis before portfolio sums
                // so a depeg shows as real drawdown, not noise.
                state.normalize_account_usd(vid, &mut acc);
                accounts.push(acc);
            }
            Err(e) => {
                if manual_locked {
                    if locked_warn_due() {
                        tracing::warn!(
                            "risk overview: reads paused (vault locked, no cached figures)"
                        );
                    }
                } else {
                    tracing::warn!("risk overview: account read failed for {vid} (no cache): {e}");
                }
            }
        }
    }

    let total_equity_dec: Decimal = accounts.iter().map(|a| a.equity).sum();
    let total_free_margin_dec: Decimal = accounts.iter().map(|a| a.free_margin).sum();
    let total_mm_dec: Decimal = accounts.iter().map(|a| a.maintenance_margin).sum();
    let all_positions: Vec<_> = accounts.iter().flat_map(|a| a.positions.clone()).collect();

    // Feed observed equity into the breaker (env-scoped portfolio total),
    // then re-read its state so a drawdown trip is reflected in this same
    // snapshot.
    let is_halted = {
        feed_breaker_equity(state, &accounts);
        cb.is_tripped()
    };
    let cb_str = match cb.trip_reason() {
        Some(reason) => format!("TRIPPED: {}", reason),
        None => {
            if is_halted {
                "TRIPPED: Manual halt".into()
            } else {
                "ACTIVE".into()
            }
        }
    };

    let delta_limit = state.delta_guard.read().max_unhedged_usd_per_asset();
    let delta_limit_f = delta_limit.to_string().parse::<f64>().unwrap_or(0.0);

    let net_deltas_map = {
        let delta_guard = state.delta_guard.read();
        delta_guard.calculate_net_delta(&all_positions)
    };

    let mut deltas_dto = Vec::new();
    for (asset, (qty, usd)) in net_deltas_map {
        let qty_f = qty.to_string().parse::<f64>().unwrap_or(0.0);
        let usd_f = usd.to_string().parse::<f64>().unwrap_or(0.0);
        deltas_dto.push(NetDeltaDto {
            asset,
            net_quantity: qty_f,
            net_exposure_usd: usd_f,
            delta_limit_usd: delta_limit_f,
            is_neutral: usd.abs() <= delta_limit,
        });
    }
    deltas_dto.sort_by(|a, b| a.asset.cmp(&b.asset));

    let total_equity = total_equity_dec.to_string().parse::<f64>().unwrap_or(0.0);
    let total_free_margin = total_free_margin_dec
        .to_string()
        .parse::<f64>()
        .unwrap_or(0.0);
    let total_mm = total_mm_dec.to_string().parse::<f64>().unwrap_or(0.0);
    let current_drawdown = cb
        .current_drawdown_usd()
        .to_string()
        .parse::<f64>()
        .unwrap_or(0.0);
    let max_drawdown_limit = cb
        .max_daily_drawdown_usd()
        .to_string()
        .parse::<f64>()
        .unwrap_or(0.0);
    let day_start = cb
        .day_start_equity()
        .to_string()
        .parse::<f64>()
        .unwrap_or(0.0);
    let drawdown_pct = if day_start > 0.0 {
        (current_drawdown / day_start) * 100.0
    } else {
        0.0
    };

    let severity = |s: MarginHealthStatus| match s {
        MarginHealthStatus::Empty => -1,
        MarginHealthStatus::Healthy => 0,
        MarginHealthStatus::Warning => 1,
        // Unverifiable liquidation data outranks a verified warning: the
        // monitor cannot rule the position safe.
        MarginHealthStatus::Unknown => 2,
        MarginHealthStatus::Critical => 3,
        MarginHealthStatus::Breached => 4,
    };
    let mut worst = MarginHealthStatus::Healthy;
    for acc in &accounts {
        let s = monitor.evaluate_account(acc);
        if severity(s) > severity(worst) {
            worst = s;
        }
    }
    let mut min_liq_dist_pct: Option<f64> = None;
    for pos in &all_positions {
        let s = monitor.evaluate_position(pos);
        if severity(s) > severity(worst) {
            worst = s;
        }
        if let Some(dist) = pos.liquidation_distance_pct() {
            let pct = (dist * dec!(100)).to_string().parse::<f64>().unwrap_or(0.0);
            min_liq_dist_pct = Some(match min_liq_dist_pct {
                Some(cur) => cur.min(pct),
                None => pct,
            });
        }
    }

    // Liquidation safety cannot be claimed without actual liquidation prices;
    // open positions with no liquidation data report UNKNOWN_LIQUIDATION
    // unless a margin severity already demands a stronger state.
    let margin_status = if is_halted {
        "HALTED"
    } else if all_positions.is_empty() {
        "FLAT"
    } else {
        match worst {
            MarginHealthStatus::Breached => "BREACHED",
            MarginHealthStatus::Critical => "CRITICAL",
            MarginHealthStatus::Warning => "WARNING",
            MarginHealthStatus::Unknown => "UNKNOWN_LIQUIDATION",
            MarginHealthStatus::Empty | MarginHealthStatus::Healthy => {
                if min_liq_dist_pct.is_none() {
                    "UNKNOWN_LIQUIDATION"
                } else {
                    "SAFE"
                }
            }
        }
    };

    Ok(RiskOverviewDto {
        circuit_breaker_state: cb_str,
        is_halted,
        current_drawdown_usd: current_drawdown,
        max_drawdown_limit_usd: max_drawdown_limit,
        drawdown_pct,
        consecutive_failures: cb.consecutive_errors(),
        max_failures_allowed: cb.max_consecutive_errors(),
        total_portfolio_value_usd: total_equity,
        net_deltas: deltas_dto,
        margin_health: MarginHealthDto {
            maintenance_margin_req_usd: total_mm,
            margin_balance_usd: total_free_margin,
            margin_ratio_pct: if total_equity > 0.0 {
                (total_mm / total_equity) * 100.0
            } else {
                0.0
            },
            liquidation_distance_pct: if all_positions.is_empty() {
                100.0
            } else {
                min_liq_dist_pct.unwrap_or(0.0)
            },
            status: margin_status.into(),
        },
    })
}

pub fn reset_circuit_breaker_impl(state: &AppEngineState) -> Result<String, String> {
    state.circuit_breaker.reset();
    Ok("Circuit Breaker reset successfully. Trading resumed.".into())
}

pub fn trigger_emergency_killswitch_impl(state: &AppEngineState) -> Result<String, String> {
    state
        .circuit_breaker
        .trip("EMERGENCY KILLSWITCH ENGAGED BY OPERATOR");
    Ok("EMERGENCY KILLSWITCH ENGAGED. All trading operations halted immediately.".into())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlattenResultDto {
    pub strategy_id: String,
    pub symbol: Option<String>,
    pub signals_processed: usize,
    pub signals_closed: usize,
    pub legs_closed: usize,
    /// Open signals whose legs could not be fully closed — residual exposure
    /// remains and those signals stay `unresolved` (ticker stays blocked).
    pub residual_signals: usize,
    pub notes: Vec<String>,
}

pub(crate) fn ledger_qty_to_dec(value: f64, what: &str) -> Result<Decimal, String> {
    if !value.is_finite() || value < 0.0 {
        return Err(format!(
            "{what} must be finite and nonnegative, got {value}"
        ));
    }
    Decimal::from_str_exact(&value.to_string())
        .map_err(|_| format!("{what} {value} is not representable as Decimal"))
}

pub(crate) fn dec_to_f64(value: Decimal, what: &str) -> Result<f64, String> {
    let f = value
        .to_string()
        .parse::<f64>()
        .map_err(|_| format!("{what} {value} is not representable as f64"))?;
    if !f.is_finite() {
        return Err(format!("{what} {value} is not finite as f64"));
    }
    Ok(f)
}

/// Resolve the passive-unwind controls for a flatten. Precedence:
///   strategy `*_passive_mode` = "off"     → never passive (market);
///   "custom"                              → the strategy's own config;
///   "inherit"/unset                       → the global engine policy;
///   unset + a legacy enabled per-strategy config → "custom" (rows saved
///   before the mode field keep their behavior).
/// Any path without its own strategy record (triangle, radar, external
/// residuals) inherits the global policy. `Some` only when the resolved
/// config is enabled and valid — everything else falls back to market.
pub(crate) fn passive_cfg_for(
    state: &AppEngineState,
    strategy_id: &str,
) -> Option<PassiveUnwindConfig> {
    let key = strategy_id.trim().to_lowercase();
    let map = state.strategy_params.read();
    let params = map.get(key.as_str());
    let (mode_raw, custom) = match key.as_str() {
        "futures" => params.map(|p| (p.futures_passive_mode.as_str(), p.futures_passive_unwind)),
        "funding" => params.map(|p| (p.funding_passive_mode.as_str(), p.funding_passive_unwind)),
        "spot" => params.map(|p| (p.spot_passive_mode.as_str(), p.spot_passive_unwind)),
        _ => None,
    }
    .unwrap_or(("", PassiveUnwindConfig::default()));
    let mode = match mode_raw.trim() {
        "" => {
            if custom.enabled {
                "custom"
            } else {
                "inherit"
            }
        }
        m => m,
    };
    let cfg = match mode {
        "off" => return None,
        "custom" => custom,
        _ => state.settings.read().passive_unwind,
    };
    if cfg.enabled && cfg.validate().is_ok() {
        Some(cfg)
    } else {
        None
    }
}

/// Venue refresher for passive re-pegs: a fresh WS-fed hub book wins;
/// otherwise the single-flight REST fetch is validated and mirrored into
/// the hub — the same resolution order every other live read takes.
pub(crate) struct FlattenLiveRefresh<'a> {
    pub(crate) state: &'a AppEngineState,
    pub(crate) venue_id: VenueId,
    pub(crate) symbol: String,
    pub(crate) adapter: Arc<dyn ExchangeAdapter>,
}

#[async_trait::async_trait]
impl PassiveBookRefresh for FlattenLiveRefresh<'_> {
    async fn fresh_book(&self) -> anyhow::Result<OrderBook> {
        match crate::ws_primary::resolve_book_ws_first(
            &self.state.market_data_hub,
            &self.state.orderbook_flights,
            &self.adapter,
            &self.symbol,
            std::time::Duration::from_secs(2),
        )
        .await
        {
            Some(crate::ws_primary::ResolvedBook::Ws(b))
            | Some(crate::ws_primary::ResolvedBook::Cached(b)) => {
                validate_adapter_book(&b, &self.venue_id, &self.symbol)
                    .map_err(|e| anyhow::anyhow!("Flatten order book rejected: {e}"))?;
                Ok(b)
            }
            Some(crate::ws_primary::ResolvedBook::Fresh(b)) => {
                validate_adapter_book(&b, &self.venue_id, &self.symbol)
                    .map_err(|e| anyhow::anyhow!("Flatten order book rejected: {e}"))?;
                self.state.market_data_hub.update_orderbook(b.clone());
                Ok(b)
            }
            None => anyhow::bail!("no live orderbook for {} on {}", self.symbol, self.venue_id),
        }
    }
}

/// Arguments for one flatten leg-close (bundled: clippy allows at most
/// seven function parameters).
struct FlattenLegArgs<'a> {
    state: &'a AppEngineState,
    venue: &'a str,
    symbol: &'a str,
    side: Side,
    qty: Decimal,
    strategy_id: &'a str,
    sig_id: &'a str,
    notes: &'a mut Vec<String>,
    execution_type: Option<&'a str>,
}

/// Outcome of one flatten-leg dispatch. `fill` is present only when the
/// venue reported an actual fill; `order_id`/`client_order_id` are present
/// whenever the venue accepted the order so the reconciler can keep polling
/// a working order instead of losing it.
struct FlattenLegOutcome {
    order_id: Option<String>,
    client_order_id: Option<String>,
    fill: Option<(Decimal, Decimal, Option<Decimal>)>,
}

/// Place a reduce-only close order on a venue to flatten one filled leg.
/// The adapter is environment-scoped: Live hits production, Testnet hits
/// the venue's demo endpoint — one code path, no simulation.
async fn flatten_leg(args: FlattenLegArgs<'_>) -> Result<FlattenLegOutcome, String> {
    let FlattenLegArgs {
        state,
        venue,
        symbol,
        side,
        qty,
        sig_id,
        notes,
        strategy_id,
        execution_type,
    } = args;
    let adapter = state
        .get_adapter(venue)
        .ok_or_else(|| format!("Venue '{venue}' is not admitted for execution"))?;
    if !adapter.live_order_submission_implemented() || !adapter.has_live_credentials() {
        return Err(format!(
            "Venue '{venue}' does not support live execution or has unauthenticated credentials"
        ));
    }
    if qty <= dec!(0) {
        return Err("Invalid leg quantity".to_string());
    }

    // Passive limit unwind (opt-in; emergency flattens stay market-only).
    // The live driver polls fetch_order while the limit works and cancels
    // before re-pegging — resting reduce orders are never stacked.
    let passive_cfg = match execution_type {
        Some("emergency_market") => None,
        _ => passive_cfg_for(state, strategy_id),
    };
    if let Some(cfg) = passive_cfg {
        let refresher = FlattenLiveRefresh {
            state,
            venue_id: adapter.venue_id(),
            symbol: symbol.to_string(),
            adapter: adapter.clone(),
        };
        let report = arbitrageur_execution::passive_unwind::execute_passive_reduce_live(
            &adapter, &refresher, symbol, side, qty, &cfg, None,
        )
        .await
        .map_err(|e| format!("passive reduce failed: {e:#}"))?;
        match &report.outcome {
            PassiveOutcome::LimitFilled => notes.push(format!(
                "{sig_id}: {venue} leg passively closed {qty} (limit, {} tries)",
                report.limit_tries
            )),
            PassiveOutcome::LimitPartialThenMarket => notes.push(format!(
                "{sig_id}: {venue} leg limit-partial, remainder at market ({qty})"
            )),
            PassiveOutcome::MarketFallback { reason } => notes.push(format!(
                "{sig_id}: {venue} leg passive skipped, market exit ({reason})"
            )),
            PassiveOutcome::Unresolved { reason } => notes.push(format!(
                "{sig_id}: {venue} leg close UNRESOLVED ({reason}) — order may still be working; reconciliation will settle it"
            )),
        }
        let resp = report.response;
        let client_order_id = resp.client_order_id.clone();
        let resp =
            arbitrageur_execution::enrich_fill_stats(&adapter, symbol, &client_order_id, resp)
                .await;
        let order_id = (!resp.order_id.trim().is_empty()).then(|| resp.order_id.clone());
        // A fill whose stats never enriched is not a lost order: return the
        // order identity and let the reconciler book the venue-truth fill —
        // erroring here would orphan an accepted (likely filled) order.
        let fill = if resp.filled_qty > dec!(0) {
            match resp.avg_fill_price.filter(|p| *p > dec!(0)) {
                Some(px) => Some((resp.filled_qty, px, resp.fee)),
                None => {
                    notes.push(format!(
                        "{sig_id}: {venue} leg order {} reported filled without a verified fill price — reconciler will settle it",
                        resp.order_id
                    ));
                    None
                }
            }
        } else {
            None
        };
        return Ok(FlattenLegOutcome {
            order_id,
            client_order_id: (!resp.client_order_id.trim().is_empty())
                .then(|| resp.client_order_id.clone()),
            fill,
        });
    }

    let mut order =
        arbitrageur_core::order::OrderRequest::new_market(adapter.venue_id(), symbol, side, qty);
    order.reduce_only = adapter.supports_reduce_only(symbol);
    let client_order_id = order.client_order_id.clone();

    let resp = adapter
        .place_order(order)
        .await
        .map_err(|e| e.to_string())?;
    let resp =
        arbitrageur_execution::enrich_fill_stats(&adapter, symbol, &client_order_id, resp).await;
    let order_id = (!resp.order_id.trim().is_empty()).then(|| resp.order_id.clone());
    let fill = if resp.filled_qty > dec!(0) {
        match resp.avg_fill_price.filter(|p| *p > dec!(0)) {
            Some(px) => Some((resp.filled_qty, px, resp.fee)),
            None => {
                notes.push(format!(
                    "{sig_id}: {venue} leg order {} reported filled without a verified fill price — reconciler will settle it",
                    resp.order_id
                ));
                None
            }
        }
    } else {
        None
    };
    Ok(FlattenLegOutcome {
        order_id,
        client_order_id: Some(client_order_id),
        fill,
    })
}

/// Close the attributed virtual-ledger lot for a flattened signal family and
/// report its realized PnL. The lot is matched on the signal's own ledger
/// namespace (see `attribution::ledger_strategy_id`), so radar, futures and
/// any future attributed strategy close their own lots instead of sharing
/// one hardcoded namespace. Returns `None` when no matching open lot holds
/// enough quantity (e.g. single-leg triangle residuals, which realize purely
/// through their signal legs) — the caller keeps the 0.0 log convention
/// there rather than inventing a number.
/// Add realized cash to the environment-scoped cumulative counter surfaced
/// as "Net Profit" in the UI, persist it under `pnl.total.{env}`, and return
/// the new total. The counter is durable: without persistence a restart
/// reset the headline figure to zero while the durable log/ledger kept the
/// underlying events. A kv write failure is logged, never fatal — the
/// in-memory counter still reflects this session's truth.
pub(crate) fn accumulate_realized_profit(state: &AppEngineState, is_live: bool, usd: f64) -> f64 {
    if !usd.is_finite() {
        return if is_live {
            state
                .total_realized_profit_live
                .read()
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0)
        } else {
            state
                .total_realized_profit
                .read()
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0)
        };
    }
    let delta = Decimal::from_str_exact(&usd.to_string()).unwrap_or(dec!(0));
    let (key, persisted) = {
        let mut total = if is_live {
            state.total_realized_profit_live.write()
        } else {
            state.total_realized_profit.write()
        };
        *total += delta;
        (
            if is_live {
                "pnl.total.live"
            } else {
                "pnl.total.testnet"
            },
            total.to_string(),
        )
    };
    if let Err(e) = state.state_store.kv_set(key, &persisted) {
        tracing::warn!("persist cumulative realized PnL failed: {e}");
    }
    persisted.parse::<f64>().unwrap_or(0.0)
}

/// Realize a liquidation trim's filled portion: accumulate its partial-close
/// cash and shrink the open lot's quantities so the final flatten's fills
/// still cover the remaining qty. Returns the realized USD (0 when no fills
/// landed). The trim child's legs encode closing sides — the long venue's
/// leg records `Short`, the short venue's records `Long`.
pub(crate) fn realize_liq_trim(
    state: &AppEngineState,
    sig: &crate::state_store::SignalExecution,
    close_id: &str,
    long_venue: &str,
    short_venue: &str,
) -> Result<f64, String> {
    use crate::state_store::LegSide;

    let child = state
        .state_store
        .get_signal(close_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("trim child {close_id} missing"))?;
    let fill_of = |venue: &str, side: LegSide| {
        child
            .legs
            .iter()
            .find(|l| l.venue.eq_ignore_ascii_case(venue) && l.side == side)
            .and_then(|l| match (l.fill_price, l.filled_qty, l.fee_usd) {
                (Some(px), q, f) if q > 0.0 && px > 0.0 => Some((q, px, f)),
                _ => None,
            })
    };
    let (long_canon, short_canon) = (
        crate::state::canonical_venue_id(long_venue),
        crate::state::canonical_venue_id(short_venue),
    );
    // The long venue's position is reduced by a Short closing fill; the
    // short venue's by a Long one.
    let long_fill = long_canon.and_then(|v| fill_of(v, LegSide::Short));
    let short_fill = short_canon.and_then(|v| fill_of(v, LegSide::Long));
    if long_fill.is_none() && short_fill.is_none() {
        return Ok(0.0);
    }

    let want_strategy = crate::attribution::ledger_strategy_id(&sig.strategy_id);
    let mut ledger = state.virtual_ledger.write();
    let lot = ledger.lots.values_mut().find(|lot| {
        lot.strategy_id == want_strategy
            && lot.symbol == sig.symbol
            && lot.status == CycleStatus::Open
            && crate::state::canonical_venue_id(&lot.venue_long) == long_canon
            && crate::state::canonical_venue_id(&lot.venue_short) == short_canon
    });
    let Some(lot) = lot else {
        // No durable lot (pre-v30 position or closed already): the trim's
        // cash is still real — accumulate it unpriced against entry rather
        // than dropping it? Entry prices are unknown without the lot, so
        // the honest bound is proceeds-minus-fees per side, which would
        // understate losses. Refuse to guess: report, don't fabricate.
        return Err(format!("no open {} lot for {}", want_strategy, sig.symbol));
    };

    let mut realized = 0.0;
    // Delta semantics: a leg's fill can grow across invocations (partial
    // fills enriched by order polling), so the durable mark tracks the
    // counted cumulative qty/pnl/fee — only the uncounted increment shrinks
    // the lot and reaches the counter.
    let realize_leg =
        |q: f64, px: f64, fee: f64, entry_px: f64, side: LegSide, venue: &str| -> (f64, f64) {
            let mark = unlotted_fill_key(close_id, venue, side);
            let (q_done, pnl_done, fee_done) = read_unlotted_mark(state, &mark);
            let delta_q = q - q_done;
            if delta_q <= 0.0 {
                return (0.0, 0.0);
            }
            let delta_fee = (fee - fee_done).max(0.0);
            // Price the increment at the leg's cumulative average fill —
            // per-fill px history isn't retained; partial fills on one order
            // cluster near the limit so the VWAP-mix error is bounded.
            let side_sign = match side {
                LegSide::Short => 1.0,
                LegSide::Long => -1.0,
            };
            let leg_pnl = side_sign * (px - entry_px) * delta_q - delta_fee;
            let cum_pnl = pnl_done + leg_pnl;
            let _ = state
                .state_store
                .kv_set(&mark, &format!("{q}|{cum_pnl}|{fee}"));
            (leg_pnl, delta_q)
        };
    if let Some((q, px, fee)) = long_fill {
        let (leg_pnl, delta_q) =
            realize_leg(q, px, fee, lot.long_entry_price, LegSide::Short, long_venue);
        realized += leg_pnl;
        lot.long_qty = (lot.long_qty - delta_q).max(0.0);
    }
    if let Some((q, px, fee)) = short_fill {
        let (leg_pnl, delta_q) = realize_leg(
            q,
            px,
            fee,
            lot.short_entry_price,
            LegSide::Long,
            short_venue,
        );
        realized += leg_pnl;
        lot.short_qty = (lot.short_qty - delta_q).max(0.0);
    }
    let lot_id = lot.id.clone();
    let snapshot = lot.clone();
    drop(ledger);
    accumulate_realized_profit(state, state.is_live(), realized);
    // Running trim total on the lot: `close_cycle` only sees the reduced
    // quantities, so the final close folds this sum back into
    // `realized_pnl_usd`/`_bps` — per-strategy metrics then reflect the
    // whole cycle (defensive trims included), not just the last slice.
    let key = format!("lot.trim_usd.{lot_id}");
    let prior: f64 = state
        .state_store
        .kv_get(&key)
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    let _ = state
        .state_store
        .kv_set(&key, &(prior + realized).to_string());
    if let Err(e) = state.state_store.upsert_cycle_lot(&snapshot) {
        tracing::warn!("persist trimmed cycle lot {} failed: {e}", snapshot.id);
    }
    Ok(realized)
}

/// Durable per-leg marker: a family child's leg fill realized through the
/// unlotted sweep (or `realize_liq_trim`) exactly once. Keyed by child id +
/// venue + closing side — fills land atomically per leg via `apply_leg_fill`,
/// so a leg present-unmarked gets picked up on the next pass even when its
/// fill arrived late through order reconciliation.
fn unlotted_fill_key(child_id: &str, venue: &str, side: crate::state_store::LegSide) -> String {
    let canon = crate::state::canonical_venue_id(venue).unwrap_or(venue);
    format!("unlotted.fill.{child_id}.{canon}.{}", side.as_str())
}

/// Sum the counted-fee share of durable marks on a family's close-aggregation
/// legs. Only reachable when `close_flattened_spatial_lot` matched a lot —
/// which requires the lot to have been created AFTER some fills were already
/// marked (the `rehydrated-` boot path): `close_cycle`'s `exit_fees` sums all
/// flat children fills, so the counted portion's fees were deducted from
/// `usd` even though that portion's leg PnL isn't in `usd` (it prices
/// `lot.qty`, the residual). Adding the marked fee share back keeps the
/// increment exactly equal to the uncounted remainder. `trim-` legs are
/// skipped: their fills never join the close VWAP or its fee total.
pub(crate) fn marked_family_fees(
    state: &AppEngineState,
    family: &[crate::state_store::SignalExecution],
) -> f64 {
    family
        .iter()
        .filter(|child| !child.id.starts_with("trim-"))
        .flat_map(|child| child.legs.iter().map(move |l| (child.id.as_str(), l)))
        .map(|(cid, leg)| {
            let (_, _, fee) =
                read_unlotted_mark(state, &unlotted_fill_key(cid, &leg.venue, leg.side));
            fee
        })
        .sum()
}

/// Read a durable fill mark: `(counted_qty, counted_pnl, counted_fee)`,
/// all cumulative. A missing or unparseable mark reads as zeroes — the
/// leg is then treated as never counted.
fn read_unlotted_mark(state: &AppEngineState, key: &str) -> (f64, f64, f64) {
    state
        .state_store
        .kv_get(key)
        .ok()
        .flatten()
        .and_then(|v| {
            let mut it = v.split('|');
            let q: f64 = it.next()?.parse().ok()?;
            let p: f64 = it.next()?.parse().ok()?;
            let f: f64 = it.next()?.parse().ok()?;
            Some((q, p, f))
        })
        .unwrap_or((0.0, 0.0, 0.0))
}

/// Accumulate realized PnL for family children whose fills will never pass
/// through a `close_cycle` — single-leg residuals, imbalanced pairs, and
/// signals whose lot can no longer close (family Closed while the lot stays
/// Open on a qty mismatch: no further children can arrive, so these fills
/// are the only record). Signals with an open lot while the family is still
/// open are skipped: their exit fills realize inside the eventual
/// `close_cycle` net, and counting them here would double-count.
///
/// Each leg fill is priced against the matching root entry leg on the same
/// venue (child side is the closing direction, root side the entry
/// direction): `Short` closes a long entry, `Long` closes a short entry.
/// Only leg PnL minus the exit fee lands here — entry fees were already
/// debited in full when the entry legs persisted, so charging them again
/// (even pro-rata) would double-count. Legs whose root entry price is
/// missing are skipped rather than priced at zero — the residual edge is
/// bounded by the documented pre-v30 lotless boundary.
///
/// `children` are the family members to sweep — callers pass the freshest
/// list (including this pass's new child). Every counted leg gets a durable
/// kv marker, so a late-arriving reconciliation fill is picked up on a
/// later pass without double-counting the ones already done.
pub(crate) fn accumulate_unlotted_family_fills(
    state: &AppEngineState,
    sig: &crate::state_store::SignalExecution,
    children: &[crate::state_store::SignalExecution],
    family_closed: bool,
) {
    use crate::state_store::LegSide;

    let venue_of = |side: LegSide| {
        sig.legs
            .iter()
            .find(|l| l.side == side)
            .and_then(|l| crate::state::canonical_venue_id(&l.venue))
    };
    let has_open_lot = match (venue_of(LegSide::Long), venue_of(LegSide::Short)) {
        (Some(long_v), Some(short_v)) => {
            let want = crate::attribution::ledger_strategy_id(&sig.strategy_id);
            state.virtual_ledger.read().lots.values().any(|lot| {
                lot.strategy_id == want
                    && lot.symbol == sig.symbol
                    && lot.status == CycleStatus::Open
                    && crate::state::canonical_venue_id(&lot.venue_long) == Some(long_v)
                    && crate::state::canonical_venue_id(&lot.venue_short) == Some(short_v)
            })
        }
        _ => false,
    };
    // An open lot on a live family defers everything to `close_cycle`; once
    // the family is closed no further fills can arrive, so a still-open lot
    // is dead and its orphan fills must realize here instead.
    if has_open_lot && !family_closed {
        return;
    }
    let mut realized = 0.0;
    for child in children {
        for leg in &child.legs {
            let (Some(px), q, fee) = (leg.fill_price, leg.filled_qty, leg.fee_usd) else {
                continue;
            };
            if !(q > 0.0 && px > 0.0 && fee.is_finite()) {
                continue;
            }
            let mark = unlotted_fill_key(&child.id, &leg.venue, leg.side);
            // Delta semantics: a leg's fill can grow across passes, so the
            // mark tracks the counted cumulative qty/pnl/fee — only the
            // uncounted increment is realized now.
            let (q_done, pnl_done, fee_done) = read_unlotted_mark(state, &mark);
            let delta_q = q - q_done;
            if delta_q <= 0.0 {
                continue;
            }
            let entry_px = sig
                .legs
                .iter()
                .find(|root| root.side != leg.side && root.venue.eq_ignore_ascii_case(&leg.venue))
                .and_then(|root| root.fill_price)
                .filter(|px| *px > 0.0);
            let Some(entry_px) = entry_px else {
                continue;
            };
            let delta_fee = (fee - fee_done).max(0.0);
            let leg_pnl = match leg.side {
                LegSide::Short => (px - entry_px) * delta_q,
                LegSide::Long => (entry_px - px) * delta_q,
            } - delta_fee;
            // Mark first: if the accumulate below were reordered after a
            // crash, a counted-but-unmarked leg would double-count on the
            // next pass — worse than one orphaned leg.
            if let Err(e) = state
                .state_store
                .kv_set(&mark, &format!("{q}|{}|{fee}", pnl_done + leg_pnl))
            {
                tracing::warn!("unlotted mark write failed for {}: {e}", child.id);
                continue;
            }
            realized += leg_pnl;
        }
    }
    if realized != 0.0 {
        accumulate_realized_profit(state, state.is_live(), realized);
        // Running per-signal total so the family-close pass can resolve
        // entry predictions with the whole unwind outcome, not just the
        // last pass's slice.
        let tkey = format!("unlotted.total.{}", sig.id);
        let prior: f64 = state
            .state_store
            .kv_get(&tkey)
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);
        let _ = state
            .state_store
            .kv_set(&tkey, &(prior + realized).to_string());
    }
    if family_closed {
        // The family can accrue no further fills — resolve the entry
        // forecast with the total realized bps on the original entry
        // notional. No marks → nothing was ever priced → leave the
        // prediction unresolved rather than fabricate a 0 outcome.
        if let Ok(Some(total_raw)) = state
            .state_store
            .kv_get(&format!("unlotted.total.{}", sig.id))
        {
            if let Ok(total) = total_raw.parse::<f64>() {
                // Average filled-leg notional — the `close_cycle`
                // convention (leg-notional sum / leg count). For a 1-leg
                // residual that is just the single leg's notional.
                let (notional_sum, leg_count) =
                    sig.legs.iter().fold((0.0_f64, 0u32), |acc, root| {
                        if root.fill_price.unwrap_or(0.0) > 0.0 && root.filled_qty > 0.0 {
                            (
                                acc.0 + root.fill_price.unwrap_or(0.0) * root.filled_qty,
                                acc.1 + 1,
                            )
                        } else {
                            acc
                        }
                    });
                let orig_notional = notional_sum / f64::from(leg_count.max(1));
                if orig_notional > 0.0 {
                    let bps = (total / orig_notional) * 10_000.0;
                    if let Err(e) = state.state_store.resolve_signal_predictions(&sig.id, bps) {
                        tracing::warn!(
                            "resolve unlotted signal predictions failed for {}: {e}",
                            sig.id
                        );
                    }
                }
            }
        }
    }
}

/// Returns `(realized_usd, realized_bps, entry_debited)` for the closed lot.
/// `entry_debited` is the entry-fee figure the entry attempt already wrote to
/// the cumulative realized counter (the lot's pre-refresh open basis) — the
/// close's counter increment is `realized_usd + entry_debited` so the counter
/// converges to the lot's true net regardless of when fee enrichment landed.
pub(crate) fn close_flattened_spatial_lot(
    state: &AppEngineState,
    sig: &crate::state_store::SignalExecution,
    family: &[crate::state_store::SignalExecution],
    close_status: CycleStatus,
) -> Option<(f64, f64, f64)> {
    use crate::state_store::LegSide;

    let venue_of = |side: LegSide| {
        sig.legs
            .iter()
            .find(|l| l.side == side)
            .and_then(|l| crate::state::canonical_venue_id(&l.venue))
    };
    let (Some(long_venue), Some(short_venue)) = (venue_of(LegSide::Long), venue_of(LegSide::Short))
    else {
        return None;
    };

    #[derive(Default)]
    struct LegAgg {
        qty: f64,
        px_qty: f64,
        fee: f64,
    }
    let mut fills: HashMap<(String, &'static str), LegAgg> = HashMap::new();
    for child in family {
        // `trim-` children realized separately at trim time — their fills
        // must not join the close VWAP or they would double-count cash and
        // break the lot-qty match for the trimmed remainder.
        if child.id.starts_with("trim-") {
            continue;
        }
        for leg in &child.legs {
            let Some(canon) = crate::state::canonical_venue_id(&leg.venue) else {
                continue;
            };
            let (Some(px), qty, fee) = (leg.fill_price, leg.filled_qty, leg.fee_usd) else {
                continue;
            };
            if !(qty.is_finite() && qty > 0.0 && px.is_finite() && px > 0.0 && fee.is_finite()) {
                continue;
            }
            let agg = fills
                .entry((canon.to_string(), leg.side.as_str()))
                .or_default();
            agg.qty += qty;
            agg.px_qty += qty * px;
            agg.fee += fee;
        }
    }
    let exit_fees: f64 = fills.values().map(|a| a.fee).sum();
    let agg_for = |venue: &str, side: LegSide| fills.get(&(venue.to_string(), side.as_str()));
    let (Some(long_exit), Some(short_exit)) = (
        agg_for(long_venue, LegSide::Short),
        agg_for(short_venue, LegSide::Long),
    ) else {
        return None;
    };

    let want_ledger_strategy = crate::attribution::ledger_strategy_id(&sig.strategy_id);
    let lot_id = {
        let ledger = state.virtual_ledger.read();
        ledger
            .lots
            .values()
            .find(|lot| {
                lot.strategy_id == want_ledger_strategy
                    && lot.symbol == sig.symbol
                    && lot.status == CycleStatus::Open
                    && crate::state::canonical_venue_id(&lot.venue_long) == Some(long_venue)
                    && crate::state::canonical_venue_id(&lot.venue_short) == Some(short_venue)
                    && long_exit.qty + 1e-9 >= lot.long_qty
                    && short_exit.qty + 1e-9 >= lot.short_qty
            })
            .map(|l| l.id.clone())
    };
    if let Some(id) = lot_id {
        // Refresh entry fees from the signal's authoritative legs before
        // closing: submit-ack fills record fee_usd=0 and the fee-enrichment
        // pass lands the real venue fee later — the lot's stored
        // fees_paid_usd would otherwise stay at the stale 0 and overstate
        // realized PnL by exactly the entry fee.
        let entry_fees: f64 = sig
            .legs
            .iter()
            .map(|l| l.fee_usd)
            .filter(|f| f.is_finite() && *f >= 0.0)
            .sum();
        let mut ledger = state.virtual_ledger.write();
        // The entry-fee figure the entry attempt already debited from the
        // cumulative counter — the lot's open-time basis (response-time fees).
        // Captured before the enrichment refresh below overwrites it; callers
        // add it back to the close's net so the counter's final state equals
        // the lot's true realized PnL regardless of when fee data landed.
        let entry_debited = ledger.lots.get(&id).map(|l| l.fees_paid_usd).unwrap_or(0.0);
        if entry_fees > 0.0 {
            if let Some(lot) = ledger.lots.get_mut(&id) {
                lot.fees_paid_usd = entry_fees;
            }
        }
        let close_usd = ledger
            .close_cycle(
                &id,
                long_exit.px_qty / long_exit.qty,
                short_exit.px_qty / short_exit.qty,
                exit_fees,
                close_status,
            )
            .and_then(|l| l.realized_pnl_usd);
        let realized = close_usd.and_then(|close_usd| {
            let l = ledger.lots.get_mut(&id)?;
            // Fold any mid-hold trim realizations into the lot's final
            // outcome: `close_cycle` only priced the reduced quantities,
            // so metrics would otherwise under-report a trimmed cycle.
            // The returned `usd` stays close-only — the counter already
            // took the trim portion at trim time.
            let trim_sum: f64 = state
                .state_store
                .kv_get(&format!("lot.trim_usd.{}", l.id))
                .ok()
                .flatten()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0);
            if trim_sum != 0.0 {
                let total = close_usd + trim_sum;
                // Denominator is the ORIGINAL entry notional — the root
                // legs keep their entry fills while the lot's qty was
                // shrunk by trims.
                let orig_notional: f64 = sig
                    .legs
                    .iter()
                    .map(|root| root.fill_price.unwrap_or(0.0) * root.filled_qty)
                    .sum::<f64>()
                    / 2.0;
                l.realized_pnl_usd = Some(total);
                if orig_notional > 0.0 {
                    l.realized_pnl_bps = Some((total / orig_notional) * 10_000.0);
                }
            }
            let total_bps = l.realized_pnl_bps;
            // Persist the closed lot (covers the pre-close fee refresh
            // too). Best-effort: a store failure loses durable metrics
            // but the in-memory close already resolved — warn, never
            // unwind the flatten's accounting.
            if let Err(e) = state.state_store.upsert_cycle_lot(l) {
                tracing::warn!("persist closed cycle lot {} failed: {e}", l.id);
            }
            // Close-only cash for the counter; total-cycle bps (trims
            // included) for prediction resolution.
            total_bps.map(|bps| (close_usd, bps, entry_debited))
        });
        // Calibration resolution: the ledger's realized net bps is the
        // authoritative outcome the entry forecast gets scored against.
        // Non-fatal — a prediction left unresolved is uncounted, not wrong.
        if let Some((_, bps, _)) = realized {
            if let Err(e) = state.state_store.resolve_signal_predictions(&sig.id, bps) {
                tracing::warn!("resolve signal predictions failed for {}: {e}", sig.id);
            }
        }
        realized
    } else {
        None
    }
}

/// Emergency per-strategy exit: for every open signal of `strategy_id`
/// (optionally scoped to `symbol`), dispatch inverse reduce-only market orders
/// sized to that root family's attributed residual on each paper venue, and
/// mark the original `closed` only once its attributed legs are neutralized.
/// Bypasses pause/circuit-breaker because it is strictly risk-reducing;
/// in live mode, dispatches reduce-only market orders to live authenticated adapters.
/// `close_status` labels the virtual-ledger lot's terminal state — pass the
/// honest exit reason (`Closed` for profit/operator exits, `StoppedOut` for
/// stop/liquidation/stale unwinds, `TimedOut` for max-hold) so durable cycle
/// analytics can segment outcomes by cause rather than PnL sign alone.
pub async fn flatten_ticker_positions_impl(
    symbol: Option<&str>,
    strategy_id: &str,
    close_status: CycleStatus,
    state: &AppEngineState,
) -> Result<FlattenResultDto, String> {
    flatten_ticker_positions_with_type_impl(symbol, strategy_id, None, close_status, state).await
}

pub async fn flatten_ticker_positions_with_type_impl(
    symbol: Option<&str>,
    strategy_id: &str,
    execution_type: Option<&str>,
    close_status: CycleStatus,
    state: &AppEngineState,
) -> Result<FlattenResultDto, String> {
    flatten_ticker_positions_scoped_impl(
        symbol,
        strategy_id,
        execution_type,
        close_status,
        state,
        None,
    )
    .await
}

/// Flatten one specific root signal family — used by fixed-yield maturity
/// resolution where only the hedged carry pair may close, never every
/// open signal sharing the symbol.
pub async fn flatten_signal_positions_impl(
    signal_id: &str,
    strategy_id: &str,
    execution_type: Option<&str>,
    close_status: CycleStatus,
    state: &AppEngineState,
) -> Result<FlattenResultDto, String> {
    flatten_ticker_positions_scoped_impl(
        None,
        strategy_id,
        execution_type,
        close_status,
        state,
        Some(signal_id),
    )
    .await
}

async fn flatten_ticker_positions_scoped_impl(
    symbol: Option<&str>,
    strategy_id: &str,
    execution_type: Option<&str>,
    close_status: CycleStatus,
    state: &AppEngineState,
    only_signal_id: Option<&str>,
) -> Result<FlattenResultDto, String> {
    use crate::state_store::{LegSide, NewSignalExecution, NewSignalLeg, SignalStatus};

    let sid = crate::state_store::normalize_strategy_id(strategy_id).map_err(|e| e.to_string())?;
    let is_live = state.is_live();
    // Live flatten dispatches real reduce-only orders — the vault must be
    // unlocked so venue calls authenticate. Testnet relies on adapter-level
    // auth failure (a locked vault has already stripped credentials).
    if is_live && !state.vault.write().is_unlocked() {
        return Err(
            "Cannot flatten while the secure credential vault is locked. Unlock the vault first."
                .into(),
        );
    }
    let _execution_guard = state.execution_lock.lock().await;
    let flatten_equity_before = snapshot_venue_equity(state).await;

    let all_signals = state
        .state_store
        .list_signals(symbol, None)
        .map_err(|e| e.to_string())?;
    let roots: Vec<crate::state_store::SignalExecution> = all_signals
        .iter()
        .filter(|s| {
            (only_signal_id.is_none() || only_signal_id == Some(s.id.as_str()))
                && !s.id.starts_with("flat-")
                && !s.id.starts_with("trim-")
                && s.strategy_id.eq_ignore_ascii_case(&sid)
                && s.status.counts_as_open()
        })
        .cloned()
        .collect();

    let mut out = FlattenResultDto {
        strategy_id: sid.clone(),
        symbol: symbol.map(|s| s.to_string()),
        signals_processed: roots.len(),
        signals_closed: 0,
        legs_closed: 0,
        residual_signals: 0,
        notes: Vec::new(),
    };

    let cycle = state.next_sim_step();

    let opposite = |s: LegSide| match s {
        LegSide::Long => LegSide::Short,
        LegSide::Short => LegSide::Long,
    };

    struct LegExposure {
        canonical: &'static str,
        entry_side: LegSide,
        expected_pos_side: Side,
        entry_qty: Decimal,
        exact_symbol: String,
    }
    struct CloseTarget {
        venue: String,
        exact_symbol: String,
        account_id: Option<String>,
        order_side: Side,
        leg_side: LegSide,
        qty: Decimal,
        /// Parent leg's venue-units-per-parent-unit scale (v39) — the
        /// child's own leg keeps it so family delta stays shared-unit.
        unit_scale: f64,
    }

    let child_fills_on = |family: &[crate::state_store::SignalExecution],
                          canonical: &str,
                          side: LegSide|
     -> Result<Decimal, String> {
        let mut total = dec!(0);
        for child in family {
            for cl in &child.legs {
                if cl.side == side && crate::state::canonical_venue_id(&cl.venue) == Some(canonical)
                {
                    total += ledger_qty_to_dec(cl.filled_qty, "child leg fill")?;
                }
            }
        }
        Ok(total)
    };
    // Quantity already dispatched but not yet filled: the unfilled remainder
    // of each open close-child's legs — across ALL families, not just this
    // signal's: two roots sharing a venue+symbol+side close against the same
    // venue position, so a sibling family's in-flight order must suppress
    // this dispatch too. An incomplete child is marked Unresolved by this
    // same pass even while its orders still work, so any non-terminal status
    // can carry in-flight qty — suppress re-dispatch of it or a second
    // flatten would double-submit. Suppression is time-bounded: past the
    // reconciler's 15-min working-order deadline (cancels exhausted), the
    // order is presumed dead and the venue position re-read bounds the
    // residual — holding pending forever would strand it.
    let mut open_children: Vec<crate::state_store::SignalExecution> = all_signals
        .iter()
        .filter(|s| {
            (s.id.starts_with("flat-") || s.id.starts_with("trim-")) && s.status.counts_as_open()
        })
        .cloned()
        .collect();
    let child_pending_on = |children: &[crate::state_store::SignalExecution],
                            canonical: &str,
                            side: LegSide,
                            symbol: &str|
     -> Result<Decimal, String> {
        let mut total = dec!(0);
        for child in children {
            if !child.status.counts_as_open() {
                continue;
            }
            let stale = chrono::DateTime::parse_from_rfc3339(&child.created_at)
                .map(|t| {
                    (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds() > 15 * 60
                })
                // Unparseable timestamp → treat as fresh: when we cannot
                // prove the order is dead, do not risk a second submit.
                .unwrap_or(false);
            if stale {
                continue;
            }
            for cl in &child.legs {
                if cl.side == side
                    && cl.symbol.eq_ignore_ascii_case(symbol)
                    && crate::state::canonical_venue_id(&cl.venue) == Some(canonical)
                {
                    let pending = ledger_qty_to_dec(cl.intended_qty, "child leg intent")?
                        - ledger_qty_to_dec(cl.filled_qty, "child leg fill")?;
                    total += pending.max(dec!(0));
                }
            }
        }
        Ok(total)
    };

    for sig in roots {
        let family: Vec<crate::state_store::SignalExecution> = all_signals
            .iter()
            .filter(|s| crate::state_store::is_flatten_child_id(&s.id, &sig.id))
            .cloned()
            .collect();

        let mut exposures: Vec<LegExposure> = Vec::new();
        let mut targets: Vec<CloseTarget> = Vec::new();
        let mut undetermined = false;
        let mut seen: std::collections::HashSet<(String, &'static str, String)> =
            std::collections::HashSet::new();
        let mut accounts: HashMap<&'static str, arbitrageur_core::position::AccountSnapshot> =
            HashMap::new();
        let mut unreadable: std::collections::HashSet<&'static str> =
            std::collections::HashSet::new();

        for leg in &sig.legs {
            let Some(canonical) = crate::state::canonical_venue_id(&leg.venue) else {
                undetermined = true;
                out.notes.push(format!(
                    "{}: venue '{}' is not a canonical venue",
                    sig.id, leg.venue
                ));
                continue;
            };
            if !seen.insert((canonical.to_string(), leg.side.as_str(), leg.symbol.clone())) {
                continue;
            }
            let entry_qty = ledger_qty_to_dec(leg.filled_qty, "root leg fill")?;
            let closed_qty = child_fills_on(&family, canonical, opposite(leg.side))?;
            let residual = (entry_qty - closed_qty).max(dec!(0));
            let expected_pos_side = match leg.side {
                LegSide::Long => Side::Buy,
                LegSide::Short => Side::Sell,
            };
            exposures.push(LegExposure {
                canonical,
                entry_side: leg.side,
                expected_pos_side,
                entry_qty,
                exact_symbol: leg.symbol.clone(),
            });
            if residual <= dec!(0) {
                continue;
            }
            if !accounts.contains_key(canonical) && !unreadable.contains(canonical) {
                match state.get_adapter(canonical) {
                    Some(adapter) => {
                        if !adapter.live_order_submission_implemented()
                            || !adapter.has_live_credentials()
                        {
                            unreadable.insert(canonical);
                            out.notes.push(format!(
                                "{}: venue '{}' lacks authenticated order-submission capability",
                                sig.id, leg.venue
                            ));
                        } else {
                            match adapter.fetch_account().await {
                                Ok(acc) => {
                                    accounts.insert(canonical, acc);
                                }
                                Err(e) => {
                                    unreadable.insert(canonical);
                                    out.notes.push(format!(
                                        "{}: cannot read {} account: {e}",
                                        sig.id, leg.venue
                                    ));
                                }
                            }
                        }
                    }
                    None => {
                        unreadable.insert(canonical);
                        out.notes.push(format!(
                            "{}: no adapter exists for venue '{}' in this environment",
                            sig.id, leg.venue
                        ));
                    }
                }
            }
            if unreadable.contains(canonical) {
                undetermined = true;
                continue;
            }
            let pos = match accounts.get(canonical) {
                Some(acc) => match state.get_adapter(canonical) {
                    Some(adapter) => match adapter.account_position_for(acc, &leg.symbol) {
                        Ok(position) => position,
                        Err(e) => {
                            undetermined = true;
                            out.notes.push(format!(
                                "{}: cannot resolve {} account position {}: {e}",
                                sig.id, leg.venue, leg.symbol
                            ));
                            None
                        }
                    },
                    None => None,
                },
                None => None,
            };
            match pos {
                None => {}
                Some(p) if p.side != expected_pos_side => {
                    undetermined = true;
                    out.notes.push(format!(
                        "{}: {} position side {:?} contradicts {:?} leg; leaving open",
                        sig.id, leg.venue, p.side, leg.side
                    ));
                }
                Some(p) => {
                    // Subtract qty already in flight on open children — a
                    // re-click while a close order works must not resubmit it.
                    let pending = child_pending_on(
                        &open_children,
                        canonical,
                        opposite(leg.side),
                        &leg.symbol,
                    )?;
                    let dispatchable = (residual - pending).max(dec!(0));
                    let basis = dispatchable.min(p.quantity);
                    // Leftover sweep: when the residual exceeds the closable
                    // quantity by less than one full ticker step, close the
                    // ENTIRE residual instead of stranding sub-step dust.
                    // Oversized reduce-only paper orders cap at the position.
                    let mut qty = basis;
                    let swept = sweep_flatten_qty(
                        dispatchable,
                        basis,
                        flatten_step_qty(state, &sig.symbol),
                    );
                    if swept > basis {
                        qty = swept;
                        out.notes.push(format!(
                            "{}: {} leg sweep closes entire residual {}/{} (sub-step leftover)",
                            sig.id, leg.venue, swept, basis
                        ));
                    }
                    if qty <= dec!(0) {
                        continue;
                    }
                    targets.push(CloseTarget {
                        venue: leg.venue.clone(),
                        exact_symbol: leg.symbol.clone(),
                        account_id: leg.account_id.clone(),
                        order_side: match p.side {
                            Side::Buy => Side::Sell,
                            Side::Sell => Side::Buy,
                        },
                        leg_side: opposite(leg.side),
                        qty,
                        unit_scale: leg.unit_scale,
                    });
                }
            }
        }

        if undetermined {
            continue;
        }

        let close_family = |note: &str| -> Result<(), String> {
            state
                .state_store
                .update_signal_status(&sig.id, SignalStatus::Closed, Some(note))
                .map_err(|e| e.to_string())?;
            for child in state
                .state_store
                .list_open_signals(Some(state.env().as_str()))
                .map_err(|e| e.to_string())?
                .into_iter()
                .filter(|s| {
                    crate::state_store::is_flatten_child_id(&s.id, &sig.id)
                        && s.status.counts_as_open()
                })
            {
                state
                    .state_store
                    .update_signal_status(&child.id, SignalStatus::Closed, Some(note))
                    .map_err(|e| e.to_string())?;
            }
            Ok(())
        };

        if targets.is_empty() {
            // Open children cover the residual only while their orders still
            // work — closing the family now would orphan their late fills
            // (resolve_flattened_parent skips a Closed parent) and could mark
            // a still-live position fully closed. Leave the parent open; the
            // reconciler closes it once the children reach a terminal state.
            if family.iter().any(|c| c.status.counts_as_open()) {
                out.notes.push(format!(
                    "{}: residual already covered by working close orders — awaiting settlement",
                    sig.id
                ));
                continue;
            }
            close_family(&format!("flatten {}: no residual venue position", sig.id))?;
            out.signals_closed += 1;
            if let Some((usd, _, fee_basis)) =
                close_flattened_spatial_lot(state, &sig, &family, close_status)
            {
                // Marked fills' exit fees sit inside `usd`'s exit-fee total
                // but their leg PnL was already counted — add the marked
                // fee share back so the increment is the uncounted rest.
                let marked_fees = marked_family_fees(state, &family);
                accumulate_realized_profit(state, is_live, usd + fee_basis + marked_fees);
            } else {
                // Family is closed but no lot covered the exit fills (lotless
                // signal, or a dead lot that can never match now). Sweep all
                // unmarked child fills — this is their last chance to count.
                accumulate_unlotted_family_fills(state, &sig, &family, true);
            }
            continue;
        }

        let close_id = format!("flat-{}-{:04x}", sig.id, rand::random::<u16>());
        let child_legs = targets
            .iter()
            .map(|t| -> Result<NewSignalLeg, String> {
                let intended = dec_to_f64(t.qty, "close quantity")?;
                if intended <= 0.0 {
                    return Err(format!("close quantity {} is not positive", t.qty));
                }
                Ok(NewSignalLeg {
                    account_id: t.account_id.clone(),
                    venue: t.venue.clone(),
                    side: t.leg_side,
                    symbol: Some(t.exact_symbol.clone()),
                    intended_qty: intended,
                    intended_price: None,
                    // Same instrument the parent leg fills in — inherit
                    // its scale so family delta math stays in shared units.
                    unit_scale: Some(t.unit_scale),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        state
            .state_store
            .record_signal_execution(
                &NewSignalExecution {
                    id: close_id.clone(),
                    strategy_id: sig.strategy_id.clone(),
                    symbol: sig.symbol.clone(),
                    route_id: sig.route_id.clone(),
                    expected_net_usd: None,
                    note: Some(format!("emergency flatten of {}", sig.id)),
                    legs: child_legs,
                },
                state.env().as_str(),
            )
            .map_err(|e| format!("Failed to persist flatten signal record: {e}"))?;
        // A child created this pass isn't in the `all_signals` snapshot —
        // join it to the pending set so a later root sharing venue+side sees
        // its in-flight qty.
        if let Ok(Some(child)) = state.state_store.get_signal(&close_id) {
            open_children.push(child);
        }

        let mut leg_desc = Vec::new();
        for t in &targets {
            match flatten_leg(FlattenLegArgs {
                state,
                venue: &t.venue,
                symbol: &t.exact_symbol,
                side: t.order_side,
                qty: t.qty,
                strategy_id: &sig.strategy_id,
                sig_id: &sig.id,
                notes: &mut out.notes,
                execution_type,
            })
            .await
            {
                Ok(outcome) => {
                    // Persist the accepted order id BEFORE any fill write so a
                    // working order is always reachable by the reconciliation
                    // supervisor — a zero-fill ack must not orphan the order.
                    if let Some(oid) = outcome.order_id.as_deref() {
                        if let Err(e) = state.state_store.record_leg_order_id(
                            &close_id,
                            t.leg_side,
                            oid,
                            outcome.client_order_id.as_deref(),
                        ) {
                            out.notes.push(format!(
                                "{}: {} leg order {oid} accepted but id persistence failed: {e}",
                                sig.id, t.venue
                            ));
                        }
                    }
                    match outcome.fill {
                        Some((qty, px, fee)) => {
                            state
                                .state_store
                                .apply_leg_fill(
                                    &close_id,
                                    t.leg_side,
                                    dec_to_f64(qty, "fill quantity")?,
                                    dec_to_f64(px, "fill price")?,
                                    dec_to_f64(fee.unwrap_or_default(), "fill fee")?,
                                    outcome.order_id.as_deref(),
                                )
                                .map_err(|e| e.to_string())?;
                            out.legs_closed += 1;
                            leg_desc
                                .push(format!("{} {:?} {}/{}", t.venue, t.leg_side, qty, t.qty));
                            if qty < t.qty {
                                out.notes.push(format!(
                                    "{}: {} leg partially closed {}/{}",
                                    sig.id, t.venue, qty, t.qty
                                ));
                            }
                        }
                        None => {
                            out.notes.push(format!(
                                "{}: {} leg got no fill (empty book or still working)",
                                sig.id, t.venue
                            ));
                        }
                    }
                }
                Err(e) => {
                    out.notes
                        .push(format!("{}: {} leg close failed: {e}", sig.id, t.venue));
                }
            }
        }

        let family_now: Vec<crate::state_store::SignalExecution> = state
            .state_store
            .list_signals(None, None)
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|s| crate::state_store::is_flatten_child_id(&s.id, &sig.id))
            .collect();

        let mut fully_flat = true;
        for exp in &exposures {
            let residual = (exp.entry_qty
                - child_fills_on(&family_now, exp.canonical, opposite(exp.entry_side))?)
            .max(dec!(0));
            if residual <= dec!(0) {
                continue;
            }
            let Some(adapter) = state.get_adapter(exp.canonical) else {
                fully_flat = false;
                continue;
            };
            match adapter.fetch_account().await {
                Ok(acc) => match adapter.account_position_for(&acc, &exp.exact_symbol) {
                    Ok(None) => {}
                    Ok(Some(p)) if p.side != exp.expected_pos_side => {
                        fully_flat = false;
                        out.notes.push(format!(
                            "{}: {} position side {:?} contradicts {:?} leg; leaving open",
                            sig.id, exp.canonical, p.side, exp.entry_side
                        ));
                    }
                    Ok(Some(_)) => fully_flat = false,
                    Err(e) => {
                        fully_flat = false;
                        out.notes.push(format!(
                            "{}: cannot resolve {} account position {} after close: {e}",
                            sig.id, exp.canonical, exp.exact_symbol
                        ));
                    }
                },
                Err(_) => fully_flat = false,
            }
        }

        let note = format!("emergency flatten via {close_id}");
        // Realized PnL comes from the attributed ledger lot when the exit
        // fills cover it; single-leg residuals (e.g. triangle legs) realize
        // purely through their signal legs and keep the 0.0 log convention.
        let mut flatten_realized: Option<(f64, f64, f64)> = None;
        let (venue_a, venue_b) = (
            sig.legs
                .first()
                .map(|l| l.venue.clone())
                .unwrap_or_default(),
            sig.legs.get(1).map(|l| l.venue.clone()).unwrap_or_default(),
        );
        if fully_flat {
            close_family(&note)?;
            out.signals_closed += 1;
            flatten_realized = close_flattened_spatial_lot(state, &sig, &family_now, close_status);
            if let Some((usd, _, fee_basis)) = flatten_realized {
                let marked_fees = marked_family_fees(state, &family_now);
                accumulate_realized_profit(state, is_live, usd + fee_basis + marked_fees);
            }
            if let Some(bot) = state.radar_bot.lock().clone() {
                bot.forget_position(&sig.symbol, &venue_a, &venue_b);
                // Scoped to the signal's own environment, not the current
                // one — the tracker row lives where the position opened.
                let _ = state.state_store.delete_bot_position(
                    &sig.symbol,
                    &venue_a,
                    &venue_b,
                    sig.environment.as_str(),
                );
                let _ = state.state_store.delete_bot_position(
                    &sig.symbol,
                    &venue_b,
                    &venue_a,
                    sig.environment.as_str(),
                );
            }
        } else {
            state
                .state_store
                .update_signal_status(&sig.id, SignalStatus::Unresolved, Some(&note))
                .map_err(|e| e.to_string())?;
            state
                .state_store
                .update_signal_status(&close_id, SignalStatus::Unresolved, Some(&note))
                .map_err(|e| e.to_string())?;
        }
        // Lotless signals (1-leg residuals, imbalanced pairs, positions
        // whose lot never persisted) realize only through child fills —
        // cash moved whether or not the position is fully flat. The sweep
        // also picks up fills that arrived late via reconciliation on
        // earlier children (per-leg marks make each fill count once).
        if flatten_realized.is_none() {
            accumulate_unlotted_family_fills(state, &sig, &family_now, false);
        }

        record_execution_log(
            state,
            ExecutionLogEntry {
                id: format!("exec-flatten-{}-{}", cycle, sig.id),
                timestamp: Utc::now().to_rfc3339(),
                trade_type: "Flatten".into(),
                strategy_id: crate::attribution::attribute_strategy_id(&sig.strategy_id),
                symbol: sig.symbol.clone(),
                venue_a,
                venue_b,
                leg_a_status: leg_desc.first().cloned().unwrap_or_else(|| "—".into()),
                leg_b_status: leg_desc.get(1).cloned().unwrap_or_else(|| "—".into()),
                success: fully_flat,
                gross_profit: 0.0,
                fees: 0.0,
                net_profit: flatten_realized.map(|(usd, _, _)| usd).unwrap_or(0.0),
                latency_us: 0,
                note: if fully_flat {
                    format!("Emergency flatten closed signal {}", sig.id)
                } else {
                    format!(
                        "Emergency flatten PARTIAL for signal {} — residual exposure",
                        sig.id
                    )
                },
                direction: Some("Emergency Flatten".into()),
                trigger_spread_bps: None,
                baseline_mean_spread_bps: None,
                position_step: None,
                max_units: None,
                nominal_hurdle_bps: None,
                is_live,
            },
        );
    }

    out.residual_signals = state
        .state_store
        .list_signals(symbol, None)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|s| {
            // A signal-scoped flatten measures residual against that
            // family only — unrelated open carries must not keep the
            // scoped row "actionable" forever.
            (only_signal_id.is_none() || only_signal_id == Some(s.id.as_str()))
                && !s.id.starts_with("flat-")
                && !s.id.starts_with("trim-")
                && s.strategy_id.eq_ignore_ascii_case(&sid)
                && s.status.counts_as_open()
        })
        .count();

    // Flatten fills move the expected-equity baselines forward in both
    // environments so the next attribution poll doesn't read own fills as
    // external flows.
    refresh_equity_expectations_after_fills(state, &flatten_equity_before).await;
    maybe_record_margin_snapshots(state, Utc::now().timestamp_millis()).await;

    Ok(out)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn execute_spatial_arbitrage(
    symbol: String,
    buy_venue: String,
    sell_venue: String,
    order_qty: f64,
    direction: Option<String>,
    trigger_spread_bps: Option<f64>,
    baseline_mean_spread_bps: Option<f64>,
    position_step: Option<u32>,
    max_units: Option<u32>,
    nominal_hurdle_bps: Option<f64>,
    predicted_win_probability: Option<f64>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SimStepResultDto, String> {
    match execute_spatial_arbitrage_inner(
        &symbol,
        &buy_venue,
        &sell_venue,
        order_qty,
        direction,
        trigger_spread_bps,
        baseline_mean_spread_bps,
        position_step,
        max_units,
        nominal_hurdle_bps,
        None,
        "futures",
        predicted_win_probability.map(|p| EntryPrediction {
            kind: "reversion",
            win_probability: p,
            expected_net_bps: None,
        }),
        None,
        &state,
    )
    .await
    {
        Ok(r) => Ok(r),
        Err(e) => {
            record_execution_failure(
                &state,
                "Spatial",
                "futures",
                &symbol,
                &buy_venue,
                &sell_venue,
                &e,
            );
            Err(e)
        }
    }
}

#[tauri::command]
pub async fn check_spatial_convergence(
    symbol: Option<String>,
    strategy_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<ConvergenceCheckDto>, String> {
    check_spatial_convergence_impl(symbol.as_deref(), &strategy_id, &state).await
}

#[tauri::command]
pub async fn execute_cross_ticker_arbitrage(
    long_symbol: String,
    short_symbol: String,
    long_venue: String,
    short_venue: String,
    order_qty: f64,
    net_spread_bps: Option<f64>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SimStepResultDto, String> {
    match execute_cross_ticker_arbitrage_impl(
        &long_symbol,
        &short_symbol,
        &long_venue,
        &short_venue,
        order_qty,
        net_spread_bps,
        &state,
    )
    .await
    {
        Ok(r) => Ok(r),
        Err(e) => {
            record_execution_failure(
                &state,
                "Spatial",
                "futures",
                &long_symbol,
                &long_venue,
                &short_venue,
                &e,
            );
            Err(e)
        }
    }
}

#[tauri::command]
pub async fn fetch_multi_market_quotes(
    symbols: Vec<String>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<TickerMarketQuoteDto>, String> {
    fetch_multi_market_quotes_impl(symbols, &state).await
}

#[tauri::command]
pub async fn execute_prediction_arbitrage(
    market_title: String,
    buy_yes_venue: String,
    buy_no_venue: String,
    quantity: f64,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SimStepResultDto, String> {
    match execute_prediction_arbitrage_impl(
        &market_title,
        &buy_yes_venue,
        &buy_no_venue,
        quantity,
        &state,
    )
    .await
    {
        Ok(r) => Ok(r),
        Err(e) => {
            record_execution_failure(
                &state,
                "Prediction",
                "prediction",
                &market_title,
                &buy_yes_venue,
                &buy_no_venue,
                &e,
            );
            Err(e)
        }
    }
}

#[tauri::command]
pub async fn get_risk_overview(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<RiskOverviewDto, String> {
    get_risk_overview_impl(&state).await
}

#[tauri::command]
pub async fn reset_circuit_breaker(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<String, String> {
    reset_circuit_breaker_impl(&state)
}

#[tauri::command]
pub async fn trigger_emergency_killswitch(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<String, String> {
    trigger_emergency_killswitch_impl(&state)
}

#[tauri::command]
pub async fn flatten_ticker_positions(
    symbol: Option<String>,
    strategy_id: String,
    execution_type: Option<String>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<FlattenResultDto, String> {
    flatten_ticker_positions_with_type_impl(
        symbol.as_deref(),
        &strategy_id,
        execution_type.as_deref(),
        // Emergency market flattens are risk-driven unwinds; a plain
        // operator flatten is an orderly exit.
        match execution_type.as_deref() {
            Some("emergency_market") => CycleStatus::StoppedOut,
            _ => CycleStatus::Closed,
        },
        &state,
    )
    .await
}

/// Re-price the live edge of an open "futures" pair from freshly validated
/// books. `Ok(None)` means the edge is currently unpriceable (stale/missing
/// books or unpriceable depth) — the caller holds (Unwatched) rather than
/// exiting blind. `Err` is reserved for fail-closed input faults.
async fn live_spatial_edge(
    state: &AppEngineState,
    symbol: &str,
    buy_venue: &str,
    sell_venue: &str,
    qty: f64,
) -> Result<Option<f64>, String> {
    let buy_adapter = state
        .get_adapter(buy_venue)
        .ok_or_else(|| format!("Buy venue '{buy_venue}' is not admitted for paper execution"))?;
    let sell_adapter = state
        .get_adapter(sell_venue)
        .ok_or_else(|| format!("Sell venue '{sell_venue}' is not admitted for paper execution"))?;
    let buy_vid = buy_adapter.venue_id();
    let sell_vid = sell_adapter.venue_id();
    let qty_dec = Decimal::from_f64_retain(qty)
        .filter(|q| *q > dec!(0))
        .ok_or_else(|| "leg quantity is not a positive number".to_string())?;
    let fetch_book = async |adapter: &Arc<dyn ExchangeAdapter>, vid: &VenueId, side: &str| {
        let fresh = match state.market_data_hub.get_valid_orderbook(
            vid,
            symbol,
            Some(std::time::Duration::from_secs(2)),
        ) {
            Some(b) => b,
            None => {
                let b = adapter
                    .fetch_orderbook(symbol)
                    .await
                    .map_err(|e| format!("No fresh {side} orderbook for {symbol}: {e}"))?;
                validate_adapter_book(&b, vid, symbol)
                    .map_err(|e| format!("{side} order book rejected: {e}"))?;
                state.market_data_hub.update_orderbook(b.clone());
                b
            }
        };
        validate_adapter_book(&fresh, vid, symbol)
            .map_err(|e| format!("{side} order book rejected: {e}"))?;
        Ok::<OrderBook, String>(fresh)
    };
    let book_buy = fetch_book(&buy_adapter, &buy_vid, "buy-side").await?;
    let book_sell = fetch_book(&sell_adapter, &sell_vid, "sell-side").await?;
    match compute_route_economics(
        &book_buy,
        &book_sell,
        qty_dec,
        effective_venue_fee_bps(state, buy_vid.as_str()),
        effective_venue_fee_bps(state, sell_vid.as_str()),
        0.0,
    ) {
        Some(econ) if econ.expected_net_bps.is_finite() => Ok(Some(econ.expected_net_bps)),
        _ => Ok(None),
    }
}

/// Evaluate every open "futures" pair position (optionally scoped to one
/// symbol) against its entry baseline and unwind converge/stop/timeout exits
/// through the existing symbol-scoped flatten path. Unwinding marks the
/// signal `closed` and flattens the paper legs, which frees the pair slot:
/// the blocking-ledger and paper-position gates both clear, so re-entry is
/// allowed afterwards. Positions without a baseline, without a live quote,
/// or whose legs do not form a long/short pair report `unwatched` and are
/// never auto-exited; the manual flatten path stays available for them.
///
/// The one-open-pair-per-symbol cap is intentionally kept (see the guard in
/// `execute_spatial_arbitrage_inner`): paper accounts are shared per venue,
/// so per-pair margin/PnL attribution is impossible without sub-accounting,
/// and flatten is symbol-scoped — it would cross-close independent pairs on
/// the same symbol. Multi-pair safety would additionally require per-pair
/// position keys in the ledger, symbol+pair-scoped flatten, and per-pair
/// margin isolation in the paper exchanges.
/// Liquidation-distance trim for one held spatial pair. When either leg's
/// live/paper liquidation distance drops under the per-strategy threshold
/// (`futures_liq_trim_pct`), both legs reduce by the same qty to stay
/// delta-neutral; a remainder below one venue step closes the pair via the
/// normal flatten path. Trims dispatch as `emergency_market` — liquidation
/// protection does not wait for passive fills.
///
/// Fail-closed on telemetry: live legs evaluate only against
/// `last_good_accounts` fresher than 120s (the risk-overview poll keeps it
/// warm); a missing position, missing liquidation price, or stale read
/// skips the check rather than trimming blind. 60s cooldown per signal so
/// the 5s supervisor cannot bleed a pair step-by-step.
/// Pure trim evaluation over a pair of resolved positions: the smaller
/// liquidation distance (fraction → percent) and the smaller leg quantity
/// feed the shared `evaluate_liq_trim`. Missing liquidation prices,
/// nonpositive quantities, or a leg-side mismatch all fail closed (None).
pub(crate) fn liq_trim_decision_for_positions(
    long_pos: &arbitrageur_core::position::Position,
    short_pos: &arbitrageur_core::position::Position,
    step: Decimal,
    threshold_pct: f64,
) -> Option<(arbitrageur_strategy::pair_trim::PairTrimDecision, f64)> {
    use arbitrageur_strategy::pair_trim::evaluate_liq_trim;
    if long_pos.side != Side::Buy || short_pos.side != Side::Sell {
        return None;
    }
    let dist_pct = |p: &arbitrageur_core::position::Position| -> Option<f64> {
        let d = dec_opt_f64(p.liquidation_distance_pct()?)? * 100.0;
        d.is_finite().then_some(d)
    };
    let qty =
        |p: &arbitrageur_core::position::Position| dec_opt_f64(p.quantity).filter(|q| *q > 0.0);
    let min_dist = dist_pct(long_pos)?.min(dist_pct(short_pos)?);
    let min_qty = qty(long_pos)?.min(qty(short_pos)?);
    let step_f = dec_opt_f64(step).filter(|s| *s > 0.0)?;
    Some((
        evaluate_liq_trim(min_dist, min_qty, step_f, Some(threshold_pct)),
        min_dist,
    ))
}

async fn maybe_liq_trim_pair(
    state: &AppEngineState,
    sig: &crate::state_store::SignalExecution,
    buy: &crate::state_store::SignalLeg,
    sell: &crate::state_store::SignalLeg,
    threshold_pct: f64,
) -> Option<(&'static str, &'static str, String, bool)> {
    use arbitrageur_strategy::pair_trim::PairTrimDecision;

    // Cooldown — positions are symbol-level, so the 60s dispatch cooldown
    // keys on the symbol: two open signals on the same pair must not
    // double-trim against the same cached account read.
    {
        let last = state.liq_trim_at.lock();
        if let Some(t) = last.get(&sig.symbol) {
            if t.elapsed() < std::time::Duration::from_secs(60) {
                return None;
            }
        }
    }

    // Resolve each leg's venue position (side-checked). Fresh cache only —
    // never fetch here (REST pressure), never trim on stale telemetry.
    // Cache keys are env-namespaced (`{env}-{venue}`).
    let env_str = state.env().as_str();
    let resolve_pos = async |leg: &crate::state_store::SignalLeg| {
        let canonical = crate::state::canonical_venue_id(&leg.venue)?;
        let key = format!("{env_str}-{canonical}");
        let (acc, ts) = state.last_good_accounts.lock().get(&key)?.clone();
        if Utc::now().timestamp_millis().saturating_sub(ts) > 120_000 {
            return None;
        }
        let adapter = state.get_adapter(canonical)?;
        adapter.account_position_for(&acc, &leg.symbol).ok()?
    };
    let (Some(long_pos), Some(short_pos)) = (resolve_pos(buy).await, resolve_pos(sell).await)
    else {
        return None;
    };

    let step = flatten_step_qty(state, &sig.symbol);
    let (decision, min_dist_pct) =
        liq_trim_decision_for_positions(&long_pos, &short_pos, step, threshold_pct)?;
    match decision {
        PairTrimDecision::Hold => None,
        PairTrimDecision::TrimBoth { qty } => {
            // Serialize against the manual flatten / residual dispatcher /
            // extclose paths — a trim decided on a position a concurrent
            // close is already reducing would double-dispatch.
            let _exec_guard = state.execution_lock.lock().await;
            state
                .liq_trim_at
                .lock()
                .insert(sig.symbol.clone(), std::time::Instant::now());
            // Never stack trims: a prior `trim-` child whose orders are
            // still working means the cached positions predate its fills —
            // deciding on them again would double-reduce the same exposure.
            let trim_in_flight = state
                .state_store
                .list_open_signals(Some(state.env().as_str()))
                .map(|sigs| {
                    sigs.iter().any(|s| {
                        s.id.starts_with("trim-")
                            && crate::state_store::is_flatten_child_id(&s.id, &sig.id)
                            && s.status.counts_as_open()
                    })
                })
                .unwrap_or(true);
            if trim_in_flight {
                return Some((
                    "hold",
                    "liq_trim",
                    "liq trim already in flight — awaiting settlement".into(),
                    false,
                ));
            }
            let trim_dec = Decimal::from_f64_retain(qty)?;
            let mut note = format!(
                "liq trim: leg within {min_dist_pct:.1}% of liquidation — reducing both legs by {qty}"
            );
            // `trim-` (not `flat-`) prefix: trim fills reduce the open
            // position but realize separately — the close-agg must exclude
            // them or a trimmed lot's remaining qty could never match the
            // final flatten's fills, leaving the lot Open forever.
            let close_id = format!("trim-{}-{:04x}", sig.id, rand::random::<u16>());
            let child_legs: Vec<crate::state_store::NewSignalLeg> = [
                (buy, crate::state_store::LegSide::Short),
                (sell, crate::state_store::LegSide::Long),
            ]
            .iter()
            .map(|(leg, close_side)| crate::state_store::NewSignalLeg {
                account_id: leg.account_id.clone(),
                venue: leg.venue.clone(),
                side: *close_side,
                symbol: Some(leg.symbol.clone()),
                intended_qty: qty,
                intended_price: None,
                unit_scale: Some(leg.unit_scale),
            })
            .collect();
            if let Err(e) = state.state_store.record_signal_execution(
                &crate::state_store::NewSignalExecution {
                    id: close_id.clone(),
                    strategy_id: sig.strategy_id.clone(),
                    symbol: sig.symbol.clone(),
                    route_id: sig.route_id.clone(),
                    expected_net_usd: None,
                    note: Some(format!("liq trim of {}", sig.id)),
                    legs: child_legs,
                },
                state.env().as_str(),
            ) {
                return Some((
                    "hold",
                    "liq_trim",
                    format!("liq trim record failed: {e}"),
                    false,
                ));
            }
            let mut notes = Vec::new();
            for (leg, pos_side) in [(buy, Side::Buy), (sell, Side::Sell)] {
                let order_side = match pos_side {
                    Side::Buy => Side::Sell,
                    Side::Sell => Side::Buy,
                };
                let close_side = match leg.side {
                    crate::state_store::LegSide::Long => crate::state_store::LegSide::Short,
                    crate::state_store::LegSide::Short => crate::state_store::LegSide::Long,
                };
                match flatten_leg(FlattenLegArgs {
                    state,
                    venue: &leg.venue,
                    symbol: &leg.symbol,
                    side: order_side,
                    qty: trim_dec,
                    strategy_id: &sig.strategy_id,
                    sig_id: &sig.id,
                    notes: &mut notes,
                    execution_type: Some("emergency_market"),
                })
                .await
                {
                    Ok(outcome) => {
                        if let Some(oid) = outcome.order_id.as_deref() {
                            if let Err(e) = state.state_store.record_leg_order_id(
                                &close_id,
                                close_side,
                                oid,
                                outcome.client_order_id.as_deref(),
                            ) {
                                note.push_str(&format!("; {oid} id persistence failed: {e}"));
                            }
                        }
                        match outcome.fill {
                            Some((q, p, f)) => {
                                match (
                                    dec_to_f64(q, "trim qty"),
                                    dec_to_f64(p, "trim price"),
                                    dec_to_f64(f.unwrap_or_default(), "trim fee"),
                                ) {
                                    (Ok(qf), Ok(pf), Ok(ff)) => {
                                        if let Err(e) = state.state_store.apply_leg_fill(
                                            &close_id,
                                            close_side,
                                            qf,
                                            pf,
                                            ff,
                                            outcome.order_id.as_deref(),
                                        ) {
                                            note.push_str(&format!(
                                                "; {} fill record failed: {e}",
                                                leg.venue
                                            ));
                                        }
                                    }
                                    _ => note.push_str(&format!(
                                        "; {} fill conversion failed",
                                        leg.venue
                                    )),
                                }
                            }
                            None => {
                                note.push_str(&format!("; {} trim order still working", leg.venue))
                            }
                        }
                    }
                    Err(e) => note.push_str(&format!("; {} leg trim failed: {e}", leg.venue)),
                }
            }
            // Realize the trim: the filled portion is a partial close whose
            // cash already moved — accumulate it and shrink the open lot so
            // the final flatten's fills still match the remaining qty.
            // Unfilled/working legs leave their qty untouched; a late fill
            // lands via reconciliation against the family (the close-agg
            // excludes `trim-` children, so only the shrink may lag — the
            // boot stale-lot reconcile bounds the residual edge).
            let trim_usd = match realize_liq_trim(
                state,
                sig,
                &close_id,
                buy.venue.as_str(),
                sell.venue.as_str(),
            ) {
                Ok(usd) => {
                    if usd != 0.0 {
                        note.push_str(&format!("; trim realized {usd:.4} USD"));
                    }
                    usd
                }
                Err(e) => {
                    note.push_str(&format!("; trim realization failed: {e}"));
                    0.0
                }
            };
            for n in notes {
                note.push_str(&format!("; {n}"));
            }
            // Risk-reducing action: mirror flatten's durable log row so the
            // trim is visible in the Log Monitor, not only in signal history.
            record_execution_log(
                state,
                ExecutionLogEntry {
                    id: format!("exec-{close_id}"),
                    timestamp: Utc::now().to_rfc3339(),
                    trade_type: "Trim".into(),
                    strategy_id: crate::attribution::attribute_strategy_id(&sig.strategy_id),
                    symbol: sig.symbol.clone(),
                    venue_a: buy.venue.clone(),
                    venue_b: sell.venue.clone(),
                    leg_a_status: format!("trim {qty} short"),
                    leg_b_status: format!("trim {qty} long"),
                    success: true,
                    gross_profit: 0.0,
                    fees: 0.0,
                    net_profit: trim_usd,
                    latency_us: 0,
                    note: note.clone(),
                    direction: Some("Liquidation Trim".into()),
                    trigger_spread_bps: None,
                    baseline_mean_spread_bps: None,
                    position_step: None,
                    max_units: None,
                    nominal_hurdle_bps: None,
                    is_live: state.is_live(),
                },
            );
            Some(("trim", "liq_trim", note, false))
        }
        PairTrimDecision::CloseBoth => {
            // No outer execution_lock here: `flatten_ticker_positions_impl`
            // acquires it internally, and a second take of the non-reentrant
            // Tokio mutex would self-deadlock (wedging every execution path
            // behind it). The dedupe marker write is atomic on its own mutex.
            state
                .liq_trim_at
                .lock()
                .insert(sig.symbol.clone(), std::time::Instant::now());
            let reason = match flatten_ticker_positions_impl(
                Some(&sig.symbol),
                "futures",
                CycleStatus::StoppedOut,
                state,
            )
            .await
            {
                Ok(r) => format!(
                    "liq trim: leg within {min_dist_pct:.1}% of liquidation and remainder below one step — closed pair ({} signals)",
                    r.signals_closed
                ),
                Err(e) => format!(
                    "liq trim: leg within {min_dist_pct:.1}% of liquidation — close failed: {e}"
                ),
            };
            Some(("exit", "liq_trim", reason, true))
        }
    }
}

pub async fn check_spatial_convergence_impl(
    symbol: Option<&str>,
    strategy_id: &str,
    state: &AppEngineState,
) -> Result<Vec<ConvergenceCheckDto>, String> {
    use arbitrageur_strategy::spatial_exit::{
        evaluate_spatial_exit, SpatialExitConfig, SpatialExitTrigger, SpatialExitVerdict,
    };
    let sid = crate::state_store::normalize_strategy_id(strategy_id).map_err(|e| e.to_string())?;
    if sid != "futures" {
        return Err(
            "convergence check supports the 'futures' strategy only (radar positions have their own bot lifecycle)"
                .into(),
        );
    }
    let cfg = SpatialExitConfig {
        max_hold_secs: i64::try_from(state.settings.read().auto_exit_max_hold_secs)
            .unwrap_or(i64::MAX),
        ..SpatialExitConfig::default()
    };
    cfg.validate().map_err(|e| e.to_string())?;
    if let Some(s) = symbol {
        validate_market_symbol(s)?;
    }
    let now_ms = Utc::now().timestamp_millis();
    let signals = state
        .state_store
        .list_signals(symbol, None)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for sig in signals.iter().filter(|s| {
        s.strategy_id.eq_ignore_ascii_case(&sid)
            && s.status.counts_as_open()
            && !s.id.starts_with("flat-")
            && !s.id.starts_with("trim-")
    }) {
        let baseline = state
            .state_store
            .spatial_baseline_for_signal(&sig.id)
            .map_err(|e| e.to_string())?;
        let entry_net = baseline.as_ref().map(|b| b.entry_net_bps);
        let entry_ts = baseline.as_ref().map(|b| b.entry_ts_ms).unwrap_or(0);
        let verdict_of = |verdict: &str,
                          trigger: Option<String>,
                          reason: String,
                          live: Option<f64>,
                          unwound: bool| {
            ConvergenceCheckDto {
                signal_id: sig.id.clone(),
                symbol: sig.symbol.clone(),
                verdict: verdict.to_string(),
                trigger,
                reason,
                live_net_bps: live,
                entry_net_bps: entry_net,
                unwound,
            }
        };
        let buy_leg = sig
            .legs
            .iter()
            .find(|l| l.side == crate::state_store::LegSide::Long);
        let sell_leg = sig
            .legs
            .iter()
            .find(|l| l.side == crate::state_store::LegSide::Short);
        let (Some(buy), Some(sell)) = (buy_leg, sell_leg) else {
            out.push(verdict_of(
                "unwatched",
                None,
                "signal legs do not form a long/short pair".to_string(),
                None,
                false,
            ));
            continue;
        };
        if !buy.filled_qty.is_finite() || buy.filled_qty <= 0.0 {
            out.push(verdict_of(
                "unwatched",
                None,
                "long leg has no positive fill to size the live quote".to_string(),
                None,
                false,
            ));
            continue;
        }
        let live =
            match live_spatial_edge(state, &sig.symbol, &buy.venue, &sell.venue, buy.filled_qty)
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    out.push(verdict_of("unwatched", None, e, None, false));
                    continue;
                }
            };
        match evaluate_spatial_exit(entry_net, live, entry_ts, now_ms, &cfg) {
            SpatialExitVerdict::Hold => {
                // Liquidation-distance trim rides the same 5s supervisor:
                // while the edge is inside the band, still protect the
                // pair against a leg approaching liquidation.
                let trim_threshold = state
                    .strategy_params
                    .read()
                    .get("futures")
                    .and_then(|p| p.futures_liq_trim_pct)
                    .filter(|t| t.is_finite() && *t > 0.0 && *t <= 100.0);
                let trim = match trim_threshold {
                    Some(t) => maybe_liq_trim_pair(state, sig, buy, sell, t).await,
                    None => None,
                };
                match trim {
                    Some((v, trig, reason, unwound)) => {
                        out.push(verdict_of(v, Some(trig.to_string()), reason, live, unwound))
                    }
                    None => out.push(verdict_of(
                        "hold",
                        None,
                        "edge inside band".to_string(),
                        live,
                        false,
                    )),
                }
            }
            SpatialExitVerdict::Unwatched { reason } => {
                out.push(verdict_of("unwatched", None, reason, live, false));
            }
            SpatialExitVerdict::Exit { trigger, reason } => {
                // Convergence unwinds route through the symbol-scoped flatten
                // path, so the leftover sweep (sub-step residual guard) and
                // the paper-flat reconciliation apply here unchanged.
                let close_status = match trigger {
                    SpatialExitTrigger::Converged => CycleStatus::Closed,
                    SpatialExitTrigger::Stopped => CycleStatus::StoppedOut,
                    SpatialExitTrigger::TimedOut => CycleStatus::TimedOut,
                };
                match flatten_ticker_positions_impl(Some(&sig.symbol), &sid, close_status, state)
                    .await
                {
                    Ok(r) if r.residual_signals == 0 => out.push(verdict_of(
                        "exit",
                        Some(trigger.as_str().to_string()),
                        format!("{reason}; unwound {} signal(s)", r.signals_closed),
                        live,
                        true,
                    )),
                    Ok(r) => out.push(verdict_of(
                        "exit",
                        Some(trigger.as_str().to_string()),
                        format!(
                            "{reason}; flatten left {} residual signal(s) — slot stays blocked",
                            r.residual_signals
                        ),
                        live,
                        false,
                    )),
                    Err(e) => out.push(verdict_of(
                        "exit",
                        Some(trigger.as_str().to_string()),
                        format!("{reason}; flatten failed: {e}"),
                        live,
                        false,
                    )),
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod account_read_tests {
    use super::*;
    use arbitrageur_core::position::AccountSnapshot;
    use arbitrageur_core::types::VenueId;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingAccountAdapter {
        calls: AtomicUsize,
        delay_ms: u64,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl ExchangeAdapter for CountingAccountAdapter {
        fn venue_id(&self) -> VenueId {
            VenueId::new("mock")
        }
        fn name(&self) -> &str {
            "mock"
        }
        async fn fetch_markets(
            &self,
        ) -> anyhow::Result<Vec<arbitrageur_core::market::MarketMetadata>> {
            unimplemented!()
        }
        async fn fetch_orderbook(&self, _s: &str) -> anyhow::Result<OrderBook> {
            unimplemented!()
        }
        async fn fetch_ticker(&self, _s: &str) -> anyhow::Result<arbitrageur_core::quote::Ticker> {
            unimplemented!()
        }
        async fn fetch_funding_rate(
            &self,
            _s: &str,
        ) -> anyhow::Result<arbitrageur_core::quote::FundingRate> {
            unimplemented!()
        }
        async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            }
            if self.fail {
                anyhow::bail!("venue account endpoint down in fixture");
            }
            Ok(AccountSnapshot {
                venue: VenueId::new("mock"),
                equity: dec!(1000),
                free_margin: dec!(900),
                maintenance_margin: dec!(100),
                margin_usage_pct: dec!(10),
                balances: vec![],
                positions: vec![],
                timestamp: Utc::now(),
            })
        }
        fn has_live_credentials(&self) -> bool {
            true
        }
        async fn place_order(
            &self,
            _o: arbitrageur_core::order::OrderRequest,
        ) -> anyhow::Result<arbitrageur_core::order::OrderResponse> {
            unimplemented!()
        }
        async fn cancel_order(
            &self,
            _r: arbitrageur_core::order::CancelRequest,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn live_account_reads_coalesce_within_fresh_window() {
        // Three pollers arriving inside the fresh window must produce ONE
        // venue REST call — the shape that 429'd Orderly /v1/positions.
        let state = AppEngineState::new();
        let inner = Arc::new(CountingAccountAdapter {
            calls: AtomicUsize::new(0),
            delay_ms: 0,
            fail: false,
        });
        let adapter: Arc<dyn ExchangeAdapter> = inner.clone();
        for _ in 0..3 {
            let (acc, stale_age) = live_account_or_cached(&state, "mock", &adapter)
                .await
                .expect("account read");
            assert_eq!(acc.equity, dec!(1000));
            // A TTL-window hit is fresh truth, not a degraded read.
            assert!(stale_age.is_none());
        }
        assert_eq!(
            inner.calls.load(Ordering::SeqCst),
            1,
            "coalesced reads must share one venue call"
        );
    }

    #[tokio::test]
    async fn concurrent_live_account_reads_share_one_fetch() {
        // Two callers arriving while the first fetch is still in flight
        // must both await the same call (singleflight), not fire a second.
        let state = Arc::new(AppEngineState::new());
        let inner = Arc::new(CountingAccountAdapter {
            calls: AtomicUsize::new(0),
            delay_ms: 150,
            fail: false,
        });
        let adapter: Arc<dyn ExchangeAdapter> = inner.clone();
        let s2 = state.clone();
        let a2 = adapter.clone();
        let h = tokio::spawn(async move { live_account_or_cached(&s2, "mock", &a2).await });
        let r1 = live_account_or_cached(&state, "mock", &adapter).await;
        let r2 = h.await.expect("join");
        assert!(r1.is_ok() && r2.is_ok());
        assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn live_account_failure_falls_back_to_last_known_good() {
        // Seed a good snapshot, then fail: callers get cached figures with
        // the real age — never fabricated zeros and never an Err while a
        // cached value exists.
        let state = AppEngineState::new();
        let good = Arc::new(CountingAccountAdapter {
            calls: AtomicUsize::new(0),
            delay_ms: 0,
            fail: false,
        });
        live_account_or_cached(&state, "mock", &(good.clone() as Arc<dyn ExchangeAdapter>))
            .await
            .expect("seed read");
        // Age the cache past the fresh window so the next call re-fetches.
        // Cache keys are env-namespaced (`{env}-{vid}`) — default env is testnet.
        {
            let mut map = state.last_good_accounts.lock();
            if let Some(entry) = map.get_mut("testnet-mock") {
                entry.1 -= 10_000;
            }
        }
        let bad: Arc<dyn ExchangeAdapter> = Arc::new(CountingAccountAdapter {
            calls: AtomicUsize::new(0),
            delay_ms: 0,
            fail: true,
        });
        let (acc, stale_age) = live_account_or_cached(&state, "mock", &bad)
            .await
            .expect("cached fallback");
        assert_eq!(acc.equity, dec!(1000));
        assert!(
            stale_age.expect("failed read must report cache age") >= 10_000,
            "age must reflect the real snapshot timestamp"
        );
    }

    #[tokio::test]
    async fn live_account_failure_without_cache_errors() {
        // Fail-closed: a venue that has never read successfully yields Err —
        // callers must not see fabricated account figures.
        let state = AppEngineState::new();
        let bad: Arc<dyn ExchangeAdapter> = Arc::new(CountingAccountAdapter {
            calls: AtomicUsize::new(0),
            delay_ms: 0,
            fail: true,
        });
        assert!(live_account_or_cached(&state, "mock", &bad).await.is_err());
    }

    #[tokio::test]
    async fn uncredentialed_account_read_never_touches_the_wire() {
        // Vault locked / nothing provisioned: the poller must serve the
        // last-known snapshot (or Err with none) without invoking
        // fetch_account — otherwise every credentialed lane burns a doomed
        // request per tick and the failures poison the venue's rate-limit
        // backoff budget for public market data too.
        struct NoCreds {
            calls: AtomicUsize,
        }
        #[async_trait::async_trait]
        impl ExchangeAdapter for NoCreds {
            fn venue_id(&self) -> VenueId {
                VenueId::new("mock")
            }
            fn name(&self) -> &str {
                "mock"
            }
            async fn fetch_markets(
                &self,
            ) -> anyhow::Result<Vec<arbitrageur_core::market::MarketMetadata>> {
                unimplemented!()
            }
            async fn fetch_orderbook(&self, _s: &str) -> anyhow::Result<OrderBook> {
                unimplemented!()
            }
            async fn fetch_ticker(
                &self,
                _s: &str,
            ) -> anyhow::Result<arbitrageur_core::quote::Ticker> {
                unimplemented!()
            }
            async fn fetch_funding_rate(
                &self,
                _s: &str,
            ) -> anyhow::Result<arbitrageur_core::quote::FundingRate> {
                unimplemented!()
            }
            async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(AccountSnapshot {
                    venue: VenueId::new("mock"),
                    equity: dec!(777),
                    free_margin: dec!(700),
                    maintenance_margin: dec!(70),
                    margin_usage_pct: dec!(9),
                    balances: vec![],
                    positions: vec![],
                    timestamp: Utc::now(),
                })
            }
            async fn place_order(
                &self,
                _o: arbitrageur_core::order::OrderRequest,
            ) -> anyhow::Result<arbitrageur_core::order::OrderResponse> {
                unimplemented!()
            }
            async fn cancel_order(
                &self,
                _c: arbitrageur_core::order::CancelRequest,
            ) -> anyhow::Result<()> {
                unimplemented!()
            }
        }

        let state = AppEngineState::new();
        let inner = Arc::new(NoCreds {
            calls: AtomicUsize::new(0),
        });
        let no_creds: Arc<dyn ExchangeAdapter> = inner.clone();

        // No snapshot at all: fail-closed Err without a wire call.
        assert!(live_account_or_cached(&state, "mock", &no_creds)
            .await
            .is_err());
        assert_eq!(inner.calls.load(Ordering::SeqCst), 0);

        // Seed the cache with a credentialed read, age it past the fresh
        // window, then swap to the uncredentialed adapter: the stale
        // snapshot must be served instead of a wire call.
        let good: Arc<dyn ExchangeAdapter> = Arc::new(CountingAccountAdapter {
            calls: AtomicUsize::new(0),
            delay_ms: 0,
            fail: false,
        });
        live_account_or_cached(&state, "mock", &good)
            .await
            .expect("seed read");
        {
            let mut map = state.last_good_accounts.lock();
            if let Some(entry) = map.get_mut("testnet-mock") {
                entry.1 -= 10_000;
            }
        }
        let (acc, stale_age) = live_account_or_cached(&state, "mock", &no_creds)
            .await
            .expect("stale snapshot must serve when uncredentialed");
        assert_eq!(acc.equity, dec!(1000));
        assert!(stale_age.unwrap() >= 10_000);
        assert_eq!(inner.calls.load(Ordering::SeqCst), 0);
    }

    fn resp_with(
        filled: Decimal,
        avg_px: Option<Decimal>,
        fee: Decimal,
    ) -> arbitrageur_core::order::OrderResponse {
        arbitrageur_core::order::OrderResponse {
            order_id: "o1".into(),
            client_order_id: "c1".into(),
            venue: arbitrageur_core::types::VenueId::new("mock"),
            symbol: "ETH".into(),
            status: arbitrageur_core::types::OrderStatus::Filled,
            original_qty: filled,
            filled_qty: filled,
            remaining_qty: dec!(0),
            avg_fill_price: avg_px,
            fee: Some(fee),
            fee_currency: "USD".into(),
            raw_response: None,
            timestamp: chrono::Utc::now(),
        }
    }

    #[test]
    fn unwind_round_trip_charges_price_move_and_fees() {
        // Buy at 100.0, unwound by a sell at 99.9 on 0.5 units, 0.02 fees:
        // price loss -0.05, fees -0.02 → -0.07 total.
        let entry = resp_with(dec!(0.5), Some(dec!(100.0)), dec!(0.01));
        let unwind = resp_with(dec!(0.5), Some(dec!(99.9)), dec!(0.01));
        let pnl = unwind_round_trip_pnl(&entry, &unwind, Side::Buy);
        assert_eq!(pnl, dec!(-0.07));
        // Sell at 100.0, unwound by a buy at 100.2: same magnitude.
        let pnl2 = unwind_round_trip_pnl(&entry, &unwind, Side::Sell);
        // (entry - unwind) * qty = (100 - 99.9) * 0.5 = +0.05 - 0.02 fees
        assert_eq!(pnl2, dec!(0.03));
        // Missing prices degrade to fee-only — never invent a move.
        let blind = resp_with(dec!(0.5), None, dec!(0.01));
        assert_eq!(
            unwind_round_trip_pnl(&entry, &blind, Side::Buy),
            dec!(-0.02)
        );
    }

    /// Mock adapter whose venue wire grid floors quantities to `step`,
    /// mirroring the real adapters' `wire_base_qty` contract (grid floor,
    /// bail below the representable minimum).
    struct WireGridAdapter {
        venue: &'static str,
        step: Decimal,
    }

    #[async_trait::async_trait]
    impl ExchangeAdapter for WireGridAdapter {
        fn venue_id(&self) -> VenueId {
            VenueId::new(self.venue)
        }
        fn name(&self) -> &str {
            self.venue
        }
        async fn fetch_markets(
            &self,
        ) -> anyhow::Result<Vec<arbitrageur_core::market::MarketMetadata>> {
            unimplemented!()
        }
        async fn fetch_orderbook(&self, _s: &str) -> anyhow::Result<OrderBook> {
            unimplemented!()
        }
        async fn fetch_ticker(&self, _s: &str) -> anyhow::Result<arbitrageur_core::quote::Ticker> {
            unimplemented!()
        }
        async fn fetch_funding_rate(
            &self,
            _s: &str,
        ) -> anyhow::Result<arbitrageur_core::quote::FundingRate> {
            unimplemented!()
        }
        async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
            unimplemented!()
        }
        async fn place_order(
            &self,
            _o: arbitrageur_core::order::OrderRequest,
        ) -> anyhow::Result<arbitrageur_core::order::OrderResponse> {
            unimplemented!()
        }
        async fn cancel_order(
            &self,
            _r: arbitrageur_core::order::CancelRequest,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        async fn wire_base_qty(&self, _s: &str, qty: Decimal) -> anyhow::Result<Decimal> {
            let wire = (qty / self.step).floor() * self.step;
            if wire < self.step {
                anyhow::bail!("below one grid unit");
            }
            Ok(wire)
        }
    }

    fn grid_adapter(venue: &'static str, step: Decimal) -> Arc<dyn ExchangeAdapter> {
        Arc::new(WireGridAdapter { venue, step })
    }

    #[tokio::test]
    async fn common_wire_qty_snaps_to_the_finer_grid() {
        // Lighter-style 4dp grid vs Binance-style 3dp grid on a 0.0059 ask:
        // un-snapped legs would fill 0.0059 vs 0.005 — a naked 0.0009
        // residual. The snap trades 0.005 on both.
        let fine = grid_adapter("lighter", dec!(0.0001));
        let coarse = grid_adapter("binance", dec!(0.001));
        let snapped = common_wire_qty("ETH-USDC", dec!(0.0059), &fine, &coarse)
            .await
            .expect("aligned grids snap");
        assert_eq!(snapped, dec!(0.005));
        // Identical grids snap identically.
        let snapped = common_wire_qty("ETH-USDC", dec!(0.0059), &coarse, &coarse)
            .await
            .expect("same grid");
        assert_eq!(snapped, dec!(0.005));
    }

    #[tokio::test]
    async fn common_wire_qty_refuses_unsizable_and_irreconcilable() {
        let coarse = grid_adapter("okx", dec!(0.01));
        let fine = grid_adapter("lighter", dec!(0.0001));
        // Below the coarse grid's minimum: the venue itself cannot size it.
        let err = common_wire_qty("ETH-USDC", dec!(0.005), &coarse, &fine)
            .await
            .expect_err("sub-grid quantity must refuse");
        assert!(err.contains("wire grid"), "{err}");
        // A quantity finer than the coarse grid snaps to the coarse floor —
        // not an irreconcilable error: 0.0159 -> 0.01 fills both.
        let snapped = common_wire_qty("ETH-USDC", dec!(0.0159), &coarse, &fine)
            .await
            .expect("coarse floor is representable on both");
        assert_eq!(snapped, dec!(0.01));
    }

    /// Venue fixture for the liq-trim CloseBoth path: one seeded account
    /// position, market orders that fill at mark.
    struct TrimVenue {
        venue: &'static str,
        position: arbitrageur_core::position::Position,
        placed: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ExchangeAdapter for TrimVenue {
        fn venue_id(&self) -> VenueId {
            VenueId::new(self.venue)
        }
        fn name(&self) -> &str {
            self.venue
        }
        async fn fetch_markets(
            &self,
        ) -> anyhow::Result<Vec<arbitrageur_core::market::MarketMetadata>> {
            unimplemented!()
        }
        async fn fetch_orderbook(&self, _s: &str) -> anyhow::Result<OrderBook> {
            unimplemented!()
        }
        async fn fetch_ticker(&self, _s: &str) -> anyhow::Result<arbitrageur_core::quote::Ticker> {
            unimplemented!()
        }
        async fn fetch_funding_rate(
            &self,
            _s: &str,
        ) -> anyhow::Result<arbitrageur_core::quote::FundingRate> {
            unimplemented!()
        }
        async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
            Ok(AccountSnapshot {
                venue: VenueId::new(self.venue),
                equity: dec!(1000),
                free_margin: dec!(900),
                maintenance_margin: dec!(100),
                margin_usage_pct: dec!(10),
                balances: vec![],
                positions: vec![self.position.clone()],
                timestamp: Utc::now(),
            })
        }
        async fn place_order(
            &self,
            o: arbitrageur_core::order::OrderRequest,
        ) -> anyhow::Result<arbitrageur_core::order::OrderResponse> {
            let n = self.placed.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(arbitrageur_core::order::OrderResponse {
                order_id: format!("{}-oid-{n}", self.venue),
                client_order_id: o.client_order_id,
                venue: VenueId::new(self.venue),
                symbol: o.symbol,
                status: arbitrageur_core::types::OrderStatus::Filled,
                original_qty: o.quantity,
                filled_qty: o.quantity,
                remaining_qty: dec!(0),
                avg_fill_price: Some(dec!(100)),
                fee: Some(dec!(0.01)),
                fee_currency: "USDC".into(),
                raw_response: None,
                timestamp: Utc::now(),
            })
        }
        async fn cancel_order(
            &self,
            _r: arbitrageur_core::order::CancelRequest,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn live_order_submission_implemented(&self) -> bool {
            true
        }
        fn has_live_credentials(&self) -> bool {
            true
        }
    }

    fn trim_position(
        venue: &'static str,
        side: Side,
        liq: Decimal,
    ) -> arbitrageur_core::position::Position {
        arbitrageur_core::position::Position {
            venue: VenueId::new(venue),
            symbol: "ETH-USDC".into(),
            side,
            quantity: dec!(0.5),
            entry_price: dec!(100),
            mark_price: dec!(100),
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            // ~2% from liquidation — inside the 5% trim threshold.
            liquidation_price: Some(liq),
            leverage: dec!(10),
            initial_margin: dec!(50),
            maintenance_margin: dec!(25),
            updated_at: Utc::now(),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn closeboth_liq_trim_does_not_self_deadlock_execution_lock() {
        // Regression: the CloseBoth arm of `maybe_liq_trim_pair` must NOT
        // take `execution_lock` itself — `flatten_ticker_positions_impl`
        // acquires the same non-reentrant Tokio mutex internally. Re-adding
        // an outer take self-deadlocks and wedges every execution path.
        // A timeout turns that hang into a loud failure.
        let state = AppEngineState::new();
        state.settings.write().passive_unwind.enabled = false;
        // unit_size >= position qty → trimming would strand sub-step dust
        // → evaluate_liq_trim chooses CloseBoth.
        state.strategy_configs.write().insert(
            "ETH-USDC".to_string(),
            crate::state::TickerStrategyConfigDto {
                symbol: "ETH-USDC".into(),
                unit_size: 0.5,
                ..Default::default()
            },
        );
        let lighter = Arc::new(TrimVenue {
            venue: "lighter",
            position: trim_position("lighter", Side::Buy, dec!(98)),
            placed: AtomicUsize::new(0),
        });
        let binance = Arc::new(TrimVenue {
            venue: "binance",
            position: trim_position("binance", Side::Sell, dec!(102)),
            placed: AtomicUsize::new(0),
        });
        {
            let mut m = state.test_adapters.write().unwrap();
            m.insert("lighter".into(), lighter.clone());
            m.insert("binance".into(), binance.clone());
        }
        // resolve_pos reads only the fresh venue-truth cache.
        let now_ms = Utc::now().timestamp_millis();
        for (venue, adapter) in [("lighter", &lighter), ("binance", &binance)] {
            let acc = adapter.fetch_account().await.unwrap();
            state
                .last_good_accounts
                .lock()
                .insert(format!("testnet-{venue}"), (acc, now_ms));
        }
        // A balanced futures pair: long on lighter, short on binance.
        state
            .state_store
            .record_signal_execution(
                &crate::state_store::NewSignalExecution {
                    id: "sig-closeboth".into(),
                    strategy_id: "futures".into(),
                    symbol: "ETH-USDC".into(),
                    route_id: None,
                    expected_net_usd: None,
                    note: None,
                    legs: vec![
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "lighter".into(),
                            side: crate::state_store::LegSide::Long,
                            symbol: Some("ETH-USDC".into()),
                            intended_qty: 0.5,
                            intended_price: Some(100.0),
                            unit_scale: None,
                        },
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "binance".into(),
                            side: crate::state_store::LegSide::Short,
                            symbol: Some("ETH-USDC".into()),
                            intended_qty: 0.5,
                            intended_price: Some(100.0),
                            unit_scale: None,
                        },
                    ],
                },
                "testnet",
            )
            .unwrap();
        for side in [
            crate::state_store::LegSide::Long,
            crate::state_store::LegSide::Short,
        ] {
            state
                .state_store
                .apply_leg_fill("sig-closeboth", side, 0.5, 100.0, 0.0, Some("entry-oid"))
                .unwrap();
        }
        let sig = state
            .state_store
            .list_signals(Some("ETH-USDC"), None)
            .unwrap()
            .into_iter()
            .find(|s| s.id == "sig-closeboth")
            .unwrap();
        let buy = sig
            .legs
            .iter()
            .find(|l| l.side == crate::state_store::LegSide::Long)
            .unwrap();
        let sell = sig
            .legs
            .iter()
            .find(|l| l.side == crate::state_store::LegSide::Short)
            .unwrap();

        let out = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            maybe_liq_trim_pair(&state, &sig, buy, sell, 5.0),
        )
        .await
        .expect("CloseBoth arm self-deadlocked on execution_lock")
        .expect("liq-trim decision expected");
        assert_eq!((out.0, out.1), ("exit", "liq_trim"), "{}", out.2);
        assert!(out.3, "CloseBoth is an exit decision");
        assert!(
            out.2.contains("closed pair (1 signals)"),
            "flatten should have closed the pair: {}",
            out.2
        );
        assert_eq!(lighter.placed.load(Ordering::SeqCst), 1);
        assert_eq!(binance.placed.load(Ordering::SeqCst), 1);
    }
}

#[cfg(test)]
mod traded_opportunity_tests {
    use super::*;
    use crate::state_store::{LegSide, SignalExecution, SignalLeg, SignalStatus};
    use arbitrageur_execution::virtual_ledger::{ArbitrageCycleLot, CycleStatus};
    use chrono::Utc;
    use std::collections::HashMap;

    #[allow(clippy::too_many_arguments)]
    fn leg(
        id: &str,
        signal: &str,
        venue: &str,
        side: LegSide,
        intended: f64,
        filled: f64,
        price: Option<f64>,
        fee: f64,
        ts: &str,
    ) -> SignalLeg {
        SignalLeg {
            id: id.into(),
            signal_id: signal.into(),
            account_id: Some(format!("acct-{venue}")),
            venue: venue.into(),
            side,
            symbol: "BTC-USDC".into(),
            intended_qty: intended,
            filled_qty: filled,
            fill_price: price,
            fee_usd: fee,
            venue_order_id: Some(format!("oid-{id}")),
            client_order_id: Some(format!("cid-{id}")),
            intended_price: Some(99.0),
            unit_scale: 1.0,
            created_at: ts.into(),
            updated_at: ts.into(),
        }
    }

    fn sig(
        id: &str,
        env: &str,
        status: SignalStatus,
        legs: Vec<SignalLeg>,
        ts: &str,
    ) -> SignalExecution {
        SignalExecution {
            id: id.into(),
            strategy_id: "futures".into(),
            symbol: "BTC-USDC".into(),
            status,
            environment: env.into(),
            route_id: None,
            account_long_id: None,
            account_short_id: None,
            expected_net_usd: Some(4.0),
            fees_usd: 0.0,
            note: None,
            legs,
            created_at: ts.into(),
            updated_at: ts.into(),
            closed_at: None,
        }
    }

    #[test]
    fn groups_root_children_and_flows_and_env() {
        let t = "2024-01-01T00:00:00Z";
        // Entry: buy 1 @100 (long hyperliquid), sell 1 @101 (short lighter).
        let root = sig(
            "sig-1-aa",
            "testnet",
            SignalStatus::Closed,
            vec![
                leg(
                    "l1",
                    "sig-1-aa",
                    "hyperliquid",
                    LegSide::Long,
                    1.0,
                    1.0,
                    Some(100.0),
                    0.5,
                    t,
                ),
                leg(
                    "l2",
                    "sig-1-aa",
                    "lighter",
                    LegSide::Short,
                    1.0,
                    1.0,
                    Some(101.0),
                    0.5,
                    t,
                ),
            ],
            t,
        );
        // Flatten child (order-direction storage): the hyperliquid long
        // closes by selling → stored Short @102; the lighter short closes
        // by buying back → stored Long @100.5.
        let flat = sig(
            "flat-sig-1-aa-0001",
            "testnet",
            SignalStatus::Closed,
            vec![
                leg(
                    "l3",
                    "flat-sig-1-aa-0001",
                    "hyperliquid",
                    LegSide::Short,
                    1.0,
                    1.0,
                    Some(102.0),
                    0.4,
                    "2024-01-01T01:00:00Z",
                ),
                leg(
                    "l4",
                    "flat-sig-1-aa-0001",
                    "lighter",
                    LegSide::Long,
                    1.0,
                    1.0,
                    Some(100.5),
                    0.4,
                    "2024-01-01T01:00:00Z",
                ),
            ],
            "2024-01-01T01:00:00Z",
        );
        // A live-env sibling root that must not leak into the testnet view.
        let live = sig(
            "sig-9-live",
            "live",
            SignalStatus::Closed,
            vec![leg(
                "lx",
                "sig-9-live",
                "hyperliquid",
                LegSide::Long,
                1.0,
                1.0,
                Some(100.0),
                0.1,
                t,
            )],
            t,
        );
        // Unknown-env row stays visible to every environment (fail-closed).
        let unknown = sig(
            "sig-7-legacy",
            "unknown",
            SignalStatus::Closed,
            vec![leg(
                "lu",
                "sig-7-legacy",
                "lighter",
                LegSide::Short,
                1.0,
                1.0,
                Some(50.0),
                0.0,
                t,
            )],
            "2024-01-02T00:00:00Z",
        );
        let signals = vec![root, flat, live, unknown];
        let lots: HashMap<String, ArbitrageCycleLot> = HashMap::new();

        let testnet_rows = build_traded_opportunities("testnet", &signals, &lots);
        // Roots only: sig-1-aa + sig-7-legacy (live filtered, flat- not a root).
        assert_eq!(testnet_rows.len(), 2);
        let opp = &testnet_rows[1]; // older created_at sorts last
        assert_eq!(opp.signal_id, "sig-1-aa");
        assert_eq!(opp.child_count, 1);
        assert_eq!(opp.transactions.len(), 4);
        // Roles + derived order sides.
        assert_eq!(opp.transactions[0].role, "entry");
        assert_eq!(opp.transactions[0].order_side, "buy");
        assert_eq!(opp.transactions[1].order_side, "sell");
        assert_eq!(opp.transactions[2].role, "flatten");
        assert_eq!(opp.transactions[2].order_side, "sell"); // stored Short = sell to close the long
        assert_eq!(opp.transactions[3].order_side, "buy"); // stored Long = buy to close the short
                                                           // Cash flow: -(100) + 101 + 102 - 100.5 = +2.5, minus fees 1.8 = +0.7.
        assert!(
            (opp.leg_net_flow_usd - 0.7).abs() < 1e-9,
            "{}",
            opp.leg_net_flow_usd
        );
        assert!((opp.total_fees_usd - 1.8).abs() < 1e-9);
        // Terminal + no lot → realized falls back to the leg cash-flow.
        assert!((opp.realized_pnl_usd.unwrap() - 0.7).abs() < 1e-9);
        // Unknown-env row is present and reported verbatim.
        assert_eq!(testnet_rows[0].signal_id, "sig-7-legacy");
        assert_eq!(testnet_rows[0].environment, "unknown");

        // Live view must exclude testnet rows entirely.
        let live_rows = build_traded_opportunities("live", &signals, &lots);
        let ids: Vec<&str> = live_rows.iter().map(|r| r.signal_id.as_str()).collect();
        assert_eq!(ids, vec!["sig-7-legacy", "sig-9-live"]);
    }

    #[test]
    fn lot_join_supplies_realized_and_open_means_none() {
        let t = "2024-01-01T00:00:00Z";
        let root = sig(
            "sig-2-bb",
            "live",
            SignalStatus::Balanced,
            vec![leg(
                "l1",
                "sig-2-bb",
                "hyperliquid",
                LegSide::Long,
                1.0,
                1.0,
                Some(100.0),
                0.5,
                t,
            )],
            t,
        );
        let mut lots: HashMap<String, ArbitrageCycleLot> = HashMap::new();
        lots.insert(
            "spatial-sig-2-bb".to_string(),
            ArbitrageCycleLot {
                id: "spatial-sig-2-bb".into(),
                strategy_id: "spatial_spread".into(),
                symbol: "BTC-USDC".into(),
                environment: "live".into(),
                venue_long: "hyperliquid".into(),
                venue_short: "lighter".into(),
                entry_time: Utc::now(),
                entry_spread_bps: 12.0,
                long_qty: 1.0,
                long_entry_price: 100.0,
                short_qty: 1.0,
                short_entry_price: 101.0,
                fees_paid_usd: 1.0,
                status: CycleStatus::Closed,
                close_time: Some(Utc::now()),
                close_spread_bps: Some(2.0),
                realized_pnl_usd: Some(4.25),
                realized_pnl_bps: Some(3.9),
            },
        );
        let rows = build_traded_opportunities("live", &[root], &lots);
        assert_eq!(rows.len(), 1);
        let opp = &rows[0];
        assert_eq!(opp.lot_id.as_deref(), Some("spatial-sig-2-bb"));
        assert_eq!(opp.lot_status, Some("closed"));
        assert_eq!(opp.entry_spread_bps, Some(12.0));
        assert_eq!(opp.close_spread_bps, Some(2.0));
        // Lot is authoritative even though the signal row is still "balanced".
        assert_eq!(opp.realized_pnl_usd, Some(4.25));
        assert_eq!(opp.realized_pnl_bps, Some(3.9));
        assert_eq!(opp.venue_long.as_deref(), Some("hyperliquid"));
        assert_eq!(opp.venue_short.as_deref(), Some("lighter"));

        // Same signal without a lot and still open → realized unknown.
        let open = sig(
            "sig-3-cc",
            "live",
            SignalStatus::Balanced,
            vec![leg(
                "l9",
                "sig-3-cc",
                "hyperliquid",
                LegSide::Long,
                1.0,
                0.5,
                Some(100.0),
                0.5,
                t,
            )],
            t,
        );
        let rows = build_traded_opportunities("live", &[open], &HashMap::new());
        assert_eq!(rows[0].realized_pnl_usd, None);
        assert_eq!(rows[0].transactions[0].fill_state, "partial");
        assert_eq!(rows[0].lot_id, None);
        // Venue falls back to the leg when no lot exists.
        assert_eq!(rows[0].venue_long.as_deref(), Some("hyperliquid"));
    }

    #[test]
    fn unwind_persist_records_entry_fill_and_flat_child() {
        use arbitrageur_core::order::OrderResponse;
        use arbitrageur_core::types::{OrderStatus, VenueId};
        use rust_decimal_macros::dec;

        let state = AppEngineState::new();
        state
            .state_store
            .record_signal_execution(
                &crate::state_store::NewSignalExecution {
                    id: "sig-unw-1".to_string(),
                    strategy_id: "futures".into(),
                    symbol: "BTC-USDC".into(),
                    route_id: None,
                    expected_net_usd: Some(1.0),
                    note: None,
                    legs: vec![
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "hyperliquid".into(),
                            side: LegSide::Long,
                            symbol: None,
                            intended_qty: 2.0,
                            intended_price: None,
                            unit_scale: None,
                        },
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "lighter".into(),
                            side: LegSide::Short,
                            symbol: None,
                            intended_qty: 2.0,
                            intended_price: None,
                            unit_scale: None,
                        },
                    ],
                },
                "testnet",
            )
            .unwrap();

        let entry = OrderResponse {
            order_id: "entry-oid".into(),
            client_order_id: "entry-cid".into(),
            venue: VenueId("hyperliquid".into()),
            symbol: "BTC-USDC".into(),
            status: OrderStatus::Filled,
            original_qty: dec!(2),
            filled_qty: dec!(2),
            remaining_qty: dec!(0),
            avg_fill_price: Some(dec!(100)),
            fee: Some(dec!(0.2)),
            fee_currency: "USD".into(),
            raw_response: None,
            timestamp: Utc::now(),
        };
        let unwind = OrderResponse {
            order_id: "unw-oid".into(),
            client_order_id: "unw-cid".into(),
            venue: VenueId("hyperliquid".into()),
            symbol: "BTC-USDC".into(),
            status: OrderStatus::Filled,
            original_qty: dec!(2),
            filled_qty: dec!(2),
            remaining_qty: dec!(0),
            avg_fill_price: Some(dec!(99.5)),
            fee: Some(dec!(0.1)),
            fee_currency: "USD".into(),
            raw_response: None,
            timestamp: Utc::now(),
        };

        persist_unwind_leg_records(&state, "sig-unw-1", LegSide::Long, &entry, &unwind);

        // The entry leg now carries its real fill + order identity.
        let root = state.state_store.get_signal("sig-unw-1").unwrap().unwrap();
        let long = root.legs.iter().find(|l| l.side == LegSide::Long).unwrap();
        assert_eq!(long.filled_qty, 2.0);
        assert_eq!(long.fill_price, Some(100.0));
        assert_eq!(long.venue_order_id.as_deref(), Some("entry-oid"));

        // The unwind is a closed `flat-` child with the position side and
        // its own order identity + fill.
        let kids = state
            .state_store
            .list_signals(None, None)
            .unwrap()
            .into_iter()
            .filter(|s| s.id.starts_with("flat-sig-unw-1-"))
            .collect::<Vec<_>>();
        assert_eq!(kids.len(), 1);
        let child = &kids[0];
        assert_eq!(child.status, SignalStatus::Closed);
        assert_eq!(child.legs.len(), 1);
        let unwind_leg = &child.legs[0];
        // Order-direction storage: the unwind SELLS the long → Short.
        assert_eq!(unwind_leg.side, LegSide::Short);
        assert_eq!(unwind_leg.filled_qty, 2.0);
        assert_eq!(unwind_leg.fill_price, Some(99.5));
        assert_eq!(unwind_leg.venue_order_id.as_deref(), Some("unw-oid"));

        // The grouped opportunity reports the full atomic trail: entry leg
        // filled, close unwind ordered sell-side cash flow −0.7 net.
        let rows = build_traded_opportunities(
            "testnet",
            &state.state_store.list_signals(None, None).unwrap(),
            &state.virtual_ledger.read().lots,
        );
        let opp = rows.iter().find(|r| r.signal_id == "sig-unw-1").unwrap();
        assert_eq!(opp.child_count, 1);
        assert_eq!(opp.transactions.len(), 3);
        // Buy 2@100 + entry-fee .2; unwind sells 2@99.5 fee .1 → -1.3 total.
        assert!((opp.leg_net_flow_usd - (-1.3)).abs() < 1e-9);
    }

    #[test]
    fn backfill_repairs_legacy_unwound_note() {
        let state = AppEngineState::new();
        // The pre-fix record shape: both legs no_fill, real venue orders
        // only as Debug text in the note (matches live LegBFailedUnwound).
        let note = concat!(
            "LegBFailedUnwound { leg_a_fill: OrderResponse { order_id: \"oid-a\", ",
            "client_order_id: \"cid-a\", venue: VenueId(\"arcus\"), symbol: \"QQQ\", ",
            "status: Filled, original_qty: 2, filled_qty: 2, remaining_qty: 0, ",
            "avg_fill_price: Some(753.5), fee: None, fee_currency: \"USDG\", ",
            "raw_response: Some(\"{\\\"orderId\\\":\\\"oid-a\\\",\\\"status\\\":\\\"FILLED\\\"}\"), ",
            "timestamp: 2026-09-25T19:59:28.293765639Z }, ",
            "unwind_resp: OrderResponse { order_id: \"oid-u\", ",
            "client_order_id: \"cid-u\", venue: VenueId(\"arcus\"), symbol: \"QQQ\", ",
            "status: Filled, original_qty: 2, filled_qty: 2, remaining_qty: 0, ",
            "avg_fill_price: Some(752.43), fee: None, fee_currency: \"USDG\", ",
            "raw_response: Some(\"{\\\"orderId\\\":\\\"oid-u\\\"}\"), ",
            "timestamp: 2026-09-25T19:59:29.137230913Z }, ",
            "error_b: \"Alpaca HTTP 422\" }"
        );
        state
            .state_store
            .record_signal_execution(
                &crate::state_store::NewSignalExecution {
                    id: "sig-legacy-1".to_string(),
                    strategy_id: "futures".into(),
                    symbol: "QQQ".into(),
                    route_id: None,
                    expected_net_usd: Some(1.0),
                    note: Some(note.to_string()),
                    legs: vec![
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "arcus".into(),
                            side: LegSide::Long,
                            symbol: None,
                            intended_qty: 2.0,
                            intended_price: None,
                            unit_scale: None,
                        },
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "alpaca".into(),
                            side: LegSide::Short,
                            symbol: None,
                            intended_qty: 1.94,
                            intended_price: None,
                            unit_scale: None,
                        },
                    ],
                },
                "testnet",
            )
            .unwrap();
        let _ = state.state_store.update_signal_status(
            "sig-legacy-1",
            crate::state_store::SignalStatus::Failed,
            Some(note),
        );

        backfill_unwound_atomic_trails(&state);

        // Entry leg carries the parsed fill + order identity.
        let root = state
            .state_store
            .get_signal("sig-legacy-1")
            .unwrap()
            .unwrap();
        let long = root.legs.iter().find(|l| l.side == LegSide::Long).unwrap();
        assert_eq!(long.filled_qty, 2.0);
        assert_eq!(long.fill_price, Some(753.5));
        assert_eq!(long.venue_order_id.as_deref(), Some("oid-a"));
        assert_eq!(long.client_order_id.as_deref(), Some("cid-a"));

        // The unwind child exists with its own fill.
        let kids = state
            .state_store
            .list_signals(None, None)
            .unwrap()
            .into_iter()
            .filter(|s| s.id.starts_with("flat-sig-legacy-1-"))
            .collect::<Vec<_>>();
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].legs[0].fill_price, Some(752.43));

        // Second run is a no-op — no duplicate child.
        backfill_unwound_atomic_trails(&state);
        let kids = state
            .state_store
            .list_signals(None, None)
            .unwrap()
            .into_iter()
            .filter(|s| s.id.starts_with("flat-sig-legacy-1-"))
            .count();
        assert_eq!(kids, 1);

        // Cash flow is honest: bought 2@753.5, unwound 2@752.43 → -2.14.
        let rows = build_traded_opportunities(
            "testnet",
            &state.state_store.list_signals(None, None).unwrap(),
            &state.virtual_ledger.read().lots,
        );
        let opp = rows.iter().find(|r| r.signal_id == "sig-legacy-1").unwrap();
        assert!((opp.leg_net_flow_usd - (-2.14)).abs() < 1e-9);
        assert!((opp.realized_pnl_usd.unwrap() - (-2.14)).abs() < 1e-9);
    }

    #[test]
    fn one_sided_family_has_no_attributable_pnl() {
        // An `extclose-` buy-back spends cash to close a short whose entry
        // never passed through the ledger — gross outflow is not P&L.
        let t = "2024-01-02T00:00:00Z";
        let ext = sig(
            "extclose-arcus-1",
            "testnet",
            SignalStatus::Closed,
            vec![leg(
                "lx",
                "extclose-arcus-1",
                "arcus",
                LegSide::Long,
                0.001,
                0.001,
                Some(84530.8),
                0.0,
                t,
            )],
            t,
        );
        let rows = build_traded_opportunities("testnet", &[ext], &HashMap::new());
        assert_eq!(rows.len(), 1);
        assert!((rows[0].leg_net_flow_usd - (-84.5308)).abs() < 1e-9);
        assert_eq!(rows[0].realized_pnl_usd, None);

        // A no-fill family provably realized $0.00 — nothing traded.
        let dead = sig(
            "sig-dead",
            "testnet",
            SignalStatus::Failed,
            vec![
                leg(
                    "la",
                    "sig-dead",
                    "arcus",
                    LegSide::Long,
                    1.0,
                    0.0,
                    None,
                    0.0,
                    t,
                ),
                leg(
                    "lb",
                    "sig-dead",
                    "alpaca",
                    LegSide::Short,
                    1.0,
                    0.0,
                    None,
                    0.0,
                    t,
                ),
            ],
            t,
        );
        let rows = build_traded_opportunities("testnet", &[dead], &HashMap::new());
        assert_eq!(rows[0].realized_pnl_usd, Some(0.0));
    }
}
