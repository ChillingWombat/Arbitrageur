use arbitrageur_core::quote::OrderBook;
use arbitrageur_core::types::{dec, Decimal, BPS_DIVISOR};

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
    // `gross_bps` is already VWAP-inclusive: each leg converted through its
    // realized execution price, so depth slippage is inside the ratio.
    // `total_slippage_bps` stays reported for display/telemetry but must
    // NOT be subtracted again — doing so double-charges every leg's walk.
    let gross_bps = (end_notional / start_notional - dec!(1)) * BPS_DIVISOR;
    let total_fee_bps = fee_bps[0] + fee_bps[1] + fee_bps[2];
    let net_bps = gross_bps - total_fee_bps;
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

use rust_decimal::prelude::ToPrimitive;
use std::collections::{HashMap, HashSet};

/// Directed edge in the currency conversion graph.
#[derive(Debug, Clone, PartialEq)]
pub struct TriangularCycleEdge {
    pub from: String,
    pub to: String,
    pub from_idx: usize,
    pub to_idx: usize,
    pub symbol: String,
    pub side: TriLegSide,
    /// Rate received per unit of `from` asset: amount_to = amount_from * rate
    pub rate: f64,
    /// Taker fee in basis points
    pub fee_bps: f64,
    /// Negative log return weight: -ln(rate * (1 - fee))
    pub weight: f64,
}

/// Candidate triangular cycle discovered by graph search.
#[derive(Debug, Clone, PartialEq)]
pub struct TriangularCycleCandidate {
    pub path: [String; 3],
    pub symbols: [String; 3],
    pub sides: [TriLegSide; 3],
    pub fee_bps: [f64; 3],
    pub expected_gross_bps: f64,
    pub expected_net_bps: f64,
}

/// Graph of currencies and exchange edges for cycle detection.
#[derive(Debug, Default, Clone)]
pub struct CurrencyGraph {
    /// Currency node names
    pub nodes: Vec<String>,
    node_to_idx: HashMap<String, usize>,
    /// Adjacency list: node_idx -> list of outgoing edges
    pub edges: Vec<Vec<TriangularCycleEdge>>,
}

impl CurrencyGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_or_insert_node(&mut self, currency: &str) -> usize {
        if let Some(&idx) = self.node_to_idx.get(currency) {
            idx
        } else {
            let idx = self.nodes.len();
            self.nodes.push(currency.to_string());
            self.node_to_idx.insert(currency.to_string(), idx);
            self.edges.push(Vec::new());
            idx
        }
    }

    /// Adds a pair book's executable edges (both BuyBase and SellBase directions).
    pub fn add_orderbook_edges(
        &mut self,
        base: &str,
        quote: &str,
        symbol: &str,
        book: &OrderBook,
        fee_bps: f64,
    ) {
        let (Some(ask), Some(bid)) = (book.best_ask_price(), book.best_bid_price()) else {
            return;
        };
        let ask_f = ask.to_f64().unwrap_or(0.0);
        let bid_f = bid.to_f64().unwrap_or(0.0);
        if ask_f <= 0.0 || bid_f <= 0.0 || ask <= bid {
            return;
        }

        let fee_rate = (fee_bps / 10000.0).clamp(0.0, 1.0);

        // Direction 1: Spend Quote -> Receive Base (lift ask)
        // receive = spend / ask
        let rate_quote_to_base = 1.0 / ask_f;
        let eff_rate_q2b = rate_quote_to_base * (1.0 - fee_rate);
        if eff_rate_q2b > 0.0 {
            let u = self.get_or_insert_node(quote);
            let v = self.get_or_insert_node(base);
            self.edges[u].push(TriangularCycleEdge {
                from: quote.to_string(),
                to: base.to_string(),
                from_idx: u,
                to_idx: v,
                symbol: symbol.to_string(),
                side: TriLegSide::BuyBase,
                rate: rate_quote_to_base,
                fee_bps,
                weight: -eff_rate_q2b.ln(),
            });
        }

        // Direction 2: Spend Base -> Receive Quote (hit bid)
        // receive = spend * bid
        let rate_base_to_quote = bid_f;
        let eff_rate_b2q = rate_base_to_quote * (1.0 - fee_rate);
        if eff_rate_b2q > 0.0 {
            let u = self.get_or_insert_node(base);
            let v = self.get_or_insert_node(quote);
            self.edges[u].push(TriangularCycleEdge {
                from: base.to_string(),
                to: quote.to_string(),
                from_idx: u,
                to_idx: v,
                symbol: symbol.to_string(),
                side: TriLegSide::SellBase,
                rate: rate_base_to_quote,
                fee_bps,
                weight: -eff_rate_b2q.ln(),
            });
        }
    }

    /// Finds 3-cycle arbitrage opportunities starting and ending at `start_currency`.
    /// Operates via adjacency graph search in O(deg(start) * deg(neighbor)), avoiding O(V³) scans.
    pub fn find_triangular_cycles(&self, start_currency: &str) -> Vec<TriangularCycleCandidate> {
        let Some(&start_idx) = self.node_to_idx.get(start_currency) else {
            return Vec::new();
        };

        let mut candidates = Vec::new();

        // 1st hop: start -> A
        for edge1 in &self.edges[start_idx] {
            let a_idx = edge1.to_idx;
            if a_idx == start_idx {
                continue;
            }

            // 2nd hop: A -> B
            for edge2 in &self.edges[a_idx] {
                let b_idx = edge2.to_idx;
                if b_idx == start_idx || b_idx == a_idx {
                    continue;
                }

                // 3rd hop: B -> start
                for edge3 in &self.edges[b_idx] {
                    if edge3.to_idx != start_idx {
                        continue;
                    }

                    let total_weight = edge1.weight + edge2.weight + edge3.weight;
                    let gross_mult = edge1.rate * edge2.rate * edge3.rate;
                    let gross_bps = (gross_mult - 1.0) * 10000.0;
                    let net_bps = (-total_weight).exp_m1() * 10000.0;

                    candidates.push(TriangularCycleCandidate {
                        path: [edge1.from.clone(), edge2.from.clone(), edge3.from.clone()],
                        symbols: [
                            edge1.symbol.clone(),
                            edge2.symbol.clone(),
                            edge3.symbol.clone(),
                        ],
                        sides: [edge1.side, edge2.side, edge3.side],
                        fee_bps: [edge1.fee_bps, edge2.fee_bps, edge3.fee_bps],
                        expected_gross_bps: gross_bps,
                        expected_net_bps: net_bps,
                    });
                }
            }
        }

        candidates.sort_by(|a, b| b.expected_net_bps.total_cmp(&a.expected_net_bps));
        candidates
    }

    /// Detects negative weight cycles using the Bellman-Ford algorithm in O(V · E).
    /// Returns any profitable arbitrage cycle sequence of currencies.
    pub fn find_negative_cycles_bellman_ford(&self) -> Vec<Vec<String>> {
        let n = self.nodes.len();
        if n < 3 {
            return Vec::new();
        }

        let mut dist = vec![0.0; n];
        let mut pred: Vec<Option<usize>> = vec![None; n];

        // Relax edges n-1 times
        for _ in 0..n - 1 {
            let mut updated = false;
            for u in 0..n {
                for edge in &self.edges[u] {
                    let v = edge.to_idx;
                    if dist[u] + edge.weight < dist[v] - 1e-12 {
                        dist[v] = dist[u] + edge.weight;
                        pred[v] = Some(u);
                        updated = true;
                    }
                }
            }
            if !updated {
                return Vec::new();
            }
        }

        // Look for negative cycle on the n-th pass
        let mut cycles = Vec::new();
        let mut seen_cycles = HashSet::new();

        for u in 0..n {
            for edge in &self.edges[u] {
                let v = edge.to_idx;
                if dist[u] + edge.weight < dist[v] - 1e-12 {
                    let mut curr = v;
                    for _ in 0..n {
                        if let Some(p) = pred[curr] {
                            curr = p;
                        }
                    }

                    // Extract cycle with visited guard against infinite loops
                    let mut visited = vec![false; n];
                    let mut cycle_indices = Vec::new();
                    let mut step = curr;
                    while !visited[step] {
                        visited[step] = true;
                        cycle_indices.push(step);
                        if let Some(p) = pred[step] {
                            step = p;
                        } else {
                            break;
                        }
                    }

                    // step is where the loop closed
                    if let Some(start_pos) = cycle_indices.iter().position(|&x| x == step) {
                        let mut cycle_nodes: Vec<String> = cycle_indices[start_pos..]
                            .iter()
                            .map(|&idx| self.nodes[idx].clone())
                            .collect();
                        cycle_nodes.reverse();

                        if cycle_nodes.len() >= 3 {
                            // Canonicalize cycle representation to deduplicate cyclic shifts
                            let min_pos = cycle_nodes
                                .iter()
                                .enumerate()
                                .min_by(|(_, a), (_, b)| a.cmp(b))
                                .map(|(idx, _)| idx)
                                .unwrap_or(0);
                            cycle_nodes.rotate_left(min_pos);

                            let key = cycle_nodes.join("->");
                            if seen_cycles.insert(key) {
                                cycles.push(cycle_nodes);
                            }
                        }
                    }
                }
            }
        }

        cycles
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrageur_core::quote::BookLevel;

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

    #[test]
    fn test_currency_graph_triangular_cycles() {
        let books = profitable_books();
        let mut graph = CurrencyGraph::new();
        // BTC-USDC (base: BTC, quote: USDC)
        graph.add_orderbook_edges("BTC", "USDC", "BTC-USDC", &books[0], 2.0);
        // ETH-BTC (base: ETH, quote: BTC)
        graph.add_orderbook_edges("ETH", "BTC", "ETH-BTC", &books[1], 2.0);
        // ETH-USDC (base: ETH, quote: USDC)
        graph.add_orderbook_edges("ETH", "USDC", "ETH-USDC", &books[2], 2.0);

        let cycles = graph.find_triangular_cycles("USDC");
        assert!(
            !cycles.is_empty(),
            "Triangular cycle must be found via graph"
        );
        let top = &cycles[0];
        assert_eq!(top.path, ["USDC", "ETH", "BTC"]);
        assert!(top.expected_net_bps > 0.0);
    }

    #[test]
    fn test_currency_graph_bellman_ford() {
        let books = profitable_books();
        let mut graph = CurrencyGraph::new();
        graph.add_orderbook_edges("BTC", "USDC", "BTC-USDC", &books[0], 2.0);
        graph.add_orderbook_edges("ETH", "BTC", "ETH-BTC", &books[1], 2.0);
        graph.add_orderbook_edges("ETH", "USDC", "ETH-USDC", &books[2], 2.0);

        let neg_cycles = graph.find_negative_cycles_bellman_ford();
        assert!(
            !neg_cycles.is_empty(),
            "Bellman-Ford must detect negative cycle"
        );
        let cycle = &neg_cycles[0];
        assert!(cycle.contains(&"USDC".to_string()));
        assert!(cycle.contains(&"BTC".to_string()));
        assert!(cycle.contains(&"ETH".to_string()));
    }
}
