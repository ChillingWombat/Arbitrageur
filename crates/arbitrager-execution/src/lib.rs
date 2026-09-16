pub mod smart_routing;
pub mod emergency_unwind;
pub mod coordinator;
pub mod virtual_ledger;
pub mod passive_unwind;

pub use smart_routing::*;
pub use emergency_unwind::*;
pub use coordinator::*;
pub use virtual_ledger::*;
pub use passive_unwind::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use async_trait::async_trait;
    use arbitrager_core::exchange::ExchangeAdapter;
    use arbitrager_core::order::{OrderRequest, OrderResponse};
    use arbitrager_core::types::{dec, Decimal, OrderStatus, Side};
    use arbitrager_risk::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};

    /// Per-call scripted behavior for MockExchange::place_order.
    #[derive(Clone)]
    #[allow(clippy::large_enum_variant)]
    enum MockResponse {
        /// place_order returns Err
        Fail,
        /// Ok with the full requested quantity filled
        Full,
        /// Ok with exactly this quantity filled (< requested)
        Partial(Decimal),
        /// Ok with zero quantity filled
        ZeroFill,
        /// Ok returning a verbatim prebuilt response, used for malformed
        /// adapter responses (wrong venue/symbol/qty/price/fee)
        Raw(OrderResponse),
    }

    /// Configurable mock venue that records every order it receives.
    struct MockExchange {
        name: String,
        script: Vec<MockResponse>,
        fallback: MockResponse,
        calls: Arc<Mutex<Vec<OrderRequest>>>,
    }

    impl MockExchange {
        fn new(name: &str, script: Vec<MockResponse>, fallback: MockResponse) -> Self {
            Self {
                name: name.into(),
                script,
                fallback,
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn call_log(&self) -> Arc<Mutex<Vec<OrderRequest>>> {
            self.calls.clone()
        }
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
            let call_idx = {
                let mut log = self.calls.lock().unwrap();
                log.push(req.clone());
                log.len() - 1
            };
            let behavior = self
                .script
                .get(call_idx)
                .cloned()
                .unwrap_or_else(|| self.fallback.clone());

            if let MockResponse::Raw(resp) = behavior {
                return Ok(resp);
            }

            let filled_qty = match behavior {
                MockResponse::Fail => anyhow::bail!("Simulated exchange order placement failure"),
                MockResponse::Full => req.quantity,
                MockResponse::Partial(qty) => qty,
                MockResponse::ZeroFill => Decimal::ZERO,
                MockResponse::Raw(_) => unreachable!("handled above"),
            };
            let remaining_qty = req.quantity - filled_qty;
            let status = if filled_qty == req.quantity && remaining_qty == Decimal::ZERO {
                OrderStatus::Filled
            } else if filled_qty > Decimal::ZERO {
                OrderStatus::PartiallyFilled
            } else {
                OrderStatus::Rejected
            };

            Ok(OrderResponse {
                order_id: format!("mock_order_{}_{}", self.name, call_idx),
                client_order_id: req.client_order_id,
                venue: self.venue_id(),
                symbol: req.symbol,
                status,
                original_qty: req.quantity,
                filled_qty,
                remaining_qty,
                avg_fill_price: if filled_qty > Decimal::ZERO {
                    Some(req.price.unwrap_or(dec!(100)))
                } else {
                    None
                },
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

    fn make_coordinator() -> (AtomicArbitrageCoordinator, Arc<CircuitBreaker>) {
        let cb = Arc::new(CircuitBreaker::new(CircuitBreakerConfig::default(), dec!(10000)));
        let router = SmartOrderRouter::new(dec!(10));
        (AtomicArbitrageCoordinator::new(router, cb.clone()), cb)
    }

    #[tokio::test]
    async fn test_two_leg_success() {
        let mock_a = MockExchange::new("ex_a", vec![], MockResponse::Full);
        let mock_b = MockExchange::new("ex_b", vec![], MockResponse::Full);
        let log_a = mock_a.call_log();
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(mock_b);

        let (coordinator, _cb) = make_coordinator();

        let order_a = OrderRequest::new_limit(ex_a.venue_id(), "BTC-USDC", Side::Buy, dec!(60000), dec!(1.0));
        let order_b = OrderRequest::new_limit(ex_b.venue_id(), "BTC-USDC", Side::Sell, dec!(60100), dec!(1.0));

        let res = coordinator.execute_two_leg(&ex_a, order_a, &ex_b, order_b).await;
        match res {
            ArbitrageExecutionResult::Success { realized_profit, leg_a, leg_b } => {
                // Generic coordinator reports immediate realized cash P&L: negative reported fees only
                assert_eq!(realized_profit, -(leg_a.fee + leg_b.fee));
                assert_eq!(realized_profit, dec!(-0.2));
            }
            other => panic!("Expected Success, got {:?}", other),
        }
        // No unwinds: each venue saw exactly the original order
        assert_eq!(log_a.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_two_leg_emergency_unwind() {
        let mock_a = MockExchange::new("ex_a", vec![], MockResponse::Full);
        let log_a = mock_a.call_log();
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(MockExchange::new("ex_b", vec![], MockResponse::Fail));

        let (coordinator, _cb) = make_coordinator();

        let order_a = OrderRequest::new_limit(ex_a.venue_id(), "BTC-USDC", Side::Buy, dec!(60000), dec!(1.0));
        let order_b = OrderRequest::new_limit(ex_b.venue_id(), "BTC-USDC", Side::Sell, dec!(60100), dec!(1.0));

        let res = coordinator.execute_two_leg(&ex_a, order_a, &ex_b, order_b).await;
        match res {
            ArbitrageExecutionResult::LegBFailedUnwound { unwind_resp, .. } => {
                assert_eq!(unwind_resp.status, OrderStatus::Filled);
            }
            other => panic!("Expected LegBFailedUnwound, got {:?}", other),
        }

        // Leg A venue received the original buy plus the opposite-side unwind sell
        let calls = log_a.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].side, Side::Sell);
        assert_eq!(calls[1].quantity, dec!(1.0));
    }

    #[tokio::test]
    async fn test_incomplete_fills_trim_to_matched_pair() {
        // Leg A partially fills 0.4 of 1.0; Leg B fills fully -> B's 0.6
        // excess is trimmed and the matched 0.4 pair stays open.
        let mock_a = MockExchange::new("ex_a", vec![MockResponse::Partial(dec!(0.4))], MockResponse::Full);
        let mock_b = MockExchange::new("ex_b", vec![], MockResponse::Full);
        let log_a = mock_a.call_log();
        let log_b = mock_b.call_log();
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(mock_b);

        let (coordinator, cb) = make_coordinator();

        let order_a = OrderRequest::new_limit(ex_a.venue_id(), "BTC-USDC", Side::Buy, dec!(60000), dec!(1.0));
        let order_b = OrderRequest::new_limit(ex_b.venue_id(), "BTC-USDC", Side::Sell, dec!(60100), dec!(1.0));

        let res = coordinator.execute_two_leg(&ex_a, order_a, &ex_b, order_b).await;
        match res {
            ArbitrageExecutionResult::PartialPairKept { matched_qty, unwind_a, unwind_b, .. } => {
                assert_eq!(matched_qty, dec!(0.4));
                assert!(unwind_a.is_none());
                assert_eq!(unwind_b.expect("B excess must trim").filled_qty, dec!(0.6));
            }
            other => panic!("Expected PartialPairKept, got {:?}", other),
        }
        assert!(!cb.is_tripped());

        // A saw only its original order; B saw original plus the trim.
        let calls_a = log_a.lock().unwrap();
        assert_eq!(calls_a.len(), 1);
        assert_eq!(calls_a[0].side, Side::Buy);

        let calls_b = log_b.lock().unwrap();
        assert_eq!(calls_b.len(), 2);
        assert_eq!(calls_b[0].side, Side::Sell);
        assert_eq!(calls_b[1].side, Side::Buy);
        assert_eq!(calls_b[1].quantity, dec!(0.6));
    }

    #[tokio::test]
    async fn test_partial_pair_kept_includes_trim_price_pnl() {
        // Same shape as above: A buys 0.4 @60000, B sells 1.0 @60100, the
        // 0.6 short excess is bought back by the market trim (mock: @100).
        let mock_a = MockExchange::new("ex_a", vec![MockResponse::Partial(dec!(0.4))], MockResponse::Full);
        let mock_b = MockExchange::new("ex_b", vec![], MockResponse::Full);
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(mock_b);

        let (coordinator, _) = make_coordinator();

        let order_a = OrderRequest::new_limit(ex_a.venue_id(), "BTC-USDC", Side::Buy, dec!(60000), dec!(1.0));
        let order_b = OrderRequest::new_limit(ex_b.venue_id(), "BTC-USDC", Side::Sell, dec!(60100), dec!(1.0));

        let res = coordinator.execute_two_leg(&ex_a, order_a, &ex_b, order_b).await;
        match res {
            ArbitrageExecutionResult::PartialPairKept { matched_qty, realized_profit, .. } => {
                assert_eq!(matched_qty, dec!(0.4));
                assert_eq!(realized_profit, dec!(36000) - dec!(0.3));
            }
            other => panic!("Expected PartialPairKept, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_zero_fill_ok_plus_err_no_unwind() {
        let mock_a = MockExchange::new("ex_a", vec![MockResponse::ZeroFill], MockResponse::Full);
        let log_a = mock_a.call_log();
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(MockExchange::new("ex_b", vec![], MockResponse::Fail));

        let (coordinator, _cb) = make_coordinator();

        let order_a = OrderRequest::new_limit(ex_a.venue_id(), "BTC-USDC", Side::Buy, dec!(60000), dec!(1.0));
        let order_b = OrderRequest::new_limit(ex_b.venue_id(), "BTC-USDC", Side::Sell, dec!(60100), dec!(1.0));

        let res = coordinator.execute_two_leg(&ex_a, order_a, &ex_b, order_b).await;
        match res {
            ArbitrageExecutionResult::BothFailed { error_a, .. } => {
                assert!(error_a.contains("zero filled quantity"), "error_a: {}", error_a);
            }
            other => panic!("Expected BothFailed, got {:?}", other),
        }

        // Zero-fill leg received only its original order; no unwind dispatched
        assert_eq!(log_a.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_incomplete_unwind_trips_circuit_breaker() {
        // Leg A fills fully, Leg B errors; Leg A's unwind only partially fills
        let mock_a = MockExchange::new(
            "ex_a",
            vec![MockResponse::Full, MockResponse::Partial(dec!(0.5))],
            MockResponse::Full,
        );
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(MockExchange::new("ex_b", vec![], MockResponse::Fail));

        let (coordinator, cb) = make_coordinator();

        let order_a = OrderRequest::new_limit(ex_a.venue_id(), "BTC-USDC", Side::Buy, dec!(60000), dec!(1.0));
        let order_b = OrderRequest::new_limit(ex_b.venue_id(), "BTC-USDC", Side::Sell, dec!(60100), dec!(1.0));

        let res = coordinator.execute_two_leg(&ex_a, order_a, &ex_b, order_b).await;
        match res {
            ArbitrageExecutionResult::UnresolvedExposure { leg_a_fill, leg_b_fill, error_a, error_b } => {
                let fill = leg_a_fill.expect("leg A fill should be reported");
                assert_eq!(fill.filled_qty, dec!(1.0));
                assert!(leg_b_fill.is_none());
                assert!(error_a.contains("unwind failed"), "error_a: {}", error_a);
                assert!(!error_b.is_empty());
            }
            other => panic!("Expected UnresolvedExposure, got {:?}", other),
        }
        assert!(cb.is_tripped());
        assert!(cb.trip_reason().unwrap().contains("Unwind failed"));
    }

    #[tokio::test]
    async fn test_unresolved_exposure_mirror_leg() {
        // Leg B fills fully, Leg A errors; Leg B's unwind fails outright
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(MockExchange::new("ex_a", vec![], MockResponse::Fail));
        let mock_b = MockExchange::new(
            "ex_b",
            vec![MockResponse::Full, MockResponse::Fail],
            MockResponse::Fail,
        );
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(mock_b);

        let (coordinator, cb) = make_coordinator();

        let order_a = OrderRequest::new_limit(ex_a.venue_id(), "BTC-USDC", Side::Buy, dec!(60000), dec!(1.0));
        let order_b = OrderRequest::new_limit(ex_b.venue_id(), "BTC-USDC", Side::Sell, dec!(60100), dec!(1.0));

        let res = coordinator.execute_two_leg(&ex_a, order_a, &ex_b, order_b).await;
        match res {
            ArbitrageExecutionResult::UnresolvedExposure { leg_a_fill, leg_b_fill, error_b, .. } => {
                assert!(leg_a_fill.is_none());
                let fill = leg_b_fill.expect("leg B fill should be reported");
                assert_eq!(fill.filled_qty, dec!(1.0));
                assert!(error_b.contains("unwind failed"), "error_b: {}", error_b);
            }
            other => panic!("Expected UnresolvedExposure, got {:?}", other),
        }
        assert!(cb.is_tripped());
        assert!(cb.trip_reason().unwrap().contains("Unwind failed"));
    }

    fn filled_response(venue: &str, symbol: &str, qty: Decimal) -> OrderResponse {
        OrderResponse {
            order_id: "malformed-1".into(),
            client_order_id: "c".into(),
            venue: venue.into(),
            symbol: symbol.into(),
            status: OrderStatus::Filled,
            original_qty: qty,
            filled_qty: qty,
            remaining_qty: Decimal::ZERO,
            avg_fill_price: Some(dec!(100)),
            fee: dec!(0.1),
            fee_currency: "USDC".into(),
            raw_response: None,
            timestamp: chrono::Utc::now(),
        }
    }

    fn attributed_fill(order: &OrderRequest, qty: Decimal) -> OrderResponse {
        OrderResponse {
            order_id: "resp-1".into(),
            client_order_id: order.client_order_id.clone(),
            venue: order.venue.clone(),
            symbol: order.symbol.clone(),
            status: OrderStatus::Filled,
            original_qty: qty,
            filled_qty: qty,
            remaining_qty: Decimal::ZERO,
            avg_fill_price: Some(dec!(100)),
            fee: dec!(0.1),
            fee_currency: "USDC".into(),
            raw_response: None,
            timestamp: chrono::Utc::now(),
        }
    }

    async fn assert_malformed_fill_unwound(mutate: impl FnOnce(&mut OrderResponse)) {
        let order_a = OrderRequest::new_limit(
            "ex_a".into(), "BTC-USDC", Side::Buy, dec!(60000), dec!(1.0),
        );
        let mut resp = attributed_fill(&order_a, dec!(1.0));
        mutate(&mut resp);
        let mock_a = MockExchange::new("ex_a", vec![MockResponse::Raw(resp)], MockResponse::Full);
        let log_a = mock_a.call_log();
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(MockExchange::new("ex_b", vec![], MockResponse::Full));
        let (coordinator, _cb) = make_coordinator();

        let order_b = OrderRequest::new_limit(ex_b.venue_id(), "BTC-USDC", Side::Sell, dec!(60100), dec!(1.0));
        let res = coordinator.execute_two_leg(&ex_a, order_a, &ex_b, order_b).await;
        match res {
            ArbitrageExecutionResult::BothFailed { error_a, .. } => {
                assert!(error_a.contains("Filled"), "error_a: {}", error_a);
                assert!(error_a.contains("unwound"), "error_a: {}", error_a);
            }
            other => panic!("Expected BothFailed, got {:?}", other),
        }
        let calls = log_a.lock().unwrap();
        assert_eq!(calls.len(), 2, "malformed fill must be compensated");
        assert_eq!(calls[1].side, Side::Sell);
    }

    async fn assert_ambiguous_fill_unresolved(mutate: impl FnOnce(&mut OrderResponse)) {
        let order_a = OrderRequest::new_limit(
            "ex_a".into(), "BTC-USDC", Side::Buy, dec!(60000), dec!(1.0),
        );
        let mut resp = attributed_fill(&order_a, dec!(1.0));
        mutate(&mut resp);
        let mock_a = MockExchange::new("ex_a", vec![MockResponse::Raw(resp)], MockResponse::Full);
        let log_a = mock_a.call_log();
        let mock_b = MockExchange::new("ex_b", vec![], MockResponse::Full);
        let log_b = mock_b.call_log();
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(mock_b);
        let (coordinator, cb) = make_coordinator();

        let order_b = OrderRequest::new_limit(ex_b.venue_id(), "BTC-USDC", Side::Sell, dec!(60100), dec!(1.0));
        let res = coordinator.execute_two_leg(&ex_a, order_a, &ex_b, order_b).await;
        match res {
            ArbitrageExecutionResult::UnresolvedExposure { leg_a_fill, error_a, error_b, .. } => {
                assert!(leg_a_fill.is_some());
                assert!(error_a.contains("identity mismatch"), "error_a: {}", error_a);
                assert!(error_b.contains("unwound"), "error_b: {}", error_b);
            }
            other => panic!("Expected UnresolvedExposure, got {:?}", other),
        }
        assert!(cb.is_tripped());
        assert_eq!(log_a.lock().unwrap().len(), 1, "ambiguous leg must not be unwound");
        let calls_b = log_b.lock().unwrap();
        assert_eq!(calls_b.len(), 2, "known counterleg must be unwound");
        assert_eq!(calls_b[1].side, Side::Buy);
    }

    #[tokio::test]
    async fn test_fill_wrong_venue_is_unresolved_not_unwound() {
        assert_ambiguous_fill_unresolved(|r| r.venue = "evil_venue".into()).await;
    }

    #[tokio::test]
    async fn test_fill_wrong_symbol_is_unresolved_not_unwound() {
        assert_ambiguous_fill_unresolved(|r| r.symbol = "WRONG-SYMBOL".into()).await;
    }

    #[tokio::test]
    async fn test_fill_wrong_client_order_id_is_unresolved() {
        assert_ambiguous_fill_unresolved(|r| r.client_order_id = "not-our-request".into()).await;
    }

    #[tokio::test]
    async fn test_fill_empty_order_id_is_unresolved() {
        assert_ambiguous_fill_unresolved(|r| r.order_id = String::new()).await;
    }

    #[tokio::test]
    async fn test_ambiguous_fill_with_failed_counterleg_is_unresolved() {
        let order_a = OrderRequest::new_limit(
            "ex_a".into(), "BTC-USDC", Side::Buy, dec!(60000), dec!(1.0),
        );
        let mut resp = attributed_fill(&order_a, dec!(1.0));
        resp.venue = "evil_venue".into();
        let mock_a = MockExchange::new("ex_a", vec![MockResponse::Raw(resp)], MockResponse::Full);
        let log_a = mock_a.call_log();
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(MockExchange::new("ex_b", vec![], MockResponse::Fail));
        let (coordinator, cb) = make_coordinator();

        let order_b = OrderRequest::new_limit(ex_b.venue_id(), "BTC-USDC", Side::Sell, dec!(60100), dec!(1.0));
        let res = coordinator.execute_two_leg(&ex_a, order_a, &ex_b, order_b).await;
        match res {
            ArbitrageExecutionResult::UnresolvedExposure { leg_a_fill, error_a, .. } => {
                assert!(leg_a_fill.is_some());
                assert!(error_a.contains("identity mismatch"), "error_a: {}", error_a);
            }
            other => panic!("Expected UnresolvedExposure, got {:?}", other),
        }
        assert!(cb.is_tripped());
        assert_eq!(log_a.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_fill_mismatched_original_qty_is_unwound() {
        assert_malformed_fill_unwound(|r| r.original_qty = dec!(999)).await;
    }

    #[tokio::test]
    async fn test_fill_missing_or_zero_price_is_unwound() {
        assert_malformed_fill_unwound(|r| r.avg_fill_price = None).await;
        assert_malformed_fill_unwound(|r| r.avg_fill_price = Some(Decimal::ZERO)).await;
    }

    #[tokio::test]
    async fn test_fill_negative_fee_is_unwound() {
        assert_malformed_fill_unwound(|r| r.fee = dec!(-0.5)).await;
    }

    async fn assert_bad_unwind_unresolved(bad_unwind: OrderResponse) {
        let mock_a = MockExchange::new(
            "ex_a",
            vec![MockResponse::Full, MockResponse::Raw(bad_unwind)],
            MockResponse::Full,
        );
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(MockExchange::new("ex_b", vec![], MockResponse::Fail));

        let (coordinator, cb) = make_coordinator();
        let order_a = OrderRequest::new_limit(ex_a.venue_id(), "BTC-USDC", Side::Buy, dec!(60000), dec!(1.0));
        let order_b = OrderRequest::new_limit(ex_b.venue_id(), "BTC-USDC", Side::Sell, dec!(60100), dec!(1.0));
        let res = coordinator.execute_two_leg(&ex_a, order_a, &ex_b, order_b).await;
        match res {
            ArbitrageExecutionResult::UnresolvedExposure { leg_a_fill, error_a, .. } => {
                assert!(leg_a_fill.is_some());
                assert!(error_a.contains("unwind failed"), "error_a: {}", error_a);
                assert!(error_a.contains("Emergency unwind incomplete"), "error_a: {}", error_a);
            }
            other => panic!("Expected UnresolvedExposure, got {:?}", other),
        }
        assert!(cb.is_tripped());
    }

    #[tokio::test]
    async fn test_malformed_unwind_response_is_unresolved_exposure() {
        let mut bad_unwind = filled_response("ex_a", "BTC-USDC", dec!(1.0));
        bad_unwind.symbol = "WRONG-SYMBOL".into();
        assert_bad_unwind_unresolved(bad_unwind).await;
    }

    #[tokio::test]
    async fn test_unwind_wrong_client_order_id_is_unresolved_exposure() {
        let mut bad_unwind = filled_response("ex_a", "BTC-USDC", dec!(1.0));
        bad_unwind.client_order_id = "not-the-unwind-order".into();
        assert_bad_unwind_unresolved(bad_unwind).await;
    }

    #[tokio::test]
    async fn test_both_partial_keeps_min_fill() {
        // A 0.4/1.0, B 0.6/1.0: matched 0.4 kept; only B's 0.2 excess unwinds.
        let mock_a = MockExchange::new("ex_a", vec![MockResponse::Partial(dec!(0.4))], MockResponse::Full);
        let log_a = mock_a.call_log();
        let mock_b = MockExchange::new("ex_b", vec![MockResponse::Partial(dec!(0.6))], MockResponse::Full);
        let log_b = mock_b.call_log();
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(mock_b);

        let (coordinator, cb) = make_coordinator();
        let order_a = OrderRequest::new_limit(ex_a.venue_id(), "BTC-USDC", Side::Buy, dec!(60000), dec!(1.0));
        let order_b = OrderRequest::new_limit(ex_b.venue_id(), "BTC-USDC", Side::Sell, dec!(60100), dec!(1.0));

        let res = coordinator.execute_two_leg(&ex_a, order_a, &ex_b, order_b).await;
        match res {
            ArbitrageExecutionResult::PartialPairKept { matched_qty, unwind_a, unwind_b, .. } => {
                assert_eq!(matched_qty, dec!(0.4));
                assert!(unwind_a.is_none());
                assert_eq!(unwind_b.expect("B excess must trim").filled_qty, dec!(0.2));
            }
            other => panic!("Expected PartialPairKept, got {:?}", other),
        }
        assert!(!cb.is_tripped());
        assert_eq!(log_a.lock().unwrap().len(), 1);
        let calls_b = log_b.lock().unwrap();
        assert_eq!(calls_b.len(), 2);
        assert_eq!(calls_b[1].quantity, dec!(0.2));
    }

    #[tokio::test]
    async fn test_trim_of_excess_failure_is_unresolved_exposure() {
        // A partial 0.4, B full 1.0, but B's 0.6 trim fails outright.
        let mock_a = MockExchange::new("ex_a", vec![MockResponse::Partial(dec!(0.4))], MockResponse::Full);
        let mock_b = MockExchange::new(
            "ex_b",
            vec![MockResponse::Full, MockResponse::Fail],
            MockResponse::Fail,
        );
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(mock_b);

        let (coordinator, cb) = make_coordinator();
        let order_a = OrderRequest::new_limit(ex_a.venue_id(), "BTC-USDC", Side::Buy, dec!(60000), dec!(1.0));
        let order_b = OrderRequest::new_limit(ex_b.venue_id(), "BTC-USDC", Side::Sell, dec!(60100), dec!(1.0));

        let res = coordinator.execute_two_leg(&ex_a, order_a, &ex_b, order_b).await;
        match res {
            ArbitrageExecutionResult::UnresolvedExposure { leg_a_fill, leg_b_fill, error_b, .. } => {
                assert_eq!(leg_a_fill.expect("A fill reported").filled_qty, dec!(0.4));
                assert_eq!(leg_b_fill.expect("B fill reported").filled_qty, dec!(1.0));
                assert!(error_b.contains("unwind failed"), "error_b: {}", error_b);
            }
            other => panic!("Expected UnresolvedExposure, got {:?}", other),
        }
        assert!(cb.is_tripped());
    }

    #[tokio::test]
    async fn test_equal_partial_fills_keep_pair_without_trim() {
        // Both legs partial at the same 0.4: matched 0.4, no excess to trim.
        let mock_a = MockExchange::new("ex_a", vec![MockResponse::Partial(dec!(0.4))], MockResponse::Full);
        let mock_b = MockExchange::new("ex_b", vec![MockResponse::Partial(dec!(0.4))], MockResponse::Full);
        let log_a = mock_a.call_log();
        let log_b = mock_b.call_log();
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(mock_b);

        let (coordinator, cb) = make_coordinator();
        let order_a = OrderRequest::new_limit(ex_a.venue_id(), "BTC-USDC", Side::Buy, dec!(60000), dec!(1.0));
        let order_b = OrderRequest::new_limit(ex_b.venue_id(), "BTC-USDC", Side::Sell, dec!(60100), dec!(1.0));

        let res = coordinator.execute_two_leg(&ex_a, order_a, &ex_b, order_b).await;
        match res {
            ArbitrageExecutionResult::PartialPairKept { matched_qty, unwind_a, unwind_b, .. } => {
                assert_eq!(matched_qty, dec!(0.4));
                assert!(unwind_a.is_none());
                assert!(unwind_b.is_none());
            }
            other => panic!("Expected PartialPairKept, got {:?}", other),
        }
        assert!(!cb.is_tripped());
        assert_eq!(log_a.lock().unwrap().len(), 1);
        assert_eq!(log_b.lock().unwrap().len(), 1);
    }
}
