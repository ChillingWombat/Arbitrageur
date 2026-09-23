//! Funding-carry strategy: cross-venue funding-rate scan (perp/perp carry
//! and spot/perp basis), 8h-normalized leg rates, gap history + persistence
//! probability, trend substrate, and the two-leg IOC carry executor.
//! Split out of `commands.rs`; shared execution helpers are reached via
//! `crate::commands`.

use crate::commands::{
    backtest_stats, dec_to_f64, effective_venue_fee_bps, get_market_universe_impl, live_entry_gate,
    live_entry_notional_guard, margin_monitor_from_settings, maybe_record_margin_snapshots,
    reconcile_signal_legs, record_execution_failure, record_execution_log,
    refresh_equity_expectations_after_fills, resolve_effective_leverage, strategy_slippage_bps,
    validate_adapter_book, validate_market_symbol, SimStepResultDto, FUNDING_STRATEGY_ID,
};
use crate::state::{AppEngineState, ExecutionLogEntry};
use crate::triangle::triangle_dust_qty;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::quote::OrderBook;
use arbitrageur_core::types::{dec, Decimal, Side, VenueId};
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
        let _ = state
            .state_store
            .record_funding_rate_samples(now_ms, &samples);
    }

    // Cross-exchange carry needs >= 2 funding prints; same-exchange basis
    // needs only one venue with funding + both books, so no early return here.
    let mut opps = Vec::new();

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

    // Ticker mid history: median mid per minute bucket across venues —
    // venue-neutral AND robust to a residual outlier surviving the store's
    // consistency filter (a mean lets one bad mid move the whole line).
    let mut buckets: HashMap<i64, Vec<f64>> = HashMap::new();
    if let Ok(samples) = state.state_store.spread_samples(symbol, from_ms, now_ms) {
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
    let is_live = state.is_live();
    if is_live {
        // Funding entries reuse the same two-leg coordinator + durable
        // signal reconciliation as the spatial path, so they run under the
        // same operator opt-in: live_entry_enabled + unlocked vault +
        // adapter submission/polling/entry eligibility + live credentials.
        // The per-leg notional cap is checked below once leg VWAPs price.
        live_entry_gate(state, &[long_venue, short_venue])?;
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

    // Equal base quantity from the requested notional at the long ask.
    let qty_f64 = trade_size_usd / long_ask_f;
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
    if is_live {
        live_entry_notional_guard(
            state,
            &[
                (long_canon, qty * long_vwap),
                (short_canon, qty * short_vwap),
            ],
        )?;
    }
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
        .blocking_signals(&root_symbol)
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
    let kind_label = if is_basis { "basis" } else { "carry" };
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
        .record_signal_execution(&NewSignalExecution {
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
                },
                NewSignalLeg {
                    account_id: account_for(short_canon),
                    venue: short_canon.to_string(),
                    side: LegSide::Short,
                    symbol: Some(short_sym.clone()),
                    intended_qty: qty_f64,
                    intended_price: Some(short_bid_f),
                },
            ],
        })
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
        if let Ok(f) = acc.equity.to_string().parse::<f64>() {
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
    let total_f = {
        let mut total_pnl = if state.is_live() {
            state.total_realized_profit_live.write()
        } else {
            state.total_realized_profit.write()
        };
        *total_pnl += Decimal::from_str_exact(&attempt_pnl_f.to_string()).unwrap_or(dec!(0));
        total_pnl.to_string().parse::<f64>().unwrap_or(0.0)
    };

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
                    "{trim_note}Funding {kind_label} opened {open_qty} {root_symbol}: {path} at {long_px_f}/{short_px_f}; {carry_note}"
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
                    "{trim_note}Funding {kind_label} dispatched on {root_symbol} ({path}): 2/2 legs filled {open_qty} (LONG {long_sym} / SHORT {short_sym}), realized P&L ${attempt_pnl_f:.2}; carry position open — flatten {root_symbol} to unwind"
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
                    let fee = f.fee.to_string().parse::<f64>().unwrap_or(0.0);
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
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SimStepResultDto, String> {
    match execute_funding_carry_impl(&symbol, &long_venue, &short_venue, trade_size_usd, &state)
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
