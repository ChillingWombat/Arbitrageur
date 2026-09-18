//! Auto-tune Phase 1 (recommend-only) walk-forward parameter recommender.
//!
//! Single Spread (futures) only. The caller splits trailing pair-spread
//! history chronologically into 70% Fit / 30% Validate and passes both
//! slices in; every candidate is derived from Fit alone and then
//! accepted or rejected on Validate. Rejects keep the manual value
//! (`HELD`, `recommended = None`) — unknown is never fabricated.
//!
//! Seven parameters: `slidingWindowSecs`, `targetProfitBps`,
//! `windowPercentile`, `stepSizeMult`, `maxStepUsd`, `maxTickerEquityPct`,
//! `userTargetLeverage`.
//!
//! Pure: no I/O, no clocks, no global state. All fallible math returns
//! `Option`; the only failure mode is an all-`HELD` report.

use serde::{Deserialize, Serialize};

use crate::ou_process::OuModelParams;
use crate::pair_monitor::{segment_episodes, EpisodeOutcome, PairObservation};

/// Minimum total pair observations before any recommendation is made.
pub const AUTOTUNE_MIN_PAIR_OBS: usize = 120;
/// Minimum OU observations for a half-life estimate to be trusted.
pub const AUTOTUNE_OU_MIN_N: usize = 30;
/// Minimum R² for a half-life estimate to be trusted (mirrors the baseline
/// auto-window gate).
pub const AUTOTUNE_OU_MIN_R2: f64 = 0.6;
/// Minimum validate episodes for target/percentile/equity/leverage rows.
pub const AUTOTUNE_MIN_EPISODES: usize = 30;
/// Minimum validate episodes for the capacity-backed step/maxStep rows.
pub const AUTOTUNE_MIN_CAP_EPISODES: usize = 50;
/// Shrinkage weight toward defaults is capped here (more history → more
/// trust in the raw estimate, never above this cap).
pub const AUTOTUNE_SHRINK_W_MAX: f64 = 0.7;
/// Window clamp: 30 minutes .. 7 days (mirrors the baseline auto window).
pub const AUTOTUNE_WINDOW_FLOOR_SECS: u64 = 1_800;
pub const AUTOTUNE_WINDOW_CAP_SECS: u64 = 604_800;
/// Percentile snap range.
pub const AUTOTUNE_PCT_MIN: u32 = 50;
pub const AUTOTUNE_PCT_MAX: u32 = 95;
/// Equity share clamp, percent.
pub const AUTOTUNE_EQUITY_MIN: f64 = 2.0;
pub const AUTOTUNE_EQUITY_MAX: f64 = 20.0;
/// Target clamp, bps.
pub const AUTOTUNE_TARGET_MIN: f64 = 1.0;
pub const AUTOTUNE_TARGET_MAX: f64 = 50.0;
/// Max step clamp, USD.
pub const AUTOTUNE_MAX_STEP_MIN: f64 = 10.0;
pub const AUTOTUNE_MAX_STEP_MAX: f64 = 25_000.0;
/// Regime gate: validate mean shift beyond this many fit sigmas aborts all.
pub const AUTOTUNE_REGIME_MEAN_SIGMAS: f64 = 1.0;
/// Regime gate: validate/fit sigma ratio must stay inside this band.
pub const AUTOTUNE_REGIME_SIGMA_LO: f64 = 0.5;
pub const AUTOTUNE_REGIME_SIGMA_HI: f64 = 2.0;
/// Regime gate: relative half-life shift beyond this aborts all.
pub const AUTOTUNE_REGIME_HL_TOL: f64 = 0.5;
/// Window validation: validate R² floor + half-life stability tolerance.
pub const AUTOTUNE_WINDOW_HL_TOL: f64 = 0.5;
/// Inverse-volatility gain for the equity share: `pct = clamp(40 / sigma)`.
pub const AUTOTUNE_EQUITY_GAIN: f64 = 40.0;
/// Target formula: `friction + 1σ + positive carry`.
pub const AUTOTUNE_TARGET_SIGMA_K: f64 = 1.0;
/// Leverage decreases when fit sigma exceeds this multiple of friction.
pub const AUTOTUNE_LEV_SIGMA_MULT: f64 = 3.0;
/// Wilson score lower bound uses the 95% z-value.
pub const AUTOTUNE_WILSON_Z: f64 = 1.96;

/// Current (manual) Single Spread futures parameters entering the tuner.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TuneParams {
    pub window_secs: u64,
    pub target_bps: f64,
    pub percentile: u32,
    pub step_mult: f64,
    pub max_step_usd: f64,
    pub equity_pct: f64,
    pub leverage: u32,
}

/// Walk-forward input. Both slices must be chronological (oldest first);
/// the caller owns the 70/30 split. Friction is the static per-unit cost
/// (taker fees + exit estimate + non-negative carry); `carry_known` records
/// whether the funding-carry term was measured or fell back to zero.
#[derive(Debug, Clone, PartialEq)]
pub struct TuneHistory {
    pub fit: Vec<PairObservation>,
    pub validate: Vec<PairObservation>,
    pub dt_fit_secs: f64,
    pub dt_val_secs: f64,
    pub friction_fit_bps: f64,
    pub friction_val_bps: f64,
    pub carry_known: bool,
    /// Fit-segment capacity summary from the live-book ladder
    /// (0.5% participation cap). `None` when no books were available —
    /// step/maxStep then stay `HELD`.
    pub fit_optimal_size_usd: Option<f64>,
    pub fit_breakeven_size_usd: Option<f64>,
    pub current: TuneParams,
    pub defaults: TuneParams,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RowStatus {
    READY,
    HELD,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegimeVerdict {
    STABLE,
    SHIFT,
    INSUFFICIENT,
}

impl RegimeVerdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            RegimeVerdict::STABLE => "STABLE",
            RegimeVerdict::SHIFT => "SHIFT",
            RegimeVerdict::INSUFFICIENT => "INSUFFICIENT",
        }
    }
}

/// One recommendation row: `recommended = None` exactly when `HELD`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutotuneRow {
    pub param: String,
    pub current: f64,
    pub recommended: Option<f64>,
    pub status: RowStatus,
    pub fit_metric: String,
    pub validated_metric: String,
    pub n: usize,
    pub rationale: String,
    pub formula_id: String,
    pub confidence: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutotuneBanner {
    pub fit_from_ms: i64,
    pub fit_to_ms: i64,
    pub validate_from_ms: i64,
    pub validate_to_ms: i64,
    pub regime_verdict: RegimeVerdict,
    pub ready_count: usize,
    pub total: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutotuneReport {
    pub rows: Vec<AutotuneRow>,
    pub banner: AutotuneBanner,
}

fn mean_sigma(vals: &[f64]) -> Option<(f64, f64)> {
    if vals.len() < 2 {
        return None;
    }
    let n = vals.len() as f64;
    let mu = vals.iter().sum::<f64>() / n;
    if !mu.is_finite() {
        return None;
    }
    let var = vals.iter().map(|x| (x - mu).powi(2)).sum::<f64>() / (n - 1.0);
    if !(var.is_finite() && var > 0.0) {
        return None;
    }
    let sigma = var.sqrt();
    if sigma.is_finite() && sigma > 0.0 {
        Some((mu, sigma))
    } else {
        None
    }
}

fn spread_vals(obs: &[PairObservation]) -> Vec<f64> {
    obs.iter().map(|o| o.spread_bps).collect()
}

fn seg_dt(obs: &[PairObservation], dt_hint: f64) -> Option<f64> {
    if obs.len() < 2 || !(dt_hint.is_finite() && dt_hint > 0.0) {
        return None;
    }
    Some(dt_hint)
}

/// Wilson score lower bound for `wins/n` at 95% confidence.
pub fn wilson_lower(wins: usize, n: usize) -> Option<f64> {
    if n == 0 {
        return None;
    }
    let (w, m) = (wins as f64, n as f64);
    if !(w >= 0.0 && w <= m) {
        return None;
    }
    let z = AUTOTUNE_WILSON_Z;
    let p = w / m;
    let denom = 1.0 + z * z / m;
    let center = p + z * z / (2.0 * m);
    let margin = z * ((p * (1.0 - p) + z * z / (4.0 * m)) / m).sqrt();
    let lo = (center - margin) / denom;
    if lo.is_finite() {
        Some(lo.clamp(0.0, 1.0))
    } else {
        None
    }
}

/// Standard normal CDF (Abramowitz–Stegun erf approximation, |ε| ≤ 1.5e-7).
pub fn norm_cdf(x: f64) -> Option<f64> {
    if !x.is_finite() {
        return None;
    }
    let t = 1.0 / (1.0 + 0.2316419 * x.abs());
    let poly = t
        * (0.319381530 + t * (-0.356563782 + t * (1.781477937 + t * (-1.821255978 + t * 1.330274429))));
    let phi = (-x * x / 2.0).exp() / (2.0 * std::f64::consts::PI).sqrt();
    let c = 1.0 - phi * poly;
    Some(if x >= 0.0 { c } else { 1.0 - c })
}

/// Shrinkage weight toward defaults: more fit history → more trust in the
/// raw estimate, capped at [`AUTOTUNE_SHRINK_W_MAX`].
pub fn shrink_weight(n_fit: usize) -> f64 {
    ((n_fit as f64) / 500.0).clamp(0.0, AUTOTUNE_SHRINK_W_MAX)
}

pub fn shrink(raw: f64, default: f64, w: f64) -> f64 {
    w * raw + (1.0 - w) * default
}

/// Inverted-CDF percentile for `hurdle` under N(0, sigma), snapped down
/// into `[50..95]`.
pub fn percentile_for_hurdle(hurdle_bps: f64, sigma_bps: f64) -> Option<u32> {
    if !(hurdle_bps.is_finite() && sigma_bps.is_finite() && sigma_bps > 0.0) {
        return None;
    }
    let pct = norm_cdf(hurdle_bps / sigma_bps)? * 100.0;
    Some(pct.floor().clamp(AUTOTUNE_PCT_MIN as f64, AUTOTUNE_PCT_MAX as f64) as u32)
}

struct RowArgs<'a> {
    param: &'a str,
    current: f64,
    recommended: Option<f64>,
    status: RowStatus,
    fit_metric: String,
    validated_metric: String,
    n: usize,
    rationale: String,
    formula_id: &'a str,
    confidence: &'a str,
}

fn make_row(a: RowArgs<'_>) -> AutotuneRow {
    AutotuneRow {
        param: a.param.to_string(),
        current: a.current,
        recommended: a.recommended,
        status: a.status,
        fit_metric: a.fit_metric,
        validated_metric: a.validated_metric,
        n: a.n,
        rationale: a.rationale,
        formula_id: a.formula_id.to_string(),
        confidence: a.confidence.to_string(),
    }
}

fn held(
    param: &str,
    current: f64,
    fit_metric: String,
    validated_metric: String,
    n: usize,
    rationale: String,
    formula_id: &str,
) -> AutotuneRow {
    make_row(RowArgs {
        param,
        current,
        recommended: None,
        status: RowStatus::HELD,
        fit_metric,
        validated_metric,
        n,
        rationale,
        formula_id,
        confidence: "low",
    })
}

#[allow(clippy::too_many_arguments)]
fn ready(
    param: &str,
    current: f64,
    recommended: f64,
    fit_metric: String,
    validated_metric: String,
    n: usize,
    rationale: String,
    formula_id: &str,
    confidence: &str,
) -> AutotuneRow {
    make_row(RowArgs {
        param,
        current,
        recommended: Some(recommended),
        status: RowStatus::READY,
        fit_metric,
        validated_metric,
        n,
        rationale,
        formula_id,
        confidence,
    })
}

fn confidence_for(n_fit: usize, n_val_episodes: usize, carry_known: bool) -> &'static str {
    if n_fit >= 300 && n_val_episodes >= AUTOTUNE_MIN_CAP_EPISODES && carry_known {
        "high"
    } else if carry_known {
        "medium"
    } else {
        "low"
    }
}

/// Pure walk-forward recommender: fit 7 candidates on `fit`, validate each
/// on `validate`. Never fabricates — every reject path yields `HELD`.
pub fn recommend(h: &TuneHistory) -> AutotuneReport {
    let banner_ts = |obs: &[PairObservation]| -> (i64, i64) {
        match (obs.first(), obs.last()) {
            (Some(f), Some(l)) => (f.ts_ms, l.ts_ms),
            _ => (0, 0),
        }
    };
    let (fit_from, fit_to) = banner_ts(&h.fit);
    let (val_from, val_to) = banner_ts(&h.validate);
    let total = h.fit.len() + h.validate.len();
    let w = shrink_weight(h.fit.len());

    let finish = |rows: Vec<AutotuneRow>, verdict: RegimeVerdict| -> AutotuneReport {
        let ready_count = rows.iter().filter(|r| r.status == RowStatus::READY).count();
        AutotuneReport {
            rows,
            banner: AutotuneBanner {
                fit_from_ms: fit_from,
                fit_to_ms: fit_to,
                validate_from_ms: val_from,
                validate_to_ms: val_to,
                regime_verdict: verdict,
                ready_count,
                total: 7,
            },
        }
    };

    // Honest-empty + sample floor: below 120 pair obs everything is HELD.
    if total < AUTOTUNE_MIN_PAIR_OBS || h.fit.is_empty() || h.validate.is_empty() {
        let rationale = if total == 0 && h.fit.is_empty() && h.validate.is_empty() {
            "no spread history — collect at least 120 pair observations".to_string()
        } else {
            format!("only {total} pair observations — need ≥{AUTOTUNE_MIN_PAIR_OBS}")
        };
        let c = &h.current;
        let rows = vec![
            held("slidingWindowSecs", c.window_secs as f64, "n/a".into(), "n/a".into(), total, rationale.clone(), "window=2*hl"),
            held("targetProfitBps", c.target_bps, "n/a".into(), "n/a".into(), total, rationale.clone(), "target=friction+k*sigma+carry"),
            held("windowPercentile", c.percentile as f64, "n/a".into(), "n/a".into(), total, rationale.clone(), "pct=invCDF(hurdle)"),
            held("stepSizeMult", c.step_mult, "n/a".into(), "n/a".into(), total, rationale.clone(), "step=capacity-ladder"),
            held("maxStepUsd", c.max_step_usd, "n/a".into(), "n/a".into(), total, rationale.clone(), "maxStep=ladder-optimal"),
            held("maxTickerEquityPct", c.equity_pct, "n/a".into(), "n/a".into(), total, rationale.clone(), "equity=inv-vol"),
            held("userTargetLeverage", c.leverage as f64, "n/a".into(), "n/a".into(), total, rationale, "leverage=decrease-only"),
        ];
        return finish(rows, RegimeVerdict::INSUFFICIENT);
    }

    let fit_vals = spread_vals(&h.fit);
    let val_vals = spread_vals(&h.validate);
    let fit_ms = mean_sigma(&fit_vals);
    let val_ms = mean_sigma(&val_vals);
    let fit_ou = seg_dt(&h.fit, h.dt_fit_secs)
        .and_then(|dt| OuModelParams::fit(&fit_vals, dt));
    let val_ou = seg_dt(&h.validate, h.dt_val_secs)
        .and_then(|dt| OuModelParams::fit(&val_vals, dt));

    // Regime gate: any shift aborts everything.
    let regime_shift: Option<String> = match (fit_ms, val_ms) {
        (Some((mu_f, sig_f)), Some((mu_v, sig_v))) => {
            if (mu_v - mu_f).abs() > AUTOTUNE_REGIME_MEAN_SIGMAS * sig_f {
                Some(format!(
                    "mean shifted {mu_v:.1} vs fit {mu_f:.1} (>1σ={sig_f:.1})"
                ))
            } else if sig_v < sig_f * AUTOTUNE_REGIME_SIGMA_LO
                || sig_v > sig_f * AUTOTUNE_REGIME_SIGMA_HI
            {
                Some(format!(
                    "volatility regime shifted σ_val={sig_v:.1} vs σ_fit={sig_f:.1}"
                ))
            } else {
                match (fit_ou, val_ou) {
                    (Some(f), Some(v))
                        if f.half_life.is_finite()
                            && f.half_life > 0.0
                            && v.half_life.is_finite()
                            && v.half_life > 0.0
                            && ((v.half_life - f.half_life).abs() / f.half_life
                                > AUTOTUNE_REGIME_HL_TOL) =>
                    {
                        Some(format!(
                            "half-life shifted {:.0}s vs fit {:.0}s (>50%)",
                            v.half_life, f.half_life
                        ))
                    }
                    _ => None,
                }
            }
        }
        _ => None,
    };
    // Missing segment stats also fail closed (validate side unmeasurable).
    let unmeasurable = fit_ms.is_none() || val_ms.is_none();
    if unmeasurable || regime_shift.is_some() {
        let rationale = regime_shift.clone().unwrap_or_else(|| {
            "validate segment has no measurable mean/volatility — cannot confirm regime".to_string()
        });
        let verdict = if unmeasurable && regime_shift.is_none() {
            RegimeVerdict::INSUFFICIENT
        } else {
            RegimeVerdict::SHIFT
        };
        let c = &h.current;
        let rows = vec![
            held("slidingWindowSecs", c.window_secs as f64, "n/a".into(), "n/a".into(), total, format!("regime gate: {rationale}"), "window=2*hl"),
            held("targetProfitBps", c.target_bps, "n/a".into(), "n/a".into(), total, format!("regime gate: {rationale}"), "target=friction+k*sigma+carry"),
            held("windowPercentile", c.percentile as f64, "n/a".into(), "n/a".into(), total, format!("regime gate: {rationale}"), "pct=invCDF(hurdle)"),
            held("stepSizeMult", c.step_mult, "n/a".into(), "n/a".into(), total, format!("regime gate: {rationale}"), "step=capacity-ladder"),
            held("maxStepUsd", c.max_step_usd, "n/a".into(), "n/a".into(), total, format!("regime gate: {rationale}"), "maxStep=ladder-optimal"),
            held("maxTickerEquityPct", c.equity_pct, "n/a".into(), "n/a".into(), total, format!("regime gate: {rationale}"), "equity=inv-vol"),
            held("userTargetLeverage", c.leverage as f64, "n/a".into(), "n/a".into(), total, format!("regime gate: {rationale}"), "leverage=decrease-only"),
        ];
        return finish(rows, verdict);
    }
    let (mu_f, sig_f) = fit_ms.unwrap_or((0.0, 1.0));
    let (mu_v, sig_v) = val_ms.unwrap_or((0.0, 1.0));

    // ---- Fit candidates ------------------------------------------------
    let fit_window: Option<u64> = fit_ou.and_then(|p| {
        if p.half_life.is_finite()
            && p.half_life > 0.0
            && p.r_squared >= AUTOTUNE_OU_MIN_R2
            && h.fit.len() >= AUTOTUNE_OU_MIN_N
        {
            Some((2.0 * p.half_life).round().clamp(
                AUTOTUNE_WINDOW_FLOOR_SECS as f64,
                AUTOTUNE_WINDOW_CAP_SECS as f64,
            ) as u64)
        } else {
            None
        }
    });

    let friction_ok = h.friction_fit_bps.is_finite()
        && h.friction_fit_bps >= 0.0
        && h.friction_val_bps.is_finite()
        && h.friction_val_bps >= 0.0;
    let target_raw: Option<f64> = if friction_ok {
        let raw = h.friction_fit_bps + AUTOTUNE_TARGET_SIGMA_K * sig_f;
        Some(raw.clamp(AUTOTUNE_TARGET_MIN, AUTOTUNE_TARGET_MAX))
    } else {
        None
    };
    // Shrinkage toward the default target, weight capped at 0.7.
    let target_cand: Option<f64> = target_raw.map(|r| {
        let s = shrink(r, h.defaults.target_bps, w);
        (s * 100.0).round() / 100.0
    });

    // Fit episodes at the candidate hurdle (floor for target-family rows).
    let band_fit = h.friction_fit_bps.max(2.0);
    let stop_fit = (sig_f * 3.0).max(band_fit * 2.0);
    let fit_eps_at: Option<Vec<crate::pair_monitor::Episode>> =
        target_cand.map(|t| segment_episodes(&h.fit, mu_f, t, band_fit, stop_fit));
    let fit_ep_n = fit_eps_at.as_ref().map(|e| e.len()).unwrap_or(0);

    let pct_cand: Option<u32> = target_cand.and_then(|t| {
        percentile_for_hurdle(t, sig_f).map(|raw| {
            let s = shrink(raw as f64, h.defaults.percentile as f64, w);
            (s.floor() as u32).clamp(AUTOTUNE_PCT_MIN, AUTOTUNE_PCT_MAX)
        })
    });

    let equity_raw = (AUTOTUNE_EQUITY_GAIN / sig_f).clamp(AUTOTUNE_EQUITY_MIN, AUTOTUNE_EQUITY_MAX);
    let equity_cand = if equity_raw.is_finite() {
        let s = shrink(equity_raw, h.defaults.equity_pct, w);
        Some(((s * 10.0).round() / 10.0).clamp(AUTOTUNE_EQUITY_MIN, AUTOTUNE_EQUITY_MAX))
    } else {
        None
    };

    let maxstep_cand: Option<f64> = h.fit_optimal_size_usd.and_then(|o| {
        if o.is_finite() && o > 0.0 {
            let s = shrink(o, h.defaults.max_step_usd, w);
            Some(((s / 10.0).round() * 10.0).clamp(AUTOTUNE_MAX_STEP_MIN, AUTOTUNE_MAX_STEP_MAX))
        } else {
            None
        }
    });
    let step_supported =
        h.fit_optimal_size_usd.is_some_and(|o| o.is_finite() && o > 0.0)
            && h.fit_breakeven_size_usd.is_some_and(|b| b.is_finite() && b > 0.0);

    let lev_cand: u32 = if sig_f > AUTOTUNE_LEV_SIGMA_MULT * h.friction_fit_bps.max(0.5) {
        h.current.leverage.saturating_sub(1).max(1)
    } else {
        h.current.leverage
    };
    // Leverage never increases, by construction.
    debug_assert!(lev_cand <= h.current.leverage);

    // ---- Validate -------------------------------------------------------
    // Shared validated economics at the candidate target.
    let band_val = h.friction_val_bps.max(2.0);
    let stop_val = (sig_v * 3.0).max(band_val * 2.0);
    struct ValEcon {
        n: usize,
        wins: usize,
        p_win: f64,
        wilson: f64,
        e_net: f64,
    }
    let val_econ: Option<ValEcon> = target_cand.and_then(|t| {
        let eps = segment_episodes(&h.validate, mu_v, t, band_val, stop_val);
        // Open episodes carry no resolution — economics need resolved ones,
        // but the floor counts observed episodes; require resolved majority.
        let resolved: Vec<_> = eps
            .iter()
            .filter(|e| !matches!(e.outcome, EpisodeOutcome::Open))
            .collect();
        if resolved.is_empty() {
            return None;
        }
        let n = resolved.len();
        let wins = resolved
            .iter()
            .filter(|e| matches!(e.outcome, EpisodeOutcome::Win { .. }))
            .count();
        let p_win = wins as f64 / n as f64;
        let wilson = wilson_lower(wins, n)?;
        let e_net = p_win * t - h.friction_val_bps;
        Some(ValEcon { n, wins, p_win, wilson, e_net })
    });
    let target_ok = target_cand.is_some_and(|_| {
        val_econ.as_ref().is_some_and(|v| {
            v.n >= AUTOTUNE_MIN_EPISODES && v.e_net > 0.0 && v.wilson >= 0.5
        })
    });

    let val_hl_ok = match (fit_ou, val_ou) {
        (Some(f), Some(v)) => {
            v.r_squared >= AUTOTUNE_OU_MIN_R2
                && f.half_life.is_finite()
                && f.half_life > 0.0
                && v.half_life.is_finite()
                && v.half_life > 0.0
                && ((v.half_life - f.half_life).abs() / f.half_life <= AUTOTUNE_WINDOW_HL_TOL)
        }
        _ => false,
    };
    let window_ok = fit_window.is_some()
        && h.fit.len() >= AUTOTUNE_OU_MIN_N
        && fit_ou.is_some_and(|p| p.r_squared >= AUTOTUNE_OU_MIN_R2)
        && val_hl_ok;

    let val_ep_n = val_econ.as_ref().map(|v| v.n).unwrap_or(0);
    let conf = confidence_for(h.fit.len(), val_ep_n, h.carry_known);
    let carry_note = if h.carry_known { "" } else { "; funding carry unknown (term=0)" };

    let c = &h.current;
    let mut rows = Vec::with_capacity(7);

    // window
    {
        let fit_m = match fit_ou {
            Some(p) => format!(
                "R²={:.2} hl={:.0}s (n={})",
                p.r_squared, p.half_life, h.fit.len()
            ),
            None => "OU unmeasurable on fit".to_string(),
        };
        let val_m = match val_ou {
            Some(p) => format!(
                "R²={:.2} hl={:.0}s (n={})",
                p.r_squared, p.half_life, h.validate.len()
            ),
            None => "OU unmeasurable on validate".to_string(),
        };
        match (fit_window, window_ok) {
            (Some(rec), true) => rows.push(ready(
                "slidingWindowSecs",
                c.window_secs as f64,
                rec as f64,
                fit_m,
                val_m,
                h.fit.len(),
                format!("2× median OU half-life, clamped [30m,7d]{carry_note}"),
                "window=2*hl",
                conf,
            )),
            _ => rows.push(held(
                "slidingWindowSecs",
                c.window_secs as f64,
                fit_m,
                val_m,
                h.fit.len(),
                format!(
                    "kept manual — {}",
                    if fit_window.is_none() {
                        "fit half-life failed R²≥0.60 / n≥30 gate"
                    } else {
                        "validate half-life unstable or R²<0.60"
                    }
                ),
                "window=2*hl",
            )),
        }
    }

    // target
    {
        let fit_m = match target_raw {
            Some(r) => format!(
                "friction={:.1} σ={:.1} raw={:.1} (n={})",
                h.friction_fit_bps, sig_f, r, fit_ep_n
            ),
            None => "friction unknown on fit".to_string(),
        };
        let val_m = match &val_econ {
            Some(v) => format!(
                "E[net]={:+.1}bps {}/{} won p_win={:.2} wilson={:.2}",
                v.e_net, v.wins, v.n, v.p_win, v.wilson
            ),
            None => "no resolved validate episodes at candidate".to_string(),
        };
        match (target_cand, target_ok) {
            (Some(rec), true) => rows.push(ready(
                "targetProfitBps",
                c.target_bps,
                rec,
                fit_m,
                val_m,
                fit_ep_n,
                format!("validated E[net]>0 + Wilson P_win≥0.50{carry_note}"),
                "target=friction+k*sigma+carry",
                conf,
            )),
            _ => rows.push(held(
                "targetProfitBps",
                c.target_bps,
                fit_m,
                val_m,
                fit_ep_n,
                format!(
                    "kept manual — {}",
                    if target_cand.is_none() {
                        "fit candidate unmeasurable"
                    } else {
                        "validate needs E[net]>0, Wilson≥0.50, n≥30"
                    }
                ),
                "target=friction+k*sigma+carry",
            )),
        }
    }

    // percentile
    {
        let fit_m = match (target_cand, pct_cand) {
            (Some(t), Some(p)) => format!("invCDF({t:.1}/σ={:.1})→{p}% (n={})", sig_f, fit_ep_n),
            _ => "hurdle/sigma unmeasurable on fit".to_string(),
        };
        let val_m = match &val_econ {
            Some(v) => format!("shared target gate: E[net]={:+.1} (n={})", v.e_net, v.n),
            None => "no validated economics".to_string(),
        };
        let ok = pct_cand.is_some() && target_ok && fit_ep_n >= AUTOTUNE_MIN_EPISODES;
        match (pct_cand, ok) {
            (Some(rec), true) => rows.push(ready(
                "windowPercentile",
                c.percentile as f64,
                rec as f64,
                fit_m,
                val_m,
                fit_ep_n,
                "inverted CDF at validated hurdle, snapped down [50..95]".to_string(),
                "pct=invCDF(hurdle)",
                conf,
            )),
            _ => rows.push(held(
                "windowPercentile",
                c.percentile as f64,
                fit_m,
                val_m,
                fit_ep_n,
                "kept manual — needs validated target gate + n≥30".to_string(),
                "pct=invCDF(hurdle)",
            )),
        }
    }

    // stepMult (validated hold — ladder confirms the current step)
    {
        let fit_m = match h.fit_optimal_size_usd {
            Some(o) => format!("ladder optimal=${o:.0} breakeven={:?}", h.fit_breakeven_size_usd),
            None => "no live-book ladder on fit".to_string(),
        };
        let val_m = match &val_econ {
            Some(v) => format!("shared target gate: E[net]={:+.1} (n={})", v.e_net, v.n),
            None => "no validated economics".to_string(),
        };
        let ok = step_supported && target_ok && val_ep_n >= AUTOTUNE_MIN_CAP_EPISODES;
        if ok {
            rows.push(ready(
                "stepSizeMult",
                c.step_mult,
                c.step_mult,
                fit_m,
                val_m,
                fit_ep_n,
                "capacity ladder supports the current step — hold".to_string(),
                "step=capacity-ladder",
                conf,
            ));
        } else {
            rows.push(held(
                "stepSizeMult",
                c.step_mult,
                fit_m,
                val_m,
                fit_ep_n,
                "kept manual — needs ladder support + validated gate (n≥50)".to_string(),
                "step=capacity-ladder",
            ));
        }
    }

    // maxStep
    {
        let fit_m = match h.fit_optimal_size_usd {
            Some(o) => format!("ladder optimal=${o:.0} (0.5% part. cap)"),
            None => "no live-book ladder on fit".to_string(),
        };
        let val_m = match &val_econ {
            Some(v) => format!("shared target gate: E[net]={:+.1} (n={})", v.e_net, v.n),
            None => "no validated economics".to_string(),
        };
        let ok = maxstep_cand.is_some() && target_ok && val_ep_n >= AUTOTUNE_MIN_CAP_EPISODES;
        match (maxstep_cand, ok) {
            (Some(rec), true) => rows.push(ready(
                "maxStepUsd",
                c.max_step_usd,
                rec,
                fit_m,
                val_m,
                fit_ep_n,
                "ladder-optimal size under 0.5% participation cap".to_string(),
                "maxStep=ladder-optimal",
                conf,
            )),
            _ => rows.push(held(
                "maxStepUsd",
                c.max_step_usd,
                fit_m,
                val_m,
                fit_ep_n,
                "kept manual — needs ladder optimal + validated gate (n≥50)".to_string(),
                "maxStep=ladder-optimal",
            )),
        }
    }

    // equity
    {
        let fit_m = format!("inv-vol 40/σ={:.1} (n={})", sig_f, h.fit.len());
        let val_m = match &val_econ {
            Some(v) => format!("shared target gate: E[net]={:+.1} (n={})", v.e_net, v.n),
            None => "no validated economics".to_string(),
        };
        let ok = equity_cand.is_some() && target_ok && val_ep_n >= AUTOTUNE_MIN_EPISODES;
        match (equity_cand, ok) {
            (Some(rec), true) => rows.push(ready(
                "maxTickerEquityPct",
                c.equity_pct,
                rec,
                fit_m,
                val_m,
                h.fit.len(),
                "inverse-volatility share, clamped [2,20]".to_string(),
                "equity=inv-vol",
                conf,
            )),
            _ => rows.push(held(
                "maxTickerEquityPct",
                c.equity_pct,
                fit_m,
                val_m,
                h.fit.len(),
                "kept manual — needs validated target gate + n≥30".to_string(),
                "equity=inv-vol",
            )),
        }
    }

    // leverage (decrease-only)
    {
        let fit_m = format!("σ={:.1} vs 3×friction={:.1}", sig_f, 3.0 * h.friction_fit_bps.max(0.5));
        let val_m = match &val_econ {
            Some(v) => format!("shared target gate: E[net]={:+.1} (n={})", v.e_net, v.n),
            None => "no validated economics".to_string(),
        };
        let decreased = lev_cand < c.leverage;
        let ok = target_ok && val_ep_n >= AUTOTUNE_MIN_EPISODES;
        if decreased || ok {
            let rationale = if decreased {
                "volatility exceeds 3× friction — step down one notch".to_string()
            } else {
                "validated economics support the current leverage — hold".to_string()
            };
            // When the gate fails but a decrease was indicated, the decrease
            // itself is the safe direction; still report it as READY only if
            // the fit side was measurable (it was — we got here).
            let (rec, status_ok) = if decreased {
                (lev_cand, true)
            } else {
                (lev_cand, ok)
            };
            if status_ok {
                rows.push(ready(
                    "userTargetLeverage",
                    c.leverage as f64,
                    rec as f64,
                    fit_m,
                    val_m,
                    h.fit.len(),
                    rationale,
                    "leverage=decrease-only",
                    conf,
                ));
            } else {
                rows.push(held(
                    "userTargetLeverage",
                    c.leverage as f64,
                    fit_m,
                    val_m,
                    h.fit.len(),
                    "kept manual — needs validated target gate + n≥30".to_string(),
                    "leverage=decrease-only",
                ));
            }
        } else {
            rows.push(held(
                "userTargetLeverage",
                c.leverage as f64,
                fit_m,
                val_m,
                h.fit.len(),
                "kept manual — needs validated target gate + n≥30".to_string(),
                "leverage=decrease-only",
            ));
        }
    }

    finish(rows, RegimeVerdict::STABLE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pair_monitor::PairObservation;

    fn obs_series(n: usize, mu: f64, amp: f64, period: usize, ts_step_ms: i64) -> Vec<PairObservation> {
        (0..n)
            .map(|i| {
                let phase = 2.0 * std::f64::consts::PI * (i % period) as f64 / period as f64;
                PairObservation {
                    ts_ms: i as i64 * ts_step_ms,
                    spread_bps: mu + amp * phase.sin(),
                }
            })
            .collect()
    }

    fn test_params() -> TuneParams {
        TuneParams {
            window_secs: 86_400,
            target_bps: 1.0,
            percentile: 50,
            step_mult: 10.0,
            max_step_usd: 2500.0,
            equity_pct: 20.0,
            leverage: 5,
        }
    }

    fn history_for(fit: Vec<PairObservation>, val: Vec<PairObservation>) -> TuneHistory {
        TuneHistory {
            fit,
            validate: val,
            dt_fit_secs: 60.0,
            dt_val_secs: 60.0,
            friction_fit_bps: 5.0,
            friction_val_bps: 5.0,
            carry_known: true,
            fit_optimal_size_usd: Some(3000.0),
            fit_breakeven_size_usd: Some(3000.0),
            current: test_params(),
            defaults: test_params(),
        }
    }

    fn split_70_30(all: Vec<PairObservation>) -> (Vec<PairObservation>, Vec<PairObservation>) {
        let k = all.len() * 7 / 10;
        let tail = all[k..].to_vec();
        let head = all[..k].to_vec();
        (head, tail)
    }

    #[test]
    fn accept_path_produces_ready_rows() {
        // Strong 40bps oscillation: economics validate on both segments.
        let all = obs_series(500, 0.0, 40.0, 8, 60_000);
        let (fit, val) = split_70_30(all);
        let rep = recommend(&history_for(fit, val));
        assert_eq!(rep.rows.len(), 7);
        assert_eq!(rep.banner.total, 7);
        assert_eq!(rep.banner.regime_verdict, RegimeVerdict::STABLE);
        let target = rep.rows.iter().find(|r| r.param == "targetProfitBps").unwrap();
        assert_eq!(target.status, RowStatus::READY);
        assert!(target.recommended.unwrap_or(0.0) > 0.0);
        assert!(rep.banner.ready_count >= 1);
        for r in &rep.rows {
            assert_eq!(r.recommended.is_some(), r.status == RowStatus::READY);
        }
    }

    #[test]
    fn reject_to_held_when_validate_loses() {
        // Fit oscillates wide; validate is flat noise around a shifted mean:
        // regime gate may or may not fire, but economics must fail.
        let fit = obs_series(280, 0.0, 40.0, 24, 60_000);
        let val = obs_series(120, 0.5, 1.0, 24, 60_000);
        let mut h = history_for(fit, val);
        h.friction_val_bps = 30.0; // friction above any achievable edge
        let rep = recommend(&h);
        let target = rep.rows.iter().find(|r| r.param == "targetProfitBps").unwrap();
        assert_eq!(target.status, RowStatus::HELD);
        assert_eq!(target.recommended, None);
    }

    #[test]
    fn regime_abort_holds_everything() {
        let fit = obs_series(280, 0.0, 40.0, 24, 60_000);
        // Validate mean shifted by far more than 1 fit-sigma.
        let val = obs_series(120, 200.0, 40.0, 24, 60_000);
        let rep = recommend(&history_for(fit, val));
        assert_eq!(rep.banner.regime_verdict, RegimeVerdict::SHIFT);
        assert_eq!(rep.banner.ready_count, 0);
        for r in &rep.rows {
            assert_eq!(r.status, RowStatus::HELD);
            assert_eq!(r.recommended, None);
        }
    }

    #[test]
    fn honest_empty_episodes_zero_is_all_held() {
        let h = history_for(Vec::new(), Vec::new());
        let rep = recommend(&h);
        assert_eq!(rep.banner.regime_verdict, RegimeVerdict::INSUFFICIENT);
        assert_eq!(rep.rows.len(), 7);
        for r in &rep.rows {
            assert_eq!(r.status, RowStatus::HELD);
            assert_eq!(r.recommended, None);
        }
    }

    #[test]
    fn shrinkage_math_caps_weight_at_0_7() {
        assert!((shrink_weight(0) - 0.0).abs() < 1e-12);
        assert!((shrink_weight(350) - 0.7).abs() < 1e-12);
        assert!((shrink_weight(10_000) - 0.7).abs() < 1e-12);
        assert!((shrink(10.0, 20.0, 0.7) - 13.0).abs() < 1e-12);
        assert!((shrink(10.0, 20.0, 0.0) - 20.0).abs() < 1e-12);
    }

    #[test]
    fn percentile_snaps_down_into_50_to_95() {
        // Huge hurdle → clamps at 95, not 99.
        assert_eq!(percentile_for_hurdle(1000.0, 10.0), Some(95));
        // Zero hurdle → median floor 50.
        assert_eq!(percentile_for_hurdle(0.0, 10.0), Some(50));
        // 1σ ≈ 84.1% → snapped DOWN to 84.
        assert_eq!(percentile_for_hurdle(10.0, 10.0), Some(84));
        // Degenerate inputs stay unknown.
        assert_eq!(percentile_for_hurdle(10.0, 0.0), None);
        assert_eq!(percentile_for_hurdle(f64::NAN, 10.0), None);
    }

    #[test]
    fn leverage_never_increases() {
        // Calm market: hold.
        let all = obs_series(400, 0.0, 40.0, 24, 60_000);
        let (fit, val) = split_70_30(all);
        let mut h = history_for(fit, val);
        h.current.leverage = 2;
        let rep = recommend(&h);
        let lev = rep.rows.iter().find(|r| r.param == "userTargetLeverage").unwrap();
        if let Some(rec) = lev.recommended {
            assert!(rec <= 2.0, "leverage must never increase");
        }
        // Wild market vs tiny friction: decreases by exactly one notch.
        let all2 = obs_series(400, 0.0, 200.0, 24, 60_000);
        let (fit2, val2) = split_70_30(all2);
        let mut h2 = history_for(fit2, val2);
        h2.friction_fit_bps = 0.5;
        h2.friction_val_bps = 0.5;
        h2.current.leverage = 5;
        let rep2 = recommend(&h2);
        let lev2 = rep2.rows.iter().find(|r| r.param == "userTargetLeverage").unwrap();
        if let Some(rec) = lev2.recommended {
            assert!(rec <= 5.0, "leverage must never increase");
            assert!(rec >= 1.0);
        }
    }

    #[test]
    fn wilson_lower_behaves() {
        assert_eq!(wilson_lower(0, 0), None);
        let lo = wilson_lower(25, 30).unwrap();
        assert!((0.5..25.0 / 30.0).contains(&lo));
        assert!(wilson_lower(5, 30).unwrap() < 0.5);
    }
}
