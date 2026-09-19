//! Variational Omni perpetuals connector.
//!
//! # Connectivity model (researched 2026-09-19)
//!
//! Variational is an RFQ protocol — there is deliberately **no orderbook and
//! no API-key trading API**. Connectivity splits in two:
//!
//! - **Market data (public, no auth):** `GET {base}/metadata/stats`
//!   (documented at <https://docs.variational.io/technical-documentation/api>)
//!   returns every listing with mark price, OLP indicative quotes at
//!   `$1k`/`$100k`/`$1m` notionals, funding rate + interval, 24h volume and
//!   open interest. Rate limit is 10 req / 10s per IP, so responses are
//!   cached for 60s and shared across concurrent scans.
//! - **Execution (wallet-signed, on-chain):** orders are RFQ quotes accepted
//!   by the taker and settled on Arbitrum (contracts in docs:
//!   `mainnet-contracts`). There is no key/secret flow; the venue needs a
//!   locally-held EVM key (see [`crate::wallet`]). Until the RFQ
//!   quote-acceptance + settlement flow is implemented and audited, order
//!   paths fail closed via [`crate::wallet::wallet_execution_unavailable`].
//!
//! # Depth honesty
//!
//! `fetch_orderbook` synthesizes indicative levels from the three RFQ quote
//! tiers (size = tier notional / tier price). These are OLP *indicative*
//! quotes subject to maker last-look — not firm L2 depth — and are marked
//! with the quote timestamp. Capacity math downstream must treat them as
//! indicative.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::Utc;
use parking_lot::RwLock;
use reqwest::Client;
use serde::Deserialize;

use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::AccountSnapshot;
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{dec, Decimal, VenueId};

use crate::wallet::{wallet_execution_unavailable, EvmWalletSigner};

/// Verified public read-only host (Variational docs, "Read-Only API").
pub const VARIATIONAL_DEFAULT_BASE_URL: &str =
    "https://omni-client-api.prod.ap-northeast-1.variational.io";
const STATS_PATH: &str = "/metadata/stats";
/// Cache TTL: comfortably inside the 10 req / 10s per-IP limit while keeping
/// scan data fresh. Concurrent scans share one in-flight fetch.
const STATS_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Deserialize)]
struct StatsQuote {
    #[serde(default)]
    bid: Option<String>,
    #[serde(default)]
    ask: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct StatsQuotes {
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    base: Option<StatsQuote>,
    #[serde(default)]
    size_1k: Option<StatsQuote>,
    #[serde(default)]
    size_100k: Option<StatsQuote>,
    #[serde(default)]
    size_1m: Option<StatsQuote>,
}

#[derive(Debug, Clone, Deserialize)]
struct StatsListing {
    #[serde(default)]
    ticker: String,
    #[serde(default)]
    mark_price: Option<String>,
    #[serde(default)]
    volume_24h: Option<String>,
    #[serde(default)]
    funding_rate: Option<String>,
    #[serde(default)]
    funding_interval_s: Option<u64>,
    #[serde(default)]
    quotes: Option<StatsQuotes>,
}

#[derive(Debug, Clone, Deserialize)]
struct StatsPayload {
    #[serde(default)]
    listings: Vec<StatsListing>,
}

fn parse_dec(raw: Option<&str>) -> Option<Decimal> {
    let s = raw?.trim();
    if s.is_empty() {
        return None;
    }
    Decimal::from_str(s).ok().filter(|d| d.is_sign_positive() && !d.is_zero())
}

/// Normalize any caller-side symbol spelling to the venue's bare ticker
/// (`"BTC"`): accepts `"BTC-PERP"`, `"BTC/USDC"`, `"BTC-PERP"` venue symbols,
/// `"BTC:PERP"` display forms, any case.
fn to_ticker(symbol: &str) -> String {
    let mut s = symbol.trim().to_uppercase();
    for suffix in ["-PERP", ":PERP", "/USDC", "-USDC", ":USDC"] {
        if let Some(stripped) = s.strip_suffix(suffix) {
            s = stripped.to_string();
        }
    }
    // Canonical raw form is "BASE-QUOTE" (e.g. "BTC-USDC").
    if let Some((base, _)) = s.split_once('-') {
        if !base.is_empty() {
            return base.to_string();
        }
    }
    if let Some((base, _)) = s.split_once('/') {
        if !base.is_empty() {
            return base.to_string();
        }
    }
    s
}

fn price_decimals_for(mark: Option<Decimal>) -> u32 {
    match mark {
        Some(m) if m >= dec!(1000) => 2,
        Some(m) if m >= dec!(10) => 3,
        _ => 4,
    }
}

/// Variational Omni-Perp connector (public market data + wallet-gated RFQ stub).
pub struct VariationalAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    wallet_address: RwLock<Option<String>>,
    wallet: RwLock<Option<EvmWalletSigner>>,
    stats_cache: tokio::sync::RwLock<Option<(Instant, Arc<StatsPayload>)>>,
}

impl VariationalAdapter {
    pub fn new(
        base_url: Option<String>,
        api_key: Option<String>,
        api_secret: Option<String>,
    ) -> Self {
        // NOTE: Variational has no API-key auth at all; key/secret params are
        // accepted only for constructor symmetry and ignored. A wallet key
        // (EVM hex) is loaded via `update_credentials`.
        let _ = (api_key, api_secret);
        let base = base_url
            .map(|b| b.trim().trim_end_matches('/').to_string())
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| VARIATIONAL_DEFAULT_BASE_URL.to_string());
        Self {
            venue_id: VenueId::new("variational"),
            client: Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            base_url: base,
            wallet_address: RwLock::new(None),
            wallet: RwLock::new(None),
            stats_cache: tokio::sync::RwLock::new(None),
        }
    }

    /// Bind (or clear) the locally-held EVM wallet used for future RFQ
    /// signing. The hex key is parsed into a zeroizing signer immediately;
    /// the raw string is never retained.
    pub fn update_credentials(
        &self,
        wallet_address: Option<String>,
        private_key_hex: Option<String>,
    ) -> anyhow::Result<()> {
        let addr = wallet_address
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty());
        match private_key_hex
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
        {
            Some(hex_key) => {
                let signer = EvmWalletSigner::from_hex(&hex_key)?;
                // If the caller also gave an address, it must match the key.
                if let Some(a) = addr.as_deref() {
                    if !a.eq_ignore_ascii_case(&signer.address_hex()) {
                        anyhow::bail!("Variational wallet address does not match the supplied private key");
                    }
                }
                *self.wallet.write() = Some(signer);
                *self.wallet_address.write() = addr;
                Ok(())
            }
            None => {
                *self.wallet.write() = None;
                *self.wallet_address.write() = addr;
                Ok(())
            }
        }
    }

    async fn fetch_stats(&self) -> anyhow::Result<Arc<StatsPayload>> {
        {
            let guard = self.stats_cache.read().await;
            if let Some((at, payload)) = guard.as_ref() {
                if at.elapsed() < STATS_TTL {
                    return Ok(Arc::clone(payload));
                }
            }
        }
        let url = format!("{}{}", self.base_url, STATS_PATH);
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Variational stats API error: {} {}", resp.status(), STATS_PATH);
        }
        let payload: StatsPayload = resp.json().await?;
        if payload.listings.is_empty() {
            anyhow::bail!("Variational stats returned no listings");
        }
        let shared = Arc::new(payload);
        *self.stats_cache.write().await = Some((Instant::now(), Arc::clone(&shared)));
        Ok(shared)
    }

    fn find_listing<'a>(stats: &'a StatsPayload, symbol: &str) -> Option<&'a StatsListing> {
        let want = to_ticker(symbol);
        stats
            .listings
            .iter()
            .find(|l| l.ticker.eq_ignore_ascii_case(&want))
    }

    /// (tier notional USD, quote) in widening order.
    fn depth_tiers(quotes: &StatsQuotes) -> Vec<(Decimal, &StatsQuote)> {
        let mut tiers = Vec::new();
        if let Some(q) = quotes.size_1k.as_ref() {
            tiers.push((dec!(1_000), q));
        }
        if let Some(q) = quotes.size_100k.as_ref() {
            tiers.push((dec!(100_000), q));
        }
        if let Some(q) = quotes.size_1m.as_ref() {
            tiers.push((dec!(1_000_000), q));
        }
        tiers
    }

    fn quote_timestamp(quotes: &StatsQuotes) -> chrono::DateTime<Utc> {
        quotes
            .updated_at
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now)
    }
}

#[async_trait]
impl ExchangeAdapter for VariationalAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Variational"
    }

    async fn ping(&self) -> anyhow::Result<()> {
        // The stats endpoint is the only public surface; a cached hit still
        // proves the venue reachable within the last minute, otherwise it
        // performs a live request with the client's 15s timeout.
        self.fetch_stats().await.map(|_| ())
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let stats = self.fetch_stats().await?;
        let mut markets = Vec::with_capacity(stats.listings.len());
        for l in stats.listings.iter() {
            if l.ticker.trim().is_empty() {
                continue;
            }
            let base = l.ticker.trim().to_uppercase();
            let mark = parse_dec(l.mark_price.as_deref());
            markets.push(MarketMetadata {
                symbol: Symbol::perp(&base, "USDC"),
                venue: self.venue_id.clone(),
                venue_symbol: format!("{base}-PERP"),
                price_decimals: price_decimals_for(mark),
                size_decimals: 4,
                min_order_size: dec!(0.001),
                min_notional: dec!(5.0),
                // Documented: no trading fees on Omni — OLP captures spread.
                maker_fee_bps: dec!(0),
                taker_fee_bps: dec!(0),
                contract_size: dec!(1.0),
                is_active: true,
            });
        }
        if markets.is_empty() {
            anyhow::bail!("Variational stats returned no tradeable listings");
        }
        Ok(markets)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let stats = self.fetch_stats().await?;
        let listing = Self::find_listing(&stats, symbol)
            .ok_or_else(|| anyhow::anyhow!("Variational has no listing for {symbol}"))?;
        let quotes = listing
            .quotes
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Variational quotes unavailable for {symbol}"))?;
        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());
        for (notional, q) in Self::depth_tiers(quotes) {
            let (Some(bid), Some(ask)) = (
                parse_dec(q.bid.as_deref()),
                parse_dec(q.ask.as_deref()),
            ) else {
                continue;
            };
            if !(bid > dec!(0) && ask > bid) {
                continue;
            }
            // Indicative tier size: notional value at the tier's own price.
            let bid_size = notional / bid;
            let ask_size = notional / ask;
            book.bids.push(BookLevel::new(bid, bid_size));
            book.asks.push(BookLevel::new(ask, ask_size));
        }
        if book.bids.is_empty() && book.asks.is_empty() {
            anyhow::bail!("Variational RFQ quotes empty for {symbol}");
        }
        book.bids.sort_by_key(|b| std::cmp::Reverse(b.price));
        book.asks.sort_by_key(|a| a.price);
        book.timestamp = Self::quote_timestamp(quotes);
        Ok(book)
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let stats = self.fetch_stats().await?;
        let listing = Self::find_listing(&stats, symbol)
            .ok_or_else(|| anyhow::anyhow!("Variational has no listing for {symbol}"))?;
        let (bid, ask) = listing
            .quotes
            .as_ref()
            .and_then(|q| q.base.as_ref())
            .and_then(|b| Some((parse_dec(b.bid.as_deref())?, parse_dec(b.ask.as_deref())?)))
            .ok_or_else(|| anyhow::anyhow!("Variational top quote unavailable for {symbol}"))?;
        let mark = parse_dec(listing.mark_price.as_deref()).unwrap_or((bid + ask) / dec!(2));
        Ok(Ticker {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            bid,
            ask,
            last: mark,
            timestamp: listing
                .quotes
                .as_ref()
                .map(Self::quote_timestamp)
                .unwrap_or_else(Utc::now),
        })
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate> {
        let stats = self.fetch_stats().await?;
        let listing = Self::find_listing(&stats, symbol)
            .ok_or_else(|| anyhow::anyhow!("Variational has no listing for {symbol}"))?;
        let rate = listing
            .funding_rate
            .as_deref()
            .and_then(|s| Decimal::from_str(s.trim()).ok())
            .ok_or_else(|| anyhow::anyhow!("Variational funding rate unavailable for {symbol}"))?;
        let interval_hours = listing
            .funding_interval_s
            .map(|s| ((s + 1800) / 3600).max(1) as u32)
            .unwrap_or(8);
        Ok(FundingRate {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            rate,
            predicted_rate: None,
            interval_hours,
            next_funding_time: Utc::now() + chrono::Duration::hours(interval_hours as i64),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rates(&self) -> anyhow::Result<Vec<FundingRate>> {
        // Bulk: one cached call covers all ~550 listings.
        let stats = self.fetch_stats().await?;
        let now = Utc::now();
        let mut out = Vec::new();
        for l in stats.listings.iter() {
            let Some(rate) = l
                .funding_rate
                .as_deref()
                .and_then(|s| Decimal::from_str(s.trim()).ok())
            else {
                continue;
            };
            let interval_hours = l
                .funding_interval_s
                .map(|s| ((s + 1800) / 3600).max(1) as u32)
                .unwrap_or(8);
            out.push(FundingRate {
                venue: self.venue_id.clone(),
                symbol: format!("{}-PERP", l.ticker.trim().to_uppercase()),
                rate,
                predicted_rate: None,
                interval_hours,
                next_funding_time: now + chrono::Duration::hours(interval_hours as i64),
                timestamp: now,
            });
        }
        Ok(out)
    }

    async fn fetch_volumes_24h(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        let stats = self.fetch_stats().await?;
        let mut volumes = HashMap::new();
        for l in stats.listings.iter() {
            let name = l.ticker.trim().to_uppercase();
            if name.is_empty() {
                continue;
            }
            if let Some(vol) = l
                .volume_24h
                .as_deref()
                .and_then(|s| Decimal::from_str(s.trim()).ok())
            {
                volumes.insert(name, vol);
            }
        }
        Ok(volumes)
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        anyhow::bail!(
            "Variational positions live in on-chain settlement pools, not behind an API: \
             connect a wallet and read pool state via Arbitrum RPC. Refusing to fabricate an account snapshot"
        )
    }

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        Err(wallet_execution_unavailable(
            "Variational",
            "Arbitrum",
            "taker-accepts-OLP-RFQ-quote then pool settlement with maker last-look",
        ))
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        Err(wallet_execution_unavailable(
            "Variational",
            "Arbitrum",
            "RFQ request withdrawal before OLP last-look",
        ))
    }

    fn supports_live_execution(&self) -> bool {
        false
    }

    fn has_live_credentials(&self) -> bool {
        self.wallet.read().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "total_volume_24h": "2666806584.169264",
        "num_markets": 2,
        "listings": [
            {
                "ticker": "BTC",
                "name": "Bitcoin",
                "mark_price": "93787.9606019699",
                "volume_24h": "1058107020.462713",
                "funding_rate": "0.000123",
                "funding_interval_s": 28800,
                "base_spread_bps": "0.43",
                "quotes": {
                    "updated_at": "2026-09-18T17:44:44.438301143Z",
                    "base": {"bid": "93750.97", "ask": "93755.01"},
                    "size_1k": {"bid": "93746.13", "ask": "93759.85"},
                    "size_100k": {"bid": "93718.95", "ask": "93779.82"}
                }
            },
            {
                "ticker": "UMA",
                "name": "UMA",
                "mark_price": "0.3854632707640781",
                "volume_24h": "1.963520",
                "funding_rate": "0.1095",
                "funding_interval_s": 14400,
                "quotes": {
                    "updated_at": "2026-09-18T17:44:44.438301143Z",
                    "base": {"bid": "0.385", "ask": "0.386"},
                    "size_1k": {"bid": "0.3848", "ask": "0.3862"},
                    "size_100k": {"bid": "0.3761", "ask": "0.4037"}
                }
            }
        ]
    }"#;

    fn sample_stats() -> Arc<StatsPayload> {
        Arc::new(serde_json::from_str(SAMPLE).unwrap())
    }

    #[test]
    fn parses_listings_shape() {
        let stats = sample_stats();
        assert_eq!(stats.listings.len(), 2);
        assert_eq!(stats.listings[0].ticker, "BTC");
        assert_eq!(stats.listings[0].funding_interval_s, Some(28800));
    }

    #[test]
    fn ticker_normalization() {
        for (input, want) in [
            ("BTC-PERP", "BTC"),
            ("btc-perp", "BTC"),
            ("BTC/USDC", "BTC"),
            ("BTC-USDC", "BTC"),
            ("BTC:PERP", "BTC"),
            ("BTC", "BTC"),
            ("uma-perp", "UMA"),
        ] {
            assert_eq!(to_ticker(input), want, "input {input}");
        }
    }

    #[tokio::test]
    async fn markets_funding_volumes_from_sample() {
        let adapter = VariationalAdapter::new(None, None, None);
        *adapter.stats_cache.write().await = Some((Instant::now(), sample_stats()));

        let markets = adapter.fetch_markets().await.unwrap();
        assert_eq!(markets.len(), 2);
        assert_eq!(markets[0].venue_symbol, "BTC-PERP");
        assert_eq!(markets[0].taker_fee_bps, dec!(0));
        assert_eq!(markets[0].price_decimals, 2);
        assert_eq!(markets[1].price_decimals, 4);

        let ticker = adapter.fetch_ticker("BTC-PERP").await.unwrap();
        assert_eq!(ticker.bid, Decimal::from_str("93750.97").unwrap());
        assert_eq!(ticker.ask, Decimal::from_str("93755.01").unwrap());

        let funding = adapter.fetch_funding_rate("btc/usdc").await.unwrap();
        assert_eq!(funding.rate, Decimal::from_str("0.000123").unwrap());
        assert_eq!(funding.interval_hours, 8);
        let all = adapter.fetch_funding_rates().await.unwrap();
        assert_eq!(all.len(), 2);
        // 14400s -> 4h buckets.
        assert_eq!(all[1].interval_hours, 4);

        let vols = adapter.fetch_volumes_24h().await.unwrap();
        assert_eq!(vols["BTC"], Decimal::from_str("1058107020.462713").unwrap());
    }

    #[tokio::test]
    async fn orderbook_synthesizes_indicative_tiers() {
        let adapter = VariationalAdapter::new(None, None, None);
        *adapter.stats_cache.write().await = Some((Instant::now(), sample_stats()));

        let book = adapter.fetch_orderbook("BTC-PERP").await.unwrap();
        // Two tiers present in the sample (no size_1m for BTC here).
        assert_eq!(book.bids.len(), 2);
        assert_eq!(book.asks.len(), 2);
        // Best level first, tier notionals priced in.
        assert!(book.bids[0].price > book.bids[1].price);
        assert!(book.asks[0].price < book.asks[1].price);
        let expect_ask0 = dec!(1_000) / Decimal::from_str("93759.85").unwrap();
        assert_eq!(book.asks[0].size, expect_ask0);
        // Quote timestamp preserved (not "now").
        assert_eq!(
            book.timestamp.to_rfc3339(),
            "2026-09-18T17:44:44.438301143+00:00"
        );
        assert!(adapter.fetch_orderbook("NOPE-PERP").await.is_err());
    }

    #[tokio::test]
    async fn execution_fails_closed_with_wallet_guidance() {
        let adapter = VariationalAdapter::new(None, None, None);
        assert!(!adapter.supports_live_execution());
        assert!(!adapter.has_live_credentials());
        let err = adapter
            .place_order(OrderRequest::new_limit(
                adapter.venue_id(),
                "BTC-PERP",
                arbitrageur_core::types::Side::Buy,
                dec!(90000),
                dec!(0.01),
            ))
            .await
            .unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("no API-key trading"), "{msg}");
        assert!(adapter.fetch_account().await.is_err());
    }

    /// Live shape check against the real public stats API. Ignored by
    /// default (network); run explicitly to re-verify the documented
    /// response contract after upstream changes.
    #[tokio::test]
    #[ignore]
    async fn live_stats_shape() {
        let adapter = VariationalAdapter::new(None, None, None);
        let stats = adapter.fetch_stats().await.unwrap();
        assert!(stats.listings.len() > 100, "expected hundreds of listings");
        let btc = stats.listings.iter().find(|l| l.ticker == "BTC").unwrap();
        assert!(parse_dec(btc.mark_price.as_deref()).is_some());
        assert!(parse_dec(btc.funding_rate.as_deref()).is_some());
        let quotes = btc.quotes.as_ref().unwrap();
        let top = quotes.base.as_ref().unwrap();
        let bid = parse_dec(top.bid.as_deref()).unwrap();
        let ask = parse_dec(top.ask.as_deref()).unwrap();
        assert!(bid > dec!(0) && ask > bid);

        let markets = adapter.fetch_markets().await.unwrap();
        assert_eq!(markets.len(), stats.listings.len());
        let book = adapter.fetch_orderbook("BTC-PERP").await.unwrap();
        assert!(!book.bids.is_empty() && !book.asks.is_empty());
        let funding = adapter.fetch_funding_rate("BTC-PERP").await.unwrap();
        assert!(funding.interval_hours >= 1);
        let vols = adapter.fetch_volumes_24h().await.unwrap();
        assert!(vols.contains_key("BTC"));
    }

    #[test]
    fn wallet_binding_validates_address_match() {
        let adapter = VariationalAdapter::new(None, None, None);
        let key = "0000000000000000000000000000000000000000000000000000000000000001";
        adapter
            .update_credentials(
                Some("0x7e5f4552091a69125d5dfcb7b8c2659029395bdf".into()),
                Some(key.into()),
            )
            .unwrap();
        assert!(adapter.has_live_credentials());
        // Mismatched address must not bind.
        assert!(adapter
            .update_credentials(
                Some("0x0000000000000000000000000000000000000001".into()),
                Some(key.into())
            )
            .is_err());
        // Clearing the key drops credentials.
        adapter.update_credentials(None, None).unwrap();
        assert!(!adapter.has_live_credentials());
    }
}
