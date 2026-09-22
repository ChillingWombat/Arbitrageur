use crate::rate_limiter::AccountRequestCoordinator;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::{AccountSnapshot, Position};
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{dec, Decimal, Side, VenueId};
use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signer, SigningKey};
use reqwest::Client;
use std::collections::HashMap;

/// Vanta Exchange (Orderly Network broker protocol) connector.
/// Vanta's perpetual API is hosted on Orderly; public read-only calls use
/// POST {base}/v1/public/query without credentials.
#[allow(dead_code)]
pub struct VantaAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    account_id: parking_lot::RwLock<Option<String>>,
    orderly_key: parking_lot::RwLock<Option<String>>,
    orderly_secret: parking_lot::RwLock<Option<String>>,
    rate_limiter: AccountRequestCoordinator,
}

fn dec_from_json(v: Option<&serde_json::Value>) -> Option<Decimal> {
    match v? {
        serde_json::Value::String(s) => Decimal::from_str_exact(s).ok(),
        serde_json::Value::Number(n) => Decimal::from_str_exact(&n.to_string()).ok(),
        _ => None,
    }
}

fn ts_from_json(v: Option<&serde_json::Value>) -> Option<DateTime<Utc>> {
    let ms = match v? {
        serde_json::Value::Number(n) => n.as_i64()?,
        serde_json::Value::String(s) => s.parse::<i64>().ok()?,
        _ => return None,
    };
    DateTime::from_timestamp_millis(ms)
}

/// Exact standard Orderly perp symbol `PERP_<BASE>_USDC`; broker-suffixed or
/// otherwise malformed symbols do not match.
fn standard_symbol_base(symbol: &str) -> Option<&str> {
    let mut parts = symbol.split('_');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("PERP"), Some(base), Some("USDC"), None) if !base.is_empty() => Some(base),
        _ => None,
    }
}

fn parse_market_summary_entry(venue: &VenueId, m: &serde_json::Value) -> Option<MarketMetadata> {
    let symbol_str = m.get("symbol")?.as_str()?;
    let base = standard_symbol_base(symbol_str)?;
    let quote_tick = dec_from_json(m.get("quote_tick"))?;
    let base_tick = dec_from_json(m.get("base_tick"))?;
    let min_notional = dec_from_json(m.get("min_notional"))?;
    if !(quote_tick > dec!(0) && base_tick > dec!(0) && min_notional >= dec!(0)) {
        return None;
    }
    Some(MarketMetadata {
        symbol: Symbol::perp(base, "USDC"),
        venue: venue.clone(),
        venue_symbol: symbol_str.to_string(),
        price_decimals: quote_tick.normalize().scale(),
        size_decimals: base_tick.normalize().scale(),
        min_order_size: base_tick,
        min_notional,
        maker_fee_bps: dec!(2.0),
        taker_fee_bps: dec!(5.0),
        contract_size: dec!(1.0),
        is_active: true,
    })
}

fn parse_market_summary_volumes(data: &serde_json::Value) -> HashMap<String, Decimal> {
    let markets = match data.get("markets").and_then(|m| m.as_array()) {
        Some(m) => m,
        None => return HashMap::new(),
    };
    let mut out = HashMap::new();
    for m in markets {
        if let Some(sym) = m.get("symbol").and_then(|s| s.as_str()) {
            if let Some(base) = standard_symbol_base(sym) {
                let amount = dec_from_json(m.get("24h_amount"));
                let vol = dec_from_json(m.get("24h_volume"));
                let mark = dec_from_json(m.get("mark_price"));
                let notional = amount
                    .filter(|a| *a > dec!(0))
                    .or_else(|| match (vol, mark) {
                        (Some(v), Some(p)) if v > dec!(0) && p > dec!(0) => Some(v * p),
                        _ => None,
                    });
                if let Some(n) = notional {
                    // Canonical BASE-QUOTE keys match what hub consumers
                    // look up; Vanta perps are all USDC-quoted.
                    out.insert(format!("{}-USDC", base.to_uppercase()), n);
                }
            }
        }
    }
    out
}

fn market_entry<'a>(
    data: &'a serde_json::Value,
    vanta_sym: &str,
) -> anyhow::Result<&'a serde_json::Value> {
    data.get("markets")
        .and_then(|m| m.as_array())
        .ok_or_else(|| anyhow::anyhow!("Vanta marketSummary response missing data.markets"))?
        .iter()
        .find(|m| m.get("symbol").and_then(|s| s.as_str()) == Some(vanta_sym))
        .ok_or_else(|| anyhow::anyhow!("Vanta marketSummary has no entry for {vanta_sym}"))
}

fn parse_orderbook_data(
    venue: &VenueId,
    symbol: &str,
    vanta_sym: &str,
    data: &serde_json::Value,
) -> anyhow::Result<OrderBook> {
    let returned = data
        .get("symbol")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow::anyhow!("Vanta orderbook response missing data.symbol"))?;
    if returned != vanta_sym {
        anyhow::bail!("Vanta orderbook returned symbol {returned} for requested {vanta_sym}");
    }
    let mut book = OrderBook::new(venue.clone(), symbol.to_string());
    if let Some(levels) = data.get("bids").and_then(|l| l.as_array()) {
        for lvl in levels {
            if let (Some(p), Some(q)) = (
                dec_from_json(lvl.get("price")),
                dec_from_json(lvl.get("quantity")),
            ) {
                book.bids.push(BookLevel::new(p, q));
            }
        }
    }
    if let Some(levels) = data.get("asks").and_then(|l| l.as_array()) {
        for lvl in levels {
            if let (Some(p), Some(q)) = (
                dec_from_json(lvl.get("price")),
                dec_from_json(lvl.get("quantity")),
            ) {
                book.asks.push(BookLevel::new(p, q));
            }
        }
    }
    book.timestamp = Utc::now();
    Ok(book)
}

fn parse_ticker_data(
    venue: &VenueId,
    symbol: &str,
    vanta_sym: &str,
    data: &serde_json::Value,
) -> anyhow::Result<Ticker> {
    let m = market_entry(data, vanta_sym)?;
    let bid = dec_from_json(m.get("bid_price"))
        .filter(|d| *d > dec!(0))
        .ok_or_else(|| {
            anyhow::anyhow!("Vanta marketSummary for {vanta_sym} has no positive bid_price")
        })?;
    let ask = dec_from_json(m.get("ask_price"))
        .filter(|d| *d > dec!(0))
        .ok_or_else(|| {
            anyhow::anyhow!("Vanta marketSummary for {vanta_sym} has no positive ask_price")
        })?;
    let last = dec_from_json(m.get("mark_price"))
        .filter(|d| *d > dec!(0))
        .unwrap_or((bid + ask) / dec!(2));
    Ok(Ticker {
        venue: venue.clone(),
        symbol: symbol.to_string(),
        bid,
        ask,
        last,
        timestamp: Utc::now(),
    })
}

fn parse_funding_data(
    venue: &VenueId,
    symbol: &str,
    vanta_sym: &str,
    data: &serde_json::Value,
) -> anyhow::Result<FundingRate> {
    let m = market_entry(data, vanta_sym)?;
    let rate = dec_from_json(m.get("last_funding_rate")).ok_or_else(|| {
        anyhow::anyhow!("Vanta marketSummary for {vanta_sym} has no parseable last_funding_rate")
    })?;
    let predicted_rate = dec_from_json(m.get("est_funding_rate"));
    let next_funding_time = ts_from_json(m.get("next_funding_time")).ok_or_else(|| {
        anyhow::anyhow!("Vanta marketSummary for {vanta_sym} has no valid next_funding_time")
    })?;
    Ok(FundingRate {
        venue: venue.clone(),
        symbol: symbol.to_string(),
        rate,
        predicted_rate,
        interval_hours: 8,
        next_funding_time,
        timestamp: Utc::now(),
    })
}

/// Decode 32-byte Ed25519 secret key from Base58 or Hex string.
fn decode_orderly_secret(raw: &str) -> anyhow::Result<[u8; 32]> {
    let unquoted = raw.trim().trim_matches('"').trim_matches('\'').trim();
    let clean = unquoted.strip_prefix("ed25519:").unwrap_or(unquoted);
    // 1. Try base58
    if let Ok(bytes) = bs58::decode(clean).into_vec() {
        if bytes.len() == 32 {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            return Ok(arr);
        } else if bytes.len() == 64 {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes[..32]);
            return Ok(arr);
        }
    }
    // 2. Try hex
    let hex_clean = clean.strip_prefix("0x").unwrap_or(clean);
    if let Ok(bytes) = hex::decode(hex_clean) {
        if bytes.len() == 32 {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            return Ok(arr);
        } else if bytes.len() == 64 {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes[..32]);
            return Ok(arr);
        }
    }
    anyhow::bail!(
        "Invalid Orderly secret format: expected 32-byte Base58 or Hex Ed25519 private key"
    )
}

/// Sign message for Orderly Network: Ed25519 signature over message bytes, URL-safe base64 encoded (no padding).
fn sign_orderly_message(secret: &str, message: &str) -> anyhow::Result<String> {
    let key_bytes = decode_orderly_secret(secret)?;
    let signing_key = SigningKey::from_bytes(&key_bytes);
    let signature = signing_key.sign(message.as_bytes());
    Ok(URL_SAFE_NO_PAD.encode(signature.to_bytes()))
}

fn parse_orderly_account_data(
    venue_id: &VenueId,
    data: &serde_json::Value,
) -> anyhow::Result<AccountSnapshot> {
    let free_margin = dec_from_json(data.get("free_collateral"))
        .or_else(|| dec_from_json(data.get("freeCollateral")))
        .or_else(|| dec_from_json(data.get("available_balance")))
        .unwrap_or(dec!(0));

    let equity = dec_from_json(data.get("total_collateral_value"))
        .or_else(|| dec_from_json(data.get("totalCollateralValue")))
        .or_else(|| dec_from_json(data.get("total_asset_value")))
        .or_else(|| dec_from_json(data.get("portfolio_value")))
        .unwrap_or(free_margin);

    let maintenance_margin = dec_from_json(data.get("total_maintenance_margin"))
        .or_else(|| dec_from_json(data.get("maintenance_margin")))
        .unwrap_or(dec!(0));

    let margin_usage_pct = if equity > dec!(0) {
        if free_margin < equity {
            ((equity - free_margin) / equity * dec!(100)).clamp(dec!(0), dec!(100))
        } else {
            dec!(0)
        }
    } else {
        dec!(0)
    };

    let mut positions = Vec::new();
    let rows = data
        .get("rows")
        .or_else(|| data.get("positions"))
        .and_then(|r| r.as_array());

    if let Some(pos_list) = rows {
        for p in pos_list {
            let sym = p.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
            let qty = dec_from_json(p.get("position_qty"))
                .or_else(|| dec_from_json(p.get("positionQty")))
                .or_else(|| dec_from_json(p.get("quantity")))
                .unwrap_or(dec!(0));

            if qty == dec!(0) {
                continue;
            }

            let side = if qty > dec!(0) { Side::Buy } else { Side::Sell };

            let entry_price = dec_from_json(p.get("average_open_price"))
                .or_else(|| dec_from_json(p.get("averageOpenPrice")))
                .or_else(|| dec_from_json(p.get("entry_price")))
                .unwrap_or(dec!(0));

            let mark_price = dec_from_json(p.get("mark_price"))
                .or_else(|| dec_from_json(p.get("markPrice")))
                .unwrap_or(entry_price);

            let unrealized_pnl = dec_from_json(p.get("unsettled_pnl"))
                .or_else(|| dec_from_json(p.get("unsettledPnl")))
                .or_else(|| dec_from_json(p.get("unrealized_pnl")))
                .unwrap_or(dec!(0));

            let leverage = dec_from_json(p.get("leverage")).unwrap_or(dec!(1));

            positions.push(Position {
                venue: venue_id.clone(),
                symbol: sym.to_string(),
                side,
                quantity: qty.abs(),
                entry_price,
                mark_price,
                unrealized_pnl,
                realized_pnl: dec!(0),
                liquidation_price: dec_from_json(p.get("liquidation_price"))
                    .or_else(|| dec_from_json(p.get("liquidationPrice"))),
                leverage,
                initial_margin: dec!(0),
                maintenance_margin: dec!(0),
                updated_at: Utc::now(),
            });
        }
    }

    Ok(AccountSnapshot {
        venue: venue_id.clone(),
        equity,
        free_margin,
        maintenance_margin,
        margin_usage_pct,
        balances: vec![arbitrageur_core::position::Balance {
            venue: venue_id.clone(),
            currency: "USDC".to_string(),
            total: equity,
            free: free_margin,
            locked: (equity - free_margin).max(dec!(0)),
        }],
        positions,
        timestamp: Utc::now(),
    })
}

impl VantaAdapter {
    pub fn new(
        base_url: Option<String>,
        account_id: Option<String>,
        orderly_key: Option<String>,
        orderly_secret: Option<String>,
    ) -> Self {
        let base = base_url.unwrap_or_else(|| "https://api.orderly.org".to_string());
        Self {
            venue_id: VenueId::new("vanta"),
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
            account_id: parking_lot::RwLock::new(account_id),
            orderly_key: parking_lot::RwLock::new(orderly_key),
            orderly_secret: parking_lot::RwLock::new(orderly_secret),
            rate_limiter: AccountRequestCoordinator::new("vanta", 150, 2000, 500, 10000),
        }
    }

    pub fn update_credentials(
        &self,
        account_id: Option<String>,
        orderly_key: Option<String>,
        orderly_secret: Option<String>,
    ) {
        *self.account_id.write() = account_id;
        *self.orderly_key.write() = orderly_key;
        *self.orderly_secret.write() = orderly_secret;
    }

    /// Futures symbol normalization. Orderly perp ids are `PERP_*`; a spot
    /// request (either `SPOT_*` exchange format or a `-SPOT` market-type
    /// suffix) must fail closed — silently mapping it onto the perp book
    /// would misattribute the market type.
    fn to_vanta_symbol(symbol: &str) -> anyhow::Result<String> {
        let tokens: Vec<&str> = symbol.split(['-', '/', '_']).collect();
        let first = tokens.first().copied().unwrap_or("");
        let last = tokens.last().copied().unwrap_or("");
        if first.eq_ignore_ascii_case("spot") || last.eq_ignore_ascii_case("spot") {
            anyhow::bail!(
                "Vanta spot instrument '{symbol}' is not supported by this adapter's futures API; refusing to query a perp market"
            );
        }
        let clean = symbol.replace('-', "_");
        if clean.starts_with("PERP_") {
            if !clean.ends_with("_USDC") {
                anyhow::bail!(
                    "Vanta perpetual markets are only quoted in USDC; '{symbol}' is not supported"
                );
            }
            Ok(clean)
        } else {
            if tokens.len() >= 2 {
                let quote = tokens[1];
                if !quote.eq_ignore_ascii_case("USDC") {
                    anyhow::bail!(
                        "Vanta perpetual markets are only quoted in USDC; '{symbol}' is not supported"
                    );
                }
            }
            let base = tokens.first().copied().unwrap_or(symbol).to_uppercase();
            if base.is_empty() {
                anyhow::bail!("Vanta symbol '{symbol}' has empty base asset");
            }
            Ok(format!("PERP_{}_USDC", base))
        }
    }

    #[allow(dead_code)]
    fn from_vanta_symbol(vanta_symbol: &str) -> String {
        vanta_symbol
            .strip_prefix("PERP_")
            .unwrap_or(vanta_symbol)
            .replace('_', "-")
    }

    /// Registered Orderly account id for the account-scoped public WS
    /// stream. Empty/whitespace resolves to `None` (lane stays disabled).
    pub fn ws_account_id(&self) -> Option<String> {
        let acc = self.account_id.read().clone().unwrap_or_default();
        let clean = acc.trim().trim_matches('"').trim_matches('\'').trim();
        (!clean.is_empty()).then(|| clean.to_string())
    }

    fn clean_account_id(&self) -> anyhow::Result<String> {
        let acc = self.account_id.read().clone().unwrap_or_default();
        let clean = acc.trim().trim_matches('"').trim_matches('\'').trim();
        if clean.is_empty() {
            anyhow::bail!(
                "Vanta live trading requires an Orderly Account ID configured in credentials"
            );
        }
        Ok(clean.to_string())
    }

    fn orderly_secret(&self) -> anyhow::Result<String> {
        let sec = self.orderly_secret.read().clone().unwrap_or_default();
        if sec.trim().is_empty() {
            anyhow::bail!(
                "Vanta live trading requires the Orderly Secret configured in credentials"
            );
        }
        Ok(sec)
    }

    /// Candidate `orderly-key` header values: the configured key first, then
    /// the key derived from the secret (which is mathematically guaranteed to
    /// match the signing key). Mirrors the /v1/positions fallback.
    fn orderly_key_candidates(&self, secret: &str) -> anyhow::Result<Vec<String>> {
        let derived = match decode_orderly_secret(secret) {
            Ok(bytes) => {
                let sk = SigningKey::from_bytes(&bytes);
                Some(bs58::encode(sk.verifying_key().to_bytes()).into_string())
            }
            Err(_) => None,
        };
        let configured = self
            .orderly_key
            .read()
            .clone()
            .unwrap_or_default()
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .strip_prefix("ed25519:")
            .unwrap_or(
                self.orderly_key
                    .read()
                    .clone()
                    .unwrap_or_default()
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\''),
            )
            .trim()
            .to_string();
        let mut out = Vec::new();
        if !configured.is_empty() {
            out.push(format!("ed25519:{configured}"));
        }
        if let Some(d) = derived {
            let full = format!("ed25519:{d}");
            if !out.contains(&full) {
                out.push(full);
            }
        }
        if out.is_empty() {
            anyhow::bail!("Cannot determine Orderly Key from credentials");
        }
        Ok(out)
    }

    /// Signed private Orderly call with -1004 key fallback across candidates.
    /// `sign_path` is the exact path+query string covered by the signature
    /// (e.g. `/v1/order` for POST, `/v1/order?symbol=..&order_id=..` for
    /// DELETE); `body` is the exact JSON bytes sent (empty for DELETE).
    async fn signed_orderly(
        &self,
        method: &str,
        sign_path: &str,
        body: &str,
    ) -> anyhow::Result<serde_json::Value> {
        let account_id = self.clean_account_id()?;
        let secret = self.orderly_secret()?;
        let candidates = self.orderly_key_candidates(&secret)?;
        let ts = Utc::now().timestamp_millis();
        let message = format!("{ts}{method}{sign_path}{body}");
        let sig = sign_orderly_message(&secret, &message)?;
        let url = format!("{}{}", self.base_url, sign_path);
        let mut last_err = String::new();
        for key in candidates {
            let mut req = match method {
                "POST" => self.client.post(&url),
                "DELETE" => self.client.delete(&url),
                "GET" => self.client.get(&url),
                _ => anyhow::bail!("Vanta unsupported signed method {method}"),
            };
            req = req
                .header("Content-Type", "application/json")
                .header("orderly-account-id", &account_id)
                .header("orderly-key", &key)
                .header("orderly-timestamp", ts.to_string())
                .header("orderly-signature", &sig);
            if !body.is_empty() {
                req = req.body(body.to_string());
            }
            let resp = req.send().await.map_err(|e| {
                anyhow::anyhow!("Vanta {method} {sign_path} transport failure: {e}")
            })?;
            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                last_err = format!("Vanta {method} {sign_path} HTTP {status}: {text}");
                if text.contains("-1004") || text.contains("orderly key error") {
                    continue;
                }
                anyhow::bail!("{last_err}");
            }
            let v: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| anyhow::anyhow!("Vanta {method} {sign_path} bad JSON: {e}"))?;
            if v.get("success").and_then(|s| s.as_bool()) != Some(true) {
                let msg = v
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error");
                last_err = format!("Vanta {method} {sign_path} error: {msg} ({v})");
                if v.to_string().contains("-1004") {
                    continue;
                }
                anyhow::bail!("{last_err}");
            }
            return Ok(v);
        }
        anyhow::bail!("{last_err}")
    }

    /// Pure build of a POST /v1/order body (no network) — unit-tested.
    /// Venue order granularity for one symbol — (quote_tick, base_tick).
    /// marketSummary is cached by the request coordinator, so this is
    /// cheap on the order path.
    async fn symbol_ticks(&self, venue_symbol: &str) -> anyhow::Result<(Decimal, Decimal)> {
        let data = self
            .public_query(serde_json::json!({"type": "marketSummary"}))
            .await?;
        let row = data
            .get("markets")
            .and_then(|m| m.as_array())
            .and_then(|arr| {
                arr.iter()
                    .find(|i| i.get("symbol").and_then(|s| s.as_str()) == Some(venue_symbol))
            })
            .ok_or_else(|| anyhow::anyhow!("Vanta marketSummary has no symbol {venue_symbol}"))?;
        let quote_tick = dec_from_json(row.get("quote_tick")).unwrap_or(dec!(0));
        let base_tick = dec_from_json(row.get("base_tick")).unwrap_or(dec!(0));
        Ok((quote_tick, base_tick))
    }

    fn build_order_body(
        order: &OrderRequest,
        venue_symbol: &str,
        quote_tick: Decimal,
        base_tick: Decimal,
    ) -> anyhow::Result<String> {
        if order.quantity <= dec!(0) {
            anyhow::bail!("Vanta order quantity must be positive");
        }
        let side_str = match order.side {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        };
        let is_market = order.order_type == arbitrageur_core::types::OrderType::Market;
        let order_type = if is_market {
            "MARKET"
        } else if order.post_only
            || order.time_in_force == arbitrageur_core::types::TimeInForce::PostOnly
        {
            "POST_ONLY"
        } else if order.time_in_force == arbitrageur_core::types::TimeInForce::ImmediateOrCancel {
            "IOC"
        } else if order.time_in_force == arbitrageur_core::types::TimeInForce::FillOrKill {
            "FOK"
        } else {
            "LIMIT"
        };
        // Orderly enforces base_tick / quote_tick — floor quantity to the
        // base tick (never oversize); the limit price is a protective
        // bound, so buys floor and sells ceil.
        let qty_wire = if base_tick > dec!(0) {
            (order.quantity / base_tick).floor() * base_tick
        } else {
            order.quantity
        };
        if qty_wire <= dec!(0) {
            anyhow::bail!(
                "Vanta {venue_symbol} quantity {} rounds to zero at base_tick {base_tick}",
                order.quantity
            );
        }
        let mut req = serde_json::json!({
            "symbol": venue_symbol,
            "order_type": order_type,
            "side": side_str,
            "order_quantity": qty_wire.normalize().to_string(),
        });
        if !is_market {
            let px = order.price.filter(|x| *x > dec!(0)).ok_or_else(|| {
                anyhow::anyhow!("Vanta {order_type} order requires a positive limit price")
            })?;
            let pxw = if quote_tick > dec!(0) {
                match order.side {
                    Side::Buy => (px / quote_tick).floor() * quote_tick,
                    Side::Sell => (px / quote_tick).ceil() * quote_tick,
                }
            } else {
                px
            };
            if pxw <= dec!(0) {
                anyhow::bail!("Vanta {venue_symbol} price {px} rounds below the quote_tick floor");
            }
            req["order_price"] = serde_json::Value::String(pxw.normalize().to_string());
        }
        if order.reduce_only {
            req["reduce_only"] = serde_json::Value::Bool(true);
        }
        if !order.client_order_id.is_empty() {
            req["client_order_id"] = serde_json::Value::String(order.client_order_id.clone());
        }
        serde_json::to_string(&req)
            .map_err(|e| anyhow::anyhow!("Vanta order body serialization failed: {e}"))
    }

    /// POST /v1/public/query — the documented public read-only endpoint.
    /// Returns the `data` payload; requires HTTP success and `success: true`.
    async fn public_query(&self, body: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let cache_key = body.to_string();
        if let Some(cached_str) = self
            .rate_limiter
            .acquire_or_cache_path(&cache_key, true)
            .await?
        {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&cached_str) {
                return Ok(val);
            }
        }

        let url = format!("{}/v1/public/query", self.base_url);
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
            anyhow::bail!("Vanta public query HTTP status: {}", resp.status());
        }

        let json_val = resp.json::<serde_json::Value>().await?;
        if json_val.get("success").and_then(|s| s.as_bool()) != Some(true) {
            self.rate_limiter.record_failure(false);
            anyhow::bail!("Vanta public query did not report success");
        }

        let data = json_val
            .get("data")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Vanta public query response missing data"))?;

        self.rate_limiter
            .record_success_path(&cache_key, data.to_string());
        Ok(data)
    }
}

#[async_trait]
impl ExchangeAdapter for VantaAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Vanta.exchange"
    }

    fn ws_account_id(&self) -> Option<String> {
        VantaAdapter::ws_account_id(self)
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let data = self
            .public_query(serde_json::json!({"type": "marketSummary"}))
            .await?;
        let markets = data
            .get("markets")
            .and_then(|m| m.as_array())
            .ok_or_else(|| anyhow::anyhow!("Vanta marketSummary response missing data.markets"))?;
        Ok(markets
            .iter()
            .filter_map(|m| parse_market_summary_entry(&self.venue_id, m))
            .collect())
    }

    async fn fetch_market_prices(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        let data = self
            .public_query(serde_json::json!({"type": "marketSummary"}))
            .await?;
        let markets = data
            .get("markets")
            .and_then(|m| m.as_array())
            .ok_or_else(|| anyhow::anyhow!("Vanta marketSummary response missing data.markets"))?;

        let mut out = HashMap::new();
        for market in markets {
            let Some(symbol) = market.get("symbol").and_then(|s| s.as_str()) else {
                continue;
            };
            let Some(price) = dec_from_json(market.get("mark_price")).filter(|p| *p > dec!(0))
            else {
                continue;
            };
            out.insert(symbol.to_string(), price);
        }
        Ok(out)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let vanta_sym = Self::to_vanta_symbol(symbol)?;
        let data = self
            .public_query(serde_json::json!({
                "type": "orderbook",
                "symbol": vanta_sym,
                "max_level": 100,
            }))
            .await?;
        parse_orderbook_data(&self.venue_id, symbol, &vanta_sym, &data)
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let vanta_sym = Self::to_vanta_symbol(symbol)?;
        let data = self
            .public_query(
                serde_json::json!({"type": "marketSummary", "symbols": [vanta_sym.clone()]}),
            )
            .await?;
        parse_ticker_data(&self.venue_id, symbol, &vanta_sym, &data)
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate> {
        let vanta_sym = Self::to_vanta_symbol(symbol)?;
        let data = self
            .public_query(
                serde_json::json!({"type": "marketSummary", "symbols": [vanta_sym.clone()]}),
            )
            .await?;
        parse_funding_data(&self.venue_id, symbol, &vanta_sym, &data)
    }

    async fn fetch_volumes_24h(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        let data = match self
            .public_query(serde_json::json!({"type": "marketSummary"}))
            .await
        {
            Ok(d) => d,
            Err(_) => return Ok(HashMap::new()),
        };
        Ok(parse_market_summary_volumes(&data))
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        let account_id = self.account_id.read().clone().unwrap_or_default();
        let orderly_key = self.orderly_key.read().clone().unwrap_or_default();
        let orderly_secret = self.orderly_secret.read().clone().unwrap_or_default();

        let clean_acc = account_id
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .trim();
        if clean_acc.is_empty() {
            anyhow::bail!(
                "Vanta live balance requires an Orderly Account ID configured in credentials"
            );
        }

        // 1. Authenticated query if orderly_secret is present
        if !orderly_secret.trim().is_empty() {
            let ts = Utc::now().timestamp_millis();
            let message = format!("{ts}GET/v1/positions");
            let sig = sign_orderly_message(&orderly_secret, &message)?;

            // Derive public key from secret key to guarantee mathematical validity
            let derived_pubkey = match decode_orderly_secret(&orderly_secret) {
                Ok(key_bytes) => {
                    let sk = SigningKey::from_bytes(&key_bytes);
                    Some(bs58::encode(sk.verifying_key().to_bytes()).into_string())
                }
                Err(_) => None,
            };

            let clean_key = orderly_key
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .strip_prefix("ed25519:")
                .unwrap_or(orderly_key.trim().trim_matches('"').trim_matches('\''))
                .trim();

            // Orderly Network requires the "ed25519:" prefix in the orderly-key header.
            // Priority:
            // 1. If configured key is provided, use `ed25519:{clean_key}`.
            // 2. If configured key fails with -1004 (or is empty), use derived key `ed25519:{derived_pubkey}`.
            let candidate_keys: Vec<String> = if !clean_key.is_empty() {
                if let Some(ref derived) = derived_pubkey {
                    if clean_key != derived {
                        vec![format!("ed25519:{clean_key}"), format!("ed25519:{derived}")]
                    } else {
                        vec![format!("ed25519:{clean_key}")]
                    }
                } else {
                    vec![format!("ed25519:{clean_key}")]
                }
            } else if let Some(ref derived) = derived_pubkey {
                vec![format!("ed25519:{derived}")]
            } else {
                anyhow::bail!("Cannot determine Orderly Key from credentials");
            };

            let url = format!("{}/v1/positions", self.base_url);
            let mut last_err = String::new();

            for k in candidate_keys {
                let resp = self
                    .client
                    .get(&url)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .header("orderly-account-id", clean_acc)
                    .header("orderly-key", &k)
                    .header("orderly-timestamp", ts.to_string())
                    .header("orderly-signature", &sig)
                    .send()
                    .await?;

                if !resp.status().is_success() {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    last_err = format!("Vanta /v1/positions HTTP {status}: {body}");
                    // If -1004 ("orderly key error"), try fallback candidate key if available
                    if body.contains("-1004") || body.contains("orderly key error") {
                        continue;
                    }
                    anyhow::bail!("{last_err}");
                }

                let json_val = resp.json::<serde_json::Value>().await?;
                if json_val.get("success").and_then(|s| s.as_bool()) != Some(true) {
                    let msg = json_val
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("unknown error");
                    last_err = format!("Vanta /v1/positions error: {msg}");
                    continue;
                }

                let data = json_val
                    .get("data")
                    .ok_or_else(|| anyhow::anyhow!("Vanta /v1/positions missing data"))?;
                return parse_orderly_account_data(&self.venue_id, data);
            }

            anyhow::bail!("{last_err}");
        }

        // 2. Fallback: if account_id is an EVM address (0x...), try public zero-auth accountState query
        if clean_acc.starts_with("0x") {
            let body = serde_json::json!({
                "type": "accountState",
                "address": clean_acc,
            });
            let data = self.public_query(body).await?;
            return parse_orderly_account_data(&self.venue_id, &data);
        }

        anyhow::bail!("Vanta live balance requires Orderly Secret configured in credentials")
    }

    fn normalize_account_position(&self, position: &Position) -> anyhow::Result<Option<Position>> {
        let mut position = position.clone();
        if let Some(base) = standard_symbol_base(&position.symbol) {
            position.symbol = format!("{base}-USDC");
        } else if position.symbol.contains('-') {
            position.symbol = position.symbol.to_uppercase();
            Self::to_vanta_symbol(&position.symbol)?;
        } else {
            anyhow::bail!(
                "Vanta account symbol '{}' is not a standard PERP_BASE_USDC market",
                position.symbol
            );
        }
        Ok(Some(position))
    }

    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        use arbitrageur_core::types::OrderStatus;
        let venue_symbol = Self::to_vanta_symbol(&order.symbol)?;
        let (quote_tick, base_tick) = self.symbol_ticks(&venue_symbol).await?;
        let body = Self::build_order_body(&order, &venue_symbol, quote_tick, base_tick)?;
        let v = self.signed_orderly("POST", "/v1/order", &body).await?;
        let data = v
            .get("data")
            .ok_or_else(|| anyhow::anyhow!("Vanta order response missing data: {v}"))?;
        let order_id = data
            .get("order_id")
            .and_then(|o| o.as_i64())
            .map(|o| o.to_string())
            .or_else(|| {
                data.get("order_id")
                    .and_then(|o| o.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default();
        if !order_id.parse::<u64>().map(|id| id > 0).unwrap_or(false) {
            anyhow::bail!("Vanta order returned no valid positive order_id: {v}");
        }
        if let Some(returned_symbol) = data.get("symbol").and_then(|s| s.as_str()) {
            if returned_symbol != venue_symbol {
                anyhow::bail!(
                    "Vanta order response symbol '{returned_symbol}' does not match expected '{venue_symbol}': {v}"
                );
            }
        }
        if let Some(returned_client) = data
            .get("client_order_id")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
        {
            if returned_client != order.client_order_id {
                anyhow::bail!(
                    "Vanta order response client_order_id '{returned_client}' does not match request '{}': {v}",
                    order.client_order_id
                );
            }
        }
        Ok(OrderResponse {
            order_id,
            client_order_id: order.client_order_id.clone(),
            venue: VenueId::new("vanta"),
            symbol: order.symbol.clone(),
            status: OrderStatus::New,
            original_qty: order.quantity,
            filled_qty: dec!(0),
            remaining_qty: order.quantity,
            avg_fill_price: None,
            fee: dec!(0),
            fee_currency: "USDC".to_string(),
            raw_response: Some(v.to_string()),
            timestamp: Utc::now(),
        })
    }

    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
        if let Some(id) = req.order_id {
            let venue_symbol = Self::to_vanta_symbol(&req.symbol)?;
            let sign_path = format!("/v1/order?symbol={venue_symbol}&order_id={}", id.trim());
            self.signed_orderly("DELETE", &sign_path, "").await?;
            return Ok(());
        }
        if let Some(cid) = req.client_order_id {
            let venue_symbol = Self::to_vanta_symbol(&req.symbol)?;
            let sign_path = format!(
                "/v1/client/order?client_order_id={}&symbol={venue_symbol}",
                cid.trim()
            );
            self.signed_orderly("DELETE", &sign_path, "").await?;
            return Ok(());
        }
        anyhow::bail!("Vanta cancel requires an order_id or client_order_id")
    }

    /// Poll authoritative order state via `GET /v1/order/{order_id}`.
    /// Orderly returns `status`, `quantity`, `executed_quantity`, and
    /// `average_executed_price` in venue-native units — validated
    /// fail-closed before they reach the ledger.
    async fn fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        _client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        use arbitrageur_core::types::OrderStatus;
        if order_id.trim().is_empty() {
            anyhow::bail!("Vanta fetch_order requires a non-empty order_id");
        }
        let venue_symbol = Self::to_vanta_symbol(symbol)?;
        let sign_path = format!("/v1/order/{}", order_id.trim());
        let v = self.signed_orderly("GET", &sign_path, "").await?;
        let data = v
            .get("data")
            .ok_or_else(|| anyhow::anyhow!("Vanta fetch_order missing data: {v}"))?;
        let returned_id = data
            .get("order_id")
            .and_then(|o| {
                o.as_i64()
                    .map(|n| n.to_string())
                    .or_else(|| o.as_str().map(str::to_string))
            })
            .ok_or_else(|| anyhow::anyhow!("Vanta fetch_order missing order_id: {v}"))?;
        if returned_id != order_id.trim() {
            anyhow::bail!(
                "Vanta fetch_order returned order_id {returned_id} != queried {order_id}: {v}"
            );
        }
        let returned_symbol = data
            .get("symbol")
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow::anyhow!("Vanta fetch_order missing symbol: {v}"))?;
        if returned_symbol != venue_symbol {
            anyhow::bail!(
                "Vanta fetch_order symbol '{returned_symbol}' != expected '{venue_symbol}': {v}"
            );
        }
        let orig_qty = dec_from_json(data.get("quantity"))
            .filter(|q| *q > dec!(0))
            .ok_or_else(|| anyhow::anyhow!("Vanta fetch_order missing quantity: {v}"))?;
        let filled_qty = dec_from_json(data.get("executed_quantity"))
            .filter(|q| *q >= dec!(0))
            .unwrap_or_default();
        if filled_qty > orig_qty {
            anyhow::bail!(
                "Vanta fetch_order executed_quantity {filled_qty} exceeds quantity {orig_qty}: {v}"
            );
        }
        let raw_status = data
            .get("status")
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow::anyhow!("Vanta fetch_order missing status: {v}"))?;
        let status = match raw_status {
            "FILLED" | "COMPLETED" => OrderStatus::Filled,
            "PARTIAL_FILLED" | "PARTIALLY_FILLED" => OrderStatus::PartiallyFilled,
            "NEW" | "OPEN" | "PENDING" | "REPLACED" | "INCOMPLETE" => OrderStatus::New,
            "CANCELLED" | "CANCELED" | "EXPIRED" => OrderStatus::Cancelled,
            "REJECTED" | "FAILED" => OrderStatus::Rejected,
            other => anyhow::bail!("Vanta fetch_order unknown status '{other}': {v}"),
        };
        let consistent = match status {
            OrderStatus::Filled => filled_qty == orig_qty,
            OrderStatus::PartiallyFilled => filled_qty > dec!(0) && filled_qty < orig_qty,
            OrderStatus::New => filled_qty == dec!(0),
            OrderStatus::Cancelled | OrderStatus::Rejected | OrderStatus::Expired => true,
        };
        if !consistent {
            anyhow::bail!(
                "Vanta fetch_order status {status:?} inconsistent with executed {filled_qty}/{orig_qty}: {v}"
            );
        }
        let avg_px = if filled_qty > dec!(0) {
            Some(
                dec_from_json(data.get("average_executed_price"))
                    .filter(|p| *p > dec!(0))
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "Vanta fetch_order has executed qty but no average_executed_price: {v}"
                        )
                    })?,
            )
        } else {
            None
        };
        let fee = dec_from_json(data.get("total_fee"))
            .or_else(|| dec_from_json(data.get("fee")))
            .unwrap_or_default()
            .abs();
        Ok(OrderResponse {
            order_id: returned_id,
            client_order_id: data
                .get("client_order_id")
                .and_then(|s| s.as_str())
                .unwrap_or_default()
                .to_string(),
            venue: VenueId::new("vanta"),
            symbol: symbol.to_string(),
            status,
            original_qty: orig_qty,
            filled_qty,
            remaining_qty: orig_qty.saturating_sub(filled_qty),
            avg_fill_price: avg_px,
            fee,
            fee_currency: "USDC".to_string(),
            raw_response: Some(v.to_string()),
            timestamp: Utc::now(),
        })
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
        let has_acc = self
            .account_id
            .read()
            .as_ref()
            .is_some_and(|a| !a.trim().is_empty());
        let has_key = self
            .orderly_key
            .read()
            .as_ref()
            .is_some_and(|k| !k.trim().is_empty());
        let has_sec = self
            .orderly_secret
            .read()
            .as_ref()
            .is_some_and(|s| !s.trim().is_empty());
        has_acc && (has_key || has_sec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn market_row(symbol: &str) -> serde_json::Value {
        serde_json::json!({
            "symbol": symbol,
            "bid_price": "64199.5",
            "ask_price": "64205.0",
            "mark_price": "64202.25",
            "last_funding_rate": "0.00012",
            "est_funding_rate": "0.00015",
            "next_funding_time": 1757779200000i64,
            "min_notional": "10",
            "quote_tick": "0.5",
            "base_tick": "0.0001"
        })
    }

    #[test]
    fn sign_orderly_message_matches_pynacl_vector() {
        // Independent vector from PyNaCl over the same seed + message.
        let secret_hex = "9d61b19deffd5a60ba844af492ec2cc4449c5697b326919703bac031cae7f109";
        let message = r#"1750000000000POST/v1/order{"symbol":"PERP_BTC_USDC","order_type":"LIMIT","side":"BUY","order_quantity":"0.01","order_price":"60000"}"#;
        let sig = sign_orderly_message(secret_hex, message).unwrap();
        assert_eq!(
            sig,
            "ERIix5e553nVkMGutNozsbihkMCBGkNk0c4Dv-9qXv2LfSdx7WDGxYY7djH5DC1pKOsil9pAyOqe5w1j7bfiDg"
        );
    }

    #[test]
    fn build_order_body_maps_types_and_fields() {
        use arbitrageur_core::order::OrderRequest;
        use arbitrageur_core::types::{Side, TimeInForce};
        let mk = |t: arbitrageur_core::types::OrderType, tif: TimeInForce, px: Option<Decimal>| {
            let mut o =
                OrderRequest::new_market(VenueId::new("vanta"), "BTC-USDC", Side::Buy, dec!(0.01));
            o.order_type = t;
            o.time_in_force = tif;
            o.price = px;
            o
        };
        let v: serde_json::Value = serde_json::from_str(
            &VantaAdapter::build_order_body(
                &mk(
                    arbitrageur_core::types::OrderType::Limit,
                    TimeInForce::GoodTilCancelled,
                    Some(dec!(60000)),
                ),
                "PERP_BTC_USDC",
                dec!(0),
                dec!(0),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(v["order_type"], "LIMIT");
        assert_eq!(v["order_price"], "60000");
        let v: serde_json::Value = serde_json::from_str(
            &VantaAdapter::build_order_body(
                &mk(
                    arbitrageur_core::types::OrderType::Market,
                    TimeInForce::ImmediateOrCancel,
                    None,
                ),
                "PERP_BTC_USDC",
                dec!(0),
                dec!(0),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(v["order_type"], "MARKET");
        assert!(v.get("order_price").is_none());
        // Limit without price fails closed.
        assert!(VantaAdapter::build_order_body(
            &mk(
                arbitrageur_core::types::OrderType::Limit,
                TimeInForce::GoodTilCancelled,
                None,
            ),
            "PERP_BTC_USDC",
            dec!(0),
            dec!(0),
        )
        .is_err());
    }

    #[test]
    fn build_order_body_respects_orderly_ticks() {
        use arbitrageur_core::order::OrderRequest;
        use arbitrageur_core::types::{Side, TimeInForce};
        let mk = |side: Side, qty: Decimal, px: Decimal| {
            let mut o = OrderRequest::new_market(VenueId::new("vanta"), "BTC-USDC", side, qty);
            o.order_type = arbitrageur_core::types::OrderType::Limit;
            o.time_in_force = TimeInForce::GoodTilCancelled;
            o.price = Some(px);
            o
        };
        // quote_tick 0.5, base_tick 0.0001 (the live PERP_ETH_USDC shape).
        let body = |o: &OrderRequest| {
            serde_json::from_str::<serde_json::Value>(
                &VantaAdapter::build_order_body(o, "PERP_BTC_USDC", dec!(0.5), dec!(0.0001))
                    .unwrap(),
            )
            .unwrap()
        };
        // Buy floors the protective cap; sell ceils it.
        let v = body(&mk(Side::Buy, dec!(0.01), dec!(60000.3)));
        assert_eq!(v["order_price"], "60000");
        let v = body(&mk(Side::Sell, dec!(0.01), dec!(60000.3)));
        assert_eq!(v["order_price"], "60000.5");
        // Quantity floors to base_tick — never oversizes.
        let v = body(&mk(Side::Buy, dec!(0.019999), dec!(60000)));
        assert_eq!(v["order_quantity"], "0.0199");
        // Sub-tick quantity fails closed rather than submitting dust.
        assert!(VantaAdapter::build_order_body(
            &mk(Side::Buy, dec!(0.00009), dec!(60000)),
            "PERP_BTC_USDC",
            dec!(0.5),
            dec!(0.0001),
        )
        .is_err());
    }

    #[test]
    fn parse_market_summary_entry_accepts_standard_perp() {
        let venue = VenueId::new("vanta");
        let m = market_row("PERP_BTC_USDC");
        let meta = parse_market_summary_entry(&venue, &m).expect("valid standard market");
        assert_eq!(meta.symbol.raw, "BTC-USDC");
        assert_eq!(meta.venue_symbol, "PERP_BTC_USDC");
        assert_eq!(meta.price_decimals, 1);
        assert_eq!(meta.size_decimals, 4);
        assert_eq!(meta.min_order_size, dec!(0.0001));
        assert_eq!(meta.min_notional, dec!(10));
        assert!(meta.is_active);
    }

    #[test]
    fn parse_market_summary_entry_rejects_broker_suffixed_and_malformed() {
        let venue = VenueId::new("vanta");
        assert!(parse_market_summary_entry(&venue, &market_row("PERP_BTC_USDC_BRK")).is_none());
        assert!(parse_market_summary_entry(&venue, &market_row("PERP__USDC")).is_none());
        assert!(parse_market_summary_entry(&venue, &market_row("SPOT_BTC_USDC")).is_none());

        let mut missing_tick = market_row("PERP_ETH_USDC");
        missing_tick.as_object_mut().unwrap().remove("base_tick");
        assert!(parse_market_summary_entry(&venue, &missing_tick).is_none());

        let mut missing_notional = market_row("PERP_ETH_USDC");
        missing_notional
            .as_object_mut()
            .unwrap()
            .remove("min_notional");
        assert!(parse_market_summary_entry(&venue, &missing_notional).is_none());
    }

    #[test]
    fn parse_orderbook_data_reads_string_levels_and_allows_empty_sides() {
        let venue = VenueId::new("vanta");
        let data = serde_json::json!({
            "symbol": "PERP_BTC_USDC",
            "bids": [{"price": "64200.0", "quantity": "0.5"}],
            "asks": [],
            "ts": 1757779200000i64
        });
        let book = parse_orderbook_data(&venue, "BTC-USDC", "PERP_BTC_USDC", &data)
            .expect("orderbook parses");
        assert_eq!(book.bids.len(), 1);
        assert_eq!(book.bids[0].price, dec!(64200.0));
        assert_eq!(book.bids[0].size, dec!(0.5));
        assert!(book.asks.is_empty());
    }

    #[test]
    fn parse_orderbook_data_fails_closed_on_symbol_mismatch() {
        let venue = VenueId::new("vanta");
        let data = serde_json::json!({
            "symbol": "PERP_ETH_USDC",
            "bids": [{"price": "3000.0", "quantity": "1.0"}],
            "asks": [{"price": "3001.0", "quantity": "1.0"}]
        });
        assert!(parse_orderbook_data(&venue, "BTC-USDC", "PERP_BTC_USDC", &data).is_err());
    }

    #[test]
    fn ticker_and_funding_fail_closed_on_missing_fields() {
        let venue = VenueId::new("vanta");
        let wrap = |row: serde_json::Value| serde_json::json!({ "markets": [row] });

        let good = wrap(market_row("PERP_BTC_USDC"));
        let ticker = parse_ticker_data(&venue, "BTC-USDC", "PERP_BTC_USDC", &good).expect("ticker");
        assert_eq!(ticker.bid, dec!(64199.5));
        assert_eq!(ticker.ask, dec!(64205.0));
        assert_eq!(ticker.last, dec!(64202.25));
        let funding =
            parse_funding_data(&venue, "BTC-USDC", "PERP_BTC_USDC", &good).expect("funding");
        assert_eq!(funding.rate, dec!(0.00012));
        assert_eq!(funding.predicted_rate, Some(dec!(0.00015)));
        assert_eq!(
            funding.next_funding_time,
            DateTime::from_timestamp_millis(1757779200000).unwrap()
        );

        let mut no_bid = market_row("PERP_BTC_USDC");
        no_bid.as_object_mut().unwrap().remove("bid_price");
        assert!(parse_ticker_data(&venue, "BTC-USDC", "PERP_BTC_USDC", &wrap(no_bid)).is_err());

        let mut no_ask = market_row("PERP_BTC_USDC");
        no_ask.as_object_mut().unwrap().remove("ask_price");
        assert!(parse_ticker_data(&venue, "BTC-USDC", "PERP_BTC_USDC", &wrap(no_ask)).is_err());

        let mut no_funding = market_row("PERP_BTC_USDC");
        no_funding
            .as_object_mut()
            .unwrap()
            .remove("last_funding_rate");
        assert!(
            parse_funding_data(&venue, "BTC-USDC", "PERP_BTC_USDC", &wrap(no_funding)).is_err()
        );

        let mut no_next = market_row("PERP_BTC_USDC");
        no_next.as_object_mut().unwrap().remove("next_funding_time");
        assert!(parse_funding_data(&venue, "BTC-USDC", "PERP_BTC_USDC", &wrap(no_next)).is_err());

        let absent = wrap(serde_json::json!({"symbol": "PERP_ETH_USDC"}));
        assert!(parse_ticker_data(&venue, "BTC-USDC", "PERP_BTC_USDC", &absent).is_err());
    }

    #[test]
    fn vanta_symbol_mapping_normalizes_perp_and_rejects_spot() {
        assert_eq!(
            VantaAdapter::to_vanta_symbol("BTC-USDC").unwrap(),
            "PERP_BTC_USDC"
        );
        assert_eq!(
            VantaAdapter::to_vanta_symbol("PERP_ETH_USDC").unwrap(),
            "PERP_ETH_USDC"
        );
        assert!(VantaAdapter::to_vanta_symbol("BTC-SPOT").is_err());
        assert!(VantaAdapter::to_vanta_symbol("eth/spot").is_err());
        assert!(VantaAdapter::to_vanta_symbol("SPOT_BTC_USDC").is_err());
        assert!(VantaAdapter::to_vanta_symbol("BTC-USD").is_err());
        assert!(VantaAdapter::to_vanta_symbol("BTC-SOL").is_err());
        assert!(VantaAdapter::to_vanta_symbol("BTC-ETH").is_err());
        assert!(VantaAdapter::to_vanta_symbol("PERP_BTC_USD").is_err());
        assert!(VantaAdapter::to_vanta_symbol("PERP_BTC_SOL").is_err());
    }

    #[test]
    fn decode_and_sign_orderly_message_works() {
        // 32-byte test key
        let raw_key = [7u8; 32];
        let base58_key = bs58::encode(&raw_key).into_string();
        let hex_key = hex::encode(raw_key);

        let decoded_b58 = decode_orderly_secret(&base58_key).expect("decode base58");
        assert_eq!(decoded_b58, raw_key);

        let decoded_hex = decode_orderly_secret(&hex_key).expect("decode hex");
        assert_eq!(decoded_hex, raw_key);

        let decoded_prefixed =
            decode_orderly_secret(&format!("ed25519:{base58_key}")).expect("decode prefixed");
        assert_eq!(decoded_prefixed, raw_key);

        let sig = sign_orderly_message(&base58_key, "1710000000000GET/v1/positions")
            .expect("sign message");
        assert!(!sig.is_empty());
        assert!(!sig.contains('+'));
        assert!(!sig.contains('/'));
        assert!(!sig.contains('='));
    }

    #[test]
    fn parse_orderly_account_data_reads_positions_and_collateral() {
        let venue = VenueId::new("vanta");
        let data = serde_json::json!({
            "total_collateral_value": "1250.50",
            "free_collateral": "1000.00",
            "total_maintenance_margin": "50.00",
            "rows": [
                {
                    "symbol": "PERP_BTC_USDC",
                    "position_qty": "0.15",
                    "average_open_price": "65000.0",
                    "mark_price": "65500.0",
                    "unsettled_pnl": "75.00",
                    "leverage": "5"
                },
                {
                    "symbol": "PERP_ETH_USDC",
                    "position_qty": "-1.5",
                    "average_open_price": "3500.0",
                    "mark_price": "3480.0",
                    "unsettled_pnl": "30.00",
                    "leverage": "10"
                },
                {
                    "symbol": "PERP_SOL_USDC",
                    "position_qty": "0",
                    "average_open_price": "140.0"
                }
            ]
        });

        let snapshot = parse_orderly_account_data(&venue, &data).expect("snapshot parses");
        assert_eq!(snapshot.venue, venue);
        assert_eq!(snapshot.equity, dec!(1250.50));
        assert_eq!(snapshot.free_margin, dec!(1000.00));
        assert_eq!(snapshot.maintenance_margin, dec!(50.00));
        assert_eq!(snapshot.positions.len(), 2);

        let btc = &snapshot.positions[0];
        assert_eq!(btc.symbol, "PERP_BTC_USDC");
        assert_eq!(btc.side, Side::Buy);
        assert_eq!(btc.quantity, dec!(0.15));
        assert_eq!(btc.entry_price, dec!(65000.0));
        assert_eq!(btc.mark_price, dec!(65500.0));
        assert_eq!(btc.unrealized_pnl, dec!(75.00));
        assert_eq!(btc.leverage, dec!(5));

        let eth = &snapshot.positions[1];
        assert_eq!(eth.symbol, "PERP_ETH_USDC");
        assert_eq!(eth.side, Side::Sell);
        assert_eq!(eth.quantity, dec!(1.5));
        assert_eq!(eth.entry_price, dec!(3500.0));
        assert_eq!(eth.mark_price, dec!(3480.0));
        assert_eq!(eth.unrealized_pnl, dec!(30.00));
        assert_eq!(eth.leverage, dec!(10));
    }

    #[test]
    fn account_position_for_maps_orderly_perp_symbol() {
        let adapter = VantaAdapter::new(None, None, None, None);
        let data = serde_json::json!({
            "total_collateral_value": "100",
            "rows": [{
                "symbol": "PERP_BTC_USDC",
                "position_qty": "0.15",
                "average_open_price": "65000.0",
                "mark_price": "65500.0"
            }]
        });
        let account = parse_orderly_account_data(&VenueId::new("vanta"), &data).unwrap();
        let pos = adapter
            .account_position_for(&account, "BTC-USDC")
            .unwrap()
            .expect("PERP_BTC_USDC should resolve");
        assert_eq!(pos.symbol, "BTC-USDC");
        assert_eq!(pos.quantity, dec!(0.15));
        assert!(adapter
            .account_position_for(&account, "ETH-USDC")
            .unwrap()
            .is_none());
    }

    #[test]
    fn parse_market_summary_volumes_extracts_notional() {
        let data = serde_json::json!({
            "markets": [
                {
                    "symbol": "PERP_BTC_USDC",
                    "24h_amount": "50000000.0",
                    "24h_volume": "650.0",
                    "mark_price": "77000.0"
                },
                {
                    "symbol": "PERP_ETH_USDC",
                    "24h_amount": "0.0",
                    "24h_volume": "100.0",
                    "mark_price": "2500.0"
                },
                {
                    "symbol": "PERP_SOL_USDC",
                    "24h_amount": "0.0",
                    "24h_volume": "0.0",
                    "mark_price": "100.0"
                }
            ]
        });
        let vols = parse_market_summary_volumes(&data);
        assert_eq!(vols.get("BTC-USDC"), Some(&dec!(50000000.0)));
        assert_eq!(vols.get("ETH-USDC"), Some(&dec!(250000.0)));
        assert_eq!(vols.get("SOL-USDC"), None);
    }
}
