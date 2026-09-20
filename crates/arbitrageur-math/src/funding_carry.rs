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

/// Same-exchange spot-vs-perp/futures (cash-and-carry) basis breakdown.
///
/// Long spot on the venue, short the venue's perp for the same base asset.
/// PnL drivers, all captured explicitly:
/// - `basis_bps`: live divergence `(perp - spot) / spot` in bps. Positive =
///   perp in contango (normal); negative = backwardation. Convergence of this
///   spread at exit is a PnL term on top of funding accrual.
/// - funding accrual: short perp earns `perp_funding` per hour; spot pays none.
/// - friction: round-trip taker fees for BOTH legs plus the measured entry
///   cross (spot-ask lift vs perp-bid hit VWAP) from `funding_entry_capacity`.
///   Exit cross is not yet measured — the scanner reports the entry cross and
///   the configurator's max-entry-spread gate bounds it.
///
/// Reverse (long perp / short spot) needs a borrow desk and is NOT modelled:
/// this evaluator only produces the long-spot / short-perp direction and
/// returns `None` when the perp funding rate is non-positive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotPerpBasisBreakdown {
    /// Underlying carry opportunity (long leg = spot, short leg = perp venue).
    pub carry: FundingCarryOpportunity,
    /// Live spot reference price used for the basis print.
    pub spot_price: Decimal,
    /// Live perp reference price used for the basis print.
    pub perp_price: Decimal,
    /// Signed divergence `(perp - spot) / spot * 10000` in bps.
    pub basis_bps: Decimal,
}

/// Evaluates a same-exchange spot-perp cash-and-carry (basis) trade.
///
/// Returns `None` when prices are non-positive, when the perp funding rate is
/// non-positive (no carry to harvest, and the reverse direction needs borrow),
/// or when the decimal math overflows. Callers apply the strategy's own gates
/// (min APR, min basis bps, max entry spread) on the returned breakdown.
pub fn evaluate_spot_perp_basis(
    spot_price: Decimal,
    perp_price: Decimal,
    perp_funding: &FundingRate,
    round_trip_fee_bps: Decimal,
    trade_size_usd: Decimal,
) -> Option<SpotPerpBasisBreakdown> {
    if spot_price <= dec!(0) || perp_price <= dec!(0) {
        return None;
    }
    // Hourly rate earned by the short-perp leg.
    let hourly_funding = perp_funding.rate / Decimal::from(perp_funding.interval_hours.max(1));
    if hourly_funding <= dec!(0) {
        return None;
    }
    let basis_bps = ((perp_price - spot_price) / spot_price) * BPS_DIVISOR;

    let net_rate_8h = hourly_funding.checked_mul(dec!(8)).unwrap_or(dec!(999999));
    let net_apr = hourly_funding
        .checked_mul(dec!(8760))
        .unwrap_or(dec!(999999));

    let fee_ratio = round_trip_fee_bps / BPS_DIVISOR;
    let break_even_hours = fee_ratio
        .checked_div(hourly_funding)
        .unwrap_or(dec!(999999));

    let daily_income_usd = trade_size_usd
        .checked_mul(hourly_funding)
        .and_then(|d| d.checked_mul(dec!(24)))
        .unwrap_or(dec!(999999));

    let carry = FundingCarryOpportunity {
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
    };
    Some(SpotPerpBasisBreakdown {
        carry,
        spot_price,
        perp_price,
        basis_bps,
    })
}

/// Evaluates Spot-Perp Cash and Carry:
/// Buy spot on `spot_venue`, short perpetual on `perp_venue`.
/// Spot has 0 funding, Perpetual short earns positive funding `perp_rate`.
///
/// Thin wrapper over [`evaluate_spot_perp_basis`] kept for API compatibility;
/// prefer the basis breakdown variant which also reports the live spot/perp
/// divergence.
pub fn evaluate_spot_perp_carry(
    spot_price: Decimal,
    perp_price: Decimal,
    perp_funding: &FundingRate,
    round_trip_fee_bps: Decimal,
    trade_size_usd: Decimal,
) -> Option<FundingCarryOpportunity> {
    evaluate_spot_perp_basis(
        spot_price,
        perp_price,
        perp_funding,
        round_trip_fee_bps,
        trade_size_usd,
    )
    .map(|b| b.carry)
}

/// Expected funding-carry holding horizon: how long the funding gap is
/// expected to stay above the exit threshold.
///
/// v1 model (transparent, testable):
/// - Total cost to cover = round-trip fees + measured entry-spread cost
///   (static spread impact). Dynamic spread *drift* beyond entry is NOT yet
///   modelled — no perp/spot basis history exists in the scanner — and is
///   disclosed in `reason` with a confidence penalty when depth slope is high.
/// - Exit threshold = `min_apr_pct` converted to an 8h rate (default 8% APR).
///   When the gap decays below this, the bot would rotate out.
/// - `history` carries `(ts_ms, net_8h_fraction)` for the CURRENT direction
///   (short-leg rate minus long-leg rate, per-8h fraction, e.g. 0.000585 for
///   +0.0585%). A least-squares trend over the trailing window predicts the
///   crossing time `t_cross = (threshold - current) / slope`.
/// - Expected hold = `t_cross` clamped to `[breakeven_total, 168h]`. If the
///   gap decays before breakeven, the hold IS `t_cross` (early exit at a loss
///   — signed net carry will be negative, honestly).
/// - Stable/growing gaps (`slope >= 0`) hold to the 7-day persistence cap.
/// - Thin history (<10 points or <6h span) falls back to fee+spread breakeven
///   with low confidence — the old frontend behaviour, now labelled.
/// - Confidence blends history depth, trend R², volatility (CV penalty),
///   predicted-rate agreement, and depth-slope (spread-drift proxy).
#[derive(Debug, Clone, PartialEq)]
pub struct FundingHoldEstimate {
    /// Expected hold in hours (already clamped, finite, > 0).
    pub hours: f64,
    /// Minimum profitable hold (fee+spread breakeven) in hours.
    pub breakeven_hours: f64,
    /// Human-readable basis: decay slope, threshold, spread inclusion.
    pub reason: String,
    /// 0..1 heuristic confidence (never 0/1).
    pub confidence: f64,
}

pub fn estimate_funding_hold_hours(
    net_rate_8h: f64,
    round_trip_fee_bps: f64,
    entry_spread_cost_bps: Option<f64>,
    history: &[(i64, f64)],
    min_apr_pct: Option<f64>,
    predicted_agrees: Option<bool>,
    depth_slope_bps_per_1k: Option<f64>,
) -> Option<FundingHoldEstimate> {
    if !net_rate_8h.is_finite() || net_rate_8h <= 0.0 {
        return None;
    }
    if !round_trip_fee_bps.is_finite() || round_trip_fee_bps < 0.0 {
        return None;
    }
    let spread_cost = entry_spread_cost_bps
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(0.0);
    let total_cost_ratio = (round_trip_fee_bps + spread_cost) / 10_000.0;
    let net_hourly = net_rate_8h / 8.0;
    if !net_hourly.is_finite() || net_hourly <= 0.0 {
        return None;
    }
    let breakeven_total = total_cost_ratio / net_hourly;
    if !breakeven_total.is_finite() || breakeven_total <= 0.0 {
        return None;
    }
    const MAX_HOLD_HOURS: f64 = 168.0; // 7-day persistence cap
    let apr = min_apr_pct.filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(8.0);
    let threshold_8h = apr / 100.0 / 365.0 / 3.0;
    let spread_note = if spread_cost > 0.0 {
        format!(" incl. {spread_cost:.1} bps entry spread")
    } else {
        String::new()
    };

    // Thin history: honest fallback to breakeven.
    let span_hours = history
        .first()
        .zip(history.last())
        .map(|((a, _), (b, _))| (*b as f64 - *a as f64) / 3_600_000.0)
        .unwrap_or(0.0);
    if history.len() < 10 || !span_hours.is_finite() || span_hours < 6.0 {
        let mut conf: f64 = 0.30;
        match predicted_agrees {
            Some(true) => conf += 0.05,
            Some(false) => conf -= 0.05,
            None => {}
        }
        return Some(FundingHoldEstimate {
            hours: breakeven_total.clamp(0.1, MAX_HOLD_HOURS),
            breakeven_hours: breakeven_total,
            reason: format!(
                "fee+spread breakeven{spread_note}; insufficient funding history ({} prints, {:.1}h span) for persistence — dynamic spread drift deferred",
                history.len(),
                span_hours.max(0.0),
            ),
            confidence: conf.clamp(0.1, 0.95),
        });
    }

    // Least-squares trend of net_8h (fraction) vs time (hours since first).
    let t0 = history[0].0 as f64;
    let mut n = 0usize;
    let (mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0);
    for (ts, r) in history {
        if !r.is_finite() {
            continue;
        }
        let x = (*ts as f64 - t0) / 3_600_000.0;
        if !x.is_finite() {
            continue;
        }
        n += 1;
        sx += x;
        sy += r;
        sxx += x * x;
        sxy += x * r;
    }
    if n < 10 {
        let mut conf: f64 = 0.30;
        match predicted_agrees {
            Some(true) => conf += 0.05,
            Some(false) => conf -= 0.05,
            None => {}
        }
        return Some(FundingHoldEstimate {
            hours: breakeven_total.clamp(0.1, MAX_HOLD_HOURS),
            breakeven_hours: breakeven_total,
            reason: format!(
                "fee+spread breakeven{spread_note}; only {n} finite history points — persistence deferred"
            ),
            confidence: conf.clamp(0.1, 0.95),
        });
    }
    let nf = n as f64;
    let denom = nf * sxx - sx * sx;
    let slope = if denom.abs() < 1e-12 { 0.0 } else { (nf * sxy - sx * sy) / denom };
    let intercept = (sy - slope * sx) / nf;
    // R² + residual volatility.
    let mean_y = sy / nf;
    let (mut ss_tot, mut ss_res) = (0.0, 0.0);
    for (ts, r) in history {
        if !r.is_finite() {
            continue;
        }
        let x = (*ts as f64 - t0) / 3_600_000.0;
        let pred = intercept + slope * x;
        ss_tot += (r - mean_y).powi(2);
        ss_res += (r - pred).powi(2);
    }
    let r2 = if ss_tot > 1e-18 { (1.0 - ss_res / ss_tot).clamp(0.0, 1.0) } else { 0.0 };
    let std = (ss_res / nf).sqrt();
    let cv = if mean_y.abs() > 1e-12 { std / mean_y.abs() } else { 1.0 };

    // Slope is per-hour in 8h-fraction units; current = last observed (robust).
    let current = history.iter().rev().find_map(|(_, r)| r.is_finite().then_some(*r)).unwrap_or(net_rate_8h);
    let (hours_raw, mode): (f64, &str) = if slope >= 0.0 {
        (MAX_HOLD_HOURS, "stable/growing funding gap; 7-day persistence cap")
    } else {
        let t_cross = (threshold_8h - current) / slope; // slope<0 -> positive when current>threshold
        if !t_cross.is_finite() {
            (breakeven_total, "decay fit unstable; fee+spread breakeven")
        } else if t_cross < breakeven_total {
            (t_cross, "gap decays below min-APR threshold before fee+spread breakeven (early exit at loss)")
        } else {
            (t_cross.min(MAX_HOLD_HOURS), "linear decay of funding gap to min-APR threshold")
        }
    };
    // Volatility haircut: choppy gaps get shorter expected holds.
    let vol_mult = 1.0 / (1.0 + cv.max(0.0) * 0.5);
    let hours = (hours_raw * vol_mult).clamp(0.1, MAX_HOLD_HOURS);

    let mut conf: f64 = 0.50;
    if span_hours >= 48.0 {
        conf += 0.10;
    }
    if r2 >= 0.5 {
        conf += 0.10;
    } else if r2 < 0.2 {
        conf -= 0.10;
    }
    if cv > 0.5 {
        conf -= 0.10;
    }
    match predicted_agrees {
        Some(true) => conf += 0.10,
        Some(false) => conf -= 0.10,
        None => {}
    }
    if let Some(slope_cost) = depth_slope_bps_per_1k.filter(|v| v.is_finite() && *v > 0.0) {
        // Steep books mean spread drift can erase carry fast — disclose + penalise.
        if slope_cost > 0.5 {
            conf -= 0.05;
        }
    }
    let slope_bps_per_day = slope * 10_000.0 * 24.0;
    let reason = format!(
        "{mode}{spread_note}; slope {slope_bps_per_day:+.2} bps/day (R² {r2:.2}), threshold {:.4}%/8h, vol CV {cv:.2}",
        threshold_8h * 100.0,
    );
    Some(FundingHoldEstimate {
        hours,
        breakeven_hours: breakeven_total,
        reason,
        confidence: conf.clamp(0.1, 0.95),
    })
}

/// Expected net carry in USD over the modelled hold with linear funding-gap
/// decay (the number the scanner card shows instead of a flat daily rate).
///
/// Same decay story as [`estimate_funding_hold_hours`] and the popup's dashed
/// projection: the 8h net rate decays linearly from `net_rate_8h` to the
/// min-APR exit threshold over `hold_hours`, so accrual uses the average of
/// the two endpoints — not today's print extrapolated forever. Round-trip
/// fees plus measured entry spread are subtracted, so early-exit holds
/// honestly report a signed loss.
///
/// - `net_rate_8h`: current net print, fraction per 8h (e.g. 0.00052).
/// - `hold_hours`: modelled hold from `estimate_funding_hold_hours`.
///   `None` (unknown hold) returns `None` — a flat-rate extrapolation
///   disguised as an expectation is refused.
/// - Below-threshold current rates average at the current rate (exit would be
///   immediate; costs dominate).
/// - `min_apr_pct` defaults to 8.0, matching the hold model.
pub fn expected_carry_return_usd(
    net_rate_8h: f64,
    round_trip_fee_bps: f64,
    entry_spread_cost_bps: Option<f64>,
    hold_hours: Option<f64>,
    min_apr_pct: Option<f64>,
    trade_size_usd: f64,
) -> Option<f64> {
    if !(net_rate_8h.is_finite() && net_rate_8h > 0.0) {
        return None;
    }
    if !(round_trip_fee_bps.is_finite() && round_trip_fee_bps >= 0.0) {
        return None;
    }
    let hold = hold_hours.filter(|h| h.is_finite() && *h > 0.0)?;
    if !(trade_size_usd.is_finite() && trade_size_usd > 0.0) {
        return None;
    }
    let apr = min_apr_pct.filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(8.0);
    let threshold_8h = apr / 100.0 / 365.0 / 3.0;
    let exit_8h = threshold_8h.min(net_rate_8h);
    let avg_8h = (net_rate_8h + exit_8h) / 2.0;
    let accrual = trade_size_usd * avg_8h * (hold / 8.0);
    let spread = entry_spread_cost_bps
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(0.0);
    let costs = trade_size_usd * (round_trip_fee_bps + spread) / 10_000.0;
    let net = accrual - costs;
    net.is_finite().then_some(net)
}

/// Informational win probability for a funding-carry / basis opportunity.
///
/// Heuristic, NOT a calibrated forecast: under a memoryless assumption the
/// edge survives each hour with equal odds, so the chance it persists past
/// fee breakeven over a standard 24h hold is `coverage / (1 + coverage)`
/// where `coverage = 24h carry / round-trip fee = 24 / break_even_hours`.
/// A 4h breakeven scores ~0.86, 24h scores 0.50, 72h scores ~0.25.
///
/// Adjustments (small, documented):
/// - `predicted_agrees = Some(true)` (venue predicted rates confirm the edge
///   direction): lift `p += (1 - p) * 0.10`. `Some(false)`: `p *= 0.90`.
/// - `basis_buffer_bps = Some(b)` with `b > 0` (long-spot/short-perp basis
///   expected to converge at exit): `p += (1 - p) * min(b, 50) / 500`.
///
/// Returns `None` when the edge is non-positive or inputs are unusable.
/// Clamped to `[0.02, 0.98]` — never 0/1: this is guidance, not certainty.
/// Informational only: must never gate ordering or executability (same
/// contract as the spread monitor's `win_probability`).
pub fn funding_win_probability(
    net_rate_8h: Decimal,
    round_trip_fee_bps: Decimal,
    basis_buffer_bps: Option<Decimal>,
    predicted_agrees: Option<bool>,
) -> Option<f64> {
    if net_rate_8h <= dec!(0) || round_trip_fee_bps < dec!(0) {
        return None;
    }
    let rate_f: f64 = net_rate_8h.to_string().parse().ok()?;
    let fee_f: f64 = round_trip_fee_bps.to_string().parse().ok()?;
    if !rate_f.is_finite() || !fee_f.is_finite() || rate_f <= 0.0 || fee_f < 0.0 {
        return None;
    }
    // 3 funding periods/day; fee_ratio is dimensionless.
    let daily_carry = 3.0 * rate_f;
    let fee_ratio = fee_f / 10_000.0;
    if daily_carry <= 0.0 {
        return None;
    }
    // Zero-fee edge: carry is pure profit if it persists at all.
    let mut p = if fee_ratio <= 0.0 {
        0.95
    } else {
        let coverage = daily_carry / fee_ratio;
        if !coverage.is_finite() || coverage <= 0.0 {
            return None;
        }
        coverage / (1.0 + coverage)
    };
    match predicted_agrees {
        Some(true) => p += (1.0 - p) * 0.10,
        Some(false) => p *= 0.90,
        None => {}
    }
    if let Some(buffer) = basis_buffer_bps {
        if let Ok(buffer_f) = buffer.to_string().parse::<f64>() {
            if buffer_f.is_finite() && buffer_f > 0.0 {
                p += (1.0 - p) * (buffer_f.min(50.0) / 500.0);
            }
        }
    }
    if !p.is_finite() {
        return None;
    }
    Some(p.clamp(0.02, 0.98))
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
mod spot_perp_basis_tests {
    use super::*;
    use arbitrageur_core::types::VenueId;
    use chrono::Utc;

    fn perp_funding(venue: &str, symbol: &str, rate: &str, interval_hours: u32) -> FundingRate {
        FundingRate {
            venue: VenueId::new(venue),
            symbol: symbol.to_string(),
            rate: Decimal::from_str_exact(rate).unwrap(),
            predicted_rate: None,
            interval_hours,
            next_funding_time: Utc::now(),
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn positive_funding_reports_signed_basis() {
        let fr = perp_funding("lighter", "BTC-USDC", "0.0001", 8);
        let b = evaluate_spot_perp_basis(dec!(100.0), dec!(100.5), &fr, dec!(3.5), dec!(2000.0))
            .expect("basis");
        // (100.5-100)/100*10000 = +50 bps contango.
        assert!((b.basis_bps - dec!(50)).abs() < dec!(0.001));
        assert_eq!(b.spot_price, dec!(100.0));
        assert_eq!(b.perp_price, dec!(100.5));
        assert_eq!(b.carry.long_venue, "SPOT");
        assert!(b.carry.net_apr > dec!(0));
        assert!(b.carry.break_even_hours > dec!(0));
    }

    #[test]
    fn backwardation_still_reports_negative_basis() {
        let fr = perp_funding("lighter", "BTC-USDC", "0.0002", 8);
        let b = evaluate_spot_perp_basis(dec!(100.0), dec!(99.5), &fr, dec!(3.5), dec!(2000.0))
            .expect("basis");
        assert!(b.basis_bps < dec!(0));
    }

    #[test]
    fn non_positive_funding_is_none_no_borrow_desk() {
        let fr = perp_funding("lighter", "BTC-USDC", "0.0", 8);
        assert!(evaluate_spot_perp_basis(dec!(100.0), dec!(100.5), &fr, dec!(3.5), dec!(2000.0)).is_none());
        let neg = perp_funding("lighter", "BTC-USDC", "-0.0001", 8);
        assert!(evaluate_spot_perp_basis(dec!(100.0), dec!(100.5), &neg, dec!(3.5), dec!(2000.0)).is_none());
    }

    #[test]
    fn non_positive_prices_are_none() {
        let fr = perp_funding("lighter", "BTC-USDC", "0.0001", 8);
        assert!(evaluate_spot_perp_basis(dec!(0), dec!(100.5), &fr, dec!(3.5), dec!(2000.0)).is_none());
        assert!(evaluate_spot_perp_basis(dec!(100.0), dec!(0), &fr, dec!(3.5), dec!(2000.0)).is_none());
    }

    #[test]
    fn legacy_wrapper_delegates() {
        let fr = perp_funding("lighter", "BTC-USDC", "0.0001", 8);
        let via_legacy =
            evaluate_spot_perp_carry(dec!(100.0), dec!(100.5), &fr, dec!(3.5), dec!(2000.0))
                .expect("legacy");
        let via_new =
            evaluate_spot_perp_basis(dec!(100.0), dec!(100.5), &fr, dec!(3.5), dec!(2000.0))
                .expect("new");
        assert_eq!(via_legacy, via_new.carry);
    }
}

#[cfg(test)]
mod funding_win_probability_tests {
    use super::*;

    #[test]
    fn breakeven_anchors_match_documented_scale() {
        // fee 7.2bps, rate 0.00024/8h -> breakeven exactly 24h ->
        // coverage 1.0 -> P = 0.50.
        let p = funding_win_probability(dec!(0.00024), dec!(7.2), None, None).expect("p");
        assert!((p - 0.50).abs() < 0.02, "unexpected {p}");
        // Faster breakeven -> higher P; slower -> lower P (monotonic).
        let fast = funding_win_probability(dec!(0.001), dec!(7.2), None, None).expect("fast");
        let slow = funding_win_probability(dec!(0.000024), dec!(7.2), None, None).expect("slow");
        assert!(fast > p && p > slow, "fast={fast} p={p} slow={slow}");
    }

    #[test]
    fn adjustments_lift_or_trim() {
        let base = funding_win_probability(dec!(0.0001), dec!(7.2), None, None).expect("base");
        let agree = funding_win_probability(dec!(0.0001), dec!(7.2), None, Some(true)).expect("agree");
        let disagree = funding_win_probability(dec!(0.0001), dec!(7.2), None, Some(false)).expect("disagree");
        let buffered = funding_win_probability(dec!(0.0001), dec!(7.2), Some(dec!(40)), None).expect("buffered");
        assert!(agree > base && base > disagree, "agree={agree} base={base} disagree={disagree}");
        assert!(buffered > base, "buffered={buffered} base={base}");
    }

    #[test]
    fn hold_thin_history_falls_back_to_breakeven() {
        let est = super::estimate_funding_hold_hours(0.000585, 3.5, Some(0.0), &[], Some(8.0), None, None).unwrap();
        // fee 3.5bps / (0.0585%/8h) = ~4.8h breakeven
        assert!((est.hours - 4.8).abs() < 0.5, "got {}", est.hours);
        assert!(est.confidence < 0.5);
    }

    #[test]
    fn hold_decaying_gap_exits_before_breakeven_when_fast_decay() {
        // Steep decay: 0.10%/8h -> 0.01%/8h over 10h; threshold 8% APR ~0.0073%/8h.
        // Breakeven on 3.5bps at 0.0585%/8h would be ~4.8h, but gap collapses fast.
        let hist: Vec<(i64, f64)> = (0..20).map(|i| (i*1_800_000, 0.001 - i as f64*0.00005)).collect();
        let est = super::estimate_funding_hold_hours(0.000585, 3.5, Some(2.0), &hist, Some(8.0), Some(false), Some(0.1)).unwrap();
        assert!(est.hours < est.breakeven_hours, "decaying gap should exit early: hold {} vs be {} reason {}", est.hours, est.breakeven_hours, est.reason);
    }

    #[test]
    fn hold_stable_gap_caps_at_7d() {
        let hist: Vec<(i64, f64)> = (0..50).map(|i| (i*3_600_000, 0.0006)).collect();
        let est = super::estimate_funding_hold_hours(0.0006, 3.5, Some(0.0), &hist, Some(8.0), Some(true), Some(0.02)).unwrap();
        assert!(est.hours > est.breakeven_hours);
        assert!(est.hours <= 168.0 + 1e-9);
    }

    #[test]
    fn degenerate_inputs_are_none_and_bounds_hold() {
        assert_eq!(funding_win_probability(dec!(0), dec!(7.2), None, None), None);
        assert_eq!(funding_win_probability(dec!(-0.0001), dec!(7.2), None, None), None);
        // Extremes never reach 0/1.
        let huge = funding_win_probability(dec!(0.01), dec!(0.1), Some(dec!(1000)), Some(true)).expect("huge");
        let tiny = funding_win_probability(dec!(0.0000001), dec!(100.0), None, Some(false)).expect("tiny");
        assert!(huge <= 0.98 && tiny >= 0.02, "huge={huge} tiny={tiny}");
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


    #[test]
    fn expected_return_decays_to_threshold_over_hold() {
        // net 0.0008/8h, 8% APR threshold (7.3e-5), 80h hold, $50k, 10 bps fees.
        let v = expected_carry_return_usd(0.0008, 10.0, None, Some(80.0), Some(8.0), 50_000.0)
            .expect("value");
        let avg = (0.0008 + 8.0 / 100.0 / 365.0 / 3.0) / 2.0;
        let want = 50_000.0 * avg * 10.0 - 50.0;
        assert!((v - want).abs() < 1e-6, "got {v}, want {want}");
        assert!(v > 100.0, "decay-aware value well below the flat $400 extrapolation");
    }

    #[test]
    fn expected_return_is_signed_loss_when_below_threshold() {
        // Current print already below the exit threshold: immediate exit.
        let v = expected_carry_return_usd(0.000001, 10.0, None, Some(10.0), Some(8.0), 50_000.0)
            .expect("value");
        assert!(v < 0.0, "costs dominate: {v}");
        assert!((v + 50.0).abs() < 1.0, "≈ −fees: {v}");
    }

    #[test]
    fn expected_return_refuses_unknown_hold_and_garbage() {
        assert!(expected_carry_return_usd(0.0008, 10.0, None, None, Some(8.0), 50_000.0).is_none());
        assert!(expected_carry_return_usd(0.0, 10.0, None, Some(80.0), Some(8.0), 50_000.0).is_none());
        assert!(expected_carry_return_usd(0.0008, 10.0, None, Some(80.0), Some(8.0), 0.0).is_none());
        assert!(expected_carry_return_usd(f64::NAN, 10.0, None, Some(80.0), Some(8.0), 50_000.0).is_none());
        // Breakeven sanity: hold exactly one 8h period at 10 bps with 10 bps
        // costs nets ~zero.
        let v = expected_carry_return_usd(0.001, 10.0, None, Some(8.0), Some(1_000_000.0), 10_000.0)
            .expect("value");
        assert!(v.abs() < 1e-6, "{v}");
    }
}
