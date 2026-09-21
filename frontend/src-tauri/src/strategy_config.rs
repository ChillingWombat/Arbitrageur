//! Per-ticker strategy configuration and universal per-strategy parameters
//! for Single Spread futures: KV-backed ticker configs (unit sizing,
//! leverage, auto-filters) plus the shared `StrategyParamsDto` records that
//! AutoTune mutates. Split out of `commands.rs`.

use crate::state::{AppEngineState, StrategyParamsDto, TickerStrategyConfigDto};
use std::sync::Arc;
use tauri::State;

pub fn get_ticker_strategy_config_impl(
    symbol: &str,
    state: &AppEngineState,
) -> Result<TickerStrategyConfigDto, String> {
    let configs = state.strategy_configs.read();
    if let Some(cfg) = configs.get(symbol) {
        Ok(cfg.clone())
    } else {
        Ok(TickerStrategyConfigDto {
            symbol: symbol.to_string(),
            ..TickerStrategyConfigDto::default()
        })
    }
}

pub fn save_ticker_strategy_config_impl(
    config: TickerStrategyConfigDto,
    state: &AppEngineState,
) -> Result<(), String> {
    if !config.target_profit_bps.is_finite() || config.target_profit_bps <= 0.0 {
        return Err("target_profit_bps must be finite and positive".into());
    }
    if config.window_percentile < 50 {
        return Err("window_percentile must be at least 50%".into());
    }
    if config.max_accumulated_units < 1 {
        return Err("max_accumulated_units must be at least 1".into());
    }
    if !(1..=10).contains(&config.user_target_leverage) {
        return Err("user_target_leverage must be between 1 and 10".into());
    }
    if !config.unit_size.is_finite() || config.unit_size <= 0.0 {
        return Err("unit_size must be finite and positive".into());
    }
    let mut configs = state.strategy_configs.write();
    configs.insert(config.symbol.clone(), config);
    Ok(())
}

pub fn get_all_ticker_strategy_configs_impl(
    state: &AppEngineState,
) -> Result<Vec<TickerStrategyConfigDto>, String> {
    let configs = state.strategy_configs.read();
    Ok(configs.values().cloned().collect())
}

#[tauri::command]
pub async fn get_ticker_strategy_config(
    symbol: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<TickerStrategyConfigDto, String> {
    get_ticker_strategy_config_impl(&symbol, &state)
}

#[tauri::command]
pub async fn save_ticker_strategy_config(
    config: TickerStrategyConfigDto,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<(), String> {
    save_ticker_strategy_config_impl(config, &state)
}

#[tauri::command]
pub async fn get_all_ticker_strategy_configs(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<TickerStrategyConfigDto>, String> {
    get_all_ticker_strategy_configs_impl(&state)
}

// ── Universal strategy parameters ────────────────────────────────────────────
// One record per strategy_id ("futures" | "funding" | "spot" | "triangle"), shared across
// all tickers. The per-symbol strategy_configs map only retains ticker-specific
// runtime state (accumulated grid units, cached unit size).

const KNOWN_STRATEGY_IDS: [&str; 4] = ["futures", "funding", "spot", "triangle"];

pub fn get_strategy_params_impl(
    strategy_id: &str,
    state: &AppEngineState,
) -> Result<StrategyParamsDto, String> {
    if !KNOWN_STRATEGY_IDS.contains(&strategy_id) {
        return Err(format!("unknown strategy_id '{strategy_id}'"));
    }
    let params = state.strategy_params.read();
    Ok(params
        .get(strategy_id)
        .cloned()
        .unwrap_or_else(|| StrategyParamsDto::defaults_for(strategy_id)))
}

pub(crate) fn canonical_sliding_window(secs: u64) -> String {
    if secs.is_multiple_of(86_400) {
        format!("{}d", secs / 86_400)
    } else if secs.is_multiple_of(3_600) {
        format!("{}h", secs / 3_600)
    } else if secs.is_multiple_of(60) {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

pub fn save_strategy_params_impl(
    mut params: StrategyParamsDto,
    state: &AppEngineState,
) -> Result<(), String> {
    if !KNOWN_STRATEGY_IDS.contains(&params.strategy_id.as_str()) {
        return Err(format!("unknown strategy_id '{}'", params.strategy_id));
    }
    if !matches!(params.sliding_window_mode.as_str(), "manual" | "auto") {
        return Err(format!(
            "sliding_window_mode must be 'manual' or 'auto', got '{}'",
            params.sliding_window_mode
        ));
    }
    if !(300..=2_592_000).contains(&params.sliding_window_secs) {
        return Err(format!(
            "sliding_window_secs must be within 300..=2592000 (5m-30d), got {}",
            params.sliding_window_secs
        ));
    }
    params.sliding_window = canonical_sliding_window(params.sliding_window_secs);
    if !params.target_profit_bps.is_finite() || params.target_profit_bps <= 0.0 {
        return Err("target_profit_bps must be finite and positive".into());
    }
    if params.window_percentile < 50 {
        return Err("window_percentile must be at least 50%".into());
    }
    if !(1..=10).contains(&params.user_target_leverage) {
        return Err("user_target_leverage must be between 1 and 10".into());
    }
    if !params.step_size_mult.is_finite() || params.step_size_mult < 1.0 {
        return Err("step_size_mult must be finite and >= 1".into());
    }
    if !params.max_step_usd.is_finite() || params.max_step_usd <= 0.0 {
        return Err("max_step_usd must be finite and positive".into());
    }
    if !params.max_ticker_equity_pct.is_finite()
        || params.max_ticker_equity_pct <= 0.0
        || params.max_ticker_equity_pct > 100.0
    {
        return Err("max_ticker_equity_pct must be finite and within (0, 100]".into());
    }
    if !params.funding_trade_size_usd.is_finite() || params.funding_trade_size_usd <= 0.0 {
        return Err("funding_trade_size_usd must be finite and positive".into());
    }
    if !params.funding_min_apr_pct.is_finite() || params.funding_min_apr_pct < 0.0 {
        return Err("funding_min_apr_pct must be finite and non-negative".into());
    }
    if params.funding_holding_days < 1 {
        return Err("funding_holding_days must be at least 1".into());
    }
    if !params.spot_trade_size_usd.is_finite() || params.spot_trade_size_usd <= 0.0 {
        return Err("spot_trade_size_usd must be finite and positive".into());
    }
    if !params.spot_max_slippage_bps.is_finite() || params.spot_max_slippage_bps <= 0.0 {
        return Err("spot_max_slippage_bps must be finite and positive".into());
    }
    for (label, value) in [
        ("futures_max_slippage_bps", params.futures_max_slippage_bps),
        ("funding_max_slippage_bps", params.funding_max_slippage_bps),
        (
            "prediction_max_slippage_bps",
            params.prediction_max_slippage_bps,
        ),
    ] {
        if !value.is_finite() || value <= 0.0 {
            return Err(format!("{label} must be finite and positive"));
        }
    }
    if !params.funding_max_entry_spread_bps.is_finite()
        || params.funding_max_entry_spread_bps <= 0.0
    {
        return Err("funding_max_entry_spread_bps must be finite and positive".into());
    }
    for (label, value) in [
        (
            "funding_auto_exit_spread_pct",
            params.funding_auto_exit_spread_pct,
        ),
        ("funding_liq_trim_pct", params.funding_liq_trim_pct),
        ("futures_liq_trim_pct", params.futures_liq_trim_pct),
        ("spot_liq_trim_pct", params.spot_liq_trim_pct),
        ("prediction_liq_trim_pct", params.prediction_liq_trim_pct),
    ] {
        if let Some(pct) = value {
            if !pct.is_finite() || pct <= 0.0 || pct > 100.0 {
                return Err(format!("{label} must be within (0, 100] when set"));
            }
        }
    }
    if let Some(threshold) = params.funding_rotation_threshold_bps {
        if !threshold.is_finite() || threshold <= 0.0 {
            return Err(
                "funding_rotation_threshold_bps must be finite and positive when set".into(),
            );
        }
    }
    if let Some(floor) = params.funding_min_basis_bps {
        if !floor.is_finite() {
            return Err("funding_min_basis_bps must be finite when set".into());
        }
    }
    for (label, cfg) in [
        ("futures_passive_unwind", &params.futures_passive_unwind),
        ("funding_passive_unwind", &params.funding_passive_unwind),
        ("spot_passive_unwind", &params.spot_passive_unwind),
    ] {
        cfg.validate().map_err(|e| format!("{label}: {e}"))?;
        if cfg.enabled && cfg.timeout_secs < 1 {
            return Err(format!(
                "{label}: timeout_secs must be at least 1 when enabled"
            ));
        }
    }
    for (label, mode) in [
        ("futures_passive_mode", params.futures_passive_mode.as_str()),
        ("funding_passive_mode", params.funding_passive_mode.as_str()),
        ("spot_passive_mode", params.spot_passive_mode.as_str()),
    ] {
        if !matches!(mode.trim(), "" | "inherit" | "custom" | "off") {
            return Err(format!(
                "{label} must be 'inherit', 'custom' or 'off' (got '{mode}')"
            ));
        }
    }
    if !params.spot_min_net_spread_bps.is_finite() || params.spot_min_net_spread_bps <= 0.0 {
        return Err("spot_min_net_spread_bps must be finite and positive".into());
    }
    if !params.triangle_trade_size_usd.is_finite() || params.triangle_trade_size_usd <= 0.0 {
        return Err("triangle_trade_size_usd must be finite and positive".into());
    }
    if !params.triangle_min_profit_bps.is_finite() || params.triangle_min_profit_bps <= 0.0 {
        return Err("triangle_min_profit_bps must be finite and positive".into());
    }
    if !params.triangle_max_slippage_bps.is_finite() || params.triangle_max_slippage_bps <= 0.0 {
        return Err("triangle_max_slippage_bps must be finite and positive".into());
    }
    crate::strategy_filters::validate_auto_threshold(
        params.auto_min_volume_usd,
        "auto_min_volume_usd",
    )?;
    crate::strategy_filters::validate_auto_threshold(
        params.auto_min_price_usd,
        "auto_min_price_usd",
    )?;
    crate::strategy_filters::validate_auto_threshold(
        params.auto_min_event_volume_usd,
        "auto_min_event_volume_usd",
    )?;
    let params_json = serde_json::to_string(&params).map_err(|e| e.to_string())?;
    state
        .state_store
        .upsert_strategy_params_json(&params.strategy_id, &params_json)
        .map_err(|e| e.to_string())?;
    let mut map = state.strategy_params.write();
    map.insert(params.strategy_id.clone(), params);
    Ok(())
}

pub fn get_all_strategy_params_impl(
    state: &AppEngineState,
) -> Result<Vec<StrategyParamsDto>, String> {
    let map = state.strategy_params.read();
    Ok(KNOWN_STRATEGY_IDS
        .iter()
        .map(|id| {
            map.get(*id)
                .cloned()
                .unwrap_or_else(|| StrategyParamsDto::defaults_for(id))
        })
        .collect())
}

#[tauri::command]
pub async fn get_strategy_params(
    strategy_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<StrategyParamsDto, String> {
    get_strategy_params_impl(&strategy_id, &state)
}

#[tauri::command]
pub async fn save_strategy_params(
    params: StrategyParamsDto,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<(), String> {
    save_strategy_params_impl(params, &state)
}

#[tauri::command]
pub async fn get_all_strategy_params(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<StrategyParamsDto>, String> {
    get_all_strategy_params_impl(&state)
}
