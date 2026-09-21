use std::collections::HashMap;
use std::str::FromStr;

use arbitrageur_core::quote::{BookLevel, OrderBook};
use arbitrageur_core::types::{Decimal, VenueId};

use crate::hyperliquid::append_hyperliquid_levels;
use crate::lighter::{append_lighter_levels, parse_lighter_price_size};

/// Single Hyperliquid WS endpoint; one connection multiplexes every coin.
pub const HYPERLIQUID_WS_URL: &str = "wss://api.hyperliquid.xyz/ws";

/// Orderly EVM WS host backing the Vanta feed.
pub const VANTA_WS_HOST: &str = "ws-evm.orderly.org";

/// How often a Lighter WS lane must emit a client frame (spec: 2 min).
pub const LIGHTER_KEEPALIVE_SECS: u64 = 110;

/// Exponential reconnect backoff in seconds, mirroring the monitor lane
/// backoff: 2, 4, 8, ... capped at 120.
pub fn reconnect_backoff_secs(fail_streak: u32) -> u64 {
    (1u64 << fail_streak.min(8)).min(120)
}

/// Jitter ratio bounds shared by all ±20% cadence jitter.
pub const JITTER_MIN_RATIO: f64 = 0.8;
pub const JITTER_MAX_RATIO: f64 = 1.2;

/// Map a caller-supplied uniform sample in [0,1] to a ±20% ratio.
/// Pure and deterministic so cadence tests can assert exact bounds.
pub fn jitter_ratio_20(sample01: f64) -> f64 {
    const SPAN: f64 = JITTER_MAX_RATIO - JITTER_MIN_RATIO;
    JITTER_MIN_RATIO + sample01.clamp(0.0, 1.0) * SPAN
}

/// Scale `base` by `ratio`, clamped to ±20% and floored at 1ms so a
/// zero base never becomes a busy loop.
pub fn apply_jitter_ratio(base: std::time::Duration, ratio: f64) -> std::time::Duration {
    let scaled = base.as_secs_f64() * ratio.clamp(JITTER_MIN_RATIO, JITTER_MAX_RATIO);
    std::time::Duration::from_secs_f64(scaled.max(0.001))
}

/// Jitter `base` by ±20% using a thread-local RNG. Same-IP request
/// shape is unchanged — only the timing spreads out.
pub fn jittered_duration_20(base: std::time::Duration) -> std::time::Duration {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    apply_jitter_ratio(base, jitter_ratio_20(rng.gen_range(0.0..=1.0)))
}

/// Jittered reconnect backoff: base table value ±20%, floored at 1s.
pub fn jittered_reconnect_backoff_secs(fail_streak: u32) -> u64 {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let base = reconnect_backoff_secs(fail_streak) as f64;
    (base * jitter_ratio_20(rng.gen_range(0.0..=1.0))).round() as u64
}

fn dec_from_value(v: Option<&serde_json::Value>) -> Option<Decimal> {
    match v? {
        serde_json::Value::String(s) => Decimal::from_str(s).ok(),
        serde_json::Value::Number(n) => Decimal::from_str(&n.to_string()).ok(),
        _ => None,
    }
}

fn u64_from_value(v: Option<&serde_json::Value>) -> Option<u64> {
    match v? {
        serde_json::Value::Number(n) => n.as_u64(),
        serde_json::Value::String(s) => s.parse::<u64>().ok(),
        _ => None,
    }
}

/* ------------------------------------------------------------------------ */
/* Hyperliquid                                                               */
/* ------------------------------------------------------------------------ */

/// `{"method":"subscribe","subscription":{"type":"l2Book","coin":"BTC"}}`.
pub fn hyperliquid_subscribe_message(coin: &str) -> serde_json::Value {
    serde_json::json!({
        "method": "subscribe",
        "subscription": { "type": "l2Book", "coin": coin },
    })
}

/// Parse the `data` payload of a `{"channel":"l2Book","data":{...}}` frame.
/// The WS `levels` layout matches the REST `l2Book` shape, so the shared
/// adapter parser is reused; the coin must equal the subscribed coin.
pub fn parse_hyperliquid_l2book_data(
    venue: &VenueId,
    symbol: &str,
    expected_coin: &str,
    data: &serde_json::Value,
) -> anyhow::Result<OrderBook> {
    let coin = data
        .get("coin")
        .and_then(|c| c.as_str())
        .ok_or_else(|| anyhow::anyhow!("Hyperliquid WS l2Book frame missing coin"))?;
    if coin != expected_coin {
        anyhow::bail!(
            "Hyperliquid WS l2Book coin '{coin}' does not match subscribed '{expected_coin}'"
        );
    }
    let mut book = OrderBook::new(venue.clone(), symbol.to_string());
    match data.get("levels") {
        Some(levels) => append_hyperliquid_levels(&mut book, levels),
        None => anyhow::bail!("Hyperliquid WS l2Book frame missing levels"),
    }
    book.timestamp = chrono::Utc::now();
    Ok(book)
}

/* ------------------------------------------------------------------------ */
/* Lighter (+ Robinhood-Lighter, same protocol)                              */
/* ------------------------------------------------------------------------ */

/// Derive `wss://<host>/stream` from an adapter `base_url` by scheme swap.
/// Fails closed on a missing/unexpected scheme or an empty host.
pub fn lighter_ws_url(base_url: &str) -> anyhow::Result<String> {
    let base = base_url.trim().trim_end_matches('/');
    let (secure, rest) = match base
        .strip_prefix("https://")
        .map(|r| (true, r))
        .or_else(|| base.strip_prefix("http://").map(|r| (false, r)))
    {
        Some(v) => v,
        None => anyhow::bail!(
            "Lighter base_url '{base_url}' has no http(s) scheme; refusing to derive a WS URL"
        ),
    };
    if rest.is_empty() {
        anyhow::bail!("Lighter base_url '{base_url}' has an empty host");
    }
    Ok(format!(
        "{}://{rest}/stream",
        if secure { "wss" } else { "ws" }
    ))
}

/// `{"type":"subscribe","channel":"order_book/{market_id}"}`.
/// Resubscribing with the same frame refreshes the snapshot after a gap.
pub fn lighter_subscribe_message(market_id: u64) -> serde_json::Value {
    serde_json::json!({
        "type": "subscribe",
        "channel": format!("order_book/{market_id}"),
    })
}

/// Build the `market_id -> canonical-base symbol` map from an
/// `orderBookDetails` payload (the WS cold-start REST call). Only perp
/// entries resolve; spot, unknown, empty, or ambiguous rows are skipped so
/// no symbol ever aliases onto the wrong market.
pub fn lighter_market_symbol_map(details: &serde_json::Value) -> HashMap<u64, String> {
    let mut out = HashMap::new();
    let Some(entries) = details.get("order_book_details").and_then(|d| d.as_array()) else {
        return out;
    };
    let mut seen: HashMap<String, u64> = HashMap::new();
    for e in entries {
        let is_perp = e
            .get("market_type")
            .and_then(|t| t.as_str())
            .map(|t| t.eq_ignore_ascii_case("perp"))
            .unwrap_or(false);
        if !is_perp {
            continue;
        }
        let (Some(sym), Some(mid)) = (
            e.get("symbol").and_then(|s| s.as_str()),
            e.get("market_id").and_then(|m| m.as_u64()),
        ) else {
            continue;
        };
        let base = sym.trim().to_uppercase();
        if base.is_empty() {
            continue;
        }
        match seen.get(&base) {
            Some(_) => {
                out.remove(&mid);
                seen.insert(base, u64::MAX);
            }
            None => {
                seen.insert(base.clone(), mid);
                out.insert(mid, base);
            }
        }
    }
    out.retain(|_, base| seen.get(base).is_some_and(|m| *m != u64::MAX));
    out
}

/// Fallback market ID to canonical symbol map for Lighter protocol when
/// the cold-start orderBookDetails HTTP call is unavailable or rate-limited.
pub fn lighter_default_market_symbol_map() -> HashMap<u64, String> {
    let mut out = HashMap::new();
    let defaults = [
        (0, "BTC"),
        (1, "ETH"),
        (2, "SOL"),
        (3, "DOGE"),
        (4, "AVAX"),
        (5, "LINK"),
        (6, "ARB"),
        (7, "OP"),
        (8, "SUI"),
        (9, "APT"),
        (10, "PEPE"),
        (11, "WIF"),
    ];
    for (id, sym) in defaults {
        out.insert(id, sym.to_string());
    }
    out
}

/// Parse a snapshot payload (subscribe ack) into a validated-shape book plus
/// its nonce. Level parsing reuses the REST adapter parser.
pub fn parse_lighter_snapshot_book(
    venue: &VenueId,
    symbol: &str,
    payload: &serde_json::Value,
) -> anyhow::Result<(OrderBook, u64)> {
    let nonce = u64_from_value(payload.get("nonce"))
        .ok_or_else(|| anyhow::anyhow!("Lighter WS snapshot missing nonce"))?;
    let mut book = OrderBook::new(venue.clone(), symbol.to_string());
    append_lighter_levels(&mut book, payload);
    book.timestamp = chrono::Utc::now();
    Ok((book, nonce))
}

/// One `type:"update/order_book"` delta. Levels accept both REST-shaped
/// objects and `[price, size]` pairs, with string or numeric scalars.
#[derive(Debug, Clone)]
pub struct LighterBookDelta {
    pub market_id: u64,
    pub nonce: u64,
    pub begin_nonce: u64,
    pub bids: Vec<(Decimal, Decimal)>,
    pub asks: Vec<(Decimal, Decimal)>,
}

fn parse_lighter_ws_levels(payload: &serde_json::Value, side: &str) -> Vec<(Decimal, Decimal)> {
    let mut out = Vec::new();
    let Some(levels) = payload.get(side).and_then(|l| l.as_array()) else {
        return out;
    };
    for lvl in levels {
        if lvl.is_array() {
            let arr = lvl.as_array().cloned().unwrap_or_default();
            let (Some(p), Some(s)) = (dec_from_value(arr.first()), dec_from_value(arr.get(1)))
            else {
                continue;
            };
            out.push((p, s));
        } else if let Some((p, s)) = parse_lighter_price_size(lvl) {
            out.push((p, s));
        } else {
            let (Some(p), Some(s)) = (
                dec_from_value(lvl.get("price").or_else(|| lvl.get("px"))),
                dec_from_value(
                    lvl.get("size")
                        .or_else(|| lvl.get("quantity"))
                        .or_else(|| lvl.get("sz")),
                ),
            ) else {
                continue;
            };
            out.push((p, s));
        }
    }
    out
}

pub fn parse_lighter_delta(payload: &serde_json::Value) -> anyhow::Result<LighterBookDelta> {
    let market_id = u64_from_value(payload.get("market_id").or_else(|| payload.get("marketId")))
        .ok_or_else(|| anyhow::anyhow!("Lighter WS delta missing market_id"))?;
    let nonce = u64_from_value(payload.get("nonce"))
        .ok_or_else(|| anyhow::anyhow!("Lighter WS delta missing nonce"))?;
    let begin_nonce = u64_from_value(
        payload
            .get("begin_nonce")
            .or_else(|| payload.get("beginNonce")),
    )
    .ok_or_else(|| anyhow::anyhow!("Lighter WS delta missing begin_nonce"))?;
    Ok(LighterBookDelta {
        market_id,
        nonce,
        begin_nonce,
        bids: parse_lighter_ws_levels(payload, "bids"),
        asks: parse_lighter_ws_levels(payload, "asks"),
    })
}

/// Continuity check: a delta applies only when its `begin_nonce` equals the
/// last applied nonce; any gap means resubscribe, never interpolate.
pub fn lighter_delta_continuous(last_nonce: u64, delta: &LighterBookDelta) -> bool {
    delta.begin_nonce == last_nonce
}

/// Apply delta levels onto a cached side: zero size removes the price level,
/// otherwise upsert, then re-sort (bids descending, asks ascending).
pub fn apply_lighter_delta(
    levels: &mut Vec<(Decimal, Decimal)>,
    changes: &[(Decimal, Decimal)],
    bids_side: bool,
) {
    for (px, sz) in changes {
        if *sz <= Decimal::ZERO {
            levels.retain(|(p, _)| p != px);
        } else if let Some(slot) = levels.iter_mut().find(|(p, _)| p == px) {
            slot.1 = *sz;
        } else {
            levels.push((*px, *sz));
        }
    }
    if bids_side {
        levels.sort_by_key(|l| std::cmp::Reverse(l.0));
    } else {
        levels.sort_by_key(|l| l.0);
    }
}

/// Rebuild a hub-ready book from cached delta-tracked sides.
pub fn lighter_book_from_sides(
    venue: &VenueId,
    symbol: &str,
    bids: &[(Decimal, Decimal)],
    asks: &[(Decimal, Decimal)],
) -> OrderBook {
    let mut book = OrderBook::new(venue.clone(), symbol.to_string());
    for (p, s) in bids {
        book.bids.push(BookLevel::new(*p, *s));
    }
    for (p, s) in asks {
        book.asks.push(BookLevel::new(*p, *s));
    }
    book.timestamp = chrono::Utc::now();
    book
}

/* ------------------------------------------------------------------------ */
/* Vanta (Orderly)                                                           */
/* ------------------------------------------------------------------------ */

/// `wss://ws-evm.orderly.org/ws/stream/{account_id}`. An empty account id
/// means WS is skipped for Vanta (REST only) — never a fabricated id.
pub fn vanta_ws_url(account_id: &str) -> anyhow::Result<String> {
    let id = account_id.trim();
    if id.is_empty() {
        anyhow::bail!("Vanta WS requires an account_id; none configured — WS skipped, REST only");
    }
    Ok(format!("wss://{VANTA_WS_HOST}/ws/stream/{id}"))
}

/// `{"id","topic":"PERP_<BASE>_USDC@orderbook","event":"subscribe"}`.
pub fn vanta_subscribe_message(id: u64, base: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "topic": format!("PERP_{}_USDC@orderbook", base.trim().to_uppercase()),
        "event": "subscribe",
    })
}

/// Exact standard Orderly perp `PERP_<BASE>_USDC`; broker-suffixed or
/// otherwise malformed symbols do not match.
pub fn vanta_standard_base(symbol: &str) -> Option<&str> {
    let mut parts = symbol.split('_');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("PERP"), Some(base), Some("USDC"), None) if !base.is_empty() => Some(base),
        _ => None,
    }
}

/// Parse a depth-snapshot push `{topic, ts, data:{symbol, asks, bids}}`.
/// The topic must equal the subscribed topic and `data.symbol` must equal
/// the topic's exact standard symbol — broker-suffixed books are rejected.
/// Levels accept `[[px, qty]]` pairs (WS shape) and `{price, quantity}`
/// objects (REST shape).
pub fn parse_vanta_ws_book(
    venue: &VenueId,
    symbol: &str,
    expected_topic: &str,
    msg: &serde_json::Value,
) -> anyhow::Result<OrderBook> {
    let topic = msg
        .get("topic")
        .and_then(|t| t.as_str())
        .ok_or_else(|| anyhow::anyhow!("Vanta WS frame missing topic"))?;
    if topic != expected_topic {
        anyhow::bail!("Vanta WS topic '{topic}' does not match subscribed '{expected_topic}'");
    }
    let data = msg
        .get("data")
        .ok_or_else(|| anyhow::anyhow!("Vanta WS frame missing data"))?;
    let returned = data
        .get("symbol")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow::anyhow!("Vanta WS frame missing data.symbol"))?;
    vanta_standard_base(returned).ok_or_else(|| {
        anyhow::anyhow!("Vanta WS symbol '{returned}' is not a standard PERP_<BASE>_USDC market")
    })?;
    let expected_orderly = expected_topic
        .strip_suffix("@orderbook")
        .unwrap_or(expected_topic);
    if returned != expected_orderly {
        anyhow::bail!("Vanta WS returned symbol {returned} for subscribed {expected_orderly}");
    }
    let mut book = OrderBook::new(venue.clone(), symbol.to_string());
    for side in ["bids", "asks"] {
        let Some(levels) = data.get(side).and_then(|l| l.as_array()) else {
            continue;
        };
        for lvl in levels {
            let parsed = if lvl.is_array() {
                let arr = lvl.as_array().cloned().unwrap_or_default();
                match (dec_from_value(arr.first()), dec_from_value(arr.get(1))) {
                    (Some(p), Some(s)) => Some((p, s)),
                    _ => None,
                }
            } else {
                match (
                    dec_from_value(lvl.get("price")),
                    dec_from_value(lvl.get("quantity")),
                ) {
                    (Some(p), Some(s)) => Some((p, s)),
                    _ => None,
                }
            };
            if let Some((p, s)) = parsed {
                let level = BookLevel::new(p, s);
                if side == "bids" {
                    book.bids.push(level);
                } else {
                    book.asks.push(level);
                }
            }
        }
    }
    book.timestamp = chrono::Utc::now();
    Ok(book)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrageur_core::types::dec;

    fn venue(id: &str) -> VenueId {
        VenueId::new(id)
    }

    #[test]
    fn hyperliquid_l2book_fixture_parses_to_validated_book() {
        let data = serde_json::json!({
            "coin": "BTC",
            "levels": [
                [{"px": "64200.5", "sz": "0.5"}, {"px": "64199.0", "sz": "1.25"}],
                [{"px": "64201.0", "sz": "0.75"}, {"px": "64202.5", "sz": "2.0"}]
            ],
            "time": 1758000000000i64,
        });
        let book = parse_hyperliquid_l2book_data(&venue("hyperliquid"), "BTC", "BTC", &data)
            .expect("fixture parses");
        assert_eq!(book.bids.len(), 2);
        assert_eq!(book.asks.len(), 2);
        assert_eq!(book.bids[0].price, dec!(64200.5));
        assert_eq!(book.bids[0].size, dec!(0.5));
        assert_eq!(book.asks[0].price, dec!(64201.0));
        assert_eq!(book.asks[0].size, dec!(0.75));
        assert!(book.validate_for_execution().is_ok());
        let sub = hyperliquid_subscribe_message("BTC");
        assert_eq!(sub["subscription"]["coin"], serde_json::json!("BTC"));
        assert_eq!(sub["subscription"]["type"], serde_json::json!("l2Book"));
    }

    #[test]
    fn hyperliquid_coin_mismatch_and_missing_levels_fail_closed() {
        let data = serde_json::json!({
            "coin": "ETH",
            "levels": [[{"px": "3000.0", "sz": "1.0"}], [{"px": "3001.0", "sz": "1.0"}]],
        });
        assert!(parse_hyperliquid_l2book_data(&venue("hyperliquid"), "BTC", "BTC", &data).is_err());
        let no_levels = serde_json::json!({"coin": "BTC"});
        assert!(
            parse_hyperliquid_l2book_data(&venue("hyperliquid"), "BTC", "BTC", &no_levels).is_err()
        );
        let no_coin = serde_json::json!({"levels": [[], []]});
        assert!(
            parse_hyperliquid_l2book_data(&venue("hyperliquid"), "BTC", "BTC", &no_coin).is_err()
        );
    }

    #[test]
    fn lighter_ws_url_derivation_covers_rh_host_and_rejects_bare_hosts() {
        assert_eq!(
            lighter_ws_url("https://mainnet.zklighter.elliot.ai").unwrap(),
            "wss://mainnet.zklighter.elliot.ai/stream"
        );
        assert_eq!(
            lighter_ws_url("https://api.rh.lighter.xyz/").unwrap(),
            "wss://api.rh.lighter.xyz/stream"
        );
        assert_eq!(
            lighter_ws_url("http://127.0.0.1:8080").unwrap(),
            "ws://127.0.0.1:8080/stream"
        );
        assert!(lighter_ws_url("api.rh.lighter.xyz").is_err());
        assert!(lighter_ws_url("wss://mainnet.zklighter.elliot.ai/stream").is_err());
        assert!(lighter_ws_url("").is_err());
        assert!(lighter_ws_url("https://").is_err());
        let sub = lighter_subscribe_message(1);
        assert_eq!(sub["channel"], serde_json::json!("order_book/1"));
    }

    #[test]
    fn lighter_snapshot_parse_matches_rest_shapes_and_delta_gap_resubscribes() {
        let venue_id = venue("lighter");
        let snapshot = serde_json::json!({
            "market_id": 1,
            "nonce": 100,
            "bids": [{"price": "79569.8", "remaining_base_amount": "0.00007"}],
            "asks": [{"price": "79570.2", "size": "0.00126"}],
        });
        let (book, nonce) =
            parse_lighter_snapshot_book(&venue_id, "BTC", &snapshot).expect("snapshot parses");
        assert_eq!(nonce, 100);
        assert_eq!(book.bids[0].price, dec!(79569.8));
        assert_eq!(book.asks[0].size, dec!(0.00126));
        assert!(book.validate_for_execution().is_ok());

        let gap = serde_json::json!({
            "market_id": 1, "nonce": 102, "begin_nonce": 101,
            "bids": [], "asks": [],
        });
        let delta = parse_lighter_delta(&gap).expect("delta parses");
        assert_eq!(delta.nonce, 102);
        assert!(!lighter_delta_continuous(nonce, &delta));

        let ok = serde_json::json!({
            "market_id": 1, "nonce": 101, "begin_nonce": 100,
            "bids": [["79569.9", "0.0001"]], "asks": [],
        });
        let delta = parse_lighter_delta(&ok).expect("delta parses");
        assert!(lighter_delta_continuous(nonce, &delta));

        let mut bids: Vec<(Decimal, Decimal)> =
            book.bids.iter().map(|l| (l.price, l.size)).collect();
        apply_lighter_delta(&mut bids, &delta.bids, true);
        assert_eq!(bids[0], (dec!(79569.9), dec!(0.0001)));
        assert_eq!(bids[1], (dec!(79569.8), dec!(0.00007)));

        let mut asks: Vec<(Decimal, Decimal)> =
            book.asks.iter().map(|l| (l.price, l.size)).collect();
        apply_lighter_delta(&mut asks, &[(dec!(79570.2), dec!(0))], false);
        assert!(asks.is_empty());

        let rebuilt =
            lighter_book_from_sides(&venue_id, "BTC", &bids, &[(dec!(79571.0), dec!(1.0))]);
        assert!(rebuilt.validate_for_execution().is_ok());
    }

    #[test]
    fn lighter_market_symbol_map_rejects_spot_and_ambiguous_rows() {
        let details = serde_json::json!({
            "order_book_details": [
                {"symbol": "BTC", "market_id": 1, "market_type": "perp"},
                {"symbol": "ETH", "market_id": 2, "market_type": "perp"},
                {"symbol": "ETH", "market_id": 2048, "market_type": "spot"},
                {"symbol": "SOL", "market_id": 3, "market_type": "perp"},
                {"symbol": "sol", "market_id": 77, "market_type": "perp"},
                {"symbol": "", "market_id": 99, "market_type": "perp"},
            ],
        });
        let map = lighter_market_symbol_map(&details);
        assert_eq!(map.get(&1).map(String::as_str), Some("BTC"));
        assert_eq!(map.get(&2).map(String::as_str), Some("ETH"));
        assert!(!map.contains_key(&2048));
        assert!(!map.contains_key(&3));
        assert!(!map.contains_key(&77));
        assert!(lighter_market_symbol_map(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn vanta_ws_snapshot_parses_pairs_and_rejects_broker_suffix() {
        let venue_id = venue("vanta");
        let msg = serde_json::json!({
            "topic": "PERP_BTC_USDC@orderbook",
            "ts": 1758000000000i64,
            "data": {
                "symbol": "PERP_BTC_USDC",
                "bids": [[64299.5, 1.0], ["64200.0", "0.5"]],
                "asks": [["64301.0", "0.75"]],
            },
        });
        let book = parse_vanta_ws_book(&venue_id, "BTC", "PERP_BTC_USDC@orderbook", &msg)
            .expect("snapshot parses");
        assert_eq!(book.bids.len(), 2);
        assert_eq!(book.bids[0].price, dec!(64299.5));
        assert_eq!(book.bids[1].price, dec!(64200.0));
        assert!(book.validate_for_execution().is_ok());

        let suffixed = serde_json::json!({
            "topic": "PERP_BTC_USDC@orderbook",
            "data": {
                "symbol": "PERP_BTC_USDC_BRK",
                "bids": [["64200.0", "0.5"]],
                "asks": [["64201.0", "0.75"]],
            },
        });
        assert!(
            parse_vanta_ws_book(&venue_id, "BTC", "PERP_BTC_USDC@orderbook", &suffixed).is_err()
        );

        let wrong_topic = serde_json::json!({
            "topic": "PERP_ETH_USDC@orderbook",
            "data": {
                "symbol": "PERP_ETH_USDC",
                "bids": [["3000.0", "1.0"]],
                "asks": [["3001.0", "1.0"]],
            },
        });
        assert!(
            parse_vanta_ws_book(&venue_id, "BTC", "PERP_BTC_USDC@orderbook", &wrong_topic).is_err()
        );

        assert!(vanta_standard_base("PERP_BTC_USDC_BRK").is_none());
        assert!(vanta_standard_base("SPOT_BTC_USDC").is_none());
        let sub = vanta_subscribe_message(7, "btc");
        assert_eq!(sub["topic"], serde_json::json!("PERP_BTC_USDC@orderbook"));
        assert_eq!(sub["id"], serde_json::json!(7));
    }

    #[test]
    fn vanta_ws_url_requires_account_id() {
        assert_eq!(
            vanta_ws_url("abc123").unwrap(),
            "wss://ws-evm.orderly.org/ws/stream/abc123"
        );
        assert!(vanta_ws_url("").is_err());
        assert!(vanta_ws_url("   ").is_err());
    }

    #[test]
    fn reconnect_backoff_caps_at_120s() {
        assert_eq!(reconnect_backoff_secs(1), 2);
        assert_eq!(reconnect_backoff_secs(2), 4);
        assert_eq!(reconnect_backoff_secs(6), 64);
        assert_eq!(reconnect_backoff_secs(7), 120);
        assert_eq!(reconnect_backoff_secs(30), 120);
    }

    #[test]
    fn jitter_ratio_maps_unit_interval_to_pm20() {
        assert!((jitter_ratio_20(0.0) - 0.8).abs() < 1e-12);
        assert!((jitter_ratio_20(1.0) - 1.2).abs() < 1e-12);
        assert!((jitter_ratio_20(0.5) - 1.0).abs() < 1e-12);
        assert!((jitter_ratio_20(-5.0) - 0.8).abs() < 1e-12);
        assert!((jitter_ratio_20(5.0) - 1.2).abs() < 1e-12);
    }

    #[test]
    fn apply_jitter_clamps_out_of_range_ratios() {
        use std::time::Duration;
        let base = Duration::from_secs(10);
        let secs = |d: Duration| d.as_secs_f64();
        assert!((secs(apply_jitter_ratio(base, 0.0)) - 8.0).abs() < 1e-6);
        assert!((secs(apply_jitter_ratio(base, 99.0)) - 12.0).abs() < 1e-6);
        assert!((secs(apply_jitter_ratio(base, 1.0)) - 10.0).abs() < 1e-9);
        assert!(apply_jitter_ratio(Duration::ZERO, 1.0) >= Duration::from_millis(1));
    }

    #[test]
    fn jittered_helpers_stay_within_pm20() {
        use std::time::Duration;
        for _ in 0..200 {
            let secs = jittered_duration_20(Duration::from_secs(10)).as_secs_f64();
            assert!(((8.0 - 1e-6)..=(12.0 + 1e-6)).contains(&secs), "got {secs}");
            let b = jittered_reconnect_backoff_secs(3);
            assert!((6..=10).contains(&b), "backoff 8s ±20% got {b}");
        }
        for _ in 0..50 {
            let capped = jittered_reconnect_backoff_secs(30);
            assert!(
                (96..=144).contains(&capped),
                "capped 120s ±20% got {capped}"
            );
        }
    }
}
