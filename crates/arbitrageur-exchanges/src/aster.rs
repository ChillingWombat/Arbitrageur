use crate::rate_limiter::AccountRequestCoordinator;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::{AccountSnapshot, Position};
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{dec, Decimal, OrderStatus, OrderType, Side, TimeInForce, VenueId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use reqwest::Client;
use std::collections::{BTreeMap, HashMap};
use zeroize::Zeroizing;

/// Aster (asterdex.com, Binance-style USDT-margined futures) connector.
///
/// Supports public market data REST endpoints as well as authenticated
/// HMAC-SHA256 order placement, cancellation, and account queries.
#[allow(dead_code)]
pub struct AsterAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    api_key: RwLock<Option<Zeroizing<String>>>,
    api_secret: RwLock<Option<Zeroizing<String>>>,
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
fn resolve_bare_base<'a>(base: &str, entries: &'a [(String, String, String)]) -> Option<&'a str> {
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
        anyhow::bail!("Aster premiumIndex returned symbol {returned} for requested {venue_symbol}");
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
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            base_url: base,
            api_key: RwLock::new(api_key.map(Zeroizing::new)),
            api_secret: RwLock::new(api_secret.map(Zeroizing::new)),
            rate_limiter: AccountRequestCoordinator::new(venue_id, 100, 2000, 500, 10000),
        }
    }

    /// Update live trading credentials dynamically (e.g. on vault unlock/lock).
    pub fn update_credentials(&self, api_key: Option<String>, api_secret: Option<String>) {
        *self.api_key.write() = api_key.map(Zeroizing::new);
        *self.api_secret.write() = api_secret.map(Zeroizing::new);
    }

    /// Whether valid, non-empty API credentials are held in memory.
    pub fn has_credentials(&self) -> bool {
        let k = self.api_key.read();
        let s = self.api_secret.read();
        k.as_ref().is_some_and(|k| !k.trim().is_empty())
            && s.as_ref().is_some_and(|s| !s.trim().is_empty())
    }

    /// Sign request parameters using HMAC-SHA256 according to Binance/Aster standard.
    pub fn sign_params(&self, params: &BTreeMap<&str, String>) -> anyhow::Result<(String, String)> {
        let api_key = self
            .api_key
            .read()
            .clone()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Aster API key not configured or empty"))?;
        let api_secret = self
            .api_secret
            .read()
            .clone()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Aster API secret not configured or empty"))?;

        let mut query = String::new();
        for (i, (k, v)) in params.iter().enumerate() {
            if i > 0 {
                query.push('&');
            }
            query.push_str(k);
            query.push('=');
            query.push_str(v);
        }

        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
            .map_err(|e| anyhow::anyhow!("Invalid HMAC key: {e}"))?;
        mac.update(query.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        query.push_str("&signature=");
        query.push_str(&signature);
        // Transient copy at the signing boundary: the secret must become a
        // request header value here; long-term storage stays zeroizing.
        Ok((api_key.to_string(), query))
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
            return Ok(format!("{}{}", base.to_uppercase(), quote.to_uppercase()));
        }
        let upper = trimmed.to_uppercase();
        if !upper.chars().all(|c| c.is_ascii_alphanumeric()) {
            anyhow::bail!("Aster symbol '{symbol}' must be ASCII alphanumeric");
        }
        // Concatenated venue spellings pass through; a bare base such as
        // BTC is intentionally NOT expanded here (quote would be a guess).
        Ok(upper)
    }

    /// Convert an account row such as `BTCUSDT` back to `BTC-USDT`. Bare
    /// bases and malformed symbols stay unresolved rather than guessing a
    /// quote asset.
    fn canonical_account_symbol(symbol: &str) -> anyhow::Result<String> {
        let sym = symbol.trim().to_uppercase().replace(['/', '_', ' '], "-");
        if sym.contains('-') {
            let parts: Vec<&str> = sym.split('-').filter(|p| !p.is_empty()).collect();
            let [base, quote] = parts.as_slice() else {
                anyhow::bail!("Aster account symbol '{symbol}' is not BASE-QUOTE");
            };
            if !matches!(*quote, "USDT" | "USDC" | "USD1" | "USD" | "U") {
                anyhow::bail!("Aster account symbol '{symbol}' has unsupported quote");
            }
            return Ok(format!("{base}-{quote}"));
        }
        for quote in ["USDT", "USDC", "USD1", "USD", "U"] {
            if let Some(base) = sym.strip_suffix(quote) {
                if !base.is_empty() {
                    return Ok(format!("{base}-{quote}"));
                }
            }
        }
        anyhow::bail!("Aster account symbol '{symbol}' has no supported quote suffix")
    }

    /// Resolve any UI symbol to the venue spelling. Qualified inputs map
    /// directly; unseparated inputs (bare bases like `BTC`/`1000PEPE`, or
    /// concatenated venue spellings like `BTCUSDT`) resolve through live
    /// `exchangeInfo` — a listed venue symbol wins verbatim, otherwise the
    /// base resolves to its preferred-listed quote (USDT, then USD1, then
    /// U, then first alphabetical among listings the venue actually reports).
    async fn resolve_venue_symbol(&self, symbol: &str) -> anyhow::Result<String> {
        let candidate = Self::to_venue_symbol(symbol)?;
        if symbol.trim().contains(['-', '/', '_', ' ']) {
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
                            && s.get("contractType").and_then(|v| v.as_str()) == Some("PERPETUAL")
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

    async fn fetch_market_prices(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        let rows = self.get_json("/fapi/v3/premiumIndex").await?;
        let rows = rows
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Aster premiumIndex response is not an array"))?;

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
        let mut volumes: HashMap<String, Decimal> = HashMap::new();
        for entry in arr {
            let venue_symbol = entry.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
            let Some(vol) = dec_from_json(entry.get("quoteVolume")) else {
                continue;
            };
            // Canonical BASE-QUOTE keys keep quotes distinct across margin
            // assets and match what hub consumers look up; unmappable rows
            // are skipped rather than keyed under the venue spelling.
            let Ok(canonical) = Self::canonical_account_symbol(venue_symbol) else {
                continue;
            };
            volumes.insert(canonical, vol);
        }
        Ok(volumes)
    }

    fn supports_live_execution(&self) -> bool {
        true
    }

    fn live_order_submission_implemented(&self) -> bool {
        true
    }

    fn has_live_credentials(&self) -> bool {
        self.has_credentials()
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        if !self.has_credentials() {
            anyhow::bail!("Cannot fetch live account on Aster: API credentials are not configured or vault is locked");
        }
        let mut params = BTreeMap::new();
        params.insert("timestamp", Utc::now().timestamp_millis().to_string());
        params.insert("recvWindow", "5000".to_string());

        let (api_key, query_string) = self.sign_params(&params)?;
        let url = format!("{}/fapi/v3/account?{}", self.base_url, query_string);

        let resp = self
            .client
            .get(&url)
            .header("X-MBX-APIKEY", api_key)
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("Aster fetch_account failed (HTTP {status}): {text}");
        }

        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            anyhow::anyhow!("Failed to parse Aster account response: {e}; body: {text}")
        })?;

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
                let liq_px = dec_from_json(p.get("liquidationPrice")).filter(|p| *p > dec!(0));
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

    fn normalize_account_position(&self, position: &Position) -> anyhow::Result<Option<Position>> {
        let mut position = position.clone();
        position.symbol = Self::canonical_account_symbol(&position.symbol)?;
        Ok(Some(position))
    }

    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        if !self.has_credentials() {
            anyhow::bail!("Cannot place live order on Aster: API credentials are not configured or vault is locked");
        }
        if order.quantity <= dec!(0) {
            anyhow::bail!("Aster order quantity must be positive");
        }
        let venue_symbol = self.resolve_venue_symbol(&order.symbol).await?;
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
        params.insert("quantity", order.quantity.to_string());
        if let Some(p) = order.price {
            if p <= dec!(0) {
                anyhow::bail!("Aster order limit price must be positive");
            }
            params.insert("price", p.to_string());
        }
        if order.reduce_only {
            params.insert("reduceOnly", "true".to_string());
        }
        if !order.client_order_id.is_empty() {
            params.insert("newClientOrderId", order.client_order_id.clone());
        }
        params.insert("timestamp", Utc::now().timestamp_millis().to_string());
        params.insert("recvWindow", "5000".to_string());

        let (api_key, query_string) = self.sign_params(&params)?;
        let url = format!("{}/fapi/v3/order?{}", self.base_url, query_string);

        let resp = self
            .client
            .post(&url)
            .header("X-MBX-APIKEY", api_key)
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("Aster place_order failed (HTTP {status}): {text}");
        }

        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            anyhow::anyhow!("Failed to parse Aster order response: {e}; body: {text}")
        })?;

        let returned_symbol = v
            .get("symbol")
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow::anyhow!("Aster order response missing symbol: {v}"))?;
        if !returned_symbol.eq_ignore_ascii_case(&venue_symbol) {
            anyhow::bail!(
                "Aster order response symbol '{returned_symbol}' does not match expected '{venue_symbol}': {v}"
            );
        }
        let order_id = match v.get("orderId") {
            Some(serde_json::Value::Number(n)) => n.to_string(),
            Some(serde_json::Value::String(s)) => s.trim().to_string(),
            _ => String::new(),
        };
        if order_id.is_empty() || order_id == "0" {
            anyhow::bail!("Aster order response missing valid orderId: {v}");
        }
        let client_order_id = v
            .get("clientOrderId")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Aster order response missing clientOrderId: {v}"))?;
        if client_order_id != order.client_order_id {
            anyhow::bail!(
                "Aster order response clientOrderId '{client_order_id}' does not match request '{}': {v}",
                order.client_order_id
            );
        }

        let raw_status = v
            .get("status")
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow::anyhow!("Aster order response missing status: {v}"))?;
        let orig_qty = dec_from_json(v.get("origQty"))
            .filter(|q| *q > dec!(0))
            .ok_or_else(|| anyhow::anyhow!("Aster order response missing valid origQty: {v}"))?;
        if orig_qty != order.quantity {
            anyhow::bail!(
                "Aster order response origQty {orig_qty} does not match request {}: {v}",
                order.quantity
            );
        }
        let executed_qty = dec_from_json(v.get("executedQty"))
            .filter(|q| *q >= dec!(0))
            .ok_or_else(|| {
                anyhow::anyhow!("Aster order response missing valid executedQty: {v}")
            })?;
        if executed_qty > orig_qty {
            anyhow::bail!(
                "Aster order response executedQty {executed_qty} exceeds origQty {orig_qty}: {v}"
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
            other => anyhow::bail!("Aster order response has unknown status '{other}': {v}"),
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
                "Aster order response status {order_status:?} is inconsistent with executedQty {executed_qty}/{orig_qty}: {v}"
            );
        }
        let avg_price = if executed_qty > dec!(0) {
            Some(
                dec_from_json(v.get("avgPrice"))
                    .filter(|p| *p > dec!(0))
                    .ok_or_else(|| {
                        anyhow::anyhow!("Aster filled order response missing valid avgPrice: {v}")
                    })?,
            )
        } else {
            None
        };

        // Aster taker fee: 3.5 BPS
        let cum_quote = if executed_qty > dec!(0) {
            dec_from_json(v.get("cumQuote"))
                .filter(|q| *q >= dec!(0))
                .ok_or_else(|| {
                    anyhow::anyhow!("Aster filled order response missing valid cumQuote: {v}")
                })?
        } else {
            dec!(0)
        };
        let fee = cum_quote * Decimal::new(35, 5);

        Ok(OrderResponse {
            order_id,
            client_order_id: client_order_id.to_string(),
            venue: self.venue_id.clone(),
            symbol: order.symbol,
            status: order_status,
            original_qty: orig_qty,
            filled_qty: executed_qty,
            remaining_qty,
            avg_fill_price: avg_price,
            fee,
            fee_currency: "USDT".to_string(),
            raw_response: Some(text),
            timestamp: Utc::now(),
        })
    }

    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
        if !self.has_credentials() {
            anyhow::bail!("Cannot cancel live order on Aster: API credentials are not configured or vault is locked");
        }
        let venue_symbol = self.resolve_venue_symbol(&req.symbol).await?;
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

        let (api_key, query_string) = self.sign_params(&params)?;
        let url = format!("{}/fapi/v3/order?{}", self.base_url, query_string);

        let resp = self
            .client
            .delete(&url)
            .header("X-MBX-APIKEY", api_key)
            .send()
            .await?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Aster cancel_order failed: {text}");
        }
        Ok(())
    }

    async fn set_leverage(&self, symbol: &str, leverage: u32) -> anyhow::Result<()> {
        if !self.has_credentials() {
            anyhow::bail!("Cannot set leverage on Aster: API credentials are not configured or vault is locked");
        }
        let venue_symbol = self.resolve_venue_symbol(symbol).await?;
        let mut params = BTreeMap::new();
        params.insert("symbol", venue_symbol);
        params.insert("leverage", leverage.to_string());
        params.insert("timestamp", Utc::now().timestamp_millis().to_string());
        params.insert("recvWindow", "5000".to_string());

        let (api_key, query_string) = self.sign_params(&params)?;
        let url = format!("{}/fapi/v3/leverage?{}", self.base_url, query_string);

        let resp = self
            .client
            .post(&url)
            .header("X-MBX-APIKEY", api_key)
            .send()
            .await?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Aster set_leverage failed: {text}");
        }
        Ok(())
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
    fn scaled_1000x_entry_and_bare_base_resolve_in_venue_units() {
        // Live Aster exchangeInfo (verified): `1000PEPEUSDT` lists
        // baseAsset `1000PEPE`, quantityPrecision 0, LOT_SIZE minQty/stepSize 1.
        // The canonical engine unit IS the venue-native `1000PEPE` unit —
        // no 1000x rescaling anywhere in the adapter.
        let venue = VenueId::new("aster");
        let mut scaled = btc_entry();
        scaled["symbol"] = serde_json::json!("1000PEPEUSDT");
        scaled["baseAsset"] = serde_json::json!("1000PEPE");
        scaled["quantityPrecision"] = serde_json::json!(0);
        scaled["filters"] = serde_json::json!([
            {"filterType": "PRICE_FILTER", "tickSize": "0.0000001"},
            {"filterType": "LOT_SIZE", "minQty": "1", "stepSize": "1"},
            {"filterType": "MIN_NOTIONAL", "notional": "5"}
        ]);
        let meta = parse_exchange_info_entry(&venue, &scaled).expect("scaled entry");
        assert_eq!(meta.symbol.raw, "1000PEPE-USDT");
        assert_eq!(meta.venue_symbol, "1000PEPEUSDT");
        assert_eq!(meta.size_decimals, 0);
        assert_eq!(meta.min_order_size, dec!(1));
        assert_eq!(meta.contract_size, dec!(1));

        let entries = vec![(
            "1000PEPEUSDT".to_string(),
            "1000PEPE".to_string(),
            "USDT".to_string(),
        )];
        assert_eq!(
            resolve_bare_base("1000PEPE", &entries),
            Some("1000PEPEUSDT")
        );
        // A bare `PEPE` must NOT alias onto the 1000x contract.
        assert_eq!(resolve_bare_base("PEPE", &entries), None);
    }

    #[test]
    fn account_position_for_normalizes_1000x_in_venue_units() {
        let adapter = AsterAdapter::new(None, None, None);
        let raw = Position {
            venue: adapter.venue_id(),
            symbol: "1000PEPEUSDT".to_string(),
            side: Side::Buy,
            quantity: dec!(2),
            entry_price: dec!(0.01),
            mark_price: dec!(0.01),
            unrealized_pnl: dec!(0),
            realized_pnl: dec!(0),
            liquidation_price: Some(dec!(0.005)),
            leverage: dec!(5),
            initial_margin: dec!(0),
            maintenance_margin: dec!(0),
            updated_at: Utc::now(),
        };
        let acc = AccountSnapshot {
            venue: adapter.venue_id(),
            equity: dec!(1000),
            free_margin: dec!(900),
            maintenance_margin: dec!(0),
            margin_usage_pct: dec!(0),
            balances: Vec::new(),
            positions: vec![raw],
            timestamp: Utc::now(),
        };
        let pos = adapter
            .account_position_for(&acc, "1000PEPE-USDT")
            .unwrap()
            .expect("1000x account row should normalize");
        assert_eq!(pos.symbol, "1000PEPE-USDT");
        assert_eq!(pos.quantity, dec!(2));
        assert_eq!(pos.entry_price, dec!(0.01));
        assert!(adapter
            .account_position_for(&acc, "PEPE-USDT")
            .unwrap()
            .is_none());
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

    #[test]
    fn canonical_account_symbol_maps_concatenated_venue_rows() {
        assert_eq!(
            AsterAdapter::canonical_account_symbol("BTCUSDT").unwrap(),
            "BTC-USDT"
        );
        assert_eq!(
            AsterAdapter::canonical_account_symbol("ETHUSD1").unwrap(),
            "ETH-USD1"
        );
        assert!(AsterAdapter::canonical_account_symbol("BTC").is_err());
        assert!(AsterAdapter::canonical_account_symbol("BTC-ETH").is_err());
    }

    #[tokio::test]
    async fn fail_closed_account_order_leverage_paths() {
        let adapter = AsterAdapter::new(None, None, None);
        assert_eq!(adapter.name(), "Aster");
        assert_eq!(adapter.venue_id(), VenueId::new("aster"));
        assert!(adapter.fetch_account().await.is_err());
        assert!(adapter
            .place_order(OrderRequest::new_limit(
                adapter.venue_id(),
                "BTC-USDT",
                arbitrageur_core::types::Side::Buy,
                dec!(70000.0),
                dec!(0.1),
            ))
            .await
            .is_err());
        assert!(adapter
            .cancel_order(CancelRequest {
                venue: adapter.venue_id(),
                order_id: Some("x".into()),
                client_order_id: None,
                symbol: "BTC-USDT".into(),
            })
            .await
            .is_err());
        assert!(adapter.set_leverage("BTC-USDT", 5).await.is_err());
    }

    #[test]
    fn test_aster_signing_and_capabilities() {
        let adapter = AsterAdapter::new(None, None, None);
        assert!(adapter.supports_live_execution());
        assert!(!adapter.has_live_credentials());

        adapter.update_credentials(
            Some("test_key".to_string()),
            Some("test_secret".to_string()),
        );
        assert!(adapter.has_live_credentials());

        let mut params = BTreeMap::new();
        params.insert("symbol", "BTCUSDT".to_string());
        params.insert("side", "BUY".to_string());
        params.insert("timestamp", "1625412345678".to_string());

        let (key, query) = adapter.sign_params(&params).expect("sign params");
        assert_eq!(key, "test_key");
        assert!(query.contains("symbol=BTCUSDT"));
        assert!(query.contains("side=BUY"));
        assert!(query.contains("timestamp=1625412345678"));
        assert!(query.contains("&signature="));

        // Clear credentials (e.g. on vault lock)
        adapter.update_credentials(None, None);
        assert!(!adapter.has_live_credentials());
        assert!(adapter.sign_params(&params).is_err());
    }
}
