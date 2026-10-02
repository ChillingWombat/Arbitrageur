use arbitrageur_core::quote::OrderBook;
use arbitrageur_core::types::{dec, Decimal, Side, VenueId, BPS_DIVISOR};
use std::collections::HashMap;

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

    // Determine mutually executable depth across both books using early-terminating bounded depth walk
    let available_buy_depth = buy_book.cumulative_depth_up_to(Side::Buy, target_qty);
    let available_sell_depth = sell_book.cumulative_depth_up_to(Side::Sell, target_qty);
    let executable_qty = available_buy_depth.min(available_sell_depth);

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

/// An evaluated candidate pair crossing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpatialCrossing {
    pub buy_venue: VenueId,
    pub sell_venue: VenueId,
    pub opportunity: SpatialOpportunity,
}

/// Evaluates spatial arbitrage across multiple venues using sorted best-ask and best-bid
/// indexed lookups to prune impossible pairs early (O(V log V + K) vs naive O(V²)).
pub fn find_best_spatial_crossings(
    books: &[(VenueId, &OrderBook)],
    fee_map: &HashMap<VenueId, Decimal>,
    target_qty: Decimal,
) -> Vec<SpatialCrossing> {
    if books.len() < 2 || target_qty <= dec!(0) {
        return Vec::new();
    }

    // Filter books with valid best ask
    let mut ask_venues: Vec<(VenueId, Decimal, &OrderBook, Decimal)> = books
        .iter()
        .filter_map(|(v, b)| {
            let ask = b.best_ask_price()?;
            if ask <= dec!(0) {
                return None;
            }
            let fee = fee_map.get(v).copied().unwrap_or(dec!(0));
            Some((v.clone(), ask, *b, fee))
        })
        .collect();

    // Filter books with valid best bid
    let mut bid_venues: Vec<(VenueId, Decimal, &OrderBook, Decimal)> = books
        .iter()
        .filter_map(|(v, b)| {
            let bid = b.best_bid_price()?;
            if bid <= dec!(0) {
                return None;
            }
            let fee = fee_map.get(v).copied().unwrap_or(dec!(0));
            Some((v.clone(), bid, *b, fee))
        })
        .collect();

    // Sort asks ascending (cheapest ask first)
    ask_venues.sort_by_key(|a| a.1);
    // Sort bids descending (highest bid first)
    bid_venues.sort_by_key(|b| std::cmp::Reverse(b.1));

    let mut crossings = Vec::new();

    // Pruned search: for each ask, check bids from highest down.
    // Once best bid <= best ask, all subsequent bids are also <= best ask,
    // so we can break the inner loop early!
    for (buy_vid, best_ask, buy_book, buy_fee) in &ask_venues {
        for (sell_vid, best_bid, sell_book, sell_fee) in &bid_venues {
            if buy_vid == sell_vid {
                continue;
            }
            // Early pruning bound: if raw top-of-book bid is already <= ask,
            // depth-weighted VWAP cannot possibly produce a positive gross spread.
            if *best_bid <= *best_ask {
                break;
            }

            if let Some(opp) =
                evaluate_spatial_spread(buy_book, sell_book, *buy_fee, *sell_fee, target_qty)
            {
                if opp.net_profit_bps > dec!(0) {
                    crossings.push(SpatialCrossing {
                        buy_venue: buy_vid.clone(),
                        sell_venue: sell_vid.clone(),
                        opportunity: opp,
                    });
                }
            }
        }
    }

    crossings.sort_by(|a, b| {
        b.opportunity
            .net_profit_bps
            .cmp(&a.opportunity.net_profit_bps)
    });
    crossings
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

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrageur_core::quote::BookLevel;

    fn make_book(venue: &str, bid: Decimal, ask: Decimal) -> OrderBook {
        let mut b = OrderBook::new(VenueId(venue.to_string()), "BTC-USDC".to_string());
        b.bids.push(BookLevel::new(bid, dec!(10.0)));
        b.asks.push(BookLevel::new(ask, dec!(10.0)));
        b
    }

    #[test]
    fn test_find_best_spatial_crossings_pruning() {
        let b1 = make_book("v1", dec!(100), dec!(101));
        let b2 = make_book("v2", dec!(103), dec!(104));
        let b3 = make_book("v3", dec!(98), dec!(99));

        let venues = [
            (VenueId("v1".into()), &b1),
            (VenueId("v2".into()), &b2),
            (VenueId("v3".into()), &b3),
        ];

        let fee_map = HashMap::new();
        let crossings = find_best_spatial_crossings(&venues, &fee_map, dec!(1.0));

        // Crossings should find buying v3 (ask 99) and selling v2 (bid 103),
        // or buying v1 (ask 101) and selling v2 (bid 103)
        assert!(!crossings.is_empty());
        let top = &crossings[0];
        assert_eq!(top.buy_venue.0, "v3");
        assert_eq!(top.sell_venue.0, "v2");
        assert!(top.opportunity.gross_spread > dec!(0));
    }
}
