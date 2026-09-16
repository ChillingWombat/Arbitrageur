//! Seasonality Phase 2 — pure estimators with no I/O.
//!
//! Three pieces share this module:
//!
//! 1. **Log-HAR anchor**: an OLS fit of next-day log realized volatility on
//!    three regressors (daily / trailing-5d mean / trailing-22d mean, in log
//!    space). The nightly job fits it over daily RV and rescales the durable
//!    session `rv_bps` norms toward the one-day-ahead forecast. No GARCH, no
//!    ML — plain 4-parameter OLS.
//! 2. **Autotune coupling**: session τ/σ regime scales that feed the
//!    walk-forward recommender inputs (`dt` and friction). Missing or
//!    provisional profiles degrade to exactly 1.0.
//! 3. **Walk-forward promotion**: fit-trailing / validate-forward windows
//!    plus a sign-agreement gate — hurdle/size tables promote only when the
//!    fit delta and the forward-validation delta agree in sign, and stress
//!    weeks are excluded from calibration.
//!
//! Fail-closed throughout: every fallible estimator returns `Option` and the
//! only failure mode is "no anchor / no promotion / neutral coupling".

use serde::{Deserialize, Serialize};

/* ------------------------------------------------------------------------ */
/* Log-HAR anchor                                                            */
/* ------------------------------------------------------------------------ */

/// Minimum daily-RV points before a HAR fit is attempted.
pub const HAR_MIN_DAYS: usize = 30;
/// Trailing means backing the weekly / monthly regressors.
pub const HAR_WEEKLY_LOOKBACK: usize = 5;
pub const HAR_MONTHLY_LOOKBACK: usize = 22;
/// Profile rescale clamp: the HAR forecast may move session RV norms by at
/// most ±50%, never further.
pub const HAR_RESCALE_LO: f64 = 0.5;
pub const HAR_RESCALE_HI: f64 = 1.5;

/// Fitted log-HAR(1,5,22) anchor:
/// `log RV[t+1] = c + b_d·logRV[t] + b_w·mean(logRV[t-4..t]) + b_m·mean(logRV[t-21..t])`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HarAnchor {
    pub intercept: f64,
    pub b_daily: f64,
    pub b_weekly: f64,
    pub b_monthly: f64,
    /// In-sample R² of the log regression (`0..=1`).
    pub r2: f64,
    /// Daily points consumed by the fit.
    pub n_days: usize,
}

fn trailing_mean(log_rv: &[f64], end: usize, lookback: usize) -> Option<f64> {
    if lookback == 0 || end >= log_rv.len() || end + 1 < lookback {
        return None;
    }
    let start = end + 1 - lookback;
    let mut sum = 0.0;
    for v in log_rv.iter().take(end + 1).skip(start) {
        if !v.is_finite() {
            return None;
        }
        sum += *v;
    }
    let m = sum / lookback as f64;
    if m.is_finite() {
        Some(m)
    } else {
        None
    }
}

/// Solve a 4×4 symmetric-positive-definite-ish system `A·x = b` by Gaussian
/// elimination with partial pivoting. `None` on singularity — the caller
/// treats it as "no anchor", never as zeros.
fn solve_4x4(a: [[f64; 4]; 4], b: [f64; 4]) -> Option<[f64; 4]> {
    let mut m = [[0.0_f64; 5]; 4];
    for (i, row) in m.iter_mut().enumerate() {
        for j in 0..4 {
            let v = a[i][j];
            if !v.is_finite() {
                return None;
            }
            row[j] = v;
        }
        if !b[i].is_finite() {
            return None;
        }
        row[4] = b[i];
    }
    for col in 0..4 {
        let mut pivot = col;
        for r in (col + 1)..4 {
            if m[r][col].abs() > m[pivot][col].abs() {
                pivot = r;
            }
        }
        if !m[pivot][col].is_finite() || m[pivot][col].abs() < 1e-12 {
            return None;
        }
        m.swap(col, pivot);
        let div = m[col][col];
        for v in m[col].iter_mut().skip(col) {
            *v /= div;
        }
        let pivot_row = m[col];
        for (r, row) in m.iter_mut().enumerate() {
            if r == col {
                continue;
            }
            let f = row[col];
            if !f.is_finite() {
                return None;
            }
            for (d, s) in row.iter_mut().skip(col).zip(pivot_row.iter().skip(col)) {
                *d -= f * s;
            }
        }
    }
    let mut out = [0.0_f64; 4];
    for (i, v) in out.iter_mut().enumerate() {
        if !m[i][4].is_finite() {
            return None;
        }
        *v = m[i][4];
    }
    Some(out)
}

/// Fit the log-HAR anchor over daily realized-volatility points (bps,
/// chronological, oldest first). `None` on empty/short/degenerate input —
/// gaps are rejected, never interpolated.
pub fn fit_log_har(daily_rv_bps: &[f64]) -> Option<HarAnchor> {
    if daily_rv_bps.len() < HAR_MIN_DAYS {
        return None;
    }
    let log_rv: Vec<f64> = daily_rv_bps
        .iter()
        .map(|v| {
            if v.is_finite() && *v > 0.0 {
                v.ln()
            } else {
                f64::NAN
            }
        })
        .collect();
    if log_rv.iter().any(|v| !v.is_finite()) {
        return None;
    }
    // Design rows start once the 22d trailing mean is defined.
    let first = HAR_MONTHLY_LOOKBACK - 1;
    if daily_rv_bps.len() <= first + 1 {
        return None;
    }
    let mut xtx = [[0.0_f64; 4]; 4];
    let mut xty = [0.0_f64; 4];
    let mut ys: Vec<f64> = Vec::new();
    let mut n = 0usize;
    for t in first..(log_rv.len() - 1) {
        let (Some(w), Some(m)) = (
            trailing_mean(&log_rv, t, HAR_WEEKLY_LOOKBACK),
            trailing_mean(&log_rv, t, HAR_MONTHLY_LOOKBACK),
        ) else {
            return None;
        };
        let x = [1.0, log_rv[t], w, m];
        let y = log_rv[t + 1];
        for (i, xi) in x.iter().enumerate() {
            for (j, xj) in x.iter().enumerate() {
                xtx[i][j] += xi * xj;
            }
            xty[i] += xi * y;
        }
        ys.push(y);
        n += 1;
    }
    if n < 5 {
        return None;
    }
    let beta = solve_4x4(xtx, xty)?;
    let mean_y = ys.iter().sum::<f64>() / ys.len() as f64;
    if !mean_y.is_finite() {
        return None;
    }
    let mut ss_tot = 0.0;
    let mut ss_res = 0.0;
    for t in first..(log_rv.len() - 1) {
        let (Some(w), Some(m)) = (
            trailing_mean(&log_rv, t, HAR_WEEKLY_LOOKBACK),
            trailing_mean(&log_rv, t, HAR_MONTHLY_LOOKBACK),
        ) else {
            return None;
        };
        let y = log_rv[t + 1];
        let pred = beta[0] + beta[1] * log_rv[t] + beta[2] * w + beta[3] * m;
        if !(pred.is_finite() && y.is_finite()) {
            return None;
        }
        ss_tot += (y - mean_y).powi(2);
        ss_res += (y - pred).powi(2);
    }
    if !(ss_tot.is_finite() && ss_res.is_finite()) || ss_tot <= 0.0 {
        return None;
    }
    let r2 = (1.0 - ss_res / ss_tot).clamp(0.0, 1.0);
    if !(beta.iter().all(|b| b.is_finite()) && r2.is_finite()) {
        return None;
    }
    Some(HarAnchor {
        intercept: beta[0],
        b_daily: beta[1],
        b_weekly: beta[2],
        b_monthly: beta[3],
        r2,
        n_days: daily_rv_bps.len(),
    })
}

/// One-day-ahead RV forecast (bps) from an anchor and the trailing daily
/// series. `None` when the tail is too short or unusable.
pub fn har_forecast_next(anchor: &HarAnchor, daily_rv_bps: &[f64]) -> Option<f64> {
    let n = daily_rv_bps.len();
    if n < HAR_MONTHLY_LOOKBACK {
        return None;
    }
    let tail: Vec<f64> = daily_rv_bps
        .iter()
        .skip(n - HAR_MONTHLY_LOOKBACK)
        .map(|v| {
            if v.is_finite() && *v > 0.0 {
                v.ln()
            } else {
                f64::NAN
            }
        })
        .collect();
    if tail.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let last = tail[tail.len() - 1];
    let weekly = tail.iter().skip(tail.len() - HAR_WEEKLY_LOOKBACK).sum::<f64>()
        / HAR_WEEKLY_LOOKBACK as f64;
    let monthly = tail.iter().sum::<f64>() / HAR_MONTHLY_LOOKBACK as f64;
    let pred = anchor.intercept + anchor.b_daily * last + anchor.b_weekly * weekly
        + anchor.b_monthly * monthly;
    if !pred.is_finite() {
        return None;
    }
    let f = pred.exp();
    if f.is_finite() && f > 0.0 {
        Some(f)
    } else {
        None
    }
}

/// Rescale factor `forecast / trailing_mean`, clamped to ±50%. Any unusable
/// input degrades to exactly 1.0 (profiles untouched).
pub fn rescale_factor(forecast_rv_bps: f64, trailing_mean_rv_bps: f64) -> f64 {
    if !forecast_rv_bps.is_finite()
        || !trailing_mean_rv_bps.is_finite()
        || forecast_rv_bps <= 0.0
        || trailing_mean_rv_bps <= 0.0
    {
        return 1.0;
    }
    (forecast_rv_bps / trailing_mean_rv_bps).clamp(HAR_RESCALE_LO, HAR_RESCALE_HI)
}

/// Rescaled session RV norm. Unusable base degrades to `None` (row skipped,
/// never fabricated); unusable anchor inputs leave the base untouched.
pub fn rescale_session_rv(
    base_rv_bps: Option<f64>,
    forecast_rv_bps: f64,
    trailing_mean_rv_bps: f64,
) -> Option<f64> {
    match base_rv_bps {
        Some(b) if b.is_finite() && b > 0.0 => {
            let r = b * rescale_factor(forecast_rv_bps, trailing_mean_rv_bps);
            if r.is_finite() && r > 0.0 {
                Some(r)
            } else {
                Some(b)
            }
        }
        _ => None,
    }
}

/* ------------------------------------------------------------------------ */
/* Autotune coupling (session τ/σ → recommender inputs)                      */
/* ------------------------------------------------------------------------ */

/// Regime scales feeding the autotune recommender: `tau_scale` multiplies
/// the OU `dt` inputs (session persistence vs global), `sigma_scale`
/// multiplies the friction inputs (session vol vs global). Both are exactly
/// 1.0 when the profile side is missing or provisional.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionCoupling {
    pub tau_scale: f64,
    pub sigma_scale: f64,
}

/// Derive coupling from shrunk session medians / RV norms against their
/// global counterparts. `None` on either side (or non-positive globals)
/// degrades that scale to 1.0.
pub fn coupling_inputs(
    session_med_bps: Option<f64>,
    global_med_bps: Option<f64>,
    session_rv_bps: Option<f64>,
    global_rv_bps: Option<f64>,
) -> SessionCoupling {
    let tau_scale = match (session_med_bps, global_med_bps) {
        (Some(s), Some(g))
            if s.is_finite() && g.is_finite() && s > 0.0 && g > 0.0 =>
        {
            (s / g).clamp(0.5, 2.0)
        }
        _ => 1.0,
    };
    let sigma_scale = match (session_rv_bps, global_rv_bps) {
        (Some(s), Some(g))
            if s.is_finite() && g.is_finite() && s > 0.0 && g > 0.0 =>
        {
            (s / g).clamp(0.5, 1.5)
        }
        _ => 1.0,
    };
    SessionCoupling {
        tau_scale,
        sigma_scale,
    }
}

/* ------------------------------------------------------------------------ */
/* Walk-forward promotion scheduler                                          */
/* ------------------------------------------------------------------------ */

/// Minimum |delta| (bps) for a promotion candidate to be meaningful.
pub const PROMOTION_MIN_ABS_DELTA_BPS: f64 = 0.5;

/// Contiguous fit-trailing / validate-forward windows ending at `now_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalkForwardWindow {
    pub fit_start_ms: i64,
    pub fit_end_ms: i64,
    pub val_start_ms: i64,
    pub val_end_ms: i64,
}

/// Split `fit_days` trailing + `val_days` forward windows ending at `now_ms`.
/// `None` on non-positive clocks, non-positive spans, or arithmetic
/// overflow — the scheduler skips the run (clock-skew fail-closed), never
/// promotes on a half window.
pub fn walk_forward_windows(
    now_ms: i64,
    fit_days: i64,
    val_days: i64,
) -> Option<WalkForwardWindow> {
    const DAY_MS: i64 = 86_400_000;
    if now_ms <= 0 || fit_days <= 0 || val_days <= 0 {
        return None;
    }
    let fit_span = fit_days.checked_mul(DAY_MS)?;
    let val_span = val_days.checked_mul(DAY_MS)?;
    let total = fit_span.checked_add(val_span)?;
    let fit_start = now_ms.checked_sub(total)?;
    if fit_start < 0 {
        return None;
    }
    let fit_end = fit_start.checked_add(fit_span)?;
    let val_end = now_ms;
    if !(fit_start < fit_end && fit_end <= val_end) {
        return None;
    }
    Some(WalkForwardWindow {
        fit_start_ms: fit_start,
        fit_end_ms: fit_end,
        val_start_ms: fit_end,
        val_end_ms: val_end,
    })
}

/// Promotion outcome for one symbol's hurdle/size tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromotionStatus {
    Promoted,
    Held,
    Demoted,
}

impl PromotionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PromotionStatus::Promoted => "promoted",
            PromotionStatus::Held => "held",
            PromotionStatus::Demoted => "demoted",
        }
    }

    pub fn parse(s: &str) -> Option<PromotionStatus> {
        match s.trim().to_lowercase().as_str() {
            "promoted" => Some(PromotionStatus::Promoted),
            "held" => Some(PromotionStatus::Held),
            "demoted" => Some(PromotionStatus::Demoted),
            _ => None,
        }
    }
}

/// One promotion verdict: deltas are fit-trailing and validate-forward
/// median-spread shifts (bps, signed) vs the current table baseline.
#[derive(Debug, Clone, PartialEq)]
pub struct PromotionDecision {
    pub status: PromotionStatus,
    pub fit_delta_bps: f64,
    pub val_delta_bps: f64,
    pub reason: String,
}

/// Sign-agreement gate: promote only when both deltas clear the minimum
/// magnitude **and** agree in sign. Disagreement demotes (the table is
/// stale); anything unmeasurable or below-magnitude holds.
pub fn decide_promotion(fit_delta_bps: f64, val_delta_bps: f64) -> PromotionDecision {
    if !fit_delta_bps.is_finite() || !val_delta_bps.is_finite() {
        return PromotionDecision {
            status: PromotionStatus::Held,
            fit_delta_bps,
            val_delta_bps,
            reason: "unmeasurable fit/validate delta — hold".to_string(),
        };
    }
    if fit_delta_bps.abs() < PROMOTION_MIN_ABS_DELTA_BPS
        || val_delta_bps.abs() < PROMOTION_MIN_ABS_DELTA_BPS
    {
        return PromotionDecision {
            status: PromotionStatus::Held,
            fit_delta_bps,
            val_delta_bps,
            reason: "deltas below minimum magnitude — hold".to_string(),
        };
    }
    if fit_delta_bps.signum() == val_delta_bps.signum() {
        PromotionDecision {
            status: PromotionStatus::Promoted,
            fit_delta_bps,
            val_delta_bps,
            reason: "fit and validate deltas agree in sign — promote".to_string(),
        }
    } else {
        PromotionDecision {
            status: PromotionStatus::Demoted,
            fit_delta_bps,
            val_delta_bps,
            reason: "fit/validate sign disagreement — demote to Phase-1 baseline".to_string(),
        }
    }
}

/// Half-open stress span `[start_ms, end_ms)` excluded from calibration
/// (e.g. mass-liquidation weeks, labeled as stress data per spec).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StressSpan {
    pub start_ms: i64,
    pub end_ms: i64,
}

impl StressSpan {
    pub fn contains(self, ts_ms: i64) -> bool {
        self.start_ms <= ts_ms && ts_ms < self.end_ms
    }
}

/// Parse the durable stress-span list (JSON array of
/// `{start_ms, end_ms}`). Invalid JSON or malformed entries degrade to an
/// empty list (no exclusions) — never to a fabricated span.
pub fn parse_stress_spans_json(json: &str) -> Vec<StressSpan> {
    let trimmed = json.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let val: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let arr = match val.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for item in arr {
        let (Some(s), Some(e)) = (
            item.get("start_ms").and_then(|v| v.as_i64()),
            item.get("end_ms").and_then(|v| v.as_i64()),
        ) else {
            continue;
        };
        if s >= 0 && e > s {
            out.push(StressSpan {
                start_ms: s,
                end_ms: e,
            });
        }
    }
    out
}

/// Drop days whose UTC-day bucket overlaps any stress span. Length mismatch
/// degrades to empty (fail closed — no calibration on misaligned input).
pub fn exclude_stress_days(
    day_start_ms: &[i64],
    day_vals: &[f64],
    stress: &[StressSpan],
) -> (Vec<i64>, Vec<f64>) {
    const DAY_MS: i64 = 86_400_000;
    if day_start_ms.len() != day_vals.len() {
        return (Vec::new(), Vec::new());
    }
    if stress.is_empty() {
        return (day_start_ms.to_vec(), day_vals.to_vec());
    }
    let mut kept_ts = Vec::new();
    let mut kept_vals = Vec::new();
    for (ts, v) in day_start_ms.iter().zip(day_vals.iter()) {
        let day_end = match ts.checked_add(DAY_MS) {
            Some(e) => e,
            None => continue,
        };
        let stressed = stress.iter().any(|s| s.start_ms < day_end && *ts < s.end_ms);
        if !stressed {
            kept_ts.push(*ts);
            kept_vals.push(*v);
        }
    }
    (kept_ts, kept_vals)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic HAR-following log-RV series with known coefficients.
    fn har_series(n: usize) -> Vec<f64> {
        // Seed 22 log values, then recurse exact HAR(0.2, 0.5, 0.3, 0.15).
        let mut log: Vec<f64> = (0..22).map(|i| 2.0 + 0.05 * (i as f64)).collect();
        for t in 21..(n - 1) {
            let w: f64 = log[(t + 1 - 5)..=t].iter().sum::<f64>() / 5.0;
            let m: f64 = log[(t + 1 - 22)..=t].iter().sum::<f64>() / 22.0;
            // Tiny deterministic wiggle so R² < 1 but recovery stays tight.
            let wiggle = 0.002 * ((t % 7) as f64 - 3.0);
            log.push(0.2 + 0.5 * log[t] + 0.3 * w + 0.15 * m + wiggle);
        }
        log.into_iter().map(|l| l.exp()).collect()
    }

    #[test]
    fn har_fit_recovers_known_coefficients() {
        let rv = har_series(90);
        let a = fit_log_har(&rv).expect("exact HAR series must fit");
        assert_eq!(a.n_days, 90);
        assert!((a.intercept - 0.2).abs() < 0.05, "c={}", a.intercept);
        assert!((a.b_daily - 0.5).abs() < 0.08, "b_d={}", a.b_daily);
        assert!((a.b_weekly - 0.3).abs() < 0.12, "b_w={}", a.b_weekly);
        assert!((a.b_monthly - 0.15).abs() < 0.12, "b_m={}", a.b_monthly);
        assert!(a.r2 > 0.9, "r2={}", a.r2);
    }

    #[test]
    fn har_fit_rejects_degenerate_input() {
        assert_eq!(fit_log_har(&[]), None);
        assert_eq!(fit_log_har(&[10.0; 10]), None);
        assert_eq!(fit_log_har(&[10.0; 90]), None); // constant → no variance
        let mut short = har_series(29);
        assert_eq!(fit_log_har(&short), None);
        short.push(f64::NAN);
        let mut nan = har_series(60);
        nan[40] = f64::NAN;
        assert_eq!(fit_log_har(&nan), None);
        let mut neg = har_series(60);
        neg[40] = -5.0;
        assert_eq!(fit_log_har(&neg), None);
    }

    #[test]
    fn har_forecast_and_rescale_behave() {
        let rv = har_series(90);
        let a = fit_log_har(&rv).expect("fit");
        let f = har_forecast_next(&a, &rv).expect("forecast");
        assert!(f.is_finite() && f > 0.0);
        // Forecast lands near the trailing mean for a smooth series.
        let mean: f64 = rv[rv.len() - 22..].iter().sum::<f64>() / 22.0;
        assert!((f / mean - 1.0).abs() < 0.25, "f={f} mean={mean}");
        assert_eq!(har_forecast_next(&a, &[]), None);
        assert_eq!(har_forecast_next(&a, &rv[..10]), None);
        // Rescale clamps and fail-closes.
        assert!((rescale_factor(10.0, 10.0) - 1.0).abs() < 1e-12);
        assert_eq!(rescale_factor(100.0, 10.0), HAR_RESCALE_HI);
        assert_eq!(rescale_factor(1.0, 10.0), HAR_RESCALE_LO);
        assert_eq!(rescale_factor(f64::NAN, 10.0), 1.0);
        assert_eq!(rescale_factor(10.0, 0.0), 1.0);
        assert_eq!(rescale_session_rv(None, 12.0, 10.0), None);
        let r = rescale_session_rv(Some(10.0), 12.0, 10.0).unwrap();
        assert!((r - 12.0).abs() < 1e-9);
        assert_eq!(rescale_session_rv(Some(10.0), f64::NAN, 10.0), Some(10.0));
    }

    #[test]
    fn coupling_degrades_to_neutral() {
        let c = coupling_inputs(Some(12.0), Some(10.0), Some(9.0), Some(10.0));
        assert!((c.tau_scale - 1.2).abs() < 1e-12);
        assert!((c.sigma_scale - 0.9).abs() < 1e-12);
        // Clamps.
        let cl = coupling_inputs(Some(100.0), Some(10.0), Some(100.0), Some(10.0));
        assert_eq!(cl.tau_scale, 2.0);
        assert_eq!(cl.sigma_scale, 1.5);
        // Missing / provisional / degenerate → 1.0.
        assert_eq!(
            coupling_inputs(None, Some(10.0), None, Some(10.0)),
            SessionCoupling {
                tau_scale: 1.0,
                sigma_scale: 1.0
            }
        );
        assert_eq!(
            coupling_inputs(Some(10.0), Some(0.0), Some(10.0), Some(f64::NAN)),
            SessionCoupling {
                tau_scale: 1.0,
                sigma_scale: 1.0
            }
        );
    }

    #[test]
    fn walk_forward_windows_split_and_skew() {
        let now = 1_000 * 86_400_000;
        let w = walk_forward_windows(now, 7, 3).expect("window");
        assert_eq!(w.val_end_ms, now);
        assert_eq!(w.val_start_ms, w.fit_end_ms);
        assert_eq!(w.fit_end_ms - w.fit_start_ms, 7 * 86_400_000);
        assert_eq!(w.val_end_ms - w.val_start_ms, 3 * 86_400_000);
        // Clock skew / degenerate spans fail closed.
        assert_eq!(walk_forward_windows(0, 7, 3), None);
        assert_eq!(walk_forward_windows(-5, 7, 3), None);
        assert_eq!(walk_forward_windows(now, 0, 3), None);
        assert_eq!(walk_forward_windows(now, 7, -1), None);
        assert_eq!(walk_forward_windows(1000, 7, 3), None); // window underflows
    }

    #[test]
    fn promotion_needs_sign_agreement() {
        let p = decide_promotion(2.0, 1.5);
        assert_eq!(p.status, PromotionStatus::Promoted);
        let d = decide_promotion(2.0, -1.5);
        assert_eq!(d.status, PromotionStatus::Demoted);
        assert_eq!(decide_promotion(0.1, 0.1).status, PromotionStatus::Held);
        assert_eq!(decide_promotion(f64::NAN, 1.0).status, PromotionStatus::Held);
        assert_eq!(decide_promotion(0.0, 0.0).status, PromotionStatus::Held);
        assert_eq!(PromotionStatus::parse("promoted"), Some(PromotionStatus::Promoted));
        assert_eq!(PromotionStatus::parse("bogus"), None);
    }

    #[test]
    fn stress_days_excluded_and_parsed() {
        let days: Vec<i64> = (0..10).map(|d| d * 86_400_000).collect();
        let vals: Vec<f64> = (0..10).map(|d| d as f64 + 1.0).collect();
        let spans = vec![StressSpan {
            start_ms: 2 * 86_400_000,
            end_ms: 4 * 86_400_000,
        }];
        let (ts, vs) = exclude_stress_days(&days, &vals, &spans);
        assert_eq!(ts.len(), 8);
        assert!(!ts.contains(&(2 * 86_400_000)));
        assert!(!ts.contains(&(3 * 86_400_000)));
        assert_eq!(vs.len(), 8);
        // Length mismatch → empty (fail closed).
        assert_eq!(exclude_stress_days(&[1], &[1.0, 2.0], &spans).0.len(), 0);
        // Parsing.
        assert!(parse_stress_spans_json("").is_empty());
        assert!(parse_stress_spans_json("bogus").is_empty());
        let ok = parse_stress_spans_json(
            r#"[{"start_ms": 1000, "end_ms": 2000}, {"start_ms": -5, "end_ms": 3}]"#,
        );
        assert_eq!(ok.len(), 1);
    }
}
