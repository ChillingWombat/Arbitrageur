//! OpportunityExecutor — autonomous convergence bot engine (GitHub issue #5).
//!
//! Opt-in, paper-only consumer of the spread monitor's qualified-opportunity
//! seam. One background task polls `qualified_opportunities()`, runs every
//! candidate through the pure gate pipeline in `arbitrager_strategy`, sizes
//! from the capacity ladder, and dispatches entries through the SAME guarded
//! paper primitive as the manual row-Execute button
//! (`execute_monitor_opportunity_impl`). Exits unwind through the existing
//! symbol-scoped flatten path. Every decision — enter, skip with reason,
//! exit — is appended to `monitor_signal_events` with the signal belief
//! snapshot.
//!
//! Task placement: a standalone tokio task sibling to the spread monitor
//! (spawned in `main`), not a monitor-integrated loop. The monitor keeps its
//! single responsibility (signal measurement); this task owns autonomy, its
//! own cadence, and its own shutdown flag. Documented here per the issue.
//!
//! Fail-closed posture:
//! - Default config is inert (`enabled = false`, mode `off`).
//! - Live mode hard-blocks entries at the tick AND inside the paper path.
//! - A corrupt persisted config falls back to inert, never to permissive.
//! - Audit-write failures are surfaced in status, never block risk exits.
//! - Flatten is symbol-scoped: at most one radar position per symbol can
//!   exist (the paper path rejects a second paper position on the same
//!   symbol), so an exit unwinds exactly the pair the bot opened.
//!
//! Restart recovery: open positions persist in `radar_bot_positions` with
//! their entry snapshot. On start the executor reloads them and keeps only
//! those still backed by an open `radar` ledger signal; the rest are dropped
//! with a `bot_exit` audit (`closed outside bot`). Per-pair cooldowns rebuild
//! from recent `bot_exit` audits. Cooldown state older than the audit window
//! is treated as expired rather than fabricated.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use arbitrager_strategy::convergence_auto::{
    evaluate_entry_gates, evaluate_exit, plan_entry, size_position, AutoExecConfig, BotMode,
    ConvergenceSignal, ExitEval, ExitTrigger, GateContext, LiveSpreadView,
};
use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::state::AppEngineState;
use crate::state_store::BotPositionRow;

/// Ledger strategy id the bot trades under. Shared with the manual
/// row-Execute button so the Spread Radar page scopes bot and manual fills
/// together, apart from Single Spread ("futures").
pub const BOT_STRATEGY_ID: &str = "radar";
/// Skip audits are deduped per pair within this window to bound the log;
/// enters and exits are always recorded.
const SKIP_DEDUP_SECS: i64 = 300;
/// Cap on the cooldown-rebuild audit scan at startup.
const REBUILD_SCAN_LIMIT: i64 = 10_000;

/// Normalized pair key. Venues lowercase, symbol uppercase — matching the
/// normalization `record_monitor_signal_event` applies, so executor keys and
/// audit rows join without case drift.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PairKey {
    pub symbol: String,
    pub buy_venue: String,
    pub sell_venue: String,
}

impl PairKey {
    pub fn new(symbol: &str, buy_venue: &str, sell_venue: &str) -> Self {
        Self {
            symbol: symbol.trim().to_uppercase(),
            buy_venue: buy_venue.trim().to_lowercase(),
            sell_venue: sell_venue.trim().to_lowercase(),
        }
    }

    pub fn label(&self) -> String {
        format!("{} {}→{}", self.symbol, self.buy_venue, self.sell_venue)
    }
}

/// Persisted bot configuration (one JSON row in `radar_bot_config`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RadarBotConfigDto {
    pub enabled: bool,
    pub mode: String,
    pub poll_interval_secs: u64,
    pub min_p_win: f64,
    pub min_expected_profit_bps: f64,
    pub max_concurrent_positions: usize,
    pub max_notional_per_position_usd: f64,
    pub max_notional_per_venue_usd: f64,
    pub max_notional_per_symbol_usd: f64,
    pub pair_cooldown_secs: u64,
    pub hold_timeout_mult: f64,
    pub stop_mult: f64,
    pub stale_unwind_secs: u64,
}

impl Default for RadarBotConfigDto {
    fn default() -> Self {
        Self::from_policy(&AutoExecConfig::default(), BotMode::Off)
    }
}

impl RadarBotConfigDto {
    pub fn from_policy(cfg: &AutoExecConfig, mode: BotMode) -> Self {
        Self {
            enabled: cfg.enabled,
            mode: mode.as_str().to_string(),
            poll_interval_secs: cfg.poll_interval_secs,
            min_p_win: cfg.min_p_win,
            min_expected_profit_bps: cfg.min_expected_profit_bps,
            max_concurrent_positions: cfg.max_concurrent_positions,
            max_notional_per_position_usd: cfg.max_notional_per_position_usd,
            max_notional_per_venue_usd: cfg.max_notional_per_venue_usd,
            max_notional_per_symbol_usd: cfg.max_notional_per_symbol_usd,
            pair_cooldown_secs: cfg.pair_cooldown_secs,
            hold_timeout_mult: cfg.hold_timeout_mult,
            stop_mult: cfg.stop_mult,
            stale_unwind_secs: cfg.stale_unwind_secs,
        }
    }

    /// Fail-closed split: unknown mode or out-of-range knob rejects the whole
    /// payload. `enabled` demands `auto` mode — an armed flag with an inert
    /// mode (or vice versa) resolves to inert, never to trading.
    pub fn to_policy(&self) -> Result<(AutoExecConfig, BotMode), String> {
        let mode = BotMode::parse_mode(&self.mode)
            .ok_or_else(|| format!("unknown bot mode '{}'", self.mode))?;
        let cfg = AutoExecConfig {
            enabled: self.enabled && mode == BotMode::Auto,
            poll_interval_secs: self.poll_interval_secs,
            min_p_win: self.min_p_win,
            min_expected_profit_bps: self.min_expected_profit_bps,
            max_concurrent_positions: self.max_concurrent_positions,
            max_notional_per_position_usd: self.max_notional_per_position_usd,
            max_notional_per_venue_usd: self.max_notional_per_venue_usd,
            max_notional_per_symbol_usd: self.max_notional_per_symbol_usd,
            pair_cooldown_secs: self.pair_cooldown_secs,
            hold_timeout_mult: self.hold_timeout_mult,
            stop_mult: self.stop_mult,
            stale_unwind_secs: self.stale_unwind_secs,
        };
        cfg.validate()?;
        Ok((cfg, mode))
    }
}

/// One managed convergence position as reported to the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotOpenPositionDto {
    pub symbol: String,
    pub buy_venue: String,
    pub sell_venue: String,
    pub entry_ts_ms: i64,
    pub entry_dev_bps: f64,
    pub baseline_bps: Option<f64>,
    pub size_usd: f64,
    pub deadline_ms: i64,
    pub live_dev_bps: Option<f64>,
    pub live_status: Option<String>,
    pub unwatched: bool,
}

/// One recent bot decision as reported to the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotDecisionDto {
    pub ts_ms: i64,
    pub pair: String,
    pub decision: String,
    pub reason: String,
}

/// Status surface for the Spread Radar bot gadget.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RadarBotStatusDto {
    pub enabled: bool,
    pub mode: String,
    pub open_positions: Vec<BotOpenPositionDto>,
    pub enters_today: i64,
    pub exits_today: i64,
    pub skips_today: i64,
    pub last_decision: Option<BotDecisionDto>,
    pub last_error: Option<String>,
    pub audit_errors: u64,
    pub monitor_running: bool,
    pub snapshot_present: bool,
}

/// Execution seam. Production calls the existing guarded paper path; tests
/// inject scripted mocks. Paper-only by construction: neither method has a
/// live-mode variant, and both callees hard-block live mode themselves.
pub trait BotDispatch: Send + Sync {
    fn enter<'a>(
        &'a self,
        symbol: &'a str,
        buy_venue: &'a str,
        sell_venue: &'a str,
        size_usd: f64,
        state: &'a AppEngineState,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;

    fn exit<'a>(
        &'a self,
        symbol: &'a str,
        state: &'a AppEngineState,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;
}

/// Production dispatch: entries reuse `execute_monitor_opportunity_impl`
/// (identical gates to the manual button); exits reuse the symbol-scoped
/// flatten path for the `radar` strategy.
pub struct PaperDispatch;

impl BotDispatch for PaperDispatch {
    fn enter<'a>(
        &'a self,
        symbol: &'a str,
        buy_venue: &'a str,
        sell_venue: &'a str,
        size_usd: f64,
        state: &'a AppEngineState,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            crate::commands::execute_monitor_opportunity_impl(
                symbol, buy_venue, sell_venue, size_usd, state,
            )
            .await
            .map(|r| r.message)
        })
    }

    fn exit<'a>(
        &'a self,
        symbol: &'a str,
        state: &'a AppEngineState,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            let res =
                crate::commands::flatten_ticker_positions_impl(Some(symbol), BOT_STRATEGY_ID, state)
                    .await?;
            if res.residual_signals > 0 {
                return Err(format!(
                    "flatten left {} residual signal(s) on {symbol}",
                    res.residual_signals
                ));
            }
            Ok(format!(
                "unwound {} signal(s) on {symbol}",
                res.signals_closed
            ))
        })
    }
}

#[derive(Debug, Clone)]
struct TrackedPosition {
    entry_ts_ms: i64,
    entry_dev_bps: f64,
    baseline_bps: Option<f64>,
    friction_band_bps: f64,
    size_usd: f64,
    deadline_ms: i64,
    unwatched_since_ms: Option<i64>,
    last_seen_ms: i64,
}

impl TrackedPosition {
    fn persist_row(&self, key: &PairKey) -> BotPositionRow {
        BotPositionRow {
            symbol: key.symbol.clone(),
            buy_venue: key.buy_venue.clone(),
            sell_venue: key.sell_venue.clone(),
            entry_ts_ms: self.entry_ts_ms,
            entry_dev_bps: self.entry_dev_bps,
            baseline_bps: self.baseline_bps,
            friction_band_bps: self.friction_band_bps,
            size_usd: self.size_usd,
            deadline_ms: self.deadline_ms,
            unwatched_since_ms: self.unwatched_since_ms,
            last_seen_ms: self.last_seen_ms,
        }
    }
}

pub struct OpportunityExecutor {
    open: Mutex<HashMap<PairKey, TrackedPosition>>,
    /// Pair → last exit timestamp (ms) for the re-entry cooldown.
    cooldowns: Mutex<HashMap<PairKey, i64>>,
    /// (pair, decision) → last skip-audit timestamp (ms) for log bounding.
    skip_log: Mutex<HashMap<(PairKey, String), i64>>,
    last_decision: Mutex<Option<BotDecisionDto>>,
    last_error: Mutex<Option<String>>,
    audit_errors: AtomicU64,
    shutdown: Mutex<bool>,
}

impl OpportunityExecutor {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            open: Mutex::new(HashMap::new()),
            cooldowns: Mutex::new(HashMap::new()),
            skip_log: Mutex::new(HashMap::new()),
            last_decision: Mutex::new(None),
            last_error: Mutex::new(None),
            audit_errors: AtomicU64::new(0),
            shutdown: Mutex::new(false),
        })
    }

    #[allow(dead_code)]
    pub fn shutdown(&self) {
        *self.shutdown.lock() = true;
    }

    #[cfg(test)]
    pub fn open_count(&self) -> usize {
        self.open.lock().len()
    }

    /// Drop dedup/cooldown entries older than twice the pair cooldown (with
    /// an hour floor): recent decisions stay suppressed, ancient pairs
    /// cannot grow the maps without bound.
    fn prune(&self, now_ms: i64, cfg: &AutoExecConfig) {
        let horizon_ms = ((cfg.pair_cooldown_secs as i64 * 1000).max(3_600_000)) * 2;
        self.skip_log
            .lock()
            .retain(|_, last| now_ms - *last < horizon_ms);
        self.cooldowns
            .lock()
            .retain(|_, last| now_ms - *last < horizon_ms);
    }

    /// Reload persisted config; any failure (missing row, bad JSON, failed
    /// validation) resolves to inert — never to permissive.
    pub fn load_config(state: &AppEngineState) -> (AutoExecConfig, BotMode) {
        let raw = state
            .state_store
            .radar_bot_config_json()
            .ok()
            .flatten();
        let Some(json) = raw else {
            return (AutoExecConfig::default(), BotMode::Off);
        };
        let dto: RadarBotConfigDto = match serde_json::from_str(&json) {
            Ok(d) => d,
            Err(_) => return (AutoExecConfig::default(), BotMode::Off),
        };
        dto.to_policy()
            .unwrap_or((AutoExecConfig::default(), BotMode::Off))
    }

    pub fn save_config(state: &AppEngineState, dto: &RadarBotConfigDto) -> Result<(), String> {
        // Validate (including mode) BEFORE persisting: a rejected payload
        // leaves the previous config untouched.
        let (cfg, mode) = dto.to_policy()?;
        let canonical = RadarBotConfigDto::from_policy(&cfg, mode);
        let json =
            serde_json::to_string(&canonical).map_err(|e| format!("config encode: {e}"))?;
        state
            .state_store
            .save_radar_bot_config_json(&json)
            .map_err(|e| format!("config persist: {e}"))?;
        Ok(())
    }

    /// Restart recovery: reload persisted positions, keep only those still
    /// backed by an open `radar` ledger signal, and rebuild cooldowns from
    /// recent `bot_exit` audits. Never fabricates watchers.
    pub fn rehydrate(&self, state: &AppEngineState) {
        let now_ms = Utc::now().timestamp_millis();
        let persisted = state.state_store.list_bot_positions().unwrap_or_default();
        for row in persisted {
            let key = PairKey::new(&row.symbol, &row.buy_venue, &row.sell_venue);
            if Self::ledger_backs(state, &key) {
                self.open.lock().insert(
                    key,
                    TrackedPosition {
                        entry_ts_ms: row.entry_ts_ms,
                        entry_dev_bps: row.entry_dev_bps,
                        baseline_bps: row.baseline_bps,
                        friction_band_bps: row.friction_band_bps,
                        size_usd: row.size_usd,
                        deadline_ms: row.deadline_ms,
                        unwatched_since_ms: row.unwatched_since_ms,
                        last_seen_ms: row.last_seen_ms,
                    },
                );
            } else {
                let _ = state
                    .state_store
                    .delete_bot_position(&row.symbol, &row.buy_venue, &row.sell_venue);
                self.audit(
                    state,
                    &key,
                    now_ms,
                    None,
                    "bot_exit",
                    "closed outside bot (no open ledger signal on restart)",
                    None,
                    None,
                    None,
                    &serde_json::json!({
                        "sizeUsd": row.size_usd,
                        "recovered": true,
                    })
                    .to_string(),
                );
            }
        }
        // Cooldown rebuild: the newest bot_exit per pair within the scan.
        if let Ok(events) =
            state
                .state_store
                .recent_monitor_signal_events(None, None, None, REBUILD_SCAN_LIMIT)
        {
            let mut fresh: HashMap<PairKey, i64> = HashMap::new();
            for e in events {
                if e.trigger == "bot_decision" && e.new_status == "bot_exit" {
                    let k = PairKey::new(&e.symbol, &e.buy_venue, &e.sell_venue);
                    fresh
                        .entry(k)
                        .and_modify(|t| *t = (*t).max(e.ts_ms))
                        .or_insert(e.ts_ms);
                }
            }
            *self.cooldowns.lock() = fresh;
        }
    }

    /// An open `radar` ledger signal whose legs span the pair's venues backs
    /// the persisted position. DB errors fail closed (no backing claimed).
    fn ledger_backs(state: &AppEngineState, key: &PairKey) -> bool {
        let signals = state.state_store.list_signals(Some(&key.symbol), None);
        let Ok(all) = signals else {
            return false;
        };
        all.iter()
            .filter(|s| {
                s.strategy_id.eq_ignore_ascii_case(BOT_STRATEGY_ID) && s.status.counts_as_open()
            })
            .any(|s| {
                let venues: Vec<String> =
                    s.legs.iter().map(|l| l.venue.to_lowercase()).collect();
                venues.contains(&key.buy_venue) && venues.contains(&key.sell_venue)
            })
    }

    #[allow(clippy::too_many_arguments)]
    fn audit(
        &self,
        state: &AppEngineState,
        key: &PairKey,
        now_ms: i64,
        old_status: Option<String>,
        decision: &str,
        reason: &str,
        edge_bps: Option<f64>,
        dev_bps: Option<f64>,
        p_win: Option<f64>,
        economics_json: &str,
    ) {
        let res = state.state_store.record_monitor_signal_event(
            &key.symbol,
            &key.buy_venue,
            &key.sell_venue,
            now_ms,
            old_status.as_deref(),
            decision,
            edge_bps,
            dev_bps,
            p_win,
            Some(economics_json),
            "bot_decision",
        );
        if res.is_err() {
            self.audit_errors.fetch_add(1, Ordering::Relaxed);
            tracing::warn!("radar bot: audit write failed for {} ({decision})", key.label());
        }
        *self.last_decision.lock() = Some(BotDecisionDto {
            ts_ms: now_ms,
            pair: key.label(),
            decision: decision.to_string(),
            reason: reason.to_string(),
        });
    }

    fn audit_skip(
        &self,
        state: &AppEngineState,
        key: &PairKey,
        now_ms: i64,
        signal: &ConvergenceSignal,
        reason: &str,
        size_usd: Option<f64>,
    ) {
        // Bound the log: one skip audit per pair per window; enters/exits
        // always pass through.
        {
            let mut log = self.skip_log.lock();
            let slot = (key.clone(), "bot_skip".to_string());
            if let Some(last) = log.get(&slot) {
                if now_ms - last < SKIP_DEDUP_SECS * 1000 {
                    return;
                }
            }
            log.insert(slot, now_ms);
        }
        self.audit(
            state,
            key,
            now_ms,
            Some(signal.status.clone()),
            "bot_skip",
            reason,
            signal.current_edge_bps,
            signal.deviation_bps,
            signal.p_win,
            &belief_snapshot(signal, size_usd, Some(reason)),
        );
    }

    fn signal_of(row: &crate::monitor::MonitorRowDto) -> ConvergenceSignal {
        ConvergenceSignal {
            symbol: row.symbol.clone(),
            buy_venue: row.buy_venue.clone(),
            sell_venue: row.sell_venue.clone(),
            status: row.status.clone(),
            executable: row.executable,
            current_edge_bps: row.current_edge_bps,
            baseline_bps: row.baseline_bps,
            deviation_bps: row.deviation_bps,
            net_capture_bps: row.net_capture_bps,
            p_win: row.p_win,
            expected_profit_bps: row.expected_profit_bps,
            expected_profit_usd: row.expected_profit_usd,
            optimal_size_usd: row.optimal_size_usd,
            breakeven_size_usd: row.breakeven_size_usd,
            funding_unknown: row.funding_unknown,
            expected_converge_secs: row.expected_converge_secs,
        }
    }

    fn gate_context(
        &self,
        state: &AppEngineState,
        signal: &ConvergenceSignal,
        cfg: &AutoExecConfig,
        size_usd: f64,
        now_ms: i64,
    ) -> GateContext {
        let open = self.open.lock();
        let mut buy_notional = 0.0;
        let mut sell_notional = 0.0;
        let mut sym_notional = 0.0;
        for (k, p) in open.iter() {
            if k.buy_venue == signal.buy_venue.to_lowercase() {
                buy_notional += p.size_usd;
            }
            if k.sell_venue == signal.sell_venue.to_lowercase() {
                sell_notional += p.size_usd;
            }
            if k.symbol == signal.symbol.to_uppercase() {
                sym_notional += p.size_usd;
            }
        }
        let positions = open.len();
        drop(open);
        let key = PairKey::new(&signal.symbol, &signal.buy_venue, &signal.sell_venue);
        let cooldown_remaining_secs = self.cooldowns.lock().get(&key).map(|last| {
            (cfg.pair_cooldown_secs as i64 * 1000 - (now_ms - last)) as f64 / 1000.0
        });
        // DB errors fail closed: an unreadable ledger blocks the symbol.
        let blocking_signal = state
            .state_store
            .ticker_blocked(&signal.symbol)
            .unwrap_or(true);
        let (circuit_ok, circuit_reason) = match state.circuit_breaker.is_trading_allowed() {
            Ok(()) => (true, None),
            Err(r) => (false, Some(r)),
        };
        GateContext {
            live_mode: *state.is_live_mode.read(),
            engine_paused: *state.is_paused.read(),
            monitor_enabled: state
                .monitor
                .lock()
                .as_ref()
                .map(|e| e.config().enabled)
                .unwrap_or(false),
            snapshot_present: true,
            circuit_ok,
            circuit_reason,
            open_positions: positions,
            buy_venue_notional_usd: buy_notional,
            sell_venue_notional_usd: sell_notional,
            symbol_notional_usd: sym_notional,
            size_usd,
            cooldown_remaining_secs,
            blocking_signal,
        }
    }

    /// One poll iteration. Returns the sleep interval in seconds. Never holds
    /// a lock across dispatch awaits; per-pair failures never abort the tick.
    pub async fn tick_once(
        &self,
        state: &AppEngineState,
        dispatch: &dyn BotDispatch,
    ) -> u64 {
        let now_ms = Utc::now().timestamp_millis();
        let (cfg, mode) = Self::load_config(state);
        if !mode.exits_allowed() {
            // Fully inert: no entries, no exit maintenance. The manual
            // flatten path remains the kill switch.
            return cfg.poll_interval_secs.max(1);
        }
        // Bound auxiliary maps every tick: pairs churn over weeks, and both
        // maps are keyed by pair.
        self.prune(now_ms, &cfg);
        let engine = state.monitor.lock().clone();
        let Some(eng) = engine else {
            return cfg.poll_interval_secs.max(1);
        };
        let snap = eng.snapshot();
        let rows: HashMap<PairKey, crate::monitor::MonitorRowDto> = snap
            .as_ref()
            .map(|s| {
                s.rows
                    .iter()
                    .map(|r| {
                        (
                            PairKey::new(&r.symbol, &r.buy_venue, &r.sell_venue),
                            r.clone(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let snapshot_present = snap.is_some();

        // Exits first: never strand risk, in every armed mode.
        if mode.exits_allowed() {
            let keys: Vec<(PairKey, TrackedPosition)> = self
                .open
                .lock()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            for (key, mut pos) in keys {
                let live = rows.get(&key).map(|r| LiveSpreadView {
                    status: r.status.clone(),
                    deviation_bps: r.deviation_bps,
                });
                let watched = live.as_ref().is_some_and(|l| {
                    l.status != "stale" && l.deviation_bps.is_some_and(|d| d.is_finite())
                });
                if watched {
                    pos.unwatched_since_ms = None;
                    pos.last_seen_ms = now_ms;
                } else if pos.unwatched_since_ms.is_none() {
                    pos.unwatched_since_ms = Some(now_ms);
                }
                let unwatched_for = pos
                    .unwatched_since_ms
                    .map(|t| (now_ms - t).max(0) as f64 / 1000.0);
                match evaluate_exit(
                    pos.entry_dev_bps,
                    pos.friction_band_bps,
                    pos.deadline_ms,
                    unwatched_for,
                    live.as_ref(),
                    now_ms,
                    &cfg,
                ) {
                    ExitEval::Hold | ExitEval::Unwatched { .. } => {
                        self.open.lock().insert(key.clone(), pos.clone());
                        let _ = state.state_store.upsert_bot_position(&pos.persist_row(&key));
                    }
                    ExitEval::Exit { trigger, reason } => {
                        self.exit_position(state, dispatch, &key, &pos, trigger, &reason, now_ms)
                            .await;
                    }
                }
            }
        }

        // Entries only when explicitly armed in full-auto. Anything else
        // (paused, reduce-only, disabled flag) skips entries silently —
        // denial audits would otherwise spam while disarmed.
        if cfg.enabled && snapshot_present {
            let qualified = eng.qualified_opportunities();
            for row in qualified {
                let signal = Self::signal_of(&row);
                let key = PairKey::new(&signal.symbol, &signal.buy_venue, &signal.sell_venue);
                if self.open.lock().contains_key(&key) {
                    self.audit_skip(
                        state,
                        &key,
                        now_ms,
                        &signal,
                        "position already open",
                        None,
                    );
                    continue;
                }
                // Size before gates so notional caps evaluate the candidate.
                // Unsizeable rows fail closed here with a named reason.
                let Some(size_usd) = size_position(&signal, &cfg) else {
                    self.audit_skip(state, &key, now_ms, &signal, "no usable capacity", None);
                    continue;
                };
                let mut ctx = self.gate_context(state, &signal, &cfg, size_usd, now_ms);
                ctx.snapshot_present = snapshot_present;
                let verdict = evaluate_entry_gates(&signal, &cfg, &ctx);
                if !verdict.allow {
                    self.audit_skip(state, &key, now_ms, &signal, &verdict.reason, Some(size_usd));
                    continue;
                }
                let plan = match plan_entry(&signal, &cfg, now_ms) {
                    Ok(p) => p,
                    Err(e) => {
                        self.audit_skip(state, &key, now_ms, &signal, &e, Some(size_usd));
                        continue;
                    }
                };
                match dispatch
                    .enter(&signal.symbol, &signal.buy_venue, &signal.sell_venue, plan.size_usd, state)
                    .await
                {
                    Ok(note) => {
                        let dev = signal.deviation_bps.unwrap_or(0.0);
                        let pos = TrackedPosition {
                            entry_ts_ms: now_ms,
                            entry_dev_bps: dev,
                            baseline_bps: signal.baseline_bps,
                            friction_band_bps: plan.friction_band_bps,
                            size_usd: plan.size_usd,
                            deadline_ms: plan.deadline_ms,
                            unwatched_since_ms: None,
                            last_seen_ms: now_ms,
                        };
                        self.open.lock().insert(key.clone(), pos.clone());
                        let _ = state.state_store.upsert_bot_position(&pos.persist_row(&key));
                        self.audit(
                            state,
                            &key,
                            now_ms,
                            Some(signal.status.clone()),
                            "bot_enter",
                            &note,
                            signal.current_edge_bps,
                            signal.deviation_bps,
                            signal.p_win,
                            &belief_snapshot(&signal, Some(plan.size_usd), None),
                        );
                    }
                    Err(e) => {
                        self.audit_skip(
                            state,
                            &key,
                            now_ms,
                            &signal,
                            &format!("entry rejected: {e}"),
                            Some(plan.size_usd),
                        );
                    }
                }
            }
        }
        cfg.poll_interval_secs.max(1)
    }

    #[allow(clippy::too_many_arguments)]
    async fn exit_position(
        &self,
        state: &AppEngineState,
        dispatch: &dyn BotDispatch,
        key: &PairKey,
        pos: &TrackedPosition,
        trigger: ExitTrigger,
        reason: &str,
        now_ms: i64,
    ) {
        // Cooldown starts at the exit decision, including failed attempts, so
        // a flapping pair cannot spin the unwind path.
        self.cooldowns.lock().insert(key.clone(), now_ms);
        match dispatch.exit(&key.symbol, state).await {
            Ok(note) => {
                self.open.lock().remove(key);
                let _ = state
                    .state_store
                    .delete_bot_position(&key.symbol, &key.buy_venue, &key.sell_venue);
                let exit_json = serde_json::json!({
                    "trigger": trigger.as_str(),
                    "sizeUsd": pos.size_usd,
                    "entryDevBps": pos.entry_dev_bps,
                    "detail": note,
                })
                .to_string();
                self.audit(
                    state,
                    key,
                    now_ms,
                    None,
                    "bot_exit",
                    reason,
                    None,
                    None,
                    None,
                    &exit_json,
                );
            }
            Err(e) => {
                *self.last_error.lock() =
                    Some(format!("exit failed for {}: {e}", key.label()));
                tracing::warn!("radar bot: exit failed for {}: {e}", key.label());
            }
        }
    }

    pub fn status(&self, state: &AppEngineState) -> RadarBotStatusDto {
        let (cfg, mode) = Self::load_config(state);
        let engine = state.monitor.lock().clone();
        let snap = engine.as_ref().and_then(|e| e.snapshot());
        let live_by_key: HashMap<PairKey, crate::monitor::MonitorRowDto> = snap
            .as_ref()
            .map(|s| {
                s.rows
                    .iter()
                    .map(|r| {
                        (
                            PairKey::new(&r.symbol, &r.buy_venue, &r.sell_venue),
                            r.clone(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let open_positions = self
            .open
            .lock()
            .iter()
            .map(|(k, p)| {
                let live = live_by_key.get(k);
                BotOpenPositionDto {
                    symbol: k.symbol.clone(),
                    buy_venue: k.buy_venue.clone(),
                    sell_venue: k.sell_venue.clone(),
                    entry_ts_ms: p.entry_ts_ms,
                    entry_dev_bps: p.entry_dev_bps,
                    baseline_bps: p.baseline_bps,
                    size_usd: p.size_usd,
                    deadline_ms: p.deadline_ms,
                    live_dev_bps: live.and_then(|r| r.deviation_bps),
                    live_status: live.map(|r| r.status.clone()),
                    unwatched: p.unwatched_since_ms.is_some(),
                }
            })
            .collect();
        let day_start_ms = Utc::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .map(|t| t.and_utc().timestamp_millis())
            .unwrap_or(0);
        let (mut enters, mut exits, mut skips) = (0i64, 0i64, 0i64);
        if let Ok(counts) = state.state_store.count_bot_decisions_since(day_start_ms) {
            for (status, n) in counts {
                match status.as_str() {
                    "bot_enter" => enters = n,
                    "bot_exit" => exits = n,
                    "bot_skip" => skips = n,
                    _ => {}
                }
            }
        }
        RadarBotStatusDto {
            enabled: cfg.enabled,
            mode: mode.as_str().to_string(),
            open_positions,
            enters_today: enters,
            exits_today: exits,
            skips_today: skips,
            last_decision: self.last_decision.lock().clone(),
            last_error: self.last_error.lock().clone(),
            audit_errors: self.audit_errors.load(Ordering::Relaxed),
            monitor_running: engine.as_ref().map(|e| e.snapshot().is_some()).unwrap_or(false),
            snapshot_present: snap.is_some(),
        }
    }

    pub async fn run(self: Arc<Self>, state: Arc<AppEngineState>) {
        self.rehydrate(&state);
        loop {
            if *self.shutdown.lock() {
                return;
            }
            let wait_secs = self.tick_once(&state, &PaperDispatch).await;
            tokio::time::sleep(Duration::from_secs(wait_secs)).await;
        }
    }
}

impl Default for OpportunityExecutor {
    fn default() -> Self {
        Self {
            open: Mutex::new(HashMap::new()),
            cooldowns: Mutex::new(HashMap::new()),
            skip_log: Mutex::new(HashMap::new()),
            last_decision: Mutex::new(None),
            last_error: Mutex::new(None),
            audit_errors: AtomicU64::new(0),
            shutdown: Mutex::new(false),
        }
    }
}

/// Belief snapshot attached to every enter/skip audit: what the monitor
/// believed at decision time, plus what the bot chose.
fn belief_snapshot(
    signal: &ConvergenceSignal,
    size_usd: Option<f64>,
    skip_reason: Option<&str>,
) -> String {
    serde_json::json!({
        "pWin": signal.p_win,
        "expectedProfitBps": signal.expected_profit_bps,
        "expectedProfitUsd": signal.expected_profit_usd,
        "optimalSizeUsd": signal.optimal_size_usd,
        "breakevenSizeUsd": signal.breakeven_size_usd,
        "deviationBps": signal.deviation_bps,
        "baselineBps": signal.baseline_bps,
        "currentEdgeBps": signal.current_edge_bps,
        "expectedConvergeSecs": signal.expected_converge_secs,
        "sizeUsd": size_usd,
        "skipReason": skip_reason,
    })
    .to_string()
}

#[tauri::command]
pub async fn get_radar_bot_config(
    state: tauri::State<'_, Arc<AppEngineState>>,
) -> Result<RadarBotConfigDto, String> {
    let (cfg, mode) = OpportunityExecutor::load_config(&state);
    Ok(RadarBotConfigDto::from_policy(&cfg, mode))
}

#[tauri::command]
pub async fn set_radar_bot_config(
    config: RadarBotConfigDto,
    state: tauri::State<'_, Arc<AppEngineState>>,
) -> Result<(), String> {
    OpportunityExecutor::save_config(&state, &config)
}

#[tauri::command]
pub async fn get_radar_bot_status(
    state: tauri::State<'_, Arc<AppEngineState>>,
) -> Result<RadarBotStatusDto, String> {
    let bot = state
        .radar_bot
        .lock()
        .clone()
        .ok_or_else(|| "radar bot is not running".to_string())?;
    Ok(bot.status(&state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::{
        CapacityTierDto, MonitorRowDto, MonitorSnapshotDto, SpreadMonitorEngine,
    };
    use crate::state_store::{LegSide, NewSignalExecution, NewSignalLeg, SignalStatus};
    use std::sync::Mutex as StdMutex;

    struct MockDispatch {
        enter_results: StdMutex<Vec<Result<String, String>>>,
        exit_results: StdMutex<Vec<Result<String, String>>>,
        enters: StdMutex<Vec<(String, String, String, f64)>>,
        exits: StdMutex<Vec<String>>,
    }

    impl MockDispatch {
        fn ok() -> Self {
            Self {
                enter_results: StdMutex::new(vec![]),
                exit_results: StdMutex::new(vec![]),
                enters: StdMutex::new(vec![]),
                exits: StdMutex::new(vec![]),
            }
        }

        fn enter_calls(&self) -> usize {
            self.enters.lock().map(|v| v.len()).unwrap_or(0)
        }

        fn exit_calls(&self) -> usize {
            self.exits.lock().map(|v| v.len()).unwrap_or(0)
        }
    }

    impl BotDispatch for MockDispatch {
        fn enter<'a>(
            &'a self,
            symbol: &'a str,
            buy_venue: &'a str,
            sell_venue: &'a str,
            size_usd: f64,
            _state: &'a AppEngineState,
        ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
            Box::pin(async move {
                self.enters.lock().map(|mut v| {
                    v.push((symbol.to_string(), buy_venue.to_string(), sell_venue.to_string(), size_usd))
                }).ok();
                self.enter_results.lock().ok().and_then(|mut q| {
                    if q.is_empty() { None } else { Some(q.remove(0)) }
                }).unwrap_or(Ok("mock fill".to_string()))
            })
        }

        fn exit<'a>(
            &'a self,
            symbol: &'a str,
            _state: &'a AppEngineState,
        ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
            Box::pin(async move {
                self.exits.lock().map(|mut v| v.push(symbol.to_string())).ok();
                self.exit_results.lock().ok().and_then(|mut q| {
                    if q.is_empty() { None } else { Some(q.remove(0)) }
                }).unwrap_or(Ok("mock unwind".to_string()))
            })
        }
    }

    fn qualified_row(symbol: &str, buy: &str, sell: &str) -> MonitorRowDto {
        MonitorRowDto {
            symbol: symbol.to_string(),
            buy_venue: buy.to_string(),
            sell_venue: sell.to_string(),
            status: "qualified".to_string(),
            current_edge_bps: Some(12.0),
            baseline_bps: Some(2.0),
            deviation_bps: Some(10.0),
            net_capture_bps: Some(6.0),
            p_win: Some(0.95),
            expected_profit_bps: Some(5.7),
            expected_profit_usd: Some(57.0),
            optimal_size_usd: Some(10_000.0),
            breakeven_size_usd: Some(8_000.0),
            capacity_ladder: vec![CapacityTierDto {
                size_usd: 1000.0,
                filled_usd: 1000.0,
                net_bps: Some(6.0),
                expected_net_bps: Some(5.7),
                marginal_p_win: Some(0.95),
            }],
            funding_carry_bps: Some(0.1),
            funding_unknown: false,
            first_seen_ms: Some(1),
            last_seen_ms: Some(2),
            signal_age_secs: Some(60.0),
            expected_converge_secs: Some(120.0),
            qualification_progress: 1.0,
            observed_episodes: 8,
            executable: true,
            block_reason: None,
            threshold_ratio: Some(0.8),
            coverage: Some(1.0),
            session_id: "overlap".to_string(),
            session_provisional: false,
            session_allow: 1.0,
            session_size_mult: 1.0,
        }
    }

    fn armed_config() -> RadarBotConfigDto {
        let mut dto = RadarBotConfigDto::from_policy(&AutoExecConfig::default(), BotMode::Auto);
        dto.enabled = true;
        dto
    }

    fn harness(rows: Vec<MonitorRowDto>) -> (AppEngineState, Arc<OpportunityExecutor>) {
        let state = AppEngineState::new();
        OpportunityExecutor::save_config(&state, &armed_config()).expect("save armed config");
        let engine = SpreadMonitorEngine::new(&state);
        engine.inject_snapshot(MonitorSnapshotDto {
            generated_ts_ms: Utc::now().timestamp_millis(),
            running: true,
            rows,
            watchlist: vec!["BTC".to_string()],
            venues: vec![],
            exempted: vec![],
        });
        *state.monitor.lock() = Some(engine);
        let bot = OpportunityExecutor::new();
        *state.radar_bot.lock() = Some(bot.clone());
        (state, bot)
    }

    fn bot_events(state: &AppEngineState, decision: &str) -> Vec<crate::state_store::MonitorSignalEventRow> {
        state
            .state_store
            .recent_monitor_signal_events(None, None, None, 10_000)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.trigger == "bot_decision" && e.new_status == decision)
            .collect()
    }

    fn record_radar_ledger(state: &AppEngineState, id: &str, symbol: &str, buy: &str, sell: &str, status: SignalStatus) {
        state
            .state_store
            .record_signal_execution(&NewSignalExecution {
                id: id.to_string(),
                strategy_id: "radar".to_string(),
                symbol: symbol.to_string(),
                route_id: None,
                expected_net_usd: None,
                note: None,
                legs: vec![
                    NewSignalLeg {
                        account_id: None,
                        venue: buy.to_string(),
                        side: LegSide::Long,
                        intended_qty: 0.01,
                    },
                    NewSignalLeg {
                        account_id: None,
                        venue: sell.to_string(),
                        side: LegSide::Short,
                        intended_qty: 0.01,
                    },
                ],
            })
            .expect("record ledger signal");
        if !matches!(status, SignalStatus::InFlight) {
            state
                .state_store
                .update_signal_status(id, status, None)
                .expect("set ledger status");
        }
    }

    #[tokio::test]
    async fn entry_on_qualified_opens_position_and_audits() {
        let (state, bot) = harness(vec![qualified_row("BTC", "hyperliquid", "lighter")]);
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.enter_calls(), 1);
        assert_eq!(bot.open_count(), 1);
        assert_eq!(state.state_store.list_bot_positions().unwrap_or_default().len(), 1);
        let enters = bot_events(&state, "bot_enter");
        assert_eq!(enters.len(), 1);
        let e = &enters[0];
        assert_eq!(e.p_win, Some(0.95));
        let econ: serde_json::Value =
            serde_json::from_str(e.economics_json.as_deref().unwrap_or("{}")).expect("valid snapshot json");
        assert_eq!(econ["pWin"], serde_json::json!(0.95));
        assert_eq!(econ["sizeUsd"], serde_json::json!(1000.0));
    }

    #[tokio::test]
    async fn second_tick_does_not_double_enter() {
        let (state, bot) = harness(vec![qualified_row("BTC", "hyperliquid", "lighter")]);
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.enter_calls(), 1, "same pair must not re-enter while open");
        assert_eq!(bot.open_count(), 1);
    }

    #[tokio::test]
    async fn skip_reasons_cover_every_gate() {
        // Disabled config: no entries, no audits (fully inert).
        let (state, bot) = harness(vec![qualified_row("BTC", "hyperliquid", "lighter")]);
        OpportunityExecutor::save_config(
            &state,
            &RadarBotConfigDto::from_policy(&AutoExecConfig::default(), BotMode::Off),
        )
        .expect("save off");
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.enter_calls(), 0);
        assert!(bot_events(&state, "bot_skip").is_empty());

        // Live mode hard-blocks.
        let (state, bot) = harness(vec![qualified_row("BTC", "hyperliquid", "lighter")]);
        *state.is_live_mode.write() = true;
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.enter_calls(), 0);
        assert!(bot_events(&state, "bot_skip").iter().any(|e| e.economics_json
            .as_deref()
            .unwrap_or("")
            .contains("paper-only")));

        // Engine pause blocks.
        let (state, bot) = harness(vec![qualified_row("BTC", "hyperliquid", "lighter")]);
        *state.is_paused.write() = true;
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.enter_calls(), 0);

        // Low P_win skips with the floor reason.
        let (state, bot) = harness(vec![{
            let mut r = qualified_row("BTC", "hyperliquid", "lighter");
            r.p_win = Some(0.5);
            r
        }]);
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.enter_calls(), 0);
        assert!(bot_events(&state, "bot_skip").iter().any(|e| e.economics_json
            .as_deref()
            .unwrap_or("")
            .contains("floor")));

        // Funding unknown never trades.
        let (state, bot) = harness(vec![{
            let mut r = qualified_row("BTC", "hyperliquid", "lighter");
            r.funding_unknown = true;
            r
        }]);
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.enter_calls(), 0);

        // Blocking ledger signal collides.
        let (state, bot) = harness(vec![qualified_row("BTC", "hyperliquid", "lighter")]);
        record_radar_ledger(&state, "sig-block-1", "BTC", "hyperliquid", "lighter", SignalStatus::InFlight);
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.enter_calls(), 0);

        // Concurrent-position cap reached.
        let (state, bot) = harness(vec![qualified_row("ETH", "hyperliquid", "lighter")]);
        let mut tight = armed_config();
        tight.max_concurrent_positions = 1;
        OpportunityExecutor::save_config(&state, &tight).expect("save tight");
        bot.open.lock().insert(
            PairKey::new("BTC", "hyperliquid", "lighter"),
            TrackedPosition {
                entry_ts_ms: 0,
                entry_dev_bps: 10.0,
                baseline_bps: Some(2.0),
                friction_band_bps: 4.0,
                size_usd: 100.0,
                deadline_ms: i64::MAX / 2,
                unwatched_since_ms: None,
                last_seen_ms: 0,
            },
        );
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.enter_calls(), 0);

        // Entry-dispatch failure becomes an audited skip, not a position.
        let (state, bot) = harness(vec![qualified_row("BTC", "hyperliquid", "lighter")]);
        let dispatch = MockDispatch {
            enter_results: StdMutex::new(vec![Err("simulated reject".to_string())]),
            exit_results: StdMutex::new(vec![]),
            enters: StdMutex::new(vec![]),
            exits: StdMutex::new(vec![]),
        };
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(bot.open_count(), 0);
        assert!(bot_events(&state, "bot_skip").iter().any(|e| e.economics_json
            .as_deref()
            .unwrap_or("")
            .contains("entry rejected")));
    }

    #[tokio::test]
    async fn exit_on_convergence_stop_and_timeout() {
        // Convergence: live dev inside the friction band.
        let (state, bot) = harness(vec![{
            let mut r = qualified_row("BTC", "hyperliquid", "lighter");
            r.deviation_bps = Some(1.0);
            r
        }]);
        bot.open.lock().insert(
            PairKey::new("BTC", "hyperliquid", "lighter"),
            TrackedPosition {
                entry_ts_ms: 0,
                entry_dev_bps: 10.0,
                baseline_bps: Some(2.0),
                friction_band_bps: 4.0,
                size_usd: 100.0,
                deadline_ms: i64::MAX / 2,
                unwatched_since_ms: None,
                last_seen_ms: 0,
            },
        );
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.exit_calls(), 1);
        assert_eq!(bot.open_count(), 0);
        let exits = bot_events(&state, "bot_exit");
        assert_eq!(exits.len(), 1);
        assert!(exits[0].economics_json.as_deref().unwrap_or("").contains("converged"));

        // Stop: live dev past 1.5x entry.
        let (state, bot) = harness(vec![{
            let mut r = qualified_row("BTC", "hyperliquid", "lighter");
            r.deviation_bps = Some(16.0);
            r
        }]);
        bot.open.lock().insert(
            PairKey::new("BTC", "hyperliquid", "lighter"),
            TrackedPosition {
                entry_ts_ms: 0,
                entry_dev_bps: 10.0,
                baseline_bps: Some(2.0),
                friction_band_bps: 4.0,
                size_usd: 100.0,
                deadline_ms: i64::MAX / 2,
                unwatched_since_ms: None,
                last_seen_ms: 0,
            },
        );
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.exit_calls(), 1);
        assert!(bot_events(&state, "bot_exit")[0]
            .economics_json
            .as_deref()
            .unwrap_or("")
            .contains("stopped"));

        // Timeout: mid-band dev, deadline in the past.
        let (state, bot) = harness(vec![qualified_row("BTC", "hyperliquid", "lighter")]);
        bot.open.lock().insert(
            PairKey::new("BTC", "hyperliquid", "lighter"),
            TrackedPosition {
                entry_ts_ms: 0,
                entry_dev_bps: 10.0,
                baseline_bps: Some(2.0),
                friction_band_bps: 4.0,
                size_usd: 100.0,
                deadline_ms: 1,
                unwatched_since_ms: None,
                last_seen_ms: 0,
            },
        );
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.exit_calls(), 1);
        assert!(bot_events(&state, "bot_exit")[0]
            .economics_json
            .as_deref()
            .unwrap_or("")
            .contains("timed_out"));

        // Failed exit keeps the position and surfaces the error.
        let (state, bot) = harness(vec![{
            let mut r = qualified_row("BTC", "hyperliquid", "lighter");
            r.deviation_bps = Some(1.0);
            r
        }]);
        bot.open.lock().insert(
            PairKey::new("BTC", "hyperliquid", "lighter"),
            TrackedPosition {
                entry_ts_ms: 0,
                entry_dev_bps: 10.0,
                baseline_bps: Some(2.0),
                friction_band_bps: 4.0,
                size_usd: 100.0,
                deadline_ms: i64::MAX / 2,
                unwatched_since_ms: None,
                last_seen_ms: 0,
            },
        );
        let dispatch = MockDispatch {
            enter_results: StdMutex::new(vec![]),
            exit_results: StdMutex::new(vec![Err("flatten blew up".to_string())]),
            enters: StdMutex::new(vec![]),
            exits: StdMutex::new(vec![]),
        };
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(bot.open_count(), 1, "failed exit must retain the watcher");
        assert!(bot.status(&state).last_error.is_some());
    }

    #[tokio::test]
    async fn signal_gone_midflight_holds_then_stale_unwinds() {
        // No rows at all: position goes unwatched, no exit yet.
        let (state, bot) = harness(vec![]);
        bot.open.lock().insert(
            PairKey::new("BTC", "hyperliquid", "lighter"),
            TrackedPosition {
                entry_ts_ms: 0,
                entry_dev_bps: 10.0,
                baseline_bps: Some(2.0),
                friction_band_bps: 4.0,
                size_usd: 100.0,
                deadline_ms: i64::MAX / 2,
                unwatched_since_ms: None,
                last_seen_ms: 0,
            },
        );
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.exit_calls(), 0);
        assert_eq!(bot.open_count(), 1);
        assert!(bot.status(&state).open_positions[0].unwatched);

        // Stale past the grace window unwinds reduce-only.
        let (state, bot) = harness(vec![]);
        bot.open.lock().insert(
            PairKey::new("BTC", "hyperliquid", "lighter"),
            TrackedPosition {
                entry_ts_ms: 0,
                entry_dev_bps: 10.0,
                baseline_bps: Some(2.0),
                friction_band_bps: 4.0,
                size_usd: 100.0,
                deadline_ms: i64::MAX / 2,
                unwatched_since_ms: Some(Utc::now().timestamp_millis() - 700_000),
                last_seen_ms: 0,
            },
        );
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.exit_calls(), 1);
        assert!(bot_events(&state, "bot_exit")[0]
            .economics_json
            .as_deref()
            .unwrap_or("")
            .contains("stale_unwind"));
    }

    #[tokio::test]
    async fn cooldown_blocks_immediate_reentry() {
        let (state, bot) = harness(vec![qualified_row("BTC", "hyperliquid", "lighter")]);
        bot.cooldowns.lock().insert(
            PairKey::new("BTC", "hyperliquid", "lighter"),
            Utc::now().timestamp_millis(),
        );
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.enter_calls(), 0);
        assert!(bot_events(&state, "bot_skip").iter().any(|e| e.economics_json
            .as_deref()
            .unwrap_or("")
            .contains("cooling down")));
    }

    #[tokio::test]
    async fn paused_mode_runs_exits_but_no_entries() {
        let mut dto = armed_config();
        dto.mode = "paused".to_string();
        let (state, bot) = harness(vec![{
            let mut r = qualified_row("BTC", "hyperliquid", "lighter");
            r.deviation_bps = Some(1.0);
            r
        }]);
        OpportunityExecutor::save_config(&state, &dto).expect("save paused");
        // Open position converges -> exit fires even while paused.
        bot.open.lock().insert(
            PairKey::new("BTC", "hyperliquid", "lighter"),
            TrackedPosition {
                entry_ts_ms: 0,
                entry_dev_bps: 10.0,
                baseline_bps: Some(2.0),
                friction_band_bps: 4.0,
                size_usd: 100.0,
                deadline_ms: i64::MAX / 2,
                unwatched_since_ms: None,
                last_seen_ms: 0,
            },
        );
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.exit_calls(), 1);
        // Fresh qualified pair in the same tick must NOT enter while paused.
        let (state2, bot2) = harness(vec![qualified_row("ETH", "hyperliquid", "lighter")]);
        OpportunityExecutor::save_config(&state2, &dto).expect("save paused");
        let dispatch2 = MockDispatch::ok();
        bot2.tick_once(&state2, &dispatch2).await;
        assert_eq!(dispatch2.enter_calls(), 0);
        let _ = bot;
    }

    #[tokio::test]
    async fn restart_recovery_keeps_backed_positions_and_drops_orphans() {
        let state = AppEngineState::new();
        OpportunityExecutor::save_config(&state, &armed_config()).expect("save armed");
        let now = Utc::now().timestamp_millis();
        // Backed by an open radar ledger signal -> survives.
        record_radar_ledger(&state, "sig-keep-1", "BTC", "hyperliquid", "lighter", SignalStatus::Balanced);
        state
            .state_store
            .upsert_bot_position(&BotPositionRow {
                symbol: "BTC".to_string(),
                buy_venue: "hyperliquid".to_string(),
                sell_venue: "lighter".to_string(),
                entry_ts_ms: now,
                entry_dev_bps: 10.0,
                baseline_bps: Some(2.0),
                friction_band_bps: 4.0,
                size_usd: 100.0,
                deadline_ms: now + 360_000,
                unwatched_since_ms: None,
                last_seen_ms: now,
            })
            .expect("persist kept");
        // No ledger signal -> orphan, dropped with audit.
        state
            .state_store
            .upsert_bot_position(&BotPositionRow {
                symbol: "ETH".to_string(),
                buy_venue: "hyperliquid".to_string(),
                sell_venue: "lighter".to_string(),
                entry_ts_ms: now,
                entry_dev_bps: 9.0,
                baseline_bps: Some(1.0),
                friction_band_bps: 3.0,
                size_usd: 50.0,
                deadline_ms: now + 360_000,
                unwatched_since_ms: None,
                last_seen_ms: now,
            })
            .expect("persist orphan");
        let bot = OpportunityExecutor::new();
        bot.rehydrate(&state);
        assert_eq!(bot.open_count(), 1);
        assert!(bot.open.lock().contains_key(&PairKey::new("BTC", "hyperliquid", "lighter")));
        let remaining = state.state_store.list_bot_positions().unwrap_or_default();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].symbol, "BTC");
        assert!(bot_events(&state, "bot_exit").iter().any(|e| e.symbol == "ETH"));
    }

    #[tokio::test]
    async fn corrupt_config_fails_closed_to_inert() {
        let (state, bot) = harness(vec![qualified_row("BTC", "hyperliquid", "lighter")]);
        state
            .state_store
            .save_radar_bot_config_json("{\"enabled\":true,\"mode\":\"auto\"}")
            .expect("save partial");
        // Missing knobs -> invalid -> inert, no entries.
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.enter_calls(), 0);
        // Invalid mode rejected on save, previous config untouched.
        let mut bad = armed_config();
        bad.mode = "yolo".to_string();
        assert!(OpportunityExecutor::save_config(&state, &bad).is_err());
        let mut risky = armed_config();
        risky.min_p_win = 0.1;
        assert!(OpportunityExecutor::save_config(&state, &risky).is_err());
    }

    #[tokio::test]
    async fn audit_trail_covers_enter_skip_exit_with_belief() {
        let (state, bot) = harness(vec![
            qualified_row("BTC", "hyperliquid", "lighter"),
            {
                let mut r = qualified_row("ETH", "hyperliquid", "lighter");
                r.p_win = Some(0.5);
                r
            },
        ]);
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(bot_events(&state, "bot_enter").len(), 1);
        assert_eq!(bot_events(&state, "bot_skip").len(), 1);
        for e in bot_events(&state, "bot_enter")
            .into_iter()
            .chain(bot_events(&state, "bot_skip"))
        {
            assert_eq!(e.trigger, "bot_decision");
            let econ: serde_json::Value =
                serde_json::from_str(e.economics_json.as_deref().unwrap_or("{}"))
                    .expect("snapshot is valid json");
            assert!(econ.get("pWin").is_some());
            assert!(econ.get("expectedProfitBps").is_some());
            assert!(econ.get("optimalSizeUsd").is_some());
        }
        // Status reflects durable counts and the last decision.
        let st = bot.status(&state);
        assert_eq!(st.enters_today, 1);
        assert_eq!(st.skips_today, 1);
        assert_eq!(st.open_positions.len(), 1);
        assert!(st.last_decision.is_some());
    }
}
