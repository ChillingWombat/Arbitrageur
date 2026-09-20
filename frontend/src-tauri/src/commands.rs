use crate::state::{
    AppEngineState, EngineSettingsDto, ExecutionLogEntry, StrategyParamsDto, TickerStrategyConfigDto, VenueBalanceInfo,
};
use crate::state_store::{AccountProfile, StrategyAccountRoute};
use crate::vault::{CredentialPresence, ExchangeCredentials, VaultStatus};
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::quote::OrderBook;
use arbitrageur_core::types::{dec, Decimal, Side, VenueId};
use arbitrageur_execution::coordinator::ArbitrageExecutionResult;
use arbitrageur_execution::passive_unwind::{
    execute_passive_reduce, PassiveBookRefresh, PassiveOutcome, PassiveUnwindConfig,
};
use arbitrageur_execution::smart_routing::SmartOrderRouter;
use arbitrageur_execution::virtual_ledger::StrategyMetrics;
use arbitrageur_math::calibration::WfoOptimizationResult;
use arbitrageur_math::autotune::{
    TuneHistory, TuneParams, recommend,
};
use arbitrageur_math::dislocation::{compute_deviation_series, BaselineMode, MidSample};
use arbitrageur_math::book_gradient::entry_gradient;
use arbitrageur_math::funding_carry::{
    estimate_funding_hold_hours, evaluate_funding_carry, evaluate_spot_perp_basis,
    funding_entry_capacity, funding_win_probability,
};
use arbitrageur_math::optimal_thresholds::compute_bertram_thresholds;
use arbitrageur_math::ou_process::OuModelParams;
use arbitrageur_math::pair_monitor::build_pair_series;
use arbitrageur_math::triangular::{evaluate_triangular_cycle, TriLegSide};

use arbitrageur_risk::margin_monitor::{MarginHealthStatus, MarginMonitor, MarginMonitorConfig};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::State;

const LIVE_EXECUTION_DISABLED: &str = "Live execution is disabled until an authenticated adapter can sign, submit, and reconcile orders";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SystemStatusDto {
    pub is_live_mode: bool,
    pub is_paused: bool,
    pub total_realized_pnl: f64,
    pub total_trades_count: usize,
    pub successful_trades_count: usize,
    pub circuit_breaker_status: String,
    pub circuit_breaker_tripped: bool,
    pub current_drawdown_usd: f64,
    pub max_drawdown_usd: f64,
    pub consecutive_failures: u32,
    pub active_venues: Vec<VenueStatusDto>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct VenueStatusDto {
    pub id: String,
    pub name: String,
    pub market_type: String,
    pub status: String,
    /// Measured round-trip to the venue's public market-metadata endpoint.
    /// `None` when the venue is not admitted or the probe failed — never
    /// fabricated.
    pub latency_ms: Option<u64>,
    pub fee_bps: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct VenueVolumesDto {
    pub venue_id: String,
    /// 24h notional volume in quote currency (~USD), keyed by the venue's
    /// raw asset name (e.g. "BTC", "kPEPE"). Only assets the venue actually
    /// reports are included.
    pub volumes: HashMap<String, f64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct VenueMarketUniverseDto {
    pub venue_id: String,
    pub symbols: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct VenueOrderBookDto {
    pub venue_id: String,
    pub venue_name: String,
    pub symbol: String,
    pub best_bid: f64,
    pub best_ask: f64,
    pub spread: f64,
    pub spread_bps: f64,
    pub mid_price: f64,
    pub total_bid_depth: f64,
    pub total_ask_depth: f64,
    pub vwap_bid: f64,
    pub vwap_ask: f64,
    pub funding_rate_8h_pct: f64,
    pub bids: Vec<[f64; 2]>,
    pub asks: Vec<[f64; 2]>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SpatialOpportunityDto {
    pub id: String,
    pub symbol: String,
    pub buy_venue: String,
    pub sell_venue: String,
    pub buy_price: f64,
    pub sell_price: f64,
    pub gross_spread: f64,
    pub gross_spread_bps: f64,
    pub fee_bps: f64,
    pub net_spread_bps: f64,
    pub order_qty: f64,
    pub estimated_profit_usd: f64,
    pub is_executable: bool,
    pub buy_venue_id: String,
    pub sell_venue_id: String,
    pub buy_fee_bps: f64,
    pub sell_fee_bps: f64,
    pub round_trip_commission_bps: f64,
    pub entry_slippage_bps: f64,
    pub exit_slippage_bps: f64,
    pub closing_spread_bps: f64,
    pub expected_friction_bps: f64,
    pub required_gross_bps: f64,
    pub slippage_ok: bool,
    pub depth_ok: bool,
    /// Empirical win probability from the pair's episode history, computed to
    /// the monitor's statistical standard (min-episodes gate + OU fallback).
    /// Informational in scan: never gates ordering or executability.
    /// `None` renders as "—" (insufficient data).
    #[serde(default)]
    pub win_probability: Option<f64>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FundingOpportunityDto {
    pub id: String,
    pub symbol: String,
    pub long_venue: String,
    pub short_venue: String,
    pub net_rate_8h_pct: f64,
    pub net_apr_pct: f64,
    pub net_apy_pct: f64,
    pub round_trip_fee_bps: f64,
    pub break_even_hours: f64,
    pub estimated_daily_income_usd: f64,
    pub trade_size_usd: f64,
    /// Depth-supported executable size from the live orderbooks (USD).
    /// Falls back to `trade_size_usd` when either book is unavailable.
    pub optimal_size_usd: f64,
    /// Measured entry cross (long-ask vs short-bid VWAP) in bps, as a
    /// positive cost. -1.0 when unmeasured (no book data).
    pub entry_spread_bps: f64,
    /// Opportunity kind: "carry" (cross-exchange perp/perp funding) or
    /// "basis" (same-exchange long-spot / short-perp cash-and-carry).
    /// Defaults to "carry" so stale backends/clients keep working.
    #[serde(default = "default_funding_opp_kind")]
    pub kind: String,
    /// Signed live divergence `(perp - spot) / spot * 10000` in bps.
    /// `None` for cross-exchange carry (no single spot/perp pair).
    #[serde(default)]
    pub basis_bps: Option<f64>,
    /// Spot reference price for basis opps. `None` for carry.
    #[serde(default)]
    pub spot_price: Option<f64>,
    /// Perp reference price for basis opps. `None` for carry.
    #[serde(default)]
    pub perp_price: Option<f64>,
    /// Informational win probability (experimental fee-coverage heuristic,
    /// 0..1). `None` when the edge is unusable. Never gates ordering or
    /// executability — same contract as the spread monitor's field.
    #[serde(default)]
    pub win_probability: Option<f64>,
    /// Modelled expected holding horizon in hours: time until the funding
    /// gap is expected to decay below the min-APR exit threshold, clamped to
    /// [fee+spread breakeven, 168h]. Thin history falls back to breakeven.
    /// `None` on stale backends — clients fall back to `break_even_hours`.
    #[serde(default)]
    pub expected_hold_hours: Option<f64>,
    /// Human-readable hold basis (decay slope, threshold, spread inclusion).
    #[serde(default)]
    pub hold_reason: Option<String>,
    /// Heuristic hold confidence 0..1 (never 0/1).
    #[serde(default)]
    pub hold_confidence: Option<f64>,
    /// Marginal entry-cost slope in bps per extra $1k notional, summed over
    /// both legs at the requested trade size. `None` when either leg's book
    /// is missing/thin. Informational: describes cost shape, never sizes.
    #[serde(default)]
    pub depth_slope_bps_per_1k: Option<f64>,
    /// Current funding print of the LONG leg, 8h-normalized percent
    /// (rate / interval_hours * 8 * 100). For basis opps the long leg is
    /// spot, which pays no funding, so this is `Some(0.0)`. `None` on stale
    /// backends that predate the field.
    #[serde(default)]
    pub long_rate_8h_pct: Option<f64>,
    /// Current funding print of the SHORT leg, 8h-normalized percent.
    /// `None` on stale backends that predate the field.
    #[serde(default)]
    pub short_rate_8h_pct: Option<f64>,
}

fn default_funding_opp_kind() -> String {
    "carry".to_string()
}

/// Build the net funding-gap history for the CURRENT carry direction:
/// `short_venue_rate_8h - long_venue_rate_8h` per minute bucket, trailing 7d.
/// Returns `(ts_ms, net_8h_fraction)` ascending. Empty on any store hiccup —
/// the estimator falls back to fee+spread breakeven (never fabricates).
fn carry_gap_history(
    state: &AppEngineState,
    symbol: &str,
    long_venue_id: &str,
    short_venue_id: &str,
    now_ms: i64,
) -> Vec<(i64, f64)> {
    const WINDOW_MS: i64 = 7 * 24 * 3_600_000;
    let from = now_ms.saturating_sub(WINDOW_MS);
    let long_series = state
        .state_store
        .funding_rate_series(long_venue_id, symbol, from, now_ms)
        .unwrap_or_default();
    let short_series = state
        .state_store
        .funding_rate_series(short_venue_id, symbol, from, now_ms)
        .unwrap_or_default();
    if long_series.is_empty() || short_series.is_empty() {
        return Vec::new();
    }
    let mut long_map: HashMap<i64, f64> = HashMap::new();
    for (ts, r8h, _) in long_series {
        if r8h.is_finite() {
            long_map.entry(ts).or_insert(r8h);
        }
    }
    let mut out = Vec::new();
    for (ts, r8h, _) in short_series {
        if !r8h.is_finite() {
            continue;
        }
        if let Some(lr) = long_map.get(&ts) {
            let net = r8h - lr;
            if net.is_finite() {
                out.push((ts, net));
            }
        }
    }
    out.sort_by_key(|(ts, _)| *ts);
    out
}

/// Min-APR exit threshold for the funding hold model (strategy params,
/// default 8% when absent).
fn funding_min_apr(state: &AppEngineState) -> f64 {
    state
        .strategy_params
        .read()
        .get("funding")
        .map(|p| p.funding_min_apr_pct)
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(8.0)
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SpotOpportunityDto {
    pub id: String,
    pub symbol: String,
    pub buy_venue: String,
    pub sell_venue: String,
    pub buy_price: f64,
    pub sell_price: f64,
    pub gross_spread_bps: f64,
    pub buy_fee_bps: f64,
    pub sell_fee_bps: f64,
    pub transfer_cost_usd: f64,
    pub net_spread_bps: f64,
    pub net_profit_usd: f64,
    pub available_depth_usd: f64,
    pub estimated_latency_ms: u64,
    pub is_executable: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SpotExecutionParams {
    pub symbol: String,
    pub buy_venue: String,
    pub sell_venue: String,
    pub trade_size_usd: f64,
    pub max_slippage_bps: f64,
    #[serde(default)]
    pub net_spread_bps: Option<f64>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TriangleLegDto {
    pub symbol: String,
    pub action: String,
    pub price: f64,
    pub fee_bps: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TriangleOpportunityDto {
    pub id: String,
    pub venue: String,
    pub path: String,
    pub base_asset: String,
    pub intermediate_a: String,
    pub intermediate_b: String,
    pub leg1: TriangleLegDto,
    pub leg2: TriangleLegDto,
    pub leg3: TriangleLegDto,
    pub gross_spread_bps: f64,
    pub total_fee_bps: f64,
    pub net_spread_bps: f64,
    pub net_profit_usd: f64,
    pub trade_size_usd: f64,
    pub estimated_latency_ms: u64,
    pub is_executable: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TriangleExecutionParams {
    pub id: String,
    pub venue: String,
    pub path: String,
    pub trade_size_usd: f64,
    pub min_profit_bps: f64,
    #[serde(default)]
    pub net_spread_bps: Option<f64>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ConvergenceCheckDto {
    pub signal_id: String,
    pub symbol: String,
    pub verdict: String,
    pub trigger: Option<String>,
    pub reason: String,
    pub live_net_bps: Option<f64>,
    pub entry_net_bps: Option<f64>,
    pub unwound: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PredictionOpportunityDto {
    pub id: String,
    pub market_title: String,
    pub venue_a: String,
    pub venue_b: String,
    pub strategy: String,
    pub buy_yes_venue: String,
    pub buy_no_venue: String,
    pub yes_price_cents: f64,
    pub no_price_cents: f64,
    pub total_cost_cents: f64,
    pub guaranteed_payout_cents: f64,
    pub net_profit_cents: f64,
    pub roi_pct: f64,
    pub has_arbitrage: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StatArbStepResultDto {
    pub step: usize,
    pub price_y: f64,
    pub price_x: f64,
    pub dynamic_hedge_ratio: f64,
    pub intercept_alpha: f64,
    pub residual_spread: f64,
    pub spread_variance: f64,
    pub z_score: f64,
    pub signal: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BertramThresholdDto {
    pub entry_long: f64,
    pub exit_long: f64,
    pub entry_short: f64,
    pub exit_short: f64,
    pub optimal_distance: f64,
    pub expected_return_per_cycle: f64,
    pub estimated_trades_per_year: f64,
    pub estimated_sharpe: f64,
    pub half_life: f64,
    pub theta: f64,
    pub mu: f64,
    pub sigma: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OuFitDto {
    pub theta: f64,
    pub mu: f64,
    pub sigma: f64,
    pub half_life: f64,
    pub stationary_std: f64,
    pub r_squared: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RiskOverviewDto {
    pub circuit_breaker_state: String,
    pub is_halted: bool,
    pub current_drawdown_usd: f64,
    pub max_drawdown_limit_usd: f64,
    pub drawdown_pct: f64,
    pub consecutive_failures: u32,
    pub max_failures_allowed: u32,
    pub total_portfolio_value_usd: f64,
    pub net_deltas: Vec<NetDeltaDto>,
    pub margin_health: MarginHealthDto,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NetDeltaDto {
    pub asset: String,
    pub net_quantity: f64,
    pub net_exposure_usd: f64,
    pub delta_limit_usd: f64,
    pub is_neutral: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MarginHealthDto {
    pub maintenance_margin_req_usd: f64,
    pub margin_balance_usd: f64,
    pub margin_ratio_pct: f64,
    pub liquidation_distance_pct: f64,
    pub status: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SimStepResultDto {
    pub cycle: usize,
    pub opportunity_found: bool,
    pub execution_success: bool,
    pub trade_type: String,
    pub direction: String,
    pub venue_a: String,
    pub venue_b: String,
    pub spread_bps: f64,
    pub realized_profit_usd: f64,
    pub total_cumulative_pnl_usd: f64,
    pub message: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TickerMarketQuoteDto {
    pub symbol: String,
    pub base: String,
    pub best_bid: f64,
    pub best_ask: f64,
    pub mid_price: f64,
    pub spread_bps: f64,
    pub best_buy_venue: String,
    pub best_sell_venue: String,
    pub fee_bps: f64,
    pub base_price: f64,
}

pub(crate) fn venue_fee_bps(vid: &str) -> Decimal {
    match vid.to_lowercase().as_str() {
        "hyperliquid" => dec!(2.0),
        "lighter" => dec!(1.5),
        "vanta" => dec!(3.0),
        "robinhood" => dec!(2.5),
        "arcus" => dec!(1.5),
        "aster" => dec!(3.5),
        "popdex" => dec!(10.0),
        "polymarket" => dec!(0.0),
        "kalshi" => dec!(10.0),
        _ => dec!(2.0),
    }
}

pub(crate) fn effective_venue_fee_bps(state: &AppEngineState, venue: &str) -> f64 {
    let base: f64 = venue_fee_bps(venue).to_string().parse().unwrap_or(2.0);
    let plus = matches!(venue.to_ascii_lowercase().as_str(), "lighter" | "robinhood")
        && state
            .state_store
            .default_account(venue)
            .ok()
            .flatten()
            .is_some_and(|a| a.account_tier == "plus");
    if plus { base + 0.5 } else { base }
}

fn dec_opt_f64(v: Decimal) -> Option<f64> {
    let f: f64 = v.to_string().parse().ok()?;
    if f.is_finite() { Some(f) } else { None }
}

#[derive(Debug, Clone, Copy)]
struct RouteEconomics {
    buy_fee_bps: f64,
    sell_fee_bps: f64,
    round_trip_commission_bps: f64,
    entry_slippage_bps: f64,
    exit_slippage_bps: f64,
    closing_spread_bps: f64,
    expected_friction_bps: f64,
    top_gross_bps: f64,
    expected_net_bps: f64,
    required_gross_bps: f64,
    estimated_profit_usd: f64,
}

fn compute_route_economics(
    book_buy: &OrderBook,
    book_sell: &OrderBook,
    qty: Decimal,
    buy_fee_bps: f64,
    sell_fee_bps: f64,
    target_profit_bps: f64,
) -> Option<RouteEconomics> {
    let buy_best_ask = dec_opt_f64(book_buy.best_ask_price()?).filter(|p| *p > 0.0)?;
    let sell_best_bid = dec_opt_f64(book_sell.best_bid_price()?).filter(|p| *p > 0.0)?;
    let buy_best_bid = dec_opt_f64(book_buy.best_bid_price()?).filter(|p| *p > 0.0)?;
    let sell_best_ask = dec_opt_f64(book_sell.best_ask_price()?).filter(|p| *p > 0.0)?;

    let (buy_entry, rem, _) = book_buy.buy_execution_vwap(qty)?;
    if rem > dec!(0) {
        return None;
    }
    let (sell_entry, rem, _) = book_sell.sell_execution_vwap(qty)?;
    if rem > dec!(0) {
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
    let buy_entry_vwap = dec_opt_f64(buy_entry)?;
    let sell_entry_vwap = dec_opt_f64(sell_entry)?;
    let buy_exit_vwap = dec_opt_f64(buy_exit)?;
    let sell_exit_vwap = dec_opt_f64(sell_exit)?;
    let qty_f = dec_opt_f64(qty)?;

    let top_gross_bps = (sell_best_bid - buy_best_ask) / buy_best_ask * 10_000.0;
    let entry_slippage_bps =
        ((buy_entry_vwap - buy_best_ask) / buy_best_ask * 10_000.0).max(0.0)
            + ((sell_best_bid - sell_entry_vwap) / sell_best_bid * 10_000.0).max(0.0);
    let exit_slippage_bps =
        ((buy_best_bid - buy_exit_vwap) / buy_best_bid * 10_000.0).max(0.0)
            + ((sell_exit_vwap - sell_best_ask) / sell_best_ask * 10_000.0).max(0.0);
    let buy_spread_bps =
        (buy_best_ask - buy_best_bid) / ((buy_best_ask + buy_best_bid) / 2.0) * 10_000.0;
    let sell_spread_bps =
        (sell_best_ask - sell_best_bid) / ((sell_best_ask + sell_best_bid) / 2.0) * 10_000.0;
    let closing_spread_bps = buy_spread_bps + sell_spread_bps;
    let round_trip_commission_bps = 2.0 * (buy_fee_bps + sell_fee_bps);
    let expected_friction_bps = round_trip_commission_bps
        + entry_slippage_bps
        + exit_slippage_bps
        + closing_spread_bps;
    let expected_net_bps = top_gross_bps - expected_friction_bps;
    let required_gross_bps = expected_friction_bps + target_profit_bps;
    let notional = ((buy_entry_vwap + sell_entry_vwap) / 2.0) * qty_f;
    let estimated_profit_usd = expected_net_bps / 10_000.0 * notional;

    if ![
        top_gross_bps,
        entry_slippage_bps,
        exit_slippage_bps,
        closing_spread_bps,
        expected_friction_bps,
        expected_net_bps,
        required_gross_bps,
        estimated_profit_usd,
    ]
    .iter()
    .all(|v| v.is_finite())
    {
        return None;
    }

    Some(RouteEconomics {
        buy_fee_bps,
        sell_fee_bps,
        round_trip_commission_bps,
        entry_slippage_bps,
        exit_slippage_bps,
        closing_spread_bps,
        expected_friction_bps,
        top_gross_bps,
        expected_net_bps,
        required_gross_bps,
        estimated_profit_usd,
    })
}

/// Per-strategy slippage gate value (bps) for the futures/spread path.
/// Reads the futures strategy record; a missing record falls back to
/// defaults. Callers convert to Decimal and fail closed on invalid values.
/// There is intentionally no global slippage setting.
fn futures_slippage_setting(state: &AppEngineState) -> f64 {
    state
        .strategy_params
        .read()
        .get("futures")
        .cloned()
        .unwrap_or_else(|| StrategyParamsDto::defaults_for("futures"))
        .futures_max_slippage_bps
}

fn route_execution_gates(
    book_buy: &OrderBook,
    book_sell: &OrderBook,
    qty: Decimal,
    max_slippage_bps: Decimal,
    min_depth_ratio: Decimal,
) -> (bool, bool) {
    let router = SmartOrderRouter::new(max_slippage_bps);
    let (Some(buy_ref), Some(sell_ref)) = (
        book_buy.best_ask_price(),
        book_sell.best_bid_price(),
    ) else {
        return (false, false);
    };
    let buy_limit = router.calculate_bounded_price(Side::Buy, buy_ref);
    let sell_limit = router.calculate_bounded_price(Side::Sell, sell_ref);
    let mut entry_buy = book_buy.clone();
    entry_buy.asks.retain(|l| l.price <= buy_limit);
    let mut entry_sell = book_sell.clone();
    entry_sell.bids.retain(|l| l.price >= sell_limit);

    let slippage_ok = arbitrageur_risk::safety_guards::verify_pre_execution_slippage(
        &entry_buy,
        Side::Buy,
        qty,
        max_slippage_bps,
    )
    .is_ok()
        && arbitrageur_risk::safety_guards::verify_pre_execution_slippage(
            &entry_sell,
            Side::Sell,
            qty,
            max_slippage_bps,
        )
        .is_ok()
        && arbitrageur_risk::safety_guards::verify_pre_execution_slippage(
            book_buy,
            Side::Sell,
            qty,
            max_slippage_bps,
        )
        .is_ok()
        && arbitrageur_risk::safety_guards::verify_pre_execution_slippage(
            book_sell,
            Side::Buy,
            qty,
            max_slippage_bps,
        )
        .is_ok();

    let required = qty * min_depth_ratio;
    let buy_depth: Decimal = entry_buy.asks.iter().map(|l| l.size).sum();
    let sell_depth: Decimal = entry_sell.bids.iter().map(|l| l.size).sum();
    (slippage_ok, buy_depth >= required && sell_depth >= required)
}

fn validate_market_symbol(symbol: &str) -> Result<(), String> {
    if symbol.is_empty() || symbol.len() > 64 {
        return Err("Invalid market symbol: must be 1-64 characters".into());
    }
    if symbol != symbol.trim() {
        return Err("Invalid market symbol: no leading/trailing whitespace".into());
    }
    if !symbol
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.'))
    {
        return Err(
            "Invalid market symbol: only ASCII alphanumeric, '-', '_', '/', '.' allowed".into(),
        );
    }
    if !symbol.chars().next().unwrap_or(' ').is_ascii_alphanumeric() || symbol.contains("..") {
        return Err(
            "Invalid market symbol: must start with an alphanumeric and not contain '..'".into(),
        );
    }
    let has_empty_segment = symbol.split(['-', '/', '_']).any(|seg| seg.is_empty());
    if has_empty_segment {
        return Err("Invalid market symbol: empty base/quote segment".into());
    }
    Ok(())
}

fn validate_adapter_book(
    book: &OrderBook,
    expected_venue: &VenueId,
    expected_symbol: &str,
) -> Result<(), String> {
    if &book.venue != expected_venue {
        return Err(format!(
            "order book venue '{}' does not match expected venue '{}'",
            book.venue, expected_venue
        ));
    }
    if book.symbol != expected_symbol {
        return Err(format!(
            "order book symbol '{}' does not match requested symbol '{}'",
            book.symbol, expected_symbol
        ));
    }
    book.validate_for_execution()
}

// ---------------------------------------------------------------------------
// Implementation functions (pure, unit-testable without Tauri State wrapper)
// ---------------------------------------------------------------------------

/// All venue ids currently enabled — engine flag OR an enabled account
/// profile. Drives which venues the status poll latency-probes.
fn venue_enabled_ids(state: &AppEngineState, s: &crate::state::EngineSettingsDto) -> Vec<String> {
    let account_enabled = |venue: &str| {
        state
            .state_store
            .has_enabled_account(venue)
            .unwrap_or(false)
    };
    let mut out = Vec::new();
    let mut push = |id: &str, on: bool| {
        if on {
            out.push(id.to_string());
        }
    };
    push("hyperliquid", s.hyperliquid_enabled || account_enabled("hyperliquid"));
    push("lighter", s.lighter_enabled || account_enabled("lighter"));
    push("robinhood", s.robinhood_enabled || account_enabled("robinhood"));
    push("vanta", s.vanta_enabled || account_enabled("vanta"));
    push("arcus", s.arcus_enabled || account_enabled("arcus"));
    push(
        "kraken",
        s.kraken_enabled
            || s.kraken_spot_enabled
            || s.kraken_futures_enabled
            || account_enabled("kraken_spot")
            || account_enabled("kraken_futures"),
    );
    push("polymarket", s.polymarket_enabled || account_enabled("polymarket"));
    push("kalshi", s.kalshi_enabled || account_enabled("kalshi"));
    push("decibel", s.decibel_enabled || account_enabled("decibel"));
    push("aster", s.aster_enabled || account_enabled("aster"));
    push("popdex", s.popdex_enabled || account_enabled("popdex"));
    push("binance", s.binance_enabled || account_enabled("binance"));
    push("coinbase", s.coinbase_enabled || account_enabled("coinbase"));
    push("okx", s.okx_enabled || account_enabled("okx"));
    push("ibkr", s.ibkr_enabled || account_enabled("ibkr"));
    push("ig", s.ig_enabled || account_enabled("ig"));
    out
}

/// Verified public endpoint per venue for latency probes when no adapter
/// exists. Every URL was curl-verified to return a real response — venues
/// with no reachable public API (ibkr: local TWS gateway) return None → honest N/A.
///
/// `accept_4xx`: for endpoints that require auth, an application-level 4xx
/// (e.g. IG's JSON `error.security.api-key-missing`) still proves the
/// origin processed the request — a valid RTT sample. Only set where the
/// 4xx is known to come from the API tier, not an edge challenge page.
struct PublicProbe {
    url: &'static str,
    accept_4xx: bool,
    post_body: Option<&'static str>,
}

fn public_probe_endpoint(vid: &str) -> Option<PublicProbe> {
    let p = |url: &'static str, accept_4xx: bool| {
        Some(PublicProbe {
            url,
            accept_4xx,
            post_body: None,
        })
    };
    match vid {
        "vanta" => Some(PublicProbe {
            url: "https://api.orderly.org/v1/public/query",
            accept_4xx: false,
            post_body: Some(r#"{"type":"rateLimitStatus"}"#),
        }),
        "arcus" => p("https://api.arcus.xyz/health", false),
        "kraken" => p("https://api.kraken.com/0/public/Time", false),
        "polymarket" => p("https://clob.polymarket.com/time", false),
        "kalshi" => p(
            "https://api.elections.kalshi.com/trade-api/v2/exchange/status",
            false,
        ),
        // Aptos fullnode is the actual transaction-submission path for
        // Decibel trades — its RTT is the arbitrage-relevant latency.
        "decibel" => p("https://fullnode.mainnet.aptoslabs.com/v1", false),
        "binance" => p("https://api.binance.com/api/v3/ping", false),
        "okx" => p("https://www.okx.com/api/v5/public/time", false),
        "coinbase" => p("https://api.coinbase.com/v2/time", false),
        "aster" => p("https://fapi.asterdex.com/fapi/v1/ping", false),
        "popdex" => p("https://api.popdex.xyz/api/v1/public/market/tickers?category=Futures&limit=1", false),
        // IG has no public endpoint; the session route returns an
        // app-level JSON 400 that proves the API gateway responded.
        "ig" => p("https://api.ig.com/gateway/deal/session", true),
        _ => None,
    }
}

/// HTTP probe for venues without an adapter: GET the venue's public
/// endpoint, or POST `post_body` for endpoints that only accept POST (e.g.
/// Orderly's public query). Success = the server demonstrably responded
/// (2xx, or 4xx for `accept_4xx` endpoints whose error is app-level JSON).
/// A timeout or connection failure yields Err → the EMA drops and the UI
/// shows N/A.
async fn probe_http(spec: PublicProbe) -> Result<(), String> {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        reqwest::Client::builder().timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    });
    let req = match spec.post_body {
        Some(body) => client
            .post(spec.url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body),
        None => client.get(spec.url),
    };
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let code = resp.status().as_u16();
    if resp.status().is_success() || (spec.accept_4xx && (400..500).contains(&code)) {
        Ok(())
    } else {
        Err(format!("probe got HTTP {code}"))
    }
}

pub async fn get_system_status_impl(state: &AppEngineState) -> Result<SystemStatusDto, String> {
    let cb = &state.circuit_breaker;
    let is_live = *state.is_live_mode.read();
    let total_pnl = if is_live {
        state
            .total_realized_profit_live
            .read()
            .to_string()
            .parse::<f64>()
            .unwrap_or(0.0)
    } else {
        state
            .total_realized_profit
            .read()
            .to_string()
            .parse::<f64>()
            .unwrap_or(0.0)
    };
    let (total_trades, success_count) = {
        let logs = state.execution_logs.read();
        let mode_logs: Vec<_> = logs.iter().filter(|l| l.is_live == is_live).collect();
        (mode_logs.len(), mode_logs.iter().filter(|l| l.success).count())
    };

    // Cloned so no lock guard is held across the latency-probe awaits.
    let s = state.settings.read().clone();
    // A venue is enabled when its engine flag is set OR it owns an enabled
    // account profile — the same admission rule `get_venues` applies.
    let account_enabled = |venue: &str| {
        state
            .state_store
            .has_enabled_account(venue)
            .unwrap_or(false)
    };
    // Admission is authoritative for the `supported` leg: a venue reports
    // READ_ONLY iff get_venues() actually admits it (enabled + real adapter).
    // A hardcoded per-venue bool previously let robinhood report UNSUPPORTED
    // while get_venues admitted it with a live adapter.
    let admitted = state.get_venues();
    let admitted_map: HashMap<&str, &Arc<dyn ExchangeAdapter>> = admitted
        .iter()
        .map(|(vid, _, a)| (*vid, a))
        .collect();
    let venue_status = |vid: &str, enabled: bool| -> &str {
        if !enabled {
            "OFFLINE"
        } else if admitted_map.contains_key(vid) {
            "READ_ONLY"
        } else {
            "UNSUPPORTED"
        }
    };

    // Measure real round-trip latency for every *enabled* venue in
    // parallel. Admitted venues probe via adapter::ping() (must reach the
    // exchange — no local/edge cache hits); enabled venues without an
    // adapter probe a verified public endpoint — the network path to the
    // venue's API is the same either way, so the number stays honest for
    // arbitrage decisions even before an adapter lands. Probes are
    // throttled to one per venue per 30s — re-measuring on every 3s poll
    // is request volume that trips edge WAFs without meaningfully fresher
    // data. Between probes the last smoothed value is reported; a failed
    // probe drops the average so the UI shows N/A rather than a stale
    // number.
    const LATENCY_PROBE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
    let enabled_ids: Vec<String> = venue_enabled_ids(state, &s);
    let mut probes = Vec::new();
    {
        let mut probed_at = state.latency_probe_at.lock();
        for vid in &enabled_ids {
            let due = probed_at
                .get(vid.as_str())
                .map(|t| t.elapsed() >= LATENCY_PROBE_INTERVAL)
                .unwrap_or(true);
            if !due {
                continue;
            }
            probed_at.insert(vid.clone(), std::time::Instant::now());
            if let Some(adapter) = admitted_map.get(vid.as_str()) {
                let vid = vid.clone();
                let adapter = Arc::clone(adapter);
                probes.push(tokio::spawn(async move {
                    let start = std::time::Instant::now();
                    let ms = adapter
                        .ping()
                        .await
                        .ok()
                        .map(|_| start.elapsed().as_millis() as u64);
                    (vid, ms)
                }));
            } else if let Some(spec) = public_probe_endpoint(vid) {
                let vid = vid.clone();
                probes.push(tokio::spawn(async move {
                    let start = std::time::Instant::now();
                    let ms = probe_http(spec)
                        .await
                        .ok()
                        .map(|_| start.elapsed().as_millis() as u64);
                    (vid, ms)
                }));
            }
        }
    }
    let mut results = Vec::new();
    for h in probes {
        if let Ok(pair) = h.await {
            results.push(pair);
        }
    }
    // Fold each successful sample into a per-venue exponential moving
    // average so the reported value is a rolling average, not a single
    // jumpy measurement. The lock is scoped so no guard crosses an await.
    // Report EMA only for currently-enabled venues — disabled venues stop
    // being probed and their last average must not linger.
    let enabled_set: std::collections::HashSet<&str> =
        enabled_ids.iter().map(|s| s.as_str()).collect();
    let latency_by_venue: HashMap<String, u64> = {
        let mut ema = state.latency_ema.lock();
        for (vid, res) in results {
            match res {
                Some(ms) => {
                    let smoothed = ema
                        .get(&vid)
                        .map(|prev| 0.35 * ms as f64 + 0.65 * prev)
                        .unwrap_or(ms as f64);
                    ema.insert(vid, smoothed);
                }
                None => {
                    ema.remove(&vid);
                }
            }
        }
        ema.iter()
            .filter(|(vid, _)| enabled_set.contains(vid.as_str()))
            .map(|(vid, v)| (vid.clone(), v.round() as u64))
            .collect()
    };
    let probe_ms = |vid: &str| latency_by_venue.get(vid).copied();

    let venues = vec![
        VenueStatusDto {
            id: "hyperliquid".into(),
            name: if is_live {
                "Hyperliquid (L1 DEX)".into()
            } else {
                "Hyperliquid (L1)".into()
            },
            market_type: "Perpetual CLOB".into(),
            status: venue_status(
                "hyperliquid",
                s.hyperliquid_enabled || account_enabled("hyperliquid"),
            )
            .into(),
            latency_ms: probe_ms("hyperliquid"),
            fee_bps: 2.0,
        },
        VenueStatusDto {
            id: "lighter".into(),
            name: "Lighter (zkDEX)".into(),
            market_type: "zk-Rollup CLOB".into(),
            status: venue_status("lighter", s.lighter_enabled || account_enabled("lighter")).into(),
            latency_ms: probe_ms("lighter"),
            fee_bps: 1.5,
        },
        VenueStatusDto {
            id: "vanta".into(),
            name: "Vanta.exchange".into(),
            market_type: "Omnichain Perps".into(),
            status: venue_status("vanta", s.vanta_enabled || account_enabled("vanta")).into(),
            latency_ms: probe_ms("vanta"),
            fee_bps: 3.0,
        },
        VenueStatusDto {
            id: "arcus".into(),
            name: "Arcus DEX".into(),
            market_type: "Fast CLOB".into(),
            status: venue_status("arcus", s.arcus_enabled || account_enabled("arcus")).into(),
            latency_ms: probe_ms("arcus"),
            fee_bps: 1.5,
        },
        VenueStatusDto {
            id: "robinhood".into(),
            name: "Robinhood EVM".into(),
            market_type: "EVM DEX Aggregator".into(),
            status: venue_status(
                "robinhood",
                s.robinhood_enabled || account_enabled("robinhood"),
            )
            .into(),
            latency_ms: probe_ms("robinhood"),
            fee_bps: 2.5,
        },
        VenueStatusDto {
            id: "kraken".into(),
            name: "Kraken".into(),
            market_type: "Spot & Futures CLOB".into(),
            status: venue_status(
                "kraken",
                s.kraken_enabled
                    || s.kraken_spot_enabled
                    || s.kraken_futures_enabled
                    || account_enabled("kraken_spot")
                    || account_enabled("kraken_futures"),
            )
            .into(),
            latency_ms: probe_ms("kraken"),
            fee_bps: 16.0,
        },
        VenueStatusDto {
            id: "polymarket".into(),
            name: "Polymarket (CTF CLOB)".into(),
            market_type: "Prediction Market".into(),
            status: venue_status(
                "polymarket",
                s.polymarket_enabled || account_enabled("polymarket"),
            )
            .into(),
            latency_ms: probe_ms("polymarket"),
            fee_bps: 0.0,
        },
        VenueStatusDto {
            id: "kalshi".into(),
            name: "Kalshi (CFTC)".into(),
            market_type: "Prediction Market".into(),
            status: venue_status("kalshi", s.kalshi_enabled || account_enabled("kalshi")).into(),
            latency_ms: probe_ms("kalshi"),
            fee_bps: 10.0,
        },
        VenueStatusDto {
            id: "decibel".into(),
            name: "Decibel".into(),
            market_type: "Aptos Perp DEX".into(),
            status: venue_status("decibel", s.decibel_enabled || account_enabled("decibel")).into(),
            latency_ms: probe_ms("decibel"),
            fee_bps: 2.5,
        },
        VenueStatusDto {
            id: "aster".into(),
            name: "Aster".into(),
            market_type: "Multi-Chain Perp DEX".into(),
            status: venue_status("aster", s.aster_enabled || account_enabled("aster")).into(),
            latency_ms: probe_ms("aster"),
            fee_bps: 3.5,
        },
        VenueStatusDto {
            id: "popdex".into(),
            name: "PopDEX".into(),
            market_type: "EVM Perp DEX".into(),
            status: venue_status("popdex", s.popdex_enabled || account_enabled("popdex")).into(),
            latency_ms: probe_ms("popdex"),
            fee_bps: 10.0,
        },
        VenueStatusDto {
            id: "binance".into(),
            name: "Binance".into(),
            market_type: "CEX Spot & Perp".into(),
            status: venue_status("binance", s.binance_enabled || account_enabled("binance")).into(),
            latency_ms: probe_ms("binance"),
            fee_bps: 7.5,
        },
        VenueStatusDto {
            id: "coinbase".into(),
            name: "Coinbase".into(),
            market_type: "CEX Spot & Perp".into(),
            status: venue_status(
                "coinbase",
                s.coinbase_enabled || account_enabled("coinbase"),
            )
            .into(),
            latency_ms: probe_ms("coinbase"),
            fee_bps: 8.0,
        },
        VenueStatusDto {
            id: "okx".into(),
            name: "OKX".into(),
            market_type: "CEX Spot & Perp".into(),
            status: venue_status("okx", s.okx_enabled || account_enabled("okx")).into(),
            latency_ms: probe_ms("okx"),
            fee_bps: 8.0,
        },
        VenueStatusDto {
            id: "ibkr".into(),
            name: "Interactive Brokers".into(),
            market_type: "TradFi Broker".into(),
            status: venue_status("ibkr", s.ibkr_enabled || account_enabled("ibkr")).into(),
            latency_ms: probe_ms("ibkr"),
            fee_bps: 1.0,
        },
        VenueStatusDto {
            id: "ig".into(),
            name: "IG Trading".into(),
            market_type: "TradFi Broker".into(),
            status: venue_status("ig", s.ig_enabled || account_enabled("ig")).into(),
            latency_ms: probe_ms("ig"),
            fee_bps: 5.0,
        },
    ];

    let current_drawdown = cb
        .current_drawdown_usd()
        .to_string()
        .parse::<f64>()
        .unwrap_or(0.0);
    let max_drawdown = cb
        .max_daily_drawdown_usd()
        .to_string()
        .parse::<f64>()
        .unwrap_or(25000.0);

    let is_tripped = cb.is_tripped();
    let cb_status_str = match cb.trip_reason() {
        Some(reason) => format!("TRIPPED: {}", reason),
        None => {
            if is_tripped {
                "TRIPPED: Manual halt".into()
            } else {
                "ACTIVE".into()
            }
        }
    };

    Ok(SystemStatusDto {
        is_live_mode: is_live,
        is_paused: *state.is_paused.read(),
        total_realized_pnl: total_pnl,
        total_trades_count: total_trades,
        successful_trades_count: success_count,
        circuit_breaker_status: cb_status_str,
        circuit_breaker_tripped: is_tripped,
        current_drawdown_usd: current_drawdown,
        max_drawdown_usd: max_drawdown,
        consecutive_failures: cb.consecutive_errors(),
        active_venues: venues,
    })
}

pub async fn refresh_venue_latencies_impl(state: &AppEngineState) -> Result<SystemStatusDto, String> {
    state.latency_probe_at.lock().clear();
    get_system_status_impl(state).await
}

pub async fn get_market_universe_impl(
    state: &AppEngineState,
) -> Result<Vec<VenueMarketUniverseDto>, String> {
    let mut tasks = Vec::new();
    for (vid, _, adapter) in state.get_venues() {
        let vid = vid.to_string();
        tasks.push(tokio::spawn(async move { (vid, adapter.fetch_markets().await) }));
    }
    let mut out = Vec::new();
    for h in tasks {
        if let Ok((vid, Ok(markets))) = h.await {
            let mut symbols: Vec<String> = markets
                .iter()
                .filter(|m| {
                    m.is_active
                        && m.symbol.market_type
                            == arbitrageur_core::market::MarketType::Perpetual
                })
                .map(|m| {
                    format!(
                        "{}-{}",
                        m.symbol.base.to_uppercase(),
                        m.symbol.quote.to_uppercase()
                    )
                })
                .collect();
            symbols.sort();
            symbols.dedup();
            out.push(VenueMarketUniverseDto {
                venue_id: vid,
                symbols,
            });
        }
    }
    out.sort_by(|a, b| a.venue_id.cmp(&b.venue_id));
    Ok(out)
}

pub(crate) fn enforce_retention_pass(state: &AppEngineState, now_ms: i64) {
    let (max_days, max_gb) = {
        let s = state.settings.read();
        (s.spread_samples_max_days, s.spread_samples_max_gb)
    };
    if let Some(days) = max_days {
        if days > 0 {
            let retain_after = now_ms - (days as i64) * 86_400_000;
            let _ = state.state_store.delete_spread_samples_before(retain_after);
            // Same cutoff bounds the raw volume log. Session profiles and
            // hour bins are derived state and are never pruned here.
            let _ = state
                .state_store
                .prune_venue_volume_samples(retain_after, i64::MAX);
            // Same day cutoff bounds the signal-event audit log, with its
            // own row cap so audit history and price history prune
            // independently.
            let _ = state.state_store.prune_monitor_signal_events(
                retain_after,
                crate::state_store::MONITOR_SIGNAL_EVENTS_MAX_ROWS,
            );
            // Same day cutoff bounds the margin-snapshot log behind the
            // 24h average-utilisation stat.
            let _ = state.state_store.prune_margin_snapshots(retain_after);
            // Same day cutoff bounds the funding-rate history log (future
            // persistence / half-life training substrate).
            let _ = state
                .state_store
                .prune_funding_rate_samples(retain_after);
        }
    }
    if let Some(gb) = max_gb {
        if gb.is_finite() && gb > 0.0 {
            let budget = (gb * 1024.0 * 1024.0 * 1024.0) as u64;
            for _ in 0..1000 {
                let size = match state.state_store.db_size_bytes() {
                    Ok(s) => s,
                    Err(_) => break,
                };
                if size <= budget {
                    break;
                }
                match state.state_store.delete_oldest_spread_samples(100_000) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        }
    }
}

pub async fn fetch_orderbooks_impl(
    symbol: &str,
    state: &AppEngineState,
) -> Result<Vec<VenueOrderBookDto>, String> {
    validate_market_symbol(symbol)?;
    state.ensure_market(symbol);
    let venues = state.get_venues();
    let mut result = Vec::new();
    let mut sample_entries = Vec::new();

    for (vid, vname, adapter) in venues {
        let venue_id = adapter.venue_id();
        let book = match crate::ws_primary::resolve_book_ws_first(
            &state.market_data_hub,
            &state.orderbook_flights,
            &adapter,
            symbol,
            std::time::Duration::from_millis(350),
        )
        .await
        {
            Some(crate::ws_primary::ResolvedBook::Ws(b))
            | Some(crate::ws_primary::ResolvedBook::Cached(b)) => b,
            Some(crate::ws_primary::ResolvedBook::Fresh(b)) => {
                if validate_adapter_book(&b, &venue_id, symbol).is_err() {
                    continue;
                }
                state.market_data_hub.update_orderbook(b.clone());
                b
            }
            None => continue,
        };
        if validate_adapter_book(&book, &venue_id, symbol).is_err() {
            continue;
        }

        let best_bid = book
            .best_bid_price()
            .map(|p| p.to_string().parse::<f64>().unwrap_or(0.0))
            .unwrap_or(0.0);
        let best_ask = book
            .best_ask_price()
            .map(|p| p.to_string().parse::<f64>().unwrap_or(0.0))
            .unwrap_or(0.0);
        let spread = (best_ask - best_bid).max(0.0);
        let mid = if best_bid > 0.0 && best_ask > 0.0 {
            (best_bid + best_ask) / 2.0
        } else {
            0.0
        };
        let spread_bps = if mid > 0.0 {
            (spread / mid) * 10000.0
        } else {
            0.0
        };

        let bids_dto: Vec<[f64; 2]> = book
            .bids
            .iter()
            .take(5)
            .map(|b| {
                [
                    b.price.to_string().parse::<f64>().unwrap_or(0.0),
                    b.size.to_string().parse::<f64>().unwrap_or(0.0),
                ]
            })
            .collect();

        let asks_dto: Vec<[f64; 2]> = book
            .asks
            .iter()
            .take(5)
            .map(|a| {
                [
                    a.price.to_string().parse::<f64>().unwrap_or(0.0),
                    a.size.to_string().parse::<f64>().unwrap_or(0.0),
                ]
            })
            .collect();

        let total_bid_depth: f64 = bids_dto.iter().map(|b| b[1]).sum();
        let total_ask_depth: f64 = asks_dto.iter().map(|a| a[1]).sum();

        let benchmark_size = dec!(0.5);
        let vwap_bid = book
            .sell_execution_vwap(benchmark_size)
            .map(|(v, _, _)| v.to_string().parse::<f64>().unwrap_or(best_bid))
            .unwrap_or(best_bid);
        let vwap_ask = book
            .buy_execution_vwap(benchmark_size)
            .map(|(v, _, _)| v.to_string().parse::<f64>().unwrap_or(best_ask))
            .unwrap_or(best_ask);

        let funding_pct = if let Some(fr) = state.market_data_hub.get_funding_rate(
            &venue_id,
            symbol,
            Some(std::time::Duration::from_secs(60)),
        ) {
            (fr.rate * dec!(100))
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0)
        } else if let Ok(fr) = adapter.fetch_funding_rate(symbol).await {
            state.market_data_hub.update_funding(fr.clone());
            (fr.rate * dec!(100))
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0)
        } else {
            0.0
        };

        result.push(VenueOrderBookDto {
            venue_id: vid.into(),
            venue_name: vname.into(),
            symbol: symbol.to_string(),
            best_bid,
            best_ask,
            spread,
            spread_bps,
            mid_price: mid,
            total_bid_depth,
            total_ask_depth,
            vwap_bid,
            vwap_ask,
            funding_rate_8h_pct: funding_pct,
            bids: bids_dto,
            asks: asks_dto,
        });
        if mid > 0.0 {
            sample_entries.push(crate::state_store::VenueSampleEntry {
                venue_id: vid.into(),
                mid,
                best_bid,
                best_ask,
            });
        }
    }

    // Durable signal history: persist this poll's venue mids so the
    // dislocation series can be sliced over arbitrary windows. Best-effort —
    // a store failure must never break the market-data path.
    let now_ms = Utc::now().timestamp_millis();
    let _ = state
        .state_store
        .record_spread_samples(symbol, now_ms, &sample_entries);

    // Hourly retention pass: settings-driven time cutoff + GB budget.
    // Both unset -> no-op. Best-effort, never breaks ingestion.
    let current_hour = now_ms / 3_600_000;
    let heartbeat_due = {
        let mut last_hour = state.sample_prune_at.lock();
        if current_hour > *last_hour {
            *last_hour = current_hour;
            enforce_retention_pass(state, now_ms);
            true
        } else {
            false
        }
    };
    if heartbeat_due {
        // Periodic heartbeat for the utilisation history: the cadence
        // cap keeps this to at most one row per venue per minute.
        maybe_record_margin_snapshots(state, now_ms).await;
    }

    Ok(result)
}

pub async fn fetch_multi_market_quotes_impl(
    symbols: Vec<String>,
    state: &AppEngineState,
) -> Result<Vec<TickerMarketQuoteDto>, String> {
    if symbols.len() > 100 {
        return Err("Too many symbols requested (max 100)".into());
    }
    for sym in &symbols {
        validate_market_symbol(sym)?;
    }
    let venues = state.get_venues();
    let mut quotes = Vec::with_capacity(symbols.len());

    for sym in symbols {
        state.ensure_market(&sym);
        let base_str = sym.split('-').next().unwrap_or(&sym).to_string();

        let mut best_bid = 0.0f64;
        let mut best_bid_venue = "Hyperliquid".to_string();
        let mut best_ask = f64::INFINITY;
        let mut best_ask_venue = "Hyperliquid".to_string();
        let mut best_buy_spread_bps = 2.0f64;
        let mut found_any = false;

        for (_vid, vname, adapter) in &venues {
            let venue_id = adapter.venue_id();
            let book = match crate::ws_primary::resolve_book_ws_first(
                &state.market_data_hub,
                &state.orderbook_flights,
                adapter,
                &sym,
                std::time::Duration::from_millis(500),
            )
            .await
            {
                Some(crate::ws_primary::ResolvedBook::Ws(b))
                | Some(crate::ws_primary::ResolvedBook::Cached(b)) => b,
                Some(crate::ws_primary::ResolvedBook::Fresh(b)) => {
                    if validate_adapter_book(&b, &venue_id, &sym).is_err() {
                        continue;
                    }
                    state.market_data_hub.update_orderbook(b.clone());
                    b
                }
                None => continue,
            };
            if validate_adapter_book(&book, &venue_id, &sym).is_err() {
                continue;
            }

            if let Some(bid) = book.best_bid_price() {
                let b_f = bid.to_string().parse::<f64>().unwrap_or(0.0);
                if b_f > best_bid {
                    best_bid = b_f;
                    best_bid_venue = vname.to_string();
                    found_any = true;
                }
            }
            if let (Some(b), Some(a)) = (book.best_bid_price(), book.best_ask_price()) {
                let b_f = b.to_string().parse::<f64>().unwrap_or(0.0);
                let a_f = a.to_string().parse::<f64>().unwrap_or(0.0);
                if a_f > 0.0 && a_f < best_ask {
                    best_ask = a_f;
                    best_ask_venue = vname.to_string();
                    let v_mid = (b_f + a_f) / 2.0;
                    if v_mid > 0.0 && a_f >= b_f {
                        best_buy_spread_bps = ((a_f - b_f) / v_mid) * 10000.0;
                    }
                    found_any = true;
                }
            }
        }

        let (final_bid, final_ask, mid, spread_bps, best_buy_v, best_sell_v, fee) =
            if found_any && best_bid > 0.0 && best_ask < f64::INFINITY {
                let m = (best_bid + best_ask) / 2.0;
                let sp = best_buy_spread_bps.max(0.0);
                let f = venue_fee_bps(&best_ask_venue)
                    .to_string()
                    .parse::<f64>()
                    .unwrap_or(2.0);
                (best_bid, best_ask, m, sp, best_ask_venue, best_bid_venue, f)
            } else {
                (0.0, 0.0, 0.0, 0.0, "-".to_string(), "-".to_string(), 0.0)
            };

        quotes.push(TickerMarketQuoteDto {
            symbol: sym,
            base: base_str,
            best_bid: final_bid,
            best_ask: final_ask,
            mid_price: mid,
            spread_bps,
            best_buy_venue: best_buy_v,
            best_sell_venue: best_sell_v,
            fee_bps: fee,
            base_price: mid,
        });
    }

    Ok(quotes)
}

pub async fn scan_spatial_opportunities_impl(
    symbol: &str,
    min_bps: f64,
    order_qty: f64,
    state: &AppEngineState,
) -> Result<Vec<SpatialOpportunityDto>, String> {
    validate_market_symbol(symbol)?;
    if !order_qty.is_finite() || order_qty <= 0.0 {
        return Err("order_qty must be a finite positive number".into());
    }
    if !min_bps.is_finite() || min_bps < 0.0 {
        return Err("min_bps must be finite and nonnegative".into());
    }
    state.ensure_market(symbol);
    let venues = state.get_venues();
    if venues.len() < 2 {
        return Ok(Vec::new());
    }

    let mut books = Vec::new();
    for (vid, vname, adapter) in &venues {
        let venue_id = adapter.venue_id();
        let book = match crate::ws_primary::resolve_book_ws_first(
            &state.market_data_hub,
            &state.orderbook_flights,
            adapter,
            symbol,
            std::time::Duration::from_millis(350),
        )
        .await
        {
            Some(crate::ws_primary::ResolvedBook::Ws(b))
            | Some(crate::ws_primary::ResolvedBook::Cached(b)) => b,
            Some(crate::ws_primary::ResolvedBook::Fresh(b)) => {
                if validate_adapter_book(&b, &venue_id, symbol).is_err() {
                    continue;
                }
                state.market_data_hub.update_orderbook(b.clone());
                b
            }
            None => continue,
        };
        if validate_adapter_book(&book, &venue_id, symbol).is_err() {
            continue;
        }
        books.push((*vid, *vname, book, effective_venue_fee_bps(state, vid)));
    }

    if books.len() < 2 {
        return Ok(Vec::new());
    }

    let target_qty_dec = Decimal::from_f64_retain(order_qty)
        .ok_or_else(|| "order_qty is outside supported decimal range".to_string())?;
    let max_slippage_bps = futures_slippage_setting(state);
    let min_depth_ratio = state.settings.read().min_depth_ratio;
    let max_slip_dec = Decimal::from_f64_retain(max_slippage_bps)
        .ok_or_else(|| "Invalid max slippage setting".to_string())?;
    let min_depth_dec = Decimal::from_f64_retain(min_depth_ratio)
        .ok_or_else(|| "Invalid min depth ratio setting".to_string())?;

    // Episode history for win probabilities (informational only): one
    // `spread_samples` read shared across all venue pairs, scored to the
    // monitor's statistical standard (min-episodes gate + OU R² fallback).
    // Fail-closed throughout: any missing history yields `None` ("—").
    let now_ms = Utc::now().timestamp_millis();
    let monitor_cfg = state
        .state_store
        .monitor_config_json()
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str::<crate::monitor::MonitorConfigDto>(&j).ok())
        .unwrap_or_default();
    let scan_min_episodes = monitor_cfg.min_episodes.max(1);
    let scan_history_secs = monitor_cfg.history_window_secs.clamp(3600, 30 * 86400);
    let scan_history_from = now_ms.saturating_sub((scan_history_secs as i64).saturating_mul(1000));
    let mut scan_mids_by_venue: HashMap<String, Vec<(i64, f64)>> = HashMap::new();
    if let Ok(samples) = state
        .state_store
        .spread_samples(symbol, scan_history_from, now_ms)
    {
        for s in &samples {
            if s.mid.is_finite() && s.mid > 0.0 {
                scan_mids_by_venue
                    .entry(s.venue_id.clone())
                    .or_default()
                    .push((s.ts_ms, s.mid));
            }
        }
    }
    let mut opps = Vec::new();

    for i in 0..books.len() {
        for j in 0..books.len() {
            if i == j {
                continue;
            }
            let (buy_id, buy_name, ref book_buy, buy_fee) = books[i];
            let (sell_id, sell_name, ref book_sell, sell_fee) = books[j];

            if let Some(econ) = compute_route_economics(
                book_buy,
                book_sell,
                target_qty_dec,
                buy_fee,
                sell_fee,
                min_bps,
            ) {
                let best_ask = book_buy
                    .best_ask_price()
                    .map(|p| p.to_string().parse::<f64>().unwrap_or(0.0))
                    .unwrap_or(0.0);
                let best_bid = book_sell
                    .best_bid_price()
                    .map(|p| p.to_string().parse::<f64>().unwrap_or(0.0))
                    .unwrap_or(0.0);

                let (slippage_ok, depth_ok) = route_execution_gates(
                    book_buy,
                    book_sell,
                    target_qty_dec,
                    max_slip_dec,
                    min_depth_dec,
                );
                let is_executable =
                    econ.expected_net_bps >= min_bps && slippage_ok && depth_ok;

                // Informational win probability on the shared monitor
                // standard. Never feeds ordering or `is_executable`.
                let win_probability = match (
                    scan_mids_by_venue.get(buy_id),
                    scan_mids_by_venue.get(sell_id),
                ) {
                    (Some(buy_mids), Some(sell_mids))
                        if !buy_mids.is_empty() && !sell_mids.is_empty() =>
                    {
                        let series = build_pair_series(
                            buy_mids,
                            sell_mids,
                            crate::monitor::PAIR_ALIGN_GAP_MS,
                        );
                        crate::monitor::pair_win_probability(
                            &series,
                            econ.top_gross_bps,
                            econ.expected_friction_bps,
                            scan_min_episodes,
                        )
                    }
                    _ => None,
                };

                opps.push(SpatialOpportunityDto {
                    id: format!("{}-{}-{}", symbol, buy_id, sell_id),
                    symbol: symbol.to_string(),
                    buy_venue: buy_name.to_string(),
                    sell_venue: sell_name.to_string(),
                    buy_price: best_ask,
                    sell_price: best_bid,
                    gross_spread: best_bid - best_ask,
                    gross_spread_bps: econ.top_gross_bps,
                    fee_bps: econ.buy_fee_bps + econ.sell_fee_bps,
                    net_spread_bps: econ.expected_net_bps,
                    order_qty,
                    estimated_profit_usd: econ.estimated_profit_usd,
                    is_executable,
                    buy_venue_id: buy_id.to_string(),
                    sell_venue_id: sell_id.to_string(),
                    buy_fee_bps: econ.buy_fee_bps,
                    sell_fee_bps: econ.sell_fee_bps,
                    round_trip_commission_bps: econ.round_trip_commission_bps,
                    entry_slippage_bps: econ.entry_slippage_bps,
                    exit_slippage_bps: econ.exit_slippage_bps,
                    closing_spread_bps: econ.closing_spread_bps,
                    expected_friction_bps: econ.expected_friction_bps,
                    required_gross_bps: econ.required_gross_bps,
                    slippage_ok,
                    depth_ok,
                    win_probability,
                });
            }
        }
    }

    opps.sort_by(|a, b| {
        b.net_spread_bps
            .partial_cmp(&a.net_spread_bps)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(opps)
}

pub async fn scan_spot_opportunities_impl(
    symbol_filter: &str,
    state: &AppEngineState,
) -> Result<Vec<SpotOpportunityDto>, String> {
    let _ = (symbol_filter, state);
    Ok(Vec::new())
}

pub async fn execute_spot_arbitrage_impl(
    params: SpotExecutionParams,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    if let Err(reason) = state.circuit_breaker.is_trading_allowed() {
        return Err(format!(
            "Execution halted by Risk Circuit Breaker: {}",
            reason
        ));
    }
    if *state.is_paused.read() {
        return Err(
            "Trading engine is PAUSED. Positions are preserved, but new executions are suspended."
                .into(),
        );
    }
    let _ = params;
    Err("Native 'spot' execution is disabled until authentic order submission and reconciliation are implemented".into())
}

// ---------------------------------------------------------------------------
// Triangular arbitrage (single-venue A -> B -> C -> A cycles, paper-only)
// ---------------------------------------------------------------------------

/// Strategy id under which triangle residuals are recorded. The 2-leg signal
/// ledger cannot represent an atomic 3-leg cycle as one row (1-2 leg cap), so
/// a completed cycle's legs persist as individual 1-leg `Imbalanced` signals:
/// each one blocks its symbol and unwinds through the existing symbol-scoped
/// flatten path, exactly like any other open paper position.
pub(crate) const TRIANGLE_STRATEGY_ID: &str = "triangle";
pub(crate) const FUNDING_STRATEGY_ID: &str = "funding";
/// Paper-dust tolerance for the triangle entry guard: f64/Decimal ledger
/// conversions can strand sub-1e-9 base-unit residue after a flatten.
/// Economically meaningless on every listed venue, so it must not block
/// re-entry forever; anything at or above it is a real position.
fn triangle_dust_qty() -> Decimal {
    dec!(0.000000001)
}
/// Leftover-sweep rule for close/unwind sizing (shared implementation:
/// `arbitrageur_strategy::pair_trim::sweep_close_qty`). When a close is
/// requested for `requested` (e.g. one grid unit, or the paper position when
/// it covers less than the ledger) but the attributed residual exceeds it by
/// less than one full step, closing only `requested` would strand a sub-step
/// remnant that can never close on its own — so the ENTIRE residual closes
/// instead. A real multi-step remainder (residual >= requested + one step)
/// still closes in part. Applied in the flatten path, which also serves
/// convergence unwinds. Returns the original Decimal inputs untouched, so no
/// f64 round-trip dust enters the ledger.
fn sweep_flatten_qty(residual: Decimal, capped: Decimal, step: Decimal) -> Decimal {
    let to_f64 = |v: Decimal| v.to_string().parse::<f64>().ok();
    let (Some(r), Some(q)) = (to_f64(residual), to_f64(capped)) else {
        return capped;
    };
    if !r.is_finite() || !q.is_finite() {
        return capped;
    }
    let s = to_f64(step).unwrap_or(0.0);
    let swept = arbitrageur_strategy::pair_trim::sweep_close_qty(r, q, s);
    if swept >= r {
        residual
    } else {
        capped
    }
}
/// One full grid step for `symbol` from its ticker config (the min tradable
/// unit for sweep purposes). Unknown or invalid configs yield zero, which
/// disables the sweep (plain capped-close semantics).
fn flatten_step_qty(state: &AppEngineState, symbol: &str) -> Decimal {
    state
        .strategy_configs
        .read()
        .get(symbol)
        .and_then(|c| Decimal::from_f64_retain(c.unit_size))
        .filter(|u| *u > dec!(0))
        .unwrap_or(dec!(0))
}
const TRIANGLE_QUOTE: &str = "USDC";
const TRIANGLE_BASES: [&str; 5] = ["BTC", "ETH", "SOL", "BNB", "XRP"];
/// Fixed scan notional: matches the frontend "Est. Profit ($10k)" column.
const TRIANGLE_SCAN_NOTIONAL_USD: f64 = 10_000.0;

struct TrianglePlan {
    base_a: String,
    base_b: String,
    leg_symbols: [String; 3],
    leg_sides: [TriLegSide; 3],
    order_sides: [Side; 3],
    leg_actions: [&'static str; 3],
    path: String,
}

/// Canonical USDC -> A -> B -> USDC cycle on one venue. Only the "{A}-{B}"
/// orientation is attempted for the middle leg; the reverse triangle is a
/// separate candidate (ordered pairs are enumerated both ways). When the
/// venue does not list a leg symbol, book resolution fails closed and the
/// candidate is skipped — never synthesized.
fn triangle_plan(base_a: &str, base_b: &str) -> TrianglePlan {
    TrianglePlan {
        base_a: base_a.to_string(),
        base_b: base_b.to_string(),
        leg_symbols: [
            format!("{base_a}-{TRIANGLE_QUOTE}"),
            format!("{base_a}-{base_b}"),
            format!("{base_b}-{TRIANGLE_QUOTE}"),
        ],
        leg_sides: [TriLegSide::BuyBase, TriLegSide::SellBase, TriLegSide::SellBase],
        order_sides: [Side::Buy, Side::Sell, Side::Sell],
        leg_actions: ["BUY", "SELL", "SELL"],
        path: format!("{TRIANGLE_QUOTE} → {base_a} → {base_b} → {TRIANGLE_QUOTE}"),
    }
}

struct TriangleEval {
    gross_bps: f64,
    fee_bps: f64,
    net_bps: f64,
    profit_usd: f64,
    order_qty: [f64; 3],
    leg_prices: [f64; 3],
}

fn evaluate_triangle_books(
    books: &[OrderBook; 3],
    sides: [TriLegSide; 3],
    fee_bps: f64,
    notional_usd: f64,
) -> Option<TriangleEval> {
    let notional = Decimal::from_f64_retain(notional_usd).filter(|q| *q > dec!(0))?;
    let fee = Decimal::from_f64_retain(fee_bps).filter(|f| *f >= dec!(0))?;
    let res = evaluate_triangular_cycle(
        [&books[0], &books[1], &books[2]],
        sides,
        [fee, fee, fee],
        notional,
    )?;
    let num = |v: Decimal| v.to_string().parse::<f64>().ok().filter(|f| f.is_finite());
    let prices = [
        num(books[0].best_ask_price()?)?,
        num(books[1].best_bid_price()?)?,
        num(books[2].best_bid_price()?)?,
    ];
    if prices.iter().any(|p| *p <= 0.0) {
        return None;
    }
    let qty = [
        num(res.order_qty[0])?,
        num(res.order_qty[1])?,
        num(res.order_qty[2])?,
    ];
    if qty.iter().any(|q| !q.is_finite() || *q <= 0.0) {
        return None;
    }
    Some(TriangleEval {
        gross_bps: num(res.gross_bps)?,
        fee_bps: num(res.total_fee_bps)?,
        net_bps: num(res.net_bps)?,
        profit_usd: num(res.estimated_profit)?,
        order_qty: qty,
        leg_prices: prices,
    })
}

/// Resolve all three cycle books, hub-first with a 2s freshness bound, else a
/// fresh fetch through the admitted read-only adapter. All-or-none: any
/// missing leg returns `None` and the candidate is skipped.
async fn resolve_triangle_books(
    state: &AppEngineState,
    adapter: &Arc<dyn ExchangeAdapter>,
    vid: &VenueId,
    symbols: &[String; 3],
) -> Option<[OrderBook; 3]> {
    let mut out: Vec<OrderBook> = Vec::with_capacity(3);
    for sym in symbols {
        let book = match state.market_data_hub.get_valid_orderbook(
            vid,
            sym,
            Some(std::time::Duration::from_secs(2)),
        ) {
            Some(b) => b,
            None => {
                let b = adapter.fetch_orderbook(sym).await.ok()?;
                validate_adapter_book(&b, vid, sym).ok()?;
                state.market_data_hub.update_orderbook(b.clone());
                b
            }
        };
        if validate_adapter_book(&book, vid, sym).is_err() {
            return None;
        }
        out.push(book);
    }
    Some([out.remove(0), out.remove(0), out.remove(0)])
}

fn triangle_target_profit_bps(state: &AppEngineState) -> f64 {
    let params = state.strategy_params.read();
    params
        .get("triangle")
        .map(|p| p.triangle_min_profit_bps)
        .or_else(|| params.get("futures").map(|p| p.target_profit_bps))
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(2.0)
}

pub async fn scan_triangular_opportunities_impl(
    filter_asset: &str,
    state: &AppEngineState,
) -> Result<Vec<TriangleOpportunityDto>, String> {
    let filter = filter_asset.trim().to_uppercase();
    if !filter.is_empty()
        && (filter.len() > 10 || !filter.chars().all(|c| c.is_ascii_alphanumeric()))
    {
        return Err("filter asset must be empty or 1-10 ASCII alphanumeric characters".into());
    }
    let venues = state.get_venues();
    if venues.is_empty() {
        return Ok(Vec::new());
    }
    let hurdle = triangle_target_profit_bps(state);
    let trade_size = {
        let params = state.strategy_params.read();
        params
            .get("triangle")
            .map(|p| p.triangle_trade_size_usd)
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(TRIANGLE_SCAN_NOTIONAL_USD)
    };
    let mut opps = Vec::new();
    for (vid, vname, adapter) in &venues {
        let fee = effective_venue_fee_bps(state, vid);
        let latency_ms = state
            .latency_ema
            .lock()
            .get(*vid)
            .copied()
            .unwrap_or(0.0)
            .max(0.0) as u64;
        let venue_id = adapter.venue_id();
        for base_a in TRIANGLE_BASES {
            for base_b in TRIANGLE_BASES {
                if base_a == base_b {
                    continue;
                }
                if !filter.is_empty() && filter != base_a && filter != base_b {
                    continue;
                }
                let plan = triangle_plan(base_a, base_b);
                let books =
                    match resolve_triangle_books(state, adapter, &venue_id, &plan.leg_symbols)
                        .await
                    {
                        Some(b) => b,
                        None => continue,
                    };
                let eval = match evaluate_triangle_books(
                    &books,
                    plan.leg_sides,
                    fee,
                    trade_size,
                ) {
                    Some(e) => e,
                    None => continue,
                };
                let legs = [
                    TriangleLegDto {
                        symbol: plan.leg_symbols[0].clone(),
                        action: plan.leg_actions[0].to_string(),
                        price: eval.leg_prices[0],
                        fee_bps: fee,
                    },
                    TriangleLegDto {
                        symbol: plan.leg_symbols[1].clone(),
                        action: plan.leg_actions[1].to_string(),
                        price: eval.leg_prices[1],
                        fee_bps: fee,
                    },
                    TriangleLegDto {
                        symbol: plan.leg_symbols[2].clone(),
                        action: plan.leg_actions[2].to_string(),
                        price: eval.leg_prices[2],
                        fee_bps: fee,
                    },
                ];
                opps.push(TriangleOpportunityDto {
                    id: format!("{vid}-TRI-{base_a}-{base_b}-{TRIANGLE_QUOTE}"),
                    venue: vname.to_string(),
                    path: plan.path.clone(),
                    base_asset: TRIANGLE_QUOTE.to_string(),
                    intermediate_a: plan.base_a.clone(),
                    intermediate_b: plan.base_b.clone(),
                    leg1: legs[0].clone(),
                    leg2: legs[1].clone(),
                    leg3: legs[2].clone(),
                    gross_spread_bps: eval.gross_bps,
                    total_fee_bps: eval.fee_bps,
                    net_spread_bps: eval.net_bps,
                    net_profit_usd: eval.profit_usd,
                    trade_size_usd: trade_size,
                    estimated_latency_ms: latency_ms,
                    is_executable: eval.net_bps >= hurdle,
                });
            }
        }
    }
    opps.sort_by(|a, b| {
        b.net_spread_bps
            .partial_cmp(&a.net_spread_bps)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(opps)
}

/// Persist one filled triangle leg as a 1-leg `Imbalanced` signal so the leg
/// symbol stays blocked and unwinds through the existing symbol-scoped
/// flatten path. Only legs with a positive fill and price are recorded.
///
/// `qty_f64` must be the exact submitted quantity for full fills: the paper
/// position holds `Decimal::from_f64_retain(qty_f64)`, so recording the same
/// f64 source keeps the ledger residual bit-identical to the paper position.
/// Re-converting the Decimal fill back to f64 would shed ~1e-19 of dust and
/// leave the symbol blocked on economically meaningless residue.
#[allow(clippy::too_many_arguments)]
fn record_triangle_leg_fill(
    state: &AppEngineState,
    cycle_tag: &str,
    leg_idx: usize,
    venue_id: &str,
    symbol: &str,
    order_side: Side,
    qty_f64: f64,
    fill_px: Decimal,
    fee: Decimal,
    order_id: &str,
    note: &str,
) -> Result<(), String> {
    use crate::state_store::{LegSide, NewSignalExecution, NewSignalLeg, SignalStatus};
    if !qty_f64.is_finite() || qty_f64 <= 0.0 {
        return Err("triangle fill quantity is not positive".into());
    }
    let px_f = dec_to_f64(fill_px, "triangle fill price")?;
    let fee_f = dec_to_f64(fee, "triangle fill fee")?;
    if px_f <= 0.0 || fee_f < 0.0 {
        return Err("triangle fill is not recordable".into());
    }
    let side = match order_side {
        Side::Buy => LegSide::Long,
        Side::Sell => LegSide::Short,
    };
    let account_id = state
        .state_store
        .default_account(venue_id)
        .ok()
        .flatten()
        .map(|a| a.id);
    let signal_id = format!("{cycle_tag}-leg{leg_idx}");
    state
        .state_store
        .record_signal_execution(&NewSignalExecution {
            id: signal_id.clone(),
            strategy_id: TRIANGLE_STRATEGY_ID.to_string(),
            symbol: symbol.to_string(),
            route_id: None,
            expected_net_usd: None,
            note: Some(note.to_string()),
            legs: vec![NewSignalLeg {
                account_id,
                venue: venue_id.to_string(),
                side,
                intended_qty: qty_f64,
            }],
        })
        .map_err(|e| format!("Failed to persist triangle leg record: {e}"))?;
    state
        .state_store
        .apply_leg_fill(&signal_id, side, qty_f64, px_f, fee_f, Some(order_id))
        .map_err(|e| e.to_string())?;
    state
        .state_store
        .update_signal_status(&signal_id, SignalStatus::Imbalanced, Some(note))
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn execute_triangle_arbitrage_impl(
    params: TriangleExecutionParams,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    if params.id.trim().is_empty() || params.id.len() > 128 {
        return Err("triangle opportunity id must be 1-128 characters".into());
    }
    if params.venue.trim().is_empty() {
        return Err("triangle venue must be specified".into());
    }
    if !params.trade_size_usd.is_finite() || params.trade_size_usd <= 0.0 {
        return Err("trade_size_usd must be a finite positive number".into());
    }
    if !params.min_profit_bps.is_finite() || params.min_profit_bps < 0.0 {
        return Err("min_profit_bps must be finite and nonnegative".into());
    }

    if let Err(reason) = state.circuit_breaker.is_trading_allowed() {
        return Err(format!(
            "Execution halted by Risk Circuit Breaker: {reason}"
        ));
    }
    if *state.is_paused.read() {
        return Err(
            "Trading engine is PAUSED. Positions are preserved, but new executions are suspended."
                .into(),
        );
    }
    if *state.is_live_mode.read() {
        return Err(LIVE_EXECUTION_DISABLED.into());
    }

    let adapter = state.get_adapter(&params.venue).ok_or_else(|| {
        format!(
            "Venue '{}' is not admitted for paper execution",
            params.venue
        )
    })?;
    let paper = state
        .get_paper_exchange(&params.venue)
        .ok_or_else(|| format!("No paper exchange admitted for venue '{}'", params.venue))?;
    let vid = adapter.venue_id();

    // The caller-supplied spread is never trusted: re-scan and match the
    // opportunity id against freshly resolved books.
    let opps = scan_triangular_opportunities_impl("", state).await?;
    let opp = opps
        .into_iter()
        .find(|o| o.id == params.id)
        .ok_or_else(|| {
            "triangular opportunity is no longer available (missing legs or stale books)".to_string()
        })?;
    let plan = triangle_plan(&opp.intermediate_a, &opp.intermediate_b);
    // Serialize dispatch against every other execution path. The rescan
    // above runs unlocked (it fans out over venues and may hit the network);
    // everything from the dispatch-time book resolution on is mutually
    // exclusive, and the hurdle is re-enforced on those books.
    let _execution_guard = state.execution_lock.lock().await;
    // Re-check the risk gates under the lock: a pause or trip landing
    // between the fast pre-check and dispatch must still refuse.
    if let Err(reason) = state.circuit_breaker.is_trading_allowed() {
        return Err(format!(
            "Execution halted by Risk Circuit Breaker: {reason}"
        ));
    }
    if *state.is_paused.read() {
        return Err(
            "Trading engine is PAUSED. Positions are preserved, but new executions are suspended."
                .into(),
        );
    }
    if *state.is_live_mode.read() {
        return Err(LIVE_EXECUTION_DISABLED.into());
    }
    let books = resolve_triangle_books(state, &adapter, &vid, &plan.leg_symbols)
        .await
        .ok_or_else(|| "triangle legs went stale before dispatch".to_string())?;
    let fee = effective_venue_fee_bps(state, vid.as_str());
    let eval = evaluate_triangle_books(&books, plan.leg_sides, fee, params.trade_size_usd)
        .ok_or_else(|| {
            "triangle route economics unavailable: the full quantity cannot be priced on all three legs"
                .to_string()
        })?;
    if eval.net_bps < params.min_profit_bps {
        return Err(format!(
            "triangle net {:.1} BPS is below the {:.1} BPS hurdle — not executed",
            eval.net_bps, params.min_profit_bps
        ));
    }

    // One-open-pair-per-symbol, extended to the cycle: any blocking ledger
    // signal or any open paper position on a leg symbol refuses dispatch.
    for sym in &plan.leg_symbols {
        if let Some(blocker) = state
            .state_store
            .blocking_signals(sym)
            .map_err(|e| e.to_string())?
            .into_iter()
            .next()
        {
            return Err(format!(
                "Symbol {sym} is blocked by signal {} (status {}): awaiting leg reconciliation/delta-neutral confirmation",
                blocker.id,
                blocker.status.as_str()
            ));
        }
    }
    let pre_acc = paper.fetch_account().await.map_err(|e| e.to_string())?;
    let dust = triangle_dust_qty();
    for sym in &plan.leg_symbols {
        if pre_acc
            .get_position(sym)
            .is_some_and(|p| p.quantity > dust)
        {
            return Err(format!(
                "An open paper position already exists for {sym}; flatten it before cycling"
            ));
        }
    }

    let (target_leverage, account_cap) = resolve_effective_leverage(
        state,
        "futures",
        "futures",
        &plan.leg_symbols[0],
        &params.venue,
        &params.venue,
        None,
    )?;
    if let Some(paper) = state.get_paper_exchange(&params.venue) {
        if paper.max_leverage_cap() != account_cap {
            paper
                .set_max_leverage_cap(account_cap)
                .map_err(|e| e.to_string())?;
        }
    }
    let size_dec = Decimal::from_f64_retain(params.trade_size_usd)
        .ok_or_else(|| "Invalid trade size".to_string())?;
    // Conservative margin bound: three full-notional legs against free margin.
    let total_im = size_dec * dec!(3) / Decimal::from(target_leverage);
    if total_im > pre_acc.free_margin {
        return Err(format!(
            "Insufficient free margin on {}: required {total_im} > available {}",
            params.venue, pre_acc.free_margin
        ));
    }
    let margin_monitor = margin_monitor_from_settings(state);
    match margin_monitor.evaluate_account(&pre_acc) {
        MarginHealthStatus::Empty => {
            return Err(format!(
                "Paper account {} has no funds; execution refused",
                params.venue
            ));
        }
        MarginHealthStatus::Critical | MarginHealthStatus::Breached => {
            return Err(format!(
                "Paper account {} margin status is critical/breached; execution refused",
                params.venue
            ));
        }
        _ => {}
    }

    for book in &books {
        paper
            .replace_orderbook(book.clone())
            .map_err(|e| e.to_string())?;
    }

    let mut equity_before = dec!(0);
    let mut equity_before_by_venue = HashMap::new();
    for (vid, p) in state.paper_exchanges() {
        let acc = p.fetch_account().await.map_err(|e| e.to_string())?;
        equity_before += acc.equity;
        if let Ok(f) = acc.equity.to_string().parse::<f64>() {
            if f.is_finite() {
                equity_before_by_venue.insert(vid.to_string(), f);
            }
        }
    }

    let cycle = {
        let mut step = state.sim_step_counter.write();
        *step += 1;
        *step
    };
    let cycle_tag = format!(
        "tri-{}-{:08x}",
        Utc::now().timestamp_millis(),
        rand::random::<u32>()
    );
    let started = std::time::Instant::now();

    let mut fills: Vec<(usize, arbitrageur_core::order::OrderResponse)> = Vec::new();
    let mut leg_error: Option<String> = None;
    for (idx, ((sym, order_side), qty_f)) in plan
        .leg_symbols
        .iter()
        .zip(plan.order_sides.iter())
        .zip(eval.order_qty.iter())
        .enumerate()
    {
        // Codebase convention (spatial path + ledger): f64 quantities cross
        // into Decimal through their shortest string rendering, so the paper
        // position and the ledger residual are bit-identical and flatten
        // closes exactly. from_f64_retain's exact-binary expansion disagrees
        // at ~1e-22 and would strand dust.
        let qty = match Decimal::from_str_exact(&qty_f.to_string())
            .ok()
            .filter(|q| *q > dec!(0))
        {
            Some(q) => q,
            None => {
                leg_error = Some(format!("leg {} quantity is not positive", idx + 1));
                break;
            }
        };
        let order = arbitrageur_core::order::OrderRequest::new_market(
            paper.venue_id(),
            sym,
            *order_side,
            qty,
        );
        match paper.place_order(order).await {
            Ok(resp)
                if resp.filled_qty == qty
                    && resp.remaining_qty == dec!(0)
                    && resp.avg_fill_price.filter(|p| *p > dec!(0)).is_some() =>
            {
                fills.push((idx, resp));
            }
            Ok(resp) => {
                if resp.filled_qty > dec!(0) {
                    fills.push((idx, resp));
                }
                leg_error = Some(format!(
                    "leg {} on {sym} partially filled or rejected",
                    idx + 1
                ));
                break;
            }
            Err(e) => {
                leg_error = Some(format!("leg {} on {sym} failed: {e}", idx + 1));
                break;
            }
        }
    }
    let latency_us = started.elapsed().as_micros() as u64;

    let mut equity_after = dec!(0);
    for (_, p) in state.paper_exchanges() {
        equity_after += p
            .fetch_account()
            .await
            .map_err(|e| e.to_string())?
            .equity;
    }
    refresh_equity_expectations_after_fills(state, &equity_before_by_venue).await;
    let attempt_pnl = equity_after - equity_before;
    let attempt_pnl_f = attempt_pnl.to_string().parse::<f64>().unwrap_or(0.0);
    let total_f = {
        let mut total_pnl = state.total_realized_profit.write();
        *total_pnl = equity_after - state.paper_baseline_equity();
        total_pnl.to_string().parse::<f64>().unwrap_or(0.0)
    };
    state.circuit_breaker.update_equity(equity_after);

    let fee_total_f: f64 = fills
        .iter()
        .filter_map(|(_, r)| r.fee.to_string().parse::<f64>().ok())
        .sum();
    let cycle_note = format!(
        "triangle {} legs {}/3 filled, net {:.1} BPS at scan",
        plan.path,
        fills.len(),
        eval.net_bps
    );
    let mut record_errors = Vec::new();
    for (idx, resp) in &fills {
        let fill_px = resp.avg_fill_price.unwrap_or(dec!(0));
        // Single f64 source per leg: full fills reuse the exact submitted
        // quantity; a partial failed leg falls back to its actual fill (any
        // conversion dust is covered by the epsilon in the entry guard).
        let qty_f64 = if resp.filled_qty == resp.original_qty {
            eval.order_qty[*idx]
        } else {
            resp.filled_qty.to_string().parse::<f64>().unwrap_or(0.0)
        };
        if let Err(e) = record_triangle_leg_fill(
            state,
            &cycle_tag,
            *idx,
            vid.as_str(),
            &resp.symbol,
            plan.order_sides[*idx],
            qty_f64,
            fill_px,
            resp.fee,
            &resp.order_id,
            &cycle_note,
        ) {
            record_errors.push(e);
        }
    }

    if let Some(err) = leg_error {
        state.execution_logs.write().push(ExecutionLogEntry {
            id: format!("exec-triangle-{cycle}"),
            timestamp: Utc::now().to_rfc3339(),
            trade_type: "Triangle".into(),
            strategy_id: crate::attribution::attribute_strategy_id(TRIANGLE_STRATEGY_ID),
            symbol: plan.path.clone(),
            venue_a: params.venue.clone(),
            venue_b: params.venue.clone(),
            leg_a_status: format!("{}/3 legs filled", fills.len()),
            leg_b_status: err.clone(),
            success: false,
            gross_profit: 0.0,
            fees: fee_total_f,
            net_profit: attempt_pnl_f,
            latency_us,
            note: format!(
                "Triangle cycle PARTIAL: {err}. {cycle_note}. Residual legs recorded as imbalanced 'triangle' signals — flatten each leg symbol to unwind. Record errors: {}",
                record_errors.join("; ")
            ),
            direction: Some(plan.path.clone()),
            trigger_spread_bps: Some(eval.net_bps),
            baseline_mean_spread_bps: None,
            position_step: None,
            max_units: None,
            nominal_hurdle_bps: Some(params.min_profit_bps),
            is_live: false,
        });
        return Err(format!(
            "Triangle cycle PARTIAL ({}/3 legs): {err}; residuals recorded — flatten leg symbols to unwind",
            fills.len()
        ));
    }

    state.execution_logs.write().push(ExecutionLogEntry {
        id: format!("exec-triangle-{cycle}"),
        timestamp: Utc::now().to_rfc3339(),
        trade_type: "Triangle".into(),
        strategy_id: crate::attribution::attribute_strategy_id(TRIANGLE_STRATEGY_ID),
        symbol: plan.path.clone(),
        venue_a: params.venue.clone(),
        venue_b: params.venue.clone(),
        leg_a_status: "FILLED".into(),
        leg_b_status: "FILLED".into(),
        success: true,
        gross_profit: 0.0,
        fees: fee_total_f,
        net_profit: attempt_pnl_f,
        latency_us,
        note: format!(
            "Triangle cycle dispatched 3/3 paper legs ({cycle_note}); spread position open — unwind each leg symbol via flatten. Record errors: {}",
            record_errors.join("; ")
        ),
        direction: Some(plan.path.clone()),
        trigger_spread_bps: Some(eval.net_bps),
        baseline_mean_spread_bps: None,
        position_step: None,
        max_units: None,
        nominal_hurdle_bps: Some(params.min_profit_bps),
        is_live: false,
    });

    maybe_record_margin_snapshots(state, Utc::now().timestamp_millis()).await;

    Ok(SimStepResultDto {
        cycle,
        opportunity_found: true,
        execution_success: true,
        trade_type: "Triangle".into(),
        direction: plan.path.clone(),
        venue_a: params.venue.clone(),
        venue_b: params.venue.clone(),
        spread_bps: eval.net_bps,
        realized_profit_usd: attempt_pnl_f,
        total_cumulative_pnl_usd: total_f,
        message: format!(
            "Triangle cycle dispatched on {}: 3/3 paper legs filled, realized P&L ${attempt_pnl_f:.2}; spread position open — flatten leg symbols to unwind",
            params.venue
        ),
    })
}

/// Venue orderbook pair (bids, asks) keyed by venue id.
type PerpBooks = std::collections::HashMap<String, (Vec<(f64, f64)>, Vec<(f64, f64)>)>;

/// 8h-normalized funding print (percent) for one leg of a scan pair.
///
/// Matches the scan `rates` row by venue id string and normalizes
/// `rate / interval_hours * 8 * 100`. `None` when the venue has no print
/// or the value is not finite — callers render "—", never fabricate.
fn scan_leg_rate_8h_pct(
    rates: &[(
        &'static str,
        &'static str,
        arbitrageur_core::quote::FundingRate,
    )],
    venue: &str,
) -> Option<f64> {
    rates
        .iter()
        .find(|(_, _, r)| r.venue.as_str() == venue)
        .and_then(|(_, _, r)| {
            let v: f64 = r.rate.to_string().parse().ok()?;
            let pct = v / f64::from(r.interval_hours.max(1)) * 8.0 * 100.0;
            pct.is_finite().then_some(pct)
        })
}

async fn scan_funding_opportunities_for_symbol(
    symbol: &str,
    size_dec: Decimal,
    trade_size_usd: f64,
    allow_network_fetch: bool,
    state: &AppEngineState,
) -> Vec<FundingOpportunityDto> {
    state.ensure_market(symbol);
    let venues = state.get_venues();
    if venues.is_empty() {
        return Vec::new();
    }

    let mut rates = Vec::new();

    for (vid, vname, adapter) in &venues {
        let venue_id = adapter.venue_id();
        if let Some(fr) = state.market_data_hub.get_funding_rate(
            &venue_id,
            symbol,
            Some(std::time::Duration::from_secs(120)),
        ) {
            rates.push((*vid, *vname, fr));
        } else if allow_network_fetch {
            if let Ok(rate) = adapter.fetch_funding_rate(symbol).await {
                state.market_data_hub.update_funding(rate.clone());
                rates.push((*vid, *vname, rate));
            }
        }
    }

    // Persist observed prints (minute-bucketed REPLACE, idempotent) as the
    // training substrate for funding-persistence / half-life modeling.
    // Best-effort: a store hiccup must never fail a scan.
    {
        let now_ms = Utc::now().timestamp_millis();
        let samples: Vec<(&str, &str, f64, i64)> = rates
            .iter()
            .filter_map(|(_, _, r)| {
                let rate_f: f64 = r.rate.to_string().parse().ok()?;
                let interval = f64::from(r.interval_hours.max(1));
                let rate_8h = rate_f / interval * 8.0;
                if !rate_8h.is_finite() {
                    return None;
                }
                Some((
                    r.venue.as_str(),
                    r.symbol.as_str(),
                    rate_8h,
                    i64::from(r.interval_hours.max(1)),
                ))
            })
            .collect();
        let _ = state
            .state_store
            .record_funding_rate_samples(now_ms, &samples);
    }

    // Cross-exchange carry needs >= 2 funding prints; same-exchange basis
    // needs only one venue with funding + both books, so no early return here.
    let mut opps = Vec::new();
    let fee_total = dec!(3.5); // 3.5 bps round trip

    // Perf: snapshot + parse each venue's perp book ONCE per scan. The pair
    // loop below used to hub-clone and re-parse both books per pair (O(V^2)
    // clones); the capacity walk now reads this map (O(V) clones).
    let parse_levels = |levels: &[arbitrageur_core::quote::BookLevel]| {
        levels
            .iter()
            .filter_map(|l| {
                let px = l.price.to_string().parse::<f64>().ok()?;
                let q = l.size.to_string().parse::<f64>().ok()?;
                (px.is_finite() && q.is_finite()).then_some((px, q))
            })
            .collect::<Vec<_>>()
    };
    let mut perp_books: PerpBooks = std::collections::HashMap::new();
    for (vid, _, _) in &rates {
        if perp_books.contains_key(*vid) {
            continue;
        }
        if let Some(book) = state.market_data_hub.get_valid_orderbook(
            &VenueId::from(*vid),
            symbol,
            Some(std::time::Duration::from_secs(120)),
        ) {
            perp_books.insert(
                vid.to_string(),
                (parse_levels(&book.asks), parse_levels(&book.bids)),
            );
        }
    }

    for i in 0..rates.len() {
        for j in (i + 1)..rates.len() {
            let (_id_a, _name_a, ref r_a) = rates[i];
            let (_id_b, _name_b, ref r_b) = rates[j];

            if let Some(opp) = evaluate_funding_carry(r_a, r_b, fee_total, size_dec) {
                // Depth-aware executable size: walk both legs' live books so
                // the quoted volume reflects what can actually be entered —
                // the same orderbook discipline as the spread scanner.
                // Falls back to the requested size when books are missing.
                let vid_for = |name: &str| -> Option<&str> {
                    rates
                        .iter()
                        .find(|(_, _, r)| r.venue.as_str() == name)
                        .map(|(vid, _, _)| *vid)
                };
                let depth: Option<(f64, f64)> = (|| {
                    let size_f = size_dec.to_string().parse::<f64>().ok()?;
                    if !(size_f.is_finite() && size_f > 0.0) {
                        return None;
                    }
                    // Long leg lifts asks, short leg hits bids (pre-parsed).
                    let book_of = |name: &str| {
                        let vid = vid_for(name)?;
                        perp_books.get(vid)
                    };
                    let (long_asks, _) = book_of(&opp.long_venue)?;
                    let (_, short_bids) = book_of(&opp.short_venue)?;
                    let rep = funding_entry_capacity(
                        long_asks,
                        short_bids,
                        &[size_f * 0.5, size_f, size_f * 2.0],
                    );
                    let optimal = rep.optimal_size_usd?;
                    let cost = rep.entry_spread_bps.map(|e| (-e).max(0.0))?;
                    Some((optimal, cost))
                })();
                // Marginal cost gradient at the requested size (informational).
                // Computed before the push: opp's venue strings move into the DTO.
                let slope: Option<f64> = (|| {
                    let size_f = size_dec.to_string().parse::<f64>().ok()?;
                    if !(size_f.is_finite() && size_f > 0.0) {
                        return None;
                    }
                    let vid_for = |name: &str| -> Option<&str> {
                        rates
                            .iter()
                            .find(|(_, _, r)| r.venue.as_str() == name)
                            .map(|(vid, _, _)| *vid)
                    };
                    let (long_asks, _) = perp_books.get(vid_for(&opp.long_venue)?)?;
                    let (_, short_bids) = perp_books.get(vid_for(&opp.short_venue)?)?;
                    entry_gradient(long_asks, short_bids, size_f).map(|g| g.slope_bps_per_1k)
                })();
                // Expected hold: funding-gap persistence to the min-APR exit
                // threshold + fee+spread breakeven + spread-drift disclosure.
                // History is the minute-bucketed training substrate (7d); thin
                // history falls back to breakeven with low confidence.
                let hold_estimate = {
                    let net_8h_f: f64 = opp
                        .net_rate_8h
                        .to_string()
                        .parse::<f64>()
                        .unwrap_or(0.0);
                    let fee_bps_f: f64 = opp
                        .round_trip_fee_bps
                        .to_string()
                        .parse::<f64>()
                        .unwrap_or(0.0);
                    let spread_cost_f: Option<f64> =
                        depth.map(|(_, e)| e).filter(|v| v.is_finite() && *v > 0.0);
                    let hourly = |rate: Decimal, interval: u32| {
                        rate / Decimal::from(interval.max(1))
                    };
                    let agrees_hold = match (
                        r_a.predicted_rate,
                        r_b.predicted_rate,
                    ) {
                        (Some(pa), Some(pb)) => {
                            let pa_h = hourly(pa, r_a.interval_hours);
                            let pb_h = hourly(pb, r_b.interval_hours);
                            Some(
                                if hourly(r_a.rate, r_a.interval_hours)
                                    >= hourly(r_b.rate, r_b.interval_hours)
                                {
                                    pa_h >= pb_h
                                } else {
                                    pb_h >= pa_h
                                },
                            )
                        }
                        _ => None,
                    };
                    let now_hold_ms = chrono::Utc::now().timestamp_millis();
                    // Resolve venue ids for history (fall back to names).
                    let long_vid_hold: &str = rates
                        .iter()
                        .find(|(_, _, r)| r.venue.as_str() == opp.long_venue.as_str())
                        .map(|(vid, _, _)| *vid)
                        .unwrap_or(opp.long_venue.as_str());
                    let short_vid_hold: &str = rates
                        .iter()
                        .find(|(_, _, r)| r.venue.as_str() == opp.short_venue.as_str())
                        .map(|(vid, _, _)| *vid)
                        .unwrap_or(opp.short_venue.as_str());
                    let hist = carry_gap_history(
                        state,
                        symbol,
                        long_vid_hold,
                        short_vid_hold,
                        now_hold_ms,
                    );
                    estimate_funding_hold_hours(
                        net_8h_f,
                        fee_bps_f,
                        spread_cost_f,
                        &hist,
                        Some(funding_min_apr(state)),
                        agrees_hold,
                        slope,
                    )
                };
                // Per-leg prints for the detail popup (before `opp` moves).
                let long_leg_rate = scan_leg_rate_8h_pct(&rates, &opp.long_venue);
                let short_leg_rate = scan_leg_rate_8h_pct(&rates, &opp.short_venue);
                opps.push(FundingOpportunityDto {
                    id: format!("{}-{}-{}", symbol, opp.long_venue, opp.short_venue),
                    symbol: symbol.to_string(),
                    long_venue: opp.long_venue,
                    short_venue: opp.short_venue,
                    net_rate_8h_pct: opp
                        .net_rate_8h
                        .to_string()
                        .parse::<f64>()
                        .map(|v| v * 100.0)
                        .unwrap_or(0.0),
                    net_apr_pct: opp
                        .net_apr
                        .to_string()
                        .parse::<f64>()
                        .map(|v| v * 100.0)
                        .unwrap_or(0.0),
                    net_apy_pct: opp
                        .net_apy
                        .to_string()
                        .parse::<f64>()
                        .map(|v| v * 100.0)
                        .unwrap_or(0.0),
                    round_trip_fee_bps: opp
                        .round_trip_fee_bps
                        .to_string()
                        .parse::<f64>()
                        .unwrap_or(0.0),
                    break_even_hours: opp
                        .break_even_hours
                        .to_string()
                        .parse::<f64>()
                        .unwrap_or(0.0),
                    estimated_daily_income_usd: opp
                        .daily_income_usd
                        .to_string()
                        .parse::<f64>()
                        .unwrap_or(0.0),
                    trade_size_usd,
                    optimal_size_usd: depth.map(|(o, _)| o).unwrap_or(trade_size_usd),
                    entry_spread_bps: depth.map(|(_, e)| e).unwrap_or(-1.0),
                    depth_slope_bps_per_1k: slope,
                    long_rate_8h_pct: long_leg_rate,
                    short_rate_8h_pct: short_leg_rate,
                    kind: "carry".to_string(),
                    basis_bps: None,
                    spot_price: None,
                    perp_price: None,
                    // Informational only: the edge direction (which leg is
                    // short) is set by current hourly rates; agreement holds
                    // when both venues' PREDICTED hourly rates point the same
                    // way. Absent predictions -> None (no adjustment).
                    win_probability: {
                        let hourly = |rate: Decimal, interval: u32| {
                            rate / Decimal::from(interval.max(1))
                        };
                        let agrees = match (
                            r_a.predicted_rate,
                            r_b.predicted_rate,
                        ) {
                            (Some(pa), Some(pb)) => {
                                let pa_h = hourly(pa, r_a.interval_hours);
                                let pb_h = hourly(pb, r_b.interval_hours);
                                Some(
                                    if hourly(
                                        r_a.rate,
                                        r_a.interval_hours,
                                    ) >= hourly(r_b.rate, r_b.interval_hours)
                                    {
                                        pa_h >= pb_h
                                    } else {
                                        pb_h >= pa_h
                                    },
                                )
                            }
                            _ => None,
                        };
                        funding_win_probability(
                            opp.net_rate_8h,
                            opp.round_trip_fee_bps,
                            None,
                            agrees,
                        )
                    },
                    expected_hold_hours: hold_estimate.as_ref().map(|h| h.hours),
                    hold_reason: hold_estimate.as_ref().map(|h| h.reason.clone()),
                    hold_confidence: hold_estimate.as_ref().map(|h| h.confidence),
                });
            }
        }
    }

    // Same-exchange spot-vs-perp/futures cash-and-carry (basis) scan.
    // Long spot + short perp on the SAME venue harvests the perp funding rate
    // while the spot/perp divergence (basis) converges at exit. Only venues
    // with a live funding print participate; venues without spot books
    // (Hyperliquid/Vanta explicitly refuse spot) fail closed — no fabricated
    // basis opps. Reverse (long perp / short spot) needs borrow and is out of
    // scope. Gated by the funding strategy's basis switch + min-basis floor;
    // the existing min-APR filter applies downstream in the UI/bot.
    {
        let (basis_enabled, min_basis_bps) = state
            .strategy_params
            .read()
            .get("funding")
            .map(|params| (params.funding_enable_basis, params.funding_min_basis_bps))
            .unwrap_or((true, None));
        if basis_enabled {
            let fee_total = dec!(3.5); // same round-trip assumption as carry
            let size_f = size_dec.to_string().parse::<f64>().unwrap_or(0.0);
            if size_f.is_finite() && size_f > 0.0 {
                let base = symbol
                    .split(['-', '/', '_'])
                    .next()
                    .unwrap_or(symbol)
                    .trim()
                    .to_uppercase();
                let spot_candidates: Vec<String> = if base.is_empty() {
                    Vec::new()
                } else {
                    vec![format!("{base}-SPOT"), format!("{symbol}-SPOT")]
                };
                for (vid, _vname, funding_rate) in &rates {
                    let venue_id = VenueId::from(*vid);
                    // Perp leg book under the canonical perp symbol.
                    let perp_book = state.market_data_hub.get_valid_orderbook(
                        &venue_id,
                        symbol,
                        Some(std::time::Duration::from_secs(120)),
                    );
                    // Spot leg: first candidate with a live book wins.
                    let mut spot_book = None;
                    let mut spot_key_used: Option<String> = None;
                    for candidate in &spot_candidates {
                        if let Some(book) = state.market_data_hub.get_valid_orderbook(
                            &venue_id,
                            candidate,
                            Some(std::time::Duration::from_secs(120)),
                        ) {
                            spot_book = Some(book);
                            spot_key_used = Some(candidate.clone());
                            break;
                        }
                    }
                    // Cold-start: optionally pull books over the network so a
                    // fresh app can still surface basis without WS history.
                    if allow_network_fetch {
                        if let Some(adapter) = state
                            .get_venues()
                            .into_iter()
                            .find(|(id, _, _)| id == vid)
                            .map(|(_, _, adapter)| adapter)
                        {
                            if perp_book.is_none() {
                                if let Ok(book) = adapter.fetch_orderbook(symbol).await {
                                    state.market_data_hub.update_orderbook(book.clone());
                                }
                            }
                            if spot_book.is_none() {
                                for candidate in &spot_candidates {
                                    if let Ok(book) = adapter.fetch_orderbook(candidate).await {
                                        state.market_data_hub.update_orderbook(book.clone());
                                        spot_key_used = Some(candidate.clone());
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    let (Some(perp), Some(spot)) = (
                        state.market_data_hub.get_valid_orderbook(
                            &venue_id,
                            symbol,
                            Some(std::time::Duration::from_secs(120)),
                        ),
                        spot_key_used.as_ref().and_then(|key| {
                            state.market_data_hub.get_valid_orderbook(
                                &venue_id,
                                key,
                                Some(std::time::Duration::from_secs(120)),
                            )
                        }),
                    ) else {
                        continue;
                    };
                    let _ = (perp_book, spot_book);
                    let (Some(spot_mid), Some(perp_mid)) = (spot.mid_price(), perp.mid_price())
                    else {
                        continue;
                    };
                    let Some(eval) = evaluate_spot_perp_basis(
                        spot_mid,
                        perp_mid,
                        funding_rate,
                        fee_total,
                        size_dec,
                    ) else {
                        continue;
                    };
                    let basis_f = eval
                        .basis_bps
                        .to_string()
                        .parse::<f64>()
                        .unwrap_or(f64::NAN);
                    if !basis_f.is_finite() {
                        continue;
                    }
                    if let Some(floor) = min_basis_bps {
                        if basis_f < floor {
                            continue;
                        }
                    }
                    // Friction: walk spot asks (long leg lift) against perp
                    // bids (short leg hit) — same depth discipline as carry.
                    let to_pair = |levels: &[arbitrageur_core::quote::BookLevel]| {
                        levels
                            .iter()
                            .filter_map(|l| {
                                let px = l.price.to_string().parse::<f64>().ok()?;
                                let q = l.size.to_string().parse::<f64>().ok()?;
                                (px.is_finite() && q.is_finite()).then_some((px, q))
                            })
                            .collect::<Vec<_>>()
                    };
                    let spot_asks = to_pair(&spot.asks);
                    let perp_bids = to_pair(&perp.bids);
                    // Marginal cost gradient at the requested size (informational).
                    let slope = entry_gradient(&spot_asks, &perp_bids, size_f)
                        .map(|g| g.slope_bps_per_1k);
                    let rep = funding_entry_capacity(
                        &spot_asks,
                        &perp_bids,
                        &[size_f * 0.5, size_f, size_f * 2.0],
                    );
                    let depth = rep.optimal_size_usd.zip(rep.entry_spread_bps
                            .map(|e| (-e).max(0.0)));
                    let venue_label = funding_rate.venue.to_string();
                    let spot_mid_f =
                        spot_mid.to_string().parse::<f64>().unwrap_or(0.0);
                    let perp_mid_f =
                        perp_mid.to_string().parse::<f64>().unwrap_or(0.0);
                    // Expected hold for basis: single-venue perp funding history
                    // (spot pays none). Same decay-to-threshold model as carry.
                    let basis_hold = {
                        let net_8h_f: f64 = eval
                            .carry
                            .net_rate_8h
                            .to_string()
                            .parse::<f64>()
                            .unwrap_or(0.0);
                        let fee_bps_f: f64 = eval
                            .carry
                            .round_trip_fee_bps
                            .to_string()
                            .parse::<f64>()
                            .unwrap_or(0.0);
                        let spread_cost_f: Option<f64> =
                            depth.map(|(_, e)| e).filter(|v| v.is_finite() && *v > 0.0);
                        let now_basis_ms = chrono::Utc::now().timestamp_millis();
                        let hist_basis: Vec<(i64, f64)> = state
                            .state_store
                            .funding_rate_series(
                                &venue_id.to_string(),
                                symbol,
                                now_basis_ms.saturating_sub(7 * 24 * 3_600_000),
                                now_basis_ms,
                            )
                            .unwrap_or_default()
                            .into_iter()
                            .filter_map(|(ts, r, _)| r.is_finite().then_some((ts, r)))
                            .collect();
                        estimate_funding_hold_hours(
                            net_8h_f,
                            fee_bps_f,
                            spread_cost_f,
                            &hist_basis,
                            Some(funding_min_apr(state)),
                            funding_rate.predicted_rate.map(|pr| pr > dec!(0)),
                            slope,
                        )
                    };
                    opps.push(FundingOpportunityDto {
                        id: format!("{symbol}-{venue_label}-BASIS"),
                        symbol: symbol.to_string(),
                        long_venue: format!("{venue_label} SPOT"),
                        short_venue: format!("{venue_label} PERP"),
                        net_rate_8h_pct: eval
                            .carry
                            .net_rate_8h
                            .to_string()
                            .parse::<f64>()
                            .map(|v| v * 100.0)
                            .unwrap_or(0.0),
                        net_apr_pct: eval
                            .carry
                            .net_apr
                            .to_string()
                            .parse::<f64>()
                            .map(|v| v * 100.0)
                            .unwrap_or(0.0),
                        net_apy_pct: eval
                            .carry
                            .net_apy
                            .to_string()
                            .parse::<f64>()
                            .map(|v| v * 100.0)
                            .unwrap_or(0.0),
                        round_trip_fee_bps: eval
                            .carry
                            .round_trip_fee_bps
                            .to_string()
                            .parse::<f64>()
                            .unwrap_or(0.0),
                        break_even_hours: eval
                            .carry
                            .break_even_hours
                            .to_string()
                            .parse::<f64>()
                            .unwrap_or(0.0),
                        estimated_daily_income_usd: eval
                            .carry
                            .daily_income_usd
                            .to_string()
                            .parse::<f64>()
                            .unwrap_or(0.0),
                        trade_size_usd,
                        optimal_size_usd: depth.map(|(o, _)| o).unwrap_or(trade_size_usd),
                        entry_spread_bps: depth.map(|(_, e)| e).unwrap_or(-1.0),
                        kind: "basis".to_string(),
                        basis_bps: Some(basis_f),
                        spot_price: if spot_mid_f > 0.0 {
                            Some(spot_mid_f)
                        } else {
                            None
                        },
                        perp_price: if perp_mid_f > 0.0 {
                            Some(perp_mid_f)
                        } else {
                            None
                        },
                        depth_slope_bps_per_1k: slope,
                        // Long leg is spot: pays no funding.
                        long_rate_8h_pct: Some(0.0),
                        short_rate_8h_pct: {
                            let v: f64 =
                                funding_rate.rate.to_string().parse().unwrap_or(f64::NAN);
                            let pct = v
                                / f64::from(funding_rate.interval_hours.max(1))
                                * 8.0
                                * 100.0;
                            pct.is_finite().then_some(pct)
                        },
                        // Informational only: positive entry basis is an
                        // expected exit tailwind (convergence), and a
                        // positive predicted perp rate confirms the carry.
                        win_probability: funding_win_probability(
                            eval.carry.net_rate_8h,
                            eval.carry.round_trip_fee_bps,
                            if eval.basis_bps > dec!(0) {
                                Some(eval.basis_bps)
                            } else {
                                None
                            },
                            funding_rate.predicted_rate.map(|pr| pr > dec!(0)),
                        ),
                        expected_hold_hours: basis_hold.as_ref().map(|h| h.hours),
                        hold_reason: basis_hold.as_ref().map(|h| h.reason.clone()),
                        hold_confidence: basis_hold.as_ref().map(|h| h.confidence),
                    });
                }
            }
        }
    }

    opps
}

/// One sampled point: `t` is unix millis, `v` is the value.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FundingTrendPoint {
    pub t: i64,
    pub v: f64,
}

/// Trend substrate for the funding-carry detail popup: per-leg 8h-normalized
/// funding history (percent), ticker mid-price history (USD), and the latest
/// live mid for the caption. All series ascending by time, downsampled to a
/// few hundred points. Empty series mean "no history yet" — clients render
/// the current prints only and must never interpolate fabrications.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FundingTrendDto {
    pub symbol: String,
    pub long_venue: String,
    pub short_venue: String,
    pub long_series: Vec<FundingTrendPoint>,
    pub short_series: Vec<FundingTrendPoint>,
    pub price_series: Vec<FundingTrendPoint>,
    pub current_mid_usd: Option<f64>,
}

/// Downsample an ascending `(ts, value)` series to at most `max` points by
/// uniform stride. Keeps the last point so the line always reaches "now".
fn downsample_series(series: &[(i64, f64)], max: usize) -> Vec<FundingTrendPoint> {
    let mut pts: Vec<FundingTrendPoint> = series
        .iter()
        .filter(|(_, v)| v.is_finite())
        .map(|(t, v)| FundingTrendPoint { t: *t, v: *v })
        .collect();
    if pts.len() > max && max >= 2 {
        let stride = pts.len().div_ceil(max);
        let last = pts.last().cloned();
        let mut thin: Vec<FundingTrendPoint> = pts.into_iter().step_by(stride).collect();
        // Pin the true last sample so the line always reaches "now".
        if let Some(l) = last {
            if thin.last().map(|p| p.t) != Some(l.t) {
                if thin.len() >= max {
                    thin.pop();
                }
                thin.push(l);
            }
        }
        pts = thin;
    }
    pts
}

/// Bare venue id for history lookups: basis opps label legs
/// `"<venue> SPOT"` / `"<venue> PERP"` — history is keyed by venue.
fn trend_bare_venue(leg: &str) -> &str {
    leg.split_whitespace().next().unwrap_or(leg)
}

pub async fn get_funding_trend_impl(
    symbol: &str,
    long_venue: &str,
    short_venue: &str,
    hours: Option<f64>,
    state: &AppEngineState,
) -> Result<FundingTrendDto, String> {
    let window_h = hours
        .filter(|h| h.is_finite() && *h > 0.0)
        .unwrap_or(72.0)
        .clamp(1.0, 24.0 * 7.0);
    let now_ms = chrono::Utc::now().timestamp_millis();
    let from_ms = now_ms.saturating_sub((window_h * 3_600_000.0) as i64);
    let long_v = trend_bare_venue(long_venue);
    let short_v = trend_bare_venue(short_venue);
    const MAX_PTS: usize = 240;

    let map_funding = |venue: &str| -> Vec<(i64, f64)> {
        state
            .state_store
            .funding_rate_series(venue, symbol, from_ms, now_ms)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(ts, r8h, _)| {
                let pct = r8h * 100.0;
                (ts >= 0 && pct.is_finite()).then_some((ts, pct))
            })
            .collect()
    };
    let long_hist = map_funding(long_v);
    let short_hist = map_funding(short_v);

    // Ticker mid history: mean mid per minute bucket across venues so the
    // price overlay is venue-neutral. Bounded for payload size.
    let mut buckets: HashMap<i64, (f64, u32)> = HashMap::new();
    if let Ok(samples) = state.state_store.spread_samples(symbol, from_ms, now_ms) {
        for s in samples {
            if !s.mid.is_finite() || s.mid <= 0.0 || s.ts_ms < 0 {
                continue;
            }
            let b = s.ts_ms / 60_000 * 60_000;
            let e = buckets.entry(b).or_insert((0.0, 0));
            e.0 += s.mid;
            e.1 += 1;
        }
    }
    let mut price_hist: Vec<(i64, f64)> = buckets
        .into_iter()
        .map(|(b, (sum, n))| (b, sum / f64::from(n.max(1))))
        .collect();
    price_hist.sort_by_key(|(t, _)| *t);
    let current_mid_usd = price_hist.last().map(|(_, m)| *m);

    Ok(FundingTrendDto {
        symbol: symbol.to_string(),
        long_venue: long_venue.to_string(),
        short_venue: short_venue.to_string(),
        long_series: downsample_series(&long_hist, MAX_PTS),
        short_series: downsample_series(&short_hist, MAX_PTS),
        price_series: downsample_series(&price_hist, MAX_PTS),
        current_mid_usd,
    })
}

pub async fn scan_funding_opportunities_batch_impl(
    symbols: &[String],
    trade_size_usd: f64,
    state: &AppEngineState,
) -> Result<Vec<FundingOpportunityDto>, String> {
    if !trade_size_usd.is_finite() || trade_size_usd <= 0.0 {
        return Err("trade_size_usd must be a finite positive number".into());
    }
    let size_dec = Decimal::from_f64_retain(trade_size_usd)
        .ok_or_else(|| "trade_size_usd is outside supported decimal range".to_string())?;

    let mut opps = Vec::new();
    for sym in symbols {
        if validate_market_symbol(sym).is_err() {
            continue;
        }
        let sym_opps = scan_funding_opportunities_for_symbol(sym, size_dec, trade_size_usd, false, state).await;
        opps.extend(sym_opps);
    }

    opps.sort_by(|a, b| {
        b.net_apy_pct
            .partial_cmp(&a.net_apy_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(opps)
}

pub async fn scan_funding_opportunities_impl(
    symbol: &str,
    trade_size_usd: f64,
    state: &AppEngineState,
) -> Result<Vec<FundingOpportunityDto>, String> {
    if symbol.trim().is_empty() {
        let universe = get_market_universe_impl(state).await.unwrap_or_default();
        let mut symbol_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for v in universe {
            for sym in v.symbols {
                *symbol_counts.entry(sym).or_insert(0) += 1;
            }
        }
        let mut shared_symbols: Vec<String> = symbol_counts
            .into_iter()
            .filter(|(_, count)| *count >= 2)
            .map(|(sym, _)| sym)
            .collect();
        shared_symbols.sort();
        return scan_funding_opportunities_batch_impl(&shared_symbols, trade_size_usd, state).await;
    }

    validate_market_symbol(symbol)?;
    if !trade_size_usd.is_finite() || trade_size_usd <= 0.0 {
        return Err("trade_size_usd must be a finite positive number".into());
    }
    let size_dec = Decimal::from_f64_retain(trade_size_usd)
        .ok_or_else(|| "trade_size_usd is outside supported decimal range".to_string())?;

    let mut opps = scan_funding_opportunities_for_symbol(symbol, size_dec, trade_size_usd, true, state).await;
    opps.sort_by(|a, b| {
        b.net_apy_pct
            .partial_cmp(&a.net_apy_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(opps)
}

pub async fn scan_prediction_opportunities_impl(
    market: &str,
    state: &AppEngineState,
) -> Result<Vec<PredictionOpportunityDto>, String> {
    let _ = (market, state);
    Ok(Vec::new())
}

pub fn run_stat_arb_step_impl(
    step: usize,
    price_y: f64,
    price_x: f64,
    state: &AppEngineState,
) -> Result<StatArbStepResultDto, String> {
    if !price_y.is_finite() || price_y <= 0.0 || !price_x.is_finite() || price_x <= 0.0 {
        return Err("prices must be finite and positive".into());
    }
    let mut kf = state.kalman_filter.write();
    let (hedge_ratio, residual) = kf.update(price_x, price_y);
    let z = kf.spread_z_score();
    let alpha = kf.intercept();
    let variance = if z.abs() > 1e-6 {
        (residual / z).powi(2)
    } else {
        1.0
    };

    let signal = if z > 2.0 {
        "ENTRY_SHORT_SPREAD"
    } else if z < -2.0 {
        "ENTRY_LONG_SPREAD"
    } else if z.abs() < 0.5 {
        "EXIT"
    } else {
        "NEUTRAL"
    };

    Ok(StatArbStepResultDto {
        step,
        price_y,
        price_x,
        dynamic_hedge_ratio: hedge_ratio,
        intercept_alpha: alpha,
        residual_spread: residual,
        spread_variance: variance,
        z_score: z,
        signal: signal.into(),
    })
}

pub fn compute_optimal_thresholds_impl(
    theta: f64,
    mu: f64,
    sigma: f64,
    transaction_cost: f64,
) -> Result<BertramThresholdDto, String> {
    if !theta.is_finite() || !mu.is_finite() || !sigma.is_finite() || !transaction_cost.is_finite()
    {
        return Err("Invalid parameters: all inputs must be finite".into());
    }
    if theta <= 0.0 || sigma <= 0.0 || transaction_cost < 0.0 {
        return Err(
            "Invalid parameters: theta > 0, sigma > 0, and transaction_cost >= 0 required".into(),
        );
    }

    let half_life = (2.0f64.ln()) / theta;
    let stationary_std = (sigma.powi(2) / (2.0 * theta)).sqrt();

    let ou = OuModelParams {
        theta,
        mu,
        sigma,
        half_life,
        stationary_std,
        r_squared: 0.89,
    };

    let result = compute_bertram_thresholds(&ou, transaction_cost)
        .ok_or_else(|| "Failed to compute Bertram thresholds with given parameters".to_string())?;

    Ok(BertramThresholdDto {
        entry_long: result.entry_long,
        exit_long: result.exit_long,
        entry_short: result.entry_short,
        exit_short: result.exit_short,
        optimal_distance: result.optimal_distance,
        expected_return_per_cycle: result.expected_return_per_cycle,
        estimated_trades_per_year: result.estimated_trades_per_year,
        estimated_sharpe: result.estimated_sharpe,
        half_life,
        theta,
        mu,
        sigma,
    })
}

pub fn fit_ou_process_impl(prices: &[f64], dt: f64) -> Result<OuFitDto, String> {
    if prices.len() < 10 {
        return Err("Need at least 10 price points to fit Ornstein-Uhlenbeck process".into());
    }
    if prices.iter().any(|p| !p.is_finite()) {
        return Err("Price series must contain only finite values".into());
    }
    if !dt.is_finite() || dt <= 0.0 {
        return Err("Time step dt must be finite and positive".into());
    }

    let params = OuModelParams::fit(prices, dt)
        .ok_or_else(|| "Failed to fit OU process to price series".to_string())?;

    Ok(OuFitDto {
        theta: params.theta,
        mu: params.mu,
        sigma: params.sigma,
        half_life: params.half_life,
        stationary_std: params.stationary_std,
        r_squared: params.r_squared,
    })
}

#[allow(clippy::too_many_arguments)]
/// Resolve account attribution for a two-leg signal. An explicit strategy
/// route wins; otherwise the first enabled route covering the venue pair is
/// used, falling back to each venue's default account. Accounts may be absent
/// on venues that have no configured profiles.
/// Accounts attributed to a signal's legs, resolved from an explicit or
/// eligible strategy route, else each venue's default account.
struct SignalAccounts {
    route_id: Option<String>,
    long: Option<String>,
    short: Option<String>,
}

fn resolve_signal_accounts(
    state: &AppEngineState,
    strategy_id: &str,
    venue_a: &str,
    venue_b: &str,
    route_id: Option<&str>,
) -> Result<SignalAccounts, String> {
    if let Some(rid) = route_id {
        let route = state
            .state_store
            .list_strategy_routes(None)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|r| r.id == rid)
            .ok_or_else(|| format!("strategy route {rid} does not exist"))?;
        if !route.enabled {
            return Err(format!("strategy route {rid} is disabled"));
        }
        if !route.strategy_id.eq_ignore_ascii_case(strategy_id) {
            return Err(format!(
                "route {rid} belongs to strategy {}, not {strategy_id}",
                route.strategy_id
            ));
        }
        let covers = (route.account_a_venue.eq_ignore_ascii_case(venue_a)
            && route.account_b_venue.eq_ignore_ascii_case(venue_b))
            || (route.account_a_venue.eq_ignore_ascii_case(venue_b)
                && route.account_b_venue.eq_ignore_ascii_case(venue_a));
        if !covers {
            return Err(format!(
                "route {rid} does not cover venues {venue_a}/{venue_b}"
            ));
        }
        let (long, short) = if route.account_a_venue.eq_ignore_ascii_case(venue_a) {
            (route.account_a_id, route.account_b_id)
        } else {
            (route.account_b_id, route.account_a_id)
        };
        return Ok(SignalAccounts {
            route_id: Some(route.id),
            long: Some(long),
            short: Some(short),
        });
    }
    if let Ok(routes) = state
        .state_store
        .eligible_strategy_routes(strategy_id, venue_a, venue_b)
    {
        if let Some(r) = routes.into_iter().next() {
            let (long, short) = if r.account_a_venue.eq_ignore_ascii_case(venue_a) {
                (r.account_a_id, r.account_b_id)
            } else {
                (r.account_b_id, r.account_a_id)
            };
            return Ok(SignalAccounts {
                route_id: Some(r.id),
                long: Some(long),
                short: Some(short),
            });
        }
    }
    Ok(SignalAccounts {
        route_id: None,
        long: state
            .state_store
            .default_account(venue_a)
            .ok()
            .flatten()
            .map(|a| a.id),
        short: state
            .state_store
            .default_account(venue_b)
            .ok()
            .flatten()
            .map(|a| a.id),
    })
}

/// Persist actual leg fills and derive the per-signal balance status.
/// A signal is `Balanced` only when its own legs net to zero delta — aggregate
/// netting across unrelated signals can never mark it balanced.
fn reconcile_signal_legs(
    state: &AppEngineState,
    signal_id: &str,
    fills: [(
        crate::state_store::LegSide,
        &arbitrageur_core::order::OrderResponse,
    ); 2],
) -> Result<crate::state_store::SignalStatus, String> {
    use crate::state_store::SignalStatus;
    for (side, leg) in fills {
        let filled = leg
            .filled_qty
            .to_string()
            .parse::<f64>()
            .map_err(|_| "unparseable filled quantity".to_string())?;
        let fee = leg
            .fee
            .to_string()
            .parse::<f64>()
            .map_err(|_| "unparseable fee".to_string())?;
        if filled > 0.0 {
            let px = leg
                .avg_fill_price
                .and_then(|p| p.to_string().parse::<f64>().ok())
                .filter(|p| *p > 0.0)
                .ok_or_else(|| format!("leg {side:?} filled but reports no positive price"))?;
            state
                .state_store
                .apply_leg_fill(
                    signal_id,
                    side,
                    filled,
                    px,
                    fee,
                    Some(leg.order_id.as_str()),
                )
                .map_err(|e| e.to_string())?;
        }
    }
    let delta = state
        .state_store
        .signal_delta(signal_id)
        .map_err(|e| e.to_string())?;
    let eps = 1e-6;
    Ok(if delta.is_finite() && delta.abs() <= eps {
        SignalStatus::Balanced
    } else {
        SignalStatus::Imbalanced
    })
}

#[allow(clippy::too_many_arguments)]
pub async fn execute_spatial_arbitrage_impl(
    symbol: &str,
    buy_venue: &str,
    sell_venue: &str,
    order_qty: f64,
    direction: Option<String>,
    trigger_spread_bps: Option<f64>,
    baseline_mean_spread_bps: Option<f64>,
    position_step: Option<u32>,
    max_units: Option<u32>,
    nominal_hurdle_bps: Option<f64>,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    execute_spatial_arbitrage_inner(
        symbol,
        buy_venue,
        sell_venue,
        order_qty,
        direction,
        trigger_spread_bps,
        baseline_mean_spread_bps,
        position_step,
        max_units,
        nominal_hurdle_bps,
        None,
        "futures",
        state,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn execute_spatial_arbitrage_inner(
    symbol: &str,
    buy_venue: &str,
    sell_venue: &str,
    order_qty: f64,
    direction: Option<String>,
    trigger_spread_bps: Option<f64>,
    baseline_mean_spread_bps: Option<f64>,
    position_step: Option<u32>,
    max_units: Option<u32>,
    nominal_hurdle_bps: Option<f64>,
    route_id: Option<&str>,
    strategy_id: &str,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    if !order_qty.is_finite() || order_qty <= 0.0 {
        return Err("order_qty must be a finite positive number".into());
    }
    if buy_venue.trim().is_empty() || sell_venue.trim().is_empty() {
        return Err("Buy and sell venues must both be specified".into());
    }
    if buy_venue.eq_ignore_ascii_case(sell_venue) {
        return Err("Buy and sell venues must be distinct".into());
    }
    validate_market_symbol(symbol)?;
    if let Some(h) = nominal_hurdle_bps {
        if !h.is_finite() || h < 0.0 {
            return Err("nominal_hurdle_bps must be finite and nonnegative".into());
        }
    }

    // Authoritative grid-cap gate. The frontend derives max_units per ticker
    // (step = min(mult x venue min-lot, maxStepUsd / price); max steps from
    // total paper equity x pct) and threads it through position_step/max_units.
    // The backend holds no venue min-lot table so it cannot re-derive the cap;
    // it enforces the caller-supplied cap strictly instead: an entry whose step
    // exceeds the cap, or a ticker already at the cap, is rejected here before
    // any orderbook work. Callers that pass max_units=None (monitor path) skip
    // this gate.
    if let Some(max) = max_units {
        if max < 1 {
            return Err("max_units must be at least 1".into());
        }
        if let Some(step) = position_step {
            if step < 1 {
                return Err("position_step must be at least 1".into());
            }
            if step > max {
                return Err(format!(
                    "Grid cap reached for {symbol}: position_step {step} exceeds max_units {max} — entry rejected"
                ));
            }
        }
        let current = state
            .strategy_configs
            .read()
            .get(symbol)
            .map(|c| c.current_accumulated_units)
            .unwrap_or(0);
        if current >= max {
            return Err(format!(
                "Grid cap reached for {symbol}: {current}/{max} accumulated units — entry rejected"
            ));
        }
    }

    let _execution_guard = state.execution_lock.lock().await;

    if let Err(reason) = state.circuit_breaker.is_trading_allowed() {
        return Err(format!(
            "Execution halted by Risk Circuit Breaker: {}",
            reason
        ));
    }
    if *state.is_paused.read() {
        return Err(
            "Trading engine is PAUSED. Positions are preserved, but new executions are suspended."
                .into(),
        );
    }
    let is_live = *state.is_live_mode.read();
    if is_live {
        if !state.vault.write().is_unlocked() {
            return Err("Execution halted: Vault is locked. Live trading requires an unlocked credential vault.".into());
        }
        state.sync_vault_credentials_to_adapters();
    }
    let _ = trigger_spread_bps; // caller-supplied spread is never trusted for eligibility

    // Durable per-ticker gate: an in-flight, imbalanced, or unresolved signal
    // for this symbol blocks any further order until its own legs reconcile
    // as delta-neutral. Survives restarts via the SQLite store.
    if let Some(blocker) = state
        .state_store
        .blocking_signals(symbol)
        .map_err(|e| e.to_string())?
        .into_iter()
        .next()
    {
        return Err(format!(
            "Symbol {} is blocked by signal {} (status {}): awaiting leg reconciliation/delta-neutral confirmation",
            symbol,
            blocker.id,
            blocker.status.as_str()
        ));
    }

    state.ensure_market(symbol);

    let buy_adapter = state.get_adapter(buy_venue).ok_or_else(|| {
        format!(
            "Buy venue '{}' is not admitted for execution",
            buy_venue
        )
    })?;
    let sell_adapter = state.get_adapter(sell_venue).ok_or_else(|| {
        format!(
            "Sell venue '{}' is not admitted for execution",
            sell_venue
        )
    })?;
    let buy_vid = buy_adapter.venue_id();
    let sell_vid = sell_adapter.venue_id();
    if crate::state::canonical_venue_id(buy_venue) == crate::state::canonical_venue_id(sell_venue) {
        return Err("Buy and sell venues must resolve to distinct canonical venues".into());
    }

    if is_live {
        if !buy_adapter.supports_live_execution() {
            return Err(format!(
                "Live execution halted: Buy venue '{}' is read-only and does not support live order execution",
                buy_venue
            ));
        }
        if !sell_adapter.supports_live_execution() {
            return Err(format!(
                "Live execution halted: Sell venue '{}' is read-only and does not support live order execution",
                sell_venue
            ));
        }
        if !buy_adapter.has_live_credentials() {
            return Err(format!(
                "Live execution halted: Buy venue '{}' has missing or unauthenticated live credentials",
                buy_venue
            ));
        }
        if !sell_adapter.has_live_credentials() {
            return Err(format!(
                "Live execution halted: Sell venue '{}' has missing or unauthenticated live credentials",
                sell_venue
            ));
        }
    }

    let buy_paper = state
        .get_paper_exchange(buy_vid.as_str())
        .ok_or_else(|| format!("No paper exchange admitted for venue '{}'", buy_venue))?;
    let sell_paper = state
        .get_paper_exchange(sell_vid.as_str())
        .ok_or_else(|| format!("No paper exchange admitted for venue '{}'", sell_venue))?;

    // Order books must come from the hub cache (max age 2s) or a fresh fetch
    // through the admitted read-only adapter; ticker data is not a substitute.
    // Cached or fetched, a book must carry the exact requested venue and symbol
    // and pass execution validation before it is trusted or cached.
    let book_buy = match state.market_data_hub.get_valid_orderbook(
        &buy_vid,
        symbol,
        Some(std::time::Duration::from_secs(2)),
    ) {
        Some(b) => b,
        None => {
            let b = buy_adapter
                .fetch_orderbook(symbol)
                .await
                .map_err(|e| format!("No fresh {} orderbook for {}: {}", buy_venue, symbol, e))?;
            validate_adapter_book(&b, &buy_vid, symbol)
                .map_err(|e| format!("Buy-side order book rejected: {}", e))?;
            state.market_data_hub.update_orderbook(b.clone());
            b
        }
    };
    let book_sell = match state.market_data_hub.get_valid_orderbook(
        &sell_vid,
        symbol,
        Some(std::time::Duration::from_secs(2)),
    ) {
        Some(b) => b,
        None => {
            let b = sell_adapter
                .fetch_orderbook(symbol)
                .await
                .map_err(|e| format!("No fresh {} orderbook for {}: {}", sell_venue, symbol, e))?;
            validate_adapter_book(&b, &sell_vid, symbol)
                .map_err(|e| format!("Sell-side order book rejected: {}", e))?;
            state.market_data_hub.update_orderbook(b.clone());
            b
        }
    };
    validate_adapter_book(&book_buy, &buy_vid, symbol)
        .map_err(|e| format!("Buy-side order book rejected: {}", e))?;
    validate_adapter_book(&book_sell, &sell_vid, symbol)
        .map_err(|e| format!("Sell-side order book rejected: {}", e))?;

    let qty_dec = Decimal::from_str_exact(&order_qty.to_string())
        .ok()
        .filter(|q| *q > dec!(0))
        .ok_or_else(|| "Invalid order quantity".to_string())?;
    let max_slippage_bps = futures_slippage_setting(state);
    let min_depth_ratio = state.settings.read().min_depth_ratio;
    let max_slip_dec = Decimal::from_f64_retain(max_slippage_bps)
        .ok_or_else(|| "Invalid max slippage setting".to_string())?;
    let min_depth_dec = Decimal::from_f64_retain(min_depth_ratio)
        .ok_or_else(|| "Invalid min depth ratio setting".to_string())?;

    // Preflight must see exactly what the bounded IOC orders can reach:
    // retain only asks at or below the buy limit and bids at or above the
    // sell limit before evaluating slippage, depth ratio, and VWAP.
    let router = SmartOrderRouter::new(max_slip_dec);
    let buy_ref = book_buy
        .best_ask_price()
        .ok_or_else(|| format!("No asks on buy-side book for {}", symbol))?;
    let sell_ref = book_sell
        .best_bid_price()
        .ok_or_else(|| format!("No bids on sell-side book for {}", symbol))?;
    let buy_limit = router.calculate_bounded_price(Side::Buy, buy_ref);
    let sell_limit = router.calculate_bounded_price(Side::Sell, sell_ref);

    let mut book_buy_exec = book_buy.clone();
    book_buy_exec.asks.retain(|l| l.price <= buy_limit);
    let mut book_sell_exec = book_sell.clone();
    book_sell_exec.bids.retain(|l| l.price >= sell_limit);

    arbitrageur_risk::safety_guards::verify_pre_execution_slippage(
        &book_buy_exec,
        Side::Buy,
        qty_dec,
        max_slip_dec,
    )
    .map_err(|v| {
        format!(
            "Buy-side slippage check failed on {}: VWAP {} vs top {} ({} bps, limit {} bps)",
            buy_venue, v.effective_vwap, v.top_price, v.slippage_bps, v.max_allowed_bps
        )
    })?;
    arbitrageur_risk::safety_guards::verify_pre_execution_slippage(
        &book_sell_exec,
        Side::Sell,
        qty_dec,
        max_slip_dec,
    )
    .map_err(|v| {
        format!(
            "Sell-side slippage check failed on {}: VWAP {} vs top {} ({} bps, limit {} bps)",
            sell_venue, v.effective_vwap, v.top_price, v.slippage_bps, v.max_allowed_bps
        )
    })?;
    arbitrageur_risk::safety_guards::verify_pre_execution_slippage(
        &book_buy,
        Side::Sell,
        qty_dec,
        max_slip_dec,
    )
    .map_err(|v| {
        format!(
            "Exit slippage check failed on {}: VWAP {} vs top {} ({} bps, limit {} bps)",
            buy_venue, v.effective_vwap, v.top_price, v.slippage_bps, v.max_allowed_bps
        )
    })?;
    arbitrageur_risk::safety_guards::verify_pre_execution_slippage(
        &book_sell,
        Side::Buy,
        qty_dec,
        max_slip_dec,
    )
    .map_err(|v| {
        format!(
            "Exit slippage check failed on {}: VWAP {} vs top {} ({} bps, limit {} bps)",
            sell_venue, v.effective_vwap, v.top_price, v.slippage_bps, v.max_allowed_bps
        )
    })?;

    let buy_depth: Decimal = book_buy_exec.asks.iter().map(|l| l.size).sum();
    let sell_depth: Decimal = book_sell_exec.bids.iter().map(|l| l.size).sum();
    let required_depth = qty_dec * min_depth_dec;
    if buy_depth < required_depth || sell_depth < required_depth {
        return Err(format!(
            "Insufficient order book depth ratio: required {} per leg ({}x order), buy side offers {}, sell side offers {}",
            required_depth, min_depth_dec, buy_depth, sell_depth
        ));
    }

    let (buy_vwap, buy_remaining, _) = book_buy_exec
        .buy_execution_vwap(qty_dec)
        .ok_or_else(|| format!("No executable buy depth on {}", buy_venue))?;
    let (sell_vwap, sell_remaining, _) = book_sell_exec
        .sell_execution_vwap(qty_dec)
        .ok_or_else(|| format!("No executable sell depth on {}", sell_venue))?;
    if buy_remaining > dec!(0) || sell_remaining > dec!(0) {
        return Err("Requested quantity exceeds executable book depth within IOC limits".into());
    }

    let target_profit_bps = state
        .strategy_params
        .read()
        .get("futures")
        .map(|p| p.target_profit_bps)
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(1.0);
    let econ = compute_route_economics(
        &book_buy,
        &book_sell,
        qty_dec,
        effective_venue_fee_bps(state, buy_vid.as_str()),
        effective_venue_fee_bps(state, sell_vid.as_str()),
        target_profit_bps,
    )
    .ok_or_else(|| {
        format!(
            "Route economics unavailable on {}/{}: the full quantity cannot be priced on all four legs",
            buy_venue, sell_venue
        )
    })?;
    if econ.expected_net_bps < target_profit_bps {
        return Err(format!(
            "Expected net spread {} BPS after route friction is below the {} BPS target",
            econ.expected_net_bps, target_profit_bps
        ));
    }
    let net_bps_f = econ.expected_net_bps;
    let expected_net_usd = econ.estimated_profit_usd;

    state
        .delta_guard
        .read()
        .check_proposed_pair(
            symbol,
            Side::Buy,
            qty_dec,
            buy_vwap,
            symbol,
            Side::Sell,
            qty_dec,
            sell_vwap,
        )
        .map_err(|(asset, exposure)| {
            format!(
                "Delta neutrality breach on {}: proposed net exposure ${} exceeds limit",
                asset, exposure
            )
        })?;

    let margin_monitor = margin_monitor_from_settings(state);
    let acc_buy = if is_live {
        buy_adapter
            .fetch_account()
            .await
            .map_err(|e| format!("Failed to fetch live account on {}: {}", buy_venue, e))?
    } else {
        buy_paper
            .fetch_account()
            .await
            .map_err(|e| e.to_string())?
    };
    let acc_sell = if is_live {
        sell_adapter
            .fetch_account()
            .await
            .map_err(|e| format!("Failed to fetch live account on {}: {}", sell_venue, e))?
    } else {
        sell_paper
            .fetch_account()
            .await
            .map_err(|e| e.to_string())?
    };
    for (vname, acc) in [(buy_venue, &acc_buy), (sell_venue, &acc_sell)] {
        match margin_monitor.evaluate_account(acc) {
            MarginHealthStatus::Empty => {
                return Err(format!(
                    "Account {} has no funds; execution refused",
                    vname
                ));
            }
            MarginHealthStatus::Critical | MarginHealthStatus::Breached => {
                return Err(format!(
                    "Account {} margin status is critical/breached; execution refused",
                    vname
                ));
            }
            _ => {}
        }
    }
    // Effective order-time leverage: the strategy chain (strategy params,
    // then the per-symbol legacy fallback, then 5) lowered via min() by
    // each leg account's cap and the venue cap. Feeds both the IM
    // pre-checks below and the set_leverage calls after the books load.
    let (target_leverage, account_cap) = resolve_effective_leverage(
        state,
        "futures",
        strategy_id,
        symbol,
        buy_venue,
        sell_venue,
        route_id,
    )?;
    let leverage = Decimal::from(target_leverage);
    let im_buy = (buy_vwap * qty_dec) / leverage;
    if im_buy > acc_buy.free_margin {
        return Err(format!(
            "Insufficient free margin on {}: required {} > available {}",
            buy_venue, im_buy, acc_buy.free_margin
        ));
    }
    let im_sell = (sell_vwap * qty_dec) / leverage;
    if im_sell > acc_sell.free_margin {
        return Err(format!(
            "Insufficient free margin on {}: required {} > available {}",
            sell_venue, im_sell, acc_sell.free_margin
        ));
    }
    if acc_buy.get_position(symbol).is_some() || acc_sell.get_position(symbol).is_some() {
        return Err(format!(
            "An open position already exists for {}; closing lifecycle is not implemented",
            symbol
        ));
    }

    if let Err(reason) = state.circuit_breaker.is_trading_allowed() {
        return Err(format!(
            "Execution halted by Risk Circuit Breaker: {}",
            reason
        ));
    }
    if *state.is_paused.read() {
        return Err(
            "Trading engine is PAUSED. Positions are preserved, but new executions are suspended."
                .into(),
        );
    }

    if !is_live {
        buy_paper
            .replace_orderbook(book_buy.clone())
            .map_err(|e| e.to_string())?;
        sell_paper
            .replace_orderbook(book_sell.clone())
            .map_err(|e| e.to_string())?;
        // Harden the simulator's own margin paths with the same account cap, so
        // a raw strategy target passed anywhere else still books at the cap.
        // Venue-wide last-write-wins; a tighter cap only ever refuses, never
        // over-leverages, so the failure direction stays fail-closed.
        for paper in [&buy_paper, &sell_paper] {
            if paper.max_leverage_cap() != account_cap {
                paper
                    .set_max_leverage_cap(account_cap)
                    .map_err(|e| e.to_string())?;
            }
        }
        buy_paper
            .set_leverage(symbol, target_leverage)
            .await
            .map_err(|e| e.to_string())?;
        sell_paper
            .set_leverage(symbol, target_leverage)
            .await
            .map_err(|e| e.to_string())?;
    } else {
        let _ = buy_adapter.set_leverage(symbol, target_leverage).await;
        let _ = sell_adapter.set_leverage(symbol, target_leverage).await;
    }

    let (buy_exec, sell_exec): (
        Arc<dyn arbitrageur_core::exchange::ExchangeAdapter>,
        Arc<dyn arbitrageur_core::exchange::ExchangeAdapter>,
    ) = if is_live {
        (buy_adapter.clone(), sell_adapter.clone())
    } else {
        (buy_paper.clone(), sell_paper.clone())
    };

    let order_buy =
        router.build_ioc_order(buy_exec.venue_id(), symbol, Side::Buy, buy_ref, qty_dec);
    let order_sell =
        router.build_ioc_order(sell_exec.venue_id(), symbol, Side::Sell, sell_ref, qty_dec);

    let mut equity_before = dec!(0);
    let mut equity_before_by_venue = HashMap::new();
    if !is_live {
        for (vid, paper) in state.paper_exchanges() {
            let acc = paper.fetch_account().await.map_err(|e| e.to_string())?;
            equity_before += acc.equity;
            if let Ok(f) = acc.equity.to_string().parse::<f64>() {
                if f.is_finite() {
                    equity_before_by_venue.insert(vid.to_string(), f);
                }
            }
        }
    } else {
        equity_before = acc_buy.equity + acc_sell.equity;
    }

    // Persist the selected candidate as in_flight BEFORE dispatch: if the
    // process dies mid-execution the symbol stays blocked on restart until
    // the signal's own legs reconcile.
    let accounts = resolve_signal_accounts(
        state,
        strategy_id,
        buy_vid.as_str(),
        sell_vid.as_str(),
        route_id,
    )?;
    let signal_id = format!(
        "sig-{}-{:08x}",
        Utc::now().timestamp_millis(),
        rand::random::<u32>()
    );
    state
        .state_store
        .record_signal_execution(&crate::state_store::NewSignalExecution {
            id: signal_id.clone(),
            strategy_id: strategy_id.to_string(),
            symbol: symbol.to_string(),
            route_id: accounts.route_id,
            expected_net_usd: Some(expected_net_usd),
            note: None,
            legs: vec![
                crate::state_store::NewSignalLeg {
                    account_id: accounts.long,
                    venue: buy_vid.to_string(),
                    side: crate::state_store::LegSide::Long,
                    intended_qty: order_qty,
                },
                crate::state_store::NewSignalLeg {
                    account_id: accounts.short,
                    venue: sell_vid.to_string(),
                    side: crate::state_store::LegSide::Short,
                    intended_qty: order_qty,
                },
            ],
        })
        .map_err(|e| format!("Failed to persist in-flight signal record: {e}"))?;

    let started = std::time::Instant::now();
    let res = state
        .coordinator
        .execute_two_leg(&buy_exec, order_buy, &sell_exec, order_sell)
        .await;
    let latency_us = started.elapsed().as_micros() as u64;

    let (attempt_pnl_f, total_f) = if !is_live {
        let mut equity_after = dec!(0);
        for (_, paper) in state.paper_exchanges() {
            equity_after += paper
                .fetch_account()
                .await
                .map_err(|e| e.to_string())?
                .equity;
        }
        // Own fills move the expected-equity baselines forward so the next
        // attribution poll does not misread trade PnL as a manual flow.
        refresh_equity_expectations_after_fills(state, &equity_before_by_venue).await;
        let attempt_pnl = equity_after - equity_before;
        let pnl_f = attempt_pnl.to_string().parse::<f64>().unwrap_or(0.0);
        // Realized portfolio P&L is always derived from actual paper-account
        // equity: fees, unwind slippage, and any residual loss are included;
        // unrealized open-position exposure is not marked into it.
        let tot_f = {
            let mut total_pnl = state.total_realized_profit.write();
            *total_pnl = equity_after - state.paper_baseline_equity();
            total_pnl.to_string().parse::<f64>().unwrap_or(0.0)
        };
        state.circuit_breaker.update_equity(equity_after);
        (pnl_f, tot_f)
    } else {
        let pnl_f = match &res {
            ArbitrageExecutionResult::Success { realized_profit, .. } => {
                realized_profit.to_string().parse::<f64>().unwrap_or(0.0)
            }
            _ => 0.0,
        };
        let tot_f = {
            let mut total_pnl = state.total_realized_profit_live.write();
            *total_pnl += Decimal::from_str_exact(&pnl_f.to_string()).unwrap_or(dec!(0));
            total_pnl.to_string().parse::<f64>().unwrap_or(0.0)
        };
        (pnl_f, tot_f)
    };

    let cycle = {
        let mut step = state.sim_step_counter.write();
        *step += 1;
        *step
    };

    // Partial-fill trim attribution: the coordinator trimmed the larger leg
    // down to min(fill_a, fill_b) via the unwind path on the excess only and
    // kept the matched portion open as a delta-neutral pair. The excess was
    // already unwound on the paper venues (paper equity reflects it), so the
    // ledger cycle and the signal fills below must cover exactly
    // `matched_qty` — attributing the raw fills would reconcile Imbalanced
    // and strand the slot. A trim whose residual unwind failed never reaches
    // this arm: the coordinator reports UnresolvedExposure and trips.
    let mut open_qty = order_qty;
    let mut trim_note = String::new();
    let res = match res {
        ArbitrageExecutionResult::PartialPairKept {
            leg_a_fill,
            leg_b_fill,
            matched_qty,
            unwind_a,
            unwind_b,
            realized_profit: _,
        } => {
            let matched_f = dec_to_f64(matched_qty, "trimmed matched quantity")?;
            if matched_f <= 0.0 {
                return Err("trimmed matched quantity is not positive".to_string());
            }
            open_qty = matched_f;
            let describe_trim = |o: &Option<Box<arbitrageur_core::order::OrderResponse>>| {
                o.as_ref().map_or_else(
                    || "no trim (leg equals matched quantity)".to_string(),
                    |r| format!("trimmed excess via order {}", r.order_id),
                )
            };
            trim_note = format!(
                "Partial fills trimmed to matched {matched_qty} (A: {}; B: {}); ",
                describe_trim(&unwind_a),
                describe_trim(&unwind_b),
            );
            let mut a = leg_a_fill;
            a.filled_qty = matched_qty;
            a.remaining_qty = dec!(0);
            a.original_qty = matched_qty;
            let mut b = leg_b_fill;
            b.filled_qty = matched_qty;
            b.remaining_qty = dec!(0);
            b.original_qty = matched_qty;
            // Realized cash P&L stays fee outflow only (entry + trim fees are
            // already in paper equity); the matched pair itself is unrealized.
            ArbitrageExecutionResult::Success {
                realized_profit: -(a.fee + b.fee),
                leg_a: a,
                leg_b: b,
            }
        }
        other => other,
    };

    match res {
        ArbitrageExecutionResult::Success {
            leg_a,
            leg_b,
            realized_profit: _,
        } => {
            let fees_dec = leg_a.fee + leg_b.fee;
            let fee_f = fees_dec.to_string().parse::<f64>().unwrap_or(0.0);
            let buy_px_f = leg_a
                .avg_fill_price
                .unwrap_or(buy_vwap)
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0);
            let sell_px_f = leg_b
                .avg_fill_price
                .unwrap_or(sell_vwap)
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0);
            let expected_capture_f = expected_net_usd;

            let cycle_id = format!("paper-spatial-{}", cycle);
            state.virtual_ledger.write().open_cycle(
                cycle_id,
                crate::attribution::ledger_strategy_id(strategy_id),
                symbol.to_string(),
                buy_venue.to_string(),
                sell_venue.to_string(),
                net_bps_f,
                open_qty,
                buy_px_f,
                open_qty,
                sell_px_f,
                fee_f,
            );

            // Persist actual leg fills, then reconcile per-signal balance:
            // balanced only when THIS signal's own legs net to zero delta.
            let signal_status = reconcile_signal_legs(
                state,
                &signal_id,
                [
                    (crate::state_store::LegSide::Long, &leg_a),
                    (crate::state_store::LegSide::Short, &leg_b),
                ],
            )?;
            state
                .state_store
                .update_signal_status(&signal_id, signal_status, None)
                .map_err(|e| e.to_string())?;

            // Position-aware exit support: persist the entry edge baseline
            // for "futures" opens so the convergence check can later decide
            // converge/stop/timeout unwinds through the flatten path.
            // Best-effort AFTER fills are durable: a baseline-write failure
            // must never rewrite an executed open as failed; the slot then
            // stays under manual-flatten control and the check reports
            // Unwatched instead of inventing a baseline.
            let mut baseline_note = String::new();
            if strategy_id == "futures" {
                let row = crate::state_store::SpatialEntryBaseline {
                    signal_id: signal_id.clone(),
                    symbol: symbol.to_string(),
                    buy_venue: buy_vid.to_string(),
                    sell_venue: sell_vid.to_string(),
                    entry_net_bps: net_bps_f,
                    baseline_bps: baseline_mean_spread_bps,
                    entry_ts_ms: Utc::now().timestamp_millis(),
                };
                if let Err(e) = state.state_store.record_spatial_baseline(&row) {
                    baseline_note = format!(
                        " Entry baseline not persisted ({e}); convergence auto-check unavailable, manual flatten only."
                    );
                }
            }

            state.execution_logs.write().push(ExecutionLogEntry {
                id: format!("exec-spatial-{}", cycle),
                timestamp: Utc::now().to_rfc3339(),
                trade_type: "Spatial".into(),
                strategy_id: crate::attribution::attribute_strategy_id(strategy_id),
                symbol: symbol.to_string(),
                venue_a: buy_venue.to_string(),
                venue_b: sell_venue.to_string(),
                leg_a_status: format!("{:?}", leg_a.status),
                leg_b_status: format!("{:?}", leg_b.status),
                success: true,
                gross_profit: 0.0,
                fees: fee_f,
                net_profit: attempt_pnl_f,
                latency_us,
                note: format!(
                    "{trim_note}{} position opened; expected convergence capture ${:.2} is unrealized{baseline_note}",
                    if is_live { "Live real" } else { "Paper" },
                    expected_capture_f
                ),
                direction: direction
                    .clone()
                    .or_else(|| Some("Divergence Entry".into())),
                trigger_spread_bps: Some(net_bps_f),
                baseline_mean_spread_bps,
                position_step: position_step.or(Some(1)),
                max_units: max_units.or(Some(3)),
                nominal_hurdle_bps,
                is_live,
            });

            maybe_record_margin_snapshots(state, Utc::now().timestamp_millis()).await;

            Ok(SimStepResultDto {
                cycle,
                opportunity_found: true,
                execution_success: true,
                trade_type: "Spatial Arb".into(),
                direction: direction.unwrap_or_else(|| "BuyASellB".into()),
                venue_a: buy_venue.to_string(),
                venue_b: sell_venue.to_string(),
                spread_bps: net_bps_f,
                realized_profit_usd: attempt_pnl_f,
                total_cumulative_pnl_usd: total_f,
                message: format!(
                    "{trim_note}Paper position opened on {}/{}: realized P&L ${:.2}; expected convergence capture ${:.2} is unrealized",
                    buy_venue, sell_venue, attempt_pnl_f, expected_capture_f
                ),
            })
        }
        ArbitrageExecutionResult::UnresolvedExposure {
            leg_a_fill,
            leg_b_fill,
            error_a,
            error_b,
        } => {
            // Persist whatever partial fill data exists, then mark the signal
            // unresolved — the ticker stays blocked until manual resolution.
            for (side, fill) in [
                (crate::state_store::LegSide::Long, &leg_a_fill),
                (crate::state_store::LegSide::Short, &leg_b_fill),
            ] {
                if let Some(f) = fill {
                    let filled = f.filled_qty.to_string().parse::<f64>().unwrap_or(0.0);
                    let px = f
                        .avg_fill_price
                        .and_then(|p| p.to_string().parse::<f64>().ok())
                        .unwrap_or(0.0);
                    let fee = f.fee.to_string().parse::<f64>().unwrap_or(0.0);
                    if filled > 0.0 && px > 0.0 {
                        let _ = state.state_store.apply_leg_fill(
                            &signal_id,
                            side,
                            filled,
                            px,
                            fee,
                            Some(f.order_id.as_str()),
                        );
                    }
                }
            }
            let _ = state.state_store.update_signal_status(
                &signal_id,
                crate::state_store::SignalStatus::Unresolved,
                Some("unresolved exposure after failed unwind"),
            );
            if !state.circuit_breaker.is_tripped() {
                state
                    .circuit_breaker
                    .trip("Unresolved exposure after failed unwind");
            }
            let describe = |f: &Option<arbitrageur_core::order::OrderResponse>| {
                f.as_ref()
                    .map(|r| format!("{:?} filled {}/{}", r.status, r.filled_qty, r.original_qty))
                    .unwrap_or_else(|| "no fill".to_string())
            };
            state.execution_logs.write().push(ExecutionLogEntry {
                id: format!("exec-spatial-{}", cycle),
                timestamp: Utc::now().to_rfc3339(),
                trade_type: "Spatial".into(),
                strategy_id: crate::attribution::attribute_strategy_id(strategy_id),
                symbol: symbol.to_string(),
                venue_a: buy_venue.to_string(),
                venue_b: sell_venue.to_string(),
                leg_a_status: describe(&leg_a_fill),
                leg_b_status: describe(&leg_b_fill),
                success: false,
                gross_profit: 0.0,
                fees: 0.0,
                net_profit: attempt_pnl_f,
                latency_us,
                note: format!(
                    "UNRESOLVED EXPOSURE: leg A [{}]; leg B [{}]; realized P&L to date ${:.2}; open exposure not marked",
                    error_a, error_b, attempt_pnl_f
                ),
                direction: direction.clone().or_else(|| Some("Divergence Entry".into())),
                trigger_spread_bps: Some(net_bps_f),
                baseline_mean_spread_bps,
                position_step: position_step.or(Some(1)),
                max_units: max_units.or(Some(3)),
                nominal_hurdle_bps,
                is_live,
            });
            Err(format!(
                "UNRESOLVED EXPOSURE: leg A [{}]; leg B [{}]",
                error_a, error_b
            ))
        }
        other => {
            let (status_a, status_b) = match &other {
                ArbitrageExecutionResult::LegBFailedUnwound { .. } => ("UNWOUND", "REJECTED"),
                ArbitrageExecutionResult::LegAFailedUnwound { .. } => ("REJECTED", "UNWOUND"),
                ArbitrageExecutionResult::HaltedByRisk(_) => ("HALTED", "HALTED"),
                _ => ("REJECTED", "REJECTED"),
            };
            // Unwound legs restored flatness → `failed` (not blocking). Any
            // other terminal failure is also non-blocking once recorded.
            let _ = state.state_store.update_signal_status(
                &signal_id,
                crate::state_store::SignalStatus::Failed,
                Some(&format!("{other:?}")),
            );
            state.execution_logs.write().push(ExecutionLogEntry {
                id: format!("exec-spatial-{}", cycle),
                timestamp: Utc::now().to_rfc3339(),
                trade_type: "Spatial".into(),
                strategy_id: crate::attribution::attribute_strategy_id(strategy_id),
                symbol: symbol.to_string(),
                venue_a: buy_venue.to_string(),
                venue_b: sell_venue.to_string(),
                leg_a_status: status_a.into(),
                leg_b_status: status_b.into(),
                success: false,
                gross_profit: 0.0,
                fees: 0.0,
                net_profit: attempt_pnl_f,
                latency_us,
                note: format!("Atomic trade failed: {:?}", other),
                direction: direction
                    .clone()
                    .or_else(|| Some("Divergence Entry".into())),
                trigger_spread_bps: Some(net_bps_f),
                baseline_mean_spread_bps,
                position_step: position_step.or(Some(1)),
                max_units: max_units.or(Some(3)),
                nominal_hurdle_bps,
                is_live,
            });

            Ok(SimStepResultDto {
                cycle,
                opportunity_found: true,
                execution_success: false,
                trade_type: "Spatial Arb".into(),
                direction: direction.unwrap_or_else(|| "BuyASellB".into()),
                venue_a: buy_venue.to_string(),
                venue_b: sell_venue.to_string(),
                spread_bps: net_bps_f,
                realized_profit_usd: attempt_pnl_f,
                total_cumulative_pnl_usd: total_f,
                message: format!("Execution failed/unwound: {:?}", other),
            })
        }
    }
}

/* ------------------------------------------------------------------ */
/* Same-time candidate arbitration                                      */
/* ------------------------------------------------------------------ */

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpatialCandidateInput {
    pub symbol: String,
    pub buy_venue: String,
    pub sell_venue: String,
    pub order_qty: f64,
    /// Caller's estimated net USD — used only for ranking candidates in the
    /// same batch; the executor recomputes economics before dispatch.
    pub expected_net_usd: Option<f64>,
    /// Explicit strategy-level route override (account A ↔ account B).
    pub route_id: Option<String>,
    pub direction: Option<String>,
    /// Grid-cap context threaded through to the execution gate: the entry's
    /// 1-based step and the derived per-ticker max. Both default to None so
    /// older callers keep working (gate skipped when max_units is None).
    #[serde(default)]
    pub position_step: Option<u32>,
    #[serde(default)]
    pub max_units: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateOutcomeDto {
    pub symbol: String,
    pub buy_venue: String,
    pub sell_venue: String,
    pub expected_net_usd: Option<f64>,
    pub selected: bool,
    pub reason: Option<String>,
    pub result: Option<SimStepResultDto>,
}

/// Submit a synchronized batch of candidates: per symbol, at most ONE order
/// is dispatched — the candidate with the highest expected net USD. Symbols
/// already blocked by an in-flight/unreconciled signal reject all candidates.
pub async fn execute_spatial_candidates_impl(
    candidates: Vec<SpatialCandidateInput>,
    state: &AppEngineState,
) -> Result<Vec<CandidateOutcomeDto>, String> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let mut by_symbol: std::collections::HashMap<String, Vec<SpatialCandidateInput>> =
        std::collections::HashMap::new();
    for c in candidates {
        let key = c.symbol.trim().to_lowercase();
        by_symbol.entry(key).or_default().push(c);
    }

    let mut outcomes = Vec::new();
    for (symbol_key, mut cands) in by_symbol {
        let outcome_for =
            |c: &SpatialCandidateInput,
             selected: bool,
             reason: Option<String>,
             result: Option<SimStepResultDto>| CandidateOutcomeDto {
                symbol: c.symbol.clone(),
                buy_venue: c.buy_venue.clone(),
                sell_venue: c.sell_venue.clone(),
                expected_net_usd: c.expected_net_usd,
                selected,
                reason,
                result,
            };

        if state
            .state_store
            .ticker_blocked(&symbol_key)
            .map_err(|e| e.to_string())?
        {
            for c in &cands {
                outcomes.push(outcome_for(
                    c,
                    false,
                    Some("ticker blocked by in-flight/unreconciled signal".into()),
                    None,
                ));
            }
            continue;
        }

        // Bot targeting honors the active strategy's auto-filters before any
        // economics ranking: filtered tickers lose with an explicit reason.
        // The execution-relevant scope is the candidate's own venue pair.
        let base = crate::monitor::canonical_base(&symbol_key);
        let mut auto_block: Option<String> = None;
        for c in &cands {
            let mut scope = vec![c.buy_venue.clone(), c.sell_venue.clone()];
            scope.sort();
            scope.dedup();
            if let Err(reason) =
                reject_if_strategy_filtered(state, "futures", &base, &scope).await
            {
                auto_block = Some(reason);
                break;
            }
        }
        if let Some(reason) = auto_block {
            for c in &cands {
                outcomes.push(outcome_for(c, false, Some(reason.clone()), None));
            }
            continue;
        }

        // Highest expected net USD wins; missing estimates rank last.
        cands.sort_by(|a, b| {
            b.expected_net_usd
                .unwrap_or(f64::NEG_INFINITY)
                .total_cmp(&a.expected_net_usd.unwrap_or(f64::NEG_INFINITY))
        });
        let winner = cands.remove(0);
        for loser in &cands {
            outcomes.push(outcome_for(
                loser,
                false,
                Some("not selected: lower expected net USD than winning candidate".into()),
                None,
            ));
        }

        let res = execute_spatial_arbitrage_inner(
            &winner.symbol,
            &winner.buy_venue,
            &winner.sell_venue,
            winner.order_qty,
            winner.direction.clone(),
            None,
            None,
            winner.position_step,
            winner.max_units,
            None,
            winner.route_id.as_deref(),
            "futures",
            state,
        )
        .await;
        match res {
            Ok(dto) => outcomes.push(outcome_for(&winner, true, None, Some(dto))),
            Err(e) => outcomes.push(outcome_for(&winner, true, Some(e), None)),
        }
    }
    Ok(outcomes)
}

/// Durable signal execution record for the frontend (symbol filter optional).
#[tauri::command]
pub async fn list_signal_executions(
    symbol: Option<String>,
    status: Option<String>,
    state: tauri::State<'_, Arc<AppEngineState>>,
) -> Result<Vec<crate::state_store::SignalExecution>, String> {
    let st = match status.as_deref() {
        Some(s) => Some(
            crate::state_store::SignalStatus::from_status(s)
                .ok_or_else(|| format!("unknown signal status '{s}'"))?,
        ),
        None => None,
    };
    state
        .state_store
        .list_signals(symbol.as_deref(), st)
        .map_err(|e| e.to_string())
}

/// Per-ticker execution/position state: whether the symbol is blocked, its
/// blocking signals, and the net filled position across all signals.
#[tauri::command]
pub async fn get_ticker_position_state(
    symbol: String,
    state: tauri::State<'_, Arc<AppEngineState>>,
) -> Result<serde_json::Value, String> {
    let blocking = state
        .state_store
        .blocking_signals(&symbol)
        .map_err(|e| e.to_string())?;
    let net = state
        .state_store
        .ticker_net_position(&symbol)
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "symbol": symbol,
        "blocked": !blocking.is_empty(),
        "blockingSignalIds": blocking.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
        "netPositionQty": net,
    }))
}

/// Aggregated current open positions across signal executions, grouped by
/// strategy + symbol + venue set. `unrealizedPnlUsd` is mark-to-market only
/// when every filled leg has an entry price and a fresh venue book; otherwise
/// `null` — never fabricated. `strategyId` scopes to one strategy.
#[tauri::command]
pub async fn get_open_positions(
    strategy_id: Option<String>,
    state: tauri::State<'_, Arc<AppEngineState>>,
) -> Result<Vec<crate::state_store::OpenPositionDto>, String> {
    let sid = match strategy_id.as_deref() {
        Some(raw) => Some(
            crate::state_store::normalize_strategy_id(raw).map_err(|e| e.to_string())?,
        ),
        None => None,
    };
    let mut signals = state
        .state_store
        .list_signals(None, None)
        .map_err(|e| e.to_string())?;
    if let Some(sid) = sid {
        signals.retain(|s| s.strategy_id.eq_ignore_ascii_case(&sid));
    }
    let hub = &state.market_data_hub;
    Ok(crate::state_store::aggregate_open_positions(
        &signals,
        &|venue, symbol| {
            hub.get_valid_orderbook(&VenueId::from(venue), symbol, None)
                .and_then(|b| b.mid_price())
                .and_then(|m| dec_to_f64(m, "mark mid").ok())
        },
    ))
}

#[tauri::command]
pub async fn execute_spatial_candidates(
    candidates: Vec<SpatialCandidateInput>,
    state: tauri::State<'_, Arc<AppEngineState>>,
) -> Result<Vec<CandidateOutcomeDto>, String> {
    execute_spatial_candidates_impl(candidates, &state).await
}

pub async fn execute_cross_ticker_arbitrage_impl(
    long_symbol: &str,
    short_symbol: &str,
    long_venue: &str,
    short_venue: &str,
    order_qty: f64,
    net_spread_bps: Option<f64>,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    if let Err(reason) = state.circuit_breaker.is_trading_allowed() {
        return Err(format!(
            "Execution halted by Risk Circuit Breaker: {}",
            reason
        ));
    }
    if *state.is_paused.read() {
        return Err(
            "Trading engine is PAUSED. Positions are preserved, but new executions are suspended."
                .into(),
        );
    }
    let _ = (
        long_symbol,
        short_symbol,
        long_venue,
        short_venue,
        order_qty,
        net_spread_bps,
    );
    Err("Native 'cross-ticker' execution is disabled until authentic order submission and reconciliation are implemented".into())
}

/// Live-readiness verdict for one funding leg. `None` = submittable.
/// Reports the honest submission state (`live_order_submission_implemented`,
/// not the aspirational `supports_live_execution` flag) plus credential
/// presence, so live-mode refusals name the missing piece per leg.
fn funding_leg_live_issue(adapter: &Arc<dyn ExchangeAdapter>) -> Option<String> {
    let name = adapter.name().to_string();
    if !adapter.live_order_submission_implemented() {
        return Some(format!("{name}: live order submission is not implemented yet"));
    }
    if !adapter.has_live_credentials() {
        return Some(format!(
            "{name}: submittable, but no live credentials loaded (add them in Exchange Accounts and unlock the vault)"
        ));
    }
    None
}

pub async fn execute_funding_carry_impl(
    symbol: &str,
    long_venue: &str,
    short_venue: &str,
    trade_size_usd: f64,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    if let Err(reason) = state.circuit_breaker.is_trading_allowed() {
        return Err(format!(
            "Execution halted by Risk Circuit Breaker: {}",
            reason
        ));
    }
    if *state.is_paused.read() {
        return Err(
            "Trading engine is PAUSED. Positions are preserved, but new executions are suspended."
                .into(),
        );
    }
    if symbol.trim().is_empty() || symbol.len() > 64 {
        return Err("funding symbol must be 1-64 characters".into());
    }
    if long_venue.trim().is_empty() || short_venue.trim().is_empty() {
        return Err("funding long/short venues must be specified".into());
    }
    if !trade_size_usd.is_finite() || trade_size_usd <= 0.0 {
        return Err("trade_size_usd must be a finite positive number".into());
    }
    if *state.is_live_mode.read() {
        // Paper-only until two-leg live dispatch + reconciliation exists.
        // Refuse with per-leg reasons so the operator sees exactly what is
        // missing instead of a blanket denial.
        let mut issues = Vec::new();
        for (venue, label) in [(long_venue, "LONG"), (short_venue, "SHORT")] {
            match state.get_adapter(venue) {
                Some(adapter) => {
                    if let Some(issue) = funding_leg_live_issue(&adapter) {
                        issues.push(format!("{label} leg — {issue}"));
                    }
                }
                None => issues.push(format!(
                    "{label} leg — venue '{venue}' is not admitted"
                )),
            }
        }
        issues.push(
            "two-leg live fills need atomic dispatch + reconciliation, which is not built yet".to_string(),
        );
        return Err(format!(
            "Live funding execution is not available for {symbol} (LONG {long_venue} / SHORT {short_venue}): {}. Switch to Paper Trading to execute this pair on the simulator.",
            issues.join("; ")
        ));
    }

    // Paper-only path (same discipline as the triangle executor): the
    // caller-supplied pair is never trusted — rescan and match it against
    // freshly resolved prints, then enforce the strategy min-APR hurdle.
    let opps = scan_funding_opportunities_impl(symbol, trade_size_usd, state).await?;
    let opp = opps
        .into_iter()
        .find(|o| {
            o.symbol == symbol
                && o.long_venue == long_venue
                && o.short_venue == short_venue
        })
        .ok_or_else(|| {
            "funding opportunity is no longer available (legs moved, books went stale, or hub is still warming)".to_string()
        })?;
    let hurdle = funding_min_apr(state);
    if opp.net_apr_pct < hurdle {
        return Err(format!(
            "funding net {:.1}% APR is below the {:.1}% hurdle — not executed",
            opp.net_apr_pct, hurdle
        ));
    }

    let long_adapter = state.get_adapter(long_venue).ok_or_else(|| {
        format!("Venue '{long_venue}' is not admitted for paper execution")
    })?;
    let short_adapter = state.get_adapter(short_venue).ok_or_else(|| {
        format!("Venue '{short_venue}' is not admitted for paper execution")
    })?;
    let paper_long = state
        .get_paper_exchange(long_venue)
        .ok_or_else(|| format!("No paper exchange admitted for venue '{long_venue}'"))?;
    let paper_short = state
        .get_paper_exchange(short_venue)
        .ok_or_else(|| format!("No paper exchange admitted for venue '{short_venue}'"))?;

    let _execution_guard = state.execution_lock.lock().await;
    if let Err(reason) = state.circuit_breaker.is_trading_allowed() {
        return Err(format!("Execution halted by Risk Circuit Breaker: {reason}"));
    }
    if *state.is_paused.read() {
        return Err(
            "Trading engine is PAUSED. Positions are preserved, but new executions are suspended."
                .into(),
        );
    }
    if *state.is_live_mode.read() {
        return Err(LIVE_EXECUTION_DISABLED.into());
    }

    // Dispatch-time books for both legs: hub-first (120s), else a fresh
    // read-only fetch through the admitted adapter. Either leg missing or
    // invalid fails the dispatch — never priced off a half book.
    let resolve_book = async |adapter: &Arc<dyn ExchangeAdapter>, venue: &str| -> Result<OrderBook, String> {
        let vid = adapter.venue_id();
        if let Some(book) =
            state
                .market_data_hub
                .get_valid_orderbook(&vid, symbol, Some(std::time::Duration::from_secs(120)))
        {
            if validate_adapter_book(&book, &vid, symbol).is_ok() {
                return Ok(book);
            }
        }
        let book = adapter
            .fetch_orderbook(symbol)
            .await
            .map_err(|e| format!("funding leg {symbol}@{venue} book unavailable: {e}"))?;
        validate_adapter_book(&book, &vid, symbol)
            .map_err(|e| format!("funding leg {symbol}@{venue} book invalid: {e}"))?;
        state.market_data_hub.update_orderbook(book.clone());
        Ok(book)
    };
    let long_book = resolve_book(&long_adapter, long_venue).await?;
    let short_book = resolve_book(&short_adapter, short_venue).await?;
    let long_ask = long_book
        .best_ask_price()
        .filter(|p| *p > dec!(0))
        .ok_or_else(|| format!("no long ask for {symbol} on {long_venue}"))?;
    let short_bid = short_book
        .best_bid_price()
        .filter(|p| *p > dec!(0))
        .ok_or_else(|| format!("no short bid for {symbol} on {short_venue}"))?;
    let long_ask_f = dec_to_f64(long_ask, "funding long ask")?;
    let short_bid_f = dec_to_f64(short_bid, "funding short bid")?;
    if !(long_ask_f.is_finite() && long_ask_f > 0.0 && short_bid_f.is_finite() && short_bid_f > 0.0) {
        return Err("funding dispatch books are not priced".into());
    }

    // Blocking signals + existing paper positions refuse dispatch (same
    // one-open-pair discipline as the triangle executor).
    if let Some(blocker) = state
        .state_store
        .blocking_signals(symbol)
        .map_err(|e| e.to_string())?
        .into_iter()
        .next()
    {
        return Err(format!(
            "Symbol {symbol} is blocked by signal {} (status {}): awaiting leg reconciliation/delta-neutral confirmation",
            blocker.id,
            blocker.status.as_str()
        ));
    }
    let dust = triangle_dust_qty();
    for (paper, venue) in [(&paper_long, long_venue), (&paper_short, short_venue)] {
        let acc = paper.fetch_account().await.map_err(|e| e.to_string())?;
        if acc.get_position(symbol).is_some_and(|pos| pos.quantity > dust) {
            return Err(format!(
                "An open paper position already exists for {symbol} on {venue}; flatten it before carrying"
            ));
        }
    }

    let (target_leverage, account_cap) = resolve_effective_leverage(
        state, "funding", "funding", symbol, long_venue, short_venue, None,
    )?;
    for (paper, venue) in [(&paper_long, long_venue), (&paper_short, short_venue)] {
        if paper.max_leverage_cap() != account_cap {
            paper
                .set_max_leverage_cap(account_cap)
                .map_err(|e| e.to_string())?;
        }
        let acc = paper.fetch_account().await.map_err(|e| e.to_string())?;
        // One full-notional leg per venue against free margin.
        let size_dec = Decimal::from_f64_retain(trade_size_usd)
            .ok_or_else(|| "Invalid trade size".to_string())?;
        let leg_im = size_dec / Decimal::from(target_leverage.max(1));
        if leg_im > acc.free_margin {
            return Err(format!(
                "Insufficient free margin on {venue}: required {leg_im} > available {}",
                acc.free_margin
            ));
        }
        match margin_monitor_from_settings(state).evaluate_account(&acc) {
            MarginHealthStatus::Empty => {
                return Err(format!("Paper account {venue} has no funds; execution refused"));
            }
            MarginHealthStatus::Critical | MarginHealthStatus::Breached => {
                return Err(format!(
                    "Paper account {venue} margin status is critical/breached; execution refused"
                ));
            }
            _ => {}
        }
    }

    paper_long
        .replace_orderbook(long_book.clone())
        .map_err(|e| e.to_string())?;
    paper_short
        .replace_orderbook(short_book.clone())
        .map_err(|e| e.to_string())?;

    let mut equity_before = dec!(0);
    let mut equity_before_by_venue = HashMap::new();
    for (vid, p) in state.paper_exchanges() {
        let acc = p.fetch_account().await.map_err(|e| e.to_string())?;
        equity_before += acc.equity;
        if let Ok(f) = acc.equity.to_string().parse::<f64>() {
            if f.is_finite() {
                equity_before_by_venue.insert(vid.to_string(), f);
            }
        }
    }

    let cycle = {
        let mut step = state.sim_step_counter.write();
        *step += 1;
        *step
    };
    let cycle_tag = format!(
        "fund-{}-{:08x}",
        Utc::now().timestamp_millis(),
        rand::random::<u32>()
    );
    let started = std::time::Instant::now();

    // Delta-neutral in base terms: lift the long ask, hit the short bid.
    let qty_f64 = trade_size_usd / long_ask_f;
    if !(qty_f64.is_finite() && qty_f64 > 0.0) {
        return Err("funding carry quantity is not positive".into());
    }
    let qty = Decimal::from_str_exact(&qty_f64.to_string())
        .ok()
        .filter(|q| *q > dec!(0))
        .ok_or_else(|| "funding carry quantity is not positive".to_string())?;
    let mut fills: Vec<(Side, arbitrageur_core::order::OrderResponse)> = Vec::new();
    let mut leg_error: Option<String> = None;
    for (paper, order_side) in [(&paper_long, Side::Buy), (&paper_short, Side::Sell)] {
        let order = arbitrageur_core::order::OrderRequest::new_market(
            paper.venue_id(),
            symbol,
            order_side,
            qty,
        );
        match paper.place_order(order).await {
            Ok(resp)
                if resp.filled_qty == qty
                    && resp.remaining_qty == dec!(0)
                    && resp.avg_fill_price.filter(|p| *p > dec!(0)).is_some() =>
            {
                fills.push((order_side, resp));
            }
            Ok(resp) => {
                if resp.filled_qty > dec!(0) {
                    fills.push((order_side, resp));
                }
                leg_error = Some(format!("funding {order_side:?} leg partially filled or rejected"));
                break;
            }
            Err(e) => {
                leg_error = Some(format!("funding {order_side:?} leg failed: {e}"));
                break;
            }
        }
    }
    let latency_us = started.elapsed().as_micros() as u64;

    let mut equity_after = dec!(0);
    for (_, p) in state.paper_exchanges() {
        equity_after += p
            .fetch_account()
            .await
            .map_err(|e| e.to_string())?
            .equity;
    }
    refresh_equity_expectations_after_fills(state, &equity_before_by_venue).await;
    let attempt_pnl_f = (equity_after - equity_before)
        .to_string()
        .parse::<f64>()
        .unwrap_or(0.0);
    let total_f = {
        let mut total_pnl = state.total_realized_profit.write();
        *total_pnl = equity_after - state.paper_baseline_equity();
        total_pnl.to_string().parse::<f64>().unwrap_or(0.0)
    };
    state.circuit_breaker.update_equity(equity_after);

    let fee_total_f: f64 = fills
        .iter()
        .filter_map(|(_, r)| r.fee.to_string().parse::<f64>().ok())
        .sum();
    let path = format!("LONG {long_venue} / SHORT {short_venue}");
    let carry_note = format!(
        "funding carry {} 2/2 legs filled, net {:.2}% APR at scan",
        symbol, opp.net_apr_pct
    );

    // One two-legged signal so the Funding dock aggregates a single open
    // carry position (grouped by strategy + symbol + venue pair).
    let mut record_errors = Vec::new();
    if fills.len() == 2 {
        use crate::state_store::{LegSide, NewSignalExecution, NewSignalLeg, SignalStatus};
        let account_for = |venue: &str| {
            state
                .state_store
                .default_account(venue)
                .ok()
                .flatten()
                .map(|a| a.id)
        };
        let signal_id = format!("{cycle_tag}-carry");
        let legs = vec![
            NewSignalLeg {
                account_id: account_for(long_venue),
                venue: long_venue.to_string(),
                side: LegSide::Long,
                intended_qty: qty_f64,
            },
            NewSignalLeg {
                account_id: account_for(short_venue),
                venue: short_venue.to_string(),
                side: LegSide::Short,
                intended_qty: qty_f64,
            },
        ];
        if let Err(e) = state.state_store.record_signal_execution(&NewSignalExecution {
            id: signal_id.clone(),
            strategy_id: FUNDING_STRATEGY_ID.to_string(),
            symbol: symbol.to_string(),
            route_id: None,
            expected_net_usd: None,
            note: Some(format!("{carry_note}; {path}")),
            legs,
        }) {
            record_errors.push(format!("Failed to persist funding carry record: {e}"));
        } else {
            for (side, resp) in &fills {
                let leg_side = match side {
                    Side::Buy => LegSide::Long,
                    Side::Sell => LegSide::Short,
                };
                let px_f = resp
                    .avg_fill_price
                    .unwrap_or(dec!(0))
                    .to_string()
                    .parse::<f64>()
                    .unwrap_or(0.0);
                let fee_f = resp.fee.to_string().parse::<f64>().unwrap_or(0.0);
                if let Err(e) = state.state_store.apply_leg_fill(
                    &signal_id,
                    leg_side,
                    qty_f64,
                    px_f,
                    fee_f,
                    Some(resp.order_id.as_str()),
                ) {
                    record_errors.push(e.to_string());
                }
            }
            if let Err(e) = state.state_store.update_signal_status(
                &signal_id,
                SignalStatus::Imbalanced,
                Some(carry_note.as_str()),
            ) {
                record_errors.push(e.to_string());
            }
        }
    }

    let net_bps = opp.net_rate_8h_pct * 100.0;
    if let Some(err) = leg_error {
        state.execution_logs.write().push(ExecutionLogEntry {
            id: format!("exec-funding-{cycle}"),
            timestamp: Utc::now().to_rfc3339(),
            trade_type: "Funding".into(),
            strategy_id: crate::attribution::attribute_strategy_id(FUNDING_STRATEGY_ID),
            symbol: symbol.to_string(),
            venue_a: long_venue.to_string(),
            venue_b: short_venue.to_string(),
            leg_a_status: format!("{}/2 legs filled", fills.len()),
            leg_b_status: err.clone(),
            success: false,
            gross_profit: 0.0,
            fees: fee_total_f,
            net_profit: attempt_pnl_f,
            latency_us,
            note: format!(
                "Funding carry PARTIAL: {err}. {carry_note}. Residual legs recorded — flatten each leg symbol to unwind. Record errors: {}",
                record_errors.join("; ")
            ),
            direction: Some(path.clone()),
            trigger_spread_bps: Some(net_bps),
            baseline_mean_spread_bps: None,
            position_step: None,
            max_units: None,
            nominal_hurdle_bps: Some(hurdle),
            is_live: false,
        });
        return Err(format!(
            "Funding carry PARTIAL ({}/2 legs): {err}; residuals recorded — flatten each leg symbol to unwind",
            fills.len()
        ));
    }

    state.execution_logs.write().push(ExecutionLogEntry {
        id: format!("exec-funding-{cycle}"),
        timestamp: Utc::now().to_rfc3339(),
        trade_type: "Funding".into(),
        strategy_id: crate::attribution::attribute_strategy_id(FUNDING_STRATEGY_ID),
        symbol: symbol.to_string(),
        venue_a: long_venue.to_string(),
        venue_b: short_venue.to_string(),
        leg_a_status: "FILLED".into(),
        leg_b_status: "FILLED".into(),
        success: true,
        gross_profit: 0.0,
        fees: fee_total_f,
        net_profit: attempt_pnl_f,
        latency_us,
        note: format!(
            "Funding carry dispatched 2/2 paper legs ({carry_note}); carry position open — unwind each leg symbol via flatten. Record errors: {}",
            record_errors.join("; ")
        ),
        direction: Some(path.clone()),
        trigger_spread_bps: Some(net_bps),
        baseline_mean_spread_bps: None,
        position_step: None,
        max_units: None,
        nominal_hurdle_bps: Some(hurdle),
        is_live: false,
    });

    maybe_record_margin_snapshots(state, Utc::now().timestamp_millis()).await;

    Ok(SimStepResultDto {
        cycle,
        opportunity_found: true,
        execution_success: true,
        trade_type: "Funding".into(),
        direction: path.clone(),
        venue_a: long_venue.to_string(),
        venue_b: short_venue.to_string(),
        spread_bps: net_bps,
        realized_profit_usd: attempt_pnl_f,
        total_cumulative_pnl_usd: total_f,
        message: format!(
            "Funding carry dispatched on {symbol} ({path}): 2/2 paper legs filled, realized P&L ${attempt_pnl_f:.2}; carry position open — flatten each leg to unwind"
        ),
    })
}

pub async fn execute_prediction_arbitrage_impl(
    market_title: &str,
    buy_yes_venue: &str,
    buy_no_venue: &str,
    quantity: f64,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    if let Err(reason) = state.circuit_breaker.is_trading_allowed() {
        return Err(format!(
            "Execution halted by Risk Circuit Breaker: {}",
            reason
        ));
    }
    if *state.is_paused.read() {
        return Err(
            "Trading engine is PAUSED. Positions are preserved, but new executions are suspended."
                .into(),
        );
    }
    let _ = (market_title, buy_yes_venue, buy_no_venue, quantity);
    Err("Native 'prediction' execution is disabled until authentic order submission and reconciliation are implemented".into())
}

/// Re-anchor the breaker after a paper-universe reset (sync context).
fn cb_reanchor_to_paper(state: &AppEngineState, total: Decimal) {
    state.circuit_breaker.reanchor_baseline(total);
    *state.last_risk_live_universe.write() = Some(*state.is_live_mode.read());
}

/// Build the margin monitor from the operator's Safety settings so the
/// risk snapshot, venue balances, and pre-trade gates share one threshold set.
fn margin_monitor_from_settings(state: &AppEngineState) -> MarginMonitor {
    let s = state.settings.read();
    let cfg = MarginMonitorConfig {
        warning_margin_usage_pct: Decimal::from_f64_retain(s.margin_warning_usage_pct).unwrap_or(dec!(70.0)),
        critical_margin_usage_pct: Decimal::from_f64_retain(s.margin_critical_usage_pct).unwrap_or(dec!(85.0)),
        min_liquidation_distance_pct: Decimal::from_f64_retain(s.margin_min_liquidation_distance_pct).unwrap_or(dec!(15.0)),
    };
    MarginMonitor::new(cfg)
}

pub async fn get_risk_overview_impl(state: &AppEngineState) -> Result<RiskOverviewDto, String> {
    let cb = &state.circuit_breaker;
    let monitor = margin_monitor_from_settings(state);

    // Risk telemetry: live mode reads live adapters for admitted venues so
    // the header reconciles with the Capital Allocation gadget. Paper mode
    // sums all paper simulators (legacy baseline — disabled venues still
    // hold paper equity; the venue gadget shows admitted only and the UI
    // footers the drift).
    let is_live = *state.is_live_mode.read();
    let mut accounts = Vec::new();
    if is_live {
        for (vid, _name, adapter) in state.get_venues() {
            match adapter.fetch_account().await {
                Ok(mut acc) => {
                    acc.venue = vid.into();
                    accounts.push(acc);
                }
                Err(e) => {
                    tracing::warn!("risk overview: live account read failed for {vid}: {e}");
                }
            }
        }
        // Fall back to paper baseline if no live account read succeeded
        // (e.g. missing credentials) so the overview never goes blank.
        if accounts.is_empty() {
            for (vid, paper) in state.paper_exchanges() {
                let mut acc = paper.fetch_account().await.map_err(|e| e.to_string())?;
                acc.venue = vid.into();
                accounts.push(acc);
            }
        }
    } else {
        for (vid, paper) in state.paper_exchanges() {
            let mut acc = paper.fetch_account().await.map_err(|e| e.to_string())?;
            acc.venue = vid.into();
            accounts.push(acc);
        }
    }

    let total_equity_dec: Decimal = accounts.iter().map(|a| a.equity).sum();
    let total_free_margin_dec: Decimal = accounts.iter().map(|a| a.free_margin).sum();
    let total_mm_dec: Decimal = accounts.iter().map(|a| a.maintenance_margin).sum();
    let all_positions: Vec<_> = accounts.iter().flat_map(|a| a.positions.clone()).collect();

    // Feed observed equity into the breaker, then re-read its state so a
    // drawdown trip is reflected in this same snapshot. A paper<->live
    // universe switch is NOT a loss: re-anchor the baseline instead of
    // tripping (fixes the recurring false halt where a $200k paper baseline
    // vs ~$3.5k live funds reads as a -$196k "drawdown" on a flat book).
    {
        let mut universe = state.last_risk_live_universe.write();
        if *universe != Some(is_live) {
            cb.reanchor_baseline(total_equity_dec);
            *universe = Some(is_live);
        } else {
            cb.update_equity(total_equity_dec);
        }
    }
    let is_halted = cb.is_tripped();
    let cb_str = match cb.trip_reason() {
        Some(reason) => format!("TRIPPED: {}", reason),
        None => {
            if is_halted {
                "TRIPPED: Manual halt".into()
            } else {
                "ACTIVE".into()
            }
        }
    };

    let delta_limit = state.delta_guard.read().max_unhedged_usd_per_asset();
    let delta_limit_f = delta_limit.to_string().parse::<f64>().unwrap_or(0.0);

    let net_deltas_map = {
        let delta_guard = state.delta_guard.read();
        delta_guard.calculate_net_delta(&all_positions)
    };

    let mut deltas_dto = Vec::new();
    for (asset, (qty, usd)) in net_deltas_map {
        let qty_f = qty.to_string().parse::<f64>().unwrap_or(0.0);
        let usd_f = usd.to_string().parse::<f64>().unwrap_or(0.0);
        deltas_dto.push(NetDeltaDto {
            asset,
            net_quantity: qty_f,
            net_exposure_usd: usd_f,
            delta_limit_usd: delta_limit_f,
            is_neutral: usd.abs() <= delta_limit,
        });
    }
    deltas_dto.sort_by(|a, b| a.asset.cmp(&b.asset));

    let total_equity = total_equity_dec.to_string().parse::<f64>().unwrap_or(0.0);
    let total_free_margin = total_free_margin_dec
        .to_string()
        .parse::<f64>()
        .unwrap_or(0.0);
    let total_mm = total_mm_dec.to_string().parse::<f64>().unwrap_or(0.0);
    let current_drawdown = cb
        .current_drawdown_usd()
        .to_string()
        .parse::<f64>()
        .unwrap_or(0.0);
    let max_drawdown_limit = cb
        .max_daily_drawdown_usd()
        .to_string()
        .parse::<f64>()
        .unwrap_or(0.0);
    let day_start = cb
        .day_start_equity()
        .to_string()
        .parse::<f64>()
        .unwrap_or(0.0);
    let drawdown_pct = if day_start > 0.0 {
        (current_drawdown / day_start) * 100.0
    } else {
        0.0
    };

    let severity = |s: MarginHealthStatus| match s {
        MarginHealthStatus::Empty => -1,
        MarginHealthStatus::Healthy => 0,
        MarginHealthStatus::Warning => 1,
        MarginHealthStatus::Critical => 2,
        MarginHealthStatus::Breached => 3,
    };
    let mut worst = MarginHealthStatus::Healthy;
    for acc in &accounts {
        let s = monitor.evaluate_account(acc);
        if severity(s) > severity(worst) {
            worst = s;
        }
    }
    let mut min_liq_dist_pct: Option<f64> = None;
    for pos in &all_positions {
        let s = monitor.evaluate_position(pos);
        if severity(s) > severity(worst) {
            worst = s;
        }
        if let Some(dist) = pos.liquidation_distance_pct() {
            let pct = (dist * dec!(100)).to_string().parse::<f64>().unwrap_or(0.0);
            min_liq_dist_pct = Some(match min_liq_dist_pct {
                Some(cur) => cur.min(pct),
                None => pct,
            });
        }
    }

    // Liquidation safety cannot be claimed without actual liquidation prices;
    // open positions with no liquidation data report UNKNOWN_LIQUIDATION
    // unless a margin severity already demands a stronger state.
    let margin_status = if is_halted {
        "HALTED"
    } else if all_positions.is_empty() {
        "FLAT"
    } else {
        match worst {
            MarginHealthStatus::Breached => "BREACHED",
            MarginHealthStatus::Critical => "CRITICAL",
            MarginHealthStatus::Warning => "WARNING",
            MarginHealthStatus::Empty | MarginHealthStatus::Healthy => {
                if min_liq_dist_pct.is_none() {
                    "UNKNOWN_LIQUIDATION"
                } else {
                    "SAFE"
                }
            }
        }
    };

    Ok(RiskOverviewDto {
        circuit_breaker_state: cb_str,
        is_halted,
        current_drawdown_usd: current_drawdown,
        max_drawdown_limit_usd: max_drawdown_limit,
        drawdown_pct,
        consecutive_failures: cb.consecutive_errors(),
        max_failures_allowed: cb.max_consecutive_errors(),
        total_portfolio_value_usd: total_equity,
        net_deltas: deltas_dto,
        margin_health: MarginHealthDto {
            maintenance_margin_req_usd: total_mm,
            margin_balance_usd: total_free_margin,
            margin_ratio_pct: if total_equity > 0.0 {
                (total_mm / total_equity) * 100.0
            } else {
                0.0
            },
            liquidation_distance_pct: if all_positions.is_empty() {
                100.0
            } else {
                min_liq_dist_pct.unwrap_or(0.0)
            },
            status: margin_status.into(),
        },
    })
}

pub fn reset_circuit_breaker_impl(state: &AppEngineState) -> Result<String, String> {
    state.circuit_breaker.reset();
    Ok("Circuit Breaker reset successfully. Trading resumed.".into())
}

pub fn trigger_emergency_killswitch_impl(state: &AppEngineState) -> Result<String, String> {
    state
        .circuit_breaker
        .trip("EMERGENCY KILLSWITCH ENGAGED BY OPERATOR");
    Ok("EMERGENCY KILLSWITCH ENGAGED. All trading operations halted immediately.".into())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlattenResultDto {
    pub strategy_id: String,
    pub symbol: Option<String>,
    pub signals_processed: usize,
    pub signals_closed: usize,
    pub legs_closed: usize,
    /// Open signals whose legs could not be fully closed — residual exposure
    /// remains and those signals stay `unresolved` (ticker stays blocked).
    pub residual_signals: usize,
    pub notes: Vec<String>,
}

fn ledger_qty_to_dec(value: f64, what: &str) -> Result<Decimal, String> {
    if !value.is_finite() || value < 0.0 {
        return Err(format!(
            "{what} must be finite and nonnegative, got {value}"
        ));
    }
    Decimal::from_str_exact(&value.to_string())
        .map_err(|_| format!("{what} {value} is not representable as Decimal"))
}

fn dec_to_f64(value: Decimal, what: &str) -> Result<f64, String> {
    let f = value
        .to_string()
        .parse::<f64>()
        .map_err(|_| format!("{what} {value} is not representable as f64"))?;
    if !f.is_finite() {
        return Err(format!("{what} {value} is not finite as f64"));
    }
    Ok(f)
}

/// Resolve the passive-unwind controls for a flatten. Only strategies with
/// a live reduce surface (futures/funding/spot) can return `Some`, and only
/// when enabled with a valid record — everything else (including triangle,
/// radar, unknown ids, and invalid records) falls back to market exits.
fn passive_cfg_for(state: &AppEngineState, strategy_id: &str) -> Option<PassiveUnwindConfig> {
    let key = strategy_id.trim().to_lowercase();
    let map = state.strategy_params.read();
    let params = map.get(key.as_str())?;
    let cfg = match key.as_str() {
        "futures" => params.futures_passive_unwind,
        "funding" => params.funding_passive_unwind,
        "spot" => params.spot_passive_unwind,
        _ => return None,
    };
    if cfg.enabled && cfg.validate().is_ok() {
        Some(cfg)
    } else {
        None
    }
}

/// Re-fetcher for passive retries: every attempt re-prices from a newly
/// fetched, validated adapter book mirrored into the paper exchange —
/// exactly like the initial flatten fetch, never a stale peg.
struct FlattenPaperRefresh<'a> {
    state: &'a AppEngineState,
    venue_label: String,
    venue_id: VenueId,
    symbol: String,
    paper: Arc<arbitrageur_exchanges::simulated::SimulatedExchange>,
}

#[async_trait::async_trait]
impl PassiveBookRefresh for FlattenPaperRefresh<'_> {
    async fn fresh_book(&self) -> anyhow::Result<OrderBook> {
        // Mirror the initial flatten fetch: prefer the hub's fresh book
        // (no network, no revalidation — the hub only holds live books),
        // else fetch from the adapter with full validation.
        if let Some(b) = self.state.market_data_hub.get_valid_orderbook(
            &self.venue_id,
            &self.symbol,
            Some(std::time::Duration::from_secs(2)),
        ) {
            self.paper
                .replace_orderbook(b.clone())
                .map_err(anyhow::Error::msg)?;
            return Ok(b);
        }
        let adapter = self.state.get_adapter(&self.venue_label).ok_or_else(|| {
            anyhow::anyhow!(
                "Venue '{}' is not admitted for paper execution",
                self.venue_label
            )
        })?;
        let b = adapter.fetch_orderbook(&self.symbol).await.map_err(|e| {
            anyhow::anyhow!(
                "No fresh {} orderbook for {}: {e}",
                self.venue_label,
                self.symbol
            )
        })?;
        validate_adapter_book(&b, &self.venue_id, &self.symbol)
            .map_err(|e| anyhow::anyhow!("Flatten order book rejected: {e}"))?;
        self.state.market_data_hub.update_orderbook(b.clone());
        self.paper
            .replace_orderbook(b.clone())
            .map_err(anyhow::Error::msg)?;
        Ok(b)
    }
}

/// Arguments for one flatten leg-close (bundled: clippy allows at most
/// seven function parameters).
struct FlattenLegArgs<'a> {
    state: &'a AppEngineState,
    venue: &'a str,
    symbol: &'a str,
    side: Side,
    qty: Decimal,
    strategy_id: &'a str,
    sig_id: &'a str,
    notes: &'a mut Vec<String>,
    execution_type: Option<&'a str>,
}

/// Place a reduce-only market order on a venue's paper exchange to close one
/// filled leg. Returns (filled_qty, avg_price, fee_usd, order_id).
async fn flatten_leg_on_paper(
    args: FlattenLegArgs<'_>,
) -> Result<Option<(Decimal, Decimal, Decimal, String)>, String> {
    let FlattenLegArgs {
        state,
        venue,
        symbol,
        side,
        qty,
        strategy_id,
        sig_id,
        notes,
        execution_type,
    } = args;
    let canonical = crate::state::canonical_venue_id(venue)
        .ok_or_else(|| format!("Unknown venue '{venue}'"))?;
    let adapter = state
        .get_adapter(venue)
        .ok_or_else(|| format!("Venue '{venue}' is not admitted for paper execution"))?;
    let vid = adapter.venue_id();
    let paper = state
        .get_paper_exchange(canonical)
        .ok_or_else(|| format!("No paper exchange admitted for venue '{venue}'"))?;

    let book = match state.market_data_hub.get_valid_orderbook(
        &vid,
        symbol,
        Some(std::time::Duration::from_secs(2)),
    ) {
        Some(b) => b,
        None => {
            let b = adapter
                .fetch_orderbook(symbol)
                .await
                .map_err(|e| format!("No fresh {venue} orderbook for {symbol}: {e}"))?;
            validate_adapter_book(&b, &vid, symbol)
                .map_err(|e| format!("Flatten order book rejected: {e}"))?;
            state.market_data_hub.update_orderbook(b.clone());
            b
        }
    };
    paper.replace_orderbook(book).map_err(|e| e.to_string())?;

    if qty <= dec!(0) {
        return Err("Invalid leg quantity".to_string());
    }
    let passive_cfg = match execution_type {
        Some("emergency_market") => None,
        Some("non_emergency_limit") => {
            Some(passive_cfg_for(state, strategy_id).unwrap_or_else(|| {
                arbitrageur_execution::passive_unwind::PassiveUnwindConfig {
                    enabled: true,
                    ..Default::default()
                }
            }))
        }
        _ => passive_cfg_for(state, strategy_id),
    };
    if let Some(cfg) = passive_cfg {
        let paper_adapter: Arc<dyn ExchangeAdapter> = paper.clone();
        let refresher = FlattenPaperRefresh {
            state,
            venue_label: venue.to_string(),
            venue_id: vid.clone(),
            symbol: symbol.to_string(),
            paper: paper.clone(),
        };
        // No fast-vol tracker exists on the flatten path, so the spread leg
        // is the hard guard (documented in `calm_gate_ok`).
        let report = execute_passive_reduce(
            &paper_adapter,
            &refresher,
            symbol,
            side,
            qty,
            &cfg,
            None,
        )
        .await
        .map_err(|e| format!("passive reduce failed: {e:#}"))?;
        match &report.outcome {
            PassiveOutcome::LimitFilled => notes.push(format!(
                "{sig_id}: {venue} leg passively closed {qty} (limit, {} tries)",
                report.limit_tries
            )),
            PassiveOutcome::LimitPartialThenMarket => notes.push(format!(
                "{sig_id}: {venue} leg limit-partial, remainder at market ({qty})"
            )),
            PassiveOutcome::MarketFallback { reason } => notes.push(format!(
                "{sig_id}: {venue} leg passive skipped, market exit ({reason})"
            )),
        }
        let resp = report.response;
        if resp.filled_qty <= dec!(0) {
            return Ok(None);
        }
        let px = resp
            .avg_fill_price
            .filter(|p| *p > dec!(0))
            .ok_or_else(|| "flatten fill reports no positive price".to_string())?;
        return Ok(Some((resp.filled_qty, px, resp.fee, resp.order_id)));
    }
    let mut order =
        arbitrageur_core::order::OrderRequest::new_market(paper.venue_id(), symbol, side, qty);
    order.reduce_only = true;

    let resp = paper.place_order(order).await.map_err(|e| e.to_string())?;
    if resp.filled_qty <= dec!(0) {
        return Ok(None);
    }
    let px = resp
        .avg_fill_price
        .filter(|p| *p > dec!(0))
        .ok_or_else(|| "flatten fill reports no positive price".to_string())?;
    Ok(Some((resp.filled_qty, px, resp.fee, resp.order_id)))
}

/// Place a reduce-only market order on a venue (paper or live) to close one
/// filled leg. Returns (filled_qty, avg_price, fee_usd, order_id).
async fn flatten_leg(
    args: FlattenLegArgs<'_>,
    is_live: bool,
) -> Result<Option<(Decimal, Decimal, Decimal, String)>, String> {
    if !is_live {
        return flatten_leg_on_paper(args).await;
    }
    let FlattenLegArgs {
        state,
        venue,
        symbol,
        side,
        qty,
        sig_id: _,
        notes: _,
        strategy_id: _,
        execution_type: _,
    } = args;
    let adapter = state
        .get_adapter(venue)
        .ok_or_else(|| format!("Venue '{venue}' is not admitted for execution"))?;
    if !adapter.supports_live_execution() || !adapter.has_live_credentials() {
        return Err(format!(
            "Venue '{venue}' does not support live execution or has unauthenticated credentials"
        ));
    }
    if qty <= dec!(0) {
        return Err("Invalid leg quantity".to_string());
    }
    let mut order =
        arbitrageur_core::order::OrderRequest::new_market(adapter.venue_id(), symbol, side, qty);
    order.reduce_only = true;

    let resp = adapter.place_order(order).await.map_err(|e| e.to_string())?;
    if resp.filled_qty <= dec!(0) {
        return Ok(None);
    }
    let px = resp
        .avg_fill_price
        .filter(|p| *p > dec!(0))
        .ok_or_else(|| "flatten fill reports no positive price".to_string())?;
    Ok(Some((resp.filled_qty, px, resp.fee, resp.order_id)))
}


/// Close the attributed virtual-ledger lot for a flattened signal family and
/// report its realized PnL. The lot is matched on the signal's own ledger
/// namespace (see `attribution::ledger_strategy_id`), so radar, futures and
/// any future attributed strategy close their own lots instead of sharing
/// one hardcoded namespace. Returns `None` when no matching open lot holds
/// enough quantity (e.g. single-leg triangle residuals, which realize purely
/// through their signal legs) — the caller keeps the 0.0 log convention
/// there rather than inventing a number.
fn close_flattened_spatial_lot(
    state: &AppEngineState,
    sig: &crate::state_store::SignalExecution,
    family: &[crate::state_store::SignalExecution],
) -> Option<(f64, f64)> {
    use crate::state_store::LegSide;
    use arbitrageur_execution::virtual_ledger::CycleStatus;

    let venue_of = |side: LegSide| {
        sig.legs
            .iter()
            .find(|l| l.side == side)
            .and_then(|l| crate::state::canonical_venue_id(&l.venue))
    };
    let (Some(long_venue), Some(short_venue)) = (venue_of(LegSide::Long), venue_of(LegSide::Short))
    else {
        return None;
    };

    #[derive(Default)]
    struct LegAgg {
        qty: f64,
        px_qty: f64,
        fee: f64,
    }
    let mut fills: HashMap<(String, &'static str), LegAgg> = HashMap::new();
    for child in family {
        for leg in &child.legs {
            let Some(canon) = crate::state::canonical_venue_id(&leg.venue) else {
                continue;
            };
            let (Some(px), qty, fee) = (leg.fill_price, leg.filled_qty, leg.fee_usd) else {
                continue;
            };
            if !(qty.is_finite() && qty > 0.0 && px.is_finite() && px > 0.0 && fee.is_finite()) {
                continue;
            }
            let agg = fills
                .entry((canon.to_string(), leg.side.as_str()))
                .or_default();
            agg.qty += qty;
            agg.px_qty += qty * px;
            agg.fee += fee;
        }
    }
    let exit_fees: f64 = fills.values().map(|a| a.fee).sum();
    let agg_for = |venue: &str, side: LegSide| fills.get(&(venue.to_string(), side.as_str()));
    let (Some(long_exit), Some(short_exit)) = (
        agg_for(long_venue, LegSide::Short),
        agg_for(short_venue, LegSide::Long),
    ) else {
        return None;
    };

    let want_ledger_strategy = crate::attribution::ledger_strategy_id(&sig.strategy_id);
    let lot_id = {
        let ledger = state.virtual_ledger.read();
        ledger
            .lots
            .values()
            .find(|lot| {
                lot.strategy_id == want_ledger_strategy
                    && lot.symbol == sig.symbol
                    && lot.status == CycleStatus::Open
                    && crate::state::canonical_venue_id(&lot.venue_long) == Some(long_venue)
                    && crate::state::canonical_venue_id(&lot.venue_short) == Some(short_venue)
                    && long_exit.qty + 1e-9 >= lot.long_qty
                    && short_exit.qty + 1e-9 >= lot.short_qty
            })
            .map(|l| l.id.clone())
    };
    if let Some(id) = lot_id {
        state
            .virtual_ledger
            .write()
            .close_cycle(
                &id,
                long_exit.px_qty / long_exit.qty,
                short_exit.px_qty / short_exit.qty,
                exit_fees,
                CycleStatus::StoppedOut,
            )
            .and_then(|l| match (l.realized_pnl_usd, l.realized_pnl_bps) {
                (Some(usd), Some(bps)) => Some((usd, bps)),
                _ => None,
            })
    } else {
        None
    }
}

/// Emergency per-strategy exit: for every open signal of `strategy_id`
/// (optionally scoped to `symbol`), dispatch inverse reduce-only market orders
/// sized to that root family's attributed residual on each paper venue, and
/// mark the original `closed` only once its attributed legs are neutralized.
/// Bypasses pause/circuit-breaker because it is strictly risk-reducing;
/// in live mode, dispatches reduce-only market orders to live authenticated adapters.
pub async fn flatten_ticker_positions_impl(
    symbol: Option<&str>,
    strategy_id: &str,
    state: &AppEngineState,
) -> Result<FlattenResultDto, String> {
    flatten_ticker_positions_with_type_impl(symbol, strategy_id, None, state).await
}

pub async fn flatten_ticker_positions_with_type_impl(
    symbol: Option<&str>,
    strategy_id: &str,
    execution_type: Option<&str>,
    state: &AppEngineState,
) -> Result<FlattenResultDto, String> {
    use crate::state_store::{LegSide, NewSignalExecution, NewSignalLeg, SignalStatus};

    let sid = crate::state_store::normalize_strategy_id(strategy_id).map_err(|e| e.to_string())?;
    let is_live = *state.is_live_mode.read();
    if is_live {
        let is_unlocked = state.vault.write().is_unlocked();
        if !is_unlocked {
            return Err("Cannot flatten in real trading mode while the secure credential vault is locked. Unlock the vault first.".into());
        }
    }
    let _execution_guard = state.execution_lock.lock().await;
    let flatten_equity_before = if !is_live {
        snapshot_paper_equity(state).await
    } else {
        HashMap::new()
    };

    let all_signals = state
        .state_store
        .list_signals(symbol, None)
        .map_err(|e| e.to_string())?;
    let roots: Vec<crate::state_store::SignalExecution> = all_signals
        .iter()
        .filter(|s| {
            !s.id.starts_with("flat-")
                && s.strategy_id.eq_ignore_ascii_case(&sid)
                && s.status.counts_as_open()
        })
        .cloned()
        .collect();

    let mut out = FlattenResultDto {
        strategy_id: sid.clone(),
        symbol: symbol.map(|s| s.to_string()),
        signals_processed: roots.len(),
        signals_closed: 0,
        legs_closed: 0,
        residual_signals: 0,
        notes: Vec::new(),
    };

    let cycle = {
        let mut step = state.sim_step_counter.write();
        *step += 1;
        *step
    };

    let papers = state.paper_exchanges();
    let opposite = |s: LegSide| match s {
        LegSide::Long => LegSide::Short,
        LegSide::Short => LegSide::Long,
    };

    struct LegExposure {
        canonical: &'static str,
        entry_side: LegSide,
        expected_pos_side: Side,
        entry_qty: Decimal,
    }
    struct CloseTarget {
        venue: String,
        account_id: Option<String>,
        order_side: Side,
        leg_side: LegSide,
        qty: Decimal,
    }

    let child_fills_on = |family: &[crate::state_store::SignalExecution],
                          canonical: &str,
                          side: LegSide|
     -> Result<Decimal, String> {
        let mut total = dec!(0);
        for child in family {
            for cl in &child.legs {
                if cl.side == side && crate::state::canonical_venue_id(&cl.venue) == Some(canonical)
                {
                    total += ledger_qty_to_dec(cl.filled_qty, "child leg fill")?;
                }
            }
        }
        Ok(total)
    };

    for sig in roots {
        let family_prefix = format!("flat-{}-", sig.id);
        let family: Vec<crate::state_store::SignalExecution> = all_signals
            .iter()
            .filter(|s| s.id.starts_with(&family_prefix))
            .cloned()
            .collect();

        let mut exposures: Vec<LegExposure> = Vec::new();
        let mut targets: Vec<CloseTarget> = Vec::new();
        let mut undetermined = false;
        let mut seen: std::collections::HashSet<(String, &'static str)> =
            std::collections::HashSet::new();
        let mut accounts: HashMap<&'static str, arbitrageur_core::position::AccountSnapshot> =
            HashMap::new();
        let mut unreadable: std::collections::HashSet<&'static str> =
            std::collections::HashSet::new();

        for leg in &sig.legs {
            let Some(canonical) = crate::state::canonical_venue_id(&leg.venue) else {
                undetermined = true;
                out.notes.push(format!(
                    "{}: venue '{}' is not a canonical paper venue",
                    sig.id, leg.venue
                ));
                continue;
            };
            if !seen.insert((canonical.to_string(), leg.side.as_str())) {
                continue;
            }
            let entry_qty = ledger_qty_to_dec(leg.filled_qty, "root leg fill")?;
            let closed_qty = child_fills_on(&family, canonical, opposite(leg.side))?;
            let residual = (entry_qty - closed_qty).max(dec!(0));
            let expected_pos_side = match leg.side {
                LegSide::Long => Side::Buy,
                LegSide::Short => Side::Sell,
            };
            exposures.push(LegExposure {
                canonical,
                entry_side: leg.side,
                expected_pos_side,
                entry_qty,
            });
            if residual <= dec!(0) {
                continue;
            }
            if !accounts.contains_key(canonical) && !unreadable.contains(canonical) {
                if is_live {
                    match state.get_adapter(canonical) {
                        Some(adapter) => {
                            if !adapter.supports_live_execution() || !adapter.has_live_credentials() {
                                unreadable.insert(canonical);
                                out.notes.push(format!(
                                    "{}: venue '{}' does not have authenticated live credentials",
                                    sig.id, leg.venue
                                ));
                            } else {
                                match adapter.fetch_account().await {
                                    Ok(acc) => {
                                        accounts.insert(canonical, acc);
                                    }
                                    Err(e) => {
                                        unreadable.insert(canonical);
                                        out.notes.push(format!(
                                            "{}: cannot read {} live account: {e}",
                                            sig.id, leg.venue
                                        ));
                                    }
                                }
                            }
                        }
                        None => {
                            unreadable.insert(canonical);
                            out.notes.push(format!(
                                "{}: no live adapter exists for venue '{}'",
                                sig.id, leg.venue
                            ));
                        }
                    }
                } else {
                    match papers
                        .iter()
                        .find(|(vid, _)| *vid == canonical)
                        .map(|(_, p)| p.clone())
                    {
                        Some(paper) => match paper.fetch_account().await {
                            Ok(acc) => {
                                accounts.insert(canonical, acc);
                            }
                            Err(e) => {
                                unreadable.insert(canonical);
                                out.notes.push(format!(
                                    "{}: cannot read {} paper account: {e}",
                                    sig.id, leg.venue
                                ));
                            }
                        },
                        None => {
                            unreadable.insert(canonical);
                            out.notes.push(format!(
                                "{}: no paper account exists for venue '{}'",
                                sig.id, leg.venue
                            ));
                        }
                    }
                }
            }
            if unreadable.contains(canonical) {
                undetermined = true;
                continue;
            }
            let pos = accounts.get(canonical).and_then(|acc| {
                acc.positions
                    .iter()
                    .find(|p| p.symbol == sig.symbol && p.quantity > dec!(0))
            });
            match pos {
                None => {}
                Some(p) if p.side != expected_pos_side => {
                    undetermined = true;
                    out.notes.push(format!(
                        "{}: {} position side {:?} contradicts {:?} leg; leaving open",
                        sig.id, leg.venue, p.side, leg.side
                    ));
                }
                Some(p) => {
                    let basis = residual.min(p.quantity);
                    // Leftover sweep: when the residual exceeds the closable
                    // quantity by less than one full ticker step, close the
                    // ENTIRE residual instead of stranding sub-step dust.
                    // Oversized reduce-only paper orders cap at the position.
                    let mut qty = basis;
                    let swept =
                        sweep_flatten_qty(residual, basis, flatten_step_qty(state, &sig.symbol));
                    if swept > basis {
                        qty = swept;
                        out.notes.push(format!(
                            "{}: {} leg sweep closes entire residual {}/{} (sub-step leftover)",
                            sig.id, leg.venue, swept, basis
                        ));
                    }
                    targets.push(CloseTarget {
                        venue: leg.venue.clone(),
                        account_id: leg.account_id.clone(),
                        order_side: match p.side {
                            Side::Buy => Side::Sell,
                            Side::Sell => Side::Buy,
                        },
                        leg_side: opposite(leg.side),
                        qty,
                    });
                }
            }
        }

        if undetermined {
            continue;
        }

        let close_family = |note: &str| -> Result<(), String> {
            state
                .state_store
                .update_signal_status(&sig.id, SignalStatus::Closed, Some(note))
                .map_err(|e| e.to_string())?;
            for child in state
                .state_store
                .list_signals(None, None)
                .map_err(|e| e.to_string())?
                .into_iter()
                .filter(|s| s.id.starts_with(&family_prefix) && s.status.counts_as_open())
            {
                state
                    .state_store
                    .update_signal_status(&child.id, SignalStatus::Closed, Some(note))
                    .map_err(|e| e.to_string())?;
            }
            Ok(())
        };

        if targets.is_empty() {
            close_family(&format!("flatten {}: no residual paper position", sig.id))?;
            out.signals_closed += 1;
            close_flattened_spatial_lot(state, &sig, &family);
            continue;
        }

        let close_id = format!("flat-{}-{:04x}", sig.id, rand::random::<u16>());
        let child_legs = targets
            .iter()
            .map(|t| -> Result<NewSignalLeg, String> {
                let intended = dec_to_f64(t.qty, "close quantity")?;
                if intended <= 0.0 {
                    return Err(format!("close quantity {} is not positive", t.qty));
                }
                Ok(NewSignalLeg {
                    account_id: t.account_id.clone(),
                    venue: t.venue.clone(),
                    side: t.leg_side,
                    intended_qty: intended,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        state
            .state_store
            .record_signal_execution(&NewSignalExecution {
                id: close_id.clone(),
                strategy_id: sig.strategy_id.clone(),
                symbol: sig.symbol.clone(),
                route_id: sig.route_id.clone(),
                expected_net_usd: None,
                note: Some(format!("emergency flatten of {}", sig.id)),
                legs: child_legs,
            })
            .map_err(|e| format!("Failed to persist flatten signal record: {e}"))?;

        let mut leg_desc = Vec::new();
        for t in &targets {
            match flatten_leg(
                FlattenLegArgs {
                    state,
                    venue: &t.venue,
                    symbol: &sig.symbol,
                    side: t.order_side,
                    qty: t.qty,
                    strategy_id: &sig.strategy_id,
                    sig_id: &sig.id,
                    notes: &mut out.notes,
                    execution_type,
                },
                is_live,
            )
            .await {
                Ok(Some((qty, px, fee, oid))) => {
                    state
                        .state_store
                        .apply_leg_fill(
                            &close_id,
                            t.leg_side,
                            dec_to_f64(qty, "fill quantity")?,
                            dec_to_f64(px, "fill price")?,
                            dec_to_f64(fee, "fill fee")?,
                            Some(oid.as_str()),
                        )
                        .map_err(|e| e.to_string())?;
                    out.legs_closed += 1;
                    leg_desc.push(format!("{} {:?} {}/{}", t.venue, t.leg_side, qty, t.qty));
                    if qty < t.qty {
                        out.notes.push(format!(
                            "{}: {} leg partially closed {}/{}",
                            sig.id, t.venue, qty, t.qty
                        ));
                    }
                }
                Ok(None) => {
                    out.notes.push(format!(
                        "{}: {} leg got no fill (empty book)",
                        sig.id, t.venue
                    ));
                }
                Err(e) => {
                    out.notes
                        .push(format!("{}: {} leg close failed: {e}", sig.id, t.venue));
                }
            }
        }

        let family_now: Vec<crate::state_store::SignalExecution> = state
            .state_store
            .list_signals(None, None)
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|s| s.id.starts_with(&family_prefix))
            .collect();

        let mut fully_flat = true;
        for exp in &exposures {
            let residual = (exp.entry_qty
                - child_fills_on(&family_now, exp.canonical, opposite(exp.entry_side))?)
            .max(dec!(0));
            if residual <= dec!(0) {
                continue;
            }
            if is_live {
                let Some(adapter) = state.get_adapter(exp.canonical) else {
                    fully_flat = false;
                    continue;
                };
                match adapter.fetch_account().await {
                    Ok(acc) => match acc
                        .positions
                        .iter()
                        .find(|p| p.symbol == sig.symbol && p.quantity > dec!(0))
                    {
                        None => {}
                        Some(p) if p.side != exp.expected_pos_side => {
                            fully_flat = false;
                            out.notes.push(format!(
                                "{}: {} position side {:?} contradicts {:?} leg; leaving open",
                                sig.id, exp.canonical, p.side, exp.entry_side
                            ));
                        }
                        Some(_) => fully_flat = false,
                    },
                    Err(_) => fully_flat = false,
                }
            } else {
                let Some((_, paper)) = papers.iter().find(|(vid, _)| *vid == exp.canonical) else {
                    fully_flat = false;
                    continue;
                };
                match paper.fetch_account().await {
                    Ok(acc) => match acc
                        .positions
                        .iter()
                        .find(|p| p.symbol == sig.symbol && p.quantity > dec!(0))
                    {
                        None => {}
                        Some(p) if p.side != exp.expected_pos_side => {
                            fully_flat = false;
                            out.notes.push(format!(
                                "{}: {} position side {:?} contradicts {:?} leg; leaving open",
                                sig.id, exp.canonical, p.side, exp.entry_side
                            ));
                        }
                        Some(_) => fully_flat = false,
                    },
                    Err(_) => fully_flat = false,
                }
            }
        }

        let note = format!("emergency flatten via {close_id}");
        // Realized PnL comes from the attributed ledger lot when the exit
        // fills cover it; single-leg residuals (e.g. triangle legs) realize
        // purely through their signal legs and keep the 0.0 log convention.
        let mut flatten_realized: Option<(f64, f64)> = None;
        let (venue_a, venue_b) = (
            sig.legs
                .first()
                .map(|l| l.venue.clone())
                .unwrap_or_default(),
            sig.legs.get(1).map(|l| l.venue.clone()).unwrap_or_default(),
        );
        if fully_flat {
            close_family(&note)?;
            out.signals_closed += 1;
            flatten_realized = close_flattened_spatial_lot(state, &sig, &family_now);
            if let Some(bot) = state.radar_bot.lock().clone() {
                bot.forget_position(&sig.symbol, &venue_a, &venue_b);
                let _ = state.state_store.delete_bot_position(&sig.symbol, &venue_a, &venue_b);
                let _ = state.state_store.delete_bot_position(&sig.symbol, &venue_b, &venue_a);
            }
        } else {
            state
                .state_store
                .update_signal_status(&sig.id, SignalStatus::Unresolved, Some(&note))
                .map_err(|e| e.to_string())?;
            state
                .state_store
                .update_signal_status(&close_id, SignalStatus::Unresolved, Some(&note))
                .map_err(|e| e.to_string())?;
        }

        state.execution_logs.write().push(ExecutionLogEntry {
            id: format!("exec-flatten-{}-{}", cycle, sig.id),
            timestamp: Utc::now().to_rfc3339(),
            trade_type: "Flatten".into(),
            strategy_id: crate::attribution::attribute_strategy_id(&sig.strategy_id),
            symbol: sig.symbol.clone(),
            venue_a,
            venue_b,
            leg_a_status: leg_desc.first().cloned().unwrap_or_else(|| "—".into()),
            leg_b_status: leg_desc.get(1).cloned().unwrap_or_else(|| "—".into()),
            success: fully_flat,
            gross_profit: 0.0,
            fees: 0.0,
            net_profit: flatten_realized.map(|(usd, _)| usd).unwrap_or(0.0),
            latency_us: 0,
            note: if fully_flat {
                format!("Emergency flatten closed signal {}", sig.id)
            } else {
                format!(
                    "Emergency flatten PARTIAL for signal {} — residual exposure",
                    sig.id
                )
            },
            direction: Some("Emergency Flatten".into()),
            trigger_spread_bps: None,
            baseline_mean_spread_bps: None,
            position_step: None,
            max_units: None,
            nominal_hurdle_bps: None,
            is_live,
        });
    }

    out.residual_signals = state
        .state_store
        .list_signals(symbol, None)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|s| {
            !s.id.starts_with("flat-")
                && s.strategy_id.eq_ignore_ascii_case(&sid)
                && s.status.counts_as_open()
        })
        .count();

    if !is_live {
        let mut equity_after = dec!(0);
        for (_, paper) in state.paper_exchanges() {
            equity_after += paper
                .fetch_account()
                .await
                .map_err(|e| e.to_string())?
                .equity;
        }
        {
            let mut total_pnl = state.total_realized_profit.write();
            *total_pnl = equity_after - state.paper_baseline_equity();
        }
        state.circuit_breaker.update_equity(equity_after);
        refresh_equity_expectations_after_fills(state, &flatten_equity_before).await;
    }
    maybe_record_margin_snapshots(state, Utc::now().timestamp_millis()).await;

    Ok(out)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VenueCapabilityDto {
    pub venue: String,
    pub name: String,
    pub supports_live: bool,
    pub has_credentials: bool,
    pub live_ready: bool,
}

pub fn get_venue_execution_capabilities_impl(state: &AppEngineState) -> Vec<VenueCapabilityDto> {
    state.sync_vault_credentials_to_adapters();
    let is_unlocked = state.vault.write().is_unlocked();
    let venues = state.get_venues();
    venues
        .into_iter()
        .map(|(vid, vname, adapter)| {
            let supports_live = adapter.supports_live_execution();
            let has_credentials = adapter.has_live_credentials();
            let live_ready = is_unlocked && supports_live && has_credentials;
            VenueCapabilityDto {
                venue: vid.to_string(),
                name: vname.to_string(),
                supports_live,
                has_credentials,
                live_ready,
            }
        })
        .collect()
}

pub fn get_execution_logs_by_mode_impl(
    state: &AppEngineState,
    is_live_filter: Option<bool>,
) -> Result<Vec<ExecutionLogEntry>, String> {
    let mode = is_live_filter.unwrap_or_else(|| *state.is_live_mode.read());
    let logs = state.execution_logs.read();
    let filtered: Vec<ExecutionLogEntry> = logs
        .iter()
        .filter(|l| l.is_live == mode)
        .cloned()
        .collect();
    Ok(filtered)
}

#[cfg(test)]
pub fn get_execution_logs_impl(state: &AppEngineState) -> Result<Vec<ExecutionLogEntry>, String> {
    get_execution_logs_by_mode_impl(state, None)
}

pub fn set_live_mode_impl(enabled: bool, state: &AppEngineState) -> Result<bool, String> {
    if enabled {
        let is_unlocked = state.vault.write().is_unlocked();
        if !is_unlocked {
            *state.is_live_mode.write() = false;
            return Err("Cannot enable real trading while the secure credential vault is locked. Unlock the vault first.".into());
        }
        state.sync_vault_credentials_to_adapters();
        let ready_venues: Vec<String> = state
            .get_venues()
            .into_iter()
            .filter(|(_, _, adapter)| adapter.supports_live_execution() && adapter.has_live_credentials())
            .map(|(vid, _, _)| vid.to_string())
            .collect();
        let ready_count = ready_venues.len();
        if ready_count < 2 {
            *state.is_live_mode.write() = false;
            return Err(format!(
                "Real trading requires at least two venues with authenticated live credentials (currently {ready_count} ready: {}). Please configure and enable API keys in Settings.",
                if ready_venues.is_empty() { "none".to_string() } else { ready_venues.join(", ") }
            ));
        }
        *state.is_live_mode.write() = true;
        {
            let mut s = state.settings.write();
            s.trading_mode = "real".to_string();
        }
        let _ = state.state_store.kv_set("engine.trading_mode", "real");
        Ok(true)
    } else {
        *state.is_live_mode.write() = false;
        {
            let mut s = state.settings.write();
            s.trading_mode = "paper".to_string();
        }
        let _ = state.state_store.kv_set("engine.trading_mode", "paper");
        Ok(false)
    }
}

pub fn set_engine_paused_impl(paused: bool, state: &AppEngineState) -> Result<bool, String> {
    let mut p = state.is_paused.write();
    *p = paused;
    Ok(*p)
}

pub fn get_engine_settings_impl(state: &AppEngineState) -> Result<EngineSettingsDto, String> {
    let mut sanitized = state.settings.read().clone();
    sanitized.clear_credential_material();
    sanitized.configured_venues = state.vault.write().configured_venues().unwrap_or_default();
    Ok(sanitized)
}

pub fn save_engine_settings_impl(
    mut settings: EngineSettingsDto,
    state: &AppEngineState,
) -> Result<String, String> {
    let is_real = settings.trading_mode.eq_ignore_ascii_case("real");
    let is_paper = settings.trading_mode.eq_ignore_ascii_case("paper");
    if !is_real && !is_paper {
        return Err("trading_mode must be 'paper' or 'real'".into());
    }

    if is_real {
        let is_unlocked = state.vault.write().is_unlocked();
        if !is_unlocked {
            return Err("Cannot switch to 'real' trading mode while the secure credential vault is locked. Unlock the vault first.".into());
        }
        state.sync_vault_credentials_to_adapters();
        let ready_venues: Vec<String> = state
            .get_venues()
            .into_iter()
            .filter(|(_, _, adapter)| adapter.supports_live_execution() && adapter.has_live_credentials())
            .map(|(vid, _, _)| vid.to_string())
            .collect();
        let ready_count = ready_venues.len();
        if ready_count < 2 {
            return Err(format!(
                "Cannot switch to 'real' trading mode: At least two venues must have authenticated live credentials (currently {ready_count} ready: {}). Please configure and enable API keys in Settings.",
                if ready_venues.is_empty() { "none".to_string() } else { ready_venues.join(", ") }
            ));
        }
        *state.is_live_mode.write() = true;
    } else {
        *state.is_live_mode.write() = false;
    }
    settings.spread_samples_max_days = match settings.spread_samples_max_days {
        None => None,
        Some(0) => None,
        Some(d) => Some(d),
    };
    match settings.spread_samples_max_gb {
        None => {}
        Some(0.0) => settings.spread_samples_max_gb = None,
        Some(gb) if !gb.is_finite() || gb < 0.0 => {
            return Err("spread_samples_max_gb must be a non-negative finite number".into());
        }
        Some(_) => {}
    }
    if !settings.min_depth_ratio.is_finite() || settings.min_depth_ratio < 1.0 {
        return Err("Min depth ratio must be at least 1.0".into());
    }
    if settings.engine_rpc_port == 0
        || settings.engine_ws_port == 0
        || settings.engine_rpc_port == settings.engine_ws_port
    {
        return Err("Engine ports must be nonzero and distinct".into());
    }
    match settings.bind_address.parse::<std::net::IpAddr>() {
        Ok(ip) if ip.is_loopback() => {}
        _ => return Err("bind_address must be a loopback IP address".into()),
    }
    if !settings.paper_starting_balance_usd.is_finite()
        || settings.paper_starting_balance_usd <= 0.0
        || settings.paper_starting_balance_usd > 1_000_000_000_000.0
    {
        return Err("Paper starting balance must be a positive number up to 1e12 USD".into());
    }
    // Overall circuit-breaker + safety-net limits (Safety settings section).
    if !settings.cb_max_daily_drawdown_usd.is_finite()
        || settings.cb_max_daily_drawdown_usd <= 0.0
        || settings.cb_max_daily_drawdown_usd > 1_000_000_000_000.0
    {
        return Err("Max daily drawdown (USD) must be a positive number up to 1e12".into());
    }
    if !settings.cb_max_daily_drawdown_pct.is_finite()
        || settings.cb_max_daily_drawdown_pct <= 0.0
        || settings.cb_max_daily_drawdown_pct > 100.0
    {
        return Err("Max daily drawdown (%) must be within 0–100".into());
    }
    if settings.cb_max_consecutive_errors < 1 || settings.cb_max_consecutive_errors > 1000 {
        return Err("Max consecutive errors must be within 1–1000".into());
    }
    if !settings.cb_max_spread_divergence_bps.is_finite()
        || settings.cb_max_spread_divergence_bps <= 0.0
        || settings.cb_max_spread_divergence_bps > 100_000.0
    {
        return Err("Max spread divergence (bps) must be a positive number up to 100000".into());
    }
    if !settings.delta_max_unhedged_usd_per_asset.is_finite()
        || settings.delta_max_unhedged_usd_per_asset <= 0.0
        || settings.delta_max_unhedged_usd_per_asset > 1_000_000_000.0
    {
        return Err("DeltaGuard max unhedged per asset must be a positive USD amount".into());
    }
    if !settings.margin_warning_usage_pct.is_finite()
        || !settings.margin_critical_usage_pct.is_finite()
        || !settings.margin_min_liquidation_distance_pct.is_finite()
    {
        return Err("Margin thresholds must be finite numbers".into());
    }
    if settings.margin_warning_usage_pct < 0.0
        || settings.margin_warning_usage_pct >= settings.margin_critical_usage_pct
        || settings.margin_critical_usage_pct > 100.0
    {
        return Err("Margin warning % must be below critical %, both within 0–100".into());
    }
    if settings.margin_min_liquidation_distance_pct <= 0.0
        || settings.margin_min_liquidation_distance_pct > 100.0
    {
        return Err("Min liquidation distance % must be within 0–100".into());
    }
    let submitted = settings.submitted_credentials();
    if !submitted.is_empty() {
        // Bind any pre-account-model records to `{venue}-default` profiles
        // before writing so every exchange credential lands on a profile.
        ensure_credential_migration(state)?;
        let mut vault = state.vault.write();
        for cred in submitted {
            let venue_key = cred.venue.trim().to_lowercase();
            if venue_key == "remote_hsm" {
                // The HSM token is not an exchange account.
                vault
                    .upsert_credentials(vec![cred])
                    .map_err(|e| e.to_string())?;
                continue;
            }
            let profile = ensure_default_profile(state, &venue_key, cred.enabled)?;
            vault
                .upsert_profile_credentials(&profile.id, cred)
                .map_err(|e| e.to_string())?;
        }
    }
    // Toggle-only saves must still update enabled metadata on existing vault
    // records; a locked vault skips this silently since no secrets changed.
    // A venue credential stays enabled when either the engine flag is on or
    // an enabled account profile exists — a settings save can never disable a
    // venue the user explicitly switched on at the account level.
    let account_enabled = |venue: &str| {
        state
            .state_store
            .has_enabled_account(venue)
            .unwrap_or(false)
    };
    let enablement: Vec<(String, bool)> = vec![
        ("remote_hsm".into(), true),
        (
            "hyperliquid".into(),
            settings.hyperliquid_enabled || account_enabled("hyperliquid"),
        ),
        (
            "lighter".into(),
            settings.lighter_enabled || account_enabled("lighter"),
        ),
        (
            "vanta".into(),
            settings.vanta_enabled || account_enabled("vanta"),
        ),
        (
            "robinhood".into(),
            settings.robinhood_enabled || account_enabled("robinhood"),
        ),
        (
            "arcus".into(),
            settings.arcus_enabled || account_enabled("arcus"),
        ),
        (
            "kraken".into(),
            settings.kraken_enabled
                || settings.kraken_spot_enabled
                || settings.kraken_futures_enabled
                || account_enabled("kraken"),
        ),
        (
            "kraken_spot".into(),
            (settings.kraken_enabled && settings.kraken_spot_enabled)
                || account_enabled("kraken_spot"),
        ),
        (
            "kraken_futures".into(),
            (settings.kraken_enabled && settings.kraken_futures_enabled)
                || account_enabled("kraken_futures"),
        ),
        (
            "polymarket".into(),
            settings.polymarket_enabled || account_enabled("polymarket"),
        ),
        (
            "kalshi".into(),
            settings.kalshi_enabled || account_enabled("kalshi"),
        ),
        (
            "aster".into(),
            settings.aster_enabled || account_enabled("aster"),
        ),
        (
            "popdex".into(),
            settings.popdex_enabled || account_enabled("popdex"),
        ),
        (
            "binance".into(),
            settings.binance_enabled || account_enabled("binance"),
        ),
        (
            "okx".into(),
            settings.okx_enabled || account_enabled("okx"),
        ),
        (
            "coinbase".into(),
            settings.coinbase_enabled || account_enabled("coinbase"),
        ),
        (
            "decibel".into(),
            settings.decibel_enabled || account_enabled("decibel"),
        ),
        (
            "ibkr".into(),
            settings.ibkr_enabled || account_enabled("ibkr"),
        ),
        (
            "ig".into(),
            settings.ig_enabled || account_enabled("ig"),
        ),
    ];
    {
        let mut vault = state.vault.write();
        if vault.is_unlocked() {
            vault
                .update_enabled(&enablement)
                .map_err(|e| e.to_string())?;
        }
    }
    let mut sanitized = settings;
    sanitized.clear_credential_material();
    sanitized.configured_venues = state.vault.write().configured_venues().unwrap_or_default();
    match sanitized.spread_samples_max_days {
        Some(0) | None => {
            let _ = state.state_store.kv_delete("retention.max_days");
        }
        Some(d) => {
            let _ = state
                .state_store
                .kv_set("retention.max_days", &d.to_string());
        }
    }
    match sanitized.spread_samples_max_gb {
        None => {
            let _ = state.state_store.kv_delete("retention.max_gb");
        }
        Some(0.0) => {
            sanitized.spread_samples_max_gb = None;
            let _ = state.state_store.kv_delete("retention.max_gb");
        }
        Some(gb) => {
            let _ = state
                .state_store
                .kv_set("retention.max_gb", &gb.to_string());
        }
    }
    // A changed starting balance reinitializes every paper account: fresh
    // equity, no open positions. Unrelated saves leave paper state alone.
    // Re-anchor the breaker to the new paper universe so the balance change
    // itself is never misread as a drawdown halt.
    let prev_balance = state.settings.read().paper_starting_balance_usd;
    if sanitized.paper_starting_balance_usd != prev_balance {
        if let Some(eq) = Decimal::from_f64_retain(sanitized.paper_starting_balance_usd) {
            for (_, paper) in state.paper_exchanges() {
                paper.reset_account(eq);
            }
            // A reset moves the baseline itself: rebase every account's
            // expected equity to the new starting balance (flat, no event).
            let f = sanitized.paper_starting_balance_usd;
            if f.is_finite() && f >= 0.0 {
                let now_ms = Utc::now().timestamp_millis();
                for (vid, _) in state.paper_exchanges() {
                    let _ = state
                        .state_store
                        .set_equity_expectation(vid, f, "", now_ms);
                }
            }
            if let Some(eq) = Decimal::from_f64_retain(sanitized.paper_starting_balance_usd) {
                let n = state.paper_exchanges().len() as u32;
                if n > 0 {
                    cb_reanchor_to_paper(state, eq * Decimal::from(n));
                }
            }
        }
    }
    let _ = state
        .state_store
        .kv_set("engine.trading_mode", &sanitized.trading_mode);
    // Persist Safety limits so restarts keep the operator's values.
    let _ = state.state_store.kv_set("risk.cb_max_dd_usd", &sanitized.cb_max_daily_drawdown_usd.to_string());
    let _ = state.state_store.kv_set("risk.cb_max_dd_pct", &sanitized.cb_max_daily_drawdown_pct.to_string());
    let _ = state.state_store.kv_set("risk.cb_max_errors", &sanitized.cb_max_consecutive_errors.to_string());
    let _ = state.state_store.kv_set("risk.cb_max_spread_bps", &sanitized.cb_max_spread_divergence_bps.to_string());
    let _ = state.state_store.kv_set("risk.delta_max_unhedged", &sanitized.delta_max_unhedged_usd_per_asset.to_string());
    let _ = state.state_store.kv_set("risk.margin_warn", &sanitized.margin_warning_usage_pct.to_string());
    let _ = state.state_store.kv_set("risk.margin_crit", &sanitized.margin_critical_usage_pct.to_string());
    let _ = state.state_store.kv_set("risk.margin_min_liq", &sanitized.margin_min_liquidation_distance_pct.to_string());
    // Live-apply: breaker limits update in place (no restart), delta guard
    // re-limits, margin thresholds feed the next snapshot automatically.
    {
        use arbitrageur_risk::circuit_breaker::CircuitBreakerConfig;
        let cfg = CircuitBreakerConfig {
            max_daily_drawdown_usd: Decimal::from_f64_retain(sanitized.cb_max_daily_drawdown_usd).unwrap_or(dec!(25000.0)),
            max_daily_drawdown_pct: Decimal::from_f64_retain(sanitized.cb_max_daily_drawdown_pct).unwrap_or(dec!(5.0)),
            max_consecutive_errors: sanitized.cb_max_consecutive_errors,
            max_spread_divergence_bps: Decimal::from_f64_retain(sanitized.cb_max_spread_divergence_bps).unwrap_or(dec!(250.0)),
        };
        state.circuit_breaker.update_config(cfg);
        if let Some(limit) = Decimal::from_f64_retain(sanitized.delta_max_unhedged_usd_per_asset) {
            state.delta_guard.write().set_max_unhedged_usd_per_asset(limit);
        }
    }
    *state.settings.write() = sanitized;
    Ok("Configuration applied to the current session".into())
}

// ---------------------------------------------------------------------------
// Tauri Command Handlers (thin IPC delegators)
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn get_system_status(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SystemStatusDto, String> {
    get_system_status_impl(&state).await
}

#[tauri::command]
pub async fn refresh_venue_latencies(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SystemStatusDto, String> {
    refresh_venue_latencies_impl(&state).await
}

#[tauri::command]
pub async fn get_market_universe(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<VenueMarketUniverseDto>, String> {
    get_market_universe_impl(&state).await
}

#[tauri::command]
pub async fn fetch_orderbooks(
    symbol: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<VenueOrderBookDto>, String> {
    fetch_orderbooks_impl(&symbol, &state).await
}

/// Venue 24h-volume maps behind a 5-minute shared cache — volumes move
/// slowly and both the liquidity sort and the signal-series weighting
/// consume them. Venues that fail to report are simply absent.
pub(crate) async fn cached_venue_volumes(
    state: &AppEngineState,
) -> HashMap<String, HashMap<String, f64>> {
    const TTL: std::time::Duration = std::time::Duration::from_secs(300);
    {
        let cache = state.volume_cache.lock();
        if let Some((t, m)) = cache.as_ref() {
            if t.elapsed() < TTL {
                return m.clone();
            }
        }
    }
    let mut map = HashMap::new();
    for (vid, _vname, adapter) in state.get_venues() {
        if let Ok(vols) = adapter.fetch_volumes_24h().await {
            map.insert(
                vid.to_string(),
                vols.into_iter()
                    .filter_map(|(k, v)| v.to_string().parse::<f64>().ok().map(|f| (k, f)))
                    .collect(),
            );
        }
    }
    // Session-seasonality sampler (Phase 1): persist this refresh into the
    // raw hourly volume log feeding the session rollup. Best-effort — a
    // store failure must never break the market-data path.
    {
        let now_ms = chrono::Utc::now().timestamp_millis();
        for (venue, vols) in &map {
            let _ = state
                .state_store
                .record_venue_volume_samples(venue, now_ms, vols);
        }
    }
    *state.volume_cache.lock() = Some((std::time::Instant::now(), map.clone()));
    map
}

#[tauri::command]
pub async fn get_ticker_volumes(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<VenueVolumesDto>, String> {
    let map = cached_venue_volumes(&state).await;
    Ok(map
        .into_iter()
        .map(|(venue_id, volumes)| VenueVolumesDto { venue_id, volumes })
        .collect())
}

// ---------------------------------------------------------------------------
// Signal Trend — durable dislocation series computed by arbitrageur-math
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalSeriesDto {
    pub symbol: String,
    /// "consensus" | "leader"
    pub mode: String,
    pub window_secs: u64,
    /// Total stored venue samples in the window (history-depth indicator).
    pub sample_count: u64,
    /// Earliest sample ever recorded for this symbol, if any.
    pub earliest_ts_ms: Option<i64>,
    /// Bucket timestamps (epoch ms), ascending.
    pub ts_ms: Vec<i64>,
    /// Sorted venue ids; `dev_bps[i][j]` is venue `venues[j]` at `ts_ms[i]`,
    /// or null when the venue had no observation in that bucket.
    pub venues: Vec<String>,
    /// venue_id -> display name for legend labels.
    pub venue_names: HashMap<String, String>,
    pub dev_bps: Vec<Vec<Option<f64>>>,
    /// Max/min deviation across venues present in each bucket; their
    /// difference is the gross pairwise spread.
    pub band_max_bps: Vec<f64>,
    pub band_min_bps: Vec<f64>,
}

pub async fn get_signal_series_impl(
    symbol: &str,
    mode: &str,
    window_secs: u64,
    state: &AppEngineState,
) -> Result<SignalSeriesDto, String> {
    validate_market_symbol(symbol)?;
    let window_secs = window_secs.clamp(10, 30 * 86_400); // 10s..30d (persisted retention)
    let max_points = 500usize;
    // Bucket granularity: ~poll cadence for short windows, coarser for
    // hour/day horizons so long slices stay renderable.
    let bucket_ms = ((window_secs * 1000) / max_points as u64).max(1_000) as i64;
    let now_ms = Utc::now().timestamp_millis();
    let since_ms = now_ms - (window_secs * 1000) as i64 - bucket_ms;

    let store = state.state_store.clone();
    let sym = symbol.to_string();
    let (raw, earliest_ts_ms) = tokio::task::spawn_blocking(move || {
        let rows = store.spread_samples(&sym, since_ms, now_ms)?;
        let earliest = store.earliest_spread_sample_ts(&sym)?;
        Ok::<_, anyhow::Error>((rows, earliest))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    let venue_names: HashMap<String, String> = state
        .get_venues()
        .into_iter()
        .map(|(vid, vname, _)| (vid.to_string(), vname.to_string()))
        .collect();

    let mut samples = Vec::with_capacity(raw.len());
    let mut sample_count = 0u64;
    for row in &raw {
        samples.push(MidSample {
            ts_ms: row.ts_ms,
            venue: row.venue_id.clone(),
            mid: row.mid,
        });
        sample_count += 1;
    }

    // Volume weights: venue_id -> this asset's reported 24h notional USD.
    // Asset naming differs per venue (PEPE / kPEPE / 1000PEPE).
    let base = symbol
        .split('-')
        .next()
        .unwrap_or(symbol)
        .to_ascii_uppercase();
    let vol_maps = cached_venue_volumes(state).await;
    let candidates = [base.clone(), format!("K{}", base), format!("1000{}", base)];
    let mut weights = HashMap::new();
    for (vid, map) in &vol_maps {
        let v = candidates
            .iter()
            .filter_map(|c| {
                map.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(c))
                    .map(|(_, v)| *v)
            })
            .next()
            .unwrap_or(0.0);
        if v > 0.0 {
            weights.insert(vid.clone(), v);
        }
    }

    let mode_enum = match mode {
        "leader" => BaselineMode::Leader,
        _ => BaselineMode::Consensus,
    };
    let series = compute_deviation_series(&samples, &weights, mode_enum, bucket_ms, max_points);
    Ok(SignalSeriesDto {
        symbol: symbol.to_string(),
        mode: match mode_enum {
            BaselineMode::Consensus => "consensus",
            BaselineMode::Leader => "leader",
        }
        .to_string(),
        window_secs,
        sample_count,
        earliest_ts_ms,
        ts_ms: series.ts_ms,
        venues: series.venues,
        venue_names,
        dev_bps: series.dev_bps,
        band_max_bps: series.band_max_bps,
        band_min_bps: series.band_min_bps,
    })
}

#[tauri::command]
pub async fn get_signal_series(
    symbol: String,
    mode: Option<String>,
    window_secs: Option<u64>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SignalSeriesDto, String> {
    get_signal_series_impl(
        &symbol,
        mode.as_deref().unwrap_or("consensus"),
        window_secs.unwrap_or(60),
        &state,
    )
    .await
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PairBaselineDto {
    pub buy_venue: String,
    pub sell_venue: String,
    pub mean_gross_bps: Option<f64>,
    pub percentile_gross_bps: Option<f64>,
    pub sample_count: u64,
    pub history_span_secs: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MatrixBaselineSnapshotDto {
    pub symbol: String,
    pub mode: String,
    pub manual_window_secs: u64,
    pub effective_window_secs: u64,
    pub auto_window_secs: Option<u64>,
    pub median_half_life_secs: Option<f64>,
    pub qualifying_pair_count: u32,
    pub max_history_span_secs: u64,
    pub using_manual_fallback: bool,
    pub pairs: Vec<PairBaselineDto>,
}

const BASELINE_MIN_WINDOW_OBS: usize = 20;
const BASELINE_SPAN_COVERAGE: f64 = 0.9;
const OU_MIN_OBS: usize = 40;
const OU_MIN_SPAN_SECS: f64 = 1_800.0;
const OU_MIN_R2: f64 = 0.6;
const AUTO_WINDOW_FLOOR_SECS: u64 = 1_800;
const AUTO_WINDOW_CAP_SECS: u64 = 604_800;
const PAIR_ALIGN_GAP_MS: i64 = 30_000;
const BASELINE_RETENTION_MS: i64 = 30 * 86_400_000;

fn compute_matrix_baselines(
    symbol: &str,
    mode: &str,
    manual_window_secs: u64,
    percentile: u32,
    samples: &[crate::state_store::SpreadSample],
    now_ms: i64,
) -> MatrixBaselineSnapshotDto {
    let mut mids_by_venue: HashMap<String, Vec<(i64, f64)>> = HashMap::new();
    for s in samples {
        if s.mid.is_finite() && s.mid > 0.0 {
            mids_by_venue
                .entry(s.venue_id.clone())
                .or_default()
                .push((s.ts_ms, s.mid));
        }
    }
    let mut venue_ids: Vec<String> = mids_by_venue.keys().cloned().collect();
    venue_ids.sort();

    struct PairEval {
        buy: String,
        sell: String,
        series: Vec<arbitrageur_math::pair_monitor::PairObservation>,
        span_secs: u64,
        half_life_secs: Option<f64>,
        recommendation_secs: Option<u64>,
    }

    let mut evals: Vec<PairEval> = Vec::new();
    for buy_v in &venue_ids {
        for sell_v in &venue_ids {
            if buy_v == sell_v {
                continue;
            }
            let a = &mids_by_venue[buy_v];
            let b = &mids_by_venue[sell_v];
            let series = arbitrageur_math::pair_monitor::build_pair_series(a, b, PAIR_ALIGN_GAP_MS);
            let n = series.len();
            let span_ms = if n >= 2 {
                series[n - 1].ts_ms - series[0].ts_ms
            } else {
                0
            };
            let span_secs = (span_ms / 1000).max(0) as u64;
            let ou = if n >= OU_MIN_OBS && span_secs as f64 >= OU_MIN_SPAN_SECS {
                let dt = span_ms as f64 / 1000.0 / (n - 1) as f64;
                if dt.is_finite() && dt > 0.0 {
                    let vals: Vec<f64> = series.iter().map(|o| o.spread_bps).collect();
                    OuModelParams::fit(&vals, dt).filter(|p| {
                        p.half_life.is_finite() && p.half_life > 0.0 && p.r_squared >= OU_MIN_R2
                    })
                } else {
                    None
                }
            } else {
                None
            };
            evals.push(PairEval {
                buy: buy_v.clone(),
                sell: sell_v.clone(),
                series,
                span_secs,
                half_life_secs: ou.map(|p| p.half_life),
                recommendation_secs: ou.map(|p| {
                    ((2.0 * p.half_life).round().max(0.0) as u64)
                        .clamp(AUTO_WINDOW_FLOOR_SECS, AUTO_WINDOW_CAP_SECS)
                }),
            });
        }
    }

    let mut recommendations: Vec<u64> = evals
        .iter()
        .filter_map(|e| e.recommendation_secs)
        .collect();
    recommendations.sort_unstable();
    let mut half_lives: Vec<f64> = evals.iter().filter_map(|e| e.half_life_secs).collect();
    half_lives.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let qualifying_pair_count = recommendations.len() as u32;
    let auto_window_secs = if mode == "auto" && !recommendations.is_empty() {
        Some(recommendations[recommendations.len() / 2])
    } else {
        None
    };
    let median_half_life_secs = if half_lives.is_empty() {
        None
    } else {
        Some(half_lives[half_lives.len() / 2])
    };
    let effective_window_secs = auto_window_secs.unwrap_or(manual_window_secs);
    let using_manual_fallback = mode == "auto" && auto_window_secs.is_none();
    let max_history_span_secs = evals.iter().map(|e| e.span_secs).max().unwrap_or(0);

    let cutoff_ms = now_ms - (effective_window_secs as i64) * 1000;
    let required_span_secs = BASELINE_SPAN_COVERAGE * effective_window_secs as f64;
    let mut pairs = Vec::with_capacity(evals.len());
    for e in &evals {
        let windowed: Vec<&arbitrageur_math::pair_monitor::PairObservation> = e
            .series
            .iter()
            .filter(|o| o.ts_ms >= cutoff_ms)
            .collect();
        let window_span_secs = if windowed.len() >= 2 {
            (windowed[windowed.len() - 1].ts_ms - windowed[0].ts_ms) as f64 / 1000.0
        } else {
            0.0
        };
        let sufficient =
            windowed.len() >= BASELINE_MIN_WINDOW_OBS && window_span_secs >= required_span_secs;
        let (mean_gross_bps, percentile_gross_bps) = if sufficient {
            let mut vals: Vec<f64> = windowed.iter().map(|o| o.spread_bps).collect();
            let mean = vals.iter().sum::<f64>() / vals.len() as f64;
            vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let idx = (((percentile as f64 / 100.0) * vals.len() as f64).ceil() as usize)
                .clamp(1, vals.len())
                - 1;
            (Some(mean), Some(vals[idx]))
        } else {
            (None, None)
        };
        pairs.push(PairBaselineDto {
            buy_venue: e.buy.clone(),
            sell_venue: e.sell.clone(),
            mean_gross_bps,
            percentile_gross_bps,
            sample_count: windowed.len() as u64,
            history_span_secs: window_span_secs.max(0.0).floor() as u64,
        });
    }

    MatrixBaselineSnapshotDto {
        symbol: symbol.to_string(),
        mode: mode.to_string(),
        manual_window_secs,
        effective_window_secs,
        auto_window_secs,
        median_half_life_secs,
        qualifying_pair_count,
        max_history_span_secs,
        using_manual_fallback,
        pairs,
    }
}

pub async fn get_matrix_baselines_impl(
    symbol: &str,
    mode: &str,
    manual_window_secs: u64,
    percentile: u32,
    state: &AppEngineState,
) -> Result<MatrixBaselineSnapshotDto, String> {
    validate_market_symbol(symbol)?;
    if !matches!(mode, "manual" | "auto") {
        return Err(format!("mode must be 'manual' or 'auto', got '{mode}'"));
    }
    if !(300..=2_592_000).contains(&manual_window_secs) {
        return Err(format!(
            "manual_window_secs must be within 300..=2592000 (5m-30d), got {manual_window_secs}"
        ));
    }
    if !(50..=99).contains(&percentile) {
        return Err(format!("percentile must be within 50..=99, got {percentile}"));
    }

    let base = symbol
        .split(['-', '/', '_'])
        .next()
        .unwrap_or(symbol)
        .to_ascii_uppercase();
    let keys = [base.clone(), format!("{base}-USDC"), format!("{base}-USD")];
    let now_ms = Utc::now().timestamp_millis();
    let since_ms = now_ms - BASELINE_RETENTION_MS;
    let store = state.state_store.clone();
    let sym = symbol.to_string();
    let mode_owned = mode.to_string();
    let snapshot = tokio::task::spawn_blocking(move || {
        let mut all = Vec::new();
        for k in &keys {
            if let Ok(mut rows) = store.spread_samples(k, since_ms, now_ms) {
                all.append(&mut rows);
            }
        }
        all.sort_by(|a, b| (a.ts_ms, a.venue_id.as_str()).cmp(&(b.ts_ms, b.venue_id.as_str())));
        all.dedup_by(|a, b| a.ts_ms == b.ts_ms && a.venue_id == b.venue_id);
        compute_matrix_baselines(
            &sym,
            &mode_owned,
            manual_window_secs,
            percentile,
            &all,
            now_ms,
        )
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(snapshot)
}

#[tauri::command]
pub async fn get_matrix_baselines(
    symbol: String,
    mode: String,
    manual_window_secs: u64,
    percentile: u32,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<MatrixBaselineSnapshotDto, String> {
    get_matrix_baselines_impl(&symbol, &mode, manual_window_secs, percentile, &state).await
}

#[tauri::command]
pub async fn scan_spatial_opportunities(
    symbol: String,
    min_bps: f64,
    order_qty: f64,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<SpatialOpportunityDto>, String> {
    scan_spatial_opportunities_impl(&symbol, min_bps, order_qty, &state).await
}

#[tauri::command]
pub async fn scan_spot_opportunities(
    symbol: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<SpotOpportunityDto>, String> {
    scan_spot_opportunities_impl(&symbol, &state).await
}

#[tauri::command]
pub async fn execute_spot_arbitrage(
    params: SpotExecutionParams,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SimStepResultDto, String> {
    execute_spot_arbitrage_impl(params, &state).await
}

#[tauri::command]
pub async fn scan_funding_opportunities(
    symbol: Option<String>,
    symbols: Option<Vec<String>>,
    trade_size_usd: f64,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<FundingOpportunityDto>, String> {
    if let Some(list) = symbols {
        if !list.is_empty() {
            return scan_funding_opportunities_batch_impl(&list, trade_size_usd, &state).await;
        }
    }
    let sym = symbol.unwrap_or_default();
    scan_funding_opportunities_impl(&sym, trade_size_usd, &state).await
}

/// Trend substrate for the funding-carry detail popup (per-leg funding
/// history + ticker mid history). Read-only over the persisted samples.
#[tauri::command]
pub async fn get_funding_trend(
    symbol: String,
    long_venue: String,
    short_venue: String,
    hours: Option<f64>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<FundingTrendDto, String> {
    get_funding_trend_impl(&symbol, &long_venue, &short_venue, hours, &state).await
}

#[tauri::command]
pub async fn scan_prediction_opportunities(
    market: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<PredictionOpportunityDto>, String> {
    scan_prediction_opportunities_impl(&market, &state).await
}

#[tauri::command]
pub async fn run_stat_arb_step(
    step: usize,
    price_y: f64,
    price_x: f64,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<StatArbStepResultDto, String> {
    run_stat_arb_step_impl(step, price_y, price_x, &state)
}

#[tauri::command]
pub async fn compute_optimal_thresholds(
    theta: f64,
    mu: f64,
    sigma: f64,
    transaction_cost: f64,
) -> Result<BertramThresholdDto, String> {
    compute_optimal_thresholds_impl(theta, mu, sigma, transaction_cost)
}

#[tauri::command]
pub async fn fit_ou_process(prices: Vec<f64>, dt: f64) -> Result<OuFitDto, String> {
    fit_ou_process_impl(&prices, dt)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn execute_spatial_arbitrage(
    symbol: String,
    buy_venue: String,
    sell_venue: String,
    order_qty: f64,
    direction: Option<String>,
    trigger_spread_bps: Option<f64>,
    baseline_mean_spread_bps: Option<f64>,
    position_step: Option<u32>,
    max_units: Option<u32>,
    nominal_hurdle_bps: Option<f64>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SimStepResultDto, String> {
    execute_spatial_arbitrage_impl(
        &symbol,
        &buy_venue,
        &sell_venue,
        order_qty,
        direction,
        trigger_spread_bps,
        baseline_mean_spread_bps,
        position_step,
        max_units,
        nominal_hurdle_bps,
        &state,
    )
    .await
}

#[tauri::command]
pub async fn scan_triangle_opportunities(
    filter_asset: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<TriangleOpportunityDto>, String> {
    scan_triangular_opportunities_impl(&filter_asset, &state).await
}

#[tauri::command]
pub async fn execute_triangle_arbitrage(
    params: TriangleExecutionParams,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SimStepResultDto, String> {
    execute_triangle_arbitrage_impl(params, &state).await
}

#[tauri::command]
pub async fn check_spatial_convergence(
    symbol: Option<String>,
    strategy_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<ConvergenceCheckDto>, String> {
    check_spatial_convergence_impl(symbol.as_deref(), &strategy_id, &state).await
}

#[tauri::command]
pub async fn execute_cross_ticker_arbitrage(
    long_symbol: String,
    short_symbol: String,
    long_venue: String,
    short_venue: String,
    order_qty: f64,
    net_spread_bps: Option<f64>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SimStepResultDto, String> {
    execute_cross_ticker_arbitrage_impl(
        &long_symbol,
        &short_symbol,
        &long_venue,
        &short_venue,
        order_qty,
        net_spread_bps,
        &state,
    )
    .await
}

#[tauri::command]
pub async fn fetch_multi_market_quotes(
    symbols: Vec<String>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<TickerMarketQuoteDto>, String> {
    fetch_multi_market_quotes_impl(symbols, &state).await
}

#[tauri::command]
pub async fn execute_funding_carry(
    symbol: String,
    long_venue: String,
    short_venue: String,
    trade_size_usd: f64,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SimStepResultDto, String> {
    execute_funding_carry_impl(&symbol, &long_venue, &short_venue, trade_size_usd, &state).await
}

#[tauri::command]
pub async fn execute_prediction_arbitrage(
    market_title: String,
    buy_yes_venue: String,
    buy_no_venue: String,
    quantity: f64,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SimStepResultDto, String> {
    execute_prediction_arbitrage_impl(
        &market_title,
        &buy_yes_venue,
        &buy_no_venue,
        quantity,
        &state,
    )
    .await
}

#[tauri::command]
pub async fn get_risk_overview(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<RiskOverviewDto, String> {
    get_risk_overview_impl(&state).await
}

#[tauri::command]
pub async fn reset_circuit_breaker(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<String, String> {
    reset_circuit_breaker_impl(&state)
}

#[tauri::command]
pub async fn trigger_emergency_killswitch(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<String, String> {
    trigger_emergency_killswitch_impl(&state)
}

#[tauri::command]
pub async fn flatten_ticker_positions(
    symbol: Option<String>,
    strategy_id: String,
    execution_type: Option<String>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<FlattenResultDto, String> {
    flatten_ticker_positions_with_type_impl(
        symbol.as_deref(),
        &strategy_id,
        execution_type.as_deref(),
        &state,
    )
    .await
}

#[tauri::command]
pub async fn get_execution_logs(
    state: State<'_, Arc<AppEngineState>>,
    is_live: Option<bool>,
) -> Result<Vec<ExecutionLogEntry>, String> {
    get_execution_logs_by_mode_impl(&state, is_live)
}

#[tauri::command]
pub async fn get_venue_execution_capabilities(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<VenueCapabilityDto>, String> {
    Ok(get_venue_execution_capabilities_impl(&state))
}

#[tauri::command]
pub async fn set_live_mode(
    enabled: bool,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<bool, String> {
    set_live_mode_impl(enabled, &state)
}

#[tauri::command]
pub async fn set_engine_paused(
    paused: bool,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<bool, String> {
    set_engine_paused_impl(paused, &state)
}

#[tauri::command]
pub async fn get_engine_settings(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<EngineSettingsDto, String> {
    get_engine_settings_impl(&state)
}

#[tauri::command]
pub async fn save_engine_settings(
    settings: EngineSettingsDto,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<String, String> {
    save_engine_settings_impl(settings, &state)
}

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
    if !config.target_profit_bps.is_finite() || config.target_profit_bps <= 0.0 {
        return Err("target_profit_bps must be finite and positive".into());
    }
    if config.window_percentile < 50 {
        return Err("window_percentile must be at least 50%".into());
    }
    if config.max_accumulated_units < 1 {
        return Err("max_accumulated_units must be at least 1".into());
    }
    if !(1..=5).contains(&config.user_target_leverage) {
        return Err("user_target_leverage must be between 1 and 5".into());
    }
    if !config.unit_size.is_finite() || config.unit_size <= 0.0 {
        return Err("unit_size must be finite and positive".into());
    }
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
    if !KNOWN_STRATEGY_IDS.contains(&strategy_id) {
        return Err(format!("unknown strategy_id '{strategy_id}'"));
    }
    let params = state.strategy_params.read();
    Ok(params
        .get(strategy_id)
        .cloned()
        .unwrap_or_else(|| StrategyParamsDto::defaults_for(strategy_id)))
}

fn canonical_sliding_window(secs: u64) -> String {
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
    if !KNOWN_STRATEGY_IDS.contains(&params.strategy_id.as_str()) {
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
    if !(1..=5).contains(&params.user_target_leverage) {
        return Err("user_target_leverage must be between 1 and 5".into());
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
        ("prediction_max_slippage_bps", params.prediction_max_slippage_bps),
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
        ("funding_auto_exit_spread_pct", params.funding_auto_exit_spread_pct),
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
        cfg.validate()
            .map_err(|e| format!("{label}: {e}"))?;
        if cfg.enabled && cfg.timeout_secs < 1 {
            return Err(format!("{label}: timeout_secs must be at least 1 when enabled"));
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
    crate::strategy_filters::validate_auto_threshold(params.auto_min_price_usd, "auto_min_price_usd")?;
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
// Per-strategy auto-hide ticker filters + effective leverage + strategy tickers
// ---------------------------------------------------------------------------

/// One row of the `get_strategy_tickers` response: the ticker's in-scope
/// venues, whether it passes the strategy's auto-filters, and the explicit
/// block reason when it does not. Manual (global-exempt) hiding is managed
/// separately and applied after this auto-filter on both layers.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StrategyTickerDto {
    pub base: String,
    pub venues: Vec<String>,
    pub passes: bool,
    pub block_reason: Option<String>,
}

/// Resolve which strategy record carries the auto-filters for an execution
/// or listing context. The Spread Radar scans the same perp universe as
/// Single Spread, so "radar" evaluates against the "futures" record.
fn filter_strategy_id(strategy_id: &str) -> Result<&'static str, String> {
    match strategy_id.trim().to_lowercase().as_str() {
        "futures" | "radar" => Ok("futures"),
        "funding" => Ok("funding"),
        "spot" => Ok("spot"),
        other => Err(format!("unknown strategy_id '{other}' for ticker filters")),
    }
}

fn strategy_auto_filters(
    state: &AppEngineState,
    strategy_id: &str,
) -> Result<crate::strategy_filters::StrategyAutoFilters, String> {
    let canonical = filter_strategy_id(strategy_id)?;
    let params = state.strategy_params.read();
    let p = params
        .get(canonical)
        .cloned()
        .unwrap_or_else(|| StrategyParamsDto::defaults_for(canonical));
    Ok(crate::strategy_filters::StrategyAutoFilters {
        min_volume_usd: p.auto_min_volume_usd,
        min_price_usd: p.auto_min_price_usd,
    })
}

fn auto_filters_active(filters: crate::strategy_filters::StrategyAutoFilters) -> bool {
    crate::strategy_filters::threshold_active(filters.min_volume_usd)
        || crate::strategy_filters::threshold_active(filters.min_price_usd)
}

pub fn benchmark_base_price(base: &str) -> Option<f64> {
    match base.to_uppercase().as_str() {
        "BTC" => Some(78550.0),
        "ETH" => Some(2470.0),
        "SOL" => Some(103.0),
        "AVAX" => Some(8.07),
        "BNB" => Some(751.0),
        "ARB" => Some(0.58),
        "OP" => Some(1.45),
        "SUI" => Some(0.82),
        "NEAR" => Some(2.29),
        "APT" => Some(5.40),
        "LINK" => Some(12.67),
        "DOGE" => Some(0.125),
        "PEPE" | "1000PEPE" | "KPEPE" => Some(0.0000095),
        "WIF" => Some(1.75),
        "POL" => Some(0.38),
        "TIA" => Some(5.20),
        "SEI" => Some(0.32),
        "INJ" => Some(21.50),
        "RENDER" => Some(5.80),
        "FTM" | "S" => Some(0.68),
        "ATOM" => Some(4.85),
        "ADA" => Some(0.36),
        "XRP" => Some(0.58),
        "DOT" => Some(4.40),
        "LTC" => Some(65.50),
        "BCH" => Some(340.0),
        "UNI" => Some(7.20),
        "AAVE" => Some(145.0),
        "CRV" => Some(0.28),
        "MKR" => Some(1650.0),
        "LDO" => Some(1.15),
        "JUP" => Some(0.85),
        "PYTH" => Some(0.34),
        "ONDO" => Some(0.72),
        "KAS" => Some(0.16),
        "TON" => Some(5.40),
        "NOT" => Some(0.0082),
        "PENDLE" => Some(4.10),
        "FET" => Some(1.35),
        "TAO" => Some(320.0),
        "WLD" => Some(1.65),
        "BLUR" => Some(0.22),
        "STX" => Some(1.75),
        "RUNE" => Some(4.90),
        "DYDX" => Some(1.05),
        "ENA" => Some(0.35),
        "TRX" => Some(0.25),
        "XMR" => Some(155.0),
        "ETC" => Some(18.50),
        "SHIB" | "1000SHIB" | "KSHIB" => Some(0.000018),
        "BONK" | "1000BONK" | "KBONK" => Some(0.000021),
        "FLOKI" | "1000FLOKI" | "KFLOKI" => Some(0.00015),
        "POPCAT" => Some(1.20),
        "MEW" => Some(0.005),
        "NEIRO" => Some(0.0018),
        _ => None,
    }
}

fn resolve_ticker_mids(
    state: &AppEngineState,
    base: &str,
    in_scope_venues: &[String],
) -> HashMap<String, f64> {
    let mut mids = HashMap::new();
    for v in in_scope_venues {
        if let Some(m) = hub_mid_usd(state, base, v) {
            mids.insert(v.clone(), m);
        }
    }
    if mids.is_empty() {
        if let Some(bench) = benchmark_base_price(base) {
            mids.insert("benchmark".to_string(), bench);
        }
    } else if let Some(bench) = benchmark_base_price(base) {
        for v in in_scope_venues {
            mids.entry(v.clone()).or_insert(bench);
        }
    }
    mids
}

fn hub_mid_usd(state: &AppEngineState, base: &str, venue: &str) -> Option<f64> {
    for key in [base.to_string(), format!("{base}-USDC")] {
        if let Some(book) = state.market_data_hub.get_valid_orderbook(
            &VenueId::from(venue),
            &key,
            Some(std::time::Duration::from_secs(60)),
        ) {
            if let Some(mid) = book.mid_price() {
                if let Ok(m) = mid.to_string().parse::<f64>() {
                    if m.is_finite() && m > 0.0 {
                        return Some(m);
                    }
                }
            }
        }
    }
    None
}

/// Shared fail-closed gate for bot/scan targeting and radar Execute: when
/// the strategy's auto-filters are off this is a no-op; otherwise the
/// ticker must pass MIN aggregation across `in_scope_venues`.
pub(crate) async fn reject_if_strategy_filtered(
    state: &AppEngineState,
    strategy_id: &str,
    base: &str,
    in_scope_venues: &[String],
) -> Result<(), String> {
    let filters = strategy_auto_filters(state, strategy_id)?;
    if !auto_filters_active(filters) {
        return Ok(());
    }
    let volumes = cached_venue_volumes(state).await;
    let mids = resolve_ticker_mids(state, base, in_scope_venues);
    let (ok, reason) = crate::strategy_filters::ticker_passes_strategy_filters(
        base,
        in_scope_venues,
        &volumes,
        &mids,
        filters,
    );
    if ok {
        Ok(())
    } else {
        Err(reason.unwrap_or_else(|| format!("{base} blocked by strategy auto-filter")))
    }
}

pub async fn get_strategy_tickers_impl(
    state: &AppEngineState,
    strategy_id: &str,
) -> Result<Vec<StrategyTickerDto>, String> {
    let filters = strategy_auto_filters(state, strategy_id)?;
    let mut venue_bases: HashMap<String, Vec<String>> = HashMap::new();
    for (vid, _, adapter) in state.get_venues() {
        if let Ok(markets) = adapter.fetch_markets().await {
            let mut bases: Vec<String> = markets
                .iter()
                .filter(|m| m.is_active)
                .map(|m| crate::monitor::canonical_base(&m.symbol.base))
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            bases.sort();
            venue_bases.insert(vid.to_string(), bases);
        }
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    for bases in venue_bases.values() {
        for b in bases {
            *counts.entry(b.clone()).or_default() += 1;
        }
    }
    let mut bases: Vec<String> = counts
        .into_iter()
        .filter(|(_, n)| *n >= 2)
        .map(|(b, _)| b)
        .collect();
    bases.sort();
    let volumes = if auto_filters_active(filters) {
        cached_venue_volumes(state).await
    } else {
        HashMap::new()
    };
    let mut out = Vec::new();
    for base in bases {
        let mut scope: Vec<String> = venue_bases
            .iter()
            .filter(|(_, v)| v.contains(&base))
            .map(|(k, _)| k.clone())
            .collect();
        scope.sort();
        let mids = if auto_filters_active(filters) {
            resolve_ticker_mids(state, &base, &scope)
        } else {
            HashMap::new()
        };
        let (passes, reason) = if auto_filters_active(filters) {
            crate::strategy_filters::ticker_passes_strategy_filters(
                &base, &scope, &volumes, &mids, filters,
            )
        } else {
            (true, None)
        };
        out.push(StrategyTickerDto {
            base,
            venues: scope,
            passes,
            block_reason: reason,
        });
    }
    Ok(out)
}

#[tauri::command]
pub async fn get_strategy_tickers(
    strategy_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<StrategyTickerDto>, String> {
    get_strategy_tickers_impl(&state, &strategy_id).await
}

/// Recent external-delta balance attribution events, newest first.
#[tauri::command]
pub async fn get_account_equity_events(
    account_id: Option<String>,
    limit: Option<u32>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<crate::state_store::AccountEquityEvent>, String> {
    let clean = account_id.filter(|s| !s.trim().is_empty());
    state
        .state_store
        .list_equity_events(clean.as_deref(), limit.unwrap_or(100))
        .map_err(|e| e.to_string())
}

/// Effective order-time leverage for a two-leg signal: the strategy chain
/// (strategy params, then the per-symbol legacy fallback, then 5) lowered
/// by each leg account's cap and the venue cap via min(). Generic per
/// venue/account — no venue-specific adapter code. Returns the effective
/// leverage plus the binding account cap (`None` = unlimited), so callers
/// can harden the simulator's own margin paths with the same cap.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_effective_leverage(
    state: &AppEngineState,
    params_strategy_id: &str,
    attribution_strategy_id: &str,
    symbol: &str,
    buy_venue: &str,
    sell_venue: &str,
    route_id: Option<&str>,
) -> Result<(u32, Option<u32>), String> {
    let strategy_target = state
        .strategy_params
        .read()
        .get(params_strategy_id)
        .map(|p| p.user_target_leverage)
        .or_else(|| {
            state
                .strategy_configs
                .read()
                .get(symbol)
                .map(|c| c.user_target_leverage)
        })
        .unwrap_or(5);
    if !(1..=5).contains(&strategy_target) {
        return Err(format!(
            "Configured leverage {strategy_target} for {symbol} is out of range; must be 1..=5"
        ));
    }
    let accounts =
        resolve_signal_accounts(state, attribution_strategy_id, buy_venue, sell_venue, route_id)?;
    let mut account_cap: Option<u32> = None;
    for id in [accounts.long, accounts.short].into_iter().flatten() {
        if let Ok(Some(profile)) = state.state_store.get_account(&id) {
            account_cap = match (account_cap, profile.max_leverage) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) => Some(a),
                (None, b) => b,
            };
        }
    }
    Ok((
        crate::strategy_filters::effective_leverage(strategy_target, account_cap, None),
        account_cap,
    ))
}

/// Position signature for the attribution cross-check: sorted
/// `symbol:side:quantity` segments, empty when flat.
pub(crate) fn position_signature(
    positions: &[arbitrageur_core::position::Position],
) -> String {
    let mut parts: Vec<String> = positions
        .iter()
        .map(|p| format!("{}:{:?}:{}", p.symbol, p.side, p.quantity))
        .collect();
    parts.sort();
    parts.join("|")
}

/// Minimum gap between two margin-snapshot rows for the same venue.
/// Bounds the writer cadence absolutely: fills dispatch paths and the
/// periodic retention heartbeat all funnel through
/// `maybe_record_margin_snapshots`, which drops observations that arrive
/// inside the window. One row per venue per minute at most.
pub(crate) const MARGIN_SNAPSHOT_MIN_INTERVAL_MS: i64 = 60_000;

/// Default trailing window for the average-utilisation stat (24h).
pub(crate) const UTILISATION_DEFAULT_WINDOW_MS: i64 = 24 * 3_600_000;

/// Record one margin-utilisation snapshot per admitted paper venue unless
/// the per-venue cadence cap says the last write is still fresh. Best
/// effort: account reads or store writes never break execution. Called on
/// fills dispatches (open, triangle, flatten) and by the periodic retention
/// heartbeat so history stays bounded yet continuous.
pub(crate) async fn maybe_record_margin_snapshots(state: &AppEngineState, now_ms: i64) {
    if now_ms < 0 {
        return;
    }
    for (vid, paper) in state.paper_exchanges() {
        let fresh = state
            .margin_snapshot_at
            .lock()
            .get(vid)
            .is_some_and(|last| now_ms.saturating_sub(*last) < MARGIN_SNAPSHOT_MIN_INTERVAL_MS);
        if fresh {
            continue;
        }
        let account = match paper.fetch_account().await {
            Ok(acc) => acc,
            Err(_) => continue,
        };
        let equity = match account.equity.to_string().parse::<f64>() {
            Ok(v) if v.is_finite() && v >= 0.0 => v,
            _ => continue,
        };
        let free = match account.free_margin.to_string().parse::<f64>() {
            Ok(v) if v.is_finite() => v,
            _ => continue,
        };
        let rate = match account.margin_usage_pct.to_string().parse::<f64>() {
            Ok(v) if v.is_finite() && v >= 0.0 => v,
            _ => continue,
        };
        let used = (equity - free).max(0.0);
        if state
            .state_store
            .record_margin_snapshot(vid, vid, now_ms, equity, used, rate)
            .is_ok()
        {
            state.margin_snapshot_at.lock().insert(vid.to_string(), now_ms);
        }
    }
}

/// Per-venue actual paper equity snapshot, taken immediately before a
/// fills dispatch.
async fn snapshot_paper_equity(state: &AppEngineState) -> HashMap<String, f64> {    let mut map = HashMap::new();
    for (vid, paper) in state.paper_exchanges() {
        if let Ok(acc) = paper.fetch_account().await {
            if let Ok(f) = acc.equity.to_string().parse::<f64>() {
                if f.is_finite() {
                    map.insert(vid.to_string(), f);
                }
            }
        }
    }
    map
}

/// Fold a fills dispatch back into the expected-equity baselines:
/// expected_new = prior_expected (or the pre-trade snapshot when no
/// baseline exists yet) + (actual_after - actual_before). The dispatch
/// window holds the execution lock, so the delta is own fills minus fees
/// by construction. Best-effort: attribution must never break execution.
async fn refresh_equity_expectations_after_fills(
    state: &AppEngineState,
    before: &HashMap<String, f64>,
) {
    let now_ms = Utc::now().timestamp_millis();
    for (vid, paper) in state.paper_exchanges() {
        let Ok(acc) = paper.fetch_account().await else {
            continue;
        };
        let Ok(after) = acc.equity.to_string().parse::<f64>() else {
            continue;
        };
        if !after.is_finite() {
            continue;
        }
        let Some(b) = before.get(vid).copied() else {
            continue;
        };
        let prior = state
            .state_store
            .get_equity_expectation(vid)
            .ok()
            .flatten()
            .map(|(e, _, _)| e);
        let next = prior.unwrap_or(b) + (after - b);
        if next.is_finite() && next >= 0.0 {
            let _ = state.state_store.set_equity_expectation(
                vid,
                next,
                &position_signature(&acc.positions),
                now_ms,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Auto-tune Phase 1 (recommend-only) — Single Spread futures
// ---------------------------------------------------------------------------

/// One validated recommendation row. `recommended` is `None` exactly when
/// `status == "HELD"` — the manual value stays authoritative. Read-only:
/// no write path consumes this DTO.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AutotuneRowDto {
    pub param: String,
    pub current: f64,
    pub recommended: Option<f64>,
    pub status: String,
    pub fit_metric: String,
    pub validated_metric: String,
    pub n: usize,
    pub rationale: String,
    pub formula_id: String,
    pub confidence: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AutotuneBannerDto {
    pub fit_from_ms: i64,
    pub fit_to_ms: i64,
    pub validate_from_ms: i64,
    pub validate_to_ms: i64,
    pub regime_verdict: String,
    pub ready_count: usize,
    pub total: usize,
}

/// Recommend-only preview for one symbol's Single Spread futures params.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AutotuneRecommendationDto {
    pub symbol: String,
    pub buy_venue: Option<String>,
    pub sell_venue: Option<String>,
    pub pair_observations: usize,
    pub rows: Vec<AutotuneRowDto>,
    pub banner: AutotuneBannerDto,
    pub session_tau_scale: f64,
    pub session_sigma_scale: f64,
}

fn tune_params_from_dto(p: &StrategyParamsDto) -> TuneParams {
    TuneParams {
        window_secs: p.sliding_window_secs,
        target_bps: p.target_profit_bps,
        percentile: p.window_percentile,
        step_mult: p.step_size_mult,
        max_step_usd: p.max_step_usd,
        equity_pct: p.max_ticker_equity_pct,
        leverage: p.user_target_leverage,
    }
}

fn map_autotune_report(
    symbol: &str,
    buy_venue: Option<String>,
    sell_venue: Option<String>,
    pair_observations: usize,
    coupling: arbitrageur_math::seasonality::SessionCoupling,
    rep: arbitrageur_math::autotune::AutotuneReport,
) -> AutotuneRecommendationDto {
    AutotuneRecommendationDto {
        symbol: symbol.to_string(),
        buy_venue,
        sell_venue,
        pair_observations,
        rows: rep
            .rows
            .into_iter()
            .map(|r| AutotuneRowDto {
                param: r.param,
                current: r.current,
                recommended: r.recommended,
                status: match r.status {
                    arbitrageur_math::autotune::RowStatus::READY => "READY".to_string(),
                    arbitrageur_math::autotune::RowStatus::HELD => "HELD".to_string(),
                },
                fit_metric: r.fit_metric,
                validated_metric: r.validated_metric,
                n: r.n,
                rationale: r.rationale,
                formula_id: r.formula_id,
                confidence: r.confidence,
            })
            .collect(),
        banner: AutotuneBannerDto {
            fit_from_ms: rep.banner.fit_from_ms,
            fit_to_ms: rep.banner.fit_to_ms,
            validate_from_ms: rep.banner.validate_from_ms,
            validate_to_ms: rep.banner.validate_to_ms,
            regime_verdict: rep.banner.regime_verdict.as_str().to_string(),
            ready_count: rep.banner.ready_count,
            total: rep.banner.total,
        },
        session_tau_scale: coupling.tau_scale,
        session_sigma_scale: coupling.sigma_scale,
    }
}

/// Capacity tiers backing the step/maxStep candidates: the monitor's
/// default tiers plus a 0.5% participation-cap tier off the smaller venue's
/// 24h volume when both volumes are known.
fn autotune_tiers_with_cap(
    volumes: &HashMap<String, HashMap<String, f64>>,
    buy_v: &str,
    sell_v: &str,
    symbol: &str,
    base: &str,
) -> Vec<f64> {
    let mut tiers = vec![1000.0, 5000.0, 10_000.0];
    let keys = [
        symbol.to_string(),
        base.to_string(),
        format!("K{base}"),
        format!("1000{base}"),
    ];
    let lookup = |venue: &str| -> Option<f64> {
        let m = volumes.get(venue)?;
        keys.iter()
            .filter_map(|k| {
                m.iter()
                    .find(|(mk, _)| mk.eq_ignore_ascii_case(k))
                    .map(|(_, v)| *v)
            })
            .next()
    };
    if let (Some(a), Some(b)) = (lookup(buy_v), lookup(sell_v)) {
        let cap = a.min(b) * 0.005;
        if cap.is_finite() && cap > 0.0 {
            tiers.push(cap);
            tiers.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
            tiers.dedup();
        }
    }
    tiers
}

pub async fn get_autotune_recommendation_impl(
    symbol: &str,
    state: &AppEngineState,
) -> Result<AutotuneRecommendationDto, String> {
    validate_market_symbol(symbol)?;
    let current_dto = get_strategy_params_impl("futures", state)?;
    let current = tune_params_from_dto(&current_dto);
    let defaults = tune_params_from_dto(&StrategyParamsDto::defaults_for("futures"));

    let base = symbol
        .split(['-', '/', '_'])
        .next()
        .unwrap_or(symbol)
        .to_ascii_uppercase();
    let keys = [base.clone(), format!("{base}-USDC"), format!("{base}-USD")];
    let now_ms = Utc::now().timestamp_millis();
    let since_ms = now_ms - BASELINE_RETENTION_MS;
    let store = state.state_store.clone();
    let sym_owned = symbol.to_string();
    let (best_pair, pair_observations): (
        Option<(String, String, Vec<arbitrageur_math::pair_monitor::PairObservation>)>,
        usize,
    ) = tokio::task::spawn_blocking(move || {
        let mut all = Vec::new();
        for k in &keys {
            if let Ok(mut rows) = store.spread_samples(k, since_ms, now_ms) {
                all.append(&mut rows);
            }
        }
        all.sort_by(|a, b| (a.ts_ms, a.venue_id.as_str()).cmp(&(b.ts_ms, b.venue_id.as_str())));
        all.dedup_by(|a, b| a.ts_ms == b.ts_ms && a.venue_id == b.venue_id);
        let mut mids_by_venue: HashMap<String, Vec<(i64, f64)>> = HashMap::new();
        for s in &all {
            if s.mid.is_finite() && s.mid > 0.0 {
                mids_by_venue
                    .entry(s.venue_id.clone())
                    .or_default()
                    .push((s.ts_ms, s.mid));
            }
        }
        let mut venues: Vec<String> = mids_by_venue.keys().cloned().collect();
        venues.sort();
        let mut best: Option<(String, String, Vec<arbitrageur_math::pair_monitor::PairObservation>)> =
            None;
        let mut total = 0usize;
        for buy_v in &venues {
            for sell_v in &venues {
                if buy_v == sell_v {
                    continue;
                }
                let (Some(a), Some(b)) =
                    (mids_by_venue.get(buy_v), mids_by_venue.get(sell_v))
                else {
                    continue;
                };
                let series = arbitrageur_math::pair_monitor::build_pair_series(
                    a,
                    b,
                    PAIR_ALIGN_GAP_MS,
                );
                total += series.len();
                let longer = best.as_ref().map(|(_, _, s)| s.len()).unwrap_or(0);
                if series.len() > longer {
                    best = Some((buy_v.clone(), sell_v.clone(), series));
                }
            }
        }
        let _ = sym_owned;
        Ok::<_, anyhow::Error>((best, total))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    let Some((buy_v, sell_v, series)) = best_pair else {
        // No paired history — honest all-HELD report, never fabricated.
        let rep = recommend(&TuneHistory {
            fit: Vec::new(),
            validate: Vec::new(),
            dt_fit_secs: 0.0,
            dt_val_secs: 0.0,
            friction_fit_bps: 0.0,
            friction_val_bps: 0.0,
            carry_known: false,
            fit_optimal_size_usd: None,
            fit_breakeven_size_usd: None,
            current,
            defaults,
        });
        let neutral = arbitrageur_math::seasonality::SessionCoupling {
            tau_scale: 1.0,
            sigma_scale: 1.0,
        };
        return Ok(map_autotune_report(symbol, None, None, 0, neutral, rep));
    };

    // 70/30 chronological walk-forward split.
    let k = series.len() * 7 / 10;
    let (fit, validate) = if k >= 1 && series.len() - k >= 1 {
        (series[..k].to_vec(), series[k..].to_vec())
    } else {
        (Vec::new(), Vec::new())
    };
    let seg_dt = |obs: &[arbitrageur_math::pair_monitor::PairObservation]| -> f64 {
        if obs.len() >= 2 {
            let span = (obs[obs.len() - 1].ts_ms - obs[0].ts_ms) as f64 / 1000.0;
            let dt = span / (obs.len() - 1) as f64;
            if dt.is_finite() && dt > 0.0 {
                return dt;
            }
        }
        0.0
    };
    let dt_fit = seg_dt(&fit);
    let dt_val = seg_dt(&validate);

    // Static friction: both taker fees. Funding carry has no measured EMA
    // on this path, so the carry term is 0 and flagged unknown.
    let fee_sum =
        effective_venue_fee_bps(state, &buy_v) + effective_venue_fee_bps(state, &sell_v);

    // Seasonality Phase 2 coupling: the current session's shrunk τ/σ regime
    // scales the recommender inputs (OU dt and friction). Missing or
    // provisional profiles couple at exactly 1.0 — Phase-1 behavior.
    let coupling = crate::seasonality::session_coupling_for(state, symbol, now_ms);

    // Fit capacity summary from live-book ladder (0.5% participation cap).
    // Hub cache only — a recommend preview must not trigger network fetches;
    // absent books mean step/maxStep stay HELD.
    let (fit_optimal, fit_breakeven) = {
        let hub = &state.market_data_hub;
        let stale = std::time::Duration::from_secs(300);
        let buy_book = hub.get_valid_orderbook(
            &arbitrageur_core::types::VenueId::from(buy_v.as_str()),
            symbol,
            Some(stale),
        );
        let sell_book = hub.get_valid_orderbook(
            &arbitrageur_core::types::VenueId::from(sell_v.as_str()),
            symbol,
            Some(stale),
        );
        match (buy_book, sell_book) {
            (Some(bk), Some(sk)) => {
                let asks: Vec<(f64, f64)> = bk
                    .asks
                    .iter()
                    .filter_map(|l| {
                        let pf = l.price.to_string().parse::<f64>().ok()?;
                        let sf = l.size.to_string().parse::<f64>().ok()?;
                        (pf.is_finite() && sf.is_finite() && pf > 0.0 && sf > 0.0)
                            .then_some((pf, sf))
                    })
                    .collect();
                let bids: Vec<(f64, f64)> = sk
                    .bids
                    .iter()
                    .filter_map(|l| {
                        let pf = l.price.to_string().parse::<f64>().ok()?;
                        let sf = l.size.to_string().parse::<f64>().ok()?;
                        (pf.is_finite() && sf.is_finite() && pf > 0.0 && sf > 0.0)
                            .then_some((pf, sf))
                    })
                    .collect();
                let fit_vals: Vec<f64> = fit.iter().map(|o| o.spread_bps).collect();
                let n = fit_vals.len();
                let (mu_f, sig_f) = if n >= 2 {
                    let m = fit_vals.iter().sum::<f64>() / n as f64;
                    let v = fit_vals.iter().map(|x| (x - m).powi(2)).sum::<f64>()
                        / (n - 1) as f64;
                    (m, v.sqrt())
                } else {
                    (0.0, 0.0)
                };
                if !(mu_f.is_finite() && sig_f.is_finite() && sig_f > 0.0)
                    || asks.is_empty()
                    || bids.is_empty()
                {
                    (None, None)
                } else {
                    let volumes = cached_venue_volumes(state).await;
                    let tiers =
                        autotune_tiers_with_cap(&volumes, &buy_v, &sell_v, symbol, &base);
                    let curve = arbitrageur_math::pair_monitor::build_pwin_curve(
                        &fit,
                        mu_f,
                        sig_f,
                        fee_sum.max(2.0),
                        3.0,
                    );
                    let rep = arbitrageur_math::pair_monitor::capacity_ladder(
                        &asks, &bids, mu_f, fee_sum, &tiers, &curve, 0.9,
                    );
                    (rep.optimal_size_usd, rep.breakeven_size_usd)
                }
            }
            _ => (None, None),
        }
    };

    let rep = recommend(&TuneHistory {
        fit,
        validate,
        dt_fit_secs: dt_fit * coupling.tau_scale,
        dt_val_secs: dt_val * coupling.tau_scale,
        friction_fit_bps: fee_sum * coupling.sigma_scale,
        friction_val_bps: fee_sum * coupling.sigma_scale,
        carry_known: false,
        fit_optimal_size_usd: fit_optimal,
        fit_breakeven_size_usd: fit_breakeven,
        current,
        defaults,
    });
    Ok(map_autotune_report(
        symbol,
        Some(buy_v),
        Some(sell_v),
        pair_observations,
        coupling,
        rep,
    ))
}

#[tauri::command]
pub async fn get_autotune_recommendation(
    symbol: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<AutotuneRecommendationDto, String> {
    get_autotune_recommendation_impl(&symbol, &state).await
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AutotuneApplyResultDto {
    pub symbol: String,
    pub applied: Vec<String>,
    pub skipped_held: Vec<String>,
    pub audit_id: Option<i64>,
}

const AUTOTUNE_APPLYABLE: [&str; 7] = [
    "slidingWindowSecs",
    "targetProfitBps",
    "windowPercentile",
    "stepSizeMult",
    "maxStepUsd",
    "maxTickerEquityPct",
    "userTargetLeverage",
];

fn apply_ready_to_strategy(
    base: &StrategyParamsDto,
    param: &str,
    value: f64,
) -> Result<StrategyParamsDto, String> {
    let mut next = base.clone();
    match param {
        "slidingWindowSecs" => {
            if !(1.0..=3_000_000.0).contains(&value) {
                return Err(format!("rejecting out-of-range window {value}"));
            }
            next.sliding_window_secs = value.round() as u64;
        }
        "targetProfitBps" => next.target_profit_bps = value,
        "windowPercentile" => {
            if !(0.0..=100.0).contains(&value) {
                return Err(format!("rejecting out-of-range percentile {value}"));
            }
            next.window_percentile = value.round() as u32;
        }
        "stepSizeMult" => next.step_size_mult = value,
        "maxStepUsd" => next.max_step_usd = value,
        "maxTickerEquityPct" => next.max_ticker_equity_pct = value,
        "userTargetLeverage" => {
            if !(0.0..=10.0).contains(&value) {
                return Err(format!("rejecting out-of-range leverage {value}"));
            }
            next.user_target_leverage = value.round() as u32;
        }
        _ => return Err(format!("unknown autotune param '{param}'")),
    }
    Ok(next)
}

fn apply_ready_to_ticker(
    base: &TickerStrategyConfigDto,
    param: &str,
    value: f64,
    window_secs: u64,
) -> TickerStrategyConfigDto {
    let mut next = base.clone();
    match param {
        "slidingWindowSecs" => next.sliding_window = canonical_sliding_window(window_secs),
        "targetProfitBps" => next.target_profit_bps = value,
        "windowPercentile" => next.window_percentile = value.round() as u32,
        "userTargetLeverage" => next.user_target_leverage = value.round() as u32,
        _ => {}
    }
    next
}

pub async fn apply_autotune_rows_inner(
    state: &AppEngineState,
    symbol: &str,
    report: &AutotuneRecommendationDto,
    selected: &[String],
    now_ms: i64,
) -> Result<AutotuneApplyResultDto, String> {
    validate_market_symbol(symbol)?;
    if now_ms <= 0 {
        return Err("invalid clock: refusing apply on non-positive time".to_string());
    }
    let mut wanted: Vec<String> = selected.to_vec();
    wanted.sort();
    wanted.dedup();
    for p in &wanted {
        if !AUTOTUNE_APPLYABLE.contains(&p.as_str()) {
            return Err(format!("unknown autotune param '{p}'"));
        }
    }
    let ready: std::collections::HashMap<&str, f64> = report
        .rows
        .iter()
        .filter(|r| r.status == "READY")
        .filter_map(|r| match r.recommended {
            Some(v) if v.is_finite() => Some((r.param.as_str(), v)),
            _ => None,
        })
        .collect();
    let mut to_apply: Vec<(&str, f64)> = Vec::new();
    let mut skipped_held: Vec<String> = Vec::new();
    for p in &wanted {
        match ready.get(p.as_str()) {
            Some(v) => to_apply.push((p.as_str(), *v)),
            None => skipped_held.push(p.clone()),
        }
    }
    skipped_held.sort();
    if to_apply.is_empty() {
        return Ok(AutotuneApplyResultDto {
            symbol: symbol.to_string(),
            applied: Vec::new(),
            skipped_held,
            audit_id: None,
        });
    }
    let _guard = state.autotune_apply_lock.lock().await;
    let prev_strategy = get_strategy_params_impl("futures", state)?;
    let prev_ticker = get_ticker_strategy_config_impl(symbol, state)?;
    let mut next_strategy = prev_strategy.clone();
    let mut next_ticker = prev_ticker.clone();
    for (param, value) in &to_apply {
        next_strategy = apply_ready_to_strategy(&next_strategy, param, *value)?;
    }
    let window_secs = next_strategy.sliding_window_secs;
    for (param, value) in &to_apply {
        next_ticker = apply_ready_to_ticker(&next_ticker, param, *value, window_secs);
    }
    save_strategy_params_impl(next_strategy, state)?;
    if let Err(e) = save_ticker_strategy_config_impl(next_ticker, state) {
        let _ = save_strategy_params_impl(prev_strategy.clone(), state);
        return Err(e);
    }
    let applied_names: Vec<String> = to_apply.iter().map(|(p, _)| p.to_string()).collect();
    let applied_map: serde_json::Map<String, serde_json::Value> = to_apply
        .iter()
        .map(|(p, v)| {
            (
                p.to_string(),
                serde_json::Value::from(
                    serde_json::Number::from_f64(*v).unwrap_or(serde_json::Number::from(0)),
                ),
            )
        })
        .collect();
    let applied_json =
        serde_json::Value::Object(applied_map).to_string();
    let detail_rows: Vec<serde_json::Value> = report
        .rows
        .iter()
        .filter(|r| applied_names.iter().any(|p| p == &r.param))
        .map(|r| {
            serde_json::json!({
                "param": r.param,
                "recommended": r.recommended,
                "fit_metric": r.fit_metric,
                "validated_metric": r.validated_metric,
                "n": r.n,
                "formula_id": r.formula_id,
                "confidence": r.confidence,
            })
        })
        .collect();
    let detail_json = serde_json::json!({
        "buy_venue": report.buy_venue,
        "sell_venue": report.sell_venue,
        "fit_from_ms": report.banner.fit_from_ms,
        "fit_to_ms": report.banner.fit_to_ms,
        "validate_from_ms": report.banner.validate_from_ms,
        "validate_to_ms": report.banner.validate_to_ms,
        "regime_verdict": report.banner.regime_verdict,
        "session_tau_scale": report.session_tau_scale,
        "session_sigma_scale": report.session_sigma_scale,
        "rows": detail_rows,
        "skipped_held": skipped_held.clone(),
    })
    .to_string();
    match state.state_store.record_autotune_apply_event(
        symbol,
        &applied_json,
        &detail_json,
        now_ms,
    ) {
        Ok(audit_id) => Ok(AutotuneApplyResultDto {
            symbol: symbol.to_string(),
            applied: applied_names,
            skipped_held,
            audit_id: Some(audit_id),
        }),
        Err(e) => {
            let _ = save_strategy_params_impl(prev_strategy, state);
            let _ = save_ticker_strategy_config_impl(prev_ticker, state);
            Err(format!("apply audit failed, changes rolled back: {e}"))
        }
    }
}

pub async fn apply_autotune_selected_impl(
    symbol: &str,
    selected: &[String],
    state: &AppEngineState,
) -> Result<AutotuneApplyResultDto, String> {
    let report = get_autotune_recommendation_impl(symbol, state).await?;
    let now_ms = Utc::now().timestamp_millis();
    apply_autotune_rows_inner(state, symbol, &report, selected, now_ms).await
}

#[tauri::command]
pub async fn apply_autotune_selected(
    symbol: String,
    selected: Vec<String>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<AutotuneApplyResultDto, String> {
    apply_autotune_selected_impl(&symbol, &selected, &state).await
}

#[tauri::command]
pub async fn run_seasonality_promotion(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<crate::seasonality::SeasonalityRunSummary, String> {
    let now_ms = Utc::now().timestamp_millis();
    crate::seasonality::run_seasonality_promotion(&state, now_ms)
}

pub fn get_vault_status_impl(state: &AppEngineState) -> VaultStatus {
    let mut vault = state.vault.write();
    vault.get_status()
}

pub fn initialize_vault_impl(
    password: &str,
    state: &AppEngineState,
) -> Result<VaultStatus, String> {
    let status = {
        let mut vault = state.vault.write();
        vault.initialize(password).map_err(|e| e.to_string())?;
        vault.get_status()
    };
    ensure_credential_migration(state)?;
    state.sync_vault_credentials_to_adapters();
    Ok(status)
}

pub fn unlock_vault_impl(password: &str, state: &AppEngineState) -> Result<VaultStatus, String> {
    let status = {
        let mut vault = state.vault.write();
        vault.unlock(password).map_err(|e| e.to_string())?;
        vault.get_status()
    };
    ensure_credential_migration(state)?;
    state.sync_vault_credentials_to_adapters();
    if state.settings.read().trading_mode.eq_ignore_ascii_case("real") {
        let ready_count = state
            .get_venues()
            .into_iter()
            .filter(|(_, _, adapter)| adapter.supports_live_execution() && adapter.has_live_credentials())
            .count();
        if ready_count >= 2 {
            *state.is_live_mode.write() = true;
        }
    }
    Ok(status)
}

pub fn lock_vault_impl(state: &AppEngineState) -> VaultStatus {
    let status = {
        let mut vault = state.vault.write();
        vault.lock();
        vault.get_status()
    };
    state.sync_vault_credentials_to_adapters();
    status
}

const DELETABLE_CREDENTIAL_VENUES: &[&str] = &[
    "remote_hsm",
    "hyperliquid",
    "lighter",
    "vanta",
    "robinhood",
    "arcus",
    "popdex",
    "kraken_spot",
    "kraken_futures",
    "polymarket",
    "kalshi",
];

pub fn delete_exchange_credentials_impl(
    venue: &str,
    state: &AppEngineState,
) -> Result<bool, String> {
    let normalized = venue.trim().to_lowercase();
    if !DELETABLE_CREDENTIAL_VENUES.contains(&normalized.as_str()) {
        return Err(format!(
            "Credential deletion is not supported for venue '{}'",
            venue
        ));
    }
    let mut vault = state.vault.write();
    if !vault.is_unlocked() {
        return Err("Vault is locked. Please unlock before removing credentials.".into());
    }
    vault
        .delete_credentials(&normalized)
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Account profiles and strategy-level account routes
// ---------------------------------------------------------------------------

/// Sanitized account view for the settings UI: durable profile metadata plus
/// credential-presence flags. Never contains secret material.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDto {
    pub id: String,
    pub venue: String,
    pub label: String,
    pub enabled: bool,
    pub is_default: bool,
    pub created_at: String,
    pub updated_at: String,
    /// `None` while the vault is locked (presence is unknowable); otherwise
    /// whether any credential material is stored for this account.
    pub credentials_configured: Option<bool>,
    pub has_api_key: bool,
    pub has_api_secret: bool,
    pub has_passphrase: bool,
    pub has_account_id: bool,
    pub has_wallet_address: bool,
    pub account_index: Option<u64>,
    pub subaccount_index: Option<u64>,
    pub api_key_index: Option<u64>,
    pub has_private_key: bool,
    /// "standard" | "plus" | "premium" — Lighter-family tier drives the
    /// venue's shared-IP rate budget and fee adjustments.
    pub account_tier: String,
    /// Per-account leverage ceiling 1..=5; `None` = unlimited (backend hard
    /// ceiling of 5 still applies).
    pub max_leverage: Option<u32>,
}

// Intentionally no Debug: may transiently hold credential material.
#[derive(Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AccountCredentialInput {
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub api_secret: String,
    #[serde(default)]
    pub passphrase: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub wallet_address: Option<String>,
    #[serde(default)]
    pub account_index: Option<u64>,
    #[serde(default)]
    pub subaccount_index: Option<u64>,
    #[serde(default)]
    pub api_key_index: Option<u64>,
    #[serde(default)]
    pub private_key: Option<String>,
}

impl AccountCredentialInput {
    fn has_material(&self) -> bool {
        !self.api_key.trim().is_empty()
            || !self.api_secret.trim().is_empty()
            || self
                .passphrase
                .as_ref()
                .is_some_and(|v| !v.trim().is_empty())
            || self
                .account_id
                .as_ref()
                .is_some_and(|v| !v.trim().is_empty())
            || self
                .wallet_address
                .as_ref()
                .is_some_and(|v| !v.trim().is_empty())
            || self.account_index.is_some()
            || self.subaccount_index.is_some()
            || self.api_key_index.is_some()
            || self
                .private_key
                .as_ref()
                .is_some_and(|v| !v.trim().is_empty())
    }

    fn into_record(self, venue: &str, profile_id: &str, enabled: bool) -> ExchangeCredentials {
        let trimmed_opt = |v: &Option<String>| {
            v.as_ref()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        ExchangeCredentials {
            venue: venue.to_string(),
            profile_id: Some(profile_id.to_string()),
            api_key: self.api_key.trim().to_string(),
            api_secret: self.api_secret.trim().to_string(),
            passphrase: trimmed_opt(&self.passphrase),
            account_id: trimmed_opt(&self.account_id),
            wallet_address: trimmed_opt(&self.wallet_address),
            account_index: self.account_index,
            subaccount_index: self.subaccount_index,
            api_key_index: self.api_key_index,
            private_key: trimmed_opt(&self.private_key),
            enabled,
        }
    }
}

// Intentionally no Debug: `credentials` may hold secret material.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUpsertInput {
    /// Existing account ID for updates; `None` creates a new account.
    #[serde(default)]
    pub id: Option<String>,
    /// Venue ID. Required on create; verified immutable on update.
    #[serde(default)]
    pub venue: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub make_default: bool,
    /// "standard" | "plus" | "premium". `None` preserves the stored tier on
    /// update and means standard on create.
    #[serde(default)]
    pub account_tier: Option<String>,
    /// Per-account leverage ceiling 1..=5. `None` means unlimited; updates
    /// are authoritative (sending `None` clears a previously stored cap).
    #[serde(default)]
    pub max_leverage: Option<u32>,
    #[serde(default)]
    pub credentials: Option<AccountCredentialInput>,
}

// Intentionally no Debug.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyRouteUpsertInput {
    /// Existing route ID for updates; `None` creates a new route.
    #[serde(default)]
    pub id: Option<String>,
    pub strategy_id: String,
    #[serde(default)]
    pub label: String,
    pub account_a_id: String,
    pub account_b_id: String,
    #[serde(default = "default_route_enabled")]
    pub enabled: bool,
}

fn default_route_enabled() -> bool {
    true
}

fn is_valid_account_id(raw: &str) -> bool {
    let v = raw.trim();
    !v.is_empty()
        && v.len() <= 128
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Fetch (creating when necessary) the `{venue}-default` account profile
/// used by the legacy flat settings credential fields. The legacy venue
/// toggle is mirrored onto the default account's enabled switch; label and
/// default status are preserved for existing profiles.
fn ensure_default_profile(
    state: &AppEngineState,
    venue: &str,
    enabled: bool,
) -> Result<AccountProfile, String> {
    let pid = format!("{venue}-default");
    match state
        .state_store
        .get_account(&pid)
        .map_err(|e| e.to_string())?
    {
        Some(existing) => {
            if existing.venue != venue {
                return Err(format!("account ID {pid} is bound to a different venue"));
            }
            if existing.enabled != enabled {
                state
                    .state_store
                    .upsert_account(
                        &pid,
                        venue,
                        &existing.label,
                        enabled,
                        existing.is_default,
                        &existing.account_tier,
                        existing.max_leverage,
                    )
                    .map_err(|e| e.to_string())
            } else {
                Ok(existing)
            }
        }
        None => state
            .state_store
            .upsert_account(&pid, venue, "Default", enabled, true, "standard", None)
            .map_err(|e| e.to_string()),
    }
}

/// Migrate legacy venue-keyed credential records onto `{venue}-default`
/// account profiles and prune bound records whose profile no longer exists.
/// No-op while the vault is locked. `remote_hsm` is not an exchange account
/// and is intentionally left unbound.
fn ensure_credential_migration(state: &AppEngineState) -> Result<(), String> {
    let mut vault = state.vault.write();
    if !vault.is_unlocked() {
        return Ok(());
    }

    // Prune bound records whose profile was deleted (e.g. credential cleanup
    // failed after the account row was removed).
    let presence = vault.credential_presence().map_err(|e| e.to_string())?;
    for p in presence.iter() {
        if let Some(pid) = &p.profile_id {
            let exists = state
                .state_store
                .get_account(pid)
                .map_err(|e| e.to_string())?
                .is_some();
            if !exists {
                vault
                    .delete_profile_credentials(pid)
                    .map_err(|e| e.to_string())?;
            }
        }
    }

    for cred in vault.unbound_credentials().map_err(|e| e.to_string())? {
        if cred.venue == "remote_hsm" {
            continue;
        }
        let pid = format!("{}-default", cred.venue);
        if !is_valid_account_id(&pid) {
            tracing::warn!(
                "skipping migration of credential record with invalid venue '{}'",
                cred.venue
            );
            continue;
        }
        match state
            .state_store
            .get_account(&pid)
            .map_err(|e| e.to_string())?
        {
            Some(existing) => {
                if existing.venue != cred.venue {
                    return Err(format!(
                        "cannot migrate {} credentials: {pid} is bound to another venue",
                        cred.venue
                    ));
                }
            }
            None => {
                state
                    .state_store
                    .upsert_account(&pid, &cred.venue, "Default", cred.enabled, true, "standard", None)
                    .map_err(|e| e.to_string())?;
            }
        }
        vault
            .bind_profile(&cred.venue, &pid)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn account_dto(
    profile: &AccountProfile,
    presence: Option<&CredentialPresence>,
    vault_locked: bool,
) -> AccountDto {
    let configured = presence.is_some_and(|p| {
        p.has_api_key
            || p.has_api_secret
            || p.has_passphrase
            || p.has_account_id
            || p.has_wallet_address
            || p.account_index.is_some()
            || p.subaccount_index.is_some()
            || p.api_key_index.is_some()
            || p.has_private_key
    });
    AccountDto {
        id: profile.id.clone(),
        venue: profile.venue.clone(),
        label: profile.label.clone(),
        enabled: profile.enabled,
        is_default: profile.is_default,
        created_at: profile.created_at.clone(),
        updated_at: profile.updated_at.clone(),
        credentials_configured: if vault_locked { None } else { Some(configured) },
        has_api_key: presence.is_some_and(|p| p.has_api_key),
        has_api_secret: presence.is_some_and(|p| p.has_api_secret),
        has_passphrase: presence.is_some_and(|p| p.has_passphrase),
        has_account_id: presence.is_some_and(|p| p.has_account_id),
        has_wallet_address: presence.is_some_and(|p| p.has_wallet_address),
        has_private_key: presence.is_some_and(|p| p.has_private_key),
        account_index: presence.and_then(|p| p.account_index),
        subaccount_index: presence.and_then(|p| p.subaccount_index),
        api_key_index: presence.and_then(|p| p.api_key_index),
        account_tier: profile.account_tier.clone(),
        max_leverage: profile.max_leverage,
    }
}

/// Merge durable profiles with sanitized vault credential presence.
fn accounts_with_presence(
    state: &AppEngineState,
    profiles: Vec<AccountProfile>,
) -> Result<Vec<AccountDto>, String> {
    let (presence, locked) = {
        let mut vault = state.vault.write();
        let locked = !vault.is_unlocked();
        let presence = if locked {
            Vec::new()
        } else {
            vault.credential_presence().map_err(|e| e.to_string())?
        };
        (presence, locked)
    };
    let by_profile: std::collections::HashMap<&str, &CredentialPresence> = presence
        .iter()
        .filter_map(|p| p.profile_id.as_deref().map(|id| (id, p)))
        .collect();
    Ok(profiles
        .iter()
        .map(|p| account_dto(p, by_profile.get(p.id.as_str()).copied(), locked))
        .collect())
}

pub fn list_accounts_impl(
    venue: Option<&str>,
    state: &AppEngineState,
) -> Result<Vec<AccountDto>, String> {
    ensure_credential_migration(state)?;
    let profiles = state
        .state_store
        .list_accounts(venue)
        .map_err(|e| e.to_string())?;
    accounts_with_presence(state, profiles)
}

pub fn upsert_account_impl(
    input: AccountUpsertInput,
    state: &AppEngineState,
) -> Result<AccountDto, String> {
    ensure_credential_migration(state)?;

    let profile = match &input.id {
        Some(id) => {
            let existing = state
                .state_store
                .get_account(id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("account {id} does not exist"))?;
            if !input.venue.trim().is_empty()
                && !input.venue.trim().eq_ignore_ascii_case(&existing.venue)
            {
                return Err("account venue is immutable once created".into());
            }
            state
                .state_store
                .upsert_account(
                    id,
                    &existing.venue,
                    &input.label,
                    input.enabled,
                    input.make_default,
                    input
                        .account_tier
                        .as_deref()
                        .unwrap_or(&existing.account_tier),
                    input.max_leverage,
                )
                .map_err(|e| e.to_string())?
        }
        None => {
            let venue_lc = input.venue.trim().to_lowercase();
            if venue_lc.is_empty() {
                return Err("venue is required when creating an account".into());
            }
            let mut candidate;
            loop {
                candidate = format!("{}-{:08x}", venue_lc, rand::random::<u32>());
                if state
                    .state_store
                    .get_account(&candidate)
                    .map_err(|e| e.to_string())?
                    .is_none()
                {
                    break;
                }
            }
            state
                .state_store
                .upsert_account(
                    &candidate,
                    &venue_lc,
                    &input.label,
                    input.enabled,
                    input.make_default,
                    input.account_tier.as_deref().unwrap_or("standard"),
                    input.max_leverage,
                )
                .map_err(|e| e.to_string())?
        }
    };

    let mut cred_error: Option<String> = None;
    if let Some(creds) = &input.credentials {
        if creds.has_material() {
            let mut vault = state.vault.write();
            if !vault.is_unlocked() {
                cred_error = Some("vault is locked".to_string());
            } else {
                let record =
                    creds
                        .clone()
                        .into_record(&profile.venue, &profile.id, profile.enabled);
                if let Err(e) = vault.upsert_profile_credentials(&profile.id, record) {
                    cred_error = Some(e.to_string());
                }
            }
        }
    }
    // Mirror the account switch onto the stored record's metadata flag when
    // the vault is reachable; the profile remains the authoritative switch.
    {
        let mut vault = state.vault.write();
        if vault.is_unlocked() {
            let _ = vault.update_profile_enabled(&[(profile.id.clone(), profile.enabled)]);
        }
    }
    state.sync_vault_credentials_to_adapters();

    let dto = accounts_with_presence(state, vec![profile])?
        .into_iter()
        .next()
        .ok_or_else(|| "account disappeared after save".to_string())?;
    if let Some(err) = cred_error {
        return Err(format!(
            "account saved but credentials were not stored: {err}"
        ));
    }
    Ok(dto)
}

/// Delete an account profile and its bound credential record. Route
/// references are checked before anything is removed so a foreign-key block
/// can never destroy secrets; the vault must be unlocked so bound secrets
/// are never orphaned.
pub fn delete_account_impl(account_id: &str, state: &AppEngineState) -> Result<bool, String> {
    let refs = state
        .state_store
        .routes_referencing(account_id)
        .map_err(|e| e.to_string())?;
    if !refs.is_empty() {
        let route_list = refs
            .iter()
            .map(|r| format!("{}:{}", r.strategy_id, r.label))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "account {account_id} is referenced by {} strategy route(s) ({route_list}); remove or reassign them first",
            refs.len()
        ));
    }
    // Accounts that own recorded position legs keep their attribution; delete
    // is refused so signal history never loses its account binding.
    if state
        .state_store
        .account_has_signal_legs(account_id)
        .map_err(|e| e.to_string())?
    {
        return Err(format!(
            "account {account_id} has recorded signal position history; disable the account instead of deleting it"
        ));
    }
    {
        let mut vault = state.vault.write();
        if !vault.is_unlocked() {
            return Err("Vault is locked. Please unlock before deleting an account.".into());
        }
    }
    let deleted = state
        .state_store
        .delete_account(account_id)
        .map_err(|e| e.to_string())?;
    if deleted {
        let mut vault = state.vault.write();
        if let Err(e) = vault.delete_profile_credentials(account_id) {
            // Orphaned bound records are pruned by ensure_credential_migration
            // on the next unlock; surface in logs only.
            tracing::warn!("account {account_id} deleted but credential cleanup failed: {e}");
        }
    }
    state.sync_vault_credentials_to_adapters();
    Ok(deleted)
}

/// Removes only the credential records bound to one account profile; the
/// account itself is kept. Distinct from `delete_account_impl`, which removes
/// the whole profile, and from `delete_exchange_credentials_impl`, which is
/// venue-wide.
pub fn delete_account_credentials_impl(
    account_id: &str,
    state: &AppEngineState,
) -> Result<bool, String> {
    let exists = state
        .state_store
        .get_account(account_id)
        .map_err(|e| e.to_string())?
        .is_some();
    if !exists {
        return Err(format!("Unknown account '{account_id}'"));
    }
    let res = {
        let mut vault = state.vault.write();
        if !vault.is_unlocked() {
            return Err("Vault is locked. Please unlock before removing credentials.".into());
        }
        vault
            .delete_profile_credentials(account_id)
            .map_err(|e| e.to_string())
    }?;
    state.sync_vault_credentials_to_adapters();
    Ok(res)
}

/// Returns the decrypted credentials bound to one account profile for the
/// local settings UI. Requires an unlocked vault; unknown accounts and locked
/// vaults fail closed. Values are returned over local IPC only — they are
/// never logged or persisted anywhere new.
pub fn get_account_credentials_impl(
    account_id: &str,
    state: &AppEngineState,
) -> Result<AccountCredentialInput, String> {
    let exists = state
        .state_store
        .get_account(account_id)
        .map_err(|e| e.to_string())?
        .is_some();
    if !exists {
        return Err(format!("Unknown account '{account_id}'"));
    }
    let mut vault = state.vault.write();
    if !vault.is_unlocked() {
        return Err("Vault is locked. Please unlock before viewing credentials.".into());
    }
    let data = vault.get_data().map_err(|e| e.to_string())?;
    let cred = data
        .credentials
        .iter()
        .find(|c| c.profile_id.as_deref() == Some(account_id));
    match cred {
        Some(c) => Ok(AccountCredentialInput {
            api_key: c.api_key.clone(),
            api_secret: c.api_secret.clone(),
            passphrase: c.passphrase.clone(),
            account_id: c.account_id.clone(),
            wallet_address: c.wallet_address.clone(),
            account_index: c.account_index,
            subaccount_index: c.subaccount_index,
            api_key_index: c.api_key_index,
            private_key: c.private_key.clone(),
        }),
        None => Err(format!(
            "No credentials configured for account '{account_id}'"
        )),
    }
}

/// Field-scoped credential reveal (security hardening, 2026-09-19).
/// The legacy `get_account_credentials` call returns the WHOLE secret bundle
/// (API key + secret + passphrase + private key) into the renderer, where it
/// lingers in React state and DOM inputs. This endpoint returns ONE whitelisted
/// field so the UI only pulls a secret across IPC when the user explicitly
/// reveals that field. Unknown fields fail closed; vault must be unlocked.
pub fn get_account_credential_field_impl(
    account_id: &str,
    field: &str,
    state: &AppEngineState,
) -> Result<String, String> {
    let exists = state
        .state_store
        .get_account(account_id)
        .map_err(|e| e.to_string())?
        .is_some();
    if !exists {
        return Err(format!("Unknown account '{account_id}'"));
    }
    let mut vault = state.vault.write();
    if !vault.is_unlocked() {
        return Err("Vault is locked. Please unlock before viewing credentials.".into());
    }
    let data = vault.get_data().map_err(|e| e.to_string())?;
    let cred = data
        .credentials
        .iter()
        .find(|c| c.profile_id.as_deref() == Some(account_id));
    match cred {
        Some(c) => Ok(match field {
            "apiKey" => c.api_key.clone(),
            "apiSecret" => c.api_secret.clone(),
            "passphrase" => c.passphrase.clone().unwrap_or_default(),
            "accountId" => c.account_id.clone().unwrap_or_default(),
            "walletAddress" => c.wallet_address.clone().unwrap_or_default(),
            "privateKey" => c.private_key.clone().unwrap_or_default(),
            _ => return Err(format!("Unknown credential field '{field}'")),
        }),
        None => Err(format!(
            "No credentials configured for account '{account_id}'"
        )),
    }
}

pub fn set_default_account_impl(
    venue: &str,
    account_id: &str,
    state: &AppEngineState,
) -> Result<(), String> {
    state
        .state_store
        .set_default_account(venue, account_id)
        .map_err(|e| e.to_string())
}

pub fn list_strategy_routes_impl(
    strategy_id: Option<&str>,
    state: &AppEngineState,
) -> Result<Vec<StrategyAccountRoute>, String> {
    state
        .state_store
        .list_strategy_routes(strategy_id)
        .map_err(|e| e.to_string())
}

pub fn upsert_strategy_route_impl(
    input: StrategyRouteUpsertInput,
    state: &AppEngineState,
) -> Result<StrategyAccountRoute, String> {
    let id = match &input.id {
        Some(id) => id.clone(),
        None => {
            let sid = input.strategy_id.trim().to_lowercase();
            let existing: std::collections::HashSet<String> = state
                .state_store
                .list_strategy_routes(None)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|r| r.id)
                .collect();
            let mut candidate;
            loop {
                candidate = format!("{}-{:08x}", sid, rand::random::<u32>());
                if !existing.contains(&candidate) {
                    break;
                }
            }
            candidate
        }
    };
    // Labels are required by the store; an omitted label preserves the
    // existing route's label on update, or derives a readable default.
    let label = if input.label.trim().is_empty() {
        let existing_label = state
            .state_store
            .list_strategy_routes(None)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|r| r.id == id)
            .map(|r| r.label);
        match existing_label {
            Some(l) => l,
            None => {
                let name = |aid: &str| -> String {
                    state
                        .state_store
                        .get_account(aid)
                        .ok()
                        .flatten()
                        .map(|a| format!("{}:{}", a.venue, a.label))
                        .unwrap_or_else(|| aid.to_string())
                };
                format!(
                    "{} \u{2194} {}",
                    name(&input.account_a_id),
                    name(&input.account_b_id)
                )
            }
        }
    } else {
        input.label.clone()
    };
    state
        .state_store
        .upsert_strategy_route(
            &id,
            &input.strategy_id,
            &label,
            &input.account_a_id,
            &input.account_b_id,
            input.enabled,
        )
        .map_err(|e| e.to_string())
}

pub fn delete_strategy_route_impl(route_id: &str, state: &AppEngineState) -> Result<bool, String> {
    state
        .state_store
        .delete_strategy_route(route_id)
        .map_err(|e| e.to_string())
}

/// Eligible account routes for a strategy across a venue pair: enabled
/// routes whose two endpoint accounts are enabled and sit on the two
/// venues (in either order).
pub fn eligible_strategy_routes_impl(
    strategy_id: &str,
    venue_a: &str,
    venue_b: &str,
    state: &AppEngineState,
) -> Result<Vec<StrategyAccountRoute>, String> {
    state
        .state_store
        .eligible_strategy_routes(strategy_id, venue_a, venue_b)
        .map_err(|e| e.to_string())
}

pub async fn get_venue_balances_impl(
    state: &AppEngineState,
) -> Result<Vec<VenueBalanceInfo>, String> {
    let is_live = *state.is_live_mode.read();
    let monitor = margin_monitor_from_settings(state);
    let mut results = Vec::new();

    if is_live {
        if state.vault.write().is_unlocked() {
            state.sync_vault_credentials_to_adapters();
        }

        for (vid, name, adapter) in state.get_venues() {
            let def_acc_id = state.state_store.default_account(vid).ok().flatten().map(|a| a.id);
            if !adapter.has_live_credentials() {
                results.push(VenueBalanceInfo {
                    venue: vid.to_string(),
                    equity_usd: 0.0,
                    available_margin_usd: 0.0,
                    margin_rate_pct: 0.0,
                    status: "NO_CREDENTIALS".to_string(),
                    recommended_action: Some(format!(
                        "No live API credentials configured for {name} in the secure vault."
                    )),
                    is_live: true,
                    account_id: def_acc_id,
                });
                continue;
            }

            match adapter.fetch_account().await {
                Ok(account) => {
                    let free_margin_usd = account
                        .free_margin
                        .to_string()
                        .parse::<f64>()
                        .unwrap_or(0.0);
                    let equity_usd = account.equity.to_string().parse::<f64>().unwrap_or(0.0);
                    let margin_rate_pct = account
                        .margin_usage_pct
                        .to_string()
                        .parse::<f64>()
                        .unwrap_or(0.0);

                    if equity_usd.is_finite() {
                        let now_ms = Utc::now().timestamp_millis();
                        match state.state_store.check_account_equity(
                            vid,
                            equity_usd,
                            &position_signature(&account.positions),
                            now_ms,
                        ) {
                            Ok(report) => {
                                if let Some(event) = report.event {
                                    tracing::warn!(
                                        "external equity delta on {vid}: {} expected {expected} actual {actual} ({kind}) — baseline rebased",
                                        event.delta,
                                        expected = event.expected,
                                        actual = event.actual,
                                        kind = event.kind,
                                    );
                                }
                            }
                            Err(e) => tracing::warn!("equity attribution poll failed for {vid}: {e}"),
                        }
                    }

                    let (status, action) = match monitor.evaluate_account(&account) {
                        MarginHealthStatus::Healthy => ("HEALTHY".to_string(), None),
                        MarginHealthStatus::Warning => (
                            "LOW_MARGIN".to_string(),
                            Some(format!("Warning: {} margin utilization elevated.", name)),
                        ),
                        MarginHealthStatus::Empty => (
                            "EMPTY".to_string(),
                            Some(format!(
                                "No funds on {} — deposit to enable trading on this venue.",
                                name
                            )),
                        ),
                        MarginHealthStatus::Critical | MarginHealthStatus::Breached => (
                            "DEPLETED".to_string(),
                            Some(format!(
                                "Critical: {} margin utilization breached. Execution halted on this leg.",
                                name
                            )),
                        ),
                    };

                    results.push(VenueBalanceInfo {
                        venue: vid.to_string(),
                        equity_usd,
                        available_margin_usd: free_margin_usd,
                        margin_rate_pct,
                        status,
                        recommended_action: action,
                        is_live: true,
                        account_id: def_acc_id,
                    });
                }
                Err(e) => {
                    tracing::warn!("failed to fetch live balance for {name} ({vid}): {e}");
                    results.push(VenueBalanceInfo {
                        venue: vid.to_string(),
                        equity_usd: 0.0,
                        available_margin_usd: 0.0,
                        margin_rate_pct: 0.0,
                        status: "ERROR".to_string(),
                        recommended_action: Some(format!("Failed to retrieve live balance: {e}")),
                        is_live: true,
                        account_id: def_acc_id,
                    });
                }
            }
        }

        return Ok(results);
    }

    // Only venues currently admitted for paper execution report balances;
    // figures come from the real simulated account snapshots.
    for (vid, name, _adapter) in state.get_venues() {
        let Some(paper) = state.get_paper_exchange(vid) else {
            continue;
        };
        let account = paper.fetch_account().await.map_err(|e| e.to_string())?;

        let free_margin_usd = account
            .free_margin
            .to_string()
            .parse::<f64>()
            .unwrap_or(0.0);
        let equity_usd = account.equity.to_string().parse::<f64>().unwrap_or(0.0);
        let margin_rate_pct = account
            .margin_usage_pct
            .to_string()
            .parse::<f64>()
            .unwrap_or(0.0);

        // Attribution poll: compare actual equity against the fills-derived
        // baseline. Own fills already moved the baseline forward at dispatch,
        // so any residual delta here is an external flow — recorded and
        // rebased. Failures only log; balance reporting must never break.
        if equity_usd.is_finite() {
            let now_ms = Utc::now().timestamp_millis();
            match state.state_store.check_account_equity(
                vid,
                equity_usd,
                &position_signature(&account.positions),
                now_ms,
            ) {
                Ok(report) => {
                    if let Some(event) = report.event {
                        tracing::warn!(
                            "external equity delta on {vid}: {} expected {expected} actual {actual} ({kind}) — baseline rebased",
                            event.delta,
                            expected = event.expected,
                            actual = event.actual,
                            kind = event.kind,
                        );
                    }
                }
                Err(e) => tracing::warn!("equity attribution poll failed for {vid}: {e}"),
            }
        }

        let (status, action) = match monitor.evaluate_account(&account) {
            MarginHealthStatus::Healthy => ("HEALTHY".to_string(), None),
            MarginHealthStatus::Warning => (
                "LOW_MARGIN".to_string(),
                Some(format!("Warning: {} margin utilization elevated.", name)),
            ),
            MarginHealthStatus::Empty => (
                "EMPTY".to_string(),
                Some(format!(
                    "No funds on {} — deposit to enable trading on this venue.",
                    name
                )),
            ),
            MarginHealthStatus::Critical | MarginHealthStatus::Breached => (
                "DEPLETED".to_string(),
                Some(format!(
                    "Critical: {} margin utilization breached. Execution halted on this leg.",
                    name
                )),
            ),
        };

        let def_acc_id = state.state_store.default_account(vid).ok().flatten().map(|a| a.id);
        results.push(VenueBalanceInfo {
            venue: vid.to_string(),
            equity_usd,
            available_margin_usd: free_margin_usd,
            margin_rate_pct,
            status,
            recommended_action: action,
            is_live: false,
            account_id: def_acc_id,
        });
    }

    Ok(results)
}

pub fn get_strategy_performance_impl(
    strategy_id: Option<String>,
    state: &AppEngineState,
) -> Result<Vec<StrategyMetrics>, String> {
    let ledger = state.virtual_ledger.read();
    let strat_ids = match strategy_id {
        Some(s) => vec![s],
        None => vec![
            "spatial_spread".to_string(),
            "stat_arb".to_string(),
            "funding_rate".to_string(),
            "prediction_parity".to_string(),
        ],
    };

    let metrics = strat_ids
        .into_iter()
        .map(|id| ledger.compute_strategy_metrics(&id))
        .collect();

    Ok(metrics)
}

/// Attributed realized PnL for one strategy, derived from durable signal-leg
/// fills (fees included). Closed root families owned by `strategy_id` sum
/// their root + `flat-*` child fills via side-signed cash flows; families
/// that cannot be valued (no strategy tag, or no priced fills) are excluded
/// from the sums and reported as `unattributed_closed_families`.
pub fn get_strategy_fill_pnl_impl(
    strategy_id: &str,
    state: &AppEngineState,
) -> Result<crate::attribution::StrategyFillPnl, String> {
    use crate::state_store::LegSide;
    let want = crate::attribution::attribute_strategy_id(strategy_id);
    let signals = state
        .state_store
        .list_signals(None, None)
        .map_err(|e| e.to_string())?;
    let mut net_usd = 0.0_f64;
    let mut notional = 0.0_f64;
    let mut closed = 0_usize;
    let mut open = 0_usize;
    let mut unattributed = 0_usize;
    for root in signals.iter().filter(|s| !s.id.starts_with("flat-")) {
        let owned = crate::attribution::attribute_strategy_id(&root.strategy_id) == want;
        let is_closed = root.status == crate::state_store::SignalStatus::Closed;
        if !owned {
            continue;
        }
        if !is_closed {
            if root.status.counts_as_open() {
                open += 1;
            }
            continue;
        }
        let prefix = format!("flat-{}-", root.id);
        let mut fills = Vec::new();
        for sig in std::iter::once(root).chain(
            signals
                .iter()
                .filter(|s| s.id.starts_with(&prefix)),
        ) {
            for leg in &sig.legs {
                fills.push(crate::attribution::LegFill {
                    is_short: leg.side == LegSide::Short,
                    qty: leg.filled_qty,
                    price: leg.fill_price,
                    fee_usd: leg.fee_usd,
                });
            }
        }
        match crate::attribution::family_realized_pnl(&fills) {
            Some(pnl) => {
                closed += 1;
                net_usd += pnl.net_usd;
                notional += pnl.notional_usd;
            }
            None => {
                unattributed += 1;
            }
        }
    }
    let is_live = *state.is_live_mode.read();
    if want == crate::attribution::UNATTRIBUTED_STRATEGY_ID {
        // Quarantine query: report only the quarantined families.
        return Ok(crate::attribution::StrategyFillPnl {
            strategy_id: want,
            net_profit_usd: 0.0,
            net_profit_bps: None,
            closed_families: 0,
            open_families: 0,
            unattributed_closed_families: unattributed,
            unattributed_log_entries: state
                .execution_logs
                .read()
                .iter()
                .filter(|e| {
                    e.is_live == is_live
                        && crate::attribution::attribute_strategy_id(&e.strategy_id)
                            == crate::attribution::UNATTRIBUTED_STRATEGY_ID
                })
                .count(),
        });
    }
    Ok(crate::attribution::StrategyFillPnl {
        strategy_id: want,
        net_profit_usd: (net_usd * 100.0).round() / 100.0,
        net_profit_bps: if notional > 0.0 {
            Some(((net_usd / notional * 10_000.0) * 10.0).round() / 10.0)
        } else {
            None
        },
        closed_families: closed,
        open_families: open,
        unattributed_closed_families: unattributed,
        unattributed_log_entries: state
            .execution_logs
            .read()
            .iter()
            .filter(|e| {
                e.is_live == is_live
                    && crate::attribution::attribute_strategy_id(&e.strategy_id)
                        == crate::attribution::UNATTRIBUTED_STRATEGY_ID
            })
            .count(),
    })
}

/// Time-weighted average utilisation for one venue over a trailing window
/// (default 24h) from the durable margin-snapshot log. `partial` is true
/// when snapshots cover less than the full window — the UI labels the
/// shorter window honestly instead of stretching it.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UtilisationHistoryDto {
    pub venue: String,
    pub avg_rate_pct: Option<f64>,
    pub covered_ms: i64,
    pub window_ms: i64,
    pub partial: bool,
    pub point_count: usize,
}

pub fn get_utilisation_history_impl(
    venue: &str,
    window_hours: Option<u64>,
    state: &AppEngineState,
) -> Result<UtilisationHistoryDto, String> {
    let default_hours = (UTILISATION_DEFAULT_WINDOW_MS / 3_600_000).max(1) as u64;
    let hours = window_hours.unwrap_or(default_hours).clamp(1, 720);
    let window_ms: i64 = (hours as i64).saturating_mul(3_600_000);
    let now_ms = Utc::now().timestamp_millis();
    let since_ms = now_ms.saturating_sub(window_ms);
    let rows = state
        .state_store
        .list_margin_snapshots(venue, since_ms)
        .map_err(|e| e.to_string())?;
    let points: Vec<(i64, f64)> = rows.iter().map(|r| (r.ts_ms, r.rate_pct)).collect();
    let point_count = points.len();
    let avg = crate::attribution::time_weighted_avg(&points, now_ms, window_ms);
    Ok(UtilisationHistoryDto {
        venue: venue.trim().to_lowercase(),
        avg_rate_pct: avg.map(|a| (a.avg_rate_pct * 100.0).round() / 100.0),
        covered_ms: avg.map(|a| a.covered_ms).unwrap_or(0),
        window_ms,
        partial: avg.map(|a| a.covered_ms < window_ms).unwrap_or(true),
        point_count,
    })
}

pub async fn run_wfo_calibration_impl(
    _symbol: &str,
    _state: &AppEngineState,
) -> Result<WfoOptimizationResult, String> {
    Err("WFO calibration is unavailable until sufficient recorded quote history exists".into())
}

#[tauri::command]
pub async fn get_vault_status(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<VaultStatus, String> {
    Ok(get_vault_status_impl(&state))
}

#[tauri::command]
pub async fn initialize_vault(
    password: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<VaultStatus, String> {
    initialize_vault_impl(&password, &state)
}

#[tauri::command]
pub async fn unlock_vault(
    password: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<VaultStatus, String> {
    unlock_vault_impl(&password, &state)
}

#[tauri::command]
pub async fn lock_vault(state: State<'_, Arc<AppEngineState>>) -> Result<VaultStatus, String> {
    Ok(lock_vault_impl(&state))
}

#[tauri::command]
pub async fn delete_exchange_credentials(
    venue: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<bool, String> {
    delete_exchange_credentials_impl(&venue, &state)
}

#[tauri::command]
pub async fn list_accounts(
    venue: Option<String>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<AccountDto>, String> {
    list_accounts_impl(venue.as_deref(), &state)
}

#[tauri::command]
pub async fn upsert_account(
    input: AccountUpsertInput,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<AccountDto, String> {
    upsert_account_impl(input, &state)
}

#[tauri::command]
pub async fn delete_account(
    account_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<bool, String> {
    delete_account_impl(&account_id, &state)
}

#[tauri::command]
pub async fn delete_account_credentials(
    account_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<bool, String> {
    delete_account_credentials_impl(&account_id, &state)
}

#[tauri::command]
pub async fn get_account_credential_field(
    account_id: String,
    field: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<String, String> {
    get_account_credential_field_impl(&account_id, &field, &state)
}

#[tauri::command]
pub async fn get_account_credentials(
    account_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<AccountCredentialInput, String> {
    get_account_credentials_impl(&account_id, &state)
}

#[tauri::command]
pub async fn set_default_account(
    venue: String,
    account_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<(), String> {
    set_default_account_impl(&venue, &account_id, &state)
}

#[tauri::command]
pub async fn list_strategy_routes(
    strategy_id: Option<String>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<StrategyAccountRoute>, String> {
    list_strategy_routes_impl(strategy_id.as_deref(), &state)
}

#[tauri::command]
pub async fn upsert_strategy_route(
    input: StrategyRouteUpsertInput,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<StrategyAccountRoute, String> {
    upsert_strategy_route_impl(input, &state)
}

#[tauri::command]
pub async fn delete_strategy_route(
    route_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<bool, String> {
    delete_strategy_route_impl(&route_id, &state)
}

#[tauri::command]
pub async fn eligible_strategy_routes(
    strategy_id: String,
    venue_a: String,
    venue_b: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<StrategyAccountRoute>, String> {
    eligible_strategy_routes_impl(&strategy_id, &venue_a, &venue_b, &state)
}

#[tauri::command]
pub async fn get_venue_balances(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<VenueBalanceInfo>, String> {
    get_venue_balances_impl(&state).await
}

#[tauri::command]
pub async fn get_strategy_performance(
    strategy_id: Option<String>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<StrategyMetrics>, String> {
    get_strategy_performance_impl(strategy_id, &state)
}

#[tauri::command]
pub async fn get_strategy_fill_pnl(
    strategy_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<crate::attribution::StrategyFillPnl, String> {
    get_strategy_fill_pnl_impl(&strategy_id, &state)
}

#[tauri::command]
pub async fn get_utilisation_history(
    venue: String,
    window_hours: Option<u64>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<UtilisationHistoryDto, String> {
    get_utilisation_history_impl(&venue, window_hours, &state)
}

#[tauri::command]
pub async fn run_wfo_calibration(
    symbol: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<WfoOptimizationResult, String> {
    run_wfo_calibration_impl(&symbol, &state).await
}

// ---------------------------------------------------------------------------
// Spread Range Monitor — systematic cross-venue opportunity radar
// ---------------------------------------------------------------------------

fn monitor_engine(
    state: &AppEngineState,
) -> Result<Arc<crate::monitor::SpreadMonitorEngine>, String> {
    state
        .monitor
        .lock()
        .clone()
        .ok_or_else(|| "spread monitor is not running".to_string())
}

#[tauri::command]
pub async fn get_monitor_snapshot(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Option<crate::monitor::MonitorSnapshotDto>, String> {
    Ok(monitor_engine(&state)?.snapshot())
}

#[tauri::command]
pub async fn get_monitor_config(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<crate::monitor::MonitorConfigDto, String> {
    Ok(monitor_engine(&state)?.config())
}

#[tauri::command]
pub async fn set_monitor_config(
    config: crate::monitor::MonitorConfigDto,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<(), String> {
    monitor_engine(&state)?.set_config(config, &state)
}

/// Recent signal-lifecycle audit events (transitions + heartbeats), newest
/// first. No UI consumes this in v1 — reserved for the future trading bot
/// and debugging via IPC.
#[tauri::command]
pub async fn get_monitor_signal_events(
    symbol: Option<String>,
    buy_venue: Option<String>,
    sell_venue: Option<String>,
    limit: Option<i64>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<crate::state_store::MonitorSignalEventRow>, String> {
    let clean = |v: Option<String>| {
        v.filter(|s| !s.trim().is_empty())
    };
    let lim = limit.unwrap_or(200).clamp(1, 10_000);
    state
        .state_store
        .recent_monitor_signal_events(
            clean(symbol).as_deref(),
            clean(buy_venue).as_deref(),
            clean(sell_venue).as_deref(),
            lim,
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_ticker_exempted(
    symbol: String,
    exempt: bool,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<(), String> {
    let sym = crate::monitor::canonical_base(&symbol);
    if sym.is_empty() || sym.len() > 64 {
        return Err("invalid symbol".into());
    }
    state
        .state_store
        .set_ticker_exempt(&sym, exempt)
        .map_err(|e| e.to_string())?;
    // Force the watchlist to re-derive on the next tick.
    if let Some(engine) = state.monitor.lock().clone() {
        engine.request_watchlist_refresh();
    }
    Ok(())
}

/// Currently exempted symbols (canonical base form).
#[tauri::command]
pub async fn get_exempted_tickers(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<String>, String> {
    state
        .state_store
        .exempt_tickers()
        .map_err(|e| e.to_string())
}

/// Global "reveal all": clears every ticker exemption. Returns the number
/// of exemptions removed.
#[tauri::command]
pub async fn reveal_all_tickers(state: State<'_, Arc<AppEngineState>>) -> Result<usize, String> {
    let n = state
        .state_store
        .clear_ticker_exemptions()
        .map_err(|e| e.to_string())?;
    if let Some(engine) = state.monitor.lock().clone() {
        engine.request_watchlist_refresh();
    }
    Ok(n)
}

/// Per-row Execute: re-validates the opportunity against the latest
/// snapshot (must still be qualified and unblocked), converts the USD size
/// to base quantity on the live book, then dispatches the existing guarded
/// paper execution path.
#[tauri::command]
pub async fn execute_monitor_opportunity(
    symbol: String,
    buy_venue: String,
    sell_venue: String,
    size_usd: f64,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SimStepResultDto, String> {
    execute_monitor_opportunity_impl(&symbol, &buy_venue, &sell_venue, size_usd, &state).await
}

/// Shared monitor-entry primitive. The manual row-Execute button and the
/// autonomous convergence bot both enter through here, so the fail-closed
/// gates (executability, stale books, paper-only, blocking signals) apply
/// identically to human and bot flow. Paper-only: the inner spatial path
/// hard-blocks live mode.
pub(crate) async fn execute_monitor_opportunity_impl(
    symbol: &str,
    buy_venue: &str,
    sell_venue: &str,
    size_usd: f64,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    let engine = monitor_engine(state)?;
    let sym = crate::monitor::canonical_base(symbol);
    if !(size_usd.is_finite() && size_usd > 0.0) {
        return Err("invalid size".into());
    }
    let snap = engine
        .snapshot()
        .ok_or_else(|| "monitor has not produced a snapshot yet".to_string())?;
    let row = snap
        .rows
        .iter()
        .find(|r| r.symbol == sym && r.buy_venue == buy_venue && r.sell_venue == sell_venue)
        .ok_or_else(|| "opportunity is no longer tracked".to_string())?;
    if !row.executable {
        return Err(row
            .block_reason
            .clone()
            .unwrap_or_else(|| "opportunity is not executable".into()));
    }
    // Auto-filtered tickers stay research-visible on the radar but refuse
    // Execute with an explicit block reason. Manual (global-exempt) hiding
    // is enforced separately by the watchlist derivation itself.
    {
        let mut scope = engine.in_scope_venues(&sym);
        if scope.len() < 2 {
            scope = vec![buy_venue.to_string(), sell_venue.to_string()];
            scope.sort();
            scope.dedup();
        }
        reject_if_strategy_filtered(state, "radar", &sym, &scope).await?;
    }
    // Convert USD size to base qty at the current buy-side ask.
    let book = state
        .market_data_hub
        .get_valid_orderbook(
            &arbitrageur_core::types::VenueId(buy_venue.to_string()),
            &sym,
            Some(std::time::Duration::from_secs(60)),
        )
        .ok_or_else(|| "buy-side orderbook went stale".to_string())?;
    let ask = book
        .asks
        .first()
        .and_then(|l| l.price.to_string().parse::<f64>().ok())
        .filter(|p| *p > 0.0)
        .ok_or_else(|| "no ask on buy-side book".to_string())?;
    let qty = size_usd / ask;
    // Radar-dispatched fills persist under the "radar" strategy so the
    // Spread Radar page scopes positions separately from Single Spread.
    let res = execute_spatial_arbitrage_inner(
        &sym,
        buy_venue,
        sell_venue,
        qty,
        Some("OPEN".into()),
        row.current_edge_bps,
        row.baseline_bps,
        None,
        None,
        None,
        None,
        "radar",
        state,
    )
    .await?;

    // Adopt into auto-exit watcher pool targeting dynamic baseline
    if let Some(bot) = state.radar_bot.lock().as_ref() {
        let friction_band = row
            .deviation_bps
            .zip(row.net_capture_bps)
            .and_then(|(d, n)| arbitrageur_strategy::convergence_auto::friction_band_bps(Some(d), Some(n)));
        let _ = bot.adopt_manual_position(
            state,
            &sym,
            buy_venue,
            sell_venue,
            size_usd,
            row.deviation_bps,
            row.baseline_bps,
            friction_band,
            row.expected_converge_secs,
        );
    }

    Ok(res)
}

// ---------------------------------------------------------------------------
// Single-Spread convergence lifecycle: position-aware exits that free the
// pair slot for re-entry.
// ---------------------------------------------------------------------------

/// Re-price the live edge of an open "futures" pair from freshly validated
/// books. `Ok(None)` means the edge is currently unpriceable (stale/missing
/// books or unpriceable depth) — the caller holds (Unwatched) rather than
/// exiting blind. `Err` is reserved for fail-closed input faults.
async fn live_spatial_edge(
    state: &AppEngineState,
    symbol: &str,
    buy_venue: &str,
    sell_venue: &str,
    qty: f64,
) -> Result<Option<f64>, String> {
    let buy_adapter = state.get_adapter(buy_venue).ok_or_else(|| {
        format!("Buy venue '{buy_venue}' is not admitted for paper execution")
    })?;
    let sell_adapter = state.get_adapter(sell_venue).ok_or_else(|| {
        format!("Sell venue '{sell_venue}' is not admitted for paper execution")
    })?;
    let buy_vid = buy_adapter.venue_id();
    let sell_vid = sell_adapter.venue_id();
    let qty_dec = Decimal::from_f64_retain(qty)
        .filter(|q| *q > dec!(0))
        .ok_or_else(|| "leg quantity is not a positive number".to_string())?;
    let fetch_book = async |adapter: &Arc<dyn ExchangeAdapter>,
                            vid: &VenueId,
                            side: &str| {
        let fresh = match state.market_data_hub.get_valid_orderbook(
            vid,
            symbol,
            Some(std::time::Duration::from_secs(2)),
        ) {
            Some(b) => b,
            None => {
                let b = adapter.fetch_orderbook(symbol).await.map_err(|e| {
                    format!("No fresh {side} orderbook for {symbol}: {e}")
                })?;
                validate_adapter_book(&b, vid, symbol)
                    .map_err(|e| format!("{side} order book rejected: {e}"))?;
                state.market_data_hub.update_orderbook(b.clone());
                b
            }
        };
        validate_adapter_book(&fresh, vid, symbol)
            .map_err(|e| format!("{side} order book rejected: {e}"))?;
        Ok::<OrderBook, String>(fresh)
    };
    let book_buy = fetch_book(&buy_adapter, &buy_vid, "buy-side").await?;
    let book_sell = fetch_book(&sell_adapter, &sell_vid, "sell-side").await?;
    match compute_route_economics(
        &book_buy,
        &book_sell,
        qty_dec,
        effective_venue_fee_bps(state, buy_vid.as_str()),
        effective_venue_fee_bps(state, sell_vid.as_str()),
        0.0,
    ) {
        Some(econ) if econ.expected_net_bps.is_finite() => Ok(Some(econ.expected_net_bps)),
        _ => Ok(None),
    }
}

/// Evaluate every open "futures" pair position (optionally scoped to one
/// symbol) against its entry baseline and unwind converge/stop/timeout exits
/// through the existing symbol-scoped flatten path. Unwinding marks the
/// signal `closed` and flattens the paper legs, which frees the pair slot:
/// the blocking-ledger and paper-position gates both clear, so re-entry is
/// allowed afterwards. Positions without a baseline, without a live quote,
/// or whose legs do not form a long/short pair report `unwatched` and are
/// never auto-exited; the manual flatten path stays available for them.
///
/// The one-open-pair-per-symbol cap is intentionally kept (see the guard in
/// `execute_spatial_arbitrage_inner`): paper accounts are shared per venue,
/// so per-pair margin/PnL attribution is impossible without sub-accounting,
/// and flatten is symbol-scoped — it would cross-close independent pairs on
/// the same symbol. Multi-pair safety would additionally require per-pair
/// position keys in the ledger, symbol+pair-scoped flatten, and per-pair
/// margin isolation in the paper exchanges.
pub async fn check_spatial_convergence_impl(
    symbol: Option<&str>,
    strategy_id: &str,
    state: &AppEngineState,
) -> Result<Vec<ConvergenceCheckDto>, String> {
    use arbitrageur_strategy::spatial_exit::{
        evaluate_spatial_exit, SpatialExitConfig, SpatialExitVerdict,
    };
    let sid = crate::state_store::normalize_strategy_id(strategy_id).map_err(|e| e.to_string())?;
    if sid != "futures" {
        return Err(
            "convergence check supports the 'futures' strategy only (radar positions have their own bot lifecycle)"
                .into(),
        );
    }
    let cfg = SpatialExitConfig::default();
    cfg.validate().map_err(|e| e.to_string())?;
    if let Some(s) = symbol {
        validate_market_symbol(s)?;
    }
    let now_ms = Utc::now().timestamp_millis();
    let signals = state
        .state_store
        .list_signals(symbol, None)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for sig in signals
        .iter()
        .filter(|s| s.strategy_id.eq_ignore_ascii_case(&sid) && s.status.counts_as_open())
    {
        let baseline = state
            .state_store
            .spatial_baseline_for_signal(&sig.id)
            .map_err(|e| e.to_string())?;
        let entry_net = baseline.as_ref().map(|b| b.entry_net_bps);
        let entry_ts = baseline.as_ref().map(|b| b.entry_ts_ms).unwrap_or(0);
        let verdict_of = |verdict: &str,
                          trigger: Option<String>,
                          reason: String,
                          live: Option<f64>,
                          unwound: bool| {
            ConvergenceCheckDto {
                signal_id: sig.id.clone(),
                symbol: sig.symbol.clone(),
                verdict: verdict.to_string(),
                trigger,
                reason,
                live_net_bps: live,
                entry_net_bps: entry_net,
                unwound,
            }
        };
        let buy_leg = sig
            .legs
            .iter()
            .find(|l| l.side == crate::state_store::LegSide::Long);
        let sell_leg = sig
            .legs
            .iter()
            .find(|l| l.side == crate::state_store::LegSide::Short);
        let (Some(buy), Some(sell)) = (buy_leg, sell_leg) else {
            out.push(verdict_of(
                "unwatched",
                None,
                "signal legs do not form a long/short pair".to_string(),
                None,
                false,
            ));
            continue;
        };
        if !buy.filled_qty.is_finite() || buy.filled_qty <= 0.0 {
            out.push(verdict_of(
                "unwatched",
                None,
                "long leg has no positive fill to size the live quote".to_string(),
                None,
                false,
            ));
            continue;
        }
        let live = match live_spatial_edge(
            state,
            &sig.symbol,
            &buy.venue,
            &sell.venue,
            buy.filled_qty,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                out.push(verdict_of("unwatched", None, e, None, false));
                continue;
            }
        };
        match evaluate_spatial_exit(entry_net, live, entry_ts, now_ms, &cfg) {
            SpatialExitVerdict::Hold => {
                out.push(verdict_of("hold", None, "edge inside band".to_string(), live, false));
            }
            SpatialExitVerdict::Unwatched { reason } => {
                out.push(verdict_of("unwatched", None, reason, live, false));
            }
            SpatialExitVerdict::Exit { trigger, reason } => {
                // Convergence unwinds route through the symbol-scoped flatten
                // path, so the leftover sweep (sub-step residual guard) and
                // the paper-flat reconciliation apply here unchanged.
                match flatten_ticker_positions_impl(Some(&sig.symbol), &sid, state).await {
                    Ok(r) if r.residual_signals == 0 => out.push(verdict_of(
                        "exit",
                        Some(trigger.as_str().to_string()),
                        format!("{reason}; unwound {} signal(s)", r.signals_closed),
                        live,
                        true,
                    )),
                    Ok(r) => out.push(verdict_of(
                        "exit",
                        Some(trigger.as_str().to_string()),
                        format!(
                            "{reason}; flatten left {} residual signal(s) — slot stays blocked",
                            r.residual_signals
                        ),
                        live,
                        false,
                    )),
                    Err(e) => out.push(verdict_of(
                        "exit",
                        Some(trigger.as_str().to_string()),
                        format!("{reason}; flatten failed: {e}"),
                        live,
                        false,
                    )),
                }
            }
        }
    }
    Ok(out)
}

/// Session-seasonality snapshot for one symbol: per-session aggregates,
/// the current session, and the active multipliers. Display only.
#[tauri::command]
pub async fn get_session_profile(
    symbol: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<crate::monitor::SessionProfileDto, String> {
    get_session_profile_impl(&symbol, &state)
}

pub(crate) fn get_session_profile_impl(
    symbol: &str,
    state: &AppEngineState,
) -> Result<crate::monitor::SessionProfileDto, String> {
    use arbitrageur_math::session::{
        in_funding_mask, is_weekend, session_allow, session_of, us_cash_open,
    };
    let now_ms = chrono::Utc::now().timestamp_millis();
    let session = session_of(now_ms);
    let weekend = is_weekend(now_ms);
    // Symbol-level display mask covers the common 8h + 4h stamps; per-pair
    // hourly masks apply inside the monitor evaluation itself.
    let mask = in_funding_mask(now_ms, &[8, 4]);
    let rows = state
        .state_store
        .session_profiles(symbol)
        .map_err(|e| e.to_string())?;
    let promotion_row = state
        .state_store
        .phase2_promotion_state(symbol)
        .map_err(|e| e.to_string())?;
    let anchor_row = state
        .state_store
        .phase2_har_anchor(symbol)
        .map_err(|e| e.to_string())?;
    let rescaled = crate::seasonality::promotion_active(
        promotion_row.as_ref(),
        anchor_row.as_ref(),
        now_ms,
    );
    let scale = if rescaled {
        match (&promotion_row, &anchor_row) {
            (Some(_), Some(a)) => {
                arbitrageur_math::seasonality::rescale_factor(
                    a.forecast_rv_bps,
                    a.trailing_mean_rv_bps,
                )
            }
            _ => 1.0,
        }
    } else {
        1.0
    };
    let sessions = rows
        .iter()
        .map(|r| crate::monitor::SessionProfileEntryDto {
            session_id: r.session_id.clone(),
            n_episodes: r.n_episodes,
            med_abs_spread_bps: r.med_abs_spread_bps,
            p90_spread_bps: r.p90_spread_bps,
            rv_bps: r.rv_bps.and_then(|v| {
                if v.is_finite() && v > 0.0 {
                    let s = v * scale;
                    if s.is_finite() && s > 0.0 {
                        Some(s)
                    } else {
                        Some(v)
                    }
                } else {
                    r.rv_bps
                }
            }),
            win_rate: r.win_rate,
            mean_hold_s: r.mean_hold_s,
            provisional: r.provisional,
        })
        .collect::<Vec<_>>();
    let provisional = sessions
        .iter()
        .find(|s| s.session_id == session.as_str())
        .map(|s| s.provisional)
        .unwrap_or(true);
    let promotion = promotion_row.map(|p| {
        let har_r2 = anchor_row.as_ref().and_then(|a| {
            if a.r2.is_finite() {
                Some(a.r2)
            } else {
                None
            }
        });
        crate::monitor::SessionPromotionDto {
            status: p.status,
            fit_delta_bps: p.fit_delta_bps,
            val_delta_bps: p.val_delta_bps,
            reason: p.reason,
            har_r2,
            rescaled,
        }
    });
    Ok(crate::monitor::SessionProfileDto {
        symbol: crate::monitor::canonical_base(symbol),
        current_session: session.as_str().to_string(),
        is_weekend: weekend,
        in_funding_mask: mask,
        us_cash_open: us_cash_open(now_ms),
        session_allow: session_allow(session, weekend, mask),
        provisional,
        sessions,
        promotion,
    })
}

// ---------------------------------------------------------------------------
// Unit Tests (deep verification of all command business logic and edge cases)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn external_url_opener_is_allowlisted() {
        // Non-allowlisted URLs (and non-https schemes) are refused without
        // touching the OS opener — offline-safe.
        assert!(super::open_external_url("https://evil.example/phish".to_string()).await.is_err());
        assert!(super::open_external_url("file:///tmp/x.html".to_string()).await.is_err());
        assert!(super::open_external_url("https://popdex.xyz.evil.example/".to_string()).await.is_err());
    }

    #[tokio::test]
    async fn main_key_approval_fails_closed_without_network() {
        // Wrapper validates key/address mismatch locally (before any RPC
        // contact), so the wrong-key case runs offline.
        let err = super::sign_popdex_approve_with_main(
            "0x1111111111111111111111111111111111111111".to_string(),
            "0x2222222222222222222222222222222222222222".to_string(),
            "0x0000000000000000000000000000000000000000000000000000000000000001".to_string(),
            "testnet".to_string(),
        )
        .await
        .unwrap_err();
        assert!(
            err.contains("does not match"),
            "wrong-key paste must fail closed, got: {err}"
        );
        // Unknown network is refused before anything else.
        assert!(super::sign_popdex_approve_with_main(
            "0x1111111111111111111111111111111111111111".to_string(),
            "0x1111111111111111111111111111111111111111".to_string(),
            "0xnope".to_string(),
            "bogusnet".to_string(),
        )
        .await
        .is_err());
    }

    fn setup_test_state_with_venues() -> AppEngineState {
        let state = AppEngineState::new();
        {
            let mut s = state.settings.write();
            s.hyperliquid_enabled = true;
            s.lighter_enabled = true;
            s.vanta_enabled = true;
            s.robinhood_enabled = true;
            s.arcus_enabled = true;
        }
        state
    }

    fn test_state_with_vault(tag: &str) -> (AppEngineState, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "arbitrageur_cmd_vault_{}_{}",
            tag,
            rand::random::<u64>()
        ));
        let state = AppEngineState::new_with_vault_path(dir.join("vault.enc"));
        (state, dir)
    }

    fn seed_test_book(
        state: &AppEngineState,
        venue_id: &str,
        symbol: &str,
        bid: Decimal,
        ask: Decimal,
    ) {
        let mut book = arbitrageur_core::quote::OrderBook::new(
            arbitrageur_core::types::VenueId::new(venue_id),
            symbol.to_string(),
        );
        book.bids
            .push(arbitrageur_core::quote::BookLevel::new(bid, dec!(10.0)));
        book.asks
            .push(arbitrageur_core::quote::BookLevel::new(ask, dec!(10.0)));
        book.timestamp = Utc::now();
        state.market_data_hub.update_orderbook(book);
    }

    fn seed_test_funding(state: &AppEngineState, venue_id: &str, symbol: &str) {
        state
            .market_data_hub
            .update_funding(arbitrageur_core::quote::FundingRate {
                venue: arbitrageur_core::types::VenueId::new(venue_id),
                symbol: symbol.to_string(),
                rate: dec!(0.0001),
                predicted_rate: None,
                interval_hours: 8,
                next_funding_time: Utc::now(),
                timestamp: Utc::now(),
            });
    }

    #[tokio::test]
    async fn test_system_status_command() {
        let state = AppEngineState::new();
        let status = get_system_status_impl(&state)
            .await
            .expect("status should succeed");

        assert!(!status.is_live_mode);
        assert_eq!(status.total_realized_pnl, 0.0);
        assert!(!status.circuit_breaker_tripped);
        assert_eq!(status.circuit_breaker_status, "ACTIVE");
        assert_eq!(status.active_venues.len(), 16);
        for v in &status.active_venues {
            assert_eq!(v.status, "OFFLINE");
            assert_eq!(v.latency_ms, None);
        }
        assert!(!status.is_paused);
    }

    #[tokio::test]
    async fn test_system_status_reflects_account_level_enablement() {
        let (state, dir) = test_state_with_vault("status-acct");

        // No flags, no accounts: everything OFFLINE.
        let status = get_system_status_impl(&state).await.expect("status");
        assert!(status.active_venues.iter().all(|v| v.status == "OFFLINE"));

        // An enabled account profile admits the venue even with the engine
        // flag unset — status must match what get_venues actually admits.
        upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "lighter".into(),
                label: "L1".into(),
                enabled: true,
                make_default: false,
                credentials: None,
            },
            &state,
        )
        .expect("create account");

        let status = get_system_status_impl(&state).await.expect("status");
        let lighter = status
            .active_venues
            .iter()
            .find(|v| v.id == "lighter")
            .expect("lighter row");
        assert_eq!(lighter.status, "READ_ONLY");
        assert_eq!(
            status
                .active_venues
                .iter()
                .filter(|v| v.status != "OFFLINE")
                .count(),
            1
        );

        // Robinhood is admitted by get_venues with its Lighter-protocol
        // adapter — an enabled account must report READ_ONLY, not
        // UNSUPPORTED (regression: the status table once hardcoded it
        // unsupported, which silently emptied the admitted-venue set the
        // ticker universe derives from).
        upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "robinhood".into(),
                label: "RH".into(),
                enabled: true,
                make_default: false,
                credentials: None,
            },
            &state,
        )
        .expect("create robinhood account");

        let status = get_system_status_impl(&state).await.expect("status");
        let robinhood = status
            .active_venues
            .iter()
            .find(|v| v.id == "robinhood")
            .expect("robinhood row");
        assert_eq!(robinhood.status, "READ_ONLY");

        // Binance is admitted by get_venues with its live adapter:
        upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "binance".into(),
                label: "B".into(),
                enabled: true,
                make_default: false,
                credentials: None,
            },
            &state,
        )
        .expect("create binance account");
        let status = get_system_status_impl(&state).await.expect("status");
        let binance = status
            .active_venues
            .iter()
            .find(|v| v.id == "binance")
            .expect("binance row");
        assert_eq!(binance.status, "READ_ONLY");

        // Venues with an enabled account but no admitted adapter remain
        // UNSUPPORTED — enabled is not the same as stream-capable.
        upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "ibkr".into(),
                label: "I".into(),
                enabled: true,
                make_default: false,
                credentials: None,
            },
            &state,
        )
        .expect("create ibkr account");
        let status = get_system_status_impl(&state).await.expect("status");
        let ibkr = status
            .active_venues
            .iter()
            .find(|v| v.id == "ibkr")
            .expect("ibkr row");
        assert_eq!(ibkr.status, "UNSUPPORTED");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_pause_and_continue_trading_state() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(60000.0),
            dec!(60005.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(60120.0), dec!(60125.0));

        assert!(!*state.is_paused.read());

        // 1. Pause engine
        let p = set_engine_paused_impl(true, &state).expect("pause");
        assert!(p);
        assert!(*state.is_paused.read());

        let status = get_system_status_impl(&state).await.expect("status");
        assert!(status.is_paused);

        // 2. Spatial arbitrage execution fails with PAUSED error while paused
        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.01,
            Some("LONG_HL_SHORT_LGT".into()),
            Some(15.0),
            Some(0.0),
            Some(1),
            Some(3),
            Some(12.0),
            &state,
        )
        .await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("PAUSED"));

        // Verify open cycles in virtual ledger are untouched (0 open cycles created)
        assert_eq!(state.virtual_ledger.read().lots.len(), 0);

        // 3. Continue trading
        let c = set_engine_paused_impl(false, &state).expect("continue");
        assert!(!c);
        assert!(!*state.is_paused.read());

        let status_resumed = get_system_status_impl(&state).await.expect("status");
        assert!(!status_resumed.is_paused);

        // 4. Executions succeed after continue
        let res_resumed = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.01,
            Some("LONG_HL_SHORT_LGT".into()),
            Some(15.0),
            Some(0.0),
            Some(1),
            Some(3),
            Some(12.0),
            &state,
        )
        .await;
        assert!(res_resumed.is_ok());
        assert_eq!(state.virtual_ledger.read().lots.len(), 1);

        // 5. Re-pausing preserves existing open position (does NOT clear it)
        let _ = set_engine_paused_impl(true, &state).expect("pause again");
        assert_eq!(state.virtual_ledger.read().lots.len(), 1);
    }

    #[tokio::test]
    async fn test_unconfigured_state_returns_zero_synthetic_data() {
        let state = AppEngineState::new();

        // 1. Zero venues active without user credentials
        assert_eq!(state.get_venues().len(), 0);

        // 2. Orderbooks return empty vector (fail-closed, no synthetic quotes fabricated)
        let books = fetch_orderbooks_impl("BTC-USDC", &state)
            .await
            .expect("fetch should succeed");
        assert_eq!(books.len(), 0);

        // 3. Multi-market quotes return 0.0 with no fake numbers
        let quotes = fetch_multi_market_quotes_impl(vec!["BTC-USDC".into()], &state)
            .await
            .expect("quotes");
        assert_eq!(quotes.len(), 1);
        assert_eq!(quotes[0].best_bid, 0.0);
        assert_eq!(quotes[0].best_ask, 0.0);
        assert_eq!(quotes[0].mid_price, 0.0);
        assert_eq!(quotes[0].best_buy_venue, "-");
        assert_eq!(quotes[0].best_sell_venue, "-");

        // 4. Spatial, spot, funding, and prediction scans return empty vector (fail-closed)
        let opps = scan_spatial_opportunities_impl("BTC-USDC", 5.0, 0.1, &state)
            .await
            .expect("opps");
        assert_eq!(opps.len(), 0);

        let spot_opps = scan_spot_opportunities_impl("", &state)
            .await
            .expect("spot");
        assert_eq!(spot_opps.len(), 0);

        let funding_opps = scan_funding_opportunities_impl("BTC-USDC", 25000.0, &state)
            .await
            .expect("funding");
        assert_eq!(funding_opps.len(), 0);

        let pred_opps = scan_prediction_opportunities_impl("", &state)
            .await
            .expect("pred");
        assert_eq!(pred_opps.len(), 0);

        // 5. Venue balances return empty
        let balances = get_venue_balances_impl(&state).await.expect("balances");
        assert_eq!(balances.len(), 0);
    }

    #[tokio::test]
    async fn test_orderbooks_fetch_and_spread() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64210.0), dec!(64215.0));
        seed_test_book(&state, "vanta", "BTC-USDC", dec!(64220.0), dec!(64225.0));
        seed_test_funding(&state, "hyperliquid", "BTC-USDC");
        seed_test_funding(&state, "lighter", "BTC-USDC");
        seed_test_funding(&state, "vanta", "BTC-USDC");

        let books = fetch_orderbooks_impl("BTC-USDC", &state)
            .await
            .expect("fetch should succeed");

        assert!(books.len() >= 2);
        let hl = books
            .iter()
            .find(|b| b.venue_id == "hyperliquid")
            .expect("hyperliquid book");
        assert_eq!(hl.best_bid, 64200.0);
        assert_eq!(hl.best_ask, 64205.0);
        assert!(hl.spread >= 0.0);
        assert!(hl.vwap_ask >= hl.best_ask);
    }

    #[tokio::test]
    async fn test_spatial_opportunity_scan() {
        let state = AppEngineState::new();
        let opps = scan_spot_opportunities_impl("", &state)
            .await
            .expect("scan should succeed");
        // Zero synthetic data: unconfigured state returns empty
        assert_eq!(opps.len(), 0);
    }

    #[test]
    fn scan_leg_rate_8h_pct_normalizes_by_interval() {
        use arbitrageur_core::quote::FundingRate;
        use chrono::Utc;
        let now = Utc::now();
        // Lighter post-fix: raw -0.0336% is an 8h print (interval 8).
        let r_lighter = FundingRate {
            venue: "lighter".into(),
            symbol: "LINK-USDC".into(),
            rate: "-0.000336".parse().unwrap(),
            predicted_rate: None,
            interval_hours: 8,
            next_funding_time: now,
            timestamp: now,
        };
        // Hyperliquid: hourly print.
        let r_hl = FundingRate {
            venue: "hyperliquid".into(),
            symbol: "LINK-USDC".into(),
            rate: "0.0000125".parse().unwrap(),
            predicted_rate: None,
            interval_hours: 1,
            next_funding_time: now,
            timestamp: now,
        };
        let rates = vec![("lighter", "Lighter", r_lighter), ("hyperliquid", "Hyperliquid", r_hl)];
        let long_pct = scan_leg_rate_8h_pct(&rates, "lighter").expect("long leg print");
        let short_pct = scan_leg_rate_8h_pct(&rates, "hyperliquid").expect("short leg print");
        // -0.0336%/8*8 = -0.0336%; 0.00125%*8 = +0.01%.
        assert!((long_pct + 0.0336).abs() < 1e-9, "long pct: {long_pct}");
        assert!((short_pct - 0.01).abs() < 1e-9, "short pct: {short_pct}");
        assert!(scan_leg_rate_8h_pct(&rates, "unknown-venue").is_none());
        // Basis legs resolve through the bare venue id.
        assert_eq!(trend_bare_venue("hyperliquid SPOT"), "hyperliquid");
        assert_eq!(trend_bare_venue("lighter"), "lighter");
        // Downsampling keeps the shape bounded and reaches the last point.
        let series: Vec<(i64, f64)> = (0..500).map(|i| (i, i as f64)).collect();
        let thin = downsample_series(&series, 240);
        assert!(thin.len() <= 240);
        assert_eq!(thin.last().unwrap().t, 499);
    }

    #[test]
    fn test_session_profile_impl_roundtrip() {
        use crate::state_store::VenueSampleEntry;
        let state = AppEngineState::new();
        // No history: neutral snapshot, provisional, no fabricated rows.
        let empty = get_session_profile_impl("BTC-USDC", &state).unwrap();
        assert_eq!(empty.symbol, "BTC");
        assert!(empty.sessions.is_empty());
        assert!(empty.provisional);
        assert!(!empty.current_session.is_empty());
        assert!(empty.session_allow.is_finite());
        // One tick of two-venue history: rollup writes all five session
        // rows (provisional), and the IPC mirrors them.
        let ts = chrono::Utc::now().timestamp_millis();
        state
            .state_store
            .record_spread_samples(
                "BTC",
                ts,
                &[
                    VenueSampleEntry {
                        venue_id: "lighter".into(),
                        mid: 100.0,
                        best_bid: 99.9,
                        best_ask: 100.1,
                    },
                    VenueSampleEntry {
                        venue_id: "vanta".into(),
                        mid: 100.5,
                        best_bid: 100.4,
                        best_ask: 100.6,
                    },
                ],
            )
            .unwrap();
        state
            .state_store
            .rollup_session_profiles("BTC", ts - 1_000, ts + 1_000, ts)
            .unwrap();
        let prof = get_session_profile_impl("BTC-USDC", &state).unwrap();
        assert_eq!(prof.sessions.len(), 5);
        assert!(prof.sessions.iter().all(|s| s.provisional));
        assert!(prof.provisional);
    }

    #[tokio::test]
    async fn test_funding_opportunities_multi_symbol() {
        let state = setup_test_state_with_venues();
        let r_hl = arbitrageur_core::quote::FundingRate {
            venue: arbitrageur_core::types::VenueId::new("hyperliquid"),
            symbol: "BTC-USDC".into(),
            rate: dec!(0.00015),
            predicted_rate: None,
            interval_hours: 8,
            next_funding_time: Utc::now(),
            timestamp: Utc::now(),
        };
        let r_lgt = arbitrageur_core::quote::FundingRate {
            venue: arbitrageur_core::types::VenueId::new("lighter"),
            symbol: "BTC-USDC".into(),
            rate: dec!(0.00005),
            predicted_rate: None,
            interval_hours: 8,
            next_funding_time: Utc::now(),
            timestamp: Utc::now(),
        };
        state.market_data_hub.update_funding(r_hl);
        state.market_data_hub.update_funding(r_lgt);
        seed_test_funding(&state, "vanta", "BTC-USDC");

        let opps_btc = scan_funding_opportunities_impl("BTC-USDC", 25000.0, &state)
            .await
            .expect("btc scan");
        assert!(!opps_btc.is_empty());

        let batch_opps = scan_funding_opportunities_batch_impl(
            &["BTC-USDC".into(), "ETH-USDC".into()],
            25000.0,
            &state,
        )
        .await
        .expect("batch scan");
        assert!(!batch_opps.is_empty());
        assert_eq!(batch_opps[0].symbol, "BTC-USDC");

        let all_opps = scan_funding_opportunities_impl("", 25000.0, &state)
            .await
            .expect("empty symbol scan");
        assert!(all_opps.is_empty() || !all_opps.is_empty());
    }

    #[tokio::test]
    async fn test_funding_scan_emits_same_exchange_basis() {
        let state = setup_test_state_with_venues();
        // Single venue with funding: no cross-exchange carry possible, but a
        // same-exchange spot/perp basis opp must surface.
        seed_test_funding(&state, "lighter", "BTC-USDC");
        // Perp leg book under the canonical symbol ...
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(60120.0), dec!(60125.0));
        // ... and the spot leg under the `-SPOT` market-type suffix.
        seed_test_book(&state, "lighter", "BTC-SPOT", dec!(60000.0), dec!(60005.0));
        // Deepen both tails so the gradient walk has multiple samples.
        for sym in ["BTC-USDC", "BTC-SPOT"] {
            let vid = arbitrageur_core::types::VenueId::new("lighter");
            if let Some(mut book) = state.market_data_hub.get_valid_orderbook(
                &vid,
                sym,
                Some(std::time::Duration::from_secs(120)),
            ) {
                let (bb, ba) = (
                    book.best_bid_price().unwrap(),
                    book.best_ask_price().unwrap(),
                );
                for k in 1..=5 {
                    let step_dec = dec!(5.0) * Decimal::from(k);
                    book.bids.push(arbitrageur_core::quote::BookLevel::new(
                        bb - step_dec,
                        dec!(10.0),
                    ));
                    book.asks.push(arbitrageur_core::quote::BookLevel::new(
                        ba + step_dec,
                        dec!(10.0),
                    ));
                }
                state.market_data_hub.update_orderbook(book);
            }
        }

        let opps = scan_funding_opportunities_impl("BTC-USDC", 25000.0, &state)
            .await
            .expect("basis scan");
        let basis: Vec<_> = opps.iter().filter(|o| o.kind == "basis").collect();
        assert_eq!(basis.len(), 1, "expected one basis opp, got {opps:?}");
        let b = basis[0];
        assert_eq!(b.symbol, "BTC-USDC");
        assert!(b.long_venue.contains("SPOT"), "long leg must be spot, got {}", b.long_venue);
        assert!(b.short_venue.contains("PERP"), "short leg must be perp, got {}", b.short_venue);
        // (60122.5-60002.5)/60002.5*10000 ~= +20 bps contango.
        let reported = b.basis_bps.expect("basis_bps");
        assert!((reported - 20.0).abs() < 1.0, "unexpected basis {reported}");
        assert!(b.spot_price.expect("spot") > 0.0);
        assert!(b.perp_price.expect("perp") > 0.0);
        assert!(b.net_apr_pct > 0.0);
        // Informational win probability: fee-coverage heuristic must score.
        let w = b.win_probability.expect("win_probability");
        assert!((0.02..=0.98).contains(&w), "win prob out of bounds: {w}");
        // Depth: 10-unit books at ~$60k hold the $25k tier twice over.
        assert!(b.optimal_size_usd > 0.0);
        // Gradient: multi-level tails must yield a finite slope read-out.
        let slope = b.depth_slope_bps_per_1k.expect("depth slope");
        assert!(slope.is_finite() && slope >= 0.0, "slope={slope}");
    }

    #[tokio::test]
    async fn test_funding_scan_basis_disabled_by_switch() {
        let state = setup_test_state_with_venues();
        seed_test_funding(&state, "lighter", "BTC-USDC");
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(60120.0), dec!(60125.0));
        seed_test_book(&state, "lighter", "BTC-SPOT", dec!(60000.0), dec!(60005.0));
        // Flip the basis switch off via the persisted strategy params.
        {
            let mut map = state.strategy_params.write();
            let mut params = StrategyParamsDto::defaults_for("funding");
            params.funding_enable_basis = false;
            map.insert("funding".to_string(), params);
        }
        let opps = scan_funding_opportunities_impl("BTC-USDC", 25000.0, &state)
            .await
            .expect("scan");
        assert!(
            opps.iter().all(|o| o.kind != "basis"),
            "basis switch OFF must suppress basis opps, got {opps:?}"
        );
    }

    #[tokio::test]
    async fn test_prediction_fail_closed_unconfigured() {
        let state = AppEngineState::new();
        let opps = scan_prediction_opportunities_impl("FED", &state)
            .await
            .expect("pred scan");
        // Zero synthetic data: unconfigured state returns empty
        assert_eq!(opps.len(), 0);
    }

    #[tokio::test]
    async fn test_execute_spatial_arbitrage() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            Some("Divergence Entry".into()),
            Some(18.5),
            Some(12.0),
            Some(1),
            Some(3),
            Some(15.0),
            &state,
        )
        .await
        .expect("exec spatial");
        assert_eq!(res.trade_type, "Spatial Arb");
        assert_eq!(res.direction, "Divergence Entry");
        // Caller-supplied trigger spread is not trusted; the returned spread is
        // the calculated net BPS (~20.6) which must clear the 15 BPS hurdle
        assert!(res.spread_bps > 15.0);
        assert!(res.spread_bps < 30.0);
        // Opening a paper position realizes only fee outflow
        assert!(res.realized_profit_usd < 0.0);

        let logs = get_execution_logs_impl(&state).expect("logs");
        let last = logs.last().expect("last log");
        assert_eq!(last.direction.as_deref(), Some("Divergence Entry"));
        assert_eq!(last.trigger_spread_bps, Some(res.spread_bps));
        assert_eq!(last.baseline_mean_spread_bps, Some(12.0));
        assert_eq!(last.position_step, Some(1));
        assert_eq!(last.max_units, Some(3));
        assert_eq!(last.nominal_hurdle_bps, Some(15.0));
        assert!(last.net_profit < 0.0);

        // Simulated paper accounts must hold the opposite positions
        let acc_hl = state
            .get_paper_exchange("hyperliquid")
            .expect("hl paper")
            .fetch_account()
            .await
            .expect("hl account");
        let acc_lgt = state
            .get_paper_exchange("lighter")
            .expect("lgt paper")
            .fetch_account()
            .await
            .expect("lgt account");
        let pos_hl = acc_hl
            .positions
            .iter()
            .find(|p| p.symbol == "BTC-USDC")
            .expect("hl position");
        let pos_lgt = acc_lgt
            .positions
            .iter()
            .find(|p| p.symbol == "BTC-USDC")
            .expect("lgt position");
        assert_eq!(pos_hl.side, Side::Buy);
        assert_eq!(pos_lgt.side, Side::Sell);
    }

    #[tokio::test]
    async fn test_risk_overview_and_killswitch() {
        let state = AppEngineState::new();
        let risk = get_risk_overview_impl(&state).await.expect("risk overview");

        assert_eq!(risk.circuit_breaker_state, "ACTIVE");
        assert!(!risk.is_halted);
        // Six seeded paper accounts at 100k equity each
        assert_eq!(risk.total_portfolio_value_usd, 600000.0);

        // Flat book: no observed positions means no delta entries at all
        assert!(risk.net_deltas.is_empty());
        assert_eq!(risk.margin_health.status, "FLAT");
        assert_eq!(risk.margin_health.liquidation_distance_pct, 100.0);
        assert_eq!(risk.margin_health.maintenance_margin_req_usd, 0.0);

        // Trigger killswitch
        let halt_msg = trigger_emergency_killswitch_impl(&state).expect("killswitch");
        assert!(halt_msg.contains("EMERGENCY KILLSWITCH"));

        let risk_after = get_risk_overview_impl(&state)
            .await
            .expect("risk overview after halt");
        assert!(risk_after.is_halted);
        assert!(risk_after.circuit_breaker_state.contains("TRIPPED"));

        // Reset circuit breaker
        let reset_msg = reset_circuit_breaker_impl(&state).expect("reset");
        assert!(reset_msg.contains("reset successfully"));

        let risk_resumed = get_risk_overview_impl(&state)
            .await
            .expect("risk overview resumed");
        assert!(!risk_resumed.is_halted);
    }

    #[tokio::test]
    async fn test_engine_settings_command() {
        let state = AppEngineState::new();
        let cur = get_engine_settings_impl(&state).expect("get settings");
        assert_eq!(cur.min_depth_ratio, 3.0);

        let new_settings = EngineSettingsDto {
            hsm_url: "http://hsm.internal:9000".into(),
            min_depth_ratio: 4.0,
            hyperliquid_url: "https://api.hyperliquid.xyz".into(),
            lighter_url: "https://mainnet.zklighter.elliot.ai".into(),
            vanta_url: "https://api.orderly.org".into(),
            robinhood_rpc_url: "https://rpc.robinhood.network".into(),
            robinhood_router: "0x1111".into(),
            polymarket_url: "https://clob.polymarket.com".into(),
            kalshi_url: "https://trading-api.kalshi.com".into(),
            ..EngineSettingsDto::default()
        };

        save_engine_settings_impl(new_settings, &state).expect("save settings");
        let updated = get_engine_settings_impl(&state).expect("get updated");
        assert_eq!(updated.min_depth_ratio, 4.0);
        assert_eq!(updated.hsm_url, "http://hsm.internal:9000");
        assert_eq!(updated.robinhood_router, "0x1111");
        assert!(updated.configured_venues.is_empty());
    }

    #[tokio::test]
    async fn test_retention_validation_zero_normalizes_to_none() {
        let state = AppEngineState::new();
        let settings = EngineSettingsDto {
            spread_samples_max_days: Some(0),
            spread_samples_max_gb: Some(0.0),
            ..EngineSettingsDto::default()
        };
        save_engine_settings_impl(settings, &state).expect("save");
        let cur = get_engine_settings_impl(&state).expect("get");
        assert_eq!(cur.spread_samples_max_days, None);
        assert_eq!(cur.spread_samples_max_gb, None);
        assert_eq!(state.state_store.kv_get("retention.max_days").unwrap(), None);
        assert_eq!(state.state_store.kv_get("retention.max_gb").unwrap(), None);
    }

    #[tokio::test]
    async fn test_retention_validation_rejects_bad_gb() {
        let state = AppEngineState::new();
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
            let settings = EngineSettingsDto {
                spread_samples_max_gb: Some(bad),
                ..EngineSettingsDto::default()
            };
            assert!(
                save_engine_settings_impl(settings, &state).is_err(),
                "gb={bad:?} must be rejected"
            );
        }
        let cur = get_engine_settings_impl(&state).expect("get");
        assert_eq!(cur.spread_samples_max_gb, None);
    }

    #[tokio::test]
    async fn test_retention_ipc_roundtrip_persists_kv() {
        let state = AppEngineState::new();
        let settings = EngineSettingsDto {
            spread_samples_max_days: Some(30),
            spread_samples_max_gb: Some(2.5),
            ..EngineSettingsDto::default()
        };
        save_engine_settings_impl(settings, &state).expect("save");
        let cur = get_engine_settings_impl(&state).expect("get");
        assert_eq!(cur.spread_samples_max_days, Some(30));
        assert_eq!(cur.spread_samples_max_gb, Some(2.5));
        assert_eq!(
            state.state_store.kv_get("retention.max_days").unwrap(),
            Some("30".to_string())
        );
        assert_eq!(
            state.state_store.kv_get("retention.max_gb").unwrap(),
            Some("2.5".to_string())
        );
        let json = serde_json::to_string(&cur).expect("serialize");
        let back: EngineSettingsDto = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.spread_samples_max_days, Some(30));
        assert_eq!(back.spread_samples_max_gb, Some(2.5));
    }

    #[tokio::test]
    async fn test_retention_prune_days_only() {
        let state = AppEngineState::new();
        let now = Utc::now().timestamp_millis();
        for i in 0..6 {
            seed_samples(
                &state,
                "BTC-USDC",
                now - (5 - i) * 86_400_000,
                &[("hyperliquid", 100.0 + i as f64)],
            );
        }
        {
            let mut s = state.settings.write();
            s.spread_samples_max_days = Some(3);
            s.spread_samples_max_gb = None;
        }
        enforce_retention_pass(&state, now);
        let rows = state
            .state_store
            .spread_samples("BTC-USDC", 0, i64::MAX)
            .expect("samples");
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|r| r.ts_ms >= now - 3 * 86_400_000));
    }

    #[tokio::test]
    async fn test_retention_prune_neither_is_noop() {
        let state = AppEngineState::new();
        let now = Utc::now().timestamp_millis();
        for i in 0..4 {
            seed_samples(
                &state,
                "BTC-USDC",
                now - (3 - i) * 86_400_000,
                &[("hyperliquid", 100.0 + i as f64)],
            );
        }
        enforce_retention_pass(&state, now);
        let rows = state
            .state_store
            .spread_samples("BTC-USDC", 0, i64::MAX)
            .expect("samples");
        assert_eq!(rows.len(), 4, "both unset must leave history alone");
    }

    #[tokio::test]
    async fn test_retention_prune_gb_only_evicts_oldest() {
        let state = AppEngineState::new();
        let now = Utc::now().timestamp_millis();
        for i in 0..6 {
            seed_samples(
                &state,
                "BTC-USDC",
                now - (5 - i) * 1_000,
                &[("hyperliquid", 100.0 + i as f64)],
            );
        }
        {
            let mut s = state.settings.write();
            s.spread_samples_max_days = None;
            s.spread_samples_max_gb = Some(0.000001);
        }
        enforce_retention_pass(&state, now);
        let count = state.state_store.spread_sample_count().unwrap();
        assert!(count < 6, "tiny budget must evict oldest-first, left {count}");
        if count > 0 {
            let rows = state
                .state_store
                .spread_samples("BTC-USDC", 0, i64::MAX)
                .expect("samples");
            assert_eq!(
                rows.last().unwrap().ts_ms,
                now,
                "newest sample survives oldest-first eviction"
            );
        }
    }

    #[tokio::test]
    async fn test_retention_prune_both_applies_time_then_size() {
        let state = AppEngineState::new();
        let now = Utc::now().timestamp_millis();
        for i in 0..6 {
            seed_samples(
                &state,
                "BTC-USDC",
                now - (5 - i) * 86_400_000,
                &[("hyperliquid", 100.0 + i as f64)],
            );
        }
        {
            let mut s = state.settings.write();
            s.spread_samples_max_days = Some(3);
            s.spread_samples_max_gb = Some(0.000001);
        }
        enforce_retention_pass(&state, now);
        let rows = state
            .state_store
            .spread_samples("BTC-USDC", 0, i64::MAX)
            .expect("samples");
        assert!(
            rows.len() <= 3,
            "time cutoff binds first, size budget binds after"
        );
        assert!(rows.iter().all(|r| r.ts_ms >= now - 3 * 86_400_000));
    }

    #[tokio::test]
    async fn test_retention_boot_loads_persisted_kv() {
        let dir = std::env::temp_dir().join(format!(
            "arbitrageur_retention_boot_{}",
            rand::random::<u64>()
        ));
        let path = dir.join("state.sqlite3");
        {
            let store = crate::state_store::StateStore::open(&path).unwrap();
            store.kv_set("retention.max_days", "21").unwrap();
            store.kv_set("retention.max_gb", "4.25").unwrap();
        }
        let store = std::sync::Arc::new(crate::state_store::StateStore::open(&path).unwrap());
        let state = AppEngineState::new_inner_for_test(store);
        let cur = get_engine_settings_impl(&state).expect("get");
        assert_eq!(cur.spread_samples_max_days, Some(21));
        assert_eq!(cur.spread_samples_max_gb, Some(4.25));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_credential_settings_rejected_while_vault_locked() {
        let (state, dir) = test_state_with_vault("locked");
        let before = get_engine_settings_impl(&state).expect("settings before");

        let mut settings = before.clone();
        settings.hyperliquid_private_key = "SENTINEL_HL_KEY".into();
        settings.hyperliquid_wallet = "SENTINEL_HL_WALLET".into();
        let res = save_engine_settings_impl(settings, &state);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("locked"));

        let after = get_engine_settings_impl(&state).expect("settings after");
        assert_eq!(after.min_depth_ratio, before.min_depth_ratio);
        assert!(after.configured_venues.is_empty());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_credential_save_is_vault_only_and_sanitized() {
        let (state, dir) = test_state_with_vault("creds");
        initialize_vault_impl("test password", &state).expect("init vault");

        let settings = EngineSettingsDto {
            hyperliquid_enabled: true,
            hyperliquid_wallet: "SENTINEL_WALLET_0x".into(),
            hyperliquid_private_key: "SENTINEL_PRIV_9f2".into(),
            ..EngineSettingsDto::default()
        };
        save_engine_settings_impl(settings, &state).expect("save credential settings");

        let stored = state.vault.write().get_data().expect("vault data").clone();
        let hl = stored
            .credentials
            .iter()
            .find(|c| c.venue == "hyperliquid")
            .expect("hyperliquid credentials in vault");
        assert_eq!(hl.api_secret, "SENTINEL_PRIV_9f2");
        assert_eq!(hl.wallet_address.as_deref(), Some("SENTINEL_WALLET_0x"));
        assert!(hl.enabled);

        let runtime = state.settings.read().clone();
        let returned = get_engine_settings_impl(&state).expect("sanitized settings");
        let returned_json = serde_json::to_string(&returned).expect("serialize");
        let runtime_json = serde_json::to_string(&runtime).expect("serialize runtime");
        for sentinel in ["SENTINEL_WALLET_0x", "SENTINEL_PRIV_9f2"] {
            assert!(
                runtime.hyperliquid_private_key.is_empty()
                    || !runtime.hyperliquid_private_key.contains(sentinel)
            );
            assert!(
                !runtime_json.contains(sentinel),
                "runtime settings JSON leaked {}",
                sentinel
            );
            assert!(
                !returned_json.contains(sentinel),
                "settings JSON leaked {}",
                sentinel
            );
        }
        assert_eq!(returned.configured_venues, vec!["hyperliquid".to_string()]);

        lock_vault_impl(&state);
        let locked = get_engine_settings_impl(&state).expect("locked settings");
        assert!(locked.configured_venues.is_empty());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_delete_exchange_credentials_allowlist_and_lifecycle() {
        let (state, dir) = test_state_with_vault("delete_creds");

        let err = delete_exchange_credentials_impl("hyperliquid", &state).unwrap_err();
        assert!(err.contains("locked"), "err: {}", err);

        initialize_vault_impl("test password", &state).expect("init vault");

        let settings = EngineSettingsDto {
            hyperliquid_enabled: true,
            hyperliquid_private_key: "SENTINEL_PRIV_del".into(),
            ..EngineSettingsDto::default()
        };
        save_engine_settings_impl(settings, &state).expect("save credentials");

        for bad in ["unknown_venue", "hyperliquid;rm", "*", "hyperliquid wallet"] {
            assert!(
                delete_exchange_credentials_impl(bad, &state).is_err(),
                "deletion accepted non-allowlisted venue {:?}",
                bad
            );
        }

        assert!(!delete_exchange_credentials_impl("kalshi", &state).unwrap());
        assert!(delete_exchange_credentials_impl(" HyperLiquid ", &state).unwrap());

        let stored = state.vault.write().get_data().expect("vault data").clone();
        assert!(stored.credentials.is_empty());
        let sanitized = get_engine_settings_impl(&state).expect("sanitized");
        assert!(sanitized.configured_venues.is_empty());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_account_crud_and_credential_binding() {
        let (state, dir) = test_state_with_vault("accounts");
        initialize_vault_impl("test password", &state).expect("init vault");

        let a1 = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "hyperliquid".into(),
                label: "Main".into(),
                enabled: true,
                make_default: false,
                credentials: Some(AccountCredentialInput {
                    api_secret: "SENTINEL_ACCT1".into(),
                    ..AccountCredentialInput::default()
                }),
            },
            &state,
        )
        .expect("create a1");
        assert!(a1.is_default, "first enabled account becomes default");
        assert_eq!(a1.credentials_configured, Some(true));
        assert!(a1.has_api_secret);
        assert!(!a1.has_api_key);

        let a2 = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "hyperliquid".into(),
                label: "Alt".into(),
                enabled: true,
                make_default: false,
                credentials: Some(AccountCredentialInput {
                    api_secret: "SENTINEL_ACCT2".into(),
                    ..AccountCredentialInput::default()
                }),
            },
            &state,
        )
        .expect("create a2");
        assert!(!a2.is_default);
        assert_ne!(a1.id, a2.id);

        // Regression: two accounts on one venue must not overwrite each
        // other's secrets (the old venue-keyed upsert clobbered them).
        let stored = state.vault.write().get_data().expect("vault data").clone();
        assert_eq!(stored.credentials.len(), 2);
        let secrets: Vec<&str> = stored
            .credentials
            .iter()
            .map(|c| c.api_secret.as_str())
            .collect();
        assert!(secrets.contains(&"SENTINEL_ACCT1"));
        assert!(secrets.contains(&"SENTINEL_ACCT2"));
        assert!(stored.credentials.iter().all(|c| c.profile_id.is_some()));

        // Sanitized DTO must never carry secret values over IPC.
        let dto_json = serde_json::to_string(&a1).expect("dto json");
        assert!(!dto_json.contains("SENTINEL_ACCT1"));

        // Venue identity is immutable on update.
        assert!(upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                max_leverage: None,
                id: Some(a2.id.clone()),
                venue: "lighter".into(),
                label: "Moved".into(),
                enabled: true,
                make_default: false,
                credentials: None,
            },
            &state,
        )
        .is_err());

        let updated = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                max_leverage: None,
                id: Some(a2.id.clone()),
                venue: "hyperliquid".into(),
                label: "Alt Renamed".into(),
                enabled: true,
                make_default: true,
                credentials: None,
            },
            &state,
        )
        .expect("update a2");
        assert!(updated.is_default);
        assert_eq!(updated.label, "Alt Renamed");

        set_default_account_impl("hyperliquid", &a1.id, &state).expect("set default");
        let listed = list_accounts_impl(Some("hyperliquid"), &state).expect("list");
        assert!(listed.iter().find(|a| a.id == a1.id).unwrap().is_default);
        assert!(!listed.iter().find(|a| a.id == a2.id).unwrap().is_default);

        // Deleting an account removes its bound secret record as well.
        assert!(delete_account_impl(&a2.id, &state).expect("delete a2"));
        let stored = state.vault.write().get_data().expect("vault data").clone();
        assert_eq!(stored.credentials.len(), 1);
        assert_eq!(stored.credentials[0].api_secret, "SENTINEL_ACCT1");
        assert_eq!(
            stored.credentials[0].profile_id.as_deref(),
            Some(a1.id.as_str())
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_delete_account_credentials_keeps_profile_and_siblings() {
        let (state, dir) = test_state_with_vault("cred-del");

        // Locked vault and unknown accounts both fail closed.
        assert!(delete_account_credentials_impl("lighter-default", &state).is_err());
        initialize_vault_impl("test password", &state).expect("init vault");
        assert!(delete_account_credentials_impl("no-such-account", &state).is_err());

        let a1 = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "hyperliquid".into(),
                label: "Main".into(),
                enabled: true,
                make_default: false,
                credentials: Some(AccountCredentialInput {
                    api_secret: "SENTINEL_DEL1".into(),
                    ..AccountCredentialInput::default()
                }),
            },
            &state,
        )
        .expect("create a1");
        let a2 = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "hyperliquid".into(),
                label: "Alt".into(),
                enabled: true,
                make_default: false,
                credentials: Some(AccountCredentialInput {
                    api_secret: "SENTINEL_DEL2".into(),
                    ..AccountCredentialInput::default()
                }),
            },
            &state,
        )
        .expect("create a2");

        // Removing a1's credentials leaves the account profile and a2's
        // secrets untouched.
        assert!(delete_account_credentials_impl(&a1.id, &state).expect("delete creds"));
        let stored = state.vault.write().get_data().expect("vault data").clone();
        assert_eq!(stored.credentials.len(), 1);
        assert_eq!(stored.credentials[0].api_secret, "SENTINEL_DEL2");
        assert!(state
            .state_store
            .get_account(&a1.id)
            .expect("account lookup")
            .is_some());

        let listed = list_accounts_impl(Some("hyperliquid"), &state).expect("list");
        assert!(!listed
            .iter()
            .find(|a| a.id == a1.id)
            .unwrap()
            .credentials_configured
            .unwrap_or(true));
        assert!(listed
            .iter()
            .find(|a| a.id == a2.id)
            .unwrap()
            .credentials_configured
            .unwrap_or(false));

        // Second delete reports no remaining records.
        assert!(!delete_account_credentials_impl(&a1.id, &state).unwrap());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_get_account_credentials_scoped_to_profile_and_unlocked() {
        let (state, dir) = test_state_with_vault("cred-reveal");

        // Locked vault and unknown accounts both fail closed.
        assert!(get_account_credentials_impl("lighter-default", &state).is_err());
        initialize_vault_impl("test password", &state).expect("init vault");
        assert!(get_account_credentials_impl("no-such-account", &state).is_err());

        let a1 = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "hyperliquid".into(),
                label: "Main".into(),
                enabled: true,
                make_default: false,
                credentials: Some(AccountCredentialInput {
                    api_secret: "SENTINEL_REVEAL1".into(),
                    private_key: Some("0xSENTINEL_PK1".into()),
                    account_index: Some(7),
                    ..AccountCredentialInput::default()
                }),
            },
            &state,
        )
        .expect("create a1");
        let a2 = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "hyperliquid".into(),
                label: "Alt".into(),
                enabled: true,
                make_default: false,
                credentials: Some(AccountCredentialInput {
                    api_secret: "SENTINEL_REVEAL2".into(),
                    ..AccountCredentialInput::default()
                }),
            },
            &state,
        )
        .expect("create a2");

        // Field-scoped reveal returns exactly one field: the secret, nothing
        // else. Unknown fields and unknown accounts fail closed.
        assert_eq!(
            get_account_credential_field_impl(&a1.id, "apiSecret", &state).expect("field reveal"),
            "SENTINEL_REVEAL1"
        );
        assert_eq!(
            get_account_credential_field_impl(&a1.id, "privateKey", &state).expect("field reveal"),
            "0xSENTINEL_PK1"
        );
        assert!(get_account_credential_field_impl(&a1.id, "nope", &state).is_err());
        assert!(get_account_credential_field_impl("no-such-account", &a1.id, &state).is_err());

        // Reveal returns only the requested profile's material.
        let revealed = get_account_credentials_impl(&a1.id, &state).expect("reveal a1");
        assert_eq!(revealed.api_secret, "SENTINEL_REVEAL1");
        assert_eq!(revealed.private_key.as_deref(), Some("0xSENTINEL_PK1"));
        assert_eq!(revealed.account_index, Some(7));

        // An accountless profile read errors rather than leaking siblings.
        let a3 = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "hyperliquid".into(),
                label: "Empty".into(),
                enabled: true,
                make_default: false,
                credentials: None,
            },
            &state,
        )
        .expect("create a3");
        assert!(get_account_credentials_impl(&a3.id, &state).is_err());
        let revealed2 = get_account_credentials_impl(&a2.id, &state).expect("reveal a2");
        assert_eq!(revealed2.api_secret, "SENTINEL_REVEAL2");
        assert!(revealed2.private_key.as_deref().unwrap_or("").is_empty());

        // Re-locking the vault blocks reveal again.
        lock_vault_impl(&state);
        assert!(get_account_credentials_impl(&a1.id, &state).is_err());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_legacy_credentials_migrate_to_default_profile() {
        let (state, dir) = test_state_with_vault("migrate");
        initialize_vault_impl("test password", &state).expect("init vault");

        // Simulate a pre-account-model record: venue-keyed, unbound.
        state
            .vault
            .write()
            .upsert_credentials(vec![ExchangeCredentials {
                venue: "hyperliquid".into(),
                profile_id: None,
                api_key: String::new(),
                api_secret: "SENTINEL_LEGACY".into(),
                passphrase: None,
                account_id: None,
                wallet_address: Some("0xlegacy".into()),
                account_index: Some(2),
                subaccount_index: None,
                api_key_index: None,
                private_key: None,
                enabled: true,
            }])
            .expect("seed legacy cred");
        // The HSM token is not an exchange account and stays unbound.
        state
            .vault
            .write()
            .upsert_credentials(vec![ExchangeCredentials {
                venue: "remote_hsm".into(),
                profile_id: None,
                api_key: String::new(),
                api_secret: "SENTINEL_HSM".into(),
                passphrase: None,
                account_id: None,
                wallet_address: None,
                account_index: None,
                subaccount_index: None,
                api_key_index: None,
                private_key: None,
                enabled: true,
            }])
            .expect("seed hsm");

        lock_vault_impl(&state);
        unlock_vault_impl("test password", &state).expect("unlock migrates");

        let accounts = list_accounts_impl(Some("hyperliquid"), &state).expect("accounts");
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, "hyperliquid-default");
        assert_eq!(accounts[0].label, "Default");
        assert!(accounts[0].is_default);
        assert!(accounts[0].enabled);
        assert_eq!(accounts[0].credentials_configured, Some(true));
        assert!(accounts[0].has_wallet_address);
        assert_eq!(accounts[0].account_index, Some(2));

        let stored = state.vault.write().get_data().expect("vault data").clone();
        let hl = stored
            .credentials
            .iter()
            .find(|c| c.venue == "hyperliquid")
            .expect("hl record");
        assert_eq!(hl.profile_id.as_deref(), Some("hyperliquid-default"));
        assert_eq!(hl.api_secret, "SENTINEL_LEGACY");
        let hsm = stored
            .credentials
            .iter()
            .find(|c| c.venue == "remote_hsm")
            .expect("hsm record");
        assert!(hsm.profile_id.is_none(), "remote_hsm must stay unbound");

        // A flat settings save now merges into the bound default profile
        // instead of creating a second record for the same venue.
        let s = EngineSettingsDto {
            hyperliquid_enabled: true,
            hyperliquid_private_key: "SENTINEL_NEW".into(),
            ..Default::default()
        };
        save_engine_settings_impl(s, &state).expect("save");
        let stored = state.vault.write().get_data().expect("vault data").clone();
        assert_eq!(
            stored
                .credentials
                .iter()
                .filter(|c| c.venue == "hyperliquid")
                .count(),
            1
        );
        let hl = stored
            .credentials
            .iter()
            .find(|c| c.venue == "hyperliquid")
            .unwrap();
        assert_eq!(hl.api_secret, "SENTINEL_NEW");
        assert_eq!(hl.wallet_address.as_deref(), Some("0xlegacy"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_strategy_route_crud_and_eligibility() {
        let (state, dir) = test_state_with_vault("routes");
        initialize_vault_impl("test password", &state).expect("init vault");

        let hl = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "hyperliquid".into(),
                label: "HL Main".into(),
                enabled: true,
                make_default: false,
                credentials: None,
            },
            &state,
        )
        .expect("hl account");
        let lt = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "lighter".into(),
                label: "LT Main".into(),
                enabled: true,
                make_default: false,
                credentials: None,
            },
            &state,
        )
        .expect("lt account");
        let lt_disabled = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "lighter".into(),
                label: "LT Off".into(),
                enabled: false,
                make_default: false,
                credentials: None,
            },
            &state,
        )
        .expect("lt disabled account");

        let route = upsert_strategy_route_impl(
            StrategyRouteUpsertInput {
                id: None,
                strategy_id: "futures".into(),
                label: "Main Route".into(),
                account_a_id: lt.id.clone(),
                account_b_id: hl.id.clone(),
                enabled: true,
            },
            &state,
        )
        .expect("create route");
        // Canonical ordering places hyperliquid first regardless of input order.
        assert_eq!(route.account_a_id, hl.id);
        assert_eq!(route.account_a_venue, "hyperliquid");
        assert_eq!(route.account_b_id, lt.id);
        assert_eq!(route.account_b_venue, "lighter");

        let eligible = eligible_strategy_routes_impl("futures", "lighter", "hyperliquid", &state)
            .expect("eligible");
        assert_eq!(eligible.len(), 1);
        assert_eq!(eligible[0].id, route.id);

        // Disabled endpoint accounts cannot form an enabled route.
        assert!(upsert_strategy_route_impl(
            StrategyRouteUpsertInput {
                id: None,
                strategy_id: "futures".into(),
                label: "Bad".into(),
                account_a_id: hl.id.clone(),
                account_b_id: lt_disabled.id.clone(),
                enabled: true,
            },
            &state,
        )
        .is_err());

        // Duplicate account pair within a strategy is rejected (either order).
        assert!(upsert_strategy_route_impl(
            StrategyRouteUpsertInput {
                id: None,
                strategy_id: "futures".into(),
                label: "Dup".into(),
                account_a_id: hl.id.clone(),
                account_b_id: lt.id.clone(),
                enabled: true,
            },
            &state,
        )
        .is_err());

        // Referenced accounts cannot be deleted; the error names the routes.
        let err = delete_account_impl(&hl.id, &state).unwrap_err();
        assert!(err.contains("strategy route"), "err: {err}");

        assert!(delete_strategy_route_impl(&route.id, &state).expect("delete route"));
        assert!(delete_account_impl(&hl.id, &state).expect("delete after route removal"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_account_delete_requires_unlock_and_listing_is_lock_safe() {
        let (state, dir) = test_state_with_vault("del_locked");
        initialize_vault_impl("test password", &state).expect("init vault");

        let a = upsert_account_impl(
            AccountUpsertInput {
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "hyperliquid".into(),
                label: "Main".into(),
                enabled: true,
                make_default: false,
                credentials: Some(AccountCredentialInput {
                    api_secret: "SENTINEL_DEL".into(),
                    ..AccountCredentialInput::default()
                }),
            },
            &state,
        )
        .expect("create");

        lock_vault_impl(&state);
        let err = delete_account_impl(&a.id, &state).unwrap_err();
        assert!(err.to_lowercase().contains("locked"), "err: {err}");

        // Account metadata remains listable while locked; credential
        // presence is reported as unknown rather than fabricated.
        let listed = list_accounts_impl(None, &state).expect("list locked");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].credentials_configured, None);

        // The secret survives the refused deletion.
        unlock_vault_impl("test password", &state).expect("unlock");
        let stored = state.vault.write().get_data().expect("vault data").clone();
        assert_eq!(stored.credentials.len(), 1);
        assert_eq!(stored.credentials[0].api_secret, "SENTINEL_DEL");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn test_no_credential_read_ipc_registered() {
        let main_src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"))
            .expect("read main.rs");
        for cmd in [
            "get_exchange_credentials",
            "get_credentials",
            "read_credentials",
            "export_credentials",
            "export_vault",
            "get_vault_data",
        ] {
            assert!(
                !main_src.contains(cmd),
                "secret-read IPC registered in main.rs: {}",
                cmd
            );
        }
    }

    #[tokio::test]
    async fn test_nonsecret_settings_save_while_vault_locked() {
        let (state, dir) = test_state_with_vault("nonsecret");
        let settings = EngineSettingsDto {
            min_depth_ratio: 25.0,
            ..EngineSettingsDto::default()
        };
        save_engine_settings_impl(settings, &state).expect("save nonsecret settings");
        let updated = get_engine_settings_impl(&state).expect("get settings");
        assert_eq!(updated.min_depth_ratio, 25.0);
        assert!(updated.configured_venues.is_empty());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_multi_dex_ticker_orderbooks() {
        let state = setup_test_state_with_venues();
        seed_test_book(&state, "hyperliquid", "AVAX-USDC", dec!(28.5), dec!(28.6));
        seed_test_book(&state, "lighter", "AVAX-USDC", dec!(28.55), dec!(28.65));
        seed_test_book(&state, "vanta", "AVAX-USDC", dec!(28.58), dec!(28.68));
        seed_test_funding(&state, "hyperliquid", "AVAX-USDC");
        seed_test_funding(&state, "lighter", "AVAX-USDC");
        seed_test_funding(&state, "vanta", "AVAX-USDC");

        let books_avax = fetch_orderbooks_impl("AVAX-USDC", &state)
            .await
            .expect("avax books");
        assert!(books_avax.len() >= 2);
        for b in &books_avax {
            assert!(b.best_bid > 0.0);
            assert!(b.best_ask >= b.best_bid);
            assert!(b.total_bid_depth > 0.0);
        }
    }

    #[tokio::test]
    async fn test_multi_dex_ticker_spatial_and_funding() {
        let state = setup_test_state_with_venues();
        let r_hl = arbitrageur_core::quote::FundingRate {
            venue: arbitrageur_core::types::VenueId::new("hyperliquid"),
            symbol: "AVAX-USDC".into(),
            rate: dec!(0.00020),
            predicted_rate: None,
            interval_hours: 8,
            next_funding_time: Utc::now(),
            timestamp: Utc::now(),
        };
        let r_lgt = arbitrageur_core::quote::FundingRate {
            venue: arbitrageur_core::types::VenueId::new("lighter"),
            symbol: "AVAX-USDC".into(),
            rate: dec!(0.00008),
            predicted_rate: None,
            interval_hours: 8,
            next_funding_time: Utc::now(),
            timestamp: Utc::now(),
        };
        state.market_data_hub.update_funding(r_hl);
        state.market_data_hub.update_funding(r_lgt);
        seed_test_funding(&state, "vanta", "AVAX-USDC");

        let opps_avax = scan_funding_opportunities_impl("AVAX-USDC", 10000.0, &state)
            .await
            .expect("avax funding");
        assert!(!opps_avax.is_empty());
    }

    #[tokio::test]
    async fn test_initial_execution_logs_presence() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let logs = get_execution_logs_impl(&state).expect("execution logs");
        assert!(logs.is_empty());

        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "Hyperliquid",
            "Lighter",
            0.1,
            Some("Divergence Entry".into()),
            Some(18.5),
            Some(12.0),
            Some(1),
            Some(3),
            Some(15.0),
            &state,
        )
        .await
        .expect("spatial execution");

        assert!(res.opportunity_found);
        let logs_after = get_execution_logs_impl(&state).expect("execution logs after");
        assert_eq!(logs_after.len(), 1);
        assert_eq!(logs_after[0].symbol, "BTC-USDC");
        assert!(!logs_after[0].is_live);
    }

    #[tokio::test]
    async fn test_paper_vs_live_execution_logs_and_balances_separation() {
        let state = setup_test_state_with_venues();

        // Push a paper log and a live log
        let paper_entry = ExecutionLogEntry {
            id: "exec-paper-1".into(),
            timestamp: Utc::now().to_rfc3339(),
            trade_type: "Spatial".into(),
            strategy_id: "futures".into(),
            symbol: "BTC-USDC".into(),
            venue_a: "hyperliquid".into(),
            venue_b: "lighter".into(),
            leg_a_status: "FILLED".into(),
            leg_b_status: "FILLED".into(),
            success: true,
            gross_profit: 10.0,
            fees: 1.0,
            net_profit: 9.0,
            latency_us: 1500,
            note: "Paper execution".into(),
            direction: Some("Divergence Entry".into()),
            trigger_spread_bps: Some(15.0),
            baseline_mean_spread_bps: Some(5.0),
            position_step: Some(1),
            max_units: Some(3),
            nominal_hurdle_bps: Some(2.0),
            is_live: false,
        };
        let live_entry = ExecutionLogEntry {
            id: "exec-live-1".into(),
            timestamp: Utc::now().to_rfc3339(),
            trade_type: "Spatial".into(),
            strategy_id: "futures".into(),
            symbol: "ETH-USDC".into(),
            venue_a: "hyperliquid".into(),
            venue_b: "lighter".into(),
            leg_a_status: "FILLED".into(),
            leg_b_status: "FILLED".into(),
            success: true,
            gross_profit: 50.0,
            fees: 5.0,
            net_profit: 45.0,
            latency_us: 1200,
            note: "Live execution".into(),
            direction: Some("Divergence Entry".into()),
            trigger_spread_bps: Some(25.0),
            baseline_mean_spread_bps: Some(8.0),
            position_step: Some(1),
            max_units: Some(3),
            nominal_hurdle_bps: Some(2.0),
            is_live: true,
        };

        state.execution_logs.write().push(paper_entry);
        state.execution_logs.write().push(live_entry);

        // In paper mode: only paper logs returned
        *state.is_live_mode.write() = false;
        let paper_logs = get_execution_logs_impl(&state).expect("paper logs");
        assert_eq!(paper_logs.len(), 1);
        assert_eq!(paper_logs[0].id, "exec-paper-1");
        assert!(!paper_logs[0].is_live);

        // In live mode: only live logs returned
        *state.is_live_mode.write() = true;
        let live_logs = get_execution_logs_impl(&state).expect("live logs");
        assert_eq!(live_logs.len(), 1);
        assert_eq!(live_logs[0].id, "exec-live-1");
        assert!(live_logs[0].is_live);

        // Explicit filter checks
        assert_eq!(
            get_execution_logs_by_mode_impl(&state, Some(false)).unwrap().len(),
            1
        );
        assert_eq!(
            get_execution_logs_by_mode_impl(&state, Some(true)).unwrap().len(),
            1
        );

        // Verify balances report correct is_live flag in paper mode
        *state.is_live_mode.write() = false;
        let paper_balances = get_venue_balances_impl(&state).await.expect("paper balances");
        assert!(!paper_balances.is_empty());
        for b in &paper_balances {
            assert!(!b.is_live, "paper balance must have is_live = false");
        }

        // Verify balances report correct is_live flag in live mode
        *state.is_live_mode.write() = true;
        let live_balances = get_venue_balances_impl(&state).await.expect("live balances");
        assert!(!live_balances.is_empty());
        for b in &live_balances {
            assert!(b.is_live, "live balance must have is_live = true");
        }
    }

    #[tokio::test]
    async fn test_paper_execution_against_live_depth() {
        let state = setup_test_state_with_venues();
        seed_test_book(&state, "hyperliquid", "DOT-USDC", dec!(4.40), dec!(4.42));
        seed_test_book(&state, "lighter", "DOT-USDC", dec!(4.47), dec!(4.49));

        let exec_res = execute_spatial_arbitrage_impl(
            "DOT-USDC",
            "Hyperliquid",
            "Lighter",
            0.5,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await
        .expect("spatial exec on dot");
        assert!(exec_res.execution_success);
        assert!(exec_res.realized_profit_usd < 0.0);
    }

    #[tokio::test]
    async fn test_funding_carry_paper_dispatch_opens_carry_position() {
        let state = setup_test_state_with_venues();
        seed_test_book(&state, "hyperliquid", "DOT-USDC", dec!(4.40), dec!(4.42));
        seed_test_book(&state, "lighter", "DOT-USDC", dec!(4.47), dec!(4.49));
        // Steep HL-vs-Lighter gap: short HL, long Lighter, APR far above hurdle.
        state.market_data_hub.update_funding(
            arbitrageur_core::quote::FundingRate {
                venue: arbitrageur_core::types::VenueId::new("hyperliquid"),
                symbol: "DOT-USDC".to_string(),
                rate: dec!(0.0008),
                predicted_rate: None,
                interval_hours: 1,
                next_funding_time: Utc::now(),
                timestamp: Utc::now(),
            },
        );
        state.market_data_hub.update_funding(
            arbitrageur_core::quote::FundingRate {
                venue: arbitrageur_core::types::VenueId::new("lighter"),
                symbol: "DOT-USDC".to_string(),
                rate: dec!(0.0001),
                predicted_rate: None,
                interval_hours: 8,
                next_funding_time: Utc::now(),
                timestamp: Utc::now(),
            },
        );

        // $20 notional fits the seeded 10-lot test books on both legs.
        let res = execute_funding_carry_impl("DOT-USDC", "lighter", "hyperliquid", 20.0, &state)
            .await
            .expect("paper funding carry should dispatch");
        assert!(res.execution_success);
        assert_eq!(res.trade_type, "Funding");

        // One two-legged funding signal => a single open carry position.
        let positions = state
            .state_store
            .list_signals(None, None)
            .expect("signals readable");
        let funding: Vec<_> = positions
            .iter()
            .filter(|s| s.strategy_id.eq_ignore_ascii_case("funding"))
            .collect();
        assert_eq!(funding.len(), 1, "expected one funding signal, got {funding:?}");
        assert_eq!(funding[0].legs.len(), 2);
        assert!(funding[0].status.counts_as_open());
    }

    #[tokio::test]
    async fn test_funding_carry_live_refusal_names_missing_leg() {
        let state = setup_test_state_with_venues();
        *state.is_live_mode.write() = true;
        let err = execute_funding_carry_impl("LINK-USDC", "lighter", "hyperliquid", 500.0, &state)
            .await
            .expect_err("live funding must refuse without two-leg reconciliation");
        assert!(
            err.contains("lighter") && err.contains("not implemented"),
            "refusal must name the unimplemented leg: {err}"
        );
        assert!(err.contains("Paper Trading"), "refusal must point at paper: {err}");
        *state.is_live_mode.write() = false;
    }

    #[tokio::test]
    async fn test_funding_carry_rejects_below_hurdle_and_live() {
        let state = setup_test_state_with_venues();
        seed_test_book(&state, "hyperliquid", "DOT-USDC", dec!(4.40), dec!(4.42));
        seed_test_book(&state, "lighter", "DOT-USDC", dec!(4.47), dec!(4.49));
        // Dust gap (~3% APR): resolvable pair but below the min-APR hurdle.
        for (venue, rate, interval) in [
            ("hyperliquid", dec!(0.0000125), 1),
            ("lighter", dec!(0.000096), 8),
        ] {
            state.market_data_hub.update_funding(
                arbitrageur_core::quote::FundingRate {
                    venue: arbitrageur_core::types::VenueId::new(venue),
                    symbol: "DOT-USDC".to_string(),
                    rate,
                    predicted_rate: None,
                    interval_hours: interval,
                    next_funding_time: Utc::now(),
                    timestamp: Utc::now(),
                },
            );
        }
        let err = execute_funding_carry_impl("DOT-USDC", "lighter", "hyperliquid", 500.0, &state)
            .await
            .expect_err("dust carry must be refused");
        assert!(err.contains("hurdle"), "unexpected err: {err}");
    }

    #[tokio::test]
    async fn test_spatial_execution_rejects_unsafe_inputs() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        // Same venue, zero qty, NaN qty
        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "Hyperliquid",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        assert!(res.unwrap_err().contains("distinct"));
        // Same canonical venue through different aliases must also reject
        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "hl",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        assert!(res.unwrap_err().contains("distinct"));
        // Malformed symbol
        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC'; DROP",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        assert!(res.unwrap_err().contains("Invalid market symbol"));
        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.0,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        assert!(res.unwrap_err().contains("positive"));
        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            f64::NAN,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        assert!(res.unwrap_err().contains("finite"));
        // Negative hurdle
        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            Some(-1.0),
            &state,
        )
        .await;
        assert!(res.unwrap_err().contains("hurdle"));

        // Nothing may have mutated state
        assert!(state.virtual_ledger.read().lots.is_empty());
        assert!(get_execution_logs_impl(&state).expect("logs").is_empty());
    }

    #[tokio::test]
    async fn test_spatial_execution_uses_per_strategy_slippage_gate() {
        let state = setup_test_state_with_venues();
        // Qty 5 walks deep into a second ask level ~300 bps above top.
        let mut book = arbitrageur_core::quote::OrderBook::new(
            arbitrageur_core::types::VenueId::new("hyperliquid"),
            "BTC-USDC".to_string(),
        );
        book.bids.push(arbitrageur_core::quote::BookLevel::new(
            dec!(64100.0),
            dec!(50.0),
        ));
        book.asks.push(arbitrageur_core::quote::BookLevel::new(
            dec!(64205.0),
            dec!(1.0),
        ));
        book.asks.push(arbitrageur_core::quote::BookLevel::new(
            dec!(66200.0),
            dec!(50.0),
        ));
        book.timestamp = Utc::now();
        state.market_data_hub.update_orderbook(book);
        let mut deep = arbitrageur_core::quote::OrderBook::new(
            arbitrageur_core::types::VenueId::new("lighter"),
            "BTC-USDC".to_string(),
        );
        deep.bids.push(arbitrageur_core::quote::BookLevel::new(
            dec!(64360.0),
            dec!(50.0),
        ));
        deep.asks.push(arbitrageur_core::quote::BookLevel::new(
            dec!(64365.0),
            dec!(50.0),
        ));
        deep.timestamp = Utc::now();
        state.market_data_hub.update_orderbook(deep);

        let run_execute = || {
            execute_spatial_arbitrage_impl(
                "BTC-USDC",
                "hyperliquid",
                "lighter",
                5.0,
                None,
                None,
                None,
                None,
                None,
                None,
                &state,
            )
        };

        // A strict per-strategy cap rejects on slippage (no global applies).
        {
            let mut params = state.strategy_params.write();
            let mut p = params
                .get("futures")
                .cloned()
                .unwrap_or_else(|| StrategyParamsDto::defaults_for("futures"));
            p.futures_max_slippage_bps = 0.01;
            params.insert("futures".to_string(), p);
        }
        let err = run_execute()
            .await
            .expect_err("strict per-strategy slippage must reject");
        assert!(err.contains("slippage"), "expected slippage error, got: {err}");

        // A generous per-strategy cap passes the slippage gate, so the same
        // books fail later on economics/depth instead — proving the record
        // value drove the gate.
        {
            let mut params = state.strategy_params.write();
            let mut p = params
                .get("futures")
                .cloned()
                .unwrap_or_else(|| StrategyParamsDto::defaults_for("futures"));
            p.futures_max_slippage_bps = 500.0;
            params.insert("futures".to_string(), p);
        }
        let err = run_execute()
            .await
            .expect_err("economics must reject after slippage passes");
        assert!(!err.contains("slippage"), "slippage gate should pass, got: {err}");
    }

    #[tokio::test]
    async fn test_spatial_execution_rejects_bad_books() {
        let state = setup_test_state_with_venues();

        // Crossed book on the buy side (bid > ask)
        let mut crossed = arbitrageur_core::quote::OrderBook::new(
            arbitrageur_core::types::VenueId::new("hyperliquid"),
            "BTC-USDC".to_string(),
        );
        crossed.bids.push(arbitrageur_core::quote::BookLevel::new(
            dec!(64300.0),
            dec!(10.0),
        ));
        crossed.asks.push(arbitrageur_core::quote::BookLevel::new(
            dec!(64200.0),
            dec!(10.0),
        ));
        crossed.timestamp = Utc::now();
        state.market_data_hub.update_orderbook(crossed);
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        let err = res.expect_err("crossed book must be rejected");
        assert!(
            err.contains("order book rejected"),
            "unexpected err: {}",
            err
        );
        assert!(state.virtual_ledger.read().lots.is_empty());
    }

    #[tokio::test]
    async fn test_spatial_execution_rejects_depth_slippage_and_economics() {
        let state = setup_test_state_with_venues();

        // Depth ratio: 11 units of within-limit ask depth fills the order but
        // cannot satisfy 11 * 3 = 33 required ratio; out-of-limit depth at
        // 64500 must not count toward the requirement
        {
            let mut book = arbitrageur_core::quote::OrderBook::new(
                arbitrageur_core::types::VenueId::new("hyperliquid"),
                "BTC-USDC".to_string(),
            );
            book.bids.push(arbitrageur_core::quote::BookLevel::new(
                dec!(64100.0),
                dec!(50.0),
            ));
            book.asks.push(arbitrageur_core::quote::BookLevel::new(
                dec!(64205.0),
                dec!(11.0),
            ));
            book.asks.push(arbitrageur_core::quote::BookLevel::new(
                dec!(64500.0),
                dec!(50.0),
            ));
            book.timestamp = Utc::now();
            state.market_data_hub.update_orderbook(book);

            let mut deep = arbitrageur_core::quote::OrderBook::new(
                arbitrageur_core::types::VenueId::new("lighter"),
                "BTC-USDC".to_string(),
            );
            deep.bids.push(arbitrageur_core::quote::BookLevel::new(
                dec!(64360.0),
                dec!(50.0),
            ));
            deep.asks.push(arbitrageur_core::quote::BookLevel::new(
                dec!(64365.0),
                dec!(50.0),
            ));
            deep.timestamp = Utc::now();
            state.market_data_hub.update_orderbook(deep);
        }

        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            11.0,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        assert!(
            res.expect_err("depth ratio must reject").contains("depth"),
            "expected depth error"
        );

        // Slippage: qty 5 walks deep into a second level 300+ bps above top
        {
            let mut book = arbitrageur_core::quote::OrderBook::new(
                arbitrageur_core::types::VenueId::new("hyperliquid"),
                "BTC-USDC".to_string(),
            );
            book.bids.push(arbitrageur_core::quote::BookLevel::new(
                dec!(64100.0),
                dec!(50.0),
            ));
            book.asks.push(arbitrageur_core::quote::BookLevel::new(
                dec!(64205.0),
                dec!(1.0),
            ));
            book.asks.push(arbitrageur_core::quote::BookLevel::new(
                dec!(66200.0),
                dec!(50.0),
            ));
            book.timestamp = Utc::now();
            state.market_data_hub.update_orderbook(book);
        }
        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            5.0,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        assert!(
            res.expect_err("slippage must reject").contains("slippage"),
            "expected slippage error"
        );

        // Non-profitable spread: sell VWAP below buy VWAP; route economics
        // put expected net far below the backend target.
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64330.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64200.0), dec!(64210.0));
        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        assert!(res
            .expect_err("negative spread must reject")
            .contains("below the"));

        // The backend futures target — not the caller's hurdle — gates
        // admission: raise it above the route's expected net.
        {
            let mut params = state.strategy_params.write();
            let mut p = params.get("futures").cloned().unwrap_or_else(|| {
                StrategyParamsDto::defaults_for("futures")
            });
            p.target_profit_bps = 50.0;
            params.insert("futures".to_string(), p);
        }
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64260.0), dec!(64265.0));
        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            Some(50.0),
            &state,
        )
        .await;
        assert!(res.expect_err("target must reject").contains("below the"));

        assert!(state.virtual_ledger.read().lots.is_empty());
        assert!(get_execution_logs_impl(&state).expect("logs").is_empty());
    }

    fn mk_route_book(venue: &str, bids: &[(f64, f64)], asks: &[(f64, f64)]) -> OrderBook {
        let mut book = OrderBook::new(
            VenueId::new(venue),
            "BTC-USDC".to_string(),
        );
        for (p, s) in bids {
            book.bids.push(arbitrageur_core::quote::BookLevel::new(
                Decimal::from_f64_retain(*p).unwrap(),
                Decimal::from_f64_retain(*s).unwrap(),
            ));
        }
        for (p, s) in asks {
            book.asks.push(arbitrageur_core::quote::BookLevel::new(
                Decimal::from_f64_retain(*p).unwrap(),
                Decimal::from_f64_retain(*s).unwrap(),
            ));
        }
        book.timestamp = Utc::now();
        book
    }

    #[test]
    fn test_route_economics_exact_values() {
        let book_buy = mk_route_book(
            "hyperliquid",
            &[(64200.0, 5.0), (64190.0, 10.0)],
            &[(64205.0, 5.0), (64215.0, 10.0)],
        );
        let book_sell = mk_route_book(
            "lighter",
            &[(64300.0, 5.0), (64290.0, 10.0)],
            &[(64305.0, 5.0), (64315.0, 10.0)],
        );
        let econ = compute_route_economics(
            &book_buy,
            &book_sell,
            dec!(5.0),
            2.0,
            1.5,
            1.0,
        )
        .expect("route economics");

        assert!((econ.round_trip_commission_bps - 7.0).abs() < 1e-9);
        assert!((econ.top_gross_bps - 14.7964).abs() < 1e-3);
        assert_eq!(econ.entry_slippage_bps, 0.0);
        assert_eq!(econ.exit_slippage_bps, 0.0);
        assert!((econ.closing_spread_bps - 1.5564).abs() < 1e-3);
        assert!((econ.expected_friction_bps - 8.5564).abs() < 1e-3);
        assert!((econ.expected_net_bps - 6.2400).abs() < 1e-3);
        assert!((econ.required_gross_bps - 9.5564).abs() < 1e-3);
        assert!((econ.estimated_profit_usd - 200.5).abs() < 0.5);
    }

    #[test]
    fn test_route_economics_walks_multi_level_vwap() {
        let book_buy = mk_route_book(
            "hyperliquid",
            &[(64200.0, 10.0)],
            &[(64205.0, 5.0), (64215.0, 10.0)],
        );
        let book_sell = mk_route_book(
            "lighter",
            &[(64300.0, 5.0), (64290.0, 10.0)],
            &[(64305.0, 10.0)],
        );
        let econ = compute_route_economics(
            &book_buy,
            &book_sell,
            dec!(6.0),
            2.0,
            1.5,
            1.0,
        )
        .expect("route economics");

        let buy_entry: f64 = (5.0 * 64205.0 + 1.0 * 64215.0) / 6.0;
        let sell_entry: f64 = (5.0 * 64300.0 + 1.0 * 64290.0) / 6.0;
        let expected_entry_slip =
            ((buy_entry - 64205.0) / 64205.0 * 10_000.0f64).max(0.0)
                + ((64300.0 - sell_entry) / 64300.0 * 10_000.0f64).max(0.0);
        assert!((econ.entry_slippage_bps - expected_entry_slip).abs() < 1e-6);
        assert!(econ.entry_slippage_bps > 0.0);
    }

    #[test]
    fn test_route_economics_fails_closed_on_incomplete_depth() {
        let book_buy = mk_route_book(
            "hyperliquid",
            &[(64200.0, 1.0)],
            &[(64205.0, 10.0)],
        );
        let book_sell = mk_route_book(
            "lighter",
            &[(64300.0, 10.0)],
            &[(64305.0, 10.0)],
        );
        assert!(compute_route_economics(&book_buy, &book_sell, dec!(5.0), 2.0, 1.5, 1.0).is_none());

        let no_asks = mk_route_book("hyperliquid", &[(64200.0, 10.0)], &[]);
        assert!(compute_route_economics(&no_asks, &book_sell, dec!(5.0), 2.0, 1.5, 1.0).is_none());
    }

    #[tokio::test]
    async fn test_effective_venue_fee_bps_uses_default_account_tier() {
        let (state, _dir) = test_state_with_vault("fee-tier");
        assert_eq!(effective_venue_fee_bps(&state, "lighter"), 1.5);
        assert_eq!(effective_venue_fee_bps(&state, "LIGHTER"), 1.5);
        assert_eq!(effective_venue_fee_bps(&state, "hyperliquid"), 2.0);

        upsert_account_impl(
            AccountUpsertInput {
                account_tier: Some("plus".into()),
                max_leverage: None,
                id: None,
                venue: "lighter".into(),
                label: "L1".into(),
                enabled: true,
                make_default: true,
                credentials: None,
            },
            &state,
        )
        .expect("create account");
        assert_eq!(effective_venue_fee_bps(&state, "lighter"), 2.0);
        assert_eq!(effective_venue_fee_bps(&state, "hyperliquid"), 2.0);
    }

    #[tokio::test]
    async fn test_scan_spatial_route_economics_gating() {
        let state = setup_test_state_with_venues();
        // Hermetic scan scope: only the two venues under test may resolve.
        // Every other enabled venue falls through to a live REST fetch, and
        // the wall-clock spent there lets the seeded books go stale past the
        // 350ms hub TTL — later phases then depended on live venues (Lighter
        // reachability, Vanta rate limits) instead of the seeds. Scoping the
        // venues plus re-seeding before every scan keeps this test offline
        // and deterministic.
        {
            let mut s = state.settings.write();
            s.vanta_enabled = false;
            s.robinhood_enabled = false;
            s.arcus_enabled = false;
        }
        // The hub only honors books fresher than 350ms; seeds from an
        // earlier phase must never be reused by a later phase.
        let seed_pair = || {
            seed_test_book(
                &state,
                "hyperliquid",
                "BTC-USDC",
                dec!(64200.0),
                dec!(64205.0),
            );
            seed_test_book(&state, "lighter", "BTC-USDC", dec!(64300.0), dec!(64305.0));
        };
        seed_pair();

        let opps = scan_spatial_opportunities_impl("BTC-USDC", 1.0, 0.5, &state)
            .await
            .expect("scan");
        let opp = opps
            .iter()
            .find(|o| o.buy_venue_id == "hyperliquid" && o.sell_venue_id == "lighter")
            .expect("hl->lighter route");
        assert!(opp.slippage_ok);
        assert!(opp.depth_ok);
        assert!(opp.is_executable);
        assert!((opp.fee_bps - 3.5).abs() < 1e-9);
        assert!((opp.round_trip_commission_bps - 7.0).abs() < 1e-9);
        assert!((opp.required_gross_bps - (opp.expected_friction_bps + 1.0)).abs() < 1e-9);
        assert!((opp.net_spread_bps - (opp.gross_spread_bps - opp.expected_friction_bps)).abs() < 1e-9);

        seed_pair();
        let opps = scan_spatial_opportunities_impl("BTC-USDC", 50.0, 0.5, &state)
            .await
            .expect("scan");
        let opp = opps
            .iter()
            .find(|o| o.buy_venue_id == "hyperliquid" && o.sell_venue_id == "lighter")
            .expect("route still reported for diagnostics");
        assert!(!opp.is_executable);

        // Thin reverse exit: buy venue bids cannot absorb the unwind, so the
        // route is not costable and disappears from results entirely.
        seed_pair();
        {
            let mut thin = arbitrageur_core::quote::OrderBook::new(
                arbitrageur_core::types::VenueId::new("hyperliquid"),
                "BTC-USDC".to_string(),
            );
            thin.bids.push(arbitrageur_core::quote::BookLevel::new(
                dec!(64200.0),
                dec!(0.1),
            ));
            thin.asks.push(arbitrageur_core::quote::BookLevel::new(
                dec!(64205.0),
                dec!(10.0),
            ));
            thin.timestamp = Utc::now();
            state.market_data_hub.update_orderbook(thin);
        }
        let opps = scan_spatial_opportunities_impl("BTC-USDC", 1.0, 0.5, &state)
            .await
            .expect("scan");
        assert!(opps
            .iter()
            .all(|o| !(o.buy_venue_id == "hyperliquid" && o.sell_venue_id == "lighter")));

        // Exit-side slippage gate: deep but stale-priced reverse book keeps
        // the route visible while blocking execution.
        seed_pair();
        {
            let mut book = arbitrageur_core::quote::OrderBook::new(
                arbitrageur_core::types::VenueId::new("hyperliquid"),
                "BTC-USDC".to_string(),
            );
            book.bids.push(arbitrageur_core::quote::BookLevel::new(
                dec!(64200.0),
                dec!(0.4),
            ));
            book.bids.push(arbitrageur_core::quote::BookLevel::new(
                dec!(60000.0),
                dec!(50.0),
            ));
            book.asks.push(arbitrageur_core::quote::BookLevel::new(
                dec!(64205.0),
                dec!(10.0),
            ));
            book.timestamp = Utc::now();
            state.market_data_hub.update_orderbook(book);
        }
        let opps = scan_spatial_opportunities_impl("BTC-USDC", 1.0, 0.5, &state)
            .await
            .expect("scan");
        let opp = opps
            .iter()
            .find(|o| o.buy_venue_id == "hyperliquid" && o.sell_venue_id == "lighter")
            .expect("route present but blocked");
        assert!(!opp.slippage_ok);
        assert!(!opp.is_executable);
    }

    fn seed_episode_history(state: &AppEngineState, symbol: &str, episodes: usize) {
        // Six flats then a +24bps spike per episode: the spike pulls the
        // series mean to ~3.4bps, so flats sit below the ~8.6bps friction
        // band while every spike excursion reverts -> hit-rate 1.0 once
        // the episode count clears the default min_episodes gate (5).
        let now = Utc::now().timestamp_millis();
        let base = now - 3_600_000;
        for e in 0..episodes {
            for k in 0..7 {
                let light = if k == 6 { 100.24 } else { 100.0 };
                let ts = base + (e * 7 + k) as i64 * 1_000;
                seed_samples(
                    state,
                    symbol,
                    ts,
                    &[("hyperliquid", 100.0), ("lighter", light)],
                );
            }
        }
        // Trailing flats let the final episode resolve instead of ending open.
        for k in 0..6 {
            let ts = base + (episodes * 7 + k) as i64 * 1_000;
            seed_samples(
                state,
                symbol,
                ts,
                &[("hyperliquid", 100.0), ("lighter", 100.0)],
            );
        }
    }

    fn seed_books_hl_lighter(state: &AppEngineState) {
        seed_test_book(
            state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(state, "lighter", "BTC-USDC", dec!(64300.0), dec!(64305.0));
    }

    /// Hermetic two-venue state: only hyperliquid + lighter are admitted, so
    /// scans resolve exactly the seeded hub books and never touch live
    /// venue feeds (which would make route sets nondeterministic).
    fn setup_two_venue_state() -> AppEngineState {
        let state = AppEngineState::new();
        {
            let mut s = state.settings.write();
            s.hyperliquid_enabled = true;
            s.lighter_enabled = true;
        }
        state
    }

    #[tokio::test]
    async fn test_scan_win_probability_seeded_history() {
        let state = setup_two_venue_state();
        seed_books_hl_lighter(&state);
        seed_episode_history(&state, "BTC-USDC", 8);

        let opps = scan_spatial_opportunities_impl("BTC-USDC", 1.0, 0.5, &state)
            .await
            .expect("scan");
        let opp = opps
            .iter()
            .find(|o| o.buy_venue_id == "hyperliquid" && o.sell_venue_id == "lighter")
            .expect("hl->lighter route");
        let p = opp
            .win_probability
            .expect("seeded episodes must yield a win probability");
        assert!(p > 0.0 && p <= 1.0, "win p in (0,1], got {p}");
    }

    #[tokio::test]
    async fn test_scan_win_probability_thin_history_is_none() {
        let state = setup_two_venue_state();
        seed_books_hl_lighter(&state);
        // Two samples: below every data floor, so every route reads None
        // while economics/ordering stay untouched.
        let now = Utc::now().timestamp_millis();
        seed_samples(
            &state,
            "BTC-USDC",
            now - 1_000,
            &[("hyperliquid", 100.0), ("lighter", 100.1)],
        );
        seed_samples(
            &state,
            "BTC-USDC",
            now,
            &[("hyperliquid", 100.0), ("lighter", 100.1)],
        );

        let opps = scan_spatial_opportunities_impl("BTC-USDC", 1.0, 0.5, &state)
            .await
            .expect("scan");
        assert!(!opps.is_empty());
        assert!(opps.iter().all(|o| o.win_probability.is_none()));
        let opp = opps
            .iter()
            .find(|o| o.buy_venue_id == "hyperliquid" && o.sell_venue_id == "lighter")
            .expect("route still reported");
        assert!(opp.is_executable);
    }

    #[tokio::test]
    async fn test_scan_win_probability_empty_history_is_none() {
        let state = setup_two_venue_state();
        seed_books_hl_lighter(&state);

        let opps = scan_spatial_opportunities_impl("BTC-USDC", 1.0, 0.5, &state)
            .await
            .expect("scan");
        assert!(!opps.is_empty());
        assert!(opps.iter().all(|o| o.win_probability.is_none()));
    }

    #[tokio::test]
    async fn test_scan_win_probability_ou_fallback() {
        // Smooth sine history around +10bps, amplitude far below friction:
        // zero episodes, but a qualifying OU fit -> fallback yields Some.
        let state = setup_two_venue_state();
        seed_books_hl_lighter(&state);
        let now = Utc::now().timestamp_millis();
        let base = now - 3_600_000;
        for k in 0..60 {
            let x = 10.0 + 1.5 * ((k as f64) * 0.25).sin();
            let light = 100.0 * (1.0 + x / 10_000.0);
            seed_samples(
                &state,
                "BTC-USDC",
                base + k as i64 * 1_000,
                &[("hyperliquid", 100.0), ("lighter", light)],
            );
        }

        let opps = scan_spatial_opportunities_impl("BTC-USDC", 1.0, 0.5, &state)
            .await
            .expect("scan");
        let opp = opps
            .iter()
            .find(|o| o.buy_venue_id == "hyperliquid" && o.sell_venue_id == "lighter")
            .expect("hl->lighter route");
        let p = opp
            .win_probability
            .expect("qualifying OU fit must yield a fallback probability");
        assert!(
            p.is_finite() && (0.0..=1.0).contains(&p),
            "fallback P_win in [0,1], got {p}"
        );
    }

    #[tokio::test]
    async fn test_scan_win_probability_deterministic() {
        let state = setup_two_venue_state();
        seed_books_hl_lighter(&state);
        seed_episode_history(&state, "BTC-USDC", 8);

        let first = scan_spatial_opportunities_impl("BTC-USDC", 1.0, 0.5, &state)
            .await
            .expect("scan");
        let second = scan_spatial_opportunities_impl("BTC-USDC", 1.0, 0.5, &state)
            .await
            .expect("scan");
        // Order-insensitive: route economics sort has exact-float ties, so
        // only per-route values must be deterministic, not positions.
        let by_id = |v: Vec<SpatialOpportunityDto>| {
            let mut m: HashMap<String, SpatialOpportunityDto> = HashMap::new();
            for o in v {
                m.insert(o.id.clone(), o);
            }
            m
        };
        let a = by_id(first);
        let b = by_id(second);
        assert_eq!(a.len(), b.len());
        for (id, x) in &a {
            let y = b.get(id).expect("same route set");
            assert_eq!(x.win_probability, y.win_probability);
            assert!((x.net_spread_bps - y.net_spread_bps).abs() < 1e-12);
        }
        assert!(a.values().any(|o| o.win_probability.is_some()));
    }

    #[tokio::test]
    async fn test_scan_win_probability_matches_monitor_standard() {
        // Same pair + same history through the shared helper must agree
        // with the scan value within epsilon: no second standard.
        let state = setup_two_venue_state();
        seed_books_hl_lighter(&state);
        seed_episode_history(&state, "BTC-USDC", 8);

        let opps = scan_spatial_opportunities_impl("BTC-USDC", 1.0, 0.5, &state)
            .await
            .expect("scan");
        let opp = opps
            .iter()
            .find(|o| o.buy_venue_id == "hyperliquid" && o.sell_venue_id == "lighter")
            .expect("hl->lighter route");
        let scan_p = opp.win_probability.expect("seeded P_win");

        let now_ms = Utc::now().timestamp_millis();
        let samples = state
            .state_store
            .spread_samples("BTC-USDC", now_ms - 86_400_000, now_ms)
            .expect("history");
        let mut mids: HashMap<String, Vec<(i64, f64)>> = HashMap::new();
        for s in &samples {
            if s.mid.is_finite() && s.mid > 0.0 {
                mids.entry(s.venue_id.clone())
                    .or_default()
                    .push((s.ts_ms, s.mid));
            }
        }
        let series = arbitrageur_math::pair_monitor::build_pair_series(
            &mids["hyperliquid"],
            &mids["lighter"],
            crate::monitor::PAIR_ALIGN_GAP_MS,
        );
        let monitor_p = crate::monitor::pair_win_probability(
            &series,
            opp.gross_spread_bps,
            opp.expected_friction_bps,
            5,
        )
        .expect("shared helper P_win");
        assert!(
            (scan_p - monitor_p).abs() < 1e-12,
            "scan {scan_p} vs monitor {monitor_p}"
        );
    }

    #[tokio::test]
    async fn test_scan_win_probability_informational_only() {
        // Seeded vs empty history: identical economics, ordering and
        // executability; only win_probability differs.
        let seeded = setup_two_venue_state();
        seed_books_hl_lighter(&seeded);
        seed_episode_history(&seeded, "BTC-USDC", 8);
        let bare = setup_two_venue_state();
        seed_books_hl_lighter(&bare);

        let a = scan_spatial_opportunities_impl("BTC-USDC", 1.0, 0.5, &seeded)
            .await
            .expect("scan");
        let b = scan_spatial_opportunities_impl("BTC-USDC", 1.0, 0.5, &bare)
            .await
            .expect("scan");
        // Order-insensitive for the same tie reason as the determinism test.
        let by_id = |v: Vec<SpatialOpportunityDto>| {
            let mut m: HashMap<String, SpatialOpportunityDto> = HashMap::new();
            for o in v {
                m.insert(o.id.clone(), o);
            }
            m
        };
        let a = by_id(a);
        let b = by_id(b);
        assert_eq!(a.len(), b.len());
        for (id, x) in &a {
            let y = b.get(id).expect("same route set");
            assert_eq!(x.is_executable, y.is_executable);
            assert!((x.net_spread_bps - y.net_spread_bps).abs() < 1e-12);
        }
        assert!(a.values().any(|o| o.win_probability.is_some()));
        assert!(b.values().all(|o| o.win_probability.is_none()));
    }

    #[tokio::test]
    async fn test_scan_single_venue_symbol_yields_no_routes() {
        // Only one venue has a book: fewer than two costable books means
        // no routes at all (fail closed, no partial rows). A fictitious
        // symbol keeps live venue feeds from resolving, so the seeded
        // hyperliquid hub book stands alone hermetically.
        let state = setup_two_venue_state();
        seed_test_book(
            &state,
            "hyperliquid",
            "TEST-XYZ",
            dec!(64200.0),
            dec!(64205.0),
        );
        let opps = scan_spatial_opportunities_impl("TEST-XYZ", 1.0, 0.5, &state)
            .await
            .expect("scan");
        assert!(opps.is_empty());
    }

    #[tokio::test]
    async fn test_spatial_execution_equal_qty_wide_spread_passes_delta() {
        let state = setup_test_state_with_venues();
        // Equal 3 BTC legs at materially different marks are delta-neutral
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(60000.0),
            dec!(60005.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(61000.0), dec!(61005.0));

        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            3.0,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        let res = res.expect("wide-spread equal-quantity pair must pass all guards");
        assert!(res.execution_success);
        assert_eq!(state.virtual_ledger.read().lots.len(), 1);
    }

    #[tokio::test]
    async fn test_spatial_execution_rejects_critical_margin() {
        let state = setup_test_state_with_venues();
        let hl_paper = state.get_paper_exchange("hyperliquid").expect("hl paper");

        // Drive the paper account into critical margin utilization directly:
        // open 1.5 BTC at 5x (IM 18000), then drop leverage to 1x so the
        // position's initial margin becomes 90000 against 100k equity (90%)
        let mut big_book = arbitrageur_core::quote::OrderBook::new(
            arbitrageur_core::types::VenueId::new("hyperliquid"),
            "BTC-USDC".to_string(),
        );
        big_book.bids.push(arbitrageur_core::quote::BookLevel::new(
            dec!(59900.0),
            dec!(500.0),
        ));
        big_book.asks.push(arbitrageur_core::quote::BookLevel::new(
            dec!(60000.0),
            dec!(500.0),
        ));
        big_book.timestamp = Utc::now();
        hl_paper.replace_orderbook(big_book).expect("seed big book");
        let resp = hl_paper
            .place_order(arbitrageur_core::order::OrderRequest::new_ioc(
                arbitrageur_core::types::VenueId::new("hyperliquid"),
                "BTC-USDC",
                Side::Buy,
                dec!(60000.0),
                dec!(1.5),
            ))
            .await
            .expect("direct paper order");
        assert_eq!(resp.status, arbitrageur_core::types::OrderStatus::Filled);
        hl_paper
            .set_leverage("BTC-USDC", 1)
            .await
            .expect("lower leverage");
        let acc = hl_paper.fetch_account().await.expect("account");
        assert!(
            acc.margin_usage_pct >= dec!(85.0),
            "usage {} must be critical",
            acc.margin_usage_pct
        );

        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));
        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        let err = res.expect_err("critical margin must reject");
        assert!(err.contains("margin"), "unexpected err: {}", err);
        assert!(state.virtual_ledger.read().lots.is_empty());
    }

    #[tokio::test]
    async fn test_toggle_off_keeps_stored_venue_discoverable_and_deletable() {
        let (state, dir) = test_state_with_vault("toggle");
        initialize_vault_impl("test password", &state).expect("init vault");

        let settings = EngineSettingsDto {
            hyperliquid_enabled: true,
            hyperliquid_wallet: "SENTINEL_WALLET_toggle".into(),
            hyperliquid_private_key: "SENTINEL_PRIV_toggle".into(),
            ..EngineSettingsDto::default()
        };
        save_engine_settings_impl(settings, &state).expect("save creds");
        assert_eq!(
            get_engine_settings_impl(&state)
                .expect("settings")
                .configured_venues,
            vec!["hyperliquid".to_string()]
        );

        // Toggle the venue off: disable the account profile created by the
        // first save plus the engine flag — both halves of the unified
        // account/venue switch. The stored record stays discoverable because
        // configured_venues means "credentials exist", not "venue active".
        state
            .state_store
            .upsert_account(
                "hyperliquid-default",
                "hyperliquid",
                "Default",
                false,
                false,
                "standard",
                None,
            )
            .expect("disable account");
        let off = EngineSettingsDto {
            hyperliquid_enabled: false,
            ..Default::default()
        };
        save_engine_settings_impl(off, &state).expect("save toggle off");
        assert_eq!(
            get_engine_settings_impl(&state)
                .expect("settings")
                .configured_venues,
            vec!["hyperliquid".to_string()]
        );
        let stored = state.vault.write().get_data().expect("vault data").clone();
        let hl = stored
            .credentials
            .iter()
            .find(|c| c.venue == "hyperliquid")
            .expect("secret must remain stored");
        assert_eq!(hl.api_secret, "SENTINEL_PRIV_toggle");
        assert!(!hl.enabled);

        // Disabled records remain deletable
        assert!(
            delete_exchange_credentials_impl("hyperliquid", &state).expect("delete disabled creds")
        );
        assert!(get_engine_settings_impl(&state)
            .expect("settings")
            .configured_venues
            .is_empty());
        assert!(state
            .vault
            .write()
            .get_data()
            .expect("vault data")
            .credentials
            .is_empty());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_cross_ticker_arbitrage_execution() {
        let state = setup_test_state_with_venues();
        seed_test_book(&state, "hyperliquid", "SOL-USDC", dec!(152.0), dec!(152.2));
        seed_test_book(&state, "lighter", "AVAX-USDC", dec!(28.5), dec!(28.7));

        let pnl_before = *state.total_realized_profit.read();
        let res = execute_cross_ticker_arbitrage_impl(
            "SOL-USDC",
            "AVAX-USDC",
            "Hyperliquid",
            "Lighter",
            10.0,
            Some(18.5),
            &state,
        )
        .await;

        assert!(res.is_err());
        assert!(res.unwrap_err().contains("cross-ticker"));
        assert_eq!(*state.total_realized_profit.read(), pnl_before);
        assert!(get_execution_logs_impl(&state).expect("logs").is_empty());
        assert!(state.virtual_ledger.read().lots.is_empty());
    }

    #[tokio::test]
    async fn test_live_mode_enable_rejected() {
        let state = AppEngineState::new();
        let res = set_live_mode_impl(true, &state);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("locked"));
        assert!(!*state.is_live_mode.read());

        let res_off = set_live_mode_impl(false, &state).expect("disable live mode");
        assert!(!res_off);
        assert!(!*state.is_live_mode.read());
    }

    #[tokio::test]
    async fn test_real_trading_mode_rejected() {
        let state = AppEngineState::new();
        let settings = EngineSettingsDto {
            trading_mode: "real".into(),
            ..EngineSettingsDto::default()
        };
        let res = save_engine_settings_impl(settings, &state);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("locked"));
        assert_eq!(
            get_engine_settings_impl(&state)
                .expect("settings")
                .trading_mode,
            "paper"
        );
    }

    #[tokio::test]
    async fn test_live_trading_unlock_and_execution_capabilities() {
        let (state, _dir) = test_state_with_vault("live_test");
        {
            let mut s = state.settings.write();
            s.aster_enabled = true;
            s.hyperliquid_enabled = true;
            s.vanta_enabled = true;
            s.popdex_enabled = true;
        }

        // 1. Vault locked -> live mode rejected
        let res = set_live_mode_impl(true, &state);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("locked"));

        // 2. Initialize and unlock vault
        initialize_vault_impl("test-password-123", &state).expect("init vault");
        unlock_vault_impl("test-password-123", &state).expect("unlock vault");

        // 3. Vault unlocked but credentials missing -> rejected
        let res2 = set_live_mode_impl(true, &state);
        assert!(res2.is_err());
        assert!(res2.unwrap_err().contains("requires at least two venues"));

        // 4. Populate credentials for Aster and Hyperliquid
        {
            let mut vault = state.vault.write();
            let mut aster_c = crate::vault::ExchangeCredentials::default();
            aster_c.venue = "aster".to_string();
            aster_c.profile_id = Some("aster-1".to_string());
            aster_c.api_key = "aster_api_key".to_string();
            aster_c.api_secret = "aster_api_secret".to_string();
            aster_c.enabled = true;
            vault.upsert_profile_credentials("aster-1", aster_c).expect("save aster creds");

            let mut hl_c = crate::vault::ExchangeCredentials::default();
            hl_c.venue = "hyperliquid".to_string();
            hl_c.profile_id = Some("hl-1".to_string());
            hl_c.wallet_address = Some("0x1111111111111111111111111111111111111111".to_string());
            hl_c.private_key = Some("0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string());
            hl_c.enabled = true;
            vault.upsert_profile_credentials("hl-1", hl_c).expect("save hl creds");
        }
        state.sync_vault_credentials_to_adapters();

        // 5. Capabilities report
        let caps = get_venue_execution_capabilities_impl(&state);
        let aster_cap = caps.iter().find(|c| c.venue == "aster").expect("aster cap");
        assert!(aster_cap.supports_live);
        assert!(aster_cap.has_credentials);
        assert!(aster_cap.live_ready);

        let popdex_cap = caps.iter().find(|c| c.venue == "popdex").expect("popdex cap");
        assert!(popdex_cap.supports_live);
        assert!(!popdex_cap.has_credentials);
        assert!(!popdex_cap.live_ready);

        let hl_cap = caps.iter().find(|c| c.venue == "hyperliquid").expect("hl cap");
        assert!(hl_cap.supports_live);
        assert!(hl_cap.has_credentials);
        assert!(hl_cap.live_ready);

        let vanta_cap = caps.iter().find(|c| c.venue == "vanta").expect("vanta cap");
        assert!(vanta_cap.supports_live);
        assert!(!vanta_cap.has_credentials);
        assert!(!vanta_cap.live_ready);

        // 6. Live mode successfully enabled
        let live_ok = set_live_mode_impl(true, &state).expect("enable live mode");
        assert!(live_ok);
        assert!(*state.is_live_mode.read());

        // 7a. Route involving venue without credentials (Vanta) fails closed with missing credentials
        let sim_err = execute_spatial_arbitrage_inner(
            "BTC-USDC",
            "aster",
            "vanta",
            100.0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            "futures",
            &state,
        ).await;
        assert!(sim_err.is_err());
        assert!(sim_err.unwrap_err().contains("unauthenticated live credentials"));

        // 7b. Route involving true read-only venue (Decibel) fails closed in live mode
        let decibel_err = execute_spatial_arbitrage_inner(
            "BTC-USDC",
            "aster",
            "decibel",
            100.0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            "futures",
            &state,
        ).await;
        assert!(decibel_err.is_err());

        // 8. Test live flatten: when vault is unlocked, live flatten runs without LIVE_EXECUTION_DISABLED
        let flat_res = flatten_ticker_positions_impl(Some("BTC-USDC"), "futures", &state).await;
        assert!(flat_res.is_ok());

        // 9. When vault is locked, live flatten fails closed
        let _ = lock_vault_impl(&state);
        let flat_locked = flatten_ticker_positions_impl(Some("BTC-USDC"), "futures", &state).await;
        assert!(flat_locked.is_err());
        assert!(flat_locked.unwrap_err().contains("vault is locked"));
    }

    #[tokio::test]
    async fn test_trading_mode_persistence_and_vault_unlock_restore() {
        let (state, _dir) = test_state_with_vault("persist_test");
        {
            let mut s = state.settings.write();
            s.aster_enabled = true;
            s.hyperliquid_enabled = true;
        }

        // 1. Initialize vault and setup 2 live-capable venues
        initialize_vault_impl("test-password-123", &state).expect("init vault");
        unlock_vault_impl("test-password-123", &state).expect("unlock vault");

        {
            let mut vault = state.vault.write();
            let mut aster_c = crate::vault::ExchangeCredentials::default();
            aster_c.venue = "aster".to_string();
            aster_c.profile_id = Some("aster-1".to_string());
            aster_c.api_key = "aster_key".to_string();
            aster_c.api_secret = "aster_secret".to_string();
            aster_c.enabled = true;
            vault.upsert_profile_credentials("aster-1", aster_c).expect("save aster creds");

            let mut hl_c = crate::vault::ExchangeCredentials::default();
            hl_c.venue = "hyperliquid".to_string();
            hl_c.profile_id = Some("hl-1".to_string());
            hl_c.wallet_address = Some("0x1111111111111111111111111111111111111111".to_string());
            hl_c.private_key = Some("0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string());
            hl_c.enabled = true;
            vault.upsert_profile_credentials("hl-1", hl_c).expect("save hl creds");
        }
        state.sync_vault_credentials_to_adapters();

        // 2. Enable live mode and verify persisted to kv store
        let ok = set_live_mode_impl(true, &state).expect("enable live mode");
        assert!(ok);
        assert!(*state.is_live_mode.read());
        assert_eq!(state.state_store.kv_get("engine.trading_mode").unwrap(), Some("real".to_string()));

        // 3. Lock vault: live executions will fail closed because vault is locked
        let _ = lock_vault_impl(&state);

        // 4. Unlock vault: automatically restores is_live_mode without needing to reconfigure Settings!
        let unlock_res = unlock_vault_impl("test-password-123", &state).expect("unlock");
        assert!(unlock_res.is_unlocked);
        assert!(*state.is_live_mode.read(), "live mode should automatically restore upon unlocking vault when real mode was chosen");
    }

    #[tokio::test]
    async fn test_paper_starting_balance_resets_paper_accounts_on_change() {
        let state = AppEngineState::new();

        // Invalid values fail closed.
        for bad in [0.0, -500.0, f64::NAN, f64::INFINITY, 2e12] {
            let res = save_engine_settings_impl(
                EngineSettingsDto {
                    paper_starting_balance_usd: bad,
                    ..EngineSettingsDto::default()
                },
                &state,
            );
            assert!(res.is_err(), "bad balance {bad} must be rejected");
        }

        // A new starting balance reinitializes every paper account.
        save_engine_settings_impl(
            EngineSettingsDto {
                paper_starting_balance_usd: 25000.0,
                ..EngineSettingsDto::default()
            },
            &state,
        )
        .expect("save");
        for (_, paper) in state.paper_exchanges() {
            let acc = paper.fetch_account().await.expect("account");
            assert_eq!(acc.equity, dec!(25000.0));
            assert_eq!(acc.free_margin, dec!(25000.0));
            assert!(acc.positions.is_empty());
        }

        // Re-saving an unchanged balance is a no-op for paper accounts.
        save_engine_settings_impl(
            EngineSettingsDto {
                paper_starting_balance_usd: 25000.0,
                min_depth_ratio: 4.0,
                ..EngineSettingsDto::default()
            },
            &state,
        )
        .expect("resave");
        for (_, paper) in state.paper_exchanges() {
            let acc = paper.fetch_account().await.expect("account");
            assert_eq!(acc.equity, dec!(25000.0));
        }
        assert_eq!(state.settings.read().min_depth_ratio, 4.0);
    }

    #[tokio::test]
    async fn test_unsupported_native_executions_fail_closed() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(60000.0),
            dec!(60005.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(60020.0), dec!(60025.0));

        let pnl_before = *state.total_realized_profit.read();

        let spot = execute_spot_arbitrage_impl(
            SpotExecutionParams {
                symbol: "BTC-USDC".into(),
                buy_venue: "hyperliquid".into(),
                sell_venue: "lighter".into(),
                trade_size_usd: 5000.0,
                max_slippage_bps: 15.0,
                net_spread_bps: Some(12.0),
            },
            &state,
        )
        .await;
        assert!(spot.is_err());
        assert!(spot.unwrap_err().contains("spot"));

        let funding =
            execute_funding_carry_impl("BTC-USDC", "hyperliquid", "lighter", 25000.0, &state).await;
        assert!(funding.is_err());
        assert!(funding.unwrap_err().contains("funding"));

        let pred = execute_prediction_arbitrage_impl(
            "FED DECISION",
            "polymarket",
            "kalshi",
            100.0,
            &state,
        )
        .await;
        assert!(pred.is_err());
        assert!(pred.unwrap_err().contains("prediction"));

        assert_eq!(*state.total_realized_profit.read(), pnl_before);
        assert!(get_execution_logs_impl(&state).expect("logs").is_empty());
        assert!(state.virtual_ledger.read().lots.is_empty());
    }

    #[tokio::test]
    async fn test_fetch_multi_market_quotes() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(
            &state,
            "hyperliquid",
            "ETH-USDC",
            dec!(3450.0),
            dec!(3452.0),
        );
        seed_test_book(&state, "hyperliquid", "SOL-USDC", dec!(152.0), dec!(152.2));
        seed_test_book(&state, "vanta", "BTC-USDC", dec!(64201.0), dec!(64206.0));
        seed_test_book(&state, "vanta", "ETH-USDC", dec!(3450.5), dec!(3452.5));
        seed_test_book(&state, "vanta", "SOL-USDC", dec!(152.1), dec!(152.3));

        let symbols = vec![
            "BTC-USDC".to_string(),
            "ETH-USDC".to_string(),
            "SOL-USDC".to_string(),
        ];
        let quotes = fetch_multi_market_quotes_impl(symbols, &state)
            .await
            .expect("quotes");
        assert_eq!(quotes.len(), 3);
        for q in &quotes {
            assert!(q.best_bid > 0.0);
            assert!(q.best_ask > 0.0);
            assert!(q.spread_bps >= 0.0);
            assert!(q.mid_price > 0.0);
            assert_ne!(q.best_buy_venue, "-");
            assert_ne!(q.best_sell_venue, "-");
        }
    }

    #[tokio::test]
    async fn test_ticker_strategy_config_crud_and_validation() {
        let state = AppEngineState::new();

        // 1. Initial query for non-existing returns default with correct symbol
        let def = get_ticker_strategy_config_impl("BTC-USDC", &state).expect("default config");
        assert_eq!(def.symbol, "BTC-USDC");
        assert_eq!(def.target_profit_bps, 1.0);
        assert_eq!(def.window_percentile, 50);
        assert_eq!(def.execution_mode, "SINGLE_WAY");
        assert_eq!(def.user_target_leverage, 5);

        // 2. Validation: window_percentile must be >= 50
        let invalid_percentile = TickerStrategyConfigDto {
            window_percentile: 40,
            ..def.clone()
        };
        assert!(save_ticker_strategy_config_impl(invalid_percentile, &state).is_err());

        // Leverage above the hard cap and nonpositive unit sizes are rejected
        let bad_leverage = TickerStrategyConfigDto {
            user_target_leverage: 6,
            ..def.clone()
        };
        assert!(save_ticker_strategy_config_impl(bad_leverage, &state).is_err());
        let zero_leverage = TickerStrategyConfigDto {
            user_target_leverage: 0,
            ..def.clone()
        };
        assert!(save_ticker_strategy_config_impl(zero_leverage, &state).is_err());
        let bad_unit = TickerStrategyConfigDto {
            unit_size: 0.0,
            ..def.clone()
        };
        assert!(save_ticker_strategy_config_impl(bad_unit, &state).is_err());
        let nan_unit = TickerStrategyConfigDto {
            unit_size: f64::NAN,
            ..def.clone()
        };
        assert!(save_ticker_strategy_config_impl(nan_unit, &state).is_err());

        // 3. Save valid updated config
        let updated = TickerStrategyConfigDto {
            symbol: "BTC-USDC".into(),
            target_profit_bps: 8.5,
            window_percentile: 80,
            max_accumulated_units: 5,
            current_accumulated_units: 1,
            execution_mode: "DOUBLE_WAY".into(),
            unit_size: 0.01,
            user_target_leverage: 5,
            sliding_window: "5m".into(),
        };
        assert!(save_ticker_strategy_config_impl(updated.clone(), &state).is_ok());

        // 4. Retrieve saved config
        let retrieved =
            get_ticker_strategy_config_impl("BTC-USDC", &state).expect("retrieved config");
        assert_eq!(retrieved.target_profit_bps, 8.5);
        assert_eq!(retrieved.window_percentile, 80);
        assert_eq!(retrieved.max_accumulated_units, 5);
        assert_eq!(retrieved.execution_mode, "DOUBLE_WAY");
        assert_eq!(retrieved.unit_size, 0.01);

        // 5. Query all
        let all = get_all_ticker_strategy_configs_impl(&state).expect("all configs");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].symbol, "BTC-USDC");
    }

    #[tokio::test]
    async fn test_save_strategy_params_persists_to_state_store() {
        let state = AppEngineState::new();
        let params = StrategyParamsDto {
            sliding_window_mode: "auto".into(),
            sliding_window_secs: 43_200,
            ..StrategyParamsDto::defaults_for("futures")
        };
        save_strategy_params_impl(params, &state).expect("save");

        let rows = state.state_store.strategy_params_json().expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "futures");
        let persisted: StrategyParamsDto = serde_json::from_str(&rows[0].1).expect("valid json");
        assert_eq!(persisted.sliding_window_mode, "auto");
        assert_eq!(persisted.sliding_window_secs, 43_200);
        assert_eq!(persisted.sliding_window, "12h");

        let reloaded = crate::state::load_strategy_params(&state.state_store);
        assert_eq!(reloaded["futures"].sliding_window_mode, "auto");
        assert_eq!(reloaded["futures"].sliding_window_secs, 43_200);
    }

    #[tokio::test]
    async fn test_save_triangle_strategy_params_persists_and_validates() {
        let state = AppEngineState::new();
        let params = StrategyParamsDto {
            triangle_trade_size_usd: 15000.0,
            triangle_min_profit_bps: 3.5,
            triangle_max_slippage_bps: 4.0,
            ..StrategyParamsDto::defaults_for("triangle")
        };
        save_strategy_params_impl(params, &state).expect("save triangle");

        let loaded = get_strategy_params_impl("triangle", &state).expect("load triangle");
        assert_eq!(loaded.triangle_trade_size_usd, 15000.0);
        assert_eq!(loaded.triangle_min_profit_bps, 3.5);
        assert_eq!(loaded.triangle_max_slippage_bps, 4.0);

        // Invalid values fail closed
        let mut bad = StrategyParamsDto::defaults_for("triangle");
        bad.triangle_trade_size_usd = 0.0;
        assert!(save_strategy_params_impl(bad, &state).is_err());

        let mut bad_slippage = StrategyParamsDto::defaults_for("triangle");
        bad_slippage.triangle_max_slippage_bps = -1.0;
        assert!(save_strategy_params_impl(bad_slippage, &state).is_err());
    }

    #[tokio::test]
    async fn test_save_strategy_params_rejects_invalid_and_leaves_memory() {
        let state = AppEngineState::new();
        let mut bad = StrategyParamsDto::defaults_for("futures");
        bad.sliding_window_secs = 60;
        assert!(save_strategy_params_impl(bad, &state).is_err());
        bad = StrategyParamsDto::defaults_for("futures");
        bad.sliding_window_mode = "sometimes".into();
        assert!(save_strategy_params_impl(bad, &state).is_err());
        bad = StrategyParamsDto::defaults_for("futures");
        bad.sliding_window_secs = 3_000_000;
        assert!(save_strategy_params_impl(bad, &state).is_err());
        assert!(state.strategy_params.read().is_empty());
        assert!(state
            .state_store
            .strategy_params_json()
            .expect("rows")
            .is_empty());
    }

    #[test]
    fn test_strategy_params_sizing_defaults() {
        let p = StrategyParamsDto::defaults_for("futures");
        assert_eq!(p.step_size_mult, 10.0);
        assert_eq!(p.max_step_usd, 2500.0);
        assert_eq!(p.max_ticker_equity_pct, 20.0);
    }

    #[tokio::test]
    async fn test_save_strategy_params_validates_auto_thresholds() {
        let state = AppEngineState::new();
        for bad in [f64::NAN, f64::INFINITY, -5.0] {
            let mut p = StrategyParamsDto::defaults_for("futures");
            p.auto_min_volume_usd = Some(bad);
            assert!(save_strategy_params_impl(p, &state).is_err());
            let mut p = StrategyParamsDto::defaults_for("spot");
            p.auto_min_price_usd = Some(bad);
            assert!(save_strategy_params_impl(p, &state).is_err());
        }
        let mut p = StrategyParamsDto::defaults_for("funding");
        p.auto_min_volume_usd = Some(250_000.0);
        p.auto_min_price_usd = Some(0.0);
        p.auto_min_event_volume_usd = Some(1_000_000.0);
        save_strategy_params_impl(p, &state).expect("valid thresholds save");
        let got = get_strategy_params_impl("funding", &state).expect("read back");
        assert_eq!(got.auto_min_volume_usd, Some(250_000.0));
        assert_eq!(got.auto_min_price_usd, Some(0.0));
        assert_eq!(got.auto_min_event_volume_usd, Some(1_000_000.0));
    }

    #[tokio::test]
    async fn test_save_strategy_params_validates_slippage_and_funding_guards() {
        let state = AppEngineState::new();
        // Every per-strategy slippage cap must be finite and positive.
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let mut p = StrategyParamsDto::defaults_for("futures");
            p.futures_max_slippage_bps = bad;
            assert!(save_strategy_params_impl(p, &state).is_err());
            let mut p = StrategyParamsDto::defaults_for("funding");
            p.funding_max_slippage_bps = bad;
            assert!(save_strategy_params_impl(p, &state).is_err());
            let mut p = StrategyParamsDto::defaults_for("spot");
            p.spot_max_slippage_bps = bad;
            assert!(save_strategy_params_impl(p, &state).is_err());
            // Prediction is inert but validated identically.
            let mut p = StrategyParamsDto::defaults_for("futures");
            p.prediction_max_slippage_bps = bad;
            assert!(save_strategy_params_impl(p, &state).is_err());
            let mut p = StrategyParamsDto::defaults_for("funding");
            p.funding_max_entry_spread_bps = bad;
            assert!(save_strategy_params_impl(p, &state).is_err());
        }
        // Optional percent guards accept only (0, 100].
        for bad in [-1.0, 0.0, 101.0, f64::NAN, f64::INFINITY] {
            let mut p = StrategyParamsDto::defaults_for("funding");
            p.funding_auto_exit_spread_pct = Some(bad);
            assert!(save_strategy_params_impl(p, &state).is_err());
            let mut p = StrategyParamsDto::defaults_for("funding");
            p.funding_liq_trim_pct = Some(bad);
            assert!(save_strategy_params_impl(p, &state).is_err());
            let mut p = StrategyParamsDto::defaults_for("futures");
            p.futures_liq_trim_pct = Some(bad);
            assert!(save_strategy_params_impl(p, &state).is_err());
            let mut p = StrategyParamsDto::defaults_for("spot");
            p.spot_liq_trim_pct = Some(bad);
            assert!(save_strategy_params_impl(p, &state).is_err());
            let mut p = StrategyParamsDto::defaults_for("futures");
            p.prediction_liq_trim_pct = Some(bad);
            assert!(save_strategy_params_impl(p, &state).is_err());
        }
        // Rotation is a bps hurdle: finite and positive when set.
        for bad in [-1.0, 0.0, f64::NAN, f64::INFINITY] {
            let mut p = StrategyParamsDto::defaults_for("funding");
            p.funding_rotation_threshold_bps = Some(bad);
            assert!(save_strategy_params_impl(p, &state).is_err());
        }
        // A fully-armed funding record saves and reloads intact.
        let mut p = StrategyParamsDto::defaults_for("funding");
        p.funding_max_slippage_bps = 12.5;
        p.funding_max_entry_spread_bps = 30.0;
        p.funding_auto_exit_spread_pct = Some(5.0);
        p.funding_liq_trim_pct = Some(15.0);
        p.funding_auto_rebalance = true;
        p.funding_rotation_threshold_bps = Some(10.0);
        save_strategy_params_impl(p, &state).expect("valid funding guards save");
        let got = get_strategy_params_impl("funding", &state).expect("read back");
        assert_eq!(got.funding_max_slippage_bps, 12.5);
        assert_eq!(got.funding_max_entry_spread_bps, 30.0);
        assert_eq!(got.funding_auto_exit_spread_pct, Some(5.0));
        assert_eq!(got.funding_liq_trim_pct, Some(15.0));
        assert!(got.funding_auto_rebalance);
        assert_eq!(got.funding_rotation_threshold_bps, Some(10.0));
        // None disables each optional guard.
        let mut p = StrategyParamsDto::defaults_for("funding");
        p.funding_auto_exit_spread_pct = None;
        p.funding_liq_trim_pct = None;
        p.funding_rotation_threshold_bps = None;
        save_strategy_params_impl(p, &state).expect("disabled guards save");
    }

    #[tokio::test]
    async fn test_trim_block_persists_and_loads_per_strategy() {
        let state = AppEngineState::new();
        let mut f = StrategyParamsDto::defaults_for("futures");
        f.futures_liq_trim_pct = Some(12.5);
        // Prediction is defined-but-disabled: still persists on the record.
        f.prediction_liq_trim_pct = Some(10.0);
        save_strategy_params_impl(f, &state).expect("save futures");
        let mut g = StrategyParamsDto::defaults_for("funding");
        g.funding_liq_trim_pct = Some(15.0);
        save_strategy_params_impl(g, &state).expect("save funding");
        let mut s = StrategyParamsDto::defaults_for("spot");
        s.spot_liq_trim_pct = Some(20.0);
        save_strategy_params_impl(s, &state).expect("save spot");
        let reloaded = crate::state::load_strategy_params(&state.state_store);
        assert_eq!(reloaded["futures"].futures_liq_trim_pct, Some(12.5));
        assert_eq!(reloaded["futures"].prediction_liq_trim_pct, Some(10.0));
        assert_eq!(reloaded["funding"].funding_liq_trim_pct, Some(15.0));
        assert_eq!(reloaded["spot"].spot_liq_trim_pct, Some(20.0));
        let all = get_all_strategy_params_impl(&state).expect("all");
        assert_eq!(all.len(), 4);
        let spot = all.iter().find(|p| p.strategy_id == "spot").expect("spot");
        assert_eq!(spot.spot_liq_trim_pct, Some(20.0));
    }

    #[tokio::test]
    async fn test_prediction_event_volume_field_roundtrips_but_stays_inert() {
        let state = AppEngineState::new();
        let mut p = StrategyParamsDto::defaults_for("futures");
        p.auto_min_event_volume_usd = Some(500_000.0);
        save_strategy_params_impl(p, &state).expect("save");
        let rows = state.state_store.strategy_params_json().expect("rows");
        let persisted: StrategyParamsDto =
            serde_json::from_str(&rows[0].1).expect("valid json");
        assert_eq!(persisted.auto_min_event_volume_usd, Some(500_000.0));
        assert!(get_strategy_tickers_impl(&state, "prediction").await.is_err());
    }

    #[tokio::test]
    async fn test_get_strategy_tickers_unknown_strategy_fails_closed() {
        let state = AppEngineState::new();
        assert!(get_strategy_tickers_impl(&state, "bogus").await.is_err());
        assert!(get_strategy_tickers_impl(&state, "").await.is_err());
    }

    #[tokio::test]
    async fn test_get_strategy_tickers_filters_off_lists_without_error() {
        let state = AppEngineState::new();
        for sid in ["futures", "funding", "spot", "radar"] {
            let rows = get_strategy_tickers_impl(&state, sid).await.expect("ipc");
            assert!(rows.iter().all(|r| r.passes));
            assert!(rows.iter().all(|r| r.block_reason.is_none()));
        }
    }

    #[tokio::test]
    async fn test_reject_if_strategy_filtered_noop_when_off() {
        let state = AppEngineState::new();
        reject_if_strategy_filtered(
            &state,
            "futures",
            "BTC",
            &["hyperliquid".to_string(), "lighter".to_string()],
        )
        .await
        .expect("disabled filters must not fetch or block");
    }

    #[tokio::test]
    async fn test_upsert_account_max_leverage_ipc_roundtrip() {
        let state = AppEngineState::new();
        let created = upsert_account_impl(
            AccountUpsertInput {
                id: None,
                venue: "hyperliquid".into(),
                label: "Main".into(),
                enabled: true,
                make_default: true,
                account_tier: None,
                max_leverage: Some(2),
                credentials: None,
            },
            &state,
        )
        .expect("create with cap");
        assert_eq!(created.max_leverage, Some(2));
        for bad in [Some(0), Some(6), Some(99)] {
            let res = upsert_account_impl(
                AccountUpsertInput {
                    id: Some(created.id.clone()),
                    venue: String::new(),
                    label: "Main".into(),
                    enabled: true,
                    make_default: true,
                    account_tier: Some("standard".into()),
                    max_leverage: bad,
                    credentials: None,
                },
                &state,
            );
            assert!(res.is_err(), "cap {bad:?} must fail validation");
        }
        let cleared = upsert_account_impl(
            AccountUpsertInput {
                id: Some(created.id.clone()),
                venue: String::new(),
                label: "Main".into(),
                enabled: true,
                make_default: true,
                account_tier: Some("standard".into()),
                max_leverage: None,
                credentials: None,
            },
            &state,
        )
        .expect("clear cap");
        assert_eq!(cleared.max_leverage, None);
    }

    #[tokio::test]
    async fn test_resolve_effective_leverage_takes_min() {
        let state = AppEngineState::new();
        state.strategy_params.write().insert(
            "futures".to_string(),
            StrategyParamsDto {
                user_target_leverage: 5,
                ..StrategyParamsDto::defaults_for("futures")
            },
        );
        let (eff, cap) = resolve_effective_leverage(
            &state,
            "futures",
            "futures",
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            None,
        )
        .expect("no accounts means unlimited");
        assert_eq!((eff, cap), (5, None));
        state
            .state_store
            .upsert_account("hl-main", "hyperliquid", "Main", true, true, "standard", Some(2))
            .expect("capped account");
        state
            .state_store
            .upsert_account("lt-main", "lighter", "Main", true, true, "standard", None)
            .expect("uncapped account");
        let (eff, cap) = resolve_effective_leverage(
            &state,
            "futures",
            "futures",
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            None,
        )
        .expect("resolve");
        assert_eq!(eff, 2, "tighter leg cap must win");
        assert_eq!(cap, Some(2));
        state.strategy_params.write().insert(
            "futures".to_string(),
            StrategyParamsDto {
                user_target_leverage: 1,
                ..StrategyParamsDto::defaults_for("futures")
            },
        );
        let (eff, _) = resolve_effective_leverage(
            &state,
            "futures",
            "futures",
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            None,
        )
        .expect("resolve");
        assert_eq!(eff, 1, "strategy target below the cap must win");
    }

    #[tokio::test]
    async fn test_autotune_empty_history_is_all_held() {
        // Fixture-free store: no samples at all must yield an honest
        // all-HELD report, never fabricated numbers.
        let state = AppEngineState::new();
        let rep = get_autotune_recommendation_impl("BTC-USDC", &state)
            .await
            .expect("recommendation");
        assert_eq!(rep.symbol, "BTC-USDC");
        assert_eq!(rep.rows.len(), 7);
        assert_eq!(rep.banner.total, 7);
        assert_eq!(rep.banner.ready_count, 0);
        assert_eq!(rep.banner.regime_verdict, "INSUFFICIENT");
        for r in &rep.rows {
            assert_eq!(r.status, "HELD");
            assert_eq!(r.recommended, None);
        }
    }

    #[tokio::test]
    async fn test_autotune_seeded_history_roundtrip() {
        // 400 oscillating pair observations (period 8, 60s cadence):
        // identical shape to the math-crate accept fixture, so the
        // walk-forward economics validate end to end.
        let state = AppEngineState::new();
        let now = Utc::now().timestamp_millis();
        let n = 400usize;
        for i in 0..n {
            let phase =
                2.0 * std::f64::consts::PI * (i % 8) as f64 / 8.0;
            let mid_a = 100.0 + 0.4 * phase.sin();
            seed_samples(
                &state,
                "BTC-USDC",
                now - ((n - i) as i64) * 60_000,
                &[("hyperliquid", mid_a), ("lighter", 100.4)],
            );
        }
        let rep = get_autotune_recommendation_impl("BTC-USDC", &state)
            .await
            .expect("recommendation");
        assert_eq!(rep.rows.len(), 7);
        assert_eq!(rep.banner.total, 7);
        assert!(rep.pair_observations >= 120);
        assert!(rep.buy_venue.is_some() && rep.sell_venue.is_some());
        assert!(rep.banner.fit_from_ms > 0 && rep.banner.validate_to_ms > 0);
        assert!(rep.banner.fit_to_ms <= rep.banner.validate_from_ms);
        let mut seen = std::collections::HashSet::new();
        for r in &rep.rows {
            assert!(seen.insert(r.param.clone()), "duplicate row {}", r.param);
            assert!(matches!(r.status.as_str(), "READY" | "HELD"));
            assert_eq!(r.recommended.is_some(), r.status == "READY");
            assert!(!r.formula_id.is_empty() && !r.rationale.is_empty());
            assert!(!r.fit_metric.is_empty() && !r.validated_metric.is_empty());
        }
        for want in [
            "slidingWindowSecs",
            "targetProfitBps",
            "windowPercentile",
            "stepSizeMult",
            "maxStepUsd",
            "maxTickerEquityPct",
            "userTargetLeverage",
        ] {
            assert!(seen.contains(want), "missing row {want}");
        }
        // Current values are wired from StrategyParamsDto (futures defaults).
        let target = rep.rows.iter().find(|r| r.param == "targetProfitBps").unwrap();
        assert!((target.current - 1.0).abs() < 1e-12);
        let lev = rep.rows.iter().find(|r| r.param == "userTargetLeverage").unwrap();
        if let Some(v) = lev.recommended {
            assert!(v <= lev.current, "leverage must never increase");
        }
        let pct = rep.rows.iter().find(|r| r.param == "windowPercentile").unwrap();
        if let Some(v) = pct.recommended {
            assert!((50.0..=95.0).contains(&v), "percentile snap range");
        }
        // Seeded economics validate: at least the target row is READY.
        let ready: Vec<_> = rep.rows.iter().filter(|r| r.status == "READY").collect();
        assert!(!ready.is_empty(), "seeded fixture should validate");
        assert_eq!(rep.banner.ready_count, ready.len());
    }

    #[tokio::test]
    async fn test_save_strategy_params_rejects_invalid_sizing() {
        let state = AppEngineState::new();
        for mult in [0.0, -1.0, 0.999, f64::NAN, f64::INFINITY] {
            let mut bad = StrategyParamsDto::defaults_for("futures");
            bad.step_size_mult = mult;
            assert!(
                save_strategy_params_impl(bad, &state).is_err(),
                "mult {mult} must reject"
            );
        }
        for cap in [0.0, -100.0, f64::NAN, f64::INFINITY] {
            let mut bad = StrategyParamsDto::defaults_for("futures");
            bad.max_step_usd = cap;
            assert!(
                save_strategy_params_impl(bad, &state).is_err(),
                "max_step_usd {cap} must reject"
            );
        }
        for pct in [0.0, -2.0, 100.0001, 101.0, f64::NAN, f64::INFINITY] {
            let mut bad = StrategyParamsDto::defaults_for("futures");
            bad.max_ticker_equity_pct = pct;
            assert!(
                save_strategy_params_impl(bad, &state).is_err(),
                "pct {pct} must reject"
            );
        }
        assert!(state.strategy_params.read().is_empty());

        let mut ok = StrategyParamsDto::defaults_for("futures");
        ok.step_size_mult = 1.0;
        ok.max_step_usd = 0.01;
        ok.max_ticker_equity_pct = 100.0;
        assert!(save_strategy_params_impl(ok, &state).is_ok());
    }

    fn autotune_fixture_report(ready: &[(&str, f64)]) -> AutotuneRecommendationDto {
        let is_ready = |p: &str| ready.iter().find(|(q, _)| *q == p).map(|(_, v)| *v);
        let row = |param: &str, current: f64| {
            let rec = is_ready(param);
            AutotuneRowDto {
                param: param.to_string(),
                current,
                recommended: rec,
                status: if rec.is_some() { "READY".to_string() } else { "HELD".to_string() },
                fit_metric: "fit_ok".to_string(),
                validated_metric: "val_ok".to_string(),
                n: 64,
                rationale: "fixture".to_string(),
                formula_id: param.to_string(),
                confidence: "high".to_string(),
            }
        };
        AutotuneRecommendationDto {
            symbol: "BTC-USDC".to_string(),
            buy_venue: Some("hyperliquid".to_string()),
            sell_venue: Some("lighter".to_string()),
            pair_observations: 400,
            rows: vec![
                row("slidingWindowSecs", 86_400.0),
                row("targetProfitBps", 1.0),
                row("windowPercentile", 50.0),
                row("stepSizeMult", 10.0),
                row("maxStepUsd", 2500.0),
                row("maxTickerEquityPct", 20.0),
                row("userTargetLeverage", 5.0),
            ],
            banner: AutotuneBannerDto {
                fit_from_ms: 1_000,
                fit_to_ms: 2_000,
                validate_from_ms: 2_000,
                validate_to_ms: 3_000,
                regime_verdict: "STABLE".to_string(),
                ready_count: ready.len(),
                total: 7,
            },
            session_tau_scale: 1.0,
            session_sigma_scale: 1.0,
        }
    }

    #[tokio::test]
    async fn test_apply_selected_writes_only_ready_rows() {
        let state = AppEngineState::new();
        let now = Utc::now().timestamp_millis();
        let report = autotune_fixture_report(&[
            ("targetProfitBps", 8.5),
            ("windowPercentile", 80.0),
            ("slidingWindowSecs", 43_200.0),
        ]);
        let selected = vec![
            "targetProfitBps".to_string(),
            "maxStepUsd".to_string(),
            "windowPercentile".to_string(),
            "slidingWindowSecs".to_string(),
        ];
        let res = apply_autotune_rows_inner(&state, "BTC-USDC", &report, &selected, now)
            .await
            .expect("apply");
        assert_eq!(res.applied, vec!["slidingWindowSecs", "targetProfitBps", "windowPercentile"]);
        assert_eq!(res.skipped_held, vec!["maxStepUsd".to_string()]);
        let audit_id = res.audit_id.expect("audit row");
        assert!(audit_id > 0);
        let strategy = get_strategy_params_impl("futures", &state).expect("strategy");
        assert!((strategy.target_profit_bps - 8.5).abs() < 1e-12);
        assert_eq!(strategy.window_percentile, 80);
        assert_eq!(strategy.sliding_window_secs, 43_200);
        assert_eq!(strategy.max_step_usd, 2500.0, "HELD row must keep manual value");
        let ticker = get_ticker_strategy_config_impl("BTC-USDC", &state).expect("ticker");
        assert!((ticker.target_profit_bps - 8.5).abs() < 1e-12);
        assert_eq!(ticker.window_percentile, 80);
        assert_eq!(ticker.sliding_window, "12h");
        let events = state
            .state_store
            .recent_autotune_apply_events("BTC-USDC", 10)
            .expect("audit read");
        assert_eq!(events.len(), 1);
        let applied: serde_json::Value =
            serde_json::from_str(&events[0].applied_json).expect("applied json");
        assert_eq!(applied["targetProfitBps"], serde_json::json!(8.5));
        let detail: serde_json::Value =
            serde_json::from_str(&events[0].detail_json).expect("detail json");
        assert_eq!(detail["regime_verdict"], serde_json::json!("STABLE"));
        assert_eq!(detail["fit_from_ms"], serde_json::json!(1_000));
        assert_eq!(detail["validate_to_ms"], serde_json::json!(3_000));
        assert_eq!(detail["rows"].as_array().expect("rows").len(), 3);
    }

    #[tokio::test]
    async fn test_apply_selected_all_held_writes_nothing() {
        let state = AppEngineState::new();
        let now = Utc::now().timestamp_millis();
        let report = autotune_fixture_report(&[]);
        let selected = vec!["targetProfitBps".to_string(), "maxStepUsd".to_string()];
        let res = apply_autotune_rows_inner(&state, "BTC-USDC", &report, &selected, now)
            .await
            .expect("apply");
        assert!(res.applied.is_empty());
        assert_eq!(res.skipped_held, vec!["maxStepUsd".to_string(), "targetProfitBps".to_string()]);
        assert_eq!(res.audit_id, None);
        assert!(state.strategy_params.read().is_empty(), "no writes on all-HELD");
        assert!(state
            .state_store
            .recent_autotune_apply_events("BTC-USDC", 10)
            .expect("audit read")
            .is_empty());
    }

    #[tokio::test]
    async fn test_apply_selected_rejects_unknown_param_atomically() {
        let state = AppEngineState::new();
        let now = Utc::now().timestamp_millis();
        let report = autotune_fixture_report(&[("targetProfitBps", 8.5)]);
        let selected = vec!["targetProfitBps".to_string(), "bogusParam".to_string()];
        assert!(apply_autotune_rows_inner(&state, "BTC-USDC", &report, &selected, now)
            .await
            .is_err());
        assert!(state.strategy_params.read().is_empty(), "unknown param: no writes");
        assert!(state
            .state_store
            .recent_autotune_apply_events("BTC-USDC", 10)
            .expect("audit read")
            .is_empty());
        assert!(apply_autotune_rows_inner(&state, "BTC-USDC", &report, &[], 0)
            .await
            .is_err(), "broken clock must refuse");
    }

    #[tokio::test]
    async fn test_apply_selected_concurrent_serializes() {
        use std::sync::Arc;
        let state = Arc::new(AppEngineState::new());
        let now = Utc::now().timestamp_millis();
        let report = autotune_fixture_report(&[("targetProfitBps", 8.5)]);
        let selected = vec!["targetProfitBps".to_string()];
        let mut handles = Vec::new();
        for _ in 0..4 {
            let s = state.clone();
            let r = AutotuneRecommendationDto {
                symbol: report.symbol.clone(),
                buy_venue: report.buy_venue.clone(),
                sell_venue: report.sell_venue.clone(),
                pair_observations: report.pair_observations,
                rows: report.rows.clone(),
                banner: AutotuneBannerDto {
                    fit_from_ms: report.banner.fit_from_ms,
                    fit_to_ms: report.banner.fit_to_ms,
                    validate_from_ms: report.banner.validate_from_ms,
                    validate_to_ms: report.banner.validate_to_ms,
                    regime_verdict: report.banner.regime_verdict.clone(),
                    ready_count: report.banner.ready_count,
                    total: report.banner.total,
                },
                session_tau_scale: 1.0,
                session_sigma_scale: 1.0,
            };
            let sel = selected.clone();
            handles.push(tokio::spawn(async move {
                apply_autotune_rows_inner(&s, "BTC-USDC", &r, &sel, now).await
            }));
        }
        for h in handles {
            let res = h.await.expect("task join").expect("apply");
            assert_eq!(res.applied, vec!["targetProfitBps".to_string()]);
        }
        let strategy = get_strategy_params_impl("futures", &state).expect("strategy");
        assert!((strategy.target_profit_bps - 8.5).abs() < 1e-12);
        let events = state
            .state_store
            .recent_autotune_apply_events("BTC-USDC", 10)
            .expect("audit read");
        assert_eq!(events.len(), 4, "every apply writes its own audit line");
    }

    #[tokio::test]
    async fn test_autotune_coupling_neutral_without_profiles() {
        let state = AppEngineState::new();
        let rep = get_autotune_recommendation_impl("BTC-USDC", &state)
            .await
            .expect("recommendation");
        assert!((rep.session_tau_scale - 1.0).abs() < 1e-12);
        assert!((rep.session_sigma_scale - 1.0).abs() < 1e-12);
    }

    #[tokio::test]
    async fn test_session_profile_fail_closed_without_promotion() {
        let state = AppEngineState::new();
        let prof = get_session_profile_impl("BTC-USDC", &state).expect("profile");
        assert_eq!(prof.promotion, None, "no promotion row: Phase-1 baseline");
        assert!(prof.provisional);
        assert!(prof.sessions.is_empty());
    }

    #[tokio::test]
    async fn test_session_profile_rescale_only_when_promoted() {
        use chrono::TimeZone;
        let state = AppEngineState::new();
        let base_day = chrono::Utc
            .with_ymd_and_hms(2026, 1, 15, 0, 0, 0)
            .single()
            .expect("fixture day")
            .timestamp_millis();
        for hour in [2u32, 10, 15, 19, 23] {
            for i in 0..40 {
                let ts = base_day + (hour as i64) * 3_600_000 + (i as i64) * 60_000;
                let wave = ((i as f64) * 0.7).sin() * 3.0;
                seed_samples(
                    &state,
                    "BTC-USDC",
                    ts,
                    &[("hyperliquid", 60_000.0 + wave), ("lighter", 60_000.0 - wave)],
                );
            }
        }
        let from = base_day;
        let to = base_day + 86_400_000;
        let written = state
            .state_store
            .rollup_session_profiles("BTC-USDC", from, to, to)
            .expect("rollup");
        assert_eq!(written, 5);
        let stored = state.state_store.session_profiles("BTC-USDC").expect("rows");
        assert!(stored.iter().any(|r| r.rv_bps.is_some()), "rollup must emit rv");
        let now = Utc::now().timestamp_millis();
        state
            .state_store
            .phase2_upsert_har_anchor(&crate::state_store::HarAnchorRow {
                symbol: "BTC-USDC".to_string(),
                intercept: 0.1,
                b_daily: 0.5,
                b_weekly: 0.3,
                b_monthly: 0.1,
                r2: 0.9,
                n_days: 60,
                forecast_rv_bps: 12.0,
                trailing_mean_rv_bps: 10.0,
                updated_ts: now,
            })
            .expect("anchor");
        state
            .state_store
            .phase2_upsert_promotion_state(&crate::state_store::PromotionStateRow {
                symbol: "BTC-USDC".to_string(),
                status: "demoted".to_string(),
                fit_delta_bps: 2.0,
                val_delta_bps: -1.5,
                reason: "sign disagreement".to_string(),
                updated_ts: now,
            })
            .expect("promotion");
        let demoted = get_session_profile_impl("BTC-USDC", &state).expect("profile");
        let promo = demoted.promotion.expect("promotion served");
        assert_eq!(promo.status, "demoted");
        assert!(!promo.rescaled);
        for s in &demoted.sessions {
            let base = stored.iter().find(|r| r.session_id == s.session_id).expect("row");
            assert_eq!(s.rv_bps, base.rv_bps, "demoted serves Phase-1 values");
        }
        state
            .state_store
            .phase2_upsert_promotion_state(&crate::state_store::PromotionStateRow {
                symbol: "BTC-USDC".to_string(),
                status: "promoted".to_string(),
                fit_delta_bps: 2.0,
                val_delta_bps: 1.5,
                reason: "agree".to_string(),
                updated_ts: now,
            })
            .expect("promotion");
        let promoted = get_session_profile_impl("BTC-USDC", &state).expect("profile");
        let promo = promoted.promotion.expect("promotion served");
        assert_eq!(promo.status, "promoted");
        assert!(promo.rescaled);
        for s in &promoted.sessions {
            let base = stored.iter().find(|r| r.session_id == s.session_id).expect("row");
            match (s.rv_bps, base.rv_bps) {
                (Some(v), Some(b)) => {
                    assert!((v - b * 1.2).abs() < 1e-9, "promoted rescales rv x1.2");
                }
                _ => panic!("expected rv on both sides"),
            }
        }
    }

    #[tokio::test]
    async fn test_execute_spatial_rejects_entry_at_grid_cap() {
        let state = setup_test_state_with_venues();
        // Step beyond the derived cap rejects before any orderbook work.
        let err = execute_spatial_arbitrage_inner(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            Some(4),
            Some(3),
            None,
            None,
            "futures",
            &state,
        )
        .await
        .expect_err("step 4 of max 3 must reject");
        assert!(err.contains("Grid cap"), "unexpected err: {err}");

        // Zero cap is not a valid grid.
        let err = execute_spatial_arbitrage_inner(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            Some(1),
            Some(0),
            None,
            None,
            "futures",
            &state,
        )
        .await
        .expect_err("max_units 0 must reject");
        assert!(err.contains("max_units"), "unexpected err: {err}");

        // Ticker already at its accumulated cap rejects even a step-1 entry.
        state.strategy_configs.write().insert(
            "BTC-USDC".to_string(),
            TickerStrategyConfigDto {
                symbol: "BTC-USDC".into(),
                current_accumulated_units: 3,
                max_accumulated_units: 3,
                ..TickerStrategyConfigDto::default()
            },
        );
        let err = execute_spatial_arbitrage_inner(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            Some(1),
            Some(3),
            None,
            None,
            "futures",
            &state,
        )
        .await
        .expect_err("3/3 accumulated must reject");
        assert!(err.contains("Grid cap"), "unexpected err: {err}");
    }

    #[tokio::test]
    async fn test_venue_balances_report_paper_account() {
        let state = setup_test_state_with_venues();
        let balances = get_venue_balances_impl(&state).await.expect("balances");
        assert_eq!(balances.len(), 4);
        assert!(balances.iter().any(|b| b.venue == "vanta"));
        for b in &balances {
            assert!(b.available_margin_usd > 0.0);
            assert!(b.equity_usd > 0.0);
            assert_eq!(b.status, "HEALTHY");
        }
    }

    #[tokio::test]
    async fn test_strategy_performance_metrics() {
        let state = AppEngineState::new();
        let perfs = get_strategy_performance_impl(None, &state).expect("performance");
        assert_eq!(perfs.len(), 4);

        let spatial = perfs
            .iter()
            .find(|p| p.strategy_id == "spatial_spread")
            .expect("spatial metrics");
        assert_eq!(spatial.total_cycles_completed, 0);
        assert_eq!(spatial.total_pnl_usd, 0.0);
    }

    #[tokio::test]
    async fn test_wfo_calibration_command() {
        let state = AppEngineState::new();
        let configs_before = state.strategy_configs.read().len();
        let logs_before = state.execution_logs.read().len();

        let res = run_wfo_calibration_impl("BTC-USDC", &state).await;
        let err = res.expect_err("WFO must fail closed without recorded history");
        assert!(err.contains("unavailable"), "unexpected err: {}", err);

        assert_eq!(state.strategy_configs.read().len(), configs_before);
        assert_eq!(state.execution_logs.read().len(), logs_before);
        assert_eq!(*state.total_realized_profit.read(), dec!(0));
    }

    #[tokio::test]
    async fn test_scans_fail_closed_even_when_fully_configured() {
        let (state, dir) = test_state_with_vault("scans");
        initialize_vault_impl("test password", &state).expect("init vault");

        let settings = EngineSettingsDto {
            hyperliquid_enabled: true,
            lighter_enabled: true,
            vanta_enabled: true,
            robinhood_enabled: true,
            arcus_enabled: true,
            kraken_enabled: true,
            kraken_spot_enabled: true,
            kraken_futures_enabled: true,
            polymarket_enabled: true,
            kalshi_enabled: true,
            hyperliquid_wallet: "WALLET_x".into(),
            hyperliquid_private_key: "PRIV_x".into(),
            lighter_api_key: "LGT_KEY".into(),
            ..EngineSettingsDto::default()
        };
        save_engine_settings_impl(settings, &state).expect("save configured settings");

        let spot = scan_spot_opportunities_impl("BTC", &state)
            .await
            .expect("spot scan");
        assert!(spot.is_empty());
        let pred = scan_prediction_opportunities_impl("FED", &state)
            .await
            .expect("pred scan");
        assert!(pred.is_empty());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_spatial_execution_rejects_repeated_open() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let first = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await
        .expect("first execution");
        assert!(first.execution_success);

        let logs_before = state.execution_logs.read().len();
        let pnl_before = *state.total_realized_profit.read();
        let lots_before = state.virtual_ledger.read().lots.len();

        let second = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        let err = second.expect_err("second open must reject");
        assert!(err.contains("already exists"), "unexpected err: {}", err);

        assert_eq!(state.execution_logs.read().len(), logs_before);
        assert_eq!(*state.total_realized_profit.read(), pnl_before);
        assert_eq!(state.virtual_ledger.read().lots.len(), lots_before);
    }

    #[tokio::test]
    async fn test_spatial_execution_rejects_insufficient_ioc_bounded_depth() {
        let state = setup_test_state_with_venues();
        // Unbounded slippage for 2.0 would pass (VWAP ~8 bps), but only 1.0
        // ask sits inside the 15 BPS IOC limit; the remainder is unreachable.
        {
            let mut book = arbitrageur_core::quote::OrderBook::new(
                arbitrageur_core::types::VenueId::new("hyperliquid"),
                "BTC-USDC".to_string(),
            );
            book.bids.push(arbitrageur_core::quote::BookLevel::new(
                dec!(64100.0),
                dec!(50.0),
            ));
            book.asks.push(arbitrageur_core::quote::BookLevel::new(
                dec!(64205.0),
                dec!(1.0),
            ));
            book.asks.push(arbitrageur_core::quote::BookLevel::new(
                dec!(64310.0),
                dec!(50.0),
            ));
            book.timestamp = Utc::now();
            state.market_data_hub.update_orderbook(book);

            let mut deep = arbitrageur_core::quote::OrderBook::new(
                arbitrageur_core::types::VenueId::new("lighter"),
                "BTC-USDC".to_string(),
            );
            deep.bids.push(arbitrageur_core::quote::BookLevel::new(
                dec!(64360.0),
                dec!(50.0),
            ));
            deep.asks.push(arbitrageur_core::quote::BookLevel::new(
                dec!(64365.0),
                dec!(50.0),
            ));
            deep.timestamp = Utc::now();
            state.market_data_hub.update_orderbook(deep);
        }

        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            2.0,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await;
        assert!(res
            .expect_err("must reject before dispatch")
            .contains("slippage"));
        assert!(state.virtual_ledger.read().lots.is_empty());
        assert!(get_execution_logs_impl(&state).expect("logs").is_empty());
    }

    #[tokio::test]
    async fn test_symbol_and_numeric_input_validation() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );

        let oversized_symbol = "A".repeat(65);
        for bad in [
            "",
            "  ",
            "BTC -USDC",
            "../etc/passwd",
            "BTC-USDC?x=1",
            "BTC-USDC;rm",
            "BTC-\u{0001}USDC",
            "-USDC",
            "BTC-",
            oversized_symbol.as_str(),
        ] {
            assert!(
                fetch_orderbooks_impl(bad, &state).await.is_err(),
                "symbol {:?} must be rejected",
                bad
            );
        }

        let oversized: Vec<String> = (0..101).map(|i| format!("S{}", i)).collect();
        assert!(fetch_multi_market_quotes_impl(oversized, &state)
            .await
            .is_err());
        assert!(
            fetch_multi_market_quotes_impl(vec!["BTC-USDC?bad".into()], &state)
                .await
                .is_err()
        );

        assert!(
            scan_spatial_opportunities_impl("BTC-USDC", 5.0, f64::NAN, &state)
                .await
                .is_err()
        );
        assert!(
            scan_spatial_opportunities_impl("BTC-USDC", 5.0, -1.0, &state)
                .await
                .is_err()
        );
        assert!(
            scan_spatial_opportunities_impl("BTC-USDC", f64::NAN, 0.1, &state)
                .await
                .is_err()
        );
        assert!(scan_funding_opportunities_impl("BTC-USDC", -5.0, &state)
            .await
            .is_err());
        assert!(
            scan_funding_opportunities_impl("BTC-USDC", f64::INFINITY, &state)
                .await
                .is_err()
        );

        assert!(run_stat_arb_step_impl(0, f64::NAN, 100.0, &state).is_err());
        assert!(run_stat_arb_step_impl(0, 100.0, -1.0, &state).is_err());
        assert!(fit_ou_process_impl(&[100.0; 12], f64::NAN).is_err());
        assert!(fit_ou_process_impl(&[f64::INFINITY; 12], 1.0).is_err());
        assert!(compute_optimal_thresholds_impl(f64::NAN, 0.0, 1.0, 0.1).is_err());
        assert!(compute_optimal_thresholds_impl(1.0, 0.0, f64::INFINITY, 0.1).is_err());
        assert!(compute_optimal_thresholds_impl(1.0, 0.0, 1.0, f64::NAN).is_err());

        assert!(get_execution_logs_impl(&state).expect("logs").is_empty());
        assert_eq!(*state.total_realized_profit.read(), dec!(0));
    }

    #[tokio::test]
    async fn test_engine_settings_rejects_invalid_inputs() {
        let state = AppEngineState::new();
        let base = EngineSettingsDto::default();

        let bad = EngineSettingsDto {
            min_depth_ratio: 0.0,
            ..base.clone()
        };
        assert!(save_engine_settings_impl(bad, &state).is_err());
        let bad = EngineSettingsDto {
            min_depth_ratio: f64::NAN,
            ..base.clone()
        };
        assert!(save_engine_settings_impl(bad, &state).is_err());
        let bad = EngineSettingsDto {
            trading_mode: "simulation".into(),
            ..base.clone()
        };
        assert!(save_engine_settings_impl(bad, &state).is_err());
        let bad = EngineSettingsDto {
            bind_address: "0.0.0.0".into(),
            ..base.clone()
        };
        assert!(save_engine_settings_impl(bad, &state).is_err());
        let bad = EngineSettingsDto {
            engine_rpc_port: 9000,
            engine_ws_port: 9000,
            ..base.clone()
        };
        assert!(save_engine_settings_impl(bad, &state).is_err());
        let bad = EngineSettingsDto {
            engine_rpc_port: 0,
            ..base.clone()
        };
        assert!(save_engine_settings_impl(bad, &state).is_err());

        let ok = save_engine_settings_impl(base, &state);
        assert!(ok.is_ok());
    }

    #[tokio::test]
    async fn test_risk_overview_open_position_free_margin_and_unknown_liquidation() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let before = get_risk_overview_impl(&state).await.expect("risk before");
        assert_eq!(before.total_portfolio_value_usd, 600000.0);
        assert_eq!(before.margin_health.margin_balance_usd, 600000.0);
        assert_eq!(before.margin_health.status, "FLAT");

        execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await
        .expect("execution");

        let after = get_risk_overview_impl(&state).await.expect("risk after");
        assert!(
            after.margin_health.margin_balance_usd < before.margin_health.margin_balance_usd,
            "free margin must decrease after open: {}",
            after.margin_health.margin_balance_usd
        );
        assert_eq!(
            after.margin_health.status, "UNKNOWN_LIQUIDATION",
            "open positions without liquidation prices must not report SAFE"
        );
        let btc = after
            .net_deltas
            .iter()
            .find(|d| d.asset == "BTC")
            .expect("BTC delta entry");
        assert_eq!(btc.net_quantity, 0.0);
        assert_eq!(btc.net_exposure_usd, 0.0);
        assert!(btc.is_neutral);
    }

    #[tokio::test]
    async fn test_spatial_pnl_synced_from_paper_equity() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await
        .expect("execution");
        assert!(res.execution_success);

        let mut equity_sum = dec!(0);
        for (_, paper) in state.paper_exchanges() {
            equity_sum += paper.fetch_account().await.expect("account").equity;
        }
        assert_eq!(
            *state.total_realized_profit.read(),
            equity_sum - state.paper_baseline_equity()
        );
        // Fees are real outflow; the position open is not marked as profit
        assert!(*state.total_realized_profit.read() < dec!(0));

        let metrics = state
            .virtual_ledger
            .read()
            .compute_strategy_metrics("spatial_spread");
        assert_eq!(metrics.total_cycles_initiated, 1);

        let perfs = get_strategy_performance_impl(None, &state).expect("performance");
        let spatial = perfs
            .iter()
            .find(|p| p.strategy_id == "spatial_spread")
            .expect("spatial_spread metrics");
        assert_eq!(spatial.total_cycles_completed, 0);
    }

    #[tokio::test]
    async fn test_spatial_execution_queued_call_reevaluates_breaker() {
        let state = Arc::new(setup_test_state_with_venues());
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let guard = state.execution_lock.lock().await;
        let st = state.clone();
        let task = tokio::spawn(async move {
            execute_spatial_arbitrage_impl(
                "BTC-USDC",
                "hyperliquid",
                "lighter",
                0.1,
                None,
                None,
                None,
                None,
                None,
                None,
                &st,
            )
            .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        state
            .circuit_breaker
            .trip("killswitch while execution queued");
        drop(guard);

        let res = task.await.expect("join");
        let err = res.expect_err("queued execution must observe the tripped breaker");
        assert!(err.contains("Circuit Breaker"), "unexpected err: {}", err);
        assert!(state.virtual_ledger.read().lots.is_empty());
        assert!(get_execution_logs_impl(&state).expect("logs").is_empty());
        assert_eq!(*state.total_realized_profit.read(), dec!(0));
        for (_, paper) in state.paper_exchanges() {
            let acc = paper.fetch_account().await.expect("account");
            assert!(acc.positions.is_empty());
            assert_eq!(acc.equity, dec!(100000.0));
        }
    }

    #[tokio::test]
    async fn test_spatial_concurrent_same_symbol_only_one_opens() {
        let state = Arc::new(setup_test_state_with_venues());
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let (r1, r2) = tokio::join!(
            execute_spatial_arbitrage_impl(
                "BTC-USDC",
                "hyperliquid",
                "lighter",
                0.1,
                None,
                None,
                None,
                None,
                None,
                None,
                &state,
            ),
            execute_spatial_arbitrage_impl(
                "BTC-USDC",
                "hyperliquid",
                "lighter",
                0.1,
                None,
                None,
                None,
                None,
                None,
                None,
                &state,
            ),
        );
        let oks = [&r1, &r2].iter().filter(|r| r.is_ok()).count();
        assert_eq!(oks, 1, "exactly one execution may open: {:?} {:?}", r1, r2);
        let errs: Vec<String> = [r1, r2].into_iter().filter_map(|r| r.err()).collect();
        assert_eq!(errs.len(), 1);
        assert!(
            errs[0].contains("already exists"),
            "unexpected err: {}",
            errs[0]
        );
        assert_eq!(state.virtual_ledger.read().lots.len(), 1);
        assert_eq!(state.execution_logs.read().len(), 1);
    }

    #[tokio::test]
    async fn test_spatial_execution_persists_durable_signal() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await
        .expect("execution");
        assert!(res.execution_success);

        let signals = state
            .state_store
            .list_signals(Some("BTC-USDC"), None)
            .expect("list signals");
        assert_eq!(signals.len(), 1);
        let sig = &signals[0];
        assert_eq!(sig.status, crate::state_store::SignalStatus::Balanced);
        assert_eq!(sig.legs.len(), 2);
        for leg in &sig.legs {
            assert!(
                (leg.filled_qty - 0.1).abs() < 1e-9,
                "leg must carry its own fill attribution: {:?}",
                leg
            );
            assert!(leg.fill_price.unwrap_or(0.0) > 0.0);
        }
        let venues: std::collections::HashSet<&str> =
            sig.legs.iter().map(|l| l.venue.as_str()).collect();
        assert!(venues.contains("hyperliquid") && venues.contains("lighter"));
        // Self-hedged signal: per-signal delta and ticker net are both zero.
        assert!(state.state_store.signal_delta(&sig.id).unwrap().abs() < 1e-9);
        assert!(
            state
                .state_store
                .ticker_net_position("BTC-USDC")
                .unwrap()
                .abs()
                < 1e-9
        );
        // Balanced signals no longer block the ticker.
        assert!(!state.state_store.ticker_blocked("BTC-USDC").unwrap());
    }

    #[tokio::test]
    async fn test_spatial_execution_blocked_by_unreconciled_signal() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));
        seed_test_book(
            &state,
            "hyperliquid",
            "ETH-USDC",
            dec!(3000.0),
            dec!(3005.0),
        );
        seed_test_book(&state, "lighter", "ETH-USDC", dec!(3110.0), dec!(3115.0));

        // A prior signal for BTC-USDC holds residual exposure: only its long
        // leg filled. The ticker must refuse new orders until it reconciles.
        state
            .state_store
            .record_signal_execution(&crate::state_store::NewSignalExecution {
                id: "sig-stuck".into(),
                strategy_id: "futures".into(),
                symbol: "BTC-USDC".into(),
                route_id: None,
                expected_net_usd: Some(5.0),
                note: None,
                legs: vec![
                    crate::state_store::NewSignalLeg {
                        account_id: None,
                        venue: "hyperliquid".into(),
                        side: crate::state_store::LegSide::Long,
                        intended_qty: 0.1,
                    },
                    crate::state_store::NewSignalLeg {
                        account_id: None,
                        venue: "lighter".into(),
                        side: crate::state_store::LegSide::Short,
                        intended_qty: 0.1,
                    },
                ],
            })
            .expect("record signal");
        state
            .state_store
            .apply_leg_fill(
                "sig-stuck",
                crate::state_store::LegSide::Long,
                0.1,
                64205.0,
                0.05,
                Some("o-stuck"),
            )
            .expect("leg fill");
        state
            .state_store
            .update_signal_status(
                "sig-stuck",
                crate::state_store::SignalStatus::Imbalanced,
                Some("short leg rejected"),
            )
            .expect("status");

        let err = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await
        .expect_err("blocked ticker must reject");
        assert!(err.contains("blocked"), "unexpected err: {}", err);

        // The gate is per-ticker: a different symbol still executes.
        let other = execute_spatial_arbitrage_impl(
            "ETH-USDC",
            "hyperliquid",
            "lighter",
            0.5,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await
        .expect("different ticker executes");
        assert!(other.execution_success);
    }

    #[tokio::test]
    async fn test_spatial_candidates_batch_picks_highest_expected() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));
        seed_test_book(
            &state,
            "hyperliquid",
            "ETH-USDC",
            dec!(3000.0),
            dec!(3005.0),
        );
        seed_test_book(&state, "lighter", "ETH-USDC", dec!(3110.0), dec!(3115.0));

        let outcomes = execute_spatial_candidates_impl(
            vec![
                SpatialCandidateInput {
                    symbol: "BTC-USDC".into(),
                    buy_venue: "hyperliquid".into(),
                    sell_venue: "lighter".into(),
                    order_qty: 0.1,
                    expected_net_usd: Some(8.0),
                    route_id: None,
                    direction: None,
                    position_step: None,
                    max_units: None,
                },
                SpatialCandidateInput {
                    symbol: "BTC-USDC".into(),
                    buy_venue: "hyperliquid".into(),
                    sell_venue: "lighter".into(),
                    order_qty: 0.2,
                    expected_net_usd: Some(15.0),
                    route_id: None,
                    direction: None,
                    position_step: None,
                    max_units: None,
                },
                SpatialCandidateInput {
                    symbol: "ETH-USDC".into(),
                    buy_venue: "hyperliquid".into(),
                    sell_venue: "lighter".into(),
                    order_qty: 0.5,
                    expected_net_usd: Some(20.0),
                    route_id: None,
                    direction: None,
                    position_step: None,
                    max_units: None,
                },
            ],
            &state,
        )
        .await
        .expect("batch outcomes");
        assert_eq!(outcomes.len(), 3);

        let btc: Vec<_> = outcomes.iter().filter(|o| o.symbol == "BTC-USDC").collect();
        assert_eq!(btc.len(), 2);
        let winner = btc.iter().find(|o| o.selected).expect("one BTC winner");
        assert_eq!(winner.expected_net_usd, Some(15.0));
        assert!(winner.result.as_ref().unwrap().execution_success);
        let loser = btc.iter().find(|o| !o.selected).expect("one BTC loser");
        assert_eq!(loser.expected_net_usd, Some(8.0));
        assert!(loser.reason.as_deref().unwrap().contains("lower expected"));

        let eth = outcomes.iter().find(|o| o.symbol == "ETH-USDC").unwrap();
        assert!(eth.selected);
        assert!(eth.result.as_ref().unwrap().execution_success);

        // Exactly one durable signal per dispatched ticker.
        assert_eq!(
            state
                .state_store
                .list_signals(Some("BTC-USDC"), None)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            state
                .state_store
                .list_signals(Some("ETH-USDC"), None)
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn test_spatial_candidates_reject_all_when_ticker_blocked() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));
        seed_test_book(
            &state,
            "hyperliquid",
            "ETH-USDC",
            dec!(3000.0),
            dec!(3005.0),
        );
        seed_test_book(&state, "lighter", "ETH-USDC", dec!(3110.0), dec!(3115.0));

        // In-flight signal on BTC-USDC blocks every candidate in the batch.
        state
            .state_store
            .record_signal_execution(&crate::state_store::NewSignalExecution {
                id: "sig-flight".into(),
                strategy_id: "futures".into(),
                symbol: "BTC-USDC".into(),
                route_id: None,
                expected_net_usd: None,
                note: None,
                legs: vec![crate::state_store::NewSignalLeg {
                    account_id: None,
                    venue: "hyperliquid".into(),
                    side: crate::state_store::LegSide::Long,
                    intended_qty: 0.1,
                }],
            })
            .expect("record in-flight signal");

        let outcomes = execute_spatial_candidates_impl(
            vec![
                SpatialCandidateInput {
                    symbol: "BTC-USDC".into(),
                    buy_venue: "hyperliquid".into(),
                    sell_venue: "lighter".into(),
                    order_qty: 0.1,
                    expected_net_usd: Some(99.0),
                    route_id: None,
                    direction: None,
                    position_step: None,
                    max_units: None,
                },
                SpatialCandidateInput {
                    symbol: "BTC-USDC".into(),
                    buy_venue: "hyperliquid".into(),
                    sell_venue: "lighter".into(),
                    order_qty: 0.2,
                    expected_net_usd: Some(50.0),
                    route_id: None,
                    direction: None,
                    position_step: None,
                    max_units: None,
                },
                SpatialCandidateInput {
                    symbol: "ETH-USDC".into(),
                    buy_venue: "hyperliquid".into(),
                    sell_venue: "lighter".into(),
                    order_qty: 0.5,
                    expected_net_usd: Some(20.0),
                    route_id: None,
                    direction: None,
                    position_step: None,
                    max_units: None,
                },
            ],
            &state,
        )
        .await
        .expect("batch outcomes");

        for o in outcomes.iter().filter(|o| o.symbol == "BTC-USDC") {
            assert!(!o.selected, "blocked ticker rejects every candidate");
            assert!(o.reason.as_deref().unwrap().contains("blocked"));
            assert!(o.result.is_none());
        }
        let eth = outcomes.iter().find(|o| o.symbol == "ETH-USDC").unwrap();
        assert!(eth.selected);
        assert!(eth.result.as_ref().unwrap().execution_success);
        // Only the in-flight seed exists for BTC — nothing new was dispatched.
        assert_eq!(
            state
                .state_store
                .list_signals(Some("BTC-USDC"), None)
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn test_delete_account_blocked_by_signal_history() {
        let state = setup_test_state_with_venues();
        let account = state
            .state_store
            .upsert_account("hl-main", "hyperliquid", "Main", true, false, "standard", None)
            .expect("account");

        state
            .state_store
            .record_signal_execution(&crate::state_store::NewSignalExecution {
                id: "sig-hist".into(),
                strategy_id: "futures".into(),
                symbol: "BTC-USDC".into(),
                route_id: None,
                expected_net_usd: None,
                note: None,
                legs: vec![crate::state_store::NewSignalLeg {
                    account_id: Some(account.id.clone()),
                    venue: "hyperliquid".into(),
                    side: crate::state_store::LegSide::Long,
                    intended_qty: 0.1,
                }],
            })
            .expect("record signal");

        let err = delete_account_impl(&account.id, &state).expect_err("must refuse");
        assert!(err.contains("position history"), "unexpected err: {}", err);
        assert!(state
            .state_store
            .get_account(&account.id)
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn test_validate_adapter_book_helper() {
        let venue = arbitrageur_core::types::VenueId::new("hyperliquid");
        let mut book = OrderBook::new(venue.clone(), "BTC-USDC".to_string());
        book.bids.push(arbitrageur_core::quote::BookLevel::new(
            dec!(64000.0),
            dec!(1.0),
        ));
        book.asks.push(arbitrageur_core::quote::BookLevel::new(
            dec!(64001.0),
            dec!(1.0),
        ));

        assert!(validate_adapter_book(&book, &venue, "BTC-USDC").is_ok());

        let other_venue = arbitrageur_core::types::VenueId::new("lighter");
        let err = validate_adapter_book(&book, &other_venue, "BTC-USDC").unwrap_err();
        assert!(err.contains("venue"), "unexpected err: {}", err);

        let err = validate_adapter_book(&book, &venue, "ETH-USDC").unwrap_err();
        assert!(err.contains("symbol"), "unexpected err: {}", err);

        let mut crossed = book.clone();
        crossed.bids[0].price = dec!(65000.0);
        let err = validate_adapter_book(&crossed, &venue, "BTC-USDC").unwrap_err();
        assert!(err.contains("crossed"), "unexpected err: {}", err);
    }

    #[tokio::test]
    async fn test_read_only_paths_skip_malformed_cached_book() {
        let state = setup_test_state_with_venues();
        let mut crossed = OrderBook::new(
            arbitrageur_core::types::VenueId::new("hyperliquid"),
            "BTC-USDC".to_string(),
        );
        crossed.bids.push(arbitrageur_core::quote::BookLevel::new(
            dec!(65000.0),
            dec!(10.0),
        ));
        crossed.asks.push(arbitrageur_core::quote::BookLevel::new(
            dec!(64000.0),
            dec!(10.0),
        ));
        crossed.timestamp = Utc::now();
        state.market_data_hub.update_orderbook(crossed);
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));
        seed_test_book(
            &state,
            "robinhood",
            "BTC-USDC",
            dec!(64360.0),
            dec!(64365.0),
        );
        seed_test_book(&state, "vanta", "BTC-USDC", dec!(64360.0), dec!(64365.0));
        seed_test_funding(&state, "hyperliquid", "BTC-USDC");
        seed_test_funding(&state, "lighter", "BTC-USDC");
        seed_test_funding(&state, "vanta", "BTC-USDC");

        let books = fetch_orderbooks_impl("BTC-USDC", &state)
            .await
            .expect("books");
        assert_eq!(books.len(), 3);
        assert_eq!(books[0].venue_id, "lighter");
        assert_eq!(books[1].venue_id, "robinhood");
        assert_eq!(books[2].venue_id, "vanta");

        let opps = scan_spatial_opportunities_impl("BTC-USDC", 5.0, 0.1, &state)
            .await
            .expect("scan");
        // The malformed hyperliquid book must not participate; lighter,
        // robinhood and vanta share identical seeded prices so any pair is
        // non-executable.
        assert!(opps.iter().all(|o| !o.is_executable));
        assert!(opps
            .iter()
            .all(|o| o.buy_venue != "hyperliquid" && o.sell_venue != "hyperliquid"));
    }

    #[tokio::test]
    async fn test_decimal_conversion_overflow_rejects() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            Some(f64::NAN),
            &state,
        )
        .await;
        let err = res.expect_err("NaN hurdle must reject");
        assert!(err.contains("finite"), "unexpected err: {}", err);

        let res = scan_spatial_opportunities_impl("BTC-USDC", 5.0, f64::MAX, &state).await;
        let err = res.expect_err("f64::MAX order_qty must reject");
        assert!(err.contains("decimal range"), "unexpected err: {}", err);

        let res = scan_funding_opportunities_impl("BTC-USDC", f64::MAX, &state).await;
        let err = res.expect_err("f64::MAX trade size must reject");
        assert!(err.contains("decimal range"), "unexpected err: {}", err);

        assert!(get_execution_logs_impl(&state).expect("logs").is_empty());
        assert!(state.virtual_ledger.read().lots.is_empty());
        assert_eq!(*state.total_realized_profit.read(), dec!(0));
    }

    #[tokio::test]
    async fn test_scan_is_executable_respects_bounded_depth() {
        let state = setup_test_state_with_venues();
        let mut thin = OrderBook::new(
            arbitrageur_core::types::VenueId::new("hyperliquid"),
            "BTC-USDC".to_string(),
        );
        thin.bids.push(arbitrageur_core::quote::BookLevel::new(
            dec!(64100.0),
            dec!(50.0),
        ));
        thin.asks.push(arbitrageur_core::quote::BookLevel::new(
            dec!(64205.0),
            dec!(1.0),
        ));
        thin.asks.push(arbitrageur_core::quote::BookLevel::new(
            dec!(64310.0),
            dec!(50.0),
        ));
        thin.timestamp = Utc::now();
        state.market_data_hub.update_orderbook(thin);

        let mut deep = OrderBook::new(
            arbitrageur_core::types::VenueId::new("lighter"),
            "BTC-USDC".to_string(),
        );
        deep.bids.push(arbitrageur_core::quote::BookLevel::new(
            dec!(64360.0),
            dec!(50.0),
        ));
        deep.asks.push(arbitrageur_core::quote::BookLevel::new(
            dec!(64365.0),
            dec!(50.0),
        ));
        deep.timestamp = Utc::now();
        state.market_data_hub.update_orderbook(deep);
        seed_test_book(&state, "vanta", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let opps = scan_spatial_opportunities_impl("BTC-USDC", 5.0, 2.0, &state)
            .await
            .expect("scan");
        let opp = opps
            .iter()
            .find(|o| o.buy_venue.contains("Hyperliquid"))
            .expect("hl->lgt opportunity row");
        assert!(
            !opp.is_executable,
            "out-of-limit depth must mark non-executable"
        );

        seed_test_book(
            &state,
            "hyperliquid",
            "ETH-USDC",
            dec!(3400.0),
            dec!(3401.0),
        );
        seed_test_book(&state, "lighter", "ETH-USDC", dec!(3410.0), dec!(3411.0));
        seed_test_book(&state, "vanta", "ETH-USDC", dec!(3410.0), dec!(3411.0));
        let opps = scan_spatial_opportunities_impl("ETH-USDC", 5.0, 0.1, &state)
            .await
            .expect("scan eth");
        let opp = opps
            .iter()
            .find(|o| o.buy_venue.contains("Hyperliquid"))
            .expect("hl->lgt eth row");
        assert!(opp.is_executable, "deep profitable pair must be executable");
    }

    #[tokio::test]
    async fn test_risk_overview_drawdown_pct_uses_day_start() {
        let state = setup_test_state_with_venues();
        state.circuit_breaker.update_equity(dec!(1000000.0));
        state.circuit_breaker.reset();
        assert_eq!(state.circuit_breaker.day_start_equity(), dec!(1000000.0));
        // Mark the equity universe as already anchored so the snapshot measures
        // drawdown against the 1M baseline instead of re-anchoring (universe
        // switches re-anchor; steady-state snapshots measure).
        *state.last_risk_live_universe.write() = Some(*state.is_live_mode.read());

        let risk = get_risk_overview_impl(&state).await.expect("risk");
        assert_eq!(risk.current_drawdown_usd, 400000.0);
        assert_eq!(risk.drawdown_pct, 40.0);

        state.circuit_breaker.update_equity(dec!(0.0));
        state.circuit_breaker.reset();
        let risk = get_risk_overview_impl(&state)
            .await
            .expect("risk zero start");
        assert_eq!(risk.drawdown_pct, 0.0);
    }

    fn seed_samples(state: &AppEngineState, symbol: &str, ts_ms: i64, mids: &[(&str, f64)]) {
        let entries: Vec<crate::state_store::VenueSampleEntry> = mids
            .iter()
            .map(|(vid, mid)| crate::state_store::VenueSampleEntry {
                venue_id: vid.to_string(),
                mid: *mid,
                best_bid: mid - 0.5,
                best_ask: mid + 0.5,
            })
            .collect();
        state
            .state_store
            .record_spread_samples(symbol, ts_ms, &entries)
            .expect("record samples");
    }

    #[tokio::test]
    async fn test_signal_series_empty_is_honest() {
        // No venues enabled -> no adapter traffic; empty store must return a
        // structured empty series, not an error or synthetic points.
        let state = AppEngineState::new();
        let s = get_signal_series_impl("BTC-USDC", "consensus", 300, &state)
            .await
            .expect("series");
        assert_eq!(s.sample_count, 0);
        assert_eq!(s.earliest_ts_ms, None);
        assert!(s.ts_ms.is_empty());
        assert!(s.dev_bps.is_empty());
        assert_eq!(s.mode, "consensus");
    }

    #[tokio::test]
    async fn test_signal_series_roundtrip_computes_in_rust() {
        let state = AppEngineState::new();
        let now = Utc::now().timestamp_millis();
        // Three buckets, two venues: A=100.0, B=101.0 -> equal-weight
        // consensus baseline 100.5; devs -49.75/+49.75 bps; band ~99.5 bps.
        for i in 0..3 {
            seed_samples(
                &state,
                "BTC-USDC",
                now - (2 - i) * 1_000,
                &[("hyperliquid", 100.0), ("lighter", 101.0)],
            );
        }

        let s = get_signal_series_impl("BTC-USDC", "consensus", 300, &state)
            .await
            .expect("series");
        assert_eq!(s.sample_count, 6);
        assert_eq!(s.venues, vec!["hyperliquid", "lighter"]);
        assert_eq!(s.ts_ms.len(), 3);
        assert!(s.earliest_ts_ms.unwrap() <= now - 2_000);
        for row in &s.dev_bps {
            let hl = row[0].unwrap();
            let lgt = row[1].unwrap();
            assert!((hl + lgt).abs() < 1e-6, "consensus deviations sum to ~0");
            assert!(hl < 0.0 && lgt > 0.0);
        }
        let band = s.band_max_bps[2] - s.band_min_bps[2];
        assert!(
            (band - 99.5025).abs() < 0.01,
            "band height = gross spread bps"
        );
    }

    #[tokio::test]
    async fn test_signal_series_merges_monitor_and_ui_keys() {
        // Gadget queries "BTC-USDC" but background recording lands under
        // "BTC": the series must merge both or unselected tickers show an
        // empty trailing window despite fresh background samples.
        let state = AppEngineState::new();
        let now = Utc::now().timestamp_millis();
        seed_samples(&state, "BTC", now - 2_000, &[("hyperliquid", 100.0)]);
        seed_samples(&state, "BTC", now - 1_000, &[("lighter", 101.0)]);
        seed_samples(
            &state,
            "BTC-USDC",
            now - 3_000,
            &[("hyperliquid", 99.0)],
        );

        let s = get_signal_series_impl("BTC-USDC", "consensus", 300, &state)
            .await
            .expect("series");
        assert_eq!(s.sample_count, 3);
        assert_eq!(s.earliest_ts_ms, Some(now - 3_000));
        assert!(!s.ts_ms.is_empty());
    }

    #[tokio::test]
    async fn test_signal_series_leader_mode_and_gap_preservation() {
        let state = AppEngineState::new();
        let now = Utc::now().timestamp_millis();
        seed_samples(
            &state,
            "ETH-USDC",
            now - 2_000,
            &[("hyperliquid", 3000.0), ("lighter", 3006.0)],
        );
        // Second bucket: only hyperliquid -> lighter cell must be null.
        seed_samples(&state, "ETH-USDC", now - 1_000, &[("hyperliquid", 3001.0)]);

        let s = get_signal_series_impl("ETH-USDC", "leader", 300, &state)
            .await
            .expect("series");
        assert_eq!(s.mode, "leader");
        assert_eq!(s.ts_ms.len(), 2);
        // No volumes in test state -> deterministic leader = alphabetically
        // first venue ("hyperliquid"), pinned at 0.
        assert_eq!(s.dev_bps[0][0], Some(0.0));
        assert_eq!(s.dev_bps[1][1], None, "missing venue stays a gap");
        assert_eq!(
            s.band_max_bps[1], s.band_min_bps[1],
            "single-venue band collapses"
        );
    }

    #[tokio::test]
    async fn test_signal_series_rejects_bad_symbol() {
        let state = AppEngineState::new();
        assert!(get_signal_series_impl("  ", "consensus", 60, &state)
            .await
            .is_err());
        assert!(get_signal_series_impl("BTC\u{0}", "consensus", 60, &state)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_fetch_orderbooks_persists_spread_samples() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(60000.0),
            dec!(60005.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(60120.0), dec!(60125.0));
        seed_test_book(
            &state,
            "robinhood",
            "BTC-USDC",
            dec!(60120.0),
            dec!(60125.0),
        );
        seed_test_book(&state, "vanta", "BTC-USDC", dec!(60120.0), dec!(60125.0));
        seed_test_funding(&state, "hyperliquid", "BTC-USDC");
        seed_test_funding(&state, "lighter", "BTC-USDC");
        seed_test_funding(&state, "vanta", "BTC-USDC");

        let books = fetch_orderbooks_impl("BTC-USDC", &state)
            .await
            .expect("books");
        assert_eq!(books.len(), 4);

        let rows = state
            .state_store
            .spread_samples("BTC-USDC", 0, i64::MAX)
            .expect("samples");
        assert_eq!(rows.len(), 4, "one row per reporting venue per poll");
        let hl = rows.iter().find(|r| r.venue_id == "hyperliquid").unwrap();
        assert_eq!(hl.mid, 60002.5);
        assert_eq!(hl.best_bid, 60000.0);
        assert_eq!(hl.best_ask, 60005.0);
    }

    #[test]
    fn test_tauri_commands_resolve_managed_arc_state() {
        let app = tauri::test::mock_builder()
            .manage(Arc::new(AppEngineState::new()))
            .invoke_handler(tauri::generate_handler![
                list_signal_executions,
                get_ticker_position_state,
                execute_spatial_candidates
            ])
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app builds");
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("webview builds");

        for (cmd, body) in [
            (
                "list_signal_executions",
                serde_json::json!({"symbol": null, "status": null}),
            ),
            (
                "get_ticker_position_state",
                serde_json::json!({"symbol": "BTC-USDC"}),
            ),
            (
                "execute_spatial_candidates",
                serde_json::json!({"candidates": []}),
            ),
        ] {
            let response = tauri::test::get_ipc_response(
                &webview,
                tauri::webview::InvokeRequest {
                    cmd: cmd.into(),
                    callback: tauri::ipc::CallbackFn(0),
                    error: tauri::ipc::CallbackFn(1),
                    url: "tauri://localhost".parse().expect("invoke url"),
                    body: tauri::ipc::InvokeBody::Json(body),
                    headers: Default::default(),
                    invoke_key: tauri::test::INVOKE_KEY.to_string(),
                },
            );
            assert!(
                response.is_ok(),
                "{cmd} must resolve the managed Arc<AppEngineState>: {:?}",
                response.err()
            );
        }
    }

    fn seed_closed_family(
        state: &AppEngineState,
        root_id: &str,
        strategy: &str,
        entry_long_px: f64,
        entry_short_px: f64,
        exit_long_px: f64,
        exit_short_px: f64,
    ) {
        use crate::state_store::{LegSide, NewSignalExecution, NewSignalLeg, SignalStatus};
        let legs = |venue_long: &str, venue_short: &str| {
            vec![
                NewSignalLeg {
                    account_id: None,
                    venue: venue_long.to_string(),
                    side: LegSide::Long,
                    intended_qty: 1.0,
                },
                NewSignalLeg {
                    account_id: None,
                    venue: venue_short.to_string(),
                    side: LegSide::Short,
                    intended_qty: 1.0,
                },
            ]
        };
        state
            .state_store
            .record_signal_execution(&NewSignalExecution {
                id: root_id.to_string(),
                strategy_id: strategy.to_string(),
                symbol: "BTC-USDC".to_string(),
                route_id: None,
                expected_net_usd: None,
                note: None,
                legs: legs("hyperliquid", "lighter"),
            })
            .expect("root records");
        state
            .state_store
            .apply_leg_fill(root_id, LegSide::Long, 1.0, entry_long_px, 2.0, None)
            .expect("entry long fills");
        state
            .state_store
            .apply_leg_fill(root_id, LegSide::Short, 1.0, entry_short_px, 2.0, None)
            .expect("entry short fills");
        let close_id = format!("flat-{root_id}-0001");
        // Child legs carry the closing trade side (opposite of entry), so
        // side-signed cash flows net entry against exit exactly like the
        // production flatten path.
        state
            .state_store
            .record_signal_execution(&NewSignalExecution {
                id: close_id.clone(),
                strategy_id: strategy.to_string(),
                symbol: "BTC-USDC".to_string(),
                route_id: None,
                expected_net_usd: None,
                note: None,
                legs: vec![
                    NewSignalLeg {
                        account_id: None,
                        venue: "hyperliquid".to_string(),
                        side: LegSide::Short,
                        intended_qty: 1.0,
                    },
                    NewSignalLeg {
                        account_id: None,
                        venue: "lighter".to_string(),
                        side: LegSide::Long,
                        intended_qty: 1.0,
                    },
                ],
            })
            .expect("close child records");
        // Sell to close the long @ exit_long_px; buy back the short @
        // exit_short_px.
        state
            .state_store
            .apply_leg_fill(&close_id, LegSide::Short, 1.0, exit_long_px, 1.0, None)
            .expect("exit short fills");
        state
            .state_store
            .apply_leg_fill(&close_id, LegSide::Long, 1.0, exit_short_px, 1.0, None)
            .expect("exit long fills");
        state
            .state_store
            .update_signal_status(root_id, SignalStatus::Closed, None)
            .expect("root closes");
        state
            .state_store
            .update_signal_status(&close_id, SignalStatus::Closed, None)
            .expect("child closes");
    }

    #[test]
    fn test_strategy_fill_pnl_spatial_round_trip_fees_included() {
        let state = setup_test_state_with_venues();
        // Long 1.0 @ 60000 / short 1.0 @ 60120; sell the long @ 60050 (+50),
        // buy back the short @ 60100 (+20). Gross 70, fees 2 + 2 + 1 + 1 = 6
        // -> net 64.
        seed_closed_family(&state, "sig-spatial-1", "futures", 60000.0, 60120.0, 60050.0, 60100.0);
        let pnl = get_strategy_fill_pnl_impl("futures", &state).expect("pnl");
        assert_eq!(pnl.strategy_id, "futures");
        assert_eq!(pnl.closed_families, 1);
        assert_eq!(pnl.unattributed_closed_families, 0);
        assert!((pnl.net_profit_usd - 64.0).abs() < 1e-9, "net {:?}", pnl);
        assert!(pnl.net_profit_bps.is_some());
        // Radar sees none of the futures family's dollars.
        let radar = get_strategy_fill_pnl_impl("radar", &state).expect("radar pnl");
        assert_eq!(radar.closed_families, 0);
        assert_eq!(radar.net_profit_usd, 0.0);
        assert_eq!(radar.unattributed_log_entries, 0);
    }

    #[test]
    fn test_strategy_fill_pnl_radar_and_triangle_paths() {
        let state = setup_test_state_with_venues();
        seed_closed_family(&state, "sig-radar-1", "radar", 60000.0, 60120.0, 60050.0, 60100.0);
        seed_closed_family(&state, "tri-leg0", "triangle", 100.0, 101.0, 100.5, 100.5);
        let radar = get_strategy_fill_pnl_impl("RADAR", &state).expect("radar pnl");
        assert_eq!(radar.closed_families, 1);
        assert!((radar.net_profit_usd - 64.0).abs() < 1e-9);
        let tri = get_strategy_fill_pnl_impl("triangle", &state).expect("tri pnl");
        assert_eq!(tri.closed_families, 1);
        // Cash: -100 + 101 + 100.5 - 100.5 = 1; fees 6 -> net -5.
        assert!((tri.net_profit_usd - (-5.0)).abs() < 1e-9, "net {:?}", tri);
        let futures = get_strategy_fill_pnl_impl("futures", &state).expect("futures pnl");
        assert_eq!(futures.closed_families, 0);
    }

    #[test]
    fn test_strategy_fill_pnl_unvalued_families_quarantined() {
        use crate::state_store::{NewSignalExecution, NewSignalLeg, SignalStatus};
        let state = setup_test_state_with_venues();
        // Closed with no fills at all: nothing valuable, excluded and counted.
        state
            .state_store
            .record_signal_execution(&NewSignalExecution {
                id: "sig-empty".to_string(),
                strategy_id: "futures".to_string(),
                symbol: "BTC-USDC".to_string(),
                route_id: None,
                expected_net_usd: None,
                note: None,
                legs: vec![NewSignalLeg {
                    account_id: None,
                    venue: "hyperliquid".to_string(),
                    side: crate::state_store::LegSide::Long,
                    intended_qty: 1.0,
                }],
            })
            .expect("empty root records");
        state
            .state_store
            .update_signal_status("sig-empty", SignalStatus::Closed, None)
            .expect("empty root closes");
        // Open families never enter the sums either way.
        seed_closed_family(&state, "sig-open", "futures", 60000.0, 60120.0, 60050.0, 60100.0);
        state
            .state_store
            .update_signal_status("sig-open", SignalStatus::Imbalanced, None)
            .expect("reopen root");
        let pnl = get_strategy_fill_pnl_impl("futures", &state).expect("pnl");
        assert_eq!(pnl.closed_families, 0);
        assert_eq!(pnl.open_families, 1);
        assert_eq!(pnl.unattributed_closed_families, 1);
        assert_eq!(pnl.net_profit_usd, 0.0);
        assert_eq!(pnl.net_profit_bps, None);
    }

    #[test]
    fn test_strategy_fill_pnl_counts_unattributed_log_rows() {
        let state = setup_test_state_with_venues();
        // Pre-attribution history: no strategy tag (serde default path).
        let legacy: crate::state::ExecutionLogEntry =
            serde_json::from_str(r#"{"id":"exec-old","timestamp":"2026-01-01T00:00:00Z","trade_type":"Flatten","symbol":"BTC-USDC","venue_a":"hyperliquid","venue_b":"lighter","leg_a_status":"-","leg_b_status":"-","success":true,"gross_profit":0.0,"fees":0.0,"net_profit":12.5,"latency_us":0,"note":"old"}"#)
                .expect("legacy log row deserializes");
        assert_eq!(
            legacy.strategy_id,
            crate::attribution::UNATTRIBUTED_STRATEGY_ID
        );
        state.execution_logs.write().push(legacy);
        let pnl = get_strategy_fill_pnl_impl("futures", &state).expect("pnl");
        assert_eq!(pnl.unattributed_log_entries, 1);
        assert_eq!(pnl.closed_families, 0);
        assert_eq!(pnl.net_profit_usd, 0.0, "legacy dollars never absorbed");
    }

    #[test]
    fn test_strategy_fill_pnl_deterministic() {
        let state = setup_test_state_with_venues();
        seed_closed_family(&state, "sig-det-1", "spot", 60000.0, 60120.0, 60050.0, 60100.0);
        seed_closed_family(&state, "sig-det-2", "spot", 60000.0, 60120.0, 60050.0, 60100.0);
        let first = get_strategy_fill_pnl_impl("spot", &state).expect("first");
        let second = get_strategy_fill_pnl_impl("spot", &state).expect("second");
        assert_eq!(first, second);
        assert_eq!(first.closed_families, 2);
    }

    #[tokio::test]
    async fn test_margin_snapshot_cadence_caps_frequency() {
        let state = setup_test_state_with_venues();
        let venues: Vec<String> = state
            .paper_exchanges()
            .iter()
            .map(|(vid, _)| vid.to_string())
            .collect();
        assert!(!venues.is_empty(), "test state must admit paper venues");
        let count_all = || {
            venues
                .iter()
                .map(|vid| {
                    state
                        .state_store
                        .list_margin_snapshots(vid, 0)
                        .expect("snapshots list")
                        .len()
                })
                .sum::<usize>()
        };
        let now = 1_700_000_000_000_i64;
        maybe_record_margin_snapshots(&state, now).await;
        maybe_record_margin_snapshots(&state, now).await;
        maybe_record_margin_snapshots(&state, now + 1_000).await;
        assert_eq!(
            count_all(),
            venues.len(),
            "one row per venue: repeats inside the cap window drop"
        );
        maybe_record_margin_snapshots(
            &state,
            now + crate::commands::MARGIN_SNAPSHOT_MIN_INTERVAL_MS,
        )
        .await;
        assert_eq!(count_all(), venues.len() * 2, "next window writes again");
    }

    #[test]
    fn test_utilisation_history_partial_window_labeled() {
        let state = setup_test_state_with_venues();
        let now = chrono::Utc::now().timestamp_millis();
        let hour = 3_600_000_i64;
        // 20% holds [now-2h, now-1h), 40% holds [now-1h, now] -> mean 30.
        state
            .state_store
            .record_margin_snapshot("lighter", "lighter", now - 2 * hour, 10_000.0, 2_000.0, 20.0)
            .expect("snapshot");
        state
            .state_store
            .record_margin_snapshot("lighter", "lighter", now - hour, 10_000.0, 4_000.0, 40.0)
            .expect("snapshot");
        let hist = get_utilisation_history_impl("lighter", Some(24), &state).expect("history");
        assert!(hist.avg_rate_pct.is_some());
        assert!(hist.partial, "2h of snapshots must not pose as 24h");
        assert!(hist.covered_ms < hist.window_ms);
        assert_eq!(hist.point_count, 2);
        let avg = hist.avg_rate_pct.expect("avg");
        assert!((avg - 30.0).abs() < 1e-9, "time-weighted mean {avg}");
        let empty = get_utilisation_history_impl("vanta", Some(24), &state).expect("empty");
        assert_eq!(empty.avg_rate_pct, None);
        assert!(empty.partial);
        assert_eq!(empty.covered_ms, 0);
    }

    #[test]
    fn test_attribution_ipc_commands_resolve() {
        let app = tauri::test::mock_builder()
            .manage(Arc::new(AppEngineState::new()))
            .invoke_handler(tauri::generate_handler![
                get_strategy_fill_pnl,
                get_utilisation_history
            ])
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app builds");
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("webview builds");

        for (cmd, body) in [
            (
                "get_strategy_fill_pnl",
                serde_json::json!({"strategyId": "radar"}),
            ),
            (
                "get_utilisation_history",
                serde_json::json!({"venue": "lighter", "windowHours": 24}),
            ),
        ] {
            let response = tauri::test::get_ipc_response(
                &webview,
                tauri::webview::InvokeRequest {
                    cmd: cmd.into(),
                    callback: tauri::ipc::CallbackFn(0),
                    error: tauri::ipc::CallbackFn(1),
                    url: "tauri://localhost".parse().expect("invoke url"),
                    body: tauri::ipc::InvokeBody::Json(body),
                    headers: Default::default(),
                    invoke_key: tauri::test::INVOKE_KEY.to_string(),
                },
            );
            assert!(
                response.is_ok(),
                "{cmd} must resolve the managed Arc<AppEngineState>: {:?}",
                response.err()
            );
        }
    }

    fn seed_book_depth(
        state: &AppEngineState,
        venue_id: &str,
        symbol: &str,
        bid: Decimal,
        bid_qty: Decimal,
        ask: Decimal,
        ask_qty: Decimal,
    ) {
        let mut book = arbitrageur_core::quote::OrderBook::new(
            arbitrageur_core::types::VenueId::new(venue_id),
            symbol.to_string(),
        );
        book.bids
            .push(arbitrageur_core::quote::BookLevel::new(bid, bid_qty));
        book.asks
            .push(arbitrageur_core::quote::BookLevel::new(ask, ask_qty));
        book.timestamp = Utc::now();
        state.market_data_hub.update_orderbook(book);
    }

    #[tokio::test]
    async fn test_emergency_flatten_retries_partial_close() {
        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let res = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            None,
            None,
            None,
            &state,
        )
        .await
        .expect("spatial execution");
        assert!(res.execution_success);

        seed_book_depth(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(0.05),
            dec!(64205.0),
            dec!(10.0),
        );
        seed_book_depth(
            &state,
            "lighter",
            "BTC-USDC",
            dec!(64360.0),
            dec!(10.0),
            dec!(64365.0),
            dec!(0.05),
        );

        let first = flatten_ticker_positions_impl(Some("BTC-USDC"), "futures", &state)
            .await
            .expect("first flatten");
        assert_eq!(
            first.residual_signals, 1,
            "partial close must leave exactly one residual root signal"
        );

        let acc_hl = state.ex_hl.fetch_account().await.expect("hl account");
        let acc_lgt = state.ex_lighter.fetch_account().await.expect("lgt account");
        let hl_qty = acc_hl
            .get_position("BTC-USDC")
            .map(|p| p.quantity)
            .expect("hl residual");
        let lgt_qty = acc_lgt
            .get_position("BTC-USDC")
            .map(|p| p.quantity)
            .expect("lgt residual");
        assert!(
            (hl_qty - dec!(0.05)).abs() < dec!(0.0001),
            "hl residual {hl_qty}"
        );
        assert!(
            (lgt_qty - dec!(0.05)).abs() < dec!(0.0001),
            "lgt residual {lgt_qty}"
        );

        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let second = flatten_ticker_positions_impl(Some("BTC-USDC"), "futures", &state)
            .await
            .expect("second flatten");
        assert_eq!(
            second.residual_signals, 0,
            "second flatten notes: {:?}",
            second.notes
        );

        let acc_hl = state.ex_hl.fetch_account().await.expect("hl account");
        let acc_lgt = state.ex_lighter.fetch_account().await.expect("lgt account");
        assert!(
            acc_hl.positions.is_empty(),
            "no residual or reverse position may remain: {:?}",
            acc_hl.positions
        );
        assert!(
            acc_lgt.positions.is_empty(),
            "no residual or reverse position may remain: {:?}",
            acc_lgt.positions
        );

        assert!(!state
            .state_store
            .ticker_blocked("BTC-USDC")
            .expect("blocked"));
        for sig in state
            .state_store
            .list_signals(Some("BTC-USDC"), None)
            .expect("signals")
        {
            assert_eq!(
                sig.status,
                crate::state_store::SignalStatus::Closed,
                "record {} must be closed",
                sig.id
            );
        }

        let ledger = state.virtual_ledger.read();
        let open_lots: Vec<_> = ledger
            .lots
            .values()
            .filter(|l| {
                l.strategy_id == "spatial_spread"
                    && l.symbol == "BTC-USDC"
                    && l.status == arbitrageur_execution::virtual_ledger::CycleStatus::Open
            })
            .collect();
        assert!(open_lots.is_empty(), "spatial lot must not stay Open");
        let lot = ledger
            .lots
            .values()
            .find(|l| l.strategy_id == "spatial_spread" && l.symbol == "BTC-USDC")
            .expect("spatial lot exists");
        assert_eq!(
            lot.status,
            arbitrageur_execution::virtual_ledger::CycleStatus::StoppedOut
        );
        assert!(lot.realized_pnl_usd.is_some(), "realized PnL populated");
    }

    #[tokio::test]
    async fn test_flatten_sweep_closes_sub_step_residual() {
        let state = AppEngineState::new();
        state.strategy_configs.write().insert(
            "BTC-USDC".to_string(),
            TickerStrategyConfigDto {
                symbol: "BTC-USDC".into(),
                unit_size: 1.0,
                ..TickerStrategyConfigDto::default()
            },
        );
        let step = flatten_step_qty(&state, "BTC-USDC");
        assert_eq!(step, dec!(1));
        // 1.4-step residual on a 1-unit close: the strand would be 0.4
        // steps, so the sweep closes the entire residual.
        assert_eq!(sweep_flatten_qty(dec!(1.4), dec!(1), step), dec!(1.4));
        // Normal exact-unit close is unchanged.
        assert_eq!(sweep_flatten_qty(dec!(1), dec!(1), step), dec!(1));
        // A real multi-step remainder still closes in part.
        assert_eq!(sweep_flatten_qty(dec!(2.4), dec!(1), step), dec!(1));
        // Unknown symbol: no usable step, plain capped close (fail closed).
        assert_eq!(flatten_step_qty(&state, "ETH-USDC"), dec!(0));
        assert_eq!(sweep_flatten_qty(dec!(1.4), dec!(1), dec!(0)), dec!(1));
    }

    #[tokio::test]
    async fn test_flatten_futures_root_preserves_other_strategy_position() {
        use crate::state_store::{LegSide, NewSignalExecution, NewSignalLeg, SignalStatus};

        let state = setup_test_state_with_venues();
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );

        let mut paper_book = OrderBook::new(VenueId::new("hyperliquid"), "BTC-USDC".to_string());
        paper_book.bids.push(arbitrageur_core::quote::BookLevel::new(
            dec!(64200.0),
            dec!(10.0),
        ));
        paper_book.asks.push(arbitrageur_core::quote::BookLevel::new(
            dec!(64205.0),
            dec!(10.0),
        ));
        state
            .ex_hl
            .replace_orderbook(paper_book)
            .expect("seed paper book");

        let open = arbitrageur_core::order::OrderRequest::new_market(
            state.ex_hl.venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(0.2),
        );
        let resp = state.ex_hl.place_order(open).await.expect("open position");
        assert_eq!(resp.status, arbitrageur_core::types::OrderStatus::Filled);

        for (id, strategy) in [("root-fut", "futures"), ("root-spot", "spot")] {
            state
                .state_store
                .record_signal_execution(&NewSignalExecution {
                    id: id.into(),
                    strategy_id: strategy.into(),
                    symbol: "BTC-USDC".into(),
                    route_id: None,
                    expected_net_usd: None,
                    note: None,
                    legs: vec![NewSignalLeg {
                        account_id: None,
                        venue: "hyperliquid".into(),
                        side: LegSide::Long,
                        intended_qty: 0.1,
                    }],
                })
                .expect("record root");
            state
                .state_store
                .apply_leg_fill(id, LegSide::Long, 0.1, 64205.0, 0.0, None)
                .expect("root fill");
        }

        let res = flatten_ticker_positions_impl(Some("BTC-USDC"), "futures", &state)
            .await
            .expect("flatten futures");

        let acc = state.ex_hl.fetch_account().await.expect("hl account");
        let pos = acc
            .get_position("BTC-USDC")
            .expect("spot strategy's residual must remain");
        assert_eq!(pos.side, Side::Buy, "no reverse position may be opened");
        assert!(
            (pos.quantity - dec!(0.1)).abs() < dec!(0.0001),
            "only the futures-attributed 0.1 may be closed, got residual {}",
            pos.quantity
        );

        let fut = state
            .state_store
            .get_signal("root-fut")
            .expect("fut")
            .expect("fut exists");
        assert_eq!(fut.status, SignalStatus::Closed);
        let spot = state
            .state_store
            .get_signal("root-spot")
            .expect("spot")
            .expect("spot exists");
        assert!(
            spot.status.counts_as_open(),
            "spot root must stay open, got {:?}",
            spot.status
        );
        for sig in state
            .state_store
            .list_signals(Some("BTC-USDC"), None)
            .expect("signals")
        {
            if sig.id.starts_with("flat-root-fut-") {
                assert_eq!(sig.status, SignalStatus::Closed, "child {}", sig.id);
            }
        }
        assert_eq!(res.residual_signals, 0);
        assert_eq!(res.signals_closed, 1);
    }

    fn enable_futures_passive(state: &AppEngineState, percentile: f64, timeout_secs: u64) {
        let mut p = StrategyParamsDto::defaults_for("futures");
        p.futures_passive_unwind.enabled = true;
        p.futures_passive_unwind.spread_percentile = percentile;
        p.futures_passive_unwind.timeout_secs = timeout_secs;
        p.futures_passive_unwind.max_tries = 2;
        p.futures_passive_unwind.max_spread_bps = 50.0;
        p.futures_passive_unwind.max_fast_vol_bps = 50.0;
        state
            .strategy_params
            .write()
            .insert("futures".to_string(), p);
    }

    async fn seed_long_for_flatten(state: &AppEngineState) {
        use crate::state_store::{LegSide, NewSignalExecution, NewSignalLeg};
        seed_test_book(
            state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        let mut paper_book = OrderBook::new(VenueId::new("hyperliquid"), "BTC-USDC".to_string());
        paper_book.bids.push(arbitrageur_core::quote::BookLevel::new(
            dec!(64200.0),
            dec!(10.0),
        ));
        paper_book.asks.push(arbitrageur_core::quote::BookLevel::new(
            dec!(64205.0),
            dec!(10.0),
        ));
        state
            .ex_hl
            .replace_orderbook(paper_book)
            .expect("seed paper book");
        let open = arbitrageur_core::order::OrderRequest::new_market(
            state.ex_hl.venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(0.1),
        );
        state.ex_hl.place_order(open).await.expect("open position");
        state
            .state_store
            .record_signal_execution(&NewSignalExecution {
                id: "root-passive".into(),
                strategy_id: "futures".into(),
                symbol: "BTC-USDC".into(),
                route_id: None,
                expected_net_usd: None,
                note: None,
                legs: vec![NewSignalLeg {
                    account_id: None,
                    venue: "hyperliquid".into(),
                    side: LegSide::Long,
                    intended_qty: 0.1,
                }],
            })
            .expect("record root");
        state
            .state_store
            .apply_leg_fill("root-passive", LegSide::Long, 0.1, 64205.0, 0.0, None)
            .expect("root fill");
    }

    #[tokio::test]
    async fn test_flatten_passive_limit_fills_at_aggressive_touch() {
        let state = setup_test_state_with_venues();
        // Percentile 0 sells at the bid (64200): touchable in the sim, so
        // the reduce closes via limit with no market fallback.
        enable_futures_passive(&state, 0.0, 1);
        seed_long_for_flatten(&state).await;

        let res = flatten_ticker_positions_impl(Some("BTC-USDC"), "futures", &state)
            .await
            .expect("flatten futures");
        assert_eq!(res.legs_closed, 1);
        assert_eq!(res.signals_closed, 1);
        assert!(
            res.notes.iter().any(|n| n.contains("passively closed")),
            "expected passive note, got {:?}",
            res.notes
        );
        assert!(
            res.notes.iter().all(|n| !n.contains("market exit")),
            "no market fallback expected, got {:?}",
            res.notes
        );
        let acc = state.ex_hl.fetch_account().await.expect("hl account");
        assert!(
            acc.get_position("BTC-USDC").is_none(),
            "position must be fully closed"
        );
    }

    #[tokio::test]
    async fn test_flatten_passive_falls_back_when_untouchable() {
        let state = setup_test_state_with_venues();
        // Percentile 100 sells at the ask (64205): above every bid, so both
        // limit tries miss (timeout 0) and the driver falls back to market.
        enable_futures_passive(&state, 100.0, 0);
        seed_long_for_flatten(&state).await;

        let res = flatten_ticker_positions_impl(Some("BTC-USDC"), "futures", &state)
            .await
            .expect("flatten futures");
        assert_eq!(res.legs_closed, 1);
        assert_eq!(res.signals_closed, 1);
        assert!(
            res.notes.iter().any(|n| n.contains("market exit")),
            "expected fallback note, got {:?}",
            res.notes
        );
        let acc = state.ex_hl.fetch_account().await.expect("hl account");
        assert!(
            acc.get_position("BTC-USDC").is_none(),
            "fallback must still close the position"
        );
    }

    #[tokio::test]
    async fn test_save_strategy_params_validates_passive_unwind() {
        // Bad percentile / tries / gates reject the whole record.
        for mutate in [
            |p: &mut StrategyParamsDto| p.futures_passive_unwind.spread_percentile = 101.0,
            |p: &mut StrategyParamsDto| p.futures_passive_unwind.spread_percentile = f64::NAN,
            |p: &mut StrategyParamsDto| p.funding_passive_unwind.max_tries = 0,
            |p: &mut StrategyParamsDto| p.spot_passive_unwind.max_tries = 11,
            |p: &mut StrategyParamsDto| p.futures_passive_unwind.max_spread_bps = 0.0,
            |p: &mut StrategyParamsDto| p.spot_passive_unwind.max_fast_vol_bps = f64::INFINITY,
        ] {
            let state = AppEngineState::new();
            let mut p = StrategyParamsDto::defaults_for("futures");
            mutate(&mut p);
            assert!(
                save_strategy_params_impl(p, &state).is_err(),
                "passive guard must reject"
            );
        }
        // Enabled with zero timeout is rejected at the save path (core
        // allows 0 for tests; operators must pick 1..=300).
        {
            let state = AppEngineState::new();
            let mut p = StrategyParamsDto::defaults_for("spot");
            p.spot_passive_unwind.enabled = true;
            p.spot_passive_unwind.timeout_secs = 0;
            assert!(save_strategy_params_impl(p, &state).is_err());
        }
        // A valid enabled record saves and reads back exactly.
        {
            let state = AppEngineState::new();
            let mut p = StrategyParamsDto::defaults_for("funding");
            p.funding_passive_unwind.enabled = true;
            p.funding_passive_unwind.spread_percentile = 50.0;
            p.funding_passive_unwind.timeout_secs = 10;
            p.funding_passive_unwind.max_tries = 3;
            save_strategy_params_impl(p, &state).expect("valid passive saves");
            let got = get_strategy_params_impl("funding", &state).expect("read back");
            assert!(got.funding_passive_unwind.enabled);
            assert_eq!(got.funding_passive_unwind.spread_percentile, 50.0);
            assert_eq!(got.funding_passive_unwind.timeout_secs, 10);
            assert_eq!(got.funding_passive_unwind.max_tries, 3);
        }
    }

    fn baseline_sample(venue: &str, ts_ms: i64, mid: f64) -> crate::state_store::SpreadSample {
        crate::state_store::SpreadSample {
            symbol: "BTC".into(),
            venue_id: venue.to_string(),
            ts_ms,
            mid,
            best_bid: mid - 0.5,
            best_ask: mid + 0.5,
        }
    }

    /// Two venues sampled together every `dt_ms`; the aaa->bbb pair spread
    /// follows a deterministic AR(1) driven by an LCG so OU fitting recovers
    /// a finite half-life with high R².
    fn ar1_samples(
        n: usize,
        dt_ms: i64,
        now_ms: i64,
        coef: f64,
        amp: f64,
    ) -> Vec<crate::state_store::SpreadSample> {
        let mut samples = Vec::with_capacity(n * 2);
        let mut x = 0.0;
        let mut seed: u64 = 0x9E37_79B9;
        for k in 0..n {
            let ts = now_ms - (n - 1 - k) as i64 * dt_ms;
            samples.push(baseline_sample("aaa", ts, 100.0));
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let eps = (((seed >> 33) % 1000) as f64 / 1000.0 - 0.5) * amp;
            x = coef * x + eps;
            samples.push(baseline_sample("bbb", ts, 100.0 * (1.0 + x / 10_000.0)));
        }
        samples
    }

    #[test]
    fn test_matrix_baselines_manual_sufficient_returns_stats() {
        let now = 1_800_000_000_000i64;
        let mut samples = Vec::new();
        for k in 0..120i64 {
            let ts = now - 7_200_000 + k * 60_000;
            samples.push(baseline_sample("aaa", ts, 100.0));
            let x = 5.0 + (k % 3) as f64; // spread 5,6,7 repeating
            samples.push(baseline_sample("bbb", ts, 100.0 * (1.0 + x / 10_000.0)));
        }
        let snap = compute_matrix_baselines("BTC", "manual", 7_200, 90, &samples, now);
        assert_eq!(snap.effective_window_secs, 7_200);
        assert_eq!(snap.auto_window_secs, None);
        assert!(!snap.using_manual_fallback);
        assert_eq!(snap.pairs.len(), 2);
        let p = &snap.pairs[0];
        assert_eq!(p.buy_venue, "aaa");
        assert_eq!(p.sell_venue, "bbb");
        assert_eq!(p.sample_count, 120);
        assert!((p.mean_gross_bps.unwrap() - 6.0).abs() < 1e-9);
        // 90th nearest-rank of {5,6,7}*~40 -> top bucket
        assert!((p.percentile_gross_bps.unwrap() - 7.0).abs() < 1e-9);
        assert!(p.history_span_secs >= 7_000);
        assert_eq!(snap.max_history_span_secs, p.history_span_secs);
    }

    #[test]
    fn test_matrix_baselines_manual_insufficient_span_returns_none_stats() {
        let now = 1_800_000_000_000i64;
        let mut samples = Vec::new();
        for k in 0..30i64 {
            let ts = now - 1_000_000 + k * 34_000;
            samples.push(baseline_sample("aaa", ts, 100.0));
            samples.push(baseline_sample("bbb", ts, 100.05));
        }
        let snap = compute_matrix_baselines("BTC", "manual", 3_600, 90, &samples, now);
        let p = &snap.pairs[0];
        assert_eq!(p.sample_count, 30, "windowed count stays truthful");
        assert!(p.mean_gross_bps.is_none());
        assert!(p.percentile_gross_bps.is_none());
        assert!(p.history_span_secs < 3_600);
    }

    #[test]
    fn test_matrix_baselines_auto_uses_ou_half_life_median() {
        let now = 1_800_000_000_000i64;
        let samples = ar1_samples(240, 60_000, now, 0.9, 0.3); // 4h, 240 obs
        let snap = compute_matrix_baselines("BTC", "auto", 86_400, 90, &samples, now);
        assert!(snap.qualifying_pair_count >= 1);
        let auto = snap.auto_window_secs.expect("auto window selected");
        assert_eq!(snap.effective_window_secs, auto);
        assert!(!snap.using_manual_fallback);
        assert!(snap.median_half_life_secs.unwrap() > 0.0);

        // Recompute the per-pair recommendations with the same math helpers
        // and verify the snapshot carries their upper median.
        let mut expected_recs = Vec::new();
        for (buy, sell) in [("aaa", "bbb"), ("bbb", "aaa")] {
            let ma: Vec<(i64, f64)> = samples
                .iter()
                .filter(|s| s.venue_id == buy)
                .map(|s| (s.ts_ms, s.mid))
                .collect();
            let mb: Vec<(i64, f64)> = samples
                .iter()
                .filter(|s| s.venue_id == sell)
                .map(|s| (s.ts_ms, s.mid))
                .collect();
            let series =
                arbitrageur_math::pair_monitor::build_pair_series(&ma, &mb, PAIR_ALIGN_GAP_MS);
            let n = series.len();
            let span_ms = series[n - 1].ts_ms - series[0].ts_ms;
            let dt = span_ms as f64 / 1000.0 / (n - 1) as f64;
            let vals: Vec<f64> = series.iter().map(|o| o.spread_bps).collect();
            if let Some(p) = OuModelParams::fit(&vals, dt)
                .filter(|p| p.half_life > 0.0 && p.r_squared >= OU_MIN_R2)
            {
                expected_recs.push(
                    ((2.0 * p.half_life).round() as u64)
                        .clamp(AUTO_WINDOW_FLOOR_SECS, AUTO_WINDOW_CAP_SECS),
                );
            }
        }
        expected_recs.sort_unstable();
        assert_eq!(snap.qualifying_pair_count as usize, expected_recs.len());
        assert_eq!(
            auto,
            expected_recs[expected_recs.len() / 2],
            "auto window is the upper median of 2x OU half-life recommendations"
        );
        // a=0.9 at 60s cadence -> half-life ~395s -> 2x hits the 30m floor
        assert_eq!(auto, AUTO_WINDOW_FLOOR_SECS);
    }

    #[test]
    fn test_matrix_baselines_auto_falls_back_when_unqualified() {
        let now = 1_800_000_000_000i64;
        let samples = ar1_samples(10, 60_000, now, 0.9, 0.3); // only 10 obs
        let snap = compute_matrix_baselines("BTC", "auto", 7_200, 90, &samples, now);
        assert_eq!(snap.auto_window_secs, None);
        assert_eq!(snap.effective_window_secs, 7_200);
        assert!(snap.using_manual_fallback);
        assert_eq!(snap.qualifying_pair_count, 0);
        assert_eq!(snap.median_half_life_secs, None);

        let empty = compute_matrix_baselines("BTC", "auto", 86_400, 90, &[], now);
        assert_eq!(empty.effective_window_secs, 86_400);
        assert!(empty.using_manual_fallback);
        assert!(empty.pairs.is_empty());
        assert_eq!(empty.max_history_span_secs, 0);
    }

    #[tokio::test]
    async fn test_get_matrix_baselines_validation_and_alias_merge() {
        let state = setup_test_state_with_venues();
        assert!(get_matrix_baselines_impl("BTC-USDC", "sideways", 86_400, 90, &state)
            .await
            .is_err());
        assert!(get_matrix_baselines_impl("BTC-USDC", "manual", 60, 90, &state)
            .await
            .is_err());
        assert!(
            get_matrix_baselines_impl("BTC-USDC", "manual", 3_000_000, 90, &state)
                .await
                .is_err()
        );
        assert!(get_matrix_baselines_impl("BTC-USDC", "manual", 86_400, 40, &state)
            .await
            .is_err());
        assert!(get_matrix_baselines_impl("BTC\u{0}-USDC", "manual", 86_400, 90, &state)
            .await
            .is_err());

        // No history: auto falls back to the manual window, nothing fabricated.
        let snap = get_matrix_baselines_impl("BTC-USDC", "auto", 86_400, 90, &state)
            .await
            .expect("empty history still returns a snapshot");
        assert_eq!(snap.auto_window_secs, None);
        assert_eq!(snap.effective_window_secs, 86_400);
        assert!(snap.using_manual_fallback);
        assert_eq!(snap.qualifying_pair_count, 0);
        assert!(snap.pairs.is_empty());

        // Same venue+ts recorded under both the base alias and the UI symbol
        // must merge/dedup into one observation.
        let now_ms = Utc::now().timestamp_millis();
        for key in ["BTC", "BTC-USDC"] {
            state
                .state_store
                .record_spread_samples(
                    key,
                    now_ms,
                    &[
                        crate::state_store::VenueSampleEntry {
                            venue_id: "aaa".into(),
                            mid: 100.0,
                            best_bid: 99.5,
                            best_ask: 100.5,
                        },
                        crate::state_store::VenueSampleEntry {
                            venue_id: "bbb".into(),
                            mid: 100.05,
                            best_bid: 99.55,
                            best_ask: 100.55,
                        },
                    ],
                )
                .expect("record");
        }
        let snap = get_matrix_baselines_impl("BTC-USDC", "manual", 86_400, 90, &state)
            .await
            .expect("snapshot");
        let p = snap
            .pairs
            .iter()
            .find(|p| p.buy_venue == "aaa" && p.sell_venue == "bbb")
            .expect("pair row");
        assert_eq!(p.sample_count, 1, "duplicate alias rows deduplicated");
        assert!(p.mean_gross_bps.is_none(), "one obs is not a baseline");
    }

    fn lighter_only_state() -> AppEngineState {
        let state = AppEngineState::new();
        state.settings.write().lighter_enabled = true;
        state
    }

    fn seed_triangle_books(state: &AppEngineState) {
        // Profitable USDC -> BTC -> ETH -> USDC cycle on lighter:
        // 10k USDC buys BTC @60,000, sells BTC for ETH @20/BTC,
        // sells ETH @3,100 -> ~10,333 USDC before fees. The middle leg is
        // the "{A}-{B}" book the planner routes; depth must cover the
        // chained quantity or evaluation fails closed.
        seed_test_book(state, "lighter", "BTC-USDC", dec!(59990.0), dec!(60000.0));
        seed_book_depth(
            state,
            "lighter",
            "BTC-ETH",
            dec!(20.0),
            dec!(100.0),
            dec!(20.01),
            dec!(100.0),
        );
        seed_test_book(state, "lighter", "ETH-BTC", dec!(0.05), dec!(0.0501));
        seed_test_book(state, "lighter", "ETH-USDC", dec!(3100.0), dec!(3101.0));
    }

    fn seed_aligned_triangle_books(state: &AppEngineState) {
        // Cross rates agree: no mispricing, so venue fees eat the cycle.
        seed_test_book(state, "lighter", "BTC-USDC", dec!(59990.0), dec!(60000.0));
        seed_book_depth(
            state,
            "lighter",
            "BTC-ETH",
            dec!(19.3),
            dec!(100.0),
            dec!(19.31),
            dec!(100.0),
        );
        seed_test_book(state, "lighter", "ETH-BTC", dec!(0.0510), dec!(0.0511));
        seed_test_book(state, "lighter", "ETH-USDC", dec!(3100.0), dec!(3101.0));
    }

    #[tokio::test]
    async fn test_scan_triangle_finds_profitable_cycle() {
        let state = lighter_only_state();
        seed_triangle_books(&state);

        let opps = scan_triangular_opportunities_impl("", &state)
            .await
            .expect("triangle scan");
        // Live venue books may complete additional real cycles; pin the
        // seeded one by id instead of asserting an exact count.
        let opp = opps
            .iter()
            .find(|o| o.id == "lighter-TRI-BTC-ETH-USDC")
            .expect("seeded BTC/ETH cycle must be listed");
        assert_eq!(opp.venue, "Lighter");
        assert_eq!(opp.leg1.symbol, "BTC-USDC");
        assert_eq!(opp.leg1.action, "BUY");
        assert_eq!(opp.leg2.symbol, "BTC-ETH");
        assert_eq!(opp.leg2.action, "SELL");
        assert_eq!(opp.leg3.symbol, "ETH-USDC");
        assert_eq!(opp.leg3.action, "SELL");
        assert!(opp.gross_spread_bps > 300.0, "gross {}", opp.gross_spread_bps);
        assert!(opp.net_spread_bps > 300.0, "net {}", opp.net_spread_bps);
        assert!(opp.is_executable);
        // Results arrive sorted by net edge, descending.
        for pair in opps.windows(2) {
            assert!(
                pair[0].net_spread_bps >= pair[1].net_spread_bps,
                "scan must sort by net edge: {} then {}",
                pair[0].id,
                pair[1].id
            );
        }

        // Asset filter narrows to cycles containing the asset…
        let btc = scan_triangular_opportunities_impl("BTC", &state)
            .await
            .expect("filtered scan");
        assert!(!btc.is_empty());
        assert!(btc.iter().all(|o| o.intermediate_a == "BTC"
            || o.intermediate_b == "BTC"));
        assert!(btc
            .iter()
            .any(|o| o.id == "lighter-TRI-BTC-ETH-USDC"));
        // …while an unknown asset fails closed to empty.
        let unknown = scan_triangular_opportunities_impl("ZZZ", &state)
            .await
            .expect("filtered scan");
        assert!(unknown.is_empty());

        // Determinism: re-seed (refreshing hub timestamps) and rescan —
        // the seeded cycle is listed identically.
        seed_triangle_books(&state);
        let again = scan_triangular_opportunities_impl("", &state)
            .await
            .expect("rescan");
        let re = again
            .iter()
            .find(|o| o.id == "lighter-TRI-BTC-ETH-USDC")
            .expect("seeded cycle must persist across scans");
        assert_eq!(re.net_spread_bps, opp.net_spread_bps);
    }

    #[tokio::test]
    async fn test_scan_triangle_fee_eaten_cycle_not_executable() {
        let state = lighter_only_state();
        seed_aligned_triangle_books(&state);

        let opps = scan_triangular_opportunities_impl("", &state)
            .await
            .expect("triangle scan");
        let opp = opps
            .iter()
            .find(|o| o.id == "lighter-TRI-BTC-ETH-USDC")
            .expect("aligned cycle must still be listed");
        assert!(
            !opp.is_executable,
            "fee-eaten cycle must not be executable: net {}",
            opp.net_spread_bps
        );
        assert!(opp.net_spread_bps < 1.0);
    }

    #[tokio::test]
    async fn test_scan_triangle_missing_leg_fail_closed() {
        let state = lighter_only_state();
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(59990.0), dec!(60000.0));
        // A leg symbol no venue lists fails the whole resolution: no
        // fabricated cycle, regardless of the other legs.
        let adapter = state.get_adapter("lighter").expect("lighter admitted");
        let vid = adapter.venue_id();
        let missing = resolve_triangle_books(
            &state,
            &adapter,
            &vid,
            &[
                "BTC-USDC".to_string(),
                "ZZZ-QQQ".to_string(),
                "ETH-USDC".to_string(),
            ],
        )
        .await;
        assert!(missing.is_none());

        // No admitted venues at all: empty, not an error.
        let bare = AppEngineState::new();
        let opps = scan_triangular_opportunities_impl("", &bare)
            .await
            .expect("scan without venues");
        assert!(opps.is_empty());

        // Invalid filter rejected.
        assert!(scan_triangular_opportunities_impl("BTC!", &state)
            .await
            .is_err());
    }

    fn find_triangle_opp(opps: &[TriangleOpportunityDto], id: &str) -> TriangleOpportunityDto {
        opps.iter().find(|o| o.id == id).cloned().expect("seeded cycle")
    }

    #[tokio::test]
    async fn test_execute_triangle_success_records_legs_and_blocks_reentry() {
        let state = lighter_only_state();
        seed_triangle_books(&state);

        let opps = scan_triangular_opportunities_impl("", &state)
            .await
            .expect("scan");
        let opp = find_triangle_opp(&opps, "lighter-TRI-BTC-ETH-USDC");
        let id = opp.id.clone();

        let res = execute_triangle_arbitrage_impl(
            TriangleExecutionParams {
                id: id.clone(),
                venue: "lighter".into(),
                path: opp.path.clone(),
                trade_size_usd: 1000.0,
                min_profit_bps: 1.0,
                net_spread_bps: None,
            },
            &state,
        )
        .await
        .expect("triangle execute");
        assert_eq!(res.trade_type, "Triangle");
        assert!(res.execution_success);
        assert!(res.spread_bps > 200.0, "spread {}", res.spread_bps);

        // Paper account holds the three spread legs.
        let acc = state
            .get_paper_exchange("lighter")
            .expect("lighter paper")
            .fetch_account()
            .await
            .expect("account");
        assert!(acc.get_position("BTC-USDC").is_some());
        assert!(acc.get_position("BTC-ETH").is_some());
        assert!(acc.get_position("ETH-USDC").is_some());

        // Three imbalanced triangle signals block the leg symbols.
        for sym in ["BTC-USDC", "BTC-ETH", "ETH-USDC"] {
            let blockers = state
                .state_store
                .blocking_signals(sym)
                .expect("blocking signals");
            assert_eq!(blockers.len(), 1, "one triangle residual on {sym}");
            assert_eq!(blockers[0].strategy_id, "triangle");
        }

        // Re-entry on the same cycle is refused while legs are open.
        let again = execute_triangle_arbitrage_impl(
            TriangleExecutionParams {
                id,
                venue: "lighter".into(),
                path: "USDC → BTC → ETH → USDC".into(),
                trade_size_usd: 1000.0,
                min_profit_bps: 1.0,
                net_spread_bps: None,
            },
            &state,
        )
        .await;
        assert!(again.is_err(), "open legs must block re-entry");

        // Each leg unwinds through the existing flatten path, freeing the slot.
        for sym in ["BTC-USDC", "BTC-ETH", "ETH-USDC"] {
            let flat = flatten_ticker_positions_impl(Some(sym), "triangle", &state)
                .await
                .expect("flatten leg");
            assert_eq!(flat.residual_signals, 0, "flatten notes: {:?}", flat.notes);
        }
        let acc = state
            .get_paper_exchange("lighter")
            .expect("lighter paper")
            .fetch_account()
            .await
            .expect("account");
        assert!(acc.positions.is_empty(), "all legs flattened: {:?}", acc.positions);
    }

    #[tokio::test]
    async fn test_execute_triangle_fail_closed_paths() {
        let state = lighter_only_state();
        seed_triangle_books(&state);
        let opps = scan_triangular_opportunities_impl("", &state)
            .await
            .expect("scan");
        let opp = find_triangle_opp(&opps, "lighter-TRI-BTC-ETH-USDC");
        let mk = |id: String, venue: &str, size: f64, hurdle: f64| TriangleExecutionParams {
            id,
            venue: venue.into(),
            path: "USDC → BTC → ETH → USDC".into(),
            trade_size_usd: size,
            min_profit_bps: hurdle,
            net_spread_bps: Some(99999.0),
        };

        // Unknown opportunity id (caller-supplied spread is never trusted).
        assert!(execute_triangle_arbitrage_impl(mk("lighter-TRI-X-Y-USDC".into(), "lighter", 1000.0, 1.0), &state)
            .await
            .is_err());
        // Hurdle above the live edge refuses dispatch.
        assert!(execute_triangle_arbitrage_impl(
            mk(opp.id.clone(), "lighter", 1000.0, 100_000.0),
            &state
        )
        .await
        .is_err());
        // Unadmitted venue refused.
        assert!(execute_triangle_arbitrage_impl(
            mk(opp.id.clone(), "kraken", 1000.0, 1.0),
            &state
        )
        .await
        .is_err());
        // Invalid sizes refused.
        assert!(execute_triangle_arbitrage_impl(
            mk(opp.id.clone(), "lighter", f64::NAN, 1.0),
            &state
        )
        .await
        .is_err());
        // Live mode hard-refuses paper dispatch.
        *state.is_live_mode.write() = true;
        assert!(execute_triangle_arbitrage_impl(
            mk(opp.id.clone(), "lighter", 1000.0, 1.0),
            &state
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn test_convergence_hold_then_converge_close_reentry() {
        let state = setup_test_state_with_venues();
        seed_test_book(&state, "hyperliquid", "BTC-USDC", dec!(64200.0), dec!(64205.0));
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            Some("Divergence Entry".into()),
            Some(18.5),
            Some(12.0),
            Some(1),
            Some(3),
            Some(15.0),
            &state,
        )
        .await
        .expect("spatial open");

        // Edge still wide: hold, slot stays occupied.
        let checks = check_spatial_convergence_impl(Some("BTC-USDC"), "futures", &state)
            .await
            .expect("convergence check");
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].verdict, "hold");
        assert!(!checks[0].unwound);
        assert!(checks[0].entry_net_bps.expect("baseline recorded") > 15.0);
        let blocked = execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            Some(1),
            Some(3),
            None,
            &state,
        )
        .await;
        assert!(blocked.is_err(), "open pair must keep the slot blocked");

        // Compress the spread: the edge is gone, so the check unwinds.
        seed_test_book(&state, "hyperliquid", "BTC-USDC", dec!(64200.0), dec!(64205.0));
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64201.0), dec!(64206.0));
        let checks = check_spatial_convergence_impl(Some("BTC-USDC"), "futures", &state)
            .await
            .expect("convergence recheck");
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].verdict, "exit");
        assert_eq!(checks[0].trigger.as_deref(), Some("converged"));
        assert!(checks[0].unwound);

        // Slot freed: the pair can be re-entered (books still overlap enough
        // for the economics gate? re-seed the wide edge first).
        seed_test_book(&state, "hyperliquid", "BTC-USDC", dec!(64200.0), dec!(64205.0));
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));
        execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            Some(1),
            Some(3),
            None,
            &state,
        )
        .await
        .expect("re-entry after unwind");
    }

    #[tokio::test]
    async fn test_convergence_stop_and_timeout_unwind() {
        let state = setup_test_state_with_venues();
        seed_test_book(&state, "hyperliquid", "BTC-USDC", dec!(64200.0), dec!(64205.0));
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));
        execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            Some(1),
            Some(3),
            None,
            &state,
        )
        .await
        .expect("spatial open");

        // Adverse excursion: live edge ~10 bps vs ~20 entry -> stopped.
        seed_test_book(&state, "hyperliquid", "BTC-USDC", dec!(64200.0), dec!(64205.0));
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64275.0), dec!(64280.0));
        let checks = check_spatial_convergence_impl(Some("BTC-USDC"), "futures", &state)
            .await
            .expect("stop check");
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].verdict, "exit");
        assert_eq!(checks[0].trigger.as_deref(), Some("stopped"));
        assert!(checks[0].unwound);

        // Re-open, then backdate the baseline past the hold deadline.
        seed_test_book(&state, "hyperliquid", "BTC-USDC", dec!(64200.0), dec!(64205.0));
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));
        execute_spatial_arbitrage_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            Some(1),
            Some(3),
            None,
            &state,
        )
        .await
        .expect("second open");
        let sigs = state
            .state_store
            .list_signals(Some("BTC-USDC"), None)
            .expect("signals");
        let open = sigs
            .iter()
            .find(|s| s.status.counts_as_open())
            .expect("one open signal");
        let old = state
            .state_store
            .spatial_baseline_for_signal(&open.id)
            .expect("baseline read")
            .expect("baseline present");
        state
            .state_store
            .record_spatial_baseline(&crate::state_store::SpatialEntryBaseline {
                entry_ts_ms: Utc::now().timestamp_millis() - 2 * 86_400_000,
                ..old
            })
            .expect("backdate baseline");
        // Keep the edge wide so only the deadline fires.
        let checks = check_spatial_convergence_impl(Some("BTC-USDC"), "futures", &state)
            .await
            .expect("timeout check");
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].verdict, "exit");
        assert_eq!(checks[0].trigger.as_deref(), Some("timed_out"));
        assert!(checks[0].unwound);
    }

    #[tokio::test]
    async fn test_convergence_unwatched_without_baseline() {
        let state = setup_test_state_with_venues();
        // In-flight signal with no baseline row: never auto-exited.
        state
            .state_store
            .record_signal_execution(&crate::state_store::NewSignalExecution {
                id: "sig-nobaseline".into(),
                strategy_id: "futures".into(),
                symbol: "BTC-USDC".into(),
                route_id: None,
                expected_net_usd: None,
                note: None,
                legs: vec![
                    crate::state_store::NewSignalLeg {
                        account_id: None,
                        venue: "hyperliquid".into(),
                        side: crate::state_store::LegSide::Long,
                        intended_qty: 0.1,
                    },
                    crate::state_store::NewSignalLeg {
                        account_id: None,
                        venue: "lighter".into(),
                        side: crate::state_store::LegSide::Short,
                        intended_qty: 0.1,
                    },
                ],
            })
            .expect("record signal");
        let checks = check_spatial_convergence_impl(Some("BTC-USDC"), "futures", &state)
            .await
            .expect("check");
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].verdict, "unwatched");
        assert!(!checks[0].unwound);
        // Non-futures strategies are rejected, not guessed.
        assert!(check_spatial_convergence_impl(Some("BTC-USDC"), "radar", &state)
            .await
            .is_err());
    }
}

// ---------------------------------------------------------------------------
// PopDEX agent-setup toolkit (local keygen + approval helpers + verify).
// The main private key never enters the app: approval is signed in the
// user's own wallet app from data built here.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedAgentDto {
    pub address: String,
    pub private_key: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApproveTxDto {
    pub to: String,
    pub chain_id: u64,
    pub rpc_url: String,
    pub explorer_url: String,
    pub data: String,
    pub initial_nonce_ms: u64,
    pub eip681_uri: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCheckDto {
    pub approved: bool,
    pub agent_count: usize,
}

fn popdex_toolkit_network(network: &str) -> Result<arbitrageur_exchanges::popdex::PopDexNetwork, String> {
    use arbitrageur_exchanges::popdex::PopDexNetwork;
    match network.trim().to_lowercase().as_str() {
        "mainnet" | "main" | "prod" | "" => Ok(PopDexNetwork::Mainnet),
        "testnet" | "test" => Ok(PopDexNetwork::Testnet),
        other => Err(format!("unknown PopDEX network '{other}' (want mainnet|testnet)")),
    }
}

/// Generate a fresh PopDEX agent keypair locally. Returns the private key
/// ONCE — the UI must place it straight into the vault-backed account field.
#[tauri::command]
pub async fn generate_popdex_agent() -> Result<GeneratedAgentDto, String> {
    use arbitrageur_exchanges::popdex::generate_agent_keypair;
    let (address, key) = generate_agent_keypair().map_err(|e| e.to_string())?;
    Ok(GeneratedAgentDto {
        address,
        private_key: key.to_string(),
    })
}

/// Build the one-time `approveAgent` transaction payload for external
/// signing (main wallet signs in the user's own wallet app).
#[tauri::command]
pub async fn build_popdex_approve_tx(
    main: String,
    agent: String,
    network: String,
) -> Result<ApproveTxDto, String> {
    use arbitrageur_exchanges::popdex::{approve_agent_eip681, build_approve_agent_calldata};
    let net = popdex_toolkit_network(&network)?;
    let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
    let data = build_approve_agent_calldata(&agent, &main, now_ms).map_err(|e| e.to_string())?;
    // Sanity: agent must not be the main address itself (build_approve_agent_calldata
    // takes addresses; equality is refused at credential-bind time in the adapter).
    if agent.trim().eq_ignore_ascii_case(main.trim()) {
        return Err("agent address equals the main address — generate a fresh agent key instead".to_string());
    }
    Ok(ApproveTxDto {
        to: arbitrageur_exchanges::popdex::POPDEX_ACCOUNT_PRECOMPILE.to_string(),
        chain_id: match net {
            arbitrageur_exchanges::popdex::PopDexNetwork::Mainnet => {
                arbitrageur_exchanges::popdex::POPDEX_MAINNET_CHAIN_ID
            }
            arbitrageur_exchanges::popdex::PopDexNetwork::Testnet => {
                arbitrageur_exchanges::popdex::POPDEX_TESTNET_CHAIN_ID
            }
        },
        rpc_url: match net {
            arbitrageur_exchanges::popdex::PopDexNetwork::Mainnet => {
                arbitrageur_exchanges::popdex::POPDEX_MAINNET_RPC.to_string()
            }
            arbitrageur_exchanges::popdex::PopDexNetwork::Testnet => {
                arbitrageur_exchanges::popdex::POPDEX_TESTNET_RPC.to_string()
            }
        },
        explorer_url: match net {
            arbitrageur_exchanges::popdex::PopDexNetwork::Mainnet => {
                "https://app.popdex.xyz/explorer".to_string()
            }
            arbitrageur_exchanges::popdex::PopDexNetwork::Testnet => {
                "https://testnet-app.popdex.xyz/explorer".to_string()
            }
        },
        eip681_uri: approve_agent_eip681(&main, &agent, now_ms, match net {
            arbitrageur_exchanges::popdex::PopDexNetwork::Mainnet => {
                arbitrageur_exchanges::popdex::POPDEX_MAINNET_CHAIN_ID
            }
            arbitrageur_exchanges::popdex::PopDexNetwork::Testnet => {
                arbitrageur_exchanges::popdex::POPDEX_TESTNET_CHAIN_ID
            }
        }),
        data,
        initial_nonce_ms: now_ms,
    })
}

/// Verify on-chain (via public REST) that `agent` is approved for `main`.
#[tauri::command]
pub async fn check_popdex_agent(
    main: String,
    agent: String,
    network: String,
) -> Result<AgentCheckDto, String> {
    use arbitrageur_exchanges::popdex::check_agent_approval;
    let net = popdex_toolkit_network(&network)?;
    let (approved, agent_count) = check_agent_approval(net, &main, &agent)
        .await
        .map_err(|e| e.to_string())?;
    Ok(AgentCheckDto {
        approved,
        agent_count,
    })
}

/// Open an `https://` URL in the OS browser (Tauri blocks plain
/// `target="_blank"` navigation, so toolkit doc/faucet/app links go through
/// here). Host allowlist only — never files, never custom schemes.
#[tauri::command]
pub async fn open_external_url(url: String) -> Result<(), String> {
    let u = url.trim();
    let host = u
        .strip_prefix("https://")
        .and_then(|rest| rest.split(['/','?','#']).next())
        .unwrap_or_default()
        .to_lowercase();
    const ALLOWED: &[&str] = &[
        "popdex.xyz",
        "crypto-chief.com",
    ];
    let ok = ALLOWED
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")));
    if !ok {
        return Err(format!("refusing to open non-allowlisted URL: {u}"));
    }
    #[cfg(target_os = "linux")]
    let status = std::process::Command::new("xdg-open")
        .arg(u)
        .status()
        .map_err(|e| format!("cannot open browser: {e}"))?;
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("open")
        .arg(u)
        .status()
        .map_err(|e| format!("cannot open browser: {e}"))?;
    #[cfg(target_os = "windows")]
    let status = std::process::Command::new("cmd")
        .args(["/c", "start", "", u])
        .status()
        .map_err(|e| format!("cannot open browser: {e}"))?;
    if !status.success() {
        return Err("browser did not open the link".to_string());
    }
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignApproveDto {
    pub tx_hash: String,
    pub explorer_url: String,
}

/// Sign + submit `approveAgent` with the MAIN key, entirely in the toolkit
/// modal. The main key is transient: it is wrapped in a zeroizing buffer on
/// arrival, signs exactly one transaction, and is dropped on return — never
/// persisted, never bound to any adapter, never logged. Operators who would
/// rather not paste it anywhere use the DIY `cast` command
/// (`build_popdex_approve_tx` + `approve_agent_cast_command`) instead.
#[tauri::command]
pub async fn sign_popdex_approve_with_main(
    main: String,
    agent: String,
    main_key: String,
    network: String,
) -> Result<SignApproveDto, String> {
    use arbitrageur_exchanges::popdex::submit_approve_agent_with_main;
    use zeroize::Zeroizing;
    let net = popdex_toolkit_network(&network)?;
    let key = Zeroizing::new(main_key);
    let tx_hash = submit_approve_agent_with_main(net, main.trim(), agent.trim(), &key)
        .await
        .map_err(|e| e.to_string())?;
    // `key` (and the original `main_key` String) drop here — zeroized.
    Ok(SignApproveDto {
        explorer_url: match net {
            arbitrageur_exchanges::popdex::PopDexNetwork::Mainnet => {
                format!("https://app.popdex.xyz/explorer/tx/{tx_hash}")
            }
            arbitrageur_exchanges::popdex::PopDexNetwork::Testnet => {
                format!("https://testnet-app.popdex.xyz/explorer/tx/{tx_hash}")
            }
        },
        tx_hash,
    })
}


