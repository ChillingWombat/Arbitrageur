use crate::rate_limiter::AccountRequestCoordinator;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::AccountSnapshot;
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{dec, Decimal, VenueId};
use async_trait::async_trait;
use chrono::Utc;
use parking_lot::RwLock;
use reqwest::Client;
use zeroize::Zeroizing;

/// Coinbase Exchange connector (https://api.exchange.coinbase.com).
#[allow(dead_code)]
pub struct CoinbaseAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    api_key: RwLock<Option<Zeroizing<String>>>,
    api_secret: RwLock<Option<Zeroizing<String>>>,
    passphrase: RwLock<Option<Zeroizing<String>>>,
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
                    .timeout(std::time::Duration::from_secs(15))
                    .default_headers(headers)
                    .build()
                    .unwrap_or_default()
            },
            base_url: base,
            api_key: RwLock::new(api_key.map(Zeroizing::new)),
            api_secret: RwLock::new(api_secret.map(Zeroizing::new)),
            passphrase: RwLock::new(passphrase.map(Zeroizing::new)),
            rate_limiter: AccountRequestCoordinator::new("coinbase", 200, 3000, 500, 15000),
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
            let snippet: String = resp
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(512)
                .collect();
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

    fn credentials(
        &self,
    ) -> anyhow::Result<(Zeroizing<String>, Zeroizing<String>, Zeroizing<String>)> {
        let api_key = self
            .api_key
            .read()
            .clone()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Coinbase API key not configured or empty"))?;
        let api_secret = self
            .api_secret
            .read()
            .clone()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Coinbase API secret not configured or empty"))?;
        let passphrase = self
            .passphrase
            .read()
            .clone()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Coinbase API passphrase not configured or empty"))?;
        Ok((api_key, api_secret, passphrase))
    }

    fn has_credentials(&self) -> bool {
        self.credentials().is_ok()
    }

    /// Coinbase Exchange (legacy) signature:
    /// `base64(HMAC-SHA256(timestamp + method + request_path + body,
    /// base64_decode(secret)))`.
    fn sign_exchange(
        secret: &Zeroizing<String>,
        timestamp: &str,
        method: &str,
        request_path: &str,
        body: &str,
    ) -> anyhow::Result<String> {
        use base64::Engine;
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(secret.trim().as_bytes())
            .map_err(|e| anyhow::anyhow!("Invalid Coinbase base64 API secret: {e}"))?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&decoded)
            .map_err(|e| anyhow::anyhow!("Invalid Coinbase HMAC key: {e}"))?;
        mac.update(timestamp.as_bytes());
        mac.update(method.as_bytes());
        mac.update(request_path.as_bytes());
        mac.update(body.as_bytes());
        Ok(base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
    }

    /// Authenticated Coinbase Exchange request. `path` includes any query
    /// string (which is part of the signed payload); `body` is the exact JSON
    /// string sent (empty for GET/DELETE).
    async fn authed(
        &self,
        method: &str,
        path: &str,
        body: Option<String>,
    ) -> anyhow::Result<serde_json::Value> {
        let (api_key, api_secret, passphrase) = self.credentials()?;
        let ts = format!("{:.3}", Utc::now().timestamp_millis() as f64 / 1000.0);
        let body_str = body.clone().unwrap_or_default();
        let sign = Self::sign_exchange(&api_secret, &ts, method, path, &body_str)?;
        let url = format!("{}{}", self.base_url, path);
        let mut req = match method {
            "GET" => self.client.get(&url),
            "POST" => self.client.post(&url),
            "DELETE" => self.client.delete(&url),
            _ => anyhow::bail!("Coinbase unsupported signed method {method}"),
        };
        req = req
            .header("CB-ACCESS-KEY", api_key.to_string())
            .header("CB-ACCESS-SIGN", sign)
            .header("CB-ACCESS-TIMESTAMP", ts)
            .header("CB-ACCESS-PASSPHRASE", passphrase.to_string())
            .header("Content-Type", "application/json");
        if let Some(b) = body {
            req = req.body(b);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Coinbase {method} {path} transport failure: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("Coinbase {method} {path} failed (HTTP {status}): {text}");
        }
        if text.trim().is_empty() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_str::<serde_json::Value>(&text).map_err(|e| {
            anyhow::anyhow!("Failed to parse Coinbase {path} response: {e}; body: {text}")
        })
    }

    /// Pure parse of a `GET /accounts` array into a snapshot. `marks_usd`
    /// maps currencies to USD marks (caller seeds stables at 1 and majors via
    /// cached product tickers); unknown codes count 0 toward equity
    /// (conservative lower bound) but stay listed raw in `balances`.
    fn parse_accounts_response(
        &self,
        v: &serde_json::Value,
        marks_usd: &std::collections::HashMap<String, Decimal>,
    ) -> anyhow::Result<AccountSnapshot> {
        let arr = v
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Coinbase accounts response not an array"))?;
        let mut balances = Vec::new();
        let mut equity = dec!(0);
        for a in arr {
            let ccy = a
                .get("currency")
                .and_then(|c| c.as_str())
                .unwrap_or_default();
            if ccy.is_empty() {
                continue;
            }
            let total = dec_from_json(a.get("balance")).unwrap_or_default();
            let free = dec_from_json(a.get("available")).unwrap_or(total);
            let hold = dec_from_json(a.get("hold")).unwrap_or_else(|| total.saturating_sub(free));
            if total.is_zero() && free.is_zero() && hold.is_zero() {
                continue;
            }
            let code = ccy.to_uppercase();
            let mark = if ["USD", "USDT", "USDC", "DAI"].contains(&code.as_str()) {
                dec!(1)
            } else {
                marks_usd.get(&code).copied().unwrap_or(dec!(0))
            };
            equity += total * mark;
            balances.push(arbitrageur_core::position::Balance {
                venue: self.venue_id.clone(),
                currency: code,
                total,
                free,
                locked: hold,
            });
        }
        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity,
            free_margin: equity,
            maintenance_margin: dec!(0),
            margin_usage_pct: dec!(0),
            balances,
            positions: Vec::new(),
            timestamp: Utc::now(),
        })
    }

    fn to_venue_symbol(symbol: &str) -> anyhow::Result<String> {
        let sym = symbol.trim().to_uppercase();
        if sym.contains('-') {
            let parts: Vec<&str> = sym.split('-').collect();
            let [base, quote] = parts.as_slice() else {
                anyhow::bail!("Coinbase symbol '{symbol}' is not BASE-QUOTE");
            };
            if !matches!(*quote, "USD" | "USDC" | "USDT") {
                anyhow::bail!(
                    "Coinbase markets only support USD/USDC/USDT quotes; '{symbol}' is not supported"
                );
            }
            return Ok(format!("{base}-{quote}"));
        }
        Ok(format!("{sym}-USD"))
    }
    /// Shared fail-closed parse for the `/orders` envelope (placement ack and
    /// `GET /orders/{id}` share the shape). `expected_*` constrain identity
    /// when the original request is known; reconciliation polls pass `None`
    /// and take `size`/`client_oid` from the venue response as authoritative.
    fn parse_order_envelope(
        &self,
        v: &serde_json::Value,
        product_id: &str,
        expected_qty: Option<Decimal>,
        expected_client_id: Option<&str>,
        expected_side: Option<&str>,
        canonical_symbol: &str,
    ) -> anyhow::Result<OrderResponse> {
        use arbitrageur_core::types::OrderStatus;
        let order_id = v
            .get("id")
            .and_then(|o| o.as_str())
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Coinbase order returned no id: {v}"))?
            .to_string();
        let returned_product = v
            .get("product_id")
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow::anyhow!("Coinbase order response missing product_id: {v}"))?;
        if returned_product != product_id {
            anyhow::bail!(
                "Coinbase order response product_id '{returned_product}' does not match expected '{product_id}': {v}"
            );
        }
        let returned_client = v
            .get("client_oid")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Coinbase order response missing client_oid: {v}"))?;
        if let Some(expected) = expected_client_id {
            if returned_client != expected {
                anyhow::bail!(
                    "Coinbase order response client_oid '{returned_client}' does not match request '{expected}': {v}"
                );
            }
        }
        if let (Some(returned_side), Some(expected)) =
            (v.get("side").and_then(|s| s.as_str()), expected_side)
        {
            if returned_side != expected {
                anyhow::bail!(
                    "Coinbase order response side '{returned_side}' does not match expected '{expected}': {v}"
                );
            }
        }
        let filled_qty = dec_from_json(v.get("filled_size"))
            .filter(|q| *q >= dec!(0))
            .ok_or_else(|| {
                anyhow::anyhow!("Coinbase order response missing valid filled_size: {v}")
            })?;
        let orig_qty = match expected_qty {
            Some(q) => {
                if filled_qty > q {
                    anyhow::bail!(
                        "Coinbase order response filled_size {filled_qty} exceeds requested {q}: {v}"
                    );
                }
                q
            }
            None => dec_from_json(v.get("size"))
                .filter(|q| *q > dec!(0))
                .ok_or_else(|| {
                    anyhow::anyhow!("Coinbase order response missing valid size: {v}")
                })?,
        };
        if filled_qty > orig_qty {
            anyhow::bail!(
                "Coinbase order response filled_size {filled_qty} exceeds size {orig_qty}: {v}"
            );
        }
        let remaining_qty = orig_qty.saturating_sub(filled_qty);
        let raw_status = v
            .get("status")
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow::anyhow!("Coinbase order response missing status: {v}"))?;
        let status = match raw_status {
            "done" | "settled" if filled_qty == orig_qty => OrderStatus::Filled,
            "done" | "settled" if filled_qty > dec!(0) => OrderStatus::PartiallyFilled,
            "done" | "settled" => OrderStatus::Cancelled,
            "rejected" if filled_qty == dec!(0) => OrderStatus::Rejected,
            "rejected" => {
                anyhow::bail!("Coinbase rejected order has nonzero filled_size {filled_qty}: {v}")
            }
            "open" | "active" | "pending" | "received" if filled_qty == dec!(0) => OrderStatus::New,
            "open" | "active" | "pending" | "received" => OrderStatus::PartiallyFilled,
            "cancelled" | "canceled" if filled_qty == dec!(0) => OrderStatus::Cancelled,
            "cancelled" | "canceled" if filled_qty < orig_qty => OrderStatus::PartiallyFilled,
            "cancelled" | "canceled" => OrderStatus::Filled,
            other => anyhow::bail!("Coinbase order response has unknown status '{other}': {v}"),
        };
        let avg_fill_price = if filled_qty > dec!(0) {
            let executed_value = dec_from_json(v.get("executed_value"))
                .filter(|x| *x > dec!(0))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Coinbase order response has fill quantity but no valid executed_value: {v}"
                    )
                })?;
            Some(executed_value / filled_qty)
        } else {
            None
        };
        let fee = if filled_qty > dec!(0) {
            dec_from_json(v.get("fill_fees"))
                .filter(|f| *f >= dec!(0))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Coinbase order response has fill quantity but no valid fill_fees: {v}"
                    )
                })?
        } else {
            dec!(0)
        };
        Ok(OrderResponse {
            order_id,
            client_order_id: returned_client.to_string(),
            venue: VenueId::new("coinbase"),
            symbol: canonical_symbol.to_string(),
            status,
            original_qty: orig_qty,
            filled_qty,
            remaining_qty,
            avg_fill_price,
            fee,
            fee_currency: "USD".to_string(),
            raw_response: Some(v.to_string()),
            timestamp: Utc::now(),
        })
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
            let trading_disabled = item
                .get("trading_disabled")
                .and_then(|b| b.as_bool())
                .unwrap_or(false);
            if status != "online" || trading_disabled {
                continue;
            }

            let id = match item.get("id").and_then(|s| s.as_str()) {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => continue,
            };

            let base_currency = item
                .get("base_currency")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            let quote_currency = item
                .get("quote_currency")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            if base_currency.is_empty() || quote_currency.is_empty() {
                continue;
            }

            let min_order_size = dec_from_json(item.get("base_min_size")).unwrap_or(dec!(0.0001));
            let quote_inc = item
                .get("quote_increment")
                .and_then(|s| s.as_str())
                .unwrap_or("0.01");
            let base_inc = item
                .get("base_increment")
                .and_then(|s| s.as_str())
                .unwrap_or("0.0001");

            let price_decimals = quote_inc
                .split('.')
                .nth(1)
                .map(|d| d.len() as u32)
                .unwrap_or(2);
            let size_decimals = base_inc
                .split('.')
                .nth(1)
                .map(|d| d.len() as u32)
                .unwrap_or(4);

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

        book.bids.sort_by_key(|b| std::cmp::Reverse(b.price));
        book.asks.sort_by_key(|a| a.price);
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
        anyhow::bail!(
            "Coinbase is a spot exchange with no perpetual funding rate for {}",
            symbol
        )
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        if !self.has_credentials() {
            anyhow::bail!("Cannot fetch live account on Coinbase: API credentials are not configured or vault is locked");
        }
        let v = self.authed("GET", "/accounts", None).await?;
        // USD marks for nonzero crypto balances via cached product tickers
        // (best-effort: unresolvable codes stay 0 in the equity roll-up).
        let mut marks = std::collections::HashMap::new();
        if let Some(arr) = v.as_array() {
            for a in arr {
                let ccy = a.get("currency").and_then(|c| c.as_str()).unwrap_or("");
                let total = dec_from_json(a.get("balance")).unwrap_or_default();
                if ccy.is_empty()
                    || total.is_zero()
                    || ["USD", "USDT", "USDC", "DAI"].contains(&ccy.to_uppercase().as_str())
                    || marks.contains_key(&ccy.to_uppercase())
                {
                    continue;
                }
                let path = format!("/products/{}-USD/ticker", ccy.to_uppercase());
                if let Ok(t) = self.get_json(&path).await {
                    if let Some(px) = dec_from_json(t.get("price")).filter(|x| *x > dec!(0)) {
                        marks.insert(ccy.to_uppercase(), px);
                    }
                }
            }
        }
        self.parse_accounts_response(&v, &marks)
    }

    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        if !self.has_credentials() {
            anyhow::bail!("Cannot place live order on Coinbase: API credentials are not configured or vault is locked");
        }
        if order.quantity <= dec!(0) {
            anyhow::bail!("Coinbase order quantity must be positive");
        }
        let product_id = Self::to_venue_symbol(&order.symbol)?;
        let side_str = match order.side {
            arbitrageur_core::types::Side::Buy => "buy",
            arbitrageur_core::types::Side::Sell => "sell",
        };
        let is_market = order.order_type == arbitrageur_core::types::OrderType::Market;
        let mut req = serde_json::json!({
            "side": side_str,
            "product_id": product_id,
        });
        if !order.client_order_id.is_empty() {
            req["client_oid"] = serde_json::Value::String(order.client_order_id.clone());
        }
        if is_market {
            req["type"] = serde_json::Value::String("market".to_string());
            match order.side {
                arbitrageur_core::types::Side::Sell => {
                    req["size"] = serde_json::Value::String(order.quantity.normalize().to_string());
                }
                arbitrageur_core::types::Side::Buy => {
                    // Market buys are quote-funded: size the spend from the
                    // live ask so the full base quantity fills.
                    let t = self
                        .get_json(&format!("/products/{product_id}/ticker"))
                        .await?;
                    let ask = dec_from_json(t.get("ask"))
                        .or_else(|| dec_from_json(t.get("price")))
                        .filter(|x| *x > dec!(0))
                        .ok_or_else(|| {
                            anyhow::anyhow!("Coinbase market buy needs a live ask for {product_id}")
                        })?;
                    let funds = (order.quantity * ask).normalize();
                    req["funds"] = serde_json::Value::String(funds.to_string());
                }
            }
        } else {
            let px = order.price.filter(|x| *x > dec!(0)).ok_or_else(|| {
                anyhow::anyhow!("Coinbase limit order requires a positive limit price")
            })?;
            req["type"] = serde_json::Value::String("limit".to_string());
            req["price"] = serde_json::Value::String(px.normalize().to_string());
            req["size"] = serde_json::Value::String(order.quantity.normalize().to_string());
            if order.post_only
                || order.time_in_force == arbitrageur_core::types::TimeInForce::PostOnly
            {
                req["post_only"] = serde_json::Value::Bool(true);
            }
            match order.time_in_force {
                arbitrageur_core::types::TimeInForce::ImmediateOrCancel => {
                    req["time_in_force"] = serde_json::Value::String("IOC".to_string())
                }
                arbitrageur_core::types::TimeInForce::FillOrKill => {
                    req["time_in_force"] = serde_json::Value::String("FOK".to_string())
                }
                _ => {}
            }
        }
        if order.reduce_only {
            // Spot has no shorts: reduce-only is a no-op guard — refusing
            // rather than pretending it constrains anything.
            anyhow::bail!("Coinbase spot does not support reduce-only orders");
        }
        let body = serde_json::to_string(&req)?;
        let v = self.authed("POST", "/orders", Some(body)).await?;
        self.parse_order_envelope(
            &v,
            &product_id,
            Some(order.quantity),
            Some(order.client_order_id.as_str()),
            Some(side_str),
            &order.symbol,
        )
    }

    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
        if !self.has_credentials() {
            anyhow::bail!("Cannot cancel live order on Coinbase: API credentials are not configured or vault is locked");
        }
        // Order-id cancels hit /orders/{id}; client-oid cancels hit
        // /orders/client:{oid} (both DELETE, empty body).
        let path = if let Some(id) = req.order_id {
            format!("/orders/{}", id.trim())
        } else if let Some(cid) = req.client_order_id {
            format!("/orders/client:{}", cid.trim())
        } else {
            anyhow::bail!("Coinbase cancel requires an order_id or client_order_id");
        };
        self.authed("DELETE", &path, None).await?;
        Ok(())
    }

    /// Poll authoritative order state via `GET /orders/{order_id}`. Shares
    /// the placement-ack envelope; `size` is read from the venue response
    /// since reconciliation doesn't hold the original request.
    async fn fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        _client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        if !self.has_credentials() {
            anyhow::bail!("Cannot fetch order on Coinbase: API credentials are not configured or vault is locked");
        }
        if order_id.trim().is_empty() {
            anyhow::bail!("Coinbase fetch_order requires a non-empty order_id");
        }
        let product_id = Self::to_venue_symbol(symbol)?;
        let v = self
            .authed("GET", &format!("/orders/{}", order_id.trim()), None)
            .await?;
        let resp = self.parse_order_envelope(&v, &product_id, None, None, None, symbol)?;
        if resp.order_id != order_id.trim() {
            anyhow::bail!(
                "Coinbase fetch_order returned id {} != queried {order_id}",
                resp.order_id
            );
        }
        Ok(resp)
    }

    fn order_status_polling_implemented(&self) -> bool {
        true
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
    fn sign_exchange_matches_independent_vectors() {
        // Cross-checked with Python hmac/hashlib/base64.
        let secret =
            Zeroizing::new("c2VjcmV0LWtleS1mb3ItdGVzdGluZy0xMjM0NTY3ODkwMTIzNDU2Nzg=".to_string());
        let get_sig =
            CoinbaseAdapter::sign_exchange(&secret, "1700000000.000", "GET", "/accounts", "")
                .unwrap();
        assert_eq!(get_sig, "vF6wthHYf+8Vv2p0sA95y4IpxGmVU/QBbIM4DuvKsBs=");
        let body = r#"{"client_oid":"abc-123","side":"buy","product_id":"BTC-USD","type":"limit","price":"60000.00","size":"0.001"}"#;
        let post_sig =
            CoinbaseAdapter::sign_exchange(&secret, "1700000000.000", "POST", "/orders", body)
                .unwrap();
        assert_eq!(post_sig, "bVeJS8x7MOeCJlcRdZFTx8XrL7ERjEcuq+N88Hb7blk=");
    }

    #[test]
    fn parse_accounts_response_maps_balances_and_equity() {
        use std::collections::HashMap;
        let adapter = CoinbaseAdapter::new(None, None, None, None);
        let v: serde_json::Value = serde_json::json!([
            {"currency": "USD", "balance": "5000.0", "available": "4500.0", "hold": "500.0"},
            {"currency": "BTC", "balance": "0.5", "available": "0.5", "hold": "0"},
            {"currency": "DUST", "balance": "0", "available": "0", "hold": "0"}
        ]);
        let mut marks = HashMap::new();
        marks.insert("BTC".to_string(), dec!(60000));
        let snap = adapter.parse_accounts_response(&v, &marks).unwrap();
        assert_eq!(snap.equity, dec!(35000));
        assert_eq!(snap.balances.len(), 2);
        assert_eq!(snap.balances[0].locked, dec!(500));
        assert!(snap.positions.is_empty());
    }

    #[test]
    fn to_venue_symbol_normalizes_and_rejects_cross() {
        assert_eq!(
            CoinbaseAdapter::to_venue_symbol("BTC-USD").unwrap(),
            "BTC-USD"
        );
        assert_eq!(
            CoinbaseAdapter::to_venue_symbol("BTC-USDC").unwrap(),
            "BTC-USDC"
        );
        assert_eq!(
            CoinbaseAdapter::to_venue_symbol("BTC-USDT").unwrap(),
            "BTC-USDT"
        );
        assert_eq!(CoinbaseAdapter::to_venue_symbol("ETH").unwrap(), "ETH-USD");
        assert!(CoinbaseAdapter::to_venue_symbol("BTC-SOL").is_err());
        assert!(CoinbaseAdapter::to_venue_symbol("BTC-ETH").is_err());
    }
}
