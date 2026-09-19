use arbitrageur_exchanges::extended_venues::ExtendedVenueAdapter;
use arbitrageur_core::exchange::ExchangeAdapter;

async fn probe(id: &str, sym: &str) {
    let a = ExtendedVenueAdapter::for_venue(id, None).unwrap();
    let mk = a.fetch_markets().await;
    match &mk {
        Ok(v) => println!("[{id}] markets={} e.g. {:?}", v.len(), v.iter().take(3).map(|m| m.venue_symbol.clone()).collect::<Vec<_>>()),
        Err(e) => println!("[{id}] MARKETS ERR: {e}"),
    }
    if mk.map(|v| !v.is_empty()).unwrap_or(false) {
        match a.fetch_orderbook(sym).await {
            Ok(b) => println!("[{id}] book {sym}: bids={} asks={} bb={:?} ba={:?}", b.bids.len(), b.asks.len(), b.best_bid_price(), b.best_ask_price()),
            Err(e) => println!("[{id}] BOOK ERR: {e}"),
        }
        match a.fetch_funding_rate(sym).await {
            Ok(f) => println!("[{id}] funding {sym}: rate={} pred={:?} next={}", f.rate, f.predicted_rate, f.next_funding_time),
            Err(e) => println!("[{id}] FUNDING ERR: {e}"),
        }
    }
}

// Live-network sweep (hits real public APIs, ~15s). Ignored by default;
// run explicitly: cargo test -p arbitrageur-exchanges --test live_probe -- --ignored --nocapture
#[tokio::test]
#[ignore]
async fn live_probe_all() {
    probe("apex", "BTC").await;
    probe("extended", "BTC").await;
    probe("derive", "ETH").await;
    probe("standx", "BTC").await;
    probe("helix", "BTC").await;
    probe("pacifica", "BTC").await;
    probe("qfex", "BTC").await;
}
