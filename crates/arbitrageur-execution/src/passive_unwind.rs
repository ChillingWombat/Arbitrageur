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
use arbitrageur_core::order::{OrderRequest, OrderResponse};
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

/// Build a reduce-only limit order at an explicit price.
pub fn build_limit_reduce_order(
    venue: VenueId,
    symbol: &str,
    side: Side,
    qty: Decimal,
    price: Decimal,
) -> OrderRequest {
    let mut order = OrderRequest::new_limit(venue, symbol, side, price, qty);
    order.reduce_only = true;
    order
}

/// Build a reduce-only market/IOC fallback order (same semantics as the
/// pre-existing flatten path).
pub fn build_market_fallback_order(
    venue: VenueId,
    symbol: &str,
    side: Side,
    qty: Decimal,
) -> OrderRequest {
    let mut order = OrderRequest::new_market(venue, symbol, side, qty);
    order.reduce_only = true;
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
    let order = build_limit_reduce_order(paper.venue_id(), symbol, side, qty, price);
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
    let order = build_market_fallback_order(paper.venue_id(), symbol, side, qty);
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
        let book = match refresher.fresh_book().await {
            Ok(b) => b,
            Err(e) => {
                return market_fallback(
                    paper,
                    symbol,
                    side,
                    qty,
                    format!("book refresh failed: {e:#}"),
                    tries,
                    limit_prices,
                )
                .await;
            }
        };
        let (Some(bid), Some(ask)) = (book.best_bid_price(), book.best_ask_price()) else {
            return market_fallback(
                paper,
                symbol,
                side,
                qty,
                "book missing touch".to_string(),
                tries,
                limit_prices,
            )
            .await;
        };
        let spread = match spread_bps(bid, ask) {
            Some(s) => s,
            None => {
                return market_fallback(
                    paper,
                    symbol,
                    side,
                    qty,
                    "uncrossable book".to_string(),
                    tries,
                    limit_prices,
                )
                .await;
            }
        };
        if !calm_gate_ok(spread, fast_vol_bps, cfg) {
            return market_fallback(
                paper,
                symbol,
                side,
                qty,
                format!("calm gate failed (spread {spread:.2} bps)"),
                tries,
                limit_prices,
            )
            .await;
        }
        let price = match limit_price_for(side, bid, ask, cfg.spread_percentile) {
            Some(p) => p,
            None => {
                return market_fallback(
                    paper,
                    symbol,
                    side,
                    qty,
                    "limit pricing failed".to_string(),
                    tries,
                    limit_prices,
                )
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
                // Take the remainder at market immediately: a half-closed
                // reduce must not be left working. Either failure below
                // still returns the partial limit fill — the Err contract
                // reserves Err for "no fill record exists at all".
                let mkt = build_market_fallback_order(paper.venue_id(), symbol, side, remaining);
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
        fee: limit_resp.fee + mkt_resp.fee,
        fee_currency: limit_resp.fee_currency.clone(),
        raw_response: None,
        timestamp: chrono::Utc::now(),
    })
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
                fee: dec!(0.01),
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
        );
        assert!(o.reduce_only);
        assert_eq!(o.order_type, arbitrageur_core::types::OrderType::Limit);
        let m = build_market_fallback_order(VenueId::new("test"), "BTC-USDC", Side::Sell, dec!(1));
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
}
