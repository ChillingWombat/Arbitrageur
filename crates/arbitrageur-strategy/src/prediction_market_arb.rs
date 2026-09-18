use std::sync::Arc;
use tracing::info;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::order::OrderRequest;
use arbitrageur_core::types::{dec, Decimal, Side};
use arbitrageur_execution::coordinator::{ArbitrageExecutionResult, AtomicArbitrageCoordinator};
use arbitrageur_math::prediction_synthetic::{evaluate_prediction_cross_venue, PredictionArbitrageOpportunity};

pub struct PredictionMarketArbStrategy {
    pub poly_exchange: Arc<dyn ExchangeAdapter>,
    pub kalshi_exchange: Arc<dyn ExchangeAdapter>,
    pub poly_yes_symbol: String,
    pub poly_no_symbol: String,
    pub kalshi_yes_symbol: String,
    pub kalshi_no_symbol: String,
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
        let p_sym = poly_symbol.into();
        let k_sym = kalshi_symbol.into();
        Self {
            poly_exchange,
            kalshi_exchange,
            poly_yes_symbol: format!("{}-YES", p_sym.trim_end_matches("-YES").trim_end_matches("-NO")),
            poly_no_symbol: format!("{}-NO", p_sym.trim_end_matches("-YES").trim_end_matches("-NO")),
            kalshi_yes_symbol: format!("{}-YES", k_sym.trim_end_matches("-YES").trim_end_matches("-NO")),
            kalshi_no_symbol: format!("{}-NO", k_sym.trim_end_matches("-YES").trim_end_matches("-NO")),
            fee_bps,
            trade_qty,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_tokens(
        poly_exchange: Arc<dyn ExchangeAdapter>,
        kalshi_exchange: Arc<dyn ExchangeAdapter>,
        poly_yes_symbol: impl Into<String>,
        poly_no_symbol: impl Into<String>,
        kalshi_yes_symbol: impl Into<String>,
        kalshi_no_symbol: impl Into<String>,
        fee_bps: Decimal,
        trade_qty: Decimal,
    ) -> Self {
        Self {
            poly_exchange,
            kalshi_exchange,
            poly_yes_symbol: poly_yes_symbol.into(),
            poly_no_symbol: poly_no_symbol.into(),
            kalshi_yes_symbol: kalshi_yes_symbol.into(),
            kalshi_no_symbol: kalshi_no_symbol.into(),
            fee_bps,
            trade_qty,
        }
    }

    /// Check orderbooks on Polymarket and Kalshi for synthetic arbitrage.
    pub async fn check_opportunity(&self) -> anyhow::Result<Option<PredictionArbitrageOpportunity>> {
        let (poly_book, kalshi_book) = tokio::try_join!(
            self.poly_exchange.fetch_orderbook(&self.poly_yes_symbol),
            self.kalshi_exchange.fetch_orderbook(&self.kalshi_no_symbol)
        )?;

        let Some(poly_yes_ask) = poly_book.best_ask_price() else { return Ok(None) };
        let Some(poly_bid) = poly_book.best_bid_price() else { return Ok(None) };
        let poly_no_ask = (dec!(1.0) - poly_bid).min(dec!(1.0));

        let Some(kalshi_no_ask) = kalshi_book.best_ask_price() else { return Ok(None) };
        let Some(kalshi_bid) = kalshi_book.best_bid_price() else { return Ok(None) };
        let kalshi_yes_ask = (dec!(1.0) - kalshi_bid).min(dec!(1.0));

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

        let is_yes_on_poly = self.poly_exchange.venue_id().as_str().eq_ignore_ascii_case(&opp.buy_yes_venue)
            || self.poly_exchange.name().eq_ignore_ascii_case(&opp.buy_yes_venue);

        let (poly_price, kalshi_price, poly_sym, kalshi_sym) = if is_yes_on_poly {
            (opp.yes_price, opp.no_price, &self.poly_yes_symbol, &self.kalshi_no_symbol)
        } else {
            (opp.no_price, opp.yes_price, &self.poly_no_symbol, &self.kalshi_yes_symbol)
        };

        let order_poly = OrderRequest::new_limit(
            self.poly_exchange.venue_id(),
            poly_sym,
            Side::Buy,
            poly_price,
            self.trade_qty,
        );

        let order_kalshi = OrderRequest::new_limit(
            self.kalshi_exchange.venue_id(),
            kalshi_sym,
            Side::Buy,
            kalshi_price,
            self.trade_qty,
        );

        coordinator.execute_two_leg(&self.poly_exchange, order_poly, &self.kalshi_exchange, order_kalshi).await
    }
}
