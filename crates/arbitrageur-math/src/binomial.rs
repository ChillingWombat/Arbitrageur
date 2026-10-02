//! Binomial-proportion estimation with honest uncertainty.
//!
//! Point-estimate win rates on small samples are the most common way
//! trading systems lie to themselves: `4/5 = 80%` looks decisive but its
//! 95% interval spans [36%, 96%]. This module provides the two standard
//! fixes used throughout the monitors:
//!
//! - **Wilson score interval** — better small-sample coverage than the
//!   normal approximation; supports fractional (weighted) counts via the
//!   Kish effective sample size `n_eff = (Σw)²/Σw²`.
//! - **Beta-prior shrinkage** — `p̂ = (w + α)/(n + α + β)` with a weak
//!   symmetric prior (α=β) pulls thin samples toward 0.5, so a 1/1 or
//!   0/1 track record never reports 100%/0% confidence.
//!
//! Gating convention: decisions use `lower` (the one-sided confidence
//! bound), never the point estimate. Display uses `rate` (shrunken).

/// A binomial proportion estimate with uncertainty.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BinomialEstimate {
    /// Beta-prior shrunken point estimate `(w + α)/(n + α + β)`. `None`
    /// when `trials == 0`.
    pub rate: Option<f64>,
    /// Raw weighted hit rate `Σwx / Σw`. `None` when `trials == 0`.
    pub rate_raw: Option<f64>,
    /// Wilson lower bound at `z` on the effective sample size —
    /// conservative; use for gating. `None` when `n_effective == 0`.
    pub lower: Option<f64>,
    /// Wilson upper bound at `z`. `None` when `n_effective == 0`.
    pub upper: Option<f64>,
    /// Sum of weights (fractional when recency-weighted).
    pub trials: f64,
    /// Kish effective sample size `(Σw)²/Σw²`. Equals `trials` when all
    /// weights are 1; shrinks toward 1 as weights concentrate.
    pub n_effective: f64,
}

/// Wilson score interval for a binomial proportion.
///
/// `wins`/`trials` may be fractional (recency-weighted); the interval is
/// computed on `n_eff` so concentrated weights widen rather than fake
/// precision. Returns `None` when inputs are unusable or `n_eff <= 0`.
pub fn wilson_interval(wins: f64, trials: f64, n_effective: f64, z: f64) -> Option<(f64, f64)> {
    if !(wins.is_finite() && trials.is_finite() && n_effective.is_finite() && z.is_finite())
        || trials <= 0.0
        || n_effective <= 0.0
        || z <= 0.0
        || wins < 0.0
        || wins > trials + 1e-9
    {
        return None;
    }
    let p = (wins / trials).clamp(0.0, 1.0);
    let n = n_effective;
    let z2 = z * z;
    let denom = 1.0 + z2 / n;
    let centre = p + z2 / (2.0 * n);
    let margin = z * ((p * (1.0 - p) / n) + z2 / (4.0 * n * n)).sqrt();
    let lo = ((centre - margin) / denom).clamp(0.0, 1.0);
    let hi = ((centre + margin) / denom).clamp(0.0, 1.0);
    if !(lo.is_finite() && hi.is_finite()) {
        return None;
    }
    Some((lo, hi))
}

/// Beta-prior shrunken win rate: `(wins + α)/(trials + α + β)` with a
/// symmetric prior `α = β = prior_strength / 2`, pulling toward 0.5.
/// `prior_strength` is the pseudo-count mass (2 ≈ "one win + one loss of
/// prior evidence"). Returns `None` when inputs are unusable.
pub fn shrunk_rate(wins: f64, trials: f64, prior_strength: f64) -> Option<f64> {
    if !(wins.is_finite() && trials.is_finite() && prior_strength.is_finite())
        || trials <= 0.0
        || prior_strength < 0.0
        || wins < 0.0
    {
        return None;
    }
    let a = prior_strength / 2.0;
    let p = (wins + a) / (trials + prior_strength);
    if !(0.0..=1.0).contains(&p) {
        return None;
    }
    Some(p)
}

/// Full estimate from weighted counts. `sum_weights` = Σw over trials,
/// `sum_w2` = Σw² (pass `sum_weights` when unweighted — every weight is
/// 1 and n_eff = n).
pub fn binomial_estimate(
    weighted_wins: f64,
    sum_weights: f64,
    sum_w2: f64,
    z: f64,
    prior_strength: f64,
) -> BinomialEstimate {
    if !(sum_weights.is_finite() && sum_w2.is_finite()) || sum_weights <= 0.0 || sum_w2 <= 0.0 {
        return BinomialEstimate {
            rate: None,
            rate_raw: None,
            lower: None,
            upper: None,
            trials: 0.0,
            n_effective: 0.0,
        };
    }
    // No cap at sum_weights: with fractional weights (recency decay)
    // n_eff legitimately exceeds Σw — e.g. ten episodes at w=0.5 are ten
    // half-stale trials, n_eff = 10 > Σw = 5.
    let n_eff = (sum_weights * sum_weights) / sum_w2;
    let (lower, upper) = wilson_interval(weighted_wins, sum_weights, n_eff, z)
        .map(|(l, u)| (Some(l), Some(u)))
        .unwrap_or((None, None));
    BinomialEstimate {
        rate: shrunk_rate(weighted_wins, sum_weights, prior_strength),
        rate_raw: Some((weighted_wins / sum_weights).clamp(0.0, 1.0)),
        lower,
        upper,
        trials: sum_weights,
        n_effective: n_eff,
    }
}

/// Kish effective sample size `(Σw)²/Σw²`. All-ones weights → n; with
/// fractional weights it can exceed Σw (ten half-weight trials ≈ ten
/// effective trials of degraded freshness).
pub fn kish_effective_n(sum_weights: f64, sum_w2: f64) -> Option<f64> {
    if !(sum_weights.is_finite() && sum_w2.is_finite()) || sum_weights <= 0.0 || sum_w2 <= 0.0 {
        return None;
    }
    Some((sum_weights * sum_weights) / sum_w2)
}

/// Weighted quantile (0..1) over `(value, weight)` pairs — inverse-CDF
/// convention: the smallest value whose cumulative weight reaches
/// `q·Σw`. Point masses stay point masses (a 90%-weighted value IS the
/// median). Returns `None` on empty or unusable input.
pub fn weighted_quantile(pairs: &[(f64, f64)], q: f64) -> Option<f64> {
    if pairs.is_empty() || !(0.0..=1.0).contains(&q) {
        return None;
    }
    let mut pts: Vec<(f64, f64)> = pairs
        .iter()
        .filter(|(v, w)| v.is_finite() && w.is_finite() && *w > 0.0)
        .copied()
        .collect();
    if pts.is_empty() {
        return None;
    }
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f64 = pts.iter().map(|(_, w)| w).sum();
    if total <= 0.0 {
        return None;
    }
    let target = q * total;
    let mut cum = 0.0;
    for &(v, w) in &pts {
        cum += w;
        if cum >= target {
            return Some(v);
        }
    }
    pts.last().map(|(v, _)| *v)
}

/// Batch-evaluates multiple weighted quantiles over `(value, weight)` pairs.
///
/// Filters and sorts the underlying data ONCE and uses binary search across
/// precomputed cumulative weights, reducing complexity from O(K · N log N)
/// with K allocations to O(N log N + K log N) with a single allocation.
pub fn weighted_quantiles(pairs: &[(f64, f64)], quantiles: &[f64]) -> Vec<Option<f64>> {
    if pairs.is_empty() || quantiles.is_empty() {
        return vec![None; quantiles.len()];
    }
    let mut pts: Vec<(f64, f64)> = pairs
        .iter()
        .filter(|(v, w)| v.is_finite() && w.is_finite() && *w > 0.0)
        .copied()
        .collect();
    if pts.is_empty() {
        return vec![None; quantiles.len()];
    }
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f64 = pts.iter().map(|(_, w)| w).sum();
    if total <= 0.0 {
        return vec![None; quantiles.len()];
    }

    let mut cum_weights = Vec::with_capacity(pts.len());
    let mut acc = 0.0;
    for &(_, w) in &pts {
        acc += w;
        cum_weights.push(acc);
    }

    quantiles
        .iter()
        .map(|&q| {
            if !(0.0..=1.0).contains(&q) {
                return None;
            }
            let target = q * total;
            let idx = match cum_weights
                .binary_search_by(|w| w.partial_cmp(&target).unwrap_or(std::cmp::Ordering::Equal))
            {
                Ok(i) => i,
                Err(i) => i.min(pts.len() - 1),
            };
            Some(pts[idx].0)
        })
        .collect()
}

/// Weighted mean of `(value, weight)` pairs. `None` on empty/unusable.
pub fn weighted_mean(pairs: &[(f64, f64)]) -> Option<f64> {
    let mut sw = 0.0;
    let mut swv = 0.0;
    for &(v, w) in pairs {
        if v.is_finite() && w.is_finite() && w > 0.0 {
            sw += w;
            swv += v * w;
        }
    }
    if sw <= 0.0 {
        return None;
    }
    Some(swv / sw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wilson_small_sample_is_wide() {
        // 4/5 = 80% point but the 95% interval must be wide.
        let (lo, hi) = wilson_interval(4.0, 5.0, 5.0, 1.96).unwrap();
        assert!(lo < 0.45 && hi > 0.95, "got [{lo}, {hi}]");
    }

    #[test]
    fn wilson_large_sample_is_tight() {
        let (lo, hi) = wilson_interval(80.0, 100.0, 100.0, 1.96).unwrap();
        assert!(lo > 0.70 && hi < 0.88, "got [{lo}, {hi}]");
    }

    #[test]
    fn wilson_rejects_bad_input() {
        assert!(wilson_interval(1.0, 0.0, 0.0, 1.96).is_none());
        assert!(wilson_interval(6.0, 5.0, 5.0, 1.96).is_none());
        assert!(wilson_interval(f64::NAN, 5.0, 5.0, 1.96).is_none());
    }

    #[test]
    fn shrinkage_pulls_thin_samples_to_half() {
        // 1/1 raw = 100%; with prior_strength=2 → (1+1)/(1+2) = 0.667.
        let p = shrunk_rate(1.0, 1.0, 2.0).unwrap();
        assert!((p - 2.0 / 3.0).abs() < 1e-9);
        // 50/50 stays near its raw value.
        let p50 = shrunk_rate(50.0, 50.0, 2.0).unwrap();
        assert!((p50 - 51.0 / 52.0).abs() < 1e-9);
    }

    #[test]
    fn kish_penalizes_concentration() {
        // 10 equal weights → n_eff = 10.
        assert_eq!(kish_effective_n(10.0, 10.0).unwrap(), 10.0);
        // One dominant weight (0.9) + nine small (0.011..): n_eff ≈ 1.9.
        let n = kish_effective_n(1.0, 0.82).unwrap();
        assert!(n < 2.0 && n > 1.0);
    }

    #[test]
    fn estimate_zero_trials_is_empty() {
        let e = binomial_estimate(0.0, 0.0, 0.0, 1.645, 2.0);
        assert!(e.rate.is_none() && e.lower.is_none() && e.n_effective == 0.0);
    }

    #[test]
    fn weighted_quantile_interpolates() {
        let pairs = vec![(1.0, 1.0), (2.0, 1.0), (3.0, 1.0), (4.0, 1.0)];
        let med = weighted_quantile(&pairs, 0.5).unwrap();
        assert!((2.0..=3.0).contains(&med));
        assert_eq!(weighted_quantile(&pairs, 0.0), Some(1.0));
        assert_eq!(weighted_quantile(&pairs, 1.0), Some(4.0));
        assert!(weighted_quantile(&[], 0.5).is_none());
    }

    #[test]
    fn weighted_quantile_respects_weights() {
        // 90% of mass at 1.0, 10% at 100.0 → median must be 1.0.
        let pairs = vec![(1.0, 9.0), (100.0, 1.0)];
        assert_eq!(weighted_quantile(&pairs, 0.5), Some(1.0));
        let p95 = weighted_quantile(&pairs, 0.95).unwrap();
        assert!(p95 > 50.0);
    }

    #[test]
    fn weighted_quantiles_batch_matches_single() {
        let pairs = vec![
            (10.0, 1.5),
            (25.0, 0.5),
            (5.0, 2.0),
            (50.0, 1.0),
            (15.0, 3.0),
            (80.0, 0.2),
        ];
        let targets: Vec<f64> = (0..=10).map(|k| k as f64 / 10.0).collect();
        let batch = weighted_quantiles(&pairs, &targets);
        assert_eq!(batch.len(), targets.len());

        for (i, &q) in targets.iter().enumerate() {
            let single = weighted_quantile(&pairs, q);
            assert_eq!(
                batch[i], single,
                "Batch and single quantile mismatch at q={q}: batch={:?}, single={:?}",
                batch[i], single
            );
        }
    }
}
