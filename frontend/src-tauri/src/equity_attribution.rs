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

/// Noise deadband, absolute floor: absolute deltas at or below this are
/// reporting noise, not external flows. Venue equity readouts wobble —
/// observed symmetric oscillation of ±$0.02–0.50 across consecutive polls
/// on flat accounts (notably PopDEX and Lighter) — and classifying that as
/// deposit/withdrawal floods `account_equity_events` with phantom flows
/// (thousands observed). Real external flows are dollars-scale, so $0.75
/// of deadband loses no information.
pub const EQUITY_DELTA_EPS_USD: f64 = 0.75;

/// Deadband floor on flat accounts (zero open positions): collateral/mark
/// wobbles up to ~$0.85 have been observed on flat venues like PopDEX
/// (~$2,000 equity). Real external flows are dollars-scale (faucet or
/// deposit minimums are >=$5–10), so a $1.50 floor on flat accounts suppresses
/// phantom deposit/withdrawal churn while preserving real flow detection.
pub const EQUITY_DELTA_FLAT_FLOOR_USD: f64 = 1.50;

/// Noise deadband, relative term: mark/collateral repricing scales with
/// account size, so a flat floor under-filters large accounts — an ~$11M
/// deribit testnet equity readout oscillated ±$90–2450 between ~7s polls
/// (~0.02% — crypto collateral repricing on a FLAT account, not flows) and
/// recorded a phantom "withdrawal" every poll. The effective band is
/// `max(EPS, REL × (expected − REL_FLOOR))`, so accounts under
/// [`EQUITY_REL_NOISE_FLOOR_USD`] keep the $0.25 floor while large-equity
/// reads absorb proportional marks noise. 0.05% of the over-floor amount is
/// ~2× the observed large-account wobble; a flow smaller than that share of
/// a >$1M account is indistinguishable from mark drift anyway.
pub const EQUITY_DELTA_EPS_REL: f64 = 5e-4;

/// Equity below this never widens its noise band — at small balances the
/// observed wobble is sub-dollar, so the $0.25 floor stays authoritative and
/// dust-scale funding/transfer classification is preserved.
pub const EQUITY_REL_NOISE_FLOOR_USD: f64 = 1_000_000.0;

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

/// Fuzzy "did the position set actually change" behind
/// `positions_changed`. Signatures carry `symbol:side:quantity` parts; on
/// venues where quantity is *derived* from a USD-denominated size and a
/// moving mark (Deribit futures report `size` in USD → qty = size/mark),
/// mark drift alone rewrites the qty on every poll. Comparing raw strings
/// flags a position change every read and dumps every mark delta into
/// `transfer`. A real change is: a different symbol set, or a shared
/// symbol's quantity moved more than ~2% (with a dust floor so sub-lot
/// residuals don't flip the bit).
pub fn position_signature_differ(prev: &str, next: &str) -> bool {
    use std::collections::HashMap;
    if prev == next {
        return false;
    }
    let parse = |s: &str| -> Option<HashMap<(String, String), f64>> {
        if s.is_empty() {
            return Some(HashMap::new());
        }
        let mut m = HashMap::new();
        for part in s.split('|') {
            // `symbol:side:qty` — symbols contain '-' but never ':'.
            let mut it = part.rsplitn(3, ':');
            let qty = it.next()?.parse::<f64>().ok()?;
            let side = it.next()?.to_string();
            let sym = it.next()?.to_string();
            m.insert((sym, side), qty);
        }
        Some(m)
    };
    match (parse(prev), parse(next)) {
        (Some(a), Some(b)) => {
            if a.len() != b.len() {
                return true;
            }
            a.iter().any(|(k, qa)| match b.get(k) {
                None => true,
                Some(qb) => {
                    let tol = (qa.abs().max(qb.abs()) * 0.02).max(1e-4);
                    (qa - qb).abs() > tol
                }
            })
        }
        _ => true,
    }
}

/// Classify a nonzero delta: a collateral-FX estimate that explains the
/// move (`repricing_usd` covers ≥75% within the noise floor) reads as
/// `mark_repricing` — a coin-margined account's index move is not an
/// external flow. Otherwise a changed position signature implies a
/// transfer/settlement, funding-scale moves on open positions read as
/// funding, and the sign decides deposit vs withdrawal on flat accounts.
pub fn classify_equity_delta(
    delta: f64,
    positions_changed: bool,
    positions_open: bool,
    repricing_usd: Option<f64>,
) -> &'static str {
    if let Some(rep) = repricing_usd.filter(|r| r.is_finite() && *r != 0.0) {
        let residual = delta - rep;
        if residual.abs() <= (delta.abs() * 0.25).max(EQUITY_DELTA_EPS_USD) {
            return "mark_repricing";
        }
    }
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
    repricing_usd: Option<f64>,
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
    let floor = if !positions_open {
        EQUITY_DELTA_FLAT_FLOOR_USD
    } else {
        EQUITY_DELTA_EPS_USD
    };
    let band = floor.max((exp.abs() - EQUITY_REL_NOISE_FLOOR_USD).max(0.0) * EQUITY_DELTA_EPS_REL);
    if delta.abs() <= band {
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
            classify_equity_delta(delta, positions_changed, positions_open, repricing_usd)
                .to_string(),
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
        let out = check_equity_delta(None, 100.0, false, false, Some(1000), None);
        assert_eq!(out.event_kind, None);
        assert!(out.rebased);
        assert!(out.note.is_some());
    }

    #[test]
    fn fills_only_equity_produces_no_event() {
        // Zero delta: no event; the baseline silently re-pins to actual so
        // reporting noise can't random-walk the stored expectation.
        let out = check_equity_delta(Some(100_000.0), 100_000.0, false, false, Some(1000), None);
        assert_eq!(out.event_kind, None);
        assert!(out.rebased);
        assert_eq!(out.note, None);
    }

    #[test]
    fn manual_deposit_detected_and_rebased() {
        let out = check_equity_delta(Some(100_000.0), 105_000.0, false, false, Some(1000), None);
        assert_eq!(out.event_kind.as_deref(), Some("deposit"));
        assert!(out.rebased);
        assert!((out.delta - 5000.0).abs() < 1e-9);
    }

    #[test]
    fn withdrawal_detected() {
        let out = check_equity_delta(Some(100_000.0), 97_500.0, false, false, Some(1000), None);
        assert_eq!(out.event_kind.as_deref(), Some("withdrawal"));
        assert!(out.rebased);
    }

    #[test]
    fn large_account_mark_drift_is_noise_not_flow() {
        // $11.1M account drifting ~0.001% between polls — index repricing,
        // not a withdrawal. The flat $0.25 band alone would emit a phantom
        // withdrawal every poll.
        let out = check_equity_delta(
            Some(11_133_435.1),
            11_133_322.6,
            false,
            true,
            Some(7000),
            None,
        );
        assert_eq!(out.event_kind, None);
        assert!(out.rebased, "noise still re-pins the baseline");
    }

    #[test]
    fn large_account_real_flow_still_detected() {
        // The band widens to ~$5.1k at $11.1M; a deliberate transfer is far
        // above that and must still surface.
        let out = check_equity_delta(
            Some(11_133_435.1),
            11_083_435.1,
            false,
            true,
            Some(7000),
            None,
        );
        assert_eq!(out.event_kind.as_deref(), Some("withdrawal"));
    }

    #[test]
    fn position_change_cross_check_yields_transfer() {
        let out = check_equity_delta(Some(100_000.0), 100_050.0, true, true, Some(1000), None);
        assert_eq!(out.event_kind.as_deref(), Some("transfer"));
    }

    #[test]
    fn dust_scale_move_with_open_positions_read_as_funding() {
        let out = check_equity_delta(Some(100_000.0), 99_997.5, false, true, Some(1000), None);
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
            None,
        );
        assert_eq!(out.event_kind, None);
        assert!(out.rebased);
        assert!(out.note.unwrap_or_default().contains("gap"));
    }

    #[test]
    fn unreadable_actual_leaves_baseline_untouched() {
        let out = check_equity_delta(Some(100.0), f64::NAN, false, false, Some(1), None);
        assert_eq!(out.event_kind, None);
        assert!(!out.rebased);
    }

    #[test]
    fn reporting_noise_produces_no_event_but_rebases() {
        // Venue equity readouts wobble (±$0.02–0.21 observed); a sub-band
        // oscillation must not be written as a deposit/withdrawal, but the
        // baseline silently re-pins to actual so noise can't accumulate.
        for actual in [516.10, 516.14, 516.09, 516.32, 516.60, 517.20] {
            let out = check_equity_delta(Some(516.11), actual, false, false, Some(1000), None);
            assert_eq!(out.event_kind, None, "actual={actual}");
            assert!(out.rebased, "sub-band reads rebase silently");
        }
        // A real move above the band still classifies and rebases.
        let out = check_equity_delta(Some(516.11), 518.50, false, false, Some(1000), None);
        assert_eq!(out.event_kind.as_deref(), Some("deposit"));
        assert!(out.rebased);
    }

    #[test]
    fn collateral_repricing_explains_delta_as_mark_not_transfer() {
        // Deribit-style book: 130 BTC collateral, positions open, BTC
        // index fell — the balance×Δfx term explains the equity move.
        // Without the repricing estimate this classifies `transfer` on
        // every poll (derived qty drifts → signature churn).
        let out = check_equity_delta(
            Some(11_020_000.0),
            11_013_000.0,
            false,
            true,
            Some(60_000),
            Some(-6_900.0), // 130 BTC × −$53 index move
        );
        assert_eq!(out.event_kind.as_deref(), Some("mark_repricing"));
        assert!(out.rebased);
        // Unexplained residual beyond 25% of the delta does NOT classify
        // as repricing — a real flow riding under collateral noise still
        // surfaces via the raw ladder.
        let out = check_equity_delta(
            Some(11_020_000.0),
            11_013_000.0,
            false,
            true,
            Some(60_000),
            Some(-3_000.0), // explains only ~43%
        );
        assert_eq!(out.event_kind.as_deref(), Some("withdrawal"));
        // Opposite-sign estimates never classify.
        let out = check_equity_delta(
            Some(11_020_000.0),
            11_013_000.0,
            false,
            true,
            Some(60_000),
            Some(6_900.0),
        );
        assert_eq!(out.event_kind.as_deref(), Some("withdrawal"));
    }

    #[test]
    fn position_signature_differ_ignores_mark_derived_qty_drift() {
        // USD-size ÷ mark derived qty drifts ~0.5% — not a position change.
        assert!(!position_signature_differ(
            "BTC-PERPETUAL:Buy:0.1007|BTC-30OCT26-88000-P:Buy:0.1",
            "BTC-PERPETUAL:Buy:0.1002|BTC-30OCT26-88000-P:Buy:0.1",
        ));
        // Same drift on a second row — still quiet.
        assert!(!position_signature_differ("", ""));
        // A real fill (+0.1 on the perp) flips the bit.
        assert!(position_signature_differ(
            "BTC-PERPETUAL:Buy:0.1007",
            "BTC-PERPETUAL:Buy:0.2007",
        ));
        // A new instrument row flips the bit.
        assert!(position_signature_differ(
            "BTC-PERPETUAL:Buy:0.1",
            "BTC-PERPETUAL:Buy:0.1|ETH-PERPETUAL:Buy:1.0",
        ));
        // Dust-scale residual drift (the observed 0.00018 BTC-PERP wedge)
        // stays under the floor.
        assert!(!position_signature_differ(
            "BTC-PERPETUAL:Buy:0.10068",
            "BTC-PERPETUAL:Buy:0.10050",
        ));
        // Unparseable signatures fall back to strict difference.
        assert!(position_signature_differ("garbage", "other"));
        assert!(!position_signature_differ("garbage", "garbage"));
    }
}
