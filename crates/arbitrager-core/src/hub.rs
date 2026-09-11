use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::{debug, trace};

use crate::quote::{FundingRate, OrderBook};
use crate::types::{Decimal, VenueId};

/// Event dispatched across the Market Data Bus whenever an orderbook or funding rate changes.
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
}

/// Entry stored in the in-memory cache with monotonic reception timestamp.
#[derive(Clone, Debug)]
struct CachedBook {
    received_at: Instant,
    book: OrderBook,
}

#[derive(Clone, Debug)]
struct CachedFunding {
    received_at: Instant,
    funding: FundingRate,
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
}

impl MarketDataHub {
    /// Initialize a new MarketDataHub with a given broadcast channel capacity and cache TTL.
    pub fn new(broadcast_capacity: usize, default_ttl_ms: u64) -> Arc<Self> {
        let (tx, _rx) = broadcast::channel(broadcast_capacity.max(256));
        Arc::new(Self {
            orderbooks: RwLock::new(HashMap::new()),
            funding_rates: RwLock::new(HashMap::new()),
            sequence_counter: AtomicU64::new(1),
            broadcaster: tx,
            default_ttl: Duration::from_millis(default_ttl_ms),
        })
    }

    /// Subscribe to real-time market updates emitted by the ingestion pipeline.
    pub fn subscribe(&self) -> broadcast::Receiver<MarketUpdateEvent> {
        self.broadcaster.subscribe()
    }

    /// Store a newly ingested orderbook and broadcast the update to all subscribed strategies.
    pub fn update_orderbook(&self, book: OrderBook) -> MarketUpdateEvent {
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
        };

        // Write to thread-safe synchronized cache
        {
            let key = (book.venue.clone(), book.symbol.clone());
            let mut lock = self.orderbooks.write();
            lock.insert(
                key,
                CachedBook {
                    received_at: now,
                    book,
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
        let key = (funding.venue.clone(), funding.symbol.clone());
        let mut lock = self.funding_rates.write();
        lock.insert(
            key,
            CachedFunding {
                received_at: now,
                funding,
            },
        );
    }

    /// Retrieve the latest cached orderbook for a specific venue and symbol.
    pub fn get_orderbook(&self, venue: &VenueId, symbol: &str) -> Option<OrderBook> {
        let key = (venue.clone(), symbol.to_string());
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
        let key = (venue.clone(), symbol.to_string());
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

    /// Retrieve consolidated cross-venue orderbooks for a given symbol in one atomic read.
    pub fn get_cross_venue_snapshot(
        &self,
        symbol: &str,
        max_age: Option<Duration>,
    ) -> Vec<(VenueId, OrderBook)> {
        let max_age = max_age.unwrap_or(self.default_ttl);
        let lock = self.orderbooks.read();
        let mut results = Vec::new();

        for ((venue, sym), cached) in lock.iter() {
            if sym == symbol && cached.received_at.elapsed() <= max_age {
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
        let key = (venue.clone(), symbol.to_string());
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
