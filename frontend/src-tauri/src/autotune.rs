//! AutoTune for Simple Spread futures: walk-forward parameter
//! recommendations, operator/scheduled apply with audit trail, and
//! per-ticker tuning modes (manual / periodic / continuous).
//!
//! Split out of `commands.rs`; the helpers it needs stay reachable through
//! `crate::commands` re-exports below.

use crate::commands::{
    cached_venue_volumes, effective_venue_fee_bps, validate_market_symbol, BASELINE_RETENTION_MS,
};
use crate::monitor::PAIR_ALIGN_GAP_MS;
use crate::state::{AppEngineState, StrategyParamsDto, TickerStrategyConfigDto};
use crate::strategy_config::{
    canonical_sliding_window, get_strategy_params_impl, get_ticker_strategy_config_impl,
    save_strategy_params_impl, save_ticker_strategy_config_impl,
};
use arbitrageur_math::autotune::{recommend, TuneHistory, TuneParams};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::State;

// ---------------------------------------------------------------------------
// Auto-tune Phase 1 (recommend-only) — Simple Spread futures
// ---------------------------------------------------------------------------

/// One validated recommendation row. `recommended` is `None` exactly when
/// `status == "HELD"` — the manual value stays authoritative. Read-only:
/// no write path consumes this DTO.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AutotuneRowDto {
    pub param: String,
    pub current: f64,
    pub recommended: Option<f64>,
    pub status: String,
    pub fit_metric: String,
    pub validated_metric: String,
    pub n: usize,
    pub rationale: String,
    pub formula_id: String,
    pub confidence: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AutotuneBannerDto {
    pub fit_from_ms: i64,
    pub fit_to_ms: i64,
    pub validate_from_ms: i64,
    pub validate_to_ms: i64,
    pub regime_verdict: String,
    pub ready_count: usize,
    pub total: usize,
}

/// Structured evidence inventory behind a recommendation — why rows are
/// READY or HELD at the data level, not just per-param rationales.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AutotuneReadinessDto {
    /// Venues that contributed at least one sample in the window.
    pub venues_seen: Vec<String>,
    /// Earliest→latest sample span, seconds.
    pub history_span_secs: f64,
    /// Fraction of sampled timestamps observed on ≥2 distinct venues.
    pub multi_venue_coverage: f64,
    /// Aligned observations on the selected (densest) venue pair.
    pub pair_observations: usize,
    /// Summed aligned observations across every ordered venue pair.
    pub total_pair_observations: usize,
    /// Sample floor below which every row stays HELD.
    pub min_observations_required: usize,
    /// Whether the selected pair clears the floor.
    pub obs_floor_met: bool,
    /// Measured direction-aware slippage (qty-weighted median bps) summed
    /// across the selected venues' recent filled legs — the fill-feedback
    /// component added to the static fee estimate. `None` until at least
    /// one venue records ≥5 legs carrying both intended and fill prices
    /// (schema v27+). Signed: negative means fills beat the dispatch quote.
    pub measured_slippage_bps: Option<f64>,
    /// Human-readable data blockers (empty when nothing blocks).
    pub blockers: Vec<String>,
}

/// Recommend-only preview for one symbol's Simple Spread futures params.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AutotuneRecommendationDto {
    pub symbol: String,
    pub buy_venue: Option<String>,
    pub sell_venue: Option<String>,
    pub pair_observations: usize,
    pub rows: Vec<AutotuneRowDto>,
    pub banner: AutotuneBannerDto,
    pub session_tau_scale: f64,
    pub session_sigma_scale: f64,
    pub readiness: AutotuneReadinessDto,
}

fn tune_params_from_dto(p: &StrategyParamsDto) -> TuneParams {
    TuneParams {
        window_secs: p.sliding_window_secs,
        target_bps: p.target_profit_bps,
        percentile: p.window_percentile,
        step_mult: p.step_size_mult,
        max_step_usd: p.max_step_usd,
        equity_pct: p.max_ticker_equity_pct,
        leverage: p.user_target_leverage,
    }
}

fn map_autotune_report(
    symbol: &str,
    buy_venue: Option<String>,
    sell_venue: Option<String>,
    pair_observations: usize,
    coupling: arbitrageur_math::seasonality::SessionCoupling,
    rep: arbitrageur_math::autotune::AutotuneReport,
    readiness: AutotuneReadinessDto,
) -> AutotuneRecommendationDto {
    AutotuneRecommendationDto {
        symbol: symbol.to_string(),
        buy_venue,
        sell_venue,
        pair_observations,
        rows: rep
            .rows
            .into_iter()
            .map(|r| AutotuneRowDto {
                param: r.param,
                current: r.current,
                recommended: r.recommended,
                status: match r.status {
                    arbitrageur_math::autotune::RowStatus::READY => "READY".to_string(),
                    arbitrageur_math::autotune::RowStatus::HELD => "HELD".to_string(),
                },
                fit_metric: r.fit_metric,
                validated_metric: r.validated_metric,
                n: r.n,
                rationale: r.rationale,
                formula_id: r.formula_id,
                confidence: r.confidence,
            })
            .collect(),
        banner: AutotuneBannerDto {
            fit_from_ms: rep.banner.fit_from_ms,
            fit_to_ms: rep.banner.fit_to_ms,
            validate_from_ms: rep.banner.validate_from_ms,
            validate_to_ms: rep.banner.validate_to_ms,
            regime_verdict: rep.banner.regime_verdict.as_str().to_string(),
            ready_count: rep.banner.ready_count,
            total: rep.banner.total,
        },
        session_tau_scale: coupling.tau_scale,
        session_sigma_scale: coupling.sigma_scale,
        readiness,
    }
}

/// Capacity tiers backing the step/maxStep candidates: the monitor's
/// default tiers plus a 0.5% participation-cap tier off the smaller venue's
/// 24h volume when both volumes are known.
fn autotune_tiers_with_cap(
    volumes: &HashMap<String, HashMap<String, f64>>,
    buy_v: &str,
    sell_v: &str,
    base: &str,
) -> Vec<f64> {
    let mut tiers = vec![1000.0, 5000.0, 10_000.0];
    // Venue volume maps are keyed by canonical BASE-QUOTE symbols; reuse the
    // shared quote-stripping matcher so every venue spelling resolves.
    let lookup = |venue: &str| -> Option<f64> {
        let m = volumes.get(venue)?;
        crate::strategy_filters::venue_asset_volume_usd(m, base)
    };
    if let (Some(a), Some(b)) = (lookup(buy_v), lookup(sell_v)) {
        let cap = a.min(b) * 0.005;
        if cap.is_finite() && cap > 0.0 {
            tiers.push(cap);
            tiers.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
            tiers.dedup();
        }
    }
    tiers
}

pub async fn get_autotune_recommendation_impl(
    symbol: &str,
    state: &AppEngineState,
) -> Result<AutotuneRecommendationDto, String> {
    validate_market_symbol(symbol)?;
    let current_dto = get_strategy_params_impl("futures", state)?;
    let current = tune_params_from_dto(&current_dto);
    let defaults = tune_params_from_dto(&StrategyParamsDto::defaults_for("futures"));

    let base = symbol
        .split(['-', '/', '_'])
        .next()
        .unwrap_or(symbol)
        .to_ascii_uppercase();
    // Quote policy: merged modes read the historical alias spellings (the
    // same series regardless of which quote it was sampled under);
    // `separate` mode keeps the requested pair quote-strict — BTC-USDT
    // evidence never mixes with BTC-USDC.
    let merge_usd = state.usd_merge_quotes();
    let keys: Vec<String> = if merge_usd {
        vec![base.clone(), format!("{base}-USDC"), format!("{base}-USD")]
    } else {
        vec![symbol.trim().to_uppercase()]
    };
    let now_ms = Utc::now().timestamp_millis();
    let since_ms = now_ms - BASELINE_RETENTION_MS;
    let store = state.state_store.clone();
    let sym_owned = symbol.to_string();
    let env = state.env().as_str();
    struct Evidence {
        best_pair: Option<(
            String,
            String,
            Vec<arbitrageur_math::pair_monitor::PairObservation>,
        )>,
        total_pair_obs: usize,
        venues_seen: Vec<String>,
        history_span_secs: f64,
        multi_venue_coverage: f64,
    }
    let evidence = tokio::task::spawn_blocking(move || {
        let mut all = Vec::new();
        for k in &keys {
            if let Ok(mut rows) = store.spread_samples_mode(k, merge_usd, since_ms, now_ms, env) {
                all.append(&mut rows);
            }
        }
        all.sort_by(|a, b| (a.ts_ms, a.venue_id.as_str()).cmp(&(b.ts_ms, b.venue_id.as_str())));
        all.dedup_by(|a, b| a.ts_ms == b.ts_ms && a.venue_id == b.venue_id);
        let mut mids_by_venue: HashMap<String, Vec<(i64, f64)>> = HashMap::new();
        for s in &all {
            if s.mid.is_finite() && s.mid > 0.0 {
                mids_by_venue
                    .entry(s.venue_id.clone())
                    .or_default()
                    .push((s.ts_ms, s.mid));
            }
        }
        let mut venues: Vec<String> = mids_by_venue.keys().cloned().collect();
        venues.sort();

        // Evidence stats: sample span and the share of timestamp buckets that
        // actually observed ≥2 distinct venues (coverage of the cross-venue
        // edge series, not just raw row count).
        let span_secs = match (all.first(), all.last()) {
            (Some(f), Some(l)) => (l.ts_ms - f.ts_ms) as f64 / 1000.0,
            _ => 0.0,
        };
        let mut buckets: HashMap<i64, std::collections::HashSet<&str>> = HashMap::new();
        for s in &all {
            buckets
                .entry(s.ts_ms)
                .or_default()
                .insert(s.venue_id.as_str());
        }
        let coverage = if buckets.is_empty() {
            0.0
        } else {
            buckets.values().filter(|set| set.len() >= 2).count() as f64 / buckets.len() as f64
        };
        let mut best: Option<(
            String,
            String,
            Vec<arbitrageur_math::pair_monitor::PairObservation>,
        )> = None;
        let mut total = 0usize;
        for buy_v in &venues {
            for sell_v in &venues {
                if buy_v == sell_v {
                    continue;
                }
                let (Some(a), Some(b)) = (mids_by_venue.get(buy_v), mids_by_venue.get(sell_v))
                else {
                    continue;
                };
                let series =
                    arbitrageur_math::pair_monitor::build_pair_series(a, b, PAIR_ALIGN_GAP_MS);
                total += series.len();
                let longer = best.as_ref().map(|(_, _, s)| s.len()).unwrap_or(0);
                if series.len() > longer {
                    best = Some((buy_v.clone(), sell_v.clone(), series));
                }
            }
        }
        let _ = sym_owned;
        Ok::<_, anyhow::Error>(Evidence {
            best_pair: best,
            total_pair_obs: total,
            venues_seen: venues,
            history_span_secs: span_secs,
            multi_venue_coverage: coverage,
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    let Evidence {
        best_pair,
        total_pair_obs: pair_observations,
        venues_seen,
        history_span_secs,
        multi_venue_coverage,
    } = evidence;

    // Structured data-readiness: which evidence gates are met and which
    // still block tuning — surfaced instead of only row-level HELD text.
    let best_pair_n = best_pair.as_ref().map(|(_, _, s)| s.len()).unwrap_or(0);
    let mut blockers: Vec<String> = Vec::new();
    if venues_seen.len() < 2 {
        blockers.push(format!(
            "only {} venue(s) reported samples — cross-venue pairs need ≥2",
            venues_seen.len()
        ));
    }
    if best_pair_n < arbitrageur_math::autotune::AUTOTUNE_MIN_PAIR_OBS {
        blockers.push(format!(
            "densest venue pair has {best_pair_n} aligned observations — need ≥{}",
            arbitrageur_math::autotune::AUTOTUNE_MIN_PAIR_OBS
        ));
    }
    if history_span_secs < 3600.0 {
        blockers.push(format!(
            "history spans {:.0}m — thin regime coverage",
            history_span_secs / 60.0
        ));
    }
    if multi_venue_coverage < 0.5 {
        blockers.push(format!(
            "{:.0}% of timestamps observed on ≥2 venues — edge series is mostly single-venue",
            multi_venue_coverage * 100.0
        ));
    }
    let mut readiness = AutotuneReadinessDto {
        venues_seen,
        history_span_secs,
        multi_venue_coverage,
        pair_observations: best_pair_n,
        total_pair_observations: pair_observations,
        min_observations_required: arbitrageur_math::autotune::AUTOTUNE_MIN_PAIR_OBS,
        obs_floor_met: best_pair_n >= arbitrageur_math::autotune::AUTOTUNE_MIN_PAIR_OBS,
        measured_slippage_bps: None,
        blockers,
    };

    let Some((buy_v, sell_v, series)) = best_pair else {
        // No paired history — honest all-HELD report, never fabricated.
        let rep = recommend(&TuneHistory {
            fit: Vec::new(),
            validate: Vec::new(),
            dt_fit_secs: 0.0,
            dt_val_secs: 0.0,
            friction_fit_bps: 0.0,
            friction_val_bps: 0.0,
            carry_known: false,
            fit_optimal_size_usd: None,
            fit_breakeven_size_usd: None,
            current,
            defaults,
        });
        let neutral = arbitrageur_math::seasonality::SessionCoupling {
            tau_scale: 1.0,
            sigma_scale: 1.0,
        };
        return Ok(map_autotune_report(
            symbol, None, None, 0, neutral, rep, readiness,
        ));
    };

    // 70/30 chronological walk-forward split.
    let k = series.len() * 7 / 10;
    let (fit, validate) = if k >= 1 && series.len() - k >= 1 {
        (series[..k].to_vec(), series[k..].to_vec())
    } else {
        (Vec::new(), Vec::new())
    };
    let seg_dt = |obs: &[arbitrageur_math::pair_monitor::PairObservation]| -> f64 {
        if obs.len() >= 2 {
            let span = (obs[obs.len() - 1].ts_ms - obs[0].ts_ms) as f64 / 1000.0;
            let dt = span / (obs.len() - 1) as f64;
            if dt.is_finite() && dt > 0.0 {
                return dt;
            }
        }
        0.0
    };
    let dt_fit = seg_dt(&fit);
    let dt_val = seg_dt(&validate);

    // Static friction: both taker fees. Funding carry has no measured EMA
    // on this path, so the carry term is 0 and flagged unknown.
    let fee_sum = effective_venue_fee_bps(state, &buy_v) + effective_venue_fee_bps(state, &sell_v);

    // Fill-feedback join: measured direction-aware slippage from this
    // venue's closed legs augments the static fee estimate once ≥5 fills
    // exist — signed (negative = fills beat the quote), never fabricated.
    let measured_slip_bps = |venue: &str| -> Option<f64> {
        let samples = state
            .state_store
            .leg_slippage_samples_bps(venue, 200)
            .ok()?;
        if samples.len() < 5 {
            return None;
        }
        arbitrageur_math::binomial::weighted_quantile(&samples, 0.5)
    };
    let slip_parts: Vec<f64> = [buy_v.as_str(), sell_v.as_str()]
        .iter()
        .filter_map(|v| measured_slip_bps(v))
        .collect();
    let measured_slippage_bps = (!slip_parts.is_empty()).then(|| slip_parts.iter().sum::<f64>());
    readiness.measured_slippage_bps = measured_slippage_bps;
    let realized_slippage_bps = measured_slippage_bps.unwrap_or(0.0);

    // Seasonality Phase 2 coupling: the current session's shrunk τ/σ regime
    // scales the recommender inputs (OU dt and friction). Missing or
    // provisional profiles couple at exactly 1.0 — Phase-1 behavior.
    let coupling = crate::seasonality::session_coupling_for(state, symbol, now_ms);

    // Fit capacity summary from live-book ladder (0.5% participation cap).
    // Hub cache only — a recommend preview must not trigger network fetches;
    // absent books mean step/maxStep stay HELD.
    let (fit_optimal, fit_breakeven) = {
        let hub = &state.market_data_hub;
        let stale = std::time::Duration::from_secs(300);
        let buy_book = hub.get_valid_orderbook(
            &arbitrageur_core::types::VenueId::from(buy_v.as_str()),
            symbol,
            Some(stale),
        );
        let sell_book = hub.get_valid_orderbook(
            &arbitrageur_core::types::VenueId::from(sell_v.as_str()),
            symbol,
            Some(stale),
        );
        match (buy_book, sell_book) {
            (Some(bk), Some(sk)) => {
                let asks: Vec<(f64, f64)> = bk
                    .asks
                    .iter()
                    .filter_map(|l| {
                        let pf = l.price.to_string().parse::<f64>().ok()?;
                        let sf = l.size.to_string().parse::<f64>().ok()?;
                        (pf.is_finite() && sf.is_finite() && pf > 0.0 && sf > 0.0)
                            .then_some((pf, sf))
                    })
                    .collect();
                let bids: Vec<(f64, f64)> = sk
                    .bids
                    .iter()
                    .filter_map(|l| {
                        let pf = l.price.to_string().parse::<f64>().ok()?;
                        let sf = l.size.to_string().parse::<f64>().ok()?;
                        (pf.is_finite() && sf.is_finite() && pf > 0.0 && sf > 0.0)
                            .then_some((pf, sf))
                    })
                    .collect();
                let fit_vals: Vec<f64> = fit.iter().map(|o| o.spread_bps).collect();
                let n = fit_vals.len();
                let (mu_f, sig_f) = if n >= 2 {
                    let m = fit_vals.iter().sum::<f64>() / n as f64;
                    let v = fit_vals.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (n - 1) as f64;
                    (m, v.sqrt())
                } else {
                    (0.0, 0.0)
                };
                if !(mu_f.is_finite() && sig_f.is_finite() && sig_f > 0.0)
                    || asks.is_empty()
                    || bids.is_empty()
                {
                    (None, None)
                } else {
                    let volumes = cached_venue_volumes(state).await;
                    let tiers = autotune_tiers_with_cap(&volumes, &buy_v, &sell_v, &base);
                    let curve = arbitrageur_math::pair_monitor::build_pwin_curve(
                        &fit,
                        mu_f,
                        sig_f,
                        fee_sum.max(2.0),
                        3.0,
                        None,
                        chrono::Utc::now().timestamp_millis(),
                    );
                    let rep = arbitrageur_math::pair_monitor::capacity_ladder(
                        &asks, &bids, mu_f, fee_sum, &tiers, &curve, 0.9, None, 2.0,
                    );
                    (rep.optimal_size_usd, rep.breakeven_size_usd)
                }
            }
            _ => (None, None),
        }
    };

    let rep = recommend(&TuneHistory {
        fit,
        validate,
        dt_fit_secs: dt_fit * coupling.tau_scale,
        dt_val_secs: dt_val * coupling.tau_scale,
        friction_fit_bps: (fee_sum + realized_slippage_bps) * coupling.sigma_scale,
        friction_val_bps: (fee_sum + realized_slippage_bps) * coupling.sigma_scale,
        carry_known: false,
        fit_optimal_size_usd: fit_optimal,
        fit_breakeven_size_usd: fit_breakeven,
        current,
        defaults,
    });
    Ok(map_autotune_report(
        symbol,
        Some(buy_v),
        Some(sell_v),
        pair_observations,
        coupling,
        rep,
        readiness,
    ))
}

#[tauri::command]
pub async fn get_autotune_recommendation(
    symbol: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<AutotuneRecommendationDto, String> {
    get_autotune_recommendation_impl(&symbol, &state).await
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AutotuneApplyResultDto {
    pub symbol: String,
    pub applied: Vec<String>,
    pub skipped_held: Vec<String>,
    pub audit_id: Option<i64>,
}

const AUTOTUNE_APPLYABLE: [&str; 7] = [
    "slidingWindowSecs",
    "targetProfitBps",
    "windowPercentile",
    "stepSizeMult",
    "maxStepUsd",
    "maxTickerEquityPct",
    "userTargetLeverage",
];

fn apply_ready_to_strategy(
    base: &StrategyParamsDto,
    param: &str,
    value: f64,
) -> Result<StrategyParamsDto, String> {
    let mut next = base.clone();
    match param {
        "slidingWindowSecs" => {
            if !(1.0..=3_000_000.0).contains(&value) {
                return Err(format!("rejecting out-of-range window {value}"));
            }
            next.sliding_window_secs = value.round() as u64;
        }
        "targetProfitBps" => next.target_profit_bps = value,
        "windowPercentile" => {
            if !(0.0..=100.0).contains(&value) {
                return Err(format!("rejecting out-of-range percentile {value}"));
            }
            next.window_percentile = value.round() as u32;
        }
        "stepSizeMult" => next.step_size_mult = value,
        "maxStepUsd" => next.max_step_usd = value,
        "maxTickerEquityPct" => next.max_ticker_equity_pct = value,
        "userTargetLeverage" => {
            if !(0.0..=10.0).contains(&value) {
                return Err(format!("rejecting out-of-range leverage {value}"));
            }
            next.user_target_leverage = value.round() as u32;
        }
        _ => return Err(format!("unknown autotune param '{param}'")),
    }
    Ok(next)
}

fn apply_ready_to_ticker(
    base: &TickerStrategyConfigDto,
    param: &str,
    value: f64,
    window_secs: u64,
) -> TickerStrategyConfigDto {
    let mut next = base.clone();
    match param {
        "slidingWindowSecs" => next.sliding_window = canonical_sliding_window(window_secs),
        "targetProfitBps" => next.target_profit_bps = value,
        "windowPercentile" => next.window_percentile = value.round() as u32,
        "userTargetLeverage" => next.user_target_leverage = value.round() as u32,
        _ => {}
    }
    next
}

pub async fn apply_autotune_rows_inner(
    state: &AppEngineState,
    symbol: &str,
    report: &AutotuneRecommendationDto,
    selected: &[String],
    now_ms: i64,
    source: &str,
) -> Result<AutotuneApplyResultDto, String> {
    validate_market_symbol(symbol)?;
    if now_ms <= 0 {
        return Err("invalid clock: refusing apply on non-positive time".to_string());
    }
    let mut wanted: Vec<String> = selected.to_vec();
    wanted.sort();
    wanted.dedup();
    for p in &wanted {
        if !AUTOTUNE_APPLYABLE.contains(&p.as_str()) {
            return Err(format!("unknown autotune param '{p}'"));
        }
    }
    let ready: std::collections::HashMap<&str, f64> = report
        .rows
        .iter()
        .filter(|r| r.status == "READY")
        .filter_map(|r| match r.recommended {
            Some(v) if v.is_finite() => Some((r.param.as_str(), v)),
            _ => None,
        })
        .collect();
    let mut to_apply: Vec<(&str, f64)> = Vec::new();
    let mut skipped_held: Vec<String> = Vec::new();
    for p in &wanted {
        match ready.get(p.as_str()) {
            Some(v) => to_apply.push((p.as_str(), *v)),
            None => skipped_held.push(p.clone()),
        }
    }
    skipped_held.sort();
    if to_apply.is_empty() {
        return Ok(AutotuneApplyResultDto {
            symbol: symbol.to_string(),
            applied: Vec::new(),
            skipped_held,
            audit_id: None,
        });
    }
    let _guard = state.autotune_apply_lock.lock().await;
    let prev_strategy = get_strategy_params_impl("futures", state)?;
    let prev_ticker = get_ticker_strategy_config_impl(symbol, state)?;
    let mut next_strategy = prev_strategy.clone();
    let mut next_ticker = prev_ticker.clone();
    for (param, value) in &to_apply {
        next_strategy = apply_ready_to_strategy(&next_strategy, param, *value)?;
    }
    let window_secs = next_strategy.sliding_window_secs;
    for (param, value) in &to_apply {
        next_ticker = apply_ready_to_ticker(&next_ticker, param, *value, window_secs);
    }
    save_strategy_params_impl(next_strategy, state)?;
    if let Err(e) = save_ticker_strategy_config_impl(next_ticker, state) {
        let _ = save_strategy_params_impl(prev_strategy.clone(), state);
        return Err(e);
    }
    let applied_names: Vec<String> = to_apply.iter().map(|(p, _)| p.to_string()).collect();
    let applied_map: serde_json::Map<String, serde_json::Value> = to_apply
        .iter()
        .map(|(p, v)| {
            (
                p.to_string(),
                serde_json::Value::from(
                    serde_json::Number::from_f64(*v).unwrap_or(serde_json::Number::from(0)),
                ),
            )
        })
        .collect();
    let applied_json = serde_json::Value::Object(applied_map).to_string();
    let detail_rows: Vec<serde_json::Value> = report
        .rows
        .iter()
        .filter(|r| applied_names.iter().any(|p| p == &r.param))
        .map(|r| {
            serde_json::json!({
                "param": r.param,
                "recommended": r.recommended,
                "fit_metric": r.fit_metric,
                "validated_metric": r.validated_metric,
                "n": r.n,
                "formula_id": r.formula_id,
                "confidence": r.confidence,
            })
        })
        .collect();
    let detail_json = serde_json::json!({
        "source": source,
        "buy_venue": report.buy_venue,
        "sell_venue": report.sell_venue,
        "fit_from_ms": report.banner.fit_from_ms,
        "fit_to_ms": report.banner.fit_to_ms,
        "validate_from_ms": report.banner.validate_from_ms,
        "validate_to_ms": report.banner.validate_to_ms,
        "regime_verdict": report.banner.regime_verdict,
        "session_tau_scale": report.session_tau_scale,
        "session_sigma_scale": report.session_sigma_scale,
        "rows": detail_rows,
        "skipped_held": skipped_held.clone(),
    })
    .to_string();
    match state.state_store.record_autotune_apply_event_mode(
        symbol,
        &applied_json,
        &detail_json,
        now_ms,
        state.usd_merge_quotes(),
        state.env().as_str(),
    ) {
        Ok(audit_id) => Ok(AutotuneApplyResultDto {
            symbol: symbol.to_string(),
            applied: applied_names,
            skipped_held,
            audit_id: Some(audit_id),
        }),
        Err(e) => {
            let _ = save_strategy_params_impl(prev_strategy, state);
            let _ = save_ticker_strategy_config_impl(prev_ticker, state);
            Err(format!("apply audit failed, changes rolled back: {e}"))
        }
    }
}

pub async fn apply_autotune_selected_impl(
    symbol: &str,
    selected: &[String],
    state: &AppEngineState,
) -> Result<AutotuneApplyResultDto, String> {
    let report = get_autotune_recommendation_impl(symbol, state).await?;
    let now_ms = Utc::now().timestamp_millis();
    apply_autotune_rows_inner(state, symbol, &report, selected, now_ms, "manual").await
}

#[tauri::command]
pub async fn apply_autotune_selected(
    symbol: String,
    selected: Vec<String>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<AutotuneApplyResultDto, String> {
    apply_autotune_selected_impl(&symbol, &selected, &state).await
}

// ---------------------------------------------------------------------------
// Per-ticker AutoTune modes + scheduler.
//
// `autotune.mode.{SYMBOL}` ∈ {manual (default), periodic, continuous}:
// periodic re-tunes every `autotune.period_secs` (default 6h), continuous
// every `autotune.continuous_min_secs` (default 15m). Both auto-apply READY
// rows through the same audited path as the manual apply (detail.source =
// the mode); manual and unknown modes never auto-run. The last-run stamp is
// written BEFORE the pass so a slow or failing run backs off a full
// interval instead of hot-looping.
// ---------------------------------------------------------------------------

const AUTOTUNE_MODE_PREFIX: &str = "autotune.mode.";
const AUTOTUNE_LAST_RUN_PREFIX: &str = "autotune.last_run_ms.";
const AUTOTUNE_PERIOD_SECS_KEY: &str = "autotune.period_secs";
const AUTOTUNE_CONT_MIN_SECS_KEY: &str = "autotune.continuous_min_secs";
const AUTOTUNE_DEFAULT_PERIOD_SECS: i64 = 6 * 3600;
const AUTOTUNE_DEFAULT_CONT_MIN_SECS: i64 = 900;
const AUTOTUNE_MODES: [&str; 3] = ["manual", "periodic", "continuous"];

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TickerAutotuneModeDto {
    pub symbol: String,
    /// manual | periodic | continuous
    pub mode: String,
    /// Last scheduler pass for this symbol, epoch ms (None = never).
    pub last_run_ms: Option<i64>,
    /// Effective cadence for periodic mode, seconds.
    pub period_secs: i64,
    /// Effective minimum cadence for continuous mode, seconds.
    pub continuous_min_secs: i64,
}

fn autotune_i64_kv(state: &AppEngineState, key: &str, default: i64) -> i64 {
    state
        .state_store
        .kv_get(key)
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

fn autotune_mode_dto(symbol: &str, state: &AppEngineState) -> TickerAutotuneModeDto {
    let mode = state
        .state_store
        .kv_get(&format!("{AUTOTUNE_MODE_PREFIX}{symbol}"))
        .ok()
        .flatten()
        .filter(|m| AUTOTUNE_MODES.contains(&m.as_str()))
        .unwrap_or_else(|| "manual".to_string());
    let last_run_ms = state
        .state_store
        .kv_get(&format!("{AUTOTUNE_LAST_RUN_PREFIX}{symbol}"))
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i64>().ok());
    TickerAutotuneModeDto {
        symbol: symbol.to_string(),
        mode,
        last_run_ms,
        period_secs: autotune_i64_kv(
            state,
            AUTOTUNE_PERIOD_SECS_KEY,
            AUTOTUNE_DEFAULT_PERIOD_SECS,
        ),
        continuous_min_secs: autotune_i64_kv(
            state,
            AUTOTUNE_CONT_MIN_SECS_KEY,
            AUTOTUNE_DEFAULT_CONT_MIN_SECS,
        ),
    }
}

pub fn get_ticker_autotune_mode_impl(
    symbol: &str,
    state: &AppEngineState,
) -> Result<TickerAutotuneModeDto, String> {
    validate_market_symbol(symbol)?;
    Ok(autotune_mode_dto(symbol, state))
}

#[tauri::command]
pub fn get_ticker_autotune_mode(
    symbol: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<TickerAutotuneModeDto, String> {
    get_ticker_autotune_mode_impl(&symbol, &state)
}

pub fn set_ticker_autotune_mode_impl(
    symbol: &str,
    mode: &str,
    state: &AppEngineState,
) -> Result<TickerAutotuneModeDto, String> {
    validate_market_symbol(symbol)?;
    let mode = mode.trim().to_ascii_lowercase();
    if !AUTOTUNE_MODES.contains(&mode.as_str()) {
        return Err(format!(
            "unknown autotune mode '{mode}' — expected manual|periodic|continuous"
        ));
    }
    let key = format!("{AUTOTUNE_MODE_PREFIX}{symbol}");
    if mode == "manual" {
        // Default behavior — drop the key so the scheduler's prefix scan
        // stays tight.
        state
            .state_store
            .kv_delete(&key)
            .map_err(|e| e.to_string())?;
    } else {
        state
            .state_store
            .kv_set(&key, &mode)
            .map_err(|e| e.to_string())?;
    }
    Ok(autotune_mode_dto(symbol, state))
}

#[tauri::command]
pub fn set_ticker_autotune_mode(
    symbol: String,
    mode: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<TickerAutotuneModeDto, String> {
    set_ticker_autotune_mode_impl(&symbol, &mode, &state)
}

/// One scheduler pass: tune every due symbol and auto-apply READY rows.
/// Returns the number of symbols evaluated. Symbols whose recommendation
/// errors or produces no READY rows still advance their last-run stamp.
pub async fn run_scheduled_autotune(state: &AppEngineState) -> Result<usize, String> {
    let now_ms = Utc::now().timestamp_millis();
    let period_secs = autotune_i64_kv(
        state,
        AUTOTUNE_PERIOD_SECS_KEY,
        AUTOTUNE_DEFAULT_PERIOD_SECS,
    );
    let cont_secs = autotune_i64_kv(
        state,
        AUTOTUNE_CONT_MIN_SECS_KEY,
        AUTOTUNE_DEFAULT_CONT_MIN_SECS,
    );
    let keys = state
        .state_store
        .kv_keys_with_prefix(AUTOTUNE_MODE_PREFIX)
        .map_err(|e| e.to_string())?;
    let mut ran = 0usize;
    for key in keys {
        let symbol = &key[AUTOTUNE_MODE_PREFIX.len()..];
        let mode = state
            .state_store
            .kv_get(&key)
            .ok()
            .flatten()
            .unwrap_or_default();
        let interval_secs = match mode.as_str() {
            "periodic" => period_secs,
            "continuous" => cont_secs,
            _ => continue,
        };
        let last_key = format!("{AUTOTUNE_LAST_RUN_PREFIX}{symbol}");
        let last = state
            .state_store
            .kv_get(&last_key)
            .ok()
            .flatten()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        if now_ms - last < interval_secs * 1000 {
            continue;
        }
        state
            .state_store
            .kv_set(&last_key, &now_ms.to_string())
            .map_err(|e| e.to_string())?;
        ran += 1;
        match get_autotune_recommendation_impl(symbol, state).await {
            Ok(report) => {
                let ready: Vec<String> = report
                    .rows
                    .iter()
                    .filter(|r| r.status == "READY")
                    .map(|r| r.param.clone())
                    .filter(|p| AUTOTUNE_APPLYABLE.contains(&p.as_str()))
                    .collect();
                if ready.is_empty() {
                    continue;
                }
                if let Err(e) =
                    apply_autotune_rows_inner(state, symbol, &report, &ready, now_ms, &mode).await
                {
                    tracing::warn!("scheduled autotune apply failed for {symbol}: {e}");
                } else {
                    tracing::info!(
                        "scheduled autotune ({mode}) applied {} row(s) for {symbol}",
                        ready.len()
                    );
                }
            }
            Err(e) => tracing::warn!("scheduled autotune recommend failed for {symbol}: {e}"),
        }
    }
    Ok(ran)
}

/// Background supervisor: checks per-ticker modes every 60s and runs due
/// tunes. Inert unless a ticker is explicitly set to periodic/continuous.
pub fn spawn_autotune_scheduler(state: Arc<AppEngineState>) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            if let Err(e) = run_scheduled_autotune(&state).await {
                tracing::warn!("autotune scheduler pass failed: {e}");
            }
        }
    });
}
