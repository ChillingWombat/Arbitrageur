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

/// Instrument-class suffixes: `-PERP`/`-SWAP`/`_PERP` reduce away
/// entirely while `-SPOT`/`_SPOT` survives on the canonical base so spot
/// books stay distinct from perp books. Shared by [`canonical_base`] and
/// [`symbol_quote`]. The `_` variants cover Orderly/Gate-style spellings
/// (`PERP_BTC_USDC`, `BTC_USDT_PERP`).
const CLASS_SUFFIXES: &[&str] = &[
    "-PERPETUAL",
    "-PERP",
    "-SWAP",
    "-SPOT",
    "_PERPETUAL",
    "_PERP",
    "_SWAP",
    "_SPOT",
];

/// Separated USD-family quote suffixes ("-USDT", "/USDC", "_USDT", …),
/// covering every member of [`USD_QUOTE_FAMILY`]. Shared by
/// [`canonical_base`] (strips them) and [`symbol_quote`] (reads them) so
/// the two can never disagree about whether a suffix is a quote. The
/// leading `-`/`/`/`_` anchors every match, so ordering is for
/// readability only.
const SEPARATED_USD_SUFFIXES: &[&str] = &[
    "-FDUSD", "-PYUSD", "-USDT", "-USDC", "-USD1", "-USDG", "-USDE", "-TUSD", "-USDS", "-USDP",
    "-BUSD", "-DAI", "-USD", "-U", "/FDUSD", "/USDT", "/USDC", "/DAI", "/USD", "_FDUSD", "_PYUSD",
    "_USDT", "_USDC", "_USD1", "_USDG", "_USDE", "_TUSD", "_USDS", "_USDP", "_BUSD", "_DAI",
    "_USD",
];

/// Bare (Binance-style) USD-family quote suffixes, longest first so e.g.
/// "BTCFDUSD" reads `FDUSD`, not `USD`+tail. Shared by both parsers with
/// the same >=3-char base guard so stablecoin tickers like "TUSD"/"SUSD"
/// never collapse into a quote read. Aster's "-U" quote is deliberately
/// separated-only — a bare "U" tail would eat real tickers.
const BARE_USD_SUFFIXES: &[&str] = &[
    "FDUSD", "PYUSD", "USDT", "USDC", "USDS", "USDP", "BUSD", "USDG", "USDE", "TUSD", "DAI", "USD",
];

/// Canonical base asset for cross-venue pairing: uppercase, quote suffixes
/// stripped ("BTC-USD"/"BTC-USDC"/"BTCUSDT"/"btc" -> "BTC").
///
/// Separator-suffixed quotes ("-USDC" etc.) strip freely; *bare* quote
/// suffixes (Binance-style "BTCUSD") only strip when at least 3 chars of
/// base remain — otherwise real tickers like "TUSD"/"SUSD"/"BUSD" would
/// collapse to a single letter. Instrument-class markers like `-SPOT`
/// are not stripped, keeping spot books distinct from perp books — though
/// a quote inside a spot spelling still reduces ("BTC-USDT-SPOT" and
/// "BTCUSDT-SPOT" both canonicalize to "BTC-SPOT").
///
/// After stripping, equivalent cash-index underlyings merge via
/// [`canonical_index_alias`]: Lighter `US100` and PopDEX `TECH100` are both
/// Nasdaq-100 index perps, so they pair, group, and net together instead of
/// appearing as two unrelated tickers.
pub fn canonical_base(raw: &str) -> String {
    let raw_up = raw.trim().to_uppercase();
    // Orderly-family class prefix: `PERP_<BASE>_<USD quote>` (e.g.
    // `PERP_ADA_USDC`) is the same market as `<BASE>` — the leading
    // `PERP_` is an instrument-class marker, not part of the base.
    // Require a second underscore inside the remainder and a USD-family
    // quote tail so `PERP_USDC` (a PERP-token pair) and `PERP_USDT_PERP`
    // (class-suffixed PERP token) are untouched. Rewriting to the
    // `-`-separated spelling lets the normal suffix loop below reduce
    // the quote (`PERP_ADA_USDC` -> `ADA-USDC` -> `ADA`).
    let up = match raw_up.strip_prefix("PERP_") {
        Some(rest)
            if rest.contains('_') && SEPARATED_USD_SUFFIXES.iter().any(|s| rest.ends_with(s)) =>
        {
            rest.replace('_', "-")
        }
        _ => raw_up,
    };
    // Strip instrument-class and quote suffixes until stable so compound
    // venue spellings (`BTC-USDT-SWAP`, `BTC-USD-PERP`) fully reduce.
    let mut stripped = up.as_str();
    let mut quote_cut = false;
    let mut spot = false;
    loop {
        let mut again = false;
        for suf in CLASS_SUFFIXES {
            if stripped.len() > suf.len() && stripped.ends_with(suf) {
                stripped = &stripped[..stripped.len() - suf.len()];
                spot |= suf.ends_with("SPOT");
                again = true;
                break;
            }
        }
        if !again {
            for suf in SEPARATED_USD_SUFFIXES {
                if stripped.len() > suf.len() && stripped.ends_with(suf) {
                    stripped = &stripped[..stripped.len() - suf.len()];
                    quote_cut = true;
                    again = true;
                    break;
                }
            }
        }
        if !again {
            break;
        }
    }
    // Bare (Binance-style) quote suffixes apply even after a class strip —
    // "BTCUSDT-PERP" is still a USDT-quoted perp — but never after an
    // explicit quote was already stripped, and never on tickers like
    // "TUSD" where the base guard leaves <3 chars.
    if !quote_cut {
        for suf in BARE_USD_SUFFIXES {
            if stripped.ends_with(suf) && stripped.len() - suf.len() >= 3 {
                stripped = &stripped[..stripped.len() - suf.len()];
                break;
            }
        }
    }
    let base = match canonical_index_alias(stripped) {
        Some(alias) => alias.to_string(),
        None => stripped.to_string(),
    };
    if spot {
        format!("{base}-SPOT")
    } else {
        base
    }
}

/// Quote tickers treated as members of the USD fiat/stablecoin family.
/// These quotes merge under the "parity"/"fx_adjusted" USD-quote modes and
/// stay separate instruments under "separate". Ordered by preference: when
/// a venue lists one base under several family quotes, earlier entries win
/// as the lane's assumed quote.
pub const USD_QUOTE_FAMILY: &[&str] = &[
    "USD", "USDT", "USDC", "USDG", "USD1", "U", "FDUSD", "USDE", "DAI", "TUSD", "PYUSD", "USDS",
    "USDP", "BUSD",
];

/// Whether `quote` is a USD fiat/stablecoin quote ticker.
pub fn is_usd_family(quote: &str) -> bool {
    let q = quote.trim().to_uppercase();
    USD_QUOTE_FAMILY.contains(&q.as_str())
}

/// Extract the quote currency from a venue/canonical symbol spelling —
/// the inverse of [`canonical_base`]. Returns `None` for bare bases and
/// pure instrument-class spellings where the quote is not written into the
/// symbol ("BTC", "BTC-PERP", "BTC-SPOT").
///
/// - "BTC-USDT" -> "USDT", "BTC-USD-PERP" -> "USD", "BTC/USDC" -> "USDC"
/// - "BTCUSDT" -> "USDT" (same >=3-char base guard as `canonical_base`)
/// - "BTC-PERP"/"BTC-SPOT"/"BTC" -> None (quote is venue-implied)
pub fn symbol_quote(raw: &str) -> Option<String> {
    let up = raw.trim().to_uppercase();
    let mut s = up.as_str();
    loop {
        let mut progressed = false;
        for suf in CLASS_SUFFIXES {
            if s.len() > suf.len() && s.ends_with(suf) {
                s = &s[..s.len() - suf.len()];
                progressed = true;
                break;
            }
        }
        if progressed {
            continue;
        }
        for suf in SEPARATED_USD_SUFFIXES {
            if s.len() > suf.len() && s.ends_with(suf) {
                return Some(suf.trim_start_matches(['-', '/', '_']).to_string());
            }
        }
        break;
    }
    // Bare-suffix pass runs on the class-stripped spelling so compound
    // forms like "BTCUSDT-PERP" still resolve their quote.
    for suf in BARE_USD_SUFFIXES {
        if s.ends_with(suf) && s.len() - suf.len() >= 3 {
            return Some(suf.to_string());
        }
    }
    None
}

/// Grouping key for cross-venue pairing under the configured USD-quote
/// policy. When `merge_usd_quotes` is false ("separate" mode) a USD-family
/// quote stays part of the identity — `BTC-USDT` and `BTC-USDC` are
/// distinct instruments. Non-USD quotes are never merged in either mode:
/// `BTC-EUR` keeps its quote inside `canonical_base` already.
pub fn pair_key(raw: &str, merge_usd_quotes: bool) -> String {
    if merge_usd_quotes {
        return canonical_base(raw);
    }
    match symbol_quote(raw) {
        Some(q) if is_usd_family(&q) => format!("{}-{}", canonical_base(raw), q),
        _ => canonical_base(raw),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_quote_reads_separated_forms() {
        assert_eq!(symbol_quote("BTC-USDT").as_deref(), Some("USDT"));
        assert_eq!(symbol_quote("BTC-USDC").as_deref(), Some("USDC"));
        assert_eq!(symbol_quote("BTC-USD-PERP").as_deref(), Some("USD"));
        assert_eq!(symbol_quote("BTC-USDT-SWAP").as_deref(), Some("USDT"));
        assert_eq!(symbol_quote("eth/usd").as_deref(), Some("USD"));
        assert_eq!(symbol_quote("BTC-USD1").as_deref(), Some("USD1"));
        assert_eq!(symbol_quote("BTC-U").as_deref(), Some("U"));
    }

    #[test]
    fn symbol_quote_reads_bare_forms_with_base_guard() {
        assert_eq!(symbol_quote("BTCUSDT").as_deref(), Some("USDT"));
        assert_eq!(symbol_quote("ETHUSDC").as_deref(), Some("USDC"));
        assert_eq!(symbol_quote("BTCUSD").as_deref(), Some("USD"));
        // Stablecoin tickers themselves must not collapse into a quote.
        assert_eq!(symbol_quote("TUSD"), None);
        assert_eq!(symbol_quote("SUSD"), None);
        assert_eq!(symbol_quote("USDT"), None);
    }

    #[test]
    fn symbol_quote_is_none_for_bare_or_class_only() {
        assert_eq!(symbol_quote("BTC"), None);
        assert_eq!(symbol_quote("BTC-PERP"), None);
        assert_eq!(symbol_quote("BTC-SPOT"), None);
    }

    #[test]
    fn compound_bare_quote_plus_class_suffix_reduces() {
        // "BTCUSDT-PERP" style: the class suffix strips first, then the
        // bare quote must still resolve — these are the same instruments
        // as their separated spellings.
        assert_eq!(symbol_quote("BTCUSDT-PERP").as_deref(), Some("USDT"));
        assert_eq!(symbol_quote("BTCUSD-SWAP").as_deref(), Some("USD"));
        assert_eq!(canonical_base("BTCUSDT-PERP"), "BTC");
        assert_eq!(canonical_base("BTCUSD-SWAP"), "BTC");
        assert_eq!(pair_key("BTCUSDT-PERP", false), "BTC-USDT");
        assert_eq!(pair_key("BTCUSDT-PERP", true), "BTC");
        // The stablecoin ticker itself never collapses.
        assert_eq!(canonical_base("TUSD"), "TUSD");
        assert_eq!(symbol_quote("TUSD-PERP"), None);
    }

    #[test]
    fn orderly_perp_prefix_reduces_to_base() {
        // Orderly/Vanta venue-native `PERP_<BASE>_<USD quote>` spelling is
        // the same market as the canonical base — the prefix is an
        // instrument-class marker, not part of the base.
        assert_eq!(canonical_base("PERP_ADA_USDC"), "ADA");
        assert_eq!(canonical_base("PERP_BTC_USDC"), "BTC");
        assert_eq!(canonical_base("PERP_1000PEPE_USDC"), "1000PEPE");
        assert_eq!(canonical_base("PERP_ETH_USDT"), "ETH");
        // The PERP token itself survives: `PERP_USDC` is a spot-style
        // PERP/USDC pair, `PERP_USDT_PERP` a class-suffixed PERP token —
        // both reduce to the PERP base, never to `USDC`/`USDT`.
        assert_eq!(canonical_base("PERP_USDC"), "PERP");
        assert_eq!(canonical_base("PERP_USDT_PERP"), "PERP");
        // Orderly spelling of the PERP token's own perp reduces too.
        assert_eq!(canonical_base("PERP_PERP_USDC"), "PERP");
        // Plain underscore pair spellings (Gate-style) reduce as well.
        assert_eq!(canonical_base("BTC_USDT"), "BTC");
        assert_eq!(canonical_base("ETH_USDT_PERP"), "ETH");
        // Quote still reads the USD tail.
        assert_eq!(symbol_quote("PERP_ADA_USDC").as_deref(), Some("USDC"));
        assert_eq!(symbol_quote("BTC_USDT").as_deref(), Some("USDT"));
    }

    #[test]
    fn usd_family_membership() {
        assert!(is_usd_family("usdt"));
        assert!(is_usd_family("USD"));
        assert!(is_usd_family("usdg"));
        assert!(!is_usd_family("EUR"));
        assert!(!is_usd_family("BTC"));
        assert!(!is_usd_family("PERP"));
    }

    #[test]
    fn pair_key_merges_usd_quotes_when_enabled() {
        assert_eq!(pair_key("BTC-USDT", true), "BTC");
        assert_eq!(pair_key("BTC-USDC", true), "BTC");
        assert_eq!(pair_key("BTC-USD-PERP", true), "BTC");
        // Non-USD quotes are never merged away.
        assert_eq!(pair_key("BTC-EUR", true), "BTC-EUR");
    }

    #[test]
    fn pair_key_separates_usd_quotes_when_disabled() {
        assert_eq!(pair_key("BTC-USDT", false), "BTC-USDT");
        assert_eq!(pair_key("BTC-USDC", false), "BTC-USDC");
        assert_eq!(pair_key("BTC-USD-PERP", false), "BTC-USD");
        // Bare bases get no invented quote.
        assert_eq!(pair_key("BTC", false), "BTC");
        // Non-USD quotes unchanged either way.
        assert_eq!(pair_key("BTC-EUR", false), "BTC-EUR");
    }

    #[test]
    fn whole_usd_family_reduces_consistently() {
        // Every family quote must strip from canonical_base AND read back
        // via symbol_quote — a suffix known to one parser but not the
        // other produced corrupted keys ("BTC-FDUSD" -> "BTC-FD") or
        // double-appended pair keys ("BTC-USDG-USDG").
        for q in USD_QUOTE_FAMILY {
            let sep = format!("BTC-{q}");
            assert_eq!(
                canonical_base(&sep),
                "BTC",
                "separated quote -{q} must strip"
            );
            assert_eq!(symbol_quote(&sep).as_deref(), Some(*q), "-{q} must read");
            assert_eq!(pair_key(&sep, false), sep);
            assert_eq!(pair_key(&sep, true), "BTC");
        }
        // Bare forms: base guard keeps >=3 chars, so a bare spelling
        // resolves identically to its separated form. "U" and "USD1" are
        // separated-only spellings — no bare pass.
        for q in [
            "FDUSD", "TUSD", "PYUSD", "USDS", "USDP", "BUSD", "USDG", "USDE", "DAI",
        ] {
            let bare = format!("BTC{q}");
            assert_eq!(canonical_base(&bare), "BTC", "bare {q} must strip");
            assert_eq!(symbol_quote(&bare).as_deref(), Some(q));
            assert_eq!(pair_key(&bare, false), format!("BTC-{q}"));
            assert_eq!(pair_key(&bare, true), "BTC");
        }
        // The stablecoin tickers themselves never reduce.
        for t in ["TUSD", "SUSD", "FDUSD", "PYUSD", "USDS", "BUSD"] {
            assert_eq!(canonical_base(t), t);
            assert_eq!(symbol_quote(t), None);
        }
    }

    #[test]
    fn separated_and_bare_spellings_share_keys() {
        // The corruption this fixes: "BTC-FDUSD" used to canonicalize to
        // "BTC-FD" while "BTCFDUSD" became "BTCFD" — the same book keyed
        // two ways.
        assert_eq!(canonical_base("BTC-FDUSD"), canonical_base("BTCFDUSD"));
        assert_eq!(pair_key("BTC-FDUSD", false), pair_key("BTCFDUSD", false));
        assert_eq!(pair_key("BTC-TUSD", false), pair_key("BTCTUSD", false));
    }

    #[test]
    fn spot_spelling_reduces_quote_but_keeps_class() {
        // Spot books keep their -SPOT marker but unify across quote
        // spellings the same way perps do.
        assert_eq!(canonical_base("BTC-USDT-SPOT"), "BTC-SPOT");
        assert_eq!(canonical_base("BTCUSDT-SPOT"), "BTC-SPOT");
        assert_eq!(canonical_base("ETH-SPOT"), "ETH-SPOT");
        assert_eq!(symbol_quote("BTC-USDT-SPOT").as_deref(), Some("USDT"));
        assert_eq!(symbol_quote("ETH-SPOT"), None);
        assert_eq!(pair_key("BTC-USDT-SPOT", false), "BTC-SPOT-USDT");
        assert_eq!(pair_key("BTCUSDT-SPOT", false), "BTC-SPOT-USDT");
        assert_eq!(pair_key("ETH-SPOT", false), "ETH-SPOT");
    }
}
