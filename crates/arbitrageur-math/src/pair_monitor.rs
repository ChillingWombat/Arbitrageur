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
        let mut best: Option<(i64, f64)> = None;
        let mut best_offset: usize = 0;
        for (offset, (ts_b, mid_b)) in mids_sell.iter().skip(j).take(2).copied().enumerate() {
            if ts_b > ts_a + max_gap_ms {
                break;
            }
            if mid_b.is_finite() && mid_b > 0.0 {
                best = Some((ts_b, mid_b));
                best_offset = offset;
                break;
            }
        }
        if let Some((ts_b, mid_b)) = best {
            let spread_bps = (mid_b - mid_a) / mid_a * 10_000.0;
            if spread_bps.is_finite() {
                out.push(PairObservation {
                    ts_ms: ts_a.max(ts_b),
                    spread_bps,
                });
                // Advance past the consumed sample (and any skipped invalid
                // sample before it) so the next buy cannot reuse it.
                j += best_offset + 1;
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
        out.push(Episode {
            start_idx: i,
            entry_ts_ms: series[i].ts_ms,
            peak_dev_abs: peak,
            outcome,
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
}

/// Build the win-rate curve over all excursions above `band_bps`.
/// Each excursion is scored win/lose from its own entry forward, and each
/// curve point `d` aggregates all excursions whose peak reached `d`.
pub fn build_pwin_curve(
    series: &[PairObservation],
    mu: f64,
    sigma: f64,
    band_bps: f64,
    stop_mult: f64,
) -> PWinCurve {
    let entry = band_bps.max(1e-9);
    let stop_bps = (sigma * stop_mult).max(band_bps * 2.0);
    let eps = segment_episodes(series, mu, entry, band_bps, stop_bps);
    if eps.is_empty() {
        return PWinCurve {
            points: Vec::new(),
            episodes: 0,
        };
    }
    let mut peaks: Vec<f64> = eps.iter().map(|e| e.peak_dev_abs).collect();
    peaks.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    peaks.dedup();
    let mut points = Vec::with_capacity(peaks.len());
    for &d in &peaks {
        let mut n = 0usize;
        let mut w = 0usize;
        for e in &eps {
            if e.peak_dev_abs >= d - 1e-12 {
                n += 1;
                if matches!(e.outcome, EpisodeOutcome::Win { .. }) {
                    w += 1;
                }
            }
        }
        if n > 0 {
            points.push((d, w as f64 / n as f64));
        }
    }
    PWinCurve {
        points,
        episodes: eps.len(),
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
        median_converge_secs: converge_secs.first().map(|_| {
            converge_secs[converge_secs.len() / 2]
        }),
    }
}

/// One rung of the capacity ladder.
#[derive(Debug, Clone, PartialEq)]
pub struct CapacityTier {
    /// Tier notional (USD) requested.
    pub size_usd: f64,
    /// Notional actually fillable inside both books (<= size_usd).
    pub filled_usd: f64,
    /// Probability-weighted expected net capture per unit notional, bps —
    /// `None` when no win-rate curve exists (unverifiable expectation).
    pub expected_net_bps: Option<f64>,
    /// Win probability at the deepest level consumed by this tier.
    pub marginal_p_win: Option<f64>,
    /// Raw (unweighted) net capture at this tier's average entry, bps.
    pub net_bps: Option<f64>,
}

/// Aggregated capacity evaluation for a pair direction.
#[derive(Debug, Clone, PartialEq)]
pub struct CapacityReport {
    pub tiers: Vec<CapacityTier>,
    /// Tier with the highest expected USD (probability-weighted); `None`
    /// when no win-rate curve exists.
    pub optimal_size_usd: Option<f64>,
    /// Largest tier whose marginal level satisfies `p_min` with positive
    /// cumulative expectation — not merely where raw benefit hits zero.
    pub breakeven_size_usd: Option<f64>,
    /// Expected USD at the optimal tier.
    pub expected_profit_usd: Option<f64>,
}

/// Walk both orderbooks in lockstep and compute the probability-weighted
/// capture curve at each fixed notional tier.
///
/// * `buy_asks`: (price, size) ascending — the book we lift.
/// * `sell_bids`: (price, size) descending — the book we hit.
/// * `baseline_bps`: `mu` for this pair (historical expected spread).
/// * `friction_bps`: static per-unit costs — entry fees + funding carry +
///   estimated exit friction (caller composes one-way vs two-way).
/// * `pwin`: conditional win-rate curve; empty → expected fields `None`.
/// * `p_min`: minimum marginal win probability for the breakeven bound.
///
/// Marginal deviation at a consumed level pair `(a_L, b_L)`:
/// `dev_L = (b_L - a_L)/a_L * 1e4 - baseline_bps`.
/// Per-level expected contribution `P_win(|dev_L|) * max(0, dev_L) - friction`
/// weighted by the level's notional.
pub fn capacity_ladder(
    buy_asks: &[(f64, f64)],
    sell_bids: &[(f64, f64)],
    baseline_bps: f64,
    friction_bps: f64,
    tiers_usd: &[f64],
    pwin: &PWinCurve,
    p_min: f64,
) -> CapacityReport {
    struct Chunk {
        notional: f64,      // qty * ask price (buy-side notional)
        dev_bps: f64,       // marginal deviation at this level pair
        p_win: Option<f64>, // conditional win rate at |dev|
    }
    let mut chunks: Vec<Chunk> = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    let (mut rem_a, mut rem_b) = (0.0, 0.0);
    while i < buy_asks.len() && j < sell_bids.len() {
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
            chunks.push(Chunk {
                notional: q * a,
                dev_bps: dev,
                p_win: p_win_at_deviation(pwin, dev.abs()),
            });
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

    let mut tiers: Vec<CapacityTier> = Vec::with_capacity(tiers_usd.len());
    let mut optimal: Option<(f64, f64)> = None; // (filled_usd, expected_usd)
    let mut breakeven: Option<f64> = None;

    for &tier in tiers_usd {
        if !(tier.is_finite() && tier > 0.0) {
            continue;
        }
        let mut filled = 0.0;
        let mut sum_net = 0.0; // unweighted net integral (usd*bps)
        let mut sum_exp = 0.0; // probability-weighted integral
        let mut any_p = false;
        let mut marginal_p: Option<f64> = None;
        for c in &chunks {
            if filled >= tier {
                break;
            }
            let take = (tier - filled).min(c.notional);
            if take <= 0.0 {
                continue;
            }
            filled += take;
            sum_net += (c.dev_bps - friction_bps) * take;
            if let Some(p) = c.p_win {
                any_p = true;
                sum_exp += (p * c.dev_bps.max(0.0) - friction_bps) * take;
            }
            marginal_p = c.p_win;
        }
        if filled <= 0.0 {
            break; // book exhausted — deeper tiers unfillable
        }
        let net_bps = Some(sum_net / filled);
        let expected_net_bps = if any_p { Some(sum_exp / filled) } else { None };
        if let (Some(eb), true) = (expected_net_bps, any_p) {
            let exp_usd = eb * filled / 10_000.0;
            if exp_usd > 0.0 && optimal.map(|(_, v)| exp_usd > v).unwrap_or(true) {
                optimal = Some((filled, exp_usd));
            }
            let p_ok = marginal_p.map(|p| p >= p_min).unwrap_or(false);
            if exp_usd > 0.0 && p_ok {
                breakeven = Some(filled);
            }
        }
        tiers.push(CapacityTier {
            size_usd: tier,
            filled_usd: filled,
            expected_net_bps,
            marginal_p_win: marginal_p,
            net_bps,
        });
    }

    CapacityReport {
        tiers,
        optimal_size_usd: optimal.map(|(s, _)| s),
        breakeven_size_usd: breakeven,
        expected_profit_usd: optimal.map(|(_, v)| v),
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
        assert!(matches!(eps[0].outcome, EpisodeOutcome::Win { secs } if (secs - 1.0).abs() < 1e-9));
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
        let curve = build_pwin_curve(&series, 0.0, 5.0, 2.0, 3.0);
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
        };
        let rep = capacity_ladder(&asks, &bids, 0.0, 10.0, &[100.0, 201.0], &pwin, 0.9);
        assert_eq!(rep.tiers.len(), 2);
        let t0 = &rep.tiers[0];
        assert!((t0.net_bps.unwrap() - 190.0).abs() < 1e-6); // 200 - 10 friction
        assert!((t0.expected_net_bps.unwrap() - 190.0).abs() < 1e-6);
        let t1 = &rep.tiers[1];
        assert!(t1.net_bps.unwrap() < t0.net_bps.unwrap()); // deep level drags
        // Tier1 exp = 190bps*$100 = $1.90; tier2's negative level drags it
        // below tier1's expected USD.
        assert_eq!(rep.optimal_size_usd, Some(t0.filled_usd));
        assert_eq!(
            rep.breakeven_size_usd,
            Some(rep.tiers.last().unwrap().filled_usd)
        );
    }

    #[test]
    fn capacity_ladder_breakeven_requires_p_min() {
        let asks = vec![(100.0, 1.0), (101.0, 10.0)];
        let bids = vec![(102.0, 1.0), (101.5, 10.0)];
        // L2 dev = (0.5/101)*1e4 ≈ 49.5bps -> bucket = first threshold >= 49.5
        // = 100 -> p=0.5 < p_min 0.9. Tier1 marginal (dev 200) -> 200 -> 0.95.
        let pwin = PWinCurve {
            points: vec![(100.0, 0.5), (200.0, 0.95)],
            episodes: 20,
        };
        let rep = capacity_ladder(&asks, &bids, 0.0, 5.0, &[100.0, 300.0], &pwin, 0.9);
        // Tier2 marginal level has dev≈49.5 -> p=0.5 < 0.9 -> not breakeven.
        assert_eq!(rep.breakeven_size_usd, Some(rep.tiers[0].filled_usd));
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
            &[100.0],
            &PWinCurve {
                points: vec![],
                episodes: 0,
            },
            0.9,
        );
        assert_eq!(rep.tiers.len(), 1);
        assert!(rep.tiers[0].expected_net_bps.is_none());
        assert!(rep.optimal_size_usd.is_none());
        assert!(rep.breakeven_size_usd.is_none());
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
            12.0, 12.0, 12.0, 12.0, 0.0, 12.0, 12.0, 12.0, 12.0, 0.0, 12.0, 12.0, 12.0,
            12.0, 0.0, 12.0, 12.0, 12.0, 12.0, 0.0,
        ]);
        let flicker = mk(&[
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 12.0,
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
        assert_eq!(
            above_threshold_ratio(&s, 0.0, 5.0, 3600.0, 3000),
            Some(0.0)
        );
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
        let dirty = vec![obs(1000, f64::NAN), obs(2000, 12.0), obs(3000, f64::INFINITY)];
        let r = above_threshold_ratio(&dirty, 0.0, 5.0, 3600.0, 3000).unwrap();
        assert!((r - 1.0).abs() < 1e-9);
    }
}
