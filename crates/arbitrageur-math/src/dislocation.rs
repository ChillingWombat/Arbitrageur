//! Cross-venue dislocation signal series.
//!
//! Given per-venue mid-price samples, produces each venue's deviation from a
//! chosen baseline in basis points plus the max–min "dislocation band" whose
//! height equals the gross pairwise spread. All functions are pure over
//! in-memory sample slices; persistence and transport live elsewhere.

use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Baseline conventions for the deviation series.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaselineMode {
    /// Volume-weighted mean of venue mids (equal weight when no weights are
    /// supplied). The "fair value" anchor: deviations sum to ~0.
    Consensus,
    /// The mid of the highest-weight venue — the price leader stays pinned
    /// at zero while every other venue floats around it.
    Leader,
}

/// A single venue's mid observation.
#[derive(Debug, Clone)]
pub struct MidSample {
    pub ts_ms: i64,
    pub venue: String,
    pub mid: f64,
}

/// Per-venue deviation series aligned to bucketed timestamps.
#[derive(Debug, Clone)]
pub struct DeviationSeries {
    /// Bucket timestamps (epoch ms), ascending.
    pub ts_ms: Vec<i64>,
    /// Sorted venue ids; index j into `dev_bps` rows refers to venues[j].
    pub venues: Vec<String>,
    /// `dev_bps[i][j]` — deviation of `venues[j]` at `ts_ms[i]` in bps, or
    /// `None` when the venue had no sample in that bucket.
    pub dev_bps: Vec<Vec<Option<f64>>>,
    /// Max/min deviation across venues present in each bucket (the band).
    /// Equal when fewer than two venues are present.
    pub band_max_bps: Vec<f64>,
    pub band_min_bps: Vec<f64>,
}

fn baseline_mid(
    mids: &HashMap<usize, f64>,
    venues: &[String],
    weights: &HashMap<String, f64>,
    mode: BaselineMode,
) -> Option<f64> {
    if mids.is_empty() {
        return None;
    }
    match mode {
        BaselineMode::Leader => {
            // Deterministic: highest weight wins; ties (and the no-weights
            // case) resolve to the lowest sorted venue index, never to hash
            // order.
            let leader_idx = mids.keys().copied().min_by(|a, b| {
                let wa = weights.get(&venues[*a]).copied().unwrap_or(0.0);
                let wb = weights.get(&venues[*b]).copied().unwrap_or(0.0);
                wb.partial_cmp(&wa)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.cmp(b))
            })?;
            mids.get(&leader_idx).copied()
        }
        BaselineMode::Consensus => {
            let any_weights = !weights.is_empty();
            let mut num = 0.0;
            let mut den = 0.0;
            for (&idx, &mid) in mids {
                let w = if any_weights {
                    weights.get(&venues[idx]).copied().unwrap_or(0.0)
                } else {
                    1.0
                };
                num += mid * w;
                den += w;
            }
            if den > 0.0 {
                Some(num / den)
            } else {
                // All present venues have zero reported weight: equal-weight.
                let sum: f64 = mids.values().sum();
                Some(sum / mids.len() as f64)
            }
        }
    }
}

/// Compute the deviation series over `samples`.
///
/// - `bucket_ms` merges observations into one column per bucket (pass the
///   poll cadence, e.g. 1000ms); samples within a bucket take the latest mid.
/// - `weights` maps venue_id -> relative weight (24h volume USD); empty map
///   means equal weighting and first-venue leader fallback.
/// - `max_points` downsamples buckets by striding while preserving endpoints.
///
/// Data-cleaning rules (these are what keep the chart honest):
/// - **Median per bucket.** A venue's bucket value is the median of its
///   in-bucket mids, so a single bad tick inside the bucket cannot move it.
/// - **Bounded carry-forward.** A venue with no in-bucket sample keeps its
///   last mid while it is younger than `stale_ms` — its standing quote, not
///   a fabricated one. Beyond the bound it becomes `None` (a real gap), never
///   a zero. This also keeps the baseline's venue set stable instead of
///   churning with per-venue update cadence (the old source of line bounce).
/// - **Comparison requires >= 2 venues.** A bucket where only one venue is
///   present is skipped entirely: a lone venue's deviation is trivially 0
///   and renders as a fake drop-to-zero touch.
/// - `stale_ms` = `max(4 * bucket_ms, 30s)` — the 30s floor matches the
///   pair-alignment window the rest of the system already treats as "in sync".
pub fn compute_deviation_series(
    samples: &[MidSample],
    weights: &HashMap<String, f64>,
    mode: BaselineMode,
    bucket_ms: i64,
    max_points: usize,
) -> DeviationSeries {
    let bucket_ms = bucket_ms.max(1);
    let max_points = max_points.max(2);
    let stale_ms = bucket_ms.saturating_mul(4).max(30_000);

    // venue -> ts-ordered valid observations; axis = union of buckets that
    // hold at least one raw sample (no raw samples -> no bucket, ever).
    let mut per_venue: BTreeMap<String, Vec<(i64, f64)>> = BTreeMap::new();
    let mut axis: BTreeSet<i64> = BTreeSet::new();
    for s in samples {
        if !s.mid.is_finite() || s.mid <= 0.0 {
            continue;
        }
        per_venue
            .entry(s.venue.clone())
            .or_default()
            .push((s.ts_ms, s.mid));
        axis.insert(s.ts_ms.div_euclid(bucket_ms) * bucket_ms);
    }
    for obs in per_venue.values_mut() {
        obs.sort_by_key(|p| p.0);
    }
    let venues: Vec<String> = per_venue.keys().cloned().collect();
    let venue_obs: Vec<&Vec<(i64, f64)>> = venues
        .iter()
        .map(|v| per_venue.get(v).expect("venue key"))
        .collect();

    // For each axis bucket: consume each venue's in-bucket observations via
    // an ascending pointer (obs between axis keys cannot exist — their own
    // buckets would be on the axis), take the median, else carry the venue's
    // last mid while it is within `stale_ms` of the bucket end.
    let mut rows: Vec<(i64, HashMap<usize, f64>)> = Vec::with_capacity(axis.len());
    let mut pos = vec![0usize; venues.len()];
    for &b in &axis {
        let b_end = b.saturating_add(bucket_ms);
        let mut mids: HashMap<usize, f64> = HashMap::new();
        for (j, obs) in venue_obs.iter().enumerate() {
            let mut in_bucket: Vec<f64> = Vec::new();
            while pos[j] < obs.len() && obs[pos[j]].0 < b_end {
                in_bucket.push(obs[pos[j]].1);
                pos[j] += 1;
            }
            let mid = if !in_bucket.is_empty() {
                Some(median(&mut in_bucket))
            } else if pos[j] > 0 {
                let (ts, m) = obs[pos[j] - 1];
                (b_end - ts <= stale_ms).then_some(m)
            } else {
                None
            };
            if let Some(m) = mid {
                mids.insert(j, m);
            }
        }
        if mids.len() >= 2 {
            rows.push((b, mids));
        }
    }

    let keep: Vec<usize> = if rows.len() <= max_points {
        (0..rows.len()).collect()
    } else {
        let n = rows.len() - 1;
        (0..max_points)
            .map(|i| ((i * n) as f64 / (max_points - 1) as f64).round() as usize)
            .collect()
    };

    let mut out = DeviationSeries {
        ts_ms: Vec::with_capacity(keep.len()),
        venues,
        dev_bps: Vec::with_capacity(keep.len()),
        band_max_bps: Vec::with_capacity(keep.len()),
        band_min_bps: Vec::with_capacity(keep.len()),
    };

    for &ki in &keep {
        let (bkey, mids) = &rows[ki];
        let base = match baseline_mid(mids, &out.venues, weights, mode) {
            Some(b) if b > 0.0 => b,
            _ => continue,
        };
        let mut row: Vec<Option<f64>> = vec![None; out.venues.len()];
        let mut max = f64::NEG_INFINITY;
        let mut min = f64::INFINITY;
        for (&idx, &mid) in mids {
            let dev = (mid / base - 1.0) * 10_000.0;
            row[idx] = Some(dev);
            if dev > max {
                max = dev;
            }
            if dev < min {
                min = dev;
            }
        }
        out.ts_ms.push(*bkey);
        out.dev_bps.push(row);
        out.band_max_bps
            .push(if max.is_finite() { max } else { 0.0 });
        out.band_min_bps
            .push(if min.is_finite() { min } else { 0.0 });
    }
    out
}

/// Median of a non-empty slice: middle element, or the mean of the two
/// middles for even counts. Callers guarantee non-empty.
fn median(vals: &mut [f64]) -> f64 {
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = vals.len();
    if n % 2 == 1 {
        vals[n / 2]
    } else {
        (vals[n / 2 - 1] + vals[n / 2]) / 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(ts: i64, venue: &str, mid: f64) -> MidSample {
        MidSample {
            ts_ms: ts,
            venue: venue.to_string(),
            mid,
        }
    }

    #[test]
    fn consensus_equal_weight_centers_deviations() {
        // A: 100, B: 102 -> mean 101; devs -99.0/+99.0 bps
        let samples = vec![s(1000, "a", 100.0), s(1000, "b", 102.0)];
        let series = compute_deviation_series(
            &samples,
            &HashMap::new(),
            BaselineMode::Consensus,
            1000,
            100,
        );
        assert_eq!(series.venues, vec!["a", "b"]);
        assert_eq!(series.ts_ms, vec![1000]);
        let row = &series.dev_bps[0];
        let dev_a = row[0].unwrap();
        let dev_b = row[1].unwrap();
        assert!((dev_a + dev_b).abs() < 1e-9, "deviations must sum to ~0");
        assert!(dev_a < 0.0 && dev_b > 0.0);
        // band height = gross pairwise spread
        assert!((series.band_max_bps[0] - series.band_min_bps[0] - 200.0 / 1.01).abs() < 1e-6);
    }

    #[test]
    fn leader_mode_anchors_leader_at_zero() {
        let samples = vec![s(1000, "a", 100.0), s(1000, "b", 102.0)];
        let mut w = HashMap::new();
        w.insert("a".to_string(), 1_000_000.0);
        w.insert("b".to_string(), 10.0);
        let series = compute_deviation_series(&samples, &w, BaselineMode::Leader, 1000, 100);
        let row = &series.dev_bps[0];
        assert_eq!(row[0].unwrap(), 0.0, "leader is pinned to baseline");
        assert!(
            (row[1].unwrap() - 200.0).abs() < 1e-9,
            "b is 2% = 200bps rich"
        );
    }

    #[test]
    fn leader_tiebreak_is_deterministic() {
        // No weights at all -> the alphabetically-first venue is the leader,
        // pinned at 0 regardless of map iteration order.
        let samples = vec![s(1000, "zzz", 50.0), s(1000, "aaa", 100.0)];
        let series =
            compute_deviation_series(&samples, &HashMap::new(), BaselineMode::Leader, 1000, 100);
        let row = &series.dev_bps[0];
        assert_eq!(row[0].unwrap(), 0.0, "'aaa' (lowest idx) must lead");
        assert!(
            (row[1].unwrap() - (-5_000.0)).abs() < 1e-9,
            "zzz is half of aaa"
        );
    }

    #[test]
    fn consensus_uses_volume_weights() {
        // A (100, w=3) + B (110, w=1) -> baseline 102.5
        let samples = vec![s(1000, "a", 100.0), s(1000, "b", 110.0)];
        let mut w = HashMap::new();
        w.insert("a".to_string(), 3.0);
        w.insert("b".to_string(), 1.0);
        let series = compute_deviation_series(&samples, &w, BaselineMode::Consensus, 1000, 100);
        let row = &series.dev_bps[0];
        let expected_a = (100.0 / 102.5 - 1.0) * 10_000.0;
        assert!((row[0].unwrap() - expected_a).abs() < 1e-9);
    }

    #[test]
    fn gaps_beyond_staleness_stay_gaps() {
        // bucket_ms=1000 -> stale bound 30s. b's last mid at t=60_000 is
        // 40s stale at bucket 100_000 -> honest gap, bucket dropped (lone a).
        // b at t=70_000 is 10s fresh at bucket 80_000 -> carried in.
        let samples = vec![
            s(0, "a", 100.0),
            s(0, "b", 101.0),
            s(60_000, "b", 101.5),
            s(70_000, "b", 101.5),
            s(80_000, "a", 100.5),
            s(100_000, "a", 100.5),
        ];
        let series = compute_deviation_series(
            &samples,
            &HashMap::new(),
            BaselineMode::Consensus,
            1000,
            100,
        );
        // Buckets 60_000 and 100_000 have a alone -> skipped; 70_000 has b
        // alone -> skipped; 0 and 80_000 have both.
        assert_eq!(series.ts_ms, vec![0, 80_000]);
        assert!(series.dev_bps[1][1].is_some(), "b@70s carries into 80s");
    }

    #[test]
    fn last_mid_carries_within_staleness() {
        // b's quote at t=500 stays standing for the t=1000 bucket (age 1.5s
        // <= 30s bound) even though b sampled nothing inside it.
        let samples = vec![s(0, "a", 100.0), s(500, "b", 101.0), s(1000, "a", 100.5)];
        let series = compute_deviation_series(
            &samples,
            &HashMap::new(),
            BaselineMode::Consensus,
            1000,
            100,
        );
        assert_eq!(series.ts_ms, vec![0, 1000]);
        let dev_b = series.dev_bps[1][1].expect("b carried within staleness");
        // baseline = mean(100.5, 101.0) = 100.75 -> b dev = +24.8bps
        let want = (101.0 / 100.75 - 1.0) * 10_000.0;
        assert!((dev_b - want).abs() < 1e-9, "{dev_b} vs {want}");
    }

    #[test]
    fn bucket_median_ignores_single_spike() {
        // One 5% spike between two clean prints: the bucket value is the
        // median (100.0), not the last sample (105.0).
        let samples = vec![
            s(0, "a", 100.0),
            s(0, "b", 100.0),
            s(1000, "a", 100.0),
            s(1100, "a", 105.0),
            s(1200, "a", 100.1),
            s(1000, "b", 100.0),
        ];
        let series = compute_deviation_series(
            &samples,
            &HashMap::new(),
            BaselineMode::Consensus,
            1000,
            100,
        );
        assert_eq!(series.ts_ms.len(), 2);
        let dev_a = series.dev_bps[1][0].unwrap();
        // median(100.0, 105.0, 100.1)=100.1 vs b 100.0 -> +4.9975bps,
        // not the +500bps the last-frame spike would have produced.
        assert!(
            (dev_a - 5.0).abs() < 0.5,
            "median damped the spike: {dev_a}"
        );
    }

    #[test]
    fn single_venue_buckets_are_skipped() {
        // Venue a alone can never produce a deviation; its buckets must not
        // emit a row (a lone venue's dev is trivially 0 -> fake zero touch).
        let samples = vec![s(0, "a", 100.0), s(1000, "a", 100.5)];
        let series = compute_deviation_series(
            &samples,
            &HashMap::new(),
            BaselineMode::Consensus,
            1000,
            100,
        );
        assert_eq!(series.venues, vec!["a"]);
        assert!(series.ts_ms.is_empty());
        assert!(series.dev_bps.is_empty());
    }

    #[test]
    fn downsampling_preserves_endpoints() {
        let mut samples = Vec::new();
        for i in 0..1000 {
            samples.push(s(i * 1000, "a", 100.0 + i as f64 * 0.01));
            samples.push(s(i * 1000, "b", 100.5 + i as f64 * 0.01));
        }
        let series = compute_deviation_series(
            &samples,
            &HashMap::new(),
            BaselineMode::Consensus,
            1000,
            100,
        );
        assert_eq!(series.ts_ms.len(), 100);
        assert_eq!(*series.ts_ms.first().unwrap(), 0);
        assert_eq!(*series.ts_ms.last().unwrap(), 999_000);
    }

    #[test]
    fn invalid_mids_are_dropped() {
        let samples = vec![
            s(1000, "a", f64::NAN),
            s(1000, "b", -5.0),
            s(1000, "c", 100.0),
            s(1000, "d", 101.0),
        ];
        let series = compute_deviation_series(
            &samples,
            &HashMap::new(),
            BaselineMode::Consensus,
            1000,
            100,
        );
        assert_eq!(series.venues, vec!["c", "d"]);
        assert_eq!(series.dev_bps.len(), 1);
        assert!(series.dev_bps[0][0].is_some());
    }
}
