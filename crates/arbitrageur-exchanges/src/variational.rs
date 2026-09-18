use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::AccountSnapshot;
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{dec, Decimal, VenueId};
use crate::rate_limiter::AccountRequestCoordinator;

/// Variational Omni-Perp connector.
#[allow(dead_code)]
pub struct VariationalAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    api_key: Option<String>,
    api_secret: Option<String>,
    rate_limiter: AccountRequestCoordinator,
}

impl VariationalAdapter {
    pub fn new(
        base_url: Option<String>,
        api_key: Option<String>,
        api_secret: Option<String>,
    ) -> Self {
        let base = base_url.unwrap_or_else(|| "https://api.variational.io".to_string());
        Self {
            venue_id: VenueId::new("variational"),
            client: {
                let mut headers = reqwest::header::HeaderMap::new();
                headers.insert(
                    reqwest::header::USER_AGENT,
                    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36".parse().unwrap(),
                );
                headers.insert(
                    reqwest::header::ACCEPT,
                    "application/json, text/plain, */*".parse().unwrap(),
                );
                Client::builder()
                    .default_headers(headers)
                    .build()
                    .unwrap_or_default()
            },
            base_url: base,
            api_key,
            api_secret,
            rate_limiter: AccountRequestCoordinator::new("variational", 100, 2000, 500, 10000),
        }
    }

    async fn get_json(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        if let Some(cached_str) = self.rate_limiter.acquire_or_cache_path(path, true).await? {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&cached_str) {
                return Ok(val);
            }
        }

        let url = format!("{}{}", self.base_url, path);
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
            anyhow::bail!("Variational API error: {} {}", resp.status(), path);
        }

        let text = resp.text().await?;
        self.rate_limiter.record_success_path(path, text.clone());
        let data = serde_json::from_str::<serde_json::Value>(&text)?;
        Ok(data)
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
        let resp = self.client.get(&format!("{}/health", self.base_url)).send().await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            anyhow::bail!("Variational health probe returned status {}", resp.status());
        }
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let mut markets = Vec::new();
        let common_pairs = ["BTC", "ETH", "SOL", "AVAX", "ARB", "OP", "SUI", "LINK", "DOGE"];
        for base in common_pairs {
            markets.push(MarketMetadata {
                symbol: Symbol::perp(base, "USDC"),
                venue: self.venue_id.clone(),
                venue_symbol: format!("{}-PERP", base),
                price_decimals: if base == "BTC" || base == "ETH" { 2 } else { 4 },
                size_decimals: 3,
                min_order_size: dec!(0.001),
                min_notional: dec!(5.0),
                maker_fee_bps: dec!(0.0),
                taker_fee_bps: dec!(2.0),
                contract_size: dec!(1.0),
                is_active: true,
            });
        }
        Ok(markets)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let path = format!("/api/v1/orderbook?symbol={}", symbol);
        let val = self.get_json(&path).await?;
        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());
        if let Some(bids) = val.get("bids").and_then(|b| b.as_array()) {
            for b in bids {
                if let (Some(px), Some(sz)) = (
                    b.get(0).and_then(|v| v.as_str()).and_then(|s| s.parse::<Decimal>().ok()),
                    b.get(1).and_then(|v| v.as_str()).and_then(|s| s.parse::<Decimal>().ok()),
                ) {
                    book.bids.push(BookLevel::new(px, sz));
                }
            }
        }
        if let Some(asks) = val.get("asks").and_then(|a| a.as_array()) {
            for a in asks {
                if let (Some(px), Some(sz)) = (
                    a.get(0).and_then(|v| v.as_str()).and_then(|s| s.parse::<Decimal>().ok()),
                    a.get(1).and_then(|v| v.as_str()).and_then(|s| s.parse::<Decimal>().ok()),
                ) {
                    book.asks.push(BookLevel::new(px, sz));
                }
            }
        }
        if book.bids.is_empty() && book.asks.is_empty() {
            anyhow::bail!("Variational orderbook empty or offline for {}", symbol);
        }
        book.timestamp = Utc::now();
        Ok(book)
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let book = self.fetch_orderbook(symbol).await?;
        let bid = book.best_bid_price().unwrap_or(dec!(0));
        let ask = book.best_ask_price().unwrap_or(dec!(0));
        let last = (bid + ask) / dec!(2);
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
        Ok(FundingRate {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            rate: dec!(0.0001),
            predicted_rate: None,
            interval_hours: 8,
            next_funding_time: Utc::now(),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        anyhow::bail!("Live account query not configured for Variational")
    }

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        anyhow::bail!("Live trading execution not configured for Variational")
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        anyhow::bail!("Live trading cancellation not configured for Variational")
    }
}
