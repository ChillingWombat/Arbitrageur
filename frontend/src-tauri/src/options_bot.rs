//! Autonomous options substrategy bot — the Options page's counterpart to
//! the radar `OpportunityExecutor`. One persisted config row per
//! (environment, substrategy) so each substrategy's bot arms, pauses and
//! drains independently, and a testnet bot never implies a live one.
//!
//! Live substrategies: `parity` plus the three Deribit package strategies
//! (box spread, jelly roll, convexity). While armed in `auto` the loop
//! scans every reachable underlying each poll and dispatches executable
//! setups, each entry re-validated fresh by its execute impl (which
//! refuses stale edges, a paused engine, a tripped circuit breaker, and —
//! via `auto` — autonomous entries on live). Options positions ride to
//! expiry, so `paused`/`reduce_only` simply freeze new entries; a
//! `reduce_only` book that reaches flat retires the persisted mode to
//! `off`, matching the radar bot's drain-down contract.

use std::cmp::Ordering;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use arbitrageur_strategy::convergence_auto::BotMode;

use crate::commands::OPTIONS_PARITY_STRATEGY_ID;
use crate::options_packages::{
    execute_options_package_impl, package_strategy_id, scan_options_packages_impl,
    OptionsPackageExecutionParams, OptionsPackageScanParams, LIVE_PACKAGE_SUBSTRATEGIES,
};
use crate::options_parity::{
    execute_options_parity_impl, scan_options_parity_impl, OptionsParityExecutionParams,
    OptionsParityScanParams,
};
use crate::state::AppEngineState;
use crate::state_store::SignalStatus;

/// Substrategies with a working scanner. Pending catalog entries never
/// reach the loop — their rail rows stay honest placeholders.
pub const LIVE_SUBSTRATEGIES: [&str; 4] = ["parity", "box_spread", "jelly_roll", "convexity"];
const SCAN_CURRENCIES: [&str; 2] = ["BTC", "ETH"];

const DEFAULT_QTY_BASE: f64 = 0.1;
const DEFAULT_MIN_EDGE_BPS: f64 = 5.0;
const DEFAULT_MIN_HOURS_TO_EXPIRY: f64 = 48.0;
const DEFAULT_MAX_DAYS_TO_EXPIRY: f64 = 60.0;
const DEFAULT_POLL_SECS: u64 = 30;
const DEFAULT_MAX_OPEN: u32 = 2;

/// Validated bot policy derived from the persisted DTO.
#[derive(Debug, Clone)]
pub struct OptionsBotPolicy {
    pub qty_base: f64,
    pub min_edge_bps: f64,
    pub min_hours_to_expiry: f64,
    pub max_days_to_expiry: f64,
    pub poll_interval_secs: u64,
    pub max_open_positions: usize,
}

impl Default for OptionsBotPolicy {
    fn default() -> Self {
        Self {
            qty_base: DEFAULT_QTY_BASE,
            min_edge_bps: DEFAULT_MIN_EDGE_BPS,
            min_hours_to_expiry: DEFAULT_MIN_HOURS_TO_EXPIRY,
            max_days_to_expiry: DEFAULT_MAX_DAYS_TO_EXPIRY,
            poll_interval_secs: DEFAULT_POLL_SECS,
            max_open_positions: DEFAULT_MAX_OPEN as usize,
        }
    }
}

/// Persisted per-substrategy bot config — camelCase to match every other
/// bot-config DTO the frontend consumes.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OptionsBotConfigDto {
    pub substrategy_id: String,
    pub enabled: bool,
    pub mode: String,
    pub qty_base: f64,
    pub min_edge_bps: f64,
    pub min_hours_to_expiry: f64,
    pub max_days_to_expiry: f64,
    pub poll_interval_secs: u64,
    pub max_open_positions: u32,
}

impl OptionsBotConfigDto {
    /// DTO → validated policy + parsed mode. Rejects unknown modes, NaN
    /// bounds and out-of-range values — a rejected payload never persists.
    pub fn to_policy(&self) -> Result<(OptionsBotPolicy, BotMode), String> {
        let mode = BotMode::parse_mode(&self.mode)
            .ok_or_else(|| format!("unknown options bot mode '{}'", self.mode))?;
        let finite = |v: f64| v.is_finite();
        if !finite(self.qty_base) || self.qty_base <= 0.0 || self.qty_base > 10.0 {
            return Err("qtyBase must be a finite number in (0, 10]".into());
        }
        if !finite(self.min_edge_bps) || !(0.0..=10_000.0).contains(&self.min_edge_bps) {
            return Err("minEdgeBps must be within [0, 10000]".into());
        }
        if !finite(self.min_hours_to_expiry) || !(0.0..=8_760.0).contains(&self.min_hours_to_expiry)
        {
            return Err("minHoursToExpiry must be within [0, 8760]".into());
        }
        if !finite(self.max_days_to_expiry)
            || self.max_days_to_expiry <= 0.0
            || self.max_days_to_expiry > 366.0
        {
            return Err("maxDaysToExpiry must be within (0, 366]".into());
        }
        if self.min_hours_to_expiry >= self.max_days_to_expiry * 24.0 {
            return Err("expiry window is empty — minHours must precede maxDays".into());
        }
        if !(5..=3_600).contains(&self.poll_interval_secs) {
            return Err("pollIntervalSecs must be within [5, 3600]".into());
        }
        if !(1..=10).contains(&self.max_open_positions) {
            return Err("maxOpenPositions must be within [1, 10]".into());
        }
        Ok((
            OptionsBotPolicy {
                qty_base: self.qty_base,
                min_edge_bps: self.min_edge_bps,
                min_hours_to_expiry: self.min_hours_to_expiry,
                max_days_to_expiry: self.max_days_to_expiry,
                poll_interval_secs: self.poll_interval_secs,
                max_open_positions: self.max_open_positions as usize,
            },
            mode,
        ))
    }

    pub fn from_policy(substrategy_id: &str, cfg: &OptionsBotPolicy, mode: BotMode) -> Self {
        Self {
            substrategy_id: substrategy_id.to_string(),
            enabled: mode == BotMode::Auto,
            mode: mode.as_str().to_string(),
            qty_base: cfg.qty_base,
            min_edge_bps: cfg.min_edge_bps,
            min_hours_to_expiry: cfg.min_hours_to_expiry,
            max_days_to_expiry: cfg.max_days_to_expiry,
            poll_interval_secs: cfg.poll_interval_secs,
            max_open_positions: cfg.max_open_positions as u32,
        }
    }
}

pub struct OptionsBotExecutor {
    shutdown: Mutex<bool>,
}

impl OptionsBotExecutor {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            shutdown: Mutex::new(false),
        })
    }

    #[allow(dead_code)]
    pub fn shutdown(&self) {
        *self.shutdown.lock() = true;
    }

    /// Reload the active environment's persisted config for a substrategy;
    /// any failure (missing row, bad JSON, failed validation) resolves to
    /// inert — never to permissive.
    pub fn load_config(
        state: &AppEngineState,
        substrategy_id: &str,
    ) -> (OptionsBotPolicy, BotMode) {
        let raw = state
            .state_store
            .options_bot_config_json(state.env().as_str(), substrategy_id)
            .ok()
            .flatten();
        let Some(json) = raw else {
            return (OptionsBotPolicy::default(), BotMode::Off);
        };
        let dto: OptionsBotConfigDto = match serde_json::from_str(&json) {
            Ok(d) => d,
            Err(_) => return (OptionsBotPolicy::default(), BotMode::Off),
        };
        dto.to_policy()
            .unwrap_or((OptionsBotPolicy::default(), BotMode::Off))
    }

    pub fn save_config(state: &AppEngineState, dto: &OptionsBotConfigDto) -> Result<(), String> {
        if !LIVE_SUBSTRATEGIES.contains(&dto.substrategy_id.as_str()) {
            return Err(format!(
                "no options bot exists for substrategy '{}' — it is still a pending scanner",
                dto.substrategy_id
            ));
        }
        let (policy, mode) = dto.to_policy()?;
        // Autonomous entries are testnet-only — the execute impl enforces it
        // at dispatch, but refuse the arming write here so the operator gets
        // the error at click time, not as silent failed ticks.
        if state.is_live() && mode == BotMode::Auto {
            return Err(
                "autonomous options entries are testnet-only — run a manual Execute for live"
                    .into(),
            );
        }
        let canonical = OptionsBotConfigDto::from_policy(&dto.substrategy_id, &policy, mode);
        let json = serde_json::to_string(&canonical).map_err(|e| format!("config encode: {e}"))?;
        state
            .state_store
            .save_options_bot_config_json(state.env().as_str(), &dto.substrategy_id, &json)
            .map_err(|e| format!("config persist: {e}"))?;
        Ok(())
    }

    /// Non-terminal *cycles* for one substrategy in the active environment —
    /// the bot's open-position book. One execution writes N leg signals
    /// sharing the `{cycle}-legN` prefix, so rows are counted by distinct
    /// cycle, not individually. `None` when the ledger is unreadable: an
    /// uncountable book fails closed (no entries, no retire).
    fn open_positions(state: &AppEngineState, strategy_id: &str) -> Option<usize> {
        let env = state.env().as_str();
        state.state_store.list_signals(None, None).ok().map(|sigs| {
            sigs.iter()
                .filter(|s| {
                    s.environment.eq_ignore_ascii_case(env)
                        && s.strategy_id.eq_ignore_ascii_case(strategy_id)
                        && !matches!(s.status, SignalStatus::Closed | SignalStatus::Failed)
                })
                .map(|s| {
                    s.id.rsplit_once("-leg")
                        .filter(|(_, tail)| !tail.is_empty())
                        .map(|(prefix, _)| prefix)
                        .unwrap_or(s.id.as_str())
                        .to_string()
                })
                .collect::<std::collections::HashSet<_>>()
                .len()
        })
    }

    pub async fn run(self: Arc<Self>, state: Arc<AppEngineState>) {
        loop {
            if *self.shutdown.lock() {
                return;
            }
            let secs = self.tick_once(&state).await;
            tokio::time::sleep(Duration::from_secs(secs)).await;
        }
    }

    /// One poll over every live substrategy. Returns the next delay —
    /// driven by the armed substrategy's own poll interval.
    pub async fn tick_once(&self, state: &AppEngineState) -> u64 {
        let mut delay = DEFAULT_POLL_SECS;
        for substrategy in LIVE_SUBSTRATEGIES {
            delay = delay.min(self.tick_substrategy(state, substrategy).await);
        }
        delay.max(5)
    }

    /// The signal-ledger strategy id one substrategy's executions write —
    /// `options_parity` for parity, `options_{sub}` for packages.
    fn ledger_strategy_id(substrategy: &str) -> String {
        if substrategy == "parity" {
            OPTIONS_PARITY_STRATEGY_ID.to_string()
        } else {
            package_strategy_id(substrategy)
        }
    }

    async fn tick_substrategy(&self, state: &AppEngineState, substrategy: &str) -> u64 {
        let (cfg, mode) = Self::load_config(state, substrategy);
        let poll = cfg.poll_interval_secs.max(5);
        if !mode.exits_allowed() {
            return poll;
        }
        let strategy_id = Self::ledger_strategy_id(substrategy);
        let Some(mut open) = Self::open_positions(state, &strategy_id) else {
            // Ledger unreadable — cannot prove the book is flat; hold mode,
            // skip entries.
            return poll;
        };
        // Flat-book retire — mirrors the radar bot: a reduce_only mode with
        // nothing left to drain drops the persisted config to off.
        if mode == BotMode::ReduceOnly && open == 0 {
            let (_, mode_now) = Self::load_config(state, substrategy);
            if mode_now == BotMode::ReduceOnly {
                let next = OptionsBotConfigDto::from_policy(substrategy, &cfg, BotMode::Off);
                if let Err(e) = Self::save_config(state, &next) {
                    tracing::warn!("options bot {substrategy}: flat-book retire failed: {e}");
                }
            }
            return poll;
        }
        if !mode.entries_allowed() || open >= cfg.max_open_positions {
            return poll;
        }

        // Fresh scan per underlying; each execute re-scans server-side, so a
        // stale edge is refused by the impl, never here.
        for ccy in SCAN_CURRENCIES {
            // (id, currency, net_edge_usd) — the two scanners' DTOs project
            // onto one shape for sorting + dispatch.
            let opps: Vec<(String, String, f64)> = if substrategy == "parity" {
                match scan_options_parity_impl(
                    &OptionsParityScanParams {
                        currency: ccy.to_string(),
                        qty_base: Some(cfg.qty_base),
                        min_hours_to_expiry: Some(cfg.min_hours_to_expiry),
                        max_days_to_expiry: Some(cfg.max_days_to_expiry),
                        min_edge_bps: Some(cfg.min_edge_bps),
                    },
                    state,
                )
                .await
                {
                    Ok(v) => v
                        .into_iter()
                        .filter(|o| o.is_executable)
                        .map(|o| (o.id, o.currency, o.net_edge_usd))
                        .collect(),
                    Err(e) => {
                        tracing::warn!("options bot {substrategy}: {ccy} scan failed: {e}");
                        continue;
                    }
                }
            } else if LIVE_PACKAGE_SUBSTRATEGIES.contains(&substrategy) {
                match scan_options_packages_impl(
                    &OptionsPackageScanParams {
                        substrategy_id: substrategy.to_string(),
                        currency: ccy.to_string(),
                        qty_base: Some(cfg.qty_base),
                        min_hours_to_expiry: Some(cfg.min_hours_to_expiry),
                        max_days_to_expiry: Some(cfg.max_days_to_expiry),
                        min_edge_bps: Some(cfg.min_edge_bps),
                    },
                    state,
                )
                .await
                {
                    Ok(v) => v
                        .into_iter()
                        .filter(|o| o.is_executable)
                        .map(|o| (o.id, o.currency, o.net_edge_usd))
                        .collect(),
                    Err(e) => {
                        tracing::warn!("options bot {substrategy}: {ccy} scan failed: {e}");
                        continue;
                    }
                }
            } else {
                continue;
            };
            let mut opps = opps;
            opps.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(Ordering::Equal));
            for (id, _ccy, _edge) in opps {
                if open >= cfg.max_open_positions {
                    break;
                }
                // One refusal ends the currency's batch — a mid-batch error
                // may mean partial fills; next poll re-evaluates honestly.
                let res = if substrategy == "parity" {
                    execute_options_parity_impl(
                        OptionsParityExecutionParams {
                            id: id.clone(),
                            currency: ccy.to_string(),
                            qty_base: cfg.qty_base,
                            min_edge_bps: Some(cfg.min_edge_bps),
                            auto: true,
                        },
                        state,
                    )
                    .await
                    .map(|_| ())
                } else {
                    execute_options_package_impl(
                        &OptionsPackageExecutionParams {
                            id: id.clone(),
                            substrategy_id: substrategy.to_string(),
                            currency: ccy.to_string(),
                            qty_base: cfg.qty_base,
                            min_edge_bps: Some(cfg.min_edge_bps),
                            auto: true,
                        },
                        state,
                    )
                    .await
                    .map(|_| ())
                };
                match res {
                    Ok(()) => open += 1,
                    Err(e) => {
                        tracing::warn!("options bot {substrategy}: entry {id} refused: {e}");
                        break;
                    }
                }
            }
        }
        poll
    }
}

#[tauri::command]
pub async fn get_options_bot_config(
    substrategy_id: String,
    state: tauri::State<'_, Arc<AppEngineState>>,
) -> Result<OptionsBotConfigDto, String> {
    let (policy, mode) = OptionsBotExecutor::load_config(&state, &substrategy_id);
    Ok(OptionsBotConfigDto::from_policy(
        &substrategy_id,
        &policy,
        mode,
    ))
}

#[tauri::command]
pub async fn set_options_bot_config(
    config: OptionsBotConfigDto,
    state: tauri::State<'_, Arc<AppEngineState>>,
) -> Result<(), String> {
    OptionsBotExecutor::save_config(&state, &config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppEngineState {
        AppEngineState::new()
    }

    fn armed(substrategy: &str) -> OptionsBotConfigDto {
        OptionsBotConfigDto {
            substrategy_id: substrategy.to_string(),
            enabled: true,
            mode: "auto".to_string(),
            qty_base: 0.1,
            min_edge_bps: 5.0,
            min_hours_to_expiry: 48.0,
            max_days_to_expiry: 60.0,
            poll_interval_secs: 30,
            max_open_positions: 2,
        }
    }

    #[test]
    fn config_roundtrips_and_validates() {
        let st = state();
        let dto = armed("parity");
        OptionsBotExecutor::save_config(&st, &dto).expect("save armed");
        let (policy, mode) = OptionsBotExecutor::load_config(&st, "parity");
        assert_eq!(mode, BotMode::Auto);
        assert_eq!(policy.max_open_positions, 2);

        // Package substrategies arm independently.
        OptionsBotExecutor::save_config(&st, &armed("box_spread")).expect("box bot arms");
        let (_, mode) = OptionsBotExecutor::load_config(&st, "box_spread");
        assert_eq!(mode, BotMode::Auto);

        // Pending substrategies cannot arm.
        assert!(OptionsBotExecutor::save_config(&st, &armed("dividend")).is_err());

        // Bad mode fails closed — never persists.
        let mut bad = armed("parity");
        bad.mode = "yolo".to_string();
        assert!(OptionsBotExecutor::save_config(&st, &bad).is_err());
        let (_, mode) = OptionsBotExecutor::load_config(&st, "parity");
        assert_eq!(mode, BotMode::Auto); // previous config untouched
    }

    #[tokio::test]
    async fn reduce_only_retires_to_off_once_flat() {
        let st = state();
        let mut dto = armed("parity");
        dto.mode = "reduce_only".to_string();
        dto.enabled = false;
        OptionsBotExecutor::save_config(&st, &dto).expect("save reduce_only");
        let bot = OptionsBotExecutor::new();
        bot.tick_once(&st).await;
        let (_, mode) = OptionsBotExecutor::load_config(&st, "parity");
        assert_eq!(mode, BotMode::Off);
    }
}
