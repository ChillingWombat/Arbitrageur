//! Per-ticker strategy configuration and universal per-strategy parameters
//! for Simple Spread futures: KV-backed ticker configs (unit sizing,
//! leverage, auto-filters) plus the shared `StrategyParamsDto` records that
//! AutoTune mutates. Split out of `commands.rs`.

use crate::state::{AppEngineState, StrategyParamsDto, TickerStrategyConfigDto};
use chrono::Utc;
use serde::{Deserialize, Serialize};
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
    crate::commands::validate_market_symbol(&config.symbol)?;
    if !config.target_profit_bps.is_finite()
        || config.target_profit_bps <= 0.0
        || config.target_profit_bps > 10_000.0
    {
        return Err("target_profit_bps must be finite, positive and at most 10000".into());
    }
    if config.window_percentile < 50 {
        return Err("window_percentile must be at least 50%".into());
    }
    if !(1..=1_000_000).contains(&config.max_accumulated_units) {
        return Err("max_accumulated_units must be between 1 and 1000000".into());
    }
    if !(1..=10).contains(&config.user_target_leverage) {
        return Err("user_target_leverage must be between 1 and 10".into());
    }
    if !config.unit_size.is_finite() || config.unit_size <= 0.0 || config.unit_size > 1_000_000.0 {
        return Err("unit_size must be finite, positive and at most 1000000".into());
    }
    // Persist alongside the in-memory map so caps survive restarts — the
    // grid-cap gate reads this record at entry time. A failed kv write is
    // fatal: silently losing the cap would leave the gate weaker than the
    // operator configured.
    let json = serde_json::to_string(&config)
        .map_err(|e| format!("ticker config serialize failed: {e}"))?;
    state
        .state_store
        .kv_set(&format!("ticker_config.{}", config.symbol), &json)
        .map_err(|e| format!("ticker config persist failed: {e}"))?;
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
    if !KNOWN_STRATEGY_IDS.contains(&strategy_id) && strategy_id != "radar" {
        return Err(format!("unknown strategy_id '{strategy_id}'"));
    }
    let params = state.strategy_params.read();
    if let Some(p) = params.get(strategy_id) {
        return Ok(p.clone());
    }
    if strategy_id == "radar" {
        if let Some(mut p) = params.get("futures").cloned() {
            p.strategy_id = "radar".to_string();
            return Ok(p);
        }
    }
    Ok(StrategyParamsDto::defaults_for(strategy_id))
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
    if !KNOWN_STRATEGY_IDS.contains(&params.strategy_id.as_str()) && params.strategy_id != "radar" {
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

// ---------------------------------------------------------------------------
// Curated strategy presets (issue #8): a static, shipped table of
// Conservative / Balanced / Aggressive postures for the futures ticker
// configurator, plus the provenance audit write that follows a one-click
// apply. These are deliberate hand-picked values — NOT data-derived
// recommendations; the desk's evidence bar for publishing recommendations
// is documented in docs/PARAMETER_GUIDE.md. Every field stays inside the
// manual save validators, and `user_target_leverage` is clamped to the
// operator's global `max_leverage` ceiling at apply time.
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StrategyPresetDto {
    pub id: String,
    pub label: String,
    /// Intended regime + risk posture, surfaced verbatim in the UI so the
    /// choice is deliberate rather than by name alone.
    pub description: String,
    /// Table revision — recorded in the audit row so later history can
    /// distinguish which parameter set was applied.
    pub version: u32,
    pub target_profit_bps: f64,
    pub window_percentile: u32,
    pub sliding_window_secs: u64,
    pub execution_mode: String,
    pub step_size_mult: f64,
    pub max_step_usd: f64,
    pub max_ticker_equity_pct: f64,
    pub user_target_leverage: u32,
}

pub const STRATEGY_PRESET_VERSION: u32 = 2;

/// The curated preset table. Ordering is the UI order: safest first.
/// Posture ladder: Conservative takes only deep, rare dislocations with
/// small steps and low leverage; Aggressive trades a wider net at double
/// the step size — still inside every manual-save bound.
pub fn strategy_presets() -> Vec<StrategyPresetDto> {
    vec![
        StrategyPresetDto {
            id: "conservative".into(),
            label: "Conservative".into(),
            description: "Quiet-regime posture: only deep, rare dislocations (90th percentile) clear the hurdle. Small steps, 10% equity cap, 2x leverage — sized so a bad print costs little.".into(),
            version: STRATEGY_PRESET_VERSION,
            target_profit_bps: 8.0,
            window_percentile: 90,
            sliding_window_secs: 21_600, // 6h
            execution_mode: "SINGLE_WAY".into(),
            step_size_mult: 5.0,
            max_step_usd: 1_000.0,
            max_ticker_equity_pct: 10.0,
            user_target_leverage: 2,
        },
        StrategyPresetDto {
            id: "balanced".into(),
            label: "Balanced".into(),
            description: "Normal-regime posture: elevated 5.0 bps hurdle to clear DEX taker friction, 80th-percentile window, 20% equity cap, 5x leverage.".into(),
            version: STRATEGY_PRESET_VERSION,
            target_profit_bps: 5.0,
            window_percentile: 80,
            sliding_window_secs: 43_200, // 12h
            execution_mode: "SINGLE_WAY".into(),
            step_size_mult: 10.0,
            max_step_usd: 2_500.0,
            max_ticker_equity_pct: 20.0,
            user_target_leverage: 5,
        },
        StrategyPresetDto {
            id: "aggressive".into(),
            label: "Aggressive".into(),
            description: "Opportunity-rich posture: 4.0 bps hurdle to ensure positive net capture after taker fees, 70th-percentile window, double-way entries, 8x leverage.".into(),
            version: STRATEGY_PRESET_VERSION,
            target_profit_bps: 4.0,
            window_percentile: 70,
            sliding_window_secs: 86_400, // 24h
            execution_mode: "DOUBLE_WAY".into(),
            step_size_mult: 15.0,
            max_step_usd: 5_000.0,
            max_ticker_equity_pct: 30.0,
            user_target_leverage: 8,
        },
    ]
}

pub fn get_strategy_presets_impl() -> Vec<StrategyPresetDto> {
    strategy_presets()
}

/// Provenance write after a preset apply: reads the *persisted* params +
/// ticker config (never the client's payload) and appends an audit row
/// naming the preset + version, so the trail reflects what actually saved.
/// Runs after the two save calls succeed; a missing/late row is surfaced
/// to the caller, not swallowed.
pub fn record_preset_applied_impl(
    symbol: &str,
    preset_id: &str,
    preset_version: u32,
    state: &AppEngineState,
) -> Result<i64, String> {
    crate::commands::validate_market_symbol(symbol)?;
    let preset = strategy_presets()
        .into_iter()
        .find(|p| p.id == preset_id)
        .ok_or_else(|| format!("unknown strategy preset '{preset_id}'"))?;
    if preset_version != preset.version {
        return Err(format!(
            "preset '{}' version mismatch (table v{}, caller v{})",
            preset_id, preset.version, preset_version
        ));
    }
    let params = get_strategy_params_impl("futures", state)?;
    let ticker = get_ticker_strategy_config_impl(symbol, state)?;
    let applied_json = serde_json::json!({
        "targetProfitBps": params.target_profit_bps,
        "windowPercentile": params.window_percentile,
        "slidingWindowSecs": params.sliding_window_secs,
        "executionMode": params.execution_mode,
        "stepSizeMult": params.step_size_mult,
        "maxStepUsd": params.max_step_usd,
        "maxTickerEquityPct": params.max_ticker_equity_pct,
        "userTargetLeverage": params.user_target_leverage,
        "ticker": {
            "targetProfitBps": ticker.target_profit_bps,
            "windowPercentile": ticker.window_percentile,
            "slidingWindow": ticker.sliding_window,
            "unitSize": ticker.unit_size,
            "maxAccumulatedUnits": ticker.max_accumulated_units,
            "executionMode": ticker.execution_mode,
            "userTargetLeverage": ticker.user_target_leverage,
        },
    })
    .to_string();
    let detail_json = serde_json::json!({
        "source": "preset",
        "preset_id": preset.id,
        "preset_version": preset.version,
        "preset_label": preset.label,
    })
    .to_string();
    state
        .state_store
        .record_autotune_apply_event(
            symbol,
            &applied_json,
            &detail_json,
            Utc::now().timestamp_millis(),
            state.env().as_str(),
        )
        .map_err(|e| format!("preset audit write failed: {e}"))
}

#[tauri::command]
pub async fn get_strategy_presets() -> Result<Vec<StrategyPresetDto>, String> {
    Ok(get_strategy_presets_impl())
}

#[tauri::command]
pub async fn record_preset_applied(
    symbol: String,
    preset_id: String,
    preset_version: u32,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<i64, String> {
    record_preset_applied_impl(&symbol, &preset_id, preset_version, &state)
}

#[cfg(test)]
mod preset_tests {
    use super::*;

    /// Apply a preset's fields onto the futures params defaults — the same
    /// merge the modal performs — then run it through the real save path.
    fn apply_preset_to_state(p: &StrategyPresetDto, state: &AppEngineState) {
        let mut params = get_strategy_params_impl("futures", state).expect("futures params");
        params.target_profit_bps = p.target_profit_bps;
        params.window_percentile = p.window_percentile;
        params.sliding_window_secs = p.sliding_window_secs;
        params.sliding_window_mode = "manual".into();
        params.execution_mode = p.execution_mode.clone();
        params.step_size_mult = p.step_size_mult;
        params.max_step_usd = p.max_step_usd;
        params.max_ticker_equity_pct = p.max_ticker_equity_pct;
        params.user_target_leverage = p.user_target_leverage;
        save_strategy_params_impl(params, state).expect("preset passes manual-save validation");
        let window = canonical_sliding_window(p.sliding_window_secs);
        let mut ticker =
            get_ticker_strategy_config_impl("BTC-USD", state).expect("ticker config loads");
        ticker.target_profit_bps = p.target_profit_bps;
        ticker.window_percentile = p.window_percentile;
        ticker.sliding_window = window.clone();
        ticker.execution_mode = p.execution_mode.clone();
        ticker.user_target_leverage = p.user_target_leverage;
        save_ticker_strategy_config_impl(ticker, state).expect("ticker config saves");
    }

    #[test]
    fn presets_are_three_ordered_and_versioned() {
        let presets = get_strategy_presets_impl();
        assert_eq!(presets.len(), 3);
        let ids: Vec<&str> = presets.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["conservative", "balanced", "aggressive"]);
        for p in &presets {
            assert_eq!(p.version, STRATEGY_PRESET_VERSION);
            assert!(!p.description.is_empty(), "{} needs a regime note", p.id);
        }
        // Posture ladder is monotone: hurdle falls, risk rises.
        assert!(presets[0].target_profit_bps > presets[1].target_profit_bps);
        assert!(presets[1].target_profit_bps > presets[2].target_profit_bps);
        assert!(presets[0].user_target_leverage < presets[2].user_target_leverage);
        assert!(presets[0].max_ticker_equity_pct < presets[2].max_ticker_equity_pct);
    }

    #[test]
    fn every_preset_passes_manual_save_bounds() {
        // The apply path writes through the ordinary validators — a preset
        // that violates them would dead-end in the UI, so prove all three
        // are saveable here.
        for p in get_strategy_presets_impl() {
            let state = AppEngineState::new();
            apply_preset_to_state(&p, &state);
            let saved = get_strategy_params_impl("futures", &state).expect("params saved");
            assert_eq!(saved.target_profit_bps, p.target_profit_bps);
            assert_eq!(saved.user_target_leverage, p.user_target_leverage);
        }
    }

    #[test]
    fn preset_audit_rejects_unknown_id_and_version_drift() {
        let state = AppEngineState::new();
        let err = record_preset_applied_impl("BTC-USD", "yolo", STRATEGY_PRESET_VERSION, &state)
            .unwrap_err();
        assert!(err.contains("unknown strategy preset"), "{err}");
        let err = record_preset_applied_impl("BTC-USD", "balanced", 999, &state).unwrap_err();
        assert!(err.contains("version mismatch"), "{err}");
        let err =
            record_preset_applied_impl("BTC USD!", "balanced", STRATEGY_PRESET_VERSION, &state)
                .unwrap_err();
        assert!(err.contains("Invalid market symbol"), "{err}");
    }

    #[test]
    fn preset_audit_records_persisted_values_and_provenance() {
        let state = AppEngineState::new();
        let preset = get_strategy_presets_impl()
            .into_iter()
            .find(|p| p.id == "conservative")
            .expect("conservative preset");
        apply_preset_to_state(&preset, &state);
        let id = record_preset_applied_impl("BTC-USD", "conservative", preset.version, &state)
            .expect("audit row writes");
        assert!(id > 0);
        let rows = state
            .state_store
            .recent_autotune_apply_events("BTC-USD", 10, state.env().as_str())
            .expect("events query");
        let row = rows.iter().find(|r| r.id == id).expect("row readable");
        let detail: serde_json::Value = serde_json::from_str(&row.detail_json).unwrap();
        assert_eq!(detail["source"], "preset");
        assert_eq!(detail["preset_id"], "conservative");
        assert_eq!(detail["preset_version"], preset.version);
        // The audit reflects the persisted record, not a client payload.
        let applied: serde_json::Value = serde_json::from_str(&row.applied_json).unwrap();
        assert_eq!(applied["targetProfitBps"], preset.target_profit_bps);
        assert_eq!(
            applied["userTargetLeverage"],
            preset.user_target_leverage as u64
        );
        assert_eq!(applied["ticker"]["executionMode"], preset.execution_mode);
    }

    #[test]
    fn ticker_config_rejects_invalid_symbol_and_unbounded_inputs() {
        let state = AppEngineState::new();
        let mut cfg = TickerStrategyConfigDto {
            symbol: "BTC-USD".into(),
            ..TickerStrategyConfigDto::default()
        };
        assert!(save_ticker_strategy_config_impl(cfg.clone(), &state).is_ok());

        cfg.symbol = "BTC USD!".into();
        assert!(save_ticker_strategy_config_impl(cfg.clone(), &state).is_err());

        cfg.symbol = "BTC-USD".into();
        cfg.unit_size = 1_000_001.0;
        assert!(save_ticker_strategy_config_impl(cfg.clone(), &state).is_err());

        cfg.unit_size = 1.0;
        cfg.max_accumulated_units = 1_000_001;
        assert!(save_ticker_strategy_config_impl(cfg.clone(), &state).is_err());

        cfg.max_accumulated_units = 10;
        cfg.target_profit_bps = 10_001.0;
        assert!(save_ticker_strategy_config_impl(cfg, &state).is_err());
    }
}
