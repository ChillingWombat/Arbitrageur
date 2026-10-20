//! OpportunityExecutor — autonomous convergence bot engine (GitHub issue #5).
//!
//! Opt-in, testnet consumer of the spread monitor's ready-opportunity
//! seam. One background task polls `ready_opportunities()`, runs every
//! candidate through the pure gate pipeline in `arbitrageur_strategy`, sizes
//! from the capacity ladder, and dispatches entries through the SAME guarded
//! primitive as the manual row-Execute button
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
//! - Live mode enforces elevated risk hurdles (expected profit >= 25.0 bps, P_win >= 90%) with fail-closed safety.
//! - A corrupt persisted config falls back to inert, never to permissive.
//! - Audit-write failures are surfaced in status, never block risk exits.
//! - Flatten is symbol-scoped: at most one radar position per symbol can
//!   exist (the dispatch path rejects a second position on the same
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

use arbitrageur_strategy::convergence_auto::{
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
/// together, apart from Simple Spread ("futures").
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
    #[serde(default = "default_invalidation_p_win")]
    pub invalidation_p_win: f64,
}

fn default_invalidation_p_win() -> f64 {
    0.50
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
            invalidation_p_win: cfg.invalidation_p_win,
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
            invalidation_p_win: self.invalidation_p_win,
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
    /// Wall-clock of the last poll iteration — proves the loop is alive even
    /// when no decision has ever fired.
    pub last_tick_ms: Option<i64>,
    /// Ready+executable candidate count on the last poll.
    pub ready_rows: u64,
    /// `triggered`-status rows on the last poll (active signals still inside
    /// the qualification gates).
    pub triggered_rows: u64,
}

/// Execution seam. Production calls the existing guarded dispatch path; tests
/// inject scripted mocks. Testnet-only by construction: both callees
/// hard-block live mode themselves (autonomous entries need manual gating
/// that only the operator path provides).
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
        key: &'a PairKey,
        trigger: ExitTrigger,
        state: &'a AppEngineState,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;
}

/// Production dispatch: entries reuse `execute_monitor_opportunity_impl`
/// (identical gates to the manual button); exits reuse the route-scoped
/// or symbol-scoped flatten path for the `radar` strategy.
pub struct VenueDispatch;

impl BotDispatch for VenueDispatch {
    fn enter<'a>(
        &'a self,
        symbol: &'a str,
        buy_venue: &'a str,
        sell_venue: &'a str,
        size_usd: f64,
        state: &'a AppEngineState,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            let res = crate::commands::execute_monitor_opportunity_for_bot_impl(
                symbol, buy_venue, sell_venue, size_usd, state,
            )
            .await?;
            if !res.execution_success {
                return Err(res.message);
            }
            Ok(res.message)
        })
    }

    fn exit<'a>(
        &'a self,
        key: &'a PairKey,
        trigger: ExitTrigger,
        state: &'a AppEngineState,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            let close_status = match trigger {
                ExitTrigger::Converged => {
                    arbitrageur_execution::virtual_ledger::CycleStatus::Closed
                }
                ExitTrigger::TimedOut => {
                    arbitrageur_execution::virtual_ledger::CycleStatus::TimedOut
                }
                // Stopped + StaleUnwind: adverse unwind, not a target exit.
                _ => arbitrageur_execution::virtual_ledger::CycleStatus::StoppedOut,
            };

            // First, check if there are specific open root signals for this exact pair route
            // (symbol + buy_venue + sell_venue). Closing by signal ID guarantees that
            // another route on the same symbol (e.g. ONDO hyperliquid→vanta vs ONDO alpaca→vanta)
            // does not collide or cause residual signal errors.
            if let Ok(all_signals) = state.state_store.list_signals(Some(&key.symbol), None) {
                let mut matched_signal_ids = Vec::new();
                let current_env = state.env().as_str();
                for s in &all_signals {
                    if !s.id.starts_with("flat-")
                        && !s.id.starts_with("trim-")
                        && s.strategy_id.eq_ignore_ascii_case(BOT_STRATEGY_ID)
                        && s.status.counts_as_open()
                        && (s.environment.eq_ignore_ascii_case(current_env)
                            || s.environment.eq_ignore_ascii_case("unknown"))
                    {
                        let has_buy = s.legs.iter().any(|l| {
                            l.venue.eq_ignore_ascii_case(&key.buy_venue)
                                && l.side == crate::state_store::LegSide::Long
                        });
                        let has_sell = s.legs.iter().any(|l| {
                            l.venue.eq_ignore_ascii_case(&key.sell_venue)
                                && l.side == crate::state_store::LegSide::Short
                        });
                        if has_buy && has_sell {
                            matched_signal_ids.push(s.id.clone());
                        }
                    }
                }

                if !matched_signal_ids.is_empty() {
                    let mut closed_count = 0;
                    let mut notes = Vec::new();
                    for sig_id in matched_signal_ids {
                        let res = crate::execution::flatten_signal_positions_impl(
                            &sig_id,
                            BOT_STRATEGY_ID,
                            None,
                            close_status,
                            state,
                        )
                        .await?;
                        if res.signals_closed > 0 {
                            closed_count += res.signals_closed;
                        }
                        if res.residual_signals > 0 {
                            notes.push(format!(
                                "signal {sig_id} left {} residual",
                                res.residual_signals
                            ));
                        }
                    }
                    if !notes.is_empty() {
                        return Err(format!("flatten for {}: {}", key.label(), notes.join("; ")));
                    }
                    if closed_count > 0 {
                        return Ok(format!(
                            "unwound {closed_count} signal(s) for {}",
                            key.label()
                        ));
                    }
                    if notes.iter().any(|n| n.contains("awaiting settlement")) {
                        return Ok(format!(
                            "exit orders working for {} — awaiting settlement",
                            key.label()
                        ));
                    }
                }
            }

            // Fallback: ticker-scoped flatten
            let res = crate::commands::flatten_ticker_positions_impl(
                Some(&key.symbol),
                BOT_STRATEGY_ID,
                close_status,
                state,
            )
            .await?;
            if res.residual_signals > 0 {
                return Err(format!(
                    "flatten left {} residual signal(s) on {}",
                    res.residual_signals, key.symbol
                ));
            }
            if res.signals_closed > 0 {
                return Ok(format!(
                    "unwound {} signal(s) on {}",
                    res.signals_closed, key.symbol
                ));
            }
            if res.notes.iter().any(|n| n.contains("awaiting settlement")) {
                return Ok(format!(
                    "exit orders working for {} — awaiting settlement",
                    key.label()
                ));
            }
            Ok(format!("unwound 0 signals on {}", key.symbol))
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
    fn persist_row(&self, key: &PairKey, environment: &str) -> BotPositionRow {
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
            environment: environment.to_string(),
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
    /// Wall-clock of the most recent completed poll. A live bot ticks
    /// regardless of whether a decision was made; a stale tick timestamp is
    /// the honest signal that the loop is dead, not that signals are absent.
    last_tick_ms: AtomicU64,
    /// Ready+executable rows seen on the last poll — nonzero means the entry
    /// gate has live candidates, zero means the bot is correctly waiting on
    /// statistical qualification rather than being broken.
    ready_rows: AtomicU64,
    /// Rows in `triggered` on the last poll — the qualification pipeline's
    /// active-signal depth, so an empty `ready_rows` is diagnosable.
    triggered_rows: AtomicU64,
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
            last_tick_ms: AtomicU64::new(0),
            ready_rows: AtomicU64::new(0),
            triggered_rows: AtomicU64::new(0),
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
        let raw = state.state_store.radar_bot_config_json().ok().flatten();
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
        if (mode == BotMode::Auto || dto.enabled) && state.circuit_breaker.is_tripped() {
            return Err(
                "Cannot enable or arm Radar bot while circuit breaker is tripped. Reset circuit breaker first."
                    .into(),
            );
        }
        let canonical = RadarBotConfigDto::from_policy(&cfg, mode);
        let json = serde_json::to_string(&canonical).map_err(|e| format!("config encode: {e}"))?;
        state
            .state_store
            .save_radar_bot_config_json(&json)
            .map_err(|e| format!("config persist: {e}"))?;
        Ok(())
    }

    /// Adopt a manually executed position into the auto-exit watcher pool.
    ///
    /// When a trader executes an opportunity manually through the spread monitor,
    /// this registers the position with its entry deviation, current baseline,
    /// derived friction band, and max-hold deadline.
    ///
    /// If the bot is currently `Off`, it automatically transitions the bot to
    /// `ReduceOnly` mode so exit watchers run every tick without opening unwanted
    /// autonomous entries.
    #[allow(clippy::too_many_arguments)]
    pub fn adopt_manual_position(
        &self,
        state: &AppEngineState,
        symbol: &str,
        buy_venue: &str,
        sell_venue: &str,
        size_usd: f64,
        entry_dev_bps: Option<f64>,
        baseline_bps: Option<f64>,
        friction_band_bps: Option<f64>,
        expected_converge_secs: Option<f64>,
    ) -> Result<(), String> {
        let now_ms = Utc::now().timestamp_millis();
        let (cfg, _mode) = Self::load_config(state);

        let friction_band = friction_band_bps
            .filter(|b| b.is_finite() && *b >= 0.0)
            .unwrap_or(4.0);

        let deadline_ms = arbitrageur_strategy::convergence_auto::hold_deadline_ms(
            now_ms,
            expected_converge_secs,
            &cfg,
        )
        .unwrap_or_else(|| now_ms.saturating_add(86_400_000));

        let key = PairKey::new(symbol, buy_venue, sell_venue);
        let pos = TrackedPosition {
            entry_ts_ms: now_ms,
            entry_dev_bps: entry_dev_bps.unwrap_or(0.0),
            baseline_bps,
            friction_band_bps: friction_band,
            size_usd,
            deadline_ms,
            unwatched_since_ms: None,
            last_seen_ms: now_ms,
        };

        state
            .state_store
            .upsert_bot_position(&pos.persist_row(&key, state.env().as_str()))
            .map_err(|e| format!("upsert bot position: {e}"))?;

        self.open.lock().insert(key.clone(), pos.clone());

        // Reload the mode after claiming the slot: a concurrent flat-book
        // retire may have flipped the persisted config to Off between the
        // earlier load and this insert — the exit watcher must be re-armed.
        let (_, mode_now) = Self::load_config(state);
        if mode_now == BotMode::Off {
            let next_dto = RadarBotConfigDto {
                enabled: false,
                mode: "reduce_only".to_string(),
                ..RadarBotConfigDto::from_policy(&cfg, BotMode::ReduceOnly)
            };
            let _ = Self::save_config(state, &next_dto);
        }

        self.audit(
            state,
            &key,
            now_ms,
            None,
            "bot_enter",
            &format!(
                "manually executed position adopted into auto-exit watcher (targeting baseline {:?} bps)",
                baseline_bps
            ),
            None,
            entry_dev_bps,
            None,
            &serde_json::json!({
                "sizeUsd": size_usd,
                "frictionBandBps": friction_band,
                "deadlineMs": deadline_ms,
                "manual": true,
            })
            .to_string(),
        );

        Ok(())
    }

    /// Remove any tracked position matching the symbol and either venue order.
    pub fn forget_position(&self, symbol: &str, venue_a: &str, venue_b: &str) {
        let key_a = PairKey::new(symbol, venue_a, venue_b);
        let key_b = PairKey::new(symbol, venue_b, venue_a);
        let mut open = self.open.lock();
        open.remove(&key_a);
        open.remove(&key_b);
    }

    /// Restart recovery: reload persisted positions, keep only those still
    /// backed by an open `radar` ledger signal, and rebuild cooldowns from
    /// recent `bot_exit` audits. Never fabricates watchers.
    pub fn rehydrate(&self, state: &AppEngineState) {
        let now_ms = Utc::now().timestamp_millis();
        let persisted = state
            .state_store
            .list_bot_positions(state.env().as_str())
            .unwrap_or_default();
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
                let _ = state.state_store.delete_bot_position(
                    &row.symbol,
                    &row.buy_venue,
                    &row.sell_venue,
                    state.env().as_str(),
                );
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
        if let Ok(events) = state.state_store.recent_monitor_signal_events(
            None,
            None,
            None,
            REBUILD_SCAN_LIMIT,
            state.env().as_str(),
        ) {
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
        let current_env = state.env().as_str();
        all.iter()
            .filter(|s| {
                s.strategy_id.eq_ignore_ascii_case(BOT_STRATEGY_ID)
                    && s.status.counts_as_open()
                    && (s.environment.eq_ignore_ascii_case(current_env)
                        || s.environment.eq_ignore_ascii_case("unknown"))
            })
            .any(|s| {
                let venues: Vec<String> = s.legs.iter().map(|l| l.venue.to_lowercase()).collect();
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
            state.env().as_str(),
        );
        if res.is_err() {
            self.audit_errors.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                "radar bot: audit write failed for {} ({decision})",
                key.label()
            );
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
            p_win: row.win_probability.or(row.p_win),
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
        let cand_buy = signal.buy_venue.to_lowercase();
        let cand_sell = signal.sell_venue.to_lowercase();
        let cand_sym = signal.symbol.to_uppercase();
        for (k, p) in open.iter() {
            if k.buy_venue == cand_buy || k.sell_venue == cand_buy {
                buy_notional += p.size_usd;
            }
            if k.buy_venue == cand_sell || k.sell_venue == cand_sell {
                sell_notional += p.size_usd;
            }
            if k.symbol == cand_sym {
                sym_notional += p.size_usd;
            }
        }
        let positions = open.len();
        drop(open);
        let key = PairKey::new(&signal.symbol, &signal.buy_venue, &signal.sell_venue);
        let cooldown_remaining_secs =
            self.cooldowns.lock().get(&key).map(|last| {
                (cfg.pair_cooldown_secs as i64 * 1000 - (now_ms - last)) as f64 / 1000.0
            });
        // DB errors fail closed: an unreadable ledger blocks the symbol.
        let blocking_signal = state
            .state_store
            .ticker_blocked(&signal.symbol, Some(state.env().as_str()))
            .unwrap_or(true)
            || state
                .state_store
                .ticker_reduce_only(&signal.symbol, BOT_STRATEGY_ID, state.env().as_str())
                .unwrap_or(true);
        let (circuit_ok, circuit_reason) = match state.circuit_breaker.is_trading_allowed() {
            Ok(()) => (true, None),
            Err(r) => (false, Some(r)),
        };
        let mut hawkes_flow_ok = true;
        let mut hawkes_flow_reason = None;
        for (venue, is_buy, direction) in [
            (signal.buy_venue.as_str(), true, "buy"),
            (signal.sell_venue.as_str(), false, "sell"),
        ] {
            let Some(verdict) = state.evaluate_hawkes_entry(venue, &signal.symbol, is_buy) else {
                continue;
            };
            if verdict.is_supercritical || !verdict.is_entry_allowed {
                hawkes_flow_ok = false;
                hawkes_flow_reason = Some(format!(
                    "{direction} leg {venue} blocked by Hawkes {:?} regime (toxicity drift {:.2} bps, burst={}, cojump={})",
                    verdict.regime,
                    verdict.toxic_drift_bps,
                    verdict.is_toxic_burst,
                    verdict.cojump_withdrawal_recommended
                ));
                break;
            }
        }
        GateContext {
            live_mode: state.is_live(),
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
            hawkes_flow_ok,
            hawkes_flow_reason,
        }
    }

    /// One poll iteration. Returns the sleep interval in seconds. Never holds
    /// a lock across dispatch awaits; per-pair failures never abort the tick.
    pub async fn tick_once(&self, state: &AppEngineState, dispatch: &dyn BotDispatch) -> u64 {
        let now_ms = Utc::now().timestamp_millis();
        self.last_tick_ms
            .store(now_ms.max(0) as u64, Ordering::Relaxed);
        let (cfg, mode) = Self::load_config(state);
        if !mode.exits_allowed() {
            // Fully inert: no entries, no exit maintenance. The manual
            // flatten path remains the kill switch.
            return cfg.poll_interval_secs.max(1);
        }
        if state.circuit_breaker.is_tripped() && mode != BotMode::Off {
            let next = RadarBotConfigDto {
                enabled: false,
                mode: BotMode::Off.as_str().to_string(),
                ..RadarBotConfigDto::from_policy(&cfg, BotMode::Off)
            };
            if let Err(e) = Self::save_config(state, &next) {
                tracing::warn!("radar bot: circuit breaker trip retire to Off failed: {e}");
            }
            return cfg.poll_interval_secs.max(1);
        }
        // Reduce-only retires to Off once the book is flat: nothing remains
        // to drain, so the persisted mode drops to Off and the loop goes
        // inert until a manual adoption re-arms it. The open-map lock spans
        // the empty-check and the persist so `adopt_manual_position` cannot
        // slip an unwatched position into an Off bot between the two.
        if mode == BotMode::ReduceOnly {
            let open = self.open.lock();
            if open.is_empty() {
                let (_, mode_now) = Self::load_config(state);
                if mode_now == BotMode::ReduceOnly {
                    let next = RadarBotConfigDto {
                        enabled: false,
                        mode: BotMode::Off.as_str().to_string(),
                        ..RadarBotConfigDto::from_policy(&cfg, BotMode::Off)
                    };
                    if let Err(e) = Self::save_config(state, &next) {
                        tracing::warn!("radar bot: flat-book retire to Off failed: {e}");
                    }
                }
                drop(open);
                return cfg.poll_interval_secs.max(1);
            }
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
                    // The monitor's *current* rolling baseline. Paired with
                    // the baseline captured at entry it lets the exit policy
                    // re-base the live deviation into the entry frame
                    // (issue #116) instead of comparing across two frames.
                    baseline_bps: r.baseline_bps,
                    p_win: r.win_probability.or(r.reversion_probability).or(r.p_win),
                });
                let watched = live.as_ref().is_some_and(|l| {
                    l.status != "no-data" && l.deviation_bps.is_some_and(|d| d.is_finite())
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
                    pos.baseline_bps,
                    pos.friction_band_bps,
                    pos.deadline_ms,
                    unwatched_for,
                    live.as_ref(),
                    now_ms,
                    &cfg,
                ) {
                    ExitEval::Hold | ExitEval::Unwatched { .. } => {
                        self.open.lock().insert(key.clone(), pos.clone());
                        let _ = state
                            .state_store
                            .upsert_bot_position(&pos.persist_row(&key, state.env().as_str()));
                    }
                    ExitEval::Exit { trigger, reason } => {
                        self.exit_position(state, dispatch, &key, &pos, trigger, &reason, now_ms)
                            .await;
                    }
                }
            }
        }

        // Liveness + pipeline telemetry: the status surface must distinguish
        // "bot alive, waiting on qualification" from "bot dead". Snapshot
        // rows and ready count are sampled every tick the loop reaches.
        let ready = if snapshot_present {
            eng.ready_opportunities()
        } else {
            Vec::new()
        };
        self.ready_rows.store(ready.len() as u64, Ordering::Relaxed);
        self.triggered_rows.store(
            snap.as_ref()
                .map(|s| s.rows.iter().filter(|r| r.status == "triggered").count() as u64)
                .unwrap_or(0),
            Ordering::Relaxed,
        );

        // Entries only when explicitly armed in full-auto. Anything else
        // (paused, reduce-only, disabled flag) skips entries silently —
        // denial audits would otherwise spam while disarmed.
        if cfg.enabled && snapshot_present {
            for row in ready {
                let signal = Self::signal_of(&row);
                let key = PairKey::new(&signal.symbol, &signal.buy_venue, &signal.sell_venue);
                if self.open.lock().contains_key(&key) {
                    self.audit_skip(state, &key, now_ms, &signal, "position already open", None);
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
                    self.audit_skip(
                        state,
                        &key,
                        now_ms,
                        &signal,
                        &verdict.reason,
                        Some(size_usd),
                    );
                    continue;
                }
                let plan = match plan_entry(&signal, &cfg, now_ms) {
                    Ok(p) => p,
                    Err(e) => {
                        self.audit_skip(state, &key, now_ms, &signal, &e, Some(size_usd));
                        continue;
                    }
                };
                let min_venue_notional =
                    match (signal.buy_venue.as_str(), signal.sell_venue.as_str()) {
                        (b, s)
                            if b == "vanta"
                                || s == "vanta"
                                || b == "hyperliquid"
                                || s == "hyperliquid"
                                || b == "lighter"
                                || s == "lighter" =>
                        {
                            10.0
                        }
                        _ => 5.0,
                    };
                if plan.size_usd < min_venue_notional {
                    self.audit_skip(
                        state,
                        &key,
                        now_ms,
                        &signal,
                        &format!(
                            "size ${:.2} below venue minimum notional ${:.2}",
                            plan.size_usd, min_venue_notional
                        ),
                        Some(plan.size_usd),
                    );
                    continue;
                }
                match dispatch
                    .enter(
                        &signal.symbol,
                        &signal.buy_venue,
                        &signal.sell_venue,
                        plan.size_usd,
                        state,
                    )
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
                        let _ = state
                            .state_store
                            .upsert_bot_position(&pos.persist_row(&key, state.env().as_str()));
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
                        self.cooldowns.lock().insert(key.clone(), now_ms);
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
        match dispatch.exit(key, trigger, state).await {
            Ok(note) => {
                *self.last_error.lock() = None;
                self.open.lock().remove(key);
                let _ = state.state_store.delete_bot_position(
                    &key.symbol,
                    &key.buy_venue,
                    &key.sell_venue,
                    state.env().as_str(),
                );
                let exit_json = serde_json::json!({
                    "trigger": trigger.as_str(),
                    "sizeUsd": pos.size_usd,
                    "entryDevBps": pos.entry_dev_bps,
                    "detail": note,
                })
                .to_string();
                self.audit(
                    state, key, now_ms, None, "bot_exit", reason, None, None, None, &exit_json,
                );
            }
            Err(e) => {
                *self.last_error.lock() = Some(format!("exit failed for {}: {e}", key.label()));
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
        if let Ok(counts) = state
            .state_store
            .count_bot_decisions_since(day_start_ms, state.env().as_str())
        {
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
            monitor_running: engine
                .as_ref()
                .map(|e| e.snapshot().is_some())
                .unwrap_or(false),
            snapshot_present: snap.is_some(),
            last_tick_ms: {
                let t = self.last_tick_ms.load(Ordering::Relaxed);
                (t > 0).then_some(t as i64)
            },
            ready_rows: self.ready_rows.load(Ordering::Relaxed),
            triggered_rows: self.triggered_rows.load(Ordering::Relaxed),
        }
    }

    pub async fn run(self: Arc<Self>, state: Arc<AppEngineState>) {
        self.rehydrate(&state);
        loop {
            if *self.shutdown.lock() {
                return;
            }
            let wait_secs = self.tick_once(&state, &VenueDispatch).await;
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
            last_tick_ms: AtomicU64::new(0),
            ready_rows: AtomicU64::new(0),
            triggered_rows: AtomicU64::new(0),
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
    use crate::monitor::{MonitorRowDto, MonitorSnapshotDto, SpreadMonitorEngine};
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
                self.enters
                    .lock()
                    .map(|mut v| {
                        v.push((
                            symbol.to_string(),
                            buy_venue.to_string(),
                            sell_venue.to_string(),
                            size_usd,
                        ))
                    })
                    .ok();
                self.enter_results
                    .lock()
                    .ok()
                    .and_then(|mut q| {
                        if q.is_empty() {
                            None
                        } else {
                            Some(q.remove(0))
                        }
                    })
                    .unwrap_or(Ok("mock fill".to_string()))
            })
        }

        fn exit<'a>(
            &'a self,
            key: &'a PairKey,
            _trigger: ExitTrigger,
            _state: &'a AppEngineState,
        ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
            Box::pin(async move {
                self.exits
                    .lock()
                    .map(|mut v| v.push(key.symbol.clone()))
                    .ok();
                self.exit_results
                    .lock()
                    .ok()
                    .and_then(|mut q| {
                        if q.is_empty() {
                            None
                        } else {
                            Some(q.remove(0))
                        }
                    })
                    .unwrap_or(Ok("mock unwind".to_string()))
            })
        }
    }

    fn ready_row(symbol: &str, buy: &str, sell: &str) -> MonitorRowDto {
        MonitorRowDto {
            symbol: symbol.to_string(),
            buy_venue: buy.to_string(),
            sell_venue: sell.to_string(),
            buy_instrument: None,
            sell_instrument: None,
            asset_carry_bps: None,
            status: "ready".to_string(),
            current_edge_bps: Some(12.0),
            baseline_bps: Some(2.0),
            deviation_bps: Some(10.0),
            net_capture_bps: Some(6.0),
            p_win: Some(0.95),
            reversion_probability: Some(0.95),
            win_probability: Some(0.95),
            win_probability_lower: Some(0.9),
            win_probability_upper: Some(0.99),
            p_win_n_effective: Some(8.0),
            p_win_censored: Some(0),
            p_win_from_model: Some(false),
            net_win_probability: Some(0.9),
            net_win_lower: Some(0.8),
            net_expected_bps: Some(5.0),
            net_deciles_bps: None,
            outcome_trials: Some(8),
            outcome_censored: Some(0),
            median_hold_secs: Some(120.0),
            median_win_secs: Some(90.0),
            expected_profit_bps: Some(5.7),
            expected_profit_usd: Some(57.0),
            optimal_size_usd: Some(10_000.0),
            breakeven_size_usd: Some(8_000.0),
            expected_capacity_usd: None,
            bps_capacity_ladder: Vec::new(),
            depth_curve: Vec::new(),
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

    fn bot_events(
        state: &AppEngineState,
        decision: &str,
    ) -> Vec<crate::state_store::MonitorSignalEventRow> {
        state
            .state_store
            .recent_monitor_signal_events(None, None, None, 10_000, state.env().as_str())
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.trigger == "bot_decision" && e.new_status == decision)
            .collect()
    }

    fn record_radar_ledger(
        state: &AppEngineState,
        id: &str,
        symbol: &str,
        buy: &str,
        sell: &str,
        status: SignalStatus,
    ) {
        state
            .state_store
            .record_signal_execution(
                &NewSignalExecution {
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
                            symbol: None,
                            intended_qty: 0.01,
                            intended_price: None,
                            unit_scale: None,
                        },
                        NewSignalLeg {
                            account_id: None,
                            venue: sell.to_string(),
                            side: LegSide::Short,
                            symbol: None,
                            intended_qty: 0.01,
                            intended_price: None,
                            unit_scale: None,
                        },
                    ],
                },
                state.env().as_str(),
            )
            .expect("record ledger signal");
        if !matches!(status, SignalStatus::InFlight) {
            state
                .state_store
                .update_signal_status(id, status, None)
                .expect("set ledger status");
        }
    }

    #[tokio::test]
    async fn entry_on_ready_opens_position_and_audits() {
        let (state, bot) = harness(vec![ready_row("BTC", "hyperliquid", "lighter")]);
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.enter_calls(), 1);
        assert_eq!(bot.open_count(), 1);
        assert_eq!(
            state
                .state_store
                .list_bot_positions(state.env().as_str())
                .unwrap_or_default()
                .len(),
            1
        );
        let enters = bot_events(&state, "bot_enter");
        assert_eq!(enters.len(), 1);
        let e = &enters[0];
        assert_eq!(e.p_win, Some(0.95));
        let econ: serde_json::Value =
            serde_json::from_str(e.economics_json.as_deref().unwrap_or("{}"))
                .expect("valid snapshot json");
        assert_eq!(econ["pWin"], serde_json::json!(0.95));
        assert_eq!(econ["sizeUsd"], serde_json::json!(1000.0));
    }

    #[tokio::test]
    async fn second_tick_does_not_double_enter() {
        let (state, bot) = harness(vec![ready_row("BTC", "hyperliquid", "lighter")]);
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(
            dispatch.enter_calls(),
            1,
            "same pair must not re-enter while open"
        );
        assert_eq!(bot.open_count(), 1);
    }

    #[tokio::test]
    async fn skip_reasons_cover_every_gate() {
        // Disabled config: no entries, no audits (fully inert).
        let (state, bot) = harness(vec![ready_row("BTC", "hyperliquid", "lighter")]);
        OpportunityExecutor::save_config(
            &state,
            &RadarBotConfigDto::from_policy(&AutoExecConfig::default(), BotMode::Off),
        )
        .expect("save off");
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.enter_calls(), 0);
        assert!(bot_events(&state, "bot_skip").is_empty());

        // Live mode enforces elevated risk hurdles (expected profit >= 25 bps, P_win >= 90%).
        let (state, bot) = harness(vec![ready_row("BTC", "hyperliquid", "lighter")]);
        state.set_environment(crate::state::TradingEnvironment::Live);
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.enter_calls(), 0);
        assert!(bot_events(&state, "bot_skip").iter().any(|e| e
            .economics_json
            .as_deref()
            .unwrap_or("")
            .contains("live mode: expected profit below 25.0 bps hurdle")));

        // Qualified candidate in live mode successfully executes.
        let (state_qual, bot_qual) = harness(vec![{
            let mut r = ready_row("BTC", "hyperliquid", "lighter");
            r.expected_profit_bps = Some(30.0);
            r.p_win = Some(0.95);
            r.win_probability = Some(0.95);
            r
        }]);
        state_qual.set_environment(crate::state::TradingEnvironment::Live);
        let dispatch_qual = MockDispatch::ok();
        bot_qual.tick_once(&state_qual, &dispatch_qual).await;
        assert_eq!(dispatch_qual.enter_calls(), 1);

        // Engine pause blocks.
        let (state, bot) = harness(vec![ready_row("BTC", "hyperliquid", "lighter")]);
        *state.is_paused.write() = true;
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.enter_calls(), 0);

        // Low P_win skips with the floor reason.
        let (state, bot) = harness(vec![{
            let mut r = ready_row("BTC", "hyperliquid", "lighter");
            r.p_win = Some(0.5);
            r.reversion_probability = Some(0.5);
            r.win_probability = Some(0.5);
            r
        }]);
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.enter_calls(), 0);
        assert!(bot_events(&state, "bot_skip").iter().any(|e| e
            .economics_json
            .as_deref()
            .unwrap_or("")
            .contains("floor")));

        // Funding unknown never trades.
        let (state, bot) = harness(vec![{
            let mut r = ready_row("BTC", "hyperliquid", "lighter");
            r.funding_unknown = true;
            r
        }]);
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.enter_calls(), 0);

        // Blocking ledger signal collides.
        let (state, bot) = harness(vec![ready_row("BTC", "hyperliquid", "lighter")]);
        record_radar_ledger(
            &state,
            "sig-block-1",
            "BTC",
            "hyperliquid",
            "lighter",
            SignalStatus::InFlight,
        );
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.enter_calls(), 0);

        // Concurrent-position cap reached.
        let (state, bot) = harness(vec![ready_row("ETH", "hyperliquid", "lighter")]);
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
        let (state, bot) = harness(vec![ready_row("BTC", "hyperliquid", "lighter")]);
        let dispatch = MockDispatch {
            enter_results: StdMutex::new(vec![Err("simulated reject".to_string())]),
            exit_results: StdMutex::new(vec![]),
            enters: StdMutex::new(vec![]),
            exits: StdMutex::new(vec![]),
        };
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(bot.open_count(), 0);
        assert!(bot_events(&state, "bot_skip").iter().any(|e| e
            .economics_json
            .as_deref()
            .unwrap_or("")
            .contains("entry rejected")));
        assert!(bot
            .cooldowns
            .lock()
            .contains_key(&PairKey::new("BTC", "hyperliquid", "lighter")));

        // Subsequent tick immediately after is blocked by cooldown, not re-dispatched.
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.enter_calls(), 1);
    }

    #[tokio::test]
    async fn exit_on_convergence_stop_and_timeout() {
        // Convergence: live dev inside the friction band.
        let (state, bot) = harness(vec![{
            let mut r = ready_row("BTC", "hyperliquid", "lighter");
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
        assert!(exits[0]
            .economics_json
            .as_deref()
            .unwrap_or("")
            .contains("converged"));

        // Stop: live dev past 1.5x entry.
        let (state, bot) = harness(vec![{
            let mut r = ready_row("BTC", "hyperliquid", "lighter");
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
        let (state, bot) = harness(vec![ready_row("BTC", "hyperliquid", "lighter")]);
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
            let mut r = ready_row("BTC", "hyperliquid", "lighter");
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
        let (state, bot) = harness(vec![ready_row("BTC", "hyperliquid", "lighter")]);
        bot.cooldowns.lock().insert(
            PairKey::new("BTC", "hyperliquid", "lighter"),
            Utc::now().timestamp_millis(),
        );
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        assert_eq!(dispatch.enter_calls(), 0);
        assert!(bot_events(&state, "bot_skip").iter().any(|e| e
            .economics_json
            .as_deref()
            .unwrap_or("")
            .contains("cooling down")));
    }

    #[tokio::test]
    async fn paused_mode_runs_exits_but_no_entries() {
        let mut dto = armed_config();
        dto.mode = "paused".to_string();
        let (state, bot) = harness(vec![{
            let mut r = ready_row("BTC", "hyperliquid", "lighter");
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
        // Fresh ready pair in the same tick must NOT enter while paused.
        let (state2, bot2) = harness(vec![ready_row("ETH", "hyperliquid", "lighter")]);
        OpportunityExecutor::save_config(&state2, &dto).expect("save paused");
        let dispatch2 = MockDispatch::ok();
        bot2.tick_once(&state2, &dispatch2).await;
        assert_eq!(dispatch2.enter_calls(), 0);
        let _ = bot;
    }

    #[tokio::test]
    async fn reduce_only_retires_to_off_once_flat() {
        // Persisted reduce_only with nothing left to drain must wind the
        // bot down — exit-watching was its only job.
        let (state, bot) = harness(vec![]);
        let mut dto = armed_config();
        dto.mode = "reduce_only".to_string();
        dto.enabled = false;
        OpportunityExecutor::save_config(&state, &dto).expect("save reduce_only");
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;
        let (_, mode) = OpportunityExecutor::load_config(&state);
        assert_eq!(mode, BotMode::Off);

        // A tracked position keeps the mode armed until it drains.
        let (state, bot) = harness(vec![]);
        OpportunityExecutor::save_config(&state, &dto).expect("save reduce_only");
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
        let (_, mode) = OpportunityExecutor::load_config(&state);
        assert_eq!(mode, BotMode::ReduceOnly);
    }

    #[tokio::test]
    async fn restart_recovery_keeps_backed_positions_and_drops_orphans() {
        let state = AppEngineState::new();
        OpportunityExecutor::save_config(&state, &armed_config()).expect("save armed");
        let now = Utc::now().timestamp_millis();
        // Backed by an open radar ledger signal -> survives.
        record_radar_ledger(
            &state,
            "sig-keep-1",
            "BTC",
            "hyperliquid",
            "lighter",
            SignalStatus::Balanced,
        );
        state
            .state_store
            .upsert_bot_position(&BotPositionRow {
                environment: state.env().as_str().to_string(),
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
                environment: state.env().as_str().to_string(),
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
        assert!(bot
            .open
            .lock()
            .contains_key(&PairKey::new("BTC", "hyperliquid", "lighter")));
        let remaining = state
            .state_store
            .list_bot_positions(state.env().as_str())
            .unwrap_or_default();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].symbol, "BTC");
        assert!(bot_events(&state, "bot_exit")
            .iter()
            .any(|e| e.symbol == "ETH"));
    }

    #[tokio::test]
    async fn corrupt_config_fails_closed_to_inert() {
        let (state, bot) = harness(vec![ready_row("BTC", "hyperliquid", "lighter")]);
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
        let (state, bot) = harness(vec![ready_row("BTC", "hyperliquid", "lighter"), {
            let mut r = ready_row("ETH", "hyperliquid", "lighter");
            r.p_win = Some(0.5);
            r.reversion_probability = Some(0.5);
            r.win_probability = Some(0.5);
            r
        }]);
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

    #[tokio::test]
    async fn test_adopt_manual_position_transitions_to_reduce_only_and_exits_on_convergence() {
        let (state, bot) = harness(vec![]);
        // Explicitly set bot mode to Off
        OpportunityExecutor::save_config(
            &state,
            &RadarBotConfigDto::from_policy(&AutoExecConfig::default(), BotMode::Off),
        )
        .expect("save off");
        let (_, mode_before) = OpportunityExecutor::load_config(&state);
        assert_eq!(mode_before, BotMode::Off);

        // Adopt a manually executed position
        bot.adopt_manual_position(
            &state,
            "BTC",
            "hyperliquid",
            "lighter",
            500.0,
            Some(35.0),
            Some(20.0),
            Some(4.0),
            Some(60.0),
        )
        .expect("adopt manual position");

        // Bot mode transitioned to ReduceOnly so exits run without opening entries
        let (_, mode_after) = OpportunityExecutor::load_config(&state);
        assert_eq!(mode_after, BotMode::ReduceOnly);
        assert_eq!(bot.open_count(), 1);

        // Verify persisted to DB
        let persisted = state
            .state_store
            .list_bot_positions(state.env().as_str())
            .expect("list bot positions");
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].symbol, "BTC");
        assert_eq!(persisted[0].size_usd, 500.0);

        // Audit was recorded
        let adopted_events = bot_events(&state, "bot_enter");
        assert_eq!(adopted_events.len(), 1);
        assert!(adopted_events[0]
            .economics_json
            .as_deref()
            .unwrap_or("")
            .contains("\"manual\":true"));

        // Inject a snapshot where spread has converged to the dynamic baseline (deviation 0.5 bps <= friction band 4.0 bps)
        let engine = state.monitor.lock().clone().unwrap();
        let mut row = ready_row("BTC", "hyperliquid", "lighter");
        row.deviation_bps = Some(0.5);
        row.status = "neutral".to_string();
        engine.inject_snapshot(MonitorSnapshotDto {
            generated_ts_ms: Utc::now().timestamp_millis(),
            running: true,
            rows: vec![row],
            watchlist: vec!["BTC".to_string()],
            venues: vec![],
            exempted: vec![],
        });

        // Run tick: should trigger exit on convergence
        let dispatch = MockDispatch::ok();
        bot.tick_once(&state, &dispatch).await;

        assert_eq!(dispatch.exit_calls(), 1);
        assert_eq!(bot.open_count(), 0);

        // Exit audit is logged
        let exit_events = bot_events(&state, "bot_exit");
        assert_eq!(exit_events.len(), 1);
        assert!(exit_events[0]
            .economics_json
            .as_deref()
            .unwrap_or("")
            .contains("converged"));
    }
}
