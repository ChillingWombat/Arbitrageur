use std::collections::HashMap;
use std::str::FromStr;
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use arbitrager_core::exchange::ExchangeAdapter;
use arbitrager_core::market::{MarketMetadata, Symbol};
use arbitrager_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrager_core::position::{AccountSnapshot, Balance, Position};
use arbitrager_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrager_core::types::{dec, Decimal, Side, VenueId};
use crate::rate_limiter::AccountRequestCoordinator;

/// Lighter zk-Rollup DEX adapter.
#[allow(dead_code)]
pub struct LighterAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    account_index: Option<u64>,
    api_key_private_key: Option<String>,
    rate_limiter: AccountRequestCoordinator,
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
            client: Client::builder().build().unwrap_or_default(),
            base_url: base,
            account_index,
            api_key_private_key,
            rate_limiter: AccountRequestCoordinator::new(venue_id, 200, 3000, 500, 15000),
        }
    }

    async fn get_json(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        // 1. Check rate limiter and in-memory single-flight cache
        if let Ok(Some(cached_str)) = self.rate_limiter.acquire_or_cache_path(path, true).await {
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
        let coin = symbol
            .trim()
            .replace('/', "-")
            .split('-')
            .next()
            .unwrap_or("")
            .to_uppercase();
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
    /// this venue's rate. Rates are hourly fractions.
    async fn fetch_funding_rates(&self) -> anyhow::Result<Vec<FundingRate>> {
        let res = self.get_json("/api/v1/funding-rates").await?;
        let rows = res
            .get("funding_rates")
            .and_then(|r| r.as_array())
            .ok_or_else(|| anyhow::anyhow!("Missing funding_rates in Lighter response"))?;
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
                    venue: self.venue_id.clone(),
                    symbol: Symbol::perp(base, "USDC").canonical(),
                    rate,
                    predicted_rate: None,
                    interval_hours: 1, // Lighter funding accrues hourly
                    next_funding_time: Utc::now(),
                    timestamp: Utc::now(),
                });
            }
        }
        Ok(out)
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

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        anyhow::bail!("Live order execution is not implemented for {}; refusing to fabricate a fill", self.name())
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        anyhow::bail!("Live order cancellation is not implemented for {}", self.name())
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

    let base = lookup.split('-').next().unwrap_or(lookup.as_str());
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
