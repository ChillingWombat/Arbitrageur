use std::sync::Arc;
use tracing::{error, info, warn};
use arbitrager_core::exchange::ExchangeAdapter;
use arbitrager_core::order::{OrderRequest, OrderResponse};
use arbitrager_core::types::{dec, Decimal, OrderStatus, Side};
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
    /// Both legs failed to fill (clean reject, no exposure)
    BothFailed {
        error_a: String,
        error_b: String,
    },
    /// Execution halted by risk controls before order dispatch
    HaltedByRisk(String),
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
        let side_b = order_b.side;
        let sym_b = order_b.symbol.clone();

        // 2. Concurrent Order Dispatch via tokio::join!
        let (res_a, res_b) = tokio::join!(
            exchange_a.place_order(order_a),
            exchange_b.place_order(order_b)
        );

        match (res_a, res_b) {
            // Case 1: Both legs executed successfully
            (Ok(fill_a), Ok(fill_b)) => {
                if fill_a.status == OrderStatus::Filled && fill_b.status == OrderStatus::Filled {
                    self.circuit_breaker.record_success();
                    info!(
                        "✅ Both legs filled! Leg A id={}, Leg B id={}",
                        fill_a.order_id, fill_b.order_id
                    );

                    let px_a = fill_a.avg_fill_price.unwrap_or(dec!(0));
                    let px_b = fill_b.avg_fill_price.unwrap_or(dec!(0));
                    let matched_qty = fill_a.filled_qty.min(fill_b.filled_qty);
                    let profit = if side_a == Side::Buy && side_b == Side::Sell {
                        (px_b - px_a) * matched_qty - (fill_a.fee + fill_b.fee)
                    } else if side_a == Side::Sell && side_b == Side::Buy {
                        (px_a - px_b) * matched_qty - (fill_a.fee + fill_b.fee)
                    } else if side_a == Side::Buy && side_b == Side::Buy {
                        // Synthetic binary parity / Arrow-Debreu portfolio (Buy YES + Buy NO)
                        // Guaranteed settlement payout is 1.00 per contract
                        (dec!(1.0) - px_a - px_b) * matched_qty - (fill_a.fee + fill_b.fee)
                    } else {
                        // Two-sided sell (e.g. Dutch book outcome shorting)
                        (px_a + px_b - dec!(1.0)) * matched_qty - (fill_a.fee + fill_b.fee)
                    };

                    ArbitrageExecutionResult::Success {
                        leg_a: fill_a,
                        leg_b: fill_b,
                        realized_profit: profit,
                    }
                } else {
                    // Partial fill handling
                    self.circuit_breaker.record_error("Incomplete or partial fill status on one or both legs");
                    ArbitrageExecutionResult::BothFailed {
                        error_a: format!("Status: {:?}", fill_a.status),
                        error_b: format!("Status: {:?}", fill_b.status),
                    }
                }
            }

            // Case 2: Leg A filled, Leg B failed -> EMERGENCY UNWIND Leg A
            (Ok(fill_a), Err(err_b)) => {
                let err_str = err_b.to_string();
                error!("Leg A filled but Leg B failed: {}. Triggering rollback on Leg A!", err_str);
                self.circuit_breaker.record_error(&format!("Leg B failed: {}", err_str));

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
                        ArbitrageExecutionResult::LegBFailedUnwound {
                            leg_a_fill: fill_a,
                            unwind_resp: OrderResponse {
                                order_id: "UNWIND_FAILED".to_string(),
                                client_order_id: "".to_string(),
                                venue: exchange_a.venue_id(),
                                symbol: sym_a,
                                status: OrderStatus::Rejected,
                                original_qty: dec!(0),
                                filled_qty: dec!(0),
                                remaining_qty: dec!(0),
                                avg_fill_price: None,
                                fee: dec!(0),
                                fee_currency: "".to_string(),
                                raw_response: None,
                                timestamp: chrono::Utc::now(),
                            },
                            error_b: err_str,
                        }
                    }
                }
            }

            // Case 3: Leg B filled, Leg A failed -> EMERGENCY UNWIND Leg B
            (Err(err_a), Ok(fill_b)) => {
                let err_str = err_a.to_string();
                error!("Leg B filled but Leg A failed: {}. Triggering rollback on Leg B!", err_str);
                self.circuit_breaker.record_error(&format!("Leg A failed: {}", err_str));

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
                        ArbitrageExecutionResult::LegAFailedUnwound {
                            leg_b_fill: fill_b,
                            unwind_resp: OrderResponse {
                                order_id: "UNWIND_FAILED".to_string(),
                                client_order_id: "".to_string(),
                                venue: exchange_b.venue_id(),
                                symbol: sym_b,
                                status: OrderStatus::Rejected,
                                original_qty: dec!(0),
                                filled_qty: dec!(0),
                                remaining_qty: dec!(0),
                                avg_fill_price: None,
                                fee: dec!(0),
                                fee_currency: "".to_string(),
                                raw_response: None,
                                timestamp: chrono::Utc::now(),
                            },
                            error_a: err_str,
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
