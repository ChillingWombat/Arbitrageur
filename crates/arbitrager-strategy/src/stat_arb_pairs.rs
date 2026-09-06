use std::sync::Arc;
use arbitrager_core::exchange::ExchangeAdapter;
use arbitrager_core::types::{dec, Decimal, Side};
use arbitrager_math::kalman_filter::HedgeRatioKalmanFilter;
use arbitrager_math::rolling_stats::RollingWindowStats;

pub struct StatArbPairsStrategy {
    pub symbol_y: String,
    pub symbol_x: String,
    pub exchange: Arc<dyn ExchangeAdapter>,
    pub kalman: HedgeRatioKalmanFilter,
    pub spread_stats: RollingWindowStats,
    pub entry_z_score: Decimal,
    pub exit_z_score: Decimal,
    pub base_trade_size_y: Decimal,
}

impl StatArbPairsStrategy {
    pub fn new(
        symbol_y: impl Into<String>,
        symbol_x: impl Into<String>,
        exchange: Arc<dyn ExchangeAdapter>,
        entry_z_score: Decimal,
        exit_z_score: Decimal,
        base_trade_size_y: Decimal,
    ) -> Self {
        Self {
            symbol_y: symbol_y.into(),
            symbol_x: symbol_x.into(),
            exchange,
            kalman: HedgeRatioKalmanFilter::new(1e-4, 1e-2),
            spread_stats: RollingWindowStats::new(100),
            entry_z_score,
            exit_z_score,
            base_trade_size_y,
        }
    }

    /// Step strategy with latest market prices and evaluate z-score.
    pub async fn step(&mut self) -> anyhow::Result<Option<(Side, Side, Decimal, Decimal)>> {
        let (tick_y, tick_x) = tokio::try_join!(
            self.exchange.fetch_ticker(&self.symbol_y),
            self.exchange.fetch_ticker(&self.symbol_x)
        )?;

        let py_f64 = tick_y.last.to_string().parse::<f64>().unwrap_or(0.0);
        let px_f64 = tick_x.last.to_string().parse::<f64>().unwrap_or(0.0);

        if py_f64 <= 0.0 || px_f64 <= 0.0 {
            return Ok(None);
        }

        let (beta, residual) = self.kalman.update(px_f64, py_f64);
        let res_dec = Decimal::from_str_exact(&format!("{:.6}", residual))
            .ok()
            .or_else(|| Decimal::from_f64_retain(residual))
            .unwrap_or(dec!(0));

        self.spread_stats.push(res_dec);

        if let Some(z) = self.spread_stats.z_score(res_dec) {
            let beta_dec = Decimal::from_str_exact(&format!("{:.4}", beta))
                .ok()
                .or_else(|| Decimal::from_f64_retain(beta))
                .unwrap_or(dec!(1.0));

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
                return Ok(Some((Side::Buy, side_x_for_long_y, qty_y, qty_x)));
            } else if z >= self.entry_z_score {
                // Short Spread: Sell Y, hedge X
                return Ok(Some((Side::Sell, side_x_for_short_y, qty_y, qty_x)));
            }
        }

        Ok(None)
    }
}
