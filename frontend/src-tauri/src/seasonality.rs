use arbitrageur_math::pair_monitor::build_pair_series;
use arbitrageur_math::seasonality::{
    coupling_inputs, decide_promotion, exclude_stress_days, fit_log_har, har_forecast_next,
    parse_stress_spans_json, rescale_factor, walk_forward_windows, PromotionStatus,
    SessionCoupling,
};
use arbitrageur_math::session::{session_of, winsorized_median};
use serde::{Deserialize, Serialize};

use crate::state::AppEngineState;
use crate::state_store::{HarAnchorRow, PromotionStateRow};

pub const STRESS_SPANS_KV: &str = "seasonality.stress_spans_json";
pub const LAST_PROMOTION_KV: &str = "seasonality.last_promotion_ts_ms";
const RV_LOOKBACK_DAYS: i64 = 60;
const FIT_DAYS: i64 = 42;
const VAL_DAYS: i64 = 18;
const DAY_MS: i64 = 86_400_000;
const MAX_SYMBOLS_PER_RUN: usize = 500;
const PROMOTION_FRESH_MS: i64 = 48 * 3_600_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeasonalityRunSummary {
    pub symbols_scanned: usize,
    pub anchors_written: usize,
    pub promoted: usize,
    pub demoted: usize,
    pub held: usize,
    pub skipped: usize,
    pub run_ts_ms: i64,
}

fn median_of(vals: &[f64]) -> Option<f64> {
    let mut finite: Vec<f64> = vals.iter().copied().filter(|v| v.is_finite()).collect();
    if finite.is_empty() {
        return None;
    }
    finite.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = finite.len() / 2;
    if finite.len() % 2 == 1 {
        Some(finite[mid])
    } else {
        Some((finite[mid - 1] + finite[mid]) / 2.0)
    }
}

fn day_realized_vol(samples: &[(i64, f64)]) -> Option<f64> {
    if samples.len() < 2 {
        return None;
    }
    let vals: Vec<f64> = samples.iter().map(|(_, m)| *m).collect();
    let mean = vals.iter().sum::<f64>() / vals.len() as f64;
    if !mean.is_finite() || mean <= 0.0 {
        return None;
    }
    let var = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (vals.len() - 1) as f64;
    if var.is_finite() && var > 0.0 {
        Some(10_000.0 * var.sqrt() / mean)
    } else {
        None
    }
}

fn daily_rv_series(
    state: &AppEngineState,
    symbol: &str,
    now_ms: i64,
    days: i64,
) -> Option<(Vec<i64>, Vec<f64>)> {
    let start_ms = now_ms.checked_sub(days.checked_mul(DAY_MS)?)?;
    if start_ms < 0 {
        return None;
    }
    let samples = state
        .state_store
        .spread_samples(symbol, start_ms, now_ms)
        .ok()?;
    let mut mids_by_venue: std::collections::HashMap<String, Vec<(i64, f64)>> =
        std::collections::HashMap::new();
    for s in &samples {
        if s.mid.is_finite() && s.mid > 0.0 {
            mids_by_venue
                .entry(s.venue_id.clone())
                .or_default()
                .push((s.ts_ms, s.mid));
        }
    }
    if mids_by_venue.len() < 2 {
        return None;
    }
    let venues: Vec<String> = mids_by_venue.keys().cloned().collect();
    let mut day_ts = Vec::new();
    let mut day_rv = Vec::new();
    for d in 0..days {
        let Some(day_start) = start_ms.checked_add(d.checked_mul(DAY_MS)?) else {
            continue;
        };
        let Some(day_end) = day_start.checked_add(DAY_MS) else {
            continue;
        };
        let mut spreads: Vec<f64> = Vec::new();
        for (ia, a) in venues.iter().enumerate() {
            for b in venues.iter().skip(ia + 1) {
                let (Some(ma), Some(mb)) = (mids_by_venue.get(a), mids_by_venue.get(b)) else {
                    continue;
                };
                let fa: Vec<(i64, f64)> = ma
                    .iter()
                    .copied()
                    .filter(|(t, _)| *t >= day_start && *t < day_end)
                    .collect();
                let fb: Vec<(i64, f64)> = mb
                    .iter()
                    .copied()
                    .filter(|(t, _)| *t >= day_start && *t < day_end)
                    .collect();
                if fa.len() < 2 || fb.len() < 2 {
                    continue;
                }
                let mut ab = build_pair_series(&fa, &fb, 30_000);
                let mut ba = build_pair_series(&fb, &fa, 30_000);
                spreads.extend(ab.drain(..).map(|o| o.spread_bps.abs()));
                spreads.extend(ba.drain(..).map(|o| o.spread_bps.abs()));
            }
        }
        let pseudo: Vec<(i64, f64)> = spreads
            .iter()
            .enumerate()
            .map(|(i, v)| (day_start + i as i64, *v))
            .collect();
        if let Some(rv) = day_realized_vol(&pseudo) {
            day_ts.push(day_start);
            day_rv.push(rv);
        }
    }
    if day_ts.is_empty() {
        return None;
    }
    Some((day_ts, day_rv))
}

fn run_one_symbol(
    state: &AppEngineState,
    symbol: &str,
    now_ms: i64,
    stress_json: &str,
) -> (bool, PromotionStatus) {
    let stress = parse_stress_spans_json(stress_json);
    let Some((day_ts, day_rv_vals)) = daily_rv_series(state, symbol, now_ms, RV_LOOKBACK_DAYS)
    else {
        return (false, PromotionStatus::Held);
    };
    let (kept_ts, kept_rv) = exclude_stress_days(&day_ts, &day_rv_vals, &stress);
    if kept_rv.len() < arbitrageur_math::seasonality::HAR_MIN_DAYS {
        return (false, PromotionStatus::Held);
    }
    let anchor_written = match fit_log_har(&kept_rv) {
        Some(a) => {
            let trailing: f64 = kept_rv.iter().sum::<f64>() / kept_rv.len() as f64;
            let forecast = har_forecast_next(&a, &kept_rv).unwrap_or(trailing);
            let row = HarAnchorRow {
                symbol: crate::state_store::canonical_base(symbol),
                intercept: a.intercept,
                b_daily: a.b_daily,
                b_weekly: a.b_weekly,
                b_monthly: a.b_monthly,
                r2: a.r2,
                n_days: a.n_days as i64,
                forecast_rv_bps: forecast,
                trailing_mean_rv_bps: trailing,
                updated_ts: now_ms,
            };
            state.state_store.phase2_upsert_har_anchor(&row).is_ok()
        }
        None => false,
    };
    let k = kept_rv.len() * 7 / 10;
    let (fit_slice, val_slice) = if k >= 1 && kept_rv.len() - k >= 1 {
        (&kept_rv[..k], &kept_rv[k..])
    } else {
        return (anchor_written, PromotionStatus::Held);
    };
    let windows = walk_forward_windows(now_ms, FIT_DAYS, VAL_DAYS);
    if windows.is_none() {
        let _ = kept_ts;
        return (anchor_written, PromotionStatus::Held);
    }
    let fit_half = fit_slice.len() / 2;
    let (Some(fit_early), Some(fit_late), Some(val_med)) = (
        median_of(&fit_slice[..fit_half.max(1).min(fit_slice.len())]),
        median_of(&fit_slice[fit_half.min(fit_slice.len().saturating_sub(1))..]),
        median_of(val_slice),
    ) else {
        return (anchor_written, PromotionStatus::Held);
    };
    let fit_med = median_of(fit_slice).unwrap_or(fit_late);
    let fit_delta = fit_late - fit_early;
    let val_delta = val_med - fit_med;
    let decision = decide_promotion(fit_delta, val_delta);
    let row = PromotionStateRow {
        symbol: crate::state_store::canonical_base(symbol),
        status: decision.status.as_str().to_string(),
        fit_delta_bps: decision.fit_delta_bps,
        val_delta_bps: decision.val_delta_bps,
        reason: decision.reason.clone(),
        updated_ts: now_ms,
    };
    if state
        .state_store
        .phase2_upsert_promotion_state(&row)
        .is_err()
    {
        return (anchor_written, PromotionStatus::Held);
    }
    (anchor_written, decision.status)
}

pub fn run_seasonality_promotion(
    state: &AppEngineState,
    now_ms: i64,
) -> Result<SeasonalityRunSummary, String> {
    if now_ms <= 0 {
        return Err("invalid clock: refusing scheduled promotion on non-positive time".to_string());
    }
    let symbols = state
        .state_store
        .distinct_sample_symbols()
        .map_err(|e| e.to_string())?;
    let stress_json = state
        .state_store
        .kv_get(STRESS_SPANS_KV)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    let mut out = SeasonalityRunSummary {
        symbols_scanned: 0,
        anchors_written: 0,
        promoted: 0,
        demoted: 0,
        held: 0,
        skipped: 0,
        run_ts_ms: now_ms,
    };
    for symbol in symbols.iter().take(MAX_SYMBOLS_PER_RUN) {
        out.symbols_scanned += 1;
        let (anchor_ok, status) = run_one_symbol(state, symbol, now_ms, &stress_json);
        if anchor_ok {
            out.anchors_written += 1;
        }
        match status {
            PromotionStatus::Promoted => out.promoted += 1,
            PromotionStatus::Demoted => out.demoted += 1,
            PromotionStatus::Held => {
                let has_anchor = state
                    .state_store
                    .phase2_har_anchor(symbol)
                    .map(|a| a.is_some())
                    .unwrap_or(false);
                let has_promo = state
                    .state_store
                    .phase2_promotion_state(symbol)
                    .map(|p| p.is_some())
                    .unwrap_or(false);
                if has_anchor || has_promo {
                    out.held += 1;
                } else {
                    out.skipped += 1;
                }
            }
        }
    }
    state
        .state_store
        .kv_set(LAST_PROMOTION_KV, &now_ms.to_string())
        .map_err(|e| e.to_string())?;
    Ok(out)
}

pub fn session_coupling_for(state: &AppEngineState, symbol: &str, now_ms: i64) -> SessionCoupling {
    let neutral = SessionCoupling {
        tau_scale: 1.0,
        sigma_scale: 1.0,
    };
    if now_ms <= 0 {
        return neutral;
    }
    let profiles = match state.state_store.session_profiles(symbol) {
        Ok(p) => p,
        Err(_) => return neutral,
    };
    let meds: Vec<f64> = profiles
        .iter()
        .filter(|r| !r.provisional)
        .filter_map(|r| r.med_abs_spread_bps)
        .filter(|m| m.is_finite() && *m > 0.0)
        .collect();
    let rvs: Vec<f64> = profiles
        .iter()
        .filter(|r| !r.provisional)
        .filter_map(|r| r.rv_bps)
        .filter(|v| v.is_finite() && *v > 0.0)
        .collect();
    let (Some(global_med), Some(global_rv)) = (
        winsorized_median(&meds).filter(|g| g.is_finite() && *g > 0.0),
        winsorized_median(&rvs).filter(|g| g.is_finite() && *g > 0.0),
    ) else {
        return neutral;
    };
    let session = session_of(now_ms);
    let row = profiles.iter().find(|r| r.session_id == session.as_str());
    match row {
        Some(r) if !r.provisional => coupling_inputs(
            r.med_abs_spread_bps,
            Some(global_med),
            r.rv_bps,
            Some(global_rv),
        ),
        _ => neutral,
    }
}

pub fn promotion_active(
    promotion: Option<&PromotionStateRow>,
    anchor: Option<&HarAnchorRow>,
    now_ms: i64,
) -> bool {
    if now_ms <= 0 {
        return false;
    }
    let (Some(p), Some(a)) = (promotion, anchor) else {
        return false;
    };
    if p.status != PromotionStatus::Promoted.as_str() {
        return false;
    }
    if a.updated_ts > now_ms || p.updated_ts > now_ms {
        return false;
    }
    if now_ms - a.updated_ts > PROMOTION_FRESH_MS {
        return false;
    }
    if !(a.forecast_rv_bps.is_finite()
        && a.trailing_mean_rv_bps.is_finite()
        && a.forecast_rv_bps > 0.0
        && a.trailing_mean_rv_bps > 0.0)
    {
        return false;
    }
    let f = rescale_factor(a.forecast_rv_bps, a.trailing_mean_rv_bps);
    f.is_finite() && (f - 1.0).abs() > f64::EPSILON
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state_store::StateStore;
    use crate::state_store::VenueSampleEntry;

    fn sample(venue: &str, mid: f64) -> VenueSampleEntry {
        VenueSampleEntry {
            venue_id: venue.to_string(),
            mid,
            best_bid: mid - 0.1,
            best_ask: mid + 0.1,
        }
    }

    fn seed_two_venue_days(store: &StateStore, symbol: &str, days: i64, per_day: usize) {
        let now = 90 * DAY_MS;
        for d in 0..days {
            let day_start = (now - days * DAY_MS) + d * DAY_MS;
            for i in 0..per_day {
                let ts = day_start + (i as i64) * (DAY_MS / per_day as i64);
                let spike = if i == d as usize % per_day {
                    0.5 + (d % 6) as f64 * 0.7
                } else {
                    0.0
                };
                let noise = (((i * 31 + d as usize * 17) % 13) as f64 - 6.0)
                    * 0.03
                    * (1.0 + (d % 5) as f64 * 0.4);
                let wave = ((i as f64) * 0.7).sin() * 2.0 + spike + noise;
                store
                    .record_spread_samples(
                        symbol,
                        ts,
                        &[
                            sample("hyperliquid", 60_000.0 + wave),
                            sample("lighter", 60_000.0 - wave),
                        ],
                    )
                    .unwrap();
            }
        }
    }

    #[test]
    fn promotion_run_writes_anchor_and_state() {
        let store = StateStore::open_in_memory().unwrap();
        let state = AppEngineState::new_inner_for_test(std::sync::Arc::new(store));
        let now = 90 * DAY_MS;
        seed_two_venue_days(&state.state_store, "BTC-USDC", 60, 48);
        let summary = run_seasonality_promotion(&state, now).unwrap();
        assert_eq!(summary.symbols_scanned, 1);
        assert_eq!(summary.anchors_written, 1);
        assert!(state
            .state_store
            .phase2_har_anchor("BTC-USDC")
            .unwrap()
            .is_some());
        assert!(state
            .state_store
            .phase2_promotion_state("BTC-USDC")
            .unwrap()
            .is_some());
        assert_eq!(
            state.state_store.kv_get(LAST_PROMOTION_KV).unwrap(),
            Some(now.to_string())
        );
    }

    #[test]
    fn promotion_run_skips_empty_history_without_writes() {
        let store = StateStore::open_in_memory().unwrap();
        let state = AppEngineState::new_inner_for_test(std::sync::Arc::new(store));
        let now = 90 * DAY_MS;
        let summary = run_seasonality_promotion(&state, now).unwrap();
        assert_eq!(summary.symbols_scanned, 0);
        assert_eq!(summary.skipped, 0);
        assert_eq!(summary.anchors_written, 0);
    }

    #[test]
    fn promotion_run_rejects_broken_clock() {
        let store = StateStore::open_in_memory().unwrap();
        let state = AppEngineState::new_inner_for_test(std::sync::Arc::new(store));
        assert!(run_seasonality_promotion(&state, 0).is_err());
        assert!(run_seasonality_promotion(&state, -100).is_err());
    }

    #[test]
    fn promotion_run_excludes_stress_weeks() {
        let store = StateStore::open_in_memory().unwrap();
        let state = AppEngineState::new_inner_for_test(std::sync::Arc::new(store));
        let now = 90 * DAY_MS;
        seed_two_venue_days(&state.state_store, "ETH-USDC", 60, 48);
        let span_start = now - 60 * DAY_MS;
        let span_end = now - 20 * DAY_MS;
        state
            .state_store
            .kv_set(
                STRESS_SPANS_KV,
                &format!(r#"[{{"start_ms": {span_start}, "end_ms": {span_end}}}]"#,),
            )
            .unwrap();
        let summary = run_seasonality_promotion(&state, now).unwrap();
        assert_eq!(summary.symbols_scanned, 1);
        assert_eq!(
            summary.anchors_written, 0,
            "40 stressed days excluded, <30 remain"
        );
        assert_eq!(summary.skipped, 1);
    }

    #[test]
    fn coupling_neutral_without_profiles() {
        let store = StateStore::open_in_memory().unwrap();
        let state = AppEngineState::new_inner_for_test(std::sync::Arc::new(store));
        let c = session_coupling_for(&state, "BTC-USDC", 90 * DAY_MS);
        assert_eq!(c.tau_scale, 1.0);
        assert_eq!(c.sigma_scale, 1.0);
        assert_eq!(session_coupling_for(&state, "BTC-USDC", 0).tau_scale, 1.0);
    }

    #[test]
    fn promotion_active_gates_display() {
        let promo = PromotionStateRow {
            symbol: "BTC".to_string(),
            status: "promoted".to_string(),
            fit_delta_bps: 2.0,
            val_delta_bps: 1.5,
            reason: "agree".to_string(),
            updated_ts: 100,
        };
        let anchor = HarAnchorRow {
            symbol: "BTC".to_string(),
            intercept: 0.1,
            b_daily: 0.5,
            b_weekly: 0.3,
            b_monthly: 0.1,
            r2: 0.9,
            n_days: 60,
            forecast_rv_bps: 12.0,
            trailing_mean_rv_bps: 10.0,
            updated_ts: 100,
        };
        let now = 100 + 3_600_000;
        assert!(promotion_active(Some(&promo), Some(&anchor), now));
        assert!(!promotion_active(None, Some(&anchor), now));
        assert!(!promotion_active(Some(&promo), None, now));
        let mut held = promo.clone();
        held.status = "held".to_string();
        assert!(!promotion_active(Some(&held), Some(&anchor), now));
        assert!(!promotion_active(Some(&promo), Some(&anchor), 50));
        assert!(!promotion_active(Some(&promo), Some(&anchor), 0));
        assert!(!promotion_active(
            Some(&promo),
            Some(&anchor),
            100 + PROMOTION_FRESH_MS + 1
        ));
    }
}
