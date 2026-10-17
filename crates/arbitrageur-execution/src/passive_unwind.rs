//! Passive limit-order unwinds / rebalances (paper-only, fail-closed).
//!
//! Entries stay IOC and emergency unwinds stay market — this module only
//! softens *reduce* paths (flatten, convergence exits, and any future
//! trim/rebalance order site, which must route through the same helpers).
//!
//! # Paper-sim fill assumption (honest model, read before tuning)
//!
//! The paper simulator evaluates every limit attempt against the **current
//! book snapshot at placement time**: a buy limit fills only against asks
//! `<= limit`, a sell limit only against bids `>= limit`. There is no
//! resting-order queue — an untouched limit returns `Rejected` with zero
//! fill instead of working. The driver therefore treats each attempt as one
//! touch-probe: on an unfilled attempt it waits `timeout_secs`, re-fetches a
//! **fresh** book, re-prices, and retries, up to `max_tries` attempts. After
//! exhaustion (or when the calm gate fails) it falls back to a market/IOC
//! reduce-only exit so exposure is never left open. No limit is ever left
//! working unbounded: every attempt is timeout-bound.
//!
//! # Data-derived defaults
//!
//! Measured read-only from the durable SQLite state (`spread_samples` ≈
//! 510k rows, `session_profiles` 470 rows; zero fills in `signal_executions`
//! today, so all live exits to date are market/IOC by construction):
//!
//! - majors are tight (BTC-USDC mean spread 0.43 bps, ETH-USDC 0.64 bps),
//!   so a mid-pegged limit (`spread_percentile = 50`) saves ~half the
//!   crossing cost versus a market take while remaining touchable;
//! - book sampling cadence is p50 ≈ 2.3 s / p90 ≈ 3.9 s, so
//!   `timeout_secs = 10` spans ~3–4 fresh book views per try — far below
//!   the mean position hold (~9532 s), i.e. no urgency is sacrificed;
//! - `max_tries = 3` bounds the worst case at ~30 s before market fallback;
//! - session realized-vol distribution is p50 3.75 / p75 6.26 / p90 10.12
//!   bps (calmest session `asia` ≈ 5.64, roughest `us_late` ≈ 7.27), so the
//!   calm gate defaults to `max_spread_bps = 15` (≈ session p90 median
//!   spread) and `max_fast_vol_bps = 10` (≈ rv p90): passive only when safe.
//!
//! Depth persistence (would a passive-edge quote get touched?) is *not*
//! measurable from `spread_samples` (top-of-book only, no sizes), which is
//! why the default pegs mid (50) rather than the passive edge (100).

use std::sync::Arc;

use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::quote::OrderBook;
use arbitrageur_core::types::{Decimal, OrderStatus, Side, VenueId};
use serde::{Deserialize, Serialize};

fn default_enabled() -> bool {
    false
}
fn default_spread_percentile() -> f64 {
    50.0
}
fn default_timeout_secs() -> u64 {
    10
}
fn default_max_tries() -> u32 {
    3
}
fn default_max_spread_bps() -> f64 {
    15.0
}
fn default_max_fast_vol_bps() -> f64 {
    10.0
}
fn default_max_adverse_drift_bps() -> Option<f64> {
    Some(5.0)
}
fn default_true() -> bool {
    true
}
fn default_min_queue_seniority_ratio() -> Option<f64> {
    Some(0.20)
}
fn default_queue_hazard_pickoff_threshold() -> Option<f64> {
    Some(0.70)
}

/// Per-strategy passive-unwind controls. `enabled` is opt-in (default off)
/// so existing market/IOC behavior is unchanged until the operator enables
/// it per strategy. Prediction carries no instance: prediction adapters are
/// scaffolds with no execution path, so passive unwind is inert there.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PassiveUnwindConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Peg inside the spread measured from the aggressive touch:
    /// 0 = touch far side (buy→ask, sell→bid; market-like fill odds),
    /// 50 = mid, 100 = passive edge (buy→bid, sell→ask; best price,
    /// lowest touch odds).
    #[serde(default = "default_spread_percentile")]
    pub spread_percentile: f64,
    /// Seconds to wait on an unfilled attempt before re-pricing. Every
    /// attempt is timeout-bound. `0` skips the wait (test/tight-loop use);
    /// the user-facing configurator requires 1..=300.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// Limit attempts before falling back to a market/IOC exit.
    #[serde(default = "default_max_tries")]
    pub max_tries: u32,
    /// Calm gate: skip passive when the live spread exceeds this (bps).
    #[serde(default = "default_max_spread_bps")]
    pub max_spread_bps: f64,
    /// Calm gate: skip passive when fast vol exceeds this (bps). `None`
    /// fast-vol readings pass this leg (the spread leg is the hard guard).
    #[serde(default = "default_max_fast_vol_bps")]
    pub max_fast_vol_bps: f64,
    /// Maximum tolerable adverse mid-price drift in basis points while an order is resting.
    /// If live market mid drifts adversely by more than this threshold against our resting limit order,
    /// the order is immediately canceled before snipers pick it off. Default: Some(5.0).
    #[serde(default = "default_max_adverse_drift_bps")]
    pub max_adverse_drift_bps: Option<f64>,
    /// Whether Level-3 MBO queue drain hazard evacuation is active. Default: true.
    #[serde(default = "default_true")]
    pub enable_l3_queue_drain_evacuation: bool,
    /// Configurable Level-3 MBO Queue Drain Forecaster for resting quote evacuation.
    #[serde(default)]
    pub l3_drain_forecaster: Option<arbitrageur_math::l3_hazard::QueueDrainHazardForecaster>,
    /// Whether real-time Queue Priority Tracking & Hazard Evacuation is active. Default: true.
    #[serde(default = "default_true")]
    pub enable_queue_priority_tracking: bool,
    /// Minimum acceptable queue seniority ratio before flagging stagnation. Default: Some(0.20).
    #[serde(default = "default_min_queue_seniority_ratio")]
    pub min_queue_seniority_ratio: Option<f64>,
    /// Pick-off hazard index threshold triggering proactive quote withdrawal. Default: Some(0.70).
    #[serde(default = "default_queue_hazard_pickoff_threshold")]
    pub queue_hazard_pickoff_threshold: Option<f64>,
    /// Configurable Queue Priority Hazard Engine.
    #[serde(default)]
    pub queue_priority_hazard_engine:
        Option<arbitrageur_math::queue_priority::QueuePriorityHazardEngine>,
    /// Optional Bouchaud Power-Law Propagator for self-impact filtering and optimal U-shaped slicing.
    #[serde(default)]
    pub power_law_propagator:
        Option<arbitrageur_math::power_law_propagator::PowerLawPropagatorModel>,
    /// Whether real-time Multi-Level OFI Toxicity Shading is active. Default: true.
    #[serde(default = "default_true")]
    pub enable_ofi_toxicity_shading: bool,
    /// Configurable Multi-Level OFI Toxicity Engine.
    #[serde(default)]
    pub ofi_toxicity_engine:
        Option<arbitrageur_math::micro_price_surface::MultiLevelOfiToxicityEngine>,
    /// Whether Multi-Level Hawkes Queue Drain Forecaster is active. Default: true.
    #[serde(default = "default_true")]
    pub enable_hawkes_queue_drain_evacuation: bool,
    /// Configurable Multi-Level Hawkes Queue Drain Forecaster for predictive quote evacuation.
    #[serde(default)]
    pub hawkes_drain_forecaster:
        Option<arbitrageur_math::cross_hawkes::MultiLevelHawkesQueueDrainForecaster>,
}

impl Default for PassiveUnwindConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            spread_percentile: default_spread_percentile(),
            timeout_secs: default_timeout_secs(),
            max_tries: default_max_tries(),
            max_spread_bps: default_max_spread_bps(),
            max_fast_vol_bps: default_max_fast_vol_bps(),
            max_adverse_drift_bps: default_max_adverse_drift_bps(),
            enable_l3_queue_drain_evacuation: true,
            l3_drain_forecaster: None,
            enable_queue_priority_tracking: true,
            min_queue_seniority_ratio: default_min_queue_seniority_ratio(),
            queue_hazard_pickoff_threshold: default_queue_hazard_pickoff_threshold(),
            queue_priority_hazard_engine: None,
            power_law_propagator: None,
            enable_ofi_toxicity_shading: true,
            ofi_toxicity_engine: None,
            enable_hawkes_queue_drain_evacuation: true,
            hawkes_drain_forecaster: None,
        }
    }
}

impl PassiveUnwindConfig {
    /// Fail-closed validation: any out-of-range field rejects the whole
    /// record (mirrors `SpatialExitConfig::validate` returning `String`).
    pub fn validate(&self) -> Result<(), String> {
        if !self.spread_percentile.is_finite()
            || self.spread_percentile < 0.0
            || self.spread_percentile > 100.0
        {
            return Err(format!(
                "spread_percentile {} must be finite within [0, 100]",
                self.spread_percentile
            ));
        }
        if self.timeout_secs > 300 {
            return Err(format!(
                "timeout_secs {} must be within 0..=300",
                self.timeout_secs
            ));
        }
        if !(1..=10).contains(&self.max_tries) {
            return Err(format!(
                "max_tries {} must be within 1..=10",
                self.max_tries
            ));
        }
        if !self.max_spread_bps.is_finite() || self.max_spread_bps <= 0.0 {
            return Err("max_spread_bps must be finite and positive".to_string());
        }
        if !self.max_fast_vol_bps.is_finite() || self.max_fast_vol_bps <= 0.0 {
            return Err("max_fast_vol_bps must be finite and positive".to_string());
        }
        if let Some(drift) = self.max_adverse_drift_bps {
            if !drift.is_finite() || drift <= 0.0 {
                return Err("max_adverse_drift_bps must be finite and positive".to_string());
            }
        }
        Ok(())
    }
}

/// Decimal → f64 through the shortest string rendering (codebase convention:
/// avoids `from_f64_retain` binary-expansion dust). `None` on garbage.
fn dec_to_f64_opt(value: Decimal) -> Option<f64> {
    let f: f64 = value.to_string().parse().ok()?;
    if f.is_finite() {
        Some(f)
    } else {
        None
    }
}

/// f64 → Decimal through the shortest string rendering. `None` on garbage.
fn f64_to_dec_opt(value: f64) -> Option<Decimal> {
    if !value.is_finite() {
        return None;
    }
    value.to_string().parse().ok()
}

/// Live top-of-book spread in bps. `None` when the book is uncrossable
/// (missing/ nonpositive touch or crossed quotes) — callers fail closed.
pub fn spread_bps(bid: Decimal, ask: Decimal) -> Option<f64> {
    let bid_f = dec_to_f64_opt(bid)?;
    let ask_f = dec_to_f64_opt(ask)?;
    if !(bid_f > 0.0 && ask_f > 0.0 && ask_f >= bid_f) {
        return None;
    }
    let mid = (bid_f + ask_f) / 2.0;
    if mid <= 0.0 {
        return None;
    }
    Some((ask_f - bid_f) / mid * 10_000.0)
}

/// Peg a reduce-limit price inside the spread. `percentile` is measured from
/// the aggressive touch: 0 = far side (buy→ask, sell→bid), 50 = mid,
/// 100 = passive edge (buy→bid, sell→ask). `None` on invalid inputs
/// (bad percentile, missing/nonpositive/crossed touch) — fail closed.
pub fn limit_price_for(side: Side, bid: Decimal, ask: Decimal, percentile: f64) -> Option<Decimal> {
    if !percentile.is_finite() || !(0.0..=100.0).contains(&percentile) {
        return None;
    }
    let bid_f = dec_to_f64_opt(bid)?;
    let ask_f = dec_to_f64_opt(ask)?;
    if !(bid_f > 0.0 && ask_f > 0.0 && ask_f >= bid_f) {
        return None;
    }
    let width = ask_f - bid_f;
    let inset = width * (percentile / 100.0);
    let px = match side {
        Side::Buy => ask_f - inset,
        Side::Sell => bid_f + inset,
    };
    let px_dec = f64_to_dec_opt(px)?;
    if px_dec <= Decimal::ZERO {
        return None;
    }
    Some(px_dec)
}

/// Peg a reduce-limit price inside the spread with Hawkes self-exciting arrival intensity adjustments.
/// If a toxic surge is detected on the adverse stream, pulls the quote immediately (`None`).
/// Otherwise, widens the peg percentile logarithmically deeper into the book to avoid adverse selection fills.
pub fn limit_price_for_hawkes(
    side: Side,
    bid: Decimal,
    ask: Decimal,
    base_percentile: f64,
    hawkes: Option<&arbitrageur_math::hawkes::MultivariateHawkes>,
    stream_idx: usize,
) -> Option<Decimal> {
    if let Some(h) = hawkes {
        // If toxic surge is detected on this stream, pull quote immediately
        if h.is_toxic_pull(stream_idx, 15.0) {
            tracing::warn!(
                "Passive unwind limit quote pulled: Hawkes toxic flow surge detected on stream {stream_idx}"
            );
            return None;
        }
    }

    let effective_percentile = if let Some(h) = hawkes {
        let mu = if stream_idx < h.mu.len() {
            h.mu[stream_idx].max(1e-6)
        } else {
            0.1
        };
        let lambda = h.current_intensity(stream_idx);
        let intensity_mult = (lambda / mu).max(1.0);
        // During arrival bursts, widen maker peg deeper toward passive edge (100.0)
        (base_percentile + 15.0 * intensity_mult.ln()).clamp(10.0, 95.0)
    } else {
        base_percentile
    };

    limit_price_for(side, bid, ask, effective_percentile)
}

/// Peg a reduce-limit price inside the spread with Multi-Level Hawkes Queue Drain Forecaster.
/// If the forecaster indicates evacuation (`should_evacuate_passive_quote == true`), pulls the quote immediately (`None`).
/// Otherwise, delegates to `limit_price_for_hawkes`.
#[allow(clippy::too_many_arguments)]
pub fn limit_price_for_hawkes_with_drain(
    side: Side,
    bid: Decimal,
    ask: Decimal,
    base_percentile: f64,
    hawkes: Option<&arbitrageur_math::hawkes::MultivariateHawkes>,
    stream_idx: usize,
    drain_forecaster: Option<&arbitrageur_math::cross_hawkes::MultiLevelHawkesQueueDrainForecaster>,
    v_ahead: f64,
    v_total: f64,
) -> Option<Decimal> {
    if let Some(forecaster) = drain_forecaster {
        let verdict = forecaster.evaluate_drain_hazard(v_ahead, v_total, 0.05);
        if verdict.should_evacuate_passive_quote {
            tracing::warn!(
                "Passive unwind limit quote pulled: Multi-level Hawkes queue drain hazard: {}",
                verdict.reason
            );
            return None;
        }
    }
    limit_price_for_hawkes(side, bid, ask, base_percentile, hawkes, stream_idx)
}

/// Peg a reduce-limit price using Guéant-Tapia-Manziadi optimal market making with perpetual funding carry.
/// Evaluates closed-form optimal half-spreads under finite remaining unwind inventory $q$,
/// perpetual funding drift $\phi_t$, and order flow imbalance adverse selection.
pub fn limit_price_for_gueant(
    side: Side,
    bid: Decimal,
    ask: Decimal,
    remaining_inventory_q: f64,
    funding_rate_phi: f64,
    ofi_delta: Option<f64>,
    gtm_model: Option<&arbitrageur_math::gueant_mm::GueantPerpMarketMaker>,
) -> Option<Decimal> {
    let bid_f = dec_to_f64_opt(bid)?;
    let ask_f = dec_to_f64_opt(ask)?;
    if !(bid_f > 0.0 && ask_f > 0.0 && ask_f >= bid_f) {
        return None;
    }
    let mid_f = (bid_f + ask_f) * 0.5;
    let default_model = arbitrageur_math::gueant_mm::GueantPerpMarketMaker::default();
    let model = gtm_model.unwrap_or(&default_model);

    // Unwind horizon: e.g. 5 minutes in year units (5.0 / (365.25 * 24.0 * 60.0))
    let tau = 5.0 / 525_960.0;
    let tick_size = (ask_f - bid_f).max(0.01) * 0.1;

    let rec = model.generate_quote_recommendation(
        mid_f,
        remaining_inventory_q,
        tau,
        funding_rate_phi,
        ofi_delta,
        tick_size,
    );

    let raw_px = match side {
        Side::Buy => {
            // Unwinding short position by buying: place bid inside [bid_f, ask_f]
            rec.recommended_bid.clamp(bid_f, ask_f)
        }
        Side::Sell => {
            // Unwinding long position by selling: place ask inside [bid_f, ask_f]
            rec.recommended_ask.clamp(bid_f, ask_f)
        }
    };

    let px_dec = f64_to_dec_opt(raw_px)?;
    if px_dec <= Decimal::ZERO {
        return None;
    }
    Some(px_dec)
}

/// Evaluates adaptive stochastic depth unwind pacing for partial fill remainders and limit attempts.
pub fn evaluate_stochastic_depth_unwind_pacing(
    remaining_qty: Decimal,
    instantaneous_depth: Option<Decimal>,
    horizon_sec: f64,
    executor: Option<&arbitrageur_math::stochastic_depth::StochasticDepthLiquidationExecutor>,
) -> arbitrageur_math::stochastic_depth::ExecutionPacingDecision {
    let rem_f = dec_to_f64_opt(remaining_qty).unwrap_or(0.0);
    let default_exec =
        arbitrageur_math::stochastic_depth::StochasticDepthLiquidationExecutor::default();
    let exec = executor.unwrap_or(&default_exec);

    let depth_f = instantaneous_depth
        .and_then(dec_to_f64_opt)
        .unwrap_or(exec.base_depth_lambda_bar);

    exec.evaluate_pacing_decision(rem_f, horizon_sec, depth_f)
}

/// Calm-market gate: passive only when the spread is within bounds and fast
/// vol (when known) is within bounds. Unknown fast vol passes its leg — the
/// spread leg remains the hard guard.
pub fn calm_gate_ok(
    live_spread_bps: f64,
    fast_vol_bps: Option<f64>,
    cfg: &PassiveUnwindConfig,
) -> bool {
    if !live_spread_bps.is_finite() || live_spread_bps > cfg.max_spread_bps {
        return false;
    }
    match fast_vol_bps {
        Some(v) => v.is_finite() && v <= cfg.max_fast_vol_bps,
        None => true,
    }
}

/// Build a reduce-only limit order at an explicit price. `reduce_only`
/// comes from `ExchangeAdapter::supports_reduce_only` — spot instruments
/// have no position model so the flag must stay unset there.
pub fn build_limit_reduce_order(
    venue: VenueId,
    symbol: &str,
    side: Side,
    qty: Decimal,
    price: Decimal,
    reduce_only: bool,
) -> OrderRequest {
    let mut order = OrderRequest::new_limit(venue, symbol, side, price, qty);
    order.reduce_only = reduce_only;
    order
}

/// Build a reduce-only market/IOC fallback order (same semantics as the
/// pre-existing flatten path).
pub fn build_market_fallback_order(
    venue: VenueId,
    symbol: &str,
    side: Side,
    qty: Decimal,
    reduce_only: bool,
) -> OrderRequest {
    let mut order = OrderRequest::new_market(venue, symbol, side, qty);
    order.reduce_only = reduce_only;
    order
}

/// Classification of one limit attempt's response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitAttempt {
    /// Nothing filled (sim `Rejected` on an untouched book).
    Untouched,
    /// Fully filled the requested quantity.
    Full,
    /// Nonzero fill short of the request (`PartiallyFilled`).
    Partial,
}

pub fn classify_limit_fill(resp: &OrderResponse) -> LimitAttempt {
    if resp.filled_qty <= Decimal::ZERO {
        LimitAttempt::Untouched
    } else if resp.filled_qty >= resp.original_qty {
        LimitAttempt::Full
    } else {
        LimitAttempt::Partial
    }
}

/// Place one reduce-only limit attempt. Transport errors propagate — the
/// driver treats them as retryable until tries run out, then falls back.
pub async fn attempt_limit_once(
    paper: &Arc<dyn ExchangeAdapter>,
    symbol: &str,
    side: Side,
    qty: Decimal,
    price: Decimal,
) -> anyhow::Result<(LimitAttempt, OrderResponse)> {
    let order = build_limit_reduce_order(
        paper.venue_id(),
        symbol,
        side,
        qty,
        price,
        paper.supports_reduce_only(symbol),
    );
    let resp = paper.place_order(order).await?;
    Ok((classify_limit_fill(&resp), resp))
}

/// Fresh-book source for the retry loop: each attempt re-prices from a newly
/// fetched book (never reuses a stale peg).
#[async_trait::async_trait]
pub trait PassiveBookRefresh: Send + Sync {
    async fn fresh_book(&self) -> anyhow::Result<OrderBook>;
}

/// How a passive reduce resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PassiveOutcome {
    /// Limit filled the full quantity.
    LimitFilled,
    /// Limit partially filled; the remainder was taken via market fallback
    /// in the same call (combined record).
    LimitPartialThenMarket,
    /// Fell back to a market/IOC exit. `reason` always explains why.
    MarketFallback { reason: String },
    /// Stopped without stacking: a resting order's cancel/fill state could
    /// not be confirmed terminal, so no further order was dispatched. The
    /// combined `response` carries the still-working order's identity —
    /// callers must persist it and leave the position open for the
    /// reconciliation supervisor to settle.
    Unresolved { reason: String },
}

/// Full report of a passive reduce: outcome, per-try limit prices (equal to
/// the number of limit attempts — empty when no limit was placed), and the
/// authoritative fill response.
#[derive(Debug, Clone)]
pub struct PassiveReduceReport {
    pub outcome: PassiveOutcome,
    pub limit_tries: u32,
    pub limit_prices: Vec<Decimal>,
    pub response: OrderResponse,
}

async fn market_fallback(
    paper: &Arc<dyn ExchangeAdapter>,
    symbol: &str,
    side: Side,
    qty: Decimal,
    reason: String,
    limit_tries: u32,
    limit_prices: Vec<Decimal>,
) -> anyhow::Result<PassiveReduceReport> {
    let order = build_market_fallback_order(
        paper.venue_id(),
        symbol,
        side,
        qty,
        paper.supports_reduce_only(symbol),
    );
    let response = paper.place_order(order).await?;
    Ok(PassiveReduceReport {
        outcome: PassiveOutcome::MarketFallback { reason },
        limit_tries,
        limit_prices,
        response,
    })
}

/// Execute a reduce with passive limit attempts and a market fallback.
///
/// Fail-closed contract: returns `Err` only when no fill record exists at
/// all (config invalid, bad qty, or the fallback placement itself errored).
/// Any placed fill — limit or market — is returned as `Ok(report)`; partial
/// fills are surfaced in the response exactly as the venue reported them.
/// Fresh book → calm gates → pegged limit price for one attempt. `Err`
/// carries the human-readable reason the caller should fall back to
/// market with; `Ok(price)` is the validated peg.
async fn peg_for_attempt(
    refresher: &dyn PassiveBookRefresh,
    side: Side,
    cfg: &PassiveUnwindConfig,
    fast_vol_bps: Option<f64>,
) -> Result<Decimal, String> {
    let book = refresher
        .fresh_book()
        .await
        .map_err(|e| format!("book refresh failed: {e:#}"))?;
    let (Some(bid), Some(ask)) = (book.best_bid_price(), book.best_ask_price()) else {
        return Err("book missing touch".to_string());
    };
    let spread = spread_bps(bid, ask).ok_or_else(|| "uncrossable book".to_string())?;
    if !calm_gate_ok(spread, fast_vol_bps, cfg) {
        return Err(format!("calm gate failed (spread {spread:.2} bps)"));
    }
    if cfg.enable_ofi_toxicity_shading {
        if let Some(mut engine) = cfg.ofi_toxicity_engine {
            let levels: Vec<arbitrageur_math::micro_price_surface::DepthLevelQuote> = (0..5)
                .filter_map(|i| {
                    let b = book.bids.get(i)?;
                    let a = book.asks.get(i)?;
                    Some(arbitrageur_math::micro_price_surface::DepthLevelQuote::new(
                        b.price.to_string().parse().ok()?,
                        b.size.to_string().parse().ok()?,
                        a.price.to_string().parse().ok()?,
                        a.size.to_string().parse().ok()?,
                    ))
                })
                .collect();
            if !levels.is_empty() {
                let mid_f64 = book
                    .mid_price()
                    .unwrap_or(bid)
                    .to_string()
                    .parse::<f64>()
                    .unwrap_or(0.0);
                let eval = engine.update(&levels, mid_f64, 0.0, 0.50, 1.0);
                if eval.should_evacuate_passive_quotes {
                    let toxic_side = match eval.regime {
                        arbitrageur_math::micro_price_surface::OfiToxicityRegime::ToxicBidSurge => {
                            Side::Sell
                        }
                        arbitrageur_math::micro_price_surface::OfiToxicityRegime::ToxicAskSurge => {
                            Side::Buy
                        }
                        _ => side,
                    };
                    if side == toxic_side {
                        return Err(format!(
                            "OFI toxic adverse flow surge detected ({:?}, metric={:.4}); aborting passive quote",
                            eval.regime, eval.toxicity_metric
                        ));
                    }
                }
            }
        }
    }
    if cfg.enable_hawkes_queue_drain_evacuation {
        if let Some(ref forecaster) = cfg.hawkes_drain_forecaster {
            let (ahead, total) = match side {
                Side::Buy => {
                    let a = book
                        .bids
                        .first()
                        .and_then(|l| l.size.to_string().parse::<f64>().ok())
                        .unwrap_or(1.0);
                    let t: f64 = book
                        .bids
                        .iter()
                        .take(5)
                        .filter_map(|l| l.size.to_string().parse::<f64>().ok())
                        .sum();
                    (a, t.max(1.0))
                }
                Side::Sell => {
                    let a = book
                        .asks
                        .first()
                        .and_then(|l| l.size.to_string().parse::<f64>().ok())
                        .unwrap_or(1.0);
                    let t: f64 = book
                        .asks
                        .iter()
                        .take(5)
                        .filter_map(|l| l.size.to_string().parse::<f64>().ok())
                        .sum();
                    (a, t.max(1.0))
                }
            };
            let verdict = forecaster.evaluate_drain_hazard(ahead, total, 0.05);
            if verdict.should_evacuate_passive_quote {
                return Err(format!(
                    "Multi-level Hawkes queue drain hazard ({}); aborting passive quote placement",
                    verdict.reason
                ));
            }
        }
    }
    limit_price_for(side, bid, ask, cfg.spread_percentile)
        .ok_or_else(|| "limit pricing failed".to_string())
}

pub async fn execute_passive_reduce(
    paper: &Arc<dyn ExchangeAdapter>,
    refresher: &dyn PassiveBookRefresh,
    symbol: &str,
    side: Side,
    qty: Decimal,
    cfg: &PassiveUnwindConfig,
    fast_vol_bps: Option<f64>,
) -> anyhow::Result<PassiveReduceReport> {
    cfg.validate().map_err(anyhow::Error::msg)?;
    if qty <= Decimal::ZERO {
        anyhow::bail!("passive reduce quantity must be positive");
    }
    if !cfg.enabled {
        return market_fallback(
            paper,
            symbol,
            side,
            qty,
            "passive unwind disabled".to_string(),
            0,
            Vec::new(),
        )
        .await;
    }

    let mut limit_prices: Vec<Decimal> = Vec::new();
    let mut tries: u32 = 0;
    while tries < cfg.max_tries {
        let price = match peg_for_attempt(refresher, side, cfg, fast_vol_bps).await {
            Ok(p) => p,
            Err(reason) => {
                return market_fallback(paper, symbol, side, qty, reason, tries, limit_prices)
                    .await;
            }
        };
        tries += 1;
        limit_prices.push(price);
        match attempt_limit_once(paper, symbol, side, qty, price).await {
            Ok((LimitAttempt::Full, resp)) => {
                return Ok(PassiveReduceReport {
                    outcome: PassiveOutcome::LimitFilled,
                    limit_tries: tries,
                    limit_prices,
                    response: resp,
                });
            }
            Ok((LimitAttempt::Partial, resp)) => {
                let remaining = resp.remaining_qty;
                if remaining <= Decimal::ZERO {
                    return Ok(PassiveReduceReport {
                        outcome: PassiveOutcome::LimitFilled,
                        limit_tries: tries,
                        limit_prices,
                        response: resp,
                    });
                }
                // Apply adaptive stochastic depth pacing: if a critical depth vacuum is detected,
                // pause briefly for resilience beta to replenish the top-of-book before market sweep.
                let pacing = evaluate_stochastic_depth_unwind_pacing(remaining, None, 10.0, None);
                if let arbitrageur_math::stochastic_depth::ExecutionPacingDecision::PauseReplenishmentWait { recommended_wait_sec, .. } = pacing {
                    tracing::warn!(
                        "Partial limit fill: stochastic depth vacuum detected. Pausing {recommended_wait_sec:.2}s for orderbook resilience replenishment"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(
                        (recommended_wait_sec * 1000.0).clamp(50.0, 500.0) as u64,
                    )).await;
                }
                // Take the remainder at market: a half-closed
                // reduce must not be left working. Either failure below
                // still returns the partial limit fill — the Err contract
                // reserves Err for "no fill record exists at all".
                let mkt = build_market_fallback_order(
                    paper.venue_id(),
                    symbol,
                    side,
                    remaining,
                    paper.supports_reduce_only(symbol),
                );
                let mkt_resp = match paper.place_order(mkt).await {
                    Ok(r) => r,
                    Err(e) => {
                        return Ok(PassiveReduceReport {
                            outcome: PassiveOutcome::MarketFallback {
                                reason: format!(
                                    "market remainder take failed after partial limit fill: {e:#}"
                                ),
                            },
                            limit_tries: tries,
                            limit_prices,
                            response: resp,
                        });
                    }
                };
                let combined = match combine_partial_with_market(&resp, &mkt_resp) {
                    Ok(c) => c,
                    Err(e) => {
                        return Ok(PassiveReduceReport {
                            outcome: PassiveOutcome::MarketFallback {
                                reason: format!(
                                    "partial limit fill kept; remainder blend failed: {e:#}"
                                ),
                            },
                            limit_tries: tries,
                            limit_prices,
                            response: resp,
                        });
                    }
                };
                return Ok(PassiveReduceReport {
                    outcome: PassiveOutcome::LimitPartialThenMarket,
                    limit_tries: tries,
                    limit_prices,
                    response: combined,
                });
            }
            Ok((LimitAttempt::Untouched, _)) => {
                if tries < cfg.max_tries {
                    tokio::time::sleep(std::time::Duration::from_secs(cfg.timeout_secs)).await;
                    continue;
                }
                return market_fallback(
                    paper,
                    symbol,
                    side,
                    qty,
                    format!("unfilled after {tries} limit tries"),
                    tries,
                    limit_prices,
                )
                .await;
            }
            Err(e) => {
                if tries < cfg.max_tries {
                    tokio::time::sleep(std::time::Duration::from_secs(cfg.timeout_secs)).await;
                    continue;
                }
                return market_fallback(
                    paper,
                    symbol,
                    side,
                    qty,
                    format!("limit attempts errored: {e:#}"),
                    tries,
                    limit_prices,
                )
                .await;
            }
        }
    }
    market_fallback(
        paper,
        symbol,
        side,
        qty,
        "limit tries exhausted".to_string(),
        tries,
        limit_prices,
    )
    .await
}

/// Live-venue driver — same peg/timeout/fallback policy as the sim path,
/// but a live limit order *rests*: venues (Lighter especially) can return
/// `New`/zero-filled on submission while the order works. Each attempt
/// therefore polls `fetch_order` until the order is filled, terminal, or
/// `timeout_secs` elapses, then cancels the resting order and confirms a
/// terminal state before re-pegging. When the cancel/fill state cannot be
/// verified the loop stops and market-closes only the *remaining* qty —
/// a second resting reduce order is never stacked behind an unverified
/// one. The aggregate `response` combines every attempt's fills
/// (qty-weighted avg price, summed fees, last order id) so callers record
/// one honest fill record.
pub async fn execute_passive_reduce_live(
    adapter: &Arc<dyn ExchangeAdapter>,
    refresher: &dyn PassiveBookRefresh,
    symbol: &str,
    side: Side,
    qty: Decimal,
    cfg: &PassiveUnwindConfig,
    fast_vol_bps: Option<f64>,
) -> anyhow::Result<PassiveReduceReport> {
    cfg.validate().map_err(anyhow::Error::msg)?;
    if qty <= Decimal::ZERO {
        anyhow::bail!("passive reduce quantity must be positive");
    }
    let venue = adapter.venue_id();
    if !cfg.enabled {
        return market_fallback(
            adapter,
            symbol,
            side,
            qty,
            "passive unwind disabled".to_string(),
            0,
            Vec::new(),
        )
        .await;
    }

    let mut limit_prices: Vec<Decimal> = Vec::new();
    let mut tries: u32 = 0;
    let mut attempts: Vec<OrderResponse> = Vec::new();
    let mut filled_total = Decimal::ZERO;

    while tries < cfg.max_tries && filled_total < qty {
        let price = match peg_for_attempt(refresher, side, cfg, fast_vol_bps).await {
            Ok(p) => p,
            Err(reason) => {
                return finish_live(
                    adapter,
                    symbol,
                    side,
                    qty,
                    filled_total,
                    reason,
                    tries,
                    limit_prices,
                    attempts,
                )
                .await;
            }
        };
        tries += 1;
        limit_prices.push(price);
        let order = build_limit_reduce_order(
            venue.clone(),
            symbol,
            side,
            qty - filled_total,
            price,
            adapter.supports_reduce_only(symbol),
        );
        let mut cur = match adapter.place_order(order).await {
            Ok(r) => r,
            Err(e) => {
                if tries < cfg.max_tries {
                    tokio::time::sleep(std::time::Duration::from_secs(cfg.timeout_secs)).await;
                    continue;
                }
                return finish_live(
                    adapter,
                    symbol,
                    side,
                    qty,
                    filled_total,
                    format!("limit attempts errored: {e:#}"),
                    tries,
                    limit_prices,
                    attempts,
                )
                .await;
            }
        };

        // Poll authoritative order state until terminal, fully filled, or
        // the attempt's timeout expires. Missed polls are tolerated (the
        // resting order may take a beat to index) but bounded.
        let deadline =
            tokio::time::Instant::now() + std::time::Duration::from_secs(cfg.timeout_secs);
        let attempt_start = std::time::Instant::now();
        let mut poll_misses = 0u32;
        loop {
            if cur.status.is_final() || cur.filled_qty >= cur.original_qty {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(750)).await;
            match adapter
                .fetch_order(symbol, &cur.order_id, Some(&cur.client_order_id))
                .await
            {
                Ok(polled) => {
                    cur = polled;
                    poll_misses = 0;
                }
                Err(_) => {
                    poll_misses += 1;
                    if poll_misses >= 8 {
                        break;
                    }
                }
            }

            // Microstructure Latency Racing & Limit Cancellation Drift Check:
            // If live market mid drifts adversely by more than the threshold (default 5.0 bps),
            // immediately break out of the resting loop to trigger priority cancellation before
            // HFT taker snipers sweep the stale quote.
            if !cur.status.is_final() && cur.filled_qty < cur.original_qty {
                if let Some(drift_threshold_bps) = cfg.max_adverse_drift_bps {
                    let maybe_mid = if let Ok(book) = refresher.fresh_book().await {
                        book.mid_price()
                    } else if let Ok(ticker) = adapter.fetch_ticker(symbol).await {
                        if ticker.ask > Decimal::ZERO && ticker.bid > Decimal::ZERO {
                            Some((ticker.ask + ticker.bid) / Decimal::from(2))
                        } else {
                            Some(ticker.last)
                        }
                    } else {
                        None
                    };

                    if let Some(current_mid) = maybe_mid {
                        let limit_f64: f64 = price.to_string().parse().unwrap_or(0.0);
                        let mid_f64: f64 = current_mid.to_string().parse().unwrap_or(0.0);
                        let side_str = match side {
                            Side::Buy => "buy",
                            Side::Sell => "sell",
                        };
                        let raw_adverse_drift_bps =
                            arbitrageur_math::latency_racing::AsymmetricLatencyRacingEngine::adverse_drift_bps(
                                side_str, limit_f64, mid_f64,
                            );
                        let adverse_drift_bps = if let Some(propagator) = cfg.power_law_propagator {
                            let filled_f64: f64 = cur.filled_qty.to_string().parse().unwrap_or(0.0);
                            if filled_f64 > 0.0 {
                                let elapsed_sec = attempt_start.elapsed().as_secs_f64();
                                let fills = [(0.0, filled_f64)];
                                propagator.filter_self_impact(
                                    raw_adverse_drift_bps,
                                    elapsed_sec,
                                    &fills,
                                    mid_f64,
                                )
                            } else {
                                raw_adverse_drift_bps
                            }
                        } else {
                            raw_adverse_drift_bps
                        };
                        if adverse_drift_bps >= drift_threshold_bps {
                            tracing::warn!(
                                "passive unwind {venue} {symbol}: adverse mid drift {adverse_drift_bps:.2} bps (raw: {raw_adverse_drift_bps:.2} bps) >= threshold {drift_threshold_bps:.2} bps; triggering priority cancellation race protection"
                            );
                            break;
                        }
                    }

                    // Level-3 MBO Queue Drain Hazard Protection
                    if cfg.enable_l3_queue_drain_evacuation {
                        if let Ok(book) = adapter.fetch_orderbook(symbol).await {
                            let forecaster = cfg.l3_drain_forecaster.unwrap_or_default();
                            let (vol_ahead, total_vol) = match side {
                                Side::Buy => {
                                    let mut ahead = Decimal::ZERO;
                                    let mut total = Decimal::ZERO;
                                    for lvl in &book.bids {
                                        if lvl.price > price {
                                            ahead += lvl.size;
                                        }
                                        if lvl.price >= price {
                                            total += lvl.size;
                                        }
                                    }
                                    (ahead, total.max(Decimal::ONE))
                                }
                                Side::Sell => {
                                    let mut ahead = Decimal::ZERO;
                                    let mut total = Decimal::ZERO;
                                    for lvl in &book.asks {
                                        if lvl.price < price {
                                            ahead += lvl.size;
                                        }
                                        if lvl.price <= price {
                                            total += lvl.size;
                                        }
                                    }
                                    (ahead, total.max(Decimal::ONE))
                                }
                            };
                            let (decision, metrics) =
                                forecaster.evaluate_hazard(vol_ahead, total_vol, false);
                            if let arbitrageur_math::l3_hazard::QueueHazardDecision::EvacuatePickoff { ref reason, .. } = decision {
                                tracing::warn!(
                                    "passive unwind {venue} {symbol}: L3 queue drain hazard evacuation triggered: {reason} (tau_drain={:.3}s, hazard={:.2}); aborting resting quote",
                                    metrics.tau_drain_sec, metrics.hazard_index
                                );
                                break;
                            }
                        }
                    }

                    // Deep Order Book Queue Priority Tracking & Microstructure Hazard Protection
                    if cfg.enable_queue_priority_tracking {
                        if let Ok(book) = adapter.fetch_orderbook(symbol).await {
                            let (vol_ahead, total_vol) = match side {
                                Side::Buy => {
                                    let mut ahead = Decimal::ZERO;
                                    let mut total = Decimal::ZERO;
                                    for lvl in &book.bids {
                                        if lvl.price > price {
                                            ahead += lvl.size;
                                        }
                                        if lvl.price >= price {
                                            total += lvl.size;
                                        }
                                    }
                                    (ahead, total.max(Decimal::ONE))
                                }
                                Side::Sell => {
                                    let mut ahead = Decimal::ZERO;
                                    let mut total = Decimal::ZERO;
                                    for lvl in &book.asks {
                                        if lvl.price < price {
                                            ahead += lvl.size;
                                        }
                                        if lvl.price <= price {
                                            total += lvl.size;
                                        }
                                    }
                                    (ahead, total.max(Decimal::ONE))
                                }
                            };
                            let current_mid = book.mid_price().unwrap_or(price);
                            let hazard_engine =
                                cfg.queue_priority_hazard_engine.unwrap_or_default();
                            let mut tracker = crate::queue_priority::QueuePriorityTracker::new(
                                cur.order_id.clone(),
                                price,
                                side,
                                total_vol,
                                qty - filled_total,
                                current_mid,
                            );
                            tracker.q_ahead = vol_ahead;
                            tracker.total_depth = total_vol;
                            let (rec, snapshot) = tracker.evaluate_action(&hazard_engine);
                            match rec {
                                arbitrageur_math::queue_priority::QueueHazardRecommendation::ProactiveQuoteWithdrawal { ref reason, pickoff_hazard, .. } => {
                                    tracing::warn!(
                                        "passive unwind {venue} {symbol}: queue priority pick-off hazard evacuation: {reason} (hazard={:.2}, seniority={:.2}); aborting resting quote",
                                        pickoff_hazard, snapshot.seniority_ratio
                                    );
                                    break;
                                }
                                arbitrageur_math::queue_priority::QueueHazardRecommendation::CancelAndReplace { ref reason, seniority_ratio, .. } => {
                                    tracing::info!(
                                        "passive unwind {venue} {symbol}: queue priority stagnation cancel-replace: {reason} (seniority={:.2}); cancelling to replace",
                                        seniority_ratio
                                    );
                                    break;
                                }
                                arbitrageur_math::queue_priority::QueueHazardRecommendation::MaintainRestingQuote { .. } => {}
                            }
                            if cfg.enable_ofi_toxicity_shading {
                                if let Some(mut engine) = cfg.ofi_toxicity_engine {
                                    let levels: Vec<arbitrageur_math::micro_price_surface::DepthLevelQuote> = (0..5)
                                        .filter_map(|i| {
                                            let b = book.bids.get(i)?;
                                            let a = book.asks.get(i)?;
                                            Some(arbitrageur_math::micro_price_surface::DepthLevelQuote::new(
                                                b.price.to_string().parse().ok()?,
                                                b.size.to_string().parse().ok()?,
                                                a.price.to_string().parse().ok()?,
                                                a.size.to_string().parse().ok()?,
                                            ))
                                        })
                                        .collect();
                                    if !levels.is_empty() {
                                        let mid_f64 =
                                            current_mid.to_string().parse::<f64>().unwrap_or(0.0);
                                        let eval = engine.update(&levels, mid_f64, 0.0, 0.50, 1.0);
                                        if eval.should_evacuate_passive_quotes {
                                            let toxic_side = match eval.regime {
                                                arbitrageur_math::micro_price_surface::OfiToxicityRegime::ToxicBidSurge => Side::Sell,
                                                arbitrageur_math::micro_price_surface::OfiToxicityRegime::ToxicAskSurge => Side::Buy,
                                                _ => side,
                                            };
                                            if side == toxic_side {
                                                tracing::warn!(
                                                    "passive unwind {venue} {symbol}: OFI toxic adverse flow surge ({:?}, metric={:.4}); evacuating resting quote",
                                                    eval.regime, eval.toxicity_metric
                                                );
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Multi-Level Hawkes Queue Drain Hazard Evacuation
                    if cfg.enable_hawkes_queue_drain_evacuation {
                        if let Ok(book) = adapter.fetch_orderbook(symbol).await {
                            let (vol_ahead, total_vol) = match side {
                                Side::Buy => {
                                    let mut ahead = Decimal::ZERO;
                                    let mut total = Decimal::ZERO;
                                    for lvl in &book.bids {
                                        if lvl.price > price {
                                            ahead += lvl.size;
                                        }
                                        if lvl.price >= price {
                                            total += lvl.size;
                                        }
                                    }
                                    (ahead, total.max(Decimal::ONE))
                                }
                                Side::Sell => {
                                    let mut ahead = Decimal::ZERO;
                                    let mut total = Decimal::ZERO;
                                    for lvl in &book.asks {
                                        if lvl.price < price {
                                            ahead += lvl.size;
                                        }
                                        if lvl.price <= price {
                                            total += lvl.size;
                                        }
                                    }
                                    (ahead, total.max(Decimal::ONE))
                                }
                            };
                            let forecaster = cfg.hawkes_drain_forecaster.unwrap_or_default();
                            let ahead_f = vol_ahead.to_string().parse::<f64>().unwrap_or(0.0);
                            let total_f = total_vol.to_string().parse::<f64>().unwrap_or(1.0);
                            let verdict = forecaster.evaluate_drain_hazard(ahead_f, total_f, 0.05);
                            if verdict.should_evacuate_passive_quote {
                                tracing::warn!(
                                    "passive unwind {venue} {symbol}: Hawkes multi-level queue drain hazard evacuation triggered: {} (pickoff_prob={:.2}, drain_add_ratio={:.2}); aborting resting quote",
                                    verdict.reason,
                                    verdict.pickoff_probability,
                                    verdict.drain_to_add_ratio
                                );
                                break;
                            }
                        }
                    }
                }
            }
        }

        // Still working (or state unverifiable) at timeout: cancel and
        // confirm terminal before another attempt may rest on the book.
        // If terminal state cannot be confirmed, STOP — a market fallback
        // here would stack a second reduce order on top of one that may
        // still be live, flipping the position past flat. The caller gets
        // `Unresolved` with the outstanding order's identity preserved.
        if !cur.status.is_final() && cur.filled_qty < cur.original_qty {
            if let Err(e) = adapter
                .cancel_order(CancelRequest {
                    venue: venue.clone(),
                    symbol: symbol.to_string(),
                    order_id: Some(cur.order_id.clone()).filter(|s| !s.is_empty()),
                    client_order_id: Some(cur.client_order_id.clone()).filter(|s| !s.is_empty()),
                })
                .await
            {
                tracing::warn!(
                    "passive unwind {venue} {symbol}: cancel request failed for order {}: {e:#}; verifying terminal state",
                    cur.order_id
                );
            }
            // Allow matching engine replication window before fetching terminal order state
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            match adapter
                .fetch_order(symbol, &cur.order_id, Some(&cur.client_order_id))
                .await
            {
                Ok(p) => cur = p,
                Err(e) => {
                    let working_id = cur.order_id.clone();
                    attempts.push(cur);
                    return Ok(PassiveReduceReport {
                        outcome: PassiveOutcome::Unresolved {
                            reason: format!(
                                "cancel/fill state unverifiable — order {working_id} may still be working ({e:#})"
                            ),
                        },
                        limit_tries: tries,
                        limit_prices,
                        response: combine_live_attempts(
                            adapter.venue_id(),
                            symbol,
                            qty,
                            &attempts,
                        ),
                    });
                }
            }
            if !cur.status.is_final() {
                let working_id = cur.order_id.clone();
                attempts.push(cur);
                return Ok(PassiveReduceReport {
                    outcome: PassiveOutcome::Unresolved {
                        reason: format!(
                            "cancel unconfirmed — order {working_id} may still be working; refusing to stack a second resting reduce order"
                        ),
                    },
                    limit_tries: tries,
                    limit_prices,
                    response: combine_live_attempts(adapter.venue_id(), symbol, qty, &attempts),
                });
            }
        }

        let full_wire_fill = cur.status == OrderStatus::Filled
            && cur.remaining_qty <= Decimal::ZERO
            && cur.filled_qty >= cur.original_qty
            && cur.original_qty > Decimal::ZERO;
        filled_total += cur.filled_qty;
        attempts.push(cur);
        if full_wire_fill {
            return Ok(PassiveReduceReport {
                outcome: PassiveOutcome::LimitFilled,
                limit_tries: tries,
                limit_prices,
                response: combine_live_attempts(adapter.venue_id(), symbol, qty, &attempts),
            });
        }
        let rem = (qty - filled_total).max(Decimal::ZERO);
        if rem > Decimal::ZERO
            && filled_total > Decimal::ZERO
            && matches!(adapter.wire_base_qty(symbol, rem).await, Ok(w) if w <= Decimal::ZERO)
        {
            return Ok(PassiveReduceReport {
                outcome: PassiveOutcome::LimitFilled,
                limit_tries: tries,
                limit_prices,
                response: combine_live_attempts(adapter.venue_id(), symbol, qty, &attempts),
            });
        }
    }

    if filled_total >= qty {
        return Ok(PassiveReduceReport {
            outcome: PassiveOutcome::LimitFilled,
            limit_tries: tries,
            limit_prices,
            response: combine_live_attempts(adapter.venue_id(), symbol, qty, &attempts),
        });
    }
    finish_live(
        adapter,
        symbol,
        side,
        qty,
        filled_total,
        format!("unfilled after {tries} limit tries"),
        tries,
        limit_prices,
        attempts,
    )
    .await
}

/// Market-close the remaining qty after the limit loop stops, then fold
/// every attempt's response into one aggregate record. Mirrors the sim
/// path's outcome taxonomy.
#[allow(clippy::too_many_arguments)]
async fn finish_live(
    adapter: &Arc<dyn ExchangeAdapter>,
    symbol: &str,
    side: Side,
    requested: Decimal,
    filled_so_far: Decimal,
    reason: String,
    tries: u32,
    limit_prices: Vec<Decimal>,
    mut attempts: Vec<OrderResponse>,
) -> anyhow::Result<PassiveReduceReport> {
    let remaining = requested - filled_so_far;
    let had_limit_fill = attempts.iter().any(|r| r.filled_qty > Decimal::ZERO);
    let sub_lot_remainder = remaining > Decimal::ZERO
        && had_limit_fill
        && matches!(adapter.wire_base_qty(symbol, remaining).await, Ok(w) if w <= Decimal::ZERO);
    if remaining <= Decimal::ZERO || sub_lot_remainder {
        return Ok(PassiveReduceReport {
            outcome: if had_limit_fill {
                PassiveOutcome::LimitFilled
            } else {
                PassiveOutcome::MarketFallback { reason }
            },
            limit_tries: tries,
            limit_prices,
            response: combine_live_attempts(adapter.venue_id(), symbol, requested, &attempts),
        });
    }
    match adapter
        .place_order(build_market_fallback_order(
            adapter.venue_id(),
            symbol,
            side,
            remaining,
            adapter.supports_reduce_only(symbol),
        ))
        .await
    {
        #[allow(unused_mut)]
        Ok(mut mkt) => {
            #[cfg(not(test))]
            if !mkt.status.is_final() {
                let oid = mkt.order_id.clone();
                let cid = (!mkt.client_order_id.is_empty()).then(|| mkt.client_order_id.clone());
                for _ in 0..5 {
                    tokio::time::sleep(std::time::Duration::from_millis(450)).await;
                    if let Ok(polled) = adapter.fetch_order(symbol, &oid, cid.as_deref()).await {
                        let done = polled.status.is_final();
                        mkt = polled;
                        if done {
                            break;
                        }
                    }
                }
            }
            attempts.push(mkt);
            Ok(PassiveReduceReport {
                outcome: if had_limit_fill {
                    PassiveOutcome::LimitPartialThenMarket
                } else {
                    PassiveOutcome::MarketFallback { reason }
                },
                limit_tries: tries,
                limit_prices,
                response: combine_live_attempts(adapter.venue_id(), symbol, requested, &attempts),
            })
        }
        Err(e) => {
            let resp = combine_live_attempts(adapter.venue_id(), symbol, requested, &attempts);
            Ok(PassiveReduceReport {
                outcome: PassiveOutcome::MarketFallback {
                    reason: format!("{reason}; market fallback errored: {e:#}"),
                },
                limit_tries: tries,
                limit_prices,
                response: resp,
            })
        }
    }
}

/// Combine every live attempt's terminal response into one honest record:
/// summed fills/fees, qty-weighted average price, last order/client id
/// retained for audit. An empty attempt set yields a zero-fill record
/// (callers still need a shaped response to persist the outcome).
fn combine_live_attempts(
    venue: VenueId,
    symbol: &str,
    requested: Decimal,
    attempts: &[OrderResponse],
) -> OrderResponse {
    let filled: Decimal = attempts.iter().map(|r| r.filled_qty).sum();
    // None only when every attempt's fee is venue-unknown — a partial
    // known-fee sum would understate cost, so unknown wins over reported.
    let fee: Option<Decimal> = if attempts.iter().all(|r| r.fee.is_some()) {
        Some(attempts.iter().filter_map(|r| r.fee).sum())
    } else {
        None
    };
    let px_num: Decimal = attempts
        .iter()
        .filter(|r| r.filled_qty > Decimal::ZERO)
        .map(|r| r.avg_fill_price.unwrap_or(Decimal::ZERO) * r.filled_qty)
        .sum();
    let avg = if filled > Decimal::ZERO && px_num > Decimal::ZERO {
        Some(px_num / filled)
    } else {
        None
    };
    let last = attempts.last();
    let status = if filled >= requested && requested > Decimal::ZERO {
        OrderStatus::Filled
    } else if filled > Decimal::ZERO {
        OrderStatus::PartiallyFilled
    } else {
        last.map(|r| r.status).unwrap_or(OrderStatus::New)
    };
    OrderResponse {
        order_id: last.map(|r| r.order_id.clone()).unwrap_or_default(),
        client_order_id: last.map(|r| r.client_order_id.clone()).unwrap_or_default(),
        venue,
        symbol: symbol.to_string(),
        status,
        original_qty: requested,
        filled_qty: filled,
        remaining_qty: (requested - filled).max(Decimal::ZERO),
        avg_fill_price: avg,
        fee,
        fee_currency: last.map(|r| r.fee_currency.clone()).unwrap_or_default(),
        raw_response: None,
        timestamp: chrono::Utc::now(),
    }
}

/// Combine a partial limit fill with its market remainder-take into one
/// honest record: summed qty/fee, quantity-weighted average price, both
/// order ids preserved. `Err` when the pair is incoherent (fail closed —
/// the caller then surfaces the raw responses instead of a blend).
fn combine_partial_with_market(
    limit_resp: &OrderResponse,
    mkt_resp: &OrderResponse,
) -> anyhow::Result<OrderResponse> {
    if limit_resp.filled_qty <= Decimal::ZERO {
        anyhow::bail!("nothing to combine: limit fill is empty");
    }
    if mkt_resp.filled_qty <= Decimal::ZERO {
        anyhow::bail!("remainder take filled nothing");
    }
    let (Some(limit_px), Some(mkt_px)) = (limit_resp.avg_fill_price, mkt_resp.avg_fill_price)
    else {
        anyhow::bail!("cannot blend fills without positive prices");
    };
    if !(limit_px > Decimal::ZERO && mkt_px > Decimal::ZERO) {
        anyhow::bail!("cannot blend fills with nonpositive prices");
    }
    let total_qty = limit_resp.filled_qty + mkt_resp.filled_qty;
    if total_qty <= Decimal::ZERO {
        anyhow::bail!("cannot blend fills with nonpositive quantity");
    }
    let vwap = (limit_resp.filled_qty * limit_px + mkt_resp.filled_qty * mkt_px) / total_qty;
    let status = if mkt_resp.status == OrderStatus::Filled {
        OrderStatus::Filled
    } else {
        OrderStatus::PartiallyFilled
    };
    Ok(OrderResponse {
        order_id: format!("{}+{}", limit_resp.order_id, mkt_resp.order_id),
        client_order_id: limit_resp.client_order_id.clone(),
        venue: limit_resp.venue.clone(),
        symbol: limit_resp.symbol.clone(),
        status,
        original_qty: limit_resp.original_qty,
        filled_qty: total_qty,
        remaining_qty: (limit_resp.original_qty - total_qty).max(Decimal::ZERO),
        avg_fill_price: Some(vwap),
        fee: match (limit_resp.fee, mkt_resp.fee) {
            (Some(a), Some(b)) => Some(a + b),
            _ => None,
        },
        fee_currency: limit_resp.fee_currency.clone(),
        raw_response: None,
        timestamp: chrono::Utc::now(),
    })
}

/// Selection route for unhedged or orphaned position unwinding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UnwindRoute {
    /// Route A: Immediate aggressive IOC market inversion on primary venue.
    RouteAMarketInversion { estimated_cost_usd: f64 },
    /// Route B: Immediate Synthetic Perpetual Delta Lock + passive limit exit.
    RouteBSyntheticPerpLock {
        estimated_cost_usd: f64,
        perp_hedge_symbol: String,
        perp_hedge_qty: f64,
    },
}

/// Evaluator that determines whether an orphaned leg should be unwound via
/// immediate IOC reversal (Route A) or synthetic perpetual delta lock (Route B)
/// using closed-form decision hurdle Delta_unwind* vs basis variance risk.
#[derive(Debug, Clone, Copy)]
pub struct DynamicPartialFillEvaluator {
    pub risk_aversion_gamma: f64,
}

impl Default for DynamicPartialFillEvaluator {
    fn default() -> Self {
        Self {
            risk_aversion_gamma: 1e-6,
        }
    }
}

impl DynamicPartialFillEvaluator {
    pub fn new(gamma: f64) -> Self {
        Self {
            risk_aversion_gamma: gamma.max(1e-12),
        }
    }

    /// Evaluates optimal unwinding route by balancing crossing friction against inventory variance risk:
    /// Delta_unwind* = (Spread_1 + 2 * f_taker_1 + Delta_adverse) - (Spread_perp + 2 * f_taker_perp - f_maker_1)
    /// R_basis = 0.5 * gamma * N^2 * sigma_basis^2 * tau
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_unwind_route(
        &self,
        unhedged_notional_usd: f64,
        leg1_spread_bps: f64,
        leg1_taker_fee_bps: f64,
        leg1_maker_rebate_bps: f64,
        adverse_selection_bps: f64,
        perp_spread_bps: f64,
        perp_taker_fee_bps: f64,
        basis_vol_annual: f64,
        expected_unwind_duration_sec: f64,
        perp_symbol: Option<&str>,
    ) -> UnwindRoute {
        let basis_vol_sec = basis_vol_annual / (365.25 * 86400.0_f64).sqrt();
        let tau = expected_unwind_duration_sec.max(0.1);

        // Route A cost in USD: N * (Spread_1 + 2 * f_taker_1 + Delta_adverse) / 10000
        let cost_a_bps = leg1_spread_bps + 2.0 * leg1_taker_fee_bps + adverse_selection_bps;
        let cost_a_usd = unhedged_notional_usd * (cost_a_bps / 10_000.0);

        // Route B crossing cost in USD: N * (Spread_perp + 2 * f_taker_perp - f_maker_1) / 10000
        let cost_b_crossing_bps =
            perp_spread_bps + 2.0 * perp_taker_fee_bps - leg1_maker_rebate_bps;
        let cost_b_crossing_usd = unhedged_notional_usd * (cost_b_crossing_bps.max(0.0) / 10_000.0);

        // Basis variance penalty: 0.5 * gamma * N^2 * sigma_basis^2 * tau
        let basis_variance_penalty_usd = 0.5
            * self.risk_aversion_gamma
            * unhedged_notional_usd.powi(2)
            * basis_vol_sec.powi(2)
            * tau;

        let total_cost_b_usd = cost_b_crossing_usd + basis_variance_penalty_usd;

        if total_cost_b_usd < cost_a_usd {
            UnwindRoute::RouteBSyntheticPerpLock {
                estimated_cost_usd: total_cost_b_usd,
                perp_hedge_symbol: perp_symbol.unwrap_or("BTC-PERP").to_string(),
                perp_hedge_qty: unhedged_notional_usd,
            }
        } else {
            UnwindRoute::RouteAMarketInversion {
                estimated_cost_usd: cost_a_usd,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrageur_core::types::{dec, VenueId};
    use std::collections::VecDeque;
    use std::sync::Mutex;

    fn test_book(bid: Decimal, ask: Decimal) -> OrderBook {
        let mut b = OrderBook::new(VenueId::new("test"), "BTC-USDC".to_string());
        b.bids
            .push(arbitrageur_core::quote::BookLevel::new(bid, dec!(10)));
        b.asks
            .push(arbitrageur_core::quote::BookLevel::new(ask, dec!(10)));
        b
    }

    /// Scripted paper venue: each `place_order` consumes one scripted fill
    /// (Full / Partial(qty) / Untouched / transport error) and records the
    /// request. Honest limit semantics are the sim's job; here the script
    /// pins the branch under test deterministically.
    struct ScriptedPaper {
        venue: VenueId,
        script: Mutex<VecDeque<ScriptStep>>,
        calls: Mutex<Vec<OrderRequest>>,
    }

    #[derive(Clone)]
    enum ScriptStep {
        Full,
        Partial(Decimal),
        Untouched,
        TransportError,
    }

    impl ScriptedPaper {
        fn new(steps: Vec<ScriptStep>) -> Self {
            Self {
                venue: VenueId::new("test"),
                script: Mutex::new(steps.into()),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn recorded(&self) -> Vec<OrderRequest> {
            self.calls.lock().expect("test mutex").clone()
        }
    }

    #[async_trait::async_trait]
    impl ExchangeAdapter for ScriptedPaper {
        fn venue_id(&self) -> VenueId {
            self.venue.clone()
        }
        fn name(&self) -> &str {
            "scripted-paper"
        }
        async fn fetch_markets(
            &self,
        ) -> anyhow::Result<Vec<arbitrageur_core::market::MarketMetadata>> {
            Ok(vec![])
        }
        async fn fetch_orderbook(&self, _symbol: &str) -> anyhow::Result<OrderBook> {
            Ok(test_book(dec!(100), dec!(101)))
        }
        async fn fetch_ticker(
            &self,
            symbol: &str,
        ) -> anyhow::Result<arbitrageur_core::quote::Ticker> {
            Ok(arbitrageur_core::quote::Ticker {
                venue: self.venue.clone(),
                symbol: symbol.to_string(),
                bid: dec!(100),
                ask: dec!(101),
                last: dec!(100.5),
                timestamp: chrono::Utc::now(),
            })
        }
        async fn fetch_funding_rate(
            &self,
            symbol: &str,
        ) -> anyhow::Result<arbitrageur_core::quote::FundingRate> {
            Ok(arbitrageur_core::quote::FundingRate {
                venue: self.venue.clone(),
                symbol: symbol.to_string(),
                rate: dec!(0.0001),
                predicted_rate: None,
                interval_hours: 8,
                next_funding_time: chrono::Utc::now(),
                timestamp: chrono::Utc::now(),
            })
        }
        async fn fetch_account(
            &self,
        ) -> anyhow::Result<arbitrageur_core::position::AccountSnapshot> {
            Ok(arbitrageur_core::position::AccountSnapshot {
                venue: self.venue.clone(),
                equity: dec!(10000),
                free_margin: dec!(10000),
                maintenance_margin: dec!(0),
                margin_usage_pct: dec!(0),
                balances: vec![],
                positions: vec![],
                timestamp: chrono::Utc::now(),
            })
        }
        async fn place_order(&self, req: OrderRequest) -> anyhow::Result<OrderResponse> {
            let step = self
                .script
                .lock()
                .expect("test mutex")
                .pop_front()
                .unwrap_or(ScriptStep::Untouched);
            self.calls.lock().expect("test mutex").push(req.clone());
            let filled_qty = match step {
                ScriptStep::TransportError => anyhow::bail!("scripted transport failure"),
                ScriptStep::Full => req.quantity,
                ScriptStep::Partial(q) => q.min(req.quantity),
                ScriptStep::Untouched => Decimal::ZERO,
            };
            let remaining_qty = req.quantity - filled_qty;
            let status = if filled_qty >= req.quantity && remaining_qty == Decimal::ZERO {
                OrderStatus::Filled
            } else if filled_qty > Decimal::ZERO {
                OrderStatus::PartiallyFilled
            } else {
                OrderStatus::Rejected
            };
            Ok(OrderResponse {
                order_id: "scripted-1".to_string(),
                client_order_id: req.client_order_id,
                venue: self.venue.clone(),
                symbol: req.symbol,
                status,
                original_qty: req.quantity,
                filled_qty,
                remaining_qty,
                avg_fill_price: if filled_qty > Decimal::ZERO {
                    req.price.or(Some(dec!(100.5)))
                } else {
                    None
                },
                fee: Some(dec!(0.01)),
                fee_currency: "USDC".to_string(),
                raw_response: None,
                timestamp: chrono::Utc::now(),
            })
        }
        async fn cancel_order(
            &self,
            _req: arbitrageur_core::order::CancelRequest,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    /// Scripted refresher: yields one book per attempt so re-pricing is
    /// observable; falls back to repeating the last book.
    struct ScriptedRefresh {
        books: Mutex<VecDeque<OrderBook>>,
        last: OrderBook,
    }

    impl ScriptedRefresh {
        fn new(books: Vec<OrderBook>) -> Self {
            let last = books
                .last()
                .cloned()
                .unwrap_or_else(|| test_book(dec!(100), dec!(101)));
            Self {
                books: Mutex::new(books.into()),
                last,
            }
        }
    }

    #[async_trait::async_trait]
    impl PassiveBookRefresh for ScriptedRefresh {
        async fn fresh_book(&self) -> anyhow::Result<OrderBook> {
            let next = self
                .books
                .lock()
                .expect("test mutex")
                .pop_front()
                .unwrap_or_else(|| self.last.clone());
            Ok(next)
        }
    }

    fn enabled_cfg() -> PassiveUnwindConfig {
        PassiveUnwindConfig {
            enabled: true,
            spread_percentile: 50.0,
            timeout_secs: 0,
            max_tries: 3,
            // Fixture books use a wide 100/101 touch for exact-decimal mids;
            // the production default (15.0) is covered by the gate unit tests.
            max_spread_bps: 200.0,
            max_fast_vol_bps: 10.0,
            max_adverse_drift_bps: Some(5.0),
            enable_l3_queue_drain_evacuation: true,
            l3_drain_forecaster: None,
            enable_queue_priority_tracking: true,
            min_queue_seniority_ratio: Some(0.20),
            queue_hazard_pickoff_threshold: Some(0.70),
            queue_priority_hazard_engine: None,
            power_law_propagator: None,
            enable_ofi_toxicity_shading: true,
            ofi_toxicity_engine: None,
            enable_hawkes_queue_drain_evacuation: true,
            hawkes_drain_forecaster: None,
        }
    }

    #[test]
    fn default_config_is_data_derived_and_valid() {
        let cfg = PassiveUnwindConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.spread_percentile, 50.0);
        assert_eq!(cfg.timeout_secs, 10);
        assert_eq!(cfg.max_tries, 3);
        assert_eq!(cfg.max_spread_bps, 15.0);
        assert_eq!(cfg.max_fast_vol_bps, 10.0);
        cfg.validate().expect("defaults validate");
    }

    #[test]
    fn validate_rejects_garbage_fail_closed() {
        let cfg = PassiveUnwindConfig::default();
        for mutate in [
            |c: &mut PassiveUnwindConfig| c.spread_percentile = -1.0,
            |c: &mut PassiveUnwindConfig| c.spread_percentile = 101.0,
            |c: &mut PassiveUnwindConfig| c.spread_percentile = f64::NAN,
            |c: &mut PassiveUnwindConfig| c.timeout_secs = 301,
            |c: &mut PassiveUnwindConfig| c.max_tries = 0,
            |c: &mut PassiveUnwindConfig| c.max_tries = 11,
            |c: &mut PassiveUnwindConfig| c.max_spread_bps = 0.0,
            |c: &mut PassiveUnwindConfig| c.max_fast_vol_bps = f64::INFINITY,
        ] {
            let mut bad = cfg;
            mutate(&mut bad);
            assert!(bad.validate().is_err(), "must reject {bad:?}");
        }
        cfg.validate().expect("untouched defaults stay valid");
    }

    #[test]
    fn percentile_semantics_buy_and_sell() {
        let bid = dec!(100);
        let ask = dec!(101);
        // 0 = aggressive touch (buy→ask, sell→bid).
        assert_eq!(limit_price_for(Side::Buy, bid, ask, 0.0), Some(dec!(101)));
        assert_eq!(limit_price_for(Side::Sell, bid, ask, 0.0), Some(dec!(100)));
        // 50 = mid.
        assert_eq!(
            limit_price_for(Side::Buy, bid, ask, 50.0),
            Some(dec!(100.5))
        );
        assert_eq!(
            limit_price_for(Side::Sell, bid, ask, 50.0),
            Some(dec!(100.5))
        );
        // 100 = passive edge (buy→bid, sell→ask).
        assert_eq!(limit_price_for(Side::Buy, bid, ask, 100.0), Some(dec!(100)));
        assert_eq!(
            limit_price_for(Side::Sell, bid, ask, 100.0),
            Some(dec!(101))
        );
    }

    #[test]
    fn test_limit_price_for_gueant_unwind() {
        let bid = dec!(50000.0);
        let ask = dec!(50005.0);

        // Long inventory (+5.0): selling to unwind -> ask spread should tighten toward bid
        let sell_px = limit_price_for_gueant(Side::Sell, bid, ask, 5.0, 0.0, None, None);
        assert!(sell_px.is_some());
        let p_sell = sell_px.unwrap();
        assert!(p_sell >= bid && p_sell <= ask);

        // Short inventory (-5.0): buying to unwind -> bid spread should widen toward ask
        let buy_px = limit_price_for_gueant(Side::Buy, bid, ask, -5.0, 0.0, None, None);
        assert!(buy_px.is_some());
        let p_buy = buy_px.unwrap();
        assert!(p_buy >= bid && p_buy <= ask);
    }

    #[test]
    fn pricing_rejects_invalid_inputs() {
        assert_eq!(limit_price_for(Side::Buy, dec!(100), dec!(101), -5.0), None);
        assert_eq!(
            limit_price_for(Side::Buy, dec!(100), dec!(101), 101.0),
            None
        );
        assert_eq!(
            limit_price_for(Side::Buy, dec!(100), dec!(101), f64::NAN),
            None
        );
        // Crossed book fails closed.
        assert_eq!(limit_price_for(Side::Buy, dec!(102), dec!(101), 50.0), None);
        // Missing touch fails closed.
        assert_eq!(
            limit_price_for(Side::Buy, Decimal::ZERO, dec!(101), 50.0),
            None
        );
        assert_eq!(spread_bps(dec!(102), dec!(101)), None);
        assert_eq!(spread_bps(Decimal::ZERO, dec!(101)), None);
    }

    #[test]
    fn spread_bps_math() {
        // (101-100)/100.5*10000 = 10000/100.5.
        let s = spread_bps(dec!(100), dec!(101)).expect("spread");
        assert!((s - 99.50248756218905).abs() < 1e-9, "got {s}");
    }

    #[test]
    fn calm_gate_blocks_wide_spread_and_fast_vol() {
        let cfg = PassiveUnwindConfig::default();
        assert!(calm_gate_ok(5.0, Some(5.0), &cfg));
        assert!(!calm_gate_ok(15.01, Some(1.0), &cfg));
        assert!(!calm_gate_ok(5.0, Some(10.01), &cfg));
        // Unknown fast vol passes its leg; spread stays the hard guard.
        assert!(calm_gate_ok(5.0, None, &cfg));
        assert!(!calm_gate_ok(99.0, None, &cfg));
        assert!(!calm_gate_ok(f64::NAN, None, &cfg));
    }

    #[test]
    fn limit_orders_are_reduce_only() {
        let o = build_limit_reduce_order(
            VenueId::new("test"),
            "BTC-USDC",
            Side::Sell,
            dec!(1),
            dec!(100),
            true,
        );
        assert!(o.reduce_only);
        assert_eq!(o.order_type, arbitrageur_core::types::OrderType::Limit);
        let m = build_market_fallback_order(
            VenueId::new("test"),
            "BTC-USDC",
            Side::Sell,
            dec!(1),
            true,
        );
        assert!(m.reduce_only);
        assert_eq!(m.order_type, arbitrageur_core::types::OrderType::Market);
    }

    #[tokio::test]
    async fn limit_fills_when_touchable() {
        let paper = ScriptedPaper::new(vec![ScriptStep::Full]);
        let paper: Arc<dyn ExchangeAdapter> = Arc::new(paper);
        let refresh = ScriptedRefresh::new(vec![test_book(dec!(100), dec!(101))]);
        let report = execute_passive_reduce(
            &paper,
            &refresh,
            "BTC-USDC",
            Side::Sell,
            dec!(1),
            &enabled_cfg(),
            Some(2.0),
        )
        .await
        .expect("reduce resolves");
        assert_eq!(report.outcome, PassiveOutcome::LimitFilled);
        assert_eq!(report.limit_tries, 1);
        assert_eq!(report.limit_prices, vec![dec!(100.5)]);
        assert_eq!(report.response.status, OrderStatus::Filled);
    }

    #[tokio::test]
    async fn timeout_retry_then_market_fallback_sequence() {
        // Two untouched attempts, then the script runs dry (Untouched),
        // so try 3 also misses and the driver falls back to market.
        let inner = Arc::new(ScriptedPaper::new(vec![
            ScriptStep::Untouched,
            ScriptStep::Untouched,
            ScriptStep::Untouched,
            ScriptStep::Full,
        ]));
        let paper: Arc<dyn ExchangeAdapter> = inner.clone();
        let refresh = ScriptedRefresh::new(vec![
            test_book(dec!(100), dec!(101)),
            test_book(dec!(100), dec!(102)),
            test_book(dec!(99), dec!(101)),
        ]);
        let report = execute_passive_reduce(
            &paper,
            &refresh,
            "BTC-USDC",
            Side::Sell,
            dec!(1),
            &enabled_cfg(),
            Some(2.0),
        )
        .await
        .expect("reduce resolves");
        assert!(
            matches!(report.outcome, PassiveOutcome::MarketFallback { .. }),
            "got {:?}",
            report.outcome
        );
        assert_eq!(report.limit_tries, 3);
        // Re-priced to each fresh book: mids 100.5, 101.0, 100.0.
        assert_eq!(report.limit_prices, vec![dec!(100.5), dec!(101), dec!(100)]);
    }

    #[tokio::test]
    async fn retry_recovers_on_second_try() {
        let inner = ScriptedPaper::new(vec![ScriptStep::Untouched, ScriptStep::Full]);
        let paper: Arc<dyn ExchangeAdapter> = Arc::new(inner);
        let refresh = ScriptedRefresh::new(vec![
            test_book(dec!(100), dec!(101)),
            test_book(dec!(100), dec!(102)),
        ]);
        let report = execute_passive_reduce(
            &paper,
            &refresh,
            "BTC-USDC",
            Side::Sell,
            dec!(1),
            &enabled_cfg(),
            Some(2.0),
        )
        .await
        .expect("reduce resolves");
        assert_eq!(report.outcome, PassiveOutcome::LimitFilled);
        assert_eq!(report.limit_tries, 2);
        assert_eq!(report.limit_prices, vec![dec!(100.5), dec!(101)]);
    }

    #[tokio::test]
    async fn volatile_market_skips_passive_entirely() {
        let inner = Arc::new(ScriptedPaper::new(vec![ScriptStep::Full]));
        let paper: Arc<dyn ExchangeAdapter> = inner.clone();
        let refresh = ScriptedRefresh::new(vec![test_book(dec!(100), dec!(101))]);
        let report = execute_passive_reduce(
            &paper,
            &refresh,
            "BTC-USDC",
            Side::Sell,
            dec!(1),
            &enabled_cfg(),
            Some(50.0),
        )
        .await
        .expect("reduce resolves");
        assert!(
            matches!(report.outcome, PassiveOutcome::MarketFallback { .. }),
            "got {:?}",
            report.outcome
        );
        assert_eq!(report.limit_tries, 0);
        assert!(report.limit_prices.is_empty());
        // Only the market fallback was placed — no limit touched the venue.
        let calls = inner.recorded();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].order_type,
            arbitrageur_core::types::OrderType::Market
        );
    }

    #[tokio::test]
    async fn disabled_config_goes_straight_to_market() {
        let inner = ScriptedPaper::new(vec![ScriptStep::Full]);
        let paper: Arc<dyn ExchangeAdapter> = Arc::new(inner);
        let refresh = ScriptedRefresh::new(vec![test_book(dec!(100), dec!(101))]);
        let report = execute_passive_reduce(
            &paper,
            &refresh,
            "BTC-USDC",
            Side::Sell,
            dec!(1),
            &PassiveUnwindConfig::default(),
            Some(1.0),
        )
        .await
        .expect("reduce resolves");
        assert!(
            matches!(report.outcome, PassiveOutcome::MarketFallback { .. }),
            "got {:?}",
            report.outcome
        );
        assert_eq!(report.limit_tries, 0);
    }

    #[tokio::test]
    async fn partial_limit_fill_takes_remainder_at_market() {
        let inner = ScriptedPaper::new(vec![ScriptStep::Partial(dec!(0.4)), ScriptStep::Full]);
        let paper: Arc<dyn ExchangeAdapter> = Arc::new(inner);
        let refresh = ScriptedRefresh::new(vec![test_book(dec!(100), dec!(101))]);
        let report = execute_passive_reduce(
            &paper,
            &refresh,
            "BTC-USDC",
            Side::Sell,
            dec!(1),
            &enabled_cfg(),
            Some(2.0),
        )
        .await
        .expect("reduce resolves");
        assert_eq!(report.outcome, PassiveOutcome::LimitPartialThenMarket);
        assert_eq!(report.response.filled_qty, dec!(1));
        assert_eq!(report.response.status, OrderStatus::Filled);
    }

    #[tokio::test]
    async fn partial_fill_survives_remainder_take_failure() {
        let paper: Arc<dyn ExchangeAdapter> = Arc::new(ScriptedPaper::new(vec![
            ScriptStep::Partial(dec!(0.4)),
            ScriptStep::TransportError,
        ]));
        let refresh = ScriptedRefresh::new(vec![test_book(dec!(100), dec!(101))]);
        let report = execute_passive_reduce(
            &paper,
            &refresh,
            "BTC-USDC",
            Side::Sell,
            dec!(1),
            &enabled_cfg(),
            Some(2.0),
        )
        .await
        .expect("partial record must be returned, not Err");
        assert!(
            matches!(report.outcome, PassiveOutcome::MarketFallback { .. }),
            "got {:?}",
            report.outcome
        );
        assert_eq!(report.response.filled_qty, dec!(0.4));
        assert_eq!(report.response.status, OrderStatus::PartiallyFilled);
    }

    #[tokio::test]
    async fn max_tries_one_bounds_everything() {
        let inner = ScriptedPaper::new(vec![ScriptStep::Untouched, ScriptStep::Full]);
        let paper: Arc<dyn ExchangeAdapter> = Arc::new(inner);
        let refresh = ScriptedRefresh::new(vec![test_book(dec!(100), dec!(101))]);
        let cfg = PassiveUnwindConfig {
            max_tries: 1,
            ..enabled_cfg()
        };
        let report = execute_passive_reduce(
            &paper,
            &refresh,
            "BTC-USDC",
            Side::Sell,
            dec!(1),
            &cfg,
            Some(2.0),
        )
        .await
        .expect("reduce resolves");
        assert!(
            matches!(report.outcome, PassiveOutcome::MarketFallback { .. }),
            "got {:?}",
            report.outcome
        );
        assert_eq!(report.limit_tries, 1);
    }

    #[tokio::test]
    async fn transport_error_retries_then_recovers() {
        let inner = ScriptedPaper::new(vec![ScriptStep::TransportError, ScriptStep::Full]);
        let paper: Arc<dyn ExchangeAdapter> = Arc::new(inner);
        let refresh = ScriptedRefresh::new(vec![
            test_book(dec!(100), dec!(101)),
            test_book(dec!(100), dec!(102)),
        ]);
        let report = execute_passive_reduce(
            &paper,
            &refresh,
            "BTC-USDC",
            Side::Sell,
            dec!(1),
            &enabled_cfg(),
            Some(2.0),
        )
        .await
        .expect("reduce resolves");
        assert_eq!(report.outcome, PassiveOutcome::LimitFilled);
        assert_eq!(report.limit_tries, 2);
    }

    #[tokio::test]
    async fn determinism_same_book_same_price() {
        async fn run_once() -> Vec<Decimal> {
            let paper: Arc<dyn ExchangeAdapter> =
                Arc::new(ScriptedPaper::new(vec![ScriptStep::Full]));
            let refresh = ScriptedRefresh::new(vec![test_book(dec!(100), dec!(101))]);
            execute_passive_reduce(
                &paper,
                &refresh,
                "BTC-USDC",
                Side::Sell,
                dec!(1),
                &enabled_cfg(),
                Some(2.0),
            )
            .await
            .expect("reduce resolves")
            .limit_prices
        }
        assert_eq!(run_once().await, run_once().await);
    }

    #[tokio::test]
    async fn invalid_config_and_qty_fail_closed() {
        let paper: Arc<dyn ExchangeAdapter> = Arc::new(ScriptedPaper::new(vec![ScriptStep::Full]));
        let refresh = ScriptedRefresh::new(vec![test_book(dec!(100), dec!(101))]);
        let mut bad = enabled_cfg();
        bad.spread_percentile = 500.0;
        assert!(execute_passive_reduce(
            &paper,
            &refresh,
            "BTC-USDC",
            Side::Sell,
            dec!(1),
            &bad,
            None
        )
        .await
        .is_err());
        assert!(execute_passive_reduce(
            &paper,
            &refresh,
            "BTC-USDC",
            Side::Sell,
            Decimal::ZERO,
            &enabled_cfg(),
            None
        )
        .await
        .is_err());
    }

    // ---- live-driver tests -------------------------------------------------
    //
    // `ScriptedLive` models the real-venue contract the paper probe does not:
    // limit submission returns an *acceptance* (New/0, Lighter-style), the
    // order rests, and its state is only knowable through `fetch_order`
    // polls. Each submitted limit order pops one scripted poll queue; market
    // orders always fill fully.

    #[derive(Debug)]
    enum Poll {
        /// Order still working; payload is the cumulative fill so far.
        Working(Decimal),
        Filled,
        Cancelled,
        /// fetch_order transport failure.
        Error,
    }

    struct LiveOrder {
        req: OrderRequest,
        polls: Mutex<VecDeque<Poll>>,
        cancelled: Mutex<bool>,
    }

    /// orders[i] is the poll script for the i-th limit order submitted.
    /// Market orders are never scripted — they fill on submission.
    struct ScriptedLive {
        venue: VenueId,
        order_scripts: Mutex<VecDeque<VecDeque<Poll>>>,
        orders: Mutex<Vec<Arc<LiveOrder>>>,
        cancels: Mutex<usize>,
    }

    impl ScriptedLive {
        fn new(order_scripts: Vec<Vec<Poll>>) -> Self {
            Self {
                venue: VenueId::new("test"),
                order_scripts: Mutex::new(order_scripts.into_iter().map(Into::into).collect()),
                orders: Mutex::new(Vec::new()),
                cancels: Mutex::new(0),
            }
        }

        fn cancel_count(&self) -> usize {
            *self.cancels.lock().expect("test mutex")
        }
    }

    fn live_resp(order: &LiveOrder, status: OrderStatus, filled: Decimal) -> OrderResponse {
        OrderResponse {
            order_id: order.req.client_order_id.clone(),
            client_order_id: order.req.client_order_id.clone(),
            venue: order.req.venue.clone(),
            symbol: order.req.symbol.clone(),
            status,
            original_qty: order.req.quantity,
            filled_qty: filled,
            remaining_qty: (order.req.quantity - filled).max(Decimal::ZERO),
            avg_fill_price: (filled > Decimal::ZERO).then_some(dec!(100.5)),
            fee: if filled > Decimal::ZERO {
                Some(dec!(0.01))
            } else {
                Some(Decimal::ZERO)
            },
            fee_currency: "USDC".to_string(),
            raw_response: None,
            timestamp: chrono::Utc::now(),
        }
    }

    #[async_trait::async_trait]
    impl ExchangeAdapter for ScriptedLive {
        fn venue_id(&self) -> VenueId {
            self.venue.clone()
        }
        fn name(&self) -> &str {
            "scripted-live"
        }
        async fn fetch_markets(
            &self,
        ) -> anyhow::Result<Vec<arbitrageur_core::market::MarketMetadata>> {
            Ok(vec![])
        }
        async fn fetch_orderbook(&self, _symbol: &str) -> anyhow::Result<OrderBook> {
            Ok(test_book(dec!(100), dec!(101)))
        }
        async fn fetch_ticker(
            &self,
            symbol: &str,
        ) -> anyhow::Result<arbitrageur_core::quote::Ticker> {
            Ok(arbitrageur_core::quote::Ticker {
                venue: self.venue.clone(),
                symbol: symbol.to_string(),
                bid: dec!(100),
                ask: dec!(101),
                last: dec!(100.5),
                timestamp: chrono::Utc::now(),
            })
        }
        async fn fetch_funding_rate(
            &self,
            symbol: &str,
        ) -> anyhow::Result<arbitrageur_core::quote::FundingRate> {
            Ok(arbitrageur_core::quote::FundingRate {
                venue: self.venue.clone(),
                symbol: symbol.to_string(),
                rate: dec!(0.0001),
                predicted_rate: None,
                interval_hours: 8,
                next_funding_time: chrono::Utc::now(),
                timestamp: chrono::Utc::now(),
            })
        }
        async fn fetch_account(
            &self,
        ) -> anyhow::Result<arbitrageur_core::position::AccountSnapshot> {
            Ok(arbitrageur_core::position::AccountSnapshot {
                venue: self.venue.clone(),
                equity: dec!(10000),
                free_margin: dec!(10000),
                maintenance_margin: dec!(0),
                margin_usage_pct: dec!(0),
                balances: vec![],
                positions: vec![],
                timestamp: chrono::Utc::now(),
            })
        }
        async fn place_order(&self, req: OrderRequest) -> anyhow::Result<OrderResponse> {
            // Market fallback orders fill immediately at touch.
            if matches!(req.order_type, arbitrageur_core::types::OrderType::Market) {
                let order = LiveOrder {
                    req,
                    polls: Mutex::new(VecDeque::new()),
                    cancelled: Mutex::new(false),
                };
                let resp = live_resp(&order, OrderStatus::Filled, order.req.quantity);
                self.orders
                    .lock()
                    .expect("test mutex")
                    .push(Arc::new(order));
                return Ok(resp);
            }
            // Limit orders: Lighter-style acceptance — New, zero reported
            // fill; the truth only arrives via fetch_order polls.
            let polls = self
                .order_scripts
                .lock()
                .expect("test mutex")
                .pop_front()
                .unwrap_or_default();
            let order = Arc::new(LiveOrder {
                req,
                polls: Mutex::new(polls),
                cancelled: Mutex::new(false),
            });
            let resp = live_resp(&order, OrderStatus::New, Decimal::ZERO);
            self.orders.lock().expect("test mutex").push(order);
            Ok(resp)
        }
        async fn fetch_order(
            &self,
            _symbol: &str,
            order_id: &str,
            _client_order_id: Option<&str>,
        ) -> anyhow::Result<OrderResponse> {
            let orders = self.orders.lock().expect("test mutex");
            let order = orders
                .iter()
                .find(|o| o.req.client_order_id == order_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("unknown order {order_id}"))?;
            drop(orders);
            let step = order.polls.lock().expect("test mutex").pop_front();
            let cancelled = *order.cancelled.lock().expect("test mutex");
            Ok(match step {
                // Transport failures beat state — a cancelled order whose
                // confirm-read fails is exactly the unverifiable case.
                Some(Poll::Error) => anyhow::bail!("scripted poll failure"),
                // A Working step observed after cancel = the venue's
                // cancelled-with-partial-fill state.
                Some(Poll::Working(f)) if cancelled => live_resp(&order, OrderStatus::Cancelled, f),
                Some(Poll::Working(f)) => live_resp(
                    &order,
                    if f > Decimal::ZERO {
                        OrderStatus::PartiallyFilled
                    } else {
                        OrderStatus::New
                    },
                    f,
                ),
                Some(Poll::Filled) => live_resp(&order, OrderStatus::Filled, order.req.quantity),
                Some(Poll::Cancelled) | None if cancelled => {
                    live_resp(&order, OrderStatus::Cancelled, Decimal::ZERO)
                }
                Some(Poll::Cancelled) => live_resp(&order, OrderStatus::Cancelled, Decimal::ZERO),
                None => live_resp(&order, OrderStatus::New, Decimal::ZERO),
            })
        }
        async fn cancel_order(
            &self,
            req: arbitrageur_core::order::CancelRequest,
        ) -> anyhow::Result<()> {
            *self.cancels.lock().expect("test mutex") += 1;
            let id = req.order_id.or(req.client_order_id).unwrap_or_default();
            let orders = self.orders.lock().expect("test mutex");
            if let Some(order) = orders.iter().find(|o| o.req.client_order_id == id) {
                *order.cancelled.lock().expect("test mutex") = true;
            }
            Ok(())
        }
    }

    /// Attempt-timeout=0 exercises the cancel+reprice edge instantly; the
    /// working-poll path is covered by the 1s-timeout fill test.
    fn live_cfg(timeout_secs: u64, max_tries: u32) -> PassiveUnwindConfig {
        PassiveUnwindConfig {
            enabled: true,
            spread_percentile: 50.0,
            timeout_secs,
            max_tries,
            max_spread_bps: 200.0,
            max_fast_vol_bps: 10.0,
            max_adverse_drift_bps: Some(5.0),
            enable_l3_queue_drain_evacuation: true,
            l3_drain_forecaster: None,
            enable_queue_priority_tracking: true,
            min_queue_seniority_ratio: Some(0.20),
            queue_hazard_pickoff_threshold: Some(0.70),
            queue_priority_hazard_engine: None,
            power_law_propagator: None,
            enable_ofi_toxicity_shading: true,
            ofi_toxicity_engine: None,
            enable_hawkes_queue_drain_evacuation: true,
            hawkes_drain_forecaster: None,
        }
    }

    #[tokio::test]
    async fn live_limit_fills_via_polling() {
        // Submit returns New/0 (acceptance only); the first poll reports the
        // fill — the driver must not mistake New for untouched.
        let live: Arc<dyn ExchangeAdapter> = Arc::new(ScriptedLive::new(vec![vec![Poll::Filled]]));
        let refresh = ScriptedRefresh::new(vec![test_book(dec!(100), dec!(101))]);
        let report = execute_passive_reduce_live(
            &live,
            &refresh,
            "BTC-USDC",
            Side::Sell,
            dec!(1),
            &live_cfg(1, 3),
            None,
        )
        .await
        .expect("reduce resolves");
        assert_eq!(report.outcome, PassiveOutcome::LimitFilled);
        assert_eq!(report.response.filled_qty, dec!(1));
        assert_eq!(report.limit_tries, 1);
    }

    #[tokio::test]
    async fn live_unfilled_attempt_cancels_before_reprice() {
        // timeout=0: no poll window — the attempt is cancelled, confirmed
        // terminal via fetch_order, then re-pegged. Attempt 2's confirm
        // fetch reports a late fill (venue raced the cancel).
        let adapter = Arc::new(ScriptedLive::new(vec![
            vec![Poll::Working(dec!(0))],
            vec![Poll::Filled],
        ]));
        let live: Arc<dyn ExchangeAdapter> = adapter.clone();
        let refresh = ScriptedRefresh::new(vec![test_book(dec!(100), dec!(101))]);
        let report = execute_passive_reduce_live(
            &live,
            &refresh,
            "BTC-USDC",
            Side::Sell,
            dec!(1),
            &live_cfg(0, 3),
            None,
        )
        .await
        .expect("reduce resolves");
        assert_eq!(report.outcome, PassiveOutcome::LimitFilled);
        assert_eq!(report.limit_tries, 2);
        assert_eq!(report.limit_prices.len(), 2);
        assert_eq!(report.response.filled_qty, dec!(1));
        // Every working attempt was cancelled before the next peg.
        assert_eq!(adapter.cancel_count(), 2);
    }

    #[tokio::test]
    async fn live_exhaustion_falls_back_to_market() {
        // Three attempts, each confirmed cancelled with zero fill → the
        // remainder goes to a market reduce-only order.
        let adapter = Arc::new(ScriptedLive::new(vec![
            vec![Poll::Working(dec!(0))],
            vec![Poll::Working(dec!(0))],
            vec![Poll::Working(dec!(0))],
        ]));
        let live: Arc<dyn ExchangeAdapter> = adapter.clone();
        let refresh = ScriptedRefresh::new(vec![test_book(dec!(100), dec!(101))]);
        let report = execute_passive_reduce_live(
            &live,
            &refresh,
            "BTC-USDC",
            Side::Sell,
            dec!(1),
            &live_cfg(0, 3),
            None,
        )
        .await
        .expect("reduce resolves");
        assert!(matches!(
            report.outcome,
            PassiveOutcome::MarketFallback { .. }
        ));
        assert_eq!(report.response.filled_qty, dec!(1));
        assert_eq!(adapter.cancel_count(), 3);
    }

    #[tokio::test]
    async fn live_unverifiable_cancel_stops_stacking_orders() {
        // The post-cancel fetch errors — the driver cannot prove the resting
        // order is dead, so it must NOT peg another limit AND must not
        // market-close either: a market order would stack a second reduce on
        // top of one that may still be live, flipping past flat. The run
        // ends Unresolved with exactly one order ever placed.
        let adapter = Arc::new(ScriptedLive::new(vec![vec![
            Poll::Error,
            Poll::Error,
            Poll::Error,
            Poll::Error,
        ]]));
        let live: Arc<dyn ExchangeAdapter> = adapter.clone();
        let refresh = ScriptedRefresh::new(vec![test_book(dec!(100), dec!(101))]);
        let report = execute_passive_reduce_live(
            &live,
            &refresh,
            "BTC-USDC",
            Side::Sell,
            dec!(1),
            &live_cfg(0, 3),
            None,
        )
        .await
        .expect("reduce resolves");
        // Exactly one limit order was ever resting — no market order was
        // stacked behind an order whose state could not be verified.
        assert_eq!(adapter.orders.lock().expect("test mutex").len(), 1);
        assert!(matches!(report.outcome, PassiveOutcome::Unresolved { .. }));
        // Nothing provably filled — the report claims no fill rather than
        // inventing one.
        assert_eq!(report.response.filled_qty, dec!(0));
    }

    #[tokio::test]
    async fn live_partial_fill_then_market_remainder() {
        // Attempt 1 partially fills 0.4 then confirms cancelled; remaining
        // 0.6 takes the market fallback. Combined response is honest: full
        // qty, weighted average price, summed fees.
        let adapter = Arc::new(ScriptedLive::new(vec![vec![
            Poll::Working(dec!(0.4)),
            Poll::Cancelled,
        ]]));
        let live: Arc<dyn ExchangeAdapter> = adapter.clone();
        let refresh = ScriptedRefresh::new(vec![test_book(dec!(100), dec!(101))]);
        let report = execute_passive_reduce_live(
            &live,
            &refresh,
            "BTC-USDC",
            Side::Sell,
            dec!(1),
            &live_cfg(0, 3),
            None,
        )
        .await
        .expect("reduce resolves");
        assert_eq!(report.outcome, PassiveOutcome::LimitPartialThenMarket);
        assert_eq!(report.response.filled_qty, dec!(1));
        // limit 0.4 @ 100.5 + market 0.6 @ 100.5 → vwap 100.5
        assert_eq!(report.response.avg_fill_price, Some(dec!(100.5)));
        assert_eq!(report.response.fee, Some(dec!(0.02)));
    }

    #[test]
    fn test_evaluate_stochastic_depth_unwind_pacing() {
        use arbitrageur_math::stochastic_depth::{
            ExecutionPacingDecision, StochasticDepthLiquidationExecutor,
        };

        let exec = StochasticDepthLiquidationExecutor::new(1e-4, 0.01, 100.0, 0.5);

        // Healthy depth: 120 vs base 100 -> Normal
        let pacing_norm = evaluate_stochastic_depth_unwind_pacing(
            dec!(5.0),
            Some(dec!(120.0)),
            10.0,
            Some(&exec),
        );
        assert!(matches!(
            pacing_norm,
            ExecutionPacingDecision::ExecuteNormal { .. }
        ));

        // Critical depth collapse: 10 vs base 100 -> PauseReplenishmentWait
        let pacing_pause =
            evaluate_stochastic_depth_unwind_pacing(dec!(5.0), Some(dec!(10.0)), 10.0, Some(&exec));
        assert!(matches!(
            pacing_pause,
            ExecutionPacingDecision::PauseReplenishmentWait { .. }
        ));
    }

    #[tokio::test]
    async fn test_passive_unwind_ofi_toxicity_evacuation() {
        let mut ofi_engine =
            arbitrageur_math::micro_price_surface::MultiLevelOfiToxicityEngine::new(
                arbitrageur_math::micro_price_surface::MultiLevelOfiConfig {
                    toxicity_threshold: 0.05,
                    ..Default::default()
                },
            );

        // Pre-condition engine with initial baseline snapshot at t0
        let t0 = vec![arbitrageur_math::micro_price_surface::DepthLevelQuote::new(
            100.0, 10.0, 101.0, 10.0,
        )];
        ofi_engine.update(&t0, 100.5, 0.0, 0.40, 1.0);

        let cfg = PassiveUnwindConfig {
            enabled: true,
            enable_ofi_toxicity_shading: true,
            ofi_toxicity_engine: Some(ofi_engine),
            ..enabled_cfg()
        };

        // When fresh book presents an aggressive selling crash (bid dropped to 99, ask dropped to 99.5),
        // peg_for_attempt must detect the adverse flow surge and abort passive Buy limit placement
        let refresh = ScriptedRefresh::new(vec![test_book(dec!(99), dec!(99.5))]);
        let res = peg_for_attempt(&refresh, Side::Buy, &cfg, None).await;
        assert!(res.is_err());
        assert!(res
            .unwrap_err()
            .contains("OFI toxic adverse flow surge detected"));
    }

    #[tokio::test]
    async fn test_passive_unwind_hawkes_queue_drain_evacuation() {
        let toxic_drain = arbitrageur_math::cross_hawkes::MultiLevelHawkesQueueDrainForecaster::new(
            50.0, 10.0, 2.0, 1.0,
        );
        let cfg = PassiveUnwindConfig {
            enabled: true,
            enable_hawkes_queue_drain_evacuation: true,
            hawkes_drain_forecaster: Some(toxic_drain),
            ..enabled_cfg()
        };

        let refresh = ScriptedRefresh::new(vec![test_book(dec!(100), dec!(101))]);
        let res = peg_for_attempt(&refresh, Side::Buy, &cfg, None).await;
        assert!(res.is_err());
        assert!(res
            .unwrap_err()
            .contains("Multi-level Hawkes queue drain hazard"));
    }
}
