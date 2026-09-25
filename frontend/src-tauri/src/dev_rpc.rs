//! Dev-only JSON bridge over a Unix socket for hands-on testing.
//!
//! Enabled only when `ARBITRAGEUR_DEV_SOCK` names a socket path — a complete
//! no-op otherwise (no listener, no surface in production launches). Each
//! request is one line of `{"id":N,"cmd":"name","args":{...}}`; the reply is
//! `{"id":N,"ok":<value>}` or `{"id":N,"err":"..."}`. Commands dispatch to the
//! exact `*_impl` functions the Tauri IPC handlers wrap, so behavior matches
//! the UI paths bit-for-bit.

use crate::state::AppEngineState;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

pub fn maybe_spawn(state: Arc<AppEngineState>) {
    let Some(path) = std::env::var_os("ARBITRAGEUR_DEV_SOCK") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    let _ = std::fs::remove_file(&path);
    tauri::async_runtime::spawn(async move {
        match UnixListener::bind(&path) {
            Ok(listener) => {
                tracing::info!("dev rpc: listening on {}", path.display());
                loop {
                    match listener.accept().await {
                        Ok((stream, _)) => {
                            let st = state.clone();
                            tauri::async_runtime::spawn(async move {
                                if let Err(e) = handle_conn(stream, st).await {
                                    tracing::warn!("dev rpc: connection error: {e}");
                                }
                            });
                        }
                        Err(e) => {
                            tracing::warn!("dev rpc: accept failed: {e}");
                            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                        }
                    }
                }
            }
            Err(e) => tracing::warn!("dev rpc: bind {} failed: {e}", path.display()),
        }
    });
}

async fn handle_conn(stream: UnixStream, state: Arc<AppEngineState>) -> std::io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await? {
        let resp = dispatch(&line, &state).await;
        let mut buf =
            serde_json::to_vec(&resp).unwrap_or_else(|_| b"{\"err\":\"serialize\"}".to_vec());
        buf.push(b'\n');
        write.write_all(&buf).await?;
    }
    Ok(())
}

fn arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn arg_f64(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(Value::as_f64)
}

fn arg_u32(args: &Value, key: &str) -> Option<u32> {
    args.get(key).and_then(Value::as_u64).map(|v| v as u32)
}

async fn dispatch(line: &str, state: &Arc<AppEngineState>) -> Value {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return json!({"err": format!("bad json: {e}")}),
    };
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let cmd = req.get("cmd").and_then(Value::as_str).unwrap_or("");
    let args = req.get("args").cloned().unwrap_or_else(|| json!({}));
    let st: &AppEngineState = state;

    let out: Result<Value, String> = match cmd {
        // ── vault / env / engine ────────────────────────────────────────────
        "vault_status" => Ok(json!(crate::accounts::get_vault_status_impl(st))),
        "unlock_vault" => {
            let pw = arg(&args, "password").unwrap_or_default().to_string();
            let totp = arg(&args, "totp").map(str::to_string);
            crate::accounts::unlock_vault_impl(&pw, totp.as_deref(), st).map(|v| json!(v))
        }
        "lock_vault" => crate::accounts::lock_vault_impl(st).map(|v| json!(v)),
        "system_status" => crate::commands::get_system_status_impl(st)
            .await
            .map(|v| json!(v)),
        "set_env" => {
            let env = arg(&args, "env").unwrap_or_default();
            match env.to_ascii_lowercase().as_str() {
                "live" => crate::commands::set_live_mode_impl(true, st).map(|v| json!(v)),
                "testnet" => crate::commands::set_live_mode_impl(false, st).map(|v| json!(v)),
                other => Err(format!("unknown env '{other}' — expected testnet|live")),
            }
        }
        "set_paused" => {
            let p = args.get("paused").and_then(Value::as_bool).unwrap_or(false);
            crate::commands::set_engine_paused_impl(p, st).map(|v| json!(v))
        }
        "engine_settings" => crate::commands::get_engine_settings_impl(st).map(|v| json!(v)),
        "risk_overview" => crate::execution::get_risk_overview_impl(st)
            .await
            .map(|v| json!(v)),

        // ── accounts / capabilities / market reads ──────────────────────────
        "list_accounts" => {
            crate::accounts::list_accounts_impl(arg(&args, "venue"), st).map(|v| json!(v))
        }
        "venue_balances" => crate::accounts::get_venue_balances_impl(st)
            .await
            .map(|v| json!(v)),
        "venue_capabilities" => Ok(json!(
            crate::commands::get_venue_execution_capabilities_impl(st)
        )),
        "strategy_capabilities" => Ok(json!(crate::execution::get_strategy_capabilities_impl())),
        "market_universe" => crate::commands::get_market_universe_impl(st)
            .await
            .map(|v| json!(v)),
        "fetch_orderbooks" => {
            let sym = arg(&args, "symbol").unwrap_or_default();
            crate::commands::fetch_orderbooks_impl(sym, st)
                .await
                .map(|v| json!(v))
        }
        "execution_logs" => {
            crate::commands::get_execution_logs_by_mode_impl(st, None).map(|v| json!(v))
        }

        // ── scans ───────────────────────────────────────────────────────────
        "scan_spatial" => {
            let sym = arg(&args, "symbol").unwrap_or_default();
            crate::commands::scan_spatial_opportunities_impl(
                sym,
                arg_f64(&args, "min_bps").unwrap_or(5.0),
                arg_f64(&args, "order_qty").unwrap_or(0.01),
                st,
            )
            .await
            .map(|v| json!(v))
        }
        "scan_spot" => {
            let sym = arg(&args, "symbol").unwrap_or_default();
            crate::commands::scan_spot_opportunities_impl(sym, st)
                .await
                .map(|v| json!(v))
        }
        "scan_funding" => {
            let sym = arg(&args, "symbol").unwrap_or_default();
            crate::funding::scan_funding_opportunities_impl(
                sym,
                arg_f64(&args, "trade_size_usd").unwrap_or(100.0),
                st,
            )
            .await
            .map(|v| json!(v))
        }

        // ── execution ───────────────────────────────────────────────────────
        "execute_spatial" => {
            let sym = arg(&args, "symbol").unwrap_or_default();
            let buy = arg(&args, "buy_venue").unwrap_or_default();
            let sell = arg(&args, "sell_venue").unwrap_or_default();
            crate::execution::execute_spatial_arbitrage_inner(
                sym,
                buy,
                sell,
                arg_f64(&args, "order_qty").unwrap_or(0.0),
                arg(&args, "direction").map(str::to_string),
                arg_f64(&args, "trigger_spread_bps"),
                arg_f64(&args, "baseline_mean_spread_bps"),
                arg_u32(&args, "position_step"),
                arg_u32(&args, "max_units"),
                arg_f64(&args, "nominal_hurdle_bps"),
                None,
                "futures",
                None,
                st,
            )
            .await
            .map(|v| json!(v))
        }
        "execute_funding_carry" => {
            let sym = arg(&args, "symbol").unwrap_or_default();
            let long = arg(&args, "long_venue").unwrap_or_default();
            let short = arg(&args, "short_venue").unwrap_or_default();
            crate::funding::execute_funding_carry_impl(
                sym,
                long,
                short,
                arg_f64(&args, "trade_size_usd").unwrap_or(0.0),
                st,
                args.get("auto").and_then(Value::as_bool).unwrap_or(false),
                arg(&args, "kind"),
            )
            .await
            .map(|v| json!(v))
        }
        "cancel_venue_order" => cancel_order(&args, st).await,
        "flatten_ticker" => {
            let sym = arg(&args, "symbol");
            let strat = arg(&args, "strategy_id").unwrap_or("futures");
            crate::execution::flatten_ticker_positions_impl(
                sym,
                strat,
                arbitrageur_execution::virtual_ledger::CycleStatus::Closed,
                st,
            )
            .await
            .map(|v| json!(v))
        }
        "killswitch" => crate::execution::trigger_emergency_killswitch_impl(st).map(|v| json!(v)),
        "reconcile_orders" => Ok(json!(
            crate::reconciliation::reconcile_pending_orders(st).await
        )),

        // ── positions / signal ledger / bot ─────────────────────────────────
        "open_positions" => open_positions(&args, st),
        "signal_executions" => signal_executions(&args, st),
        "radar_bot_status" => radar_bot_status(st),
        "radar_bot_config" => radar_bot_config(st),
        "set_radar_bot_config" => set_radar_bot_config(&args, st),

        _ => Err(format!("unknown cmd '{cmd}'")),
    };

    match out {
        Ok(v) => json!({"id": id, "ok": v}),
        Err(e) => json!({"id": id, "err": e}),
    }
}

async fn cancel_order(args: &Value, state: &AppEngineState) -> Result<Value, String> {
    // Same body as commands::cancel_venue_order (thin Tauri wrapper).
    if state.is_live() && !state.vault.write().is_unlocked() {
        return Err(
            "Cannot cancel a venue order in Live Trading mode while the secure credential vault is locked. Unlock the vault first."
                .into(),
        );
    }
    let venue = arg(args, "venue").unwrap_or_default().trim().to_lowercase();
    let symbol = arg(args, "symbol").unwrap_or_default().trim().to_string();
    let order_id = arg(args, "order_id").unwrap_or_default().trim().to_string();
    if venue.is_empty() || symbol.is_empty() || order_id.is_empty() {
        return Err("venue, symbol, and order_id are required".into());
    }
    let adapter = state
        .get_adapter(&venue)
        .ok_or_else(|| format!("Venue '{venue}' is not admitted for execution"))?;
    if !adapter.live_order_submission_implemented() || !adapter.has_live_credentials() {
        return Err(format!(
            "Venue '{venue}' does not support live order cancellation or has unauthenticated credentials"
        ));
    }
    let client_order_id = arg(args, "client_order_id")
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_string);
    adapter
        .cancel_order(arbitrageur_core::order::CancelRequest {
            venue: adapter.venue_id(),
            symbol: symbol.clone(),
            order_id: Some(order_id.clone()),
            client_order_id,
        })
        .await
        .map_err(|e| format!("Cancel on {venue} for order {order_id} failed: {e}"))?;
    Ok(json!(format!(
        "Cancel request accepted on {venue} for order {order_id}"
    )))
}

fn open_positions(args: &Value, state: &AppEngineState) -> Result<Value, String> {
    // Same body as execution::get_open_positions.
    let sid = match arg(args, "strategy_id") {
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
        hub.get_valid_orderbook(&arbitrageur_core::types::VenueId::from(venue), symbol, None)
            .and_then(|b| b.mid_price())
            .and_then(|m| crate::execution::dec_to_f64(m, "mark mid").ok())
    });
    for p in &mut positions {
        p.reduce_only = state
            .state_store
            .ticker_reduce_only(&p.symbol, &p.strategy_id, state.env().as_str())
            .map_err(|e| e.to_string())?;
    }
    Ok(json!(positions))
}

fn signal_executions(args: &Value, state: &AppEngineState) -> Result<Value, String> {
    let st = match arg(args, "status") {
        Some(s) => Some(
            crate::state_store::SignalStatus::from_status(s)
                .ok_or_else(|| format!("unknown signal status '{s}'"))?,
        ),
        None => None,
    };
    state
        .state_store
        .list_signals(arg(args, "symbol"), st)
        .map(|v| json!(v))
        .map_err(|e| e.to_string())
}

fn radar_bot_status(state: &AppEngineState) -> Result<Value, String> {
    let bot = state
        .radar_bot
        .lock()
        .clone()
        .ok_or_else(|| "radar bot is not running".to_string())?;
    Ok(json!(bot.status(state)))
}

fn radar_bot_config(state: &AppEngineState) -> Result<Value, String> {
    let (cfg, mode) = crate::auto_exec::OpportunityExecutor::load_config(state);
    Ok(json!(crate::auto_exec::RadarBotConfigDto::from_policy(
        &cfg, mode
    )))
}

fn set_radar_bot_config(args: &Value, state: &AppEngineState) -> Result<Value, String> {
    let cfg: crate::auto_exec::RadarBotConfigDto =
        serde_json::from_value(args.get("config").cloned().unwrap_or_else(|| json!({})))
            .map_err(|e| format!("bad config: {e}"))?;
    crate::auto_exec::OpportunityExecutor::save_config(state, &cfg).map(|_| json!(true))
}
