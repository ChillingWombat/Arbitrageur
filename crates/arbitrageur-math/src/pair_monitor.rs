//! Pair-spread monitor estimators.
//!
//! Pure functions powering the Spread Range Monitor: aligned pair-spread
//! series construction, empirical reversion statistics (episode
//! segmentation + conditional win-rate curve), the probability-weighted
//! capacity ladder over both orderbooks, and conservative funding carry.
//!
//! Every estimator is fail-closed: below its data floor it returns `None`
//! and callers must surface "insufficient data" rather than a number.

/// One aligned observation of a directional pair spread, in basis points:
/// `s_t = (mid_sell - mid_buy) / mid_buy * 1e4` — positive when selling on
/// venue B and buying on venue A captures a premium.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PairObservation {
    pub ts_ms: i64,
    pub spread_bps: f64,
}

/// Build a time-aligned directional spread series from two venues' mid
/// samples. Samples must be sorted ascending; a pair is emitted whenever a
/// sample from each venue falls within `max_gap_ms` (nearest match, each
/// sample consumed at most once). Venues that never overlap yield an empty
/// series — never an interpolated guess.
pub fn build_pair_series(
    mids_buy: &[(i64, f64)],
    mids_sell: &[(i64, f64)],
    max_gap_ms: i64,
) -> Vec<PairObservation> {
    let mut out = Vec::new();
    let mut j = 0usize;
    for &(ts_a, mid_a) in mids_buy {
        if !(mid_a.is_finite() && mid_a > 0.0) {
            continue;
        }
        while j < mids_sell.len() && mids_sell[j].0 < ts_a - max_gap_ms {
            j += 1;
        }
        // Nearest-in-time match inside [ts_a-gap, ts_a+gap], not first-in-
        // window: picking the window's oldest sell injects up to `gap` of
        // staleness into the spread. |Δt| is V-shaped over the sorted sell
        // series, so once past ts_a the first worsening candidate ends the
        // scan.
        let mut best: Option<(usize, i64, f64)> = None;
        let mut best_dt = i64::MAX;
        let mut k = j;
        while k < mids_sell.len() && mids_sell[k].0 <= ts_a + max_gap_ms {
            let (ts_b, mid_b) = mids_sell[k];
            if mid_b.is_finite() && mid_b > 0.0 {
                let dt = (ts_b - ts_a).abs();
                if dt < best_dt {
                    best = Some((k, ts_b, mid_b));
                    best_dt = dt;
                } else if ts_b > ts_a {
                    break;
                }
            }
            k += 1;
        }
        if let Some((best_idx, ts_b, mid_b)) = best {
            let spread_bps = (mid_b - mid_a) / mid_a * 10_000.0;
            if spread_bps.is_finite() {
                out.push(PairObservation {
                    ts_ms: ts_a.max(ts_b),
                    spread_bps,
                });
                // Advance past the consumed sample (and any skipped or
                // nearer-losing candidates before it) so the next buy
                // cannot reuse it — each sell feeds at most one pair.
                j = best_idx + 1;
            }
        }
    }
    out
}

/// Canonical bucket width for window-coverage accounting, ms. Mirrors the
/// monitor's alignment gap (`PAIR_ALIGN_GAP_MS` in the Tauri backend):
/// one expected pair observation per 30s bucket; empty buckets are nulls
/// (gaps stay gaps, never interpolated).
pub const PAIR_BUCKET_MS: i64 = 30_000;

/// Fraction of in-window observed buckets whose directional deviation
/// `spread - mu` sits at or above `entry_thresh_bps` — the same sign
/// convention as the monitor's `signal_active` (`dev >= entry_thresh`,
/// never `abs`). The window is the trailing `window_secs` ending at
/// `window_end_ms`; only observations with
/// `window_end - window_secs*1000 < ts <= window_end` count.
///
/// Returns `None` when uncomputable: non-finite `mu`/`entry_thresh`,
/// non-positive window, or zero finite in-window observations. Never
/// fabricates: no interpolation, no default ratio.
pub fn above_threshold_ratio(
    series: &[PairObservation],
    mu: f64,
    entry_thresh_bps: f64,
    window_secs: f64,
    window_end_ms: i64,
) -> Option<f64> {
    if !mu.is_finite() || !entry_thresh_bps.is_finite() {
        return None;
    }
    if !window_secs.is_finite() || window_secs <= 0.0 {
        return None;
    }
    let window_ms = window_secs * 1000.0;
    if !window_ms.is_finite() || window_ms <= 0.0 {
        return None;
    }
    let start_ms = window_end_ms as f64 - window_ms;
    if !start_ms.is_finite() {
        return None;
    }
    let mut denom = 0usize;
    let mut numer = 0usize;
    for o in series {
        let ts = o.ts_ms as f64;
        if !(ts > start_ms && ts <= window_end_ms as f64) {
            continue;
        }
        if !o.spread_bps.is_finite() {
            continue;
        }
        denom += 1;
        let dev = o.spread_bps - mu;
        if dev.is_finite() && dev >= entry_thresh_bps {
            numer += 1;
        }
    }
    if denom == 0 {
        return None;
    }
    let ratio = numer as f64 / denom as f64;
    if ratio.is_finite() {
        Some(ratio.clamp(0.0, 1.0))
    } else {
        None
    }
}

/// Fraction of expected window buckets that contain at least one pair
/// observation (both venues observed, non-null). The window is the trailing
/// `window_secs` ending at `window_end_ms`, split into `bucket_ms` buckets
/// (`ceil(window_ms / bucket_ms)` expected); a bucket counts when any
/// finite observation falls inside. Gaps stay gaps: empty buckets are nulls.
///
/// Returns `None` when uncomputable: non-finite/non-positive window or
/// bucket, or zero expected buckets. Never fabricates: sparse data reads as
/// low coverage, never as an interpolated fill.
pub fn effective_coverage(
    series: &[PairObservation],
    window_secs: f64,
    window_end_ms: i64,
    bucket_ms: i64,
) -> Option<f64> {
    if !window_secs.is_finite() || window_secs <= 0.0 {
        return None;
    }
    if bucket_ms <= 0 {
        return None;
    }
    let window_ms_f = window_secs * 1000.0;
    if !window_ms_f.is_finite() || window_ms_f <= 0.0 {
        return None;
    }
    let expected = (window_ms_f / bucket_ms as f64).ceil() as i64;
    if expected <= 0 {
        return None;
    }
    if expected > 1_000_000 {
        return None;
    }
    let start_f = window_end_ms as f64 - window_ms_f;
    if !start_f.is_finite() {
        return None;
    }
    let mut occupied = std::collections::HashSet::new();
    for o in series {
        let ts = o.ts_ms as f64;
        if !(ts > start_f && ts <= window_end_ms as f64) {
            continue;
        }
        if !o.spread_bps.is_finite() {
            continue;
        }
        let idx = ((ts - start_f) / bucket_ms as f64).floor() as i64;
        if idx >= 0 && idx < expected {
            occupied.insert(idx);
        } else if idx == expected {
            occupied.insert(expected - 1);
        }
    }
    let cov = occupied.len() as f64 / expected as f64;
    if cov.is_finite() {
        Some(cov.clamp(0.0, 1.0))
    } else {
        None
    }
}

/// Outcome of one deviation episode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EpisodeOutcome {
    /// Reverted to `|dev| <= band_bps`; seconds from episode entry.
    Win { secs: f64 },
    /// Widened to `|dev| >= stop_bps` before reverting.
    Stop,
    /// Series ended while the episode was still open.
    Open,
}

/// One deviation episode: entry index, peak |dev|, and its resolution.
#[derive(Debug, Clone)]
pub struct Episode {
    pub start_idx: usize,
    pub entry_ts_ms: i64,
    pub peak_dev_abs: f64,
    pub outcome: EpisodeOutcome,
    /// Signed deviation (`spread - mu`) at resolution — at the band for a
    /// Win (sign may have flipped through zero), at the stop for a Stop,
    /// at the last observation for an Open (censored).
    pub exit_dev_signed: f64,
    /// Timestamp of the resolution observation (last obs when Open).
    pub end_ts_ms: i64,
}

/// Segment the deviation series into episodes at `entry_dev_abs`:
/// an episode opens when `|dev|` first exceeds the level, then continues
/// (dips below the entry level stay inside) until it wins
/// (`|dev| <= band_bps`), loses (`|dev| >= stop_bps`), or the series ends.
pub fn segment_episodes(
    series: &[PairObservation],
    mu: f64,
    entry_dev_abs: f64,
    band_bps: f64,
    stop_bps: f64,
) -> Vec<Episode> {
    let entry = entry_dev_abs.max(1e-9);
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < series.len() {
        let d0 = (series[i].spread_bps - mu).abs();
        if d0 < entry {
            i += 1;
            continue;
        }
        // Episode opened at i. Scan forward until resolution.
        let mut peak = d0;
        let mut j = i + 1;
        let mut outcome = EpisodeOutcome::Open;
        while j < series.len() {
            let d = (series[j].spread_bps - mu).abs();
            peak = peak.max(d);
            if d <= band_bps {
                let secs = (series[j].ts_ms - series[i].ts_ms) as f64 / 1000.0;
                outcome = EpisodeOutcome::Win { secs };
                break;
            }
            if d >= stop_bps {
                outcome = EpisodeOutcome::Stop;
                break;
            }
            j += 1;
        }
        let end_idx = if j < series.len() {
            j
        } else {
            series.len() - 1
        };
        out.push(Episode {
            start_idx: i,
            entry_ts_ms: series[i].ts_ms,
            peak_dev_abs: peak,
            outcome,
            exit_dev_signed: series[end_idx].spread_bps - mu,
            end_ts_ms: series[end_idx].ts_ms,
        });
        // Continue after the resolved end (or after the open episode's tail).
        i = match outcome {
            EpisodeOutcome::Open => series.len(),
            _ => j + 1,
        };
    }
    out
}

/// Result of empirical episode analysis on a pair's deviation series.
#[derive(Debug, Clone, PartialEq)]
pub struct ReversionStats {
    /// Episodes that reached `|dev| >= dev_now_abs` (resolved or open).
    pub episodes: usize,
    /// Of those, how many resolved as wins.
    pub wins: usize,
    /// `wins / episodes` — `None` when `episodes < min_episodes`.
    pub p_win: Option<f64>,
    /// Median seconds-to-revert among winning episodes.
    pub median_converge_secs: Option<f64>,
}

/// Conditional win-rate curve: `P(win | excursion reached |dev| >= d)`
/// for each observed excursion magnitude `d`, ascending. Deeper book levels
/// enter at worse deviations → lower win probability; the ladder weights
/// each level by this curve.
#[derive(Debug, Clone, PartialEq)]
pub struct PWinCurve {
    /// (deviation_threshold_bps, conditional win rate), ascending.
    pub points: Vec<(f64, f64)>,
    /// Total excursions used to build the curve.
    pub episodes: usize,
    /// Weighted win count behind each point — parallel to `points`.
    /// Fractional when episodes are recency-weighted.
    pub point_wins: Vec<f64>,
    /// Weighted trial count behind each point — parallel to `points`.
    /// Includes Open (censored) episodes, which score as non-wins.
    pub point_trials: Vec<f64>,
    /// Censored (Open) episodes behind each point — parallel to `points`.
    /// A high censored fraction means the bucket's win rate is
    /// understated: those excursions never resolved inside the window.
    pub point_censored: Vec<usize>,
    /// Sum of squared episode weights (for Kish n_eff at each point).
    pub point_sum_w2: Vec<f64>,
}

/// Build the win-rate curve over all excursions above `band_bps`.
/// Each excursion is scored win/lose from its own entry forward, and each
/// curve point `d` aggregates all excursions whose peak reached `d`.
///
/// `half_life_secs` applies exponential recency weighting
/// (`w = 0.5^(age/half_life)` on each episode's resolution time) so stale
/// regimes fade instead of counting equally with fresh evidence;
/// `None`/`0` disables weighting. Open (censored) episodes count as
/// non-wins in the point estimate — conservative — but are tracked in
/// `point_censored` so consumers can see the unresolved share.
pub fn build_pwin_curve(
    series: &[PairObservation],
    mu: f64,
    sigma: f64,
    band_bps: f64,
    stop_mult: f64,
    half_life_secs: Option<f64>,
    now_ms: i64,
) -> PWinCurve {
    let entry = band_bps.max(1e-9);
    let stop_bps = (sigma * stop_mult).max(band_bps * 2.0);
    let eps = segment_episodes(series, mu, entry, band_bps, stop_bps);
    if eps.is_empty() {
        return PWinCurve {
            points: Vec::new(),
            episodes: 0,
            point_wins: Vec::new(),
            point_trials: Vec::new(),
            point_censored: Vec::new(),
            point_sum_w2: Vec::new(),
        };
    }
    let hl = half_life_secs.filter(|h| h.is_finite() && *h > 0.0);
    let weight = |e: &Episode| -> f64 {
        match hl {
            Some(h) => {
                let age_secs = (now_ms - e.end_ts_ms).max(0) as f64 / 1000.0;
                0.5_f64.powf(age_secs / h)
            }
            None => 1.0,
        }
    };
    let mut peaks: Vec<f64> = eps.iter().map(|e| e.peak_dev_abs).collect();
    peaks.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    peaks.dedup();
    let mut points = Vec::with_capacity(peaks.len());
    let mut point_wins = Vec::with_capacity(peaks.len());
    let mut point_trials = Vec::with_capacity(peaks.len());
    let mut point_censored = Vec::with_capacity(peaks.len());
    let mut point_sum_w2 = Vec::with_capacity(peaks.len());
    for &d in &peaks {
        let mut n = 0.0f64;
        let mut w = 0.0f64;
        let mut w2 = 0.0f64;
        let mut censored = 0usize;
        for e in &eps {
            if e.peak_dev_abs >= d - 1e-12 {
                let ew = weight(e);
                n += ew;
                w2 += ew * ew;
                if matches!(e.outcome, EpisodeOutcome::Win { .. }) {
                    w += ew;
                } else if matches!(e.outcome, EpisodeOutcome::Open) {
                    censored += 1;
                }
            }
        }
        if n > 0.0 {
            points.push((d, w / n));
            point_wins.push(w);
            point_trials.push(n);
            point_censored.push(censored);
            point_sum_w2.push(w2);
        }
    }
    PWinCurve {
        points,
        episodes: eps.len(),
        point_wins,
        point_trials,
        point_censored,
        point_sum_w2,
    }
}

/// Conditional win rate for a fresh entry at deviation magnitude `dev_abs`.
/// The correct conditional is "excursions whose peak reached ≥ dev_abs" —
/// since curve points are the observed peak thresholds, that bucket is the
/// **smallest** threshold ≥ `dev_abs`. `None` when the curve is empty or
/// `dev_abs` exceeds every observed peak — no historical evidence at that
/// magnitude, fail closed.
pub fn p_win_at_deviation(curve: &PWinCurve, dev_abs: f64) -> Option<f64> {
    for &(d, p) in &curve.points {
        if d >= dev_abs - 1e-12 {
            return Some(p);
        }
    }
    None
}

/// Uncertainty-aware conditional win rate at `dev_abs`: the same bucket
/// selection as [`p_win_at_deviation`], but returns a
/// [`crate::binomial::BinomialEstimate`] built from the bucket's weighted
/// win/trial counts — shrunken point estimate plus a Wilson interval on
/// the Kish-effective sample size. `lower` is the gateable quantity: on
/// thin buckets it collapses toward 0 instead of reporting a confident
/// small-sample rate. `None` when no bucket covers `dev_abs`.
pub fn p_win_est_at_deviation(
    curve: &PWinCurve,
    dev_abs: f64,
    z: f64,
    prior_strength: f64,
) -> Option<crate::binomial::BinomialEstimate> {
    for (i, &(d, _)) in curve.points.iter().enumerate() {
        if d >= dev_abs - 1e-12 {
            return Some(crate::binomial::binomial_estimate(
                curve.point_wins[i],
                curve.point_trials[i],
                curve.point_sum_w2[i],
                z,
                prior_strength,
            ));
        }
    }
    None
}

/// One simulated entry outcome at a fixed entry depth: replay a historical
/// excursion forward from the moment |dev| first crossed `entry_dev_abs`
/// and score the net bps a real reversion exit would have captured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntryOutcome {
    /// `sign(entry_dev)·(entry_dev − exit_dev) − friction_bps`. Positive
    /// on profitable exits; for censored episodes this is the mark at the
    /// series end — a real "if you exited now" number, not a guess.
    pub net_bps: f64,
    /// Seconds from entry crossing to resolution (or series end).
    pub hold_secs: f64,
    /// True when the episode never resolved inside the observed window.
    pub censored: bool,
    /// Recency weight (1.0 when weighting disabled).
    pub weight: f64,
}

/// Simulated-entry outcome statistics for one entry depth — the radar
/// analog of the funding monitor's [`crate::funding_carry::GapBacktest`]:
/// the win label is defined on **net P&L > 0**, not on the reversion
/// event, and the full outcome distribution is exposed via deciles.
#[derive(Debug, Clone, PartialEq)]
pub struct EntryOutcomeStats {
    /// Episodes that crossed the entry depth (weighted-trial count is in
    /// `p_net_pos.trials`).
    pub trials: usize,
    /// Episodes still open at the series end (censored).
    pub censored: usize,
    /// P(net > 0) over resolved episodes: shrunken `rate`, raw
    /// `rate_raw`, Wilson `lower`/`upper`. The `lower` bound counts every
    /// censored episode as a non-win — the fail-closed reading.
    /// `None` when `trials < min_trials`.
    pub p_net_pos: Option<crate::binomial::BinomialEstimate>,
    /// Weighted mean net bps per entry (censored included at their mark).
    pub expected_net_bps: Option<f64>,
    /// Ascending weighted net-bps deciles (11 points) — enables
    /// P(net >= any bps target) client-side, mirroring the funding
    /// monitor's `deciles_usd`.
    pub deciles_bps: Option<Vec<f64>>,
    /// Weighted median seconds from entry to exit (all episodes).
    pub median_hold_secs: Option<f64>,
    /// Weighted median seconds to resolution among winning episodes —
    /// feeds hold-time cost and capital-turnover estimates.
    pub median_win_secs: Option<f64>,
}

/// Replay every historical excursion that crossed `entry_dev_abs` and
/// score it as a real trade: enter at the crossing's signed deviation,
/// exit when |dev| reverts to `band_bps` (win), widens to `stop_bps`
/// (stop-out), or the series ends (censored — net marked at the last
/// observation). `net = sign(e0)·(e0 − dev_exit) − friction_bps`, so a
/// win at shallow depth can still be a small net loss — the label is
/// honest P&L, not a price-path event.
///
/// `half_life_secs` recency-weights episodes by resolution age
/// (`0.5^(age/half_life)`); `None`/`0` disables.
#[allow(clippy::too_many_arguments)]
pub fn simulate_entry_outcomes(
    series: &[PairObservation],
    mu: f64,
    entry_dev_abs: f64,
    band_bps: f64,
    stop_bps: f64,
    friction_bps: f64,
    half_life_secs: Option<f64>,
    now_ms: i64,
) -> Vec<EntryOutcome> {
    if !(entry_dev_abs.is_finite()
        && band_bps.is_finite()
        && stop_bps.is_finite()
        && friction_bps.is_finite()
        && mu.is_finite())
        || entry_dev_abs <= 0.0
        || band_bps <= 0.0
        || stop_bps <= 0.0
    {
        return Vec::new();
    }
    let hl = half_life_secs.filter(|h| h.is_finite() && *h > 0.0);
    let eps = segment_episodes(series, mu, entry_dev_abs, band_bps, stop_bps);
    eps.iter()
        .map(|e| {
            let e0 = series[e.start_idx].spread_bps - mu;
            let sign = if e0 >= 0.0 { 1.0 } else { -1.0 };
            let net = sign * (e0 - e.exit_dev_signed) - friction_bps;
            let hold_secs = (e.end_ts_ms - e.entry_ts_ms).max(0) as f64 / 1000.0;
            let weight = hl
                .map(|h| 0.5_f64.powf(((now_ms - e.end_ts_ms).max(0) as f64 / 1000.0) / h))
                .unwrap_or(1.0);
            EntryOutcome {
                net_bps: net,
                hold_secs,
                censored: matches!(e.outcome, EpisodeOutcome::Open),
                weight,
            }
        })
        .collect()
}

/// Aggregate [`EntryOutcome`]s into [`EntryOutcomeStats`]. `None`-valued
/// fields below `min_trials` resolved episodes — fail closed, never
/// extrapolate.
pub fn entry_outcome_stats(
    outcomes: &[EntryOutcome],
    min_trials: usize,
    z: f64,
    prior_strength: f64,
) -> EntryOutcomeStats {
    let trials = outcomes.len();
    let censored = outcomes.iter().filter(|o| o.censored).count();
    let resolved: Vec<&EntryOutcome> = outcomes.iter().filter(|o| !o.censored).collect();
    if resolved.len() < min_trials.max(1) {
        return EntryOutcomeStats {
            trials,
            censored,
            p_net_pos: None,
            expected_net_bps: None,
            deciles_bps: None,
            median_hold_secs: None,
            median_win_secs: None,
        };
    }
    // Resolved-sample point estimate; censored-as-losses lower bound.
    let (mut w_win, mut w_res, mut w_res2) = (0.0, 0.0, 0.0);
    let (mut w_all, mut w_all2) = (0.0, 0.0);
    for o in outcomes {
        w_all += o.weight;
        w_all2 += o.weight * o.weight;
        if !o.censored {
            w_res += o.weight;
            w_res2 += o.weight * o.weight;
            if o.net_bps > 0.0 {
                w_win += o.weight;
            }
        }
    }
    let resolved_est = crate::binomial::binomial_estimate(w_win, w_res, w_res2, z, prior_strength);
    // Lower bound over ALL trials with censored counted as non-wins —
    // the conservative reading when some episodes never resolved.
    let conservative = crate::binomial::binomial_estimate(w_win, w_all, w_all2, z, prior_strength);
    let p_net_pos = Some(crate::binomial::BinomialEstimate {
        lower: conservative.lower.or(resolved_est.lower),
        ..resolved_est
    });
    let nets: Vec<(f64, f64)> = outcomes.iter().map(|o| (o.net_bps, o.weight)).collect();
    let holds: Vec<(f64, f64)> = outcomes.iter().map(|o| (o.hold_secs, o.weight)).collect();
    let win_holds: Vec<(f64, f64)> = resolved
        .iter()
        .filter(|o| o.net_bps > 0.0)
        .map(|o| (o.hold_secs, o.weight))
        .collect();
    EntryOutcomeStats {
        trials,
        censored,
        p_net_pos,
        expected_net_bps: crate::binomial::weighted_mean(&nets),
        deciles_bps: Some(
            (0..=10)
                .filter_map(|k| crate::binomial::weighted_quantile(&nets, k as f64 / 10.0))
                .collect(),
        ),
        median_hold_secs: crate::binomial::weighted_quantile(&holds, 0.5),
        median_win_secs: crate::binomial::weighted_quantile(&win_holds, 0.5),
    }
}

/// Empirical reversion stats conditioned on the current deviation level.
pub fn reversion_stats(
    series: &[PairObservation],
    mu: f64,
    sigma: f64,
    dev_now: f64,
    band_bps: f64,
    stop_mult: f64,
    min_episodes: usize,
) -> ReversionStats {
    let stop_bps = (sigma * stop_mult).max(band_bps * 2.0);
    let eps = segment_episodes(series, mu, dev_now.abs(), band_bps, stop_bps);
    let episodes = eps.len();
    let mut wins = 0usize;
    let mut converge_secs = Vec::new();
    for e in &eps {
        if let EpisodeOutcome::Win { secs } = e.outcome {
            wins += 1;
            converge_secs.push(secs);
        }
    }
    converge_secs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    ReversionStats {
        episodes,
        wins,
        p_win: if episodes >= min_episodes && episodes > 0 {
            Some(wins as f64 / episodes as f64)
        } else {
            None
        },
        median_converge_secs: converge_secs
            .first()
            .map(|_| converge_secs[converge_secs.len() / 2]),
    }
}

/// Depth capacity at a marginal expected-net floor: how much notional can
/// be consumed from both books while each unit still contributes at least
/// `min_net_bps` of probability-weighted net capture.
#[derive(Debug, Clone, PartialEq)]
pub struct BpsCapacityTier {
    /// Marginal expected-net floor, bps per unit notional.
    pub min_net_bps: f64,
    /// Total book notional (USD) fillable while marginal expected net
    /// stays at/above the floor.
    pub capacity_usd: f64,
}

/// Marginal expected-net floors (bps/unit) evaluated for the depth-capacity
/// profile — from chunky edges down to barely-positive captures.
pub const BPS_CAPACITY_LEVELS: &[f64] = &[10.0, 5.0, 2.0, 1.0, 0.5];

/// One point on the capacity depth curve — the state after consuming a
/// book chunk, for charting cumulative and marginal profit vs size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DepthCurvePoint {
    /// Cumulative fillable notional up to and including this chunk, USD.
    pub cum_usd: f64,
    /// Cumulative raw (unweighted) net capture per unit notional, bps.
    pub cum_net_bps: f64,
    /// Cumulative probability-weighted expected net per unit, bps —
    /// `None` when no win-rate curve exists (unverifiable expectation).
    pub cum_exp_bps: Option<f64>,
    /// Marginal raw net per unit at this depth, bps (`dev − friction`).
    pub marg_net_bps: f64,
    /// Marginal expected net per unit at this depth, bps
    /// (`p·dev⁺ − friction`). `None` when no win-rate curve exists.
    pub marg_exp_bps: Option<f64>,
}

/// Aggregated capacity evaluation for a pair direction.
#[derive(Debug, Clone, PartialEq)]
pub struct CapacityReport {
    /// Fillable size with the highest expected USD (probability-weighted),
    /// evaluated at exact book-level boundaries — not probe tiers. `None`
    /// when no win-rate curve exists.
    pub optimal_size_usd: Option<f64>,
    /// Largest fillable size whose marginal level satisfies `p_min` with
    /// positive cumulative expectation — not merely where raw benefit hits
    /// zero.
    pub breakeven_size_usd: Option<f64>,
    /// Expected USD at the optimal size.
    pub expected_profit_usd: Option<f64>,
    /// Cumulative probability-weighted expected net capture per unit at
    /// the optimal size, bps.
    pub expected_net_bps_at_optimal: Option<f64>,
    /// Probability-weighted fillable notional across every book level whose
    /// marginal expected net (`p·dev⁺ − friction`) is positive — the
    /// opportunity's total expected capacity. `None` with no win-rate curve.
    pub expected_capacity_usd: Option<f64>,
    /// Fillable notional at each `BPS_CAPACITY_LEVELS` marginal expected-net
    /// floor; empty when no win-rate curve exists.
    pub bps_capacity_ladder: Vec<BpsCapacityTier>,
    /// Per-chunk capacity curve for charting: cumulative and marginal
    /// net/expected capture at each consumed book depth.
    pub depth_curve: Vec<DepthCurvePoint>,
}

/// Walk both orderbooks in lockstep and compute the probability-weighted
/// capture curve over the fillable depth — continuously, at exact book
/// boundaries rather than configured probe tiers.
///
/// * `buy_asks`: (price, size) ascending — the book we lift.
/// * `sell_bids`: (price, size) descending — the book we hit.
/// * `baseline_bps`: `mu` for this pair (historical expected spread).
/// * `friction_bps`: static per-unit costs — entry fees + funding carry +
///   estimated exit friction (caller composes one-way vs two-way).
/// * `usable_cap_usd`: optional bound on total usable notional (e.g. a
///   participation cap) — truncates the depth walk so every output
///   reflects only the depth we may actually consume.
/// * `pwin`: conditional win-rate curve; empty → expected fields `None`.
/// * `p_min`: minimum marginal win probability for the breakeven bound.
///
/// Marginal deviation at a consumed level pair `(a_L, b_L)`:
/// `dev_L = (b_L - a_L)/a_L * 1e4 - baseline_bps`.
/// Per-level expected contribution `P_win(|dev_L|) * max(0, dev_L) - friction`
/// weighted by the level's notional.
#[allow(clippy::too_many_arguments)]
pub fn capacity_ladder(
    buy_asks: &[(f64, f64)],
    sell_bids: &[(f64, f64)],
    baseline_bps: f64,
    friction_bps: f64,
    usable_cap_usd: Option<f64>,
    pwin: &PWinCurve,
    p_min: f64,
    z_gate: Option<f64>,
    prior_strength: f64,
) -> CapacityReport {
    struct Chunk {
        notional: f64,       // qty * ask price (buy-side notional)
        dev_bps: f64,        // marginal deviation at this level pair
        p_win: Option<f64>,  // shrunken conditional win rate at |dev|
        p_gate: Option<f64>, // Wilson lower bound at z_gate, else shrunken rate
    }
    let mut chunks: Vec<Chunk> = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    let (mut rem_a, mut rem_b) = (0.0, 0.0);
    let mut walked_usd = 0.0;
    while i < buy_asks.len() && j < sell_bids.len() {
        if let Some(cap) = usable_cap_usd {
            if walked_usd >= cap {
                break;
            }
        }
        if rem_a <= 0.0 {
            rem_a = buy_asks[i].1;
            if rem_a <= 0.0 {
                i += 1;
                continue;
            }
        }
        if rem_b <= 0.0 {
            rem_b = sell_bids[j].1;
            if rem_b <= 0.0 {
                j += 1;
                continue;
            }
        }
        let q = rem_a.min(rem_b);
        let a = buy_asks[i].0;
        let b = sell_bids[j].0;
        if a.is_finite() && a > 0.0 && b.is_finite() && b > 0.0 && q.is_finite() && q > 0.0 {
            let dev = (b - a) / a * 10_000.0 - baseline_bps;
            let est =
                p_win_est_at_deviation(pwin, dev.abs(), z_gate.unwrap_or(1.645), prior_strength);
            let mut notional = q * a;
            if let Some(cap) = usable_cap_usd {
                notional = notional.min((cap - walked_usd).max(0.0));
            }
            if notional > 0.0 {
                walked_usd += notional;
                chunks.push(Chunk {
                    notional,
                    dev_bps: dev,
                    p_win: est
                        .and_then(|e| e.rate)
                        .or_else(|| p_win_at_deviation(pwin, dev.abs())),
                    p_gate: match z_gate {
                        Some(_) => est.and_then(|e| e.lower),
                        None => est
                            .and_then(|e| e.rate)
                            .or_else(|| p_win_at_deviation(pwin, dev.abs())),
                    },
                });
            }
        }
        rem_a -= q;
        rem_b -= q;
        if rem_a <= 0.0 {
            i += 1;
        }
        if rem_b <= 0.0 {
            j += 1;
        }
    }

    // Single prefix walk over consumed depth: optimal is the boundary with
    // max cumulative expected USD (piecewise-linear within a constant-
    // marginal chunk, so the argmax always lands on a boundary); breakeven
    // is the deepest boundary whose marginal level still clears p_min with
    // positive cumulative expectation.
    let marginal_exp = |c: &Chunk| c.p_win.map(|p| p * c.dev_bps.max(0.0) - friction_bps);
    let any_p = chunks.iter().any(|c| c.p_win.is_some());
    let mut optimal: Option<(f64, f64, f64)> = None; // (cum_usd, exp_usd, cum_exp_bps)
    let mut breakeven: Option<f64> = None;
    let mut depth_curve: Vec<DepthCurvePoint> = Vec::with_capacity(chunks.len());
    {
        let mut cum_usd = 0.0;
        let mut net_integral = 0.0; // Σ (dev−friction)·notional  (usd·bps)
        let mut exp_integral = 0.0; // Σ (p·dev⁺−friction)·notional
        for c in &chunks {
            cum_usd += c.notional;
            net_integral += (c.dev_bps - friction_bps) * c.notional;
            if let Some(e) = marginal_exp(c) {
                exp_integral += e * c.notional;
            }
            depth_curve.push(DepthCurvePoint {
                cum_usd,
                cum_net_bps: net_integral / cum_usd,
                cum_exp_bps: if any_p {
                    Some(exp_integral / cum_usd)
                } else {
                    None
                },
                marg_net_bps: c.dev_bps - friction_bps,
                marg_exp_bps: marginal_exp(c),
            });
            if any_p && cum_usd > 0.0 {
                let cum_exp = exp_integral / cum_usd;
                let exp_usd = cum_exp * cum_usd / 10_000.0;
                if exp_usd > 0.0 {
                    if optimal.map(|(_, v, _)| exp_usd > v).unwrap_or(true) {
                        optimal = Some((cum_usd, exp_usd, cum_exp));
                    }
                    if c.p_gate.map(|p| p >= p_min).unwrap_or(false) {
                        breakeven = Some(cum_usd);
                    }
                }
            }
        }
    }

    // Depth-capacity profile: per book level, marginal expected net per
    // unit is `p·dev⁺ − friction`. Expected capacity weights each viable
    // level's notional by its win probability; the bps ladder reports raw
    // fillable notional while marginal expected net clears each floor.
    let expected_capacity_usd = if any_p {
        let mut sum = 0.0;
        for c in &chunks {
            if let (Some(p), Some(e)) = (c.p_win, marginal_exp(c)) {
                if e > 0.0 {
                    sum += c.notional * p;
                }
            }
        }
        Some(sum)
    } else {
        None
    };
    let bps_capacity_ladder = if any_p {
        BPS_CAPACITY_LEVELS
            .iter()
            .map(|&floor| BpsCapacityTier {
                min_net_bps: floor,
                capacity_usd: chunks
                    .iter()
                    .filter(|c| marginal_exp(c).map(|e| e >= floor).unwrap_or(false))
                    .map(|c| c.notional)
                    .sum(),
            })
            .collect()
    } else {
        Vec::new()
    };

    CapacityReport {
        optimal_size_usd: optimal.map(|(s, _, _)| s),
        breakeven_size_usd: breakeven,
        expected_profit_usd: optimal.map(|(_, v, _)| v),
        expected_net_bps_at_optimal: optimal.map(|(_, _, e)| e),
        expected_capacity_usd,
        bps_capacity_ladder,
        depth_curve,
    }
}

/// Conservative funding carry in bps for holding `long A / short B` over
/// the expected holding time.
///
/// * `rate_a_per_hour`, `rate_b_per_hour`: funding rates as fractions per
///   hour (0.0001 = 1 bps/hr), EMA-smoothed upstream. Positive rate means
///   longs pay shorts on that venue.
/// * Net cost (`rate_a > rate_b`) → assume `max(t_hold, 8h)`.
/// * Net gain → assume `min(t_hold, 1h)` of benefit.
/// * Any missing input → `None` (caller flags `funding_unknown`).
///
/// Returns signed bps: positive = cost, negative = gain.
pub fn funding_carry_bps(
    rate_a_per_hour: Option<f64>,
    rate_b_per_hour: Option<f64>,
    t_hold_secs: Option<f64>,
) -> Option<f64> {
    let (ra, rb) = (rate_a_per_hour?, rate_b_per_hour?);
    let net_per_hour = ra - rb;
    let t_est = t_hold_secs.unwrap_or(0.0).max(0.0);
    let t_hours = if net_per_hour > 0.0 {
        (t_est / 3600.0).max(8.0)
    } else {
        (t_est / 3600.0).min(1.0)
    };
    Some(net_per_hour * t_hours * 10_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(ts: i64, bps: f64) -> PairObservation {
        PairObservation {
            ts_ms: ts,
            spread_bps: bps,
        }
    }

    #[test]
    fn pair_series_aligns_within_gap() {
        let a = vec![(0, 100.0), (10, 100.0), (20, 100.0)];
        let b = vec![(1, 101.0), (12, 101.0), (21, 101.0)];
        let s = build_pair_series(&a, &b, 5);
        assert_eq!(s.len(), 3);
        assert!((s[0].spread_bps - 100.0).abs() < 1e-9);
    }

    #[test]
    fn pair_series_skips_unpaired_and_bad_mids() {
        let a = vec![(0, 100.0), (10, 0.0), (100, 100.0)];
        let b = vec![(1, 101.0)];
        let s = build_pair_series(&a, &b, 5);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].ts_ms, 1);
    }

    #[test]
    fn pair_series_picks_nearest_sell_not_oldest() {
        // Buy at t=10_000 with gap=30s: sells at t=-15_000 (25s stale, still
        // in window) and t=11_000 (1s fresh). First-in-window would take the
        // stale one; nearest-match takes the fresh print.
        let a = vec![(10_000, 100.0)];
        let b = vec![(-15_000, 105.0), (11_000, 101.0)];
        let s = build_pair_series(&a, &b, 30_000);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].ts_ms, 11_000, "nearest sell wins over oldest");
        assert!((s[0].spread_bps - 100.0).abs() < 1e-9);
    }

    #[test]
    fn pair_series_invalid_first_sell_consumed_once() {
        let buy = vec![(0, 100.0), (2, 100.0), (4, 100.0)];
        let sell = vec![(0, f64::NAN), (1, 101.0), (3, 102.0)];
        let s = build_pair_series(&buy, &sell, 5);
        assert_eq!(s.len(), 2, "consumed sell sample must not be reused");
        assert_eq!(s[0].ts_ms, 1);
        assert_eq!(s[1].ts_ms, 3);
        assert!((s[0].spread_bps - 100.0).abs() < 1e-9);
        assert!((s[1].spread_bps - 200.0).abs() < 1e-9);
    }

    #[test]
    fn episodes_win_stop_open() {
        // mu=0, band=2, stop=15.
        let series = vec![
            obs(0, 0.0),
            obs(1_000, 5.0),
            obs(2_000, 0.0), // ep1: entry 5 -> win in 1s
            obs(3_000, 10.0),
            obs(4_000, 16.0), // ep2: entry 10 -> hits stop
            obs(5_000, 0.0),
            obs(6_000, 7.0), // ep3: open at end
        ];
        let eps = segment_episodes(&series, 0.0, 4.0, 2.0, 15.0);
        assert_eq!(eps.len(), 3);
        assert!(
            matches!(eps[0].outcome, EpisodeOutcome::Win { secs } if (secs - 1.0).abs() < 1e-9)
        );
        assert!(matches!(eps[1].outcome, EpisodeOutcome::Stop));
        assert!(matches!(eps[2].outcome, EpisodeOutcome::Open));
    }

    #[test]
    fn pwin_curve_monotone_covers_levels() {
        let series = vec![
            obs(0, 0.0),
            obs(1, 5.0),
            obs(2, 0.0),
            obs(3, 10.0),
            obs(4, 0.0),
            obs(5, 20.0),
            obs(6, 30.0),
            obs(7, 0.0),
        ];
        // mu=0 sigma=5 stop=15 band=2
        let curve = build_pwin_curve(&series, 0.0, 5.0, 2.0, 3.0, None, 0);
        assert_eq!(curve.episodes, 3);
        let p5 = p_win_at_deviation(&curve, 5.0).unwrap();
        assert!((p5 - 2.0 / 3.0).abs() < 1e-9); // 2 of 3 excursions won
        let p30 = p_win_at_deviation(&curve, 30.0).unwrap();
        assert_eq!(p30, 0.0); // the only excursion reaching 30 lost
        assert!(p_win_at_deviation(&curve, 31.0).is_none()); // fail closed
    }

    #[test]
    fn reversion_stats_respects_min_episodes() {
        // ts in ms: entry at t=1000, win at t=2000 -> 1.0s converge.
        let series = vec![obs(0, 0.0), obs(1_000, 5.0), obs(2_000, 0.0)];
        let s = reversion_stats(&series, 0.0, 5.0, 5.0, 2.0, 3.0, 5);
        assert_eq!(s.episodes, 1);
        assert!(s.p_win.is_none()); // below min_episodes
        let s2 = reversion_stats(&series, 0.0, 5.0, 5.0, 2.0, 3.0, 1);
        assert_eq!(s2.p_win, Some(1.0));
        assert_eq!(s2.median_converge_secs, Some(1.0));
    }

    #[test]
    fn capacity_ladder_weights_levels_and_finds_breakeven() {
        let asks = vec![(100.0, 1.0), (101.0, 1.0)];
        let bids = vec![(102.0, 1.0), (99.0, 1.0)];
        // L1: a=100,b=102 -> dev 200bps; L2: a=101,b=99 -> dev ≈ -198bps
        // Curve semantics: p(x) = conditional over excursions with peak >= x
        // (lookup = smallest observed threshold >= x).
        let pwin = PWinCurve {
            points: vec![(0.0, 1.0), (200.0, 1.0), (300.0, 0.5)],
            episodes: 10,
            point_wins: vec![10.0, 10.0, 5.0],
            point_trials: vec![10.0, 10.0, 10.0],
            point_censored: vec![0, 0, 0],
            point_sum_w2: vec![10.0, 10.0, 10.0],
        };
        let rep = capacity_ladder(&asks, &bids, 0.0, 10.0, None, &pwin, 0.9, None, 0.0);
        // Continuous curve: one point per consumed chunk boundary.
        assert_eq!(rep.depth_curve.len(), 2);
        let c0 = &rep.depth_curve[0];
        assert!((c0.cum_usd - 100.0).abs() < 1e-6);
        assert!((c0.cum_net_bps - 190.0).abs() < 1e-6); // 200 − 10 friction
        assert!((c0.cum_exp_bps.unwrap() - 190.0).abs() < 1e-6);
        assert!((c0.marg_net_bps - 190.0).abs() < 1e-6);
        assert!((c0.marg_exp_bps.unwrap() - 190.0).abs() < 1e-6);
        let c1 = &rep.depth_curve[1];
        assert!((c1.cum_usd - 201.0).abs() < 1e-6);
        assert!(c1.cum_net_bps < c0.cum_net_bps); // deep level drags
                                                  // Optimal is the chunk boundary maximizing expected USD: prefix 1
                                                  // earns 190bps·$100 = $1.90; L2's negative level drags it lower.
        assert_eq!(rep.optimal_size_usd, Some(100.0));
        assert!((rep.expected_profit_usd.unwrap() - 1.9).abs() < 1e-6);
        assert!((rep.expected_net_bps_at_optimal.unwrap() - 190.0).abs() < 1e-6);
        // Both marginal gates clear p_min=0.9 (p=1.0) with positive
        // cumulative expectation — deepest boundary stands.
        assert_eq!(rep.breakeven_size_usd, Some(201.0));
        // Expected capacity = Σ notional·p over chunks with positive
        // marginal expected net. L1: dev 200 → p=1.0 → exp 190>0 →
        // $100·1.0. L2: dev ≈ −198 → dev⁺=0 → exp −10<0 → excluded.
        let cap = rep.expected_capacity_usd.unwrap();
        assert!((cap - 100.0).abs() < 1e-6);
        // Bps ladder: floor 10 → only L1 clears (exp 190 ≥ 10) → $100;
        // no floor admits L2 (exp < 0). At floor 0.5 still only L1.
        let l10 = rep
            .bps_capacity_ladder
            .iter()
            .find(|t| t.min_net_bps == 10.0)
            .unwrap();
        assert!((l10.capacity_usd - 100.0).abs() < 1e-6);
        let l05 = rep
            .bps_capacity_ladder
            .iter()
            .find(|t| t.min_net_bps == 0.5)
            .unwrap();
        assert!((l05.capacity_usd - 100.0).abs() < 1e-6);
    }

    #[test]
    fn capacity_ladder_optimal_lands_on_chunk_boundary() {
        // Profitable depth ends at $3,700 — a size no fixed probe tier
        // (1k/5k/10k) would ever evaluate. The continuous walk must land
        // the optimal exactly on the boundary.
        let asks = vec![(100.0, 37.0)];
        let bids = vec![(102.0, 37.0)];
        // dev 200bps -> bucket = first threshold ≥ 200 -> p = 1.0.
        let pwin = PWinCurve {
            points: vec![(300.0, 1.0)],
            episodes: 5,
            point_wins: vec![5.0],
            point_trials: vec![5.0],
            point_censored: vec![0],
            point_sum_w2: vec![5.0],
        };
        let rep = capacity_ladder(&asks, &bids, 0.0, 10.0, None, &pwin, 0.9, None, 0.0);
        assert_eq!(rep.depth_curve.len(), 1);
        assert!((rep.depth_curve[0].cum_usd - 3700.0).abs() < 1e-6);
        assert_eq!(rep.optimal_size_usd, Some(3700.0));
        assert!((rep.expected_profit_usd.unwrap() - 190.0 * 3700.0 / 10_000.0).abs() < 1e-6);
        assert_eq!(rep.breakeven_size_usd, Some(3700.0));
    }

    #[test]
    fn capacity_ladder_usable_cap_truncates_walk() {
        let asks = vec![(100.0, 1.0), (101.0, 100.0)];
        let bids = vec![(102.0, 1.0), (103.0, 100.0)];
        // L2: dev = (103−101)/101·1e4 ≈ 198bps — also profitable (p=1.0).
        let pwin = PWinCurve {
            points: vec![(0.0, 1.0), (200.0, 1.0), (300.0, 0.5)],
            episodes: 10,
            point_wins: vec![10.0, 10.0, 5.0],
            point_trials: vec![10.0, 10.0, 10.0],
            point_censored: vec![0, 0, 0],
            point_sum_w2: vec![10.0, 10.0, 10.0],
        };
        // Usable notional capped at $150: chunk1 fills $100, chunk2 is
        // truncated to $50 of its raw $10,100 — the boundary sits on the
        // cap, not on any book or probe size.
        let rep = capacity_ladder(&asks, &bids, 0.0, 10.0, Some(150.0), &pwin, 0.9, None, 0.0);
        assert_eq!(rep.depth_curve.len(), 2);
        assert!((rep.depth_curve[1].cum_usd - 150.0).abs() < 1e-6);
        assert!(
            (rep.depth_curve[1].marg_net_bps - ((103.0 - 101.0) / 101.0 * 10_000.0 - 10.0)).abs()
                < 1e-6
        );
        // Both chunks profitable with gate 1.0 — optimal + breakeven at cap.
        assert_eq!(rep.optimal_size_usd, Some(150.0));
        assert_eq!(rep.breakeven_size_usd, Some(150.0));
        // Cap below the first chunk zeroes everything downstream.
        let rep = capacity_ladder(&asks, &bids, 0.0, 10.0, Some(0.0), &pwin, 0.9, None, 0.0);
        assert!(rep.depth_curve.is_empty());
        assert!(rep.optimal_size_usd.is_none());
    }

    #[test]
    fn capacity_ladder_bps_profile_sums_deep_levels() {
        // Both levels profitable: L1 dev 200→p=1.0→exp 190; L2 dev ~49.5→
        // p=0.95→exp ~42 — every level clears the 10bps floor.
        let asks = vec![(100.0, 1.0), (101.0, 10.0)];
        let bids = vec![(102.0, 1.0), (101.5, 10.0)];
        let pwin = PWinCurve {
            points: vec![(100.0, 0.95), (200.0, 1.0)],
            episodes: 20,
            point_wins: vec![19.0, 20.0],
            point_trials: vec![20.0, 20.0],
            point_censored: vec![0, 0],
            point_sum_w2: vec![20.0, 20.0],
        };
        let rep = capacity_ladder(&asks, &bids, 0.0, 5.0, None, &pwin, 0.5, None, 0.0);
        // Book notional: L1 1·100 = $100, L2 10·101 = $1010 → total $1110.
        let l10 = rep
            .bps_capacity_ladder
            .iter()
            .find(|t| t.min_net_bps == 10.0)
            .unwrap();
        assert!((l10.capacity_usd - 1110.0).abs() < 1e-6);
        // Expected capacity = 100·1.0 + 1010·0.95 ≈ $1059.5
        let cap = rep.expected_capacity_usd.unwrap();
        assert!((cap - (100.0 + 1010.0 * 0.95)).abs() < 1e-6);
    }

    #[test]
    fn capacity_ladder_expected_capacity_none_without_curve() {
        let asks = vec![(100.0, 1.0)];
        let bids = vec![(102.0, 1.0)];
        let rep = capacity_ladder(
            &asks,
            &bids,
            0.0,
            10.0,
            None,
            &PWinCurve {
                points: vec![],
                episodes: 0,
                point_wins: vec![],
                point_trials: vec![],
                point_censored: vec![],
                point_sum_w2: vec![],
            },
            0.9,
            None,
            0.0,
        );
        assert!(rep.expected_capacity_usd.is_none());
        assert!(rep.bps_capacity_ladder.is_empty());
    }

    #[test]
    fn capacity_ladder_breakeven_requires_p_min() {
        let asks = vec![(100.0, 1.0), (101.0, 10.0)];
        let bids = vec![(102.0, 1.0), (101.5, 10.0)];
        // L2 dev = (0.5/101)*1e4 ≈ 49.5bps -> bucket = first threshold >= 49.5
        // = 100 -> p=0.5 < p_min 0.9. Chunk1 marginal (dev 200) -> 200 -> 0.95.
        let pwin = PWinCurve {
            points: vec![(100.0, 0.5), (200.0, 0.95)],
            episodes: 20,
            point_wins: vec![10.0, 19.0],
            point_trials: vec![20.0, 20.0],
            point_censored: vec![0, 0],
            point_sum_w2: vec![20.0, 20.0],
        };
        let rep = capacity_ladder(&asks, &bids, 0.0, 5.0, None, &pwin, 0.9, None, 0.0);
        // Chunk2's marginal gate is 0.5 < 0.9 -> breakeven stops at the
        // first boundary ($100), even though cumulative exp stays positive.
        assert_eq!(rep.breakeven_size_usd, Some(100.0));
        // Optimal still walks deeper: exp_usd at $1110 (0.5·49.5−5 weighted
        // in) exceeds the $100 prefix's 185bps·$100.
        assert_eq!(rep.optimal_size_usd, Some(1110.0));
    }

    #[test]
    fn capacity_ladder_no_curve_is_honest() {
        let asks = vec![(100.0, 1.0)];
        let bids = vec![(102.0, 1.0)];
        let rep = capacity_ladder(
            &asks,
            &bids,
            0.0,
            10.0,
            None,
            &PWinCurve {
                points: vec![],
                episodes: 0,
                point_wins: vec![],
                point_trials: vec![],
                point_censored: vec![],
                point_sum_w2: vec![],
            },
            0.9,
            None,
            0.0,
        );
        // Raw curve still renders; every expectation-gated field is None.
        assert_eq!(rep.depth_curve.len(), 1);
        assert!(rep.depth_curve[0].cum_exp_bps.is_none());
        assert!((rep.depth_curve[0].cum_net_bps - 190.0).abs() < 1e-6);
        assert!(rep.expected_net_bps_at_optimal.is_none());
        assert!(rep.optimal_size_usd.is_none());
        assert!(rep.breakeven_size_usd.is_none());
        assert!(rep.expected_profit_usd.is_none());
    }

    #[test]
    fn funding_carry_conservative_horizons() {
        let cost = funding_carry_bps(Some(0.0002), Some(0.0001), Some(7200.0)).unwrap();
        assert!((cost - 8.0).abs() < 1e-9); // cost: hold assumed >= 8h
        let gain = funding_carry_bps(Some(0.0001), Some(0.0003), Some(86400.0)).unwrap();
        assert!((gain + 2.0).abs() < 1e-9); // gain: benefit capped at 1h
        assert!(funding_carry_bps(None, Some(0.0001), Some(1.0)).is_none());
    }

    #[test]
    fn threshold_ratio_persistent_beats_flicker_same_peak() {
        let end = 1_000_000i64;
        let mk = |spreads: &[f64]| -> Vec<PairObservation> {
            spreads
                .iter()
                .enumerate()
                .map(|(i, s)| obs(end - (spreads.len() as i64 - i as i64) * 30_000, *s))
                .collect()
        };
        // Same peak (12bps), mu=0, thresh=5: persistent 16/20 above,
        // flicker 1/20 above.
        let persistent = mk(&[
            12.0, 12.0, 12.0, 12.0, 0.0, 12.0, 12.0, 12.0, 12.0, 0.0, 12.0, 12.0, 12.0, 12.0, 0.0,
            12.0, 12.0, 12.0, 12.0, 0.0,
        ]);
        let flicker = mk(&[
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 12.0,
        ]);
        let rp = above_threshold_ratio(&persistent, 0.0, 5.0, 3600.0, end).unwrap();
        let rf = above_threshold_ratio(&flicker, 0.0, 5.0, 3600.0, end).unwrap();
        assert!((rp - 0.8).abs() < 1e-9, "persistent ratio {rp}");
        assert!((rf - 0.05).abs() < 1e-9, "flicker ratio {rf}");
        assert!(rp >= 0.2 && rf < 0.2, "floor 0.2 separates them");
    }

    #[test]
    fn threshold_ratio_is_directional_not_abs() {
        let s = vec![obs(1000, -12.0), obs(2000, -12.0), obs(3000, -12.0)];
        // |dev| is 12 everywhere but directional dev (-12) never >= +5.
        assert_eq!(above_threshold_ratio(&s, 0.0, 5.0, 3600.0, 3000), Some(0.0));
        let s2 = vec![obs(1000, 12.0), obs(2000, 12.0), obs(3000, -12.0)];
        let r = above_threshold_ratio(&s2, 0.0, 5.0, 3600.0, 3000).unwrap();
        assert!((r - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn coverage_full_vs_thin_same_dev() {
        let end = 5_000_000i64;
        let window = 3600.0; // 120 buckets of 30s
        let full: Vec<PairObservation> = (0..120)
            .map(|i| obs(end - (119 - i) * 30_000, 12.0))
            .collect();
        let thin: Vec<PairObservation> = (0..12)
            .map(|i| obs(end - (11 - i) * 300_000, 12.0))
            .collect();
        let cf = effective_coverage(&full, window, end, PAIR_BUCKET_MS).unwrap();
        let ct = effective_coverage(&thin, window, end, PAIR_BUCKET_MS).unwrap();
        assert!(cf >= 0.99, "full coverage {cf}");
        assert!(ct < 0.5, "thin coverage {ct} must fail the 0.5 floor");
    }

    #[test]
    fn signal_gates_deterministic_nan_empty_guards() {
        let s = vec![obs(1000, 12.0), obs(2000, 0.0), obs(3000, 12.0)];
        let a = above_threshold_ratio(&s, 0.0, 5.0, 3600.0, 3000);
        let b = above_threshold_ratio(&s, 0.0, 5.0, 3600.0, 3000);
        assert_eq!(a, b);
        let c = effective_coverage(&s, 60.0, 3000, 30_000);
        let d = effective_coverage(&s, 60.0, 3000, 30_000);
        assert_eq!(c, d);
        // Empty / degenerate inputs fail closed.
        let empty: Vec<PairObservation> = Vec::new();
        assert!(above_threshold_ratio(&empty, 0.0, 5.0, 60.0, 3000).is_none());
        assert!(above_threshold_ratio(&s, f64::NAN, 5.0, 60.0, 3000).is_none());
        assert!(above_threshold_ratio(&s, 0.0, f64::NAN, 60.0, 3000).is_none());
        assert!(above_threshold_ratio(&s, 0.0, 5.0, 0.0, 3000).is_none());
        assert!(effective_coverage(&empty, 60.0, 3000, 30_000).is_some());
        assert!(effective_coverage(&s, 0.0, 3000, 30_000).is_none());
        assert!(effective_coverage(&s, 60.0, 3000, 0).is_none());
        assert!(effective_coverage(&s, f64::NAN, 3000, 30_000).is_none());
        // Non-finite spreads are skipped, never counted as above.
        let dirty = vec![
            obs(1000, f64::NAN),
            obs(2000, 12.0),
            obs(3000, f64::INFINITY),
        ];
        let r = above_threshold_ratio(&dirty, 0.0, 5.0, 3600.0, 3000).unwrap();
        assert!((r - 1.0).abs() < 1e-9);
    }
}
