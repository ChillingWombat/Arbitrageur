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
/// - S&P 500 → `SPX500` (`US500`, `SP500`, `SPX`, …)
/// - Dow Jones → `US30` (`DJ30`, `WS30`, `DJI`, …)
pub fn canonical_index_alias(base: &str) -> Option<&'static str> {
    match base {
        "US100" | "TECH100" | "NAS100" | "USTEC" | "USTEC100" | "NQ100" | "NQ" => Some("NAS100"),
        "US500" | "SPX500" | "SP500" | "SPX" => Some("SPX500"),
        "US30" | "DJ30" | "WS30" | "DJI" | "DJIA" => Some("US30"),
        _ => None,
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
