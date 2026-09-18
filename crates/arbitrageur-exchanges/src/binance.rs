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

/// Binance USDT-M Futures connector (https://fapi.binance.com).
///
/// Provides public market data (exchange info, orderbooks, tickers, bulk funding rates,
/// 24h volumes) and fail-closed live trading execution.
#[allow(dead_code)]
pub struct BinanceAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    api_key: RwLock<Option<String>>,
    api_secret: RwLock<Option<String>>,
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

impl BinanceAdapter {
    pub fn new(
        base_url: Option<String>,
        api_key: Option<String>,
        api_secret: Option<String>,
    ) -> Self {
        let base = base_url.unwrap_or_else(|| "https://fapi.binance.com".to_string());
        let venue_id = VenueId::new("binance");
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
            base_url: base,
            api_key: RwLock::new(api_key),
            api_secret: RwLock::new(api_secret),
            rate_limiter: AccountRequestCoordinator::new("binance", 200, 3000, 500, 15000),
        }
    }

    pub fn update_credentials(&self, api_key: Option<String>, api_secret: Option<String>) {
        *self.api_key.write() = api_key;
        *self.api_secret.write() = api_secret;
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
                anyhow::bail!("Binance edge challenge on {} — cooling down", path);
            }
            if status == 400 && snippet.contains("-1121") {
                anyhow::bail!("Binance symbol not found: {}", path);
            }
            self.rate_limiter.record_failure(is_429);
            anyhow::bail!("Binance API error: {} {}", status, path);
        }

        let text = resp.text().await?;
        self.rate_limiter.record_success_path(path, text.clone());
        let data = serde_json::from_str::<serde_json::Value>(&text)?;
        Ok(data)
    }

    fn to_venue_symbol(symbol: &str) -> anyhow::Result<(String, bool)> {
        let sym = symbol.trim().to_uppercase();
        let (base, quote) = if sym.contains('-') {
            let parts: Vec<&str> = sym.split('-').collect();
            let b = parts[0];
            let q = parts.get(1).copied().unwrap_or("");
            (b, q)
        } else if sym.ends_with("USDT") {
            (&sym[..sym.len().saturating_sub(4)], "USDT")
        } else if sym.ends_with("USDC") {
            (&sym[..sym.len().saturating_sub(4)], "USDC")
        } else {
            (sym.as_str(), "USDT")
        };
        if !quote.is_empty() && quote != "USDT" && quote != "USDC" && quote != "USD" {
            anyhow::bail!("Binance futures markets are quoted in USDT/USDC; '{symbol}' is not supported");
        }
        let res = match base {
            "PEPE" => ("1000PEPEUSDT".to_string(), true),
            "BONK" => ("1000BONKUSDT".to_string(), true),
            "SHIB" => ("1000SHIBUSDT".to_string(), true),
            "FLOKI" => ("1000FLOKIUSDT".to_string(), true),
            "LUNC" => ("1000LUNCUSDT".to_string(), true),
            "XEC" => ("1000XECUSDT".to_string(), true),
            "RATS" => ("1000RATSUSDT".to_string(), true),
            "SATS" => ("1000SATSUSDT".to_string(), true),
            "CAT" => ("1000CATUSDT".to_string(), true),
            "CHEEMS" => ("1000CHEEMSUSDT".to_string(), true),
            b => (format!("{}USDT", b), false),
        };
        Ok(res)
    }
}

#[async_trait]
impl ExchangeAdapter for BinanceAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Binance"
    }

    async fn ping(&self) -> anyhow::Result<()> {
        let url = format!("{}/fapi/v1/ping", self.base_url);
        let resp = self.client.get(&url).send().await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            anyhow::bail!("Binance ping failed with status {}", resp.status());
        }
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let val = self.get_json("/fapi/v1/exchangeInfo").await?;
        let symbols = val
            .get("symbols")
            .and_then(|s| s.as_array())
            .ok_or_else(|| anyhow::anyhow!("Binance exchangeInfo missing 'symbols'"))?;

        let mut out = Vec::new();
        for item in symbols {
            let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("");
            let contract_type = item.get("contractType").and_then(|s| s.as_str()).unwrap_or("");
            if status != "TRADING" || contract_type != "PERPETUAL" {
                continue;
            }

            let venue_symbol = match item.get("symbol").and_then(|s| s.as_str()) {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => continue,
            };
            let base_asset = item.get("baseAsset").and_then(|s| s.as_str()).unwrap_or("");
            let quote_asset = item.get("quoteAsset").and_then(|s| s.as_str()).unwrap_or("");
            if base_asset.is_empty() || quote_asset.is_empty() {
                continue;
            }

            let price_decimals = item.get("pricePrecision").and_then(|n| n.as_u64()).unwrap_or(2) as u32;
            let size_decimals = item.get("quantityPrecision").and_then(|n| n.as_u64()).unwrap_or(3) as u32;

            let mut min_order_size = dec!(0.001);
            let mut min_notional = dec!(5.0);

            if let Some(filters) = item.get("filters").and_then(|f| f.as_array()) {
                for f in filters {
                    match f.get("filterType").and_then(|t| t.as_str()) {
                        Some("LOT_SIZE") => {
                            if let Some(min_qty) = dec_from_json(f.get("minQty")) {
                                min_order_size = min_qty;
                            }
                        }
                        Some("MIN_NOTIONAL") | Some("NOTIONAL") => {
                            if let Some(notional) = dec_from_json(f.get("notional")) {
                                min_notional = notional;
                            }
                        }
                        _ => {}
                    }
                }
            }

            out.push(MarketMetadata {
                symbol: Symbol::perp(base_asset, quote_asset),
                venue: self.venue_id.clone(),
                venue_symbol,
                price_decimals,
                size_decimals,
                min_order_size,
                min_notional,
                maker_fee_bps: dec!(2.0),
                taker_fee_bps: dec!(5.0),
                contract_size: dec!(1.0),
                is_active: true,
            });
        }
        Ok(out)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let (venue_sym, is_1000x) = Self::to_venue_symbol(symbol)?;
        let path = format!("/fapi/v1/depth?symbol={}&limit={}", venue_sym, ORDERBOOK_LIMIT);
        let val = self.get_json(&path).await?;

        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());
        let scale_p = if is_1000x { dec!(1000) } else { dec!(1) };
        let scale_q = if is_1000x { dec!(1000) } else { dec!(1) };

        if let Some(bids) = val.get("bids").and_then(|b| b.as_array()) {
            for b in bids {
                if let Some((p, q)) = parse_pair(b) {
                    book.bids.push(BookLevel::new(p / scale_p, q * scale_q));
                }
            }
        }
        if let Some(asks) = val.get("asks").and_then(|a| a.as_array()) {
            for a in asks {
                if let Some((p, q)) = parse_pair(a) {
                    book.asks.push(BookLevel::new(p / scale_p, q * scale_q));
                }
            }
        }

        book.bids.sort_by(|a, b| b.price.cmp(&a.price));
        book.asks.sort_by(|a, b| a.price.cmp(&b.price));
        book.timestamp = Utc::now();
        Ok(book)
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let (venue_sym, is_1000x) = Self::to_venue_symbol(symbol)?;
        let path = format!("/fapi/v1/ticker/bookTicker?symbol={}", venue_sym);
        let val = self.get_json(&path).await?;

        let scale_p = if is_1000x { dec!(1000) } else { dec!(1) };
        let bid = dec_from_json(val.get("bidPrice")).unwrap_or(dec!(0)) / scale_p;
        let ask = dec_from_json(val.get("askPrice")).unwrap_or(dec!(0)) / scale_p;
        let last = if bid > dec!(0) && ask > dec!(0) {
            (bid + ask) / dec!(2)
        } else {
            dec!(0)
        };

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
        let (venue_sym, _is_1000x) = Self::to_venue_symbol(symbol)?;
        let path = format!("/fapi/v1/premiumIndex?symbol={}", venue_sym);
        let val = self.get_json(&path).await?;

        let rate = dec_from_json(val.get("lastFundingRate"))
            .ok_or_else(|| anyhow::anyhow!("Binance missing lastFundingRate for {}", symbol))?;
        let next_time_ms = val.get("nextFundingTime").and_then(|n| n.as_i64()).unwrap_or(0);
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

    async fn fetch_funding_rates(&self) -> anyhow::Result<Vec<FundingRate>> {
        let val = self.get_json("/fapi/v1/premiumIndex").await?;
        let arr = val
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Binance bulk premiumIndex not an array"))?;

        let mut out = Vec::new();
        for item in arr {
            let symbol = match item.get("symbol").and_then(|s| s.as_str()) {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => continue,
            };
            if let Some(rate) = dec_from_json(item.get("lastFundingRate")) {
                let next_time_ms = item.get("nextFundingTime").and_then(|n| n.as_i64()).unwrap_or(0);
                let next_funding_time = DateTime::from_timestamp_millis(next_time_ms).unwrap_or_else(Utc::now);
                out.push(FundingRate {
                    venue: self.venue_id.clone(),
                    symbol,
                    rate,
                    predicted_rate: None,
                    interval_hours: 8,
                    next_funding_time,
                    timestamp: Utc::now(),
                });
            }
        }
        Ok(out)
    }

    async fn fetch_volumes_24h(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        let val = self.get_json("/fapi/v1/ticker/24hr").await?;
        let arr = val
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Binance 24hr ticker not an array"))?;

        let mut out = HashMap::new();
        for item in arr {
            if let (Some(sym), Some(vol)) = (
                item.get("symbol").and_then(|s| s.as_str()),
                dec_from_json(item.get("quoteVolume")),
            ) {
                out.insert(sym.to_string(), vol);
            }
        }
        Ok(out)
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        anyhow::bail!("Live account query not configured for Binance; refusing to fabricate")
    }

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        anyhow::bail!("Live trading execution not configured for Binance")
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        anyhow::bail!("Live trading cancellation not configured for Binance")
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
            BinanceAdapter::to_venue_symbol("BTC-USDT").unwrap(),
            ("BTCUSDT".to_string(), false)
        );
        assert_eq!(
            BinanceAdapter::to_venue_symbol("PEPE-USDT").unwrap(),
            ("1000PEPEUSDT".to_string(), true)
        );
        assert!(BinanceAdapter::to_venue_symbol("BTC-SOL").is_err());
        assert!(BinanceAdapter::to_venue_symbol("BTC-ETH").is_err());
    }
}
