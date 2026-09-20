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
}
