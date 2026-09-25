//! Tracked-instrument groups — instruments that track one economic
//! underlying without being identical to it (index perps vs ETF perps vs
//! ETF shares). Unlike [`canonical_index_alias`] (identity: `US100` IS
//! `NAS100`), a tracking relationship needs a fair-value conversion that
//! drifts with carry: QQQ reinvests dividends and pays an expense ratio,
//! so NDX/QQQ is a slowly-moving ratio, not a constant.
//!
//! The monitor converts member books into the group's numéraire units via
//! a ratio learned from live co-observations (never a fabricated constant),
//! which turns index↔ETF dislocations into ordinary spread rows — the same
//! "stablecoin depeg" pattern, applied on the base side instead of the
//! quote side.
//!
//! Carry modeling lives here too: per-asset-class annualized carry rates
//! (dividend yield, borrow, expense ratio) so every strategy's expected
//! profit can charge the right asset-class cost for the expected hold.

use std::collections::HashMap;

/// One tracked underlying — the group's numéraire units are the index's
/// own perp units, so index-member prices convert at scale 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackGroup {
    /// Nasdaq-100: index perps (NAS100 family) vs QQQ perps/shares.
    Ndx,
    /// S&P 500: SPX500 family vs SPY.
    Spx,
    /// Dow Jones Industrial: US30 family vs DIA.
    Dji,
    /// Russell 2000: IWM ETF vs any future RUT index perp.
    Rut,
    /// Gold: XAU spot/perp vs GLD.
    Xau,
    /// Silver: XAG spot/perp vs SLV.
    Xag,
    /// WTI crude: WTI perp vs USO.
    Wti,
}

impl TrackGroup {
    /// Canonical name used as the monitor watchlist/row key — reuses the
    /// index family's canonical alias where one exists so converted samples
    /// merge with the index lane's native series.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Ndx => "NAS100",
            Self::Spx => "SPX500",
            Self::Dji => "US30",
            Self::Rut => "RUT2000",
            Self::Xau => "XAU",
            Self::Xag => "XAG",
            Self::Wti => "WTI",
        }
    }

    /// Accepted mid-ratio band (numéraire units per member unit) for each
    /// non-numéraire member — observations outside are corrupt prints, not
    /// a ratio move, and never update the learner.
    /// (member canonical base) -> (min, max)
    pub fn ratio_band(&self, member_base: &str) -> Option<(f64, f64)> {
        match (self, member_base) {
            // NDX ~25k vs QQQ ~600 → ~40; band covers testnet premium prints.
            (Self::Ndx, "QQQ") => Some((15.0, 90.0)),
            // SPX ~6800 vs SPY ~680 → ~10
            (Self::Spx, "SPY") => Some((5.0, 20.0)),
            // DJI ~46k vs DIA ~460 → ~100
            (Self::Dji, "DIA") => Some((40.0, 160.0)),
            // RUT ~2500 vs IWM ~250 → ~10
            (Self::Rut, "IWM") => Some((4.0, 20.0)),
            // XAU ~4000 vs GLD ~370 → ~11
            (Self::Xau, "GLD") => Some((5.0, 25.0)),
            // XAG ~48 vs SLV ~44 → ~1.1
            (Self::Xag, "SLV") => Some((0.5, 3.0)),
            // WTI ~60 vs USO ~75 → ~0.8
            (Self::Wti, "USO") => Some((0.3, 2.5)),
            _ => None,
        }
    }
}

/// Instrument class drives the carry model: what a position in this asset
/// costs or earns per year on top of venue funding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AssetClass {
    /// Index perpetual — carry is venue funding only (already modeled).
    IndexPerp,
    /// ETF perpetual — venue funding only; dividends are priced into the
    /// perp by the venue (no direct dividend cash flow).
    EtfPerp,
    /// ETF shares (e.g. Alpaca equities) — long legs collect dividends and
    /// bear expense-ratio drag; short legs owe dividends + borrow + drag.
    EtfShares,
    /// Single-name equity shares — same carry shape as ETF shares, no
    /// expense ratio.
    EquityShares,
    /// Commodity/FX/other perp — funding only.
    CommodityPerp,
    /// Crypto or unclassified — no explicit asset carry.
    #[default]
    Other,
}

/// Annualized asset carry for one instrument, basis points per year.
/// Signed convention: positive = cost to a LONG position (it pays).
#[derive(Debug, Clone, Copy, Default)]
pub struct CarryProfile {
    /// Cash distributions: QQQ ~55, SPY ~120, DIA ~160, IWM ~120 bps/yr.
    /// Long legs receive (+income), short legs pay (+cost).
    pub dividend_yield_bps_pa: f64,
    /// Fund expense drag baked into ETF price performance. Affects both
    /// directions symmetrically only through tracking-ratio drift — the
    /// learned ratio already absorbs it, so it is reported for display but
    /// never double-charged into friction.
    pub expense_ratio_bps_pa: f64,
    /// Borrow fee a SHORT leg pays (equity/ETF shares only; perp legs
    /// express borrow through funding instead).
    pub borrow_bps_pa: f64,
    pub asset_class: AssetClass,
}

/// Carry profile for an instrument, spelled however the caller has it —
/// canonicalized internally so venue spellings ("QQQ-USD", "qqq") resolve.
/// Unknown instruments return `None` — callers must then fail closed or
/// treat the leg as funding-only, never assume zero dividend risk on a
/// short equity leg silently.
pub fn carry_profile(raw: &str) -> Option<CarryProfile> {
    let b = crate::symbol::canonical_base(raw);
    let prof = match b.as_str() {
        // ETFs on equity rails (Alpaca-style shares).
        "QQQ" => CarryProfile {
            dividend_yield_bps_pa: 55.0,
            expense_ratio_bps_pa: 20.0,
            borrow_bps_pa: 40.0,
            asset_class: AssetClass::EtfShares,
        },
        "SPY" => CarryProfile {
            dividend_yield_bps_pa: 120.0,
            expense_ratio_bps_pa: 9.0,
            borrow_bps_pa: 30.0,
            asset_class: AssetClass::EtfShares,
        },
        "DIA" => CarryProfile {
            dividend_yield_bps_pa: 160.0,
            expense_ratio_bps_pa: 16.0,
            borrow_bps_pa: 40.0,
            asset_class: AssetClass::EtfShares,
        },
        "IWM" => CarryProfile {
            dividend_yield_bps_pa: 120.0,
            expense_ratio_bps_pa: 19.0,
            borrow_bps_pa: 50.0,
            asset_class: AssetClass::EtfShares,
        },
        // Commodity ETFs: no distributions, expense drag only.
        "GLD" => CarryProfile {
            dividend_yield_bps_pa: 0.0,
            expense_ratio_bps_pa: 40.0,
            borrow_bps_pa: 30.0,
            asset_class: AssetClass::EtfShares,
        },
        "SLV" => CarryProfile {
            dividend_yield_bps_pa: 0.0,
            expense_ratio_bps_pa: 50.0,
            borrow_bps_pa: 40.0,
            asset_class: AssetClass::EtfShares,
        },
        "USO" => CarryProfile {
            dividend_yield_bps_pa: 0.0,
            expense_ratio_bps_pa: 75.0,
            borrow_bps_pa: 60.0,
            asset_class: AssetClass::EtfShares,
        },
        _ => return None,
    };
    Some(prof)
}

/// Map an instrument to its track group, if any — canonicalized internally
/// ("US100-USDC" and "us100" both resolve to the `NAS100` family).
/// Index-family members are the group's numéraire; every other member
/// converts via the learned ratio.
pub fn track_group(raw: &str) -> Option<TrackGroup> {
    let b = crate::symbol::canonical_base(raw);
    match b.as_str() {
        "NAS100" | "QQQ" => Some(TrackGroup::Ndx),
        "SPX500" | "SPY" => Some(TrackGroup::Spx),
        "US30" | "DIA" => Some(TrackGroup::Dji),
        "RUT2000" | "IWM" => Some(TrackGroup::Rut),
        "XAU" | "GLD" => Some(TrackGroup::Xau),
        "XAG" | "SLV" => Some(TrackGroup::Xag),
        "WTI" | "USO" => Some(TrackGroup::Wti),
        _ => None,
    }
}

/// Whether the instrument is its group's numéraire member — its prices
/// convert at scale 1.0 with no learned ratio required.
pub fn is_numeraire(raw: &str) -> bool {
    let b = crate::symbol::canonical_base(raw);
    match track_group(&b) {
        Some(g) => g.key() == b,
        None => false,
    }
}

/// Group membership across one venue's markets: canonical base of each
/// listed member keyed by group. A venue lists at most one instrument per
/// group lane — the numéraire member wins when several coexist (no
/// conversion needed beats learned conversion).
pub fn group_members(bases: impl Iterator<Item = String>) -> HashMap<TrackGroup, String> {
    let mut members: HashMap<TrackGroup, Vec<String>> = HashMap::new();
    for base in bases {
        let canon = crate::symbol::canonical_base(&base);
        if let Some(g) = track_group(&canon) {
            members.entry(g).or_default().push(canon);
        }
    }
    members
        .into_iter()
        .map(|(g, mut xs)| {
            xs.sort();
            let pick = xs
                .iter()
                .find(|b| is_numeraire(b))
                .cloned()
                .unwrap_or_else(|| xs[0].clone());
            (g, pick)
        })
        .collect()
}

/// Conservative short-side carry for any share on equity rails without a
/// curated profile — an unknown dividend/borrow is never assumed free.
/// 80 bps generic dividend + 120 bps generic borrow.
pub const GENERIC_SHORT_SHARE_BPS_PA: f64 = 200.0;

/// Signed annualized carry cost in bps/yr for holding one unit of this
/// instrument, per the leg direction. Conservative by construction:
/// dividend income is never credited (a feed error in our favor is not a
/// reason to trade), short-side costs always count.
///
/// `is_long`: the leg buys the instrument. Returns bps/yr; the caller
/// scales by hold fraction of a year.
pub fn leg_carry_bps_pa(raw: &str, is_long: bool, venue_is_equity_rails: bool) -> f64 {
    // On perp venues the instrument is a perp — venue funding already
    // carries borrow/dividend economics; charging the shares table would
    // double-count. The shares profile applies only on equity rails.
    if !venue_is_equity_rails {
        return 0.0;
    }
    match (carry_profile(raw), is_long) {
        // Long: pays expense drag (conservative), dividend income ignored.
        (Some(p), true) => p.expense_ratio_bps_pa,
        // Short: owes dividend + borrow + expense drag.
        (Some(p), false) => p.dividend_yield_bps_pa + p.borrow_bps_pa + p.expense_ratio_bps_pa,
        // Long shares without a profile carry no asset cost.
        (None, true) => 0.0,
        // Short shares without a profile: charge the generic floor rather
        // than assume the name is free to borrow and pays no dividend.
        (None, false) => GENERIC_SHORT_SHARE_BPS_PA,
    }
}

const MS_PER_YEAR: f64 = 365.25 * 24.0 * 3600.0 * 1000.0;

/// Carry cost in bps for a hold of `hold_secs`, summed over both legs.
/// Buy leg is long, sell leg is short. Perp legs contribute 0 here —
/// their carry is venue funding, already inside `funding_carry_bps`.
pub fn pair_carry_bps(
    buy_base: &str,
    sell_base: &str,
    buy_equity_rails: bool,
    sell_equity_rails: bool,
    hold_secs: f64,
) -> f64 {
    let frac = (hold_secs.max(0.0) * 1000.0) / MS_PER_YEAR;
    (leg_carry_bps_pa(buy_base, true, buy_equity_rails)
        + leg_carry_bps_pa(sell_base, false, sell_equity_rails))
        * frac
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_cover_index_etf_commodity() {
        // Venue spellings resolve through canonicalization.
        assert_eq!(track_group("US100-USDC").unwrap().key(), "NAS100");
        assert_eq!(track_group("QQQ").unwrap().key(), "NAS100");
        assert_eq!(track_group("SPY").unwrap().key(), "SPX500");
        assert_eq!(track_group("DIA").unwrap().key(), "US30");
        assert_eq!(track_group("GLD").unwrap().key(), "XAU");
        assert_eq!(track_group("SLV").unwrap().key(), "XAG");
        assert_eq!(track_group("USO").unwrap().key(), "WTI");
        assert!(track_group("BTC").is_none());
        assert!(track_group("AAPL").is_none());
    }

    #[test]
    fn numeraire_is_the_index_member() {
        assert!(is_numeraire("NAS100"));
        assert!(is_numeraire("SPX500"));
        assert!(!is_numeraire("QQQ"));
        assert!(!is_numeraire("SPY"));
        assert!(!is_numeraire("BTC"));
    }

    #[test]
    fn canonical_bases_resolve_before_grouping() {
        // Venue spellings canonicalize first: US100-USDC → NAS100.
        let members = group_members(
            ["US100-USDC", "QQQ-USD", "BTC-USDC"]
                .iter()
                .map(|s| s.to_string()),
        );
        assert_eq!(
            members.get(&TrackGroup::Ndx).map(String::as_str),
            Some("NAS100")
        );
        // A venue with only the ETF member still joins the group lane.
        let m2 = group_members(["QQQ-USD", "ETH-USD"].iter().map(|s| s.to_string()));
        assert_eq!(m2.get(&TrackGroup::Ndx).map(String::as_str), Some("QQQ"));
    }

    #[test]
    fn ratio_bands_bound_corrupt_prints() {
        assert!(TrackGroup::Ndx.ratio_band("QQQ").unwrap().0 < 40.0);
        assert!(TrackGroup::Ndx.ratio_band("QQQ").unwrap().1 > 40.0);
        assert!(TrackGroup::Xau.ratio_band("GLD").is_some());
        assert!(TrackGroup::Ndx.ratio_band("SPY").is_none());
    }

    #[test]
    fn short_etf_leg_charges_dividend_borrow_expense() {
        // Short QQQ on equity rails owes ~115 bps/yr all-in (55 div + 40
        // borrow + 20 expense drag).
        let short = leg_carry_bps_pa("QQQ", false, true);
        assert!((short - 115.0).abs() < 1e-9, "short carry {short}");
        // Long leg: expense drag only — dividend income never credited.
        let long = leg_carry_bps_pa("QQQ", true, true);
        assert!((long - 20.0).abs() < 1e-9, "long carry {long}");
        // Same instrument on a perp venue: funding carries it, shares
        // table must not double-charge.
        assert_eq!(leg_carry_bps_pa("QQQ", false, false), 0.0);
        // An unprofiled share shorted on equity rails pays the generic
        // floor — unknown dividend/borrow is never free.
        assert_eq!(
            leg_carry_bps_pa("ZZZZ", false, true),
            GENERIC_SHORT_SHARE_BPS_PA
        );
        assert_eq!(leg_carry_bps_pa("ZZZZ", true, true), 0.0);
    }

    #[test]
    fn pair_carry_scales_with_hold_fraction() {
        // One-day hold, ETF short on equity rails: ~0.31 bps.
        let c = pair_carry_bps("NAS100", "QQQ", false, true, 86_400.0);
        let expected = 115.0 / 365.25;
        assert!((c - expected).abs() < 1e-6, "carry {c} vs {expected}");
        // Perp-vs-perp legs pay no asset carry (funding handles it).
        assert_eq!(pair_carry_bps("NAS100", "QQQ", false, false, 86_400.0), 0.0);
    }
}
