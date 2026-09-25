use std::collections::HashMap;
use std::time::Duration;

use crate::rate_limiter::AccountRequestCoordinator;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::AccountSnapshot;
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{dec, Decimal, VenueId};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;

/// Variational Omni connector — read-only market data.
///
/// The public API is a single `GET /metadata/stats` document: per-listing
/// mark price, 24h volume, long/short open interest, funding rate + interval,
/// and size-tiered executable quotes ($1k / $100k / $1m notional). All books
/// are quoted and settled in USDC on Arbitrum. The trading API is not yet
/// public (per docs.variational.io), so every write path fails closed —
/// admission is market-data only.
///
/// Rate limit: 10 requests / 10s per IP. `fetch_markets`, prices, books and
/// funding all fan out from the same stats document, so the rate limiter's
/// per-path response cache (4s TTL) coalesces concurrent pollers instead of
/// refetching per call.
pub struct VariationalAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    rate_limiter: AccountRequestCoordinator,
}

/// Notional tiers (USD) the venue publishes executable quotes for.
const QUOTE_TIERS: [(&str, f64); 3] = [
    ("size_1k", 1_000.0),
    ("size_100k", 100_000.0),
    ("size_1m", 1_000_000.0),
];

const STATS_PATH: &str = "/metadata/stats";

impl VariationalAdapter {
    pub fn new(base_url: Option<String>) -> Self {
        let base = base_url.unwrap_or_else(|| {
            "https://omni-client-api.prod.ap-northeast-1.variational.io".to_string()
        });
        Self {
            venue_id: VenueId::new("variational"),
            client: Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            base_url: base,
            // ~1 rps spacing with a 4s shared cache — far under 10 req/10s.
            rate_limiter: AccountRequestCoordinator::new("variational", 1000, 4000, 500, 10_000),
        }
    }

    /// The stats document — the limiter serves the cached body while fresh.
    async fn stats(&self) -> anyhow::Result<serde_json::Value> {
        if let Some(cached) = self
            .rate_limiter
            .acquire_or_cache_path(STATS_PATH, true)
            .await?
        {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&cached) {
                return Ok(v);
            }
        }
        let url = format!("{}{}", self.base_url, STATS_PATH);
        let resp = match self.client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                self.rate_limiter.record_failure(false);
                return Err(e.into());
            }
        };
        if !resp.status().is_success() {
            let is_429 = resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS;
            self.rate_limiter.record_failure(is_429);
            anyhow::bail!("Variational stats error: {}", resp.status());
        }
        let text = resp.text().await?;
        self.rate_limiter
            .record_success_path(STATS_PATH, text.clone());
        Ok(serde_json::from_str(&text)?)
    }

    fn listings(doc: &serde_json::Value) -> Vec<&serde_json::Value> {
        doc.get("listings")
            .and_then(|l| l.as_array())
            .map(|a| a.iter().collect())
            .unwrap_or_default()
    }

    fn listing_for<'a>(
        doc: &'a serde_json::Value,
        symbol: &str,
    ) -> anyhow::Result<&'a serde_json::Value> {
        let base = arbitrageur_core::symbol::canonical_base(symbol);
        Self::listings(doc)
            .into_iter()
            .find(|l| {
                l.get("ticker")
                    .and_then(|t| t.as_str())
                    .map(|t| t.eq_ignore_ascii_case(&base))
                    .unwrap_or(false)
            })
            .ok_or_else(|| anyhow::anyhow!("Variational lists no market for {symbol}"))
    }
}

#[async_trait]
impl ExchangeAdapter for VariationalAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Variational Omni"
    }

    fn rest_base_url(&self) -> Option<String> {
        Some(self.base_url.clone())
    }

    /// Latency probe must reach the venue — bypass the stats response cache.
    async fn ping(&self) -> anyhow::Result<()> {
        self.rate_limiter
            .acquire_or_cache_path(STATS_PATH, false)
            .await?;
        let url = format!("{}{}", self.base_url, STATS_PATH);
        let resp = match self.client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                self.rate_limiter.record_failure(false);
                return Err(e.into());
            }
        };
        if !resp.status().is_success() {
            let is_429 = resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS;
            self.rate_limiter.record_failure(is_429);
            anyhow::bail!("Variational ping error: {}", resp.status());
        }
        let text = resp.text().await?;
        // A fresh ping is real data — seed the shared cache with it.
        self.rate_limiter.record_success_path(STATS_PATH, text);
        Ok(())
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let doc = self.stats().await?;
        let mut markets = Vec::new();
        for l in Self::listings(&doc) {
            let Some(ticker) = l.get("ticker").and_then(|t| t.as_str()) else {
                continue;
            };
            let ticker = ticker.trim().to_uppercase();
            if ticker.is_empty() {
                continue;
            }
            markets.push(MarketMetadata {
                symbol: Symbol::perp(ticker.clone(), "USDC"),
                venue: self.venue_id.clone(),
                venue_symbol: ticker,
                price_decimals: 6,
                size_decimals: 8,
                min_order_size: dec!(0.0001),
                min_notional: dec!(1.0),
                // Omni charges zero taker/maker fees — revenue comes from the
                // integrated OLP liquidity provider's market-making yield.
                maker_fee_bps: dec!(0.0),
                taker_fee_bps: dec!(0.0),
                contract_size: dec!(1.0),
                is_active: true,
            });
        }
        if markets.is_empty() {
            anyhow::bail!("Variational stats returned no listings");
        }
        Ok(markets)
    }

    async fn fetch_market_prices(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        let doc = self.stats().await?;
        let mut out = HashMap::new();
        for l in Self::listings(&doc) {
            let Some(ticker) = l.get("ticker").and_then(|t| t.as_str()) else {
                continue;
            };
            if let Some(p) = dec_str(l.get("mark_price")).filter(|p| *p > dec!(0)) {
                out.insert(ticker.to_uppercase(), p);
            }
        }
        Ok(out)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let doc = self.stats().await?;
        let listing = Self::listing_for(&doc, symbol)?;
        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());
        book.timestamp = Utc::now();

        // The venue publishes one executable bid/ask per order-size tier —
        // not a level book. Converted to marginal depth levels: the $100k
        // tier's size is (100k − 1k) since the first $1k already sits at the
        // tighter quote; cumulative executable price is preserved exactly.
        let mut cum_notional = 0.0f64;
        for (tier, notional) in QUOTE_TIERS {
            let Some(q) = listing.get("quotes").and_then(|qs| qs.get(tier)) else {
                continue;
            };
            let marginal = notional - cum_notional;
            cum_notional = notional;
            if let Some(bid) = dec_str(q.get("bid")).filter(|p| *p > dec!(0)) {
                let qty = Decimal::from_f64_retain(marginal).unwrap_or(dec!(0)) / bid;
                book.bids.push(BookLevel::new(bid, qty));
            }
            if let Some(ask) = dec_str(q.get("ask")).filter(|p| *p > dec!(0)) {
                let qty = Decimal::from_f64_retain(marginal).unwrap_or(dec!(0)) / ask;
                book.asks.push(BookLevel::new(ask, qty));
            }
        }

        if book.bids.is_empty() && book.asks.is_empty() {
            anyhow::bail!("Variational returned no executable quotes for {symbol}");
        }
        Ok(book)
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let doc = self.stats().await?;
        let listing = Self::listing_for(&doc, symbol)?;
        let q1k = listing.get("quotes").and_then(|qs| qs.get("size_1k"));
        let bid = q1k
            .and_then(|q| dec_str(q.get("bid")))
            .filter(|p| *p > dec!(0))
            .unwrap_or(dec!(0));
        let ask = q1k
            .and_then(|q| dec_str(q.get("ask")))
            .filter(|p| *p > dec!(0))
            .unwrap_or(dec!(0));
        let last = dec_str(listing.get("mark_price"))
            .filter(|p| *p > dec!(0))
            .unwrap_or_else(|| (bid + ask) / dec!(2));
        Ok(Ticker {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            bid,
            ask,
            last,
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate> {
        let doc = self.stats().await?;
        let listing = Self::listing_for(&doc, symbol)?;
        // Fail-closed: a missing print is an outage, not a 0% print.
        let rate = dec_str(listing.get("funding_rate")).ok_or_else(|| {
            anyhow::anyhow!("Variational stats missing funding_rate for {symbol}")
        })?;
        let interval_s = listing
            .get("funding_interval_s")
            .and_then(|v| v.as_u64())
            .unwrap_or(28_800);
        let interval_hours = ((interval_s / 3600).max(1)) as u32;
        Ok(FundingRate {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            rate,
            predicted_rate: None,
            interval_hours,
            next_funding_time: Utc::now() + chrono::Duration::seconds(interval_s as i64),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rates(&self) -> anyhow::Result<Vec<FundingRate>> {
        let doc = self.stats().await?;
        let mut out = Vec::new();
        for l in Self::listings(&doc) {
            let (Some(ticker), Some(rate)) = (
                l.get("ticker").and_then(|t| t.as_str()),
                dec_str(l.get("funding_rate")),
            ) else {
                continue;
            };
            let interval_s = l
                .get("funding_interval_s")
                .and_then(|v| v.as_u64())
                .unwrap_or(28_800);
            let interval_hours = ((interval_s / 3600).max(1)) as u32;
            out.push(FundingRate {
                venue: self.venue_id.clone(),
                symbol: ticker.to_uppercase(),
                rate,
                predicted_rate: None,
                interval_hours,
                next_funding_time: Utc::now() + chrono::Duration::seconds(interval_s as i64),
                timestamp: Utc::now(),
            });
        }
        Ok(out)
    }

    async fn fetch_volumes_24h(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        let doc = self.stats().await?;
        let mut out = HashMap::new();
        for l in Self::listings(&doc) {
            let Some(ticker) = l.get("ticker").and_then(|t| t.as_str()) else {
                continue;
            };
            if let Some(v) = dec_str(l.get("volume_24h")).filter(|v| *v > dec!(0)) {
                out.insert(ticker.to_uppercase(), v);
            }
        }
        Ok(out)
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        anyhow::bail!(
            "Variational's trading API is not yet public — refusing to fabricate an account snapshot"
        )
    }

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        anyhow::bail!("Variational's trading API is not yet public — refusing to fabricate a fill")
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        anyhow::bail!("Variational's trading API is not yet public — nothing to cancel")
    }
}

/// String-or-number JSON decimal → `Decimal` (all stats fields arrive as
/// strings per the docs; accept numbers for forward-compat).
fn dec_str(v: Option<&serde_json::Value>) -> Option<Decimal> {
    match v? {
        serde_json::Value::String(s) => Decimal::from_str_exact(s.trim()).ok(),
        serde_json::Value::Number(n) => Decimal::from_str_exact(&n.to_string()).ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrageur_core::types::{OrderType, Side, TimeInForce};

    const STATS_FIXTURE: &str = r#"{
        "total_volume_24h": "1852892926.854888",
        "num_markets": 2,
        "listings": [
            {
                "ticker": "BTC",
                "name": "Bitcoin",
                "mark_price": "93787.9606019699",
                "volume_24h": "1058107020.462713",
                "open_interest": {
                    "long_open_interest": "113883049.01452301878687056000",
                    "short_open_interest": "82403040.51901045963533924000"
                },
                "funding_rate": "0.037347",
                "funding_interval_s": 28800,
                "base_spread_bps": "0.4307589134116585963643440000",
                "quotes": {
                    "updated_at": "2026-01-06T06:38:52.476166127Z",
                    "size_1k": { "bid": "93750.97", "ask": "93755.01" },
                    "size_100k": { "bid": "93746.13", "ask": "93759.85" },
                    "size_1m": { "bid": "93718.95", "ask": "93779.82" }
                }
            },
            {
                "ticker": "ETH",
                "name": "Ethereum",
                "mark_price": "3101.5",
                "volume_24h": "50000000",
                "funding_rate": "-0.001",
                "funding_interval_s": 3600,
                "quotes": {
                    "size_1k": { "bid": "3100.9", "ask": "3102.1" }
                }
            }
        ]
    }"#;

    fn adapter() -> VariationalAdapter {
        VariationalAdapter::new(Some("http://127.0.0.1:1".into()))
    }

    fn warm(a: &VariationalAdapter) {
        // Seed the limiter path-cache with the fixture — no network needed.
        a.rate_limiter
            .record_success_path(STATS_PATH, STATS_FIXTURE.to_string());
    }

    #[tokio::test]
    async fn markets_listings_parse_as_usdc_perps() {
        let a = adapter();
        warm(&a);
        let markets = a.fetch_markets().await.unwrap();
        assert_eq!(markets.len(), 2);
        let btc = markets.iter().find(|m| m.venue_symbol == "BTC").unwrap();
        assert_eq!(btc.symbol.canonical(), "BTC-USDC");
        assert_eq!(btc.taker_fee_bps, dec!(0));
        assert!(btc.is_active);
    }

    #[tokio::test]
    async fn prices_map_keys_by_venue_ticker() {
        let a = adapter();
        warm(&a);
        let prices = a.fetch_market_prices().await.unwrap();
        assert_eq!(prices.get("BTC").unwrap().to_string(), "93787.9606019699");
        assert_eq!(prices.get("ETH").unwrap().to_string(), "3101.5");
    }

    #[tokio::test]
    async fn orderbook_builds_marginal_depth_from_quote_tiers() {
        let a = adapter();
        warm(&a);
        let book = a.fetch_orderbook("BTC").await.unwrap();
        assert_eq!(book.bids.len(), 3);
        assert_eq!(book.asks.len(), 3);
        // Best bid is the tightest (1k) tier.
        assert_eq!(book.bids[0].price.to_string(), "93750.97");
        // Marginal sizing: tier2 covers the notional above tier1.
        let l2_qty: f64 = book.bids[1].size.to_string().parse().unwrap();
        let expected = 99_000.0 / 93746.13;
        assert!((l2_qty - expected).abs() / expected < 1e-9);
        // Tiers deepen monotonically outward from mid.
        assert!(book.bids[1].price < book.bids[0].price);
        assert!(book.asks[1].price > book.asks[0].price);
    }

    #[tokio::test]
    async fn orderbook_accepts_pair_and_bare_spellings() {
        let a = adapter();
        warm(&a);
        for sym in ["BTC", "BTC-USDC", "btc-usdt"] {
            assert!(a.fetch_orderbook(sym).await.is_ok(), "{sym} should resolve");
        }
        assert!(a.fetch_orderbook("NOPE").await.is_err());
    }

    #[tokio::test]
    async fn funding_rates_carry_interval_seconds() {
        let a = adapter();
        warm(&a);
        let rates = a.fetch_funding_rates().await.unwrap();
        assert_eq!(rates.len(), 2);
        let btc = rates.iter().find(|r| r.symbol == "BTC").unwrap();
        assert_eq!(btc.rate.to_string(), "0.037347");
        assert_eq!(btc.interval_hours, 8);
        let eth = rates.iter().find(|r| r.symbol == "ETH").unwrap();
        assert_eq!(eth.interval_hours, 1);
    }

    #[tokio::test]
    async fn write_paths_fail_closed() {
        let a = adapter();
        assert!(a.fetch_account().await.is_err());
        assert!(a
            .place_order(OrderRequest {
                client_order_id: "t".into(),
                venue: a.venue_id(),
                symbol: "BTC".into(),
                side: Side::Buy,
                order_type: OrderType::Market,
                price: None,
                quantity: dec!(1),
                time_in_force: TimeInForce::ImmediateOrCancel,
                post_only: false,
                reduce_only: false,
                created_at: Utc::now(),
            })
            .await
            .is_err());
    }

    #[tokio::test]
    async fn volumes_keyed_by_ticker() {
        let a = adapter();
        warm(&a);
        let vols = a.fetch_volumes_24h().await.unwrap();
        assert!(vols.get("BTC").unwrap() > &dec!(0));
    }
}
