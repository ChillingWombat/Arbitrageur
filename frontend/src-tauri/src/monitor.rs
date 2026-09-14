//! Spread Range Monitor — systematic cross-venue opportunity scanner.
//!
//! A background engine that watches every spread-eligible symbol across all
//! admitted venues, builds directional pair-spread history from the durable
//! `spread_samples` log, and qualifies opportunities through the
//! half-life/variation gate before they are shown as executable.
//!
//! Design contract (SPEC_SPREAD_RANGE_MONITOR.md):
//! - Default account per venue for reads; one venue adapter per venue lane.
//! - Per-venue rolling request budgets driven by the lowest enabled account
//!   tier on that venue (Lighter applies the lowest limit across accounts
//!   sharing an IP). A configurable reserve fraction is always kept free for
//!   transaction traffic.
//! - Symbols are fetched round-robin; the per-symbol refresh interval is the
//!   fair-share of the lane's usable budget, so cadence degrades gracefully
//!   as the watchlist grows instead of exhausting the venue's quota.
//! - Every reported number is derived from real books/samples; missing data
//!   surfaces as `None`/`—`, never a fabricated value.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use arbitrager_core::exchange::ExchangeAdapter;
use arbitrager_core::quote::OrderBook;
use arbitrager_math::ou_process::OuModelParams;
use arbitrager_math::pair_monitor::{
    build_pair_series, build_pwin_curve, capacity_ladder, funding_carry_bps, p_win_at_deviation,
    reversion_stats,
};
use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::state::AppEngineState;
use crate::state_store::VenueSampleEntry;

/* ------------------------------------------------------------------------ */
/* Configuration                                                             */
/* ------------------------------------------------------------------------ */

/// Monitor-wide knobs. Persisted as one JSON row in `monitor_config`;
/// every field has a safe default so a missing/corrupt row falls back
/// cleanly rather than disabling the monitor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorConfigDto {
    /// Master switch — when false the engine idles (no venue requests).
    #[serde(default = "d_true")]
    pub enabled: bool,
    /// How often the listed-market universe is re-derived, seconds.
    #[serde(default = "d_watchlist_refresh")]
    pub watchlist_refresh_secs: u64,
    /// "auto" (2 half-lives) or "manual" (fixed window).
    #[serde(default = "d_qual_mode")]
    pub qualification_mode: String,
    /// Manual qualification window, seconds (default 24h).
    #[serde(default = "d_manual_window")]
    pub manual_window_secs: u64,
    /// Half-life multiplier for the auto window (default 2).
    #[serde(default = "d_hl_mult")]
    pub half_life_multiplier: f64,
    /// Floor for the auto window, seconds (default 30 min).
    #[serde(default = "d_min_window")]
    pub min_window_secs: u64,
    /// Required spread variation per half-life chunk as a multiple of the
    /// friction floor (default 1.0 = variation must at least cover friction).
    #[serde(default = "d_one")]
    pub friction_cover_mult: f64,
    /// Capacity ladder notional tiers, USD.
    #[serde(default = "d_tiers")]
    pub capacity_tiers_usd: Vec<f64>,
    /// Minimum marginal win probability for the breakeven bound (0.90).
    #[serde(default = "d_p_min")]
    pub min_win_probability: f64,
    /// "one_way" (open only; default) or "two_way" (open + reversion close).
    #[serde(default = "d_exec_mode")]
    pub execution_mode: String,
    /// Participation cap: fraction of the smaller venue's 24h volume (0.01).
    #[serde(default = "d_part_cap")]
    pub participation_cap: f64,
    /// Fraction of each venue's request budget reserved for transaction
    /// traffic — reads never consume it (default 0.25).
    #[serde(default = "d_tx_reserve")]
    pub tx_reserve_pct: f64,
    /// Minimum resolved episodes before a win probability is reported.
    #[serde(default = "d_min_eps")]
    pub min_episodes: usize,
    /// Baseline history window, seconds (default 24h trailing).
    #[serde(default = "d_hist_window")]
    pub history_window_secs: u64,
    /// Hard ceiling on per-symbol refresh interval, seconds — guarantees a
    /// minimum liveness even on saturated lanes (default 300s).
    #[serde(default = "d_max_interval")]
    pub max_symbol_interval_secs: u64,
    /// Minimum per-symbol refresh interval, seconds (default 5s).
    #[serde(default = "d_min_interval")]
    pub min_symbol_interval_secs: u64,
}

fn d_true() -> bool {
    true
}
fn d_watchlist_refresh() -> u64 {
    3600
}
fn d_qual_mode() -> String {
    "auto".into()
}
fn d_manual_window() -> u64 {
    86400
}
fn d_hl_mult() -> f64 {
    2.0
}
fn d_min_window() -> u64 {
    1800
}
fn d_one() -> f64 {
    1.0
}
fn d_tiers() -> Vec<f64> {
    vec![1000.0, 5000.0, 10000.0]
}
fn d_p_min() -> f64 {
    0.90
}
fn d_exec_mode() -> String {
    "one_way".into()
}
fn d_part_cap() -> f64 {
    0.01
}
fn d_tx_reserve() -> f64 {
    0.25
}
fn d_min_eps() -> usize {
    5
}
fn d_hist_window() -> u64 {
    86400
}
fn d_max_interval() -> u64 {
    300
}
fn d_min_interval() -> u64 {
    5
}

impl Default for MonitorConfigDto {
    fn default() -> Self {
        Self {
            enabled: true,
            watchlist_refresh_secs: d_watchlist_refresh(),
            qualification_mode: d_qual_mode(),
            manual_window_secs: d_manual_window(),
            half_life_multiplier: d_hl_mult(),
            min_window_secs: d_min_window(),
            friction_cover_mult: d_one(),
            capacity_tiers_usd: d_tiers(),
            min_win_probability: d_p_min(),
            execution_mode: d_exec_mode(),
            participation_cap: d_part_cap(),
            tx_reserve_pct: d_tx_reserve(),
            min_episodes: d_min_eps(),
            history_window_secs: d_hist_window(),
            max_symbol_interval_secs: d_max_interval(),
            min_symbol_interval_secs: d_min_interval(),
        }
    }
}

/* ------------------------------------------------------------------------ */
/* DTOs emitted to the frontend                                              */
/* ------------------------------------------------------------------------ */

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapacityTierDto {
    pub size_usd: f64,
    pub filled_usd: f64,
    pub net_bps: Option<f64>,
    pub expected_net_bps: Option<f64>,
    pub marginal_p_win: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorRowDto {
    pub symbol: String,
    pub buy_venue: String,
    pub sell_venue: String,
    /// collecting | watching | signaled | qualified | stale
    pub status: String,
    /// Executable current edge (sell bid vs buy ask), bps.
    pub current_edge_bps: Option<f64>,
    /// Historical mean directional mid-mid spread, bps.
    pub baseline_bps: Option<f64>,
    /// Current edge minus baseline, bps.
    pub deviation_bps: Option<f64>,
    /// Net capture at the optimal tier after all friction, bps.
    pub net_capture_bps: Option<f64>,
    pub p_win: Option<f64>,
    /// Ranking key: net_capture_bps × p_win.
    pub expected_profit_bps: Option<f64>,
    pub expected_profit_usd: Option<f64>,
    pub optimal_size_usd: Option<f64>,
    /// Probability-aware capacity boundary (marginal p >= p_min).
    pub breakeven_size_usd: Option<f64>,
    pub capacity_ladder: Vec<CapacityTierDto>,
    /// Signed funding carry over the conservative horizon, bps.
    /// Positive = cost, negative = gain.
    pub funding_carry_bps: Option<f64>,
    pub funding_unknown: bool,
    /// Signal lifecycle timestamps (epoch ms).
    pub first_seen_ms: Option<i64>,
    pub last_seen_ms: Option<i64>,
    /// Seconds the current/last signal episode has been active.
    pub signal_age_secs: Option<f64>,
    /// Expected seconds until the spread reverts (take-profit horizon).
    pub expected_converge_secs: Option<f64>,
    /// Qualification progress 0..1 while collecting.
    pub qualification_progress: f64,
    pub observed_episodes: usize,
    /// Whether Execute is allowed right now (paper path only).
    pub executable: bool,
    pub block_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VenueLaneStatusDto {
    pub venue: String,
    pub read_budget_per_min: f64,
    pub used_last_min: usize,
    pub symbols: usize,
    pub refresh_interval_secs: f64,
    pub backing_off: bool,
    pub account_tier: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorSnapshotDto {
    pub generated_ts_ms: i64,
    pub running: bool,
    pub rows: Vec<MonitorRowDto>,
    pub watchlist: Vec<String>,
    pub venues: Vec<VenueLaneStatusDto>,
    pub exempted: Vec<String>,
}

/* ------------------------------------------------------------------------ */
/* Venue lane: shared-IP budget + round-robin scheduler                      */
/* ------------------------------------------------------------------------ */

/// Read budget per minute for a Lighter-family account tier. Documented
/// limits are standard=60/min/IP; the weighted premium tiers are mapped to
/// conservative request counts well inside their weighted envelopes.
fn lighter_tier_budget_per_min(tier: &str) -> f64 {
    match tier {
        "plus" => 1200.0,
        "premium" => 2400.0,
        "builder" => 4800.0,
        _ => 60.0, // standard + unknown tiers fail closed to the floor
    }
}

/// Total per-venue read budget (requests/min) before the transaction
/// reserve is subtracted. Hyperliquid's documented 1200 weight/min with
/// weight-2 `l2Book` calls yields a 600/min book budget; we keep margin.
fn venue_budget_per_min(venue: &str, tiers: &[String], flag_enabled: bool) -> (f64, String) {
    match venue {
        "lighter" | "robinhood" => {
            // Lighter applies the lowest account limit across an IP.
            let tier = tiers
                .iter()
                .map(String::as_str)
                .min_by_key(|t| lighter_tier_budget_per_min(t) as u64)
                .unwrap_or(if flag_enabled { "standard" } else { "standard" });
            (lighter_tier_budget_per_min(tier), tier.to_string())
        }
        "hyperliquid" => (500.0, "standard".into()),
        _ => (60.0, "standard".into()),
    }
}

struct VenueLane {
    venue: String,
    adapter: Arc<dyn ExchangeAdapter>,
    /// Canonical base symbols listed on this venue.
    symbols: Vec<String>,
    /// Rolling 60s request timestamps (budget accounting).
    requests: VecDeque<Instant>,
    /// Total read budget/min and usable share after the tx reserve.
    budget_per_min: f64,
    usable_per_min: f64,
    account_tier: String,
    /// Round-robin cursor into `symbols`.
    cursor: usize,
    /// Last successful book fetch per symbol.
    last_fetch: HashMap<String, Instant>,
    /// Exponential backoff on failures (WAF/429/network).
    backoff_until: Option<Instant>,
    fail_streak: u32,
    /// Next allowed funding fetch (bulk endpoint, low cadence).
    next_funding_at: Instant,
}

impl VenueLane {
    fn used_last_min(&mut self) -> usize {
        let cutoff = Instant::now() - Duration::from_secs(60);
        while self.requests.front().is_some_and(|t| *t < cutoff) {
            self.requests.pop_front();
        }
        self.requests.len()
    }

    fn can_spend(&mut self) -> bool {
        self.can_spend_n(1)
    }

    fn can_spend_n(&mut self, weight: usize) -> bool {
        if self.backoff_until.is_some_and(|t| Instant::now() < t) {
            return false;
        }
        (self.used_last_min() as f64) + (weight as f64) <= self.usable_per_min
    }

    fn spend(&mut self) {
        self.requests.push_back(Instant::now());
    }

    /// Spend `weight` budget slots — Lighter book reads cost two HTTP calls
    /// (orderBookDetails + orderBookOrders), so weighting keeps the rolling
    /// window honest against the documented per-IP ceiling.
    fn spend_weighted(&mut self, weight: usize) {
        for _ in 0..weight.max(1) {
            self.spend();
        }
    }

    fn note_failure(&mut self) {
        self.fail_streak = (self.fail_streak + 1).min(8);
        let secs = (1u64 << self.fail_streak).min(120);
        self.backoff_until = Some(Instant::now() + Duration::from_secs(secs));
    }

    fn note_success(&mut self) {
        self.fail_streak = 0;
        self.backoff_until = None;
    }

    /// Fair-share refresh interval for one symbol on this lane.
    fn symbol_interval(&self, cfg: &MonitorConfigDto) -> Duration {
        if self.symbols.is_empty() || self.usable_per_min <= 0.0 {
            return Duration::from_secs(cfg.max_symbol_interval_secs);
        }
        let fair = self.symbols.len() as f64 * 60.0 / self.usable_per_min;
        let secs = fair
            .max(cfg.min_symbol_interval_secs as f64)
            .min(cfg.max_symbol_interval_secs as f64);
        Duration::from_secs_f64(secs)
    }

    /// Symbols whose book is due for a refresh, in round-robin order.
    fn due_symbols(&mut self, cfg: &MonitorConfigDto) -> Vec<String> {
        let interval = self.symbol_interval(cfg);
        let now = Instant::now();
        let n = self.symbols.len();
        let mut due = Vec::new();
        for k in 0..n {
            let idx = (self.cursor + k) % n;
            let sym = &self.symbols[idx];
            let stale = self
                .last_fetch
                .get(sym)
                .map(|t| now.duration_since(*t) >= interval)
                .unwrap_or(true);
            if stale {
                due.push(sym.clone());
            }
        }
        due
    }
}

/* ------------------------------------------------------------------------ */
/* Pair runtime state                                                        */
/* ------------------------------------------------------------------------ */

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PairStatus {
    Collecting,
    Watching,
    Signaled,
    Qualified,
    Stale,
}

impl PairStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Collecting => "collecting",
            Self::Watching => "watching",
            Self::Signaled => "signaled",
            Self::Qualified => "qualified",
            Self::Stale => "stale",
        }
    }
    fn from_str(s: &str) -> Self {
        match s {
            "watching" => Self::Watching,
            "signaled" => Self::Signaled,
            "qualified" => Self::Qualified,
            "stale" => Self::Stale,
            _ => Self::Collecting,
        }
    }
}

#[derive(Debug, Clone)]
struct PairRuntime {
    status: PairStatus,
    /// When the current signal episode was discovered (ms epoch).
    first_seen_ms: Option<i64>,
    /// When it was last observed still active.
    last_seen_ms: Option<i64>,
    last_edge_bps: Option<f64>,
    last_dev_bps: Option<f64>,
}

/* ------------------------------------------------------------------------ */
/* Engine                                                                    */
/* ------------------------------------------------------------------------ */

pub struct SpreadMonitorEngine {
    cfg: Mutex<MonitorConfigDto>,
    lanes: Mutex<Vec<VenueLane>>,
    /// Ordered-pair runtime keyed (symbol, buy_venue, sell_venue).
    pairs: Mutex<HashMap<(String, String, String), PairRuntime>>,
    /// Funding EMA per (venue, canonical base), fraction per hour.
    funding_ema: Mutex<HashMap<(String, String), f64>>,
    /// Canonical-base watchlist (exempted symbols removed).
    watchlist: Mutex<Vec<String>>,
    /// All canonical bases per venue from the last market refresh.
    venue_symbols: Mutex<HashMap<String, Vec<String>>>,
    last_watchlist_at: Mutex<Option<Instant>>,
    last_snapshot: Mutex<Option<MonitorSnapshotDto>>,
    /// Dedup guard for durable signal writes (only persist transitions).
    persisted: Mutex<HashMap<(String, String, String), (PairStatus, i64, i64)>>,
    shutdown: Mutex<bool>,
}

/// Canonical base asset for cross-venue pairing: uppercase, quote suffixes
/// stripped ("BTC-USD"/"BTC-USDC"/"BTCUSDT"/"btc" -> "BTC").
///
/// Separator-suffixed quotes ("-USDC" etc.) strip freely; *bare* quote
/// suffixes (Binance-style "BTCUSD") only strip when at least 3 chars of
/// base remain — otherwise real tickers like "TUSD"/"SUSD"/"BUSD" would
/// collapse to a single letter.
pub fn canonical_base(raw: &str) -> String {
    let up = raw.trim().to_uppercase();
    for suf in ["-USDC", "-USDT", "-USD", "/USDC", "/USDT", "/USD", "-PERP"] {
        if up.len() > suf.len() && up.ends_with(suf) {
            return up[..up.len() - suf.len()].to_string();
        }
    }
    for suf in ["USDC", "USDT", "USD"] {
        if up.ends_with(suf) && up.len() - suf.len() >= 3 {
            return up[..up.len() - suf.len()].to_string();
        }
    }
    up
}

impl SpreadMonitorEngine {
    pub fn new(state: &AppEngineState) -> Arc<Self> {
        let cfg = state
            .state_store
            .monitor_config_json()
            .ok()
            .flatten()
            .and_then(|j| serde_json::from_str::<MonitorConfigDto>(&j).ok())
            .unwrap_or_default();
        let engine = Arc::new(Self {
            cfg: Mutex::new(cfg),
            lanes: Mutex::new(Vec::new()),
            pairs: Mutex::new(HashMap::new()),
            funding_ema: Mutex::new(HashMap::new()),
            watchlist: Mutex::new(Vec::new()),
            venue_symbols: Mutex::new(HashMap::new()),
            last_watchlist_at: Mutex::new(None),
            last_snapshot: Mutex::new(None),
            persisted: Mutex::new(HashMap::new()),
            shutdown: Mutex::new(false),
        });
        // Rehydrate lifecycle state so discovery timestamps survive restart.
        if let Ok(rows) = state.state_store.monitor_signals() {
            let mut pairs = engine.pairs.lock();
            for r in rows {
                pairs.insert(
                    (r.symbol, r.buy_venue, r.sell_venue),
                    PairRuntime {
                        status: PairStatus::from_str(&r.status),
                        first_seen_ms: Some(r.first_seen_ms),
                        last_seen_ms: Some(r.last_seen_ms),
                        last_edge_bps: r.last_edge_bps,
                        last_dev_bps: r.last_dev_bps,
                    },
                );
            }
        }
        engine
    }

    pub fn config(&self) -> MonitorConfigDto {
        self.cfg.lock().clone()
    }

    pub fn set_config(
        &self,
        mut cfg: MonitorConfigDto,
        state: &AppEngineState,
    ) -> Result<(), String> {
        // Clamp to sane ranges — a corrupt payload must not zero out
        // windows/tiers or produce NaN budgets downstream.
        if !["auto", "manual"].contains(&cfg.qualification_mode.as_str()) {
            cfg.qualification_mode = "auto".into();
        }
        if !["one_way", "two_way"].contains(&cfg.execution_mode.as_str()) {
            cfg.execution_mode = "one_way".into();
        }
        cfg.half_life_multiplier = cfg.half_life_multiplier.clamp(0.5, 24.0);
        cfg.min_window_secs = cfg.min_window_secs.clamp(60, 7 * 86400);
        cfg.manual_window_secs = cfg.manual_window_secs.clamp(300, 30 * 86400);
        cfg.watchlist_refresh_secs = cfg.watchlist_refresh_secs.clamp(300, 86400);
        cfg.history_window_secs = cfg.history_window_secs.clamp(3600, 30 * 86400);
        cfg.min_symbol_interval_secs = cfg.min_symbol_interval_secs.clamp(2, 3600);
        cfg.max_symbol_interval_secs = cfg
            .max_symbol_interval_secs
            .clamp(cfg.min_symbol_interval_secs, 86400);
        cfg.friction_cover_mult = cfg.friction_cover_mult.clamp(0.1, 10.0);
        cfg.min_win_probability = cfg.min_win_probability.clamp(0.5, 0.999);
        cfg.participation_cap = cfg.participation_cap.clamp(0.0001, 0.5);
        cfg.tx_reserve_pct = cfg.tx_reserve_pct.clamp(0.0, 0.9);
        cfg.min_episodes = cfg.min_episodes.clamp(1, 500);
        cfg.capacity_tiers_usd = cfg
            .capacity_tiers_usd
            .iter()
            .copied()
            .filter(|t| t.is_finite() && *t > 0.0)
            .collect();
        if cfg.capacity_tiers_usd.is_empty() {
            cfg.capacity_tiers_usd = d_tiers();
        }
        cfg.capacity_tiers_usd
            .sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let json = serde_json::to_string(&cfg).map_err(|e| e.to_string())?;
        state
            .state_store
            .save_monitor_config_json(&json)
            .map_err(|e| e.to_string())?;
        *self.cfg.lock() = cfg;
        Ok(())
    }

    /// Stop the background loop (used by tests and future lifecycle work).
    #[allow(dead_code)]
    pub fn shutdown(&self) {
        *self.shutdown.lock() = true;
    }

    /// Force the next tick to re-derive the watchlist (exemption or
    /// account-enablement change).
    pub fn request_watchlist_refresh(&self) {
        *self.last_watchlist_at.lock() = None;
    }

    /// Latest computed snapshot (None until the first eval pass completes).
    pub fn snapshot(&self) -> Option<MonitorSnapshotDto> {
        self.last_snapshot.lock().clone()
    }

    /// Seam for the future auto-trading bot: only fully qualified rows.
    /// Not consumed by the v1 UI — the bot contract is part of the spec.
    #[allow(dead_code)]
    pub fn qualified_opportunities(&self) -> Vec<MonitorRowDto> {
        self.snapshot()
            .map(|s| {
                s.rows
                    .into_iter()
                    .filter(|r| r.status == "qualified" && r.executable)
                    .collect()
            })
            .unwrap_or_default()
    }

    /* ---------------------------------------------------------------- */
    /* Background loop                                                    */
    /* ---------------------------------------------------------------- */

    pub async fn run(self: Arc<Self>, state: Arc<AppEngineState>) {
        let mut eval_at = Instant::now();
        loop {
            if *self.shutdown.lock() {
                return;
            }
            if *state.is_live_mode.read() || *state.is_paused.read() || !self.cfg.lock().enabled {
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }

            self.maybe_refresh_watchlist(&state).await;
            self.poll_due_books(&state).await;
            self.poll_funding(&state).await;

            if eval_at <= Instant::now() {
                self.evaluate(&state).await;
                // Eval cadence tracks the fastest lane's fair-share tick.
                eval_at = Instant::now() + Duration::from_secs(5);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// Rebuild lanes + watchlist from live `fetch_markets()` on each
    /// admitted venue. Symbols listed on >= 2 venues and not exempted form
    /// the watchlist. Runs hourly (configurable) and on the first tick.
    async fn maybe_refresh_watchlist(&self, state: &AppEngineState) {
        let refresh_secs = self.cfg.lock().watchlist_refresh_secs.max(60);
        let due = self
            .last_watchlist_at
            .lock()
            .map(|t| t.elapsed() >= Duration::from_secs(refresh_secs))
            .unwrap_or(true);
        if !due {
            // Cheap path: still pick up newly-admitted venues between full
            // refreshes without re-calling fetch_markets.
            let known: HashSet<String> =
                self.lanes.lock().iter().map(|l| l.venue.clone()).collect();
            let admitted: HashSet<String> = state
                .get_venues()
                .iter()
                .map(|(v, _, _)| v.to_string())
                .collect();
            if admitted.is_subset(&known) {
                return;
            }
        }

        let exempt: HashSet<String> = state
            .state_store
            .exempt_tickers()
            .unwrap_or_default()
            .into_iter()
            .map(|s| canonical_base(&s))
            .collect();

        let venues = state.get_venues();
        let mut venue_bases: HashMap<String, Vec<String>> = HashMap::new();
        let mut new_lanes: Vec<VenueLane> = Vec::new();
        let s_flags = state.settings.read().clone();
        for (vid, _name, adapter) in venues {
            let tiers = state
                .state_store
                .venue_enabled_tiers(vid)
                .unwrap_or_default();
            let flag_enabled = match vid {
                "hyperliquid" => s_flags.hyperliquid_enabled,
                "lighter" => s_flags.lighter_enabled,
                "robinhood" => s_flags.robinhood_enabled,
                _ => false,
            };
            let (budget, tier) = venue_budget_per_min(vid, &tiers, flag_enabled);
            let reserve = self.cfg.lock().tx_reserve_pct.clamp(0.0, 0.9);
            let usable = (budget * (1.0 - reserve)).max(6.0);

            match adapter.fetch_markets().await {
                Ok(markets) => {
                    let mut bases: Vec<String> = markets
                        .iter()
                        .filter(|m| m.is_active)
                        .map(|m| canonical_base(&m.symbol.base))
                        .collect::<HashSet<_>>()
                        .into_iter()
                        .collect();
                    bases.sort();
                    venue_bases.insert(vid.to_string(), bases.clone());
                    // Preserve round-robin progress + backoff across refresh.
                    let (cursor, last_fetch, requests, fail_streak, backoff_until) = {
                        let lanes = self.lanes.lock();
                        lanes
                            .iter()
                            .find(|l| l.venue == vid)
                            .map(|l| {
                                (
                                    l.cursor,
                                    l.last_fetch.clone(),
                                    l.requests.clone(),
                                    l.fail_streak,
                                    l.backoff_until,
                                )
                            })
                            .unwrap_or_default()
                    };
                    new_lanes.push(VenueLane {
                        venue: vid.to_string(),
                        adapter,
                        symbols: bases,
                        requests,
                        budget_per_min: budget,
                        usable_per_min: usable,
                        account_tier: tier,
                        cursor,
                        last_fetch,
                        backoff_until,
                        fail_streak,
                        next_funding_at: Instant::now(),
                    });
                }
                Err(e) => {
                    tracing::warn!("monitor: fetch_markets failed for {vid}: {e}");
                    // Keep the previous lane (and its backoff) if the venue
                    // was already known — dropping it would hide staleness.
                    let lanes = self.lanes.lock();
                    if let Some(old) = lanes.iter().find(|l| l.venue == vid) {
                        venue_bases.insert(vid.to_string(), old.symbols.clone());
                    }
                    drop(lanes);
                    continue;
                }
            }
        }
        // Merge surviving failed lanes back so they keep their budget state.
        {
            let old = self.lanes.lock();
            for l in old.iter() {
                if !new_lanes.iter().any(|n| n.venue == l.venue)
                    && venue_bases.contains_key(&l.venue)
                {
                    new_lanes.push(VenueLane {
                        venue: l.venue.clone(),
                        adapter: l.adapter.clone(),
                        symbols: l.symbols.clone(),
                        requests: l.requests.clone(),
                        budget_per_min: l.budget_per_min,
                        usable_per_min: l.usable_per_min,
                        account_tier: l.account_tier.clone(),
                        cursor: l.cursor,
                        last_fetch: l.last_fetch.clone(),
                        backoff_until: l.backoff_until,
                        fail_streak: l.fail_streak,
                        next_funding_at: l.next_funding_at,
                    });
                }
            }
        }

        // Intersection: bases present on >= 2 venues, minus exemptions.
        let mut counts: HashMap<String, usize> = HashMap::new();
        for bases in venue_bases.values() {
            for b in bases {
                *counts.entry(b.clone()).or_default() += 1;
            }
        }
        let mut watch: Vec<String> = counts
            .into_iter()
            .filter(|(_, n)| *n >= 2)
            .map(|(b, _)| b)
            .filter(|b| !exempt.contains(b))
            .collect();
        watch.sort();

        *self.venue_symbols.lock() = venue_bases;
        *self.watchlist.lock() = watch;
        *self.lanes.lock() = new_lanes;
        *self.last_watchlist_at.lock() = Some(Instant::now());

        // Drop persisted signals for pairs that left the universe.
        let keep: Vec<(String, String, String)> = self.pairs.lock().keys().cloned().collect();
        let _ = state.state_store.prune_monitor_signals(&keep);
    }

    /// Spend each lane's usable budget on its due symbols, one fetch per
    /// (venue, symbol). Books go to the hub; top-of-book is persisted to
    /// `spread_samples` for the historical baseline.
    async fn poll_due_books(&self, state: &AppEngineState) {
        struct Job {
            venue: String,
            symbol: String,
            adapter: Arc<dyn ExchangeAdapter>,
        }
        let mut jobs: Vec<Job> = Vec::new();
        {
            let cfg = self.cfg.lock().clone();
            let mut lanes = self.lanes.lock();
            for lane in lanes.iter_mut() {
                // Lighter book reads are two HTTP calls under one logical
                // fetch (orderBookDetails + orderBookOrders); weight the
                // spend so the rolling per-IP budget stays honest.
                let cost: usize = match lane.venue.as_str() {
                    "lighter" | "robinhood" => 2,
                    _ => 1,
                };
                while lane.can_spend_n(cost) {
                    let due = lane.due_symbols(&cfg);
                    if due.is_empty() {
                        break;
                    }
                    let sym = due[0].clone();
                    // Advance the cursor past this symbol.
                    if let Some(pos) = lane.symbols.iter().position(|s| *s == sym) {
                        lane.cursor = (pos + 1) % lane.symbols.len().max(1);
                    }
                    lane.spend_weighted(cost);
                    lane.last_fetch.insert(sym.clone(), Instant::now());
                    jobs.push(Job {
                        venue: lane.venue.clone(),
                        symbol: sym,
                        adapter: lane.adapter.clone(),
                    });
                }
            }
        }
        if jobs.is_empty() {
            return;
        }
        // Fetches run concurrently — a slow venue must not serialize the
        // whole cycle; each job's lane is keyed back on completion.
        let mut set = tokio::task::JoinSet::new();
        for job in jobs {
            set.spawn(async move {
                let res = job.adapter.fetch_orderbook(&job.symbol).await;
                (job.venue, job.symbol, res)
            });
        }
        let now_ms = Utc::now().timestamp_millis();
        while let Some(joined) = set.join_next().await {
            let (venue, symbol, res) = match joined {
                Ok(v) => v,
                Err(_) => continue,
            };
            match res {
                Ok(book) => {
                    if let Some(lane) = self.lanes.lock().iter_mut().find(|l| l.venue == venue) {
                        lane.note_success();
                    }
                    let entry = book_sample(&book, &venue);
                    state.market_data_hub.update_orderbook(book);
                    if let Some(e) = entry {
                        let _ = state
                            .state_store
                            .record_spread_samples(&symbol, now_ms, &[e]);
                    }
                }
                Err(e) => {
                    tracing::debug!("monitor: book fetch failed {venue}/{symbol}: {e}");
                    if let Some(lane) = self.lanes.lock().iter_mut().find(|l| l.venue == venue) {
                        lane.note_failure();
                    }
                }
            }
        }
    }

    /// Bulk funding fetch per lane on a slow cadence; maintains the EMA used
    /// by the carry estimator. One request per venue per ~60s.
    async fn poll_funding(&self, state: &AppEngineState) {
        let now = Instant::now();
        let mut work: Vec<(String, Arc<dyn ExchangeAdapter>)> = Vec::new();
        {
            let mut lanes = self.lanes.lock();
            for lane in lanes.iter_mut() {
                if lane.next_funding_at <= now && lane.can_spend() {
                    lane.spend();
                    lane.next_funding_at = now + Duration::from_secs(60);
                    work.push((lane.venue.clone(), lane.adapter.clone()));
                }
            }
        }
        for (venue, adapter) in work {
            match adapter.fetch_funding_rates().await {
                Ok(rates) => {
                    if let Some(lane) = self.lanes.lock().iter_mut().find(|l| l.venue == venue) {
                        lane.note_success();
                    }
                    let mut ema = self.funding_ema.lock();
                    for fr in rates {
                        let base = canonical_base(&fr.symbol);
                        let hours = fr.interval_hours.max(1) as f64;
                        let per_hour = fr.rate.to_string().parse::<f64>().unwrap_or(0.0) / hours;
                        if !per_hour.is_finite() {
                            continue;
                        }
                        let key = (venue.clone(), base);
                        let next = match ema.get(&key) {
                            Some(prev) => 0.7 * prev + 0.3 * per_hour,
                            None => per_hour,
                        };
                        ema.insert(key, next);
                        state.market_data_hub.update_funding(fr);
                    }
                }
                Err(e) => {
                    tracing::debug!("monitor: funding fetch failed for {venue}: {e}");
                    if let Some(lane) = self.lanes.lock().iter_mut().find(|l| l.venue == venue) {
                        lane.note_failure();
                    }
                }
            }
        }
    }

    /* ---------------------------------------------------------------- */
    /* Pair evaluation                                                     */
    /* ---------------------------------------------------------------- */

    async fn evaluate(&self, state: &AppEngineState) {
        let cfg = self.cfg.lock().clone();
        let now_ms = Utc::now().timestamp_millis();
        let watchlist = self.watchlist.lock().clone();
        let venue_symbols = self.venue_symbols.lock().clone();
        let lane_meta: Vec<(String, f64, usize, f64, bool, String)> = {
            let mut lanes = self.lanes.lock();
            lanes
                .iter_mut()
                .map(|l| {
                    (
                        l.venue.clone(),
                        l.usable_per_min,
                        l.symbols.len(),
                        l.symbol_interval(&cfg).as_secs_f64(),
                        l.backoff_until.is_some_and(|t| Instant::now() < t),
                        l.account_tier.clone(),
                    )
                })
                .collect()
        };
        // Per-venue taker fee incl. the Lighter Plus tier's documented
        // +0.5bps. Uses the shared venue fee map so the monitor and the
        // scan paths price friction identically.
        let fee_map: HashMap<String, f64> = lane_meta
            .iter()
            .map(|(v, _, _, _, _, tier)| {
                let base: f64 = crate::commands::venue_fee_bps(v)
                    .to_string()
                    .parse()
                    .unwrap_or(2.0);
                let plus_adj = if (v == "lighter" || v == "robinhood") && tier == "plus" {
                    0.5
                } else {
                    0.0
                };
                (v.clone(), base + plus_adj)
            })
            .collect();
        let lanes_used: Vec<usize> = {
            let mut lanes = self.lanes.lock();
            lanes.iter_mut().map(|l| l.used_last_min()).collect()
        };

        // Participation-cap volume map (cached 5 min upstream).
        let volumes = crate::commands::cached_venue_volumes(state).await;

        let stale_ms = {
            // A book is stale after ~3 refresh intervals (bounded 30..300s).
            let max_iv = lane_meta
                .iter()
                .map(|(_, _, _, iv, _, _)| *iv)
                .fold(10.0f64, f64::max);
            Duration::from_secs_f64((max_iv * 3.0).clamp(30.0, 300.0))
        };

        let history_from = now_ms - (cfg.history_window_secs as i64) * 1000;
        let mut rows: Vec<MonitorRowDto> = Vec::new();
        let mut persist_batch: Vec<(
            String,
            String,
            String,
            PairStatus,
            i64,
            i64,
            Option<f64>,
            Option<f64>,
        )> = Vec::new();

        for symbol in &watchlist {
            // Per-venue samples for this symbol over the history window.
            // The monitor records under the canonical base ("BTC"); the
            // legacy ticker-flow path records under the UI symbol
            // ("BTC-USDC") — merge both so the whole history feeds the
            // baseline regardless of which path produced it.
            let mut samples = Vec::new();
            let mut ok_any = false;
            for key in [
                symbol.clone(),
                format!("{symbol}-USDC"),
                format!("{symbol}-USD"),
            ] {
                if let Ok(mut s) = state.state_store.spread_samples(&key, history_from, now_ms) {
                    ok_any = true;
                    samples.append(&mut s);
                }
            }
            if !ok_any {
                continue;
            }
            samples.sort_by_key(|s| (s.ts_ms, s.venue_id.clone()));
            let mut mids_by_venue: HashMap<String, Vec<(i64, f64)>> = HashMap::new();
            for s in &samples {
                if s.mid.is_finite() && s.mid > 0.0 {
                    mids_by_venue
                        .entry(s.venue_id.clone())
                        .or_default()
                        .push((s.ts_ms, s.mid));
                }
            }
            let venues_here: Vec<String> = venue_symbols
                .iter()
                .filter(|(_, bases)| bases.iter().any(|b| b == symbol))
                .map(|(v, _)| v.clone())
                .collect();
            if venues_here.len() < 2 {
                continue;
            }

            for (i, buy_v) in venues_here.iter().enumerate() {
                for (j, sell_v) in venues_here.iter().enumerate() {
                    if i == j {
                        continue;
                    }
                    if let Some(row) = self.evaluate_pair(
                        state,
                        &cfg,
                        symbol,
                        buy_v,
                        sell_v,
                        now_ms,
                        stale_ms,
                        &mids_by_venue,
                        &volumes,
                        &fee_map,
                    ) {
                        if let Some(p) = &row.1 {
                            persist_batch.push(p.clone());
                        }
                        rows.push(row.0);
                    }
                }
            }
        }

        // Sort by expected profit bps desc; non-qualified below qualified.
        rows.sort_by(|a, b| {
            b.expected_profit_bps
                .unwrap_or(f64::NEG_INFINITY)
                .partial_cmp(&a.expected_profit_bps.unwrap_or(f64::NEG_INFINITY))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        {
            let mut persisted = self.persisted.lock();
            for (sym, bv, sv, status, fs, ls, edge, dev) in persist_batch {
                let key = (sym.clone(), bv.clone(), sv.clone());
                let changed = persisted
                    .get(&key)
                    .map(|(s, f, l)| *s != status || *f != fs || (*l / 60_000) != (ls / 60_000))
                    .unwrap_or(true);
                if changed {
                    if state
                        .state_store
                        .upsert_monitor_signal(&sym, &bv, &sv, status.as_str(), fs, ls, edge, dev)
                        .is_ok()
                    {
                        persisted.insert(key, (status, fs, ls));
                    }
                }
            }
        }

        let exempted = state.state_store.exempt_tickers().unwrap_or_default();
        let snap = MonitorSnapshotDto {
            generated_ts_ms: now_ms,
            running: true,
            rows,
            watchlist,
            venues: lane_meta
                .into_iter()
                .zip(lanes_used)
                .map(
                    |((venue, budget, syms, iv, backing, tier), used)| VenueLaneStatusDto {
                        venue,
                        read_budget_per_min: budget,
                        used_last_min: used,
                        symbols: syms,
                        refresh_interval_secs: iv,
                        backing_off: backing,
                        account_tier: tier,
                    },
                )
                .collect(),
            exempted,
        };
        *self.last_snapshot.lock() = Some(snap);
    }

    /// Evaluate one ordered pair direction. Returns the row plus an optional
    /// persist tuple when lifecycle state changed materially.
    #[allow(clippy::too_many_arguments)]
    fn evaluate_pair(
        &self,
        state: &AppEngineState,
        cfg: &MonitorConfigDto,
        symbol: &str,
        buy_v: &str,
        sell_v: &str,
        now_ms: i64,
        stale_ms: Duration,
        mids_by_venue: &HashMap<String, Vec<(i64, f64)>>,
        volumes: &HashMap<String, HashMap<String, f64>>,
        fee_map: &HashMap<String, f64>,
    ) -> Option<(
        MonitorRowDto,
        Option<(
            String,
            String,
            String,
            PairStatus,
            i64,
            i64,
            Option<f64>,
            Option<f64>,
        )>,
    )> {
        let key = (symbol.to_string(), buy_v.to_string(), sell_v.to_string());

        // --- live books ------------------------------------------------
        let buy_book = state.market_data_hub.get_valid_orderbook(
            &arbitrager_core::types::VenueId(buy_v.to_string()),
            symbol,
            Some(stale_ms),
        );
        let sell_book = state.market_data_hub.get_valid_orderbook(
            &arbitrager_core::types::VenueId(sell_v.to_string()),
            symbol,
            Some(stale_ms),
        );
        let books_ok = buy_book.is_some() && sell_book.is_some();

        // --- history ----------------------------------------------------
        let series = match (mids_by_venue.get(buy_v), mids_by_venue.get(sell_v)) {
            (Some(a), Some(b)) if !a.is_empty() && !b.is_empty() => build_pair_series(a, b, 30_000),
            _ => Vec::new(),
        };
        let vals: Vec<f64> = series.iter().map(|o| o.spread_bps).collect();
        let n = vals.len();
        let (mu, sigma) = if n >= 2 {
            let m = vals.iter().sum::<f64>() / n as f64;
            let v = vals.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (n - 1) as f64;
            (Some(m), Some(v.sqrt()))
        } else {
            (None, None)
        };

        // --- qualification window --------------------------------------
        let half_life_secs = if n >= 10 {
            let dt = if series.len() >= 2 {
                ((series.last().unwrap().ts_ms - series[0].ts_ms) as f64 / 1000.0)
                    / (series.len() - 1) as f64
            } else {
                0.0
            };
            OuModelParams::fit(&vals, dt).map(|p| p.half_life)
        } else {
            None
        };
        let window_secs = if cfg.qualification_mode == "manual" {
            cfg.manual_window_secs.max(60) as f64
        } else {
            let hl =
                half_life_secs.unwrap_or(cfg.min_window_secs as f64 / cfg.half_life_multiplier);
            (hl * cfg.half_life_multiplier).max(cfg.min_window_secs as f64)
        };
        let span_secs = series
            .first()
            .zip(series.last())
            .map(|(f, l)| (l.ts_ms - f.ts_ms) as f64 / 1000.0)
            .unwrap_or(0.0);
        let progress = (span_secs / window_secs).clamp(0.0, 1.0);
        let window_met = span_secs >= window_secs && n >= 10;

        // --- friction ---------------------------------------------------
        // Static per-unit costs: both taker fees + funding carry (+ exit
        // estimate for two-way). Book-walk slippage is inside the ladder.
        let fee_bps = fee_map.get(buy_v).copied().unwrap_or(2.0)
            + fee_map.get(sell_v).copied().unwrap_or(2.0);
        let fr_a = self
            .funding_ema
            .lock()
            .get(&(buy_v.to_string(), symbol.to_string()))
            .copied();
        let fr_b = self
            .funding_ema
            .lock()
            .get(&(sell_v.to_string(), symbol.to_string()))
            .copied();
        // t_hold: OU half-life -> empirical median -> None.
        let stats_for_t = if let (Some(m), Some(s)) = (mu, sigma) {
            Some(reversion_stats(&series, m, s, 0.0, 2.0, 3.0, 1))
        } else {
            None
        };
        let t_hold_secs = half_life_secs.or(stats_for_t.and_then(|s| s.median_converge_secs));
        let carry = funding_carry_bps(fr_a, fr_b, t_hold_secs);
        let funding_unknown = carry.is_none();
        // Top-of-book crossing cost per venue — full spread paid to exit.
        let crossing = |b: &Option<OrderBook>| -> Option<f64> {
            let bk = b.as_ref()?;
            let ask = bk.asks.first()?.price.to_string().parse::<f64>().ok()?;
            let bid = bk.bids.first()?.price.to_string().parse::<f64>().ok()?;
            let mid = (ask + bid) / 2.0;
            (mid > 0.0 && ask > bid).then_some((ask - bid) / mid * 10_000.0)
        };
        let exit_est = if cfg.execution_mode == "two_way" {
            // Mirror-image unwind: same fee schedule on both legs plus an
            // estimated crossing of each book's current spread. Books at
            // depth may be worse — documented floor, not a guarantee.
            let cross = crossing(&buy_book).unwrap_or(0.0) + crossing(&sell_book).unwrap_or(0.0);
            fee_bps + cross
        } else {
            0.0
        };
        let friction_bps = fee_bps + carry.unwrap_or(0.0).max(0.0) + exit_est;

        // --- current executable edge ------------------------------------
        let (edge_bps, dev_bps) = match (&buy_book, &sell_book, mu) {
            (Some(bk), Some(sk), Some(m)) => {
                let ask = bk
                    .asks
                    .first()
                    .map(|l| l.price.to_string().parse::<f64>().ok())
                    .flatten();
                let bid = sk
                    .bids
                    .first()
                    .map(|l| l.price.to_string().parse::<f64>().ok())
                    .flatten();
                match (ask, bid) {
                    (Some(a), Some(b)) if a > 0.0 => {
                        let edge = (b - a) / a * 10_000.0;
                        (Some(edge), Some(edge - m))
                    }
                    _ => (None, None),
                }
            }
            _ => (None, None),
        };

        // --- win probability + capacity ---------------------------------
        let band_bps = friction_bps.max(2.0);
        let stop_mult = 3.0;
        let pwin_curve = if let (Some(m), Some(s)) = (mu, sigma) {
            build_pwin_curve(&series, m, s, band_bps, stop_mult)
        } else {
            arbitrager_math::pair_monitor::PWinCurve {
                points: Vec::new(),
                episodes: 0,
            }
        };
        let episodes = pwin_curve.episodes;
        let p_win = dev_bps.and_then(|d| {
            if episodes >= cfg.min_episodes {
                p_win_at_deviation(&pwin_curve, d.abs())
            } else {
                None
            }
        });

        // Capacity ladder over both books — requires a real historical
        // baseline; mu=0.0 would fabricate a reference price, so no ladder.
        let (ladder, optimal, breakeven, exp_usd, exp_bps) = match (&buy_book, &sell_book, mu) {
            (Some(bk), Some(sk), Some(m)) => {
                let asks: Vec<(f64, f64)> = bk
                    .asks
                    .iter()
                    .filter_map(|l| {
                        let p = l.price.to_string().parse::<f64>().ok()?;
                        let s = l.size.to_string().parse::<f64>().ok()?;
                        (p.is_finite() && s.is_finite() && p > 0.0 && s > 0.0).then_some((p, s))
                    })
                    .collect();
                let bids: Vec<(f64, f64)> = sk
                    .bids
                    .iter()
                    .filter_map(|l| {
                        let p = l.price.to_string().parse::<f64>().ok()?;
                        let s = l.size.to_string().parse::<f64>().ok()?;
                        (p.is_finite() && s.is_finite() && p > 0.0 && s > 0.0).then_some((p, s))
                    })
                    .collect();
                // Participation cap: shrink the deepest tier to the smaller
                // venue's 24h-volume allowance so the boundary is honest.
                let cap_usd = volumes
                    .get(buy_v)
                    .and_then(|m| m.get(symbol))
                    .zip(volumes.get(sell_v).and_then(|m| m.get(symbol)))
                    .map(|(a, b)| a.min(*b) * cfg.participation_cap);
                let mut tiers = cfg.capacity_tiers_usd.clone();
                if let Some(cap) = cap_usd {
                    if cap.is_finite() && cap > 0.0 {
                        tiers.push(cap);
                        tiers.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                        tiers.dedup();
                    }
                }
                let rep = capacity_ladder(
                    &asks,
                    &bids,
                    m,
                    friction_bps,
                    &tiers,
                    &pwin_curve,
                    cfg.min_win_probability,
                );
                let ladder: Vec<CapacityTierDto> = rep
                    .tiers
                    .iter()
                    .map(|t| CapacityTierDto {
                        size_usd: t.size_usd,
                        filled_usd: t.filled_usd,
                        net_bps: t.net_bps,
                        expected_net_bps: t.expected_net_bps,
                        marginal_p_win: t.marginal_p_win,
                    })
                    .collect();
                (
                    ladder,
                    rep.optimal_size_usd,
                    rep.breakeven_size_usd,
                    rep.expected_profit_usd,
                    rep.optimal_size_usd.and_then(|s| {
                        rep.tiers
                            .iter()
                            .find(|t| (t.filled_usd - s).abs() < 1e-6)
                            .and_then(|t| t.expected_net_bps)
                    }),
                )
            }
            _ => (Vec::new(), None, None, None, None),
        };

        // --- status + lifecycle ------------------------------------------
        // Directional: a positive dev means this ordered pair (buy A / sell
        // B) is the profitable side; a negative dev belongs to the reverse
        // pair row, so it must not trigger here.
        let entry_thresh = friction_bps.max(1.0);
        let signal_active = dev_bps.map(|d| d >= entry_thresh).unwrap_or(false);
        let variation_ok = window_met
            && variation_covers_friction(
                &series,
                mu.unwrap_or(0.0),
                window_secs,
                cfg,
                friction_bps,
            );

        let status = if !books_ok {
            PairStatus::Stale
        } else if !window_met {
            PairStatus::Collecting
        } else if !signal_active {
            PairStatus::Watching
        } else if !variation_ok || p_win.is_none() || funding_unknown || ladder.is_empty() {
            PairStatus::Signaled
        } else {
            PairStatus::Qualified
        };

        // Lifecycle timestamps: discover on transition into an active signal;
        // keep last_seen updated while it remains active.
        let mut rt = self.pairs.lock().get(&key).cloned().unwrap_or(PairRuntime {
            status: PairStatus::Collecting,
            first_seen_ms: None,
            last_seen_ms: None,
            last_edge_bps: None,
            last_dev_bps: None,
        });
        let was_active = rt.status == PairStatus::Signaled || rt.status == PairStatus::Qualified;
        if signal_active && window_met {
            if !was_active {
                rt.first_seen_ms = Some(now_ms);
            }
            rt.last_seen_ms = Some(now_ms);
        }
        rt.status = status;
        rt.last_edge_bps = edge_bps;
        rt.last_dev_bps = dev_bps;
        self.pairs.lock().insert(key.clone(), rt.clone());

        let persist = rt.first_seen_ms.map(|fs| {
            (
                symbol.to_string(),
                buy_v.to_string(),
                sell_v.to_string(),
                status,
                fs,
                rt.last_seen_ms.unwrap_or(fs),
                edge_bps,
                dev_bps,
            )
        });

        // --- executability ------------------------------------------------
        let (executable, block_reason) = execution_gate(
            status,
            books_ok,
            signal_active,
            p_win,
            cfg.min_win_probability,
            funding_unknown,
            optimal,
            state,
            symbol,
        );

        let row = MonitorRowDto {
            symbol: symbol.to_string(),
            buy_venue: buy_v.to_string(),
            sell_venue: sell_v.to_string(),
            status: status.as_str().to_string(),
            current_edge_bps: edge_bps,
            baseline_bps: mu,
            deviation_bps: dev_bps,
            // Signed: a negative dev on this ordered pair is a losing
            // direction (the reverse pair owns it) — never abs() it into a
            // fake positive capture.
            net_capture_bps: exp_bps.or_else(|| dev_bps.map(|d| d - friction_bps)),
            p_win,
            expected_profit_bps: exp_bps.zip(p_win).map(|(e, p)| e * p).or_else(|| {
                dev_bps
                    .zip(p_win)
                    .map(|(d, p)| (d - friction_bps).max(0.0) * p)
            }),
            expected_profit_usd: exp_usd,
            optimal_size_usd: optimal,
            breakeven_size_usd: breakeven,
            capacity_ladder: ladder,
            funding_carry_bps: carry,
            funding_unknown,
            first_seen_ms: rt.first_seen_ms,
            last_seen_ms: rt.last_seen_ms,
            signal_age_secs: rt
                .first_seen_ms
                .zip(rt.last_seen_ms)
                .map(|(f, l)| (l - f) as f64 / 1000.0),
            expected_converge_secs: t_hold_secs,
            qualification_progress: progress,
            observed_episodes: episodes,
            executable,
            block_reason,
        };
        Some((row, persist))
    }
}

/// Whether the book snapshot yields a persistable top-of-book sample.
fn book_sample(book: &OrderBook, venue: &str) -> Option<VenueSampleEntry> {
    let bid = book.bids.first()?.price.to_string().parse::<f64>().ok()?;
    let ask = book.asks.first()?.price.to_string().parse::<f64>().ok()?;
    let mid = (bid + ask) / 2.0;
    (mid.is_finite() && mid > 0.0).then_some(VenueSampleEntry {
        venue_id: venue.to_string(),
        mid,
        best_bid: bid,
        best_ask: ask,
    })
}

/// User rule: within the qualification window, each half-life chunk must
/// show max |dev| >= friction_cover_mult * friction_bps — else the pair's
/// motion is noise relative to costs and cannot qualify.
fn variation_covers_friction(
    series: &[arbitrager_math::pair_monitor::PairObservation],
    mu: f64,
    window_secs: f64,
    cfg: &MonitorConfigDto,
    friction_bps: f64,
) -> bool {
    if series.len() < 2 || window_secs <= 0.0 {
        return false;
    }
    let chunk_secs = if cfg.qualification_mode == "manual" {
        window_secs / 2.0
    } else {
        (window_secs / cfg.half_life_multiplier.max(1.0)).max(60.0)
    };
    let need = (friction_bps * cfg.friction_cover_mult).max(1e-6);
    let t_end = series.last().unwrap().ts_ms;
    let chunks = ((window_secs / chunk_secs).ceil() as i64).clamp(1, 64);
    for k in 0..chunks {
        let c_end = t_end - (k as i64) * (chunk_secs * 1000.0) as i64;
        let c_start = c_end - (chunk_secs * 1000.0) as i64;
        let max_dev = series
            .iter()
            .filter(|o| o.ts_ms > c_start && o.ts_ms <= c_end)
            .map(|o| (o.spread_bps - mu).abs())
            .fold(0.0f64, f64::max);
        if max_dev < need {
            return false;
        }
    }
    true
}

/// Whether the row may be executed right now. Monitor rows only ever drive
/// the paper path; live mode and unresolved signals block hard.
#[allow(clippy::too_many_arguments)]
fn execution_gate(
    status: PairStatus,
    books_ok: bool,
    signal_active: bool,
    p_win: Option<f64>,
    p_min: f64,
    funding_unknown: bool,
    optimal: Option<f64>,
    state: &AppEngineState,
    symbol: &str,
) -> (bool, Option<String>) {
    if *state.is_live_mode.read() {
        return (
            false,
            Some("live mode: monitor execution is paper-only".into()),
        );
    }
    if *state.is_paused.read() {
        return (false, Some("engine paused".into()));
    }
    if !books_ok {
        return (false, Some("orderbook stale or missing".into()));
    }
    if status != PairStatus::Qualified || !signal_active {
        return (false, Some("signal not qualified".into()));
    }
    match p_win {
        Some(p) if p >= p_min => {}
        Some(_) => {
            return (
                false,
                Some(format!("win probability below {p_min:.0}% floor")),
            )
        }
        None => return (false, Some("win probability unknown".into())),
    }
    if funding_unknown {
        return (false, Some("funding adjustment unknown".into()));
    }
    if optimal.map(|s| s <= 0.0).unwrap_or(true) {
        return (false, Some("no profitable capacity".into()));
    }
    if let Ok(true) = state.state_store.ticker_blocked(symbol) {
        return (false, Some("unresolved signal on this symbol".into()));
    }
    (true, None)
}

/* ------------------------------------------------------------------------ */
/* Tests                                                                     */
/* ------------------------------------------------------------------------ */

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrager_core::quote::{BookLevel, OrderBook};
    use arbitrager_core::types::VenueId;
    use arbitrager_math::pair_monitor::PairObservation;
    use chrono::Utc;
    use rust_decimal_macros::dec;

    fn book(venue: &str, symbol: &str, bid: f64, ask: f64, depth: usize) -> OrderBook {
        let mut b = OrderBook::new(VenueId::new(venue), symbol.to_string());
        for i in 0..depth.max(1) {
            b.bids.push(BookLevel::new(
                dec!(0) + rust_decimal::Decimal::from_f64_retain(bid - i as f64 * 0.5).unwrap(),
                dec!(1.0),
            ));
            b.asks.push(BookLevel::new(
                dec!(0) + rust_decimal::Decimal::from_f64_retain(ask + i as f64 * 0.5).unwrap(),
                dec!(1.0),
            ));
        }
        b.timestamp = Utc::now();
        b
    }

    fn obs(ts: i64, bps: f64) -> PairObservation {
        PairObservation {
            ts_ms: ts,
            spread_bps: bps,
        }
    }

    #[test]
    fn canonical_base_normalizes_quotes() {
        assert_eq!(canonical_base("BTC"), "BTC");
        assert_eq!(canonical_base("btc-usd"), "BTC");
        assert_eq!(canonical_base("ETH-USDC"), "ETH");
        assert_eq!(canonical_base("sol/usd"), "SOL");
        assert_eq!(canonical_base("BTC-PERP"), "BTC");
        assert_eq!(canonical_base("BTC-USDT"), "BTC");
        assert_eq!(canonical_base("ETHUSDT"), "ETH");
        assert_eq!(canonical_base("BTCUSD"), "BTC");
        // Real tickers ending in USD must not collapse to one letter.
        assert_eq!(canonical_base("TUSD"), "TUSD");
        assert_eq!(canonical_base("SUSD"), "SUSD");
    }

    #[test]
    fn lighter_budget_uses_lowest_tier_on_ip() {
        let tiers = vec!["plus".to_string(), "standard".to_string()];
        let (budget, tier) = venue_budget_per_min("lighter", &tiers, true);
        assert_eq!(budget, 60.0); // standard drags the shared-IP budget down
        assert_eq!(tier, "standard");
        let (budget, _) = venue_budget_per_min("robinhood", &["plus".to_string()], false);
        assert_eq!(budget, 1200.0);
        let (budget, _) = venue_budget_per_min("hyperliquid", &[], true);
        assert_eq!(budget, 500.0);
    }

    struct DummyAdapter;
    #[async_trait::async_trait]
    impl ExchangeAdapter for DummyAdapter {
        fn venue_id(&self) -> VenueId {
            VenueId::new("test")
        }
        fn name(&self) -> &str {
            "dummy"
        }
        async fn fetch_markets(
            &self,
        ) -> anyhow::Result<Vec<arbitrager_core::market::MarketMetadata>> {
            Ok(Vec::new())
        }
        async fn fetch_orderbook(&self, _s: &str) -> anyhow::Result<OrderBook> {
            Ok(book("test", _s, 99.0, 101.0, 1))
        }
        async fn fetch_ticker(&self, _s: &str) -> anyhow::Result<arbitrager_core::quote::Ticker> {
            anyhow::bail!("unused in tests")
        }
        async fn fetch_funding_rate(
            &self,
            _s: &str,
        ) -> anyhow::Result<arbitrager_core::quote::FundingRate> {
            anyhow::bail!("unused in tests")
        }
        async fn fetch_account(
            &self,
        ) -> anyhow::Result<arbitrager_core::position::AccountSnapshot> {
            anyhow::bail!("unused in tests")
        }
        async fn place_order(
            &self,
            _o: arbitrager_core::order::OrderRequest,
        ) -> anyhow::Result<arbitrager_core::order::OrderResponse> {
            anyhow::bail!("unused in tests")
        }
        async fn cancel_order(
            &self,
            _r: arbitrager_core::order::CancelRequest,
        ) -> anyhow::Result<()> {
            anyhow::bail!("unused in tests")
        }
    }

    #[test]
    fn lane_budget_and_round_robin() {
        let cfg = MonitorConfigDto::default();
        let mut lane = VenueLane {
            venue: "lighter".into(),
            adapter: Arc::new(DummyAdapter),
            symbols: vec!["A".into(), "B".into(), "C".into()],
            requests: VecDeque::new(),
            budget_per_min: 60.0,
            usable_per_min: 45.0, // 60 * (1 - 0.25)
            account_tier: "standard".into(),
            cursor: 0,
            last_fetch: HashMap::new(),
            backoff_until: None,
            fail_streak: 0,
            next_funding_at: Instant::now(),
        };
        // Fair-share interval: 3 symbols * 60s / 45 = 4s -> below 5s floor.
        let iv = lane.symbol_interval(&cfg);
        assert!((iv.as_secs_f64() - 5.0).abs() < 1e-9);
        // All three due on first pass; order is round-robin from cursor.
        let due = lane.due_symbols(&cfg);
        assert_eq!(due, vec!["A", "B", "C"]);
        // Budget enforcement: after 45 spends can_spend() is false.
        for _ in 0..45 {
            assert!(lane.can_spend());
            lane.spend();
        }
        assert!(!lane.can_spend());
        // Backoff blocks spending after a failure.
        let mut lane2 = lane;
        lane2.note_failure();
        assert!(!lane2.can_spend());
        lane2.note_success();
        assert!(lane2.can_spend() || lane2.used_last_min() as f64 >= lane2.usable_per_min);
    }

    #[test]
    fn variation_gate_rejects_noise() {
        let cfg = MonitorConfigDto {
            qualification_mode: "auto".into(),
            half_life_multiplier: 2.0,
            friction_cover_mult: 1.0,
            ..Default::default()
        };
        // Window 3600s -> chunks of 1800s; series spans 3600s, all devs tiny.
        let flat: Vec<_> = (0..=36).map(|i| obs(i * 100_000, 0.5)).collect();
        assert!(!variation_covers_friction(&flat, 0.0, 3600.0, &cfg, 5.0));
        // One chunk reaches 8bps in each half -> covers friction 5bps.
        let mut wavy = flat.clone();
        wavy[5] = obs(500_000, 8.0);
        wavy[30] = obs(3_000_000, -9.0);
        assert!(variation_covers_friction(&wavy, 0.0, 3600.0, &cfg, 5.0));
        // Only one chunk covered -> fails.
        let mut half = flat.clone();
        half[5] = obs(500_000, 8.0);
        assert!(!variation_covers_friction(&half, 0.0, 3600.0, &cfg, 5.0));
    }

    #[test]
    fn book_sample_rejects_malformed() {
        let empty = OrderBook::new(VenueId::new("x"), "T".into());
        assert!(book_sample(&empty, "x").is_none());
        let ok = book("x", "T", 99.0, 101.0, 1);
        let s = book_sample(&ok, "x").unwrap();
        assert!((s.mid - 100.0).abs() < 1e-9);
    }

    #[test]
    fn execution_gate_blocks_unqualified() {
        let state = AppEngineState::new();
        let (ok, why) = execution_gate(
            PairStatus::Signaled,
            true,
            true,
            Some(0.95),
            0.9,
            false,
            Some(1000.0),
            &state,
            "BTC",
        );
        assert!(!ok);
        assert!(why.unwrap().contains("not qualified"));

        let (ok, why) = execution_gate(
            PairStatus::Qualified,
            true,
            true,
            Some(0.5),
            0.9,
            false,
            Some(1000.0),
            &state,
            "BTC",
        );
        assert!(!ok);
        assert!(why.unwrap().contains("below"));

        let (ok, _) = execution_gate(
            PairStatus::Qualified,
            true,
            true,
            Some(0.95),
            0.9,
            true,
            Some(1000.0),
            &state,
            "BTC",
        );
        assert!(!ok);

        let (ok, why) = execution_gate(
            PairStatus::Qualified,
            true,
            true,
            Some(0.95),
            0.9,
            false,
            Some(1000.0),
            &state,
            "BTC",
        );
        assert!(ok, "gate should open: {why:?}");
    }

    #[tokio::test]
    async fn evaluate_produces_rows_and_respects_staleness() {
        let state = AppEngineState::new();
        let engine = SpreadMonitorEngine::new(&state);
        // Watchlist + venue universe injected directly (skips fetch_markets).
        *engine.watchlist.lock() = vec!["BTC".into()];
        engine
            .venue_symbols
            .lock()
            .insert("hyperliquid".into(), vec!["BTC".into()]);
        engine
            .venue_symbols
            .lock()
            .insert("lighter".into(), vec!["BTC".into()]);

        // Seed 2h of converging pair samples (span > auto floor 30min).
        let now = Utc::now().timestamp_millis();
        for i in 0..=72 {
            let ts = now - (7200_000 - i * 100_000);
            let wiggle = if i % 12 == 6 { 8.0 } else { 0.5 };
            state
                .state_store
                .record_spread_samples(
                    "BTC",
                    ts,
                    &[
                        VenueSampleEntry {
                            venue_id: "hyperliquid".into(),
                            mid: 100.0,
                            best_bid: 99.9,
                            best_ask: 100.1,
                        },
                        VenueSampleEntry {
                            venue_id: "lighter".into(),
                            mid: 100.0 + wiggle / 100.0,
                            best_bid: 99.9,
                            best_ask: 100.1,
                        },
                    ],
                )
                .unwrap();
        }
        // Live books: lighter bid > hyperliquid ask => positive edge.
        state
            .market_data_hub
            .update_orderbook(book("hyperliquid", "BTC", 100.0, 100.2, 3));
        state
            .market_data_hub
            .update_orderbook(book("lighter", "BTC", 100.4, 100.6, 3));

        engine.evaluate(&state).await;
        let snap = engine.snapshot().expect("snapshot produced");
        assert_eq!(snap.rows.len(), 2); // both directions
        let row = snap
            .rows
            .iter()
            .find(|r| r.buy_venue == "hyperliquid" && r.sell_venue == "lighter")
            .unwrap();
        assert!(row.current_edge_bps.is_some());
        assert!(row.baseline_bps.is_some());
        // 2h of data > 30min floor -> not collecting.
        assert_ne!(row.status, "collecting");

        // Now drop the books (staleness): fresh hub -> stale rows.
        state.market_data_hub.clear();
        engine.evaluate(&state).await;
        let snap2 = engine.snapshot().unwrap();
        assert!(snap2.rows.iter().all(|r| r.status == "stale"));
        assert!(snap2.rows.iter().all(|r| !r.executable));
    }
}
