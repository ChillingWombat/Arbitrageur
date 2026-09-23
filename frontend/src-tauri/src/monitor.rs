//! Spread Range Monitor — systematic cross-venue opportunity scanner.
//!
//! A background engine that watches every spread-eligible symbol across all
//! admitted venues, builds directional pair-spread history from the durable
//! `spread_samples` log, and qualifies opportunities through the
//! half-life/variation gate before they are shown as executable.
//!
//! Design contract (SPEC.md, Part 3 — Spread Range Monitor):
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

use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::quote::OrderBook;
use arbitrageur_core::types::{dec, Decimal};
use arbitrageur_math::ou_process::OuModelParams;
use arbitrageur_math::pair_monitor::{
    above_threshold_ratio, build_pair_series, build_pwin_curve, capacity_ladder,
    effective_coverage, entry_outcome_stats, funding_carry_bps, p_win_est_at_deviation,
    reversion_stats, simulate_entry_outcomes, PairObservation, PAIR_BUCKET_MS,
};
use arbitrageur_math::session::{
    apply_entry_adjust_bps, depth_mult, in_funding_mask, inverse_vol_mult, is_ondo_venue,
    is_weekend, session_allow, session_of, us_cash_open, winsorized_median, FastVolEwma, SessionId,
    FAST_VOL_HALT_MULT, SPREAD_STANDDOWN_MULT,
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
    /// Display floor for the radar table: rows are shown only when expected
    /// profit exceeds this many bps (default 0 = positive-only). A negative
    /// value relaxes the filter to include losing rows.
    #[serde(default)]
    pub min_expected_profit_bps: f64,
    /// Persistence floor: fraction of in-window observed buckets that must
    /// sit at/above the entry threshold for qualification (default 0.2).
    /// 0.0 disables the gate.
    #[serde(default = "d_min_ratio")]
    pub min_threshold_ratio: f64,
    /// Data-quality floor: fraction of expected window buckets that must
    /// contain a both-venues observation for qualification (default 0.5).
    /// 0.0 disables the gate.
    #[serde(default = "d_min_coverage")]
    pub min_coverage: f64,
    /// Episode recency half-life for win-rate evidence, seconds (default
    /// 3d). Episodes older than this decay exponentially
    /// (`0.5^(age/half_life)`) so stale regimes fade instead of counting
    /// equally with fresh ones. 0 disables weighting.
    #[serde(default = "d_episode_hl")]
    pub episode_half_life_secs: f64,
    /// Confidence floor on the win probability's Wilson lower bound
    /// (z=1.645): a row is not executable when its lower bound falls
    /// below this even if the shrunken point estimate passes
    /// `min_win_probability` (default 0.5 — must beat coin-flip with
    /// confidence; raise toward 0.8 for stricter). 0 disables.
    #[serde(default = "d_min_win_lb")]
    pub min_win_lower_bound: f64,
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
fn d_min_ratio() -> f64 {
    0.2
}
fn d_min_coverage() -> f64 {
    0.5
}
fn d_min_interval() -> u64 {
    5
}
fn d_episode_hl() -> f64 {
    3.0 * 86_400.0
}
fn d_min_win_lb() -> f64 {
    0.5
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
            min_expected_profit_bps: 0.0,
            min_threshold_ratio: d_min_ratio(),
            min_coverage: d_min_coverage(),
            episode_half_life_secs: d_episode_hl(),
            min_win_lower_bound: d_min_win_lb(),
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

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BpsCapacityTierDto {
    /// Marginal expected-net floor (bps/unit).
    pub min_net_bps: f64,
    /// Book notional fillable while marginal expected net >= floor.
    pub capacity_usd: f64,
}

/// One capacity-curve sample: capture state after consuming a book chunk.
/// USD values derive client-side as `bps · cumUsd / 1e4`.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DepthCurvePointDto {
    /// Cumulative fillable notional through this chunk, USD (x-axis).
    pub cum_usd: f64,
    /// Cumulative raw net capture per unit, bps.
    pub cum_net_bps: f64,
    /// Cumulative probability-weighted expected net per unit, bps.
    pub cum_exp_bps: Option<f64>,
    /// Marginal raw net per unit at this depth, bps.
    pub marg_net_bps: f64,
    /// Marginal expected net per unit at this depth, bps.
    pub marg_exp_bps: Option<f64>,
}

impl From<&arbitrageur_math::pair_monitor::DepthCurvePoint> for DepthCurvePointDto {
    fn from(p: &arbitrageur_math::pair_monitor::DepthCurvePoint) -> Self {
        Self {
            cum_usd: p.cum_usd,
            cum_net_bps: p.cum_net_bps,
            cum_exp_bps: p.cum_exp_bps,
            marg_net_bps: p.marg_net_bps,
            marg_exp_bps: p.marg_exp_bps,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorRowDto {
    pub symbol: String,
    pub buy_venue: String,
    pub sell_venue: String,
    /// preparing | tracking | triggered | ready | no-data
    pub status: String,
    /// Executable current edge (sell bid vs buy ask), bps.
    pub current_edge_bps: Option<f64>,
    /// Historical mean directional mid-mid spread, bps.
    pub baseline_bps: Option<f64>,
    /// Current edge minus baseline, bps.
    pub deviation_bps: Option<f64>,
    /// Net capture at the optimal tier after all friction, bps.
    pub net_capture_bps: Option<f64>,
    /// Deprecated alias of `reversion_probability` (kept for old clients).
    pub p_win: Option<f64>,
    /// Direction-insensitive mean-reversion probability P(|dev| reverts
    /// before the stop band). Informational only — NOT a trade win rate.
    /// Render as "Reversion Probability" in detail views, never as "Win %".
    pub reversion_probability: Option<f64>,
    /// Directional trade win probability after every execution detail
    /// (edge direction, profitable capacity, funding known, exit depth,
    /// persistence, coverage, variation, session). `Some(0.0)` when the
    /// current ordered direction has no edge or is hard-blocked; `None`
    /// only when uncomputable (thin history / missing books). This is the
    /// table's "Win %" column.
    pub win_probability: Option<f64>,
    /// Wilson lower bound of `win_probability` at z=1.645 on the bucket's
    /// effective episode count — the confidence-aware floor the
    /// executable gate checks against `min_win_lower_bound`.
    pub win_probability_lower: Option<f64>,
    /// Wilson upper bound of `win_probability` — interval width shows how
    /// much the bucket's episode evidence actually constrains the rate.
    pub win_probability_upper: Option<f64>,
    /// Kish-effective episode count behind the win estimate (shrinks under
    /// recency weighting). 0.0 when the OU fallback produced the estimate.
    pub p_win_n_effective: Option<f64>,
    /// Still-open (censored) episodes inside the win bucket — a high share
    /// means the rate is understated, not confirmed.
    pub p_win_censored: Option<u32>,
    /// True when the win estimate came from the OU first-passage fallback
    /// rather than empirical episode evidence.
    pub p_win_from_model: Option<bool>,
    /// Empirical P(net > 0) for a fresh entry at the current deviation:
    /// every historical excursion replayed through the exit policy and
    /// scored on real net bps (`sign(e0)·(e0 − dev_exit) − friction`),
    /// Beta-shrunken. `None` below `min_episodes` resolved trials.
    /// THIS is the P&L-defined win rate — `reversion_probability`/`p_win`
    /// answer "does the spread normalize", this answers "does the trade
    /// make money".
    pub net_win_probability: Option<f64>,
    /// Wilson lower bound of `net_win_probability` counting censored
    /// episodes as non-wins — the fail-closed reading.
    pub net_win_lower: Option<f64>,
    /// Recency-weighted mean net bps per simulated entry at the current
    /// deviation (censored episodes marked at last observation).
    pub net_expected_bps: Option<f64>,
    /// Ascending weighted net-bps deciles (P0…P100, 11 points) of
    /// simulated entries at this deviation — enables P(net >= any bps
    /// target) client-side, same contract as the funding monitor's
    /// `backtest_deciles_usd`.
    pub net_deciles_bps: Option<Vec<f64>>,
    /// Simulated entry trials / still-open share at this deviation.
    pub outcome_trials: Option<u32>,
    pub outcome_censored: Option<u32>,
    /// Weighted median entry→exit seconds across simulated episodes, and
    /// among profitable ones — capital-turnover context.
    pub median_hold_secs: Option<f64>,
    pub median_win_secs: Option<f64>,
    /// Signed probability-weighted PnL expectation, bps. Negative means an
    /// expected loss — never floored to 0. `exp_bps` from the ladder is
    /// already probability-weighted, so it is used directly; the fallback
    /// is `(dev - friction) * reversion` with no `max(0)` floor.
    pub expected_profit_bps: Option<f64>,
    pub expected_profit_usd: Option<f64>,
    pub optimal_size_usd: Option<f64>,
    /// Probability-aware capacity boundary (marginal p >= p_min).
    pub breakeven_size_usd: Option<f64>,
    pub capacity_ladder: Vec<CapacityTierDto>,
    /// Total expected capacity: probability-weighted fillable notional over
    /// all book levels with positive marginal expected net. `None` when no
    /// win-rate curve exists.
    pub expected_capacity_usd: Option<f64>,
    /// Fillable notional at each marginal expected-net floor (bps).
    pub bps_capacity_ladder: Vec<BpsCapacityTierDto>,
    /// Per-chunk capacity curve for the detail-modal chart.
    pub depth_curve: Vec<DepthCurvePointDto>,
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
    /// Qualification progress 0..1 while preparing.
    pub qualification_progress: f64,
    pub observed_episodes: usize,
    /// Whether Execute is allowed right now (paper path only).
    pub executable: bool,
    pub block_reason: Option<String>,
    /// Current UTC session id ("asia" | "europe" | "overlap" | "us_late" | "dead").
    pub session_id: String,
    /// True when the session aggregate is missing or provisional: every
    /// session multiplier on this row is neutral x1.0, flagged here.
    pub session_provisional: bool,
    /// Session size-allowance factor (overlap 1.0 … dead 0.4, weekend x0.5,
    /// funding mask 0.0). Always present; neutral regimes read 1.0.
    pub session_allow: f64,
    /// Combined session sizing multiplier (allow x inverse-vol x depth,
    /// halved on fast-vol breach). Applied to optimal/breakeven sizes.
    pub session_size_mult: f64,
    /// Persistence: fraction of in-window observed buckets with directional
    /// dev >= entry threshold. `None` when uncomputable (never fabricated).
    pub threshold_ratio: Option<f64>,
    /// Data quality: fraction of expected window buckets holding a
    /// both-venues observation. `None` when uncomputable (never fabricated).
    pub coverage: Option<f64>,
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

/// Per-session aggregate snapshot for one IPC response row.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionProfileEntryDto {
    pub session_id: String,
    pub n_episodes: i64,
    pub med_abs_spread_bps: Option<f64>,
    pub p90_spread_bps: Option<f64>,
    pub rv_bps: Option<f64>,
    pub win_rate: Option<f64>,
    pub mean_hold_s: Option<f64>,
    pub provisional: bool,
}

/// Snapshot behind `get_session_profile`: per-session rows, the current
/// session, and the active multipliers. Display only in Phase 1.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionPromotionDto {
    pub status: String,
    pub fit_delta_bps: f64,
    pub val_delta_bps: f64,
    pub reason: String,
    pub har_r2: Option<f64>,
    pub rescaled: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionProfileDto {
    pub symbol: String,
    pub current_session: String,
    pub is_weekend: bool,
    pub in_funding_mask: bool,
    pub us_cash_open: bool,
    pub session_allow: f64,
    pub provisional: bool,
    pub sessions: Vec<SessionProfileEntryDto>,
    pub promotion: Option<SessionPromotionDto>,
}

/* ------------------------------------------------------------------------ */
/* Session seasonality (Phase 1)                                             */
/* ------------------------------------------------------------------------ */

/// Seeded funding cadence per venue, hours. Hyperliquid funds hourly;
/// every other venue defaults to the 8h stamps (fail closed: unknown
/// cadences mask 00/08/16 UTC rather than nothing).
fn venue_funding_intervals(venue: &str) -> Vec<u32> {
    match venue.trim().to_lowercase().as_str() {
        "hyperliquid" => vec![1],
        _ => vec![8],
    }
}

/// Per-hour funding from a signed rate string and its interval. `None` when
/// the rate is unparseable or the result is non-finite — callers skip the
/// sample rather than fold unknown funding into the EMA as zero carry.
fn funding_per_hour(rate: &str, interval_hours: u32) -> Option<f64> {
    let hours = interval_hours.max(1) as f64;
    let rate_f: f64 = rate.parse().ok()?;
    let per_hour = rate_f / hours;
    per_hour.is_finite().then_some(per_hour)
}

/// Fresh per-hour funding rate for one signal leg, or `None` when the
/// venue owes a rate and hasn't reported within 5 minutes. Spot legs have
/// no funding market by design — they contribute 0 rather than blocking
/// the exit decision.
fn funding_leg_rate_hourly(
    ema: &HashMap<(String, String), (f64, Instant)>,
    venue: &str,
    symbol: &str,
    now: Instant,
) -> Option<f64> {
    if symbol.to_uppercase().ends_with("-SPOT") {
        return Some(0.0);
    }
    let base = canonical_base(symbol);
    match ema.get(&(venue.to_string(), base)) {
        Some((rate, ts)) if now.duration_since(*ts) < Duration::from_secs(300) => Some(*rate),
        _ => None,
    }
}

/// True inside the funding mask of either leg's venue.
fn pair_in_funding_mask(ts_ms: i64, buy_v: &str, sell_v: &str) -> bool {
    let mut intervals = venue_funding_intervals(buy_v);
    intervals.extend(venue_funding_intervals(sell_v));
    in_funding_mask(ts_ms, &intervals)
}

/// Resolved session inputs for one ordered pair at one timestamp.
/// Provisional or missing aggregates degrade every multiplier to neutral.
#[derive(Debug, Clone)]
struct SessionCtx {
    session: SessionId,
    funding_mask: bool,
    allow: f64,
    /// Additive entry-threshold delta (shrunk session med minus global
    /// med); exactly 0.0 when provisional or unknown.
    entry_delta_bps: f64,
    provisional: bool,
    /// Shrunk session median magnitude; `None` when unusable, which also
    /// disables the spread stand-down (no reference, no block).
    session_med: Option<f64>,
    /// Session vol norm for the inverse-vol factor; `None` reads 1.0.
    session_rv: Option<f64>,
    /// Calibrated session win rate for the confidence gate.
    win_rate: Option<f64>,
}

fn session_context(
    state: &AppEngineState,
    symbol: &str,
    buy_v: &str,
    sell_v: &str,
    now_ms: i64,
) -> SessionCtx {
    let session = session_of(now_ms);
    let weekend = is_weekend(now_ms);
    let funding_mask = pair_in_funding_mask(now_ms, buy_v, sell_v);
    let allow = session_allow(session, weekend, funding_mask);
    let neutral = SessionCtx {
        session,
        funding_mask,
        allow,
        entry_delta_bps: 0.0,
        provisional: true,
        session_med: None,
        session_rv: None,
        win_rate: None,
    };
    let profiles = match state.state_store.session_profiles(symbol) {
        Ok(p) => p,
        Err(_) => return neutral,
    };
    let meds: Vec<f64> = profiles
        .iter()
        .filter(|r| !r.provisional)
        .filter_map(|r| r.med_abs_spread_bps)
        .filter(|m| m.is_finite() && *m > 0.0)
        .collect();
    let global = winsorized_median(&meds).filter(|g| g.is_finite() && *g > 0.0);
    let row = profiles.iter().find(|r| r.session_id == session.as_str());
    let (row, global) = match (row, global) {
        (Some(r), Some(g)) => (r, g),
        _ => return neutral,
    };
    if row.provisional {
        return neutral;
    }
    let med = match row.med_abs_spread_bps {
        Some(m) if m.is_finite() && m > 0.0 => m,
        _ => return neutral,
    };
    SessionCtx {
        session,
        funding_mask,
        allow,
        entry_delta_bps: med - global,
        provisional: false,
        session_med: Some(med),
        session_rv: row.rv_bps.filter(|v| v.is_finite() && *v > 0.0),
        win_rate: row.win_rate.filter(|w| w.is_finite()),
    }
}

/// First matching session stand-down, in precedence order: funding mask,
/// Ondo-closed equity legs, spread beyond 3x the session median,
/// persistent fast-vol breach. `None` means no session block.
fn session_standdown(
    funding_mask: bool,
    ondo_closed: bool,
    dev_abs_bps: Option<f64>,
    session_med_bps: Option<f64>,
    fast_vol_halted: bool,
) -> Option<String> {
    if funding_mask {
        return Some("funding window stand-down".into());
    }
    if ondo_closed {
        return Some("ondo equity market closed".into());
    }
    if let (Some(dev), Some(med)) = (dev_abs_bps, session_med_bps) {
        if dev.is_finite() && med.is_finite() && med > 0.0 && dev > med * SPREAD_STANDDOWN_MULT {
            return Some("spread exceeds 3x session median".into());
        }
    }
    if fast_vol_halted {
        return Some("fast-vol above 3x session norm (halted)".into());
    }
    None
}

/// Halve-then-halt transition for one eval tick. Returns the consecutive
/// breach count, the size factor (0.5 on breach, 1.0 otherwise), and
/// whether the pair halts (second consecutive breach).
fn vol_halt_next(breaches: u32, ratio_above_halt: bool) -> (u32, f64, bool) {
    if !ratio_above_halt {
        return (0, 1.0, false);
    }
    let next = breaches.saturating_add(1);
    if next >= 2 {
        (next, 0.5, true)
    } else {
        (next, 0.5, false)
    }
}

/* ------------------------------------------------------------------------ */
/* Shared win-probability (monitor evaluate_pair + scan path)                 */
/* ------------------------------------------------------------------------ */

/// R² floor for the OU fallback — mirrors the matrix-baseline gate
/// (`OU_MIN_R2`) so the scan never applies a second, weaker standard.
pub(crate) const PAIR_OU_MIN_R2: f64 = 0.6;
/// Stop-band multiplier for episode segmentation, shared by both paths.
pub(crate) const PAIR_STOP_MULT: f64 = 3.0;
/// Series alignment gap (ms) shared by both paths.
pub(crate) const PAIR_ALIGN_GAP_MS: i64 = 30_000;

/// Max concurrent REST book fetches per venue lane. HTTP/1.1-only venue
/// edges open one TCP conn per in-flight request; keeping the burst small
/// stays under per-IP connection limits and protects the WS lane.
const MAX_INFLIGHT_BOOK_FETCHES: usize = 4;

/// Persisted pair lifecycle older than this does not rehydrate on startup.
/// A `triggered`/`ready` row only proves the pair qualified at
/// `last_seen_ms`; a day-stale row restarting as still-active would carry a
/// false `first_seen` age and never be pruned. 24h is comfortably beyond
/// any legitimate sampling gap (slowest lane cadence is minutes) while
/// short enough that a day-closed app re-discovers rather than resumes.
const REHYDRATE_STALE_MS: i64 = 24 * 60 * 60 * 1000;

/// Standard normal CDF via the Abramowitz-Stegun erf approximation
/// (|error| <= 1.5e-7). No extra dependency for one closed-form call.
fn normal_cdf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let ax = x.abs() / std::f64::consts::SQRT_2;
    let t = 1.0 / (1.0 + 0.3275911 * ax);
    let poly = ((((1.061405429 * t - 1.453152027) * t + 1.421413741) * t - 0.284496736) * t
        + 0.254829592)
        * t;
    let erf = 1.0 - poly * (-ax * ax).exp();
    0.5 * (1.0 + sign * erf)
}

/// OU first-passage fallback: when the episode count is below `min_episodes`
/// but the OU fit qualifies (`R² >= PAIR_OU_MIN_R2`), approximate
/// Mean inter-observation spacing of a pair series, seconds. Zero on any
/// degenerate input — the OU fit behind it treats non-positive dt as unusable.
fn series_mean_dt_secs(series: &[PairObservation]) -> f64 {
    let (Some(first), Some(last)) = (series.first(), series.last()) else {
        return 0.0;
    };
    if series.len() < 2 {
        return 0.0;
    }
    ((last.ts_ms - first.ts_ms) as f64 / 1000.0) / (series.len() - 1) as f64
}

/// `P(hit the μ-band before the stop within one half-life)` from the
/// conditional distribution at horizon `H = half-life`:
/// `P(|X_H − μ| <= band)`. `None` on any unmeasurable input (fail closed).
fn ou_fallback_p_win(
    vals: &[f64],
    series: &[PairObservation],
    mu: f64,
    current_spread_bps: f64,
    band_bps: f64,
) -> Option<f64> {
    if vals.len() < 10 || series.len() < 2 {
        return None;
    }
    let first_ts = series.first().map(|o| o.ts_ms)?;
    let last_ts = series.last().map(|o| o.ts_ms)?;
    let span_secs = (last_ts - first_ts) as f64 / 1000.0;
    if !span_secs.is_finite() || span_secs <= 0.0 {
        return None;
    }
    let dt = span_secs / (series.len() - 1) as f64;
    if !dt.is_finite() || dt <= 0.0 {
        return None;
    }
    let ou = OuModelParams::fit(vals, dt).filter(|p| {
        p.half_life.is_finite()
            && p.half_life > 0.0
            && p.theta.is_finite()
            && p.theta > 0.0
            && p.sigma.is_finite()
            && p.sigma > 0.0
            && p.r_squared.is_finite()
            && p.r_squared >= PAIR_OU_MIN_R2
    })?;
    let horizon = ou.half_life;
    if !horizon.is_finite() || horizon <= 0.0 {
        return None;
    }
    if !current_spread_bps.is_finite() {
        return None;
    }
    let mean = ou.conditional_mean(current_spread_bps, horizon);
    let var = ou.conditional_variance(horizon);
    if !mean.is_finite() || !var.is_finite() || var <= 0.0 {
        return None;
    }
    let sd = var.sqrt();
    if !sd.is_finite() || sd <= 0.0 {
        return None;
    }
    let upper = (mu + band_bps - mean) / sd;
    let lower = (mu - band_bps - mean) / sd;
    if !upper.is_finite() || !lower.is_finite() {
        return None;
    }
    let p = normal_cdf(upper) - normal_cdf(lower);
    if !p.is_finite() {
        return None;
    }
    Some(p.clamp(0.0, 1.0))
}

/// Empirical win probability for one ordered pair at one current spread —
/// the single statistical standard shared by the monitor and the scan.
///
/// Primary: episode segmentation + conditional hit-rate
/// (`build_pwin_curve` / `p_win_at_deviation`) behind the `min_episodes`
/// gate. Fallback: OU first-passage approximation behind the `PAIR_OU_MIN_R2`
/// gate. Otherwise `None` (fail closed — callers render "—").
///
/// `current_spread_bps` is the executable edge for the ordered direction
/// (sell bid vs buy ask); the baseline `μ` is the series mean, exactly as
/// `evaluate_pair` derives it. Informational only — never gates execution.
/// Z for one-sided confidence bounds used by gating — 1.645 ≈ 95%
/// one-sided (90% two-sided Wilson interval).
pub(crate) const WIN_GATE_Z: f64 = 1.645;
/// Beta-prior pseudo-count mass for win-rate shrinkage: one win + one
/// loss of prior evidence, pulling thin samples toward 0.5.
pub(crate) const WIN_PRIOR_STRENGTH: f64 = 2.0;

/// Win-rate estimate with honest uncertainty: the shrunken point estimate
/// (`p`), a Wilson lower bound on the bucket's effective trial count
/// (`lower` — the gateable quantity), the raw upper bound, the Kish
/// effective sample size, the censored-episode share, and whether `p`
/// came from the OU first-passage model rather than episode evidence.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PairWinEstimate {
    pub p: f64,
    pub lower: f64,
    pub upper: f64,
    pub n_effective: f64,
    pub censored: usize,
    /// True when `p` is the OU model fallback — model uncertainty is
    /// carried by its R² gate, so `lower == p` rather than a count bound.
    pub from_model: bool,
}

/// Shared PWinCurve construction for a pair series: the same μ/σ episode
/// bucketing `pair_win_probability_ex` evaluates a scalar against, lifted
/// so capacity walks (`capacity_ladder`) can reuse the identical win-rate
/// model rather than re-deriving statistics. Returns `(curve, mu, sigma)`;
/// `None` fail-closed on unusable inputs.
pub(crate) fn pair_pwin_curve(
    series: &[PairObservation],
    friction_bps: f64,
    half_life_secs: Option<f64>,
    now_ms: i64,
) -> Option<(arbitrageur_math::pair_monitor::PWinCurve, f64, f64)> {
    if !friction_bps.is_finite() || series.len() < 2 {
        return None;
    }
    let vals: Vec<f64> = series.iter().map(|o| o.spread_bps).collect();
    if vals.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let n = vals.len();
    let mu = vals.iter().sum::<f64>() / n as f64;
    if !mu.is_finite() {
        return None;
    }
    let var = vals.iter().map(|x| (x - mu).powi(2)).sum::<f64>() / (n - 1) as f64;
    if !var.is_finite() || var < 0.0 {
        return None;
    }
    let sigma = var.sqrt();
    if !sigma.is_finite() || sigma < 0.0 {
        return None;
    }
    let band_bps = friction_bps.max(2.0);
    if !band_bps.is_finite() || band_bps <= 0.0 {
        return None;
    }
    let curve = build_pwin_curve(
        series,
        mu,
        sigma,
        band_bps,
        PAIR_STOP_MULT,
        half_life_secs,
        now_ms,
    );
    Some((curve, mu, sigma))
}

/// Uncertainty-aware form of [`pair_win_probability`]: same episode curve
/// and OU fallback, but returns [`PairWinEstimate`] with a shrunken point
/// estimate and Wilson bounds. `half_life_secs` recency-weights episodes
/// (`0.5^(age/half_life)` on resolution time); `None`/`0` disables.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pair_win_probability_ex(
    series: &[PairObservation],
    current_spread_bps: f64,
    friction_bps: f64,
    min_episodes: usize,
    half_life_secs: Option<f64>,
    now_ms: i64,
) -> Option<PairWinEstimate> {
    if !current_spread_bps.is_finite() || !friction_bps.is_finite() {
        return None;
    }
    let (curve, mu, _sigma) = pair_pwin_curve(series, friction_bps, half_life_secs, now_ms)?;
    let band_bps = friction_bps.max(2.0);
    if curve.episodes >= min_episodes {
        let dev_abs = (current_spread_bps - mu).abs();
        // Ready episode count but no historical evidence at this
        // magnitude — fail closed rather than extrapolating.
        let est = p_win_est_at_deviation(&curve, dev_abs, WIN_GATE_Z, WIN_PRIOR_STRENGTH)?;
        let p = est.rate?;
        if !(0.0..=1.0).contains(&p) {
            return None;
        }
        let censored = curve
            .points
            .iter()
            .position(|(d, _)| *d >= dev_abs - 1e-12)
            .and_then(|i| curve.point_censored.get(i).copied())
            .unwrap_or(0);
        return Some(PairWinEstimate {
            p,
            lower: est.lower.unwrap_or(p),
            upper: est.upper.unwrap_or(p),
            n_effective: est.n_effective,
            censored,
            from_model: false,
        });
    }
    let vals: Vec<f64> = series.iter().map(|o| o.spread_bps).collect();
    ou_fallback_p_win(&vals, series, mu, current_spread_bps, band_bps).map(|p| PairWinEstimate {
        p,
        lower: p,
        upper: p,
        n_effective: 0.0,
        censored: 0,
        from_model: true,
    })
}

/// Thin wrapper retained for tests — production paths use
/// `pair_win_probability_ex` directly for the uncertainty fields.
#[cfg(test)]
pub(crate) fn pair_win_probability(
    series: &[PairObservation],
    current_spread_bps: f64,
    friction_bps: f64,
    min_episodes: usize,
) -> Option<f64> {
    pair_win_probability_ex(
        series,
        current_spread_bps,
        friction_bps,
        min_episodes,
        None,
        0,
    )
    .map(|e| e.p)
}

/// Directional trade win probability: reversion gated by every execution
/// detail. Returns:
/// - `None` when reversion is unknown, books are missing, or the
///   qualification window is unmet (genuinely uncomputable).
/// - `Some(0.0)` when the ordered direction has no edge (`dev < entry_thresh`)
///   or is hard-blocked (funding unknown, exit uncostable, no profitable
///   capacity, persistence/coverage/variation/session gates fail).
/// - `Some(reversion)` only when a trade in this direction could actually
///   execute at a profit. Session sizing (`allow` < 1) affects size, not
///   probability, so it does not discount here.
#[allow(clippy::too_many_arguments)]
pub(crate) fn trade_win_probability(
    reversion: Option<f64>,
    dev_bps: Option<f64>,
    entry_thresh_bps: f64,
    books_ok: bool,
    window_met: bool,
    funding_unknown: bool,
    exit_uncostable: bool,
    optimal_size_usd: Option<f64>,
    variation_ok: bool,
    ratio_ok: bool,
    coverage_ok: bool,
    session_conf_ok: bool,
    session_blocked: bool,
) -> Option<f64> {
    let p = reversion.filter(|v| v.is_finite() && (0.0..=1.0).contains(v))?;
    if !books_ok || !window_met {
        return None;
    }
    let dev = dev_bps.filter(|d| d.is_finite())?;
    if !entry_thresh_bps.is_finite() {
        return None;
    }
    // Wrong direction for this ordered pair: entering here bets against the
    // historical edge. P(profit) ≈ 0 regardless of reversion magnitude.
    if dev < entry_thresh_bps {
        return Some(0.0);
    }
    // Cost unknown or exit unpriceable: expectation is unknowable, but the
    // trade cannot be taken as modelled — hard-block to 0, not None, so the
    // table shows 0% instead of inheriting a misleading 100% reversion.
    if funding_unknown || exit_uncostable {
        return Some(0.0);
    }
    if optimal_size_usd
        .map(|s| !(s.is_finite() && s > 0.0))
        .unwrap_or(true)
    {
        return Some(0.0);
    }
    if !variation_ok || !ratio_ok || !coverage_ok || !session_conf_ok || session_blocked {
        return Some(0.0);
    }
    Some(p)
}

/* ------------------------------------------------------------------------ */
/* Two-way exit walk (depth-dependent, mirrors route economics)            */
/* ------------------------------------------------------------------------ */

use crate::num::dec_opt_f64;

/// Configured close quantity for the two-way exit walk, in base units: the
/// smallest positive capacity tier (USD) divided by the buy-side ask. The
/// walk prices the minimum executable close, so a thin reverse side fails
/// closed instead of hiding behind a top-of-book floor. `None` when no tier
/// or reference price is usable.
pub(crate) fn monitor_exit_qty(tiers_usd: &[f64], ref_price: Option<Decimal>) -> Option<Decimal> {
    let tier = tiers_usd
        .iter()
        .copied()
        .filter(|t| t.is_finite() && *t > 0.0)
        .fold(None::<f64>, |best, t| {
            Some(best.map(|b: f64| b.min(t)).unwrap_or(t))
        })?;
    let price = dec_opt_f64(ref_price?)?;
    if !price.is_finite() || price <= 0.0 {
        return None;
    }
    let qty = Decimal::from_f64_retain(tier / price)?;
    if qty <= dec!(0) {
        return None;
    }
    dec_opt_f64(qty)
        .filter(|f| f.is_finite() && *f > 0.0)
        .map(|_| qty)
}

/// Expected two-way exit friction in bps: exit VWAP slippage on BOTH reverse
/// sides at `qty` + exit fees + closing spread. Formula-for-formula the exit
/// half of `compute_route_economics` (no second model): the buy leg unwinds
/// into the buy book's bids, the sell leg lifts the sell book's asks.
///
/// Fail-closed `None` when either reverse side lacks depth at `qty`, a book
/// is empty/crossed, `qty` is non-positive, or any input is non-finite — the
/// caller must then treat the route as uncostable for two-way mode.
pub(crate) fn two_way_exit_estimate(
    book_buy: &OrderBook,
    book_sell: &OrderBook,
    qty: Decimal,
    exit_fee_bps: f64,
) -> Option<f64> {
    if qty <= dec!(0) || !exit_fee_bps.is_finite() || exit_fee_bps < 0.0 {
        return None;
    }
    let buy_best_bid = dec_opt_f64(book_buy.best_bid_price()?).filter(|p| *p > 0.0)?;
    let sell_best_ask = dec_opt_f64(book_sell.best_ask_price()?).filter(|p| *p > 0.0)?;
    let buy_best_ask = dec_opt_f64(book_buy.best_ask_price()?).filter(|p| *p > 0.0)?;
    let sell_best_bid = dec_opt_f64(book_sell.best_bid_price()?).filter(|p| *p > 0.0)?;
    if !(buy_best_ask > buy_best_bid && sell_best_ask > sell_best_bid) {
        return None;
    }
    let (buy_exit, rem, _) = book_buy.sell_execution_vwap(qty)?;
    if rem > dec!(0) {
        return None;
    }
    let (sell_exit, rem, _) = book_sell.buy_execution_vwap(qty)?;
    if rem > dec!(0) {
        return None;
    }
    let buy_exit_vwap = dec_opt_f64(buy_exit)?;
    let sell_exit_vwap = dec_opt_f64(sell_exit)?;
    let exit_slippage_bps = ((buy_best_bid - buy_exit_vwap) / buy_best_bid * 10_000.0).max(0.0)
        + ((sell_exit_vwap - sell_best_ask) / sell_best_ask * 10_000.0).max(0.0);
    let buy_spread_bps =
        (buy_best_ask - buy_best_bid) / ((buy_best_ask + buy_best_bid) / 2.0) * 10_000.0;
    let sell_spread_bps =
        (sell_best_ask - sell_best_bid) / ((sell_best_ask + sell_best_bid) / 2.0) * 10_000.0;
    let out = exit_fee_bps + exit_slippage_bps + buy_spread_bps + sell_spread_bps;
    if out.is_finite() {
        Some(out)
    } else {
        None
    }
}

/// Composes the exit-friction input for one pair evaluation from the
/// execution mode. One-way mode never prices a close (`Some(0.0)` whatever
/// the books hold — untouched by the depth walk). Two-way mode walks both
/// reverse sides at the configured close qty; `None` marks the route
/// uncostable and the caller must fail closed (cap at triggered, block
/// execution, record it in the audit snapshot).
pub(crate) fn exit_estimate_for_mode(
    execution_mode: &str,
    buy_book: &Option<OrderBook>,
    sell_book: &Option<OrderBook>,
    tiers_usd: &[f64],
    exit_fee_bps: f64,
) -> Option<f64> {
    if !crate::commands::execution_mode_is_two_way(execution_mode) {
        return Some(0.0);
    }
    let (bk, sk) = buy_book.as_ref().zip(sell_book.as_ref())?;
    let qty = monitor_exit_qty(tiers_usd, bk.best_ask_price())?;
    two_way_exit_estimate(bk, sk, qty, exit_fee_bps)
}

/// Which audit write one pair evaluation owes the log, if any: first
/// sighting and status changes log a `transition`; a still-active signal
/// re-logs one `heartbeat` per minute bucket; everything else (same-minute
/// re-polls, inactive steady state) logs nothing — idempotent by
/// construction so repeated polls can never duplicate events.
pub(crate) fn audit_trigger(
    prev_status: Option<PairStatus>,
    status: PairStatus,
    signal_active: bool,
    window_met: bool,
    prev_last_seen_ms: Option<i64>,
    now_ms: i64,
) -> Option<&'static str> {
    match prev_status {
        None => Some("transition"),
        Some(prev) if prev != status => Some("transition"),
        Some(_)
            if signal_active
                && window_met
                && (prev_last_seen_ms.unwrap_or(0) / 60_000) != (now_ms / 60_000) =>
        {
            Some("heartbeat")
        }
        _ => None,
    }
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
fn venue_budget_per_min(venue: &str, tiers: &[String], _flag_enabled: bool) -> (f64, String) {
    match venue {
        "lighter" | "robinhood" => {
            // Lighter applies the lowest account limit across an IP.
            let tier = tiers
                .iter()
                .map(String::as_str)
                .min_by_key(|t| lighter_tier_budget_per_min(t) as u64)
                .unwrap_or("standard");
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
    /// The fair-share interval carries ±20% jitter so lanes sharing an IP
    /// do not poll in lockstep; `symbol_interval` itself stays pure.
    fn due_symbols(&mut self, cfg: &MonitorConfigDto) -> Vec<String> {
        let interval = arbitrageur_exchanges::ws::jittered_duration_20(self.symbol_interval(cfg));
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
pub(crate) enum PairStatus {
    Preparing,
    Tracking,
    Triggered,
    Ready,
    NoData,
}

impl PairStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::Tracking => "tracking",
            Self::Triggered => "triggered",
            Self::Ready => "ready",
            Self::NoData => "no-data",
        }
    }
    fn from_str(s: &str) -> Self {
        match s {
            "tracking" => Self::Tracking,
            "triggered" => Self::Triggered,
            "ready" => Self::Ready,
            "no-data" => Self::NoData,
            _ => Self::Preparing,
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

/// One audit-log write derived from a pair evaluation: a lifecycle
/// transition (status changed, or first sighting with `old_status = None`)
/// or a minute-bucketed heartbeat while the signal stays active. Same-tick
/// re-evaluations inside one minute bucket produce no draft, so repeated
/// polls are idempotent and the log cannot spam.
#[derive(Debug, Clone)]
struct SignalEventDraft {
    symbol: String,
    buy_venue: String,
    sell_venue: String,
    ts_ms: i64,
    old_status: Option<String>,
    new_status: String,
    edge_bps: Option<f64>,
    dev_bps: Option<f64>,
    p_win: Option<f64>,
    economics_json: String,
    trigger: &'static str,
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
    /// (venue, canonical base) → (per-hour funding rate, last sample).
    /// The timestamp keeps the exit supervisor honest: a stale lane can
    /// never decide a position's fate.
    funding_ema: Mutex<HashMap<(String, String), (f64, Instant)>>,
    /// Canonical-base watchlist (exempted symbols removed).
    watchlist: Mutex<Vec<String>>,
    /// All canonical bases per venue from the last market refresh.
    venue_symbols: Mutex<HashMap<String, Vec<String>>>,
    last_watchlist_at: Mutex<Option<Instant>>,
    last_snapshot: Mutex<Option<MonitorSnapshotDto>>,
    /// In-memory EWMA fast vol per ordered pair (O(1)/tick). Durable
    /// session norms live in SQLite; this tracker resets on restart.
    fast_vol: Mutex<HashMap<(String, String, String), FastVolEwma>>,
    /// Consecutive fast-vol breaches per ordered pair (halve-then-halt).
    vol_halt: Mutex<HashMap<(String, String, String), u32>>,
    /// Funding-exit supervisor cadence: evaluates open carry positions
    /// against the entry APR hurdle at most once per 60s.
    last_funding_exit_check: Mutex<Option<Instant>>,
    /// Last hourly session-rollup pass over the trailing spread log.
    last_rollup_at: Mutex<Option<Instant>>,
    /// Last nightly seasonality-promotion pass (HAR anchors + walk-forward
    /// verdicts). At most one pass per 24h; manual IPC shares the same
    /// runner — never a second code path.
    last_promotion_at: Mutex<Option<Instant>>,
    /// Dedup guard for durable signal writes (only persist transitions).
    #[allow(clippy::type_complexity)]
    persisted: Mutex<HashMap<(String, String, String), (PairStatus, i64, i64)>>,
    shutdown: Mutex<bool>,
}

pub use crate::state_store::canonical_base;

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
            fast_vol: Mutex::new(HashMap::new()),
            vol_halt: Mutex::new(HashMap::new()),
            last_funding_exit_check: Mutex::new(None),
            last_rollup_at: Mutex::new(None),
            last_promotion_at: Mutex::new(None),
            persisted: Mutex::new(HashMap::new()),
            shutdown: Mutex::new(false),
        });
        // Rehydrate lifecycle state so discovery timestamps survive restart.
        // Rows whose pair hasn't been sampled within the staleness bound are
        // skipped: a persisted "triggered" only proves the pair qualified at
        // `last_seen_ms`, and resurrecting it as still-active would keep its
        // original `first_seen` (lying about signal age) and exempt it from
        // `prune_monitor_signals` forever — rehydrate loads every row into
        // `pairs`, and prune keeps every `pairs` key. Skipped rows are not in
        // `pairs`, so the next universe prune deletes them cleanly.
        let now_ms = Utc::now().timestamp_millis();
        if let Ok(rows) = state.state_store.monitor_signals() {
            let mut pairs = engine.pairs.lock();
            let mut skipped = 0usize;
            for r in rows {
                if now_ms.saturating_sub(r.last_seen_ms) > REHYDRATE_STALE_MS {
                    skipped += 1;
                    continue;
                }
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
            if skipped > 0 {
                tracing::info!(
                    "monitor: skipped {skipped} stale persisted signal(s) on rehydrate (>{REHYDRATE_STALE_MS}ms since last sample)"
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
        cfg.min_expected_profit_bps = cfg.min_expected_profit_bps.clamp(-10_000.0, 10_000.0);
        cfg.min_threshold_ratio = if cfg.min_threshold_ratio.is_finite() {
            cfg.min_threshold_ratio.clamp(0.0, 1.0)
        } else {
            d_min_ratio()
        };
        cfg.min_coverage = if cfg.min_coverage.is_finite() {
            cfg.min_coverage.clamp(0.0, 1.0)
        } else {
            d_min_coverage()
        };
        cfg.episode_half_life_secs = if cfg.episode_half_life_secs.is_finite() {
            cfg.episode_half_life_secs.clamp(0.0, 30.0 * 86_400.0)
        } else {
            d_episode_hl()
        };
        cfg.min_win_lower_bound = if cfg.min_win_lower_bound.is_finite() {
            cfg.min_win_lower_bound.clamp(0.0, 0.999)
        } else {
            d_min_win_lb()
        };
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

    /// Venues currently listing `base` in the derived >= 2-venue
    /// intersection — the in-scope set for per-strategy auto-filters.
    pub fn in_scope_venues(&self, base: &str) -> Vec<String> {
        let wanted = canonical_base(base);
        let map = self.venue_symbols.lock();
        let mut out: Vec<String> = map
            .iter()
            .filter(|(_, symbols)| symbols.iter().any(|s| s == &wanted))
            .map(|(venue, _)| venue.clone())
            .collect();
        out.sort();
        out
    }

    /// Test seam: install a fabricated snapshot without running the loop.
    #[cfg(test)]
    pub fn inject_snapshot(&self, snap: MonitorSnapshotDto) {
        *self.last_snapshot.lock() = Some(snap);
    }

    /// Seam for the future auto-trading bot: only fully ready rows.
    /// Not consumed by the v1 UI — the bot contract is part of the spec.
    #[allow(dead_code)]
    pub fn ready_opportunities(&self) -> Vec<MonitorRowDto> {
        self.snapshot()
            .map(|s| {
                s.rows
                    .into_iter()
                    .filter(|r| r.status == "ready" && r.executable)
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
            // Live mode keeps the monitor running: it is the hub's REST
            // feeder for venues/symbols the WS lanes don't cover and the
            // only funding-rate refresher. Its execution path stays gated
            // and gated downstream (live-mode refusal at the executor plus
            // the explicit live-entry gate), so suspending here bought no
            // safety — it only darkened the radar and starved the hub.
            if *state.is_paused.read() || !self.cfg.lock().enabled {
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }

            self.maybe_refresh_watchlist(&state).await;
            self.poll_due_books(&state).await;
            self.poll_funding(&state).await;
            self.maybe_rollup_sessions(&state);
            self.maybe_promote_sessions(&state);

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

    /// Hourly session-rollup pass: rebuild every sampled symbol's session
    /// aggregates over the trailing 7 days. At most one pass per hour;
    /// failures only log (a missed rollup retries next hour).
    fn maybe_rollup_sessions(&self, state: &AppEngineState) {
        let due = self
            .last_rollup_at
            .lock()
            .map(|t| t.elapsed() >= Duration::from_secs(3600))
            .unwrap_or(true);
        if !due {
            return;
        }
        *self.last_rollup_at.lock() = Some(Instant::now());
        let now_ms = Utc::now().timestamp_millis();
        let from_ms = now_ms - 7 * 86_400_000;
        let symbols = match state.state_store.distinct_sample_symbols() {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("session rollup: symbol scan failed: {e}");
                return;
            }
        };
        for symbol in symbols.iter().take(500) {
            if let Err(e) = state
                .state_store
                .rollup_session_profiles(symbol, from_ms, now_ms, now_ms)
            {
                tracing::warn!("session rollup failed for {symbol}: {e}");
            }
        }
    }

    fn maybe_promote_sessions(&self, state: &AppEngineState) {
        let due = self
            .last_promotion_at
            .lock()
            .map(|t| t.elapsed() >= Duration::from_secs(86_400))
            .unwrap_or(true);
        if !due {
            return;
        }
        *self.last_promotion_at.lock() = Some(Instant::now());
        let now_ms = Utc::now().timestamp_millis();
        match crate::seasonality::run_seasonality_promotion(state, now_ms) {
            Ok(summary) => tracing::info!(
                "seasonality promotion: scanned={} anchors={} promoted={} demoted={} held={} skipped={}",
                summary.symbols_scanned,
                summary.anchors_written,
                summary.promoted,
                summary.demoted,
                summary.held,
                summary.skipped,
            ),
            Err(e) => tracing::warn!("seasonality promotion failed: {e}"),
        }
    }

    /// Spend each lane's usable budget on its due symbols, one fetch per
    /// (venue, symbol). WS-fresh hub books skip the REST fetch entirely
    /// (no budget spent) but are still validated and sampled. Books go to
    /// the hub; top-of-book is persisted to `spread_samples` for the
    /// historical baseline. Funding cadence is unchanged (no WS channel).
    async fn poll_due_books(&self, state: &AppEngineState) {
        struct Job {
            venue: String,
            symbol: String,
            adapter: Arc<dyn ExchangeAdapter>,
        }
        let mut jobs: Vec<Job> = Vec::new();
        let mut ws_hits: Vec<(String, String, OrderBook)> = Vec::new();
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
                loop {
                    let due = lane.due_symbols(&cfg);
                    if due.is_empty() {
                        break;
                    }
                    let sym = due[0].clone();
                    // Advance the cursor past this symbol.
                    if let Some(pos) = lane.symbols.iter().position(|s| *s == sym) {
                        lane.cursor = (pos + 1) % lane.symbols.len().max(1);
                    }
                    let venue_id = lane.adapter.venue_id();
                    if let Some(book) = state
                        .market_data_hub
                        .get_valid_ws_book(&venue_id, &sym, None)
                    {
                        if validate_monitor_book(&book, &venue_id, &sym).is_ok() {
                            // Mark the symbol fetched: without this stamp
                            // `due_symbols()` keeps returning it and this
                            // loop never terminates on a WS-fresh lane.
                            // No budget is spent (no REST fetch ran).
                            lane.last_fetch.insert(sym.clone(), Instant::now());
                            ws_hits.push((lane.venue.clone(), sym, book));
                            continue;
                        }
                    }
                    if !lane.can_spend_n(cost) {
                        break;
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
        let now_ms = Utc::now().timestamp_millis();
        for (venue, symbol, book) in ws_hits {
            if let Some(entry) = book_sample(&book, &venue) {
                let _ = state
                    .state_store
                    .record_spread_samples(&symbol, now_ms, &[entry]);
            }
        }
        if jobs.is_empty() {
            return;
        }
        // Fetches run concurrently — a slow venue must not serialize the
        // whole cycle; each job's lane is keyed back on completion.
        // Concurrent identical keys share one request via the single-flight
        // map instead of firing N.
        //
        // Bound in-flight requests per venue: venue edges that only speak
        // HTTP/1.1 (e.g. api.hyperliquid.xyz negotiates no ALPN) open one
        // TCP conn per in-flight request. An unbounded burst plus keep-alive
        // linger trips the edge's per-IP connection limit, which then RSTs
        // unrelated new conns — including the venue's own WS upgrade.
        let flights = state.orderbook_flights.clone();
        let mut set = tokio::task::JoinSet::new();
        let mut venue_sems: HashMap<String, Arc<tokio::sync::Semaphore>> = HashMap::new();
        for job in jobs {
            let flights = flights.clone();
            let sem = venue_sems
                .entry(job.venue.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(MAX_INFLIGHT_BOOK_FETCHES)))
                .clone();
            set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                let res = flights.fetch(&job.adapter, &job.symbol).await;
                (job.venue, job.symbol, res)
            });
        }
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
                        let Some(per_hour) =
                            funding_per_hour(&fr.rate.to_string(), fr.interval_hours)
                        else {
                            continue;
                        };
                        let key = (venue.clone(), base);
                        let next = match ema.get(&key) {
                            Some((prev, _)) => 0.7 * prev + 0.3 * per_hour,
                            None => per_hour,
                        };
                        ema.insert(key, (next, Instant::now()));
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
        self.supervise_funding_exits(state).await;
    }

    /// Funding-exit supervisor: an open carry/basis position whose
    /// EMA-smoothed net funding no longer clears the entry hurdle pays (or
    /// soon pays) to stay open — flatten it through the proven
    /// symbol-scoped path so the position never rides past its thesis.
    /// Runs at most once per 60s; evaluates `Balanced` signals only
    /// (in-flight/imbalanced/unresolved belong to the reconciler).
    /// A leg with no funding market (spot) reads 0; a perp leg requires a
    /// fresh (<5min) EMA sample — stale lanes never decide.
    async fn supervise_funding_exits(&self, state: &AppEngineState) {
        {
            let mut last = self.last_funding_exit_check.lock();
            if last.is_some_and(|t| t.elapsed() < Duration::from_secs(60)) {
                return;
            }
            *last = Some(Instant::now());
        }
        let min_apr = crate::funding::funding_min_apr(state);
        let signals = match state.state_store.list_open_signals() {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!("monitor: funding-exit check skipped (signals unreadable): {e}");
                return;
            }
        };
        let now = Instant::now();
        let ema = self.funding_ema.lock().clone();
        for sig in signals.iter().filter(|s| {
            s.strategy_id.eq_ignore_ascii_case("funding")
                && s.status == crate::state_store::SignalStatus::Balanced
                && !s.id.starts_with("flat-")
                && !s.id.starts_with("trim-")
        }) {
            let (long_leg, short_leg) = (
                sig.legs
                    .iter()
                    .find(|l| l.side == crate::state_store::LegSide::Long),
                sig.legs
                    .iter()
                    .find(|l| l.side == crate::state_store::LegSide::Short),
            );
            let (Some(long_leg), Some(short_leg)) = (long_leg, short_leg) else {
                continue;
            };
            // net carry = short_leg rate − long_leg rate (same convention
            // as the entry scanner).
            let (Some(lr), Some(sr)) = (
                funding_leg_rate_hourly(&ema, &long_leg.venue, &long_leg.symbol, now),
                funding_leg_rate_hourly(&ema, &short_leg.venue, &short_leg.symbol, now),
            ) else {
                continue;
            };
            let net_apr_pct = (sr - lr) * 8760.0 * 100.0;
            if net_apr_pct < min_apr {
                tracing::warn!(
                    "monitor: funding carry {} net APR {:.1}% below the {:.1}% exit hurdle — flattening",
                    sig.id,
                    net_apr_pct,
                    min_apr
                );
                match crate::commands::flatten_ticker_positions_impl(
                    Some(&sig.symbol),
                    "funding",
                    arbitrageur_execution::virtual_ledger::CycleStatus::StoppedOut,
                    state,
                )
                .await
                {
                    Ok(res) => tracing::info!(
                        "monitor: funding exit {} flattened ({} closed, {} residual)",
                        sig.id,
                        res.signals_closed,
                        res.residual_signals
                    ),
                    Err(e) => tracing::error!(
                        "monitor: funding exit {} flatten failed: {e} — retrying next pass",
                        sig.id
                    ),
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
        let fee_map: HashMap<String, f64> = lane_meta
            .iter()
            .map(|(v, _, _, _, _, _)| {
                (
                    v.clone(),
                    crate::commands::effective_venue_fee_bps(state, v),
                )
            })
            .collect();
        let lanes_used: Vec<usize> = {
            let mut lanes = self.lanes.lock();
            lanes.iter_mut().map(|l| l.used_last_min()).collect()
        };

        // Participation-cap volume map (cached 5 min upstream).
        let volumes = crate::commands::cached_venue_volumes(state).await;

        // Session depth snapshot: trailing-7d volume series per symbol and
        // venue, read once per eval pass so every pair shares one query.
        let depth_from_ms = now_ms - 7 * 86_400_000;
        let mut depth_snapshots: HashMap<String, HashMap<String, (f64, f64)>> = HashMap::new();

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
        #[allow(clippy::type_complexity)]
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
        let mut event_batch: Vec<SignalEventDraft> = Vec::new();

        for symbol in &watchlist {
            // Alias-aware read merges the monitor/feed base key ("BTC")
            // with UI full-symbol keys ("BTC-USDC") so the whole history
            // feeds the baseline regardless of which path produced it.
            let samples = match state
                .state_store
                .spread_samples(symbol, history_from, now_ms)
            {
                Ok(s) => s,
                Err(_) => continue,
            };
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
            let mut depth_snapshot: HashMap<String, (f64, f64)> = HashMap::new();
            for venue in &venues_here {
                if let Ok(series) =
                    state
                        .state_store
                        .venue_volume_series(symbol, venue, depth_from_ms, now_ms)
                {
                    let vols: Vec<f64> = series.iter().map(|(_, v)| *v).collect();
                    if let (Some((_, cur)), Some(med)) = (series.last(), winsorized_median(&vols)) {
                        if cur.is_finite() && *cur > 0.0 && med.is_finite() && med > 0.0 {
                            depth_snapshot.insert(venue.clone(), (*cur, med));
                        }
                    }
                }
            }
            depth_snapshots.insert(symbol.clone(), depth_snapshot);

            for (i, buy_v) in venues_here.iter().enumerate() {
                for (j, sell_v) in venues_here.iter().enumerate() {
                    if i == j {
                        continue;
                    }
                    let empty_depths: HashMap<String, (f64, f64)> = HashMap::new();
                    let depths = depth_snapshots.get(symbol).unwrap_or(&empty_depths);
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
                        depths,
                    ) {
                        if let Some(p) = &row.1 {
                            persist_batch.push(p.clone());
                        }
                        if let Some(ev) = row.2 {
                            event_batch.push(ev);
                        }
                        rows.push(row.0);
                    }
                }
            }
        }

        // Sort by expected profit bps desc; not-ready rows below ready rows.
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
                if changed
                    && state
                        .state_store
                        .upsert_monitor_signal(&sym, &bv, &sv, status.as_str(), fs, ls, edge, dev)
                        .is_ok()
                {
                    persisted.insert(key, (status, fs, ls));
                }
            }
        }

        // Audit history: one row per transition/heartbeat draft. Best-effort
        // like the lifecycle upsert above — a store failure must never break
        // the eval pass. Drafts are already deduped at construction
        // (transitions + one heartbeat per minute per active pair), so this
        // loop never spams on repeated polls.
        for ev in event_batch {
            let _ = state.state_store.record_monitor_signal_event(
                &ev.symbol,
                &ev.buy_venue,
                &ev.sell_venue,
                ev.ts_ms,
                ev.old_status.as_deref(),
                &ev.new_status,
                ev.edge_bps,
                ev.dev_bps,
                ev.p_win,
                Some(ev.economics_json.as_str()),
                ev.trigger,
            );
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
    /// persist tuple when lifecycle state changed materially, plus an audit
    /// draft on transitions and minute-bucketed heartbeats (idempotent —
    /// same-minute re-polls yield no draft).
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
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
        depths: &HashMap<String, (f64, f64)>,
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
        Option<SignalEventDraft>,
    )> {
        let key = (symbol.to_string(), buy_v.to_string(), sell_v.to_string());

        // Session regime for this pair at this timestamp. Provisional or
        // missing aggregates degrade every multiplier below to neutral.
        let sess = session_context(state, symbol, buy_v, sell_v, now_ms);
        let ondo_closed = (is_ondo_venue(buy_v) || is_ondo_venue(sell_v)) && !us_cash_open(now_ms);

        // --- live books ------------------------------------------------
        let buy_book = state.market_data_hub.get_valid_orderbook(
            &arbitrageur_core::types::VenueId(buy_v.to_string()),
            symbol,
            Some(stale_ms),
        );
        let sell_book = state.market_data_hub.get_valid_orderbook(
            &arbitrageur_core::types::VenueId(sell_v.to_string()),
            symbol,
            Some(stale_ms),
        );
        let books_ok = buy_book.is_some() && sell_book.is_some();

        // Symbol-collision guard: venues quoting irreconcilably different
        // mids for one canonical symbol are different instruments under a
        // shared ticker (e.g. an index perp vs a same-named memecoin), not a
        // dislocation. Real cross-venue spreads are basis points — >2x can
        // never be a tradeable divergence, so the pair is refused outright.
        if books_ok {
            let mid_of = |b: &OrderBook| -> Option<f64> {
                b.mid_price()
                    .and_then(|m| m.to_string().parse::<f64>().ok())
                    .filter(|m| m.is_finite() && *m > 0.0)
            };
            if let (Some(bm), Some(sm)) = (
                mid_of(buy_book.as_ref().expect("books_ok")),
                mid_of(sell_book.as_ref().expect("books_ok")),
            ) {
                if (bm / sm).max(sm / bm) > 2.0 {
                    return None;
                }
            }
        }

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
            let dt = series_mean_dt_secs(&series);
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
        let fee_bps = fee_map.get(buy_v).copied().unwrap_or(4.0)
            + fee_map.get(sell_v).copied().unwrap_or(4.0);
        let fr_a = self
            .funding_ema
            .lock()
            .get(&(buy_v.to_string(), symbol.to_string()))
            .map(|(r, _)| *r);
        let fr_b = self
            .funding_ema
            .lock()
            .get(&(sell_v.to_string(), symbol.to_string()))
            .map(|(r, _)| *r);
        // t_hold: OU half-life -> empirical median -> None.
        let stats_for_t = if let (Some(m), Some(s)) = (mu, sigma) {
            Some(reversion_stats(&series, m, s, 0.0, 2.0, 3.0, 1))
        } else {
            None
        };
        let t_hold_secs = half_life_secs.or(stats_for_t.and_then(|s| s.median_converge_secs));
        let carry = funding_carry_bps(fr_a, fr_b, t_hold_secs);
        let funding_unknown = carry.is_none();
        // Depth-dependent unwind for two-way mode: walk BOTH reverse sides
        // at the configured close qty — exit VWAP slippage + exit fees +
        // closing spread, the exit half of `compute_route_economics` with no
        // second model. Either reverse side thin at that qty => route
        // uncostable (fail closed below). One-way mode is untouched.
        // Per-ticker override: the Ticker Configurator's Execution Mode is
        // persisted on the symbol's ticker config and wins over the global
        // monitor policy when present (both vocabularies normalize through
        // `execution_mode_is_two_way`).
        let ticker_mode = state
            .strategy_configs
            .read()
            .get(symbol)
            .map(|c| c.execution_mode.clone());
        let exit_est_opt = exit_estimate_for_mode(
            ticker_mode.as_deref().unwrap_or(&cfg.execution_mode),
            &buy_book,
            &sell_book,
            &cfg.capacity_tiers_usd,
            fee_bps,
        );
        let exit_uncostable = exit_est_opt.is_none();
        let friction_bps = fee_bps + carry.unwrap_or(0.0).max(0.0) + exit_est_opt.unwrap_or(0.0);

        // --- current executable edge ------------------------------------
        let (edge_bps, dev_bps) = match (&buy_book, &sell_book, mu) {
            (Some(bk), Some(sk), Some(m)) => {
                let ask = bk
                    .asks
                    .first()
                    .and_then(|l| l.price.to_string().parse::<f64>().ok());
                let bid = sk
                    .bids
                    .first()
                    .and_then(|l| l.price.to_string().parse::<f64>().ok());
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
        let episode_hl = (cfg.episode_half_life_secs > 0.0).then_some(cfg.episode_half_life_secs);
        let pwin_curve = if let (Some(m), Some(s)) = (mu, sigma) {
            build_pwin_curve(&series, m, s, band_bps, PAIR_STOP_MULT, episode_hl, now_ms)
        } else {
            arbitrageur_math::pair_monitor::PWinCurve {
                points: Vec::new(),
                episodes: 0,
                point_wins: Vec::new(),
                point_trials: Vec::new(),
                point_censored: Vec::new(),
                point_sum_w2: Vec::new(),
            }
        };
        let episodes = pwin_curve.episodes;
        // Single statistical standard shared with the scan path: empirical
        // hit-rate behind the min-episodes gate, OU first-passage fallback
        // behind the R² gate, else fail-closed None. The estimate carries
        // a shrunken point rate plus a Wilson lower bound — the gateable
        // quantity on thin evidence.
        let p_est = edge_bps.and_then(|e| {
            pair_win_probability_ex(
                &series,
                e,
                friction_bps,
                cfg.min_episodes,
                episode_hl,
                now_ms,
            )
        });
        let p_win = p_est.map(|e| e.p);

        // Capacity ladder over both books — requires a real historical
        // baseline; mu=0.0 would fabricate a reference price, so no ladder.
        let (ladder, optimal, breakeven, exp_usd, exp_bps, exp_capacity, bps_ladder, depth_curve) =
            match (&buy_book, &sell_book, mu) {
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
                            tiers.sort_by(|a, b| {
                                a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                            });
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
                        // Marginal tier gate on the shrunken rate (thin
                        // buckets pull toward 0.5) — sizing already scales
                        // by p, so the row-level lower bound stays the
                        // strict gate.
                        None,
                        WIN_PRIOR_STRENGTH,
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
                        rep.expected_capacity_usd,
                        rep.bps_capacity_ladder
                            .iter()
                            .map(|t| BpsCapacityTierDto {
                                min_net_bps: t.min_net_bps,
                                capacity_usd: t.capacity_usd,
                            })
                            .collect(),
                        rep.depth_curve
                            .iter()
                            .map(DepthCurvePointDto::from)
                            .collect(),
                    )
                }
                _ => (
                    Vec::new(),
                    None,
                    None,
                    None,
                    None,
                    None,
                    Vec::new(),
                    Vec::new(),
                ),
            };

        let dev_abs_bps = dev_bps.map(|d| d.abs());
        // Net-P&L outcome distribution for a fresh entry at the current
        // deviation: every historical excursion that crossed this depth,
        // replayed to its real exit (reversion band, stop, or censored),
        // scored as sign(e0)·(e0 − dev_exit) − friction. Gives P(net>0)
        // with a Wilson bound plus net-bps deciles — the same honest
        // semantics the funding monitor's gap backtest uses, so the two
        // radars now share one "win" definition: net profit > 0.
        let outcome_stats = match (mu, sigma, dev_abs_bps) {
            (Some(m), Some(s), Some(d)) if d > band_bps => {
                let stop_bps = (s * PAIR_STOP_MULT).max(band_bps * 2.0);
                let outcomes = simulate_entry_outcomes(
                    &series,
                    m,
                    d,
                    band_bps,
                    stop_bps,
                    friction_bps,
                    episode_hl,
                    now_ms,
                );
                Some(entry_outcome_stats(
                    &outcomes,
                    cfg.min_episodes,
                    WIN_GATE_Z,
                    WIN_PRIOR_STRENGTH,
                ))
            }
            _ => None,
        };
        let mut ewma = self.fast_vol.lock().remove(&key).unwrap_or_default();
        if let Some(dev) = dev_abs_bps {
            ewma.update(dev);
        }
        let fast_ratio = sess.session_rv.and_then(|norm| ewma.ratio_to(norm));
        let breached = fast_ratio
            .map(|r| r.is_finite() && r > FAST_VOL_HALT_MULT)
            .unwrap_or(false);
        let prior_flags = self.vol_halt.lock().get(&key).copied().unwrap_or(0);
        let (next_flags, halve_factor, fast_halted) = vol_halt_next(prior_flags, breached);
        if breached || prior_flags > 0 {
            self.vol_halt.lock().insert(key.clone(), next_flags);
        }
        self.fast_vol.lock().insert(key.clone(), ewma.clone());
        let fast_value = ewma.value();
        let inv_vol = match (sess.session_rv, fast_value) {
            (Some(norm), Some(fast)) => inverse_vol_mult(norm, fast),
            _ => 1.0,
        };
        let depth_factor = match (depths.get(buy_v), depths.get(sell_v)) {
            (Some((cur_b, med_b)), Some((cur_s, med_s))) => {
                depth_mult(cur_b.min(*cur_s), med_b.min(*med_s))
            }
            _ => 1.0,
        };
        let session_size_mult = sess.allow * inv_vol * depth_factor * halve_factor;
        let scale_opt = |v: Option<f64>| {
            v.map(|x| {
                if x.is_finite() {
                    x * session_size_mult
                } else {
                    x
                }
            })
        };
        let optimal = scale_opt(optimal);
        let breakeven = scale_opt(breakeven);
        let exp_usd = scale_opt(exp_usd);
        let session_block = session_standdown(
            sess.funding_mask,
            ondo_closed,
            dev_abs_bps,
            sess.session_med,
            fast_halted,
        );
        let session_conf_ok = sess.provisional
            || sess
                .win_rate
                .map(|w| w >= cfg.min_win_probability)
                .unwrap_or(true);

        // --- status + lifecycle ------------------------------------------
        // Directional: a positive dev means this ordered pair (buy A / sell
        // B) is the profitable side; a negative dev belongs to the reverse
        // pair row, so it must not trigger here. The session aggregate
        // shifts the hurdle additively (capped at 2x); provisional reads 0.
        let entry_thresh = apply_entry_adjust_bps(friction_bps.max(1.0), sess.entry_delta_bps);
        let signal_active = dev_bps.map(|d| d >= entry_thresh).unwrap_or(false);
        let variation_ok = window_met
            && variation_covers_friction(
                &series,
                mu.unwrap_or(0.0),
                window_secs,
                cfg,
                friction_bps,
            );

        // Persistence (above-threshold ratio) + data-quality (effective
        // coverage) gates over the trailing qualification window ending now.
        // Pure helpers in pair_monitor (documented there): directional
        // dev >= entry_thresh, buckets of PAIR_BUCKET_MS, None when
        // uncomputable, never fabricated. Floors at 0.0 disable each gate;
        // uncomputable with an active floor blocks (never force-qualifies).
        let threshold_ratio = match mu {
            Some(m) => above_threshold_ratio(&series, m, entry_thresh, window_secs, now_ms),
            None => None,
        };
        let coverage = effective_coverage(&series, window_secs, now_ms, PAIR_BUCKET_MS);
        let ratio_floor = if cfg.min_threshold_ratio.is_finite() {
            cfg.min_threshold_ratio.max(0.0)
        } else {
            d_min_ratio()
        };
        let cov_floor = if cfg.min_coverage.is_finite() {
            cfg.min_coverage.max(0.0)
        } else {
            d_min_coverage()
        };
        let ratio_ok = if ratio_floor <= 0.0 {
            true
        } else {
            threshold_ratio.map(|r| r >= ratio_floor).unwrap_or(false)
        };
        let coverage_ok = if cov_floor <= 0.0 {
            true
        } else {
            coverage.map(|c| c >= cov_floor).unwrap_or(false)
        };
        let mut gate_reasons: Vec<String> = Vec::new();
        if !ratio_ok {
            match threshold_ratio {
                Some(r) => {
                    gate_reasons.push(format!("persistence {r:.2} < floor {ratio_floor:.2}"))
                }
                None => gate_reasons.push(format!("persistence unknown < floor {ratio_floor:.2}")),
            }
        }
        if !coverage_ok {
            match coverage {
                Some(c) => {
                    gate_reasons.push(format!("coverage {c:.2} < floor {cov_floor:.2}"));
                }
                None => gate_reasons.push(format!("coverage unknown < floor {cov_floor:.2}")),
            }
        }

        let status = if !books_ok {
            PairStatus::NoData
        } else if !window_met {
            PairStatus::Preparing
        } else if !signal_active {
            PairStatus::Tracking
        } else if exit_uncostable
            || !variation_ok
            || p_win.is_none()
            || funding_unknown
            || ladder.is_empty()
            || !session_conf_ok
            || !ratio_ok
            || !coverage_ok
        {
            PairStatus::Triggered
        } else {
            PairStatus::Ready
        };

        // Lifecycle timestamps: discover on transition into an active signal;
        // keep last_seen updated while it remains active.
        let (prev_status, prev_last_seen): (Option<PairStatus>, Option<i64>) = self
            .pairs
            .lock()
            .get(&key)
            .map(|r| (Some(r.status), r.last_seen_ms))
            .unwrap_or((None, None));
        let mut rt = self.pairs.lock().get(&key).cloned().unwrap_or(PairRuntime {
            status: PairStatus::Preparing,
            first_seen_ms: None,
            last_seen_ms: None,
            last_edge_bps: None,
            last_dev_bps: None,
        });
        let was_active = rt.status == PairStatus::Triggered || rt.status == PairStatus::Ready;
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

        // Audit draft via the shared idempotency rule (transitions +
        // minute-bucketed heartbeats only).
        let trigger = audit_trigger(
            prev_status,
            status,
            signal_active,
            window_met,
            prev_last_seen,
            now_ms,
        );
        let event = trigger.map(|t| {
            let economics_json = serde_json::json!({
                "friction_bps": friction_bps.is_finite().then_some(friction_bps),
                "exit_est_bps": exit_est_opt.filter(|e| e.is_finite()),
                "exit_uncostable": exit_uncostable,
                "execution_mode": cfg.execution_mode,
                "funding_carry_bps": carry.filter(|c| c.is_finite()),
                "fee_bps": fee_bps.is_finite().then_some(fee_bps),
            })
            .to_string();
            SignalEventDraft {
                symbol: symbol.to_string(),
                buy_venue: buy_v.to_string(),
                sell_venue: sell_v.to_string(),
                ts_ms: now_ms,
                old_status: prev_status.map(|s| s.as_str().to_string()),
                new_status: status.as_str().to_string(),
                edge_bps,
                dev_bps,
                p_win,
                economics_json,
                trigger: t,
            }
        });

        // --- directional trade win probability + signed expectation --------
        // `p_win` is direction-insensitive reversion P(|dev| reverts). The
        // table's Win % must be directional: wrong-direction or hard-blocked
        // rows read 0%, never 100%. Signed expectation never floors to 0.
        let win_probability = trade_win_probability(
            p_win,
            dev_bps,
            entry_thresh,
            books_ok,
            window_met,
            funding_unknown,
            exit_uncostable,
            optimal,
            variation_ok,
            ratio_ok,
            coverage_ok,
            session_conf_ok,
            session_block.is_some(),
        );
        // Same directional gating applied to the estimate's Wilson lower
        // bound — executability requires the confident floor, not just a
        // lucky point estimate, to clear `min_win_lower_bound`.
        let p_win_lower = p_est.map(|e| e.lower);
        let win_probability_lower = trade_win_probability(
            p_win_lower,
            dev_bps,
            entry_thresh,
            books_ok,
            window_met,
            funding_unknown,
            exit_uncostable,
            optimal,
            variation_ok,
            ratio_ok,
            coverage_ok,
            session_conf_ok,
            session_block.is_some(),
        );
        let win_probability_upper = trade_win_probability(
            p_est.map(|e| e.upper),
            dev_bps,
            entry_thresh,
            books_ok,
            window_met,
            funding_unknown,
            exit_uncostable,
            optimal,
            variation_ok,
            ratio_ok,
            coverage_ok,
            session_conf_ok,
            session_block.is_some(),
        );
        // --- executability ------------------------------------------------
        // Gate on the directional trade win probability (falls back to
        // reversion only when the directional value is uncomputable).
        let gate_p = win_probability.or(p_win);
        let gate_p_lower = win_probability_lower.or(p_win_lower);
        let (executable, mut block_reason) = execution_gate(
            status,
            books_ok,
            session_block,
            signal_active,
            gate_p,
            cfg.min_win_probability,
            gate_p_lower,
            cfg.min_win_lower_bound,
            funding_unknown,
            exit_uncostable,
            optimal,
            state,
            symbol,
        );
        if !gate_reasons.is_empty()
            && status == PairStatus::Triggered
            && books_ok
            && window_met
            && signal_active
        {
            block_reason = Some(gate_reasons.join("; "));
        }

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
            reversion_probability: p_win,
            win_probability,
            win_probability_lower,
            win_probability_upper,
            p_win_n_effective: p_est.map(|e| e.n_effective),
            p_win_censored: p_est.map(|e| e.censored as u32),
            p_win_from_model: p_est.map(|e| e.from_model),
            net_win_probability: outcome_stats
                .as_ref()
                .and_then(|s| s.p_net_pos.and_then(|b| b.rate)),
            net_win_lower: outcome_stats
                .as_ref()
                .and_then(|s| s.p_net_pos.and_then(|b| b.lower)),
            net_expected_bps: outcome_stats.as_ref().and_then(|s| s.expected_net_bps),
            net_deciles_bps: outcome_stats.as_ref().and_then(|s| s.deciles_bps.clone()),
            outcome_trials: outcome_stats.as_ref().map(|s| s.trials as u32),
            outcome_censored: outcome_stats.as_ref().map(|s| s.censored as u32),
            median_hold_secs: outcome_stats.as_ref().and_then(|s| s.median_hold_secs),
            median_win_secs: outcome_stats.as_ref().and_then(|s| s.median_win_secs),
            expected_profit_bps: exp_bps
                .or_else(|| dev_bps.zip(p_win).map(|(d, p)| (d - friction_bps) * p)),
            expected_profit_usd: exp_usd,
            optimal_size_usd: optimal,
            breakeven_size_usd: breakeven,
            capacity_ladder: ladder,
            expected_capacity_usd: exp_capacity,
            bps_capacity_ladder: bps_ladder,
            depth_curve,
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
            threshold_ratio,
            coverage,
            session_id: sess.session.as_str().to_string(),
            session_provisional: sess.provisional,
            session_allow: sess.allow,
            session_size_mult,
        };
        Some((row, persist, event))
    }
}

fn validate_monitor_book(
    book: &OrderBook,
    expected_venue: &arbitrageur_core::types::VenueId,
    expected_symbol: &str,
) -> Result<(), String> {
    if &book.venue != expected_venue {
        return Err(format!(
            "order book venue '{}' does not match expected venue '{}'",
            book.venue, expected_venue
        ));
    }
    // Hub keys normalize spellings — identity holds on canonical base.
    if crate::state_store::canonical_base(&book.symbol)
        != crate::state_store::canonical_base(expected_symbol)
    {
        return Err(format!(
            "order book symbol '{}' does not match requested symbol '{}'",
            book.symbol, expected_symbol
        ));
    }
    book.validate_for_execution()
}

/// Whether the book snapshot yields a persistable top-of-book sample.
fn book_sample(book: &OrderBook, venue: &str) -> Option<VenueSampleEntry> {
    let bid = book.bids.first()?.price.to_string().parse::<f64>().ok()?;
    let ask = book.asks.first()?.price.to_string().parse::<f64>().ok()?;
    let mid = (bid + ask) / 2.0;
    let bid_depth: f64 = book
        .bids
        .iter()
        .take(5)
        .map(|l| l.size.to_string().parse::<f64>().unwrap_or(0.0))
        .sum();
    let ask_depth: f64 = book
        .asks
        .iter()
        .take(5)
        .map(|l| l.size.to_string().parse::<f64>().unwrap_or(0.0))
        .sum();
    (mid.is_finite() && mid > 0.0).then_some(VenueSampleEntry {
        venue_id: venue.to_string(),
        mid,
        best_bid: bid,
        best_ask: ask,
        bid_depth: (bid_depth > 0.0).then_some(bid_depth),
        ask_depth: (ask_depth > 0.0).then_some(ask_depth),
    })
}

/// User rule: within the qualification window, each half-life chunk must
/// show max |dev| >= friction_cover_mult * friction_bps — else the pair's
/// motion is noise relative to costs and cannot qualify.
fn variation_covers_friction(
    series: &[arbitrageur_math::pair_monitor::PairObservation],
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
    let Some(last) = series.last() else {
        return false;
    };
    let t_end = last.ts_ms;
    let chunks = ((window_secs / chunk_secs).ceil() as i64).clamp(1, 64);
    for k in 0..chunks {
        let c_end = t_end - k * (chunk_secs * 1000.0) as i64;
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

/// Whether the row may be executed right now. Live mode manual radar
/// entries dispatch through the spatial two-leg path, which enforces the
/// vault, adapter-eligibility, and credential checks downstream.
/// Autonomous bot entries stay testnet-only regardless: `evaluate_entry_gates`
/// hard-blocks live mode on its own layer.
#[allow(clippy::too_many_arguments)]
fn execution_gate(
    status: PairStatus,
    books_ok: bool,
    session_block: Option<String>,
    signal_active: bool,
    p_win: Option<f64>,
    p_min: f64,
    p_lower: Option<f64>,
    lower_min: f64,
    funding_unknown: bool,
    exit_uncostable: bool,
    optimal: Option<f64>,
    state: &AppEngineState,
    symbol: &str,
) -> (bool, Option<String>) {
    if *state.is_paused.read() {
        return (false, Some("engine paused".into()));
    }
    if !books_ok {
        return (false, Some("orderbook stale or missing".into()));
    }
    if let Some(reason) = session_block {
        return (false, Some(reason));
    }
    if status != PairStatus::Ready || !signal_active {
        return (false, Some("signal not ready".into()));
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
    // Confidence floor: the Wilson lower bound must still beat
    // `lower_min`. A shrunken estimate can clear `p_min` while its
    // evidence is too thin to trust — this is the honest-uncertainty gate.
    if lower_min > 0.0 {
        match p_lower {
            Some(lb) if lb >= lower_min => {}
            Some(lb) => {
                return (
                    false,
                    Some(format!(
                    "win-rate confidence bound {lb:.2} below {lower_min:.2} floor (thin evidence)"
                )),
                )
            }
            None => return (false, Some("win-rate confidence bound unknown".into())),
        }
    }
    if funding_unknown {
        return (false, Some("funding adjustment unknown".into()));
    }
    if exit_uncostable {
        return (
            false,
            Some("exit depth insufficient for two-way close".into()),
        );
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
    use arbitrageur_core::quote::{BookLevel, OrderBook};
    use arbitrageur_core::types::VenueId;
    use arbitrageur_math::pair_monitor::PairObservation;
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
    fn mean_dt_secs_degrades_to_zero() {
        assert_eq!(series_mean_dt_secs(&[]), 0.0);
        assert_eq!(series_mean_dt_secs(&[obs(1_000, 1.0)]), 0.0);
        assert_eq!(
            series_mean_dt_secs(&[obs(0, 1.0), obs(30_000, 1.0), obs(60_000, 1.0)]),
            30.0
        );
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
        ) -> anyhow::Result<Vec<arbitrageur_core::market::MarketMetadata>> {
            Ok(Vec::new())
        }
        async fn fetch_orderbook(&self, _s: &str) -> anyhow::Result<OrderBook> {
            Ok(book("test", _s, 99.0, 101.0, 1))
        }
        async fn fetch_ticker(&self, _s: &str) -> anyhow::Result<arbitrageur_core::quote::Ticker> {
            anyhow::bail!("unused in tests")
        }
        async fn fetch_funding_rate(
            &self,
            _s: &str,
        ) -> anyhow::Result<arbitrageur_core::quote::FundingRate> {
            anyhow::bail!("unused in tests")
        }
        async fn fetch_account(
            &self,
        ) -> anyhow::Result<arbitrageur_core::position::AccountSnapshot> {
            anyhow::bail!("unused in tests")
        }
        async fn place_order(
            &self,
            _o: arbitrageur_core::order::OrderRequest,
        ) -> anyhow::Result<arbitrageur_core::order::OrderResponse> {
            anyhow::bail!("unused in tests")
        }
        async fn cancel_order(
            &self,
            _r: arbitrageur_core::order::CancelRequest,
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

    #[tokio::test]
    async fn poll_due_books_terminates_with_ws_fresh_lane() {
        let state = AppEngineState::new();
        let engine = SpreadMonitorEngine::new(&state);
        {
            let mut lanes = engine.lanes.lock();
            lanes.push(VenueLane {
                venue: "test".into(),
                adapter: Arc::new(DummyAdapter),
                symbols: vec!["A".into(), "B".into()],
                requests: VecDeque::new(),
                budget_per_min: 60.0,
                usable_per_min: 45.0,
                account_tier: "standard".into(),
                cursor: 0,
                last_fetch: HashMap::new(),
                backoff_until: None,
                fail_streak: 0,
                next_funding_at: Instant::now(),
            });
        }
        state
            .market_data_hub
            .update_orderbook_from_ws(book("test", "A", 99.0, 101.0, 2));
        state
            .market_data_hub
            .update_orderbook_from_ws(book("test", "B", 99.0, 101.0, 2));
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            engine.poll_due_books(&state),
        )
        .await
        .expect("poll_due_books must terminate when every due symbol is WS-fresh");
        let lanes = engine.lanes.lock();
        let lane = lanes.iter().find(|l| l.venue == "test").expect("lane");
        assert!(lane.last_fetch.contains_key("A"));
        assert!(lane.last_fetch.contains_key("B"));
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
    fn funding_per_hour_skips_unparseable_and_scales_by_interval() {
        assert_eq!(funding_per_hour("0.0004", 8), Some(0.00005));
        assert_eq!(funding_per_hour("0.0004", 0), Some(0.0004));
        assert_eq!(funding_per_hour("NaN", 8), None);
        assert_eq!(funding_per_hour("not-a-number", 8), None);
        assert_eq!(funding_per_hour("inf", 8), None);
    }

    #[test]
    fn funding_leg_rate_fresh_spot_and_stale_cases() {
        let now = Instant::now();
        let mut ema: HashMap<(String, String), (f64, Instant)> = HashMap::new();
        ema.insert(("lighter".into(), "BTC".into()), (0.0001, now));
        // Fresh perp rate resolves by (venue, canonical base).
        assert_eq!(
            funding_leg_rate_hourly(&ema, "lighter", "BTC-USDC", now),
            Some(0.0001)
        );
        // Spot legs contribute 0 by design — never blocked, never lying.
        assert_eq!(
            funding_leg_rate_hourly(&ema, "lighter", "BTC-SPOT", now),
            Some(0.0)
        );
        // A perp leg on a venue with no sample → None (no decision).
        assert_eq!(
            funding_leg_rate_hourly(&ema, "hyperliquid", "BTC-USDC", now),
            None
        );
        // Stale samples (>5min) → None.
        ema.insert(
            ("kraken".into(), "BTC".into()),
            (0.0002, now - Duration::from_secs(600)),
        );
        assert_eq!(
            funding_leg_rate_hourly(&ema, "kraken", "BTC-USDC", now),
            None
        );
    }

    #[tokio::test]
    async fn supervise_funding_exits_flattens_decayed_carry() {
        use crate::state_store::{LegSide, NewSignalExecution, NewSignalLeg, SignalStatus};

        let state = AppEngineState::new();
        state.install_test_sim("lighter");
        state.install_test_sim("hyperliquid");
        for (venue, sym) in [("lighter", "BTC-USDC"), ("hyperliquid", "BTC-USDC")] {
            let mut book = book(venue, sym, 99_900.0, 100_100.0, 5);
            book.timestamp = Utc::now();
            if let Some(sim) = state
                .test_sims
                .read()
                .ok()
                .and_then(|m| m.get(venue).cloned())
            {
                let _ = sim.replace_orderbook(book);
            }
        }
        // A balanced funding carry: long perp on lighter, short perp on
        // hyperliquid. EMA carry has decayed to ~0 — below the 8% floor.
        let sig_id = "sig-funding-supervise-1";
        state
            .state_store
            .record_signal_execution(&NewSignalExecution {
                id: sig_id.into(),
                strategy_id: "funding".into(),
                symbol: "BTC-USDC".into(),
                route_id: None,
                expected_net_usd: Some(1.0),
                note: None,
                legs: vec![
                    NewSignalLeg {
                        account_id: None,
                        venue: "lighter".into(),
                        side: LegSide::Long,
                        symbol: Some("BTC-USDC".into()),
                        intended_qty: 0.01,
                        intended_price: Some(100_000.0),
                    },
                    NewSignalLeg {
                        account_id: None,
                        venue: "hyperliquid".into(),
                        side: LegSide::Short,
                        symbol: Some("BTC-USDC".into()),
                        intended_qty: 0.01,
                        intended_price: Some(100_000.0),
                    },
                ],
            })
            .expect("record funding signal");
        for side in [LegSide::Long, LegSide::Short] {
            state
                .state_store
                .apply_leg_fill(sig_id, side, 0.01, 100_000.0, 0.0, None)
                .expect("leg fill");
        }
        state
            .state_store
            .update_signal_status(sig_id, SignalStatus::Balanced, None)
            .expect("mark balanced");

        let engine = SpreadMonitorEngine::new(&state);
        {
            let mut ema = engine.funding_ema.lock();
            // Short earns +0.01%/h, long pays +0.01%/h → net ~0% APR.
            ema.insert(("lighter".into(), "BTC".into()), (0.0001, Instant::now()));
            ema.insert(
                ("hyperliquid".into(), "BTC".into()),
                (0.0001, Instant::now()),
            );
        }
        engine.supervise_funding_exits(&state).await;

        let sigs = state
            .state_store
            .list_signals(Some("BTC-USDC"), None)
            .expect("list");
        let sig = sigs.iter().find(|s| s.id == sig_id).expect("signal");
        assert_eq!(
            sig.status,
            SignalStatus::Closed,
            "decayed carry must flatten through the supervisor (got {:?})",
            sig.status
        );
    }

    #[tokio::test]
    async fn supervise_funding_exits_holds_fresh_carry() {
        use crate::state_store::{LegSide, NewSignalExecution, NewSignalLeg, SignalStatus};

        let state = AppEngineState::new();
        let sig_id = "sig-funding-supervise-hold";
        state
            .state_store
            .record_signal_execution(&NewSignalExecution {
                id: sig_id.into(),
                strategy_id: "funding".into(),
                symbol: "BTC-USDC".into(),
                route_id: None,
                expected_net_usd: Some(1.0),
                note: None,
                legs: vec![
                    NewSignalLeg {
                        account_id: None,
                        venue: "lighter".into(),
                        side: LegSide::Long,
                        symbol: Some("BTC-USDC".into()),
                        intended_qty: 0.01,
                        intended_price: Some(100_000.0),
                    },
                    NewSignalLeg {
                        account_id: None,
                        venue: "hyperliquid".into(),
                        side: LegSide::Short,
                        symbol: Some("BTC-USDC".into()),
                        intended_qty: 0.01,
                        intended_price: Some(100_000.0),
                    },
                ],
            })
            .expect("record funding signal");
        for side in [LegSide::Long, LegSide::Short] {
            state
                .state_store
                .apply_leg_fill(sig_id, side, 0.01, 100_000.0, 0.0, None)
                .expect("leg fill");
        }
        state
            .state_store
            .update_signal_status(sig_id, SignalStatus::Balanced, None)
            .expect("mark balanced");

        let engine = SpreadMonitorEngine::new(&state);
        {
            let mut ema = engine.funding_ema.lock();
            // Short earns +0.05%/h while long pays 0 → ~438% APR, far above
            // the floor — the position must be held.
            ema.insert(("lighter".into(), "BTC".into()), (0.0, Instant::now()));
            ema.insert(
                ("hyperliquid".into(), "BTC".into()),
                (0.0005, Instant::now()),
            );
        }
        engine.supervise_funding_exits(&state).await;

        let sigs = state
            .state_store
            .list_signals(Some("BTC-USDC"), None)
            .expect("list");
        let sig = sigs.iter().find(|s| s.id == sig_id).expect("signal");
        assert_eq!(
            sig.status,
            SignalStatus::Balanced,
            "carry above the floor must be held (got {:?})",
            sig.status
        );
    }

    #[test]
    fn execution_gate_blocks_not_ready() {
        let state = AppEngineState::new();
        let (ok, why) = execution_gate(
            PairStatus::Triggered,
            true,
            None,
            true,
            Some(0.95),
            0.9,
            None,
            0.0,
            false,
            false,
            Some(1000.0),
            &state,
            "BTC",
        );
        assert!(!ok);
        assert!(why.unwrap().contains("not ready"));

        let (ok, why) = execution_gate(
            PairStatus::Ready,
            true,
            None,
            true,
            Some(0.5),
            0.9,
            None,
            0.0,
            false,
            false,
            Some(1000.0),
            &state,
            "BTC",
        );
        assert!(!ok);
        assert!(why.unwrap().contains("below"));

        let (ok, _) = execution_gate(
            PairStatus::Ready,
            true,
            None,
            true,
            Some(0.95),
            0.9,
            None,
            0.0,
            true,
            false,
            Some(1000.0),
            &state,
            "BTC",
        );
        assert!(!ok);

        let (ok, why) = execution_gate(
            PairStatus::Ready,
            true,
            None,
            true,
            Some(0.95),
            0.9,
            None,
            0.0,
            false,
            false,
            Some(1000.0),
            &state,
            "BTC",
        );
        assert!(ok, "gate should open: {why:?}");
    }

    #[test]
    fn execution_gate_blocks_uncostable_two_way_exit() {
        let state = AppEngineState::new();
        // A fully ready row with an uncostable two-way exit is blocked
        // on the exit reason (checked before capacity/ticker gates).
        let (ok, why) = execution_gate(
            PairStatus::Ready,
            true,
            None,
            true,
            Some(0.95),
            0.9,
            None,
            0.0,
            false,
            true,
            Some(1000.0),
            &state,
            "BTC",
        );
        assert!(!ok);
        assert_eq!(why.unwrap(), "exit depth insufficient for two-way close");
        // Same row with a priced exit opens.
        let (ok, _) = execution_gate(
            PairStatus::Ready,
            true,
            None,
            true,
            Some(0.95),
            0.9,
            None,
            0.0,
            false,
            false,
            Some(1000.0),
            &state,
            "BTC",
        );
        assert!(ok);
    }

    #[test]
    fn execution_gate_session_block_precedes_signal_checks() {
        let state = AppEngineState::new();
        let (ok, why) = execution_gate(
            PairStatus::Ready,
            true,
            Some("funding window stand-down".into()),
            true,
            Some(0.95),
            0.9,
            None,
            0.0,
            false,
            false,
            Some(1000.0),
            &state,
            "BTC",
        );
        assert!(!ok);
        assert_eq!(why.unwrap(), "funding window stand-down");
    }

    #[test]
    fn session_standdown_precedence() {
        assert_eq!(
            session_standdown(true, true, Some(999.0), Some(1.0), true),
            Some("funding window stand-down".into())
        );
        assert_eq!(
            session_standdown(false, true, Some(999.0), Some(1.0), true),
            Some("ondo equity market closed".into())
        );
        assert_eq!(
            session_standdown(false, false, Some(9.0), Some(2.0), true),
            Some("spread exceeds 3x session median".into())
        );
        assert_eq!(
            session_standdown(false, false, Some(5.9), Some(2.0), true),
            Some("fast-vol above 3x session norm (halted)".into())
        );
        assert_eq!(
            session_standdown(false, false, Some(5.9), Some(2.0), false),
            None
        );
        assert_eq!(
            session_standdown(false, false, None, Some(2.0), false),
            None
        );
        assert_eq!(
            session_standdown(false, false, Some(5.0), None, false),
            None
        );
    }

    #[test]
    fn vol_halt_halves_then_halts() {
        assert_eq!(vol_halt_next(0, false), (0, 1.0, false));
        assert_eq!(vol_halt_next(7, false), (0, 1.0, false));
        assert_eq!(vol_halt_next(0, true), (1, 0.5, false));
        assert_eq!(vol_halt_next(1, true), (2, 0.5, true));
        assert_eq!(vol_halt_next(2, true), (3, 0.5, true));
    }

    #[test]
    fn session_context_neutral_without_profiles() {
        use chrono::TimeZone;
        let state = AppEngineState::new();
        let ts = |y: i32, mo: u32, d: u32, h: u32, mi: u32| {
            chrono::Utc
                .with_ymd_and_hms(y, mo, d, h, mi, 0)
                .single()
                .unwrap()
                .timestamp_millis()
        };
        // Overlap on a weekday, clear of 8h stamps (8h venues only).
        let ctx = session_context(&state, "BTC", "lighter", "vanta", ts(2026, 1, 15, 14, 0));
        assert_eq!(ctx.session, SessionId::Overlap);
        assert!(!ctx.funding_mask);
        assert!(ctx.provisional);
        assert_eq!(ctx.entry_delta_bps, 0.0);
        assert_eq!(ctx.allow, 1.0);
        assert_eq!(ctx.session_med, None);
        // 08:00 stamp blocks 8h venues.
        let ctx = session_context(&state, "BTC", "lighter", "vanta", ts(2026, 1, 15, 8, 0));
        assert!(ctx.funding_mask);
        assert_eq!(ctx.allow, 0.0);
        // Weekend halves the overlap allowance.
        let ctx = session_context(&state, "BTC", "lighter", "vanta", ts(2026, 1, 17, 14, 0));
        assert!(is_weekend(ts(2026, 1, 17, 14, 0)));
        assert_eq!(ctx.allow, 0.5);
        assert!(ctx.provisional);
        // Hourly-funding venue masks every hour.
        let ctx = session_context(
            &state,
            "BTC",
            "hyperliquid",
            "lighter",
            ts(2026, 1, 15, 14, 1),
        );
        assert!(ctx.funding_mask);
    }

    #[test]
    fn session_context_uses_calibrated_profiles() {
        use chrono::TimeZone;
        let state = AppEngineState::new();
        let base = chrono::Utc
            .with_ymd_and_hms(2026, 1, 15, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        let peak_for_hour = |h: i64| match h % 24 {
            0..=7 => 4.0,
            8..=12 => 12.0,
            13..=16 => 22.0,
            17..=21 => 14.0,
            _ => 3.0,
        };
        for i in 0..(16 * 24) {
            let ts = base + i * 3_600_000 + 60_000;
            let spread_bps = if i % 2 == 0 { peak_for_hour(i) } else { 7.0 };
            state
                .state_store
                .record_spread_samples(
                    "BTC",
                    ts,
                    &[
                        crate::state_store::VenueSampleEntry {
                            venue_id: "lighter".into(),
                            mid: 100.0,
                            best_bid: 99.9,
                            best_ask: 100.1,
                            bid_depth: None,
                            ask_depth: None,
                        },
                        crate::state_store::VenueSampleEntry {
                            venue_id: "vanta".into(),
                            mid: 100.0 * (1.0 + spread_bps / 10_000.0),
                            best_bid: 99.9,
                            best_ask: 100.1,
                            bid_depth: None,
                            ask_depth: None,
                        },
                    ],
                )
                .unwrap();
        }
        let to = base + 16 * 24 * 3_600_000;
        state
            .state_store
            .rollup_session_profiles("BTC", base - 1_000, to + 1_000, to)
            .unwrap();
        let overlap_ts = chrono::Utc
            .with_ymd_and_hms(2026, 1, 20, 14, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        let ctx = session_context(&state, "BTC", "lighter", "vanta", overlap_ts);
        assert_eq!(ctx.session, SessionId::Overlap);
        assert!(!ctx.provisional);
        assert!(ctx.session_med.unwrap() > 0.0);
        assert!(ctx.win_rate.is_some());
        assert_eq!(ctx.allow, 1.0);
        assert_ne!(ctx.entry_delta_bps, 0.0);
        let dead_ts = chrono::Utc
            .with_ymd_and_hms(2026, 1, 20, 23, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        let ctx = session_context(&state, "BTC", "lighter", "vanta", dead_ts);
        assert_eq!(ctx.session, SessionId::Dead);
        assert!(ctx.provisional);
        assert_eq!(ctx.entry_delta_bps, 0.0);
        assert_eq!(ctx.session_med, None);
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
            let ts = now - (7_200_000 - i * 100_000);
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
                            bid_depth: None,
                            ask_depth: None,
                        },
                        VenueSampleEntry {
                            venue_id: "lighter".into(),
                            mid: 100.0 + wiggle / 100.0,
                            best_bid: 99.9,
                            best_ask: 100.1,
                            bid_depth: None,
                            ask_depth: None,
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
        // 2h of data > 30min floor -> not preparing.
        assert_ne!(row.status, "preparing");

        // Now drop the books: fresh hub -> no-data rows.
        state.market_data_hub.clear();
        engine.evaluate(&state).await;
        let snap2 = engine.snapshot().unwrap();
        assert!(snap2.rows.iter().all(|r| r.status == "no-data"));
        assert!(snap2.rows.iter().all(|r| !r.executable));
    }

    fn win_series(spreads: &[f64], step_ms: i64) -> Vec<PairObservation> {
        spreads
            .iter()
            .enumerate()
            .map(|(i, s)| PairObservation {
                ts_ms: i as i64 * step_ms,
                spread_bps: *s,
            })
            .collect()
    }

    #[test]
    fn pair_win_probability_empirical_hit_rate() {
        // Six clean win episodes: six flats then a +12bps spike, band 5.0.
        // The spike pulls the mean to ~1.7, so flats sit below the band
        // while every spike excursion reverts -> P_win exactly 1.0.
        let mut spreads = Vec::new();
        for _ in 0..6 {
            spreads.extend_from_slice(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 12.0]);
        }
        // Trailing flats let the final episode resolve instead of ending open.
        spreads.extend_from_slice(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let series = win_series(&spreads, 1_000);
        let p = pair_win_probability(&series, 12.0, 5.0, 5);
        // Beta(1,1)-shrunken: 6 wins / 6 episodes -> (6+1)/(6+2) = 0.875.
        // The shrunken point estimate is intentional — raw hit rates
        // overclaim confidence on thin samples.
        assert_eq!(p, Some(0.875));
    }

    #[test]
    fn pair_win_probability_thin_history_is_none() {
        let empty: Vec<PairObservation> = Vec::new();
        assert_eq!(pair_win_probability(&empty, 20.0, 2.0, 5), None);
        let one = win_series(&[5.0], 1_000);
        assert_eq!(pair_win_probability(&one, 5.0, 2.0, 5), None);
        // Three points: one episode at most, below min_episodes, and too
        // few points for the OU fallback (needs >= 10).
        let few = win_series(&[0.0, 20.0, 0.0], 1_000);
        assert_eq!(pair_win_probability(&few, 20.0, 2.0, 5), None);
    }

    #[test]
    fn pair_win_probability_ou_fallback_when_episodes_thin() {
        // Smooth sine around mu=10, amplitude far below the 8.5 band: zero
        // episodes, but a near-perfect AR(1) fit -> fallback yields Some.
        let spreads: Vec<f64> = (0..60)
            .map(|k| 10.0 + 1.5 * ((k as f64) * 0.25).sin())
            .collect();
        let series = win_series(&spreads, 1_000);
        let p = pair_win_probability(&series, 18.0, 8.5, 5).expect("OU fallback must qualify");
        assert!(p > 0.5 && p <= 1.0, "fallback P_win in range, got {p}");
    }

    #[test]
    fn pair_win_probability_beyond_evidence_is_none() {
        // Ready episodes exist, but the current deviation exceeds every
        // observed peak -> no bucket, fail closed.
        let mut spreads = Vec::new();
        for _ in 0..6 {
            spreads.extend_from_slice(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 12.0]);
        }
        // Trailing flats let the final episode resolve instead of ending open.
        spreads.extend_from_slice(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let series = win_series(&spreads, 1_000);
        assert_eq!(pair_win_probability(&series, 100.0, 5.0, 5), None);
    }

    #[test]
    fn pair_win_probability_nan_guards() {
        let mut spreads = Vec::new();
        for _ in 0..6 {
            spreads.extend_from_slice(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 12.0]);
        }
        // Trailing flats let the final episode resolve instead of ending open.
        spreads.extend_from_slice(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let series = win_series(&spreads, 1_000);
        assert_eq!(pair_win_probability(&series, f64::NAN, 5.0, 5), None);
        assert_eq!(pair_win_probability(&series, 12.0, f64::NAN, 5), None);
        assert_eq!(pair_win_probability(&series, f64::INFINITY, 5.0, 5), None);
    }

    #[test]
    fn trade_win_probability_wrong_direction_is_zero_not_reversion() {
        // High reversion (1.0) but negative dev for this ordered direction:
        // trade win must be 0.0, never 100%.
        let w = trade_win_probability(
            Some(1.0),
            Some(-16.6),
            4.0,
            true,
            true,
            false,
            false,
            Some(1000.0),
            true,
            true,
            true,
            true,
            false,
        );
        assert_eq!(w, Some(0.0));
    }

    #[test]
    fn trade_win_probability_ready_keeps_reversion_blocked_goes_zero() {
        // Ready path: all gates pass -> keeps reversion.
        let ready = trade_win_probability(
            Some(0.98),
            Some(5.0),
            4.0,
            true,
            true,
            false,
            false,
            Some(1000.0),
            true,
            true,
            true,
            true,
            false,
        );
        assert_eq!(ready, Some(0.98));
        // Same edge but funding unknown -> hard-blocked to 0.
        let blocked = trade_win_probability(
            Some(0.98),
            Some(5.0),
            4.0,
            true,
            true,
            true,
            false,
            Some(1000.0),
            true,
            true,
            true,
            true,
            false,
        );
        assert_eq!(blocked, Some(0.0));
        // No profitable capacity -> 0.
        let nocap = trade_win_probability(
            Some(0.98),
            Some(5.0),
            4.0,
            true,
            true,
            false,
            false,
            None,
            true,
            true,
            true,
            true,
            false,
        );
        assert_eq!(nocap, Some(0.0));
        // Unmet window -> unknown, not zero.
        let prep = trade_win_probability(
            Some(0.98),
            Some(5.0),
            4.0,
            true,
            false,
            false,
            false,
            Some(1000.0),
            true,
            true,
            true,
            true,
            false,
        );
        assert_eq!(prep, None);
    }

    #[test]
    fn pair_win_probability_deterministic() {
        let mut spreads = Vec::new();
        for _ in 0..6 {
            spreads.extend_from_slice(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 12.0]);
        }
        // Trailing flats let the final episode resolve instead of ending open.
        spreads.extend_from_slice(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let series = win_series(&spreads, 1_000);
        let a = pair_win_probability(&series, 8.0, 5.0, 5);
        let b = pair_win_probability(&series, 8.0, 5.0, 5);
        assert_eq!(a, b);
        assert!(a.is_some());
    }

    fn levels_book(
        venue: &str,
        symbol: &str,
        bids: &[(f64, f64)],
        asks: &[(f64, f64)],
    ) -> OrderBook {
        let mut b = OrderBook::new(VenueId::new(venue), symbol.to_string());
        for (p, s) in bids {
            b.bids.push(BookLevel::new(
                rust_decimal::Decimal::from_f64_retain(*p).unwrap(),
                rust_decimal::Decimal::from_f64_retain(*s).unwrap(),
            ));
        }
        for (p, s) in asks {
            b.asks.push(BookLevel::new(
                rust_decimal::Decimal::from_f64_retain(*p).unwrap(),
                rust_decimal::Decimal::from_f64_retain(*s).unwrap(),
            ));
        }
        b.timestamp = Utc::now();
        b
    }

    #[test]
    fn two_way_exit_walk_matches_hand_computed_values() {
        // Buy book: best bid 100 (1) / 99 (3), best ask 101.
        // Sell book: best bid 120, best ask 121 (1) / 122 (3).
        let buy = levels_book(
            "a",
            "BTC",
            &[(100.0, 1.0), (99.0, 3.0)],
            &[(101.0, 5.0), (102.0, 5.0)],
        );
        let sell = levels_book(
            "b",
            "BTC",
            &[(120.0, 5.0), (119.0, 5.0)],
            &[(121.0, 1.0), (122.0, 3.0)],
        );
        let qty = rust_decimal::Decimal::from_f64_retain(2.0).unwrap();
        let got = two_way_exit_estimate(&buy, &sell, qty, 4.0).expect("deep books price");
        // Buy exit sells 2 into bids: (1*100 + 1*99)/2 = 99.5 -> 50 bps.
        // Sell exit buys 2 from asks: (1*121 + 1*122)/2 = 121.5 -> 0.5/121.
        // Closing: buy (101-100)/100.5 + sell (121-120)/120.5, all x1e4.
        let expected = 4.0
            + 50.0
            + (0.5 / 121.0 * 10_000.0)
            + (1.0 / 100.5 * 10_000.0)
            + (1.0 / 120.5 * 10_000.0);
        assert!(
            (got - expected).abs() < 1e-6,
            "exit walk got {got}, want {expected}"
        );
    }

    #[test]
    fn two_way_exit_thin_reverse_side_fails_closed() {
        let qty = rust_decimal::Decimal::from_f64_retain(2.0).unwrap();
        // Thin buy bids: unwind needs 2, only 0.5 rests.
        let buy = levels_book("a", "BTC", &[(100.0, 0.5)], &[(101.0, 100.0)]);
        let sell = levels_book("b", "BTC", &[(120.0, 100.0)], &[(121.0, 100.0)]);
        assert!(two_way_exit_estimate(&buy, &sell, qty, 4.0).is_none());
        // Thin sell asks, mirror image.
        let buy2 = levels_book("a", "BTC", &[(100.0, 100.0)], &[(101.0, 100.0)]);
        let sell2 = levels_book("b", "BTC", &[(120.0, 100.0)], &[(121.0, 0.5)]);
        assert!(two_way_exit_estimate(&buy2, &sell2, qty, 4.0).is_none());
        // Exact-depth boundary still fills (remainder zero).
        let exact = rust_decimal::Decimal::from_f64_retain(0.5).unwrap();
        assert!(two_way_exit_estimate(&buy, &sell, exact, 4.0).is_some());
        // Both deep prices.
        assert!(two_way_exit_estimate(&buy2, &sell, qty, 4.0).is_some());
    }

    #[test]
    fn two_way_exit_rejects_degenerate_books_and_inputs() {
        let good_buy = levels_book("a", "BTC", &[(100.0, 10.0)], &[(101.0, 10.0)]);
        let good_sell = levels_book("b", "BTC", &[(120.0, 10.0)], &[(121.0, 10.0)]);
        let qty = rust_decimal::Decimal::from_f64_retain(1.0).unwrap();
        let empty = OrderBook::new(VenueId::new("x"), "BTC".into());
        assert!(two_way_exit_estimate(&empty, &good_sell, qty, 4.0).is_none());
        assert!(two_way_exit_estimate(&good_buy, &empty, qty, 4.0).is_none());
        // Crossed/locked books fail closed (never a negative spread credit).
        let crossed = levels_book("a", "BTC", &[(102.0, 10.0)], &[(101.0, 10.0)]);
        assert!(two_way_exit_estimate(&crossed, &good_sell, qty, 4.0).is_none());
        let crossed_s = levels_book("b", "BTC", &[(120.0, 10.0)], &[(119.0, 10.0)]);
        assert!(two_way_exit_estimate(&good_buy, &crossed_s, qty, 4.0).is_none());
        // Zero qty and non-finite/negative fees.
        assert!(two_way_exit_estimate(&good_buy, &good_sell, dec!(0), 4.0).is_none());
        assert!(two_way_exit_estimate(&good_buy, &good_sell, qty, f64::NAN).is_none());
        assert!(two_way_exit_estimate(&good_buy, &good_sell, qty, -1.0).is_none());
        // Zero-fee schedule is legitimate and prices.
        assert!(two_way_exit_estimate(&good_buy, &good_sell, qty, 0.0).is_some());
    }

    #[test]
    fn monitor_exit_qty_uses_smallest_tier() {
        let ask = rust_decimal::Decimal::from_f64_retain(100.0).unwrap();
        let q = monitor_exit_qty(&[10_000.0, 1_000.0, 5_000.0], Some(ask)).unwrap();
        assert_eq!(q, rust_decimal::Decimal::from_f64_retain(10.0).unwrap());
        assert!(monitor_exit_qty(&[], Some(ask)).is_none());
        assert!(monitor_exit_qty(&[1_000.0], None).is_none());
        assert!(monitor_exit_qty(&[1_000.0], Some(dec!(0))).is_none());
        assert!(monitor_exit_qty(&[f64::NAN, f64::INFINITY, -5.0], Some(ask)).is_none());
    }

    #[test]
    fn exit_mode_composition_one_way_untouched_two_way_depth_gated() {
        let thin_buy = levels_book("a", "BTC", &[(100.0, 0.1)], &[(101.0, 100.0)]);
        let thin_sell = levels_book("b", "BTC", &[(120.0, 100.0)], &[(121.0, 0.1)]);
        let bb = Some(thin_buy);
        let sb = Some(thin_sell);
        // One-way never prices a close, however thin the reverse sides are.
        assert_eq!(
            exit_estimate_for_mode("one_way", &bb, &sb, &[1_000.0], 4.0),
            Some(0.0)
        );
        assert_eq!(
            exit_estimate_for_mode("other", &bb, &sb, &[1_000.0], 4.0),
            Some(0.0)
        );
        // Two-way on the same thin books is uncostable...
        assert!(exit_estimate_for_mode("two_way", &bb, &sb, &[1_000.0], 4.0).is_none());
        // ...as are missing books — while one-way still reads 0.0.
        let none: Option<OrderBook> = None;
        assert!(exit_estimate_for_mode("two_way", &none, &sb, &[1_000.0], 4.0).is_none());
        assert_eq!(
            exit_estimate_for_mode("one_way", &none, &sb, &[1_000.0], 4.0),
            Some(0.0)
        );
        // Deep books price in two-way mode.
        let deep_buy = levels_book("a", "BTC", &[(100.0, 100.0)], &[(101.0, 100.0)]);
        let deep_sell = levels_book("b", "BTC", &[(120.0, 100.0)], &[(121.0, 100.0)]);
        let est = exit_estimate_for_mode(
            "two_way",
            &Some(deep_buy),
            &Some(deep_sell),
            &[1_000.0],
            4.0,
        )
        .expect("deep books price");
        assert!(est > 4.0, "walk adds slippage + spread over fees: {est}");
    }

    #[test]
    fn audit_trigger_transitions_heartbeat_idempotency() {
        // First sighting always logs.
        assert_eq!(
            audit_trigger(None, PairStatus::Preparing, false, false, None, 60_001),
            Some("transition")
        );
        // Status change logs with the previous status attached.
        assert_eq!(
            audit_trigger(
                Some(PairStatus::Tracking),
                PairStatus::Triggered,
                true,
                true,
                Some(60_000),
                120_000
            ),
            Some("transition")
        );
        // Same status, active, new minute bucket -> one heartbeat.
        assert_eq!(
            audit_trigger(
                Some(PairStatus::Triggered),
                PairStatus::Triggered,
                true,
                true,
                Some(60_000),
                120_000
            ),
            Some("heartbeat")
        );
        // Same status, active, same minute -> silent (idempotent re-poll).
        assert_eq!(
            audit_trigger(
                Some(PairStatus::Triggered),
                PairStatus::Triggered,
                true,
                true,
                Some(120_001),
                120_500
            ),
            None
        );
        // Inactive steady state never heartbeats.
        assert_eq!(
            audit_trigger(
                Some(PairStatus::Tracking),
                PairStatus::Tracking,
                false,
                true,
                Some(60_000),
                120_000
            ),
            None
        );
        assert_eq!(
            audit_trigger(
                Some(PairStatus::Preparing),
                PairStatus::Preparing,
                false,
                false,
                None,
                120_000
            ),
            None
        );
        // Active but qualification window unmet -> silent.
        assert_eq!(
            audit_trigger(
                Some(PairStatus::Triggered),
                PairStatus::Triggered,
                true,
                false,
                Some(60_000),
                120_000
            ),
            None
        );
    }

    #[tokio::test]
    async fn evaluate_appends_audit_events_without_duplicates() {
        let state = AppEngineState::new();
        let engine = SpreadMonitorEngine::new(&state);
        *engine.watchlist.lock() = vec!["BTC".into()];
        engine
            .venue_symbols
            .lock()
            .insert("hyperliquid".into(), vec!["BTC".into()]);
        engine
            .venue_symbols
            .lock()
            .insert("lighter".into(), vec!["BTC".into()]);
        // No books and no samples: every pair reports no-data (inactive) — the
        // first pass logs one first-sighting transition per ordered pair and
        // re-polls log nothing.
        engine.evaluate(&state).await;
        let first = state.state_store.monitor_signal_event_count().unwrap();
        assert_eq!(first, 2, "one first-sighting event per ordered pair");
        engine.evaluate(&state).await;
        let second = state.state_store.monitor_signal_event_count().unwrap();
        assert_eq!(second, 2, "same-minute re-poll must not duplicate events");
        let evs = state
            .state_store
            .recent_monitor_signal_events(None, None, None, 10)
            .unwrap();
        assert_eq!(evs.len(), 2);
        for ev in &evs {
            assert_eq!(ev.old_status, None);
            assert_eq!(ev.new_status, "no-data");
            assert_eq!(ev.trigger, "transition");
            let econ: serde_json::Value =
                serde_json::from_str(ev.economics_json.as_ref().unwrap()).unwrap();
            assert_eq!(econ["execution_mode"], serde_json::json!("one_way"));
        }
    }

    fn gate_cfg(ratio_floor: f64, cov_floor: f64) -> MonitorConfigDto {
        MonitorConfigDto {
            qualification_mode: "manual".into(),
            manual_window_secs: 1500,
            half_life_multiplier: 2.0,
            friction_cover_mult: 0.1,
            min_episodes: 1,
            min_threshold_ratio: ratio_floor,
            min_coverage: cov_floor,
            execution_mode: "one_way".into(),
            ..Default::default()
        }
    }

    fn gate_now_ms() -> i64 {
        use chrono::TimeZone;
        chrono::Utc
            .with_ymd_and_hms(2026, 1, 15, 14, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis()
    }

    fn gate_eval(
        engine: &SpreadMonitorEngine,
        state: &AppEngineState,
        cfg: &MonitorConfigDto,
        spreads: &[(i64, f64)],
        now_ms: i64,
    ) -> MonitorRowDto {
        let mut mids_a: Vec<(i64, f64)> = Vec::new();
        let mut mids_b: Vec<(i64, f64)> = Vec::new();
        for (ts, sp) in spreads {
            mids_a.push((*ts, 100.0));
            mids_b.push((*ts, 100.0 * (1.0 + sp / 10_000.0)));
        }
        let mut mids: HashMap<String, Vec<(i64, f64)>> = HashMap::new();
        mids.insert("a".into(), mids_a);
        mids.insert("b".into(), mids_b);
        engine
            .funding_ema
            .lock()
            .insert(("a".into(), "BTC".into()), (0.0, Instant::now()));
        engine
            .funding_ema
            .lock()
            .insert(("b".into(), "BTC".into()), (0.0, Instant::now()));
        state
            .market_data_hub
            .update_orderbook(book("a", "BTC", 99.8, 100.0, 5));
        state
            .market_data_hub
            .update_orderbook(book("b", "BTC", 100.12, 100.3, 5));
        let mut fees: HashMap<String, f64> = HashMap::new();
        fees.insert("a".into(), 1.0);
        fees.insert("b".into(), 1.0);
        let volumes: HashMap<String, HashMap<String, f64>> = HashMap::new();
        let depths: HashMap<String, (f64, f64)> = HashMap::new();
        let (row, _, _) = engine
            .evaluate_pair(
                state,
                cfg,
                "BTC",
                "a",
                "b",
                now_ms,
                Duration::from_secs(300),
                &mids,
                &volumes,
                &fees,
                &depths,
            )
            .expect("pair evaluates");
        row
    }

    fn window_spreads(
        now_ms: i64,
        n: usize,
        step_ms: i64,
        f: impl Fn(usize) -> f64,
    ) -> Vec<(i64, f64)> {
        (0..n)
            .map(|i| (now_ms - ((n - 1 - i) as i64) * step_ms, f(i)))
            .collect()
    }

    #[test]
    fn persistence_gate_persistent_qualifies_flicker_blocked() {
        let state = AppEngineState::new();
        let now = gate_now_ms();
        // Full 60-bucket window (1800s/30s), same 12bps peak.
        // Persistent holds 48/60 above; flicker holds 3/60 above with one
        // spike per 900s chunk so variation still passes for both.
        let persistent = window_spreads(now, 60, 30_000, |i| if i % 5 == 4 { 0.0 } else { 12.0 });
        let flicker = window_spreads(now, 60, 30_000, |i| {
            if i == 10 || i == 35 || i == 55 {
                12.0
            } else {
                0.0
            }
        });
        let cfg = gate_cfg(0.2, 0.0);
        let engine_p = SpreadMonitorEngine::new(&state);
        let row_p = gate_eval(&engine_p, &state, &cfg, &persistent, now);
        assert_eq!(
            row_p.status, "ready",
            "persistent must qualify: {:?}",
            row_p.block_reason
        );
        assert!(row_p.threshold_ratio.unwrap() >= 0.2);
        let engine_f = SpreadMonitorEngine::new(&state);
        let row_f = gate_eval(&engine_f, &state, &cfg, &flicker, now);
        assert_eq!(row_f.status, "triggered", "flicker must stay triggered");
        assert!(row_f.threshold_ratio.unwrap() < 0.2);
        assert!(
            row_f
                .block_reason
                .as_deref()
                .unwrap_or("")
                .contains("persistence"),
            "explicit persistence reason, got {:?}",
            row_f.block_reason
        );
    }

    #[test]
    fn ticker_execution_mode_overrides_global_exit_costing() {
        // Global one-way prices no exit, so the qualifying window reads as
        // ready. A persisted DOUBLE_WAY ticker config for the symbol must
        // win: the reverse depth walk then has to price — on these thin
        // 5-unit books it cannot, so the row must demote ready → triggered
        // (exit_uncostable), never silently executable.
        let state = AppEngineState::new();
        let now = gate_now_ms();
        let spreads = window_spreads(now, 60, 30_000, |i| if i % 5 == 4 { 0.0 } else { 12.0 });
        let cfg = gate_cfg(0.2, 0.0); // execution_mode = "one_way"
        let engine_one = SpreadMonitorEngine::new(&state);
        let row_one = gate_eval(&engine_one, &state, &cfg, &spreads, now);
        assert_eq!(
            row_one.status, "ready",
            "one-way must qualify: {:?}",
            row_one.block_reason
        );

        state.strategy_configs.write().insert(
            "BTC".into(),
            crate::state::TickerStrategyConfigDto {
                symbol: "BTC".into(),
                execution_mode: "DOUBLE_WAY".into(),
                ..crate::state::TickerStrategyConfigDto::default()
            },
        );
        let engine_two = SpreadMonitorEngine::new(&state);
        let row_two = gate_eval(&engine_two, &state, &cfg, &spreads, now);
        assert_eq!(
            row_two.status, "triggered",
            "ticker two-way override must demote on uncostable exit: {:?}",
            row_two.block_reason
        );
    }

    #[test]
    fn coverage_gate_full_qualifies_thin_blocked() {
        let state = AppEngineState::new();
        let now = gate_now_ms();
        // Shared flat prefix keeps mu low so the recent 12bps prints read
        // as real deviations; recent window differs only in density: full
        // occupies ~50/50 buckets, thin ~10/50.
        let prefix: Vec<(i64, f64)> = (0..60)
            .map(|i| (now - 3_600_000 - (59 - i as i64) * 30_000, 0.0))
            .collect();
        let mut full = prefix.clone();
        full.extend(window_spreads(now, 60, 30_000, |_| 12.0));
        let mut thin = prefix;
        thin.extend(window_spreads(now, 12, 150_000, |_| 12.0));
        let cfg = gate_cfg(0.0, 0.5);
        let engine_full = SpreadMonitorEngine::new(&state);
        let row_full = gate_eval(&engine_full, &state, &cfg, &full, now);
        assert_eq!(
            row_full.status, "ready",
            "full coverage must be ready: {:?}",
            row_full.block_reason
        );
        assert!(row_full.coverage.unwrap() >= 0.5);
        let engine_thin = SpreadMonitorEngine::new(&state);
        let row_thin = gate_eval(&engine_thin, &state, &cfg, &thin, now);
        assert_eq!(
            row_thin.status, "triggered",
            "thin coverage must not be ready"
        );
        assert!(row_thin.coverage.unwrap() < 0.5);
        assert!(
            row_thin
                .block_reason
                .as_deref()
                .unwrap_or("")
                .contains("coverage"),
            "explicit coverage reason, got {:?}",
            row_thin.block_reason
        );
    }

    #[test]
    fn signal_gates_deterministic_uncomputable_never_qualifies() {
        let state = AppEngineState::new();
        let now = gate_now_ms();
        let cfg = gate_cfg(0.2, 0.5);
        let spreads = window_spreads(now, 60, 30_000, |i| if i % 5 == 4 { 0.0 } else { 12.0 });
        let engine = SpreadMonitorEngine::new(&state);
        let a = gate_eval(&engine, &state, &cfg, &spreads, now);
        let engine2 = SpreadMonitorEngine::new(&state);
        let b = gate_eval(&engine2, &state, &cfg, &spreads, now);
        assert_eq!(a.status, b.status);
        assert_eq!(a.threshold_ratio, b.threshold_ratio);
        assert_eq!(a.coverage, b.coverage);
        // Cold history (all observations older than the window): ratio has
        // zero denominator -> None, must never force-ready.
        let stale: Vec<(i64, f64)> = (0..20)
            .map(|i| (now - 7_200_000 + i as i64 * 30_000, 12.0))
            .collect();
        let engine3 = SpreadMonitorEngine::new(&state);
        let row = gate_eval(&engine3, &state, &cfg, &stale, now);
        assert_ne!(row.status, "ready", "uncomputable gates never qualify");
        assert!(row.threshold_ratio.is_none());
    }

    #[test]
    fn rehydrate_skips_stale_persisted_signals() {
        let state = AppEngineState::new();
        let now = Utc::now().timestamp_millis();
        let store = &state.state_store;
        // Fresh row rehydrates; a day+ stale row must not resurrect as
        // still-active (it would carry a false first_seen age and never be
        // pruned — rehydrate feeds `pairs`, prune keeps `pairs` keys).
        store
            .upsert_monitor_signal(
                "SOL-USD",
                "hyperliquid",
                "lighter",
                "triggered",
                now - 3_600_000,
                now - 60_000,
                Some(8.0),
                Some(3.0),
            )
            .unwrap();
        store
            .upsert_monitor_signal(
                "SPX500",
                "hyperliquid",
                "vanta",
                "triggered",
                now - 90_000_000,
                now - 90_000_000,
                Some(147_000_000.0),
                None,
            )
            .unwrap();
        let engine = SpreadMonitorEngine::new(&state);
        let pairs = engine.pairs.lock();
        assert!(pairs.contains_key(&("SOL-USD".into(), "hyperliquid".into(), "lighter".into())));
        assert!(
            !pairs.contains_key(&("SPX500".into(), "hyperliquid".into(), "vanta".into())),
            "stale triggered row must not rehydrate as a live pair"
        );
    }

    #[test]
    fn signal_gates_backward_compat_zero_disables() {
        let state = AppEngineState::new();
        let now = gate_now_ms();
        // Flicker + thin would fail active floors; with both at 0.0 the
        // gates disable and the same series qualifies on other merits.
        let flicker_thin = window_spreads(now, 12, 150_000, |i| if i == 5 { 12.0 } else { 0.0 });
        let active = gate_cfg(0.2, 0.5);
        let engine_a = SpreadMonitorEngine::new(&state);
        let blocked = gate_eval(&engine_a, &state, &active, &flicker_thin, now);
        assert_eq!(blocked.status, "triggered");
        let disabled = gate_cfg(0.0, 0.0);
        let engine_b = SpreadMonitorEngine::new(&state);
        let row = gate_eval(&engine_b, &state, &disabled, &flicker_thin, now);
        assert_eq!(
            row.status, "ready",
            "zero floors disable gates: {:?}",
            row.block_reason
        );
        // Old persisted JSON without the new keys falls back to sane defaults.
        let old_json = serde_json::json!({"enabled": true, "qualificationMode": "auto"});
        let cfg: MonitorConfigDto = serde_json::from_value(old_json).unwrap();
        assert!((cfg.min_threshold_ratio - 0.2).abs() < 1e-9);
        assert!((cfg.min_coverage - 0.5).abs() < 1e-9);
        // set_config clamps floors into 0..=1 and rejects NaN with defaults.
        let engine_c = SpreadMonitorEngine::new(&state);
        let bad = MonitorConfigDto {
            min_threshold_ratio: 9.0,
            min_coverage: f64::NAN,
            ..Default::default()
        };
        engine_c.set_config(bad, &state).expect("clamp ok");
        let got = engine_c.config();
        assert!((got.min_threshold_ratio - 1.0).abs() < 1e-9);
        assert!((got.min_coverage - 0.5).abs() < 1e-9);
    }
}
