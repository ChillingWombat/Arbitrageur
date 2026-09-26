#![allow(dead_code)]

use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const ALLOWED_STRATEGY_IDS: [&str; 12] = [
    "futures",
    "radar",
    "funding",
    "spot",
    "prediction",
    "stat_arb",
    "triangle",
    "options_parity",
    "options_box_spread",
    "options_jelly_roll",
    "options_convexity",
    "external",
];

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ExecutionLogEntry {
    pub id: String,
    pub timestamp: String,
    pub trade_type: String, // "Spatial" or "Funding" or "Prediction"
    /// Owning strategy for this fill event (`futures`, `radar`, `triangle`,
    /// ...). Rows that predate attribution carry `"unattributed"`: they are
    /// excluded from per-strategy sums and counted separately, never
    /// absorbed. `serde(default)` keeps old serialized rows loadable.
    #[serde(default = "default_unattributed_strategy")]
    pub strategy_id: String,
    pub symbol: String,
    pub venue_a: String,
    pub venue_b: String,
    pub leg_a_status: String,
    pub leg_b_status: String,
    pub success: bool,
    pub gross_profit: f64,
    pub fees: f64,
    pub net_profit: f64,
    pub latency_us: u64,
    pub note: String,
    #[serde(default)]
    pub direction: Option<String>,
    #[serde(default)]
    pub trigger_spread_bps: Option<f64>,
    #[serde(default)]
    pub baseline_mean_spread_bps: Option<f64>,
    #[serde(default)]
    pub position_step: Option<u32>,
    #[serde(default)]
    pub max_units: Option<u32>,
    #[serde(default)]
    pub nominal_hurdle_bps: Option<f64>,
    #[serde(default)]
    pub is_live: bool,
}

fn default_unattributed_strategy() -> String {
    crate::attribution::UNATTRIBUTED_STRATEGY_ID.to_string()
}

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
    /// hard ceiling of 100 still applies). Effective order-time leverage is
    /// min(strategy target, this cap, venue cap, Risk Control global max).
    #[serde(default)]
    pub max_leverage: Option<u32>,
    /// Which venue environment this account's credentials belong to:
    /// 'live' (production) or 'testnet' (the venue's public demo API).
    /// Admission and credential sync scope to the active environment, so a
    /// live key can never provision a testnet adapter or vice versa.
    #[serde(default = "default_account_environment")]
    pub environment: String,
    pub created_at: String,
    pub updated_at: String,
}

fn default_account_environment() -> String {
    "live".into()
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

fn equity_event_from_row(row: &Row<'_>) -> rusqlite::Result<AccountEquityEvent> {
    Ok(AccountEquityEvent {
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
    /// Exact execution symbol for this leg. Funding legs diverge from the
    /// parent root (long `BTC-SPOT` vs root `BTC-USDC`); backfilled from the
    /// parent root for pre-v22 rows.
    pub symbol: String,
    pub intended_qty: f64,
    pub filled_qty: f64,
    pub fill_price: Option<f64>,
    pub fee_usd: f64,
    pub venue_order_id: Option<String>,
    /// Engine-generated client order id, persisted alongside the venue
    /// order id so venues keyed on client order index (Lighter) can be
    /// polled by reconciliation after restart.
    pub client_order_id: Option<String>,
    /// Dispatch-time expected fill price (schema v27). `None` for legs
    /// recorded before v27 or without book context.
    pub intended_price: Option<f64>,
    /// Leg venue-units per unit of the parent signal's symbol (schema
    /// v39). Tracked-group legs fill in their own instrument's units —
    /// a QQQ leg on a NAS100 row carries ~39.4 — so delta and exposure
    /// reads divide by this scale to compare legs in shared units.
    /// `1.0` for every same-instrument and pre-v39 leg.
    pub unit_scale: f64,
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
    /// Trading environment this signal's legs execute in — `testnet` or
    /// `live` for v32+ rows; `unknown` for legacy rows whose account
    /// bindings could not attribute one. Environment-scoped reads treat
    /// `unknown` as visible to every environment (fail-closed: exposure
    /// that cannot be attributed still blocks and still gets polled).
    pub environment: String,
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

/// One fixed-yield YU lock's maturity tracker (v37). Lifecycle:
/// `locked` → `due` (wall-clock reached `lock_until_ms`) → `settled`
/// (Boros account read shows no residual position) or
/// `settlement_unverified` (venue unreachable or a position still
/// shows). `closed_early` is terminal — the YU legs exited through the
/// priced early-exit path before maturity. `resolved_action` records
/// the operator's explicit choice —
/// `close_pair` flattens the carry signal's perp legs; `keep_carry`
/// leaves them running as floating carry.
#[derive(Debug, Clone)]
pub struct FixedYieldMaturity {
    /// The `-yu` sibling signal id this tracker belongs to.
    pub signal_id: String,
    /// The carry signal the YU legs hedge.
    pub parent_signal_id: Option<String>,
    pub lock_until_ms: i64,
    /// Boros market ids of the YU legs — position matching after the
    /// market delists from active feeds falls back to these.
    pub yu_market_ids: Vec<u64>,
    pub maturity_state: String,
    pub resolved_action: Option<String>,
    /// Last venue-truth observation (positions seen, or the read error).
    pub venue_observation: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub resolved_at: Option<String>,
}

/// True when `child_id` is a `flat-{parent_id}-XXXX` or `trim-{parent_id}-XXXX`
/// family child: the trailing segment must be exactly the 4-hex sequence
/// suffix, so nested parents never collide (`flat-s-foo-0001` is a child of
/// `s-foo`, not `s`). `trim-` children are liquidation trims — family
/// members for exposure/residual math, but realized separately (the
/// cycle-lot close aggregation excludes them so a trimmed lot's remaining
/// quantity still matches the final flatten's fills).
pub fn is_flatten_child_id(child_id: &str, parent_id: &str) -> bool {
    let rest = child_id
        .strip_prefix("flat-")
        .or_else(|| child_id.strip_prefix("trim-"));
    rest.and_then(|rest| rest.strip_prefix(parent_id))
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|suf| suf.len() == 4 && suf.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Candidate persisted before dispatch; legs carry account attribution.
pub struct NewSignalLeg {
    pub account_id: Option<String>,
    pub venue: String,
    pub side: LegSide,
    /// Exact per-leg symbol. `None` resolves to the parent root symbol at
    /// record time; funding entry/flatten legs pass `Some(exact)`.
    pub symbol: Option<String>,
    pub intended_qty: f64,
    /// Dispatch-time expected fill price (the quote the order was priced
    /// against). `None` for legs recorded without book context.
    pub intended_price: Option<f64>,
    /// Leg venue-units per unit of the parent symbol. `None`/`Some(1.0)`
    /// for same-instrument legs; cross-instrument tracked-group legs
    /// carry the conversion the entry was sized under so delta and
    /// exposure reads compare legs in shared units.
    pub unit_scale: Option<f64>,
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

/// One persisted forecast attached to a signal execution
/// (signal_predictions table, v26). `kind` fixes the event definition the
/// probability describes; only kinds whose event is realized net P&L are
/// scored by the calibration report once `realized_net_bps` lands.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalPredictionRow {
    pub signal_id: String,
    /// Event semantics: 'net_win' = P(realized net bps > 0),
    /// 'reversion' = spread reverts inside the friction band,
    /// 'funding_persist' = funding gap survives the breakeven horizon,
    /// 'heuristic' = closed-form EV transform (not outcome-estimated).
    pub kind: String,
    pub predicted_p: f64,
    /// Expected net bps attached to the same forecast, when the strategy
    /// produced one alongside the probability.
    pub expected_net_bps: Option<f64>,
    pub strategy_id: String,
    pub symbol: String,
    /// Canonical "venueA/venueB" pair key for per-route calibration cuts.
    pub venue_pair: String,
    pub is_live: bool,
    pub created_at: String,
    /// Realized net P&L bps once the signal's position family closed with
    /// reconciled fills; `NULL` while unresolved.
    pub realized_net_bps: Option<f64>,
    pub resolved_at: Option<String>,
}

/// Kinds whose event is "realized net P&L > 0" — the only definitions a
/// net-bps outcome can honestly score.
pub const NET_PNL_PREDICTION_KINDS: &[&str] = &["net_win"];

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
    /// Top-5-level bid/ask quantities (base units) at sample time — the
    /// WFO/VWMA depth weight (schema v28). `None` for pre-v28 rows and
    /// record paths without book context.
    pub bid_depth: Option<f64>,
    pub ask_depth: Option<f64>,
}

/// Batch entry accepted by `record_spread_samples`.
#[derive(Clone)]
pub struct VenueSampleEntry {
    pub venue_id: String,
    pub mid: f64,
    pub best_bid: f64,
    pub best_ask: f64,
    /// Top-of-book depth proxies (top-5 qty sums, base units). `None` when
    /// the sampler has no book depth — persisted NULL, never fabricated.
    pub bid_depth: Option<f64>,
    pub ask_depth: Option<f64>,
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
    /// Owning trading environment — v38+. 'unknown' = pre-migration row.
    #[serde(default)]
    pub environment: String,
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
    /// Owning trading environment — v38+. 'unknown' = pre-migration row.
    #[serde(default)]
    pub environment: String,
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
    /// Owning trading environment — v38+. 'unknown' = pre-migration row.
    #[serde(default)]
    pub environment: String,
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
    /// Owning trading environment — v38+. 'unknown' = pre-migration row.
    #[serde(default)]
    pub environment: String,
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
    /// Owning trading environment — v38+. 'unknown' = pre-migration row.
    #[serde(default)]
    pub environment: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AutotuneApplyEventRow {
    pub id: i64,
    pub symbol: String,
    pub applied_json: String,
    pub detail_json: String,
    pub ts_ms: i64,
    /// Owning trading environment — v38+. 'unknown' = pre-migration row.
    pub environment: String,
}

/// One venue's signed net filled quantity inside an aggregated open position.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionVenueDto {
    pub venue: String,
    /// Exact leg symbol behind this row's net quantity.
    pub symbol: String,
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
    /// Durable exits-only flag for (environment, strategy, symbol) — set by
    /// the positions command post-aggregation, `false` in the raw group.
    pub reduce_only: bool,
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
    // Flatten children carry `flat-{parent}-xxxx` ids and single-venue legs —
    // their fills offset the PARENT's exposure, so they must join the parent's
    // group rather than form a separate (usually zero-fill) card.
    let by_id: std::collections::HashMap<&str, &SignalExecution> =
        signals.iter().map(|s| (s.id.as_str(), s)).collect();
    let venue_key_of = |s: &SignalExecution| -> String {
        let mut segs: Vec<String> = s
            .legs
            .iter()
            .map(|l| format!("{}:{}", l.venue.to_lowercase(), l.symbol))
            .collect();
        segs.sort();
        segs.dedup();
        segs.join("+")
    };
    for s in signals {
        // A `flat-*` child's fills offset its parent's exposure — count them
        // whenever the parent group is open, even if the child itself has
        // already closed (its fills are real reductions). An open parent's
        // own fills count while the parent is open.
        let owner: &SignalExecution = match s
            .id
            .strip_prefix("flat-")
            .or_else(|| s.id.strip_prefix("trim-"))
            .and_then(|t| t.rsplit_once('-').map(|(p, _)| p))
            .and_then(|pid| by_id.get(pid).copied())
        {
            Some(parent) => {
                if !parent.status.counts_as_open() {
                    continue;
                }
                parent
            }
            None => {
                if !s.status.counts_as_open() {
                    continue;
                }
                s
            }
        };
        let venue_key = venue_key_of(owner);
        groups
            .entry((
                owner.strategy_id.to_lowercase(),
                owner.symbol.clone(),
                venue_key,
            ))
            .or_default()
            .push(s);
    }

    let mut out = Vec::with_capacity(groups.len());
    for ((strategy_id, symbol, venue_key), members) in groups {
        let mut nets: BTreeMap<(String, String), f64> = BTreeMap::new();
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
                *nets
                    .entry((leg.venue.clone(), leg.symbol.clone()))
                    .or_insert(0.0) += signed;

                match (leg.fill_price, mark(&leg.venue, &leg.symbol)) {
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
            .map(|((v, sym), q)| PositionVenueDto {
                venue: v.clone(),
                symbol: sym.clone(),
                net_qty: *q,
            })
            .collect();
        if venue_rows.is_empty() {
            venue_rows = venue_key
                .split('+')
                .filter_map(|seg| seg.split_once(':'))
                .map(|(v, s)| PositionVenueDto {
                    venue: v.to_string(),
                    symbol: s.to_string(),
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
            reduce_only: false,
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
    /// Owning trading environment — v38+. 'unknown' = pre-migration row.
    pub environment: String,
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
    /// Owning trading environment — v38+. 'unknown' = pre-migration row.
    #[serde(default)]
    pub environment: String,
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
    /// Owning trading environment — v38+. 'unknown' rows stay visible to
    /// both environments (fail-closed: an unattributed position tracker
    /// must never go unrewatched).
    pub environment: String,
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

/// A one-shot user-scheduled position close (Portfolio Monitor → Close →
/// Schedule). `side` is the *position* side ('buy' = long, 'sell' = short);
/// the dispatch computes the opposing reduce-only close side. `mode`
/// mirrors the popup: 'auto' runs the passive limit unwind, 'limit' rests
/// at `limit_price`, 'market' takes immediately. Status lifecycle:
/// pending → fired | failed | cancelled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledExit {
    pub id: String,
    pub venue: String,
    pub symbol: String,
    pub side: String,
    pub close_pct: f64,
    pub mode: String,
    pub limit_price: Option<f64>,
    pub execute_at_ms: i64,
    pub status: String,
    /// Dispatch attempts so far — reschedules increment it; a row that
    /// exhausts its retry budget goes `failed` rather than retry forever.
    pub attempts: i64,
    pub note: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Owning trading environment — v38+. 'unknown' rows are listed for
    /// operator review but `due_scheduled_exits` never fires them: an
    /// exit whose environment cannot be attributed must not dispatch a
    /// real close under the wrong env's adapters.
    #[serde(default)]
    pub environment: String,
}

/// Separate row cap for the signal-event audit log. Retention shares the
/// spread-sample `max_days` cutoff (same setting, one pass), but the cap is
/// independent so a chatty pair can never evict price history — and price
/// pruning can never wipe the audit trail's newest window either.
pub const MONITOR_SIGNAL_EVENTS_MAX_ROWS: i64 = 50_000;

/// Hard row caps for the telemetry tables — enforced unconditionally on
/// every retention pass, independent of `spread_samples_max_days` (an
/// operator who disables day retention still cannot grow the file without
/// bound). Sized as backstops above what the 14-day default produces:
/// ~4M spread rows ≈ 650MB, ~1M volume rows ≈ 250MB.
pub const SPREAD_SAMPLES_MAX_ROWS: i64 = 4_000_000;
pub const VENUE_VOLUME_SAMPLES_MAX_ROWS: i64 = 1_000_000;

const ACCOUNT_COLS: &str = "id, venue, label, enabled, is_default, account_tier, max_leverage, environment, created_at, updated_at";

const ACCOUNT_BY_ID_SQL: &str = "\
SELECT id, venue, label, enabled, is_default, account_tier, max_leverage, environment, created_at, updated_at
FROM exchange_accounts
WHERE id = ?1";

const ACCOUNT_ALL_SQL: &str = "\
SELECT id, venue, label, enabled, is_default, account_tier, max_leverage, environment, created_at, updated_at
FROM exchange_accounts
ORDER BY venue COLLATE NOCASE, environment, is_default DESC, label COLLATE NOCASE, id";

const ACCOUNT_BY_VENUE_SQL: &str = "\
SELECT id, venue, label, enabled, is_default, account_tier, max_leverage, environment, created_at, updated_at
FROM exchange_accounts
WHERE venue = ?1
ORDER BY environment, is_default DESC, label COLLATE NOCASE, id";

const ENABLED_DEFAULT_BY_VENUE_SQL: &str = "\
SELECT id FROM exchange_accounts
WHERE venue = ?1 AND environment = ?2 AND is_default = 1 AND enabled = 1
LIMIT 1";

const FALLBACK_DEFAULT_SQL: &str = "\
SELECT id FROM exchange_accounts
WHERE venue = ?1 AND environment = ?2 AND enabled = 1
ORDER BY label COLLATE NOCASE, id
LIMIT 1";

const CLEAR_DEFAULTS_SQL: &str = "\
UPDATE exchange_accounts SET is_default = 0, updated_at = ?2 WHERE venue = ?1 AND environment = ?3";

const SET_DEFAULT_SQL: &str = "\
UPDATE exchange_accounts SET is_default = 1, updated_at = ?2
WHERE venue = ?1 AND environment = ?4 AND id = ?3 AND enabled = 1";

const ACCOUNT_UPSERT_SQL: &str = "\
INSERT INTO exchange_accounts
    (id, venue, label, enabled, is_default, account_tier, max_leverage, environment, created_at, updated_at)
VALUES (?1, ?2, ?3, ?4, 0, ?5, ?7, ?8, ?6, ?6)
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

/// Telemetry-file schema: `spread_samples` at its post-v28 shape (depth
/// columns included). Versioned implicitly by DDL — `IF NOT EXISTS` keeps
/// the file forward-compatible without a migration chain of its own.
const TELEMETRY_SCHEMA_SQL: &str = "\
CREATE TABLE IF NOT EXISTS spread_samples (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    venue_id TEXT NOT NULL COLLATE NOCASE CHECK(length(venue_id) BETWEEN 1 AND 64),
    ts_ms INTEGER NOT NULL CHECK(ts_ms >= 0),
    mid REAL NOT NULL CHECK(mid > 0),
    best_bid REAL NOT NULL,
    best_ask REAL NOT NULL,
    bid_depth REAL,
    ask_depth REAL,
    environment TEXT NOT NULL DEFAULT 'unknown'
        CHECK(environment IN ('unknown','testnet','live'))
);
CREATE INDEX IF NOT EXISTS ix_spread_samples_symbol_ts ON spread_samples(symbol, ts_ms);";

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
/// hard ceiling of 100 still applies); otherwise 1..=5, enforced at order
/// time as min(strategy target, account cap, venue cap, Risk Control max).
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
/// v20 repairs Lighter-family funding history: the venue `/funding-rates`
/// `rate` is an 8-hour print but was persisted with `interval_hours = 1`,
/// inflating stored `rate_8h` 8x. Rescale those rows and mark interval 8.
/// New samples are correct at write time; this is a one-time backfill.
const MIGRATION_V20_SQL: &str = "BEGIN IMMEDIATE; UPDATE funding_rate_samples SET rate_8h = rate_8h / 8.0, interval_hours = 8 WHERE LOWER(venue_id) IN ('lighter', 'robinhood') AND interval_hours = 1; PRAGMA user_version = 20; COMMIT;";

/// v21 persists the execution log (Log Monitor source). Previously the log
/// lived only in memory: every restart wiped it, and — worse — execution
/// *failures* were never recorded at all, so a blocked click left no trace
/// anywhere. New rows land via `insert_execution_log` (called by the shared
/// record helper); reads take the newest N.
const MIGRATION_V21_SQL: &str = "BEGIN IMMEDIATE; CREATE TABLE IF NOT EXISTS execution_log (id TEXT PRIMARY KEY, ts TEXT NOT NULL DEFAULT '', trade_type TEXT NOT NULL DEFAULT '', strategy_id TEXT NOT NULL DEFAULT '', symbol TEXT NOT NULL DEFAULT '', venue_a TEXT NOT NULL DEFAULT '', venue_b TEXT NOT NULL DEFAULT '', leg_a_status TEXT NOT NULL DEFAULT '', leg_b_status TEXT NOT NULL DEFAULT '', success INTEGER NOT NULL DEFAULT 0, gross_profit REAL NOT NULL DEFAULT 0, fees REAL NOT NULL DEFAULT 0, net_profit REAL NOT NULL DEFAULT 0, latency_us INTEGER NOT NULL DEFAULT 0, note TEXT NOT NULL DEFAULT '', direction TEXT, trigger_spread_bps REAL, baseline_mean_spread_bps REAL, position_step INTEGER, max_units INTEGER, nominal_hurdle_bps REAL, is_live INTEGER NOT NULL DEFAULT 0); CREATE INDEX IF NOT EXISTS idx_execution_log_ts ON execution_log(ts); PRAGMA user_version = 21; COMMIT;";

/// v22 adds the per-leg exact execution symbol. Pre-v22 rows carry the
/// parent root on every leg (funding entries then shared the root), so the
/// backfill copies each leg's symbol from its parent `signal_executions`
/// row — v21 data reads back identically, with symbols populated.
const MIGRATION_V22_SQL: &str = "BEGIN IMMEDIATE; ALTER TABLE signal_legs ADD COLUMN symbol TEXT NOT NULL DEFAULT ''; UPDATE signal_legs SET symbol = (SELECT symbol FROM signal_executions WHERE signal_executions.id = signal_legs.signal_id) WHERE symbol = ''; PRAGMA user_version = 22; COMMIT;";

const MIGRATION_V23_SQL: &str = "BEGIN IMMEDIATE; ALTER TABLE signal_legs ADD COLUMN client_order_id TEXT NOT NULL DEFAULT ''; PRAGMA user_version = 23; COMMIT;";

/// v24 admits 'external' into the signal_executions strategy CHECK. The
/// external-close flow has always persisted signals with strategy_id
/// 'external' (ALLOWED_STRATEGY_IDS admits it) but every prior rebuild of
/// the table omitted it from the CHECK — external closes failed to persist
/// with a constraint error before ever dispatching. The rebuild mirrors
/// v13 exactly, widened by one value.
const MIGRATION_V24_SQL: &str = "\
PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;
CREATE TABLE signal_executions_v24 (
    id TEXT PRIMARY KEY NOT NULL CHECK(length(id) BETWEEN 1 AND 128),
    strategy_id TEXT NOT NULL COLLATE NOCASE CHECK(strategy_id IN ('futures', 'radar', 'funding', 'spot', 'prediction', 'stat_arb', 'triangle', 'external')),
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
INSERT INTO signal_executions_v24
    (id, strategy_id, symbol, status, route_id, account_long_id,
     account_short_id, expected_net_usd, fees_usd, note,
     created_at, updated_at, closed_at)
    SELECT id, strategy_id, symbol, status, route_id, account_long_id,
           account_short_id, expected_net_usd, fees_usd, note,
           created_at, updated_at, closed_at
    FROM signal_executions;
DROP TABLE signal_executions;
ALTER TABLE signal_executions_v24 RENAME TO signal_executions;
CREATE INDEX ix_signal_executions_symbol_status
    ON signal_executions(symbol, status);
PRAGMA user_version = 24;
COMMIT;
PRAGMA foreign_keys = ON;";

/// v25 adds the scheduled-exit table: one-shot, user-requested position
/// closes that fire at a wall-clock time (e.g. just before a funding
/// timestamp). Each row captures the venue/symbol/position-side identity,
/// the fraction to close, the exit mode the popup selected
/// (auto-passive / manual limit / market), and a status lifecycle
/// (pending → fired | failed | cancelled) so a restart re-arms pending
/// rows without duplicating an execution that already dispatched.
const MIGRATION_V25_SQL: &str = "\
BEGIN IMMEDIATE;
CREATE TABLE scheduled_exits (
    id TEXT PRIMARY KEY NOT NULL CHECK(length(id) BETWEEN 1 AND 64),
    venue TEXT NOT NULL COLLATE NOCASE CHECK(length(venue) BETWEEN 1 AND 64),
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    side TEXT NOT NULL CHECK(side IN ('buy', 'sell')),
    close_pct REAL NOT NULL CHECK(close_pct > 0 AND close_pct <= 100),
    mode TEXT NOT NULL CHECK(mode IN ('auto', 'limit', 'market')),
    limit_price REAL CHECK(limit_price IS NULL OR limit_price > 0),
    execute_at_ms INTEGER NOT NULL CHECK(execute_at_ms >= 0),
    status TEXT NOT NULL CHECK(status IN ('pending', 'firing', 'fired', 'failed', 'cancelled')),
    note TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX ix_scheduled_exits_status_due ON scheduled_exits(status, execute_at_ms);
PRAGMA user_version = 25;
COMMIT;";

/// v26 adds the durable prediction log feeding the calibration report.
/// Every execution that carried a forecast probability records it here at
/// dispatch, keyed to the signal that owns it; when the position family
/// closes with a reconciled realized P&L the row resolves and becomes
/// scorable. `kind` preserves the event definition — only kinds whose
/// event is defined on net P&L score against realized net bps, so a
/// reversion forecast can never masquerade as a profit forecast.
const MIGRATION_V26_SQL: &str = "\
BEGIN IMMEDIATE;
CREATE TABLE signal_predictions (
    signal_id TEXT NOT NULL REFERENCES signal_executions(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK(kind IN ('net_win', 'reversion', 'funding_persist', 'heuristic')),
    predicted_p REAL NOT NULL CHECK(predicted_p >= 0.0 AND predicted_p <= 1.0),
    expected_net_bps REAL,
    strategy_id TEXT NOT NULL COLLATE NOCASE,
    symbol TEXT NOT NULL COLLATE NOCASE,
    venue_pair TEXT NOT NULL DEFAULT '',
    is_live INTEGER NOT NULL DEFAULT 0 CHECK(is_live IN (0, 1)),
    created_at TEXT NOT NULL,
    realized_net_bps REAL,
    resolved_at TEXT,
    PRIMARY KEY (signal_id, kind)
);
CREATE INDEX ix_signal_predictions_resolved ON signal_predictions(kind, resolved_at);
PRAGMA user_version = 26;
COMMIT;";

/// v27 persists the dispatch-time expected fill price per leg. Realized
/// slippage (fill vs intended, direction-aware) then becomes measurable
/// from the ledger instead of being inferred from net-P&L forecast error.
const MIGRATION_V27_SQL: &str = "\
BEGIN IMMEDIATE;
ALTER TABLE signal_legs ADD COLUMN intended_price REAL;
PRAGMA user_version = 27;
COMMIT;";

/// v28 adds top-of-book depth proxies (top-5 qty sums, base units) to
/// spread_samples so the WFO edge series can weight observations by real
/// depth instead of the equal-weight 1.0 stub.
const MIGRATION_V28_SQL: &str = "\
BEGIN IMMEDIATE;
ALTER TABLE spread_samples ADD COLUMN bid_depth REAL;
ALTER TABLE spread_samples ADD COLUMN ask_depth REAL;
PRAGMA user_version = 28;
COMMIT;";

/// v29 adds `exchange_accounts.environment` ('live' | 'testnet') — the two
/// credential sets model: an account belongs to exactly one environment and
/// venue admission/credential sync scope to the active one. Existing rows
/// default to 'live'. The table is rebuilt because the old UNIQUE(venue,
/// label) must widen to (venue, label, environment) — the same account
/// label may legitimately exist once per environment — and the one-default
/// partial index becomes per-(venue, environment).
const MIGRATION_V29_SQL: &str = "\
PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;
CREATE TABLE exchange_accounts_v29 (
    id TEXT PRIMARY KEY NOT NULL CHECK(length(id) BETWEEN 1 AND 128),
    venue TEXT NOT NULL COLLATE NOCASE CHECK(length(venue) BETWEEN 1 AND 64),
    label TEXT NOT NULL COLLATE NOCASE CHECK(length(label) BETWEEN 1 AND 80),
    enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
    is_default INTEGER NOT NULL CHECK(is_default IN (0, 1)),
    account_tier TEXT NOT NULL DEFAULT 'standard'
        CHECK(account_tier IN ('standard', 'plus', 'premium', 'builder')),
    max_leverage INTEGER CHECK(max_leverage IS NULL OR max_leverage BETWEEN 1 AND 5),
    environment TEXT NOT NULL DEFAULT 'live' CHECK(environment IN ('live', 'testnet')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(venue, label, environment)
);
INSERT INTO exchange_accounts_v29
    (id, venue, label, enabled, is_default, account_tier, max_leverage,
     environment, created_at, updated_at)
    SELECT id, venue, label, enabled, is_default, account_tier, max_leverage,
           'live', created_at, updated_at
    FROM exchange_accounts;
DROP TABLE exchange_accounts;
ALTER TABLE exchange_accounts_v29 RENAME TO exchange_accounts;
CREATE UNIQUE INDEX ux_exchange_accounts_one_default
    ON exchange_accounts(venue, environment) WHERE is_default = 1;
CREATE INDEX ix_exchange_accounts_venue_enabled
    ON exchange_accounts(venue, environment, enabled);
PRAGMA user_version = 29;
COMMIT;
PRAGMA foreign_keys = ON;";

/// v30 makes the virtual cycle ledger durable. Cycle lots were previously
/// in-memory only — a restart lost every open lot, so post-restart flattens
/// could not match a lot (no realized PnL, no prediction resolution, no
/// counter accumulation) and strategy metrics forgot all history. The table
/// mirrors `ArbitrageCycleLot`; closed lots are retained for metrics.
const MIGRATION_V30_SQL: &str = "\
BEGIN IMMEDIATE;
CREATE TABLE cycle_lots (
    id TEXT PRIMARY KEY NOT NULL CHECK(length(id) BETWEEN 1 AND 128),
    strategy_id TEXT NOT NULL COLLATE NOCASE,
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    venue_long TEXT NOT NULL COLLATE NOCASE CHECK(length(venue_long) BETWEEN 1 AND 64),
    venue_short TEXT NOT NULL COLLATE NOCASE CHECK(length(venue_short) BETWEEN 1 AND 64),
    entry_time TEXT NOT NULL,
    entry_spread_bps REAL NOT NULL,
    long_qty REAL NOT NULL CHECK(long_qty >= 0),
    long_entry_price REAL NOT NULL CHECK(long_entry_price >= 0),
    short_qty REAL NOT NULL CHECK(short_qty >= 0),
    short_entry_price REAL NOT NULL CHECK(short_entry_price >= 0),
    fees_paid_usd REAL NOT NULL DEFAULT 0 CHECK(fees_paid_usd >= 0),
    status TEXT NOT NULL CHECK(status IN ('open', 'closed', 'stopped_out', 'timed_out')),
    close_time TEXT,
    close_spread_bps REAL,
    realized_pnl_usd REAL,
    realized_pnl_bps REAL
);
CREATE INDEX ix_cycle_lots_status ON cycle_lots(status);
CREATE INDEX ix_cycle_lots_strategy_symbol ON cycle_lots(strategy_id, symbol);
PRAGMA user_version = 30;
COMMIT;";

/// v31 gives scheduled exits a retry counter. A claimed row that fails on
/// a transient condition (venue unreachable, locked vault before the
/// auto-lock-aware close gate) reschedules itself pending with backoff
/// instead of dying on the first failure — bounded by `attempts`.
const MIGRATION_V31_SQL: &str = "\
BEGIN IMMEDIATE;
ALTER TABLE scheduled_exits ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0;
PRAGMA user_version = 31;
COMMIT;";

/// v32 adds a durable environment dimension to signal_executions so live
/// and testnet exposure/performance never blend. Rows whose legs bind an
/// account backfill that account's environment; anything that cannot be
/// attributed stays `unknown` — environment-scoped reads treat `unknown`
/// as visible to every environment (fail-closed), while P&L aggregation
/// excludes it from both cuts rather than guessing.
const MIGRATION_V32_SQL: &str = "\
BEGIN IMMEDIATE;
ALTER TABLE signal_executions ADD COLUMN environment TEXT NOT NULL DEFAULT 'unknown'
    CHECK(environment IN ('unknown', 'testnet', 'live'));
UPDATE signal_executions SET environment = COALESCE((
    SELECT a.environment FROM signal_legs l
    JOIN exchange_accounts a ON a.id = l.account_id
    WHERE l.signal_id = signal_executions.id
      AND a.environment IN ('testnet', 'live')
    ORDER BY l.created_at LIMIT 1
), 'unknown')
WHERE environment = 'unknown';
CREATE INDEX ix_signal_executions_env ON signal_executions(environment);
PRAGMA user_version = 32;
COMMIT;";

/// v33 adds `cycle_lots.environment` — the virtual-cycle ledger aggregates
/// strategy metrics per environment, so each persisted lot carries the
/// environment that opened it. Lots have no account bindings to backfill
/// from, so pre-v33 rows stay `unknown` and are excluded from both
/// environments' metrics rather than guessed into one.
const MIGRATION_V33_SQL: &str = "\
BEGIN IMMEDIATE;
ALTER TABLE cycle_lots ADD COLUMN environment TEXT NOT NULL DEFAULT 'unknown'
    CHECK(environment IN ('unknown', 'testnet', 'live'));
PRAGMA user_version = 33;
COMMIT;";

/// v34 enables incremental auto-vacuum so pages freed by retention deletes
/// can return to the OS via `PRAGMA incremental_vacuum`. The pragma only
/// takes effect when set before schema creation or paired with VACUUM, so
/// this step runs both — deliberately WITHOUT a transaction wrapper
/// (VACUUM cannot run inside one). It is one-time by construction: the
/// pragma lands in the DB header and the rebuild writes pointer maps;
/// failure leaves user_version at 33 and retries on next open.
const MIGRATION_V34_SQL: &str = "\
PRAGMA auto_vacuum = INCREMENTAL;
VACUUM;
PRAGMA user_version = 34;";

/// v35 adds `ticker_reduce_only`: a server-side per-(environment, strategy,
/// symbol) exits-only flag. The previous client-side localStorage flag could
/// be bypassed by any manual entry path and was not environment-scoped —
/// reduce-only must be durable state that entry gates enforce.
const MIGRATION_V35_SQL: &str = "\
BEGIN IMMEDIATE;
CREATE TABLE ticker_reduce_only (
    environment TEXT NOT NULL CHECK(environment IN ('testnet','live')),
    strategy_id TEXT NOT NULL CHECK(length(strategy_id) BETWEEN 1 AND 64),
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    set_ts_ms INTEGER NOT NULL CHECK(set_ts_ms >= 0),
    PRIMARY KEY (environment, strategy_id, symbol)
);
PRAGMA user_version = 35;
COMMIT;";

/// v36 admits 'options_parity' into the signal_executions strategy CHECK —
/// the Deribit options-parity executor persists one leg-intent signal per
/// dispatched leg. The rebuild mirrors v24 exactly, widened by one value
/// and carrying the v32 `environment` column forward.
const MIGRATION_V36_SQL: &str = "\
PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;
CREATE TABLE signal_executions_v36 (
    id TEXT PRIMARY KEY NOT NULL CHECK(length(id) BETWEEN 1 AND 128),
    strategy_id TEXT NOT NULL COLLATE NOCASE CHECK(strategy_id IN ('futures', 'radar', 'funding', 'spot', 'prediction', 'stat_arb', 'triangle', 'options_parity', 'external')),
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
    closed_at TEXT,
    environment TEXT NOT NULL DEFAULT 'unknown' CHECK(environment IN ('unknown', 'testnet', 'live'))
);
INSERT INTO signal_executions_v36
    (id, strategy_id, symbol, status, route_id, account_long_id,
     account_short_id, expected_net_usd, fees_usd, note,
     created_at, updated_at, closed_at, environment)
    SELECT id, strategy_id, symbol, status, route_id, account_long_id,
           account_short_id, expected_net_usd, fees_usd, note,
           created_at, updated_at, closed_at, environment
    FROM signal_executions;
DROP TABLE signal_executions;
ALTER TABLE signal_executions_v36 RENAME TO signal_executions;
CREATE INDEX ix_signal_executions_symbol_status
    ON signal_executions(symbol, status);
CREATE INDEX ix_signal_executions_env ON signal_executions(environment);
PRAGMA user_version = 36;
COMMIT;
PRAGMA foreign_keys = ON;";

/// v37 adds the fixed-yield maturity ledger: one row per `-yu` sibling
/// signal recording the Boros YU lock's maturity, the venue-truth
/// settlement state, and the operator's explicit resolution. Wall-clock
/// maturity alone never marks a row settled — only a Boros account read
/// showing no residual position does. `yu_market_ids` survives the
/// market's delisting from `isMatured=false` feeds so a still-open
/// position remains matchable after expiry.
const MIGRATION_V37_SQL: &str = "\
BEGIN IMMEDIATE;
CREATE TABLE fixed_yield_maturities (
    signal_id TEXT PRIMARY KEY REFERENCES signal_executions(id) ON DELETE CASCADE,
    parent_signal_id TEXT REFERENCES signal_executions(id) ON DELETE SET NULL,
    lock_until_ms INTEGER NOT NULL CHECK(lock_until_ms >= 0),
    yu_market_ids TEXT NOT NULL DEFAULT '',
    maturity_state TEXT NOT NULL DEFAULT 'locked'
        CHECK(maturity_state IN ('locked','due','settled','settlement_unverified','closed_early')),
    resolved_action TEXT
        CHECK(resolved_action IS NULL OR resolved_action IN ('close_pair','keep_carry')),
    venue_observation TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    resolved_at TEXT
);
PRAGMA user_version = 37;
COMMIT;";

/// v38 environment-scopes every venue-data table: live and testnet write
/// the same (venue_id, symbol) key space, so without an environment
/// column a testnet sample/signal/position row is indistinguishable from
/// a mainnet one — charts, baselines, seasonality, funding history, and
/// bot exit watchers would silently mix demo data into live decisions.
///
/// Tables whose PK can collide across environments are rebuilt with
/// `environment` inside the primary key (v19/v36 pattern); append-only
/// and id-keyed tables get a plain column. Copied rows are stamped
/// 'unknown': pre-migration data cannot be attributed to an environment,
/// so environment-filtered reads exclude it rather than guessing.
/// `radar_bot_positions` is the deliberate exception — an unattributed
/// open-position tracker stays visible to BOTH environments (fail-closed,
/// mirroring v32 signal semantics): an unwatched real position is worse
/// than a watched ambiguous one.
/// v39 adds `signal_legs.unit_scale`: the leg's venue-unit quantity per
/// unit of the parent signal's symbol. Cross-instrument tracked-group
/// pairs (index perp ↔ ETF perp — long 39.4 QQQ vs short 1.0 NAS100)
/// store fills in venue-native units, so per-signal delta and ticker net
/// position must divide by this scale to compare legs in shared units.
/// Same-instrument legs keep the default 1.0 — the queries are identical
/// for every pre-v39 row.
const MIGRATION_V39_SQL: &str = "\
BEGIN IMMEDIATE;
ALTER TABLE signal_legs ADD COLUMN unit_scale
    REAL NOT NULL DEFAULT 1.0 CHECK(unit_scale > 0);
PRAGMA user_version = 39;
COMMIT;";

/// v40 adds the per-substrategy options bot config — one JSON row per
/// (environment, substrategy). Testnet and live bots arm independently:
/// arming the testnet parity bot never touches the live row.
const MIGRATION_V40_SQL: &str = "\
BEGIN IMMEDIATE;
CREATE TABLE options_bot_config (
    environment TEXT NOT NULL CHECK(environment IN ('testnet','live')),
    substrategy_id TEXT NOT NULL COLLATE NOCASE
        CHECK(length(substrategy_id) BETWEEN 1 AND 64),
    config_json TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (environment, substrategy_id)
);
PRAGMA user_version = 40;
COMMIT;";

/// v41 admits the three options-package strategy ids ('options_box_spread',
/// 'options_jelly_roll', 'options_convexity') into the signal_executions
/// strategy CHECK — the Deribit package executor persists one leg-intent
/// signal per dispatched leg under `options_{sub}`. The rebuild mirrors
/// v36 exactly, widened by three values and carrying the same columns.
const MIGRATION_V41_SQL: &str = "\
PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;
CREATE TABLE signal_executions_v41 (
    id TEXT PRIMARY KEY NOT NULL CHECK(length(id) BETWEEN 1 AND 128),
    strategy_id TEXT NOT NULL COLLATE NOCASE CHECK(strategy_id IN ('futures', 'radar', 'funding', 'spot', 'prediction', 'stat_arb', 'triangle', 'options_parity', 'options_box_spread', 'options_jelly_roll', 'options_convexity', 'external')),
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
    closed_at TEXT,
    environment TEXT NOT NULL DEFAULT 'unknown' CHECK(environment IN ('unknown', 'testnet', 'live'))
);
INSERT INTO signal_executions_v41
    (id, strategy_id, symbol, status, route_id, account_long_id,
     account_short_id, expected_net_usd, fees_usd, note,
     created_at, updated_at, closed_at, environment)
    SELECT id, strategy_id, symbol, status, route_id, account_long_id,
           account_short_id, expected_net_usd, fees_usd, note,
           created_at, updated_at, closed_at, environment
    FROM signal_executions;
DROP TABLE signal_executions;
ALTER TABLE signal_executions_v41 RENAME TO signal_executions;
CREATE INDEX ix_signal_executions_symbol_status
    ON signal_executions(symbol, status);
CREATE INDEX ix_signal_executions_env ON signal_executions(environment);
PRAGMA user_version = 41;
COMMIT;
PRAGMA foreign_keys = ON;";

const MIGRATION_V38_SQL: &str = "\
PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;
CREATE TABLE monitor_signals_v38 (
    symbol TEXT NOT NULL COLLATE NOCASE,
    buy_venue TEXT NOT NULL COLLATE NOCASE,
    sell_venue TEXT NOT NULL COLLATE NOCASE,
    status TEXT NOT NULL CHECK(status IN ('preparing','tracking','triggered','ready','no-data')),
    first_seen_ms INTEGER NOT NULL CHECK(first_seen_ms >= 0),
    last_seen_ms INTEGER NOT NULL CHECK(last_seen_ms >= 0),
    last_edge_bps REAL,
    last_dev_bps REAL,
    updated_at TEXT NOT NULL,
    environment TEXT NOT NULL DEFAULT 'unknown'
        CHECK(environment IN ('unknown','testnet','live')),
    PRIMARY KEY (symbol, buy_venue, sell_venue, environment)
);
INSERT INTO monitor_signals_v38
    (symbol, buy_venue, sell_venue, status, first_seen_ms, last_seen_ms,
     last_edge_bps, last_dev_bps, updated_at)
    SELECT symbol, buy_venue, sell_venue, status, first_seen_ms, last_seen_ms,
           last_edge_bps, last_dev_bps, updated_at FROM monitor_signals;
DROP TABLE monitor_signals;
ALTER TABLE monitor_signals_v38 RENAME TO monitor_signals;
ALTER TABLE monitor_signal_events ADD COLUMN environment
    TEXT NOT NULL DEFAULT 'unknown' CHECK(environment IN ('unknown','testnet','live'));
CREATE TABLE radar_bot_positions_v38 (
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
    environment TEXT NOT NULL DEFAULT 'unknown'
        CHECK(environment IN ('unknown','testnet','live')),
    PRIMARY KEY (symbol, buy_venue, sell_venue, environment)
);
INSERT INTO radar_bot_positions_v38
    (symbol, buy_venue, sell_venue, entry_ts_ms, entry_dev_bps, baseline_bps,
     friction_band_bps, size_usd, deadline_ms, unwatched_since_ms, last_seen_ms)
    SELECT symbol, buy_venue, sell_venue, entry_ts_ms, entry_dev_bps,
           baseline_bps, friction_band_bps, size_usd, deadline_ms,
           unwatched_since_ms, last_seen_ms FROM radar_bot_positions;
DROP TABLE radar_bot_positions;
ALTER TABLE radar_bot_positions_v38 RENAME TO radar_bot_positions;
ALTER TABLE scheduled_exits ADD COLUMN environment
    TEXT NOT NULL DEFAULT 'unknown' CHECK(environment IN ('unknown','testnet','live'));
CREATE TABLE venue_volume_samples_v38 (
    venue_id TEXT NOT NULL COLLATE NOCASE CHECK(length(venue_id) BETWEEN 1 AND 64),
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    ts_ms INTEGER NOT NULL CHECK(ts_ms >= 0),
    vol_usd REAL NOT NULL CHECK(vol_usd > 0),
    environment TEXT NOT NULL DEFAULT 'unknown'
        CHECK(environment IN ('unknown','testnet','live')),
    PRIMARY KEY (venue_id, symbol, ts_ms, environment)
);
INSERT INTO venue_volume_samples_v38 (venue_id, symbol, ts_ms, vol_usd)
    SELECT venue_id, symbol, ts_ms, vol_usd FROM venue_volume_samples;
DROP TABLE venue_volume_samples;
ALTER TABLE venue_volume_samples_v38 RENAME TO venue_volume_samples;
CREATE INDEX ix_venue_volume_samples_symbol_ts ON venue_volume_samples(symbol, ts_ms);
CREATE TABLE funding_rate_samples_v38 (
    venue_id TEXT NOT NULL COLLATE NOCASE CHECK(length(venue_id) BETWEEN 1 AND 64),
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    ts_ms INTEGER NOT NULL CHECK(ts_ms >= 0),
    rate_8h REAL NOT NULL CHECK(rate_8h IS NOT NULL),
    interval_hours INTEGER NOT NULL CHECK(interval_hours >= 1),
    environment TEXT NOT NULL DEFAULT 'unknown'
        CHECK(environment IN ('unknown','testnet','live')),
    PRIMARY KEY (venue_id, symbol, ts_ms, environment)
);
INSERT INTO funding_rate_samples_v38 (venue_id, symbol, ts_ms, rate_8h, interval_hours)
    SELECT venue_id, symbol, ts_ms, rate_8h, interval_hours FROM funding_rate_samples;
DROP TABLE funding_rate_samples;
ALTER TABLE funding_rate_samples_v38 RENAME TO funding_rate_samples;
CREATE INDEX ix_funding_rate_samples_symbol_ts ON funding_rate_samples(symbol, ts_ms);
CREATE INDEX ix_funding_rate_samples_venue_symbol_ts ON funding_rate_samples(venue_id, symbol, ts_ms);
CREATE TABLE session_profiles_v38 (
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
    environment TEXT NOT NULL DEFAULT 'unknown'
        CHECK(environment IN ('unknown','testnet','live')),
    PRIMARY KEY (symbol, session_id, environment)
);
INSERT INTO session_profiles_v38
    (symbol, session_id, n_episodes, med_abs_spread_bps, p90_spread_bps,
     rv_bps, win_rate, mean_hold_s, provisional, updated_ts)
    SELECT symbol, session_id, n_episodes, med_abs_spread_bps, p90_spread_bps,
           rv_bps, win_rate, mean_hold_s, provisional, updated_ts FROM session_profiles;
DROP TABLE session_profiles;
ALTER TABLE session_profiles_v38 RENAME TO session_profiles;
CREATE TABLE session_hour_bins_v38 (
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    hour INTEGER NOT NULL CHECK(hour BETWEEN 0 AND 23),
    n_obs INTEGER NOT NULL CHECK(n_obs >= 0),
    med_spread_bps REAL,
    modulator REAL NOT NULL,
    provisional INTEGER NOT NULL CHECK(provisional IN (0, 1)),
    updated_ts INTEGER NOT NULL CHECK(updated_ts >= 0),
    environment TEXT NOT NULL DEFAULT 'unknown'
        CHECK(environment IN ('unknown','testnet','live')),
    PRIMARY KEY (symbol, hour, environment)
);
INSERT INTO session_hour_bins_v38
    (symbol, hour, n_obs, med_spread_bps, modulator, provisional, updated_ts)
    SELECT symbol, hour, n_obs, med_spread_bps, modulator, provisional, updated_ts
    FROM session_hour_bins;
DROP TABLE session_hour_bins;
ALTER TABLE session_hour_bins_v38 RENAME TO session_hour_bins;
CREATE TABLE har_anchors_v38 (
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    intercept REAL NOT NULL,
    b_daily REAL NOT NULL,
    b_weekly REAL NOT NULL,
    b_monthly REAL NOT NULL,
    r2 REAL NOT NULL CHECK(r2 >= 0.0 AND r2 <= 1.0),
    n_days INTEGER NOT NULL CHECK(n_days >= 0),
    forecast_rv_bps REAL NOT NULL CHECK(forecast_rv_bps > 0),
    trailing_mean_rv_bps REAL NOT NULL CHECK(trailing_mean_rv_bps > 0),
    updated_ts INTEGER NOT NULL CHECK(updated_ts >= 0),
    environment TEXT NOT NULL DEFAULT 'unknown'
        CHECK(environment IN ('unknown','testnet','live')),
    PRIMARY KEY (symbol, environment)
);
INSERT INTO har_anchors_v38
    (symbol, intercept, b_daily, b_weekly, b_monthly, r2, n_days,
     forecast_rv_bps, trailing_mean_rv_bps, updated_ts)
    SELECT symbol, intercept, b_daily, b_weekly, b_monthly, r2, n_days,
           forecast_rv_bps, trailing_mean_rv_bps, updated_ts FROM har_anchors;
DROP TABLE har_anchors;
ALTER TABLE har_anchors_v38 RENAME TO har_anchors;
CREATE TABLE promotion_state_v38 (
    symbol TEXT NOT NULL COLLATE NOCASE CHECK(length(symbol) BETWEEN 1 AND 64),
    status TEXT NOT NULL CHECK(status IN ('promoted','held','demoted')),
    fit_delta_bps REAL NOT NULL,
    val_delta_bps REAL NOT NULL,
    reason TEXT NOT NULL CHECK(length(reason) BETWEEN 1 AND 512),
    updated_ts INTEGER NOT NULL CHECK(updated_ts >= 0),
    environment TEXT NOT NULL DEFAULT 'unknown'
        CHECK(environment IN ('unknown','testnet','live')),
    PRIMARY KEY (symbol, environment)
);
INSERT INTO promotion_state_v38
    (symbol, status, fit_delta_bps, val_delta_bps, reason, updated_ts)
    SELECT symbol, status, fit_delta_bps, val_delta_bps, reason, updated_ts
    FROM promotion_state;
DROP TABLE promotion_state;
ALTER TABLE promotion_state_v38 RENAME TO promotion_state;
ALTER TABLE autotune_apply_events ADD COLUMN environment
    TEXT NOT NULL DEFAULT 'unknown' CHECK(environment IN ('unknown','testnet','live'));
ALTER TABLE margin_snapshots ADD COLUMN environment
    TEXT NOT NULL DEFAULT 'unknown' CHECK(environment IN ('unknown','testnet','live'));
ALTER TABLE spread_samples ADD COLUMN environment
    TEXT NOT NULL DEFAULT 'unknown' CHECK(environment IN ('unknown','testnet','live'));
CREATE INDEX ix_spread_samples_env_ts ON spread_samples(environment, ts_ms);
PRAGMA user_version = 38;
COMMIT;
PRAGMA foreign_keys = ON;";

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
    /// Telemetry-only connection on a sibling file (`<db>.telemetry.db`).
    /// `spread_samples` rows arrive per WebSocket frame — hundreds a second —
    /// and are lossy-at-worst, so they get their own `synchronous = NORMAL`
    /// writer rather than queueing fsync-bound writes ahead of
    /// execution-ledger work on `conn`. `None` falls back to `conn`
    /// (in-memory stores and telemetry-open failures).
    telemetry: Option<Mutex<Connection>>,
    /// Last persisted (symbol, venue) timestamp for the telemetry throttle —
    /// excess frames drop here instead of queueing on the telemetry writer.
    /// Throttle watermarks keyed (symbol, venue, environment) — an env
    /// switch must not let the old env's last-write suppress the new
    /// env's first frame.
    sample_watermarks: Mutex<HashMap<(String, String, String), i64>>,
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
    // Persist the canonical id — reconciler identity checks compare the
    // adapter's venue verbatim, so an alias ("hl", "binance futures") must
    // never reach durable state.
    Ok(crate::state::canonical_venue_id(&v)
        .map(str::to_string)
        .unwrap_or(v))
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
    // Stored verbatim (trim only) — leg symbols carry venue-native case
    // (Hyperliquid `kPEPE` must reach the venue byte-for-byte). Blocking
    // comparisons canonicalize case at the query layer instead.
    Ok(v.to_string())
}

// Symbol normalization lives in arbitrageur-core (single implementation
// shared with the market-data hub, monitor lanes, and execution readers —
// the same spelling rules can never drift). Re-exported here so existing
// `crate::state_store::canonical_base` call sites stay unchanged.
pub use arbitrageur_core::symbol::canonical_base;

/// Validate + normalize an environment tag for venue-data rows. Only the
/// two real environments may be written — 'unknown' exists solely as the
/// pre-v38 backfill marker and is never a valid write target (a writer
/// that cannot name its environment must not persist the sample).
pub(crate) fn normalize_environment(raw: &str) -> anyhow::Result<String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "testnet" => Ok("testnet".to_string()),
        "live" => Ok("live".to_string()),
        other => anyhow::bail!("invalid environment '{other}' — expected testnet|live"),
    }
}
// Quote-policy-aware key sets — `separate` mode restricts reads to the
// exact pair key, merged modes return the full USD-family alias set.
pub(crate) use samples::sample_keys;

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
    // Index-family siblings are proven-identical instruments: a `SPX500`
    // read must also see rows a venue recorded under `US500`/`SP500`
    // (and their quoted forms). Only unambiguous spellings merge —
    // `index_alias_siblings` excludes collision-prone tickers like `SPX`.
    for sib in arbitrageur_core::symbol::index_alias_siblings(&base) {
        for key in [
            sib.to_string(),
            format!("{sib}-USDC"),
            format!("{sib}-USD"),
            format!("{sib}-USDT"),
        ] {
            if !out.contains(&key) {
                out.push(key);
            }
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
        max_leverage: row
            .get::<_, Option<i64>>(6)?
            .and_then(|v| u32::try_from(v).ok()),
        environment: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
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
       created_at, updated_at, closed_at, environment
FROM signal_executions
WHERE id = ?1";

fn signal_exec_from_row(row: &Row<'_>) -> rusqlite::Result<SignalExecution> {
    let status_raw: String = row.get(3)?;
    Ok(SignalExecution {
        id: row.get(0)?,
        strategy_id: row.get(1)?,
        symbol: row.get(2)?,
        status: SignalStatus::from_status(&status_raw).unwrap_or(SignalStatus::Unresolved),
        environment: row.get(13)?,
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
        symbol: row.get(12).unwrap_or_default(),
        intended_qty: row.get(5)?,
        filled_qty: row.get(6)?,
        fill_price: row.get(7)?,
        fee_usd: row.get(8)?,
        venue_order_id: row.get(9)?,
        client_order_id: row
            .get::<_, Option<String>>(13)
            .ok()
            .flatten()
            .filter(|s| !s.trim().is_empty()),
        intended_price: row.get::<_, Option<f64>>(14).ok().flatten(),
        // Corrupt/NULL scale reads as 1.0 — the same-unit semantic —
        // rather than poisoning shared-unit math with a bad divisor.
        unit_scale: row
            .get::<_, f64>(15)
            .ok()
            .filter(|s| s.is_finite() && *s > 0.0)
            .unwrap_or(1.0),
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn scheduled_exit_from_row(row: &Row<'_>) -> rusqlite::Result<ScheduledExit> {
    Ok(ScheduledExit {
        id: row.get(0)?,
        venue: row.get(1)?,
        symbol: row.get(2)?,
        side: row.get(3)?,
        close_pct: row.get(4)?,
        mode: row.get(5)?,
        limit_price: row.get(6)?,
        execute_at_ms: row.get(7)?,
        status: row.get(8)?,
        attempts: row.get(12).unwrap_or(0),
        note: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
        environment: row.get(13).unwrap_or_else(|_| "unknown".to_string()),
    })
}

mod accounts;
mod autotune;
mod config;
mod cycle_lots;
mod equity;
mod exits;
mod logs;
mod monitor;
mod samples;
mod seasonality;
mod signals;

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
        let telemetry = file_path.and_then(|p| match Self::open_telemetry(p) {
            Ok(t) => Some(t),
            Err(e) => {
                // Telemetry is lossy-at-worst — fall back to the shared
                // connection rather than failing store open.
                tracing::warn!(
                    "state_store: telemetry db unavailable ({e}); sharing main connection"
                );
                None
            }
        });
        Ok(Self {
            conn: Mutex::new(conn),
            telemetry,
            sample_watermarks: Mutex::new(HashMap::new()),
        })
    }

    /// Connection hosting `spread_samples` — the dedicated telemetry writer
    /// when available, else the shared main connection.
    fn telemetry(&self) -> &Mutex<Connection> {
        self.telemetry.as_ref().unwrap_or(&self.conn)
    }

    /// Sibling file path for the telemetry database.
    fn telemetry_path(main_path: &Path) -> PathBuf {
        main_path.with_extension("telemetry.db")
    }

    /// Open (or create) the telemetry database and drain any pre-split
    /// `spread_samples` rows out of the main database. The drain runs once
    /// per open; subsequent boots find an empty legacy table and detach
    /// immediately.
    fn open_telemetry(main_path: &Path) -> anyhow::Result<Mutex<Connection>> {
        let tpath = Self::telemetry_path(main_path);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            drop(
                fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(false)
                    .mode(0o600)
                    .open(&tpath)?,
            );
        }
        let tconn = Connection::open(&tpath)?;
        tconn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;
             PRAGMA trusted_schema = OFF;
             PRAGMA auto_vacuum = INCREMENTAL;",
        )?;
        tconn.execute_batch(TELEMETRY_SCHEMA_SQL)?;
        // Telemetry files predate v38's environment column: add it
        // idempotently (no version chain exists on this sidecar db).
        // Pre-existing rows stay 'unknown' — env-filtered reads exclude
        // them rather than attribute provenance that was never recorded.
        // The env index is built HERE, after the column is guaranteed —
        // keeping it in TELEMETRY_SCHEMA_SQL made every pre-v38 telemetry
        // file fail to open (CREATE INDEX on a missing column), which
        // silently routed per-frame spread writes onto the main connection.
        let has_env: bool = tconn
            .prepare(
                "SELECT 1 FROM pragma_table_info('spread_samples') WHERE name = 'environment'",
            )?
            .exists([])?;
        if !has_env {
            tconn.execute_batch(
                "ALTER TABLE spread_samples ADD COLUMN environment
                     TEXT NOT NULL DEFAULT 'unknown'
                     CHECK(environment IN ('unknown','testnet','live'));",
            )?;
        }
        tconn.execute_batch(
            "CREATE INDEX IF NOT EXISTS ix_spread_samples_env_ts
                 ON spread_samples(environment, ts_ms);
             -- (environment, symbol, ts_ms): the per-frame incumbent_anchor
             -- probe filters env+symbol and orders by ts; without it SQLite
             -- scans every env row in ts order once per write batch —
             -- hundreds of calls/sec × a degenerating range is a page-read
             -- storm. Both equalities first keeps the scan bounded to the
             -- key's own rows.
             CREATE INDEX IF NOT EXISTS ix_spread_samples_env_symbol_ts
                 ON spread_samples(environment, symbol, ts_ms);",
        )?;
        if let Err(e) = Self::drain_legacy_samples(&tconn, main_path) {
            tracing::warn!("state_store: legacy spread_samples drain failed ({e}); leftover rows stay in the main db");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tpath, fs::Permissions::from_mode(0o600))?;
        }
        Ok(Mutex::new(tconn))
    }

    /// Copy pre-split `spread_samples` rows from the main database into the
    /// telemetry file in bounded batches, deleting each copied batch so the
    /// legacy table converges to empty. Batch deletes hold a short write
    /// lock on the main db — this runs at open before feeds or execution
    /// start, so the contention window is bounded and one-time.
    fn drain_legacy_samples(tconn: &Connection, main_path: &Path) -> anyhow::Result<()> {
        const DRAIN_BATCH: i64 = 100_000;
        tconn.execute(
            "ATTACH DATABASE ?1 AS legacy",
            [main_path.to_string_lossy().as_ref()],
        )?;
        let drain = (|| -> anyhow::Result<i64> {
            let mut total = 0i64;
            loop {
                let copied = tconn.execute(
                    "INSERT INTO spread_samples
                        (symbol, venue_id, ts_ms, mid, best_bid, best_ask, bid_depth, ask_depth)
                     SELECT symbol, venue_id, ts_ms, mid, best_bid, best_ask, bid_depth, ask_depth
                     FROM (SELECT * FROM legacy.spread_samples ORDER BY id LIMIT ?1)",
                    [DRAIN_BATCH],
                )? as i64;
                if copied == 0 {
                    break;
                }
                total += copied;
                tconn.execute(
                    "DELETE FROM legacy.spread_samples WHERE id IN (
                         SELECT id FROM legacy.spread_samples ORDER BY id LIMIT ?1)",
                    [copied],
                )?;
            }
            Ok(total)
        })();
        let _ = tconn.execute("DETACH DATABASE legacy", []);
        let total = drain?;
        if total > 0 {
            tracing::info!(
                "state_store: drained {total} legacy spread_samples rows into telemetry db"
            );
        }
        Ok(())
    }

    fn migrate(conn: &Connection) -> anyhow::Result<()> {
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > 41 {
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
        if version <= 19 {
            if let Err(err) = conn.execute_batch(MIGRATION_V20_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 20 {
            if let Err(err) = conn.execute_batch(MIGRATION_V21_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 21 {
            if let Err(err) = conn.execute_batch(MIGRATION_V22_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 22 {
            if let Err(err) = conn.execute_batch(MIGRATION_V23_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 23 {
            if let Err(err) = conn.execute_batch(MIGRATION_V24_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 24 {
            if let Err(err) = conn.execute_batch(MIGRATION_V25_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 25 {
            if let Err(err) = conn.execute_batch(MIGRATION_V26_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 26 {
            if let Err(err) = conn.execute_batch(MIGRATION_V27_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 27 {
            if let Err(err) = conn.execute_batch(MIGRATION_V28_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 28 {
            if let Err(err) = conn.execute_batch(MIGRATION_V29_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 29 {
            if let Err(err) = conn.execute_batch(MIGRATION_V30_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 30 {
            if let Err(err) = conn.execute_batch(MIGRATION_V31_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 31 {
            if let Err(err) = conn.execute_batch(MIGRATION_V32_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 32 {
            if let Err(err) = conn.execute_batch(MIGRATION_V33_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 33 {
            if let Err(err) = conn.execute_batch(MIGRATION_V34_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 34 {
            if let Err(err) = conn.execute_batch(MIGRATION_V35_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 35 {
            if let Err(err) = conn.execute_batch(MIGRATION_V36_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 36 {
            if let Err(err) = conn.execute_batch(MIGRATION_V37_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 37 {
            if let Err(err) = conn.execute_batch(MIGRATION_V38_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 38 {
            if let Err(err) = conn.execute_batch(MIGRATION_V39_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 39 {
            if let Err(err) = conn.execute_batch(MIGRATION_V40_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        if version <= 40 {
            if let Err(err) = conn.execute_batch(MIGRATION_V41_SQL) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err.into());
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests;
