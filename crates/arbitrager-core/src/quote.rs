use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::types::{dec, Decimal, VenueId};

/// Single price level in the orderbook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookLevel {
    pub price: Decimal,
    pub size: Decimal,
}

impl BookLevel {
    pub fn new(price: Decimal, size: Decimal) -> Self {
        Self { price, size }
    }
}

/// L2 Order Book with bids and asks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBook {
    pub venue: VenueId,
    pub symbol: String,
    /// Bids sorted descending by price (highest bid first)
    pub bids: Vec<BookLevel>,
    /// Asks sorted ascending by price (lowest ask first)
    pub asks: Vec<BookLevel>,
    pub timestamp: DateTime<Utc>,
    pub sequence: u64,
}

impl OrderBook {
    pub fn new(venue: VenueId, symbol: String) -> Self {
        Self {
            venue,
            symbol,
            bids: Vec::new(),
            asks: Vec::new(),
            timestamp: Utc::now(),
            sequence: 0,
        }
    }

    pub fn best_bid(&self) -> Option<BookLevel> {
        self.bids.first().copied()
    }

    pub fn best_ask(&self) -> Option<BookLevel> {
        self.asks.first().copied()
    }

    pub fn best_bid_price(&self) -> Option<Decimal> {
        self.best_bid().map(|b| b.price)
    }

    pub fn best_ask_price(&self) -> Option<Decimal> {
        self.best_ask().map(|a| a.price)
    }

    pub fn mid_price(&self) -> Option<Decimal> {
        match (self.best_bid_price(), self.best_ask_price()) {
            (Some(b), Some(a)) => Some((b + a) / dec!(2)),
            _ => None,
        }
    }

    pub fn spread(&self) -> Option<Decimal> {
        match (self.best_bid_price(), self.best_ask_price()) {
            (Some(b), Some(a)) => Some(a - b),
            _ => None,
        }
    }

    /// Rejects books that cannot safely drive execution: empty symbol, missing
    /// bids/asks, nonpositive prices or sizes, unsorted levels, or a
    /// crossed/locked book (`best_bid >= best_ask`).
    pub fn validate_for_execution(&self) -> Result<(), String> {
        if self.symbol.trim().is_empty() {
            return Err("order book symbol is empty".to_string());
        }
        if self.bids.is_empty() {
            return Err(format!("order book {} has no bids", self.symbol));
        }
        if self.asks.is_empty() {
            return Err(format!("order book {} has no asks", self.symbol));
        }
        for level in &self.bids {
            if level.price <= dec!(0) {
                return Err(format!(
                    "order book {} has nonpositive bid price {}",
                    self.symbol, level.price
                ));
            }
            if level.size <= dec!(0) {
                return Err(format!(
                    "order book {} has nonpositive bid size {}",
                    self.symbol, level.size
                ));
            }
        }
        for level in &self.asks {
            if level.price <= dec!(0) {
                return Err(format!(
                    "order book {} has nonpositive ask price {}",
                    self.symbol, level.price
                ));
            }
            if level.size <= dec!(0) {
                return Err(format!(
                    "order book {} has nonpositive ask size {}",
                    self.symbol, level.size
                ));
            }
        }
        for pair in self.bids.windows(2) {
            if pair[0].price < pair[1].price {
                return Err(format!(
                    "order book {} bids are not monotonically descending",
                    self.symbol
                ));
            }
        }
        for pair in self.asks.windows(2) {
            if pair[0].price > pair[1].price {
                return Err(format!(
                    "order book {} asks are not monotonically ascending",
                    self.symbol
                ));
            }
        }
        if let (Some(bid), Some(ask)) = (self.best_bid_price(), self.best_ask_price()) {
            if bid >= ask {
                return Err(format!(
                    "order book {} is crossed or locked: best bid {} >= best ask {}",
                    self.symbol, bid, ask
                ));
            }
        }
        Ok(())
    }

    /// Calculate volume-weighted average price (VWAP) for executing a buy of size `target_qty`
    /// traversing the asks. Returns (vwap, remaining_unfilled_qty, slippage_pct).
    pub fn buy_execution_vwap(&self, target_qty: Decimal) -> Option<(Decimal, Decimal, Decimal)> {
        if self.asks.is_empty() || target_qty <= dec!(0) {
            return None;
        }
        let best_ask = self.asks[0].price;
        let mut filled_qty = dec!(0);
        let mut total_cost = dec!(0);

        for level in &self.asks {
            let available = level.size;
            let needed = target_qty - filled_qty;
            let fill = needed.min(available);
            filled_qty += fill;
            total_cost += fill * level.price;

            if filled_qty >= target_qty {
                break;
            }
        }

        if filled_qty == dec!(0) {
            return None;
        }

        let vwap = total_cost / filled_qty;
        let unfilled = target_qty - filled_qty;
        let slippage = if best_ask > dec!(0) {
            (vwap - best_ask) / best_ask
        } else {
            dec!(0)
        };

        Some((vwap, unfilled, slippage))
    }

    /// Calculate volume-weighted average price (VWAP) for executing a sell of size `target_qty`
    /// traversing the bids. Returns (vwap, remaining_unfilled_qty, slippage_pct).
    pub fn sell_execution_vwap(&self, target_qty: Decimal) -> Option<(Decimal, Decimal, Decimal)> {
        if self.bids.is_empty() || target_qty <= dec!(0) {
            return None;
        }
        let best_bid = self.bids[0].price;
        let mut filled_qty = dec!(0);
        let mut total_proceeds = dec!(0);

        for level in &self.bids {
            let available = level.size;
            let needed = target_qty - filled_qty;
            let fill = needed.min(available);
            filled_qty += fill;
            total_proceeds += fill * level.price;

            if filled_qty >= target_qty {
                break;
            }
        }

        if filled_qty == dec!(0) {
            return None;
        }

        let vwap = total_proceeds / filled_qty;
        let unfilled = target_qty - filled_qty;
        let slippage = if best_bid > dec!(0) {
            (best_bid - vwap) / best_bid
        } else {
            dec!(0)
        };

        Some((vwap, unfilled, slippage))
    }
}

/// Instantaneous price ticker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ticker {
    pub venue: VenueId,
    pub symbol: String,
    pub bid: Decimal,
    pub ask: Decimal,
    pub last: Decimal,
    pub volume_24h: Decimal,
    pub timestamp: DateTime<Utc>,
}

/// Funding rate for perpetual contracts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundingRate {
    pub venue: VenueId,
    pub symbol: String,
    /// Funding rate per period (e.g., 0.0001 = 0.01% per period)
    pub rate: Decimal,
    /// Predicted next funding rate
    pub predicted_rate: Option<Decimal>,
    /// Interval in hours (e.g. 1 hour on Hyperliquid, 8 hours on Binance/OKX)
    pub interval_hours: u32,
    pub next_funding_time: DateTime<Utc>,
    pub timestamp: DateTime<Utc>,
}

impl FundingRate {
    /// Annualized Percentage Rate (APR) based on linear periodic multiplication.
    pub fn apr(&self) -> Decimal {
        if self.interval_hours == 0 {
            return dec!(0);
        }
        let periods_per_day = dec!(24) / Decimal::from(self.interval_hours);
        let periods_per_year = periods_per_day * dec!(365);
        self.rate * periods_per_year
    }

    /// Annualized Percentage Yield (APY) based on periodic compounding.
    /// APY = (1 + r)^n - 1
    pub fn apy(&self) -> Decimal {
        if self.interval_hours == 0 {
            return dec!(0);
        }
        let periods_per_day = 24.0 / self.interval_hours as f64;
        let n = periods_per_day * 365.0;
        let r = self.rate.to_string().parse::<f64>().unwrap_or(0.0);
        let apy_f64 = (1.0 + r).powf(n) - 1.0;
        Decimal::from_str_exact(&format!("{:.8}", apy_f64))
            .unwrap_or_else(|_| Decimal::from_f64_retain(apy_f64).unwrap_or(dec!(0)))
    }
}

/// Mark price and index price for perpetuals / derivatives.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkPrice {
    pub venue: VenueId,
    pub symbol: String,
    pub mark_price: Decimal,
    pub index_price: Decimal,
    pub timestamp: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_book() -> OrderBook {
        let mut book = OrderBook::new(VenueId::new("hyperliquid"), "BTC-USDC".to_string());
        book.bids.push(BookLevel::new(dec!(100.0), dec!(1.0)));
        book.bids.push(BookLevel::new(dec!(99.5), dec!(2.0)));
        book.asks.push(BookLevel::new(dec!(100.5), dec!(1.0)));
        book.asks.push(BookLevel::new(dec!(101.0), dec!(2.0)));
        book
    }

    #[test]
    fn validate_accepts_valid_book() {
        assert!(valid_book().validate_for_execution().is_ok());
    }

    #[test]
    fn validate_rejects_empty_symbol() {
        let mut book = valid_book();
        book.symbol = "  ".to_string();
        assert!(book.validate_for_execution().unwrap_err().contains("symbol"));
    }

    #[test]
    fn validate_rejects_missing_side() {
        let mut book = valid_book();
        book.asks.clear();
        assert!(book.validate_for_execution().unwrap_err().contains("asks"));
        let mut book = valid_book();
        book.bids.clear();
        assert!(book.validate_for_execution().unwrap_err().contains("bids"));
    }

    #[test]
    fn validate_rejects_nonpositive_price_and_size() {
        let mut book = valid_book();
        book.asks[0].price = dec!(0);
        assert!(book.validate_for_execution().unwrap_err().contains("nonpositive ask price"));
        let mut book = valid_book();
        book.bids[1].size = dec!(-1);
        assert!(book.validate_for_execution().unwrap_err().contains("nonpositive bid size"));
    }

    #[test]
    fn validate_rejects_unsorted_levels() {
        let mut book = valid_book();
        book.bids.swap(0, 1);
        assert!(book.validate_for_execution().unwrap_err().contains("descending"));
        let mut book = valid_book();
        book.asks.swap(0, 1);
        assert!(book.validate_for_execution().unwrap_err().contains("ascending"));
    }

    #[test]
    fn validate_rejects_crossed_and_locked_book() {
        let mut book = valid_book();
        book.bids[0].price = dec!(101.0);
        assert!(book.validate_for_execution().unwrap_err().contains("crossed"));
        let mut book = valid_book();
        book.asks[0].price = dec!(100.0);
        assert!(book.validate_for_execution().unwrap_err().contains("crossed"));
    }
}
