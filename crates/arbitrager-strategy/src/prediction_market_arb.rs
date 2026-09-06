use std::sync::Arc;
use tracing::info;
use arbitrager_core::exchange::ExchangeAdapter;
use arbitrager_core::order::OrderRequest;
use arbitrager_core::types::{dec, Decimal, Side};
use arbitrager_execution::coordinator::{ArbitrageExecutionResult, AtomicArbitrageCoordinator};
use arbitrager_math::prediction_synthetic::{evaluate_prediction_cross_venue, PredictionArbitrageOpportunity};

pub struct PredictionMarketArbStrategy {
    pub poly_exchange: Arc<dyn ExchangeAdapter>,
    pub kalshi_exchange: Arc<dyn ExchangeAdapter>,
    pub poly_symbol: String,
    pub kalshi_symbol: String,
    pub fee_bps: Decimal,
    pub trade_qty: Decimal,
}

impl PredictionMarketArbStrategy {
    pub fn new(
        poly_exchange: Arc<dyn ExchangeAdapter>,
        kalshi_exchange: Arc<dyn ExchangeAdapter>,
        poly_symbol: impl Into<String>,
        kalshi_symbol: impl Into<String>,
        fee_bps: Decimal,
        trade_qty: Decimal,
    ) -> Self {
        Self {
            poly_exchange,
            kalshi_exchange,
            poly_symbol: poly_symbol.into(),
            kalshi_symbol: kalshi_symbol.into(),
            fee_bps,
            trade_qty,
        }
    }

    /// Check orderbooks on Polymarket and Kalshi for synthetic arbitrage.
    pub async fn check_opportunity(&self) -> anyhow::Result<Option<PredictionArbitrageOpportunity>> {
        let (poly_book, kalshi_book) = tokio::try_join!(
            self.poly_exchange.fetch_orderbook(&self.poly_symbol),
            self.kalshi_exchange.fetch_orderbook(&self.kalshi_symbol)
        )?;

        // Polymarket YES ask / NO ask
        let poly_yes_ask = poly_book.best_ask_price().unwrap_or(dec!(1.0));
        let poly_no_ask = (dec!(1.0) - poly_book.best_bid_price().unwrap_or(dec!(0.0))).min(dec!(1.0));

        // Kalshi YES ask / NO ask
        let kalshi_no_ask = kalshi_book.best_ask_price().unwrap_or(dec!(1.0));
        let kalshi_yes_ask = (dec!(1.0) - kalshi_book.best_bid_price().unwrap_or(dec!(0.0))).min(dec!(1.0));

        if let Some(opp) = evaluate_prediction_cross_venue(
            self.poly_exchange.name(),
            poly_yes_ask,
            poly_no_ask,
            self.kalshi_exchange.name(),
            kalshi_yes_ask,
            kalshi_no_ask,
            self.fee_bps,
        ) {
            return Ok(Some(opp));
        }

        Ok(None)
    }

    /// Execute synthetic binary arbitrage trade.
    pub async fn execute(
        &self,
        opp: &PredictionArbitrageOpportunity,
        coordinator: &AtomicArbitrageCoordinator,
    ) -> ArbitrageExecutionResult {
        info!(
            "🎯 Executing Prediction Market Arbitrage: {}. Expected Net Profit: ${:.4} ({:.2}%)",
            opp.strategy_name, opp.net_profit * self.trade_qty, opp.net_return_pct
        );

        let order_poly = OrderRequest::new_limit(
            self.poly_exchange.venue_id(),
            &self.poly_symbol,
            Side::Buy,
            opp.yes_price,
            self.trade_qty,
        );

        let order_kalshi = OrderRequest::new_limit(
            self.kalshi_exchange.venue_id(),
            &self.kalshi_symbol,
            Side::Buy,
            opp.no_price,
            self.trade_qty,
        );

        coordinator.execute_two_leg(&self.poly_exchange, order_poly, &self.kalshi_exchange, order_kalshi).await
    }
}
