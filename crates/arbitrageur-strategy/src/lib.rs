pub mod convergence_auto;
pub mod engine;
pub mod funding_rate_arb;
pub mod hawkes_flow;
pub mod options_dispersion;
pub mod pair_trim;
pub mod prediction_market_arb;
pub mod spatial_exit;
pub mod spatial_price_arb;
pub mod stat_arb_pairs;

pub use convergence_auto::*;
pub use engine::*;
pub use funding_rate_arb::*;
pub use hawkes_flow::*;
pub use options_dispersion::*;
pub use pair_trim::*;
pub use prediction_market_arb::*;
pub use spatial_exit::*;
pub use spatial_price_arb::*;
pub use stat_arb_pairs::*;

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrageur_core::exchange::ExchangeAdapter;
    use arbitrageur_core::types::dec;
    use arbitrageur_exchanges::simulated::SimulatedExchange;
    use arbitrageur_execution::coordinator::{
        ArbitrageExecutionResult, AtomicArbitrageCoordinator,
    };
    use arbitrageur_execution::smart_routing::SmartOrderRouter;
    use arbitrageur_risk::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
    use std::sync::Arc;

    #[tokio::test]
    async fn test_spatial_strategy_opportunity_detection() {
        let ex_a = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(10000.0),
        ));
        let ex_b = Arc::new(SimulatedExchange::new(
            "lighter",
            "Lighter",
            0,
            dec!(2.0),
            dec!(10000.0),
        ));

        // Market A: Bid 60000, Ask 60010
        ex_a.seed_market(
            "BTC-USDC",
            dec!(60000.0),
            dec!(60010.0),
            dec!(5.0),
            dec!(0.0001),
        );
        // Market B: Bid 60150, Ask 60160 (Higher price on Lighter!)
        ex_b.seed_market(
            "BTC-USDC",
            dec!(60150.0),
            dec!(60160.0),
            dec!(5.0),
            dec!(0.0001),
        );

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
            1, // 1 min sample for unit test
        );

        let opp = strat
            .evaluate_opportunity()
            .await
            .expect("Evaluate opportunity");
        assert!(opp.is_some());
        let signal = opp.unwrap();
        // Should buy on A (ask 60010) and sell on B (bid 60150)
        assert_eq!(signal.direction, SpatialDirection::BuyASellB);
        assert!(signal.expected_profit_usd > dec!(50.0));

        let cb = Arc::new(CircuitBreaker::new(
            CircuitBreakerConfig::default(),
            dec!(10000),
        ));
        let router = SmartOrderRouter::new(dec!(10));
        let coordinator = AtomicArbitrageCoordinator::new(router, cb);

        let result = strat.execute_signal(&signal, &coordinator).await;
        match result {
            ArbitrageExecutionResult::Success {
                realized_profit,
                leg_a,
                leg_b,
            } => {
                // Realized cash P&L on an open is fee outflow only
                assert_eq!(
                    realized_profit,
                    -(leg_a.fee.unwrap_or_default() + leg_b.fee.unwrap_or_default())
                );
            }
            other => panic!("Expected Success, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_prediction_market_strategy_both_directions() {
        let ex_poly = Arc::new(SimulatedExchange::new(
            "polymarket",
            "Polymarket",
            0,
            dec!(0.0),
            dec!(1000.0),
        ));
        let ex_kalshi = Arc::new(SimulatedExchange::new(
            "kalshi",
            "Kalshi",
            0,
            dec!(0.0),
            dec!(1000.0),
        ));

        let cb = Arc::new(CircuitBreaker::new(
            CircuitBreakerConfig::default(),
            dec!(10000),
        ));
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

        let opp1 = strat1
            .check_opportunity()
            .await
            .expect("check opportunity")
            .expect("opp1 should exist");
        assert_eq!(opp1.buy_yes_venue, "Polymarket");
        assert_eq!(opp1.buy_no_venue, "Kalshi");

        let res1 = strat1.execute(&opp1, &coordinator).await;
        match res1 {
            ArbitrageExecutionResult::Success {
                realized_profit,
                leg_a,
                leg_b,
            } => {
                assert_eq!(leg_a.venue, ex_poly.venue_id());
                assert_eq!(leg_a.symbol, "FED-YES");
                assert_eq!(leg_b.venue, ex_kalshi.venue_id());
                assert_eq!(leg_b.symbol, "FED-NO");
                assert_eq!(
                    realized_profit,
                    -(leg_a.fee.unwrap_or_default() + leg_b.fee.unwrap_or_default())
                );
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

        let opp2 = strat2
            .check_opportunity()
            .await
            .expect("check opportunity")
            .expect("opp2 should exist");
        assert_eq!(opp2.buy_yes_venue, "Kalshi");
        assert_eq!(opp2.buy_no_venue, "Polymarket");

        let res2 = strat2.execute(&opp2, &coordinator).await;
        match res2 {
            ArbitrageExecutionResult::Success {
                realized_profit,
                leg_a,
                leg_b,
            } => {
                // Leg A is Polymarket (must buy NO)
                assert_eq!(leg_a.venue, ex_poly.venue_id());
                assert_eq!(leg_a.symbol, "ELEC-NO");
                assert_eq!(leg_a.avg_fill_price, Some(dec!(0.50)));
                // Leg B is Kalshi (must buy YES)
                assert_eq!(leg_b.venue, ex_kalshi.venue_id());
                assert_eq!(leg_b.symbol, "ELEC-YES");
                assert_eq!(leg_b.avg_fill_price, Some(dec!(0.42)));
                assert_eq!(
                    realized_profit,
                    -(leg_a.fee.unwrap_or_default() + leg_b.fee.unwrap_or_default())
                );
            }
            other => panic!("Expected Success, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_funding_rate_strategy_both_directions() {
        let ex_a = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));
        let ex_b = Arc::new(SimulatedExchange::new(
            "lighter",
            "Lighter",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));

        let cb = Arc::new(CircuitBreaker::new(
            CircuitBreakerConfig::default(),
            dec!(50000),
        ));
        let router = SmartOrderRouter::new(dec!(10));
        let coordinator = AtomicArbitrageCoordinator::new(router, cb);

        // Scenario 1: A has higher funding (0.03% / 8h) vs B (0.01% / 8h)
        // Expected: Short A (Hyperliquid), Long B (Lighter)
        ex_a.seed_market(
            "BTC-USDC",
            dec!(60000.0),
            dec!(60010.0),
            dec!(5.0),
            dec!(0.0003),
        );
        ex_b.seed_market(
            "BTC-USDC",
            dec!(60000.0),
            dec!(60010.0),
            dec!(5.0),
            dec!(0.0001),
        );

        let strat1 = FundingRateArbStrategy::new(
            "BTC-USDC",
            ex_a.clone(),
            ex_b.clone(),
            dec!(0.0001),
            dec!(100.0),
            dec!(6000.0),
            dec!(4.0),
        );

        let opp1 = strat1
            .scan_opportunity()
            .await
            .expect("scan")
            .expect("opp1 should exist");
        assert_eq!(opp1.short_venue, "hyperliquid");
        assert_eq!(opp1.long_venue, "lighter");

        let res1 = strat1
            .execute_open_carry(&opp1, &coordinator)
            .await
            .expect("execute open");
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
        ex_a.seed_market(
            "ETH-USDC",
            dec!(3000.0),
            dec!(3005.0),
            dec!(5.0),
            dec!(0.0001),
        );
        ex_b.seed_market(
            "ETH-USDC",
            dec!(3000.0),
            dec!(3005.0),
            dec!(5.0),
            dec!(0.0004),
        );

        let strat2 = FundingRateArbStrategy::new(
            "ETH-USDC",
            ex_a.clone(),
            ex_b.clone(),
            dec!(0.0001),
            dec!(100.0),
            dec!(6000.0),
            dec!(4.0),
        );

        let opp2 = strat2
            .scan_opportunity()
            .await
            .expect("scan")
            .expect("opp2 should exist");
        assert_eq!(opp2.short_venue, "lighter");
        assert_eq!(opp2.long_venue, "hyperliquid");

        let res2 = strat2
            .execute_open_carry(&opp2, &coordinator)
            .await
            .expect("execute open");
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
    async fn test_funding_strategy_leung_li_and_vecm_discovery() {
        let ex_a = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));
        let ex_b = Arc::new(SimulatedExchange::new(
            "lighter",
            "Lighter",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));

        let strat = FundingRateArbStrategy::new(
            "BTC-PERP",
            ex_a,
            ex_b,
            dec!(0.0001),
            dec!(100.0),
            dec!(6000.0),
            dec!(4.0),
        )
        .with_leung_li_router(
            arbitrageur_math::fast_basis_harvest::LeungLiBasisStoppingRouter::default(),
        )
        .with_vecm_analyzer(arbitrageur_math::fast_basis_harvest::VecmBasisAnalyzer::new(40));

        // Evaluate Leung-Li exit
        let exit_decision = strat
            .evaluate_leung_li_exit(
                35.0, // current basis 35 bps
                5.0,  // friction 5 bps
                2.0,  // theta 2 bps
                12.0, // sigma 12 bps
                0.06, // kappa
                0.16, // rho
                8.0,  // carry yield 8%
            )
            .expect("Leung-Li exit decision should exist");

        match exit_decision {
            arbitrageur_math::fast_basis_harvest::BasisExitDecision::TakeProfitConvergence {
                exit_spread_bps,
                hurdle_b_u_bps,
            } => {
                assert_eq!(exit_spread_bps, 35.0);
                assert!(hurdle_b_u_bps < 35.0);
            }
            other => panic!("Expected TakeProfitConvergence, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_funding_strategy_dynamic_fast_basis_harvest() {
        let ex_a = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));
        let ex_b = Arc::new(SimulatedExchange::new(
            "lighter",
            "Lighter",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));

        let spline = arbitrageur_math::funding_spline::FritschCarlsonMonotoneSpline::fit(
            &[1.0, 4.0, 8.0, 24.0],
            &[4.0, 7.5, 12.0, 16.5],
        )
        .expect("monotone spline should fit");

        let harvester =
            arbitrageur_math::funding_spline::DynamicFastBasisHarvestEngine::new(2.5, 0.25, 1.0);

        let strat = FundingRateArbStrategy::new(
            "BTC-PERP",
            ex_a,
            ex_b,
            dec!(0.0001),
            dec!(100.0),
            dec!(10000.0),
            dec!(4.0),
        )
        .with_funding_spline(spline)
        .with_dynamic_fast_basis_harvest_engine(harvester);

        assert!(strat.funding_spline.is_some());
        assert!(strat.dynamic_fast_basis_harvest_engine.is_some());

        // Fast compression scenario: 25 bps basis collapsed to 3 bps in 0.5 hours
        let decision = strat
            .evaluate_dynamic_fast_basis_harvest(
                25.0, // entry basis
                3.0,  // current basis
                0.5,  // elapsed hours
                8.0,  // epoch hours
                0.8,  // accrued funding
                2.5,  // round trip fee
                18.0, // passive target pnl
                10000.0,
            )
            .expect("decision should be returned");

        assert!(decision.should_harvest_early);
        assert!(decision.irr_velocity_multiplier >= 2.5);
        assert!(decision.message.contains("Fast Basis Harvest Triggered"));
    }

    #[tokio::test]
    async fn test_stat_arb_negative_beta_hedging() {
        let ex = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));
        ex.seed_market("Y", dec!(100.0), dec!(100.5), dec!(10.0), dec!(0));
        ex.seed_market("X", dec!(50.0), dec!(50.2), dec!(10.0), dec!(0));

        let mut strat =
            StatArbPairsStrategy::new("Y", "X", ex.clone(), dec!(1.5), dec!(0.5), dec!(2.0))
                .expect("valid config")
                .with_round_trip_fee_bps(dec!(0))
                .with_min_profit_hurdle_bps(dec!(0));

        // Manually configure Kalman filter with negative beta and fixed state
        strat.kalman.state = [0.0, -1.5]; // Beta = -1.5 (inverse relationship)
        strat.kalman.p = [[0.0, 0.0], [0.0, 0.0]];
        strat.kalman.q = [0.0, 0.0]; // Zero process noise freezes beta under single tick
        strat.spread_stats.push(dec!(10.0));
        strat.spread_stats.push(dec!(-10.0));

        let step_res = strat.step().await.expect("step");
        let signal =
            step_res.expect("should emit entry signal when z-score breaches entry threshold");
        match signal {
            SpreadSignal::Enter {
                side_y,
                side_x,
                qty_y,
                qty_x,
            } => {
                assert!(qty_x > dec!(0));
                assert_eq!(qty_y, dec!(2.0));
                assert_eq!(qty_x, dec!(3.0)); // 2.0 * |-1.5| = 3.0
                                              // When beta < 0, same direction: Long Y buys X, Short Y sells X
                if side_y == arbitrageur_core::types::Side::Buy {
                    assert_eq!(side_x, arbitrageur_core::types::Side::Buy);
                } else {
                    assert_eq!(side_x, arbitrageur_core::types::Side::Sell);
                }
            }
            _ => panic!("Expected Enter signal"),
        }
    }

    /// Regression (#46): entry/exit z-scores are validated â€” an exit band
    /// outside the entry band can never trigger, and non-positive entries
    /// are meaningless.
    #[tokio::test]
    async fn test_stat_arb_config_validation() {
        let ex = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));
        // exit >= entry is unreachable.
        assert!(
            StatArbPairsStrategy::new("Y", "X", ex.clone(), dec!(1.0), dec!(1.5), dec!(2.0))
                .is_err()
        );
        // non-positive entry is meaningless.
        assert!(
            StatArbPairsStrategy::new("Y", "X", ex.clone(), dec!(0.0), dec!(0.0), dec!(2.0))
                .is_err()
        );
        // negative exit band rejected.
        assert!(
            StatArbPairsStrategy::new("Y", "X", ex.clone(), dec!(1.5), dec!(-0.5), dec!(2.0))
                .is_err()
        );
        // sane config accepted.
        assert!(StatArbPairsStrategy::new("Y", "X", ex, dec!(1.5), dec!(0.5), dec!(2.0)).is_ok());
    }

    #[tokio::test]
    async fn test_stat_arb_structural_stop_loss() {
        let ex = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));
        // Y at 99.8, X at 50 with beta=2 => residual = 99.8 - 100 = -0.2. With sqrt(R)=0.1, z = -2.0 (breaches entry threshold -1.5)
        ex.seed_market("Y", dec!(99.8), dec!(100.0), dec!(10.0), dec!(0));
        ex.seed_market("X", dec!(50.0), dec!(50.2), dec!(10.0), dec!(0));

        let mut strat =
            StatArbPairsStrategy::new("Y", "X", ex.clone(), dec!(1.5), dec!(0.5), dec!(2.0))
                .expect("valid config")
                .with_round_trip_fee_bps(dec!(0))
                .with_min_profit_hurdle_bps(dec!(0))
                .with_stop_loss_z_score(Some(dec!(3.0)))
                .expect("valid stop loss");

        // Manually configure Kalman filter
        strat.kalman.state = [0.0, 2.0]; // Y = 2 * X
        strat.kalman.p = [[0.0, 0.0], [0.0, 0.0]];
        strat.kalman.q = [0.0, 0.0];

        // Step 1: Entry signal triggers at z = -2.0
        let entry_sig = strat.step().await.expect("step").expect("entry signal");
        assert!(matches!(entry_sig, SpreadSignal::Enter { .. }));

        // Now simulate adverse divergence: Y drops to 99.0 while X is at 50.0
        // Frozen spread: 99.0 - 100.0 = -1.0. eval_z = -1.0 / 0.1 = -10.0 (breaches stop loss limit -3.0)
        ex.seed_market("Y", dec!(99.0), dec!(99.2), dec!(10.0), dec!(0));
        let exit_sig = strat
            .step()
            .await
            .expect("step")
            .expect("stop loss exit signal");
        assert!(matches!(exit_sig, SpreadSignal::Exit { .. }));
    }

    #[tokio::test]
    async fn test_stat_arb_holding_duration_timeout() {
        let ex = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));
        // Y at 99.8, X at 50 with beta=2 => residual = 99.8 - 100 = -0.2. With sqrt(R)=0.1, z = -2.0
        ex.seed_market("Y", dec!(99.8), dec!(100.0), dec!(10.0), dec!(0));
        ex.seed_market("X", dec!(50.0), dec!(50.2), dec!(10.0), dec!(0));

        let mut strat =
            StatArbPairsStrategy::new("Y", "X", ex.clone(), dec!(1.5), dec!(0.5), dec!(2.0))
                .expect("valid config")
                .with_round_trip_fee_bps(dec!(0))
                .with_min_profit_hurdle_bps(dec!(0))
                .with_stop_loss_z_score(Some(dec!(5.0)))
                .expect("valid stop loss")
                .with_max_holding_ticks(2);

        strat.kalman.state = [0.0, 2.0];
        strat.kalman.p = [[0.0, 0.0], [0.0, 0.0]];
        strat.kalman.q = [0.0, 0.0];

        // Step 1: Entry at z = -2.0
        let entry_sig = strat.step().await.expect("step").expect("entry signal");
        assert!(matches!(entry_sig, SpreadSignal::Enter { .. }));

        // Step 2: In position, holding_ticks = 1 < 2, price unchanged at 99.8 => eval_z = -2.0 (holds)
        let mid_sig = strat.step().await.expect("step");
        assert!(mid_sig.is_none());

        // Step 3: In position, holding_ticks = 2 >= 2 => Timeout exit!
        let timeout_sig = strat
            .step()
            .await
            .expect("step")
            .expect("timeout exit signal");
        assert!(matches!(timeout_sig, SpreadSignal::Exit { .. }));
    }

    #[tokio::test]
    async fn test_stat_arb_frozen_entry_prevents_beta_chaser() {
        let ex = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));
        ex.seed_market("Y", dec!(99.8), dec!(100.0), dec!(10.0), dec!(0));
        ex.seed_market("X", dec!(50.0), dec!(50.2), dec!(10.0), dec!(0));

        let mut strat =
            StatArbPairsStrategy::new("Y", "X", ex.clone(), dec!(1.5), dec!(0.5), dec!(2.0))
                .expect("valid config")
                .with_round_trip_fee_bps(dec!(0))
                .with_min_profit_hurdle_bps(dec!(0))
                .with_stop_loss_z_score(Some(dec!(8.0)))
                .expect("valid stop loss")
                .with_max_holding_ticks(10);

        strat.kalman.state = [0.0, 2.0];
        strat.kalman.p = [[0.0, 0.0], [0.0, 0.0]];
        strat.kalman.q = [0.0, 0.0];

        // Step 1: Entry at z = -2.0
        let entry_sig = strat.step().await.expect("step").expect("entry signal");
        assert!(matches!(entry_sig, SpreadSignal::Enter { .. }));

        // Now Y drops to 99.5. The true physical spread (locked at entry beta=2.0) has widened:
        // frozen_residual = 99.6 - 100.2 = -0.6 => frozen_z = -6.0.
        // Even if we manually corrupt online Kalman state to beta = 1.99 (which would give online residual ~0):
        strat.kalman.state = [0.0, 1.99]; // Artificially simulate beta chaser drift
        ex.seed_market("Y", dec!(99.5), dec!(99.7), dec!(10.0), dec!(0));

        // Step 2: Frozen model must compute eval_z = -6.0 and NOT trigger a false reversion exit!
        let step_sig = strat.step().await.expect("step");
        assert!(
            step_sig.is_none(),
            "Strategy must not trigger phantom reversion exit when online beta drifts"
        );
    }

    #[tokio::test]
    async fn test_stat_arb_extreme_beta_rejection() {
        let ex = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));
        ex.seed_market("Y", dec!(100.0), dec!(100.5), dec!(10.0), dec!(0));
        ex.seed_market("X", dec!(50.0), dec!(50.2), dec!(10.0), dec!(0));

        let mut strat =
            StatArbPairsStrategy::new("Y", "X", ex.clone(), dec!(1.5), dec!(0.5), dec!(2.0))
                .expect("valid config");

        // Manually configure Kalman filter with extreme anomalous beta = 25.0
        strat.kalman.state = [0.0, 25.0];
        strat.kalman.p = [[0.0, 0.0], [0.0, 0.0]];
        strat.kalman.q = [0.0, 0.0];
        strat.spread_stats.push(dec!(100.0));
        strat.spread_stats.push(dec!(-100.0));

        let step_res = strat.step().await.expect("step");
        assert!(
            step_res.is_none(),
            "Anomalous beta > 10.0 must be rejected to prevent runaway order sizes"
        );
    }

    #[tokio::test]
    async fn test_stat_arb_dynamic_beta_drift_rebalance() {
        let ex = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));
        ex.seed_market("Y", dec!(99.8), dec!(100.0), dec!(10.0), dec!(0));
        ex.seed_market("X", dec!(50.0), dec!(50.2), dec!(10.0), dec!(0));

        let mut strat =
            StatArbPairsStrategy::new("Y", "X", ex.clone(), dec!(1.5), dec!(0.5), dec!(2.0))
                .expect("valid config")
                .with_round_trip_fee_bps(dec!(0))
                .with_min_profit_hurdle_bps(dec!(0))
                .with_beta_rebalance_threshold_pct(0.15);

        strat.kalman.state = [0.0, 2.0];
        strat.kalman.p = [[0.0, 0.0], [0.0, 0.0]];
        strat.kalman.q = [0.0, 0.0];

        // Step 1: Entry signal (Long spread: Buy Y (2.0), Sell X (4.0))
        let entry_sig = strat.step().await.expect("step").expect("entry signal");
        assert!(matches!(entry_sig, SpreadSignal::Enter { .. }));

        // Now beta drifts by 25% from 2.0 to 2.5
        strat.kalman.state = [0.0, 2.5];
        // Target Q_x = 2.0 * 2.5 = 5.0. Current Q_x = 4.0. Delta Q_x = 1.0 (Sell 1.0 X).
        let rebal_sig = strat.step().await.expect("step").expect("rebalance signal");
        match rebal_sig {
            SpreadSignal::RebalanceHedge {
                side_x,
                delta_qty_x,
                new_total_qty_x,
            } => {
                assert_eq!(side_x, arbitrageur_core::types::Side::Sell);
                assert!(delta_qty_x >= dec!(0.9) && delta_qty_x <= dec!(1.1));
                assert!(new_total_qty_x >= dec!(4.9) && new_total_qty_x <= dec!(5.1));
            }
            other => panic!("Expected RebalanceHedge signal, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_stat_arb_fee_hurdle_rejection() {
        let ex = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));
        ex.seed_market("Y", dec!(99.8), dec!(100.0), dec!(10.0), dec!(0));
        ex.seed_market("X", dec!(50.0), dec!(50.2), dec!(10.0), dec!(0));

        let mut strat =
            StatArbPairsStrategy::new("Y", "X", ex.clone(), dec!(1.5), dec!(0.5), dec!(2.0))
                .expect("valid config")
                .with_round_trip_fee_bps(dec!(30.0))
                .with_min_profit_hurdle_bps(dec!(25.0));

        strat.kalman.state = [0.0, 2.0];
        strat.kalman.p = [[0.0, 0.0], [0.0, 0.0]];
        strat.kalman.q = [0.0, 0.0];
        strat.kalman.latest_variance = 0.01;

        let step_res = strat.step().await.expect("step");
        assert!(
            step_res.is_none(),
            "StatArb must reject entries where expected reversion edge is below fee hurdle"
        );
    }

    #[tokio::test]
    async fn test_stat_arb_rolling_cointegration_breakdown() {
        let ex = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));
        ex.seed_market("Y", dec!(99.8), dec!(100.0), dec!(10.0), dec!(0));
        ex.seed_market("X", dec!(50.0), dec!(50.2), dec!(10.0), dec!(0));

        let mut strat =
            StatArbPairsStrategy::new("Y", "X", ex.clone(), dec!(1.5), dec!(0.5), dec!(2.0))
                .expect("valid config")
                .with_round_trip_fee_bps(dec!(0))
                .with_min_profit_hurdle_bps(dec!(0))
                .with_stop_loss_z_score(Some(dec!(8.0)))
                .expect("valid stop loss")
                .with_max_holding_ticks(100);

        strat.kalman.state = [0.0, 2.0];
        strat.kalman.p = [[0.0, 0.0], [0.0, 0.0]];
        strat.kalman.q = [0.0, 0.0];

        // Step 1: Enter position
        let entry_sig = strat.step().await.expect("step").expect("entry signal");
        assert!(matches!(entry_sig, SpreadSignal::Enter { .. }));

        // Now populate history with 60 non-cointegrated random walk prices
        strat.prices_y.clear();
        strat.prices_x.clear();
        let mut cur_y = 100.0;
        let mut cur_x = 50.0;
        for i in 0..60 {
            cur_y += ((i % 3) as f64 - 1.0) * 1.5;
            cur_x += (((i * 7) % 5) as f64 - 2.0) * 1.5;
            strat.prices_y.push(cur_y);
            strat.prices_x.push(cur_x);
        }

        // Set holding_ticks = 29 so the next step reaches tick 30 (multiples of 30 trigger ADF check)
        strat.holding_ticks = 29;

        // Ensure current prices don't trip stop loss (eval_z within bounds)
        strat.entry_snapshot = Some(stat_arb_pairs::EntryModelSnapshot {
            alpha: 0.0,
            beta: 2.0,
            variance: 100.0,
        });

        let exit_sig = strat
            .step()
            .await
            .expect("step")
            .expect("cointegration breakdown exit signal");
        assert!(
            matches!(exit_sig, SpreadSignal::Exit { .. }),
            "Must exit on structural cointegration breakdown"
        );
    }

    #[tokio::test]
    async fn test_spatial_strategy_inventory_skew_reservation_hurdle() {
        let ex_a = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(10000.0),
        ));
        let ex_b = Arc::new(SimulatedExchange::new(
            "lighter",
            "Lighter",
            0,
            dec!(2.0),
            dec!(10000.0),
        ));

        // Spread: Bid B 60100 vs Ask A 60000 -> 100 profit (~16.6 bps net)
        ex_a.seed_market(
            "BTC-USDC",
            dec!(59990.0),
            dec!(60000.0),
            dec!(5.0),
            dec!(0.0001),
        );
        ex_b.seed_market(
            "BTC-USDC",
            dec!(60100.0),
            dec!(60110.0),
            dec!(5.0),
            dec!(0.0001),
        );

        let skew_cfg = arbitrageur_math::inventory_skew::InventorySkewConfig {
            risk_aversion_gamma: 0.5,
            barrier_kappa: 2.0,
            max_inventory_usd: 10_000.0,
            min_friction_bps: 2.0,
        };

        // When inventory is heavily positive on Venue A (+8000 USD),
        // entering BuyASellB further expands inventory imbalance -> hurdle spikes.
        let mut strat = SpatialPriceArbStrategy::new(
            "BTC-USDC",
            ex_a.clone(),
            ex_b.clone(),
            dec!(5.0), // 5 BPS target profit
            dec!(10.0),
            dec!(0.5),
            dec!(2.0),
            dec!(2.0),
            50,
            1,
        )
        .with_inventory_skew(skew_cfg, dec!(8000.0));

        let hurdle_inc = strat.effective_hurdle_bps(spatial_price_arb::SpatialDirection::BuyASellB);
        assert!(
            hurdle_inc > dec!(30.0),
            "Hurdle for skew-increasing direction must spike: {hurdle_inc}"
        );

        // For rebalancing direction (BuyBSellA), hurdle is discounted
        let hurdle_dec = strat.effective_hurdle_bps(spatial_price_arb::SpatialDirection::BuyBSellA);
        assert!(
            hurdle_dec <= dec!(5.0),
            "Hurdle for rebalancing direction must be discounted: {hurdle_dec}"
        );

        // With 16.6 bps edge, BuyASellB is blocked by elevated hurdle (30+ bps)
        let opp = strat.evaluate_opportunity().await.expect("eval");
        assert!(
            opp.is_none(),
            "Opportunity should be blocked by inventory skew penalty"
        );

        // When inventory is reset to neutral, opportunity is accepted
        strat.set_net_inventory_usd(dec!(0));
        let opp_neutral = strat.evaluate_opportunity().await.expect("eval");
        assert!(
            opp_neutral.is_some(),
            "Opportunity should be accepted when inventory is neutral"
        );
    }

    #[tokio::test]
    async fn test_funding_strategy_cusum_collapse_blocking() {
        let ex_hl = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(10000.0),
        ));
        let ex_light = Arc::new(SimulatedExchange::new(
            "lighter",
            "Lighter",
            0,
            dec!(2.0),
            dec!(10000.0),
        ));

        // High funding rate on Hyperliquid (+0.05% per 8h = 0.0005)
        ex_hl.seed_market(
            "ETH-PERP",
            dec!(3000.0),
            dec!(3001.0),
            dec!(10.0),
            dec!(0.0005),
        );
        ex_light.seed_market(
            "ETH-PERP",
            dec!(3000.0),
            dec!(3001.0),
            dec!(10.0),
            dec!(0.0001),
        );

        let mut detector =
            arbitrageur_math::regime_switching::CusumChangeDetector::default_funding_detector();
        // Force detector into alarm state by feeding consecutive negative drops
        for _ in 0..15 {
            detector.update(-0.001);
        }

        let strat = FundingRateArbStrategy::new(
            "ETH-PERP",
            ex_hl.clone(),
            ex_light.clone(),
            dec!(0.0001),
            dec!(168.0),
            dec!(1000.0),
            dec!(5.0),
        )
        .with_regime_detector(detector);

        let opp = strat.scan_opportunity().await.expect("scan");
        assert!(opp.is_none(), "CUSUM alarm must block funding carry entry");
    }

    #[tokio::test]
    async fn test_funding_strategy_entry_spread_hurdle_blocks_entry() {
        use arbitrageur_execution::coordinator::AtomicArbitrageCoordinator;
        use arbitrageur_execution::smart_routing::SmartOrderRouter;
        use arbitrageur_risk::circuit_breaker::CircuitBreaker;

        let ex_hl = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(10000.0),
        ));
        let ex_light = Arc::new(SimulatedExchange::new(
            "lighter",
            "Lighter",
            0,
            dec!(2.0),
            dec!(10000.0),
        ));

        // Short venue: Hyperliquid (bid 3000, ask 3001, funding +0.0005)
        ex_hl.seed_market(
            "ETH-PERP",
            dec!(3000.0),
            dec!(3001.0),
            dec!(10.0),
            dec!(0.0005),
        );
        // Long venue: Lighter (bid 3009, ask 3010, funding +0.0001)
        // Entry spread: ask (3010) - bid (3000) = 10.0, on mid 3005 is ~33.2 bps
        ex_light.seed_market(
            "ETH-PERP",
            dec!(3009.0),
            dec!(3010.0),
            dec!(10.0),
            dec!(0.0001),
        );

        let strat = FundingRateArbStrategy::new(
            "ETH-PERP",
            ex_hl.clone(),
            ex_light.clone(),
            dec!(0.0001),
            dec!(168.0),
            dec!(1000.0),
            dec!(5.0),
        )
        .with_max_entry_spread_bps(dec!(15.0));

        let opp = strat
            .scan_opportunity()
            .await
            .expect("scan")
            .expect("opportunity exists");

        use arbitrageur_risk::circuit_breaker::CircuitBreakerConfig;

        let router = SmartOrderRouter::new(dec!(20.0));
        let cb = Arc::new(CircuitBreaker::new(
            CircuitBreakerConfig::default(),
            dec!(10000),
        ));
        let coordinator = AtomicArbitrageCoordinator::new(router, cb);

        let res = strat.execute_open_carry(&opp, &coordinator).await;
        assert!(res.is_err(), "Must reject entry when spread exceeds 15 bps");
        let err_msg = res.unwrap_err().to_string();
        assert!(
            err_msg.contains("exceeds maximum allowed 15.00 bps"),
            "Error msg: {err_msg}"
        );
    }

    #[tokio::test]
    async fn test_funding_strategy_kalman_filter_inversion_blocking() {
        use arbitrageur_math::kalman_filter::FundingPremiumKalmanFilter;

        let ex_hl = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(10000.0),
        ));
        let ex_light = Arc::new(SimulatedExchange::new(
            "lighter",
            "Lighter",
            0,
            dec!(2.0),
            dec!(10000.0),
        ));

        // Short venue: Hyperliquid (funding +0.0002)
        ex_hl.seed_market(
            "ETH-PERP",
            dec!(3000.0),
            dec!(3001.0),
            dec!(10.0),
            dec!(0.0002),
        );
        // Long venue: Lighter (funding +0.0001)
        ex_light.seed_market(
            "ETH-PERP",
            dec!(3000.0),
            dec!(3001.0),
            dec!(10.0),
            dec!(0.0001),
        );

        // Pre-condition Kalman filter with a rapidly declining rate:
        // Premium was 10, then 8, then 5, then 2, now 1 (drifting down sharply)
        let mut kf = FundingPremiumKalmanFilter::new(0.05, 0.01, 0.10);
        kf.update(10.0, 1.0);
        kf.update(7.0, 1.0);
        kf.update(4.0, 1.0);
        kf.update(1.0, 1.0);

        let strat = FundingRateArbStrategy::new(
            "ETH-PERP",
            ex_hl.clone(),
            ex_light.clone(),
            dec!(0.00005),
            dec!(168.0),
            dec!(1000.0),
            dec!(1.0),
        )
        .with_kalman_filter(kf);

        // Scan opportunity should detect forecasted inversion and block entry
        let opp = strat.scan_opportunity().await.expect("scan");
        assert!(
            opp.is_none(),
            "Kalman filter forecasting inversion must block funding carry entry"
        );
    }

    #[tokio::test]
    async fn test_stat_arb_hamilton_regime_switching_gating_and_exit() {
        use arbitrageur_math::factor_stat_arb::HamiltonKalmanSpreadFilter;

        let ex = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));
        ex.seed_market("Y", dec!(99.8), dec!(100.0), dec!(10.0), dec!(0));
        ex.seed_market("X", dec!(50.0), dec!(50.2), dec!(10.0), dec!(0));

        let mut hf = HamiltonKalmanSpreadFilter::new(12.0, 0.0, 0.02, 0.01, 0.5, 0.15, 0.98, 0.98);

        // Precondition filter into trending/breakdown regime:
        hf.prob_regime1 = 0.25;

        let mut strat =
            StatArbPairsStrategy::new("Y", "X", ex.clone(), dec!(1.5), dec!(0.5), dec!(2.0))
                .expect("valid config")
                .with_hamilton_filter(hf);

        strat.kalman.state = [0.0, 2.0];
        strat.kalman.latest_variance = 0.01;
        strat.spread_stats.push(dec!(10.0));
        strat.spread_stats.push(dec!(-10.0));

        // 1. Entry should be blocked while in breakdown regime
        let step_entry = strat.step().await.expect("step");
        assert!(
            step_entry.is_none(),
            "Entry must be blocked by Hamilton trending/breakdown regime"
        );

        // 2. Now open a position, set regime to breakdown, and verify exit is triggered
        strat.position = SpreadPosition::Long;
        strat.open_sides = Some((
            arbitrageur_core::types::Side::Buy,
            arbitrageur_core::types::Side::Sell,
            dec!(2.0),
            dec!(4.0),
        ));

        // When in position with low regime prob, step() takes exit
        let step_exit = strat.step().await.expect("step");
        assert!(
            step_exit.is_some(),
            "Regime breakdown must trigger early exit"
        );
        match step_exit.unwrap() {
            SpreadSignal::Exit {
                side_y,
                side_x,
                qty_y,
                qty_x,
            } => {
                assert_eq!(side_y, arbitrageur_core::types::Side::Sell);
                assert_eq!(side_x, arbitrageur_core::types::Side::Buy);
                assert_eq!(qty_y, dec!(2.0));
                assert_eq!(qty_x, dec!(4.0));
            }
            other => panic!("Expected Exit signal, got {:?}", other),
        }
        assert_eq!(strat.position, SpreadPosition::Flat);
    }

    #[tokio::test]
    async fn test_stat_arb_vecm_hawkes_gating_and_protective_exit() {
        use arbitrageur_math::hf_vecm::HfVecmHawkesEngine;

        let ex = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));
        ex.seed_market("Y", dec!(99.8), dec!(100.0), dec!(10.0), dec!(0));
        ex.seed_market("X", dec!(50.0), dec!(50.2), dec!(10.0), dec!(0));

        let vecm = HfVecmHawkesEngine::default();

        let mut strat =
            StatArbPairsStrategy::new("Y", "X", ex.clone(), dec!(1.5), dec!(0.5), dec!(2.0))
                .expect("valid config")
                .with_vecm_hawkes_engine(vecm);

        strat.kalman.state = [0.0, 2.0];
        strat.kalman.latest_variance = 0.01;
        strat.spread_stats.push(dec!(10.0));
        strat.spread_stats.push(dec!(-10.0));

        // 1. Set supercritical Hawkes cascade -> entry blocked
        strat.update_hawkes_metrics(20.0, 0.95);
        let step_entry = strat.step().await.expect("step");
        assert!(
            step_entry.is_none(),
            "Supercritical Hawkes burst must block new entries"
        );

        // 2. Open position, trigger supercritical burst -> protective exit
        strat.position = SpreadPosition::Long;
        strat.open_sides = Some((
            arbitrageur_core::types::Side::Buy,
            arbitrageur_core::types::Side::Sell,
            dec!(2.0),
            dec!(4.0),
        ));

        let step_exit = strat.step().await.expect("step");
        assert!(
            step_exit.is_some(),
            "Supercritical Hawkes burst must trigger protective exit"
        );
        match step_exit.unwrap() {
            SpreadSignal::Exit { side_y, side_x, .. } => {
                assert_eq!(side_y, arbitrageur_core::types::Side::Sell);
                assert_eq!(side_x, arbitrageur_core::types::Side::Buy);
            }
            other => panic!("Expected Exit signal, got {:?}", other),
        }
        assert_eq!(strat.position, SpreadPosition::Flat);
    }

    #[tokio::test]
    async fn test_funding_strategy_term_structure_butterfly() {
        use arbitrageur_math::basis_term_structure::NelsonSiegelCurve;

        let ex_a = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));
        let ex_b = Arc::new(SimulatedExchange::new(
            "lighter",
            "Lighter",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));

        let curve = NelsonSiegelCurve::new(0.05, 0.08, 0.06, 0.20);
        let strat = FundingRateArbStrategy::new(
            "BTC-PERP",
            ex_a,
            ex_b,
            dec!(0.0001),
            dec!(168.0),
            dec!(1000.0),
            dec!(2.0),
        )
        .with_term_structure_curve(curve);

        let opp = strat
            .evaluate_term_structure_butterfly(0.0833, 0.25, 0.50, 0.02)
            .expect("butterfly opportunity");

        assert!(opp.is_actionable);
        assert_eq!(opp.weights[1], -1.0); // Short body
        assert!(opp.roll_down_yield_bps > 0.0);
        assert!(opp.action.contains("Sell rich body"));
    }

    #[tokio::test]
    async fn test_stat_arb_rough_fou_gating() {
        use arbitrageur_math::rough_ou::RoughFouSpreadEngine;

        let ex = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));
        ex.seed_market("Y", dec!(99.8), dec!(100.0), dec!(10.0), dec!(0));
        ex.seed_market("X", dec!(50.0), dec!(50.2), dec!(10.0), dec!(0));

        let rf = RoughFouSpreadEngine::new(0.08, 1.5, 0.01, 0.0, 50);

        let mut strat =
            StatArbPairsStrategy::new("Y", "X", ex.clone(), dec!(1.5), dec!(0.5), dec!(2.0))
                .expect("valid config")
                .with_rough_fou_engine(rf);

        strat.kalman.state = [0.0, 2.0];
        strat.kalman.latest_variance = 0.01;

        // Step through market: spread is small (near 0), rough fOU holds within band -> entry blocked
        let step_res = strat.step().await.expect("step");
        assert!(
            step_res.is_none(),
            "Small spread within rough fOU band must not trigger entry"
        );
    }
}
