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
use zeroize::Zeroizing;

/// OKX Perpetual Swaps connector (https://www.okx.com).
#[allow(dead_code)]
pub struct OkxAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    api_key: RwLock<Option<Zeroizing<String>>>,
    api_secret: RwLock<Option<Zeroizing<String>>>,
    passphrase: RwLock<Option<Zeroizing<String>>>,
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
                Client::builder().timeout(std::time::Duration::from_secs(15))
                    .default_headers(headers)
                    .build()
                    .unwrap_or_default()
            },
            base_url: base,
            api_key: RwLock::new(api_key.map(Zeroizing::new)),
            api_secret: RwLock::new(api_secret.map(Zeroizing::new)),
            passphrase: RwLock::new(passphrase.map(Zeroizing::new)),
            rate_limiter: AccountRequestCoordinator::new("okx", 200, 3000, 500, 15000),
        }
    }

    pub fn update_credentials(
        &self,
        api_key: Option<String>,
        api_secret: Option<String>,
        passphrase: Option<String>,
    ) {
        *self.api_key.write() = api_key.map(Zeroizing::new);
        *self.api_secret.write() = api_secret.map(Zeroizing::new);
        *self.passphrase.write() = passphrase.map(Zeroizing::new);
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

    fn credentials(
        &self,
    ) -> anyhow::Result<(Zeroizing<String>, Zeroizing<String>, Zeroizing<String>)> {
        let key = |v: &RwLock<Option<Zeroizing<String>>>, what: &str| {
            v.read()
                .clone()
                .filter(|k| !k.trim().is_empty())
                .ok_or_else(|| anyhow::anyhow!("OKX {what} not configured or empty"))
        };
        Ok((
            key(&self.api_key, "API key")?,
            key(&self.api_secret, "API secret")?,
            key(&self.passphrase, "passphrase")?,
        ))
    }

    fn has_credentials(&self) -> bool {
        self.credentials().is_ok()
    }

    /// OKX signature preimage: `timestamp + METHOD + requestPath + body`,
    /// HMAC-SHA256, base64. Pure for unit testing.
    fn sign_request(secret: &Zeroizing<String>, preimage: &str) -> anyhow::Result<String> {
        use base64::Engine;
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|e| anyhow::anyhow!("Invalid OKX HMAC key: {e}"))?;
        mac.update(preimage.as_bytes());
        Ok(base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
    }

    fn okx_timestamp() -> String {
        Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
    }

    async fn signed_request(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> anyhow::Result<serde_json::Value> {
        let (api_key, api_secret, passphrase) = self.credentials()?;
        let ts = Self::okx_timestamp();
        let body_str = body.unwrap_or("");
        let preimage = format!("{ts}{method}{path}{body_str}");
        let sign = Self::sign_request(&api_secret, &preimage)?;
        let url = format!("{}{}", self.base_url, path);
        let mut req = match method {
            "GET" => self.client.get(&url),
            "POST" => self.client.post(&url),
            _ => anyhow::bail!("OKX unsupported signed method {method}"),
        };
        req = req
            .header("OK-ACCESS-KEY", api_key.to_string())
            .header("OK-ACCESS-SIGN", sign)
            .header("OK-ACCESS-TIMESTAMP", ts)
            .header("OK-ACCESS-PASSPHRASE", passphrase.to_string())
            .header("Content-Type", "application/json");
        if !body_str.is_empty() {
            req = req.body(body_str.to_string());
        }
        let resp = req.send().await.map_err(|e| {
            self.rate_limiter.record_failure(false);
            anyhow::anyhow!("OKX {method} {path} transport failure: {e}")
        })?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            self.rate_limiter
                .record_failure(status == reqwest::StatusCode::TOO_MANY_REQUESTS);
            anyhow::bail!("OKX {method} {path} failed (HTTP {status}): {text}");
        }
        let v: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("Failed to parse OKX {path} response: {e}; body: {text}"))?;
        if v.get("code").and_then(|c| c.as_str()) != Some("0") {
            anyhow::bail!("OKX {method} {path} error response: {text}");
        }
        Ok(v)
    }

    /// Contract face value (ctVal, base units per contract) with a tiny
    /// in-memory cache. Public endpoint — safe before credentials exist.
    async fn contract_face_value(&self, inst_id: &str) -> anyhow::Result<Decimal> {
        let path = format!("/api/v5/public/instruments?instType=SWAP&instId={inst_id}");
        let v = self.get_json(&path).await?;
        let ct_val = v
            .get("data")
            .and_then(|d| d.as_array())
            .and_then(|a| a.first())
            .and_then(|d| dec_from_json(d.get("ctVal")))
            .filter(|c| *c > dec!(0))
            .ok_or_else(|| anyhow::anyhow!("OKX missing ctVal for {inst_id}"))?;
        Ok(ct_val)
    }

    /// Engine base-unit quantity into whole OKX contracts (floor). Pure.
    fn contracts_for_qty(qty: Decimal, ct_val: Decimal) -> Option<Decimal> {
        if qty <= dec!(0) || ct_val <= dec!(0) {
            return None;
        }
        let contracts = (qty / ct_val).trunc();
        (contracts >= dec!(1)).then_some(contracts)
    }

    /// Pure parse of a `/trade/order` response envelope.
    fn parse_order_response(
        v: &serde_json::Value,
        order: &OrderRequest,
    ) -> anyhow::Result<OrderResponse> {
        use arbitrageur_core::types::OrderStatus;
        let data = v
            .get("data")
            .and_then(|d| d.as_array())
            .and_then(|a| a.first())
            .ok_or_else(|| anyhow::anyhow!("OKX order response missing data"))?;
        let code = data.get("sCode").and_then(|c| c.as_str()).unwrap_or("");
        if code != "0" {
            let msg = data.get("sMsg").and_then(|m| m.as_str()).unwrap_or("unknown");
            anyhow::bail!("OKX order rejected (sCode {code}): {msg}");
        }
        let order_id = data
            .get("ordId")
            .and_then(|id| id.as_str())
            .unwrap_or_default()
            .to_string();
        let client_order_id = data
            .get("clOrdId")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&order.client_order_id)
            .to_string();
        // Market fills report asynchronously; the ack carries no fill price.
        Ok(OrderResponse {
            order_id,
            client_order_id,
            venue: VenueId::new("okx"),
            symbol: order.symbol.clone(),
            status: OrderStatus::New,
            original_qty: order.quantity,
            filled_qty: dec!(0),
            remaining_qty: order.quantity,
            avg_fill_price: None,
            fee: dec!(0),
            fee_currency: "USDT".to_string(),
            raw_response: Some(v.to_string()),
            timestamp: Utc::now(),
        })
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

    /// Poll a resting order for its true fill state (up to ~5s). Returns
    /// (status, filled_base_qty, avg_px, fee).
    async fn poll_order_fill(
        &self,
        inst_id: &str,
        ord_id: &str,
        ct_val: Decimal,
    ) -> anyhow::Result<(
        arbitrageur_core::types::OrderStatus,
        Decimal,
        Option<Decimal>,
        Decimal,
    )> {
        use arbitrageur_core::types::OrderStatus;
        for _ in 0..6 {
            let path = format!("/api/v5/trade/order?instId={inst_id}&ordId={ord_id}");
            let v = self.signed_request("GET", &path, None).await?;
            let data = v
                .get("data")
                .and_then(|d| d.as_array())
                .and_then(|a| a.first())
                .ok_or_else(|| anyhow::anyhow!("OKX order query missing data"))?;
            let state = data.get("state").and_then(|s| s.as_str()).unwrap_or("live");
            let acc_contracts = dec_from_json(data.get("accFillSz")).unwrap_or(dec!(0));
            let filled_base = acc_contracts * ct_val;
            let avg_px = dec_from_json(data.get("avgPx")).filter(|p| *p > dec!(0));
            let fee = dec_from_json(data.get("fee")).unwrap_or(dec!(0)).abs();
            let terminal = matches!(
                state,
                "filled" | "partially_filled" | "canceled" | "cancelled" | "failed"
            );
            if terminal || filled_base > dec!(0) {
                let status = match state {
                    "filled" => OrderStatus::Filled,
                    "partially_filled" => OrderStatus::PartiallyFilled,
                    "canceled" | "cancelled" if filled_base > dec!(0) => {
                        OrderStatus::PartiallyFilled
                    }
                    "canceled" | "cancelled" => OrderStatus::Cancelled,
                    "failed" => OrderStatus::Rejected,
                    _ => OrderStatus::New,
                };
                return Ok((status, filled_base, avg_px, fee));
            }
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        }
        anyhow::bail!("OKX order {ord_id} fill state unknown after polling")
    }

    /// Pure parse of an `/account/balance` `data[0]` envelope into an
    /// [`AccountSnapshot`] with no positions (filled in by the caller).
    /// Split out for unit testing (no network).
    fn parse_balance_envelope(
        &self,
        acct: &serde_json::Value,
    ) -> anyhow::Result<AccountSnapshot> {
        let equity = dec_from_json(acct.get("totalEq")).unwrap_or_default();
        let free_margin = dec_from_json(acct.get("availEq"))
            .or_else(|| dec_from_json(acct.get("availBal")))
            .unwrap_or_default();
        let maint_margin = dec_from_json(acct.get("mmr")).unwrap_or_default();
        let initial_margin = dec_from_json(acct.get("imr")).unwrap_or_default();
        let margin_usage_pct = if equity > dec!(0) {
            (initial_margin / equity) * dec!(100)
        } else {
            dec!(0)
        };
        let mut balances = Vec::new();
        if let Some(details) = acct.get("details").and_then(|d| d.as_array()) {
            for b in details {
                let ccy = b.get("ccy").and_then(|c| c.as_str()).unwrap_or_default();
                if ccy.is_empty() {
                    continue;
                }
                let total = dec_from_json(b.get("eq")).unwrap_or_default();
                let free = dec_from_json(b.get("availBal")).unwrap_or_default();
                let frozen = dec_from_json(b.get("frozenBal"))
                    .unwrap_or_else(|| total.saturating_sub(free));
                if total.is_zero() && free.is_zero() && frozen.is_zero() {
                    continue;
                }
                balances.push(arbitrageur_core::position::Balance {
                    venue: self.venue_id.clone(),
                    currency: ccy.to_string(),
                    total,
                    free,
                    locked: frozen,
                });
            }
        }
        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity,
            free_margin,
            maintenance_margin: maint_margin,
            margin_usage_pct,
            balances,
            positions: Vec::new(),
            timestamp: Utc::now(),
        })
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

        book.bids.sort_by_key(|b| std::cmp::Reverse(b.price));
        book.asks.sort_by_key(|a| a.price);
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
        if !self.has_credentials() {
            anyhow::bail!("Cannot fetch live account on OKX: API credentials are not configured or vault is locked");
        }
        // Wallet-level balances + margin (cross/imr/mmr are account-wide).
        let bal = self
            .signed_request("GET", "/api/v5/account/balance", None)
            .await?;
        let acct = bal
            .get("data")
            .and_then(|d| d.as_array())
            .and_then(|a| a.first())
            .ok_or_else(|| anyhow::anyhow!("OKX account balance response missing data"))?;
        let snap = self.parse_balance_envelope(acct)?;

        // Open swap positions (separate endpoint). Fail-closed: a leg whose
        // contract face value cannot be resolved aborts the read rather than
        // reporting a silently understated exposure.
        let pos_v = self
            .signed_request("GET", "/api/v5/account/positions", None)
            .await?;
        let mut positions = Vec::new();
        if let Some(arr) = pos_v.get("data").and_then(|d| d.as_array()) {
            for leg in arr {
                let inst_id = leg.get("instId").and_then(|s| s.as_str()).unwrap_or_default();
                if inst_id.is_empty() {
                    continue;
                }
                let contracts = dec_from_json(leg.get("pos")).unwrap_or_default();
                if contracts.is_zero() {
                    continue;
                }
                let ct_val = self.contract_face_value(inst_id).await.map_err(|e| {
                    anyhow::anyhow!("OKX position {inst_id} face value unresolvable, aborting account read: {e}")
                })?;
                let qty = (contracts.abs() * ct_val).abs();
                if qty <= dec!(0) {
                    continue;
                }
                let entry_px = dec_from_json(leg.get("avgPx")).unwrap_or_default();
                let upnl = dec_from_json(leg.get("upl")).unwrap_or_default();
                let liq_px = dec_from_json(leg.get("liqPx")).filter(|x| *x > dec!(0));
                let lev = leg
                    .get("lever")
                    .and_then(|l| match l {
                        serde_json::Value::Number(n) => {
                            n.as_f64().and_then(Decimal::from_f64_retain)
                        }
                        serde_json::Value::String(s) => Decimal::from_str_exact(s).ok(),
                        _ => None,
                    })
                    .unwrap_or(dec!(1));
                let side = if contracts > dec!(0) {
                    arbitrageur_core::types::Side::Buy
                } else {
                    arbitrageur_core::types::Side::Sell
                };
                let notional = qty * entry_px;
                let im = if lev > dec!(0) { notional / lev } else { notional };
                positions.push(arbitrageur_core::position::Position {
                    venue: self.venue_id.clone(),
                    symbol: inst_id.to_string(),
                    side,
                    quantity: qty,
                    entry_price: entry_px,
                    mark_price: entry_px,
                    unrealized_pnl: upnl,
                    realized_pnl: dec!(0),
                    liquidation_price: liq_px,
                    leverage: lev,
                    initial_margin: im,
                    maintenance_margin: im * dec!(0.5),
                    updated_at: Utc::now(),
                });
            }
        }

        Ok(AccountSnapshot {
            positions,
            ..snap
        })
    }

    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        if !self.has_credentials() {
            anyhow::bail!("Cannot place live order on OKX: API credentials are not configured or vault is locked");
        }
        if order.quantity <= dec!(0) {
            anyhow::bail!("OKX order quantity must be positive");
        }
        let inst_id = Self::to_venue_symbol(&order.symbol)?;
        let ct_val = self.contract_face_value(&inst_id).await?;
        // Engine quantities are base units; OKX `sz` is whole contracts.
        let contracts = Self::contracts_for_qty(order.quantity, ct_val).ok_or_else(|| {
            anyhow::anyhow!(
                "OKX order quantity {} {} is below one contract ({inst_id} ctVal {ct_val})",
                order.quantity, order.symbol
            )
        })?;
        let side_str = match order.side {
            arbitrageur_core::types::Side::Buy => "buy",
            arbitrageur_core::types::Side::Sell => "sell",
        };
        let (ord_type, px) = match order.order_type {
            arbitrageur_core::types::OrderType::Market
            | arbitrageur_core::types::OrderType::ImmediateOrCancel => ("market", None),
            arbitrageur_core::types::OrderType::Limit
            | arbitrageur_core::types::OrderType::PostOnly => {
                let p = order.price.filter(|p| *p > dec!(0)).ok_or_else(|| {
                    anyhow::anyhow!("OKX limit order requires a positive price")
                })?;
                ("limit", Some(p.to_string()))
            }
        };
        let mut body = serde_json::json!({
            "instId": inst_id,
            "tdMode": "isolated",
            "side": side_str,
            "ordType": ord_type,
            "sz": contracts.to_string(),
        });
        if let Some(px) = px {
            body["px"] = serde_json::Value::String(px);
        }
        if order.reduce_only {
            body["reduceOnly"] = serde_json::Value::Bool(true);
        }
        if !order.client_order_id.is_empty() {
            body["clOrdId"] = serde_json::Value::String(order.client_order_id.clone());
        }
        let body_str = serde_json::to_string(&body)?;
        let v = self
            .signed_request("POST", "/api/v5/trade/order", Some(&body_str))
            .await?;
        let mut resp = Self::parse_order_response(&v, &order)?;
        // Market fills land asynchronously: poll the order a few times and
        // report truthful fills. Never fabricate: unknown states stay New.
        if order.order_type == arbitrageur_core::types::OrderType::Market
            || order.order_type == arbitrageur_core::types::OrderType::ImmediateOrCancel
        {
            if let Ok(fill) = self.poll_order_fill(&inst_id, &resp.order_id, ct_val).await {
                resp.status = fill.0;
                resp.filled_qty = fill.1;
                resp.remaining_qty = order.quantity.saturating_sub(fill.1);
                resp.avg_fill_price = fill.2;
                resp.fee = fill.3;
            }
        }
        Ok(resp)
    }


    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
        if !self.has_credentials() {
            anyhow::bail!("Cannot cancel live order on OKX: API credentials are not configured or vault is locked");
        }
        let inst_id = Self::to_venue_symbol(&req.symbol)?;
        let mut body = serde_json::json!({ "instId": inst_id });
        if let Some(id) = req.order_id {
            body["ordId"] = serde_json::Value::String(id);
        }
        if let Some(cid) = req.client_order_id {
            body["clOrdId"] = serde_json::Value::String(cid);
        }
        let body_str = serde_json::to_string(&body)?;
        let v = self
            .signed_request("POST", "/api/v5/trade/cancel-order", Some(&body_str))
            .await?;
        let code = v
            .get("data")
            .and_then(|d| d.as_array())
            .and_then(|a| a.first())
            .and_then(|d| d.get("sCode"))
            .and_then(|c| c.as_str())
            .unwrap_or("");
        if code != "0" {
            anyhow::bail!("OKX cancel rejected (sCode {code}): {}", v);
        }
        Ok(())
    }

    async fn set_leverage(&self, symbol: &str, leverage: u32) -> anyhow::Result<()> {
        if !self.has_credentials() {
            anyhow::bail!("Cannot set leverage on OKX: API credentials are not configured or vault is locked");
        }
        if !(1..=125).contains(&leverage) {
            anyhow::bail!("OKX leverage {leverage} out of range 1..=125");
        }
        let inst_id = Self::to_venue_symbol(symbol)?;
        let body = serde_json::json!({
            "instId": inst_id,
            "lever": leverage.to_string(),
            "mgnMode": "isolated",
        });
        let body_str = serde_json::to_string(&body)?;
        let v = self
            .signed_request("POST", "/api/v5/account/set-leverage", Some(&body_str))
            .await?;
        let got = v
            .get("data")
            .and_then(|d| d.as_array())
            .and_then(|a| a.first())
            .and_then(|d| d.get("lever"))
            .and_then(|l| l.as_str())
            .unwrap_or("");
        if got != leverage.to_string() {
            anyhow::bail!("OKX leverage ack mismatch: wanted {leverage}, venue reports {got}");
        }
        Ok(())
    }

    fn live_order_submission_implemented(&self) -> bool {
        true
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
    fn sign_request_matches_verified_vector() {
        // Cross-checked with `openssl dgst -sha256 -hmac secret -binary | openssl base64`
        // and Python hmac over the same preimage.
        let secret = Zeroizing::new("secret".to_string());
        let pre = "2020-12-08T09:08:57.715ZPOST/api/v5/trade/order{\"instId\":\"BTC-USDT-SWAP\",\"tdMode\":\"isolated\",\"side\":\"buy\",\"ordType\":\"market\",\"sz\":\"1\"}";
        let sig = OkxAdapter::sign_request(&secret, pre).unwrap();
        assert_eq!(sig, "1mGn7nuHsjf/9bT57H1rzqrdrjs0S9gIS4QTbfF3yM0=");
    }

    #[test]
    fn contracts_for_qty_floors_to_whole_contracts() {
        assert_eq!(
            OkxAdapter::contracts_for_qty(dec!(0.05), dec!(0.01)),
            Some(dec!(5))
        );
        assert_eq!(
            OkxAdapter::contracts_for_qty(dec!(0.015), dec!(0.01)),
            Some(dec!(1))
        );
        assert_eq!(OkxAdapter::contracts_for_qty(dec!(0.005), dec!(0.01)), None);
        assert_eq!(OkxAdapter::contracts_for_qty(dec!(0), dec!(0.01)), None);
    }

    #[test]
    fn parse_order_response_accepts_and_rejects() {
        use arbitrageur_core::order::OrderRequest;
        use arbitrageur_core::types::{OrderStatus, Side};
        let order = OrderRequest::new_market(
            VenueId::new("okx"),
            "BTC-USDC",
            Side::Buy,
            dec!(0.05),
        );
        let v: serde_json::Value = serde_json::json!({
            "code": "0",
            "data": [{ "ordId": "999", "clOrdId": "", "sCode": "0", "sMsg": "" }]
        });
        let resp = OkxAdapter::parse_order_response(&v, &order).unwrap();
        assert_eq!(resp.order_id, "999");
        assert_eq!(resp.status, OrderStatus::New);
        let rej: serde_json::Value = serde_json::json!({
            "code": "0",
            "data": [{ "ordId": "", "sCode": "51000", "sMsg": "boom" }]
        });
        let err = OkxAdapter::parse_order_response(&rej, &order).expect_err("must reject");
        assert!(err.to_string().contains("51000"), "unexpected: {err}");
    }

    #[test]
    fn parse_balance_envelope_maps_equity_and_balances() {
        let adapter = OkxAdapter::new(None, None, None, None);
        let acct: serde_json::Value = serde_json::json!({
            "totalEq": "10120.5", "availEq": "9000.0",
            "imr": "1120.0", "mmr": "56.0",
            "details": [
                {"ccy": "USDT", "eq": "10000.0", "availBal": "8900.0", "frozenBal": "1100.0"},
                {"ccy": "BTC", "eq": "0", "availBal": "0", "frozenBal": "0"}
            ]
        });
        let snap = adapter.parse_balance_envelope(&acct).unwrap();
        assert_eq!(snap.equity, dec!(10120.5));
        assert_eq!(snap.free_margin, dec!(9000.0));
        assert_eq!(snap.maintenance_margin, dec!(56.0));
        assert!((snap.margin_usage_pct - dec!(11.07)).abs() < dec!(0.01));
        assert_eq!(snap.balances.len(), 1);
        assert_eq!(snap.balances[0].currency, "USDT");
        assert_eq!(snap.balances[0].locked, dec!(1100.0));
        assert!(snap.positions.is_empty());
    }

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
