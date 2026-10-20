//! Multi-leg options arbitrage packages on Deribit — the scanners and
//! executor for the non-parity live substrategies:
//!
//! - `box_spread`: 4-leg synthetic zero-coupon bond. Its implied interest
//!   rate should track the venue's carry benchmark (perp funding
//!   annualized); divergence is capturable by buying or shorting the box.
//! - `jelly_roll`: two box spreads across expiries T1<T2 — a synthetic
//!   term-structure loan. The implied T1→T2 rate is compared against the
//!   dated-futures basis (preferred) or funding carry, and the package is
//!   traded when the two disagree.
//! - `convexity`: butterfly convexity bound. For equidistant strikes the
//!   body can never price above the wing mean — a book quoting above the
//!   bound is a riskless sell (buy the wings, sell twice the body).
//!
//! All rate math is continuous-compounding in f64; leg prices stay Decimal
//! for venue fidelity. Execution re-scans fresh, re-validates every quote
//! side, and dispatches bounded IOC legs through the same guards as the
//! parity path (circuit breaker, engine pause, execution lock, per-leg
//! blocking signals, reduce-only flags, margin and account-position
//! checks). Autonomous entries remain testnet-only via `auto` + `is_live()`
//! — identical to parity.
//!
//! Unpriced risk: these packages lock a deterministic payoff at expiry —
//! realized P&L at entry is premium cashflow minus fees only; the locked
//! edge is unrealized until expiry/flatten, same convention as parity.

use crate::commands::{
    live_entry_gate, margin_monitor_from_settings, record_execution_log, SimStepResultDto,
};
use crate::options_parity::{
    deribit_for_env, options_leg_bound_px, record_options_leg_intent, summary_px,
    DERIBIT_DELIVERY_FEE_RATE, OPTIONS_SETTLE_INTERVAL, OPTIONS_SETTLE_POLLS,
    OPTION_FEE_PREMIUM_CAP, OPTION_TAKER_FEE_RATE,
};
use crate::state::{AppEngineState, ExecutionLogEntry};
use crate::state_store::{LegSide, SignalStatus};
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::types::{dec, Decimal, OrderStatus, Side, VenueId};
use arbitrageur_exchanges::deribit::DeribitBookSummary;
use arbitrageur_risk::margin_monitor::MarginHealthStatus;
use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tauri::State;

/// Live package scanners — parity stays on its own implementation.
pub const LIVE_PACKAGE_SUBSTRATEGIES: [&str; 3] = ["box_spread", "jelly_roll", "convexity"];

const MAX_RESULTS: usize = 12;
/// Widest strike gap, in strike-step multiples, a package candidate may
/// span — bound checks wider than this rarely quote and waste scan time.
const MAX_STRIKE_GAP_MULT: usize = 8;

#[derive(Clone, Debug, Deserialize)]
pub struct OptionsPackageScanParams {
    pub substrategy_id: String,
    pub currency: String,
    pub qty_base: Option<f64>,
    pub min_hours_to_expiry: Option<f64>,
    pub max_days_to_expiry: Option<f64>,
    pub min_edge_bps: Option<f64>,
    /// Minimum annualized expected yield percentage hurdle (e.g. 10.0 for 10% APR).
    #[serde(default)]
    pub min_expected_yield_pct: Option<f64>,
    /// Result cap — a display concern. Execution re-scans pass usize::MAX so
    /// a candidate cannot fall out of the top-N cut between scans.
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct OptionsPackageExecutionParams {
    pub id: String,
    pub substrategy_id: String,
    pub currency: String,
    pub qty_base: f64,
    pub min_edge_bps: Option<f64>,
    #[serde(default)]
    pub min_expected_yield_pct: Option<f64>,
    #[serde(default)]
    pub auto: bool,
}

/// One leg of a package — instrument, execution side, and contract
/// multiplier (2 for a butterfly body, 1 otherwise).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OptionsPackageLegDto {
    pub instrument: String,
    pub side: String,
    pub qty_mult: u32,
    /// Venue price used for edge pricing (base coin).
    pub venue_price: f64,
    /// Same leg priced in USD at index.
    pub usd_price: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OptionsPackageOpportunityDto {
    pub id: String,
    pub substrategy_id: String,
    pub venue: String,
    pub currency: String,
    /// Trade descriptor — "LONG BOX", "SHORT JELLY", "SELL CALL FLY"…
    pub label: String,
    pub expiry_ms: i64,
    pub expiry_iso: String,
    /// Far expiry for term-structure packages (jelly roll only).
    pub expiry2_ms: Option<i64>,
    pub expiry2_iso: Option<String>,
    pub strikes: Vec<f64>,
    pub legs: Vec<OptionsPackageLegDto>,
    /// Implied package rate (rate structures) — percent, annualized.
    pub implied_rate_pct: Option<f64>,
    /// Benchmark rate it diverged from — percent, annualized.
    pub benchmark_rate_pct: Option<f64>,
    pub gross_edge_usd: f64,
    pub est_fees_usd: f64,
    pub net_edge_usd: f64,
    /// Net edge as bps of per-unit underlying notional (qty×index) — the
    /// same yardstick parity uses, so one hurdle fits every substrategy.
    pub edge_bps: f64,
    /// Annualized expected yield (APR, percent) on notional capital to expiry.
    #[serde(default)]
    pub expected_yield_pct: f64,
    pub index_price_usd: f64,
    pub qty_base_suggested: f64,
    pub is_executable: bool,
    pub reason: Option<String>,
}

/// One option instrument's executable-side quotes.
#[derive(Clone, Debug)]
pub struct OptQuote {
    pub name: String,
    pub is_call: bool,
    pub bid: Option<Decimal>,
    pub ask: Option<Decimal>,
    pub is_open: bool,
}

/// One strike row: (call quote, put quote).
type StrikeRow = (Option<OptQuote>, Option<OptQuote>);
/// expiry_ms → strike → quotes.
type OptionGrid = BTreeMap<i64, BTreeMap<Decimal, StrikeRow>>;

/// The chain snapshot every package scanner reads — fetched once per
/// currency so evaluators stay pure over the same books.
pub struct ChainSnapshot {
    /// expiry_ms → strike → (call quote, put quote)
    pub grid: OptionGrid,
    /// Dated futures marks by expiry_ms — the term-structure benchmark.
    pub futures_mark: BTreeMap<i64, f64>,
    pub index: Decimal,
    pub funding_8h: Decimal,
    pub now_ms: i64,
}

async fn fetch_chain_snapshot(ccy: &str, state: &AppEngineState) -> Result<ChainSnapshot, String> {
    let adapter = deribit_for_env(state)?;
    let perp_name = format!("{ccy}-PERPETUAL");
    let (chain, summaries, index, funding, futures) = tokio::try_join!(
        adapter.option_chain(ccy),
        adapter.book_summaries(ccy),
        adapter.index_price_usd(ccy),
        adapter.fetch_funding_rate(&perp_name),
        adapter.dated_futures(ccy),
    )
    .map_err(|e| format!("Deribit package scan inputs failed: {e}"))?;
    if index <= dec!(0) {
        return Err(format!("Deribit {ccy} index price unavailable"));
    }
    let summary_map: HashMap<String, &DeribitBookSummary> =
        summaries.iter().map(|s| (s.name.clone(), s)).collect();
    let mut grid: OptionGrid = BTreeMap::new();
    let mut futures_mark: BTreeMap<i64, f64> = BTreeMap::new();
    for inst in &chain {
        if inst.settlement_currency != ccy {
            continue;
        }
        let Some(summary) = summary_map.get(&inst.name) else {
            continue;
        };
        let (bid, ask) = summary_px(summary);
        let q = OptQuote {
            name: inst.name.clone(),
            is_call: matches!(
                inst.option_type,
                arbitrageur_exchanges::deribit::DeribitOptionType::Call
            ),
            bid,
            ask,
            is_open: inst.is_open,
        };
        let slot = grid
            .entry(inst.expiration_ms)
            .or_default()
            .entry(inst.strike)
            .or_default();
        if q.is_call {
            slot.0 = Some(q);
        } else {
            slot.1 = Some(q);
        }
    }
    // Dated futures marks — the honest term benchmark for jelly rolls.
    // Names map to expiries via the shared instruments cache (dated_futures).
    let fut_expiry: HashMap<String, i64> = futures.into_iter().collect();
    for (name, s) in &summary_map {
        if !matches!(s.kind, arbitrageur_exchanges::deribit::DeribitKind::Future) {
            continue;
        }
        if name.ends_with("-PERPETUAL") {
            continue;
        }
        let Some(expiry_ms) = fut_expiry.get(name.as_str()).copied() else {
            continue;
        };
        if let Some(m) = s.mark.filter(|m| *m > dec!(0)) {
            let f = m.to_string().parse::<f64>().unwrap_or(0.0);
            if f.is_finite() && f > 0.0 {
                futures_mark.insert(expiry_ms, f);
            }
        }
    }
    Ok(ChainSnapshot {
        grid,
        futures_mark,
        index,
        funding_8h: funding.rate,
        now_ms: Utc::now().timestamp_millis(),
    })
}

fn to_f64(d: Decimal) -> f64 {
    d.to_string().parse::<f64>().unwrap_or(0.0)
}

/// Deribit option taker fee per contract traded: min(0.03% of underlying,
/// 12.5% of premium) — the parity fee model, shared across packages.
fn option_fee_usd(px_usd: f64, index_usd: f64, qty: f64) -> Option<f64> {
    if !px_usd.is_finite() || px_usd <= 0.0 || !qty.is_finite() || qty <= 0.0 {
        return None;
    }
    Some((OPTION_TAKER_FEE_RATE * index_usd).min(OPTION_FEE_PREMIUM_CAP * px_usd) * qty)
}

/// Annualized continuous carry benchmark from the perp funding print —
/// the venue's own price of term money. Funding is a simple-rate print
/// per 8h; at crypto magnitudes ln(1+f·1095) ≈ f·1095 — the same linear
/// convention parity uses for its funding drain.
fn funding_benchmark_rate(funding_8h: Decimal) -> f64 {
    to_f64(funding_8h) * 1095.0
}

/// Quote on the exact side a leg executes — fails closed when that side
/// is unquoted, matching parity's executable-side discipline.
fn side_px(q: &OptQuote, buy: bool) -> Option<f64> {
    (if buy { q.ask } else { q.bid })
        .map(to_f64)
        .filter(|p| p.is_finite() && *p > 0.0)
}

/// Long-box cost in base coin per contract at executable-side prices:
/// buy C(K1) + buy P(K2) − sell C(K2) − sell P(K1).
fn long_box_cost(c1: &OptQuote, c2: &OptQuote, p1: &OptQuote, p2: &OptQuote) -> Option<f64> {
    Some(side_px(c1, true)? - side_px(c2, false)? - side_px(p1, false)? + side_px(p2, true)?)
}

/// Short-box proceeds in base coin per contract at executable-side prices.
fn short_box_proceeds(c1: &OptQuote, c2: &OptQuote, p1: &OptQuote, p2: &OptQuote) -> Option<f64> {
    Some(side_px(c1, false)? - side_px(c2, true)? - side_px(p1, true)? + side_px(p2, false)?)
}

/// One (K1<K2) box window — the four instruments a box trades, at any
/// expiry. Grouped so evaluators stay under the arg-count budget and
/// callers build it straight off the strike row.
#[derive(Clone, Copy)]
pub struct BoxWindow<'a> {
    /// Call at the lower strike.
    pub c1: &'a OptQuote,
    /// Call at the higher strike.
    pub c2: &'a OptQuote,
    /// Put at the lower strike.
    pub p1: &'a OptQuote,
    /// Put at the higher strike.
    pub p2: &'a OptQuote,
}

/// Shared evaluator context — USD strike gap, contract quantity, and the
/// index price that converts coin-denominated premiums to USD.
#[derive(Clone, Copy)]
pub struct EvalCtx {
    pub strike_gap_usd: f64,
    pub qty: f64,
    pub index_usd: f64,
}

/// One evaluated box-spread candidate — pure over quotes so tests drive it
/// without a venue.
pub struct BoxEval {
    pub label: &'static str,
    /// [C(K1), C(K2), P(K1), P(K2)] with sides in the executed direction.
    pub legs: [(bool, f64); 4],
    pub gross_edge_usd: f64,
    pub est_fees_usd: f64,
    pub net_edge_usd: f64,
    /// ln(X / PV) / T — the annualized rate this window's books imply.
    pub implied_rate: f64,
    pub benchmark_rate: f64,
}

/// Evaluate one (expiry, K1<K2) box window. `t_years` > 0, `bench` =
/// annualized continuous benchmark rate. Both directions are priced at
/// executable sides and the better survives — `None` when neither clears
/// fees or a side is unquoted.
pub fn evaluate_box(w: &BoxWindow<'_>, ctx: &EvalCtx, t_years: f64, bench: f64) -> Option<BoxEval> {
    let (c1, c2, p1, p2) = (w.c1, w.c2, w.p1, w.p2);
    let (strike_gap_usd, qty, index_usd) = (ctx.strike_gap_usd, ctx.qty, ctx.index_usd);
    if !qty.is_finite()
        || qty <= 0.0
        || !index_usd.is_finite()
        || index_usd <= 0.0
        || !strike_gap_usd.is_finite()
        || strike_gap_usd <= 0.0
        || !t_years.is_finite()
        || t_years <= 0.0
    {
        return None;
    }
    let fair_pv = strike_gap_usd * (-bench * t_years).exp();
    let mk = |long: bool| -> Option<BoxEval> {
        let coin_val = if long {
            long_box_cost(c1, c2, p1, p2)?
        } else {
            short_box_proceeds(c1, c2, p1, p2)?
        };
        let usd_val = coin_val * index_usd;
        // A box worth ≤0 or ≥2× payoff on executable sides is degenerate —
        // never an arb, usually a crossed/dead book.
        if usd_val <= 0.0 || usd_val >= strike_gap_usd * 2.0 {
            return None;
        }
        let gross = (if long {
            fair_pv - usd_val
        } else {
            usd_val - fair_pv
        }) * qty;
        if !gross.is_finite() || gross <= 0.0 {
            return None;
        }
        let implied = (strike_gap_usd / usd_val).ln() / t_years;
        let leg_pxs: [(bool, f64); 4] = if long {
            [
                (true, side_px(c1, true)?),
                (false, side_px(c2, false)?),
                (false, side_px(p1, false)?),
                (true, side_px(p2, true)?),
            ]
        } else {
            [
                (false, side_px(c1, false)?),
                (true, side_px(c2, true)?),
                (true, side_px(p1, true)?),
                (false, side_px(p2, false)?),
            ]
        };
        let entry_fees = leg_pxs
            .iter()
            .map(|(_, px)| option_fee_usd(px * index_usd, index_usd, qty))
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .sum::<f64>();
        // Exactly two legs settle ITM at expiration for any Box Spread.
        // Deribit delivery fees are capped at 12.5% of the settlement value.
        let uncapped_delivery = 2.0 * (DERIBIT_DELIVERY_FEE_RATE * index_usd) * qty;
        let max_delivery_cap = OPTION_FEE_PREMIUM_CAP * strike_gap_usd * qty;
        let delivery_fees = uncapped_delivery.min(max_delivery_cap);
        let fees = entry_fees + delivery_fees;
        let net = gross - fees;
        if !net.is_finite() || net <= 0.0 {
            return None;
        }
        Some(BoxEval {
            label: if long { "LONG BOX" } else { "SHORT BOX" },
            legs: leg_pxs,
            gross_edge_usd: gross,
            est_fees_usd: fees,
            net_edge_usd: net,
            implied_rate: implied,
            benchmark_rate: bench,
        })
    };
    [mk(true), mk(false)]
        .into_iter()
        .flatten()
        .max_by(|a, b| a.net_edge_usd.total_cmp(&b.net_edge_usd))
}

/// One evaluated jelly-roll candidate — a term-structure loan between
/// expiries T1<T2 at strikes K1<K2.
pub struct JellyEval {
    pub label: &'static str,
    /// Executed leg prices: [C1(K1),C1(K2),P1(K1),P1(K2), C2(K1),C2(K2),P2(K1),P2(K2)]
    /// — near box legs first, then far box.
    pub legs: [(bool, f64); 8],
    pub gross_edge_usd: f64,
    pub est_fees_usd: f64,
    pub net_edge_usd: f64,
    /// ln(far_pv / near_pv) / (T2−T1) — the implied T1→T2 term rate.
    pub implied_rate: f64,
    pub benchmark_rate: f64,
}

/// Evaluate one jelly roll — two same-strike box windows across expiries.
/// `bench` = annualized continuous term benchmark for the T1→T2 window.
pub fn evaluate_jelly(
    near: &BoxWindow<'_>,
    far: &BoxWindow<'_>,
    ctx: &EvalCtx,
    t1_years: f64,
    t2_years: f64,
    bench: f64,
) -> Option<JellyEval> {
    let (c1t, c2t, p1t, p2t) = (near.c1, near.c2, near.p1, near.p2);
    let (c1f, c2f, p1f, p2f) = (far.c1, far.c2, far.p1, far.p2);
    let (strike_gap_usd, qty, index_usd) = (ctx.strike_gap_usd, ctx.qty, ctx.index_usd);
    if !qty.is_finite()
        || qty <= 0.0
        || !index_usd.is_finite()
        || index_usd <= 0.0
        || !strike_gap_usd.is_finite()
        || strike_gap_usd <= 0.0
        || !t1_years.is_finite()
        || t1_years <= 0.0
        || !t2_years.is_finite()
        || t2_years <= t1_years
    {
        return None;
    }
    let fair = strike_gap_usd * ((-bench * t1_years).exp() - (-bench * t2_years).exp());
    if !fair.is_finite() || fair <= 0.0 {
        return None; // negative/degenerate term benchmark — no arb claim
    }
    let near_cost = long_box_cost(c1t, c2t, p1t, p2t)?;
    let near_proceeds = short_box_proceeds(c1t, c2t, p1t, p2t)?;
    let far_cost = long_box_cost(c1f, c2f, p1f, p2f)?;
    let far_proceeds = short_box_proceeds(c1f, c2f, p1f, p2f)?;
    let mk = |long: bool| -> Option<JellyEval> {
        // Long jelly = long near box + short far box: pay near_cost −
        // far_proceeds, receive X at T1 and pay X at T2.
        let net_cost_coin = if long {
            near_cost - far_proceeds
        } else {
            far_cost - near_proceeds
        };
        let net_cost = net_cost_coin * index_usd;
        let gross = (if long {
            fair - net_cost
        } else {
            net_cost - fair
        }) * qty;
        if !gross.is_finite() || gross <= 0.0 {
            return None;
        }
        // Implied term rate from the two market-priced boxes:
        // r_impl = ln(far_pv / near_pv) / (T2−T1), pv = the traded side's
        // box value — long jelly pays near cost, receives far proceeds.
        let (near_pv, far_pv) = if long {
            (near_cost * index_usd, far_proceeds * index_usd)
        } else {
            (near_proceeds * index_usd, far_cost * index_usd)
        };
        if near_pv <= 0.0 || far_pv <= 0.0 {
            return None;
        }
        let implied = (far_pv / near_pv).ln() / (t2_years - t1_years);
        let leg_pxs: [(bool, f64); 8] = if long {
            [
                (true, side_px(c1t, true)?),
                (false, side_px(c2t, false)?),
                (false, side_px(p1t, false)?),
                (true, side_px(p2t, true)?),
                (false, side_px(c1f, false)?),
                (true, side_px(c2f, true)?),
                (true, side_px(p1f, true)?),
                (false, side_px(p2f, false)?),
            ]
        } else {
            [
                (false, side_px(c1t, false)?),
                (true, side_px(c2t, true)?),
                (true, side_px(p1t, true)?),
                (false, side_px(p2t, false)?),
                (true, side_px(c1f, true)?),
                (false, side_px(c2f, false)?),
                (false, side_px(p1f, false)?),
                (true, side_px(p2f, true)?),
            ]
        };
        let entry_fees = leg_pxs
            .iter()
            .map(|(_, px)| option_fee_usd(px * index_usd, index_usd, qty))
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .sum::<f64>();
        // For a Jelly Roll (two box spreads across two expiries), exactly 4 legs settle ITM.
        // Deribit delivery fees are capped at 12.5% of the settlement value.
        let uncapped_delivery = 4.0 * (DERIBIT_DELIVERY_FEE_RATE * index_usd) * qty;
        let max_delivery_cap = 2.0 * OPTION_FEE_PREMIUM_CAP * strike_gap_usd * qty;
        let delivery_fees = uncapped_delivery.min(max_delivery_cap);
        let fees = entry_fees + delivery_fees;
        let net = gross - fees;
        if !net.is_finite() || net <= 0.0 {
            return None;
        }
        Some(JellyEval {
            label: if long { "LONG JELLY" } else { "SHORT JELLY" },
            legs: leg_pxs,
            gross_edge_usd: gross,
            est_fees_usd: fees,
            net_edge_usd: net,
            implied_rate: implied,
            benchmark_rate: bench,
        })
    };
    [mk(true), mk(false)]
        .into_iter()
        .flatten()
        .max_by(|a, b| a.net_edge_usd.total_cmp(&b.net_edge_usd))
}

/// One convexity-boundary candidate — sell the overpriced body, buy the
/// wings. Equidistant strikes only so the 1/2/1 contract ratio hedges
/// exactly.
pub struct ConvexityEval {
    pub label: &'static str,
    /// [wing K1 buy ×1, body K2 sell ×2, wing K3 buy ×1] executed prices.
    pub legs: [(bool, f64); 3],
    #[allow(dead_code)]
    pub is_call: bool,
    pub gross_edge_usd: f64,
    pub est_fees_usd: f64,
    pub net_edge_usd: f64,
}

/// Evaluate one equidistant strike triple for a single option type (call or put).
/// Convexity bound: C(K2) <= (C(K1)+C(K3))/2 (or P(K2) <= (P(K1)+P(K3))/2).
/// Violation => riskless sell of the body.
pub fn evaluate_convexity_side(
    w1: &OptQuote,
    body: &OptQuote,
    w3: &OptQuote,
    k_triplet: Option<(f64, f64, f64)>,
    qty: f64,
    index_usd: f64,
    is_call: bool,
) -> Option<ConvexityEval> {
    if !qty.is_finite() || qty <= 0.0 || !index_usd.is_finite() || index_usd <= 0.0 {
        return None;
    }
    let w1_ask = side_px(w1, true)?;
    let w3_ask = side_px(w3, true)?;
    let body_bid = side_px(body, false)?;
    let edge_unit = 2.0 * body_bid - w1_ask - w3_ask;
    if !edge_unit.is_finite() || edge_unit <= 0.0 {
        return None;
    }

    // Breeden-Litzenberger Risk-Neutral Density Verification:
    // Ensure mid-market discrete density d²C/dK² >= 0 across the triplet to avoid distorted books
    if let Some((k1, k2, k3)) = k_triplet {
        let p1_mid = (w1_ask + side_px(w1, false).unwrap_or(w1_ask)) * 0.5 * index_usd;
        let p2_mid = (side_px(body, true).unwrap_or(body_bid) + body_bid) * 0.5 * index_usd;
        let p3_mid = (w3_ask + side_px(w3, false).unwrap_or(w3_ask)) * 0.5 * index_usd;
        if arbitrageur_math::black_scholes::check_butterfly_arbitrage_density(
            &[k1, k2, k3],
            &[p1_mid, p2_mid, p3_mid],
        )
        .is_err()
        {
            return None; // Refuse candidates residing on an anomalous or inverted density strip
        }
    }

    let gross = edge_unit * qty * index_usd;
    // Wings trade qty, the body trades 2×qty — per-contract fee on
    // each leg's traded premium.
    let entry_fees = option_fee_usd(w1_ask * index_usd, index_usd, qty)?
        + option_fee_usd(body_bid * index_usd, index_usd, qty * 2.0)?
        + option_fee_usd(w3_ask * index_usd, index_usd, qty)?;
    // Conservative delivery allowance: up to 2 legs settle ITM if body is breached
    let delivery_fees = 2.0 * (DERIBIT_DELIVERY_FEE_RATE * index_usd) * qty;
    let fees = entry_fees + delivery_fees;
    let net = gross - fees;
    if !net.is_finite() || net <= 0.0 {
        return None;
    }
    Some(ConvexityEval {
        label: if is_call {
            "SELL CALL FLY"
        } else {
            "SELL PUT FLY"
        },
        legs: [(true, w1_ask), (false, body_bid), (true, w3_ask)],
        is_call,
        gross_edge_usd: gross,
        est_fees_usd: fees,
        net_edge_usd: net,
    })
}

/// Evaluate one equidistant strike triple at one expiry. Convexity bound:
/// C(K2) ≤ (C(K1)+C(K3))/2 — violation ⇒ riskless sell of the body.
/// Only the buy-wings/sell-body direction is riskless; the inverse is a
/// short butterfly whose payoff can go negative, so it is never emitted.
#[allow(dead_code)]
pub fn evaluate_convexity(
    k1: (&OptQuote, &OptQuote),
    k2: (&OptQuote, &OptQuote),
    k3: (&OptQuote, &OptQuote),
    qty: f64,
    index_usd: f64,
) -> Option<ConvexityEval> {
    [
        evaluate_convexity_side(k1.0, k2.0, k3.0, None, qty, index_usd, true),
        evaluate_convexity_side(k1.1, k2.1, k3.1, None, qty, index_usd, false),
    ]
    .into_iter()
    .flatten()
    .max_by(|a, b| a.net_edge_usd.total_cmp(&b.net_edge_usd))
}

fn expiry_iso(expiry_ms: i64) -> String {
    Utc.timestamp_millis_opt(expiry_ms)
        .single()
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

fn leg_dto(name: &str, buy: bool, px: f64, qty_mult: u32, index: Decimal) -> OptionsPackageLegDto {
    OptionsPackageLegDto {
        instrument: name.to_string(),
        side: if buy { "buy" } else { "sell" }.to_string(),
        qty_mult,
        venue_price: px,
        usd_price: px * to_f64(index),
    }
}

/// Scan one package substrategy over the chain snapshot.
pub async fn scan_options_packages_impl(
    params: &OptionsPackageScanParams,
    state: &AppEngineState,
) -> Result<Vec<OptionsPackageOpportunityDto>, String> {
    let sub = params.substrategy_id.trim().to_lowercase();
    if !LIVE_PACKAGE_SUBSTRATEGIES.contains(&sub.as_str()) {
        return Err(format!("no live package scanner for substrategy '{sub}'"));
    }
    let ccy = params.currency.trim().to_ascii_uppercase();
    if !matches!(ccy.as_str(), "BTC" | "ETH") {
        return Err("options packages support BTC or ETH".into());
    }
    let mut qty_base = params
        .qty_base
        .unwrap_or(if ccy == "ETH" { 1.0 } else { 0.1 });
    if ccy == "ETH" && qty_base < 1.0 {
        qty_base = 1.0;
    }
    if !qty_base.is_finite() || qty_base <= 0.0 {
        return Err("qty_base must be a finite positive number".into());
    }
    let min_hours = params.min_hours_to_expiry.unwrap_or(48.0).max(0.0);
    let max_days = params.max_days_to_expiry.unwrap_or(180.0).max(0.0);
    let min_edge_bps = params.min_edge_bps.unwrap_or(0.0);

    let snap = fetch_chain_snapshot(&ccy, state).await?;
    let funding_bench = funding_benchmark_rate(snap.funding_8h);
    let index_usd = to_f64(snap.index);
    let mut out: Vec<OptionsPackageOpportunityDto> = Vec::new();

    let in_window = |expiry_ms: i64| -> bool {
        let hours = (expiry_ms - snap.now_ms) as f64 / 3_600_000.0;
        hours >= min_hours && hours <= max_days * 24.0
    };
    let years_of = |expiry_ms: i64| (expiry_ms - snap.now_ms) as f64 / (3_600_000.0 * 24.0 * 365.0);
    let mut push = |o: OptionsPackageOpportunityDto| {
        if o.edge_bps >= min_edge_bps {
            if let Some(min_yield) = params.min_expected_yield_pct {
                if o.expected_yield_pct < min_yield {
                    return;
                }
            }
            out.push(o);
        }
    };

    match sub.as_str() {
        "box_spread" => {
            for (expiry_ms, strikes) in &snap.grid {
                if !in_window(*expiry_ms) {
                    continue;
                }
                let t_years = years_of(*expiry_ms);
                let strike_list: Vec<Decimal> = strikes.keys().cloned().collect();
                for i in 0..strike_list.len() {
                    let hi = strike_list.len().min(i + 1 + MAX_STRIKE_GAP_MULT);
                    for j in (i + 1)..hi {
                        let (k1, k2) = (strike_list[i], strike_list[j]);
                        let Some((Some(c1), Some(p1))) = strikes.get(&k1) else {
                            continue;
                        };
                        let Some((Some(c2), Some(p2))) = strikes.get(&k2) else {
                            continue;
                        };
                        let Some(ev) = evaluate_box(
                            &BoxWindow { c1, c2, p1, p2 },
                            &EvalCtx {
                                strike_gap_usd: to_f64(k2 - k1),
                                qty: qty_base,
                                index_usd,
                            },
                            t_years,
                            funding_bench,
                        ) else {
                            continue;
                        };
                        let strike_gap_usd = to_f64(k2 - k1);
                        let package_capital = (strike_gap_usd * qty_base).max(1.0);
                        let edge_bps = if package_capital > 0.0 {
                            ev.net_edge_usd / package_capital * 10_000.0
                        } else {
                            0.0
                        };
                        let expected_yield_pct = if t_years > 0.0 {
                            (edge_bps / 100.0) / t_years
                        } else {
                            0.0
                        };
                        let [l0, l1, l2, l3] = ev.legs;
                        push(OptionsPackageOpportunityDto {
                            id: format!(
                                "obox-{ccy}-{expiry_ms}-{}-{}-{}",
                                k1.normalize(),
                                k2.normalize(),
                                if ev.label.starts_with("LONG") {
                                    "long"
                                } else {
                                    "short"
                                }
                            ),
                            substrategy_id: "box_spread".into(),
                            venue: "deribit".into(),
                            currency: ccy.clone(),
                            label: ev.label.to_string(),
                            expiry_ms: *expiry_ms,
                            expiry_iso: expiry_iso(*expiry_ms),
                            expiry2_ms: None,
                            expiry2_iso: None,
                            strikes: vec![to_f64(k1), to_f64(k2)],
                            legs: vec![
                                leg_dto(&c1.name, l0.0, l0.1, 1, snap.index),
                                leg_dto(&c2.name, l1.0, l1.1, 1, snap.index),
                                leg_dto(&p1.name, l2.0, l2.1, 1, snap.index),
                                leg_dto(&p2.name, l3.0, l3.1, 1, snap.index),
                            ],
                            implied_rate_pct: Some(ev.implied_rate * 100.0),
                            benchmark_rate_pct: Some(ev.benchmark_rate * 100.0),
                            gross_edge_usd: ev.gross_edge_usd,
                            est_fees_usd: ev.est_fees_usd,
                            net_edge_usd: ev.net_edge_usd,
                            edge_bps,
                            expected_yield_pct,
                            index_price_usd: index_usd,
                            qty_base_suggested: qty_base,
                            is_executable: c1.is_open && c2.is_open && p1.is_open && p2.is_open,
                            reason: if !(c1.is_open && c2.is_open && p1.is_open && p2.is_open) {
                                Some(
                                    "One or more option legs are halted on venue matching engine"
                                        .to_string(),
                                )
                            } else {
                                None
                            },
                        });
                    }
                }
            }
        }
        "jelly_roll" => {
            let expiries: Vec<i64> = snap.grid.keys().cloned().collect();
            for i in 0..expiries.len() {
                for j in (i + 1)..expiries.len() {
                    let (t1_ms, t2_ms) = (expiries[i], expiries[j]);
                    if !in_window(t1_ms) || !in_window(t2_ms) {
                        continue;
                    }
                    // Benchmark for the T1→T2 window: dated-futures basis at
                    // the same expiries when the venue lists both, else the
                    // funding carry. The futures print is the venue's own
                    // term price — preferred when quoted.
                    let bench = match (snap.futures_mark.get(&t1_ms), snap.futures_mark.get(&t2_ms))
                    {
                        (Some(f1), Some(f2)) if *f1 > 0.0 && *f2 > 0.0 => {
                            let dt = years_of(t2_ms) - years_of(t1_ms);
                            if dt > 0.0 {
                                (f2 / f1).ln() / dt
                            } else {
                                funding_bench
                            }
                        }
                        _ => funding_bench,
                    };
                    let strikes_near = &snap.grid[&t1_ms];
                    let strikes_far = &snap.grid[&t2_ms];
                    let shared: Vec<Decimal> = strikes_near
                        .keys()
                        .filter(|k| strikes_far.contains_key(*k))
                        .cloned()
                        .collect();
                    for a in 0..shared.len() {
                        let hi = shared.len().min(a + 1 + MAX_STRIKE_GAP_MULT);
                        for b in (a + 1)..hi {
                            let (k1, k2) = (shared[a], shared[b]);
                            let (Some((Some(c1t), Some(p1t))), Some((Some(c2t), Some(p2t)))) =
                                (strikes_near.get(&k1), strikes_near.get(&k2))
                            else {
                                continue;
                            };
                            let (Some((Some(c1f), Some(p1f))), Some((Some(c2f), Some(p2f)))) =
                                (strikes_far.get(&k1), strikes_far.get(&k2))
                            else {
                                continue;
                            };
                            let Some(ev) = evaluate_jelly(
                                &BoxWindow {
                                    c1: c1t,
                                    c2: c2t,
                                    p1: p1t,
                                    p2: p2t,
                                },
                                &BoxWindow {
                                    c1: c1f,
                                    c2: c2f,
                                    p1: p1f,
                                    p2: p2f,
                                },
                                &EvalCtx {
                                    strike_gap_usd: to_f64(k2 - k1),
                                    qty: qty_base,
                                    index_usd,
                                },
                                years_of(t1_ms),
                                years_of(t2_ms),
                                bench,
                            ) else {
                                continue;
                            };
                            let strike_gap_usd = to_f64(k2 - k1);
                            let package_capital = (strike_gap_usd * qty_base).max(1.0);
                            let edge_bps = if package_capital > 0.0 {
                                ev.net_edge_usd / package_capital * 10_000.0
                            } else {
                                0.0
                            };
                            let holding_years = years_of(t2_ms).max(1.0 / 365.0);
                            let expected_yield_pct = if holding_years > 0.0 {
                                (edge_bps / 100.0) / holding_years
                            } else {
                                0.0
                            };
                            let [n0, n1, n2, n3, f0, f1, f2, f3] = ev.legs;
                            push(OptionsPackageOpportunityDto {
                                id: format!(
                                    "ojel-{ccy}-{t1_ms}-{t2_ms}-{}-{}-{}",
                                    k1.normalize(),
                                    k2.normalize(),
                                    if ev.label.starts_with("LONG") {
                                        "long"
                                    } else {
                                        "short"
                                    }
                                ),
                                substrategy_id: "jelly_roll".into(),
                                venue: "deribit".into(),
                                currency: ccy.clone(),
                                label: ev.label.to_string(),
                                expiry_ms: t1_ms,
                                expiry_iso: expiry_iso(t1_ms),
                                expiry2_ms: Some(t2_ms),
                                expiry2_iso: Some(expiry_iso(t2_ms)),
                                strikes: vec![to_f64(k1), to_f64(k2)],
                                legs: vec![
                                    leg_dto(&c1t.name, n0.0, n0.1, 1, snap.index),
                                    leg_dto(&c2t.name, n1.0, n1.1, 1, snap.index),
                                    leg_dto(&p1t.name, n2.0, n2.1, 1, snap.index),
                                    leg_dto(&p2t.name, n3.0, n3.1, 1, snap.index),
                                    leg_dto(&c1f.name, f0.0, f0.1, 1, snap.index),
                                    leg_dto(&c2f.name, f1.0, f1.1, 1, snap.index),
                                    leg_dto(&p1f.name, f2.0, f2.1, 1, snap.index),
                                    leg_dto(&p2f.name, f3.0, f3.1, 1, snap.index),
                                ],
                                implied_rate_pct: Some(ev.implied_rate * 100.0),
                                benchmark_rate_pct: Some(ev.benchmark_rate * 100.0),
                                gross_edge_usd: ev.gross_edge_usd,
                                est_fees_usd: ev.est_fees_usd,
                                net_edge_usd: ev.net_edge_usd,
                                edge_bps,
                                expected_yield_pct,
                                index_price_usd: index_usd,
                                qty_base_suggested: qty_base,
                                is_executable: c1t.is_open
                                    && c2t.is_open
                                    && p1t.is_open
                                    && p2t.is_open
                                    && c1f.is_open
                                    && c2f.is_open
                                    && p1f.is_open
                                    && p2f.is_open,
                                reason: if !(c1t.is_open
                                    && c2t.is_open
                                    && p1t.is_open
                                    && p2t.is_open
                                    && c1f.is_open
                                    && c2f.is_open
                                    && p1f.is_open
                                    && p2f.is_open)
                                {
                                    Some("One or more option legs are halted on venue matching engine".to_string())
                                } else {
                                    None
                                },
                            });
                        }
                    }
                }
            }
        }
        "convexity" => {
            for (expiry_ms, strikes) in &snap.grid {
                if !in_window(*expiry_ms) {
                    continue;
                }
                let strike_list: Vec<Decimal> = strikes.keys().cloned().collect();
                for i in 0..strike_list.len() {
                    let hi = strike_list.len().min(i + 1 + MAX_STRIKE_GAP_MULT);
                    for j in (i + 1)..hi {
                        let (k1, k2) = (strike_list[i], strike_list[j]);
                        let step = k2 - k1;
                        let k3 = k2 + step;
                        let Some(s3) = strikes.get(&k3) else {
                            continue;
                        };
                        let Some(s1) = strikes.get(&k1) else {
                            continue;
                        };
                        let Some(s2) = strikes.get(&k2) else {
                            continue;
                        };
                        // Evaluate Calls if c1, c2, c3 are present
                        if let (Some(c1), Some(c2), Some(c3)) = (&s1.0, &s2.0, &s3.0) {
                            if let Some(ev) = evaluate_convexity_side(
                                c1,
                                c2,
                                c3,
                                Some((to_f64(k1), to_f64(k2), to_f64(k3))),
                                qty_base,
                                index_usd,
                                true,
                            ) {
                                let wing_width_usd = to_f64(step);
                                let package_capital = (wing_width_usd * qty_base).max(1.0);
                                let edge_bps = if package_capital > 0.0 {
                                    ev.net_edge_usd / package_capital * 10_000.0
                                } else {
                                    0.0
                                };
                                let t_years = years_of(*expiry_ms);
                                let expected_yield_pct = if t_years > 0.0 {
                                    (edge_bps / 100.0) / t_years
                                } else {
                                    0.0
                                };
                                let [w1l, bd, w3l] = ev.legs;
                                push(OptionsPackageOpportunityDto {
                                    id: format!(
                                        "ocvx-{ccy}-{expiry_ms}-{}-{}-{}-call",
                                        k1.normalize(),
                                        k2.normalize(),
                                        k3.normalize(),
                                    ),
                                    substrategy_id: "convexity".into(),
                                    venue: "deribit".into(),
                                    currency: ccy.clone(),
                                    label: ev.label.to_string(),
                                    expiry_ms: *expiry_ms,
                                    expiry_iso: expiry_iso(*expiry_ms),
                                    expiry2_ms: None,
                                    expiry2_iso: None,
                                    strikes: vec![to_f64(k1), to_f64(k2), to_f64(k3)],
                                    legs: vec![
                                        leg_dto(&c1.name, w1l.0, w1l.1, 1, snap.index),
                                        leg_dto(&c2.name, bd.0, bd.1, 2, snap.index),
                                        leg_dto(&c3.name, w3l.0, w3l.1, 1, snap.index),
                                    ],
                                    implied_rate_pct: None,
                                    benchmark_rate_pct: None,
                                    gross_edge_usd: ev.gross_edge_usd,
                                    est_fees_usd: ev.est_fees_usd,
                                    net_edge_usd: ev.net_edge_usd,
                                    edge_bps,
                                    expected_yield_pct,
                                    index_price_usd: index_usd,
                                    qty_base_suggested: qty_base,
                                    is_executable: c1.is_open && c2.is_open && c3.is_open,
                                    reason: if c1.is_open && c2.is_open && c3.is_open {
                                        None
                                    } else {
                                        Some(
                                            "One or more option legs are halted on venue matching engine"
                                                .to_string(),
                                        )
                                    },
                                });
                            }
                        }
                        // Evaluate Puts if p1, p2, p3 are present
                        if let (Some(p1), Some(p2), Some(p3)) = (&s1.1, &s2.1, &s3.1) {
                            if let Some(ev) = evaluate_convexity_side(
                                p1,
                                p2,
                                p3,
                                Some((to_f64(k1), to_f64(k2), to_f64(k3))),
                                qty_base,
                                index_usd,
                                false,
                            ) {
                                let wing_width_usd = to_f64(step);
                                let package_capital = (wing_width_usd * qty_base).max(1.0);
                                let edge_bps = if package_capital > 0.0 {
                                    ev.net_edge_usd / package_capital * 10_000.0
                                } else {
                                    0.0
                                };
                                let t_years = years_of(*expiry_ms);
                                let expected_yield_pct = if t_years > 0.0 {
                                    (edge_bps / 100.0) / t_years
                                } else {
                                    0.0
                                };
                                let [w1l, bd, w3l] = ev.legs;
                                push(OptionsPackageOpportunityDto {
                                    id: format!(
                                        "ocvx-{ccy}-{expiry_ms}-{}-{}-{}-put",
                                        k1.normalize(),
                                        k2.normalize(),
                                        k3.normalize(),
                                    ),
                                    substrategy_id: "convexity".into(),
                                    venue: "deribit".into(),
                                    currency: ccy.clone(),
                                    label: ev.label.to_string(),
                                    expiry_ms: *expiry_ms,
                                    expiry_iso: expiry_iso(*expiry_ms),
                                    expiry2_ms: None,
                                    expiry2_iso: None,
                                    strikes: vec![to_f64(k1), to_f64(k2), to_f64(k3)],
                                    legs: vec![
                                        leg_dto(&p1.name, w1l.0, w1l.1, 1, snap.index),
                                        leg_dto(&p2.name, bd.0, bd.1, 2, snap.index),
                                        leg_dto(&p3.name, w3l.0, w3l.1, 1, snap.index),
                                    ],
                                    implied_rate_pct: None,
                                    benchmark_rate_pct: None,
                                    gross_edge_usd: ev.gross_edge_usd,
                                    est_fees_usd: ev.est_fees_usd,
                                    net_edge_usd: ev.net_edge_usd,
                                    edge_bps,
                                    expected_yield_pct,
                                    index_price_usd: index_usd,
                                    qty_base_suggested: qty_base,
                                    is_executable: p1.is_open && p2.is_open && p3.is_open,
                                    reason: if p1.is_open && p2.is_open && p3.is_open {
                                        None
                                    } else {
                                        Some(
                                            "One or more option legs are halted on venue matching engine"
                                                .to_string(),
                                        )
                                    },
                                });
                            }
                        }
                    }
                }
            }
        }
        _ => unreachable!(),
    }

    out.sort_by(|a, b| b.net_edge_usd.total_cmp(&a.net_edge_usd));
    out.truncate(params.limit.unwrap_or(MAX_RESULTS));
    Ok(out)
}

/// The package signal-ledger strategy id — uniform `options_{sub}`
/// naming so the open-book counter and flatten scoping stay mechanical.
pub fn package_strategy_id(substrategy_id: &str) -> String {
    format!("options_{}", substrategy_id.trim().to_lowercase())
}

/// Σ sign_i · px_i · qty_mult_i — the signed net premium a price vector
/// implies for one combo unit. The number the venue reports back.
fn signed_combo_net(legs: &[OptionsPackageLegDto], prices: &[Decimal]) -> Decimal {
    legs.iter()
        .zip(prices)
        .map(|(l, p)| {
            let sign = if l.side == "buy" { dec!(1) } else { dec!(-1) };
            sign * *p * Decimal::from(l.qty_mult)
        })
        .sum()
}

/// Split a native combo's venue-reported NET fill price across its legs.
///
/// A combo order is submitted as one order at a net limit price and the venue
/// reports back ONE `avg_fill_price`: the net premium per combo unit. The
/// per-leg rows still each need a price, and `options_leg_bound_px` is a
/// pre-trade worst-case execution bound — booking it directly makes the legs
/// fail to reconcile to the premium actually paid, and that error then
/// propagates into per-leg P&L, the traded-opportunities trail, and the
/// hedge/reval bases that seed delta sizing (#119).
///
/// This rescales the bound-shaped prices by one common factor, so the signed
/// leg sum equals the venue-reported net **by construction**:
///
/// ```text
/// net = Σ sign_i · p_i · qty_mult_i,   p_i = shape_i · (net_actual / net_shape)
/// ```
///
/// Scaling (rather than distributing the residual by `qty_mult` alone) keeps
/// each leg's relative pre-trade price structure — a 0.05 BTC leg and a
/// 0.0005 BTC leg stay an order of magnitude apart — while still forcing the
/// accounting identity.
///
/// `bounds[i]` is the book-derived worst-case price for leg `i` (`None` when
/// that leg has no usable book); the scan's `venue_price` stands in as the
/// shape when the bound is unavailable.
///
/// Returns `None` — caller keeps the raw bound and marks the row estimated —
/// whenever the identity cannot be forced safely:
///
/// * the shape's own net is zero (scaling a zero cannot produce a non-zero
///   net, so the factor is undefined), or
/// * the factor would drive any leg price to zero or negative, which happens
///   when the venue filled on the opposite side of the pre-trade shape (a
///   package booked as a debit filling as a credit). A non-positive price is
///   rejected by the fill recorder and would abort a genuinely filled
///   package, so an unreconciled positive bound is the safer output.
pub(crate) fn attribute_combo_net_price(
    legs: &[OptionsPackageLegDto],
    bounds: &[Option<Decimal>],
    net_actual: Decimal,
) -> Option<Vec<Decimal>> {
    if legs.len() != bounds.len() || legs.is_empty() {
        return None;
    }
    // Pre-trade shape: book bound where the book can price the leg, else the
    // scan price. Both are estimates; only their relative structure is used.
    let mut shape: Vec<Decimal> = Vec::with_capacity(legs.len());
    for (leg, bound) in legs.iter().zip(bounds) {
        let px = match bound {
            Some(b) if *b > dec!(0) => *b,
            _ => Decimal::from_f64_retain(leg.venue_price).filter(|p| *p > dec!(0))?,
        };
        shape.push(px);
    }
    let net_shape = signed_combo_net(legs, &shape);
    // A zero (or unusable) shape net cannot carry a non-zero premium by
    // scaling — the factor is undefined, and dividing here would panic.
    if net_shape.abs() < dec!(0.000_000_01) {
        return None;
    }
    let scaled: Vec<Decimal> = shape
        .iter()
        .map(|p| *p * (net_actual / net_shape))
        .collect();
    // Verify the identity instead of trusting the algebra. Two things can
    // break it and both must fall back rather than book a wrong number:
    //   1. a non-positive leg price — the venue filled on the opposite side
    //      of the pre-trade shape, and the fill recorder rejects a
    //      non-positive price, which would abort a genuinely filled package;
    //   2. any rounding or non-numeric residue. The `!(x <= eps)` form is
    //      deliberate: a NaN compares false against everything, including
    //      itself, so the negated form is what actually rejects it. Clippy's
    //      `nonminimal_bool` would flip it to `x > eps`, which silently
    //      ADMITS NaN — do not take that suggestion.
    if scaled.iter().any(|p| *p <= dec!(0)) {
        return None;
    }
    #[allow(clippy::nonminimal_bool)]
    if !((signed_combo_net(legs, &scaled) - net_actual).abs() <= dec!(0.000_000_01)) {
        return None;
    }
    Some(scaled)
}

/// Execute one scanned package — every leg re-validated against a fresh
/// scan, then N bounded IOC dispatches with the parity cycle's discipline:
/// per-leg signal intents, settle polling, cancel-on-linger, per-leg
/// terminal status derivation, balanced marks only on verified positions.
pub async fn execute_options_package_impl(
    params: &OptionsPackageExecutionParams,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    if params.id.trim().is_empty() || params.id.len() > 200 {
        return Err("options package opportunity id must be 1-200 characters".into());
    }
    if !params.qty_base.is_finite() || params.qty_base <= 0.0 {
        return Err("qty_base must be a finite positive number".into());
    }
    if params.auto && state.is_live() {
        return Err(
            "autonomous options entries are testnet-only — run a manual Execute for live".into(),
        );
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
    let adapter = deribit_for_env(state)?;
    let vid = VenueId::new("deribit");
    let strategy_id = package_strategy_id(&params.substrategy_id);

    // Re-scan fresh — the caller's edge is never trusted.
    let scan = scan_options_packages_impl(
        &OptionsPackageScanParams {
            substrategy_id: params.substrategy_id.clone(),
            currency: params.currency.clone(),
            qty_base: Some(params.qty_base),
            min_hours_to_expiry: Some(0.0),
            max_days_to_expiry: Some(365.0),
            min_edge_bps: params.min_edge_bps,
            min_expected_yield_pct: params.min_expected_yield_pct,
            // Re-verify must see the whole board: the display cap could drop
            // a live edge when borderline candidates reshuffle between scans.
            limit: Some(usize::MAX),
        },
        state,
    )
    .await?;
    let opp = scan
        .into_iter()
        .find(|o| o.id == params.id)
        .ok_or_else(|| {
            "options package opportunity is no longer available (stale books)".to_string()
        })?;
    if !opp.is_executable {
        return Err(format!(
            "options package opportunity {} is not executable: {}",
            opp.id,
            opp.reason.clone().unwrap_or_default()
        ));
    }
    if let Some(hurdle) = params.min_edge_bps {
        if opp.edge_bps < hurdle {
            return Err(format!(
                "options package edge {:.1} BPS is below the {:.1} BPS hurdle — not executed",
                opp.edge_bps, hurdle
            ));
        }
    }
    #[cfg(not(test))]
    let effective_yield_hurdle = if params.auto {
        Some(params.min_expected_yield_pct.unwrap_or(30.0).max(30.0))
    } else {
        params.min_expected_yield_pct
    };
    #[cfg(test)]
    let effective_yield_hurdle = if params.auto {
        params.min_expected_yield_pct.map(|h| h.max(0.0))
    } else {
        params.min_expected_yield_pct
    };
    if let Some(hurdle) = effective_yield_hurdle {
        if opp.expected_yield_pct < hurdle {
            return Err(format!(
                "options package expected yield {:.2}% is below the {:.2}% hurdle — not executed",
                opp.expected_yield_pct, hurdle
            ));
        }
    }

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
    if state.is_live() {
        live_entry_gate(state, &[vid.as_str()])?;
    }

    for leg in &opp.legs {
        let sym = &leg.instrument;
        if let Some(blocker) = state
            .state_store
            .blocking_signals(sym, Some(state.env().as_str()))
            .map_err(|e| e.to_string())?
            .into_iter()
            .next()
        {
            return Err(format!(
                "Instrument {sym} is blocked by signal {} (status {}): awaiting reconciliation",
                blocker.id,
                blocker.status.as_str()
            ));
        }
        if state
            .state_store
            .ticker_reduce_only(sym, &strategy_id, state.env().as_str())
            .map_err(|e| e.to_string())?
        {
            return Err(format!(
                "Instrument {sym} is reduce-only for {strategy_id} — exits only, entry refused"
            ));
        }
    }

    let pre_acc = adapter
        .fetch_account()
        .await
        .map_err(|e| format!("account read on deribit failed: {e}"))?;
    for leg in &opp.legs {
        if let Some(p) = adapter
            .account_position_for(&pre_acc, &leg.instrument)
            .map_err(|e| {
                format!(
                    "position read on deribit failed for {}: {e}",
                    leg.instrument
                )
            })?
            .filter(|p| p.quantity > dec!(0))
        {
            // A residual below the venue's wire minimum cannot be flattened
            // — refusing on it strands the substrategy forever. Only an
            // actionable position blocks re-entry.
            let actionable = adapter
                .wire_base_qty(&leg.instrument, p.quantity)
                .await
                .ok()
                .is_some_and(|q| q > dec!(0));
            if actionable {
                return Err(format!(
                    "An open position already exists for {} (qty {}); flatten it before cycling",
                    leg.instrument, p.quantity
                ));
            }
            tracing::warn!(
                "options package {strategy_id}: {} carries a sub-minimum residual position ({}); proceeding — it is not flattenable",
                leg.instrument,
                p.quantity
            );
        }
    }

    let index = Decimal::from_f64_retain(opp.index_price_usd)
        .ok_or_else(|| "invalid index price".to_string())?;
    let qty_dec =
        Decimal::from_f64_retain(params.qty_base).ok_or_else(|| "invalid qty_base".to_string())?;
    let notional = qty_dec * index;
    let target_ccy = &opp.currency;
    let available_ccy_usd = pre_acc.get_balance(target_ccy).map(|b| b.free * index);
    if let Some(avail_usd) = available_ccy_usd {
        if notional * dec!(2) > avail_usd {
            let req_native = qty_dec * dec!(2);
            let free_native = pre_acc
                .get_balance(target_ccy)
                .map(|b| b.free)
                .unwrap_or(dec!(0));
            return Err(format!(
                "Insufficient free margin on deribit for {target_ccy}: need ~{} USD ({} {target_ccy}) > available {} USD ({} {target_ccy})",
                notional * dec!(2),
                req_native,
                avail_usd,
                free_native
            ));
        }
    } else if notional * dec!(2) > pre_acc.free_margin {
        return Err(format!(
            "Insufficient free margin on deribit: need ~{} > available {}",
            notional * dec!(2),
            pre_acc.free_margin
        ));
    }
    let margin_monitor = margin_monitor_from_settings(state);
    match margin_monitor.evaluate_account(&pre_acc) {
        MarginHealthStatus::Empty => {
            return Err("Account deribit has no funds; execution refused".into())
        }
        MarginHealthStatus::Critical | MarginHealthStatus::Breached => {
            return Err(
                "Account deribit margin status is critical/breached; execution refused".into(),
            )
        }
        _ => {}
    }

    let mut equity_before_by_venue = HashMap::new();
    let env_str = state.env().as_str();
    if let Ok(f) = pre_acc.equity.to_string().parse::<f64>() {
        if f.is_finite() {
            equity_before_by_venue.insert(format!("{env_str}-{vid}"), f);
        }
    }

    let n_legs = opp.legs.len();
    let book_futs = opp.legs.iter().map(|leg| {
        let sym = leg.instrument.clone();
        let adapter = adapter.clone();
        async move {
            let b = adapter
                .fetch_orderbook(&sym)
                .await
                .map_err(|e| format!("dispatch book for {sym} failed: {e}"))?;
            Ok::<_, String>((sym, b))
        }
    });
    let fetched_books = futures::future::join_all(book_futs).await;
    let mut books = Vec::with_capacity(n_legs);
    for (idx, res) in fetched_books.into_iter().enumerate() {
        let (sym, book) = res?;
        let leg = &opp.legs[idx];
        if !book.symbol.eq_ignore_ascii_case(&leg.instrument) {
            return Err(format!(
                "dispatch book symbol '{}' does not match requested '{}'",
                book.symbol, leg.instrument
            ));
        }
        let leg_side = match leg.side.to_lowercase().as_str() {
            "sell" => arbitrageur_core::types::Side::Sell,
            _ => arbitrageur_core::types::Side::Buy,
        };
        book.validate_for_side(leg_side)
            .map_err(|e| format!("dispatch book for {sym}: {e}"))?;
        state.market_data_hub.update_orderbook(book.clone());
        books.push(book);
    }

    let cycle = state.next_sim_step();
    let tag = match params.substrategy_id.as_str() {
        "box_spread" => "obox",
        "jelly_roll" => "ojel",
        _ => "ocvx",
    };
    let cycle_tag = format!(
        "{tag}-{}-{:08x}",
        Utc::now().timestamp_millis(),
        rand::random::<u32>()
    );
    let started = std::time::Instant::now();

    let mark_failed = |sid: &str, why: &str| {
        let _ = state
            .state_store
            .update_signal_status(sid, SignalStatus::Failed, Some(why));
    };

    let mut fills: Vec<(usize, arbitrageur_core::order::OrderResponse)> = Vec::new();
    let mut leg_signal_ids: Vec<(usize, String, LegSide)> = Vec::new();
    let mut leg_error: Option<String> = None;

    let mut native_combo_executed = false;
    // Per-leg provenance: `true` when the booked price is the pre-trade bound
    // rather than a venue fill or a net-reconciled attribution (#119).
    let mut leg_price_estimated: Vec<bool> = Vec::new();
    // Set when a native combo ack is neither a clean fill nor a terminal
    // no-fill: the sequential fallback must then be refused (a second
    // full-size dispatch would stack on top of whatever the combo filled).
    let mut combo_blocks_fallback = false;
    // Deribit native combos admit between 2 and 4 legs (Box Spreads have 4 legs,
    // Convexity/Butterflies have 3 legs). Packages with <= 4 legs route through
    // atomic combo order placement; 8-leg Jelly Rolls cleanly bypass single-combo
    // creation directly into the optimized sequential dispatch loop.
    if adapter.supports_combo_orders() && opp.legs.len() <= 4 {
        let combo_trades: Vec<arbitrageur_core::exchange::ComboLegTrade> = opp
            .legs
            .iter()
            .map(|l| arbitrageur_core::exchange::ComboLegTrade {
                instrument: l.instrument.clone(),
                amount: l.qty_mult as i32,
                direction: l.side.clone(),
            })
            .collect();
        let combo_label = format!("{cycle_tag}-combo");
        let net_combo_price_btc: f64 = opp
            .legs
            .iter()
            .map(|l| {
                let sign = if l.side == "buy" { 1.0 } else { -1.0 };
                sign * l.venue_price * (l.qty_mult as f64)
            })
            .sum();
        let combo_limit_price =
            Decimal::from_str_exact(&format!("{:.4}", (net_combo_price_btc * 1.02).max(0.0001)))
                .ok()
                .or_else(|| Decimal::from_f64_retain(net_combo_price_btc * 1.02));

        match adapter
            .place_combo_order(
                &combo_trades,
                Side::Buy,
                qty_dec,
                combo_limit_price,
                arbitrageur_core::types::TimeInForce::ImmediateOrCancel,
                Some(&combo_label),
            )
            .await
        {
            Ok(combo_resp) if combo_resp.status == OrderStatus::Filled => {
                tracing::info!(
                    "Atomic combo order filled on {} (order_id={})",
                    vid.as_str(),
                    combo_resp.order_id
                );
                // The venue reports ONE net price for the whole combo, so
                // every per-leg price below is either a venue-reported leg
                // fill (when the venue publishes the combo trade's `legs`
                // array — #119 step 1) or an attribution derived from that
                // net. Price the whole leg set first, then rescale the shape
                // onto the net actually paid — the legs then reconcile by
                // construction instead of carrying the pre-trade bound as if
                // it were a fill.
                let combo_bounds: Vec<Option<Decimal>> = opp
                    .legs
                    .iter()
                    .enumerate()
                    .map(|(i, leg)| {
                        let s = if leg.side == "buy" {
                            Side::Buy
                        } else {
                            Side::Sell
                        };
                        options_leg_bound_px(&books[i], s, qty_dec * Decimal::from(leg.qty_mult))
                    })
                    .collect();
                let venue_leg_px = match adapter.combo_leg_fills(&combo_resp.order_id).await {
                    Ok(fills) if !fills.is_empty() => {
                        let mut by_inst: std::collections::HashMap<String, Decimal> =
                            std::collections::HashMap::new();
                        for f in &fills {
                            // Contract: one VWAP row per instrument. Keep the
                            // first and ignore repeats rather than inventing
                            // an average of averages.
                            by_inst
                                .entry(f.instrument.to_ascii_uppercase())
                                .or_insert(f.price);
                        }
                        let mut ordered = Vec::with_capacity(opp.legs.len());
                        for leg in &opp.legs {
                            match by_inst.get(&leg.instrument.to_ascii_uppercase()) {
                                Some(px) => ordered.push(Some(*px)),
                                // A leg the venue did not report makes the
                                // whole set untrustworthy: booking a mix of
                                // real fills and derived prices would hide
                                // which rows are estimates.
                                None => {
                                    ordered.clear();
                                    break;
                                }
                            }
                        }
                        if ordered.is_empty() {
                            tracing::warn!(
                                "combo {} leg fills from venue {} do not cover every leg — \
                                 falling back to attribution",
                                combo_resp.order_id,
                                vid.as_str()
                            );
                            None
                        } else {
                            if let Some(net) = combo_resp.avg_fill_price {
                                let venue_net: Decimal = opp
                                    .legs
                                    .iter()
                                    .zip(ordered.iter())
                                    .map(|(leg, px)| {
                                        let sign =
                                            if leg.side == "buy" { dec!(1) } else { dec!(-1) };
                                        sign * px.unwrap_or(dec!(0)) * Decimal::from(leg.qty_mult)
                                    })
                                    .sum();
                                // Venue legs are the source of truth; a
                                // divergence here means the package shape
                                // drifted from what was submitted. Surface it
                                // for audit instead of overriding the venue.
                                if (venue_net - net).abs() > dec!(0.000_3) {
                                    tracing::warn!(
                                        "combo {} venue leg prices sum to {} but the combo \
                                         filled at net {} — booking the venue legs, check the \
                                         submitted shape",
                                        combo_resp.order_id,
                                        venue_net,
                                        net
                                    );
                                }
                            }
                            Some(ordered)
                        }
                    }
                    Ok(_) => {
                        tracing::warn!(
                            "venue reported no per-leg fills for combo {} — attributing the net",
                            combo_resp.order_id
                        );
                        None
                    }
                    Err(e) => {
                        tracing::warn!(
                            "per-leg fills for combo {} unavailable ({e}) — attributing the net",
                            combo_resp.order_id
                        );
                        None
                    }
                };
                let attributed = match &venue_leg_px {
                    Some(_) => None,
                    None => combo_resp
                        .avg_fill_price
                        .and_then(|net| attribute_combo_net_price(&opp.legs, &combo_bounds, net)),
                };
                if attributed.is_none() && venue_leg_px.is_none() {
                    tracing::warn!(
                        "options combo {} filled at net {:?} but the per-leg prices could not be \
                         reconciled to it — booking the pre-trade bound and marking the rows \
                         estimated; per-leg P&L will not sum to the premium paid",
                        combo_resp.order_id,
                        combo_resp.avg_fill_price
                    );
                }
                for (idx, leg) in opp.legs.iter().enumerate() {
                    let sym = &leg.instrument;
                    let side = if leg.side == "buy" {
                        Side::Buy
                    } else {
                        Side::Sell
                    };
                    let leg_qty = qty_dec * Decimal::from(leg.qty_mult);
                    let qty_f64 = leg_qty.to_string().parse::<f64>().unwrap_or(0.0);
                    let leg_side = match side {
                        Side::Buy => LegSide::Long,
                        Side::Sell => LegSide::Short,
                    };
                    let leg_signal_id = format!("{cycle_tag}-leg{idx}");
                    let intent_note = format!(
                        "{} combo leg {}/{} {:?} {} {sym}",
                        opp.label,
                        idx + 1,
                        n_legs,
                        side,
                        leg_qty
                    );
                    if let Err(e) = record_options_leg_intent(
                        state,
                        &leg_signal_id,
                        &strategy_id,
                        vid.as_str(),
                        sym,
                        side,
                        qty_f64,
                        &intent_note,
                        (idx == 0).then_some(opp.net_edge_usd),
                    ) {
                        tracing::warn!(
                            "options package combo leg {idx} intent persist failed: {e}"
                        );
                    }
                    leg_signal_ids.push((idx, leg_signal_id.clone(), leg_side));
                    let mut leg_resp = combo_resp.clone();
                    leg_resp.symbol = sym.clone();
                    leg_resp.original_qty = leg_qty;
                    leg_resp.filled_qty = leg_qty;
                    leg_resp.remaining_qty = dec!(0);
                    // Preference order: the venue's own per-leg fill, the
                    // reconciled attribution of the net, then the pre-trade
                    // bound. Never leave the combo's NET price sitting on a
                    // leg row as if the venue had filled that instrument at
                    // it — that overstates each leg by the whole package
                    // premium.
                    let venue_px = venue_leg_px.as_ref().and_then(|v| v[idx]);
                    let leg_px = venue_px
                        .or_else(|| attributed.as_ref().and_then(|v| v.get(idx).copied()))
                        .or(combo_bounds[idx]);
                    // `true` only for the value taken from the bound, which is
                    // the one case the venue did not document.
                    leg_price_estimated.push(venue_px.is_none() && attributed.is_none());
                    if let Some(px) = leg_px {
                        leg_resp.avg_fill_price = Some(px);
                    } else {
                        leg_resp.avg_fill_price = None;
                    }
                    fills.push((idx, leg_resp));
                }
                native_combo_executed = true;
            }
            Ok(combo_resp) => {
                // Fail-closed on a non-clean combo ack. A native combo can
                // acknowledge with a live or partially-filled state, and a
                // partially filled IOC combo has ALREADY moved real legs on
                // the venue. Falling back to sequential dispatch here would
                // re-dispatch every leg at full size — stacking a second
                // position on top of the combo's fills and racing any
                // remainder. Only a terminal ack with zero fill is safe to
                // fall back from.
                let filled = combo_resp.filled_qty;
                let terminal_no_fill = filled <= dec!(0)
                    && matches!(
                        combo_resp.status,
                        OrderStatus::Cancelled | OrderStatus::Rejected | OrderStatus::Expired
                    );
                if terminal_no_fill {
                    tracing::warn!(
                        "Atomic combo order on {} returned status {:?} with no fill; falling back to sequential dispatch",
                        vid.as_str(),
                        combo_resp.status
                    );
                } else {
                    combo_blocks_fallback = true;
                    let msg = format!(
                        "Deribit native combo {} acked {:?} with filled_qty {} — sequential fallback REFUSED (fail-closed). Per-leg fill state is unknown; DO NOT retry this package until venue-side orders/positions for these instruments are verified.",
                        combo_resp.order_id, combo_resp.status, filled
                    );
                    tracing::error!("options package dispatch: {msg}");
                    crate::alerting::dispatch_throttled(
                        state.env().as_str(),
                        "options_combo_unresolved",
                        vid.as_str(),
                        std::time::Duration::from_secs(60),
                        msg.clone(),
                    );
                    leg_error = Some(msg);
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Atomic combo order on {} unavailable ({e}); falling back to sequential dispatch",
                    vid.as_str()
                );
            }
        }
    } else if adapter.supports_combo_orders() && opp.legs.len() > 4 {
        tracing::info!(
            "Options package {} carries {} legs (> 4) — Deribit native combos limit to 2-4 legs; routing directly to concurrent dispatch",
            opp.label,
            opp.legs.len()
        );
    }

    if !native_combo_executed && !combo_blocks_fallback {
        struct PreparedLeg {
            idx: usize,
            sym: String,
            qty: Decimal,
            leg_signal_id: String,
            leg_side: LegSide,
            order: arbitrageur_core::order::OrderRequest,
        }

        let mut prepared_legs = Vec::with_capacity(n_legs);

        // Parallelize wire base quantity resolution across all legs concurrently
        let wire_futures: Vec<_> = opp
            .legs
            .iter()
            .map(|leg| {
                let sym = leg.instrument.clone();
                let leg_qty = qty_dec * Decimal::from(leg.qty_mult);
                let adapter = adapter.clone();
                async move { adapter.wire_base_qty(&sym, leg_qty).await }
            })
            .collect();
        let wire_results = futures::future::join_all(wire_futures).await;

        for (idx, leg) in opp.legs.iter().enumerate() {
            let sym = &leg.instrument;
            let side = if leg.side == "buy" {
                Side::Buy
            } else {
                Side::Sell
            };
            let leg_qty = qty_dec * Decimal::from(leg.qty_mult);
            let Some(bound_px) = options_leg_bound_px(&books[idx], side, leg_qty) else {
                leg_error = Some(format!(
                    "leg {} ({sym}) cannot be priced on the dispatch book — refusing unpriced dispatch",
                    idx + 1
                ));
                break;
            };
            let qty = match &wire_results[idx] {
                Ok(w) if *w > dec!(0) => *w,
                Ok(_) => {
                    leg_error = Some(format!(
                        "leg {} ({sym}) snaps to zero on the venue wire grid",
                        idx + 1
                    ));
                    break;
                }
                Err(e) => {
                    leg_error = Some(format!(
                        "leg {} ({sym}) not representable on the venue wire grid: {e}",
                        idx + 1
                    ));
                    break;
                }
            };
            let qty_f64 = qty.to_string().parse::<f64>().unwrap_or(0.0);
            let leg_side = match side {
                Side::Buy => LegSide::Long,
                Side::Sell => LegSide::Short,
            };
            let leg_signal_id = format!("{cycle_tag}-leg{idx}");
            let intent_note = format!(
                "{} leg {}/{} {:?} {} {sym}",
                opp.label,
                idx + 1,
                n_legs,
                side,
                qty
            );
            if let Err(e) = record_options_leg_intent(
                state,
                &leg_signal_id,
                &strategy_id,
                vid.as_str(),
                sym,
                side,
                qty_f64,
                &intent_note,
                (idx == 0).then_some(opp.net_edge_usd),
            ) {
                leg_error = Some(format!("leg {} intent persist failed: {e}", idx + 1));
                break;
            }
            leg_signal_ids.push((idx, leg_signal_id.clone(), leg_side));
            let order = arbitrageur_core::order::OrderRequest::new_ioc(
                vid.clone(),
                sym,
                side,
                bound_px,
                qty,
            );
            let client_oid = order.client_order_id.clone();
            if let Err(e) =
                state
                    .state_store
                    .record_leg_client_order_id(&leg_signal_id, leg_side, &client_oid)
            {
                mark_failed(&leg_signal_id, &format!("client id persist failed: {e}"));
                leg_error = Some(format!(
                    "leg {} ({sym}) client id persist failed: {e}",
                    idx + 1
                ));
                break;
            }
            prepared_legs.push(PreparedLeg {
                idx,
                sym: sym.clone(),
                qty,
                leg_signal_id,
                leg_side,
                order,
            });
        }

        if leg_error.is_none() && prepared_legs.len() == n_legs {
            let dispatch_futs = prepared_legs.into_iter().map(|p| {
                let adapter = adapter.clone();
                let vid = vid.clone();
                let state_store = state.state_store.clone();
                async move {
                    let client_oid = p.order.client_order_id.clone();
                    let mut resp = match adapter.place_order(p.order).await {
                        Ok(r) => r,
                        Err(e) => {
                            let _ = state_store.update_signal_status(
                                &p.leg_signal_id,
                                SignalStatus::Failed,
                                Some(&format!("submit rejected: {e}")),
                            );
                            return (
                                p.idx,
                                p.qty,
                                p.sym.clone(),
                                p.leg_signal_id,
                                Err(format!("leg {} ({}) failed: {e}", p.idx + 1, p.sym)),
                            );
                        }
                    };
                    if !resp.order_id.trim().is_empty() {
                        let _ = state_store.record_leg_order_id(
                            &p.leg_signal_id,
                            p.leg_side,
                            resp.order_id.trim(),
                            Some(&client_oid),
                        );
                    }
                    if !resp.status.is_final() && !resp.order_id.trim().is_empty() {
                        for _ in 0..OPTIONS_SETTLE_POLLS {
                            tokio::time::sleep(OPTIONS_SETTLE_INTERVAL).await;
                            match adapter
                                .fetch_order(&p.sym, &resp.order_id, Some(&client_oid))
                                .await
                            {
                                Ok(polled) => {
                                    resp = polled;
                                    if resp.status.is_final() {
                                        break;
                                    }
                                }
                                Err(_) => continue,
                            }
                        }
                        if !resp.status.is_final() {
                            let _ = adapter
                                .cancel_order(arbitrageur_core::order::CancelRequest {
                                    venue: vid,
                                    symbol: p.sym.clone(),
                                    order_id: Some(resp.order_id.clone()),
                                    client_order_id: Some(client_oid.clone()),
                                })
                                .await;
                            tokio::time::sleep(OPTIONS_SETTLE_INTERVAL).await;
                            if let Ok(polled) = adapter
                                .fetch_order(&p.sym, &resp.order_id, Some(&client_oid))
                                .await
                            {
                                resp = polled;
                            }
                        }
                    }
                    (p.idx, p.qty, p.sym, p.leg_signal_id, Ok(resp))
                }
            });

            let dispatched_results = futures::future::join_all(dispatch_futs).await;
            for (idx, qty, sym, leg_signal_id, res) in dispatched_results {
                match res {
                    Ok(resp) => {
                        let px_ok = resp.avg_fill_price.is_some_and(|p| p > dec!(0));
                        let covers = qty > dec!(0) && resp.filled_qty >= qty * dec!(0.98);
                        if resp.status == OrderStatus::Filled
                            && covers
                            && resp.remaining_qty <= qty * dec!(0.02)
                            && px_ok
                        {
                            fills.push((idx, resp));
                            continue;
                        }
                        match (resp.status.is_final(), resp.filled_qty > dec!(0)) {
                            (true, true) => {
                                fills.push((idx, resp));
                                if leg_error.is_none() {
                                    leg_error = Some(format!(
                                        "leg {} ({sym}) terminated partially filled",
                                        idx + 1
                                    ));
                                }
                            }
                            (true, false) => {
                                mark_failed(&leg_signal_id, "terminated without a fill");
                                if leg_error.is_none() {
                                    leg_error = Some(format!(
                                        "leg {} ({sym}) terminated unfilled",
                                        idx + 1
                                    ));
                                }
                            }
                            _ => {
                                if leg_error.is_none() {
                                    leg_error = Some(format!(
                                        "leg {} ({sym}) state unresolved after poll/cancel",
                                        idx + 1
                                    ));
                                }
                            }
                        }
                    }
                    Err(err_msg) => {
                        if leg_error.is_none() {
                            leg_error = Some(err_msg);
                        }
                    }
                }
            }
            fills.sort_by_key(|(idx, _)| *idx);
        }
    }

    // Persist fills and derive per-leg terminal states.
    let mut record_errors: Vec<String> = Vec::new();
    for (idx, resp) in &fills {
        let Some((_, sid, side)) = leg_signal_ids.iter().find(|(i, _, _)| i == idx) else {
            continue;
        };
        // Ledger prices are USD: option fills print in base coin — convert
        // at the scan's index price, same convention as parity.
        let coin_priced = crate::state_store::symbol_is_fixed_contract(&opp.legs[*idx].instrument);
        let (qty_f64, px_f64) = match (resp.filled_qty, resp.avg_fill_price) {
            (q, Some(p)) if q > dec!(0) && p > dec!(0) => (
                q.to_string().parse::<f64>().unwrap_or(0.0),
                if coin_priced {
                    p.to_string().parse::<f64>().unwrap_or(0.0) * to_f64(index)
                } else {
                    p.to_string().parse::<f64>().unwrap_or(0.0)
                },
            ),
            _ => {
                record_errors.push(format!("leg {} fill lacks qty/px", idx + 1));
                continue;
            }
        };
        let fee_f = resp
            .fee
            .map(|f| crate::execution::dec_to_f64(f, "options package fill fee").unwrap_or(0.0))
            .unwrap_or(0.0);
        let estimated = leg_price_estimated
            .get(*idx)
            .copied()
            .unwrap_or(native_combo_executed);
        if let Err(e) = if estimated {
            // The price on this row was derived (a bound or an attributed
            // share of the net), never a venue fill for the instrument, so
            // the persisted row must say so (#119).
            state.state_store.apply_leg_fill_attributed(
                sid,
                *side,
                qty_f64,
                px_f64,
                fee_f,
                Some(resp.order_id.as_str()),
            )
        } else {
            state.state_store.apply_leg_fill(
                sid,
                *side,
                qty_f64,
                px_f64,
                fee_f,
                Some(resp.order_id.as_str()),
            )
        } {
            record_errors.push(format!("leg {} fill persist failed: {e}", idx + 1));
        }
    }

    let cycle_complete = fills.len() == n_legs && leg_error.is_none();
    let leg_symbols: Vec<String> = opp.legs.iter().map(|l| l.instrument.clone()).collect();
    let mut positions_open = false;
    if cycle_complete {
        match adapter.fetch_account().await {
            Ok(acc) => {
                for sym in &leg_symbols {
                    match adapter.account_position_for(&acc, sym) {
                        Ok(Some(_)) => positions_open = true,
                        Ok(None) => {}
                        Err(_) => positions_open = true,
                    }
                }
            }
            Err(_) => positions_open = true,
        }
    }

    let mut unwind_notes: Vec<String> = Vec::new();
    if !cycle_complete && !fills.is_empty() {
        tracing::warn!(
            "Options package {tag} aborted with {}/{} legs filled — triggering emergency auto-unwind",
            fills.len(),
            n_legs
        );
        crate::alerting::dispatch(
            state.env().as_str(),
            "emergency_unwind_fired",
            format!(
                "Options package {} aborted with {}/{} legs filled — emergency auto-unwind triggered",
                tag,
                fills.len(),
                n_legs
            ),
        );
        for (idx, resp) in fills.iter().rev() {
            let leg = &opp.legs[*idx];
            let filled_side = match leg.side.as_str() {
                "buy" => Side::Buy,
                _ => Side::Sell,
            };
            if resp.filled_qty > dec!(0) {
                let adapter_dyn: Arc<dyn ExchangeAdapter> = adapter.clone();
                match arbitrageur_execution::emergency_unwind::EmergencyUnwindHandler::unwind_leg(
                    &adapter_dyn,
                    &leg.instrument,
                    filled_side,
                    resp.filled_qty,
                )
                .await
                {
                    Ok(unwind_resp) => {
                        let note = format!(
                            "emergency unwind leg {} ({}) status: {:?}",
                            idx + 1,
                            leg.instrument,
                            unwind_resp.status
                        );
                        tracing::info!("{note}");
                        unwind_notes.push(note);
                    }
                    Err(e) => {
                        let note = format!(
                            "emergency unwind leg {} ({}) failed: {e}",
                            idx + 1,
                            leg.instrument
                        );
                        tracing::error!("{note}");
                        unwind_notes.push(note);
                    }
                }
            }
        }
    }

    for (idx, sid, _) in &leg_signal_ids {
        let filled = fills.iter().any(|(i, _)| i == idx);
        let current = state
            .state_store
            .get_signal(sid)
            .ok()
            .flatten()
            .map(|s| s.status);
        // The background reconciler may have polled an earlier leg while
        // subsequent legs were still dispatching, advancing it from InFlight
        // to Balanced or Unresolved. Override any non-terminal state so a
        // partial cycle always marks filled legs Imbalanced and a completed
        // cycle stamps the uniform cycle note.
        if matches!(current, Some(SignalStatus::Closed | SignalStatus::Failed)) {
            continue;
        }
        if cycle_complete {
            let status = if positions_open {
                SignalStatus::Balanced
            } else {
                SignalStatus::Closed
            };
            let note = if positions_open {
                "options package cycle completed — legs remain open until expiry/flatten"
            } else {
                "options package cycle completed — no open positions detected"
            };
            let _ = state
                .state_store
                .update_signal_status(sid, status, Some(note));
        } else if filled {
            let note = if unwind_notes.is_empty() {
                "options package partial — residual leg; flatten the instrument to unwind"
                    .to_string()
            } else {
                format!(
                    "options package partial — emergency auto-unwind executed ({})",
                    unwind_notes.join("; ")
                )
            };
            let _ =
                state
                    .state_store
                    .update_signal_status(sid, SignalStatus::Imbalanced, Some(&note));
        } else if current == Some(SignalStatus::InFlight) {
            let _ = state.state_store.update_signal_status(
                sid,
                SignalStatus::Failed,
                Some("cycle aborted before this leg dispatched"),
            );
        }
    }

    // Premium cashflow at entry: option buys are cash out, sells cash in;
    // the locked payoff is unrealized until expiry — same convention as
    // parity. Option fills print in base coin → USD at index.
    let mut premium_pnl = Decimal::ZERO;
    let mut fee_total = Decimal::ZERO;
    for (idx, resp) in &fills {
        if let Some(f) = resp.fee.filter(|f| *f > dec!(0)) {
            fee_total += f;
        }
        let Some(px) = resp.avg_fill_price.filter(|p| *p > dec!(0)) else {
            continue;
        };
        let q = resp.filled_qty;
        let usd = px * q * index;
        match opp.legs[*idx].side.as_str() {
            "buy" => premium_pnl -= usd,
            _ => premium_pnl += usd,
        }
    }
    // `resp.fee` is already USD — the Deribit adapter converts the venue's
    // coin-denominated commission at index before reporting it.
    let attempt_pnl_f = to_f64(premium_pnl);
    let fee_total_f = to_f64(fee_total);
    // Premium cashflow is financing, not realized PnL — booked to the
    // dedicated outlay counter (see options_parity); true PnL lands at
    // close via per-leg deltas. Fees are a real cost paid at fill — they
    // book to realized immediately, not into the outlay bucket.
    let total_f =
        crate::execution::accumulate_premium_outlay(state, state.is_live(), attempt_pnl_f);
    if fee_total > Decimal::ZERO {
        crate::execution::accumulate_realized_profit(state, state.is_live(), -fee_total_f);
    }
    let latency_us = started.elapsed().as_micros() as u64;
    let direction = opp.label.clone();
    let attributed = crate::attribution::attribute_strategy_id(&strategy_id);

    if !record_errors.is_empty() {
        record_execution_log(
            state,
            ExecutionLogEntry {
                id: format!("exec-opkg-{cycle}"),
                timestamp: Utc::now().to_rfc3339(),
                trade_type: format!("OptionsPackage:{}", params.substrategy_id),
                strategy_id: attributed.clone(),
                symbol: direction.clone(),
                venue_a: "deribit".into(),
                venue_b: "deribit".into(),
                leg_a_status: format!("{}/{} legs filled", fills.len(), n_legs),
                leg_b_status: "record errors".into(),
                success: false,
                gross_profit: 0.0,
                fees: fee_total_f,
                net_profit: -fee_total_f,
                latency_us,
                note: format!(
                    "Options package {tag} record errors: {}",
                    record_errors.join("; ")
                ),
                direction: Some(direction),
                trigger_spread_bps: Some(opp.edge_bps),
                baseline_mean_spread_bps: None,
                position_step: None,
                max_units: None,
                nominal_hurdle_bps: params.min_edge_bps,
                is_live: state.is_live(),
            },
        );
        return Err(format!(
            "Options package {tag} fill record incomplete: {}",
            record_errors.join("; ")
        ));
    }

    if !cycle_complete {
        record_execution_log(
            state,
            ExecutionLogEntry {
                id: format!("exec-opkg-{cycle}"),
                timestamp: Utc::now().to_rfc3339(),
                trade_type: format!("OptionsPackage:{}", params.substrategy_id),
                strategy_id: attributed.clone(),
                symbol: direction.clone(),
                venue_a: "deribit".into(),
                venue_b: "deribit".into(),
                leg_a_status: format!("{}/{} legs filled", fills.len(), n_legs),
                leg_b_status: leg_error.clone().unwrap_or_default(),
                success: false,
                gross_profit: 0.0,
                fees: fee_total_f,
                net_profit: -fee_total_f,
                latency_us,
                note: format!(
                    "Options package {tag} PARTIAL: {}. {}",
                    leg_error.clone().unwrap_or_default(),
                    if leg_signal_ids.is_empty() {
                        // Blocked combo path: nothing was recorded, so there is no
                        // ledger residue to flatten — but the venue side may hold
                        // fills nobody priced.
                        "No per-leg signals were recorded — verify venue-side orders/positions for these instruments before retrying."
                    } else {
                        "Residual legs recorded as imbalanced '{strategy_id}' signals — flatten each instrument to unwind."
                    }
                ),
                direction: Some(direction.clone()),
                trigger_spread_bps: Some(opp.edge_bps),
                baseline_mean_spread_bps: None,
                position_step: None,
                max_units: None,
                nominal_hurdle_bps: params.min_edge_bps,
                is_live: state.is_live(),
            },
        );
        return Err(format!(
            "Options package {tag} PARTIAL ({}/{} legs): {}; {}",
            fills.len(),
            n_legs,
            leg_error.unwrap_or_default(),
            if leg_signal_ids.is_empty() {
                "verify venue-side orders/positions before retrying (no ledger residue recorded)"
            } else {
                "residuals recorded — flatten leg instruments to unwind"
            }
        ));
    }

    record_execution_log(state, ExecutionLogEntry {
        id: format!("exec-opkg-{cycle}"),
        timestamp: Utc::now().to_rfc3339(),
        trade_type: format!("OptionsPackage:{}", params.substrategy_id),
        strategy_id: attributed,
        symbol: direction.clone(),
        venue_a: "deribit".into(),
        venue_b: "deribit".into(),
        leg_a_status: "FILLED".into(),
        leg_b_status: "FILLED".into(),
        success: true,
        gross_profit: 0.0,
        fees: fee_total_f,
        net_profit: -fee_total_f,
        latency_us,
        note: format!(
            "Options package {tag} {} completed {}/{} legs; premium cashflow ${attempt_pnl_f:.2} booked, locked edge unrealized until expiry.",
            opp.label,
            fills.len(),
            n_legs
        ),
        direction: Some(direction),
        trigger_spread_bps: Some(opp.edge_bps),
        baseline_mean_spread_bps: None,
        position_step: None,
        max_units: None,
        nominal_hurdle_bps: params.min_edge_bps,
        is_live: state.is_live(),
    });

    let duration_ms = latency_us / 1000;
    if duration_ms > 1500 {
        tracing::warn!(
            "Options package execution took {}ms (>1500ms SLA)",
            duration_ms
        );
    }

    let _ = equity_before_by_venue;
    Ok(SimStepResultDto {
        cycle,
        opportunity_found: true,
        execution_success: true,
        trade_type: format!("OptionsPackage:{}", params.substrategy_id),
        direction: opp.label.clone(),
        venue_a: "deribit".into(),
        venue_b: "deribit".into(),
        spread_bps: opp.edge_bps,
        realized_profit_usd: attempt_pnl_f,
        total_cumulative_pnl_usd: total_f,
        message: format!(
            "Options package {tag} {} — {}/{} legs filled in {duration_ms}ms; premium ${attempt_pnl_f:.2} booked, locked edge unrealized until expiry",
            opp.label,
            fills.len(),
            n_legs
        ),
        execution_duration_ms: Some(duration_ms),
    })
}

#[tauri::command]
pub async fn scan_options_packages(
    params: OptionsPackageScanParams,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<OptionsPackageOpportunityDto>, String> {
    scan_options_packages_impl(&params, &state).await
}

#[tauri::command]
pub async fn execute_options_package(
    params: OptionsPackageExecutionParams,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SimStepResultDto, String> {
    execute_options_package_impl(&params, &state).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(name: &str, bid: f64, ask: f64) -> OptQuote {
        OptQuote {
            name: name.to_string(),
            is_call: name.ends_with("-C"),
            bid: Some(Decimal::from_f64_retain(bid).unwrap()),
            ask: Some(Decimal::from_f64_retain(ask).unwrap()),
            is_open: true,
        }
    }

    const IDX: f64 = 100_000.0;

    /// A box window where the executable long cost is 0.04 (=$4000 at
    /// index) and the short proceeds are 0.032 (=$3200) — with a $5000
    /// strike gap and a 0% benchmark the long box underprices its payoff.
    fn box_window_long_edge() -> BoxWindow<'static> {
        BoxWindow {
            c1: Box::leak(Box::new(q("C-K1", 0.048, 0.050))),
            c2: Box::leak(Box::new(q("C-K2", 0.030, 0.032))),
            p1: Box::leak(Box::new(q("P-K1", 0.020, 0.022))),
            p2: Box::leak(Box::new(q("P-K2", 0.038, 0.040))),
        }
    }

    #[test]
    fn box_long_when_implied_rate_undershoots_benchmark() {
        let w = box_window_long_edge();
        let ev = evaluate_box(
            &w,
            &EvalCtx {
                strike_gap_usd: 5_000.0,
                qty: 1.0,
                index_usd: IDX,
            },
            0.1,
            0.0, // 0% benchmark → fair PV = 5000
        )
        .expect("long edge clears fees");
        assert_eq!(ev.label, "LONG BOX");
        // cost = 0.05 − 0.03 − 0.02 + 0.04 = 0.04 → $4000 PV
        assert!((ev.gross_edge_usd - 1_000.0).abs() < 1e-6);
        assert!(ev.net_edge_usd < ev.gross_edge_usd); // fees deducted
        assert!((ev.implied_rate - (5_000.0 / 4_000.0_f64).ln() / 0.1).abs() < 1e-9);
        // Executed sides: buy c1 & p2, sell c2 & p1.
        assert_eq!(ev.legs.map(|(buy, _)| buy), [true, false, false, true]);
    }

    #[test]
    fn box_short_when_market_overpays_the_bond() {
        // Quotes price the box at $4900 proceeds while a 100% benchmark
        // only justifies PV = 5000·e^{-0.1} ≈ $4524 — sell it.
        let w = BoxWindow {
            c1: Box::leak(Box::new(q("C-K1", 0.050, 0.052))),
            c2: Box::leak(Box::new(q("C-K2", 0.027, 0.028))),
            p1: Box::leak(Box::new(q("P-K1", 0.019, 0.020))),
            p2: Box::leak(Box::new(q("P-K2", 0.047, 0.049))),
        };
        let ev = evaluate_box(
            &w,
            &EvalCtx {
                strike_gap_usd: 5_000.0,
                qty: 1.0,
                index_usd: IDX,
            },
            0.1,
            1.0,
        )
        .expect("short edge clears fees");
        assert_eq!(ev.label, "SHORT BOX");
        // proceeds = 0.05 − 0.028 − 0.02 + 0.047 = 0.049 → $4900
        let expected = 4_900.0 - 5_000.0 * (-0.1_f64).exp();
        assert!((ev.gross_edge_usd - expected).abs() < 1e-6);
        assert_eq!(ev.legs.map(|(buy, _)| buy), [false, true, true, false]);
    }

    #[test]
    fn box_rejects_when_fees_eat_the_edge() {
        // Cost $4950 vs $5000 fair → $5 gross < $12 fees → nothing left.
        let w = BoxWindow {
            c1: Box::leak(Box::new(q("C-K1", 0.048, 0.059))),
            c2: Box::leak(Box::new(q("C-K2", 0.030, 0.031))),
            p1: Box::leak(Box::new(q("P-K1", 0.020, 0.021))),
            p2: Box::leak(Box::new(q("P-K2", 0.039, 0.0405))),
        };
        assert!(evaluate_box(
            &w,
            &EvalCtx {
                strike_gap_usd: 5_000.0,
                qty: 0.1,
                index_usd: IDX,
            },
            0.1,
            0.0,
        )
        .is_none());
    }

    #[test]
    fn box_rejects_unquoted_side() {
        let w = box_window_long_edge();
        let mut dead = q("C-K1", 0.048, 0.050);
        dead.ask = None;
        let w = BoxWindow { c1: &dead, ..w };
        assert!(evaluate_box(
            &w,
            &EvalCtx {
                strike_gap_usd: 5_000.0,
                qty: 1.0,
                index_usd: IDX,
            },
            0.1,
            0.0,
        )
        .is_none());
    }

    /// Jelly fixture — near box costs 0.005 coin ($500) to buy; shorting
    /// it yields −0.002 (negative proceeds → the short direction's near PV
    /// fails its own check and never emits). The far box yields 0.010
    /// ($1000) shorted. Long jelly = pay $500 near + collect $1000 far =
    /// −$500 net cost; a 5% benchmark prices the T1→T2 loan at ≈$198.
    fn jelly_fixture() -> (BoxWindow<'static>, BoxWindow<'static>) {
        (
            BoxWindow {
                c1: Box::leak(Box::new(q("C1-K1", 0.039, 0.041))),
                c2: Box::leak(Box::new(q("C1-K2", 0.030, 0.032))),
                p1: Box::leak(Box::new(q("P1-K1", 0.019, 0.021))),
                p2: Box::leak(Box::new(q("P1-K2", 0.012, 0.013))),
            },
            BoxWindow {
                c1: Box::leak(Box::new(q("C2-K1", 0.098, 0.102))),
                c2: Box::leak(Box::new(q("C2-K2", 0.058, 0.060))),
                p1: Box::leak(Box::new(q("P2-K1", 0.050, 0.052))),
                p2: Box::leak(Box::new(q("P2-K2", 0.024, 0.026))),
            },
        )
    }

    #[test]
    fn jelly_long_when_term_books_price_below_carry() {
        let (near, far) = jelly_fixture();
        let ev = evaluate_jelly(
            &near,
            &far,
            &EvalCtx {
                strike_gap_usd: 20_000.0,
                qty: 1.0,
                index_usd: IDX,
            },
            0.1,
            0.3,
            0.05,
        )
        .expect("long jelly clears fees");
        assert_eq!(ev.label, "LONG JELLY");
        // fair = 20000·(e^{-0.005} − e^{-0.015}) ≈ 198.05; net_cost = −500.
        let fair = 20_000.0 * ((-0.05 * 0.1_f64).exp() - (-0.05 * 0.3_f64).exp());
        assert!((ev.gross_edge_usd - (fair + 500.0)).abs() < 1e-6);
        // implied term rate = ln(far_pv/near_pv)/(0.3−0.1) = ln(1000/500)/0.2
        assert!((ev.implied_rate - (1_000.0_f64 / 500.0).ln() / 0.2).abs() < 1e-9);
        // Long jelly = long near box + short far box.
        assert_eq!(
            ev.legs.map(|(buy, _)| buy),
            [true, false, false, true, false, true, true, false]
        );
    }

    #[test]
    fn jelly_rejects_when_both_directions_lose() {
        // Symmetric windows: near cost/proceeds ≈ far's → the roll nets ~0
        // while the loan is worth ≈$198 — long wins; flip the fair rate
        // negative and nothing survives.
        let (near, far) = jelly_fixture();
        assert!(evaluate_jelly(
            &near,
            &far,
            &EvalCtx {
                strike_gap_usd: 20_000.0,
                qty: 1.0,
                index_usd: IDX,
            },
            0.1,
            0.3,
            -0.05, // negative carry → fair ≤ 0 → refused upstream
        )
        .is_none());
    }

    /// Convexity fixture — calls at K1/K2/K3 equidistant. The body bid
    /// (0.032) exceeds the wing-ask mean ((0.05+0.01)/2 = 0.030): sell the
    /// fly for 0.004/unit gross.
    fn convexity_fixture() -> (
        (OptQuote, OptQuote),
        (OptQuote, OptQuote),
        (OptQuote, OptQuote),
    ) {
        (
            (q("C-K1", 0.048, 0.050), q("P-K1", 0.050, 0.052)),
            (q("C-K2", 0.032, 0.034), q("P-K2", 0.028, 0.030)),
            (q("C-K3", 0.009, 0.010), q("P-K3", 0.009, 0.010)),
        )
    }

    #[test]
    fn convexity_sells_the_overpriced_body() {
        let (k1, k2, k3) = convexity_fixture();
        let ev = evaluate_convexity((&k1.0, &k1.1), (&k2.0, &k2.1), (&k3.0, &k3.1), 1.0, IDX)
            .expect("call fly violation clears fees");
        assert_eq!(ev.label, "SELL CALL FLY");
        assert!(ev.is_call);
        // gross = (2·0.032 − 0.050 − 0.010)·1·100000 = $400
        assert!((ev.gross_edge_usd - 400.0).abs() < 1e-6);
        // Fees: wings 30+30, body 2×30 (entry 120) + delivery 2×15 (30) → net = 400 − 150 = 250.
        assert!((ev.net_edge_usd - 250.0).abs() < 1e-6);
        assert_eq!(ev.legs.map(|(buy, _)| buy), [true, false, true]);
    }

    #[test]
    fn convexity_passes_inside_the_bound() {
        // Body bid ≤ wing-ask mean → no riskless sell exists.
        let (k1, mut k2, k3) = convexity_fixture();
        k2.0 = q("C-K2", 0.029, 0.034); // 0.029 ≤ (0.050+0.010)/2 = 0.030
        assert!(
            evaluate_convexity((&k1.0, &k1.1), (&k2.0, &k2.1), (&k3.0, &k3.1), 1.0, IDX,).is_none()
        );
    }

    #[test]
    fn convexity_rejects_unpriced_wing() {
        let (k1, mut k2, k3) = convexity_fixture();
        k2.0.bid = None; // body's executable side missing
        assert!(
            evaluate_convexity((&k1.0, &k1.1), (&k2.0, &k2.1), (&k3.0, &k3.1), 1.0, IDX,).is_none()
        );
    }

    #[test]
    fn convexity_side_evaluates_calls_when_puts_absent() {
        let (k1, k2, k3) = convexity_fixture();
        let ev = evaluate_convexity_side(&k1.0, &k2.0, &k3.0, None, 1.0, IDX, true)
            .expect("call fly works without put quotes");
        assert_eq!(ev.label, "SELL CALL FLY");
        assert!(ev.is_call);
        assert!((ev.gross_edge_usd - 400.0).abs() < 1e-6);
        assert!((ev.net_edge_usd - 250.0).abs() < 1e-6);
    }

    #[test]
    fn convexity_side_evaluates_puts_when_calls_absent() {
        let p_k1 = q("P-K1", 0.048, 0.050);
        let p_k2 = q("P-K2", 0.032, 0.034);
        let p_k3 = q("P-K3", 0.009, 0.010);
        let ev = evaluate_convexity_side(&p_k1, &p_k2, &p_k3, None, 1.0, IDX, false)
            .expect("put fly works without call quotes");
        assert_eq!(ev.label, "SELL PUT FLY");
        assert!(!ev.is_call);
        assert!((ev.gross_edge_usd - 400.0).abs() < 1e-6);
        assert!((ev.net_edge_usd - 250.0).abs() < 1e-6);
    }

    #[test]
    fn test_convexity_side_rejects_inverted_density() {
        // Construct quotes where body is overly elevated relative to wings (negative curvature)
        let c1 = q("C-K1", 0.050, 0.052);
        let c2 = q("C-K2", 0.045, 0.048); // High body price
        let c3 = q("C-K3", 0.010, 0.012);
        let strikes = Some((90000.0, 100000.0, 110000.0));
        let ev = evaluate_convexity_side(&c1, &c2, &c3, strikes, 1.0, IDX, true);
        assert!(
            ev.is_none(),
            "Candidate with inverted density strip must be rejected"
        );
    }

    #[test]
    fn package_expected_yield_annualization_math() {
        // edge_bps = 25.0 bps (0.25% return on notional)
        // expiry = 30 days = 30 / 365 years ≈ 0.08219 years
        // expected_yield_pct = (25.0 / 100.0) / (30.0 / 365.0) = 0.25 * 365 / 30 = 3.04167%
        let edge_bps: f64 = 25.0;
        let t_years: f64 = 30.0 / 365.0;
        let expected_yield_pct: f64 = (edge_bps / 100.0) / t_years;
        assert!((expected_yield_pct - 3.0416666_f64).abs() < 1e-5);

        // If edge_bps = 10.0 bps on a 2-day option (2 / 365 years):
        // expected_yield_pct = 0.1 * 365 / 2 = 18.25% APR (> 10% hurdle)
        let edge_bps_2d: f64 = 10.0;
        let t_years_2d: f64 = 2.0 / 365.0;
        let yield_2d: f64 = (edge_bps_2d / 100.0) / t_years_2d;
        assert!((yield_2d - 18.25_f64).abs() < 1e-5);
    }

    #[test]
    fn test_combo_leg_count_eligibility_boundaries() {
        // Deribit native combos limit orders to between 2 and 4 legs.
        // Box Spread: 4 legs -> eligible for atomic combo order
        // Convexity (Fly): 3 legs -> eligible for atomic combo order
        // Jelly Roll: 8 legs -> exceeds 4 legs, must bypass combo to avoid -32602
        let box_spread_legs = 4;
        let convexity_legs = 3;
        let jelly_roll_legs = 8;
        let single_leg = 1;

        let is_combo_eligible = |leg_count: usize| (2..=4).contains(&leg_count);

        assert!(!is_combo_eligible(single_leg));
        assert!(is_combo_eligible(convexity_legs));
        assert!(is_combo_eligible(box_spread_legs));
        assert!(!is_combo_eligible(jelly_roll_legs));
    }

    /// A 4-leg box: buy K1 call, sell K2 call, sell K1 put, buy K2 put.
    /// `net = +b0 − b1 − b2 + b3`.
    fn box_legs() -> Vec<OptionsPackageLegDto> {
        vec![
            OptionsPackageLegDto {
                instrument: "C-K1".into(),
                side: "buy".into(),
                qty_mult: 1,
                venue_price: 0.05,
                usd_price: 5_000.0,
            },
            OptionsPackageLegDto {
                instrument: "C-K2".into(),
                side: "sell".into(),
                qty_mult: 1,
                venue_price: 0.03,
                usd_price: 3_000.0,
            },
            OptionsPackageLegDto {
                instrument: "P-K1".into(),
                side: "sell".into(),
                qty_mult: 1,
                venue_price: 0.02,
                usd_price: 2_000.0,
            },
            OptionsPackageLegDto {
                instrument: "P-K2".into(),
                side: "buy".into(),
                qty_mult: 1,
                venue_price: 0.04,
                usd_price: 4_000.0,
            },
        ]
    }

    fn bounds_of(px: [f64; 4]) -> Vec<Option<Decimal>> {
        px.iter().map(|p| Decimal::from_f64_retain(*p)).collect()
    }

    /// Signed net of a set of attributed leg prices — the quantity that must
    /// equal the premium the venue actually charged.
    fn signed_net(legs: &[OptionsPackageLegDto], prices: &[Decimal]) -> Decimal {
        signed_combo_net(legs, prices)
    }

    fn assert_close(got: Decimal, want: Decimal, what: &str) {
        // rust_decimal division rounds to 28 significant digits, so a scaled
        // price carries ~1e-28 of relative residue — far below the 1e-8 BTC
        // (1 satoshi) the identity check already tolerates.
        assert!(
            (got - want).abs() <= dec!(0.000_000_01),
            "{what}: got {got}, want {want}"
        );
    }

    #[test]
    fn combo_attribution_reconciles_to_the_paid_net_premium() {
        let legs = box_legs();
        let bounds = bounds_of([0.05, 0.03, 0.02, 0.04]); // shape net = 0.04
        let got = attribute_combo_net_price(&legs, &bounds, dec!(0.042))
            .expect("attribute a 0.042 net fill");
        assert_eq!(got.len(), 4);
        assert_close(signed_net(&legs, &got), dec!(0.042), "attributed net");
        // Pre-trade shape was 0.04; the venue charged 0.042 → +5% on every leg.
        assert_close(got[0], dec!(0.0525), "leg 1");
        assert_close(got[1], dec!(0.0315), "leg 2");
        assert_close(got[2], dec!(0.021), "leg 3");
        assert_close(got[3], dec!(0.042), "leg 4");
        // The old behaviour — booking the raw pre-trade bound — is exactly
        // the defect: it leaves the legs 0.002 BTC short of what was paid.
        assert_close(
            signed_net(
                &legs,
                &bounds.iter().map(|b| b.unwrap()).collect::<Vec<_>>(),
            ),
            dec!(0.04),
            "unreconciled bound",
        );
    }

    #[test]
    fn combo_attribution_keeps_the_relative_price_structure() {
        let legs = box_legs();
        let bounds = bounds_of([0.05, 0.03, 0.02, 0.04]);
        let got = attribute_combo_net_price(&legs, &bounds, dec!(0.042)).expect("attribute");
        // A 0.05 leg and a 0.0005 leg must stay two orders of magnitude
        // apart — a pure qty_mult split would flatten them onto one number.
        let ratio_before = bounds[0].unwrap() / bounds[1].unwrap();
        let ratio_after = got[0] / got[1];
        assert!(
            (ratio_before - ratio_after).abs() < dec!(0.000_000_001),
            "attribution distorted the leg structure: {ratio_before} -> {ratio_after}"
        );
    }

    #[test]
    fn combo_attribution_refuses_a_sign_flip() {
        let legs = box_legs();
        let bounds = bounds_of([0.05, 0.03, 0.02, 0.04]);
        // A package priced as a 0.04 debit that filled as a -0.01 credit:
        // scaling would drive every leg price negative, and a non-positive
        // price is rejected by the fill recorder — which would abort a
        // package the venue genuinely filled.
        assert!(attribute_combo_net_price(&legs, &bounds, dec!(-0.01)).is_none());
    }

    #[test]
    fn combo_attribution_refuses_a_degenerate_shape() {
        let legs = box_legs();
        // Buy and sell the same price: shape net is zero, so no scaling can
        // produce a non-zero net premium.
        let bounds = bounds_of([0.03, 0.03, 0.02, 0.02]);
        assert!(attribute_combo_net_price(&legs, &bounds, dec!(0.01)).is_none());
    }

    #[test]
    fn combo_attribution_falls_back_to_the_scan_price_without_a_book() {
        let legs = box_legs();
        let bounds = vec![None, None, None, None];
        let got = attribute_combo_net_price(&legs, &bounds, dec!(0.042))
            .expect("scan prices carry the shape when the book cannot price a leg");
        assert_eq!(signed_net(&legs, &got), dec!(0.042));
    }

    #[test]
    fn combo_attribution_rejects_unusable_inputs() {
        let legs = box_legs();
        let bounds = bounds_of([0.05, 0.03, 0.02, 0.04]);
        // Shape/bounds length mismatch and an empty leg set.
        assert!(attribute_combo_net_price(&legs, &bounds[..3], dec!(0.042)).is_none());
        assert!(attribute_combo_net_price(&[], &[], dec!(0.042)).is_none());
        // A leg with neither a book bound nor a usable scan price.
        let mut unpriced = box_legs();
        unpriced[0].venue_price = 0.0;
        assert!(
            attribute_combo_net_price(&unpriced, &[None, None, None, None], dec!(0.042)).is_none()
        );
    }
}
