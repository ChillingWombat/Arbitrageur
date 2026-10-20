use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arbitrageur_core::hub::PublicTradeEvent;
use arbitrageur_core::quote::OrderBook;
use arbitrageur_core::types::{Decimal, Side, VenueId};
use arbitrageur_exchanges::ws::{
    apply_lighter_delta, hyperliquid_subscribe_message, hyperliquid_ws_url,
    lighter_book_from_sides, lighter_default_market_symbol_map, lighter_delta_continuous,
    lighter_market_symbol_map, lighter_subscribe_message, lighter_ws_url,
    parse_hyperliquid_l2book_data, parse_lighter_delta, parse_lighter_snapshot_book,
    parse_vanta_ws_book, vanta_subscribe_topic, vanta_ws_url, LIGHTER_KEEPALIVE_SECS,
};
use chrono::Utc;
use futures::{FutureExt, Sink, SinkExt, StreamExt};
use std::panic::AssertUnwindSafe;
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
/// Orderly closes WS conns that miss its application-level ping (~every
/// 10-30s expected); send `{"event":"ping"}` well inside that window.
const VANTA_KEEPALIVE_SECS: u64 = 20;

/// Delay between subscription frames during the connect burst — instant
/// floods get the connection RST by venue-side WS flood protection.
const SUB_BURST_PACE_MS: u64 = 50;
/// A half-open TCP path produces no inbound frames even while protocol
/// pings appear to succeed. Force a reconnect after this long without data.
const WS_READ_TIMEOUT_SECS: u64 = 45;

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
        // Pre-seed the health-surface row so a lane is observable before
        // its first attempt completes.
        state
            .feed_lanes
            .lock()
            .entry(venue.to_string())
            .or_default()
            .phase = "starting".into();
        let task_state = state.clone();
        crate::spawn_supervised("ws_feed", move || {
            let task_state = task_state.clone();
            async move {
                tracing::info!("starting WS feed lane {venue}");
                supervise(venue, task_state).await;
            }
        });
    }
}

fn lane_phase(state: &AppEngineState, venue: &str, phase: &str) {
    state
        .feed_lanes
        .lock()
        .entry(venue.to_string())
        .or_default()
        .phase = phase.into();
}

/// Mark a WS handshake success — the only writer of `last_connected_ms`,
/// so the timestamp is always a real socket-open, never an attempt.
fn mark_ws_connected(state: &AppEngineState, venue: &str) {
    let mut lanes = state.feed_lanes.lock();
    let t = lanes.entry(venue.to_string()).or_default();
    t.last_connected_ms = Some(Utc::now().timestamp_millis());
    t.phase = "connected".into();
}

/// Bounded WS connect: `connect_async` has no internal timeout, so a
/// stalled upgrade (edge holding the socket open on a flagged IP) wedges
/// the lane forever with zero log output. 10s covers a healthy handshake
/// with margin; failure surfaces as a normal lane end + backoff retry.
///
/// The request carries a User-Agent: CloudFront in front of some venue WS
/// endpoints (api.rh-testnet.lighter.xyz verified) 403s handshakes that
/// omit it, while accepting the same handshake with any UA present.
async fn ws_connect(
    url: &str,
) -> anyhow::Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = url
        .into_client_request()
        .map_err(|e| anyhow::anyhow!("WS request build failed for {url}: {e}"))?;
    request.headers_mut().insert(
        tokio_tungstenite::tungstenite::http::header::USER_AGENT,
        concat!("arbitrageur-desktop/", env!("CARGO_PKG_VERSION"))
            .parse()
            .expect("UA header is valid ASCII"),
    );
    match tokio::time::timeout(Duration::from_secs(10), connect_async(request)).await {
        Ok(Ok((ws, _))) => Ok(ws),
        Ok(Err(e)) => Err(anyhow::anyhow!("WS connect failed for {url}: {e}")),
        Err(_) => Err(anyhow::anyhow!("WS connect timed out for {url}")),
    }
}

fn venue_ws_enabled(state: &AppEngineState, venue: &str) -> bool {
    // An admitted env-scoped adapter is the definitive "venue is on" check:
    // it folds in the engine flag OR an enabled account, AND refuses venues
    // the active environment does not support (e.g. Robinhood under
    // Testnet — feeding mainnet books into a testnet session would poison
    // the hub with prices the env's adapters cannot trade on).
    state.get_adapter(venue).is_some()
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
            lane_phase(&state, venue, "paused");
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        if !venue_ws_enabled(&state, venue) {
            lane_phase(&state, venue, "not-admitted");
            tokio::time::sleep(Duration::from_secs(30)).await;
            continue;
        }
        {
            let mut lanes = state.feed_lanes.lock();
            let t = lanes.entry(venue.to_string()).or_default();
            t.attempts += 1;
            t.phase = "connecting".into();
            t.backoff_until_ms = None;
        }
        let started = std::time::Instant::now();
        let outcome = AssertUnwindSafe(run_connection(venue, &state))
            .catch_unwind()
            .await
            .unwrap_or_else(|_| {
                Err(anyhow::anyhow!(
                    "{venue} WS lane panicked; supervisor preserving lane for retry"
                ))
            });
        {
            let mut lanes = state.feed_lanes.lock();
            let t = lanes.entry(venue.to_string()).or_default();
            t.last_ended_ms = Some(Utc::now().timestamp_millis());
            match &outcome {
                Ok(FeedOutcome::Stop) => t.phase = "stopped".into(),
                Ok(FeedOutcome::Disabled) => t.phase = "disabled".into(),
                Ok(FeedOutcome::Reconnect) => {
                    t.phase = "backoff".into();
                    t.last_error = None;
                }
                Err(e) => {
                    t.phase = "backoff".into();
                    t.last_error = Some(e.to_string());
                }
            }
        }
        match outcome {
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
        {
            let mut lanes = state.feed_lanes.lock();
            let t = lanes.entry(venue.to_string()).or_default();
            t.consecutive_failures = fail_streak;
            t.instant_kills = instant_kills;
            t.backoff_until_ms =
                Some(Utc::now().timestamp_millis() + (backoff_secs.max(1.0) * 1000.0) as i64);
        }
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
    let previous = state.market_data_hub.get_orderbook(venue_id, symbol);
    state.feed_hawkes_book(&venue_id.0, book);
    state.market_data_hub.update_orderbook_from_ws(book.clone());
    if let Some(previous) = previous {
        if let Some(trade_event) = infer_book_fill(&previous, book) {
            state.ingest_public_trade(&trade_event);
        }
    }
    if let Some(entry) = top_sample(book, venue_id) {
        // Normalize onto the USD basis and the mode's storage key: merged
        // modes keep the canonical-base key; `separate` keys on the pair
        // ("BTC-USDT") so quote variants stay independent series.
        let quote = arbitrageur_core::symbol::symbol_quote(&book.symbol)
            .or_else(|| {
                state
                    .monitor
                    .lock()
                    .as_ref()
                    .and_then(|m| m.book_quote(&venue_id.0, symbol, state.usd_merge_quotes()))
            })
            .unwrap_or_else(|| "USD".into());
        let scale = state.fx_price_factor(Some(&venue_id.0), &quote);
        let base = canonical_base(symbol);
        let store_key =
            if state.usd_merge_quotes() || !arbitrageur_core::symbol::is_usd_family(&quote) {
                base
            } else {
                format!("{base}-{quote}")
            };
        let entry = VenueSampleEntry {
            mid: entry.mid * scale,
            best_bid: entry.best_bid * scale,
            best_ask: entry.best_ask * scale,
            ..entry
        };
        // Throttled telemetry write — per-frame flood drops to one persisted
        // row per (symbol, venue) per `SAMPLE_MIN_INTERVAL_MS` on a dedicated
        // connection, so fsync-bound frames can never queue ahead of
        // execution-ledger writes.
        let _ = state.state_store.record_spread_samples_throttled(
            &store_key,
            state.usd_merge_quotes(),
            Utc::now().timestamp_millis(),
            &[entry],
            state.env().as_str(),
        );
    }
}

/// Infer a public trade only when the new top of book proves that liquidity
/// traded through the previous frame: a top level was consumed at the same
/// price, or the opposite side moved through it. Pure liquidity additions
/// never emit a synthetic trade.
fn infer_book_fill(previous: &OrderBook, current: &OrderBook) -> Option<PublicTradeEvent> {
    if previous.venue != current.venue {
        return None;
    }
    let mut candidates: Vec<(Side, Decimal, Decimal)> = Vec::new();
    if let (Some(before), Some(after)) = (previous.best_ask(), current.best_ask()) {
        if after.price > before.price {
            candidates.push((Side::Buy, before.price, before.size));
        } else if after.price == before.price && after.size < before.size {
            candidates.push((Side::Buy, before.price, before.size - after.size));
        }
    }
    if let (Some(before), Some(after)) = (previous.best_bid(), current.best_bid()) {
        if after.price < before.price {
            candidates.push((Side::Sell, before.price, before.size));
        } else if after.price == before.price && after.size < before.size {
            candidates.push((Side::Sell, before.price, before.size - after.size));
        }
    }
    candidates.retain(|(_, _, size)| *size > Decimal::ZERO);
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.2));
    let (side, price, size) = candidates.into_iter().next()?;
    Some(PublicTradeEvent {
        timestamp: current.timestamp,
        venue: current.venue.clone(),
        symbol: canonical_base(&current.symbol),
        side,
        price,
        size,
        is_liquidation: false,
    })
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
/// Native coin -> canonical base. WS subscriptions must use `venue_symbol`
/// verbatim: Hyperliquid kills the connection on any unlisted-coin sub, and
/// canonicalization uppercases (`kPEPE` -> `KPEPE`), producing invalid subs.
fn hyperliquid_coin_map(
    markets: &[arbitrageur_core::market::MarketMetadata],
) -> HashMap<String, String> {
    markets
        .iter()
        .filter(|m| m.is_active)
        .filter_map(|m| {
            let coin = m.venue_symbol.trim();
            (!coin.is_empty() && !coin.to_uppercase().ends_with("-SPOT"))
                .then(|| (coin.to_string(), canonical_base(&m.symbol.base)))
        })
        .collect()
}

/// "<native_symbol>@orderbook" -> canonical base. Same rule as
/// `hyperliquid_coin_map`: Orderly-family edges close conns on topics whose
/// symbol is not a listed native name.
fn vanta_topic_map(
    markets: &[arbitrageur_core::market::MarketMetadata],
) -> HashMap<String, String> {
    markets
        .iter()
        .filter(|m| m.is_active)
        .filter_map(|m| {
            let sym = m.venue_symbol.trim();
            (!sym.is_empty()).then(|| (format!("{sym}@orderbook"), canonical_base(&m.symbol.base)))
        })
        .collect()
}

async fn run_hyperliquid(state: &AppEngineState) -> anyhow::Result<FeedOutcome> {
    let venue_id = VenueId::new("hyperliquid");
    let adapter = state
        .get_adapter("hyperliquid")
        .ok_or_else(|| anyhow::anyhow!("hyperliquid is not admitted; skipping WS lane"))?;
    // Subscribe with the venue-native coin (`venue_symbol`), not the
    // canonical base: canonicalization uppercases (`kPEPE` -> `KPEPE`) and
    // Hyperliquid RSTs the whole connection on any unlisted-coin sub.
    let markets = adapter
        .fetch_markets()
        .await
        .map_err(|e| anyhow::anyhow!("venue_bases: {e}"))?;
    let coin_map = hyperliquid_coin_map(&markets);
    if coin_map.is_empty() {
        anyhow::bail!("hyperliquid lists no subscribable markets");
    }
    // Derive the WS endpoint from the admitted adapter's REST base so a
    // testnet adapter connects to the testnet socket — connecting mainnet
    // while orders route to testnet would poison the hub cross-env.
    let ws_url = hyperliquid_ws_url(
        &adapter
            .rest_base_url()
            .unwrap_or_else(|| "https://api.hyperliquid.xyz".to_string()),
    )?;
    let ws = ws_connect(&ws_url)
        .await
        .map_err(|e| anyhow::anyhow!("ws_connect: {e}"))?;
    mark_ws_connected(state, "hyperliquid");
    // Environment this socket was bound under: after a live/testnet flip
    // the hub is cleared and re-admission is re-evaluated — frames from
    // the old env's venue must stop, not repopulate it.
    let connect_env = state.env();
    {
        use tokio_tungstenite::MaybeTlsStream;
        let peer = match ws.get_ref() {
            MaybeTlsStream::Plain(s) => s.peer_addr().ok(),
            MaybeTlsStream::Rustls(s) => s.get_ref().0.peer_addr().ok(),
            _ => None,
        };
        if let Some(peer) = peer {
            tracing::info!("feed: hyperliquid WS handshake ok via {peer}");
        }
    }
    let (mut write, mut read) = ws.split();
    for (i, coin) in coin_map.keys().enumerate() {
        let frame = hyperliquid_subscribe_message(coin).to_string();
        write
            .send(Message::Text(frame))
            .await
            .map_err(|e| anyhow::anyhow!("sub_burst[{i}] {coin}: {e}"))?;
        // Pace the burst: servers RST connections that flood subscribes.
        tokio::time::sleep(Duration::from_millis(SUB_BURST_PACE_MS)).await;
    }
    tracing::info!(
        "feed: hyperliquid WS connected; subscribed {} coins",
        coin_map.len()
    );
    let mut keepalive = tokio::time::interval(Duration::from_secs(WS_KEEPALIVE_SECS));
    keepalive.tick().await;
    let read_deadline = tokio::time::sleep(Duration::from_secs(WS_READ_TIMEOUT_SECS));
    tokio::pin!(read_deadline);
    loop {
        let msg = tokio::select! {
            _ = keepalive.tick() => {
                write.send(Message::Ping(Vec::new())).await?;
                continue;
            }
            msg = read.next() => {
                read_deadline
                    .as_mut()
                    .reset(tokio::time::Instant::now() + Duration::from_secs(WS_READ_TIMEOUT_SECS));
                msg
            },
            _ = &mut read_deadline => {
                tracing::warn!(
                    "feed: hyperliquid WS read timed out after {}s — reconnecting",
                    WS_READ_TIMEOUT_SECS
                );
                return Ok(FeedOutcome::Reconnect);
            },
        };
        let Some(msg) = msg else {
            return Ok(FeedOutcome::Reconnect);
        };
        let msg = msg?;
        let Message::Text(text) = msg else {
            continue;
        };
        if state.env() != connect_env {
            tracing::info!(
                "feed: hyperliquid env switched mid-connection; reconnecting under new env"
            );
            return Ok(FeedOutcome::Reconnect);
        }
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
    fallback_base_url: &str,
) -> anyhow::Result<FeedOutcome> {
    let venue_id = VenueId::new(venue);
    // The admitted adapter's own base URL is authoritative — under Testnet
    // the `tn_*` adapter points at the venue's demo host and the feed must
    // follow it, otherwise the hub mixes mainnet books with testnet orders.
    let base_url = state
        .get_adapter(venue)
        .and_then(|a| a.rest_base_url())
        .unwrap_or_else(|| fallback_base_url.to_string());
    let base_url = base_url.as_str();
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
            .build()
            .unwrap_or_default()
    };
    let details_res = client
        .get(format!("{trimmed}/api/v1/orderBookDetails"))
        .send()
        .await;

    // The static fallback map encodes *mainnet* market ids. On a non-mainnet
    // base URL (testnet / regional), falling back would subscribe guessed
    // ids and mislabel whatever streams back — the venue's own
    // orderBookDetails is the only honest source there.
    let is_mainnet = base_url.contains("mainnet.") || base_url.contains("api.rh.lighter");
    let symbol_map = match details_res {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(details) = resp.json::<serde_json::Value>().await {
                let map = lighter_market_symbol_map(&details);
                if !map.is_empty() {
                    map
                } else if is_mainnet {
                    lighter_default_market_symbol_map()
                } else {
                    anyhow::bail!("{venue} orderBookDetails returned no usable markets; refusing fallback map on non-mainnet host");
                }
            } else if is_mainnet {
                lighter_default_market_symbol_map()
            } else {
                anyhow::bail!("{venue} orderBookDetails unparseable; refusing fallback map on non-mainnet host");
            }
        }
        _ => {
            if is_mainnet {
                tracing::info!("{venue} HTTP orderBookDetails failed/WAF challenge; falling back to default perp market map");
                lighter_default_market_symbol_map()
            } else {
                anyhow::bail!("{venue} orderBookDetails unreachable; refusing fallback map on non-mainnet host");
            }
        }
    };
    let ws = ws_connect(&ws_url).await?;
    mark_ws_connected(state, venue);
    // Environment this socket was bound under — see run_hyperliquid.
    let connect_env = state.env();
    let (mut write, mut read) = ws.split();
    for market_id in symbol_map.keys() {
        let frame = lighter_subscribe_message(*market_id).to_string();
        write.send(Message::Text(frame)).await?;
        tokio::time::sleep(Duration::from_millis(SUB_BURST_PACE_MS)).await;
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
    let read_deadline = tokio::time::sleep(Duration::from_secs(WS_READ_TIMEOUT_SECS));
    tokio::pin!(read_deadline);
    loop {
        let msg = tokio::select! {
            _ = keepalive.tick() => {
                write.send(Message::Ping(Vec::new())).await?;
                continue;
            }
            msg = read.next() => {
                read_deadline
                    .as_mut()
                    .reset(tokio::time::Instant::now() + Duration::from_secs(WS_READ_TIMEOUT_SECS));
                msg
            }
            _ = &mut read_deadline => {
                tracing::warn!(
                    "feed: {venue} WS read timed out after {}s — reconnecting",
                    WS_READ_TIMEOUT_SECS
                );
                return Ok(FeedOutcome::Reconnect);
            }
        };
        let Some(msg) = msg else {
            return Ok(FeedOutcome::Reconnect);
        };
        let msg = msg?;
        let Message::Text(text) = msg else {
            continue;
        };
        if state.env() != connect_env {
            tracing::info!("feed: {venue} env switched mid-connection; reconnecting under new env");
            return Ok(FeedOutcome::Reconnect);
        }
        handle_lighter_text(state, &venue_id, &mut lanes, &mut write, &text).await?;
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
    let venue_id = VenueId::new("vanta");
    let adapter = state
        .get_adapter("vanta")
        .ok_or_else(|| anyhow::anyhow!("vanta is not admitted; skipping WS lane"))?;
    // The Orderly account_id provisioned into the adapter from vault
    // credentials is authoritative; the settings field is a legacy override.
    let account_id: Option<String> = adapter.ws_account_id().or_else(|| {
        let s = state.settings.read().vanta_account_id.trim().to_string();
        (!s.is_empty()).then_some(s)
    });
    // Orderly closes `/ws/stream/public` and unregistered account ids with a
    // 1000 right after upgrade — no account_id means this lane cannot run
    // (REST polling still covers the venue). The WS host follows the
    // adapter's REST base so testnet adapters reach the testnet stream.
    let ws_url = match account_id
        .and_then(|id| vanta_ws_url(&id, adapter.rest_base_url().as_deref()).ok())
    {
        Some(url) => url,
        None => {
            tracing::info!("feed: vanta WS disabled: no registered Orderly account_id configured");
            return Ok(FeedOutcome::Disabled);
        }
    };
    // Subscribe with the venue-native symbol (`venue_symbol`), not the
    // canonical base — canonicalization can produce names the venue does
    // not list, and Orderly-family edges close conns on invalid topics.
    let markets = adapter
        .fetch_markets()
        .await
        .map_err(|e| anyhow::anyhow!("venue_bases: {e}"))?;
    let topics = vanta_topic_map(&markets);
    if topics.is_empty() {
        anyhow::bail!("vanta lists no subscribable markets");
    }
    let ws = ws_connect(&ws_url).await?;
    mark_ws_connected(state, "vanta");
    // Environment this socket was bound under — see run_hyperliquid.
    let connect_env = state.env();
    let (mut write, mut read) = ws.split();
    for (sub_id, topic) in (1u64..).zip(topics.keys()) {
        let frame = vanta_subscribe_topic(sub_id, topic).to_string();
        write.send(Message::Text(frame)).await?;
        tokio::time::sleep(Duration::from_millis(SUB_BURST_PACE_MS)).await;
    }
    tracing::info!(
        "feed: vanta WS connected; subscribed {} topics",
        topics.len()
    );
    let mut keepalive = tokio::time::interval(Duration::from_secs(VANTA_KEEPALIVE_SECS));
    keepalive.tick().await;
    let read_deadline = tokio::time::sleep(Duration::from_secs(WS_READ_TIMEOUT_SECS));
    tokio::pin!(read_deadline);
    loop {
        let msg = tokio::select! {
            // Orderly keepalive is application-level `{"event":"ping"}` —
            // protocol-level WS Ping frames do not count, and the server
            // closes conns that look idle (~2min).
            _ = keepalive.tick() => {
                write
                    .send(Message::Text(r#"{"event":"ping"}"#.to_string()))
                    .await?;
                continue;
            }
            msg = read.next() => {
                read_deadline
                    .as_mut()
                    .reset(tokio::time::Instant::now() + Duration::from_secs(WS_READ_TIMEOUT_SECS));
                msg
            },
            _ = &mut read_deadline => {
                tracing::warn!(
                    "feed: vanta WS read timed out after {}s — reconnecting",
                    WS_READ_TIMEOUT_SECS
                );
                return Ok(FeedOutcome::Reconnect);
            },
        };
        let Some(msg) = msg else {
            return Ok(FeedOutcome::Reconnect);
        };
        let msg = msg?;
        let Message::Text(text) = msg else {
            continue;
        };
        if state.env() != connect_env {
            tracing::info!("feed: vanta env switched mid-connection; reconnecting under new env");
            return Ok(FeedOutcome::Reconnect);
        }
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
    use arbitrageur_core::market::{MarketMetadata, Symbol};
    use arbitrageur_core::types::dec;

    fn market(venue: &str, venue_symbol: &str, base: &str) -> MarketMetadata {
        MarketMetadata {
            symbol: Symbol::perp(base, "USDC"),
            venue: VenueId::new(venue),
            venue_symbol: venue_symbol.to_string(),
            price_decimals: 5,
            size_decimals: 3,
            min_order_size: dec!(0.001),
            min_notional: dec!(10),
            maker_fee_bps: dec!(0),
            taker_fee_bps: dec!(2.5),
            contract_size: dec!(1),
            is_active: true,
        }
    }

    #[test]
    fn hyperliquid_coin_map_uses_native_coin_not_canonical() {
        // Regression: Hyperliquid RSTs the conn on unlisted-coin subs.
        // `kPEPE` must be subscribed verbatim — canonical base `KPEPE`
        // is an unlisted name and must never reach the wire.
        let markets = vec![
            market("hyperliquid", "BTC", "BTC"),
            market("hyperliquid", "kPEPE", "kPEPE"),
        ];
        let map = hyperliquid_coin_map(&markets);
        assert_eq!(map.get("BTC").map(String::as_str), Some("BTC"));
        assert_eq!(map.get("kPEPE").map(String::as_str), Some("KPEPE"));
        assert!(!map.contains_key("KPEPE"), "canonical form is not a coin");
    }

    #[test]
    fn vanta_topic_map_uses_native_symbol_not_canonical() {
        let markets = vec![
            market("vanta", "PERP_BTC_USDC", "BTC"),
            market("vanta", "PERP_1000PEPE_USDC", "1000PEPE"),
        ];
        let map = vanta_topic_map(&markets);
        assert_eq!(
            map.get("PERP_BTC_USDC@orderbook").map(String::as_str),
            Some("BTC")
        );
        assert_eq!(
            map.get("PERP_1000PEPE_USDC@orderbook").map(String::as_str),
            Some("1000PEPE")
        );
    }

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
    fn infer_book_fill_uses_consumed_top_level_only() {
        let previous = book("hyperliquid", "BTC-USDC", 100.0, 101.0);
        let unchanged = previous.clone();
        assert!(infer_book_fill(&previous, &unchanged).is_none());

        let mut ask_consumed = unchanged.clone();
        ask_consumed.asks[0].size = dec!(0.4);
        let trade = infer_book_fill(&previous, &ask_consumed).expect("ask consumption is a fill");
        assert_eq!(trade.side, Side::Buy);
        assert_eq!(trade.price, dec!(101));
        assert_eq!(trade.size, dec!(0.6));
        assert_eq!(trade.symbol, "BTC");
    }

    #[test]
    fn infer_book_fill_tracks_price_moving_bid_and_ask() {
        let previous = book("lighter", "ETH-USDC", 100.0, 101.0);
        let higher_ask = book("lighter", "ETH-USDC", 100.0, 102.0);
        let buy = infer_book_fill(&previous, &higher_ask).expect("ask lift is a buy");
        assert_eq!(buy.side, Side::Buy);
        assert_eq!(buy.price, dec!(101));

        let lower_bid = book("lighter", "ETH-USDC", 99.0, 101.0);
        let sell = infer_book_fill(&previous, &lower_bid).expect("bid hit is a sell");
        assert_eq!(sell.side, Side::Sell);
        assert_eq!(sell.price, dec!(100));
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
    fn top_sample_rejects_malformed_books() {
        let venue = VenueId::new("lighter");
        let good = book("lighter", "BTC", 100.0, 101.0);
        let sample = top_sample(&good, &venue).expect("sample");
        assert_eq!(sample.mid, 100.5);
        let empty = OrderBook::new(VenueId::new("lighter"), "BTC".to_string());
        assert!(top_sample(&empty, &venue).is_none());
    }
}
