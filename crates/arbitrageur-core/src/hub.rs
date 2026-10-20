use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tracing::{debug, trace};

use crate::quote::{FundingRate, OrderBook};
use crate::types::{Decimal, VenueId};

/// Public trade tick ingested from exchange WebSockets.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct PublicTradeEvent {
    pub timestamp: DateTime<Utc>,
    pub venue: VenueId,
    pub symbol: String,
    pub side: crate::types::Side,
    pub price: Decimal,
    pub size: Decimal,
    pub is_liquidation: bool,
}

/// Event dispatched across the Market Data Bus whenever an orderbook, funding rate, or trade arrives.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct MarketUpdateEvent {
    pub timestamp: DateTime<Utc>,
    pub sequence: u64,
    pub venue_id: VenueId,
    pub symbol: String,
    pub best_bid: Option<Decimal>,
    pub best_ask: Option<Decimal>,
    pub mid_price: Option<Decimal>,
    pub spread_bps: Option<Decimal>,
    pub funding_rate_8h: Option<Decimal>,
    pub trade_event: Option<PublicTradeEvent>,
}

/// Provenance of a cached orderbook: which ingestion path fed the hub.
/// WS-fed books let REST polling stand down; REST-fed books are the
/// cold-start / stale-feed fallback. Funding rates are REST-only (no WS
/// funding channel) and carry no provenance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookSource {
    Ws,
    Rest,
}

/// Entry stored in the in-memory cache with monotonic reception timestamp.
#[derive(Clone, Debug)]
struct CachedBook {
    received_at: Instant,
    book: OrderBook,
    source: BookSource,
}

#[derive(Clone, Debug)]
struct CachedFunding {
    received_at: Instant,
    funding: FundingRate,
}

/// Hub storage key for a symbol: uppercase canonical base with quote and
/// instrument-class suffixes stripped ("ETH-USDC" -> "ETH", "BTC-SPOT" ->
/// "BTC-SPOT" — spot and perp stay distinct), plus the economic index
/// aliases used by the engine's canonical_base ("US100" -> "NAS100").
///
/// WS feed lanes publish bare bases while REST/UI callers pass canonical
/// venue symbols; normalizing the key at the storage layer means a book
/// fed under any spelling is found under every spelling. The monitor
/// universe is already deduplicated per (venue, base), so one slot per
/// base per venue matches the system's granularity.
fn normalize_symbol_key(symbol: &str) -> String {
    crate::symbol::canonical_base(symbol)
}

/// Mode-aware hub key: under the `separate` USD-quote policy a quoted
/// instrument keeps its quote in the slot key ("BTC-USDT" ≠ "BTC-USDC");
/// merged modes collapse to the canonical base exactly as before.
fn hub_key(symbol: &str, merge_usd: bool) -> String {
    if merge_usd {
        normalize_symbol_key(symbol)
    } else {
        crate::symbol::pair_key(symbol, false)
    }
}

/// Centralized Market Data Hub (Ticker Plant).
///
/// Implements the Single Ingestion -> Synchronized In-Memory Cache -> Pub/Sub Broadcast
/// architectural pattern recommended for high-frequency cross-venue arbitrage.
/// Guarantees that all strategies evaluate trading logic on identical, synchronized snapshots
/// without duplicate network calls.
pub struct MarketDataHub {
    orderbooks: RwLock<HashMap<(VenueId, String), CachedBook>>,
    funding_rates: RwLock<HashMap<(VenueId, String), CachedFunding>>,
    sequence_counter: AtomicU64,
    broadcaster: broadcast::Sender<MarketUpdateEvent>,
    default_ttl: Duration,
    /// USD-quote policy for slot keys: `false` (the `separate` mode) keeps
    /// BASE-QUOTE identity per slot so quote variants never share a book.
    merge_usd_quotes: AtomicBool,
}

impl MarketDataHub {
    /// Switch the USD-quote slot policy. Called when the engine's
    /// `usd_quote_mode` changes; slots written under the other policy stop
    /// resolving (a miss is fail-closed — callers fall back to REST).
    pub fn set_merge_usd_quotes(&self, merge: bool) {
        self.merge_usd_quotes.store(merge, Ordering::SeqCst);
    }

    fn merge_usd(&self) -> bool {
        self.merge_usd_quotes.load(Ordering::SeqCst)
    }
    /// Initialize a new MarketDataHub with a given broadcast channel capacity and cache TTL.
    pub fn new(broadcast_capacity: usize, default_ttl_ms: u64) -> Arc<Self> {
        let (tx, _rx) = broadcast::channel(broadcast_capacity.max(256));
        Arc::new(Self {
            orderbooks: RwLock::new(HashMap::new()),
            funding_rates: RwLock::new(HashMap::new()),
            sequence_counter: AtomicU64::new(1),
            broadcaster: tx,
            default_ttl: Duration::from_millis(default_ttl_ms),
            merge_usd_quotes: AtomicBool::new(true),
        })
    }

    /// Subscribe to real-time market updates emitted by the ingestion pipeline.
    pub fn subscribe(&self) -> broadcast::Receiver<MarketUpdateEvent> {
        self.broadcaster.subscribe()
    }

    /// Store a newly ingested orderbook and broadcast the update to all subscribed strategies.
    /// REST ingestion path (cold-start / stale-feed fallback provenance).
    pub fn update_orderbook(&self, book: OrderBook) -> MarketUpdateEvent {
        self.store_orderbook(book, BookSource::Rest)
    }

    /// Store a WS-ingested orderbook. Same semantics as
    /// [`update_orderbook`](Self::update_orderbook) with WS provenance, so
    /// REST polling can stand down while the feed is fresh.
    pub fn update_orderbook_from_ws(&self, book: OrderBook) -> MarketUpdateEvent {
        self.store_orderbook(book, BookSource::Ws)
    }

    fn store_orderbook(&self, book: OrderBook, source: BookSource) -> MarketUpdateEvent {
        let seq = self.sequence_counter.fetch_add(1, Ordering::SeqCst);
        let now = Instant::now();
        let utc_now = Utc::now();

        let best_bid = book.best_bid_price();
        let best_ask = book.best_ask_price();
        let mid_price = match (best_bid, best_ask) {
            (Some(b), Some(a)) => Some((b + a) / Decimal::from(2)),
            _ => None,
        };
        let spread_bps = match (best_bid, best_ask, mid_price) {
            (Some(b), Some(a), Some(m)) if m > Decimal::ZERO => {
                Some(((a - b) / m) * Decimal::from(10000))
            }
            _ => None,
        };

        let event = MarketUpdateEvent {
            timestamp: utc_now,
            sequence: seq,
            venue_id: book.venue.clone(),
            symbol: book.symbol.clone(),
            best_bid,
            best_ask,
            mid_price,
            spread_bps,
            funding_rate_8h: None,
            trade_event: None,
        };

        // Write to thread-safe synchronized cache — keyed on the
        // normalized base so WS ("ETH") and REST ("ETH-USDC") spellings
        // share one slot.
        {
            let key = (book.venue.clone(), hub_key(&book.symbol, self.merge_usd()));
            let mut lock = self.orderbooks.write();
            lock.insert(
                key,
                CachedBook {
                    received_at: now,
                    book,
                    source,
                },
            );
        }

        // Broadcast event to all listening strategy workers (non-blocking)
        let _ = self.broadcaster.send(event.clone());
        trace!(
            "MarketDataHub: updated {:?} {} seq={} mid={:?}",
            event.venue_id,
            event.symbol,
            seq,
            event.mid_price
        );

        event
    }

    /// Store a newly ingested funding rate.
    pub fn update_funding(&self, funding: FundingRate) {
        let now = Instant::now();
        let key = (
            funding.venue.clone(),
            hub_key(&funding.symbol, self.merge_usd()),
        );
        let mut lock = self.funding_rates.write();
        lock.insert(
            key,
            CachedFunding {
                received_at: now,
                funding,
            },
        );
    }

    /// Dispatches a newly ingested public trade tick and broadcasts the update to all subscribed strategies.
    pub fn update_public_trade(&self, trade: PublicTradeEvent) -> MarketUpdateEvent {
        let seq = self.sequence_counter.fetch_add(1, Ordering::SeqCst);
        let key = (
            trade.venue.clone(),
            hub_key(&trade.symbol, self.merge_usd()),
        );
        let mid_price = self
            .orderbooks
            .read()
            .get(&key)
            .and_then(|cached| cached.book.mid_price());
        let event = MarketUpdateEvent {
            timestamp: trade.timestamp,
            sequence: seq,
            venue_id: trade.venue.clone(),
            symbol: trade.symbol.clone(),
            best_bid: None,
            best_ask: None,
            mid_price,
            spread_bps: None,
            funding_rate_8h: None,
            trade_event: Some(trade),
        };

        let _ = self.broadcaster.send(event.clone());
        trace!(
            "MarketDataHub: trade {:?} {} seq={} px={:?} sz={:?}",
            event.venue_id,
            event.symbol,
            seq,
            event.mid_price,
            event.trade_event.as_ref().map(|t| t.size)
        );

        event
    }

    /// Retrieve the latest cached orderbook for a specific venue and symbol.
    pub fn get_orderbook(&self, venue: &VenueId, symbol: &str) -> Option<OrderBook> {
        let key = (venue.clone(), hub_key(symbol, self.merge_usd()));
        let lock = self.orderbooks.read();
        lock.get(&key).map(|c| c.book.clone())
    }

    /// Retrieve the cached orderbook only if it is fresher than `max_age`.
    pub fn get_valid_orderbook(
        &self,
        venue: &VenueId,
        symbol: &str,
        max_age: Option<Duration>,
    ) -> Option<OrderBook> {
        let max_age = max_age.unwrap_or(self.default_ttl);
        let key = (venue.clone(), hub_key(symbol, self.merge_usd()));
        let lock = self.orderbooks.read();
        if let Some(cached) = lock.get(&key) {
            if cached.received_at.elapsed() <= max_age {
                return Some(cached.book.clone());
            } else {
                debug!(
                    "MarketDataHub: orderbook for {:?} {} is stale ({:?} > {:?})",
                    venue,
                    symbol,
                    cached.received_at.elapsed(),
                    max_age
                );
            }
        }
        None
    }

    /// Retrieve the cached orderbook only if it is WS-sourced and fresher
    /// than `max_age`. Returns `None` for REST-sourced or stale books, so
    /// callers can skip REST fetches only when the WS feed is live.
    /// Fail-closed: a missing entry yields `None`, never a synthesized book.
    pub fn get_valid_ws_book(
        &self,
        venue: &VenueId,
        symbol: &str,
        max_age: Option<Duration>,
    ) -> Option<OrderBook> {
        let max_age = max_age.unwrap_or(self.default_ttl);
        let key = (venue.clone(), hub_key(symbol, self.merge_usd()));
        let lock = self.orderbooks.read();
        if let Some(cached) = lock.get(&key) {
            if cached.source == BookSource::Ws && cached.received_at.elapsed() <= max_age {
                return Some(cached.book.clone());
            }
        }
        None
    }

    /// Retrieve consolidated cross-venue orderbooks for a given symbol in one atomic read.
    pub fn get_cross_venue_snapshot(
        &self,
        symbol: &str,
        max_age: Option<Duration>,
    ) -> Vec<(VenueId, OrderBook)> {
        let max_age = max_age.unwrap_or(self.default_ttl);
        let wanted = hub_key(symbol, self.merge_usd());
        let lock = self.orderbooks.read();
        let mut results = Vec::new();

        for ((venue, sym), cached) in lock.iter() {
            if *sym == wanted && cached.received_at.elapsed() <= max_age {
                results.push((venue.clone(), cached.book.clone()));
            }
        }

        results
    }

    /// Retrieve the latest cached funding rate.
    pub fn get_funding_rate(
        &self,
        venue: &VenueId,
        symbol: &str,
        max_age: Option<Duration>,
    ) -> Option<FundingRate> {
        let max_age = max_age.unwrap_or(self.default_ttl * 10);
        let key = (venue.clone(), hub_key(symbol, self.merge_usd()));
        let lock = self.funding_rates.read();
        if let Some(cached) = lock.get(&key) {
            if cached.received_at.elapsed() <= max_age {
                return Some(cached.funding.clone());
            }
        }
        None
    }

    /// Get current monotonic sequence counter.
    pub fn current_sequence(&self) -> u64 {
        self.sequence_counter.load(Ordering::Relaxed)
    }

    /// Clear all cached data (useful during mode switches or re-calibration).
    pub fn clear(&self) {
        self.orderbooks.write().clear();
        self.funding_rates.write().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quote::BookLevel;

    fn book(venue: &str, symbol: &str) -> OrderBook {
        let mut b = OrderBook::new(VenueId::new(venue), symbol.to_string());
        b.bids.push(BookLevel::new(dec(100.0), dec(1.0)));
        b.asks.push(BookLevel::new(dec(101.0), dec(1.0)));
        b
    }

    fn dec(v: f64) -> Decimal {
        Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO)
    }

    #[test]
    fn ws_book_is_returned_while_fresh() {
        let hub = MarketDataHub::new(16, 400);
        hub.update_orderbook_from_ws(book("hyperliquid", "BTC"));
        let vid = VenueId::new("hyperliquid");
        assert!(hub
            .get_valid_ws_book(&vid, "BTC", Some(Duration::from_millis(350)))
            .is_some());
    }

    #[test]
    fn rest_book_never_counts_as_ws() {
        let hub = MarketDataHub::new(16, 400);
        hub.update_orderbook(book("hyperliquid", "BTC"));
        let vid = VenueId::new("hyperliquid");
        assert!(hub
            .get_valid_ws_book(&vid, "BTC", Some(Duration::from_millis(350)))
            .is_none());
        assert!(hub
            .get_valid_orderbook(&vid, "BTC", Some(Duration::from_millis(350)))
            .is_some());
    }

    #[test]
    fn stale_ws_book_falls_back_to_none() {
        let hub = MarketDataHub::new(16, 400);
        hub.update_orderbook_from_ws(book("lighter", "ETH"));
        let vid = VenueId::new("lighter");
        std::thread::sleep(Duration::from_millis(60));
        assert!(hub
            .get_valid_ws_book(&vid, "ETH", Some(Duration::from_millis(20)))
            .is_none());
    }

    #[test]
    fn symbol_spellings_share_one_slot() {
        let hub = MarketDataHub::new(16, 400);
        let vid = VenueId::new("lighter");
        // WS lanes publish bare bases; REST/UI readers ask canonical —
        // both resolve to the same normalized key.
        hub.update_orderbook_from_ws(book("lighter", "ETH"));
        assert!(hub
            .get_valid_orderbook(&vid, "ETH-USDC", Some(Duration::from_millis(350)))
            .is_some());
        hub.update_orderbook(book("lighter", "BTC-USDC"));
        assert!(hub
            .get_valid_ws_book(&vid, "BTC", Some(Duration::from_millis(350)))
            .is_none());
        assert!(hub
            .get_valid_orderbook(&vid, "BTC", Some(Duration::from_millis(350)))
            .is_some());
    }

    #[test]
    fn spot_and_perp_stay_distinct() {
        let hub = MarketDataHub::new(16, 400);
        let vid = VenueId::new("lighter");
        hub.update_orderbook(book("lighter", "BTC-SPOT"));
        assert!(hub
            .get_valid_orderbook(&vid, "BTC-USDC", Some(Duration::from_millis(350)))
            .is_none());
        assert!(hub
            .get_valid_orderbook(&vid, "BTC-SPOT", Some(Duration::from_millis(350)))
            .is_some());
    }

    #[test]
    fn rest_overwrite_revokes_ws_provenance() {
        let hub = MarketDataHub::new(16, 400);
        hub.update_orderbook_from_ws(book("hyperliquid", "BTC"));
        hub.update_orderbook(book("hyperliquid", "BTC"));
        let vid = VenueId::new("hyperliquid");
        assert!(hub
            .get_valid_ws_book(&vid, "BTC", Some(Duration::from_millis(350)))
            .is_none());
    }

    #[test]
    fn public_trade_does_not_fabricate_mid_without_book() {
        let hub = MarketDataHub::new(16, 400);
        let event = hub.update_public_trade(PublicTradeEvent {
            timestamp: Utc::now(),
            venue: VenueId::new("hyperliquid"),
            symbol: "BTC".to_string(),
            side: crate::types::Side::Buy,
            price: dec(100.5),
            size: dec(1.0),
            is_liquidation: false,
        });

        assert!(event.mid_price.is_none());
    }

    #[test]
    fn public_trade_uses_cached_book_mid() {
        let hub = MarketDataHub::new(16, 400);
        hub.update_orderbook(book("hyperliquid", "BTC"));
        let event = hub.update_public_trade(PublicTradeEvent {
            timestamp: Utc::now(),
            venue: VenueId::new("hyperliquid"),
            symbol: "BTC-USDC".to_string(),
            side: crate::types::Side::Buy,
            price: dec(100.5),
            size: dec(1.0),
            is_liquidation: false,
        });

        assert_eq!(event.mid_price, Some(dec(100.5)));
    }
}
