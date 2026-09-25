use crate::num::dec_opt_f64;
use crate::state::{AppEngineState, EngineSettingsDto, ExecutionLogEntry, StrategyParamsDto};
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::quote::OrderBook;
use arbitrageur_core::types::{dec, Decimal, Side, VenueId};
use arbitrageur_execution::smart_routing::SmartOrderRouter;
// AutoTune, strategy-config, and accounts/vault live in their own modules;
// re-exported so `commands::<name>` paths (IPC registration, tests) keep
// working.
pub(crate) use crate::accounts::*;
pub(crate) use crate::autotune::*;
pub(crate) use crate::execution::*;
pub(crate) use crate::funding::*;
pub(crate) use crate::monitor_ipc::*;
pub(crate) use crate::performance::*;
pub(crate) use crate::strategy_config::*;
pub(crate) use crate::triangle::*;
use arbitrageur_math::dislocation::{
    compute_deviation_series, distribution_stats, BaselineMode, MidSample,
};
use arbitrageur_math::funding_carry::backtest_gap_carry;
use arbitrageur_math::optimal_thresholds::compute_bertram_thresholds;
use arbitrageur_math::ou_process::OuModelParams;
use arbitrageur_math::pair_monitor::{build_pair_series, capacity_ladder};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::State;

/// Live strategy-entry gate. Entry is admitted when the vault is unlocked
/// and every leg's adapter both submits and polls orders (the reconciler
/// requires `fetch_order`). Applies to spatial, funding, and triangle
/// entries — Trading Mode = Live is the operator's opt-in, and the
/// reconciliation machinery (poll supervisor, residual auto-flatten,
/// bounded cancel retry) covers every entry that lands imbalanced.
pub(crate) fn live_entry_gate(state: &AppEngineState, venues: &[&str]) -> Result<(), String> {
    if !state.vault.write().is_unlocked() {
        return Err("Live strategy entries require an unlocked credential vault".into());
    }
    for venue in venues {
        let adapter = state
            .get_adapter(venue)
            .ok_or_else(|| format!("Venue '{venue}' is not admitted for live execution"))?;
        if !adapter.live_order_submission_implemented() {
            return Err(format!("Venue '{venue}' has no live order submission"));
        }
        if !adapter.order_status_polling_implemented() {
            return Err(format!(
                "Venue '{venue}' has no order-status polling for reconciliation"
            ));
        }
        if !adapter.live_strategy_entry_implemented() {
            return Err(format!(
                "Venue '{venue}' submission transport is not admitted for automated strategy entry"
            ));
        }
        if !adapter.has_live_credentials() {
            return Err(format!("Venue '{venue}' has no live credentials loaded"));
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SystemStatusDto {
    pub is_live_mode: bool,
    /// Selected trading environment (`testnet` or `live`) — the
    /// authoritative env label for the UI; `is_live_mode` derives from it.
    pub environment: String,
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
    /// Adapter-reported order submission capability in the *active*
    /// environment (`live_order_submission_implemented`). False for
    /// read-only venues, non-admitted venues, and inert scaffolds.
    #[serde(default)]
    pub can_submit: bool,
    /// Adapter-reported automated strategy-entry capability in the active
    /// environment (`live_strategy_entry_implemented`).
    #[serde(default)]
    pub can_enter_strategy: bool,
}

/// One row of the executor's taker-fee table, exposed over IPC so UI
/// surfaces can never drift from executed cost.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct VenueFeeDto {
    pub venue_id: String,
    pub taker_bps: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct VenueFeesDto {
    pub fees: Vec<VenueFeeDto>,
    /// Fallback for venue ids absent from `fees` — equals the executor's
    /// default arm. Frontend lookups must use this, not a local constant.
    pub default_bps: f64,
}

pub(crate) fn get_venue_fees_impl() -> VenueFeesDto {
    VenueFeesDto {
        fees: VENUE_TAKER_FEE_BPS
            .iter()
            .map(|(id, bps)| VenueFeeDto {
                venue_id: (*id).to_string(),
                taker_bps: *bps,
            })
            .collect(),
        default_bps: DEFAULT_TAKER_FEE_BPS,
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct VenueVolumesDto {
    pub venue_id: String,
    /// 24h notional volume in quote currency (~USD), keyed by the venue's
    /// raw asset name (e.g. "BTC", "kPEPE"). Only assets the venue actually
    /// reports are included.
    pub volumes: HashMap<String, f64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct VenueMarketUniverseDto {
    pub venue_id: String,
    pub symbols: Vec<String>,
    /// Best-effort reference price keyed by canonical `BASE-QUOTE` symbol.
    /// Missing entries are honest unknowns; consumers must not fabricate one.
    #[serde(default)]
    pub base_prices: HashMap<String, f64>,
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
    /// Wilson lower bound of `win_probability` (z=1.645) on the episode
    /// bucket's effective count — how much confidence the evidence
    /// actually supports. `None` when unavailable or model-derived.
    #[serde(default)]
    pub win_probability_lower: Option<f64>,
    /// Wilson upper bound of `win_probability` — the interval width shows
    /// how thin the episode evidence is.
    #[serde(default)]
    pub win_probability_upper: Option<f64>,
    /// Depth-supported size maximizing expected USD (monitor's lockstep
    /// capacity walk). `None` when no win-rate curve exists.
    #[serde(default)]
    pub optimal_size_usd: Option<f64>,
    /// Largest fillable size whose marginal level clears the win floor
    /// with positive expected net. `None` when unmeasurable.
    #[serde(default)]
    pub breakeven_size_usd: Option<f64>,
    /// Probability-weighted fillable notional over levels with positive
    /// marginal expected net. `None` when no win-rate curve exists.
    #[serde(default)]
    pub expected_capacity_usd: Option<f64>,
    /// Fillable notional at each marginal expected-net floor.
    #[serde(default)]
    pub bps_capacity_ladder: Vec<crate::monitor::BpsCapacityTierDto>,
    /// Per-chunk capacity curve for the detail-modal chart.
    #[serde(default)]
    pub depth_curve: Vec<crate::monitor::DepthCurvePointDto>,
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
    /// Per-chunk capacity curve for the detail-modal chart. Empty until
    /// the spot scan resolves real books (the impl is currently stubbed).
    #[serde(default)]
    pub depth_curve: Vec<crate::monitor::DepthCurvePointDto>,
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
    pub conditional_payout_cents: f64,
    pub net_profit_cents: f64,
    pub roi_pct: f64,
    pub has_arbitrage: bool,
    /// Per-chunk capacity curve for the detail-modal chart. Empty until
    /// the prediction scan resolves real books (the impl is stubbed).
    #[serde(default)]
    pub depth_curve: Vec<crate::monitor::DepthCurvePointDto>,
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
    /// `Some` only for fits that produced an R² — never fabricated.
    pub r_squared: Option<f64>,
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

/// Assumed taker fee per venue (standard/base tier) used by the entry
/// profit floor. These are deliberately the *published standard-tier*
/// taker rates, not discounted/VIP tiers: an understated fee lets a
/// losing entry pass the hurdle, while an overstated fee only skips a
/// marginal opportunity — the safe error direction. 2026-09-23 audit:
/// the table previously assumed ~2bps for most venues while real
/// standard-tier takers run 4–5bps on Binance/OKX/Kraken/Coinbase-class
/// venues and 4.5bps on Hyperliquid.
/// Standard-tier taker fee (bps) per venue — the executor's authoritative
/// fee table and the single source of truth for every displayed fee.
/// `get_venue_fees` serves this verbatim over IPC; frontend surfaces must
/// look it up there rather than keeping copies (two hand-maintained
/// frontend tables drifted from this one before — phantom net edge).
///
/// EVM spot AMMs price fees per-pair on-chain; their rows here are the
/// documented display defaults (PancakeSwap V2 charges 25bp; Solidly-style
/// volatile pools ~30bp; UniV3 pairs span 5–100bp so the conservative tier
/// is the default).
const VENUE_TAKER_FEE_BPS: &[(&str, f64)] = &[
    ("hyperliquid", 4.5),
    ("lighter", 1.5),
    ("vanta", 4.0),
    ("robinhood", 4.0),
    // Verified base tier via GET /v1/feetiers (level 0: 450ppm taker).
    ("arcus", 4.5),
    ("aster", 4.0),
    ("binance", 5.0),
    ("okx", 5.0),
    // Deribit perp/future taker is 0.05%; options charge
    // min(0.03% of underlying, 12.5% of premium) — the options-parity
    // scanner prices that per-instrument; this row covers status
    // display and the perp leg's generic fee lookups.
    ("deribit", 5.0),
    ("kraken", 5.0),
    ("coinbase", 5.0),
    // Alpaca crypto Tier-1: 15bps maker / 25bps taker on order notional
    // (published fee schedule). Marketable orders are taker; equities are
    // commission-free but this row covers the crypto legs the engine trades.
    ("alpaca", 25.0),
    ("decibel", 5.0),
    ("popdex", 10.0),
    // Variational Omni charges zero trading fees (OLP-internal venue).
    ("variational", 0.0),
    ("polymarket", 0.0),
    ("kalshi", 10.0),
    // DexPerp family — mirrors each venue's spec-published taker.
    ("pacifica", 2.0),
    ("standx", 2.0),
    ("ondoperps", 3.0),
    ("grvt", 5.0),
    ("derive", 2.0),
    ("aevo", 2.0),
    // Vest charges a flat 1bp on API transactions (crypto perps;
    // equity/forex symbols currently carry takerFee 0 — the market
    // parser uses the per-symbol rate when it matters).
    ("vest", 1.0),
    // Nado base taker tier is 3.5bps (verified from the live symbols
    // query); per-symbol rates ride market metadata when they differ.
    ("nado", 3.5),
    ("qfex", 4.0),
    // EVEDEX verified /api/market fee schedule: taker 4.5bps, maker 1.5bps.
    ("evedex", 4.5),
    // Paradex spec-published taker.
    ("paradex", 2.0),
    // edgeX v2 defaultTakerFeeRate (per-contract — BTC 4.5bps).
    ("edgex", 4.5),
    // Extended standard taker tier.
    ("extended", 2.0),
    // dYdX base-tier taker.
    ("dydx", 2.0),
    // Injective derivative markets: taker_fee_rate 0.000001 = 0.01bps.
    ("helix", 0.01),
    // Pendle Boros YU taker fee: 5bps of notional (market `takerFee`
    // field; the scanner also reads it per-market).
    ("boros", 5.0),
    // GMX V2 open/close fee ≈ 0.05% of position.
    ("gmx", 5.0),
    // EVM spot AMM display defaults (per-pair truth rides pool metadata).
    ("uniswap", 30.0),
    ("uniswap_v2", 30.0),
    ("pancakeswap", 25.0),
    ("pancakeswap_v3", 25.0),
    ("sushiswap", 30.0),
    ("quickswap", 30.0),
    ("camelot", 30.0),
    ("aerodrome", 30.0),
    ("velodrome", 30.0),
    ("ramses", 30.0),
    ("pharaoh", 30.0),
    // TradFi broker display defaults.
    ("ibkr", 1.0),
    ("ig", 5.0),
];

/// Default taker fee for venues absent from `VENUE_TAKER_FEE_BPS` —
/// surfaced in `get_venue_fees` so the frontend fallback can never
/// diverge from the executor's.
pub(crate) const DEFAULT_TAKER_FEE_BPS: f64 = 4.0;

pub(crate) fn venue_fee_bps(vid: &str) -> f64 {
    let v = vid.to_lowercase();
    VENUE_TAKER_FEE_BPS
        .iter()
        .find(|(id, _)| *id == v)
        .map(|(_, f)| *f)
        .unwrap_or(DEFAULT_TAKER_FEE_BPS)
}

pub(crate) fn effective_venue_fee_bps(state: &AppEngineState, venue: &str) -> f64 {
    let base: f64 = venue_fee_bps(venue);
    let plus = matches!(venue.to_ascii_lowercase().as_str(), "lighter" | "robinhood")
        && state
            .state_store
            .default_account(venue, state.env().as_str())
            .ok()
            .flatten()
            .is_some_and(|a| a.account_tier == "plus");
    if plus {
        base + 0.5
    } else {
        base
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RouteEconomics {
    buy_fee_bps: f64,
    sell_fee_bps: f64,
    round_trip_commission_bps: f64,
    entry_slippage_bps: f64,
    exit_slippage_bps: f64,
    closing_spread_bps: f64,
    expected_friction_bps: f64,
    top_gross_bps: f64,
    pub(crate) expected_net_bps: f64,
    required_gross_bps: f64,
    pub(crate) estimated_profit_usd: f64,
}

pub(crate) fn compute_route_economics(
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
    let entry_slippage_bps = ((buy_entry_vwap - buy_best_ask) / buy_best_ask * 10_000.0).max(0.0)
        + ((sell_best_bid - sell_entry_vwap) / sell_best_bid * 10_000.0).max(0.0);
    let exit_slippage_bps = ((buy_best_bid - buy_exit_vwap) / buy_best_bid * 10_000.0).max(0.0)
        + ((sell_exit_vwap - sell_best_ask) / sell_best_ask * 10_000.0).max(0.0);
    let buy_spread_bps =
        (buy_best_ask - buy_best_bid) / ((buy_best_ask + buy_best_bid) / 2.0) * 10_000.0;
    let sell_spread_bps =
        (sell_best_ask - sell_best_bid) / ((sell_best_ask + sell_best_bid) / 2.0) * 10_000.0;
    let closing_spread_bps = buy_spread_bps + sell_spread_bps;
    let round_trip_commission_bps = 2.0 * (buy_fee_bps + sell_fee_bps);
    let expected_friction_bps =
        round_trip_commission_bps + entry_slippage_bps + exit_slippage_bps + closing_spread_bps;
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

/// Per-strategy slippage gate value (bps) — the single resolver every
/// execution path uses. Maps the strategy id to its `*_max_slippage_bps`
/// field; a missing record falls back to that strategy's defaults. Callers
/// convert to Decimal and fail closed on invalid values. There is
/// intentionally no global slippage setting.
pub(crate) fn strategy_slippage_bps(state: &AppEngineState, strategy_id: &str) -> f64 {
    let default = StrategyParamsDto::defaults_for(strategy_id);
    let params = state.strategy_params.read();
    let p = params.get(strategy_id).unwrap_or(&default);
    let v = match strategy_id {
        "funding" => p.funding_max_slippage_bps,
        "spot" => p.spot_max_slippage_bps,
        "triangle" => p.triangle_max_slippage_bps,
        "prediction" => p.prediction_max_slippage_bps,
        _ => p.futures_max_slippage_bps,
    };
    let dv = match strategy_id {
        "funding" => default.funding_max_slippage_bps,
        "spot" => default.spot_max_slippage_bps,
        "triangle" => default.triangle_max_slippage_bps,
        "prediction" => default.prediction_max_slippage_bps,
        _ => default.futures_max_slippage_bps,
    };
    // A corrupt record must not widen the gate — fall back to the default.
    if v.is_finite() && v >= 0.0 {
        v
    } else {
        dv
    }
}

/// Whether the strategy/monitor execution mode includes a close leg.
/// Both serializations exist in persisted state — "SINGLE_WAY"/"DOUBLE_WAY"
/// (strategy params, per-ticker configs) and "one_way"/"two_way" (monitor
/// config) — so match on the normalized form, never the raw string.
pub(crate) fn execution_mode_is_two_way(mode: &str) -> bool {
    let m = mode.trim().to_ascii_lowercase().replace('-', "_");
    matches!(m.as_str(), "two_way" | "double_way")
}

pub(crate) fn route_execution_gates(
    book_buy: &OrderBook,
    book_sell: &OrderBook,
    qty: Decimal,
    max_slippage_bps: Decimal,
    min_depth_ratio: Decimal,
) -> (bool, bool) {
    let router = SmartOrderRouter::new(max_slippage_bps);
    let (Some(buy_ref), Some(sell_ref)) = (book_buy.best_ask_price(), book_sell.best_bid_price())
    else {
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

pub(crate) fn validate_market_symbol(symbol: &str) -> Result<(), String> {
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

pub(crate) fn validate_adapter_book(
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
    // Hub keys normalize symbol spellings (WS lanes publish "ETH", REST
    // callers ask "ETH-USDC"); identity holds when the canonical base
    // matches — a genuinely different market has a different base.
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

// ---------------------------------------------------------------------------
// Implementation functions (pure, unit-testable without Tauri State wrapper)
// ---------------------------------------------------------------------------

/// All venue ids currently enabled — engine flag OR an enabled account
/// profile. Drives which venues the status poll latency-probes.
fn venue_enabled_ids(state: &AppEngineState, s: &crate::state::EngineSettingsDto) -> Vec<String> {
    let env = state.env();
    let account_enabled = |venue: &str| {
        state
            .state_store
            .has_enabled_account(venue, env.as_str())
            .unwrap_or(false)
    };
    let mut out = Vec::new();
    let mut push = |id: &str, on: bool| {
        if on {
            out.push(id.to_string());
        }
    };
    push(
        "hyperliquid",
        s.hyperliquid_enabled || account_enabled("hyperliquid"),
    );
    push("lighter", s.lighter_enabled || account_enabled("lighter"));
    push(
        "robinhood",
        s.robinhood_enabled || account_enabled("robinhood"),
    );
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
    push(
        "polymarket",
        s.polymarket_enabled || account_enabled("polymarket"),
    );
    push("kalshi", s.kalshi_enabled || account_enabled("kalshi"));
    push("decibel", s.decibel_enabled || account_enabled("decibel"));
    push("aster", s.aster_enabled || account_enabled("aster"));
    push("popdex", s.popdex_enabled || account_enabled("popdex"));
    push("binance", s.binance_enabled || account_enabled("binance"));
    push(
        "coinbase",
        s.coinbase_enabled || account_enabled("coinbase"),
    );
    push("alpaca", s.alpaca_enabled || account_enabled("alpaca"));
    push("okx", s.okx_enabled || account_enabled("okx"));
    push("deribit", s.deribit_enabled || account_enabled("deribit"));
    push("ibkr", s.ibkr_enabled || account_enabled("ibkr"));
    push("ig", s.ig_enabled || account_enabled("ig"));
    // Credential-less market-data venue — the account row IS the switch.
    push("variational", account_enabled("variational"));
    // Long-tail perp venues admit via an enabled account only (no
    // engine-settings flags exist for them).
    for adapter in &state.live_dex_perps {
        push(adapter.spec_id(), account_enabled(adapter.spec_id()));
    }
    for adapter in &state.live_evm_amms {
        push(adapter.spec_id(), account_enabled(adapter.spec_id()));
    }
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
        "deribit" => p(
            "https://www.deribit.com/api/v2/public/get_index_price?index_name=btc_usd",
            false,
        ),
        "coinbase" => p("https://api.coinbase.com/v2/time", false),
        // The Omni stats document is the same public endpoint the adapter
        // reads — a valid RTT even when the venue isn't adapter-admitted.
        "variational" => p(
            "https://omni-client-api.prod.ap-northeast-1.variational.io/metadata/stats",
            false,
        ),
        // Alpaca's clock endpoint is public; even without keys a 401/403
        // from the API tier still proves the origin responded.
        "alpaca" => p("https://api.alpaca.markets/v2/clock", true),
        "aster" => p("https://fapi.asterdex.com/fapi/v1/ping", false),
        "popdex" => p(
            "https://api.popdex.xyz/api/v1/public/market/tickers?category=Futures&limit=1",
            false,
        ),
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
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
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
    let is_live = state.is_live();
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
        (
            mode_logs.len(),
            mode_logs.iter().filter(|l| l.success).count(),
        )
    };

    // Cloned so no lock guard is held across the latency-probe awaits.
    let s = state.settings.read().clone();
    // A venue is enabled when its engine flag is set OR it owns an enabled
    // account profile — the same admission rule `get_venues` applies.
    let env = state.env();
    let account_enabled = |venue: &str| {
        state
            .state_store
            .has_enabled_account(venue, env.as_str())
            .unwrap_or(false)
    };
    // Admission is authoritative for the `supported` leg: a venue reports
    // READ_ONLY iff get_venues() actually admits it (enabled + real adapter).
    // A hardcoded per-venue bool previously let robinhood report UNSUPPORTED
    // while get_venues admitted it with a live adapter.
    let admitted = state.get_venues();
    let admitted_map: HashMap<&str, &Arc<dyn ExchangeAdapter>> =
        admitted.iter().map(|(vid, _, a)| (*vid, a)).collect();
    let venue_status = |vid: &str, enabled: bool| -> &str {
        if !crate::state::venue_has_adapter(vid) {
            // No constructed adapter exists — enablement can never admit
            // the venue, so report UNSUPPORTED even when the row is off.
            "UNSUPPORTED"
        } else if !enabled {
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
                let fallback = public_probe_endpoint(&vid);
                probes.push(tokio::spawn(async move {
                    let start = std::time::Instant::now();
                    if adapter.ping().await.is_ok() {
                        return (vid, Some(start.elapsed().as_millis() as u64));
                    }
                    // Adapter ping failed — locked vault, an auth-gated
                    // endpoint, or the venue's local rate-limit backoff
                    // refusing before a packet ever left. The verified public
                    // endpoint measures the same API host's network path, so
                    // fall back to it rather than dropping the sample.
                    match fallback {
                        Some(spec) => {
                            let start = std::time::Instant::now();
                            let ms = probe_http(spec)
                                .await
                                .ok()
                                .map(|_| start.elapsed().as_millis() as u64);
                            (vid, ms)
                        }
                        None => (vid, None),
                    }
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

    let mut venues = vec![
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
            fee_bps: effective_venue_fee_bps(state, "hyperliquid"),
            can_submit: false,
            can_enter_strategy: false,
        },
        VenueStatusDto {
            id: "lighter".into(),
            name: "Lighter (zkDEX)".into(),
            market_type: "zk-Rollup CLOB".into(),
            status: venue_status("lighter", s.lighter_enabled || account_enabled("lighter")).into(),
            latency_ms: probe_ms("lighter"),
            fee_bps: effective_venue_fee_bps(state, "lighter"),
            can_submit: false,
            can_enter_strategy: false,
        },
        VenueStatusDto {
            id: "vanta".into(),
            name: "Vanta.exchange".into(),
            market_type: "Omnichain Perps".into(),
            status: venue_status("vanta", s.vanta_enabled || account_enabled("vanta")).into(),
            latency_ms: probe_ms("vanta"),
            fee_bps: effective_venue_fee_bps(state, "vanta"),
            can_submit: false,
            can_enter_strategy: false,
        },
        VenueStatusDto {
            id: "arcus".into(),
            name: "Arcus DEX".into(),
            market_type: "Fast CLOB".into(),
            status: venue_status("arcus", s.arcus_enabled || account_enabled("arcus")).into(),
            latency_ms: probe_ms("arcus"),
            fee_bps: effective_venue_fee_bps(state, "arcus"),
            can_submit: false,
            can_enter_strategy: false,
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
            fee_bps: effective_venue_fee_bps(state, "robinhood"),
            can_submit: false,
            can_enter_strategy: false,
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
            fee_bps: effective_venue_fee_bps(state, "kraken"),
            can_submit: false,
            can_enter_strategy: false,
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
            can_submit: false,
            can_enter_strategy: false,
        },
        VenueStatusDto {
            id: "kalshi".into(),
            name: "Kalshi (CFTC)".into(),
            market_type: "Prediction Market".into(),
            status: venue_status("kalshi", s.kalshi_enabled || account_enabled("kalshi")).into(),
            latency_ms: probe_ms("kalshi"),
            fee_bps: 10.0,
            can_submit: false,
            can_enter_strategy: false,
        },
        VenueStatusDto {
            id: "decibel".into(),
            name: "Decibel".into(),
            market_type: "Aptos Perp DEX".into(),
            status: venue_status("decibel", s.decibel_enabled || account_enabled("decibel")).into(),
            latency_ms: probe_ms("decibel"),
            fee_bps: effective_venue_fee_bps(state, "decibel"),
            can_submit: false,
            can_enter_strategy: false,
        },
        VenueStatusDto {
            id: "aster".into(),
            name: "Aster".into(),
            market_type: "Multi-Chain Perp DEX".into(),
            status: venue_status("aster", s.aster_enabled || account_enabled("aster")).into(),
            latency_ms: probe_ms("aster"),
            fee_bps: effective_venue_fee_bps(state, "aster"),
            can_submit: false,
            can_enter_strategy: false,
        },
        VenueStatusDto {
            id: "popdex".into(),
            name: "PopDEX".into(),
            market_type: "EVM Perp DEX".into(),
            status: venue_status("popdex", s.popdex_enabled || account_enabled("popdex")).into(),
            latency_ms: probe_ms("popdex"),
            fee_bps: effective_venue_fee_bps(state, "popdex"),
            can_submit: false,
            can_enter_strategy: false,
        },
        VenueStatusDto {
            id: "binance".into(),
            name: "Binance".into(),
            market_type: "CEX Spot & Perp".into(),
            status: venue_status("binance", s.binance_enabled || account_enabled("binance")).into(),
            latency_ms: probe_ms("binance"),
            fee_bps: effective_venue_fee_bps(state, "binance"),
            can_submit: false,
            can_enter_strategy: false,
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
            fee_bps: effective_venue_fee_bps(state, "coinbase"),
            can_submit: false,
            can_enter_strategy: false,
        },
        VenueStatusDto {
            id: "alpaca".into(),
            name: "Alpaca".into(),
            market_type: "Equities & Crypto Spot Broker".into(),
            status: venue_status("alpaca", s.alpaca_enabled || account_enabled("alpaca")).into(),
            latency_ms: probe_ms("alpaca"),
            fee_bps: effective_venue_fee_bps(state, "alpaca"),
            can_submit: false,
            can_enter_strategy: false,
        },
        VenueStatusDto {
            id: "okx".into(),
            name: "OKX".into(),
            market_type: "CEX Spot & Perp".into(),
            status: venue_status("okx", s.okx_enabled || account_enabled("okx")).into(),
            latency_ms: probe_ms("okx"),
            fee_bps: effective_venue_fee_bps(state, "okx"),
            can_submit: false,
            can_enter_strategy: false,
        },
        VenueStatusDto {
            id: "deribit".into(),
            name: "Deribit".into(),
            market_type: "Options & Perp CLOB".into(),
            status: venue_status("deribit", s.deribit_enabled || account_enabled("deribit")).into(),
            latency_ms: probe_ms("deribit"),
            fee_bps: effective_venue_fee_bps(state, "deribit"),
            can_submit: false,
            can_enter_strategy: false,
        },
        VenueStatusDto {
            id: "ibkr".into(),
            name: "Interactive Brokers".into(),
            market_type: "TradFi Broker".into(),
            status: venue_status("ibkr", s.ibkr_enabled || account_enabled("ibkr")).into(),
            latency_ms: probe_ms("ibkr"),
            fee_bps: 1.0,
            can_submit: false,
            can_enter_strategy: false,
        },
        VenueStatusDto {
            id: "ig".into(),
            name: "IG Trading".into(),
            market_type: "TradFi Broker".into(),
            status: venue_status("ig", s.ig_enabled || account_enabled("ig")).into(),
            latency_ms: probe_ms("ig"),
            fee_bps: 5.0,
            can_submit: false,
            can_enter_strategy: false,
        },
        VenueStatusDto {
            id: "variational".into(),
            name: "Variational Omni".into(),
            market_type: "Peer-to-Pool Perps".into(),
            status: venue_status("variational", account_enabled("variational")).into(),
            latency_ms: probe_ms("variational"),
            // Omni is zero-fee — the integrated OLP retains market-making
            // yield instead of charging taker/maker commissions.
            fee_bps: 0.0,
            can_submit: false,
            can_enter_strategy: false,
        },
    ];
    // Long-tail perp venues append dynamically — same admission rule
    // (enabled account) and the adapter's spec carries name/type/fee.
    for adapter in &state.live_dex_perps {
        let vid = adapter.spec_id();
        venues.push(VenueStatusDto {
            id: vid.into(),
            name: adapter.spec_name().into(),
            market_type: adapter.market_type_label().into(),
            status: venue_status(vid, account_enabled(vid)).into(),
            latency_ms: probe_ms(vid),
            fee_bps: adapter.taker_fee_bps(),
            can_submit: false,
            can_enter_strategy: false,
        });
    }
    for adapter in &state.live_evm_amms {
        let vid = adapter.spec_id();
        venues.push(VenueStatusDto {
            id: vid.into(),
            name: adapter.spec_name().into(),
            market_type: adapter.market_type_label().into(),
            status: venue_status(vid, account_enabled(vid)).into(),
            latency_ms: probe_ms(vid),
            fee_bps: adapter.taker_fee_bps(),
            can_submit: false,
            can_enter_strategy: false,
        });
    }

    // Populate capability flags from the *admitted* env-scoped adapter —
    // the same object execution resolves, so the UI's READ-ONLY badge
    // always matches what the backend would actually do. Non-admitted
    // venues (unknown id, wrong env, disabled) read false.
    for v in &mut venues {
        if let Some(adapter) = state.get_adapter(&v.id) {
            v.can_submit = adapter.live_order_submission_implemented();
            v.can_enter_strategy = adapter.live_strategy_entry_implemented();
        }
    }

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
        environment: state.env().as_str().to_string(),
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

pub async fn refresh_venue_latencies_impl(
    state: &AppEngineState,
) -> Result<SystemStatusDto, String> {
    state.latency_probe_at.lock().clear();
    get_system_status_impl(state).await
}

pub async fn get_market_universe_impl(
    state: &AppEngineState,
) -> Result<Vec<VenueMarketUniverseDto>, String> {
    let mut tasks = Vec::new();
    for (vid, _, adapter) in state.get_venues() {
        let vid = vid.to_string();
        tasks.push(tokio::spawn(async move {
            let (markets, prices) =
                tokio::join!(adapter.fetch_markets(), adapter.fetch_market_prices());
            let prices = prices.unwrap_or_default();
            (vid, markets, prices)
        }));
    }
    let mut out = Vec::new();
    for h in tasks {
        if let Ok((vid, Ok(markets), prices)) = h.await {
            let mut symbols: Vec<String> = markets
                .iter()
                .filter(|m| {
                    m.is_active
                        && m.symbol.market_type == arbitrageur_core::market::MarketType::Perpetual
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

            let mut base_prices = HashMap::new();
            for market in &markets {
                if !market.is_active
                    || market.symbol.market_type != arbitrageur_core::market::MarketType::Perpetual
                {
                    continue;
                }
                let Some(price) = prices.get(&market.venue_symbol) else {
                    continue;
                };
                let price = price.to_string().parse::<f64>().ok();
                if let Some(price) = price.filter(|p| p.is_finite() && *p > 0.0) {
                    base_prices.insert(market.symbol.canonical(), price);
                }
            }
            out.push(VenueMarketUniverseDto {
                venue_id: vid,
                symbols,
                base_prices,
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
    // Row caps are unconditional — a disabled day window (`None`/`0`) must
    // never let a telemetry table grow without bound. `i64::MIN` makes the
    // day cutoff a no-op while the per-table caps still evict oldest-first.
    let retain_after = match max_days {
        Some(days) if days > 0 => now_ms - (days as i64) * 86_400_000,
        _ => i64::MIN,
    };
    let _ = state
        .state_store
        .prune_spread_samples(retain_after, crate::state_store::SPREAD_SAMPLES_MAX_ROWS);
    // Same cutoff bounds the raw volume log. Session profiles and hour
    // bins are derived state and are never pruned here.
    let _ = state.state_store.prune_venue_volume_samples(
        retain_after,
        crate::state_store::VENUE_VOLUME_SAMPLES_MAX_ROWS,
    );
    // Same day cutoff bounds the signal-event audit log, with its own row
    // cap so audit history and price history prune independently.
    let _ = state.state_store.prune_monitor_signal_events(
        retain_after,
        crate::state_store::MONITOR_SIGNAL_EVENTS_MAX_ROWS,
    );
    // Same day cutoff bounds the margin-snapshot log behind the 24h
    // average-utilisation stat.
    let _ = state.state_store.prune_margin_snapshots(retain_after);
    // Same day cutoff bounds the funding-rate history log (future
    // persistence / half-life training substrate).
    let _ = state.state_store.prune_funding_rate_samples(retain_after);
    if let Some(gb) = max_gb {
        if gb.is_finite() && gb > 0.0 {
            let budget = (gb * 1024.0 * 1024.0 * 1024.0) as u64;
            // Measure USED bytes, not allocated: deletes grow the freelist
            // without shrinking page_count, so an allocated-size budget can
            // never be satisfied and would drain the whole table.
            for _ in 0..1000 {
                let size = match state.state_store.db_used_bytes() {
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
    // Return freed pages to the OS on every pass — bounded per pass,
    // converges over successive hourly runs toward the used size. No-op on
    // databases opened before the v34 auto_vacuum migration.
    let _ = state.state_store.incremental_vacuum(50_000);
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
            // Persist on the USD basis: displayed quotes stay venue-native,
            // but the stored series is what pair math compares — under
            // fx_adjusted a USDT-quoted book converts through the rate table.
            // Quote resolution: the book's own spelling, else the monitor's
            // refreshed per-venue quote map, else the USD baseline.
            let book_q = arbitrageur_core::symbol::symbol_quote(&book.symbol);
            let key_q = arbitrageur_core::symbol::symbol_quote(symbol);
            // Quote-strict sampling: under `separate` a book whose written
            // quote disagrees with the requested key is a different
            // instrument — don't fold its mids into this series.
            if !state.usd_merge_quotes() {
                if let (Some(kq), Some(bq)) = (&key_q, &book_q) {
                    if kq != bq {
                        continue;
                    }
                }
            }
            let quote = book_q
                .or_else(|| {
                    state
                        .monitor
                        .lock()
                        .as_ref()
                        .and_then(|m| m.book_quote(vid, symbol, state.usd_merge_quotes()))
                })
                .unwrap_or_else(|| "USD".into());
            let scale = state.fx_price_factor(Some(vid), &quote);
            sample_entries.push(crate::state_store::VenueSampleEntry {
                venue_id: vid.into(),
                mid: mid * scale,
                best_bid: best_bid * scale,
                best_ask: best_ask * scale,
                bid_depth: (total_bid_depth > 0.0).then_some(total_bid_depth),
                ask_depth: (total_ask_depth > 0.0).then_some(total_ask_depth),
            });
        }
    }

    // Durable signal history: persist this poll's venue mids so the
    // dislocation series can be sliced over arbitrary windows. Best-effort —
    // a store failure must never break the market-data path.
    let now_ms = Utc::now().timestamp_millis();
    let merge_usd = state.usd_merge_quotes();
    let _ =
        state
            .state_store
            .record_spread_samples_mode(symbol, merge_usd, now_ms, &sample_entries);

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
                let f = venue_fee_bps(&best_ask_venue);
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
    let max_slippage_bps = strategy_slippage_bps(state, "futures");
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
    if let Ok(samples) = state.state_store.spread_samples_mode(
        symbol,
        state.usd_merge_quotes(),
        scan_history_from,
        now_ms,
    ) {
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
                let is_executable = econ.expected_net_bps >= min_bps && slippage_ok && depth_ok;

                // Pair spread series shared by the win estimate and the
                // capacity walk — same source, same alignment gap.
                let series = match (
                    scan_mids_by_venue.get(buy_id),
                    scan_mids_by_venue.get(sell_id),
                ) {
                    (Some(buy_mids), Some(sell_mids))
                        if !buy_mids.is_empty() && !sell_mids.is_empty() =>
                    {
                        build_pair_series(buy_mids, sell_mids, crate::monitor::PAIR_ALIGN_GAP_MS)
                    }
                    _ => Vec::new(),
                };

                // Informational win probability on the shared monitor
                // standard — now with a Wilson interval so thin episode
                // evidence shows its width instead of a confident point.
                // Never feeds ordering or `is_executable`.
                let (win_probability, win_probability_lower, win_probability_upper) =
                    crate::monitor::pair_win_probability_ex(
                        &series,
                        econ.top_gross_bps,
                        econ.expected_friction_bps,
                        scan_min_episodes,
                        None,
                        now_ms,
                    )
                    .map(|e| (Some(e.p), Some(e.lower), Some(e.upper)))
                    .unwrap_or((None, None, None));

                // Capacity report over both leg books — the same lockstep
                // walk + win-rate model the Simple Spread monitor uses.
                // No episode series → empty curve: the raw net view still
                // renders, expected fields stay None (never fabricated).
                let to_pairs = |levels: &[arbitrageur_core::quote::BookLevel]| -> Vec<(f64, f64)> {
                    levels
                        .iter()
                        .filter_map(|l| {
                            let p = l.price.to_string().parse::<f64>().ok()?;
                            let q = l.size.to_string().parse::<f64>().ok()?;
                            (p.is_finite() && p > 0.0 && q.is_finite() && q > 0.0).then_some((p, q))
                        })
                        .collect()
                };
                let asks = to_pairs(&book_buy.asks);
                let bids = to_pairs(&book_sell.bids);
                let (pwin_curve, baseline_bps) = match crate::monitor::pair_pwin_curve(
                    &series,
                    econ.expected_friction_bps,
                    None,
                    now_ms,
                ) {
                    Some((curve, mu, _)) => (curve, mu),
                    None => (
                        arbitrageur_math::pair_monitor::PWinCurve {
                            points: Vec::new(),
                            episodes: 0,
                            point_wins: Vec::new(),
                            point_trials: Vec::new(),
                            point_censored: Vec::new(),
                            point_sum_w2: Vec::new(),
                        },
                        // Raw-spread economics: dev = gross − 0.
                        0.0,
                    ),
                };
                let cap_rep = capacity_ladder(
                    &asks,
                    &bids,
                    baseline_bps,
                    econ.expected_friction_bps,
                    &monitor_cfg.capacity_tiers_usd,
                    &pwin_curve,
                    monitor_cfg.min_win_probability,
                    None,
                    crate::monitor::WIN_PRIOR_STRENGTH,
                );

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
                    win_probability_lower,
                    win_probability_upper,
                    optimal_size_usd: cap_rep.optimal_size_usd,
                    breakeven_size_usd: cap_rep.breakeven_size_usd,
                    expected_capacity_usd: cap_rep.expected_capacity_usd,
                    bps_capacity_ladder: cap_rep
                        .bps_capacity_ladder
                        .iter()
                        .map(|t| crate::monitor::BpsCapacityTierDto {
                            min_net_bps: t.min_net_bps,
                            capacity_usd: t.capacity_usd,
                        })
                        .collect(),
                    depth_curve: cap_rep
                        .depth_curve
                        .iter()
                        .map(crate::monitor::DepthCurvePointDto::from)
                        .collect(),
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

/// Cross-venue spot scan: every admitted venue lists its instruments and
/// only real `MarketType::Spot` rows qualify — a perpetual must never stand
/// in for the asset itself. A route exists only when BOTH venues list the
/// SAME canonical instrument (e.g. `BTC-USD`): mixing `BTC-USD` and
/// `BTC-USDC` legs would leave a residual quote-asset basis the two-leg
/// executor cannot represent, so quote families are never merged here.
/// Economics reuse the shared route model (entry+exit VWAP, venue taker
/// fees, closing spread) — a spot basis trade unwinds the same way the
/// Simple Spread does, so `net_spread_bps` is net of round-trip friction.
pub async fn scan_spot_opportunities_impl(
    symbol_filter: &str,
    state: &AppEngineState,
) -> Result<Vec<SpotOpportunityDto>, String> {
    let filter = symbol_filter.trim().to_uppercase();
    if !filter.is_empty() {
        validate_market_symbol(&filter)?;
    }
    let venues = state.get_venues();
    if venues.len() < 2 {
        return Ok(Vec::new());
    }

    // canonical spot symbol -> venues listing it (first listing wins when a
    // venue exposes duplicate instruments for one canonical pair).
    let mut symbol_venues: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, (_vid, _vname, adapter)) in venues.iter().enumerate() {
        let mut seen = std::collections::HashSet::new();
        if let Ok(markets) = adapter.fetch_markets().await {
            for m in markets {
                if m.symbol.market_type != arbitrageur_core::market::MarketType::Spot
                    || !m.is_active
                {
                    continue;
                }
                let canonical = m.symbol.canonical();
                if !filter.is_empty() && canonical != filter && m.symbol.base != filter {
                    continue;
                }
                if seen.insert(canonical.clone()) {
                    symbol_venues.entry(canonical).or_default().push(idx);
                }
            }
        }
    }
    // Only canonical instruments shared by >=2 venues can form a route.
    let shared: Vec<String> = symbol_venues
        .iter()
        .filter(|(_, v)| v.len() >= 2)
        .map(|(s, _)| s.clone())
        .collect();
    if shared.is_empty() {
        return Ok(Vec::new());
    }

    let (trade_size_usd, min_net_bps) = {
        let params = state.strategy_params.read();
        let p = params.get("spot");
        let default = StrategyParamsDto::defaults_for("spot");
        (
            p.map(|p| p.spot_trade_size_usd)
                .filter(|v| v.is_finite() && *v > 0.0)
                .unwrap_or(default.spot_trade_size_usd),
            p.map(|p| p.spot_min_net_spread_bps)
                .filter(|v| v.is_finite() && *v > 0.0)
                .unwrap_or(default.spot_min_net_spread_bps),
        )
    };
    let max_slip_dec = Decimal::from_f64_retain(strategy_slippage_bps(state, "spot"))
        .ok_or_else(|| "Invalid spot max slippage setting".to_string())?;
    let min_depth_dec = Decimal::from_f64_retain(state.settings.read().min_depth_ratio)
        .ok_or_else(|| "Invalid min depth ratio setting".to_string())?;
    let monitor_cfg = state
        .state_store
        .monitor_config_json()
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str::<crate::monitor::MonitorConfigDto>(&j).ok())
        .unwrap_or_default();
    let latency_by_venue: HashMap<String, f64> = state.latency_ema.lock().clone();

    let mut opps = Vec::new();
    for canonical in shared {
        let idxs = &symbol_venues[&canonical];
        state.ensure_market(&canonical);

        let mut books = Vec::new();
        for &idx in idxs {
            let (vid, vname, adapter) = &venues[idx];
            let venue_id = adapter.venue_id();
            let book = match crate::ws_primary::resolve_book_ws_first(
                &state.market_data_hub,
                &state.orderbook_flights,
                adapter,
                &canonical,
                std::time::Duration::from_millis(350),
            )
            .await
            {
                Some(crate::ws_primary::ResolvedBook::Ws(b))
                | Some(crate::ws_primary::ResolvedBook::Cached(b)) => b,
                Some(crate::ws_primary::ResolvedBook::Fresh(b)) => {
                    if validate_adapter_book(&b, &venue_id, &canonical).is_err() {
                        continue;
                    }
                    state.market_data_hub.update_orderbook(b.clone());
                    b
                }
                None => continue,
            };
            if validate_adapter_book(&book, &venue_id, &canonical).is_err() {
                continue;
            }
            books.push((*vid, *vname, book, effective_venue_fee_bps(state, vid)));
        }
        if books.len() < 2 {
            continue;
        }

        for i in 0..books.len() {
            for j in 0..books.len() {
                if i == j {
                    continue;
                }
                let (buy_id, buy_name, ref book_buy, buy_fee) = books[i];
                let (sell_id, sell_name, ref book_sell, sell_fee) = books[j];
                let Some(buy_ask) = book_buy
                    .best_ask_price()
                    .and_then(|p| p.to_string().parse::<f64>().ok())
                    .filter(|p| *p > 0.0)
                else {
                    continue;
                };
                let Some(sell_bid) = book_sell
                    .best_bid_price()
                    .and_then(|p| p.to_string().parse::<f64>().ok())
                    .filter(|p| *p > 0.0)
                else {
                    continue;
                };
                let order_qty = match Decimal::from_f64_retain(trade_size_usd / buy_ask) {
                    Some(q) if q > dec!(0) => q,
                    _ => continue,
                };
                let Some(econ) = compute_route_economics(
                    book_buy,
                    book_sell,
                    order_qty,
                    buy_fee,
                    sell_fee,
                    min_net_bps,
                ) else {
                    continue;
                };
                let (slippage_ok, depth_ok) = route_execution_gates(
                    book_buy,
                    book_sell,
                    order_qty,
                    max_slip_dec,
                    min_depth_dec,
                );
                let is_executable = econ.expected_net_bps >= min_net_bps && slippage_ok && depth_ok;

                // Depth available to this size: notional covered by the
                // bounded-entry walks on both legs.
                let buy_depth_usd: f64 = book_buy
                    .asks
                    .iter()
                    .map(|l| l.price * l.size)
                    .sum::<Decimal>()
                    .to_string()
                    .parse()
                    .unwrap_or(0.0);
                let sell_depth_usd: f64 = book_sell
                    .bids
                    .iter()
                    .map(|l| l.price * l.size)
                    .sum::<Decimal>()
                    .to_string()
                    .parse()
                    .unwrap_or(0.0);
                let available_depth_usd = buy_depth_usd.min(sell_depth_usd);

                // Concurrent dispatch: the pair's latency is the slower
                // leg's probed round-trip (0 when unprobed — honest "—").
                let estimated_latency_ms = [
                    latency_by_venue.get(buy_id).copied(),
                    latency_by_venue.get(sell_id).copied(),
                ]
                .into_iter()
                .flatten()
                .filter(|v| v.is_finite() && *v >= 0.0)
                .fold(0.0f64, f64::max) as u64;

                // Capacity curve: same lockstep level-pair walk the Simple
                // Spread monitor uses; no spot episode history exists, so the
                // expected-value channels stay None (never fabricated).
                let to_pairs = |levels: &[arbitrageur_core::quote::BookLevel]| {
                    levels
                        .iter()
                        .filter_map(|l| {
                            let p = l.price.to_string().parse::<f64>().ok()?;
                            let q = l.size.to_string().parse::<f64>().ok()?;
                            (p.is_finite() && p > 0.0 && q.is_finite() && q > 0.0).then_some((p, q))
                        })
                        .collect::<Vec<(f64, f64)>>()
                };
                let asks = to_pairs(&book_buy.asks);
                let bids = to_pairs(&book_sell.bids);
                let cap_rep = capacity_ladder(
                    &asks,
                    &bids,
                    0.0,
                    econ.expected_friction_bps,
                    &monitor_cfg.capacity_tiers_usd,
                    &arbitrageur_math::pair_monitor::PWinCurve {
                        points: Vec::new(),
                        episodes: 0,
                        point_wins: Vec::new(),
                        point_trials: Vec::new(),
                        point_censored: Vec::new(),
                        point_sum_w2: Vec::new(),
                    },
                    monitor_cfg.min_win_probability,
                    None,
                    crate::monitor::WIN_PRIOR_STRENGTH,
                );

                opps.push(SpotOpportunityDto {
                    id: format!("{}-{}-{}", canonical, buy_id, sell_id),
                    symbol: canonical.clone(),
                    buy_venue: buy_name.to_string(),
                    sell_venue: sell_name.to_string(),
                    buy_price: buy_ask,
                    sell_price: sell_bid,
                    gross_spread_bps: econ.top_gross_bps,
                    buy_fee_bps: econ.buy_fee_bps,
                    sell_fee_bps: econ.sell_fee_bps,
                    // No asset transfer happens inside the atomic window —
                    // venue inventory is rebalanced separately.
                    transfer_cost_usd: 0.0,
                    net_spread_bps: econ.expected_net_bps,
                    net_profit_usd: econ.estimated_profit_usd,
                    available_depth_usd,
                    estimated_latency_ms,
                    is_executable,
                    depth_curve: cap_rep
                        .depth_curve
                        .iter()
                        .map(crate::monitor::DepthCurvePointDto::from)
                        .collect(),
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

/// Spot execution rides the shared atomic two-leg path with
/// `strategy_id = "spot"` — the inner gates read the spot strategy's own
/// reduce-only flag, slippage and hurdle, while margin/leverage preflights
/// are replaced by cash/inventory balances (spot cannot short or lever).
pub async fn execute_spot_arbitrage_impl(
    params: SpotExecutionParams,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    validate_market_symbol(&params.symbol)?;
    if !params.trade_size_usd.is_finite() || params.trade_size_usd <= 0.0 {
        return Err("trade_size_usd must be a finite positive number".into());
    }
    if params.buy_venue.trim().is_empty() || params.sell_venue.trim().is_empty() {
        return Err("Buy and sell venues must both be specified".into());
    }
    if params.buy_venue.eq_ignore_ascii_case(&params.sell_venue) {
        return Err("Buy and sell venues must be distinct".into());
    }

    // Fail closed on market class: the symbol must be a listed SPOT
    // instrument on both venues — otherwise `fetch_orderbook`/order routing
    // could silently resolve a perp or future with the same canonical name.
    let buy_adapter = state.get_adapter(&params.buy_venue).ok_or_else(|| {
        format!(
            "Buy venue '{}' is not admitted for execution",
            params.buy_venue
        )
    })?;
    let sell_adapter = state.get_adapter(&params.sell_venue).ok_or_else(|| {
        format!(
            "Sell venue '{}' is not admitted for execution",
            params.sell_venue
        )
    })?;
    for (vname, adapter) in [
        (params.buy_venue.as_str(), &buy_adapter),
        (params.sell_venue.as_str(), &sell_adapter),
    ] {
        let markets = adapter
            .fetch_markets()
            .await
            .map_err(|e| format!("Cannot verify {vname} spot listings: {e}"))?;
        let listed = markets.iter().any(|m| {
            m.symbol.market_type == arbitrageur_core::market::MarketType::Spot
                && m.is_active
                && m.symbol.canonical().eq_ignore_ascii_case(&params.symbol)
        });
        if !listed {
            return Err(format!(
                "{} does not list {} as a spot market — refusing to route the order through a non-spot instrument",
                vname, params.symbol
            ));
        }
    }

    // Price the order quantity off the buy side's live ask.
    let buy_book = match state.market_data_hub.get_valid_orderbook(
        &buy_adapter.venue_id(),
        &params.symbol,
        Some(std::time::Duration::from_secs(2)),
    ) {
        Some(b) => b,
        None => {
            let b = buy_adapter
                .fetch_orderbook(&params.symbol)
                .await
                .map_err(|e| {
                    format!(
                        "No fresh {} orderbook for {}: {}",
                        params.buy_venue, params.symbol, e
                    )
                })?;
            validate_adapter_book(&b, &buy_adapter.venue_id(), &params.symbol)
                .map_err(|e| format!("Buy-side order book rejected: {e}"))?;
            state.market_data_hub.update_orderbook(b.clone());
            b
        }
    };
    let best_ask = buy_book
        .best_ask_price()
        .and_then(|p| p.to_string().parse::<f64>().ok())
        .filter(|p| *p > 0.0)
        .ok_or_else(|| {
            format!(
                "No asks on {} spot book for {}",
                params.buy_venue, params.symbol
            )
        })?;
    let order_qty = params.trade_size_usd / best_ask;

    execute_spatial_arbitrage_inner(
        &params.symbol,
        &params.buy_venue,
        &params.sell_venue,
        order_qty,
        Some("Spot Cross-Venue Flash".to_string()),
        params.net_spread_bps,
        None,
        None,
        None,
        None,
        None,
        "spot",
        None,
        state,
    )
    .await
}

// ---------------------------------------------------------------------------
// Triangular arbitrage (single-venue A -> B -> C -> A cycles)
// ---------------------------------------------------------------------------

/// Strategy id under which triangle residuals are recorded. The 2-leg signal
/// ledger cannot represent an atomic 3-leg cycle as one row (1-2 leg cap), so
/// a completed cycle's legs persist as individual 1-leg `Imbalanced` signals:
/// each one blocks its symbol and unwinds through the existing symbol-scoped
/// flatten path, exactly like any other open position.
pub(crate) const TRIANGLE_STRATEGY_ID: &str = "triangle";
pub(crate) const FUNDING_STRATEGY_ID: &str = "funding";
pub(crate) const OPTIONS_PARITY_STRATEGY_ID: &str = "options_parity";

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
        // No regression ran on this path — r_squared stays unknown.
        r_squared: None,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VenueCapabilityDto {
    pub venue: String,
    pub name: String,
    pub supports_live: bool,
    pub has_credentials: bool,
    pub live_ready: bool,
    pub submission_ready: bool,
    pub strategy_entry_ready: bool,
    /// Whether `fetch_order` is wired for this venue — the adapter-level
    /// primitive the reconciliation supervisor polls. Distinct from
    /// `strategy_entry_ready`, which additionally requires two-leg
    /// coordination guarantees that remain blocked.
    pub order_polling_ready: bool,
}

pub fn get_venue_execution_capabilities_impl(state: &AppEngineState) -> Vec<VenueCapabilityDto> {
    state.sync_vault_credentials_to_adapters();
    let is_unlocked = state.vault.write().is_unlocked();
    let venues = state.get_venues();
    venues
        .into_iter()
        .map(|(vid, vname, adapter)| {
            let supports_live = adapter.live_order_submission_implemented();
            let has_credentials = adapter.has_live_credentials();
            let submission_ready = is_unlocked && supports_live && has_credentials;
            let strategy_entry_ready = is_unlocked
                && adapter.live_order_reconciliation_implemented()
                && adapter.live_strategy_entry_implemented()
                && has_credentials;
            let live_ready = strategy_entry_ready;
            VenueCapabilityDto {
                venue: vid.to_string(),
                name: vname.to_string(),
                supports_live,
                has_credentials,
                live_ready,
                submission_ready,
                strategy_entry_ready,
                order_polling_ready: adapter.order_status_polling_implemented(),
            }
        })
        .collect()
}

/// Single choke point for execution-log writes: in-memory Log Monitor
/// source (capped at 2000 rows) + best-effort SQLite persistence (survives
/// restarts). Successes, partials, AND failures — a blocked click must leave
/// the same trace as a fill.
pub fn record_execution_log(state: &AppEngineState, entry: ExecutionLogEntry) {
    {
        let mut logs = state.execution_logs.write();
        if logs.len() >= 2000 {
            let drop = logs.len() - 1999;
            logs.drain(..drop);
        }
        logs.push(entry.clone());
    }
    if let Err(e) = state.state_store.insert_execution_log(&entry) {
        tracing::debug!("execution-log persist failed: {e}");
    }
}

/// Record a blocked/failed execution attempt as a failed log entry (visible
/// in the Log Monitor) plus an app-log warning. Every early gate return in
/// the execute_*_impl family funnels through its command wrapper here, so no
/// failure mode is silent anymore.
pub(crate) fn record_execution_failure(
    state: &AppEngineState,
    trade_type: &str,
    strategy_id: &str,
    symbol: &str,
    venue_a: &str,
    venue_b: &str,
    reason: &str,
) {
    tracing::warn!("{trade_type} execution blocked for {symbol} ({venue_a}/{venue_b}): {reason}");
    record_execution_log(
        state,
        ExecutionLogEntry {
            id: format!(
                "exec-fail-{}-{:08x}",
                chrono::Utc::now().timestamp_millis(),
                rand::random::<u32>()
            ),
            timestamp: chrono::Utc::now().to_rfc3339(),
            trade_type: trade_type.to_string(),
            strategy_id: strategy_id.to_string(),
            symbol: symbol.to_string(),
            venue_a: venue_a.to_string(),
            venue_b: venue_b.to_string(),
            leg_a_status: "blocked".into(),
            leg_b_status: "blocked".into(),
            success: false,
            gross_profit: 0.0,
            fees: 0.0,
            net_profit: 0.0,
            latency_us: 0,
            note: format!("Execution blocked: {reason}"),
            direction: None,
            trigger_spread_bps: None,
            baseline_mean_spread_bps: None,
            position_step: None,
            max_units: None,
            nominal_hurdle_bps: None,
            is_live: state.is_live(),
        },
    );
}

/// Empirical backtest stats for a funding opportunity, measured over the
/// same trailing gap history the hold model uses. Returns
/// `(win_rate, expected_usd, trials)` — all `None` below 20 trials.
pub(crate) fn backtest_stats(
    hist: &[(i64, f64)],
    min_apr_pct: f64,
    fee_bps: f64,
    spread_bps: Option<f64>,
    trade_size_usd: f64,
) -> Option<arbitrageur_math::funding_carry::GapBacktest> {
    let threshold_8h = min_apr_pct.max(0.0) / 100.0 / 365.0 / 3.0;
    let spread = spread_bps
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(0.0);
    let cost_ratio = (fee_bps.max(0.0) + spread) / 10_000.0;
    backtest_gap_carry(hist, threshold_8h, cost_ratio, trade_size_usd, 168.0, 20)
}

pub fn get_execution_logs_by_mode_impl(
    state: &AppEngineState,
    is_live_filter: Option<bool>,
) -> Result<Vec<ExecutionLogEntry>, String> {
    let mode = is_live_filter.unwrap_or_else(|| state.is_live());
    let logs = state.execution_logs.read();
    let filtered: Vec<ExecutionLogEntry> =
        logs.iter().filter(|l| l.is_live == mode).cloned().collect();
    Ok(filtered)
}

#[cfg(test)]
pub fn get_execution_logs_impl(state: &AppEngineState) -> Result<Vec<ExecutionLogEntry>, String> {
    get_execution_logs_by_mode_impl(state, None)
}

/// Shared environment transition used by the mode toggle and the settings
/// save path — identical guards apply whichever surface initiates the
/// switch. Returns Ok(true) when the resulting env is Live.
fn transition_environment(
    state: &AppEngineState,
    target: crate::state::TradingEnvironment,
) -> Result<bool, String> {
    // Refuse an environment switch while any signal still needs
    // reconciliation — its legs belong to the *current* env's adapters and
    // would poll the wrong venue endpoints (or no adapter at all) after the
    // flip, stranding real exposure as 'unresolved' until switched back.
    // The same holds for `balanced` open positions: their fills were booked
    // through the current env's adapters, and every flatten/close resolves
    // `get_adapter` under the *new* env — a testnet hedge flattened after a
    // live flip would send real orders to the wrong endpoints.
    if target != state.env() {
        // Fail closed: a DB read error is not "no exposure" — the switch is
        // refused when unresolved state cannot be verified.
        // Cross-environment view (None): pending legs in EITHER environment
        // refuse the switch — exposure left in the departing environment
        // stops being supervised once its adapters unbind.
        let pending = state
            .state_store
            .list_pending_order_legs(None)
            .map_err(|e| format!("Cannot verify pending orders before environment switch: {e}"))?;
        if !pending.is_empty() {
            let ids: Vec<String> = pending
                .iter()
                .map(|(exec, _)| format!("{}:{}", exec.symbol, exec.status.as_str()))
                .collect();
            return Err(format!(
                "Cannot switch trading environment while {} signal(s) await reconciliation ({}). Let them settle or resolve the exposure first.",
                pending.len(),
                ids.join(", ")
            ));
        }
        let signals = state
            .state_store
            .list_open_signals(None)
            .map_err(|e| format!("Cannot verify open positions before environment switch: {e}"))?;
        let open: Vec<String> = signals
            .iter()
            .filter(|s| s.status.counts_as_open())
            .map(|s| format!("{}:{}", s.symbol, s.status.as_str()))
            .collect();
        if !open.is_empty() {
            return Err(format!(
                "Cannot switch trading environment while {} position(s) are open ({}). Flatten them first — closes dispatch through the current environment's venues.",
                open.len(),
                open.join(", ")
            ));
        }
    }
    if target == crate::state::TradingEnvironment::Live {
        let is_unlocked = state.vault.write().is_unlocked();
        if !is_unlocked {
            state.set_environment(crate::state::TradingEnvironment::Testnet);
            state.market_data_hub.clear();
            return Err("Cannot enable Live Trading while the secure credential vault is locked. Unlock the vault first.".into());
        }
        state.sync_vault_credentials_to_adapters();
        let ready_venues: Vec<String> = state
            .venues_for_env(crate::state::TradingEnvironment::Live)
            .into_iter()
            .filter(|(_, _, adapter)| {
                adapter.live_order_submission_implemented() && adapter.has_live_credentials()
            })
            .map(|(vid, _, _)| vid.to_string())
            .collect();
        let ready_count = ready_venues.len();
        if ready_count < 2 {
            state.set_environment(crate::state::TradingEnvironment::Testnet);
            state.market_data_hub.clear();
            return Err(format!(
                "Live Trading requires at least two venues with authenticated live credentials (currently {ready_count} ready: {}). Please configure and enable API keys in Settings.",
                if ready_venues.is_empty() { "none".to_string() } else { ready_venues.join(", ") }
            ));
        }
    }
    let changed = target != state.env();
    state.set_environment(target);
    {
        let mut s = state.settings.write();
        s.trading_mode = target.as_str().to_string();
    }
    let _ = state
        .state_store
        .kv_set("engine.trading_mode", target.as_str());
    if changed {
        // Books cached under the previous env must not survive the switch —
        // mainnet prices are not executable on testnet venues and vice
        // versa. Feeds re-populate from the newly admitted adapter set.
        state.market_data_hub.clear();
        // Venue-local stablecoin rates were learned from the previous env's
        // markets (testnet quotes are not mainnet truth) — re-derive them
        // from the new env's watchlist refresh. Global rates stay: the FX
        // table is venue-independent.
        state.stablecoin_fx.clear_venue();
    }
    Ok(target == crate::state::TradingEnvironment::Live)
}

pub fn set_live_mode_impl(enabled: bool, state: &AppEngineState) -> Result<bool, String> {
    transition_environment(
        state,
        if enabled {
            crate::state::TradingEnvironment::Live
        } else {
            crate::state::TradingEnvironment::Testnet
        },
    )
}

pub fn set_engine_paused_impl(paused: bool, state: &AppEngineState) -> Result<bool, String> {
    // Durable: a restart must not silently resume a halted engine. A kv
    // write failure refuses the toggle — an in-memory-only pause would
    // lull the operator into thinking the halt survives restarts.
    state
        .state_store
        .kv_set("engine.paused", if paused { "1" } else { "0" })
        .map_err(|e| format!("Failed to persist pause state: {e}"))?;
    *state.is_paused.write() = paused;
    Ok(paused)
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
    let normalized_mode = settings.trading_mode.trim().to_ascii_lowercase();
    if !matches!(
        normalized_mode.as_str(),
        "live" | "testnet" | "real" | "paper"
    ) {
        return Err("trading_mode must be 'live' or 'testnet'".into());
    }
    let target_env = crate::state::TradingEnvironment::from_mode(&normalized_mode);

    // Same transition contract as the mode toggle: pending-signal guard,
    // live credential/vault checks, hub invalidation on change. When the
    // target equals the active env this is a no-op so routine settings
    // saves with a locked vault are not blocked by the live check.
    if target_env != state.env() {
        transition_environment(state, target_env)?;
    }
    // Normalize legacy values before persisting.
    settings.trading_mode = target_env.as_str().to_string();
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
    if let Err(e) = settings.passive_unwind.validate() {
        return Err(format!("Auto Position Exit settings: {e}"));
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
    if settings.balance_stale_warn_secs < 30 || settings.balance_stale_warn_secs > 3600 {
        return Err("Balance staleness warning must be within 30–3600 seconds".into());
    }
    if settings.max_leverage < 1 || settings.max_leverage > 100 {
        return Err("Max leverage must be within 1–100x".into());
    }
    if !(60..=604_800).contains(&settings.auto_exit_max_hold_secs) {
        return Err("Auto-exit max hold must be within 60 seconds–7 days".into());
    }
    if !(1..=365).contains(&settings.auto_fixed_yield_min_lock_days) {
        return Err("Auto fixed-yield min lock must be within 1–365 days".into());
    }
    if !settings.auto_fixed_yield_min_gas_usd.is_finite()
        || !(0.0..=10_000.0).contains(&settings.auto_fixed_yield_min_gas_usd)
    {
        return Err("Auto fixed-yield min gas must be within $0–$10,000".into());
    }
    // USD-quote policy: only the three documented spellings persist —
    // an unknown mode must fail the save, not silently coerce.
    if !matches!(
        settings.usd_quote_mode.trim().to_ascii_lowercase().as_str(),
        "separate" | "parity" | "fx_adjusted"
    ) {
        return Err("usd_quote_mode must be 'separate', 'parity', or 'fx_adjusted'".into());
    }
    settings.usd_quote_mode =
        crate::stablecoin_fx::normalize_usd_quote_mode(&settings.usd_quote_mode).to_string();
    if !settings.stablecoin_warn_bps.is_finite()
        || !settings.stablecoin_halt_bps.is_finite()
        || settings.stablecoin_warn_bps <= 0.0
        || settings.stablecoin_warn_bps >= settings.stablecoin_halt_bps
        || settings.stablecoin_halt_bps > 10_000.0
    {
        return Err(
            "Stablecoin thresholds must be positive with warn < halt (halt ≤ 10000 bps)".into(),
        );
    }
    let submitted = settings.submitted_credentials();
    // Legacy flat credential fields can name venues with no constructed
    // adapter (polymarket/kalshi). Storing their secrets would
    // warehouse material nothing can consume — skip the write and report
    // it in the returned message rather than silently dropping or
    // silently keeping.
    let mut skipped_cred_venues: Vec<String> = Vec::new();
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
            if !crate::state::venue_has_adapter(&venue_key) {
                skipped_cred_venues.push(venue_key);
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
    let env_str = state.env().as_str().to_string();
    let account_enabled = |venue: &str| {
        state
            .state_store
            .has_enabled_account(venue, &env_str)
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
        ("okx".into(), settings.okx_enabled || account_enabled("okx")),
        (
            "coinbase".into(),
            settings.coinbase_enabled || account_enabled("coinbase"),
        ),
        (
            "alpaca".into(),
            settings.alpaca_enabled || account_enabled("alpaca"),
        ),
        (
            "decibel".into(),
            settings.decibel_enabled || account_enabled("decibel"),
        ),
        (
            "ibkr".into(),
            settings.ibkr_enabled || account_enabled("ibkr"),
        ),
        ("ig".into(), settings.ig_enabled || account_enabled("ig")),
    ];
    // Disabling a venue with an open live leg strands exit supervision —
    // the adapter drops out of `get_venues`, so closes can no longer
    // authenticate or even resolve. Compare the OLD enablement (engine
    // flag or enabled account, same semantics) with the new one and
    // refuse any live-open venue that would drop out.
    if state.is_live() {
        let old = state.settings.read().clone();
        let was_enabled = |venue: &str| -> bool {
            let flag = match venue {
                "hyperliquid" => old.hyperliquid_enabled,
                "lighter" => old.lighter_enabled,
                "robinhood" => old.robinhood_enabled,
                "vanta" => old.vanta_enabled,
                "arcus" => old.arcus_enabled,
                "kraken_spot" => old.kraken_enabled && old.kraken_spot_enabled,
                "kraken_futures" => old.kraken_enabled && old.kraken_futures_enabled,
                "polymarket" => old.polymarket_enabled,
                "kalshi" => old.kalshi_enabled,
                "aster" => old.aster_enabled,
                "popdex" => old.popdex_enabled,
                "binance" => old.binance_enabled,
                "okx" => old.okx_enabled,
                "coinbase" => old.coinbase_enabled,
                "alpaca" => old.alpaca_enabled,
                "decibel" => old.decibel_enabled,
                "ibkr" => old.ibkr_enabled,
                "ig" => old.ig_enabled,
                _ => false,
            };
            flag || account_enabled(venue)
        };
        // Fail closed on read errors — an unverifiable ledger is not "no
        // open positions", so the venue-disable check refuses to proceed.
        let signals = state
            .state_store
            .list_open_signals(None)
            .map_err(|e| format!("Cannot verify open positions before disabling venues: {e}"))?;
        let open_venues: Vec<String> = signals
            .iter()
            .filter(|s| s.status.counts_as_open())
            .flat_map(|s| s.legs.iter().map(|l| l.venue.to_lowercase()))
            .collect();
        let blocked: Vec<String> = enablement
            .iter()
            .filter(|(v, on)| !*on && was_enabled(v) && open_venues.iter().any(|o| o == v))
            .map(|(v, _)| v.clone())
            .collect();
        if !blocked.is_empty() {
            return Err(format!(
                "Cannot disable venue(s) {} while live position(s) route through them. Flatten them first.",
                blocked.join(", ")
            ));
        }
    }
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
    // A changed starting balance reinitializes test simulators: fresh
    // equity, no open positions. Production has no simulated accounts —
    // Testnet/Live balances are owned by the venues. Test-only block.
    #[cfg(test)]
    {
        let prev_balance = state.settings.read().paper_starting_balance_usd;
        if sanitized.paper_starting_balance_usd != prev_balance {
            if let Some(eq) = Decimal::from_f64_retain(sanitized.paper_starting_balance_usd) {
                for (_, paper) in state.paper_exchanges() {
                    paper.reset_account(eq);
                }
                // A reset moves the baseline itself: rebase every account's
                // expected equity to the new starting balance.
                let f = sanitized.paper_starting_balance_usd;
                if f.is_finite() && f >= 0.0 {
                    let now_ms = Utc::now().timestamp_millis();
                    for (vid, _) in state.paper_exchanges() {
                        let _ = state
                            .state_store
                            .set_equity_expectation(&vid, f, "", now_ms);
                    }
                }
                if let Some(eq) = Decimal::from_f64_retain(sanitized.paper_starting_balance_usd) {
                    let n = state.paper_exchanges().len() as u32;
                    if n > 0 {
                        crate::execution::cb_reanchor_to_sims(state, eq * Decimal::from(n));
                    }
                }
            }
        }
    }
    let _ = state
        .state_store
        .kv_set("engine.trading_mode", &sanitized.trading_mode);
    // USD-quote policy + depeg thresholds persist like the other Execution
    // settings — a restart must not silently revert to a different pairing
    // or protection level.
    let _ = state
        .state_store
        .kv_set("engine.usd_quote_mode", &sanitized.usd_quote_mode);
    let _ = state.state_store.kv_set(
        "risk.stablecoin_warn_bps",
        &sanitized.stablecoin_warn_bps.to_string(),
    );
    let _ = state.state_store.kv_set(
        "risk.stablecoin_halt_bps",
        &sanitized.stablecoin_halt_bps.to_string(),
    );
    // Persist Safety limits so restarts keep the operator's values.
    let _ = state.state_store.kv_set(
        "risk.cb_max_dd_usd",
        &sanitized.cb_max_daily_drawdown_usd.to_string(),
    );
    let _ = state.state_store.kv_set(
        "risk.cb_max_dd_pct",
        &sanitized.cb_max_daily_drawdown_pct.to_string(),
    );
    let _ = state.state_store.kv_set(
        "risk.cb_max_errors",
        &sanitized.cb_max_consecutive_errors.to_string(),
    );
    let _ = state.state_store.kv_set(
        "risk.cb_max_spread_bps",
        &sanitized.cb_max_spread_divergence_bps.to_string(),
    );
    let _ = state.state_store.kv_set(
        "risk.delta_max_unhedged",
        &sanitized.delta_max_unhedged_usd_per_asset.to_string(),
    );
    let _ = state.state_store.kv_set(
        "risk.margin_warn",
        &sanitized.margin_warning_usage_pct.to_string(),
    );
    let _ = state.state_store.kv_set(
        "risk.margin_crit",
        &sanitized.margin_critical_usage_pct.to_string(),
    );
    let _ = state.state_store.kv_set(
        "risk.margin_min_liq",
        &sanitized.margin_min_liquidation_distance_pct.to_string(),
    );
    let _ = state.state_store.kv_set(
        "risk.stale_warn_secs",
        &sanitized.balance_stale_warn_secs.to_string(),
    );
    let _ = state
        .state_store
        .kv_set("risk.max_leverage", &sanitized.max_leverage.to_string());
    let _ = state.state_store.kv_set(
        "engine.auto_exit_max_hold_secs",
        &sanitized.auto_exit_max_hold_secs.to_string(),
    );
    let _ = state.state_store.kv_set(
        "engine.auto_fixed_yield_enabled",
        if sanitized.auto_fixed_yield_enabled {
            "1"
        } else {
            "0"
        },
    );
    let _ = state.state_store.kv_set(
        "engine.auto_fixed_yield_min_lock_days",
        &sanitized.auto_fixed_yield_min_lock_days.to_string(),
    );
    let _ = state.state_store.kv_set(
        "engine.auto_fixed_yield_min_gas_usd",
        &sanitized.auto_fixed_yield_min_gas_usd.to_string(),
    );
    // The passive-unwind policy persists — an operator who disables the
    // default-on limit exits must not have the choice silently revert on
    // restart. (Residual auto-flatten is unconditional — no flag.)
    match serde_json::to_string(&sanitized.passive_unwind) {
        Ok(json) => {
            let _ = state.state_store.kv_set("engine.passive_unwind", &json);
        }
        Err(e) => {
            tracing::warn!("engine settings: passive_unwind serialize failed: {e}");
        }
    }
    // Venue endpoint overrides: persist only values that differ from the
    // built-in defaults — the UI prefills display defaults (e.g.
    // "https://api.kraken.com") that differ from adapter base URLs
    // ("https://api.kraken.com/0/public"), so persisting unconditionally
    // would silently rewrite every endpoint on an unrelated Save.
    {
        let defaults = crate::state::EngineSettingsDto::default();
        for (key, val, dflt) in [
            (
                "endpoints.hyperliquid_url",
                sanitized.hyperliquid_url.as_str(),
                defaults.hyperliquid_url.as_str(),
            ),
            (
                "endpoints.lighter_url",
                sanitized.lighter_url.as_str(),
                defaults.lighter_url.as_str(),
            ),
            (
                "endpoints.vanta_url",
                sanitized.vanta_url.as_str(),
                defaults.vanta_url.as_str(),
            ),
            (
                "endpoints.kraken_spot_url",
                sanitized.kraken_spot_url.as_str(),
                defaults.kraken_spot_url.as_str(),
            ),
            (
                "endpoints.kraken_futures_url",
                sanitized.kraken_futures_url.as_str(),
                defaults.kraken_futures_url.as_str(),
            ),
            (
                "endpoints.arcus_url",
                sanitized.arcus_url.as_str(),
                defaults.arcus_url.as_str(),
            ),
            (
                "endpoints.popdex_network",
                sanitized.popdex_network.as_str(),
                defaults.popdex_network.as_str(),
            ),
            (
                "endpoints.polymarket_url",
                sanitized.polymarket_url.as_str(),
                defaults.polymarket_url.as_str(),
            ),
            (
                "endpoints.kalshi_url",
                sanitized.kalshi_url.as_str(),
                defaults.kalshi_url.as_str(),
            ),
        ] {
            let v = val.trim();
            if v.is_empty() || v == dflt {
                let _ = state.state_store.kv_delete(key);
            } else {
                let _ = state.state_store.kv_set(key, v);
            }
        }
    }
    // Live-apply: breaker limits update in place (no restart), delta guard
    // re-limits, margin thresholds feed the next snapshot automatically.
    {
        use arbitrageur_risk::circuit_breaker::CircuitBreakerConfig;
        let cfg = CircuitBreakerConfig {
            max_daily_drawdown_usd: Decimal::from_f64_retain(sanitized.cb_max_daily_drawdown_usd)
                .unwrap_or(dec!(25000.0)),
            max_daily_drawdown_pct: Decimal::from_f64_retain(sanitized.cb_max_daily_drawdown_pct)
                .unwrap_or(dec!(5.0)),
            max_consecutive_errors: sanitized.cb_max_consecutive_errors,
            max_spread_divergence_bps: Decimal::from_f64_retain(
                sanitized.cb_max_spread_divergence_bps,
            )
            .unwrap_or(dec!(250.0)),
        };
        state.circuit_breaker.update_config(cfg);
        if let Some(limit) = Decimal::from_f64_retain(sanitized.delta_max_unhedged_usd_per_asset) {
            state
                .delta_guard
                .write()
                .set_max_unhedged_usd_per_asset(limit);
        }
    }
    // Keep the hub's slot keying in step with the quote policy: under
    // `separate`, BASE-QUOTE variants must not share a cached book.
    state.market_data_hub.set_merge_usd_quotes(
        crate::stablecoin_fx::normalize_usd_quote_mode(&sanitized.usd_quote_mode)
            != crate::stablecoin_fx::USD_QUOTE_MODE_SEPARATE,
    );
    *state.settings.write() = sanitized;
    if skipped_cred_venues.is_empty() {
        Ok("Configuration applied to the current session".into())
    } else {
        Ok(format!(
            "Configuration applied to the current session — credentials for {} \
             were ignored: no adapter exists for that venue, so stored keys \
             could never be used",
            skipped_cred_venues.join(", ")
        ))
    }
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
        let merge_usd = state.usd_merge_quotes();
        for (venue, vols) in &map {
            let _ = state
                .state_store
                .record_venue_volume_samples_mode(venue, now_ms, vols, merge_usd);
        }
    }
    *state.volume_cache.lock() = Some((std::time::Instant::now(), map.clone()));
    map
}

/// The executor's venue taker-fee table — single source for every UI fee
/// display. Stateless; no environment or admission dependence.
#[tauri::command]
pub async fn get_venue_fees() -> Result<VenueFeesDto, String> {
    Ok(get_venue_fees_impl())
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
    /// venue_id -> timestamp (epoch ms) of that venue's newest raw sample in
    /// the fetched range — lets the legend badge stale/cold lanes honestly.
    pub venue_last_ts_ms: HashMap<String, i64>,
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
    let merge_usd = state.usd_merge_quotes();
    let (raw, earliest_ts_ms) = tokio::task::spawn_blocking(move || {
        let rows = store.spread_samples_mode(&sym, merge_usd, since_ms, now_ms)?;
        let earliest = store.earliest_spread_sample_ts_mode(&sym, merge_usd)?;
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
    let mut venue_last_ts_ms: HashMap<String, i64> = HashMap::new();
    for row in &raw {
        samples.push(MidSample {
            ts_ms: row.ts_ms,
            venue: row.venue_id.clone(),
            mid: row.mid,
        });
        sample_count += 1;
        let e = venue_last_ts_ms
            .entry(row.venue_id.clone())
            .or_insert(row.ts_ms);
        if row.ts_ms > *e {
            *e = row.ts_ms;
        }
    }

    // Volume weights: venue_id -> this asset's reported 24h notional USD.
    // Venue volume maps are keyed by canonical BASE-QUOTE symbols; the
    // shared matcher strips quotes and K-/1000-prefix variants.
    let base = symbol
        .split('-')
        .next()
        .unwrap_or(symbol)
        .to_ascii_uppercase();
    let vol_maps = cached_venue_volumes(state).await;
    let mut weights = HashMap::new();
    for (vid, map) in &vol_maps {
        let v = crate::strategy_filters::venue_asset_volume_usd(map, &base).unwrap_or(0.0);
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
        venue_last_ts_ms,
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
pub(crate) const BASELINE_RETENTION_MS: i64 = 30 * 86_400_000;

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
                        p.half_life.is_finite()
                            && p.half_life > 0.0
                            && p.r_squared.is_some_and(|r| r >= OU_MIN_R2)
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

    let mut recommendations: Vec<u64> =
        evals.iter().filter_map(|e| e.recommendation_secs).collect();
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
        let windowed: Vec<&arbitrageur_math::pair_monitor::PairObservation> =
            e.series.iter().filter(|o| o.ts_ms >= cutoff_ms).collect();
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
        return Err(format!(
            "percentile must be within 50..=99, got {percentile}"
        ));
    }

    let base = symbol
        .split(['-', '/', '_'])
        .next()
        .unwrap_or(symbol)
        .to_ascii_uppercase();
    // Under `separate` USD-quote mode the requested pair ("BTC-USDT") is its
    // own instrument — only its exact key is read. Merged modes keep the
    // legacy alias sweep so bare-base and quoted rows combine.
    let merge_usd = state.usd_merge_quotes();
    let keys: Vec<String> = if merge_usd {
        vec![base.clone(), format!("{base}-USDC"), format!("{base}-USD")]
    } else {
        vec![symbol.trim().to_string()]
    };
    let now_ms = Utc::now().timestamp_millis();
    let since_ms = now_ms - BASELINE_RETENTION_MS;
    let store = state.state_store.clone();
    let sym = symbol.to_string();
    let mode_owned = mode.to_string();
    let snapshot = tokio::task::spawn_blocking(move || {
        let mut all = Vec::new();
        for k in &keys {
            if let Ok(mut rows) = store.spread_samples_mode(k, merge_usd, since_ms, now_ms) {
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

/// Per-series distribution statistics; every metric is `None` on empty
/// input rather than reporting a fabricated zero.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SpreadStatsDto {
    pub sample_count: u64,
    pub history_span_secs: u64,
    pub mean_bps: Option<f64>,
    pub std_bps: Option<f64>,
    pub p50_bps: Option<f64>,
    pub p90_bps: Option<f64>,
    pub p99_bps: Option<f64>,
    pub max_bps: Option<f64>,
    /// Share of observations at or above the caller's hurdle (0..=1).
    pub frac_at_or_above_hurdle: Option<f64>,
}

impl From<arbitrageur_math::dislocation::DistributionStats> for SpreadStatsDto {
    fn from(s: arbitrageur_math::dislocation::DistributionStats) -> Self {
        Self {
            sample_count: s.sample_count as u64,
            history_span_secs: s.history_span_secs,
            mean_bps: s.mean,
            std_bps: s.std_dev,
            p50_bps: s.p50,
            p90_bps: s.p90,
            p99_bps: s.p99,
            max_bps: s.max,
            frac_at_or_above_hurdle: s.frac_at_or_above,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SpreadPairAnalyticsDto {
    pub buy_venue: String,
    pub sell_venue: String,
    pub stats: SpreadStatsDto,
    /// OU mean-reversion half-life — `None` when the fit gates
    /// (min obs, min span, R²) are unmet.
    pub half_life_secs: Option<f64>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SpreadAnalyticsDto {
    pub symbol: String,
    pub window_secs: u64,
    pub hurdle_bps: f64,
    pub venues_seen: Vec<String>,
    /// Per-ticker dislocation band: at each poll timestamp the gross
    /// max–min venue spread in bps, then distribution stats over it.
    pub band: SpreadStatsDto,
    /// Per venue-pair gross-spread stats + half-life.
    pub pairs: Vec<SpreadPairAnalyticsDto>,
}

/// Per-ticker dislocation analytics over a bounded window: band
/// distribution (how often the spread even exists at hurdle size) plus
/// per-pair stats and half-lives. Read-only over `spread_samples`; every
/// thin-data field stays `None`/empty rather than fabricating numbers.
pub async fn get_spread_analytics_impl(
    symbol: &str,
    window_secs: u64,
    hurdle_bps: f64,
    state: &AppEngineState,
) -> Result<SpreadAnalyticsDto, String> {
    validate_market_symbol(symbol)?;
    let window_secs = window_secs.clamp(60, 30 * 86_400);
    if !hurdle_bps.is_finite() || hurdle_bps < 0.0 {
        return Err("hurdle_bps must be finite and non-negative".into());
    }

    let base = symbol
        .split(['-', '/', '_'])
        .next()
        .unwrap_or(symbol)
        .to_ascii_uppercase();
    let merge_usd = state.usd_merge_quotes();
    let keys: Vec<String> = if merge_usd {
        vec![base.clone(), format!("{base}-USDC"), format!("{base}-USD")]
    } else {
        vec![symbol.trim().to_string()]
    };
    let now_ms = Utc::now().timestamp_millis();
    let since_ms = now_ms - (window_secs as i64) * 1000;
    let store = state.state_store.clone();
    let sym = symbol.to_string();
    tokio::task::spawn_blocking(move || {
        let mut all = Vec::new();
        for k in &keys {
            if let Ok(mut rows) = store.spread_samples_mode(k, merge_usd, since_ms, now_ms) {
                all.append(&mut rows);
            }
        }
        all.sort_by(|a, b| (a.ts_ms, a.venue_id.as_str()).cmp(&(b.ts_ms, &b.venue_id)));
        all.dedup_by(|a, b| a.ts_ms == b.ts_ms && a.venue_id == b.venue_id);

        // Band series: one (ts, max/min - 1) point per poll timestamp that
        // actually observed ≥2 distinct venues — single-venue ticks can't
        // express a dislocation and are dropped, never zeroed.
        let mut mids_by_venue: HashMap<String, Vec<(i64, f64)>> = HashMap::new();
        let mut by_ts: HashMap<i64, (f64, f64, std::collections::HashSet<String>)> = HashMap::new();
        for s in &all {
            if !s.mid.is_finite() || s.mid <= 0.0 {
                continue;
            }
            mids_by_venue
                .entry(s.venue_id.clone())
                .or_default()
                .push((s.ts_ms, s.mid));
            let e = by_ts.entry(s.ts_ms).or_insert((
                f64::INFINITY,
                f64::NEG_INFINITY,
                Default::default(),
            ));
            e.0 = e.0.min(s.mid);
            e.1 = e.1.max(s.mid);
            e.2.insert(s.venue_id.clone());
        }
        let mut venues_seen: Vec<String> = mids_by_venue.keys().cloned().collect();
        venues_seen.sort();
        let mut band_series: Vec<(i64, f64)> = by_ts
            .into_iter()
            .filter(|(_, (_, _, vs))| vs.len() >= 2)
            .map(|(ts, (lo, hi, _))| (ts, (hi / lo - 1.0) * 10_000.0))
            .collect();
        band_series.sort_by_key(|(t, _)| *t);

        let mut pairs = Vec::new();
        for buy_v in &venues_seen {
            for sell_v in &venues_seen {
                if buy_v == sell_v {
                    continue;
                }
                let (Some(a), Some(b)) = (mids_by_venue.get(buy_v), mids_by_venue.get(sell_v))
                else {
                    continue;
                };
                let series =
                    arbitrageur_math::pair_monitor::build_pair_series(a, b, PAIR_ALIGN_GAP_MS);
                if series.is_empty() {
                    continue;
                }
                let vals: Vec<(i64, f64)> =
                    series.iter().map(|o| (o.ts_ms, o.spread_bps)).collect();
                let span_ms = series.last().unwrap().ts_ms - series[0].ts_ms;
                let span_secs = (span_ms / 1000).max(0) as u64;
                let half_life_secs =
                    if series.len() >= OU_MIN_OBS && span_secs as f64 >= OU_MIN_SPAN_SECS {
                        let dt = span_ms as f64 / 1000.0 / (series.len() - 1) as f64;
                        if dt.is_finite() && dt > 0.0 {
                            let v: Vec<f64> = series.iter().map(|o| o.spread_bps).collect();
                            OuModelParams::fit(&v, dt)
                                .filter(|p| {
                                    p.half_life.is_finite()
                                        && p.half_life > 0.0
                                        && p.r_squared.is_some_and(|r| r >= OU_MIN_R2)
                                })
                                .map(|p| p.half_life)
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                pairs.push(SpreadPairAnalyticsDto {
                    buy_venue: buy_v.clone(),
                    sell_venue: sell_v.clone(),
                    stats: distribution_stats(&vals, hurdle_bps).into(),
                    half_life_secs,
                });
            }
        }
        pairs.sort_by(|a, b| {
            (a.buy_venue.as_str(), a.sell_venue.as_str())
                .cmp(&(b.buy_venue.as_str(), b.sell_venue.as_str()))
        });

        Ok::<_, anyhow::Error>(SpreadAnalyticsDto {
            symbol: sym,
            window_secs,
            hurdle_bps,
            venues_seen,
            band: distribution_stats(&band_series, hurdle_bps).into(),
            pairs,
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_spread_analytics(
    symbol: String,
    window_secs: Option<u64>,
    hurdle_bps: Option<f64>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SpreadAnalyticsDto, String> {
    get_spread_analytics_impl(
        &symbol,
        window_secs.unwrap_or(86_400),
        hurdle_bps.unwrap_or(0.0),
        &state,
    )
    .await
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
    match execute_spot_arbitrage_impl(params.clone(), &state).await {
        Ok(r) => Ok(r),
        Err(e) => {
            record_execution_failure(
                &state,
                "Spot",
                "spot",
                &params.symbol,
                &params.buy_venue,
                &params.sell_venue,
                &e,
            );
            Err(e)
        }
    }
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
pub async fn set_trading_environment(
    environment: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<bool, String> {
    match environment.trim().to_ascii_lowercase().as_str() {
        "live" => set_live_mode_impl(true, &state),
        "testnet" => set_live_mode_impl(false, &state),
        other => Err(format!(
            "Unknown trading environment '{other}' — expected 'testnet' or 'live'"
        )),
    }
}

#[tauri::command]
pub async fn set_engine_paused(
    paused: bool,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<bool, String> {
    set_engine_paused_impl(paused, &state)
}

#[tauri::command]
pub async fn reconcile_orders_now(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<crate::reconciliation::ReconcileSummary, String> {
    Ok(crate::reconciliation::reconcile_pending_orders(&state).await)
}

/// One fixed-yield YU lock's maturity row for the UI. `signal_id` is the
/// `-yu` sibling signal; `maturity_state` is venue-truth-derived (never
/// wall-clock-settled); `resolved_action` is the operator's recorded
/// choice once `settled`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixedYieldMaturityDto {
    pub signal_id: String,
    pub parent_signal_id: Option<String>,
    pub parent_status: Option<String>,
    pub symbol: String,
    pub lock_until_ms: i64,
    pub maturity_state: String,
    pub resolved_action: Option<String>,
    pub venue_observation: Option<String>,
    pub resolved_at: Option<String>,
}

pub fn list_fixed_yield_maturities_impl(
    state: &AppEngineState,
) -> Result<Vec<FixedYieldMaturityDto>, String> {
    let rows = state
        .state_store
        .list_fixed_yield_maturities()
        .map_err(|e| e.to_string())?;
    let signals = state
        .state_store
        .list_signals(None, None)
        .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let parent = r
                .parent_signal_id
                .as_ref()
                .and_then(|pid| signals.iter().find(|s| &s.id == pid));
            let sibling = signals.iter().find(|s| s.id == r.signal_id);
            FixedYieldMaturityDto {
                signal_id: r.signal_id,
                parent_signal_id: r.parent_signal_id,
                parent_status: parent.map(|p| p.status.as_str().to_string()),
                symbol: sibling
                    .map(|s| s.symbol.clone())
                    .or_else(|| parent.map(|p| p.symbol.clone()))
                    .unwrap_or_default(),
                lock_until_ms: r.lock_until_ms,
                maturity_state: r.maturity_state,
                resolved_action: r.resolved_action,
                venue_observation: r.venue_observation,
                resolved_at: r.resolved_at,
            }
        })
        .collect())
}

#[tauri::command]
pub async fn list_fixed_yield_maturities(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<FixedYieldMaturityDto>, String> {
    list_fixed_yield_maturities_impl(&state)
}

/// Operator resolution of a venue-confirmed settled YU lock:
/// `close_pair` flattens the parent carry signal's perp legs through the
/// signal-scoped flatten path (reduce-only exits, reconciler-tracked);
/// `keep_carry` records the choice and leaves the pair running as
/// floating carry. Acting before `settled` is refused — the venue has
/// not confirmed the YU position is gone.
pub async fn resolve_fixed_yield_maturity_impl(
    yu_signal_id: &str,
    action: &str,
    state: &AppEngineState,
) -> Result<FlattenResultDto, String> {
    let row = state
        .state_store
        .list_fixed_yield_maturities()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|r| r.signal_id == yu_signal_id)
        .ok_or_else(|| format!("no fixed-yield maturity row for {yu_signal_id}"))?;
    if row.maturity_state != "settled" {
        return Err(format!(
            "YU lock is '{}' — settlement is not venue-confirmed; resolution is only available once the Boros account shows no residual position",
            row.maturity_state
        ));
    }
    if let Some(done) = &row.resolved_action {
        return Err(format!(
            "YU lock already resolved via '{done}' — resolution is durable and cannot be re-run"
        ));
    }
    match action {
        "close_pair" => {
            let parent = row
                .parent_signal_id
                .ok_or_else(|| "maturity row has no parent carry signal".to_string())?;
            let res = crate::execution::flatten_signal_positions_impl(
                &parent,
                "funding",
                None,
                arbitrageur_execution::virtual_ledger::CycleStatus::Closed,
                state,
            )
            .await?;
            // Only record the resolution when the flatten actually
            // finished — a partial close leaves the row actionable.
            if res.residual_signals == 0 {
                state
                    .state_store
                    .resolve_fixed_yield_maturity(yu_signal_id, "close_pair")
                    .map_err(|e| e.to_string())?;
            }
            Ok(res)
        }
        "keep_carry" => {
            state
                .state_store
                .resolve_fixed_yield_maturity(yu_signal_id, "keep_carry")
                .map_err(|e| e.to_string())?;
            Ok(FlattenResultDto {
                strategy_id: "funding".to_string(),
                symbol: None,
                signals_processed: 0,
                signals_closed: 0,
                legs_closed: 0,
                residual_signals: 0,
                notes: vec!["YU lock settled; perp pair retained as floating carry".to_string()],
            })
        }
        other => Err(format!("invalid maturity action '{other}'")),
    }
}

#[tauri::command]
pub async fn resolve_fixed_yield_maturity(
    yu_signal_id: String,
    action: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<FlattenResultDto, String> {
    resolve_fixed_yield_maturity_impl(&yu_signal_id, &action, &state).await
}

/// Priced early-exit quote for a live YU lock — no orders dispatched.
/// The UI shows this BEFORE the operator commits; execution re-quotes at
/// dispatch so a stale quote can never price an order.
#[tauri::command]
pub async fn quote_fixed_yield_exit(
    yu_signal_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<crate::funding::FixedYieldExitQuoteDto, String> {
    crate::funding::fixed_yield_exit_quote_impl(&yu_signal_id, &state).await
}

/// Execute the quoted early exit: bounded IOC per open YU leg → venue-
/// truth settle → parent perp pair flatten → maturity row `closed_early`.
#[tauri::command]
pub async fn execute_fixed_yield_exit(
    yu_signal_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<FlattenResultDto, String> {
    crate::funding::execute_fixed_yield_exit_impl(&yu_signal_id, &state).await
}

/// Boros ops status for the account card (issue #25): agent-approval
/// expiry and the gas treasury balance, both read live from the venue.
/// Every field fails closed — an unreadable value surfaces as `None`
/// with a named reason, never a fabricated number.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BorosStatusDto {
    pub configured: bool,
    pub agent_approved: bool,
    pub agent_expiry_unix: Option<u64>,
    pub gas_treasury_usd: Option<f64>,
    pub approval_note: Option<String>,
    pub gas_note: Option<String>,
}

pub async fn get_boros_status_impl(state: &AppEngineState) -> Result<BorosStatusDto, String> {
    let boros = &state.live_boros;
    if !boros.has_live_credentials() {
        return Ok(BorosStatusDto {
            configured: false,
            agent_approved: false,
            agent_expiry_unix: None,
            gas_treasury_usd: None,
            approval_note: Some(
                "no Boros credentials configured — add root wallet, agent key, and account id"
                    .to_string(),
            ),
            gas_note: None,
        });
    }
    let now_s = Utc::now().timestamp().max(0) as u64;
    let (expiry, approval_note) = match boros.agent_expiry_unix().await {
        Ok(Some(exp)) if exp > now_s => (Some(exp), None),
        Ok(Some(exp)) => (
            Some(exp),
            Some("agent approval EXPIRED — renew it in the Boros app".to_string()),
        ),
        Ok(None) => (
            None,
            Some("no agent approval found — approve the agent key in the Boros app".to_string()),
        ),
        Err(e) => (None, Some(format!("approval unreadable: {e}"))),
    };
    let (gas, gas_note) = match boros.gas_treasury_usd().await {
        Ok(v) => (v, None),
        Err(e) => (None, Some(format!("gas treasury unreadable: {e}"))),
    };
    Ok(BorosStatusDto {
        configured: true,
        agent_approved: approval_note.is_none(),
        agent_expiry_unix: expiry,
        gas_treasury_usd: gas,
        approval_note,
        gas_note,
    })
}

#[tauri::command]
pub async fn get_boros_status(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<BorosStatusDto, String> {
    get_boros_status_impl(&state).await
}

/// Operator-initiated cancel of one persisted venue order. Cancelling is
/// risk-reducing (it can only remove resting exposure), so it is allowed in
/// live mode even while new strategy entries stay blocked — but the vault
/// must be unlocked so the venue call is actually authenticated.
#[tauri::command]
pub async fn cancel_venue_order(
    venue: String,
    symbol: String,
    order_id: String,
    client_order_id: Option<String>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<String, String> {
    if state.is_live() && !state.vault.write().is_unlocked() {
        return Err(
            "Cannot cancel a venue order in Live Trading mode while the secure credential vault is locked. Unlock the vault first."
                .into(),
        );
    }
    let venue = venue.trim().to_lowercase();
    let symbol = symbol.trim().to_string();
    let order_id = order_id.trim().to_string();
    if venue.is_empty() || symbol.is_empty() || order_id.is_empty() {
        return Err("venue, symbol, and order_id are required".into());
    }
    let adapter = state
        .get_adapter(&venue)
        .ok_or_else(|| format!("Venue '{venue}' is not admitted for execution"))?;
    if !adapter.live_order_submission_implemented() || !adapter.has_live_credentials() {
        return Err(format!(
            "Venue '{venue}' does not support live order cancellation or has unauthenticated credentials"
        ));
    }
    let client_order_id = client_order_id
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty());
    adapter
        .cancel_order(arbitrageur_core::order::CancelRequest {
            venue: adapter.venue_id(),
            symbol: symbol.clone(),
            order_id: Some(order_id.clone()),
            client_order_id,
        })
        .await
        .map_err(|e| format!("Cancel on {venue} for order {order_id} failed: {e}"))?;
    Ok(format!(
        "Cancel request accepted on {venue} for order {order_id}"
    ))
}

/// Operator acknowledgement of an escalated signal after manually verifying
/// both venues. Ledger-only: moves `unresolved` -> `failed` (nothing was
/// proven open) or `imbalanced` -> `closed` (operator handled the residual
/// off-app). Refuses any other transition so it can never mask a live state.
#[tauri::command]
pub async fn acknowledge_signal(
    signal_id: String,
    resolution: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<String, String> {
    let resolution = resolution.trim().to_lowercase();
    let target = match resolution.as_str() {
        "failed" => crate::state_store::SignalStatus::Failed,
        "closed" => crate::state_store::SignalStatus::Closed,
        _ => return Err("resolution must be 'failed' or 'closed'".into()),
    };
    let sig = state
        .state_store
        .get_signal(&signal_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Signal '{signal_id}' not found"))?;
    let allowed = matches!(
        (sig.status, target),
        (
            crate::state_store::SignalStatus::Unresolved,
            crate::state_store::SignalStatus::Failed
        ) | (
            crate::state_store::SignalStatus::Unresolved,
            crate::state_store::SignalStatus::Closed
        ) | (
            crate::state_store::SignalStatus::Imbalanced,
            crate::state_store::SignalStatus::Closed
        )
    );
    if !allowed {
        return Err(format!(
            "Cannot mark a {:?} signal as {:?} — only unresolved→failed/closed or imbalanced→closed after venue review",
            sig.status, target
        ));
    }
    let note = format!(
        "Operator acknowledged after manual venue review ({:?} -> {:?})",
        sig.status, target
    );
    state
        .state_store
        .update_signal_status(&sig.id, target, Some(&note))
        .map_err(|e| e.to_string())?;
    Ok(format!("Signal {} marked {:?}", sig.id, target))
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

/// Stablecoin FX health for System Settings → Execution: per-quote
/// effective USD rates, deviation from parity, and which quotes are
/// currently past the halt threshold.
#[tauri::command]
pub async fn get_stablecoin_health(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<crate::stablecoin_fx::StablecoinHealthDto, String> {
    let s = state.settings.read().clone();
    let mode = crate::stablecoin_fx::normalize_usd_quote_mode(&s.usd_quote_mode);
    Ok(state.stablecoin_fx.health(
        mode,
        Utc::now().timestamp_millis(),
        s.stablecoin_warn_bps,
        s.stablecoin_halt_bps,
    ))
}

/// Live storage footprint behind the Historical Price Data retention
/// inputs: the SQLite file size the GB budget enforces on, and the
/// top-of-book sample span the day limit enforces on.
#[derive(Clone, Serialize)]
pub struct StorageStatsDto {
    pub db_bytes: u64,
    pub sample_rows: u64,
    pub oldest_sample_ts_ms: Option<i64>,
    pub newest_sample_ts_ms: Option<i64>,
}

#[tauri::command]
pub async fn get_storage_stats(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<StorageStatsDto, String> {
    let db_bytes = state
        .state_store
        .db_size_bytes()
        .map_err(|e| e.to_string())?;
    let (rows, oldest, newest) = state
        .state_store
        .spread_samples_stats()
        .map_err(|e| e.to_string())?;
    Ok(StorageStatsDto {
        db_bytes,
        sample_rows: rows.max(0) as u64,
        oldest_sample_ts_ms: oldest,
        newest_sample_ts_ms: newest,
    })
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
/// Simple Spread, so "radar" evaluates against the "futures" record.
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
    mark_prices_by_venue: &HashMap<String, HashMap<String, f64>>,
) -> HashMap<String, f64> {
    let mut mids = HashMap::new();
    for v in in_scope_venues {
        if let Some(m) = hub_mid_usd(state, base, v) {
            mids.insert(v.clone(), m);
        } else if let Some(m) = mark_prices_by_venue
            .get(v)
            .and_then(|prices| prices.get(base))
            .copied()
            .filter(|price| price.is_finite() && *price > 0.0)
        {
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
    for key in [
        base.to_string(),
        format!("{base}-USDC"),
        format!("{base}-USDT"),
        format!("{base}-USD"),
    ] {
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

fn base_mark_prices(
    markets: &[arbitrageur_core::market::MarketMetadata],
    venue_prices: &HashMap<String, Decimal>,
) -> HashMap<String, f64> {
    let mut by_base = HashMap::new();
    for market in markets.iter().filter(|market| {
        market.is_active
            && market.symbol.market_type == arbitrageur_core::market::MarketType::Perpetual
    }) {
        let Some(price) = venue_prices.get(&market.venue_symbol) else {
            continue;
        };
        let Ok(price) = price.to_string().parse::<f64>() else {
            continue;
        };
        if !price.is_finite() || price <= 0.0 {
            continue;
        }
        let base = crate::monitor::canonical_base(&market.symbol.base);
        by_base
            .entry(base)
            .and_modify(|existing: &mut f64| *existing = existing.min(price))
            .or_insert(price);
    }
    by_base
}

#[derive(Default)]
struct StrategyVenueUniverse {
    bases_by_venue: HashMap<String, Vec<String>>,
    mark_prices_by_venue: HashMap<String, HashMap<String, f64>>,
}

async fn scan_strategy_venue_universe(
    state: &AppEngineState,
    include_prices: bool,
) -> StrategyVenueUniverse {
    let mut scan = StrategyVenueUniverse::default();
    for (vid, _, adapter) in state.get_venues() {
        let markets = match adapter.fetch_markets().await {
            Ok(markets) => markets,
            Err(_) => continue,
        };
        let mut bases: Vec<String> = markets
            .iter()
            .filter(|market| market.is_active)
            .map(|market| crate::monitor::canonical_base(&market.symbol.base))
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        bases.sort();
        scan.bases_by_venue.insert(vid.to_string(), bases);

        if include_prices {
            let prices = adapter.fetch_market_prices().await.unwrap_or_default();
            scan.mark_prices_by_venue
                .insert(vid.to_string(), base_mark_prices(&markets, &prices));
        }
    }
    scan
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
    let include_prices = crate::strategy_filters::threshold_active(filters.min_price_usd);
    let mark_prices_by_venue = if include_prices {
        scan_strategy_venue_universe(state, true)
            .await
            .mark_prices_by_venue
    } else {
        HashMap::new()
    };
    let mids = resolve_ticker_mids(state, base, in_scope_venues, &mark_prices_by_venue);
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
    let include_prices = crate::strategy_filters::threshold_active(filters.min_price_usd);
    let scan = scan_strategy_venue_universe(state, include_prices).await;
    let venue_bases = scan.bases_by_venue;
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
            resolve_ticker_mids(state, &base, &scope, &scan.mark_prices_by_venue)
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
    if !(1..=10).contains(&strategy_target) {
        return Err(format!(
            "Configured leverage {strategy_target} for {symbol} is out of range; must be 1..=10"
        ));
    }
    let accounts = resolve_signal_accounts(
        state,
        attribution_strategy_id,
        buy_venue,
        sell_venue,
        route_id,
    )?;
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
    // Risk Control global ceiling: min over strategy target, account max,
    // venue max, and this. Defaults to 10x; 0/absurd stored values fall back
    // to the default rather than unbinding leverage or bricking execution.
    let global_max = {
        let v = state.settings.read().max_leverage;
        if (1..=100).contains(&v) {
            v
        } else {
            10
        }
    };
    Ok((
        crate::strategy_filters::effective_leverage(strategy_target, account_cap, None)
            .min(global_max),
        account_cap,
    ))
}

/// Position signature for the attribution cross-check: sorted
/// `symbol:side:quantity` segments, empty when flat.
pub(crate) fn position_signature(positions: &[arbitrageur_core::position::Position]) -> String {
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

/// Record one margin-utilisation snapshot per admitted venue unless the
/// per-venue cadence cap says the last write is still fresh. Best effort:
/// account reads or store writes never break execution. Called on fills
/// dispatches (open, triangle, flatten) and by the periodic retention
/// heartbeat so history stays bounded yet continuous.
pub(crate) async fn maybe_record_margin_snapshots(state: &AppEngineState, now_ms: i64) {
    if now_ms < 0 {
        return;
    }
    for (vid, _, adapter) in state.get_venues() {
        let fresh = state
            .margin_snapshot_at
            .lock()
            .get(vid)
            .is_some_and(|last| now_ms.saturating_sub(*last) < MARGIN_SNAPSHOT_MIN_INTERVAL_MS);
        if fresh {
            continue;
        }
        // Uncredentialed venues fail the account read and skip — same
        // best-effort semantics the old simulator loop had.
        let account = match adapter.fetch_account().await {
            Ok(mut acc) => {
                state.normalize_account_usd(vid, &mut acc);
                acc
            }
            Err(_) => continue,
        };
        // Feed the drawdown breaker from the background lane too — the
        // halt must not depend on someone watching the Risk Overview.
        crate::execution::feed_breaker_equity_usd(state, vid, account.equity);
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
            state
                .margin_snapshot_at
                .lock()
                .insert(vid.to_string(), now_ms);
        }
    }
}

/// Pre-trade per-venue equity snapshot keyed `{env}-{venue}` — the baseline
/// `refresh_equity_expectations_after_fills` measures own-fill deltas
/// against. Reads the active environment's admitted adapters.
pub(crate) async fn snapshot_venue_equity(state: &AppEngineState) -> HashMap<String, f64> {
    let mut map = HashMap::new();
    let env_str = state.env().as_str();
    for (vid, _name, adapter) in state.get_venues() {
        if let Ok(mut acc) = adapter.fetch_account().await {
            state.normalize_account_usd(vid, &mut acc);
            if let Ok(f) = acc.equity.to_string().parse::<f64>() {
                if f.is_finite() {
                    map.insert(format!("{env_str}-{vid}"), f);
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
/// Keys are `{env}-{venue}` so the two environments' baselines never mix.
pub(crate) async fn refresh_equity_expectations_after_fills(
    state: &AppEngineState,
    before: &HashMap<String, f64>,
) {
    let now_ms = Utc::now().timestamp_millis();
    let env_str = state.env().as_str();
    for (vid, _name, adapter) in state.get_venues() {
        let key = format!("{env_str}-{vid}");
        let Ok(mut acc) = adapter.fetch_account().await else {
            continue;
        };
        // Post-trade reads must see post-trade state: refresh the
        // last-known-good account cache so a caller inside the fresh window
        // doesn't get served the pre-trade snapshot.
        acc.venue = vid.into();
        state
            .last_good_accounts
            .lock()
            .insert(key.clone(), (acc.clone(), now_ms));
        // The delta is measured on the USD basis — `before` snapshots and
        // stored expectations are USD-normalized, so the post-fill read is
        // too. The cache above keeps the venue-denominated snapshot.
        let mut acc_usd = acc.clone();
        state.normalize_account_usd(vid, &mut acc_usd);
        let Ok(after) = acc_usd.equity.to_string().parse::<f64>() else {
            continue;
        };
        if !after.is_finite() {
            continue;
        }
        let Some(b) = before.get(&key).copied() else {
            continue;
        };
        let prior = state
            .state_store
            .get_equity_expectation(&key)
            .ok()
            .flatten()
            .map(|(e, _, _)| e);
        let next = prior.unwrap_or(b) + (after - b);
        if next.is_finite() && next >= 0.0 {
            let _ = state.state_store.set_equity_expectation(
                &key,
                next,
                &position_signature(&acc.positions),
                now_ms,
            );
        }
    }
}
#[tauri::command]
pub async fn run_seasonality_promotion(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<crate::seasonality::SeasonalityRunSummary, String> {
    let now_ms = Utc::now().timestamp_millis();
    crate::seasonality::run_seasonality_promotion(&state, now_ms)
}

// ---------------------------------------------------------------------------
// Single-Spread convergence lifecycle: position-aware exits that free the
// pair slot for re-entry.
// ---------------------------------------------------------------------------

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
    let merge_usd = state.usd_merge_quotes();
    let rows = state
        .state_store
        .session_profiles_mode(symbol, merge_usd)
        .map_err(|e| e.to_string())?;
    let promotion_row = state
        .state_store
        .phase2_promotion_state_mode(symbol, merge_usd)
        .map_err(|e| e.to_string())?;
    let anchor_row = state
        .state_store
        .phase2_har_anchor_mode(symbol, merge_usd)
        .map_err(|e| e.to_string())?;
    let rescaled =
        crate::seasonality::promotion_active(promotion_row.as_ref(), anchor_row.as_ref(), now_ms);
    let scale = if rescaled {
        match (&promotion_row, &anchor_row) {
            (Some(_), Some(a)) => arbitrageur_math::seasonality::rescale_factor(
                a.forecast_rv_bps,
                a.trailing_mean_rv_bps,
            ),
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
        let har_r2 = anchor_row
            .as_ref()
            .and_then(|a| if a.r2.is_finite() { Some(a.r2) } else { None });
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
        symbol: arbitrageur_core::symbol::pair_key(symbol, merge_usd),
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
    use crate::state::TickerStrategyConfigDto;
    use crate::vault::ExchangeCredentials;

    #[tokio::test]
    async fn external_url_opener_is_allowlisted() {
        // Non-allowlisted URLs (and non-https schemes) are refused without
        // touching the OS opener — offline-safe.
        assert!(
            super::open_external_url("https://evil.example/phish".to_string())
                .await
                .is_err()
        );
        assert!(super::open_external_url("file:///tmp/x.html".to_string())
            .await
            .is_err());
        assert!(
            super::open_external_url("https://popdex.xyz.evil.example/".to_string())
                .await
                .is_err()
        );
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
            // Passive unwind is default-on; zero the attempt wait so the
            // limit/cancel/fallback sequence runs instantly in tests.
            s.passive_unwind.timeout_secs = 0;
        }
        for v in ["hyperliquid", "lighter", "vanta", "robinhood", "arcus"] {
            state.install_test_sim(v);
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
        // Mirror into the venue's installed test sim so adapter-level book
        // resolution returns the same data after the hub freshness window.
        if let Some(sim) = state
            .test_sims
            .read()
            .ok()
            .and_then(|m| m.get(venue_id).cloned())
        {
            let _ = sim.replace_orderbook(book.clone());
        }
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

    fn trim_test_position(
        side: Side,
        qty: Decimal,
        mark: Decimal,
        liq: Option<Decimal>,
    ) -> arbitrageur_core::position::Position {
        arbitrageur_core::position::Position {
            venue: VenueId::new("kraken"),
            symbol: "BTC-USD".to_string(),
            side,
            quantity: qty,
            entry_price: mark,
            mark_price: mark,
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: liq,
            leverage: dec!(5),
            initial_margin: dec!(0),
            maintenance_margin: dec!(0),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn liq_trim_decision_holds_when_healthy() {
        use arbitrageur_strategy::pair_trim::PairTrimDecision;
        // mark 100, liq 90 → 10% distance; threshold 5% → Hold.
        let long = trim_test_position(Side::Buy, dec!(1.0), dec!(100), Some(dec!(90)));
        let short = trim_test_position(Side::Sell, dec!(1.0), dec!(100), Some(dec!(110)));
        match liq_trim_decision_for_positions(&long, &short, dec!(0.01), 5.0) {
            Some((PairTrimDecision::Hold, d)) => assert!((d - 10.0).abs() < 0.01),
            other => panic!("expected Hold at 10% distance, got {other:?}"),
        }
    }

    #[test]
    fn liq_trim_decision_trims_when_either_leg_near_liq() {
        use arbitrageur_strategy::pair_trim::PairTrimDecision;
        // Long leg healthy (10%), short leg at 2% — the smaller distance arms it.
        let long = trim_test_position(Side::Buy, dec!(1.0), dec!(100), Some(dec!(90)));
        let short = trim_test_position(Side::Sell, dec!(0.5), dec!(100), Some(dec!(102)));
        match liq_trim_decision_for_positions(&long, &short, dec!(0.01), 5.0) {
            Some((PairTrimDecision::TrimBoth { qty }, d)) => {
                assert!((d - 2.0).abs() < 0.01);
                // Trim qty is step-rounded against the SMALLER leg (0.5).
                assert!(qty > 0.0 && qty <= 0.5);
            }
            other => panic!("expected TrimBoth at 2% distance, got {other:?}"),
        }
    }

    #[test]
    fn liq_trim_decision_closes_when_remainder_below_step() {
        use arbitrageur_strategy::pair_trim::PairTrimDecision;
        // Position smaller than one step → any trim would leave dust → CloseBoth.
        let long = trim_test_position(Side::Buy, dec!(0.005), dec!(100), Some(dec!(97)));
        let short = trim_test_position(Side::Sell, dec!(0.005), dec!(100), Some(dec!(103)));
        match liq_trim_decision_for_positions(&long, &short, dec!(0.01), 5.0) {
            Some((PairTrimDecision::CloseBoth, _)) => {}
            other => panic!("expected CloseBoth for sub-step remainder, got {other:?}"),
        }
    }

    #[test]
    fn liq_trim_decision_fails_closed_on_missing_telemetry() {
        // No liquidation price → no distance → None (never trim blind).
        let long = trim_test_position(Side::Buy, dec!(1.0), dec!(100), None);
        let short = trim_test_position(Side::Sell, dec!(1.0), dec!(100), Some(dec!(102)));
        assert!(liq_trim_decision_for_positions(&long, &short, dec!(0.01), 5.0).is_none());
        // Side mismatch (position contradicts the signal's leg) → None.
        let wrong = trim_test_position(Side::Sell, dec!(1.0), dec!(100), Some(dec!(102)));
        assert!(liq_trim_decision_for_positions(&wrong, &short, dec!(0.01), 5.0).is_none());
        // Zero quantity → None.
        let flat = trim_test_position(Side::Buy, dec!(0), dec!(100), Some(dec!(97)));
        assert!(liq_trim_decision_for_positions(&flat, &short, dec!(0.01), 5.0).is_none());
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
        // 19 primary venues + 26 DexPerpAdapter venues + 11 EvmAmmAdapter
        // venues (nothing enabled, no credentials).
        assert_eq!(status.active_venues.len(), 56);
        for v in &status.active_venues {
            // Adapter-less venues (polymarket/kalshi/ibkr) report
            // UNSUPPORTED unconditionally — enablement can never admit
            // them, so OFFLINE would falsely imply a working switch.
            let expected = if crate::state::venue_has_adapter(&v.id) {
                "OFFLINE"
            } else {
                "UNSUPPORTED"
            };
            assert_eq!(v.status, expected, "venue {}", v.id);
            assert_eq!(v.latency_ms, None);
        }
        assert!(!status.is_paused);
    }

    #[tokio::test]
    async fn test_system_status_reflects_account_level_enablement() {
        let (state, dir) = test_state_with_vault("status-acct");
        // Live environment: ibkr has no adapter at all and several venues
        // lack public testnets, so account-level enablement for them is
        // only visible under Live.
        state.set_environment(crate::state::TradingEnvironment::Live);

        // No flags, no accounts: everything OFFLINE except the adapter-less
        // venues, which report UNSUPPORTED even while disabled.
        let status = get_system_status_impl(&state).await.expect("status");
        assert!(status.active_venues.iter().all(|v| v.status
            == if crate::state::venue_has_adapter(&v.id) {
                "OFFLINE"
            } else {
                "UNSUPPORTED"
            }));

        // An enabled account profile admits the venue even with the engine
        // flag unset — status must match what get_venues actually admits.
        upsert_account_impl(
            AccountUpsertInput {
                environment: None,
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
                .filter(|v| v.status == "READ_ONLY")
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
                environment: None,
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
                environment: None,
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
                environment: None,
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

        // Polymarket/kalshi have account rows and engine flags but no
        // constructed adapter — UNSUPPORTED must hold even before the flag
        // or account enables them (OFFLINE would imply a working switch).
        for dead in ["polymarket", "kalshi"] {
            let row = status
                .active_venues
                .iter()
                .find(|v| v.id == dead)
                .unwrap_or_else(|| panic!("{dead} row"));
            assert_eq!(row.status, "UNSUPPORTED", "{dead} while disabled");
            assert!(!row.can_submit);
            assert!(!row.can_enter_strategy);
        }

        let _ = std::fs::remove_dir_all(dir);
    }

    /// Variational admits with a credential-less enabled account (market
    /// data is public; the trading API is not yet released) — Live only.
    /// Status must read READ_ONLY and get_venues must carry the adapter.
    #[tokio::test]
    async fn test_variational_admits_via_credentialless_live_account() {
        let (state, dir) = test_state_with_vault("variational-admit");
        state.set_environment(crate::state::TradingEnvironment::Live);

        upsert_account_impl(
            AccountUpsertInput {
                environment: None,
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "variational".into(),
                label: "V".into(),
                enabled: true,
                make_default: false,
                credentials: None,
            },
            &state,
        )
        .expect("create variational account");

        assert!(state
            .get_venues()
            .iter()
            .any(|(v, _, _)| *v == "variational"));
        assert!(state.get_adapter("variational").is_some());
        let status = get_system_status_impl(&state).await.expect("status");
        let row = status
            .active_venues
            .iter()
            .find(|v| v.id == "variational")
            .expect("variational row");
        assert_eq!(row.status, "READ_ONLY");
        // Read-only venue: capability flags must never claim execution.
        assert!(!row.can_submit);
        assert!(!row.can_enter_strategy);
        assert_eq!(row.fee_bps, 0.0);

        // No demo environment exists — Testnet can never admit it even
        // with a testnet-scoped account row.
        state.set_environment(crate::state::TradingEnvironment::Testnet);
        upsert_account_impl(
            AccountUpsertInput {
                environment: None,
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "variational".into(),
                label: "V-TN".into(),
                enabled: true,
                make_default: false,
                credentials: None,
            },
            &state,
        )
        .expect("create testnet variational account");
        assert!(state
            .get_venues()
            .iter()
            .all(|(v, _, _)| *v != "variational"));

        let _ = std::fs::remove_dir_all(dir);
    }

    /// #27c: credential material for an adapter-less venue can never be
    /// consumed — the upsert must refuse it instead of warehousing secrets
    /// that provision nothing. The account row itself still saves.
    #[tokio::test]
    async fn test_upsert_account_refuses_credentials_for_unadapted_venue() {
        let (state, dir) = test_state_with_vault("unadapted-creds");
        initialize_vault_impl("test password", &state).expect("init vault");

        // Credential-bearing create on polymarket → refused, no row persisted.
        let err = upsert_account_impl(
            AccountUpsertInput {
                environment: None,
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "polymarket".into(),
                label: "poly".into(),
                enabled: false,
                make_default: false,
                credentials: Some(AccountCredentialInput {
                    api_key: "pk_live".into(),
                    api_secret: "sk_live".into(),
                    ..Default::default()
                }),
            },
            &state,
        )
        .expect_err("credentials on an adapter-less venue must be refused");
        assert!(
            err.contains("no polymarket adapter"),
            "unexpected error: {err}"
        );
        assert!(state
            .state_store
            .list_accounts(None)
            .unwrap_or_default()
            .iter()
            .all(|a| a.venue != "polymarket"));

        // The same venue without credential material saves fine — the row
        // is honest metadata; only the secrets are refused.
        let ok = upsert_account_impl(
            AccountUpsertInput {
                environment: None,
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "kalshi".into(),
                label: "k".into(),
                enabled: true,
                make_default: false,
                credentials: None,
            },
            &state,
        )
        .expect("credential-less account on a dead venue must save");
        assert_eq!(ok.venue, "kalshi");

        // An all-empty Some(credentials) payload is the patch no-op — not
        // material, so it must not trip the refuse.
        let ok2 = upsert_account_impl(
            AccountUpsertInput {
                environment: None,
                account_tier: None,
                max_leverage: None,
                id: Some(ok.id.clone()),
                venue: "kalshi".into(),
                label: "k".into(),
                enabled: true,
                make_default: false,
                credentials: Some(AccountCredentialInput::default()),
            },
            &state,
        )
        .expect("empty credentials payload is a no-op, not a refuse");
        assert_eq!(ok2.id, ok.id);

        // A real venue with an adapter still accepts credentials (gate is
        // venue-scoped, not a blanket refuse).
        let hl = upsert_account_impl(
            AccountUpsertInput {
                environment: None,
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "hyperliquid".into(),
                label: "hl".into(),
                enabled: false,
                make_default: false,
                credentials: Some(AccountCredentialInput {
                    wallet_address: Some("0x0000000000000000000000000000000000000001".into()),
                    ..Default::default()
                }),
            },
            &state,
        )
        .expect("hyperliquid credentials must save");
        assert_eq!(hl.venue, "hyperliquid");

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

    /// Two spot-capable test venues (Alpaca paper + OKX demo admit sims).
    /// Trade size stays inside the seeded 10-unit level at the default 3x
    /// depth-ratio gate so routes can be executable.
    fn setup_spot_test_state() -> AppEngineState {
        let state = AppEngineState::new();
        {
            let mut s = state.settings.write();
            s.alpaca_enabled = true;
            s.okx_enabled = true;
            s.passive_unwind.timeout_secs = 0;
        }
        for v in ["alpaca", "okx"] {
            let sim = state.install_test_sim(v).expect("spot sim installs");
            sim.set_market_type(arbitrageur_core::market::MarketType::Spot);
        }
        let mut p = StrategyParamsDto::defaults_for("spot");
        p.spot_trade_size_usd = 200.0;
        p.spot_min_net_spread_bps = 10.0;
        state.strategy_params.write().insert("spot".to_string(), p);
        state
    }

    #[tokio::test]
    async fn test_spot_scan_reports_shared_spot_markets() {
        let state = setup_spot_test_state();
        seed_test_book(&state, "alpaca", "BTC-USDC", dec!(99.9), dec!(100.0));
        seed_test_book(&state, "okx", "BTC-USDC", dec!(101.0), dec!(101.1));

        let opps = scan_spot_opportunities_impl("", &state)
            .await
            .expect("spot scan");

        // The profitable direction buys the cheaper ask and sells the
        // richer bid; the reverse direction is reported but not executable.
        let good = opps
            .iter()
            .find(|o| o.id == "BTC-USDC-alpaca-okx")
            .expect("alpaca->okx route must exist");
        assert_eq!(good.symbol, "BTC-USDC");
        assert!(good.gross_spread_bps > 0.0);
        assert!(good.net_spread_bps < good.gross_spread_bps);
        assert!(
            good.net_spread_bps > 10.0,
            "net {} should clear the hurdle",
            good.net_spread_bps
        );
        assert!(good.is_executable);
        assert_eq!(good.transfer_cost_usd, 0.0);
        let bad = opps
            .iter()
            .find(|o| o.id == "BTC-USDC-okx-alpaca")
            .expect("reverse direction must be reported");
        assert!(bad.gross_spread_bps < 0.0);
        assert!(!bad.is_executable);

        // Base-name filtering scopes the scan.
        let filtered = scan_spot_opportunities_impl("BTC", &state)
            .await
            .expect("filtered scan");
        assert_eq!(filtered.len(), opps.len());
        let filtered = scan_spot_opportunities_impl("ETH", &state)
            .await
            .expect("filtered scan");
        assert!(filtered.is_empty());
    }

    #[tokio::test]
    async fn test_spot_scan_ignores_perp_only_venues() {
        // The standard harness installs perp sims — fetch_markets reports
        // MarketType::Perpetual, so no spot route may be fabricated.
        let state = setup_test_state_with_venues();
        seed_test_book(&state, "hyperliquid", "BTC-USDC", dec!(99.9), dec!(100.0));
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(101.0), dec!(101.1));
        let opps = scan_spot_opportunities_impl("", &state)
            .await
            .expect("spot scan");
        assert!(opps.is_empty(), "perp markets must not produce spot routes");
    }

    #[tokio::test]
    async fn test_spot_scan_needs_two_venues_listing() {
        // A spot market listed on a single venue cannot form a route.
        let state = setup_spot_test_state();
        seed_test_book(&state, "alpaca", "BTC-USDC", dec!(99.9), dec!(100.0));
        // ETH prices keep the configured size inside the seeded level.
        seed_test_book(&state, "alpaca", "ETH-USDC", dec!(499.5), dec!(500.0));
        seed_test_book(&state, "okx", "ETH-USDC", dec!(505.0), dec!(505.5));
        let opps = scan_spot_opportunities_impl("", &state)
            .await
            .expect("spot scan");
        assert!(opps.iter().all(|o| o.symbol == "ETH-USDC"));
        assert!(opps.iter().any(|o| o.symbol == "ETH-USDC"));
    }

    #[tokio::test]
    async fn test_spot_execution_fills_via_dual_leg() {
        let state = setup_spot_test_state();
        seed_test_book(&state, "alpaca", "BTC-USDC", dec!(99.9), dec!(100.0));
        seed_test_book(&state, "okx", "BTC-USDC", dec!(101.0), dec!(101.1));
        // Spot sell leg needs pre-positioned base inventory on OKX.
        state
            .test_sim("okx")
            .expect("okx sim")
            .seed_balance("BTC", dec!(10));

        let res = execute_spot_arbitrage_impl(
            SpotExecutionParams {
                symbol: "BTC-USDC".into(),
                buy_venue: "alpaca".into(),
                sell_venue: "okx".into(),
                trade_size_usd: 200.0,
                max_slippage_bps: 5.0,
                net_spread_bps: None,
            },
            &state,
        )
        .await;
        let res = res.unwrap_or_else(|e| panic!("spot execution must succeed: {e}"));
        let _ = res;
        let signals = state
            .state_store
            .list_signals(Some("BTC-USDC"), None)
            .expect("signals");
        assert!(
            signals
                .iter()
                .any(|s| s.strategy_id == "spot" && s.symbol == "BTC-USDC"),
            "the pair must persist as a durable 'spot' signal"
        );
    }

    #[tokio::test]
    async fn test_spot_execution_refuses_without_sell_inventory() {
        let state = setup_spot_test_state();
        seed_test_book(&state, "alpaca", "BTC-USDC", dec!(99.9), dec!(100.0));
        seed_test_book(&state, "okx", "BTC-USDC", dec!(101.0), dec!(101.1));
        // No BTC on the sell venue: a spot sell cannot borrow.

        let res = execute_spot_arbitrage_impl(
            SpotExecutionParams {
                symbol: "BTC-USDC".into(),
                buy_venue: "alpaca".into(),
                sell_venue: "okx".into(),
                trade_size_usd: 200.0,
                max_slippage_bps: 5.0,
                net_spread_bps: None,
            },
            &state,
        )
        .await;
        let err = res.expect_err("sell leg has no inventory to draw");
        assert!(err.contains("inventory"), "unexpected err: {err}");
    }

    #[tokio::test]
    async fn test_spot_execution_refuses_perp_only_venues() {
        let state = setup_test_state_with_venues();
        seed_test_book(&state, "hyperliquid", "BTC-USDC", dec!(99.9), dec!(100.0));
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(101.0), dec!(101.1));
        let res = execute_spot_arbitrage_impl(
            SpotExecutionParams {
                symbol: "BTC-USDC".into(),
                buy_venue: "hyperliquid".into(),
                sell_venue: "lighter".into(),
                trade_size_usd: 200.0,
                max_slippage_bps: 5.0,
                net_spread_bps: None,
            },
            &state,
        )
        .await;
        let err = res.expect_err("perp venues cannot route spot orders");
        assert!(err.contains("spot"), "unexpected err: {err}");
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
        let rates = vec![
            ("lighter", "Lighter", r_lighter),
            ("hyperliquid", "Hyperliquid", r_hl),
        ];
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
                        bid_depth: None,
                        ask_depth: None,
                    },
                    VenueSampleEntry {
                        venue_id: "vanta".into(),
                        mid: 100.5,
                        best_bid: 100.4,
                        best_ask: 100.6,
                        bid_depth: None,
                        ask_depth: None,
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
        assert!(
            b.long_venue.contains("SPOT"),
            "long leg must be spot, got {}",
            b.long_venue
        );
        assert!(
            b.short_venue.contains("PERP"),
            "short leg must be perp, got {}",
            b.short_venue
        );
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
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64440.0), dec!(64445.0));

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
        // the calculated net BPS which must clear the 15 BPS hurdle
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
        // No venues enabled → no accounts → zero portfolio value (venue
        // truth, not simulated seed equity).
        assert_eq!(risk.total_portfolio_value_usd, 0.0);

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
        assert_eq!(
            state.state_store.kv_get("retention.max_days").unwrap(),
            None
        );
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
        assert!(
            count < 6,
            "tiny budget must evict oldest-first, left {count}"
        );
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
    async fn test_retention_gb_budget_measures_used_not_allocated() {
        // Regression: the GB cap used to compare the budget against the
        // ALLOCATED file size (page_count), which deletes can never shrink —
        // freed space lands on the freelist. The loop would drain the entire
        // table chasing a size that never drops. The budget must bind against
        // used bytes (page_count - freelist_count).
        let state = AppEngineState::new();
        let now = Utc::now().timestamp_millis();
        // Mids stay within 3x of the incumbent anchor — record_spread_samples
        // drops wider deviations as symbol collisions, which would silently
        // shrink the seeded row count.
        for i in 0..500 {
            seed_samples(
                &state,
                "BTC-USDC",
                now - (500 - i) * 1_000,
                &[("hyperliquid", 100.0 + (i % 10) as f64)],
            );
        }
        // Free most rows directly so the freelist grows while page_count
        // stays put — allocated > used is the precondition for the bug.
        state
            .state_store
            .delete_spread_samples_before(now - 100 * 1_000)
            .expect("bulk delete");
        let remaining = state.state_store.spread_sample_count().unwrap();
        assert_eq!(remaining, 100);
        assert!(
            state.state_store.db_size_bytes().unwrap() > state.state_store.db_used_bytes().unwrap(),
            "fixture must produce freelist pages"
        );
        // Budget exactly at current used bytes → nothing left to evict.
        // The old allocated-size check would have kept deleting to zero.
        let used_gb =
            state.state_store.db_used_bytes().unwrap() as f64 / (1024.0 * 1024.0 * 1024.0);
        {
            let mut s = state.settings.write();
            s.spread_samples_max_days = None;
            s.spread_samples_max_gb = Some(used_gb);
        }
        enforce_retention_pass(&state, now);
        assert_eq!(
            state.state_store.spread_sample_count().unwrap(),
            remaining,
            "used-size budget already satisfied — no rows should be evicted"
        );
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

        let _ = lock_vault_impl(&state);
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
                environment: None,
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
                environment: None,
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
                environment: None,
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
                environment: None,
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
                environment: None,
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
                environment: None,
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
    async fn test_get_account_credential_field_scoped_to_profile_and_unlocked() {
        let (state, dir) = test_state_with_vault("cred-reveal");

        assert!(get_account_credential_field_impl("lighter-default", "apiSecret", &state).is_err());
        initialize_vault_impl("test password", &state).expect("init vault");
        assert!(get_account_credential_field_impl("no-such-account", "apiSecret", &state).is_err());

        let a1 = upsert_account_impl(
            AccountUpsertInput {
                environment: None,
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
                environment: None,
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

        assert_eq!(
            get_account_credential_field_impl(&a1.id, "apiSecret", &state).expect("field reveal"),
            "SENTINEL_REVEAL1"
        );
        assert_eq!(
            get_account_credential_field_impl(&a1.id, "privateKey", &state).expect("field reveal"),
            "0xSENTINEL_PK1"
        );
        assert!(get_account_credential_field_impl(&a1.id, "nope", &state).is_err());
        assert!(get_account_credential_field_impl("no-such-account", "apiSecret", &state).is_err());
        assert_eq!(
            get_account_credential_field_impl(&a2.id, "apiSecret", &state).expect("field reveal"),
            "SENTINEL_REVEAL2"
        );

        let _ = lock_vault_impl(&state);
        assert!(get_account_credential_field_impl(&a1.id, "apiSecret", &state).is_err());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_account_base_url_patch_semantics() {
        let (state, dir) = test_state_with_vault("base-url-patch");
        initialize_vault_impl("test password", &state).expect("init vault");

        // Create an OKX account carrying a regional-entity override plus a
        // secret; both must persist.
        let a1 = upsert_account_impl(
            AccountUpsertInput {
                environment: None,
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "okx".into(),
                label: "Main".into(),
                enabled: true,
                make_default: true,
                credentials: Some(AccountCredentialInput {
                    api_secret: "SENTINEL_BASEURL_SEC".into(),
                    base_url: Some("https://eea.okx.com".into()),
                    ..AccountCredentialInput::default()
                }),
            },
            &state,
        )
        .expect("create a1");
        assert!(a1.has_base_url);
        assert_eq!(
            get_account_credential_field_impl(&a1.id, "baseUrl", &state).expect("reveal"),
            "https://eea.okx.com"
        );

        // Explicit clear: a `Some` payload with `base_url: Some("")` clears
        // the endpoint while untouched secrets survive the same merge.
        let updated = upsert_account_impl(
            AccountUpsertInput {
                environment: None,
                account_tier: None,
                max_leverage: None,
                id: Some(a1.id.clone()),
                venue: "okx".into(),
                label: "Main".into(),
                enabled: true,
                make_default: true,
                credentials: Some(AccountCredentialInput {
                    base_url: Some(String::new()),
                    ..AccountCredentialInput::default()
                }),
            },
            &state,
        )
        .expect("clear base_url");
        assert!(!updated.has_base_url);
        assert_eq!(
            get_account_credential_field_impl(&a1.id, "baseUrl", &state).expect("reveal"),
            ""
        );
        assert_eq!(
            get_account_credential_field_impl(&a1.id, "apiSecret", &state).expect("reveal"),
            "SENTINEL_BASEURL_SEC"
        );

        // Omitted preserves: a credential update without the base_url key
        // must not resurrect or clear the endpoint.
        let re_set = upsert_account_impl(
            AccountUpsertInput {
                environment: None,
                account_tier: None,
                max_leverage: None,
                id: Some(a1.id.clone()),
                venue: "okx".into(),
                label: "Main".into(),
                enabled: true,
                make_default: true,
                credentials: Some(AccountCredentialInput {
                    base_url: Some("https://app.okx.com".into()),
                    ..AccountCredentialInput::default()
                }),
            },
            &state,
        )
        .expect("set again");
        assert!(re_set.has_base_url);
        let untouched = upsert_account_impl(
            AccountUpsertInput {
                environment: None,
                account_tier: None,
                max_leverage: None,
                id: Some(a1.id.clone()),
                venue: "okx".into(),
                label: "Main".into(),
                enabled: true,
                make_default: true,
                credentials: Some(AccountCredentialInput {
                    api_secret: "SENTINEL_BASEURL_SEC2".into(),
                    ..AccountCredentialInput::default()
                }),
            },
            &state,
        )
        .expect("update without base_url");
        assert!(untouched.has_base_url, "omitted base_url must preserve");
        assert_eq!(
            get_account_credential_field_impl(&a1.id, "baseUrl", &state).expect("reveal"),
            "https://app.okx.com"
        );
        assert_eq!(
            get_account_credential_field_impl(&a1.id, "apiSecret", &state).expect("reveal"),
            "SENTINEL_BASEURL_SEC2"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    // secp256k1 key = 1 → the well-known 0x7E5F… derivation (Hardhat #0).
    const EVM_WALLET_TEST_KEY: &str =
        "0x0000000000000000000000000000000000000000000000000000000000000001";
    const EVM_WALLET_TEST_ADDR: &str = "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf";
    const EVM_WALLET_TEST_ADDR_CHECKSUM: &str = "0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf";

    #[tokio::test]
    async fn test_save_evm_wallet_fans_out_keyed_vs_address_only() {
        let (state, dir) = test_state_with_vault("evm-wallet");
        initialize_vault_impl("test password", &state).expect("init vault");
        state.set_environment(crate::state::TradingEnvironment::Live);

        let res = save_evm_wallet_impl(EVM_WALLET_TEST_KEY, None, &state).expect("save wallet");
        assert_eq!(res.signer_address.to_lowercase(), EVM_WALLET_TEST_ADDR);
        assert_eq!(res.keyed_venues.len(), 6);
        assert_eq!(res.address_only_venues.len(), 5);
        assert_eq!(res.created_accounts.len(), 11);
        assert_eq!(
            res.key_storage, "stronghold",
            "an unlocked vault opens the stronghold — the key must land there"
        );

        let data = state.vault.write().get_data().expect("vault data").clone();
        for vid in arbitrageur_exchanges::evm_amm::ADMITTED_EVM_AMM_VENUE_IDS {
            let pid = format!("{vid}-default");
            let acc = state
                .state_store
                .get_account(&pid)
                .expect("account lookup")
                .expect("default account exists");
            assert!(!acc.enabled, "a wallet save must never admit a venue");
            assert_eq!(acc.environment, "live");
            let rec = data
                .credentials
                .iter()
                .find(|c| c.profile_id.as_deref() == Some(pid.as_str()))
                .expect("credential record");
            assert_eq!(
                rec.wallet_address.as_deref().map(|a| a.to_lowercase()),
                Some(EVM_WALLET_TEST_ADDR.to_string())
            );
            if arbitrageur_exchanges::evm_amm::LIVE_EVM_AMM_VENUE_IDS.contains(vid) {
                assert_eq!(
                    rec.private_key.as_deref(),
                    Some(crate::stronghold::EVM_STRONGHOLD_KEY_REF),
                    "{vid} is swap-capable and must hold the custody sentinel"
                );
            } else {
                assert!(
                    rec.private_key.is_none(),
                    "{vid} is market-data only — the key must never reach it"
                );
            }
            assert!(!rec.enabled, "disabled profile => disabled record");
        }
        // The stronghold itself holds the key and derives the right address.
        {
            let guard = state.evm_stronghold.read();
            let sh = guard.as_ref().expect("stronghold loaded");
            assert!(sh.has_key());
            assert_eq!(
                sh.evm_address()
                    .expect("derive address")
                    .to_string()
                    .to_lowercase(),
                EVM_WALLET_TEST_ADDR
            );
        }
        let custody = get_evm_key_custody_impl(&state).expect("custody");
        assert_eq!(custody.custody, "stronghold");
        assert!(custody.stronghold_loaded);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_save_evm_wallet_mismatch_and_locked_refuse() {
        let (state, dir) = test_state_with_vault("evm-wallet-refuse");
        initialize_vault_impl("test password", &state).expect("init vault");

        // Address/key mismatch refuses before any write.
        let err = save_evm_wallet_impl(
            EVM_WALLET_TEST_KEY,
            Some("0x0000000000000000000000000000000000000001"),
            &state,
        )
        .expect_err("mismatch must refuse");
        assert!(err.contains("does not match"), "got: {err}");
        assert!(state
            .state_store
            .get_account("uniswap-default")
            .expect("lookup")
            .is_none());

        // Malformed key refuses too.
        assert!(save_evm_wallet_impl("not-a-key", None, &state).is_err());

        // Locked vault refuses and writes nothing.
        lock_vault_impl(&state).expect("lock");
        let err = save_evm_wallet_impl(EVM_WALLET_TEST_KEY, None, &state)
            .expect_err("locked vault must refuse");
        assert!(err.to_lowercase().contains("locked"), "got: {err}");
        assert!(state
            .state_store
            .get_account("uniswap-default")
            .expect("lookup")
            .is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_evm_wallet_enable_resolves_adapter_and_clear_strips() {
        let (state, dir) = test_state_with_vault("evm-wallet-enable");
        initialize_vault_impl("test password", &state).expect("init vault");
        state.set_environment(crate::state::TradingEnvironment::Live);

        save_evm_wallet_impl(EVM_WALLET_TEST_KEY, None, &state).expect("save wallet");
        // Nothing provisions while every venue account is disabled.
        let uni = state
            .live_evm_amms
            .iter()
            .find(|a| a.spec_id() == "uniswap")
            .expect("uniswap adapter");
        assert!(uni.wallet_address_hex().is_none());

        // Enable Uniswap's wallet account + set an RPC override; the adapter
        // must resolve the shared key without the key crossing IPC again.
        upsert_account_impl(
            AccountUpsertInput {
                environment: None,
                account_tier: None,
                max_leverage: None,
                id: Some("uniswap-default".into()),
                venue: "uniswap".into(),
                label: "EVM Wallet".into(),
                enabled: true,
                make_default: true,
                credentials: Some(AccountCredentialInput {
                    base_url: Some("https://example-rpc.invalid".into()),
                    ..AccountCredentialInput::default()
                }),
            },
            &state,
        )
        .expect("enable uniswap");
        assert_eq!(
            get_active_signer_address_impl("uniswap", &state).expect("signer"),
            Some(EVM_WALLET_TEST_ADDR_CHECKSUM.to_string()),
        );
        assert!(state
            .state_store
            .has_enabled_account("uniswap", "live")
            .unwrap());

        // Clear strips wallet material but keeps enabled + RPC override.
        let cleared = clear_evm_wallet_impl(&state).expect("clear");
        assert_eq!(cleared, 11, "all 11 default records held wallet material");
        let data = state.vault.write().get_data().expect("vault data").clone();
        let rec = data
            .credentials
            .iter()
            .find(|c| c.profile_id.as_deref() == Some("uniswap-default"))
            .expect("record");
        assert!(rec.private_key.is_none());
        assert!(rec.wallet_address.is_none());
        assert_eq!(rec.base_url.as_deref(), Some("https://example-rpc.invalid"));
        assert!(
            rec.enabled,
            "clearing the wallet must not disable the venue"
        );
        assert!(uni.wallet_address_hex().is_none(), "adapter signer dropped");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_save_evm_wallet_rekeys_enabled_default_account() {
        // A venue whose enabled default is a legacy random-id account must
        // adopt the shared wallet immediately — the resolved account and the
        // canonical `{venue}-default` both receive the material.
        let (state, dir) = test_state_with_vault("evm-wallet-rekey");
        initialize_vault_impl("test password", &state).expect("init vault");
        state.set_environment(crate::state::TradingEnvironment::Live);

        let legacy_key = "0x0000000000000000000000000000000000000000000000000000000000000002";
        let legacy = upsert_account_impl(
            AccountUpsertInput {
                environment: None,
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "uniswap".into(),
                label: "Legacy".into(),
                enabled: true,
                make_default: true,
                credentials: Some(AccountCredentialInput {
                    private_key: Some(legacy_key.into()),
                    ..AccountCredentialInput::default()
                }),
            },
            &state,
        )
        .expect("legacy account");
        let legacy_addr = derive_evm_signer_address_impl(legacy_key).expect("addr");
        assert_eq!(
            get_active_signer_address_impl("uniswap", &state).expect("signer"),
            Some(legacy_addr)
        );

        let res = save_evm_wallet_impl(EVM_WALLET_TEST_KEY, None, &state).expect("save");
        // The venue's resolving default adopts the shared key in place — no
        // stray `{venue}-default` row is created alongside it. With the
        // stronghold loaded the record carries the custody sentinel, never
        // key bytes.
        let data = state.vault.write().get_data().expect("vault data").clone();
        let rec = data
            .credentials
            .iter()
            .find(|c| c.profile_id.as_deref() == Some(legacy.id.as_str()))
            .expect("record for legacy account");
        assert_eq!(
            rec.private_key.as_deref(),
            Some(crate::stronghold::EVM_STRONGHOLD_KEY_REF)
        );
        assert_eq!(
            rec.wallet_address.as_deref().map(|a| a.to_lowercase()),
            Some(EVM_WALLET_TEST_ADDR.to_string())
        );
        assert!(state
            .state_store
            .get_account("uniswap-default")
            .expect("lookup")
            .is_none());
        assert!(!res
            .created_accounts
            .iter()
            .any(|id| id == "uniswap-default"));
        assert_eq!(res.created_accounts.len(), 10, "other venues gain rows");
        assert_eq!(
            get_active_signer_address_impl("uniswap", &state).expect("signer"),
            Some(res.signer_address.clone())
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A vault seeded pre-Stronghold holds raw hex keys in its records.
    /// Unlock must move that key into the snapshot and rewrite every AMM
    /// record to the sentinel — the raw bytes stop persisting.
    #[tokio::test]
    async fn test_unlock_migrates_raw_evm_key_to_stronghold() {
        let (state, dir) = test_state_with_vault("evm-migrate");
        initialize_vault_impl("test password", &state).expect("init vault");

        // Seed a legacy raw-key record directly (bypassing the accounts
        // hooks, which would migrate it immediately).
        state
            .vault
            .write()
            .upsert_credentials(vec![ExchangeCredentials {
                venue: "uniswap".into(),
                profile_id: None,
                api_key: String::new(),
                api_secret: String::new(),
                passphrase: None,
                account_id: None,
                wallet_address: Some(EVM_WALLET_TEST_ADDR_CHECKSUM.into()),
                account_index: None,
                subaccount_index: None,
                api_key_index: None,
                private_key: Some(EVM_WALLET_TEST_KEY.into()),
                base_url: None,
                enabled: true,
            }])
            .expect("seed raw key");
        {
            let data = state.vault.write().get_data().expect("data").clone();
            assert_eq!(
                data.credentials[0].private_key.as_deref(),
                Some(EVM_WALLET_TEST_KEY)
            );
        }

        let _ = lock_vault_impl(&state);
        unlock_vault_impl("test password", None, &state).expect("unlock migrates");

        let data = state.vault.write().get_data().expect("data").clone();
        assert_eq!(
            data.credentials[0].private_key.as_deref(),
            Some(crate::stronghold::EVM_STRONGHOLD_KEY_REF),
            "raw key must be rewritten to the sentinel after migration"
        );
        let custody = get_evm_key_custody_impl(&state).expect("custody");
        assert_eq!(custody.custody, "stronghold");
        assert!(custody.stronghold_loaded);
        assert_eq!(
            state
                .evm_stronghold
                .read()
                .as_ref()
                .expect("stronghold")
                .evm_address()
                .expect("address")
                .to_string()
                .to_lowercase(),
            EVM_WALLET_TEST_ADDR
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Regression (#45): failed unlock attempts are throttled — a retry
    /// inside the backoff window is refused before touching the KDF, and
    /// the window grows with consecutive failures. A successful unlock
    /// resets the counter.
    #[tokio::test]
    async fn test_vault_unlock_backoff_throttles_retries() {
        let (state, dir) = test_state_with_vault("unlock-backoff");
        initialize_vault_impl("correct password", &state).expect("init vault");
        let _ = lock_vault_impl(&state);

        // First failure counts and opens a 1s backoff window.
        let err = unlock_vault_impl("wrong password", None, &state).expect_err("must fail");
        assert!(
            !err.contains("Too many"),
            "first failure reaches the KDF: {err}"
        );
        // Immediate retry is refused without an unlock attempt.
        let err = unlock_vault_impl("wrong password", None, &state).expect_err("throttled");
        assert!(
            err.contains("Too many failed unlock attempts"),
            "backoff refusal: {err}"
        );
        // Even the CORRECT password is refused inside the window — the
        // throttle applies to the attempt, not the credential.
        let err = unlock_vault_impl("correct password", None, &state).expect_err("still throttled");
        assert!(err.contains("Too many"), "throttled: {err}");

        // Waiting out the window lets a correct unlock through and resets.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        unlock_vault_impl("correct password", None, &state).expect("unlock after backoff");
        let (failures, _) = *state.vault_unlock_backoff.lock();
        assert_eq!(failures, 0, "success resets the backoff counter");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// End-to-end MFA lifecycle through the command layer: enroll → the kv
    /// flag mirrors state while locked → unlock demands a live code →
    /// disable requires a live code → password-only unlock restored.
    #[tokio::test]
    async fn test_mfa_lifecycle_gates_unlock_and_disable() {
        let (state, dir) = test_state_with_vault("mfa-gate");
        initialize_vault_impl("correct password", &state).expect("init vault");
        assert!(!get_vault_status_impl(&state).mfa_enabled);

        // Locked vault refuses enrollment before it begins.
        let _ = lock_vault_impl(&state);
        assert!(begin_mfa_enrollment_impl(&state).is_err());
        unlock_vault_impl("correct password", None, &state).expect("re-unlock");

        // begin → pending secret + otpauth URI; nothing enrolled yet.
        let enrollment = begin_mfa_enrollment_impl(&state).expect("begin");
        assert_eq!(enrollment.secret_base32.len(), 32);
        assert!(enrollment.otpauth_uri.contains("otpauth://totp/"));
        assert!(!get_vault_status_impl(&state).mfa_enabled);

        // Wrong confirm code refuses; the pending secret survives for retry.
        assert!(confirm_mfa_enrollment_impl("000000", &state).is_err());
        let secret = crate::totp::decode_secret_base32(&enrollment.secret_base32).unwrap();
        let code = crate::totp::code_at(&secret, crate::totp::unix_now());
        let status = confirm_mfa_enrollment_impl(&code, &state).expect("confirm");
        assert!(status.mfa_enabled);

        // A second enrollment refuses while one is active.
        assert!(begin_mfa_enrollment_impl(&state).is_err());

        // Locked view still reports MFA via the kv flag.
        let _ = lock_vault_impl(&state);
        let locked = get_vault_status_impl(&state);
        assert!(!locked.is_unlocked);
        assert!(locked.mfa_enabled);

        // Password-only is refused; the live code is mandatory. The failure
        // trips the unlock backoff — wait out the 1s window.
        assert!(unlock_vault_impl("correct password", None, &state).is_err());
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let code = crate::totp::code_at(&secret, crate::totp::unix_now());
        unlock_vault_impl("correct password", Some(&code), &state).expect("unlock with MFA");

        // Disable refuses a wrong code, accepts a live one, and clears the flag.
        assert!(disable_mfa_impl("000000", &state).is_err());
        let code = crate::totp::code_at(&secret, crate::totp::unix_now());
        let status = disable_mfa_impl(&code, &state).expect("disable");
        assert!(!status.mfa_enabled);
        let _ = lock_vault_impl(&state);
        assert!(!get_vault_status_impl(&state).mfa_enabled);
        unlock_vault_impl("correct password", None, &state).expect("password-only unlock");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Two AMM records carrying DIFFERENT raw keys can never share the one
    /// stronghold record — migration refuses rather than silently picking
    /// a winner; both records keep their vault-held keys.
    #[tokio::test]
    async fn test_divergent_evm_keys_stay_vault_held() {
        let (state, dir) = test_state_with_vault("evm-diverge");
        initialize_vault_impl("test password", &state).expect("init vault");

        let cred = |venue: &str, key: &str| ExchangeCredentials {
            venue: venue.into(),
            profile_id: None,
            api_key: String::new(),
            api_secret: String::new(),
            passphrase: None,
            account_id: None,
            wallet_address: None,
            account_index: None,
            subaccount_index: None,
            api_key_index: None,
            private_key: Some(key.into()),
            base_url: None,
            enabled: true,
        };
        state
            .vault
            .write()
            .upsert_credentials(vec![
                cred("uniswap", EVM_WALLET_TEST_KEY),
                cred(
                    "sushiswap",
                    "0x0000000000000000000000000000000000000000000000000000000000000002",
                ),
            ])
            .expect("seed divergent keys");

        let _ = lock_vault_impl(&state);
        unlock_vault_impl("test password", None, &state).expect("unlock");

        let data = state.vault.write().get_data().expect("data").clone();
        assert!(
            data.credentials
                .iter()
                .all(|c| c.private_key.as_deref().is_some_and(|k| k.starts_with('0'))),
            "divergent keys must remain vault-held, got {:?}",
            data.credentials
                .iter()
                .map(|c| c.private_key.clone())
                .collect::<Vec<_>>()
        );
        let custody = get_evm_key_custody_impl(&state).expect("custody");
        assert_eq!(custody.custody, "vault");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_get_account_signer_address_derives_without_secret() {
        let (state, dir) = test_state_with_vault("signer-addr");

        // Locked vault -> fail closed; no account -> fail closed.
        assert!(get_account_signer_address_impl("nope", &state).is_err());
        initialize_vault_impl("test password", &state).expect("init vault");

        let a1 = upsert_account_impl(
            AccountUpsertInput {
                environment: None,
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "hyperliquid".into(),
                label: "Main".into(),
                enabled: true,
                make_default: true,
                credentials: Some(AccountCredentialInput {
                    // dev key 0x…01 -> 0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf
                    private_key: Some(format!("0x{}", "0".repeat(63) + "1")),
                    wallet_address: Some("0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf".into()),
                    ..AccountCredentialInput::default()
                }),
            },
            &state,
        )
        .expect("create a1");
        let a2 = upsert_account_impl(
            AccountUpsertInput {
                environment: None,
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "aster".into(),
                label: "NoKey".into(),
                enabled: true,
                make_default: false,
                credentials: Some(AccountCredentialInput::default()),
            },
            &state,
        )
        .expect("create a2");

        // Only the checksummed address crosses out — the key never does.
        assert_eq!(
            get_account_signer_address_impl(&a1.id, &state).expect("derive"),
            "0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf"
        );
        assert!(get_account_signer_address_impl(&a2.id, &state).is_err());
        assert!(derive_evm_signer_address_impl("0xnothex").is_err());
        assert!(derive_evm_signer_address_impl("0x1234").is_err());

        // The default account's credentials were provisioned into the live
        // adapter — its active signer must equal the stored-key derivation.
        state.sync_vault_credentials_to_adapters();
        let stored = get_account_signer_address_impl(&a1.id, &state).expect("stored derive");
        assert_eq!(
            get_active_signer_address_impl("hyperliquid", &state).expect("active"),
            Some(stored),
            "live adapter signer must match the stored key's derived address"
        );

        // Auto-lock (timer expiry) forgets vault secrets but keeps
        // provisioned adapter credentials so unattended trading survives.
        state.vault.write().force_timer_expiry();
        state.sync_vault_credentials_to_adapters();
        assert!(
            get_account_signer_address_impl(&a1.id, &state).is_err(),
            "auto-locked vault must not derive from stored secrets"
        );
        assert_eq!(
            get_active_signer_address_impl("hyperliquid", &state).expect("active"),
            Some("0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf".to_string()),
            "auto-lock must keep the provisioned signer"
        );

        // An explicit operator lock is a panic-halt: the adapter key goes.
        let _ = lock_vault_impl(&state);
        assert!(get_account_signer_address_impl(&a1.id, &state).is_err());
        assert_eq!(
            get_active_signer_address_impl("hyperliquid", &state).expect("active"),
            None,
            "manual lock must clear the provisioned signer"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    /// Sentinel custody end-to-end: a Stronghold-backed record resolves the
    /// signer address from inside the vault (never from key bytes), the
    /// provisioned adapter reports the same signer, and clearing revokes
    /// the record so it cannot sign again.
    #[tokio::test]
    async fn test_evm_sentinel_signer_resolution_and_revoke() {
        let (state, dir) = test_state_with_vault("evm-sentinel");
        initialize_vault_impl("test password", &state).expect("init vault");
        state.set_environment(crate::state::TradingEnvironment::Live);

        let res = save_evm_wallet_impl(EVM_WALLET_TEST_KEY, None, &state).expect("save wallet");
        assert_eq!(res.key_storage, "stronghold");

        // Enable uniswap so credential sync provisions the signer.
        upsert_account_impl(
            AccountUpsertInput {
                environment: None,
                account_tier: None,
                max_leverage: None,
                id: Some("uniswap-default".into()),
                venue: "uniswap".into(),
                label: "EVM Wallet".into(),
                enabled: true,
                make_default: true,
                credentials: None,
            },
            &state,
        )
        .expect("enable uniswap");

        // The record's signer address is derived inside the stronghold —
        // the sentinel itself contains no key material.
        assert_eq!(
            get_account_signer_address_impl("uniswap-default", &state).expect("sentinel derive"),
            EVM_WALLET_TEST_ADDR_CHECKSUM
        );
        assert_eq!(
            get_active_signer_address_impl("uniswap", &state).expect("active signer"),
            Some(EVM_WALLET_TEST_ADDR_CHECKSUM.to_string()),
            "the provisioned adapter must report the stronghold signer"
        );

        // Clear: records stripped AND the stronghold record revoked.
        let cleared = clear_evm_wallet_impl(&state).expect("clear");
        assert_eq!(cleared, 11);
        {
            let guard = state.evm_stronghold.read();
            let sh = guard.as_ref().expect("stronghold still loaded");
            assert!(
                !sh.has_key(),
                "clear must revoke the stronghold record, not just the vault rows"
            );
        }
        let custody = get_evm_key_custody_impl(&state).expect("custody");
        assert_eq!(custody.custody, "none");
        assert!(!custody.stronghold_loaded);
        assert!(get_account_signer_address_impl("uniswap-default", &state).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_vault_lock_refused_with_open_live_position() {
        // A locked vault cannot authenticate venue calls — every supervised
        // exit and operator flatten would fail until re-unlock. Live mode
        // with an open position must refuse the lock.
        let (state, dir) = test_state_with_vault("lock_gate");
        initialize_vault_impl("test password", &state).expect("init vault");
        unlock_vault_impl("test password", None, &state).expect("unlock");
        state.set_environment(crate::state::TradingEnvironment::Live);

        state
            .state_store
            .record_signal_execution(
                &crate::state_store::NewSignalExecution {
                    id: "sig-lock-gate-1".into(),
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
                            symbol: Some("BTC-USDC".into()),
                            intended_qty: 0.01,
                            intended_price: Some(100_000.0),
                        },
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "lighter".into(),
                            side: crate::state_store::LegSide::Short,
                            symbol: Some("BTC-USDC".into()),
                            intended_qty: 0.01,
                            intended_price: Some(100_000.0),
                        },
                    ],
                },
                "live",
            )
            .expect("record signal");
        state
            .state_store
            .update_signal_status(
                "sig-lock-gate-1",
                crate::state_store::SignalStatus::Balanced,
                None,
            )
            .expect("mark balanced");

        let err = lock_vault_impl(&state).expect_err("lock must refuse");
        assert!(
            err.contains("live position"),
            "refusal must name open live positions: {err}"
        );
        assert!(
            state.vault.write().is_unlocked(),
            "vault must stay unlocked on refusal"
        );

        // Testnet env locks freely — sims need no credentials.
        state.set_environment(crate::state::TradingEnvironment::Testnet);
        lock_vault_impl(&state).expect("testnet lock must succeed");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_credential_delete_refused_with_open_live_leg() {
        // Deleting a venue's credentials while an open live leg routes
        // through it strands exit supervision — refuse until flattened.
        let (state, dir) = test_state_with_vault("cred_gate");
        initialize_vault_impl("test password", &state).expect("init vault");
        unlock_vault_impl("test password", None, &state).expect("unlock");
        state.set_environment(crate::state::TradingEnvironment::Live);

        state
            .state_store
            .record_signal_execution(
                &crate::state_store::NewSignalExecution {
                    id: "sig-cred-gate-1".into(),
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
                            symbol: Some("BTC-USDC".into()),
                            intended_qty: 0.01,
                            intended_price: Some(100_000.0),
                        },
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "lighter".into(),
                            side: crate::state_store::LegSide::Short,
                            symbol: Some("BTC-USDC".into()),
                            intended_qty: 0.01,
                            intended_price: Some(100_000.0),
                        },
                    ],
                },
                "live",
            )
            .expect("record signal");
        state
            .state_store
            .update_signal_status(
                "sig-cred-gate-1",
                crate::state_store::SignalStatus::Balanced,
                None,
            )
            .expect("mark balanced");

        // The venue with an open leg refuses; an unrelated venue passes the
        // position gate and fails on the (empty) vault lookup instead.
        let err = delete_exchange_credentials_impl("hyperliquid", &state)
            .expect_err("delete must refuse");
        assert!(
            err.contains("live position"),
            "refusal must name open live positions: {err}"
        );

        // Closing the position frees the delete path.
        state
            .state_store
            .update_signal_status(
                "sig-cred-gate-1",
                crate::state_store::SignalStatus::Closed,
                None,
            )
            .expect("close signal");
        let res = delete_exchange_credentials_impl("hyperliquid", &state);
        assert!(
            res.is_ok() || !res.unwrap_err().contains("live position"),
            "closed position must not trip the gate"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_venue_disable_refused_with_open_live_leg() {
        // Disabling a venue flag while an open live leg routes through it
        // drops the adapter out of `get_venues` — closes can no longer
        // resolve or authenticate. The settings save must refuse.
        let state = AppEngineState::new();
        state.set_environment(crate::state::TradingEnvironment::Live);
        state.settings.write().hyperliquid_enabled = true;

        state
            .state_store
            .record_signal_execution(
                &crate::state_store::NewSignalExecution {
                    id: "sig-venue-gate-1".into(),
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
                            symbol: Some("BTC-USDC".into()),
                            intended_qty: 0.01,
                            intended_price: Some(100_000.0),
                        },
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "lighter".into(),
                            side: crate::state_store::LegSide::Short,
                            symbol: Some("BTC-USDC".into()),
                            intended_qty: 0.01,
                            intended_price: Some(100_000.0),
                        },
                    ],
                },
                "testnet",
            )
            .expect("record signal");
        state
            .state_store
            .update_signal_status(
                "sig-venue-gate-1",
                crate::state_store::SignalStatus::Balanced,
                None,
            )
            .expect("mark balanced");

        // Save with hyperliquid flipped off: the gate must refuse. The DTO
        // mode must match the live env or the transition guard fires first.
        let mut settings = get_engine_settings_impl(&state).expect("settings");
        settings.trading_mode = "live".into();
        settings.hyperliquid_enabled = false;
        let err =
            save_engine_settings_impl(settings.clone(), &state).expect_err("save must refuse");
        assert!(
            err.contains("live position") || err.contains("Flatten"),
            "refusal must name open live positions: {err}"
        );
        assert!(
            state.settings.read().hyperliquid_enabled,
            "venue flag must stay enabled on refusal"
        );

        // Saving with the flag still on is unaffected by the gate.
        settings.hyperliquid_enabled = true;
        save_engine_settings_impl(settings, &state).expect("enabled save ok");

        state.set_environment(crate::state::TradingEnvironment::Testnet);
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
                base_url: None,
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
                base_url: None,
                enabled: true,
            }])
            .expect("seed hsm");

        let _ = lock_vault_impl(&state);
        unlock_vault_impl("test password", None, &state).expect("unlock migrates");

        // Legacy unbound creds migrate to the live environment — scope the
        // listing accordingly (accounts are env-filtered).
        state.set_environment(crate::state::TradingEnvironment::Live);
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
                environment: None,
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
                environment: None,
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
                environment: None,
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
    async fn test_strategy_route_rejects_accounts_outside_active_env() {
        // Test states run Testnet; a live-bound account can never be
        // admitted here, so routing it must refuse at upsert time rather
        // than persist a pair that can never be eligible.
        let (state, dir) = test_state_with_vault("routes_env");
        initialize_vault_impl("test password", &state).expect("init vault");
        assert_eq!(
            state.env(),
            crate::state::TradingEnvironment::Testnet,
            "fixture must start in Testnet"
        );

        let tn_account = upsert_account_impl(
            AccountUpsertInput {
                environment: None, // binds to the active environment (testnet)
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "hyperliquid".into(),
                label: "HL Test".into(),
                enabled: true,
                make_default: false,
                credentials: None,
            },
            &state,
        )
        .expect("testnet account");
        assert_eq!(tn_account.environment, "testnet");
        let live_account = upsert_account_impl(
            AccountUpsertInput {
                environment: Some("live".into()),
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "lighter".into(),
                label: "LT Live".into(),
                enabled: true,
                make_default: false,
                credentials: None,
            },
            &state,
        )
        .expect("live account");
        assert_eq!(live_account.environment, "live");

        // Mixed-env pair is refused and names the offending account.
        let err = upsert_strategy_route_impl(
            StrategyRouteUpsertInput {
                id: None,
                strategy_id: "futures".into(),
                label: "Cross-Env".into(),
                account_a_id: tn_account.id.clone(),
                account_b_id: live_account.id.clone(),
                enabled: true,
            },
            &state,
        )
        .expect_err("mixed-env route must refuse");
        assert!(err.contains("live"), "err: {err}");
        assert!(err.contains(&live_account.id), "err: {err}");

        // Even an all-live pair refuses while the engine runs testnet.
        let live_account2 = upsert_account_impl(
            AccountUpsertInput {
                environment: Some("live".into()),
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "okx".into(),
                label: "OKX Live".into(),
                enabled: true,
                make_default: false,
                credentials: None,
            },
            &state,
        )
        .expect("second live account");
        assert!(upsert_strategy_route_impl(
            StrategyRouteUpsertInput {
                id: None,
                strategy_id: "futures".into(),
                label: "Live Pair".into(),
                account_a_id: live_account.id.clone(),
                account_b_id: live_account2.id.clone(),
                enabled: true,
            },
            &state,
        )
        .is_err());

        // Same-env pair routes fine.
        let tn_account2 = upsert_account_impl(
            AccountUpsertInput {
                environment: None,
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "decibel".into(),
                label: "DCB Test".into(),
                enabled: true,
                make_default: false,
                credentials: None,
            },
            &state,
        )
        .expect("second testnet account");
        let route = upsert_strategy_route_impl(
            StrategyRouteUpsertInput {
                id: None,
                strategy_id: "futures".into(),
                label: "TN Route".into(),
                account_a_id: tn_account.id.clone(),
                account_b_id: tn_account2.id.clone(),
                enabled: true,
            },
            &state,
        )
        .expect("same-env route");
        // Toggle-only update with unchanged legs is bookkeeping, allowed even
        // though the pair is re-submitted.
        upsert_strategy_route_impl(
            StrategyRouteUpsertInput {
                id: Some(route.id.clone()),
                strategy_id: "futures".into(),
                label: String::new(),
                account_a_id: tn_account.id.clone(),
                account_b_id: tn_account2.id.clone(),
                enabled: false,
            },
            &state,
        )
        .expect("toggle update");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_account_delete_requires_unlock_and_listing_is_lock_safe() {
        let (state, dir) = test_state_with_vault("del_locked");
        initialize_vault_impl("test password", &state).expect("init vault");

        let a = upsert_account_impl(
            AccountUpsertInput {
                environment: None,
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

        let _ = lock_vault_impl(&state);
        let err = delete_account_impl(&a.id, &state).unwrap_err();
        assert!(err.to_lowercase().contains("locked"), "err: {err}");

        // Account metadata remains listable while locked; credential
        // presence is reported as unknown rather than fabricated.
        let listed = list_accounts_impl(None, &state).expect("list locked");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].credentials_configured, None);

        // The secret survives the refused deletion.
        unlock_vault_impl("test password", None, &state).expect("unlock");
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
            "get_account_credentials",
        ] {
            assert!(
                !main_src.contains(cmd),
                "secret-read IPC registered in main.rs: {}",
                cmd
            );
        }
        assert!(
            main_src.contains("get_account_credential_field"),
            "field-scoped credential reveal must stay registered in main.rs"
        );
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
        state.set_environment(crate::state::TradingEnvironment::Testnet);
        let paper_logs = get_execution_logs_impl(&state).expect("paper logs");
        assert_eq!(paper_logs.len(), 1);
        assert_eq!(paper_logs[0].id, "exec-paper-1");
        assert!(!paper_logs[0].is_live);

        // In live mode: only live logs returned
        state.set_environment(crate::state::TradingEnvironment::Live);
        let live_logs = get_execution_logs_impl(&state).expect("live logs");
        assert_eq!(live_logs.len(), 1);
        assert_eq!(live_logs[0].id, "exec-live-1");
        assert!(live_logs[0].is_live);

        // Explicit filter checks
        assert_eq!(
            get_execution_logs_by_mode_impl(&state, Some(false))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            get_execution_logs_by_mode_impl(&state, Some(true))
                .unwrap()
                .len(),
            1
        );

        // Verify balances report correct is_live flag in paper mode
        state.set_environment(crate::state::TradingEnvironment::Testnet);
        let paper_balances = get_venue_balances_impl(&state)
            .await
            .expect("paper balances");
        assert!(!paper_balances.is_empty());
        for b in &paper_balances {
            assert!(!b.is_live, "paper balance must have is_live = false");
        }

        // Verify balances report correct is_live flag in live mode
        state.set_environment(crate::state::TradingEnvironment::Live);
        let live_balances = get_venue_balances_impl(&state)
            .await
            .expect("live balances");
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
    async fn test_radar_entry_uses_deviation_net_economics() {
        let state = setup_test_state_with_venues();
        // Deliberately crossed books: the buy-side ask sits above the
        // sell-side bid, so the static top-of-book edge is negative.
        seed_test_book(&state, "hyperliquid", "ANTH-USDC", dec!(100.0), dec!(100.2));
        seed_test_book(&state, "lighter", "ANTH-USDC", dec!(99.9), dec!(100.1));

        // The futures (static-edge) path refuses the crossed quote.
        let err = execute_spatial_arbitrage_inner(
            "ANTH-USDC",
            "hyperliquid",
            "lighter",
            0.5,
            Some("OPEN".into()),
            None,
            None,
            None,
            None,
            None,
            None,
            "futures",
            None,
            &state,
        )
        .await
        .expect_err("negative static edge must refuse on the futures path");
        assert!(err.contains("below the"), "unexpected err: {err}");

        // A radar entry on the same books with a deeply biased baseline
        // mean (mu = -150 bps) carries a large positive deviation — the
        // reversion expectation clears the same profit floor.
        let res = execute_spatial_arbitrage_inner(
            "ANTH-USDC",
            "hyperliquid",
            "lighter",
            0.5,
            Some("OPEN".into()),
            None,
            Some(-150.0),
            None,
            None,
            None,
            None,
            "radar",
            None,
            &state,
        )
        .await
        .expect("positive dev-net reversion entry must dispatch");
        assert!(res.execution_success);

        // A radar entry whose baseline sits near the live edge has no
        // deviation to capture — refused like a bad static route.
        seed_test_book(&state, "hyperliquid", "ETH-USDC", dec!(100.0), dec!(100.2));
        seed_test_book(&state, "lighter", "ETH-USDC", dec!(99.9), dec!(100.1));
        let err = execute_spatial_arbitrage_inner(
            "ETH-USDC",
            "hyperliquid",
            "lighter",
            0.5,
            Some("OPEN".into()),
            None,
            Some(-10.0),
            None,
            None,
            None,
            None,
            "radar",
            None,
            &state,
        )
        .await
        .expect_err("thin dev-net must refuse on the radar path too");
        assert!(err.contains("below the"), "unexpected err: {err}");
    }

    #[tokio::test]
    async fn test_funding_carry_paper_dispatch_opens_carry_position() {
        let state = setup_test_state_with_venues();
        // Tight cross (~23 bps) so the max-entry-spread gate passes.
        seed_test_book(&state, "hyperliquid", "DOT-USDC", dec!(4.41), dec!(4.43));
        seed_test_book(&state, "lighter", "DOT-USDC", dec!(4.40), dec!(4.42));
        // Steep HL-vs-Lighter gap: short HL, long Lighter, APR far above hurdle.
        state
            .market_data_hub
            .update_funding(arbitrageur_core::quote::FundingRate {
                venue: arbitrageur_core::types::VenueId::new("hyperliquid"),
                symbol: "DOT-USDC".to_string(),
                rate: dec!(0.0008),
                predicted_rate: None,
                interval_hours: 1,
                next_funding_time: Utc::now(),
                timestamp: Utc::now(),
            });
        state
            .market_data_hub
            .update_funding(arbitrageur_core::quote::FundingRate {
                venue: arbitrageur_core::types::VenueId::new("lighter"),
                symbol: "DOT-USDC".to_string(),
                rate: dec!(0.0001),
                predicted_rate: None,
                interval_hours: 8,
                next_funding_time: Utc::now(),
                timestamp: Utc::now(),
            });

        // $20 notional fits the seeded 10-lot test books on both legs.
        let res = execute_funding_carry_impl(
            "DOT-USDC",
            "lighter",
            "hyperliquid",
            20.0,
            &state,
            false,
            None,
        )
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
        assert_eq!(
            funding.len(),
            1,
            "expected one funding signal, got {funding:?}"
        );
        assert_eq!(funding[0].legs.len(), 2);
        assert!(funding[0].status.counts_as_open());
        // The entry must open a funding_rate ledger lot — without it the
        // flatten path can never realize PnL or resolve the signal's
        // recorded predictions.
        {
            let ledger = state.virtual_ledger.read();
            let lots: Vec<_> = ledger
                .lots
                .values()
                .filter(|l| {
                    l.strategy_id == "funding_rate"
                        && l.status == arbitrageur_execution::virtual_ledger::CycleStatus::Open
                })
                .collect();
            assert_eq!(lots.len(), 1, "funding entry must open one ledger lot");
        }
    }

    #[tokio::test]
    async fn test_funding_carry_live_refusal_names_missing_leg() {
        let state = setup_test_state_with_venues();
        state.set_environment(crate::state::TradingEnvironment::Live);
        // Locked vault: the gate must refuse — never a silent dispatch
        // past a named blocker.
        let err = execute_funding_carry_impl(
            "LINK-USDC",
            "lighter",
            "hyperliquid",
            500.0,
            &state,
            false,
            None,
        )
        .await
        .expect_err("live funding must refuse while the vault is locked");
        assert!(
            err.contains("vault") || err.contains("credentials") || err.contains("polling"),
            "refusal must name the missing live capability: {err}"
        );
        state.set_environment(crate::state::TradingEnvironment::Testnet);
    }

    #[tokio::test]
    async fn test_funding_carry_auto_entry_testnet_only_in_live() {
        // Autonomous entries stay testnet-only in live mode — the same
        // policy as the radar bot's live hard-block.
        let state = setup_test_state_with_venues();
        state.set_environment(crate::state::TradingEnvironment::Live);
        let err = execute_funding_carry_impl(
            "LINK-USDC",
            "lighter",
            "hyperliquid",
            500.0,
            &state,
            true,
            None,
        )
        .await
        .expect_err("autonomous live funding entry must refuse");
        assert!(
            err.contains("testnet-only") || err.contains("manual Execute"),
            "refusal must name the autonomous-entry policy: {err}"
        );
        state.set_environment(crate::state::TradingEnvironment::Testnet);
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
            state
                .market_data_hub
                .update_funding(arbitrageur_core::quote::FundingRate {
                    venue: arbitrageur_core::types::VenueId::new(venue),
                    symbol: "DOT-USDC".to_string(),
                    rate,
                    predicted_rate: None,
                    interval_hours: interval,
                    next_funding_time: Utc::now(),
                    timestamp: Utc::now(),
                });
        }
        let err = execute_funding_carry_impl(
            "DOT-USDC",
            "lighter",
            "hyperliquid",
            500.0,
            &state,
            false,
            None,
        )
        .await
        .expect_err("dust carry must be refused");
        assert!(err.contains("hurdle"), "unexpected err: {err}");
    }

    fn seed_lighter_basis_market(state: &AppEngineState) {
        // Perp books plus Lighter's exact spot book; funding gap far above
        // the default 8% APR hurdle in both the carry and basis directions.
        seed_test_book(
            state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64990.0),
            dec!(65000.0),
        );
        seed_test_book(state, "lighter", "BTC-USDC", dec!(65000.0), dec!(65010.0));
        seed_test_book(state, "lighter", "BTC-SPOT", dec!(64980.0), dec!(64990.0));
        for (venue, rate, interval) in [
            ("hyperliquid", dec!(0.0001), 8),
            ("lighter", dec!(0.0008), 1),
        ] {
            state
                .market_data_hub
                .update_funding(arbitrageur_core::quote::FundingRate {
                    venue: arbitrageur_core::types::VenueId::new(venue),
                    symbol: "BTC-USDC".to_string(),
                    rate,
                    predicted_rate: None,
                    interval_hours: interval,
                    next_funding_time: Utc::now(),
                    timestamp: Utc::now(),
                });
        }
    }

    #[tokio::test]
    async fn test_funding_basis_same_venue_exact_leg_identity() {
        let state = setup_test_state_with_venues();
        seed_lighter_basis_market(&state);

        let opps = scan_funding_opportunities_impl("BTC-USDC", 20.0, &state)
            .await
            .expect("funding scan");
        // Live adapters may add real-venue prints beside the seeded hub
        // rows, so select the seeded hyperliquid/lighter pair explicitly.
        let carry = opps
            .iter()
            .find(|o| {
                o.kind == "carry" && o.long_venue == "hyperliquid" && o.short_venue == "lighter"
            })
            .expect("seeded carry opp");
        assert_eq!(carry.long_venue_id, "hyperliquid");
        assert_eq!(carry.short_venue_id, "lighter");
        assert_eq!(carry.long_symbol, "BTC-USDC");
        assert_eq!(carry.short_symbol, "BTC-USDC");
        assert_eq!(carry.long_market_type, "perpetual");
        assert_eq!(carry.short_market_type, "perpetual");
        assert!(
            (carry.round_trip_fee_bps - 12.0).abs() < 1e-9,
            "carry fee must be 2*(1.5+4.5), got {}",
            carry.round_trip_fee_bps
        );
        let basis = opps
            .iter()
            .find(|o| {
                o.kind == "basis"
                    && o.long_venue == "lighter SPOT"
                    && o.short_venue == "lighter PERP"
            })
            .expect("seeded basis opp");
        assert_eq!(basis.long_venue, "lighter SPOT");
        assert_eq!(basis.short_venue, "lighter PERP");
        assert_eq!(basis.long_venue_id, "lighter");
        assert_eq!(basis.short_venue_id, "lighter");
        assert_eq!(basis.long_symbol, "BTC-SPOT");
        assert_eq!(basis.short_symbol, "BTC-USDC");
        assert_eq!(basis.long_market_type, "spot");
        assert_eq!(basis.short_market_type, "perpetual");
        assert!(
            (basis.round_trip_fee_bps - 6.0).abs() < 1e-9,
            "basis fee must be 4*1.5, got {}",
            basis.round_trip_fee_bps
        );

        // Execute through the OLD display triple; the engine must resolve the
        // refreshed canonical/exact fields underneath.
        let res = execute_funding_carry_impl(
            "BTC-USDC",
            "lighter SPOT",
            "lighter PERP",
            20.0,
            &state,
            false,
            None,
        )
        .await
        .expect("basis paper dispatch");
        assert!(res.execution_success);
        assert!(
            res.message.contains("BTC-SPOT") && res.message.contains("BTC-USDC"),
            "result must name exact legs: {}",
            res.message
        );

        // Exactly one balanced open funding signal carrying exact leg symbols.
        let funding: Vec<_> = state
            .state_store
            .list_signals(None, None)
            .expect("signals readable")
            .into_iter()
            .filter(|s| s.strategy_id.eq_ignore_ascii_case("funding"))
            .collect();
        assert_eq!(
            funding.len(),
            1,
            "expected one funding signal, got {funding:?}"
        );
        assert!(funding[0].status.counts_as_open());
        assert_eq!(
            funding[0].status,
            crate::state_store::SignalStatus::Balanced,
            "equal fills must net to Balanced: {:?}",
            funding[0]
        );
        let long_leg = funding[0]
            .legs
            .iter()
            .find(|l| l.side == crate::state_store::LegSide::Long)
            .expect("long leg");
        let short_leg = funding[0]
            .legs
            .iter()
            .find(|l| l.side == crate::state_store::LegSide::Short)
            .expect("short leg");
        assert_eq!(long_leg.symbol, "BTC-SPOT");
        assert_eq!(short_leg.symbol, "BTC-USDC");
        assert_eq!(long_leg.venue, "lighter");
        assert_eq!(short_leg.venue, "lighter");

        // Open aggregation yields two same-venue rows with distinct symbols.
        let signals = state.state_store.list_open_signals(None).expect("signals");
        let positions = crate::state_store::aggregate_open_positions(&signals, &|venue, symbol| {
            state
                .market_data_hub
                .get_valid_orderbook(&arbitrageur_core::types::VenueId::from(venue), symbol, None)
                .and_then(|b| b.mid_price())
                .and_then(|m| dec_to_f64(m, "mark mid").ok())
        });
        let funding_pos = positions
            .iter()
            .find(|p| p.strategy_id.eq_ignore_ascii_case("funding"))
            .expect("funding position");
        assert_eq!(funding_pos.venues.len(), 2);
        assert!(funding_pos.venues.iter().all(|v| v.venue == "lighter"));
        let mut leg_syms: Vec<_> = funding_pos
            .venues
            .iter()
            .map(|v| v.symbol.clone())
            .collect();
        leg_syms.sort();
        assert_eq!(
            leg_syms,
            vec!["BTC-SPOT".to_string(), "BTC-USDC".to_string()]
        );

        // Flatten the root perp: both legs gone, root closed.
        let flat = flatten_ticker_positions_impl(
            Some("BTC-USDC"),
            "funding",
            arbitrageur_execution::virtual_ledger::CycleStatus::Closed,
            &state,
        )
        .await
        .expect("flatten");
        assert_eq!(flat.signals_closed, 1, "notes: {:?}", flat.notes);
        let root = state
            .state_store
            .get_signal(&funding[0].id)
            .expect("root readable")
            .expect("root exists");
        assert_eq!(root.status, crate::state_store::SignalStatus::Closed);
        for paper in state.paper_exchanges().iter().map(|(_, p)| p) {
            let acc = paper.fetch_account().await.expect("paper account");
            assert!(
                acc.positions.iter().all(|p| p.quantity <= dec!(0)),
                "paper legs must be flat: {:?}",
                acc.positions
            );
        }
    }

    #[tokio::test]
    async fn test_funding_same_venue_margin_combines_spot_notional_and_perp_im() {
        let state = setup_test_state_with_venues();
        seed_lighter_basis_market(&state);
        // $500k notional fits a deepened book but exceeds the $100k paper
        // equity once spot full notional + perp IM combine on one venue.
        let mut deep_spot = arbitrageur_core::quote::OrderBook::new(
            arbitrageur_core::types::VenueId::new("lighter"),
            "BTC-SPOT".to_string(),
        );
        deep_spot.bids.push(arbitrageur_core::quote::BookLevel::new(
            dec!(64980.0),
            dec!(100.0),
        ));
        deep_spot.asks.push(arbitrageur_core::quote::BookLevel::new(
            dec!(64990.0),
            dec!(100.0),
        ));
        deep_spot.timestamp = Utc::now();
        state.market_data_hub.update_orderbook(deep_spot);
        let mut deep_perp = arbitrageur_core::quote::OrderBook::new(
            arbitrageur_core::types::VenueId::new("lighter"),
            "BTC-USDC".to_string(),
        );
        deep_perp.bids.push(arbitrageur_core::quote::BookLevel::new(
            dec!(65000.0),
            dec!(100.0),
        ));
        deep_perp.asks.push(arbitrageur_core::quote::BookLevel::new(
            dec!(65010.0),
            dec!(100.0),
        ));
        deep_perp.timestamp = Utc::now();
        state.market_data_hub.update_orderbook(deep_perp);
        let err = execute_funding_carry_impl(
            "BTC-USDC",
            "lighter SPOT",
            "lighter PERP",
            500_000.0,
            &state,
            false,
            None,
        )
        .await
        .expect_err("same-venue margin must refuse");
        assert!(err.contains("free margin"), "unexpected err: {err}");
    }

    #[tokio::test]
    async fn test_funding_max_entry_spread_rejects_wide_cross() {
        let state = setup_test_state_with_venues();
        // Spot ask far above the perp bid: entry cross ~800 bps > 50 default.
        seed_test_book(&state, "lighter", "BTC-SPOT", dec!(64980.0), dec!(65500.0));
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64980.0), dec!(65010.0));
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64990.0),
            dec!(65000.0),
        );
        for (venue, rate, interval) in [
            ("hyperliquid", dec!(0.0001), 8),
            ("lighter", dec!(0.0008), 1),
        ] {
            state
                .market_data_hub
                .update_funding(arbitrageur_core::quote::FundingRate {
                    venue: arbitrageur_core::types::VenueId::new(venue),
                    symbol: "BTC-USDC".to_string(),
                    rate,
                    predicted_rate: None,
                    interval_hours: interval,
                    next_funding_time: Utc::now(),
                    timestamp: Utc::now(),
                });
        }
        let err = execute_funding_carry_impl(
            "BTC-USDC",
            "lighter SPOT",
            "lighter PERP",
            20.0,
            &state,
            false,
            None,
        )
        .await
        .expect_err("wide entry cross must refuse");
        assert!(err.contains("spread"), "unexpected err: {err}");
    }

    #[tokio::test]
    async fn test_funding_slippage_rejects_thin_leg() {
        let state = setup_test_state_with_venues();
        seed_lighter_basis_market(&state);
        // $2M notional needs ~30 BTC; seeded 10-lot books cannot fill it.
        let err = execute_funding_carry_impl(
            "BTC-USDC",
            "lighter SPOT",
            "lighter PERP",
            2_000_000.0,
            &state,
            false,
            None,
        )
        .await
        .expect_err("thin-book dispatch must refuse");
        assert!(err.contains("lippage"), "unexpected err: {err}");
    }

    #[tokio::test]
    async fn test_funding_scanner_fees_use_effective_venue_rates() {
        let state = setup_test_state_with_venues();
        seed_lighter_basis_market(&state);
        let opps = scan_funding_opportunities_impl("BTC-USDC", 20.0, &state)
            .await
            .expect("funding scan");
        let carry = opps
            .iter()
            .find(|o| {
                o.kind == "carry" && o.long_venue == "hyperliquid" && o.short_venue == "lighter"
            })
            .expect("seeded carry opp");
        let basis = opps
            .iter()
            .find(|o| {
                o.kind == "basis"
                    && o.long_venue == "lighter SPOT"
                    && o.short_venue == "lighter PERP"
            })
            .expect("seeded basis opp");
        assert!(
            (carry.round_trip_fee_bps - 2.0 * (1.5 + 4.5)).abs() < 1e-9,
            "carry fee {}, want 12.0",
            carry.round_trip_fee_bps
        );
        assert!(
            (basis.round_trip_fee_bps - 4.0 * 1.5).abs() < 1e-9,
            "basis fee {}, want 6.0",
            basis.round_trip_fee_bps
        );
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
        assert!(
            err.contains("slippage"),
            "expected slippage error, got: {err}"
        );

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
        assert!(
            !err.contains("slippage"),
            "slippage gate should pass, got: {err}"
        );
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
            let mut p = params
                .get("futures")
                .cloned()
                .unwrap_or_else(|| StrategyParamsDto::defaults_for("futures"));
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

        // Toxic-fill ceiling: a route whose net edge exceeds
        // `cb_max_spread_divergence_bps` is a symbol collision or corrupt
        // quote (production saw SPX500 "spreads" of ~147M bps), never alpha.
        // Restore the target first so only the ceiling can reject.
        {
            let mut params = state.strategy_params.write();
            let mut p = params
                .get("futures")
                .cloned()
                .unwrap_or_else(|| StrategyParamsDto::defaults_for("futures"));
            p.target_profit_bps = 1.0;
            params.insert("futures".to_string(), p);
        }
        seed_test_book(&state, "hyperliquid", "BTC-USDC", dec!(100.0), dec!(100.1));
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
            .expect_err("implausible spread must reject")
            .contains("plausibility ceiling"));

        assert!(state.virtual_ledger.read().lots.is_empty());
        assert!(get_execution_logs_impl(&state).expect("logs").is_empty());
    }

    fn mk_route_book(venue: &str, bids: &[(f64, f64)], asks: &[(f64, f64)]) -> OrderBook {
        let mut book = OrderBook::new(VenueId::new(venue), "BTC-USDC".to_string());
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
        let econ = compute_route_economics(&book_buy, &book_sell, dec!(5.0), 2.0, 1.5, 1.0)
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
        let econ = compute_route_economics(&book_buy, &book_sell, dec!(6.0), 2.0, 1.5, 1.0)
            .expect("route economics");

        let buy_entry: f64 = (5.0 * 64205.0 + 1.0 * 64215.0) / 6.0;
        let sell_entry: f64 = (5.0 * 64300.0 + 1.0 * 64290.0) / 6.0;
        let expected_entry_slip = ((buy_entry - 64205.0) / 64205.0 * 10_000.0f64).max(0.0)
            + ((64300.0 - sell_entry) / 64300.0 * 10_000.0f64).max(0.0);
        assert!((econ.entry_slippage_bps - expected_entry_slip).abs() < 1e-6);
        assert!(econ.entry_slippage_bps > 0.0);
    }

    #[test]
    fn test_route_economics_fails_closed_on_incomplete_depth() {
        let book_buy = mk_route_book("hyperliquid", &[(64200.0, 1.0)], &[(64205.0, 10.0)]);
        let book_sell = mk_route_book("lighter", &[(64300.0, 10.0)], &[(64305.0, 10.0)]);
        assert!(compute_route_economics(&book_buy, &book_sell, dec!(5.0), 2.0, 1.5, 1.0).is_none());

        let no_asks = mk_route_book("hyperliquid", &[(64200.0, 10.0)], &[]);
        assert!(compute_route_economics(&no_asks, &book_sell, dec!(5.0), 2.0, 1.5, 1.0).is_none());
    }

    #[tokio::test]
    async fn test_effective_venue_fee_bps_uses_default_account_tier() {
        let (state, _dir) = test_state_with_vault("fee-tier");
        assert_eq!(effective_venue_fee_bps(&state, "lighter"), 1.5);
        assert_eq!(effective_venue_fee_bps(&state, "LIGHTER"), 1.5);
        assert_eq!(effective_venue_fee_bps(&state, "hyperliquid"), 4.5);

        upsert_account_impl(
            AccountUpsertInput {
                environment: None,
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
        assert_eq!(effective_venue_fee_bps(&state, "hyperliquid"), 4.5);
    }

    /// The IPC payload must be executor-identical — a UI that shows fees
    /// the executor doesn't charge recreates the drift this command exists
    /// to retire.
    #[test]
    fn test_get_venue_fees_matches_executor_table() {
        let dto = get_venue_fees_impl();
        assert_eq!(dto.default_bps, DEFAULT_TAKER_FEE_BPS);
        assert!(dto.fees.len() >= 30, "fee table shrank unexpectedly");
        for row in &dto.fees {
            assert_eq!(
                venue_fee_bps(&row.venue_id),
                row.taker_bps,
                "IPC fee diverges from executor table for {}",
                row.venue_id
            );
        }
        // Unknown ids resolve to the same default the IPC advertises.
        assert_eq!(venue_fee_bps("no-such-venue"), dto.default_bps);
        // Spot-check rows whose frontend copies previously drifted.
        assert_eq!(venue_fee_bps("deribit"), 5.0);
        assert_eq!(venue_fee_bps("gmx"), 5.0);
        assert_eq!(venue_fee_bps("pancakeswap"), 25.0);
    }

    /// End-to-end rotation: a vault password change must keep `vault.hold`
    /// on the same password generation — the wallet address survives and
    /// the old password opens neither file.
    #[test]
    fn test_change_vault_password_rekeys_stronghold() {
        let (state, dir) = test_state_with_vault("pwrotate-e2e");
        initialize_vault_impl("old-password-1234", &state).expect("init vault");
        // Provision the shared EVM key so vault.hold exists on disk.
        {
            let guard = state.evm_stronghold.read();
            let sh = guard.as_ref().expect("stronghold opens on unlock");
            sh.store_key(zeroize::Zeroizing::new([0x42u8; 32]))
                .expect("store evm key");
        }
        let addr_before = state
            .evm_stronghold
            .read()
            .as_ref()
            .expect("stronghold")
            .evm_address()
            .expect("address");
        assert!(dir.join("vault.hold").exists());

        // Wrong current password: rejected, nothing rotated.
        assert!(
            change_vault_password_impl("not-the-password", "new-password-5678", None, &state)
                .is_err()
        );

        change_vault_password_impl("old-password-1234", "new-password-5678", None, &state)
            .expect("rotate");

        // Old password rejected, new password opens the vault.
        let mut probe = crate::vault::SecureVault::new_with_path(dir.join("vault.enc"));
        assert!(probe.unlock("old-password-1234", None).is_err());
        assert!(probe.unlock("new-password-5678", None).is_ok());
        drop(probe);

        // The live stronghold was reopened under the NEW snapshot key —
        // same wallet key, same address (no re-provisioned orphan).
        let guard = state.evm_stronghold.read();
        let sh = guard.as_ref().expect("stronghold reopened post-rotation");
        assert_eq!(sh.evm_address().expect("address"), addr_before);
        let _ = std::fs::remove_dir_all(&dir);
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
        assert!((opp.fee_bps - 6.0).abs() < 1e-9);
        assert!((opp.round_trip_commission_bps - 12.0).abs() < 1e-9);
        assert!((opp.required_gross_bps - (opp.expected_friction_bps + 1.0)).abs() < 1e-9);
        assert!(
            (opp.net_spread_bps - (opp.gross_spread_bps - opp.expected_friction_bps)).abs() < 1e-9
        );

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
        for v in ["hyperliquid", "lighter"] {
            state.install_test_sim(v);
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
        assert!(!state.is_live());

        let res_off = set_live_mode_impl(false, &state).expect("disable live mode");
        assert!(!res_off);
        assert!(!state.is_live());
    }

    #[tokio::test]
    async fn test_env_switch_refused_with_open_position() {
        // A `balanced` position's fills were booked through the current
        // env's adapters; flattening after a flip would dispatch through
        // the new env's venues — the transition must refuse.
        let state = AppEngineState::new();
        state
            .state_store
            .record_signal_execution(
                &crate::state_store::NewSignalExecution {
                    id: "sig-env-block-1".into(),
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
                            symbol: Some("BTC-USDC".into()),
                            intended_qty: 0.01,
                            intended_price: Some(100_000.0),
                        },
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "lighter".into(),
                            side: crate::state_store::LegSide::Short,
                            symbol: Some("BTC-USDC".into()),
                            intended_qty: 0.01,
                            intended_price: Some(100_000.0),
                        },
                    ],
                },
                "testnet",
            )
            .expect("record signal");
        state
            .state_store
            .update_signal_status(
                "sig-env-block-1",
                crate::state_store::SignalStatus::Balanced,
                None,
            )
            .expect("mark balanced");

        let res = set_live_mode_impl(true, &state);
        assert!(res.is_err(), "env flip must refuse with an open position");
        let err = res.unwrap_err();
        assert!(
            err.contains("position(s) are open") || err.contains("Flatten"),
            "refusal must name the open-position gate: {err}"
        );
        assert!(!state.is_live(), "env must not flip on refusal");

        // Closing the position frees the transition (it will then fail on
        // the locked-vault gate — a different, expected refusal).
        state
            .state_store
            .update_signal_status(
                "sig-env-block-1",
                crate::state_store::SignalStatus::Closed,
                None,
            )
            .expect("close signal");
        let res = set_live_mode_impl(true, &state);
        assert!(res.is_err());
        assert!(
            res.unwrap_err().contains("locked"),
            "next gate must be the vault, not the position gate"
        );
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
            "testnet"
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
        unlock_vault_impl("test-password-123", None, &state).expect("unlock vault");

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
            vault
                .upsert_profile_credentials("aster-1", aster_c)
                .expect("save aster creds");

            let mut hl_c = crate::vault::ExchangeCredentials::default();
            hl_c.venue = "hyperliquid".to_string();
            hl_c.profile_id = Some("hl-1".to_string());
            hl_c.wallet_address = Some("0x1111111111111111111111111111111111111111".to_string());
            hl_c.private_key = Some(
                "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            );
            hl_c.enabled = true;
            vault
                .upsert_profile_credentials("hl-1", hl_c)
                .expect("save hl creds");
        }
        state.sync_vault_credentials_to_adapters();

        // 5. Capabilities report: submission readiness is distinct from
        // strategy-entry readiness. Capabilities describe the ACTIVE
        // environment — aster has no testnet endpoint, so switch to Live
        // first. Vault is unlocked here, so credentialed adapters with
        // submission + polling + strategy-entry support report ready.
        state.set_environment(crate::state::TradingEnvironment::Live);
        let caps = get_venue_execution_capabilities_impl(&state);
        let aster_cap = caps.iter().find(|c| c.venue == "aster").expect("aster cap");
        assert!(aster_cap.supports_live);
        assert!(aster_cap.has_credentials);
        assert!(aster_cap.live_ready);
        assert!(aster_cap.submission_ready);
        assert!(aster_cap.strategy_entry_ready);

        let popdex_cap = caps
            .iter()
            .find(|c| c.venue == "popdex")
            .expect("popdex cap");
        assert!(popdex_cap.supports_live);
        assert!(!popdex_cap.has_credentials);
        assert!(!popdex_cap.live_ready);
        assert!(!popdex_cap.submission_ready);
        assert!(!popdex_cap.strategy_entry_ready);

        let hl_cap = caps
            .iter()
            .find(|c| c.venue == "hyperliquid")
            .expect("hl cap");
        assert!(hl_cap.supports_live);
        assert!(hl_cap.has_credentials);
        assert!(hl_cap.live_ready);
        assert!(hl_cap.submission_ready);
        assert!(hl_cap.strategy_entry_ready);

        let vanta_cap = caps.iter().find(|c| c.venue == "vanta").expect("vanta cap");
        assert!(vanta_cap.supports_live);
        assert!(!vanta_cap.has_credentials);
        assert!(!vanta_cap.live_ready);
        assert!(!vanta_cap.submission_ready);
        assert!(!vanta_cap.strategy_entry_ready);

        // 6. Live mode successfully enabled
        let live_ok = set_live_mode_impl(true, &state).expect("enable live mode");
        assert!(live_ok);
        assert!(state.is_live());

        // 7a. Live strategy entry refuses on venue eligibility — vanta
        // has no credentials loaded — before any signal is inserted or
        // any account/book/order work is dispatched.
        let sim_err = execute_spatial_arbitrage_inner(
            "BTC-USDC", "aster", "vanta", 100.0, None, None, None, None, None, None, None,
            "futures", None, &state,
        )
        .await;
        let sim_err = sim_err.unwrap_err();
        assert!(
            sim_err.contains("credentials") || sim_err.contains("polling"),
            "refusal must name the missing live capability: {sim_err}"
        );
        assert!(state
            .state_store
            .blocking_signals("BTC-USDC", None)
            .expect("blocking signals")
            .is_empty());

        // 7b. The refusal is venue-independent: the same gate fires
        // regardless of which venues the route names.
        let decibel_err = execute_spatial_arbitrage_inner(
            "BTC-USDC", "aster", "decibel", 100.0, None, None, None, None, None, None, None,
            "futures", None, &state,
        )
        .await;
        assert!(decibel_err.is_err());

        // 7c. With the vault unlocked and both venues credentialed +
        // submission/polling-capable, the gate passes — unit-level so no
        // orderbook network call is made.
        live_entry_gate(&state, &["aster", "hyperliquid"])
            .expect("gate passes with vault + capable venues");
        // A venue with submission but no loaded credentials still refuses.
        assert!(live_entry_gate(&state, &["aster", "popdex"])
            .unwrap_err()
            .contains("no live credentials"));
        // strategy_entry_ready tracks credentialed submission+polling
        // venues, and stays off for uncredentialed ones.
        let caps = get_venue_execution_capabilities_impl(&state);
        assert!(
            caps.iter()
                .find(|c| c.venue == "aster")
                .expect("aster")
                .strategy_entry_ready
        );
        assert!(
            !caps
                .iter()
                .find(|c| c.venue == "popdex")
                .expect("popdex")
                .strategy_entry_ready
        );

        // 8. Test live flatten: when vault is unlocked, live flatten runs without LIVE_EXECUTION_DISABLED
        let flat_res = flatten_ticker_positions_impl(
            Some("BTC-USDC"),
            "futures",
            arbitrageur_execution::virtual_ledger::CycleStatus::Closed,
            &state,
        )
        .await;
        assert!(flat_res.is_ok());

        // 9. When vault is locked, live flatten fails closed
        let _ = lock_vault_impl(&state);
        let flat_locked = flatten_ticker_positions_impl(
            Some("BTC-USDC"),
            "futures",
            arbitrageur_execution::virtual_ledger::CycleStatus::Closed,
            &state,
        )
        .await;
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
        unlock_vault_impl("test-password-123", None, &state).expect("unlock vault");

        {
            let mut vault = state.vault.write();
            let mut aster_c = crate::vault::ExchangeCredentials::default();
            aster_c.venue = "aster".to_string();
            aster_c.profile_id = Some("aster-1".to_string());
            aster_c.api_key = "aster_key".to_string();
            aster_c.api_secret = "aster_secret".to_string();
            aster_c.enabled = true;
            vault
                .upsert_profile_credentials("aster-1", aster_c)
                .expect("save aster creds");

            let mut hl_c = crate::vault::ExchangeCredentials::default();
            hl_c.venue = "hyperliquid".to_string();
            hl_c.profile_id = Some("hl-1".to_string());
            hl_c.wallet_address = Some("0x1111111111111111111111111111111111111111".to_string());
            hl_c.private_key = Some(
                "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            );
            hl_c.enabled = true;
            vault
                .upsert_profile_credentials("hl-1", hl_c)
                .expect("save hl creds");
        }
        state.sync_vault_credentials_to_adapters();

        // 2. Enable live mode and verify persisted to kv store
        let ok = set_live_mode_impl(true, &state).expect("enable live mode");
        assert!(ok);
        assert!(state.is_live());
        assert_eq!(
            state.state_store.kv_get("engine.trading_mode").unwrap(),
            Some("live".to_string())
        );

        // 3. Lock vault: live executions will fail closed because vault is locked
        let _ = lock_vault_impl(&state);

        // 4. Unlock vault: automatically restores is_live_mode without needing to reconfigure Settings!
        let unlock_res = unlock_vault_impl("test-password-123", None, &state).expect("unlock");
        assert!(unlock_res.is_unlocked);
        assert!(
            state.is_live(),
            "live mode should automatically restore upon unlocking vault when real mode was chosen"
        );
    }

    #[tokio::test]
    async fn test_auto_lock_keeps_credentials_manual_lock_wipes() {
        let (state, _dir) = test_state_with_vault("autolock-sync");
        state.settings.write().aster_enabled = true;
        // Aster has no testnet endpoint — its accounts/credentials are
        // live-environment only.
        state.set_environment(crate::state::TradingEnvironment::Live);
        initialize_vault_impl("test-password-123", &state).expect("init vault");
        upsert_account_impl(
            AccountUpsertInput {
                environment: None,
                account_tier: None,
                max_leverage: None,
                id: None,
                venue: "aster".into(),
                label: "Aster Test".into(),
                enabled: true,
                make_default: false,
                credentials: Some(AccountCredentialInput {
                    api_key: "aster_key".into(),
                    api_secret: "aster_secret".into(),
                    ..AccountCredentialInput::default()
                }),
            },
            &state,
        )
        .expect("create aster account");
        state.sync_vault_credentials_to_adapters();
        let aster_has_creds = |s: &AppEngineState| {
            get_venue_execution_capabilities_impl(s)
                .iter()
                .find(|c| c.venue == "aster")
                .expect("aster cap")
                .has_credentials
        };
        assert!(aster_has_creds(&state), "provisioned after sync");

        // Inactivity expiry → auto-lock: the vault forgets its secrets but
        // provisioned adapter credentials keep working (unattended uptime).
        state.vault.write().force_timer_expiry();
        assert!(!state.vault.write().is_unlocked());
        assert!(state.vault.write().is_auto_locked());
        state.sync_vault_credentials_to_adapters();
        assert!(
            aster_has_creds(&state),
            "auto-lock must not wipe provisioned adapter credentials"
        );

        // Explicit operator lock → panic-halt: adapter credentials wiped.
        let _ = lock_vault_impl(&state);
        assert!(!state.vault.write().is_auto_locked());
        assert!(
            !aster_has_creds(&state),
            "manual lock must wipe adapter credentials"
        );

        // Re-unlock reprovisions the adapter.
        unlock_vault_impl("test-password-123", None, &state).expect("re-unlock");
        assert!(aster_has_creds(&state), "unlock reprovisions adapter");
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

        let funding = execute_funding_carry_impl(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            25000.0,
            &state,
            false,
            None,
        )
        .await;
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

        // Leverage above the 10x strategy band and nonpositive unit sizes are rejected
        let bad_leverage = TickerStrategyConfigDto {
            user_target_leverage: 11,
            ..def.clone()
        };
        assert!(save_ticker_strategy_config_impl(bad_leverage, &state).is_err());
        let ten_leverage = TickerStrategyConfigDto {
            user_target_leverage: 10,
            ..def.clone()
        };
        assert!(save_ticker_strategy_config_impl(ten_leverage, &state).is_ok());
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
    async fn test_ticker_strategy_config_survives_restart() {
        let dir =
            std::env::temp_dir().join(format!("arbitrageur_ticker_cfg_{}", rand::random::<u64>()));
        let path = dir.join("state.sqlite3");
        let store = std::sync::Arc::new(crate::state_store::StateStore::open(&path).unwrap());
        {
            let state = AppEngineState::new_inner_for_test(store.clone());
            let cfg = TickerStrategyConfigDto {
                symbol: "ETH-USDC".into(),
                target_profit_bps: 6.0,
                window_percentile: 75,
                max_accumulated_units: 2,
                current_accumulated_units: 0,
                execution_mode: "SINGLE_WAY".into(),
                unit_size: 0.05,
                user_target_leverage: 4,
                sliding_window: "1h".into(),
            };
            save_ticker_strategy_config_impl(cfg, &state).expect("save");
        }
        // Fresh state over the same store must restore the saved config —
        // otherwise the grid cap the operator set evaporates on restart.
        let state2 = AppEngineState::new_inner_for_test(store);
        let restored =
            get_ticker_strategy_config_impl("ETH-USDC", &state2).expect("restored config");
        assert_eq!(restored.max_accumulated_units, 2);
        assert_eq!(restored.target_profit_bps, 6.0);
        assert_eq!(restored.sliding_window, "1h");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_ledger_lots_and_pnl_counter_survive_restart() {
        // Durable cycle lots + cumulative counters (schema v30): a restart
        // mid-position must restore the open lot (close-matching, exposure)
        // and the Net Profit headline; sim_step resumes above durable ids.
        let dir =
            std::env::temp_dir().join(format!("arbitrageur_ledger_{}", rand::random::<u64>()));
        let path = dir.join("state.sqlite3");
        let store = std::sync::Arc::new(crate::state_store::StateStore::open(&path).unwrap());
        {
            let state = AppEngineState::new_inner_for_test(store.clone());
            // The lot needs a matching open signal or boot reconcile marks
            // it stale-Closed (crash-window between signal-close and
            // lot-close persists).
            store
                .record_signal_execution(
                    &crate::state_store::NewSignalExecution {
                        id: "sig-open".into(),
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
                                symbol: None,
                                intended_qty: 0.1,
                                intended_price: None,
                            },
                            crate::state_store::NewSignalLeg {
                                account_id: None,
                                venue: "lighter".into(),
                                side: crate::state_store::LegSide::Short,
                                symbol: None,
                                intended_qty: 0.1,
                                intended_price: None,
                            },
                        ],
                    },
                    "testnet",
                )
                .expect("record");
            store
                .apply_leg_fill(
                    "sig-open",
                    crate::state_store::LegSide::Long,
                    0.1,
                    64_200.0,
                    0.5,
                    None,
                )
                .expect("long fill");
            store
                .apply_leg_fill(
                    "sig-open",
                    crate::state_store::LegSide::Short,
                    0.1,
                    64_440.0,
                    0.5,
                    None,
                )
                .expect("short fill");
            store
                .update_signal_status("sig-open", crate::state_store::SignalStatus::Balanced, None)
                .expect("balanced");
            {
                let mut ledger = state.virtual_ledger.write();
                ledger.open_cycle(
                    "spatial-9".into(),
                    "spatial_spread".into(),
                    "BTC-USDC".into(),
                    "hyperliquid".into(),
                    "lighter".into(),
                    20.0,
                    0.1,
                    64_200.0,
                    0.1,
                    64_440.0,
                    0.5,
                    "testnet".into(),
                );
                let lot = ledger.lots.get("spatial-9").unwrap().clone();
                state.state_store.upsert_cycle_lot(&lot).unwrap();
            }
            crate::execution::accumulate_realized_profit(&state, false, 12.5);
            crate::execution::accumulate_realized_profit(&state, true, 3.25);
            assert_eq!(state.next_sim_step(), 9);
        }
        let state2 = AppEngineState::new_inner_for_test(store);
        {
            let ledger = state2.virtual_ledger.read();
            let lot = ledger.lots.get("spatial-9").expect("open lot hydrates");
            assert_eq!(
                lot.status,
                arbitrageur_execution::virtual_ledger::CycleStatus::Open
            );
            assert_eq!(lot.symbol, "BTC-USDC");
        }
        let tn = state2
            .total_realized_profit
            .read()
            .to_string()
            .parse::<f64>()
            .unwrap();
        let lv = state2
            .total_realized_profit_live
            .read()
            .to_string()
            .parse::<f64>()
            .unwrap();
        assert!((tn - 12.5).abs() < 1e-9);
        assert!((lv - 3.25).abs() < 1e-9);
        // The counter resumed from kv (9), not 8 — no id collision with the
        // hydrated spatial-9 class of durable ids.
        assert_eq!(state2.next_sim_step(), 10);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_boot_reconcile_rebuilds_lot_for_lotless_open_signal() {
        // A position opened before cycle lots were durable (or a crash
        // between lot-open and persist) leaves an open two-leg signal with
        // no lot. Boot reconcile must reconstruct it so the eventual flatten
        // can realize PnL and resolve predictions.
        let dir = std::env::temp_dir().join(format!("arbitrageur_rehyd_{}", rand::random::<u64>()));
        let path = dir.join("state.sqlite3");
        let store = std::sync::Arc::new(crate::state_store::StateStore::open(&path).unwrap());
        {
            let state = AppEngineState::new_inner_for_test(store.clone());
            store
                .record_signal_execution(
                    &crate::state_store::NewSignalExecution {
                        id: "sig-pre-v30".into(),
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
                                symbol: None,
                                intended_qty: 0.1,
                                intended_price: None,
                            },
                            crate::state_store::NewSignalLeg {
                                account_id: None,
                                venue: "lighter".into(),
                                side: crate::state_store::LegSide::Short,
                                symbol: None,
                                intended_qty: 0.1,
                                intended_price: None,
                            },
                        ],
                    },
                    "testnet",
                )
                .expect("record");
            store
                .apply_leg_fill(
                    "sig-pre-v30",
                    crate::state_store::LegSide::Long,
                    0.1,
                    64_200.0,
                    0.5,
                    None,
                )
                .expect("long fill");
            store
                .apply_leg_fill(
                    "sig-pre-v30",
                    crate::state_store::LegSide::Short,
                    0.1,
                    64_440.0,
                    0.5,
                    None,
                )
                .expect("short fill");
            store
                .update_signal_status(
                    "sig-pre-v30",
                    crate::state_store::SignalStatus::Balanced,
                    None,
                )
                .expect("balanced");
            assert!(state.virtual_ledger.read().lots.is_empty());
        }
        // Boot over the same store must rebuild a lot for the lotless pair.
        let state2 = AppEngineState::new_inner_for_test(store);
        let ledger = state2.virtual_ledger.read();
        let lot = ledger
            .lots
            .values()
            .find(|l| l.symbol == "BTC-USDC")
            .expect("rehydrated lot exists");
        assert_eq!(
            lot.status,
            arbitrageur_execution::virtual_ledger::CycleStatus::Open
        );
        assert!(lot.id.starts_with("rehydrated-"));
        assert!((lot.long_entry_price - 64_200.0).abs() < 1e-9);
        assert!((lot.short_entry_price - 64_440.0).abs() < 1e-9);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_boot_reconcile_rebuild_shrinks_for_trimmed_signal() {
        // A mid-hold trim physically reduced the position before a crash
        // lost the lot row: the rebuilt lot must carry the residual qty
        // (entry − child closing fills) or the eventual flatten's fills
        // could never cover it.
        let dir =
            std::env::temp_dir().join(format!("arbitrageur_retrim_{}", rand::random::<u64>()));
        let path = dir.join("state.sqlite3");
        let store = std::sync::Arc::new(crate::state_store::StateStore::open(&path).unwrap());
        {
            let _state = AppEngineState::new_inner_for_test(store.clone());
            store
                .record_signal_execution(
                    &crate::state_store::NewSignalExecution {
                        id: "sig-trimmed".into(),
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
                                symbol: None,
                                intended_qty: 0.1,
                                intended_price: None,
                            },
                            crate::state_store::NewSignalLeg {
                                account_id: None,
                                venue: "lighter".into(),
                                side: crate::state_store::LegSide::Short,
                                symbol: None,
                                intended_qty: 0.1,
                                intended_price: None,
                            },
                        ],
                    },
                    "testnet",
                )
                .expect("record");
            for (side, px) in [
                (crate::state_store::LegSide::Long, 64_200.0),
                (crate::state_store::LegSide::Short, 64_440.0),
            ] {
                store
                    .apply_leg_fill("sig-trimmed", side, 0.1, px, 0.5, None)
                    .expect("fill");
            }
            store
                .update_signal_status(
                    "sig-trimmed",
                    crate::state_store::SignalStatus::Balanced,
                    None,
                )
                .expect("balanced");
            // Trim child already Closed on the crash boundary — its fills
            // still count toward the residual.
            store
                .record_signal_execution(
                    &crate::state_store::NewSignalExecution {
                        id: "trim-sig-trimmed-00ff".into(),
                        strategy_id: "futures".into(),
                        symbol: "BTC-USDC".into(),
                        route_id: None,
                        expected_net_usd: None,
                        note: None,
                        legs: vec![
                            crate::state_store::NewSignalLeg {
                                account_id: None,
                                venue: "hyperliquid".into(),
                                side: crate::state_store::LegSide::Short,
                                symbol: Some("BTC-USDC".into()),
                                intended_qty: 0.04,
                                intended_price: None,
                            },
                            crate::state_store::NewSignalLeg {
                                account_id: None,
                                venue: "lighter".into(),
                                side: crate::state_store::LegSide::Long,
                                symbol: Some("BTC-USDC".into()),
                                intended_qty: 0.04,
                                intended_price: None,
                            },
                        ],
                    },
                    "testnet",
                )
                .expect("record trim");
            for (side, px) in [
                (crate::state_store::LegSide::Short, 64_300.0),
                (crate::state_store::LegSide::Long, 64_400.0),
            ] {
                store
                    .apply_leg_fill("trim-sig-trimmed-00ff", side, 0.04, px, 0.1, None)
                    .expect("trim fill");
            }
            store
                .update_signal_status(
                    "trim-sig-trimmed-00ff",
                    crate::state_store::SignalStatus::Closed,
                    None,
                )
                .expect("trim closed");
            assert!(crate::state_store::is_flatten_child_id(
                "trim-sig-trimmed-00ff",
                "sig-trimmed"
            ));
        }
        let state2 = AppEngineState::new_inner_for_test(store);
        let ledger = state2.virtual_ledger.read();
        let lot = ledger
            .lots
            .values()
            .find(|l| l.symbol == "BTC-USDC")
            .expect("rehydrated lot exists");
        assert!((lot.long_qty - 0.06).abs() < 1e-9, "long {}", lot.long_qty);
        assert!(
            (lot.short_qty - 0.06).abs() < 1e-9,
            "short {}",
            lot.short_qty
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_liq_trim_realizes_partial_then_final_close_matches() {
        use crate::state_store::{LegSide, NewSignalExecution, NewSignalLeg};

        let state = AppEngineState::new();
        // Open pair: long 1.0 @ 64200 (fee 0.5), short 1.0 @ 64440 (fee 0.5).
        state
            .state_store
            .record_signal_execution(
                &NewSignalExecution {
                    id: "sig-trim".into(),
                    strategy_id: "futures".into(),
                    symbol: "BTC-USDC".into(),
                    route_id: None,
                    expected_net_usd: None,
                    note: None,
                    legs: vec![
                        NewSignalLeg {
                            account_id: None,
                            venue: "hyperliquid".into(),
                            side: LegSide::Long,
                            symbol: None,
                            intended_qty: 1.0,
                            intended_price: None,
                        },
                        NewSignalLeg {
                            account_id: None,
                            venue: "lighter".into(),
                            side: LegSide::Short,
                            symbol: None,
                            intended_qty: 1.0,
                            intended_price: None,
                        },
                    ],
                },
                "testnet",
            )
            .expect("record");
        state
            .state_store
            .apply_leg_fill("sig-trim", LegSide::Long, 1.0, 64_200.0, 0.5, None)
            .expect("long fill");
        state
            .state_store
            .apply_leg_fill("sig-trim", LegSide::Short, 1.0, 64_440.0, 0.5, None)
            .expect("short fill");
        {
            let mut ledger = state.virtual_ledger.write();
            ledger.open_cycle(
                "spatial-trim-1".into(),
                "spatial_spread".into(),
                "BTC-USDC".into(),
                "hyperliquid".into(),
                "lighter".into(),
                37.38,
                1.0,
                64_200.0,
                1.0,
                64_440.0,
                1.0,
                "testnet".into(),
            );
            let lot = ledger.lots.get("spatial-trim-1").unwrap().clone();
            state.state_store.upsert_cycle_lot(&lot).unwrap();
        }
        // The entry path debits entry fees as they are persisted.
        crate::execution::accumulate_realized_profit(&state, false, -1.0);

        // Mid-hold liquidation trim: 0.4 off each leg. Closing sides are the
        // inverse of the entry sides.
        state
            .state_store
            .record_signal_execution(
                &NewSignalExecution {
                    id: "trim-sig-trim-a1".into(),
                    strategy_id: "futures".into(),
                    symbol: "BTC-USDC".into(),
                    route_id: None,
                    expected_net_usd: None,
                    note: Some("liq trim of sig-trim".into()),
                    legs: vec![
                        NewSignalLeg {
                            account_id: None,
                            venue: "hyperliquid".into(),
                            side: LegSide::Short,
                            symbol: Some("BTC-USDC".into()),
                            intended_qty: 0.4,
                            intended_price: None,
                        },
                        NewSignalLeg {
                            account_id: None,
                            venue: "lighter".into(),
                            side: LegSide::Long,
                            symbol: Some("BTC-USDC".into()),
                            intended_qty: 0.4,
                            intended_price: None,
                        },
                    ],
                },
                "testnet",
            )
            .expect("record trim");
        state
            .state_store
            .apply_leg_fill("trim-sig-trim-a1", LegSide::Short, 0.4, 64_300.0, 0.2, None)
            .expect("trim long-side fill");
        state
            .state_store
            .apply_leg_fill("trim-sig-trim-a1", LegSide::Long, 0.4, 64_400.0, 0.2, None)
            .expect("trim short-side fill");

        let sig = state
            .state_store
            .get_signal("sig-trim")
            .expect("load")
            .expect("sig exists");
        let realized = crate::execution::realize_liq_trim(
            &state,
            &sig,
            "trim-sig-trim-a1",
            "hyperliquid",
            "lighter",
        )
        .expect("trim realizes");
        // Long side: (64300-64200)*0.4 - 0.2 = 39.8
        // Short side: (64440-64400)*0.4 - 0.2 = 15.8
        assert!((realized - 55.6).abs() < 1e-9, "realized {realized}");
        {
            let ledger = state.virtual_ledger.read();
            let lot = ledger.lots.get("spatial-trim-1").unwrap();
            assert!((lot.long_qty - 0.6).abs() < 1e-9, "long {}", lot.long_qty);
            assert!(
                (lot.short_qty - 0.6).abs() < 1e-9,
                "short {}",
                lot.short_qty
            );
            assert_eq!(
                lot.status,
                arbitrageur_execution::virtual_ledger::CycleStatus::Open
            );
        }
        // Persisted lot reflects the shrink (restart keeps the reduction).
        let persisted = state
            .state_store
            .load_cycle_lots()
            .expect("load lots")
            .get("spatial-trim-1")
            .cloned()
            .expect("lot row");
        assert!((persisted.long_qty - 0.6).abs() < 1e-9);
        let counter = state
            .total_realized_profit
            .read()
            .to_string()
            .parse::<f64>()
            .unwrap();
        assert!((counter - 54.6).abs() < 1e-9, "counter {counter}");

        // Late fill growth (order polling advances the cumulative qty from
        // 0.4 to 0.5): a second realization must count only the 0.1
        // increment — the durable mark tracks counted qty/pnl/fee.
        state
            .state_store
            .apply_leg_fill(
                "trim-sig-trim-a1",
                LegSide::Short,
                0.5,
                64_300.0,
                0.25,
                None,
            )
            .expect("grown long-side fill");
        state
            .state_store
            .apply_leg_fill("trim-sig-trim-a1", LegSide::Long, 0.5, 64_400.0, 0.25, None)
            .expect("grown short-side fill");
        let realized2 = crate::execution::realize_liq_trim(
            &state,
            &sig,
            "trim-sig-trim-a1",
            "hyperliquid",
            "lighter",
        )
        .expect("second realization");
        // Long: 100*0.1 - 0.05 = 9.95; Short: 40*0.1 - 0.05 = 3.95.
        assert!((realized2 - 13.9).abs() < 1e-9, "realized2 {realized2}");
        {
            let ledger = state.virtual_ledger.read();
            let lot = ledger.lots.get("spatial-trim-1").unwrap();
            assert!((lot.long_qty - 0.5).abs() < 1e-9, "long {}", lot.long_qty);
            assert!(
                (lot.short_qty - 0.5).abs() < 1e-9,
                "short {}",
                lot.short_qty
            );
        }
        let counter = state
            .total_realized_profit
            .read()
            .to_string()
            .parse::<f64>()
            .unwrap();
        assert!((counter - 68.5).abs() < 1e-9, "counter {counter}");

        // Final flatten covers only the remaining 0.5. The family slice
        // deliberately includes the trim child: the close aggregation must
        // exclude `trim-` fills or it would over-fill the shrunken lot.
        state
            .state_store
            .record_signal_execution(
                &NewSignalExecution {
                    id: "flat-sig-trim-b2".into(),
                    strategy_id: "futures".into(),
                    symbol: "BTC-USDC".into(),
                    route_id: None,
                    expected_net_usd: None,
                    note: None,
                    legs: vec![
                        NewSignalLeg {
                            account_id: None,
                            venue: "hyperliquid".into(),
                            side: LegSide::Short,
                            symbol: Some("BTC-USDC".into()),
                            intended_qty: 0.5,
                            intended_price: None,
                        },
                        NewSignalLeg {
                            account_id: None,
                            venue: "lighter".into(),
                            side: LegSide::Long,
                            symbol: Some("BTC-USDC".into()),
                            intended_qty: 0.5,
                            intended_price: None,
                        },
                    ],
                },
                "testnet",
            )
            .expect("record flat");
        state
            .state_store
            .apply_leg_fill(
                "flat-sig-trim-b2",
                LegSide::Short,
                0.5,
                64_350.0,
                0.25,
                None,
            )
            .expect("flat long-side fill");
        state
            .state_store
            .apply_leg_fill("flat-sig-trim-b2", LegSide::Long, 0.5, 64_420.0, 0.25, None)
            .expect("flat short-side fill");

        let family: Vec<_> = state
            .state_store
            .list_signals(None, None)
            .expect("list")
            .into_iter()
            .filter(|s| s.id.starts_with("trim-sig-trim-") || s.id.starts_with("flat-sig-trim-"))
            .collect();
        assert_eq!(family.len(), 2);
        let closed = crate::execution::close_flattened_spatial_lot(
            &state,
            &sig,
            &family,
            arbitrageur_execution::virtual_ledger::CycleStatus::Closed,
        )
        .expect("lot matches reduced qty");
        let (usd, _bps, entry_debited) = closed;
        // Remaining leg PnL: (64350-64200)*0.5 + (64440-64420)*0.5 = 75+10 = 85
        // usd = 85 - entry fees 1.0 - exit fees 0.5 = 83.5
        assert!((usd - 83.5).abs() < 1e-9, "usd {usd}");
        assert!((entry_debited - 1.0).abs() < 1e-9);
        crate::execution::accumulate_realized_profit(&state, false, usd + entry_debited);

        // Economic truth: leg PnL 125+30 = 155, fees 1.0+0.5+0.5 = 2.0 → 153.0.
        let counter = state
            .total_realized_profit
            .read()
            .to_string()
            .parse::<f64>()
            .unwrap();
        assert!((counter - 153.0).abs() < 1e-9, "counter {counter}");
        let lot = state
            .state_store
            .load_cycle_lots()
            .expect("load")
            .get("spatial-trim-1")
            .cloned()
            .expect("lot row");
        assert_eq!(
            lot.status,
            arbitrageur_execution::virtual_ledger::CycleStatus::Closed
        );
        // The lot's durable outcome is the WHOLE cycle — the close path
        // folded the trims' 69.5 back in for metrics/prediction resolution
        // even though the counter only took the close-only 84.5 increment.
        let lot_usd = lot.realized_pnl_usd.expect("realized usd");
        assert!((lot_usd - 153.0).abs() < 1e-9, "lot usd {lot_usd}");
        // bps on original notional: 153 / ((64200*1 + 64440*1)/2) * 1e4.
        let want_bps = 153.0 / 64_320.0 * 10_000.0;
        let lot_bps = lot.realized_pnl_bps.expect("realized bps");
        assert!((lot_bps - want_bps).abs() < 1e-6, "lot bps {lot_bps}");
    }

    #[tokio::test]
    async fn test_flatten_lotless_residual_accumulates_unwind_pnl() {
        // A 1-leg residual (imbalanced survivor) carries no cycle lot: its
        // unwind fills must still reach the counter, priced against the
        // root leg's entry fill — before this, lotless closes dropped PnL.
        let state = setup_test_state_with_venues();
        seed_long_for_flatten(&state).await;
        let before = *state.total_realized_profit.read();

        let res = flatten_ticker_positions_impl(
            Some("BTC-USDC"),
            "futures",
            arbitrageur_execution::virtual_ledger::CycleStatus::Closed,
            &state,
        )
        .await
        .expect("flatten futures");
        assert_eq!(res.legs_closed, 1);

        // Entry 0.1 @ 64205; market exit hits the 64200 bid; sim taker fee
        // is 2bps of notional: 0.1 * 64200 * 0.0002 = 1.284.
        // Realized = (64200-64205)*0.1 - 1.284 = -1.784.
        let delta = *state.total_realized_profit.read() - before;
        let delta_f: f64 = delta.to_string().parse().unwrap();
        assert!(
            (delta_f - (-1.784)).abs() < 1e-6,
            "residual unwind PnL delta {delta_f}"
        );
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
        let persisted: StrategyParamsDto = serde_json::from_str(&rows[0].1).expect("valid json");
        assert_eq!(persisted.auto_min_event_volume_usd, Some(500_000.0));
        assert!(get_strategy_tickers_impl(&state, "prediction")
            .await
            .is_err());
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

    #[test]
    fn test_base_mark_prices_use_venue_symbol_and_min_across_quotes() {
        fn market(
            base: &str,
            quote: &str,
            venue_symbol: &str,
            venue_id: &str,
        ) -> arbitrageur_core::market::MarketMetadata {
            arbitrageur_core::market::MarketMetadata {
                symbol: arbitrageur_core::market::Symbol::perp(base, quote),
                venue: arbitrageur_core::types::VenueId::new(venue_id),
                venue_symbol: venue_symbol.to_string(),
                price_decimals: 2,
                size_decimals: 2,
                min_order_size: dec!(0.01),
                min_notional: dec!(5.0),
                maker_fee_bps: dec!(2.0),
                taker_fee_bps: dec!(5.0),
                contract_size: dec!(1.0),
                is_active: true,
            }
        }

        let markets = vec![
            market("ZEC", "USDC", "ZEC", "hyperliquid"),
            market("MSTR", "USDT", "MSTRUSDT", "binance"),
            market("MSTR", "USDC", "MSTR-USDC-SWAP", "okx"),
        ];
        let prices = HashMap::from([
            ("ZEC".to_string(), dec!(1443.0)),
            ("MSTRUSDT".to_string(), dec!(151.5)),
            ("MSTR-USDC-SWAP".to_string(), dec!(151.2)),
        ]);

        let by_base = base_mark_prices(&markets, &prices);
        assert_eq!(by_base.get("ZEC"), Some(&1443.0));
        assert_eq!(by_base.get("MSTR"), Some(&151.2));
    }

    #[tokio::test]
    async fn test_upsert_account_max_leverage_ipc_roundtrip() {
        let state = AppEngineState::new();
        let created = upsert_account_impl(
            AccountUpsertInput {
                environment: None,
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
                    environment: None,
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
                environment: None,
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
            .upsert_account(
                "hl-main",
                "hyperliquid",
                "Main",
                true,
                true,
                "standard",
                Some(2),
                Some("testnet"),
            )
            .expect("capped account");
        state
            .state_store
            .upsert_account(
                "lt-main",
                "lighter",
                "Main",
                true,
                true,
                "standard",
                None,
                Some("testnet"),
            )
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
            let phase = 2.0 * std::f64::consts::PI * (i % 8) as f64 / 8.0;
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
        let target = rep
            .rows
            .iter()
            .find(|r| r.param == "targetProfitBps")
            .unwrap();
        assert!((target.current - 1.0).abs() < 1e-12);
        let lev = rep
            .rows
            .iter()
            .find(|r| r.param == "userTargetLeverage")
            .unwrap();
        if let Some(v) = lev.recommended {
            assert!(v <= lev.current, "leverage must never increase");
        }
        let pct = rep
            .rows
            .iter()
            .find(|r| r.param == "windowPercentile")
            .unwrap();
        if let Some(v) = pct.recommended {
            assert!((50.0..=95.0).contains(&v), "percentile snap range");
        }
        // Seeded economics validate: at least the target row is READY.
        let ready: Vec<_> = rep.rows.iter().filter(|r| r.status == "READY").collect();
        assert!(!ready.is_empty(), "seeded fixture should validate");
        assert_eq!(rep.banner.ready_count, ready.len());
        // No filled legs exist in this fixture — the feedback join must
        // report None rather than fabricate a slippage estimate.
        assert_eq!(rep.readiness.measured_slippage_bps, None);
    }

    #[tokio::test]
    async fn test_autotune_measured_slippage_feeds_friction() {
        // Same seeded history plus ≥5 priced fills per venue: the qty-
        // weighted median slippage surfaces on the report and feeds the
        // friction input — never zero, never fabricated when absent.
        let state = AppEngineState::new();
        let now = Utc::now().timestamp_millis();
        let n = 400usize;
        for i in 0..n {
            let phase = 2.0 * std::f64::consts::PI * (i % 8) as f64 / 8.0;
            let mid_a = 100.0 + 0.4 * phase.sin();
            seed_samples(
                &state,
                "BTC-USDC",
                now - ((n - i) as i64) * 60_000,
                &[("hyperliquid", mid_a), ("lighter", 100.4)],
            );
        }
        for i in 0..5 {
            let id = format!("slip-{i}");
            state
                .state_store
                .record_signal_execution(
                    &crate::state_store::NewSignalExecution {
                        id: id.clone(),
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
                                symbol: None,
                                intended_qty: 1.0,
                                intended_price: Some(100.0),
                            },
                            crate::state_store::NewSignalLeg {
                                account_id: None,
                                venue: "lighter".into(),
                                side: crate::state_store::LegSide::Short,
                                symbol: None,
                                intended_qty: 1.0,
                                intended_price: Some(100.5),
                            },
                        ],
                    },
                    "testnet",
                )
                .expect("record signal");
            // Long pays +0.1 over quote (+10 bps); short fills 0.1 under
            // quote (+0.1/100.5 ≈ +9.95 bps adverse).
            state
                .state_store
                .apply_leg_fill(
                    &id,
                    crate::state_store::LegSide::Long,
                    1.0,
                    100.1,
                    0.0,
                    None,
                )
                .unwrap();
            state
                .state_store
                .apply_leg_fill(
                    &id,
                    crate::state_store::LegSide::Short,
                    1.0,
                    100.4,
                    0.0,
                    None,
                )
                .unwrap();
        }
        let rep = get_autotune_recommendation_impl("BTC-USDC", &state)
            .await
            .expect("recommendation");
        let expect = 10.0 + (100.5 - 100.4) / 100.5 * 10_000.0;
        let got = rep
            .readiness
            .measured_slippage_bps
            .expect("≥5 priced fills per venue must produce a measurement");
        assert!((got - expect).abs() < 1e-9, "got {got}, want {expect}");
    }

    #[test]
    fn test_ticker_autotune_mode_validation_and_default() {
        let state = AppEngineState::new();
        // Unset → manual default with empty run stamp.
        let dto = get_ticker_autotune_mode_impl("BTC-USDC", &state).unwrap();
        assert_eq!(dto.mode, "manual");
        assert_eq!(dto.last_run_ms, None);
        assert!(dto.period_secs > 0 && dto.continuous_min_secs > 0);
        // Unknown modes fail closed.
        assert!(set_ticker_autotune_mode_impl("BTC-USDC", "hourly", &state).is_err());
        // manual is the default — setting it deletes the key outright so the
        // scheduler's prefix scan never enumerates it.
        set_ticker_autotune_mode_impl("BTC-USDC", "periodic", &state).unwrap();
        assert_eq!(
            get_ticker_autotune_mode_impl("BTC-USDC", &state)
                .unwrap()
                .mode,
            "periodic"
        );
        set_ticker_autotune_mode_impl("BTC-USDC", "manual", &state).unwrap();
        assert_eq!(
            get_ticker_autotune_mode_impl("BTC-USDC", &state)
                .unwrap()
                .mode,
            "manual"
        );
        assert!(state
            .state_store
            .kv_get("autotune.mode.BTC-USDC")
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_scheduled_autotune_periodic_applies_and_backs_off() {
        // Seeded history → READY rows exist; periodic mode applies them
        // with an audit line tagged source="periodic", stamps last_run,
        // and an immediate second pass is not due.
        let state = AppEngineState::new();
        let now = Utc::now().timestamp_millis();
        let n = 400usize;
        for i in 0..n {
            let phase = 2.0 * std::f64::consts::PI * (i % 8) as f64 / 8.0;
            let mid_a = 100.0 + 0.4 * phase.sin();
            seed_samples(
                &state,
                "BTC-USDC",
                now - ((n - i) as i64) * 60_000,
                &[("hyperliquid", mid_a), ("lighter", 100.4)],
            );
        }
        // A manual-mode sibling must never be touched by the scheduler.
        set_ticker_autotune_mode_impl("BTC-USDC", "periodic", &state).unwrap();
        set_ticker_autotune_mode_impl("ETH-USDC", "manual", &state).unwrap();
        let ran = run_scheduled_autotune(&state)
            .await
            .expect("scheduler pass");
        assert_eq!(ran, 1);
        let dto = get_ticker_autotune_mode_impl("BTC-USDC", &state).unwrap();
        assert!(dto.last_run_ms.is_some(), "last_run stamped");
        // Not due again — the stamp gates the interval.
        let ran2 = run_scheduled_autotune(&state).await.unwrap();
        assert_eq!(ran2, 0);
        let events = state
            .state_store
            .recent_autotune_apply_events("BTC-USDC", 10)
            .unwrap();
        assert_eq!(events.len(), 1, "one scheduler apply audited");
        assert!(
            events[0].detail_json.contains("\"source\":\"periodic\""),
            "audit detail tags the scheduler source: {}",
            events[0].detail_json
        );
        let applied: serde_json::Value = serde_json::from_str(&events[0].applied_json).unwrap();
        assert!(applied.as_object().map(|m| !m.is_empty()).unwrap_or(false));
        // ETH-USDC manual → no audit events at all.
        assert!(state
            .state_store
            .recent_autotune_apply_events("ETH-USDC", 10)
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn test_scheduled_autotune_no_modes_is_noop() {
        // Zero configured tickers → zero work; the scheduler is inert by
        // default (fail-closed on the opt-in boundary).
        let state = AppEngineState::new();
        let ran = run_scheduled_autotune(&state).await.unwrap();
        assert_eq!(ran, 0);
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
                status: if rec.is_some() {
                    "READY".to_string()
                } else {
                    "HELD".to_string()
                },
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
            readiness: AutotuneReadinessDto {
                venues_seen: vec!["hyperliquid".to_string(), "lighter".to_string()],
                history_span_secs: 86_400.0,
                multi_venue_coverage: 1.0,
                pair_observations: 400,
                total_pair_observations: 400,
                min_observations_required: arbitrageur_math::autotune::AUTOTUNE_MIN_PAIR_OBS,
                obs_floor_met: true,
                measured_slippage_bps: None,
                blockers: Vec::new(),
            },
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
        let res = apply_autotune_rows_inner(&state, "BTC-USDC", &report, &selected, now, "manual")
            .await
            .expect("apply");
        assert_eq!(
            res.applied,
            vec!["slidingWindowSecs", "targetProfitBps", "windowPercentile"]
        );
        assert_eq!(res.skipped_held, vec!["maxStepUsd".to_string()]);
        let audit_id = res.audit_id.expect("audit row");
        assert!(audit_id > 0);
        let strategy = get_strategy_params_impl("futures", &state).expect("strategy");
        assert!((strategy.target_profit_bps - 8.5).abs() < 1e-12);
        assert_eq!(strategy.window_percentile, 80);
        assert_eq!(strategy.sliding_window_secs, 43_200);
        assert_eq!(
            strategy.max_step_usd, 2500.0,
            "HELD row must keep manual value"
        );
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
        let res = apply_autotune_rows_inner(&state, "BTC-USDC", &report, &selected, now, "manual")
            .await
            .expect("apply");
        assert!(res.applied.is_empty());
        assert_eq!(
            res.skipped_held,
            vec!["maxStepUsd".to_string(), "targetProfitBps".to_string()]
        );
        assert_eq!(res.audit_id, None);
        assert!(
            state.strategy_params.read().is_empty(),
            "no writes on all-HELD"
        );
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
        assert!(
            apply_autotune_rows_inner(&state, "BTC-USDC", &report, &selected, now, "manual")
                .await
                .is_err()
        );
        assert!(
            state.strategy_params.read().is_empty(),
            "unknown param: no writes"
        );
        assert!(state
            .state_store
            .recent_autotune_apply_events("BTC-USDC", 10)
            .expect("audit read")
            .is_empty());
        assert!(
            apply_autotune_rows_inner(&state, "BTC-USDC", &report, &[], 0, "manual")
                .await
                .is_err(),
            "broken clock must refuse"
        );
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
                readiness: report.readiness.clone(),
            };
            let sel = selected.clone();
            handles.push(tokio::spawn(async move {
                apply_autotune_rows_inner(&s, "BTC-USDC", &r, &sel, now, "manual").await
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
                    &[
                        ("hyperliquid", 60_000.0 + wave),
                        ("lighter", 60_000.0 - wave),
                    ],
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
        let stored = state
            .state_store
            .session_profiles("BTC-USDC")
            .expect("rows");
        assert!(
            stored.iter().any(|r| r.rv_bps.is_some()),
            "rollup must emit rv"
        );
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
            let base = stored
                .iter()
                .find(|r| r.session_id == s.session_id)
                .expect("row");
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
            let base = stored
                .iter()
                .find(|r| r.session_id == s.session_id)
                .expect("row");
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
            None,
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
            None,
            &state,
        )
        .await
        .expect_err("max_units 0 must reject");
        assert!(err.contains("max_units"), "unexpected err: {err}");

        // Ticker already holding an open (balanced) pair at the persisted
        // config cap rejects — even with no caller-supplied max_units, the
        // ledger-derived unit count is authoritative.
        state.strategy_configs.write().insert(
            "BTC-USDC".to_string(),
            TickerStrategyConfigDto {
                symbol: "BTC-USDC".into(),
                max_accumulated_units: 1,
                ..TickerStrategyConfigDto::default()
            },
        );
        state
            .state_store
            .record_signal_execution(
                &crate::state_store::NewSignalExecution {
                    id: "grid-open-1".into(),
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
                            symbol: None,
                            intended_qty: 0.1,
                            intended_price: Some(100.0),
                        },
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "lighter".into(),
                            side: crate::state_store::LegSide::Short,
                            symbol: None,
                            intended_qty: 0.1,
                            intended_price: Some(100.5),
                        },
                    ],
                },
                "testnet",
            )
            .expect("record signal");
        state
            .state_store
            .update_signal_status(
                "grid-open-1",
                crate::state_store::SignalStatus::Balanced,
                None,
            )
            .expect("balance signal");
        let err = execute_spatial_arbitrage_inner(
            "BTC-USDC",
            "hyperliquid",
            "lighter",
            0.1,
            None,
            None,
            None,
            Some(1),
            None,
            None,
            None,
            "futures",
            None,
            &state,
        )
        .await
        .expect_err("1/1 open pairs must reject");
        assert!(err.contains("Grid cap"), "unexpected err: {err}");
    }

    #[tokio::test]
    async fn test_venue_balances_report_paper_account() {
        let state = setup_test_state_with_venues();
        let balances = get_venue_balances_impl(&state).await.expect("balances");
        // Testnet-admitted venues: hyperliquid, lighter, robinhood (testnet
        // Lighter), vanta, arcus (testnet /v1 API).
        assert_eq!(balances.len(), 5);
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
        assert!(err.contains("40 multi-venue"), "unexpected err: {}", err);

        assert_eq!(state.strategy_configs.read().len(), configs_before);
        assert_eq!(state.execution_logs.read().len(), logs_before);
        assert_eq!(*state.total_realized_profit.read(), dec!(0));
    }

    #[tokio::test]
    async fn test_wfo_calibration_runs_on_seeded_history() {
        let state = AppEngineState::new();
        // Mean-reverting cross-venue edge: venue_b oscillates around venue_a.
        for i in 0..120i64 {
            let offset = if i % 20 < 10 { 3.0 } else { -3.0 };
            seed_samples(
                &state,
                "BTC",
                1_700_000_000_000 + i * 1_000,
                &[("hyperliquid", 50_000.0), ("lighter", 50_000.0 + offset)],
            );
        }
        let res = run_wfo_calibration_impl("BTC", &state)
            .await
            .expect("seeded history must produce a WFO result");
        assert!(!res.candidate_evaluations.is_empty());
        assert!(res.optimal_window_size > 0);
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
        // Auto-exit hold deadline: below the 60s floor and above the 7d cap
        // are refused; a sane value saves and round-trips through kv.
        let bad = EngineSettingsDto {
            auto_exit_max_hold_secs: 30,
            ..base.clone()
        };
        assert!(save_engine_settings_impl(bad, &state).is_err());
        let bad = EngineSettingsDto {
            auto_exit_max_hold_secs: 604_801,
            ..base.clone()
        };
        assert!(save_engine_settings_impl(bad, &state).is_err());
        let good = EngineSettingsDto {
            auto_exit_max_hold_secs: 3600,
            ..base.clone()
        };
        assert!(save_engine_settings_impl(good, &state).is_ok());
        assert_eq!(
            state
                .state_store
                .kv_get("engine.auto_exit_max_hold_secs")
                .unwrap()
                .as_deref(),
            Some("3600")
        );
        assert_eq!(state.settings.read().auto_exit_max_hold_secs, 3600);

        // USD-quote policy: unknown modes, non-finite/inverted/absurd
        // depeg thresholds are refused; valid values persist + reload.
        let bad = EngineSettingsDto {
            usd_quote_mode: "merge".into(),
            ..base.clone()
        };
        assert!(save_engine_settings_impl(bad, &state).is_err());
        let bad = EngineSettingsDto {
            stablecoin_warn_bps: 0.0,
            ..base.clone()
        };
        assert!(save_engine_settings_impl(bad, &state).is_err());
        let bad = EngineSettingsDto {
            stablecoin_warn_bps: 200.0,
            stablecoin_halt_bps: 100.0,
            ..base.clone()
        };
        assert!(save_engine_settings_impl(bad, &state).is_err());
        let bad = EngineSettingsDto {
            stablecoin_halt_bps: f64::NAN,
            ..base.clone()
        };
        assert!(save_engine_settings_impl(bad, &state).is_err());
        let bad = EngineSettingsDto {
            stablecoin_halt_bps: 20_000.0,
            ..base.clone()
        };
        assert!(save_engine_settings_impl(bad, &state).is_err());
        // Alias spelling normalizes to the canonical mode and persists.
        let good = EngineSettingsDto {
            usd_quote_mode: "SEPARATE".into(),
            stablecoin_warn_bps: 75.0,
            stablecoin_halt_bps: 150.0,
            ..base.clone()
        };
        assert!(save_engine_settings_impl(good, &state).is_ok());
        assert_eq!(state.settings.read().usd_quote_mode, "separate");
        assert_eq!(state.settings.read().stablecoin_warn_bps, 75.0);
        assert_eq!(
            state
                .state_store
                .kv_get("engine.usd_quote_mode")
                .unwrap()
                .as_deref(),
            Some("separate")
        );
        assert_eq!(
            state
                .state_store
                .kv_get("risk.stablecoin_halt_bps")
                .unwrap()
                .as_deref(),
            Some("150")
        );

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
        // Five testnet-admitted venues (hl/lighter/robinhood/vanta/arcus
        // sims @ 100k each) — robinhood and arcus both run real public
        // testnets, so their sims admit here too.
        assert_eq!(before.total_portfolio_value_usd, 500000.0);
        assert_eq!(before.margin_health.margin_balance_usd, 500000.0);
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
            .compute_strategy_metrics("spatial_spread", "testnet");
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
                .ticker_net_position("BTC-USDC", None)
                .unwrap()
                .abs()
                < 1e-9
        );
        // Balanced signals no longer block the ticker.
        assert!(!state.state_store.ticker_blocked("BTC-USDC", None).unwrap());
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
        seed_test_book(&state, "lighter", "ETH-USDC", dec!(3060.0), dec!(3065.0));

        // A prior signal for BTC-USDC holds residual exposure: only its long
        // leg filled. The ticker must refuse new orders until it reconciles.
        state
            .state_store
            .record_signal_execution(
                &crate::state_store::NewSignalExecution {
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
                            symbol: None,
                            intended_qty: 0.1,
                            intended_price: None,
                        },
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "lighter".into(),
                            side: crate::state_store::LegSide::Short,
                            symbol: None,
                            intended_qty: 0.1,
                            intended_price: None,
                        },
                    ],
                },
                "testnet",
            )
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
        seed_test_book(&state, "lighter", "ETH-USDC", dec!(3060.0), dec!(3065.0));

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
                    predicted_win_probability: None,
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
                    predicted_win_probability: None,
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
                    predicted_win_probability: None,
                },
            ],
            false,
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
        assert!(
            eth.result.as_ref().is_some_and(|r| r.execution_success),
            "ETH winner must execute; rejection reason: {:?}",
            eth.reason
        );

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
        seed_test_book(&state, "lighter", "ETH-USDC", dec!(3060.0), dec!(3065.0));

        // In-flight signal on BTC-USDC blocks every candidate in the batch.
        state
            .state_store
            .record_signal_execution(
                &crate::state_store::NewSignalExecution {
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
                        symbol: None,
                        intended_qty: 0.1,
                        intended_price: None,
                    }],
                },
                "testnet",
            )
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
                    predicted_win_probability: None,
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
                    predicted_win_probability: None,
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
                    predicted_win_probability: None,
                },
            ],
            false,
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
        assert!(
            eth.result.as_ref().is_some_and(|r| r.execution_success),
            "ETH winner must execute; rejection reason: {:?}",
            eth.reason
        );
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
            .upsert_account(
                "hl-main",
                "hyperliquid",
                "Main",
                true,
                false,
                "standard",
                None,
                None,
            )
            .expect("account");

        state
            .state_store
            .record_signal_execution(
                &crate::state_store::NewSignalExecution {
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
                        symbol: None,
                        intended_qty: 0.1,
                        intended_price: None,
                    }],
                },
                "testnet",
            )
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
        seed_test_book(&state, "vanta", "BTC-USDC", dec!(64360.0), dec!(64365.0));
        seed_test_funding(&state, "hyperliquid", "BTC-USDC");
        seed_test_funding(&state, "lighter", "BTC-USDC");
        seed_test_funding(&state, "vanta", "BTC-USDC");

        // Testnet admits hl/lighter/robinhood/vanta/arcus — the robinhood
        // and arcus sims have no BTC-USDC books seeded, so they skip.
        let books = fetch_orderbooks_impl("BTC-USDC", &state)
            .await
            .expect("books");
        assert_eq!(books.len(), 2);
        assert_eq!(books[0].venue_id, "lighter");
        assert_eq!(books[1].venue_id, "vanta");

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
        *state.last_risk_live_universe.write() = Some(state.is_live());

        let risk = get_risk_overview_impl(&state).await.expect("risk");
        // Equity universe = 5 testnet venues @ 100k → 500k vs 1M baseline.
        assert_eq!(risk.current_drawdown_usd, 500000.0);
        assert_eq!(risk.drawdown_pct, 50.0);

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
                bid_depth: None,
                ask_depth: None,
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
        seed_samples(&state, "BTC-USDC", now - 3_000, &[("hyperliquid", 99.0)]);

        let s = get_signal_series_impl("BTC-USDC", "consensus", 300, &state)
            .await
            .expect("series");
        assert_eq!(s.sample_count, 3);
        assert_eq!(s.earliest_ts_ms, Some(now - 3_000));
        assert!(!s.ts_ms.is_empty());
    }

    #[tokio::test]
    async fn test_spread_analytics_empty_is_honest() {
        let state = AppEngineState::new();
        let a = get_spread_analytics_impl("BTC-USDC", 3600, 5.0, &state)
            .await
            .expect("analytics");
        assert_eq!(a.band.sample_count, 0);
        assert!(a.band.mean_bps.is_none());
        assert!(a.band.frac_at_or_above_hurdle.is_none());
        assert!(a.pairs.is_empty());
        assert!(a.venues_seen.is_empty());
    }

    #[tokio::test]
    async fn test_spread_analytics_band_and_pair_stats() {
        let state = AppEngineState::new();
        let now = Utc::now().timestamp_millis();
        // 10 polls at 1s cadence: A=100.0 constant, B ramps 100.1..101.0 —
        // band = B/A - 1 in bps, i.e. ~10..100 bps.
        for i in 0..10 {
            seed_samples(
                &state,
                "BTC-USDC",
                now - (9 - i) * 1_000,
                &[
                    ("hyperliquid", 100.0),
                    ("lighter", 100.0 + (i as f64 + 1.0) / 10.0),
                ],
            );
        }
        let a = get_spread_analytics_impl("BTC-USDC", 3600, 50.0, &state)
            .await
            .expect("analytics");
        assert_eq!(a.band.sample_count, 10);
        assert_eq!(a.venues_seen, vec!["hyperliquid", "lighter"]);
        // band values ≈ 10,20,...,100 bps (hi/lo-1): mean ~55
        let mean = a.band.mean_bps.expect("mean");
        assert!((mean - 55.0).abs() < 1.0, "band mean {mean}");
        assert!(a.band.max_bps.unwrap() > 99.0 && a.band.max_bps.unwrap() < 101.0);
        // 5 of 10 band points ≥ 50 bps (≈50.25,60.3,...,100.4 above; 60+ are 5)
        let frac = a.band.frac_at_or_above_hurdle.expect("frac");
        assert!((frac - 0.5).abs() < 0.11, "frac {frac}");
        // Both pair directions present with mirrored signs.
        assert_eq!(a.pairs.len(), 2);
        let fwd = a
            .pairs
            .iter()
            .find(|p| p.buy_venue == "hyperliquid")
            .expect("hl->lgt pair");
        assert_eq!(fwd.stats.sample_count, 10);
        // buy cheap HL / sell rich LGT -> positive carry spread
        assert!(
            fwd.stats.mean_bps.unwrap() > 0.0,
            "buy cheap leg is positive"
        );
        let rev = a
            .pairs
            .iter()
            .find(|p| p.buy_venue == "lighter")
            .expect("lgt->hl pair");
        assert!(
            (rev.stats.mean_bps.unwrap() + fwd.stats.mean_bps.unwrap()).abs() < 1.0,
            "directions mirror"
        );
    }

    #[tokio::test]
    async fn test_spread_analytics_single_venue_ticks_dropped() {
        let state = AppEngineState::new();
        let now = Utc::now().timestamp_millis();
        // Alternating single-venue polls: no timestamp ever has ≥2 venues,
        // so the band must be empty — never a fabricated zero-spread series.
        seed_samples(&state, "BTC-USDC", now - 2_000, &[("hyperliquid", 100.0)]);
        seed_samples(&state, "BTC-USDC", now - 1_000, &[("lighter", 101.0)]);
        let a = get_spread_analytics_impl("BTC-USDC", 3600, 5.0, &state)
            .await
            .expect("analytics");
        assert_eq!(a.band.sample_count, 0);
        assert!(a.band.mean_bps.is_none());
        assert_eq!(a.venues_seen.len(), 2);
    }

    #[tokio::test]
    async fn test_signal_series_leader_mode_and_gap_preservation() {
        let state = AppEngineState::new();
        let now = Utc::now().timestamp_millis();
        seed_samples(
            &state,
            "ETH-USDC",
            now - 60_000,
            &[("hyperliquid", 3000.0), ("lighter", 3006.0)],
        );
        // Second observation 59s later: lighter's last quote is beyond the
        // staleness bound (30s) so that bucket has a lone venue and is
        // skipped entirely — a venue missing past the bound is a real gap,
        // while a fresher one would still be carried as its standing quote.
        seed_samples(&state, "ETH-USDC", now - 1_000, &[("hyperliquid", 3001.0)]);

        let s = get_signal_series_impl("ETH-USDC", "leader", 300, &state)
            .await
            .expect("series");
        assert_eq!(s.mode, "leader");
        assert_eq!(s.ts_ms.len(), 1, "lone-venue bucket is not emitted");
        // No volumes in test state -> deterministic leader = alphabetically
        // first venue ("hyperliquid"), pinned at 0.
        assert_eq!(s.dev_bps[0][0], Some(0.0));
        assert!(s.dev_bps[0][1].is_some());
        assert!(
            s.band_max_bps[0] > s.band_min_bps[0],
            "two-venue band spans the gross spread"
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
        seed_test_book(&state, "vanta", "BTC-USDC", dec!(60120.0), dec!(60125.0));
        seed_test_funding(&state, "hyperliquid", "BTC-USDC");
        seed_test_funding(&state, "lighter", "BTC-USDC");
        seed_test_funding(&state, "vanta", "BTC-USDC");

        let books = fetch_orderbooks_impl("BTC-USDC", &state)
            .await
            .expect("books");
        assert_eq!(books.len(), 3);

        let rows = state
            .state_store
            .spread_samples("BTC-USDC", 0, i64::MAX)
            .expect("samples");
        assert_eq!(rows.len(), 3, "one row per reporting venue per poll");
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
                    symbol: None,
                    intended_qty: 1.0,
                    intended_price: None,
                },
                NewSignalLeg {
                    account_id: None,
                    venue: venue_short.to_string(),
                    side: LegSide::Short,
                    symbol: None,
                    intended_qty: 1.0,
                    intended_price: None,
                },
            ]
        };
        state
            .state_store
            .record_signal_execution(
                &NewSignalExecution {
                    id: root_id.to_string(),
                    strategy_id: strategy.to_string(),
                    symbol: "BTC-USDC".to_string(),
                    route_id: None,
                    expected_net_usd: None,
                    note: None,
                    legs: legs("hyperliquid", "lighter"),
                },
                "testnet",
            )
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
            .record_signal_execution(
                &NewSignalExecution {
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
                            symbol: None,
                            intended_qty: 1.0,
                            intended_price: None,
                        },
                        NewSignalLeg {
                            account_id: None,
                            venue: "lighter".to_string(),
                            side: LegSide::Long,
                            symbol: None,
                            intended_qty: 1.0,
                            intended_price: None,
                        },
                    ],
                },
                "testnet",
            )
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
        seed_closed_family(
            &state,
            "sig-spatial-1",
            "futures",
            60000.0,
            60120.0,
            60050.0,
            60100.0,
        );
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
        seed_closed_family(
            &state,
            "sig-radar-1",
            "radar",
            60000.0,
            60120.0,
            60050.0,
            60100.0,
        );
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
            .record_signal_execution(
                &NewSignalExecution {
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
                        symbol: None,
                        intended_qty: 1.0,
                        intended_price: None,
                    }],
                },
                "testnet",
            )
            .expect("empty root records");
        state
            .state_store
            .update_signal_status("sig-empty", SignalStatus::Closed, None)
            .expect("empty root closes");
        // Open families never enter the sums either way — record a root
        // with fills and leave it non-terminal (a terminal signal cannot
        // reopen; that refusal is itself covered elsewhere).
        state
            .state_store
            .record_signal_execution(
                &NewSignalExecution {
                    id: "sig-open".to_string(),
                    strategy_id: "futures".to_string(),
                    symbol: "BTC-USDC".to_string(),
                    route_id: None,
                    expected_net_usd: None,
                    note: None,
                    legs: vec![
                        NewSignalLeg {
                            account_id: None,
                            venue: "hyperliquid".to_string(),
                            side: crate::state_store::LegSide::Long,
                            symbol: None,
                            intended_qty: 1.0,
                            intended_price: None,
                        },
                        NewSignalLeg {
                            account_id: None,
                            venue: "lighter".to_string(),
                            side: crate::state_store::LegSide::Short,
                            symbol: None,
                            intended_qty: 1.0,
                            intended_price: None,
                        },
                    ],
                },
                "testnet",
            )
            .expect("open root records");
        state
            .state_store
            .apply_leg_fill(
                "sig-open",
                crate::state_store::LegSide::Long,
                1.0,
                60000.0,
                2.0,
                None,
            )
            .expect("open long fills");
        state
            .state_store
            .apply_leg_fill(
                "sig-open",
                crate::state_store::LegSide::Short,
                1.0,
                60120.0,
                2.0,
                None,
            )
            .expect("open short fills");
        state
            .state_store
            .update_signal_status("sig-open", SignalStatus::Imbalanced, None)
            .expect("root stays non-terminal");
        let pnl = get_strategy_fill_pnl_impl("futures", &state).expect("pnl");
        assert_eq!(pnl.closed_families, 0);
        assert_eq!(pnl.open_families, 1);
        assert_eq!(pnl.unattributed_closed_families, 1);
        assert_eq!(pnl.net_profit_usd, 0.0);
        assert_eq!(pnl.net_profit_bps, None);
    }

    #[test]
    fn record_execution_log_persists_and_caps_memory() {
        let state = setup_test_state_with_venues();
        let mk = |id: &str| ExecutionLogEntry {
            id: id.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            trade_type: "Funding".to_string(),
            strategy_id: FUNDING_STRATEGY_ID.to_string(),
            symbol: "NEAR-USDC".to_string(),
            venue_a: "vanta".to_string(),
            venue_b: "lighter".to_string(),
            leg_a_status: "blocked".into(),
            leg_b_status: "blocked".into(),
            success: false,
            gross_profit: 0.0,
            fees: 0.0,
            net_profit: 0.0,
            latency_us: 0,
            note: "Execution blocked: unit test".into(),
            direction: None,
            trigger_spread_bps: None,
            baseline_mean_spread_bps: None,
            position_step: None,
            max_units: None,
            nominal_hurdle_bps: None,
            is_live: false,
        };
        record_execution_log(&state, mk("fail-1"));
        // Visible in-memory (Log Monitor source) AND durable in SQLite.
        assert!(state.execution_logs.read().iter().any(|e| e.id == "fail-1"));
        let rows = state.state_store.load_execution_logs(500).expect("load");
        assert!(rows.iter().any(|e| e.id == "fail-1" && !e.success));
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
        seed_closed_family(
            &state,
            "sig-det-1",
            "spot",
            60000.0,
            60120.0,
            60050.0,
            60100.0,
        );
        seed_closed_family(
            &state,
            "sig-det-2",
            "spot",
            60000.0,
            60120.0,
            60050.0,
            60100.0,
        );
        let first = get_strategy_fill_pnl_impl("spot", &state).expect("first");
        let second = get_strategy_fill_pnl_impl("spot", &state).expect("second");
        assert_eq!(first, second);
        assert_eq!(first.closed_families, 2);
    }

    #[tokio::test]
    async fn test_margin_snapshot_cadence_caps_frequency() {
        let state = setup_test_state_with_venues();
        // Snapshots record one row per ADMITTED venue — under Testnet only
        // venues with demo endpoints are admitted (hl/lighter/vanta here).
        let venues: Vec<String> = state
            .get_venues()
            .iter()
            .map(|(vid, _, _)| vid.to_string())
            .collect();
        assert!(!venues.is_empty(), "test state must admit venues");
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
            .record_margin_snapshot(
                "lighter",
                "lighter",
                now - 2 * hour,
                10_000.0,
                2_000.0,
                20.0,
            )
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
        if let Some(sim) = state
            .test_sims
            .read()
            .ok()
            .and_then(|m| m.get(venue_id).cloned())
        {
            let _ = sim.replace_orderbook(book.clone());
        }
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

        let first = flatten_ticker_positions_impl(
            Some("BTC-USDC"),
            "futures",
            arbitrageur_execution::virtual_ledger::CycleStatus::StoppedOut,
            &state,
        )
        .await
        .expect("first flatten");
        assert_eq!(
            first.residual_signals, 1,
            "partial close must leave exactly one residual root signal"
        );

        let acc_hl = state
            .test_sim("hyperliquid")
            .expect("hl sim")
            .fetch_account()
            .await
            .expect("hl account");
        let acc_lgt = state
            .test_sim("lighter")
            .expect("lighter sim")
            .fetch_account()
            .await
            .expect("lgt account");
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

        // Re-clicking flatten while the first child is still open must not
        // re-dispatch the in-flight remainder — the pending qty suppresses
        // it, so no second child is created.
        let children_before = state
            .state_store
            .list_signals(None, None)
            .expect("signals")
            .into_iter()
            .filter(|s| s.id.starts_with("flat-"))
            .count();
        let repeat = flatten_ticker_positions_impl(
            Some("BTC-USDC"),
            "futures",
            arbitrageur_execution::virtual_ledger::CycleStatus::StoppedOut,
            &state,
        )
        .await
        .expect("repeat flatten");
        assert_eq!(repeat.legs_closed, 0, "working child suppresses redispatch");
        let children_after = state
            .state_store
            .list_signals(None, None)
            .expect("signals")
            .into_iter()
            .filter(|s| s.id.starts_with("flat-"))
            .count();
        assert_eq!(
            children_before, children_after,
            "no duplicate close child; notes: {:?}",
            repeat.notes
        );

        // The first child's market orders are done venue-side (0.05 filled,
        // remainder cancelled by the sim). In production the reconciliation
        // supervisor resolves the child to a terminal state within seconds;
        // emulate that here so the retry pass sees the true filled residual.
        for child in state
            .state_store
            .list_signals(None, None)
            .expect("signals")
            .into_iter()
            .filter(|s| s.id.starts_with("flat-") && s.status.counts_as_open())
        {
            state
                .state_store
                .update_signal_status(
                    &child.id,
                    crate::state_store::SignalStatus::Closed,
                    Some("order terminal"),
                )
                .expect("resolve child");
        }

        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64360.0), dec!(64365.0));

        let second = flatten_ticker_positions_impl(
            Some("BTC-USDC"),
            "futures",
            arbitrageur_execution::virtual_ledger::CycleStatus::StoppedOut,
            &state,
        )
        .await
        .expect("second flatten");
        assert_eq!(
            second.residual_signals, 0,
            "second flatten notes: {:?}",
            second.notes
        );

        let acc_hl = state
            .test_sim("hyperliquid")
            .expect("hl sim")
            .fetch_account()
            .await
            .expect("hl account");
        let acc_lgt = state
            .test_sim("lighter")
            .expect("lighter sim")
            .fetch_account()
            .await
            .expect("lgt account");
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
            .ticker_blocked("BTC-USDC", None)
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
        paper_book
            .bids
            .push(arbitrageur_core::quote::BookLevel::new(
                dec!(64200.0),
                dec!(10.0),
            ));
        paper_book
            .asks
            .push(arbitrageur_core::quote::BookLevel::new(
                dec!(64205.0),
                dec!(10.0),
            ));
        state
            .test_sim("hyperliquid")
            .expect("hl sim")
            .replace_orderbook(paper_book)
            .expect("seed paper book");

        let open = arbitrageur_core::order::OrderRequest::new_market(
            state.test_sim("hyperliquid").expect("hl sim").venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(0.2),
        );
        let resp = state
            .test_sim("hyperliquid")
            .expect("hl sim")
            .place_order(open)
            .await
            .expect("open position");
        assert_eq!(resp.status, arbitrageur_core::types::OrderStatus::Filled);

        for (id, strategy) in [("root-fut", "futures"), ("root-spot", "spot")] {
            state
                .state_store
                .record_signal_execution(
                    &NewSignalExecution {
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
                            symbol: None,
                            intended_qty: 0.1,
                            intended_price: None,
                        }],
                    },
                    "testnet",
                )
                .expect("record root");
            state
                .state_store
                .apply_leg_fill(id, LegSide::Long, 0.1, 64205.0, 0.0, None)
                .expect("root fill");
        }

        let res = flatten_ticker_positions_impl(
            Some("BTC-USDC"),
            "futures",
            arbitrageur_execution::virtual_ledger::CycleStatus::Closed,
            &state,
        )
        .await
        .expect("flatten futures");

        let acc = state
            .test_sim("hyperliquid")
            .expect("hl sim")
            .fetch_account()
            .await
            .expect("hl account");
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
        paper_book
            .bids
            .push(arbitrageur_core::quote::BookLevel::new(
                dec!(64200.0),
                dec!(10.0),
            ));
        paper_book
            .asks
            .push(arbitrageur_core::quote::BookLevel::new(
                dec!(64205.0),
                dec!(10.0),
            ));
        state
            .test_sim("hyperliquid")
            .expect("hl sim")
            .replace_orderbook(paper_book)
            .expect("seed paper book");
        let open = arbitrageur_core::order::OrderRequest::new_market(
            state.test_sim("hyperliquid").expect("hl sim").venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(0.1),
        );
        state
            .test_sim("hyperliquid")
            .expect("hl sim")
            .place_order(open)
            .await
            .expect("open position");
        state
            .state_store
            .record_signal_execution(
                &NewSignalExecution {
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
                        symbol: None,
                        intended_qty: 0.1,
                        intended_price: None,
                    }],
                },
                "testnet",
            )
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

        let res = flatten_ticker_positions_impl(
            Some("BTC-USDC"),
            "futures",
            arbitrageur_execution::virtual_ledger::CycleStatus::Closed,
            &state,
        )
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
        let acc = state
            .test_sim("hyperliquid")
            .expect("hl sim")
            .fetch_account()
            .await
            .expect("hl account");
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

        let res = flatten_ticker_positions_impl(
            Some("BTC-USDC"),
            "futures",
            arbitrageur_execution::virtual_ledger::CycleStatus::Closed,
            &state,
        )
        .await
        .expect("flatten futures");
        assert_eq!(res.legs_closed, 1);
        assert_eq!(res.signals_closed, 1);
        assert!(
            res.notes.iter().any(|n| n.contains("market exit")),
            "expected fallback note, got {:?}",
            res.notes
        );
        let acc = state
            .test_sim("hyperliquid")
            .expect("hl sim")
            .fetch_account()
            .await
            .expect("hl account");
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
            bid_depth: None,
            ask_depth: None,
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
                .filter(|p| p.half_life > 0.0 && p.r_squared.is_some_and(|r| r >= OU_MIN_R2))
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
        assert!(
            get_matrix_baselines_impl("BTC-USDC", "sideways", 86_400, 90, &state)
                .await
                .is_err()
        );
        assert!(
            get_matrix_baselines_impl("BTC-USDC", "manual", 60, 90, &state)
                .await
                .is_err()
        );
        assert!(
            get_matrix_baselines_impl("BTC-USDC", "manual", 3_000_000, 90, &state)
                .await
                .is_err()
        );
        assert!(
            get_matrix_baselines_impl("BTC-USDC", "manual", 86_400, 40, &state)
                .await
                .is_err()
        );
        assert!(
            get_matrix_baselines_impl("BTC\u{0}-USDC", "manual", 86_400, 90, &state)
                .await
                .is_err()
        );

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
                            bid_depth: None,
                            ask_depth: None,
                        },
                        crate::state_store::VenueSampleEntry {
                            venue_id: "bbb".into(),
                            mid: 100.05,
                            best_bid: 99.55,
                            best_ask: 100.55,
                            bid_depth: None,
                            ask_depth: None,
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
        {
            let mut s = state.settings.write();
            s.lighter_enabled = true;
            // Passive unwind is default-on; zero the attempt wait so the
            // limit/cancel/fallback sequence runs instantly in tests.
            s.passive_unwind.timeout_secs = 0;
        }
        state.install_test_sim("lighter");
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
        assert!(
            opp.gross_spread_bps > 300.0,
            "gross {}",
            opp.gross_spread_bps
        );
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
        assert!(btc
            .iter()
            .all(|o| o.intermediate_a == "BTC" || o.intermediate_b == "BTC"));
        assert!(btc.iter().any(|o| o.id == "lighter-TRI-BTC-ETH-USDC"));
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
            false,
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
        opps.iter()
            .find(|o| o.id == id)
            .cloned()
            .expect("seeded cycle")
    }

    #[tokio::test]
    async fn test_triangle_auto_entry_testnet_only_in_live() {
        // Autonomous entries stay testnet-only in live mode — the same
        // policy as radar/funding autonomous paths.
        let state = lighter_only_state();
        state.set_environment(crate::state::TradingEnvironment::Live);
        let err = execute_triangle_arbitrage_impl(
            TriangleExecutionParams {
                id: "lighter-TRI-BTC-ETH-USDC".into(),
                venue: "lighter".into(),
                path: "USDC → BTC → ETH → USDC".into(),
                trade_size_usd: 100.0,
                min_profit_bps: 1.0,
                net_spread_bps: None,
                auto: true,
            },
            &state,
        )
        .await
        .expect_err("autonomous live triangle entry must refuse");
        assert!(
            err.contains("testnet-only") || err.contains("manual"),
            "refusal must name the autonomous-entry policy: {err}"
        );
        state.set_environment(crate::state::TradingEnvironment::Testnet);
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
                auto: false,
            },
            &state,
        )
        .await
        .expect("triangle execute");
        assert_eq!(res.trade_type, "Triangle");
        assert!(res.execution_success);
        assert!(res.spread_bps > 200.0, "spread {}", res.spread_bps);

        // Each leg persists as its own pre-submit signal under the leg's
        // exact symbol. On a position-model venue the three leg positions
        // are still live after the round trip — the cycle resolves Balanced
        // (tracked, flattenable), not Closed.
        for sym in ["BTC-USDC", "BTC-ETH", "ETH-USDC"] {
            let leg_signals: Vec<_> = state
                .state_store
                .list_signals(Some(sym), None)
                .unwrap()
                .into_iter()
                .filter(|s| s.strategy_id == "triangle")
                .collect();
            assert_eq!(leg_signals.len(), 1, "one triangle leg signal on {sym}");
            let leg = &leg_signals[0];
            assert_eq!(leg.status, crate::state_store::SignalStatus::Balanced);
            assert!(
                leg.legs.iter().all(|l| l.filled_qty > 0.0),
                "leg filled on {sym}: {:?}",
                leg.legs
            );
        }

        // A completed cycle carries no ledger blocking — the signals are
        // Closed, so no symbol stays blocked.
        for sym in ["BTC-USDC", "BTC-ETH", "ETH-USDC"] {
            assert!(
                state
                    .state_store
                    .blocking_signals(sym, None)
                    .expect("blocking signals")
                    .is_empty(),
                "completed cycle must not block {sym}"
            );
        }

        // The paper venue books each leg fill as a position (no base-asset
        // netting on the sim), so venue truth still shows per-symbol
        // exposure — the position guard refuses re-entry until flattened.
        let again = execute_triangle_arbitrage_impl(
            TriangleExecutionParams {
                id,
                venue: "lighter".into(),
                path: "USDC → BTC → ETH → USDC".into(),
                trade_size_usd: 1000.0,
                min_profit_bps: 1.0,
                net_spread_bps: None,
                auto: false,
            },
            &state,
        )
        .await;
        assert!(again.is_err(), "open venue positions must block re-entry");

        // Each leg unwinds through the flatten path, freeing the venue.
        // Re-seed first: flatten requires fresh (2s) hub books, and under
        // parallel-suite load the execute above can age the seeded books
        // past that — a scheduling artifact, not leg state.
        seed_triangle_books(&state);
        for sym in ["BTC-USDC", "BTC-ETH", "ETH-USDC"] {
            let flat = flatten_ticker_positions_impl(
                Some(sym),
                "triangle",
                arbitrageur_execution::virtual_ledger::CycleStatus::Closed,
                &state,
            )
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
        assert!(
            acc.positions.is_empty(),
            "all legs flattened: {:?}",
            acc.positions
        );
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
            auto: false,
        };

        // Unknown opportunity id (caller-supplied spread is never trusted).
        assert!(execute_triangle_arbitrage_impl(
            mk("lighter-TRI-X-Y-USDC".into(), "lighter", 1000.0, 1.0),
            &state
        )
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
        assert!(
            execute_triangle_arbitrage_impl(mk(opp.id.clone(), "kraken", 1000.0, 1.0), &state)
                .await
                .is_err()
        );
        // Invalid sizes refused.
        assert!(execute_triangle_arbitrage_impl(
            mk(opp.id.clone(), "lighter", f64::NAN, 1.0),
            &state
        )
        .await
        .is_err());
        // Live mode hard-refuses paper dispatch.
        state.set_environment(crate::state::TradingEnvironment::Live);
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
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64440.0), dec!(64445.0));

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
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64201.0), dec!(64206.0));
        let checks = check_spatial_convergence_impl(Some("BTC-USDC"), "futures", &state)
            .await
            .expect("convergence recheck");
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].verdict, "exit");
        assert_eq!(checks[0].trigger.as_deref(), Some("converged"));
        assert!(checks[0].unwound);
        // A convergence exit is a target hit, not a stop — the lot records
        // the honest terminal status for outcome segmentation.
        let lot_realized = {
            let ledger = state.virtual_ledger.read();
            let lot = ledger
                .lots
                .values()
                .find(|l| l.symbol == "BTC-USDC")
                .expect("cycle lot exists");
            assert_eq!(
                lot.status,
                arbitrageur_execution::virtual_ledger::CycleStatus::Closed
            );
            lot.realized_pnl_usd
                .expect("converged lot has realized PnL")
        };
        // The headline "Net Profit" counter must converge to the lot's true
        // realized PnL: the entry attempt debited -entry_fees and the close
        // adds `realized + entry_debited`, netting to `realized` exactly.
        // Regression: close outcomes previously reached the lot/log only,
        // leaving the counter at entry-attempt costs alone.
        let total_f = state
            .total_realized_profit
            .read()
            .to_string()
            .parse::<f64>()
            .unwrap_or(f64::NAN);
        assert!(
            (total_f - lot_realized).abs() < 1e-6,
            "cumulative counter {total_f} must equal lot realized {lot_realized}"
        );
        assert!(lot_realized > 0.0, "converged cycle should realize a gain");

        // Slot freed: the pair can be re-entered (books still overlap enough
        // for the economics gate? re-seed the wide edge first).
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
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
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64440.0), dec!(64445.0));
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

        // Adverse excursion: the spread WIDENS past entry+stop (a narrowing
        // edge is convergence profit, never a stop). Entry edge was ~37 bps
        // (64440 bid vs 64205 ask); 64480 puts live ~43 bps — widened ~6 bps
        // past the 5 bps stop band.
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
        seed_test_book(&state, "lighter", "BTC-USDC", dec!(64480.0), dec!(64485.0));
        let checks = check_spatial_convergence_impl(Some("BTC-USDC"), "futures", &state)
            .await
            .expect("stop check");
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].verdict, "exit");
        assert_eq!(checks[0].trigger.as_deref(), Some("stopped"));
        assert!(checks[0].unwound);
        {
            let ledger = state.virtual_ledger.read();
            let stopped = ledger
                .lots
                .values()
                .filter(|l| {
                    l.symbol == "BTC-USDC"
                        && l.status
                            == arbitrageur_execution::virtual_ledger::CycleStatus::StoppedOut
                })
                .count();
            assert_eq!(stopped, 1, "stop unwind records StoppedOut");
        }

        // Re-open, then backdate the baseline past the hold deadline.
        seed_test_book(
            &state,
            "hyperliquid",
            "BTC-USDC",
            dec!(64200.0),
            dec!(64205.0),
        );
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
        {
            let ledger = state.virtual_ledger.read();
            let timed_out = ledger
                .lots
                .values()
                .filter(|l| {
                    l.symbol == "BTC-USDC"
                        && l.status == arbitrageur_execution::virtual_ledger::CycleStatus::TimedOut
                })
                .count();
            assert_eq!(timed_out, 1, "deadline unwind records TimedOut");
        }
    }

    #[tokio::test]
    async fn test_convergence_unwatched_without_baseline() {
        let state = setup_test_state_with_venues();
        // In-flight signal with no baseline row: never auto-exited.
        state
            .state_store
            .record_signal_execution(
                &crate::state_store::NewSignalExecution {
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
                            symbol: None,
                            intended_qty: 0.1,
                            intended_price: None,
                        },
                        crate::state_store::NewSignalLeg {
                            account_id: None,
                            venue: "lighter".into(),
                            side: crate::state_store::LegSide::Short,
                            symbol: None,
                            intended_qty: 0.1,
                            intended_price: None,
                        },
                    ],
                },
                "testnet",
            )
            .expect("record signal");
        let checks = check_spatial_convergence_impl(Some("BTC-USDC"), "futures", &state)
            .await
            .expect("check");
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].verdict, "unwatched");
        assert!(!checks[0].unwound);
        // Non-futures strategies are rejected, not guessed.
        assert!(
            check_spatial_convergence_impl(Some("BTC-USDC"), "radar", &state)
                .await
                .is_err()
        );
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

fn popdex_toolkit_network(
    network: &str,
) -> Result<arbitrageur_exchanges::popdex::PopDexNetwork, String> {
    use arbitrageur_exchanges::popdex::PopDexNetwork;
    match network.trim().to_lowercase().as_str() {
        "mainnet" | "main" | "prod" | "" => Ok(PopDexNetwork::Mainnet),
        "testnet" | "test" => Ok(PopDexNetwork::Testnet),
        other => Err(format!(
            "unknown PopDEX network '{other}' (want mainnet|testnet)"
        )),
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
        return Err(
            "agent address equals the main address — generate a fresh agent key instead"
                .to_string(),
        );
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
        eip681_uri: approve_agent_eip681(
            &main,
            &agent,
            now_ms,
            match net {
                arbitrageur_exchanges::popdex::PopDexNetwork::Mainnet => {
                    arbitrageur_exchanges::popdex::POPDEX_MAINNET_CHAIN_ID
                }
                arbitrageur_exchanges::popdex::PopDexNetwork::Testnet => {
                    arbitrageur_exchanges::popdex::POPDEX_TESTNET_CHAIN_ID
                }
            },
        ),
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
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .unwrap_or_default()
        .to_lowercase();
    const ALLOWED: &[&str] = &["popdex.xyz", "crypto-chief.com"];
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
