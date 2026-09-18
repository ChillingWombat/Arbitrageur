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

/// Arcus High-Speed CLOB Perpetual DEX connector.
#[allow(dead_code)]
pub struct ArcusAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    api_key: Option<String>,
    api_secret: Option<String>,
    rate_limiter: AccountRequestCoordinator,
}

impl ArcusAdapter {
    pub fn new(
        base_url: Option<String>,
        api_key: Option<String>,
        api_secret: Option<String>,
    ) -> Self {
        // api.arcus.exchange does not resolve; the live host is
        // api.arcus.xyz (verified via /health, 2026-09).
        let base = base_url.unwrap_or_else(|| "https://api.arcus.xyz".to_string());
        Self {
            venue_id: VenueId::new("arcus"),
            client: Client::builder().timeout(std::time::Duration::from_secs(15)).build().unwrap_or_default(),
            base_url: base,
            api_key,
            api_secret,
            rate_limiter: AccountRequestCoordinator::new("arcus", 100, 2000, 500, 10000),
        }
    }

    #[allow(dead_code)]
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
            anyhow::bail!("Arcus API error: {} {}", resp.status(), path);
        }

        let text = resp.text().await?;
        self.rate_limiter.record_success_path(path, text.clone());
        let data = serde_json::from_str::<serde_json::Value>(&text)?;
        Ok(data)
    }
}

#[async_trait]
impl ExchangeAdapter for ArcusAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Arcus"
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let mut markets = Vec::new();
        let common_pairs = ["BTC", "ETH", "SOL", "AVAX", "BNB", "ARB", "OP", "SUI", "NEAR", "APT", "LINK", "DOGE"];
        for base in common_pairs {
            markets.push(MarketMetadata {
                symbol: Symbol::perp(base, "USDC"),
                venue: self.venue_id.clone(),
                venue_symbol: format!("{}-PERP", base),
                price_decimals: if base == "BTC" || base == "ETH" { 2 } else { 4 },
                size_decimals: 3,
                min_order_size: dec!(0.001),
                min_notional: dec!(10.0),
                maker_fee_bps: dec!(0.0),
                taker_fee_bps: dec!(1.5),
                contract_size: dec!(1.0),
                is_active: true,
            });
        }
        Ok(markets)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let key = self.api_key.as_deref().unwrap_or("").trim();
        if key.is_empty() {
            anyhow::bail!("Arcus API key not configured");
        }

        let clean_sym = symbol.replace('/', "-").to_uppercase();
        let path = format!("/api/v1/orderbook?symbol={}", clean_sym);
        let res = self.get_json(&path).await?;

        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());
        book.timestamp = Utc::now();

        if let Some(bids) = res.get("bids").and_then(|b| b.as_array()) {
            for b in bids {
                if let (Some(p_val), Some(s_val)) = (b.get(0), b.get(1)) {
                    let p_str = p_val.as_str().unwrap_or("");
                    let s_str = s_val.as_str().unwrap_or("");
                    if let (Ok(p), Ok(s)) = (Decimal::from_str_exact(p_str), Decimal::from_str_exact(s_str)) {
                        if p > dec!(0) && s > dec!(0) {
                            book.bids.push(BookLevel::new(p, s));
                        }
                    }
                }
            }
        }
        if let Some(asks) = res.get("asks").and_then(|a| a.as_array()) {
            for a in asks {
                if let (Some(p_val), Some(s_val)) = (a.get(0), a.get(1)) {
                    let p_str = p_val.as_str().unwrap_or("");
                    let s_str = s_val.as_str().unwrap_or("");
                    if let (Ok(p), Ok(s)) = (Decimal::from_str_exact(p_str), Decimal::from_str_exact(s_str)) {
                        if p > dec!(0) && s > dec!(0) {
                            book.asks.push(BookLevel::new(p, s));
                        }
                    }
                }
            }
        }

        if book.bids.is_empty() && book.asks.is_empty() {
            anyhow::bail!("No live orderbook data returned from Arcus API for {}", symbol);
        }

        Ok(book)
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let book = self.fetch_orderbook(symbol).await?;
        let bid = book.best_bid_price().unwrap_or(dec!(0));
        let ask = book.best_ask_price().unwrap_or(dec!(0));
        Ok(Ticker {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            bid,
            ask,
            last: (bid + ask) / dec!(2),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate> {
        let key = self.api_key.as_deref().unwrap_or("").trim();
        if key.is_empty() {
            anyhow::bail!("Arcus API key not configured");
        }
        let clean_sym = symbol.replace('/', "-").to_uppercase();
        let path = format!("/api/v1/funding?symbol={}", clean_sym);
        let res = self.get_json(&path).await?;
        let rate_f = res.get("funding_rate").and_then(|r| r.as_f64()).unwrap_or(0.0);
        let rate_dec = Decimal::from_f64_retain(rate_f).unwrap_or(dec!(0));
        Ok(FundingRate {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            rate: rate_dec,
            predicted_rate: None,
            interval_hours: 8,
            next_funding_time: Utc::now() + chrono::Duration::hours(8),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        anyhow::bail!("Live account fetch is not implemented for {}; refusing to fabricate an account snapshot", self.name())
    }

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        anyhow::bail!("Live order execution is not implemented for {}; refusing to fabricate a fill", self.name())
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        anyhow::bail!("Live order cancellation is not implemented for {}", self.name())
    }
}
