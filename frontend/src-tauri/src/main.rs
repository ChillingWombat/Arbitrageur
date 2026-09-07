#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod state;

use std::sync::Arc;
use state::AppEngineState;

fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let engine_state = Arc::new(AppEngineState::new());

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(engine_state)
        .invoke_handler(tauri::generate_handler![
            commands::get_system_status,
            commands::fetch_orderbooks,
            commands::scan_spatial_opportunities,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running Arbitrager Tauri desktop application");
}
