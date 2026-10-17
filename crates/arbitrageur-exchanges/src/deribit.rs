use crate::rate_limiter::AccountRequestCoordinator;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, MarketType, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::{AccountSnapshot, Balance, Position};
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{dec, Decimal, OrderStatus, OrderType, Side, TimeInForce, VenueId};
use async_trait::async_trait;
use chrono::Utc;
use parking_lot::RwLock;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use zeroize::Zeroizing;

/// Deribit connector (https://www.deribit.com/api/v2).
///
/// Inverse-settlement venue: BTC/ETH options and dated futures settle in the
/// base coin; BTC-PERPETUAL/ETH-PERPETUAL are USD-sized but coin-margined.
/// Option premiums and marks are quoted in the base currency (BTC option
/// prices are in BTC), while futures book prices are USD.
///
/// Auth is OAuth2 `client_credentials` — a bearer token cached until ~80% of
/// `expires_in` and refreshed via `refresh_token`. Error 13009
/// (`unauthorized`/`invalid_token`) triggers one re-auth retry.
pub struct DeribitAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    client_id: RwLock<Option<Zeroizing<String>>>,
    client_secret: RwLock<Option<Zeroizing<String>>>,
    token: tokio::sync::Mutex<Option<DeribitToken>>,
    testnet: bool,
    rate_limiter: AccountRequestCoordinator,
    instruments_cache: tokio::sync::Mutex<HashMap<String, CachedInstruments>>,
    /// Settlement index value applied inside the last `fetch_account`, per
    /// currency — the exact rate embedded in the returned USD equity.
    account_index_used: RwLock<HashMap<String, Decimal>>,
}

struct DeribitToken {
    access: Zeroizing<String>,
    refresh: Zeroizing<String>,
    expires_at: std::time::Instant,
}

/// Per-instrument trading metadata needed to translate engine orders into
/// venue units: options trade in contracts (1 contract = `contract_size`
/// base) priced in the base coin; futures/perps trade in USD.
#[derive(Clone, Debug)]
struct InstrumentMeta {
    name: String,
    kind: DeribitKind,
    tick_size: Decimal,
    tick_size_steps: Vec<(Decimal, Decimal)>, // (above_price, tick_size)
    min_amount: Decimal,
    contract_size: Decimal,
    settlement_currency: String,
    strike: Option<Decimal>,
    option_type: Option<DeribitOptionType>,
    expiration_ms: Option<i64>,
    is_active: bool,
    is_open: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DeribitKind {
    #[default]
    Option,
    Future,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeribitOptionType {
    Call,
    Put,
}

/// Top-of-book summary row for one instrument — the scanner's quote
/// surface. Prices are venue units (base coin for options, USD for
/// futures); `underlying_price` is the venue's USD reference.
#[derive(Clone, Debug, Default)]
pub struct DeribitBookSummary {
    pub name: String,
    pub kind: DeribitKind,
    pub bid: Option<Decimal>,
    pub ask: Option<Decimal>,
    pub mark: Option<Decimal>,
    pub underlying_price: Option<Decimal>,
    pub open_interest: Option<Decimal>,
    pub mark_iv: Option<Decimal>,
    pub estimated_delivery_price: Option<Decimal>,
}

/// Option chain entry for the synthetic-forward parity scanner — one row
/// per listed option instrument with the fields needed to pair calls and
/// puts at the same strike/expiry.
#[derive(Clone, Debug)]
pub struct DeribitOptionInstrument {
    pub name: String,
    pub strike: Decimal,
    pub option_type: DeribitOptionType,
    pub expiration_ms: i64,
    pub tick_size: Decimal,
    pub min_amount: Decimal,
    pub contract_size: Decimal,
    pub settlement_currency: String,
    pub is_open: bool,
}

/// One leg trade descriptor for Deribit /private/create_combo
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeribitComboTrade {
    pub instrument_name: String,
    pub amount: i32,
    pub direction: String,
}

/// Leg details within a Deribit combo instrument
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeribitComboLegInfo {
    pub instrument_name: String,
    pub amount: i32,
}

/// Summary details of a Deribit combo instrument
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeribitComboSummary {
    pub id: String,
    pub state: String,
    pub legs: Vec<DeribitComboLegInfo>,
}

struct CachedInstruments {
    meta: HashMap<String, InstrumentMeta>,
    fetched_at: std::time::Instant,
}

const INSTRUMENT_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(300);
const SUPPORTED_CURRENCIES: [&str; 2] = ["BTC", "ETH"];

fn dec_from_json(v: Option<&serde_json::Value>) -> Option<Decimal> {
    match v? {
        serde_json::Value::String(s) => Decimal::from_str_exact(s).ok(),
        // serde_json::Number may print small values in scientific notation
        // (funding_8h: 3e-07) which `from_str_exact` rejects, while
        // `from_scientific` rejects plain decimals — try both.
        serde_json::Value::Number(n) => {
            let s = n.to_string();
            Decimal::from_str_exact(&s)
                .ok()
                .or_else(|| Decimal::from_scientific(&s).ok())
        }
        _ => None,
    }
}

/// Aggregate the `legs` arrays of `private/get_user_trades_by_order` into one
/// VWAP row per instrument (#119).
///
/// Deribit attaches `legs` to each trade when the order was a combo: every
/// entry is a real per-instrument fill with its own `price`, `amount` and
/// `direction`, tied back to the parent through `combo_id`/`combo_trade_id`.
/// Partial fills appear as several trades, so the legs are size-weighted
/// rather than taken from the first trade. An empty result is an error: the
/// caller must know it has to fall back to attribution.
fn combo_leg_fills_from_response(
    v: &serde_json::Value,
    order_id: &str,
) -> anyhow::Result<Vec<arbitrageur_core::exchange::ComboLegFill>> {
    let trades = v
        .get("trades")
        .and_then(|t| t.as_array())
        .or_else(|| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("Deribit trades response for '{order_id}' has no array"))?;
    // instrument -> (Σ price·amount, Σ amount)
    let mut agg: std::collections::HashMap<String, (Decimal, Decimal)> =
        std::collections::HashMap::new();
    for trade in trades {
        let Some(legs) = trade.get("legs").and_then(|l| l.as_array()) else {
            continue;
        };
        for leg in legs {
            let Some(inst) = leg.get("instrument_name").and_then(|s| s.as_str()) else {
                continue;
            };
            let Some(px) = dec_from_json(leg.get("price")) else {
                continue;
            };
            // A leg's amount is always positive; direction carries the side.
            let amount = dec_from_json(leg.get("amount"))
                .map(|a| a.abs())
                .unwrap_or(dec!(0));
            if px <= dec!(0) || amount <= dec!(0) {
                continue;
            }
            let e = agg.entry(inst.to_string()).or_insert((dec!(0), dec!(0)));
            e.0 += px * amount;
            e.1 += amount;
        }
    }
    if agg.is_empty() {
        anyhow::bail!(
            "Deribit returned no per-leg fills for combo order '{order_id}' (not a combo, or trades not yet visible)"
        );
    }
    let mut out: Vec<arbitrageur_core::exchange::ComboLegFill> = agg
        .into_iter()
        .filter(|(_, (_, amount))| *amount > dec!(0))
        .map(
            |(instrument, (notional, amount))| arbitrageur_core::exchange::ComboLegFill {
                instrument,
                price: notional / amount,
                amount,
            },
        )
        .collect();
    out.sort_by(|a, b| a.instrument.cmp(&b.instrument));
    Ok(out)
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

/// Instrument kind from the name alone: `BTC-27MAR26-100000-C` → option,
/// `BTC-PERPETUAL`/`BTC-27MAR26` → future. Option names always end in
/// `-C`/`-P` after a numeric strike segment. The live path trusts the
/// venue's `kind` field; this name heuristic exists for tests/fixtures.
#[cfg(test)]
fn instrument_kind(name: &str) -> DeribitKind {
    let upper = name.to_ascii_uppercase();
    let last = upper.rsplit('-').next().unwrap_or("");
    if last == "C" || last == "P" {
        DeribitKind::Option
    } else {
        DeribitKind::Future
    }
}

/// Base currency of a Deribit instrument (`BTC-PERPETUAL` → "BTC").
fn instrument_currency(name: &str) -> Option<&str> {
    name.split('-').next().filter(|s| !s.is_empty())
}

fn is_supported_currency(name: &str) -> bool {
    // Instrument names are always `<CCY>-<...>`; a bare currency is not an
    // instrument.
    name.contains('-')
        && instrument_currency(name)
            .map(|c| {
                SUPPORTED_CURRENCIES
                    .iter()
                    .any(|s| c.eq_ignore_ascii_case(s))
            })
            .unwrap_or(false)
}

/// Resolve bare currency into its active perpetual instrument name ("BTC" -> "BTC-PERPETUAL").
pub fn resolve_deribit_instrument(name: &str) -> String {
    let s = name.trim().to_ascii_uppercase();
    if !s.contains('-')
        && SUPPORTED_CURRENCIES
            .iter()
            .any(|c| c.eq_ignore_ascii_case(&s))
    {
        format!("{s}-PERPETUAL")
    } else {
        s
    }
}

impl DeribitAdapter {
    pub fn new(
        base_url: Option<String>,
        client_id: Option<String>,
        client_secret: Option<String>,
    ) -> Self {
        let url = base_url.unwrap_or_else(|| "https://www.deribit.com/api/v2".to_string());
        let venue_id = VenueId::new("deribit");
        Self {
            venue_id,
            client: {
                let mut headers = reqwest::header::HeaderMap::new();
                headers.insert(reqwest::header::ACCEPT, "application/json".parse().unwrap());
                Client::builder()
                    .timeout(std::time::Duration::from_secs(15))
                    .default_headers(headers)
                    .build()
                    .unwrap_or_default()
            },
            base_url: url.trim_end_matches('/').to_string(),
            client_id: RwLock::new(client_id.map(Zeroizing::new)),
            client_secret: RwLock::new(client_secret.map(Zeroizing::new)),
            token: tokio::sync::Mutex::new(None),
            testnet: false,
            rate_limiter: AccountRequestCoordinator::new("deribit", 200, 3000, 500, 15000),
            instruments_cache: tokio::sync::Mutex::new(HashMap::new()),
            account_index_used: RwLock::new(HashMap::new()),
        }
    }

    /// Public testnet adapter — same endpoints under test.deribit.com.
    pub fn for_testnet(client_id: Option<String>, client_secret: Option<String>) -> Self {
        let mut adapter = Self::new(
            Some("https://test.deribit.com/api/v2".to_string()),
            client_id,
            client_secret,
        );
        adapter.testnet = true;
        adapter
    }

    pub fn update_credentials(&self, client_id: Option<String>, client_secret: Option<String>) {
        *self.client_id.write() = client_id.map(Zeroizing::new);
        *self.client_secret.write() = client_secret.map(Zeroizing::new);
        // New credentials invalidate any bearer token minted under the old pair.
        if let Ok(mut t) = self.token.try_lock() {
            *t = None;
        }
    }

    pub fn is_testnet(&self) -> bool {
        self.testnet
    }

    fn credentials(&self) -> anyhow::Result<(String, String)> {
        let id = self.client_id.read().clone();
        let sec = self.client_secret.read().clone();
        match (id, sec) {
            (Some(i), Some(s)) if !i.trim().is_empty() && !s.trim().is_empty() => {
                Ok((i.to_string(), s.to_string()))
            }
            _ => anyhow::bail!("Deribit API credentials are not configured or vault is locked"),
        }
    }

    fn has_credentials(&self) -> bool {
        self.credentials().is_ok()
    }

    /// Unwrap the JSON-RPC envelope: `{result}` on success, `{error:{code,
    /// message}}` fails closed with the venue's own code.
    fn unwrap_envelope(v: serde_json::Value, what: &str) -> anyhow::Result<serde_json::Value> {
        if let Some(err) = v.get("error") {
            let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown");
            let reason = err
                .get("data")
                .and_then(|d| d.get("reason"))
                .and_then(|r| r.as_str())
                .map(|r| format!(" ({r})"))
                .unwrap_or_default();
            anyhow::bail!("Deribit {what} error {code}: {msg}{reason}");
        }
        v.get("result")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Deribit {what} response missing result: {v}"))
    }

    fn is_token_error(v: &serde_json::Value) -> bool {
        v.get("error")
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_i64())
            == Some(13009)
    }

    fn parse_collar_price(err_str: &str) -> Option<Decimal> {
        for key in &["price_too_high", "price_too_low"] {
            if let Some(idx) = err_str.find(key) {
                let after = &err_str[idx + key.len()..];
                let token = after
                    .split(|c: char| {
                        c.is_whitespace() || c == ')' || c == ';' || c == ',' || c == '"'
                    })
                    .find(|s| !s.is_empty())?;
                if let Ok(d) = Decimal::from_str_exact(token) {
                    if d > dec!(0) {
                        return Some(d);
                    }
                }
            }
        }
        None
    }

    async fn get_raw(&self, path: &str, bearer: Option<&str>) -> anyhow::Result<serde_json::Value> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.client.get(&url);
        if let Some(tok) = bearer {
            req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {tok}"));
        }
        let resp = req.send().await.inspect_err(|_| {
            self.rate_limiter.record_failure(false);
        })?;
        if !resp.status().is_success() {
            let is_429 = resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS;
            let status = resp.status().as_u16();
            if status == 400 {
                let text = resp.text().await.unwrap_or_default();
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                    if v.get("error").is_some() {
                        return Ok(v);
                    }
                }
                anyhow::bail!("Deribit HTTP 400 for {path}: {text}");
            }
            self.rate_limiter.record_failure(is_429);
            anyhow::bail!("Deribit HTTP {status} for {path}");
        }
        let text = resp.text().await?;
        serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("Deribit {path} invalid JSON: {e}"))
    }

    /// Public GET — rate-limited and cache-eligible like every other adapter.
    async fn public_get(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        if let Some(cached_str) = self.rate_limiter.acquire_or_cache_path(path, true).await? {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&cached_str) {
                return Self::unwrap_envelope(val, path);
            }
        }
        let v = self.get_raw(path, None).await?;
        Self::unwrap_envelope(v, path)
    }

    /// Private GET — signed-rate-limited, bearer-authenticated, one
    /// re-auth retry on an invalid/expired token.
    async fn private_get(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        self.rate_limiter.acquire_signed().await?;
        let token = self.ensure_token().await?;
        let v = self.get_raw(path, Some(&token)).await?;
        if Self::is_token_error(&v) {
            let token = self.refresh_token(true).await?;
            let v = self.get_raw(path, Some(&token)).await?;
            return Self::unwrap_envelope(v, path);
        }
        Self::unwrap_envelope(v, path)
    }

    async fn post_raw(
        &self,
        path: &str,
        body: &serde_json::Value,
        bearer: Option<&str>,
    ) -> anyhow::Result<serde_json::Value> {
        let url = format!("{}{}", self.base_url, path);
        let method = path.trim_start_matches('/');
        let rpc_payload = if body.get("jsonrpc").is_none() {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": rand::random::<u32>(),
                "method": method,
                "params": body,
            })
        } else {
            body.clone()
        };
        let mut req = self.client.post(&url).json(&rpc_payload);
        if let Some(tok) = bearer {
            req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {tok}"));
        }
        let resp = req.send().await.inspect_err(|_| {
            self.rate_limiter.record_failure(false);
        })?;
        if !resp.status().is_success() {
            let is_429 = resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS;
            let status = resp.status().as_u16();
            if status == 400 {
                let text = resp.text().await.unwrap_or_default();
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                    if v.get("error").is_some() {
                        return Ok(v);
                    }
                }
                anyhow::bail!("Deribit HTTP 400 for {path}: {text}");
            }
            self.rate_limiter.record_failure(is_429);
            anyhow::bail!("Deribit HTTP {status} for {path}");
        }
        let text = resp.text().await?;
        serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("Deribit {path} invalid JSON: {e}"))
    }

    /// Private POST — signed-rate-limited, bearer-authenticated, JSON body.
    pub async fn private_post(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        self.rate_limiter.acquire_signed().await?;
        let token = self.ensure_token().await?;
        let v = self.post_raw(path, body, Some(&token)).await?;
        if Self::is_token_error(&v) {
            let token = self.refresh_token(true).await?;
            let v = self.post_raw(path, body, Some(&token)).await?;
            return Self::unwrap_envelope(v, path);
        }
        Self::unwrap_envelope(v, path)
    }

    /// Fetch a fresh bearer token via client_credentials, or refresh the
    /// existing one. `force` discards the cached token outright.
    async fn refresh_token(&self, force: bool) -> anyhow::Result<String> {
        let (id, secret) = self.credentials()?;
        let mut guard = self.token.lock().await;
        if !force {
            if let Some(t) = guard.as_ref() {
                if std::time::Instant::now() < t.expires_at {
                    return Ok(t.access.to_string());
                }
            }
        }
        // Prefer refresh grant while a refresh token survives.
        let path = if let Some(t) = guard.as_ref() {
            format!(
                "/public/auth?grant_type=refresh_token&refresh_token={}",
                urlencoding(&t.refresh)
            )
        } else {
            format!(
                "/public/auth?grant_type=client_credentials&client_id={}&client_secret={}",
                urlencoding(&id),
                urlencoding(&secret)
            )
        };
        let v = self.get_raw(&path, None).await?;
        let res = match Self::unwrap_envelope(v, "auth") {
            Ok(r) => r,
            Err(e) => {
                // A dead refresh token falls back to a full credential auth
                // once — after that the operator's credentials are wrong.
                if guard.is_some() {
                    *guard = None;
                    let (id, secret) = self.credentials()?;
                    let retry = format!(
                        "/public/auth?grant_type=client_credentials&client_id={}&client_secret={}",
                        urlencoding(&id),
                        urlencoding(&secret)
                    );
                    let v = self.get_raw(&retry, None).await?;
                    Self::unwrap_envelope(v, "auth")?
                } else {
                    return Err(e);
                }
            }
        };
        let access = res
            .get("access_token")
            .and_then(|t| t.as_str())
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Deribit auth returned no access_token: {res}"))?
            .to_string();
        let refresh = res
            .get("refresh_token")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let expires_in = res
            .get("expires_in")
            .and_then(|e| e.as_u64())
            .unwrap_or(300);
        *guard = Some(DeribitToken {
            access: Zeroizing::new(access.clone()),
            refresh: Zeroizing::new(refresh),
            expires_at: std::time::Instant::now()
                + std::time::Duration::from_secs((expires_in * 4) / 5),
        });
        Ok(access)
    }

    async fn ensure_token(&self) -> anyhow::Result<String> {
        self.refresh_token(false).await
    }

    /// Instrument metadata for one name — fetched (and cached 5 min) via
    /// `get_instruments` keyed by the instrument's currency. Both kinds are
    /// cached together so the parity scanner can pair options against the
    /// perpetual without a second fetch.
    async fn instrument_meta(&self, instrument: &str) -> anyhow::Result<InstrumentMeta> {
        let currency = instrument_currency(instrument)
            .ok_or_else(|| anyhow::anyhow!("Deribit instrument '{instrument}' has no currency"))?
            .to_ascii_uppercase();
        let meta = self.currency_instruments(&currency).await?;
        let upper = instrument.to_ascii_uppercase();
        let m = if let Some(m) = meta.get(&upper).cloned() {
            m
        } else {
            let perp = format!("{currency}-PERPETUAL");
            meta.get(&perp).cloned().ok_or_else(|| {
                anyhow::anyhow!("Deribit instrument '{instrument}' not listed or expired")
            })?
        };
        if !m.is_active {
            anyhow::bail!("Deribit instrument '{}' is inactive on venue", m.name);
        }
        Ok(m)
    }

    /// Active option chain for a currency — the parity scanner's pairing
    /// surface (strike, expiry, call/put, tick, min size).
    pub async fn option_chain(
        &self,
        currency: &str,
    ) -> anyhow::Result<Vec<DeribitOptionInstrument>> {
        let ccy = currency.trim().to_ascii_uppercase();
        if !SUPPORTED_CURRENCIES.iter().any(|s| *s == ccy) {
            anyhow::bail!("Deribit unsupported option currency '{currency}'");
        }
        let meta = self.currency_instruments(&ccy).await?;
        let mut out: Vec<DeribitOptionInstrument> = meta
            .iter()
            .filter_map(|(name, m)| {
                if m.kind != DeribitKind::Option || !m.is_active {
                    return None;
                }
                Some(DeribitOptionInstrument {
                    name: name.clone(),
                    strike: m.strike?,
                    option_type: m.option_type?,
                    expiration_ms: m.expiration_ms?,
                    tick_size: m.tick_size,
                    min_amount: m.min_amount,
                    contract_size: m.contract_size,
                    settlement_currency: m.settlement_currency.clone(),
                    is_open: m.is_open,
                })
            })
            .collect();
        out.sort_by(|a, b| {
            a.expiration_ms
                .cmp(&b.expiration_ms)
                .then(a.strike.cmp(&b.strike))
                .then(a.name.cmp(&b.name))
        });
        Ok(out)
    }

    /// Dated futures for a currency — (instrument name, expiration_ms).
    /// Shares the instruments cache with `option_chain`, so the jelly-roll
    /// term-structure benchmark needs no extra round trip.
    pub async fn dated_futures(&self, currency: &str) -> anyhow::Result<Vec<(String, i64)>> {
        let ccy = currency.trim().to_ascii_uppercase();
        if !SUPPORTED_CURRENCIES.iter().any(|s| *s == ccy) {
            anyhow::bail!("Deribit unsupported currency '{currency}'");
        }
        let meta = self.currency_instruments(&ccy).await?;
        let mut out: Vec<(String, i64)> = meta
            .iter()
            .filter_map(|(name, m)| {
                if m.kind != DeribitKind::Future || !m.is_active {
                    return None;
                }
                m.expiration_ms.map(|exp| (name.clone(), exp))
            })
            .collect();
        out.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
        Ok(out)
    }

    /// Create or retrieve an existing combo matching the specified trades/legs.
    pub async fn create_combo(
        &self,
        trades: &[DeribitComboTrade],
    ) -> anyhow::Result<DeribitComboSummary> {
        if trades.len() < 2 || trades.len() > 4 {
            anyhow::bail!(
                "Deribit combo requires between 2 and 4 legs (got {})",
                trades.len()
            );
        }
        let payload = serde_json::json!({
            "trades": trades
        });
        let v = self.private_post("/private/create_combo", &payload).await?;
        let id = v
            .get("id")
            .or_else(|| v.get("instrument_name"))
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow::anyhow!("Deribit create_combo response missing id: {v}"))?;
        let state = v
            .get("state")
            .and_then(|s| s.as_str())
            .unwrap_or("active")
            .to_string();
        let legs = v
            .get("legs")
            .and_then(|l| l.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let inst = item.get("instrument_name")?.as_str()?.to_string();
                        let amt = item.get("amount").and_then(|a| a.as_i64()).unwrap_or(1) as i32;
                        Some(DeribitComboLegInfo {
                            instrument_name: inst,
                            amount: amt,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(DeribitComboSummary {
            id: id.to_string(),
            state,
            legs,
        })
    }

    /// Place an atomic combo order on a Deribit combo instrument created via `create_combo`.
    pub async fn place_combo_instrument_order(
        &self,
        combo_id: &str,
        side: Side,
        amount: Decimal,
        price: Option<Decimal>,
        time_in_force: TimeInForce,
        label: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        let verb = match side {
            Side::Buy => "buy",
            Side::Sell => "sell",
        };
        let mut params = vec![
            ("instrument_name".to_string(), combo_id.to_string()),
            (
                "amount".to_string(),
                amount.round_dp(4).normalize().to_string(),
            ),
        ];
        if let Some(p) = price {
            params.push(("type".into(), "limit".into()));
            params.push(("price".into(), p.normalize().to_string()));
        } else {
            anyhow::bail!("Deribit combo orders require a limit price; market order type is not supported on combo instruments");
        }
        match time_in_force {
            TimeInForce::ImmediateOrCancel => {
                params.push(("time_in_force".into(), "immediate_or_cancel".into()));
            }
            TimeInForce::FillOrKill => {
                params.push(("time_in_force".into(), "fill_or_kill".into()));
            }
            _ => {}
        }
        if let Some(lbl) = label {
            params.push(("label".into(), lbl.to_string()));
        }
        let qs = params
            .iter()
            .map(|(k, v)| format!("{k}={}", urlencoding(v)))
            .collect::<Vec<_>>()
            .join("&");
        let v = self.private_get(&format!("/private/{verb}?{qs}")).await?;
        let o = v.get("order").ok_or_else(|| {
            anyhow::anyhow!("Deribit combo {verb} missing order in response: {v}")
        })?;
        let order_id = o
            .get("order_id")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string();
        let state = o
            .get("order_state")
            .and_then(|s| s.as_str())
            .unwrap_or("open");
        let filled_amount = o
            .get("filled_amount")
            .and_then(|a| a.as_f64())
            .and_then(Decimal::from_f64_retain)
            .unwrap_or(dec!(0));
        let avg_price = o
            .get("average_price")
            .and_then(|p| p.as_f64())
            .and_then(Decimal::from_f64_retain);

        let status = match state {
            "filled" => OrderStatus::Filled,
            "cancelled" => {
                if filled_amount > dec!(0) {
                    OrderStatus::PartiallyFilled
                } else {
                    OrderStatus::Cancelled
                }
            }
            "rejected" => OrderStatus::Rejected,
            _ => OrderStatus::New,
        };

        Ok(OrderResponse {
            order_id,
            client_order_id: label.map(|s| s.to_string()).unwrap_or_default(),
            venue: self.venue_id.clone(),
            symbol: combo_id.to_string(),
            status,
            original_qty: amount,
            filled_qty: filled_amount,
            remaining_qty: (amount - filled_amount).max(dec!(0)),
            avg_fill_price: avg_price,
            fee: Some(dec!(0)),
            fee_currency: "USD".to_string(),
            raw_response: Some(v.to_string()),
            timestamp: Utc::now(),
        })
    }

    /// High-level atomic multi-leg combo execution: creates combo and executes order.
    pub async fn place_combo_order(
        &self,
        trades: &[arbitrageur_core::exchange::ComboLegTrade],
        side: Side,
        amount: Decimal,
        price: Option<Decimal>,
        time_in_force: TimeInForce,
        label: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        let deribit_trades: Vec<DeribitComboTrade> = trades
            .iter()
            .map(|t| DeribitComboTrade {
                instrument_name: t.instrument.clone(),
                amount: t.amount,
                direction: t.direction.clone(),
            })
            .collect();
        let combo = self.create_combo(&deribit_trades).await?;
        self.place_combo_instrument_order(&combo.id, side, amount, price, time_in_force, label)
            .await
    }

    /// Per-instrument fills Deribit recorded for a combo order (#119).
    ///
    /// A combo order acknowledges with one `average_price` for the whole
    /// structure, but `private/get_user_trades_by_order` returns the trades
    /// with a `legs` array — one entry per instrument, each carrying its own
    /// `price`, `amount` and `direction` plus the `combo_id`/`combo_trade_id`
    /// that tie it back to the parent. This aggregates those legs into a VWAP
    /// per instrument across every partial fill of the order.
    ///
    /// Returns an error (never a silently empty result) when the response has
    /// no legs, e.g. the order was not a combo or the trades are not yet
    /// visible; the caller then books its derived attribution instead.
    pub async fn combo_leg_fills(
        &self,
        order_id: &str,
    ) -> anyhow::Result<Vec<arbitrageur_core::exchange::ComboLegFill>> {
        let id = order_id.trim();
        if id.is_empty() {
            anyhow::bail!("Deribit combo_leg_fills needs an order id");
        }
        let v = self
            .private_get(&format!(
                "/private/get_user_trades_by_order?order_id={}",
                urlencoding(id)
            ))
            .await?;
        combo_leg_fills_from_response(&v, id)
    }

    /// Retrieve active combos for a currency.
    pub async fn combos(&self, currency: &str) -> anyhow::Result<Vec<DeribitComboSummary>> {
        let ccy = currency.trim().to_ascii_uppercase();
        let v = self
            .public_get(&format!("/public/get_combos?currency={ccy}"))
            .await?;
        let arr = v.as_array().cloned().unwrap_or_default();
        let mut out = Vec::new();
        for item in arr {
            if let Some(id) = item.get("id").and_then(|s| s.as_str()) {
                let state = item
                    .get("state")
                    .and_then(|s| s.as_str())
                    .unwrap_or("active")
                    .to_string();
                let legs = item
                    .get("legs")
                    .and_then(|l| l.as_array())
                    .map(|legs_arr| {
                        legs_arr
                            .iter()
                            .filter_map(|l| {
                                let inst = l.get("instrument_name")?.as_str()?.to_string();
                                let amt =
                                    l.get("amount").and_then(|a| a.as_i64()).unwrap_or(1) as i32;
                                Some(DeribitComboLegInfo {
                                    instrument_name: inst,
                                    amount: amt,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                out.push(DeribitComboSummary {
                    id: id.to_string(),
                    state,
                    legs,
                })
            }
        }
        Ok(out)
    }

    /// Top-of-book for every instrument of a currency — one call per kind,
    /// giving the parity scanner both option and future quotes without
    /// per-instrument round trips.
    pub async fn book_summaries(&self, currency: &str) -> anyhow::Result<Vec<DeribitBookSummary>> {
        let ccy = currency.trim().to_ascii_uppercase();
        if !SUPPORTED_CURRENCIES.iter().any(|s| *s == ccy) {
            anyhow::bail!("Deribit unsupported currency '{currency}'");
        }
        let mut out = Vec::new();
        for (kind_str, kind) in [
            ("option", DeribitKind::Option),
            ("future", DeribitKind::Future),
        ] {
            let v = self
                .public_get(&format!(
                    "/public/get_book_summary_by_currency?currency={ccy}&kind={kind_str}"
                ))
                .await?;
            for row in v.as_array().cloned().unwrap_or_default() {
                let name = row
                    .get("instrument_name")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_ascii_uppercase();
                if name.is_empty() {
                    continue;
                }
                out.push(DeribitBookSummary {
                    name,
                    kind,
                    bid: dec_from_json(row.get("bid_price")),
                    ask: dec_from_json(row.get("ask_price")),
                    mark: dec_from_json(row.get("mark_price")),
                    underlying_price: dec_from_json(row.get("underlying_price"))
                        .or_else(|| dec_from_json(row.get("estimated_delivery_price"))),
                    open_interest: dec_from_json(row.get("open_interest")),
                    mark_iv: dec_from_json(row.get("mark_iv")),
                    estimated_delivery_price: dec_from_json(row.get("estimated_delivery_price")),
                });
            }
        }
        Ok(out)
    }

    /// Full instrument map for one currency (options + futures, both
    /// fetched and cached together under the 5-minute TTL).
    async fn currency_instruments(
        &self,
        currency: &str,
    ) -> anyhow::Result<HashMap<String, InstrumentMeta>> {
        {
            let cache = self.instruments_cache.lock().await;
            if let Some(c) = cache.get(currency) {
                if c.fetched_at.elapsed() < INSTRUMENT_CACHE_TTL {
                    return Ok(c.meta.clone());
                }
            }
        }
        let mut meta = HashMap::new();
        let mut errs = Vec::new();
        for kind_str in ["option", "future"] {
            let v = match self
                .public_get(&format!(
                    "/public/get_instruments?currency={currency}&kind={kind_str}&expired=false"
                ))
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    errs.push(format!("{kind_str}: {e}"));
                    continue;
                }
            };
            let arr = v
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("Deribit get_instruments missing array"))?;
            for item in arr {
                let name = item
                    .get("instrument_name")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_ascii_uppercase();
                if name.is_empty() {
                    continue;
                }
                let mut steps = Vec::new();
                if let Some(arr) = item.get("tick_size_steps").and_then(|t| t.as_array()) {
                    for s in arr {
                        if let (Some(above), Some(tick)) = (
                            dec_from_json(s.get("above_price")),
                            dec_from_json(s.get("tick_size")),
                        ) {
                            steps.push((above, tick));
                        }
                    }
                }
                steps.sort_by_key(|a| a.0);
                let is_active = item
                    .get("is_active")
                    .and_then(|a| a.as_bool())
                    .unwrap_or(false);
                let is_open = if self.testnet {
                    // Deribit testnet API (test.deribit.com) sets state: "halted" on
                    // instruments even when actively trading/simulated. Only mainnet
                    // reports truthful matching engine states.
                    is_active
                } else {
                    item.get("state")
                        .and_then(|s| s.as_str())
                        .map(|s| s == "open")
                        .unwrap_or(false)
                };
                meta.insert(
                    name.clone(),
                    InstrumentMeta {
                        name,
                        kind: if item.get("kind").and_then(|k| k.as_str()) == Some("option") {
                            DeribitKind::Option
                        } else {
                            DeribitKind::Future
                        },
                        tick_size: dec_from_json(item.get("tick_size")).unwrap_or(dec!(0.0001)),
                        tick_size_steps: steps,
                        min_amount: dec_from_json(item.get("min_trade_amount"))
                            .unwrap_or(dec!(0.1)),
                        contract_size: dec_from_json(item.get("contract_size")).unwrap_or(dec!(1)),
                        settlement_currency: item
                            .get("settlement_currency")
                            .and_then(|s| s.as_str())
                            .unwrap_or(currency)
                            .to_string(),
                        strike: dec_from_json(item.get("strike")),
                        option_type: match item.get("option_type").and_then(|o| o.as_str()) {
                            Some("call") => Some(DeribitOptionType::Call),
                            Some("put") => Some(DeribitOptionType::Put),
                            _ => None,
                        },
                        expiration_ms: item.get("expiration_timestamp").and_then(|t| t.as_i64()),
                        is_active,
                        is_open,
                    },
                );
            }
        }
        if meta.is_empty() && !errs.is_empty() {
            anyhow::bail!("Deribit get_instruments failed: {}", errs.join("; "));
        }
        self.instruments_cache.lock().await.insert(
            currency.to_string(),
            CachedInstruments {
                meta: meta.clone(),
                fetched_at: std::time::Instant::now(),
            },
        );
        Ok(meta)
    }

    /// Tick size for an instrument at a given price (tick_size_steps ladder).
    fn tick_at(meta: &InstrumentMeta, price: Decimal) -> Decimal {
        let mut tick = meta.tick_size;
        for (above, step) in &meta.tick_size_steps {
            if price >= *above {
                tick = *step;
            }
        }
        tick
    }

    /// USD index price for a currency (`btc_usd`) — used to translate
    /// coin-denominated equity/fees into USD for the ledger.
    pub async fn index_price_usd(&self, currency: &str) -> anyhow::Result<Decimal> {
        let v = self
            .public_get(&format!(
                "/public/get_index_price?index_name={}_usd",
                currency.to_ascii_lowercase()
            ))
            .await?;
        let px = dec_from_json(v.get("index_price"))
            .filter(|p| *p > dec!(0))
            .ok_or_else(|| anyhow::anyhow!("Deribit {currency} index price missing"))?;
        self.account_index_used
            .write()
            .insert(currency.to_ascii_uppercase(), px);
        Ok(px)
    }

    /// Current mark price for an instrument (ticker mark_price, USD for
    /// futures, base-coin for options).
    async fn mark_price(&self, instrument: &str) -> anyhow::Result<Decimal> {
        let v = self
            .public_get(&format!("/public/ticker?instrument_name={instrument}"))
            .await?;
        dec_from_json(v.get("mark_price"))
            .filter(|p| *p > dec!(0))
            .ok_or_else(|| anyhow::anyhow!("Deribit {instrument} mark price missing"))
    }

    /// Venue unit conversion: options take `amount` in contracts (≈ base
    /// units / contract_size); futures take USD (`qty_base × ref_price`).
    async fn to_venue_amount(
        &self,
        meta: &InstrumentMeta,
        qty_base: Decimal,
        ref_price_usd: Option<Decimal>,
    ) -> anyhow::Result<Decimal> {
        match meta.kind {
            DeribitKind::Option => {
                let contracts = qty_base / meta.contract_size;
                Ok(contracts)
            }
            DeribitKind::Future => {
                let px = ref_price_usd.filter(|p| *p > dec!(0)).ok_or_else(|| {
                    anyhow::anyhow!("Deribit future order needs a USD reference price")
                })?;
                Ok(qty_base * px)
            }
        }
    }

    /// Inverse of `to_venue_amount` for reported fills.
    fn from_venue_amount(
        meta: &InstrumentMeta,
        amount: Decimal,
        ref_price_usd: Option<Decimal>,
    ) -> Decimal {
        match meta.kind {
            DeribitKind::Option => amount * meta.contract_size,
            DeribitKind::Future => match ref_price_usd {
                Some(p) if p > dec!(0) => amount / p,
                _ => dec!(0),
            },
        }
    }

    fn map_order_state(state: &str, filled: Decimal) -> Option<OrderStatus> {
        match state {
            "filled" => Some(OrderStatus::Filled),
            "cancelled" => Some(OrderStatus::Cancelled),
            "rejected" => Some(OrderStatus::Rejected),
            "open" | "untriggered" => Some(if filled > dec!(0) {
                OrderStatus::PartiallyFilled
            } else {
                OrderStatus::New
            }),
            _ => None,
        }
    }

    /// Shared order-object → OrderResponse mapping, with identity and
    /// quantity validation. `ref_price_usd` converts futures USD amounts
    /// back to base units; options amounts are already base-denominated.
    fn order_response(
        &self,
        o: &serde_json::Value,
        order: &OrderRequest,
        meta: &InstrumentMeta,
        ref_price_usd: Option<Decimal>,
        index_usd: Option<Decimal>,
        raw: &serde_json::Value,
    ) -> anyhow::Result<OrderResponse> {
        let venue_name = o
            .get("instrument_name")
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow::anyhow!("Deribit order response missing instrument_name"))?;
        if !venue_name.eq_ignore_ascii_case(&order.symbol) {
            anyhow::bail!(
                "Deribit order instrument '{venue_name}' does not match requested '{}'",
                order.symbol
            );
        }
        if let Some(label) = o.get("label").and_then(|l| l.as_str()) {
            if !order.client_order_id.is_empty()
                && !label.is_empty()
                && label != order.client_order_id
            {
                anyhow::bail!(
                    "Deribit order label '{label}' does not match client_order_id '{}'",
                    order.client_order_id
                );
            }
        }
        let order_id = o
            .get("order_id")
            .and_then(|s| s.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Deribit order response missing order_id"))?
            .to_string();
        let amount = dec_from_json(o.get("amount")).unwrap_or(dec!(0));
        let filled_v = dec_from_json(o.get("filled_amount")).unwrap_or(dec!(0));
        let avg_px = dec_from_json(o.get("average_price")).filter(|p| *p > dec!(0));
        let filled = Self::from_venue_amount(meta, filled_v, avg_px.or(ref_price_usd));
        let original = Self::from_venue_amount(meta, amount, avg_px.or(ref_price_usd));
        if filled < dec!(0) || original < dec!(0) || filled > original + dec!(0.000001) {
            anyhow::bail!(
                "Deribit order quantity mismatch: filled {filled} of original {original}"
            );
        }
        let state = o
            .get("order_state")
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow::anyhow!("Deribit order missing order_state"))?;
        let status = Self::map_order_state(state, filled_v).ok_or_else(|| {
            anyhow::anyhow!("Deribit unknown order_state '{state}' — failing closed")
        })?;
        // Commission is venue-reported in the settlement currency (BTC for
        // BTC instruments) — convert to USD at the index so the ledger's
        // USD-only fee accounting stays exact instead of unaccounted.
        // Fall back to summing `trades[*].fee` when `order.commission` is zero/absent.
        let raw_comm = dec_from_json(o.get("commission"))
            .filter(|c| *c > dec!(0))
            .or_else(|| {
                let trades = raw.get("trades").and_then(|t| t.as_array())?;
                let sum: Decimal = trades
                    .iter()
                    .filter_map(|tr| dec_from_json(tr.get("fee")))
                    .filter(|f| *f > dec!(0))
                    .sum();
                (sum > dec!(0)).then_some(sum)
            })
            .or_else(|| dec_from_json(o.get("commission")));
        let fee = raw_comm.and_then(|c| {
            if c < dec!(0) {
                return None;
            }
            match meta.settlement_currency.as_str() {
                "USD" | "USDC" | "USDT" => Some(c),
                _ => index_usd.map(|i| c * i),
            }
        });
        Ok(OrderResponse {
            order_id,
            client_order_id: order.client_order_id.clone(),
            venue: self.venue_id.clone(),
            symbol: order.symbol.clone(),
            status,
            original_qty: original,
            filled_qty: filled,
            remaining_qty: (original - filled).max(dec!(0)),
            avg_fill_price: avg_px,
            fee,
            fee_currency: "USD".to_string(),
            raw_response: Some(raw.to_string()),
            timestamp: Utc::now(),
        })
    }
}

fn urlencoding(s: &str) -> String {
    // Minimal percent-encoding for credential params (alphanumerics +
    // unreserved pass through; everything else is %-encoded).
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[async_trait]
impl ExchangeAdapter for DeribitAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        if self.testnet {
            "Deribit Testnet"
        } else {
            "Deribit"
        }
    }

    fn rest_base_url(&self) -> Option<String> {
        Some(self.base_url.clone())
    }

    fn supports_reduce_only(&self, symbol: &str) -> bool {
        let s = symbol.trim().to_ascii_uppercase();
        !s.ends_with("-C") && !s.ends_with("-P")
    }

    async fn ping(&self) -> anyhow::Result<()> {
        self.public_get("/public/get_time").await?;
        Ok(())
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let mut out = Vec::new();
        let mut errs = Vec::new();
        for currency in SUPPORTED_CURRENCIES {
            for kind in ["option", "future"] {
                match self
                    .public_get(&format!(
                        "/public/get_instruments?currency={currency}&kind={kind}&expired=false"
                    ))
                    .await
                {
                    Ok(v) => {
                        for item in v.as_array().cloned().unwrap_or_default() {
                            let name = item
                                .get("instrument_name")
                                .and_then(|s| s.as_str())
                                .unwrap_or("");
                            let is_closed = item
                                .get("state")
                                .and_then(|s| s.as_str())
                                .map(|s| s == "closed" || s == "cancelled")
                                .unwrap_or(false);
                            if name.is_empty()
                                || !item
                                    .get("is_active")
                                    .and_then(|a| a.as_bool())
                                    .unwrap_or(false)
                                || is_closed
                            {
                                continue;
                            }
                            let tick = dec_from_json(item.get("tick_size")).unwrap_or(dec!(0.0001));
                            let min_amt =
                                dec_from_json(item.get("min_trade_amount")).unwrap_or(dec!(0.1));
                            let price_dec = {
                                let s = tick.normalize().to_string();
                                s.split('.').nth(1).map(|d| d.len() as u32).unwrap_or(0)
                            };
                            let size_dec = {
                                let s = min_amt.normalize().to_string();
                                s.split('.').nth(1).map(|d| d.len() as u32).unwrap_or(0)
                            };
                            let taker = dec_from_json(item.get("taker_commission"))
                                .unwrap_or(dec!(0.0003))
                                * dec!(10000);
                            let maker = dec_from_json(item.get("maker_commission"))
                                .unwrap_or(dec!(0.0003))
                                * dec!(10000);
                            let symbol = match kind {
                                "option" => Symbol::crypto_option(name, currency),
                                _ if name.ends_with("-PERPETUAL") => Symbol::perp(currency, "USD"),
                                _ => Symbol::new(name, "USD", MarketType::DatedFutures),
                            };
                            out.push(MarketMetadata {
                                symbol,
                                venue: self.venue_id.clone(),
                                venue_symbol: name.to_string(),
                                price_decimals: price_dec,
                                size_decimals: size_dec,
                                min_order_size: min_amt,
                                min_notional: dec!(0),
                                maker_fee_bps: maker,
                                taker_fee_bps: taker,
                                contract_size: dec_from_json(item.get("contract_size"))
                                    .unwrap_or(dec!(1)),
                                is_active: true,
                            });
                        }
                    }
                    Err(e) => errs.push(format!("{currency}/{kind}: {e}")),
                }
            }
        }
        if out.is_empty() && !errs.is_empty() {
            anyhow::bail!("Deribit get_instruments failed: {}", errs.join("; "));
        }
        Ok(out)
    }

    async fn fetch_market_prices(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        // Book summaries carry mark prices for every instrument — bulk fetch
        // per currency+kind to seed dynamic sizing/filters.
        let mut map = HashMap::new();
        for currency in SUPPORTED_CURRENCIES {
            // Options mark in base coin — one index lookup per currency
            // converts the whole chain to USD notional marks.
            let index = match self.index_price_usd(currency).await {
                Ok(i) => Some(i),
                Err(e) => {
                    tracing::warn!("Deribit index price failed for {currency}: {e}");
                    None
                }
            };
            for kind in ["option", "future"] {
                let v = self
                    .public_get(&format!(
                        "/public/get_book_summary_by_currency?currency={currency}&kind={kind}"
                    ))
                    .await?;
                for row in v.as_array().cloned().unwrap_or_default() {
                    if let (Some(name), Some(mark)) = (
                        row.get("instrument_name").and_then(|s| s.as_str()),
                        dec_from_json(row.get("mark_price")),
                    ) {
                        let usd = if kind == "option" {
                            match index {
                                Some(i) => mark * i,
                                None => continue,
                            }
                        } else {
                            mark
                        };
                        map.insert(name.to_string(), usd);
                    }
                }
            }
        }
        Ok(map)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let instrument = resolve_deribit_instrument(symbol);
        if !is_supported_currency(&instrument) {
            anyhow::bail!("Deribit orderbook unsupported instrument '{symbol}'");
        }
        let v = self
            .public_get(&format!(
                "/public/get_order_book?instrument_name={instrument}&depth=20"
            ))
            .await?;
        let mut book = OrderBook::new(self.venue_id.clone(), instrument.clone());
        let bids = v
            .get("bids")
            .and_then(|b| b.as_array())
            .cloned()
            .unwrap_or_default();
        let asks = v
            .get("asks")
            .and_then(|a| a.as_array())
            .cloned()
            .unwrap_or_default();
        for lvl in &bids {
            if let Some((p, q)) = parse_pair(lvl) {
                book.bids.push(BookLevel::new(p, q));
            }
        }
        for lvl in &asks {
            if let Some((p, q)) = parse_pair(lvl) {
                book.asks.push(BookLevel::new(p, q));
            }
        }
        book.sequence = v.get("change_id").and_then(|c| c.as_u64()).unwrap_or(0);
        // The venue's model mark rides the same response — keep it for
        // valuation so one-sided option books (bids-empty far strikes)
        // still carry the venue's own price instead of no mark at all.
        // A zero mark is real data for a dead option but a broken feed on
        // a future — accept `0` only when the instrument is an option.
        let is_option = matches!(instrument.rsplit('-').next(), Some("C") | Some("P"));
        book.mark_price = dec_from_json(v.get("mark_price"))
            .filter(|m| *m > dec!(0) || (is_option && *m >= dec!(0)));
        Ok(book)
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let instrument = resolve_deribit_instrument(symbol);
        let v = self
            .public_get(&format!("/public/ticker?instrument_name={instrument}"))
            .await?;
        let bid = dec_from_json(v.get("best_bid_price")).unwrap_or(dec!(0));
        let ask = dec_from_json(v.get("best_ask_price")).unwrap_or(dec!(0));
        let last = dec_from_json(v.get("last_price"))
            .or_else(|| dec_from_json(v.get("mark_price")))
            .unwrap_or(dec!(0));
        Ok(Ticker {
            venue: self.venue_id.clone(),
            symbol: instrument,
            bid,
            ask,
            last,
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate> {
        let instrument = resolve_deribit_instrument(symbol);
        if !instrument.ends_with("-PERPETUAL") {
            anyhow::bail!("Deribit funding rate only exists for perpetuals, not '{symbol}'");
        }
        let v = self
            .public_get(&format!("/public/ticker?instrument_name={instrument}"))
            .await?;
        let rate = dec_from_json(v.get("funding_8h"))
            .ok_or_else(|| anyhow::anyhow!("Deribit ticker missing funding_8h for {instrument}"))?;
        Ok(FundingRate {
            venue: self.venue_id.clone(),
            symbol: instrument,
            rate,
            predicted_rate: dec_from_json(v.get("current_funding")),
            interval_hours: 8,
            next_funding_time: Utc::now(),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rates(&self) -> anyhow::Result<Vec<FundingRate>> {
        let mut out = Vec::new();
        for ccy in SUPPORTED_CURRENCIES {
            match self.fetch_funding_rate(&format!("{ccy}-PERPETUAL")).await {
                Ok(f) => out.push(f),
                Err(e) => {
                    tracing::warn!("Deribit funding fetch failed for {ccy}-PERPETUAL: {e}")
                }
            }
        }
        Ok(out)
    }

    async fn index_price_usd(&self, currency: &str) -> anyhow::Result<Decimal> {
        DeribitAdapter::index_price_usd(self, currency).await
    }

    fn last_account_index_usd(&self, currency: &str) -> Option<Decimal> {
        self.account_index_used.read().get(currency).copied()
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        if !self.has_credentials() {
            anyhow::bail!(
                "Cannot fetch Deribit account: API credentials are not configured or vault is locked"
            );
        }
        let mut equity_usd = dec!(0);
        let mut maint_usd = dec!(0);
        let mut free_usd = dec!(0);
        let mut balances = Vec::new();
        let mut positions = Vec::new();
        let mut staged_indices = Vec::new();
        for ccy in SUPPORTED_CURRENCIES {
            let idx = self.index_price_usd(ccy).await?;
            staged_indices.push((ccy.to_string(), idx));
            let summary = self
                .private_get(&format!(
                    "/private/get_account_summary?currency={ccy}&extended=true"
                ))
                .await?;
            let eq = dec_from_json(summary.get("equity")).unwrap_or(dec!(0));
            let avail = dec_from_json(summary.get("available_funds"))
                .or_else(|| dec_from_json(summary.get("available_withdrawal_funds")))
                .unwrap_or(dec!(0));
            let maint = dec_from_json(summary.get("maintenance_margin")).unwrap_or(dec!(0));
            let _bal = dec_from_json(summary.get("balance")).unwrap_or(eq);
            equity_usd += eq * idx;
            maint_usd += maint * idx;
            free_usd += avail * idx;
            balances.push(Balance {
                venue: self.venue_id.clone(),
                currency: ccy.to_string(),
                // Use coin-denominated `eq` (which includes option market value
                // and futures session UPL) so `breaker_frozen_equity_usd`
                // freezes the exact coin notional backing `equity_usd`.
                total: eq,
                free: avail,
                locked: (eq - avail).max(dec!(0)),
            });
            for kind in ["future", "option"] {
                let rows = self
                    .private_get(&format!(
                        "/private/get_positions?currency={ccy}&kind={kind}"
                    ))
                    .await?;
                for p in rows.as_array().cloned().unwrap_or_default() {
                    let dir = p.get("direction").and_then(|d| d.as_str()).unwrap_or("");
                    // Deribit reports short-position `size` as NEGATIVE —
                    // `size <= 0` here would drop every short option/future
                    // and read the account as flat on live exposure.
                    let size = dec_from_json(p.get("size")).unwrap_or(dec!(0)).abs();
                    if dir == "zero" || size == dec!(0) {
                        continue;
                    }
                    let name = p
                        .get("instrument_name")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_ascii_uppercase();
                    if name.is_empty() {
                        continue;
                    }
                    let meta = self.instrument_meta(&name).await?;
                    let mark = dec_from_json(p.get("mark_price")).unwrap_or(dec!(0));
                    let entry = dec_from_json(p.get("average_price")).unwrap_or(dec!(0));
                    let fpl = dec_from_json(p.get("floating_profit_loss")).unwrap_or(dec!(0));
                    let im = dec_from_json(p.get("initial_margin")).unwrap_or(dec!(0));
                    let mm = dec_from_json(p.get("maintenance_margin")).unwrap_or(dec!(0));
                    let liq = dec_from_json(p.get("estimated_liquidation_price"));
                    // Futures report `size` in USD; options in contracts of
                    // `contract_size` base. Normalize both to base units;
                    // option marks/entries convert to USD at the index so
                    // notional math stays in USD.
                    let (qty, mark_usd, entry_usd) = match meta.kind {
                        DeribitKind::Future => {
                            let qty = if mark > dec!(0) { size / mark } else { dec!(0) };
                            (qty, mark, entry)
                        }
                        DeribitKind::Option => (size * meta.contract_size, mark * idx, entry * idx),
                    };
                    positions.push(Position {
                        venue: self.venue_id.clone(),
                        symbol: name,
                        side: if dir == "buy" { Side::Buy } else { Side::Sell },
                        quantity: qty,
                        entry_price: entry_usd,
                        mark_price: mark_usd,
                        unrealized_pnl: fpl * idx,
                        realized_pnl: dec!(0),
                        liquidation_price: liq,
                        leverage: dec_from_json(p.get("leverage")).unwrap_or(dec!(1)),
                        initial_margin: im * idx,
                        maintenance_margin: mm * idx,
                        updated_at: Utc::now(),
                    });
                }
            }
        }
        {
            let mut idx_map = self.account_index_used.write();
            for (ccy, idx) in staged_indices {
                idx_map.insert(ccy, idx);
            }
        }
        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity: equity_usd,
            free_margin: free_usd,
            maintenance_margin: maint_usd,
            margin_usage_pct: if equity_usd > dec!(0) {
                maint_usd / equity_usd * dec!(100)
            } else {
                dec!(0)
            },
            balances,
            positions,
            timestamp: Utc::now(),
        })
    }

    fn normalize_account_position(&self, position: &Position) -> anyhow::Result<Option<Position>> {
        let mut p = position.clone();
        p.symbol = p.symbol.to_ascii_uppercase();
        if !is_supported_currency(&p.symbol) {
            return Ok(None);
        }
        Ok(Some(p))
    }

    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        if !self.has_credentials() {
            anyhow::bail!(
                "Cannot place live order on Deribit: API credentials are not configured or vault is locked"
            );
        }
        if order.quantity <= dec!(0) {
            anyhow::bail!("Deribit order quantity must be positive");
        }
        let instrument = order.symbol.trim().to_ascii_uppercase();
        if !is_supported_currency(&instrument) {
            anyhow::bail!("Deribit unsupported instrument '{instrument}'");
        }
        let meta = self.instrument_meta(&instrument).await?;
        if !meta.is_open {
            anyhow::bail!(
                "Cannot place order on Deribit: instrument '{instrument}' is currently {} on venue matching engine (orders rejected by venue)",
                if meta.is_active { "halted" } else { "inactive" }
            );
        }
        let currency = instrument_currency(&instrument)
            .unwrap_or("BTC")
            .to_string();
        let index = self.index_price_usd(&currency).await.ok();

        // Reference USD price for futures sizing: ALWAYS the venue mark, not
        // the order's limit price. USD amount = qty_base × px determines the
        // base delivered (fills at fill_px convert amount/fill_px), so pegging
        // the conversion to an aggressive limit systematically over-fills —
        // observed ~0.6% long-side overshoot (qty 1.0 → 1.006 base) that then
        // strands as unexplained residual at close.
        let ref_price_usd = match meta.kind {
            DeribitKind::Future => Some(self.mark_price(&instrument).await?),
            DeribitKind::Option => index,
        };
        let mut amount = self
            .to_venue_amount(&meta, order.quantity, ref_price_usd)
            .await?;
        let lot = meta.min_amount.max(dec!(0.00000001));
        if order.reduce_only {
            amount = (amount / lot).round() * lot;
        } else {
            let ratio = amount / lot;
            let nearest = ratio.round();
            if (ratio - nearest).abs() <= dec!(0.05) {
                amount = nearest * lot;
            } else {
                amount = ratio.floor() * lot;
            }
        }
        if amount < meta.min_amount {
            anyhow::bail!(
                "Deribit order amount {amount} below min {} for {instrument}",
                meta.min_amount
            );
        }

        // Wire policy: every semantic maps explicitly or refuses.
        let mut params: Vec<(String, String)> = vec![
            ("instrument_name".into(), instrument.clone()),
            ("amount".into(), amount.normalize().to_string()),
            ("label".into(), order.client_order_id.clone()),
        ];
        let type_str = match order.order_type {
            OrderType::Market => {
                if order.time_in_force != TimeInForce::GoodTilCancelled
                    && order.time_in_force != TimeInForce::ImmediateOrCancel
                {
                    anyhow::bail!(
                        "Deribit market order cannot express {:?} — refusing instead of downgrading",
                        order.time_in_force
                    );
                }
                "market"
            }
            OrderType::ImmediateOrCancel => {
                params.push(("time_in_force".into(), "immediate_or_cancel".into()));
                "limit"
            }
            OrderType::PostOnly => {
                params.push(("post_only".into(), "true".into()));
                "limit"
            }
            OrderType::Limit => {
                match order.time_in_force {
                    TimeInForce::GoodTilCancelled => {}
                    TimeInForce::ImmediateOrCancel => {
                        params.push(("time_in_force".into(), "immediate_or_cancel".into()))
                    }
                    TimeInForce::FillOrKill => {
                        params.push(("time_in_force".into(), "fill_or_kill".into()))
                    }
                    TimeInForce::PostOnly => params.push(("post_only".into(), "true".into())),
                }
                if order.post_only {
                    params.push(("post_only".into(), "true".into()));
                }
                "limit"
            }
        };
        params.push(("type".into(), type_str.to_string()));

        if type_str == "limit" {
            let px = order
                .price
                .filter(|p| *p > dec!(0))
                .ok_or_else(|| anyhow::anyhow!("Deribit limit order requires a positive price"))?;
            // Tick rounding never crosses the caller's bound: floor for
            // buys, ceil for sells.
            let tick = Self::tick_at(&meta, px);
            let rounded = match order.side {
                Side::Buy => (px / tick).floor() * tick,
                Side::Sell => (px / tick).ceil() * tick,
            };
            params.push(("price".into(), rounded.normalize().to_string()));
        }
        let is_option = matches!(meta.kind, DeribitKind::Option);
        if order.reduce_only && !is_option {
            params.push(("reduce_only".into(), "true".into()));
        }
        let qs = params
            .iter()
            .map(|(k, v)| format!("{k}={}", urlencoding(v)))
            .collect::<Vec<_>>()
            .join("&");
        let verb = match order.side {
            Side::Buy => "buy",
            Side::Sell => "sell",
        };
        let v = match self.private_get(&format!("/private/{verb}?{qs}")).await {
            Ok(v) => v,
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("10031") || err_str.contains("invalid_args_for_instrument") {
                    anyhow::bail!(
                        "Deribit {verb} rejected ({instrument}): {err_str} (instrument may be halted or index locked by venue)"
                    );
                }
                let is_ioc = order.order_type == OrderType::ImmediateOrCancel
                    || order.time_in_force == TimeInForce::ImmediateOrCancel;
                let is_collar_retryable = is_ioc
                    || order.reduce_only
                    || matches!(order.side, Side::Sell if err_str.contains("price_too_low"))
                    || matches!(order.side, Side::Buy if err_str.contains("price_too_high"));
                if is_collar_retryable
                    && (err_str.contains("price_too_high") || err_str.contains("price_too_low"))
                {
                    if let Some(collar_px) = Self::parse_collar_price(&err_str) {
                        let can_clamp = match order.side {
                            Side::Buy => {
                                order.price.is_some_and(|p| collar_px <= p)
                                    || order.reduce_only
                                    || is_ioc
                            }
                            Side::Sell => {
                                order.price.is_some_and(|p| collar_px >= p)
                                    || order.reduce_only
                                    || is_ioc
                            }
                        };
                        if can_clamp {
                            let tick = Self::tick_at(&meta, collar_px);
                            let rounded = match order.side {
                                Side::Buy => (collar_px / tick).floor() * tick,
                                Side::Sell => (collar_px / tick).ceil() * tick,
                            };
                            let mut retry_params = params.clone();
                            if let Some(p) = retry_params.iter_mut().find(|(k, _)| k == "price") {
                                p.1 = rounded.normalize().to_string();
                            }
                            let retry_qs = retry_params
                                .iter()
                                .map(|(k, v)| format!("{k}={}", urlencoding(v)))
                                .collect::<Vec<_>>()
                                .join("&");
                            tracing::info!(
                                "Deribit {verb} for {instrument}: clamped price from {:?} to {rounded} on venue collar ({err_str}); retrying",
                                order.price
                            );
                            self.private_get(&format!("/private/{verb}?{retry_qs}"))
                                .await?
                        } else {
                            return Err(e);
                        }
                    } else {
                        return Err(e);
                    }
                } else {
                    return Err(e);
                }
            }
        };
        let o = v
            .get("order")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Deribit {verb} missing order object: {v}"))?;
        self.order_response(&o, &order, &meta, ref_price_usd, index, &v)
    }

    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
        let order_id = req
            .order_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Deribit cancel requires a venue order_id"))?;
        let v = match self
            .private_get(&format!("/private/cancel?order_id={order_id}"))
            .await
        {
            Ok(v) => v,
            Err(e) => {
                let err_msg = e.to_string();
                if err_msg.contains("not_found")
                    || err_msg.contains("already_closed")
                    || err_msg.contains("10004")
                    || err_msg.contains("11044")
                    || err_msg.contains("HTTP 400")
                {
                    tracing::info!(
                        "Deribit order {order_id} already dead or not found on venue ({err_msg}) — treating as cancelled"
                    );
                    return Ok(());
                }
                return Err(e);
            }
        };
        // The cancel response is the order object — confirm terminal state
        // through venue truth rather than trusting the ack.
        let state = v
            .get("order_state")
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow::anyhow!("Deribit cancel response missing order_state"))?;
        match state {
            "cancelled" | "filled" => Ok(()),
            other => anyhow::bail!(
                "Deribit cancel returned order_state '{other}' — order may still be working"
            ),
        }
    }

    async fn fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        let instrument = symbol.trim().to_ascii_uppercase();
        let meta = self.instrument_meta(&instrument).await?;
        let currency = instrument_currency(&instrument)
            .unwrap_or("BTC")
            .to_string();
        let index = self.index_price_usd(&currency).await.ok();
        let v = self
            .private_get(&format!(
                "/private/get_order_state?order_id={}",
                urlencoding(order_id)
            ))
            .await?;
        // Reject stale-cache ambiguity: the returned order must carry the
        // queried id and requested instrument.
        let returned_id = v.get("order_id").and_then(|s| s.as_str()).unwrap_or("");
        if returned_id != order_id {
            anyhow::bail!(
                "Deribit get_order_state returned order '{returned_id}' for query '{order_id}'"
            );
        }
        let req = OrderRequest {
            client_order_id: client_order_id.unwrap_or("").to_string(),
            venue: self.venue_id.clone(),
            symbol: instrument.clone(),
            side: match v.get("direction").and_then(|d| d.as_str()) {
                Some("sell") => Side::Sell,
                _ => Side::Buy,
            },
            order_type: OrderType::Limit,
            price: dec_from_json(v.get("price")),
            quantity: dec!(0),
            time_in_force: TimeInForce::GoodTilCancelled,
            post_only: false,
            reduce_only: false,
            created_at: Utc::now(),
        };
        let avg = dec_from_json(v.get("average_price")).filter(|p| *p > dec!(0));
        let ref_px = if meta.kind == DeribitKind::Future {
            avg.or_else(|| dec_from_json(v.get("price"))).or(index)
        } else {
            index
        };
        let mut raw_v = v.clone();
        if dec_from_json(v.get("commission")).unwrap_or(dec!(0)) <= dec!(0) {
            if let Ok(t_res) = self
                .private_get(&format!(
                    "/private/get_user_trades_by_order?order_id={}",
                    urlencoding(order_id)
                ))
                .await
            {
                if let Some(trades) = t_res
                    .get("trades")
                    .or_else(|| t_res.as_array().is_some().then_some(&t_res))
                {
                    if let Some(obj) = raw_v.as_object_mut() {
                        obj.insert("trades".into(), trades.clone());
                    }
                }
            }
        }
        self.order_response(&v, &req, &meta, ref_px, index, &raw_v)
    }

    /// Snap an engine quantity (base units) to the instrument's wire grid:
    /// options floor to `min_amount` contract lots (×`contract_size` back
    /// to base); futures floor the USD amount to its lot and convert back
    /// at the venue mark.
    async fn wire_base_qty(&self, symbol: &str, qty: Decimal) -> anyhow::Result<Decimal> {
        let instrument = symbol.trim().to_ascii_uppercase();
        let meta = self.instrument_meta(&instrument).await?;
        let ref_px = match meta.kind {
            DeribitKind::Option => None,
            DeribitKind::Future => Some(self.mark_price(&meta.name).await?),
        };
        let amount = self.to_venue_amount(&meta, qty, ref_px).await?;
        let lot = meta.min_amount.max(dec!(0.00000001));
        let ratio = amount / lot;
        let nearest = ratio.round();
        if nearest > dec!(0) && (ratio - nearest).abs() <= dec!(0.05) {
            // qty already maps to `nearest * lot` contracts within venue tolerance;
            // retain the caller's exact base quantity so it aligns with linear orderbook grids
            // rather than dividing `snapped / ref_px` into an irrational float residual.
            return Ok(qty);
        }
        let snapped = ratio.floor() * lot;
        if snapped <= dec!(0) {
            return Ok(dec!(0));
        }
        Ok(Self::from_venue_amount(&meta, snapped, ref_px))
    }

    fn supports_live_execution(&self) -> bool {
        true
    }

    fn live_order_submission_implemented(&self) -> bool {
        true
    }

    fn order_status_polling_implemented(&self) -> bool {
        true
    }

    fn has_live_credentials(&self) -> bool {
        // Env-agnostic like every other adapter: live_* and tn_* instances
        // are distinct objects already scoped by env, so a testnet adapter's
        // loaded credentials ARE the right answer. The previous `&& !testnet`
        // permanently marked testnet Deribit as uncredentialed to every
        // credential gate — account reads, capability reports, dispatch.
        self.has_credentials()
    }

    fn supports_combo_orders(&self) -> bool {
        true
    }

    async fn place_combo_order(
        &self,
        trades: &[arbitrageur_core::exchange::ComboLegTrade],
        side: Side,
        amount: Decimal,
        price: Option<Decimal>,
        time_in_force: TimeInForce,
        label: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        self.place_combo_order(trades, side, amount, price, time_in_force, label)
            .await
    }

    async fn combo_leg_fills(
        &self,
        order_id: &str,
    ) -> anyhow::Result<Vec<arbitrageur_core::exchange::ComboLegFill>> {
        // Fully qualified: the inherent method has the same name, and a
        // mistyped delegation here would recurse through the trait instead.
        DeribitAdapter::combo_leg_fills(self, order_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter() -> DeribitAdapter {
        DeribitAdapter::for_testnet(None, None)
    }

    #[test]
    fn instrument_kind_parses_names() {
        assert_eq!(instrument_kind("BTC-PERPETUAL"), DeribitKind::Future);
        assert_eq!(instrument_kind("BTC-27MAR26"), DeribitKind::Future);
        assert_eq!(instrument_kind("BTC-27MAR26-100000-C"), DeribitKind::Option);
        assert_eq!(instrument_kind("ETH-27MAR26-4000-P"), DeribitKind::Option);
    }

    #[test]
    fn currency_parse_and_gate() {
        assert_eq!(instrument_currency("BTC-27MAR26-100000-C"), Some("BTC"));
        assert!(is_supported_currency("ETH-PERPETUAL"));
        assert!(!is_supported_currency("SOL-PERPETUAL"));
        assert!(!is_supported_currency("BTC"));
    }

    #[test]
    fn order_state_maps_fail_closed() {
        assert_eq!(
            DeribitAdapter::map_order_state("filled", dec!(0)),
            Some(OrderStatus::Filled)
        );
        assert_eq!(
            DeribitAdapter::map_order_state("open", dec!(0)),
            Some(OrderStatus::New)
        );
        assert_eq!(
            DeribitAdapter::map_order_state("open", dec!(0.4)),
            Some(OrderStatus::PartiallyFilled)
        );
        assert_eq!(
            DeribitAdapter::map_order_state("cancelled", dec!(0)),
            Some(OrderStatus::Cancelled)
        );
        assert_eq!(DeribitAdapter::map_order_state("bogus", dec!(0)), None);
    }

    fn option_meta() -> InstrumentMeta {
        InstrumentMeta {
            name: "BTC-25SEP26-100000-C".into(),
            kind: DeribitKind::Option,
            tick_size: dec!(0.0001),
            tick_size_steps: vec![(dec!(0.005), dec!(0.0005))],
            min_amount: dec!(0.1),
            contract_size: dec!(1),
            settlement_currency: "BTC".into(),
            strike: Some(dec!(100000)),
            option_type: Some(DeribitOptionType::Call),
            expiration_ms: Some(1_790_323_200_000),
            is_active: true,
            is_open: true,
        }
    }

    #[test]
    fn order_response_validates_identity_and_quantities() {
        let a = adapter();
        let order = OrderRequest::new_limit(
            VenueId::new("deribit"),
            "BTC-25SEP26-100000-C",
            Side::Buy,
            dec!(0.02),
            dec!(0.5),
        );
        let meta = option_meta();
        let raw = serde_json::json!({"ok": true});
        let good = serde_json::json!({
            "instrument_name": "BTC-25SEP26-100000-C",
            "order_id": "12345",
            "label": order.client_order_id,
            "order_state": "open",
            "direction": "buy",
            "amount": 0.5,
            "filled_amount": 0.0,
            "price": 0.02,
            "commission": 0.0
        });
        let resp = a
            .order_response(
                &good,
                &order,
                &meta,
                Some(dec!(84500)),
                Some(dec!(84500)),
                &raw,
            )
            .unwrap();
        assert_eq!(resp.status, OrderStatus::New);
        assert_eq!(resp.original_qty, dec!(0.5));

        // Instrument mismatch refuses.
        let mut bad = good.clone();
        bad["instrument_name"] = serde_json::json!("BTC-25SEP26-90000-C");
        assert!(a
            .order_response(
                &bad,
                &order,
                &meta,
                Some(dec!(84500)),
                Some(dec!(84500)),
                &raw
            )
            .is_err());

        // Label mismatch refuses.
        let mut bad = good.clone();
        bad["label"] = serde_json::json!("other-label");
        assert!(a
            .order_response(
                &bad,
                &order,
                &meta,
                Some(dec!(84500)),
                Some(dec!(84500)),
                &raw
            )
            .is_err());

        // BTC-denominated commission converts to USD at the index.
        let mut with_fee = good.clone();
        with_fee["commission"] = serde_json::json!(0.00002);
        with_fee["order_state"] = serde_json::json!("filled");
        with_fee["filled_amount"] = serde_json::json!(0.5);
        with_fee["average_price"] = serde_json::json!(0.019);
        let resp = a
            .order_response(
                &with_fee,
                &order,
                &meta,
                Some(dec!(84500)),
                Some(dec!(84500)),
                &raw,
            )
            .unwrap();
        assert_eq!(resp.status, OrderStatus::Filled);
        assert_eq!(resp.fee_currency, "USD");
        let fee = resp.fee.unwrap();
        assert!((fee - dec!(1.69)).abs() < dec!(0.01), "fee={fee}");
    }

    #[test]
    fn tick_ladder_selects_step() {
        let m = option_meta();
        assert_eq!(DeribitAdapter::tick_at(&m, dec!(0.001)), dec!(0.0001));
        assert_eq!(DeribitAdapter::tick_at(&m, dec!(0.02)), dec!(0.0005));
    }

    #[test]
    fn envelope_unwrap_fails_closed_on_error() {
        let err = serde_json::json!({"jsonrpc":"2.0","error":{"code":13004,"message":"invalid_credentials"}});
        assert!(DeribitAdapter::unwrap_envelope(err, "auth").is_err());
        let missing = serde_json::json!({"jsonrpc":"2.0"});
        assert!(DeribitAdapter::unwrap_envelope(missing, "x").is_err());
    }

    #[test]
    fn capabilities_are_live_ready() {
        let a = adapter();
        assert!(a.live_order_submission_implemented());
        assert!(a.order_status_polling_implemented());
        assert!(!a.has_live_credentials()); // no creds loaded
    }

    #[test]
    fn url_encoding_encodes_reserved() {
        assert_eq!(urlencoding("a+b/c=d"), "a%2Bb%2Fc%3Dd");
        assert_eq!(urlencoding("plain_1.2-~"), "plain_1.2-~");
    }

    #[test]
    fn testnet_deribit_considers_active_instruments_open() {
        let tn = DeribitAdapter::for_testnet(None, None);
        assert!(tn.is_testnet());
        let prod = DeribitAdapter::new(None, None, None);
        assert!(!prod.is_testnet());
    }

    #[test]
    fn parse_collar_price_extracts_bounds() {
        let err_high =
            "Deribit /private/buy?price=0.2395 error 10007: price_too_high 0.2385; residuals";
        assert_eq!(
            DeribitAdapter::parse_collar_price(err_high),
            Some(dec!(0.2385))
        );

        let err_low = "Deribit /private/sell?price=0.0100 error 10007: price_too_low 0.0125";
        assert_eq!(
            DeribitAdapter::parse_collar_price(err_low),
            Some(dec!(0.0125))
        );

        let err_none = "Deribit /private/buy error 10004: not_found";
        assert_eq!(DeribitAdapter::parse_collar_price(err_none), None);
    }

    #[test]
    fn test_resolve_deribit_instrument() {
        assert_eq!(resolve_deribit_instrument("BTC"), "BTC-PERPETUAL");
        assert_eq!(resolve_deribit_instrument("btc"), "BTC-PERPETUAL");
        assert_eq!(resolve_deribit_instrument("ETH"), "ETH-PERPETUAL");
        assert_eq!(resolve_deribit_instrument("eth"), "ETH-PERPETUAL");
        assert_eq!(resolve_deribit_instrument("BTC-PERPETUAL"), "BTC-PERPETUAL");
        assert_eq!(
            resolve_deribit_instrument("BTC-25DEC26-100000-C"),
            "BTC-25DEC26-100000-C"
        );
        assert_eq!(resolve_deribit_instrument("SOL"), "SOL");
    }

    #[tokio::test]
    #[ignore]
    async fn deribit_bare_currency_resolves_perpetual() {
        let a = DeribitAdapter::for_testnet(None, None);
        let m = a.instrument_meta("BTC").await.unwrap();
        assert_eq!(m.name, "BTC-PERPETUAL");
        assert_eq!(m.kind, DeribitKind::Future);
    }

    /// Live testnet smoke test — validates real wire shapes against
    /// test.deribit.com. Run explicitly:
    /// `cargo test -p arbitrageur-exchanges deribit_live -- --ignored`
    #[tokio::test]
    #[ignore]
    async fn deribit_live_testnet_public_endpoints() {
        let a = DeribitAdapter::for_testnet(None, None);
        a.ping().await.unwrap();
        let markets = a.fetch_markets().await.unwrap();
        assert!(markets.len() > 50, "markets={}", markets.len());
        // Testnet books are often empty — the parse path is what matters.
        let book = a.fetch_orderbook("BTC-PERPETUAL").await.unwrap();
        assert_eq!(book.venue, VenueId::new("deribit"));
        let f = a.fetch_funding_rate("BTC-PERPETUAL").await.unwrap();
        assert_eq!(f.symbol, "BTC-PERPETUAL");
        let chain = a.option_chain("BTC").await.unwrap();
        assert!(chain.len() > 10, "chain={}", chain.len());
        assert!(chain
            .iter()
            .any(|o| o.option_type == DeribitOptionType::Call));
        assert!(chain
            .iter()
            .any(|o| o.option_type == DeribitOptionType::Put));
        let opt = a
            .fetch_orderbook(&chain.first().unwrap().name)
            .await
            .unwrap();
        assert_eq!(opt.venue, VenueId::new("deribit"));
        let prices = a.fetch_market_prices().await.unwrap();
        assert!(prices.contains_key("BTC-PERPETUAL"));
    }

    #[tokio::test]
    async fn test_deribit_combo_leg_count_bounds() {
        let a = DeribitAdapter::for_testnet(None, None);
        // Empty trades: < 2 legs must fail closed
        let empty: Vec<DeribitComboTrade> = vec![];
        assert!(a.create_combo(&empty).await.is_err());

        // Single trade: < 2 legs must fail closed
        let one = vec![DeribitComboTrade {
            instrument_name: "BTC-25DEC26-100000-C".into(),
            amount: 1,
            direction: "buy".into(),
        }];
        assert!(a.create_combo(&one).await.is_err());

        // 5 trades: > 4 legs must fail closed
        let five = vec![
            DeribitComboTrade {
                instrument_name: "L1".into(),
                amount: 1,
                direction: "buy".into(),
            },
            DeribitComboTrade {
                instrument_name: "L2".into(),
                amount: 1,
                direction: "buy".into(),
            },
            DeribitComboTrade {
                instrument_name: "L3".into(),
                amount: 1,
                direction: "buy".into(),
            },
            DeribitComboTrade {
                instrument_name: "L4".into(),
                amount: 1,
                direction: "buy".into(),
            },
            DeribitComboTrade {
                instrument_name: "L5".into(),
                amount: 1,
                direction: "buy".into(),
            },
        ];
        assert!(a.create_combo(&five).await.is_err());

        // 8 trades (e.g. Jelly Roll as single combo): must fail closed
        let eight: Vec<DeribitComboTrade> = (1..=8)
            .map(|i| DeribitComboTrade {
                instrument_name: format!("L{i}"),
                amount: 1,
                direction: "buy".into(),
            })
            .collect();
        assert!(a.create_combo(&eight).await.is_err());
    }

    /// The per-leg fills of a combo trade are the only venue-documented
    /// per-instrument prices a combo produces (#119); partial fills must be
    /// size-weighted, not overwritten.
    #[test]
    fn combo_leg_fills_vwap_across_partial_trades() {
        let resp = serde_json::json!({
            "trades": [
                {
                    "order_id": "X1",
                    "combo_id": "BTC-CS-1",
                    "legs": [
                        {"instrument_name": "BTC-25DEC26-100000-C", "price": 0.050, "amount": 1, "direction": "buy"},
                        {"instrument_name": "BTC-25DEC26-110000-C", "price": 0.030, "amount": 1, "direction": "sell"}
                    ]
                },
                {
                    // Second partial: 3 contracts at a better price.
                    "order_id": "X1",
                    "combo_id": "BTC-CS-1",
                    "legs": [
                        {"instrument_name": "BTC-25DEC26-100000-C", "price": 0.040, "amount": 3, "direction": "buy"},
                        {"instrument_name": "BTC-25DEC26-110000-C", "price": 0.020, "amount": -3, "direction": "sell"}
                    ]
                }
            ]
        });
        let fills = combo_leg_fills_from_response(&resp, "X1").expect("legs parse");
        assert_eq!(fills.len(), 2);
        let c100 = fills
            .iter()
            .find(|f| f.instrument == "BTC-25DEC26-100000-C")
            .expect("call leg");
        // (0.05·1 + 0.04·3) / 4 = 0.0425 — a first-trade-only read would say 0.05.
        assert_eq!(c100.price, dec!(0.0425));
        assert_eq!(c100.amount, dec!(4));
        let c110 = fills
            .iter()
            .find(|f| f.instrument == "BTC-25DEC26-110000-C")
            .expect("call leg");
        // Negative amounts (the sell side) are normalised to size.
        assert_eq!(c110.price, dec!(0.0225));
        assert_eq!(c110.amount, dec!(4));
    }

    /// A response without a legs array (a plain single-instrument order) must
    /// be an error so the caller attributes the net instead of booking nothing.
    #[test]
    fn combo_leg_fills_reject_a_response_without_legs() {
        let plain = serde_json::json!({
            "trades": [{"order_id": "X2", "instrument_name": "BTC-PERPETUAL", "price": 100.0, "amount": 1}]
        });
        assert!(combo_leg_fills_from_response(&plain, "X2").is_err());

        // Legs with no usable price/size are not fills either.
        let broken = serde_json::json!({
            "trades": [{"legs": [{"instrument_name": "BTC-25DEC26-100000-C", "price": 0, "amount": 0}]}]
        });
        assert!(combo_leg_fills_from_response(&broken, "X3").is_err());

        // A non-array payload is a client error, not an empty success.
        assert!(
            combo_leg_fills_from_response(&serde_json::json!({"trades": "nope"}), "X4").is_err()
        );
    }
}
