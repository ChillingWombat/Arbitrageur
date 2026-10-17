use crate::types::{dec, Decimal, Side, VenueId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
    /// Venue-published model mark (e.g. Deribit `mark_price`), when the
    /// venue reports one — *valuation only*. Never executable truth: the
    /// execution path reads bids/asks and rejects one-sided books, while
    /// position valuation may legitimately fall back to the venue mark
    /// for instruments whose book has no real mid (e.g. a bids-empty
    /// far-out option).
    #[serde(default)]
    pub mark_price: Option<Decimal>,
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
            mark_price: None,
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

    /// Calculate top-of-book Stoikov Micro-Price:
    /// P_{micro} = (Q_b * P_a + Q_a * P_b) / (Q_b + Q_a)
    /// Returns None if either side is empty or total top-of-book size is zero.
    pub fn micro_price(&self) -> Option<Decimal> {
        let best_bid = self.best_bid()?;
        let best_ask = self.best_ask()?;
        let total_size = best_bid.size + best_ask.size;
        if total_size <= dec!(0) {
            return None;
        }
        Some((best_bid.size * best_ask.price + best_ask.size * best_bid.price) / total_size)
    }

    /// Calculate multi-level order book imbalance across up to `max_levels` levels,
    /// with exponential depth decay weight w_l = exp(-decay_eta * l):
    /// I_K = \sum_l w_l (Q_b^l - Q_a^l) / \sum_l w_l (Q_b^l + Q_a^l) \in [-1.0, 1.0]
    pub fn multi_level_imbalance(&self, max_levels: usize, decay_eta: f64) -> Option<f64> {
        if self.bids.is_empty() || self.asks.is_empty() || max_levels == 0 {
            return None;
        }
        let k = max_levels.min(self.bids.len()).min(self.asks.len());
        let eta = decay_eta.max(0.0);
        let mut weighted_diff = 0.0;
        let mut weighted_total = 0.0;

        for l in 0..k {
            let w = (-eta * l as f64).exp();
            let q_b = self.bids[l].size.to_string().parse::<f64>().unwrap_or(0.0);
            let q_a = self.asks[l].size.to_string().parse::<f64>().unwrap_or(0.0);
            weighted_diff += w * (q_b - q_a);
            weighted_total += w * (q_b + q_a);
        }

        if weighted_total <= 1e-12 {
            return None;
        }
        Some((weighted_diff / weighted_total).clamp(-1.0, 1.0))
    }

    /// Calculate multi-level Stoikov-Cartea micro-price using multi-level order book imbalance:
    /// P_{micro}^{(K)} = P_{mid} + I_K * (Spread / 2)
    pub fn multi_level_micro_price(&self, max_levels: usize, decay_eta: f64) -> Option<Decimal> {
        let mid = self.mid_price()?;
        let spread = self.spread()?;
        let imb = self.multi_level_imbalance(max_levels, decay_eta)?;
        let imb_dec = Decimal::from_str_exact(&format!("{:.6}", imb))
            .ok()
            .or_else(|| Decimal::from_f64_retain(imb))?;
        Some(mid + imb_dec * (spread / dec!(2)))
    }

    /// Calculate dynamic multi-level Order Flow Imbalance (OFI) delta relative to a previous book state,
    /// implementing Cont, Kukanov & Stoikov (2014) event-driven order flow across depth levels:
    /// e_b^l = Q_b,t^l (if P_b,t > P_b,t-1) | Q_b,t^l - Q_b,t-1^l (if ==) | -Q_b,t-1^l (if <)
    /// e_a^l = -Q_a,t^l (if P_a,t < P_a,t-1) | -(Q_a,t^l - Q_a,t-1^l) (if ==) | Q_a,t-1^l (if >)
    /// OFI_multi = \sum_{l=0}^{k-1} exp(-decay_lambda * l) * (e_b^l + e_a^l)
    pub fn compute_ofi_delta(&self, prev: &OrderBook, max_levels: usize, decay_lambda: f64) -> f64 {
        if max_levels == 0 || self.bids.is_empty() || self.asks.is_empty() {
            return 0.0;
        }
        let k = max_levels
            .min(self.bids.len())
            .min(self.asks.len())
            .min(prev.bids.len())
            .min(prev.asks.len());
        if k == 0 {
            return 0.0;
        }

        let lambda = decay_lambda.max(0.0);
        let mut total_ofi = 0.0;

        for l in 0..k {
            let w = (-lambda * l as f64).exp();

            let curr_b_px = self.bids[l].price.to_string().parse::<f64>().unwrap_or(0.0);
            let prev_b_px = prev.bids[l].price.to_string().parse::<f64>().unwrap_or(0.0);
            let curr_b_sz = self.bids[l].size.to_string().parse::<f64>().unwrap_or(0.0);
            let prev_b_sz = prev.bids[l].size.to_string().parse::<f64>().unwrap_or(0.0);

            let e_b = if curr_b_px > prev_b_px {
                curr_b_sz
            } else if (curr_b_px - prev_b_px).abs() < 1e-9 {
                curr_b_sz - prev_b_sz
            } else {
                -prev_b_sz
            };

            let curr_a_px = self.asks[l].price.to_string().parse::<f64>().unwrap_or(0.0);
            let prev_a_px = prev.asks[l].price.to_string().parse::<f64>().unwrap_or(0.0);
            let curr_a_sz = self.asks[l].size.to_string().parse::<f64>().unwrap_or(0.0);
            let prev_a_sz = prev.asks[l].size.to_string().parse::<f64>().unwrap_or(0.0);

            let e_a = if curr_a_px < prev_a_px {
                -curr_a_sz
            } else if (curr_a_px - prev_a_px).abs() < 1e-9 {
                -(curr_a_sz - prev_a_sz)
            } else {
                prev_a_sz
            };

            total_ofi += w * (e_b + e_a);
        }

        total_ofi
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

    /// Rejects books that cannot safely drive execution on the specified `side`:
    /// empty symbol, missing bids (for Sell) or asks (for Buy), nonpositive
    /// prices or sizes, unsorted levels, or a crossed/locked book if both sides exist.
    pub fn validate_for_side(&self, side: Side) -> Result<(), String> {
        if self.symbol.trim().is_empty() {
            return Err("order book symbol is empty".to_string());
        }
        match side {
            Side::Buy => {
                if self.asks.is_empty() {
                    return Err(format!("order book {} has no asks", self.symbol));
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
                for pair in self.asks.windows(2) {
                    if pair[0].price > pair[1].price {
                        return Err(format!(
                            "order book {} asks are not monotonically ascending",
                            self.symbol
                        ));
                    }
                }
            }
            Side::Sell => {
                if self.bids.is_empty() {
                    return Err(format!("order book {} has no bids", self.symbol));
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
                for pair in self.bids.windows(2) {
                    if pair[0].price < pair[1].price {
                        return Err(format!(
                            "order book {} bids are not monotonically descending",
                            self.symbol
                        ));
                    }
                }
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

    /// Calculate the marginal (worst) ask price required to execute `target_qty`.
    pub fn buy_execution_marginal_price(&self, target_qty: Decimal) -> Option<Decimal> {
        if self.asks.is_empty() || target_qty <= dec!(0) {
            return None;
        }
        let mut filled_qty = dec!(0);
        let mut marginal_price = self.asks[0].price;

        for level in &self.asks {
            let available = level.size;
            let needed = target_qty - filled_qty;
            let fill = needed.min(available);
            filled_qty += fill;
            marginal_price = level.price;

            if filled_qty >= target_qty {
                return Some(marginal_price);
            }
        }
        if filled_qty > dec!(0) {
            Some(marginal_price)
        } else {
            None
        }
    }

    /// Calculate the marginal (worst) bid price required to execute `target_qty`.
    pub fn sell_execution_marginal_price(&self, target_qty: Decimal) -> Option<Decimal> {
        if self.bids.is_empty() || target_qty <= dec!(0) {
            return None;
        }
        let mut filled_qty = dec!(0);
        let mut marginal_price = self.bids[0].price;

        for level in &self.bids {
            let available = level.size;
            let needed = target_qty - filled_qty;
            let fill = needed.min(available);
            filled_qty += fill;
            marginal_price = level.price;

            if filled_qty >= target_qty {
                return Some(marginal_price);
            }
        }
        if filled_qty > dec!(0) {
            Some(marginal_price)
        } else {
            None
        }
    }

    /// Calculate cumulative volume available up to `max_qty`, breaking early as soon as reached.
    /// Operates in O(k) where k <= N is the number of levels needed to fulfill `max_qty`.
    pub fn cumulative_depth_up_to(&self, side: Side, max_qty: Decimal) -> Decimal {
        if max_qty <= dec!(0) {
            return dec!(0);
        }
        let levels = match side {
            Side::Buy => &self.asks,
            Side::Sell => &self.bids,
        };
        let mut depth = dec!(0);
        for l in levels {
            if l.size <= dec!(0) {
                continue;
            }
            depth += l.size;
            if depth >= max_qty {
                return max_qty;
            }
        }
        depth
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
        assert!(book
            .validate_for_execution()
            .unwrap_err()
            .contains("symbol"));
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
        assert!(book
            .validate_for_execution()
            .unwrap_err()
            .contains("nonpositive ask price"));
        let mut book = valid_book();
        book.bids[1].size = dec!(-1);
        assert!(book
            .validate_for_execution()
            .unwrap_err()
            .contains("nonpositive bid size"));
    }

    #[test]
    fn validate_rejects_unsorted_levels() {
        let mut book = valid_book();
        book.bids.swap(0, 1);
        assert!(book
            .validate_for_execution()
            .unwrap_err()
            .contains("descending"));
        let mut book = valid_book();
        book.asks.swap(0, 1);
        assert!(book
            .validate_for_execution()
            .unwrap_err()
            .contains("ascending"));
    }

    #[test]
    fn validate_rejects_crossed_and_locked_book() {
        let mut book = valid_book();
        book.bids[0].price = dec!(101.0);
        assert!(book
            .validate_for_execution()
            .unwrap_err()
            .contains("crossed"));
        let mut book = valid_book();
        book.asks[0].price = dec!(100.0);
        assert!(book
            .validate_for_execution()
            .unwrap_err()
            .contains("crossed"));
    }

    #[test]
    fn validate_for_side_accepts_one_sided_book() {
        let mut buy_only = OrderBook::new(VenueId::new("deribit"), "BTC-OPT".to_string());
        buy_only.asks.push(BookLevel::new(dec!(10.0), dec!(1.0)));
        assert!(buy_only.validate_for_side(Side::Buy).is_ok());
        assert!(buy_only.validate_for_side(Side::Sell).is_err());

        let mut sell_only = OrderBook::new(VenueId::new("deribit"), "BTC-OPT".to_string());
        sell_only.bids.push(BookLevel::new(dec!(10.0), dec!(1.0)));
        assert!(sell_only.validate_for_side(Side::Sell).is_ok());
        assert!(sell_only.validate_for_side(Side::Buy).is_err());
    }

    #[test]
    fn test_order_book_micro_price_and_multi_level_imbalance() {
        let mut book = OrderBook::new(VenueId::new("hyperliquid"), "BTC-USDC".to_string());
        // Bid: 100.0 (size 3.0), Ask: 102.0 (size 1.0)
        // Mid: 101.0, Spread: 2.0
        book.bids.push(BookLevel::new(dec!(100.0), dec!(3.0)));
        book.bids.push(BookLevel::new(dec!(99.0), dec!(5.0)));
        book.asks.push(BookLevel::new(dec!(102.0), dec!(1.0)));
        book.asks.push(BookLevel::new(dec!(103.0), dec!(2.0)));

        // Top of book micro-price:
        // (3.0 * 102.0 + 1.0 * 100.0) / 4.0 = (306 + 100) / 4 = 406 / 4 = 101.5
        let micro = book.micro_price().unwrap();
        assert_eq!(micro, dec!(101.5));

        // Top level imbalance: (3 - 1) / (3 + 1) = 0.5
        let imb_l1 = book.multi_level_imbalance(1, 0.5).unwrap();
        assert!((imb_l1 - 0.5).abs() < 1e-6);

        // Multi-level micro price should shift toward the heavy bid side (higher price)
        let multi_micro = book.multi_level_micro_price(2, 0.5).unwrap();
        assert!(multi_micro > dec!(101.0));
    }

    #[test]
    fn test_compute_ofi_delta() {
        let mut prev = OrderBook::new(VenueId::new("hyperliquid"), "BTC-USDC".to_string());
        prev.bids.push(BookLevel::new(dec!(100.0), dec!(2.0)));
        prev.asks.push(BookLevel::new(dec!(102.0), dec!(2.0)));

        let mut curr = OrderBook::new(VenueId::new("hyperliquid"), "BTC-USDC".to_string());
        // Bid price improved to 100.5 with size 3.0 -> buying pressure (e_b = +3.0)
        curr.bids.push(BookLevel::new(dec!(100.5), dec!(3.0)));
        // Ask price unchanged at 102.0 but size reduced to 1.0 -> buying pressure (e_a = -(1.0 - 2.0) = +1.0)
        curr.asks.push(BookLevel::new(dec!(102.0), dec!(1.0)));

        let ofi = curr.compute_ofi_delta(&prev, 1, 0.5);
        // Total ofi should be 3.0 + 1.0 = 4.0
        assert!((ofi - 4.0).abs() < 1e-6);
    }
}
