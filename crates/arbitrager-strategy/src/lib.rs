pub mod spatial_price_arb;
pub mod funding_rate_arb;
pub mod stat_arb_pairs;
pub mod prediction_market_arb;
pub mod engine;

pub use spatial_price_arb::*;
pub use funding_rate_arb::*;
pub use stat_arb_pairs::*;
pub use prediction_market_arb::*;
pub use engine::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use arbitrager_core::types::dec;
    use arbitrager_exchanges::simulated::SimulatedExchange;
    use arbitrager_execution::coordinator::{ArbitrageExecutionResult, AtomicArbitrageCoordinator};
    use arbitrager_execution::smart_routing::SmartOrderRouter;
    use arbitrager_risk::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};

    #[tokio::test]
    async fn test_spatial_strategy_opportunity_detection() {
        let ex_a = Arc::new(SimulatedExchange::new("hyperliquid", "Hyperliquid", 0, dec!(2.0), dec!(10000.0)));
        let ex_b = Arc::new(SimulatedExchange::new("lighter", "Lighter", 0, dec!(2.0), dec!(10000.0)));

        // Market A: Bid 60000, Ask 60010
        ex_a.seed_market("BTC-USDC", dec!(60000.0), dec!(60010.0), dec!(5.0), dec!(0.0001));
        // Market B: Bid 60150, Ask 60160 (Higher price on Lighter!)
        ex_b.seed_market("BTC-USDC", dec!(60150.0), dec!(60160.0), dec!(5.0), dec!(0.0001));

        let mut strat = SpatialPriceArbStrategy::new(
            "BTC-USDC",
            ex_a.clone(),
            ex_b.clone(),
            dec!(5.0),  // 5 BPS target profit
            dec!(10.0), // 10 BPS max slippage
            dec!(0.5),  // 0.5 BTC order qty
            dec!(2.0),  // 2 BPS fee A
            dec!(2.0),  // 2 BPS fee B
            50,
            1,          // 1 min sample for unit test
        );

        let opp = strat.evaluate_opportunity().await.expect("Evaluate opportunity");
        assert!(opp.is_some());
        let signal = opp.unwrap();
        // Should buy on A (ask 60010) and sell on B (bid 60150)
        assert_eq!(signal.direction, SpatialDirection::BuyASellB);
        assert!(signal.expected_profit_usd > dec!(50.0));

        let cb = Arc::new(CircuitBreaker::new(CircuitBreakerConfig::default(), dec!(10000)));
        let router = SmartOrderRouter::new(dec!(10));
        let coordinator = AtomicArbitrageCoordinator::new(router, cb);

        let result = strat.execute_signal(&signal, &coordinator).await;
        match result {
            ArbitrageExecutionResult::Success { realized_profit, .. } => {
                assert!(realized_profit > dec!(0));
            }
            other => panic!("Expected Success, got {:?}", other),
        }
    }
}
