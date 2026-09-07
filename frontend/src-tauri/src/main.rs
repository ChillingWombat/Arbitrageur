#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod state;
mod vault;

use std::sync::Arc;
use state::AppEngineState;
use tauri::Manager;

fn main() {
    #[cfg(target_os = "linux")]
    {
        std::env::set_var("GDK_BACKEND", "x11");
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
        std::env::set_var("__NV_DISABLE_EXPLICIT_SYNC", "1");
    }

    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let engine_state = Arc::new(AppEngineState::new());

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(engine_state)
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_system_status,
            commands::fetch_orderbooks,
            commands::scan_spatial_opportunities,
            commands::scan_spot_opportunities,
            commands::execute_spot_arbitrage,
            commands::scan_funding_opportunities,
            commands::scan_prediction_opportunities,
            commands::run_stat_arb_step,
            commands::compute_optimal_thresholds,
            commands::fit_ou_process,
            commands::run_simulation_step,
            commands::execute_spatial_arbitrage,
            commands::execute_cross_ticker_arbitrage,
            commands::fetch_multi_market_quotes,
            commands::execute_funding_carry,
            commands::execute_prediction_arbitrage,
            commands::get_risk_overview,
            commands::reset_circuit_breaker,
            commands::trigger_emergency_killswitch,
            commands::get_execution_logs,
            commands::set_live_mode,
            commands::get_engine_settings,
            commands::save_engine_settings,
            commands::get_ticker_strategy_config,
            commands::save_ticker_strategy_config,
            commands::get_all_ticker_strategy_configs,
            commands::get_vault_status,
            commands::initialize_vault,
            commands::unlock_vault,
            commands::lock_vault,
            commands::get_exchange_credentials,
            commands::save_exchange_credentials,
            commands::get_venue_balances,
            commands::get_strategy_performance,
            commands::run_wfo_calibration,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Arbitrager Tauri desktop application");
}
