use async_trait::async_trait;
use chrono::{DateTime, Utc};
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

/// OKX Perpetual Swaps connector (https://www.okx.com).
#[allow(dead_code)]
pub struct OkxAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    api_key: RwLock<Option<String>>,
    api_secret: RwLock<Option<String>>,
    passphrase: RwLock<Option<String>>,
    rate_limiter: AccountRequestCoordinator,
}

const ORDERBOOK_LIMIT: u32 = 100;

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

impl OkxAdapter {
    pub fn new(
        base_url: Option<String>,
        api_key: Option<String>,
        api_secret: Option<String>,
        passphrase: Option<String>,
    ) -> Self {
        let base = base_url.unwrap_or_else(|| "https://www.okx.com".to_string());
        let venue_id = VenueId::new("okx");
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
            rate_limiter: AccountRequestCoordinator::new("okx", 200, 3000, 500, 15000),
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
                anyhow::bail!("OKX edge challenge on {} — cooling down", path);
            }
            self.rate_limiter.record_failure(is_429);
            anyhow::bail!("OKX API error: {} {}", status, path);
        }

        let text = resp.text().await?;
        let data = serde_json::from_str::<serde_json::Value>(&text)?;
        if let Some(code) = data.get("code").and_then(|c| c.as_str()) {
            if code != "0" {
                if code == "51001" {
                    anyhow::bail!("OKX instrument not found: {}", path);
                }
                self.rate_limiter.record_failure(false);
                anyhow::bail!("OKX API returned error code {}: {}", code, path);
            }
        }
        self.rate_limiter.record_success_path(path, text.clone());
        Ok(data)
    }

    fn to_venue_symbol(symbol: &str) -> anyhow::Result<String> {
        let sym = symbol.trim().to_uppercase();
        if sym.ends_with("-SWAP") {
            Ok(sym)
        } else {
            let (base, quote) = if sym.contains('-') {
                let parts: Vec<&str> = sym.split('-').collect();
                (parts[0], parts.get(1).copied().unwrap_or(""))
            } else if sym.ends_with("USDT") {
                (&sym[..sym.len().saturating_sub(4)], "USDT")
            } else if sym.ends_with("USDC") {
                (&sym[..sym.len().saturating_sub(4)], "USDC")
            } else {
                (sym.as_str(), "USDT")
            };
            if !quote.is_empty() && quote != "USDT" && quote != "USDC" && quote != "USD" {
                anyhow::bail!("OKX swap markets are quoted in USDT/USDC; '{symbol}' is not supported");
            }
            Ok(format!("{}-USDT-SWAP", base))
        }
    }
}

#[async_trait]
impl ExchangeAdapter for OkxAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "OKX"
    }

    async fn ping(&self) -> anyhow::Result<()> {
        let val = self.get_json("/api/v5/public/time").await?;
        if val.get("code").and_then(|c| c.as_str()) == Some("0") {
            Ok(())
        } else {
            anyhow::bail!("OKX ping returned non-zero code");
        }
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let val = self.get_json("/api/v5/public/instruments?instType=SWAP").await?;
        let items = val
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| anyhow::anyhow!("OKX instruments missing 'data' array"))?;

        let mut out = Vec::new();
        for item in items {
            let state = item.get("state").and_then(|s| s.as_str()).unwrap_or("");
            let ct_type = item.get("ctType").and_then(|s| s.as_str()).unwrap_or("");
            if state != "live" || ct_type != "linear" {
                continue;
            }

            let inst_id = match item.get("instId").and_then(|s| s.as_str()) {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => continue,
            };

            let uly = item.get("uly").and_then(|s| s.as_str()).unwrap_or("");
            let mut parts = uly.split('-');
            let base_asset = parts.next().unwrap_or("");
            let quote_asset = parts.next().unwrap_or("USDT");
            if base_asset.is_empty() {
                continue;
            }

            let tick_sz = item.get("tickSz").and_then(|s| s.as_str()).unwrap_or("0.01");
            let lot_sz = item.get("lotSz").and_then(|s| s.as_str()).unwrap_or("0.01");
            let min_sz = dec_from_json(item.get("minSz")).unwrap_or(dec!(0.01));
            let ct_val = dec_from_json(item.get("ctVal")).unwrap_or(dec!(1.0));

            let price_decimals = tick_sz.split('.').nth(1).map(|d| d.len() as u32).unwrap_or(2);
            let size_decimals = lot_sz.split('.').nth(1).map(|d| d.len() as u32).unwrap_or(2);

            out.push(MarketMetadata {
                symbol: Symbol::perp(base_asset, quote_asset),
                venue: self.venue_id.clone(),
                venue_symbol: inst_id,
                price_decimals,
                size_decimals,
                min_order_size: min_sz,
                min_notional: dec!(5.0),
                maker_fee_bps: dec!(2.0),
                taker_fee_bps: dec!(5.0),
                contract_size: ct_val,
                is_active: true,
            });
        }
        Ok(out)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let inst_id = Self::to_venue_symbol(symbol)?;
        let path = format!("/api/v5/market/books?instId={}&sz={}", inst_id, ORDERBOOK_LIMIT);
        let val = self.get_json(&path).await?;

        let data = val
            .get("data")
            .and_then(|d| d.as_array())
            .and_then(|a| a.first())
            .ok_or_else(|| anyhow::anyhow!("OKX orderbook missing data for {}", symbol))?;

        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());
        if let Some(bids) = data.get("bids").and_then(|b| b.as_array()) {
            for b in bids {
                if let Some((p, q)) = parse_pair(b) {
                    book.bids.push(BookLevel::new(p, q));
                }
            }
        }
        if let Some(asks) = data.get("asks").and_then(|a| a.as_array()) {
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
        let inst_id = Self::to_venue_symbol(symbol)?;
        let path = format!("/api/v5/market/ticker?instId={}", inst_id);
        let val = self.get_json(&path).await?;

        let data = val
            .get("data")
            .and_then(|d| d.as_array())
            .and_then(|a| a.first())
            .ok_or_else(|| anyhow::anyhow!("OKX ticker missing data for {}", symbol))?;

        let bid = dec_from_json(data.get("bidPx")).unwrap_or(dec!(0));
        let ask = dec_from_json(data.get("askPx")).unwrap_or(dec!(0));
        let last = dec_from_json(data.get("last")).unwrap_or_else(|| {
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
        let inst_id = Self::to_venue_symbol(symbol)?;
        let path = format!("/api/v5/public/funding-rate?instId={}", inst_id);
        let val = self.get_json(&path).await?;

        let data = val
            .get("data")
            .and_then(|d| d.as_array())
            .and_then(|a| a.first())
            .ok_or_else(|| anyhow::anyhow!("OKX funding-rate missing data for {}", symbol))?;

        let rate = dec_from_json(data.get("fundingRate"))
            .ok_or_else(|| anyhow::anyhow!("OKX missing fundingRate for {}", symbol))?;
        let next_time_ms = data.get("fundingTime").and_then(|n| {
            if let Some(s) = n.as_str() {
                s.parse::<i64>().ok()
            } else {
                n.as_i64()
            }
        }).unwrap_or(0);
        let next_funding_time = DateTime::from_timestamp_millis(next_time_ms).unwrap_or_else(Utc::now);

        Ok(FundingRate {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            rate,
            predicted_rate: None,
            interval_hours: 8,
            next_funding_time,
            timestamp: Utc::now(),
        })
    }

    async fn fetch_volumes_24h(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        let val = self.get_json("/api/v5/market/tickers?instType=SWAP").await?;
        let items = val
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| anyhow::anyhow!("OKX tickers missing data"))?;

        let mut out = HashMap::new();
        for item in items {
            if let (Some(sym), Some(vol)) = (
                item.get("instId").and_then(|s| s.as_str()),
                dec_from_json(item.get("volCcy24h")),
            ) {
                out.insert(sym.to_string(), vol);
            }
        }
        Ok(out)
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        anyhow::bail!("Live account query not configured for OKX; refusing to fabricate")
    }

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        anyhow::bail!("Live trading execution not configured for OKX")
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        anyhow::bail!("Live trading cancellation not configured for OKX")
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
            OkxAdapter::to_venue_symbol("BTC-USDT").unwrap(),
            "BTC-USDT-SWAP"
        );
        assert_eq!(
            OkxAdapter::to_venue_symbol("BTC-USDT-SWAP").unwrap(),
            "BTC-USDT-SWAP"
        );
        assert!(OkxAdapter::to_venue_symbol("BTC-SOL").is_err());
        assert!(OkxAdapter::to_venue_symbol("BTC-ETH").is_err());
    }
}
