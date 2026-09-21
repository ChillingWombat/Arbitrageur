//! Live execution core for Single Spread (spatial) plus the adjacent
//! execution domains: cross-ticker/prediction dispatch, candidate
//! arbitration, circuit-breaker controls, killswitch, symbol flattening,
//! passive unwind, and liquidation-trim/convergence exits.
//!
//! Split out of `commands.rs`. Shared helpers that also serve the scanner
//! and funding domains are reached via `crate::commands`; sizing helpers
//! used by flatten paths come from `crate::triangle`.

use crate::commands::{
    compute_route_economics, effective_venue_fee_bps, fetch_multi_market_quotes_impl,
    live_entry_gate, live_entry_notional_guard, maybe_record_margin_snapshots,
    record_execution_failure, record_execution_log, refresh_equity_expectations_after_fills,
    reject_if_strategy_filtered, resolve_effective_leverage, snapshot_paper_equity,
    strategy_slippage_bps, validate_adapter_book, validate_market_symbol, ConvergenceCheckDto,
    MarginHealthDto, NetDeltaDto, RiskOverviewDto, SimStepResultDto, TickerMarketQuoteDto,
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
    execute_passive_reduce, PassiveBookRefresh, PassiveOutcome, PassiveUnwindConfig,
};
use arbitrageur_execution::smart_routing::SmartOrderRouter;
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
    if let Ok(routes) = state
        .state_store
        .eligible_strategy_routes(strategy_id, venue_a, venue_b)
    {
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
            .default_account(venue_a)
            .ok()
            .flatten()
            .map(|a| a.id),
        short: state
            .state_store
            .default_account(venue_b)
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
            .to_string()
            .parse::<f64>()
            .map_err(|_| "unparseable fee".to_string())?;
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
    let delta = state
        .state_store
        .signal_delta(signal_id)
        .map_err(|e| e.to_string())?;
    let eps = 1e-6;
    Ok(if delta.is_finite() && delta.abs() <= eps {
        SignalStatus::Balanced
    } else {
        SignalStatus::Imbalanced
    })
}

#[allow(clippy::too_many_arguments)]
/// Test-facing wrapper over `execute_spatial_arbitrage_inner` — production
/// callers (command, candidates batch, radar) reach `inner` directly so the
/// canary-loss parameter stays off every path except the explicit canary
/// command.
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
    canary_max_loss_bps: Option<f64>,
    prediction: Option<EntryPrediction>,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    if !order_qty.is_finite() || order_qty <= 0.0 {
        return Err("order_qty must be a finite positive number".into());
    }
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

    // Authoritative grid-cap gate. The frontend derives max_units per ticker
    // (step = min(mult x venue min-lot, maxStepUsd / price); max steps from
    // total paper equity x pct) and threads it through position_step/max_units.
    // The backend holds no venue min-lot table so it cannot re-derive the cap;
    // it enforces the caller-supplied cap strictly instead: an entry whose step
    // exceeds the cap, or a ticker already at the cap, is rejected here before
    // any orderbook work. Callers that pass max_units=None (monitor path) skip
    // this gate.
    if let Some(max) = max_units {
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
        let current = state
            .strategy_configs
            .read()
            .get(symbol)
            .map(|c| c.current_accumulated_units)
            .unwrap_or(0);
        if current >= max {
            return Err(format!(
                "Grid cap reached for {symbol}: {current}/{max} accumulated units — entry rejected"
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
    let is_live = *state.is_live_mode.read();
    if is_live {
        live_entry_gate(state, &[buy_venue, sell_venue])?;
    }
    let _ = trigger_spread_bps; // caller-supplied spread is never trusted for eligibility

    // Durable per-ticker gate: an in-flight, imbalanced, or unresolved signal
    // for this symbol blocks any further order until its own legs reconcile
    // as delta-neutral. Survives restarts via the SQLite store.
    if let Some(blocker) = state
        .state_store
        .blocking_signals(symbol)
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

    let buy_paper = state
        .get_paper_exchange(buy_vid.as_str())
        .ok_or_else(|| format!("No paper exchange admitted for venue '{}'", buy_venue))?;
    let sell_paper = state
        .get_paper_exchange(sell_vid.as_str())
        .ok_or_else(|| format!("No paper exchange admitted for venue '{}'", sell_venue))?;

    // Order books must come from the hub cache (max age 2s) or a fresh fetch
    // through the admitted read-only adapter; ticker data is not a substitute.
    // Cached or fetched, a book must carry the exact requested venue and symbol
    // and pass execution validation before it is trusted or cached.
    let book_buy = match state.market_data_hub.get_valid_orderbook(
        &buy_vid,
        symbol,
        Some(std::time::Duration::from_secs(2)),
    ) {
        Some(b) => b,
        None => {
            let b = buy_adapter
                .fetch_orderbook(symbol)
                .await
                .map_err(|e| format!("No fresh {} orderbook for {}: {}", buy_venue, symbol, e))?;
            validate_adapter_book(&b, &buy_vid, symbol)
                .map_err(|e| format!("Buy-side order book rejected: {}", e))?;
            state.market_data_hub.update_orderbook(b.clone());
            b
        }
    };
    let book_sell = match state.market_data_hub.get_valid_orderbook(
        &sell_vid,
        symbol,
        Some(std::time::Duration::from_secs(2)),
    ) {
        Some(b) => b,
        None => {
            let b = sell_adapter
                .fetch_orderbook(symbol)
                .await
                .map_err(|e| format!("No fresh {} orderbook for {}: {}", sell_venue, symbol, e))?;
            validate_adapter_book(&b, &sell_vid, symbol)
                .map_err(|e| format!("Sell-side order book rejected: {}", e))?;
            state.market_data_hub.update_orderbook(b.clone());
            b
        }
    };
    validate_adapter_book(&book_buy, &buy_vid, symbol)
        .map_err(|e| format!("Buy-side order book rejected: {}", e))?;
    validate_adapter_book(&book_sell, &sell_vid, symbol)
        .map_err(|e| format!("Sell-side order book rejected: {}", e))?;

    let qty_dec = Decimal::from_str_exact(&order_qty.to_string())
        .ok()
        .filter(|q| *q > dec!(0))
        .ok_or_else(|| "Invalid order quantity".to_string())?;
    let max_slippage_bps = strategy_slippage_bps(state, "futures");
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

    // Live canary bound: refuse (never clamp) when either leg's notional
    // exceeds the operator cap. Runs after VWAP sizing so it measures the
    // real executable notional, not the caller's intent.
    if is_live {
        live_entry_notional_guard(
            state,
            &[
                (buy_venue, qty_dec * buy_vwap),
                (sell_venue, qty_dec * sell_vwap),
            ],
        )?;
    }

    let target_profit_bps = state
        .strategy_params
        .read()
        .get("futures")
        .map(|p| p.target_profit_bps)
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(1.0);
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
    // Canary override: an explicit `canary_max_loss_bps` (<=50 bps, live
    // entries only) lowers the profit floor to accept the spread+fee cost of
    // a mechanics test. Bounded so a canary can never burn real money on a
    // genuinely bad route. All other checks (margin, depth, delta-neutrality,
    // blocking, cap) still apply.
    let profit_floor_bps = match (is_live, canary_max_loss_bps) {
        (true, Some(loss)) => {
            if !loss.is_finite() || !(0.0..=50.0).contains(&loss) {
                return Err("canary_max_loss_bps must be finite and within 0-50 bps".into());
            }
            -loss
        }
        _ => target_profit_bps,
    };
    if econ.expected_net_bps < profit_floor_bps {
        return Err(format!(
            "Expected net spread {} BPS after route friction is below the {} BPS {}",
            econ.expected_net_bps,
            profit_floor_bps.abs(),
            if profit_floor_bps < 0.0 {
                "canary loss limit"
            } else {
                "target"
            }
        ));
    }
    let net_bps_f = econ.expected_net_bps;
    let expected_net_usd = econ.estimated_profit_usd;

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
    let acc_buy = if is_live {
        buy_adapter
            .fetch_account()
            .await
            .map_err(|e| format!("Failed to fetch live account on {}: {}", buy_venue, e))?
    } else {
        buy_paper.fetch_account().await.map_err(|e| e.to_string())?
    };
    let acc_sell = if is_live {
        sell_adapter
            .fetch_account()
            .await
            .map_err(|e| format!("Failed to fetch live account on {}: {}", sell_venue, e))?
    } else {
        sell_paper
            .fetch_account()
            .await
            .map_err(|e| e.to_string())?
    };
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
    let (target_leverage, account_cap) = resolve_effective_leverage(
        state,
        "futures",
        strategy_id,
        symbol,
        buy_venue,
        sell_venue,
        route_id,
    )?;
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
    if acc_buy.get_position(symbol).is_some() || acc_sell.get_position(symbol).is_some() {
        return Err(format!(
            "An open position already exists for {}; closing lifecycle is not implemented",
            symbol
        ));
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

    if !is_live {
        buy_paper
            .replace_orderbook(book_buy.clone())
            .map_err(|e| e.to_string())?;
        sell_paper
            .replace_orderbook(book_sell.clone())
            .map_err(|e| e.to_string())?;
        // Harden the simulator's own margin paths with the same account cap, so
        // a raw strategy target passed anywhere else still books at the cap.
        // Venue-wide last-write-wins; a tighter cap only ever refuses, never
        // over-leverages, so the failure direction stays fail-closed.
        for paper in [&buy_paper, &sell_paper] {
            if paper.max_leverage_cap() != account_cap {
                paper
                    .set_max_leverage_cap(account_cap)
                    .map_err(|e| e.to_string())?;
            }
        }
        buy_paper
            .set_leverage(symbol, target_leverage)
            .await
            .map_err(|e| e.to_string())?;
        sell_paper
            .set_leverage(symbol, target_leverage)
            .await
            .map_err(|e| e.to_string())?;
    } else {
        // Live leverage set is best-effort: venues may reject no-op changes
        // or cap at account level. A rejected change is NOT fatal (the order
        // submits at existing leverage) but must be visible — an operator
        // reading "5x" while the venue kept 20x changes margin math.
        for (vname, adapter) in [(buy_venue, &buy_adapter), (sell_venue, &sell_adapter)] {
            if let Err(e) = adapter.set_leverage(symbol, target_leverage).await {
                tracing::warn!(
                    "live set_leverage {target_leverage}x on {vname} {symbol} failed: {e} — order will submit at the venue's existing leverage"
                );
            }
        }
    }

    let (buy_exec, sell_exec): (
        Arc<dyn arbitrageur_core::exchange::ExchangeAdapter>,
        Arc<dyn arbitrageur_core::exchange::ExchangeAdapter>,
    ) = if is_live {
        (buy_adapter.clone(), sell_adapter.clone())
    } else {
        (buy_paper.clone(), sell_paper.clone())
    };

    let order_buy =
        router.build_ioc_order(buy_exec.venue_id(), symbol, Side::Buy, buy_ref, qty_dec);
    let order_sell =
        router.build_ioc_order(sell_exec.venue_id(), symbol, Side::Sell, sell_ref, qty_dec);

    let mut equity_before = dec!(0);
    let mut equity_before_by_venue = HashMap::new();
    if !is_live {
        for (vid, paper) in state.paper_exchanges() {
            let acc = paper.fetch_account().await.map_err(|e| e.to_string())?;
            equity_before += acc.equity;
            if let Ok(f) = acc.equity.to_string().parse::<f64>() {
                if f.is_finite() {
                    equity_before_by_venue.insert(vid.to_string(), f);
                }
            }
        }
    } else {
        equity_before = acc_buy.equity + acc_sell.equity;
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
        .record_signal_execution(&crate::state_store::NewSignalExecution {
            id: signal_id.clone(),
            strategy_id: strategy_id.to_string(),
            symbol: symbol.to_string(),
            route_id: accounts.route_id,
            expected_net_usd: Some(expected_net_usd),
            note: if is_live && canary_max_loss_bps.is_some() {
                Some(format!(
                    "CANARY entry: profit floor bypassed down to -{:.0} bps to test live mechanics",
                    canary_max_loss_bps.unwrap_or(0.0)
                ))
            } else {
                None
            },
            legs: vec![
                crate::state_store::NewSignalLeg {
                    account_id: accounts.long,
                    venue: buy_vid.to_string(),
                    side: crate::state_store::LegSide::Long,
                    symbol: None,
                    intended_qty: order_qty,
                    intended_price: buy_ref.to_string().parse::<f64>().ok(),
                },
                crate::state_store::NewSignalLeg {
                    account_id: accounts.short,
                    venue: sell_vid.to_string(),
                    side: crate::state_store::LegSide::Short,
                    symbol: None,
                    intended_qty: order_qty,
                    intended_price: sell_ref.to_string().parse::<f64>().ok(),
                },
            ],
        })
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
    let res = state
        .coordinator
        .execute_two_leg(&buy_exec, order_buy, &sell_exec, order_sell)
        .await;
    let latency_us = started.elapsed().as_micros() as u64;

    let (attempt_pnl_f, total_f) = if !is_live {
        let mut equity_after = dec!(0);
        for (_, paper) in state.paper_exchanges() {
            equity_after += paper
                .fetch_account()
                .await
                .map_err(|e| e.to_string())?
                .equity;
        }
        // Own fills move the expected-equity baselines forward so the next
        // attribution poll does not misread trade PnL as a manual flow.
        refresh_equity_expectations_after_fills(state, &equity_before_by_venue).await;
        let attempt_pnl = equity_after - equity_before;
        let pnl_f = attempt_pnl.to_string().parse::<f64>().unwrap_or(0.0);
        // Realized portfolio P&L is always derived from actual paper-account
        // equity: fees, unwind slippage, and any residual loss are included;
        // unrealized open-position exposure is not marked into it.
        let tot_f = {
            let mut total_pnl = state.total_realized_profit.write();
            *total_pnl = equity_after - state.paper_baseline_equity();
            total_pnl.to_string().parse::<f64>().unwrap_or(0.0)
        };
        state.circuit_breaker.update_equity(equity_after);
        (pnl_f, tot_f)
    } else {
        let pnl_f = match &res {
            ArbitrageExecutionResult::Success {
                realized_profit, ..
            } => realized_profit.to_string().parse::<f64>().unwrap_or(0.0),
            _ => 0.0,
        };
        let tot_f = {
            let mut total_pnl = state.total_realized_profit_live.write();
            *total_pnl += Decimal::from_str_exact(&pnl_f.to_string()).unwrap_or(dec!(0));
            total_pnl.to_string().parse::<f64>().unwrap_or(0.0)
        };
        (pnl_f, tot_f)
    };

    let cycle = {
        let mut step = state.sim_step_counter.write();
        *step += 1;
        *step
    };

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
    let res = match res {
        ArbitrageExecutionResult::PartialPairKept {
            leg_a_fill,
            leg_b_fill,
            matched_qty,
            unwind_a,
            unwind_b,
            realized_profit: _,
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
            let mut a = leg_a_fill;
            a.filled_qty = matched_qty;
            a.remaining_qty = dec!(0);
            a.original_qty = matched_qty;
            let mut b = leg_b_fill;
            b.filled_qty = matched_qty;
            b.remaining_qty = dec!(0);
            b.original_qty = matched_qty;
            // Realized cash P&L stays fee outflow only (entry + trim fees are
            // already in paper equity); the matched pair itself is unrealized.
            ArbitrageExecutionResult::Success {
                realized_profit: -(a.fee + b.fee),
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
            let fees_dec = leg_a.fee + leg_b.fee;
            let fee_f = fees_dec.to_string().parse::<f64>().unwrap_or(0.0);
            let buy_px_f = leg_a
                .avg_fill_price
                .unwrap_or(buy_vwap)
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0);
            let sell_px_f = leg_b
                .avg_fill_price
                .unwrap_or(sell_vwap)
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0);
            let expected_capture_f = expected_net_usd;

            let cycle_id = format!("paper-spatial-{}", cycle);
            state.virtual_ledger.write().open_cycle(
                cycle_id,
                crate::attribution::ledger_strategy_id(strategy_id),
                symbol.to_string(),
                buy_venue.to_string(),
                sell_venue.to_string(),
                net_bps_f,
                open_qty,
                buy_px_f,
                open_qty,
                sell_px_f,
                fee_f,
            );

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
                    if is_live { "Live real" } else { "Paper" },
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
                    if is_live { "Live" } else { "Paper" },
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
                    let fee = f.fee.to_string().parse::<f64>().unwrap_or(0.0);
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
    state: &AppEngineState,
) -> Result<Vec<CandidateOutcomeDto>, String> {
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
            .ticker_blocked(&symbol_key)
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
            None,
            winner.predicted_win_probability.map(|p| EntryPrediction {
                kind: "reversion",
                win_probability: p,
                expected_net_bps: None,
            }),
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

/// Per-ticker execution/position state: whether the symbol is blocked, its
/// blocking signals, and the net filled position across all signals.
#[tauri::command]
pub async fn get_ticker_position_state(
    symbol: String,
    state: tauri::State<'_, Arc<AppEngineState>>,
) -> Result<serde_json::Value, String> {
    let blocking = state
        .state_store
        .blocking_signals(&symbol)
        .map_err(|e| e.to_string())?;
    let net = state
        .state_store
        .ticker_net_position(&symbol)
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "symbol": symbol,
        "blocked": !blocking.is_empty(),
        "blockingSignalIds": blocking.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
        "netPositionQty": net,
    }))
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
    Ok(crate::state_store::aggregate_open_positions(
        &signals,
        &|venue, symbol| {
            hub.get_valid_orderbook(&VenueId::from(venue), symbol, None)
                .and_then(|b| b.mid_price())
                .and_then(|m| dec_to_f64(m, "mark mid").ok())
        },
    ))
}

#[tauri::command]
pub async fn execute_spatial_candidates(
    candidates: Vec<SpatialCandidateInput>,
    state: tauri::State<'_, Arc<AppEngineState>>,
) -> Result<Vec<CandidateOutcomeDto>, String> {
    execute_spatial_candidates_impl(candidates, &state).await
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

/// Re-anchor the breaker after a paper-universe reset (sync context).
pub(crate) fn cb_reanchor_to_paper(state: &AppEngineState, total: Decimal) {
    state.circuit_breaker.reanchor_baseline(total);
    *state.last_risk_live_universe.write() = Some(*state.is_live_mode.read());
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
    match adapter.fetch_account().await {
        Ok(mut acc) => {
            acc.venue = vid.into();
            let now_ms = Utc::now().timestamp_millis();
            state
                .last_good_accounts
                .lock()
                .insert(vid.to_string(), (acc.clone(), now_ms));
            Ok((acc, None))
        }
        Err(e) => {
            let cached = state.last_good_accounts.lock().get(vid).cloned();
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

    // Risk telemetry: live mode reads live adapters for admitted venues so
    // the header reconciles with the Capital Allocation gadget. Paper mode
    // sums all paper simulators (legacy baseline — disabled venues still
    // hold paper equity; the venue gadget shows admitted only and the UI
    // footers the drift).
    let is_live = *state.is_live_mode.read();
    let mut accounts = Vec::new();
    if is_live {
        // A venue whose read fails keeps contributing its last-known-good
        // figures: a connectivity blip is not a loss of funds and must not
        // move the portfolio total or trip the drawdown breaker.
        for (vid, _name, adapter) in state.get_venues() {
            match live_account_or_cached(state, vid, &adapter).await {
                Ok((acc, stale_age)) => {
                    if let Some(age) = stale_age {
                        tracing::warn!(
                            "risk overview: {vid} read failed, using last-known figures ({}s old)",
                            age / 1000
                        );
                    }
                    accounts.push(acc);
                }
                Err(e) => {
                    tracing::warn!(
                        "risk overview: live account read failed for {vid} (no cache): {e}"
                    );
                }
            }
        }
        // Fall back to paper baseline if no live account read succeeded
        // (e.g. missing credentials) so the overview never goes blank.
        if accounts.is_empty() {
            for (vid, paper) in state.paper_exchanges() {
                let mut acc = paper.fetch_account().await.map_err(|e| e.to_string())?;
                acc.venue = vid.into();
                accounts.push(acc);
            }
        }
    } else {
        for (vid, paper) in state.paper_exchanges() {
            let mut acc = paper.fetch_account().await.map_err(|e| e.to_string())?;
            acc.venue = vid.into();
            accounts.push(acc);
        }
    }

    let total_equity_dec: Decimal = accounts.iter().map(|a| a.equity).sum();
    let total_free_margin_dec: Decimal = accounts.iter().map(|a| a.free_margin).sum();
    let total_mm_dec: Decimal = accounts.iter().map(|a| a.maintenance_margin).sum();
    let all_positions: Vec<_> = accounts.iter().flat_map(|a| a.positions.clone()).collect();

    // Feed observed equity into the breaker, then re-read its state so a
    // drawdown trip is reflected in this same snapshot. A paper<->live
    // universe switch is NOT a loss: re-anchor the baseline instead of
    // tripping (fixes the recurring false halt where a $200k paper baseline
    // vs ~$3.5k live funds reads as a -$196k "drawdown" on a flat book).
    {
        let mut universe = state.last_risk_live_universe.write();
        if *universe != Some(is_live) {
            cb.reanchor_baseline(total_equity_dec);
            *universe = Some(is_live);
        } else {
            cb.update_equity(total_equity_dec);
        }
    }
    let is_halted = cb.is_tripped();
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
        MarginHealthStatus::Critical => 2,
        MarginHealthStatus::Breached => 3,
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

fn ledger_qty_to_dec(value: f64, what: &str) -> Result<Decimal, String> {
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
fn passive_cfg_for(state: &AppEngineState, strategy_id: &str) -> Option<PassiveUnwindConfig> {
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

/// Re-fetcher for passive retries: every attempt re-prices from a newly
/// fetched, validated adapter book mirrored into the paper exchange —
/// exactly like the initial flatten fetch, never a stale peg.
struct FlattenPaperRefresh<'a> {
    state: &'a AppEngineState,
    venue_label: String,
    venue_id: VenueId,
    symbol: String,
    paper: Arc<arbitrageur_exchanges::simulated::SimulatedExchange>,
}

#[async_trait::async_trait]
impl PassiveBookRefresh for FlattenPaperRefresh<'_> {
    async fn fresh_book(&self) -> anyhow::Result<OrderBook> {
        // Mirror the initial flatten fetch: prefer the hub's fresh book
        // (no network, no revalidation — the hub only holds live books),
        // else fetch from the adapter with full validation.
        if let Some(b) = self.state.market_data_hub.get_valid_orderbook(
            &self.venue_id,
            &self.symbol,
            Some(std::time::Duration::from_secs(2)),
        ) {
            self.paper
                .replace_orderbook(b.clone())
                .map_err(anyhow::Error::msg)?;
            return Ok(b);
        }
        let adapter = self.state.get_adapter(&self.venue_label).ok_or_else(|| {
            anyhow::anyhow!(
                "Venue '{}' is not admitted for paper execution",
                self.venue_label
            )
        })?;
        let b = adapter.fetch_orderbook(&self.symbol).await.map_err(|e| {
            anyhow::anyhow!(
                "No fresh {} orderbook for {}: {e}",
                self.venue_label,
                self.symbol
            )
        })?;
        validate_adapter_book(&b, &self.venue_id, &self.symbol)
            .map_err(|e| anyhow::anyhow!("Flatten order book rejected: {e}"))?;
        self.state.market_data_hub.update_orderbook(b.clone());
        self.paper
            .replace_orderbook(b.clone())
            .map_err(anyhow::Error::msg)?;
        Ok(b)
    }
}

/// Live-venue refresher for passive re-pegs: a fresh WS-fed hub book wins;
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

/// Place a reduce-only market order on a venue's paper exchange to close one
/// filled leg. Returns (filled_qty, avg_price, fee_usd, order_id).
async fn flatten_leg_on_paper(
    args: FlattenLegArgs<'_>,
) -> Result<Option<(Decimal, Decimal, Decimal, String)>, String> {
    let FlattenLegArgs {
        state,
        venue,
        symbol,
        side,
        qty,
        strategy_id,
        sig_id,
        notes,
        execution_type,
    } = args;
    let canonical = crate::state::canonical_venue_id(venue)
        .ok_or_else(|| format!("Unknown venue '{venue}'"))?;
    let adapter = state
        .get_adapter(venue)
        .ok_or_else(|| format!("Venue '{venue}' is not admitted for paper execution"))?;
    let vid = adapter.venue_id();
    let paper = state
        .get_paper_exchange(canonical)
        .ok_or_else(|| format!("No paper exchange admitted for venue '{venue}'"))?;

    let book = match state.market_data_hub.get_valid_orderbook(
        &vid,
        symbol,
        Some(std::time::Duration::from_secs(2)),
    ) {
        Some(b) => b,
        None => {
            let b = adapter
                .fetch_orderbook(symbol)
                .await
                .map_err(|e| format!("No fresh {venue} orderbook for {symbol}: {e}"))?;
            validate_adapter_book(&b, &vid, symbol)
                .map_err(|e| format!("Flatten order book rejected: {e}"))?;
            state.market_data_hub.update_orderbook(b.clone());
            b
        }
    };
    paper.replace_orderbook(book).map_err(|e| e.to_string())?;

    if qty <= dec!(0) {
        return Err("Invalid leg quantity".to_string());
    }
    let passive_cfg = match execution_type {
        Some("emergency_market") => None,
        Some("non_emergency_limit") => {
            Some(passive_cfg_for(state, strategy_id).unwrap_or_else(|| {
                arbitrageur_execution::passive_unwind::PassiveUnwindConfig {
                    enabled: true,
                    ..Default::default()
                }
            }))
        }
        _ => passive_cfg_for(state, strategy_id),
    };
    if let Some(cfg) = passive_cfg {
        let paper_adapter: Arc<dyn ExchangeAdapter> = paper.clone();
        let refresher = FlattenPaperRefresh {
            state,
            venue_label: venue.to_string(),
            venue_id: vid.clone(),
            symbol: symbol.to_string(),
            paper: paper.clone(),
        };
        // No fast-vol tracker exists on the flatten path, so the spread leg
        // is the hard guard (documented in `calm_gate_ok`).
        let report =
            execute_passive_reduce(&paper_adapter, &refresher, symbol, side, qty, &cfg, None)
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
        }
        let resp = report.response;
        if resp.filled_qty <= dec!(0) {
            return Ok(None);
        }
        let px = resp
            .avg_fill_price
            .filter(|p| *p > dec!(0))
            .ok_or_else(|| "flatten fill reports no positive price".to_string())?;
        return Ok(Some((resp.filled_qty, px, resp.fee, resp.order_id)));
    }
    let mut order =
        arbitrageur_core::order::OrderRequest::new_market(paper.venue_id(), symbol, side, qty);
    order.reduce_only = true;

    let resp = paper.place_order(order).await.map_err(|e| e.to_string())?;
    if resp.filled_qty <= dec!(0) {
        return Ok(None);
    }
    let px = resp
        .avg_fill_price
        .filter(|p| *p > dec!(0))
        .ok_or_else(|| "flatten fill reports no positive price".to_string())?;
    Ok(Some((resp.filled_qty, px, resp.fee, resp.order_id)))
}

/// Outcome of one flatten-leg dispatch. `fill` is present only when the
/// venue reported an actual fill; `order_id`/`client_order_id` are present
/// whenever the venue accepted the order so the reconciler can keep polling
/// a working order instead of losing it.
struct FlattenLegOutcome {
    order_id: Option<String>,
    client_order_id: Option<String>,
    fill: Option<(Decimal, Decimal, Decimal)>,
}

/// Place a reduce-only market order on a venue (paper or live) to close one
/// filled leg.
async fn flatten_leg(args: FlattenLegArgs<'_>, is_live: bool) -> Result<FlattenLegOutcome, String> {
    if !is_live {
        return flatten_leg_on_paper(args).await.map(|res| match res {
            Some((q, p, f, oid)) => FlattenLegOutcome {
                order_id: Some(oid),
                client_order_id: None,
                fill: Some((q, p, f)),
            },
            None => FlattenLegOutcome {
                order_id: None,
                client_order_id: None,
                fill: None,
            },
        });
    }
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
        }
        let resp = report.response;
        let order_id = (!resp.order_id.trim().is_empty()).then(|| resp.order_id.clone());
        let fill = if resp.filled_qty > dec!(0) {
            let px = resp
                .avg_fill_price
                .filter(|p| *p > dec!(0))
                .ok_or_else(|| "flatten fill reports no positive price".to_string())?;
            Some((resp.filled_qty, px, resp.fee))
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
    order.reduce_only = true;
    let client_order_id = order.client_order_id.clone();

    let resp = adapter
        .place_order(order)
        .await
        .map_err(|e| e.to_string())?;
    let order_id = (!resp.order_id.trim().is_empty()).then(|| resp.order_id.clone());
    let fill = if resp.filled_qty > dec!(0) {
        let px = resp
            .avg_fill_price
            .filter(|p| *p > dec!(0))
            .ok_or_else(|| {
                format!(
                    "flatten fill on order {} reports no positive price",
                    resp.order_id
                )
            })?;
        Some((resp.filled_qty, px, resp.fee))
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
fn close_flattened_spatial_lot(
    state: &AppEngineState,
    sig: &crate::state_store::SignalExecution,
    family: &[crate::state_store::SignalExecution],
) -> Option<(f64, f64)> {
    use crate::state_store::LegSide;
    use arbitrageur_execution::virtual_ledger::CycleStatus;

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
        let realized = state
            .virtual_ledger
            .write()
            .close_cycle(
                &id,
                long_exit.px_qty / long_exit.qty,
                short_exit.px_qty / short_exit.qty,
                exit_fees,
                CycleStatus::StoppedOut,
            )
            .and_then(|l| match (l.realized_pnl_usd, l.realized_pnl_bps) {
                (Some(usd), Some(bps)) => Some((usd, bps)),
                _ => None,
            });
        // Calibration resolution: the ledger's realized net bps is the
        // authoritative outcome the entry forecast gets scored against.
        // Non-fatal — a prediction left unresolved is uncounted, not wrong.
        if let Some((_, bps)) = realized {
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
pub async fn flatten_ticker_positions_impl(
    symbol: Option<&str>,
    strategy_id: &str,
    state: &AppEngineState,
) -> Result<FlattenResultDto, String> {
    flatten_ticker_positions_with_type_impl(symbol, strategy_id, None, state).await
}

pub async fn flatten_ticker_positions_with_type_impl(
    symbol: Option<&str>,
    strategy_id: &str,
    execution_type: Option<&str>,
    state: &AppEngineState,
) -> Result<FlattenResultDto, String> {
    use crate::state_store::{LegSide, NewSignalExecution, NewSignalLeg, SignalStatus};

    let sid = crate::state_store::normalize_strategy_id(strategy_id).map_err(|e| e.to_string())?;
    let is_live = *state.is_live_mode.read();
    if is_live {
        let is_unlocked = state.vault.write().is_unlocked();
        if !is_unlocked {
            return Err("Cannot flatten in Live Trading mode while the secure credential vault is locked. Unlock the vault first.".into());
        }
    }
    let _execution_guard = state.execution_lock.lock().await;
    let flatten_equity_before = if !is_live {
        snapshot_paper_equity(state).await
    } else {
        HashMap::new()
    };

    let all_signals = state
        .state_store
        .list_signals(symbol, None)
        .map_err(|e| e.to_string())?;
    let roots: Vec<crate::state_store::SignalExecution> = all_signals
        .iter()
        .filter(|s| {
            !s.id.starts_with("flat-")
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

    let cycle = {
        let mut step = state.sim_step_counter.write();
        *step += 1;
        *step
    };

    let papers = state.paper_exchanges();
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
                    "{}: venue '{}' is not a canonical paper venue",
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
                if is_live {
                    match state.get_adapter(canonical) {
                        Some(adapter) => {
                            if !adapter.live_order_submission_implemented()
                                || !adapter.has_live_credentials()
                            {
                                unreadable.insert(canonical);
                                out.notes.push(format!(
                                    "{}: venue '{}' lacks authenticated live order-submission capability",
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
                                            "{}: cannot read {} live account: {e}",
                                            sig.id, leg.venue
                                        ));
                                    }
                                }
                            }
                        }
                        None => {
                            unreadable.insert(canonical);
                            out.notes.push(format!(
                                "{}: no live adapter exists for venue '{}'",
                                sig.id, leg.venue
                            ));
                        }
                    }
                } else {
                    match papers
                        .iter()
                        .find(|(vid, _)| *vid == canonical)
                        .map(|(_, p)| p.clone())
                    {
                        Some(paper) => match paper.fetch_account().await {
                            Ok(acc) => {
                                accounts.insert(canonical, acc);
                            }
                            Err(e) => {
                                unreadable.insert(canonical);
                                out.notes.push(format!(
                                    "{}: cannot read {} paper account: {e}",
                                    sig.id, leg.venue
                                ));
                            }
                        },
                        None => {
                            unreadable.insert(canonical);
                            out.notes.push(format!(
                                "{}: no paper account exists for venue '{}'",
                                sig.id, leg.venue
                            ));
                        }
                    }
                }
            }
            if unreadable.contains(canonical) {
                undetermined = true;
                continue;
            }
            let pos = match accounts.get(canonical) {
                Some(acc) if is_live => match state.get_adapter(canonical) {
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
                Some(acc) => acc.get_position(&leg.symbol).cloned(),
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
                    let basis = residual.min(p.quantity);
                    // Leftover sweep: when the residual exceeds the closable
                    // quantity by less than one full ticker step, close the
                    // ENTIRE residual instead of stranding sub-step dust.
                    // Oversized reduce-only paper orders cap at the position.
                    let mut qty = basis;
                    let swept =
                        sweep_flatten_qty(residual, basis, flatten_step_qty(state, &sig.symbol));
                    if swept > basis {
                        qty = swept;
                        out.notes.push(format!(
                            "{}: {} leg sweep closes entire residual {}/{} (sub-step leftover)",
                            sig.id, leg.venue, swept, basis
                        ));
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
                .list_signals(None, None)
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
            close_family(&format!("flatten {}: no residual paper position", sig.id))?;
            out.signals_closed += 1;
            close_flattened_spatial_lot(state, &sig, &family);
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
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        state
            .state_store
            .record_signal_execution(&NewSignalExecution {
                id: close_id.clone(),
                strategy_id: sig.strategy_id.clone(),
                symbol: sig.symbol.clone(),
                route_id: sig.route_id.clone(),
                expected_net_usd: None,
                note: Some(format!("emergency flatten of {}", sig.id)),
                legs: child_legs,
            })
            .map_err(|e| format!("Failed to persist flatten signal record: {e}"))?;

        let mut leg_desc = Vec::new();
        for t in &targets {
            match flatten_leg(
                FlattenLegArgs {
                    state,
                    venue: &t.venue,
                    symbol: &t.exact_symbol,
                    side: t.order_side,
                    qty: t.qty,
                    strategy_id: &sig.strategy_id,
                    sig_id: &sig.id,
                    notes: &mut out.notes,
                    execution_type,
                },
                is_live,
            )
            .await
            {
                Ok(outcome) => {
                    // Persist the accepted order id BEFORE any fill write so a
                    // working order is always reachable by the reconciliation
                    // supervisor — a zero-fill ack must not orphan the order.
                    if let (true, Some(oid)) = (is_live, outcome.order_id.as_deref()) {
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
                                    dec_to_f64(fee, "fill fee")?,
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
            if is_live {
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
            } else {
                let Some((_, paper)) = papers.iter().find(|(vid, _)| *vid == exp.canonical) else {
                    fully_flat = false;
                    continue;
                };
                match paper.fetch_account().await {
                    Ok(acc) => match acc
                        .positions
                        .iter()
                        .find(|p| p.symbol == exp.exact_symbol && p.quantity > dec!(0))
                    {
                        None => {}
                        Some(p) if p.side != exp.expected_pos_side => {
                            fully_flat = false;
                            out.notes.push(format!(
                                "{}: {} position side {:?} contradicts {:?} leg; leaving open",
                                sig.id, exp.canonical, p.side, exp.entry_side
                            ));
                        }
                        Some(_) => fully_flat = false,
                    },
                    Err(_) => fully_flat = false,
                }
            }
        }

        let note = format!("emergency flatten via {close_id}");
        // Realized PnL comes from the attributed ledger lot when the exit
        // fills cover it; single-leg residuals (e.g. triangle legs) realize
        // purely through their signal legs and keep the 0.0 log convention.
        let mut flatten_realized: Option<(f64, f64)> = None;
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
            flatten_realized = close_flattened_spatial_lot(state, &sig, &family_now);
            if let Some(bot) = state.radar_bot.lock().clone() {
                bot.forget_position(&sig.symbol, &venue_a, &venue_b);
                let _ = state
                    .state_store
                    .delete_bot_position(&sig.symbol, &venue_a, &venue_b);
                let _ = state
                    .state_store
                    .delete_bot_position(&sig.symbol, &venue_b, &venue_a);
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
                net_profit: flatten_realized.map(|(usd, _)| usd).unwrap_or(0.0),
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
            !s.id.starts_with("flat-")
                && s.strategy_id.eq_ignore_ascii_case(&sid)
                && s.status.counts_as_open()
        })
        .count();

    if !is_live {
        let mut equity_after = dec!(0);
        for (_, paper) in state.paper_exchanges() {
            equity_after += paper
                .fetch_account()
                .await
                .map_err(|e| e.to_string())?
                .equity;
        }
        {
            let mut total_pnl = state.total_realized_profit.write();
            *total_pnl = equity_after - state.paper_baseline_equity();
        }
        state.circuit_breaker.update_equity(equity_after);
        refresh_equity_expectations_after_fills(state, &flatten_equity_before).await;
    }
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
    canary_max_loss_bps: Option<f64>,
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
        canary_max_loss_bps,
        predicted_win_probability.map(|p| EntryPrediction {
            kind: "reversion",
            win_probability: p,
            expected_net_bps: None,
        }),
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
    is_live: bool,
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

    // Resolve each leg's live/paper position (side-checked).
    let resolve_pos = async |leg: &crate::state_store::SignalLeg| {
        let canonical = crate::state::canonical_venue_id(&leg.venue)?;
        let acc = if is_live {
            // Fresh cache only — never fetch here (REST pressure), never
            // trim on stale telemetry.
            let (acc, ts) = state.last_good_accounts.lock().get(canonical)?.clone();
            if Utc::now().timestamp_millis().saturating_sub(ts) > 120_000 {
                return None;
            }
            acc
        } else {
            state
                .get_paper_exchange(canonical)?
                .fetch_account()
                .await
                .ok()?
        };
        if is_live {
            let adapter = state.get_adapter(canonical)?;
            adapter.account_position_for(&acc, &leg.symbol).ok()?
        } else {
            acc.get_position(&leg.symbol).cloned()
        }
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
            state
                .liq_trim_at
                .lock()
                .insert(sig.symbol.clone(), std::time::Instant::now());
            let trim_dec = Decimal::from_f64_retain(qty)?;
            let mut note = format!(
                "liq trim: leg within {min_dist_pct:.1}% of liquidation — reducing both legs by {qty}"
            );
            let close_id = format!("flat-{}-{:04x}", sig.id, rand::random::<u16>());
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
            })
            .collect();
            if let Err(e) =
                state
                    .state_store
                    .record_signal_execution(&crate::state_store::NewSignalExecution {
                        id: close_id.clone(),
                        strategy_id: sig.strategy_id.clone(),
                        symbol: sig.symbol.clone(),
                        route_id: sig.route_id.clone(),
                        expected_net_usd: None,
                        note: Some(format!("liq trim of {}", sig.id)),
                        legs: child_legs,
                    })
            {
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
                match flatten_leg(
                    FlattenLegArgs {
                        state,
                        venue: &leg.venue,
                        symbol: &leg.symbol,
                        side: order_side,
                        qty: trim_dec,
                        strategy_id: &sig.strategy_id,
                        sig_id: &sig.id,
                        notes: &mut notes,
                        execution_type: Some("emergency_market"),
                    },
                    is_live,
                )
                .await
                {
                    Ok(outcome) => {
                        if is_live {
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
                        }
                        match outcome.fill {
                            Some((q, p, f)) => {
                                match (
                                    dec_to_f64(q, "trim qty"),
                                    dec_to_f64(p, "trim price"),
                                    dec_to_f64(f, "trim fee"),
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
            for n in notes {
                note.push_str(&format!("; {n}"));
            }
            Some(("trim", "liq_trim", note, false))
        }
        PairTrimDecision::CloseBoth => {
            state
                .liq_trim_at
                .lock()
                .insert(sig.symbol.clone(), std::time::Instant::now());
            let reason = match flatten_ticker_positions_impl(Some(&sig.symbol), "futures", state)
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
        evaluate_spatial_exit, SpatialExitConfig, SpatialExitVerdict,
    };
    let sid = crate::state_store::normalize_strategy_id(strategy_id).map_err(|e| e.to_string())?;
    if sid != "futures" {
        return Err(
            "convergence check supports the 'futures' strategy only (radar positions have their own bot lifecycle)"
                .into(),
        );
    }
    let cfg = SpatialExitConfig::default();
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
    for sig in signals
        .iter()
        .filter(|s| s.strategy_id.eq_ignore_ascii_case(&sid) && s.status.counts_as_open())
    {
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
                let is_live = *state.is_live_mode.read();
                let trim_threshold = state
                    .strategy_params
                    .read()
                    .get("futures")
                    .and_then(|p| p.futures_liq_trim_pct)
                    .filter(|t| t.is_finite() && *t > 0.0 && *t <= 100.0);
                let trim = match trim_threshold {
                    Some(t) => maybe_liq_trim_pair(state, sig, buy, sell, t, is_live).await,
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
                match flatten_ticker_positions_impl(Some(&sig.symbol), &sid, state).await {
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
