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
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

use arbitrageur_core::ExchangeAdapter;
use arbitrageur_risk::margin_monitor::MarginHealthStatus;
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
pub const DEFAULT_MIN_EXPECTED_YIELD_PCT: f64 = 30.0;
/// Minimum absolute nominal net USD edge required for autonomous options entry.
/// Prevents near-dated options with tiny nominal edges from triggering trades
/// where exchange friction would eliminate real profit.
pub const MIN_OPTIONS_BOT_NET_EDGE_USD: f64 = 10.0;

fn default_min_expected_yield_pct() -> f64 {
    DEFAULT_MIN_EXPECTED_YIELD_PCT
}

/// A refused candidate id cools down this long — an edge that evaporates
/// on re-verification must not re-dispatch every poll against flickering
/// testnet books.
const REFUSAL_COOLDOWN: Duration = Duration::from_secs(600);

/// Validated bot policy derived from the persisted DTO.
#[derive(Debug, Clone)]
pub struct OptionsBotPolicy {
    pub qty_base: f64,
    pub min_edge_bps: f64,
    pub min_hours_to_expiry: f64,
    pub max_days_to_expiry: f64,
    pub poll_interval_secs: u64,
    pub max_open_positions: usize,
    pub min_expected_yield_pct: f64,
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
            min_expected_yield_pct: DEFAULT_MIN_EXPECTED_YIELD_PCT,
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
    #[serde(default = "default_min_expected_yield_pct")]
    pub min_expected_yield_pct: f64,
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
        if !finite(self.min_expected_yield_pct)
            || !(30.0..=10_000.0).contains(&self.min_expected_yield_pct)
        {
            return Err("minExpectedYieldPct must be at least 30.0% and <= 10000".into());
        }
        Ok((
            OptionsBotPolicy {
                qty_base: self.qty_base,
                min_edge_bps: self.min_edge_bps,
                min_hours_to_expiry: self.min_hours_to_expiry,
                max_days_to_expiry: self.max_days_to_expiry,
                poll_interval_secs: self.poll_interval_secs,
                max_open_positions: self.max_open_positions as usize,
                min_expected_yield_pct: self.min_expected_yield_pct,
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
            min_expected_yield_pct: cfg.min_expected_yield_pct,
        }
    }
}

/// Last-tick telemetry per substrategy — mode, open cycle count, scan
/// stats, and the latest refusal. Written by `tick_substrategy`; surfaced
/// through `options_bot_status` so a silent scan loop is distinguishable
/// from a dead one.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionsBotTickStatus {
    pub substrategy: String,
    pub mode: String,
    pub open_positions: usize,
    pub max_open_positions: usize,
    pub last_scan_ms: i64,
    /// Rows the scanner returned before the `is_executable` gate —
    /// distinguishes "chain scanned, nothing qualified" from "no data".
    pub rows_scanned: usize,
    pub candidates: usize,
    pub best_edge_usd: Option<f64>,
    pub last_refusal: Option<String>,
}

/// Extracts the expiration timestamp in ms from an option instrument (e.g. `BTC-25DEC26-125000-C`)
/// or opportunity ID (e.g. `opar-BTC-1766707200000-...` or `obox-BTC-1766707200000-...`).
pub(crate) fn parse_option_expiry_ms(token: &str) -> Option<i64> {
    // 1. Try finding a millisecond timestamp part in [1.6e12, 3.0e12]
    for part in token.split('-') {
        if let Ok(ts) = part.parse::<i64>() {
            if (1_600_000_000_000..=3_000_000_000_000).contains(&ts) {
                return Some(ts);
            }
        }
    }
    // 2. Try parsing Deribit date string (e.g. "25DEC26")
    for part in token.split('-') {
        if part.len() >= 6 && part.len() <= 7 {
            if let Ok(date) = chrono::NaiveDate::parse_from_str(part, "%d%b%y") {
                if let Some(dt) = date.and_hms_opt(8, 0, 0) {
                    return Some(
                        chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(dt, chrono::Utc)
                            .timestamp_millis(),
                    );
                }
            }
        }
    }
    None
}

/// Fresh executable value of the package's option legs, using bid for a
/// long close and ask for a short close. `None` means at least one option
/// mark is unavailable, so elapsed-time synthesis must not authorize an
/// early harvest.
fn option_chain_close_proceeds(
    state: &AppEngineState,
    position: &crate::state_store::OpenPositionDto,
) -> Option<f64> {
    let mut total = 0.0f64;
    let mut option_legs = 0usize;
    for leg in &position.venues {
        if !crate::state_store::symbol_is_fixed_contract(&leg.symbol) || leg.net_qty.abs() <= 1e-9 {
            continue;
        }
        option_legs += 1;
        let book = state.market_data_hub.get_valid_orderbook(
            &arbitrageur_core::types::VenueId::new(&leg.venue),
            &leg.symbol,
            Some(Duration::from_secs(30)),
        )?;
        let price = if leg.net_qty > 0.0 {
            book.best_bid_price()
        } else {
            book.best_ask_price()
        }?;
        let price = price.to_string().parse::<f64>().ok()?;
        if !price.is_finite() || price <= 0.0 {
            return None;
        }
        let signed_proceeds = if leg.net_qty > 0.0 {
            leg.net_qty.abs() * price
        } else {
            -leg.net_qty.abs() * price
        };
        total += signed_proceeds;
    }
    (option_legs > 0 && total.is_finite()).then_some(total)
}

fn option_symbol_parts(symbol: &str) -> Option<(String, String, f64)> {
    let parts: Vec<&str> = symbol.split('-').collect();
    if parts.len() < 4 {
        return None;
    }
    let asset = parts[0].to_ascii_uppercase();
    let expiry = parts
        .iter()
        .skip(1)
        .find(|part| part.len() >= 6)
        .copied()?
        .to_string();
    let strike = parse_option_expiry_ms(symbol).and_then(|expiry_ms| {
        parts.iter().skip(2).find_map(|part| {
            let value = part.parse::<f64>().ok()?;
            (value > 0.0 && value < expiry_ms as f64).then_some(value)
        })
    })?;
    Some((asset, expiry, strike))
}

fn live_linear_mid(state: &AppEngineState, asset: &str) -> Option<Decimal> {
    if let Some(price) = state
        .vol_surface
        .get_spot(asset)
        .or_else(|| state.vol_surface.latest_forward(asset))
        .and_then(Decimal::from_f64_retain)
        .filter(|price| *price > Decimal::ZERO)
    {
        return Some(price);
    }
    let symbols = [
        format!("{asset}-USDC"),
        format!("{asset}-USDT"),
        format!("{asset}-USD"),
    ];
    for (venue, _, _) in state.get_venues() {
        for symbol in &symbols {
            if let Some(mid) = state
                .market_data_hub
                .get_valid_orderbook(
                    &arbitrageur_core::types::VenueId::new(venue),
                    symbol,
                    Some(Duration::from_secs(5)),
                )
                .and_then(|book| book.mid_price().or(book.mark_price))
                .filter(|price| *price > Decimal::ZERO)
            {
                return Some(mid);
            }
        }
    }
    None
}

fn realized_vol_30d(
    store: &crate::state_store::StateStore,
    asset: &str,
    environment: &str,
) -> Option<f64> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let from_ms = now_ms.saturating_sub(30 * 24 * 60 * 60 * 1000);
    let samples = store
        .spread_samples_mode(asset, true, from_ms, now_ms, environment)
        .ok()?;
    let mut by_venue: HashMap<String, Vec<(i64, f64)>> = HashMap::new();
    for sample in samples {
        if sample.mid.is_finite() && sample.mid > 0.0 {
            by_venue
                .entry(sample.venue_id)
                .or_default()
                .push((sample.ts_ms, sample.mid));
        }
    }
    let mut best: Option<f64> = None;
    for points in by_venue.values_mut() {
        if points.len() < 30 {
            continue;
        }
        points.sort_by_key(|(ts, _)| *ts);
        let mut returns = Vec::with_capacity(points.len().saturating_sub(1));
        let mut deltas = Vec::with_capacity(points.len().saturating_sub(1));
        for pair in points.windows(2) {
            let dt_secs = (pair[1].0 - pair[0].0) as f64 / 1000.0;
            if dt_secs <= 0.0 {
                continue;
            }
            returns.push((pair[1].1 / pair[0].1).ln());
            deltas.push(dt_secs);
        }
        if returns.len() < 20 {
            continue;
        }
        deltas.sort_by(f64::total_cmp);
        let median_dt = deltas[deltas.len() / 2].max(1.0);
        let variance_per_step =
            returns.iter().map(|ret| ret * ret).sum::<f64>() / returns.len() as f64;
        let annualized = (variance_per_step * (365.25 * 24.0 * 60.0 * 60.0) / median_dt).sqrt();
        if annualized.is_finite() {
            best = Some(best.map_or(annualized, |prior| prior.max(annualized)));
        }
    }
    best
}

fn cached_variance_strip(
    cache: &arbitrageur_strategy::options_dispersion::LiveVolSurfaceCache,
    asset: &str,
    expiry: &str,
) -> Option<(
    f64,
    f64,
    Vec<arbitrageur_math::variance_swap::DiscreteOptionQuote>,
)> {
    let slice = cache
        .slices
        .read()
        .get(&asset.to_ascii_uppercase())?
        .get(expiry)?
        .clone();
    if slice.forward_price <= 0.0 || slice.tenor_years <= 1e-4 {
        return None;
    }
    let mut quotes = Vec::with_capacity(slice.observed_strikes.len());
    for (strike_key, iv) in slice.observed_strikes {
        let strike = strike_key as f64 / 100.0;
        if strike <= 0.0 || !iv.is_finite() || iv <= 0.0 {
            continue;
        }
        let is_call = strike >= slice.forward_price;
        let option_type = if is_call {
            arbitrageur_math::black_scholes::OptionType::Call
        } else {
            arbitrageur_math::black_scholes::OptionType::Put
        };
        let price = arbitrageur_math::black_scholes::black_scholes_price(
            option_type,
            slice.forward_price,
            strike,
            slice.tenor_years,
            0.04,
            iv,
        )?;
        let half_spread = (price * 0.0005).max(1e-8);
        quotes.push(arbitrageur_math::variance_swap::DiscreteOptionQuote::new(
            strike,
            price - half_spread,
            price + half_spread,
            is_call,
        ));
    }
    quotes.sort_by(|a, b| a.strike.total_cmp(&b.strike));
    (quotes.len() >= 3).then_some((slice.forward_price, slice.tenor_years, quotes))
}

fn evaluate_cached_variance_dispersion(
    cache: &arbitrageur_strategy::options_dispersion::LiveVolSurfaceCache,
    store: &crate::state_store::StateStore,
    environment: &str,
) -> Option<(
    String,
    arbitrageur_math::variance_swap::VarianceDispersionOpportunity,
)> {
    let mut common_expiries = {
        let slices = cache.slices.read();
        let btc = slices.get("BTC")?;
        let eth = slices.get("ETH")?;
        btc.keys()
            .filter(|expiry| eth.contains_key(*expiry))
            .cloned()
            .collect::<Vec<_>>()
    };
    common_expiries.sort();
    let engine =
        arbitrageur_strategy::options_dispersion::VarianceSwapDispersionEngine::new(0.04, 0.05);
    for expiry in common_expiries {
        let rv_btc = realized_vol_30d(store, "BTC", environment);
        let rv_eth = realized_vol_30d(store, "ETH", environment);
        let (Some(rv_btc), Some(rv_eth)) = (rv_btc, rv_eth) else {
            continue;
        };
        let (
            Some((btc_forward, btc_tenor, btc_quotes)),
            Some((eth_forward, eth_tenor, eth_quotes)),
        ) = (
            cached_variance_strip(cache, "BTC", &expiry),
            cached_variance_strip(cache, "ETH", &expiry),
        )
        else {
            continue;
        };
        if let Some(opportunity) = engine.evaluate_dispersion_opportunity(
            "BTC",
            btc_forward,
            btc_tenor,
            btc_quotes,
            rv_btc,
            "ETH",
            eth_forward,
            eth_tenor,
            eth_quotes,
            rv_eth,
        ) {
            return Some((expiry, opportunity));
        }
    }
    None
}

fn apply_svi_delta_adjustments(
    state: &AppEngineState,
    positions: &[arbitrageur_core::position::Position],
    greeks: &mut HashMap<String, arbitrageur_risk::delta_guard::PortfolioGreeks>,
    now_ms: i64,
) {
    for leg in positions {
        if !crate::state_store::symbol_is_fixed_contract(&leg.symbol) {
            continue;
        }
        let Some((asset, expiry, strike)) = option_symbol_parts(&leg.symbol) else {
            continue;
        };
        let Some(spot) =
            live_linear_mid(state, &asset).and_then(|price| price.to_string().parse::<f64>().ok())
        else {
            continue;
        };
        let Some(tau) = arbitrageur_risk::delta_guard::parse_expiry_tenor_years(&expiry, now_ms)
        else {
            continue;
        };
        let Some(iv) = state
            .vol_surface
            .evaluate_volatility(&asset, &expiry, strike)
        else {
            continue;
        };
        let option_type = if leg.symbol.to_ascii_uppercase().ends_with("-C") {
            arbitrageur_math::black_scholes::OptionType::Call
        } else {
            arbitrageur_math::black_scholes::OptionType::Put
        };
        let Some(bs) = arbitrageur_math::black_scholes::black_scholes_greeks(
            option_type,
            spot,
            strike,
            tau,
            0.0,
            iv,
        ) else {
            continue;
        };
        let Ok(signed_qty) = leg.signed_quantity().to_string().parse::<f64>() else {
            continue;
        };
        let Some(expiry_ms) = parse_option_expiry_ms(&expiry) else {
            continue;
        };
        let Some(smile_delta) = state
            .vol_surface
            .get_smile_adjusted_delta(&asset, expiry_ms, strike, spot, bs.delta, bs.vega)
        else {
            continue;
        };
        let bs_delta = bs.delta * signed_qty;
        let smile_delta = smile_delta * signed_qty;
        if let Some(adjustment) = Decimal::from_f64_retain(smile_delta - bs_delta) {
            greeks
                .entry(asset)
                .or_insert(arbitrageur_risk::delta_guard::PortfolioGreeks {
                    net_delta: dec!(0),
                    net_gamma: dec!(0),
                    net_vega: dec!(0),
                    net_theta: dec!(0),
                })
                .net_delta += adjustment;
            tracing::debug!(
                "options bot: SVI smile delta on {} adjusted {:+.6}",
                leg.symbol,
                smile_delta - bs_delta
            );
        }
    }
}

static LAST_HEDGED_SPOTS: Mutex<Option<HashMap<String, Decimal>>> = Mutex::new(None);

fn get_or_init_last_hedged_spot(asset: &str, current_spot: Decimal) -> Decimal {
    let mut guard = LAST_HEDGED_SPOTS.lock();
    let map = guard.get_or_insert_with(HashMap::new);
    *map.entry(asset.to_string()).or_insert(current_spot)
}

fn update_last_hedged_spot(asset: &str, spot: Decimal) {
    let mut guard = LAST_HEDGED_SPOTS.lock();
    let map = guard.get_or_insert_with(HashMap::new);
    map.insert(asset.to_string(), spot);
}

pub struct OptionsBotExecutor {
    shutdown: Mutex<bool>,
    /// Refused (substrategy, candidate id) pairs with their refusal
    /// timestamp — in-memory only, so a restart retries immediately.
    refusal_cooldown: Mutex<HashMap<String, Instant>>,
}

impl OptionsBotExecutor {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            shutdown: Mutex::new(false),
            refusal_cooldown: Mutex::new(HashMap::new()),
        })
    }

    fn cooldown_key(substrategy: &str, id: &str) -> String {
        format!("{substrategy}:{id}")
    }

    fn note_refusal(&self, substrategy: &str, id: &str) {
        self.refusal_cooldown
            .lock()
            .insert(Self::cooldown_key(substrategy, id), Instant::now());
    }

    /// True while the candidate still sits inside its refusal window.
    #[cfg(test)]
    fn is_cooling(&self, substrategy: &str, id: &str) -> bool {
        let cd = self.refusal_cooldown.lock();
        cd.get(&Self::cooldown_key(substrategy, id))
            .is_some_and(|t| t.elapsed() < REFUSAL_COOLDOWN)
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
        if (mode == BotMode::Auto || dto.enabled) && state.circuit_breaker.is_tripped() {
            return Err(format!(
                "Cannot enable or arm Options bot for '{}' while circuit breaker is tripped. Reset circuit breaker first.",
                dto.substrategy_id
            ));
        }
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
            let mut trades: std::collections::HashMap<String, bool> =
                std::collections::HashMap::new();
            for s in sigs.iter().filter(|s| {
                s.environment.eq_ignore_ascii_case(env)
                    && s.strategy_id.eq_ignore_ascii_case(strategy_id)
                    && !matches!(s.status, SignalStatus::Closed | SignalStatus::Failed)
            }) {
                let trade =
                    s.id.rsplit_once("-leg")
                        .filter(|(_, tail)| !tail.is_empty())
                        .map(|(prefix, _)| prefix)
                        .unwrap_or(s.id.as_str())
                        .to_string();
                // A trade counts toward the entry cap only while it still
                // holds a live package leg. Perp-residual groups (option legs
                // closed, hedge dust left under the venue minimum) are tracked
                // exposure — but permanently counting them would strand the
                // substrategy at cap forever.
                let has_option_leg = crate::state_store::symbol_is_fixed_contract(&s.symbol)
                    || s.legs
                        .iter()
                        .any(|l| crate::state_store::symbol_is_fixed_contract(&l.symbol));
                trades
                    .entry(trade)
                    .and_modify(|v| *v |= has_option_leg)
                    .or_insert(has_option_leg);
            }
            trades.values().filter(|v| **v).count()
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

    /// Active Position Supervision:
    /// 1. Options Parity Expiration Auto-Unwind: When option legs expire (or within 30m of 08:00 UTC),
    ///    unwind the perpetual future leg to eliminate 100% naked directional delta!
    /// 2. Options Package Early Harvest (The 88% Rule & 2h Pin Risk Boundary):
    ///    When an open package (Box Spread, Jelly Roll, Convexity) has captured >= 88% of target edge,
    ///    or is within 2 hours of expiry, unwind early via IOC to avoid Deribit's 3.0-6.0 bps expiration
    ///    delivery fees, pin risk, and to recycle margin capital.
    pub async fn supervise_open_positions(
        state: &AppEngineState,
        substrategy: &str,
        strategy_id: &str,
    ) {
        let Ok(positions) =
            crate::execution::get_open_positions_impl(Some(strategy_id.to_string()), state).await
        else {
            return;
        };

        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut portfolio_all_positions = Vec::new();

        for pos in positions {
            // Find expiry timestamp from leg symbols or pos.symbol or trade_id
            let mut expiry_ms: Option<i64> = parse_option_expiry_ms(&pos.trade_id)
                .or_else(|| parse_option_expiry_ms(&pos.symbol));
            if expiry_ms.is_none() {
                for leg_sym in &pos.leg_symbols {
                    if let Some(exp) = parse_option_expiry_ms(leg_sym) {
                        expiry_ms = Some(exp);
                        break;
                    }
                }
            }

            let hours_to_expiry = expiry_ms.map(|exp| (exp - now_ms) as f64 / 3_600_000.0);

            // 1. Options Parity Expiration Protection:
            // When options legs expire at 08:00 UTC (or within 30m prior, or already post-expiry),
            // market-close the open perpetual future to eliminate 100% naked directional delta!
            if substrategy == "parity" {
                let is_near_or_past_expiry = hours_to_expiry.is_some_and(|h| h <= 0.5);
                if is_near_or_past_expiry {
                    tracing::warn!(
                        "options bot: parity trade {} reached expiration window (hours to expiry: {:?}); triggering auto-unwind of perpetual hedge",
                        pos.trade_id,
                        hours_to_expiry
                    );
                    let _ = crate::execution::flatten_trade_positions_impl(
                        &pos.trade_id,
                        strategy_id,
                        Some("market"),
                        arbitrageur_execution::virtual_ledger::CycleStatus::Closed,
                        state,
                    )
                    .await;
                    continue;
                }
            }

            // 2. Options Package Early Harvest (The 88% Rule & 2h Expiry Pin Risk Boundary):
            // Deribit charges 0.015% per ITM leg at settlement (3.0 bps on Box, 6.0 bps on Jelly Roll).
            // When >= 88% of edge is realized, holding to expiry pays more in delivery fees than the remaining edge.
            let edge_harvest_ratio = match (pos.expected_net_usd, pos.unrealized_pnl_usd) {
                (Some(expected), Some(upnl)) if expected > 0.0 => upnl / expected,
                _ => 0.0,
            };

            let option_close_proceeds = option_chain_close_proceeds(state, &pos);
            let is_edge_harvest_ready =
                option_close_proceeds.is_some() && edge_harvest_ratio >= 0.88;
            let is_pin_risk_imminent = hours_to_expiry.is_some_and(|h| h <= 3.0 && h > 0.0);

            if is_edge_harvest_ready || is_pin_risk_imminent {
                let reason = if is_edge_harvest_ready {
                    format!(
                        "early harvest triggered: {:.1}% of target edge captured (+${:.2} USD; option-chain close value ${:.2}) — avoiding Deribit expiration delivery fees",
                        edge_harvest_ratio * 100.0,
                        pos.unrealized_pnl_usd.unwrap_or(0.0),
                        option_close_proceeds.unwrap_or(0.0)
                    )
                } else {
                    format!(
                        "near-expiry exit triggered ({:.1}h remaining) — eliminating pin risk and delivery friction",
                        hours_to_expiry.unwrap_or(0.0)
                    )
                };

                tracing::info!("options bot {substrategy}: trade {} {reason}", pos.trade_id);

                let _ = crate::execution::flatten_trade_positions_impl(
                    &pos.trade_id,
                    strategy_id,
                    Some("ioc"),
                    arbitrageur_execution::virtual_ledger::CycleStatus::Closed,
                    state,
                )
                .await;
                continue;
            }

            // 3. Dynamic Delta Drift Supervision:
            // For open positions, reconstruct leg exposures and evaluate net directional delta.
            // If delta drift violates DeltaGuard limits, log alert.
            let dg = state.delta_guard.read();
            let mut leg_positions = Vec::new();
            for v in &pos.venues {
                let qty_dec = Decimal::from_f64_retain(v.net_qty.abs()).unwrap_or(dec!(0));
                if qty_dec > dec!(0) {
                    leg_positions.push(arbitrageur_core::position::Position {
                        venue: arbitrageur_core::types::VenueId::new(&v.venue),
                        symbol: v.symbol.clone(),
                        side: if v.net_qty > 0.0 {
                            arbitrageur_core::types::Side::Buy
                        } else {
                            arbitrageur_core::types::Side::Sell
                        },
                        quantity: qty_dec,
                        entry_price: dec!(0),
                        mark_price: Decimal::from_f64_retain(
                            pos.notional_usd.unwrap_or(0.0) / v.net_qty.abs(),
                        )
                        .unwrap_or(dec!(0)),
                        unrealized_pnl: Decimal::from_f64_retain(
                            pos.unrealized_pnl_usd.unwrap_or(0.0),
                        )
                        .unwrap_or(dec!(0)),
                        realized_pnl: dec!(0),
                        liquidation_price: None,
                        leverage: dec!(1),
                        initial_margin: dec!(0),
                        maintenance_margin: dec!(0),
                        updated_at: chrono::Utc::now(),
                    });
                }
            }
            if !leg_positions.is_empty() {
                portfolio_all_positions.extend(leg_positions.clone());
                // Analytical Black-76 Greeks supervision (delta, gamma & vega) with live SABR volatility smile
                let now_ms = chrono::Utc::now().timestamp_millis();
                let mut greeks = dg.calculate_analytical_greeks_with_smile(
                    &leg_positions,
                    Some(&*state.vol_surface),
                    None,
                    Some(now_ms),
                );
                apply_svi_delta_adjustments(state, &leg_positions, &mut greeks, now_ms);
                for (asset, g) in &greeks {
                    if g.net_delta.abs() > dec!(0.05) {
                        tracing::warn!(
                            "options bot {substrategy}: trade {} carries non-linear analytical delta drift on {asset}: {:.4} base delta (gamma: {:.6})",
                            pos.trade_id, g.net_delta, g.net_gamma
                        );
                    }
                    if g.net_gamma.abs() > dec!(0.0005) {
                        tracing::warn!(
                            "options bot {substrategy}: trade {} carries elevated non-linear gamma curvature on {asset}: {:.6} (1/USD)",
                            pos.trade_id, g.net_gamma
                        );
                    }
                    if g.net_vega.abs() > dec!(100.0) {
                        tracing::warn!(
                            "options bot {substrategy}: trade {} carries elevated vega exposure on {asset}: ${:.2} USD/vol",
                            pos.trade_id, g.net_vega
                        );
                    }
                }
                if let Err((asset, exposure)) = dg.check_delta_neutrality(&leg_positions) {
                    tracing::warn!(
                        "options bot {substrategy}: trade {} carries unhedged delta drift on {asset}: ${exposure:.2} USD",
                        pos.trade_id
                    );
                }
            }
        }

        // 4. Portfolio-wide Multi-Asset Greeks Aggregation & Whalley-Wilmott Rehedging Supervision
        if !portfolio_all_positions.is_empty() {
            let mut rehedge_tasks = Vec::new();

            {
                let dg = state.delta_guard.read();
                let now_ms = chrono::Utc::now().timestamp_millis();
                let mut portfolio_greeks = dg.calculate_analytical_greeks_with_smile(
                    &portfolio_all_positions,
                    Some(&*state.vol_surface),
                    None,
                    Some(now_ms),
                );
                apply_svi_delta_adjustments(
                    state,
                    &portfolio_all_positions,
                    &mut portfolio_greeks,
                    now_ms,
                );
                for (asset, g) in &portfolio_greeks {
                    // Fail closed: a rehedge sized off a fabricated spot is a
                    // fabricated order. Skip the asset until a real mark
                    // exists instead of falling back to a hardcoded price.
                    let Some(spot) = live_linear_mid(state, asset) else {
                        tracing::warn!(
                            "options bot portfolio: no live spot/forward/mid for {asset} — rehedge skipped (no synthetic price)"
                        );
                        continue;
                    };

                    let last_spot = get_or_init_last_hedged_spot(asset, spot);

                    let bichuch_cfg = arbitrageur_risk::delta_guard::BichuchDeltaGammaConfig {
                        prop_cost_rate: dec!(0.0005),
                        quad_impact_coeff: dec!(0.000001),
                        option_cost_rate: dec!(0.0005),
                        risk_aversion: dec!(0.0001),
                        implied_volatility: dec!(0.65),
                        jump_intensity: dec!(2.0),
                        jump_variance: dec!(0.04),
                    };

                    let bichuch_decision = dg.evaluate_bichuch_2d_rehedge_trigger(
                        spot,
                        last_spot,
                        g.net_delta,
                        dec!(0),
                        g.net_gamma,
                        dec!(0),
                        Some(&bichuch_cfg),
                    );
                    let ww_decision = dg
                        .compute_whalley_wilmott_bandwidth(spot, g.net_gamma, None)
                        .and_then(|bands| {
                            let spot_drift = (spot - last_spot).abs();
                            let delta_drift = g.net_delta.abs();
                            (spot_drift >= bands.delta_s_star
                                || delta_drift >= bands.delta_band_width)
                                .then(|| {
                                    arbitrageur_risk::delta_guard::BichuchRehedgeDecision::DeltaRebalance {
                                        rebalance_qty: -g.net_delta,
                                        reason: format!(
                                            "Whalley-Wilmott breach: spot drift ${spot_drift:.2} (band ${:.2}) or delta drift {:.4} (band {:.4})",
                                            bands.delta_s_star, delta_drift, bands.delta_band_width
                                        ),
                                    }
                                })
                        });
                    let decision = match bichuch_decision {
                        arbitrageur_risk::delta_guard::BichuchRehedgeDecision::NoAction => {
                            ww_decision.unwrap_or(
                                arbitrageur_risk::delta_guard::BichuchRehedgeDecision::NoAction,
                            )
                        }
                        other => other,
                    };

                    match decision {
                        arbitrageur_risk::delta_guard::BichuchRehedgeDecision::DeltaRebalance {
                            rebalance_qty,
                            reason,
                        }
                        | arbitrageur_risk::delta_guard::BichuchRehedgeDecision::Both {
                            rebalance_delta: rebalance_qty,
                            reason,
                            ..
                        } => {
                            tracing::warn!(
                                "options bot portfolio: Bichuch 2D re-hedge recommended for {asset}: net delta {:.4}, rehedge qty {:.4} ({})",
                                g.net_delta,
                                rebalance_qty,
                                reason
                            );
                            crate::alerting::dispatch_throttled(
                                state.env().as_str(),
                                "options_portfolio_rehedge",
                                asset,
                                std::time::Duration::from_secs(300),
                                format!(
                                    "Portfolio Delta Re-hedge Recommended for {asset}: net delta {:.4}, suggested rehedge {:.4} base contracts ({})",
                                    g.net_delta, rebalance_qty, reason
                                ),
                            );

                            // If unhedged quantity exceeds threshold, evaluate perpetual venue selection
                            if rebalance_qty.abs() > dec!(0.001) {
                                let perp_symbol = format!("{asset}-USDC");
                                let hedge_side = if rebalance_qty > dec!(0) {
                                    arbitrageur_core::types::Side::Buy
                                } else {
                                    arbitrageur_core::types::Side::Sell
                                };
                                let hedge_qty = rebalance_qty.abs();

                                let candidate_venues = [
                                    ("hyperliquid", dec!(1.5), dec!(0.0001)),
                                    ("binance", dec!(4.5), dec!(0.0001)),
                                ];
                                let best_venue = dg
                                    .evaluate_perpetual_venue_selection(
                                        &candidate_venues,
                                        hedge_qty * spot,
                                        4.0,
                                    )
                                    .unwrap_or_else(|| "hyperliquid".to_string());

                                tracing::info!(
                                    "options bot portfolio: optimal perpetual rehedge venue selected for {asset}: {best_venue} (qty: {hedge_qty} {perp_symbol}, side: {hedge_side:?})"
                                );

                                rehedge_tasks.push((
                                    asset.clone(),
                                    perp_symbol,
                                    hedge_side,
                                    hedge_qty,
                                    spot,
                                    best_venue,
                                ));
                            }
                        }
                        arbitrageur_risk::delta_guard::BichuchRehedgeDecision::GammaRebalance {
                            gamma_error,
                            reason,
                        } => {
                            tracing::warn!(
                                "options bot portfolio: Bichuch Gamma curvature adjustment recommended for {asset}: gamma error {:.6} ({})",
                                gamma_error,
                                reason
                            );
                        }
                        arbitrageur_risk::delta_guard::BichuchRehedgeDecision::NoAction => {}
                    }
                }

                let (_, correlated_delta) = dg.calculate_correlated_portfolio_delta(
                    &portfolio_all_positions,
                    Some(dec!(0.85)),
                );
                let correlated_limit = dg.max_unhedged_usd_per_asset() * dec!(2.0);
                if correlated_delta > correlated_limit {
                    tracing::warn!(
                        "options bot portfolio: correlated delta ${:.2} exceeds ${:.2} limit",
                        correlated_delta,
                        correlated_limit
                    );
                    crate::alerting::dispatch_throttled(
                        state.env().as_str(),
                        "options_correlated_delta",
                        "portfolio",
                        std::time::Duration::from_secs(300),
                        format!(
                            "Correlated portfolio delta ${:.2} exceeds ${:.2}",
                            correlated_delta, correlated_limit
                        ),
                    );
                }

                // Cross-asset curvature and beta-adjusted BTC-equivalent
                // delta use live mids from the market-data hub (or the vol
                // surface); no hardcoded BTC/ETH fallback is permitted.
                match (live_linear_mid(state, "BTC"), live_linear_mid(state, "ETH")) {
                    (Some(btc_spot), Some(eth_spot)) => {
                        let cross_report = dg.evaluate_cross_asset_risk(
                            &portfolio_greeks,
                            btc_spot,
                            eth_spot,
                            dec!(1.15),
                            dec!(0.85),
                            dec!(15000.0),
                        );
                        if !cross_report.is_portfolio_neutral {
                            tracing::warn!(
                                "options bot portfolio: Cross-asset BTC-equivalent delta skew: {:.4} BTC exceeds neutrality band",
                                cross_report.btc_equivalent_delta
                            );
                            crate::alerting::dispatch_throttled(
                                state.env().as_str(),
                                "options_cross_asset_skew",
                                "portfolio",
                                std::time::Duration::from_secs(300),
                                format!(
                                    "Cross-asset BTC-equivalent delta skew {:.4} BTC exceeds neutrality tolerance",
                                    cross_report.btc_equivalent_delta
                                ),
                            );
                        }
                    }
                    _ => tracing::debug!(
                        "options bot portfolio: live BTC/ETH mids unavailable — cross-asset report skipped"
                    ),
                }
            } // drop(dg) occurs here

            // Execute collected perpetual rehedge orders under the same
            // guardrails as every other live dispatch: pause/breaker checks,
            // the execution lock (a concurrent package or reconciler cycle
            // must not interleave legs), live-entry eligibility in live
            // mode, and a durable ledger intent + client-order-id BEFORE
            // submit. A failed hedge no longer advances the last-hedged
            // spot — doing so silently suppressed the next attempt.
            if !rehedge_tasks.is_empty() {
                let _execution_guard = state.execution_lock.lock().await;
                let gate_err: Option<String> = {
                    let venues: Vec<&str> = rehedge_tasks.iter().map(|t| t.5.as_str()).collect();
                    if let Err(reason) = state.circuit_breaker.is_trading_allowed() {
                        Some(format!("circuit breaker: {reason}"))
                    } else if *state.is_paused.read() {
                        Some("trading engine is PAUSED".to_string())
                    } else if state.is_live() {
                        crate::commands::live_entry_gate(state, &venues).err()
                    } else {
                        None
                    }
                };
                if let Some(reason) = gate_err {
                    tracing::warn!(
                        "options bot portfolio: {} rehedge order(s) withheld — {reason}",
                        rehedge_tasks.len()
                    );
                } else {
                    for (asset, perp_symbol, hedge_side, hedge_qty, spot, best_venue) in
                        rehedge_tasks
                    {
                        let Some(adapter) = state.get_adapter(&best_venue) else {
                            tracing::error!(
                                "options bot portfolio: rehedge venue {best_venue} not admitted — {asset} left unhedged"
                            );
                            continue;
                        };
                        if !adapter.live_order_submission_implemented()
                            || !adapter.has_live_credentials()
                        {
                            tracing::error!(
                                "options bot portfolio: {best_venue} cannot submit live orders — {asset} rehedge skipped"
                            );
                            continue;
                        }
                        let bound_px = match hedge_side {
                            arbitrageur_core::types::Side::Buy => spot * dec!(1.0020), // 20 bps protective slippage limit
                            arbitrageur_core::types::Side::Sell => spot * dec!(0.9980),
                        };
                        let rehedge_order = arbitrageur_core::order::OrderRequest::new_ioc(
                            adapter.venue_id(),
                            &perp_symbol,
                            hedge_side,
                            bound_px.round_dp(2),
                            hedge_qty.round_dp(4),
                        );
                        // Durable intent BEFORE submit: an accepted-then-crashed
                        // hedge stays discoverable by reconciliation.
                        let signal_id = format!(
                            "opt-rehedge-{}-{asset}",
                            chrono::Utc::now().timestamp_millis()
                        );
                        let qty_f64 = hedge_qty.to_string().parse::<f64>().unwrap_or(0.0);
                        if let Err(e) = crate::options_parity::record_options_leg_intent(
                            state,
                            &signal_id,
                            "options_portfolio",
                            best_venue.as_str(),
                            &perp_symbol,
                            hedge_side,
                            qty_f64,
                            &format!(
                                "autonomous portfolio delta rehedge {asset}: {hedge_side:?} {hedge_qty} {perp_symbol} (spot {spot})"
                            ),
                            None,
                        ) {
                            tracing::error!(
                                "options bot portfolio: rehedge intent persist failed — dispatch refused: {e}"
                            );
                            continue;
                        }
                        let leg_side = match hedge_side {
                            arbitrageur_core::types::Side::Buy => crate::state_store::LegSide::Long,
                            arbitrageur_core::types::Side::Sell => {
                                crate::state_store::LegSide::Short
                            }
                        };
                        if let Err(e) = state.state_store.record_leg_client_order_id(
                            &signal_id,
                            leg_side,
                            &rehedge_order.client_order_id,
                        ) {
                            tracing::error!(
                                "options bot portfolio: rehedge client id persist failed — dispatch refused: {e}"
                            );
                            // The intent row exists but will never be matched
                            // to a venue order — retire it so it cannot block
                            // this symbol forever.
                            let _ = state.state_store.update_signal_status(
                                &signal_id,
                                crate::state_store::SignalStatus::Failed,
                                Some("client order id persist failed before dispatch"),
                            );
                            continue;
                        }
                        match adapter.place_order(rehedge_order).await {
                            Ok(resp) => {
                                // Accepted, not necessarily filled: the signal
                                // stays Pending and the reconciler settles it
                                // from venue truth via the persisted client id.
                                tracing::info!(
                                    "options bot portfolio: rehedge SUBMITTED on {best_venue}: id={}, status={:?}, ack filled={} — venue truth settles via reconciliation",
                                    resp.order_id,
                                    resp.status,
                                    resp.filled_qty
                                );
                                update_last_hedged_spot(&asset, spot);
                            }
                            Err(e) => {
                                tracing::error!(
                                    "options bot portfolio: rehedge submission failed on {best_venue}: {e} — {asset} remains unhedged (spot advance withheld)"
                                );
                                let reason = format!("rehedge submission rejected: {e}");
                                let _ = state.state_store.update_signal_status(
                                    &signal_id,
                                    crate::state_store::SignalStatus::Failed,
                                    Some(reason.as_str()),
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    fn note_tick(state: &AppEngineState, st: OptionsBotTickStatus) {
        state
            .options_tick_status
            .lock()
            .insert(st.substrategy.clone(), st);
    }

    async fn tick_substrategy(&self, state: &AppEngineState, substrategy: &str) -> u64 {
        let (cfg, mode) = Self::load_config(state, substrategy);
        let poll = cfg.poll_interval_secs.max(5);
        let mut retired_on_breaker = false;
        if state.circuit_breaker.is_tripped() && mode != BotMode::Off {
            let next = OptionsBotConfigDto::from_policy(substrategy, &cfg, BotMode::Off);
            if let Err(e) = Self::save_config(state, &next) {
                tracing::warn!(
                    "options bot {substrategy}: circuit breaker trip retire to Off failed: {e}"
                );
            }
            let status = OptionsBotTickStatus {
                substrategy: substrategy.to_string(),
                mode: "off".to_string(),
                open_positions: 0,
                max_open_positions: cfg.max_open_positions,
                last_scan_ms: chrono::Utc::now().timestamp_millis(),
                rows_scanned: 0,
                candidates: 0,
                best_edge_usd: None,
                last_refusal: None,
            };
            Self::note_tick(state, status);
            retired_on_breaker = true;
        }
        let mut status = OptionsBotTickStatus {
            substrategy: substrategy.to_string(),
            mode: if retired_on_breaker {
                "off".to_string()
            } else {
                format!("{mode:?}").to_lowercase()
            },
            open_positions: 0,
            max_open_positions: cfg.max_open_positions,
            last_scan_ms: chrono::Utc::now().timestamp_millis(),
            rows_scanned: 0,
            candidates: 0,
            best_edge_usd: None,
            last_refusal: None,
        };
        let strategy_id = Self::ledger_strategy_id(substrategy);
        // Active position supervision runs even when entries are paused,
        // reduce-only, retired by a breaker, or explicitly Off. Expiry and
        // pin-risk exits are safety actions, not strategy entries.
        // 1. Unwinds options parity perpetual hedges upon option expiration (eliminating naked delta).
        // 2. Early-unwinds packages (box spread, jelly roll, convexity) when >= 88% edge is realized
        //    or when within 2h of expiration to bypass Deribit delivery fees & pin risk.
        Self::supervise_open_positions(state, substrategy, &strategy_id).await;

        let Some(mut open) = Self::open_positions(state, &strategy_id) else {
            // Ledger unreadable — cannot prove the book is flat; hold mode,
            // skip entries.
            Self::note_tick(state, status);
            return poll;
        };
        status.open_positions = open;
        if retired_on_breaker || !mode.exits_allowed() {
            Self::note_tick(state, status);
            return poll;
        }
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
            Self::note_tick(state, status);
            return poll;
        }
        if !mode.entries_allowed() || open >= cfg.max_open_positions {
            Self::note_tick(state, status);
            return poll;
        }

        // Pre-flight Deribit margin check: do not scan or attempt entries if Deribit
        // account is under critical margin pressure or margin utilization exceeds 70%.
        if let Ok(deribit_adapter) = crate::options_parity::deribit_for_env(state) {
            if let Ok(acc) = deribit_adapter.fetch_account().await {
                let margin_monitor = crate::commands::margin_monitor_from_settings(state);
                let health = margin_monitor.evaluate_account(&acc);
                if matches!(
                    health,
                    MarginHealthStatus::Critical | MarginHealthStatus::Breached
                ) || acc.margin_usage_pct >= dec!(70)
                {
                    tracing::warn!(
                        "options bot {substrategy}: Deribit account margin health is {:?} (usage {}%) — entry scan skipped",
                        health, acc.margin_usage_pct
                    );
                    Self::note_tick(state, status);
                    return poll;
                }
            }
        }

        let env_str = state.env().as_str();
        let mut open_instruments: std::collections::HashSet<String> = state
            .state_store
            .list_signals(None, None)
            .unwrap_or_default()
            .into_iter()
            .filter(|s| {
                (s.environment.eq_ignore_ascii_case(env_str) || s.environment == "unknown")
                    && s.status.counts_as_open()
            })
            .flat_map(|s| {
                let mut v: Vec<String> = s
                    .legs
                    .into_iter()
                    .map(|l| l.symbol.to_ascii_uppercase())
                    .collect();
                v.push(s.symbol.to_ascii_uppercase());
                v
            })
            .collect();

        // Fresh scan per underlying; each execute re-scans server-side, so a
        // stale edge is refused by the impl, never here.
        for ccy in SCAN_CURRENCIES {
            let scan_qty = if ccy.eq_ignore_ascii_case("ETH") {
                cfg.qty_base.max(1.0)
            } else {
                cfg.qty_base
            };
            // (id, currency, net_edge_usd, leg_instruments) — the two scanners'
            // DTOs project onto one shape for sorting + dispatch.
            let opps: Vec<(String, String, f64, Vec<String>)> = if substrategy == "parity" {
                match scan_options_parity_impl(
                    &OptionsParityScanParams {
                        currency: ccy.to_string(),
                        qty_base: Some(scan_qty),
                        min_hours_to_expiry: Some(cfg.min_hours_to_expiry),
                        max_days_to_expiry: Some(cfg.max_days_to_expiry),
                        min_edge_bps: Some(cfg.min_edge_bps),
                        min_expected_yield_pct: Some(cfg.min_expected_yield_pct),
                        limit: Some(100),
                    },
                    state,
                )
                .await
                {
                    Ok(v) => {
                        status.rows_scanned += v.len();
                        v.into_iter()
                            .filter(|o| {
                                let pin_distance = if o.strike > 0.0 {
                                    (o.index_price_usd - o.strike).abs() / o.strike
                                } else {
                                    1.0
                                };
                                let is_pin_risk = (o.expiry_ms
                                    - chrono::Utc::now().timestamp_millis())
                                    < 172_800_000
                                    && pin_distance < 0.015;
                                o.is_executable
                                    && o.expected_yield_pct >= cfg.min_expected_yield_pct
                                    && o.net_edge_usd >= MIN_OPTIONS_BOT_NET_EDGE_USD
                                    && !is_pin_risk
                            })
                            .map(|o| {
                                let legs = vec![
                                    o.call_leg.instrument.to_ascii_uppercase(),
                                    o.put_leg.instrument.to_ascii_uppercase(),
                                ];
                                (o.id, o.currency, o.net_edge_usd, legs)
                            })
                            .collect()
                    }
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
                        qty_base: Some(scan_qty),
                        min_hours_to_expiry: Some(cfg.min_hours_to_expiry),
                        max_days_to_expiry: Some(cfg.max_days_to_expiry),
                        min_edge_bps: Some(cfg.min_edge_bps),
                        min_expected_yield_pct: Some(cfg.min_expected_yield_pct),
                        limit: Some(100),
                    },
                    state,
                )
                .await
                {
                    Ok(v) => {
                        status.rows_scanned += v.len();
                        v.into_iter()
                            .filter(|o| {
                                let now_ms = chrono::Utc::now().timestamp_millis();
                                let is_pin_risk = (o.expiry_ms - now_ms) < 172_800_000
                                    && o.strikes.iter().any(|&k| {
                                        if o.index_price_usd > 0.0 && k > 0.0 {
                                            (o.index_price_usd - k).abs() / k < 0.015
                                        } else {
                                            false
                                        }
                                    });
                                o.is_executable
                                    && o.expected_yield_pct >= cfg.min_expected_yield_pct
                                    && o.net_edge_usd >= MIN_OPTIONS_BOT_NET_EDGE_USD
                                    && !is_pin_risk
                            })
                            .map(|o| {
                                let legs = o
                                    .legs
                                    .iter()
                                    .map(|l| l.instrument.to_ascii_uppercase())
                                    .collect();
                                (o.id, o.currency, o.net_edge_usd, legs)
                            })
                            .collect()
                    }
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
            status.candidates += opps.len();
            if let Some(best) = opps.first().map(|o| o.2) {
                status.best_edge_usd =
                    Some(
                        status
                            .best_edge_usd
                            .map_or(best, |b| if best > b { best } else { b }),
                    );
            }
            // Drop candidates still cooling from a prior refusal or sharing an
            // instrument with an already-open signal, then prune expired
            // cooldown entries so the map stays bounded.
            {
                let mut cd = self.refusal_cooldown.lock();
                cd.retain(|_, t| t.elapsed() < REFUSAL_COOLDOWN);
                opps.retain(|(id, _, _, legs)| {
                    !cd.contains_key(&Self::cooldown_key(substrategy, id))
                        && !legs.iter().any(|inst| open_instruments.contains(inst))
                });
            }
            for (id, _ccy, _edge, legs) in opps {
                if open >= cfg.max_open_positions {
                    break;
                }
                if legs.iter().any(|inst| open_instruments.contains(inst)) {
                    continue;
                }
                let res = if substrategy == "parity" {
                    execute_options_parity_impl(
                        OptionsParityExecutionParams {
                            id: id.clone(),
                            currency: ccy.to_string(),
                            qty_base: scan_qty,
                            min_edge_bps: Some(cfg.min_edge_bps),
                            min_expected_yield_pct: Some(cfg.min_expected_yield_pct),
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
                            qty_base: scan_qty,
                            min_edge_bps: Some(cfg.min_edge_bps),
                            min_expected_yield_pct: Some(cfg.min_expected_yield_pct),
                            auto: true,
                        },
                        state,
                    )
                    .await
                    .map(|_| ())
                };
                match res {
                    Ok(()) => {
                        open += 1;
                        status.open_positions = open;
                        open_instruments.extend(legs);
                    }
                    Err(e) => {
                        tracing::warn!("options bot {substrategy}: entry {id} refused: {e}");
                        status.last_refusal = Some(format!("{id}: {e}"));
                        self.note_refusal(substrategy, &id);
                        if e.contains("An open position already exists for")
                            || e.contains("is blocked by signal")
                            || e.contains("is no longer available")
                        {
                            continue;
                        }
                        break;
                    }
                }
            }
        }
        Self::note_tick(state, status);
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

/// Spawns background worker that polls Deribit options book summaries for BTC and ETH,
/// dynamically feeding live synthetic forward prices, mark IVs, and index spots into
/// `state.vol_surface` (`LiveVolSurfaceCache`).
/// Executes SABR parameter calibrations off-thread via `tokio::task::spawn_blocking`
/// to avoid async runtime threadpool starvation.
pub fn spawn_volatility_surface_worker(
    state: Arc<AppEngineState>,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(2500));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        tracing::info!(
            "volatility surface cache worker started: polling Deribit BTC/ETH book summaries"
        );

        loop {
            interval.tick().await;

            if *state.is_stopped.read() {
                tracing::info!("volatility surface worker exiting on engine stop");
                break;
            }

            let deribit_adapter = match crate::options_parity::deribit_for_env(&state) {
                Ok(a) => a,
                Err(_) => {
                    // Deribit not admitted or unconfigured in this environment
                    continue;
                }
            };

            for ccy in SCAN_CURRENCIES {
                match deribit_adapter.book_summaries(ccy).await {
                    Ok(summaries) => {
                        let vol_cache = state.vol_surface.clone();
                        let ccy_str = ccy.to_string();
                        let store = state.state_store.clone();
                        let env_str = state.env().as_str().to_string();
                        // Run parsing and SABR calibrations in spawn_blocking to keep async reactor responsive
                        let _ = tokio::task::spawn_blocking(move || {
                            let now_ms = chrono::Utc::now().timestamp_millis();
                            let mut ingested_count = 0usize;
                            let mut variance_quotes: HashMap<
                                String,
                                (f64, Vec<arbitrageur_math::variance_swap::DiscreteOptionQuote>),
                            > = HashMap::new();

                            for summary in summaries {
                                // Extract spot / estimated delivery price if present
                                if let Some(est_delivery) = summary.estimated_delivery_price {
                                    if let Ok(spot_f64) = est_delivery.to_string().parse::<f64>() {
                                        if spot_f64 > 0.0 {
                                            vol_cache.update_spot(&ccy_str, spot_f64);
                                        }
                                    }
                                }

                                // Instrument name e.g. "BTC-27OCT23-30000-C"
                                let parts: Vec<&str> = summary.name.split('-').collect();
                                if parts.len() < 4 {
                                    continue;
                                }
                                let asset = parts[0];
                                let expiry_str = parts[1];
                                let Ok(strike) = parts[2].parse::<f64>() else {
                                    continue;
                                };

                                let Some(expiry_ms) = parse_option_expiry_ms(expiry_str) else {
                                    continue;
                                };
                                let Some(tenor_years) = arbitrageur_risk::delta_guard::parse_expiry_tenor_years(expiry_str, now_ms) else {
                                    continue;
                                };

                                // Forward price from underlying_price or estimated_delivery_price
                                let forward_opt = summary
                                    .underlying_price
                                    .or(summary.estimated_delivery_price)
                                    .and_then(|p| p.to_string().parse::<f64>().ok());

                                let Some(forward_price) = forward_opt else {
                                    continue;
                                };
                                let is_call = parts
                                    .get(3)
                                    .is_some_and(|kind| kind.eq_ignore_ascii_case("C"));
                                if (is_call && strike >= forward_price)
                                    || (!is_call && strike < forward_price)
                                {
                                    let quote = match (
                                        summary.bid.and_then(|v| v.to_string().parse::<f64>().ok()),
                                        summary.ask.and_then(|v| v.to_string().parse::<f64>().ok()),
                                    ) {
                                        (Some(bid), Some(ask))
                                            if bid.is_finite()
                                                && ask.is_finite()
                                                && bid >= 0.0
                                                && ask >= bid =>
                                        {
                                            Some(
                                                arbitrageur_math::variance_swap::DiscreteOptionQuote::new(
                                                    strike,
                                                    bid * forward_price,
                                                    ask * forward_price,
                                                    is_call,
                                                ),
                                            )
                                        }
                                        _ => None,
                                    };
                                    if let Some(quote) = quote {
                                        let entry = variance_quotes
                                            .entry(expiry_str.to_string())
                                            .or_insert_with(|| (forward_price, Vec::new()));
                                        entry.0 = forward_price;
                                        entry.1.push(quote);
                                    }
                                }

                                // Mark IV is given in percent (e.g. 53.58 -> 0.5358)
                                if let Some(iv_dec) = summary.mark_iv {
                                    if let Ok(iv_raw) = iv_dec.to_string().parse::<f64>() {
                                        let fractional_iv = iv_raw * 0.01;
                                        if fractional_iv > 0.01 && fractional_iv < 5.0 && forward_price > 0.0 && strike > 0.0 {
                                            vol_cache.ingest_quote(
                                                asset,
                                                expiry_str,
                                                expiry_ms,
                                                tenor_years,
                                                forward_price,
                                                strike,
                                                fractional_iv,
                                            );
                                            ingested_count += 1;
                                        }
                                    }
                                }
                            }

                            // Prune expired slices
                            vol_cache.prune_expired(now_ms);

                            let replicator =
                                arbitrageur_math::variance_swap::VarianceSwapReplicator::default();
                            let dispersion_engine =
                                arbitrageur_strategy::options_dispersion::VarianceSwapDispersionEngine::new(
                                    0.04, 0.05,
                                );
                            let realized_vol = realized_vol_30d(&store, &ccy_str, &env_str);
                            let mut calendar_slices = Vec::new();
                            for (expiry, (forward, quotes)) in variance_quotes {
                                if quotes.len() < 3 {
                                    continue;
                                }
                                let slice_svi = {
                                    vol_cache
                                        .slices
                                        .read()
                                        .get(&ccy_str.to_ascii_uppercase())
                                        .and_then(|slices| slices.get(&expiry))
                                        .cloned()
                                        .and_then(|slice| {
                                            let mut strikes = Vec::new();
                                            let mut ivs = Vec::new();
                                            for (strike_key, iv) in &slice.observed_strikes {
                                                strikes.push(*strike_key as f64 / 100.0);
                                                ivs.push(*iv);
                                            }
                                            arbitrageur_math::svi_arbitrage_free::SviSliceParameters::fit_from_iv_smile(
                                                &strikes,
                                                &ivs,
                                                slice.forward_price,
                                                slice.tenor_years,
                                            )
                                        })
                                }
                                .map(|svi| {
                                    arbitrageur_math::variance_swap::SviParameters::new(
                                        svi.a, svi.b, svi.rho, svi.m, svi.sigma,
                                    )
                                });
                                let Some(tenor) =
                                    arbitrageur_risk::delta_guard::parse_expiry_tenor_years(
                                        &expiry, now_ms,
                                    )
                                else {
                                    continue;
                                };
                                let variance_strike = if let Some(svi) = slice_svi.as_ref() {
                                    replicator.replicate_variance_strike_svi_tails(
                                        forward,
                                        tenor,
                                        quotes.clone(),
                                        svi,
                                    )
                                } else {
                                    replicator.replicate_variance_strike(
                                        forward, tenor, quotes,
                                    )
                                };
                                let Some(variance_strike) = variance_strike else {
                                    continue;
                                };
                                calendar_slices.push((
                                    expiry.clone(),
                                    tenor,
                                    variance_strike,
                                ));
                                let prefix = format!("variance_swap.{ccy_str}.{expiry}");
                                let _ = store.kv_set(
                                    &format!("{prefix}.variance_strike"),
                                    &variance_strike.to_string(),
                                );
                                if let Some(realized_vol) = realized_vol {
                                    let vrp =
                                        replicator.compute_vrp(variance_strike, realized_vol);
                                    let _ = store.kv_set(
                                        &format!("{prefix}.realized_vol_30d"),
                                        &realized_vol.to_string(),
                                    );
                                    let _ = store
                                        .kv_set(&format!("{prefix}.vrp_vol_pts"), &vrp.to_string());
                                    tracing::info!(
                                        "variance telemetry {ccy_str}/{expiry}: synthetic variance strike {:.6}, RV30 {:.4}, VRP {:+.4} vol pts",
                                        variance_strike,
                                        realized_vol,
                                        vrp
                                    );
                                } else {
                                    tracing::info!(
                                        "variance telemetry {ccy_str}/{expiry}: synthetic variance strike {:.6} (RV unavailable)",
                                        variance_strike
                                    );
                                }
                                let _ = store.kv_set(
                                    &format!("{prefix}.updated_at_ms"),
                                    &now_ms.to_string(),
                                );
                            }

                            calendar_slices.sort_by(|a, b| a.1.total_cmp(&b.1));
                            if let Some(realized_vol) = realized_vol {
                                for pair in calendar_slices.windows(2) {
                                    let (front_expiry, front_t, front_var) = &pair[0];
                                    let (back_expiry, back_t, back_var) = &pair[1];
                                    let opportunity = dispersion_engine
                                        .evaluate_calendar_vrp_opportunity(
                                            &ccy_str,
                                            *front_t,
                                            *front_var,
                                            realized_vol,
                                            *back_t,
                                            *back_var,
                                            realized_vol,
                                            0.02,
                                        );
                                    let prefix = format!(
                                        "variance_swap.{ccy_str}.calendar.{front_expiry}.{back_expiry}"
                                    );
                                    let _ = store.kv_set(
                                        &format!("{prefix}.calendar_spread"),
                                        &opportunity.calendar_spread.to_string(),
                                    );
                                    let _ = store.kv_set(
                                        &format!("{prefix}.action"),
                                        &format!("{:?}", opportunity.action),
                                    );
                                    tracing::info!(
                                        "calendar VRP {ccy_str}/{front_expiry}->{back_expiry}: spread {:+.4}, action {:?}",
                                        opportunity.calendar_spread,
                                        opportunity.action
                                    );
                                }
                            }

                            if ccy_str == "ETH" {
                                if let Some((expiry, opportunity)) =
                                    evaluate_cached_variance_dispersion(
                                        &vol_cache,
                                        &store,
                                        &env_str,
                                    )
                                {
                                    let prefix = format!(
                                        "variance_swap.dispersion.BTC.ETH.{expiry}"
                                    );
                                    let _ = store.kv_set(
                                        &format!("{prefix}.vrp_spread"),
                                        &opportunity.vrp_spread.to_string(),
                                    );
                                    let _ = store.kv_set(
                                        &format!("{prefix}.action"),
                                        &format!("{:?}", opportunity.action),
                                    );
                                    tracing::info!(
                                        "variance dispersion BTC/ETH {expiry}: spread {:+.4}, action {:?}",
                                        opportunity.vrp_spread,
                                        opportunity.action
                                    );
                                }
                            }

                            if ingested_count > 0 {
                                tracing::trace!(
                                    "volatility surface cache: updated {ccy_str} with {ingested_count} quotes (total slices: {}, total quotes: {})",
                                    vol_cache.total_slices_count(),
                                    vol_cache.total_quotes_count()
                                );
                            }
                        })
                        .await;
                    }
                    Err(e) => {
                        tracing::debug!("deribit book_summaries poll failed for {ccy}: {e}");
                    }
                }
            }
        }
    })
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
            min_expected_yield_pct: 30.0,
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
        assert_eq!(policy.min_expected_yield_pct, 30.0);

        // Package substrategies arm independently and keep custom expected yield.
        let mut box_dto = armed("box_spread");
        box_dto.min_expected_yield_pct = 35.5;
        OptionsBotExecutor::save_config(&st, &box_dto).expect("box bot arms");
        let (box_policy, mode) = OptionsBotExecutor::load_config(&st, "box_spread");
        assert_eq!(mode, BotMode::Auto);
        assert_eq!(box_policy.min_expected_yield_pct, 35.5);

        // Pending substrategies cannot arm.
        assert!(OptionsBotExecutor::save_config(&st, &armed("dividend")).is_err());

        // Bad mode fails closed — never persists.
        let mut bad = armed("parity");
        bad.mode = "yolo".to_string();
        assert!(OptionsBotExecutor::save_config(&st, &bad).is_err());
        let (_, mode) = OptionsBotExecutor::load_config(&st, "parity");
        assert_eq!(mode, BotMode::Auto); // previous config untouched

        // Bad expected yield (< 30.0, > 10000, NaN) fails validation.
        let mut bad_yield = armed("parity");
        bad_yield.min_expected_yield_pct = 29.9;
        assert!(OptionsBotExecutor::save_config(&st, &bad_yield).is_err());
        bad_yield.min_expected_yield_pct = -1.0;
        assert!(OptionsBotExecutor::save_config(&st, &bad_yield).is_err());
        bad_yield.min_expected_yield_pct = f64::NAN;
        assert!(OptionsBotExecutor::save_config(&st, &bad_yield).is_err());
        bad_yield.min_expected_yield_pct = 10_001.0;
        assert!(OptionsBotExecutor::save_config(&st, &bad_yield).is_err());

        // Backward compatibility: JSON missing minExpectedYieldPct defaults to 30.0.
        let legacy_json = r#"{
            "substrategyId": "parity",
            "enabled": true,
            "mode": "auto",
            "qtyBase": 0.1,
            "minEdgeBps": 5.0,
            "minHoursToExpiry": 48.0,
            "maxDaysToExpiry": 60.0,
            "pollIntervalSecs": 30,
            "maxOpenPositions": 2
        }"#;
        let legacy_dto: OptionsBotConfigDto =
            serde_json::from_str(legacy_json).expect("deserialize legacy JSON");
        assert_eq!(legacy_dto.min_expected_yield_pct, 30.0);
        let (legacy_policy, _) = legacy_dto.to_policy().expect("convert legacy to policy");
        assert_eq!(legacy_policy.min_expected_yield_pct, 30.0);
    }

    #[test]
    fn refusal_cooldown_scopes_per_substrategy() {
        let bot = OptionsBotExecutor::new();
        bot.note_refusal("parity", "opar-BTC-1-2-conversion");
        assert!(bot.is_cooling("parity", "opar-BTC-1-2-conversion"));
        // Same id under another substrategy must not share the window.
        assert!(!bot.is_cooling("box_spread", "opar-BTC-1-2-conversion"));
        assert!(!bot.is_cooling("parity", "opar-ETH-1-2-conversion"));
        // An expired entry stops cooling.
        bot.refusal_cooldown.lock().insert(
            OptionsBotExecutor::cooldown_key("parity", "old"),
            Instant::now() - REFUSAL_COOLDOWN - Duration::from_secs(1),
        );
        assert!(!bot.is_cooling("parity", "old"));
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

    #[test]
    fn test_parse_option_expiry_ms() {
        // Deribit date formatted symbol: 25DEC26 08:00 UTC
        let exp_call = parse_option_expiry_ms("BTC-25DEC26-125000-C");
        assert!(exp_call.is_some());
        assert_eq!(exp_call.unwrap(), 1798185600000);

        // Opportunity ID with embedded millisecond timestamp
        let exp_opp = parse_option_expiry_ms("opar-BTC-1766707200000-88000-deadbeef");
        assert_eq!(exp_opp, Some(1766707200000));

        // Non-option tickers fail cleanly
        assert_eq!(parse_option_expiry_ms("BTC-PERPETUAL"), None);
        assert_eq!(parse_option_expiry_ms("ETH-USDC"), None);
    }

    #[tokio::test]
    async fn test_supervise_open_positions_safely_runs() {
        let st = state();
        OptionsBotExecutor::supervise_open_positions(&st, "parity", "options_parity").await;
        OptionsBotExecutor::supervise_open_positions(&st, "box_spread", "options_box_spread").await;
    }

    #[tokio::test]
    async fn test_volatility_surface_worker_spawns_and_exits() {
        let st = Arc::new(state());
        *st.is_stopped.write() = true;
        let handle = spawn_volatility_surface_worker(st.clone());
        let res = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(
            res.is_ok(),
            "worker should terminate promptly when engine is stopped"
        );
    }

    #[tokio::test]
    async fn test_volatility_surface_cache_greeks_integration() {
        let st = Arc::new(state());
        // Seed surface cache with live BTC quote
        st.vol_surface.update_spot("BTC", 65000.0);
        st.vol_surface.ingest_quote(
            "BTC",
            "25DEC26",
            1798185600000,
            1.25,
            66000.0,
            65000.0,
            0.55,
        );
        assert_eq!(st.vol_surface.get_spot("BTC"), Some(65000.0));
        assert!(st.vol_surface.total_quotes_count() > 0);
        OptionsBotExecutor::supervise_open_positions(&st, "parity", "options_parity").await;
    }
}
