use crate::types::{Decimal, OrderStatus, OrderType, Side, TimeInForce, VenueId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Request to place a new order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRequest {
    pub client_order_id: String,
    pub venue: VenueId,
    pub symbol: String,
    pub side: Side,
    pub order_type: OrderType,
    pub price: Option<Decimal>,
    pub quantity: Decimal,
    pub time_in_force: TimeInForce,
    pub post_only: bool,
    pub reduce_only: bool,
    pub created_at: DateTime<Utc>,
}

impl OrderRequest {
    pub fn new_limit(
        venue: VenueId,
        symbol: impl Into<String>,
        side: Side,
        price: Decimal,
        quantity: Decimal,
    ) -> Self {
        Self {
            client_order_id: Uuid::new_v4().to_string(),
            venue,
            symbol: symbol.into(),
            side,
            order_type: OrderType::Limit,
            price: Some(price),
            quantity,
            time_in_force: TimeInForce::GoodTilCancelled,
            post_only: false,
            reduce_only: false,
            created_at: Utc::now(),
        }
    }

    pub fn new_ioc(
        venue: VenueId,
        symbol: impl Into<String>,
        side: Side,
        price: Decimal,
        quantity: Decimal,
    ) -> Self {
        Self {
            client_order_id: Uuid::new_v4().to_string(),
            venue,
            symbol: symbol.into(),
            side,
            order_type: OrderType::ImmediateOrCancel,
            price: Some(price),
            quantity,
            time_in_force: TimeInForce::ImmediateOrCancel,
            post_only: false,
            reduce_only: false,
            created_at: Utc::now(),
        }
    }

    pub fn new_market(
        venue: VenueId,
        symbol: impl Into<String>,
        side: Side,
        quantity: Decimal,
    ) -> Self {
        Self {
            client_order_id: Uuid::new_v4().to_string(),
            venue,
            symbol: symbol.into(),
            side,
            order_type: OrderType::Market,
            price: None,
            quantity,
            time_in_force: TimeInForce::ImmediateOrCancel,
            post_only: false,
            reduce_only: false,
            created_at: Utc::now(),
        }
    }
}

/// Result returned from placing or querying an order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderResponse {
    pub order_id: String,
    pub client_order_id: String,
    pub venue: VenueId,
    pub symbol: String,
    pub status: OrderStatus,
    pub original_qty: Decimal,
    pub filled_qty: Decimal,
    pub remaining_qty: Decimal,
    pub avg_fill_price: Option<Decimal>,
    /// Order-level fee rollup. `None` when the venue response carries no
    /// commission field — a fabricated rate would misstate realized PnL.
    pub fee: Option<Decimal>,
    pub fee_currency: String,
    pub raw_response: Option<String>,
    pub timestamp: DateTime<Utc>,
}

/// Execution fill details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fill {
    pub trade_id: String,
    pub order_id: String,
    pub client_order_id: String,
    pub venue: VenueId,
    pub symbol: String,
    pub side: Side,
    pub price: Decimal,
    pub quantity: Decimal,
    pub fee: Decimal,
    pub fee_currency: String,
    pub is_maker: bool,
    pub timestamp: DateTime<Utc>,
}

/// Request to cancel an existing order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelRequest {
    pub venue: VenueId,
    pub symbol: String,
    pub order_id: Option<String>,
    pub client_order_id: Option<String>,
}
