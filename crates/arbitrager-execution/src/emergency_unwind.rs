use std::sync::Arc;
use tracing::{error, info};
use arbitrager_core::exchange::ExchangeAdapter;
use arbitrager_core::order::{OrderRequest, OrderResponse};
use arbitrager_core::types::{Decimal, Side};

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

        match exchange.place_order(unwind_order).await {
            Ok(resp) => {
                info!(
                    "Emergency unwind order filled successfully on {}: order_id={}",
                    exchange.name(),
                    resp.order_id
                );
                Ok(resp)
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
