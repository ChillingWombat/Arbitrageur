//! Per-strategy auto-hide ticker filters shared by bot/scan targeting,
//! radar Execute gating, and the `get_strategy_tickers` IPC.
//!
//! Aggregation is the MIN across the ticker's in-scope venues with the
//! frontend's K/1000 asset-name normalization; any in-scope venue missing
//! the metric excludes the ticker (fail-closed). Thresholds of None or <= 0
//! disable their filter.

use std::collections::HashMap;

/// Backend hard ceiling for leverage. All gates pin this value; per-account
/// caps and strategy targets can only lower the effective leverage.
pub const HARD_LEVERAGE_CEILING: u32 = 5;

/// Effective order-time leverage: the minimum of the strategy target, the
/// account cap (`None` = unlimited), and the venue cap (`None` = unknown,
/// treated as the hard ceiling). Result is always within 1..=5.
pub fn effective_leverage(
    strategy_leverage: u32,
    account_cap: Option<u32>,
    venue_cap: Option<u32>,
) -> u32 {
    let mut eff = strategy_leverage.clamp(1, HARD_LEVERAGE_CEILING);
    if let Some(cap) = account_cap {
        eff = eff.min(cap.clamp(1, HARD_LEVERAGE_CEILING));
    }
    if let Some(cap) = venue_cap {
        eff = eff.min(cap.clamp(1, HARD_LEVERAGE_CEILING));
    }
    eff.clamp(1, HARD_LEVERAGE_CEILING)
}

/// Validate an optional auto-filter threshold. `None` (or <= 0, treated as
/// off downstream) is always accepted; otherwise the value must be finite
/// and non-negative.
pub fn validate_auto_threshold(value: Option<f64>, field: &str) -> Result<(), String> {
    match value {
        None => Ok(()),
        Some(v) if v.is_finite() && v >= 0.0 => Ok(()),
        _ => Err(format!("{field} must be finite and non-negative (None/0 = off)")),
    }
}

/// Threshold is active only when set to a finite value above zero.
pub fn threshold_active(value: Option<f64>) -> bool {
    value.is_some_and(|v| v.is_finite() && v > 0.0)
}

/// Normalize one venue's 24h-volume map lookup for a base asset. Mirrors the
/// frontend `getVenueAssetVolumeUsd` K/1000 handling (`PEPE` matches `PEPE`,
/// `KPEPE`, `1000PEPE`, case-insensitive) and additionally tolerates keys
/// carrying a quote suffix (`BTC-USDC`, `BTCUSDC`). Returns `None` when the
/// venue reports no positive finite volume for the asset.
pub fn venue_asset_volume_usd(volumes: &HashMap<String, f64>, base: &str) -> Option<f64> {
    let b = base.trim().to_uppercase();
    if b.is_empty() {
        return None;
    }
    let wanted = [b.clone(), format!("K{b}"), format!("1000{b}")];
    for (raw_key, v) in volumes {
        if !v.is_finite() || *v <= 0.0 {
            continue;
        }
        let key = raw_key.trim().to_uppercase();
        let stripped = strip_quote_suffix(&key);
        if wanted.iter().any(|w| w == &key || *w == stripped) {
            return Some(*v);
        }
    }
    None
}

fn strip_quote_suffix(key: &str) -> String {
    for suf in [
        "-USDC", "-USDT", "-USD", "/USDC", "/USDT", "/USD", "-PERP", "USDC", "USDT", "USD",
    ] {
        if key.len() > suf.len() && key.ends_with(suf) {
            let rest = &key[..key.len() - suf.len()];
            let rest = rest.trim_end_matches(['-', '/']);
            if rest.len() >= 2 {
                return rest.to_string();
            }
        }
    }
    key.to_string()
}

/// Per-strategy auto-filter thresholds resolved from a strategy record.
#[derive(Debug, Clone, Copy, Default)]
pub struct StrategyAutoFilters {
    pub min_volume_usd: Option<f64>,
    pub min_price_usd: Option<f64>,
}

/// Fail-closed per-ticker check against one strategy's auto-filters.
///
/// `in_scope_venues` are the venue ids forming the ticker's >= 2-venue
/// intersection. `volumes_by_venue` maps venue id -> raw 24h-volume map and
/// `mids_by_venue` maps venue id -> current mid price. Returns `(true, None)`
/// when every active filter passes; otherwise `(false, Some(reason))` naming
/// the first failing venue/metric.
pub fn ticker_passes_strategy_filters(
    base: &str,
    in_scope_venues: &[String],
    volumes_by_venue: &HashMap<String, HashMap<String, f64>>,
    mids_by_venue: &HashMap<String, f64>,
    filters: StrategyAutoFilters,
) -> (bool, Option<String>) {
    if threshold_active(filters.min_volume_usd) {
        let floor = filters.min_volume_usd.unwrap_or(f64::MAX);
        let mut min_seen: Option<f64> = None;
        for venue in in_scope_venues {
            match volumes_by_venue
                .get(venue)
                .and_then(|m| venue_asset_volume_usd(m, base))
            {
                Some(v) => min_seen = Some(min_seen.map_or(v, |m: f64| m.min(v))),
                None => {
                    return (
                        false,
                        Some(format!(
                            "{base} blocked by auto volume filter: venue {venue} reports no 24h volume"
                        )),
                    )
                }
            }
        }
        match min_seen {
            Some(m) if m >= floor => {}
            _ => {
                return (
                    false,
                    Some(format!(
                        "{base} blocked by auto volume filter: min venue volume below floor"
                    )),
                )
            }
        }
    }
    if threshold_active(filters.min_price_usd) {
        let floor = filters.min_price_usd.unwrap_or(f64::MAX);
        let mut min_seen: Option<f64> = None;
        for venue in in_scope_venues {
            match mids_by_venue.get(venue).copied().filter(|p| p.is_finite() && *p > 0.0) {
                Some(p) => min_seen = Some(min_seen.map_or(p, |m: f64| m.min(p))),
                None => {
                    return (
                        false,
                        Some(format!(
                            "{base} blocked by auto price filter: venue {venue} reports no mid price"
                        )),
                    )
                }
            }
        }
        match min_seen {
            Some(m) if m >= floor => {}
            _ => {
                return (
                    false,
                    Some(format!(
                        "{base} blocked by auto price filter: min venue mid below floor"
                    )),
                )
            }
        }
    }
    (true, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vols(pairs: &[(&str, f64)]) -> HashMap<String, f64> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), *v))
            .collect()
    }

    fn by_venue(pairs: &[(&str, HashMap<String, f64>)]) -> HashMap<String, HashMap<String, f64>> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    fn mids(pairs: &[(&str, f64)]) -> HashMap<String, f64> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), *v))
            .collect()
    }

    #[test]
    fn volume_key_matching_mirrors_frontend_k_variants() {
        let m = vols(&[("kPEPE", 5.0), ("BTC", 10.0), ("1000SHIB", 7.0)]);
        assert_eq!(venue_asset_volume_usd(&m, "pepe"), Some(5.0));
        assert_eq!(venue_asset_volume_usd(&m, "BTC"), Some(10.0));
        assert_eq!(venue_asset_volume_usd(&m, "shib"), Some(7.0));
        assert_eq!(venue_asset_volume_usd(&m, "TAO"), None);
    }

    #[test]
    fn volume_key_matching_tolerates_quote_suffixes() {
        let m = vols(&[("BTC-USDC", 9.0)]);
        assert_eq!(venue_asset_volume_usd(&m, "BTC"), Some(9.0));
    }

    #[test]
    fn volume_key_matching_rejects_nonpositive_or_nan() {
        let m = vols(&[("BTC", 0.0), ("ETH", f64::NAN)]);
        assert_eq!(venue_asset_volume_usd(&m, "BTC"), None);
        assert_eq!(venue_asset_volume_usd(&m, "ETH"), None);
    }

    #[test]
    fn min_aggregation_uses_smallest_in_scope_venue() {
        let scope = ["hyperliquid".to_string(), "lighter".to_string()];
        let volumes = by_venue(&[
            ("hyperliquid", vols(&[("BTC", 1_000_000_000.0)])),
            ("lighter", vols(&[("BTC", 50_000.0)])),
        ]);
        let filters = StrategyAutoFilters {
            min_volume_usd: Some(100_000.0),
            min_price_usd: None,
        };
        let (ok, _) = ticker_passes_strategy_filters("BTC", &scope, &volumes, &mids(&[]), filters);
        assert!(!ok, "MIN aggregation must fail on the smallest venue");
        let filters = StrategyAutoFilters {
            min_volume_usd: Some(10_000.0),
            min_price_usd: None,
        };
        let (ok, reason) =
            ticker_passes_strategy_filters("BTC", &scope, &volumes, &mids(&[]), filters);
        assert!(ok, "unexpected block: {reason:?}");
    }

    #[test]
    fn missing_venue_volume_excludes_ticker_fail_closed() {
        let scope = ["hyperliquid".to_string(), "vanta".to_string()];
        let volumes = by_venue(&[("hyperliquid", vols(&[("BTC", 1_000_000.0)]))]);
        let filters = StrategyAutoFilters {
            min_volume_usd: Some(1.0),
            min_price_usd: None,
        };
        let (ok, reason) =
            ticker_passes_strategy_filters("BTC", &scope, &volumes, &mids(&[]), filters);
        assert!(!ok);
        assert!(reason.unwrap_or_default().contains("vanta"));
    }

    #[test]
    fn price_filter_uses_min_mid_and_missing_excludes() {
        let scope = ["hyperliquid".to_string(), "lighter".to_string()];
        let filters = StrategyAutoFilters {
            min_volume_usd: None,
            min_price_usd: Some(100.0),
        };
        let (ok, _) = ticker_passes_strategy_filters(
            "BTC",
            &scope,
            &by_venue(&[]),
            &mids(&[("hyperliquid", 90.0), ("lighter", 500.0)]),
            filters,
        );
        assert!(!ok, "min mid below floor must block");
        let (ok, _) = ticker_passes_strategy_filters(
            "BTC",
            &scope,
            &by_venue(&[]),
            &mids(&[("hyperliquid", 500.0)]),
            filters,
        );
        assert!(!ok, "missing mid must block fail-closed");
    }

    #[test]
    fn disabled_filters_always_pass() {
        let scope = ["hyperliquid".to_string()];
        for f in [
            StrategyAutoFilters {
                min_volume_usd: None,
                min_price_usd: None,
            },
            StrategyAutoFilters {
                min_volume_usd: Some(0.0),
                min_price_usd: Some(-1.0),
            },
        ] {
            let (ok, reason) =
                ticker_passes_strategy_filters("BTC", &scope, &by_venue(&[]), &mids(&[]), f);
            assert!(ok, "disabled filters must pass: {reason:?}");
        }
    }

    #[test]
    fn effective_leverage_takes_min_with_unlimited_and_ceiling() {
        assert_eq!(effective_leverage(5, None, None), 5);
        assert_eq!(effective_leverage(5, Some(2), None), 2);
        assert_eq!(effective_leverage(3, Some(5), None), 3);
        assert_eq!(effective_leverage(5, None, Some(3)), 3);
        assert_eq!(effective_leverage(4, Some(3), Some(2)), 2);
        assert_eq!(effective_leverage(9, None, None), 5);
        assert_eq!(effective_leverage(0, None, None), 1);
        assert_eq!(effective_leverage(5, Some(9), Some(99)), 5);
    }

    #[test]
    fn auto_threshold_validation_bounds() {
        assert!(validate_auto_threshold(None, "f").is_ok());
        assert!(validate_auto_threshold(Some(0.0), "f").is_ok());
        assert!(validate_auto_threshold(Some(12.5), "f").is_ok());
        assert!(validate_auto_threshold(Some(f64::INFINITY), "f").is_err());
        assert!(validate_auto_threshold(Some(f64::NAN), "f").is_err());
        assert!(validate_auto_threshold(Some(-3.0), "f").is_err());
    }
}
