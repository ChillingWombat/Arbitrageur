use anyhow::bail;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::types::{dec, Decimal, Side};
use arbitrageur_math::kalman_filter::HedgeRatioKalmanFilter;
use arbitrageur_math::rolling_stats::RollingWindowStats;
use rust_decimal::prelude::ToPrimitive;
use std::sync::Arc;

/// Signal emitted by the stat-arb pair strategy. Entry legs open the
/// spread; exit legs close it with the opposite sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpreadSignal {
    /// Open a spread position: (side_y, side_x, qty_y, qty_x).
    Enter {
        side_y: Side,
        side_x: Side,
        qty_y: Decimal,
        qty_x: Decimal,
    },
    /// Close the open spread position — opposite sides of the entry.
    Exit {
        side_y: Side,
        side_x: Side,
        qty_y: Decimal,
        qty_x: Decimal,
    },
    /// Dynamic delta rebalance: adjust hedge leg X to match evolving beta_t
    RebalanceHedge {
        side_x: Side,
        delta_qty_x: Decimal,
        new_total_qty_x: Decimal,
    },
}

/// Compatibility alias for consumers that refer to the emitted spread action
/// as a pair signal.
pub type PairSignal = SpreadSignal;
/// Compatibility alias for the strategy's fallible observation path.
pub type ArbitrageError = anyhow::Error;

/// Hedge-leg side for a Y position under spread `Y - beta * X`.
/// The sign of beta is economically meaningful: a negative ratio flips the
/// hedge direction instead of merely changing its magnitude.
fn hedge_side_for_y(side_y: Side, beta: Decimal) -> Side {
    match (side_y, beta >= dec!(0)) {
        (Side::Buy, true) | (Side::Sell, false) => Side::Sell,
        (Side::Buy, false) | (Side::Sell, true) => Side::Buy,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpreadPosition {
    Flat,
    /// Entered at z <= -entry: long Y, hedged X.
    Long,
    /// Entered at z >= +entry: short Y, hedged X.
    Short,
}

/// Frozen snapshot of Kalman parameters at trade entry to prevent
/// the online filter from adapting to structural price drift ("beta chaser")
/// and triggering premature phantom exits while physical positions suffer loss.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntryModelSnapshot {
    pub alpha: f64,
    pub beta: f64,
    pub variance: f64,
}

pub struct StatArbPairsStrategy {
    pub symbol_y: String,
    pub symbol_x: String,
    pub exchange: Arc<dyn ExchangeAdapter>,
    pub kalman: HedgeRatioKalmanFilter,
    pub spread_stats: RollingWindowStats,
    pub entry_z_score: Decimal,
    pub exit_z_score: Decimal,
    pub stop_loss_z_score: Option<Decimal>,
    pub max_holding_ticks: usize,
    pub base_trade_size_y: Decimal,
    pub round_trip_fee_bps: Decimal,
    pub min_profit_hurdle_bps: Decimal,
    pub beta_rebalance_threshold_pct: f64,
    /// Internal position state. NOTE: this tracks *emitted signals*, not
    /// venue truth — it assumes the consumer executes every signal. A
    /// consumer that may drop a signal must reconcile position state with
    /// the venue before relying on this machine (live paths fail closed
    /// at the venue-position layer regardless).
    pub position: SpreadPosition,
    /// Sides of the open position so Exit emits the closing legs.
    pub open_sides: Option<(Side, Side, Decimal, Decimal)>,
    /// Frozen entry parameters to evaluate open position without Kalman beta-chaser distortion.
    pub entry_snapshot: Option<EntryModelSnapshot>,
    /// Number of steps elapsed while in current position.
    pub holding_ticks: usize,
    pub prices_y: Vec<f64>,
    pub prices_x: Vec<f64>,
    /// Optional Hamilton 2-state regime-switching Kalman filter for structural breakdown gating.
    pub hamilton_filter: Option<arbitrageur_math::factor_stat_arb::HamiltonKalmanSpreadFilter>,
    /// Optional high-frequency VECM engine with Hawkes-modulated adjustment speed.
    pub vecm_hawkes_engine: Option<arbitrageur_math::hf_vecm::HfVecmHawkesEngine>,
    pub last_hawkes_intensity: f64,
    pub last_hawkes_spectral_radius: f64,
    /// Optional Rough Fractional Ornstein-Uhlenbeck (fOU) spread engine for sub-diffusive memory.
    pub rough_fou_engine: Option<arbitrageur_math::rough_ou::RoughFouSpreadEngine>,
    /// Optional high-frequency multi-asset VECM residual sniper with Leung-Li optimal stopping boundaries.
    pub vecm_residual_sniper: Option<arbitrageur_math::hf_vecm::HfMultiAssetVecmResidualSniper>,
    /// Optional online Granger causality matrix to identify directional information leaders and followers.
    pub granger_causality_matrix: Option<arbitrageur_math::hf_vecm::GrangerCausalityMatrix>,
}

impl StatArbPairsStrategy {
    /// `entry_z_score` must be positive and strictly above `exit_z_score`
    /// (the exit band must sit inside the entry band for a reversion exit
    /// to be reachable); `exit_z_score` must be non-negative.
    pub fn new(
        symbol_y: impl Into<String>,
        symbol_x: impl Into<String>,
        exchange: Arc<dyn ExchangeAdapter>,
        entry_z_score: Decimal,
        exit_z_score: Decimal,
        base_trade_size_y: Decimal,
    ) -> anyhow::Result<Self> {
        if entry_z_score <= dec!(0) {
            bail!("entry_z_score must be positive, got {entry_z_score}");
        }
        if exit_z_score < dec!(0) {
            bail!("exit_z_score must be non-negative, got {exit_z_score}");
        }
        if exit_z_score >= entry_z_score {
            bail!(
                "exit_z_score ({exit_z_score}) must be below entry_z_score \
                 ({entry_z_score}) or exits can never trigger"
            );
        }
        if base_trade_size_y <= dec!(0) {
            bail!("base_trade_size_y must be positive, got {base_trade_size_y}");
        }
        let stop_loss_z_score = Some(dec!(3.5).max(entry_z_score * dec!(1.5)));
        Ok(Self {
            symbol_y: symbol_y.into(),
            symbol_x: symbol_x.into(),
            exchange,
            kalman: HedgeRatioKalmanFilter::new(1e-4, 1e-2),
            spread_stats: RollingWindowStats::new(100),
            entry_z_score,
            exit_z_score,
            stop_loss_z_score,
            max_holding_ticks: 300,
            base_trade_size_y,
            round_trip_fee_bps: dec!(10.0),
            min_profit_hurdle_bps: dec!(5.0),
            beta_rebalance_threshold_pct: 0.15,
            position: SpreadPosition::Flat,
            open_sides: None,
            entry_snapshot: None,
            holding_ticks: 0,
            prices_y: Vec::new(),
            prices_x: Vec::new(),
            hamilton_filter: None,
            vecm_hawkes_engine: None,
            last_hawkes_intensity: 5.0,
            last_hawkes_spectral_radius: 0.40,
            rough_fou_engine: None,
            vecm_residual_sniper: None,
            granger_causality_matrix: None,
        })
    }

    /// Builder to configure high-frequency VECM Hawkes engine.
    pub fn with_vecm_hawkes_engine(
        mut self,
        engine: arbitrageur_math::hf_vecm::HfVecmHawkesEngine,
    ) -> Self {
        self.vecm_hawkes_engine = Some(engine);
        self
    }

    /// Builder to configure Rough Fractional Ornstein-Uhlenbeck spread engine.
    pub fn with_rough_fou_engine(
        mut self,
        engine: arbitrageur_math::rough_ou::RoughFouSpreadEngine,
    ) -> Self {
        self.rough_fou_engine = Some(engine);
        self
    }

    /// Builder to configure high-frequency multi-asset VECM residual sniper.
    pub fn with_vecm_residual_sniper(
        mut self,
        sniper: arbitrageur_math::hf_vecm::HfMultiAssetVecmResidualSniper,
    ) -> Self {
        self.vecm_residual_sniper = Some(sniper);
        self
    }

    /// Builder to configure online Granger causality matrix.
    pub fn with_granger_causality_matrix(
        mut self,
        matrix: arbitrageur_math::hf_vecm::GrangerCausalityMatrix,
    ) -> Self {
        self.granger_causality_matrix = Some(matrix);
        self
    }

    /// Updates live Hawkes intensity and spectral radius from orderbook streams.
    pub fn update_hawkes_metrics(&mut self, intensity: f64, spectral_radius: f64) {
        self.last_hawkes_intensity = intensity;
        self.last_hawkes_spectral_radius = spectral_radius;
    }

    /// Builder method to configure custom stop-loss z-score threshold.
    /// `None` disables the divergence stop-loss. If `Some(sl)`, `sl` must be strictly
    /// greater than `entry_z_score`.
    pub fn with_stop_loss_z_score(mut self, sl: Option<Decimal>) -> anyhow::Result<Self> {
        if let Some(s) = sl {
            if s <= self.entry_z_score {
                bail!(
                    "stop_loss_z_score ({s}) must be strictly above entry_z_score ({})",
                    self.entry_z_score
                );
            }
        }
        self.stop_loss_z_score = sl;
        Ok(self)
    }

    /// Builder method to configure max holding duration (in steps/ticks) before timing out.
    /// 0 disables the holding duration timeout.
    pub fn with_max_holding_ticks(mut self, ticks: usize) -> Self {
        self.max_holding_ticks = ticks;
        self
    }

    pub fn with_round_trip_fee_bps(mut self, fee_bps: Decimal) -> Self {
        self.round_trip_fee_bps = fee_bps;
        self
    }

    pub fn with_min_profit_hurdle_bps(mut self, hurdle_bps: Decimal) -> Self {
        self.min_profit_hurdle_bps = hurdle_bps;
        self
    }

    pub fn with_beta_rebalance_threshold_pct(mut self, threshold_pct: f64) -> Self {
        self.beta_rebalance_threshold_pct = threshold_pct.max(0.01);
        self
    }

    /// Builder method to configure optional Hamilton 2-state regime-switching Kalman filter.
    pub fn with_hamilton_filter(
        mut self,
        filter: arbitrageur_math::factor_stat_arb::HamiltonKalmanSpreadFilter,
    ) -> Self {
        self.hamilton_filter = Some(filter);
        self
    }

    /// Step strategy with latest market prices and evaluate z-score.
    /// Emits at most one signal per step; while a position is open only an
    /// exit can fire — entries cannot stack.
    pub async fn step(&mut self) -> anyhow::Result<Option<SpreadSignal>> {
        let (tick_y, tick_x) = tokio::try_join!(
            self.exchange.fetch_ticker(&self.symbol_y),
            self.exchange.fetch_ticker(&self.symbol_x)
        )?;
        self.observe_pair_prices(tick_y.last, tick_x.last)
    }

    /// Pure observation entry point: runs the Kalman/Hamilton state machine
    /// on supplied prices without fetching from an exchange adapter.
    pub fn observe_pair_prices(
        &mut self,
        price_y: Decimal,
        price_x: Decimal,
    ) -> Result<Option<PairSignal>, ArbitrageError> {
        let Some(py_f64) = price_y.to_f64() else {
            return Ok(None);
        };
        let Some(px_f64) = price_x.to_f64() else {
            return Ok(None);
        };

        if py_f64 <= 0.0 || px_f64 <= 0.0 {
            return Ok(None);
        }

        self.prices_y.push(py_f64);
        self.prices_x.push(px_f64);
        if self.prices_y.len() > 120 {
            self.prices_y.remove(0);
            self.prices_x.remove(0);
        }

        let (_beta, residual) = self.kalman.update(px_f64, py_f64);
        let res_dec = Decimal::from_str_exact(&format!("{:.6}", residual))
            .ok()
            .or_else(|| Decimal::from_f64_retain(residual))
            .unwrap_or(dec!(0));

        self.spread_stats.push(res_dec);

        // Update Hamilton regime-switching filter if configured (1-minute observation interval dt ~ 1.9e-6 yr)
        if let Some(ref mut hf) = self.hamilton_filter {
            hf.update(residual, 1.0 / (365.25 * 24.0 * 60.0));
        }

        // Ingest residual into Rough fOU engine if configured
        if let Some(ref mut rf) = self.rough_fou_engine {
            rf.ingest_spread(residual);
            rf.update_hurst_adaptive(0.05);
        }

        let kz = self.kalman.spread_z_score();
        let z = if kz.is_finite() && self.kalman.latest_variance > 1e-12 {
            let Some(kz_dec) = Decimal::from_str_exact(&format!("{:.4}", kz))
                .ok()
                .or_else(|| Decimal::from_f64_retain(kz))
            else {
                return Ok(None);
            };
            kz_dec
        } else if let Some(rolling_z) = self.spread_stats.z_score(res_dec) {
            rolling_z
        } else {
            return Ok(None);
        };

        if self.position == SpreadPosition::Flat {
            // Block entry if Hamilton filter indicates trending/breakdown regime (P(Regime 1) < 0.70)
            if self
                .hamilton_filter
                .as_ref()
                .map(|hf| !hf.is_mean_reverting(0.70))
                .unwrap_or(false)
            {
                tracing::debug!(
                    "StatArbPairsStrategy: entry blocked by Hamilton trending/breakdown regime"
                );
                return Ok(None);
            }

            // Block entry if VECM Hawkes indicates supercritical cascade
            if let Some(ref vecm) = self.vecm_hawkes_engine {
                let log_px = [py_f64.ln(), px_f64.ln()];
                let regime = vecm.evaluate_execution_regime(
                    &log_px,
                    self.last_hawkes_intensity,
                    self.last_hawkes_spectral_radius,
                );
                if matches!(
                    regime,
                    arbitrageur_math::hf_vecm::VecmRegimeDecision::CircuitHaltSupercriticalBurst { .. }
                ) {
                    tracing::warn!(
                        "StatArbPairsStrategy: entry blocked by VECM Supercritical Hawkes cascade"
                    );
                    return Ok(None);
                }
            }

            // Block entry if Rough fOU engine indicates spread is within sub-diffusive rough memory band
            if let Some(ref rf) = self.rough_fou_engine {
                let entry_z_f64 = self.entry_z_score.to_string().parse::<f64>().unwrap_or(2.0);
                let exit_z_f64 = self.exit_z_score.to_string().parse::<f64>().unwrap_or(0.5);
                let rf_sig = rf.evaluate_signal(residual, 1.0, entry_z_f64, exit_z_f64);
                if matches!(
                    rf_sig,
                    arbitrageur_math::rough_ou::RoughFouSignal::HoldWithinBand { .. }
                ) {
                    tracing::debug!(
                        "StatArbPairsStrategy: entry blocked by Rough fOU dynamic band (microstructure roughness)"
                    );
                    return Ok(None);
                }
            }

            // Granger Causality Lead-Lag Protection:
            // If asset X leads asset Y, and the dislocation is driven by asset X moving,
            // do NOT take a naive mean-reverting entry fading asset X's price discovery.
            if let Some(ref gc) = self.granger_causality_matrix {
                // Asset 1 (X) Granger-leads Asset 0 (Y)
                if gc.is_leader(1, 0) && self.prices_x.len() >= 2 {
                    let px_prev = self.prices_x[self.prices_x.len() - 2];
                    let px_curr = self.prices_x[self.prices_x.len() - 1];
                    let delta_x_bps = ((px_curr - px_prev) / px_prev).abs() * 10_000.0;
                    if delta_x_bps > 15.0 {
                        tracing::warn!(
                            "StatArbPairsStrategy: entry blocked by Granger causality: leading asset {} jumped {:.1} bps, follower {} lagging",
                            self.symbol_x, delta_x_bps, self.symbol_y
                        );
                        return Ok(None);
                    }
                }
            }

            // VECM Multi-Asset Residual Sniper & Leung-Li optimal stopping boundary gating:
            if let Some(ref sniper) = self.vecm_residual_sniper {
                let log_px = [py_f64.ln(), px_f64.ln()];
                let (action, res_z, (d_star, b_star)) = sniper.evaluate_signal(&log_px);
                if action == arbitrageur_math::hf_vecm::VecmSignalAction::HoldNeutral {
                    tracing::debug!(
                        "StatArbPairsStrategy: entry held neutral by Leung-Li boundaries: z={:.4} within [{:.4}, {:.4}]",
                        res_z, d_star, b_star
                    );
                    return Ok(None);
                }
            }

            return self.entry_signal(z, py_f64);
        }

        self.holding_ticks += 1;

        // 1. Max holding duration timeout
        if self.max_holding_ticks > 0 && self.holding_ticks >= self.max_holding_ticks {
            tracing::warn!(
                "StatArbPairsStrategy: holding timeout reached ({} ticks) — taking exit",
                self.holding_ticks
            );
            self.position = SpreadPosition::Flat;
            self.entry_snapshot = None;
            self.holding_ticks = 0;
            return Ok(self.take_exit());
        }

        // 2a. Hamilton regime breakdown supervision: exit if mean-reverting confidence collapses (< 50%)
        if self
            .hamilton_filter
            .as_ref()
            .map(|hf| !hf.is_mean_reverting(0.50))
            .unwrap_or(false)
        {
            tracing::warn!(
                "StatArbPairsStrategy: Hamilton Regime Breakdown (P(Regime 1) < 0.50) — taking exit"
            );
            self.position = SpreadPosition::Flat;
            self.entry_snapshot = None;
            self.holding_ticks = 0;
            return Ok(self.take_exit());
        }

        // 2b. VECM Supercritical Hawkes cascade supervision: protective exit
        if let Some(ref vecm) = self.vecm_hawkes_engine {
            let log_px = [py_f64.ln(), px_f64.ln()];
            let regime = vecm.evaluate_execution_regime(
                &log_px,
                self.last_hawkes_intensity,
                self.last_hawkes_spectral_radius,
            );
            if matches!(
                regime,
                arbitrageur_math::hf_vecm::VecmRegimeDecision::CircuitHaltSupercriticalBurst { .. }
            ) {
                tracing::warn!(
                    "StatArbPairsStrategy: VECM Supercritical Hawkes cascade detected — taking protective exit"
                );
                self.position = SpreadPosition::Flat;
                self.entry_snapshot = None;
                self.holding_ticks = 0;
                return Ok(self.take_exit());
            }
        }

        // 2b. Rolling cointegration breakdown supervision (every 30 ticks, if history >= 60)
        if self.holding_ticks.is_multiple_of(30) && self.prices_y.len() >= 60 {
            if let Some(coint) =
                arbitrageur_math::cointegration::engle_granger_test(&self.prices_y, &self.prices_x)
            {
                if !coint.is_cointegrated_5pct || coint.half_life > 150.0 {
                    tracing::warn!(
                        "StatArbPairsStrategy: Structural Cointegration Breakdown detected (ADF t={:.2} > crit {:.2}, half-life {:.1}) — taking exit",
                        coint.adf_statistic, coint.critical_values.1, coint.half_life
                    );
                    self.position = SpreadPosition::Flat;
                    self.entry_snapshot = None;
                    self.holding_ticks = 0;
                    return Ok(self.take_exit());
                }
            }
        }

        // 3. Dynamic beta drift rebalancing
        let current_beta = self.kalman.state[1];
        if let Some(snap) = self.entry_snapshot {
            let beta_drift_pct = (current_beta - snap.beta).abs() / snap.beta.abs().max(1e-4);
            if beta_drift_pct >= self.beta_rebalance_threshold_pct {
                if let Some((side_y, side_x, qty_y, cur_qty_x)) = self.open_sides {
                    let current_beta_dec =
                        Decimal::from_f64_retain(current_beta).unwrap_or(dec!(0));
                    let target_side_x = hedge_side_for_y(side_y, current_beta_dec);
                    let target_qty_x =
                        qty_y * Decimal::from_f64_retain(current_beta.abs()).unwrap_or(dec!(1.0));

                    // A sign flip requires closing the old hedge before
                    // opening the reversed one. Emit the close first and
                    // leave the desired side in place for the next rebalance.
                    if target_side_x != side_x {
                        if cur_qty_x > dec!(0) {
                            self.open_sides = Some((side_y, side_x, qty_y, dec!(0)));
                            return Ok(Some(SpreadSignal::RebalanceHedge {
                                side_x: side_x.opposite(),
                                delta_qty_x: cur_qty_x,
                                new_total_qty_x: dec!(0),
                            }));
                        }
                        if target_qty_x > dec!(0) {
                            self.open_sides = Some((side_y, target_side_x, qty_y, target_qty_x));
                            self.entry_snapshot = Some(EntryModelSnapshot {
                                alpha: snap.alpha,
                                beta: current_beta,
                                variance: snap.variance,
                            });
                            return Ok(Some(SpreadSignal::RebalanceHedge {
                                side_x: target_side_x,
                                delta_qty_x: target_qty_x,
                                new_total_qty_x: target_qty_x,
                            }));
                        }
                    }

                    let delta_x = target_qty_x - cur_qty_x;
                    if delta_x.abs() >= dec!(0.001) {
                        let rebal_side = if delta_x > dec!(0) {
                            target_side_x
                        } else {
                            target_side_x.opposite()
                        };
                        self.open_sides = Some((side_y, target_side_x, qty_y, target_qty_x));
                        self.entry_snapshot = Some(EntryModelSnapshot {
                            alpha: snap.alpha,
                            beta: current_beta,
                            variance: snap.variance,
                        });
                        return Ok(Some(SpreadSignal::RebalanceHedge {
                            side_x: rebal_side,
                            delta_qty_x: delta_x.abs(),
                            new_total_qty_x: target_qty_x,
                        }));
                    }
                }
            }
        }

        // 4. Evaluate spread against frozen entry snapshot to prevent Kalman beta chaser
        let eval_z = if let Some(snap) = self.entry_snapshot {
            let frozen_residual = py_f64 - (snap.alpha + snap.beta * px_f64);
            if snap.variance > 1e-12 {
                let kz = frozen_residual / snap.variance.sqrt();
                Decimal::from_str_exact(&format!("{:.4}", kz))
                    .ok()
                    .or_else(|| Decimal::from_f64_retain(kz))
                    .unwrap_or(z)
            } else {
                z
            }
        } else {
            z
        };

        // 5. Evaluate stop-loss and reversion exits
        match self.position {
            SpreadPosition::Long => {
                // Divergence stop loss (e.g. z <= -3.5)
                if let Some(sl) = self.stop_loss_z_score {
                    if eval_z <= -sl {
                        tracing::warn!(
                            "StatArbPairsStrategy: Long spread stopped out at z={:.4} (limit -{:.4})",
                            eval_z, sl
                        );
                        self.position = SpreadPosition::Flat;
                        self.entry_snapshot = None;
                        self.holding_ticks = 0;
                        return Ok(self.take_exit());
                    }
                }
                // Mean-reversion exit (e.g. z >= -0.5)
                if eval_z >= -self.exit_z_score {
                    self.position = SpreadPosition::Flat;
                    self.entry_snapshot = None;
                    self.holding_ticks = 0;
                    return Ok(self.take_exit());
                }

                // Rough fOU sub-diffusive reversion exit
                if let Some(ref rf) = self.rough_fou_engine {
                    let entry_z_f64 = self.entry_z_score.to_string().parse::<f64>().unwrap_or(2.0);
                    let exit_z_f64 = self.exit_z_score.to_string().parse::<f64>().unwrap_or(0.5);
                    let rf_sig = rf.evaluate_signal(residual, 1.0, entry_z_f64, exit_z_f64);
                    if matches!(
                        rf_sig,
                        arbitrageur_math::rough_ou::RoughFouSignal::ExitMeanReverted { .. }
                    ) {
                        tracing::info!(
                            "StatArbPairsStrategy: Long spread exited via Rough fOU sub-diffusive mean-reversion"
                        );
                        self.position = SpreadPosition::Flat;
                        self.entry_snapshot = None;
                        self.holding_ticks = 0;
                        return Ok(self.take_exit());
                    }
                }
            }
            SpreadPosition::Short => {
                // Divergence stop loss (e.g. z >= +3.5)
                if let Some(sl) = self.stop_loss_z_score {
                    if eval_z >= sl {
                        tracing::warn!(
                            "StatArbPairsStrategy: Short spread stopped out at z={:.4} (limit +{:.4})",
                            eval_z, sl
                        );
                        self.position = SpreadPosition::Flat;
                        self.entry_snapshot = None;
                        self.holding_ticks = 0;
                        return Ok(self.take_exit());
                    }
                }
                // Mean-reversion exit (e.g. z <= +0.5)
                if eval_z <= self.exit_z_score {
                    self.position = SpreadPosition::Flat;
                    self.entry_snapshot = None;
                    self.holding_ticks = 0;
                    return Ok(self.take_exit());
                }

                // Rough fOU sub-diffusive reversion exit
                if let Some(ref rf) = self.rough_fou_engine {
                    let entry_z_f64 = self.entry_z_score.to_string().parse::<f64>().unwrap_or(2.0);
                    let exit_z_f64 = self.exit_z_score.to_string().parse::<f64>().unwrap_or(0.5);
                    let rf_sig = rf.evaluate_signal(residual, 1.0, entry_z_f64, exit_z_f64);
                    if matches!(
                        rf_sig,
                        arbitrageur_math::rough_ou::RoughFouSignal::ExitMeanReverted { .. }
                    ) {
                        tracing::info!(
                            "StatArbPairsStrategy: Short spread exited via Rough fOU sub-diffusive mean-reversion"
                        );
                        self.position = SpreadPosition::Flat;
                        self.entry_snapshot = None;
                        self.holding_ticks = 0;
                        return Ok(self.take_exit());
                    }
                }
            }
            SpreadPosition::Flat => {}
        }

        // 5b. VECM Leung-Li optimal stopping take-profit exit check
        if self.position != SpreadPosition::Flat {
            if let Some(ref sniper) = self.vecm_residual_sniper {
                let log_px = [py_f64.ln(), px_f64.ln()];
                let (action, _, _) = sniper.evaluate_signal(&log_px);
                if action == arbitrageur_math::hf_vecm::VecmSignalAction::ExitEquilibrium {
                    tracing::info!(
                        "StatArbPairsStrategy: Leung-Li double optimal stopping equilibrium reached — taking profit exit"
                    );
                    self.position = SpreadPosition::Flat;
                    self.entry_snapshot = None;
                    self.holding_ticks = 0;
                    return Ok(self.take_exit());
                }
            }
        }

        Ok(None)
    }

    fn entry_signal(&mut self, z: Decimal, py_f64: f64) -> anyhow::Result<Option<SpreadSignal>> {
        let beta_dec = {
            let b = self.kalman.state[1];
            Decimal::from_str_exact(&format!("{:.4}", b))
                .ok()
                .or_else(|| Decimal::from_f64_retain(b))
                .unwrap_or(dec!(1.0))
        };

        let abs_beta = beta_dec.abs();
        if abs_beta <= dec!(0.0001) || abs_beta > dec!(10.0) {
            tracing::warn!(
                "[{}/{}] StatArb pairs rejected anomalous hedge ratio: beta={:.4} (valid range [0.0001, 10.0])",
                self.symbol_y,
                self.symbol_x,
                abs_beta
            );
            return Ok(None);
        }

        // Fee Hurdle Verification: ensure expected reversion edge exceeds fees + hurdle
        let residual_std_bps = if py_f64 > 0.0 {
            (self.kalman.latest_variance.sqrt() * 10000.0) / py_f64
        } else {
            0.0
        };
        let expected_edge_bps = {
            let z_diff = (self.entry_z_score - self.exit_z_score)
                .to_string()
                .parse::<f64>()
                .unwrap_or(1.5);
            Decimal::from_f64_retain(z_diff * residual_std_bps).unwrap_or(dec!(0))
        };
        if expected_edge_bps < self.round_trip_fee_bps + self.min_profit_hurdle_bps {
            tracing::warn!(
                "[{}/{}] StatArb rejected: edge {:.1} bps < fees {:.1} bps + hurdle {:.1} bps",
                self.symbol_y,
                self.symbol_x,
                expected_edge_bps,
                self.round_trip_fee_bps,
                self.min_profit_hurdle_bps
            );
            return Ok(None);
        }

        let qty_y = self.base_trade_size_y;
        let qty_x = qty_y * abs_beta;

        // If beta >= 0, spread = Y - beta*X: Long spread -> Buy Y, Sell X.
        // If beta < 0, spread = Y + |beta|*X: Long spread -> Buy Y, Buy X.
        let side_x_for_long_y = hedge_side_for_y(Side::Buy, beta_dec);
        let side_x_for_short_y = hedge_side_for_y(Side::Sell, beta_dec);

        if z <= -self.entry_z_score {
            // Long Spread: Buy Y, hedge X
            let side_x = side_x_for_long_y;
            self.position = SpreadPosition::Long;
            self.open_sides = Some((Side::Buy, side_x, qty_y, qty_x));
            self.holding_ticks = 0;
            self.entry_snapshot = Some(EntryModelSnapshot {
                alpha: self.kalman.state[0],
                beta: self.kalman.state[1],
                variance: self.kalman.latest_variance,
            });
            return Ok(Some(SpreadSignal::Enter {
                side_y: Side::Buy,
                side_x,
                qty_y,
                qty_x,
            }));
        } else if z >= self.entry_z_score {
            // Short Spread: Sell Y, hedge X
            let side_x = side_x_for_short_y;
            self.position = SpreadPosition::Short;
            self.open_sides = Some((Side::Sell, side_x, qty_y, qty_x));
            self.holding_ticks = 0;
            self.entry_snapshot = Some(EntryModelSnapshot {
                alpha: self.kalman.state[0],
                beta: self.kalman.state[1],
                variance: self.kalman.latest_variance,
            });
            return Ok(Some(SpreadSignal::Enter {
                side_y: Side::Sell,
                side_x,
                qty_y,
                qty_x,
            }));
        }
        Ok(None)
    }

    /// Emit the closing legs (opposite sides of the recorded entry).
    /// Position state is reset by the caller before this runs.
    fn take_exit(&mut self) -> Option<SpreadSignal> {
        self.entry_snapshot = None;
        self.holding_ticks = 0;
        let (open_y, open_x, qty_y, qty_x) = self.open_sides.take()?;
        let flip = |s: Side| match s {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        };
        Some(SpreadSignal::Exit {
            side_y: flip(open_y),
            side_x: flip(open_x),
            qty_y,
            qty_x,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrageur_exchanges::simulated::SimulatedExchange;

    fn simulated_pair() -> Arc<SimulatedExchange> {
        let ex = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));
        ex.seed_market("Y", dec!(100.0), dec!(100.1), dec!(10.0), dec!(0));
        ex.seed_market("X", dec!(100.0), dec!(100.1), dec!(10.0), dec!(0));
        ex
    }

    #[test]
    fn default_fee_and_profit_hurdles_are_non_zero() {
        let strat =
            StatArbPairsStrategy::new("Y", "X", simulated_pair(), dec!(1.5), dec!(0.5), dec!(1.0))
                .expect("valid config");
        assert_eq!(strat.round_trip_fee_bps, dec!(10.0));
        assert_eq!(strat.min_profit_hurdle_bps, dec!(5.0));
    }

    #[test]
    fn negative_beta_flips_the_hedge_side() {
        assert_eq!(hedge_side_for_y(Side::Buy, dec!(0.5)), Side::Sell);
        assert_eq!(hedge_side_for_y(Side::Buy, dec!(-0.5)), Side::Buy);
        assert_eq!(hedge_side_for_y(Side::Sell, dec!(0.5)), Side::Buy);
        assert_eq!(hedge_side_for_y(Side::Sell, dec!(-0.5)), Side::Sell);
    }

    #[test]
    fn observe_pair_prices_runs_without_exchange_io() {
        let mut strat =
            StatArbPairsStrategy::new("Y", "X", simulated_pair(), dec!(1.5), dec!(0.5), dec!(1.0))
                .expect("valid config");
        let signal = strat
            .observe_pair_prices(dec!(100.0), dec!(100.0))
            .expect("observation");
        assert!(signal.is_none());
        assert_eq!(strat.prices_y, vec![100.0]);
        assert_eq!(strat.prices_x, vec![100.0]);
    }

    #[tokio::test]
    async fn test_stat_arb_granger_causality_gating() {
        let ex = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));
        ex.seed_market("Y", dec!(100.0), dec!(100.1), dec!(10.0), dec!(0));
        ex.seed_market("X", dec!(100.0), dec!(100.1), dec!(10.0), dec!(0));

        let mut strat =
            StatArbPairsStrategy::new("Y", "X", ex.clone(), dec!(1.5), dec!(0.5), dec!(1.0))
                .expect("valid config");

        // Seed price history
        strat.prices_x = vec![100.0, 100.5]; // 50 bps jump on leading asset X
        strat.prices_y = vec![100.0, 100.0];

        // Configure Granger causality matrix where Asset 1 (X) leads Asset 0 (Y)
        let mut gc = arbitrageur_math::hf_vecm::GrangerCausalityMatrix::new(2, 0.30);
        let gamma = vec![
            vec![0.05, 0.40], // Asset 0 impacted by Asset 1 (0.40)
            vec![0.01, 0.05], // Asset 1 impacted by Asset 0 (0.01)
        ];
        gc.update_from_gammas(&[gamma]);
        strat = strat.with_granger_causality_matrix(gc);

        // Dislocation occurs because X jumped, but X is the information leader!
        // Entry fading X should be blocked by Granger causality.
        strat.spread_stats.push(dec!(100.0));
        strat.spread_stats.push(dec!(-100.0));

        let res = strat.step().await.expect("step");
        assert!(
            res.is_none(),
            "Entry should be blocked when leader asset X jumped"
        );
    }

    #[tokio::test]
    async fn test_stat_arb_vecm_leung_li_exit() {
        let ex = Arc::new(SimulatedExchange::new(
            "hyperliquid",
            "Hyperliquid",
            0,
            dec!(2.0),
            dec!(20000.0),
        ));
        ex.seed_market("Y", dec!(100.0), dec!(100.1), dec!(10.0), dec!(0));
        ex.seed_market("X", dec!(100.0), dec!(100.1), dec!(10.0), dec!(0));

        let mut strat =
            StatArbPairsStrategy::new("Y", "X", ex.clone(), dec!(1.5), dec!(0.5), dec!(1.0))
                .expect("valid config");

        // Simulate an open Long spread position
        strat.position = SpreadPosition::Long;
        strat.open_sides = Some((Side::Buy, Side::Sell, dec!(1.0), dec!(1.0)));

        // Configure VECM residual sniper where current prices [ln(100), ln(100)] are at equilibrium
        let beta = vec![1.0, -1.0];
        let sniper = arbitrageur_math::hf_vecm::HfMultiAssetVecmResidualSniper::new(
            beta, 10.0, 0.0, 0.002, 0.0005,
        );
        strat = strat.with_vecm_residual_sniper(sniper);

        // When evaluating at equilibrium, Leung-Li triggers take-profit exit
        let res = strat.step().await.expect("step");
        assert!(matches!(res, Some(SpreadSignal::Exit { .. })));
        assert_eq!(strat.position, SpreadPosition::Flat);
    }
}
