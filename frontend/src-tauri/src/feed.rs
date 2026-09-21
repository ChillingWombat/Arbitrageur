use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arbitrageur_core::quote::OrderBook;
use arbitrageur_core::types::{Decimal, VenueId};
use arbitrageur_exchanges::ws::{
    apply_lighter_delta, hyperliquid_subscribe_message, lighter_book_from_sides,
    lighter_default_market_symbol_map, lighter_delta_continuous, lighter_market_symbol_map,
    lighter_subscribe_message, lighter_ws_url, parse_hyperliquid_l2book_data, parse_lighter_delta,
    parse_lighter_snapshot_book, parse_vanta_ws_book, vanta_subscribe_message, vanta_ws_url,
    HYPERLIQUID_WS_URL, LIGHTER_KEEPALIVE_SECS,
};
use chrono::Utc;
use futures::{Sink, SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::monitor::canonical_base;
use crate::state::AppEngineState;
use crate::state_store::VenueSampleEntry;

/// Robinhood Chain serves the Lighter protocol at this base URL; its WS lane
/// is derived exactly like the main Lighter lane.
const ROBINHOOD_LIGHTER_BASE_URL: &str = "https://api.rh.lighter.xyz";

/// Generic WS keepalive for venues without a protocol-level heartbeat
/// (hyperliquid, vanta). Lighter uses `LIGHTER_KEEPALIVE_SECS`.
const WS_KEEPALIVE_SECS: u64 = 30;

/// Delay between subscription frames during the connect burst — instant
/// floods get the connection RST by venue-side WS flood protection.
const SUB_BURST_PACE_MS: u64 = 50;

/// Outcome of one connection attempt: keep supervising or stop the lane.
enum FeedOutcome {
    Stop,
    Reconnect,
    /// Lane cannot run without config the venue requires (e.g. a registered
    /// Orderly account_id). Rest-checked on a slow cadence — not a failure.
    Disabled,
}

/// Spawn one supervisor task per admitted WS venue. REST polling stays
/// untouched as the cold-start and stale-feed fallback; a dead WS lane
/// simply stops feeding the hub and its pairs age out via the hub TTL.
pub fn spawn_all(state: Arc<AppEngineState>) {
    for venue in ["hyperliquid", "lighter", "robinhood", "vanta"] {
        let task_state = state.clone();
        tauri::async_runtime::spawn(async move {
            supervise(venue, task_state).await;
        });
    }
}

/// Bounded WS connect: `connect_async` has no internal timeout, so a
/// stalled upgrade (edge holding the socket open on a flagged IP) wedges
/// the lane forever with zero log output. 10s covers a healthy handshake
/// with margin; failure surfaces as a normal lane end + backoff retry.
async fn ws_connect(
    url: &str,
) -> anyhow::Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
> {
    match tokio::time::timeout(Duration::from_secs(10), connect_async(url)).await {
        Ok(Ok((ws, _))) => Ok(ws),
        Ok(Err(e)) => Err(anyhow::anyhow!("WS connect failed for {url}: {e}")),
        Err(_) => Err(anyhow::anyhow!("WS connect timed out for {url}")),
    }
}

fn venue_ws_enabled(state: &AppEngineState, venue: &str) -> bool {
    let flag = {
        let s = state.settings.read();
        match venue {
            "hyperliquid" => s.hyperliquid_enabled,
            "lighter" => s.lighter_enabled,
            "robinhood" => s.robinhood_enabled,
            "vanta" => s.vanta_enabled,
            _ => false,
        }
    };
    flag || state
        .state_store
        .has_enabled_account(venue)
        .unwrap_or(false)
}

async fn supervise(venue: &'static str, state: Arc<AppEngineState>) {
    let mut fail_streak: u32 = 0;
    let mut instant_kills: u32 = 0;
    loop {
        // Keep WS feeds alive in live mode: a real-time pushed book is
        // fresher than any REST poll, and every WS-fed read is one less REST
        // call against the venue's edge quota. Execution still enforces its
        // own freshness bound + identity validation on whatever the hub
        // serves, and falls back to a single REST fetch when the hub is
        // stale — suspending the feed here would force ALL reads (monitor,
        // radar, balances, execution) onto REST and has already tripped
        // Lighter's WAF once. Engine pause still suspends feeds.
        if *state.is_paused.read() {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        if !venue_ws_enabled(&state, venue) {
            tokio::time::sleep(Duration::from_secs(30)).await;
            continue;
        }
        let started = std::time::Instant::now();
        match run_connection(venue, &state).await {
            Ok(FeedOutcome::Stop) => return,
            Ok(FeedOutcome::Disabled) => {
                // Required config missing (e.g. unregistered venue account_id)
                // — recheck slowly in case the operator supplies it.
                tokio::time::sleep(Duration::from_secs(60)).await;
                continue;
            }
            Ok(FeedOutcome::Reconnect) => {
                tracing::info!("feed: {venue} connection ended; reconnecting");
            }
            Err(e) => {
                tracing::info!("feed: {venue} lane ended: {e}");
            }
        }
        fail_streak = (fail_streak + 1).min(8);
        // A lane whose whole cycle (REST pre-flight + connect + subscribe)
        // finishes in <5s died essentially instantly — venue-side rejection.
        // Tight retries feed the block; back off to ~5 minutes on a streak.
        if started.elapsed() < Duration::from_secs(5) {
            instant_kills = instant_kills.saturating_add(1);
        } else {
            instant_kills = 0;
        }
        let backoff_secs = if instant_kills >= 5 {
            300.0 * arbitrageur_exchanges::ws::jitter_ratio_20(rand::random::<f64>())
        } else {
            arbitrageur_exchanges::ws::jittered_reconnect_backoff_secs(fail_streak) as f64
        };
        tokio::time::sleep(Duration::from_secs_f64(backoff_secs.max(1.0))).await;
    }
}

async fn run_connection(
    venue: &'static str,
    state: &AppEngineState,
) -> anyhow::Result<FeedOutcome> {
    match venue {
        "hyperliquid" => run_hyperliquid(state).await,
        "lighter" => {
            let base = state.settings.read().lighter_url.clone();
            run_lighter_protocol(state, venue, &base).await
        }
        "robinhood" => run_lighter_protocol(state, venue, ROBINHOOD_LIGHTER_BASE_URL).await,
        "vanta" => run_vanta(state).await,
        _ => Ok(FeedOutcome::Stop),
    }
}

/// Shared parse -> identity-validate -> hub pipeline. Anything failing
/// validation is dropped; the lane's books go stale, never fabricated.
fn ingest_book(state: &AppEngineState, book: &OrderBook, venue_id: &VenueId, symbol: &str) {
    if let Err(e) = validate_feed_book(book, venue_id, symbol) {
        tracing::debug!("feed: {venue_id} dropped book for {symbol}: {e}");
        return;
    }
    state.market_data_hub.update_orderbook_from_ws(book.clone());
    if let Some(entry) = top_sample(book, venue_id) {
        let _ = state.state_store.record_spread_samples(
            symbol,
            Utc::now().timestamp_millis(),
            &[entry],
        );
    }
}

fn validate_feed_book(
    book: &OrderBook,
    expected_venue: &VenueId,
    expected_symbol: &str,
) -> Result<(), String> {
    if &book.venue != expected_venue {
        return Err(format!(
            "order book venue '{}' does not match expected venue '{}'",
            book.venue, expected_venue
        ));
    }
    // Hub keys normalize spellings — identity holds on canonical base.
    if crate::state_store::canonical_base(&book.symbol)
        != crate::state_store::canonical_base(expected_symbol)
    {
        return Err(format!(
            "order book symbol '{}' does not match requested symbol '{}'",
            book.symbol, expected_symbol
        ));
    }
    book.validate_for_execution()
}

fn top_sample(book: &OrderBook, venue_id: &VenueId) -> Option<VenueSampleEntry> {
    let bid = book.bids.first()?.price.to_string().parse::<f64>().ok()?;
    let ask = book.asks.first()?.price.to_string().parse::<f64>().ok()?;
    let mid = (bid + ask) / 2.0;
    let bid_depth: f64 = book
        .bids
        .iter()
        .take(5)
        .map(|l| l.size.to_string().parse::<f64>().unwrap_or(0.0))
        .sum();
    let ask_depth: f64 = book
        .asks
        .iter()
        .take(5)
        .map(|l| l.size.to_string().parse::<f64>().unwrap_or(0.0))
        .sum();
    (mid.is_finite() && mid > 0.0).then_some(VenueSampleEntry {
        venue_id: venue_id.0.clone(),
        mid,
        best_bid: bid,
        best_ask: ask,
        bid_depth: (bid_depth > 0.0).then_some(bid_depth),
        ask_depth: (ask_depth > 0.0).then_some(ask_depth),
    })
}

/// Canonical-base symbols listed on an admitted venue, via the existing
/// REST `fetch_markets` cold-start path.
async fn venue_bases(state: &AppEngineState, venue: &str) -> anyhow::Result<Vec<String>> {
    let adapter = state
        .get_adapter(venue)
        .ok_or_else(|| anyhow::anyhow!("{venue} is not admitted; skipping WS lane"))?;
    let markets = adapter.fetch_markets().await?;
    let mut bases: Vec<String> = markets
        .iter()
        .map(|m| canonical_base(&m.symbol.base))
        .filter(|b| !b.is_empty())
        .collect();
    bases.sort();
    bases.dedup();
    Ok(bases)
}

fn hyperliquid_coin(symbol_base: &str) -> Option<String> {
    if symbol_base.trim().is_empty() || symbol_base.to_uppercase().ends_with("-SPOT") {
        return None;
    }
    Some(
        symbol_base
            .split('-')
            .next()
            .unwrap_or(symbol_base)
            .to_string(),
    )
}

async fn run_hyperliquid(state: &AppEngineState) -> anyhow::Result<FeedOutcome> {
    let venue_id = VenueId::new("hyperliquid");
    let mut coin_map: HashMap<String, String> = HashMap::new();
    for base in venue_bases(state, "hyperliquid").await? {
        if let Some(coin) = hyperliquid_coin(&base) {
            coin_map.insert(coin, base);
        }
    }
    if coin_map.is_empty() {
        anyhow::bail!("hyperliquid lists no subscribable markets");
    }
    let ws = ws_connect(HYPERLIQUID_WS_URL).await?;
    let (mut write, mut read) = ws.split();
    for coin in coin_map.keys() {
        let frame = hyperliquid_subscribe_message(coin).to_string();
        write.send(Message::Text(frame)).await?;
        // Pace the burst: servers RST connections that flood subscribes.
        tokio::time::sleep(Duration::from_millis(SUB_BURST_PACE_MS)).await;
    }
    tracing::info!(
        "feed: hyperliquid WS connected; subscribed {} coins",
        coin_map.len()
    );
    let mut keepalive = tokio::time::interval(Duration::from_secs(WS_KEEPALIVE_SECS));
    keepalive.tick().await;
    loop {
        let msg = tokio::select! {
            _ = keepalive.tick() => {
                write.send(Message::Ping(Vec::new())).await?;
                continue;
            }
            msg = read.next() => msg,
        };
        let Some(msg) = msg else {
            return Ok(FeedOutcome::Reconnect);
        };
        let msg = msg?;
        let Message::Text(text) = msg else {
            continue;
        };
        let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if val.get("channel").and_then(|c| c.as_str()) != Some("l2Book") {
            tracing::debug!("feed: hyperliquid non-book frame: {text}");
            continue;
        }
        let (Some(data), Some(coin)) = (
            val.get("data"),
            val.get("data")
                .and_then(|d| d.get("coin"))
                .and_then(|c| c.as_str()),
        ) else {
            continue;
        };
        let Some(symbol) = coin_map.get(coin) else {
            continue;
        };
        match parse_hyperliquid_l2book_data(&venue_id, symbol, coin, data) {
            Ok(book) => ingest_book(state, &book, &venue_id, symbol),
            Err(e) => tracing::debug!("feed: hyperliquid dropped frame for {coin}: {e}"),
        }
    }
}

struct LighterLaneBook {
    symbol: String,
    bids: Vec<(Decimal, Decimal)>,
    asks: Vec<(Decimal, Decimal)>,
    nonce: u64,
    has_snapshot: bool,
}

async fn run_lighter_protocol(
    state: &AppEngineState,
    venue: &'static str,
    base_url: &str,
) -> anyhow::Result<FeedOutcome> {
    let venue_id = VenueId::new(venue);
    let ws_url = lighter_ws_url(base_url)?;
    let trimmed = base_url.trim().trim_end_matches('/');
    let client = {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::USER_AGENT,
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36".parse().unwrap(),
        );
        headers.insert(
            reqwest::header::ACCEPT,
            "application/json, text/plain, */*".parse().unwrap(),
        );
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .default_headers(headers)
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_default()
    };
    let details_res = client
        .get(format!("{trimmed}/api/v1/orderBookDetails"))
        .send()
        .await;

    let symbol_map = match details_res {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(details) = resp.json::<serde_json::Value>().await {
                let map = lighter_market_symbol_map(&details);
                if !map.is_empty() {
                    map
                } else {
                    lighter_default_market_symbol_map()
                }
            } else {
                lighter_default_market_symbol_map()
            }
        }
        _ => {
            tracing::info!("{venue} HTTP orderBookDetails failed/WAF challenge; falling back to default perp market map");
            lighter_default_market_symbol_map()
        }
    };
    let ws = ws_connect(&ws_url).await?;
    let (mut write, mut read) = ws.split();
    for market_id in symbol_map.keys() {
        let frame = lighter_subscribe_message(*market_id).to_string();
        write.send(Message::Text(frame)).await?;
    }
    tracing::info!(
        "feed: {venue} WS connected; subscribed {} markets ({})",
        symbol_map.len(),
        symbol_map.values().cloned().collect::<Vec<_>>().join(",")
    );
    let mut lanes: HashMap<u64, LighterLaneBook> = symbol_map
        .iter()
        .map(|(mid, sym)| {
            (
                *mid,
                LighterLaneBook {
                    symbol: sym.clone(),
                    bids: Vec::new(),
                    asks: Vec::new(),
                    nonce: 0,
                    has_snapshot: false,
                },
            )
        })
        .collect();

    let mut keepalive = tokio::time::interval(Duration::from_secs(LIGHTER_KEEPALIVE_SECS));
    keepalive.tick().await;
    loop {
        tokio::select! {
            _ = keepalive.tick() => {
                write.send(Message::Ping(Vec::new())).await?;
            }
            msg = read.next() => {
                let Some(msg) = msg else {
                    return Ok(FeedOutcome::Reconnect);
                };
                let msg = msg?;
                let Message::Text(text) = msg else {
                    continue;
                };
                handle_lighter_text(state, &venue_id, &mut lanes, &mut write, &text).await?;
            }
        }
    }
}

async fn handle_lighter_text<S>(
    state: &AppEngineState,
    venue_id: &VenueId,
    lanes: &mut HashMap<u64, LighterLaneBook>,
    write: &mut S,
    text: &str,
) -> anyhow::Result<()>
where
    S: Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let Ok(val) = serde_json::from_str::<serde_json::Value>(text) else {
        return Ok(());
    };
    let channel = val.get("channel").and_then(|c| c.as_str()).unwrap_or("");
    let msg_type = val.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let payload = val.get("data").unwrap_or(&val);
    let market_id = payload
        .get("market_id")
        .or_else(|| payload.get("marketId"))
        .and_then(|m| {
            m.as_u64()
                .or_else(|| m.as_str().and_then(|s| s.parse::<u64>().ok()))
        })
        .or_else(|| {
            channel
                .strip_prefix("order_book/")
                .and_then(|s| s.parse::<u64>().ok())
        });
    let Some(market_id) = market_id else {
        return Ok(());
    };
    let Some(lane) = lanes.get_mut(&market_id) else {
        return Ok(());
    };
    if msg_type == "update/order_book" {
        let Ok(delta) = parse_lighter_delta(payload) else {
            return Ok(());
        };
        if !lane.has_snapshot {
            return Ok(());
        }
        if !lighter_delta_continuous(lane.nonce, &delta) {
            let frame = lighter_subscribe_message(market_id).to_string();
            write.send(Message::Text(frame)).await?;
            lane.has_snapshot = false;
            return Ok(());
        }
        apply_lighter_delta(&mut lane.bids, &delta.bids, true);
        apply_lighter_delta(&mut lane.asks, &delta.asks, false);
        lane.nonce = delta.nonce;
        let book = lighter_book_from_sides(venue_id, &lane.symbol, &lane.bids, &lane.asks);
        ingest_book(state, &book, venue_id, &lane.symbol);
    } else if channel.starts_with("order_book/") {
        let Ok((book, nonce)) = parse_lighter_snapshot_book(venue_id, &lane.symbol, payload) else {
            return Ok(());
        };
        lane.bids = book.bids.iter().map(|l| (l.price, l.size)).collect();
        lane.asks = book.asks.iter().map(|l| (l.price, l.size)).collect();
        lane.nonce = nonce;
        lane.has_snapshot = true;
        ingest_book(state, &book, venue_id, &lane.symbol);
    }
    Ok(())
}

async fn run_vanta(state: &AppEngineState) -> anyhow::Result<FeedOutcome> {
    let account_id = state.settings.read().vanta_account_id.clone();
    // Orderly closes `/ws/stream/public` and unregistered account ids with a
    // 1000 right after upgrade — no account_id means this lane cannot run
    // (REST polling still covers the venue).
    let ws_url = match vanta_ws_url(&account_id) {
        Ok(url) => url,
        Err(e) => {
            tracing::info!("feed: vanta WS disabled: {e}");
            return Ok(FeedOutcome::Disabled);
        }
    };
    let venue_id = VenueId::new("vanta");
    let bases = venue_bases(state, "vanta").await?;
    if bases.is_empty() {
        anyhow::bail!("vanta lists no subscribable markets");
    }
    let mut topics: HashMap<String, String> = HashMap::new();
    for base in &bases {
        topics.insert(format!("PERP_{base}_USDC@orderbook"), base.clone());
    }
    let ws = ws_connect(&ws_url).await?;
    let (mut write, mut read) = ws.split();
    for (sub_id, base) in (1u64..).zip(topics.values()) {
        let frame = vanta_subscribe_message(sub_id, base).to_string();
        write.send(Message::Text(frame)).await?;
        tokio::time::sleep(Duration::from_millis(SUB_BURST_PACE_MS)).await;
    }
    tracing::info!(
        "feed: vanta WS connected; subscribed {} topics",
        topics.len()
    );
    let mut keepalive = tokio::time::interval(Duration::from_secs(WS_KEEPALIVE_SECS));
    keepalive.tick().await;
    loop {
        let msg = tokio::select! {
            _ = keepalive.tick() => {
                write.send(Message::Ping(Vec::new())).await?;
                continue;
            }
            msg = read.next() => msg,
        };
        let Some(msg) = msg else {
            return Ok(FeedOutcome::Reconnect);
        };
        let msg = msg?;
        let Message::Text(text) = msg else {
            continue;
        };
        let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let Some(topic) = val.get("topic").and_then(|t| t.as_str()) else {
            tracing::debug!("feed: vanta untopiced frame: {text}");
            continue;
        };
        let Some(symbol) = topics.get(topic).cloned() else {
            tracing::debug!("feed: vanta frame for unknown topic {topic}: {text}");
            continue;
        };
        match parse_vanta_ws_book(&venue_id, &symbol, topic, &val) {
            Ok(book) => ingest_book(state, &book, &venue_id, &symbol),
            Err(e) => tracing::debug!("feed: vanta dropped frame for {topic}: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrageur_core::types::dec;

    fn book(venue: &str, symbol: &str, bid: f64, ask: f64) -> OrderBook {
        let mut b = OrderBook::new(VenueId::new(venue), symbol.to_string());
        b.bids.push(arbitrageur_core::quote::BookLevel::new(
            Decimal::from_f64_retain(bid).unwrap_or(dec!(0)),
            dec!(1),
        ));
        b.asks.push(arbitrageur_core::quote::BookLevel::new(
            Decimal::from_f64_retain(ask).unwrap_or(dec!(0)),
            dec!(1),
        ));
        b
    }

    #[test]
    fn validate_feed_book_enforces_identity_and_structure() {
        let venue = VenueId::new("hyperliquid");
        assert!(
            validate_feed_book(&book("hyperliquid", "BTC", 100.0, 101.0), &venue, "BTC").is_ok()
        );
        assert!(validate_feed_book(&book("lighter", "BTC", 100.0, 101.0), &venue, "BTC").is_err());
        assert!(
            validate_feed_book(&book("hyperliquid", "ETH", 100.0, 101.0), &venue, "BTC").is_err()
        );
        assert!(
            validate_feed_book(&book("hyperliquid", "BTC", 101.0, 100.0), &venue, "BTC").is_err()
        );
    }

    #[test]
    fn hyperliquid_coin_rejects_spot_suffixed_bases() {
        assert_eq!(hyperliquid_coin("BTC").as_deref(), Some("BTC"));
        assert!(hyperliquid_coin("").is_none());
        assert!(hyperliquid_coin("ETH-SPOT").is_none());
    }

    #[test]
    fn top_sample_rejects_malformed_books() {
        let venue = VenueId::new("lighter");
        let good = book("lighter", "BTC", 100.0, 101.0);
        let sample = top_sample(&good, &venue).expect("sample");
        assert_eq!(sample.mid, 100.5);
        let empty = OrderBook::new(VenueId::new("lighter"), "BTC".to_string());
        assert!(top_sample(&empty, &venue).is_none());
    }
}
