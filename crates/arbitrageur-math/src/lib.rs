pub mod arakelov_geometry_arb;
pub mod atomic_mev;
pub mod autotune;
pub mod basis_pca;
pub mod basis_term_structure;
pub mod berkovich_analytic_arb;
pub mod binomial;
pub mod black_scholes;
pub mod book_gradient;
pub mod bouchaud_propagator;
pub mod calibration;
pub mod coha_yangian_arb;
pub mod cointegration;
pub mod collateral_barrier;
pub mod collateral_dag;
pub mod concentrated_liquidity;
pub mod condensed_liquid_arb;
pub mod condensed_solid_arb;
pub mod cross_chain_intent_dag;
pub mod cross_hawkes;
pub mod debiased_ou;
pub mod delta_gamma_rehedge;
pub mod derived_artin_stack_arb;
pub mod derived_chow_steenrod_arb;
pub mod derived_critical_locus_arb;
pub mod derived_deformation_arb;
pub mod derived_siegel_modular_arb;
pub mod dislocation;
pub mod dmrg_portfolio;
pub mod donaldson_thomas_arb;
pub mod epoch_jump;
pub mod erc7683_intent_slashing;
pub mod factor_stat_arb;
pub mod fast_basis_harvest;
pub mod fixed_yield;
pub mod floer_novikov_arb;
pub mod fontaine_fargues_arb;
pub mod forecast_calibration;
pub mod funding_carry;
pub mod funding_epoch_countdown;
pub mod funding_spline;
pub mod funding_velocity;
pub mod geometric_langlands_arb;
pub mod grothendieck_six_operations_arb;
pub mod gueant_mm;
pub mod hawkes;
pub mod hf_vecm;
pub mod hmm_liquidity;
pub mod htlc_settlement;
pub mod hyperbolic_poincare_arb;
pub mod intent_bridge;
pub mod inventory_skew;
pub mod kalman_filter;
pub mod koszul_tate_poisson_arb;
pub mod l3_hawkes_mm;
pub mod l3_hazard;
pub mod l3_orderbook;
pub mod l3_tensor_ofi;
pub mod latency_racing;
pub mod lead_lag;
pub mod margin_deleveraging;
pub mod matrix_propagator;
pub mod micro_price_surface;
pub mod motivic_cohomology_arb;
pub mod multi_asset_avellaneda;
pub mod multi_asset_impact;
pub mod multi_commodity_flow;
pub mod neural_stopping_qubo;
pub mod non_abelian_gauge_arb;
pub mod noncommutative_hodge_arb;
pub mod noncommutative_motives_arb;
pub mod optimal_thresholds;
pub mod ou_process;
pub mod padic_simpson_arb;
pub mod pair_monitor;
pub mod perfectoid_shimura_arb;
pub mod perfectoid_tilting_arb;
pub mod perverse_sheaves_arb;
pub mod portfolio_greeks_opt;
pub mod power_law_propagator;
pub mod prediction_synthetic;
pub mod private_relay;
pub mod quantum_frobenius_arb;
pub mod queue_drift;
pub mod queue_priority;
pub mod queue_survival;
pub mod regime_switching;
pub mod relative_prismatic_arb;
pub mod resilience_pacer;
pub mod rigid_analytic_tate_arb;
pub mod risk_math;
pub mod rolling_stats;
pub mod rough_heston_avx;
pub mod rough_ou;
pub mod rough_path_signature;
pub mod seasonality;
pub mod session;
pub mod spatial_spread;
pub mod stochastic_depth;
pub mod susy_instanton;
pub mod svi_arbitrage_free;
pub mod tail_copula;
pub mod tannakian_frobenius_arb;
pub mod triangular;
pub mod triangular_surface;
pub mod tropical_arbitrage;
pub mod tsrv_volatility;
pub mod unified_margin_warning;
pub mod variance_swap;
pub mod voevodsky_motives_arb;
pub mod volatility_jump;
pub mod vpin;
pub mod wardrop_frank_wolfe;
pub mod wrapped_fukaya_arb;

pub use arakelov_geometry_arb::*;
pub use atomic_mev::*;
pub use autotune::*;
pub use basis_pca::*;
pub use basis_term_structure::*;
pub use berkovich_analytic_arb::*;
pub use binomial::*;
pub mod black_scholes_reexport {
    pub use super::black_scholes::*;
}
pub use black_scholes::*;
pub use bouchaud_propagator::*;
pub use calibration::*;
pub use cointegration::*;
pub use collateral_barrier::*;
pub use collateral_dag::*;
pub use concentrated_liquidity::*;
pub use condensed_liquid_arb::*;
pub use condensed_solid_arb::*;
pub use cross_chain_intent_dag::*;
pub use cross_hawkes::*;
pub use debiased_ou::*;
pub use delta_gamma_rehedge::*;
pub use derived_artin_stack_arb::*;
pub use derived_critical_locus_arb::*;
pub use derived_deformation_arb::*;
pub use dislocation::*;
pub use dmrg_portfolio::*;
pub use donaldson_thomas_arb::*;
pub use epoch_jump::*;
pub use erc7683_intent_slashing::*;
pub use factor_stat_arb::*;
pub use fast_basis_harvest::*;
pub use fixed_yield::*;
pub use floer_novikov_arb::*;
pub use fontaine_fargues_arb::*;
pub use forecast_calibration::*;
pub use funding_carry::*;
pub use funding_epoch_countdown::*;
pub use funding_spline::*;
pub use funding_velocity::*;
pub use geometric_langlands_arb::*;
pub use grothendieck_six_operations_arb::*;
pub use gueant_mm::*;
pub use hawkes::*;
pub use hf_vecm::*;
pub use hmm_liquidity::*;
pub use htlc_settlement::*;
pub use hyperbolic_poincare_arb::*;
pub use intent_bridge::*;
pub use inventory_skew::*;
pub use kalman_filter::*;
pub use koszul_tate_poisson_arb::*;
pub use l3_hawkes_mm::*;
pub use l3_hazard::*;
pub use l3_orderbook::*;
pub use l3_tensor_ofi::*;
pub use latency_racing::*;
pub use lead_lag::*;
pub use margin_deleveraging::*;
pub mod multi_commodity_flow_reexport {
    pub use super::multi_commodity_flow::*;
}
pub use matrix_propagator::*;
pub use micro_price_surface::*;
pub use motivic_cohomology_arb::*;
pub use multi_asset_avellaneda::*;
pub use multi_asset_impact::*;
pub use multi_commodity_flow::*;
pub use neural_stopping_qubo::*;
pub use non_abelian_gauge_arb::*;
pub use noncommutative_motives_arb::*;
pub use optimal_thresholds::*;
pub use ou_process::*;
pub use padic_simpson_arb::*;
pub use pair_monitor::*;
pub use perfectoid_shimura_arb::*;
pub use perfectoid_tilting_arb::*;
pub use perverse_sheaves_arb::*;
pub use portfolio_greeks_opt::*;
pub use power_law_propagator::*;
pub use prediction_synthetic::*;
pub use private_relay::*;
pub use quantum_frobenius_arb::*;
pub use queue_drift::*;
pub use queue_priority::*;
pub use queue_survival::*;
pub use regime_switching::*;
pub use relative_prismatic_arb::*;
pub use resilience_pacer::*;
pub use rigid_analytic_tate_arb::*;
pub use risk_math::*;
pub use rolling_stats::*;
pub use rough_heston_avx::*;
pub use rough_ou::*;
pub use rough_path_signature::*;
pub use seasonality::*;
pub use session::*;
pub use spatial_spread::*;
pub use stochastic_depth::*;
pub use susy_instanton::*;
pub use svi_arbitrage_free::*;
pub use tail_copula::*;
pub use tannakian_frobenius_arb::*;
pub use triangular::*;
pub use triangular_surface::*;
pub use tropical_arbitrage::*;
pub use tsrv_volatility::*;
pub use unified_margin_warning::*;
pub use variance_swap::*;
pub use voevodsky_motives_arb::*;
pub use volatility_jump::*;
pub use vpin::*;
pub use wardrop_frank_wolfe::*;
pub use wrapped_fukaya_arb::*;

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook};
    use arbitrageur_core::types::dec;
    use chrono::Utc;

    #[test]
    fn test_welford_stats() {
        let mut stats = OnlineWelfordStats::new();
        let values = [10.0, 12.0, 8.0, 14.0, 16.0];
        for v in values {
            stats.update(v);
        }
        assert_eq!(stats.count(), 5);
        assert!((stats.mean() - 12.0).abs() < 1e-6);
        assert!((stats.variance() - 10.0).abs() < 1e-6);
        assert!((stats.std_dev() - 10.0_f64.sqrt()).abs() < 1e-6);
    }

    #[test]
    fn test_spatial_spread_evaluation() {
        let mut buy_book = OrderBook::new("venue_a".into(), "BTC-USD".into());
        buy_book.asks.push(BookLevel::new(dec!(60000), dec!(1.5)));
        buy_book.asks.push(BookLevel::new(dec!(60010), dec!(2.0)));

        let mut sell_book = OrderBook::new("venue_b".into(), "BTC-USD".into());
        sell_book.bids.push(BookLevel::new(dec!(60100), dec!(1.0)));
        sell_book.bids.push(BookLevel::new(dec!(60090), dec!(2.0)));

        let opp = evaluate_spatial_spread(
            &buy_book,
            &sell_book,
            dec!(2), // 2 bps fee
            dec!(2), // 2 bps fee
            dec!(1.0),
        )
        .expect("Opportunity should exist");

        assert!(opp.gross_spread > dec!(0));
        assert!(opp.net_profit_per_unit > dec!(0));
        assert_eq!(opp.executable_qty, dec!(1.0));
    }

    #[test]
    fn test_funding_carry_evaluation() {
        let now = Utc::now();
        let rate_a = FundingRate {
            venue: "hyperliquid".into(),
            symbol: "ETH-PERP".into(),
            rate: dec!(0.0003), // 0.03% per hour
            predicted_rate: None,
            interval_hours: 1,
            next_funding_time: now,
            timestamp: now,
        };

        let rate_b = FundingRate {
            venue: "lighter".into(),
            symbol: "ETH-PERP".into(),
            rate: dec!(0.0001), // 0.01% per 8 hours
            predicted_rate: None,
            interval_hours: 8,
            next_funding_time: now,
            timestamp: now,
        };

        let carry = evaluate_funding_carry(&rate_a, &rate_b, dec!(5), dec!(10000))
            .expect("Should find carry opportunity");

        assert_eq!(carry.short_venue, "hyperliquid");
        assert_eq!(carry.long_venue, "lighter");
        assert!(carry.net_apr > dec!(0));
        assert!(carry.daily_income_usd > dec!(0));
        // Modest rate: a finite compounded APY is expressible.
        assert!(carry.net_apy.is_some());
    }

    /// Regression (#46): extreme hourly rates must not produce a clamped
    /// "100000x" APY — the compounded figure overflows f64 and is reported
    /// as unavailable (None) while APR stays truthful.
    #[test]
    fn test_funding_carry_extreme_rate_apy_is_unavailable() {
        let now = Utc::now();
        let rate_a = FundingRate {
            venue: "hyperliquid".into(),
            symbol: "ETH-PERP".into(),
            rate: dec!(0.05), // 5% per hour — pathological
            predicted_rate: None,
            interval_hours: 1,
            next_funding_time: now,
            timestamp: now,
        };
        let rate_b = FundingRate {
            venue: "lighter".into(),
            symbol: "ETH-PERP".into(),
            rate: dec!(0.0001),
            predicted_rate: None,
            interval_hours: 8,
            next_funding_time: now,
            timestamp: now,
        };
        let carry = evaluate_funding_carry(&rate_a, &rate_b, dec!(5), dec!(10000))
            .expect("carry opportunity");
        assert!(carry.net_apr > dec!(0));
        assert_eq!(carry.net_apy, None);
    }

    #[test]
    fn test_kalman_filter_convergence() {
        let mut kf = HedgeRatioKalmanFilter::new(1e-4, 1e-2);
        // True relationship: y = 2.5 * x + 5.0
        for i in 1..=100 {
            let x = i as f64 * 0.5;
            let y = 2.5 * x + 5.0 + ((i % 3) as f64 - 1.0) * 0.05;
            kf.update(x, y);
        }
        assert!((kf.hedge_ratio() - 2.5).abs() < 0.2);
    }

    #[test]
    fn test_prediction_synthetic_arbitrage() {
        let opp = evaluate_prediction_cross_venue(
            "Polymarket",
            dec!(0.40), // YES
            dec!(0.60), // NO
            "Kalshi",
            dec!(0.48), // YES
            dec!(0.55), // NO
            dec!(10),   // 10 BPS fee
        )
        .expect("Arbitrage should exist: Polymarket YES (0.40) + Kalshi NO (0.55) = 0.95 < 1.00");

        assert_eq!(opp.buy_yes_venue, "Polymarket");
        assert_eq!(opp.buy_no_venue, "Kalshi");
        assert!(opp.net_profit > dec!(0));
    }

    #[test]
    fn test_spatial_spread_depth_asymmetry() {
        let mut buy_book = OrderBook::new("venue_a".into(), "BTC-USD".into());
        // Buy book has depth: 1.0 BTC @ 60000, 9.0 BTC @ 60500
        buy_book.asks.push(BookLevel::new(dec!(60000), dec!(1.0)));
        buy_book.asks.push(BookLevel::new(dec!(60500), dec!(9.0)));

        let mut sell_book = OrderBook::new("venue_b".into(), "BTC-USD".into());
        // Sell book only has 1.0 BTC @ 60100
        sell_book.bids.push(BookLevel::new(dec!(60100), dec!(1.0)));

        // Target 10 BTC, but only 1.0 BTC is mutually executable
        let opp = evaluate_spatial_spread(&buy_book, &sell_book, dec!(0), dec!(0), dec!(10.0))
            .expect("Opportunity should exist for 1.0 BTC");

        assert_eq!(opp.executable_qty, dec!(1.0));
        // Buy VWAP must be evaluated on the 1.0 BTC matched quantity (@ 60000), NOT on the 10.0 BTC deep quantity!
        assert_eq!(opp.gross_spread, dec!(100.0)); // 60100 - 60000
        assert_eq!(opp.total_net_profit_usd, dec!(100.0));
    }

    #[test]
    fn test_prediction_synthetic_price_bounds() {
        // Price <= 0 or >= 1.0 must be rejected
        assert!(evaluate_prediction_cross_venue(
            "A",
            dec!(-0.1),
            dec!(0.5),
            "B",
            dec!(0.4),
            dec!(0.5),
            dec!(0)
        )
        .is_none());
        assert!(evaluate_prediction_cross_venue(
            "A",
            dec!(1.0),
            dec!(0.5),
            "B",
            dec!(0.4),
            dec!(0.5),
            dec!(0)
        )
        .is_none());
        assert!(evaluate_prediction_cross_venue(
            "A",
            dec!(0.4),
            dec!(0.5),
            "B",
            dec!(0.4),
            dec!(1.5),
            dec!(0)
        )
        .is_none());
    }

    #[test]
    fn test_prediction_synthetic_separate_fees() {
        // Poly fee 0 bps, Kalshi fee 20 bps
        let opp = evaluate_prediction_cross_venue_fees(
            "Polymarket",
            dec!(0.45),
            dec!(0.55),
            dec!(0), // 0 bps
            "Kalshi",
            dec!(0.50),
            dec!(0.50),
            dec!(20), // 20 bps
        )
        .expect("Arbitrage should exist: 0.45 + 0.50 = 0.95 < 1.0");

        assert_eq!(opp.buy_yes_venue, "Polymarket");
        assert_eq!(opp.buy_no_venue, "Kalshi");
        assert!(opp.net_profit > dec!(0));
    }

    #[test]
    fn test_kalman_filter_symmetrization_and_variance() {
        let mut kf = HedgeRatioKalmanFilter::new(1e-4, 1e-2);
        for i in 1..1000 {
            let x = 100.0 + (i as f64 * 0.1);
            let y = 2.0 * x + 5.0 + ((i % 5) as f64 - 2.0) * 0.01;
            let (_, _) = kf.update(x, y);
            assert!(kf.latest_variance > 0.0);
            assert!(!kf.latest_variance.is_nan());
            assert!(
                (kf.p[0][1] - kf.p[1][0]).abs() < 1e-10,
                "Covariance matrix must stay symmetric"
            );
        }
    }

    #[test]
    fn test_expected_basis_carry_return_incorporates_convergence() {
        // Trade size 10,000 USD
        // Funding rate is modest (+1 bps per 8h)
        // Fees = 4 bps round-trip
        // Basis entry = +25 bps contango, exit target = +5 bps (20 bps compression)
        let funding_only = expected_carry_return_usd(0.0001, 4.0, None, Some(8.0), None, 10000.0)
            .expect("Valid funding calculation");

        let total_with_basis = expected_basis_carry_return_usd(
            0.0001,
            Some(25.0),
            Some(5.0),
            4.0,
            None,
            Some(8.0),
            None,
            10000.0,
        )
        .expect("Valid basis carry calculation");

        // Basis compression adds: 10,000 * 20 / 10,000 = +20.0 USD
        assert!((total_with_basis - (funding_only + 20.0)).abs() < 1e-6);
        assert!(total_with_basis > funding_only);
    }

    #[test]
    fn test_lead_lag_and_margin_deleveraging_integration() {
        let est = HayashiYoshidaEstimator::new(50, 5);
        let res = est.evaluate_lead_lag(&[], &[]);
        assert_eq!(res.leader, LeadVenue::Synchronous);

        let ctrl = ProportionalDeleveragingController::default();
        assert_eq!(ctrl.target_mbr, 0.35);

        let epoch = FundingEpochPredictor::for_8h_epoch();
        assert_eq!(epoch.epoch_duration_secs, 28800.0);
    }

    #[test]
    fn test_tranche_65_integration() {
        // 1. Rough Path Signature Engine
        let mut sig_engine = RoughPathSignatureEngine::new();
        sig_engine.update(RoughPathPoint::new(0.0, 10.0, 0.0, 0.50));
        sig_engine.update(RoughPathPoint::new(1.0, 10.05, 0.20, 0.55));
        let weights = [0.005; 84];
        let iv = sig_engine.evaluate_linear_functional(0.50, &weights);
        assert!(iv > 0.50 && iv.is_finite());
        assert_eq!(sig_engine.current_signature.to_flat_vector().len(), 84);

        // 2. L3 Hawkes Jump-Diffusion Market Maker
        let mm = L3HawkesMarketMaker::new(L3HawkesMmConfig::default());
        let quotes = mm.compute_quotes(3000.0, 2.0, 15.0, 10.0, 8.0, 0.2, 0.15, 10.0, 50.0);
        assert!(quotes.recommended_bid > 0.0);
        assert!(quotes.recommended_ask > quotes.recommended_bid);
        assert!(quotes.queue_gating_passed);

        // 3. Cross-Chain Intent DAG Netting & Concave Dutch Decay
        let mut intent_dag = CrossChainIntentDag::new();
        intent_dag.add_edge(IntentDebtEdge {
            edge_id: "edge-1".into(),
            from_venue: "Arbitrum".into(),
            to_venue: "Base".into(),
            asset: "USDC".into(),
            amount_usd: 100_000.0,
            unit_fee_bps: 3.5,
            latency_sec: 12.0,
        });
        intent_dag.add_edge(IntentDebtEdge {
            edge_id: "edge-2".into(),
            from_venue: "Base".into(),
            to_venue: "Arbitrum".into(),
            asset: "USDC".into(),
            amount_usd: 100_000.0,
            unit_fee_bps: 3.5,
            latency_sec: 12.0,
        });
        let netting = intent_dag.cancel_netting_cycles();
        assert_eq!(netting.len(), 1);
        assert_eq!(netting[0].netted_volume_usd, 200_000.0);
        assert_eq!(netting[0].volume_reduction_pct, 100.0);

        let decay = CrossChainIntentDag::compute_optimal_dutch_decay(0.60, 6.0, 2.0, 1.0, 0.05);
        assert!(decay.gamma_decay >= 0.55 && decay.gamma_decay <= 0.65);
    }

    #[test]
    fn test_tranche_66_integration() {
        // 1. Neural Double Stopping & QUBO Discrete Lot Allocation
        let stopping_eval = NeuralDoubleStoppingEvaluator::default();
        let boundaries = stopping_eval.compute_boundaries(0.15, 12.0, 0.0, 4.0, 4.0);
        assert!(boundaries.entry_z > 0.0);
        assert!(boundaries.exit_z < boundaries.entry_z);

        let candidates = vec![
            QuboAssetCandidate {
                symbol: "BTC".into(),
                target_continuous_qty: 0.0025,
                lot_step_size: 0.001,
                asset_price_usd: 65_000.0,
                min_notional_usd: 10.0,
                fee_rate: 0.0002,
            },
            QuboAssetCandidate {
                symbol: "SOL".into(),
                target_continuous_qty: 0.01, // 0.01 * 150 = $1.50 < $10.0 min notional
                lot_step_size: 0.1,
                asset_price_usd: 150.0,
                min_notional_usd: 10.0,
                fee_rate: 0.0002,
            },
        ];
        let allocated = QuboLotRebalancer::solve_discrete_allocation(&candidates, &[1.0, 1.0]);
        assert_eq!(allocated[0], 0.003); // Rounded to 3 lots ($195 > $10)
        assert_eq!(allocated[1], 0.0); // Dropped because $1.50 < $10.0 dust floor

        // 2. Arbitrage-Free SVI Durrleman Condition & Smile Greeks
        let svi = SviSliceParameters {
            a: 0.04,
            b: 0.15,
            rho: -0.4,
            m: 0.0,
            sigma: 0.1,
        };
        assert!(svi.is_arbitrage_free());

        let spot = 100.0;
        let strike = 95.0;
        let tau = 30.0 / 365.0;
        let delta_skew = svi.smile_adjusted_delta(0.65, 20.0, spot, strike, tau);
        assert!(delta_skew > 0.65); // Skew adjustment adds put downside protection
        let gamma_skew = svi.smile_adjusted_gamma(0.015, 20.0, spot, strike, tau);
        assert!(gamma_skew.is_finite());
    }
}
