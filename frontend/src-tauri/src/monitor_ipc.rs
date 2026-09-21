//! IPC surface for the Spread Range Monitor (the radar engine lives in
//! `crate::monitor`): snapshot/config reads, watchlist exemptions, signal
//! events, and the manual-entry path that funnels a radar row into spatial
//! execution. Split out of `commands.rs`.

use crate::commands::{
    execute_spatial_arbitrage_inner, reject_if_strategy_filtered, EntryPrediction, SimStepResultDto,
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
        .recent_monitor_signal_events(
            clean(symbol).as_deref(),
            clean(buy_venue).as_deref(),
            clean(sell_venue).as_deref(),
            lim,
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_ticker_exempted(
    symbol: String,
    exempt: bool,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<(), String> {
    let sym = crate::monitor::canonical_base(&symbol);
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
/// to base quantity on the live book, then dispatches the existing guarded
/// paper execution path.
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
/// gates (executability, stale books, paper-only, blocking signals) apply
/// identically to human and bot flow. Paper-only: the inner spatial path
/// hard-blocks live mode.
pub(crate) async fn execute_monitor_opportunity_impl(
    symbol: &str,
    buy_venue: &str,
    sell_venue: &str,
    size_usd: f64,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    let engine = monitor_engine(state)?;
    let sym = crate::monitor::canonical_base(symbol);
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
    // Convert USD size to base qty at the current buy-side ask.
    let book = state
        .market_data_hub
        .get_valid_orderbook(
            &arbitrageur_core::types::VenueId(buy_venue.to_string()),
            &sym,
            Some(std::time::Duration::from_secs(60)),
        )
        .ok_or_else(|| "buy-side orderbook went stale".to_string())?;
    let ask = book
        .asks
        .first()
        .and_then(|l| l.price.to_string().parse::<f64>().ok())
        .filter(|p| *p > 0.0)
        .ok_or_else(|| "no ask on buy-side book".to_string())?;
    let qty = size_usd / ask;
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
    // Spread Radar page scopes positions separately from Single Spread.
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
        None,
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
