/// Position-aware exit policy for Single-Spread ("futures") pair positions.
///
/// Pure decision function over an entry baseline and a live edge quote: no
/// I/O, no engine state, deterministic in its arguments. The backend stores
/// the entry baseline at open, re-prices the live edge from validated books,
/// and unwinds through the existing symbol-scoped flatten path on any exit
/// verdict — freeing the pair slot for re-entry.
///
/// Ordering mirrors `convergence_auto::evaluate_exit`: convergence wins ties
/// (a vanished edge is never held for a stop), then stop, then timeout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpatialExitConfig {
    /// Exit as converged when the live edge falls to this level (bps).
    pub converge_exit_bps: f64,
    /// Exit as stopped when the live edge drops this far below entry (bps).
    pub stop_drop_bps: f64,
    /// Exit as timed out this long after entry (seconds).
    pub max_hold_secs: i64,
}

impl Default for SpatialExitConfig {
    fn default() -> Self {
        Self {
            converge_exit_bps: 0.0,
            stop_drop_bps: 5.0,
            max_hold_secs: 86_400,
        }
    }
}

impl SpatialExitConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !self.converge_exit_bps.is_finite() || self.converge_exit_bps < 0.0 {
            return Err(format!(
                "converge_exit_bps {} must be finite and nonnegative",
                self.converge_exit_bps
            ));
        }
        if !self.stop_drop_bps.is_finite() || self.stop_drop_bps <= 0.0 {
            return Err(format!(
                "stop_drop_bps {} must be finite and positive",
                self.stop_drop_bps
            ));
        }
        if self.max_hold_secs < 60 {
            return Err(format!(
                "max_hold_secs {} must be at least 60",
                self.max_hold_secs
            ));
        }
        Ok(())
    }
}

/// Exit verdict for one open pair position on one check.
#[derive(Debug, Clone, PartialEq)]
pub enum SpatialExitVerdict {
    Hold,
    /// No baseline or no live quote: never auto-exit blind. The operator
    /// path (manual flatten) stays available; the slot stays occupied.
    Unwatched {
        reason: String,
    },
    Exit {
        trigger: SpatialExitTrigger,
        reason: String,
    },
}

/// Why a pair position is being unwound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpatialExitTrigger {
    Converged,
    Stopped,
    TimedOut,
}

impl SpatialExitTrigger {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Converged => "converged",
            Self::Stopped => "stopped",
            Self::TimedOut => "timed_out",
        }
    }
}

pub fn evaluate_spatial_exit(
    entry_net_bps: Option<f64>,
    live_net_bps: Option<f64>,
    entry_ts_ms: i64,
    now_ms: i64,
    cfg: &SpatialExitConfig,
) -> SpatialExitVerdict {
    let Some(entry) = entry_net_bps.filter(|v| v.is_finite()) else {
        return SpatialExitVerdict::Unwatched {
            reason: "no entry baseline recorded".to_string(),
        };
    };
    let Some(live) = live_net_bps.filter(|v| v.is_finite()) else {
        return SpatialExitVerdict::Unwatched {
            reason: "no live edge quote".to_string(),
        };
    };
    if live <= cfg.converge_exit_bps {
        return SpatialExitVerdict::Exit {
            trigger: SpatialExitTrigger::Converged,
            reason: format!(
                "edge decayed to {live:.1} bps (exit at <= {:.1})",
                cfg.converge_exit_bps
            ),
        };
    }
    if live <= entry - cfg.stop_drop_bps {
        return SpatialExitVerdict::Exit {
            trigger: SpatialExitTrigger::Stopped,
            reason: format!(
                "edge fell to {live:.1} bps, {d:.1} below entry {entry:.1}",
                d = entry - live
            ),
        };
    }
    if now_ms.saturating_sub(entry_ts_ms) >= cfg.max_hold_secs.saturating_mul(1000) {
        return SpatialExitVerdict::Exit {
            trigger: SpatialExitTrigger::TimedOut,
            reason: "max-hold deadline elapsed".to_string(),
        };
    }
    SpatialExitVerdict::Hold
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SpatialExitConfig {
        SpatialExitConfig::default()
    }

    #[test]
    fn hold_inside_band_before_deadline() {
        assert_eq!(
            evaluate_spatial_exit(Some(8.0), Some(5.0), 0, 1000, &cfg()),
            SpatialExitVerdict::Hold
        );
    }

    #[test]
    fn converged_when_edge_gone() {
        for live in [0.0, -2.5] {
            assert!(matches!(
                evaluate_spatial_exit(Some(8.0), Some(live), 0, 1000, &cfg()),
                SpatialExitVerdict::Exit {
                    trigger: SpatialExitTrigger::Converged,
                    ..
                }
            ));
        }
    }

    #[test]
    fn convergence_wins_over_stop_on_ties() {
        // Entry 2, stop band at -3: live -4 is both converged (<=0) and
        // past the stop; convergence must win.
        assert!(matches!(
            evaluate_spatial_exit(Some(2.0), Some(-4.0), 0, 1000, &cfg()),
            SpatialExitVerdict::Exit {
                trigger: SpatialExitTrigger::Converged,
                ..
            }
        ));
    }

    #[test]
    fn stopped_on_adverse_excursion() {
        assert!(matches!(
            evaluate_spatial_exit(Some(8.0), Some(2.0), 0, 1000, &cfg()),
            SpatialExitVerdict::Exit {
                trigger: SpatialExitTrigger::Stopped,
                ..
            }
        ));
        // The stop band is inclusive, matching convergence_auto semantics.
        assert!(matches!(
            evaluate_spatial_exit(Some(8.0), Some(3.0), 0, 1000, &cfg()),
            SpatialExitVerdict::Exit {
                trigger: SpatialExitTrigger::Stopped,
                ..
            }
        ));
        assert_eq!(
            evaluate_spatial_exit(Some(8.0), Some(3.1), 0, 1000, &cfg()),
            SpatialExitVerdict::Hold
        );
    }

    #[test]
    fn timed_out_past_deadline() {
        let c = cfg();
        let at = c.max_hold_secs * 1000;
        assert!(matches!(
            evaluate_spatial_exit(Some(8.0), Some(6.0), 0, at, &c),
            SpatialExitVerdict::Exit {
                trigger: SpatialExitTrigger::TimedOut,
                ..
            }
        ));
        assert_eq!(
            evaluate_spatial_exit(Some(8.0), Some(6.0), 0, at - 1, &c),
            SpatialExitVerdict::Hold
        );
    }

    #[test]
    fn unwatched_without_baseline_or_quote() {
        assert!(matches!(
            evaluate_spatial_exit(None, Some(5.0), 0, 0, &cfg()),
            SpatialExitVerdict::Unwatched { .. }
        ));
        assert!(matches!(
            evaluate_spatial_exit(Some(f64::NAN), Some(5.0), 0, 0, &cfg()),
            SpatialExitVerdict::Unwatched { .. }
        ));
        assert!(matches!(
            evaluate_spatial_exit(Some(8.0), None, 0, 0, &cfg()),
            SpatialExitVerdict::Unwatched { .. }
        ));
        assert!(matches!(
            evaluate_spatial_exit(Some(8.0), Some(f64::INFINITY), 0, 0, &cfg()),
            SpatialExitVerdict::Unwatched { .. }
        ));
    }

    #[test]
    fn config_validation_rejects_unsafe_ranges() {
        assert!(cfg().validate().is_ok());
        let mut bad = cfg();
        bad.stop_drop_bps = 0.0;
        assert!(bad.validate().is_err());
        let mut bad = cfg();
        bad.converge_exit_bps = f64::NAN;
        assert!(bad.validate().is_err());
        let mut bad = cfg();
        bad.max_hold_secs = 10;
        assert!(bad.validate().is_err());
    }

    #[test]
    fn verdicts_are_deterministic() {
        let a = evaluate_spatial_exit(Some(8.0), Some(5.0), 0, 1000, &cfg());
        let b = evaluate_spatial_exit(Some(8.0), Some(5.0), 0, 1000, &cfg());
        assert_eq!(a, b);
    }
}
