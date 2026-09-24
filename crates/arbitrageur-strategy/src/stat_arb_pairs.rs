use anyhow::bail;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::types::{dec, Decimal, Side};
use arbitrageur_math::kalman_filter::HedgeRatioKalmanFilter;
use arbitrageur_math::rolling_stats::RollingWindowStats;
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpreadPosition {
    Flat,
    /// Entered at z <= -entry: long Y, hedged X.
    Long,
    /// Entered at z >= +entry: short Y, hedged X.
    Short,
}

pub struct StatArbPairsStrategy {
    pub symbol_y: String,
    pub symbol_x: String,
    pub exchange: Arc<dyn ExchangeAdapter>,
    pub kalman: HedgeRatioKalmanFilter,
    pub spread_stats: RollingWindowStats,
    pub entry_z_score: Decimal,
    pub exit_z_score: Decimal,
    pub base_trade_size_y: Decimal,
    /// Internal position state. NOTE: this tracks *emitted signals*, not
    /// venue truth — it assumes the consumer executes every signal. A
    /// consumer that may drop a signal must reconcile position state with
    /// the venue before relying on this machine (live paths fail closed
    /// at the venue-position layer regardless).
    position: SpreadPosition,
    /// Sides of the open position so Exit emits the closing legs.
    open_sides: Option<(Side, Side, Decimal, Decimal)>,
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
        Ok(Self {
            symbol_y: symbol_y.into(),
            symbol_x: symbol_x.into(),
            exchange,
            kalman: HedgeRatioKalmanFilter::new(1e-4, 1e-2),
            spread_stats: RollingWindowStats::new(100),
            entry_z_score,
            exit_z_score,
            base_trade_size_y,
            position: SpreadPosition::Flat,
            open_sides: None,
        })
    }

    /// Step strategy with latest market prices and evaluate z-score.
    /// Emits at most one signal per step; while a position is open only an
    /// exit can fire — entries cannot stack.
    pub async fn step(&mut self) -> anyhow::Result<Option<SpreadSignal>> {
        let (tick_y, tick_x) = tokio::try_join!(
            self.exchange.fetch_ticker(&self.symbol_y),
            self.exchange.fetch_ticker(&self.symbol_x)
        )?;

        let py_f64 = tick_y.last.to_string().parse::<f64>().unwrap_or(0.0);
        let px_f64 = tick_x.last.to_string().parse::<f64>().unwrap_or(0.0);

        if py_f64 <= 0.0 || px_f64 <= 0.0 {
            return Ok(None);
        }

        let (_beta, residual) = self.kalman.update(px_f64, py_f64);
        let res_dec = Decimal::from_str_exact(&format!("{:.6}", residual))
            .ok()
            .or_else(|| Decimal::from_f64_retain(residual))
            .unwrap_or(dec!(0));

        self.spread_stats.push(res_dec);

        let Some(z) = self.spread_stats.z_score(res_dec) else {
            return Ok(None);
        };

        match self.position {
            SpreadPosition::Flat => self.entry_signal(z),
            // Long spread entered at z <= -entry exits on reversion to
            // z >= -exit; short spread (z >= +entry) exits at z <= +exit.
            SpreadPosition::Long if z >= -self.exit_z_score => {
                self.position = SpreadPosition::Flat;
                Ok(self.take_exit())
            }
            SpreadPosition::Short if z <= self.exit_z_score => {
                self.position = SpreadPosition::Flat;
                Ok(self.take_exit())
            }
            _ => Ok(None),
        }
    }

    fn entry_signal(&mut self, z: Decimal) -> anyhow::Result<Option<SpreadSignal>> {
        let beta_dec = {
            let b = self.kalman.state[1];
            Decimal::from_str_exact(&format!("{:.4}", b))
                .ok()
                .or_else(|| Decimal::from_f64_retain(b))
                .unwrap_or(dec!(1.0))
        };

        let abs_beta = beta_dec.abs();
        if abs_beta <= dec!(0.0001) {
            return Ok(None);
        }

        let qty_y = self.base_trade_size_y;
        let qty_x = qty_y * abs_beta;

        // If beta >= 0, spread = Y - beta*X: Long spread -> Buy Y, Sell X.
        // If beta < 0, spread = Y + |beta|*X: Long spread -> Buy Y, Buy X.
        let (side_x_for_long_y, side_x_for_short_y) = if beta_dec >= dec!(0) {
            (Side::Sell, Side::Buy)
        } else {
            (Side::Buy, Side::Sell)
        };

        if z <= -self.entry_z_score {
            // Long Spread: Buy Y, hedge X
            let side_x = side_x_for_long_y;
            self.position = SpreadPosition::Long;
            self.open_sides = Some((Side::Buy, side_x, qty_y, qty_x));
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
