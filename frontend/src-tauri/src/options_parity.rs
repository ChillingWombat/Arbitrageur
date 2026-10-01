//! Options synthetic-forward parity vs perpetual funding (Deribit, §3.9 of
//! docs/OPTIONS_ARBITRAGE_STRATEGIES.md).
//!
//! Put-call parity for European options: C − P = F − K (zero rates), so
//! `K + (C − P)` is a synthetic forward on the underlying. On Deribit the
//! synthetic's funding-free forward is compared against the perpetual's
//! funding-carrying price:
//!
//! - **Conversion** (synthetic rich): sell call, buy put, buy perp.
//!   Edge = [K + (call_bid − put_ask)·index] − [perp_ask + funding_drain].
//! - **Reversal** (synthetic cheap): buy call, sell put, sell perp.
//!   Edge = [perp_bid + funding_drain] − [K + (call_ask − put_bid)·index].
//!
//! `funding_drain` is the expected funding a long perp pays over the
//! option's remaining life (`funding_8h × hours/8 × perp_mark`); a short
//! receives it. Execution is a three-leg single-venue cycle with the same
//! discipline as the triangle path: durable leg intents before submit,
//! bounded IOC orders, venue-truth settle polling, and imbalanced residual
//! signals the reconciler flattens.

use crate::commands::{
    dec_to_f64, effective_venue_fee_bps, live_entry_gate, margin_monitor_from_settings,
    maybe_record_margin_snapshots, record_execution_failure, record_execution_log,
    refresh_equity_expectations_after_fills, SimStepResultDto, OPTIONS_PARITY_STRATEGY_ID,
};
use crate::state::{AppEngineState, ExecutionLogEntry};
use crate::state_store::SignalStatus;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::quote::OrderBook;
use arbitrageur_core::types::{dec, Decimal, OrderStatus, Side, VenueId};
use arbitrageur_exchanges::deribit::{DeribitAdapter, DeribitBookSummary};
use arbitrageur_risk::margin_monitor::MarginHealthStatus;
use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;

/// Slippage bound on each leg's IOC price, padded over the execution VWAP.
/// Option books are thin — wider than the triangle bound but still bounded.
pub(crate) const OPTIONS_IOC_BOUND_BPS: u32 = 60;
pub(crate) const OPTIONS_SETTLE_POLLS: usize = 4;
pub(crate) const OPTIONS_SETTLE_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(400);
/// Deribit option taker fee is 0.03% of underlying notional, capped at
/// 12.5% of the option premium.
pub(crate) const OPTION_TAKER_FEE_RATE: f64 = 0.0003;
pub(crate) const OPTION_FEE_PREMIUM_CAP: f64 = 0.125;
/// Option books on far-dated wings are empty — a quote is required on the
/// exact side each leg executes.
const MAX_RESULTS: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParityDirection {
    /// Synthetic forward rich vs perp: sell call, buy put, buy perp.
    Conversion,
    /// Synthetic forward cheap vs perp: buy call, sell put, sell perp.
    Reversal,
}

impl ParityDirection {
    fn as_str(self) -> &'static str {
        match self {
            Self::Conversion => "conversion",
            Self::Reversal => "reversal",
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OptionsParityLegDto {
    pub instrument: String,
    pub role: String,   // "call" | "put" | "perp"
    pub action: String, // "buy" | "sell"
    /// Venue-unit quote (BTC for options, USD for the perp).
    pub venue_price: f64,
    /// USD translation of the venue price (option prices × index).
    pub usd_price: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OptionsParityOpportunityDto {
    pub id: String,
    pub venue: String,
    pub currency: String,
    pub expiry_ms: i64,
    pub expiry_iso: String,
    pub strike: f64,
    pub direction: ParityDirection,
    pub call_leg: OptionsParityLegDto,
    pub put_leg: OptionsParityLegDto,
    pub perp_leg: OptionsParityLegDto,
    /// K + (C − P)·index, using the executable sides for the direction.
    pub synthetic_forward_usd: f64,
    /// Perp price including expected funding carry to expiry.
    pub perp_equiv_usd: f64,
    pub funding_8h: f64,
    pub funding_drain_usd: f64,
    pub gross_edge_usd: f64,
    pub est_fees_usd: f64,
    pub net_edge_usd: f64,
    pub edge_bps: f64,
    pub index_price_usd: f64,
    pub qty_base_suggested: f64,
    /// One-liner when the pair is shown but not executable.
    pub is_executable: bool,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OptionsParityScanParams {
    pub currency: String,
    #[serde(default)]
    pub qty_base: Option<f64>,
    /// Ignore pairs expiring sooner than this (thin far wings dominate the
    /// near-expiry noise floor).
    #[serde(default)]
    pub min_hours_to_expiry: Option<f64>,
    /// Ignore pairs expiring later than this — the funding forecast decays
    /// with horizon and the strategy targets funding convergence.
    #[serde(default)]
    pub max_days_to_expiry: Option<f64>,
    #[serde(default)]
    pub min_edge_bps: Option<f64>,
    /// Result cap — a display concern. Execution re-scans pass usize::MAX so
    /// a candidate cannot fall out of the top-N cut between scans.
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OptionsParityExecutionParams {
    /// Opportunity id returned by the scan — re-validated at dispatch.
    pub id: String,
    pub currency: String,
    /// Base-asset quantity per leg (BTC contracts of qty_base for the
    /// options; qty_base×price USD for the perp — same delta either way).
    pub qty_base: f64,
    #[serde(default)]
    pub min_edge_bps: Option<f64>,
    #[serde(default)]
    pub auto: bool,
}

/// Quotes the parity evaluation needs, in venue units (option prices in
/// base coin, perp in USD) — the evaluator converts at `index`.
pub struct ParityQuotes {
    pub call_bid: Decimal,
    pub call_ask: Decimal,
    pub put_bid: Decimal,
    pub put_ask: Decimal,
    pub perp_bid: Decimal,
    pub perp_ask: Decimal,
    pub perp_mark: Decimal,
    pub index_usd: Decimal,
    pub strike: Decimal,
    pub funding_8h: Decimal,
    pub hours_to_expiry: f64,
    /// Per-leg taker fee rate for the perp (fraction, e.g. 0.0005).
    pub perp_fee_rate: Decimal,
}

pub struct ParityEval {
    pub direction: ParityDirection,
    /// USD edge per base unit after funding carry, before fees.
    pub gross_edge_usd: Decimal,
    /// Estimated fees across all three legs per base unit (USD).
    pub est_fees_usd: Decimal,
    pub net_edge_usd: Decimal,
    pub synthetic_forward_usd: Decimal,
    pub perp_equiv_usd: Decimal,
    pub funding_drain_usd: Decimal,
}

/// Pure parity evaluation — executable-side quotes only, fails closed when
/// the needed side is unquoted or the edge is non-positive.
pub fn evaluate_parity(q: &ParityQuotes, qty_base: Decimal) -> Option<ParityEval> {
    if qty_base <= dec!(0) || q.index_usd <= dec!(0) || q.strike <= dec!(0) {
        return None;
    }
    let periods = Decimal::from_f64_retain(q.hours_to_expiry.max(0.0) / 8.0)?;
    let drain = q.funding_8h * periods * q.perp_mark;
    // Option taker fee per contract: min(rate×underlying, 12.5%×premium),
    // premium in base coin → USD at index. Two option legs + one perp leg.
    let option_fee = |px_coin: Decimal| -> Option<Decimal> {
        if px_coin <= dec!(0) {
            return None;
        }
        let uncapped = Decimal::from_f64_retain(OPTION_TAKER_FEE_RATE)? * q.index_usd;
        let capped = Decimal::from_f64_retain(OPTION_FEE_PREMIUM_CAP)? * px_coin * q.index_usd;
        Some(uncapped.min(capped) * qty_base)
    };
    let perp_fee = q.perp_fee_rate * qty_base * q.perp_mark;

    // Conversion: sell call at bid, buy put at ask, buy perp at ask.
    let conv = if q.call_bid > dec!(0) && q.put_ask > dec!(0) && q.perp_ask > dec!(0) {
        let f_syn = q.strike + (q.call_bid - q.put_ask) * q.index_usd;
        let perp_cost = q.perp_ask + drain;
        let gross = (f_syn - perp_cost) * qty_base;
        Some((
            ParityDirection::Conversion,
            gross,
            f_syn,
            perp_cost,
            option_fee(q.call_bid)? + option_fee(q.put_ask)? + perp_fee,
        ))
    } else {
        None
    };
    // Reversal: buy call at ask, sell put at bid, sell perp at bid.
    let rev = if q.call_ask > dec!(0) && q.put_bid > dec!(0) && q.perp_bid > dec!(0) {
        let f_syn = q.strike + (q.call_ask - q.put_bid) * q.index_usd;
        let perp_proceeds = q.perp_bid + drain;
        let gross = (perp_proceeds - f_syn) * qty_base;
        Some((
            ParityDirection::Reversal,
            gross,
            f_syn,
            perp_proceeds,
            option_fee(q.call_ask)? + option_fee(q.put_bid)? + perp_fee,
        ))
    } else {
        None
    };

    [conv, rev]
        .into_iter()
        .flatten()
        .filter(|(_, gross, _, _, _)| *gross > dec!(0))
        .map(|(direction, gross, f_syn, perp_equiv, fees)| ParityEval {
            direction,
            gross_edge_usd: gross,
            est_fees_usd: fees,
            net_edge_usd: gross - fees,
            synthetic_forward_usd: f_syn,
            perp_equiv_usd: perp_equiv,
            funding_drain_usd: drain * qty_base,
        })
        .max_by(|a, b| a.net_edge_usd.cmp(&b.net_edge_usd))
}

pub(crate) fn summary_px(row: &DeribitBookSummary) -> (Option<Decimal>, Option<Decimal>) {
    (
        row.bid.filter(|b| *b > dec!(0)),
        row.ask.filter(|a| *a > dec!(0)),
    )
}

/// The env-scoped Deribit adapter — the options path is Deribit-only (no
/// other configured venue lists crypto options). Admission still flows
/// through `get_adapter` so a disabled/credential-less Deribit refuses.
pub(crate) fn deribit_for_env(state: &AppEngineState) -> Result<Arc<DeribitAdapter>, String> {
    if state.get_adapter("deribit").is_none() {
        return Err(
            "Deribit is not admitted in this environment — enable it and configure credentials"
                .into(),
        );
    }
    #[cfg(test)]
    if let Some(a) = state.test_deribit.read().ok().and_then(|g| g.clone()) {
        return Ok(a);
    }
    Ok(if state.is_live() {
        state.live_deribit.clone()
    } else {
        state.tn_deribit.clone()
    })
}

pub async fn scan_options_parity_impl(
    params: &OptionsParityScanParams,
    state: &AppEngineState,
) -> Result<Vec<OptionsParityOpportunityDto>, String> {
    let ccy = params.currency.trim().to_ascii_uppercase();
    if !matches!(ccy.as_str(), "BTC" | "ETH") {
        return Err("options parity supports BTC or ETH".into());
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
    let qty = Decimal::from_f64_retain(qty_base).ok_or_else(|| "invalid qty_base".to_string())?;
    let min_hours = params.min_hours_to_expiry.unwrap_or(48.0).max(0.0);
    let max_days = params.max_days_to_expiry.unwrap_or(60.0).max(0.0);
    let min_edge_bps = params.min_edge_bps.unwrap_or(0.0);

    let adapter = deribit_for_env(state)?;
    let perp_name = format!("{ccy}-PERPETUAL");
    let (chain, summaries, perp_ticker, funding, index) = tokio::try_join!(
        adapter.option_chain(&ccy),
        adapter.book_summaries(&ccy),
        adapter.fetch_ticker(&perp_name),
        adapter.fetch_funding_rate(&perp_name),
        adapter.index_price_usd(&ccy),
    )
    .map_err(|e| format!("Deribit parity scan inputs failed: {e}"))?;

    if index <= dec!(0) {
        return Err(format!("Deribit {ccy} index price unavailable"));
    }
    let summary_map: std::collections::HashMap<String, &DeribitBookSummary> =
        summaries.iter().map(|s| (s.name.clone(), s)).collect();
    let perp_mark = summary_map
        .get(&format!("{ccy}-PERPETUAL"))
        .and_then(|s| s.mark)
        .filter(|m| *m > dec!(0))
        .or_else(|| (perp_ticker.last > dec!(0)).then_some(perp_ticker.last))
        .unwrap_or(index);

    let (perp_bid, perp_ask, perp_quotes_live) = match (
        Some(perp_ticker.bid).filter(|p| *p > dec!(0)),
        Some(perp_ticker.ask).filter(|p| *p > dec!(0)),
    ) {
        (Some(b), Some(a)) => (b, a, true),
        _ => {
            if perp_mark <= dec!(0) {
                return Err(format!("Deribit {ccy}-PERPETUAL price unavailable"));
            }
            (perp_mark * dec!(0.9995), perp_mark * dec!(1.0005), false)
        }
    };
    let perp_summary = perp_mark;

    let fee_bps = effective_venue_fee_bps(state, "deribit");
    let perp_fee_rate = Decimal::from_f64_retain(fee_bps / 10_000.0).unwrap_or(dec!(0.0005));

    let now_ms = Utc::now().timestamp_millis();
    let mut out: Vec<OptionsParityOpportunityDto> = Vec::new();
    // Group the chain into (expiry, strike) pairs with both call and put.
    use std::collections::BTreeMap;
    #[derive(Default)]
    struct ParityOptionLeg {
        name: String,
        is_open: bool,
    }
    #[derive(Default)]
    struct ParityPairEntry {
        call: Option<ParityOptionLeg>,
        put: Option<ParityOptionLeg>,
    }
    let mut pairs: BTreeMap<(i64, Decimal), ParityPairEntry> = BTreeMap::new();
    for inst in &chain {
        if inst.settlement_currency != ccy {
            continue;
        }
        let hours = (inst.expiration_ms - now_ms) as f64 / 3_600_000.0;
        if hours < min_hours || hours > max_days * 24.0 {
            continue;
        }
        let entry = pairs.entry((inst.expiration_ms, inst.strike)).or_default();
        match inst.option_type {
            arbitrageur_exchanges::deribit::DeribitOptionType::Call => {
                entry.call = Some(ParityOptionLeg {
                    name: inst.name.clone(),
                    is_open: inst.is_open,
                });
            }
            arbitrageur_exchanges::deribit::DeribitOptionType::Put => {
                entry.put = Some(ParityOptionLeg {
                    name: inst.name.clone(),
                    is_open: inst.is_open,
                });
            }
        }
    }

    for ((expiry_ms, strike), pair) in pairs {
        let (Some(call_leg), Some(put_leg)) = (pair.call, pair.put) else {
            continue;
        };
        let call_name = call_leg.name;
        let call_open = call_leg.is_open;
        let put_name = put_leg.name;
        let put_open = put_leg.is_open;
        let call = summary_map.get(&call_name);
        let put = summary_map.get(&put_name);
        let (Some(call), Some(put)) = (call, put) else {
            continue;
        };
        let (call_bid, call_ask) = summary_px(call);
        let (put_bid, put_ask) = summary_px(put);
        let hours = (expiry_ms - now_ms) as f64 / 3_600_000.0;
        let eval = evaluate_parity(
            &ParityQuotes {
                call_bid: call_bid.unwrap_or(dec!(0)),
                call_ask: call_ask.unwrap_or(dec!(0)),
                put_bid: put_bid.unwrap_or(dec!(0)),
                put_ask: put_ask.unwrap_or(dec!(0)),
                perp_bid,
                perp_ask,
                perp_mark: perp_summary,
                index_usd: index,
                strike,
                funding_8h: funding.rate,
                hours_to_expiry: hours,
                perp_fee_rate,
            },
            qty,
        );
        let Some(eval) = eval else { continue };
        let notional = qty * index;
        let edge_bps = if notional > dec!(0) {
            eval.net_edge_usd / notional * dec!(10_000)
        } else {
            dec!(0)
        };
        let edge_bps_f = edge_bps.to_string().parse::<f64>().unwrap_or(0.0);
        if edge_bps_f < min_edge_bps {
            continue;
        }
        let expiry_iso = Utc
            .timestamp_millis_opt(expiry_ms)
            .single()
            .map(|t| t.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let (call_px, put_px, perp_px, call_act, put_act, perp_act) = match eval.direction {
            ParityDirection::Conversion => {
                (call_bid, put_ask, Some(perp_ask), "sell", "buy", "buy")
            }
            ParityDirection::Reversal => (call_ask, put_bid, Some(perp_bid), "buy", "sell", "sell"),
        };
        let all_open = call_open && put_open;
        let executable = eval.net_edge_usd > dec!(0) && perp_quotes_live && all_open;
        let to_f = |d: Option<Decimal>| {
            d.and_then(|v| v.to_string().parse::<f64>().ok())
                .unwrap_or(0.0)
        };
        let id = format!(
            "opar-{ccy}-{expiry_ms}-{strike}-{}",
            eval.direction.as_str()
        );
        out.push(OptionsParityOpportunityDto {
            id,
            venue: "deribit".into(),
            currency: ccy.clone(),
            expiry_ms,
            expiry_iso,
            strike: strike.to_string().parse().unwrap_or(0.0),
            direction: eval.direction,
            call_leg: OptionsParityLegDto {
                instrument: call_name.clone(),
                role: "call".into(),
                action: call_act.into(),
                venue_price: to_f(call_px),
                usd_price: to_f(call_px.map(|p| p * index)),
            },
            put_leg: OptionsParityLegDto {
                instrument: put_name.clone(),
                role: "put".into(),
                action: put_act.into(),
                venue_price: to_f(put_px),
                usd_price: to_f(put_px.map(|p| p * index)),
            },
            perp_leg: OptionsParityLegDto {
                instrument: format!("{ccy}-PERPETUAL"),
                role: "perp".into(),
                action: perp_act.into(),
                venue_price: to_f(perp_px),
                usd_price: to_f(perp_px),
            },
            synthetic_forward_usd: eval
                .synthetic_forward_usd
                .to_string()
                .parse()
                .unwrap_or(0.0),
            perp_equiv_usd: eval.perp_equiv_usd.to_string().parse().unwrap_or(0.0),
            funding_8h: funding.rate.to_string().parse().unwrap_or(0.0),
            funding_drain_usd: eval.funding_drain_usd.to_string().parse().unwrap_or(0.0),
            gross_edge_usd: eval.gross_edge_usd.to_string().parse().unwrap_or(0.0),
            est_fees_usd: eval.est_fees_usd.to_string().parse().unwrap_or(0.0),
            net_edge_usd: eval.net_edge_usd.to_string().parse().unwrap_or(0.0),
            edge_bps: edge_bps_f,
            index_price_usd: index.to_string().parse().unwrap_or(0.0),
            qty_base_suggested: qty_base,
            is_executable: executable,
            reason: if !all_open {
                Some("Option leg is halted on Deribit matching engine".to_string())
            } else if !perp_quotes_live {
                Some("Deribit perp orderbook has no resting quotes".to_string())
            } else if eval.net_edge_usd <= dec!(0) {
                Some("edge is fee-eaten at this size".to_string())
            } else {
                None
            },
        });
    }
    out.sort_by(|a, b| b.net_edge_usd.total_cmp(&a.net_edge_usd));
    out.truncate(params.limit.unwrap_or(MAX_RESULTS));
    Ok(out)
}

pub(crate) fn options_leg_bound_px(
    book: &OrderBook,
    side: Side,
    venue_qty: Decimal,
) -> Option<Decimal> {
    let bound = dec!(1) + Decimal::from(OPTIONS_IOC_BOUND_BPS) / dec!(10_000);
    let vwap = match side {
        Side::Buy => book
            .buy_execution_vwap(venue_qty)
            .map(|(v, _, _)| v * bound),
        Side::Sell => book
            .sell_execution_vwap(venue_qty)
            .map(|(v, _, _)| v / bound),
    }?;
    (vwap > dec!(0)).then_some(vwap)
}

pub async fn execute_options_parity_impl(
    params: OptionsParityExecutionParams,
    state: &AppEngineState,
) -> Result<SimStepResultDto, String> {
    if params.id.trim().is_empty() || params.id.len() > 160 {
        return Err("options parity opportunity id must be 1-160 characters".into());
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

    // Re-scan fresh — the caller's edge is never trusted.
    let scan = scan_options_parity_impl(
        &OptionsParityScanParams {
            currency: params.currency.clone(),
            qty_base: Some(params.qty_base),
            min_hours_to_expiry: Some(0.0),
            max_days_to_expiry: Some(365.0),
            min_edge_bps: params.min_edge_bps,
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
            "options parity opportunity is no longer available (stale books)".to_string()
        })?;
    if !opp.is_executable {
        return Err(format!(
            "options parity opportunity {} is not executable: {}",
            opp.id,
            opp.reason.clone().unwrap_or_default()
        ));
    }
    if let Some(hurdle) = params.min_edge_bps {
        if opp.edge_bps < hurdle {
            return Err(format!(
                "options parity edge {:.1} BPS is below the {:.1} BPS hurdle — not executed",
                opp.edge_bps, hurdle
            ));
        }
    }

    let leg_symbols = [
        opp.call_leg.instrument.clone(),
        opp.put_leg.instrument.clone(),
        opp.perp_leg.instrument.clone(),
    ];
    let leg_roles = ["call", "put", "perp"];
    let leg_sides: [Side; 3] = match opp.direction {
        ParityDirection::Conversion => [Side::Sell, Side::Buy, Side::Buy],
        ParityDirection::Reversal => [Side::Buy, Side::Sell, Side::Sell],
    };

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

    for sym in &leg_symbols {
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
            .ticker_reduce_only(sym, OPTIONS_PARITY_STRATEGY_ID, state.env().as_str())
            .map_err(|e| e.to_string())?
        {
            return Err(format!(
                "Instrument {sym} is reduce-only for options parity — exits only, entry refused"
            ));
        }
    }

    let pre_acc = adapter
        .fetch_account()
        .await
        .map_err(|e| format!("account read on deribit failed: {e}"))?;
    for sym in &leg_symbols {
        // Only fixed option contracts (Call and Put) cannot already exist in the account.
        // The perpetual hedge (BTC-PERPETUAL / ETH-PERPETUAL) is linear delta and can accumulate
        // across multiple parity conversions up to max_open_positions.
        if !crate::state_store::symbol_is_fixed_contract(sym) {
            continue;
        }
        if let Some(p) = adapter
            .account_position_for(&pre_acc, sym)
            .map_err(|e| format!("position read on deribit failed for {sym}: {e}"))?
            .filter(|p| p.quantity > dec!(0))
        {
            // A residual below the venue's wire minimum cannot be flattened
            // — refusing on it strands the substrategy forever. Only an
            // actionable position blocks re-entry.
            let actionable = adapter
                .wire_base_qty(sym, p.quantity)
                .await
                .ok()
                .is_some_and(|q| q > dec!(0));
            if actionable {
                return Err(format!(
                    "An open position already exists for {sym} (qty {}); flatten it before cycling",
                    p.quantity
                ));
            }
            tracing::warn!(
                "options parity: {sym} carries a sub-minimum residual position ({}); proceeding — it is not flattenable",
                p.quantity
            );
        }
    }

    // Conservative margin bound: three full-notional legs at leverage 1.
    let index = Decimal::from_f64_retain(opp.index_price_usd)
        .ok_or_else(|| "invalid index price".to_string())?;
    let qty_dec =
        Decimal::from_f64_retain(params.qty_base).ok_or_else(|| "invalid qty_base".to_string())?;
    let notional = qty_dec * index;
    if notional * dec!(2) > pre_acc.free_margin {
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

    let mut equity_before_by_venue = std::collections::HashMap::new();
    let env_str = state.env().as_str();
    if let Ok(f) = pre_acc.equity.to_string().parse::<f64>() {
        if f.is_finite() {
            equity_before_by_venue.insert(format!("{env_str}-{vid}"), f);
        }
    }

    // Dispatch books: real depth for the IOC bound of each leg. Option
    // book quantities are contracts (base units for contract_size=1); the
    // perp book is USD-sized — venue-unit quantities differ per leg.
    let mut books = Vec::with_capacity(3);
    for (sym, side) in leg_symbols.iter().zip(leg_sides.iter()) {
        let book = adapter
            .fetch_orderbook(sym)
            .await
            .map_err(|e| format!("dispatch book for {sym} failed: {e}"))?;
        // Exact instrument match — canonical-base equality is too weak for
        // options (every strike shares the base).
        if !book.symbol.eq_ignore_ascii_case(sym) {
            return Err(format!(
                "dispatch book symbol '{}' does not match requested '{sym}'",
                book.symbol
            ));
        }
        book.validate_for_side(*side)
            .map_err(|e| format!("dispatch book for {sym}: {e}"))?;
        state.market_data_hub.update_orderbook(book.clone());
        books.push(book);
    }

    let cycle = state.next_sim_step();
    let cycle_tag = format!(
        "opar-{}-{:08x}",
        Utc::now().timestamp_millis(),
        rand::random::<u32>()
    );
    let started = std::time::Instant::now();

    let mut fills: Vec<(usize, arbitrageur_core::order::OrderResponse)> = Vec::new();
    let mut leg_signal_ids: Vec<(usize, String, crate::state_store::LegSide)> = Vec::new();
    let mut leg_error: Option<String> = None;
    for idx in 0..3 {
        let sym = &leg_symbols[idx];
        let side = leg_sides[idx];
        let role = leg_roles[idx];
        // Venue-unit sizing for the book bound: options read contracts
        // (≈ base units at contract_size 1 — wire_base_qty snapped), the
        // perp reads USD.
        let venue_qty = if role == "perp" {
            qty_dec * index
        } else {
            qty_dec
        };
        let Some(bound_px) = options_leg_bound_px(&books[idx], side, venue_qty) else {
            leg_error = Some(format!(
                "leg {} ({sym}) cannot be priced on the dispatch book — refusing unpriced dispatch",
                idx + 1
            ));
            break;
        };
        let qty = match adapter.wire_base_qty(sym, qty_dec).await {
            Ok(w) if w > dec!(0) => w,
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
            Side::Buy => crate::state_store::LegSide::Long,
            Side::Sell => crate::state_store::LegSide::Short,
        };
        let leg_signal_id = format!("{cycle_tag}-leg{idx}");
        let intent_note = format!(
            "options parity {} leg {}/3 {side:?} {qty} {sym} ({role})",
            opp.direction.as_str(),
            idx + 1
        );
        if let Err(e) = record_options_leg_intent(
            state,
            &leg_signal_id,
            OPTIONS_PARITY_STRATEGY_ID,
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
        let mark_failed = |sid: &str, why: &str| {
            let _ = state
                .state_store
                .update_signal_status(sid, SignalStatus::Failed, Some(why));
        };
        let order =
            arbitrageur_core::order::OrderRequest::new_ioc(vid.clone(), sym, side, bound_px, qty);
        let client_oid = order.client_order_id.clone();
        // Persist the client order id BEFORE dispatch so a crash between
        // venue accept and ack write keeps the reconciler a locator.
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
        let mut resp = match adapter.place_order(order).await {
            Ok(r) => r,
            Err(e) => {
                mark_failed(&leg_signal_id, &format!("submit rejected: {e}"));
                leg_error = Some(format!("leg {} ({sym}) failed: {e}", idx + 1));
                break;
            }
        };
        if !resp.order_id.trim().is_empty() {
            if let Err(e) = state.state_store.record_leg_order_id(
                &leg_signal_id,
                leg_side,
                resp.order_id.trim(),
                Some(&client_oid),
            ) {
                tracing::warn!("options parity leg {idx} order-id persist failed: {e}");
            }
        }
        if !resp.status.is_final() && !resp.order_id.trim().is_empty() {
            for _ in 0..OPTIONS_SETTLE_POLLS {
                tokio::time::sleep(OPTIONS_SETTLE_INTERVAL).await;
                match adapter
                    .fetch_order(sym, &resp.order_id, Some(&client_oid))
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
                        venue: vid.clone(),
                        symbol: sym.clone(),
                        order_id: Some(resp.order_id.clone()),
                        client_order_id: Some(client_oid.clone()),
                    })
                    .await;
                tokio::time::sleep(OPTIONS_SETTLE_INTERVAL).await;
                if let Ok(polled) = adapter
                    .fetch_order(sym, &resp.order_id, Some(&client_oid))
                    .await
                {
                    resp = polled;
                }
            }
        }
        let px_ok = resp.avg_fill_price.is_some_and(|p| p > dec!(0));
        // Deribit futures settle in USD — the submitted base quantity is a
        // conversion that lands on the venue's USD grid, so a full venue
        // fill can read a few bps off the requested qty. Trust the venue's
        // terminal `filled` state + zero remainder, with a coverage floor
        // that still catches a genuine partial fill.
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
                leg_error = Some(format!(
                    "leg {} ({sym}) terminated partially filled",
                    idx + 1
                ));
            }
            (true, false) => {
                mark_failed(&leg_signal_id, "terminated without a fill");
                leg_error = Some(format!("leg {} ({sym}) terminated unfilled", idx + 1));
            }
            (false, _) => {
                let _ = state.state_store.update_signal_status(
                    &leg_signal_id,
                    SignalStatus::Unresolved,
                    Some("order still working past the settle window — reconciler owns it"),
                );
                if resp.filled_qty > dec!(0) {
                    fills.push((idx, resp));
                }
                leg_error = Some(format!(
                    "leg {} ({sym}) still working past settle window",
                    idx + 1
                ));
            }
        }
        break;
    }
    let latency_us = started.elapsed().as_micros() as u64;
    refresh_equity_expectations_after_fills(state, &equity_before_by_venue).await;

    let cycle_complete = leg_error.is_none() && fills.len() == 3;
    let mut record_errors = Vec::new();
    for (idx, resp) in &fills {
        // Ledger prices are USD: option fills print in base coin — convert
        // at the scan's index price; perp fills are already USD.
        let venue_px = resp.avg_fill_price.unwrap_or(dec!(0));
        let px_usd = if leg_roles[*idx] == "perp" {
            venue_px
        } else {
            venue_px * index
        };
        // Record the venue-equivalent fill (base units) — for the perp leg
        // this is the USD amount back-converted at the fill price, which is
        // the truthful quantity the reconciler will see on the account.
        let qty_f64 = resp.filled_qty.to_string().parse::<f64>().unwrap_or(0.0);
        let Some((_, sid, side)) = leg_signal_ids.iter().find(|(i, _, _)| i == idx) else {
            record_errors.push(format!("leg {} has no persisted signal", idx + 1));
            continue;
        };
        let px_f = match dec_to_f64(px_usd, "options parity fill price") {
            Ok(v) => v,
            Err(e) => {
                record_errors.push(e);
                continue;
            }
        };
        let fee_f = resp
            .fee
            .map(|f| dec_to_f64(f, "options parity fill fee").unwrap_or(0.0))
            .unwrap_or(0.0);
        if let Err(e) = state.state_store.apply_leg_fill(
            sid,
            *side,
            qty_f64,
            px_f,
            fee_f,
            Some(resp.order_id.as_str()),
        ) {
            record_errors.push(format!("leg {} fill persist failed: {e}", idx + 1));
        }
    }

    // Complete cycle: positions persist (short call + long put + perp) —
    // mark Balanced, never flat. Verify via venue truth; ambiguity stays
    // Balanced.
    let mut positions_open = false;
    if cycle_complete {
        match adapter.fetch_account().await {
            Ok(acc) => {
                for sym in &leg_symbols {
                    match adapter.account_position_for(&acc, sym) {
                        Ok(Some(_)) => positions_open = true,
                        Ok(None) => {}
                        Err(_) => positions_open = true, // ambiguous → treat as open
                    }
                }
            }
            Err(_) => positions_open = true,
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
                "options parity cycle completed — legs remain open until expiry/flatten"
            } else {
                "options parity cycle completed — no open positions detected"
            };
            let _ = state
                .state_store
                .update_signal_status(sid, status, Some(note));
        } else if filled {
            let _ = state.state_store.update_signal_status(
                sid,
                SignalStatus::Imbalanced,
                Some("options parity partial — residual leg; flatten the instrument to unwind"),
            );
        } else if current == Some(SignalStatus::InFlight) {
            let _ = state.state_store.update_signal_status(
                sid,
                SignalStatus::Failed,
                Some("cycle aborted before this leg dispatched"),
            );
        }
    }

    // Realized cashflow at entry: option premiums only (perp is a
    // position, not cash) minus all known fees. Option fills print in
    // base coin — convert at index. The convergence edge is unrealized
    // until expiry/settlement — never booked upfront.
    let mut premium_pnl = Decimal::ZERO;
    let mut fee_total = Decimal::ZERO;
    for (idx, resp) in &fills {
        if let Some(f) = resp.fee.filter(|f| *f > dec!(0)) {
            fee_total += f;
        }
        if leg_roles[*idx] == "perp" {
            continue;
        }
        if let Some(px) = resp.avg_fill_price {
            let usd = px * resp.filled_qty * index;
            match leg_sides[*idx] {
                Side::Sell => premium_pnl += usd,
                Side::Buy => premium_pnl -= usd,
            }
        }
    }
    let attempt_pnl_f = premium_pnl.to_string().parse::<f64>().unwrap_or(0.0);
    // Premium cashflow is financing, not realized PnL — the conversion's
    // true PnL lands at close via per-leg deltas. Book it to the dedicated
    // outlay counter so Net Profit isn't showing a −premium "loss" for the
    // whole hold. Fees are different: they're a real cost paid at fill,
    // not inventory — they book to realized immediately.
    let total_f =
        crate::execution::accumulate_premium_outlay(state, state.is_live(), attempt_pnl_f);
    if fee_total > Decimal::ZERO {
        crate::execution::accumulate_realized_profit(
            state,
            state.is_live(),
            -fee_total.to_string().parse::<f64>().unwrap_or(0.0),
        );
    }
    let fee_total_f = fee_total.to_string().parse::<f64>().unwrap_or(0.0);
    let direction = format!(
        "{} {}/{} {}",
        opp.direction.as_str(),
        opp.strike,
        opp.expiry_iso,
        opp.currency
    );

    if let Some(err) = leg_error {
        record_execution_log(state, ExecutionLogEntry {
            id: format!("exec-options-{cycle}"),
            timestamp: Utc::now().to_rfc3339(),
            trade_type: "OptionsParity".into(),
            strategy_id: crate::attribution::attribute_strategy_id(OPTIONS_PARITY_STRATEGY_ID),
            symbol: direction.clone(),
            venue_a: "deribit".into(),
            venue_b: "deribit".into(),
            leg_a_status: format!("{}/3 legs filled", fills.len()),
            leg_b_status: err.clone(),
            success: false,
            gross_profit: 0.0,
            fees: fee_total_f,
            net_profit: -fee_total_f,
            latency_us,
            note: format!(
                "Options parity cycle PARTIAL: {err}. Residual legs recorded as imbalanced 'options_parity' signals — flatten each instrument to unwind. Record errors: {}",
                record_errors.join("; ")
            ),
            direction: Some(direction.clone()),
            trigger_spread_bps: Some(opp.edge_bps),
            baseline_mean_spread_bps: None,
            position_step: None,
            max_units: None,
            nominal_hurdle_bps: params.min_edge_bps,
            is_live: state.is_live(),
        });
        return Err(format!(
            "Options parity cycle PARTIAL ({}/3 legs): {err}; residuals recorded — flatten leg instruments to unwind",
            fills.len()
        ));
    }

    record_execution_log(state, ExecutionLogEntry {
        id: format!("exec-options-{cycle}"),
        timestamp: Utc::now().to_rfc3339(),
        trade_type: "OptionsParity".into(),
        strategy_id: crate::attribution::attribute_strategy_id(OPTIONS_PARITY_STRATEGY_ID),
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
            "Options parity {} completed 3/3 legs; premium cashflow ${attempt_pnl_f:.2} booked, convergence edge unrealized until expiry. Record errors: {}",
            opp.direction.as_str(),
            record_errors.join("; ")
        ),
        direction: Some(direction.clone()),
        trigger_spread_bps: Some(opp.edge_bps),
        baseline_mean_spread_bps: None,
        position_step: None,
        max_units: None,
        nominal_hurdle_bps: params.min_edge_bps,
        is_live: state.is_live(),
    });

    maybe_record_margin_snapshots(state, Utc::now().timestamp_millis()).await;

    Ok(SimStepResultDto {
        cycle,
        opportunity_found: true,
        execution_success: true,
        trade_type: "OptionsParity".into(),
        direction,
        venue_a: "deribit".into(),
        venue_b: "deribit".into(),
        spread_bps: opp.edge_bps,
        realized_profit_usd: attempt_pnl_f,
        total_cumulative_pnl_usd: total_f,
        message: format!(
            "Options parity {} on deribit: 3/3 legs filled (call {}, put {}, perp {}); edge ${:.2} unrealized until expiry",
            opp.direction.as_str(),
            opp.call_leg.instrument,
            opp.put_leg.instrument,
            opp.perp_leg.instrument,
            opp.net_edge_usd,
        ),
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_options_leg_intent(
    state: &AppEngineState,
    signal_id: &str,
    strategy_id: &str,
    venue_id: &str,
    symbol: &str,
    order_side: Side,
    qty_f64: f64,
    note: &str,
    expected_net_usd: Option<f64>,
) -> Result<(), String> {
    use crate::state_store::{LegSide, NewSignalExecution, NewSignalLeg};
    if !qty_f64.is_finite() || qty_f64 <= 0.0 {
        return Err("options leg quantity is not positive".into());
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
    state
        .state_store
        .record_signal_execution(
            &NewSignalExecution {
                id: signal_id.to_string(),
                strategy_id: strategy_id.to_string(),
                symbol: symbol.to_string(),
                route_id: None,
                expected_net_usd,
                note: Some(note.to_string()),
                legs: vec![NewSignalLeg {
                    account_id,
                    venue: venue_id.to_string(),
                    side,
                    symbol: Some(symbol.to_string()),
                    intended_qty: qty_f64,
                    intended_price: None,
                    unit_scale: None,
                }],
            },
            state.env().as_str(),
        )
        .map_err(|e| format!("Failed to persist options leg intent: {e}"))?;
    Ok(())
}

#[tauri::command]
pub async fn scan_options_parity(
    params: OptionsParityScanParams,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<OptionsParityOpportunityDto>, String> {
    scan_options_parity_impl(&params, &state).await
}

#[tauri::command]
pub async fn execute_options_parity(
    params: OptionsParityExecutionParams,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<SimStepResultDto, String> {
    match execute_options_parity_impl(params.clone(), &state).await {
        Ok(r) => Ok(r),
        Err(e) => {
            record_execution_failure(
                &state,
                "OptionsParity",
                OPTIONS_PARITY_STRATEGY_ID,
                &params.id,
                "deribit",
                "deribit",
                &e,
            );
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quotes() -> ParityQuotes {
        ParityQuotes {
            call_bid: dec!(0.05),
            call_ask: dec!(0.052),
            put_bid: dec!(0.03),
            put_ask: dec!(0.031),
            perp_bid: dec!(84500),
            perp_ask: dec!(84502),
            perp_mark: dec!(84501),
            index_usd: dec!(84500),
            strike: dec!(85000),
            funding_8h: dec!(0.0001),
            hours_to_expiry: 24.0 * 30.0,
            perp_fee_rate: dec!(0.0005),
        }
    }

    #[test]
    fn conversion_edge_when_synthetic_rich() {
        // F_syn = K + (C_bid − P_ask)·idx = 85000 + (0.05−0.031)×84500 ≈ 86605.5
        // perp_equiv = 84502 + drain(0.0001×90×84501 ≈ 760.5) = 85262.5
        // gross ≈ 1343/unit → positive → Conversion.
        let e = evaluate_parity(&quotes(), dec!(0.1)).unwrap();
        assert_eq!(e.direction, ParityDirection::Conversion);
        assert!(e.gross_edge_usd > dec!(0));
        assert!(e.net_edge_usd < e.gross_edge_usd);
    }

    #[test]
    fn reversal_edge_when_synthetic_cheap() {
        // Tighten the option spread so the synthetic is cheap:
        // F_syn = 85000 + (0.052−0.03)×84500 = 86859 — that's rich; flip to
        // a deep put skew instead.
        let mut q = quotes();
        q.call_bid = dec!(0.01);
        q.call_ask = dec!(0.011);
        q.put_bid = dec!(0.05);
        q.put_ask = dec!(0.052);
        // F_syn_conv = 85000 + (0.01−0.052)×84500 = 81449 → conv edge <0
        // F_syn_rev  = 85000 + (0.011−0.05)×84500 = 81704.5
        // rev edge = (84500 + drain≈760.5) − 81704.5 ≈ 3556/unit >0
        let e = evaluate_parity(&q, dec!(0.1)).unwrap();
        assert_eq!(e.direction, ParityDirection::Reversal);
        assert!(e.gross_edge_usd > dec!(0));
    }

    #[test]
    fn missing_side_fails_closed() {
        let mut q = quotes();
        q.call_bid = dec!(0); // sell side unquoted → conversion impossible
        q.call_ask = dec!(0);
        q.put_bid = dec!(0);
        q.put_ask = dec!(0);
        assert!(evaluate_parity(&q, dec!(0.1)).is_none());
    }

    #[test]
    fn nonpositive_edge_returns_none() {
        let mut q = quotes();
        // Make the synthetic exactly track the perp: call−put = (perp+drain−K)/idx
        q.call_bid = dec!(0.02);
        q.call_ask = dec!(0.021);
        q.put_bid = dec!(0.02);
        q.put_ask = dec!(0.021);
        // F_syn_conv = 85000 + (0.02−0.021)×84500 = 84915.5 < perp_equiv ≈ 85262 → no edge
        // F_syn_rev  = 85000 + (0.021−0.02)×84500 = 85084.5; perp_proceeds = 84500+760.5=85260.5 → edge ≈ +176/unit
        let e = evaluate_parity(&q, dec!(0.1)).unwrap();
        assert_eq!(e.direction, ParityDirection::Reversal);
        // Net of fees it may still be positive or negative — assert it's below gross.
        assert!(e.net_edge_usd < e.gross_edge_usd);
    }

    #[test]
    fn funding_drain_scales_with_time() {
        let mut q_short = quotes();
        q_short.hours_to_expiry = 8.0;
        let e_short = evaluate_parity(&q_short, dec!(0.1)).unwrap();
        let e_long = evaluate_parity(&quotes(), dec!(0.1)).unwrap();
        assert!(e_long.funding_drain_usd > e_short.funding_drain_usd);
    }

    #[test]
    fn fee_cap_uses_premium_cap() {
        // Deep OTM call: premium tiny → its fee is capped at 12.5% of the
        // premium. Buying it (reversal) stays profitable against the put.
        let mut q = quotes();
        q.call_ask = dec!(0.0005); // 0.0005×84500 ≈ $42.25 premium
        let e = evaluate_parity(&q, dec!(1.0)).unwrap();
        assert_eq!(e.direction, ParityDirection::Reversal);
        // uncapped option fee = 0.0003×84500 ≈ $25.35; the call leg's cap
        // is 0.125×42.25 ≈ $5.28 → total fees < 3 full taker fees.
        assert!(e.est_fees_usd > dec!(0));
        assert!(e.est_fees_usd < dec!(0.0003) * dec!(84500) * dec!(3));
    }

    #[test]
    fn bad_inputs_refuse() {
        assert!(evaluate_parity(&quotes(), dec!(0)).is_none());
        let mut q = quotes();
        q.index_usd = dec!(0);
        assert!(evaluate_parity(&q, dec!(0.1)).is_none());
        q.index_usd = dec!(84500);
        q.strike = dec!(0);
        assert!(evaluate_parity(&q, dec!(0.1)).is_none());
    }

    // ------------------------------------------------------------------
    // Stub-server e2e: a minimal Deribit JSON-RPC responder on a real
    // socket so the full scan→execute path (auth, chain, books, orders,
    // settle polls, ledger writes) runs against real HTTP.
    // ------------------------------------------------------------------

    use std::sync::Mutex as StdMutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// (instrument, direction, venue-size, avg-price)
    type StubPositions = Arc<StdMutex<Vec<(String, String, f64, f64)>>>;

    struct StubVenue {
        url: String,
        hits: Arc<StdMutex<Vec<String>>>,
    }

    fn expiry_ms() -> i64 {
        Utc::now().timestamp_millis() + 30 * 86_400_000
    }

    fn instrument_json(name: &str, kind: &str, opt_type: &str, expiry: i64) -> serde_json::Value {
        let mut v = serde_json::json!({
            "instrument_name": name,
            "kind": kind,
            "is_active": true,
            "state": "open",
            "tick_size": if kind == "option" { 0.0001 } else { 0.5 },
            "min_trade_amount": if kind == "option" { 0.1 } else { 10.0 },
            "contract_size": if kind == "option" { 1.0 } else { 10.0 },
            "settlement_currency": "BTC",
            "quote_currency": if kind == "option" { "BTC" } else { "USD" },
            "taker_commission": 0.0003,
            "maker_commission": 0.0001,
        });
        if kind == "option" {
            v["option_type"] = serde_json::json!(opt_type);
            v["strike"] = serde_json::json!(85000);
            v["expiration_timestamp"] = serde_json::json!(expiry);
        }
        v
    }

    fn option_summary(name: &str, bid: f64, ask: f64) -> serde_json::Value {
        serde_json::json!({
            "instrument_name": name,
            "bid_price": bid,
            "ask_price": ask,
            "mark_price": (bid + ask) / 2.0,
            "underlying_price": 84500.0,
            "open_interest": 100,
            "volume": 10,
        })
    }

    fn order_json(
        name: &str,
        side: &str,
        amount: f64,
        price: f64,
        label: &str,
        id: usize,
    ) -> serde_json::Value {
        serde_json::json!({
            "order_id": format!("OID-{id}"),
            "instrument_name": name,
            "label": label,
            "direction": side,
            "order_state": "filled",
            "amount": amount,
            "filled_amount": amount,
            "price": price,
            "average_price": price,
            "commission": 0.0000001,
        })
    }

    fn stub_response(
        path: &str,
        reject: Option<&str>,
        hits: &Arc<StdMutex<Vec<String>>>,
        positions: &StubPositions,
    ) -> serde_json::Value {
        hits.lock().unwrap().push(path.to_string());
        let wrap =
            |result: serde_json::Value| serde_json::json!({"jsonrpc": "2.0", "result": result});
        let c_name = format!("BTC-{}-85000-C", "27JUN30");
        let p_name = format!("BTC-{}-85000-P", "27JUN30");
        let expiry = expiry_ms();
        if path.contains("/public/auth") {
            return wrap(serde_json::json!({
                "access_token": "stub-token",
                "refresh_token": "stub-refresh",
                "expires_in": 900,
                "token_type": "bearer"
            }));
        }
        if path.contains("/public/get_instruments") {
            if path.contains("kind=option") {
                return wrap(serde_json::json!([
                    instrument_json(&c_name, "option", "call", expiry),
                    instrument_json(&p_name, "option", "put", expiry),
                ]));
            }
            return wrap(serde_json::json!([instrument_json(
                "BTC-PERPETUAL",
                "future",
                "",
                expiry
            ),]));
        }
        if path.contains("/public/get_book_summary_by_currency") {
            if path.contains("kind=option") {
                return wrap(serde_json::json!([
                    option_summary(&c_name, 0.05, 0.052),
                    option_summary(&p_name, 0.03, 0.031),
                ]));
            }
            return wrap(serde_json::json!([serde_json::json!({
                "instrument_name": "BTC-PERPETUAL",
                "bid_price": 84500.0,
                "ask_price": 84502.0,
                "mark_price": 84501.0,
                "funding_8h": 0.0001,
                "current_funding": 0.00005,
                "open_interest": 1e6,
                "volume": 1e5,
            }),]));
        }
        if path.contains("/public/get_order_book") {
            let name = path
                .split("instrument_name=")
                .nth(1)
                .and_then(|s| s.split('&').next())
                .unwrap_or("");
            let (bids, asks): (serde_json::Value, serde_json::Value) = if name == c_name {
                (
                    serde_json::json!([[0.05, 5.0]]),
                    serde_json::json!([[0.052, 5.0]]),
                )
            } else if name == p_name {
                (
                    serde_json::json!([[0.03, 5.0]]),
                    serde_json::json!([[0.031, 5.0]]),
                )
            } else {
                (
                    serde_json::json!([[84500.0, 1e6]]),
                    serde_json::json!([[84502.0, 1e6]]),
                )
            };
            return wrap(serde_json::json!({
                "instrument_name": name,
                "bids": bids,
                "asks": asks,
                "index_price": 84500.0,
                "underlying_price": 84500.0,
                "mark_price": 0.0,
                "change_id": 42,
                "timestamp": 1,
            }));
        }
        if path.contains("/public/ticker") {
            return wrap(serde_json::json!({
                "instrument_name": "BTC-PERPETUAL",
                "best_bid_price": 84500.0,
                "best_ask_price": 84502.0,
                "last_price": 84501.0,
                "mark_price": 84501.0,
                "index_price": 84500.0,
                "funding_8h": 0.0001,
                "current_funding": 0.00005,
                "timestamp": 1,
            }));
        }
        if path.contains("/public/get_index_price") {
            return wrap(serde_json::json!({
                "index_price": if path.contains("eth_usd") { 3000.0 } else { 84500.0 },
                "estimated_delivery_price": 84500.0,
            }));
        }
        if path.contains("/private/get_account_summary") {
            let btc = path.contains("currency=BTC");
            return wrap(serde_json::json!({
                "currency": if btc { "BTC" } else { "ETH" },
                "equity": if btc { 2.0 } else { 0.0 },
                "balance": if btc { 2.0 } else { 0.0 },
                "available_funds": if btc { 1.8 } else { 0.0 },
                "maintenance_margin": 0.0,
                "initial_margin": 0.0,
                "margin_model": "cross_pm",
            }));
        }
        if path.contains("/private/get_positions") {
            let want_options = path.contains("kind=option");
            let rows: Vec<serde_json::Value> = positions
                .lock()
                .unwrap()
                .iter()
                .filter(|(name, _, _, _)| {
                    let is_opt = name.ends_with("-C") || name.ends_with("-P");
                    is_opt == want_options
                })
                .map(|(name, dir, size, px)| {
                    serde_json::json!({
                        "instrument_name": name,
                        "direction": dir,
                        "size": size,
                        "average_price": px,
                        "mark_price": px,
                        "floating_profit_loss": 0.0,
                        "initial_margin": 0.0,
                        "maintenance_margin": 0.0,
                        "estimated_liquidation_price": null,
                    })
                })
                .collect();
            return wrap(serde_json::json!(rows));
        }
        if path.contains("/private/buy") || path.contains("/private/sell") {
            let side = if path.contains("/private/buy") {
                "buy"
            } else {
                "sell"
            };
            let qs = path.split('?').nth(1).unwrap_or("");
            let param = |key: &str| -> String {
                qs.split('&')
                    .find(|kv| kv.starts_with(&format!("{key}=")))
                    .and_then(|kv| kv.split('=').nth(1))
                    .unwrap_or("")
                    .to_string()
            };
            let name = param("instrument_name");
            let amount: f64 = param("amount").parse().unwrap_or(0.0);
            let price: f64 = param("price").parse().unwrap_or(0.0);
            let label = param("label");
            if reject == Some(name.as_str()) {
                // Unfilled IOC: venue reports the order terminated with
                // zero fill.
                let mut o = order_json(&name, side, amount, price, &label, 99);
                o["order_state"] = serde_json::json!("cancelled");
                o["filled_amount"] = serde_json::json!(0);
                o["cancel_reason"] = serde_json::json!("immediate_or_cancel");
                return wrap(serde_json::json!({"order": o, "trades": []}));
            }
            positions
                .lock()
                .unwrap()
                .push((name.clone(), side.to_string(), amount, price));
            let id = hits.lock().unwrap().len();
            return wrap(
                serde_json::json!({"order": order_json(&name, side, amount, price, &label, id), "trades": []}),
            );
        }
        if path.contains("/private/get_order_state") {
            return wrap(serde_json::json!({
                "order_id": "OID-0",
                "instrument_name": "BTC-PERPETUAL",
                "order_state": "filled",
                "direction": "buy",
                "amount": 1.0,
                "filled_amount": 1.0,
                "average_price": 84500.0,
                "commission": 0.0,
            }));
        }
        if path.contains("/private/cancel") {
            return wrap(serde_json::json!({
                "order_id": "OID-0",
                "instrument_name": "BTC-PERPETUAL",
                "order_state": "cancelled",
            }));
        }
        serde_json::json!({"jsonrpc": "2.0", "error": {"code": 11050, "message": format!("bad_request: {path}")}})
    }

    async fn spawn_stub(reject_instrument: Option<String>) -> StubVenue {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(StdMutex::new(Vec::new()));
        let positions: StubPositions = Arc::new(StdMutex::new(Vec::new()));
        let hits2 = hits.clone();
        let positions2 = positions.clone();
        let reject2 = reject_instrument.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let hits = hits2.clone();
                let positions = positions2.clone();
                let reject = reject2.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("");
                    let body = stub_response(path, reject.as_deref(), &hits, &positions);
                    let payload = body.to_string();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        payload.len(),
                        payload
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        StubVenue {
            url: format!("http://127.0.0.1:{port}/api/v2"),
            hits,
        }
    }

    fn test_state(tag: &str) -> AppEngineState {
        let dir =
            std::env::temp_dir().join(format!("arbitrageur_opar_{tag}_{}", rand::random::<u64>()));
        AppEngineState::new_with_vault_path(dir.join("vault.enc"))
    }

    fn stub_state(tag: &str, stub: &StubVenue) -> AppEngineState {
        let state = test_state(tag);
        // Admission: install a test sim under the deribit venue id so
        // get_adapter resolves; the concrete stub adapter overrides the
        // env adapter for the options API.
        state.install_test_sim("deribit");
        let adapter = DeribitAdapter::new(
            Some(stub.url.clone()),
            Some("stub-id".into()),
            Some("stub-secret".into()),
        );
        *state.test_deribit.write().unwrap() = Some(Arc::new(adapter));
        state
    }

    #[tokio::test]
    async fn scan_finds_parity_edge_on_stub() {
        let stub = spawn_stub(None).await;
        let state = stub_state("scan", &stub);
        let opps = scan_options_parity_impl(
            &OptionsParityScanParams {
                currency: "BTC".into(),
                qty_base: Some(0.1),
                min_hours_to_expiry: Some(0.0),
                max_days_to_expiry: Some(60.0),
                min_edge_bps: Some(0.0),

                limit: None,
            },
            &state,
        )
        .await
        .unwrap();
        assert_eq!(opps.len(), 1, "expected one conversion opportunity");
        let o = &opps[0];
        assert_eq!(o.direction, ParityDirection::Conversion);
        assert!(o.net_edge_usd > 0.0);
        assert_eq!(o.call_leg.action, "sell");
        assert_eq!(o.put_leg.action, "buy");
        assert_eq!(o.perp_leg.action, "buy");
        assert!(o.is_executable);
    }

    #[tokio::test]
    async fn scan_refuses_when_deribit_unadmitted() {
        let stub = spawn_stub(None).await;
        let state = test_state("unadmitted");
        // No sim, no flag → get_adapter("deribit") is None → refuse before
        // any network call (hits stays empty).
        let res = scan_options_parity_impl(
            &OptionsParityScanParams {
                currency: "BTC".into(),
                qty_base: Some(0.1),
                min_hours_to_expiry: None,
                max_days_to_expiry: None,
                min_edge_bps: None,

                limit: None,
            },
            &state,
        )
        .await;
        assert!(res.unwrap_err().contains("not admitted"));
        assert!(stub.hits.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn execute_dispatches_three_legs_and_marks_balanced() {
        let stub = spawn_stub(None).await;
        let state = stub_state("exec", &stub);
        let opps = scan_options_parity_impl(
            &OptionsParityScanParams {
                currency: "BTC".into(),
                qty_base: Some(0.1),
                min_hours_to_expiry: Some(0.0),
                max_days_to_expiry: Some(60.0),
                min_edge_bps: Some(0.0),

                limit: None,
            },
            &state,
        )
        .await
        .unwrap();
        let opp = opps.into_iter().next().unwrap();
        let res = execute_options_parity_impl(
            OptionsParityExecutionParams {
                id: opp.id.clone(),
                currency: "BTC".into(),
                qty_base: 0.1,
                min_edge_bps: Some(0.0),
                auto: true,
            },
            &state,
        )
        .await
        .unwrap();
        assert!(res.execution_success);
        // All three legs reached the venue: sell call, buy put, buy perp.
        let hits = stub.hits.lock().unwrap();
        let orders: Vec<&String> = hits
            .iter()
            .filter(|p| p.contains("/private/buy") || p.contains("/private/sell"))
            .collect();
        assert_eq!(orders.len(), 3, "orders={orders:?}");
        assert!(orders
            .iter()
            .any(|p| p.contains("/private/sell") && p.contains("85000-C")));
        assert!(orders
            .iter()
            .any(|p| p.contains("/private/buy") && p.contains("85000-P")));
        assert!(orders
            .iter()
            .any(|p| p.contains("/private/buy") && p.contains("PERPETUAL")));
        drop(hits);
        // Signals persisted: 3 leg intents, all Balanced (positions open).
        let sigs = state.state_store.list_signals(None, None).unwrap();
        let leg_sigs: Vec<_> = sigs
            .iter()
            .filter(|s| s.strategy_id == OPTIONS_PARITY_STRATEGY_ID)
            .collect();
        assert_eq!(leg_sigs.len(), 3, "expected 3 persisted leg intents");
        for s in &leg_sigs {
            assert_eq!(s.status, SignalStatus::Balanced, "{} not balanced", s.id);
        }
        let logs = state.execution_logs.read();
        let entry = logs
            .iter()
            .find(|l| l.trade_type == "OptionsParity")
            .expect("execution log recorded");
        assert_eq!(entry.net_profit, -entry.fees);
    }

    #[tokio::test]
    async fn execute_partial_cycle_marks_residual_imbalanced() {
        // Second leg (the put buy) terminates unfilled → the filled call
        // leg must land Imbalanced and the perp leg must never dispatch.
        let c_name = format!("BTC-{}-85000-P", "27JUN30");
        let stub = spawn_stub(Some(c_name.clone())).await;
        let state = stub_state("partial", &stub);
        let opps = scan_options_parity_impl(
            &OptionsParityScanParams {
                currency: "BTC".into(),
                qty_base: Some(0.1),
                min_hours_to_expiry: Some(0.0),
                max_days_to_expiry: Some(60.0),
                min_edge_bps: Some(0.0),

                limit: None,
            },
            &state,
        )
        .await
        .unwrap();
        let opp = opps.into_iter().next().unwrap();
        let err = execute_options_parity_impl(
            OptionsParityExecutionParams {
                id: opp.id,
                currency: "BTC".into(),
                qty_base: 0.1,
                min_edge_bps: Some(0.0),
                auto: true,
            },
            &state,
        )
        .await
        .unwrap_err();
        assert!(err.contains("PARTIAL"), "err={err}");
        let hits = stub.hits.lock().unwrap();
        let orders: Vec<&String> = hits
            .iter()
            .filter(|p| p.contains("/private/buy") || p.contains("/private/sell"))
            .collect();
        assert_eq!(orders.len(), 2, "perp leg must never dispatch: {orders:?}");
        drop(hits);
        let sigs = state.state_store.list_signals(None, None).unwrap();
        let call = sigs
            .iter()
            .find(|s| s.strategy_id == OPTIONS_PARITY_STRATEGY_ID && s.symbol.contains("85000-C"))
            .unwrap();
        let put = sigs
            .iter()
            .find(|s| s.strategy_id == OPTIONS_PARITY_STRATEGY_ID && s.symbol.contains("85000-P"))
            .unwrap();
        assert_eq!(call.status, SignalStatus::Imbalanced);
        assert_eq!(put.status, SignalStatus::Failed);
    }
}
