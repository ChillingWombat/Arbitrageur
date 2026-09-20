use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::order::{OrderRequest, OrderResponse};
use arbitrageur_core::types::{Decimal, OrderStatus, Side};
use std::sync::Arc;
use tracing::{error, info};

/// Handles auto-rollback and market unwinding when an atomic multi-leg arbitrage leg fails.
pub struct EmergencyUnwindHandler;

impl EmergencyUnwindHandler {
    /// Executes emergency unwind on an unhedged filled leg:
    /// Submits an immediate market order in the opposite direction for the filled quantity.
    pub async fn unwind_leg(
        exchange: &Arc<dyn ExchangeAdapter>,
        symbol: &str,
        filled_side: Side,
        unhedged_qty: Decimal,
    ) -> anyhow::Result<OrderResponse> {
        let unwind_side = filled_side.opposite();
        error!(
            "🚨 CRITICAL: Executing EMERGENCY UNWIND on {} {}! Unwinding {} {:?} with market order",
            exchange.name(),
            symbol,
            unhedged_qty,
            unwind_side
        );

        let mut unwind_order =
            OrderRequest::new_market(exchange.venue_id(), symbol, unwind_side, unhedged_qty);
        unwind_order.reduce_only = true;
        let unwind_client_id = unwind_order.client_order_id.clone();

        match exchange.place_order(unwind_order).await {
            Ok(resp) => {
                let valid = resp.status == OrderStatus::Filled
                    && resp.filled_qty == unhedged_qty
                    && resp.remaining_qty == Decimal::ZERO
                    && resp.venue == exchange.venue_id()
                    && resp.symbol.as_str() == symbol
                    && resp.original_qty == unhedged_qty
                    && resp.client_order_id == unwind_client_id
                    && !resp.order_id.is_empty()
                    && resp
                        .avg_fill_price
                        .map(|p| p > Decimal::ZERO)
                        .unwrap_or(false)
                    && resp.fee >= Decimal::ZERO;
                if valid {
                    info!(
                        "Emergency unwind order filled successfully on {}: order_id={}",
                        exchange.name(),
                        resp.order_id
                    );
                    Ok(resp)
                } else {
                    error!(
                        "FATAL: EMERGENCY UNWIND INCOMPLETE ON {} {}: status={:?} filled={}/{} remaining={} venue={} symbol={} client_order_id={} order_id={} avg_px={:?} fee={}. MANUAL OPERATOR INTERVENTION REQUIRED!",
                        exchange.name(),
                        symbol,
                        resp.status,
                        resp.filled_qty,
                        unhedged_qty,
                        resp.remaining_qty,
                        resp.venue,
                        resp.symbol,
                        resp.client_order_id,
                        resp.order_id,
                        resp.avg_fill_price,
                        resp.fee
                    );
                    anyhow::bail!(
                        "Emergency unwind incomplete on {} {}: status={:?} filled={}/{} remaining={} venue={} symbol={} client_order_id={} order_id={} avg_px={:?} fee={}",
                        exchange.name(),
                        symbol,
                        resp.status,
                        resp.filled_qty,
                        unhedged_qty,
                        resp.remaining_qty,
                        resp.venue,
                        resp.symbol,
                        resp.client_order_id,
                        resp.order_id,
                        resp.avg_fill_price,
                        resp.fee
                    )
                }
            }
            Err(e) => {
                error!(
                    "FATAL: EMERGENCY UNWIND FAILED ON {} {}: {}. MANUAL OPERATOR INTERVENTION REQUIRED!",
                    exchange.name(),
                    symbol,
                    e
                );
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct CaptureExchange {
        captured: Arc<parking_lot::Mutex<Option<OrderRequest>>>,
    }

    #[async_trait]
    impl ExchangeAdapter for CaptureExchange {
        fn venue_id(&self) -> arbitrageur_core::types::VenueId {
            "test-venue".into()
        }

        fn name(&self) -> &str {
            "test"
        }

        async fn fetch_markets(
            &self,
        ) -> anyhow::Result<Vec<arbitrageur_core::market::MarketMetadata>> {
            anyhow::bail!("not used")
        }

        async fn fetch_orderbook(
            &self,
            _symbol: &str,
        ) -> anyhow::Result<arbitrageur_core::quote::OrderBook> {
            anyhow::bail!("not used")
        }

        async fn fetch_ticker(
            &self,
            _symbol: &str,
        ) -> anyhow::Result<arbitrageur_core::quote::Ticker> {
            anyhow::bail!("not used")
        }

        async fn fetch_funding_rate(
            &self,
            _symbol: &str,
        ) -> anyhow::Result<arbitrageur_core::quote::FundingRate> {
            anyhow::bail!("not used")
        }

        async fn fetch_account(
            &self,
        ) -> anyhow::Result<arbitrageur_core::position::AccountSnapshot> {
            anyhow::bail!("not used")
        }

        async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
            let response = OrderResponse {
                order_id: "unwind_1".to_string(),
                client_order_id: order.client_order_id.clone(),
                venue: order.venue.clone(),
                symbol: order.symbol.clone(),
                status: OrderStatus::Filled,
                original_qty: order.quantity,
                filled_qty: order.quantity,
                remaining_qty: Decimal::ZERO,
                avg_fill_price: Some(arbitrageur_core::types::dec!(100)),
                fee: Decimal::ZERO,
                fee_currency: "USDC".to_string(),
                raw_response: None,
                timestamp: chrono::Utc::now(),
            };
            *self.captured.lock() = Some(order);
            Ok(response)
        }

        async fn cancel_order(
            &self,
            _req: arbitrageur_core::order::CancelRequest,
        ) -> anyhow::Result<()> {
            anyhow::bail!("not used")
        }
    }

    #[tokio::test]
    async fn test_unwind_leg_places_reduce_only_market_order() {
        let captured: Arc<parking_lot::Mutex<Option<OrderRequest>>> =
            Arc::new(parking_lot::Mutex::new(None));
        let exchange: Arc<dyn ExchangeAdapter> = Arc::new(CaptureExchange {
            captured: captured.clone(),
        });

        EmergencyUnwindHandler::unwind_leg(
            &exchange,
            "BTC-USDC",
            Side::Buy,
            arbitrageur_core::types::dec!(1.0),
        )
        .await
        .unwrap();

        let order = captured.lock().take().expect("place_order must be called");
        assert!(order.reduce_only, "emergency unwind must be reduce-only");
        assert_eq!(order.side, Side::Sell);
        assert_eq!(order.quantity, arbitrageur_core::types::dec!(1.0));
        assert_eq!(order.symbol, "BTC-USDC");
    }
}
