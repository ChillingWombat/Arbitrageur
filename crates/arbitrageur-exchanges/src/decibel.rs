use async_trait::async_trait;
use chrono::Utc;
use parking_lot::RwLock;
use reqwest::Client;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::{AccountSnapshot, Balance};
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{dec, Decimal, VenueId};
use crate::rate_limiter::AccountRequestCoordinator;

fn dec_from_json(v: Option<&serde_json::Value>) -> Option<Decimal> {
    match v? {
        serde_json::Value::String(s) => Decimal::from_str_exact(s).ok(),
        serde_json::Value::Number(n) => Decimal::from_str_exact(&n.to_string()).ok(),
        _ => None,
    }
}

/// Decibel Aptos CLOB Perpetual DEX connector.
#[allow(dead_code)]
pub struct DecibelAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    api_key: RwLock<Option<String>>,
    node_api_key: RwLock<Option<String>>,
    wallet_address: RwLock<Option<String>>,
    private_key: RwLock<Option<String>>,
    rate_limiter: AccountRequestCoordinator,
}

impl DecibelAdapter {
    pub fn new(
        base_url: Option<String>,
        api_key: Option<String>,
        api_secret: Option<String>,
    ) -> Self {
        let base = base_url.unwrap_or_else(|| "https://api.mainnet.aptoslabs.com/decibel".to_string());
        Self {
            venue_id: VenueId::new("decibel"),
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
                Client::builder().timeout(std::time::Duration::from_secs(15))
                    .default_headers(headers)
                    .build()
                    .unwrap_or_default()
            },
            base_url: base,
            api_key: RwLock::new(api_key),
            node_api_key: RwLock::new(api_secret),
            wallet_address: RwLock::new(None),
            private_key: RwLock::new(None),
            rate_limiter: AccountRequestCoordinator::new("decibel", 100, 2000, 500, 10000),
        }
    }

    pub fn update_credentials(
        &self,
        api_key: Option<String>,
        node_api_key: Option<String>,
        wallet_address: Option<String>,
        private_key: Option<String>,
    ) {
        *self.api_key.write() = api_key;
        *self.node_api_key.write() = node_api_key;
        *self.wallet_address.write() = wallet_address;
        *self.private_key.write() = private_key;
    }

    async fn get_json(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        let key = {
            let guard = self.api_key.read();
            match &*guard {
                Some(k) if !k.trim().is_empty() => k.trim().to_string(),
                _ => anyhow::bail!("Decibel requires a Geomi/Aptos client API key"),
            }
        };

        if let Some(cached_str) = self.rate_limiter.acquire_or_cache_path(path, true).await? {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&cached_str) {
                return Ok(val);
            }
        }

        let url = format!("{}{}", self.base_url, path);
        let resp = match self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", key))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                self.rate_limiter.record_failure(false);
                return Err(e.into());
            }
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let is_429 = status == reqwest::StatusCode::TOO_MANY_REQUESTS;
            if status.as_u16() == 401 || status.as_u16() == 404 {
                anyhow::bail!("Decibel API {}: {}", status, path);
            }
            self.rate_limiter.record_failure(is_429);
            anyhow::bail!("Decibel API error: {} {}", status, path);
        }

        let text = resp.text().await?;
        self.rate_limiter.record_success_path(path, text.clone());
        let data = serde_json::from_str::<serde_json::Value>(&text)?;
        Ok(data)
    }
}

#[async_trait]
impl ExchangeAdapter for DecibelAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Decibel"
    }

    async fn ping(&self) -> anyhow::Result<()> {
        let resp = self.client.get("https://fullnode.mainnet.aptoslabs.com/v1").send().await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            anyhow::bail!("Decibel Aptos probe returned status {}", resp.status());
        }
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let mut markets = Vec::new();
        let common_pairs = ["APT", "BTC", "ETH", "SOL", "USDT", "SUI", "DOGE"];
        for base in common_pairs {
            markets.push(MarketMetadata {
                symbol: Symbol::perp(base, "USDC"),
                venue: self.venue_id.clone(),
                venue_symbol: format!("{}-PERP", base),
                price_decimals: if base == "BTC" || base == "ETH" { 2 } else { 4 },
                size_decimals: 3,
                min_order_size: dec!(0.001),
                min_notional: dec!(5.0),
                maker_fee_bps: dec!(2.0),
                taker_fee_bps: dec!(5.0),
                contract_size: dec!(1.0),
                is_active: true,
            });
        }
        Ok(markets)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let path = format!("/v1/orderbook?market={}", symbol);
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
            anyhow::bail!("Decibel orderbook empty or offline for {}", symbol);
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
        let wallet = self.wallet_address.read().clone();
        let Some(w) = wallet.filter(|s| !s.trim().is_empty()) else {
            anyhow::bail!("Decibel account query requires an Aptos wallet address. Please set the Login Wallet Address in Settings > Exchange Accounts.");
        };

        let path = format!("/v1/subaccounts?owner={}", w.trim());
        let val = self.get_json(&path).await?;
        let total_equity = dec_from_json(val.get("totalEquity").or_else(|| val.get("equity"))).unwrap_or_default();
        let free_margin = dec_from_json(val.get("freeMargin").or_else(|| val.get("availableBalance"))).unwrap_or(total_equity);
        let locked = if total_equity > free_margin { total_equity - free_margin } else { dec!(0) };
        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity: total_equity,
            free_margin,
            maintenance_margin: dec!(0),
            margin_usage_pct: dec!(0),
            balances: vec![Balance {
                venue: self.venue_id.clone(),
                currency: "USDC".to_string(),
                total: total_equity,
                free: free_margin,
                locked,
            }],
            positions: Vec::new(),
            timestamp: Utc::now(),
        })
    }

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        anyhow::bail!("Live trading execution not configured for Decibel")
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        anyhow::bail!("Live trading cancellation not configured for Decibel")
    }

    fn supports_live_execution(&self) -> bool {
        false
    }

    fn has_live_credentials(&self) -> bool {
        self.api_key.read().as_ref().map_or(false, |k| !k.trim().is_empty())
            || self.wallet_address.read().as_ref().map_or(false, |w| !w.trim().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_decibel_credentials_flow() {
        let adapter = DecibelAdapter::new(None, None, None);
        assert!(!adapter.has_live_credentials());
        assert!(!adapter.supports_live_execution());
        assert!(adapter.fetch_account().await.is_err());

        adapter.update_credentials(
            Some("geomi_client_key".to_string()),
            Some("node_key".to_string()),
            Some("0x1234567890abcdef".to_string()),
            Some("0xprivkey".to_string()),
        );
        assert!(adapter.has_live_credentials());
        assert!(!adapter.supports_live_execution());

        adapter.update_credentials(None, None, None, None);
        assert!(!adapter.has_live_credentials());
        assert!(!adapter.supports_live_execution());
    }
}
