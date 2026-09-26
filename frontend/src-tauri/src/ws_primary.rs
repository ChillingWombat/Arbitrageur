use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::hub::MarketDataHub;
use arbitrageur_core::quote::OrderBook;
use parking_lot::Mutex;

struct FlightSlot {
    notify: tokio::sync::Notify,
    result: Mutex<Option<Result<OrderBook, String>>>,
}

/// Per-(venue, symbol) in-flight dedup for orderbook REST fetches.
/// Concurrent callers for the same key await the leader's request instead
/// of firing N identical HTTP calls. Orderbook scope only; funding,
/// markets, and ticker paths are untouched.
pub struct OrderbookSingleFlight {
    slots: Mutex<HashMap<(String, String), Arc<FlightSlot>>>,
    /// Last-failure instant per (venue, symbol), with the cooldown the
    /// failure earned. A book fetch that just failed used to be retried on
    /// every poll tick, burning a TLS handshake per failure forever. Within
    /// the cooldown the error is replayed locally. Transient failures get a
    /// short window so recovery stays prompt; errors that prove the venue
    /// doesn't list the symbol (HTTP 404, "no quote", "unknown instrument")
    /// get a long one — the answer won't change until listings refresh.
    failures: Mutex<HashMap<(String, String), (std::time::Instant, std::time::Duration)>>,
}

/// How long a transiently-failed (venue, symbol) book fetch replays its
/// error locally.
const BOOK_FAIL_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(15);
/// Cooldown for failures that prove the venue does not list the symbol.
const BOOK_UNLISTED_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(300);

/// Whether a book-fetch error proves the venue doesn't list the symbol —
/// HTTP 404 plus the phrasing venues use for missing instruments. Anything
/// else (network, auth, rate limit, 5xx) stays transient: it may recover
/// within seconds and the short cooldown preserves that.
fn book_failure_is_unlisted(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("404")
        || e.contains("no quote found")
        || e.contains("unknown symbol")
        || e.contains("unknown instrument")
        || e.contains("instrument not found")
        || e.contains("symbol not found")
        || e.contains("not listed")
        || e.contains("unsupported")
        || e.contains("ws-only")
        || e.contains("ws only")
}

impl OrderbookSingleFlight {
    pub fn new() -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
            failures: Mutex::new(HashMap::new()),
        }
    }

    /// Fetch through the single-flight map. Exactly one caller per key
    /// becomes the leader (the one that inserts the slot under the map
    /// lock) and runs the adapter request; arrivals while the slot is
    /// present await the leader's outcome. Fail-closed: errors propagate
    /// to every waiter, never a synthesized book.
    pub async fn fetch(
        &self,
        adapter: &Arc<dyn ExchangeAdapter>,
        symbol: &str,
    ) -> Result<OrderBook, String> {
        let key = (adapter.venue_id().0.clone(), symbol.to_string());
        if let Some((at, cooldown)) = self.failures.lock().get(&key) {
            if at.elapsed() < *cooldown {
                return Err(format!(
                    "{}: book fetch for {symbol} cooling down after failure",
                    key.0
                ));
            }
        }
        let (slot, am_leader) = {
            let mut slots = self.slots.lock();
            match slots.get(&key) {
                Some(slot) => (slot.clone(), false),
                None => {
                    let slot = Arc::new(FlightSlot {
                        notify: tokio::sync::Notify::new(),
                        result: Mutex::new(None),
                    });
                    slots.insert(key.clone(), slot.clone());
                    (slot, true)
                }
            }
        };
        if am_leader {
            let outcome = adapter
                .fetch_orderbook(symbol)
                .await
                .map_err(|e| format!("{e:?}"));
            match &outcome {
                Ok(_) => {
                    self.failures.lock().remove(&key);
                }
                Err(e) => {
                    let cooldown = if book_failure_is_unlisted(e) {
                        BOOK_UNLISTED_COOLDOWN
                    } else {
                        BOOK_FAIL_COOLDOWN
                    };
                    self.failures
                        .lock()
                        .insert(key.clone(), (std::time::Instant::now(), cooldown));
                }
            }
            *slot.result.lock() = Some(outcome.clone());
            self.slots.lock().remove(&key);
            slot.notify.notify_waiters();
            outcome
        } else {
            loop {
                // Register interest BEFORE checking the result: `Notify`
                // drops notifications with no waiter, so check-then-await
                // can miss the leader's `notify_waiters` and wedge forever.
                let notified = slot.notify.notified();
                if let Some(outcome) = slot.result.lock().clone() {
                    return outcome;
                }
                notified.await;
            }
        }
    }
}

/// Where a WS-first resolved book came from. Only [`ResolvedBook::Fresh`]
/// needs a REST-provenance hub store; WS and hub-cached books are already
/// stored (re-storing a cached book would refresh its age forever).
pub enum ResolvedBook {
    Ws(OrderBook),
    Cached(OrderBook),
    Fresh(OrderBook),
}

/// Resolve one venue book WS-first: a fresh WS-fed hub book wins outright;
/// otherwise a fresh any-source hub book is reused; otherwise a
/// single-flight REST fetch runs.
/// Identity/structure validation stays with the caller, which still runs
/// its checks and still records samples on either path.
pub async fn resolve_book_ws_first(
    hub: &MarketDataHub,
    flights: &OrderbookSingleFlight,
    adapter: &Arc<dyn ExchangeAdapter>,
    symbol: &str,
    max_age: Duration,
) -> Option<ResolvedBook> {
    let venue_id = adapter.venue_id();
    if let Some(book) = hub.get_valid_ws_book(&venue_id, symbol, Some(max_age)) {
        return Some(ResolvedBook::Ws(book));
    }
    if let Some(book) = hub.get_valid_orderbook(&venue_id, symbol, Some(max_age)) {
        return Some(ResolvedBook::Cached(book));
    }
    match flights.fetch(adapter, symbol).await {
        Ok(book) => Some(ResolvedBook::Fresh(book)),
        Err(e) => {
            // The REST fallback failure used to vanish here — every poll
            // then retried a doomed call with no trace. The real error is
            // logged once per wire attempt; a cooldown replay is not new
            // information, so it stays at debug.
            if e.contains("cooling down after failure") {
                tracing::debug!("ws_first: {venue_id} REST book fetch for {symbol}: {e}");
            } else {
                tracing::info!("ws_first: {venue_id} REST book fetch for {symbol} failed: {e}");
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrageur_core::market::MarketMetadata;
    use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
    use arbitrageur_core::position::AccountSnapshot;
    use arbitrageur_core::quote::{BookLevel, FundingRate, Ticker};
    use arbitrageur_core::types::{Decimal, VenueId};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn fixture_book(venue: &VenueId, symbol: &str) -> OrderBook {
        let mut b = OrderBook::new(venue.clone(), symbol.to_string());
        b.bids.push(BookLevel::new(
            Decimal::from_f64_retain(100.0).unwrap_or(Decimal::ZERO),
            Decimal::ONE,
        ));
        b.asks.push(BookLevel::new(
            Decimal::from_f64_retain(101.0).unwrap_or(Decimal::ZERO),
            Decimal::ONE,
        ));
        b
    }

    struct CountingAdapter {
        venue: VenueId,
        calls: AtomicUsize,
        delay_ms: u64,
        fail: bool,
    }

    impl CountingAdapter {
        fn new(venue: &str, delay_ms: u64, fail: bool) -> Self {
            Self {
                venue: VenueId::new(venue),
                calls: AtomicUsize::new(0),
                delay_ms,
                fail,
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl ExchangeAdapter for CountingAdapter {
        fn venue_id(&self) -> VenueId {
            self.venue.clone()
        }

        fn name(&self) -> &str {
            "counting"
        }

        async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
            anyhow::bail!("no markets in fixture")
        }

        async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            }
            if self.fail {
                anyhow::bail!("venue down in fixture");
            }
            Ok(fixture_book(&self.venue, symbol))
        }

        async fn fetch_ticker(&self, _symbol: &str) -> anyhow::Result<Ticker> {
            anyhow::bail!("no ticker in fixture")
        }

        async fn fetch_funding_rate(&self, _symbol: &str) -> anyhow::Result<FundingRate> {
            anyhow::bail!("no funding in fixture")
        }

        async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
            anyhow::bail!("no account in fixture")
        }

        async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
            anyhow::bail!("no orders in fixture")
        }

        async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
            anyhow::bail!("no orders in fixture")
        }
    }

    #[tokio::test]
    async fn ws_fresh_skips_adapter_entirely() {
        let hub = MarketDataHub::new(16, 400);
        let flights = OrderbookSingleFlight::new();
        let inner = Arc::new(CountingAdapter::new("mock", 0, false));
        let adapter: Arc<dyn ExchangeAdapter> = inner.clone();
        hub.update_orderbook_from_ws(fixture_book(&VenueId::new("mock"), "BTC"));
        let ttl = Duration::from_millis(350);
        let resolved = resolve_book_ws_first(&hub, &flights, &adapter, "BTC", ttl).await;
        assert!(matches!(resolved, Some(ResolvedBook::Ws(_))));
        assert_eq!(inner.calls(), 0);
    }

    #[tokio::test]
    async fn stale_ws_falls_back_to_rest_exactly_once() {
        let hub = MarketDataHub::new(16, 400);
        let flights = OrderbookSingleFlight::new();
        let inner = Arc::new(CountingAdapter::new("mock", 0, false));
        let adapter: Arc<dyn ExchangeAdapter> = inner.clone();
        hub.update_orderbook_from_ws(fixture_book(&VenueId::new("mock"), "BTC"));
        tokio::time::sleep(Duration::from_millis(60)).await;
        let resolved =
            resolve_book_ws_first(&hub, &flights, &adapter, "BTC", Duration::from_millis(20)).await;
        assert!(matches!(resolved, Some(ResolvedBook::Fresh(_))));
        assert_eq!(inner.calls(), 1);
        if let Some(ResolvedBook::Fresh(book)) = resolved {
            hub.update_orderbook(book);
        }
        let again =
            resolve_book_ws_first(&hub, &flights, &adapter, "BTC", Duration::from_millis(350))
                .await;
        assert!(matches!(again, Some(ResolvedBook::Cached(_))));
        assert_eq!(inner.calls(), 1);
    }

    #[tokio::test]
    async fn rest_cache_hit_needs_no_fetch() {
        let hub = MarketDataHub::new(16, 400);
        let flights = OrderbookSingleFlight::new();
        let inner = Arc::new(CountingAdapter::new("mock", 0, false));
        let adapter: Arc<dyn ExchangeAdapter> = inner.clone();
        hub.update_orderbook(fixture_book(&VenueId::new("mock"), "BTC"));
        let resolved =
            resolve_book_ws_first(&hub, &flights, &adapter, "BTC", Duration::from_millis(350))
                .await;
        assert!(matches!(resolved, Some(ResolvedBook::Cached(_))));
        assert_eq!(inner.calls(), 0);
    }

    #[tokio::test]
    async fn single_flight_coalesces_concurrent_callers() {
        let flights = Arc::new(OrderbookSingleFlight::new());
        let inner = Arc::new(CountingAdapter::new("mock", 100, false));
        let adapter: Arc<dyn ExchangeAdapter> = inner.clone();
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..10 {
            let flights = flights.clone();
            let adapter = adapter.clone();
            set.spawn(async move { flights.fetch(&adapter, "BTC").await });
        }
        let mut oks = 0;
        while let Some(joined) = set.join_next().await {
            let res = joined.expect("task panicked");
            assert!(res.is_ok());
            oks += 1;
        }
        assert_eq!(oks, 10);
        assert_eq!(inner.calls(), 1);
    }

    #[tokio::test]
    async fn single_flight_propagates_errors_to_all_waiters() {
        let flights = Arc::new(OrderbookSingleFlight::new());
        let inner = Arc::new(CountingAdapter::new("mock", 50, true));
        let adapter: Arc<dyn ExchangeAdapter> = inner.clone();
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..3 {
            let flights = flights.clone();
            let adapter = adapter.clone();
            set.spawn(async move { flights.fetch(&adapter, "BTC").await });
        }
        let mut errs = 0;
        while let Some(joined) = set.join_next().await {
            assert!(joined.expect("task panicked").is_err());
            errs += 1;
        }
        assert_eq!(errs, 3);
        assert_eq!(inner.calls(), 1);
    }

    #[tokio::test]
    async fn waiters_arriving_during_completion_never_wedge() {
        let flights = Arc::new(OrderbookSingleFlight::new());
        let inner = Arc::new(CountingAdapter::new("mock", 50, false));
        let adapter: Arc<dyn ExchangeAdapter> = inner.clone();
        let outcome = tokio::time::timeout(Duration::from_secs(15), async {
            for _ in 0..20 {
                let mut set = tokio::task::JoinSet::new();
                for _ in 0..8 {
                    let flights = flights.clone();
                    let adapter = adapter.clone();
                    set.spawn(async move { flights.fetch(&adapter, "BTC").await });
                }
                while let Some(joined) = set.join_next().await {
                    assert!(joined.expect("task panicked").is_ok());
                }
            }
        })
        .await;
        assert!(
            outcome.is_ok(),
            "waiters wedged: single-flight fetch did not complete in time"
        );
        assert_eq!(inner.calls(), 20);
    }
}
