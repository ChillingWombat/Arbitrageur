//! Triangular (intra-venue three-leg) arbitrage: opportunity scan, book
//! evaluation, and execution. Split out of `commands.rs`; shared execution
//! helpers stay reachable via `crate::commands`.

use crate::commands::{
    dec_to_f64, effective_venue_fee_bps, live_entry_gate, margin_monitor_from_settings,
    maybe_record_margin_snapshots, record_execution_failure, record_execution_log,
    refresh_equity_expectations_after_fills, resolve_effective_leverage, validate_adapter_book,
    SimStepResultDto, TRIANGLE_STRATEGY_ID,
};
use crate::state::{AppEngineState, ExecutionLogEntry};
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::quote::OrderBook;
use arbitrageur_core::types::{dec, Decimal, Side, VenueId};
use arbitrageur_math::triangular::{evaluate_triangular_cycle, TriLegSide};
use arbitrageur_risk::margin_monitor::MarginHealthStatus;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::State;

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
    /// Per-size capacity curve from the depth-aware cycle evaluation.
    /// `net` fields only — atomic cycles carry no win-rate model, so
    /// expected fields are `None` by construction.
    #[serde(default)]
    pub depth_curve: Vec<crate::monitor::DepthCurvePointDto>,
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
    /// Autonomous bot dispatch — autonomous entries stay testnet-only; live
    /// entries must come from the operator-directed Execute path.
    #[serde(default)]
    pub auto: bool,
}

/// Paper-dust tolerance for the triangle entry guard: f64/Decimal ledger
/// conversions can strand sub-1e-9 base-unit residue after a flatten.
/// Economically meaningless on every listed venue, so it must not block
/// re-entry forever; anything at or above it is a real position.
pub(crate) fn triangle_dust_qty() -> Decimal {
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
pub(crate) fn sweep_flatten_qty(residual: Decimal, capped: Decimal, step: Decimal) -> Decimal {
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
pub(crate) fn flatten_step_qty(state: &AppEngineState, symbol: &str) -> Decimal {
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
        leg_sides: [
            TriLegSide::BuyBase,
            TriLegSide::SellBase,
            TriLegSide::SellBase,
        ],
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
pub(crate) async fn resolve_triangle_books(
    state: &AppEngineState,
    adapter: &Arc<dyn ExchangeAdapter>,
    vid: &VenueId,
    symbols: &[String; 3],
    rest_fallback: bool,
) -> Option<[OrderBook; 3]> {
    let mut out: Vec<OrderBook> = Vec::with_capacity(3);
    for sym in symbols {
        // Hub keys normalize spellings, so a WS-fed bare-base book hits a
        // canonical-symbol lookup. Scans pass rest_fallback = false — the
        // REST fallback exists for dispatch-time resolution only; probing
        // every candidate pair over REST would turn each scan into a
        // per-venue request storm.
        let book = match state.market_data_hub.get_valid_orderbook(
            vid,
            sym,
            Some(std::time::Duration::from_secs(2)),
        ) {
            Some(b) => b,
            None if rest_fallback => {
                let b = adapter.fetch_orderbook(sym).await.ok()?;
                validate_adapter_book(&b, vid, sym).ok()?;
                state.market_data_hub.update_orderbook(b.clone());
                b
            }
            None => return None,
        };
        if validate_adapter_book(&book, vid, sym).is_err() {
            return None;
        }
        out.push(book);
    }
    Some([out.remove(0), out.remove(0), out.remove(0)])
}

/// Capacity curve for one triangle cycle: evaluate the atomic cycle at a
/// geometric notional ladder (leg-1 is always `BuyBase`, so its ask-side
/// notional caps the start size). Each rung's cumulative net bps comes
/// from the evaluator directly; marginal bps are finite differences
/// between consecutive rungs (first rung uses its own cumulative rate).
/// Expected fields stay `None` — atomic cycles have no win-rate model.
/// Stops at the first unfillable size (depth is monotonic).
fn triangle_depth_curve(
    books: &[OrderBook; 3],
    sides: [TriLegSide; 3],
    fee_bps: f64,
    ref_size_usd: f64,
) -> Vec<crate::monitor::DepthCurvePointDto> {
    const N: usize = 24;
    let Some(fee) = Decimal::from_f64_retain(fee_bps).filter(|f| *f >= dec!(0)) else {
        return Vec::new();
    };
    // Start-notional ceiling: leg-1 lifts asks, so the spend cannot exceed
    // that side's visible quote notional.
    let depth_cap: f64 = books[0]
        .asks
        .iter()
        .filter_map(|l| {
            let p = l.price.to_string().parse::<f64>().ok()?;
            let q = l.size.to_string().parse::<f64>().ok()?;
            (p.is_finite() && p > 0.0 && q.is_finite() && q > 0.0).then_some(p * q)
        })
        .sum();
    let lo = (ref_size_usd / 32.0).max(25.0).min(depth_cap);
    let hi = (ref_size_usd * 64.0).min(depth_cap);
    if !(lo.is_finite() && hi.is_finite() && hi > lo && lo > 0.0) {
        return Vec::new();
    }
    let ratio = (hi / lo).powf(1.0 / (N - 1) as f64);
    let mut sizes: Vec<f64> = (0..N).map(|i| lo * ratio.powi(i as i32)).collect();
    // Always include the configured size so the curve is exact there.
    if ref_size_usd.is_finite() && ref_size_usd > lo && ref_size_usd < hi {
        sizes.push(ref_size_usd);
        sizes.sort_by(f64::total_cmp);
    }
    let mut points = Vec::with_capacity(N);
    let mut prev: Option<(f64, f64)> = None; // (size, profit_usd)
    for s in sizes {
        let Some(n) = Decimal::from_f64_retain(s).filter(|v| *v > dec!(0)) else {
            continue;
        };
        let Some(res) =
            evaluate_triangular_cycle([&books[0], &books[1], &books[2]], sides, [fee, fee, fee], n)
        else {
            break; // depth exhausted — deeper sizes cannot fill either
        };
        let profit = res
            .estimated_profit
            .to_string()
            .parse::<f64>()
            .ok()
            .filter(|p| p.is_finite());
        let net_bps = res
            .net_bps
            .to_string()
            .parse::<f64>()
            .ok()
            .filter(|p| p.is_finite());
        let (Some(profit), Some(cum_net)) = (profit, net_bps) else {
            continue;
        };
        let marg = prev
            .map(|(ps, pp)| (profit - pp) / (s - ps) * 10_000.0)
            .filter(|m| m.is_finite())
            .unwrap_or(cum_net);
        points.push(crate::monitor::DepthCurvePointDto {
            cum_usd: s,
            cum_net_bps: cum_net,
            cum_exp_bps: None,
            marg_net_bps: marg,
            marg_exp_bps: None,
        });
        prev = Some((s, profit));
    }
    points
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
                let books = match resolve_triangle_books(
                    state,
                    adapter,
                    &venue_id,
                    &plan.leg_symbols,
                    false,
                )
                .await
                {
                    Some(b) => b,
                    None => continue,
                };
                let eval = match evaluate_triangle_books(&books, plan.leg_sides, fee, trade_size) {
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
                    depth_curve: triangle_depth_curve(&books, plan.leg_sides, fee, trade_size),
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
    expected_net_usd: Option<f64>,
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
        .default_account(venue_id, state.env().as_str())
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
            expected_net_usd,
            note: Some(note.to_string()),
            legs: vec![NewSignalLeg {
                account_id,
                venue: venue_id.to_string(),
                side,
                symbol: None,
                intended_qty: qty_f64,
                intended_price: None,
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
    // Autonomous entries stay testnet-only in live mode (same policy as
    // the radar bot's evaluate_entry_gates hard-block and the funding
    // auto flag); operator-directed manual entries are the live path.
    if params.auto && state.is_live() {
        return Err(
            "autonomous triangle entries are testnet-only — run a manual Execute for live".into(),
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

    let adapter = state
        .get_adapter(&params.venue)
        .ok_or_else(|| format!("Venue '{}' is not admitted for execution", params.venue))?;
    let vid = adapter.venue_id();

    // The caller-supplied spread is never trusted: re-scan and match the
    // opportunity id against freshly resolved books.
    let opps = scan_triangular_opportunities_impl("", state).await?;
    let opp = opps
        .into_iter()
        .find(|o| o.id == params.id)
        .ok_or_else(|| {
            "triangular opportunity is no longer available (missing legs or stale books)"
                .to_string()
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
    if state.is_live() {
        // Triangle legs dispatch sequentially on one venue with durable
        // leg records; a partial cycle lands `imbalanced` and the
        // reconciler auto-flattens the residual reduce-only. Same live
        // gate as spatial/funding: unlocked vault + adapter
        // submission/polling/entry eligibility + credentials.
        live_entry_gate(state, &[vid.as_str()])?;
    }
    let books = resolve_triangle_books(state, &adapter, &vid, &plan.leg_symbols, true)
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
    // signal or any open position on a leg symbol refuses dispatch.
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
    let pre_acc = adapter
        .fetch_account()
        .await
        .map_err(|e| format!("account read on {} failed: {e}", params.venue))?;
    let dust = triangle_dust_qty();
    for sym in &plan.leg_symbols {
        if adapter
            .account_position_for(&pre_acc, sym)
            .ok()
            .flatten()
            .is_some_and(|p| p.quantity > dust)
        {
            return Err(format!(
                "An open position already exists for {sym}; flatten it before cycling"
            ));
        }
    }

    let (target_leverage, _account_cap) = resolve_effective_leverage(
        state,
        "futures",
        "futures",
        &plan.leg_symbols[0],
        &params.venue,
        &params.venue,
        None,
    )?;
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
                "Account {} has no funds; execution refused",
                params.venue
            ));
        }
        MarginHealthStatus::Critical | MarginHealthStatus::Breached => {
            return Err(format!(
                "Account {} margin status is critical/breached; execution refused",
                params.venue
            ));
        }
        _ => {}
    }

    // Pre-trade equity snapshot of the traded venue (env-namespaced key) —
    // the baseline the post-fill attribution refresh measures against.
    let mut equity_before_by_venue = HashMap::new();
    let env_str = state.env().as_str();
    if let Ok(f) = pre_acc.equity.to_string().parse::<f64>() {
        if f.is_finite() {
            equity_before_by_venue.insert(format!("{env_str}-{vid}"), f);
        }
    }

    let cycle = state.next_sim_step();
    let cycle_tag = format!(
        "tri-{}-{:08x}",
        Utc::now().timestamp_millis(),
        rand::random::<u32>()
    );
    let started = std::time::Instant::now();

    let mut fills: Vec<(usize, arbitrageur_core::order::OrderResponse)> = Vec::new();
    let mut submitted_qty: Vec<f64> = Vec::new();
    let mut leg_error: Option<String> = None;
    for (idx, ((sym, order_side), qty_f)) in plan
        .leg_symbols
        .iter()
        .zip(plan.order_sides.iter())
        .zip(eval.order_qty.iter())
        .enumerate()
    {
        // Codebase convention (spatial path + ledger): f64 quantities cross
        // into Decimal through their shortest string rendering, so the venue
        // position and the ledger residual stay bit-identical and flatten
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
        // Snap each leg onto the venue's wire grid before dispatch: a fill
        // floored by the adapter would otherwise read as a partial fill and
        // flatten the whole cycle on a representability artifact.
        let qty = match adapter.wire_base_qty(sym, qty).await {
            Ok(w) if w > dec!(0) => w,
            Ok(_) => {
                leg_error = Some(format!(
                    "leg {} on {sym} snaps to zero on the venue wire grid",
                    idx + 1
                ));
                break;
            }
            Err(e) => {
                leg_error = Some(format!(
                    "leg {} on {sym} not representable on the venue wire grid: {e}",
                    idx + 1
                ));
                break;
            }
        };
        submitted_qty.push(qty.to_string().parse::<f64>().unwrap_or(0.0));
        let order = arbitrageur_core::order::OrderRequest::new_market(
            adapter.venue_id(),
            sym,
            *order_side,
            qty,
        );
        match adapter.place_order(order).await {
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

    refresh_equity_expectations_after_fills(state, &equity_before_by_venue).await;
    // Realized cash delta of the fills: buys spend, sells receive, fees
    // subtract. For a balanced triangle cycle this is the negative
    // spread+fee cost; the opened position's value is unrealized on venue.
    let attempt_pnl: Decimal = fills
        .iter()
        .map(|(idx, r)| {
            let px = r.avg_fill_price.unwrap_or(dec!(0));
            let signed = match plan.order_sides[*idx] {
                Side::Sell => r.filled_qty * px,
                Side::Buy => -r.filled_qty * px,
            };
            signed - r.fee
        })
        .sum();
    let attempt_pnl_f = attempt_pnl.to_string().parse::<f64>().unwrap_or(0.0);
    let total_f =
        crate::execution::accumulate_realized_profit(state, state.is_live(), attempt_pnl_f);

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
            submitted_qty[*idx]
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
            // The cycle-level projection persists once — on leg 0 — so the
            // family carries the eval's expectation without triple-counting.
            (*idx == 0).then_some(eval.profit_usd),
        ) {
            record_errors.push(e);
        }
    }

    if let Some(err) = leg_error {
        record_execution_log(state, ExecutionLogEntry {
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
            is_live: state.is_live(),
        });
        return Err(format!(
            "Triangle cycle PARTIAL ({}/3 legs): {err}; residuals recorded — flatten leg symbols to unwind",
            fills.len()
        ));
    }

    record_execution_log(state, ExecutionLogEntry {
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
            "Triangle cycle dispatched 3/3 legs ({cycle_note}); spread position open — unwind each leg symbol via flatten. Record errors: {}",
            record_errors.join("; ")
        ),
        direction: Some(plan.path.clone()),
        trigger_spread_bps: Some(eval.net_bps),
        baseline_mean_spread_bps: None,
        position_step: None,
        max_units: None,
        nominal_hurdle_bps: Some(params.min_profit_bps),
        is_live: state.is_live(),
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
            "Triangle cycle dispatched on {}: 3/3 legs filled, realized P&L ${attempt_pnl_f:.2}; spread position open — flatten leg symbols to unwind",
            params.venue
        ),
    })
}

/// Venue orderbook pair (bids, asks) keyed by venue id.

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
    match execute_triangle_arbitrage_impl(params.clone(), &state).await {
        Ok(r) => Ok(r),
        Err(e) => {
            record_execution_failure(
                &state,
                "Triangle",
                TRIANGLE_STRATEGY_ID,
                &params.path,
                &params.venue,
                &params.venue,
                &e,
            );
            Err(e)
        }
    }
}
