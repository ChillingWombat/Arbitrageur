use arbitrageur_core::types::{dec, Decimal, BPS_DIVISOR};
use arbitrageur_core::quote::FundingRate;

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

/// One rung of the funding entry-capacity walk.
#[derive(Debug, Clone, PartialEq)]
pub struct FundingCapacityTier {
    /// Tier notional (USD) requested.
    pub size_usd: f64,
    /// Notional actually fillable inside both books (<= size_usd).
    pub filled_usd: f64,
    /// Signed cross at this tier's average entry, bps ((bid−ask)/ask).
    /// Usually <= 0 (a cost); `None` when nothing filled.
    pub entry_spread_bps: Option<f64>,
}

/// Depth-aware entry sizing for a funding-carry pair direction.
#[derive(Debug, Clone, PartialEq)]
pub struct FundingCapacityReport {
    pub tiers: Vec<FundingCapacityTier>,
    /// Largest tier filled to >= 99% — the depth-supported executable size.
    /// `None` when the books hold no fillable depth at all.
    pub optimal_size_usd: Option<f64>,
    /// Signed entry cross at the optimal tier, bps.
    pub entry_spread_bps: Option<f64>,
}

/// Walk the long leg's asks and the short leg's bids in lockstep and measure
/// how much notional can actually be entered at each tier, plus what crossing
/// the spread costs there. Unlike the spread monitor's probability-weighted
/// ladder, funding income is deterministic per unit — depth only bounds the
/// executable size and prices the entry cross.
///
/// * `buy_asks`: (price, qty) ascending — the long leg we lift.
/// * `sell_bids`: (price, qty) descending — the short leg we hit.
/// * `tiers_usd`: requested notionals, evaluated independently.
pub fn funding_entry_capacity(
    buy_asks: &[(f64, f64)],
    sell_bids: &[(f64, f64)],
    tiers_usd: &[f64],
) -> FundingCapacityReport {
    struct Chunk {
        notional: f64, // qty * ask price (buy-side notional)
        ask: f64,
        bid: f64,
    }
    let mut chunks: Vec<Chunk> = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    let (mut rem_a, mut rem_b) = (0.0, 0.0);
    while i < buy_asks.len() && j < sell_bids.len() {
        if rem_a <= 0.0 {
            rem_a = buy_asks[i].1;
            if rem_a <= 0.0 {
                i += 1;
                continue;
            }
        }
        if rem_b <= 0.0 {
            rem_b = sell_bids[j].1;
            if rem_b <= 0.0 {
                j += 1;
                continue;
            }
        }
        let q = rem_a.min(rem_b);
        let a = buy_asks[i].0;
        let b = sell_bids[j].0;
        if a.is_finite() && a > 0.0 && b.is_finite() && b > 0.0 && q.is_finite() && q > 0.0 {
            chunks.push(Chunk {
                notional: q * a,
                ask: a,
                bid: b,
            });
        }
        rem_a -= q;
        rem_b -= q;
        if rem_a <= 0.0 {
            i += 1;
        }
        if rem_b <= 0.0 {
            j += 1;
        }
    }

    let mut tiers = Vec::with_capacity(tiers_usd.len());
    let mut optimal: Option<(f64, f64)> = None; // (filled_usd, entry_spread_bps)
    for &tier in tiers_usd {
        if !(tier.is_finite() && tier > 0.0) {
            continue;
        }
        let mut filled = 0.0;
        let mut cost_a = 0.0; // ask-side notional accumulator for VWAP
        let mut qty_a = 0.0;
        let mut cost_b = 0.0;
        let mut qty_b = 0.0;
        for c in &chunks {
            if filled >= tier {
                break;
            }
            let take = (tier - filled).min(c.notional);
            if take <= 0.0 {
                continue;
            }
            filled += take;
            let q = take / c.ask;
            qty_a += q;
            cost_a += take;
            qty_b += q;
            cost_b += q * c.bid;
        }
        if filled <= 0.0 {
            break; // book exhausted — deeper tiers unfillable
        }
        let spread = if qty_a > 0.0 && qty_b > 0.0 {
            Some((cost_b / qty_b - cost_a / qty_a) / (cost_a / qty_a) * 10_000.0)
        } else {
            None
        };
        if filled >= tier * 0.99 {
            optimal = Some((filled, spread.unwrap_or(0.0)));
        }
        tiers.push(FundingCapacityTier {
            size_usd: tier,
            filled_usd: filled,
            entry_spread_bps: spread,
        });
    }

    FundingCapacityReport {
        tiers,
        optimal_size_usd: optimal.map(|(s, _)| s),
        entry_spread_bps: optimal.map(|(_, e)| e),
    }
}

#[cfg(test)]
mod funding_capacity_tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn empty_books_yield_no_capacity() {
        let rep = funding_entry_capacity(&[], &[], &[1000.0]);
        assert!(rep.tiers.is_empty());
        assert_eq!(rep.optimal_size_usd, None);
        assert_eq!(rep.entry_spread_bps, None);
    }

    #[test]
    fn full_fill_measures_flat_cross() {
        // Ask 100 x10, bid 99.9 x10: $1000 tier fills exactly.
        let rep = funding_entry_capacity(&[(100.0, 10.0)], &[(99.9, 10.0)], &[1000.0]);
        assert_eq!(rep.tiers.len(), 1);
        assert!(approx(rep.tiers[0].filled_usd, 1000.0));
        assert!(approx(rep.tiers[0].entry_spread_bps.unwrap(), -10.0));
        assert!(approx(rep.optimal_size_usd.unwrap(), 1000.0));
    }

    #[test]
    fn thin_book_caps_optimal_below_requested() {
        // Only $500 fillable of a $2000 tier.
        let rep = funding_entry_capacity(&[(100.0, 5.0)], &[(99.9, 5.0)], &[1000.0, 2000.0]);
        assert!(rep.optimal_size_usd.is_none() || rep.optimal_size_usd.unwrap() < 2000.0);
        assert!(approx(rep.tiers[0].filled_usd, 500.0));
        // $1000 tier unfillable (only $500) — still recorded, not optimal.
        assert!(rep.tiers.iter().all(|t| t.filled_usd <= 500.0 + 1e-6));
    }

    #[test]
    fn deeper_levels_price_the_cross() {
        // Best ask 100 x1, next 101 x100; bids mirror at -0.1%.
        let rep = funding_entry_capacity(
            &[(100.0, 1.0), (101.0, 100.0)],
            &[(99.9, 1.0), (100.899, 100.0)],
            &[100.0, 5000.0],
        );
        let small = &rep.tiers[0];
        let big = &rep.tiers[1];
        assert!(small.entry_spread_bps.unwrap() > big.entry_spread_bps.unwrap());
        assert!(approx(rep.optimal_size_usd.unwrap(), big.filled_usd));
    }
}
