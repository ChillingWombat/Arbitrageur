use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::order::OrderRequest;
use arbitrageur_core::types::{dec, Decimal, Side};
use arbitrageur_execution::coordinator::{ArbitrageExecutionResult, AtomicArbitrageCoordinator};
use arbitrageur_math::funding_carry::{evaluate_funding_carry, FundingCarryOpportunity};
use std::sync::Arc;
use tracing::info;

pub struct FundingRateArbStrategy {
    pub symbol: String,
    pub exchange_a: Arc<dyn ExchangeAdapter>,
    pub exchange_b: Arc<dyn ExchangeAdapter>,
    pub min_funding_diff_8h: Decimal,
    pub max_breakeven_hours: Decimal,
    pub trade_size_usd: Decimal,
    pub round_trip_fee_bps: Decimal,
    pub max_entry_spread_bps: Option<Decimal>,
    pub regime_detector:
        Option<Arc<parking_lot::Mutex<arbitrageur_math::regime_switching::CusumChangeDetector>>>,
    pub kalman_filter: Option<
        Arc<parking_lot::Mutex<arbitrageur_math::kalman_filter::FundingPremiumKalmanFilter>>,
    >,
    pub epoch_predictor: Option<arbitrageur_math::funding_epoch_countdown::FundingEpochPredictor>,
    pub deleveraging_controller:
        Option<arbitrageur_math::margin_deleveraging::ProportionalDeleveragingController>,
    pub fast_basis_harvester:
        Option<arbitrageur_math::fast_basis_harvest::FastBasisHarvestEvaluator>,
    pub leung_li_router: Option<arbitrageur_math::fast_basis_harvest::LeungLiBasisStoppingRouter>,
    pub vecm_analyzer: Option<arbitrageur_math::fast_basis_harvest::VecmBasisAnalyzer>,
    pub last_kalman_update: Arc<parking_lot::Mutex<Option<std::time::Instant>>>,
    pub last_cusum_update: Arc<parking_lot::Mutex<Option<std::time::Instant>>>,
    pub max_allowed_liquidation_prob_14d: Option<f64>,
    pub term_structure_curve: Option<arbitrageur_math::basis_term_structure::NelsonSiegelCurve>,
    pub funding_spline: Option<arbitrageur_math::funding_spline::FritschCarlsonMonotoneSpline>,
    pub dynamic_fast_basis_harvest_engine:
        Option<arbitrageur_math::funding_spline::DynamicFastBasisHarvestEngine>,
    pub funding_velocity_tracker: Option<
        Arc<parking_lot::Mutex<arbitrageur_math::funding_velocity::FundingVelocityKalmanHawkes>>,
    >,
    pub pre_epoch_optimizer: Option<arbitrageur_math::epoch_jump::PreEpochEntryOptimizer>,
    pub fast_basis_reversal_guard: Option<
        Arc<parking_lot::Mutex<arbitrageur_math::epoch_jump::FastBasisHarvestReversalGuard>>,
    >,
    pub cross_venue_deleveraging_controller:
        Option<arbitrageur_math::margin_deleveraging::CrossVenueMarginDeleveragingController>,
}

/// Actionable opportunity on the basis term structure curve via duration/slope-matched butterfly.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TermStructureButterflyOpportunity {
    pub symbol: String,
    pub m1: f64,
    pub m2: f64,
    pub m3: f64,
    pub weights: [f64; 3],
    pub curvature_beta2: f64,
    pub roll_down_yield_bps: f64,
    pub is_actionable: bool,
    pub action: String,
}

impl FundingRateArbStrategy {
    pub fn new(
        symbol: impl Into<String>,
        exchange_a: Arc<dyn ExchangeAdapter>,
        exchange_b: Arc<dyn ExchangeAdapter>,
        min_funding_diff_8h: Decimal,
        max_breakeven_hours: Decimal,
        trade_size_usd: Decimal,
        round_trip_fee_bps: Decimal,
    ) -> Self {
        Self {
            symbol: symbol.into(),
            exchange_a,
            exchange_b,
            min_funding_diff_8h,
            max_breakeven_hours,
            trade_size_usd,
            round_trip_fee_bps,
            max_entry_spread_bps: None,
            regime_detector: None,
            kalman_filter: None,
            epoch_predictor: None,
            deleveraging_controller: None,
            fast_basis_harvester: None,
            leung_li_router: None,
            vecm_analyzer: None,
            last_kalman_update: Arc::new(parking_lot::Mutex::new(None)),
            last_cusum_update: Arc::new(parking_lot::Mutex::new(None)),
            max_allowed_liquidation_prob_14d: None,
            term_structure_curve: None,
            funding_spline: None,
            dynamic_fast_basis_harvest_engine: None,
            funding_velocity_tracker: None,
            pre_epoch_optimizer: None,
            fast_basis_reversal_guard: None,
            cross_venue_deleveraging_controller: None,
        }
    }

    /// Attaches a pre-epoch jump-diffusion harvest timing optimizer.
    pub fn with_pre_epoch_optimizer(
        mut self,
        optimizer: arbitrageur_math::epoch_jump::PreEpochEntryOptimizer,
    ) -> Self {
        self.pre_epoch_optimizer = Some(optimizer);
        self
    }

    /// Attaches a post-settlement fast basis reversal guard.
    pub fn with_fast_basis_reversal_guard(
        mut self,
        guard: arbitrageur_math::epoch_jump::FastBasisHarvestReversalGuard,
    ) -> Self {
        self.fast_basis_reversal_guard = Some(Arc::new(parking_lot::Mutex::new(guard)));
        self
    }

    /// Attaches an online 3D Kinematic Kalman filter & Hawkes jump shock tracker for funding rate velocity forecasting.
    pub fn with_funding_velocity_tracker(
        mut self,
        tracker: arbitrageur_math::funding_velocity::FundingVelocityKalmanHawkes,
    ) -> Self {
        self.funding_velocity_tracker = Some(Arc::new(parking_lot::Mutex::new(tracker)));
        self
    }

    /// Attaches a maximum allowed 14-day liquidation probability hurdle.
    pub fn with_max_allowed_liquidation_prob_14d(mut self, max_prob: f64) -> Self {
        self.max_allowed_liquidation_prob_14d = Some(max_prob);
        self
    }

    /// Attaches a maximum entry cross spread hurdle (in bps) to reject trades where the cross-venue price gap exceeds allowable friction.
    pub fn with_max_entry_spread_bps(mut self, max_bps: Decimal) -> Self {
        self.max_entry_spread_bps = Some(max_bps);
        self
    }

    /// Attaches a CUSUM change detector to guard against entering before or during a regime collapse.
    pub fn with_regime_detector(
        mut self,
        detector: arbitrageur_math::regime_switching::CusumChangeDetector,
    ) -> Self {
        self.regime_detector = Some(Arc::new(parking_lot::Mutex::new(detector)));
        self
    }

    /// Attaches an online Kalman Filter to track structural funding rate equilibrium and forecast inversions.
    pub fn with_kalman_filter(
        mut self,
        kf: arbitrageur_math::kalman_filter::FundingPremiumKalmanFilter,
    ) -> Self {
        self.kalman_filter = Some(Arc::new(parking_lot::Mutex::new(kf)));
        self
    }

    /// Attaches a pre-settlement funding epoch countdown predictor for TWAP variance collapse modeling.
    pub fn with_epoch_predictor(
        mut self,
        pred: arbitrageur_math::funding_epoch_countdown::FundingEpochPredictor,
    ) -> Self {
        self.epoch_predictor = Some(pred);
        self
    }

    /// Attaches a proportional deleveraging controller for segregated margin safety.
    pub fn with_deleveraging_controller(
        mut self,
        ctrl: arbitrageur_math::margin_deleveraging::ProportionalDeleveragingController,
    ) -> Self {
        self.deleveraging_controller = Some(ctrl);
        self
    }

    /// Attaches a fast basis harvest evaluator for early profit locking upon basis compression.
    pub fn with_fast_basis_harvester(
        mut self,
        harvester: arbitrageur_math::fast_basis_harvest::FastBasisHarvestEvaluator,
    ) -> Self {
        self.fast_basis_harvester = Some(harvester);
        self
    }

    /// Attaches a Leung-Li double optimal stopping router for dynamic basis exit boundary computation.
    pub fn with_leung_li_router(
        mut self,
        router: arbitrageur_math::fast_basis_harvest::LeungLiBasisStoppingRouter,
    ) -> Self {
        self.leung_li_router = Some(router);
        self
    }

    /// Attaches a VECM basis price discovery analyzer for Spot, Perp CEX, and Perp DEX lead-lag decomposition.
    pub fn with_vecm_analyzer(
        mut self,
        analyzer: arbitrageur_math::fast_basis_harvest::VecmBasisAnalyzer,
    ) -> Self {
        self.vecm_analyzer = Some(analyzer);
        self
    }

    /// Attaches a calibrated Nelson-Siegel 3-factor basis term structure curve.
    pub fn with_term_structure_curve(
        mut self,
        curve: arbitrageur_math::basis_term_structure::NelsonSiegelCurve,
    ) -> Self {
        self.term_structure_curve = Some(curve);
        self
    }

    /// Attaches a Fritsch-Carlson monotone cubic spline for continuous cross-epoch funding interpolation.
    pub fn with_funding_spline(
        mut self,
        spline: arbitrageur_math::funding_spline::FritschCarlsonMonotoneSpline,
    ) -> Self {
        self.funding_spline = Some(spline);
        self
    }

    /// Attaches a dynamic fast basis harvest engine for early capital recycling upon rapid basis compression.
    pub fn with_dynamic_fast_basis_harvest_engine(
        mut self,
        engine: arbitrageur_math::funding_spline::DynamicFastBasisHarvestEngine,
    ) -> Self {
        self.dynamic_fast_basis_harvest_engine = Some(engine);
        self
    }

    /// Evaluates calendar basis butterfly spread opportunity matching level and slope duration to extract pure curvature roll-down.
    pub fn evaluate_term_structure_butterfly(
        &self,
        m1: f64,
        m2: f64,
        m3: f64,
        min_curvature_hurdle: f64,
    ) -> Option<TermStructureButterflyOpportunity> {
        let curve = self.term_structure_curve.as_ref()?;
        let weights =
            arbitrageur_math::basis_term_structure::BasisButterflyBuilder::build_butterfly(
                curve, m1, m2, m3,
            )?;

        let roll_down_bps = curve.roll_down_rate(m2) * 10000.0;
        let is_actionable = curve.beta2.abs() >= min_curvature_hurdle.abs();

        let action = if curve.beta2 > min_curvature_hurdle {
            format!(
                "Sell rich body at m={:.2}yr, buy wings at m={:.2}yr & m={:.2}yr to capture {:.1} bps roll-down",
                m2, m1, m3, roll_down_bps
            )
        } else if curve.beta2 < -min_curvature_hurdle {
            format!(
                "Buy cheap body at m={:.2}yr, sell wings at m={:.2}yr & m={:.2}yr to capture inverted curvature",
                m2, m1, m3
            )
        } else {
            "Hold neutral: curvature within no-arbitrage band".to_string()
        };

        Some(TermStructureButterflyOpportunity {
            symbol: self.symbol.clone(),
            m1,
            m2,
            m3,
            weights,
            curvature_beta2: curve.beta2,
            roll_down_yield_bps: roll_down_bps,
            is_actionable,
            action,
        })
    }

    /// Evaluates dynamic basis exit boundary under Leung-Li (2015) double optimal stopping:
    /// Returns TakeProfitConvergence, StopLossRoll, or HoldCarryHarvest.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_leung_li_exit(
        &self,
        current_basis_bps: f64,
        unwind_friction_bps: f64,
        theta_basis_bps: f64,
        sigma_basis_bps: f64,
        kappa_reversion: f64,
        discount_rate_rho: f64,
        net_carry_yield_annual: f64,
    ) -> Option<arbitrageur_math::fast_basis_harvest::BasisExitDecision> {
        self.leung_li_router.as_ref().map(|router| {
            router.evaluate_basis_exit(
                current_basis_bps,
                unwind_friction_bps,
                theta_basis_bps,
                sigma_basis_bps,
                kappa_reversion,
                discount_rate_rho,
                net_carry_yield_annual,
            )
        })
    }

    /// Evaluates lead-lag price discovery and cointegrated Information Shares across Spot, Perp CEX, and Perp DEX.
    pub fn analyze_vecm_price_discovery(
        &self,
        spot_prices: &[f64],
        perp_cex_prices: &[f64],
        perp_dex_prices: &[f64],
    ) -> Option<arbitrageur_math::fast_basis_harvest::PriceDiscoveryReport> {
        self.vecm_analyzer.as_ref().and_then(|analyzer| {
            analyzer.analyze_price_discovery(spot_prices, perp_cex_prices, perp_dex_prices)
        })
    }

    /// Evaluates segregated margin buffer and determines whether proportional deleveraging is required.
    pub fn evaluate_deleveraging(
        &self,
        long_leg: &arbitrageur_math::margin_deleveraging::LegMarginState,
        short_leg: &arbitrageur_math::margin_deleveraging::LegMarginState,
    ) -> Option<arbitrageur_math::margin_deleveraging::MarginDeleveragingDecision> {
        self.deleveraging_controller
            .as_ref()
            .map(|ctrl| ctrl.evaluate_pair_margin(long_leg, short_leg))
    }

    /// Attaches a dynamic cross-venue margin deleveraging and water-filling rebalance controller.
    pub fn with_cross_venue_deleveraging_controller(
        mut self,
        ctrl: arbitrageur_math::margin_deleveraging::CrossVenueMarginDeleveragingController,
    ) -> Self {
        self.cross_venue_deleveraging_controller = Some(ctrl);
        self
    }

    /// Evaluates multi-account segregated cross-venue margin state and determines
    /// whether an automated water-filling DAG transfer should be dispatched before emergency trimming.
    pub fn evaluate_cross_venue_margin_rebalancing(
        &self,
        account_a: &arbitrageur_math::margin_deleveraging::VenueAccountState,
        account_b: &arbitrageur_math::margin_deleveraging::VenueAccountState,
        transfer_latency_hours: f64,
        transfer_fee_usd: f64,
    ) -> Option<arbitrageur_math::margin_deleveraging::CrossVenueRebalanceAction> {
        self.cross_venue_deleveraging_controller
            .as_ref()
            .map(|ctrl| {
                ctrl.evaluate_pair_rebalancing(
                    account_a,
                    account_b,
                    transfer_latency_hours,
                    transfer_fee_usd,
                )
            })
    }

    /// Evaluates whether an open carry trade should be harvested early due to rapid basis compression.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_fast_basis_harvest(
        &self,
        entry_basis_bps: f64,
        current_basis_bps: f64,
        elapsed_hours: f64,
        total_epoch_hours: f64,
        accrued_funding_usd: f64,
        unrealized_basis_pnl_usd: f64,
        round_trip_fee_usd: f64,
    ) -> Option<bool> {
        self.fast_basis_harvester.as_ref().map(|h| {
            h.should_harvest_early(
                entry_basis_bps,
                current_basis_bps,
                elapsed_hours,
                total_epoch_hours,
                accrued_funding_usd,
                unrealized_basis_pnl_usd,
                round_trip_fee_usd,
            )
        })
    }

    /// Evaluates dynamic fast basis harvest opportunity using continuous monotone spline and capital velocity IRR hurdles.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_dynamic_fast_basis_harvest(
        &self,
        entry_basis_bps: f64,
        current_basis_bps: f64,
        elapsed_hours: f64,
        total_epoch_hours: f64,
        accrued_funding_bps: f64,
        round_trip_fee_bps: f64,
        passive_target_pnl_bps: f64,
        notional_usd: f64,
    ) -> Option<arbitrageur_math::funding_spline::FastHarvestDecision> {
        self.dynamic_fast_basis_harvest_engine
            .as_ref()
            .map(|engine| {
                engine.evaluate_harvest(
                    entry_basis_bps,
                    current_basis_bps,
                    elapsed_hours,
                    total_epoch_hours,
                    accrued_funding_bps,
                    round_trip_fee_bps,
                    passive_target_pnl_bps,
                    notional_usd,
                )
            })
    }

    /// Evaluates predictive funding carry harvesting using the continuous kinematic Kalman filter and Hawkes shock detector.
    pub fn evaluate_predictive_carry_harvest(
        &self,
        tau_rem_sec: f64,
        round_trip_cost_bps: f64,
        annualized_rate_vol: f64,
        current_time_sec: f64,
    ) -> Option<arbitrageur_math::funding_velocity::CarryHarvestDecision> {
        self.funding_velocity_tracker.as_ref().map(|tracker| {
            let guard = tracker.lock();
            arbitrageur_math::funding_velocity::PredictiveCarryYieldHarvester::evaluate(
                &guard,
                tau_rem_sec,
                round_trip_cost_bps,
                annualized_rate_vol,
                current_time_sec,
            )
        })
    }

    /// Evaluates pre-epoch jump-diffusion harvest timing and execution window.
    pub fn evaluate_pre_epoch_jump_timing(
        &self,
        seconds_to_epoch: f64,
        spot_price: f64,
        predicted_settled_rate: f64,
        position_notional_usd: f64,
        roundtrip_fees_usd: f64,
    ) -> Option<arbitrageur_math::epoch_jump::PreEpochEntryVerdict> {
        self.pre_epoch_optimizer.as_ref().map(|opt| {
            opt.evaluate_entry_window(
                seconds_to_epoch,
                spot_price,
                predicted_settled_rate,
                position_notional_usd,
                roundtrip_fees_usd,
            )
        })
    }

    /// Evaluates post-settlement basis reversal guard to prevent snapback from degrading dividend.
    pub fn evaluate_fast_basis_reversal_guard(
        &self,
        current_time_ms: i64,
        current_basis_usd: f64,
        spot_price: f64,
    ) -> Option<arbitrageur_math::epoch_jump::FastBasisReversalAction> {
        self.fast_basis_reversal_guard.as_ref().map(|guard| {
            let mut g = guard.lock();
            g.evaluate_reversal_guard(current_time_ms, current_basis_usd, spot_price)
        })
    }

    /// Scan funding rates on both exchanges and evaluate carry profitability.
    pub async fn scan_opportunity(&self) -> anyhow::Result<Option<FundingCarryOpportunity>> {
        let (rate_a, rate_b) = tokio::try_join!(
            self.exchange_a.fetch_funding_rate(&self.symbol),
            self.exchange_b.fetch_funding_rate(&self.symbol)
        )?;

        if let Some(opp) = evaluate_funding_carry(
            &rate_a,
            &rate_b,
            self.round_trip_fee_bps,
            self.trade_size_usd,
        ) {
            // Guard against entering into an imminent funding rate inversion / collapse
            if let Some(detector_lock) = &self.regime_detector {
                let rate_f64 = opp.net_rate_8h.to_string().parse::<f64>().unwrap_or(0.0);
                let now = std::time::Instant::now();
                let dt_8h = {
                    let mut last = self.last_cusum_update.lock();
                    let dt = if let Some(t) = *last {
                        (now - t).as_secs_f64() / (8.0 * 3600.0)
                    } else {
                        // First tick warmup duration: 1/8th of an 8h cycle (1 hour equivalent)
                        0.125
                    };
                    *last = Some(now);
                    dt.clamp(0.0001, 3.0)
                };
                let is_collapse = detector_lock.lock().update_scaled(rate_f64, dt_8h);
                if is_collapse {
                    tracing::warn!(
                        "[{}] Funding carry candidate blocked: CUSUM detected downward regime shift / sign-flip",
                        self.symbol
                    );
                    return Ok(None);
                }
            }

            // Guard against structural funding inversion or transient flash spike via Kalman Filter
            if let Some(kf_lock) = &self.kalman_filter {
                let rate_bps = (opp.net_rate_8h * dec!(10000))
                    .to_string()
                    .parse::<f64>()
                    .unwrap_or(0.0);
                let now = std::time::Instant::now();
                let dt_hours = {
                    let mut last = self.last_kalman_update.lock();
                    let dt = if let Some(t) = *last {
                        (now - t).as_secs_f64() / 3600.0
                    } else {
                        1.0
                    };
                    *last = Some(now);
                    dt.clamp(0.001, 24.0)
                };
                let (structural_bps, drift) = kf_lock.lock().update(rate_bps, dt_hours);
                if kf_lock.lock().is_predicted_to_invert(8.0) {
                    tracing::warn!(
                        "[{}] Funding carry candidate blocked: Kalman filter forecasts structural inversion (premium {:.2} bps, drift {:.2} bps/h)",
                        self.symbol, structural_bps, drift
                    );
                    return Ok(None);
                }
                if kf_lock.lock().z_score() > 2.5 {
                    tracing::warn!(
                        "[{}] Funding carry candidate blocked: unsustainable flash premium spike (Z-score {:.2} > 2.5)",
                        self.symbol, kf_lock.lock().z_score()
                    );
                    return Ok(None);
                }
            }

            // Guard against funding velocity degradation or Hawkes liquidation shock
            if let Some(vt_lock) = &self.funding_velocity_tracker {
                let net_rate_f64 = opp.net_rate_8h.to_string().parse::<f64>().unwrap_or(0.0);
                let rt_fee_f64 = self
                    .round_trip_fee_bps
                    .to_string()
                    .parse::<f64>()
                    .unwrap_or(2.0);
                let now_sec = chrono::Utc::now().timestamp() as f64;
                let decision = {
                    let mut tracker = vt_lock.lock();
                    tracker.update(net_rate_f64, 10.0, now_sec);
                    arbitrageur_math::funding_velocity::PredictiveCarryYieldHarvester::evaluate(
                        &tracker, 900.0, // 15-minute countdown horizon
                        rt_fee_f64, 0.05, now_sec,
                    )
                };

                if !decision.should_enter {
                    tracing::warn!(
                        "[{}] Funding carry candidate blocked by kinematic velocity harvester: net_edge={:.2} bps, shock_intensity={:.2}",
                        self.symbol, decision.net_edge_bps, decision.shock_intensity
                    );
                    return Ok(None);
                }
            }

            if opp.net_rate_8h >= self.min_funding_diff_8h
                && opp.break_even_hours <= self.max_breakeven_hours
            {
                return Ok(Some(opp));
            }
        }

        Ok(None)
    }

    /// Open delta-neutral funding carry position atomically.
    pub async fn execute_open_carry(
        &self,
        opp: &FundingCarryOpportunity,
        coordinator: &AtomicArbitrageCoordinator,
    ) -> anyhow::Result<ArbitrageExecutionResult> {
        self.execute_open_carry_staggered(opp, coordinator, None)
            .await
    }

    /// Open delta-neutral funding carry position atomically with latency staggering computed from venue RTTs.
    pub async fn execute_open_carry_with_rtt(
        &self,
        opp: &FundingCarryOpportunity,
        coordinator: &AtomicArbitrageCoordinator,
        rtt_long_ms: Option<f64>,
        rtt_short_ms: Option<f64>,
    ) -> anyhow::Result<ArbitrageExecutionResult> {
        let stagger = Some(
            arbitrageur_execution::coordinator::LatencyStagger::from_rtt_ms_spatial(
                rtt_long_ms,
                rtt_short_ms,
            ),
        );
        self.execute_open_carry_staggered(opp, coordinator, stagger)
            .await
    }

    /// Open delta-neutral funding carry position with real-time latency staggering.
    pub async fn execute_open_carry_staggered(
        &self,
        opp: &FundingCarryOpportunity,
        coordinator: &AtomicArbitrageCoordinator,
        stagger: Option<arbitrageur_execution::coordinator::LatencyStagger>,
    ) -> anyhow::Result<ArbitrageExecutionResult> {
        let is_a_long = self
            .exchange_a
            .venue_id()
            .as_str()
            .eq_ignore_ascii_case(&opp.long_venue)
            || self.exchange_a.name().eq_ignore_ascii_case(&opp.long_venue);
        let is_b_long = self
            .exchange_b
            .venue_id()
            .as_str()
            .eq_ignore_ascii_case(&opp.long_venue)
            || self.exchange_b.name().eq_ignore_ascii_case(&opp.long_venue);

        let (long_ex, short_ex) = if is_a_long {
            (&self.exchange_a, &self.exchange_b)
        } else if is_b_long {
            (&self.exchange_b, &self.exchange_a)
        } else {
            anyhow::bail!(
                "carry opportunity long_venue '{}' matches neither exchange_a ('{}') nor exchange_b ('{}')",
                opp.long_venue,
                self.exchange_a.venue_id().as_str(),
                self.exchange_b.venue_id().as_str()
            );
        };

        let is_short_matched = short_ex
            .venue_id()
            .as_str()
            .eq_ignore_ascii_case(&opp.short_venue)
            || short_ex.name().eq_ignore_ascii_case(&opp.short_venue);
        if !is_short_matched {
            anyhow::bail!(
                "carry opportunity short_venue '{}' does not match resolved short exchange '{}'",
                opp.short_venue,
                short_ex.venue_id().as_str()
            );
        }

        // Position guard: a carry already open on this symbol must not be
        // stacked — the scanner fires every poll while funding persists.
        // Fail closed: unverifiable venue state refuses the entry.
        let (acc_long, acc_short) =
            tokio::try_join!(long_ex.fetch_account(), short_ex.fetch_account())?;
        for (ex, acc) in [(long_ex, &acc_long), (short_ex, &acc_short)] {
            if let Some(pos) = ex.account_position_for(acc, &self.symbol)? {
                anyhow::bail!(
                    "carry already open on {} for {} (qty {}); refusing to stack",
                    ex.venue_id().as_str(),
                    self.symbol,
                    pos.quantity
                );
            }
        }

        // Determine prices to size order and build protective slippage-bounded IOC orders
        let (book_long, book_short) = tokio::try_join!(
            long_ex.fetch_orderbook(&self.symbol),
            short_ex.fetch_orderbook(&self.symbol)
        )?;
        let ask_long = book_long.best_ask_price().unwrap_or(dec!(0));
        let bid_short = book_short.best_bid_price().unwrap_or(dec!(0));
        if ask_long <= dec!(0) || bid_short <= dec!(0) {
            anyhow::bail!("Invalid orderbook price for symbol {}", self.symbol);
        }

        if let Some(max_spread) = self.max_entry_spread_bps {
            let mid = (ask_long + bid_short) / dec!(2);
            if mid > dec!(0) && ask_long > bid_short {
                let spread_bps = ((ask_long - bid_short) / mid) * dec!(10000);
                if spread_bps > max_spread {
                    anyhow::bail!(
                        "funding entry spread {:.2} bps on {} exceeds maximum allowed {:.2} bps",
                        spread_bps,
                        self.symbol,
                        max_spread
                    );
                }
            }
        }

        let raw_qty = self.trade_size_usd / ask_long;
        let qty = arbitrageur_execution::coordinator::common_wire_qty(
            &self.symbol,
            raw_qty,
            long_ex,
            short_ex,
        )
        .await?;

        let worst_long_px = book_long
            .buy_execution_marginal_price(qty)
            .unwrap_or(ask_long);
        let worst_short_px = book_short
            .sell_execution_marginal_price(qty)
            .unwrap_or(bid_short);

        let max_long_px = coordinator
            .router()
            .calculate_bounded_price(Side::Buy, ask_long);
        let min_short_px = coordinator
            .router()
            .calculate_bounded_price(Side::Sell, bid_short);

        let long_limit = worst_long_px.min(max_long_px);
        let short_limit = worst_short_px.max(min_short_px);

        let order_long =
            OrderRequest::new_ioc(long_ex.venue_id(), &self.symbol, Side::Buy, long_limit, qty);
        let order_short = OrderRequest::new_ioc(
            short_ex.venue_id(),
            &self.symbol,
            Side::Sell,
            short_limit,
            qty,
        );

        info!(
            "💰 Opening Funding Carry: Long {} on {} and Short on {}. Expected 8h Rate: {:.4}%, APR: {:.2}%",
            self.symbol, opp.long_venue, opp.short_venue, opp.net_rate_8h * dec!(100), opp.net_apr * dec!(100)
        );

        let res = coordinator
            .execute_two_leg_staggered(long_ex, order_long, short_ex, order_short, None, stagger)
            .await;
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrageur_exchanges::simulated::SimulatedExchange;
    use arbitrageur_math::funding_velocity::FundingVelocityKalmanHawkes;

    #[test]
    fn test_funding_rate_arb_kinematic_velocity_tracker() {
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

        let tracker = FundingVelocityKalmanHawkes::new(0.0008, 1e-7, 1e-10, 0.1, 0.8, 1.5);
        let strat = FundingRateArbStrategy::new(
            "ETH-USDC",
            ex_a,
            ex_b,
            dec!(0.0005),
            dec!(12.0),
            dec!(1000.0),
            dec!(4.0),
        )
        .with_funding_velocity_tracker(tracker);

        let decision = strat
            .evaluate_predictive_carry_harvest(900.0, 2.0, 0.05, 100.0)
            .expect("decision");

        assert!(decision.projected_accrued_bps > 0.0);
        assert!(decision.required_hurdle_bps >= 4.0);
    }

    #[test]
    fn test_funding_rate_arb_cross_venue_margin_rebalancing() {
        let ex_a = Arc::new(SimulatedExchange::new(
            "binance",
            "Binance",
            0,
            dec!(2.0),
            dec!(10000.0),
        ));
        let ex_b = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(10000.0),
        ));

        let ctrl =
            arbitrageur_math::margin_deleveraging::CrossVenueMarginDeleveragingController::new(
                0.65, 0.90,
            );

        let strat = FundingRateArbStrategy::new(
            "BTC-USDT",
            ex_a,
            ex_b,
            dec!(0.0005),
            dec!(12.0),
            dec!(1000.0),
            dec!(4.0),
        )
        .with_cross_venue_deleveraging_controller(ctrl);

        let acc_a = arbitrageur_math::margin_deleveraging::VenueAccountState {
            venue_id: "binance".into(),
            total_equity_usd: 50_000.0,
            maintenance_margin_required_usd: 5_000.0, // 10%
            initial_margin_required_usd: 10_000.0,
            total_notional_usd: 100_000.0,
            hourly_equity_volatility: 0.01,
            is_dex_perp: false,
        };

        let acc_b = arbitrageur_math::margin_deleveraging::VenueAccountState {
            venue_id: "hyperliquid".into(),
            total_equity_usd: 8_000.0,
            maintenance_margin_required_usd: 6_000.0, // 75% (stressed > 65% target)
            initial_margin_required_usd: 12_000.0,
            total_notional_usd: 120_000.0,
            hourly_equity_volatility: 0.01,
            is_dex_perp: true,
        };

        let action = strat
            .evaluate_cross_venue_margin_rebalancing(&acc_a, &acc_b, 0.005, 2.0)
            .expect("rebalance action");

        match action {
            arbitrageur_math::margin_deleveraging::CrossVenueRebalanceAction::DispatchWaterFillingTransfer {
                transfer,
                ..
            } => {
                assert_eq!(transfer.from_venue, "binance");
                assert_eq!(transfer.to_venue, "hyperliquid");
                assert!(transfer.transfer_amount_usd > 10_000.0);
            }
            _ => panic!("Expected DispatchWaterFillingTransfer"),
        }
    }
}
