use arbitrageur_core::types::{dec, Decimal, BPS_DIVISOR};

/// Binary prediction market arbitrage opportunity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredictionArbitrageOpportunity {
    pub strategy_name: String,
    pub buy_yes_venue: String,
    pub buy_no_venue: String,
    pub yes_price: Decimal,
    pub no_price: Decimal,
    pub total_cost: Decimal,
    pub guaranteed_payout: Decimal,
    pub net_profit: Decimal,
    pub net_return_pct: Decimal,
}

/// Evaluates synthetic cross-venue arbitrage between two prediction markets.
/// E.g., Polymarket vs Kalshi.
pub fn evaluate_prediction_cross_venue(
    venue_a_name: &str,
    venue_a_yes: Decimal,
    venue_a_no: Decimal,
    venue_b_name: &str,
    venue_b_yes: Decimal,
    venue_b_no: Decimal,
    fee_bps: Decimal,
) -> Option<PredictionArbitrageOpportunity> {
    evaluate_prediction_cross_venue_fees(
        venue_a_name,
        venue_a_yes,
        venue_a_no,
        fee_bps,
        venue_b_name,
        venue_b_yes,
        venue_b_no,
        fee_bps,
    )
}

/// Evaluates synthetic cross-venue arbitrage with distinct fee schedules for each venue.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_prediction_cross_venue_fees(
    venue_a_name: &str,
    venue_a_yes: Decimal,
    venue_a_no: Decimal,
    fee_a_bps: Decimal,
    venue_b_name: &str,
    venue_b_yes: Decimal,
    venue_b_no: Decimal,
    fee_b_bps: Decimal,
) -> Option<PredictionArbitrageOpportunity> {
    // Prediction market price sanity bounds: 0 < Price < 1.00
    if venue_a_yes <= dec!(0)
        || venue_a_yes >= dec!(1)
        || venue_a_no <= dec!(0)
        || venue_a_no >= dec!(1)
        || venue_b_yes <= dec!(0)
        || venue_b_yes >= dec!(1)
        || venue_b_no <= dec!(0)
        || venue_b_no >= dec!(1)
    {
        return None;
    }

    let guaranteed_payout = dec!(1.0);
    let fee_a_mult = fee_a_bps / BPS_DIVISOR;
    let fee_b_mult = fee_b_bps / BPS_DIVISOR;

    // Strategy 1: Buy YES on A, Buy NO on B
    let cost_1 = venue_a_yes + venue_b_no;
    let fees_1 = venue_a_yes * fee_a_mult + venue_b_no * fee_b_mult;
    let profit_1 = guaranteed_payout - cost_1 - fees_1;

    // Strategy 2: Buy YES on B, Buy NO on A
    let cost_2 = venue_b_yes + venue_a_no;
    let fees_2 = venue_b_yes * fee_b_mult + venue_a_no * fee_a_mult;
    let profit_2 = guaranteed_payout - cost_2 - fees_2;

    if profit_1 > dec!(0) && profit_1 >= profit_2 {
        Some(PredictionArbitrageOpportunity {
            strategy_name: format!("Buy YES on {} + Buy NO on {}", venue_a_name, venue_b_name),
            buy_yes_venue: venue_a_name.to_string(),
            buy_no_venue: venue_b_name.to_string(),
            yes_price: venue_a_yes,
            no_price: venue_b_no,
            total_cost: cost_1,
            guaranteed_payout,
            net_profit: profit_1,
            net_return_pct: if cost_1 > dec!(0) { (profit_1 / cost_1) * dec!(100) } else { dec!(0) },
        })
    } else if profit_2 > dec!(0) {
        Some(PredictionArbitrageOpportunity {
            strategy_name: format!("Buy YES on {} + Buy NO on {}", venue_b_name, venue_a_name),
            buy_yes_venue: venue_b_name.to_string(),
            buy_no_venue: venue_a_name.to_string(),
            yes_price: venue_b_yes,
            no_price: venue_a_no,
            total_cost: cost_2,
            guaranteed_payout,
            net_profit: profit_2,
            net_return_pct: if cost_2 > dec!(0) { (profit_2 / cost_2) * dec!(100) } else { dec!(0) },
        })
    } else {
        None
    }
}

/// Evaluates intra-market Dutch book arbitrage for mutually exclusive & exhaustive outcomes.
/// If sum of ask prices across all outcomes < 1.0 - fees, buying the entire basket is guaranteed profitable.
pub fn evaluate_categorical_dutch_book(
    outcome_ask_prices: &[Decimal],
    fee_bps: Decimal,
) -> Option<(Decimal, Decimal)> {
    if outcome_ask_prices.len() < 2 {
        return None;
    }

    let total_cost: Decimal = outcome_ask_prices.iter().copied().sum();
    let fees = total_cost * (fee_bps / BPS_DIVISOR);
    let guaranteed_payout = dec!(1.0);
    let net_profit = guaranteed_payout - total_cost - fees;

    if net_profit > dec!(0) {
        Some((total_cost, net_profit))
    } else {
        None
    }
}
