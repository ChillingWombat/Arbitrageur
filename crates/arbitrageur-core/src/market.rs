use crate::types::{Decimal, VenueId};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Types of tradable financial markets supported across asset classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MarketType {
    /// Crypto Spot market (e.g. BTC/USDC on Hyperliquid Spot)
    Spot,
    /// Crypto Perpetual Swap with periodic funding rates (e.g. BTC-PERP on Hyperliquid, Lighter, Vanta)
    Perpetual,
    /// Dated Futures contracts (e.g. BTC quarterly)
    DatedFutures,
    /// Prediction Market Binary Option (e.g. Polymarket YES/NO outcome token, Kalshi contract)
    PredictionBinary,
    /// Prediction Market Categorical multi-outcome
    PredictionCategorical,
    /// Traditional Equities (e.g. AAPL, SPY)
    TraditionalEquity,
    /// Traditional Futures (e.g. CME E-mini S&P, Gold futures)
    TraditionalFutures,
    /// Traditional Foreign Exchange (FX)
    TraditionalFx,
}

impl MarketType {
    pub fn is_crypto(&self) -> bool {
        matches!(
            self,
            MarketType::Spot | MarketType::Perpetual | MarketType::DatedFutures
        )
    }

    pub fn is_prediction(&self) -> bool {
        matches!(
            self,
            MarketType::PredictionBinary | MarketType::PredictionCategorical
        )
    }

    pub fn is_traditional(&self) -> bool {
        matches!(
            self,
            MarketType::TraditionalEquity
                | MarketType::TraditionalFutures
                | MarketType::TraditionalFx
        )
    }

    pub fn has_funding_rate(&self) -> bool {
        matches!(self, MarketType::Perpetual)
    }
}

impl fmt::Display for MarketType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MarketType::Spot => write!(f, "SPOT"),
            MarketType::Perpetual => write!(f, "PERP"),
            MarketType::DatedFutures => write!(f, "FUTURES"),
            MarketType::PredictionBinary => write!(f, "PREDICTION_BINARY"),
            MarketType::PredictionCategorical => write!(f, "PREDICTION_CATEGORICAL"),
            MarketType::TraditionalEquity => write!(f, "EQUITY"),
            MarketType::TraditionalFutures => write!(f, "TRAD_FUTURES"),
            MarketType::TraditionalFx => write!(f, "FX"),
        }
    }
}

/// Trading symbol representation with base and quote components.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Symbol {
    pub base: String,
    pub quote: String,
    pub raw: String,
    pub market_type: MarketType,
}

impl Symbol {
    pub fn new(base: impl Into<String>, quote: impl Into<String>, market_type: MarketType) -> Self {
        let b = base.into().to_uppercase();
        let q = quote.into().to_uppercase();
        let raw = format!("{}-{}", b, q);
        Self {
            base: b,
            quote: q,
            raw,
            market_type,
        }
    }

    pub fn perp(base: impl Into<String>, quote: impl Into<String>) -> Self {
        Self::new(base, quote, MarketType::Perpetual)
    }

    pub fn spot(base: impl Into<String>, quote: impl Into<String>) -> Self {
        Self::new(base, quote, MarketType::Spot)
    }

    pub fn prediction_binary(outcome: impl Into<String>, collateral: impl Into<String>) -> Self {
        Self::new(outcome, collateral, MarketType::PredictionBinary)
    }

    pub fn canonical(&self) -> String {
        format!("{}-{}", self.base, self.quote)
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.raw, self.market_type)
    }
}

/// Metadata and exchange parameters for an instrument.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketMetadata {
    pub symbol: Symbol,
    pub venue: VenueId,
    pub venue_symbol: String,
    pub price_decimals: u32,
    pub size_decimals: u32,
    pub min_order_size: Decimal,
    pub min_notional: Decimal,
    pub maker_fee_bps: Decimal,
    pub taker_fee_bps: Decimal,
    pub contract_size: Decimal,
    pub is_active: bool,
}
