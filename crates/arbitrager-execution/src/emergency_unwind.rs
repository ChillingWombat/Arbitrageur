use std::sync::Arc;
use tracing::{error, info};
use arbitrager_core::exchange::ExchangeAdapter;
use arbitrager_core::order::{OrderRequest, OrderResponse};
use arbitrager_core::types::{Decimal, OrderStatus, Side};

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

        let unwind_order = OrderRequest::new_market(
            exchange.venue_id(),
            symbol,
            unwind_side,
            unhedged_qty,
        );
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
                    && resp.avg_fill_price.map(|p| p > Decimal::ZERO).unwrap_or(false)
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
