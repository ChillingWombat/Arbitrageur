//! Shared fill-attribution plumbing.
//!
//! Any strategy or bot consumes these helpers so realized fills always carry
//! their owning strategy: signal strategies (`futures`, `radar`, `funding`,
//! `spot`, `prediction`, `stat_arb`, `triangle`) map to one canonical ledger
//! identity, family realized PnL is computed from recorded leg fills (fees
//! included), and pre-attribution rows are quarantined under
//! [`UNATTRIBUTED_STRATEGY_ID`] — excluded from attributed sums and counted
//! separately, never silently absorbed.
//!
//! Paper-only accounting over already-recorded fills. No fee schedule or
//! route-economics input lives here.

use serde::{Deserialize, Serialize};

/// Strategy tag for fills that predate attribution (or arrive without an
/// owner). Quarantined: excluded from per-strategy sums, counted separately.
pub const UNATTRIBUTED_STRATEGY_ID: &str = "unattributed";

/// Canonical ledger identity for a signal strategy. `futures` keeps its
/// legacy `spatial_spread` lot namespace; `radar` gets its own namespace so
/// radar cycles no longer co-mingle with manual futures fills in the shared
/// spatial paper path. Funding/spot/prediction/stat_arb map to their stable
/// metric namespaces for future fill paths (those executors are currently
/// disabled and record no fills).
pub fn ledger_strategy_id(signal_strategy: &str) -> String {
    match signal_strategy.trim().to_lowercase().as_str() {
        "futures" => "spatial_spread".to_string(),
        "radar" => "radar".to_string(),
        "triangle" => "triangle".to_string(),
        "funding" => "funding_rate".to_string(),
        "spot" => "spot".to_string(),
        "prediction" => "prediction_parity".to_string(),
        "stat_arb" => "stat_arb".to_string(),
        "" => UNATTRIBUTED_STRATEGY_ID.to_string(),
        other => other.to_string(),
    }
}

/// Normalize an owning strategy id for a fill record. Unknown or blank
/// owners quarantine to [`UNATTRIBUTED_STRATEGY_ID`] instead of being
/// absorbed into another strategy's sums.
pub fn attribute_strategy_id(raw: &str) -> String {
    match crate::state_store::normalize_strategy_id(raw) {
        Ok(sid) => sid,
        Err(_) => UNATTRIBUTED_STRATEGY_ID.to_string(),
    }
}

/// One recorded leg fill: (is_short, qty, price, fee_usd).
/// Only fills with `qty > 0` and a positive finite price are valuable;
/// anything else is counted as unattributed volume, never priced.
#[derive(Debug, Clone, Copy)]
pub struct LegFill {
    pub is_short: bool,
    pub qty: f64,
    pub price: Option<f64>,
    pub fee_usd: f64,
}

/// Realized PnL of one closed signal family, derived purely from recorded
/// fills. Cash-flow convention: a short fill is cash in (`+px*qty`), a long
/// fill is cash out (`-px*qty`); every leg fee is subtracted. Returns `None`
/// when no fill in the family carries a usable price.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FamilyPnl {
    pub net_usd: f64,
    pub notional_usd: f64,
    pub bps: Option<f64>,
}

pub fn family_realized_pnl(fills: &[LegFill]) -> Option<FamilyPnl> {
    let mut cash = 0.0_f64;
    let mut fees = 0.0_f64;
    let mut notional = 0.0_f64;
    let mut priced = 0_usize;
    for f in fills {
        if !f.qty.is_finite() || f.qty <= 0.0 || !f.fee_usd.is_finite() || f.fee_usd < 0.0 {
            continue;
        }
        let Some(px) = f.price else { continue };
        if !px.is_finite() || px <= 0.0 {
            continue;
        }
        let leg_cash = if f.is_short { px * f.qty } else { -px * f.qty };
        cash += leg_cash;
        fees += f.fee_usd;
        notional += px * f.qty;
        priced += 1;
    }
    if priced == 0 {
        return None;
    }
    let net_usd = cash - fees;
    let bps = if notional > 0.0 {
        Some(net_usd / notional * 10_000.0)
    } else {
        None
    };
    Some(FamilyPnl {
        net_usd,
        notional_usd: notional,
        bps,
    })
}

/// Per-strategy attributed roll-up over closed signal families.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StrategyFillPnl {
    pub strategy_id: String,
    pub net_profit_usd: f64,
    pub net_profit_bps: Option<f64>,
    pub closed_families: usize,
    pub open_families: usize,
    /// Closed families that carry no usable strategy tag or no valuable
    /// fills. Excluded from the sums above — never absorbed.
    pub unattributed_closed_families: usize,
    /// Execution-log rows still tagged `"unattributed"` (pre-attribution
    /// history). Excluded from every attributed sum; counted here.
    pub unattributed_log_entries: usize,
}

/// Time-weighted average utilisation over a trailing window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeWeightedAvg {
    pub avg_rate_pct: f64,
    pub covered_ms: i64,
    pub window_ms: i64,
}

/// Step-function time-weighted average of `(ts_ms, rate_pct)` points over
/// `(now_ms - window_ms, now_ms]`. Each point's rate holds until the next
/// point; the last point extends to `now_ms`. Points outside the window are
/// clipped (the latest pre-window point seeds the window start). Returns
/// `None` when no point covers any part of the window.
pub fn time_weighted_avg(points: &[(i64, f64)], now_ms: i64, window_ms: i64) -> Option<TimeWeightedAvg> {
    if window_ms <= 0 || now_ms < 0 {
        return None;
    }
    let start = now_ms.saturating_sub(window_ms);
    let mut pts: Vec<(i64, f64)> = points
        .iter()
        .copied()
        .filter(|(ts, r)| *ts >= 0 && *ts <= now_ms && r.is_finite() && *r >= 0.0)
        .collect();
    if pts.is_empty() {
        return None;
    }
    pts.sort_by_key(|a| a.0);
    // Seed: latest point at or before window start, else earliest in-window.
    let mut cur_rate = None::<f64>;
    let mut cur_ts = start;
    for (ts, r) in &pts {
        if *ts <= start {
            cur_rate = Some(*r);
        }
    }
    let mut acc = 0.0_f64;
    let mut covered = 0_i64;
    for (ts, r) in &pts {
        if *ts <= start {
            continue;
        }
        let seg_start = cur_ts.max(start);
        let seg_end = (*ts).min(now_ms);
        if seg_end > seg_start {
            if let Some(rate) = cur_rate {
                acc += rate * (seg_end - seg_start) as f64;
                covered += seg_end - seg_start;
            }
            cur_ts = seg_end;
        }
        cur_rate = Some(*r);
        cur_ts = (*ts).max(cur_ts);
    }
    if now_ms > cur_ts {
        if let Some(rate) = cur_rate {
            acc += rate * (now_ms - cur_ts) as f64;
            covered += now_ms - cur_ts;
        }
    }
    if covered <= 0 {
        return None;
    }
    Some(TimeWeightedAvg {
        avg_rate_pct: acc / covered as f64,
        covered_ms: covered,
        window_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_mapping_covers_all_signal_paths() {
        assert_eq!(ledger_strategy_id("futures"), "spatial_spread");
        assert_eq!(ledger_strategy_id("radar"), "radar");
        assert_eq!(ledger_strategy_id("triangle"), "triangle");
        assert_eq!(ledger_strategy_id("funding"), "funding_rate");
        assert_eq!(ledger_strategy_id("spot"), "spot");
        assert_eq!(ledger_strategy_id("prediction"), "prediction_parity");
        assert_eq!(ledger_strategy_id("stat_arb"), "stat_arb");
        assert_eq!(ledger_strategy_id("  RADAR "), "radar");
        assert_eq!(ledger_strategy_id(""), UNATTRIBUTED_STRATEGY_ID);
    }

    #[test]
    fn unknown_owner_quarantines_to_unattributed() {
        assert_eq!(attribute_strategy_id("radar"), "radar");
        assert_eq!(attribute_strategy_id("nonsense"), UNATTRIBUTED_STRATEGY_ID);
        assert_eq!(attribute_strategy_id(""), UNATTRIBUTED_STRATEGY_ID);
    }

    #[test]
    fn family_pnl_round_trip_with_fees() {
        // Long 1.0 @ 60000, short 1.0 @ 60120; sell to close the long @
        // 60100 (+100), buy back the short @ 60050 (+70). Gross 170, fees
        // 5 + 5 = 10 -> net 160. Exit fills carry the closing trade side.
        let fills = vec![
            LegFill { is_short: false, qty: 1.0, price: Some(60000.0), fee_usd: 5.0 },
            LegFill { is_short: true, qty: 1.0, price: Some(60120.0), fee_usd: 0.0 },
            LegFill { is_short: true, qty: 1.0, price: Some(60100.0), fee_usd: 5.0 },
            LegFill { is_short: false, qty: 1.0, price: Some(60050.0), fee_usd: 0.0 },
        ];
        let pnl = family_realized_pnl(&fills).expect("priced fills");
        assert!((pnl.net_usd - 160.0).abs() < 1e-9);
        assert!(pnl.bps.is_some());
    }

    #[test]
    fn family_pnl_unpriced_fills_are_none() {
        let fills = vec![LegFill { is_short: false, qty: 1.0, price: None, fee_usd: 0.0 }];
        assert!(family_realized_pnl(&fills).is_none());
        assert!(family_realized_pnl(&[]).is_none());
    }

    #[test]
    fn tw_avg_weights_by_time() {
        // 10% for 1h, then 30% for 1h -> avg 20%.
        let h: i64 = 3_600_000;
        let now = 4 * h;
        let pts = vec![(now - 2 * h, 10.0), (now - h, 30.0)];
        let avg = time_weighted_avg(&pts, now, 2 * h).expect("covered");
        assert!((avg.avg_rate_pct - 20.0).abs() < 1e-9);
        assert_eq!(avg.covered_ms, 2 * h);
    }

    #[test]
    fn tw_avg_seeds_from_pre_window_point() {
        let h: i64 = 3_600_000;
        let now = 20 * h;
        let pts = vec![(now - 10 * h, 50.0)];
        let avg = time_weighted_avg(&pts, now, 2 * h).expect("seeded");
        assert!((avg.avg_rate_pct - 50.0).abs() < 1e-9);
        assert_eq!(avg.covered_ms, 2 * h);
    }

    #[test]
    fn tw_avg_empty_or_future_is_none() {
        assert!(time_weighted_avg(&[], 1000, 500).is_none());
        assert!(time_weighted_avg(&[(2000, 10.0)], 1000, 500).is_none());
        assert!(time_weighted_avg(&[(100, 10.0)], 1000, 0).is_none());
    }

    #[test]
    fn tw_avg_is_deterministic() {
        let pts = vec![(100, 12.5), (300, 7.25), (900, 40.0)];
        let a = time_weighted_avg(&pts, 1000, 1000);
        let b = time_weighted_avg(&pts, 1000, 1000);
        assert_eq!(a, b);
    }
}
