//! Cross-venue dislocation signal series.
//!
//! Given per-venue mid-price samples, produces each venue's deviation from a
//! chosen baseline in basis points plus the max–min "dislocation band" whose
//! height equals the gross pairwise spread. All functions are pure over
//! in-memory sample slices; persistence and transport live elsewhere.

use std::collections::{BTreeMap, HashMap};

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
pub fn compute_deviation_series(
    samples: &[MidSample],
    weights: &HashMap<String, f64>,
    mode: BaselineMode,
    bucket_ms: i64,
    max_points: usize,
) -> DeviationSeries {
    let bucket_ms = bucket_ms.max(1);
    let max_points = max_points.max(2);

    // Bucket -> venue -> latest mid in bucket
    let mut buckets: BTreeMap<i64, HashMap<String, f64>> = BTreeMap::new();
    for s in samples {
        if !s.mid.is_finite() || s.mid <= 0.0 {
            continue;
        }
        let b = s.ts_ms.div_euclid(bucket_ms) * bucket_ms;
        buckets.entry(b).or_default().insert(s.venue.clone(), s.mid);
    }

    let mut venues: Vec<String> = Vec::new();
    for mids in buckets.values() {
        for v in mids.keys() {
            if !venues.contains(v) {
                venues.push(v.clone());
            }
        }
    }
    venues.sort();
    let venue_idx: HashMap<String, usize> = venues
        .iter()
        .enumerate()
        .map(|(i, v)| (v.clone(), i))
        .collect();

    let bucket_keys: Vec<i64> = buckets.keys().copied().collect();
    let keep: Vec<usize> = if bucket_keys.len() <= max_points {
        (0..bucket_keys.len()).collect()
    } else {
        let n = bucket_keys.len() - 1;
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
        let bkey = bucket_keys[ki];
        let raw = &buckets[&bkey];
        // venue index -> mid, present only
        let mids: HashMap<usize, f64> = raw
            .iter()
            .filter_map(|(v, m)| venue_idx.get(v.as_str()).map(|&i| (i, *m)))
            .collect();
        let base = match baseline_mid(&mids, &out.venues, weights, mode) {
            Some(b) if b > 0.0 => b,
            _ => continue,
        };
        let mut row: Vec<Option<f64>> = vec![None; out.venues.len()];
        let mut max = f64::NEG_INFINITY;
        let mut min = f64::INFINITY;
        for (&idx, &mid) in &mids {
            let dev = (mid / base - 1.0) * 10_000.0;
            row[idx] = Some(dev);
            if dev > max {
                max = dev;
            }
            if dev < min {
                min = dev;
            }
        }
        out.ts_ms.push(bkey);
        out.dev_bps.push(row);
        out.band_max_bps
            .push(if max.is_finite() { max } else { 0.0 });
        out.band_min_bps
            .push(if min.is_finite() { min } else { 0.0 });
    }
    out
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
    fn gaps_stay_gaps() {
        // venue b absent in bucket 2 -> None, not interpolated
        let samples = vec![
            s(1000, "a", 100.0),
            s(1000, "b", 101.0),
            s(2000, "a", 100.5),
        ];
        let series = compute_deviation_series(
            &samples,
            &HashMap::new(),
            BaselineMode::Consensus,
            1000,
            100,
        );
        assert_eq!(series.ts_ms, vec![1000, 2000]);
        assert_eq!(series.dev_bps[1][1], None);
        // single-venue bucket collapses the band
        assert_eq!(series.band_max_bps[1], series.band_min_bps[1]);
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
        ];
        let series = compute_deviation_series(
            &samples,
            &HashMap::new(),
            BaselineMode::Consensus,
            1000,
            100,
        );
        assert_eq!(series.venues, vec!["c"]);
        assert_eq!(series.dev_bps[0][0].unwrap(), 0.0);
    }
}
