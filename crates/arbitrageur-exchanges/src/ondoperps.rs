use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::AccountSnapshot;
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{dec, Decimal, VenueId};
use crate::rate_limiter::AccountRequestCoordinator;
use crate::wallet::{wallet_execution_unavailable, EvmWalletSigner};
use parking_lot::RwLock;

/// Ondo Perps (api.ondoperps.xyz — perpetual venue for tokenized equities
/// plus crypto) connector.
///
/// Public read-only REST only, no credentials. Verified live 2026-09-15:
/// `GET /status` -> `{"success":true,...}`,
/// `GET /v1/markets` -> perps `tradingPairs` (81 markets),
/// `GET /v1/perps/depth?market=BTC-USD.P&depth=..`,
/// `GET /v1/perps/funding_rates?market=..`.
/// WebSocket (`wss://api.ondoperps.xyz/ws`, topOfBooksPerps/depthBooksPerps)
/// exists but stays stubbed for v1 — REST only, `subscribe_*` fail closed
/// at the trait default.
///
/// NOTE: this is NOT Ondo Finance proper (USDY/OUSG mint-redeem via
/// api.gm.ondo.finance), which is not an exchange — nothing is built for it.
#[allow(dead_code)]
pub struct OndoPerpsAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    rate_limiter: AccountRequestCoordinator,
    wallet_address: RwLock<Option<String>>,
    wallet: RwLock<Option<EvmWalletSigner>>,
}

/// Orderbook depth requested per snapshot.
const ORDERBOOK_DEPTH: u32 = 10;

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

/// Require the Ondo envelope `{"success":true,"result":...}` and return the
/// result payload. Anything else fails closed — never parse an error page
/// as market data.
fn envelope_result(data: &serde_json::Value) -> anyhow::Result<&serde_json::Value> {
    if data.get("success").and_then(|s| s.as_bool()) != Some(true) {
        anyhow::bail!("Ondo Perps response did not report success");
    }
    data.get("result")
        .ok_or_else(|| anyhow::anyhow!("Ondo Perps response missing result"))
}

/// Parse one perps `tradingPairs` entry from `GET /v1/markets`.
fn parse_market_entry(venue: &VenueId, m: &serde_json::Value) -> Option<MarketMetadata> {
    let market = m.get("market").and_then(|v| v.as_str())?;
    if !market.ends_with(".P") {
        return None;
    }
    let pair = m.get("pair")?;
    let base = pair.get("base").and_then(|v| v.as_str())?;
    let quote = pair.get("quote").and_then(|v| v.as_str())?;
    if base.is_empty() || quote.is_empty() {
        return None;
    }
    // Sanity: the market id must be the canonical BASE-QUOTE.P spelling.
    if !market.eq_ignore_ascii_case(&format!("{base}-{quote}.P")) {
        return None;
    }
    let size_tick = dec_from_json(m.get("baseIncrement"))?;
    let price_tick = dec_from_json(m.get("quoteIncrement"))?;
    if !(size_tick > dec!(0) && price_tick > dec!(0)) {
        return None;
    }
    // Venue fees are decimal fractions (0.00025 = 2.5 bps).
    let taker_fee_bps = dec_from_json(m.get("takerFee")).map(|f| f * dec!(10000))?;
    let maker_fee_bps = dec_from_json(m.get("makerFee")).map(|f| f * dec!(10000))?;
    if taker_fee_bps < dec!(0) || maker_fee_bps < dec!(0) {
        return None;
    }
    Some(MarketMetadata {
        symbol: Symbol::perp(base, quote),
        venue: venue.clone(),
        venue_symbol: market.to_string(),
        price_decimals: price_tick.normalize().scale(),
        size_decimals: size_tick.normalize().scale(),
        min_order_size: size_tick,
        // The venue exposes no per-market min notional; scaffold default.
        min_notional: dec!(10.0),
        maker_fee_bps,
        taker_fee_bps,
        contract_size: dec!(1.0),
        is_active: true,
    })
}

fn parse_depth(
    venue: &VenueId,
    symbol: &str,
    market: &str,
    data: &serde_json::Value,
) -> anyhow::Result<OrderBook> {
    let result = envelope_result(data)?;
    let returned = result.get("market").and_then(|v| v.as_str()).unwrap_or("");
    if !returned.eq_ignore_ascii_case(market) {
        anyhow::bail!("Ondo Perps depth returned market {returned} for requested {market}");
    }
    let mut book = OrderBook::new(venue.clone(), symbol.to_string());
    if let Some(bids) = result.get("bids").and_then(|b| b.as_array()) {
        for b in bids {
            if let Some((p, q)) = parse_pair(b) {
                book.bids.push(BookLevel::new(p, q));
            }
        }
    }
    if let Some(asks) = result.get("asks").and_then(|a| a.as_array()) {
        for a in asks {
            if let Some((p, q)) = parse_pair(a) {
                book.asks.push(BookLevel::new(p, q));
            }
        }
    }
    if book.bids.is_empty() && book.asks.is_empty() {
        anyhow::bail!("No live orderbook levels returned from Ondo Perps depth for {symbol}");
    }
    book.timestamp = Utc::now();
    Ok(book)
}

fn parse_funding(
    venue: &VenueId,
    symbol: &str,
    market: &str,
    interval_hours: u32,
    data: &serde_json::Value,
) -> anyhow::Result<FundingRate> {
    let result = envelope_result(data)?;
    let returned = result.get("market").and_then(|v| v.as_str()).unwrap_or("");
    if !returned.eq_ignore_ascii_case(market) {
        anyhow::bail!("Ondo Perps funding returned market {returned} for requested {market}");
    }
    let rate = dec_from_json(result.get("rate")).ok_or_else(|| {
        anyhow::anyhow!("Ondo Perps funding for {market} has no parseable rate")
    })?;
    let ends = result
        .get("intervalEnds")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            anyhow::anyhow!("Ondo Perps funding for {market} has no intervalEnds")
        })?;
    let next_funding_time =
        DateTime::parse_from_rfc3339(ends)
            .map(|d| d.with_timezone(&Utc))
            .map_err(|_| {
                anyhow::anyhow!("Ondo Perps funding for {market} has invalid intervalEnds")
            })?;
    Ok(FundingRate {
        venue: venue.clone(),
        symbol: symbol.to_string(),
        rate,
        predicted_rate: None,
        interval_hours,
        next_funding_time,
        timestamp: Utc::now(),
    })
}

impl OndoPerpsAdapter {
    pub fn new(base_url: Option<String>) -> Self {
        Self::new_with_venue(base_url, "ondoperps")
    }

    /// Venue-parameterized constructor following the Lighter/Vanta pattern.
    pub fn new_with_venue(base_url: Option<String>, venue_id: &str) -> Self {
        let base = base_url.unwrap_or_else(|| "https://api.ondoperps.xyz".to_string());
        Self {
            venue_id: VenueId::new(venue_id),
            client: Client::builder().timeout(std::time::Duration::from_secs(15)).build().unwrap_or_default(),
            base_url: base,
            // Venue guidance is ~1 req/s; 1100ms keeps public polling inside it.
            rate_limiter: AccountRequestCoordinator::new(venue_id, 1100, 10000, 1000, 30000),
            wallet_address: RwLock::new(None),
            wallet: RwLock::new(None),
        }
    }

    /// Map a UI/canonical symbol to the Ondo market spelling `BASE-USD.P`.
    /// `BTC`, `BTC-USD.P`, `btc-usd.p` all map to `BTC-USD.P`; equities
    /// follow the same rule (`AAPL` -> `AAPL-USD.P`).
    fn to_venue_symbol(symbol: &str) -> anyhow::Result<String> {
        let trimmed = symbol.trim();
        if trimmed.is_empty() {
            anyhow::bail!("Ondo Perps symbol is empty");
        }
        let without_suffix = trimmed
            .strip_suffix(".P")
            .or_else(|| trimmed.strip_suffix(".p"))
            .unwrap_or(trimmed);
        let tokens: Vec<&str> = without_suffix.split(['-', '/', '_']).collect();
        if tokens.len() >= 2 {
            let quote = tokens[1];
            if !quote.eq_ignore_ascii_case("USD") && !quote.eq_ignore_ascii_case("USDC") {
                anyhow::bail!("Ondo Perps markets are quoted in USD/USDC; '{symbol}' is not supported");
            }
        }
        let base = tokens.first().copied().unwrap_or("").trim();
        if base.is_empty() || !base.chars().all(|c| c.is_ascii_alphanumeric()) {
            anyhow::bail!("Ondo Perps symbol '{symbol}' has no usable base asset");
        }
        Ok(format!("{}-USD.P", base.to_uppercase()))
    }

    #[allow(dead_code)]
    fn from_venue_symbol(market: &str) -> String {
        market
            .strip_suffix(".P")
            .or_else(|| market.strip_suffix(".p"))
            .unwrap_or(market)
            .to_string()
    }

    /// Bind (or clear) the locally-held EVM wallet used for future
    /// wallet-direct signing. The hex key is parsed into a zeroizing signer
    /// immediately; the raw string is never retained. An invalid key fails
    /// the whole update so a half-bound state is impossible. Execution stays
    /// fail-closed until this venue's signing flow is implemented and
    /// audited — binding a key only prepares the vault.
    pub fn update_credentials(
        &self,
        wallet_address: Option<String>,
        private_key_hex: Option<String>,
    ) -> anyhow::Result<()> {
        let addr = wallet_address
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty());
        match private_key_hex
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
        {
            Some(hex_key) => {
                let signer = EvmWalletSigner::from_hex(&hex_key)?;
                if let Some(a) = addr.as_deref() {
                    if !a.eq_ignore_ascii_case(&signer.address_hex()) {
                        anyhow::bail!("Ondo Perps wallet address does not match the supplied private key");
                    }
                }
                *self.wallet.write() = Some(signer);
                *self.wallet_address.write() = addr;
                Ok(())
            }
            None => {
                *self.wallet.write() = None;
                *self.wallet_address.write() = addr;
                Ok(())
            }
        }
    }

    /// Currently bound wallet address, if any (preparation only — execution
    /// stays disabled until the signing flow is audited).
    pub fn wallet_address_hex(&self) -> Option<String> {
        self.wallet_address.read().clone()
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
            anyhow::bail!("Ondo Perps API error: {} {}", resp.status(), path);
        }

        let text = resp.text().await?;
        self.rate_limiter.record_success_path(path, text.clone());
        let data = serde_json::from_str::<serde_json::Value>(&text)?;
        Ok(data)
    }

    /// Funding interval in hours, derived from the market's
    /// `fundingIntervalDivisions` (divisions of a 24h day; 8 -> 3h).
    /// Falls back to 8h when the listing omits the field.
    fn interval_hours_for(market_json: Option<&serde_json::Value>) -> u32 {
        market_json
            .and_then(|m| m.get("fundingIntervalDivisions"))
            .and_then(|v| v.as_u64())
            .filter(|d| (1..=24).contains(d))
            .map(|d| (24 / d) as u32)
            .unwrap_or(8)
    }

    async fn market_json_for(&self, market: &str) -> Option<serde_json::Value> {
        let data = self.get_json("/v1/markets").await.ok()?;
        let result = envelope_result(&data).ok()?;
        result
            .get("perps")?
            .get("tradingPairs")?
            .as_array()?
            .iter()
            .find(|m| {
                m.get("market")
                    .and_then(|v| v.as_str())
                    .map(|id| id.eq_ignore_ascii_case(market))
                    .unwrap_or(false)
            })
            .cloned()
    }
}

#[async_trait]
impl ExchangeAdapter for OndoPerpsAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Ondo Perps"
    }

    async fn ping(&self) -> anyhow::Result<()> {
        // Bypass the single-flight cache (get_json) so the measured
        // round-trip reflects real exchange latency, not a cache hit.
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let url = format!("{}/status?cb={}", self.base_url, nonce);
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Ondo Perps ping failed: {}", resp.status());
        }
        let is_json = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| ct.contains("json"))
            .unwrap_or(false);
        if !is_json {
            anyhow::bail!("Ondo Perps ping returned non-JSON response (likely edge challenge)");
        }
        Ok(())
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let data = self.get_json("/v1/markets").await?;
        let result = envelope_result(&data)?;
        let pairs = result
            .get("perps")
            .and_then(|p| p.get("tradingPairs"))
            .and_then(|t| t.as_array())
            .ok_or_else(|| {
                anyhow::anyhow!("Ondo Perps markets response missing result.perps.tradingPairs")
            })?;
        Ok(pairs
            .iter()
            .filter_map(|m| parse_market_entry(&self.venue_id, m))
            .collect())
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let market = Self::to_venue_symbol(symbol)?;
        let path = format!("/v1/perps/depth?market={}&depth={}", market, ORDERBOOK_DEPTH);
        let res = self.get_json(&path).await?;
        parse_depth(&self.venue_id, symbol, &market, &res)
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let book = self.fetch_orderbook(symbol).await?;
        let bid = book
            .best_bid_price()
            .ok_or_else(|| anyhow::anyhow!("Ondo Perps book for {symbol} has no bid"))?;
        let ask = book
            .best_ask_price()
            .ok_or_else(|| anyhow::anyhow!("Ondo Perps book for {symbol} has no ask"))?;
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
        let market = Self::to_venue_symbol(symbol)?;
        let listing = self.market_json_for(&market).await;
        let interval_hours = Self::interval_hours_for(listing.as_ref());
        let path = format!("/v1/perps/funding_rates?market={}", market);
        let res = self.get_json(&path).await?;
        parse_funding(&self.venue_id, symbol, &market, interval_hours, &res)
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        anyhow::bail!("Live account fetch is not implemented for {}; refusing to fabricate an account snapshot", self.name())
    }

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        anyhow::bail!(wallet_execution_unavailable(self.name(), "Ondo Chain", "wallet-signed order flow"))
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        anyhow::bail!(wallet_execution_unavailable(self.name(), "Ondo Chain", "wallet-signed order flow"))
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
            "market": "BTC-USD.P",
            "pair": {"base": "BTC", "quote": "USD"},
            "baseIncrement": "0.0001",
            "quoteIncrement": "1",
            "makerFee": "0.0001",
            "takerFee": "0.00025",
            "fundingIntervalDivisions": 8
        })
    }

    #[test]
    fn parse_market_entry_reads_live_fee_and_tick_fields() {
        let venue = VenueId::new("ondoperps");
        let meta = parse_market_entry(&venue, &btc_entry()).expect("valid entry");
        assert_eq!(meta.symbol.raw, "BTC-USD");
        assert_eq!(meta.venue_symbol, "BTC-USD.P");
        assert_eq!(meta.price_decimals, 0);
        assert_eq!(meta.size_decimals, 4);
        assert_eq!(meta.min_order_size, dec!(0.0001));
        assert_eq!(meta.maker_fee_bps, dec!(1.0));
        assert_eq!(meta.taker_fee_bps, dec!(2.5));

        // Non-perp market ids and mismatched spellings fail closed.
        let mut spot = btc_entry();
        spot["market"] = serde_json::json!("BTC-USDC");
        assert!(parse_market_entry(&venue, &spot).is_none());
        let mut mismatched = btc_entry();
        mismatched["market"] = serde_json::json!("ETH-USD.P");
        assert!(parse_market_entry(&venue, &mismatched).is_none());
        let mut no_fee = btc_entry();
        no_fee.as_object_mut().unwrap().remove("takerFee");
        assert!(parse_market_entry(&venue, &no_fee).is_none());
    }

    #[test]
    fn venue_symbol_mapping_covers_crypto_and_equities() {
        assert_eq!(
            OndoPerpsAdapter::to_venue_symbol("BTC").unwrap(),
            "BTC-USD.P"
        );
        assert_eq!(
            OndoPerpsAdapter::to_venue_symbol("AAPL").unwrap(),
            "AAPL-USD.P"
        );
        assert_eq!(
            OndoPerpsAdapter::to_venue_symbol("btc-usd.p").unwrap(),
            "BTC-USD.P"
        );
        assert_eq!(
            OndoPerpsAdapter::from_venue_symbol("AAPL-USD.P"),
            "AAPL-USD"
        );
        assert!(OndoPerpsAdapter::to_venue_symbol("").is_err());
        assert!(OndoPerpsAdapter::to_venue_symbol("BTC!").is_err());
        assert!(OndoPerpsAdapter::to_venue_symbol("BTC-SOL").is_err());
        assert!(OndoPerpsAdapter::to_venue_symbol("BTC-ETH").is_err());
    }

    #[test]
    fn parse_depth_requires_success_envelope_and_market_match() {
        let venue = VenueId::new("ondoperps");
        let data = serde_json::json!({
            "success": true,
            "result": {
                "market": "BTC-USD.P",
                "asks": [["76882", "0.195"]],
                "bids": [["76869", "0.0019"]]
            }
        });
        let book = parse_depth(&venue, "BTC", "BTC-USD.P", &data).expect("parses");
        assert_eq!(book.asks[0].price, dec!(76882));
        assert_eq!(book.bids[0].size, dec!(0.0019));

        let failed = serde_json::json!({"success": false, "result": {}});
        assert!(parse_depth(&venue, "BTC", "BTC-USD.P", &failed).is_err());
        let wrong = serde_json::json!({
            "success": true,
            "result": {"market": "ETH-USD.P", "asks": [], "bids": []}
        });
        assert!(parse_depth(&venue, "BTC", "BTC-USD.P", &wrong).is_err());
        let empty = serde_json::json!({
            "success": true,
            "result": {"market": "BTC-USD.P", "asks": [], "bids": []}
        });
        assert!(parse_depth(&venue, "BTC", "BTC-USD.P", &empty).is_err());
    }

    #[test]
    fn parse_funding_reads_rate_and_interval_end() {
        let venue = VenueId::new("ondoperps");
        let data = serde_json::json!({
            "success": true,
            "result": {
                "market": "BTC-USD.P",
                "rate": "0.0000125",
                "intervalEnds": "2026-09-15T08:00:00Z"
            }
        });
        let fr = parse_funding(&venue, "BTC", "BTC-USD.P", 3, &data).expect("parses");
        assert_eq!(fr.rate, dec!(0.0000125));
        assert_eq!(fr.interval_hours, 3);

        let mut no_rate = data.clone();
        no_rate["result"].as_object_mut().unwrap().remove("rate");
        assert!(parse_funding(&venue, "BTC", "BTC-USD.P", 3, &no_rate).is_err());
    }

    #[test]
    fn interval_hours_derive_from_divisions() {
        assert_eq!(
            OndoPerpsAdapter::interval_hours_for(Some(&btc_entry())),
            3
        );
        assert_eq!(OndoPerpsAdapter::interval_hours_for(None), 8);
    }

    #[tokio::test]
    async fn fail_closed_account_order_leverage_paths() {
        let adapter = OndoPerpsAdapter::new(None);
        assert_eq!(adapter.name(), "Ondo Perps");
        assert_eq!(adapter.venue_id(), VenueId::new("ondoperps"));
        assert!(adapter.fetch_account().await.is_err());
        assert!(
            adapter
                .place_order(OrderRequest::new_limit(
                    adapter.venue_id(),
                    "BTC",
                    arbitrageur_core::types::Side::Buy,
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
                    symbol: "BTC".into(),
                })
                .await
                .is_err()
        );
        assert!(adapter.set_leverage("BTC", 5).await.is_err());
    }
}

#[cfg(test)]
mod wallet_direct_tests {
    use super::*;

    #[test]
    fn wallet_binding_prepares_without_enabling_execution() {
        let a = OndoPerpsAdapter::new(None);
        // Empty binding clears without error.
        assert!(a.update_credentials(None, None).is_ok());
        assert_eq!(a.wallet_address_hex(), None);
        // Mismatched address fails the whole update (no half-bound state).
        assert!(a
            .update_credentials(
                Some("0x0000000000000000000000000000000000000001".to_string()),
                Some("0x0000000000000000000000000000000000000000000000000000000000000001".to_string()),
            )
            .is_err());
        assert_eq!(a.wallet_address_hex(), None);
        // Scalar-1 test vector binds; address must match the derived key.
        assert!(a
            .update_credentials(
                Some("0x7E5F4552091a69125d5dfcb7b8c2659029395bdf".to_string()),
                Some("0x0000000000000000000000000000000000000000000000000000000000000001".to_string()),
            )
            .is_ok());
        assert_eq!(
            a.wallet_address_hex(),
            Some("0x7E5F4552091a69125d5dfcb7b8c2659029395bdf".to_string())
        );
        // Binding a key never enables live execution — the signing flow
        // stays fail-closed until implemented and audited.
        assert!(!a.supports_live_execution());
    }
}
