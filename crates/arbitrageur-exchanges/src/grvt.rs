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

/// GRVT (market-data.grvt.io, JSON POST) connector.
///
/// Public read-only POST only, no credentials. Verified live 2026-09-15:
/// `POST /full/v1/book {"instrument":"BTC_USDT_Perp","depth":N}` works for
/// depth in {10, 50, 100} and returns 3031 "Depth is invalid" for
/// {1, 5, 15, 20, 25, 30, 200} — the adapter only ever requests a value
/// from [`VALID_BOOK_DEPTHS`] and fails closed otherwise.
/// `POST /full/v1/all_instruments {"is_active":true,"kinds":["PERPETUAL"]}`
/// and `POST /full/v1/funding {"instrument":..,"limit":1}` verified live.
/// WebSocket (`wss://market-data.grvt.io/ws/full` JSONRPC subscribe) is
/// recorded for future work; this adapter is REST-only for v1.
///
/// Admitted to `get_venues` (paper read-only): the payload shape is
/// live-verified above and the adapter serves books, tickers, markets, and
/// funding through the read-only paths. Account/order/leverage stay
/// fail-closed.
#[allow(dead_code)]
fn is_valid_evm_address(a: &str) -> bool {
    let s = a.trim();
    s.len() == 42 && s.starts_with("0x") && s[2..].chars().all(|c| c.is_ascii_hexdigit())
}

pub struct GrvtAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    rate_limiter: AccountRequestCoordinator,
    wallet_address: RwLock<Option<String>>,
    wallet: RwLock<Option<EvmWalletSigner>>,
}

/// Live-probed accepted `depth` values for `POST /full/v1/book`
/// (2026-09-15: 10/50/100 ok; 1/5/15/20/25/30/200 -> 3031 invalid).
const VALID_BOOK_DEPTHS: [u32; 3] = [10, 50, 100];
/// Orderbook depth requested per snapshot.
const ORDERBOOK_DEPTH: u32 = 50;

fn dec_from_json(v: Option<&serde_json::Value>) -> Option<Decimal> {
    match v? {
        serde_json::Value::String(s) => Decimal::from_str_exact(s).ok(),
        serde_json::Value::Number(n) => Decimal::from_str_exact(&n.to_string()).ok(),
        _ => None,
    }
}

fn parse_level(o: &serde_json::Value) -> Option<(Decimal, Decimal)> {
    let p = dec_from_json(o.get("price"))?;
    let q = dec_from_json(o.get("size"))?;
    if p > dec!(0) && q > dec!(0) {
        Some((p, q))
    } else {
        None
    }
}

/// Parse one `all_instruments` entry. Only `PERPETUAL` kind entries with a
/// non-empty base/quote become markets; everything else fails closed per
/// entry (never aborts the whole listing).
fn parse_instrument_entry(venue: &VenueId, e: &serde_json::Value) -> Option<MarketMetadata> {
    if e.get("kind").and_then(|v| v.as_str()) != Some("PERPETUAL") {
        return None;
    }
    let instrument = e.get("instrument").and_then(|v| v.as_str())?;
    let base = e.get("base").and_then(|v| v.as_str())?;
    let quote = e.get("quote").and_then(|v| v.as_str())?;
    if base.is_empty() || quote.is_empty() {
        return None;
    }
    // Sanity: instrument id must be the canonical BASE_QUOTE_Perp spelling.
    if instrument != format!("{base}_{quote}_Perp") {
        return None;
    }
    let price_tick = dec_from_json(e.get("tick_size"))?;
    let size_tick = dec_from_json(e.get("min_size"))?;
    if !(price_tick > dec!(0) && size_tick > dec!(0)) {
        return None;
    }
    let min_notional = dec_from_json(e.get("min_notional")).unwrap_or(dec!(5.0));
    Some(MarketMetadata {
        symbol: Symbol::perp(base, quote),
        venue: venue.clone(),
        venue_symbol: instrument.to_string(),
        price_decimals: price_tick.normalize().scale(),
        size_decimals: size_tick.normalize().scale(),
        min_order_size: size_tick,
        min_notional,
        // GRVT publishes no per-instrument fee in market-data; taker uses
        // the venue's standard rate until the fee schedule is confirmed.
        maker_fee_bps: dec!(0.0),
        taker_fee_bps: dec!(2.0),
        contract_size: dec!(1.0),
        is_active: true,
    })
}

fn parse_book(
    venue: &VenueId,
    symbol: &str,
    instrument: &str,
    data: &serde_json::Value,
) -> anyhow::Result<OrderBook> {
    if data.get("code").and_then(|c| c.as_u64()).is_some() {
        anyhow::bail!(
            "GRVT book rejected request for {instrument}: {}",
            data.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error")
        );
    }
    let result = data
        .get("result")
        .ok_or_else(|| anyhow::anyhow!("GRVT book response missing result"))?;
    let returned = result.get("instrument").and_then(|v| v.as_str()).unwrap_or("");
    if returned != instrument {
        anyhow::bail!("GRVT book returned instrument {returned} for requested {instrument}");
    }
    let mut book = OrderBook::new(venue.clone(), symbol.to_string());
    if let Some(bids) = result.get("bids").and_then(|b| b.as_array()) {
        for b in bids {
            if let Some((p, q)) = parse_level(b) {
                book.bids.push(BookLevel::new(p, q));
            }
        }
    }
    if let Some(asks) = result.get("asks").and_then(|a| a.as_array()) {
        for a in asks {
            if let Some((p, q)) = parse_level(a) {
                book.asks.push(BookLevel::new(p, q));
            }
        }
    }
    if book.bids.is_empty() && book.asks.is_empty() {
        anyhow::bail!("No live orderbook levels returned from GRVT book for {symbol}");
    }
    book.timestamp = Utc::now();
    Ok(book)
}

fn nanos_to_dt(ns: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_nanos(ns)
}

fn parse_funding_entry(
    venue: &VenueId,
    symbol: &str,
    instrument: &str,
    entry: &serde_json::Value,
) -> anyhow::Result<FundingRate> {
    let returned = entry.get("instrument").and_then(|v| v.as_str()).unwrap_or("");
    if returned != instrument {
        anyhow::bail!("GRVT funding returned instrument {returned} for requested {instrument}");
    }
    let rate = dec_from_json(entry.get("funding_rate")).ok_or_else(|| {
        anyhow::anyhow!("GRVT funding for {instrument} has no parseable funding_rate")
    })?;
    let time_ns = entry.get("funding_time").and_then(|v| match v {
        serde_json::Value::String(s) => s.parse::<i64>().ok(),
        serde_json::Value::Number(n) => n.as_i64(),
        _ => None,
    });
    let interval_hours = entry
        .get("funding_interval_hours")
        .and_then(|v| v.as_u64())
        .unwrap_or(8) as u32;
    let next_funding_time = time_ns
        .map(|ns| nanos_to_dt(ns) + chrono::Duration::hours(i64::from(interval_hours)))
        .unwrap_or_else(Utc::now);
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

impl GrvtAdapter {
    pub fn new(base_url: Option<String>) -> Self {
        Self::new_with_venue(base_url, "grvt")
    }

    /// Venue-parameterized constructor following the Lighter/Vanta pattern.
    pub fn new_with_venue(base_url: Option<String>, venue_id: &str) -> Self {
        let base = base_url.unwrap_or_else(|| "https://market-data.grvt.io".to_string());
        Self {
            venue_id: VenueId::new(venue_id),
            client: Client::builder().timeout(std::time::Duration::from_secs(15)).build().unwrap_or_default(),
            base_url: base,
            rate_limiter: AccountRequestCoordinator::new(venue_id, 150, 2000, 500, 10000),
            wallet_address: RwLock::new(None),
            wallet: RwLock::new(None),
        }
    }

    /// Map a UI/canonical symbol to the GRVT instrument spelling
    /// `BASE_QUOTE_Perp`. `BTC`, `BTC-USDT`, `BTC_USDT_Perp` all map to
    /// `BTC_USDT_Perp`; the quote defaults to USDT (all live GRVT perps are
    /// USDT-quoted) and is explicit when the caller qualifies it.
    fn to_venue_symbol(symbol: &str) -> anyhow::Result<String> {
        let trimmed = symbol.trim();
        if trimmed.is_empty() {
            anyhow::bail!("GRVT symbol is empty");
        }
        let upper = trimmed.to_uppercase();
        if upper.ends_with("_PERP") {
            let parts: Vec<&str> = upper.trim_end_matches("_PERP").split('_').collect();
            if parts.len() == 2
                && !parts[0].is_empty()
                && !parts[1].is_empty()
                && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_alphanumeric()))
            {
                return Ok(format!("{}_{}_Perp", parts[0], parts[1]));
            }
            anyhow::bail!("GRVT symbol '{symbol}' is not BASE_QUOTE_Perp");
        }
        let tokens: Vec<&str> = trimmed.split(['-', '/', '_']).collect();
        let (base, quote) = match tokens.as_slice() {
            [b] => (*b, "USDT"),
            [b, q] => (*b, *q),
            _ => anyhow::bail!("GRVT symbol '{symbol}' must be BASE or BASE-QUOTE"),
        };
        if base.is_empty()
            || quote.is_empty()
            || !base.chars().all(|c| c.is_ascii_alphanumeric())
            || !quote.chars().all(|c| c.is_ascii_alphanumeric())
        {
            anyhow::bail!("GRVT symbol '{symbol}' must be ASCII alphanumeric");
        }
        Ok(format!("{}_{}_Perp", base.to_uppercase(), quote.to_uppercase()))
    }

    /// Bind (or clear) the locally-held EVM agent wallet used for future
    /// wallet-direct signing (agent-delegation model: main address is public,
    /// private key is the agent key only — they intentionally differ). The hex key is parsed into a zeroizing signer
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
        if let Some(a) = addr.as_deref() {
            if !is_valid_evm_address(a) {
                anyhow::bail!("GRVT wallet address is not a valid 0x EVM address");
            }
        }
        match private_key_hex
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
        {
            Some(hex_key) => {
                let signer = EvmWalletSigner::from_hex(&hex_key)?;
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

    /// Derived agent-signer address (from the delegated private key), if bound.
    pub fn agent_address_hex(&self) -> Option<String> {
        self.wallet.read().as_ref().map(|w| w.address_hex())
    }

    #[allow(dead_code)]
    async fn post_json(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let cache_key = format!("{path}|{body}");
        if let Some(cached_str) = self
            .rate_limiter
            .acquire_or_cache_path(&cache_key, true)
            .await?
        {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&cached_str) {
                return Ok(val);
            }
        }

        let url = format!("{}{}", self.base_url, path);
        let resp = match self.client.post(&url).json(&body).send().await {
            Ok(r) => r,
            Err(e) => {
                self.rate_limiter.record_failure(false);
                return Err(e.into());
            }
        };

        if !resp.status().is_success() {
            let is_429 = resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS;
            self.rate_limiter.record_failure(is_429);
            anyhow::bail!("GRVT API error: {} {}", resp.status(), path);
        }

        let text = resp.text().await?;
        self.rate_limiter
            .record_success_path(&cache_key, text.clone());
        let data = serde_json::from_str::<serde_json::Value>(&text)?;
        Ok(data)
    }
}

#[async_trait]
impl ExchangeAdapter for GrvtAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "GRVT"
    }

    async fn ping(&self) -> anyhow::Result<()> {
        // POST bypasses the single-flight GET-style cache by construction;
        // depth 10 is the cheapest live-probed valid depth.
        let url = format!("{}/full/v1/book", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(&serde_json::json!({"instrument": "BTC_USDT_Perp", "depth": 10}))
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("GRVT ping failed: {}", resp.status());
        }
        let is_json = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| ct.contains("json"))
            .unwrap_or(false);
        if !is_json {
            anyhow::bail!("GRVT ping returned non-JSON response (likely edge challenge)");
        }
        Ok(())
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let data = self
            .post_json(
                "/full/v1/all_instruments",
                serde_json::json!({"is_active": true, "kinds": ["PERPETUAL"]}),
            )
            .await?;
        let arr = data
            .get("result")
            .and_then(|r| r.as_array())
            .ok_or_else(|| anyhow::anyhow!("GRVT all_instruments response missing result array"))?;
        Ok(arr
            .iter()
            .filter_map(|e| parse_instrument_entry(&self.venue_id, e))
            .collect())
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        if !VALID_BOOK_DEPTHS.contains(&ORDERBOOK_DEPTH) {
            anyhow::bail!("GRVT ORDERBOOK_DEPTH {ORDERBOOK_DEPTH} is not venue-accepted (want one of {VALID_BOOK_DEPTHS:?})");
        }
        let instrument = Self::to_venue_symbol(symbol)?;
        let res = self
            .post_json(
                "/full/v1/book",
                serde_json::json!({"instrument": instrument, "depth": ORDERBOOK_DEPTH}),
            )
            .await?;
        parse_book(&self.venue_id, symbol, &instrument, &res)
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let book = self.fetch_orderbook(symbol).await?;
        let bid = book
            .best_bid_price()
            .ok_or_else(|| anyhow::anyhow!("GRVT book for {symbol} has no bid"))?;
        let ask = book
            .best_ask_price()
            .ok_or_else(|| anyhow::anyhow!("GRVT book for {symbol} has no ask"))?;
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
        let instrument = Self::to_venue_symbol(symbol)?;
        let res = self
            .post_json(
                "/full/v1/funding",
                serde_json::json!({"instrument": instrument, "limit": 1}),
            )
            .await?;
        let entry = res
            .get("result")
            .and_then(|r| r.as_array())
            .and_then(|a| a.first())
            .ok_or_else(|| {
                anyhow::anyhow!("GRVT funding for {instrument} returned an empty result")
            })?;
        parse_funding_entry(&self.venue_id, symbol, &instrument, entry)
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        anyhow::bail!("Live account fetch is not implemented for {}; refusing to fabricate an account snapshot", self.name())
    }

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        anyhow::bail!(wallet_execution_unavailable(self.name(), "its settlement chain", "wallet-signed order flow"))
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        anyhow::bail!(wallet_execution_unavailable(self.name(), "its settlement chain", "wallet-signed order flow"))
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
            "instrument": "BTC_USDT_Perp",
            "base": "BTC",
            "quote": "USDT",
            "kind": "PERPETUAL",
            "tick_size": "0.1",
            "min_size": "0.001",
            "min_notional": "5.0",
            "funding_interval_hours": 8
        })
    }

    #[test]
    fn orderbook_depth_constant_is_venue_accepted() {
        assert!(VALID_BOOK_DEPTHS.contains(&ORDERBOOK_DEPTH));
    }

    #[test]
    fn parse_instrument_entry_checks_spelling_and_kind() {
        let venue = VenueId::new("grvt");
        let meta = parse_instrument_entry(&venue, &btc_entry()).expect("valid entry");
        assert_eq!(meta.symbol.raw, "BTC-USDT");
        assert_eq!(meta.venue_symbol, "BTC_USDT_Perp");
        assert_eq!(meta.price_decimals, 1);
        assert_eq!(meta.size_decimals, 3);

        let mut spot = btc_entry();
        spot["kind"] = serde_json::json!("SPOT");
        assert!(parse_instrument_entry(&venue, &spot).is_none());
        let mut renamed = btc_entry();
        renamed["instrument"] = serde_json::json!("BTC_USDT_Perp_X");
        assert!(parse_instrument_entry(&venue, &renamed).is_none());
        let mut no_tick = btc_entry();
        no_tick.as_object_mut().unwrap().remove("tick_size");
        assert!(parse_instrument_entry(&venue, &no_tick).is_none());
    }

    #[test]
    fn parse_instrument_entry_rejects_malformed_shapes() {
        let venue = VenueId::new("grvt");
        // Zero ticks fail closed.
        let mut zero_tick = btc_entry();
        zero_tick["tick_size"] = serde_json::json!("0");
        assert!(parse_instrument_entry(&venue, &zero_tick).is_none());
        let mut zero_size = btc_entry();
        zero_size["min_size"] = serde_json::json!("0");
        assert!(parse_instrument_entry(&venue, &zero_size).is_none());
        // Malformed numbers fail closed.
        let mut bad_tick = btc_entry();
        bad_tick["tick_size"] = serde_json::json!("abc");
        assert!(parse_instrument_entry(&venue, &bad_tick).is_none());
        let mut null_tick = btc_entry();
        null_tick["tick_size"] = serde_json::json!(null);
        assert!(parse_instrument_entry(&venue, &null_tick).is_none());
        // Empty base/quote fail closed.
        let mut empty_base = btc_entry();
        empty_base["base"] = serde_json::json!("");
        assert!(parse_instrument_entry(&venue, &empty_base).is_none());
        // Missing kind fails closed (delisted/unknown shape).
        let mut no_kind = btc_entry();
        no_kind.as_object_mut().unwrap().remove("kind");
        assert!(parse_instrument_entry(&venue, &no_kind).is_none());
        // Broker-suffixed instrument ids are rejected by the spelling check.
        let mut suffixed = btc_entry();
        suffixed["instrument"] = serde_json::json!("BTC_USDT_Perp_BROKER");
        assert!(parse_instrument_entry(&venue, &suffixed).is_none());
        // Missing min_notional falls back to the documented 5.0 default.
        let mut no_notional = btc_entry();
        no_notional.as_object_mut().unwrap().remove("min_notional");
        let meta = parse_instrument_entry(&venue, &no_notional).expect("defaults");
        assert_eq!(meta.min_notional, dec!(5.0));
        // Numeric ticks (if the venue ever sends numbers) parse like strings.
        let mut numeric = btc_entry();
        numeric["tick_size"] = serde_json::json!(0.1);
        numeric["min_size"] = serde_json::json!(0.001);
        assert!(parse_instrument_entry(&venue, &numeric).is_some());
    }

    #[test]
    fn venue_symbol_mapping_defaults_usdt_and_accepts_qualified() {
        assert_eq!(
            GrvtAdapter::to_venue_symbol("BTC").unwrap(),
            "BTC_USDT_Perp"
        );
        assert_eq!(
            GrvtAdapter::to_venue_symbol("BTC-USDT").unwrap(),
            "BTC_USDT_Perp"
        );
        assert_eq!(
            GrvtAdapter::to_venue_symbol("BTC_USDT_Perp").unwrap(),
            "BTC_USDT_Perp"
        );
        assert!(GrvtAdapter::to_venue_symbol("").is_err());
        assert!(GrvtAdapter::to_venue_symbol("A-B-C").is_err());
        assert!(GrvtAdapter::to_venue_symbol("BTC!").is_err());
    }

    #[test]
    fn parse_book_rejects_error_code_and_instrument_mismatch() {
        let venue = VenueId::new("grvt");
        let data = serde_json::json!({
            "result": {
                "instrument": "BTC_USDT_Perp",
                "bids": [{"price": "76904.5", "size": "7.281", "num_orders": 2}],
                "asks": [{"price": "76910.0", "size": "0.5", "num_orders": 1}]
            }
        });
        let book = parse_book(&venue, "BTC", "BTC_USDT_Perp", &data).expect("parses");
        assert_eq!(book.bids[0].price, dec!(76904.5));
        assert_eq!(book.asks[0].size, dec!(0.5));

        // Live shape of an invalid-depth rejection.
        let invalid = serde_json::json!({"code": 3031, "message": "Depth is invalid", "status": 400});
        assert!(parse_book(&venue, "BTC", "BTC_USDT_Perp", &invalid).is_err());
        let wrong = serde_json::json!({
            "result": {"instrument": "ETH_USDT_Perp", "bids": [], "asks": []}
        });
        assert!(parse_book(&venue, "BTC", "BTC_USDT_Perp", &wrong).is_err());
        let empty = serde_json::json!({
            "result": {"instrument": "BTC_USDT_Perp", "bids": [], "asks": []}
        });
        assert!(parse_book(&venue, "BTC", "BTC_USDT_Perp", &empty).is_err());
        // Missing result fails closed.
        let no_result = serde_json::json!({"foo": 1});
        assert!(parse_book(&venue, "BTC", "BTC_USDT_Perp", &no_result).is_err());
        // Zero/negative/malformed levels are skipped; a book with only bad
        // levels is empty and fails closed.
        let bad_levels = serde_json::json!({
            "result": {
                "instrument": "BTC_USDT_Perp",
                "bids": [
                    {"price": "0", "size": "1"},
                    {"price": "-5", "size": "1"},
                    {"price": "abc", "size": "1"},
                    {"price": "70000", "size": "0"}
                ],
                "asks": []
            }
        });
        assert!(parse_book(&venue, "BTC", "BTC_USDT_Perp", &bad_levels).is_err());
        // Numeric price/size parse; bad levels are skipped, good ones kept.
        let mixed = serde_json::json!({
            "result": {
                "instrument": "BTC_USDT_Perp",
                "bids": [{"price": 76904.5, "size": 7.281}, {"price": "bad", "size": "1"}],
                "asks": [{"price": "76910.0", "size": "0.5"}]
            }
        });
        let book = parse_book(&venue, "BTC", "BTC_USDT_Perp", &mixed).expect("mixed");
        assert_eq!(book.bids.len(), 1);
        assert_eq!(book.bids[0].price, dec!(76904.5));
    }

    #[test]
    fn parse_funding_entry_reads_rate_and_interval() {
        let venue = VenueId::new("grvt");
        let entry = serde_json::json!({
            "instrument": "BTC_USDT_Perp",
            "funding_rate": "0.0044",
            "funding_time": "1789459200000000000",
            "funding_interval_hours": 8
        });
        let fr = parse_funding_entry(&venue, "BTC", "BTC_USDT_Perp", &entry).expect("parses");
        assert_eq!(fr.rate, dec!(0.0044));
        assert_eq!(fr.interval_hours, 8);
        assert_eq!(
            fr.next_funding_time,
            DateTime::from_timestamp_nanos(1789459200000000000) + chrono::Duration::hours(8)
        );

        let wrong = serde_json::json!({
            "instrument": "ETH_USDT_Perp",
            "funding_rate": "0.001",
            "funding_time": "1789459200000000000"
        });
        assert!(parse_funding_entry(&venue, "BTC", "BTC_USDT_Perp", &wrong).is_err());
        // Missing/malformed funding_rate fails closed.
        let no_rate = serde_json::json!({
            "instrument": "BTC_USDT_Perp",
            "funding_time": "1789459200000000000"
        });
        assert!(parse_funding_entry(&venue, "BTC", "BTC_USDT_Perp", &no_rate).is_err());
        let bad_rate = serde_json::json!({
            "instrument": "BTC_USDT_Perp",
            "funding_rate": "n/a",
            "funding_time": "1789459200000000000"
        });
        assert!(parse_funding_entry(&venue, "BTC", "BTC_USDT_Perp", &bad_rate).is_err());
        // Numeric funding_time and missing interval default cleanly.
        let numeric_time = serde_json::json!({
            "instrument": "BTC_USDT_Perp",
            "funding_rate": "0.001",
            "funding_time": 1789459200000000000i64
        });
        let fr = parse_funding_entry(&venue, "BTC", "BTC_USDT_Perp", &numeric_time)
            .expect("numeric time");
        assert_eq!(fr.rate, dec!(0.001));
        assert_eq!(fr.interval_hours, 8);
    }

    #[tokio::test]
    async fn fail_closed_account_order_leverage_paths() {
        let adapter = GrvtAdapter::new(None);
        assert_eq!(adapter.name(), "GRVT");
        assert_eq!(adapter.venue_id(), VenueId::new("grvt"));
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
        let a = GrvtAdapter::new(None);
        // Empty binding clears without error.
        assert!(a.update_credentials(None, None).is_ok());
        assert_eq!(a.wallet_address_hex(), None);
        // Delegated-agent case: main address differs from agent key — must bind.
        assert!(a
            .update_credentials(
                Some("0x0000000000000000000000000000000000000001".to_string()),
                Some("0x0000000000000000000000000000000000000000000000000000000000000001".to_string()),
            )
            .is_ok());
        assert_eq!(
            a.wallet_address_hex(),
            Some("0x0000000000000000000000000000000000000001".to_string())
        );
        // Invalid address format fails the whole update (no half-bound state).
        assert!(a
            .update_credentials(
                Some("not-an-address".to_string()),
                Some("0x0000000000000000000000000000000000000000000000000000000000000001".to_string()),
            )
            .is_err());
        // Scalar-1 test vector binds (direct-key case).
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
