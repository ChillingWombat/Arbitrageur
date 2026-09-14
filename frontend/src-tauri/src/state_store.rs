#![allow(dead_code)]

use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const ALLOWED_STRATEGY_IDS: [&str; 5] =
    ["futures", "funding", "spot", "prediction", "stat_arb"];

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
    pub created_at: String,
    pub updated_at: String,
}

fn default_account_tier() -> String {
    "standard".into()
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

const ACCOUNT_COLS: &str =
    "id, venue, label, enabled, is_default, account_tier, created_at, updated_at";

const ACCOUNT_BY_ID_SQL: &str = "\
SELECT id, venue, label, enabled, is_default, account_tier, created_at, updated_at
FROM exchange_accounts
WHERE id = ?1";

const ACCOUNT_ALL_SQL: &str = "\
SELECT id, venue, label, enabled, is_default, account_tier, created_at, updated_at
FROM exchange_accounts
ORDER BY venue COLLATE NOCASE, is_default DESC, label COLLATE NOCASE, id";

const ACCOUNT_BY_VENUE_SQL: &str = "\
SELECT id, venue, label, enabled, is_default, account_tier, created_at, updated_at
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
    (id, venue, label, enabled, is_default, account_tier, created_at, updated_at)
VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?6)
ON CONFLICT(id) DO UPDATE SET
    label = excluded.label,
    enabled = excluded.enabled,
    is_default = 0,
    account_tier = excluded.account_tier,
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

fn account_from_row(row: &Row<'_>) -> rusqlite::Result<AccountProfile> {
    Ok(AccountProfile {
        id: row.get(0)?,
        venue: row.get(1)?,
        label: row.get(2)?,
        enabled: row.get(3)?,
        is_default: row.get(4)?,
        account_tier: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
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
        side: LegSide::from_side(&side_raw).unwrap_or(LegSide::Long),
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
        base.join(".arbitrager").join("state.sqlite3")
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
        if version > 5 {
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
        Ok(())
    }

    pub fn upsert_account(
        &self,
        id: &str,
        venue: &str,
        label: &str,
        enabled: bool,
        make_default: bool,
        account_tier: &str,
    ) -> anyhow::Result<AccountProfile> {
        let id = normalize_id(id)?;
        let venue = normalize_venue(venue)?;
        let label = normalize_label(label)?;
        let account_tier = normalize_account_tier(account_tier)?;
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
            params![id, venue, label, enabled, account_tier, now],
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
                "SELECT id, venue, label, enabled, is_default, account_tier, created_at, updated_at
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
    pub fn spread_samples(
        &self,
        symbol: &str,
        from_ts_ms: i64,
        to_ts_ms: i64,
    ) -> anyhow::Result<Vec<SpreadSample>> {
        let symbol = normalize_symbol(symbol)?;
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT symbol, venue_id, ts_ms, mid, best_bid, best_ask
             FROM spread_samples
             WHERE symbol = ?1 AND ts_ms >= ?2 AND ts_ms <= ?3
             ORDER BY ts_ms ASC, venue_id ASC",
        )?;
        let rows = stmt.query_map(params![symbol, from_ts_ms, to_ts_ms], |r| {
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
    pub fn earliest_spread_sample_ts(&self, symbol: &str) -> anyhow::Result<Option<i64>> {
        let symbol = normalize_symbol(symbol)?;
        let conn = self.conn.lock();
        Ok(conn.query_row(
            "SELECT MIN(ts_ms) FROM spread_samples WHERE symbol = ?1",
            [&symbol],
            |r| r.get(0),
        )?)
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

    /// Upsert one pair signal's lifecycle state.
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
            "collecting" | "watching" | "signaled" | "qualified" | "stale" => {}
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
            "arbitrager_state_store_test_{}_{}",
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
            assert_eq!(version, 5);
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
    fn test_default_account_lifecycle() {
        let store = StateStore::open_in_memory().unwrap();
        let first = store
            .upsert_account("hl_main", "HyperLiquid", "Main", true, false, "standard")
            .unwrap();
        assert!(first.is_default);
        assert!(store.has_enabled_account("hyperliquid").unwrap());
        assert!(!store.has_enabled_account("lighter").unwrap());

        let second = store
            .upsert_account("hl_alt", "hyperliquid", "Alt", true, false, "standard")
            .unwrap();
        assert!(!second.is_default);
        assert_eq!(
            store.default_account("hyperliquid").unwrap().unwrap().id,
            "hl_main"
        );

        store
            .upsert_account("hl_zulu", "hyperliquid", "Zulu", true, false, "standard")
            .unwrap();
        store.set_default_account("hyperliquid", "hl_zulu").unwrap();
        assert!(store.get_account("hl_zulu").unwrap().unwrap().is_default);
        assert!(!store.get_account("hl_main").unwrap().unwrap().is_default);

        store
            .upsert_account("hl_zulu", "hyperliquid", "Zulu", false, false, "standard")
            .unwrap();
        let promoted = store.default_account("hyperliquid").unwrap().unwrap();
        assert_eq!(promoted.id, "hl_alt");

        store
            .upsert_account("hl_alt", "hyperliquid", "Alt", false, false, "standard")
            .unwrap();
        store
            .upsert_account("hl_main", "hyperliquid", "Main", false, false, "standard")
            .unwrap();
        assert!(store.default_account("hyperliquid").unwrap().is_none());
        assert!(!store.has_enabled_account("hyperliquid").unwrap());

        let reenabled = store
            .upsert_account("hl_alt", "hyperliquid", "Alt", true, false, "standard")
            .unwrap();
        assert!(reenabled.is_default);
    }

    #[test]
    fn test_delete_default_promotes_alphabetically_first_enabled() {
        let store = StateStore::open_in_memory().unwrap();
        store
            .upsert_account("a_main", "hyperliquid", "Main", true, true, "standard")
            .unwrap();
        store
            .upsert_account("a_zulu", "hyperliquid", "Zulu", true, false, "standard")
            .unwrap();
        store
            .upsert_account("a_beta", "hyperliquid", "Beta", true, false, "standard")
            .unwrap();
        assert!(store.delete_account("a_main").unwrap());
        let promoted = store.default_account("hyperliquid").unwrap().unwrap();
        assert_eq!(promoted.id, "a_beta");
    }

    #[test]
    fn test_validation_and_immutable_venue() {
        let store = StateStore::open_in_memory().unwrap();
        assert!(store
            .upsert_account("", "hyperliquid", "Main", true, false, "standard")
            .is_err());
        assert!(store
            .upsert_account("bad id", "hyperliquid", "Main", true, false, "standard")
            .is_err());
        assert!(store
            .upsert_account(
                &"x".repeat(129),
                "hyperliquid",
                "Main",
                true,
                false,
                "standard"
            )
            .is_err());
        assert!(store
            .upsert_account("a", "", "Main", true, false, "standard")
            .is_err());
        assert!(store
            .upsert_account("a", "bad venue", "Main", true, false, "standard")
            .is_err());
        assert!(store
            .upsert_account("a", "hyperliquid", "", true, false, "standard")
            .is_err());
        assert!(store
            .upsert_account("a", "hyperliquid", &"y".repeat(81), true, false, "standard")
            .is_err());
        assert!(store
            .upsert_account(
                "a",
                "hyperliquid",
                "bad\u{0007}label",
                true,
                false,
                "standard"
            )
            .is_err());

        store
            .upsert_account("acct1", "hyperliquid", "Main", true, false, "standard")
            .unwrap();
        assert!(store
            .upsert_account("acct1", "lighter", "Moved", true, false, "standard")
            .is_err());
        let renamed = store
            .upsert_account("acct1", "HyperLiquid", "Renamed", true, false, "standard")
            .unwrap();
        assert_eq!(renamed.venue, "hyperliquid");
        assert_eq!(renamed.label, "Renamed");
    }

    #[test]
    fn test_label_uniqueness_scoped_to_venue() {
        let store = StateStore::open_in_memory().unwrap();
        store
            .upsert_account("a1", "hyperliquid", "Main", true, false, "standard")
            .unwrap();
        assert!(store
            .upsert_account("a2", "hyperliquid", "MAIN", true, false, "standard")
            .is_err());
        store
            .upsert_account("a2", "lighter", "main", true, false, "standard")
            .unwrap();
        assert_eq!(store.list_accounts(None).unwrap().len(), 2);
    }

    #[test]
    fn test_route_rules_and_canonicalization() {
        let store = StateStore::open_in_memory().unwrap();
        store
            .upsert_account("hl_a", "hyperliquid", "HL A", true, false, "standard")
            .unwrap();
        store
            .upsert_account("hl_b", "hyperliquid", "HL B", true, false, "standard")
            .unwrap();
        store
            .upsert_account("lt_a", "lighter", "LT A", true, false, "standard")
            .unwrap();
        store
            .upsert_account("lt_off", "lighter", "LT Off", false, false, "standard")
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
            .upsert_account("hl_a", "hyperliquid", "HL A", true, false, "standard")
            .unwrap();
        store
            .upsert_account("lt_a", "lighter", "LT A", true, false, "standard")
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
            .upsert_account("hl_a", "hyperliquid", "HL A", true, false, "standard")
            .unwrap();
        store
            .upsert_account("hl_b", "hyperliquid", "HL B", true, false, "standard")
            .unwrap();
        store
            .upsert_account("lt_a", "lighter", "LT A", true, false, "standard")
            .unwrap();
        store
            .upsert_account("lt_b", "lighter", "LT B", true, false, "standard")
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
            .upsert_account("lt_b", "lighter", "LT B", false, false, "standard")
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
                .upsert_account("hl_a", "hyperliquid", "Main", true, false, "standard")
                .unwrap();
            store
                .upsert_account("lt_a", "lighter", "Alt", true, false, "standard")
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
            .upsert_account("hl_a", "hyperliquid", "Main", true, false, "standard")
            .unwrap();
        store
            .upsert_account("lt_a", "lighter", "Main", true, false, "standard")
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
            .upsert_account("hl_a", "hyperliquid", "Main", true, false, "standard")
            .unwrap();
        assert!(!store.account_has_signal_legs("hl_a").unwrap());
        store
            .record_signal_execution(&two_leg_signal("s9", "BTC-USDC", Some("hl_a"), None))
            .unwrap();
        assert!(store.account_has_signal_legs("hl_a").unwrap());
        assert!(store.delete_account("hl_a").is_err());
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
}
