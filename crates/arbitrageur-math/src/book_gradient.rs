//! Orderbook shape (gradient) metrics for liquidity, slippage and capacity.
//!
//! What the engine already captures (static depth gradient):
//! - `OrderBook::{buy,sell}_execution_vwap` walks price levels to VWAP-price
//!   an exact quantity (spatial spread).
//! - `funding_entry_capacity` lockstep-walks both legs across 0.5x/1x/2x
//!   tiers, so deeper (worse) levels price the entry cross.
//! - the monitor's `two_way_exit_estimate` prices the minimum executable
//!   close including the reverse side.
//!
//! What was MISSING and lives here: an explicit *marginal* gradient — how
//! fast execution cost grows per extra dollar of size (slope), plus the
//! top-of-book imbalance and microprice that say which side is heavier.
//! Snapshots are still point-in-time (hub TTL): no replenishment, market
//! impact beyond the visible book, or time decay is modelled. All outputs
//! are informational: they describe cost shape, never authorize size.

/// Gradient read-out for one side-pair of books at a reference notional.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BookGradient {
    /// Marginal entry-cost slope in bps per extra $1,000 notional, summed
    /// over both legs (ask-side worsening + bid-side worsening). 0 = flat
    /// book (cost does not grow with size); larger = thinner tail.
    pub slope_bps_per_1k: f64,
    /// Top-of-book imbalance `(bid_depth - ask_depth) / (bid_depth + ask_depth)`
    /// over the first 5 levels of each side, in [-1, 1]. Positive = bid-heavy
    /// (easier to sell into, harder to buy from).
    pub imbalance: f64,
    /// Microprice `(ask * bid_size + bid * ask_size) / (bid_size + ask_size)`
    /// at the top level — the size-weighted fair anchor vs the plain mid.
    pub microprice: f64,
    /// Notional actually covered by the walk (<= requested depth_usd).
    pub covered_usd: f64,
}

/// Walk one side's levels accumulating `(cum_notional_usd, slip_bps)` points
/// and return the marginal slope between the half-depth and full-depth marks
/// (bps per $1k), plus covered notional. `is_ask` selects the reference:
/// asks compare against the best ask, bids against the best bid.
fn side_slope(levels: &[(f64, f64)], depth_usd: f64, is_ask: bool) -> Option<(f64, f64)> {
    if levels.is_empty() || !(depth_usd.is_finite() && depth_usd > 0.0) {
        return None;
    }
    let ref_px = levels[0].0;
    if !ref_px.is_finite() || ref_px <= 0.0 {
        return None;
    }
    // Fixed fractional stepping (10 notional chunks) so the sample count —
    // and hence the slope read-out — does not depend on how lumpy the
    // levels are. One giant level still yields a full 10-sample walk.
    let step_usd = depth_usd / 10.0;
    let mut li = 0usize;
    let mut level_rem = 0.0f64;
    let mut cum_usd = 0.0;
    let mut cum_qty = 0.0;
    let mut cum_cost = 0.0;
    let mut samples: Vec<(f64, f64)> = Vec::new();
    for _ in 0..10 {
        let mut need = step_usd;
        while need > 1e-9 {
            if level_rem <= 1e-9 {
                if li >= levels.len() {
                    break;
                }
                let (px, qty) = levels[li];
                li += 1;
                if !px.is_finite() || !qty.is_finite() || px <= 0.0 || qty <= 0.0 {
                    continue;
                }
                level_rem = qty;
            }
            let (px, _) = levels[li - 1];
            let take_qty = level_rem.min(need / px);
            if take_qty <= 0.0 {
                break;
            }
            level_rem -= take_qty;
            need -= take_qty * px;
            cum_qty += take_qty;
            cum_cost += take_qty * px;
            cum_usd += take_qty * px;
        }
        if cum_qty > 0.0 {
            let vwap = cum_cost / cum_qty;
            let slip = if is_ask {
                (vwap - ref_px) / ref_px * 10_000.0
            } else {
                (ref_px - vwap) / ref_px * 10_000.0
            };
            if slip.is_finite() {
                samples.push((cum_usd, slip));
            }
        }
        if li >= levels.len() && level_rem <= 1e-9 {
            break;
        }
    }
    if samples.len() < 2 || cum_usd <= 0.0 {
        return None;
    }
    // Interpolate slip at half and full covered depth for a stable slope.
    let at = |target: f64| -> f64 {
        let mut prev = samples[0];
        for s in &samples {
            if s.0 >= target {
                if (s.0 - prev.0).abs() < 1e-9 {
                    return s.1;
                }
                let t = (target - prev.0) / (s.0 - prev.0);
                return prev.1 + t * (s.1 - prev.1);
            }
            prev = *s;
        }
        samples.last().map(|s| s.1).unwrap_or(0.0)
    };
    let half = cum_usd / 2.0;
    let slope = (at(cum_usd) - at(half)) / half * 1000.0;
    if !slope.is_finite() {
        return None;
    }
    if slope < 0.0 {
        // Defensive: crossed/stale levels can invert the walk; a negative
        // "bigger size is cheaper" read-out must never ship.
        return Some((0.0, cum_usd));
    }
    Some((slope, cum_usd))
}

/// Gradient of an entry that lifts `asks` (long leg) and hits `bids`
/// (short leg) for `depth_usd` notional.
///
/// Returns `None` when either side is missing/thin (fewer than 2 walk
/// samples) — fail closed, never a zero-slope "looks liquid" read-out.
pub fn entry_gradient(
    asks: &[(f64, f64)],
    bids: &[(f64, f64)],
    depth_usd: f64,
) -> Option<BookGradient> {
    if asks.is_empty() || bids.is_empty() {
        return None;
    }
    let (ask_slope, ask_covered) = side_slope(asks, depth_usd, true)?;
    let (bid_slope, bid_covered) = side_slope(bids, depth_usd, false)?;
    // Imbalance over the first 5 levels (notional-weighted).
    let top_notion = |levels: &[(f64, f64)]| {
        levels
            .iter()
            .take(5)
            .filter_map(|(px, qty)| {
                if px.is_finite() && qty.is_finite() && *px > 0.0 && *qty > 0.0 {
                    Some(px * qty)
                } else {
                    None
                }
            })
            .sum::<f64>()
    };
    let bid_n = top_notion(bids);
    let ask_n = top_notion(asks);
    let imbalance = if bid_n + ask_n > 0.0 {
        (bid_n - ask_n) / (bid_n + ask_n)
    } else {
        0.0
    };
    // Microprice at the top level.
    let (bb_px, bb_qty) = bids[0];
    let (ba_px, ba_qty) = asks[0];
    let denom = bb_qty + ba_qty;
    let microprice = if denom.is_finite() && denom > 0.0 {
        (ba_px * bb_qty + bb_px * ba_qty) / denom
    } else {
        (bb_px + ba_px) / 2.0
    };
    if !microprice.is_finite() {
        return None;
    }
    Some(BookGradient {
        slope_bps_per_1k: ask_slope + bid_slope,
        imbalance,
        microprice,
        covered_usd: ask_covered.min(bid_covered),
    })
}

#[cfg(test)]
mod book_gradient_tests {
    use super::*;

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn flat_book_reports_near_zero_slope() {
        // 100 deep flat levels: cost must not grow with size.
        let asks = vec![(100.0, 1000.0); 100];
        let bids = vec![(99.9, 1000.0); 100];
        let g = entry_gradient(&asks, &bids, 50_000.0).expect("gradient");
        assert!(g.slope_bps_per_1k < 0.01, "slope={}", g.slope_bps_per_1k);
        assert!(approx(g.imbalance, 0.0, 0.01), "imb={}", g.imbalance);
        assert!(g.microprice > 99.9 && g.microprice < 100.0);
        assert!(approx(g.covered_usd, 50_000.0, 1.0));
    }

    #[test]
    fn linear_tail_reports_theoretical_slope() {
        // Each level worsens by 0.01 on a ~100 asset with qty 10 (~$1k per
        // level): slip grows ~1bp per $1k walked per side, so the two-leg
        // slope must read ~2 bps per $1k. (A top-cliff book would NOT score
        // here by design: once past the cliff the marginal cost is flat —
        // the cliff's damage shows in the VWAP cross, not the margin.)
        let asks: Vec<(f64, f64)> = (0..200).map(|k| (100.0 + 0.01 * k as f64, 10.0)).collect();
        let bids: Vec<(f64, f64)> = (0..200).map(|k| (99.9 - 0.01 * k as f64, 10.0)).collect();
        let g = entry_gradient(&asks, &bids, 50_000.0).expect("gradient");
        assert!(
            (1.0..=3.0).contains(&g.slope_bps_per_1k),
            "slope={}",
            g.slope_bps_per_1k
        );
        let flat_asks = vec![(100.0, 1000.0); 100];
        let flat_bids = vec![(99.9, 1000.0); 100];
        let flat = entry_gradient(&flat_asks, &flat_bids, 50_000.0).expect("flat");
        assert!(g.slope_bps_per_1k > flat.slope_bps_per_1k + 0.5);
    }

    #[test]
    fn imbalance_sign_follows_heavier_side() {
        let asks = vec![(100.0, 1.0); 5];
        let bids = vec![(99.9, 10.0); 5];
        let g = entry_gradient(&asks, &bids, 500.0).expect("gradient");
        assert!(g.imbalance > 0.5, "imb={}", g.imbalance);
    }

    #[test]
    fn thin_or_empty_books_fail_closed() {
        assert_eq!(entry_gradient(&[], &[(99.9, 1.0)], 1000.0), None);
        assert_eq!(entry_gradient(&[(100.0, 1.0)], &[], 1000.0), None);
        // Single level each: fewer than 2 walk samples -> None, never a
        // fabricated zero slope.
        assert_eq!(
            entry_gradient(&[(100.0, 1.0)], &[(99.9, 1.0)], 1000.0),
            None
        );
    }
}
