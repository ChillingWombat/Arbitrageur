use arbitrager_core::types::{dec, Decimal, BPS_DIVISOR};
use arbitrager_core::quote::FundingRate;

/// Detailed breakdown of a cross-exchange funding rate carry opportunity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FundingCarryOpportunity {
    /// Long venue side (pays or receives long_rate)
    pub long_venue: String,
    /// Short venue side (pays or receives short_rate)
    pub short_venue: String,
    /// Symbol
    pub symbol: String,
    /// Effective net funding rate per 8-hour normalized period
    pub net_rate_8h: Decimal,
    /// Annualized Percentage Rate (APR)
    pub net_apr: Decimal,
    /// Annualized Percentage Yield (APY) with periodic compounding
    pub net_apy: Decimal,
    /// Estimated round-trip transaction fees across both legs in BPS
    pub round_trip_fee_bps: Decimal,
    /// Hours required to break even on fees and start earning positive carry
    pub break_even_hours: Decimal,
    /// Recommended position size in USD based on liquidity and leverage
    pub recommended_size_usd: Decimal,
    /// Expected daily income in USD
    pub daily_income_usd: Decimal,
}

/// Evaluates funding rate arbitrage between two perpetual markets.
/// Venue A funding: `rate_a` (interval `interval_a_hours`)
/// Venue B funding: `rate_b` (interval `interval_b_hours`)
///
/// If `rate_a > rate_b`:
///   We Short Venue A (earn `rate_a`) and Long Venue B (pay `rate_b`).
///   Net yield = `rate_a - rate_b`.
/// If `rate_b > rate_a`:
///   We Short Venue B (earn `rate_b`) and Long Venue A (pay `rate_a`).
///   Net yield = `rate_b - rate_a`.
pub fn evaluate_funding_carry(
    venue_a: &FundingRate,
    venue_b: &FundingRate,
    round_trip_fee_bps: Decimal,
    trade_size_usd: Decimal,
) -> Option<FundingCarryOpportunity> {
    if venue_a.symbol != venue_b.symbol {
        return None;
    }

    // Normalize both rates to hourly rate
    let hourly_rate_a = venue_a.rate / Decimal::from(venue_a.interval_hours.max(1));
    let hourly_rate_b = venue_b.rate / Decimal::from(venue_b.interval_hours.max(1));

    let (short_venue, long_venue, net_hourly) = if hourly_rate_a >= hourly_rate_b {
        (venue_a.venue.to_string(), venue_b.venue.to_string(), hourly_rate_a - hourly_rate_b)
    } else {
        (venue_b.venue.to_string(), venue_a.venue.to_string(), hourly_rate_b - hourly_rate_a)
    };

    if net_hourly <= dec!(0) {
        return None;
    }

    let net_rate_8h = net_hourly.checked_mul(dec!(8)).unwrap_or(dec!(999999));
    let net_apr = net_hourly
        .checked_mul(dec!(8760))
        .unwrap_or(dec!(999999));

    // APY = (1 + net_hourly)^8760 - 1
    let hourly_f64 = net_hourly.to_string().parse::<f64>().unwrap_or(0.0);
    let apy_f64 = if hourly_f64 > 0.0 && hourly_f64 < 1.0 {
        let compounding = (1.0 + hourly_f64).powf(8760.0) - 1.0;
        if compounding.is_finite() && compounding < 100_000.0 {
            compounding
        } else {
            100_000.0
        }
    } else if hourly_f64 >= 1.0 {
        100_000.0
    } else {
        0.0
    };
    let net_apy = Decimal::from_str_exact(&format!("{:.6}", apy_f64))
        .ok()
        .or_else(|| Decimal::from_f64_retain(apy_f64))
        .unwrap_or(net_apr);

    let fee_ratio = round_trip_fee_bps / BPS_DIVISOR;
    let break_even_hours = if net_hourly > dec!(0) {
        fee_ratio.checked_div(net_hourly).unwrap_or(dec!(999999))
    } else {
        dec!(999999)
    };

    let daily_income_usd = trade_size_usd
        .checked_mul(net_hourly)
        .and_then(|d| d.checked_mul(dec!(24)))
        .unwrap_or(dec!(999999));

    Some(FundingCarryOpportunity {
        long_venue,
        short_venue,
        symbol: venue_a.symbol.clone(),
        net_rate_8h,
        net_apr,
        net_apy,
        round_trip_fee_bps,
        break_even_hours,
        recommended_size_usd: trade_size_usd,
        daily_income_usd,
    })
}

/// Evaluates Spot-Perp Cash and Carry:
/// Buy spot on `spot_venue`, short perpetual on `perp_venue`.
/// Spot has 0 funding, Perpetual short earns positive funding `perp_rate`.
pub fn evaluate_spot_perp_carry(
    spot_price: Decimal,
    perp_price: Decimal,
    perp_funding: &FundingRate,
    round_trip_fee_bps: Decimal,
    trade_size_usd: Decimal,
) -> Option<FundingCarryOpportunity> {
    let basis = perp_price - spot_price;
    let basis_bps = (basis / spot_price) * BPS_DIVISOR;

    // Hourly rate on perp short
    let hourly_funding = perp_funding.rate / Decimal::from(perp_funding.interval_hours.max(1));
    let net_hourly = hourly_funding;

    if net_hourly <= dec!(0) && basis_bps <= dec!(0) {
        return None;
    }

    let net_rate_8h = net_hourly.checked_mul(dec!(8)).unwrap_or(dec!(999999));
    let net_apr = net_hourly
        .checked_mul(dec!(8760))
        .unwrap_or(dec!(999999));

    let fee_ratio = round_trip_fee_bps / BPS_DIVISOR;
    let break_even_hours = if net_hourly > dec!(0) {
        fee_ratio.checked_div(net_hourly).unwrap_or(dec!(999999))
    } else {
        dec!(999999)
    };

    let daily_income_usd = trade_size_usd
        .checked_mul(net_hourly)
        .and_then(|d| d.checked_mul(dec!(24)))
        .unwrap_or(dec!(999999));

    Some(FundingCarryOpportunity {
        long_venue: "SPOT".to_string(),
        short_venue: perp_funding.venue.to_string(),
        symbol: perp_funding.symbol.clone(),
        net_rate_8h,
        net_apr,
        net_apy: net_apr,
        round_trip_fee_bps,
        break_even_hours,
        recommended_size_usd: trade_size_usd,
        daily_income_usd,
    })
}
