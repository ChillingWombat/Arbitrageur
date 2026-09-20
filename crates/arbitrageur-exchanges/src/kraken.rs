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

    fn credentials(&self) -> anyhow::Result<(Zeroizing<String>, Zeroizing<String>)> {
        let api_key = self
            .api_key
            .read()
            .clone()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Kraken API key not configured or empty"))?;
        let api_secret = self
            .api_secret
            .read()
            .clone()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Kraken API secret not configured or empty"))?;
        Ok((api_key, api_secret))
    }

    fn has_credentials(&self) -> bool {
        self.credentials().is_ok()
    }

    /// Kraken Futures private-API signature.
    ///
    /// `Authent = base64(HMAC-SHA512(endpoint_path + SHA256(nonce + post_data),
    /// base64_decode(api_secret)))` where `endpoint_path` is the `/api/v3/...`
    /// form (the `/derivatives` URL prefix is NOT signed) and `post_data` is
    /// the raw url-encoded body the server will receive.
    fn sign_futures(
        secret: &Zeroizing<String>,
        endpoint_path: &str,
        nonce: &str,
        post_data: &str,
    ) -> anyhow::Result<String> {
        use base64::Engine;
        use hmac::{Hmac, Mac};
        use sha2::{Digest, Sha256, Sha512};
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(secret.trim().as_bytes())
            .map_err(|e| anyhow::anyhow!("Invalid Kraken base64 API secret: {e}"))?;
        let mut sha = Sha256::new();
        sha.update(format!("{nonce}{post_data}").as_bytes());
        let sha_digest = sha.finalize();
        let mut message = Vec::with_capacity(endpoint_path.len() + sha_digest.len());
        message.extend_from_slice(endpoint_path.as_bytes());
        message.extend_from_slice(&sha_digest);
        let mut mac = Hmac::<Sha512>::new_from_slice(&decoded)
            .map_err(|e| anyhow::anyhow!("Invalid Kraken HMAC key: {e}"))?;
        mac.update(&message);
        Ok(base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
    }

    /// Authenticated POST to a Kraken Futures private endpoint.
    /// `path` is the full `/derivatives/api/v3/...` URL path; only the
    /// `/api/v3/...` suffix is signed.
    async fn authed_post(
        &self,
        path: &str,
        params: &std::collections::BTreeMap<&str, String>,
    ) -> anyhow::Result<serde_json::Value> {
        use std::fmt::Write;
        let (api_key, api_secret) = self.credentials()?;
        let nonce = Utc::now().timestamp_millis().to_string();
        let mut post_data = String::from("nonce=");
        post_data.push_str(&nonce);
        for (k, v) in params {
            let _ = write!(post_data, "&{k}={v}");
        }
        let endpoint = path
            .strip_prefix("/derivatives")
            .unwrap_or(path);
        let authent = Self::sign_futures(&api_secret, endpoint, &nonce, &post_data)?;
        let url = format!("{}{}", self.futures_base_url, path);
        let resp = self
            .client
            .post(&url)
            .header("APIKey", api_key.to_string())
            .header("Nonce", nonce)
            .header("Authent", authent)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(post_data)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Kraken POST {path} transport failure: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("Kraken POST {path} failed (HTTP {status}): {text}");
        }
        let v: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("Failed to parse Kraken {path} response: {e}; body: {text}"))?;
        if v.get("result").and_then(|r| r.as_str()) != Some("success") {
            anyhow::bail!("Kraken POST {path} error response: {text}");
        }
        Ok(v)
    }

    /// Authenticated GET to a Kraken Futures private endpoint (nonce travels
    /// as a query param and inside the signed payload).
    async fn authed_get(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        let (api_key, api_secret) = self.credentials()?;
        let nonce = Utc::now().timestamp_millis().to_string();
        let post_data = format!("nonce={nonce}");
        let endpoint = path
            .strip_prefix("/derivatives")
            .unwrap_or(path);
        let authent = Self::sign_futures(&api_secret, endpoint, &nonce, &post_data)?;
        let url = format!("{}{}?nonce={}", self.futures_base_url, path, nonce);
        let resp = self
            .client
            .get(&url)
            .header("APIKey", api_key.to_string())
            .header("Nonce", nonce)
            .header("Authent", authent)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Kraken GET {path} transport failure: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("Kraken GET {path} failed (HTTP {status}): {text}");
        }
        let v: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("Failed to parse Kraken {path} response: {e}; body: {text}"))?;
        if v.get("result").and_then(|r| r.as_str()) != Some("success") {
            anyhow::bail!("Kraken GET {path} error response: {text}");
        }
        Ok(v)
    }

    /// Contracts per engine base unit for a canonical symbol (from the public
    /// instrument list; cached upstream by the rate limiter).
    async fn contract_size_for(&self, symbol: &str) -> anyhow::Result<Decimal> {
        let venue_sym = Self::to_futures_symbol(symbol)?;
        let val = self.get_futures_json("/instruments").await?;
        let instruments = val
            .get("instruments")
            .and_then(|i| i.as_array())
            .ok_or_else(|| anyhow::anyhow!("Kraken instruments missing array"))?;
        for item in instruments {
            if item.get("symbol").and_then(|s| s.as_str()) == Some(venue_sym.as_str()) {
                let size = dec_from_json(item.get("contractSize")).unwrap_or(dec!(1));
                if size > dec!(0) {
                    return Ok(size);
                }
            }
        }
        anyhow::bail!("Kraken contract size unknown for {symbol} ({venue_sym})")
    }

    /// Pure parse of `/accounts` + `/openpositions` envelopes into a snapshot.
    /// `marks_usd` maps collateral codes (XBT, ETH, ...) to USD marks for the
    /// equity roll-up; unknown codes count 0 toward equity (conservative lower
    /// bound) but are still listed raw in `balances` — never fabricated.
    fn parse_account_response(
        &self,
        accounts_v: &serde_json::Value,
        positions_v: &serde_json::Value,
        marks_usd: &HashMap<String, Decimal>,
    ) -> anyhow::Result<AccountSnapshot> {
        let mut balances = Vec::new();
        let mut equity = dec!(0);
        let mut free_margin = dec!(0);
        let mut maint_margin = dec!(0);
        let mut saw_avail = false;
        if let Some(accounts) = accounts_v.get("accounts").and_then(|a| a.as_object()) {
            for (_name, acct) in accounts {
                if let Some(bals) = acct.get("balances").and_then(|b| b.as_object()) {
                    for (ccy, amt_v) in bals {
                        let amt = dec_from_json(Some(amt_v)).unwrap_or_default();
                        if amt.is_zero() {
                            continue;
                        }
                        let code = ccy.to_uppercase();
                        let mark = if ["USD", "USDT", "USDC", "DAI", "ZUSD"].contains(&code.as_str()) {
                            dec!(1)
                        } else {
                            marks_usd.get(&code).copied().unwrap_or(dec!(0))
                        };
                        equity += amt * mark;
                        if let Some(entry) = balances.iter_mut().find(|b: &&mut arbitrageur_core::position::Balance| b.currency == code) {
                            entry.total += amt;
                            entry.free += amt;
                        } else {
                            balances.push(arbitrageur_core::position::Balance {
                                venue: self.venue_id.clone(),
                                currency: code,
                                total: amt,
                                free: amt,
                                locked: dec!(0),
                            });
                        }
                    }
                }
                if let Some(mr) = acct.get("marginRequirements") {
                    maint_margin += dec_from_json(mr.get("maintenanceMargin")).unwrap_or_default();
                    let avail = dec_from_json(mr.get("availableMargin")).unwrap_or_default();
                    if avail > dec!(0) {
                        saw_avail = true;
                    }
                    free_margin += avail;
                }
            }
        }
        if !saw_avail {
            // Venue did not quote available margin: fall back to USD-par cash
            // (conservative — crypto collateral stays out of "free").
            free_margin = balances
                .iter()
                .filter(|b| ["USD", "USDT", "USDC", "DAI", "ZUSD"].contains(&b.currency.as_str()))
                .map(|b| b.free)
                .fold(dec!(0), |a, x| a + x);
        }

        let mut positions = Vec::new();
        if let Some(arr) = positions_v.get("openPositions").and_then(|p| p.as_array()) {
            for leg in arr {
                let sym = leg.get("symbol").and_then(|s| s.as_str()).unwrap_or_default();
                if sym.is_empty() {
                    continue;
                }
                // Size is in contracts; scale to base units via the venue
                // symbol's contract size carried in `marks_usd` under
                // `CT:<venue-symbol>` (populated by the caller).
                let ct_key = format!("CT:{sym}");
                let ct = marks_usd.get(&ct_key).copied().unwrap_or(dec!(1));
                let size_ct = dec_from_json(leg.get("size")).unwrap_or_default();
                if size_ct.is_zero() {
                    continue;
                }
                let qty = (size_ct * ct).abs();
                let entry_px = dec_from_json(leg.get("entryPrice"))
                    .or_else(|| dec_from_json(leg.get("price")))
                    .unwrap_or_default();
                let mark_px = dec_from_json(leg.get("markPrice")).unwrap_or(entry_px);
                let upnl = dec_from_json(leg.get("unrealizedPnl"))
                    .or_else(|| dec_from_json(leg.get("unrealisedPnl")))
                    .unwrap_or_default();
                let side = match leg.get("side").and_then(|s| s.as_str()).unwrap_or("") {
                    "short" => arbitrageur_core::types::Side::Sell,
                    _ => arbitrageur_core::types::Side::Buy,
                };
                let notional = qty * mark_px;
                equity += upnl;
                positions.push(arbitrageur_core::position::Position {
                    venue: self.venue_id.clone(),
                    symbol: sym.to_string(),
                    side,
                    quantity: qty,
                    entry_price: entry_px,
                    mark_price: mark_px,
                    unrealized_pnl: upnl,
                    realized_pnl: dec!(0),
                    liquidation_price: None,
                    leverage: dec!(1),
                    initial_margin: dec!(0),
                    maintenance_margin: dec!(0),
                    updated_at: Utc::now(),
                });
                let _ = notional;
            }
        }

        let margin_usage_pct = if equity > dec!(0) {
            // Initial margin is locked inside positions here; usage derives
            // from maintenance cover instead of a borrowed figure.
            ((maint_margin / equity) * dec!(100)).min(dec!(100))
        } else {
            dec!(0)
        };
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

    /// Canonical linear-perp symbol for a Kraken Futures venue symbol
    /// (`PF_XBTUSD` -> `BTC-USD`). Inverse (`PI_*`) prints are excluded:
    /// this adapter quotes, sizes, and settles the linear book, so mixing
    /// inverse funding into the hub would corrupt carry math.
    fn canonical_linear_symbol(venue_sym: &str) -> Option<String> {
        let core = venue_sym.strip_prefix("PF_")?;
        let base = core
            .strip_suffix("USD")
            .or_else(|| core.strip_suffix("USDT"))
            .or_else(|| core.strip_suffix("USDC"))?;
        if base.is_empty() {
            return None;
        }
        let canonical_base = if base == "XBT" { "BTC" } else { base };
        Some(format!("{canonical_base}-USD"))
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

    async fn fetch_market_prices(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        let val = self.get_futures_json("/tickers").await?;
        let tickers = val
            .get("tickers")
            .and_then(|t| t.as_array())
            .ok_or_else(|| anyhow::anyhow!("Kraken tickers missing array"))?;

        let mut out = HashMap::new();
        for ticker in tickers {
            let Some(symbol) = ticker.get("symbol").and_then(|s| s.as_str()) else {
                continue;
            };
            let Some(price) = dec_from_json(ticker.get("markPrice"))
                .or_else(|| dec_from_json(ticker.get("last")))
                .filter(|p| *p > dec!(0))
            else {
                continue;
            };
            out.insert(symbol.to_string(), price);
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
                // Hub keys are canonical symbols; raw venue symbols never
                // match a scan. Inverse legs are excluded (see above).
                let Some(canonical) = Self::canonical_linear_symbol(sym) else {
                    continue;
                };
                let pred = dec_from_json(item.get("fundingRatePrediction"));
                out.push(FundingRate {
                    venue: self.venue_id.clone(),
                    symbol: canonical,
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
        if !self.has_credentials() {
            anyhow::bail!("Cannot fetch live account on Kraken: API credentials are not configured or vault is locked");
        }
        let accounts_v = self.authed_get("/derivatives/api/v3/accounts").await?;
        let positions_v = self.authed_get("/derivatives/api/v3/openpositions").await?;
        // USD marks for major collateral + contract sizes for open legs.
        // Best-effort: a mark failure only degrades the equity roll-up to a
        // conservative lower bound (raw balances are still exact).
        let mut marks: HashMap<String, Decimal> = HashMap::new();
        if let Ok(tickers) = self.get_futures_json("/tickers").await {
            if let Some(arr) = tickers.get("tickers").and_then(|t| t.as_array()) {
                for t in arr {
                    let vsym = t.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
                    let last = dec_from_json(t.get("last")).unwrap_or_default();
                    if vsym.is_empty() || last <= dec!(0) {
                        continue;
                    }
                    // PF_XBTUSD -> XBT, PF_ETHUSD -> ETH, ...
                    if let Some(core) = vsym.strip_prefix("PF_").and_then(|s| s.strip_suffix("USD")) {
                        marks.insert(core.to_string(), last);
                    }
                    if let Some(ct) = t.get("contractSize").and_then(|c| c.as_str()) {
                        // tickers carry no contractSize; sizes resolve below.
                        let _ = ct;
                    }
                }
            }
        }
        if let Ok(instruments) = self.get_futures_json("/instruments").await {
            if let Some(arr) = instruments.get("instruments").and_then(|i| i.as_array()) {
                for item in arr {
                    if let (Some(sym), Some(ct)) = (
                        item.get("symbol").and_then(|s| s.as_str()),
                        dec_from_json(item.get("contractSize")),
                    ) {
                        if ct > dec!(0) {
                            marks.insert(format!("CT:{sym}"), ct);
                        }
                    }
                }
            }
        }
        self.parse_account_response(&accounts_v, &positions_v, &marks)
    }

    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        use arbitrageur_core::types::OrderStatus;
        if !self.has_credentials() {
            anyhow::bail!("Cannot place live order on Kraken: API credentials are not configured or vault is locked");
        }
        if order.quantity <= dec!(0) {
            anyhow::bail!("Kraken order quantity must be positive");
        }
        let venue_symbol = Self::to_futures_symbol(&order.symbol)?;
        let ct = self.contract_size_for(&order.symbol).await?;
        let contracts = (order.quantity / ct).trunc();
        if contracts <= dec!(0) {
            anyhow::bail!(
                "Kraken order {qty} {sym} is below one contract ({ct} base units)",
                qty = order.quantity,
                sym = order.symbol,
                ct = ct
            );
        }
        let side_str = match order.side {
            arbitrageur_core::types::Side::Buy => "buy",
            arbitrageur_core::types::Side::Sell => "sell",
        };
        // Engine PostOnly/IOC map onto Kraken order types; plain Limit rests.
        let order_type = if order.order_type == arbitrageur_core::types::OrderType::Market {
            "mkt"
        } else if order.post_only
            || order.time_in_force == arbitrageur_core::types::TimeInForce::PostOnly
        {
            "post"
        } else if order.time_in_force == arbitrageur_core::types::TimeInForce::ImmediateOrCancel {
            "ioc"
        } else {
            "lmt"
        };
        let mut params = std::collections::BTreeMap::new();
        params.insert("orderType", order_type.to_string());
        params.insert("symbol", venue_symbol.clone());
        params.insert("side", side_str.to_string());
        params.insert("size", contracts.normalize().to_string());
        if order_type != "mkt" {
            let px = order.price.filter(|x| *x > dec!(0)).ok_or_else(|| {
                anyhow::anyhow!("Kraken {order_type} order requires a positive limit price")
            })?;
            params.insert("limitPrice", px.normalize().to_string());
        }
        if order.reduce_only {
            params.insert("reduceOnly", "true".to_string());
        }
        if !order.client_order_id.is_empty() {
            params.insert("cliOrdId", order.client_order_id.clone());
        }
        let v = self.authed_post("/derivatives/api/v3/sendorder", &params).await?;
        let st = v
            .get("sendStatus")
            .ok_or_else(|| anyhow::anyhow!("Kraken sendorder missing sendStatus: {v}"))?;
        let order_id = st
            .get("order_id")
            .and_then(|o| o.as_str())
            .unwrap_or("")
            .to_string();
        if order_id.is_empty() {
            anyhow::bail!("Kraken sendorder returned no order_id: {v}");
        }
        let status = match st.get("status").and_then(|s| s.as_str()).unwrap_or("placed") {
            "filled" => OrderStatus::Filled,
            "cancelled" | "rejected" | "expired" => OrderStatus::Rejected,
            "partiallyFilled" => OrderStatus::PartiallyFilled,
            _ => OrderStatus::New,
        };
        Ok(OrderResponse {
            order_id,
            client_order_id: order.client_order_id.clone(),
            venue: VenueId::new("kraken"),
            symbol: order.symbol.clone(),
            status,
            original_qty: order.quantity,
            filled_qty: dec!(0),
            remaining_qty: order.quantity,
            avg_fill_price: None,
            fee: dec!(0),
            fee_currency: "USD".to_string(),
            raw_response: Some(v.to_string()),
            timestamp: Utc::now(),
        })
    }

    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
        if !self.has_credentials() {
            anyhow::bail!("Cannot cancel live order on Kraken: API credentials are not configured or vault is locked");
        }
        let mut params = std::collections::BTreeMap::new();
        if let Some(id) = req.order_id {
            params.insert("order_id", id);
        } else if let Some(cid) = req.client_order_id {
            params.insert("cliOrdId", cid);
        } else {
            anyhow::bail!("Kraken cancel requires an order_id or cliOrdId");
        }
        // Symbol scoping is optional venue-side; include when resolvable.
        if let Ok(vsym) = Self::to_futures_symbol(&req.symbol) {
            params.insert("symbol", vsym);
        }
        self.authed_post("/derivatives/api/v3/cancelorder", &params).await?;
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
    fn parse_account_response_rolls_up_balances_and_positions() {
        let adapter = KrakenAdapter::new(None, None, None, None);
        let accounts_v: serde_json::Value = serde_json::json!({
            "result": "success",
            "accounts": {
                "cash": {
                    "balances": {"usd": 5000.0, "usdt": 1000.0, "xbt": 0.5},
                    "marginRequirements": {"initialMargin": "200.0", "maintenanceMargin": "100.0", "availableMargin": "4500.0"}
                },
                "flex": {"balances": {}, "marginRequirements": {}}
            }
        });
        let positions_v: serde_json::Value = serde_json::json!({
            "result": "success",
            "openPositions": [
                {"symbol": "PF_XBTUSD", "side": "long", "size": 1000, "entryPrice": "60000",
                 "markPrice": "61000", "unrealizedPnl": "50.0"}
            ]
        });
        let mut marks = HashMap::new();
        marks.insert("XBT".to_string(), dec!(60000));
        marks.insert("CT:PF_XBTUSD".to_string(), dec!(0.001));
        let snap = adapter
            .parse_account_response(&accounts_v, &positions_v, &marks)
            .unwrap();
        // equity = 5000 + 1000 + 0.5*60000 + 50 uPnL = 36050
        assert_eq!(snap.equity, dec!(36050));
        assert_eq!(snap.free_margin, dec!(4500));
        assert_eq!(snap.maintenance_margin, dec!(100));
        assert_eq!(snap.positions.len(), 1);
        assert_eq!(snap.positions[0].quantity, dec!(1));
        assert_eq!(snap.balances.len(), 3);
    }

    #[test]
    fn sign_futures_matches_independent_hmac_vector() {
        // Cross-checked with Python hmac/hashlib/base64 over the same
        // endpoint + nonce + body (see commit message for the script).
        let secret = Zeroizing::new(
            "c2VjcmV0LWtleS1mb3ItdGVzdGluZy0xMjM0NTY3ODkwMTIzNDU2Nzg=".to_string(),
        );
        let authent = KrakenAdapter::sign_futures(
            &secret,
            "/api/v3/sendorder",
            "1700000000000",
            "nonce=1700000000000&orderType=lmt&symbol=PF_XBTUSD&side=buy&size=10&limitPrice=60000",
        )
        .unwrap();
        assert_eq!(authent, "I7U4L8ldFZE+jjZv4JDtMFNJbYnSXSxqclINxdYzMcfULHT2g89T7SLoa2r72JiP+bf8qC4Wpo9sE/GBu9fkPQ==");
    }

    #[test]
    fn canonical_linear_symbol_maps_pf_and_skips_inverse() {
        assert_eq!(
            KrakenAdapter::canonical_linear_symbol("PF_XBTUSD"),
            Some("BTC-USD".to_string())
        );
        assert_eq!(
            KrakenAdapter::canonical_linear_symbol("PF_ETHUSD"),
            Some("ETH-USD".to_string())
        );
        assert_eq!(KrakenAdapter::canonical_linear_symbol("PI_XBTUSD"), None);
        assert_eq!(KrakenAdapter::canonical_linear_symbol("BTC-USD"), None);
    }

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
