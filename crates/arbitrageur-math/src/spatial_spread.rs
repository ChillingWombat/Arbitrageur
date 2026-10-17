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
    /// Depth-weighted VWAP execution price for the buy leg
    pub buy_vwap: Decimal,
    /// Depth-weighted VWAP execution price for the sell leg
    pub sell_vwap: Decimal,
    /// Marginal (worst) swept price for the buy leg
    pub worst_buy_price: Decimal,
    /// Marginal (worst) swept price for the sell leg
    pub worst_sell_price: Decimal,
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

    if buy_vwap <= dec!(0) || sell_vwap <= dec!(0) {
        return None;
    }

    let worst_buy_price = buy_book
        .buy_execution_marginal_price(executable_qty)
        .unwrap_or(buy_vwap);
    let worst_sell_price = sell_book
        .sell_execution_marginal_price(executable_qty)
        .unwrap_or(sell_vwap);

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
        buy_vwap,
        sell_vwap,
        worst_buy_price,
        worst_sell_price,
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

/// Computes a volatility-scaled baseline threshold.
/// In high-volatility regimes (annualized volatility > baseline), the profit margin expands
/// by (1.0 + vol_scaling_factor * (realized_vol - baseline_vol).max(0.0)) to defend against adverse selection.
pub fn compute_volatility_scaled_baseline_threshold(
    baseline_avg_spread: Decimal,
    reference_price: Decimal,
    target_profit_bps: Decimal,
    spread_baseline_sum: Decimal,
    realized_vol_annualized: Decimal,
    baseline_vol_annualized: Decimal,
    vol_scaling_factor: Decimal,
) -> Decimal {
    let vol_diff = (realized_vol_annualized - baseline_vol_annualized).max(dec!(0));
    let vol_multiplier = dec!(1) + vol_scaling_factor * vol_diff;
    let scaled_profit_bps = target_profit_bps * vol_multiplier;
    let profit_margin = reference_price * (scaled_profit_bps / BPS_DIVISOR);
    baseline_avg_spread + profit_margin - spread_baseline_sum
}

/// Continuous Power-Law Depth-Swept Slippage Surface and Optimal Sizing Engine.
/// Models marginal price impact p(u) = P_touch + \kappa * u^\alpha,
/// where \alpha = 1 / \gamma \in [0.4, 1.0] (\alpha = 0.5 for square-root market impact).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PowerLawDepthSurface {
    /// Elasticity exponent \alpha = 1 / \gamma
    pub alpha: f64,
    /// Depth curvature friction parameter \kappa
    pub kappa: f64,
}

impl PowerLawDepthSurface {
    pub fn new(alpha: f64, kappa: f64) -> Self {
        Self {
            alpha: alpha.clamp(0.20, 2.0),
            kappa: kappa.max(1e-9),
        }
    }

    /// Calibrates parameter \kappa from an observed discrete swept marginal price at depth Q_obs:
    /// \kappa = (P_marginal - P_touch) / Q_obs^\alpha
    pub fn calibrate_from_observed_sweep(
        touch_price: f64,
        marginal_price: f64,
        observed_qty: f64,
        alpha: f64,
    ) -> Option<Self> {
        if touch_price <= 0.0 || marginal_price <= 0.0 || observed_qty <= 1e-6 {
            return None;
        }
        let price_diff = (marginal_price - touch_price).abs();
        let q_pow = observed_qty.powf(alpha.clamp(0.20, 2.0));
        if q_pow <= 1e-12 {
            return None;
        }
        let kappa = price_diff / q_pow;
        Some(Self::new(alpha, kappa))
    }

    /// Computes cumulative dollar slippage relative to touch:
    /// Slip_$(Q) = \int_0^Q \kappa u^\alpha du = [\kappa / (\alpha + 1)] * Q^(\alpha + 1)
    pub fn slippage_usd(&self, quantity: f64) -> f64 {
        if quantity <= 0.0 {
            return 0.0;
        }
        (self.kappa / (self.alpha + 1.0)) * quantity.powf(self.alpha + 1.0)
    }

    /// Computes realized execution VWAP for target quantity:
    /// VWAP(Q) = P_touch \pm [\kappa / (\alpha + 1)] * Q^\alpha
    pub fn execution_vwap(&self, touch_price: f64, quantity: f64, side: Side) -> f64 {
        if quantity <= 0.0 || touch_price <= 0.0 {
            return touch_price;
        }
        let vwap_offset = (self.kappa / (self.alpha + 1.0)) * quantity.powf(self.alpha);
        match side {
            Side::Buy => touch_price + vwap_offset,
            Side::Sell => (touch_price - vwap_offset).max(1e-6),
        }
    }

    /// Solves closed-form optimal order size Q* maximizing net profit net of price impact
    /// and inventory risk:
    /// \max_Q \Pi(Q) = Q * \mathcal{E}_{net} - [\kappa / (\alpha + 1)] * Q^(\alpha + 1) - \frac{1}{2} \gamma \sigma^2 \tau Q^2
    pub fn solve_optimal_size(
        &self,
        net_unit_edge_usd: f64,
        inventory_risk_gamma: f64,
        annualized_vol: f64,
        horizon_sec: f64,
    ) -> f64 {
        if net_unit_edge_usd <= 0.0 {
            return 0.0;
        }
        let horizon_years = (horizon_sec.max(0.001) / (365.25 * 86400.0)).max(1e-12);
        let risk_term = inventory_risk_gamma.max(0.0) * annualized_vol.powi(2) * horizon_years;

        if (self.alpha - 0.5).abs() < 1e-3 {
            // Exact closed-form for Square-Root Law (\alpha = 0.5):
            if risk_term <= 1e-12 {
                (net_unit_edge_usd / self.kappa).powi(2)
            } else {
                let discr = self.kappa.powi(2) + 4.0 * risk_term * net_unit_edge_usd;
                let z_star = (discr.sqrt() - self.kappa) / (2.0 * risk_term);
                z_star.max(0.0).powi(2)
            }
        } else if (self.alpha - 1.0).abs() < 1e-3 {
            // Exact closed-form for Linear Kyle's Lambda (\alpha = 1.0):
            net_unit_edge_usd / (self.kappa + risk_term)
        } else {
            // General power-law inversion fallback
            (net_unit_edge_usd / self.kappa).powf(1.0 / self.alpha)
        }
    }
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

    #[test]
    fn test_compute_volatility_scaled_baseline_threshold() {
        let base_avg = dec!(10.0);
        let ref_px = dec!(50000.0);
        let target_bps = dec!(5.0);
        let sum_pts = dec!(2.0);

        let unscaled = compute_baseline_threshold(base_avg, ref_px, target_bps, sum_pts);
        // baseline profit margin = 50000 * 5 / 10000 = 25.0
        // total = 10.0 + 25.0 - 2.0 = 33.0
        assert_eq!(unscaled, dec!(33.0));

        // When realized vol <= baseline vol, scaling does not expand
        let scaled_calm = compute_volatility_scaled_baseline_threshold(
            base_avg,
            ref_px,
            target_bps,
            sum_pts,
            dec!(0.5),
            dec!(0.6),
            dec!(2.0),
        );
        assert_eq!(scaled_calm, unscaled);

        // When realized vol = 0.9 and baseline vol = 0.5 (diff = 0.4), with scaling factor 2.0:
        // vol_multiplier = 1.0 + 2.0 * 0.4 = 1.8
        // scaled_bps = 5.0 * 1.8 = 9.0 bps
        // profit_margin = 50000 * 9.0 / 10000 = 45.0
        // total = 10.0 + 45.0 - 2.0 = 53.0
        let scaled_turbulent = compute_volatility_scaled_baseline_threshold(
            base_avg,
            ref_px,
            target_bps,
            sum_pts,
            dec!(0.9),
            dec!(0.5),
            dec!(2.0),
        );
        assert_eq!(scaled_turbulent, dec!(53.0));
    }

    #[test]
    fn test_power_law_depth_surface_calibration_and_sizing() {
        // Touch price = 100.0, Marginal price = 102.0 at Q = 4.0, alpha = 0.5 (square-root)
        // Delta P = 2.0, Q^0.5 = 2.0 => kappa = 2.0 / 2.0 = 1.0
        let surface = PowerLawDepthSurface::calibrate_from_observed_sweep(100.0, 102.0, 4.0, 0.5)
            .expect("calibration should succeed");
        assert!((surface.kappa - 1.0).abs() < 1e-6);

        // Slippage USD for Q = 4.0: [1.0 / 1.5] * 4.0^1.5 = (2/3) * 8.0 = 5.3333 USD
        let slip = surface.slippage_usd(4.0);
        assert!((slip - (16.0 / 3.0)).abs() < 1e-4);

        // Execution VWAP for Buy: 100.0 + (1.0 / 1.5) * 4.0^0.5 = 100.0 + 2.0 / 1.5 = 101.3333
        let buy_vwap = surface.execution_vwap(100.0, 4.0, Side::Buy);
        assert!((buy_vwap - (100.0 + 4.0 / 3.0)).abs() < 1e-4);

        // Optimal sizing under zero inventory risk: (E_unit / kappa)^2
        // E_unit = 2.5 => Q* = (2.5 / 1.0)^2 = 6.25
        let q_star_zero_risk = surface.solve_optimal_size(2.5, 0.0, 0.60, 60.0);
        assert!((q_star_zero_risk - 6.25).abs() < 1e-4);

        // Optimal sizing under positive inventory risk dampens size:
        let q_star_with_risk = surface.solve_optimal_size(2.5, 2.0, 0.60, 3600.0);
        assert!(q_star_with_risk < q_star_zero_risk);
        assert!(q_star_with_risk > 0.0);
    }
}
