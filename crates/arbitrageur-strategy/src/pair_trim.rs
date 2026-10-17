//! Shared pair-position decision helpers: leftover sweep, funding-bot gates,
//! and the universal liquidation-distance trim block.
//!
//! Every function here is pure and paper-safe by construction: invalid inputs
//! fail closed (no action), never fabricate quantities, and never touch live
//! execution. Position-sizing callers convert to their own numeric types.

/// Residual epsilon for delta-neutrality comparisons. Mirrors the 1e-9
/// paper-dust tolerance used by the flatten entry guards: f64/Decimal ledger
/// conversions can strand sub-1e-9 base-unit residue that is economically
/// meaningless on every listed venue.
pub const PAIR_DUST_EPS: f64 = 1e-9;

/// Leftover-sweep rule for close/unwind sizing.
///
/// When a close is requested for `requested_qty` (e.g. one grid unit) but the
/// attributed residual is larger by less than one full `step_qty`, closing
/// only `requested_qty` would strand a sub-step remnant that can never be
/// closed on its own. In that case close the ENTIRE residual instead.
///
/// Concretely: `residual <= requested` → `residual` (exact or over-covering
/// close, unchanged); `residual - requested < step` → `residual` (sweep);
/// otherwise → `requested` (a real multi-step remainder stays for later
/// closes). Invalid inputs fail closed: nonpositive residual closes nothing,
/// nonpositive requested sizes nothing, and a nonpositive step disables the
/// sweep (plain `min(residual, requested)` semantics).
pub fn sweep_close_qty(residual_qty: f64, requested_qty: f64, step_qty: f64) -> f64 {
    if !residual_qty.is_finite() || residual_qty <= 0.0 {
        return 0.0;
    }
    if !requested_qty.is_finite() || requested_qty <= 0.0 {
        return 0.0;
    }
    if requested_qty >= residual_qty {
        return residual_qty;
    }
    if step_qty.is_finite() && step_qty > 0.0 && residual_qty - requested_qty < step_qty {
        return residual_qty;
    }
    requested_qty
}

/// Funding entry-spread gate: refuse entry when the gap between the venues'
/// execution prices (`entry_spread_bps`) exceeds `max_entry_spread_bps`.
/// Misconfigured caps and unmeasurable spreads refuse (fail closed).
pub fn entry_spread_refused(entry_spread_bps: f64, max_entry_spread_bps: f64) -> bool {
    if !max_entry_spread_bps.is_finite() || max_entry_spread_bps <= 0.0 {
        return true;
    }
    if !entry_spread_bps.is_finite() || entry_spread_bps < 0.0 {
        return true;
    }
    entry_spread_bps > max_entry_spread_bps
}

/// Funding auto-exit gate: exit when the live inter-venue spread
/// (`live_spread_pct`, percent) falls below `threshold_pct`. A `None`
/// threshold disables the gate; out-of-range thresholds never trigger.
pub fn exit_spread_triggered(live_spread_pct: f64, threshold_pct: Option<f64>) -> bool {
    let Some(t) = threshold_pct else { return false };
    if !t.is_finite() || t <= 0.0 || t > 100.0 {
        return false;
    }
    if !live_spread_pct.is_finite() {
        return false;
    }
    live_spread_pct < t
}

/// Verdict of the universal liquidation-distance trim block. When EITHER leg
/// of a neutral pair comes within `threshold_pct` (percent) of liquidation,
/// BOTH legs are trimmed by the same quantity to stay neutral. If the
/// remainder would fall below one full step (the min tradable unit), both
/// legs close entirely instead of stranding dust.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PairTrimDecision {
    Hold,
    TrimBoth { qty: f64 },
    CloseBoth,
}

/// Shared trim evaluator behind every bot configurator's trim block.
/// `min_liq_distance_pct` is the SMALLER of the two legs' liquidation
/// distances (either leg breaching arms the trim). `position_qty` is the
/// per-leg open quantity; `step_qty` is one full step / min tradable unit.
/// `None` (or nonpositive) threshold disables the block. Invalid quantities
/// hold (fail closed — no automatic action on bad telemetry).
pub fn evaluate_liq_trim(
    min_liq_distance_pct: f64,
    position_qty: f64,
    step_qty: f64,
    threshold_pct: Option<f64>,
) -> PairTrimDecision {
    let Some(t) = threshold_pct else {
        return PairTrimDecision::Hold;
    };
    if !t.is_finite() || t <= 0.0 || t > 100.0 {
        return PairTrimDecision::Hold;
    }
    if !min_liq_distance_pct.is_finite()
        || !position_qty.is_finite()
        || position_qty <= 0.0
        || !step_qty.is_finite()
        || step_qty <= 0.0
    {
        return PairTrimDecision::Hold;
    }
    if min_liq_distance_pct > t {
        return PairTrimDecision::Hold;
    }
    let trim_qty = step_qty.min(position_qty);
    if position_qty - trim_qty < step_qty {
        return PairTrimDecision::CloseBoth;
    }
    PairTrimDecision::TrimBoth { qty: trim_qty }
}

/// Proportional liquidation-distance trim block: computes the exact minimal quantity to trim
/// from BOTH legs to restore the threatened venue's margin buffer ratio (MBR) to the target level.
///
/// Formula:
///   ΔQ* = [(1 + MBR*) * Q * P * MMR - Equity] / [P * ((1 + MBR*) * MMR - f_taker)]
pub fn evaluate_proportional_liq_trim(
    current_qty: f64,
    mark_price: f64,
    threatened_equity: f64,
    threatened_mmr: f64,    // e.g. 0.05 for 5% maintenance margin
    target_buffer_pct: f64, // target buffer ratio e.g. 75.0%
    taker_fee_rate: f64,    // e.g. 0.0005 for 5 bps
    step_qty: f64,
) -> PairTrimDecision {
    if !current_qty.is_finite()
        || current_qty <= 0.0
        || !mark_price.is_finite()
        || mark_price <= 0.0
        || !threatened_equity.is_finite()
        || !threatened_mmr.is_finite()
        || threatened_mmr <= 0.0
        || !step_qty.is_finite()
        || step_qty <= 0.0
    {
        return PairTrimDecision::Hold;
    }

    if threatened_equity <= 0.0 {
        return PairTrimDecision::CloseBoth;
    }

    let target_mbr = target_buffer_pct / 100.0;
    let required_margin = current_qty * mark_price * threatened_mmr;
    let current_mbr = (threatened_equity - required_margin) / required_margin;

    // Buffer is already healthy: hold
    if current_mbr >= target_mbr {
        return PairTrimDecision::Hold;
    }

    let numerator =
        (1.0 + target_mbr) * current_qty * mark_price * threatened_mmr - threatened_equity;
    let denominator = mark_price * ((1.0 + target_mbr) * threatened_mmr - taker_fee_rate);

    if denominator <= 0.0 {
        return PairTrimDecision::CloseBoth;
    }

    let raw_trim = numerator / denominator;
    if raw_trim <= 0.0 {
        return PairTrimDecision::Hold;
    }

    if raw_trim >= current_qty || (current_qty - raw_trim) < step_qty - PAIR_DUST_EPS {
        return PairTrimDecision::CloseBoth;
    }

    // Snap up to whole steps
    let steps = (raw_trim / step_qty).ceil();
    let snapped_qty = (steps * step_qty).min(current_qty);

    if snapped_qty >= current_qty || (current_qty - snapped_qty) < step_qty - PAIR_DUST_EPS {
        return PairTrimDecision::CloseBoth;
    }

    PairTrimDecision::TrimBoth { qty: snapped_qty }
}

/// Which leg of a neutral pair is overweight, and by how much.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OverweightLeg {
    Long,
    Short,
}

/// Auto-rebalance to neutral: reduce the overweight leg by the full
/// overweight amount so both legs match again. Returns `None` when the pair
/// is already neutral (within dust) or inputs are invalid.
pub fn rebalance_to_neutral(long_qty: f64, short_qty: f64) -> Option<(OverweightLeg, f64)> {
    if !long_qty.is_finite() || !short_qty.is_finite() || long_qty < 0.0 || short_qty < 0.0 {
        return None;
    }
    let diff = long_qty - short_qty;
    if diff.abs() <= PAIR_DUST_EPS {
        return None;
    }
    if diff > 0.0 {
        Some((OverweightLeg::Long, diff))
    } else {
        Some((OverweightLeg::Short, -diff))
    }
}

/// Beta-aware statistical arbitrage pair rebalancing:
/// Targets Q_x* = Q_y * |beta|. Trims the overweight leg to restore neutrality.
pub fn rebalance_stat_arb_pair_to_neutral(
    qty_y: f64,
    qty_x: f64,
    beta: f64,
) -> Option<(OverweightLeg, f64)> {
    if !qty_y.is_finite()
        || !qty_x.is_finite()
        || !beta.is_finite()
        || qty_y < 0.0
        || qty_x < 0.0
        || beta.abs() < 1e-4
    {
        return None;
    }
    let target_qty_x = qty_y * beta.abs();
    let diff_x = qty_x - target_qty_x;
    if diff_x.abs() <= PAIR_DUST_EPS {
        return None;
    }
    if diff_x > 0.0 {
        Some((OverweightLeg::Short, diff_x)) // Leg X is overweight
    } else {
        Some((OverweightLeg::Long, -diff_x / beta.abs())) // Leg Y is overweight relative to Leg X
    }
}

/// Opportunity rotation gate: exit the current funding position to enter a
/// higher-yield one only when the net-of-friction improvement exceeds
/// `threshold_bps`. Invalid inputs (or a nonpositive threshold) never rotate.
pub fn rotation_improves(
    current_net_bps: f64,
    candidate_net_bps: f64,
    friction_bps: f64,
    threshold_bps: f64,
) -> bool {
    if !current_net_bps.is_finite()
        || !candidate_net_bps.is_finite()
        || !friction_bps.is_finite()
        || friction_bps < 0.0
        || !threshold_bps.is_finite()
        || threshold_bps <= 0.0
    {
        return false;
    }
    candidate_net_bps - current_net_bps - friction_bps > threshold_bps
}

/// Closed-form mathematical hysteresis rotation rule:
/// Evaluates whether rotating from an active carry asset to a candidate asset
/// yields an excess return that exceeds the decay-adjusted switching friction
/// plus a safety hysteresis margin, preventing fee churn between similar-yielding assets.
pub fn rotation_improves_with_hysteresis(
    current_apr_pct: f64,
    candidate_apr_pct: f64,
    switching_friction_bps: f64,
    candidate_halflife_hours: f64,
    hysteresis_buffer_apr_pct: f64,
    hold_duration_hours: f64,
    min_hold_hours: f64,
) -> bool {
    if !current_apr_pct.is_finite()
        || !candidate_apr_pct.is_finite()
        || !switching_friction_bps.is_finite()
        || switching_friction_bps < 0.0
        || !candidate_halflife_hours.is_finite()
        || candidate_halflife_hours <= 0.0
        || !hysteresis_buffer_apr_pct.is_finite()
        || hysteresis_buffer_apr_pct < 0.0
        || !hold_duration_hours.is_finite()
        || hold_duration_hours < 0.0
        || !min_hold_hours.is_finite()
        || min_hold_hours < 0.0
    {
        return false;
    }

    // Emergency exit if current position has flipped to negative carry
    if current_apr_pct < 0.0 {
        return candidate_apr_pct > 0.0;
    }

    // Must satisfy minimum holding period (e.g. 8-16 hours) to prevent churn
    if hold_duration_hours < min_hold_hours {
        return false;
    }

    // Effective horizon H_eff = (1 - e^(-lambda * H_max)) / lambda (capped at 168 hours = 7 days)
    let lambda = (std::f64::consts::LN_2 / candidate_halflife_hours).max(0.001);
    let h_eff = (1.0 - (-lambda * 168.0).exp()) / lambda;

    // Convert switching friction (bps) to required annual excess yield (% APR)
    // friction_bps / 10,000 = decimal cost.
    // decimal_cost / h_eff = cost per hour.
    // cost per hour * 8760 = annualized friction hurdle.
    let friction_fraction = switching_friction_bps / 10_000.0;
    let friction_apr_hurdle_pct = (friction_fraction * 8760.0 / h_eff) * 100.0;
    let total_hurdle_apr_pct = friction_apr_hurdle_pct + hysteresis_buffer_apr_pct;

    candidate_apr_pct - current_apr_pct > total_hurdle_apr_pct
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweep_closes_sub_step_leftover() {
        // 1.4-step residual on a 1-unit close: strand would be 0.4 steps.
        assert!((sweep_close_qty(1.4, 1.0, 1.0) - 1.4).abs() < 1e-12);
    }

    #[test]
    fn sweep_leaves_multi_step_remainder() {
        // 2.4-step residual on a 1-unit close: 1.4 steps remain — a real
        // remainder, so the requested unit closes unchanged.
        assert!((sweep_close_qty(2.4, 1.0, 1.0) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn sweep_exact_and_over_covering_close_unchanged() {
        assert!((sweep_close_qty(1.0, 1.0, 1.0) - 1.0).abs() < 1e-12);
        assert!((sweep_close_qty(0.7, 1.0, 1.0) - 0.7).abs() < 1e-12);
    }

    #[test]
    fn sweep_boundary_is_strictly_below_one_step() {
        // Strand of exactly one step is closable on its own — no sweep.
        assert!((sweep_close_qty(2.0, 1.0, 1.0) - 1.0).abs() < 1e-12);
        // Anything below one step sweeps.
        assert!((sweep_close_qty(1.999, 1.0, 1.0) - 1.999).abs() < 1e-12);
    }

    #[test]
    fn sweep_invalid_inputs_fail_closed() {
        assert_eq!(sweep_close_qty(0.0, 1.0, 1.0), 0.0);
        assert_eq!(sweep_close_qty(-1.0, 1.0, 1.0), 0.0);
        assert_eq!(sweep_close_qty(1.4, 0.0, 1.0), 0.0);
        assert_eq!(sweep_close_qty(1.4, f64::NAN, 1.0), 0.0);
        // No valid step: plain min semantics, no sweep.
        assert!((sweep_close_qty(1.4, 1.0, 0.0) - 1.0).abs() < 1e-12);
        assert!((sweep_close_qty(1.4, 1.0, f64::NAN) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn entry_spread_gate() {
        assert!(!entry_spread_refused(10.0, 50.0));
        assert!(entry_spread_refused(50.01, 50.0));
        assert!(!entry_spread_refused(50.0, 50.0));
        // Misconfigured cap or unmeasurable spread refuses.
        assert!(entry_spread_refused(10.0, 0.0));
        assert!(entry_spread_refused(10.0, f64::NAN));
        assert!(entry_spread_refused(f64::NAN, 50.0));
        assert!(entry_spread_refused(-1.0, 50.0));
    }

    #[test]
    fn exit_spread_gate() {
        assert!(exit_spread_triggered(4.9, Some(5.0)));
        assert!(!exit_spread_triggered(5.0, Some(5.0)));
        assert!(!exit_spread_triggered(5.1, Some(5.0)));
        assert!(!exit_spread_triggered(1.0, None));
        assert!(!exit_spread_triggered(1.0, Some(0.0)));
        assert!(!exit_spread_triggered(1.0, Some(101.0)));
        assert!(!exit_spread_triggered(f64::NAN, Some(5.0)));
    }

    #[test]
    fn liq_trim_disabled_or_invalid_holds() {
        assert_eq!(
            evaluate_liq_trim(2.0, 3.0, 1.0, None),
            PairTrimDecision::Hold
        );
        assert_eq!(
            evaluate_liq_trim(2.0, 3.0, 1.0, Some(0.0)),
            PairTrimDecision::Hold
        );
        assert_eq!(
            evaluate_liq_trim(2.0, 3.0, 1.0, Some(101.0)),
            PairTrimDecision::Hold
        );
        // Comfortably far from liquidation holds.
        assert_eq!(
            evaluate_liq_trim(20.0, 3.0, 1.0, Some(5.0)),
            PairTrimDecision::Hold
        );
        // Bad telemetry holds.
        assert_eq!(
            evaluate_liq_trim(f64::NAN, 3.0, 1.0, Some(5.0)),
            PairTrimDecision::Hold
        );
        assert_eq!(
            evaluate_liq_trim(2.0, 0.0, 1.0, Some(5.0)),
            PairTrimDecision::Hold
        );
    }

    #[test]
    fn liq_trim_boundary_trim_vs_close() {
        // 3 steps open, trim one, 2 remain (>= step): trim both by one step.
        assert_eq!(
            evaluate_liq_trim(4.0, 3.0, 1.0, Some(5.0)),
            PairTrimDecision::TrimBoth { qty: 1.0 }
        );
        // 1 step open: remainder would be 0 (< step): close both.
        assert_eq!(
            evaluate_liq_trim(4.0, 1.0, 1.0, Some(5.0)),
            PairTrimDecision::CloseBoth
        );
        // 1.4 steps open: remainder 0.4 (< step): close both.
        assert_eq!(
            evaluate_liq_trim(4.0, 1.4, 1.0, Some(5.0)),
            PairTrimDecision::CloseBoth
        );
        // Exactly at the threshold arms the trim.
        assert_eq!(
            evaluate_liq_trim(5.0, 3.0, 1.0, Some(5.0)),
            PairTrimDecision::TrimBoth { qty: 1.0 }
        );
    }

    #[test]
    fn rebalance_neutral_and_overweight() {
        assert_eq!(rebalance_to_neutral(1.0, 1.0), None);
        let (leg, qty) = rebalance_to_neutral(1.4, 1.0).expect("long overweight");
        assert_eq!(leg, OverweightLeg::Long);
        assert!((qty - 0.4).abs() < 1e-9, "qty: {}", qty);
        let (leg, qty) = rebalance_to_neutral(1.0, 1.6).expect("short overweight");
        assert_eq!(leg, OverweightLeg::Short);
        assert!((qty - 0.6).abs() < 1e-9, "qty: {}", qty);
        assert_eq!(rebalance_to_neutral(f64::NAN, 1.0), None);
        assert_eq!(rebalance_to_neutral(-1.0, 1.0), None);
    }

    #[test]
    fn rotation_net_of_friction_gate() {
        // +10 bps candidate, 2 bps friction, 5 bps hurdle: 10-0-2=8 > 5.
        assert!(rotation_improves(0.0, 10.0, 2.0, 5.0));
        // Improvement exactly at the hurdle does not rotate.
        assert!(!rotation_improves(0.0, 7.0, 2.0, 5.0));
        // Friction eats the edge.
        assert!(!rotation_improves(8.0, 10.0, 2.0, 5.0));
        // Invalid inputs never rotate.
        assert!(!rotation_improves(0.0, 10.0, 2.0, 0.0));
        assert!(!rotation_improves(f64::NAN, 10.0, 2.0, 5.0));
        assert!(!rotation_improves(0.0, 10.0, -1.0, 5.0));
    }

    #[test]
    fn test_proportional_liq_trim_scenarios() {
        // Position: 10 SOL at $100 = $1,000 notional.
        // MMR = 5% ($50 margin requirement).
        // Equity = $60 (Buffer = ($60 - $50)/$50 = 20% < 75% target).
        // Step size = 1.0 SOL.
        let decision = evaluate_proportional_liq_trim(
            10.0,   // current_qty
            100.0,  // mark_price
            60.0,   // threatened_equity
            0.05,   // MMR
            75.0,   // target_buffer_pct (0.75)
            0.0005, // taker_fee_rate (5 bps)
            1.0,    // step_qty
        );
        match decision {
            PairTrimDecision::TrimBoth { qty } => {
                assert!(
                    (1.0..10.0).contains(&qty),
                    "expected partial trim, got {}",
                    qty
                );
            }
            other => panic!("expected TrimBoth, got {:?}", other),
        }

        // When buffer is already healthy (equity = $100 -> MBR = 100% > 75%):
        assert_eq!(
            evaluate_proportional_liq_trim(10.0, 100.0, 100.0, 0.05, 75.0, 0.0005, 1.0),
            PairTrimDecision::Hold
        );

        // When equity is negative or zero (insolvent):
        assert_eq!(
            evaluate_proportional_liq_trim(10.0, 100.0, 0.0, 0.05, 75.0, 0.0005, 1.0),
            PairTrimDecision::CloseBoth
        );

        // When position is small (1.0) and trimming would leave < step_qty:
        assert_eq!(
            evaluate_proportional_liq_trim(1.0, 100.0, 2.0, 0.05, 75.0, 0.0005, 1.0),
            PairTrimDecision::CloseBoth
        );
    }

    #[test]
    fn rotation_with_hysteresis_churn_prevention() {
        // Current: 20% APR, Candidate: 22% APR (+2% delta)
        // 28 bps friction, 48h half-life, 5% hysteresis buffer.
        // Even held for 24h (> 16h min), a +2% delta is rejected because friction hurdle requires ~43% excess APR.
        assert!(!rotation_improves_with_hysteresis(
            20.0, // current_apr
            22.0, // candidate_apr
            28.0, // switching_friction_bps
            48.0, // halflife_hours
            5.0,  // hysteresis_buffer_apr
            24.0, // hold_duration_hours
            16.0  // min_hold_hours
        ));

        // When candidate is a massive breakout yielding 120% APR (+100% delta):
        assert!(rotation_improves_with_hysteresis(
            20.0, 120.0, 28.0, 48.0, 5.0, 24.0, 16.0
        ));

        // Premature rotation before min_hold_hours (e.g. held only 2 hours):
        assert!(!rotation_improves_with_hysteresis(
            20.0, 120.0, 28.0, 48.0, 5.0, 2.0, 16.0
        ));

        // Emergency flip: current turned negative (-5% APR):
        assert!(rotation_improves_with_hysteresis(
            -5.0, 10.0, 28.0, 48.0, 5.0, 1.0, 16.0
        ));
    }

    #[test]
    fn test_rebalance_stat_arb_pair_to_neutral() {
        // 1 BTC vs 15 ETH with beta = 15.0 -> perfectly neutral
        assert_eq!(rebalance_stat_arb_pair_to_neutral(1.0, 15.0, 15.0), None);

        // 1 BTC vs 18 ETH with beta = 15.0 -> ETH (leg X) overweight by 3.0
        let res = rebalance_stat_arb_pair_to_neutral(1.0, 18.0, 15.0);
        assert_eq!(res, Some((OverweightLeg::Short, 3.0)));

        // 1 BTC vs 12 ETH with beta = 15.0 -> BTC (leg Y) overweight by 0.2 BTC (3 ETH equivalent)
        let res_y = rebalance_stat_arb_pair_to_neutral(1.0, 12.0, 15.0);
        assert_eq!(res_y, Some((OverweightLeg::Long, 0.2)));

        // Degenerate inputs fail safe
        assert_eq!(rebalance_stat_arb_pair_to_neutral(-1.0, 10.0, 1.0), None);
        assert_eq!(rebalance_stat_arb_pair_to_neutral(1.0, 10.0, 0.0), None);
    }
}
