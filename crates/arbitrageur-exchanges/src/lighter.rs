use std::collections::HashMap;
use std::str::FromStr;
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::{AccountSnapshot, Balance, Position};
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{dec, Decimal, OrderStatus, OrderType, Side, TimeInForce, VenueId};
use crate::rate_limiter::AccountRequestCoordinator;
use parking_lot::RwLock;
use zeroize::Zeroizing;

/// Lighter zk-Rollup DEX adapter.
#[allow(dead_code)]
pub struct LighterAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    account_index: RwLock<Option<u64>>,
    api_key_index: RwLock<Option<u8>>,
    api_key_private_key: RwLock<Option<Zeroizing<String>>>,
    rate_limiter: AccountRequestCoordinator,
    /// Serialises nonce-fetch -> sign -> submit so concurrent orders never
    /// reuse a server nonce (a reused nonce is rejected; worse, a caller
    /// might misread the rejection as a fill problem).
    order_lock: tokio::sync::Mutex<()>,
}

impl LighterAdapter {
    pub fn new(
        base_url: Option<String>,
        account_index: Option<u64>,
        api_key_private_key: Option<String>,
    ) -> Self {
        Self::new_with_venue(base_url, account_index, api_key_private_key, "lighter")
    }

    /// Lighter-protocol deployment under a different venue identity — e.g.
    /// Robinhood Chain serves the same API at api.rh.lighter.xyz.
    pub fn new_with_venue(
        base_url: Option<String>,
        account_index: Option<u64>,
        api_key_private_key: Option<String>,
        venue_id: &str,
    ) -> Self {
        let base = base_url.unwrap_or_else(|| "https://mainnet.zklighter.elliot.ai".to_string());
        Self {
            venue_id: VenueId::new(venue_id),
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
            account_index: parking_lot::RwLock::new(account_index),
            api_key_index: parking_lot::RwLock::new(None),
            api_key_private_key: parking_lot::RwLock::new(api_key_private_key.map(Zeroizing::new)),
            order_lock: tokio::sync::Mutex::new(()),
            rate_limiter: AccountRequestCoordinator::new(venue_id, 200, 3000, 500, 15000),
        }
    }

    pub fn update_credentials(
        &self,
        account_index: Option<u64>,
        api_key_private_key: Option<String>,
    ) {
        *self.account_index.write() = account_index;
        *self.api_key_private_key.write() = api_key_private_key.map(Zeroizing::new);
    }

    /// Lighter API-key slot that signed the key (UI: 2-254; 0-1 are the
    /// web/mobile slots). The Go-side validator rejects 255, so there is no
    /// wildcard default — orders fail closed until this is configured.
    pub fn set_api_key_index(&self, api_key_index: Option<u8>) {
        *self.api_key_index.write() = api_key_index;
    }

    fn signing_identity(&self) -> anyhow::Result<(u64, u8, Zeroizing<String>)> {
        let account = self
            .account_index
            .read()
            .ok_or_else(|| anyhow::anyhow!("Lighter live orders require an account index"))?;
        if account == 0 {
            anyhow::bail!("Lighter live orders require a non-zero account index");
        }
        let key_idx = self
            .api_key_index
            .read()
            .ok_or_else(|| anyhow::anyhow!("Lighter live orders require an API Key Index (2-254)"))?;
        if key_idx > 254 {
            anyhow::bail!("Lighter API Key Index must be 0-254");
        }
        let key = self
            .api_key_private_key
            .read()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Lighter live orders require the API private key"))?;
        if key.trim().is_empty() {
            anyhow::bail!("Lighter live orders require a non-empty API private key");
        }
        Ok((account, key_idx, key))
    }

    fn chain_id(&self) -> u32 {
        lighter_chain_id_for_url(&self.base_url)
    }

    /// Fresh server nonce for (account, api-key). Bypasses the read cache:
    /// a cached nonce would double-submit and get rejected.
    async fn fetch_next_nonce(&self, account: u64, api_key_index: u8) -> anyhow::Result<i64> {
        let url = format!(
            "{}/api/v1/nextNonce?account_index={}&api_key_index={}",
            self.base_url, account, api_key_index
        );
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let snippet: String = resp.text().await.unwrap_or_default().chars().take(256).collect();
            anyhow::bail!("Lighter nextNonce failed: {} {}", status, snippet);
        }
        let v: serde_json::Value = resp.json().await?;
        v.get("nonce")
            .and_then(|n| n.as_i64())
            .ok_or_else(|| anyhow::anyhow!("Lighter nextNonce response carries no nonce"))
    }

    /// Submit a signed L2 transaction exactly like the official Python SDK:
    /// `POST /api/v1/sendTx` as `application/x-www-form-urlencoded` with
    /// `tx_type` + `tx_info`. `code == 200` is the only accepted outcome.
    async fn submit_signed_tx(&self, signed: &crate::lighter_signer::SignedLighterTx) -> anyhow::Result<String> {
        let body = format!(
            "tx_type={}&tx_info={}",
            signed.tx_type,
            urlencoding::encode(&signed.tx_info)
        );
        let resp = self
            .client
            .post(format!("{}/api/v1/sendTx", self.base_url))
            .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await?;
        let ok = resp.status().is_success();
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        parse_send_tx_result(ok, status, &text, &signed.tx_hash)
    }

    async fn get_json(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        // 1. Check rate limiter and in-memory single-flight cache
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
                anyhow::bail!("Lighter edge challenge (WAF) on {} — cooling down", path);
            }
            self.rate_limiter.record_failure(is_429);
            anyhow::bail!("Lighter API error: {} {}", status, path);
        }

        let text = resp.text().await?;
        self.rate_limiter.record_success_path(path, text.clone());
        let data = serde_json::from_str::<serde_json::Value>(&text)?;
        Ok(data)
    }
}

#[async_trait]
impl ExchangeAdapter for LighterAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Lighter"
    }

    async fn ping(&self) -> anyhow::Result<()> {
        // Bypass the single-flight cache (get_json) and any CDN edge cache:
        // a unique query param forces the request to reach origin so the
        // measured round-trip reflects real exchange latency, not a cache hit.
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let url = format!("{}/api/v1/orderBookDetails?cb={}", self.base_url, nonce);
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            let headers = resp.headers().clone();
            let status = resp.status().as_u16();
            let snippet: String = resp.text().await.unwrap_or_default().chars().take(512).collect();
            if crate::rate_limiter::is_waf_challenge(status, &headers, &snippet) {
                self.rate_limiter.record_challenge();
                anyhow::bail!("Lighter ping hit edge challenge — cooling down");
            }
            anyhow::bail!("Lighter ping failed: {}", status);
        }
        // A WAF challenge page can return 200 HTML — only a real JSON
        // response counts as reaching the exchange.
        let is_json = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| ct.contains("json"))
            .unwrap_or(false);
        if !is_json {
            anyhow::bail!("Lighter ping returned non-JSON response (likely edge challenge)");
        }
        Ok(())
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let res = self.get_json("/api/v1/orderBookDetails").await?;
        let details = res.get("order_book_details").and_then(|d| d.as_array()).ok_or_else(|| {
            anyhow::anyhow!("Missing order_book_details in Lighter response")
        })?;

        let mut markets = Vec::new();
        for item in details {
            let symbol_str = item.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
            let market_id = item.get("market_id").and_then(|m| m.as_u64()).unwrap_or(0);
            let price_dec = item.get("price_decimals").and_then(|p| p.as_u64()).unwrap_or(4) as u32;
            let size_dec = item.get("size_decimals").and_then(|s| s.as_u64()).unwrap_or(2) as u32;

            let is_spot = item
                .get("market_type")
                .and_then(|t| t.as_str())
                .map(|t| t.eq_ignore_ascii_case("spot"))
                .unwrap_or(false);
            let symbol = if is_spot {
                Symbol::spot(symbol_str, "USDC")
            } else {
                Symbol::perp(symbol_str, "USDC")
            };
            markets.push(MarketMetadata {
                symbol,
                venue: self.venue_id.clone(),
                venue_symbol: market_id.to_string(),
                price_decimals: price_dec,
                size_decimals: size_dec,
                min_order_size: dec!(0.01),
                min_notional: dec!(10.0),
                maker_fee_bps: dec!(0.0),
                taker_fee_bps: dec!(2.0),
                contract_size: dec!(1.0),
                is_active: true,
            });
        }
        Ok(markets)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let details = self.get_json("/api/v1/orderBookDetails").await?;
        let market_id = resolve_market_id(&details, symbol)?;

        let res = self
            .get_json(&format!(
                "/api/v1/orderBookOrders?market_id={}&limit=50",
                market_id
            ))
            .await?;

        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());
        append_lighter_levels(&mut book, &res);

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
        let clean = symbol.trim().replace('/', "-");
        let parts: Vec<&str> = clean.split('-').collect();
        if parts.len() >= 2 {
            let quote = parts[1];
            if !quote.eq_ignore_ascii_case("USDC") && !quote.eq_ignore_ascii_case("USD") {
                anyhow::bail!("Lighter markets are quoted in USDC/USD; '{symbol}' is not supported");
            }
        }
        let coin = parts.first().copied().unwrap_or("").to_uppercase();
        if coin.is_empty() {
            anyhow::bail!("Lighter funding-rate request carries no market identifier");
        }
        let rates = self.fetch_funding_rates().await?;
        rates
            .into_iter()
            .find(|r| {
                r.symbol
                    .split('-')
                    .next()
                    .map(|b| b.eq_ignore_ascii_case(&coin))
                    .unwrap_or(false)
            })
            .ok_or_else(|| anyhow::anyhow!("No Lighter funding rate for {}", symbol))
    }

    /// Lighter-protocol venues expose `/api/v1/funding-rates`: one call
    /// returns every market's funding alongside third-party reference
    /// rates (binance, bybit, hyperliquid). Only the venue's own entries —
    /// labelled `exchange: "lighter"` on both mainnet and the Robinhood
    /// Chain deployment — are used; external exchange rows are context, not
    /// this venue's rate.
    ///
    /// The venue `rate` is an **8-hour aggregated print** (it tracks the
    /// venue UI's "8hr" figure; hourly ~= rate / 8), so `interval_hours` is
    /// 8 — the shared carry math normalizes by it. Mislabeling it as hourly
    /// inflates every Lighter-legged carry quote ~8x.
    async fn fetch_funding_rates(&self) -> anyhow::Result<Vec<FundingRate>> {
        let res = self.get_json("/api/v1/funding-rates").await?;
        let rows = res
            .get("funding_rates")
            .and_then(|r| r.as_array())
            .ok_or_else(|| anyhow::anyhow!("Missing funding_rates in Lighter response"))?;
        Ok(parse_lighter_funding_rates(rows, &self.venue_id))
    }

    async fn fetch_volumes_24h(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        let res = self.get_json("/api/v1/orderBookDetails").await?;
        let details = res.get("order_book_details").and_then(|d| d.as_array()).ok_or_else(|| {
            anyhow::anyhow!("Missing order_book_details in Lighter response")
        })?;

        fn as_decimal(v: &serde_json::Value) -> Option<Decimal> {
            v.as_str()
                .and_then(|s| Decimal::from_str(s).ok())
                .or_else(|| v.as_f64().and_then(|f| Decimal::try_from(f).ok()))
        }

        let mut volumes = HashMap::new();
        for item in details {
            let symbol_str = item.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
            if symbol_str.is_empty() {
                continue;
            }
            let quote_vol = item.get("daily_quote_token_volume").and_then(as_decimal);
            let base_vol = item.get("daily_base_token_volume").and_then(as_decimal);
            let last_px = item.get("last_trade_price").and_then(as_decimal);
            let vol = quote_vol.or_else(|| match (base_vol, last_px) {
                (Some(b), Some(p)) => Some(b * p),
                _ => None,
            });
            if let Some(v) = vol {
                volumes.insert(symbol_str.to_string(), v);
            }
        }
        Ok(volumes)
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        let idx = self
            .account_index
            .read()
            .ok_or_else(|| anyhow::anyhow!("Lighter account fetch requires an account index"))?;
        let path = format!("/api/v1/account?by=index&value={}", idx);

        let data = match self.rate_limiter.acquire_or_cache(true).await? {
            Some(cached) => serde_json::from_str(&cached)?,
            None => {
                let res = self.get_json(&path).await;
                match res {
                    Ok(val) => {
                        self.rate_limiter.record_success(val.to_string());
                        val
                    }
                    Err(e) => {
                        self.rate_limiter.record_failure(false);
                        return Err(e);
                    }
                }
            }
        };

        let account = data.get("account").or_else(|| {
            data.get("accounts").and_then(|a| a.as_array()).and_then(|arr| arr.first())
        });

        let total_val = account
            .and_then(|a| a.get("total_asset_value").or_else(|| a.get("portfolio_value")))
            .and_then(|v| v.as_str())
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(dec!(0));

        let free_val = account
            .and_then(|a| a.get("free_collateral").or_else(|| a.get("available_balance")))
            .and_then(|v| v.as_str())
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(total_val);

        let mut positions = Vec::new();
        if let Some(acc) = account {
            if let Some(pos_list) = acc.get("positions").and_then(|p| p.as_array()) {
                for p in pos_list {
                    let sym = p.get("symbol").and_then(|s| s.as_str()).unwrap_or("UNKNOWN");
                    // Lighter `position` is unsigned; direction comes from `sign` (1/-1).
                    let sz_str = p.get("position").and_then(|s| s.as_str()).unwrap_or("0");
                    let sz = Decimal::from_str(sz_str).unwrap_or(dec!(0));
                    let sign = p.get("sign").and_then(|s| s.as_i64()).unwrap_or(1);
                    let parse_str = |key: &str| {
                        p.get(key)
                            .and_then(|v| v.as_str())
                            .and_then(|s| Decimal::from_str(s).ok())
                    };
                    let entry_px = parse_str("avg_entry_price").unwrap_or(dec!(0));
                    let position_value = parse_str("position_value").unwrap_or(dec!(0));
                    let allocated = parse_str("allocated_margin").unwrap_or(dec!(0));

                    if sz != dec!(0) {
                        let qty = sz.abs();
                        positions.push(Position {
                            venue: self.venue_id.clone(),
                            symbol: sym.to_string(),
                            side: if sign < 0 { Side::Sell } else { Side::Buy },
                            quantity: qty,
                            entry_price: entry_px,
                            mark_price: if position_value > dec!(0) {
                                position_value / qty
                            } else {
                                entry_px
                            },
                            unrealized_pnl: parse_str("unrealized_pnl").unwrap_or(dec!(0)),
                            realized_pnl: parse_str("realized_pnl").unwrap_or(dec!(0)),
                            liquidation_price: parse_str("liquidation_price").filter(|p| *p > dec!(0)),
                            leverage: if allocated > dec!(0) {
                                position_value / allocated
                            } else {
                                dec!(0)
                            },
                            initial_margin: allocated,
                            // Lighter does not expose maintenance margin; using allocated
                            // (initial) margin overstates it toward earlier risk warnings.
                            maintenance_margin: allocated,
                            updated_at: Utc::now(),
                        });
                    }
                }
            }
        }

        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity: total_val,
            free_margin: free_val,
            maintenance_margin: total_val - free_val,
            margin_usage_pct: if total_val > dec!(0) { ((total_val - free_val) / total_val) * dec!(100) } else { dec!(0) },
            balances: vec![Balance {
                venue: self.venue_id.clone(),
                currency: "USD".to_string(),
                total: total_val,
                free: free_val,
                locked: total_val - free_val,
            }],
            positions,
            timestamp: Utc::now(),
        })
    }

    fn supports_live_execution(&self) -> bool {
        true
    }

    fn has_live_credentials(&self) -> bool {
        self.api_key_private_key
            .read()
            .as_ref()
            .is_some_and(|k| !k.trim().is_empty())
    }

    fn live_order_submission_implemented(&self) -> bool {
        true
    }

    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        let _guard = self.order_lock.lock().await;
        let (account, api_key_index, api_key) = self.signing_identity()?;
        if order.quantity <= dec!(0) {
            anyhow::bail!("Lighter order quantity must be positive");
        }
        if order.side != Side::Buy && order.side != Side::Sell {
            anyhow::bail!("Lighter order side must be buy or sell");
        }
        let is_ask = order.side == Side::Sell;

        // Market id + fixed-point scales come from the same payload so a
        // mid-update market list cannot mix an id with another market's
        // decimals (which would silently move the price ~10x per digit).
        let details = self.get_json("/api/v1/orderBookDetails").await?;
        let (market_id, price_dec, size_dec) = resolve_market_order_params(&details, &order.symbol)?;

        let base_amount = scale_lighter_amount(order.quantity, size_dec, &order.symbol)?;
        let (l2_type, tif, expiry_ms, limit_px) = match order.order_type {
            OrderType::Market => {
                if order.post_only {
                    anyhow::bail!("Lighter post-only market orders are not supported");
                }
                let book = self.fetch_orderbook(&order.symbol).await?;
                let worst_px =
                    market_limit_price(&book, order.side, order.quantity, &order.symbol)?;
                (1u8, 0u8, 0i64, worst_px)
            }
            OrderType::Limit | OrderType::ImmediateOrCancel => {
                let px = order.price.ok_or_else(|| {
                    anyhow::anyhow!("Lighter limit orders require a price")
                })?;
                if px <= dec!(0) {
                    anyhow::bail!("Lighter order limit price must be positive");
                }
                let tif = if order.post_only {
                    2u8
                } else {
                    match order.time_in_force {
                        TimeInForce::ImmediateOrCancel => 0u8,
                        TimeInForce::GoodTilCancelled => 1u8,
                        TimeInForce::PostOnly => 2u8,
                        TimeInForce::FillOrKill => anyhow::bail!(
                            "Lighter has no fill-or-kill; refusing to downgrade to IOC silently"
                        ),
                    }
                };
                let expiry_ms = if tif == 1 { -1 } else { 0 };
                (0u8, tif, expiry_ms, px)
            }
            OrderType::PostOnly => {
                let px = order.price.ok_or_else(|| {
                    anyhow::anyhow!("Lighter post-only orders require a price")
                })?;
                if px <= dec!(0) {
                    anyhow::bail!("Lighter order limit price must be positive");
                }
                (0u8, 2u8, -1i64, px)
            }
        };
        let price = scale_lighter_price(limit_px, price_dec, order.side, &order.symbol)?;

        let nonce = self.fetch_next_nonce(account, api_key_index).await?;
        let client_order_index =
            lighter_client_order_index(&order.client_order_id);
        let params = crate::lighter_signer::CreateOrderParams {
            market_index: market_id as i64,
            client_order_index,
            base_amount,
            price,
            is_ask,
            order_type: l2_type,
            time_in_force: tif,
            reduce_only: order.reduce_only,
            order_expiry_ms: expiry_ms,
            nonce,
            api_key_index: api_key_index as i64,
            account_index: account as i64,
        };
        let signed = crate::lighter_signer::sign_create_order(
            None,
            &self.base_url,
            self.chain_id(),
            api_key.as_str(),
            &params,
        )?;
        let tx_hash = self.submit_signed_tx(&signed).await?;
        Ok(OrderResponse {
            order_id: tx_hash,
            client_order_id: order.client_order_id.clone(),
            venue: self.venue_id.clone(),
            symbol: order.symbol.clone(),
            status: OrderStatus::New,
            original_qty: order.quantity,
            filled_qty: dec!(0),
            remaining_qty: order.quantity,
            avg_fill_price: None,
            fee: dec!(0),
            fee_currency: "USDC".to_string(),
            raw_response: Some(signed.tx_info),
            timestamp: Utc::now(),
        })
    }

    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
        let _guard = self.order_lock.lock().await;
        let (account, api_key_index, api_key) = self.signing_identity()?;
        let details = self.get_json("/api/v1/orderBookDetails").await?;
        let market_id = resolve_market_id(&details, &req.symbol)?;
        // The venue cancels by numeric index: a live order id from this
        // adapter is a tx_hash (hex, not an index), so cancellation keys off
        // the client order index — recomputed deterministically from the
        // client order id — or a directly supplied numeric index.
        let order_index: i64 = match req.order_id {
            Some(ref id) if id.parse::<i64>().is_ok() => id.parse::<i64>().unwrap_or(0),
            _ => match req.client_order_id {
                Some(ref cid) if !cid.trim().is_empty() => lighter_client_order_index(cid),
                _ => anyhow::bail!(
                    "Lighter cancel needs a numeric order index or the client order id"
                ),
            },
        };
        let nonce = self.fetch_next_nonce(account, api_key_index).await?;
        let signed = crate::lighter_signer::sign_cancel_order(
            None,
            &self.base_url,
            self.chain_id(),
            api_key.as_str(),
            market_id as i64,
            order_index,
            nonce,
            api_key_index as i64,
            account as i64,
        )?;
        self.submit_signed_tx(&signed).await?;
        Ok(())
    }

    async fn set_leverage(&self, _symbol: &str, _leverage: u32) -> anyhow::Result<()> {
        anyhow::bail!("Leverage updates are not implemented for venue {}", self.name())
    }
}

/// Parse one Lighter price-size entry: `price` plus
/// `remaining_base_amount` / `size` / `initial_base_amount` (all strings).
/// Returns `None` for malformed entries so callers skip rather than fabricate.
pub fn parse_lighter_price_size(entry: &serde_json::Value) -> Option<(Decimal, Decimal)> {
    let px = entry.get("price").and_then(|p| p.as_str())?;
    let sz = entry
        .get("remaining_base_amount")
        .or_else(|| entry.get("size"))
        .or_else(|| entry.get("initial_base_amount"))
        .and_then(|s| s.as_str())?;
    Some((Decimal::from_str(px).ok()?, Decimal::from_str(sz).ok()?))
}

/// Append REST/WS `bids` + `asks` arrays onto a book via
/// [`parse_lighter_price_size`].
pub fn append_lighter_levels(book: &mut OrderBook, payload: &serde_json::Value) {
    if let Some(bids) = payload.get("bids").and_then(|b| b.as_array()) {
        for b in bids {
            if let Some((p, s)) = parse_lighter_price_size(b) {
                book.bids.push(BookLevel::new(p, s));
            }
        }
    }
    if let Some(asks) = payload.get("asks").and_then(|a| a.as_array()) {
        for a in asks {
            if let Some((p, s)) = parse_lighter_price_size(a) {
                book.asks.push(BookLevel::new(p, s));
            }
        }
    }
}

/// Chain id for Lighter-protocol deployments, selected from the base URL:
/// mainnet `304`, testnet `300`, Robinhood Chain `466324`. A wrong chain id
/// signs a hash the sequencer rejects, so anything unrecognised stays on
/// mainnet rather than guessing a testnet.
pub fn lighter_chain_id_for_url(base_url: &str) -> u32 {
    let lower = base_url.to_lowercase();
    if lower.contains("testnet") {
        300
    } else if lower.contains("rh.lighter") {
        466324
    } else {
        304
    }
}

/// Market id plus fixed-point scales for order submission, resolved from one
/// `orderBookDetails` payload so the id can never pair with another market's
/// decimals. Unlike the read path (which defaults missing scales), orders
/// fail closed when a scale is absent: a guessed scale moves the price ~10x
/// per digit.
fn resolve_market_order_params(
    details: &serde_json::Value,
    symbol: &str,
) -> anyhow::Result<(u64, u32, u32)> {
    let market_id = resolve_market_id(details, symbol)?;
    let entries = details
        .get("order_book_details")
        .and_then(|d| d.as_array())
        .ok_or_else(|| anyhow::anyhow!("Missing order_book_details in Lighter response"))?;
    let entry = entries
        .iter()
        .find(|e| e.get("market_id").and_then(|m| m.as_u64()) == Some(market_id))
        .ok_or_else(|| anyhow::anyhow!("Lighter market {} vanished mid-order", market_id))?;
    let scale = |key: &str| -> anyhow::Result<u32> {
        let v = entry
            .get(key)
            .and_then(|x| x.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Lighter market {} lacks '{}'", market_id, key))?;
        if v > 18 {
            anyhow::bail!("Lighter market {} '{}' scale {} is absurd", market_id, key, v);
        }
        Ok(v as u32)
    };
    Ok((market_id, scale("price_decimals")?, scale("size_decimals")?))
}

/// Scale a human price to the L2 integer (`human x 10^price_decimals`),
/// rounding toward the trader's favour (buys down, sells up) so the
/// submitted price is never worse than quoted.
pub fn scale_lighter_price(
    human: Decimal,
    price_decimals: u32,
    side: Side,
    symbol: &str,
) -> anyhow::Result<u32> {
    if human <= dec!(0) {
        anyhow::bail!("Lighter price for '{}' must be positive", symbol);
    }
    if price_decimals > 18 {
        anyhow::bail!("Lighter price scale {} is absurd", price_decimals);
    }
    let factor = Decimal::from(10u64.pow(price_decimals));
    let scaled = human * factor;
    let rounded = match side {
        Side::Buy => scaled.floor(),
        Side::Sell => scaled.ceil(),
    };
    if rounded < dec!(1) || rounded > Decimal::from(u32::MAX) {
        anyhow::bail!("Lighter price {} for '{}' out of range", human, symbol);
    }
    Ok(rounded.to_string().parse::<u32>().unwrap_or(0))
}

/// Scale a human size to the L2 integer (`human x 10^size_decimals`),
/// always rounding down so the order never exceeds the requested size.
pub fn scale_lighter_amount(
    human: Decimal,
    size_decimals: u32,
    symbol: &str,
) -> anyhow::Result<i64> {
    if human <= dec!(0) {
        anyhow::bail!("Lighter quantity for '{}' must be positive", symbol);
    }
    if size_decimals > 18 {
        anyhow::bail!("Lighter size scale {} is absurd", size_decimals);
    }
    const MAX_BASE_AMOUNT: i64 = (1 << 48) - 1;
    let scaled = (human * Decimal::from(10u64.pow(size_decimals))).floor();
    if scaled < dec!(1) || scaled > Decimal::from(MAX_BASE_AMOUNT) {
        anyhow::bail!("Lighter quantity {} for '{}' out of range", human, symbol);
    }
    Ok(scaled.to_string().parse::<i64>().unwrap_or(0))
}

/// Deterministic L2 client order index (1..=2^48-1) from the hex/uuid client
/// order id. The same id always maps to the same index, which is also what
/// lets `cancel_order` key off the client order id.
pub fn lighter_client_order_index(client_order_id: &str) -> i64 {
    use sha2::{Digest, Sha256};
    const MASK_48: u64 = (1 << 48) - 1;
    let digest = Sha256::digest(client_order_id.trim().as_bytes());
    let mut buf = [0u8; 8];
    buf[2..8].copy_from_slice(&digest[..6]);
    let idx = (u64::from_be_bytes(buf) & MASK_48) as i64;
    if idx < 1 {
        1
    } else {
        idx
    }
}

/// Worst-acceptable limit price for a Lighter market (IOC) order, mirroring
/// the official SDK's limited-slippage path: best-touch +/- 2% band, plus a
/// full-size depth walk. Bails when the book cannot fill the whole quantity
/// or the walk already breaches the band — submitting anyway would accept an
/// unbounded price.
pub fn market_limit_price(
    book: &OrderBook,
    side: Side,
    quantity: Decimal,
    symbol: &str,
) -> anyhow::Result<Decimal> {
    let mut levels: Vec<(Decimal, Decimal)> = match side {
        // A buy lifts asks from the cheapest up.
        Side::Buy => book.asks.iter().map(|l| (l.price, l.size)).collect(),
        // A sell hits bids from the dearest down.
        Side::Sell => book.bids.iter().map(|l| (l.price, l.size)).collect(),
    };
    if levels.is_empty() {
        anyhow::bail!("No Lighter {} book to price a market order for '{}'", side_name(side), symbol);
    }
    match side {
        Side::Buy => levels.sort_by_key(|l| l.0),
        Side::Sell => levels.sort_by_key(|l| std::cmp::Reverse(l.0)),
    }
    let ideal = levels[0].0;
    if ideal <= dec!(0) {
        anyhow::bail!("Lighter top-of-book for '{}' is not positive", symbol);
    }
    let band = dec!(0.02);
    let acceptable = match side {
        Side::Buy => ideal * (dec!(1) + band),
        Side::Sell => ideal * (dec!(1) - band),
    };
    let mut filled = dec!(0);
    let mut notional = dec!(0);
    for (px, sz) in &levels {
        if filled >= quantity {
            break;
        }
        let take = (*sz).min(quantity - filled);
        filled += take;
        notional += take * *px;
    }
    if filled < quantity {
        anyhow::bail!(
            "Lighter book too thin for {} {} '{}' (only {} available)",
            quantity,
            side_name(side),
            symbol,
            filled
        );
    }
    let vwap = notional / filled;
    let breached = match side {
        Side::Buy => vwap > acceptable,
        Side::Sell => vwap < acceptable,
    };
    if breached {
        anyhow::bail!(
            "Lighter market order for '{}' breaches the 2% slippage band (vwap {})",
            symbol,
            vwap
        );
    }
    Ok(acceptable)
}

/// Pure validation of a `sendTx` HTTP response: HTTP success plus `code ==
/// 200` plus a `tx_hash` echoing the hash that was signed. Anything else is a
/// rejection — never treated as an order.
pub fn parse_send_tx_result(
    http_ok: bool,
    http_status: u16,
    body: &str,
    expected_tx_hash: &str,
) -> anyhow::Result<String> {
    let v: serde_json::Value =
        serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    let code = v.get("code").and_then(|c| c.as_i64());
    if http_ok && code == Some(200) {
        let tx_hash = v
            .get("tx_hash")
            .and_then(|h| h.as_str())
            .unwrap_or("")
            .to_string();
        if tx_hash.is_empty() {
            anyhow::bail!("Lighter sendTx accepted but returned no tx_hash");
        }
        if tx_hash != expected_tx_hash {
            anyhow::bail!("Lighter sendTx returned a different tx_hash than signed — refusing to track");
        }
        return Ok(tx_hash);
    }
    let msg = v
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or(body.chars().take(256).collect::<String>().as_str())
        .to_string();
    anyhow::bail!(
        "Lighter sendTx rejected (http {} code {:?}): {}",
        http_status,
        code,
        msg
    )
}

fn side_name(side: Side) -> &'static str {
    match side {
        Side::Buy => "buy",
        Side::Sell => "sell",
    }
}

fn resolve_market_id(details: &serde_json::Value, symbol: &str) -> anyhow::Result<u64> {
    let entries = details
        .get("order_book_details")
        .and_then(|d| d.as_array())
        .ok_or_else(|| anyhow::anyhow!("Missing order_book_details in Lighter response"))?;

    let clean = symbol.trim().replace('/', "-").to_uppercase();
    // Market type is a trailing `-SPOT` suffix only — a perp literally named
    // "SPOT" (e.g. "SPOT-USDC") must still resolve as a perp.
    let (market_type, lookup) = match clean.strip_suffix("-SPOT") {
        Some(base) => ("spot", base.to_string()),
        None => ("perp", clean.clone()),
    };
    let matches_type = |e: &serde_json::Value| {
        e.get("market_type")
            .and_then(|t| t.as_str())
            .map(|t| t.eq_ignore_ascii_case(market_type))
            .unwrap_or(false)
    };

    if lookup.is_empty() {
        anyhow::bail!("Lighter symbol '{}' carries no market identifier", symbol);
    }

    if let Ok(id) = lookup.parse::<u64>() {
        if entries
            .iter()
            .any(|e| e.get("market_id").and_then(|m| m.as_u64()) == Some(id) && matches_type(e))
        {
            return Ok(id);
        }
        anyhow::bail!(
            "No Lighter {} market with id {} for '{}'",
            market_type,
            id,
            symbol
        );
    }

    let parts: Vec<&str> = lookup.split('-').collect();
    if parts.len() >= 2 {
        let quote = parts[1];
        if !quote.eq_ignore_ascii_case("USDC") && !quote.eq_ignore_ascii_case("USD") {
            anyhow::bail!("Lighter markets are quoted in USDC/USD; '{symbol}' is not supported");
        }
    }
    let base = parts.first().copied().unwrap_or(lookup.as_str());
    let matched: Vec<u64> = entries
        .iter()
        .filter(|e| matches_type(e))
        .filter(|e| {
            e.get("symbol")
                .and_then(|s| s.as_str())
                .map(|s| s.eq_ignore_ascii_case(base))
                .unwrap_or(false)
        })
        .filter_map(|e| e.get("market_id").and_then(|m| m.as_u64()))
        .collect();
    match matched.as_slice() {
        [id] => Ok(*id),
        [] => anyhow::bail!("Unknown Lighter {} symbol '{}'", market_type, symbol),
        ids => anyhow::bail!(
            "Ambiguous Lighter {} symbol '{}' matches market ids {:?}",
            market_type,
            symbol,
            ids
        ),
    }
}

/// Pure parse of Lighter `/api/v1/funding-rates` rows into venue-owned
/// [`FundingRate`] prints. Split out for unit testing: the venue `rate` is
/// an 8-hour print, hence `interval_hours = 8`.
fn parse_lighter_funding_rates(
    rows: &[serde_json::Value],
    venue_id: &VenueId,
) -> Vec<FundingRate> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for row in rows {
        let exchange = row.get("exchange").and_then(|e| e.as_str()).unwrap_or("");
        if !exchange.eq_ignore_ascii_case("lighter") {
            continue;
        }
        let base = row.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
        let rate_f = row.get("rate").and_then(|r| r.as_f64());
        if base.is_empty() || !seen.insert(base.to_uppercase()) {
            continue;
        }
        if let Some(rate_f) = rate_f {
            let rate = Decimal::try_from(rate_f).unwrap_or(dec!(0));
            out.push(FundingRate {
                venue: venue_id.clone(),
                symbol: Symbol::perp(base, "USDC").canonical(),
                rate,
                predicted_rate: None,
                interval_hours: 8, // 8h-aggregated print (venue UI "8hr")
                next_funding_time: Utc::now(),
                timestamp: Utc::now(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn details(entries: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "order_book_details": entries })
    }

    #[test]
    fn resolve_market_id_matches_exact_perp_symbol() {
        let d = details(serde_json::json!([
            { "symbol": "BTC", "market_id": 1, "market_type": "perp" },
            { "symbol": "SOL", "market_id": 2, "market_type": "perp" }
        ]));
        assert_eq!(resolve_market_id(&d, "SOL").unwrap(), 2);
        assert_eq!(resolve_market_id(&d, "sol/usdc").unwrap(), 2);
        assert_eq!(resolve_market_id(&d, "SOL-USDC").unwrap(), 2);
    }

    #[test]
    fn resolve_market_id_spot_selects_only_spot_entry() {
        let d = details(serde_json::json!([
            { "symbol": "ETH", "market_id": 0, "market_type": "perp" },
            { "symbol": "ETH", "market_id": 2048, "market_type": "spot" }
        ]));
        assert_eq!(resolve_market_id(&d, "ETH-SPOT").unwrap(), 2048);
        assert_eq!(resolve_market_id(&d, "eth/spot").unwrap(), 2048);
        assert_eq!(resolve_market_id(&d, "ETH").unwrap(), 0);
    }

    #[test]
    fn resolve_market_id_unknown_symbol_errors_not_btc() {
        let d = details(serde_json::json!([
            { "symbol": "BTC", "market_id": 1, "market_type": "perp" }
        ]));
        let err = resolve_market_id(&d, "AVAX").expect_err("unknown symbol must error");
        assert!(err.to_string().contains("AVAX"), "unexpected err: {err}");
    }

    #[test]
    fn resolve_market_id_never_aliases_pepe_to_1000pepe() {
        let d = details(serde_json::json!([
            { "symbol": "1000PEPE", "market_id": 4, "market_type": "perp" }
        ]));
        assert!(resolve_market_id(&d, "PEPE").is_err());
        assert!(resolve_market_id(&d, "PEPE-USDC").is_err());
        assert_eq!(resolve_market_id(&d, "1000PEPE").unwrap(), 4);
    }

    #[test]
    fn resolve_market_id_numeric_requires_real_market_and_type() {
        let d = details(serde_json::json!([
            { "symbol": "BTC", "market_id": 1, "market_type": "perp" },
            { "symbol": "ETH", "market_id": 2048, "market_type": "spot" }
        ]));
        assert_eq!(resolve_market_id(&d, "1").unwrap(), 1);
        assert!(resolve_market_id(&d, "999").is_err());
        assert!(
            resolve_market_id(&d, "2048").is_err(),
            "a spot market id must not resolve as perp"
        );
        assert_eq!(resolve_market_id(&d, "2048-SPOT").unwrap(), 2048);
    }

    #[test]
    fn resolve_market_id_malformed_details_error() {
        assert!(resolve_market_id(&serde_json::json!({}), "SOL").is_err());
        assert!(
            resolve_market_id(&serde_json::json!({"order_book_details": {"x": 1}}), "SOL").is_err()
        );
        assert!(resolve_market_id(&serde_json::Value::Null, "SOL").is_err());
        let d = details(serde_json::json!([{ "symbol": "SOL" }]));
        assert!(
            resolve_market_id(&d, "SOL").is_err(),
            "entry without market_id cannot resolve"
        );
    }

    #[test]
    fn resolve_market_id_ambiguous_entries_error() {
        let d = details(serde_json::json!([
            { "symbol": "SOL", "market_id": 2, "market_type": "perp" },
            { "symbol": "sol", "market_id": 77, "market_type": "perp" }
        ]));
        assert!(resolve_market_id(&d, "SOL").is_err());
    }

    #[test]
    fn resolve_market_id_rejects_unsupported_quote() {
        let d = details(serde_json::json!([
            { "symbol": "BTC", "market_id": 1, "market_type": "perp" },
            { "symbol": "SOL", "market_id": 2, "market_type": "perp" }
        ]));
        assert_eq!(resolve_market_id(&d, "BTC-USDC").unwrap(), 1);
        assert!(resolve_market_id(&d, "BTC-SOL").is_err());
        assert!(resolve_market_id(&d, "BTC-ETH").is_err());
    }

    #[test]
    fn parse_lighter_funding_rates_uses_8h_interval() {
        // Captured 2026-09-20 shape: LINK's venue print (-0.0336%) tracks
        // the UI "8hr" figure, so hourly ~= rate / 8 ~= UI "1hr" (-0.0039%).
        let rows = vec![
            serde_json::json!({"exchange": "binance", "symbol": "LINK", "rate": 0.0001}),
            serde_json::json!({"exchange": "lighter", "symbol": "LINK", "rate": -0.000336}),
        ];
        let out = parse_lighter_funding_rates(&rows, &VenueId::new("lighter"));
        assert_eq!(out.len(), 1, "external rows are context, not our rate");
        let fr = &out[0];
        assert_eq!(fr.interval_hours, 8);
        let hourly = fr.rate / Decimal::from(fr.interval_hours);
        let hourly_f = hourly.to_string().parse::<f64>().unwrap();
        // ~= -0.0042%/h vs UI -0.0039%/h (prints drift between polls).
        assert!(
            (hourly_f + 0.000042).abs() < 0.00001,
            "hourly normalization off: {hourly_f}"
        );
    }

    #[test]
    fn chain_id_mapping() {
        assert_eq!(lighter_chain_id_for_url("https://mainnet.zklighter.elliot.ai"), 304);
        assert_eq!(lighter_chain_id_for_url("https://testnet.zklighter.elliot.ai"), 300);
        assert_eq!(lighter_chain_id_for_url("https://api.rh.lighter.xyz"), 466324);
        assert_eq!(lighter_chain_id_for_url("https://example.com"), 304);
    }

    #[test]
    fn scale_price_rounds_toward_trader() {
        // 4dp market: 12.34567 -> buy 123456, sell 123457.
        assert_eq!(
            scale_lighter_price(dec!(12.34567), 4, Side::Buy, "T").unwrap(),
            123456
        );
        assert_eq!(
            scale_lighter_price(dec!(12.34567), 4, Side::Sell, "T").unwrap(),
            123457
        );
        assert!(scale_lighter_price(dec!(0), 4, Side::Buy, "T").is_err());
        assert!(scale_lighter_price(dec!(-1), 4, Side::Buy, "T").is_err());
    }

    #[test]
    fn scale_amount_floors() {
        // 3dp market: 1.2349 -> 1234 (never rounds the size up).
        assert_eq!(scale_lighter_amount(dec!(1.2349), 3, "T").unwrap(), 1234);
        assert!(scale_lighter_amount(dec!(0.0001), 3, "T").is_err());
        assert!(scale_lighter_amount(dec!(0), 3, "T").is_err());
    }

    #[test]
    fn client_order_index_stable_and_bounded() {
        let a = lighter_client_order_index("d290f1ee-6c54-4b01-90e6-d701748f0851");
        let b = lighter_client_order_index("d290f1ee-6c54-4b01-90e6-d701748f0851");
        assert_eq!(a, b);
        assert!((1..=(1i64 << 48) - 1).contains(&a));
        let c = lighter_client_order_index("different-id");
        assert_ne!(a, c);
    }

    fn book_for(side_px: &[(f64, f64)], symbol: &str) -> OrderBook {
        let mut book = OrderBook::new(VenueId::new("lighter"), symbol.to_string());
        for (p, s) in side_px {
            book.asks.push(BookLevel::new(
                Decimal::from_str(&p.to_string()).unwrap(),
                Decimal::from_str(&s.to_string()).unwrap(),
            ));
        }
        book
    }

    #[test]
    fn market_limit_price_buy_within_band() {
        // Asks 100x10, 101x10. Buy 15 -> ideal 100, acceptable 102.
        let book = book_for(&[(100.0, 10.0), (101.0, 10.0)], "T");
        let px = market_limit_price(&book, Side::Buy, dec!(15), "T").unwrap();
        assert_eq!(px, dec!(102));
    }

    #[test]
    fn market_limit_price_thin_book_fails_closed() {
        let book = book_for(&[(100.0, 10.0)], "T");
        assert!(market_limit_price(&book, Side::Buy, dec!(11), "T").is_err());
    }

    #[test]
    fn market_limit_price_slippage_breach_fails_closed() {
        // Walk VWAP (100*1 + 200*10)/11 = 190.9 > 102 band -> reject.
        let book = book_for(&[(100.0, 1.0), (200.0, 10.0)], "T");
        assert!(market_limit_price(&book, Side::Buy, dec!(11), "T").is_err());
    }

    #[test]
    fn parse_send_tx_accepts_only_matching_hash() {
        let body = serde_json::json!({"code": 200, "tx_hash": "abc", "message": "ok"}).to_string();
        assert_eq!(
            parse_send_tx_result(true, 200, &body, "abc").unwrap(),
            "abc"
        );
        assert!(parse_send_tx_result(true, 200, &body, "other").is_err());
        let bad = serde_json::json!({"code": 400, "message": "bad nonce"}).to_string();
        let err = parse_send_tx_result(true, 200, &bad, "abc").expect_err("code != 200");
        assert!(err.to_string().contains("bad nonce"));
        assert!(parse_send_tx_result(false, 400, &bad, "abc").is_err());
    }

    #[test]
    fn resolve_order_params_pairs_id_with_decimals() {
        let details = serde_json::json!({"order_book_details": [
            {"symbol": "BTC", "market_id": 1, "market_type": "perp", "price_decimals": 2, "size_decimals": 5},
            {"symbol": "ETH", "market_id": 2, "market_type": "perp", "price_decimals": 3, "size_decimals": 4}
        ]});
        assert_eq!(
            resolve_market_order_params(&details, "ETH-USDC").unwrap(),
            (2, 3, 4)
        );
        let missing = serde_json::json!({"order_book_details": [
            {"symbol": "BTC", "market_id": 1, "market_type": "perp"}
        ]});
        assert!(resolve_market_order_params(&missing, "BTC-USDC").is_err());
    }

    #[tokio::test]
    async fn fetch_account_requires_account_index() {
        let adapter = LighterAdapter::new(Some("http://127.0.0.1:1".to_string()), None, None);
        let err = adapter
            .fetch_account()
            .await
            .expect_err("must fail closed without identity");
        assert_eq!(
            err.to_string(),
            "Lighter account fetch requires an account index"
        );
    }
}
