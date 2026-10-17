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

/// Order Book Fragility Index for triangular legs:
/// F_i = D_i(Q) / (sigma_i * S_i)
/// - D_i(Q): book depth up to target volume or total available depth
/// - sigma_i: tick volatility in bps
/// - S_i: top-of-book bid-ask spread in bps
pub fn compute_book_fragility(
    book: &OrderBook,
    side: TriLegSide,
    target_qty: Decimal,
    tick_vol_bps: f64,
) -> f64 {
    let spread_bps = match (book.best_bid_price(), book.best_ask_price()) {
        (Some(b), Some(a)) if a > b && b > dec!(0) => {
            let mid = (a + b) / dec!(2);
            let s = ((a - b) / mid) * BPS_DIVISOR;
            s.to_string().parse::<f64>().unwrap_or(1.0).max(0.1)
        }
        _ => 50.0,
    };
    let depth: Decimal = match side {
        TriLegSide::BuyBase => book.asks.iter().map(|l| l.size).sum(),
        TriLegSide::SellBase => book.bids.iter().map(|l| l.size).sum(),
    };
    let depth_f = depth.to_string().parse::<f64>().unwrap_or(0.0).max(1e-6);
    let target_f = target_qty
        .to_string()
        .parse::<f64>()
        .unwrap_or(1.0)
        .max(1e-6);
    let depth_ratio = (depth_f / target_f).max(1e-4);
    let vol_f = tick_vol_bps.max(0.1);

    // F_i = Depth / (Volatility * Spread)
    depth_ratio / (vol_f * spread_bps)
}

/// Fragility-First Triangular Sequencing Theorem:
/// Returns the indices [0, 1, 2] sorted in order of lowest fragility index
/// (most fragile / shallowest bottleneck leg first) to highest fragility (most liquid last).
/// Committing the most fragile leg first ensures that leg failure occurs at t=0 when capital
/// loss is zero, and subsequent legs are deep and guaranteed to fill.
pub fn rank_legs_by_fragility(
    books: [&OrderBook; 3],
    sides: [TriLegSide; 3],
    target_qtys: [Decimal; 3],
    tick_vols_bps: [f64; 3],
) -> [usize; 3] {
    let mut fragilities = [
        (
            0,
            compute_book_fragility(books[0], sides[0], target_qtys[0], tick_vols_bps[0]),
        ),
        (
            1,
            compute_book_fragility(books[1], sides[1], target_qtys[1], tick_vols_bps[1]),
        ),
        (
            2,
            compute_book_fragility(books[2], sides[2], target_qtys[2], tick_vols_bps[2]),
        ),
    ];
    // Sort ascending by fragility (bottleneck first)
    fragilities.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    [fragilities[0].0, fragilities[1].0, fragilities[2].0]
}

/// Evaluates expected Maker-Taker-Taker (MTT) net profit in basis points.
/// Leg 1 is posted as a passive maker limit order (earning maker rebate or paying maker fee),
/// while Legs 2 and 3 are executed as aggressive taker IOCs.
/// MTT compresses fees from 3 taker fees down to 1 maker + 2 takers.
pub fn compute_mtt_net_edge(
    gross_bps: Decimal,
    leg1_maker_fee_bps: Decimal,
    leg2_taker_fee_bps: Decimal,
    leg3_taker_fee_bps: Decimal,
    p_fill: f64,
    slippage_legs23_bps: Decimal,
) -> Decimal {
    let mtt_fees = leg1_maker_fee_bps + leg2_taker_fee_bps + leg3_taker_fee_bps;
    let net_fill_bps = gross_bps - mtt_fees - slippage_legs23_bps;
    let p_fill_dec = Decimal::from_str_exact(&format!("{:.4}", p_fill))
        .ok()
        .or_else(|| Decimal::from_f64_retain(p_fill))
        .unwrap_or(dec!(0.65));
    (net_fill_bps * p_fill_dec).round_dp(4)
}

/// Closed-form optimal capital sizing for triangular cycles under non-linear orderbook depth:
/// S* = (R_gross - 1) / (2 * Lambda_total)
/// where Lambda_total = sum_{k=1}^3 1 / (2 * Depth_k_usd).
/// Maximum net profit at optimal capital:
/// Delta Pi* = (R_gross - 1)^2 / (4 * Lambda_total)
pub fn compute_optimal_triangular_capital(
    gross_ratio: f64,
    total_fee_rate: f64,
    depths_usd: [f64; 3],
) -> Option<(f64, f64)> {
    let net_ratio = gross_ratio * (1.0 - total_fee_rate);
    if net_ratio <= 1.0 || !net_ratio.is_finite() {
        return None;
    }
    let excess_edge = net_ratio - 1.0;

    let mut lambda_total = 0.0;
    for &d in &depths_usd {
        if d <= 0.0 || !d.is_finite() {
            return None;
        }
        lambda_total += 1.0 / (2.0 * d);
    }

    if lambda_total <= 0.0 || !lambda_total.is_finite() {
        return None;
    }

    let s_star = excess_edge / (2.0 * lambda_total);
    let max_profit = (excess_edge * excess_edge) / (4.0 * lambda_total);
    Some((s_star, max_profit))
}

/// Partial fill recovery action for triangular cycle legs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriangularRecoveryAction {
    /// Immediate market inversion (IOC reversal of filled inventory)
    ImmediateMarketInversion,
    /// Synthetic hedge lock via correlated futures/perpetual contract
    SyntheticPerpetualLock,
}

/// Evaluates optimal recovery route when an intermediate leg of a triangular cycle experiences partial fill or failure.
/// Compares Route A (Immediate Market Inversion) vs Route B (Synthetic Perpetual Lock).
pub fn evaluate_triangular_unwind_route(
    unhedged_residual_usd: f64,
    leg_spread_bps: f64,
    perp_spread_bps: f64,
    taker_fee_bps: f64,
    basis_volatility: f64,
    max_acceptable_basis_vol: f64,
) -> TriangularRecoveryAction {
    if basis_volatility > max_acceptable_basis_vol
        || !basis_volatility.is_finite()
        || unhedged_residual_usd <= 0.0
    {
        return TriangularRecoveryAction::ImmediateMarketInversion;
    }
    // Route A cost: unhedged * (Spread_leg + 2 * f_taker) / 10000
    let cost_route_a = unhedged_residual_usd * (leg_spread_bps + 2.0 * taker_fee_bps) / 10_000.0;

    // Route B cost: unhedged * (0.5 * Spread_perp + f_taker) / 10000 + 0.5 * sigma_basis^2 * unhedged
    let basis_penalty = unhedged_residual_usd * 0.5 * basis_volatility * basis_volatility;
    let cost_route_b =
        unhedged_residual_usd * (0.5 * perp_spread_bps + taker_fee_bps) / 10_000.0 + basis_penalty;

    if cost_route_b < cost_route_a {
        TriangularRecoveryAction::SyntheticPerpetualLock
    } else {
        TriangularRecoveryAction::ImmediateMarketInversion
    }
}

use rust_decimal::prelude::ToPrimitive;
use std::collections::{HashMap, HashSet, VecDeque};

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
    /// Power-law depth slippage scale kappa in bps per (unit^alpha)
    pub kappa_bps: f64,
    /// Power-law depth slippage exponent (default 0.5 for square-root market impact)
    pub alpha_slippage: f64,
}

impl TriangularCycleEdge {
    /// Nominal-dependent dynamic weight incorporating power-law book depth slippage:
    /// w(Q) = -ln(rate * (1 - fee)) + (kappa_bps / 10000.0) * Q^alpha
    pub fn dynamic_weight(&self, nominal_qty: f64) -> f64 {
        if nominal_qty <= 0.0 || self.kappa_bps <= 0.0 {
            self.weight
        } else {
            let slip_impact = (self.kappa_bps / 10_000.0) * nominal_qty.powf(self.alpha_slippage);
            self.weight + slip_impact
        }
    }
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

/// Multi-hop / polygon arbitrage cycle candidate discovered by graph search.
#[derive(Debug, Clone, PartialEq)]
pub struct PolygonCycleCandidate {
    pub path: Vec<String>,
    pub symbols: Vec<String>,
    pub sides: Vec<TriLegSide>,
    pub fee_bps: Vec<f64>,
    pub expected_gross_bps: f64,
    pub expected_net_bps: f64,
    pub optimal_capital_usd: f64,
    pub max_net_profit_usd: f64,
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
        self.add_orderbook_edges_with_depth(base, quote, symbol, book, fee_bps, 0.5);
    }

    /// Adds a pair book's executable edges with depth-swept slippage calibration.
    pub fn add_orderbook_edges_with_depth(
        &mut self,
        base: &str,
        quote: &str,
        symbol: &str,
        book: &OrderBook,
        fee_bps: f64,
        alpha_slippage: f64,
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
        let alpha = alpha_slippage.clamp(0.1, 2.0);

        // Calibrate Ask kappa_bps from available book depth
        let cum_ask_depth: f64 = book
            .asks
            .iter()
            .map(|l| l.size.to_f64().unwrap_or(0.0))
            .sum();
        let ask_kappa = if book.asks.len() > 1 && cum_ask_depth > 0.0 {
            let deepest_ask = book
                .asks
                .last()
                .map(|l| l.price.to_f64().unwrap_or(ask_f))
                .unwrap_or(ask_f);
            let spread_bps = ((deepest_ask - ask_f) / ask_f) * 10000.0;
            (spread_bps / cum_ask_depth.powf(alpha)).max(0.01)
        } else {
            let top_qty = book
                .asks
                .first()
                .map(|l| l.size.to_f64().unwrap_or(1.0))
                .unwrap_or(1.0)
                .max(1e-4);
            (1.0 / top_qty.powf(alpha)).clamp(0.01, 100.0)
        };

        // Calibrate Bid kappa_bps from available book depth
        let cum_bid_depth: f64 = book
            .bids
            .iter()
            .map(|l| l.size.to_f64().unwrap_or(0.0))
            .sum();
        let bid_kappa = if book.bids.len() > 1 && cum_bid_depth > 0.0 {
            let deepest_bid = book
                .bids
                .last()
                .map(|l| l.price.to_f64().unwrap_or(bid_f))
                .unwrap_or(bid_f);
            let spread_bps = ((bid_f - deepest_bid) / bid_f) * 10000.0;
            (spread_bps / cum_bid_depth.powf(alpha)).max(0.01)
        } else {
            let top_qty = book
                .bids
                .first()
                .map(|l| l.size.to_f64().unwrap_or(1.0))
                .unwrap_or(1.0)
                .max(1e-4);
            (1.0 / top_qty.powf(alpha)).clamp(0.01, 100.0)
        };

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
                kappa_bps: ask_kappa,
                alpha_slippage: alpha,
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
                kappa_bps: bid_kappa,
                alpha_slippage: alpha,
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

    /// Detects negative weight cycles (profitable polygon arbitrage) using
    /// Shortest Path Faster Algorithm (SPFA) with Small-Label-First (SLF) queue heuristic
    /// and early cycle extraction in O(|E|) average time.
    pub fn find_negative_cycles_spfa(
        &self,
        nominal_qty: f64,
        max_cycle_len: usize,
    ) -> Vec<PolygonCycleCandidate> {
        let n = self.nodes.len();
        if n < 3 {
            return Vec::new();
        }

        let mut dist = vec![0.0; n];
        let mut pred: Vec<Option<(usize, usize)>> = vec![None; n];
        let mut in_queue = vec![true; n];
        let mut relax_count = vec![0; n];
        let mut queue = VecDeque::with_capacity(n);

        for i in 0..n {
            queue.push_back(i);
        }

        let mut candidates = Vec::new();
        let mut seen_keys = HashSet::new();

        let mut iterations = 0;
        let max_iterations = (n * 20).max(200);

        while let Some(u) = queue.pop_front() {
            in_queue[u] = false;
            iterations += 1;
            if iterations > max_iterations {
                break;
            }

            for (edge_idx, edge) in self.edges[u].iter().enumerate() {
                let v = edge.to_idx;
                let w = edge.dynamic_weight(nominal_qty);
                if dist[u] + w < dist[v] - 1e-12 {
                    dist[v] = dist[u] + w;
                    pred[v] = Some((u, edge_idx));
                    relax_count[v] += 1;

                    // Early cycle check if relax count exceeds vertex count
                    if relax_count[v] >= n {
                        relax_count[v] = 0;
                        let mut curr = v;
                        for _ in 0..n {
                            if let Some((p, _)) = pred[curr] {
                                curr = p;
                            }
                        }

                        let mut visited = vec![false; n];
                        let mut loop_edges = Vec::new();
                        let mut step = curr;
                        while !visited[step] {
                            visited[step] = true;
                            if let Some((p, e_idx)) = pred[step] {
                                loop_edges.push((p, step, e_idx));
                                step = p;
                            } else {
                                break;
                            }
                        }

                        if let Some(start_pos) = loop_edges.iter().position(|&(p, _, _)| p == step)
                        {
                            let cycle_slice = &loop_edges[..=start_pos];
                            if cycle_slice.len() >= 3 && cycle_slice.len() <= max_cycle_len {
                                let mut resolved_edges = Vec::with_capacity(cycle_slice.len());
                                for &(from_n, _to_n, e_idx) in cycle_slice.iter().rev() {
                                    resolved_edges.push(self.edges[from_n][e_idx].clone());
                                }

                                if let Some(cand) = self.build_polygon_candidate(&resolved_edges) {
                                    let mut path_norm = cand.path.clone();
                                    if let Some(min_pos) = path_norm
                                        .iter()
                                        .enumerate()
                                        .min_by(|(_, a), (_, b)| a.cmp(b))
                                        .map(|(i, _)| i)
                                    {
                                        path_norm.rotate_left(min_pos);
                                    }
                                    let key = path_norm.join("->");
                                    if seen_keys.insert(key) {
                                        candidates.push(cand);
                                    }
                                }
                            }
                        }
                    }

                    if !in_queue[v] {
                        in_queue[v] = true;
                        // Small-Label-First (SLF) queue heuristic
                        if let Some(&front) = queue.front() {
                            if dist[v] < dist[front] {
                                queue.push_front(v);
                            } else {
                                queue.push_back(v);
                            }
                        } else {
                            queue.push_back(v);
                        }
                    }
                }
            }
        }

        candidates.sort_by(|a, b| b.expected_net_bps.total_cmp(&a.expected_net_bps));
        candidates
    }

    /// Builds a PolygonCycleCandidate from a sequence of edges, computing return and optimal capital.
    pub fn build_polygon_candidate(
        &self,
        cycle_edges: &[TriangularCycleEdge],
    ) -> Option<PolygonCycleCandidate> {
        if cycle_edges.len() < 3 {
            return None;
        }

        let mut path = Vec::with_capacity(cycle_edges.len());
        let mut symbols = Vec::with_capacity(cycle_edges.len());
        let mut sides = Vec::with_capacity(cycle_edges.len());
        let mut fee_bps = Vec::with_capacity(cycle_edges.len());
        let mut gross_mult = 1.0;
        let mut total_weight = 0.0;
        let mut k_total = 0.0;

        for edge in cycle_edges {
            path.push(edge.from.clone());
            symbols.push(edge.symbol.clone());
            sides.push(edge.side);
            fee_bps.push(edge.fee_bps);
            gross_mult *= edge.rate;
            total_weight += edge.weight;
            k_total += edge.kappa_bps / 10_000.0;
        }

        let gross_bps = (gross_mult - 1.0) * 10000.0;
        let net_mult = (-total_weight).exp();
        let net_bps = (net_mult - 1.0) * 10000.0;

        let r_gross = net_mult - 1.0;
        let (optimal_capital_usd, max_net_profit_usd) = if r_gross > 0.0 {
            solve_optimal_polygon_capital(r_gross, k_total, 0.5, 0.05, 0.65, 5.0)
                .unwrap_or((0.0, 0.0))
        } else {
            (0.0, 0.0)
        };

        Some(PolygonCycleCandidate {
            path,
            symbols,
            sides,
            fee_bps,
            expected_gross_bps: gross_bps,
            expected_net_bps: net_bps,
            optimal_capital_usd,
            max_net_profit_usd,
        })
    }
}

/// Closed-form solution for optimal polygon cycle capital Q* under power-law orderbook slippage
/// and inventory/holding risk:
/// \max_Q \Pi(Q) = \mathcal{R}_{\text{gross}} Q - K_{\text{total}} Q^{\alpha + 1} - \frac{1}{2} \gamma \sigma^2 \tau Q^2
pub fn solve_optimal_polygon_capital(
    r_gross: f64,
    k_total: f64,
    alpha_slip: f64,
    gamma_risk: f64,
    vol: f64,
    horizon_sec: f64,
) -> Option<(f64, f64)> {
    if r_gross <= 0.0 || !r_gross.is_finite() {
        return None;
    }
    let horizon_years = (horizon_sec.max(0.001) / (365.25 * 86400.0)).max(1e-12);
    let risk_term = gamma_risk.max(0.0) * vol.powi(2) * horizon_years;
    let k_tot = k_total.max(1e-9);

    let q_star = if (alpha_slip - 0.5).abs() < 1e-3 {
        // Square-root law (\alpha = 0.5):
        // dPi/dQ = R_gross - (3/2) K_tot Q^0.5 - risk_term Q = 0
        // Letting x = sqrt(Q): risk_term x^2 + 1.5 K_tot x - R_gross = 0
        if risk_term <= 1e-12 {
            (r_gross / (1.5 * k_tot)).powi(2)
        } else {
            let b = 1.5 * k_tot;
            let discr = b * b + 4.0 * risk_term * r_gross;
            let x_star = (discr.sqrt() - b) / (2.0 * risk_term);
            x_star.max(0.0).powi(2)
        }
    } else if (alpha_slip - 1.0).abs() < 1e-3 {
        // Linear slippage impact (\alpha = 1.0):
        // dPi/dQ = R_gross - 2 K_tot Q - risk_term Q = 0
        r_gross / (2.0 * k_tot + risk_term)
    } else {
        // General power-law inversion fallback
        let denom = (alpha_slip + 1.0) * k_tot + risk_term;
        (r_gross / denom).powf(1.0 / alpha_slip)
    };

    if q_star <= 0.0 || !q_star.is_finite() {
        return None;
    }

    let profit =
        r_gross * q_star - k_tot * q_star.powf(alpha_slip + 1.0) - 0.5 * risk_term * q_star.powi(2);
    Some((q_star, profit.max(0.0)))
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

    #[test]
    fn test_fragility_first_triangular_ranking() {
        let books = profitable_books();
        // Book 0: BTC-USDC (deep, tight)
        // Book 1: ETH-BTC (thinner, more volatile)
        // Book 2: ETH-USDC (medium)
        let sides = [
            TriLegSide::BuyBase,
            TriLegSide::BuyBase,
            TriLegSide::SellBase,
        ];
        let targets = [dec!(0.1), dec!(1.0), dec!(1.0)];
        let vols = [1.5, 5.0, 2.5]; // ETH-BTC has higher volatility

        let f0 = compute_book_fragility(&books[0], sides[0], targets[0], vols[0]);
        let f1 = compute_book_fragility(&books[1], sides[1], targets[1], vols[1]);
        let f2 = compute_book_fragility(&books[2], sides[2], targets[2], vols[2]);

        assert!(f0 > 0.0);
        assert!(f1 > 0.0);
        assert!(f2 > 0.0);

        let ranking =
            rank_legs_by_fragility([&books[0], &books[1], &books[2]], sides, targets, vols);
        // The most fragile leg (lowest F) must be ranked first
        assert_eq!(
            ranking[0], 1,
            "ETH-BTC must be ranked first as the bottleneck leg"
        );
    }

    #[test]
    fn test_mtt_fee_compression_advantage() {
        let gross_bps = dec!(12.0);
        let maker_fee_rebate = dec!(-0.5); // 0.5 bps maker rebate
        let taker_fee_leg2 = dec!(3.5);
        let taker_fee_leg3 = dec!(3.5);
        let p_fill = 0.80;
        let slip_23 = dec!(2.0);

        let mtt_edge = compute_mtt_net_edge(
            gross_bps,
            maker_fee_rebate,
            taker_fee_leg2,
            taker_fee_leg3,
            p_fill,
            slip_23,
        );

        // Net on fill = 12.0 - (-0.5 + 3.5 + 3.5) - 2.0 = 12.0 - 6.5 - 2.0 = 3.5 bps
        // Expected = 3.5 * 0.80 = 2.8 bps
        assert_eq!(mtt_edge, dec!(2.8));
    }

    #[test]
    fn test_compute_optimal_triangular_capital() {
        // Gross return ratio = 1.0020 (+20 bps gross)
        // Total fees = 10 bps (0.0010)
        // Net edge = +10 bps (0.0010)
        let gross_ratio = 1.0020;
        let fee_rate = 0.0010;
        // Depths: Leg 1: $50,000, Leg 2: $20,000, Leg 3: $100,000
        let depths = [50_000.0, 20_000.0, 100_000.0];

        let result = compute_optimal_triangular_capital(gross_ratio, fee_rate, depths);
        assert!(result.is_some());
        let (s_star, max_profit) = result.unwrap();

        assert!(s_star > 0.0, "Optimal capital must be positive");
        assert!(max_profit > 0.0, "Max profit must be positive");
        assert!(
            s_star < 50_000.0,
            "Optimal capital should be bounded by depth"
        );

        // Edge case: unprofitable cycle (gross < fees) returns None
        let unprof = compute_optimal_triangular_capital(1.0005, 0.0010, depths);
        assert!(unprof.is_none());
    }

    #[test]
    fn test_evaluate_triangular_unwind_route() {
        let unhedged_usd = 5000.0;
        let leg_spread_bps = 8.0;
        let perp_spread_bps = 1.0;
        let taker_fee_bps = 3.5;

        // Calm regime: low basis volatility (3%) -> SyntheticPerpetualLock is cheaper
        let low_vol = 0.03;
        let action_low = evaluate_triangular_unwind_route(
            unhedged_usd,
            leg_spread_bps,
            perp_spread_bps,
            taker_fee_bps,
            low_vol,
            0.15,
        );
        assert_eq!(
            action_low,
            TriangularRecoveryAction::SyntheticPerpetualLock,
            "Low basis vol should choose synthetic perp lock"
        );

        // High basis volatility (25% > 15% max acceptable) -> ImmediateMarketInversion
        let high_vol = 0.25;
        let action_high = evaluate_triangular_unwind_route(
            unhedged_usd,
            leg_spread_bps,
            perp_spread_bps,
            taker_fee_bps,
            high_vol,
            0.15,
        );
        assert_eq!(
            action_high,
            TriangularRecoveryAction::ImmediateMarketInversion,
            "High basis vol must choose immediate market inversion"
        );
    }

    #[test]
    fn test_solve_optimal_polygon_capital() {
        // Gross edge = +1.0% (0.0100)
        // K_total = 0.0002
        // Square-root slippage
        let r_gross = 0.0100;
        let k_total = 0.0002;
        let res = solve_optimal_polygon_capital(r_gross, k_total, 0.5, 0.01, 0.60, 10.0);
        assert!(res.is_some());
        let (q_star, profit) = res.unwrap();
        assert!(q_star > 0.0, "Optimal capital must be positive");
        assert!(profit > 0.0, "Max net profit must be positive");

        // Linear slippage (\alpha = 1.0)
        let res_lin = solve_optimal_polygon_capital(r_gross, k_total, 1.0, 0.01, 0.60, 10.0);
        assert!(res_lin.is_some());
        let (q_lin, profit_lin) = res_lin.unwrap();
        assert!(q_lin > 0.0);
        assert!(profit_lin > 0.0);

        // Zero edge returns None
        assert!(solve_optimal_polygon_capital(0.0, k_total, 0.5, 0.01, 0.60, 10.0).is_none());
        assert!(solve_optimal_polygon_capital(-0.01, k_total, 0.5, 0.01, 0.60, 10.0).is_none());
    }

    #[test]
    fn test_currency_graph_spfa_cycle_detection() {
        let mut graph = CurrencyGraph::new();

        // Build a profitable 3-cycle: USDC -> BTC -> ETH -> USDC
        // BTC-USDC: buy @ 60,000, sell @ 59,990
        // ETH-BTC: buy @ 20.01, sell @ 20.00
        // ETH-USDC: buy @ 3,101, sell @ 3,100
        let b_btc = deep_book("lighter", "BTC-USDC", dec!(59990), dec!(60000));
        let b_eth_btc = deep_book("lighter", "ETH-BTC", dec!(20), dec!(20.01));
        let b_eth = deep_book("lighter", "ETH-USDC", dec!(3100), dec!(3101));

        graph.add_orderbook_edges("BTC", "USDC", "BTC-USDC", &b_btc, 2.0);
        graph.add_orderbook_edges("ETH", "BTC", "ETH-BTC", &b_eth_btc, 2.0);
        graph.add_orderbook_edges("ETH", "USDC", "ETH-USDC", &b_eth, 2.0);

        let cycles = graph.find_negative_cycles_spfa(100.0, 5);
        assert!(!cycles.is_empty(), "SPFA must discover profitable cycle");
        let top = &cycles[0];
        assert!(top.expected_net_bps > 0.0);
        assert!(top.path.len() >= 3);
        assert!(top.optimal_capital_usd > 0.0);
    }
}
