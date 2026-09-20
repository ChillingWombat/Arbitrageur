use arbitrageur_core::exchange::ExchangeAdapter;
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

        if let Some(opp) = evaluate_funding_carry(
            &rate_a,
            &rate_b,
            self.round_trip_fee_bps,
            self.trade_size_usd,
        ) {
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
        let is_a_long = self
            .exchange_a
            .venue_id()
            .as_str()
            .eq_ignore_ascii_case(&opp.long_venue)
            || self.exchange_a.name().eq_ignore_ascii_case(&opp.long_venue);
        let (long_ex, short_ex) = if is_a_long {
            (&self.exchange_a, &self.exchange_b)
        } else {
            (&self.exchange_b, &self.exchange_a)
        };

        // Determine prices to size order and build protective slippage-bounded IOC orders
        let (ticker_long, ticker_short) = tokio::try_join!(
            long_ex.fetch_ticker(&self.symbol),
            short_ex.fetch_ticker(&self.symbol)
        )?;
        if ticker_long.ask <= dec!(0) || ticker_short.bid <= dec!(0) {
            anyhow::bail!("Invalid price for symbol {}", self.symbol);
        }
        let qty = self.trade_size_usd / ticker_long.ask;

        let order_long = coordinator.router().build_ioc_order(
            long_ex.venue_id(),
            &self.symbol,
            Side::Buy,
            ticker_long.ask,
            qty,
        );
        let order_short = coordinator.router().build_ioc_order(
            short_ex.venue_id(),
            &self.symbol,
            Side::Sell,
            ticker_short.bid,
            qty,
        );

        info!(
            "💰 Opening Funding Carry: Long {} on {} and Short on {}. Expected 8h Rate: {:.4}%, APR: {:.2}%",
            self.symbol, opp.long_venue, opp.short_venue, opp.net_rate_8h * dec!(100), opp.net_apr * dec!(100)
        );

        let res = coordinator
            .execute_two_leg(long_ex, order_long, short_ex, order_short)
            .await;
        Ok(res)
    }
}
