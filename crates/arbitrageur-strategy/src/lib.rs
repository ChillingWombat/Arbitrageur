pub mod spatial_price_arb;
pub mod funding_rate_arb;
pub mod stat_arb_pairs;
pub mod prediction_market_arb;
pub mod convergence_auto;
pub mod engine;
pub mod spatial_exit;
pub mod pair_trim;

pub use spatial_price_arb::*;
pub use funding_rate_arb::*;
pub use stat_arb_pairs::*;
pub use prediction_market_arb::*;
pub use engine::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use arbitrageur_core::exchange::ExchangeAdapter;
    use arbitrageur_core::types::dec;
    use arbitrageur_exchanges::simulated::SimulatedExchange;
    use arbitrageur_execution::coordinator::{ArbitrageExecutionResult, AtomicArbitrageCoordinator};
    use arbitrageur_execution::smart_routing::SmartOrderRouter;
    use arbitrageur_risk::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};

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
            ArbitrageExecutionResult::Success { realized_profit, leg_a, leg_b } => {
                // Realized cash P&L on an open is fee outflow only
                assert_eq!(realized_profit, -(leg_a.fee + leg_b.fee));
            }
            other => panic!("Expected Success, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_prediction_market_strategy_both_directions() {
        let ex_poly = Arc::new(SimulatedExchange::new("polymarket", "Polymarket", 0, dec!(0.0), dec!(1000.0)));
        let ex_kalshi = Arc::new(SimulatedExchange::new("kalshi", "Kalshi", 0, dec!(0.0), dec!(1000.0)));

        let cb = Arc::new(CircuitBreaker::new(CircuitBreakerConfig::default(), dec!(10000)));
        let router = SmartOrderRouter::new(dec!(10));
        let coordinator = AtomicArbitrageCoordinator::new(router, cb);

        // Direction 1: Poly YES (0.40) + Kalshi NO (0.55) = 0.95 < 1.00
        ex_poly.seed_market("FED-YES", dec!(0.39), dec!(0.40), dec!(1000.0), dec!(0));
        ex_kalshi.seed_market("FED-NO", dec!(0.54), dec!(0.55), dec!(1000.0), dec!(0));

        let strat1 = PredictionMarketArbStrategy::new_with_tokens(
            ex_poly.clone(),
            ex_kalshi.clone(),
            "FED-YES",
            "FED-NO",
            "FED-YES",
            "FED-NO",
            dec!(5), // 5 BPS fee
            dec!(100.0),
        );

        let opp1 = strat1.check_opportunity().await.expect("check opportunity").expect("opp1 should exist");
        assert_eq!(opp1.buy_yes_venue, "Polymarket");
        assert_eq!(opp1.buy_no_venue, "Kalshi");

        let res1 = strat1.execute(&opp1, &coordinator).await;
        match res1 {
            ArbitrageExecutionResult::Success { realized_profit, leg_a, leg_b } => {
                assert_eq!(leg_a.venue, ex_poly.venue_id());
                assert_eq!(leg_a.symbol, "FED-YES");
                assert_eq!(leg_b.venue, ex_kalshi.venue_id());
                assert_eq!(leg_b.symbol, "FED-NO");
                assert_eq!(realized_profit, -(leg_a.fee + leg_b.fee));
            }
            other => panic!("Expected Success, got {:?}", other),
        }

        // Direction 2: Kalshi YES (0.42) + Poly NO (0.50) = 0.92 < 1.00
        ex_poly.seed_market("ELEC-YES", dec!(0.48), dec!(0.52), dec!(1000.0), dec!(0));
        ex_poly.seed_market("ELEC-NO", dec!(0.49), dec!(0.50), dec!(1000.0), dec!(0));
        ex_kalshi.seed_market("ELEC-YES", dec!(0.41), dec!(0.42), dec!(1000.0), dec!(0));
        ex_kalshi.seed_market("ELEC-NO", dec!(0.56), dec!(0.58), dec!(1000.0), dec!(0));

        let strat2 = PredictionMarketArbStrategy::new_with_tokens(
            ex_poly.clone(),
            ex_kalshi.clone(),
            "ELEC-YES",
            "ELEC-NO",
            "ELEC-YES",
            "ELEC-NO",
            dec!(5),
            dec!(100.0),
        );

        let opp2 = strat2.check_opportunity().await.expect("check opportunity").expect("opp2 should exist");
        assert_eq!(opp2.buy_yes_venue, "Kalshi");
        assert_eq!(opp2.buy_no_venue, "Polymarket");

        let res2 = strat2.execute(&opp2, &coordinator).await;
        match res2 {
            ArbitrageExecutionResult::Success { realized_profit, leg_a, leg_b } => {
                // Leg A is Polymarket (must buy NO)
                assert_eq!(leg_a.venue, ex_poly.venue_id());
                assert_eq!(leg_a.symbol, "ELEC-NO");
                assert_eq!(leg_a.avg_fill_price, Some(dec!(0.50)));
                // Leg B is Kalshi (must buy YES)
                assert_eq!(leg_b.venue, ex_kalshi.venue_id());
                assert_eq!(leg_b.symbol, "ELEC-YES");
                assert_eq!(leg_b.avg_fill_price, Some(dec!(0.42)));
                assert_eq!(realized_profit, -(leg_a.fee + leg_b.fee));
            }
            other => panic!("Expected Success, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_funding_rate_strategy_both_directions() {
        let ex_a = Arc::new(SimulatedExchange::new("hyperliquid", "Hyperliquid", 0, dec!(2.0), dec!(20000.0)));
        let ex_b = Arc::new(SimulatedExchange::new("lighter", "Lighter", 0, dec!(2.0), dec!(20000.0)));

        let cb = Arc::new(CircuitBreaker::new(CircuitBreakerConfig::default(), dec!(50000)));
        let router = SmartOrderRouter::new(dec!(10));
        let coordinator = AtomicArbitrageCoordinator::new(router, cb);

        // Scenario 1: A has higher funding (0.03% / 8h) vs B (0.01% / 8h)
        // Expected: Short A (Hyperliquid), Long B (Lighter)
        ex_a.seed_market("BTC-USDC", dec!(60000.0), dec!(60010.0), dec!(5.0), dec!(0.0003));
        ex_b.seed_market("BTC-USDC", dec!(60000.0), dec!(60010.0), dec!(5.0), dec!(0.0001));

        let strat1 = FundingRateArbStrategy::new(
            "BTC-USDC",
            ex_a.clone(),
            ex_b.clone(),
            dec!(0.0001),
            dec!(100.0),
            dec!(6000.0),
            dec!(4.0),
        );

        let opp1 = strat1.scan_opportunity().await.expect("scan").expect("opp1 should exist");
        assert_eq!(opp1.short_venue, "hyperliquid");
        assert_eq!(opp1.long_venue, "lighter");

        let res1 = strat1.execute_open_carry(&opp1, &coordinator).await.expect("execute open");
        match res1 {
            ArbitrageExecutionResult::Success { leg_a, leg_b, .. } => {
                // Long is B (Lighter), Short is A (Hyperliquid)
                assert_eq!(leg_a.venue, ex_b.venue_id());
                assert_eq!(leg_b.venue, ex_a.venue_id());
            }
            other => panic!("Expected Success, got {:?}", other),
        }

        // Scenario 2: B has higher funding (0.04% / 8h) vs A (0.01% / 8h)
        // Expected: Short B (Lighter), Long A (Hyperliquid)
        ex_a.seed_market("ETH-USDC", dec!(3000.0), dec!(3005.0), dec!(5.0), dec!(0.0001));
        ex_b.seed_market("ETH-USDC", dec!(3000.0), dec!(3005.0), dec!(5.0), dec!(0.0004));

        let strat2 = FundingRateArbStrategy::new(
            "ETH-USDC",
            ex_a.clone(),
            ex_b.clone(),
            dec!(0.0001),
            dec!(100.0),
            dec!(6000.0),
            dec!(4.0),
        );

        let opp2 = strat2.scan_opportunity().await.expect("scan").expect("opp2 should exist");
        assert_eq!(opp2.short_venue, "lighter");
        assert_eq!(opp2.long_venue, "hyperliquid");

        let res2 = strat2.execute_open_carry(&opp2, &coordinator).await.expect("execute open");
        match res2 {
            ArbitrageExecutionResult::Success { leg_a, leg_b, .. } => {
                // Long is A (Hyperliquid), Short is B (Lighter)
                assert_eq!(leg_a.venue, ex_a.venue_id());
                assert_eq!(leg_b.venue, ex_b.venue_id());
            }
            other => panic!("Expected Success, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_stat_arb_negative_beta_hedging() {
        let ex = Arc::new(SimulatedExchange::new("hyperliquid", "Hyperliquid", 0, dec!(2.0), dec!(20000.0)));
        ex.seed_market("Y", dec!(100.0), dec!(100.5), dec!(10.0), dec!(0));
        ex.seed_market("X", dec!(50.0), dec!(50.2), dec!(10.0), dec!(0));

        let mut strat = StatArbPairsStrategy::new(
            "Y",
            "X",
            ex.clone(),
            dec!(1.5),
            dec!(0.5),
            dec!(2.0),
        );

        // Manually configure Kalman filter with negative beta
        strat.kalman.state = [0.0, -1.5]; // Beta = -1.5 (inverse relationship)
        strat.spread_stats.push(dec!(10.0));
        strat.spread_stats.push(dec!(-10.0));

        let step_res = strat.step().await.expect("step");
        if let Some((side_y, side_x, qty_y, qty_x)) = step_res {
            assert!(qty_x > dec!(0));
            assert_eq!(qty_y, dec!(2.0));
            assert_eq!(qty_x, dec!(3.0)); // 2.0 * |-1.5| = 3.0
            // When beta < 0, Long Y requires Buying X (same side) rather than Selling X
            if side_y == arbitrageur_core::types::Side::Buy {
                assert_eq!(side_x, arbitrageur_core::types::Side::Buy);
            }
        }
    }
}
