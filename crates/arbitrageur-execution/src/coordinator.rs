use crate::emergency_unwind::EmergencyUnwindHandler;
use crate::smart_routing::SmartOrderRouter;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::types::{Decimal, OrderStatus, Side};
use arbitrageur_risk::circuit_breaker::CircuitBreaker;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};

/// Bounded venue-status window for a still-working leg. Venue acks
/// regularly precede the fill report, so a non-terminal leg is polled
/// before the coordinator classifies it — a fill landing moments later must
/// resolve on fresh venue truth, not force an unnecessary unwind of the
/// other leg. Kept short: every poll defers compensation for a real fill.
#[cfg(not(test))]
const LEG_SETTLE_POLLS: u32 = 5;
#[cfg(test)]
const LEG_SETTLE_POLLS: u32 = 3;
#[cfg(not(test))]
const LEG_SETTLE_INTERVAL: Duration = Duration::from_millis(600);
#[cfg(test)]
const LEG_SETTLE_INTERVAL: Duration = Duration::from_millis(5);

/// Poll a non-terminal, identity-verified leg until it reaches a terminal
/// status or the poll budget is spent. When the leg still reported no fill
/// — or `cancel_partial` is set (the sibling leg can never pair, so no
/// matched portion is worth keeping) — a still-working order is then
/// cancelled and polled once more, so classification runs on a terminal
/// state: an order left working can fill later, outside every tracked
/// quantity. A wrong-order echo is never adopted; a leg that stays
/// non-terminal regardless returns with its verified identity intact for
/// the reconciler, which retries cancel under its own budget.
async fn settle_working_leg(
    exchange: &Arc<dyn ExchangeAdapter>,
    symbol: &str,
    client_order_id: &str,
    resp: OrderResponse,
    cancel_partial: bool,
) -> OrderResponse {
    if resp.status.is_final()
        || !response_identity_matches(&resp, exchange, symbol, client_order_id)
    {
        return resp;
    }
    let mut latest = resp;
    for _ in 0..LEG_SETTLE_POLLS {
        tokio::time::sleep(LEG_SETTLE_INTERVAL).await;
        match exchange
            .fetch_order(symbol, &latest.order_id, Some(client_order_id))
            .await
        {
            Ok(polled) if response_identity_matches(&polled, exchange, symbol, client_order_id) => {
                latest = polled;
                if latest.status.is_final() {
                    return latest;
                }
            }
            Ok(_) => return latest,
            Err(_) => continue,
        }
    }
    if (latest.filled_qty == Decimal::ZERO || cancel_partial) && !latest.status.is_final() {
        let _ = exchange
            .cancel_order(CancelRequest {
                venue: exchange.venue_id(),
                symbol: symbol.to_string(),
                order_id: Some(latest.order_id.clone()),
                client_order_id: Some(client_order_id.to_string()),
            })
            .await;
        tokio::time::sleep(LEG_SETTLE_INTERVAL).await;
        if let Ok(polled) = exchange
            .fetch_order(symbol, &latest.order_id, Some(client_order_id))
            .await
        {
            if response_identity_matches(&polled, exchange, symbol, client_order_id) {
                latest = polled;
            }
        }
    }
    latest
}

/// Result of an atomic two-leg arbitrage execution attempt.
#[derive(Debug)]
pub enum ArbitrageExecutionResult {
    /// Both legs filled cleanly
    Success {
        leg_a: OrderResponse,
        leg_b: OrderResponse,
        realized_profit: Decimal,
    },
    /// Leg A filled, Leg B failed; Leg A was successfully unwound
    LegBFailedUnwound {
        leg_a_fill: OrderResponse,
        unwind_resp: OrderResponse,
        error_b: String,
    },
    /// Leg B filled, Leg A failed; Leg B was successfully unwound
    LegAFailedUnwound {
        leg_b_fill: OrderResponse,
        unwind_resp: OrderResponse,
        error_a: String,
    },
    /// Both legs filled but only partially (or incompletely): the excess of
    /// the larger fill was trimmed via the unwind path and the matched
    /// portion remains open as a delta-neutral pair. Callers must attribute
    /// the matched quantity (e.g. ledger cycle) — it is live exposure.
    /// `realized_profit` is the closed trim-leg price PnL minus entry and
    /// trim fees; the matched pair itself is unrealized and excluded. A trim
    /// leg without fill prices contributes its fees only.
    PartialPairKept {
        leg_a_fill: OrderResponse,
        leg_b_fill: OrderResponse,
        matched_qty: Decimal,
        unwind_a: Option<Box<OrderResponse>>,
        unwind_b: Option<Box<OrderResponse>>,
        realized_profit: Decimal,
    },
    /// Both legs failed with no residual exposure: clean rejects, zero-fill
    /// responses, or fills that were successfully unwound
    BothFailed { error_a: String, error_b: String },
    /// Exposure may remain on a filled leg because its compensation order failed
    UnresolvedExposure {
        leg_a_fill: Option<OrderResponse>,
        leg_b_fill: Option<OrderResponse>,
        error_a: String,
        error_b: String,
    },
    /// Execution halted by risk controls before order dispatch
    HaltedByRisk(String),
}

/// A response is attributable to a dispatched order only when the venue,
/// symbol, and captured client order ID all match and the venue order ID is
/// nonempty.
fn response_identity_matches(
    resp: &OrderResponse,
    exchange: &Arc<dyn ExchangeAdapter>,
    symbol: &str,
    client_order_id: &str,
) -> bool {
    resp.venue == exchange.venue_id()
        && resp.symbol.as_str() == symbol
        && !client_order_id.trim().is_empty()
        && resp.client_order_id == client_order_id
        && !resp.order_id.trim().is_empty()
}

/// A fill is only complete when it is attributed to the expected venue,
/// symbol, and client order ID, echoes the requested quantity, reports Filled
/// status with nothing remaining open, a positive fill price, and a
/// nonnegative fee.
fn is_complete_fill(
    resp: &OrderResponse,
    exchange: &Arc<dyn ExchangeAdapter>,
    symbol: &str,
    client_order_id: &str,
    requested_qty: Decimal,
) -> bool {
    response_identity_matches(resp, exchange, symbol, client_order_id)
        && resp.original_qty == requested_qty
        && resp.status == OrderStatus::Filled
        && resp.filled_qty == requested_qty
        && resp.remaining_qty == Decimal::ZERO
        && resp
            .avg_fill_price
            .map(|p| p > Decimal::ZERO)
            .unwrap_or(false)
        && resp.fee >= Decimal::ZERO
}

/// A partial fill is trimmable only when it is attributable to the
/// dispatched order, echoes the requested quantity, reports PartiallyFilled
/// with the exact shortfall remaining, and carries a positive fill price with
/// a nonnegative fee. Malformed fills (wrong echoed qty, missing price,
/// negative fee) are never kept open — they fall through to the full-unwind
/// path.
fn is_trimmable_partial(
    resp: &OrderResponse,
    exchange: &Arc<dyn ExchangeAdapter>,
    symbol: &str,
    client_order_id: &str,
    requested_qty: Decimal,
) -> bool {
    response_identity_matches(resp, exchange, symbol, client_order_id)
        && resp.original_qty == requested_qty
        && resp.status == OrderStatus::PartiallyFilled
        && resp.filled_qty > Decimal::ZERO
        && resp.filled_qty < requested_qty
        && resp.remaining_qty == requested_qty - resp.filled_qty
        && resp
            .avg_fill_price
            .map(|p| p > Decimal::ZERO)
            .unwrap_or(false)
        && resp.fee >= Decimal::ZERO
}

/// Realized price PnL of closing `closed_qty` of a leg opened at
/// `fill_price` via an opposite-side trim filled at `trim_price`.
/// Zero when there is nothing to close or either price is missing
/// (fees still net through the caller — PnL is never invented).
fn trim_leg_pnl(
    side: Side,
    fill_price: Option<Decimal>,
    trim_price: Option<Decimal>,
    closed_qty: Decimal,
) -> Decimal {
    let (Some(fill), Some(trim)) = (fill_price, trim_price) else {
        return Decimal::ZERO;
    };
    if !(fill > Decimal::ZERO && trim > Decimal::ZERO && closed_qty > Decimal::ZERO) {
        return Decimal::ZERO;
    }
    match side {
        Side::Buy => (trim - fill) * closed_qty,
        Side::Sell => (fill - trim) * closed_qty,
    }
}

/// Concurrent execution coordinator for atomic multi-leg arbitrage.
#[allow(dead_code)]
pub struct AtomicArbitrageCoordinator {
    router: SmartOrderRouter,
    circuit_breaker: Arc<CircuitBreaker>,
}

impl AtomicArbitrageCoordinator {
    pub fn new(router: SmartOrderRouter, circuit_breaker: Arc<CircuitBreaker>) -> Self {
        Self {
            router,
            circuit_breaker,
        }
    }

    pub fn router(&self) -> &SmartOrderRouter {
        &self.router
    }

    pub fn circuit_breaker(&self) -> &Arc<CircuitBreaker> {
        &self.circuit_breaker
    }

    /// Execute atomic two-leg arbitrage concurrently.
    /// Leg A: buy/sell on `exchange_a`
    /// Leg B: opposite side on `exchange_b`
    pub async fn execute_two_leg(
        &self,
        exchange_a: &Arc<dyn ExchangeAdapter>,
        order_a: OrderRequest,
        exchange_b: &Arc<dyn ExchangeAdapter>,
        order_b: OrderRequest,
    ) -> ArbitrageExecutionResult {
        // 1. Pre-trade Risk Check
        if let Err(reason) = self.circuit_breaker.is_trading_allowed() {
            warn!("Arbitrage execution blocked by circuit breaker: {}", reason);
            return ArbitrageExecutionResult::HaltedByRisk(reason);
        }

        info!(
            "🚀 Dispatching concurrent two-leg arbitrage: Leg A: {} {:?} {} @ {:?}, Leg B: {} {:?} {} @ {:?}",
            exchange_a.name(),
            order_a.side,
            order_a.quantity,
            order_a.price,
            exchange_b.name(),
            order_b.side,
            order_b.quantity,
            order_b.price
        );

        let side_a = order_a.side;
        let sym_a = order_a.symbol.clone();
        let cid_a = order_a.client_order_id.clone();
        let side_b = order_b.side;
        let sym_b = order_b.symbol.clone();
        let cid_b = order_b.client_order_id.clone();
        let requested_qty_a = order_a.quantity;
        let requested_qty_b = order_b.quantity;

        // 2. Concurrent Order Dispatch via tokio::join!
        let (res_a, res_b) = tokio::join!(
            exchange_a.place_order(order_a),
            exchange_b.place_order(order_b)
        );

        match (res_a, res_b) {
            // Case 1: Both legs executed successfully
            (Ok(fill_a), Ok(fill_b)) => {
                // Acks can precede the fill report: settle still-working
                // legs on venue truth first (zero-fill remainders are
                // cancelled so they can never fill later untracked).
                // Partially-filled legs keep working for the matched-pair
                // trim below — their remainder is the pair's inventory.
                let (fill_a, fill_b) = tokio::join!(
                    settle_working_leg(exchange_a, &sym_a, &cid_a, fill_a, false),
                    settle_working_leg(exchange_b, &sym_b, &cid_b, fill_b, false),
                );
                let identity_a = response_identity_matches(&fill_a, exchange_a, &sym_a, &cid_a);
                let identity_b = response_identity_matches(&fill_b, exchange_b, &sym_b, &cid_b);
                let complete_a =
                    is_complete_fill(&fill_a, exchange_a, &sym_a, &cid_a, requested_qty_a);
                let complete_b =
                    is_complete_fill(&fill_b, exchange_b, &sym_b, &cid_b, requested_qty_b);

                if complete_a && complete_b {
                    self.circuit_breaker.record_success();
                    info!(
                        "✅ Both legs filled! Leg A id={}, Leg B id={}",
                        fill_a.order_id, fill_b.order_id
                    );

                    // Immediate realized cash P&L on opening fills is fee outflow only
                    ArbitrageExecutionResult::Success {
                        realized_profit: -(fill_a.fee + fill_b.fee),
                        leg_a: fill_a,
                        leg_b: fill_b,
                    }
                } else {
                    // Incomplete fill on one or both legs. A nonzero fill whose
                    // identity cannot be correlated to the dispatched order is
                    // never compensated: its true exposure is unknown.
                    self.circuit_breaker
                        .record_error("Incomplete or partial fill status on one or both legs");
                    let ambiguous_a = fill_a.filled_qty > Decimal::ZERO && !identity_a;
                    let ambiguous_b = fill_b.filled_qty > Decimal::ZERO && !identity_b;

                    // Partial-fill trim: both legs returned Ok with
                    // attributable nonzero fills, but at least one leg is
                    // incomplete. Trim the larger fill down to
                    // min(fill_a, fill_b) via the existing unwind path on the
                    // excess only; the matched portion stays open as a
                    // delta-neutral pair. If either residual unwind fails the
                    // breaker trips with UnresolvedExposure (existing
                    // semantics). Each leg must be complete or a sound
                    // partial (exact shortfall, sane price/fee); zero-fill,
                    // identity-mismatch, and malformed legs never reach this
                    // branch — they fall through to the full-unwind path
                    // below, unchanged.
                    let sound_a = complete_a
                        || is_trimmable_partial(
                            &fill_a,
                            exchange_a,
                            &sym_a,
                            &cid_a,
                            requested_qty_a,
                        );
                    let sound_b = complete_b
                        || is_trimmable_partial(
                            &fill_b,
                            exchange_b,
                            &sym_b,
                            &cid_b,
                            requested_qty_b,
                        );
                    if !ambiguous_a
                        && !ambiguous_b
                        && fill_a.filled_qty > Decimal::ZERO
                        && fill_b.filled_qty > Decimal::ZERO
                        && sound_a
                        && sound_b
                    {
                        let matched_qty = fill_a.filled_qty.min(fill_b.filled_qty);
                        let excess_a = fill_a.filled_qty - matched_qty;
                        let excess_b = fill_b.filled_qty - matched_qty;

                        let (trim_a, trim_b) = tokio::join!(
                            async {
                                if excess_a > Decimal::ZERO {
                                    Some(
                                        EmergencyUnwindHandler::unwind_leg(
                                            exchange_a, &sym_a, side_a, excess_a,
                                        )
                                        .await,
                                    )
                                } else {
                                    None
                                }
                            },
                            async {
                                if excess_b > Decimal::ZERO {
                                    Some(
                                        EmergencyUnwindHandler::unwind_leg(
                                            exchange_b, &sym_b, side_b, excess_b,
                                        )
                                        .await,
                                    )
                                } else {
                                    None
                                }
                            },
                        );

                        let describe_trim = |fill: &OrderResponse,
                                             requested_qty: Decimal,
                                             excess: Decimal,
                                             trim: &Option<anyhow::Result<OrderResponse>>| {
                            let trim_outcome = match trim {
                                Some(Ok(resp)) => {
                                    format!(
                                        "trimmed excess {} via order {}",
                                        resp.filled_qty, resp.order_id
                                    )
                                }
                                Some(Err(e)) => format!("unwind failed: {}", e),
                                None => "no trim required (leg equals matched quantity)"
                                    .to_string(),
                            };
                            format!(
                                "status={:?} filled_qty={}/{} remaining_qty={} matched_qty={} excess={}; {}",
                                fill.status,
                                fill.filled_qty,
                                requested_qty,
                                fill.remaining_qty,
                                matched_qty,
                                excess,
                                trim_outcome
                            )
                        };

                        let trim_failed =
                            matches!(trim_a, Some(Err(_))) || matches!(trim_b, Some(Err(_)));
                        let error_a = describe_trim(&fill_a, requested_qty_a, excess_a, &trim_a);
                        let error_b = describe_trim(&fill_b, requested_qty_b, excess_b, &trim_b);

                        if trim_failed {
                            self.circuit_breaker.trip(format!(
                                "Partial-fill trim failed: Leg A [{}]; Leg B [{}]",
                                error_a, error_b
                            ));
                            return ArbitrageExecutionResult::UnresolvedExposure {
                                leg_a_fill: Some(fill_a),
                                leg_b_fill: Some(fill_b),
                                error_a,
                                error_b,
                            };
                        }

                        self.circuit_breaker.record_success();
                        let trim_fee = trim_a
                            .as_ref()
                            .and_then(|r| r.as_ref().ok())
                            .map(|resp| resp.fee)
                            .unwrap_or(Decimal::ZERO)
                            + trim_b
                                .as_ref()
                                .and_then(|r| r.as_ref().ok())
                                .map(|resp| resp.fee)
                                .unwrap_or(Decimal::ZERO);
                        // Closed trim-leg price PnL: each trimmed excess was
                        // opened at the leg fill price and closed at the trim
                        // price. The matched pair stays open (unrealized).
                        let trim_pnl = trim_leg_pnl(
                            side_a,
                            fill_a.avg_fill_price,
                            trim_a
                                .as_ref()
                                .and_then(|r| r.as_ref().ok())
                                .and_then(|resp| resp.avg_fill_price),
                            trim_a
                                .as_ref()
                                .and_then(|r| r.as_ref().ok())
                                .map(|resp| resp.filled_qty.min(excess_a))
                                .unwrap_or(Decimal::ZERO),
                        ) + trim_leg_pnl(
                            side_b,
                            fill_b.avg_fill_price,
                            trim_b
                                .as_ref()
                                .and_then(|r| r.as_ref().ok())
                                .and_then(|resp| resp.avg_fill_price),
                            trim_b
                                .as_ref()
                                .and_then(|r| r.as_ref().ok())
                                .map(|resp| resp.filled_qty.min(excess_b))
                                .unwrap_or(Decimal::ZERO),
                        );
                        info!(
                            "✂️ Partial fills trimmed to matched qty {} (A excess {}, B excess {}); pair kept open",
                            matched_qty, excess_a, excess_b
                        );
                        return ArbitrageExecutionResult::PartialPairKept {
                            realized_profit: trim_pnl - (fill_a.fee + fill_b.fee + trim_fee),
                            leg_a_fill: fill_a,
                            leg_b_fill: fill_b,
                            matched_qty,
                            unwind_a: trim_a.and_then(|r| r.ok().map(Box::new)),
                            unwind_b: trim_b.and_then(|r| r.ok().map(Box::new)),
                        };
                    }

                    let (unwind_a, unwind_b) = tokio::join!(
                        async {
                            if fill_a.filled_qty > Decimal::ZERO && identity_a {
                                Some(
                                    EmergencyUnwindHandler::unwind_leg(
                                        exchange_a,
                                        &sym_a,
                                        side_a,
                                        fill_a.filled_qty,
                                    )
                                    .await,
                                )
                            } else {
                                None
                            }
                        },
                        async {
                            if fill_b.filled_qty > Decimal::ZERO && identity_b {
                                Some(
                                    EmergencyUnwindHandler::unwind_leg(
                                        exchange_b,
                                        &sym_b,
                                        side_b,
                                        fill_b.filled_qty,
                                    )
                                    .await,
                                )
                            } else {
                                None
                            }
                        },
                    );

                    let describe_leg =
                        |fill: &OrderResponse,
                         requested_qty: Decimal,
                         ambiguous: bool,
                         unwind: &Option<anyhow::Result<OrderResponse>>| {
                            let unwind_outcome = if ambiguous {
                                "response identity mismatch; no unwind attempted".to_string()
                            } else {
                                match unwind {
                                    Some(Ok(resp)) => {
                                        format!(
                                            "unwound {} via order {}",
                                            resp.filled_qty, resp.order_id
                                        )
                                    }
                                    Some(Err(e)) => format!("unwind failed: {}", e),
                                    None => "no unwind required (zero filled quantity)".to_string(),
                                }
                            };
                            format!(
                                "status={:?} filled_qty={}/{} remaining_qty={}; {}",
                                fill.status,
                                fill.filled_qty,
                                requested_qty,
                                fill.remaining_qty,
                                unwind_outcome
                            )
                        };

                    let unwind_failed =
                        matches!(unwind_a, Some(Err(_))) || matches!(unwind_b, Some(Err(_)));
                    let error_a = describe_leg(&fill_a, requested_qty_a, ambiguous_a, &unwind_a);
                    let error_b = describe_leg(&fill_b, requested_qty_b, ambiguous_b, &unwind_b);

                    // A leg is "live" if it filled (position exists) or the
                    // venue accepted a still-working order (New /
                    // PartiallyFilled). A working order's identity must be
                    // preserved for the reconciler — never dropped into a
                    // BothFailed error string, which would orphan it.
                    let live_a = fill_a.filled_qty > Decimal::ZERO || !fill_a.status.is_final();
                    let live_b = fill_b.filled_qty > Decimal::ZERO || !fill_b.status.is_final();

                    if ambiguous_a || ambiguous_b || unwind_failed {
                        self.circuit_breaker.trip(format!(
                            "Incomplete or unattributable fill: Leg A [{}]; Leg B [{}]",
                            error_a, error_b
                        ));
                        return ArbitrageExecutionResult::UnresolvedExposure {
                            leg_a_fill: live_a.then_some(fill_a),
                            leg_b_fill: live_b.then_some(fill_b),
                            error_a,
                            error_b,
                        };
                    }

                    if !fill_a.status.is_final() || !fill_b.status.is_final() {
                        // Accepted-but-working order(s): attribution is clean,
                        // nothing filled, no unwind attempted or failed — but
                        // the pair is NOT safely dead. Surface the working
                        // order's identity so the reconciler tracks it to a
                        // terminal state (a late fill escalates to imbalanced
                        // and residual auto-resolution applies). Terminal legs
                        // are carried only in the note strings — a filled leg
                        // that reached here was already unwound, so recording
                        // it as a fill would invent phantom exposure.
                        return ArbitrageExecutionResult::UnresolvedExposure {
                            leg_a_fill: (!fill_a.status.is_final()).then_some(fill_a),
                            leg_b_fill: (!fill_b.status.is_final()).then_some(fill_b),
                            error_a,
                            error_b,
                        };
                    }

                    ArbitrageExecutionResult::BothFailed { error_a, error_b }
                }
            }

            // Case 2: Leg A filled, Leg B failed -> EMERGENCY UNWIND Leg A
            (Ok(fill_a), Err(err_b)) => {
                let err_str = err_b.to_string();
                self.circuit_breaker
                    .record_error(&format!("Leg B failed: {}", err_str));

                // Leg B can never pair, so a still-working leg A is settled
                // to a terminal state first (bounded poll, then cancel): the
                // unwind below must cover the final filled quantity —
                // unwinding a snapshot while the order keeps working orphans
                // the remainder as untracked exposure.
                let fill_a = settle_working_leg(exchange_a, &sym_a, &cid_a, fill_a, true).await;

                if fill_a.filled_qty <= Decimal::ZERO {
                    error!("Leg B failed and Leg A reported zero filled quantity; no exposure to unwind: {}", err_str);
                    if !fill_a.status.is_final() {
                        // Order accepted but still working on the venue —
                        // preserve its identity for the reconciler instead of
                        // orphaning it in a BothFailed string.
                        let note_a = format!(
                            "order {} accepted but not terminal (status={:?}); reconciler will track to terminal state",
                            fill_a.order_id, fill_a.status
                        );
                        return ArbitrageExecutionResult::UnresolvedExposure {
                            leg_a_fill: Some(fill_a),
                            leg_b_fill: None,
                            error_a: note_a,
                            error_b: err_str,
                        };
                    }
                    return ArbitrageExecutionResult::BothFailed {
                        error_a: format!(
                            "status={:?} filled_qty={}/{} remaining_qty={}; no unwind required (zero filled quantity)",
                            fill_a.status,
                            fill_a.filled_qty,
                            requested_qty_a,
                            fill_a.remaining_qty
                        ),
                        error_b: err_str,
                    };
                }

                if !response_identity_matches(&fill_a, exchange_a, &sym_a, &cid_a) {
                    self.circuit_breaker.trip(format!(
                        "Leg A response cannot be attributed to the submitted order (venue={} symbol={} client_order_id={} order_id={}) after Leg B error: {}",
                        fill_a.venue, fill_a.symbol, fill_a.client_order_id, fill_a.order_id, err_str
                    ));
                    return ArbitrageExecutionResult::UnresolvedExposure {
                        leg_a_fill: Some(fill_a),
                        leg_b_fill: None,
                        error_a: "response identity mismatch; nonzero fill cannot be attributed to request and was not unwound".into(),
                        error_b: err_str,
                    };
                }

                error!(
                    "Leg A filled but Leg B failed: {}. Triggering rollback on Leg A!",
                    err_str
                );

                match EmergencyUnwindHandler::unwind_leg(
                    exchange_a,
                    &sym_a,
                    side_a,
                    fill_a.filled_qty,
                )
                .await
                {
                    Ok(unwind_resp) => ArbitrageExecutionResult::LegBFailedUnwound {
                        leg_a_fill: fill_a,
                        unwind_resp,
                        error_b: err_str,
                    },
                    Err(unwind_err) => {
                        self.circuit_breaker.trip(format!(
                            "Unwind failed on Leg A: {} after Leg B error: {}",
                            unwind_err, err_str
                        ));
                        ArbitrageExecutionResult::UnresolvedExposure {
                            leg_a_fill: Some(fill_a),
                            leg_b_fill: None,
                            error_a: format!("unwind failed: {}", unwind_err),
                            error_b: err_str,
                        }
                    }
                }
            }

            // Case 3: Leg B filled, Leg A failed -> EMERGENCY UNWIND Leg B
            (Err(err_a), Ok(fill_b)) => {
                let err_str = err_a.to_string();
                self.circuit_breaker
                    .record_error(&format!("Leg A failed: {}", err_str));

                // Same settle-first rule as Case 2, mirrored.
                let fill_b = settle_working_leg(exchange_b, &sym_b, &cid_b, fill_b, true).await;

                if fill_b.filled_qty <= Decimal::ZERO {
                    error!("Leg A failed and Leg B reported zero filled quantity; no exposure to unwind: {}", err_str);
                    if !fill_b.status.is_final() {
                        // Order accepted but still working on the venue —
                        // preserve its identity for the reconciler instead of
                        // orphaning it in a BothFailed string.
                        let note_b = format!(
                            "order {} accepted but not terminal (status={:?}); reconciler will track to terminal state",
                            fill_b.order_id, fill_b.status
                        );
                        return ArbitrageExecutionResult::UnresolvedExposure {
                            leg_a_fill: None,
                            leg_b_fill: Some(fill_b),
                            error_a: err_str,
                            error_b: note_b,
                        };
                    }
                    return ArbitrageExecutionResult::BothFailed {
                        error_a: err_str,
                        error_b: format!(
                            "status={:?} filled_qty={}/{} remaining_qty={}; no unwind required (zero filled quantity)",
                            fill_b.status,
                            fill_b.filled_qty,
                            requested_qty_b,
                            fill_b.remaining_qty
                        ),
                    };
                }

                if !response_identity_matches(&fill_b, exchange_b, &sym_b, &cid_b) {
                    self.circuit_breaker.trip(format!(
                        "Leg B response cannot be attributed to the submitted order (venue={} symbol={} client_order_id={} order_id={}) after Leg A error: {}",
                        fill_b.venue, fill_b.symbol, fill_b.client_order_id, fill_b.order_id, err_str
                    ));
                    return ArbitrageExecutionResult::UnresolvedExposure {
                        leg_a_fill: None,
                        leg_b_fill: Some(fill_b),
                        error_a: err_str,
                        error_b: "response identity mismatch; nonzero fill cannot be attributed to request and was not unwound".into(),
                    };
                }

                error!(
                    "Leg B filled but Leg A failed: {}. Triggering rollback on Leg B!",
                    err_str
                );

                match EmergencyUnwindHandler::unwind_leg(
                    exchange_b,
                    &sym_b,
                    side_b,
                    fill_b.filled_qty,
                )
                .await
                {
                    Ok(unwind_resp) => ArbitrageExecutionResult::LegAFailedUnwound {
                        leg_b_fill: fill_b,
                        unwind_resp,
                        error_a: err_str,
                    },
                    Err(unwind_err) => {
                        self.circuit_breaker.trip(format!(
                            "Unwind failed on Leg B: {} after Leg A error: {}",
                            unwind_err, err_str
                        ));
                        ArbitrageExecutionResult::UnresolvedExposure {
                            leg_a_fill: None,
                            leg_b_fill: Some(fill_b),
                            error_a: err_str,
                            error_b: format!("unwind failed: {}", unwind_err),
                        }
                    }
                }
            }

            // Case 4: Both legs failed cleanly
            (Err(err_a), Err(err_b)) => {
                warn!("Both legs failed to place: A={}, B={}", err_a, err_b);
                self.circuit_breaker
                    .record_error(&format!("Both legs rejected: A={}, B={}", err_a, err_b));
                ArbitrageExecutionResult::BothFailed {
                    error_a: err_a.to_string(),
                    error_b: err_b.to_string(),
                }
            }
        }
    }
}
