use crate::rate_limiter::AccountRequestCoordinator;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::{AccountSnapshot, Position};
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{dec, Decimal, OrderType, Side, TimeInForce, VenueId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use reqwest::Client;
use std::collections::{BTreeMap, HashMap};
use zeroize::Zeroizing;

/// Binance USDT-M Futures connector (https://fapi.binance.com).
///
/// Provides public market data (exchange info, orderbooks, tickers, bulk funding rates,
/// 24h volumes) and fail-closed live trading execution.
#[allow(dead_code)]
pub struct BinanceAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    api_key: RwLock<Option<Zeroizing<String>>>,
    api_secret: RwLock<Option<Zeroizing<String>>>,
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

/// Binance enforces PRICE_FILTER.tickSize and LOT_SIZE.stepSize — raw
/// Decimal precision is rejected outright. Quantity floors to the step
/// (never oversizes); the limit price is a protective bound, so buys
/// floor and sells ceil (never crosses the intended cap/floor).
fn binance_wire_qty(qty: Decimal, step: Decimal) -> Decimal {
    if step > dec!(0) {
        (qty / step).floor() * step
    } else {
        qty
    }
}

fn binance_wire_price(price: Decimal, side: Side, tick: Decimal) -> Decimal {
    if tick <= dec!(0) {
        return price;
    }
    match side {
        Side::Buy => (price / tick).floor() * tick,
        Side::Sell => (price / tick).ceil() * tick,
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
                Client::builder()
                    .timeout(std::time::Duration::from_secs(15))
                    .default_headers(headers)
                    .build()
                    .unwrap_or_default()
            },
            base_url: base,
            api_key: RwLock::new(api_key.map(Zeroizing::new)),
            api_secret: RwLock::new(api_secret.map(Zeroizing::new)),
            rate_limiter: AccountRequestCoordinator::new("binance", 200, 3000, 500, 15000),
        }
    }

    pub fn update_credentials(&self, api_key: Option<String>, api_secret: Option<String>) {
        *self.api_key.write() = api_key.map(Zeroizing::new);
        *self.api_secret.write() = api_secret.map(Zeroizing::new);
    }

    /// Venue order filters for one symbol — (PRICE_FILTER.tickSize,
    /// LOT_SIZE.stepSize, LOT_SIZE.minQty). exchangeInfo is cached by the
    /// request coordinator, so this is cheap on the order path.
    async fn symbol_filters(
        &self,
        venue_symbol: &str,
    ) -> anyhow::Result<(Decimal, Decimal, Decimal)> {
        let val = self.get_json("/fapi/v1/exchangeInfo").await?;
        let symbols = val
            .get("symbols")
            .and_then(|s| s.as_array())
            .ok_or_else(|| anyhow::anyhow!("Binance exchangeInfo missing 'symbols'"))?;
        let item = symbols
            .iter()
            .find(|i| i.get("symbol").and_then(|s| s.as_str()) == Some(venue_symbol))
            .ok_or_else(|| anyhow::anyhow!("Binance exchangeInfo has no symbol {venue_symbol}"))?;
        let mut tick = dec!(0);
        let mut step = dec!(0);
        let mut min_qty = dec!(0);
        if let Some(filters) = item.get("filters").and_then(|f| f.as_array()) {
            for f in filters {
                match f.get("filterType").and_then(|t| t.as_str()) {
                    Some("PRICE_FILTER") => tick = dec_from_json(f.get("tickSize")).unwrap_or(tick),
                    Some("LOT_SIZE") => {
                        step = dec_from_json(f.get("stepSize")).unwrap_or(step);
                        min_qty = dec_from_json(f.get("minQty")).unwrap_or(min_qty);
                    }
                    _ => {}
                }
            }
        }
        Ok((tick, step, min_qty))
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

    /// Map a canonical/engine symbol to the exact Binance venue spelling.
    /// `BASE-QUOTE` and `BASEQUOTE` (explicit quote) are preserved verbatim —
    /// `BTC-USDC` routes to `BTCUSDC`, `1000PEPE-USDT` to `1000PEPEUSDT`.
    /// Bare bases without an explicit quote are rejected rather than guessed
    /// onto USDT, and no 1000x aliasing or unit scaling is applied: live
    /// `exchangeInfo` lists `1000PEPE` as the actual base asset, so venue
    /// quantities are already denominated in that unit.
    fn to_venue_symbol(symbol: &str) -> anyhow::Result<String> {
        let sym = symbol.trim().to_uppercase().replace('/', "-");
        let (base, quote) = if sym.contains('-') {
            let parts: Vec<&str> = sym.split('-').filter(|p| !p.is_empty()).collect();
            let [base, quote] = parts.as_slice() else {
                anyhow::bail!("Binance symbol '{symbol}' is not BASE-QUOTE");
            };
            (*base, *quote)
        } else if let Some(base) = sym.strip_suffix("USDT") {
            (base, "USDT")
        } else if let Some(base) = sym.strip_suffix("USDC") {
            (base, "USDC")
        } else {
            anyhow::bail!(
                "Binance symbol '{symbol}' has no explicit USDT/USDC quote; refusing to guess"
            );
        };
        if base.is_empty()
            || !base.chars().all(|c| c.is_ascii_alphanumeric())
            || !matches!(quote, "USDT" | "USDC")
        {
            anyhow::bail!(
                "Binance futures markets are quoted in USDT/USDC; '{symbol}' is not supported"
            );
        }
        Ok(format!("{base}{quote}"))
    }

    /// Convert an account row such as `BTCUSDT` or `1000PEPEUSDT` back to the
    /// engine's `BASE-QUOTE` spelling. Bare bases and malformed rows are
    /// rejected rather than guessed onto a USDT market.
    fn canonical_account_symbol(symbol: &str) -> anyhow::Result<String> {
        let sym = symbol.trim().to_uppercase().replace('/', "-");
        if sym.contains('-') {
            let parts: Vec<&str> = sym.split('-').filter(|p| !p.is_empty()).collect();
            let [base, quote] = parts.as_slice() else {
                anyhow::bail!("Binance account symbol '{symbol}' is not BASE-QUOTE");
            };
            if !matches!(*quote, "USDT" | "USDC" | "USD") {
                anyhow::bail!("Binance account symbol '{symbol}' has unsupported quote");
            }
            return Ok(format!("{base}-{quote}"));
        }
        for quote in ["USDT", "USDC", "USD"] {
            if let Some(base) = sym.strip_suffix(quote) {
                if !base.is_empty() {
                    return Ok(format!("{base}-{quote}"));
                }
            }
        }
        anyhow::bail!("Binance account symbol '{symbol}' has no supported quote suffix")
    }

    fn credentials(&self) -> anyhow::Result<(Zeroizing<String>, Zeroizing<String>)> {
        let api_key = self
            .api_key
            .read()
            .clone()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Binance API key not configured or empty"))?;
        let api_secret = self
            .api_secret
            .read()
            .clone()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Binance API secret not configured or empty"))?;
        Ok((api_key, api_secret))
    }

    fn has_credentials(&self) -> bool {
        self.credentials().is_ok()
    }

    /// Sign request parameters with HMAC-SHA256 (Binance USDT-M standard).
    /// Returns the raw query string with `&signature=` appended.
    fn sign_query(secret: &Zeroizing<String>, query: &str) -> anyhow::Result<String> {
        use hmac::{digest::KeyInit, Hmac, Mac};
        use sha2::Sha256;
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|e| anyhow::anyhow!("Invalid Binance HMAC key: {e}"))?;
        mac.update(query.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());
        Ok(format!("{query}&signature={signature}"))
    }

    fn sign_params(&self, params: &BTreeMap<&str, String>) -> anyhow::Result<(String, String)> {
        let (api_key, api_secret) = self.credentials()?;
        let mut query = String::new();
        for (i, (k, v)) in params.iter().enumerate() {
            if i > 0 {
                query.push('&');
            }
            query.push_str(k);
            query.push('=');
            query.push_str(v);
        }
        let signed = Self::sign_query(&api_secret, &query)?;
        Ok((api_key.to_string(), signed))
    }

    /// Pure parse of a USDT-M `/fapi/v1/order` response. Split out so the
    /// wire mapping (statuses, fills, 1000-contract economics) is unit-tested
    /// without touching the network. Malformed responses fail closed: an
    /// acknowledgement is never converted into an invented order id, client
    /// id, fill quantity, price, or fee.
    fn parse_order_response(
        v: &serde_json::Value,
        order: &OrderRequest,
    ) -> anyhow::Result<OrderResponse> {
        use arbitrageur_core::types::OrderStatus;
        let expected_venue_symbol = Self::to_venue_symbol(&order.symbol)?;
        let venue_symbol = v
            .get("symbol")
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow::anyhow!("Binance order response missing symbol: {v}"))?;
        if venue_symbol != expected_venue_symbol {
            anyhow::bail!(
                "Binance order response symbol '{venue_symbol}' does not match expected '{expected_venue_symbol}': {v}"
            );
        }
        let order_id = match v.get("orderId") {
            Some(serde_json::Value::Number(n)) => n.to_string(),
            Some(serde_json::Value::String(s)) => s.trim().to_string(),
            _ => String::new(),
        };
        if order_id.is_empty() || order_id == "0" {
            anyhow::bail!("Binance order response missing valid orderId: {v}");
        }
        let client_order_id = v
            .get("clientOrderId")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Binance order response missing clientOrderId: {v}"))?;
        if client_order_id != order.client_order_id {
            anyhow::bail!(
                "Binance order response clientOrderId '{client_order_id}' does not match request '{}': {v}",
                order.client_order_id
            );
        }
        let raw_status = v
            .get("status")
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow::anyhow!("Binance order response missing status: {v}"))?;
        let orig_qty = dec_from_json(v.get("origQty"))
            .filter(|q| *q > dec!(0))
            .ok_or_else(|| anyhow::anyhow!("Binance order response missing valid origQty: {v}"))?;
        if orig_qty != order.quantity {
            anyhow::bail!(
                "Binance order response origQty {orig_qty} does not match request {}: {v}",
                order.quantity
            );
        }
        let executed_qty = dec_from_json(v.get("executedQty"))
            .filter(|q| *q >= dec!(0))
            .ok_or_else(|| {
                anyhow::anyhow!("Binance order response missing valid executedQty: {v}")
            })?;
        if executed_qty > orig_qty {
            anyhow::bail!(
                "Binance order response executedQty {executed_qty} exceeds origQty {orig_qty}: {v}"
            );
        }
        let remaining_qty = orig_qty.saturating_sub(executed_qty);
        let order_status = match raw_status {
            "FILLED" => OrderStatus::Filled,
            "PARTIALLY_FILLED" => OrderStatus::PartiallyFilled,
            "EXPIRED" if executed_qty > dec!(0) => OrderStatus::PartiallyFilled,
            "EXPIRED" => OrderStatus::Cancelled,
            "CANCELED" => OrderStatus::Cancelled,
            "REJECTED" => OrderStatus::Rejected,
            "NEW" => OrderStatus::New,
            other => anyhow::bail!("Binance order response has unknown status '{other}': {v}"),
        };
        let status_consistent = match order_status {
            OrderStatus::Filled => executed_qty == orig_qty && remaining_qty == dec!(0),
            OrderStatus::PartiallyFilled => executed_qty > dec!(0) && executed_qty < orig_qty,
            OrderStatus::New => executed_qty == dec!(0) && remaining_qty == orig_qty,
            OrderStatus::Cancelled | OrderStatus::Rejected | OrderStatus::Expired => {
                executed_qty == dec!(0) && remaining_qty == orig_qty
            }
        };
        if !status_consistent {
            anyhow::bail!(
                "Binance order response status {order_status:?} is inconsistent with executedQty {executed_qty}/{orig_qty}: {v}"
            );
        }
        let avg_price = if executed_qty > dec!(0) {
            let px = dec_from_json(v.get("avgPrice"))
                .filter(|p| *p > dec!(0))
                .ok_or_else(|| {
                    anyhow::anyhow!("Binance filled order response missing valid avgPrice: {v}")
                })?;
            Some(px)
        } else {
            None
        };
        let cum_quote = if executed_qty > dec!(0) {
            dec_from_json(v.get("cumQuote"))
                .filter(|q| *q >= dec!(0))
                .ok_or_else(|| {
                    anyhow::anyhow!("Binance filled order response missing valid cumQuote: {v}")
                })?
        } else {
            dec!(0)
        };
        // Binance USDT-M VIP0 taker fee: 5 BPS of notional.
        let fee = cum_quote * Decimal::new(5, 4);
        Ok(OrderResponse {
            order_id,
            client_order_id: client_order_id.to_string(),
            venue: VenueId::new("binance"),
            symbol: order.symbol.clone(),
            status: order_status,
            original_qty: orig_qty,
            filled_qty: executed_qty,
            remaining_qty,
            avg_fill_price: avg_price,
            fee,
            fee_currency: "USDT".to_string(),
            raw_response: Some(v.to_string()),
            timestamp: Utc::now(),
        })
    }

    async fn signed_post(
        &self,
        path: &str,
        params: &BTreeMap<&str, String>,
    ) -> anyhow::Result<serde_json::Value> {
        let (api_key, query_string) = self.sign_params(params)?;
        let url = format!("{}{}?{}", self.base_url, path, query_string);
        let resp = self
            .client
            .post(&url)
            .header("X-MBX-APIKEY", api_key)
            .send()
            .await
            .map_err(|e| {
                self.rate_limiter.record_failure(false);
                anyhow::anyhow!("Binance POST {path} transport failure: {e}")
            })?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            self.rate_limiter
                .record_failure(status == reqwest::StatusCode::TOO_MANY_REQUESTS);
            anyhow::bail!("Binance POST {path} failed (HTTP {status}): {text}");
        }
        serde_json::from_str::<serde_json::Value>(&text).map_err(|e| {
            anyhow::anyhow!("Failed to parse Binance {path} response: {e}; body: {text}")
        })
    }

    async fn signed_get(
        &self,
        path: &str,
        params: &BTreeMap<&str, String>,
    ) -> anyhow::Result<serde_json::Value> {
        let (api_key, query_string) = self.sign_params(params)?;
        let url = format!("{}{}?{}", self.base_url, path, query_string);
        let resp = self
            .client
            .get(&url)
            .header("X-MBX-APIKEY", api_key)
            .send()
            .await
            .map_err(|e| {
                self.rate_limiter.record_failure(false);
                anyhow::anyhow!("Binance GET {path} transport failure: {e}")
            })?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            self.rate_limiter
                .record_failure(status == reqwest::StatusCode::TOO_MANY_REQUESTS);
            anyhow::bail!("Binance GET {path} failed (HTTP {status}): {text}");
        }
        serde_json::from_str::<serde_json::Value>(&text).map_err(|e| {
            anyhow::anyhow!("Failed to parse Binance {path} response: {e}; body: {text}")
        })
    }

    /// Pure parse of a USDT-M `GET /fapi/v2/account` envelope into an
    /// [`AccountSnapshot`]. Split out for unit testing (no network).
    fn parse_account_response(&self, v: &serde_json::Value) -> anyhow::Result<AccountSnapshot> {
        let wallet_balance = dec_from_json(v.get("totalWalletBalance")).unwrap_or_default();
        let unrealized_pnl = dec_from_json(v.get("totalUnrealizedProfit")).unwrap_or_default();
        let equity =
            dec_from_json(v.get("totalMarginBalance")).unwrap_or(wallet_balance + unrealized_pnl);
        let free_margin = dec_from_json(v.get("availableBalance")).unwrap_or_default();
        let maint_margin = dec_from_json(v.get("totalMaintMargin")).unwrap_or_default();
        let initial_margin = dec_from_json(v.get("totalInitialMargin")).unwrap_or_default();

        let margin_usage_pct = if equity > dec!(0) {
            (initial_margin / equity) * dec!(100)
        } else {
            dec!(0)
        };

        let mut positions = Vec::new();
        if let Some(pos_arr) = v.get("positions").and_then(|p| p.as_array()) {
            for p in pos_arr {
                let amt = dec_from_json(p.get("positionAmt")).unwrap_or_default();
                if amt.is_zero() {
                    continue;
                }
                let sym = p.get("symbol").and_then(|s| s.as_str()).unwrap_or_default();
                let entry_px = dec_from_json(p.get("entryPrice")).unwrap_or_default();
                let upnl = dec_from_json(p.get("unrealizedProfit")).unwrap_or_default();
                let liq_px = dec_from_json(p.get("liquidationPrice")).filter(|x| *x > dec!(0));
                let lev = p
                    .get("leverage")
                    .and_then(|l| match l {
                        serde_json::Value::Number(n) => {
                            n.as_f64().and_then(Decimal::from_f64_retain)
                        }
                        serde_json::Value::String(s) => Decimal::from_str_exact(s).ok(),
                        _ => None,
                    })
                    .unwrap_or(dec!(1));

                let side = if amt > dec!(0) { Side::Buy } else { Side::Sell };
                let qty = amt.abs();
                let notional = qty * entry_px;
                let im = if lev > dec!(0) {
                    notional / lev
                } else {
                    notional
                };

                positions.push(arbitrageur_core::position::Position {
                    venue: self.venue_id.clone(),
                    symbol: sym.to_string(),
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

        let balances = vec![arbitrageur_core::position::Balance {
            venue: self.venue_id.clone(),
            currency: "USDT".to_string(),
            total: wallet_balance,
            free: free_margin,
            locked: wallet_balance.saturating_sub(free_margin),
        }];

        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity,
            free_margin,
            maintenance_margin: maint_margin,
            margin_usage_pct,
            balances,
            positions,
            timestamp: Utc::now(),
        })
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
            let contract_type = item
                .get("contractType")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            if status != "TRADING" || contract_type != "PERPETUAL" {
                continue;
            }

            let venue_symbol = match item.get("symbol").and_then(|s| s.as_str()) {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => continue,
            };
            let base_asset = item.get("baseAsset").and_then(|s| s.as_str()).unwrap_or("");
            let quote_asset = item
                .get("quoteAsset")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            if base_asset.is_empty() || quote_asset.is_empty() {
                continue;
            }

            let price_decimals = item
                .get("pricePrecision")
                .and_then(|n| n.as_u64())
                .unwrap_or(2) as u32;
            let size_decimals = item
                .get("quantityPrecision")
                .and_then(|n| n.as_u64())
                .unwrap_or(3) as u32;

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

    async fn fetch_market_prices(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        let val = self.get_json("/fapi/v1/premiumIndex").await?;
        let rows = val
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Binance premiumIndex response is not an array"))?;

        let mut out = HashMap::new();
        for row in rows {
            let Some(symbol) = row.get("symbol").and_then(|s| s.as_str()) else {
                continue;
            };
            let Some(price) = dec_from_json(row.get("markPrice")).filter(|p| *p > dec!(0)) else {
                continue;
            };
            out.insert(symbol.to_string(), price);
        }
        Ok(out)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let venue_sym = Self::to_venue_symbol(symbol)?;
        let path = format!(
            "/fapi/v1/depth?symbol={}&limit={}",
            venue_sym, ORDERBOOK_LIMIT
        );
        let val = self.get_json(&path).await?;

        // Venue-native units: depth prices/quantities are already denominated
        // in the listed contract (e.g. 1000PEPE contracts) — no scaling.
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
        let venue_sym = Self::to_venue_symbol(symbol)?;
        let path = format!("/fapi/v1/ticker/bookTicker?symbol={}", venue_sym);
        let val = self.get_json(&path).await?;

        let bid = dec_from_json(val.get("bidPrice")).unwrap_or(dec!(0));
        let ask = dec_from_json(val.get("askPrice")).unwrap_or(dec!(0));
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
        let venue_sym = Self::to_venue_symbol(symbol)?;
        let path = format!("/fapi/v1/premiumIndex?symbol={}", venue_sym);
        let val = self.get_json(&path).await?;

        let rate = dec_from_json(val.get("lastFundingRate"))
            .ok_or_else(|| anyhow::anyhow!("Binance missing lastFundingRate for {}", symbol))?;
        let next_time_ms = val
            .get("nextFundingTime")
            .and_then(|n| n.as_i64())
            .unwrap_or(0);
        let next_funding_time =
            DateTime::from_timestamp_millis(next_time_ms).unwrap_or_else(Utc::now);

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
        // Join to exchangeInfo so hub keys are canonical BASE-QUOTE — the
        // exact spelling the market universe advertises and scanners query.
        // Raw venue symbols (BTCUSDT) never match a hub lookup, so without
        // this join the bulk sweep feeds only the EMA and every scan falls
        // back to per-symbol fetches.
        let info = self.get_json("/fapi/v1/exchangeInfo").await?;
        let canon_of: HashMap<String, String> = info
            .get("symbols")
            .and_then(|s| s.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| {
                        let vsym = s.get("symbol")?.as_str()?.to_uppercase();
                        let base = s.get("baseAsset")?.as_str()?;
                        let quote = s.get("quoteAsset")?.as_str()?;
                        if base.is_empty() || quote.is_empty() {
                            return None;
                        }
                        Some((
                            vsym,
                            format!("{}-{}", base.to_uppercase(), quote.to_uppercase()),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let val = self.get_json("/fapi/v1/premiumIndex").await?;
        let arr = val
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Binance bulk premiumIndex not an array"))?;

        let mut out = Vec::new();
        for item in arr {
            let venue_sym = match item.get("symbol").and_then(|s| s.as_str()) {
                Some(s) if !s.is_empty() => s.to_uppercase(),
                _ => continue,
            };
            let Some(symbol) = canon_of.get(&venue_sym).cloned() else {
                continue;
            };
            if let Some(rate) = dec_from_json(item.get("lastFundingRate")) {
                let next_time_ms = item
                    .get("nextFundingTime")
                    .and_then(|n| n.as_i64())
                    .unwrap_or(0);
                let next_funding_time =
                    DateTime::from_timestamp_millis(next_time_ms).unwrap_or_else(Utc::now);
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
                // Canonical `BASE-QUOTE` keys so downstream matchers resolve
                // regardless of venue spelling (BTCUSDT -> BTC-USDT).
                if let Ok(key) = Self::canonical_account_symbol(sym) {
                    out.insert(key, vol);
                }
            }
        }
        Ok(out)
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        if !self.has_credentials() {
            anyhow::bail!("Cannot fetch live account on Binance: API credentials are not configured or vault is locked");
        }
        let mut params = BTreeMap::new();
        params.insert("timestamp", Utc::now().timestamp_millis().to_string());
        params.insert("recvWindow", "5000".to_string());
        let v = self.signed_get("/fapi/v2/account", &params).await?;
        self.parse_account_response(&v)
    }

    fn normalize_account_position(&self, position: &Position) -> anyhow::Result<Option<Position>> {
        let mut position = position.clone();
        position.symbol = Self::canonical_account_symbol(&position.symbol)?;
        Ok(Some(position))
    }

    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        if !self.has_credentials() {
            anyhow::bail!("Cannot place live order on Binance: API credentials are not configured or vault is locked");
        }
        if order.quantity <= dec!(0) {
            anyhow::bail!("Binance order quantity must be positive");
        }
        let venue_symbol = Self::to_venue_symbol(&order.symbol)?;
        // Binance enforces PRICE_FILTER.tickSize and LOT_SIZE.stepSize —
        // resolved before the params map consumes the symbol.
        let (tick_size, step_size, min_qty) = self.symbol_filters(&venue_symbol).await?;
        let mut params = BTreeMap::new();
        params.insert("symbol", venue_symbol.clone());
        let side_str = match order.side {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        };
        params.insert("side", side_str.to_string());
        let (type_str, tif_str) = match order.order_type {
            OrderType::Market => ("MARKET", None),
            OrderType::PostOnly => ("LIMIT", Some("GTX")),
            OrderType::Limit | OrderType::ImmediateOrCancel => {
                let tif = match order.time_in_force {
                    TimeInForce::ImmediateOrCancel => "IOC",
                    TimeInForce::FillOrKill => "FOK",
                    TimeInForce::GoodTilCancelled => "GTC",
                    TimeInForce::PostOnly => "GTX",
                };
                ("LIMIT", Some(tif))
            }
        };
        params.insert("type", type_str.to_string());
        if let Some(tif) = tif_str {
            params.insert("timeInForce", tif.to_string());
        }
        let qty_wire = binance_wire_qty(order.quantity, step_size);
        if qty_wire <= dec!(0) || qty_wire < min_qty {
            anyhow::bail!(
                "Binance {} quantity {} below lot floor (step {}, min {})",
                venue_symbol,
                order.quantity,
                step_size,
                min_qty
            );
        }
        params.insert("quantity", qty_wire.normalize().to_string());
        if let Some(p) = order.price {
            if p <= dec!(0) {
                anyhow::bail!("Binance order limit price must be positive");
            }
            let pw = binance_wire_price(p, order.side, tick_size);
            if pw <= dec!(0) {
                anyhow::bail!(
                    "Binance {} price {p} rounds below the tick floor",
                    venue_symbol
                );
            }
            params.insert("price", pw.normalize().to_string());
        }
        if order.reduce_only {
            params.insert("reduceOnly", "true".to_string());
        }
        if !order.client_order_id.is_empty() {
            params.insert("newClientOrderId", order.client_order_id.clone());
        }
        params.insert("timestamp", Utc::now().timestamp_millis().to_string());
        params.insert("recvWindow", "5000".to_string());

        let v = self.signed_post("/fapi/v1/order", &params).await?;
        Self::parse_order_response(&v, &order)
    }

    async fn wire_base_qty(&self, symbol: &str, qty: Decimal) -> anyhow::Result<Decimal> {
        let venue_symbol = Self::to_venue_symbol(symbol)?;
        let (_tick, step_size, min_qty) = self.symbol_filters(&venue_symbol).await?;
        let wire = binance_wire_qty(qty, step_size);
        if wire <= dec!(0) || wire < min_qty {
            anyhow::bail!(
                "Binance {venue_symbol} quantity {qty} below lot floor (step {step_size}, min {min_qty})"
            );
        }
        Ok(wire)
    }

    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
        if !self.has_credentials() {
            anyhow::bail!("Cannot cancel live order on Binance: API credentials are not configured or vault is locked");
        }
        let venue_symbol = Self::to_venue_symbol(&req.symbol)?;
        let mut params = BTreeMap::new();
        params.insert("symbol", venue_symbol);
        if let Some(id) = req.order_id {
            params.insert("orderId", id);
        }
        if let Some(cid) = req.client_order_id {
            params.insert("origClientOrderId", cid);
        }
        params.insert("timestamp", Utc::now().timestamp_millis().to_string());
        params.insert("recvWindow", "5000".to_string());
        // Binance cancels via DELETE /fapi/v1/order with a signed query.
        let (api_key, query_string) = self.sign_params(&params)?;
        let url = format!("{}/fapi/v1/order?{}", self.base_url, query_string);
        let resp = self
            .client
            .delete(&url)
            .header("X-MBX-APIKEY", api_key)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Binance DELETE /fapi/v1/order transport failure: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("Binance cancel failed (HTTP {status}): {text}");
        }
        Ok(())
    }

    async fn set_leverage(&self, symbol: &str, leverage: u32) -> anyhow::Result<()> {
        if !self.has_credentials() {
            anyhow::bail!("Cannot set leverage on Binance: API credentials are not configured or vault is locked");
        }
        if !(1..=125).contains(&leverage) {
            anyhow::bail!("Binance leverage {leverage} out of range 1..=125");
        }
        let venue_symbol = Self::to_venue_symbol(symbol)?;
        let mut params = BTreeMap::new();
        params.insert("symbol", venue_symbol);
        params.insert("leverage", leverage.to_string());
        params.insert("timestamp", Utc::now().timestamp_millis().to_string());
        params.insert("recvWindow", "5000".to_string());
        let v = self.signed_post("/fapi/v1/leverage", &params).await?;
        let got = v.get("leverage").and_then(|l| l.as_u64()).unwrap_or(0);
        if got != u64::from(leverage) {
            anyhow::bail!("Binance leverage ack mismatch: wanted {leverage}, venue reports {got}");
        }
        Ok(())
    }

    /// Poll authoritative order state via `GET /fapi/v1/order`. Same field
    /// shape as the placement ack, so the fail-closed parser is reused —
    /// quantities/prices arrive venue-native, matching submission.
    async fn fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        _client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        if !self.has_credentials() {
            anyhow::bail!("Cannot fetch order on Binance: API credentials are not configured or vault is locked");
        }
        if order_id.trim().is_empty() {
            anyhow::bail!("Binance fetch_order requires a non-empty order_id");
        }
        let venue_symbol = Self::to_venue_symbol(symbol)?;
        let mut params = BTreeMap::new();
        params.insert("symbol", venue_symbol);
        params.insert("orderId", order_id.to_string());
        params.insert("timestamp", Utc::now().timestamp_millis().to_string());
        params.insert("recvWindow", "5000".to_string());
        let v = self.signed_get("/fapi/v1/order", &params).await?;

        // Probe request echoes response fields so the shared parser validates
        // symbol + status/quantity coherence; orderId identity is checked
        // explicitly below.
        let probe = OrderRequest {
            client_order_id: v
                .get("clientOrderId")
                .and_then(|s| s.as_str())
                .unwrap_or_default()
                .to_string(),
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            side: Side::Buy,
            order_type: OrderType::Limit,
            price: None,
            quantity: dec_from_json(v.get("origQty")).unwrap_or_default(),
            time_in_force: TimeInForce::GoodTilCancelled,
            post_only: false,
            reduce_only: false,
            created_at: Utc::now(),
        };
        let resp = Self::parse_order_response(&v, &probe)?;
        if resp.order_id != order_id {
            anyhow::bail!(
                "Binance fetch_order returned orderId {} != queried {order_id}",
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
    fn sign_query_matches_verified_hmac_vector() {
        // Cross-checked with `openssl dgst -sha256 -hmac` and Python hmac.
        let secret = Zeroizing::new("NhqPtmdSJgDdgUiO4H5tzsCKM2HUAngnh".to_string());
        let query = "symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC&quantity=1&price=0.1&recvWindow=5000&timestamp=1499827319559";
        let signed = BinanceAdapter::sign_query(&secret, query).unwrap();
        assert!(
            signed.ends_with(
                "&signature=8a926a3f21039123592b924588745b6d3fa7de4aa613870fe8dc657ff047706a"
            ),
            "unexpected signed query: {signed}"
        );
    }

    #[test]
    fn parse_order_response_maps_statuses_and_fees() {
        use arbitrageur_core::order::OrderRequest;
        use arbitrageur_core::types::{OrderStatus, Side};
        let order =
            OrderRequest::new_market(VenueId::new("binance"), "BTC-USDT", Side::Buy, dec!(0.01));
        let v: serde_json::Value = serde_json::json!({
            "symbol": "BTCUSDT",
            "orderId": 12345,
            "clientOrderId": order.client_order_id,
            "status": "FILLED",
            "origQty": "0.010",
            "executedQty": "0.010",
            "avgPrice": "80000.5",
            "cumQuote": "800.005"
        });
        let resp = BinanceAdapter::parse_order_response(&v, &order).unwrap();
        assert_eq!(resp.status, OrderStatus::Filled);
        assert_eq!(resp.filled_qty, dec!(0.01));
        assert_eq!(resp.avg_fill_price, Some(dec!(80000.5)));
        // 5 BPS taker of 800.005 notional.
        assert!((resp.fee - dec!(0.4000025)).abs() < dec!(0.0000001));
        let rej: serde_json::Value = serde_json::json!({
            "symbol": "BTCUSDT",
            "orderId": 12346,
            "clientOrderId": order.client_order_id,
            "status": "REJECTED",
            "origQty": "0.010",
            "executedQty": "0"
        });
        let resp = BinanceAdapter::parse_order_response(&rej, &order).unwrap();
        assert_eq!(resp.status, OrderStatus::Rejected);
        assert_eq!(resp.filled_qty, dec!(0));
    }

    #[test]
    fn parse_order_response_rejects_mismatched_identity_and_inconsistent_fill() {
        use arbitrageur_core::order::OrderRequest;
        use arbitrageur_core::types::Side;
        let order =
            OrderRequest::new_market(VenueId::new("binance"), "BTC-USDT", Side::Buy, dec!(0.01));
        let base = serde_json::json!({
            "symbol": "BTCUSDT",
            "orderId": 12345,
            "clientOrderId": order.client_order_id,
            "status": "FILLED",
            "origQty": "0.010",
            "executedQty": "0.010",
            "avgPrice": "80000.5",
            "cumQuote": "800.005"
        });
        let mut wrong_symbol = base.clone();
        wrong_symbol["symbol"] = serde_json::json!("ETHUSDT");
        assert!(BinanceAdapter::parse_order_response(&wrong_symbol, &order).is_err());
        let mut wrong_client = base.clone();
        wrong_client["clientOrderId"] = serde_json::json!("other-client-id");
        assert!(BinanceAdapter::parse_order_response(&wrong_client, &order).is_err());
        let mut inconsistent = base.clone();
        inconsistent["executedQty"] = serde_json::json!("0");
        assert!(BinanceAdapter::parse_order_response(&inconsistent, &order).is_err());
        let mut unknown_status = base.clone();
        unknown_status["status"] = serde_json::json!("PENDING_NEW");
        assert!(BinanceAdapter::parse_order_response(&unknown_status, &order).is_err());
    }

    #[test]
    fn parse_account_response_maps_balances_and_positions() {
        let adapter = BinanceAdapter::new(None, None, None);
        let v: serde_json::Value = serde_json::json!({
            "totalWalletBalance": "10000.0",
            "totalUnrealizedProfit": "120.0",
            "totalMarginBalance": "10120.0",
            "availableBalance": "9000.0",
            "totalInitialMargin": "1120.0",
            "totalMaintMargin": "56.0",
            "positions": [
                {"symbol": "BTCUSDT", "positionAmt": "0.500", "entryPrice": "80000.0",
                 "unrealizedProfit": "100.0", "liquidationPrice": "40000.0", "leverage": "10"},
                {"symbol": "ETHUSDT", "positionAmt": "0.000", "entryPrice": "0",
                 "unrealizedProfit": "0", "leverage": "10"}
            ]
        });
        let acc = adapter.parse_account_response(&v).unwrap();
        assert_eq!(acc.equity, dec!(10120.0));
        assert_eq!(acc.free_margin, dec!(9000.0));
        assert_eq!(acc.maintenance_margin, dec!(56.0));
        // 1120 / 10120 * 100 ~= 11.067%.
        assert!((acc.margin_usage_pct - dec!(11.067)).abs() < dec!(0.01));
        assert_eq!(acc.positions.len(), 1);
        assert_eq!(acc.positions[0].symbol, "BTCUSDT");
        assert_eq!(acc.positions[0].quantity, dec!(0.5));
        assert_eq!(acc.balances.len(), 1);
        assert_eq!(acc.balances[0].currency, "USDT");
        let pos = adapter
            .account_position_for(&acc, "BTC-USDT")
            .unwrap()
            .expect("venue-native symbol should normalize");
        assert_eq!(pos.symbol, "BTC-USDT");
        assert!(adapter
            .account_position_for(&acc, "BTC-USDC")
            .unwrap()
            .is_none());
    }

    #[test]
    fn to_venue_symbol_preserves_quote_and_1000x_base() {
        // Exact-quote preservation: BTC-USDC must never route to BTCUSDT.
        assert_eq!(
            BinanceAdapter::to_venue_symbol("BTC-USDC").unwrap(),
            "BTCUSDC"
        );
        assert_eq!(
            BinanceAdapter::to_venue_symbol("BTC-USDT").unwrap(),
            "BTCUSDT"
        );
        // 1000-prefixed contracts are venue-native units — verbatim passthrough,
        // no scaling flag. `PEPE-USDT` stays `PEPEUSDT` (a different listing).
        assert_eq!(
            BinanceAdapter::to_venue_symbol("1000PEPE-USDT").unwrap(),
            "1000PEPEUSDT"
        );
        assert_eq!(
            BinanceAdapter::to_venue_symbol("PEPE-USDT").unwrap(),
            "PEPEUSDT"
        );
        // Concatenated explicit-quote spellings also work.
        assert_eq!(
            BinanceAdapter::to_venue_symbol("ETHUSDT").unwrap(),
            "ETHUSDT"
        );
        assert_eq!(
            BinanceAdapter::to_venue_symbol("1000PEPEUSDT").unwrap(),
            "1000PEPEUSDT"
        );
        // Bare bases and foreign quotes fail closed.
        assert!(BinanceAdapter::to_venue_symbol("PEPE").is_err());
        assert!(BinanceAdapter::to_venue_symbol("BTC").is_err());
        assert!(BinanceAdapter::to_venue_symbol("BTC-SOL").is_err());
        assert!(BinanceAdapter::to_venue_symbol("BTC-EUR").is_err());
    }

    #[test]
    fn parse_order_response_keeps_1000x_qty_venue_native() {
        use arbitrageur_core::order::OrderRequest;
        use arbitrageur_core::types::{OrderStatus, Side};
        // Engine qty 5 on `1000PEPE-USDT` = 5 venue contracts — submitted
        // verbatim, so the ack must echo origQty=5, not 5000.
        let order =
            OrderRequest::new_market(VenueId::new("binance"), "1000PEPE-USDT", Side::Buy, dec!(5));
        let v: serde_json::Value = serde_json::json!({
            "symbol": "1000PEPEUSDT",
            "orderId": 777,
            "clientOrderId": order.client_order_id,
            "status": "FILLED",
            "origQty": "5",
            "executedQty": "5",
            "avgPrice": "0.012",
            "cumQuote": "0.06"
        });
        let resp = BinanceAdapter::parse_order_response(&v, &order).unwrap();
        assert_eq!(resp.status, OrderStatus::Filled);
        assert_eq!(resp.filled_qty, dec!(5));
        assert_eq!(resp.avg_fill_price, Some(dec!(0.012)));
    }

    #[test]
    fn wire_values_respect_tick_and_step_filters() {
        use arbitrageur_core::types::Side;
        // Quantity floors to stepSize — never rounds up into an oversize.
        assert_eq!(binance_wire_qty(dec!(0.004999), dec!(0.001)), dec!(0.004));
        assert_eq!(binance_wire_qty(dec!(1.005), dec!(0.01)), dec!(1.00));
        assert_eq!(binance_wire_qty(dec!(2.5), dec!(0)), dec!(2.5));
        // Buy price floors (protective cap never rises); sell price ceils.
        assert_eq!(
            binance_wire_price(dec!(2734.2783), Side::Buy, dec!(0.01)),
            dec!(2734.27)
        );
        assert_eq!(
            binance_wire_price(dec!(2734.2783), Side::Sell, dec!(0.01)),
            dec!(2734.28)
        );
        // Exact multiples pass through unchanged; zero tick disables.
        assert_eq!(
            binance_wire_price(dec!(2734.28), Side::Buy, dec!(0.01)),
            dec!(2734.28)
        );
        assert_eq!(
            binance_wire_price(dec!(0.012345), Side::Sell, dec!(0.000001)),
            dec!(0.012345)
        );
        assert_eq!(
            binance_wire_price(dec!(1.234), Side::Buy, dec!(0)),
            dec!(1.234)
        );
    }
}
