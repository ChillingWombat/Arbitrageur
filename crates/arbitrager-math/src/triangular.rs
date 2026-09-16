use arbitrager_core::quote::OrderBook;
use arbitrager_core::types::{dec, Decimal, BPS_DIVISOR};

/// Which side of a triangular leg's book is consumed.
///
/// - `BuyBase`: spend quote currency, receive base (lift the ask).
/// - `SellBase`: spend base currency, receive quote (hit the bid).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriLegSide {
    BuyBase,
    SellBase,
}

/// Depth-aware evaluation of one 3-leg triangular cycle (A -> B -> C -> A).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TriangularCycleResult {
    /// Starting notional in the cycle's quote currency (e.g. USDC).
    pub start_notional: Decimal,
    /// Ending notional after chaining all three legs at VWAP, before fees.
    pub end_notional: Decimal,
    /// Gross cycle return in basis points: `(end / start - 1) * 10000`.
    pub gross_bps: Decimal,
    /// Additive per-leg taker fees, in basis points (route-economics style:
    /// same additive convention as `compute_route_economics`).
    pub total_fee_bps: Decimal,
    /// Additive per-leg VWAP-vs-top slippage, in basis points.
    pub total_slippage_bps: Decimal,
    /// `gross_bps - total_fee_bps - total_slippage_bps`.
    pub net_bps: Decimal,
    /// Expected dollar profit at the starting notional.
    pub estimated_profit: Decimal,
    /// Base-unit quantities to submit per leg (buy legs: base to buy;
    /// sell legs: base to sell). Buy legs are sized at VWAP so the modeled
    /// spend matches the chained amount — the dispatch path submits exactly
    /// these and requires full fills.
    pub order_qty: [Decimal; 3],
}

/// Evaluates a single-venue triangular cycle starting from `start_notional`
/// units of the cycle's quote currency, chaining proceeds leg to leg:
///
/// - `BuyBase` with spend `S`: `receive = S / ask_vwap`, sized at VWAP
///   so the modeled spend never exceeds `S`.
/// - `SellBase` with base quantity `Q`: `receive = Q * bid_vwap(Q)`.
///
/// Asset continuity (leg `i`'s receive asset == leg `i+1`'s spend asset, final
/// receive == start asset) is the CALLER's responsibility: books carry symbol
/// strings, not asset pairs, so this function prices whatever three books it
/// is given. The backend builds legs from parsed symbols and unit-tests pin
/// correct construction.
///
/// Fail-closed: returns `None` when any book is missing a side, any top or
/// VWAP price is nonpositive, depth cannot cover the chained quantity, or any
/// input is nonpositive. No I/O, no global state — deterministic in inputs.
pub fn evaluate_triangular_cycle(
    books: [&OrderBook; 3],
    sides: [TriLegSide; 3],
    fee_bps: [Decimal; 3],
    start_notional: Decimal,
) -> Option<TriangularCycleResult> {
    if start_notional <= dec!(0) {
        return None;
    }
    for fee in &fee_bps {
        if *fee < dec!(0) {
            return None;
        }
    }

    let mut amount = start_notional;
    let mut total_slippage_bps = dec!(0);
    let mut order_qty = [dec!(0); 3];

    for (idx, ((book, side), _fee)) in books
        .iter()
        .zip(sides.iter())
        .zip(fee_bps.iter())
        .enumerate()
    {
        // Reject crossed/locked books outright: a best bid at or above the
        // best ask cannot price either direction safely.
        if let (Some(bid), Some(ask)) = (book.best_bid_price(), book.best_ask_price()) {
            if bid >= ask {
                return None;
            }
        }
        match side {
            TriLegSide::BuyBase => {
                let top_ask = book.best_ask_price()?;
                if top_ask <= dec!(0) || amount <= dec!(0) {
                    return None;
                }
                let est_qty = amount / top_ask;
                if est_qty <= dec!(0) {
                    return None;
                }
                let (vwap_est, remaining_est, _) = book.buy_execution_vwap(est_qty)?;
                if remaining_est > dec!(0) || vwap_est <= dec!(0) {
                    return None;
                }
                // Size at the realized VWAP so modeled spend (qty * vwap)
                // equals the available amount: sizing at top_ask overspends
                // by vwap/top_ask on a non-flat book.
                let qty = amount / vwap_est;
                if qty <= dec!(0) {
                    return None;
                }
                let (vwap, remaining, _) = book.buy_execution_vwap(qty)?;
                if remaining > dec!(0) || vwap <= dec!(0) {
                    return None;
                }
                order_qty[idx] = qty;
                let slip = (vwap - top_ask) / top_ask * BPS_DIVISOR;
                if slip < dec!(0) {
                    return None;
                }
                total_slippage_bps += slip;
                amount /= vwap;
                if amount <= dec!(0) {
                    return None;
                }
            }
            TriLegSide::SellBase => {
                let top_bid = book.best_bid_price()?;
                if top_bid <= dec!(0) || amount <= dec!(0) {
                    return None;
                }
                let (vwap, remaining, _) = book.sell_execution_vwap(amount)?;
                if remaining > dec!(0) || vwap <= dec!(0) {
                    return None;
                }
                order_qty[idx] = amount;
                let slip = (top_bid - vwap) / top_bid * BPS_DIVISOR;
                if slip < dec!(0) {
                    return None;
                }
                total_slippage_bps += slip;
                amount *= vwap;
                if amount <= dec!(0) {
                    return None;
                }
            }
        }
    }

    let end_notional = amount;
    let gross_bps = (end_notional / start_notional - dec!(1)) * BPS_DIVISOR;
    let total_fee_bps = fee_bps[0] + fee_bps[1] + fee_bps[2];
    let net_bps = gross_bps - total_fee_bps - total_slippage_bps;
    let estimated_profit = net_bps / BPS_DIVISOR * start_notional;

    Some(TriangularCycleResult {
        start_notional,
        end_notional,
        gross_bps,
        total_fee_bps,
        total_slippage_bps,
        net_bps,
        estimated_profit,
        order_qty,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrager_core::quote::BookLevel;

    fn deep_book(venue: &str, symbol: &str, bid: Decimal, ask: Decimal) -> OrderBook {
        let mut book = OrderBook::new(venue.into(), symbol.to_string());
        for _ in 0..10 {
            book.bids.push(BookLevel::new(bid, dec!(1000.0)));
            book.asks.push(BookLevel::new(ask, dec!(1000.0)));
        }
        book
    }

    /// Profitable USDC -> BTC -> ETH -> USDC cycle:
    /// 10,000 USDC buys BTC @60,000, sells BTC for ETH @20/BTC,
    /// sells ETH @3,100 -> 10,333 USDC before fees.
    fn profitable_books() -> [OrderBook; 3] {
        [
            deep_book("lighter", "BTC-USDC", dec!(59990), dec!(60000)),
            deep_book("lighter", "ETH-BTC", dec!(20), dec!(20.01)),
            deep_book("lighter", "ETH-USDC", dec!(3100), dec!(3101)),
        ]
    }

    fn cycle_sides() -> [TriLegSide; 3] {
        [
            TriLegSide::BuyBase,
            TriLegSide::SellBase,
            TriLegSide::SellBase,
        ]
    }

    #[test]
    fn profitable_cycle_found_with_fee_accounting() {
        let books = profitable_books();
        let res = evaluate_triangular_cycle(
            [&books[0], &books[1], &books[2]],
            cycle_sides(),
            [dec!(2), dec!(2), dec!(2)],
            dec!(10000),
        )
        .expect("profitable cycle must evaluate");
        assert!(res.gross_bps > dec!(300), "gross {0}", res.gross_bps);
        assert_eq!(res.total_fee_bps, dec!(6));
        assert_eq!(res.total_slippage_bps, dec!(0));
        assert!(res.net_bps > dec!(300), "net {0}", res.net_bps);
        assert!(res.estimated_profit > dec!(300));
    }

    #[test]
    fn fee_eaten_cycle_is_negative() {
        let books = profitable_books();
        let res = evaluate_triangular_cycle(
            [&books[0], &books[1], &books[2]],
            cycle_sides(),
            [dec!(200), dec!(200), dec!(200)],
            dec!(10000),
        )
        .expect("evaluation still prices a fee-eaten cycle");
        assert!(res.net_bps < dec!(0), "net {0}", res.net_bps);
        assert!(res.estimated_profit < dec!(0));
    }

    #[test]
    fn missing_leg_fail_closed() {
        let books = profitable_books();
        // Empty ask side on leg 1.
        let mut no_asks = deep_book("lighter", "BTC-USDC", dec!(59990), dec!(60000));
        no_asks.asks.clear();
        assert!(evaluate_triangular_cycle(
            [&no_asks, &books[1], &books[2]],
            cycle_sides(),
            [dec!(2), dec!(2), dec!(2)],
            dec!(10000),
        )
        .is_none());
        // Empty bid side on leg 3.
        let mut no_bids = deep_book("lighter", "ETH-USDC", dec!(3100), dec!(3101));
        no_bids.bids.clear();
        assert!(evaluate_triangular_cycle(
            [&books[0], &books[1], &no_bids],
            cycle_sides(),
            [dec!(2), dec!(2), dec!(2)],
            dec!(10000),
        )
        .is_none());
        // Crossed book on the middle leg.
        let crossed = deep_book("lighter", "ETH-BTC", dec!(20.05), dec!(20.01));
        let _ = crossed.validate_for_execution().unwrap_err();
        assert!(evaluate_triangular_cycle(
            [&books[0], &crossed, &books[2]],
            cycle_sides(),
            [dec!(2), dec!(2), dec!(2)],
            dec!(10000),
        )
        .is_none());
    }

    #[test]
    fn thin_depth_fail_closed() {
        let mut thin = deep_book("lighter", "BTC-USDC", dec!(59990), dec!(60000));
        thin.asks.clear();
        thin.asks.push(BookLevel::new(dec!(60000), dec!(0.0001)));
        let books = profitable_books();
        assert!(evaluate_triangular_cycle(
            [&thin, &books[1], &books[2]],
            cycle_sides(),
            [dec!(2), dec!(2), dec!(2)],
            dec!(10000),
        )
        .is_none());
    }

    #[test]
    fn invalid_inputs_fail_closed() {
        let books = profitable_books();
        let refs = [&books[0], &books[1], &books[2]];
        let fees = [dec!(2), dec!(2), dec!(2)];
        assert!(evaluate_triangular_cycle(refs, cycle_sides(), fees, dec!(0)).is_none());
        assert!(evaluate_triangular_cycle(refs, cycle_sides(), fees, dec!(-5)).is_none());
        assert!(evaluate_triangular_cycle(
            refs,
            cycle_sides(),
            [dec!(2), dec!(-1), dec!(2)],
            dec!(10000),
        )
        .is_none());
    }

    #[test]
    fn evaluation_is_deterministic() {
        let books = profitable_books();
        let refs = [&books[0], &books[1], &books[2]];
        let fees = [dec!(2), dec!(2), dec!(2)];
        let a = evaluate_triangular_cycle(refs, cycle_sides(), fees, dec!(10000)).expect("a");
        let b = evaluate_triangular_cycle(refs, cycle_sides(), fees, dec!(10000)).expect("b");
        assert_eq!(a, b);
    }

    #[test]
    fn buy_leg_sized_at_vwap_never_overspends() {
        // Non-flat ask ladder: 1.0 @ 100, then depth @ 110.
        let mut leg = OrderBook::new("lighter".into(), "BTC-USDC".to_string());
        leg.bids.push(BookLevel::new(dec!(99), dec!(1000)));
        leg.asks.push(BookLevel::new(dec!(100), dec!(1)));
        leg.asks.push(BookLevel::new(dec!(110), dec!(1000)));
        let books = profitable_books();
        let start = dec!(1000);
        let res = evaluate_triangular_cycle(
            [&leg, &books[1], &books[2]],
            cycle_sides(),
            [dec!(0), dec!(0), dec!(0)],
            start,
        )
        .expect("non-flat book must evaluate");
        let qty = res.order_qty[0];
        // Old code submitted start/top_ask; VWAP sizing must be smaller.
        assert!(qty < start / dec!(100), "qty {qty} not sized at vwap");
        // Modeled spend at the quoted depth must not exceed the amount.
        let (vwap, remaining, _) = leg.buy_execution_vwap(qty).expect("depth");
        assert_eq!(remaining, dec!(0));
        assert!(qty * vwap <= start, "overspend: {qty} * {vwap} > {start}");
    }
}
