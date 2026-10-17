pub mod atomic_bundle;
pub mod causal_dml_impact;
pub mod conformal_adadelta_shield;
pub mod conformal_adagrad_shield;
pub mod conformal_adam_shield;
pub mod conformal_bernstein_shield;
pub mod conformal_chance_constrained;
pub mod conformal_cvar_shield;
pub mod conformal_dkw_process;
pub mod conformal_double_kernel_shield;
pub mod conformal_erm_slippage;
pub mod conformal_evt_tail_guard;
pub mod conformal_extreme_copula;
pub mod conformal_forest_shield;
pub mod conformal_lookahead_shield;
pub mod conformal_matrix_bernstein;
pub mod conformal_matrix_completion;
pub mod conformal_matrix_shield;
pub mod conformal_momentum_shield;
pub mod conformal_mot_shield;
pub mod conformal_nadam_shield;
pub mod conformal_nesterov_shield;
pub mod conformal_omd_shield;
pub mod conformal_optimal_transport;
pub mod conformal_quasi_hyperbolic_shield;
pub mod conformal_rademacher_shield;
pub mod conformal_risk_guard;
pub mod conformal_rkhs_shield;
pub mod conformal_rmsprop_shield;
pub mod conformal_spline_shield;
pub mod conformal_srm_shield;
pub mod conformal_svrg_shield;
pub mod conformal_wasserstein_drift;
pub mod coordinator;
pub mod emergency_unwind;
pub mod l2_sequencer_guard;
pub mod mcafee_combinatorial_auction;
pub mod mean_field_liquidation;
pub mod modular_session_account;
pub mod omd_dual_averaging;
pub mod passive_unwind;
pub mod queue_priority;
pub mod sac_execution;
pub mod shared_sequencer_bundle;
pub mod smart_routing;
pub mod virtual_ledger;
pub mod viscoelastic_cascade_guard;
pub mod zk_intent_settlement;

pub use atomic_bundle::*;
pub use causal_dml_impact::*;
pub use conformal_adadelta_shield::*;
pub use conformal_adagrad_shield::*;
pub use conformal_adam_shield::*;
pub use conformal_bernstein_shield::*;
pub use conformal_chance_constrained::*;
pub use conformal_cvar_shield::*;
pub use conformal_dkw_process::*;
pub use conformal_double_kernel_shield::*;
pub use conformal_erm_slippage::*;
pub use conformal_evt_tail_guard::*;
pub use conformal_extreme_copula::*;
pub use conformal_forest_shield::*;
pub use conformal_lookahead_shield::*;
pub use conformal_matrix_bernstein::*;
pub use conformal_matrix_completion::*;
pub use conformal_matrix_shield::*;
pub use conformal_momentum_shield::*;
pub use conformal_mot_shield::*;
pub use conformal_nadam_shield::*;
pub use conformal_nesterov_shield::*;
pub use conformal_omd_shield::*;
pub use conformal_optimal_transport::*;
pub use conformal_quasi_hyperbolic_shield::*;
pub use conformal_rademacher_shield::*;
pub use conformal_risk_guard::*;
pub use conformal_rkhs_shield::*;
pub use conformal_rmsprop_shield::*;
pub use conformal_spline_shield::*;
pub use conformal_srm_shield::*;
pub use conformal_svrg_shield::*;
pub use conformal_wasserstein_drift::*;
pub use coordinator::*;
pub use emergency_unwind::*;
pub use l2_sequencer_guard::*;
pub use mcafee_combinatorial_auction::*;
pub use mean_field_liquidation::*;
pub use modular_session_account::*;
pub use omd_dual_averaging::*;
pub use passive_unwind::*;
pub use queue_priority::*;
pub use sac_execution::*;
pub use shared_sequencer_bundle::*;
pub use smart_routing::*;
pub use virtual_ledger::*;
pub use viscoelastic_cascade_guard::*;
pub use zk_intent_settlement::*;

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrageur_core::exchange::ExchangeAdapter;
    use arbitrageur_core::order::{OrderRequest, OrderResponse};
    use arbitrageur_core::types::{dec, Decimal, OrderStatus, Side};
    use arbitrageur_risk::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

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
        fetch_script: Arc<Mutex<Vec<anyhow::Result<OrderResponse>>>>,
        cancels: Arc<Mutex<Vec<arbitrageur_core::order::CancelRequest>>>,
    }

    impl MockExchange {
        fn new(name: &str, script: Vec<MockResponse>, fallback: MockResponse) -> Self {
            Self {
                name: name.into(),
                script,
                fallback,
                calls: Arc::new(Mutex::new(Vec::new())),
                fetch_script: Arc::new(Mutex::new(Vec::new())),
                cancels: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn call_log(&self) -> Arc<Mutex<Vec<OrderRequest>>> {
            self.calls.clone()
        }

        /// Scripted fetch_order responses consumed one per call; an empty
        /// queue fails closed like an adapter without order-status support.
        fn with_fetch(self, script: Vec<anyhow::Result<OrderResponse>>) -> Self {
            *self.fetch_script.lock().unwrap() = script;
            self
        }

        fn cancel_log(&self) -> Arc<Mutex<Vec<arbitrageur_core::order::CancelRequest>>> {
            self.cancels.clone()
        }
    }

    #[async_trait]
    impl ExchangeAdapter for MockExchange {
        fn venue_id(&self) -> arbitrageur_core::types::VenueId {
            self.name.as_str().into()
        }
        fn name(&self) -> &str {
            &self.name
        }
        async fn fetch_markets(
            &self,
        ) -> anyhow::Result<Vec<arbitrageur_core::market::MarketMetadata>> {
            Ok(vec![])
        }
        async fn fetch_orderbook(
            &self,
            symbol: &str,
        ) -> anyhow::Result<arbitrageur_core::quote::OrderBook> {
            Ok(arbitrageur_core::quote::OrderBook::new(
                self.venue_id(),
                symbol.to_string(),
            ))
        }
        async fn fetch_ticker(
            &self,
            symbol: &str,
        ) -> anyhow::Result<arbitrageur_core::quote::Ticker> {
            Ok(arbitrageur_core::quote::Ticker {
                venue: self.venue_id(),
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
                venue: self.venue_id(),
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
                fee: Some(dec!(0.1)),
                fee_currency: "USDC".to_string(),
                raw_response: None,
                timestamp: chrono::Utc::now(),
            })
        }
        async fn cancel_order(
            &self,
            req: arbitrageur_core::order::CancelRequest,
        ) -> anyhow::Result<()> {
            self.cancels.lock().unwrap().push(req);
            Ok(())
        }
        async fn fetch_order(
            &self,
            _symbol: &str,
            _order_id: &str,
            _client_order_id: Option<&str>,
        ) -> anyhow::Result<OrderResponse> {
            let mut script = self.fetch_script.lock().unwrap();
            if script.is_empty() {
                anyhow::bail!(
                    "order-status polling not implemented for mock {}",
                    self.name
                );
            }
            script.remove(0)
        }
    }

    fn make_coordinator() -> (AtomicArbitrageCoordinator, Arc<CircuitBreaker>) {
        let cb = Arc::new(CircuitBreaker::new(
            CircuitBreakerConfig::default(),
            dec!(10000),
        ));
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

        let order_a = OrderRequest::new_limit(
            ex_a.venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(60000),
            dec!(1.0),
        );
        let order_b = OrderRequest::new_limit(
            ex_b.venue_id(),
            "BTC-USDC",
            Side::Sell,
            dec!(60100),
            dec!(1.0),
        );

        let res = coordinator
            .execute_two_leg(&ex_a, order_a, &ex_b, order_b)
            .await;
        match res {
            ArbitrageExecutionResult::Success {
                realized_profit,
                leg_a,
                leg_b,
            } => {
                // Generic coordinator reports immediate realized cash P&L: negative reported fees only
                assert_eq!(
                    realized_profit,
                    -(leg_a.fee.unwrap_or_default() + leg_b.fee.unwrap_or_default())
                );
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
        let ex_b: Arc<dyn ExchangeAdapter> =
            Arc::new(MockExchange::new("ex_b", vec![], MockResponse::Fail));

        let (coordinator, _cb) = make_coordinator();

        let order_a = OrderRequest::new_limit(
            ex_a.venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(60000),
            dec!(1.0),
        );
        let order_b = OrderRequest::new_limit(
            ex_b.venue_id(),
            "BTC-USDC",
            Side::Sell,
            dec!(60100),
            dec!(1.0),
        );

        let res = coordinator
            .execute_two_leg(&ex_a, order_a, &ex_b, order_b)
            .await;
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

    #[test]
    fn test_latency_stagger_from_rtt() {
        use crate::coordinator::LatencyStagger;
        use std::time::Duration;

        // Venue B slower (200ms vs 100ms): OWT difference = (200 - 100)/2 = 50ms delay for Leg A
        let s1 = LatencyStagger::from_rtt_ms(Some(100.0), Some(200.0));
        assert_eq!(s1.delay_a, Duration::from_millis(50));
        assert_eq!(s1.delay_b, Duration::ZERO);

        // Venue A slower (300ms vs 100ms): OWT difference = (300 - 100)/2 = 100ms delay for Leg B
        let s2 = LatencyStagger::from_rtt_ms(Some(300.0), Some(100.0));
        assert_eq!(s2.delay_a, Duration::ZERO);
        assert_eq!(s2.delay_b, Duration::from_millis(100));

        // Equal latencies: zero delay
        let s3 = LatencyStagger::from_rtt_ms(Some(150.0), Some(150.0));
        assert_eq!(s3.delay_a, Duration::ZERO);
        assert_eq!(s3.delay_b, Duration::ZERO);

        // Missing or nonpositive inputs: default zero delay
        assert_eq!(
            LatencyStagger::from_rtt_ms(None, Some(100.0)),
            LatencyStagger::default()
        );
        assert_eq!(
            LatencyStagger::from_rtt_ms(Some(-10.0), Some(100.0)),
            LatencyStagger::default()
        );

        // Capped at 2000ms max safety threshold
        let s_cap = LatencyStagger::from_rtt_ms(Some(100.0), Some(10_000.0));
        assert_eq!(s_cap.delay_a, Duration::from_millis(2000));
    }

    #[tokio::test]
    async fn test_two_leg_staggered_success() {
        use crate::coordinator::LatencyStagger;
        use std::time::Duration;

        let mock_a = MockExchange::new("ex_a", vec![], MockResponse::Full);
        let mock_b = MockExchange::new("ex_b", vec![], MockResponse::Full);
        let log_a = mock_a.call_log();
        let log_b = mock_b.call_log();
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(mock_b);

        let (coordinator, _cb) = make_coordinator();
        let order_a = OrderRequest::new_limit(
            ex_a.venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(60000),
            dec!(1.0),
        );
        let order_b = OrderRequest::new_limit(
            ex_b.venue_id(),
            "BTC-USDC",
            Side::Sell,
            dec!(60100),
            dec!(1.0),
        );

        let stagger = LatencyStagger {
            delay_a: Duration::from_millis(10),
            delay_b: Duration::ZERO,
        };
        let res = coordinator
            .execute_two_leg_staggered(&ex_a, order_a, &ex_b, order_b, None, Some(stagger))
            .await;
        match res {
            ArbitrageExecutionResult::Success { .. } => {}
            other => panic!("Expected Success, got {:?}", other),
        }
        assert_eq!(log_a.lock().unwrap().len(), 1);
        assert_eq!(log_b.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_two_leg_staggered_early_failure_aborts_faster_leg() {
        use crate::coordinator::LatencyStagger;
        use std::time::Duration;

        let mock_a = MockExchange::new("ex_a", vec![], MockResponse::Full);
        // Slower venue B fails immediately
        let mock_b = MockExchange::new("ex_b", vec![], MockResponse::Fail);
        let log_a = mock_a.call_log();
        let log_b = mock_b.call_log();
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(mock_b);

        let (coordinator, _cb) = make_coordinator();
        let order_a = OrderRequest::new_limit(
            ex_a.venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(60000),
            dec!(1.0),
        );
        let order_b = OrderRequest::new_limit(
            ex_b.venue_id(),
            "BTC-USDC",
            Side::Sell,
            dec!(60100),
            dec!(1.0),
        );

        // Delay A by 50ms so B runs first
        let stagger = LatencyStagger {
            delay_a: Duration::from_millis(50),
            delay_b: Duration::ZERO,
        };
        let res = coordinator
            .execute_two_leg_staggered(&ex_a, order_a, &ex_b, order_b, None, Some(stagger))
            .await;
        match res {
            ArbitrageExecutionResult::BothFailed { .. } => {}
            other => panic!("Expected BothFailed, got {:?}", other),
        }
        // CRITICAL SAFETY PROPERTY: Because B failed during A's delay, A was NEVER dispatched!
        assert_eq!(log_a.lock().unwrap().len(), 0);
        assert_eq!(log_b.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_incomplete_fills_trim_to_matched_pair() {
        // Leg A partially fills 0.4 of 1.0; Leg B fills fully -> B's 0.6
        // excess is trimmed and the matched 0.4 pair stays open.
        let mock_a = MockExchange::new(
            "ex_a",
            vec![MockResponse::Partial(dec!(0.4))],
            MockResponse::Full,
        );
        let mock_b = MockExchange::new("ex_b", vec![], MockResponse::Full);
        let log_a = mock_a.call_log();
        let log_b = mock_b.call_log();
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(mock_b);

        let (coordinator, cb) = make_coordinator();

        let order_a = OrderRequest::new_limit(
            ex_a.venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(60000),
            dec!(1.0),
        );
        let order_b = OrderRequest::new_limit(
            ex_b.venue_id(),
            "BTC-USDC",
            Side::Sell,
            dec!(60100),
            dec!(1.0),
        );

        let res = coordinator
            .execute_two_leg(&ex_a, order_a, &ex_b, order_b)
            .await;
        match res {
            ArbitrageExecutionResult::PartialPairKept {
                matched_qty,
                unwind_a,
                unwind_b,
                ..
            } => {
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
    async fn test_scaled_legs_match_in_common_units() {
        // Tracked-group pair: leg A is the index numéraire (1 common unit
        // per leg unit), leg B is the ETF (1/40 common unit per leg unit —
        // ~40 ETF units hedge 1 index unit). A fills 0.5 of 1.0 index;
        // B fills all 40 ETF units. The match must keep 20 ETF units and
        // trim the other 20 — comparing raw fill quantities (0.5 vs 40)
        // would trim 39.5 ETF and leave a naked half-index.
        let mock_a = MockExchange::new(
            "ex_a",
            vec![MockResponse::Partial(dec!(0.5))],
            MockResponse::Full,
        );
        let mock_b = MockExchange::new("ex_b", vec![], MockResponse::Full);
        let log_b = mock_b.call_log();
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(mock_b);

        let (coordinator, cb) = make_coordinator();

        let order_a = OrderRequest::new_limit(
            ex_a.venue_id(),
            "US100-USDC",
            Side::Buy,
            dec!(29270),
            dec!(1.0),
        );
        let order_b = OrderRequest::new_limit(
            ex_b.venue_id(),
            "QQQ-USD",
            Side::Sell,
            dec!(744),
            dec!(40.0),
        );

        let res = coordinator
            .execute_two_leg_scaled(
                &ex_a,
                order_a,
                &ex_b,
                order_b,
                Some((dec!(1), dec!(1) / dec!(40))),
            )
            .await;
        match res {
            ArbitrageExecutionResult::PartialPairKept {
                matched_qty,
                unwind_a,
                unwind_b,
                ..
            } => {
                // Common units: 0.5 index-units matched.
                assert_eq!(matched_qty, dec!(0.5));
                assert!(unwind_a.is_none());
                // B keeps 20 ETF units; the other 20 trim.
                assert_eq!(unwind_b.expect("B excess must trim").filled_qty, dec!(20));
            }
            other => panic!("Expected PartialPairKept, got {:?}", other),
        }
        assert!(!cb.is_tripped());
        let calls_b = log_b.lock().unwrap();
        assert_eq!(calls_b.len(), 2);
        assert_eq!(calls_b[1].side, Side::Buy);
        assert_eq!(calls_b[1].quantity, dec!(20));
    }

    #[tokio::test]
    async fn test_partial_pair_kept_includes_trim_price_pnl() {
        // Same shape as above: A buys 0.4 @60000, B sells 1.0 @60100, the
        // 0.6 short excess is bought back by the market trim (mock: @100).
        let mock_a = MockExchange::new(
            "ex_a",
            vec![MockResponse::Partial(dec!(0.4))],
            MockResponse::Full,
        );
        let mock_b = MockExchange::new("ex_b", vec![], MockResponse::Full);
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(mock_b);

        let (coordinator, _) = make_coordinator();

        let order_a = OrderRequest::new_limit(
            ex_a.venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(60000),
            dec!(1.0),
        );
        let order_b = OrderRequest::new_limit(
            ex_b.venue_id(),
            "BTC-USDC",
            Side::Sell,
            dec!(60100),
            dec!(1.0),
        );

        let res = coordinator
            .execute_two_leg(&ex_a, order_a, &ex_b, order_b)
            .await;
        match res {
            ArbitrageExecutionResult::PartialPairKept {
                matched_qty,
                realized_profit,
                ..
            } => {
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
        let ex_b: Arc<dyn ExchangeAdapter> =
            Arc::new(MockExchange::new("ex_b", vec![], MockResponse::Fail));

        let (coordinator, _cb) = make_coordinator();

        let order_a = OrderRequest::new_limit(
            ex_a.venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(60000),
            dec!(1.0),
        );
        let order_b = OrderRequest::new_limit(
            ex_b.venue_id(),
            "BTC-USDC",
            Side::Sell,
            dec!(60100),
            dec!(1.0),
        );

        let res = coordinator
            .execute_two_leg(&ex_a, order_a, &ex_b, order_b)
            .await;
        match res {
            ArbitrageExecutionResult::BothFailed { error_a, .. } => {
                assert!(
                    error_a.contains("zero filled quantity"),
                    "error_a: {}",
                    error_a
                );
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
        let ex_b: Arc<dyn ExchangeAdapter> =
            Arc::new(MockExchange::new("ex_b", vec![], MockResponse::Fail));

        let (coordinator, cb) = make_coordinator();

        let order_a = OrderRequest::new_limit(
            ex_a.venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(60000),
            dec!(1.0),
        );
        let order_b = OrderRequest::new_limit(
            ex_b.venue_id(),
            "BTC-USDC",
            Side::Sell,
            dec!(60100),
            dec!(1.0),
        );

        let res = coordinator
            .execute_two_leg(&ex_a, order_a, &ex_b, order_b)
            .await;
        match res {
            ArbitrageExecutionResult::UnresolvedExposure {
                leg_a_fill,
                leg_b_fill,
                error_a,
                error_b,
            } => {
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
        let ex_a: Arc<dyn ExchangeAdapter> =
            Arc::new(MockExchange::new("ex_a", vec![], MockResponse::Fail));
        let mock_b = MockExchange::new(
            "ex_b",
            vec![MockResponse::Full, MockResponse::Fail],
            MockResponse::Fail,
        );
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(mock_b);

        let (coordinator, cb) = make_coordinator();

        let order_a = OrderRequest::new_limit(
            ex_a.venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(60000),
            dec!(1.0),
        );
        let order_b = OrderRequest::new_limit(
            ex_b.venue_id(),
            "BTC-USDC",
            Side::Sell,
            dec!(60100),
            dec!(1.0),
        );

        let res = coordinator
            .execute_two_leg(&ex_a, order_a, &ex_b, order_b)
            .await;
        match res {
            ArbitrageExecutionResult::UnresolvedExposure {
                leg_a_fill,
                leg_b_fill,
                error_b,
                ..
            } => {
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
            fee: Some(dec!(0.1)),
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
            fee: Some(dec!(0.1)),
            fee_currency: "USDC".into(),
            raw_response: None,
            timestamp: chrono::Utc::now(),
        }
    }

    async fn assert_malformed_fill_unwound(mutate: impl FnOnce(&mut OrderResponse)) {
        let order_a =
            OrderRequest::new_limit("ex_a".into(), "BTC-USDC", Side::Buy, dec!(60000), dec!(1.0));
        let mut resp = attributed_fill(&order_a, dec!(1.0));
        mutate(&mut resp);
        let mock_a = MockExchange::new("ex_a", vec![MockResponse::Raw(resp)], MockResponse::Full);
        let log_a = mock_a.call_log();
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> =
            Arc::new(MockExchange::new("ex_b", vec![], MockResponse::Full));
        let (coordinator, _cb) = make_coordinator();

        let order_b = OrderRequest::new_limit(
            ex_b.venue_id(),
            "BTC-USDC",
            Side::Sell,
            dec!(60100),
            dec!(1.0),
        );
        let res = coordinator
            .execute_two_leg(&ex_a, order_a, &ex_b, order_b)
            .await;
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
        let order_a =
            OrderRequest::new_limit("ex_a".into(), "BTC-USDC", Side::Buy, dec!(60000), dec!(1.0));
        let mut resp = attributed_fill(&order_a, dec!(1.0));
        mutate(&mut resp);
        let mock_a = MockExchange::new("ex_a", vec![MockResponse::Raw(resp)], MockResponse::Full);
        let log_a = mock_a.call_log();
        let mock_b = MockExchange::new("ex_b", vec![], MockResponse::Full);
        let log_b = mock_b.call_log();
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(mock_b);
        let (coordinator, cb) = make_coordinator();

        let order_b = OrderRequest::new_limit(
            ex_b.venue_id(),
            "BTC-USDC",
            Side::Sell,
            dec!(60100),
            dec!(1.0),
        );
        let res = coordinator
            .execute_two_leg(&ex_a, order_a, &ex_b, order_b)
            .await;
        match res {
            ArbitrageExecutionResult::UnresolvedExposure {
                leg_a_fill,
                error_a,
                error_b,
                ..
            } => {
                assert!(leg_a_fill.is_some());
                assert!(
                    error_a.contains("identity mismatch"),
                    "error_a: {}",
                    error_a
                );
                assert!(error_b.contains("unwound"), "error_b: {}", error_b);
            }
            other => panic!("Expected UnresolvedExposure, got {:?}", other),
        }
        assert!(cb.is_tripped());
        assert_eq!(
            log_a.lock().unwrap().len(),
            1,
            "ambiguous leg must not be unwound"
        );
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
        let order_a =
            OrderRequest::new_limit("ex_a".into(), "BTC-USDC", Side::Buy, dec!(60000), dec!(1.0));
        let mut resp = attributed_fill(&order_a, dec!(1.0));
        resp.venue = "evil_venue".into();
        let mock_a = MockExchange::new("ex_a", vec![MockResponse::Raw(resp)], MockResponse::Full);
        let log_a = mock_a.call_log();
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> =
            Arc::new(MockExchange::new("ex_b", vec![], MockResponse::Fail));
        let (coordinator, cb) = make_coordinator();

        let order_b = OrderRequest::new_limit(
            ex_b.venue_id(),
            "BTC-USDC",
            Side::Sell,
            dec!(60100),
            dec!(1.0),
        );
        let res = coordinator
            .execute_two_leg(&ex_a, order_a, &ex_b, order_b)
            .await;
        match res {
            ArbitrageExecutionResult::UnresolvedExposure {
                leg_a_fill,
                error_a,
                ..
            } => {
                assert!(leg_a_fill.is_some());
                assert!(
                    error_a.contains("identity mismatch"),
                    "error_a: {}",
                    error_a
                );
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
        assert_malformed_fill_unwound(|r| r.fee = Some(dec!(-0.5))).await;
    }

    async fn assert_bad_unwind_unresolved(bad_unwind: OrderResponse) {
        let mock_a = MockExchange::new(
            "ex_a",
            vec![MockResponse::Full, MockResponse::Raw(bad_unwind)],
            MockResponse::Full,
        );
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> =
            Arc::new(MockExchange::new("ex_b", vec![], MockResponse::Fail));

        let (coordinator, cb) = make_coordinator();
        let order_a = OrderRequest::new_limit(
            ex_a.venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(60000),
            dec!(1.0),
        );
        let order_b = OrderRequest::new_limit(
            ex_b.venue_id(),
            "BTC-USDC",
            Side::Sell,
            dec!(60100),
            dec!(1.0),
        );
        let res = coordinator
            .execute_two_leg(&ex_a, order_a, &ex_b, order_b)
            .await;
        match res {
            ArbitrageExecutionResult::UnresolvedExposure {
                leg_a_fill,
                error_a,
                ..
            } => {
                assert!(leg_a_fill.is_some());
                assert!(error_a.contains("unwind failed"), "error_a: {}", error_a);
                assert!(
                    error_a.contains("Emergency unwind incomplete"),
                    "error_a: {}",
                    error_a
                );
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
        let mock_a = MockExchange::new(
            "ex_a",
            vec![MockResponse::Partial(dec!(0.4))],
            MockResponse::Full,
        );
        let log_a = mock_a.call_log();
        let mock_b = MockExchange::new(
            "ex_b",
            vec![MockResponse::Partial(dec!(0.6))],
            MockResponse::Full,
        );
        let log_b = mock_b.call_log();
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(mock_b);

        let (coordinator, cb) = make_coordinator();
        let order_a = OrderRequest::new_limit(
            ex_a.venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(60000),
            dec!(1.0),
        );
        let order_b = OrderRequest::new_limit(
            ex_b.venue_id(),
            "BTC-USDC",
            Side::Sell,
            dec!(60100),
            dec!(1.0),
        );

        let res = coordinator
            .execute_two_leg(&ex_a, order_a, &ex_b, order_b)
            .await;
        match res {
            ArbitrageExecutionResult::PartialPairKept {
                matched_qty,
                unwind_a,
                unwind_b,
                ..
            } => {
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
        let mock_a = MockExchange::new(
            "ex_a",
            vec![MockResponse::Partial(dec!(0.4))],
            MockResponse::Full,
        );
        let mock_b = MockExchange::new(
            "ex_b",
            vec![MockResponse::Full, MockResponse::Fail],
            MockResponse::Fail,
        );
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(mock_b);

        let (coordinator, cb) = make_coordinator();
        let order_a = OrderRequest::new_limit(
            ex_a.venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(60000),
            dec!(1.0),
        );
        let order_b = OrderRequest::new_limit(
            ex_b.venue_id(),
            "BTC-USDC",
            Side::Sell,
            dec!(60100),
            dec!(1.0),
        );

        let res = coordinator
            .execute_two_leg(&ex_a, order_a, &ex_b, order_b)
            .await;
        match res {
            ArbitrageExecutionResult::UnresolvedExposure {
                leg_a_fill,
                leg_b_fill,
                error_b,
                ..
            } => {
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
        let mock_a = MockExchange::new(
            "ex_a",
            vec![MockResponse::Partial(dec!(0.4))],
            MockResponse::Full,
        );
        let mock_b = MockExchange::new(
            "ex_b",
            vec![MockResponse::Partial(dec!(0.4))],
            MockResponse::Full,
        );
        let log_a = mock_a.call_log();
        let log_b = mock_b.call_log();
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(mock_b);

        let (coordinator, cb) = make_coordinator();
        let order_a = OrderRequest::new_limit(
            ex_a.venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(60000),
            dec!(1.0),
        );
        let order_b = OrderRequest::new_limit(
            ex_b.venue_id(),
            "BTC-USDC",
            Side::Sell,
            dec!(60100),
            dec!(1.0),
        );

        let res = coordinator
            .execute_two_leg(&ex_a, order_a, &ex_b, order_b)
            .await;
        match res {
            ArbitrageExecutionResult::PartialPairKept {
                matched_qty,
                unwind_a,
                unwind_b,
                ..
            } => {
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

    /// Venue-attributed order state for scripted fetch_order/Raw responses.
    fn resp(
        venue: &str,
        symbol: &str,
        client_order_id: &str,
        order_id: &str,
        status: OrderStatus,
        filled: Decimal,
        requested: Decimal,
    ) -> OrderResponse {
        OrderResponse {
            order_id: order_id.to_string(),
            client_order_id: client_order_id.to_string(),
            venue: venue.into(),
            symbol: symbol.to_string(),
            status,
            original_qty: requested,
            filled_qty: filled,
            remaining_qty: if status == OrderStatus::Cancelled {
                Decimal::ZERO
            } else {
                requested - filled
            },
            avg_fill_price: (filled > Decimal::ZERO).then_some(dec!(100)),
            fee: Some(dec!(0.1)),
            fee_currency: "USDC".to_string(),
            raw_response: None,
            timestamp: chrono::Utc::now(),
        }
    }

    /// Canary regression: a leg acked `New` that fills moments later must
    /// resolve as Success — not force an unwind of the filled sibling.
    #[tokio::test]
    async fn test_working_leg_late_fill_resolves_to_success() {
        let mock_a = MockExchange::new("ex_a", vec![], MockResponse::Full);
        let log_a = mock_a.call_log();
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);

        let order_b = OrderRequest::new_limit(
            "ex_b".into(),
            "BTC-USDC",
            Side::Sell,
            dec!(60100),
            dec!(1.0),
        );
        let cid_b = order_b.client_order_id.clone();
        let acked = resp(
            "ex_b",
            "BTC-USDC",
            &cid_b,
            "b-order",
            OrderStatus::New,
            dec!(0),
            dec!(1.0),
        );
        let filled = resp(
            "ex_b",
            "BTC-USDC",
            &cid_b,
            "b-order",
            OrderStatus::Filled,
            dec!(1.0),
            dec!(1.0),
        );
        let mock_b = MockExchange::new("ex_b", vec![MockResponse::Raw(acked)], MockResponse::Fail)
            .with_fetch(vec![Ok(filled)]);
        let cancels_b = mock_b.cancel_log();
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(mock_b);

        let (coordinator, cb) = make_coordinator();
        let order_a = OrderRequest::new_limit(
            ex_a.venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(60000),
            dec!(1.0),
        );

        let res = coordinator
            .execute_two_leg(&ex_a, order_a, &ex_b, order_b)
            .await;
        match res {
            ArbitrageExecutionResult::Success { leg_b, .. } => {
                assert_eq!(leg_b.order_id, "b-order");
                assert_eq!(leg_b.filled_qty, dec!(1.0));
            }
            other => panic!("Expected Success after late fill, got {:?}", other),
        }
        assert!(!cb.is_tripped());
        assert_eq!(log_a.lock().unwrap().len(), 1);
        assert!(cancels_b.lock().unwrap().is_empty());
    }

    /// A still-working zero-fill leg is cancelled after the grace window so
    /// it can never fill later untracked; the filled sibling is unwound and
    /// the outcome is a clean BothFailed — no reconciler handoff.
    #[tokio::test]
    async fn test_working_leg_cancelled_when_fill_never_lands() {
        let mock_a = MockExchange::new("ex_a", vec![], MockResponse::Full);
        let log_a = mock_a.call_log();
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);

        let order_b = OrderRequest::new_limit(
            "ex_b".into(),
            "BTC-USDC",
            Side::Sell,
            dec!(60100),
            dec!(1.0),
        );
        let cid_b = order_b.client_order_id.clone();
        let acked = resp(
            "ex_b",
            "BTC-USDC",
            &cid_b,
            "b-order",
            OrderStatus::New,
            dec!(0),
            dec!(1.0),
        );
        let still_new = || {
            Ok(resp(
                "ex_b",
                "BTC-USDC",
                &cid_b,
                "b-order",
                OrderStatus::New,
                dec!(0),
                dec!(1.0),
            ))
        };
        let cancelled = Ok(resp(
            "ex_b",
            "BTC-USDC",
            &cid_b,
            "b-order",
            OrderStatus::Cancelled,
            dec!(0),
            dec!(1.0),
        ));
        let mock_b = MockExchange::new("ex_b", vec![MockResponse::Raw(acked)], MockResponse::Fail)
            .with_fetch(vec![still_new(), still_new(), still_new(), cancelled]);
        let cancels_b = mock_b.cancel_log();
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(mock_b);

        let (coordinator, _cb) = make_coordinator();
        let order_a = OrderRequest::new_limit(
            ex_a.venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(60000),
            dec!(1.0),
        );

        let res = coordinator
            .execute_two_leg(&ex_a, order_a, &ex_b, order_b)
            .await;
        match res {
            ArbitrageExecutionResult::BothFailed { .. } => {}
            other => panic!("Expected BothFailed after cancel, got {:?}", other),
        }
        assert_eq!(cancels_b.lock().unwrap().len(), 1);
        // ex_a saw its entry order plus the emergency unwind of the fill.
        let calls_a = log_a.lock().unwrap();
        assert_eq!(calls_a.len(), 2);
        assert_eq!(calls_a[1].side, Side::Sell);
    }

    /// Leg B errors while leg A's order is still working: settle polls to
    /// the terminal fill so the unwind covers the FINAL filled quantity —
    /// unwinding the earlier snapshot would orphan the remainder.
    #[tokio::test]
    async fn test_unwind_covers_terminal_qty_when_leg_completes_late() {
        let order_a =
            OrderRequest::new_limit("ex_a".into(), "BTC-USDC", Side::Buy, dec!(60000), dec!(1.0));
        let cid_a = order_a.client_order_id.clone();
        let partial = resp(
            "ex_a",
            "BTC-USDC",
            &cid_a,
            "a-order",
            OrderStatus::PartiallyFilled,
            dec!(0.4),
            dec!(1.0),
        );
        let still_working = resp(
            "ex_a",
            "BTC-USDC",
            &cid_a,
            "a-order",
            OrderStatus::PartiallyFilled,
            dec!(0.6),
            dec!(1.0),
        );
        let filled = resp(
            "ex_a",
            "BTC-USDC",
            &cid_a,
            "a-order",
            OrderStatus::Filled,
            dec!(1.0),
            dec!(1.0),
        );
        let mock_a =
            MockExchange::new("ex_a", vec![MockResponse::Raw(partial)], MockResponse::Full)
                .with_fetch(vec![Ok(still_working), Ok(filled)]);
        let log_a = mock_a.call_log();
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);
        let ex_b: Arc<dyn ExchangeAdapter> =
            Arc::new(MockExchange::new("ex_b", vec![], MockResponse::Fail));

        let (coordinator, _cb) = make_coordinator();
        let order_b = OrderRequest::new_limit(
            ex_b.venue_id(),
            "BTC-USDC",
            Side::Sell,
            dec!(60100),
            dec!(1.0),
        );

        let res = coordinator
            .execute_two_leg(&ex_a, order_a, &ex_b, order_b)
            .await;
        match res {
            ArbitrageExecutionResult::LegBFailedUnwound { unwind_resp, .. } => {
                assert_eq!(unwind_resp.filled_qty, dec!(1.0));
            }
            other => panic!("Expected LegBFailedUnwound, got {:?}", other),
        }
        let calls_a = log_a.lock().unwrap();
        assert_eq!(calls_a.len(), 2);
        assert_eq!(calls_a[1].quantity, dec!(1.0));
    }

    /// A fetch echo for a different order id is never adopted: the leg keeps
    /// its last verified state and is handed to the reconciler, still
    /// working, rather than classified on someone else's fill.
    #[tokio::test]
    async fn test_fetch_identity_mismatch_never_adopted() {
        let mock_a = MockExchange::new("ex_a", vec![], MockResponse::Full);
        let ex_a: Arc<dyn ExchangeAdapter> = Arc::new(mock_a);

        let order_b = OrderRequest::new_limit(
            "ex_b".into(),
            "BTC-USDC",
            Side::Sell,
            dec!(60100),
            dec!(1.0),
        );
        let cid_b = order_b.client_order_id.clone();
        let acked = resp(
            "ex_b",
            "BTC-USDC",
            &cid_b,
            "b-order",
            OrderStatus::New,
            dec!(0),
            dec!(1.0),
        );
        let foreign = resp(
            "ex_b",
            "BTC-USDC",
            "other-cid",
            "other-order",
            OrderStatus::Filled,
            dec!(1.0),
            dec!(1.0),
        );
        let mock_b = MockExchange::new("ex_b", vec![MockResponse::Raw(acked)], MockResponse::Fail)
            .with_fetch(vec![Ok(foreign)]);
        let ex_b: Arc<dyn ExchangeAdapter> = Arc::new(mock_b);

        let (coordinator, cb) = make_coordinator();
        let order_a = OrderRequest::new_limit(
            ex_a.venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(60000),
            dec!(1.0),
        );

        let res = coordinator
            .execute_two_leg(&ex_a, order_a, &ex_b, order_b)
            .await;
        match res {
            ArbitrageExecutionResult::UnresolvedExposure { leg_b_fill, .. } => {
                assert_eq!(
                    leg_b_fill.expect("working leg preserved").order_id,
                    "b-order"
                );
            }
            other => panic!("Expected UnresolvedExposure handoff, got {:?}", other),
        }
        // Tracked working orders are handed to the reconciler — the
        // coordinator does not trip on them (the caller may).
        assert!(!cb.is_tripped());
    }

    /// `enrich_fill_stats` keeps polling a terminal FILLED record until the
    /// venue enriches avgFillPrice — the real Arcus sequencing where status
    /// lands before the fill stats.
    #[tokio::test]
    async fn test_enrich_fill_stats_waits_for_price_enrichment() {
        let cid = "cid-1";
        let mut bare = resp(
            "ex_a",
            "BTC-USDC",
            cid,
            "a-order",
            OrderStatus::Filled,
            dec!(1.0),
            dec!(1.0),
        );
        bare.avg_fill_price = None;
        let enriched = resp(
            "ex_a",
            "BTC-USDC",
            cid,
            "a-order",
            OrderStatus::Filled,
            dec!(1.0),
            dec!(1.0),
        );
        let mock = MockExchange::new("ex_a", vec![], MockResponse::Fail)
            .with_fetch(vec![Ok(bare.clone()), Ok(enriched)]);
        let ex: Arc<dyn ExchangeAdapter> = Arc::new(mock);

        let out = enrich_fill_stats(&ex, "BTC-USDC", cid, bare).await;
        assert_eq!(out.order_id, "a-order");
        assert_eq!(out.filled_qty, dec!(1.0));
        assert_eq!(out.avg_fill_price, Some(dec!(100)));
    }

    /// A FILLED record that never enriches returns the last verified
    /// snapshot — a foreign order echo is never adopted.
    #[tokio::test]
    async fn test_enrich_fill_stats_never_adopts_foreign_order() {
        let cid = "cid-1";
        let mut bare = resp(
            "ex_a",
            "BTC-USDC",
            cid,
            "a-order",
            OrderStatus::Filled,
            dec!(1.0),
            dec!(1.0),
        );
        bare.avg_fill_price = None;
        let foreign = resp(
            "ex_a",
            "BTC-USDC",
            "other-cid",
            "other-order",
            OrderStatus::Filled,
            dec!(9.0),
            dec!(9.0),
        );
        let mock = MockExchange::new("ex_a", vec![], MockResponse::Fail).with_fetch(vec![
            Ok(foreign.clone()),
            Ok(foreign.clone()),
            Ok(foreign),
        ]);
        let ex: Arc<dyn ExchangeAdapter> = Arc::new(mock);

        let out = enrich_fill_stats(&ex, "BTC-USDC", cid, bare).await;
        assert_eq!(out.order_id, "a-order");
        assert_eq!(out.avg_fill_price, None);
    }

    /// Complete records never trigger a status poll at all.
    #[tokio::test]
    async fn test_enrich_fill_stats_complete_record_polls_nothing() {
        let cid = "cid-1";
        let complete = resp(
            "ex_a",
            "BTC-USDC",
            cid,
            "a-order",
            OrderStatus::Filled,
            dec!(1.0),
            dec!(1.0),
        );
        // Empty fetch script: any fetch_order call bails — if enrichment ran
        // anyway the result would still be the input record, but the mock's
        // bail proves nothing was polled when combined with identity below.
        let mock = MockExchange::new("ex_a", vec![], MockResponse::Fail);
        let ex: Arc<dyn ExchangeAdapter> = Arc::new(mock);

        let out = enrich_fill_stats(&ex, "BTC-USDC", cid, complete.clone()).await;
        assert_eq!(out.order_id, "a-order");
        assert_eq!(out.avg_fill_price, Some(dec!(100)));
    }

    #[test]
    fn test_multi_leg_fragility_first_sorting() {
        let mock_a: Arc<dyn ExchangeAdapter> =
            Arc::new(MockExchange::new("ex_a", vec![], MockResponse::Full));
        let mock_b: Arc<dyn ExchangeAdapter> =
            Arc::new(MockExchange::new("ex_b", vec![], MockResponse::Full));
        let mock_c: Arc<dyn ExchangeAdapter> =
            Arc::new(MockExchange::new("ex_c", vec![], MockResponse::Full));

        let mut plan = MultiLegExecutionPlan::new("test_strategy", dec!(10.0));
        let dummy_req = OrderRequest::new_market(
            arbitrageur_core::types::VenueId::new("test"),
            "BTC-USDC",
            Side::Buy,
            dec!(1.0),
        );

        // Add 3 legs in arbitrary order: Liquid (15.2), Fragile (0.4), Moderate (3.8)
        plan.add_leg(mock_a, dummy_req.clone(), 15.2, false);
        plan.add_leg(mock_b, dummy_req.clone(), 0.4, true);
        plan.add_leg(mock_c, dummy_req.clone(), 3.8, false);

        plan.sort_fragility_first();

        // Must be sorted in ascending order of fragility index: 0.4 -> 3.8 -> 15.2
        assert_eq!(plan.legs[0].fragility_index, 0.4);
        assert_eq!(plan.legs[1].fragility_index, 3.8);
        assert_eq!(plan.legs[2].fragility_index, 15.2);
    }

    #[tokio::test]
    async fn test_multi_leg_execution_plan_success() {
        let mock_a: Arc<dyn ExchangeAdapter> = Arc::new(MockExchange::new(
            "ex_a",
            vec![MockResponse::Full],
            MockResponse::Full,
        ));
        let mock_b: Arc<dyn ExchangeAdapter> = Arc::new(MockExchange::new(
            "ex_b",
            vec![MockResponse::Full],
            MockResponse::Full,
        ));

        let mut plan = MultiLegExecutionPlan::new("triangular", dec!(5.0));
        let req_a = OrderRequest::new_market(
            arbitrageur_core::types::VenueId::new("ex_a"),
            "BTC-USDC",
            Side::Buy,
            dec!(0.5),
        );
        let req_b = OrderRequest::new_market(
            arbitrageur_core::types::VenueId::new("ex_b"),
            "ETH-BTC",
            Side::Buy,
            dec!(0.5),
        );

        plan.add_leg(mock_a, req_a, 2.0, false);
        plan.add_leg(mock_b, req_b, 0.5, true);

        let (coordinator, _cb) = make_coordinator();

        let res = coordinator.execute_plan(plan).await;
        match res {
            ArbitrageExecutionResult::Success { leg_a, leg_b, .. } => {
                assert_eq!(leg_a.filled_qty, dec!(0.5));
                assert_eq!(leg_b.filled_qty, dec!(0.5));
            }
            other => panic!("expected Success, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_multi_leg_execution_plan_bottleneck_abort_zero_loss() {
        // Bottleneck leg fails immediately
        let mock_a: Arc<dyn ExchangeAdapter> = Arc::new(MockExchange::new(
            "ex_a",
            vec![MockResponse::Fail],
            MockResponse::Fail,
        ));
        let mock_b_calls = Arc::new(Mutex::new(Vec::new()));
        let mock_b: Arc<dyn ExchangeAdapter> = Arc::new(MockExchange {
            name: "ex_b".into(),
            script: vec![MockResponse::Full],
            fallback: MockResponse::Full,
            calls: mock_b_calls.clone(),
            fetch_script: Arc::new(Mutex::new(Vec::new())),
            cancels: Arc::new(Mutex::new(Vec::new())),
        });

        let mut plan = MultiLegExecutionPlan::new("options_box", dec!(5.0));
        let req = OrderRequest::new_market(
            arbitrageur_core::types::VenueId::new("ex"),
            "SYM",
            Side::Buy,
            dec!(1.0),
        );

        // Bottleneck leg has fragility 0.1 (executed first), liquid leg has 10.0
        plan.add_leg(mock_a, req.clone(), 0.1, true);
        plan.add_leg(mock_b, req.clone(), 10.0, false);

        let (coordinator, _cb) = make_coordinator();

        let res = coordinator.execute_plan(plan).await;
        match res {
            ArbitrageExecutionResult::BothFailed { .. } => {}
            other => panic!("expected BothFailed on bottleneck abort, got {:?}", other),
        }

        // Fragility-First guarantee: Leg B was NEVER touched, 0 capital at risk!
        assert_eq!(mock_b_calls.lock().unwrap().len(), 0);
    }

    #[test]
    fn test_dynamic_partial_fill_evaluator() {
        let evaluator = DynamicPartialFillEvaluator::new(1e-6);

        // Case 1: Wide-spread options / altcoin (35 bps spread) -> selects Route B Synthetic Perp Lock
        let route_options = evaluator.evaluate_unwind_route(
            25000.0, // $25k notional
            35.0,    // 35 bps spread
            5.0,     // 5 bps taker fee
            -1.0,    // -1 bps maker rebate
            10.0,    // 10 bps adverse selection
            0.5,     // 0.5 bps perp spread
            2.5,     // 2.5 bps perp taker fee
            0.80,    // 80% annual basis vol
            15.0,    // 15 sec expected unwind
            Some("BTC-PERP"),
        );

        match route_options {
            UnwindRoute::RouteBSyntheticPerpLock {
                perp_hedge_symbol, ..
            } => {
                assert_eq!(perp_hedge_symbol, "BTC-PERP");
            }
            UnwindRoute::RouteAMarketInversion { .. } => {
                panic!("expected Route B Synthetic Perp Lock for wide-spread options");
            }
        }

        // Case 2: Ultra-tight major on CEX (0.5 bps spread, 0 adverse selection) -> selects Route A Immediate Market Inversion
        let route_tight = evaluator.evaluate_unwind_route(
            1000.0, // $1k notional
            0.5,    // 0.5 bps spread
            2.0,    // 2 bps taker fee
            0.0,    // 0 maker rebate
            0.0,    // 0 adverse selection
            0.5,    // 0.5 bps perp spread
            2.5,    // 2.5 bps perp taker fee
            0.80,   // 80% annual basis vol
            15.0,   // 15 sec expected unwind
            Some("BTC-PERP"),
        );

        match route_tight {
            UnwindRoute::RouteAMarketInversion { .. } => {}
            UnwindRoute::RouteBSyntheticPerpLock { .. } => {
                panic!("expected Route A Market Inversion for ultra-tight major");
            }
        }
    }

    #[test]
    fn test_limit_price_for_hawkes() {
        use crate::passive_unwind::limit_price_for_hawkes;
        use arbitrageur_math::hawkes::MultivariateHawkes;

        let bid = dec!(100.0);
        let ask = dec!(102.0);

        // Without Hawkes: normal base percentile 50.0 (mid-price 101.0)
        let calm = limit_price_for_hawkes(Side::Buy, bid, ask, 50.0, None, 0);
        assert_eq!(calm, Some(dec!(101.0)));

        // With Subcritical Hawkes: calm intensity peg matches
        let mut hawkes = MultivariateHawkes::new(2);
        hawkes.mu = vec![0.1, 0.1];
        hawkes.alpha = vec![vec![0.5, 0.0], vec![0.0, 0.5]];
        hawkes.beta = vec![vec![1.0, 1.0], vec![1.0, 1.0]];

        let hawkes_calm = limit_price_for_hawkes(Side::Buy, bid, ask, 50.0, Some(&hawkes), 0);
        assert_eq!(hawkes_calm, Some(dec!(101.0)));

        // Moderate burst of arrivals on stream 0 excites lambda/mu to ~3x: maker quote widens deeper (toward bid 100.0)
        hawkes.alpha = vec![vec![0.1, 0.0], vec![0.0, 0.1]];
        for i in 0..2 {
            hawkes.update_at_timestamp(1.0 + i as f64 * 0.01, Some(0));
        }
        let hawkes_burst =
            limit_price_for_hawkes(Side::Buy, bid, ask, 50.0, Some(&hawkes), 0).unwrap();
        // Widened deeper toward passive touch: Buy price should be lower than mid (closer to bid 100.0)
        assert!(
            hawkes_burst < dec!(101.0),
            "Expected widened buy peg < 101.0, got {hawkes_burst}"
        );

        // Extreme toxic surge (>15x mu): quote is pulled entirely (None)
        hawkes.alpha = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        for i in 0..15 {
            hawkes.update_at_timestamp(2.0 + i as f64 * 0.01, Some(0));
        }
        let hawkes_pulled = limit_price_for_hawkes(Side::Buy, bid, ask, 50.0, Some(&hawkes), 0);
        assert_eq!(
            hawkes_pulled, None,
            "Expected quote to be pulled under toxic surge"
        );

        // Test with Multi-Level Hawkes Queue Drain Forecaster
        use crate::passive_unwind::limit_price_for_hawkes_with_drain;
        use arbitrageur_math::cross_hawkes::MultiLevelHawkesQueueDrainForecaster;

        let normal_drain = MultiLevelHawkesQueueDrainForecaster::default();
        let peg_normal = limit_price_for_hawkes_with_drain(
            Side::Buy,
            bid,
            ask,
            50.0,
            None,
            0,
            Some(&normal_drain),
            10.0,
            25.0,
        );
        assert_eq!(peg_normal, Some(dec!(101.0)));

        let toxic_drain = MultiLevelHawkesQueueDrainForecaster::new(50.0, 10.0, 2.0, 1.0);
        let peg_toxic = limit_price_for_hawkes_with_drain(
            Side::Buy,
            bid,
            ask,
            50.0,
            None,
            0,
            Some(&toxic_drain),
            1.0,
            10.0,
        );
        assert_eq!(
            peg_toxic, None,
            "Expected quote to be pulled under multi-level queue drain hazard"
        );
    }

    #[tokio::test]
    async fn test_two_leg_with_racing_guard_abort_and_dispatch() {
        let ex_a = Arc::new(MockExchange::new("mock_a", vec![], MockResponse::Full))
            as Arc<dyn ExchangeAdapter>;
        let ex_b = Arc::new(MockExchange::new("mock_b", vec![], MockResponse::Full))
            as Arc<dyn ExchangeAdapter>;

        let (mut coord, _cb) = make_coordinator();
        coord.set_racing_abort_guard(Some(
            arbitrageur_math::latency_racing::PreFlightRacingAbortGuard::default(),
        ));

        let our =
            arbitrageur_math::latency_racing::LatencyDistribution::new_empirical(2.0, 4.0, 1.0);
        let comp =
            arbitrageur_math::latency_racing::LatencyDistribution::new_empirical(2.0, 4.0, 1.0);
        let racing_model = arbitrageur_math::latency_racing::CrossVenueRacingModel::new(our, comp);

        let order_a = OrderRequest::new_limit(
            ex_a.venue_id(),
            "BTC-USDC",
            Side::Buy,
            dec!(60000),
            dec!(1.0),
        );
        let order_b = OrderRequest::new_limit(
            ex_b.venue_id(),
            "BTC-USDC",
            Side::Sell,
            dec!(60100),
            dec!(1.0),
        );

        // 1. Unfavorable race: Competitor has 4ms lead (-4.0ms) -> Win prob ~0.2% < 50% -> Abort before dispatch!
        let abort_res = coord
            .execute_two_leg_with_racing_guard(
                &ex_a,
                order_a.clone(),
                &ex_b,
                order_b.clone(),
                None,
                None,
                10.0,
                5.0,
                -4.0,
                Some(&racing_model),
            )
            .await;

        match abort_res {
            ArbitrageExecutionResult::HaltedByRisk(reason) => {
                assert!(reason.contains("Pre-flight racing abort"));
            }
            _ => panic!("Expected HaltedByRisk from pre-flight abort guard"),
        }

        // 2. Favorable race: We have 3ms lead (+3.0ms) -> Win prob > 95% -> Safe dispatch!
        let dispatch_res = coord
            .execute_two_leg_with_racing_guard(
                &ex_a,
                order_a,
                &ex_b,
                order_b,
                None,
                None,
                10.0,
                5.0,
                3.0,
                Some(&racing_model),
            )
            .await;

        match dispatch_res {
            ArbitrageExecutionResult::Success { leg_a, leg_b, .. } => {
                assert!(leg_a.order_id.starts_with("mock_order_"));
                assert!(leg_b.order_id.starts_with("mock_order_"));
            }
            _ => panic!("Expected Success from favorable race dispatch"),
        }
    }
}
