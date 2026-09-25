//! IPC surface for the Spread Range Monitor (the radar engine lives in
//! `crate::monitor`): snapshot/config reads, watchlist exemptions, signal
//! events, and the manual-entry path that funnels a radar row into spatial
//! execution. Split out of `commands.rs`.

use crate::commands::{
    execute_spatial_arbitrage_inner, reject_if_strategy_filtered, validate_adapter_book,
    EntryPrediction, SimStepResultDto,
};
use crate::state::AppEngineState;
use std::sync::Arc;
use tauri::State;

// Spread Range Monitor — systematic cross-venue opportunity radar
// ---------------------------------------------------------------------------

fn monitor_engine(
    state: &AppEngineState,
) -> Result<Arc<crate::monitor::SpreadMonitorEngine>, String> {
    state
        .monitor
        .lock()
        .clone()
        .ok_or_else(|| "spread monitor is not running".to_string())
}

#[tauri::command]
pub async fn get_monitor_snapshot(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Option<crate::monitor::MonitorSnapshotDto>, String> {
    Ok(monitor_engine(&state)?.snapshot())
}

#[tauri::command]
pub async fn get_monitor_config(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<crate::monitor::MonitorConfigDto, String> {
    Ok(monitor_engine(&state)?.config())
}

#[tauri::command]
pub async fn set_monitor_config(
    config: crate::monitor::MonitorConfigDto,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<(), String> {
    monitor_engine(&state)?.set_config(config, &state)
}

/// Recent signal-lifecycle audit events (transitions + heartbeats), newest
/// first. No UI consumes this in v1 — reserved for the future trading bot
/// and debugging via IPC.
#[tauri::command]
pub async fn get_monitor_signal_events(
    symbol: Option<String>,
    buy_venue: Option<String>,
    sell_venue: Option<String>,
    limit: Option<i64>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<crate::state_store::MonitorSignalEventRow>, String> {
    let clean = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
    let lim = limit.unwrap_or(200).clamp(1, 10_000);
    state
        .state_store
        .recent_monitor_signal_events_mode(
            clean(symbol).as_deref(),
            clean(buy_venue).as_deref(),
            clean(sell_venue).as_deref(),
            lim,
            state.usd_merge_quotes(),
            state.env().as_str(),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_ticker_exempted(
    symbol: String,
    exempt: bool,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<(), String> {
    // Store the exemption under the active quote policy's key: merged
    // modes canonicalize to the base (exempts every USD-family variant);
    // `separate` keeps the exact pair so a user can exempt BTC-USDT
    // without exempting BTC-USDC. Matching at refresh also honors a bare
    // base stored row across all variants.
    let sym = arbitrageur_core::symbol::pair_key(&symbol, state.usd_merge_quotes());
    if sym.is_empty() || sym.len() > 64 {
        return Err("invalid symbol".into());
    }
    state
        .state_store
        .set_ticker_exempt(&sym, exempt)
        .map_err(|e| e.to_string())?;
    // Force the watchlist to re-derive on the next tick.
    if let Some(engine) = state.monitor.lock().clone() {
        engine.request_watchlist_refresh();
    }
    Ok(())
}

/// Currently exempted symbols (canonical base form).
#[tauri::command]
pub async fn get_exempted_tickers(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<String>, String> {
    state
        .state_store
        .exempt_tickers()
        .map_err(|e| e.to_string())
}

/// Global "reveal all": clears every ticker exemption. Returns the number
/// of exemptions removed.
#[tauri::command]
pub async fn reveal_all_tickers(state: State<'_, Arc<AppEngineState>>) -> Result<usize, String> {
    let n = state
        .state_store
        .clear_ticker_exemptions()
        .map_err(|e| e.to_string())?;
    if let Some(engine) = state.monitor.lock().clone() {
        engine.request_watchlist_refresh();
    }
    Ok(n)
}

/// Per-row Execute: re-validates the opportunity against the latest
/// snapshot (must still be qualified and unblocked), converts the USD size
/// to base quantity on the live book, then dispatches the guarded
/// execution path (env-aware adapters; live additionally gated by
/// `live_entry_gate`).
#[tauri::command]
pub async fn execute_monitor_opportunity(
    symbol: String,
    buy_venue: String,
    sell_venue: String,
    size_usd: f64,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SimStepResultDto, String> {
    execute_monitor_opportunity_impl(&symbol, &buy_venue, &sell_venue, size_usd, &state).await
}

/// Shared monitor-entry primitive. The manual row-Execute button and the
/// autonomous convergence bot both enter through here, so the fail-closed
/// gates (executability, stale books, env-capability, blocking signals) apply
/// identically to human and bot flow. In live mode the inner spatial path
/// applies `live_entry_gate` (vault + adapter eligibility + credentials).
pub(crate) async fn execute_monitor_opportunity_impl(
    symbol: &str,
    buy_venue: &str,
    sell_venue: &str,
    size_usd: f64,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    let engine = monitor_engine(state)?;
    // Watchlist key under the active quote policy: merged modes canonicalize
    // to the base ("BTC"), `separate` keeps the pair key ("BTC-USDT") so the
    // snapshot row lookup lands on the same key the monitor produced.
    let sym = arbitrageur_core::symbol::pair_key(symbol, state.usd_merge_quotes());
    if !(size_usd.is_finite() && size_usd > 0.0) {
        return Err("invalid size".into());
    }
    let snap = engine
        .snapshot()
        .ok_or_else(|| "monitor has not produced a snapshot yet".to_string())?;
    let row = snap
        .rows
        .iter()
        .find(|r| r.symbol == sym && r.buy_venue == buy_venue && r.sell_venue == sell_venue)
        .ok_or_else(|| "opportunity is no longer tracked".to_string())?;
    if !row.executable {
        return Err(row
            .block_reason
            .clone()
            .unwrap_or_else(|| "opportunity is not executable".into()));
    }
    // Auto-filtered tickers stay research-visible on the radar but refuse
    // Execute with an explicit block reason. Manual (global-exempt) hiding
    // is enforced separately by the watchlist derivation itself.
    {
        let mut scope = engine.in_scope_venues(&sym);
        if scope.len() < 2 {
            scope = vec![buy_venue.to_string(), sell_venue.to_string()];
            scope.sort();
            scope.dedup();
        }
        reject_if_strategy_filtered(state, "radar", &sym, &scope).await?;
    }
    // Depeg gate: books priced in a halted stablecoin are not tradeable.
    // Stable-vs-USD books (base is itself USD-family) stay open — that
    // spread is the depeg arbitrage.
    let buy_quote = state.book_quote(buy_venue, &sym);
    let sell_quote = state.book_quote(sell_venue, &sym);
    state.stablecoin_gate(&sym, &buy_quote)?;
    state.stablecoin_gate(&sym, &sell_quote)?;

    // Convert USD size to base qty at the current buy-side ask, adjusted
    // for the quote coin's USD rate (a USDT ask at 0.99 costs fewer real
    // dollars per unit than its face value suggests). Sizing reads the same
    // dispatch-time freshness bound as execution: hub cache ≤2s, else a
    // fresh fetch through the admitted adapter — a minute-old ask would
    // silently misprice the quantity.
    let buy_vid = arbitrageur_core::types::VenueId(buy_venue.to_string());
    let book = match state.market_data_hub.get_valid_orderbook(
        &buy_vid,
        &sym,
        Some(std::time::Duration::from_secs(2)),
    ) {
        Some(b) => b,
        None => {
            let adapter = state
                .get_adapter(buy_venue)
                .ok_or_else(|| format!("Venue '{buy_venue}' is not admitted for execution"))?;
            let b = adapter
                .fetch_orderbook(&sym)
                .await
                .map_err(|e| format!("No fresh {buy_venue} orderbook for {sym}: {e}"))?;
            validate_adapter_book(&b, &buy_vid, &sym)
                .map_err(|e| format!("Buy-side order book rejected: {e}"))?;
            state.market_data_hub.update_orderbook(b.clone());
            b
        }
    };
    let ask = book
        .asks
        .first()
        .and_then(|l| l.price.to_string().parse::<f64>().ok())
        .filter(|p| *p > 0.0)
        .ok_or_else(|| "no ask on buy-side book".to_string())?;
    let qty = size_usd / (ask * state.fx_price_factor(Some(buy_venue), &buy_quote));
    // Calibration forecast for this entry: prefer the net-P&L win
    // probability (scored against realized net bps at close); fall back to
    // the reversion probability, which is logged for audit but never
    // scored against P&L — the events differ.
    let prediction = row
        .net_win_probability
        .map(|p| EntryPrediction {
            kind: "net_win",
            win_probability: p,
            expected_net_bps: row.net_expected_bps.or(row.net_capture_bps),
        })
        .or_else(|| {
            row.win_probability.or(row.p_win).map(|p| EntryPrediction {
                kind: "reversion",
                win_probability: p,
                expected_net_bps: None,
            })
        });
    // Radar-dispatched fills persist under the "radar" strategy so the
    // Spread Radar page scopes positions separately from Simple Spread.
    let res = execute_spatial_arbitrage_inner(
        &sym,
        buy_venue,
        sell_venue,
        qty,
        Some("OPEN".into()),
        row.current_edge_bps,
        row.baseline_bps,
        None,
        None,
        None,
        None,
        "radar",
        prediction,
        state,
    )
    .await?;

    // Adopt into auto-exit watcher pool targeting dynamic baseline
    if let Some(bot) = state.radar_bot.lock().as_ref() {
        let friction_band = row
            .deviation_bps
            .zip(row.net_capture_bps)
            .and_then(|(d, n)| {
                arbitrageur_strategy::convergence_auto::friction_band_bps(Some(d), Some(n))
            });
        let _ = bot.adopt_manual_position(
            state,
            &sym,
            buy_venue,
            sell_venue,
            size_usd,
            row.deviation_bps,
            row.baseline_bps,
            friction_band,
            row.expected_converge_secs,
        );
    }

    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::{MonitorRowDto, MonitorSnapshotDto};
    use arbitrageur_core::quote::{BookLevel, OrderBook};
    use arbitrageur_core::types::{dec, Decimal, VenueId};
    use chrono::Utc;

    /// Sims stand in for hyperliquid/lighter; venue flags admit them so
    /// `get_venues`/`get_adapter` resolve like production wiring.
    fn ipc_state() -> AppEngineState {
        let state = AppEngineState::new();
        {
            let mut s = state.settings.write();
            s.hyperliquid_enabled = true;
            s.lighter_enabled = true;
            s.passive_unwind.timeout_secs = 0;
        }
        state.install_test_sim("hyperliquid");
        state.install_test_sim("lighter");
        state
    }

    /// Seed a book into the installed sim only — NOT the hub — so the IPC
    /// path must exercise the adapter-level fallback fetch.
    fn seed_sim_book(
        state: &AppEngineState,
        venue: &str,
        symbol: &str,
        bid: Decimal,
        ask: Decimal,
    ) {
        let mut b = OrderBook::new(VenueId::new(venue), symbol.to_string());
        b.bids.push(BookLevel::new(bid, dec!(10.0)));
        b.asks.push(BookLevel::new(ask, dec!(10.0)));
        b.timestamp = Utc::now();
        state
            .test_sims
            .read()
            .unwrap()
            .get(venue)
            .expect("sim installed")
            .replace_orderbook(b)
            .unwrap();
    }

    fn engine_with_rows(
        state: &AppEngineState,
        rows: Vec<MonitorRowDto>,
    ) -> Arc<crate::monitor::SpreadMonitorEngine> {
        let engine = crate::monitor::SpreadMonitorEngine::new(state);
        engine.inject_snapshot(MonitorSnapshotDto {
            generated_ts_ms: Utc::now().timestamp_millis(),
            running: true,
            rows,
            watchlist: vec![],
            venues: vec![],
            exempted: vec![],
        });
        engine
    }

    #[tokio::test]
    async fn execute_monitor_opportunity_requires_engine_and_snapshot() {
        // No engine at all — the IPC surfaces a plain "not running".
        let state = ipc_state();
        let err = execute_monitor_opportunity_impl("ANTH", "hyperliquid", "lighter", 100.0, &state)
            .await
            .expect_err("no engine");
        assert!(err.contains("not running"), "{err}");

        // Engine installed but no snapshot produced yet.
        let engine = crate::monitor::SpreadMonitorEngine::new(&state);
        *state.monitor.lock() = Some(engine);
        let err = execute_monitor_opportunity_impl("ANTH", "hyperliquid", "lighter", 100.0, &state)
            .await
            .expect_err("no snapshot");
        assert!(err.contains("not produced a snapshot"), "{err}");

        // Snapshot present but the requested row is gone — the opportunity
        // aged out between render and click; never dispatch on staleness.
        *state.monitor.lock() = Some(engine_with_rows(
            &state,
            vec![MonitorRowDto::test_row("BTC", "hyperliquid", "lighter")],
        ));
        let err = execute_monitor_opportunity_impl("ANTH", "hyperliquid", "lighter", 100.0, &state)
            .await
            .expect_err("row not tracked");
        assert!(err.contains("no longer tracked"), "{err}");
    }

    #[tokio::test]
    async fn execute_monitor_opportunity_refuses_blocked_row_and_bad_size() {
        let state = ipc_state();
        let mut row = MonitorRowDto::test_row("ANTH", "hyperliquid", "lighter");
        row.executable = false;
        row.block_reason = Some("session gate blocks entry".into());
        *state.monitor.lock() = Some(engine_with_rows(&state, vec![row]));
        // A blocked row surfaces ITS block reason — never a generic error
        // and never a dispatch.
        let err = execute_monitor_opportunity_impl("ANTH", "hyperliquid", "lighter", 100.0, &state)
            .await
            .expect_err("blocked row must refuse");
        assert_eq!(err, "session gate blocks entry");
        // Invalid sizes refuse before any dispatch work.
        for bad in [0.0, -50.0, f64::NAN, f64::INFINITY] {
            let err =
                execute_monitor_opportunity_impl("ANTH", "hyperliquid", "lighter", bad, &state)
                    .await
                    .expect_err("bad size");
            assert!(err.contains("invalid size"), "{err}");
        }
    }

    #[tokio::test]
    async fn execute_monitor_opportunity_dispatches_via_adapter_fallback() {
        // Full seam: an executable row + NO hub books — sizing must fall
        // back to a fresh adapter fetch, then dispatch the spatial entry
        // under the "radar" strategy id.
        let state = ipc_state();
        // Crossed books (buy ask > sell bid): negative static edge, but the
        // row's reversion baseline (-150bps) carries the entry economics —
        // same economics the commands-level radar test proves.
        seed_sim_book(&state, "hyperliquid", "ANTH", dec!(100.0), dec!(100.2));
        seed_sim_book(&state, "lighter", "ANTH", dec!(99.9), dec!(100.1));
        *state.monitor.lock() = Some(engine_with_rows(
            &state,
            vec![MonitorRowDto::test_row("ANTH", "hyperliquid", "lighter")],
        ));

        let res = execute_monitor_opportunity_impl("ANTH", "hyperliquid", "lighter", 100.0, &state)
            .await
            .expect("executable row must dispatch");
        assert!(res.execution_success, "{}", res.message);
        // The fills persist under the radar strategy so attribution scopes
        // them apart from the Simple Spread page.
        let sigs = state
            .state_store
            .list_signals(Some("ANTH"), None)
            .expect("list");
        assert!(
            sigs.iter().any(|s| s.strategy_id == "radar"),
            "radar signal must persist: {:?}",
            sigs.iter().map(|s| &s.id).collect::<Vec<_>>()
        );
    }
}
