use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use std::collections::HashMap;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::AccountSnapshot;
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{dec, Decimal, VenueId};
use crate::rate_limiter::AccountRequestCoordinator;
use crate::wallet::{wallet_execution_unavailable, EvmWalletSigner};
use parking_lot::RwLock;

/// edgeX (StarkEx perp DEX) connector.
///
/// Public read-only REST only, no credentials. Verified live 2026-09-15
/// against `https://edgex-prod-v2.edgex.exchange` (the docs' production
/// api-domain; `pro.edgex.exchange` serves the web app and 404s
/// `/api/v2/public/*`):
/// `GET /api/v2/public/meta/getMetaData` -> 173 USDC-settled contracts
/// (`contractId` 30000001 = BTCUSDC, taker 0.00045, maker 0.0004,
/// `fundingRateIntervalMin` 240),
/// `GET /api/v2/public/quote/getDepth?contractId=..&level=15`,
/// `GET /api/v2/public/quote/getTicker?contractId=..`,
/// `GET /api/v2/public/funding/getLatestFundingRate?contractId=..`.
/// All responses are enveloped `{code:"SUCCESS",data:..}` — any other code
/// fails closed. Contract ids always resolve via the live metadata
/// endpoint; ids are never hardcoded (the legacy 10000001 BTCUSDT id
/// already returns empty data).
/// WebSocket (`wss://quote.edgex.exchange`, `depth.{id}.{15|200}`) is
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

pub struct EdgeXAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    rate_limiter: AccountRequestCoordinator,
    wallet_address: RwLock<Option<String>>,
    wallet: RwLock<Option<EvmWalletSigner>>,
}

/// Venue-valid depth levels for `getDepth` (docs + live probe).
const VALID_DEPTH_LEVELS: [u32; 2] = [15, 200];
/// Depth level requested per snapshot.
const ORDERBOOK_LEVEL: u32 = 15;

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

/// Require the edgeX envelope `{"code":"SUCCESS","data":...}` and return
/// the data payload. Anything else fails closed.
fn envelope_data(data: &serde_json::Value) -> anyhow::Result<&serde_json::Value> {
    if data.get("code").and_then(|c| c.as_str()) != Some("SUCCESS") {
        anyhow::bail!(
            "edgeX response code was not SUCCESS: {}",
            data.get("code")
                .and_then(|c| c.as_str())
                .unwrap_or("missing")
        );
    }
    data.get("data")
        .ok_or_else(|| anyhow::anyhow!("edgeX SUCCESS response missing data"))
}

/// Split a concatenated contract name (`BTCUSDC`, `ETHUSDT`) into
/// (base, quote) using the venue's documented quote suffixes, longest
/// first so `USDC` wins over `USD`.
fn split_contract_name(name: &str) -> Option<(&str, &str)> {
    for quote in ["USDC", "USDT", "USD"] {
        if let Some(base) = name.strip_suffix(quote) {
            if !base.is_empty() && base.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Some((base, quote));
            }
        }
    }
    None
}

/// Parse one metadata `contractList` entry. Only `enableTrade` contracts
/// with a splittable name become markets; everything else fails closed per
/// entry.
fn parse_contract_entry(venue: &VenueId, c: &serde_json::Value) -> Option<MarketMetadata> {
    if c.get("enableTrade").and_then(|v| v.as_bool()) != Some(true) {
        return None;
    }
    let contract_id = c.get("contractId").and_then(|v| v.as_str())?;
    let name = c.get("contractName").and_then(|v| v.as_str())?;
    let (base, quote) = split_contract_name(name)?;
    let price_tick = dec_from_json(c.get("tickSize"))?;
    let size_tick = dec_from_json(c.get("stepSize"))?;
    if !(price_tick > dec!(0) && size_tick > dec!(0)) {
        return None;
    }
    let min_order_size = dec_from_json(c.get("minOrderSize")).unwrap_or(size_tick);
    // Venue fee rates are decimal fractions (0.00045 = 4.5 bps).
    let taker_fee_bps =
        dec_from_json(c.get("defaultTakerFeeRate")).map(|f| f * dec!(10000))?;
    let maker_fee_bps =
        dec_from_json(c.get("defaultMakerFeeRate")).map(|f| f * dec!(10000))?;
    if taker_fee_bps < dec!(0) || maker_fee_bps < dec!(0) {
        return None;
    }
    Some(MarketMetadata {
        symbol: Symbol::perp(base, quote),
        venue: venue.clone(),
        // Depth/ticker/funding endpoints all key off the numeric contract
        // id, so the venue spelling IS the contract id.
        venue_symbol: contract_id.to_string(),
        price_decimals: price_tick.normalize().scale(),
        size_decimals: size_tick.normalize().scale(),
        min_order_size,
        min_notional: dec!(10.0),
        maker_fee_bps,
        taker_fee_bps,
        contract_size: dec!(1.0),
        is_active: true,
    })
}

/// Find the tradable contract id for a UI base inside a live metadata
/// payload. Prefers USDC-settled (the entire live catalog), then USDT.
fn match_contract_id<'a>(
    base: &str,
    contracts: &'a [serde_json::Value],
) -> Option<&'a str> {
    for quote in ["USDC", "USDT"] {
        let want = format!("{base}{quote}");
        if let Some(c) = contracts.iter().find(|c| {
            c.get("enableTrade").and_then(|v| v.as_bool()) == Some(true)
                && c
                    .get("contractName")
                    .and_then(|v| v.as_str())
                    .map(|n| n.eq_ignore_ascii_case(&want))
                    .unwrap_or(false)
        }) {
            return c.get("contractId").and_then(|v| v.as_str());
        }
    }
    None
}

fn parse_depth_entry(
    venue: &VenueId,
    symbol: &str,
    contract_id: &str,
    entry: &serde_json::Value,
) -> anyhow::Result<OrderBook> {
    let returned = entry.get("contractId").and_then(|v| match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    });
    if returned.as_deref() != Some(contract_id) {
        anyhow::bail!("edgeX depth returned contract {returned:?} for requested {contract_id}");
    }
    let mut book = OrderBook::new(venue.clone(), symbol.to_string());
    if let Some(bids) = entry.get("bids").and_then(|b| b.as_array()) {
        for b in bids {
            if let Some((p, q)) = parse_level(b) {
                book.bids.push(BookLevel::new(p, q));
            }
        }
    }
    if let Some(asks) = entry.get("asks").and_then(|a| a.as_array()) {
        for a in asks {
            if let Some((p, q)) = parse_level(a) {
                book.asks.push(BookLevel::new(p, q));
            }
        }
    }
    if book.bids.is_empty() && book.asks.is_empty() {
        anyhow::bail!("No live orderbook levels returned from edgeX depth for {symbol}");
    }
    book.timestamp = Utc::now();
    Ok(book)
}

fn funding_interval_hours(meta: &serde_json::Value, contract_id: &str) -> u32 {
    meta.get("data")
        .and_then(|d| d.get("contractList"))
        .and_then(|l| l.as_array())
        .and_then(|contracts| {
            contracts.iter().find(|c| {
                c.get("contractId").and_then(|v| v.as_str()) == Some(contract_id)
            })
        })
        .and_then(|c| c.get("fundingRateIntervalMin"))
        .and_then(|v| match v {
            serde_json::Value::String(s) => s.parse::<u64>().ok(),
            serde_json::Value::Number(n) => n.as_u64(),
            _ => None,
        })
        .filter(|m| *m >= 60 && *m % 60 == 0)
        .map(|m| (m / 60) as u32)
        .unwrap_or(4)
}

fn parse_ticker_funding(
    venue: &VenueId,
    _contract_id: &str,
    contract_name: &str,
    interval_hours: u32,
    t: &serde_json::Value,
) -> anyhow::Result<FundingRate> {
    let rate = dec_from_json(t.get("fundingRate")).ok_or_else(|| {
        anyhow::anyhow!("edgeX ticker for {contract_name} has no parseable fundingRate")
    })?;
    let next_ms = t.get("nextFundingTime").and_then(|v| match v {
        serde_json::Value::String(s) => s.parse::<i64>().ok(),
        serde_json::Value::Number(n) => n.as_i64(),
        _ => None,
    });
    let next_funding_time = next_ms
        .and_then(chrono::DateTime::from_timestamp_millis)
        .unwrap_or_else(Utc::now);
    let (base, quote) = split_contract_name(contract_name).unwrap_or((contract_name, "USDC"));
    Ok(FundingRate {
        venue: venue.clone(),
        symbol: Symbol::perp(base, quote).canonical(),
        rate,
        predicted_rate: None,
        interval_hours,
        next_funding_time,
        timestamp: Utc::now(),
    })
}

impl EdgeXAdapter {
    pub fn new(base_url: Option<String>) -> Self {
        Self::new_with_venue(base_url, "edgex")
    }

    /// Venue-parameterized constructor following the Lighter/Vanta pattern.
    pub fn new_with_venue(base_url: Option<String>, venue_id: &str) -> Self {
        let base =
            base_url.unwrap_or_else(|| "https://edgex-prod-v2.edgex.exchange".to_string());
        Self {
            venue_id: VenueId::new(venue_id),
            client: Client::builder().timeout(std::time::Duration::from_secs(15)).build().unwrap_or_default(),
            base_url: base,
            rate_limiter: AccountRequestCoordinator::new(venue_id, 200, 5000, 1000, 30000),
            wallet_address: RwLock::new(None),
            wallet: RwLock::new(None),
        }
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
                anyhow::bail!("edgeX wallet address is not a valid 0x EVM address");
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
            anyhow::bail!("edgeX API error: {} {}", resp.status(), path);
        }

        let text = resp.text().await?;
        self.rate_limiter.record_success_path(path, text.clone());
        let data = serde_json::from_str::<serde_json::Value>(&text)?;
        Ok(data)
    }

    async fn fetch_metadata(&self) -> anyhow::Result<serde_json::Value> {
        self.get_json("/api/v2/public/meta/getMetaData").await
    }

    /// Resolve any UI symbol (`BTC`, `BTC-USDC`, `BTCUSDC`) to the live
    /// numeric contract id via the metadata endpoint. Never hardcodes ids.
    async fn resolve_contract_id(&self, symbol: &str) -> anyhow::Result<String> {
        let trimmed = symbol.trim();
        if trimmed.is_empty() {
            anyhow::bail!("edgeX symbol is empty");
        }
        // A raw numeric id passes through only after the metadata confirms
        // it names a tradable contract (typos fail closed, not misrouted).
        let meta = self.fetch_metadata().await?;
        let data = envelope_data(&meta)?;
        let contracts: Vec<serde_json::Value> = data
            .get("contractList")
            .and_then(|l| l.as_array())
            .cloned()
            .unwrap_or_default();
        if trimmed.chars().all(|c| c.is_ascii_digit()) {
            let known = contracts.iter().any(|c| {
                c.get("enableTrade").and_then(|v| v.as_bool()) == Some(true)
                    && c.get("contractId").and_then(|v| v.as_str()) == Some(trimmed)
            });
            if known {
                return Ok(trimmed.to_string());
            }
            anyhow::bail!("edgeX lists no tradable contract id '{trimmed}'");
        }
        let upper = trimmed.to_uppercase().replace(['-', '/', '_', ' '], "");
        let base = ["USDC", "USDT", "USD"]
            .iter()
            .find_map(|q| upper.strip_suffix(q))
            .unwrap_or(&upper);
        if base.is_empty() || !base.chars().all(|c| c.is_ascii_alphanumeric()) {
            anyhow::bail!("edgeX symbol '{symbol}' has no usable base asset");
        }
        match_contract_id(base, &contracts).map(|s| s.to_string()).ok_or_else(|| {
            anyhow::anyhow!("edgeX lists no tradable contract for base '{base}'")
        })
    }
}

#[async_trait]
impl ExchangeAdapter for EdgeXAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "edgeX"
    }

    async fn ping(&self) -> anyhow::Result<()> {
        // Bypass the single-flight cache (get_json) and any CDN edge cache:
        // a unique query param forces the request to reach origin.
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let url = format!(
            "{}/api/v2/public/meta/getServerTime?cb={}",
            self.base_url, nonce
        );
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("edgeX ping failed: {}", resp.status());
        }
        let is_json = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| ct.contains("json"))
            .unwrap_or(false);
        if !is_json {
            anyhow::bail!("edgeX ping returned non-JSON response (likely edge challenge)");
        }
        Ok(())
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let meta = self.fetch_metadata().await?;
        let data = envelope_data(&meta)?;
        let contracts = data
            .get("contractList")
            .and_then(|l| l.as_array())
            .ok_or_else(|| anyhow::anyhow!("edgeX metadata response missing data.contractList"))?;
        Ok(contracts
            .iter()
            .filter_map(|c| parse_contract_entry(&self.venue_id, c))
            .collect())
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        if !VALID_DEPTH_LEVELS.contains(&ORDERBOOK_LEVEL) {
            anyhow::bail!("edgeX ORDERBOOK_LEVEL {ORDERBOOK_LEVEL} is not venue-valid (want one of {VALID_DEPTH_LEVELS:?})");
        }
        let contract_id = self.resolve_contract_id(symbol).await?;
        let path = format!(
            "/api/v2/public/quote/getDepth?contractId={}&level={}",
            contract_id, ORDERBOOK_LEVEL
        );
        let res = self.get_json(&path).await?;
        let data = envelope_data(&res)?;
        let entry = data
            .as_array()
            .and_then(|a| {
                a.iter().find(|e| {
                    e.get("contractId").and_then(|v| match v {
                        serde_json::Value::String(s) => Some(s.as_str()),
                        _ => None,
                    }) == Some(contract_id.as_str())
                })
            })
            .ok_or_else(|| {
                anyhow::anyhow!("edgeX depth has no entry for contract {contract_id}")
            })?;
        parse_depth_entry(&self.venue_id, symbol, &contract_id, entry)
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let book = self.fetch_orderbook(symbol).await?;
        let bid = book
            .best_bid_price()
            .ok_or_else(|| anyhow::anyhow!("edgeX book for {symbol} has no bid"))?;
        let ask = book
            .best_ask_price()
            .ok_or_else(|| anyhow::anyhow!("edgeX book for {symbol} has no ask"))?;
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
        let contract_id = self.resolve_contract_id(symbol).await?;
        self.fetch_funding_rates()
            .await?
            .into_iter()
            .find(|r| {
                // Bulk symbols are canonical BASE-QUOTE; match the request's base.
                r.symbol
                    .split('-')
                    .next()
                    .map(|b| {
                        symbol
                            .split(['-', '/', '_'])
                            .next()
                            .unwrap_or(symbol)
                            .eq_ignore_ascii_case(b)
                    })
                    .unwrap_or(false)
            })
            .ok_or_else(|| {
                anyhow::anyhow!("edgeX funding has no entry for contract {contract_id}")
            })
    }

    /// Bulk funding via `getTicker` (no contract id returns every
    /// contract), which carries `fundingRate` + `nextFundingTime` per the
    /// documented ticker shape — far cheaper than per-contract queries.
    async fn fetch_funding_rates(&self) -> anyhow::Result<Vec<FundingRate>> {
        let meta = self.fetch_metadata().await?;
        let res = self.get_json("/api/v2/public/quote/getTicker").await?;
        let data = envelope_data(&res)?;
        let arr = data
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("edgeX ticker response data is not an array"))?;
        let mut out = Vec::new();
        for t in arr {
            let (Some(id), Some(name)) = (
                t.get("contractId").and_then(|v| v.as_str()),
                t.get("contractName").and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            let interval = funding_interval_hours(&meta, id);
            if let Ok(fr) = parse_ticker_funding(&self.venue_id, id, name, interval, t) {
                out.push(fr);
            }
        }
        Ok(out)
    }

    /// Bulk 24h notional via `getTicker`'s `value` field, keyed by
    /// canonical base.
    async fn fetch_volumes_24h(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        let res = self.get_json("/api/v2/public/quote/getTicker").await?;
        let data = envelope_data(&res)?;
        let arr = data
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("edgeX ticker response data is not an array"))?;
        let mut volumes = HashMap::new();
        for t in arr {
            let (Some(name), Some(vol)) = (
                t.get("contractName").and_then(|v| v.as_str()),
                dec_from_json(t.get("value")),
            ) else {
                continue;
            };
            if let Some((base, _)) = split_contract_name(name) {
                volumes.insert(base.to_string(), vol);
            }
        }
        Ok(volumes)
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        anyhow::bail!("Live account fetch is not implemented for {}; refusing to fabricate an account snapshot", self.name())
    }

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        anyhow::bail!(wallet_execution_unavailable(self.name(), "its StarkEx settlement layer", "wallet-signed order flow"))
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        anyhow::bail!(wallet_execution_unavailable(self.name(), "its StarkEx settlement layer", "wallet-signed order flow"))
    }

    async fn set_leverage(&self, _symbol: &str, _leverage: u32) -> anyhow::Result<()> {
        anyhow::bail!("Leverage updates are not implemented for {}; refusing to pretend to apply it", self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn btc_contract() -> serde_json::Value {
        serde_json::json!({
            "contractId": "30000001",
            "contractName": "BTCUSDC",
            "tickSize": "0.1",
            "stepSize": "0.001",
            "minOrderSize": "0.001",
            "defaultTakerFeeRate": "0.00045",
            "defaultMakerFeeRate": "0.0004",
            "enableTrade": true,
            "fundingRateIntervalMin": "240"
        })
    }

    #[test]
    fn orderbook_level_constant_is_venue_valid() {
        assert!(VALID_DEPTH_LEVELS.contains(&ORDERBOOK_LEVEL));
    }

    #[test]
    fn split_contract_name_prefers_longest_quote_suffix() {
        assert_eq!(split_contract_name("BTCUSDC"), Some(("BTC", "USDC")));
        assert_eq!(split_contract_name("ETHUSDT"), Some(("ETH", "USDT")));
        assert_eq!(split_contract_name("USDC"), None);
        assert_eq!(split_contract_name(""), None);
    }

    #[test]
    fn parse_contract_entry_reads_live_fee_fields() {
        let venue = VenueId::new("edgex");
        let meta = parse_contract_entry(&venue, &btc_contract()).expect("valid entry");
        assert_eq!(meta.symbol.raw, "BTC-USDC");
        // Venue spelling is the numeric contract id (what depth/ticker take).
        assert_eq!(meta.venue_symbol, "30000001");
        assert_eq!(meta.taker_fee_bps, dec!(4.5));
        assert_eq!(meta.maker_fee_bps, dec!(4.0));

        let mut halted = btc_contract();
        halted["enableTrade"] = serde_json::json!(false);
        assert!(parse_contract_entry(&venue, &halted).is_none());
        let mut legacy = btc_contract();
        legacy["contractName"] = serde_json::json!("BTCUSDT");
        let meta = parse_contract_entry(&venue, &legacy).expect("usdt entry");
        assert_eq!(meta.symbol.raw, "BTC-USDT");
    }

    #[test]
    fn parse_contract_entry_rejects_malformed_and_delisted_shapes() {
        let venue = VenueId::new("edgex");
        // Zero ticks fail closed.
        let mut zero_tick = btc_contract();
        zero_tick["tickSize"] = serde_json::json!("0");
        assert!(parse_contract_entry(&venue, &zero_tick).is_none());
        let mut zero_step = btc_contract();
        zero_step["stepSize"] = serde_json::json!("0");
        assert!(parse_contract_entry(&venue, &zero_step).is_none());
        // Missing/negative/malformed fee tiers fail closed (never guessed).
        let mut no_fees = btc_contract();
        no_fees.as_object_mut().unwrap().remove("defaultTakerFeeRate");
        no_fees.as_object_mut().unwrap().remove("defaultMakerFeeRate");
        assert!(parse_contract_entry(&venue, &no_fees).is_none());
        let mut neg_fee = btc_contract();
        neg_fee["defaultTakerFeeRate"] = serde_json::json!("-0.00045");
        assert!(parse_contract_entry(&venue, &neg_fee).is_none());
        let mut bad_fee = btc_contract();
        bad_fee["defaultMakerFeeRate"] = serde_json::json!("free");
        assert!(parse_contract_entry(&venue, &bad_fee).is_none());
        // Missing id/name and unsplittable names fail closed.
        let mut no_id = btc_contract();
        no_id.as_object_mut().unwrap().remove("contractId");
        assert!(parse_contract_entry(&venue, &no_id).is_none());
        let mut odd_name = btc_contract();
        odd_name["contractName"] = serde_json::json!("BTC");
        assert!(parse_contract_entry(&venue, &odd_name).is_none());
        let mut usd = btc_contract();
        usd["contractName"] = serde_json::json!("BTCUSD");
        let meta = parse_contract_entry(&venue, &usd).expect("usd entry");
        assert_eq!(meta.symbol.raw, "BTC-USD");
    }

    #[test]
    fn match_contract_id_prefers_usdc_and_respects_enable_trade() {
        let contracts = vec![btc_contract()];
        assert_eq!(match_contract_id("BTC", &contracts), Some("30000001"));
        // Base matching is case-insensitive; unknown bases miss.
        assert_eq!(match_contract_id("btc", &contracts), Some("30000001"));
        assert_eq!(match_contract_id("ETH", &contracts), None);
        // A delisted (enableTrade=false) contract never resolves.
        let mut halted = btc_contract();
        halted["enableTrade"] = serde_json::json!(false);
        assert_eq!(match_contract_id("BTC", &[halted]), None);
        // USDT fallback when no USDC contract exists.
        let mut usdt_only = btc_contract();
        usdt_only["contractId"] = serde_json::json!("10000001");
        usdt_only["contractName"] = serde_json::json!("BTCUSDT");
        assert_eq!(
            match_contract_id("BTC", &[usdt_only]),
            Some("10000001")
        );
    }

    #[test]
    fn envelope_requires_success_code() {
        let ok = serde_json::json!({"code": "SUCCESS", "data": []});
        assert!(envelope_data(&ok).is_ok());
        let fail = serde_json::json!({"code": "ERROR", "data": []});
        assert!(envelope_data(&fail).is_err());
        let httpish = serde_json::json!({"status": 404, "error": "Not Found"});
        assert!(envelope_data(&httpish).is_err());
    }

    #[test]
    fn parse_depth_entry_checks_contract_and_levels() {
        let venue = VenueId::new("edgex");
        let entry = serde_json::json!({
            "contractId": "30000001",
            "contractName": "BTCUSDC",
            "asks": [{"price": "76818.6", "size": "1.078"}],
            "bids": [{"price": "76818.4", "size": "2.5"}],
            "depthType": "SNAPSHOT"
        });
        let book = parse_depth_entry(&venue, "BTC", "30000001", &entry).expect("parses");
        assert_eq!(book.asks[0].price, dec!(76818.6));
        assert_eq!(book.bids[0].size, dec!(2.5));

        let wrong = serde_json::json!({
            "contractId": "30000002",
            "asks": [{"price": "1", "size": "1"}],
            "bids": [{"price": "1", "size": "1"}]
        });
        assert!(parse_depth_entry(&venue, "BTC", "30000001", &wrong).is_err());
        // An empty book fails closed.
        let empty = serde_json::json!({
            "contractId": "30000001",
            "asks": [],
            "bids": []
        });
        assert!(parse_depth_entry(&venue, "BTC", "30000001", &empty).is_err());
        // Numeric contract ids match; bad levels are skipped, good ones kept.
        let mixed = serde_json::json!({
            "contractId": 30000001,
            "asks": [{"price": "76818.6", "size": "1.078"}],
            "bids": [{"price": 76818.4, "size": 2.5}, {"price": "0", "size": "1"}]
        });
        let book = parse_depth_entry(&venue, "BTC", "30000001", &mixed).expect("mixed");
        assert_eq!(book.bids.len(), 1);
        assert_eq!(book.bids[0].price, dec!(76818.4));
    }

    #[test]
    fn parse_ticker_funding_reads_rate_and_next_time() {
        let venue = VenueId::new("edgex");
        let t = serde_json::json!({
            "contractId": "30000001",
            "contractName": "BTCUSDC",
            "lastPrice": "76818.4",
            "fundingRate": "0.00005000",
            "fundingTime": "1789459200000",
            "nextFundingTime": "1789473600000"
        });
        let fr = parse_ticker_funding(&venue, "30000001", "BTCUSDC", 4, &t).expect("parses");
        assert_eq!(fr.rate, dec!(0.00005000));
        assert_eq!(fr.interval_hours, 4);
        assert_eq!(fr.symbol, "BTC-USDC");

        // Missing/malformed fundingRate fails closed.
        let no_rate = serde_json::json!({
            "contractId": "30000001",
            "contractName": "BTCUSDC",
            "nextFundingTime": "1789473600000"
        });
        assert!(parse_ticker_funding(&venue, "30000001", "BTCUSDC", 4, &no_rate).is_err());
        let bad_rate = serde_json::json!({
            "contractId": "30000001",
            "contractName": "BTCUSDC",
            "fundingRate": "pending"
        });
        assert!(parse_ticker_funding(&venue, "30000001", "BTCUSDC", 4, &bad_rate).is_err());
    }

    #[test]
    fn funding_interval_hours_defaults_and_validates() {
        let meta = serde_json::json!({
            "data": {"contractList": [
                {"contractId": "30000001", "fundingRateIntervalMin": "240"},
                {"contractId": "30000002", "fundingRateIntervalMin": 240},
                {"contractId": "30000003", "fundingRateIntervalMin": "90"},
                {"contractId": "30000004", "fundingRateIntervalMin": "30"}
            ]}
        });
        assert_eq!(funding_interval_hours(&meta, "30000001"), 4);
        assert_eq!(funding_interval_hours(&meta, "30000002"), 4);
        // Non-hour multiples and sub-hour intervals fall back to 4h.
        assert_eq!(funding_interval_hours(&meta, "30000003"), 4);
        assert_eq!(funding_interval_hours(&meta, "30000004"), 4);
        // Unknown contracts and missing lists fall back to 4h.
        assert_eq!(funding_interval_hours(&meta, "99999999"), 4);
        assert_eq!(funding_interval_hours(&serde_json::json!({}), "30000001"), 4);
    }

    #[tokio::test]
    async fn fail_closed_account_order_leverage_paths() {
        let adapter = EdgeXAdapter::new(None);
        assert_eq!(adapter.name(), "edgeX");
        assert_eq!(adapter.venue_id(), VenueId::new("edgex"));
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
        let a = EdgeXAdapter::new(None);
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
