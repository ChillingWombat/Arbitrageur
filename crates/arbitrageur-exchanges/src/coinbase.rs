use async_trait::async_trait;
use chrono::Utc;
use parking_lot::RwLock;
use reqwest::Client;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::AccountSnapshot;
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{dec, Decimal, VenueId};
use crate::rate_limiter::AccountRequestCoordinator;

/// Coinbase Exchange connector (https://api.exchange.coinbase.com).
#[allow(dead_code)]
pub struct CoinbaseAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    api_key: RwLock<Option<String>>,
    api_secret: RwLock<Option<String>>,
    passphrase: RwLock<Option<String>>,
    rate_limiter: AccountRequestCoordinator,
}

fn dec_from_json(v: Option<&serde_json::Value>) -> Option<Decimal> {
    match v? {
        serde_json::Value::String(s) => Decimal::from_str_exact(s).ok(),
        serde_json::Value::Number(n) => Decimal::from_str_exact(&n.to_string()).ok(),
        _ => None,
    }
}

fn parse_pair(level: &serde_json::Value) -> Option<(Decimal, Decimal)> {
    let arr = level.as_array()?;
    if arr.len() < 2 {
        return None;
    }
    let p = dec_from_json(arr.first())?;
    let q = dec_from_json(arr.get(1))?;
    if p > dec!(0) && q > dec!(0) {
        Some((p, q))
    } else {
        None
    }
}

impl CoinbaseAdapter {
    pub fn new(
        base_url: Option<String>,
        api_key: Option<String>,
        api_secret: Option<String>,
        passphrase: Option<String>,
    ) -> Self {
        let base = base_url.unwrap_or_else(|| "https://api.exchange.coinbase.com".to_string());
        let venue_id = VenueId::new("coinbase");
        Self {
            venue_id,
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
            api_key: RwLock::new(api_key),
            api_secret: RwLock::new(api_secret),
            passphrase: RwLock::new(passphrase),
            rate_limiter: AccountRequestCoordinator::new("coinbase", 200, 3000, 500, 15000),
        }
    }

    pub fn update_credentials(
        &self,
        api_key: Option<String>,
        api_secret: Option<String>,
        passphrase: Option<String>,
    ) {
        *self.api_key.write() = api_key;
        *self.api_secret.write() = api_secret;
        *self.passphrase.write() = passphrase;
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
            let headers = resp.headers().clone();
            let status = resp.status().as_u16();
            let snippet: String = resp.text().await.unwrap_or_default().chars().take(512).collect();
            if crate::rate_limiter::is_waf_challenge(status, &headers, &snippet) {
                self.rate_limiter.record_challenge();
                anyhow::bail!("Coinbase edge challenge on {} — cooling down", path);
            }
            if status == 404 {
                anyhow::bail!("Coinbase symbol not found: {}", path);
            }
            self.rate_limiter.record_failure(is_429);
            anyhow::bail!("Coinbase API error: {} {}", status, path);
        }

        let text = resp.text().await?;
        self.rate_limiter.record_success_path(path, text.clone());
        let data = serde_json::from_str::<serde_json::Value>(&text)?;
        Ok(data)
    }

    fn to_venue_symbol(symbol: &str) -> anyhow::Result<String> {
        let sym = symbol.trim().to_uppercase();
        if sym.contains('-') {
            let parts: Vec<&str> = sym.split('-').collect();
            if parts.len() >= 2 && parts[1] != "USD" && parts[1] != "USDC" && parts[1] != "USDT" {
                anyhow::bail!("Coinbase USD markets only support USD/USDC quote; '{symbol}' is not supported");
            }
        }
        let base = if sym.contains('-') {
            sym.split('-').next().unwrap_or(&sym)
        } else {
            &sym
        };
        Ok(format!("{}-USD", base))
    }
}

#[async_trait]
impl ExchangeAdapter for CoinbaseAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Coinbase"
    }

    async fn ping(&self) -> anyhow::Result<()> {
        let val = self.get_json("/time").await?;
        if val.get("iso").is_some() {
            Ok(())
        } else {
            anyhow::bail!("Coinbase ping failed to get time");
        }
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let val = self.get_json("/products").await?;
        let items = val
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Coinbase products not an array"))?;

        let mut out = Vec::new();
        for item in items {
            let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("");
            let trading_disabled = item.get("trading_disabled").and_then(|b| b.as_bool()).unwrap_or(false);
            if status != "online" || trading_disabled {
                continue;
            }

            let id = match item.get("id").and_then(|s| s.as_str()) {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => continue,
            };

            let base_currency = item.get("base_currency").and_then(|s| s.as_str()).unwrap_or("");
            let quote_currency = item.get("quote_currency").and_then(|s| s.as_str()).unwrap_or("");
            if base_currency.is_empty() || quote_currency.is_empty() {
                continue;
            }

            let min_order_size = dec_from_json(item.get("base_min_size")).unwrap_or(dec!(0.0001));
            let quote_inc = item.get("quote_increment").and_then(|s| s.as_str()).unwrap_or("0.01");
            let base_inc = item.get("base_increment").and_then(|s| s.as_str()).unwrap_or("0.0001");

            let price_decimals = quote_inc.split('.').nth(1).map(|d| d.len() as u32).unwrap_or(2);
            let size_decimals = base_inc.split('.').nth(1).map(|d| d.len() as u32).unwrap_or(4);

            out.push(MarketMetadata {
                symbol: Symbol::spot(base_currency, quote_currency),
                venue: self.venue_id.clone(),
                venue_symbol: id,
                price_decimals,
                size_decimals,
                min_order_size,
                min_notional: dec!(1.0),
                maker_fee_bps: dec!(40.0),
                taker_fee_bps: dec!(60.0),
                contract_size: dec!(1.0),
                is_active: true,
            });
        }
        Ok(out)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let product_id = Self::to_venue_symbol(symbol)?;
        let path = format!("/products/{}/book?level=2", product_id);
        let val = self.get_json(&path).await?;

        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());
        if let Some(bids) = val.get("bids").and_then(|b| b.as_array()) {
            for b in bids {
                if let Some((p, q)) = parse_pair(b) {
                    book.bids.push(BookLevel::new(p, q));
                }
            }
        }
        if let Some(asks) = val.get("asks").and_then(|a| a.as_array()) {
            for a in asks {
                if let Some((p, q)) = parse_pair(a) {
                    book.asks.push(BookLevel::new(p, q));
                }
            }
        }

        book.bids.sort_by(|a, b| b.price.cmp(&a.price));
        book.asks.sort_by(|a, b| a.price.cmp(&b.price));
        book.timestamp = Utc::now();
        Ok(book)
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let product_id = Self::to_venue_symbol(symbol)?;
        let path = format!("/products/{}/ticker", product_id);
        let val = self.get_json(&path).await?;

        let bid = dec_from_json(val.get("bid")).unwrap_or(dec!(0));
        let ask = dec_from_json(val.get("ask")).unwrap_or(dec!(0));
        let last = dec_from_json(val.get("price")).unwrap_or_else(|| {
            if bid > dec!(0) && ask > dec!(0) {
                (bid + ask) / dec!(2)
            } else {
                dec!(0)
            }
        });

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
        anyhow::bail!("Coinbase is a spot exchange with no perpetual funding rate for {}", symbol)
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        anyhow::bail!("Live account query not configured for Coinbase; refusing to fabricate")
    }

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        anyhow::bail!("Live trading execution not configured for Coinbase")
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        anyhow::bail!("Live trading cancellation not configured for Coinbase")
    }

    fn supports_live_execution(&self) -> bool {
        true
    }

    fn has_live_credentials(&self) -> bool {
        self.api_key
            .read()
            .as_ref()
            .map_or(false, |k| !k.trim().is_empty())
            && self
                .api_secret
                .read()
                .as_ref()
                .map_or(false, |s| !s.trim().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_venue_symbol_normalizes_and_rejects_cross() {
        assert_eq!(
            CoinbaseAdapter::to_venue_symbol("BTC-USD").unwrap(),
            "BTC-USD"
        );
        assert_eq!(
            CoinbaseAdapter::to_venue_symbol("ETH").unwrap(),
            "ETH-USD"
        );
        assert!(CoinbaseAdapter::to_venue_symbol("BTC-SOL").is_err());
        assert!(CoinbaseAdapter::to_venue_symbol("BTC-ETH").is_err());
    }
}
