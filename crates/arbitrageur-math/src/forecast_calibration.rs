//! Forecast calibration: scores predicted probabilities against realized
//! binary outcomes. A probability that is never scored is a guess with
//! extra steps — every downstream gate and sizing rule silently assumes
//! calibration, so the system measures it explicitly.
//!
//! Scored pairs are `(predicted_p, outcome)` where `outcome` is the
//! realized event the prediction described (e.g. net bps > 0 for a
//! `net_win` prediction). Predictions whose event semantics do not match
//! the realized quantity must be filtered out by the caller before they
//! reach this module — mixing event definitions corrupts every statistic
//! here.
//!
//! Distinct from `calibration.rs`, which holds baseline-estimator / WFO
//! parameter calibration; this module scores probability forecasts.

use serde::Serialize;

/// One reliability-diagram bucket: predictions whose probability fell in
/// the bin's range, their mean prediction, and the observed frequency.
#[derive(Debug, Clone, Serialize)]
pub struct CalibrationBin {
    /// Inclusive lower edge of the bin's probability range.
    pub lo: f64,
    /// Exclusive upper edge (inclusive on the final bin).
    pub hi: f64,
    /// Number of scored predictions in the bin.
    pub n: usize,
    /// Mean predicted probability inside the bin.
    pub mean_pred: f64,
    /// Observed outcome frequency inside the bin.
    pub observed: f64,
}

/// Aggregate calibration statistics for one stream of scored forecasts.
#[derive(Debug, Clone, Serialize)]
pub struct CalibrationReport {
    /// Predictions paired with a realized outcome.
    pub scored: usize,
    /// Brier score: mean (p - y)^2 over scored pairs. Lower is better;
    /// 0.25 is the always-0.5 baseline, well below that is required for a
    /// useful forecast.
    pub brier: Option<f64>,
    /// Linear-probability-model calibration: OLS fit of outcome on the
    /// prediction. Perfect calibration is intercept 0 / slope 1;
    /// intercept > 0 with slope < 1 is the classic overconfidence
    /// signature (predictions too extreme). `None` under 3 scored pairs
    /// or zero prediction variance.
    pub intercept: Option<f64>,
    pub slope: Option<f64>,
    /// Reliability diagram, one bucket per decile of predicted p.
    pub bins: Vec<CalibrationBin>,
}

/// Score a stream of `(predicted_p, outcome)` pairs. Invalid predictions
/// (non-finite or outside [0, 1]) are skipped rather than silently
/// poisoning the aggregates.
pub fn calibration_report(predictions: &[(f64, bool)]) -> CalibrationReport {
    let scored: Vec<(f64, f64)> = predictions
        .iter()
        .filter(|(p, _)| p.is_finite() && (0.0..=1.0).contains(p))
        .map(|(p, y)| (*p, if *y { 1.0 } else { 0.0 }))
        .collect();
    let n = scored.len();
    if n == 0 {
        return CalibrationReport {
            scored: 0,
            brier: None,
            intercept: None,
            slope: None,
            bins: Vec::new(),
        };
    }

    let brier = scored.iter().map(|(p, y)| (p - y).powi(2)).sum::<f64>() / n as f64;

    // OLS of outcome on prediction (linear probability model): a cheap,
    // interpretable calibration diagnostic. slope 1 / intercept 0 is
    // ideal; slope < 1 with intercept > 0 flags overconfident tails.
    let mean_p = scored.iter().map(|(p, _)| p).sum::<f64>() / n as f64;
    let mean_y = scored.iter().map(|(_, y)| y).sum::<f64>() / n as f64;
    let (mut s_pp, mut s_py) = (0.0, 0.0);
    for (p, y) in &scored {
        s_pp += (p - mean_p).powi(2);
        s_py += (p - mean_p) * (y - mean_y);
    }
    let (slope, intercept) = if n >= 3 && s_pp > 1e-12 {
        let s = s_py / s_pp;
        (Some(s), Some(mean_y - s * mean_p))
    } else {
        (None, None)
    };

    const BINS: usize = 10;
    let mut acc = vec![(0usize, 0.0f64, 0.0f64); BINS];
    for (p, y) in &scored {
        let idx = ((p * BINS as f64) as usize).min(BINS - 1);
        acc[idx].0 += 1;
        acc[idx].1 += p;
        acc[idx].2 += y;
    }
    let bins = acc
        .iter()
        .enumerate()
        .filter(|(_, (n, _, _))| *n > 0)
        .map(|(i, (n, sp, sy))| CalibrationBin {
            lo: i as f64 / BINS as f64,
            hi: (i + 1) as f64 / BINS as f64,
            n: *n,
            mean_pred: sp / *n as f64,
            observed: sy / *n as f64,
        })
        .collect();

    CalibrationReport {
        scored: n,
        brier: Some(brier),
        intercept,
        slope,
        bins,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_report_is_honest() {
        let r = calibration_report(&[]);
        assert_eq!(r.scored, 0);
        assert!(r.brier.is_none());
        assert!(r.bins.is_empty());
    }

    #[test]
    fn perfect_forecast_scores_zero() {
        let r = calibration_report(&[(0.9, true), (0.1, false), (0.95, true)]);
        assert_eq!(r.scored, 3);
        assert!(r.brier.unwrap() < 0.02);
    }

    #[test]
    fn always_half_brier_is_quarter() {
        let r = calibration_report(&[(0.5, true), (0.5, false), (0.5, true), (0.5, false)]);
        assert!((r.brier.unwrap() - 0.25).abs() < 1e-12);
    }

    #[test]
    fn overconfident_forecast_shows_flat_slope() {
        // Predictions spread across the top half but outcomes are a coin
        // flip regardless: the fit slope is far below the calibrated 1.
        let pairs: Vec<(f64, bool)> = (0..40)
            .map(|i| (0.6 + (i % 8) as f64 * 0.05, i % 2 == 0))
            .collect();
        let r = calibration_report(&pairs);
        let slope = r.slope.unwrap();
        assert!(slope < 0.6, "slope {slope}");
        // The top bin sits at ~0.9+ predicted with observed at/below 0.5 —
        // the anticorrelated construction keeps the highest forecasts poor.
        let top = r.bins.last().unwrap();
        assert!(top.mean_pred > 0.9 && top.observed <= 0.5);
    }

    #[test]
    fn calibrated_forecast_has_unit_slope() {
        // Construct a perfectly calibrated stream: p bucket k wins k/10 of the time.
        let mut pairs = Vec::new();
        for k in 1..10 {
            let p = k as f64 / 10.0;
            for i in 0..10 {
                pairs.push((p, i < k));
            }
        }
        let r = calibration_report(&pairs);
        let slope = r.slope.unwrap();
        assert!((slope - 1.0).abs() < 0.05, "slope {slope}");
        assert!(
            r.intercept.unwrap().abs() < 0.05,
            "intercept {:?}",
            r.intercept
        );
    }

    #[test]
    fn invalid_predictions_are_skipped() {
        let r = calibration_report(&[(f64::NAN, true), (1.5, true), (0.8, true)]);
        assert_eq!(r.scored, 1);
    }
}
