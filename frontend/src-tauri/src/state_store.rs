#![allow(dead_code)]

use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const ALLOWED_STRATEGY_IDS: [&str; 8] =
    ["futures", "radar", "funding", "spot", "prediction", "stat_arb", "triangle", "external"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccountProfile {
    pub id: String,
    pub venue: String,
    pub label: String,
    pub enabled: bool,
    pub is_default: bool,
    /// Lighter-family account tier: "standard" | "plus" | "premium".
    /// Drives the venue's shared-IP rate budget (the lowest tier among
    /// enabled accounts applies) and fee adjustments (+0.5bps for plus).
    #[serde(default = "default_account_tier")]
    pub account_tier: String,
    /// Per-account leverage ceiling, 1..=5. `None` = unlimited (the backend
    /// hard ceiling of 5 still applies). Effective order-time leverage is
    /// min(strategy target, this cap, venue cap).
    #[serde(default)]
    pub max_leverage: Option<u32>,
    pub created_at: String,
    pub updated_at: String,
}

fn default_account_tier() -> String {
    "standard".into()
}

/// One external-delta attribution row: actual equity disagreed with the
/// fills-derived baseline, so the move was classified, recorded, and the
/// baseline rebased to actual.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountEquityEvent {
    pub id: i64,
    pub account_id: String,
    pub ts_ms: i64,
    pub expected: f64,
    pub actual: f64,
    pub delta: f64,
    pub kind: String,
    pub note: String,
}

/// Outcome of [`StateStore::check_account_equity`]: the recorded event, if
/// any, plus a coverage note when no event was emitted.
#[derive(Debug, Clone, PartialEq)]
pub struct EquityCheckReport {
    pub event: Option<AccountEquityEvent>,
    pub note: Option<String>,
}

struct EquityEventDraft {
    account_id: String,
    ts_ms: i64,
    expected: f64,
    actual: f64,
    delta: f64,
    kind: String,
    note: String,
}

fn equity_event_from_row(row: &Row<'_>) -> rusqlite::Result<AccountEquityEvent> {    Ok(AccountEquityEvent {
        id: row.get(0)?,
        account_id: row.get(1)?,
        ts_ms: row.get(2)?,
        expected: row.get(3)?,
        actual: row.get(4)?,
        delta: row.get(5)?,
        kind: row.get(6)?,
        note: row.get(7)?,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StrategyAccountRoute {
    pub id: String,
    pub strategy_id: String,
    pub label: String,
    pub enabled: bool,
    pub account_a_id: String,
    pub account_a_venue: String,
    pub account_a_label: String,
    pub account_b_id: String,
    pub account_b_venue: String,
    pub account_b_label: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Lifecycle of a recorded signal execution. `InFlight`, `Imbalanced`, and
/// `Unresolved` all block further orders on the same symbol; `Balanced` means
/// the signal's own legs confirmed delta-neutral so the ticker is free again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalStatus {
    InFlight,
    Balanced,
    Imbalanced,
    Unresolved,
    Closed,
    Failed,
}

impl SignalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InFlight => "in_flight",
            Self::Balanced => "balanced",
            Self::Imbalanced => "imbalanced",
            Self::Unresolved => "unresolved",
            Self::Closed => "closed",
            Self::Failed => "failed",
        }
    }

    pub fn from_status(raw: &str) -> Option<Self> {
        match raw {
            "in_flight" => Some(Self::InFlight),
            "balanced" => Some(Self::Balanced),
            "imbalanced" => Some(Self::Imbalanced),
            "unresolved" => Some(Self::Unresolved),
            "closed" => Some(Self::Closed),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    pub fn blocks_ticker(&self) -> bool {
        matches!(self, Self::InFlight | Self::Imbalanced | Self::Unresolved)
    }

    /// Whether the signal's filled legs still count toward open position.
    pub fn counts_as_open(&self) -> bool {
        !matches!(self, Self::Closed | Self::Failed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegSide {
    Long,
    Short,
}

impl LegSide {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Long => "long",
            Self::Short => "short",
        }
    }

    fn from_side(raw: &str) -> Option<Self> {
        match raw {
            "long" => Some(Self::Long),
            "short" => Some(Self::Short),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalLeg {
    pub id: String,
    pub signal_id: String,
    pub account_id: Option<String>,
    pub venue: String,
    pub side: LegSide,
    pub intended_qty: f64,
    pub filled_qty: f64,
    pub fill_price: Option<f64>,
    pub fee_usd: f64,
    pub venue_order_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalExecution {
    pub id: String,
    pub strategy_id: String,
    pub symbol: String,
    pub status: SignalStatus,
    pub route_id: Option<String>,
    pub account_long_id: Option<String>,
    pub account_short_id: Option<String>,
    pub expected_net_usd: Option<f64>,
    pub fees_usd: f64,
    pub note: Option<String>,
    pub legs: Vec<SignalLeg>,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
}

/// Candidate persisted before dispatch; legs carry account attribution.
pub struct NewSignalLeg {
    pub account_id: Option<String>,
    pub venue: String,
    pub side: LegSide,
    pub intended_qty: f64,
}

pub struct NewSignalExecution {
    pub id: String,
    pub strategy_id: String,
    pub symbol: String,
    pub route_id: Option<String>,
    pub expected_net_usd: Option<f64>,
    pub note: Option<String>,
    pub legs: Vec<NewSignalLeg>,
}

/// One venue's orderbook top-of-book observation for a symbol at `ts_ms`
/// (epoch milliseconds). Raw mids are stored so baseline choice stays a
/// query-time projection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpreadSample {
    pub symbol: String,
    pub venue_id: String,
    pub ts_ms: i64,
    pub mid: f64,
    pub best_bid: f64,
    pub best_ask: f64,
}

/// Batch entry accepted by `record_spread_samples`.
pub struct VenueSampleEntry {
    pub venue_id: String,
    pub mid: f64,
    pub best_bid: f64,
    pub best_ask: f64,
}

/// One shrunk per-session aggregate for a symbol. `med_abs_spread_bps`
/// is the decision value: the winsorized session median shrunk toward the
/// global median (n0 = 150). Provisional rows must be read as neutral
/// x1.0 by every consumer — never as a tuned multiplier.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionProfileRow {
    pub symbol: String,
    pub session_id: String,
    pub n_episodes: i64,
    pub med_abs_spread_bps: Option<f64>,
    pub p90_spread_bps: Option<f64>,
    pub rv_bps: Option<f64>,
    pub win_rate: Option<f64>,
    pub mean_hold_s: Option<f64>,
    pub provisional: bool,
    pub updated_ts: i64,
}

/// One 24h hour-bin modulator inside a session's row family. Decisions
/// read sessions, never raw bins; the modulator only ever scales within
/// +/-25% and provisional bins read as exactly 1.0.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionHourBinRow {
    pub symbol: String,
    pub hour: i64,
    pub n_obs: i64,
    pub med_spread_bps: Option<f64>,
    pub modulator: f64,
    pub provisional: bool,
    pub updated_ts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HarAnchorRow {
    pub symbol: String,
    pub intercept: f64,
    pub b_daily: f64,
    pub b_weekly: f64,
    pub b_monthly: f64,
    pub r2: f64,
    pub n_days: i64,
    pub forecast_rv_bps: f64,
    pub trailing_mean_rv_bps: f64,
    pub updated_ts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PromotionStateRow {
    pub symbol: String,
    pub status: String,
    pub fit_delta_bps: f64,
    pub val_delta_bps: f64,
    pub reason: String,
    pub updated_ts: i64,
}

/// One durable margin-utilisation observation for a paper venue: equity and
/// used margin in USD plus the utilisation rate in percent. Written on a
/// bounded cadence (fills + periodic heartbeat, capped frequency) and pruned
/// by the shared retention-days cutoff. Feeds the 24h time-weighted average
/// utilisation stat; any strategy or bot may consume it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MarginSnapshot {
    pub venue: String,
    pub account_id: String,
    pub ts_ms: i64,
    pub equity: f64,
    pub used_margin: f64,
    pub rate_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AutotuneApplyEventRow {
    pub id: i64,
    pub symbol: String,
    pub applied_json: String,
    pub detail_json: String,
    pub ts_ms: i64,
}

/// One venue's signed net filled quantity inside an aggregated open position.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionVenueDto {
    pub venue: String,
    /// Positive = net long (buy side), negative = net short (sell side).
    pub net_qty: f64,
}

/// Aggregated current open position for one signal-type identity:
/// (strategy, symbol/event, venue set). Built from every signal execution
/// that still counts as open; partial-flatten children net against their
/// parents because they share the same grouping key.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenPositionDto {
    /// Stable grouping identity: `strategy_id|symbol|venue_a+venue_b`.
    pub key: String,
    pub strategy_id: String,
    pub symbol: String,
    pub signal_count: usize,
    /// Per-venue signed net quantity; long (positive) rows first.
    pub venues: Vec<PositionVenueDto>,
    /// Sum of entry-time expected net USD where signals carried an estimate.
    pub expected_net_usd: Option<f64>,
    /// Mark-to-market PnL minus recorded fees. `None` whenever any filled leg
    /// lacks an entry fill price or a fresh venue mark — never fabricated.
    pub unrealized_pnl_usd: Option<f64>,
    pub fees_usd: f64,
    /// Gross entry notional across filled legs; `None` when any filled leg
    /// lacks an entry price.
    pub notional_usd: Option<f64>,
    /// Earliest signal creation time in the group.
    pub opened_at: String,
}

/// Group open signal executions into per-(strategy, symbol, venue-set)
/// positions. `mark` supplies a fresh current mid for (venue, symbol); when it
/// returns `None` for a filled leg the group's unrealized PnL stays `None`
/// rather than being marked at a stale or invented price.
pub fn aggregate_open_positions(
    signals: &[SignalExecution],
    mark: &dyn Fn(&str, &str) -> Option<f64>,
) -> Vec<OpenPositionDto> {
    use std::collections::BTreeMap;

    const EPS: f64 = 1e-9;
    let mut groups: BTreeMap<(String, String, String), Vec<&SignalExecution>> = BTreeMap::new();
    for s in signals {
        if !s.status.counts_as_open() {
            continue;
        }
        let mut venues: Vec<String> =
            s.legs.iter().map(|l| l.venue.to_lowercase()).collect();
        venues.sort();
        venues.dedup();
        let venue_key = venues.join("+");
        groups
            .entry((s.strategy_id.to_lowercase(), s.symbol.clone(), venue_key))
            .or_default()
            .push(s);
    }

    let mut out = Vec::with_capacity(groups.len());
    for ((strategy_id, symbol, venue_key), members) in groups {
        let mut nets: BTreeMap<String, f64> = BTreeMap::new();
        let mut fees = 0.0f64;
        let mut expected_sum = 0.0f64;
        let mut expected_any = false;
        let mut pnl = Some(0.0f64);
        let mut notional = Some(0.0f64);
        let mut opened_at = String::new();

        for s in &members {
            fees += s.fees_usd;
            if let Some(e) = s.expected_net_usd {
                expected_sum += e;
                expected_any = true;
            }
            if opened_at.is_empty() || s.created_at < opened_at {
                opened_at = s.created_at.clone();
            }
            for leg in &s.legs {
                if leg.filled_qty.abs() <= EPS {
                    continue;
                }
                let signed = match leg.side {
                    LegSide::Long => leg.filled_qty,
                    LegSide::Short => -leg.filled_qty,
                };
                *nets.entry(leg.venue.clone()).or_insert(0.0) += signed;

                match (leg.fill_price, mark(&leg.venue, &s.symbol)) {
                    (Some(entry), Some(mid)) => {
                        if let Some(p) = pnl.as_mut() {
                            *p += (mid - entry) * signed;
                        }
                    }
                    _ => pnl = None,
                }
                match leg.fill_price {
                    Some(entry) => {
                        if let Some(n) = notional.as_mut() {
                            *n += leg.filled_qty * entry;
                        }
                    }
                    None => notional = None,
                }
            }
        }

        // Zero-net rows (a venue flattened to flat by an offsetting child
        // signal) carry no current exposure — drop them; if the whole group
        // nets flat (e.g. unfilled in-flight signal) keep the venues at zero.
        let mut venue_rows: Vec<PositionVenueDto> = nets
            .iter()
            .filter(|(_, q)| q.abs() > EPS)
            .map(|(v, q)| PositionVenueDto {
                venue: v.clone(),
                net_qty: *q,
            })
            .collect();
        if venue_rows.is_empty() {
            venue_rows = venue_key
                .split('+')
                .map(|v| PositionVenueDto {
                    venue: v.to_string(),
                    net_qty: 0.0,
                })
                .collect();
        }
        // Long (positive) rows on top, short (negative) rows below.
        venue_rows.sort_by(|a, b| {
            b.net_qty
                .signum()
                .partial_cmp(&a.net_qty.signum())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.venue.cmp(&b.venue))
        });

        out.push(OpenPositionDto {
            key: format!("{strategy_id}|{symbol}|{venue_key}"),
            strategy_id,
            symbol,
            signal_count: members.len(),
            venues: venue_rows,
            expected_net_usd: expected_any.then_some(expected_sum),
            unrealized_pnl_usd: pnl.map(|p| p - fees),
            fees_usd: fees,
            notional_usd: notional,
            opened_at,
        });
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

/// One persisted pair-signal lifecycle row (monitor_signals table).
#[derive(Debug, Clone)]
pub struct MonitorSignalRow {
    pub symbol: String,
    pub buy_venue: String,
    pub sell_venue: String,
    pub status: String,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
    pub last_edge_bps: Option<f64>,
    pub last_dev_bps: Option<f64>,
}

/// One persisted pair-signal lifecycle event (monitor_signal_events table).
/// Append-only audit history behind `monitor_signals`: every lifecycle
/// transition plus a minute-bucketed heartbeat while a signal stays active.
/// `economics_json` carries the point-in-time friction/exit snapshot so a
/// later reader can replay why the transition fired. No UI reads this table
/// in v1 — the only consumer is the `get_monitor_signal_events` IPC (future
/// bot + debugging).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MonitorSignalEventRow {
    pub id: i64,
    pub symbol: String,
    pub buy_venue: String,
    pub sell_venue: String,
    pub ts_ms: i64,
    pub old_status: Option<String>,
    pub new_status: String,
    pub edge_bps: Option<f64>,
    pub dev_bps: Option<f64>,
    pub p_win: Option<f64>,
    pub economics_json: Option<String>,
    pub trigger: String,
}

/// One open convergence-bot position (radar_bot_positions table): the entry
/// snapshot exit watchers need to survive restarts.
#[derive(Debug, Clone, PartialEq)]
pub struct BotPositionRow {
    pub symbol: String,
    pub buy_venue: String,
    pub sell_venue: String,
    pub entry_ts_ms: i64,
    pub entry_dev_bps: f64,
    pub baseline_bps: Option<f64>,
    pub friction_band_bps: f64,
    pub size_usd: f64,
    pub deadline_ms: i64,
    pub unwatched_since_ms: Option<i64>,
    pub last_seen_ms: i64,
}

/// One Single-Spread entry baseline (spatial_entry_baseline table, v13): the
/// edge snapshot the convergence check needs to decide converge/stop/timeout
/// unwinds for "futures" pair positions.
#[derive(Debug, Clone, PartialEq)]
pub struct SpatialEntryBaseline {
    pub signal_id: String,
    pub symbol: String,
    pub buy_venue: String,
    pub sell_venue: String,
    pub entry_net_bps: f64,
    pub baseline_bps: Option<f64>,
    pub entry_ts_ms: i64,
}

/// Separate row cap for the signal-event audit log. Retention shares the
/// spread-sample `max_days` cutoff (same setting, one pass), but the cap is
/// independent so a chatty pair can never evict price history — and price
/// pruning can never wipe the audit trail's newest window either.
pub const MONITOR_SIGNAL_EVENTS_MAX_ROWS: i64 = 50_000;

const ACCOUNT_COLS: &str =
    "id, venue, label, enabled, is_default, account_tier, max_leverage, created_at, updated_at";

const ACCOUNT_BY_ID_SQL: &str = "\
SELECT id, venue, label, enabled, is_default, account_tier, max_leverage, created_at, updated_at
FROM exchange_accounts
WHERE id = ?1";

const ACCOUNT_ALL_SQL: &str = "\
SELECT id, venue, label, enabled, is_default, account_tier, max_leverage, created_at, updated_at
FROM exchange_accounts
ORDER BY venue COLLATE NOCASE, is_default DESC, label COLLATE NOCASE, id";

const ACCOUNT_BY_VENUE_SQL: &str = "\
SELECT id, venue, label, enabled, is_default, account_tier, max_leverage, created_at, updated_at
FROM exchange_accounts
WHERE venue = ?1
ORDER BY is_default DESC, label COLLATE NOCASE, id";

const ENABLED_DEFAULT_BY_VENUE_SQL: &str = "\
SELECT id FROM exchange_accounts
WHERE venue = ?1 AND is_default = 1 AND enabled = 1
LIMIT 1";

const FALLBACK_DEFAULT_SQL: &str = "\
SELECT id FROM exchange_accounts
WHERE venue = ?1 AND enabled = 1
ORDER BY label COLLATE NOCASE, id
LIMIT 1";

const CLEAR_DEFAULTS_SQL: &str = "\
UPDATE exchange_accounts SET is_default = 0, updated_at = ?2 WHERE venue = ?1";

const SET_DEFAULT_SQL: &str = "\
UPDATE exchange_accounts SET is_default = 1, updated_at = ?2
WHERE venue = ?1 AND id = ?3 AND enabled = 1";

const ACCOUNT_UPSERT_SQL: &str = "\
INSERT INTO exchange_accounts
    (id, venue, label, enabled, is_default, account_tier, max_leverage, created_at, updated_at)
VALUES (?1, ?2, ?3, ?4, 0, ?5, ?7, ?6, ?6)
ON CONFLICT(id) DO UPDATE SET
    label = excluded.label,
    enabled = excluded.enabled,
    is_default = 0,
    account_tier = excluded.account_tier,
    max_leverage = excluded.max_leverage,
    updated_at = excluded.updated_at";

const ROUTE_JOIN_SQL: &str = "\
SELECT r.id, r.strategy_id, r.label, r.enabled,
       a.id, a.venue, a.label,
       b.id, b.venue, b.label,
       r.created_at, r.updated_at
FROM strategy_account_routes r
JOIN exchange_accounts a ON a.id = r.account_a_id
JOIN exchange_accounts b ON b.id = r.account_b_id";

const ROUTE_UPSERT_SQL: &str = "\
INSERT INTO strategy_account_routes
    (id, strategy_id, label, enabled, account_a_id, account_b_id, created_at, updated_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
ON CONFLICT(id) DO UPDATE SET
    strategy_id = excluded.strategy_id,
    label = excluded.label,
    enabled = excluded.enabled,
    account_a_id = excluded.account_a_id,
    account_b_id = excluded.account_b_id,
    updated_at = excluded.updated_at";

const MIGRATION_V1_SQL: &str = "\
BEGIN IMMEDIATE;
CREATE TABLE exchange_accounts (
    id TEXT PRIMARY KEY NOT NULL CHECK(length(id) BETWEEN 1 AND 128),
    venue TEXT NOT NULL COLLATE NOCASE CHECK(length(venue) BETWEEN 1 AND 64),
    label TEXT NOT NULL COLLATE NOCASE CHECK(length(label) BETWEEN 1 AND 80),
    enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
    is_default INTEGER NOT NULL CHECK(is_default IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(venue, label)
);
CREATE UNIQUE INDEX ux_exchange_accounts_one_default
    ON exchange_accounts(venue) WHERE is_default = 1;
CREATE INDEX ix_exchange_accounts_venue_enabled
    ON exchange_accounts(venue, enabled);
CREATE TABLE strategy_account_routes (
    id TEXT PRIMARY KEY NOT NULL CHECK(length(id) BETWEEN 1 AND 128),
    strategy_id TEXT NOT NULL COLLATE NOCASE CHECK(strategy_id IN ('futures', 'funding', 'spot', 'prediction', 'stat_arb')),
    label TEXT NOT NULL COLLATE NOCASE CHECK(length(label) BETWEEN 1 AND 80),
    enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
    account_a_id TEXT NOT NULL REFERENCES exchange_accounts(id) ON DELETE RESTRICT,
    account_b_id TEXT NOT NULL REFERENCES exchange_accounts(id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK(account_a_id <> account_b_id),
    UNIQUE(strategy_id, account_a_id, account_b_id)
);
CREATE INDEX ix_strategy_account_routes_strategy_enabled
    ON strategy_account_routes(strategy_id, enabled);
PRAGMA user_version = 1;
COMMIT;";

/// v2 adds the durable signal-execution ledger. Every dispatched candidate is
/// recorded before execution so an interrupted or unresolved signal keeps its
/// ticker blocked across restarts. Per-leg fills carry account attribution so
/// signal-level balance is evaluated independently of aggregate ticker
/// netting.
const MIGRATION_V2_SQL: &str = "\
BEGIN IMMEDIATE;
CREATE TABLE signal_executions (
    id TEXT PRIMARY KEY NOT NULL CHECK(length(id) BETWEEN 1 AND 128),
    strategy_id TEXT NOT NULL COLLATE NOCASE CHECK(strategy_id IN ('futures', 'funding', 'spot', 'prediction', 'stat_arb')),
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    status TEXT NOT NULL CHECK(status IN ('in_flight', 'balanced', 'imbalanced', 'unresolved', 'closed', 'failed')),
    route_id TEXT REFERENCES strategy_account_routes(id) ON DELETE SET NULL,
    account_long_id TEXT REFERENCES exchange_accounts(id) ON DELETE RESTRICT,
    account_short_id TEXT REFERENCES exchange_accounts(id) ON DELETE RESTRICT,
    expected_net_usd REAL,
    fees_usd REAL NOT NULL DEFAULT 0 CHECK(fees_usd >= 0),
    note TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    closed_at TEXT
);
CREATE INDEX ix_signal_executions_symbol_status
    ON signal_executions(symbol, status);
CREATE TABLE signal_legs (
    id TEXT PRIMARY KEY NOT NULL CHECK(length(id) BETWEEN 1 AND 128),
    signal_id TEXT NOT NULL REFERENCES signal_executions(id) ON DELETE CASCADE,
    account_id TEXT REFERENCES exchange_accounts(id) ON DELETE RESTRICT,
    venue TEXT NOT NULL COLLATE NOCASE CHECK(length(venue) BETWEEN 1 AND 64),
    side TEXT NOT NULL CHECK(side IN ('long', 'short')),
    intended_qty REAL NOT NULL CHECK(intended_qty >= 0),
    filled_qty REAL NOT NULL DEFAULT 0 CHECK(filled_qty >= 0),
    fill_price REAL,
    fee_usd REAL NOT NULL DEFAULT 0 CHECK(fee_usd >= 0),
    venue_order_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX ix_signal_legs_signal ON signal_legs(signal_id);
CREATE INDEX ix_signal_legs_account ON signal_legs(account_id);
PRAGMA user_version = 2;
COMMIT;";

/// v3 adds the durable venue mid-price sample log feeding the dislocation
/// signal series. One row per venue per observed poll; timestamps are epoch
/// milliseconds so arbitrary windows (seconds to days) can be sliced by range.
const MIGRATION_V3_SQL: &str = "\
BEGIN IMMEDIATE;
CREATE TABLE spread_samples (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    venue_id TEXT NOT NULL COLLATE NOCASE CHECK(length(venue_id) BETWEEN 1 AND 64),
    ts_ms INTEGER NOT NULL CHECK(ts_ms >= 0),
    mid REAL NOT NULL CHECK(mid > 0),
    best_bid REAL NOT NULL,
    best_ask REAL NOT NULL
);
CREATE INDEX ix_spread_samples_symbol_ts ON spread_samples(symbol, ts_ms);
PRAGMA user_version = 3;
COMMIT;";

/// v4 adds the exchange account tier (Lighter-family Standard/Plus/Premium
/// drive rate-limit budgets and fee adjustments) and the durable ticker
/// exemption table for the spread monitor's hide/exclude control.
const MIGRATION_V4_SQL: &str = "\
BEGIN IMMEDIATE;
ALTER TABLE exchange_accounts
    ADD COLUMN account_tier TEXT NOT NULL DEFAULT 'standard'
    CHECK(account_tier IN ('standard', 'plus', 'premium', 'builder'));
CREATE TABLE ticker_exemptions (
    symbol TEXT PRIMARY KEY COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    exempt INTEGER NOT NULL CHECK(exempt IN (0, 1)),
    updated_at TEXT NOT NULL
);
PRAGMA user_version = 4;
COMMIT;";

/// v5 adds the spread monitor's persisted configuration (single-row JSON)
/// and the durable pair-signal lifecycle table so discovery timestamps and
/// status survive restarts without replaying the full sample history.
const MIGRATION_V5_SQL: &str = "\
BEGIN IMMEDIATE;
CREATE TABLE monitor_config (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    config_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE monitor_signals (
    symbol TEXT NOT NULL COLLATE NOCASE,
    buy_venue TEXT NOT NULL COLLATE NOCASE,
    sell_venue TEXT NOT NULL COLLATE NOCASE,
    status TEXT NOT NULL CHECK(status IN ('collecting','watching','signaled','qualified','stale')),
    first_seen_ms INTEGER NOT NULL CHECK(first_seen_ms >= 0),
    last_seen_ms INTEGER NOT NULL CHECK(last_seen_ms >= 0),
    last_edge_bps REAL,
    last_dev_bps REAL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (symbol, buy_venue, sell_venue)
);
PRAGMA user_version = 5;
COMMIT;";

const MIGRATION_V6_SQL: &str = "\
BEGIN IMMEDIATE;
CREATE TABLE strategy_params (
  strategy_id TEXT PRIMARY KEY COLLATE NOCASE CHECK(strategy_id IN ('futures','funding','spot','triangle')),
  params_json TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
PRAGMA user_version = 6;
COMMIT;";

const MIGRATION_V7_SQL: &str = "\
BEGIN IMMEDIATE;
CREATE TABLE kv (
  k TEXT PRIMARY KEY,
  v TEXT NOT NULL
);
PRAGMA user_version = 7;
COMMIT;";

/// v8 adds the session-seasonality layer (Phase 1): raw hourly venue
/// volume samples plus shrunk per-session aggregates and their 24h
/// hour-bin modulators. Profiles are derived (rollup) state — the
/// retention passes only ever delete from `spread_samples` (and the raw
/// volume table), so profiles survive every raw-sample prune.
const MIGRATION_V8_SQL: &str = "\
BEGIN IMMEDIATE;
CREATE TABLE venue_volume_samples (
    venue_id TEXT NOT NULL COLLATE NOCASE CHECK(length(venue_id) BETWEEN 1 AND 64),
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    ts_ms INTEGER NOT NULL CHECK(ts_ms >= 0),
    vol_usd REAL NOT NULL CHECK(vol_usd > 0),
    PRIMARY KEY (venue_id, symbol, ts_ms)
);
CREATE INDEX ix_venue_volume_samples_symbol_ts ON venue_volume_samples(symbol, ts_ms);
CREATE TABLE session_profiles (
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    session_id TEXT NOT NULL CHECK(session_id IN ('asia','europe','overlap','us_late','dead')),
    n_episodes INTEGER NOT NULL CHECK(n_episodes >= 0),
    med_abs_spread_bps REAL,
    p90_spread_bps REAL,
    rv_bps REAL,
    win_rate REAL,
    mean_hold_s REAL,
    provisional INTEGER NOT NULL CHECK(provisional IN (0, 1)),
    updated_ts INTEGER NOT NULL CHECK(updated_ts >= 0),
    PRIMARY KEY (symbol, session_id)
);
CREATE TABLE session_hour_bins (
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    hour INTEGER NOT NULL CHECK(hour BETWEEN 0 AND 23),
    n_obs INTEGER NOT NULL CHECK(n_obs >= 0),
    med_spread_bps REAL,
    modulator REAL NOT NULL,
    provisional INTEGER NOT NULL CHECK(provisional IN (0, 1)),
    updated_ts INTEGER NOT NULL CHECK(updated_ts >= 0),
    PRIMARY KEY (symbol, hour)
);
PRAGMA user_version = 8;
COMMIT;";

/// v12 adds Seasonality Phase 2 + Autotune Phase 2 tables:
/// - `har_anchors`: one nightly log-HAR(1,5,22) fit per symbol with its
///   one-day-ahead forecast (rescales session RV norms, never rewrites them
///   beyond ±50%).
/// - `promotion_state`: one walk-forward promotion verdict per symbol
///   (promoted/held/demoted with fit/validate deltas + reason). Absent row =
///   Phase-1 baseline display (fail-closed, no tuned tables served).
/// - `autotune_apply_events`: append-only audit log for one-click
///   apply-selected writes (params + fit/validate metrics + span).
const MIGRATION_V12_SQL: &str = "\
BEGIN IMMEDIATE;
CREATE TABLE har_anchors (
    symbol TEXT PRIMARY KEY NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    intercept REAL NOT NULL,
    b_daily REAL NOT NULL,
    b_weekly REAL NOT NULL,
    b_monthly REAL NOT NULL,
    r2 REAL NOT NULL CHECK(r2 >= 0.0 AND r2 <= 1.0),
    n_days INTEGER NOT NULL CHECK(n_days >= 0),
    forecast_rv_bps REAL NOT NULL CHECK(forecast_rv_bps > 0),
    trailing_mean_rv_bps REAL NOT NULL CHECK(trailing_mean_rv_bps > 0),
    updated_ts INTEGER NOT NULL CHECK(updated_ts >= 0)
);
CREATE TABLE promotion_state (
    symbol TEXT PRIMARY KEY NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    status TEXT NOT NULL CHECK(status IN ('promoted','held','demoted')),
    fit_delta_bps REAL NOT NULL,
    val_delta_bps REAL NOT NULL,
    reason TEXT NOT NULL CHECK(length(reason) BETWEEN 1 AND 512),
    updated_ts INTEGER NOT NULL CHECK(updated_ts >= 0)
);
CREATE TABLE autotune_apply_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    applied_json TEXT NOT NULL CHECK(length(applied_json) BETWEEN 2 AND 8192),
    detail_json TEXT NOT NULL CHECK(length(detail_json) BETWEEN 2 AND 8192),
    ts_ms INTEGER NOT NULL CHECK(ts_ms >= 0)
);
CREATE INDEX ix_autotune_apply_events_symbol_ts
    ON autotune_apply_events(symbol, ts_ms);
PRAGMA user_version = 12;
COMMIT;";

/// v13 admits 'triangle' into the signal_executions strategy CHECK and adds
/// the Single-Spread entry-baseline table. The backend records one baseline
/// row per successful "futures" open (entry edge, optional caller baseline,
/// entry timestamp); the convergence check reads it to decide
/// converge/stop/timeout unwinds. Absent row = fail-closed Unwatched (manual
/// flatten only), never an invented baseline.
const MIGRATION_V13_SQL: &str = "\
PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;
CREATE TABLE signal_executions_v13 (
    id TEXT PRIMARY KEY NOT NULL CHECK(length(id) BETWEEN 1 AND 128),
    strategy_id TEXT NOT NULL COLLATE NOCASE CHECK(strategy_id IN ('futures', 'radar', 'funding', 'spot', 'prediction', 'stat_arb', 'triangle')),
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    status TEXT NOT NULL CHECK(status IN ('in_flight', 'balanced', 'imbalanced', 'unresolved', 'closed', 'failed')),
    route_id TEXT REFERENCES strategy_account_routes(id) ON DELETE SET NULL,
    account_long_id TEXT REFERENCES exchange_accounts(id) ON DELETE RESTRICT,
    account_short_id TEXT REFERENCES exchange_accounts(id) ON DELETE RESTRICT,
    expected_net_usd REAL,
    fees_usd REAL NOT NULL DEFAULT 0 CHECK(fees_usd >= 0),
    note TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    closed_at TEXT
);
INSERT INTO signal_executions_v13
    (id, strategy_id, symbol, status, route_id, account_long_id,
     account_short_id, expected_net_usd, fees_usd, note,
     created_at, updated_at, closed_at)
    SELECT id, strategy_id, symbol, status, route_id, account_long_id,
           account_short_id, expected_net_usd, fees_usd, note,
           created_at, updated_at, closed_at
    FROM signal_executions;
DROP TABLE signal_executions;
ALTER TABLE signal_executions_v13 RENAME TO signal_executions;
CREATE INDEX ix_signal_executions_symbol_status
    ON signal_executions(symbol, status);
CREATE TABLE spatial_entry_baseline (
    signal_id TEXT PRIMARY KEY NOT NULL CHECK(length(signal_id) BETWEEN 1 AND 128)
        REFERENCES signal_executions(id) ON DELETE CASCADE,
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    buy_venue TEXT NOT NULL COLLATE NOCASE CHECK(length(buy_venue) BETWEEN 1 AND 64),
    sell_venue TEXT NOT NULL COLLATE NOCASE CHECK(length(sell_venue) BETWEEN 1 AND 64),
    entry_net_bps REAL NOT NULL,
    baseline_bps REAL,
    entry_ts_ms INTEGER NOT NULL CHECK(entry_ts_ms >= 0)
);
PRAGMA user_version = 13;
COMMIT;
PRAGMA foreign_keys = ON;";

/// v14 adds the per-account leverage ceiling. NULL = unlimited (the backend
/// hard ceiling of 5 still applies); otherwise 1..=5, enforced at order
/// time as min(strategy target, account cap, venue cap).
const MIGRATION_V14_SQL: &str = "\
BEGIN IMMEDIATE;
ALTER TABLE exchange_accounts
    ADD COLUMN max_leverage INTEGER NULL
    CHECK(max_leverage IS NULL OR (max_leverage >= 1 AND max_leverage <= 5));
PRAGMA user_version = 14;
COMMIT;";

/// v15 adds live/external balance attribution: one expected-equity baseline
/// row per paper account plus the append-only external-delta event log.
/// Paper fills move equity only through the fills-only writer; any other
/// nonzero actual-vs-expected move is recorded here and the baseline is
/// rebased so total PnL never absorbs manual flows.
const MIGRATION_V15_SQL: &str = "\
BEGIN IMMEDIATE;
CREATE TABLE paper_equity_expectations (
    account_id TEXT PRIMARY KEY NOT NULL CHECK(length(account_id) BETWEEN 1 AND 128),
    expected REAL NOT NULL CHECK(expected >= 0),
    last_pos_sig TEXT NOT NULL DEFAULT '',
    last_poll_ms INTEGER NOT NULL DEFAULT 0 CHECK(last_poll_ms >= 0),
    updated_at TEXT NOT NULL
);
CREATE TABLE account_equity_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id TEXT NOT NULL CHECK(length(account_id) BETWEEN 1 AND 128),
    ts_ms INTEGER NOT NULL CHECK(ts_ms >= 0),
    expected REAL NOT NULL,
    actual REAL NOT NULL,
    delta REAL NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('deposit', 'withdrawal', 'transfer', 'funding')),
    note TEXT NOT NULL DEFAULT ''
);
CREATE INDEX ix_account_equity_events_account_ts
    ON account_equity_events(account_id, ts_ms);
PRAGMA user_version = 15;
COMMIT;";

/// v16 adds the margin-utilisation snapshot log. One row per paper venue per
/// observation (`venue`, `ts_ms`, `equity`, `used_margin`, `rate_pct`): the
/// durable history behind the 24h time-weighted average utilisation stat.
/// Bounded by the shared retention-days cutoff; never pruned by the GB pass
/// (rows are tiny and independently capped by the writer cadence).
const MIGRATION_V16_SQL: &str = "\
BEGIN IMMEDIATE;
CREATE TABLE margin_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    venue TEXT NOT NULL COLLATE NOCASE CHECK(length(venue) BETWEEN 1 AND 64),
    account_id TEXT NOT NULL DEFAULT '' CHECK(length(account_id) BETWEEN 0 AND 128),
    ts_ms INTEGER NOT NULL CHECK(ts_ms >= 0),
    equity REAL NOT NULL CHECK(equity >= 0),
    used_margin REAL NOT NULL CHECK(used_margin >= 0),
    rate_pct REAL NOT NULL CHECK(rate_pct >= 0)
);
CREATE INDEX ix_margin_snapshots_venue_ts
    ON margin_snapshots(venue, ts_ms);
PRAGMA user_version = 16;
COMMIT;";

/// v17 admits 'triangle' into the strategy_params CHECK constraint.
/// v18 adds the funding-rate history log. Each scan records its observed
/// per-venue perp prints (timestamp bucketed to the minute, so repeat scans
/// within a minute REPLACE rather than duplicate). This is the training
/// substrate the deferred funding-persistence / half-life estimator needs:
/// until now funding lived only in a 120s hub cache. Pruned by the shared
/// retention pass like spread/volume samples.
const MIGRATION_V18_SQL: &str = "\
BEGIN IMMEDIATE;
CREATE TABLE funding_rate_samples (
    venue_id TEXT NOT NULL COLLATE NOCASE CHECK(length(venue_id) BETWEEN 1 AND 64),
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    ts_ms INTEGER NOT NULL CHECK(ts_ms >= 0),
    rate_8h REAL NOT NULL CHECK(rate_8h IS NOT NULL),
    interval_hours INTEGER NOT NULL CHECK(interval_hours >= 1),
    PRIMARY KEY (venue_id, symbol, ts_ms)
);
CREATE INDEX ix_funding_rate_samples_symbol_ts ON funding_rate_samples(symbol, ts_ms);
CREATE INDEX ix_funding_rate_samples_venue_symbol_ts ON funding_rate_samples(venue_id, symbol, ts_ms);
PRAGMA user_version = 18;
COMMIT;";

/// v19 renames the spread-monitor lifecycle statuses to plainer terms and
/// remaps every persisted row: collecting→preparing, watching→tracking,
/// signaled→triggered, qualified→ready, stale→no-data. SQLite cannot alter
/// a CHECK in place, so both tables are rebuilt (V10/V17 pattern) with the
/// new status sets; bot decision rows (bot_enter/skip/exit) pass through
/// untouched. Fresh databases skip straight to v19 via the chained
/// migrations, so the remap is a no-op for them.
const MIGRATION_V19_SQL: &str = "\
BEGIN IMMEDIATE;
CREATE TABLE monitor_signals_v19 (
    symbol TEXT NOT NULL COLLATE NOCASE,
    buy_venue TEXT NOT NULL COLLATE NOCASE,
    sell_venue TEXT NOT NULL COLLATE NOCASE,
    status TEXT NOT NULL CHECK(status IN ('preparing','tracking','triggered','ready','no-data')),
    first_seen_ms INTEGER NOT NULL CHECK(first_seen_ms >= 0),
    last_seen_ms INTEGER NOT NULL CHECK(last_seen_ms >= 0),
    last_edge_bps REAL,
    last_dev_bps REAL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (symbol, buy_venue, sell_venue)
);
INSERT INTO monitor_signals_v19
    (symbol, buy_venue, sell_venue, status, first_seen_ms, last_seen_ms,
     last_edge_bps, last_dev_bps, updated_at)
    SELECT symbol, buy_venue, sell_venue,
           CASE status WHEN 'collecting' THEN 'preparing'
                       WHEN 'watching' THEN 'tracking'
                       WHEN 'signaled' THEN 'triggered'
                       WHEN 'qualified' THEN 'ready'
                       WHEN 'stale' THEN 'no-data'
                       ELSE status END,
           first_seen_ms, last_seen_ms, last_edge_bps, last_dev_bps, updated_at
    FROM monitor_signals;
DROP TABLE monitor_signals;
ALTER TABLE monitor_signals_v19 RENAME TO monitor_signals;
CREATE TABLE monitor_signal_events_v19 (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    buy_venue TEXT NOT NULL COLLATE NOCASE CHECK(length(buy_venue) BETWEEN 1 AND 64),
    sell_venue TEXT NOT NULL COLLATE NOCASE CHECK(length(sell_venue) BETWEEN 1 AND 64),
    ts_ms INTEGER NOT NULL CHECK(ts_ms >= 0),
    old_status TEXT CHECK(old_status IN ('preparing','tracking','triggered','ready','no-data')),
    new_status TEXT NOT NULL CHECK(new_status IN ('preparing','tracking','triggered','ready','no-data','bot_enter','bot_skip','bot_exit')),
    edge_bps REAL,
    dev_bps REAL,
    p_win REAL,
    economics_json TEXT,
    trigger TEXT NOT NULL CHECK(trigger IN ('transition','heartbeat','bot_decision'))
);
INSERT INTO monitor_signal_events_v19
    (id, symbol, buy_venue, sell_venue, ts_ms, old_status, new_status,
     edge_bps, dev_bps, p_win, economics_json, trigger)
    SELECT id, symbol, buy_venue, sell_venue, ts_ms,
           CASE old_status WHEN 'collecting' THEN 'preparing'
                           WHEN 'watching' THEN 'tracking'
                           WHEN 'signaled' THEN 'triggered'
                           WHEN 'qualified' THEN 'ready'
                           WHEN 'stale' THEN 'no-data'
                           ELSE old_status END,
           CASE new_status WHEN 'collecting' THEN 'preparing'
                           WHEN 'watching' THEN 'tracking'
                           WHEN 'signaled' THEN 'triggered'
                           WHEN 'qualified' THEN 'ready'
                           WHEN 'stale' THEN 'no-data'
                           ELSE new_status END,
           edge_bps, dev_bps, p_win, economics_json, trigger
    FROM monitor_signal_events;
DROP TABLE monitor_signal_events;
ALTER TABLE monitor_signal_events_v19 RENAME TO monitor_signal_events;
CREATE INDEX ix_monitor_signal_events_pair_ts
    ON monitor_signal_events(symbol, buy_venue, sell_venue, ts_ms);
PRAGMA user_version = 19;
COMMIT;";
const MIGRATION_V17_SQL: &str = "\
BEGIN IMMEDIATE;
CREATE TABLE strategy_params_v17 (
  strategy_id TEXT PRIMARY KEY COLLATE NOCASE CHECK(strategy_id IN ('futures','funding','spot','triangle')),
  params_json TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
INSERT INTO strategy_params_v17 (strategy_id, params_json, updated_at)
  SELECT strategy_id, params_json, updated_at FROM strategy_params;
DROP TABLE strategy_params;
ALTER TABLE strategy_params_v17 RENAME TO strategy_params;
PRAGMA user_version = 17;
COMMIT;";

/// v11 adds the convergence-bot open-position table. Positions are keyed by
/// ordered pair; each row carries the entry snapshot the exit watchers need
/// (entry deviation, friction band, hold deadline) so a restart re-attaches
/// watchers without replaying the audit log. It also admits 'radar' into the
/// signal_executions strategy CHECK (the app allowlist already accepted it,
/// so ledger inserts for radar fills failed closed at the SQL layer).
const MIGRATION_V11_SQL: &str = "\
PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;
CREATE TABLE radar_bot_positions (
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    buy_venue TEXT NOT NULL COLLATE NOCASE CHECK(length(buy_venue) BETWEEN 1 AND 64),
    sell_venue TEXT NOT NULL COLLATE NOCASE CHECK(length(sell_venue) BETWEEN 1 AND 64),
    entry_ts_ms INTEGER NOT NULL CHECK(entry_ts_ms >= 0),
    entry_dev_bps REAL NOT NULL,
    baseline_bps REAL,
    friction_band_bps REAL NOT NULL CHECK(friction_band_bps >= 0),
    size_usd REAL NOT NULL CHECK(size_usd > 0),
    deadline_ms INTEGER NOT NULL,
    unwatched_since_ms INTEGER,
    last_seen_ms INTEGER NOT NULL,
    PRIMARY KEY (symbol, buy_venue, sell_venue)
);
CREATE TABLE signal_executions_v11 (
    id TEXT PRIMARY KEY NOT NULL CHECK(length(id) BETWEEN 1 AND 128),
    strategy_id TEXT NOT NULL COLLATE NOCASE CHECK(strategy_id IN ('futures', 'radar', 'funding', 'spot', 'prediction', 'stat_arb')),
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    status TEXT NOT NULL CHECK(status IN ('in_flight', 'balanced', 'imbalanced', 'unresolved', 'closed', 'failed')),
    route_id TEXT REFERENCES strategy_account_routes(id) ON DELETE SET NULL,
    account_long_id TEXT REFERENCES exchange_accounts(id) ON DELETE RESTRICT,
    account_short_id TEXT REFERENCES exchange_accounts(id) ON DELETE RESTRICT,
    expected_net_usd REAL,
    fees_usd REAL NOT NULL DEFAULT 0 CHECK(fees_usd >= 0),
    note TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    closed_at TEXT
);
INSERT INTO signal_executions_v11
    (id, strategy_id, symbol, status, route_id, account_long_id,
     account_short_id, expected_net_usd, fees_usd, note,
     created_at, updated_at, closed_at)
    SELECT id, strategy_id, symbol, status, route_id, account_long_id,
           account_short_id, expected_net_usd, fees_usd, note,
           created_at, updated_at, closed_at
    FROM signal_executions;
DROP TABLE signal_executions;
ALTER TABLE signal_executions_v11 RENAME TO signal_executions;
CREATE INDEX ix_signal_executions_symbol_status
    ON signal_executions(symbol, status);
PRAGMA user_version = 11;
COMMIT;
PRAGMA foreign_keys = ON;";

/// v10 admits the convergence-bot audit decisions into the append-only
/// signal-event log and adds the bot's own config row. SQLite cannot alter
/// a CHECK in place, so the events table is rebuilt with the extended
/// decision/trigger sets and every existing row is copied verbatim
/// (lossless: same columns, same order, same index). The lifecycle
/// statuses and the old_status set are untouched — bot rows carry the
/// pair's live lifecycle status (or NULL) in old_status and one of
/// {bot_enter, bot_skip, bot_exit} in new_status with trigger
/// 'bot_decision'.
const MIGRATION_V10_SQL: &str = "\
BEGIN IMMEDIATE;
CREATE TABLE monitor_signal_events_v10 (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    buy_venue TEXT NOT NULL COLLATE NOCASE CHECK(length(buy_venue) BETWEEN 1 AND 64),
    sell_venue TEXT NOT NULL COLLATE NOCASE CHECK(length(sell_venue) BETWEEN 1 AND 64),
    ts_ms INTEGER NOT NULL CHECK(ts_ms >= 0),
    old_status TEXT CHECK(old_status IN ('collecting','watching','signaled','qualified','stale')),
    new_status TEXT NOT NULL CHECK(new_status IN ('collecting','watching','signaled','qualified','stale','bot_enter','bot_skip','bot_exit')),
    edge_bps REAL,
    dev_bps REAL,
    p_win REAL,
    economics_json TEXT,
    trigger TEXT NOT NULL CHECK(trigger IN ('transition','heartbeat','bot_decision'))
);
INSERT INTO monitor_signal_events_v10
    (id, symbol, buy_venue, sell_venue, ts_ms, old_status, new_status,
     edge_bps, dev_bps, p_win, economics_json, trigger)
    SELECT id, symbol, buy_venue, sell_venue, ts_ms, old_status, new_status,
           edge_bps, dev_bps, p_win, economics_json, trigger
    FROM monitor_signal_events;
DROP TABLE monitor_signal_events;
ALTER TABLE monitor_signal_events_v10 RENAME TO monitor_signal_events;
CREATE INDEX ix_monitor_signal_events_pair_ts
    ON monitor_signal_events(symbol, buy_venue, sell_venue, ts_ms);
CREATE TABLE radar_bot_config (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    config_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
PRAGMA user_version = 10;
COMMIT;";

/// v9 adds the append-only signal-event audit log behind `monitor_signals`.
const MIGRATION_V9_SQL: &str = "\
BEGIN IMMEDIATE;
CREATE TABLE monitor_signal_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    buy_venue TEXT NOT NULL COLLATE NOCASE CHECK(length(buy_venue) BETWEEN 1 AND 64),
    sell_venue TEXT NOT NULL COLLATE NOCASE CHECK(length(sell_venue) BETWEEN 1 AND 64),
    ts_ms INTEGER NOT NULL CHECK(ts_ms >= 0),
    old_status TEXT CHECK(old_status IN ('collecting','watching','signaled','qualified','stale')),
    new_status TEXT NOT NULL CHECK(new_status IN ('collecting','watching','signaled','qualified','stale')),
    edge_bps REAL,
    dev_bps REAL,
    p_win REAL,
    economics_json TEXT,
    trigger TEXT NOT NULL CHECK(trigger IN ('transition','heartbeat'))
);
CREATE INDEX ix_monitor_signal_events_pair_ts
    ON monitor_signal_events(symbol, buy_venue, sell_venue, ts_ms);
PRAGMA user_version = 9;
COMMIT;";

/// Durable, non-secret account/route metadata store. Contains no credential
/// material; secrets remain exclusively in the encrypted vault.
pub struct StateStore {
    conn: Mutex<Connection>,
}

fn normalize_id(raw: &str) -> anyhow::Result<String> {
    let v = raw.trim().to_lowercase();
    if v.is_empty()
        || v.len() > 128
        || !v
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        anyhow::bail!("invalid ID: expected 1-128 ASCII alphanumeric/underscore/hyphen");
    }
    Ok(v)
}

fn normalize_venue(raw: &str) -> anyhow::Result<String> {
    let v = raw.trim().to_lowercase();
    if v.is_empty()
        || v.len() > 64
        || !v
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        anyhow::bail!("invalid venue ID: expected 1-64 ASCII alphanumeric/underscore/hyphen");
    }
    Ok(v)
}

fn normalize_label(raw: &str) -> anyhow::Result<String> {
    let v = raw.trim();
    let n = v.chars().count();
    if n == 0 || n > 80 || v.chars().any(|c| c.is_control()) {
        anyhow::bail!("invalid label: expected 1-80 non-control characters");
    }
    Ok(v.to_string())
}

pub(crate) fn normalize_strategy_id(raw: &str) -> anyhow::Result<String> {
    let v = raw.trim().to_lowercase();
    if !ALLOWED_STRATEGY_IDS.contains(&v.as_str()) {
        anyhow::bail!("unsupported strategy ID: {v}");
    }
    Ok(v)
}

fn normalize_symbol(raw: &str) -> anyhow::Result<String> {
    let v = raw.trim();
    if v.is_empty() || v.chars().count() > 64 || v.chars().any(|c| c.is_control()) {
        anyhow::bail!("invalid symbol: expected 1-64 non-control characters");
    }
    Ok(v.to_string())
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

/// Every storage key that may hold samples for one asset. Background paths
/// (monitor lanes, WS feed) record under the canonical base ("BTC") while
/// UI polls record under the full symbol ("BTC-USDC"); reads must cover
/// both or unselected tickers lose their trailing window.
pub fn spread_symbol_aliases(raw: &str) -> Vec<String> {
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let base = canonical_base(&trimmed);
    let mut out = Vec::with_capacity(6);
    for key in [
        trimmed,
        raw.trim().to_uppercase(),
        base.clone(),
        format!("{base}-USDC"),
        format!("{base}-USD"),
        format!("{base}-USDT"),
    ] {
        if !out.contains(&key) {
            out.push(key);
        }
    }
    out
}

fn account_from_row(row: &Row<'_>) -> rusqlite::Result<AccountProfile> {
    Ok(AccountProfile {
        id: row.get(0)?,
        venue: row.get(1)?,
        label: row.get(2)?,
        enabled: row.get(3)?,
        is_default: row.get(4)?,
        account_tier: row.get(5)?,
        max_leverage: row.get::<_, Option<i64>>(6)?.and_then(|v| u32::try_from(v).ok()),
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

/// Validated account tier label for `exchange_accounts.account_tier`.
fn normalize_account_tier(raw: &str) -> anyhow::Result<String> {
    let t = raw.trim().to_lowercase();
    match t.as_str() {
        "standard" | "plus" | "premium" | "builder" => Ok(t),
        "" => Ok("standard".into()),
        other => anyhow::bail!(
            "invalid account tier '{other}': expected standard, plus, premium, or builder"
        ),
    }
}

fn route_from_row(row: &Row<'_>) -> rusqlite::Result<StrategyAccountRoute> {
    Ok(StrategyAccountRoute {
        id: row.get(0)?,
        strategy_id: row.get(1)?,
        label: row.get(2)?,
        enabled: row.get(3)?,
        account_a_id: row.get(4)?,
        account_a_venue: row.get(5)?,
        account_a_label: row.get(6)?,
        account_b_id: row.get(7)?,
        account_b_venue: row.get(8)?,
        account_b_label: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

const SIGNAL_BY_ID_SQL: &str = "\
SELECT id, strategy_id, symbol, status, route_id, account_long_id,
       account_short_id, expected_net_usd, fees_usd, note,
       created_at, updated_at, closed_at
FROM signal_executions
WHERE id = ?1";

fn signal_exec_from_row(row: &Row<'_>) -> rusqlite::Result<SignalExecution> {
    let status_raw: String = row.get(3)?;
    Ok(SignalExecution {
        id: row.get(0)?,
        strategy_id: row.get(1)?,
        symbol: row.get(2)?,
        status: SignalStatus::from_status(&status_raw).unwrap_or(SignalStatus::Unresolved),
        route_id: row.get(4)?,
        account_long_id: row.get(5)?,
        account_short_id: row.get(6)?,
        expected_net_usd: row.get(7)?,
        fees_usd: row.get(8)?,
        note: row.get(9)?,
        legs: Vec::new(),
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
        closed_at: row.get(12)?,
    })
}

fn leg_from_row(row: &Row<'_>) -> rusqlite::Result<SignalLeg> {
    let side_raw: String = row.get(4)?;
    Ok(SignalLeg {
        id: row.get(0)?,
        signal_id: row.get(1)?,
        account_id: row.get(2)?,
        venue: row.get(3)?,
        side: LegSide::from_side(&side_raw).ok_or_else(|| {
            rusqlite::Error::InvalidColumnType(4, "side".into(), rusqlite::types::Type::Text)
        })?,
        intended_qty: row.get(5)?,
        filled_qty: row.get(6)?,
        fill_price: row.get(7)?,
        fee_usd: row.get(8)?,
        venue_order_id: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

impl StateStore {
    pub fn default_path() -> PathBuf {
        let base = if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home)
        } else if let Ok(profile) = std::env::var("USERPROFILE") {
            PathBuf::from(profile)
        } else {
            PathBuf::from(".")
        };
        base.join(".arbitrageur").join("state.sqlite3")
    }

    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            if let Some(parent) = path.parent() {
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
            }
            drop(
                fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(false)
                    .mode(0o600)
                    .open(path)?,
            );
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
        let conn = Connection::open(path)?;
        Self::init(conn, Some(path))
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        Self::init(Connection::open_in_memory()?, None)
    }

    fn init(conn: Connection, file_path: Option<&Path>) -> anyhow::Result<Self> {
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA synchronous = FULL;
             PRAGMA busy_timeout = 5000;
             PRAGMA trusted_schema = OFF;",
        )?;
        if file_path.is_some() {
            conn.execute_batch("PRAGMA journal_mode = WAL;")?;
        }
        Self::migrate(&conn)?;
        #[cfg(unix)]
        if let Some(path) = file_path {
            use std::os::unix::fs::PermissionsExt;
            if let Some(parent) = path.parent() {
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
            }
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn migrate(conn: &Connection) -> anyhow::Result<()> {
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > 19 {
            anyhow::bail!("unsupported state store schema version: {version}");
        }
        if version == 0 {
            if let Err(err) = conn.execute_batch(MIGRATION_V1_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 1 {
            if let Err(err) = conn.execute_batch(MIGRATION_V2_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 2 {
            if let Err(err) = conn.execute_batch(MIGRATION_V3_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 3 {
            if let Err(err) = conn.execute_batch(MIGRATION_V4_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 4 {
            if let Err(err) = conn.execute_batch(MIGRATION_V5_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 5 {
            if let Err(err) = conn.execute_batch(MIGRATION_V6_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 6 {
            if let Err(err) = conn.execute_batch(MIGRATION_V7_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 7 {
            if let Err(err) = conn.execute_batch(MIGRATION_V8_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 8 {
            if let Err(err) = conn.execute_batch(MIGRATION_V9_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 9 {
            if let Err(err) = conn.execute_batch(MIGRATION_V10_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 10 {
            if let Err(err) = conn.execute_batch(MIGRATION_V11_SQL) {
                let _ = conn.execute_batch("ROLLBACK; PRAGMA foreign_keys = ON;");
                return Err(err.into());
            }
        }
        if version <= 11 {
            if let Err(err) = conn.execute_batch(MIGRATION_V12_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 12 {
            if let Err(err) = conn.execute_batch(MIGRATION_V13_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 13 {
            if let Err(err) = conn.execute_batch(MIGRATION_V14_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 14 {
            if let Err(err) = conn.execute_batch(MIGRATION_V15_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 15 {
            if let Err(err) = conn.execute_batch(MIGRATION_V16_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 16 {
            if let Err(err) = conn.execute_batch(MIGRATION_V17_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 17 {
            if let Err(err) = conn.execute_batch(MIGRATION_V18_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 18 {
            if let Err(err) = conn.execute_batch(MIGRATION_V19_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_account(
        &self,
        id: &str,
        venue: &str,
        label: &str,
        enabled: bool,
        make_default: bool,
        account_tier: &str,
        max_leverage: Option<u32>,
    ) -> anyhow::Result<AccountProfile> {
        let id = normalize_id(id)?;
        let venue = normalize_venue(venue)?;
        let label = normalize_label(label)?;
        let account_tier = normalize_account_tier(account_tier)?;
        let max_leverage_i64 = match max_leverage {
            None => None,
            Some(v) if (1..=5).contains(&v) => Some(i64::from(v)),
            Some(v) => anyhow::bail!("max_leverage {v} out of range; must be 1..=5 or unset (unlimited)"),
        };
        let now = Utc::now().to_rfc3339();

        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let existing_venue: Option<String> = tx
            .query_row(
                "SELECT venue FROM exchange_accounts WHERE id = ?1",
                [&id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(existing) = existing_venue {
            if existing != venue {
                anyhow::bail!("account venue is immutable once created");
            }
        }

        let prior_default: Option<String> = tx
            .query_row(ENABLED_DEFAULT_BY_VENUE_SQL, [&venue], |r| r.get(0))
            .optional()?;

        tx.execute(
            ACCOUNT_UPSERT_SQL,
            params![id, venue, label, enabled, account_tier, now, max_leverage_i64],
        )?;

        let chosen: Option<String> = if make_default && enabled {
            Some(id.clone())
        } else {
            let preserved = match prior_default {
                Some(pid) => tx
                    .query_row(ACCOUNT_BY_ID_SQL, [&pid], account_from_row)
                    .optional()?
                    .filter(|a| a.enabled)
                    .map(|a| a.id),
                None => None,
            };
            match preserved {
                Some(pid) => Some(pid),
                None => tx
                    .query_row(FALLBACK_DEFAULT_SQL, [&venue], |r| r.get(0))
                    .optional()?,
            }
        };

        tx.execute(CLEAR_DEFAULTS_SQL, params![venue, now])?;
        if let Some(chosen_id) = chosen {
            let updated = tx.execute(SET_DEFAULT_SQL, params![venue, now, chosen_id])?;
            if updated != 1 {
                anyhow::bail!("failed to assign default account for venue {venue}");
            }
        }

        let profile = tx.query_row(ACCOUNT_BY_ID_SQL, [&id], account_from_row)?;
        tx.commit()?;
        Ok(profile)
    }

    pub fn set_default_account(&self, venue: &str, account_id: &str) -> anyhow::Result<()> {
        let venue = normalize_venue(venue)?;
        let account_id = normalize_id(account_id)?;
        let now = Utc::now().to_rfc3339();

        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let row: Option<(String, bool)> = tx
            .query_row(
                "SELECT venue, enabled FROM exchange_accounts WHERE id = ?1",
                [&account_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (stored_venue, stored_enabled) =
            row.ok_or_else(|| anyhow::anyhow!("account {account_id} does not exist"))?;
        if stored_venue != venue {
            anyhow::bail!("account {account_id} does not belong to venue {venue}");
        }
        if !stored_enabled {
            anyhow::bail!("disabled account cannot be the venue default");
        }

        tx.execute(CLEAR_DEFAULTS_SQL, params![venue, now])?;
        let updated = tx.execute(SET_DEFAULT_SQL, params![venue, now, account_id])?;
        if updated != 1 {
            anyhow::bail!("failed to assign default account for venue {venue}");
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_accounts(&self, venue: Option<&str>) -> anyhow::Result<Vec<AccountProfile>> {
        let conn = self.conn.lock();
        match venue {
            Some(raw) => {
                let venue = normalize_venue(raw)?;
                let mut stmt = conn.prepare(ACCOUNT_BY_VENUE_SQL)?;
                let rows = stmt.query_map([&venue], account_from_row)?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            }
            None => {
                let mut stmt = conn.prepare(ACCOUNT_ALL_SQL)?;
                let rows = stmt.query_map([], account_from_row)?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            }
        }
    }

    pub fn get_account(&self, account_id: &str) -> anyhow::Result<Option<AccountProfile>> {
        let account_id = normalize_id(account_id)?;
        let conn = self.conn.lock();
        Ok(conn
            .query_row(ACCOUNT_BY_ID_SQL, [&account_id], account_from_row)
            .optional()?)
    }

    pub fn delete_account(&self, account_id: &str) -> anyhow::Result<bool> {
        let account_id = normalize_id(account_id)?;
        let now = Utc::now().to_rfc3339();

        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let venue: Option<String> = tx
            .query_row(
                "SELECT venue FROM exchange_accounts WHERE id = ?1",
                [&account_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(venue) = venue else {
            return Ok(false);
        };

        let deleted = match tx.execute("DELETE FROM exchange_accounts WHERE id = ?1", [&account_id])
        {
            Ok(n) => n,
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                anyhow::bail!(
                    "account {account_id} is referenced by strategy routes; remove or reassign those routes first"
                );
            }
            Err(err) => return Err(err.into()),
        };
        if deleted == 0 {
            return Ok(false);
        }

        let next_default: Option<String> = tx
            .query_row(FALLBACK_DEFAULT_SQL, [&venue], |r| r.get(0))
            .optional()?;
        tx.execute(CLEAR_DEFAULTS_SQL, params![venue, now])?;
        if let Some(chosen_id) = next_default {
            let updated = tx.execute(SET_DEFAULT_SQL, params![venue, now, chosen_id])?;
            if updated != 1 {
                anyhow::bail!("failed to reassign default account for venue {venue}");
            }
        }
        tx.commit()?;
        Ok(true)
    }

    pub fn has_enabled_account(&self, venue: &str) -> anyhow::Result<bool> {
        let venue = normalize_venue(venue)?;
        let conn = self.conn.lock();
        let exists: bool = conn.query_row(
            "SELECT EXISTS(
    SELECT 1 FROM exchange_accounts WHERE venue = ?1 AND enabled = 1
)",
            [&venue],
            |r| r.get(0),
        )?;
        Ok(exists)
    }

    pub fn default_account(&self, venue: &str) -> anyhow::Result<Option<AccountProfile>> {
        let venue = normalize_venue(venue)?;
        let conn = self.conn.lock();
        Ok(conn
            .query_row(
                "SELECT id, venue, label, enabled, is_default, account_tier, max_leverage, created_at, updated_at
FROM exchange_accounts
WHERE venue = ?1 AND enabled = 1
ORDER BY is_default DESC, label COLLATE NOCASE, id
LIMIT 1",
                [&venue],
                account_from_row,
            )
            .optional()?)
    }

    pub fn upsert_strategy_route(
        &self,
        id: &str,
        strategy_id: &str,
        label: &str,
        account_a_id: &str,
        account_b_id: &str,
        enabled: bool,
    ) -> anyhow::Result<StrategyAccountRoute> {
        let id = normalize_id(id)?;
        let strategy_id = normalize_strategy_id(strategy_id)?;
        let label = normalize_label(label)?;
        let a_id = normalize_id(account_a_id)?;
        let b_id = normalize_id(account_b_id)?;
        if a_id == b_id {
            anyhow::bail!("route endpoints must reference two different accounts");
        }
        let now = Utc::now().to_rfc3339();

        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let account_a = tx
            .query_row(ACCOUNT_BY_ID_SQL, [&a_id], account_from_row)
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("route endpoint account {a_id} does not exist"))?;
        let account_b = tx
            .query_row(ACCOUNT_BY_ID_SQL, [&b_id], account_from_row)
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("route endpoint account {b_id} does not exist"))?;

        if account_a.venue == account_b.venue {
            anyhow::bail!("route endpoint accounts must be on different venues");
        }
        if enabled && !(account_a.enabled && account_b.enabled) {
            anyhow::bail!("enabled routes require both endpoint accounts to be enabled");
        }

        let (first, second) = if (account_a.venue.as_str(), account_a.id.as_str())
            <= (account_b.venue.as_str(), account_b.id.as_str())
        {
            (account_a, account_b)
        } else {
            (account_b, account_a)
        };

        let prior_created: Option<String> = tx
            .query_row(
                "SELECT created_at FROM strategy_account_routes WHERE id = ?1",
                [&id],
                |r| r.get(0),
            )
            .optional()?;

        tx.execute(
            ROUTE_UPSERT_SQL,
            params![id, strategy_id, label, enabled, first.id, second.id, now],
        )?;

        let route = StrategyAccountRoute {
            id,
            strategy_id,
            label,
            enabled,
            account_a_id: first.id,
            account_a_venue: first.venue,
            account_a_label: first.label,
            account_b_id: second.id,
            account_b_venue: second.venue,
            account_b_label: second.label,
            created_at: prior_created.unwrap_or_else(|| now.clone()),
            updated_at: now,
        };
        tx.commit()?;
        Ok(route)
    }

    pub fn list_strategy_routes(
        &self,
        strategy_id: Option<&str>,
    ) -> anyhow::Result<Vec<StrategyAccountRoute>> {
        let conn = self.conn.lock();
        match strategy_id {
            Some(raw) => {
                let sid = normalize_strategy_id(raw)?;
                let mut stmt = conn.prepare(&format!(
                    "{ROUTE_JOIN_SQL}
WHERE r.strategy_id = ?1
ORDER BY r.label COLLATE NOCASE, r.id"
                ))?;
                let rows = stmt.query_map([&sid], route_from_row)?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            }
            None => {
                let mut stmt = conn.prepare(&format!(
                    "{ROUTE_JOIN_SQL}
ORDER BY r.strategy_id COLLATE NOCASE, r.label COLLATE NOCASE, r.id"
                ))?;
                let rows = stmt.query_map([], route_from_row)?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            }
        }
    }

    pub fn eligible_strategy_routes(
        &self,
        strategy_id: &str,
        venue_a: &str,
        venue_b: &str,
    ) -> anyhow::Result<Vec<StrategyAccountRoute>> {
        let sid = normalize_strategy_id(strategy_id)?;
        let venue_a = normalize_venue(venue_a)?;
        let venue_b = normalize_venue(venue_b)?;
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&format!(
            "{ROUTE_JOIN_SQL}
WHERE r.strategy_id = ?1 AND r.enabled = 1
  AND a.enabled = 1 AND b.enabled = 1
  AND ((a.venue = ?2 AND b.venue = ?3) OR (a.venue = ?3 AND b.venue = ?2))
ORDER BY r.label COLLATE NOCASE, r.id"
        ))?;
        let rows = stmt.query_map(params![sid, venue_a, venue_b], route_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// All strategy routes that reference `account_id` on either leg. Used
    /// to block account deletion with an explanatory error before any secret
    /// material is touched.
    pub fn routes_referencing(
        &self,
        account_id: &str,
    ) -> anyhow::Result<Vec<StrategyAccountRoute>> {
        let account_id = normalize_id(account_id)?;
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&format!(
            "{ROUTE_JOIN_SQL}
WHERE r.account_a_id = ?1 OR r.account_b_id = ?1
ORDER BY r.strategy_id COLLATE NOCASE, r.label COLLATE NOCASE, r.id"
        ))?;
        let rows = stmt.query_map([&account_id], route_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn delete_strategy_route(&self, route_id: &str) -> anyhow::Result<bool> {
        let route_id = normalize_id(route_id)?;
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let deleted = tx.execute(
            "DELETE FROM strategy_account_routes WHERE id = ?1",
            [&route_id],
        )?;
        tx.commit()?;
        Ok(deleted > 0)
    }

    /* ------------------------------------------------------------------ */
    /* Signal executions & legs                                             */
    /* ------------------------------------------------------------------ */

    /// Persist a selected candidate as `in_flight` before any order is
    /// dispatched. Because the row exists before execution, an interrupted
    /// run still blocks the symbol on restart until the legs reconcile.
    pub fn record_signal_execution(
        &self,
        signal: &NewSignalExecution,
    ) -> anyhow::Result<SignalExecution> {
        let id = normalize_id(&signal.id)?;
        let strategy_id = normalize_strategy_id(&signal.strategy_id)?;
        let symbol = normalize_symbol(&signal.symbol)?;
        if signal.legs.is_empty() || signal.legs.len() > 2 {
            anyhow::bail!("signal executions require 1 or 2 legs");
        }
        let mut account_long: Option<String> = None;
        let mut account_short: Option<String> = None;
        for leg in &signal.legs {
            if !leg.intended_qty.is_finite() || leg.intended_qty <= 0.0 {
                anyhow::bail!("leg intended quantity must be finite and positive");
            }
            normalize_venue(&leg.venue)?;
            if let Some(aid) = &leg.account_id {
                normalize_id(aid)?;
            }
            match leg.side {
                LegSide::Long => account_long = leg.account_id.clone(),
                LegSide::Short => account_short = leg.account_id.clone(),
            }
        }
        if let Some(rid) = &signal.route_id {
            normalize_id(rid)?;
        }
        let now = Utc::now().to_rfc3339();

        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO signal_executions
                (id, strategy_id, symbol, status, route_id, account_long_id,
                 account_short_id, expected_net_usd, fees_usd, note, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'in_flight', ?4, ?5, ?6, ?7, 0, ?8, ?9, ?9)",
            params![
                id,
                strategy_id,
                symbol,
                signal.route_id,
                account_long,
                account_short,
                signal.expected_net_usd,
                signal.note,
                now
            ],
        )?;
        for leg in &signal.legs {
            let leg_id = normalize_id(&format!("{id}-{}", leg.side.as_str()))?;
            tx.execute(
                "INSERT INTO signal_legs
                    (id, signal_id, account_id, venue, side, intended_qty,
                     filled_qty, fill_price, fee_usd, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, NULL, 0, ?7, ?7)",
                params![
                    leg_id,
                    id,
                    leg.account_id,
                    leg.venue,
                    leg.side.as_str(),
                    leg.intended_qty,
                    now
                ],
            )?;
        }
        tx.commit()?;
        drop(conn);
        Ok(self
            .get_signal(&id)?
            .expect("signal execution must exist after insert"))
    }

    /// Transition a signal's status; terminal states stamp `closed_at`.
    pub fn update_signal_status(
        &self,
        signal_id: &str,
        status: SignalStatus,
        note: Option<&str>,
    ) -> anyhow::Result<()> {
        let signal_id = normalize_id(signal_id)?;
        let now = Utc::now().to_rfc3339();
        let terminal = matches!(status, SignalStatus::Closed | SignalStatus::Failed);
        let conn = self.conn.lock();
        let changed = if terminal {
            conn.execute(
                "UPDATE signal_executions
                 SET status = ?2, note = COALESCE(?3, note), updated_at = ?4, closed_at = ?4
                 WHERE id = ?1",
                params![signal_id, status.as_str(), note, now],
            )?
        } else {
            conn.execute(
                "UPDATE signal_executions
                 SET status = ?2, note = COALESCE(?3, note), updated_at = ?4
                 WHERE id = ?1",
                params![signal_id, status.as_str(), note, now],
            )?
        };
        if changed == 0 {
            anyhow::bail!("signal execution {signal_id} does not exist");
        }
        Ok(())
    }

    /// Record an actual fill on one leg of a signal (matched by side).
    pub fn apply_leg_fill(
        &self,
        signal_id: &str,
        side: LegSide,
        filled_qty: f64,
        fill_price: f64,
        fee_usd: f64,
        venue_order_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let signal_id = normalize_id(signal_id)?;
        if !filled_qty.is_finite() || filled_qty < 0.0 {
            anyhow::bail!("filled quantity must be finite and nonnegative");
        }
        if !fill_price.is_finite() || fill_price <= 0.0 {
            anyhow::bail!("fill price must be finite and positive");
        }
        if !fee_usd.is_finite() || fee_usd < 0.0 {
            anyhow::bail!("fee must be finite and nonnegative");
        }
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock();
        let changed = conn.execute(
            "UPDATE signal_legs
             SET filled_qty = ?3, fill_price = ?4, fee_usd = ?5,
                 venue_order_id = COALESCE(?6, venue_order_id), updated_at = ?7
             WHERE signal_id = ?1 AND side = ?2",
            params![
                signal_id,
                side.as_str(),
                filled_qty,
                fill_price,
                fee_usd,
                venue_order_id,
                now
            ],
        )?;
        if changed == 0 {
            anyhow::bail!("signal {signal_id} has no {side:?} leg");
        }
        conn.execute(
            "UPDATE signal_executions
             SET fees_usd = (SELECT COALESCE(SUM(fee_usd), 0) FROM signal_legs WHERE signal_id = ?1),
                 updated_at = ?2
             WHERE id = ?1",
            params![signal_id, now],
        )?;
        Ok(())
    }

    pub fn get_signal(&self, signal_id: &str) -> anyhow::Result<Option<SignalExecution>> {
        let signal_id = normalize_id(signal_id)?;
        let conn = self.conn.lock();
        let exec = conn
            .query_row(SIGNAL_BY_ID_SQL, [&signal_id], signal_exec_from_row)
            .optional()?;
        match exec {
            Some(mut e) => {
                e.legs = Self::legs_for(&conn, &e.id)?;
                Ok(Some(e))
            }
            None => Ok(None),
        }
    }

    pub fn list_signals(
        &self,
        symbol: Option<&str>,
        status: Option<SignalStatus>,
    ) -> anyhow::Result<Vec<SignalExecution>> {
        let conn = self.conn.lock();
        let mut sql = String::from(
            "SELECT id, strategy_id, symbol, status, route_id, account_long_id,
                    account_short_id, expected_net_usd, fees_usd, note,
                    created_at, updated_at, closed_at
             FROM signal_executions WHERE 1=1",
        );
        let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(s) = symbol {
            sql.push_str(" AND symbol = ?");
            binds.push(Box::new(normalize_symbol(s)?));
        }
        if let Some(st) = status {
            sql.push_str(" AND status = ?");
            binds.push(Box::new(st.as_str().to_string()));
        }
        sql.push_str(" ORDER BY created_at DESC, id");
        let refs: Vec<&dyn rusqlite::types::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(refs.as_slice(), signal_exec_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        let mut out = Vec::with_capacity(rows.len());
        for mut e in rows {
            e.legs = Self::legs_for(&conn, &e.id)?;
            out.push(e);
        }
        Ok(out)
    }

    /// Signals that block new orders on a symbol: in-flight, imbalanced, or
    /// unresolved legs must reconcile before another candidate may dispatch.
    pub fn blocking_signals(&self, symbol: &str) -> anyhow::Result<Vec<SignalExecution>> {
        let symbol = normalize_symbol(symbol)?;
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, strategy_id, symbol, status, route_id, account_long_id,
                    account_short_id, expected_net_usd, fees_usd, note,
                    created_at, updated_at, closed_at
             FROM signal_executions
             WHERE symbol = ?1
               AND status IN ('in_flight', 'imbalanced', 'unresolved')
             ORDER BY created_at, id",
        )?;
        let rows = stmt
            .query_map([&symbol], signal_exec_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        let mut out = Vec::with_capacity(rows.len());
        for mut e in rows {
            e.legs = Self::legs_for(&conn, &e.id)?;
            out.push(e);
        }
        Ok(out)
    }

    pub fn ticker_blocked(&self, symbol: &str) -> anyhow::Result<bool> {
        Ok(!self.blocking_signals(symbol)?.is_empty())
    }

    /// Signed net delta of one signal's filled legs (long +, short −).
    /// Delta ≈ 0 means the signal hedged itself; anything else is residual
    /// directional exposure belonging to that signal alone.
    pub fn signal_delta(&self, signal_id: &str) -> anyhow::Result<f64> {
        let signal_id = normalize_id(signal_id)?;
        let conn = self.conn.lock();
        Ok(conn.query_row(
            "SELECT COALESCE(SUM(CASE side WHEN 'long' THEN filled_qty ELSE -filled_qty END), 0)
             FROM signal_legs WHERE signal_id = ?1",
            [&signal_id],
            |r| r.get(0),
        )?)
    }

    /// Signed net position for a symbol across every signal whose filled legs
    /// still count as open (excludes closed and failed signals).
    pub fn ticker_net_position(&self, symbol: &str) -> anyhow::Result<f64> {
        let symbol = normalize_symbol(symbol)?;
        let conn = self.conn.lock();
        Ok(conn.query_row(
            "SELECT COALESCE(SUM(CASE l.side WHEN 'long' THEN l.filled_qty ELSE -l.filled_qty END), 0)
             FROM signal_legs l
             JOIN signal_executions s ON s.id = l.signal_id
             WHERE s.symbol = ?1 AND s.status NOT IN ('closed', 'failed')",
            [&symbol],
            |r| r.get(0),
        )?)
    }

    /// Per-account net position on a symbol — the attribution exchanges lose
    /// when they only report aggregate positions.
    pub fn account_symbol_net(&self, account_id: &str, symbol: &str) -> anyhow::Result<f64> {
        let account_id = normalize_id(account_id)?;
        let symbol = normalize_symbol(symbol)?;
        let conn = self.conn.lock();
        Ok(conn.query_row(
            "SELECT COALESCE(SUM(CASE l.side WHEN 'long' THEN l.filled_qty ELSE -l.filled_qty END), 0)
             FROM signal_legs l
             JOIN signal_executions s ON s.id = l.signal_id
             WHERE l.account_id = ?1 AND s.symbol = ?2
               AND s.status NOT IN ('closed', 'failed')",
            params![account_id, symbol],
            |r| r.get(0),
        )?)
    }

    /// Per-venue net position on a symbol across all accounts/signals.
    pub fn venue_symbol_net(&self, venue: &str, symbol: &str) -> anyhow::Result<f64> {
        let venue = normalize_venue(venue)?;
        let symbol = normalize_symbol(symbol)?;
        let conn = self.conn.lock();
        Ok(conn.query_row(
            "SELECT COALESCE(SUM(CASE l.side WHEN 'long' THEN l.filled_qty ELSE -l.filled_qty END), 0)
             FROM signal_legs l
             JOIN signal_executions s ON s.id = l.signal_id
             WHERE l.venue = ?1 AND s.symbol = ?2
               AND s.status NOT IN ('closed', 'failed')",
            params![venue, symbol],
            |r| r.get(0),
        )?)
    }

    /// Whether any signal leg references the account — recorded position
    /// history blocks account deletion (disable the account instead).
    pub fn account_has_signal_legs(&self, account_id: &str) -> anyhow::Result<bool> {
        let account_id = normalize_id(account_id)?;
        let conn = self.conn.lock();
        Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_legs WHERE account_id = ?1)",
            [&account_id],
            |r| r.get::<_, i64>(0),
        )? == 1)
    }

    pub fn upsert_strategy_params_json(
        &self,
        strategy_id: &str,
        params_json: &str,
    ) -> anyhow::Result<()> {
        let id = normalize_id(strategy_id)?;
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO strategy_params (strategy_id, params_json, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(strategy_id) DO UPDATE SET params_json = ?2, updated_at = ?3",
            params![id, params_json, now],
        )?;
        Ok(())
    }

    pub fn strategy_params_json(&self) -> anyhow::Result<Vec<(String, String)>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT strategy_id, params_json FROM strategy_params ORDER BY strategy_id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /* ------------------------------------------------------------------ */
    /* Spread samples (durable dislocation signal history)                   */
    /* ------------------------------------------------------------------ */

    /// Record one poll's per-venue top-of-book observations in a single
    /// transaction. Rows with non-positive/non-finite mids are dropped rather
    /// than persisted as plausible-looking numbers.
    pub fn record_spread_samples(
        &self,
        symbol: &str,
        ts_ms: i64,
        entries: &[VenueSampleEntry],
    ) -> anyhow::Result<()> {
        let symbol = normalize_symbol(symbol)?;
        if entries.is_empty() || ts_ms < 0 {
            return Ok(());
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO spread_samples (symbol, venue_id, ts_ms, mid, best_bid, best_ask)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for e in entries {
                let venue_id = normalize_venue(&e.venue_id)?;
                if !e.mid.is_finite() || e.mid <= 0.0 {
                    continue;
                }
                stmt.execute(params![
                    symbol, venue_id, ts_ms, e.mid, e.best_bid, e.best_ask
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Samples for a symbol in `[from_ts_ms, to_ts_ms]`, ordered by time then venue.
    /// Matches every storage alias for the asset (canonical base plus
    /// quoted UI forms) so background and UI recording paths read as one.
    pub fn spread_samples(
        &self,
        symbol: &str,
        from_ts_ms: i64,
        to_ts_ms: i64,
    ) -> anyhow::Result<Vec<SpreadSample>> {
        let symbol = normalize_symbol(symbol)?;
        let keys = spread_symbol_aliases(&symbol);
        let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT symbol, venue_id, ts_ms, mid, best_bid, best_ask
             FROM spread_samples
             WHERE symbol IN ({placeholders}) AND ts_ms >= ? AND ts_ms <= ?
             ORDER BY ts_ms ASC, venue_id ASC"
        );
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let mut params: Vec<&dyn rusqlite::ToSql> = keys
            .iter()
            .map(|k| k as &dyn rusqlite::ToSql)
            .collect();
        params.push(&from_ts_ms);
        params.push(&to_ts_ms);
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            Ok(SpreadSample {
                symbol: r.get(0)?,
                venue_id: r.get(1)?,
                ts_ms: r.get(2)?,
                mid: r.get(3)?,
                best_bid: r.get(4)?,
                best_ask: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Earliest recorded sample timestamp for a symbol (history depth indicator).
    /// Alias-aware, matching [`Self::spread_samples`].
    pub fn earliest_spread_sample_ts(&self, symbol: &str) -> anyhow::Result<Option<i64>> {
        let symbol = normalize_symbol(symbol)?;
        let keys = spread_symbol_aliases(&symbol);
        let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT MIN(ts_ms) FROM spread_samples WHERE symbol IN ({placeholders})");
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> = keys
            .iter()
            .map(|k| k as &dyn rusqlite::ToSql)
            .collect();
        Ok(stmt.query_row(rusqlite::params_from_iter(params), |r| r.get(0))?)
    }

    /// Drop samples older than `retain_after_ts_ms`, then enforce a global row
    /// cap by evicting the oldest rows first.
    pub fn prune_spread_samples(
        &self,
        retain_after_ts_ms: i64,
        max_rows: i64,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM spread_samples WHERE ts_ms < ?1",
            [retain_after_ts_ms],
        )?;
        conn.execute(
            "DELETE FROM spread_samples WHERE id IN (
                 SELECT id FROM spread_samples ORDER BY ts_ms DESC, id DESC LIMIT -1 OFFSET ?1
             )",
            [max_rows],
        )?;
        Ok(())
    }

    /* ------------------------------------------------------------------ */
    /* Session seasonality (Phase 1): volume sampler + profiles + rollup    */
    /* ------------------------------------------------------------------ */

    /// Persist one volume-refresh poll per venue. Symbols are stored under
    /// the canonical base so reads stay alias-normalized through
    /// `spread_symbol_aliases`. Best-effort shape: bad rows are skipped,
    /// never persisted as plausible numbers.
    pub fn record_venue_volume_samples(
        &self,
        venue: &str,
        ts_ms: i64,
        volumes: &HashMap<String, f64>,
    ) -> anyhow::Result<()> {
        let venue = normalize_venue(venue)?;
        if volumes.is_empty() || ts_ms < 0 {
            return Ok(());
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO venue_volume_samples (venue_id, symbol, ts_ms, vol_usd)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (asset, vol) in volumes {
                if !vol.is_finite() || *vol <= 0.0 {
                    continue;
                }
                let symbol = canonical_base(asset);
                if symbol.is_empty() {
                    continue;
                }
                stmt.execute(params![venue, symbol, ts_ms, vol])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Raw volume series for one symbol/venue window. Alias-aware like
    /// `spread_samples`.
    pub fn venue_volume_series(
        &self,
        symbol: &str,
        venue: &str,
        from_ts_ms: i64,
        to_ts_ms: i64,
    ) -> anyhow::Result<Vec<(i64, f64)>> {
        let symbol = normalize_symbol(symbol)?;
        let venue = normalize_venue(venue)?;
        let keys = spread_symbol_aliases(&symbol);
        let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT ts_ms, vol_usd FROM venue_volume_samples
             WHERE venue_id = ? AND symbol IN ({placeholders})
               AND ts_ms >= ? AND ts_ms <= ?
             ORDER BY ts_ms ASC"
        );
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(keys.len() + 3);
        params.push(&venue);
        for k in &keys {
            params.push(k);
        }
        params.push(&from_ts_ms);
        params.push(&to_ts_ms);
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Bound the raw volume log with the same retention cutoffs as spread
    /// samples. Profile and hour-bin tables are never touched here — they
    /// are derived state that survives every raw-sample prune.
    pub fn prune_venue_volume_samples(
        &self,
        retain_after_ts_ms: i64,
        max_rows: i64,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM venue_volume_samples WHERE ts_ms < ?1",
            [retain_after_ts_ms],
        )?;
        conn.execute(
            "DELETE FROM venue_volume_samples WHERE rowid IN (
                 SELECT rowid FROM venue_volume_samples
                 ORDER BY ts_ms DESC, rowid DESC LIMIT -1 OFFSET ?1
             )",
            [max_rows],
        )?;
        Ok(())
    }

    /// Distinct canonical symbols present in the spread log (rollup driver).
    pub fn distinct_sample_symbols(&self) -> anyhow::Result<Vec<String>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT DISTINCT symbol FROM spread_samples")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out: Vec<String> = Vec::new();
        for raw in rows.collect::<rusqlite::Result<Vec<_>>>()? {
            let base = canonical_base(&raw);
            if !base.is_empty() && !out.contains(&base) {
                out.push(base);
            }
        }
        out.sort();
        Ok(out)
    }

    /// Read shrunk session aggregates for a symbol (alias-aware). Missing
    /// sessions simply have no row — callers read that as neutral x1.0.
    pub fn session_profiles(&self, symbol: &str) -> anyhow::Result<Vec<SessionProfileRow>> {
        let symbol = normalize_symbol(symbol)?;
        let keys = spread_symbol_aliases(&symbol);
        let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT symbol, session_id, n_episodes, med_abs_spread_bps, p90_spread_bps,
                    rv_bps, win_rate, mean_hold_s, provisional, updated_ts
             FROM session_profiles WHERE symbol IN ({placeholders})
             ORDER BY session_id ASC"
        );
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            keys.iter().map(|k| k as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            Ok(SessionProfileRow {
                symbol: r.get(0)?,
                session_id: r.get(1)?,
                n_episodes: r.get(2)?,
                med_abs_spread_bps: r.get(3)?,
                p90_spread_bps: r.get(4)?,
                rv_bps: r.get(5)?,
                win_rate: r.get(6)?,
                mean_hold_s: r.get(7)?,
                provisional: r.get::<_, i64>(8)? != 0,
                updated_ts: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Read hour-bin modulators for a symbol (alias-aware).
    pub fn session_hour_bins(&self, symbol: &str) -> anyhow::Result<Vec<SessionHourBinRow>> {
        let symbol = normalize_symbol(symbol)?;
        let keys = spread_symbol_aliases(&symbol);
        let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT symbol, hour, n_obs, med_spread_bps, modulator, provisional, updated_ts
             FROM session_hour_bins WHERE symbol IN ({placeholders})
             ORDER BY hour ASC"
        );
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            keys.iter().map(|k| k as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            Ok(SessionHourBinRow {
                symbol: r.get(0)?,
                hour: r.get(1)?,
                n_obs: r.get(2)?,
                med_spread_bps: r.get(3)?,
                modulator: r.get(4)?,
                provisional: r.get::<_, i64>(5)? != 0,
                updated_ts: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Hourly rollup: rebuild one symbol's session aggregates plus 24h
    /// hour-bin modulators from the trailing spread log. Pair spreads are
    /// aligned with the same 30s gap rule as the live monitor; gaps stay
    /// gaps (no interpolation). Returns profile rows written (0 when the
    /// history cannot support a single pair observation).
    pub fn rollup_session_profiles(
        &self,
        symbol: &str,
        from_ts_ms: i64,
        to_ts_ms: i64,
        now_ms: i64,
    ) -> anyhow::Result<usize> {
        use arbitrageur_math::pair_monitor::{
            build_pair_series, reversion_stats, PairObservation,
        };
        use arbitrageur_math::session::{
            hour_modulator, hour_of_day_utc, percentile_sorted, session_of, session_stats,
            winsorized_median, SessionId, SESSION_MIN_EPISODES,
        };

        let canonical = canonical_base(symbol);
        if canonical.is_empty() {
            anyhow::bail!("invalid symbol: expected 1-64 non-control characters");
        }
        let samples = self.spread_samples(symbol, from_ts_ms, to_ts_ms)?;
        let mut mids_by_venue: HashMap<String, Vec<(i64, f64)>> = HashMap::new();
        for s in &samples {
            if s.mid.is_finite() && s.mid > 0.0 {
                mids_by_venue
                    .entry(s.venue_id.clone())
                    .or_default()
                    .push((s.ts_ms, s.mid));
            }
        }
        let venues: Vec<String> = mids_by_venue.keys().cloned().collect();
        if venues.len() < 2 {
            return Ok(0);
        }
        let mut all: Vec<PairObservation> = Vec::new();
        for a in &venues {
            for b in &venues {
                if a == b {
                    continue;
                }
                if let (Some(ma), Some(mb)) = (mids_by_venue.get(a), mids_by_venue.get(b)) {
                    all.extend(build_pair_series(ma, mb, 30_000));
                }
            }
        }
        if all.is_empty() {
            return Ok(0);
        }
        // Episode segmentation runs on absolute spreads against the global
        // median magnitude: session profiles calibrate how far the spread
        // stretches per regime and how fast magnitude reverts. Signed
        // directions would cancel across ordered pairs and fuse the whole
        // window into one open episode.
        let mut all_abs: Vec<PairObservation> = all
            .iter()
            .map(|o| PairObservation {
                ts_ms: o.ts_ms,
                spread_bps: o.spread_bps.abs(),
            })
            .collect();
        all_abs.sort_by_key(|o| o.ts_ms);
        let abs_vals: Vec<f64> = all_abs.iter().map(|o| o.spread_bps).collect();
        let global_med = match winsorized_median(&abs_vals) {
            Some(g) if g.is_finite() => g,
            _ => return Ok(0),
        };
        let spreads: Vec<f64> = abs_vals.clone();
        let n_all = spreads.len();
        let sigma_abs = if n_all >= 2 {
            let v = spreads.iter().map(|x| (x - global_med).powi(2)).sum::<f64>()
                / (n_all - 1) as f64;
            if v.is_finite() && v >= 0.0 {
                v.sqrt()
            } else {
                0.0
            }
        } else {
            0.0
        };
        let have_moments = n_all >= 2 && sigma_abs.is_finite() && sigma_abs > 0.0;

        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut written = 0usize;
        // Per-session aggregates first so hour bins can reference them.
        let mut session_shrunk: HashMap<String, f64> = HashMap::new();
        {
            let mut upsert = tx.prepare(
                "INSERT OR REPLACE INTO session_profiles
                 (symbol, session_id, n_episodes, med_abs_spread_bps, p90_spread_bps,
                  rv_bps, win_rate, mean_hold_s, provisional, updated_ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            for sess in SessionId::all() {
                let sub: Vec<PairObservation> = all_abs
                    .iter()
                    .filter(|o| session_of(o.ts_ms) == sess)
                    .cloned()
                    .collect();
                let abs_vals: Vec<f64> = sub.iter().map(|o| o.spread_bps).collect();
                let stats = session_stats(&abs_vals, global_med);
                let sid = sess.as_str().to_string();
                if sub.is_empty() {
                    upsert.execute(params![
                        canonical,
                        sid,
                        0,
                        Option::<f64>::None,
                        Option::<f64>::None,
                        Option::<f64>::None,
                        Option::<f64>::None,
                        Option::<f64>::None,
                        1,
                        now_ms
                    ])?;
                    written += 1;
                    continue;
                }
                let (n_episodes, win_rate, mean_hold_s) = if have_moments {
                    let rs = reversion_stats(&sub, global_med, sigma_abs, 2.0, 2.0, 3.0, 1);
                    (
                        rs.episodes as i64,
                        rs.p_win,
                        rs.median_converge_secs,
                    )
                } else {
                    (0i64, None, None)
                };
                let p90 = percentile_sorted(
                    &{
                        let mut v = abs_vals.clone();
                        v.sort_by(|a, b| {
                            a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                        });
                        v
                    },
                    0.90,
                );
                let rv = if sub.len() >= 2 {
                    let v = sub
                        .iter()
                        .map(|o| (o.spread_bps - global_med).powi(2))
                        .sum::<f64>()
                        / (sub.len() - 1) as f64;
                    if v.is_finite() && v >= 0.0 {
                        Some(v.sqrt())
                    } else {
                        None
                    }
                } else {
                    None
                };
                let provisional = (n_episodes as usize) < SESSION_MIN_EPISODES;
                if let Some(shrunk) = stats.shrunk_med {
                    if shrunk.is_finite() {
                        session_shrunk.insert(sid.clone(), shrunk);
                    }
                }
                upsert.execute(params![
                    canonical,
                    sid,
                    n_episodes,
                    stats.shrunk_med,
                    p90,
                    rv,
                    win_rate,
                    mean_hold_s,
                    if provisional { 1 } else { 0 },
                    now_ms
                ])?;
                written += 1;
            }
        }
        {
            let mut upsert = tx.prepare(
                "INSERT OR REPLACE INTO session_hour_bins
                 (symbol, hour, n_obs, med_spread_bps, modulator, provisional, updated_ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for hour in 0..24i64 {
                let vals: Vec<f64> = all_abs
                    .iter()
                    .filter(|o| hour_of_day_utc(o.ts_ms) == Some(hour as u32))
                    .map(|o| o.spread_bps)
                    .collect();
                let med = winsorized_median(&vals);
                let session_of_hour = SessionId::all().into_iter().find(|s| {
                    let open = s.open_hour_utc() as i64;
                    let next = match s {
                        SessionId::Asia => 8,
                        SessionId::Europe => 13,
                        SessionId::Overlap => 17,
                        SessionId::UsLate => 22,
                        SessionId::Dead => 24,
                    };
                    hour >= open && hour < next
                });
                let modulator = match (
                    med,
                    session_of_hour
                        .as_ref()
                        .and_then(|s| session_shrunk.get(s.as_str())),
                ) {
                    (Some(m), Some(sm)) => hour_modulator(m, *sm),
                    _ => 1.0,
                };
                let provisional = vals.len() < SESSION_MIN_EPISODES;
                let modulator = if provisional { 1.0 } else { modulator };
                upsert.execute(params![
                    canonical,
                    hour,
                    vals.len() as i64,
                    med,
                    modulator,
                    if provisional { 1 } else { 0 },
                    now_ms
                ])?;
            }
        }
        tx.commit()?;
        Ok(written)
    }

    pub fn phase2_har_anchor(&self, symbol: &str) -> anyhow::Result<Option<HarAnchorRow>> {
        let symbol = normalize_symbol(symbol)?;
        let keys = spread_symbol_aliases(&symbol);
        let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT symbol, intercept, b_daily, b_weekly, b_monthly, r2, n_days,
                    forecast_rv_bps, trailing_mean_rv_bps, updated_ts
             FROM har_anchors WHERE symbol IN ({placeholders}) LIMIT 1"
        );
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            keys.iter().map(|k| k as &dyn rusqlite::ToSql).collect();
        let row = stmt
            .query_map(rusqlite::params_from_iter(params), |r| {
                Ok(HarAnchorRow {
                    symbol: r.get(0)?,
                    intercept: r.get(1)?,
                    b_daily: r.get(2)?,
                    b_weekly: r.get(3)?,
                    b_monthly: r.get(4)?,
                    r2: r.get(5)?,
                    n_days: r.get(6)?,
                    forecast_rv_bps: r.get(7)?,
                    trailing_mean_rv_bps: r.get(8)?,
                    updated_ts: r.get(9)?,
                })
            })?
            .next()
            .transpose()?;
        Ok(row)
    }

    pub fn phase2_upsert_har_anchor(&self, row: &HarAnchorRow) -> anyhow::Result<()> {
        let symbol = canonical_base(&row.symbol);
        if symbol.is_empty() {
            anyhow::bail!("invalid symbol: expected 1-64 non-control characters");
        }
        for (name, v) in [
            ("intercept", row.intercept),
            ("b_daily", row.b_daily),
            ("b_weekly", row.b_weekly),
            ("b_monthly", row.b_monthly),
        ] {
            if !v.is_finite() {
                anyhow::bail!("non-finite HAR anchor {name}");
            }
        }
        if !(0.0..=1.0).contains(&row.r2) || !row.r2.is_finite() {
            anyhow::bail!("HAR anchor r2 outside 0..=1");
        }
        if row.n_days < 0 || row.updated_ts < 0 {
            anyhow::bail!("negative HAR anchor day count or timestamp");
        }
        if !(row.forecast_rv_bps.is_finite()
            && row.forecast_rv_bps > 0.0
            && row.trailing_mean_rv_bps.is_finite()
            && row.trailing_mean_rv_bps > 0.0)
        {
            anyhow::bail!("non-positive HAR anchor forecast or trailing mean");
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO har_anchors
                 (symbol, intercept, b_daily, b_weekly, b_monthly, r2, n_days,
                  forecast_rv_bps, trailing_mean_rv_bps, updated_ts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(symbol) DO UPDATE SET
                 intercept = ?2, b_daily = ?3, b_weekly = ?4, b_monthly = ?5,
                 r2 = ?6, n_days = ?7, forecast_rv_bps = ?8,
                 trailing_mean_rv_bps = ?9, updated_ts = ?10",
            params![
                symbol,
                row.intercept,
                row.b_daily,
                row.b_weekly,
                row.b_monthly,
                row.r2,
                row.n_days,
                row.forecast_rv_bps,
                row.trailing_mean_rv_bps,
                row.updated_ts
            ],
        )?;
        Ok(())
    }

    pub fn phase2_promotion_state(
        &self,
        symbol: &str,
    ) -> anyhow::Result<Option<PromotionStateRow>> {
        let symbol = normalize_symbol(symbol)?;
        let keys = spread_symbol_aliases(&symbol);
        let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT symbol, status, fit_delta_bps, val_delta_bps, reason, updated_ts
             FROM promotion_state WHERE symbol IN ({placeholders}) LIMIT 1"
        );
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            keys.iter().map(|k| k as &dyn rusqlite::ToSql).collect();
        let row = stmt
            .query_map(rusqlite::params_from_iter(params), |r| {
                Ok(PromotionStateRow {
                    symbol: r.get(0)?,
                    status: r.get(1)?,
                    fit_delta_bps: r.get(2)?,
                    val_delta_bps: r.get(3)?,
                    reason: r.get(4)?,
                    updated_ts: r.get(5)?,
                })
            })?
            .next()
            .transpose()?;
        Ok(row)
    }

    pub fn phase2_upsert_promotion_state(&self, row: &PromotionStateRow) -> anyhow::Result<()> {
        let symbol = canonical_base(&row.symbol);
        if symbol.is_empty() {
            anyhow::bail!("invalid symbol: expected 1-64 non-control characters");
        }
        if !matches!(row.status.as_str(), "promoted" | "held" | "demoted") {
            anyhow::bail!("invalid promotion status '{}'", row.status);
        }
        for (name, v) in [
            ("fit_delta", row.fit_delta_bps),
            ("val_delta", row.val_delta_bps),
        ] {
            if !v.is_finite() {
                anyhow::bail!("non-finite promotion {name}");
            }
        }
        if row.reason.is_empty() || row.reason.len() > 512 || row.updated_ts < 0 {
            anyhow::bail!("invalid promotion reason or timestamp");
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO promotion_state
                 (symbol, status, fit_delta_bps, val_delta_bps, reason, updated_ts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(symbol) DO UPDATE SET
                 status = ?2, fit_delta_bps = ?3, val_delta_bps = ?4,
                 reason = ?5, updated_ts = ?6",
            params![
                symbol,
                row.status,
                row.fit_delta_bps,
                row.val_delta_bps,
                row.reason,
                row.updated_ts
            ],
        )?;
        Ok(())
    }

    pub fn record_autotune_apply_event(
        &self,
        symbol: &str,
        applied_json: &str,
        detail_json: &str,
        ts_ms: i64,
    ) -> anyhow::Result<i64> {
        let symbol = normalize_symbol(symbol)?;
        if ts_ms < 0 {
            anyhow::bail!("negative autotune apply timestamp");
        }
        for (name, j) in [("applied", applied_json), ("detail", detail_json)] {
            if j.len() < 2 || j.len() > 8192 {
                anyhow::bail!("autotune apply {name} snapshot has invalid size");
            }
            if serde_json::from_str::<serde_json::Value>(j).is_err() {
                anyhow::bail!("autotune apply {name} snapshot is not valid JSON");
            }
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO autotune_apply_events (symbol, applied_json, detail_json, ts_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![symbol, applied_json, detail_json, ts_ms],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn recent_autotune_apply_events(
        &self,
        symbol: &str,
        limit: i64,
    ) -> anyhow::Result<Vec<AutotuneApplyEventRow>> {
        let symbol = normalize_symbol(symbol)?;
        if limit <= 0 {
            anyhow::bail!("event limit must be positive");
        }
        let limit = limit.min(1000);
        let keys = spread_symbol_aliases(&symbol);
        let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT id, symbol, applied_json, detail_json, ts_ms
             FROM autotune_apply_events WHERE symbol IN ({placeholders})
             ORDER BY ts_ms DESC, id DESC LIMIT ?"
        );
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let mut binds: Vec<&dyn rusqlite::ToSql> =
            keys.iter().map(|k| k as &dyn rusqlite::ToSql).collect();
        binds.push(&limit);
        let rows = stmt.query_map(rusqlite::params_from_iter(binds), |r| {
            Ok(AutotuneApplyEventRow {
                id: r.get(0)?,
                symbol: r.get(1)?,
                applied_json: r.get(2)?,
                detail_json: r.get(3)?,
                ts_ms: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn kv_get(&self, key: &str) -> anyhow::Result<Option<String>> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row("SELECT v FROM kv WHERE k = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn kv_set(&self, key: &str, value: &str) -> anyhow::Result<()> {
        if key.is_empty() || key.len() > 128 {
            anyhow::bail!("invalid kv key");
        }
        if value.len() > 4096 {
            anyhow::bail!("invalid kv value");
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO kv (k, v) VALUES (?1, ?2)
             ON CONFLICT(k) DO UPDATE SET v = ?2",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    pub fn kv_delete(&self, key: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM kv WHERE k = ?1", [key])?;
        Ok(())
    }

    pub fn db_size_bytes(&self) -> anyhow::Result<u64> {
        let conn = self.conn.lock();
        let page_count: i64 =
            conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let page_size: i64 =
            conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        Ok((page_count.max(0) as u64) * (page_size.max(0) as u64))
    }

    pub fn delete_spread_samples_before(&self, retain_after_ts_ms: i64) -> anyhow::Result<usize> {
        let conn = self.conn.lock();
        Ok(conn.execute(
            "DELETE FROM spread_samples WHERE ts_ms < ?1",
            [retain_after_ts_ms],
        )?)
    }

    /// Record funding-rate prints observed by a scan. `samples` carries
    /// `(venue_id, symbol, rate_8h_normalized, interval_hours)`; timestamps
    /// are bucketed to the minute so repeat scans within a minute REPLACE
    /// rather than duplicate (PK covers venue/symbol/minute). Fail-soft on
    /// bad rows: they are skipped, never aborting the batch.
    pub fn record_funding_rate_samples(
        &self,
        ts_ms: i64,
        samples: &[(&str, &str, f64, i64)],
    ) -> anyhow::Result<()> {
        if ts_ms < 0 || samples.is_empty() {
            return Ok(());
        }
        let bucketed = ts_ms / 60_000 * 60_000;
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "INSERT OR REPLACE INTO funding_rate_samples
                 (venue_id, symbol, ts_ms, rate_8h, interval_hours)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for (venue_raw, symbol_raw, rate_8h, interval_hours) in samples {
            let Ok(venue) = normalize_venue(venue_raw) else {
                continue;
            };
            let Ok(symbol) = normalize_symbol(symbol_raw) else {
                continue;
            };
            if !rate_8h.is_finite() || *interval_hours < 1 {
                continue;
            }
            stmt.execute(rusqlite::params![
                venue,
                symbol,
                bucketed,
                rate_8h,
                interval_hours
            ])?;
        }
        Ok(())
    }

    /// Ordered `(ts_ms, rate_8h, interval_hours)` history for one
    /// venue+symbol window. Training substrate for funding-persistence /
    /// half-life modeling; no modeling happens here.
    pub fn funding_rate_series(
        &self,
        venue: &str,
        symbol: &str,
        from_ts_ms: i64,
        to_ts_ms: i64,
    ) -> anyhow::Result<Vec<(i64, f64, i64)>> {
        let venue = normalize_venue(venue)?;
        let symbol = normalize_symbol(symbol)?;
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT ts_ms, rate_8h, interval_hours FROM funding_rate_samples
             WHERE venue_id = ?1 AND symbol = ?2
               AND ts_ms >= ?3 AND ts_ms <= ?4
             ORDER BY ts_ms ASC",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![venue, symbol, from_ts_ms, to_ts_ms],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?, r.get::<_, i64>(2)?)),
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Bound the funding-rate log with the same day cutoff the retention
    /// pass computes for spread/volume samples.
    pub fn prune_funding_rate_samples(
        &self,
        retain_after_ts_ms: i64,
    ) -> anyhow::Result<usize> {
        let conn = self.conn.lock();
        Ok(conn.execute(
            "DELETE FROM funding_rate_samples WHERE ts_ms < ?1",
            [retain_after_ts_ms],
        )?)
    }

    pub fn delete_oldest_spread_samples(&self, limit: u64) -> anyhow::Result<usize> {
        let conn = self.conn.lock();
        Ok(conn.execute(
            "DELETE FROM spread_samples WHERE id IN (
                 SELECT id FROM spread_samples ORDER BY ts_ms ASC, id ASC LIMIT ?1
             )",
            [limit as i64],
        )?)
    }

    pub fn spread_sample_count(&self) -> anyhow::Result<u64> {
        let conn = self.conn.lock();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM spread_samples", [], |r| r.get(0))?;
        Ok(n.max(0) as u64)
    }

    /// Set or clear a ticker's monitor/strategy exemption. Exempt tickers are
    /// dropped from the spread-monitor watchlist; a row with `exempt = false`
    /// is deleted so the table only ever holds active exemptions.
    pub fn set_ticker_exempt(&self, symbol: &str, exempt: bool) -> anyhow::Result<()> {
        let symbol = normalize_symbol(symbol)?;
        let conn = self.conn.lock();
        if exempt {
            conn.execute(
                "INSERT INTO ticker_exemptions (symbol, exempt, updated_at)
                 VALUES (?1, 1, ?2)
                 ON CONFLICT(symbol) DO UPDATE SET exempt = 1, updated_at = ?2",
                params![symbol, Utc::now().to_rfc3339()],
            )?;
        } else {
            conn.execute("DELETE FROM ticker_exemptions WHERE symbol = ?1", [&symbol])?;
        }
        Ok(())
    }

    /// Symbols currently exempted from the monitor.
    pub fn exempt_tickers(&self) -> anyhow::Result<Vec<String>> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare("SELECT symbol FROM ticker_exemptions WHERE exempt = 1 ORDER BY symbol")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Remove every ticker exemption — the global "reveal all" control.
    pub fn clear_ticker_exemptions(&self) -> anyhow::Result<usize> {
        let conn = self.conn.lock();
        Ok(conn.execute("DELETE FROM ticker_exemptions", [])?)
    }

    /* ------------------------------------------------------------------ */
    /* Balance attribution: expected equity + external-delta events        */
    /* ------------------------------------------------------------------ */

    /* ------------------------------------------------------------------ */
    /* Margin utilisation snapshots (24h time-weighted average history)    */
    /* ------------------------------------------------------------------ */

    /// Append one margin-utilisation observation for a paper venue. The
    /// writer enforces the cadence cap (see `MARGIN_SNAPSHOT_MIN_INTERVAL_MS`
    /// in commands); this method validates and persists unconditionally.
    pub fn record_margin_snapshot(
        &self,
        venue: &str,
        account_id: &str,
        ts_ms: i64,
        equity: f64,
        used_margin: f64,
        rate_pct: f64,
    ) -> anyhow::Result<()> {
        let venue = normalize_venue(venue)?;
        let account_id = account_id.trim().to_lowercase();
        if account_id.len() > 128 {
            anyhow::bail!("margin snapshot account id must fit in 128 characters");
        }
        if ts_ms < 0 {
            anyhow::bail!("margin snapshot timestamp must be non-negative");
        }
        for (name, v) in [("equity", equity), ("used margin", used_margin), ("rate", rate_pct)] {
            if !v.is_finite() || v < 0.0 {
                anyhow::bail!("margin snapshot {name} must be finite and non-negative");
            }
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO margin_snapshots
                 (venue, account_id, ts_ms, equity, used_margin, rate_pct)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![venue, account_id, ts_ms, equity, used_margin, rate_pct],
        )?;
        Ok(())
    }

    /// `(ts_ms, rate_pct)` points for one venue at or after `since_ms`,
    /// oldest first. Callers cap the window; the table itself is bounded by
    /// the shared retention-days prune.
    pub fn list_margin_snapshots(
        &self,
        venue: &str,
        since_ms: i64,
    ) -> anyhow::Result<Vec<MarginSnapshot>> {
        let venue = normalize_venue(venue)?;
        if since_ms < 0 {
            anyhow::bail!("margin snapshot window start must be non-negative");
        }
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT venue, account_id, ts_ms, equity, used_margin, rate_pct
             FROM margin_snapshots WHERE venue = ?1 AND ts_ms >= ?2
             ORDER BY ts_ms ASC, id ASC",
        )?;
        let rows = stmt
            .query_map(params![venue, since_ms], |r| {
                Ok(MarginSnapshot {
                    venue: r.get(0)?,
                    account_id: r.get(1)?,
                    ts_ms: r.get(2)?,
                    equity: r.get(3)?,
                    used_margin: r.get(4)?,
                    rate_pct: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Delete snapshots older than `before_ms` (the shared retention-days
    /// cutoff). Returns rows removed.
    pub fn prune_margin_snapshots(&self, before_ms: i64) -> anyhow::Result<usize> {
        if before_ms < 0 {
            anyhow::bail!("margin snapshot prune cutoff must be non-negative");
        }
        let conn = self.conn.lock();
        Ok(conn.execute("DELETE FROM margin_snapshots WHERE ts_ms < ?1", [before_ms])?)
    }

    /// Stored baseline behind one paper account, if any.
    pub fn get_equity_expectation(
        &self,
        account_id: &str,
    ) -> anyhow::Result<Option<(f64, String, i64)>> {
        let account_id = normalize_id(account_id)?;
        let conn = self.conn.lock();
        Ok(conn
            .query_row(
                "SELECT expected, last_pos_sig, last_poll_ms
                 FROM paper_equity_expectations WHERE account_id = ?1",
                [&account_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?)
    }

    /// Persist (or refresh) one paper account's expected-equity baseline.
    pub fn set_equity_expectation(
        &self,
        account_id: &str,
        expected: f64,
        pos_sig: &str,
        poll_ms: i64,
    ) -> anyhow::Result<()> {
        let account_id = normalize_id(account_id)?;
        if !expected.is_finite() || expected < 0.0 {
            anyhow::bail!("expected equity must be finite and non-negative");
        }
        if poll_ms < 0 {
            anyhow::bail!("poll timestamp must be non-negative");
        }
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO paper_equity_expectations
                 (account_id, expected, last_pos_sig, last_poll_ms, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(account_id) DO UPDATE SET
                 expected = excluded.expected,
                 last_pos_sig = excluded.last_pos_sig,
                 last_poll_ms = excluded.last_poll_ms,
                 updated_at = excluded.updated_at",
            params![account_id, expected, pos_sig, poll_ms, now],
        )?;
        Ok(())
    }

    fn insert_equity_event(&self, draft: &EquityEventDraft) -> anyhow::Result<AccountEquityEvent> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO account_equity_events
                 (account_id, ts_ms, expected, actual, delta, kind, note)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                draft.account_id,
                draft.ts_ms,
                draft.expected,
                draft.actual,
                draft.delta,
                draft.kind,
                draft.note
            ],
        )?;
        let id = conn.last_insert_rowid();
        Ok(AccountEquityEvent {
            id,
            account_id: draft.account_id.clone(),
            ts_ms: draft.ts_ms,
            expected: draft.expected,
            actual: draft.actual,
            delta: draft.delta,
            kind: draft.kind.clone(),
            note: draft.note.clone(),
        })
    }

    /// Compare one poll of actual equity against the stored baseline and
    /// persist the outcome. A nonzero delta records an external-delta event
    /// and rebases the baseline to actual so total PnL never absorbs manual
    /// flows; a comparison gap records no event, only a coverage note.
    pub fn check_account_equity(
        &self,
        account_id: &str,
        actual: f64,
        pos_sig: &str,
        now_ms: i64,
    ) -> anyhow::Result<EquityCheckReport> {
        let account_id = normalize_id(account_id)?;
        if now_ms < 0 {
            anyhow::bail!("poll timestamp must be non-negative");
        }
        let prev = self.get_equity_expectation(&account_id)?;
        let gap_ms = prev
            .as_ref()
            .map(|(_, _, last_poll)| now_ms.saturating_sub(*last_poll));
        let expected = prev.as_ref().map(|(e, _, _)| *e);
        let positions_changed = prev
            .as_ref()
            .is_some_and(|(_, sig, _)| sig.as_str() != pos_sig);
        let positions_open = !pos_sig.is_empty();
        let outcome =
            crate::equity_attribution::check_equity_delta(expected, actual, positions_changed, positions_open, gap_ms);
        if outcome.rebased && actual.is_finite() {
            self.set_equity_expectation(&account_id, actual, pos_sig, now_ms)?;
        } else if prev.is_some() {
            let keep = expected.unwrap_or(actual);
            if keep.is_finite() {
                self.set_equity_expectation(&account_id, keep, pos_sig, now_ms)?;
            }
        }
        let event = match outcome.event_kind {
            Some(kind) => Some(self.insert_equity_event(&EquityEventDraft {
                account_id: account_id.clone(),
                ts_ms: now_ms,
                expected: expected.unwrap_or(actual),
                actual,
                delta: outcome.delta,
                kind,
                note: outcome.note.clone().unwrap_or_default(),
            })?),
            None => None,
        };
        Ok(EquityCheckReport {
            event,
            note: outcome.note,
        })
    }

    /// Recent external-delta events, newest first. `limit` clamps to 1..=500.
    pub fn list_equity_events(
        &self,
        account_id: Option<&str>,
        limit: u32,
    ) -> anyhow::Result<Vec<AccountEquityEvent>> {
        let limit = limit.clamp(1, 500) as i64;
        let conn = self.conn.lock();
        let mut out = Vec::new();
        if let Some(raw) = account_id {
            let account_id = normalize_id(raw)?;
            let mut stmt = conn.prepare(
                "SELECT id, account_id, ts_ms, expected, actual, delta, kind, note
                 FROM account_equity_events WHERE account_id = ?1
                 ORDER BY ts_ms DESC, id DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![account_id, limit], equity_event_from_row)?;
            for r in rows {
                out.push(r?);
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, account_id, ts_ms, expected, actual, delta, kind, note
                 FROM account_equity_events
                 ORDER BY ts_ms DESC, id DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map([limit], equity_event_from_row)?;
            for r in rows {
                out.push(r?);
            }
        }
        Ok(out)
    }

    /// Account tiers of enabled accounts on a venue — the venue's shared-IP
    /// rate budget is driven by the lowest tier present (Lighter applies the
    /// lowest limit across accounts on the same IP).
    pub fn venue_enabled_tiers(&self, venue: &str) -> anyhow::Result<Vec<String>> {
        let venue = normalize_venue(venue)?;
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT account_tier FROM exchange_accounts
             WHERE venue = ?1 AND enabled = 1",
        )?;
        let rows = stmt.query_map([&venue], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /* ------------------------------------------------------------------ */
    /* Spread monitor: persisted config + pair-signal lifecycle            */
    /* ------------------------------------------------------------------ */

    /// Load the monitor config JSON blob, if one was saved.
    pub fn monitor_config_json(&self) -> anyhow::Result<Option<String>> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row(
                "SELECT config_json FROM monitor_config WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Persist the monitor config JSON blob (single row, upserted).
    pub fn save_monitor_config_json(&self, json: &str) -> anyhow::Result<()> {
        if json.is_empty() || json.len() > 64 * 1024 {
            anyhow::bail!("invalid monitor config payload");
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO monitor_config (id, config_json, updated_at)
             VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET config_json = ?1, updated_at = ?2",
            params![json, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn radar_bot_config_json(&self) -> anyhow::Result<Option<String>> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row(
                "SELECT config_json FROM radar_bot_config WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn save_radar_bot_config_json(&self, json: &str) -> anyhow::Result<()> {
        if json.is_empty() || json.len() > 64 * 1024 {
            anyhow::bail!("invalid radar bot config payload");
        }
        if serde_json::from_str::<serde_json::Value>(json).is_err() {
            anyhow::bail!("radar bot config payload is not valid JSON");
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO radar_bot_config (id, config_json, updated_at)
             VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET config_json = ?1, updated_at = ?2",
            params![json, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn count_bot_decisions_since(&self, since_ts_ms: i64) -> anyhow::Result<Vec<(String, i64)>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT new_status, COUNT(*) FROM monitor_signal_events
             WHERE trigger = 'bot_decision' AND ts_ms >= ?1
             GROUP BY new_status",
        )?;
        let rows = stmt.query_map([since_ts_ms], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_bot_position(&self, row: &BotPositionRow) -> anyhow::Result<()> {
        let symbol = normalize_symbol(&row.symbol)?;
        let buy_venue = normalize_venue(&row.buy_venue)?;
        let sell_venue = normalize_venue(&row.sell_venue)?;
        if row.entry_ts_ms < 0 || row.deadline_ms < 0 || row.last_seen_ms < 0 {
            anyhow::bail!("negative bot position timestamp");
        }
        if let Some(u) = row.unwatched_since_ms {
            if u < 0 {
                anyhow::bail!("negative bot position unwatched timestamp");
            }
        }
        for (name, v) in [
            ("entry_dev", row.entry_dev_bps),
            ("friction_band", row.friction_band_bps),
            ("size_usd", row.size_usd),
        ] {
            if !v.is_finite() {
                anyhow::bail!("non-finite bot position {name}");
            }
        }
        if row.friction_band_bps < 0.0 || row.size_usd <= 0.0 {
            anyhow::bail!("invalid bot position band/size");
        }
        if let Some(b) = row.baseline_bps {
            if !b.is_finite() {
                anyhow::bail!("non-finite bot position baseline");
            }
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO radar_bot_positions
                 (symbol, buy_venue, sell_venue, entry_ts_ms, entry_dev_bps,
                  baseline_bps, friction_band_bps, size_usd, deadline_ms,
                  unwatched_since_ms, last_seen_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(symbol, buy_venue, sell_venue) DO UPDATE SET
                 entry_ts_ms = ?4, entry_dev_bps = ?5, baseline_bps = ?6,
                 friction_band_bps = ?7, size_usd = ?8, deadline_ms = ?9,
                 unwatched_since_ms = ?10, last_seen_ms = ?11",
            params![
                symbol,
                buy_venue,
                sell_venue,
                row.entry_ts_ms,
                row.entry_dev_bps,
                row.baseline_bps,
                row.friction_band_bps,
                row.size_usd,
                row.deadline_ms,
                row.unwatched_since_ms,
                row.last_seen_ms
            ],
        )?;
        Ok(())
    }

    pub fn delete_bot_position(
        &self,
        symbol: &str,
        buy_venue: &str,
        sell_venue: &str,
    ) -> anyhow::Result<()> {
        let symbol = normalize_symbol(symbol)?;
        let buy_venue = normalize_venue(buy_venue)?;
        let sell_venue = normalize_venue(sell_venue)?;
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM radar_bot_positions WHERE symbol = ?1 AND buy_venue = ?2 AND sell_venue = ?3",
            params![symbol, buy_venue, sell_venue],
        )?;
        Ok(())
    }

    pub fn list_bot_positions(&self) -> anyhow::Result<Vec<BotPositionRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT symbol, buy_venue, sell_venue, entry_ts_ms, entry_dev_bps,
                    baseline_bps, friction_band_bps, size_usd, deadline_ms,
                    unwatched_since_ms, last_seen_ms
             FROM radar_bot_positions ORDER BY entry_ts_ms, symbol",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(BotPositionRow {
                symbol: r.get(0)?,
                buy_venue: r.get(1)?,
                sell_venue: r.get(2)?,
                entry_ts_ms: r.get(3)?,
                entry_dev_bps: r.get(4)?,
                baseline_bps: r.get(5)?,
                friction_band_bps: r.get(6)?,
                size_usd: r.get(7)?,
                deadline_ms: r.get(8)?,
                    unwatched_since_ms: r.get(9)?,
                    last_seen_ms: r.get(10)?,
                })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn record_spatial_baseline(&self, row: &SpatialEntryBaseline) -> anyhow::Result<()> {
        let signal_id = normalize_id(&row.signal_id)?;
        let symbol = normalize_symbol(&row.symbol)?;
        let buy_venue = normalize_venue(&row.buy_venue)?;
        let sell_venue = normalize_venue(&row.sell_venue)?;
        if !row.entry_net_bps.is_finite() {
            anyhow::bail!("non-finite spatial entry edge");
        }
        if let Some(b) = row.baseline_bps {
            if !b.is_finite() {
                anyhow::bail!("non-finite spatial entry baseline");
            }
        }
        if row.entry_ts_ms < 0 {
            anyhow::bail!("negative spatial entry timestamp");
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO spatial_entry_baseline
                 (signal_id, symbol, buy_venue, sell_venue,
                  entry_net_bps, baseline_bps, entry_ts_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(signal_id) DO UPDATE SET
                 symbol = ?2, buy_venue = ?3, sell_venue = ?4,
                 entry_net_bps = ?5, baseline_bps = ?6, entry_ts_ms = ?7",
            params![
                signal_id,
                symbol,
                buy_venue,
                sell_venue,
                row.entry_net_bps,
                row.baseline_bps,
                row.entry_ts_ms
            ],
        )?;
        Ok(())
    }

    pub fn spatial_baseline_for_signal(
        &self,
        signal_id: &str,
    ) -> anyhow::Result<Option<SpatialEntryBaseline>> {
        let signal_id = normalize_id(signal_id)?;
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT signal_id, symbol, buy_venue, sell_venue,
                    entry_net_bps, baseline_bps, entry_ts_ms
             FROM spatial_entry_baseline WHERE signal_id = ?1",
            [&signal_id],
            |r| {
                Ok(SpatialEntryBaseline {
                    signal_id: r.get(0)?,
                    symbol: r.get(1)?,
                    buy_venue: r.get(2)?,
                    sell_venue: r.get(3)?,
                    entry_net_bps: r.get(4)?,
                    baseline_bps: r.get(5)?,
                    entry_ts_ms: r.get(6)?,
                })
            },
        )
        .optional()
        .map_err(anyhow::Error::from)
    }

    /// Upsert one pair signal's lifecycle state.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_monitor_signal(
        &self,
        symbol: &str,
        buy_venue: &str,
        sell_venue: &str,
        status: &str,
        first_seen_ms: i64,
        last_seen_ms: i64,
        last_edge_bps: Option<f64>,
        last_dev_bps: Option<f64>,
    ) -> anyhow::Result<()> {
        let symbol = normalize_symbol(symbol)?;
        let buy_venue = normalize_venue(buy_venue)?;
        let sell_venue = normalize_venue(sell_venue)?;
        match status {
            "preparing" | "tracking" | "triggered" | "ready" | "no-data" => {}
            other => anyhow::bail!("invalid monitor signal status '{other}'"),
        }
        if first_seen_ms < 0 || last_seen_ms < 0 {
            anyhow::bail!("negative signal timestamp");
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO monitor_signals
                 (symbol, buy_venue, sell_venue, status, first_seen_ms, last_seen_ms,
                  last_edge_bps, last_dev_bps, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(symbol, buy_venue, sell_venue) DO UPDATE SET
                 status = ?4, first_seen_ms = ?5, last_seen_ms = ?6,
                 last_edge_bps = ?7, last_dev_bps = ?8, updated_at = ?9",
            params![
                symbol,
                buy_venue,
                sell_venue,
                status,
                first_seen_ms,
                last_seen_ms,
                last_edge_bps,
                last_dev_bps,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    /// Load every persisted pair signal's lifecycle state.
    pub fn monitor_signals(&self) -> anyhow::Result<Vec<MonitorSignalRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT symbol, buy_venue, sell_venue, status, first_seen_ms, last_seen_ms,
                    last_edge_bps, last_dev_bps
             FROM monitor_signals",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(MonitorSignalRow {
                symbol: r.get(0)?,
                buy_venue: r.get(1)?,
                sell_venue: r.get(2)?,
                status: r.get(3)?,
                first_seen_ms: r.get(4)?,
                last_seen_ms: r.get(5)?,
                last_edge_bps: r.get(6)?,
                last_dev_bps: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Drop signal rows whose pair is no longer tracked (venue or symbol
    /// left the watchlist) so stale lifecycle state can't resurface.
    pub fn prune_monitor_signals(&self, keep: &[(String, String, String)]) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        let mut all: Vec<(String, String, String)> = {
            let mut s =
                conn.prepare("SELECT symbol, buy_venue, sell_venue FROM monitor_signals")?;
            let rows = s
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        all.retain(|k| {
            !keep
                .iter()
                .any(|(s, b, sv)| s == &k.0 && b == &k.1 && sv == &k.2)
        });
        let mut del = conn.prepare(
            "DELETE FROM monitor_signals WHERE symbol = ?1 AND buy_venue = ?2 AND sell_venue = ?3",
        )?;
        for k in all {
            del.execute(params![k.0, k.1, k.2])?;
        }
        Ok(())
    }

    /// Append one lifecycle audit event. `old_status` is `None` on first
    /// sighting; `trigger` is `transition` when the status changed and
    /// `heartbeat` for the minute-bucketed still-active re-assertion.
    /// Non-finite metric snapshots are rejected rather than persisted as
    /// plausible numbers.
    #[allow(clippy::too_many_arguments)]
    pub fn record_monitor_signal_event(
        &self,
        symbol: &str,
        buy_venue: &str,
        sell_venue: &str,
        ts_ms: i64,
        old_status: Option<&str>,
        new_status: &str,
        edge_bps: Option<f64>,
        dev_bps: Option<f64>,
        p_win: Option<f64>,
        economics_json: Option<&str>,
        trigger: &str,
    ) -> anyhow::Result<()> {
        let symbol = normalize_symbol(symbol)?;
        let buy_venue = normalize_venue(buy_venue)?;
        let sell_venue = normalize_venue(sell_venue)?;
        if ts_ms < 0 {
            anyhow::bail!("negative signal event timestamp");
        }
        const LIFECYCLE: [&str; 5] = ["preparing", "tracking", "triggered", "ready", "no-data"];
        const DECISIONS: [&str; 3] = ["bot_enter", "bot_skip", "bot_exit"];
        if let Some(old) = old_status {
            if !LIFECYCLE.contains(&old) {
                anyhow::bail!("invalid old monitor signal status '{old}'");
            }
        }
        if !(LIFECYCLE.contains(&new_status) || DECISIONS.contains(&new_status)) {
            anyhow::bail!("invalid new monitor signal status '{new_status}'");
        }
        if !matches!(trigger, "transition" | "heartbeat" | "bot_decision") {
            anyhow::bail!("invalid signal event trigger '{trigger}'");
        }
        for (name, v) in [("edge", edge_bps), ("dev", dev_bps), ("p_win", p_win)] {
            if let Some(x) = v {
                if !x.is_finite() {
                    anyhow::bail!("non-finite signal event {name}");
                }
            }
        }
        if let Some(p) = p_win {
            if !(0.0..=1.0).contains(&p) {
                anyhow::bail!("signal event p_win outside 0..=1");
            }
        }
        if let Some(j) = economics_json {
            if j.len() > 8192 {
                anyhow::bail!("signal event economics snapshot too large");
            }
            if !j.is_empty() && serde_json::from_str::<serde_json::Value>(j).is_err() {
                anyhow::bail!("signal event economics snapshot is not valid JSON");
            }
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO monitor_signal_events
                 (symbol, buy_venue, sell_venue, ts_ms, old_status, new_status,
                  edge_bps, dev_bps, p_win, economics_json, trigger)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                symbol,
                buy_venue,
                sell_venue,
                ts_ms,
                old_status,
                new_status,
                edge_bps,
                dev_bps,
                p_win,
                economics_json,
                trigger
            ],
        )?;
        Ok(())
    }

    /// Recent audit events for one pair (or every pair when the key is
    /// `None`), newest first, bounded by `limit`. Fail-closed read path for
    /// the future bot + debugging IPC.
    pub fn recent_monitor_signal_events(
        &self,
        symbol: Option<&str>,
        buy_venue: Option<&str>,
        sell_venue: Option<&str>,
        limit: i64,
    ) -> anyhow::Result<Vec<MonitorSignalEventRow>> {
        if limit <= 0 {
            anyhow::bail!("event limit must be positive");
        }
        let limit = limit.min(10_000);
        let mut sql = String::from(
            "SELECT id, symbol, buy_venue, sell_venue, ts_ms, old_status, new_status,
                    edge_bps, dev_bps, p_win, economics_json, trigger
             FROM monitor_signal_events WHERE 1=1",
        );
        let mut binds: Vec<String> = Vec::new();
        if let Some(s) = symbol {
            sql.push_str(" AND symbol = ?");
            binds.push(normalize_symbol(s)?);
        }
        if let Some(b) = buy_venue {
            sql.push_str(" AND buy_venue = ?");
            binds.push(normalize_venue(b)?);
        }
        if let Some(s) = sell_venue {
            sql.push_str(" AND sell_venue = ?");
            binds.push(normalize_venue(s)?);
        }
        sql.push_str(" ORDER BY ts_ms DESC, id DESC LIMIT ?");
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            binds.iter().map(|b| b as &dyn rusqlite::types::ToSql).collect();
        let mut all_refs: Vec<&dyn rusqlite::types::ToSql> = refs;
        all_refs.push(&limit);
        let rows = stmt.query_map(rusqlite::params_from_iter(all_refs), |r| {
            Ok(MonitorSignalEventRow {
                id: r.get(0)?,
                symbol: r.get(1)?,
                buy_venue: r.get(2)?,
                sell_venue: r.get(3)?,
                ts_ms: r.get(4)?,
                old_status: r.get(5)?,
                new_status: r.get(6)?,
                edge_bps: r.get(7)?,
                dev_bps: r.get(8)?,
                p_win: r.get(9)?,
                economics_json: r.get(10)?,
                trigger: r.get(11)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn monitor_signal_event_count(&self) -> anyhow::Result<u64> {
        let conn = self.conn.lock();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM monitor_signal_events", [], |r| {
            r.get(0)
        })?;
        Ok(n.max(0) as u64)
    }

    /// Bound the audit log with the shared spread-sample day cutoff plus the
    /// independent `MONITOR_SIGNAL_EVENTS_MAX_ROWS` cap (newest rows survive).
    /// Derived session profiles are never touched here.
    pub fn prune_monitor_signal_events(
        &self,
        retain_after_ts_ms: i64,
        max_rows: i64,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM monitor_signal_events WHERE ts_ms < ?1",
            [retain_after_ts_ms],
        )?;
        conn.execute(
            "DELETE FROM monitor_signal_events WHERE id IN (
                 SELECT id FROM monitor_signal_events
                 ORDER BY ts_ms DESC, id DESC LIMIT -1 OFFSET ?1
             )",
            [max_rows],
        )?;
        Ok(())
    }

    fn legs_for(conn: &Connection, signal_id: &str) -> rusqlite::Result<Vec<SignalLeg>> {
        let mut stmt = conn.prepare(
            "SELECT id, signal_id, account_id, venue, side, intended_qty,
                    filled_qty, fill_price, fee_usd, venue_order_id,
                    created_at, updated_at
             FROM signal_legs WHERE signal_id = ?1
             ORDER BY CASE side WHEN 'long' THEN 0 ELSE 1 END",
        )?;
        let legs = stmt
            .query_map([&signal_id], leg_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        Ok(legs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store_path(tag: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "arbitrageur_state_store_test_{}_{}",
            tag,
            rand::random::<u64>()
        ));
        let file = dir.join("state.sqlite3");
        (dir, file)
    }

    #[test]
    fn test_file_store_config_and_permissions() {
        let (dir, file) = temp_store_path("config");
        let store = StateStore::open(&file).unwrap();
        {
            let conn = store.conn.lock();
            let version: i64 = conn
                .query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap();
            assert_eq!(version, 19);
            let mode: String = conn
                .query_row("PRAGMA journal_mode", [], |r| r.get(0))
                .unwrap();
            assert_eq!(mode, "wal");
            let fk: i64 = conn
                .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
                .unwrap();
            assert_eq!(fk, 1);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir_mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(dir_mode, 0o700);
            let file_mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
            assert_eq!(file_mode, 0o600);
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_strategy_params_json_roundtrip_and_reopen() {
        let (dir, file) = temp_store_path("strategy_params");
        {
            let store = StateStore::open(&file).unwrap();
            store
                .upsert_strategy_params_json("futures", "{\"strategyId\":\"futures\"}")
                .unwrap();
            store
                .upsert_strategy_params_json("futures", "{\"strategyId\":\"futures\",\"v\":2}")
                .unwrap();
            store
                .upsert_strategy_params_json("spot", "{\"strategyId\":\"spot\"}")
                .unwrap();
            assert!(
                store.upsert_strategy_params_json("bogus", "{}").is_err(),
                "unknown strategy id must fail the CHECK constraint"
            );
        }
        {
            let reopened = StateStore::open(&file).unwrap();
            let rows = reopened.strategy_params_json().unwrap();
            assert_eq!(
                rows,
                vec![
                    (
                        "futures".to_string(),
                        "{\"strategyId\":\"futures\",\"v\":2}".to_string()
                    ),
                    ("spot".to_string(), "{\"strategyId\":\"spot\"}".to_string()),
                ]
            );
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_funding_rate_samples_roundtrip_and_minute_bucket() {
        let store = StateStore::open_in_memory().unwrap();
        // Two writes inside the same minute collapse to one row (REPLACE).
        store
            .record_funding_rate_samples(
                1_700_000_000_012,
                &[("lighter", "BTC-USDC", 0.0008, 8)],
            )
            .unwrap();
        store
            .record_funding_rate_samples(
                1_700_000_000_045,
                &[("lighter", "BTC-USDC", 0.0009, 8)],
            )
            .unwrap();
        // A later minute appends.
        store
            .record_funding_rate_samples(
                1_700_000_060_000,
                &[("lighter", "BTC-USDC", 0.001, 8)],
            )
            .unwrap();
        let series = store
            .funding_rate_series("lighter", "BTC-USDC", 0, i64::MAX)
            .unwrap();
        assert_eq!(series.len(), 2, "got {series:?}");
        assert_eq!(series[0].0, 1_700_000_000_012 / 60_000 * 60_000);
        assert!((series[0].1 - 0.0009).abs() < 1e-12);
        assert_eq!(series[0].2, 8);
        // Bad rows never abort the batch.
        store
            .record_funding_rate_samples(
                1_700_000_120_000,
                &[("!!", "BTC-USDC", 0.001, 8), ("lighter", "BTC-USDC", f64::NAN, 8)],
            )
            .unwrap();
        let after = store
            .funding_rate_series("lighter", "BTC-USDC", 0, i64::MAX)
            .unwrap();
        assert_eq!(after.len(), 2);
        // Retention prune keeps only the fresh minute bucket (1_700_000_040_000).
        let removed = store
            .prune_funding_rate_samples(1_700_000_040_000)
            .unwrap();
        assert_eq!(removed, 1);
        let kept = store
            .funding_rate_series("lighter", "BTC-USDC", 0, i64::MAX)
            .unwrap();
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn test_default_account_lifecycle() {
        let store = StateStore::open_in_memory().unwrap();
        let first = store
            .upsert_account("hl_main", "HyperLiquid", "Main", true, false, "standard", None)
            .unwrap();
        assert!(first.is_default);
        assert!(store.has_enabled_account("hyperliquid").unwrap());
        assert!(!store.has_enabled_account("lighter").unwrap());

        let second = store
            .upsert_account("hl_alt", "hyperliquid", "Alt", true, false, "standard", None)
            .unwrap();
        assert!(!second.is_default);
        assert_eq!(
            store.default_account("hyperliquid").unwrap().unwrap().id,
            "hl_main"
        );

        store
            .upsert_account("hl_zulu", "hyperliquid", "Zulu", true, false, "standard", None)
            .unwrap();
        store.set_default_account("hyperliquid", "hl_zulu").unwrap();
        assert!(store.get_account("hl_zulu").unwrap().unwrap().is_default);
        assert!(!store.get_account("hl_main").unwrap().unwrap().is_default);

        store
            .upsert_account("hl_zulu", "hyperliquid", "Zulu", false, false, "standard", None)
            .unwrap();
        let promoted = store.default_account("hyperliquid").unwrap().unwrap();
        assert_eq!(promoted.id, "hl_alt");

        store
            .upsert_account("hl_alt", "hyperliquid", "Alt", false, false, "standard", None)
            .unwrap();
        store
            .upsert_account("hl_main", "hyperliquid", "Main", false, false, "standard", None)
            .unwrap();
        assert!(store.default_account("hyperliquid").unwrap().is_none());
        assert!(!store.has_enabled_account("hyperliquid").unwrap());

        let reenabled = store
            .upsert_account("hl_alt", "hyperliquid", "Alt", true, false, "standard", None)
            .unwrap();
        assert!(reenabled.is_default);
    }

    #[test]
    fn test_delete_default_promotes_alphabetically_first_enabled() {
        let store = StateStore::open_in_memory().unwrap();
        store
            .upsert_account("a_main", "hyperliquid", "Main", true, true, "standard", None)
            .unwrap();
        store
            .upsert_account("a_zulu", "hyperliquid", "Zulu", true, false, "standard", None)
            .unwrap();
        store
            .upsert_account("a_beta", "hyperliquid", "Beta", true, false, "standard", None)
            .unwrap();
        assert!(store.delete_account("a_main").unwrap());
        let promoted = store.default_account("hyperliquid").unwrap().unwrap();
        assert_eq!(promoted.id, "a_beta");
    }

    #[test]
    fn test_validation_and_immutable_venue() {
        let store = StateStore::open_in_memory().unwrap();
        assert!(store
            .upsert_account("", "hyperliquid", "Main", true, false, "standard", None)
            .is_err());
        assert!(store
            .upsert_account("bad id", "hyperliquid", "Main", true, false, "standard", None)
            .is_err());
        assert!(store
            .upsert_account(
                &"x".repeat(129),
                "hyperliquid",
                "Main",
                true,
                false,
                "standard",
                None
            )
            .is_err());
        assert!(store
            .upsert_account("a", "", "Main", true, false, "standard", None)
            .is_err());
        assert!(store
            .upsert_account("a", "bad venue", "Main", true, false, "standard", None)
            .is_err());
        assert!(store
            .upsert_account("a", "hyperliquid", "", true, false, "standard", None)
            .is_err());
        assert!(store
            .upsert_account("a", "hyperliquid", &"y".repeat(81), true, false, "standard", None)
            .is_err());
        assert!(store
            .upsert_account(
                "a",
                "hyperliquid",
                "bad\u{0007}label",
                true,
                false,
                "standard",
                None
            )
            .is_err());

        store
            .upsert_account("acct1", "hyperliquid", "Main", true, false, "standard", None)
            .unwrap();
        assert!(store
            .upsert_account("acct1", "lighter", "Moved", true, false, "standard", None)
            .is_err());
        let renamed = store
            .upsert_account("acct1", "HyperLiquid", "Renamed", true, false, "standard", None)
            .unwrap();
        assert_eq!(renamed.venue, "hyperliquid");
        assert_eq!(renamed.label, "Renamed");
    }

    #[test]
    fn test_label_uniqueness_scoped_to_venue() {
        let store = StateStore::open_in_memory().unwrap();
        store
            .upsert_account("a1", "hyperliquid", "Main", true, false, "standard", None)
            .unwrap();
        assert!(store
            .upsert_account("a2", "hyperliquid", "MAIN", true, false, "standard", None)
            .is_err());
        store
            .upsert_account("a2", "lighter", "main", true, false, "standard", None)
            .unwrap();
        assert_eq!(store.list_accounts(None).unwrap().len(), 2);
    }

    #[test]
    fn test_route_rules_and_canonicalization() {
        let store = StateStore::open_in_memory().unwrap();
        store
            .upsert_account("hl_a", "hyperliquid", "HL A", true, false, "standard", None)
            .unwrap();
        store
            .upsert_account("hl_b", "hyperliquid", "HL B", true, false, "standard", None)
            .unwrap();
        store
            .upsert_account("lt_a", "lighter", "LT A", true, false, "standard", None)
            .unwrap();
        store
            .upsert_account("lt_off", "lighter", "LT Off", false, false, "standard", None)
            .unwrap();

        assert!(store
            .upsert_strategy_route("r1", "unknown", "R", "hl_a", "lt_a", true)
            .is_err());
        assert!(store
            .upsert_strategy_route("r1", "futures", "R", "hl_a", "hl_a", true)
            .is_err());
        assert!(store
            .upsert_strategy_route("r1", "futures", "R", "hl_a", "hl_b", true)
            .is_err());
        assert!(store
            .upsert_strategy_route("r1", "futures", "R", "hl_a", "lt_off", true)
            .is_err());
        assert!(store
            .upsert_strategy_route("r1", "futures", "R", "missing", "lt_a", true)
            .is_err());

        let disabled_route = store
            .upsert_strategy_route("r_off", "futures", "Off", "hl_a", "lt_off", false)
            .unwrap();
        assert!(!disabled_route.enabled);

        let route = store
            .upsert_strategy_route("r1", "Futures", "Main Route", "lt_a", "hl_a", true)
            .unwrap();
        assert_eq!(route.account_a_id, "hl_a");
        assert_eq!(route.account_a_venue, "hyperliquid");
        assert_eq!(route.account_b_id, "lt_a");
        assert_eq!(route.account_b_venue, "lighter");
        assert_eq!(route.strategy_id, "futures");

        assert!(store
            .upsert_strategy_route("r2", "futures", "Dup", "hl_a", "lt_a", true)
            .is_err());
        assert!(store
            .upsert_strategy_route("r2", "futures", "Dup", "lt_a", "hl_a", true)
            .is_err());
        store
            .upsert_strategy_route("r2", "spot", "Other", "hl_a", "lt_a", true)
            .unwrap();
    }

    #[test]
    fn test_referenced_account_delete_requires_route_removal() {
        let store = StateStore::open_in_memory().unwrap();
        store
            .upsert_account("hl_a", "hyperliquid", "HL A", true, false, "standard", None)
            .unwrap();
        store
            .upsert_account("lt_a", "lighter", "LT A", true, false, "standard", None)
            .unwrap();
        store
            .upsert_strategy_route("r1", "futures", "Route", "hl_a", "lt_a", true)
            .unwrap();

        let err = store.delete_account("hl_a").unwrap_err();
        assert!(err.to_string().contains("strategy routes"));
        assert!(!store.delete_account("missing").unwrap());

        assert!(store.delete_strategy_route("r1").unwrap());
        assert!(!store.delete_strategy_route("r1").unwrap());
        assert!(store.delete_account("hl_a").unwrap());
    }

    #[test]
    fn test_eligible_strategy_routes_filters() {
        let store = StateStore::open_in_memory().unwrap();
        store
            .upsert_account("hl_a", "hyperliquid", "HL A", true, false, "standard", None)
            .unwrap();
        store
            .upsert_account("hl_b", "hyperliquid", "HL B", true, false, "standard", None)
            .unwrap();
        store
            .upsert_account("lt_a", "lighter", "LT A", true, false, "standard", None)
            .unwrap();
        store
            .upsert_account("lt_b", "lighter", "LT B", true, false, "standard", None)
            .unwrap();

        store
            .upsert_strategy_route("r1", "futures", "Active", "hl_a", "lt_a", true)
            .unwrap();
        store
            .upsert_strategy_route("r2", "futures", "Inactive", "hl_b", "lt_a", false)
            .unwrap();
        store
            .upsert_strategy_route("r3", "futures", "Stale", "hl_a", "lt_b", true)
            .unwrap();
        store
            .upsert_account("lt_b", "lighter", "LT B", false, false, "standard", None)
            .unwrap();

        let eligible = store
            .eligible_strategy_routes("futures", "hyperliquid", "lighter")
            .unwrap();
        assert_eq!(eligible.len(), 1);
        assert_eq!(eligible[0].id, "r1");

        let reversed = store
            .eligible_strategy_routes("futures", "lighter", "hyperliquid")
            .unwrap();
        assert_eq!(reversed.len(), 1);
        assert_eq!(reversed[0].id, "r1");

        assert!(store
            .eligible_strategy_routes("futures", "hyperliquid", "kraken")
            .unwrap()
            .is_empty());
        assert!(store
            .eligible_strategy_routes("spot", "hyperliquid", "lighter")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn test_file_store_persists_across_reopen() {
        let (dir, file) = temp_store_path("persist");
        {
            let store = StateStore::open(&file).unwrap();
            store
                .upsert_account("hl_a", "hyperliquid", "Main", true, false, "standard", None)
                .unwrap();
            store
                .upsert_account("lt_a", "lighter", "Alt", true, false, "standard", None)
                .unwrap();
            store
                .upsert_strategy_route("r1", "futures", "Route", "hl_a", "lt_a", true)
                .unwrap();
        }

        let store = StateStore::open(&file).unwrap();
        let accounts = store.list_accounts(None).unwrap();
        assert_eq!(accounts.len(), 2);
        assert!(store.get_account("hl_a").unwrap().unwrap().is_default);
        let routes = store.list_strategy_routes(Some("futures")).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].account_a_id, "hl_a");
        assert_eq!(store.list_strategy_routes(None).unwrap().len(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    fn two_leg_signal(
        id: &str,
        symbol: &str,
        acct_a: Option<&str>,
        acct_b: Option<&str>,
    ) -> NewSignalExecution {
        NewSignalExecution {
            id: id.to_string(),
            strategy_id: "futures".into(),
            symbol: symbol.to_string(),
            route_id: None,
            expected_net_usd: Some(12.5),
            note: None,
            legs: vec![
                NewSignalLeg {
                    account_id: acct_a.map(|s| s.to_string()),
                    venue: "hyperliquid".into(),
                    side: LegSide::Long,
                    intended_qty: 1.0,
                },
                NewSignalLeg {
                    account_id: acct_b.map(|s| s.to_string()),
                    venue: "lighter".into(),
                    side: LegSide::Short,
                    intended_qty: 1.0,
                },
            ],
        }
    }

    #[test]
    fn test_signal_lifecycle_blocks_then_frees_ticker() {
        let store = StateStore::open_in_memory().unwrap();
        store
            .upsert_account("hl_a", "hyperliquid", "Main", true, false, "standard", None)
            .unwrap();
        store
            .upsert_account("lt_a", "lighter", "Main", true, false, "standard", None)
            .unwrap();

        let sig = store
            .record_signal_execution(&two_leg_signal(
                "s1",
                "BTC-USDC",
                Some("hl_a"),
                Some("lt_a"),
            ))
            .unwrap();
        assert_eq!(sig.status, SignalStatus::InFlight);
        assert_eq!(sig.account_long_id.as_deref(), Some("hl_a"));
        assert_eq!(sig.account_short_id.as_deref(), Some("lt_a"));
        assert_eq!(sig.legs.len(), 2);
        assert!(store.ticker_blocked("btc-usdc").unwrap());

        // Full fills on both legs → per-signal delta reconciles to zero.
        store
            .apply_leg_fill("s1", LegSide::Long, 1.0, 60000.0, 1.2, Some("o-long"))
            .unwrap();
        store
            .apply_leg_fill("s1", LegSide::Short, 1.0, 60120.0, 1.3, Some("o-short"))
            .unwrap();
        assert!((store.signal_delta("s1").unwrap()).abs() < 1e-9);
        store
            .update_signal_status("s1", SignalStatus::Balanced, None)
            .unwrap();
        assert!(!store.ticker_blocked("BTC-USDC").unwrap());
        // The open position still contributes legs to per-venue/account nets.
        assert!((store.venue_symbol_net("hyperliquid", "BTC-USDC").unwrap() - 1.0).abs() < 1e-9);
        assert!((store.venue_symbol_net("lighter", "BTC-USDC").unwrap() + 1.0).abs() < 1e-9);
        assert!((store.account_symbol_net("hl_a", "BTC-USDC").unwrap() - 1.0).abs() < 1e-9);
        assert!((store.account_symbol_net("lt_a", "BTC-USDC").unwrap() + 1.0).abs() < 1e-9);
        // Ticker net is zero only because the signal hedged itself.
        assert!((store.ticker_net_position("BTC-USDC").unwrap()).abs() < 1e-9);

        let recorded = store.get_signal("s1").unwrap().unwrap();
        assert_eq!(recorded.status, SignalStatus::Balanced);
        assert!((recorded.fees_usd - 2.5).abs() < 1e-9);
    }

    #[test]
    fn test_imbalanced_signal_blocks_ticker_despite_net_zero() {
        let store = StateStore::open_in_memory().unwrap();
        // The user's exact scenario: the ticker's aggregate position reads
        // flat, but only because signal A carries −1 while signal B carries
        // +1. Neither signal hedged itself, so both must block.
        store
            .record_signal_execution(&two_leg_signal("sa", "BTC-USDC", None, None))
            .unwrap();
        store
            .apply_leg_fill("sa", LegSide::Short, 1.0, 60120.0, 0.0, Some("o-a"))
            .unwrap();
        store
            .update_signal_status("sa", SignalStatus::Imbalanced, Some("long leg failed"))
            .unwrap();

        let mut b = two_leg_signal("sb", "BTC-USDC", None, None);
        b.legs[0].venue = "lighter".into();
        b.legs[1].venue = "hyperliquid".into();
        store.record_signal_execution(&b).unwrap();
        store
            .apply_leg_fill("sb", LegSide::Long, 1.0, 60050.0, 0.0, Some("o-b"))
            .unwrap();
        store
            .update_signal_status("sb", SignalStatus::Imbalanced, Some("short leg failed"))
            .unwrap();

        // Aggregate ticker position is flat (−1 + 1 = 0)...
        assert!((store.ticker_net_position("BTC-USDC").unwrap()).abs() < 1e-9);
        // ...yet each signal individually holds residual exposure.
        assert!((store.signal_delta("sa").unwrap() + 1.0).abs() < 1e-9);
        assert!((store.signal_delta("sb").unwrap() - 1.0).abs() < 1e-9);
        // The ticker stays blocked until BOTH imbalanced signals reconcile.
        assert!(store.ticker_blocked("BTC-USDC").unwrap());
        let blockers = store.blocking_signals("BTC-USDC").unwrap();
        assert_eq!(blockers.len(), 2);

        // Closing B's exposure alone is not enough — A still blocks.
        store
            .update_signal_status("sb", SignalStatus::Closed, None)
            .unwrap();
        assert!(store.ticker_blocked("BTC-USDC").unwrap());
        let blockers = store.blocking_signals("BTC-USDC").unwrap();
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].id, "sa");
    }

    #[test]
    fn test_in_flight_signal_survives_reopen() {
        let (dir, file) = temp_store_path("signal_persist");
        {
            let store = StateStore::open(&file).unwrap();
            store
                .record_signal_execution(&two_leg_signal("s-live", "ETH-USDC", None, None))
                .unwrap();
            assert!(store.ticker_blocked("ETH-USDC").unwrap());
        }
        // Restart: the interrupted in-flight signal still blocks the ticker.
        let store = StateStore::open(&file).unwrap();
        assert!(store.ticker_blocked("eth-usdc").unwrap());
        let blockers = store.blocking_signals("ETH-USDC").unwrap();
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].status, SignalStatus::InFlight);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_signal_validation_and_account_fk() {
        let store = StateStore::open_in_memory().unwrap();
        // Unknown strategy IDs and empty symbols are rejected.
        let mut bad = two_leg_signal("bad", "BTC-USDC", None, None);
        bad.strategy_id = "nonsense".into();
        assert!(store.record_signal_execution(&bad).is_err());
        let mut bad2 = two_leg_signal("bad2", "  ", None, None);
        bad2.symbol = " ".into();
        assert!(store.record_signal_execution(&bad2).is_err());
        // Legs referencing a nonexistent account violate the FK.
        assert!(store
            .record_signal_execution(&two_leg_signal("bad3", "BTC-USDC", Some("ghost"), None))
            .is_err());
        // Zero/negative intended quantities are rejected.
        let mut bad4 = two_leg_signal("bad4", "BTC-USDC", None, None);
        bad4.legs[0].intended_qty = 0.0;
        assert!(store.record_signal_execution(&bad4).is_err());

        // account_has_signal_legs gates deletion.
        store
            .upsert_account("hl_a", "hyperliquid", "Main", true, false, "standard", None)
            .unwrap();
        assert!(!store.account_has_signal_legs("hl_a").unwrap());
        store
            .record_signal_execution(&two_leg_signal("s9", "BTC-USDC", Some("hl_a"), None))
            .unwrap();
        assert!(store.account_has_signal_legs("hl_a").unwrap());
        assert!(store.delete_account("hl_a").is_err());
    }

    #[test]
    fn test_spatial_baseline_roundtrip_and_validation() {
        let store = StateStore::open_in_memory().unwrap();
        assert_eq!(normalize_strategy_id("triangle").unwrap(), "triangle");
        store
            .record_signal_execution(&two_leg_signal("sig-base-1", "BTC-USDC", None, None))
            .unwrap();
        let row = SpatialEntryBaseline {
            signal_id: "sig-base-1".into(),
            symbol: "BTC-USDC".into(),
            buy_venue: "hyperliquid".into(),
            sell_venue: "lighter".into(),
            entry_net_bps: 20.5,
            baseline_bps: Some(12.0),
            entry_ts_ms: 1_000_000,
        };
        store.record_spatial_baseline(&row).unwrap();
        let got = store
            .spatial_baseline_for_signal("sig-base-1")
            .unwrap()
            .expect("baseline present");
        assert_eq!(got, row);
        assert!(store
            .spatial_baseline_for_signal("sig-missing")
            .unwrap()
            .is_none());
        let mut bad = row.clone();
        bad.signal_id = "sig-bad".into();
        bad.entry_net_bps = f64::NAN;
        assert!(store.record_spatial_baseline(&bad).is_err());
        let mut bad = row.clone();
        bad.signal_id = "sig-bad2".into();
        bad.entry_ts_ms = -1;
        assert!(store.record_spatial_baseline(&bad).is_err());
        // Re-recording the same signal replaces the baseline (re-entry).
        let mut updated = row.clone();
        updated.entry_net_bps = 18.0;
        store.record_spatial_baseline(&updated).unwrap();
        let got = store
            .spatial_baseline_for_signal("sig-base-1")
            .unwrap()
            .expect("baseline present");
        assert_eq!(got.entry_net_bps, 18.0);
    }

    fn sample(venue_id: &str, mid: f64) -> VenueSampleEntry {
        VenueSampleEntry {
            venue_id: venue_id.into(),
            mid,
            best_bid: mid - 0.5,
            best_ask: mid + 0.5,
        }
    }

    #[test]
    fn test_spread_samples_record_order_and_filter() {
        let store = StateStore::open_in_memory().unwrap();
        store
            .record_spread_samples(
                "BTC-USDC",
                1_000,
                &[sample("hyperliquid", 100.0), sample("lighter", 101.0)],
            )
            .unwrap();
        store
            .record_spread_samples("BTC-USDC", 2_000, &[sample("hyperliquid", 100.5)])
            .unwrap();
        // Other symbol is isolated by the query.
        store
            .record_spread_samples("ETH-USDC", 1_500, &[sample("lighter", 50.0)])
            .unwrap();

        let all = store.spread_samples("BTC-USDC", 0, i64::MAX).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].ts_ms, 1_000);
        assert_eq!(all[0].venue_id, "hyperliquid");
        assert_eq!(all[1].venue_id, "lighter");
        assert_eq!(all[2].ts_ms, 2_000);

        let windowed = store.spread_samples("BTC-USDC", 1_500, 2_500).unwrap();
        assert_eq!(windowed.len(), 1);
        assert_eq!(windowed[0].mid, 100.5);

        assert_eq!(
            store.earliest_spread_sample_ts("BTC-USDC").unwrap(),
            Some(1_000)
        );
        assert_eq!(store.earliest_spread_sample_ts("SOL-USDC").unwrap(), None);
    }

    #[test]
    fn test_spread_samples_drop_invalid_and_normalize() {
        let store = StateStore::open_in_memory().unwrap();
        store
            .record_spread_samples(
                "BTC-USDC",
                1_000,
                &[
                    sample("HyperLiquid", 100.0),
                    sample("lighter", f64::NAN),
                    sample("lighter", -5.0),
                    sample("lighter", 101.0),
                ],
            )
            .unwrap();
        let rows = store.spread_samples("BTC-USDC", 0, i64::MAX).unwrap();
        assert_eq!(rows.len(), 2, "non-finite/non-positive mids are dropped");
        assert_eq!(rows[0].venue_id, "hyperliquid", "venue ids normalize");
        // Empty batches and negative timestamps are no-ops.
        store.record_spread_samples("BTC-USDC", 5_000, &[]).unwrap();
        store
            .record_spread_samples("BTC-USDC", -1, &[sample("lighter", 1.0)])
            .unwrap();
        assert_eq!(
            store.spread_samples("BTC-USDC", 0, i64::MAX).unwrap().len(),
            2
        );
    }

    #[test]
    fn test_spread_samples_merge_background_and_ui_keys() {
        // Regression: the monitor + WS feed record under the canonical base
        // ("BTC") while UI polls record under the full symbol ("BTC-USDC").
        // Reads for either key must see both so unselected tickers keep a
        // fresh trailing window from background recording alone.
        let store = StateStore::open_in_memory().unwrap();
        let now = 1_700_000_000_000i64;
        store
            .record_spread_samples("BTC", now - 1_000, &[sample("hyperliquid", 100.0)])
            .unwrap();
        store
            .record_spread_samples("BTC-USDC", now - 60_000, &[sample("lighter", 101.0)])
            .unwrap();

        let via_ui = store.spread_samples("BTC-USDC", 0, i64::MAX).unwrap();
        assert_eq!(via_ui.len(), 2, "UI key must see background base-keyed rows");
        let via_base = store.spread_samples("BTC", 0, i64::MAX).unwrap();
        assert_eq!(via_base.len(), 2, "base key must see UI full-symbol rows");
        // Trailing-window slice sees the fresh background row.
        let windowed = store
            .spread_samples("BTC-USDC", now - 5_000, now)
            .unwrap();
        assert_eq!(windowed.len(), 1);
        assert_eq!(windowed[0].venue_id, "hyperliquid");
        assert_eq!(
            store.earliest_spread_sample_ts("BTC-USDC").unwrap(),
            Some(now - 60_000)
        );
        assert_eq!(
            store.earliest_spread_sample_ts("BTC").unwrap(),
            Some(now - 60_000)
        );
    }

    #[test]
    fn test_spread_samples_aliases_keep_symbols_isolated() {
        // Alias merging must never leak one asset's rows into another's.
        let store = StateStore::open_in_memory().unwrap();
        store
            .record_spread_samples("ETH", 1_000, &[sample("lighter", 50.0)])
            .unwrap();
        store
            .record_spread_samples("ETH-USDC", 2_000, &[sample("lighter", 51.0)])
            .unwrap();
        store
            .record_spread_samples("BTC", 1_500, &[sample("lighter", 100.0)])
            .unwrap();
        let rows = store.spread_samples("BTC-USDC", 0, i64::MAX).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].mid, 100.0);
        assert_eq!(
            store.earliest_spread_sample_ts("BTC-USDC").unwrap(),
            Some(1_500)
        );
    }

    #[test]
    fn test_kv_roundtrip_and_retention_keys() {
        let store = StateStore::open_in_memory().unwrap();
        assert_eq!(store.kv_get("retention.max_days").unwrap(), None);
        store.kv_set("retention.max_days", "30").unwrap();
        store.kv_set("retention.max_gb", "2.5").unwrap();
        assert_eq!(
            store.kv_get("retention.max_days").unwrap(),
            Some("30".to_string())
        );
        assert_eq!(
            store.kv_get("retention.max_gb").unwrap(),
            Some("2.5".to_string())
        );
        store.kv_set("retention.max_days", "7").unwrap();
        assert_eq!(
            store.kv_get("retention.max_days").unwrap(),
            Some("7".to_string())
        );
        store.kv_delete("retention.max_days").unwrap();
        assert_eq!(store.kv_get("retention.max_days").unwrap(), None);
        assert!(store.kv_set("", "x").is_err());
    }

    #[test]
    fn test_kv_survives_reopen() {
        let (dir, file) = temp_store_path("kv_reopen");
        {
            let store = StateStore::open(&file).unwrap();
            store.kv_set("retention.max_days", "14").unwrap();
            store.kv_set("retention.max_gb", "1.5").unwrap();
        }
        {
            let reopened = StateStore::open(&file).unwrap();
            assert_eq!(
                reopened.kv_get("retention.max_days").unwrap(),
                Some("14".to_string())
            );
            assert_eq!(
                reopened.kv_get("retention.max_gb").unwrap(),
                Some("1.5".to_string())
            );
            let version: i64 = reopened
                .conn
                .lock()
                .query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap();
            assert_eq!(version, 19);
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_retention_time_cutoff_deletes_oldest_first() {
        let store = StateStore::open_in_memory().unwrap();
        for i in 0..10 {
            store
                .record_spread_samples(
                    "BTC-USDC",
                    i * 1_000,
                    &[sample("hyperliquid", 100.0 + i as f64)],
                )
                .unwrap();
        }
        let removed = store.delete_spread_samples_before(5_000).unwrap();
        assert_eq!(removed, 5);
        let rows = store.spread_samples("BTC-USDC", 0, i64::MAX).unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].ts_ms, 5_000);
    }

    #[test]
    fn test_retention_cutoff_keeps_fresh_rows_across_alias_keys() {
        // Retention is purely time-based: fresh background ("BTC") and UI
        // ("BTC-USDC") rows must both survive a cutoff that drops old rows.
        let store = StateStore::open_in_memory().unwrap();
        store
            .record_spread_samples("BTC", 1_000, &[sample("hyperliquid", 100.0)])
            .unwrap();
        store
            .record_spread_samples("BTC-USDC", 2_000, &[sample("lighter", 101.0)])
            .unwrap();
        store
            .record_spread_samples("BTC", 9_000, &[sample("hyperliquid", 102.0)])
            .unwrap();
        store
            .record_spread_samples("BTC-USDC", 10_000, &[sample("lighter", 103.0)])
            .unwrap();
        let removed = store.delete_spread_samples_before(5_000).unwrap();
        assert_eq!(removed, 2);
        let rows = store.spread_samples("BTC-USDC", 0, i64::MAX).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.ts_ms >= 5_000));
    }

    #[test]
    fn test_retention_oldest_first_batch_delete() {
        let store = StateStore::open_in_memory().unwrap();
        for i in 0..10 {
            store
                .record_spread_samples(
                    "BTC-USDC",
                    i * 1_000,
                    &[sample("hyperliquid", 100.0 + i as f64)],
                )
                .unwrap();
        }
        let removed = store.delete_oldest_spread_samples(3).unwrap();
        assert_eq!(removed, 3);
        let rows = store.spread_samples("BTC-USDC", 0, i64::MAX).unwrap();
        assert_eq!(rows.len(), 7);
        assert_eq!(rows[0].ts_ms, 3_000);
        assert_eq!(store.spread_sample_count().unwrap(), 7);
        assert!(store.db_size_bytes().unwrap() > 0);
    }

    #[test]
    fn test_spread_samples_prune_retention_and_cap() {
        let store = StateStore::open_in_memory().unwrap();
        for i in 0..10 {
            store
                .record_spread_samples(
                    "BTC-USDC",
                    i * 1_000,
                    &[sample("hyperliquid", 100.0 + i as f64)],
                )
                .unwrap();
        }
        // Retention: drop everything before ts 5_000.
        store.prune_spread_samples(5_000, 1_000_000).unwrap();
        let rows = store.spread_samples("BTC-USDC", 0, i64::MAX).unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].ts_ms, 5_000);
        // Cap: keep only the newest 2 rows.
        store.prune_spread_samples(0, 2).unwrap();
        let rows = store.spread_samples("BTC-USDC", 0, i64::MAX).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].ts_ms, 8_000);
        assert_eq!(rows[1].ts_ms, 9_000);
    }

    /* ------------------ signal-event audit log (V9) ------------------ */

    #[test]
    fn test_signal_events_append_readback_and_validation() {
        let store = StateStore::open_in_memory().unwrap();
        store
            .record_monitor_signal_event(
                "BTC",
                "HyperLiquid",
                "lighter",
                60_000,
                None,
                "preparing",
                Some(12.5),
                Some(3.0),
                Some(0.9),
                Some(r#"{"friction_bps":5.0}"#),
                "transition",
            )
            .unwrap();
        store
            .record_monitor_signal_event(
                "BTC",
                "hyperliquid",
                "lighter",
                120_000,
                Some("preparing"),
                "triggered",
                None,
                None,
                None,
                None,
                "transition",
            )
            .unwrap();
        store
            .record_monitor_signal_event(
                "BTC",
                "lighter",
                "hyperliquid",
                120_000,
                None,
                "tracking",
                Some(1.0),
                None,
                None,
                Some("{}"),
                "transition",
            )
            .unwrap();

        // Pair-filtered readback, newest first; venue ids normalize.
        let rows = store
            .recent_monitor_signal_events(Some("BTC"), Some("hyperliquid"), Some("lighter"), 10)
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].ts_ms, 120_000);
        assert_eq!(rows[0].old_status.as_deref(), Some("preparing"));
        assert_eq!(rows[0].new_status, "triggered");
        assert_eq!(rows[1].old_status, None);
        assert_eq!(
            rows[1].economics_json.as_deref(),
            Some(r#"{"friction_bps":5.0}"#)
        );
        assert_eq!(rows[1].buy_venue, "hyperliquid");
        // Unfiltered sees all three; limit bounds the read.
        assert_eq!(
            store
                .recent_monitor_signal_events(None, None, None, 10)
                .unwrap()
                .len(),
            3
        );
        assert_eq!(
            store
                .recent_monitor_signal_events(None, None, None, 2)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(store.monitor_signal_event_count().unwrap(), 3);

        // Every invalid append is rejected and stores nothing new.
        assert!(store
            .record_monitor_signal_event(
                "BTC", "hyperliquid", "lighter", -1, None, "triggered",
                None, None, None, None, "transition",
            )
            .is_err());
        assert!(store
            .record_monitor_signal_event(
                "BTC", "hyperliquid", "lighter", 1, None, "bogus",
                None, None, None, None, "transition",
            )
            .is_err());
        assert!(store
            .record_monitor_signal_event(
                "BTC", "hyperliquid", "lighter", 1, Some("bogus"), "triggered",
                None, None, None, None, "transition",
            )
            .is_err());
        assert!(store
            .record_monitor_signal_event(
                "BTC", "hyperliquid", "lighter", 1, None, "triggered",
                None, None, None, None, "pulse",
            )
            .is_err());
        assert!(store
            .record_monitor_signal_event(
                "BTC", "hyperliquid", "lighter", 1, None, "triggered",
                Some(f64::NAN), None, None, None, "transition",
            )
            .is_err());
        assert!(store
            .record_monitor_signal_event(
                "BTC", "hyperliquid", "lighter", 1, None, "triggered",
                None, None, Some(1.5), None, "transition",
            )
            .is_err());
        assert!(store
            .record_monitor_signal_event(
                "BTC", "hyperliquid", "lighter", 1, None, "triggered",
                None, None, None, Some("{nope"), "transition",
            )
            .is_err());
        assert_eq!(store.monitor_signal_event_count().unwrap(), 3);
        assert!(store
            .recent_monitor_signal_events(None, None, None, 0)
            .is_err());
    }

    #[test]
    fn test_signal_events_prune_retention_and_cap() {
        let store = StateStore::open_in_memory().unwrap();
        for i in 0..6 {
            store
                .record_monitor_signal_event(
                    "BTC",
                    "hyperliquid",
                    "lighter",
                    i * 1_000,
                    None,
                    "tracking",
                    None,
                    None,
                    None,
                    None,
                    "transition",
                )
                .unwrap();
        }
        // Same-days time cutoff drops old audit rows first.
        store
            .prune_monitor_signal_events(3_000, MONITOR_SIGNAL_EVENTS_MAX_ROWS)
            .unwrap();
        let rows = store
            .recent_monitor_signal_events(None, None, None, 100)
            .unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|r| r.ts_ms >= 3_000));
        // Separate cap keeps only the newest rows.
        store.prune_monitor_signal_events(0, 2).unwrap();
        let rows = store
            .recent_monitor_signal_events(None, None, None, 100)
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].ts_ms, 5_000);
        assert_eq!(rows[1].ts_ms, 4_000);
    }

    /* ------------------ session seasonality (V8) ------------------ */

    #[test]
    fn test_migration_v7_to_v8_preserves_data() {
        let conn = Connection::open_in_memory().unwrap();
        for batch in [
            MIGRATION_V1_SQL,
            MIGRATION_V2_SQL,
            MIGRATION_V3_SQL,
            MIGRATION_V4_SQL,
            MIGRATION_V5_SQL,
            MIGRATION_V6_SQL,
            MIGRATION_V7_SQL,
        ] {
            conn.execute_batch(batch).unwrap();
        }
        let v7: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v7, 7);
        conn.execute(
            "INSERT INTO spread_samples (symbol, venue_id, ts_ms, mid, best_bid, best_ask)
             VALUES ('BTC', 'hyperliquid', 1000, 100.0, 99.9, 100.1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO monitor_signals
                 (symbol, buy_venue, sell_venue, status, first_seen_ms, last_seen_ms, updated_at)
             VALUES ('BTC', 'hyperliquid', 'lighter', 'watching', 1, 2, 't')",
            [],
        )
        .unwrap();
        StateStore::migrate(&conn).unwrap();
        let v10: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v10, 19);
        // v19 remaps the renamed lifecycle statuses on the way through.
        let remapped: String = conn
            .query_row(
                "SELECT status FROM monitor_signals WHERE symbol = 'BTC'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remapped, "tracking");
        let kept: i64 = conn
            .query_row("SELECT COUNT(*) FROM spread_samples", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kept, 1, "V7 rows must survive the V8..V18 migration");
        let kept_signals: i64 = conn
            .query_row("SELECT COUNT(*) FROM monitor_signals", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kept_signals, 1, "V5 lifecycle rows must survive");
        for table in [
            "venue_volume_samples",
            "session_profiles",
            "session_hour_bins",
            "monitor_signal_events",
            "radar_bot_config",
            "radar_bot_positions",
            "har_anchors",
            "promotion_state",
            "autotune_apply_events",
            "spatial_entry_baseline",
            "paper_equity_expectations",
            "account_equity_events",
            "funding_rate_samples",
        ] {
            let found: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(found, 1, "migration must create {table}");
        }
    }

    #[test]
    fn test_volume_sampler_roundtrip_and_alias() {
        use std::collections::HashMap;
        let store = StateStore::open_in_memory().unwrap();
        let mut vols = HashMap::new();
        vols.insert("BTCUSDT".to_string(), 1_000_000.0);
        vols.insert("ETHUSDT".to_string(), 500_000.0);
        vols.insert("JUNK".to_string(), f64::NAN);
        vols.insert("ZERO".to_string(), 0.0);
        store
            .record_venue_volume_samples("hyperliquid", 60_000, &vols)
            .unwrap();
        // Canonical-base storage reads back through quoted aliases.
        let btc = store
            .venue_volume_series("BTC-USDC", "hyperliquid", 0, i64::MAX)
            .unwrap();
        assert_eq!(btc, vec![(60_000, 1_000_000.0)]);
        let eth = store
            .venue_volume_series("eth", "hyperliquid", 0, i64::MAX)
            .unwrap();
        assert_eq!(eth.len(), 1);
        // Non-finite / non-positive rows are never persisted.
        let all: i64 = store
            .conn
            .lock()
            .query_row("SELECT COUNT(*) FROM venue_volume_samples", [], |r| r.get(0))
            .unwrap();
        assert_eq!(all, 2);
        // Retention bounds the raw log.
        store.prune_venue_volume_samples(60_000, 1_000_000).unwrap();
        let kept: i64 = store
            .conn
            .lock()
            .query_row("SELECT COUNT(*) FROM venue_volume_samples", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kept, 2);
        store.prune_venue_volume_samples(60_001, 1_000_000).unwrap();
        let kept: i64 = store
            .conn
            .lock()
            .query_row("SELECT COUNT(*) FROM venue_volume_samples", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kept, 0);
    }

    /// Frozen sixteen-day fixture: hourly two-venue polls, Thursday 2026-01-15
    /// 00:00 UTC onward. Spreads alternate between a session peak and a
    /// 7bps revert level so every session segments winning episodes. Peaks
    /// land on even poll indexes, so a session with H hours calibrates
    /// H/2 episodes per day.
    fn seed_session_history(store: &StateStore, days: i64) -> (i64, i64) {
        use chrono::TimeZone;
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
        for i in 0..(days * 24) {
            let ts = base + i * 3_600_000 + 60_000;
            let spread_bps = if i % 2 == 0 {
                peak_for_hour(i)
            } else {
                7.0
            };
            store
                .record_spread_samples(
                    "BTC",
                    ts,
                    &[
                        sample("hyperliquid", 100.0),
                        sample("lighter", 100.0 * (1.0 + spread_bps / 10_000.0)),
                    ],
                )
                .unwrap();
        }
        (base, base + days * 24 * 3_600_000)
    }

    #[test]
    fn test_rollup_seeded_multi_day_history() {
        let store = StateStore::open_in_memory().unwrap();
        let (from, to) = seed_session_history(&store, 16);
        let written = store
            .rollup_session_profiles("BTC-USDC", from - 1_000, to + 1_000, to)
            .unwrap();
        assert_eq!(written, 5, "one row per session");
        let profiles = store.session_profiles("BTC").unwrap();
        assert_eq!(profiles.len(), 5);
        let by_session: std::collections::HashMap<&str, &SessionProfileRow> = profiles
            .iter()
            .map(|r| (r.session_id.as_str(), r))
            .collect();
        // Overlap carries 32 episodes: calibrated, not provisional.
        let overlap = by_session["overlap"];
        assert!(overlap.n_episodes >= 30, "overlap episodes: {}", overlap.n_episodes);
        assert!(!overlap.provisional);
        assert!(overlap.med_abs_spread_bps.unwrap() > 0.0);
        assert!(
            overlap.win_rate.unwrap() >= 0.9,
            "alternating fixture must mostly revert, got {:?}",
            overlap.win_rate
        );
        assert!(overlap.mean_hold_s.unwrap() > 0.0);
        assert!(overlap.rv_bps.unwrap() > 0.0);
        // Dead zone carries 16 episodes: provisional, never fabricated.
        let dead = by_session["dead"];
        assert!(dead.n_episodes < 30);
        assert!(dead.provisional);
        // Hour bins: 24 rows, modulators inside +/-25%, provisional reads 1.0.
        let bins = store.session_hour_bins("BTC-USDC").unwrap();
        assert_eq!(bins.len(), 24);
        for b in &bins {
            assert!(
                (0.75..=1.25).contains(&b.modulator),
                "hour {} modulator {}",
                b.hour,
                b.modulator
            );
            if b.provisional {
                assert_eq!(b.modulator, 1.0);
            }
        }
        let peak_bin = bins.iter().find(|b| b.hour == 14).unwrap();
        assert!(!peak_bin.provisional);
        assert!(peak_bin.med_spread_bps.unwrap() > 0.0);
    }

    #[test]
    fn test_rollup_thin_data_stays_provisional() {
        let store = StateStore::open_in_memory().unwrap();
        use chrono::TimeZone;
        let ts = chrono::Utc
            .with_ymd_and_hms(2026, 1, 15, 14, 5, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        store
            .record_spread_samples(
                "BTC",
                ts,
                &[sample("hyperliquid", 100.0), sample("lighter", 100.5)],
            )
            .unwrap();
        let written = store
            .rollup_session_profiles("BTC", ts - 1_000, ts + 1_000, ts)
            .unwrap();
        assert_eq!(written, 5);
        for row in store.session_profiles("BTC").unwrap() {
            assert!(
                row.provisional,
                "single-tick history must stay provisional: {:?}",
                row.session_id
            );
        }
    }

    #[test]
    fn test_prune_keeps_session_profiles() {
        let store = StateStore::open_in_memory().unwrap();
        let (from, to) = seed_session_history(&store, 16);
        store
            .rollup_session_profiles("BTC", from - 1_000, to + 1_000, to)
            .unwrap();
        assert_eq!(store.session_profiles("BTC").unwrap().len(), 5);
        // Nuke the entire raw log: time cutoff plus zero-row cap.
        store.prune_spread_samples(i64::MAX, 0).unwrap();
        assert_eq!(store.spread_sample_count().unwrap(), 0);
        assert_eq!(
            store
                .delete_spread_samples_before(i64::MAX)
                .unwrap_or(0),
            0
        );
        // Derived state survives every raw-sample prune path.
        assert_eq!(store.session_profiles("BTC").unwrap().len(), 5);
        assert_eq!(store.session_hour_bins("BTC").unwrap().len(), 24);
    }

    /* ------------------ open-position aggregation ------------------ */

    #[allow(clippy::too_many_arguments)]
    fn filled_two_leg(
        store: &StateStore,
        id: &str,
        symbol: &str,
        strategy: &str,
        long_venue: &str,
        short_venue: &str,
        qty: f64,
        long_px: f64,
        short_px: f64,
    ) {
        let mut sig = two_leg_signal(id, symbol, None, None);
        sig.strategy_id = strategy.to_string();
        sig.legs[0].venue = long_venue.to_string();
        sig.legs[1].venue = short_venue.to_string();
        sig.legs[0].intended_qty = qty;
        sig.legs[1].intended_qty = qty;
        store.record_signal_execution(&sig).unwrap();
        store
            .apply_leg_fill(id, LegSide::Long, qty, long_px, 0.5, None)
            .unwrap();
        store
            .apply_leg_fill(id, LegSide::Short, qty, short_px, 0.5, None)
            .unwrap();
        store
            .update_signal_status(id, SignalStatus::Balanced, None)
            .unwrap();
    }

    #[test]
    fn test_unknown_leg_side_fails_closed_instead_of_defaulting_long() {
        let store = StateStore::open_in_memory().unwrap();
        let sig = two_leg_signal("sig_bad_side", "BTC-USDC", None, None);
        store.record_signal_execution(&sig).unwrap();
        {
            let conn = store.conn.lock();
            conn.execute_batch("PRAGMA ignore_check_constraints = ON;")
                .unwrap();
            conn.execute(
                "UPDATE signal_legs SET side = 'sideways' WHERE signal_id = ?1",
                [&sig.id],
            )
            .unwrap();
        }
        assert!(store.get_signal(&sig.id).is_err());
    }

    #[test]
    fn test_aggregate_open_positions_groups_by_venue_pair() {
        let store = StateStore::open_in_memory().unwrap();
        // BTC traded twice on hyperliquid-vs-lighter, once on
        // hyperliquid-vs-arcus: two cards, first aggregates both signals.
        filled_two_leg(&store, "s1", "BTC-USDC", "futures", "hyperliquid", "lighter", 1.0, 60000.0, 60100.0);
        filled_two_leg(&store, "s2", "BTC-USDC", "futures", "hyperliquid", "lighter", 0.5, 60000.0, 60100.0);
        filled_two_leg(&store, "s3", "BTC-USDC", "futures", "hyperliquid", "arcus", 1.0, 60000.0, 60100.0);

        let signals = store.list_signals(None, None).unwrap();
        let positions = aggregate_open_positions(&signals, &|_, _| None);
        assert_eq!(positions.len(), 2, "A-vs-B merges, A-vs-C stays separate");

        let hl_lt = positions
            .iter()
            .find(|p| p.venues.len() == 2 && p.venues[1].venue == "lighter")
            .unwrap();
        assert_eq!(hl_lt.signal_count, 2);
        assert_eq!(hl_lt.symbol, "BTC-USDC");
        // Long row on top (+1.5 on hyperliquid), short row below.
        assert_eq!(hl_lt.venues[0].venue, "hyperliquid");
        assert!((hl_lt.venues[0].net_qty - 1.5).abs() < 1e-9);
        assert_eq!(hl_lt.venues[1].venue, "lighter");
        assert!((hl_lt.venues[1].net_qty + 1.5).abs() < 1e-9);
        // No marks supplied -> PnL honestly unavailable, not fabricated zero.
        assert!(hl_lt.unrealized_pnl_usd.is_none());
        assert!(hl_lt.notional_usd.is_some());
        assert_eq!(hl_lt.expected_net_usd, Some(25.0));
    }

    #[test]
    fn test_aggregate_open_positions_marks_to_market_and_filters() {
        let store = StateStore::open_in_memory().unwrap();
        filled_two_leg(&store, "s1", "BTC-USDC", "futures", "hyperliquid", "lighter", 1.0, 100.0, 105.0);
        filled_two_leg(&store, "s2", "ETH-USDC", "funding", "hyperliquid", "lighter", 2.0, 10.0, 10.0);
        // A closed signal must not contribute.
        filled_two_leg(&store, "s3", "SOL-USDC", "futures", "hyperliquid", "lighter", 5.0, 1.0, 1.0);
        store
            .update_signal_status("s3", SignalStatus::Closed, None)
            .unwrap();

        let signals = store.list_signals(None, None).unwrap();
        // Marks: both venues at 110 -> long +10, short loses (110-105)*1 = -5.
        let positions = aggregate_open_positions(&signals, &|_, _| Some(110.0));
        assert_eq!(positions.len(), 2, "closed signals drop out");

        let btc = positions.iter().find(|p| p.symbol == "BTC-USDC").unwrap();
        // gross = +10 - 5 = 5; minus fees (2 legs * 0.5 = 1.0) => 4.0
        let pnl = btc.unrealized_pnl_usd.expect("all legs markable");
        assert!((pnl - 4.0).abs() < 1e-9, "unexpected pnl {pnl}");

        // Strategy scoping: the funding signal is a separate group.
        let funding: Vec<_> = positions
            .iter()
            .filter(|p| p.strategy_id == "funding")
            .collect();
        assert_eq!(funding.len(), 1);
        assert_eq!(funding[0].symbol, "ETH-USDC");
    }

    #[test]
    fn test_aggregate_open_positions_partial_flatten_nets_residual() {
        let store = StateStore::open_in_memory().unwrap();
        filled_two_leg(&store, "s1", "BTC-USDC", "futures", "hyperliquid", "lighter", 1.0, 100.0, 100.0);
        // Partial flatten child: opposite-side legs on the same venues,
        // still open (unresolved) -> nets the group to residual 0.4.
        let mut child = two_leg_signal("flat-s1-0001", "BTC-USDC", None, None);
        child.legs[0].side = LegSide::Short;
        child.legs[1].side = LegSide::Long;
        child.legs[0].intended_qty = 0.6;
        child.legs[1].intended_qty = 0.6;
        store.record_signal_execution(&child).unwrap();
        store
            .apply_leg_fill("flat-s1-0001", LegSide::Short, 0.6, 100.0, 0.0, None)
            .unwrap();
        store
            .apply_leg_fill("flat-s1-0001", LegSide::Long, 0.6, 100.0, 0.0, None)
            .unwrap();
        store
            .update_signal_status("flat-s1-0001", SignalStatus::Unresolved, None)
            .unwrap();

        let signals = store.list_signals(None, None).unwrap();
        let positions = aggregate_open_positions(&signals, &|_, _| None);
        assert_eq!(positions.len(), 1, "flatten child joins the same group");
        let p = &positions[0];
        assert!((p.venues[0].net_qty - 0.4).abs() < 1e-9);
        assert!((p.venues[1].net_qty + 0.4).abs() < 1e-9);
        assert_eq!(p.signal_count, 2);
    }

    /* ------------------ seasonality Phase 2 + autotune apply (V12) -------- */

    fn har_row() -> HarAnchorRow {
        HarAnchorRow {
            symbol: "BTC-USDC".to_string(),
            intercept: 0.2,
            b_daily: 0.5,
            b_weekly: 0.3,
            b_monthly: 0.15,
            r2: 0.92,
            n_days: 60,
            forecast_rv_bps: 11.0,
            trailing_mean_rv_bps: 10.0,
            updated_ts: 7_000,
        }
    }

    #[test]
    fn test_phase2_har_anchor_roundtrip_and_validation() {
        let store = StateStore::open_in_memory().unwrap();
        assert_eq!(store.phase2_har_anchor("BTC-USDC").unwrap(), None);
        store.phase2_upsert_har_anchor(&har_row()).unwrap();
        let got = store.phase2_har_anchor("BTC").unwrap().expect("alias read");
        assert!((got.forecast_rv_bps - 11.0).abs() < 1e-12);
        let mut bad = har_row();
        bad.r2 = 1.5;
        assert!(store.phase2_upsert_har_anchor(&bad).is_err());
        let mut nan = har_row();
        nan.b_daily = f64::NAN;
        assert!(store.phase2_upsert_har_anchor(&nan).is_err());
        let mut neg = har_row();
        neg.forecast_rv_bps = -1.0;
        assert!(store.phase2_upsert_har_anchor(&neg).is_err());
    }

    #[test]
    fn test_phase2_promotion_state_roundtrip_and_validation() {
        let store = StateStore::open_in_memory().unwrap();
        assert_eq!(store.phase2_promotion_state("ETH-USDC").unwrap(), None);
        store
            .phase2_upsert_promotion_state(&PromotionStateRow {
                symbol: "ETH-USDC".to_string(),
                status: "promoted".to_string(),
                fit_delta_bps: 2.0,
                val_delta_bps: 1.5,
                reason: "agree".to_string(),
                updated_ts: 9_000,
            })
            .unwrap();
        let got = store.phase2_promotion_state("ETH").unwrap().expect("alias read");
        assert_eq!(got.status, "promoted");
        assert!(store
            .phase2_upsert_promotion_state(&PromotionStateRow {
                symbol: "ETH-USDC".to_string(),
                status: "live".to_string(),
                fit_delta_bps: 2.0,
                val_delta_bps: 1.5,
                reason: "agree".to_string(),
                updated_ts: 9_000,
            })
            .is_err());
    }

    #[test]
    fn test_autotune_apply_events_audit_roundtrip() {
        let store = StateStore::open_in_memory().unwrap();
        assert!(store.recent_autotune_apply_events("BTC-USDC", 10).unwrap().is_empty());
        let id = store
            .record_autotune_apply_event(
                "BTC-USDC",
                r#"{"targetProfitBps": 8.5}"#,
                r#"{"span": [1, 2], "fit": "ok"}"#,
                12_000,
            )
            .unwrap();
        assert!(id > 0);
        let rows = store.recent_autotune_apply_events("BTC", 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].applied_json, r#"{"targetProfitBps": 8.5}"#);
        assert!(store
            .record_autotune_apply_event("BTC-USDC", "not json", "{}", 1)
            .is_err());
        assert!(store.recent_autotune_apply_events("BTC-USDC", 0).is_err());
    }

    #[test]
    fn test_max_leverage_validation_bounds() {
        let store = StateStore::open_in_memory().unwrap();
        assert!(store
            .upsert_account("a1", "hyperliquid", "Main", true, false, "standard", Some(0))
            .is_err());
        assert!(store
            .upsert_account("a1", "hyperliquid", "Main", true, false, "standard", Some(6))
            .is_err());
        assert!(store
            .upsert_account("a1", "hyperliquid", "Main", true, false, "standard", Some(99))
            .is_err());
        for ok in [None, Some(1), Some(3), Some(5)] {
            store
                .upsert_account("a1", "hyperliquid", "Main", true, false, "standard", ok)
                .unwrap();
            let got = store.get_account("a1").unwrap().expect("row");
            assert_eq!(got.max_leverage, ok);
        }
    }

    #[test]
    fn test_max_leverage_roundtrip_update_and_clear() {
        let store = StateStore::open_in_memory().unwrap();
        let created = store
            .upsert_account("a1", "lighter", "Main", true, true, "standard", Some(2))
            .unwrap();
        assert_eq!(created.max_leverage, Some(2));
        let listed = store.list_accounts(Some("lighter")).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].max_leverage, Some(2));
        let cleared = store
            .upsert_account("a1", "lighter", "Main", true, true, "standard", None)
            .unwrap();
        assert_eq!(cleared.max_leverage, None);
        let fetched = store.get_account("a1").unwrap().expect("row");
        assert_eq!(fetched.max_leverage, None);
        assert!(fetched.is_default);
    }

    #[test]
    fn test_equity_expectation_baseline_init_then_quiet() {
        let store = StateStore::open_in_memory().unwrap();
        assert_eq!(store.get_equity_expectation("hyperliquid").unwrap(), None);
        let first = store
            .check_account_equity("hyperliquid", 100_000.0, "", 1_000)
            .unwrap();
        assert_eq!(first.event, None);
        assert!(first.note.is_some(), "init must leave a coverage note");
        let (expected, sig, poll) = store
            .get_equity_expectation("hyperliquid")
            .unwrap()
            .expect("baseline stored");
        assert_eq!(expected, 100_000.0);
        assert_eq!(sig, "");
        assert_eq!(poll, 1_000);
        let quiet = store
            .check_account_equity("hyperliquid", 100_000.0, "", 2_000)
            .unwrap();
        assert_eq!(quiet.event, None);
        assert_eq!(quiet.note, None);
        assert!(store.list_equity_events(Some("hyperliquid"), 10).unwrap().is_empty());
    }

    #[test]
    fn test_equity_manual_deposit_detected_and_rebased() {
        let store = StateStore::open_in_memory().unwrap();
        store
            .check_account_equity("lighter", 50_000.0, "", 1_000)
            .unwrap();
        let report = store
            .check_account_equity("lighter", 55_000.0, "", 2_000)
            .unwrap();
        let event = report.event.expect("deposit event");
        assert_eq!(event.kind, "deposit");
        assert_eq!(event.delta, 5_000.0);
        assert_eq!(event.expected, 50_000.0);
        assert_eq!(event.actual, 55_000.0);
        let (expected, _, _) = store
            .get_equity_expectation("lighter")
            .unwrap()
            .expect("rebased");
        assert_eq!(expected, 55_000.0, "baseline must rebase so PnL never absorbs the flow");
        let rows = store.list_equity_events(Some("lighter"), 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, event.id);
    }

    #[test]
    fn test_equity_withdrawal_and_transfer_guess() {
        let store = StateStore::open_in_memory().unwrap();
        store.check_account_equity("vanta", 20_000.0, "", 1_000).unwrap();
        let out = store
            .check_account_equity("vanta", 19_000.0, "", 2_000)
            .unwrap();
        assert_eq!(out.event.as_ref().map(|e| e.kind.as_str()), Some("withdrawal"));
        let moved = store
            .check_account_equity("vanta", 19_050.0, "BTC-USDC:Buy:1", 3_000)
            .unwrap();
        assert_eq!(
            moved.event.as_ref().map(|e| e.kind.as_str()),
            Some("transfer"),
            "position change cross-check must win over the sign guess"
        );
    }

    #[test]
    fn test_equity_missed_poll_gap_produces_no_event() {
        let store = StateStore::open_in_memory().unwrap();
        store.check_account_equity("grvt", 10_000.0, "", 1_000).unwrap();
        let gap = crate::equity_attribution::EQUITY_POLL_GAP_MAX_MS + 1;
        let report = store
            .check_account_equity("grvt", 99_000.0, "", 1_000 + gap)
            .unwrap();
        assert_eq!(report.event, None);
        assert!(report.note.unwrap_or_default().contains("gap"));
        let (expected, _, _) = store.get_equity_expectation("grvt").unwrap().expect("rebased");
        assert_eq!(expected, 99_000.0);
        assert!(store.list_equity_events(None, 100).unwrap().is_empty());
    }

    #[test]
    fn test_equity_events_ipc_listing_filters_and_limits() {        let store = StateStore::open_in_memory().unwrap();
        for (acct, base) in [("hyperliquid", 10_000.0), ("lighter", 20_000.0)] {
            store.check_account_equity(acct, base, "", 1_000).unwrap();
            store.check_account_equity(acct, base + 100.0, "", 2_000).unwrap();
        }
        let all = store.list_equity_events(None, 100).unwrap();
        assert_eq!(all.len(), 2);
        assert!(all[0].ts_ms >= all[1].ts_ms, "newest first");
        let one = store.list_equity_events(Some("lighter"), 100).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].account_id, "lighter");
        let capped = store.list_equity_events(None, 1).unwrap();
        assert_eq!(capped.len(), 1);
        assert!(store.list_equity_events(Some("nope"), 10).unwrap().is_empty());
    }

    #[test]
    fn test_margin_snapshots_record_list_prune() {
        let store = StateStore::open_in_memory().unwrap();
        store
            .record_margin_snapshot("hyperliquid", "hyperliquid", 1_000, 10_000.0, 2_000.0, 20.0)
            .unwrap();
        store
            .record_margin_snapshot("hyperliquid", "hyperliquid", 2_000, 10_000.0, 3_000.0, 30.0)
            .unwrap();
        store
            .record_margin_snapshot("lighter", "lighter", 2_000, 5_000.0, 500.0, 10.0)
            .unwrap();
        let rows = store.list_margin_snapshots("hyperliquid", 0).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].ts_ms, 1_000);
        assert_eq!(rows[1].rate_pct, 30.0);
        assert!(rows[0].ts_ms <= rows[1].ts_ms, "oldest first");
        let windowed = store.list_margin_snapshots("hyperliquid", 2_000).unwrap();
        assert_eq!(windowed.len(), 1);
        let removed = store.prune_margin_snapshots(2_000).unwrap();
        assert_eq!(removed, 1);
        assert_eq!(store.list_margin_snapshots("hyperliquid", 0).unwrap().len(), 1);
        assert_eq!(store.list_margin_snapshots("lighter", 0).unwrap().len(), 1);
        let removed_all = store.prune_margin_snapshots(3_000).unwrap();
        assert_eq!(removed_all, 2);
        assert!(store.list_margin_snapshots("lighter", 0).unwrap().is_empty());
    }

    #[test]
    fn test_margin_snapshots_reject_bad_rows() {
        let store = StateStore::open_in_memory().unwrap();
        assert!(store
            .record_margin_snapshot("", "a", 1_000, 10_000.0, 1_000.0, 10.0)
            .is_err());
        assert!(store
            .record_margin_snapshot("hyperliquid", "a", -1, 10_000.0, 1_000.0, 10.0)
            .is_err());
        assert!(store
            .record_margin_snapshot("hyperliquid", "a", 1_000, f64::NAN, 1_000.0, 10.0)
            .is_err());
        assert!(store
            .record_margin_snapshot("hyperliquid", "a", 1_000, 10_000.0, -5.0, 10.0)
            .is_err());
        assert!(store.list_margin_snapshots("hyperliquid", -1).is_err());
        assert!(store.prune_margin_snapshots(-1).is_err());
    }

    #[test]
    fn test_margin_snapshots_table_exists_after_migration() {
        let store = StateStore::open_in_memory().unwrap();
        let count: i64 = store
            .conn
            .lock()
            .query_row("SELECT COUNT(*) FROM margin_snapshots", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}
