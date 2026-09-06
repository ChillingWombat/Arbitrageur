use arbitrager_core::order::OrderRequest;
use arbitrager_core::types::{dec, Decimal, Side, VenueId, BPS_DIVISOR};

/// Pricing helper to construct bounded limit and IOC orders with protective slippage limits.
#[derive(Debug, Clone)]
pub struct SmartOrderRouter {
    pub max_slippage_bps: Decimal,
}

impl SmartOrderRouter {
    pub fn new(max_slippage_bps: Decimal) -> Self {
        Self { max_slippage_bps }
    }

    /// Calculate limit price with protective slippage allowance:
    /// - For BUY: price = reference_price * (1 + slippage_bps / 10000)
    /// - For SELL: price = reference_price * (1 - slippage_bps / 10000)
    pub fn calculate_bounded_price(&self, side: Side, reference_price: Decimal) -> Decimal {
        let allowance = reference_price * (self.max_slippage_bps / BPS_DIVISOR);
        match side {
            Side::Buy => reference_price + allowance,
            Side::Sell => (reference_price - allowance).max(dec!(0.0001)),
        }
    }

    /// Build an IOC (Immediate-Or-Cancel) order with bounded price.
    pub fn build_ioc_order(
        &self,
        venue: VenueId,
        symbol: impl Into<String>,
        side: Side,
        reference_price: Decimal,
        quantity: Decimal,
    ) -> OrderRequest {
        let price = self.calculate_bounded_price(side, reference_price);
        OrderRequest::new_ioc(venue, symbol, side, price, quantity)
    }
}
