use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use std::collections::HashMap;
use arbitrager_core::exchange::ExchangeAdapter;
use arbitrager_core::market::{MarketMetadata, Symbol};
use arbitrager_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrager_core::position::AccountSnapshot;
use arbitrager_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrager_core::types::{dec, Decimal, VenueId};
use crate::rate_limiter::AccountRequestCoordinator;

/// Aster (asterdex.com, Binance-style USDT-margined futures) connector.
///
/// Public read-only REST only — `GET {base}/fapi/v3/*`, no credentials.
/// Verified live 2026-09-15: `/fapi/v3/ping` -> `{}`, `/fapi/v3/depth`,
/// `/fapi/v3/exchangeInfo`, `/fapi/v3/premiumIndex`, `/fapi/v3/ticker/24hr`.
/// WebSocket (`wss://fstream.asterdex.com`) is recorded for future work;
/// this adapter is REST-only for v1 and leaves `subscribe_orderbook`
/// at the fail-closed default.
#[allow(dead_code)]
pub struct AsterAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    api_key: Option<String>,
    api_secret: Option<String>,
    rate_limiter: AccountRequestCoordinator,
}

/// Valid `depth` limits per the futures docs (weight 2-20 by limit).
const VALID_DEPTH_LIMITS: [u32; 7] = [5, 10, 20, 50, 100, 500, 1000];
/// Orderbook depth requested per snapshot.
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

/// Quote preference when a bare UI base (e.g. `BTC`) maps to several live
/// Aster listings (USDT-margined, USD1-margined, U-stablecoin). Order is a
/// liquidity preference, never an assumption: only quotes actually present
/// in `exchangeInfo` are candidates.
fn preferred_quote<'a>(quotes: &[&'a str]) -> Option<&'a str> {
    for want in ["USDT", "USD1", "U"] {
        if quotes.contains(&want) {
            return Some(want);
        }
    }
    quotes.first().copied()
}

/// Resolve a bare base to its venue symbol using live `exchangeInfo`
/// entries as `(symbol, base_asset, quote_asset)` triples. Never hardcodes
/// a quote suffix: the quote always comes from the venue's own listing.
fn resolve_bare_base<'a>(
    base: &str,
    entries: &'a [(String, String, String)],
) -> Option<&'a str> {
    let mut quotes: Vec<&str> = entries
        .iter()
        .filter(|(_, b, _)| b.eq_ignore_ascii_case(base))
        .map(|(_, _, q)| q.as_str())
        .collect();
    quotes.sort();
    quotes.dedup();
    let quote = preferred_quote(&quotes)?;
    entries
        .iter()
        .find(|(_, b, q)| b.eq_ignore_ascii_case(base) && q.eq_ignore_ascii_case(quote))
        .map(|(s, _, _)| s.as_str())
}

/// Parse one `exchangeInfo` symbol entry. The base/quote pair always comes
/// from the entry's own `baseAsset`/`quoteAsset` fields — concatenated
/// venue spellings (BTCUSDT, BTCUSD1, BTCU) are never split by suffix
/// guessing, because Aster lists non-USDT quotes with no USDC perps.
fn parse_exchange_info_entry(venue: &VenueId, s: &serde_json::Value) -> Option<MarketMetadata> {
    if s.get("status").and_then(|v| v.as_str()) != Some("TRADING") {
        return None;
    }
    if s.get("contractType").and_then(|v| v.as_str()) != Some("PERPETUAL") {
        return None;
    }
    let venue_symbol = s.get("symbol").and_then(|v| v.as_str())?;
    let base = s.get("baseAsset").and_then(|v| v.as_str())?;
    let quote = s.get("quoteAsset").and_then(|v| v.as_str())?;
    if base.is_empty() || quote.is_empty() {
        return None;
    }
    // Sanity: the concatenated venue symbol must be base+quote; anything
    // else (renamed listings, inverse contracts) fails closed per market.
    if !venue_symbol.eq_ignore_ascii_case(&format!("{base}{quote}")) {
        return None;
    }
    let price_decimals = s
        .get("pricePrecision")
        .and_then(|v| v.as_u64())
        .unwrap_or(2) as u32;
    let size_decimals = s
        .get("quantityPrecision")
        .and_then(|v| v.as_u64())
        .unwrap_or(3) as u32;
    let mut min_order_size: Option<Decimal> = None;
    let mut min_notional: Option<Decimal> = None;
    if let Some(filters) = s.get("filters").and_then(|f| f.as_array()) {
        for f in filters {
            match f.get("filterType").and_then(|t| t.as_str()) {
                Some("LOT_SIZE") => {
                    if let Some(m) = dec_from_json(f.get("minQty")) {
                        if m > dec!(0) {
                            min_order_size = Some(m);
                        }
                    }
                }
                Some("MIN_NOTIONAL") => {
                    if let Some(m) = dec_from_json(f.get("notional")) {
                        if m >= dec!(0) {
                            min_notional = Some(m);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Some(MarketMetadata {
        symbol: Symbol::perp(base, quote),
        venue: venue.clone(),
        venue_symbol: venue_symbol.to_string(),
        price_decimals,
        size_decimals,
        min_order_size: min_order_size.unwrap_or(dec!(0.001)),
        min_notional: min_notional.unwrap_or(dec!(5.0)),
        maker_fee_bps: dec!(2.0),
        taker_fee_bps: dec!(3.5),
        contract_size: dec!(1.0),
        is_active: true,
    })
}

fn parse_depth(
    venue: &VenueId,
    symbol: &str,
    _venue_symbol: &str,
    data: &serde_json::Value,
) -> anyhow::Result<OrderBook> {
    let mut book = OrderBook::new(venue.clone(), symbol.to_string());
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
    if book.bids.is_empty() && book.asks.is_empty() {
        anyhow::bail!("No live orderbook levels returned from Aster depth for {symbol}");
    }
    book.timestamp = Utc::now();
    Ok(book)
}

fn parse_premium_index(
    venue: &VenueId,
    symbol: &str,
    venue_symbol: &str,
    data: &serde_json::Value,
) -> anyhow::Result<FundingRate> {
    let returned = data.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
    if !returned.eq_ignore_ascii_case(venue_symbol) {
        anyhow::bail!(
            "Aster premiumIndex returned symbol {returned} for requested {venue_symbol}"
        );
    }
    let rate = dec_from_json(data.get("lastFundingRate")).ok_or_else(|| {
        anyhow::anyhow!("Aster premiumIndex for {venue_symbol} has no parseable lastFundingRate")
    })?;
    let next_ms = data
        .get("nextFundingTime")
        .and_then(|v| match v {
            serde_json::Value::Number(n) => n.as_i64(),
            serde_json::Value::String(s) => s.parse::<i64>().ok(),
            _ => None,
        })
        .ok_or_else(|| {
            anyhow::anyhow!("Aster premiumIndex for {venue_symbol} has no valid nextFundingTime")
        })?;
    let next_funding_time = DateTime::from_timestamp_millis(next_ms).ok_or_else(|| {
        anyhow::anyhow!("Aster premiumIndex for {venue_symbol} has out-of-range nextFundingTime")
    })?;
    Ok(FundingRate {
        venue: venue.clone(),
        symbol: symbol.to_string(),
        rate,
        predicted_rate: None,
        interval_hours: 8,
        next_funding_time,
        timestamp: Utc::now(),
    })
}

impl AsterAdapter {
    pub fn new(
        base_url: Option<String>,
        api_key: Option<String>,
        api_secret: Option<String>,
    ) -> Self {
        Self::new_with_venue(base_url, api_key, api_secret, "aster")
    }

    /// Venue-parameterized constructor following the Lighter/Vanta pattern;
    /// Aster always uses the `aster` venue id in production.
    pub fn new_with_venue(
        base_url: Option<String>,
        api_key: Option<String>,
        api_secret: Option<String>,
        venue_id: &str,
    ) -> Self {
        let base = base_url.unwrap_or_else(|| "https://fapi.asterdex.com".to_string());
        Self {
            venue_id: VenueId::new(venue_id),
            client: Client::builder().build().unwrap_or_default(),
            base_url: base,
            api_key,
            api_secret,
            rate_limiter: AccountRequestCoordinator::new(venue_id, 100, 2000, 500, 10000),
        }
    }

    /// Map a UI/canonical symbol to the concatenated Aster venue spelling.
    /// Qualified inputs (`BTC-USDT`, `BTC/USDT`, `BTCUSDT`) map
    /// deterministically; anything else fails closed — callers with a bare
    /// base must go through [`Self::resolve_venue_symbol`], which reads the
    /// live quote from `exchangeInfo` instead of assuming USDT.
    fn to_venue_symbol(symbol: &str) -> anyhow::Result<String> {
        let trimmed = symbol.trim();
        if trimmed.is_empty() {
            anyhow::bail!("Aster symbol is empty");
        }
        if trimmed.contains(['-', '/', '_', ' ']) {
            let tokens: Vec<&str> = trimmed
                .split(['-', '/', '_', ' '])
                .filter(|t| !t.is_empty())
                .collect();
            if tokens.len() != 2 {
                anyhow::bail!(
                    "Aster symbol '{symbol}' must be BASE or BASE-QUOTE; refusing to guess"
                );
            }
            let (base, quote) = (tokens[0], tokens[1]);
            if base.is_empty() || quote.is_empty() {
                anyhow::bail!("Aster symbol '{symbol}' has an empty base or quote");
            }
            if !base.chars().all(|c| c.is_ascii_alphanumeric())
                || !quote.chars().all(|c| c.is_ascii_alphanumeric())
            {
                anyhow::bail!("Aster symbol '{symbol}' must be ASCII alphanumeric");
            }
            return Ok(format!(
                "{}{}",
                base.to_uppercase(),
                quote.to_uppercase()
            ));
        }
        let upper = trimmed.to_uppercase();
        if !upper.chars().all(|c| c.is_ascii_alphanumeric()) {
            anyhow::bail!("Aster symbol '{symbol}' must be ASCII alphanumeric");
        }
        // Concatenated venue spellings pass through; a bare base such as
        // BTC is intentionally NOT expanded here (quote would be a guess).
        Ok(upper)
    }

    /// Resolve any UI symbol to the venue spelling, reading the quote for
    /// bare bases from live `exchangeInfo`. Prefers USDT-margined, then
    /// USD1, then U, then first alphabetical — among listings the venue
    /// actually reports.
    async fn resolve_venue_symbol(&self, symbol: &str) -> anyhow::Result<String> {
        let candidate = Self::to_venue_symbol(symbol)?;
        let looks_bare = !symbol.trim().contains(['-', '/', '_', ' ']) && candidate.len() <= 6;
        if !looks_bare {
            return Ok(candidate);
        }
        let info = self.get_json("/fapi/v3/exchangeInfo").await?;
        let entries: Vec<(String, String, String)> = info
            .get("symbols")
            .and_then(|s| s.as_array())
            .map(|arr| {
                arr.iter()
                    .filter(|s| {
                        s.get("status").and_then(|v| v.as_str()) == Some("TRADING")
                            && s.get("contractType").and_then(|v| v.as_str())
                                == Some("PERPETUAL")
                    })
                    .filter_map(|s| {
                        Some((
                            s.get("symbol")?.as_str()?.to_string(),
                            s.get("baseAsset")?.as_str()?.to_string(),
                            s.get("quoteAsset")?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        // A bare candidate that is itself a listed venue symbol wins
        // verbatim (e.g. short concatenated spellings).
        if entries
            .iter()
            .any(|(s, _, _)| s.eq_ignore_ascii_case(&candidate))
        {
            return Ok(candidate.to_uppercase());
        }
        resolve_bare_base(&candidate, &entries)
            .map(|s| s.to_string())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Aster lists no TRADING perpetual for base '{candidate}'; refusing to guess a quote suffix"
                )
            })
    }

    #[allow(dead_code)]
    async fn get_json(&self, path: &str) -> anyhow::Result<serde_json::Value> {
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
            self.rate_limiter.record_failure(is_429);
            anyhow::bail!("Aster API error: {} {}", resp.status(), path);
        }

        let text = resp.text().await?;
        self.rate_limiter.record_success_path(path, text.clone());
        let data = serde_json::from_str::<serde_json::Value>(&text)?;
        Ok(data)
    }
}

#[async_trait]
impl ExchangeAdapter for AsterAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Aster"
    }

    async fn ping(&self) -> anyhow::Result<()> {
        // Bypass the single-flight cache (get_json) and any CDN edge cache:
        // a unique query param forces the request to reach origin so the
        // measured round-trip reflects real exchange latency.
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let url = format!("{}/fapi/v3/ping?cb={}", self.base_url, nonce);
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Aster ping failed: {}", resp.status());
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
            anyhow::bail!("Aster ping returned non-JSON response (likely edge challenge)");
        }
        Ok(())
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let info = self.get_json("/fapi/v3/exchangeInfo").await?;
        let symbols = info
            .get("symbols")
            .and_then(|s| s.as_array())
            .ok_or_else(|| anyhow::anyhow!("Aster exchangeInfo response missing symbols"))?;
        Ok(symbols
            .iter()
            .filter_map(|s| parse_exchange_info_entry(&self.venue_id, s))
            .collect())
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        if !VALID_DEPTH_LIMITS.contains(&ORDERBOOK_LIMIT) {
            anyhow::bail!("Aster ORDERBOOK_LIMIT {ORDERBOOK_LIMIT} is not a venue-valid depth");
        }
        let venue_symbol = self.resolve_venue_symbol(symbol).await?;
        let path = format!(
            "/fapi/v3/depth?symbol={}&limit={}",
            venue_symbol, ORDERBOOK_LIMIT
        );
        let res = self.get_json(&path).await?;
        parse_depth(&self.venue_id, symbol, &venue_symbol, &res)
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let book = self.fetch_orderbook(symbol).await?;
        let bid = book
            .best_bid_price()
            .ok_or_else(|| anyhow::anyhow!("Aster book for {symbol} has no bid"))?;
        let ask = book
            .best_ask_price()
            .ok_or_else(|| anyhow::anyhow!("Aster book for {symbol} has no ask"))?;
        Ok(Ticker {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            bid,
            ask,
            last: (bid + ask) / dec!(2),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate> {
        let venue_symbol = self.resolve_venue_symbol(symbol).await?;
        let path = format!("/fapi/v3/premiumIndex?symbol={}", venue_symbol);
        let res = self.get_json(&path).await?;
        parse_premium_index(&self.venue_id, symbol, &venue_symbol, &res)
    }

    /// Bulk funding via `GET /fapi/v3/premiumIndex` (all symbols) joined to
    /// the `exchangeInfo` quote map — far cheaper than per-symbol queries
    /// for the monitor's venue sweep.
    async fn fetch_funding_rates(&self) -> anyhow::Result<Vec<FundingRate>> {
        let info = self.get_json("/fapi/v3/exchangeInfo").await?;
        let quote_of: HashMap<String, String> = info
            .get("symbols")
            .and_then(|s| s.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| {
                        Some((
                            s.get("symbol")?.as_str()?.to_uppercase(),
                            s.get("quoteAsset")?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let all = self.get_json("/fapi/v3/premiumIndex").await?;
        let arr = all
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Aster premiumIndex response is not an array"))?;
        let mut out = Vec::new();
        for entry in arr {
            let venue_symbol = entry.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
            let Some(quote) = quote_of.get(&venue_symbol.to_uppercase()) else {
                continue;
            };
            let base = venue_symbol
                .strip_suffix(quote.as_str())
                .filter(|b| !b.is_empty());
            let Some(base) = base else { continue };
            let rate = dec_from_json(entry.get("lastFundingRate"));
            let next_ms = entry.get("nextFundingTime").and_then(|v| match v {
                serde_json::Value::Number(n) => n.as_i64(),
                serde_json::Value::String(s) => s.parse::<i64>().ok(),
                _ => None,
            });
            let (Some(rate), Some(next_ms)) = (rate, next_ms) else {
                continue;
            };
            let Some(next_funding_time) = DateTime::from_timestamp_millis(next_ms) else {
                continue;
            };
            out.push(FundingRate {
                venue: self.venue_id.clone(),
                symbol: Symbol::perp(base, quote).canonical(),
                rate,
                predicted_rate: None,
                interval_hours: 8,
                next_funding_time,
                timestamp: Utc::now(),
            });
        }
        Ok(out)
    }

    /// Bulk 24h notional via `GET /fapi/v3/ticker/24hr`, keyed by canonical
    /// base. USDT-margined entries win on base collisions (deepest book);
    /// other quotes only fill bases USDT does not cover.
    async fn fetch_volumes_24h(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        let all = self.get_json("/fapi/v3/ticker/24hr").await?;
        let arr = all
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Aster 24hr ticker response is not an array"))?;
        let mut usdt: HashMap<String, Decimal> = HashMap::new();
        let mut other: HashMap<String, Decimal> = HashMap::new();
        for entry in arr {
            let venue_symbol = entry.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
            let vol = dec_from_json(entry.get("quoteVolume"));
            let Some(vol) = vol else { continue };
            if let Some(base) = venue_symbol.strip_suffix("USDT") {
                if !base.is_empty() {
                    usdt.insert(base.to_string(), vol);
                }
            } else {
                // Non-USDT quotes keyed by full venue spelling so bases
                // never collide across margin assets.
                other.insert(venue_symbol.to_string(), vol);
            }
        }
        let mut volumes: HashMap<String, Decimal> = HashMap::new();
        for (base, vol) in usdt {
            volumes.insert(base, vol);
        }
        for (sym, vol) in other {
            volumes.entry(sym).or_insert(vol);
        }
        Ok(volumes)
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        anyhow::bail!("Live account fetch is not implemented for {}; refusing to fabricate an account snapshot", self.name())
    }

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        anyhow::bail!("Live order execution is not implemented for {}; refusing to fabricate a fill", self.name())
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        anyhow::bail!("Live order cancellation is not implemented for {}", self.name())
    }

    async fn set_leverage(&self, _symbol: &str, _leverage: u32) -> anyhow::Result<()> {
        anyhow::bail!("Leverage updates are not implemented for {}; refusing to pretend to apply it", self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn btc_entry() -> serde_json::Value {
        serde_json::json!({
            "symbol": "BTCUSDT",
            "status": "TRADING",
            "contractType": "PERPETUAL",
            "baseAsset": "BTC",
            "quoteAsset": "USDT",
            "pricePrecision": 1,
            "quantityPrecision": 3,
            "filters": [
                {"filterType": "PRICE_FILTER", "tickSize": "0.1"},
                {"filterType": "LOT_SIZE", "minQty": "0.001", "maxQty": "1000"},
                {"filterType": "MIN_NOTIONAL", "notional": "5"}
            ]
        })
    }

    #[test]
    fn parse_entry_uses_venue_quote_fields_not_suffix_guessing() {
        let venue = VenueId::new("aster");
        let meta = parse_exchange_info_entry(&venue, &btc_entry()).expect("valid entry");
        assert_eq!(meta.symbol.raw, "BTC-USDT");
        assert_eq!(meta.venue_symbol, "BTCUSDT");
        assert_eq!(meta.price_decimals, 1);
        assert_eq!(meta.size_decimals, 3);
        assert_eq!(meta.min_order_size, dec!(0.001));
        assert_eq!(meta.min_notional, dec!(5));

        // USD1-margined listing parses with its own quote.
        let mut usd1 = btc_entry();
        usd1["symbol"] = serde_json::json!("BTCUSD1");
        usd1["quoteAsset"] = serde_json::json!("USD1");
        let meta = parse_exchange_info_entry(&venue, &usd1).expect("usd1 entry");
        assert_eq!(meta.symbol.raw, "BTC-USD1");

        // Non-TRADING, non-perpetual, and symbol/quote mismatches fail closed.
        let mut halted = btc_entry();
        halted["status"] = serde_json::json!("BREAK");
        assert!(parse_exchange_info_entry(&venue, &halted).is_none());
        let mut dated = btc_entry();
        dated["contractType"] = serde_json::json!("CURRENT_QUARTER");
        assert!(parse_exchange_info_entry(&venue, &dated).is_none());
        let mut mismatched = btc_entry();
        mismatched["quoteAsset"] = serde_json::json!("USDC");
        assert!(parse_exchange_info_entry(&venue, &mismatched).is_none());
    }

    #[test]
    fn bare_base_resolution_prefers_usdt_without_hardcoding() {
        let entries = vec![
            ("BTCU".to_string(), "BTC".to_string(), "U".to_string()),
            ("BTCUSD1".to_string(), "BTC".to_string(), "USD1".to_string()),
            ("BTCUSDT".to_string(), "BTC".to_string(), "USDT".to_string()),
        ];
        assert_eq!(resolve_bare_base("BTC", &entries), Some("BTCUSDT"));
        let usd1_only = vec![("BTCUSD1".to_string(), "BTC".to_string(), "USD1".to_string())];
        assert_eq!(resolve_bare_base("BTC", &usd1_only), Some("BTCUSD1"));
        assert_eq!(resolve_bare_base("ETH", &entries), None);
    }

    #[test]
    fn to_venue_symbol_maps_qualified_and_passes_through_concatenated() {
        assert_eq!(to_venue_symbol_helper("BTC-USDT"), "BTCUSDT");
        assert_eq!(to_venue_symbol_helper("btc/usdt"), "BTCUSDT");
        assert_eq!(to_venue_symbol_helper("BTCUSDT"), "BTCUSDT");
        assert!(AsterAdapter::to_venue_symbol("BTC-USDT-PERP").is_err());
        assert!(AsterAdapter::to_venue_symbol("").is_err());
        assert!(AsterAdapter::to_venue_symbol("BTC!USDT").is_err());
    }

    fn to_venue_symbol_helper(s: &str) -> String {
        AsterAdapter::to_venue_symbol(s).expect("qualified maps")
    }

    #[test]
    fn parse_depth_reads_string_pairs_and_rejects_empty() {
        let venue = VenueId::new("aster");
        let data = serde_json::json!({
            "lastUpdateId": 1,
            "bids": [["76888.3", "5.266"]],
            "asks": [["76892.3", "0.064"]]
        });
        let book = parse_depth(&venue, "BTC-USDT", "BTCUSDT", &data).expect("parses");
        assert_eq!(book.bids[0].price, dec!(76888.3));
        assert_eq!(book.asks[0].size, dec!(0.064));
        let empty = serde_json::json!({"bids": [], "asks": []});
        assert!(parse_depth(&venue, "BTC-USDT", "BTCUSDT", &empty).is_err());
    }

    #[test]
    fn parse_premium_index_checks_symbol_and_time() {
        let venue = VenueId::new("aster");
        let data = serde_json::json!({
            "symbol": "BTCUSDT",
            "lastFundingRate": "0.00001473",
            "nextFundingTime": 1789459200000i64
        });
        let fr = parse_premium_index(&venue, "BTC-USDT", "BTCUSDT", &data).expect("parses");
        assert_eq!(fr.rate, dec!(0.00001473));
        assert_eq!(fr.interval_hours, 8);

        let wrong = serde_json::json!({
            "symbol": "ETHUSDT",
            "lastFundingRate": "0.00001",
            "nextFundingTime": 1789459200000i64
        });
        assert!(parse_premium_index(&venue, "BTC-USDT", "BTCUSDT", &wrong).is_err());
        let mut no_rate = data.clone();
        no_rate.as_object_mut().unwrap().remove("lastFundingRate");
        assert!(parse_premium_index(&venue, "BTC-USDT", "BTCUSDT", &no_rate).is_err());
    }

    #[tokio::test]
    async fn fail_closed_account_order_leverage_paths() {
        let adapter = AsterAdapter::new(None, None, None);
        assert_eq!(adapter.name(), "Aster");
        assert_eq!(adapter.venue_id(), VenueId::new("aster"));
        assert!(adapter.fetch_account().await.is_err());
        assert!(
            adapter
                .place_order(OrderRequest::new_limit(
                    adapter.venue_id(),
                    "BTC-USDT",
                    arbitrager_core::types::Side::Buy,
                    dec!(70000.0),
                    dec!(0.1),
                ))
                .await
                .is_err()
        );
        assert!(
            adapter
                .cancel_order(CancelRequest {
                    venue: adapter.venue_id(),
                    order_id: Some("x".into()),
                    client_order_id: None,
                    symbol: "BTC-USDT".into(),
                })
                .await
                .is_err()
        );
        assert!(adapter.set_leverage("BTC-USDT", 5).await.is_err());
    }
}
