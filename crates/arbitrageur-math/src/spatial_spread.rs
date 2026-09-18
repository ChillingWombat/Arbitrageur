use arbitrageur_core::types::{dec, Decimal, BPS_DIVISOR};
use arbitrageur_core::quote::OrderBook;

/// Cross-market spatial spread calculation result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpatialOpportunity {
    /// Gross price spread = Bid(Sell Venue) - Ask(Buy Venue)
    pub gross_spread: Decimal,
    /// Gross spread in basis points relative to the buy price
    pub gross_spread_bps: Decimal,
    /// Total transaction fees paid across both venues
    pub total_fees: Decimal,
    /// Net profit per unit after deducting fees
    pub net_profit_per_unit: Decimal,
    /// Net profit in basis points
    pub net_profit_bps: Decimal,
    /// Target execution quantity executable immediately without exceeding slippage
    pub executable_qty: Decimal,
    /// Total expected dollar net profit
    pub total_net_profit_usd: Decimal,
}

/// Evaluates instantaneous spatial arbitrage between two orderbooks.
/// Leg 1: Buy on `buy_book` at ask price
/// Leg 2: Sell on `sell_book` at bid price
pub fn evaluate_spatial_spread(
    buy_book: &OrderBook,
    sell_book: &OrderBook,
    buy_fee_bps: Decimal,
    sell_fee_bps: Decimal,
    target_qty: Decimal,
) -> Option<SpatialOpportunity> {
    let best_ask = buy_book.best_ask_price()?;
    let best_bid = sell_book.best_bid_price()?;

    if best_ask <= dec!(0) || best_bid <= dec!(0) || target_qty <= dec!(0) {
        return None;
    }

    // Determine mutually executable depth across both books
    let available_buy_depth: Decimal = buy_book.asks.iter().map(|l| l.size).sum();
    let available_sell_depth: Decimal = sell_book.bids.iter().map(|l| l.size).sum();
    let executable_qty = target_qty.min(available_buy_depth).min(available_sell_depth);

    if executable_qty <= dec!(0) {
        return None;
    }

    // Calculate actual depth-weighted execution price (VWAP) for the exact matched quantity
    let (buy_vwap, _, _) = buy_book.buy_execution_vwap(executable_qty)?;
    let (sell_vwap, _, _) = sell_book.sell_execution_vwap(executable_qty)?;

    let gross_spread = sell_vwap - buy_vwap;
    let gross_spread_bps = (gross_spread / buy_vwap) * BPS_DIVISOR;

    let buy_fee = buy_vwap * (buy_fee_bps / BPS_DIVISOR);
    let sell_fee = sell_vwap * (sell_fee_bps / BPS_DIVISOR);
    let total_fees = buy_fee + sell_fee;

    let net_profit_per_unit = gross_spread - total_fees;
    let net_profit_bps = (net_profit_per_unit / buy_vwap) * BPS_DIVISOR;
    let total_net_profit_usd = net_profit_per_unit * executable_qty;

    Some(SpatialOpportunity {
        gross_spread,
        gross_spread_bps,
        total_fees,
        net_profit_per_unit,
        net_profit_bps,
        executable_qty,
        total_net_profit_usd,
    })
}

/// Computes the statistical baseline threshold for mean-reverting spread execution.
/// As validated in PandaZhai / Lighter arbitrage tool:
/// Threshold = Avg(Spread) + Price * Target_BPS / 10000 - Point(Avg_Short + Avg_Long)
pub fn compute_baseline_threshold(
    baseline_avg_spread: Decimal,
    reference_price: Decimal,
    target_profit_bps: Decimal,
    spread_baseline_sum: Decimal,
) -> Decimal {
    let profit_margin = reference_price * (target_profit_bps / BPS_DIVISOR);
    baseline_avg_spread + profit_margin - spread_baseline_sum
}
