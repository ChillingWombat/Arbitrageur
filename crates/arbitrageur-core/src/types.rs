use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

pub use rust_decimal::Decimal;
pub use rust_decimal_macros::dec;

/// Represents a trading venue (exchange, DEX, prediction market, broker).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VenueId(pub String);

impl VenueId {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into().to_lowercase())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for VenueId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for VenueId {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for VenueId {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

/// Order side: Buy or Sell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn opposite(&self) -> Self {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }

    pub fn is_buy(&self) -> bool {
        matches!(self, Side::Buy)
    }

    pub fn is_sell(&self) -> bool {
        matches!(self, Side::Sell)
    }

    pub fn sign(&self) -> Decimal {
        match self {
            Side::Buy => dec!(1),
            Side::Sell => dec!(-1),
        }
    }
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Side::Buy => write!(f, "BUY"),
            Side::Sell => write!(f, "SELL"),
        }
    }
}

impl FromStr for Side {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "BUY" | "B" | "LONG" => Ok(Side::Buy),
            "SELL" | "S" | "SHORT" => Ok(Side::Sell),
            other => anyhow::bail!("Unknown order side: {}", other),
        }
    }
}

/// Order execution type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderType {
    Limit,
    Market,
    PostOnly,
    ImmediateOrCancel,
}

/// Time in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum TimeInForce {
    #[default]
    GoodTilCancelled,
    ImmediateOrCancel,
    FillOrKill,
    PostOnly,
}

/// Lifecycle status of an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderStatus {
    New,
    PartiallyFilled,
    Filled,
    Cancelled,
    Rejected,
    Expired,
}

impl OrderStatus {
    pub fn is_final(&self) -> bool {
        matches!(
            self,
            OrderStatus::Filled
                | OrderStatus::Cancelled
                | OrderStatus::Rejected
                | OrderStatus::Expired
        )
    }

    pub fn is_active(&self) -> bool {
        !self.is_final()
    }
}

/// Basis points constant (1 BPS = 0.0001 = 0.01%).
pub const BPS_DIVISOR: Decimal = dec!(10000);
