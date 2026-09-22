//! Canonical symbol normalization shared by every layer that pairs,
//! groups, or caches market data across venues. One implementation so the
//! market-data hub, monitor lanes, spread-sample store, and execution
//! readers can never drift into different spellings again.

/// Venue-name aliases for the same economic underlying (cash equity indices
/// traded as perps under different tickers). Returns the canonical family
/// name, or `None` when the base is not a known index alias.
///
/// Families (identity is unambiguous — same index, same currency):
/// - Nasdaq-100 → `NAS100` (Lighter `US100`, PopDEX `TECH100`, …)
/// - S&P 500 → `SPX500` (`US500`, `SP500`, …)
/// - Dow Jones → `US30` (`DJ30`, `WS30`, `DJIA`, …)
///
/// Deliberately NOT aliased: bare `SPX`, `NQ`, `DJI`. Those spellings are
/// live crypto tickers on the same venues (e.g. `SPX` = the SPX6900
/// memecoin, ~$0.5), so fusing them into an index family corrupts every
/// downstream pairing — observed in production as index/memecoin mids
/// interleaved under one `SPX500` key.
pub fn canonical_index_alias(base: &str) -> Option<&'static str> {
    match base {
        "US100" | "TECH100" | "NAS100" | "USTEC" | "USTEC100" | "NQ100" => Some("NAS100"),
        // NB: only unambiguous index spellings merge. Short forms that are
        // also live crypto tickers must NOT alias — "SPX" is the SPX6900
        // memecoin on crypto venues (~$0.5) while "SPX500"/"US500" is the
        // S&P index perp (~$7.6k); fusing them corrupts every downstream
        // pairing, chart, and baseline. Same reasoning for "NQ"/"DJI".
        "US500" | "SPX500" | "SP500" => Some("SPX500"),
        "US30" | "DJ30" | "WS30" | "DJIA" => Some("US30"),
        _ => None,
    }
}

/// Every venue spelling that canonicalizes into an index family — the
/// unambiguous members only, kept in lockstep with [`canonical_index_alias`].
/// Storage reads use this to merge history recorded under sibling spellings
/// (e.g. `US500` rows into a `SPX500` read); ambiguous tickers like bare
/// `SPX` are family members of nothing and never merge.
pub fn index_alias_siblings(canonical: &str) -> &'static [&'static str] {
    match canonical {
        "NAS100" => &["US100", "TECH100", "NAS100", "USTEC", "USTEC100", "NQ100"],
        "SPX500" => &["US500", "SPX500", "SP500"],
        "US30" => &["US30", "DJ30", "WS30", "DJIA"],
        _ => &[],
    }
}

/// Canonical base asset for cross-venue pairing: uppercase, quote suffixes
/// stripped ("BTC-USD"/"BTC-USDC"/"BTCUSDT"/"btc" -> "BTC").
///
/// Separator-suffixed quotes ("-USDC" etc.) strip freely; *bare* quote
/// suffixes (Binance-style "BTCUSD") only strip when at least 3 chars of
/// base remain — otherwise real tickers like "TUSD"/"SUSD"/"BUSD" would
/// collapse to a single letter. Instrument-class markers like `-SPOT`
/// are not stripped, keeping spot books distinct from perp books.
///
/// After stripping, equivalent cash-index underlyings merge via
/// [`canonical_index_alias`]: Lighter `US100` and PopDEX `TECH100` are both
/// Nasdaq-100 index perps, so they pair, group, and net together instead of
/// appearing as two unrelated tickers.
pub fn canonical_base(raw: &str) -> String {
    let up = raw.trim().to_uppercase();
    // Strip instrument-class and quote suffixes until stable so compound
    // venue spellings (`BTC-USDT-SWAP`, `BTC-USD-PERP`) fully reduce.
    let mut stripped = up.as_str();
    let mut cut = false;
    loop {
        let mut again = false;
        for suf in [
            "-USDC", "-USDT", "-USD1", "-USD", "-PERP", "-SWAP", "-U", "/USDC", "/USDT", "/USD",
        ] {
            if stripped.len() > suf.len() && stripped.ends_with(suf) {
                stripped = &stripped[..stripped.len() - suf.len()];
                cut = true;
                again = true;
                break;
            }
        }
        if !again {
            break;
        }
    }
    if !cut {
        for suf in ["USDC", "USDT", "USD"] {
            if up.ends_with(suf) && up.len() - suf.len() >= 3 {
                stripped = &up[..up.len() - suf.len()];
                break;
            }
        }
    }
    match canonical_index_alias(stripped) {
        Some(alias) => alias.to_string(),
        None => stripped.to_string(),
    }
}
