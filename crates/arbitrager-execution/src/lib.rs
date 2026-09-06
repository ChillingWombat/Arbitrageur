pub mod smart_routing;
pub mod emergency_unwind;
pub mod coordinator;

pub use smart_routing::*;
pub use emergency_unwind::*;
pub use coordinator::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use async_trait::async_trait;
    use arbitrager_core::exchange::ExchangeAdapter;
    use arbitrager_core::order::{OrderRequest, OrderResponse};
    use arbitrager_core::types::{dec, OrderStatus, Side};
    use arbitrager_risk::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};

    struct MockExchange {
        name: String,
        should_fail: bool,
    }

    #[async_trait]
    impl ExchangeAdapter for MockExchange {
        fn venue_id(&self) -> arbitrager_core::types::VenueId {
            self.name.as_str().into()
        }
        fn name(&self) -> &str {
            &self.name
        }
        async fn fetch_markets(&self) -> anyhow::Result<Vec<arbitrager_core::market::MarketMetadata>> {
            Ok(vec![])
        }
        async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<arbitrager_core::quote::OrderBook> {
            Ok(arbitrager_core::quote::OrderBook::new(self.venue_id(), symbol.to_string()))
        }
        async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<arbitrager_core::quote::Ticker> {
            Ok(arbitrager_core::quote::Ticker {
                venue: self.venue_id(),
                symbol: symbol.to_string(),
                bid: dec!(100),
                ask: dec!(101),
                last: dec!(100.5),
                volume_24h: dec!(1000),
                timestamp: chrono::Utc::now(),
            })
        }
        async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<arbitrager_core::quote::FundingRate> {
            Ok(arbitrager_core::quote::FundingRate {
                venue: self.venue_id(),
                symbol: symbol.to_string(),
                rate: dec!(0.0001),
                predicted_rate: None,
                interval_hours: 8,
                next_funding_time: chrono::Utc::now(),
                timestamp: chrono::Utc::now(),
            })
        }
        async fn fetch_account(&self) -> anyhow::Result<arbitrager_core::position::AccountSnapshot> {
            Ok(arbitrager_core::position::AccountSnapshot {
                venue: self.venue_id(),
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
            if self.should_fail {
                anyhow::bail!("Simulated exchange order placement failure");
            }
            Ok(OrderResponse {
                order_id: "mock_order".to_string(),
                client_order_id: req.client_order_id,
                venue: self.venue_id(),
                symbol: req.symbol,
                status: OrderStatus::Filled,
                original_qty: req.quantity,
                filled_qty: req.quantity,
                remaining_qty: dec!(0),
                avg_fill_price: req.price,
                fee: dec!(0.1),
                fee_currency: "USDC".to_string(),
                raw_response: None,
                timestamp: chrono::Utc::now(),
            })
        }
        async fn cancel_order(&self, _req: arbitrager_core::order::CancelRequest) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_two_leg_success() {
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(MockExchange {
            name: "ex_a".into(),
            should_fail: false,
        });
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(MockExchange {
            name: "ex_b".into(),
            should_fail: false,
        });

        let cb = Arc::new(CircuitBreaker::new(CircuitBreakerConfig::default(), dec!(10000)));
        let router = SmartOrderRouter::new(dec!(10));
        let coordinator = AtomicArbitrageCoordinator::new(router, cb);

        let order_a = OrderRequest::new_limit(ex_a.venue_id(), "BTC-USDC", Side::Buy, dec!(60000), dec!(1.0));
        let order_b = OrderRequest::new_limit(ex_b.venue_id(), "BTC-USDC", Side::Sell, dec!(60100), dec!(1.0));

        let res = coordinator.execute_two_leg(&ex_a, order_a, &ex_b, order_b).await;
        match res {
            ArbitrageExecutionResult::Success { realized_profit, .. } => {
                assert!(realized_profit > dec!(0));
            }
            other => panic!("Expected Success, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_two_leg_emergency_unwind() {
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(MockExchange {
            name: "ex_a".into(),
            should_fail: false, // Leg A succeeds
        });
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(MockExchange {
            name: "ex_b".into(),
            should_fail: true,  // Leg B fails!
        });

        let cb = Arc::new(CircuitBreaker::new(CircuitBreakerConfig::default(), dec!(10000)));
        let router = SmartOrderRouter::new(dec!(10));
        let coordinator = AtomicArbitrageCoordinator::new(router, cb);

        let order_a = OrderRequest::new_limit(ex_a.venue_id(), "BTC-USDC", Side::Buy, dec!(60000), dec!(1.0));
        let order_b = OrderRequest::new_limit(ex_b.venue_id(), "BTC-USDC", Side::Sell, dec!(60100), dec!(1.0));

        let res = coordinator.execute_two_leg(&ex_a, order_a, &ex_b, order_b).await;
        match res {
            ArbitrageExecutionResult::LegBFailedUnwound { unwind_resp, .. } => {
                assert_eq!(unwind_resp.status, OrderStatus::Filled);
            }
            other => panic!("Expected LegBFailedUnwound, got {:?}", other),
        }
    }
}
