use async_trait::async_trait;
use chrono::Utc;
use parking_lot::RwLock;
use reqwest::Client;
use std::collections::HashMap;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::AccountSnapshot;
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{dec, Decimal, VenueId};
use crate::rate_limiter::AccountRequestCoordinator;
use zeroize::Zeroizing;

/// Kraken Futures and Spot connector (https://futures.kraken.com/derivatives/api/v3).
#[allow(dead_code)]
pub struct KrakenAdapter {
    venue_id: VenueId,
    client: Client,
    futures_base_url: String,
    spot_base_url: String,
    api_key: RwLock<Option<Zeroizing<String>>>,
    api_secret: RwLock<Option<Zeroizing<String>>>,
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

impl KrakenAdapter {
    pub fn new(
        futures_url: Option<String>,
        spot_url: Option<String>,
        api_key: Option<String>,
        api_secret: Option<String>,
    ) -> Self {
        let f_url = futures_url.unwrap_or_else(|| "https://futures.kraken.com/derivatives/api/v3".to_string());
        let s_url = spot_url.unwrap_or_else(|| "https://api.kraken.com/0/public".to_string());
        let venue_id = VenueId::new("kraken");
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
                Client::builder().timeout(std::time::Duration::from_secs(15))
                    .default_headers(headers)
                    .build()
                    .unwrap_or_default()
            },
            futures_base_url: f_url,
            spot_base_url: s_url,
            api_key: RwLock::new(api_key.map(Zeroizing::new)),
            api_secret: RwLock::new(api_secret.map(Zeroizing::new)),
            rate_limiter: AccountRequestCoordinator::new("kraken", 200, 3000, 500, 15000),
        }
    }

    pub fn update_credentials(&self, api_key: Option<String>, api_secret: Option<String>) {
        *self.api_key.write() = api_key.map(Zeroizing::new);
        *self.api_secret.write() = api_secret.map(Zeroizing::new);
    }

    async fn get_futures_json(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        if let Some(cached_str) = self.rate_limiter.acquire_or_cache_path(path, true).await? {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&cached_str) {
                return Ok(val);
            }
        }

        let url = format!("{}{}", self.futures_base_url, path);
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
                anyhow::bail!("Kraken edge challenge on {} — cooling down", path);
            }
            if status == 404 || status == 400 {
                anyhow::bail!("Kraken symbol not found: {}", path);
            }
            self.rate_limiter.record_failure(is_429);
            anyhow::bail!("Kraken API error: {} {}", status, path);
        }

        let text = resp.text().await?;
        self.rate_limiter.record_success_path(path, text.clone());
        let data = serde_json::from_str::<serde_json::Value>(&text)?;
        Ok(data)
    }

    fn to_futures_symbol(symbol: &str) -> anyhow::Result<String> {
        let sym = symbol.trim().to_uppercase();
        if sym.starts_with("PF_") || sym.starts_with("PI_") {
            Ok(sym)
        } else {
            if sym.contains('-') {
                let parts: Vec<&str> = sym.split('-').collect();
                if parts.len() >= 2 && parts[1] != "USD" && parts[1] != "USDC" && parts[1] != "USDT" {
                    anyhow::bail!("Kraken futures markets are quoted in USD/USDC; '{symbol}' is not supported");
                }
            }
            let base = sym.split('-').next().unwrap_or(&sym);
            let coin = if base == "BTC" { "XBT" } else { base };
            Ok(format!("PF_{}USD", coin))
        }
    }
}

#[async_trait]
impl ExchangeAdapter for KrakenAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Kraken"
    }

    async fn ping(&self) -> anyhow::Result<()> {
        let val = self.get_futures_json("/instruments").await?;
        if val.get("result").and_then(|r| r.as_str()) == Some("success") {
            Ok(())
        } else {
            anyhow::bail!("Kraken ping failed");
        }
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let val = self.get_futures_json("/instruments").await?;
        let instruments = val
            .get("instruments")
            .and_then(|i| i.as_array())
            .ok_or_else(|| anyhow::anyhow!("Kraken instruments missing array"))?;

        let mut out = Vec::new();
        for item in instruments {
            let tradeable = item.get("tradeable").and_then(|t| t.as_bool()).unwrap_or(false);
            let is_expired = item.get("isExpired").and_then(|t| t.as_bool()).unwrap_or(false);
            if !tradeable || is_expired {
                continue;
            }

            let symbol_str = match item.get("symbol").and_then(|s| s.as_str()) {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => continue,
            };

            let tag = item.get("tag").and_then(|t| t.as_str()).unwrap_or("");
            let inst_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if tag != "perpetual" && inst_type != "futures_inverse" {
                continue;
            }

            let base = item.get("base").and_then(|b| b.as_str()).unwrap_or("");
            let quote = item.get("quote").and_then(|q| q.as_str()).unwrap_or("USD");
            let canonical_base = if base == "XBT" { "BTC" } else { base };
            if canonical_base.is_empty() {
                continue;
            }

            let tick_size = item.get("tickSize").and_then(|n| n.as_f64()).unwrap_or(0.5);
            let contract_size = dec_from_json(item.get("contractSize")).unwrap_or(dec!(1.0));

            out.push(MarketMetadata {
                symbol: Symbol::perp(canonical_base, quote),
                venue: self.venue_id.clone(),
                venue_symbol: symbol_str,
                price_decimals: if tick_size < 0.01 { 4 } else { 2 },
                size_decimals: 3,
                min_order_size: dec!(0.001),
                min_notional: dec!(5.0),
                maker_fee_bps: dec!(2.0),
                taker_fee_bps: dec!(5.0),
                contract_size,
                is_active: true,
            });
        }
        Ok(out)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let venue_sym = Self::to_futures_symbol(symbol)?;
        let path = format!("/orderbook?symbol={}", venue_sym);
        let val = self.get_futures_json(&path).await?;

        let ob = val.get("orderBook").ok_or_else(|| anyhow::anyhow!("Kraken missing orderBook"))?;
        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());

        if let Some(bids) = ob.get("bids").and_then(|b| b.as_array()) {
            for b in bids {
                if let Some((p, q)) = parse_pair(b) {
                    book.bids.push(BookLevel::new(p, q));
                }
            }
        }
        if let Some(asks) = ob.get("asks").and_then(|a| a.as_array()) {
            for a in asks {
                if let Some((p, q)) = parse_pair(a) {
                    book.asks.push(BookLevel::new(p, q));
                }
            }
        }

        // Kraken Futures returns bids sorted ascending; sort descending for canonical representation
        book.bids.sort_by_key(|b| std::cmp::Reverse(b.price));
        book.asks.sort_by_key(|a| a.price);
        book.timestamp = Utc::now();
        Ok(book)
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let venue_sym = Self::to_futures_symbol(symbol)?;
        let val = self.get_futures_json("/tickers").await?;
        let tickers = val.get("tickers").and_then(|t| t.as_array()).ok_or_else(|| anyhow::anyhow!("Kraken missing tickers"))?;

        let item = tickers.iter().find(|t| {
            t.get("symbol").and_then(|s| s.as_str()) == Some(&venue_sym)
        }).ok_or_else(|| anyhow::anyhow!("Kraken ticker not found for {}", symbol))?;

        let bid = dec_from_json(item.get("bid")).unwrap_or(dec!(0));
        let ask = dec_from_json(item.get("ask")).unwrap_or(dec!(0));
        let last = dec_from_json(item.get("last")).unwrap_or_else(|| {
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
        let venue_sym = Self::to_futures_symbol(symbol)?;
        let val = self.get_futures_json("/tickers").await?;
        let tickers = val.get("tickers").and_then(|t| t.as_array()).ok_or_else(|| anyhow::anyhow!("Kraken missing tickers"))?;

        let item = tickers.iter().find(|t| {
            t.get("symbol").and_then(|s| s.as_str()) == Some(&venue_sym)
        }).ok_or_else(|| anyhow::anyhow!("Kraken funding rate not found for {}", symbol))?;

        let rate = dec_from_json(item.get("fundingRate")).unwrap_or(dec!(0));
        let pred = dec_from_json(item.get("fundingRatePrediction"));

        Ok(FundingRate {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            rate,
            predicted_rate: pred,
            interval_hours: 8,
            next_funding_time: Utc::now(),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rates(&self) -> anyhow::Result<Vec<FundingRate>> {
        let val = self.get_futures_json("/tickers").await?;
        let tickers = val.get("tickers").and_then(|t| t.as_array()).ok_or_else(|| anyhow::anyhow!("Kraken missing tickers"))?;

        let mut out = Vec::new();
        for item in tickers {
            if let (Some(sym), Some(rate)) = (
                item.get("symbol").and_then(|s| s.as_str()),
                dec_from_json(item.get("fundingRate")),
            ) {
                let pred = dec_from_json(item.get("fundingRatePrediction"));
                out.push(FundingRate {
                    venue: self.venue_id.clone(),
                    symbol: sym.to_string(),
                    rate,
                    predicted_rate: pred,
                    interval_hours: 8,
                    next_funding_time: Utc::now(),
                    timestamp: Utc::now(),
                });
            }
        }
        Ok(out)
    }

    async fn fetch_volumes_24h(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        let val = self.get_futures_json("/tickers").await?;
        let tickers = val.get("tickers").and_then(|t| t.as_array()).ok_or_else(|| anyhow::anyhow!("Kraken missing tickers"))?;

        let mut out = HashMap::new();
        for item in tickers {
            if let (Some(sym), Some(vol)) = (
                item.get("symbol").and_then(|s| s.as_str()),
                dec_from_json(item.get("volumeQuote")).or_else(|| dec_from_json(item.get("vol24h"))),
            ) {
                out.insert(sym.to_string(), vol);
            }
        }
        Ok(out)
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        anyhow::bail!("Live account query not configured for Kraken; refusing to fabricate")
    }

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        anyhow::bail!("Live trading execution not configured for Kraken")
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        anyhow::bail!("Live trading cancellation not configured for Kraken")
    }

    fn supports_live_execution(&self) -> bool {
        true
    }

    fn has_live_credentials(&self) -> bool {
        self.api_key
            .read()
            .as_ref()
            .is_some_and(|k| !k.trim().is_empty())
            && self
                .api_secret
                .read()
                .as_ref()
                .is_some_and(|s| !s.trim().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_futures_symbol_normalizes_and_rejects_cross() {
        assert_eq!(
            KrakenAdapter::to_futures_symbol("BTC-USD").unwrap(),
            "PF_XBTUSD"
        );
        assert_eq!(
            KrakenAdapter::to_futures_symbol("ETH-USD").unwrap(),
            "PF_ETHUSD"
        );
        assert!(KrakenAdapter::to_futures_symbol("BTC-SOL").is_err());
        assert!(KrakenAdapter::to_futures_symbol("BTC-ETH").is_err());
    }
}
