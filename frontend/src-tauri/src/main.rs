#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod attribution;
mod auto_exec;
mod commands;
mod equity_attribution;
mod external_positions;
mod feed;
mod monitor;
mod num;
mod reconciliation;
mod seasonality;
mod state;
mod state_store;
mod strategy_filters;
mod vault;
mod ws_primary;

use state::AppEngineState;
use std::sync::Arc;
use tauri::Manager;

fn main() {
    // Deterministic TLS: install ring as the process-wide rustls provider
    // before any HTTPS client builds (alloy + reqwest otherwise disagree).
    let _ = rustls::crypto::ring::default_provider().install_default();
    #[cfg(target_os = "linux")]
    {
        // Native Wayland + GPU compositing is the fast path: scrolling
        // repaints on the GPU instead of the CPU. The legacy X11/software
        // fallback stays one env var away (ARBITRAGEUR_X11=1) for drivers
        // where native compositing misbehaves.
        if std::env::var_os("ARBITRAGEUR_X11").is_some() {
            std::env::set_var("GDK_BACKEND", "x11");
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
            std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
            std::env::set_var("__NV_DISABLE_EXPLICIT_SYNC", "1");
        }
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let engine_state = Arc::new(
        AppEngineState::new_persistent().expect("failed to initialize durable application state"),
    );

    // Install and spawn the spread-range monitor engine.
    {
        let monitor = monitor::SpreadMonitorEngine::new(&engine_state);
        *engine_state.monitor.lock() = Some(monitor.clone());
        let bg_state = engine_state.clone();
        tauri::async_runtime::spawn(async move { monitor.run(bg_state).await });
    }

    // Install and spawn the autonomous convergence bot (opt-in, paper-only,
    // inert until explicitly armed via the radar bot config).
    {
        let bot = auto_exec::OpportunityExecutor::new();
        *engine_state.radar_bot.lock() = Some(bot.clone());
        let bg_state = engine_state.clone();
        tauri::async_runtime::spawn(async move { bot.run(bg_state).await });
    }

    // Spawn periodic convergence supervisor for Single Spread ("futures") strategy.
    // Automatically monitors open manual/automated futures positions every 5s and
    // unwinds them upon convergence into the friction band or on timeout/stop.
    {
        let bg_state = engine_state.clone();
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            loop {
                interval.tick().await;
                let _ = commands::check_spatial_convergence_impl(None, "futures", &bg_state).await;
            }
        });
    }

    // Spawn the live-order reconciliation supervisor. Runs an immediate
    // startup pass (restart recovery for orders accepted before shutdown),
    // then polls every 5s until each persisted working order reaches a
    // terminal venue state or escalates to `unresolved` on deadline.
    reconciliation::spawn_supervisor(engine_state.clone());

    // Spawn the WebSocket-first orderbook feed supervisors. REST polling
    // stays as the cold-start/stale fallback; dead WS lanes go stale.
    feed::spawn_all(engine_state.clone());

    // Spawn the scheduled-exit dispatcher: durable `scheduled_exits` rows
    // fire through the normal reduce-only close path at their wall-clock
    // time; the claim transition keeps restarts idempotent.
    external_positions::spawn_exit_scheduler(engine_state.clone());

    tauri::Builder::default()
        .manage(engine_state)
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_system_status,
            commands::refresh_venue_latencies,
            commands::get_market_universe,
            commands::fetch_orderbooks,
            commands::get_ticker_volumes,
            commands::get_signal_series,
            commands::get_matrix_baselines,
            commands::scan_spatial_opportunities,
            commands::scan_triangle_opportunities,
            commands::execute_triangle_arbitrage,
            commands::check_spatial_convergence,
            commands::scan_spot_opportunities,
            commands::execute_spot_arbitrage,
            commands::scan_funding_opportunities,
            commands::get_funding_trend,
            commands::scan_prediction_opportunities,
            commands::run_stat_arb_step,
            commands::compute_optimal_thresholds,
            commands::fit_ou_process,
            commands::execute_spatial_arbitrage,
            commands::execute_cross_ticker_arbitrage,
            commands::fetch_multi_market_quotes,
            commands::execute_funding_carry,
            commands::execute_prediction_arbitrage,
            commands::get_risk_overview,
            commands::reset_circuit_breaker,
            commands::trigger_emergency_killswitch,
            commands::flatten_ticker_positions,
            commands::get_execution_logs,
            commands::get_venue_execution_capabilities,
            commands::set_live_mode,
            commands::set_engine_paused,
            commands::reconcile_orders_now,
            commands::cancel_venue_order,
            commands::acknowledge_signal,
            commands::get_engine_settings,
            commands::save_engine_settings,
            commands::get_ticker_strategy_config,
            commands::save_ticker_strategy_config,
            commands::get_all_ticker_strategy_configs,
            commands::get_strategy_params,
            commands::save_strategy_params,
            commands::get_all_strategy_params,
            commands::get_strategy_tickers,
            commands::get_account_equity_events,
            commands::get_autotune_recommendation,
            commands::apply_autotune_selected,
            commands::run_seasonality_promotion,
            commands::get_vault_status,
            commands::initialize_vault,
            commands::unlock_vault,
            commands::lock_vault,
            commands::delete_exchange_credentials,
            commands::list_accounts,
            commands::upsert_account,
            commands::delete_account,
            commands::delete_account_credentials,
            commands::get_account_credential_field,
            commands::set_default_account,
            commands::list_strategy_routes,
            commands::upsert_strategy_route,
            commands::delete_strategy_route,
            commands::eligible_strategy_routes,
            commands::execute_spatial_candidates,
            commands::list_signal_executions,
            commands::get_ticker_position_state,
            commands::get_open_positions,
            external_positions::get_external_positions,
            external_positions::recommend_external_close,
            external_positions::close_external_position,
            external_positions::schedule_external_exit,
            external_positions::list_scheduled_exits,
            external_positions::cancel_scheduled_exit,
            commands::get_venue_balances,
            commands::get_strategy_performance,
            commands::get_strategy_fill_pnl,
            commands::get_calibration_report,
            commands::get_utilisation_history,
            commands::run_wfo_calibration,
            commands::get_monitor_snapshot,
            commands::get_monitor_config,
            commands::set_monitor_config,
            commands::get_monitor_signal_events,
            commands::get_session_profile,
            commands::set_ticker_exempted,
            commands::get_exempted_tickers,
            commands::reveal_all_tickers,
            commands::execute_monitor_opportunity,
            auto_exec::get_radar_bot_config,
            auto_exec::set_radar_bot_config,
            auto_exec::get_radar_bot_status,
            commands::generate_popdex_agent,
            commands::build_popdex_approve_tx,
            commands::check_popdex_agent,
            commands::sign_popdex_approve_with_main,
            commands::open_external_url,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Arbitrageur Tauri desktop application");
}
