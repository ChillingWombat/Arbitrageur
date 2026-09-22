//! Live/external balance attribution: own trades vs manual flows.
//!
//! Paper equity is fills-only by construction (the simulator mutates equity
//! exclusively in `place_order`, and `reset_account` moves the baseline), so
//! this detector exists to catch everything else: manual deposits,
//! withdrawals, transfers, and funding settlements observed on a real
//! `fetch_account` poll. A nonzero actual-vs-expected delta emits one
//! `account_equity_events` row and rebases the baseline so total PnL never
//! absorbs manual flows. Comparison gaps (missed polls) emit no event, only
//! a coverage note.

/// Poll-gap ceiling: when the previous comparison is older than this, the
/// window is discontinuous, so the check rebases silently with a coverage
/// note instead of attributing the move.
pub const EQUITY_POLL_GAP_MAX_MS: i64 = 300_000;

/// Noise deadband: absolute deltas at or below this are reporting noise, not
/// external flows. Venue equity readouts wobble — observed symmetric
/// oscillation of ±$0.02–0.21 across consecutive polls on flat accounts —
/// and classifying that as deposit/withdrawal floods `account_equity_events`
/// with phantom flows (thousands observed). Real external flows are
/// dollars-scale, so a quarter of deadband loses no information.
pub const EQUITY_DELTA_EPS_USD: f64 = 0.25;

/// Absolute deltas at or below this with open positions are classified as
/// funding settlements rather than manual transfers.
pub const FUNDING_GUESS_MAX_USD: f64 = 10.0;

/// Outcome of one actual-vs-expected comparison.
#[derive(Debug, Clone, PartialEq)]
pub struct EquityCheckOutcome {
    pub delta: f64,
    pub event_kind: Option<String>,
    pub rebased: bool,
    pub note: Option<String>,
}

/// Classify a nonzero delta by sign with a position-delta cross-check: a
/// changed position signature means the move rode alongside position
/// changes (transfer/funding); otherwise the sign decides deposit vs
/// withdrawal, with dust-scale moves on open positions read as funding.
pub fn classify_equity_delta(
    delta: f64,
    positions_changed: bool,
    positions_open: bool,
) -> &'static str {
    if positions_changed {
        return "transfer";
    }
    if positions_open && delta.abs() <= FUNDING_GUESS_MAX_USD {
        return "funding";
    }
    if delta > 0.0 {
        "deposit"
    } else {
        "withdrawal"
    }
}

/// Pure comparison behind the per-account check. `expected` is `None` on
/// first sight (baseline initializes, coverage note, no event). `gap_ms`
/// beyond [`EQUITY_POLL_GAP_MAX_MS`] rebases with a coverage note and no
/// event. Otherwise a nonzero delta classifies and rebases with an event.
pub fn check_equity_delta(
    expected: Option<f64>,
    actual: f64,
    positions_changed: bool,
    positions_open: bool,
    gap_ms: Option<i64>,
) -> EquityCheckOutcome {
    if !actual.is_finite() {
        return EquityCheckOutcome {
            delta: 0.0,
            event_kind: None,
            rebased: false,
            note: Some("equity poll unreadable; baseline untouched".to_string()),
        };
    }
    let Some(exp) = expected.filter(|e| e.is_finite()) else {
        return EquityCheckOutcome {
            delta: 0.0,
            event_kind: None,
            rebased: true,
            note: Some(
                "expected-equity baseline initialized; no prior window to compare".to_string(),
            ),
        };
    };
    if gap_ms.is_some_and(|g| g > EQUITY_POLL_GAP_MAX_MS) {
        return EquityCheckOutcome {
            delta: actual - exp,
            event_kind: None,
            rebased: true,
            note: Some(
                "comparison gap exceeded poll window; rebased without attribution".to_string(),
            ),
        };
    }
    let delta = actual - exp;
    if delta.abs() <= EQUITY_DELTA_EPS_USD {
        // Noise band: no event, but DO rebase to actual. Pinning the
        // baseline to the last noise reading keeps the next comparison
        // honest — otherwise symmetric wobble could random-walk the stored
        // expectation and eventually surface as one phantom large delta.
        return EquityCheckOutcome {
            delta: 0.0,
            event_kind: None,
            rebased: true,
            note: None,
        };
    }
    EquityCheckOutcome {
        delta,
        event_kind: Some(
            classify_equity_delta(delta, positions_changed, positions_open).to_string(),
        ),
        rebased: true,
        note: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_expected_baseline_initializes_without_event() {
        let out = check_equity_delta(None, 100.0, false, false, Some(1000));
        assert_eq!(out.event_kind, None);
        assert!(out.rebased);
        assert!(out.note.is_some());
    }

    #[test]
    fn fills_only_equity_produces_no_event() {
        // Zero delta: no event; the baseline silently re-pins to actual so
        // reporting noise can't random-walk the stored expectation.
        let out = check_equity_delta(Some(100_000.0), 100_000.0, false, false, Some(1000));
        assert_eq!(out.event_kind, None);
        assert!(out.rebased);
        assert_eq!(out.note, None);
    }

    #[test]
    fn manual_deposit_detected_and_rebased() {
        let out = check_equity_delta(Some(100_000.0), 105_000.0, false, false, Some(1000));
        assert_eq!(out.event_kind.as_deref(), Some("deposit"));
        assert!(out.rebased);
        assert!((out.delta - 5000.0).abs() < 1e-9);
    }

    #[test]
    fn withdrawal_detected() {
        let out = check_equity_delta(Some(100_000.0), 97_500.0, false, false, Some(1000));
        assert_eq!(out.event_kind.as_deref(), Some("withdrawal"));
        assert!(out.rebased);
    }

    #[test]
    fn position_change_cross_check_yields_transfer() {
        let out = check_equity_delta(Some(100_000.0), 100_050.0, true, true, Some(1000));
        assert_eq!(out.event_kind.as_deref(), Some("transfer"));
    }

    #[test]
    fn dust_scale_move_with_open_positions_read_as_funding() {
        let out = check_equity_delta(Some(100_000.0), 99_997.5, false, true, Some(1000));
        assert_eq!(out.event_kind.as_deref(), Some("funding"));
    }

    #[test]
    fn missed_poll_gap_produces_no_event_only_note() {
        let out = check_equity_delta(
            Some(100_000.0),
            120_000.0,
            false,
            false,
            Some(EQUITY_POLL_GAP_MAX_MS + 1),
        );
        assert_eq!(out.event_kind, None);
        assert!(out.rebased);
        assert!(out.note.unwrap_or_default().contains("gap"));
    }

    #[test]
    fn unreadable_actual_leaves_baseline_untouched() {
        let out = check_equity_delta(Some(100.0), f64::NAN, false, false, Some(1));
        assert_eq!(out.event_kind, None);
        assert!(!out.rebased);
    }

    #[test]
    fn reporting_noise_produces_no_event_but_rebases() {
        // Venue equity readouts wobble (±$0.02–0.21 observed); a sub-band
        // oscillation must not be written as a deposit/withdrawal, but the
        // baseline silently re-pins to actual so noise can't accumulate.
        for actual in [516.10, 516.14, 516.09, 516.32] {
            let out = check_equity_delta(Some(516.11), actual, false, false, Some(1000));
            assert_eq!(out.event_kind, None, "actual={actual}");
            assert!(out.rebased, "sub-band reads rebase silently");
        }
        // A real move above the band still classifies and rebases.
        let out = check_equity_delta(Some(516.11), 516.40, false, false, Some(1000));
        assert_eq!(out.event_kind.as_deref(), Some("deposit"));
        assert!(out.rebased);
    }
}
