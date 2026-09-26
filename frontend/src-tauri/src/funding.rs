//! Funding-carry strategy: cross-venue funding-rate scan (perp/perp carry
//! and spot/perp basis), 8h-normalized leg rates, gap history + persistence
//! probability, trend substrate, and the two-leg IOC carry executor.
//! Split out of `commands.rs`; shared execution helpers are reached via
//! `crate::commands`.

use crate::commands::{
    backtest_stats, dec_to_f64, effective_venue_fee_bps, get_market_universe_impl, live_entry_gate,
    margin_monitor_from_settings, maybe_record_margin_snapshots, reconcile_signal_legs,
    record_execution_failure, record_execution_log, refresh_equity_expectations_after_fills,
    resolve_effective_leverage, strategy_slippage_bps, validate_adapter_book,
    validate_market_symbol, SimStepResultDto, FUNDING_STRATEGY_ID,
};
use crate::state::{AppEngineState, ExecutionLogEntry};
use crate::triangle::triangle_dust_qty;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::order::{OrderRequest, OrderResponse};
use arbitrageur_core::quote::OrderBook;
use arbitrageur_core::types::{dec, Decimal, OrderStatus, Side, VenueId};
use arbitrageur_execution::coordinator::ArbitrageExecutionResult;
use arbitrageur_math::book_gradient::entry_gradient;
use arbitrageur_math::funding_carry::{
    estimate_funding_hold_hours, evaluate_funding_carry, evaluate_spot_perp_basis,
    expected_carry_return_usd, funding_entry_capacity, funding_win_probability, WinHoldContext,
};
use arbitrageur_risk::margin_monitor::MarginHealthStatus;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::State;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FundingOpportunityDto {
    pub id: String,
    pub symbol: String,
    pub long_venue: String,
    pub short_venue: String,
    /// Canonical venue ID of the long leg (adapter/paper key, never a
    /// display label such as "Lighter SPOT").
    pub long_venue_id: String,
    /// Canonical venue ID of the short leg.
    pub short_venue_id: String,
    /// Exact execution symbol of the long leg (requested perp, or the
    /// discovered `*-SPOT` symbol for basis).
    pub long_symbol: String,
    /// Exact execution symbol of the short leg (always the requested perp).
    pub short_symbol: String,
    /// Market type of the long leg: "perpetual" (carry) or "spot" (basis).
    pub long_market_type: String,
    /// Market type of the short leg: always "perpetual".
    pub short_market_type: String,
    pub net_rate_8h_pct: f64,
    pub net_apr_pct: f64,
    /// Compounded-if-reinvested APY; `None` when the backend cannot express
    /// a finite compounded figure (extreme rates) — render as unavailable,
    /// never as zero.
    pub net_apy_pct: Option<f64>,
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
    /// Opportunity kind: "carry" (cross-exchange perp/perp funding),
    /// "basis" (same-exchange long-spot / short-perp cash-and-carry), or
    /// "fixed_yield" (the same perp pair plus two Pendle Boros YU hedge
    /// legs locking the net rate to maturity). Defaults to "carry" so
    /// stale backends/clients keep working.
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
    /// Informational win probability (modelled-hold fee-coverage heuristic,
    /// 0..1, same decay-averaged accrual and fee+spread costs as
    /// `expected_return_usd`, so p >= 0.5 agrees in sign with a non-negative
    /// expectation). `None` when the edge is unusable. Never gates ordering or
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
    /// Expected net carry in USD over the modelled hold with linear
    /// funding-gap decay to the min-APR exit threshold, net of round-trip
    /// fees + measured entry spread (signed; negative = expected loss).
    /// `None` when the hold is unknown — clients fall back to a flat
    /// projection, never invent one.
    #[serde(default)]
    pub expected_return_usd: Option<f64>,
    /// Empirical backtest win rate over trailing gap history (fraction of
    /// simulated entries covering costs). `None` below 20 trials.
    /// Display only — gate on `backtest_win_rate_lower`.
    #[serde(default)]
    pub backtest_win_rate: Option<f64>,
    /// Wilson lower/upper bounds of the backtest win rate at z=1.645 on
    /// `backtest_episodes` (distinct gap episodes — the effective sample
    /// size; overlapping forward paths from one episode are correlated).
    #[serde(default)]
    pub backtest_win_rate_lower: Option<f64>,
    #[serde(default)]
    pub backtest_win_rate_upper: Option<f64>,
    /// Distinct contiguous above-threshold episodes the trials entered
    /// in — the honest sample size behind the win rate.
    #[serde(default)]
    pub backtest_episodes: Option<u32>,
    /// Regime split of the backtest win rate: entries made Mon–Fri vs
    /// Sat–Sun UTC (`None` below 8 trials per regime). Funding behaviour
    /// differs materially on weekends.
    #[serde(default)]
    pub weekday_win_rate: Option<f64>,
    #[serde(default)]
    pub weekend_win_rate: Option<f64>,
    /// Measured persistence probability: of historical prints above the
    /// entry threshold, the fraction whose gap stayed above it for the
    /// full fee+spread breakeven horizon — a real estimated "edge
    /// survives to breakeven" probability, unlike the `win_probability`
    /// coverage heuristic. `None` below 20 scoreable prints.
    #[serde(default)]
    pub persistence_probability: Option<f64>,
    /// Wilson lower bound of `persistence_probability`.
    #[serde(default)]
    pub persistence_probability_lower: Option<f64>,
    /// Empirical backtest mean net USD per entry. `None` below 20 trials.
    #[serde(default)]
    pub backtest_expected_usd: Option<f64>,
    /// Standard error of `backtest_expected_usd` at the effective
    /// (episode-level) sample size.
    #[serde(default)]
    pub backtest_expected_usd_se: Option<f64>,
    /// Backtest entry trials behind the two figures above.
    #[serde(default)]
    pub backtest_trials: Option<u32>,
    /// Ascending net-USD deciles (P0…P100, 11 points) enabling P(net >= any
    /// target) client-side without re-running trials. `None` below 20 trials.
    #[serde(default)]
    pub backtest_deciles_usd: Option<Vec<f64>>,
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
    /// Depth curve: cumulative/marginal net & expected carry profit per
    /// unit while walking both leg books in lockstep. Income is
    /// depth-invariant (net rate × modelled hold); the curve degrades
    /// purely through the entry cross. Empty when either book is missing.
    #[serde(default)]
    pub depth_curve: Vec<crate::monitor::DepthCurvePointDto>,
    /// Profit-maximizing size (USD): depth where the marginal unit's net
    /// turns non-positive — beyond it every added dollar shrinks total
    /// profit. `None` when no book depth exists.
    #[serde(default)]
    pub optimal_capacity_usd: Option<f64>,
    /// Largest still-profitable size (USD): depth where the whole
    /// position's cumulative net crosses zero. `None` when no book depth
    /// exists.
    #[serde(default)]
    pub max_capacity_usd: Option<f64>,
    // --- Fixed-yield (Boros YU hedge) fields — `kind == "fixed_yield"` ---
    /// Locked net APR in percent (`F_short − F_long`), executable at the
    /// YU book's top: short leg receives the bid-side fixed rate, long
    /// leg pays the ask-side fixed rate. `None` for carry/basis rows.
    #[serde(default)]
    pub fixed_apr_pct: Option<f64>,
    /// The two Boros YU hedge legs that convert this carry's floating
    /// funding into a locked rate. `None` for carry/basis rows.
    #[serde(default)]
    pub yu_legs: Option<Vec<YuLegDto>>,
    /// Extra entry cost of the 4-leg variant vs the 2-leg carry, in bps
    /// of notional: two YU taker fees plus amortized Arbitrum gas.
    #[serde(default)]
    pub yu_hedge_cost_bps: Option<f64>,
    /// Estimated Boros collateral required for the two YU legs (USD) —
    /// Boros max leverage is low (~1.5-3x), so this is a real capital
    /// bound, not a rounding detail.
    #[serde(default)]
    pub boros_margin_usd: Option<f64>,
    /// Estimated Arbitrum gas for the YU batch (USD).
    #[serde(default)]
    pub boros_gas_usd: Option<f64>,
    /// Unix-ms timestamp of the shared YU maturity — the fixed rate is
    /// locked until then. `None` for carry/basis rows.
    #[serde(default)]
    pub lock_until_ms: Option<i64>,
}

/// One Boros Yield-Unit hedge leg of a fixed-yield opportunity.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct YuLegDto {
    /// Canonical venue id whose funding this leg swaps (e.g. "binance").
    pub venue_id: String,
    pub yu_market_id: u64,
    /// Boros market symbol, e.g. `BINANCE-BTCUSDT-30OCT2026`.
    pub yu_symbol: String,
    /// "long" = pay fixed / receive floating (hedges the long perp leg);
    /// "short" = receive fixed / pay floating (hedges the short perp leg).
    pub yu_side: String,
    /// Executable fixed APR in percent (bid for short, ask for long).
    pub fixed_apr_pct: f64,
    /// YU taker fee in bps of notional.
    pub fee_bps: f64,
    /// Unix seconds — the maturity this leg locks to.
    pub maturity_unix: i64,
}

fn default_funding_opp_kind() -> String {
    "carry".to_string()
}

/// Capacity bounds from a depth curve: `(optimal, maximum)` USD.
/// Optimal = deepest point whose marginal net is still positive (the
/// profit peak — deeper sizing only shrinks the total). Maximum =
/// deepest point while the whole position's cumulative net stays
/// positive (the breakeven bound). Marginal cost is monotone worsening
/// in depth, so `take_while` lands on the true boundaries; an empty or
/// instantly-unprofitable curve yields `None`.
fn capacity_bounds(curve: &[crate::monitor::DepthCurvePointDto]) -> (Option<f64>, Option<f64>) {
    if curve.is_empty() {
        return (None, None);
    }
    let optimal = curve
        .iter()
        .take_while(|p| p.marg_net_bps > 0.0)
        .map(|p| p.cum_usd)
        .last();
    let maximum = curve
        .iter()
        .take_while(|p| p.cum_net_bps > 0.0)
        .map(|p| p.cum_usd)
        .last();
    (optimal, maximum)
}

// ---------------------------------------------------------------------------
// Pendle Boros fixed-yield variant (4-leg: perp pair + two YU hedge legs)
// ---------------------------------------------------------------------------

/// Minimum lock term for a fixed-yield row: sub-day locks almost never
/// cover the extra leg costs, and emitting them would just be noise.
const FIXED_MIN_LOCK_HOURS: f64 = 24.0;

/// The Boros adapter the fixed-yield executor trades through. Production
/// always uses `live_boros` (Boros has no testnet — fixed-yield is
/// live-only); tests inject a stub-backed `BorosAdapter` here so the real
/// signing/wire path runs against a local fixture.
#[cfg(not(test))]
fn boros_for_exec(state: &AppEngineState) -> Arc<arbitrageur_exchanges::boros::BorosAdapter> {
    state.live_boros.clone()
}
#[cfg(test)]
fn boros_for_exec(state: &AppEngineState) -> Arc<arbitrageur_exchanges::boros::BorosAdapter> {
    if let Some(a) = state.test_boros.read().ok().and_then(|g| g.clone()) {
        return a;
    }
    state.live_boros.clone()
}

/// YU order settle-poll cadence: each poll is real venue truth
/// (`fetch_order`), the budget bounds how long a hedge dispatch can run
/// past its ack. Test builds shorten the cadence so the never-resolves
/// scenario finishes in milliseconds instead of seconds.
#[cfg(not(test))]
const YU_SETTLE_POLL_MS: u64 = 900;
#[cfg(test)]
const YU_SETTLE_POLL_MS: u64 = 15;
const YU_SETTLE_POLLS: u32 = 8;

/// Fetch-or-reuse the Boros YU market snapshot. Public data — no
/// credentials needed and identical in both environments (Boros has no
/// testnet). Fail-open by design: `None` simply means no fixed-yield
/// variants this scan, never a scan failure.
async fn boros_yu_snapshot(
    state: &AppEngineState,
    allow_network_fetch: bool,
) -> Option<(Arc<Vec<arbitrageur_exchanges::BorosYuMarket>>, f64)> {
    use arbitrageur_exchanges::BOROS_API_BASE;
    const TTL: std::time::Duration = std::time::Duration::from_secs(120);
    let stale = {
        let guard = state.boros_yu_cache.lock();
        guard.as_ref().and_then(|c| {
            if c.fetched_at.elapsed() < TTL {
                Some((c.markets.clone(), c.gas_usd))
            } else {
                None
            }
        })
    };
    if stale.is_some() {
        return stale;
    }
    if !allow_network_fetch {
        // Serve whatever the cache holds — even expired data beats none,
        // and callers treat rates as advisory until execution re-checks.
        return state
            .boros_yu_cache
            .lock()
            .as_ref()
            .map(|c| (c.markets.clone(), c.gas_usd));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .ok()?;
    let markets = arbitrageur_exchanges::fetch_boros_yu_markets(&client, BOROS_API_BASE)
        .await
        .ok()?;
    if markets.is_empty() {
        return None;
    }
    // Gas for the YU batch: the API's per-order estimate covers one
    // place-order tx; two legs plus one enter-markets buffer ≈ 3×.
    let gas_usd = match client
        .get(format!("{BOROS_API_BASE}/v1/gas-price/current"))
        .send()
        .await
    {
        Ok(r) => r
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v.get("estimatedOrderGasCostUsd").and_then(|g| g.as_f64()))
            .map(|g| g * 3.0)
            .unwrap_or(0.15),
        Err(_) => 0.15,
    };
    let arc = Arc::new(markets);
    *state.boros_yu_cache.lock() = Some(crate::state::BorosYuCache {
        fetched_at: std::time::Instant::now(),
        markets: arc.clone(),
        gas_usd,
    });
    Some((arc, gas_usd))
}

/// Build the `fixed_yield` row for a carry pair whose venues both host a
/// Boros YU market on this asset at a shared maturity. Returns `None`
/// (emit no row) when no YU market covers a leg, maturities never
/// coincide, the locked spread is not positive after all four legs'
/// costs, or the lock term is shorter than [`FIXED_MIN_LOCK_HOURS`].
#[allow(clippy::too_many_arguments)]
fn boros_fixed_yield_variant(
    _state: &AppEngineState,
    yu: &Option<(Arc<Vec<arbitrageur_exchanges::BorosYuMarket>>, f64)>,
    symbol: &str,
    long_name: &str,
    short_name: &str,
    long_vid: &str,
    short_vid: &str,
    trade_size_usd: f64,
    perp_rt_fee_bps: f64,
    entry_spread_bps: Option<f64>,
    size_usd: f64,
) -> Option<FundingOpportunityDto> {
    use arbitrageur_exchanges::{boros_platform_venue_id, BorosYuMarket};
    use arbitrageur_math::fixed_yield::{evaluate_fixed_yield_carry, YuHedgeLeg};
    let (markets, gas_usd) = yu.as_ref()?;
    // Canonical symbol base ("BTC-USDC" → "BTC") ↔ Boros `assetSymbol`.
    let asset = symbol.split('-').next()?.trim().to_uppercase();
    if asset.is_empty() {
        return None;
    }
    let markets_of = |vid: &str| -> Vec<&BorosYuMarket> {
        markets
            .iter()
            .filter(|m| {
                boros_platform_venue_id(&m.platform) == Some(vid)
                    && m.asset.eq_ignore_ascii_case(&asset)
                    && m.best_bid_apr.is_some()
                    && m.best_ask_apr.is_some()
            })
            .collect()
    };
    let long_mkts = markets_of(long_vid);
    let short_mkts = markets_of(short_vid);
    if long_mkts.is_empty() || short_mkts.is_empty() {
        return None;
    }
    let now = Utc::now().timestamp();
    let mut best: Option<arbitrageur_math::fixed_yield::FixedYieldOpportunity> = None;
    for lm in &long_mkts {
        for sm in &short_mkts {
            if lm.maturity_unix != sm.maturity_unix {
                continue; // the lock must share one maturity on both legs
            }
            // Long perp leg pays floating → LONG YU pays fixed (executes
            // at the ask-side rate). Short perp leg receives floating →
            // SHORT YU receives fixed (executes at the bid-side rate).
            let yl = YuHedgeLeg {
                venue_id: long_vid.to_string(),
                yu_market_id: lm.market_id,
                yu_symbol: lm.symbol.clone(),
                yu_side: "long".into(),
                fixed_apr: lm.best_ask_apr?,
                taker_fee_rate: lm.taker_fee_rate,
                max_leverage: lm.max_leverage,
                maturity_unix: lm.maturity_unix,
            };
            let ys = YuHedgeLeg {
                venue_id: short_vid.to_string(),
                yu_market_id: sm.market_id,
                yu_symbol: sm.symbol.clone(),
                yu_side: "short".into(),
                fixed_apr: sm.best_bid_apr?,
                taker_fee_rate: sm.taker_fee_rate,
                max_leverage: sm.max_leverage,
                maturity_unix: sm.maturity_unix,
            };
            if let Some(ev) = evaluate_fixed_yield_carry(
                yl,
                ys,
                size_usd,
                perp_rt_fee_bps,
                entry_spread_bps.unwrap_or(0.0),
                *gas_usd,
                now,
                FIXED_MIN_LOCK_HOURS,
            ) {
                let better = best
                    .as_ref()
                    .map(|b| ev.expected_return_usd > b.expected_return_usd)
                    .unwrap_or(true);
                if better {
                    best = Some(ev);
                }
            }
        }
    }
    let ev = best?;
    let mat_ms = ev.lock_until_unix.saturating_mul(1000);
    let mat_date = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(mat_ms)
        .map(|d| d.format("%d %b %Y").to_string())
        .unwrap_or_default();
    Some(FundingOpportunityDto {
        id: format!("{symbol}-{long_name}-{short_name}-FIXED"),
        symbol: symbol.to_string(),
        long_venue: long_name.to_string(),
        short_venue: short_name.to_string(),
        long_venue_id: long_vid.to_string(),
        short_venue_id: short_vid.to_string(),
        long_symbol: symbol.to_string(),
        short_symbol: symbol.to_string(),
        long_market_type: "perpetual".to_string(),
        short_market_type: "perpetual".to_string(),
        net_rate_8h_pct: ev.fixed_apr * 8.0 / 8760.0 * 100.0,
        net_apr_pct: ev.fixed_apr_pct,
        // Fixed yield locks a flat rate — the annual figure is the locked
        // APR itself, which is a real (not compounded) annual yield.
        net_apy_pct: Some(ev.fixed_apr_pct),
        round_trip_fee_bps: perp_rt_fee_bps + ev.yu_hedge_cost_bps,
        break_even_hours: ev.break_even_hours,
        estimated_daily_income_usd: ev.daily_income_usd,
        trade_size_usd,
        optimal_size_usd: size_usd,
        entry_spread_bps: entry_spread_bps.unwrap_or(-1.0),
        kind: "fixed_yield".to_string(),
        basis_bps: None,
        spot_price: None,
        perp_price: None,
        win_probability: None,
        expected_hold_hours: Some(ev.lock_hours),
        hold_reason: Some(format!("Locked via Boros YU to {mat_date}")),
        hold_confidence: None,
        expected_return_usd: Some(ev.expected_return_usd),
        backtest_win_rate: None,
        backtest_win_rate_lower: None,
        backtest_win_rate_upper: None,
        backtest_episodes: None,
        weekday_win_rate: None,
        weekend_win_rate: None,
        persistence_probability: None,
        persistence_probability_lower: None,
        backtest_expected_usd: None,
        backtest_expected_usd_se: None,
        backtest_trials: None,
        backtest_deciles_usd: None,
        depth_slope_bps_per_1k: None,
        long_rate_8h_pct: None,
        short_rate_8h_pct: None,
        depth_curve: Vec::new(),
        optimal_capacity_usd: None,
        max_capacity_usd: None,
        fixed_apr_pct: Some(ev.fixed_apr_pct),
        yu_legs: Some(vec![
            YuLegDto {
                venue_id: ev.long_leg.venue_id.clone(),
                yu_market_id: ev.long_leg.yu_market_id,
                yu_symbol: ev.long_leg.yu_symbol.clone(),
                yu_side: ev.long_leg.yu_side.clone(),
                fixed_apr_pct: ev.long_leg.fixed_apr * 100.0,
                fee_bps: ev.long_leg.taker_fee_rate * 1e4,
                maturity_unix: ev.long_leg.maturity_unix,
            },
            YuLegDto {
                venue_id: ev.short_leg.venue_id.clone(),
                yu_market_id: ev.short_leg.yu_market_id,
                yu_symbol: ev.short_leg.yu_symbol.clone(),
                yu_side: ev.short_leg.yu_side.clone(),
                fixed_apr_pct: ev.short_leg.fixed_apr * 100.0,
                fee_bps: ev.short_leg.taker_fee_rate * 1e4,
                maturity_unix: ev.short_leg.maturity_unix,
            },
        ]),
        yu_hedge_cost_bps: Some(ev.yu_hedge_cost_bps),
        boros_margin_usd: Some(ev.boros_margin_usd),
        boros_gas_usd: Some(*gas_usd),
        lock_until_ms: Some(mat_ms),
    })
}

/// Deterministic carry income per unit over the modelled hold, bps.
/// Prefers the expected-return back-out — `expected_return = income −
/// (fees + entry cross)·size/1e4` — so the curve's income matches the
/// displayed expected return exactly. Falls back to raw accrual
/// `net_8h·hold/8`, else a single funding period: the cost side of the
/// curve stays identical either way, so a conservative income floor never
/// overstates economics.
fn carry_income_bps(
    expected_return_usd: Option<f64>,
    trade_size_usd: f64,
    fee_bps: f64,
    spread_cost_bps: Option<f64>,
    net_8h_pct: f64,
    hold_hours: Option<f64>,
) -> Option<f64> {
    let v = if let Some(e) = expected_return_usd {
        e / trade_size_usd * 10_000.0 + fee_bps + spread_cost_bps.unwrap_or(0.0)
    } else {
        net_8h_pct * 100.0 * (hold_hours.unwrap_or(8.0) / 8.0)
    };
    v.is_finite().then_some(v)
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
    let env = state.env().as_str();
    let long_series = state
        .state_store
        .funding_rate_series(long_venue_id, symbol, from, now_ms, env)
        .unwrap_or_default();
    let short_series = state
        .state_store
        .funding_rate_series(short_venue_id, symbol, from, now_ms, env)
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
pub(crate) fn funding_min_apr(state: &AppEngineState) -> f64 {
    state
        .strategy_params
        .read()
        .get("funding")
        .map(|p| p.funding_min_apr_pct)
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(8.0)
}

/// Round-trip taker-fee estimate for a funding pair from per-leg effective
/// rates: `2 * (long + short)`. Same-venue basis passes the venue rate twice
/// (`4 * venue`). `None` on non-finite/negative inputs or Decimal overflow —
/// callers skip the pair rather than pricing off garbage.
fn funding_round_trip_fee_bps(long_eff_bps: f64, short_eff_bps: f64) -> Option<Decimal> {
    if !(long_eff_bps.is_finite()
        && short_eff_bps.is_finite()
        && long_eff_bps >= 0.0
        && short_eff_bps >= 0.0)
    {
        return None;
    }
    let (long_dec, short_dec) = (
        Decimal::from_f64_retain(long_eff_bps)?,
        Decimal::from_f64_retain(short_eff_bps)?,
    );
    long_dec.checked_add(short_dec)?.checked_mul(dec!(2))
}

type PerpBooks = std::collections::HashMap<String, (Vec<(f64, f64)>, Vec<(f64, f64)>)>;

/// 8h-normalized funding print (percent) for one leg of a scan pair.
///
/// Matches the scan `rates` row by venue id string and normalizes
/// `rate / interval_hours * 8 * 100`. `None` when the venue has no print
/// or the value is not finite — callers render "—", never fabricate.
pub(crate) fn scan_leg_rate_8h_pct(
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
        let _ =
            state
                .state_store
                .record_funding_rate_samples(now_ms, &samples, state.env().as_str());
    }

    // Cross-exchange carry needs >= 2 funding prints; same-exchange basis
    // needs only one venue with funding + both books, so no early return here.
    let mut opps = Vec::new();

    // Boros YU snapshot for fixed-yield variants — public data, fetched
    // once per scan behind a 120s cache. `None` disables the variant
    // silently (fixed-yield rows are additive, never blocking).
    let yu_snapshot = boros_yu_snapshot(state, allow_network_fetch).await;

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

    // Symbol-collision guard: two venues quoting irreconcilable mids for one
    // canonical symbol are different instruments (index perp vs same-named
    // memecoin) — a carry pair across them is NOT hedged, so refuse it
    // rather than fabricate a delta-neutral position. Only evaluated when
    // both legs' books exist; missing books fall through ungated.
    type BookSides = (Vec<(f64, f64)>, Vec<(f64, f64)>);
    let top_mid = |books: Option<&BookSides>| -> Option<f64> {
        let (asks, bids) = books?;
        let a = asks.first()?.0;
        let b = bids.first()?.0;
        (a.is_finite() && b.is_finite() && a > 0.0 && b > 0.0).then_some((a + b) / 2.0)
    };

    for i in 0..rates.len() {
        for j in (i + 1)..rates.len() {
            let (id_a, _name_a, ref r_a) = rates[i];
            let (id_b, _name_b, ref r_b) = rates[j];

            if let (Some(ma), Some(mb)) =
                (top_mid(perp_books.get(id_a)), top_mid(perp_books.get(id_b)))
            {
                if (ma / mb).max(mb / ma) > 2.0 {
                    tracing::warn!(
                        "funding: refusing {symbol} pair {id_a}/{id_b} — mid mismatch {ma} vs {mb} (symbol collision)"
                    );
                    continue;
                }
            }

            let Some(fee_total) = funding_round_trip_fee_bps(
                effective_venue_fee_bps(state, id_a),
                effective_venue_fee_bps(state, id_b),
            ) else {
                continue;
            };
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
                let net_8h_f: f64 = opp.net_rate_8h.to_string().parse::<f64>().unwrap_or(0.0);
                let fee_bps_f: f64 = opp
                    .round_trip_fee_bps
                    .to_string()
                    .parse::<f64>()
                    .unwrap_or(0.0);
                let spread_cost_f: Option<f64> =
                    depth.map(|(_, e)| e).filter(|v| v.is_finite() && *v > 0.0);
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
                let hist =
                    carry_gap_history(state, symbol, long_vid_hold, short_vid_hold, now_hold_ms);
                let hold_estimate = {
                    let hourly =
                        |rate: Decimal, interval: u32| rate / Decimal::from(interval.max(1));
                    let agrees_hold = match (r_a.predicted_rate, r_b.predicted_rate) {
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
                let bt = backtest_stats(
                    &hist,
                    funding_min_apr(state),
                    fee_bps_f,
                    spread_cost_f,
                    trade_size_usd,
                );
                // Measured persistence: of historical prints above the
                // entry threshold, the share whose gap stayed above it
                // for the full fee+spread breakeven horizon.
                let persistence = hold_estimate.as_ref().and_then(|h| {
                    arbitrageur_math::funding_carry::gap_persistence_probability(
                        &hist,
                        funding_min_apr(state).max(0.0) / 100.0 / 365.0 / 3.0,
                        h.breakeven_hours,
                        20,
                    )
                });
                // Canonical leg identity: display strings never serve as
                // adapter keys downstream; resolve both legs to venue IDs.
                let vid_for = |name: &str| -> Option<&str> {
                    rates
                        .iter()
                        .find(|(_, _, r)| r.venue.as_str() == name)
                        .map(|(vid, _, _)| *vid)
                };
                let (Some(long_vid), Some(short_vid)) =
                    (vid_for(&opp.long_venue), vid_for(&opp.short_venue))
                else {
                    continue;
                };
                // Informational only: the edge direction (which leg is
                // short) is set by current hourly rates; agreement holds
                // when both venues' PREDICTED hourly rates point the same
                // way. Absent predictions -> None (no adjustment).
                let win_probability = {
                    let hourly =
                        |rate: Decimal, interval: u32| rate / Decimal::from(interval.max(1));
                    let agrees = match (r_a.predicted_rate, r_b.predicted_rate) {
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
                    funding_win_probability(
                        opp.net_rate_8h,
                        opp.round_trip_fee_bps,
                        None,
                        agrees,
                        hold_estimate.as_ref().map(|h| WinHoldContext {
                            hold_hours: h.hours,
                            min_apr_pct: Some(funding_min_apr(state)),
                            entry_spread_bps: spread_cost_f,
                        }),
                    )
                };
                let expected_return_usd = expected_carry_return_usd(
                    net_8h_f,
                    fee_bps_f,
                    spread_cost_f,
                    hold_estimate.as_ref().map(|h| h.hours),
                    Some(funding_min_apr(state)),
                    trade_size_usd,
                );
                // Capacity curve: long leg lifts asks, short leg hits bids.
                let depth_curve: Vec<crate::monitor::DepthCurvePointDto> = match (
                    perp_books.get(long_vid),
                    perp_books.get(short_vid),
                    carry_income_bps(
                        expected_return_usd,
                        trade_size_usd,
                        fee_bps_f,
                        spread_cost_f,
                        net_8h_f,
                        hold_estimate.as_ref().map(|h| h.hours),
                    ),
                ) {
                    (Some((long_asks, _)), Some((_, short_bids)), Some(inc)) => {
                        arbitrageur_math::funding_carry::funding_depth_curve(
                            long_asks,
                            short_bids,
                            inc,
                            fee_bps_f,
                            win_probability,
                        )
                        .iter()
                        .map(crate::monitor::DepthCurvePointDto::from)
                        .collect()
                    }
                    _ => Vec::new(),
                };
                let (optimal_capacity_usd, max_capacity_usd) = capacity_bounds(&depth_curve);
                // Fixed-yield variant: when both legs' venues have a
                // live Boros YU market on this asset sharing one
                // maturity, the floating carry can be locked to a fixed
                // rate by two YU legs (long YU on the long venue, short
                // YU on the short venue). Emitted as a SEPARATE row so
                // the autobot can weigh 2-leg vs 4-leg economics.
                let fixed_variant = boros_fixed_yield_variant(
                    state,
                    &yu_snapshot,
                    symbol,
                    &opp.long_venue,
                    &opp.short_venue,
                    long_vid,
                    short_vid,
                    trade_size_usd,
                    fee_bps_f,
                    spread_cost_f,
                    depth.map(|(o, _)| o).unwrap_or(trade_size_usd),
                );
                opps.push(FundingOpportunityDto {
                    id: format!("{}-{}-{}", symbol, opp.long_venue, opp.short_venue),
                    symbol: symbol.to_string(),
                    long_venue: opp.long_venue,
                    short_venue: opp.short_venue,
                    long_venue_id: long_vid.to_string(),
                    short_venue_id: short_vid.to_string(),
                    long_symbol: symbol.to_string(),
                    short_symbol: symbol.to_string(),
                    long_market_type: "perpetual".to_string(),
                    short_market_type: "perpetual".to_string(),
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
                        .and_then(|a| a.to_string().parse::<f64>().ok())
                        .map(|v| v * 100.0),
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
                    win_probability,
                    expected_hold_hours: hold_estimate.as_ref().map(|h| h.hours),
                    hold_reason: hold_estimate.as_ref().map(|h| h.reason.clone()),
                    hold_confidence: hold_estimate.as_ref().map(|h| h.confidence),
                    expected_return_usd,
                    depth_curve,
                    optimal_capacity_usd,
                    max_capacity_usd,
                    backtest_win_rate: bt.as_ref().map(|b| b.win_rate),
                    backtest_win_rate_lower: bt.as_ref().and_then(|b| b.win_rate_lower),
                    backtest_win_rate_upper: bt.as_ref().and_then(|b| b.win_rate_upper),
                    backtest_episodes: bt.as_ref().map(|b| b.episodes),
                    weekday_win_rate: bt.as_ref().and_then(|b| b.weekday_win_rate),
                    weekend_win_rate: bt.as_ref().and_then(|b| b.weekend_win_rate),
                    persistence_probability: persistence.and_then(|p| p.rate),
                    persistence_probability_lower: persistence.and_then(|p| p.lower),
                    backtest_expected_usd: bt.as_ref().map(|b| b.expected_usd),
                    backtest_expected_usd_se: bt.as_ref().and_then(|b| b.expected_usd_se),
                    backtest_trials: bt.as_ref().map(|b| b.trials),
                    backtest_deciles_usd: bt.as_ref().map(|b| b.deciles_usd.clone()),
                    fixed_apr_pct: None,
                    yu_legs: None,
                    yu_hedge_cost_bps: None,
                    boros_margin_usd: None,
                    boros_gas_usd: None,
                    lock_until_ms: None,
                });
                if let Some(fixed) = fixed_variant {
                    opps.push(fixed);
                }
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
                    let eff = effective_venue_fee_bps(state, vid);
                    let Some(fee_total) = funding_round_trip_fee_bps(eff, eff) else {
                        continue;
                    };
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
                    let slope =
                        entry_gradient(&spot_asks, &perp_bids, size_f).map(|g| g.slope_bps_per_1k);
                    let rep = funding_entry_capacity(
                        &spot_asks,
                        &perp_bids,
                        &[size_f * 0.5, size_f, size_f * 2.0],
                    );
                    let depth = rep
                        .optimal_size_usd
                        .zip(rep.entry_spread_bps.map(|e| (-e).max(0.0)));
                    let venue_label = funding_rate.venue.to_string();
                    let spot_mid_f = spot_mid.to_string().parse::<f64>().unwrap_or(0.0);
                    let perp_mid_f = perp_mid.to_string().parse::<f64>().unwrap_or(0.0);
                    // Expected hold for basis: single-venue perp funding history
                    // (spot pays none). Same decay-to-threshold model as carry.
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
                            state.env().as_str(),
                        )
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|(ts, r, _)| r.is_finite().then_some((ts, r)))
                        .collect();
                    let basis_hold = {
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
                    let bt = backtest_stats(
                        &hist_basis,
                        funding_min_apr(state),
                        fee_bps_f,
                        spread_cost_f,
                        trade_size_usd,
                    );
                    let persistence = basis_hold.as_ref().and_then(|h| {
                        arbitrageur_math::funding_carry::gap_persistence_probability(
                            &hist_basis,
                            funding_min_apr(state).max(0.0) / 100.0 / 365.0 / 3.0,
                            h.breakeven_hours,
                            20,
                        )
                    });
                    let Some(spot_exact) = spot_key_used.clone() else {
                        continue;
                    };
                    // Informational only: positive entry basis is an
                    // expected exit tailwind (convergence), and a
                    // positive predicted perp rate confirms the carry.
                    let win_probability = funding_win_probability(
                        eval.carry.net_rate_8h,
                        eval.carry.round_trip_fee_bps,
                        if eval.basis_bps > dec!(0) {
                            Some(eval.basis_bps)
                        } else {
                            None
                        },
                        funding_rate.predicted_rate.map(|pr| pr > dec!(0)),
                        basis_hold.as_ref().map(|h| WinHoldContext {
                            hold_hours: h.hours,
                            min_apr_pct: Some(funding_min_apr(state)),
                            entry_spread_bps: spread_cost_f,
                        }),
                    );
                    let expected_return_usd = expected_carry_return_usd(
                        net_8h_f,
                        fee_bps_f,
                        spread_cost_f,
                        basis_hold.as_ref().map(|h| h.hours),
                        Some(funding_min_apr(state)),
                        trade_size_usd,
                    );
                    // Capacity curve: long leg lifts spot asks, short leg
                    // hits perp bids.
                    let depth_curve: Vec<crate::monitor::DepthCurvePointDto> =
                        match carry_income_bps(
                            expected_return_usd,
                            trade_size_usd,
                            fee_bps_f,
                            spread_cost_f,
                            net_8h_f,
                            basis_hold.as_ref().map(|h| h.hours),
                        ) {
                            Some(inc) => arbitrageur_math::funding_carry::funding_depth_curve(
                                &spot_asks,
                                &perp_bids,
                                inc,
                                fee_bps_f,
                                win_probability,
                            )
                            .iter()
                            .map(crate::monitor::DepthCurvePointDto::from)
                            .collect(),
                            None => Vec::new(),
                        };
                    let (optimal_capacity_usd, max_capacity_usd) = capacity_bounds(&depth_curve);
                    opps.push(FundingOpportunityDto {
                        id: format!("{symbol}-{venue_label}-BASIS"),
                        symbol: symbol.to_string(),
                        long_venue: format!("{venue_label} SPOT"),
                        short_venue: format!("{venue_label} PERP"),
                        long_venue_id: vid.to_string(),
                        short_venue_id: vid.to_string(),
                        long_symbol: spot_exact,
                        short_symbol: symbol.to_string(),
                        long_market_type: "spot".to_string(),
                        short_market_type: "perpetual".to_string(),
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
                            .and_then(|a| a.to_string().parse::<f64>().ok())
                            .map(|v| v * 100.0),
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
                            let v: f64 = funding_rate.rate.to_string().parse().unwrap_or(f64::NAN);
                            let pct =
                                v / f64::from(funding_rate.interval_hours.max(1)) * 8.0 * 100.0;
                            pct.is_finite().then_some(pct)
                        },
                        win_probability,
                        expected_hold_hours: basis_hold.as_ref().map(|h| h.hours),
                        hold_reason: basis_hold.as_ref().map(|h| h.reason.clone()),
                        hold_confidence: basis_hold.as_ref().map(|h| h.confidence),
                        expected_return_usd,
                        depth_curve,
                        optimal_capacity_usd,
                        max_capacity_usd,
                        backtest_win_rate: bt.as_ref().map(|b| b.win_rate),
                        backtest_win_rate_lower: bt.as_ref().and_then(|b| b.win_rate_lower),
                        backtest_win_rate_upper: bt.as_ref().and_then(|b| b.win_rate_upper),
                        backtest_episodes: bt.as_ref().map(|b| b.episodes),
                        weekday_win_rate: bt.as_ref().and_then(|b| b.weekday_win_rate),
                        weekend_win_rate: bt.as_ref().and_then(|b| b.weekend_win_rate),
                        persistence_probability: persistence.and_then(|p| p.rate),
                        persistence_probability_lower: persistence.and_then(|p| p.lower),
                        backtest_expected_usd: bt.as_ref().map(|b| b.expected_usd),
                        backtest_expected_usd_se: bt.as_ref().and_then(|b| b.expected_usd_se),
                        backtest_trials: bt.as_ref().map(|b| b.trials),
                        backtest_deciles_usd: bt.as_ref().map(|b| b.deciles_usd.clone()),
                        fixed_apr_pct: None,
                        yu_legs: None,
                        yu_hedge_cost_bps: None,
                        boros_margin_usd: None,
                        boros_gas_usd: None,
                        lock_until_ms: None,
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
/// the current prints only; gaps may be carried forward (a funding print is
/// a standing rate until superseded) but never synthesized.
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
pub(crate) fn downsample_series(series: &[(i64, f64)], max: usize) -> Vec<FundingTrendPoint> {
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
pub(crate) fn trend_bare_venue(leg: &str) -> &str {
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
            .funding_rate_series(venue, symbol, from_ms, now_ms, state.env().as_str())
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

    // Ticker mid history: median mid per minute bucket across venues —
    // venue-neutral AND robust to a residual outlier surviving the store's
    // consistency filter (a mean lets one bad mid move the whole line).
    let mut buckets: HashMap<i64, Vec<f64>> = HashMap::new();
    if let Ok(samples) = state.state_store.spread_samples_mode(
        symbol,
        state.usd_merge_quotes(),
        from_ms,
        now_ms,
        state.env().as_str(),
    ) {
        for s in samples {
            if !s.mid.is_finite() || s.mid <= 0.0 || s.ts_ms < 0 {
                continue;
            }
            buckets
                .entry(s.ts_ms / 60_000 * 60_000)
                .or_default()
                .push(s.mid);
        }
    }
    let mut price_hist: Vec<(i64, f64)> = buckets
        .into_iter()
        .filter_map(|(b, mut ms)| {
            ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let n = ms.len();
            let m = if n % 2 == 1 {
                ms[n / 2]
            } else {
                (ms[n / 2 - 1] + ms[n / 2]) / 2.0
            };
            (m.is_finite() && m > 0.0).then_some((b, m))
        })
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
        let sym_opps =
            scan_funding_opportunities_for_symbol(sym, size_dec, trade_size_usd, false, state)
                .await;
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
        let mut symbol_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
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

    let mut opps =
        scan_funding_opportunities_for_symbol(symbol, size_dec, trade_size_usd, true, state).await;
    opps.sort_by(|a, b| {
        b.net_apy_pct
            .partial_cmp(&a.net_apy_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(opps)
}
/// Fixed-yield maturity sweep (issue #25). Runs on the monitor cadence —
/// live env only, Boros has no testnet.
///
/// State machine per unresolved `-yu` maturity row:
///   locked  — before `lock_until_ms`; wall-clock only, no venue call.
///   due     — past `lock_until_ms`; venue truth has not yet confirmed
///             settlement. Every pass re-reads the Boros account.
///   settled — the account read shows no residual position on any tracked
///             market id. Only now may the operator resolve.
///   settlement_unverified — a position still shows after maturity, or
///             the previous read failed; the next pass retries.
///
/// Wall-clock expiry alone NEVER marks settled — a Boros position that
/// survived maturity is still live exposure. Read failures keep the row
/// unresolved (fail-closed) and record the error as the observation.
pub async fn check_fixed_yield_maturities(state: &AppEngineState) {
    if !state.is_live() {
        return;
    }
    let rows = match state.state_store.list_fixed_yield_maturities() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("fixed-yield maturity scan failed: {e}");
            return;
        }
    };
    let now_ms = Utc::now().timestamp_millis();
    let pending: Vec<_> = rows
        .into_iter()
        .filter(|r| r.resolved_action.is_none())
        .filter(|r| r.maturity_state != "settled" && r.maturity_state != "closed_early")
        .collect();
    if pending.is_empty() {
        return;
    }

    for row in pending {
        if row.maturity_state == "locked" && now_ms < row.lock_until_ms {
            continue;
        }
        if row.maturity_state == "locked" {
            let _ = state.state_store.update_fixed_yield_maturity_state(
                &row.signal_id,
                "due",
                Some("lock_until reached — awaiting venue-confirmed settlement"),
            );
        }
        // Venue truth: a residual position on any tracked market id (or a
        // leg symbol still open) means NOT settled. Positions on matured
        // markets report as `BOROS-{marketId}` once the market delists
        // from the active feed — match both.
        let acct = match boros_for_exec(state).fetch_account().await {
            Ok(a) => a,
            Err(e) => {
                let _ = state.state_store.update_fixed_yield_maturity_state(
                    &row.signal_id,
                    "settlement_unverified",
                    Some(&format!("venue read failed: {e}")),
                );
                continue;
            }
        };
        let leg_symbols: Vec<String> = state
            .state_store
            .get_signal(&row.signal_id)
            .ok()
            .flatten()
            .map(|s| {
                s.legs
                    .iter()
                    .map(|l| l.symbol.clone())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let open: Vec<String> = acct
            .positions
            .iter()
            .filter(|p| {
                row.yu_market_ids
                    .iter()
                    .any(|mid| p.symbol == format!("BOROS-{mid}"))
                    || leg_symbols.iter().any(|s| s == &p.symbol)
            })
            .map(|p| p.symbol.clone())
            .collect();
        if open.is_empty() {
            let _ = state.state_store.update_fixed_yield_maturity_state(
                &row.signal_id,
                "settled",
                Some("venue read: no residual YU position — settlement confirmed"),
            );
        } else {
            let _ = state.state_store.update_fixed_yield_maturity_state(
                &row.signal_id,
                "settlement_unverified",
                Some(&format!(
                    "venue still shows position(s) post-maturity: {}",
                    open.join(", ")
                )),
            );
        }
    }
}

/// Autonomous fixed-yield admission (issue #25). Runs only after the
/// rescan validated the four-leg row — every check hits live venue truth
/// or the refreshed DTO, and every refusal names its remediation. The
/// setting is opt-in: absent `auto_fixed_yield_enabled`, `auto` fixed
/// entries refuse like every other autonomous live entry.
async fn fixed_yield_auto_gate(
    state: &AppEngineState,
    opp: &FundingOpportunityDto,
    yu_legs: &[YuLegDto],
    trade_size_usd: f64,
) -> Result<(), String> {
    let s = state.settings.read().clone();
    if !s.auto_fixed_yield_enabled {
        return Err(
            "autonomous fixed-yield entries are disabled — enable 'Auto fixed-yield entries' in System Settings"
                .into(),
        );
    }
    // Post-cost edge must exist — the scan's expected_return_usd is net
    // of entry costs and YU taker fees.
    let ret = opp.expected_return_usd.unwrap_or(0.0);
    if !ret.is_finite() || ret <= 0.0 {
        return Err(format!(
            "fixed-yield expected return ${ret:.2} is not positive post-costs"
        ));
    }
    // Lock term: a sub-minimum lock's costs dominate the carry.
    let now_ms = Utc::now().timestamp_millis();
    let lock_ms = opp
        .lock_until_ms
        .ok_or_else(|| "fixed-yield row carries no lock_until".to_string())?;
    let lock_days = (lock_ms - now_ms) as f64 / 86_400_000.0;
    if lock_days < s.auto_fixed_yield_min_lock_days as f64 {
        return Err(format!(
            "fixed-yield lock term {lock_days:.1}d is under the {}d minimum",
            s.auto_fixed_yield_min_lock_days
        ));
    }
    let boros = boros_for_exec(state);
    // Agent approval must outlive the entry — entering under an approval
    // that dies mid-lock strands the unwind path. One hour of buffer is
    // the minimum sane margin for an entry that polls over seconds.
    let expiry = boros
        .agent_expiry_unix()
        .await
        .map_err(|e| format!("boros agent approval unreadable: {e}"))?
        .ok_or_else(|| {
            "boros agent approval missing — approve the agent key in the Boros app".to_string()
        })?;
    let now_s = Utc::now().timestamp().max(0) as u64;
    if expiry <= now_s {
        return Err("boros agent approval EXPIRED — renew it in the Boros app".into());
    }
    if expiry - now_s < 3600 {
        return Err(
            "boros agent approval expires within the hour — renew before auto entry".into(),
        );
    }
    // Gas treasury: Boros sponsors tx gas from it — an empty treasury
    // cannot submit the YU legs at all.
    let gas = boros
        .gas_treasury_usd()
        .await
        .map_err(|e| format!("boros gas treasury unreadable: {e}"))?
        .ok_or_else(|| {
            "boros gas treasury balance unknown — the venue did not report one".to_string()
        })?;
    if gas < s.auto_fixed_yield_min_gas_usd {
        return Err(format!(
            "boros gas treasury ${gas:.2} is below the ${:.2} minimum for auto entry",
            s.auto_fixed_yield_min_gas_usd
        ));
    }
    // Margin: free margin must cover both YU legs' initial margin. The
    // estimate is notional / market max-leverage per leg — deliberately
    // conservative (cross-margin offsets are ignored).
    let acct = boros
        .fetch_account()
        .await
        .map_err(|e| format!("boros account read failed: {e}"))?;
    let free_usd: f64 = acct.free_margin.to_string().parse().unwrap_or(0.0);
    let mut required = 0.0f64;
    for leg in yu_legs {
        let m = boros
            .market_by_id(leg.yu_market_id)
            .await
            .map_err(|e| format!("YU market {} unresolved: {e}", leg.yu_market_id))?;
        required += trade_size_usd / m.max_leverage.max(1.0);
    }
    if free_usd < required {
        return Err(format!(
            "boros free margin ${free_usd:.2} under estimated ${required:.2} requirement"
        ));
    }
    Ok(())
}

pub async fn execute_funding_carry_impl(
    symbol: &str,
    long_venue: &str,
    short_venue: &str,
    trade_size_usd: f64,
    state: &AppEngineState,
    auto: bool,
    kind: Option<&str>,
) -> Result<SimStepResultDto, String> {
    // Autonomous entries stay testnet-only in live mode (same policy as
    // the radar bot's evaluate_entry_gates hard-block); operator-directed
    // manual entries are the live path. The one exception is the
    // fixed-yield kind behind its explicit opt-in — gated below after the
    // rescan, where the refreshed DTO's lock/return fields exist.
    if auto && state.is_live() && kind != Some("fixed_yield") {
        return Err(
            "autonomous funding entries are testnet-only — run a manual Execute for live carry"
                .into(),
        );
    }
    // Auto fixed-yield is opt-in: refuse fast (before any scan) when the
    // operator never enabled it — the full venue-side gate runs later.
    if auto
        && state.is_live()
        && kind == Some("fixed_yield")
        && !state.settings.read().auto_fixed_yield_enabled
    {
        return Err(
            "autonomous fixed-yield entries are disabled — enable 'Auto fixed-yield entries' in System Settings"
                .into(),
        );
    }
    // The 4-leg fixed-yield variant executes real Boros YU legs on
    // Arbitrum mainnet — Boros runs no testnet, and mixing testnet perps
    // with mainnet hedges is incoherent, so the whole kind is live-only.
    if kind == Some("fixed_yield") && !state.is_live() {
        return Err("fixed-yield entries are live-only — Pendle Boros has no testnet".into());
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
    if symbol.trim().is_empty() || symbol.len() > 64 {
        return Err("funding symbol must be 1-64 characters".into());
    }
    if long_venue.trim().is_empty() || short_venue.trim().is_empty() {
        return Err("funding long/short venues must be specified".into());
    }
    if !trade_size_usd.is_finite() || trade_size_usd <= 0.0 {
        return Err("trade_size_usd must be a finite positive number".into());
    }
    let is_live = state.is_live();
    if is_live {
        // Funding entries reuse the same two-leg coordinator + durable
        // signal reconciliation as the spatial path, under the same live
        // gate: unlocked vault + adapter submission/polling/entry
        // eligibility + live credentials. A fixed-yield entry adds Boros —
        // the YU hedge is an order-bearing leg like any other.
        if kind == Some("fixed_yield") {
            live_entry_gate(state, &[long_venue, short_venue, "boros"])?;
        } else {
            live_entry_gate(state, &[long_venue, short_venue])?;
        }
    }

    // The caller-supplied triple is never trusted — rescan and match it
    // against freshly resolved prints (OLD display triple), then use ONLY
    // the refreshed canonical/exact DTO fields.
    let opps = scan_funding_opportunities_impl(symbol, trade_size_usd, state).await?;
    let opp = opps
        .into_iter()
        .find(|o| {
            o.symbol == symbol
                && o.long_venue == long_venue
                && o.short_venue == short_venue
                && match kind {
                    // Explicit kind selects that exact row — a fixed_yield
                    // request must never silently degrade to the 2-leg row.
                    Some(k) => o.kind == k,
                    // Legacy callers (no kind) must never pick up the 4-leg
                    // row — its cost/risk profile is different.
                    None => o.kind != "fixed_yield",
                }
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

    // Exact leg identity from the refreshed DTO — display labels are never
    // adapter IDs. Carry: distinct canonical venues, both perpetual, one
    // exact perp symbol. Basis: one canonical venue, long spot / short perp,
    // two distinct exact symbols. Anything else fails closed.
    let is_basis = opp.kind == "basis";
    let is_fixed = opp.kind == "fixed_yield";
    // Fixed-yield rows carry the two Boros YU legs inside the DTO —
    // validate them hard before any order goes out. A malformed or stale
    // hedge spec fails closed: the alternative is an unhedged carry the
    // operator did not ask for.
    let yu_legs: Vec<YuLegDto> = if is_fixed {
        let legs = opp.yu_legs.clone().unwrap_or_default();
        if legs.len() != 2 {
            return Err(format!(
                "fixed-yield opportunity carries {} YU legs, expected 2",
                legs.len()
            ));
        }
        let now = Utc::now().timestamp();
        if legs[0].maturity_unix != legs[1].maturity_unix {
            return Err("fixed-yield YU legs must share one maturity — refusing entry".into());
        }
        for leg in &legs {
            if leg.yu_market_id == 0
                || !leg.fixed_apr_pct.is_finite()
                || leg.fixed_apr_pct <= 0.0
                || leg.maturity_unix <= now
            {
                return Err(format!(
                    "fixed-yield YU leg {} is malformed or matured — refusing entry",
                    leg.yu_symbol
                ));
            }
            let (want_venue, want_side) = if leg.yu_side == "long" {
                (opp.long_venue_id.as_str(), "long")
            } else {
                (opp.short_venue_id.as_str(), "short")
            };
            if leg.venue_id != want_venue || (leg.yu_side != "long" && leg.yu_side != "short") {
                return Err(format!(
                    "fixed-yield YU leg {want_side} does not match its perp leg venue — refusing entry"
                ));
            }
        }
        legs
    } else {
        Vec::new()
    };
    // Autonomous fixed-yield admission (issue #25): the opt-in setting
    // plus venue-side gates — agent approval validity, gas treasury,
    // Boros margin, minimum lock term, positive post-cost return — all
    // checked against LIVE reads after the rescan. Every refusal names
    // its reason so the UI/log can show what to remediate.
    if auto && is_fixed {
        fixed_yield_auto_gate(state, &opp, &yu_legs, trade_size_usd).await?;
    }
    let (long_vid, short_vid) = (opp.long_venue_id.clone(), opp.short_venue_id.clone());
    let (long_sym, short_sym) = (opp.long_symbol.clone(), opp.short_symbol.clone());
    if long_vid.trim().is_empty() || short_vid.trim().is_empty() {
        return Err("funding legs carry no canonical venue IDs".into());
    }
    if long_sym.trim().is_empty() || short_sym.trim().is_empty() {
        return Err("funding legs carry no exact symbols".into());
    }
    if is_basis {
        if long_vid != short_vid {
            return Err(format!(
                "funding basis legs must share one venue, got {long_vid} vs {short_vid}"
            ));
        }
        if opp.long_market_type != "spot" || opp.short_market_type != "perpetual" {
            return Err(format!(
                "funding basis legs must be spot/perpetual, got {}/{}",
                opp.long_market_type, opp.short_market_type
            ));
        }
        if long_sym == short_sym {
            return Err("funding basis legs must use distinct exact symbols".into());
        }
    } else {
        if long_vid == short_vid {
            return Err("funding carry legs must resolve to distinct canonical venues".into());
        }
        if opp.long_market_type != "perpetual" || opp.short_market_type != "perpetual" {
            return Err(format!(
                "funding carry legs must both be perpetual, got {}/{}",
                opp.long_market_type, opp.short_market_type
            ));
        }
        if long_sym != short_sym {
            return Err("funding carry legs must share one exact perp symbol".into());
        }
    }
    let long_canon = crate::state::canonical_venue_id(&long_vid)
        .ok_or_else(|| format!("funding long leg venue '{long_vid}' is not admitted"))?;
    let short_canon = crate::state::canonical_venue_id(&short_vid)
        .ok_or_else(|| format!("funding short leg venue '{short_vid}' is not admitted"))?;
    // Depeg gate per leg quote — a book priced in a halted stablecoin is
    // untradeable; stable-vs-USD legs stay exempt (they ARE the depeg arb).
    let long_quote = state.book_quote(long_canon, &long_sym);
    let short_quote = state.book_quote(short_canon, &short_sym);
    state
        .stablecoin_gate(&long_sym, &long_quote)
        .map_err(|e| format!("{long_canon}: {e}"))?;
    state
        .stablecoin_gate(&short_sym, &short_quote)
        .map_err(|e| format!("{short_canon}: {e}"))?;

    // Root symbol for the persisted signal family: the requested perp the
    // caller scanned. Flatten scopes funding to this root and closes the
    // exact spot + perp legs beneath it.
    let root_symbol = symbol.to_string();

    let long_adapter = state
        .get_adapter(long_canon)
        .ok_or_else(|| format!("Venue '{long_canon}' is not admitted for execution"))?;
    let short_adapter = state
        .get_adapter(short_canon)
        .ok_or_else(|| format!("Venue '{short_canon}' is not admitted for execution"))?;

    let _execution_guard = state.execution_lock.lock().await;
    // Re-run the live gate under the lock after the rescan — environment,
    // credentials, or adapter admission may have changed between the
    // pre-flight gate above and dispatch (TOCTOU). The early gate stays as
    // a fast-fail; this one is authoritative.
    if state.is_live() {
        if is_fixed {
            live_entry_gate(state, &[long_venue, short_venue, "boros"])?;
        } else {
            live_entry_gate(state, &[long_venue, short_venue])?;
        }
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
    // Dispatch-time books per EXACT leg symbol: hub-first (120s), else a
    // fresh read-only fetch through the admitted adapter. Either leg missing
    // or invalid fails the dispatch — never priced off a half book.
    let resolve_book = async |adapter: &Arc<dyn ExchangeAdapter>,
                              exact_symbol: &str|
           -> Result<OrderBook, String> {
        let vid = adapter.venue_id();
        if let Some(book) = state.market_data_hub.get_valid_orderbook(
            &vid,
            exact_symbol,
            Some(std::time::Duration::from_secs(120)),
        ) {
            if validate_adapter_book(&book, &vid, exact_symbol).is_ok() {
                return Ok(book);
            }
        }
        let book = adapter
            .fetch_orderbook(exact_symbol)
            .await
            .map_err(|e| format!("funding leg {exact_symbol}@{vid} book unavailable: {e}"))?;
        validate_adapter_book(&book, &vid, exact_symbol)
            .map_err(|e| format!("funding leg {exact_symbol}@{vid} book invalid: {e}"))?;
        state.market_data_hub.update_orderbook(book.clone());
        Ok(book)
    };
    let long_book = resolve_book(&long_adapter, &long_sym).await?;
    let short_book = resolve_book(&short_adapter, &short_sym).await?;
    let long_ask = long_book
        .best_ask_price()
        .filter(|p| *p > dec!(0))
        .ok_or_else(|| format!("no long ask for {long_sym} on {long_canon}"))?;
    let short_bid = short_book
        .best_bid_price()
        .filter(|p| *p > dec!(0))
        .ok_or_else(|| format!("no short bid for {short_sym} on {short_canon}"))?;
    let long_ask_f = dec_to_f64(long_ask, "funding long ask")?;
    let short_bid_f = dec_to_f64(short_bid, "funding short bid")?;
    if !(long_ask_f.is_finite() && long_ask_f > 0.0 && short_bid_f.is_finite() && short_bid_f > 0.0)
    {
        return Err("funding dispatch books are not priced".into());
    }

    // Equal base quantity from the requested USD notional at the long ask,
    // converting through the quote coin's USD rate (fx_adjusted mode;
    // parity/separate read the factor as 1.0).
    let qty_f64 =
        trade_size_usd / (long_ask_f * state.fx_price_factor(Some(long_canon), &long_quote));
    if !(qty_f64.is_finite() && qty_f64 > 0.0) {
        return Err("funding carry quantity is not positive".into());
    }
    let qty = Decimal::from_str_exact(&qty_f64.to_string())
        .ok()
        .filter(|q| *q > dec!(0))
        .ok_or_else(|| "funding carry quantity is not positive".to_string())?;
    // Delta-neutral means identical base quantity on both legs — snap onto
    // the finer common wire grid so per-venue lot/contract normalization
    // cannot desync the hedge.
    let qty =
        crate::execution::common_wire_qty(&root_symbol, qty, &long_adapter, &short_adapter).await?;
    let qty_f64: f64 = qty
        .to_string()
        .parse()
        .map_err(|_| "snapped funding quantity is not finite".to_string())?;

    let max_slippage_bps = strategy_slippage_bps(state, "funding");
    let max_slip_dec = Decimal::from_f64_retain(max_slippage_bps)
        .ok_or_else(|| "Invalid funding max slippage setting".to_string())?;
    arbitrageur_risk::safety_guards::verify_pre_execution_slippage(
        &long_book,
        Side::Buy,
        qty,
        max_slip_dec,
    )
    .map_err(|v| {
        format!(
            "funding long-leg slippage check failed on {long_sym}@{long_canon}: VWAP {} vs top {} ({} bps, limit {} bps)",
            v.effective_vwap, v.top_price, v.slippage_bps, v.max_allowed_bps
        )
    })?;
    arbitrageur_risk::safety_guards::verify_pre_execution_slippage(
        &short_book,
        Side::Sell,
        qty,
        max_slip_dec,
    )
    .map_err(|v| {
        format!(
            "funding short-leg slippage check failed on {short_sym}@{short_canon}: VWAP {} vs top {} ({} bps, limit {} bps)",
            v.effective_vwap, v.top_price, v.slippage_bps, v.max_allowed_bps
        )
    })?;
    let (long_vwap, long_remaining, _) = long_book
        .buy_execution_vwap(qty)
        .ok_or_else(|| format!("No executable buy depth for {long_sym} on {long_canon}"))?;
    let (short_vwap, short_remaining, _) = short_book
        .sell_execution_vwap(qty)
        .ok_or_else(|| format!("No executable sell depth for {short_sym} on {short_canon}"))?;
    if long_remaining > dec!(0) || short_remaining > dec!(0) {
        return Err("Requested quantity exceeds executable book depth".into());
    }
    let long_vwap_f = dec_to_f64(long_vwap, "funding long VWAP")?;
    let short_vwap_f = dec_to_f64(short_vwap, "funding short VWAP")?;
    let max_entry_spread_bps = state
        .strategy_params
        .read()
        .get("funding")
        .map(|p| p.funding_max_entry_spread_bps)
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(50.0);
    let entry_spread_bps =
        (long_vwap_f - short_vwap_f).abs() / ((long_vwap_f + short_vwap_f) / 2.0) * 10_000.0;
    if !entry_spread_bps.is_finite() || entry_spread_bps > max_entry_spread_bps {
        return Err(format!(
            "funding entry spread {entry_spread_bps:.1} bps on {long_sym}/{short_sym} exceeds the {max_entry_spread_bps:.1} bps limit"
        ));
    }

    // Blocking signals + existing paper positions refuse dispatch. The block
    // is scoped to the root perp; residual checks read each exact leg symbol.
    if let Some(blocker) = state
        .state_store
        .blocking_signals(&root_symbol, Some(state.env().as_str()))
        .map_err(|e| e.to_string())?
        .into_iter()
        .next()
    {
        return Err(format!(
            "Symbol {root_symbol} is blocked by signal {} (status {}): awaiting leg reconciliation/delta-neutral confirmation",
            blocker.id,
            blocker.status.as_str()
        ));
    }
    if state
        .state_store
        .ticker_reduce_only(&root_symbol, "funding", state.env().as_str())
        .map_err(|e| e.to_string())?
    {
        return Err(format!(
            "Symbol {root_symbol} is reduce-only for funding — exits only, entry refused"
        ));
    }
    let dust = triangle_dust_qty();
    let acc_long_pre = long_adapter
        .fetch_account()
        .await
        .map_err(|e| format!("funding long-leg account read on {long_canon} failed: {e}"))?;
    let acc_short_pre = short_adapter
        .fetch_account()
        .await
        .map_err(|e| format!("funding short-leg account read on {short_canon} failed: {e}"))?;
    for (adapter, acc, exact, canon) in [
        (&long_adapter, &acc_long_pre, long_sym.as_str(), long_canon),
        (
            &short_adapter,
            &acc_short_pre,
            short_sym.as_str(),
            short_canon,
        ),
    ] {
        if adapter
            .account_position_for(acc, exact)
            .ok()
            .flatten()
            .is_some_and(|pos| pos.quantity > dust)
        {
            return Err(format!(
                "An open position already exists for {exact} on {canon}; flatten it before carrying"
            ));
        }
    }

    // Leverage is per leg: a spot leg books at 1x, perp legs at the
    // resolved funding leverage. Venue-side set is best-effort — a rejected
    // change is not fatal but must be visible.
    let (target_leverage, _account_cap) = resolve_effective_leverage(
        state,
        "funding",
        "funding",
        &root_symbol,
        long_canon,
        short_canon,
        None,
    )?;
    let long_lev: u32 = if is_basis { 1 } else { target_leverage };
    let short_lev: u32 = target_leverage;
    for (adapter, lev, exact, canon) in [
        (&long_adapter, long_lev, long_sym.as_str(), long_canon),
        (&short_adapter, short_lev, short_sym.as_str(), short_canon),
    ] {
        if let Err(e) = adapter.set_leverage(exact, lev).await {
            tracing::warn!(
                "funding set_leverage {lev}x on {canon} {exact} failed: {e} — order will submit at the venue's existing leverage"
            );
        }
    }

    // Initial margin aggregates by canonical venue: a same-venue pair must
    // cover the spot leg's full notional plus the perp leg's IM.
    let long_im = (qty * long_vwap) / Decimal::from(long_lev.max(1));
    let short_im = (qty * short_vwap) / Decimal::from(short_lev.max(1));
    let acc_long = acc_long_pre;
    let acc_short = acc_short_pre;
    let monitor = margin_monitor_from_settings(state);
    if long_canon == short_canon {
        match monitor.evaluate_account(&acc_long) {
            MarginHealthStatus::Empty => {
                return Err(format!(
                    "Account {long_canon} has no funds; execution refused"
                ));
            }
            MarginHealthStatus::Critical | MarginHealthStatus::Breached => {
                return Err(format!(
                    "Account {long_canon} margin status is critical/breached; execution refused"
                ));
            }
            _ => {}
        }
        if long_im + short_im > acc_long.free_margin {
            return Err(format!(
                "Insufficient free margin on {long_canon}: required {} (spot full notional plus perp IM) > available {}",
                long_im + short_im,
                acc_long.free_margin
            ));
        }
    } else {
        for (canon, acc, req) in [
            (long_canon, &acc_long, long_im),
            (short_canon, &acc_short, short_im),
        ] {
            match monitor.evaluate_account(acc) {
                MarginHealthStatus::Empty => {
                    return Err(format!("Account {canon} has no funds; execution refused"));
                }
                MarginHealthStatus::Critical | MarginHealthStatus::Breached => {
                    return Err(format!(
                        "Account {canon} margin status is critical/breached; execution refused"
                    ));
                }
                _ => {}
            }
            if req > acc.free_margin {
                return Err(format!(
                    "Insufficient free margin on {canon}: required {req} > available {}",
                    acc.free_margin
                ));
            }
        }
    }

    let cycle = state.next_sim_step();
    let cycle_tag = format!(
        "fund-{}-{:08x}",
        Utc::now().timestamp_millis(),
        rand::random::<u32>()
    );
    let kind_label = if is_fixed {
        "fixed-yield"
    } else if is_basis {
        "basis"
    } else {
        "carry"
    };
    let path = format!("LONG {long_sym}@{long_canon} / SHORT {short_sym}@{short_canon}");
    let carry_note = format!(
        "funding {kind_label} {root_symbol} 2/2 legs filled, net {:.2}% APR at scan",
        opp.net_apr_pct
    );

    // One two-leg funding signal persisted BEFORE dispatch: an interrupted
    // run keeps the root blocked until its own legs reconcile. Legs carry
    // their exact symbols; the root carries the requested perp.
    use crate::state_store::{LegSide, NewSignalExecution, NewSignalLeg};
    let env_str = state.env().as_str();
    let account_for = |venue: &str| {
        state
            .state_store
            .default_account(venue, env_str)
            .ok()
            .flatten()
            .map(|a| a.id)
    };
    let signal_id = format!("{cycle_tag}-carry");
    state
        .state_store
        .record_signal_execution(
            &NewSignalExecution {
                id: signal_id.clone(),
                strategy_id: FUNDING_STRATEGY_ID.to_string(),
                symbol: root_symbol.clone(),
                route_id: None,
                // The scan's cost-adjusted projection (carry − fees − spread
                // over the hold estimate) is the honest entry expectation.
                expected_net_usd: opp.expected_return_usd.filter(|v| v.is_finite()),
                note: Some(format!("{carry_note}; {path}")),
                legs: vec![
                    NewSignalLeg {
                        account_id: account_for(long_canon),
                        venue: long_canon.to_string(),
                        side: LegSide::Long,
                        symbol: Some(long_sym.clone()),
                        intended_qty: qty_f64,
                        intended_price: Some(long_ask_f),
                        unit_scale: None,
                    },
                    NewSignalLeg {
                        account_id: account_for(short_canon),
                        venue: short_canon.to_string(),
                        side: LegSide::Short,
                        symbol: Some(short_sym.clone()),
                        intended_qty: qty_f64,
                        intended_price: Some(short_bid_f),
                        unit_scale: None,
                    },
                ],
            },
            state.env().as_str(),
        )
        .map_err(|e| format!("Failed to persist funding signal record: {e}"))?;

    // Calibration log: the funding card carries three forecast numbers,
    // each a different event — persist each under its own kind so scoring
    // never mixes semantics. `net_win` (backtest P(net>0)) scores against
    // realized net bps; `funding_persist` and the EV-transform heuristic
    // persist for audit. Non-fatal on failure.
    for (kind, p) in [
        ("net_win", opp.backtest_win_rate),
        ("funding_persist", opp.persistence_probability),
        ("heuristic", opp.win_probability),
    ]
    .into_iter()
    .filter_map(|(k, v)| v.map(|p| (k, p)))
    {
        if let Err(e) = state.state_store.record_signal_prediction(
            &signal_id,
            kind,
            p,
            None,
            FUNDING_STRATEGY_ID,
            &root_symbol,
            &format!("{long_canon}/{short_canon}"),
            state.is_live(),
        ) {
            tracing::warn!("persist funding prediction {kind} failed for {signal_id}: {e}");
        }
    }

    // Pre-trade equity snapshot of the two traded venues (env-namespaced
    // keys) — the baseline the post-fill attribution refresh measures
    // own-fill deltas against.
    let mut equity_before_by_venue = HashMap::new();
    for (vid, acc) in [(long_canon, &acc_long), (short_canon, &acc_short)] {
        let mut acc_usd = (*acc).clone();
        state.normalize_account_usd(vid, &mut acc_usd);
        if let Ok(f) = acc_usd.equity.to_string().parse::<f64>() {
            if f.is_finite() {
                equity_before_by_venue.insert(format!("{env_str}-{vid}"), f);
            }
        }
    }

    // Delta-neutral in base terms — lift the long ask, hit the short bid —
    // dispatched concurrently through the atomic coordinator.
    let long_exec: Arc<dyn ExchangeAdapter> = long_adapter.clone();
    let short_exec: Arc<dyn ExchangeAdapter> = short_adapter.clone();
    let order_long = state.coordinator.router().build_ioc_order(
        long_exec.venue_id(),
        long_sym.clone(),
        Side::Buy,
        long_ask,
        qty,
    );
    let order_short = state.coordinator.router().build_ioc_order(
        short_exec.venue_id(),
        short_sym.clone(),
        Side::Sell,
        short_bid,
        qty,
    );
    let started = std::time::Instant::now();
    let res = state
        .coordinator
        .execute_two_leg(&long_exec, order_long, &short_exec, order_short)
        .await;
    let latency_us = started.elapsed().as_micros() as u64;

    refresh_equity_expectations_after_fills(state, &equity_before_by_venue).await;
    // Realized P&L comes from the coordinator's verified fill accounting —
    // identical semantics in both environments. PartialPairKept carries the
    // closed trim's real P&L and unwound variants a real round-trip cost —
    // neither may fall into the 0.0 arm.
    let attempt_pnl_f = match &res {
        ArbitrageExecutionResult::Success {
            realized_profit, ..
        }
        | ArbitrageExecutionResult::PartialPairKept {
            realized_profit, ..
        } => realized_profit.to_string().parse::<f64>().unwrap_or(0.0),
        ArbitrageExecutionResult::LegBFailedUnwound {
            leg_a_fill,
            unwind_resp,
            ..
        } => crate::execution::unwind_round_trip_pnl(leg_a_fill, unwind_resp, Side::Buy)
            .to_string()
            .parse::<f64>()
            .unwrap_or(0.0),
        ArbitrageExecutionResult::LegAFailedUnwound {
            leg_b_fill,
            unwind_resp,
            ..
        } => crate::execution::unwind_round_trip_pnl(leg_b_fill, unwind_resp, Side::Sell)
            .to_string()
            .parse::<f64>()
            .unwrap_or(0.0),
        _ => 0.0,
    };
    let total_f =
        crate::execution::accumulate_realized_profit(state, state.is_live(), attempt_pnl_f);

    let net_bps = opp.net_rate_8h_pct * 100.0;

    // A partial fill whose excess trims cleanly keeps the matched quantity
    // open as a delta-neutral pair. A trim whose residual unwind failed never
    // reaches this arm: the coordinator reports UnresolvedExposure and trips.
    let mut open_qty = qty_f64;
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
                "Partial fills trimmed to matched {matched_qty} (long: {}; short: {}); ",
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
            ArbitrageExecutionResult::Success {
                realized_profit: -(a.fee.unwrap_or_default() + b.fee.unwrap_or_default()),
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
            let fees_dec = leg_a.fee.unwrap_or_default() + leg_b.fee.unwrap_or_default();
            let fee_f = fees_dec.to_string().parse::<f64>().unwrap_or(0.0);
            let long_px_f = leg_a
                .avg_fill_price
                .and_then(|p| p.to_string().parse::<f64>().ok())
                .filter(|p| *p > 0.0)
                .unwrap_or(long_vwap_f);
            let short_px_f = leg_b
                .avg_fill_price
                .and_then(|p| p.to_string().parse::<f64>().ok())
                .filter(|p| *p > 0.0)
                .unwrap_or(short_vwap_f);
            // Persist actual leg fills, then reconcile per-signal balance:
            // Balanced only when THIS signal's own legs net to zero delta.
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

            // ---- Boros YU hedge legs (kind == "fixed_yield") ----
            // The perp pair is verified open; now lock its funding via two
            // YU legs. Failure policy: a YU leg that fills nothing means
            // the requested lock does not exist — unwind the perp pair
            // (emergency exits stay market) and exit any filled YU side,
            // then fail. Partial YU fills keep the pair open with the
            // residual floating share reported honestly.
            let mut fixed_note = String::new();
            if is_fixed {
                let yu_signal_id = format!("{cycle_tag}-yu");
                let boros_concrete = boros_for_exec(state);
                let boros_adapter: Arc<dyn ExchangeAdapter> = boros_concrete.clone();
                // Resolve markets + sizes BEFORE persisting or dispatching —
                // the signal ledger must carry real intended quantities,
                // and an unresolvable market fails before any YU order.
                let mut yu_plan: Vec<(&YuLegDto, Decimal, Decimal)> = Vec::new();
                for leg in &yu_legs {
                    let leg_px_f = if leg.venue_id == long_vid {
                        long_px_f
                    } else {
                        short_px_f
                    };
                    let market = boros_concrete
                        .market_by_id(leg.yu_market_id)
                        .await
                        .map_err(|e| format!("YU market {} unavailable: {e}", leg.yu_market_id))?;
                    // YU size is denominated in the market's collateral
                    // token — convert the perp leg's USD notional.
                    let notional_usd = open_qty * leg_px_f;
                    let size_yu = notional_usd / market.collateral_usd_price;
                    let qty_dec = Decimal::from_str_exact(&size_yu.to_string())
                        .ok()
                        .filter(|q| *q > dec!(0))
                        .ok_or_else(|| {
                            format!("YU size {size_yu} for {} not positive", leg.yu_symbol)
                        })?;
                    // Rate bound: long YU pays fixed — cap ~2% above the
                    // quoted ask; short YU receives fixed — floor ~2%
                    // below the quoted bid. IOC so an unexecutable bound
                    // never rests on the book.
                    let rate = leg.fixed_apr_pct / 100.0;
                    let bound = if leg.yu_side == "long" {
                        rate * 1.02
                    } else {
                        rate * 0.98
                    };
                    let bound_dec = Decimal::from_f64_retain(bound)
                        .ok_or_else(|| "YU rate bound not decimal-safe".to_string())?;
                    yu_plan.push((leg, qty_dec, bound_dec));
                }

                // Sibling signal row: the pair-ledger schema is two-sided
                // per signal, so the YU hedge persists as its own two-leg
                // signal under the shared cycle tag (triangle precedent).
                state
                    .state_store
                    .record_signal_execution(
                        &NewSignalExecution {
                            id: yu_signal_id.clone(),
                            strategy_id: FUNDING_STRATEGY_ID.to_string(),
                            symbol: root_symbol.clone(),
                            route_id: None,
                            expected_net_usd: opp.expected_return_usd.filter(|v| v.is_finite()),
                            note: Some(format!(
                                "boros YU hedge for {signal_id} (fixed yield to maturity)"
                            )),
                            legs: yu_plan
                                .iter()
                                .map(|(l, qty, bound)| NewSignalLeg {
                                    account_id: account_for("boros"),
                                    venue: "boros".to_string(),
                                    side: if l.yu_side == "long" {
                                        LegSide::Long
                                    } else {
                                        LegSide::Short
                                    },
                                    symbol: Some(l.yu_symbol.clone()),
                                    intended_qty: qty.to_string().parse::<f64>().unwrap_or(0.0),
                                    intended_price: Some(
                                        bound.to_string().parse::<f64>().unwrap_or(0.0),
                                    ),
                                    unit_scale: None,
                                })
                                .collect(),
                        },
                        state.env().as_str(),
                    )
                    .map_err(|e| format!("Failed to persist YU hedge signal record: {e}"))?;

                let mut yu_fills: Vec<(crate::state_store::LegSide, OrderResponse)> = Vec::new();
                let mut yu_fatal: Option<String> = None;
                for (leg, qty_dec, bound_dec) in &yu_plan {
                    let side = if leg.yu_side == "long" {
                        Side::Buy
                    } else {
                        Side::Sell
                    };
                    let req = OrderRequest::new_ioc(
                        VenueId::from("boros"),
                        format!("BOROS-{}", leg.yu_market_id),
                        side,
                        *bound_dec,
                        *qty_dec,
                    );
                    let step = async {
                        let mut resp = boros_adapter
                            .place_order(req)
                            .await
                            .map_err(|e| format!("YU order on {} rejected: {e}", leg.yu_symbol))?;
                        // Settle to terminal via venue truth — a submission
                        // ack is never a fill.
                        for _ in 0..YU_SETTLE_POLLS {
                            if resp.status.is_final() {
                                break;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(YU_SETTLE_POLL_MS))
                                .await;
                            match boros_adapter
                                .fetch_order(
                                    &format!("BOROS-{}", leg.yu_market_id),
                                    &resp.order_id,
                                    None,
                                )
                                .await
                            {
                                Ok(polled) => resp = polled,
                                Err(_) => continue,
                            }
                        }
                        Ok::<OrderResponse, String>(resp)
                    }
                    .await;
                    match step {
                        Ok(resp) => {
                            let side = if leg.yu_side == "long" {
                                crate::state_store::LegSide::Long
                            } else {
                                crate::state_store::LegSide::Short
                            };
                            if resp.filled_qty <= dec!(0) && resp.status.is_final() {
                                yu_fatal = Some(format!(
                                    "YU leg {} filled nothing ({:?})",
                                    leg.yu_symbol, resp.status
                                ));
                                yu_fills.push((side, resp));
                                break;
                            }
                            if !resp.status.is_final() && resp.filled_qty <= dec!(0) {
                                // An order that never reached a terminal
                                // state AND shows no fill is unverifiable —
                                // stop rather than stack a second YU leg on
                                // top. A PartiallyFilled order has proven
                                // fills and proceeds (marked Imbalanced).
                                yu_fatal = Some(format!(
                                    "YU leg {} order {} never resolved ({:?}, zero fills) — refusing to stack the second leg",
                                    leg.yu_symbol, resp.order_id, resp.status
                                ));
                                yu_fills.push((side, resp));
                                break;
                            }
                            yu_fills.push((side, resp));
                        }
                        Err(e) => {
                            yu_fatal = Some(e);
                            break;
                        }
                    }
                }

                // Persist every accepted YU order id + fill on the sibling
                // signal before deciding the outcome — reconciliation reads
                // these rows on any path, including failure.
                for (side, fill) in &yu_fills {
                    if !fill.order_id.is_empty() {
                        let _ = state.state_store.record_leg_order_id(
                            &yu_signal_id,
                            *side,
                            fill.order_id.as_str(),
                            None,
                        );
                    }
                    let filled = fill.filled_qty.to_string().parse::<f64>().unwrap_or(0.0);
                    let px = fill
                        .avg_fill_price
                        .and_then(|p| p.to_string().parse::<f64>().ok())
                        .unwrap_or(0.0);
                    if filled > 0.0 && px > 0.0 {
                        let fee = fill
                            .fee
                            .map(|v| v.to_string().parse::<f64>().unwrap_or(0.0))
                            .unwrap_or(0.0);
                        let _ = state.state_store.apply_leg_fill(
                            &yu_signal_id,
                            *side,
                            filled,
                            px,
                            fee,
                            Some(fill.order_id.as_str()),
                        );
                    }
                }

                if let Some(fatal) = yu_fatal {
                    // The requested fixed lock cannot be assembled — return
                    // to flat. Perp unwind is a market emergency exit per
                    // leg; any filled YU side is exited the same way.
                    let mut unwind_errors: Vec<String> = Vec::new();
                    if let Err(e) =
                        arbitrageur_execution::emergency_unwind::EmergencyUnwindHandler::unwind_leg(
                            &long_adapter,
                            &long_sym,
                            Side::Buy,
                            Decimal::from_str_exact(&open_qty.to_string()).unwrap_or(dec!(0)),
                        )
                        .await
                    {
                        unwind_errors.push(format!("long perp unwind: {e}"));
                    }
                    if let Err(e) =
                        arbitrageur_execution::emergency_unwind::EmergencyUnwindHandler::unwind_leg(
                            &short_adapter,
                            &short_sym,
                            Side::Sell,
                            Decimal::from_str_exact(&open_qty.to_string()).unwrap_or(dec!(0)),
                        )
                        .await
                    {
                        unwind_errors.push(format!("short perp unwind: {e}"));
                    }
                    for (side, fill) in &yu_fills {
                        if fill.filled_qty > dec!(0) {
                            // Emergency exit: Boros has no market order —
                            // IOC with a wide bound is the equivalent. A
                            // long-YU exit sells (rate floor: 50% below
                            // entry); a short-YU exit buys (cap: 50%
                            // above entry). Never bound an unwind at the
                            // entry rate — a moved market must still exit.
                            let exit_side = if *side == crate::state_store::LegSide::Long {
                                Side::Sell
                            } else {
                                Side::Buy
                            };
                            let entry_rate = fill.avg_fill_price.unwrap_or(dec!(0.05));
                            let bound = if exit_side == Side::Sell {
                                entry_rate * dec!(0.5)
                            } else {
                                entry_rate * dec!(1.5)
                            };
                            let req = OrderRequest::new_ioc(
                                VenueId::from("boros"),
                                fill.symbol.clone(),
                                exit_side,
                                bound,
                                fill.filled_qty,
                            );
                            if let Err(e) = boros_adapter.place_order(req).await {
                                unwind_errors.push(format!("YU exit {}: {e}", fill.symbol));
                            }
                        }
                    }
                    let _ = state.state_store.update_signal_status(
                        &yu_signal_id,
                        crate::state_store::SignalStatus::Failed,
                        Some(&fatal),
                    );
                    let _ = state.state_store.update_signal_status(
                        &signal_id,
                        crate::state_store::SignalStatus::Failed,
                        Some(&format!("fixed-yield hedge failed: {fatal}")),
                    );
                    if !unwind_errors.is_empty() {
                        if !state.circuit_breaker.is_tripped() {
                            state
                                .circuit_breaker
                                .trip("Unresolved exposure: fixed-yield unwind incomplete");
                        }
                        return Err(format!(
                            "UNRESOLVED EXPOSURE — fixed-yield hedge failed ({fatal}) and unwind incomplete: {}",
                            unwind_errors.join("; ")
                        ));
                    }
                    return Err(format!(
                        "fixed-yield hedge failed ({fatal}); perp pair unwound to flat"
                    ));
                }

                // Both YU legs submitted; reconcile the sibling signal.
                // Equal filled sizes → Balanced (fully locked). A partial
                // side leaves an honest Imbalanced marker — the residual
                // funding stays floating.
                let yu_status = if yu_fills.len() == 2 {
                    reconcile_signal_legs(
                        state,
                        &yu_signal_id,
                        [
                            (yu_fills[0].0, &yu_fills[0].1),
                            (yu_fills[1].0, &yu_fills[1].1),
                        ],
                    )
                    .unwrap_or(crate::state_store::SignalStatus::Imbalanced)
                } else {
                    crate::state_store::SignalStatus::Imbalanced
                };
                let _ = state
                    .state_store
                    .update_signal_status(&yu_signal_id, yu_status, None);
                // Maturity tracking starts when YU fills are live — the
                // fatal path returned above, so any row here hedges real
                // exposure until venue-confirmed settlement.
                if yu_fills.iter().any(|(_, f)| f.filled_qty > dec!(0)) {
                    let market_ids: Vec<u64> = yu_legs.iter().map(|l| l.yu_market_id).collect();
                    match opp.lock_until_ms {
                        // No venue-derived maturity → no tracker row; the
                        // signal note records the gap honestly.
                        None => {
                            let _ = state.state_store.update_signal_status(
                                &yu_signal_id,
                                yu_status,
                                Some("no venue lock_until — maturity lifecycle untracked"),
                            );
                        }
                        Some(lock_ms) => {
                            if let Err(e) = state.state_store.record_fixed_yield_maturity(
                                &yu_signal_id,
                                &signal_id,
                                lock_ms,
                                &market_ids,
                            ) {
                                let _ = state.state_store.update_signal_status(
                                    &yu_signal_id,
                                    yu_status,
                                    Some(&format!("maturity tracker persist failed: {e}")),
                                );
                            }
                        }
                    }
                }
                let partial = yu_fills
                    .iter()
                    .any(|(_, f)| f.remaining_qty > dec!(0) || f.status != OrderStatus::Filled);
                fixed_note = if yu_status == crate::state_store::SignalStatus::Balanced && !partial
                {
                    " + Boros YU hedge locked (2/2 filled)".to_string()
                } else {
                    format!(" + Boros YU hedge PARTIAL ({yu_status:?}) — residual funding stays floating")
                };
            }

            // Open the attributed ledger lot so the eventual flatten can
            // realize PnL and resolve this signal's entry predictions —
            // without it close_flattened_spatial_lot finds no funding_rate
            // lot and every funding prediction stays unresolved forever.
            let funding_cycle_id = format!("funding-{cycle}");
            {
                let mut ledger = state.virtual_ledger.write();
                ledger.open_cycle(
                    funding_cycle_id.clone(),
                    crate::attribution::ledger_strategy_id(FUNDING_STRATEGY_ID),
                    root_symbol.clone(),
                    long_canon.to_string(),
                    short_canon.to_string(),
                    net_bps,
                    open_qty,
                    long_px_f,
                    open_qty,
                    short_px_f,
                    fee_f,
                    state.env().as_str().to_string(),
                );
                if let Some(lot) = ledger.lots.get(&funding_cycle_id) {
                    state
                        .state_store
                        .upsert_cycle_lot(lot)
                        .map_err(|e| format!("Failed to persist cycle lot: {e}"))?;
                }
            }

            record_execution_log(state, ExecutionLogEntry {
                id: format!("exec-funding-{cycle}"),
                timestamp: Utc::now().to_rfc3339(),
                trade_type: "Funding".into(),
                strategy_id: crate::attribution::attribute_strategy_id(FUNDING_STRATEGY_ID),
                symbol: root_symbol.clone(),
                venue_a: opp.long_venue.clone(),
                venue_b: opp.short_venue.clone(),
                leg_a_status: format!("{:?}", leg_a.status),
                leg_b_status: format!("{:?}", leg_b.status),
                success: true,
                gross_profit: 0.0,
                fees: fee_f,
                net_profit: attempt_pnl_f,
                latency_us,
                note: format!(
                    "{trim_note}Funding {kind_label} opened {open_qty} {root_symbol}: {path} at {long_px_f}/{short_px_f}; {carry_note}{fixed_note}"
                ),
                direction: Some(path.clone()),
                trigger_spread_bps: Some(net_bps),
                baseline_mean_spread_bps: None,
                position_step: None,
                max_units: None,
                nominal_hurdle_bps: Some(hurdle),
                is_live,
            });

            maybe_record_margin_snapshots(state, Utc::now().timestamp_millis()).await;

            Ok(SimStepResultDto {
                cycle,
                opportunity_found: true,
                execution_success: true,
                trade_type: "Funding".into(),
                direction: path.clone(),
                venue_a: opp.long_venue.clone(),
                venue_b: opp.short_venue.clone(),
                spread_bps: net_bps,
                realized_profit_usd: attempt_pnl_f,
                total_cumulative_pnl_usd: total_f,
                message: format!(
                    "{trim_note}Funding {kind_label} dispatched on {root_symbol} ({path}): 2/2 legs filled {open_qty} (LONG {long_sym} / SHORT {short_sym}), realized P&L ${attempt_pnl_f:.2}; carry position open — flatten {root_symbol} to unwind{fixed_note}"
                ),
            })
        }
        ArbitrageExecutionResult::UnresolvedExposure {
            leg_a_fill,
            leg_b_fill,
            error_a,
            error_b,
        } => {
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
                    let fee = f
                        .fee
                        .map(|v| v.to_string().parse::<f64>().unwrap_or(0.0))
                        .unwrap_or(0.0);
                    // Keep the venue order id reachable even when the leg was
                    // accepted but unfilled — reconciliation polls it to
                    // terminal state instead of losing the exposure.
                    if !f.order_id.is_empty() {
                        let cid =
                            (!f.client_order_id.is_empty()).then_some(f.client_order_id.as_str());
                        if let Err(e) = state.state_store.record_leg_order_id(
                            &signal_id,
                            side,
                            f.order_id.as_str(),
                            cid,
                        ) {
                            // A lost order id orphans a live venue order —
                            // louder than the surrounding `let _` style.
                            tracing::error!(
                                "persist venue order id failed for {signal_id} {side:?}: {e}"
                            );
                        }
                    }
                    if filled > 0.0 && px > 0.0 {
                        if let Err(e) = state.state_store.apply_leg_fill(
                            &signal_id,
                            side,
                            filled,
                            px,
                            fee,
                            Some(f.order_id.as_str()),
                        ) {
                            tracing::error!(
                                "persist leg fill failed for {signal_id} {side:?}: {e}"
                            );
                        }
                    }
                }
            }
            let _ = state.state_store.update_signal_status(
                &signal_id,
                crate::state_store::SignalStatus::Unresolved,
                Some(&format!(
                    "unresolved funding exposure — leg A: {error_a}; leg B: {error_b}"
                )),
            );
            if !state.circuit_breaker.is_tripped() {
                state
                    .circuit_breaker
                    .trip("Unresolved funding exposure after failed unwind");
            }
            let describe = |f: &Option<arbitrageur_core::order::OrderResponse>| {
                f.as_ref()
                    .map(|r| format!("{:?} filled {}/{}", r.status, r.filled_qty, r.original_qty))
                    .unwrap_or_else(|| "no fill".to_string())
            };
            record_execution_log(state, ExecutionLogEntry {
                id: format!("exec-funding-{cycle}"),
                timestamp: Utc::now().to_rfc3339(),
                trade_type: "Funding".into(),
                strategy_id: crate::attribution::attribute_strategy_id(FUNDING_STRATEGY_ID),
                symbol: root_symbol.clone(),
                venue_a: opp.long_venue.clone(),
                venue_b: opp.short_venue.clone(),
                leg_a_status: describe(&leg_a_fill),
                leg_b_status: describe(&leg_b_fill),
                success: false,
                gross_profit: 0.0,
                fees: 0.0,
                net_profit: attempt_pnl_f,
                latency_us,
                note: format!(
                    "UNRESOLVED FUNDING EXPOSURE on {root_symbol} ({path}): leg A [{error_a}]; leg B [{error_b}]"
                ),
                direction: Some(path.clone()),
                trigger_spread_bps: Some(net_bps),
                baseline_mean_spread_bps: None,
                position_step: None,
                max_units: None,
                nominal_hurdle_bps: Some(hurdle),
                is_live,
            });
            Err(format!(
                "UNRESOLVED FUNDING EXPOSURE: leg A [{error_a}]; leg B [{error_b}]"
            ))
        }
        other => {
            let (status_a, status_b) = match &other {
                ArbitrageExecutionResult::LegBFailedUnwound { .. } => ("UNWOUND", "REJECTED"),
                ArbitrageExecutionResult::LegAFailedUnwound { .. } => ("REJECTED", "UNWOUND"),
                ArbitrageExecutionResult::HaltedByRisk(_) => ("HALTED", "HALTED"),
                _ => ("REJECTED", "REJECTED"),
            };
            // Persist the true atomic trail BEFORE the failed mark: the
            // filled leg's entry fill plus its compensation unwind are real
            // venue orders — as leg rows, not only raw JSON in the note.
            match &other {
                ArbitrageExecutionResult::LegBFailedUnwound {
                    leg_a_fill,
                    unwind_resp,
                    ..
                } => crate::execution::persist_unwind_leg_records(
                    state,
                    &signal_id,
                    crate::state_store::LegSide::Long,
                    leg_a_fill,
                    unwind_resp,
                ),
                ArbitrageExecutionResult::LegAFailedUnwound {
                    leg_b_fill,
                    unwind_resp,
                    ..
                } => crate::execution::persist_unwind_leg_records(
                    state,
                    &signal_id,
                    crate::state_store::LegSide::Short,
                    leg_b_fill,
                    unwind_resp,
                ),
                _ => {}
            }
            let _ = state.state_store.update_signal_status(
                &signal_id,
                crate::state_store::SignalStatus::Failed,
                Some(&format!("{other:?}")),
            );
            record_execution_log(
                state,
                ExecutionLogEntry {
                    id: format!("exec-funding-{cycle}"),
                    timestamp: Utc::now().to_rfc3339(),
                    trade_type: "Funding".into(),
                    strategy_id: crate::attribution::attribute_strategy_id(FUNDING_STRATEGY_ID),
                    symbol: root_symbol.clone(),
                    venue_a: opp.long_venue.clone(),
                    venue_b: opp.short_venue.clone(),
                    leg_a_status: status_a.into(),
                    leg_b_status: status_b.into(),
                    success: false,
                    gross_profit: 0.0,
                    fees: 0.0,
                    net_profit: attempt_pnl_f,
                    latency_us,
                    note: format!(
                    "Funding {kind_label} compensated failure on {root_symbol} ({path}): {other:?}"
                ),
                    direction: Some(path.clone()),
                    trigger_spread_bps: Some(net_bps),
                    baseline_mean_spread_bps: None,
                    position_step: None,
                    max_units: None,
                    nominal_hurdle_bps: Some(hurdle),
                    is_live,
                },
            );

            Ok(SimStepResultDto {
                cycle,
                opportunity_found: true,
                execution_success: false,
                trade_type: "Funding".into(),
                direction: path.clone(),
                venue_a: opp.long_venue.clone(),
                venue_b: opp.short_venue.clone(),
                spread_bps: net_bps,
                realized_profit_usd: attempt_pnl_f,
                total_cumulative_pnl_usd: total_f,
                message: format!("Funding {kind_label} execution failed/unwound: {other:?}"),
            })
        }
    }
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
pub async fn execute_funding_carry(
    symbol: String,
    long_venue: String,
    short_venue: String,
    trade_size_usd: f64,
    auto: Option<bool>,
    kind: Option<String>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SimStepResultDto, String> {
    match execute_funding_carry_impl(
        &symbol,
        &long_venue,
        &short_venue,
        trade_size_usd,
        &state,
        auto.unwrap_or(false),
        kind.as_deref(),
    )
    .await
    {
        Ok(r) => Ok(r),
        Err(e) => {
            record_execution_failure(
                &state,
                "Funding",
                FUNDING_STRATEGY_ID,
                &symbol,
                &long_venue,
                &short_venue,
                &e,
            );
            Err(e)
        }
    }
}

/* --------------------------------------------------------------------- */
/* Fixed-yield early exit (issue #25)                                     */
/*                                                                       */
/* A YU lock can be unwound before maturity — the exit is PRICED: the     */
/* quote fetches each leg's current executable rate and reports the       */
/* estimated PnL of closing now vs riding to maturity. Execution re-      */
/* quotes (books move), dispatches bounded IOCs, then flattens the        */
/* hedged perp pair. The emergency unwind path is unchanged — wide-bound  */
/* market-equivalent, no quote dependency.                                */
/* --------------------------------------------------------------------- */

/// Per-leg exit quote: entry rate vs the rate the book will actually
/// pay/charge to close now, and the estimated PnL of that close over the
/// remaining term.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixedYieldExitQuoteLegDto {
    pub leg_side: String,
    pub yu_symbol: String,
    pub market_id: u64,
    /// Collateral-unit size still open (entry fills minus prior exits).
    pub open_qty: f64,
    /// Entry implied APR (fraction) from the ledger fill.
    pub entry_rate: f64,
    /// Current executable exit rate: best bid to close a long, best ask
    /// to close a short. `None` when the book shows no executable side —
    /// the leg cannot be quoted, and execution refuses.
    pub exit_rate: Option<f64>,
    /// Signed estimate: (exit − entry) × notional × years for a long;
    /// the mirror for a short. Positive = exiting now is profitable vs
    /// the entry rate over remaining term.
    pub est_exit_pnl_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixedYieldExitQuoteDto {
    pub signal_id: String,
    pub parent_signal_id: Option<String>,
    pub maturity_state: String,
    pub lock_until_ms: i64,
    pub years_remaining: f64,
    pub legs: Vec<FixedYieldExitQuoteLegDto>,
    /// Sum of quotable legs' estimates — `None` when any leg is
    /// unquotable, so the UI never shows a partial total as complete.
    pub est_total_pnl_usd: Option<f64>,
    /// Residual perp unwind: the parent pair still needs its perp legs
    /// closed — quoted separately because perp exit cost depends on the
    /// flatten path, not the YU book.
    pub notes: Vec<String>,
}

/// Open collateral-units per sibling leg: entry fills minus fills already
/// recorded on `flat-{yu_signal_id}-*` close children.
fn yu_open_qty_by_leg(
    state: &AppEngineState,
    yu_signal: &crate::state_store::SignalExecution,
) -> Result<Vec<Decimal>, String> {
    let all = state
        .state_store
        .list_signals(None, None)
        .map_err(|e| e.to_string())?;
    let children: Vec<_> = all
        .iter()
        .filter(|s| crate::state_store::is_flatten_child_id(&s.id, &yu_signal.id))
        .collect();
    let mut out = Vec::new();
    for leg in &yu_signal.legs {
        let entry = crate::execution::ledger_qty_to_dec(leg.filled_qty, "YU leg fill")?;
        let mut closed = dec!(0);
        for child in &children {
            for cl in &child.legs {
                if cl.side == leg.side {
                    closed +=
                        crate::execution::ledger_qty_to_dec(cl.filled_qty, "YU exit leg fill")?;
                }
            }
        }
        out.push((entry - closed).max(dec!(0)));
    }
    Ok(out)
}

/// Quote-only view of an early exit — no orders dispatched. Usable for
/// `locked`/`due`/`settlement_unverified` rows; a `settled`/`closed_early`
/// row has no live YU position to exit.
pub async fn fixed_yield_exit_quote_impl(
    yu_signal_id: &str,
    state: &AppEngineState,
) -> Result<FixedYieldExitQuoteDto, String> {
    let row = state
        .state_store
        .list_fixed_yield_maturities()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|r| r.signal_id == yu_signal_id)
        .ok_or_else(|| format!("no fixed-yield maturity row for {yu_signal_id}"))?;
    if matches!(row.maturity_state.as_str(), "settled" | "closed_early") {
        return Err(format!(
            "YU lock {} is {} — no live position to exit",
            yu_signal_id, row.maturity_state
        ));
    }
    let yu_signal = state
        .state_store
        .get_signal(yu_signal_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("signal {yu_signal_id} not found"))?;
    let boros = boros_for_exec(state);
    let now_ms = Utc::now().timestamp_millis();
    let years_remaining =
        ((row.lock_until_ms - now_ms).max(0) as f64) / (365.25 * 86_400.0 * 1000.0);
    let open_by_leg = yu_open_qty_by_leg(state, &yu_signal)?;

    let mut notes = Vec::new();
    let mut legs = Vec::new();
    for (leg, open_qty) in yu_signal.legs.iter().zip(open_by_leg.iter()) {
        let leg_side = match leg.side {
            crate::state_store::LegSide::Long => "long",
            crate::state_store::LegSide::Short => "short",
        };
        let entry_rate = leg.fill_price.or(leg.intended_price).unwrap_or(0.0);
        let open_f = open_qty.to_string().parse::<f64>().unwrap_or(0.0);
        if open_f <= 0.0 {
            notes.push(format!(
                "{}: no open YU size — leg already flat",
                leg.symbol
            ));
            legs.push(FixedYieldExitQuoteLegDto {
                leg_side: leg_side.to_string(),
                yu_symbol: leg.symbol.clone(),
                market_id: 0,
                open_qty: 0.0,
                entry_rate,
                exit_rate: None,
                est_exit_pnl_usd: Some(0.0),
            });
            continue;
        }
        let market = match boros.market_by_symbol(&leg.symbol).await {
            Ok(m) => m,
            Err(e) => {
                notes.push(format!("{}: market unavailable — {e}", leg.symbol));
                legs.push(FixedYieldExitQuoteLegDto {
                    leg_side: leg_side.to_string(),
                    yu_symbol: leg.symbol.clone(),
                    market_id: 0,
                    open_qty: open_f,
                    entry_rate,
                    exit_rate: None,
                    est_exit_pnl_usd: None,
                });
                continue;
            }
        };
        // Closing a long YU sells into the bid; closing a short buys the
        // ask. Missing side → unquotable, never guessed from mid.
        let exit_rate = match leg.side {
            crate::state_store::LegSide::Long => market.best_bid_apr,
            crate::state_store::LegSide::Short => market.best_ask_apr,
        };
        let est = exit_rate.map(|rx| {
            let diff = if leg.side == crate::state_store::LegSide::Long {
                rx - entry_rate
            } else {
                entry_rate - rx
            };
            diff * open_f * market.collateral_usd_price * years_remaining
        });
        legs.push(FixedYieldExitQuoteLegDto {
            leg_side: leg_side.to_string(),
            yu_symbol: leg.symbol.clone(),
            market_id: market.market_id,
            open_qty: open_f,
            entry_rate,
            exit_rate,
            est_exit_pnl_usd: est,
        });
    }
    let all_quoted = legs
        .iter()
        .all(|l| l.open_qty <= 0.0 || l.est_exit_pnl_usd.is_some());
    let est_total = if all_quoted {
        Some(legs.iter().filter_map(|l| l.est_exit_pnl_usd).sum())
    } else {
        notes.push(
            "one or more legs unquotable — total withheld (partial totals are misleading)"
                .to_string(),
        );
        None
    };
    Ok(FixedYieldExitQuoteDto {
        signal_id: yu_signal_id.to_string(),
        parent_signal_id: row.parent_signal_id,
        maturity_state: row.maturity_state,
        lock_until_ms: row.lock_until_ms,
        years_remaining,
        legs,
        est_total_pnl_usd: est_total,
        notes,
    })
}

/// Execute a priced early exit: re-quote at dispatch time (never trust a
/// stale quote), IOC each open YU leg at a bounded rate, poll to
/// terminal, then flatten the parent carry pair. Any YU leg that cannot
/// close leaves the pair OPEN and the row unresolved — a partially
/// unhedged unwind is more dangerous than no unwind.
pub async fn execute_fixed_yield_exit_impl(
    yu_signal_id: &str,
    state: &AppEngineState,
) -> Result<crate::execution::FlattenResultDto, String> {
    if !state.is_live() {
        return Err("fixed-yield exits are live-only — Boros has no testnet".into());
    }
    if !state.vault.write().is_unlocked() {
        return Err("vault is locked — cannot sign Boros exit orders".into());
    }
    let _execution_guard = state.execution_lock.lock().await;

    let quote = fixed_yield_exit_quote_impl(yu_signal_id, state).await?;
    let actionable: Vec<_> = quote.legs.iter().filter(|l| l.open_qty > 0.0).collect();
    if actionable.is_empty() {
        return Err("no open YU size to exit".into());
    }
    if let Some(l) = actionable.iter().find(|l| l.exit_rate.is_none()) {
        return Err(format!(
            "cannot price {} — no executable rate on the Boros book; refusing a blind exit",
            l.yu_symbol
        ));
    }

    let boros = boros_for_exec(state);
    let boros_adapter: Arc<dyn ExchangeAdapter> = boros.clone();
    let close_id = format!("flat-{}-{:04x}", yu_signal_id, rand::random::<u16>());
    let yu_signal = state
        .state_store
        .get_signal(yu_signal_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "signal vanished mid-exit".to_string())?;

    // Persist the close-child BEFORE dispatch — durable order identity
    // precedes order flow (same convention as the perp flatten path).
    let child_legs: Vec<crate::state_store::NewSignalLeg> = yu_signal
        .legs
        .iter()
        .zip(quote.legs.iter())
        .filter(|(_, q)| q.open_qty > 0.0)
        .map(|(leg, q)| crate::state_store::NewSignalLeg {
            account_id: leg.account_id.clone(),
            venue: "boros".to_string(),
            side: leg.side,
            symbol: Some(leg.symbol.clone()),
            intended_qty: q.open_qty,
            intended_price: q.exit_rate,
            unit_scale: None,
        })
        .collect();
    state
        .state_store
        .record_signal_execution(
            &crate::state_store::NewSignalExecution {
                id: close_id.clone(),
                strategy_id: FUNDING_STRATEGY_ID.to_string(),
                symbol: yu_signal.symbol.clone(),
                route_id: None,
                expected_net_usd: quote.est_total_pnl_usd,
                note: Some(format!("priced early exit of YU lock {yu_signal_id}")),
                legs: child_legs,
            },
            state.env().as_str(),
        )
        .map_err(|e| format!("Failed to persist YU exit signal record: {e}"))?;

    let mut exit_fills: Vec<(crate::state_store::LegSide, OrderResponse)> = Vec::new();
    let mut fatal: Option<String> = None;
    for (leg, q) in yu_signal.legs.iter().zip(quote.legs.iter()) {
        if q.open_qty <= 0.0 {
            continue;
        }
        let exit_rate = q.exit_rate.unwrap_or(0.0);
        let qty_dec = Decimal::from_f64_retain(q.open_qty)
            .ok_or_else(|| "YU exit qty not decimal-safe".to_string())?;
        let rate_dec = Decimal::from_f64_retain(exit_rate)
            .ok_or_else(|| "YU exit rate not decimal-safe".to_string())?;
        // Long YU exits by selling — bound 2% under the quoted bid so a
        // moved book still fails the IOC rather than filling through the
        // floor. Short exits by buying — 2% over ask.
        let (order_side, bound) = match leg.side {
            crate::state_store::LegSide::Long => (Side::Sell, rate_dec * dec!(0.98)),
            crate::state_store::LegSide::Short => (Side::Buy, rate_dec * dec!(1.02)),
        };
        let req = OrderRequest::new_ioc(
            VenueId::from("boros"),
            format!("BOROS-{}", q.market_id),
            order_side,
            bound,
            qty_dec,
        );
        let step = async {
            let mut resp = boros_adapter
                .place_order(req)
                .await
                .map_err(|e| format!("YU exit on {} rejected: {e}", leg.symbol))?;
            for _ in 0..YU_SETTLE_POLLS {
                if resp.status.is_final() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(YU_SETTLE_POLL_MS)).await;
                match boros_adapter
                    .fetch_order(&format!("BOROS-{}", q.market_id), &resp.order_id, None)
                    .await
                {
                    Ok(polled) => resp = polled,
                    Err(_) => continue,
                }
            }
            Ok::<OrderResponse, String>(resp)
        }
        .await;
        match step {
            Ok(resp) => {
                if resp.filled_qty <= dec!(0) {
                    fatal = Some(format!(
                        "YU exit for {} filled nothing ({:?}) — residual position remains",
                        leg.symbol, resp.status
                    ));
                    exit_fills.push((leg.side, resp));
                    break;
                }
                exit_fills.push((leg.side, resp));
            }
            Err(e) => {
                fatal = Some(e);
                break;
            }
        }
    }

    for (side, fill) in &exit_fills {
        if !fill.order_id.is_empty() {
            let _ = state.state_store.record_leg_order_id(
                &close_id,
                *side,
                fill.order_id.as_str(),
                None,
            );
        }
        let filled = fill.filled_qty.to_string().parse::<f64>().unwrap_or(0.0);
        let px = fill
            .avg_fill_price
            .and_then(|p| p.to_string().parse::<f64>().ok())
            .unwrap_or(0.0);
        if filled > 0.0 && px > 0.0 {
            let fee = fill
                .fee
                .map(|v| v.to_string().parse::<f64>().unwrap_or(0.0))
                .unwrap_or(0.0);
            let _ = state.state_store.apply_leg_fill(
                &close_id,
                *side,
                filled,
                px,
                fee,
                Some(fill.order_id.as_str()),
            );
        }
    }

    if let Some(fatal) = fatal {
        let _ = state.state_store.update_signal_status(
            &close_id,
            crate::state_store::SignalStatus::Unresolved,
            Some(&fatal),
        );
        return Err(format!(
            "YU early exit incomplete: {fatal} — perp pair left open; reconcile or retry the exit"
        ));
    }

    // All YU legs exited and are verified by venue truth — mark the lock
    // signal closed BEFORE the perp flatten so its residual_signals count
    // reflects only the carry pair (open siblings would inflate it). Then
    // release the execution lock: the flatten acquires the same
    // non-reentrant mutex internally and would self-deadlock.
    let _ = state.state_store.update_signal_status(
        &close_id,
        crate::state_store::SignalStatus::Closed,
        None,
    );
    let _ = state.state_store.update_signal_status(
        yu_signal_id,
        crate::state_store::SignalStatus::Closed,
        Some("YU lock exited early at quoted rates"),
    );
    drop(_execution_guard);
    let parent = quote
        .parent_signal_id
        .ok_or_else(|| "maturity row has no parent carry signal".to_string())?;
    let flat = crate::execution::flatten_signal_positions_impl(
        &parent,
        FUNDING_STRATEGY_ID,
        None,
        arbitrageur_execution::virtual_ledger::CycleStatus::Closed,
        state,
    )
    .await?;
    let _ = state.state_store.mark_fixed_yield_closed_early(
        yu_signal_id,
        "priced early exit — all YU legs closed via bounded IOC",
    );
    Ok(flat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrageur_exchanges::BorosYuMarket;

    fn yu_mkt(
        market_id: u64,
        platform: &str,
        asset: &str,
        maturity: i64,
        bid: f64,
        ask: f64,
    ) -> BorosYuMarket {
        BorosYuMarket {
            market_id,
            token_id: 3,
            symbol: format!("{}-{}USDT-TEST", platform.to_uppercase(), asset),
            platform: platform.to_string(),
            asset: asset.to_string(),
            maturity_unix: maturity,
            tick_step: 2,
            mid_apr: (bid + ask) / 2.0,
            best_bid_apr: Some(bid),
            best_ask_apr: Some(ask),
            floating_apr: 0.05,
            notional_oi_usd: 500_000.0,
            taker_fee_rate: 0.0005,
            payment_period_secs: 28800,
            max_leverage: 3.0,
            collateral_symbol: "USDT".to_string(),
            collateral_usd_price: 1.0,
        }
    }

    fn snap(markets: Vec<BorosYuMarket>) -> Option<(Arc<Vec<BorosYuMarket>>, f64)> {
        Some((Arc::new(markets), 0.10))
    }

    /// A carry pair whose venues both host same-asset Boros markets at a
    /// shared maturity emits one `fixed_yield` row carrying both YU legs.
    #[test]
    fn fixed_yield_variant_emits_when_markets_align() {
        let state = AppEngineState::new();
        let mat = Utc::now().timestamp() + 30 * 86_400;
        // Long perp on Binance → LONG YU pays the Binance ask (3%);
        // short perp on Hyperliquid → SHORT YU receives the HL bid (8%).
        // Locked spread = 0.08 − 0.03 = 5% for 30 days: $50k × 5% ×
        // 30/365 ≈ $205 gross vs ≈$100 all-in leg costs → profitable.
        let yu = snap(vec![
            yu_mkt(11, "Binance", "BTC", mat, 0.029, 0.030),
            yu_mkt(22, "Hyperliquid", "BTC", mat, 0.080, 0.081),
        ]);
        let row = boros_fixed_yield_variant(
            &state,
            &yu,
            "BTC-USDC",
            "Binance",
            "Hyperliquid",
            "binance",
            "hyperliquid",
            50_000.0,
            8.0,
            Some(2.0),
            50_000.0,
        )
        .expect("aligned markets emit a fixed-yield row");
        assert_eq!(row.kind, "fixed_yield");
        assert_eq!(row.id, "BTC-USDC-Binance-Hyperliquid-FIXED");
        let legs = row.yu_legs.expect("two YU legs recorded");
        assert_eq!(legs.len(), 2);
        assert_eq!(legs[0].yu_side, "long");
        assert_eq!(legs[0].yu_market_id, 11);
        assert_eq!(legs[1].yu_side, "short");
        assert_eq!(legs[1].yu_market_id, 22);
        assert!(row.fixed_apr_pct.unwrap() > 0.0);
        assert!(row.boros_margin_usd.unwrap() > 0.0);
        assert_eq!(row.lock_until_ms, Some(mat * 1000));
    }

    /// Mismatched maturities leave a residual floating window — no row.
    #[test]
    fn fixed_yield_variant_rejects_maturity_mismatch() {
        let state = AppEngineState::new();
        let mat = Utc::now().timestamp() + 7 * 86_400;
        let yu = snap(vec![
            yu_mkt(11, "Binance", "BTC", mat, 0.029, 0.030),
            yu_mkt(22, "Hyperliquid", "BTC", mat + 14 * 86_400, 0.080, 0.081),
        ]);
        assert!(boros_fixed_yield_variant(
            &state,
            &yu,
            "BTC-USDC",
            "Binance",
            "Hyperliquid",
            "binance",
            "hyperliquid",
            5_000.0,
            8.0,
            Some(2.0),
            5_000.0,
        )
        .is_none());
    }

    /// A locked spread that cannot cover four legs' costs emits nothing.
    #[test]
    fn fixed_yield_variant_rejects_unprofitable_spread() {
        let state = AppEngineState::new();
        let mat = Utc::now().timestamp() + 2 * 86_400; // short lock too
        let yu = snap(vec![
            yu_mkt(11, "Binance", "BTC", mat, 0.029, 0.030),
            yu_mkt(22, "Hyperliquid", "BTC", mat, 0.0305, 0.031), // ~5bps edge
        ]);
        assert!(boros_fixed_yield_variant(
            &state,
            &yu,
            "BTC-USDC",
            "Binance",
            "Hyperliquid",
            "binance",
            "hyperliquid",
            5_000.0,
            8.0,
            Some(2.0),
            5_000.0,
        )
        .is_none());
    }

    /// A venue with no admitted Boros market (Gate) pairs fail closed.
    #[test]
    fn fixed_yield_variant_rejects_unmapped_platform() {
        let state = AppEngineState::new();
        let mat = Utc::now().timestamp() + 7 * 86_400;
        let yu = snap(vec![
            yu_mkt(11, "Gate", "BTC", mat, 0.029, 0.030),
            yu_mkt(22, "Hyperliquid", "BTC", mat, 0.080, 0.081),
        ]);
        assert!(boros_fixed_yield_variant(
            &state,
            &yu,
            "BTC-USDC",
            "Gate",
            "Hyperliquid",
            "gate",
            "hyperliquid",
            5_000.0,
            8.0,
            Some(2.0),
            5_000.0,
        )
        .is_none());
    }

    /// No Boros snapshot (public API down) → no rows, scan unaffected.
    #[test]
    fn fixed_yield_variant_fail_open_without_snapshot() {
        let state = AppEngineState::new();
        assert!(boros_fixed_yield_variant(
            &state,
            &None,
            "BTC-USDC",
            "Binance",
            "Hyperliquid",
            "binance",
            "hyperliquid",
            5_000.0,
            8.0,
            Some(2.0),
            5_000.0,
        )
        .is_none());
    }

    // ------------------------------------------------------------------
    // Fixed-yield executor e2e (#27d): scripted perp venues drive the
    // two-leg coordinator; a stub-socket Boros responder serves the real
    // HTTP wire surface (markets, calldata-builder, bulk-calls, order
    // polling) so the YU hedge path runs end-to-end without credentials.
    // ------------------------------------------------------------------

    use arbitrageur_core::market::MarketMetadata;
    use arbitrageur_core::order::CancelRequest;
    use arbitrageur_core::position::AccountSnapshot;
    use arbitrageur_core::quote::{BookLevel, FundingRate, Ticker};
    use arbitrageur_exchanges::boros::BorosAdapter;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Perp venue mock for the two-leg coordinator: always-fill IOC
    /// responses, a fixed tight book, deep account. Order calls are
    /// recorded so tests can assert the emergency unwind fired.
    struct ScriptedPerpVenue {
        vid: &'static str,
        display: &'static str,
        bid: Decimal,
        ask: Decimal,
        rate_8h: Decimal,
        placed: StdMutex<Vec<(String, Side, Decimal)>>,
        place_seq: AtomicUsize,
        fail_place: std::sync::atomic::AtomicBool,
    }

    impl ScriptedPerpVenue {
        fn new(vid: &'static str, display: &'static str, rate_8h: Decimal) -> Self {
            Self {
                vid,
                display,
                bid: dec!(99.99),
                ask: dec!(100.01),
                rate_8h,
                placed: StdMutex::new(Vec::new()),
                place_seq: AtomicUsize::new(0),
                fail_place: std::sync::atomic::AtomicBool::new(false),
            }
        }

        fn book(&self, symbol: &str) -> OrderBook {
            OrderBook {
                venue: VenueId::new(self.vid),
                symbol: symbol.to_string(),
                bids: vec![BookLevel::new(self.bid, dec!(10000))],
                asks: vec![BookLevel::new(self.ask, dec!(10000))],
                timestamp: Utc::now(),
                sequence: 1,
            }
        }
    }

    #[async_trait::async_trait]
    impl ExchangeAdapter for ScriptedPerpVenue {
        fn venue_id(&self) -> VenueId {
            VenueId::new(self.vid)
        }
        fn name(&self) -> &str {
            self.display
        }
        async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
            Ok(Vec::new())
        }
        async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
            Ok(self.book(symbol))
        }
        async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
            Ok(Ticker {
                venue: VenueId::new(self.vid),
                symbol: symbol.to_string(),
                bid: self.bid,
                ask: self.ask,
                last: dec!(100),
                timestamp: Utc::now(),
            })
        }
        async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate> {
            Ok(FundingRate {
                venue: VenueId::new(self.vid),
                symbol: symbol.to_string(),
                rate: self.rate_8h,
                predicted_rate: None,
                interval_hours: 8,
                next_funding_time: Utc::now(),
                timestamp: Utc::now(),
            })
        }
        async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
            Ok(AccountSnapshot {
                venue: VenueId::new(self.vid),
                equity: dec!(100000),
                free_margin: dec!(100000),
                maintenance_margin: dec!(0),
                margin_usage_pct: dec!(0),
                balances: vec![],
                positions: vec![],
                timestamp: Utc::now(),
            })
        }
        async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
            if self.fail_place.load(Ordering::SeqCst) {
                anyhow::bail!("{} venue rejected the order", self.vid);
            }
            let n = self.place_seq.fetch_add(1, Ordering::SeqCst) + 1;
            self.placed
                .lock()
                .unwrap()
                .push((order.symbol.clone(), order.side, order.quantity));
            Ok(OrderResponse {
                order_id: format!("MOCK-{}-{n}", self.vid),
                client_order_id: order.client_order_id.clone(),
                venue: VenueId::new(self.vid),
                symbol: order.symbol.clone(),
                status: OrderStatus::Filled,
                original_qty: order.quantity,
                filled_qty: order.quantity,
                remaining_qty: dec!(0),
                // Fill at the mid (price improvement on the IOC bound) so
                // both perp legs book identical notional — equal YU sizes
                // are what the balanced-signal check requires.
                avg_fill_price: Some(dec!(100)),
                fee: Some(dec!(0.05)),
                fee_currency: "USD".into(),
                raw_response: None,
                timestamp: Utc::now(),
            })
        }
        async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
            Ok(())
        }
        async fn fetch_order(
            &self,
            symbol: &str,
            order_id: &str,
            client_order_id: Option<&str>,
        ) -> anyhow::Result<OrderResponse> {
            Ok(OrderResponse {
                order_id: order_id.to_string(),
                client_order_id: client_order_id.unwrap_or("").to_string(),
                venue: VenueId::new(self.vid),
                symbol: symbol.to_string(),
                status: OrderStatus::Filled,
                original_qty: dec!(0),
                filled_qty: dec!(0),
                remaining_qty: dec!(0),
                avg_fill_price: Some(dec!(100)),
                fee: None,
                fee_currency: "USD".into(),
                raw_response: None,
                timestamp: Utc::now(),
            })
        }
        fn supports_live_execution(&self) -> bool {
            true
        }
        fn live_order_submission_implemented(&self) -> bool {
            true
        }
        fn order_status_polling_implemented(&self) -> bool {
            true
        }
        fn live_strategy_entry_implemented(&self) -> bool {
            true
        }
        fn has_live_credentials(&self) -> bool {
            true
        }
    }

    /// Boros wire stub: serves the REST surface the adapter actually calls
    /// (assets, markets, agent approval, calldata-builder, bulk-calls, order
    /// resolution + polling). Order lifecycle is scripted per place-order
    /// call; every submitted tx hash resolves to its order row.
    struct BorosStubOrder {
        market_id: u64,
        order_id: String,
        placed_size_x18: String,
        ack_status: u64,
        polls: VecDeque<u64>,
        cancelled: bool,
    }

    /// (ack_status, poll_statuses) consumed per place-order POST; a
    /// `None` script slot simulates a transport failure.
    type StubPlaceScript = Option<(u64, VecDeque<u64>)>;

    struct BorosStub {
        url: String,
        orders: Arc<StdMutex<Vec<BorosStubOrder>>>,
        tx_to_order: Arc<StdMutex<HashMap<String, usize>>>,
        place_scripts: Arc<StdMutex<VecDeque<StubPlaceScript>>>,
        pending_cancel: Arc<StdMutex<Option<usize>>>,
        place_posts: Arc<AtomicUsize>,
        tx_seq: Arc<AtomicUsize>,
        /// Positions the account read reports — maturity/settlement tests
        /// set these to simulate a residual or fully-settled YU book.
        positions: Arc<StdMutex<Vec<serde_json::Value>>>,
        /// Gas-treasury field on the account row (USD) — None = venue
        /// reports no balance.
        gas_usd: Arc<StdMutex<Option<serde_json::Value>>>,
    }

    fn boros_market_json(market_id: u64, platform: &str, maturity: i64) -> serde_json::Value {
        serde_json::json!({
            "marketId": market_id,
            "tokenId": 3,
            "platform": {"name": platform, "platformId": platform},
            "imData": {
                // Match `yu_mkt`'s cache-seeded symbols — the ledger legs
                // persist those, and the exit path re-resolves by symbol.
                "symbol": format!("{}-BTCUSDT-TEST", platform.to_uppercase()),
                "maturity": maturity,
                "tickStep": 2,
            },
            "metadata": {
                "underlyingSymbol": "BTC",
                "maxLeverage": 3.0,
            },
            "data": {
                // APR fields parse as plain fractions (the live venue
                // returns numbers, not x18 strings).
                "midApr": 0.08,
                "bestBid": 0.15,
                "bestAsk": 0.03,
                "floatingApr": 0.05,
                "notionalOI": "500000",
            },
            "config": {"status": 2, "takerFee": "500000000000000"},
            "extConfig": {"paymentPeriod": 28800},
        })
    }

    fn boros_order_row(o: &BorosStubOrder, status: u64) -> serde_json::Value {
        let filled = status == 2;
        serde_json::json!({
            "orderId": o.order_id,
            "marketId": o.market_id,
            "placedSize": o.placed_size_x18,
            "unfilledSize": if filled { "0".to_string() } else { o.placed_size_x18.clone() },
            "status": status,
        })
    }

    fn boros_stub_dispatch(
        method: &str,
        path: &str,
        body: &str,
        stub: &BorosStub,
    ) -> (u16, serde_json::Value) {
        if path.starts_with("/v1/assets") {
            return (
                200,
                serde_json::json!({"results": [
                    {"tokenId": 3, "symbol": "USDT", "isCollateral": true, "usdPrice": 1.0}
                ]}),
            );
        }
        if path.starts_with("/v1/markets") {
            let maturity = Utc::now().timestamp() + 30 * 86_400;
            return (
                200,
                serde_json::json!({"results": [
                    boros_market_json(11, "Binance", maturity),
                    boros_market_json(22, "Hyperliquid", maturity),
                ]}),
            );
        }
        if path.starts_with("/v1/agents/expiry-time") {
            return (
                200,
                serde_json::json!({"expiryTime": Utc::now().timestamp() + 86_400}),
            );
        }
        if method == "POST" && path == "/v1/accounts/market-acc-infos" {
            // marketAcc deliberately unparseable → the adapter falls back
            // to the request's tokenIds (token 3 = USDT @ $1) for the
            // collateral USD conversion.
            let mut row = serde_json::json!({
                "marketAcc": "0xZZZ",
                "netBalance": "1000000000000000000000",
                "availableInitialMargin": "100000000000000000000000",
                "positions": stub.positions.lock().unwrap().clone(),
            });
            if let Some(g) = stub.gas_usd.lock().unwrap().clone() {
                row["gasBalanceUsd"] = g;
            }
            return (200, serde_json::json!({"results": [row]}));
        }
        if method == "POST" && path == "/v1/calldata-builder/agent/place-order" {
            stub.place_posts.fetch_add(1, Ordering::SeqCst);
            let script = stub.place_scripts.lock().unwrap().pop_front().flatten();
            let Some((ack_status, polls)) = script else {
                // Missing script = simulated transport failure.
                return (
                    500,
                    serde_json::json!({"error": "stub place-order failure"}),
                );
            };
            let v: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
            let market_id = v.get("marketId").and_then(|m| m.as_u64()).unwrap_or(0);
            let size = v
                .get("size")
                .and_then(|s| s.as_str())
                .unwrap_or("0")
                .to_string();
            let mut orders = stub.orders.lock().unwrap();
            let idx = orders.len();
            orders.push(BorosStubOrder {
                market_id,
                order_id: format!("O{idx}"),
                placed_size_x18: size,
                ack_status,
                polls,
                cancelled: false,
            });
            return (
                200,
                serde_json::json!({"calls": [{"calldata": "0xdeadbeef", "accountId": 0}]}),
            );
        }
        if method == "POST" && path == "/v1/calldata-builder/agent/cancel-orders" {
            let v: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
            if let Some(ids) = v
                .get("markets")
                .and_then(|m| m.as_array())
                .and_then(|m| m.first())
                .and_then(|m| m.get("orderIds"))
                .and_then(|i| i.as_array())
            {
                let mut orders = stub.orders.lock().unwrap();
                for (i, o) in orders.iter_mut().enumerate() {
                    if ids.iter().any(|id| {
                        id.as_str().map(|s| s == o.order_id).unwrap_or(false)
                            || id
                                .as_u64()
                                .map(|n| n.to_string() == o.order_id)
                                .unwrap_or(false)
                    }) {
                        o.cancelled = true;
                        *stub.pending_cancel.lock().unwrap() = Some(i);
                    }
                }
            }
            return (
                200,
                serde_json::json!({"calls": [{"calldata": "0xcancel", "accountId": 0}]}),
            );
        }
        if method == "POST" && path == "/v1/send-txs/bulk-calls" {
            let n = stub.tx_seq.fetch_add(1, Ordering::SeqCst);
            let tx = format!("0x{:064x}", n + 1);
            // A cancel batch targets an existing order; a place batch opens
            // the most recently built one.
            let idx = stub
                .pending_cancel
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| stub.orders.lock().unwrap().len().saturating_sub(1));
            stub.tx_to_order.lock().unwrap().insert(tx.clone(), idx);
            return (
                200,
                serde_json::json!([{"status": "success", "txHash": tx}]),
            );
        }
        if path.starts_with("/v1/orders/by-tx-hash") {
            let tx = path
                .split("placedTxHash=")
                .nth(1)
                .and_then(|s| s.split('&').next())
                .unwrap_or("");
            let map = stub.tx_to_order.lock().unwrap();
            let orders = stub.orders.lock().unwrap();
            if let Some(&idx) = map.get(tx) {
                if let Some(o) = orders.get(idx) {
                    let status = if o.cancelled { 1 } else { o.ack_status };
                    return (200, serde_json::json!([boros_order_row(o, status)]));
                }
            }
            return (200, serde_json::json!([]));
        }
        if path.starts_with("/v1/accounts/orders") {
            let market_id: u64 = path
                .split("marketId=")
                .nth(1)
                .and_then(|s| s.split('&').next())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let mut orders = stub.orders.lock().unwrap();
            let rows: Vec<serde_json::Value> = orders
                .iter_mut()
                .filter(|o| o.market_id == market_id)
                .map(|o| {
                    let status = if o.cancelled {
                        1
                    } else {
                        o.polls.pop_front().unwrap_or(o.ack_status)
                    };
                    boros_order_row(o, status)
                })
                .collect();
            return (200, serde_json::json!(rows));
        }
        (
            404,
            serde_json::json!({"error": format!("unhandled: {method} {path}")}),
        )
    }

    async fn spawn_boros_stub(place_scripts: VecDeque<Option<(u64, VecDeque<u64>)>>) -> BorosStub {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let stub = BorosStub {
            url: format!("http://127.0.0.1:{port}"),
            orders: Arc::new(StdMutex::new(Vec::new())),
            tx_to_order: Arc::new(StdMutex::new(HashMap::new())),
            place_scripts: Arc::new(StdMutex::new(place_scripts)),
            pending_cancel: Arc::new(StdMutex::new(None)),
            place_posts: Arc::new(AtomicUsize::new(0)),
            tx_seq: Arc::new(AtomicUsize::new(0)),
            positions: Arc::new(StdMutex::new(Vec::new())),
            gas_usd: Arc::new(StdMutex::new(Some(serde_json::json!(500.0)))),
        };
        let orders = stub.orders.clone();
        let tx_map = stub.tx_to_order.clone();
        let scripts = stub.place_scripts.clone();
        let pending_cancel = stub.pending_cancel.clone();
        let place_posts = stub.place_posts.clone();
        let tx_seq = stub.tx_seq.clone();
        let positions = stub.positions.clone();
        let gas_usd = stub.gas_usd.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let inner = BorosStub {
                    url: String::new(),
                    orders: orders.clone(),
                    tx_to_order: tx_map.clone(),
                    place_scripts: scripts.clone(),
                    pending_cancel: pending_cancel.clone(),
                    place_posts: place_posts.clone(),
                    tx_seq: tx_seq.clone(),
                    positions: positions.clone(),
                    gas_usd: gas_usd.clone(),
                };
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let mut lines = req.lines();
                    let first = lines.next().unwrap_or("");
                    let mut parts = first.split_whitespace();
                    let method = parts.next().unwrap_or("");
                    let path = parts.next().unwrap_or("");
                    // Body follows the blank line; Content-Length governs.
                    let body = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
                    let (code, resp) = boros_stub_dispatch(method, path, &body, &inner);
                    let status = if code == 200 { "200 OK" } else { "500 ERR" };
                    let payload = resp.to_string();
                    let out = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
                        payload.len(),
                    );
                    let _ = sock.write_all(out.as_bytes()).await;
                });
            }
        });
        stub
    }

    /// Live-env state with scripted perp venues (Binance long / Hyperliquid
    /// short at divergent funding) + a stub-socket Boros adapter + seeded
    /// hub funding/books and a warm YU market cache — the exact inputs the
    /// scan and the executor each consume.
    async fn fixed_yield_state(
        tag: &str,
        stub: &BorosStub,
    ) -> (
        AppEngineState,
        Arc<ScriptedPerpVenue>,
        Arc<ScriptedPerpVenue>,
    ) {
        let dir =
            std::env::temp_dir().join(format!("arbitrageur_fy_{tag}_{}", rand::random::<u64>()));
        let state = AppEngineState::new_with_vault_path(dir.join("vault.enc"));
        state.set_environment(crate::state::TradingEnvironment::Live);
        crate::accounts::initialize_vault_impl("test password", &state).expect("init vault");
        {
            let mut s = state.settings.write();
            s.binance_enabled = true;
            s.hyperliquid_enabled = true;
        }
        let long = Arc::new(ScriptedPerpVenue::new("binance", "Binance", dec!(-0.0005)));
        let short = Arc::new(ScriptedPerpVenue::new(
            "hyperliquid",
            "Hyperliquid",
            dec!(0.0015),
        ));
        state.test_adapters.write().unwrap().insert(
            "binance".to_string(),
            long.clone() as Arc<dyn ExchangeAdapter>,
        );
        state.test_adapters.write().unwrap().insert(
            "hyperliquid".to_string(),
            short.clone() as Arc<dyn ExchangeAdapter>,
        );
        // The live gate resolves "boros" through the dyn registry; the YU
        // order path itself uses the concrete stub adapter below.
        state.test_adapters.write().unwrap().insert(
            "boros".to_string(),
            Arc::new(ScriptedPerpVenue::new("boros", "Boros", dec!(0))) as Arc<dyn ExchangeAdapter>,
        );
        let boros = BorosAdapter::new(Some(stub.url.clone()));
        boros
            .update_credentials(
                Some("0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80".into()),
                Some("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266".into()),
                Some("0".into()),
            )
            .expect("boros test creds");
        *state.test_boros.write().unwrap() = Some(Arc::new(boros));

        // Hub seeds the scan consumes (executor rescan is network-fetch=off).
        let sym = "BTC-USDC";
        state.market_data_hub.update_funding(FundingRate {
            venue: VenueId::new("binance"),
            symbol: sym.into(),
            rate: dec!(-0.0005),
            predicted_rate: None,
            interval_hours: 8,
            next_funding_time: Utc::now(),
            timestamp: Utc::now(),
        });
        state.market_data_hub.update_funding(FundingRate {
            venue: VenueId::new("hyperliquid"),
            symbol: sym.into(),
            rate: dec!(0.0015),
            predicted_rate: None,
            interval_hours: 8,
            next_funding_time: Utc::now(),
            timestamp: Utc::now(),
        });
        state.market_data_hub.update_orderbook(long.book(sym));
        state.market_data_hub.update_orderbook(short.book(sym));

        // Warm YU cache: Binance YU ask 3% (long pays), Hyperliquid YU bid
        // 15% (short receives) → ~12% locked APR over a 30d maturity.
        let maturity = Utc::now().timestamp() + 30 * 86_400;
        *state.boros_yu_cache.lock() = Some(crate::state::BorosYuCache {
            fetched_at: std::time::Instant::now(),
            markets: Arc::new(vec![
                yu_mkt(11, "Binance", "BTC", maturity, 0.029, 0.030),
                yu_mkt(22, "Hyperliquid", "BTC", maturity, 0.150, 0.151),
            ]),
            gas_usd: 0.10,
        });
        (state, long, short)
    }

    /// Scan → pick the fixed_yield row → execute with its display triple.
    async fn run_fixed_yield(state: &AppEngineState) -> Result<SimStepResultDto, String> {
        let opps = scan_funding_opportunities_impl("BTC-USDC", 50_000.0, state).await?;
        let row = opps
            .iter()
            .find(|o| o.kind == "fixed_yield")
            .cloned()
            .ok_or_else(|| {
                format!(
                    "no fixed_yield row emitted; opps={:?}",
                    opps.iter().map(|o| o.kind.clone()).collect::<Vec<_>>()
                )
            })?;
        execute_funding_carry_impl(
            "BTC-USDC",
            &row.long_venue,
            &row.short_venue,
            50_000.0,
            state,
            false,
            Some("fixed_yield"),
        )
        .await
    }

    /// Happy path: both perp legs fill through the coordinator, both YU
    /// orders ack FILLED at the venue — four real legs, two balanced
    /// signals, hedged-to-maturity note in the message.
    #[tokio::test]
    async fn fixed_yield_e2e_all_four_legs_fill() {
        let mut scripts = VecDeque::new();
        scripts.push_back(Some((2, VecDeque::new()))); // long YU: ack Filled
        scripts.push_back(Some((2, VecDeque::new()))); // short YU: ack Filled
        let stub = spawn_boros_stub(scripts).await;
        let (state, _long, _short) = fixed_yield_state("fill", &stub).await;
        let res = run_fixed_yield(&state).await.expect("fixed-yield executes");
        assert!(res.execution_success);
        assert!(
            res.message.contains("hedge locked"),
            "expected locked-hedge note, got: {}",
            res.message
        );
        assert_eq!(stub.place_posts.load(Ordering::SeqCst), 2);
        let signals = state
            .state_store
            .list_signals(Some("BTC-USDC"), None)
            .expect("list signals");
        assert_eq!(signals.len(), 2, "carry + YU sibling signals");
        for s in &signals {
            assert_eq!(
                s.status,
                crate::state_store::SignalStatus::Balanced,
                "signal {} must reconcile Balanced",
                s.id
            );
        }
        assert!(signals.iter().any(|s| s.id.ends_with("-yu")));
    }

    /// YU leg acks non-terminal (Filling) and never resolves within the
    /// settle budget → fail closed: perp pair emergency-unwound, both
    /// signals Failed, no second YU leg stacked on top.
    #[tokio::test]
    async fn fixed_yield_e2e_unresolved_yu_unwinds_perps() {
        let mut scripts = VecDeque::new();
        scripts.push_back(Some((0, VecDeque::from([0; 16])))); // stays Filling forever
        let stub = spawn_boros_stub(scripts).await;
        let (state, long_perp, _short) = fixed_yield_state("never", &stub).await;
        let err = run_fixed_yield(&state)
            .await
            .expect_err("unresolved YU must fail");
        assert!(
            err.contains("never resolved") || err.contains("unwound to flat"),
            "expected unresolved/unwound error, got: {err}"
        );
        // Only one YU place-order went out — no stacking.
        assert_eq!(stub.place_posts.load(Ordering::SeqCst), 1);
        // Both perp mocks took an emergency exit order (2nd order each).
        assert!(
            long_perp.placed.lock().unwrap().len() >= 2,
            "long perp must see the entry buy + emergency exit"
        );
        let signals = state
            .state_store
            .list_signals(Some("BTC-USDC"), None)
            .expect("list signals");
        assert!(
            signals
                .iter()
                .all(|s| s.status == crate::state_store::SignalStatus::Failed),
            "both signals must be Failed after the unwind"
        );
    }

    /// YU leg acks a terminal reject (Expired) with zero fills → fatal: perp
    /// pair unwound, sibling signals Failed.
    #[tokio::test]
    async fn fixed_yield_e2e_expired_yu_unwinds_perps() {
        let mut scripts = VecDeque::new();
        scripts.push_back(Some((3, VecDeque::new()))); // Expired, zero fill
        let stub = spawn_boros_stub(scripts).await;
        let (state, _long, _short) = fixed_yield_state("expired", &stub).await;
        let err = run_fixed_yield(&state)
            .await
            .expect_err("expired YU must fail");
        assert!(
            err.contains("filled nothing") || err.contains("unwound to flat"),
            "expected zero-fill failure, got: {err}"
        );
        assert_eq!(stub.place_posts.load(Ordering::SeqCst), 1);
        let signals = state
            .state_store
            .list_signals(Some("BTC-USDC"), None)
            .expect("list signals");
        assert!(
            signals
                .iter()
                .all(|s| s.status == crate::state_store::SignalStatus::Failed),
            "both signals must be Failed after the unwind"
        );
    }

    /// First YU leg fills, second's place-order POST fails at the wire →
    /// fatal: perps unwind AND the filled YU side gets an IOC exit (a third
    /// place-order call carrying the opposite side).
    #[tokio::test]
    async fn fixed_yield_e2e_second_yu_failure_exits_filled_leg() {
        let mut scripts = VecDeque::new();
        scripts.push_back(Some((2, VecDeque::new()))); // leg 1 fills
        scripts.push_back(None); // leg 2: transport failure
        scripts.push_back(Some((2, VecDeque::new()))); // leg-1 IOC exit fills
        let stub = spawn_boros_stub(scripts).await;
        let (state, _long, _short) = fixed_yield_state("leg2fail", &stub).await;
        let err = run_fixed_yield(&state)
            .await
            .expect_err("second-leg failure must fail the entry");
        assert!(
            err.contains("rejected") || err.contains("unwound to flat"),
            "expected rejection + unwind, got: {err}"
        );
        assert_eq!(
            stub.place_posts.load(Ordering::SeqCst),
            3,
            "place leg1 + failed leg2 + IOC exit of leg1"
        );
        let signals = state
            .state_store
            .list_signals(Some("BTC-USDC"), None)
            .expect("list signals");
        assert!(
            signals
                .iter()
                .all(|s| s.status == crate::state_store::SignalStatus::Failed),
            "both signals must be Failed after the unwind"
        );
    }

    /// fixed_yield is live-only (Boros has no testnet) — testnet env
    /// refuses before any scan or dispatch.
    #[tokio::test]
    async fn fixed_yield_refuses_testnet_env() {
        let state = AppEngineState::new();
        let err = execute_funding_carry_impl(
            "BTC-USDC",
            "Binance",
            "Hyperliquid",
            50_000.0,
            &state,
            false,
            Some("fixed_yield"),
        )
        .await
        .expect_err("testnet fixed_yield must refuse");
        assert!(err.contains("live-only"), "unexpected error: {err}");
    }

    /// Autonomous fixed-yield entries require the explicit opt-in: with
    /// `auto_fixed_yield_enabled` unset, `auto` refuses fast with the
    /// named remediation — no scan, no dispatch.
    #[tokio::test]
    async fn fixed_yield_refuses_auto_in_live() {
        let dir =
            std::env::temp_dir().join(format!("arbitrageur_fy_auto_{}", rand::random::<u64>()));
        let state = AppEngineState::new_with_vault_path(dir.join("vault.enc"));
        state.set_environment(crate::state::TradingEnvironment::Live);
        let err = execute_funding_carry_impl(
            "BTC-USDC",
            "Binance",
            "Hyperliquid",
            50_000.0,
            &state,
            true,
            Some("fixed_yield"),
        )
        .await
        .expect_err("auto fixed_yield in live must refuse");
        assert!(err.contains("disabled"), "unexpected error: {err}");
    }

    /// Entry persists a maturity tracker on the `-yu` sibling — the row
    /// carries the venue lock_until and both market ids.
    #[tokio::test]
    async fn fixed_yield_entry_records_maturity_row() {
        let mut scripts = VecDeque::new();
        scripts.push_back(Some((2, VecDeque::new())));
        scripts.push_back(Some((2, VecDeque::new())));
        let stub = spawn_boros_stub(scripts).await;
        let (state, _l, _s) = fixed_yield_state("maturity", &stub).await;
        run_fixed_yield(&state).await.expect("fixed-yield executes");
        let rows = state
            .state_store
            .list_fixed_yield_maturities()
            .expect("list maturities");
        assert_eq!(rows.len(), 1, "one maturity row per YU hedge");
        let row = &rows[0];
        assert!(row.signal_id.ends_with("-yu"));
        assert_eq!(row.maturity_state, "locked");
        assert!(row.lock_until_ms > Utc::now().timestamp_millis());
        assert_eq!(row.yu_market_ids.len(), 2);
        assert!(row.parent_signal_id.is_some());
        assert!(row.resolved_action.is_none());
    }

    /// The sweep never settles on wall-clock alone: pre-maturity stays
    /// `locked`; past-maturity with a residual venue position is
    /// `settlement_unverified`; only a clean venue read confirms
    /// `settled`.
    #[tokio::test]
    async fn maturity_sweep_is_venue_truth_only() {
        let mut scripts = VecDeque::new();
        scripts.push_back(Some((2, VecDeque::new())));
        scripts.push_back(Some((2, VecDeque::new())));
        let stub = spawn_boros_stub(scripts).await;
        let (state, _l, _s) = fixed_yield_state("sweep", &stub).await;
        run_fixed_yield(&state).await.expect("fixed-yield executes");
        let yu_id = state.state_store.list_fixed_yield_maturities().unwrap()[0]
            .signal_id
            .clone();

        // Pre-maturity: no venue call, stays locked.
        check_fixed_yield_maturities(&state).await;
        let row = &state.state_store.list_fixed_yield_maturities().unwrap()[0];
        assert_eq!(row.maturity_state, "locked");

        // Past lock_until + residual position → settlement_unverified.
        let past = Utc::now().timestamp_millis() - 60_000;
        state
            .state_store
            .record_fixed_yield_maturity(
                &yu_id,
                row.parent_signal_id.as_deref().unwrap(),
                past,
                &[11, 22],
            )
            .unwrap();
        *stub.positions.lock().unwrap() = vec![serde_json::json!({
            "marketId": 11, "signedSize": "1000000000000000000",
        })];
        check_fixed_yield_maturities(&state).await;
        let row = &state.state_store.list_fixed_yield_maturities().unwrap()[0];
        assert_eq!(row.maturity_state, "settlement_unverified");
        assert!(
            row.venue_observation
                .as_deref()
                .unwrap_or("")
                .contains("position"),
            "observation must name the residual: {:?}",
            row.venue_observation
        );

        // Position cleared → settled (venue-confirmed, not wall-clock).
        stub.positions.lock().unwrap().clear();
        check_fixed_yield_maturities(&state).await;
        let row = &state.state_store.list_fixed_yield_maturities().unwrap()[0];
        assert_eq!(row.maturity_state, "settled");
    }

    /// Resolution is gated on venue-confirmed settlement: a `due` row
    /// refuses, `keep_carry` resolves without touching perps, and a
    /// second resolution refuses.
    #[tokio::test]
    async fn maturity_resolution_gated_on_venue_truth() {
        let mut scripts = VecDeque::new();
        scripts.push_back(Some((2, VecDeque::new())));
        scripts.push_back(Some((2, VecDeque::new())));
        let stub = spawn_boros_stub(scripts).await;
        let (state, _l, _s) = fixed_yield_state("resolve", &stub).await;
        run_fixed_yield(&state).await.expect("fixed-yield executes");
        let row = state.state_store.list_fixed_yield_maturities().unwrap()[0].clone();
        let yu_id = row.signal_id.clone();
        let parent = row.parent_signal_id.clone().unwrap();

        // Not settled → refused.
        let err = crate::commands::resolve_fixed_yield_maturity_impl(&yu_id, "keep_carry", &state)
            .await
            .expect_err("unresolved maturity must refuse");
        assert!(err.contains("not venue-confirmed"), "unexpected: {err}");

        // Age past maturity; sweep confirms settlement.
        let past = Utc::now().timestamp_millis() - 60_000;
        state
            .state_store
            .record_fixed_yield_maturity(&yu_id, &parent, past, &[11, 22])
            .unwrap();
        check_fixed_yield_maturities(&state).await;

        // keep_carry resolves; the parent signal stays open.
        let res = crate::commands::resolve_fixed_yield_maturity_impl(&yu_id, "keep_carry", &state)
            .await
            .expect("keep_carry resolves");
        assert_eq!(res.legs_closed, 0);
        let parent_sig = state.state_store.get_signal(&parent).unwrap().unwrap();
        assert!(
            parent_sig.status.counts_as_open(),
            "parent keeps running as floating carry"
        );
        let row = &state.state_store.list_fixed_yield_maturities().unwrap()[0];
        assert_eq!(row.resolved_action.as_deref(), Some("keep_carry"));

        // Second resolution refuses — the choice is durable.
        assert!(
            crate::commands::resolve_fixed_yield_maturity_impl(&yu_id, "close_pair", &state)
                .await
                .is_err()
        );
    }

    /// The priced exit: quote reflects the live book, execution fills
    /// both YU exits, flattens the parent pair, and closes the tracker.
    #[tokio::test]
    async fn fixed_yield_priced_exit_round_trip() {
        // Entry fills + two exit fills.
        let mut scripts = VecDeque::new();
        for _ in 0..4 {
            scripts.push_back(Some((2, VecDeque::new())));
        }
        let stub = spawn_boros_stub(scripts).await;
        let (state, _l, _s) = fixed_yield_state("exit", &stub).await;
        run_fixed_yield(&state).await.expect("fixed-yield executes");
        let yu_id = state.state_store.list_fixed_yield_maturities().unwrap()[0]
            .signal_id
            .clone();

        let quote = fixed_yield_exit_quote_impl(&yu_id, &state)
            .await
            .expect("quote");
        assert_eq!(quote.legs.len(), 2, "notes: {:?}", quote.notes);
        for leg in &quote.legs {
            assert!(leg.open_qty > 0.0, "leg must carry open size");
            assert!(
                leg.exit_rate.is_some(),
                "stub book quotes both sides (leg {} notes {:?})",
                leg.yu_symbol,
                quote.notes
            );
            assert!(leg.est_exit_pnl_usd.is_some());
        }
        assert!(quote.est_total_pnl_usd.is_some());

        let flat = execute_fixed_yield_exit_impl(&yu_id, &state)
            .await
            .expect("exit executes");
        assert_eq!(flat.residual_signals, 0, "parent pair fully closed");
        let row = &state.state_store.list_fixed_yield_maturities().unwrap()[0];
        assert_eq!(row.maturity_state, "closed_early");
        assert_eq!(row.resolved_action.as_deref(), Some("close_pair"));
        // The exit is durable: the sibling closed, a flat- child carries
        // the exit fills, and a second exit refuses.
        let yu_sig = state.state_store.get_signal(&yu_id).unwrap().unwrap();
        assert_eq!(yu_sig.status, crate::state_store::SignalStatus::Closed);
        assert!(
            fixed_yield_exit_quote_impl(&yu_id, &state).await.is_err(),
            "closed lock has nothing to quote"
        );
    }

    /// Auto-gate refuses on each named bound and admits when all pass.
    #[tokio::test]
    async fn fixed_yield_auto_gate_named_refusals() {
        let stub = spawn_boros_stub(VecDeque::new()).await;
        let (state, _l, _s) = fixed_yield_state("gate", &stub).await;
        // A real scanned row carries the honest lock/return/legs — mutate
        // fields per case rather than fabricating a DTO.
        let opp = scan_funding_opportunities_impl("BTC-USDC", 50_000.0, &state)
            .await
            .expect("scan")
            .into_iter()
            .find(|o| o.kind == "fixed_yield")
            .expect("fixed_yield row");
        let legs = opp.yu_legs.clone().expect("yu legs");
        let now_ms = Utc::now().timestamp_millis();

        // Disabled → refuse by name.
        let err = fixed_yield_auto_gate(&state, &opp, &legs, 50_000.0)
            .await
            .expect_err("disabled refuses");
        assert!(err.contains("disabled"), "{err}");

        state.settings.write().auto_fixed_yield_enabled = true;
        // Non-positive post-cost return → refuse.
        let mut o = opp.clone();
        o.expected_return_usd = Some(0.0);
        let err = fixed_yield_auto_gate(&state, &o, &legs, 50_000.0)
            .await
            .expect_err("zero return refuses");
        assert!(err.contains("not positive"), "{err}");
        // Short lock → refuse.
        let mut o = opp.clone();
        o.lock_until_ms = Some(now_ms + 3 * 86_400_000);
        let err = fixed_yield_auto_gate(&state, &o, &legs, 50_000.0)
            .await
            .expect_err("short lock refuses");
        assert!(err.contains("lock term"), "{err}");
        // Gas below minimum → refuse.
        *stub.gas_usd.lock().unwrap() = Some(serde_json::json!(1.0));
        let err = fixed_yield_auto_gate(&state, &opp, &legs, 50_000.0)
            .await
            .expect_err("low gas refuses");
        assert!(err.contains("gas"), "{err}");
        // Gas missing entirely → refuse (never assume).
        *stub.gas_usd.lock().unwrap() = None;
        let err = fixed_yield_auto_gate(&state, &opp, &legs, 50_000.0)
            .await
            .expect_err("unknown gas refuses");
        assert!(err.contains("unknown"), "{err}");
        // All gates pass → admitted.
        *stub.gas_usd.lock().unwrap() = Some(serde_json::json!(500.0));
        fixed_yield_auto_gate(&state, &opp, &legs, 50_000.0)
            .await
            .expect("all gates pass");
    }
}
