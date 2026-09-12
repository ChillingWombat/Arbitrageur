use std::sync::Arc;
use tracing::{error, info, warn};
use arbitrager_core::exchange::ExchangeAdapter;
use arbitrager_core::order::{OrderRequest, OrderResponse};
use arbitrager_core::types::{Decimal, OrderStatus};
use arbitrager_risk::circuit_breaker::CircuitBreaker;
use crate::emergency_unwind::EmergencyUnwindHandler;
use crate::smart_routing::SmartOrderRouter;

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
    /// Both legs failed with no residual exposure: clean rejects, zero-fill
    /// responses, or fills that were successfully unwound
    BothFailed {
        error_a: String,
        error_b: String,
    },
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
        && resp.client_order_id == client_order_id
        && !resp.order_id.is_empty()
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
        && resp.avg_fill_price.map(|p| p > Decimal::ZERO).unwrap_or(false)
        && resp.fee >= Decimal::ZERO
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
                let identity_a =
                    response_identity_matches(&fill_a, exchange_a, &sym_a, &cid_a);
                let identity_b =
                    response_identity_matches(&fill_b, exchange_b, &sym_b, &cid_b);
                let complete_a = is_complete_fill(
                    &fill_a,
                    exchange_a,
                    &sym_a,
                    &cid_a,
                    requested_qty_a,
                );
                let complete_b = is_complete_fill(
                    &fill_b,
                    exchange_b,
                    &sym_b,
                    &cid_b,
                    requested_qty_b,
                );

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
                    self.circuit_breaker.record_error("Incomplete or partial fill status on one or both legs");
                    let ambiguous_a =
                        fill_a.filled_qty > Decimal::ZERO && !identity_a;
                    let ambiguous_b =
                        fill_b.filled_qty > Decimal::ZERO && !identity_b;

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

                    let describe_leg = |fill: &OrderResponse,
                                        requested_qty: Decimal,
                                        ambiguous: bool,
                                        unwind: &Option<anyhow::Result<OrderResponse>>| {
                        let unwind_outcome = if ambiguous {
                            "response identity mismatch; no unwind attempted".to_string()
                        } else {
                            match unwind {
                                Some(Ok(resp)) => {
                                    format!("unwound {} via order {}", resp.filled_qty, resp.order_id)
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

                    let unwind_failed = matches!(unwind_a, Some(Err(_)))
                        || matches!(unwind_b, Some(Err(_)));
                    let error_a =
                        describe_leg(&fill_a, requested_qty_a, ambiguous_a, &unwind_a);
                    let error_b =
                        describe_leg(&fill_b, requested_qty_b, ambiguous_b, &unwind_b);

                    if ambiguous_a || ambiguous_b || unwind_failed {
                        self.circuit_breaker.trip(format!(
                            "Incomplete or unattributable fill: Leg A [{}]; Leg B [{}]",
                            error_a, error_b
                        ));
                        return ArbitrageExecutionResult::UnresolvedExposure {
                            leg_a_fill: (fill_a.filled_qty > Decimal::ZERO).then_some(fill_a),
                            leg_b_fill: (fill_b.filled_qty > Decimal::ZERO).then_some(fill_b),
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
                self.circuit_breaker.record_error(&format!("Leg B failed: {}", err_str));

                if fill_a.filled_qty <= Decimal::ZERO {
                    error!("Leg B failed and Leg A reported zero filled quantity; no exposure to unwind: {}", err_str);
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

                error!("Leg A filled but Leg B failed: {}. Triggering rollback on Leg A!", err_str);

                match EmergencyUnwindHandler::unwind_leg(exchange_a, &sym_a, side_a, fill_a.filled_qty).await {
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
                self.circuit_breaker.record_error(&format!("Leg A failed: {}", err_str));

                if fill_b.filled_qty <= Decimal::ZERO {
                    error!("Leg A failed and Leg B reported zero filled quantity; no exposure to unwind: {}", err_str);
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

                error!("Leg B filled but Leg A failed: {}. Triggering rollback on Leg B!", err_str);

                match EmergencyUnwindHandler::unwind_leg(exchange_b, &sym_b, side_b, fill_b.filled_qty).await {
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
                self.circuit_breaker.record_error(&format!("Both legs rejected: A={}, B={}", err_a, err_b));
                ArbitrageExecutionResult::BothFailed {
                    error_a: err_a.to_string(),
                    error_b: err_b.to_string(),
                }
            }
        }
    }
}
