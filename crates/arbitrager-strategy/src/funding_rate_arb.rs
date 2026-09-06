use std::sync::Arc;
use tracing::info;
use arbitrager_core::exchange::ExchangeAdapter;
use arbitrager_core::order::OrderRequest;
use arbitrager_core::types::{dec, Decimal, Side};
use arbitrager_execution::coordinator::{ArbitrageExecutionResult, AtomicArbitrageCoordinator};
use arbitrager_math::funding_carry::{evaluate_funding_carry, FundingCarryOpportunity};

pub struct FundingRateArbStrategy {
    pub symbol: String,
    pub exchange_a: Arc<dyn ExchangeAdapter>,
    pub exchange_b: Arc<dyn ExchangeAdapter>,
    pub min_funding_diff_8h: Decimal,
    pub max_breakeven_hours: Decimal,
    pub trade_size_usd: Decimal,
    pub round_trip_fee_bps: Decimal,
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
        }
    }

    /// Scan funding rates on both exchanges and evaluate carry profitability.
    pub async fn scan_opportunity(&self) -> anyhow::Result<Option<FundingCarryOpportunity>> {
        let (rate_a, rate_b) = tokio::try_join!(
            self.exchange_a.fetch_funding_rate(&self.symbol),
            self.exchange_b.fetch_funding_rate(&self.symbol)
        )?;

        if let Some(opp) = evaluate_funding_carry(&rate_a, &rate_b, self.round_trip_fee_bps, self.trade_size_usd) {
            if opp.net_rate_8h >= self.min_funding_diff_8h && opp.break_even_hours <= self.max_breakeven_hours {
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
        let (long_ex, short_ex) = if self.exchange_a.name() == opp.long_venue {
            (&self.exchange_a, &self.exchange_b)
        } else {
            (&self.exchange_b, &self.exchange_a)
        };

        // Determine price to size order
        let ticker = long_ex.fetch_ticker(&self.symbol).await?;
        if ticker.ask <= dec!(0) {
            anyhow::bail!("Invalid ask price for symbol {}", self.symbol);
        }
        let qty = self.trade_size_usd / ticker.ask;

        let order_long = OrderRequest::new_ioc(long_ex.venue_id(), &self.symbol, Side::Buy, dec!(99999999), qty);
        let order_short = OrderRequest::new_ioc(short_ex.venue_id(), &self.symbol, Side::Sell, dec!(0.0001), qty);

        info!(
            "💰 Opening Funding Carry: Long {} on {} and Short on {}. Expected 8h Rate: {:.4}%, APR: {:.2}%",
            self.symbol, opp.long_venue, opp.short_venue, opp.net_rate_8h * dec!(100), opp.net_apr * dec!(100)
        );

        let res = coordinator.execute_two_leg(long_ex, order_long, short_ex, order_short).await;
        Ok(res)
    }
}
