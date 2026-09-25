use crate::rate_limiter::AccountRequestCoordinator;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::{AccountSnapshot, Balance, Position};
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{dec, Decimal, OrderStatus, OrderType, Side, TimeInForce, VenueId};
use async_trait::async_trait;
use chrono::Utc;
use parking_lot::RwLock;
use reqwest::Client;
use std::collections::HashMap;
use zeroize::Zeroizing;

/// Alpaca Trading API adapter (https://docs.alpaca.markets).
///
/// Alpaca is a US broker serving **crypto spot** pairs and US equities —
/// there are no perpetuals and no funding rates, so those trait methods
/// fail closed. Order entry is `POST /v2/orders` on the trading host;
/// crypto market data (real L2 books, quotes, trades, snapshots) lives on
/// `data.alpaca.markets`.
///
/// Two environments exist and are honest: production `api.alpaca.markets`
/// and Alpaca's real sandbox `paper-api.alpaca.markets` (independent
/// account + keys — venue-side paper fills, not local simulation). The
/// desktop state layer constructs `live_alpaca`/`tn_alpaca` accordingly.
#[allow(dead_code)]
pub struct AlpacaAdapter {
    venue_id: VenueId,
    client: Client,
    /// Trading host — api.alpaca.markets or paper-api.alpaca.markets.
    trade_url: String,
    /// Market-data host — data.alpaca.markets for both environments.
    data_url: String,
    /// APCA-API-KEY-ID header value.
    key_id: RwLock<Option<Zeroizing<String>>>,
    /// APCA-API-SECRET-KEY header value.
    secret: RwLock<Option<Zeroizing<String>>>,
    rate_limiter: AccountRequestCoordinator,
}

/// Crypto fee schedule is volume-tiered; Tier-1 (0–$100k/30d) is the
/// honest default: 15 maker / 25 taker bps. Equities are commission-free
/// but no cross-venue leg exists for them here.
const ALPACA_CRYPTO_MAKER_BPS: u32 = 15;
const ALPACA_CRYPTO_TAKER_BPS: u32 = 25;
/// Alpaca crypto minimum order is ~$1 notional; asset rows may carry a
/// tighter per-asset `min_order_size` which wins when present.
const ALPACA_CRYPTO_MIN_NOTIONAL_USD: u32 = 1;

fn dec_from_json(v: Option<&serde_json::Value>) -> Option<Decimal> {
    match v? {
        serde_json::Value::String(s) => Decimal::from_str_exact(s).ok(),
        serde_json::Value::Number(n) => Decimal::from_str_exact(&n.to_string()).ok(),
        _ => None,
    }
}

fn decimals_from_step(step: Decimal) -> u32 {
    if step <= dec!(0) {
        return 0;
    }
    let mut d = 0u32;
    let mut v = step;
    while d < 12 && v.fract() > dec!(0) {
        v *= dec!(10);
        d += 1;
    }
    d
}

impl AlpacaAdapter {
    pub fn new(
        trade_url: Option<String>,
        data_url: Option<String>,
        key_id: Option<String>,
        secret: Option<String>,
    ) -> Self {
        let trade = trade_url.unwrap_or_else(|| "https://api.alpaca.markets".to_string());
        let data = data_url.unwrap_or_else(|| "https://data.alpaca.markets".to_string());
        Self {
            venue_id: VenueId::new("alpaca"),
            client: {
                let mut headers = reqwest::header::HeaderMap::new();
                headers.insert(reqwest::header::ACCEPT, "application/json".parse().unwrap());
                Client::builder()
                    .timeout(std::time::Duration::from_secs(15))
                    .default_headers(headers)
                    .build()
                    .unwrap_or_default()
            },
            trade_url: trade.trim_end_matches('/').to_string(),
            data_url: data.trim_end_matches('/').to_string(),
            key_id: RwLock::new(key_id.map(Zeroizing::new)),
            secret: RwLock::new(secret.map(Zeroizing::new)),
            rate_limiter: AccountRequestCoordinator::new("alpaca", 200, 3000, 500, 15000),
        }
    }

    /// Paper environment — Alpaca's real sandbox host. Market data stays
    /// on the shared data host (identical feed for both environments).
    pub fn for_paper(key_id: Option<String>, secret: Option<String>) -> Self {
        Self::new(
            Some("https://paper-api.alpaca.markets".to_string()),
            None,
            key_id,
            secret,
        )
    }

    pub fn update_credentials(&self, key_id: Option<String>, secret: Option<String>) {
        *self.key_id.write() = key_id.map(Zeroizing::new);
        *self.secret.write() = secret.map(Zeroizing::new);
    }

    fn creds(&self) -> anyhow::Result<(String, String)> {
        let k = self.key_id.read().clone();
        let s = self.secret.read().clone();
        match (k, s) {
            (Some(k), Some(s)) if !k.trim().is_empty() && !s.trim().is_empty() => {
                Ok((k.to_string(), s.to_string()))
            }
            _ => anyhow::bail!("Alpaca API credentials are not configured or vault is locked"),
        }
    }

    fn is_crypto_symbol(symbol: &str) -> bool {
        symbol.contains('/')
    }

    /// Canonical `BTC-USD` / `AAPL` → Alpaca `BTC/USD` / `AAPL`.
    /// Crypto venue symbols always carry the `/` quote separator; an
    /// equity ticker passes through uppercase-validated.
    pub fn to_venue_symbol(symbol: &str) -> anyhow::Result<String> {
        let s = symbol.trim();
        if s.contains('/') {
            // Already venue form — keep if it parses as BASE/QUOTE.
            let mut it = s.split('/');
            let (b, q) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
            if it.next().is_none() && !b.is_empty() && !q.is_empty() {
                return Ok(format!("{}/{}", b.to_uppercase(), q.to_uppercase()));
            }
            anyhow::bail!("malformed Alpaca symbol '{symbol}'");
        }
        if let Some((b, q)) = s.split_once('-') {
            if !b.is_empty() && !q.is_empty() {
                return Ok(format!("{}/{}", b.to_uppercase(), q.to_uppercase()));
            }
        }
        if s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') && !s.is_empty() {
            return Ok(s.to_uppercase());
        }
        anyhow::bail!("unsupported Alpaca symbol '{symbol}'")
    }

    /// Alpaca position symbols for crypto arrive WITHOUT the separator
    /// (`BTCUSD`); orders use `BTC/USD`. Split before a known quote so
    /// position↔signal matching resolves the same instrument.
    fn normalize_position_symbol(asset_class: &str, symbol: &str) -> String {
        if asset_class == "crypto" && !symbol.contains('/') {
            for quote in ["USDT", "USDC", "USD", "BTC", "ETH"] {
                if let Some(base) = symbol.strip_suffix(quote) {
                    if !base.is_empty() {
                        return format!("{base}/{quote}");
                    }
                }
            }
        }
        symbol.to_string()
    }

    fn map_status(raw: &str, filled_qty: Decimal) -> anyhow::Result<OrderStatus> {
        match raw {
            "filled" => Ok(OrderStatus::Filled),
            "canceled" => Ok(OrderStatus::Cancelled),
            "expired" => Ok(OrderStatus::Expired),
            "rejected" | "stopped" | "suspended" => Ok(OrderStatus::Rejected),
            // Day-order session close: terminal, unfilled.
            "done_for_day" => Ok(OrderStatus::Cancelled),
            "partially_filled" => Ok(OrderStatus::PartiallyFilled),
            // Cancel/replace round-trips keep the order working.
            "pending_cancel" | "pending_replace" => Ok(if filled_qty > dec!(0) {
                OrderStatus::PartiallyFilled
            } else {
                OrderStatus::New
            }),
            "new" | "accepted" | "pending_new" | "accepted_for_bidding" | "calculated" => {
                Ok(OrderStatus::New)
            }
            other => anyhow::bail!("unmapped Alpaca order status '{other}'"),
        }
    }

    fn parse_order(&self, v: &serde_json::Value) -> anyhow::Result<OrderResponse> {
        let order_id = v
            .get("id")
            .and_then(|x| x.as_str())
            .ok_or_else(|| anyhow::anyhow!("Alpaca order row missing id"))?
            .to_string();
        let raw_status = v
            .get("status")
            .and_then(|x| x.as_str())
            .ok_or_else(|| anyhow::anyhow!("Alpaca order row missing status"))?;
        let qty = dec_from_json(v.get("qty"))
            .or_else(|| dec_from_json(v.get("notional")))
            .unwrap_or(dec!(0));
        let filled = dec_from_json(v.get("filled_qty")).unwrap_or(dec!(0));
        let remaining = (qty - filled).max(dec!(0));
        let status = Self::map_status(raw_status, filled)?;
        Ok(OrderResponse {
            order_id,
            client_order_id: v
                .get("client_order_id")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            venue: self.venue_id.clone(),
            symbol: v
                .get("symbol")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            status,
            original_qty: qty,
            filled_qty: filled,
            remaining_qty: remaining,
            avg_fill_price: dec_from_json(v.get("filled_avg_price")),
            // Alpaca does not report per-order commission — crypto fees are
            // an end-of-day ledger entry. None, never a fabricated zero.
            fee: None,
            fee_currency: String::new(),
            raw_response: Some(v.to_string()),
            timestamp: Utc::now(),
        })
    }

    async fn request(
        &self,
        method: &str,
        url: &str,
        body: Option<&serde_json::Value>,
        authed: bool,
    ) -> anyhow::Result<serde_json::Value> {
        self.rate_limiter.acquire_signed().await?;
        let mut req = match method {
            "GET" => self.client.get(url),
            "POST" => self.client.post(url),
            "DELETE" => self.client.delete(url),
            _ => anyhow::bail!("unsupported method {method}"),
        };
        if authed {
            let (k, s) = self.creds()?;
            req = req
                .header("APCA-API-KEY-ID", k.as_str())
                .header("APCA-API-SECRET-KEY", s.as_str());
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                self.rate_limiter.record_failure(false);
                return Err(e.into());
            }
        };
        let status = resp.status();
        if !status.is_success() {
            let is_429 = status == reqwest::StatusCode::TOO_MANY_REQUESTS;
            let snippet: String = resp
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(512)
                .collect();
            self.rate_limiter.record_failure(is_429);
            anyhow::bail!("Alpaca HTTP {status}: {snippet}");
        }
        // 204 (cancel) and empty bodies are valid successes.
        let text = resp.text().await.unwrap_or_default();
        self.rate_limiter.record_success_path(url, text.clone());
        if text.trim().is_empty() {
            return Ok(serde_json::json!({}));
        }
        Ok(serde_json::from_str(&text)?)
    }

    async fn data_get(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        // Crypto market data requires the same key headers on the data
        // host — a 401 is the honest answer when creds are absent.
        let url = format!("{}{}", self.data_url, path);
        self.request("GET", &url, None, self.has_live_credentials())
            .await
    }

    async fn trade_request(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> anyhow::Result<serde_json::Value> {
        let url = format!("{}{}", self.trade_url, path);
        self.request(method, &url, body, true).await
    }
}

#[async_trait]
impl ExchangeAdapter for AlpacaAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Alpaca"
    }

    fn rest_base_url(&self) -> Option<String> {
        Some(self.trade_url.clone())
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        // Crypto spot pairs only — the venue's equity universe has no
        // counterparty venue here, so listing it would be pure scan noise.
        let v = self
            .trade_request("GET", "/v2/assets?status=active&asset_class=crypto", None)
            .await?;
        let rows = v.as_array().cloned().unwrap_or_default();
        let mut out = Vec::new();
        for row in rows {
            let tradable = row
                .get("tradable")
                .and_then(|t| t.as_bool())
                .unwrap_or(false);
            let sym = match row.get("symbol").and_then(|s| s.as_str()) {
                Some(s) => s,
                None => continue,
            };
            let canonical = sym.replace('/', "-");
            let (base_ccy, quote_ccy) = canonical
                .split_once('-')
                .map(|(b, q)| (b.to_string(), q.to_string()))
                .unwrap_or_else(|| (canonical.clone(), "USD".to_string()));
            let min_size = dec_from_json(row.get("min_order_size")).unwrap_or(dec!(0.0001));
            let tick = dec_from_json(row.get("price_increment")).unwrap_or(dec!(0.01));
            let step = dec_from_json(row.get("min_trade_increment")).unwrap_or(min_size);
            out.push(MarketMetadata {
                symbol: Symbol::spot(base_ccy, quote_ccy),
                venue: self.venue_id.clone(),
                venue_symbol: sym.to_string(),
                price_decimals: decimals_from_step(tick),
                size_decimals: decimals_from_step(step),
                min_order_size: min_size,
                min_notional: Decimal::from(ALPACA_CRYPTO_MIN_NOTIONAL_USD),
                maker_fee_bps: Decimal::from(ALPACA_CRYPTO_MAKER_BPS),
                taker_fee_bps: Decimal::from(ALPACA_CRYPTO_TAKER_BPS),
                contract_size: dec!(1),
                is_active: tradable,
            });
        }
        Ok(out)
    }

    async fn ping(&self) -> anyhow::Result<()> {
        // Public clock endpoint — proves the trade host is reachable
        // without burning an authed call.
        let url = format!("{}/v2/clock", self.trade_url);
        let _ = self.request("GET", &url, None, false).await?;
        Ok(())
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let venue_sym = Self::to_venue_symbol(symbol)?;
        if Self::is_crypto_symbol(&venue_sym) {
            let v = self
                .data_get(&format!(
                    "/v1beta3/crypto/us/latest/orderbooks?symbols={}",
                    urlencoding(&venue_sym)
                ))
                .await?;
            let book = v
                .get("orderbooks")
                .and_then(|o| o.get(&venue_sym))
                .ok_or_else(|| anyhow::anyhow!("Alpaca returned no orderbook for {venue_sym}"))?;
            let bids = parse_book_side(book.get("b"));
            let asks = parse_book_side(book.get("a"));
            if bids.is_empty() || asks.is_empty() {
                anyhow::bail!("Alpaca orderbook for {venue_sym} is empty");
            }
            return Ok(OrderBook {
                venue: self.venue_id.clone(),
                symbol: symbol.to_string(),
                bids,
                asks,
                timestamp: Utc::now(),
                sequence: 0,
            });
        }
        // Equities: Alpaca serves top-of-quote only (no public L2) — a
        // single-level book is the honest shape, never fabricated depth.
        let v = self
            .data_get(&format!(
                "/v2/stocks/{}/quotes/latest",
                urlencoding(&venue_sym)
            ))
            .await?;
        let q = v
            .get("quote")
            .ok_or_else(|| anyhow::anyhow!("Alpaca returned no latest quote for {venue_sym}"))?;
        let (bp, bs) = (dec_from_json(q.get("bp")), dec_from_json(q.get("bs")));
        let (ap, asz) = (dec_from_json(q.get("ap")), dec_from_json(q.get("as")));
        let (Some(bp), Some(bs), Some(ap), Some(asz)) = (bp, bs, ap, asz) else {
            anyhow::bail!("Alpaca quote for {venue_sym} missing bid/ask");
        };
        Ok(OrderBook {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            bids: vec![BookLevel {
                price: bp,
                size: bs,
            }],
            asks: vec![BookLevel {
                price: ap,
                size: asz,
            }],
            timestamp: Utc::now(),
            sequence: 0,
        })
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let venue_sym = Self::to_venue_symbol(symbol)?;
        if Self::is_crypto_symbol(&venue_sym) {
            let v = self
                .data_get(&format!(
                    "/v1beta3/crypto/us/snapshots?symbols={}",
                    urlencoding(&venue_sym)
                ))
                .await?;
            let snap = v
                .get("snapshots")
                .and_then(|s| s.get(&venue_sym))
                .ok_or_else(|| anyhow::anyhow!("Alpaca returned no snapshot for {venue_sym}"))?;
            let trade = snap.get("latestTrade");
            let quote = snap.get("latestQuote");
            let last = trade.and_then(|t| dec_from_json(t.get("p")));
            let bid = quote.and_then(|q| dec_from_json(q.get("bp")));
            let ask = quote.and_then(|q| dec_from_json(q.get("ap")));
            let (Some(last), Some(bid), Some(ask)) = (last, bid, ask) else {
                anyhow::bail!("Alpaca snapshot for {venue_sym} missing trade/quote fields");
            };
            return Ok(Ticker {
                venue: self.venue_id.clone(),
                symbol: symbol.to_string(),
                bid,
                ask,
                last,
                timestamp: Utc::now(),
            });
        }
        let v = self
            .data_get(&format!("/v2/stocks/{}/snapshot", urlencoding(&venue_sym)))
            .await?;
        let last = v.get("latestTrade").and_then(|t| dec_from_json(t.get("p")));
        let bid = v
            .get("latestQuote")
            .and_then(|q| dec_from_json(q.get("bp")));
        let ask = v
            .get("latestQuote")
            .and_then(|q| dec_from_json(q.get("ap")));
        let (Some(last), Some(bid), Some(ask)) = (last, bid, ask) else {
            anyhow::bail!("Alpaca snapshot for {venue_sym} missing trade/quote fields");
        };
        Ok(Ticker {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            bid,
            ask,
            last,
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rate(&self, _symbol: &str) -> anyhow::Result<FundingRate> {
        anyhow::bail!("Alpaca is a spot broker — no perpetual funding rates exist")
    }

    async fn fetch_funding_rates(&self) -> anyhow::Result<Vec<FundingRate>> {
        // Spot venue: the bulk poll must return empty rather than Err — a
        // failure streak would backoff the whole lane and starve book
        // fetches. Truthful: Alpaca lists zero funding markets.
        Ok(Vec::new())
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        let acct = self.trade_request("GET", "/v2/account", None).await?;
        let positions_v = self.trade_request("GET", "/v2/positions", None).await?;
        let equity = dec_from_json(acct.get("equity")).unwrap_or(dec!(0));
        let cash = dec_from_json(acct.get("cash")).unwrap_or(dec!(0));
        let buying_power = dec_from_json(acct.get("buying_power")).unwrap_or(cash);
        let mut balances = vec![Balance {
            venue: self.venue_id.clone(),
            currency: "USD".to_string(),
            total: cash,
            free: buying_power,
            locked: (cash - buying_power).max(dec!(0)),
        }];
        // Held crypto arrives as positions, not cash balances — surface
        // each as its own balance row too so consumers see the asset.
        let mut positions = Vec::new();
        for p in positions_v.as_array().cloned().unwrap_or_default() {
            let qty = dec_from_json(p.get("qty")).unwrap_or(dec!(0));
            if qty <= dec!(0) {
                continue;
            }
            let sym_raw = p.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
            let asset_class = p.get("asset_class").and_then(|s| s.as_str()).unwrap_or("");
            let sym = Self::normalize_position_symbol(asset_class, sym_raw);
            let entry = dec_from_json(p.get("avg_entry_price")).unwrap_or(dec!(0));
            let mark = dec_from_json(p.get("current_price")).unwrap_or(entry);
            let pnl = dec_from_json(p.get("unrealized_pl")).unwrap_or(dec!(0));
            positions.push(Position {
                venue: self.venue_id.clone(),
                symbol: sym.clone(),
                side: Side::Buy,
                quantity: qty,
                entry_price: entry,
                mark_price: mark,
                unrealized_pnl: pnl,
                realized_pnl: dec!(0),
                liquidation_price: None,
                leverage: dec!(1),
                initial_margin: dec!(0),
                maintenance_margin: dec!(0),
                updated_at: Utc::now(),
            });
            if asset_class == "crypto" {
                if let Some((base, _)) = sym.split_once('/') {
                    balances.push(Balance {
                        venue: self.venue_id.clone(),
                        currency: base.to_string(),
                        total: qty,
                        free: qty,
                        locked: dec!(0),
                    });
                }
            }
        }
        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity,
            free_margin: buying_power,
            maintenance_margin: dec!(0),
            margin_usage_pct: dec!(0),
            balances,
            positions,
            timestamp: Utc::now(),
        })
    }

    async fn fetch_volumes_24h(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        // Crypto daily bars: one request covering every listed pair —
        // Alpaca crypto symbols enumerable from fetch_markets' same table.
        let assets = self
            .trade_request("GET", "/v2/assets?status=active&asset_class=crypto", None)
            .await?;
        let syms: Vec<String> = assets
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|r| {
                        r.get("tradable")
                            .and_then(|t| t.as_bool())
                            .unwrap_or(false)
                            .then(|| {
                                r.get("symbol")
                                    .and_then(|s| s.as_str())
                                    .map(|s| s.to_string())
                            })
                            .flatten()
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut out = HashMap::new();
        if syms.is_empty() {
            return Ok(out);
        }
        // Batch up to 100 symbols per request (Alpaca caps the list).
        for chunk in syms.chunks(100) {
            let v = self
                .data_get(&format!(
                    "/v1beta3/crypto/us/snapshots?symbols={}",
                    chunk.join(",")
                ))
                .await?;
            if let Some(snaps) = v.get("snapshots").and_then(|s| s.as_object()) {
                for (sym, snap) in snaps {
                    if let Some(vol) = snap.get("dailyBar").and_then(|b| dec_from_json(b.get("v")))
                    {
                        out.insert(sym.replace('/', "-"), vol);
                    }
                }
            }
        }
        Ok(out)
    }

    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        if !self.has_live_credentials() {
            anyhow::bail!(
                "Cannot place live order on Alpaca: API credentials are not configured or vault is locked"
            );
        }
        if order.quantity <= dec!(0) {
            anyhow::bail!("Alpaca order quantity must be positive");
        }
        if order.post_only {
            anyhow::bail!(
                "Alpaca has no post-only flag — refusing rather than silently crossing the spread"
            );
        }
        let venue_sym = Self::to_venue_symbol(&order.symbol)?;
        let crypto = Self::is_crypto_symbol(&venue_sym);
        let (ty, tif) = match order.order_type {
            OrderType::Market => ("market", if crypto { "gtc" } else { "day" }),
            OrderType::ImmediateOrCancel => {
                // IOC on Alpaca is a limit order with ioc TIF — a price
                // is mandatory; an unpriced IOC would be a market order.
                ("limit", "ioc")
            }
            OrderType::Limit => ("limit", if crypto { "gtc" } else { "day" }),
            OrderType::PostOnly => {
                anyhow::bail!("Alpaca has no post-only order type")
            }
        };
        // Explicit TIF overrides: crypto accepts gtc|ioc only.
        let tif = match order.time_in_force {
            TimeInForce::GoodTilCancelled => tif,
            TimeInForce::ImmediateOrCancel => "ioc",
            TimeInForce::FillOrKill => {
                if crypto {
                    anyhow::bail!("Alpaca crypto has no FOK — use IOC");
                }
                "fok"
            }
            TimeInForce::PostOnly => anyhow::bail!("Alpaca has no post-only TIF"),
        };
        if crypto && !matches!(tif, "gtc" | "ioc") {
            anyhow::bail!("Alpaca crypto orders accept only gtc/ioc time-in-force");
        }
        if order.reduce_only && crypto {
            // Spot has no short to reduce — a sell is inherently
            // position-reducing and Alpaca rejects oversells natively.
            if order.side != Side::Sell {
                anyhow::bail!("reduce-only buy is meaningless on a spot venue");
            }
        }
        if order.reduce_only && !crypto {
            anyhow::bail!("reduce-only is not supported for Alpaca equities");
        }
        let mut body = serde_json::json!({
            "symbol": venue_sym,
            "qty": order.quantity.normalize().to_string(),
            "side": match order.side { Side::Buy => "buy", Side::Sell => "sell" },
            "type": ty,
            "time_in_force": tif,
        });
        if !order.client_order_id.is_empty() {
            body["client_order_id"] = order.client_order_id.clone().into();
        }
        if ty == "limit" {
            let price = order
                .price
                .ok_or_else(|| anyhow::anyhow!("limit order on Alpaca requires a price"))?;
            if price <= dec!(0) {
                anyhow::bail!("limit price must be positive");
            }
            body["limit_price"] = price.normalize().to_string().into();
        }
        let v = self
            .trade_request("POST", "/v2/orders", Some(&body))
            .await?;
        self.parse_order(&v)
    }

    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
        if !self.has_live_credentials() {
            anyhow::bail!("Cannot cancel on Alpaca: credentials are not configured");
        }
        let order_id = match req.order_id.as_deref().filter(|s| !s.is_empty()) {
            Some(id) => id.to_string(),
            None => {
                // Resolve the durable venue id from the client id — Alpaca
                // cancels by order id only.
                let cid = req
                    .client_order_id
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("cancel needs an order id or client order id")
                    })?;
                let v = self
                    .trade_request("GET", &format!("/v2/orders/by_client_order_id/{cid}"), None)
                    .await?;
                v.get("id")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| anyhow::anyhow!("client order id {cid} resolved no order"))?
                    .to_string()
            }
        };
        let _ = self
            .trade_request("DELETE", &format!("/v2/orders/{order_id}"), None)
            .await?;
        Ok(())
    }

    async fn fetch_order(
        &self,
        _symbol: &str,
        order_id: &str,
        client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        if !self.has_live_credentials() {
            anyhow::bail!("Cannot fetch Alpaca order: credentials are not configured");
        }
        let path = if !order_id.is_empty() {
            format!("/v2/orders/{order_id}")
        } else {
            let cid = client_order_id.filter(|s| !s.is_empty()).ok_or_else(|| {
                anyhow::anyhow!("fetch_order needs an order id or client order id")
            })?;
            format!("/v2/orders/by_client_order_id/{cid}")
        };
        let v = self.trade_request("GET", &path, None).await?;
        self.parse_order(&v)
    }

    async fn set_leverage(&self, _symbol: &str, _leverage: u32) -> anyhow::Result<()> {
        anyhow::bail!("Alpaca is a spot broker — per-symbol leverage does not exist")
    }

    fn supports_reduce_only(&self, _symbol: &str) -> bool {
        false
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

    fn live_order_reconciliation_implemented(&self) -> bool {
        true
    }

    fn has_live_credentials(&self) -> bool {
        self.creds().is_ok()
    }
}

fn parse_book_side(v: Option<&serde_json::Value>) -> Vec<BookLevel> {
    v.and_then(|a| a.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    let p = dec_from_json(r.get("p"))?;
                    let s = dec_from_json(r.get("s"))?;
                    (p > dec!(0) && s > dec!(0)).then_some(BookLevel { price: p, size: s })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Minimal percent-encoder for query params (Alpaca symbols carry "/").
fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~' | ',') {
                c.to_string()
            } else {
                format!("%{:02X}", c as u32)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// One-connection scripted responder — FIFO per path prefix, same
    /// pattern as the Boros stub.
    async fn stub_api(routes: Vec<(&str, Vec<(u16, serde_json::Value)>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let map: HashMap<String, Mutex<VecDeque<(u16, String)>>> = routes
            .into_iter()
            .map(|(p, rs)| {
                (
                    p.to_string(),
                    Mutex::new(rs.into_iter().map(|(s, v)| (s, v.to_string())).collect()),
                )
            })
            .collect();
        let map = std::sync::Arc::new(map);
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let map = map.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 65536];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
                    // Longest prefix wins — "/v2/orders" must not shadow
                    // "/v2/orders/{id}" under HashMap iteration order.
                    let hit = map
                        .iter()
                        .filter(|(prefix, _)| path.starts_with(prefix.as_str()))
                        .max_by_key(|(prefix, _)| prefix.len())
                        .and_then(|(_, q)| q.lock().unwrap().pop_front())
                        .unwrap_or_else(|| (404, "{\"message\":\"unstubbed\"}".to_string()));
                    let resp = format!(
                        "HTTP/1.1 {} OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        hit.0,
                        hit.1.len(),
                        hit.1
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    fn adapter(base: &str) -> AlpacaAdapter {
        let a = AlpacaAdapter::new(Some(base.to_string()), Some(base.to_string()), None, None);
        a.update_credentials(Some("PKTEST123".to_string()), Some("secret".to_string()));
        a
    }

    #[test]
    fn symbol_mapping_normalizes_and_rejects() {
        assert_eq!(
            AlpacaAdapter::to_venue_symbol("BTC-USD").unwrap(),
            "BTC/USD"
        );
        assert_eq!(
            AlpacaAdapter::to_venue_symbol("BTC/USD").unwrap(),
            "BTC/USD"
        );
        assert_eq!(AlpacaAdapter::to_venue_symbol("aapl").unwrap(), "AAPL");
        assert!(AlpacaAdapter::to_venue_symbol("BTC/USD/EXTRA").is_err());
        assert!(AlpacaAdapter::to_venue_symbol("").is_err());
        assert_eq!(
            AlpacaAdapter::normalize_position_symbol("crypto", "BTCUSD"),
            "BTC/USD"
        );
        assert_eq!(
            AlpacaAdapter::normalize_position_symbol("crypto", "ETHUSDT"),
            "ETH/USDT"
        );
        assert_eq!(
            AlpacaAdapter::normalize_position_symbol("us_equity", "AAPL"),
            "AAPL"
        );
    }

    #[test]
    fn order_status_mapping_is_fail_closed() {
        assert_eq!(
            AlpacaAdapter::map_status("filled", dec!(0)).unwrap(),
            OrderStatus::Filled
        );
        assert_eq!(
            AlpacaAdapter::map_status("partially_filled", dec!(1)).unwrap(),
            OrderStatus::PartiallyFilled
        );
        assert_eq!(
            AlpacaAdapter::map_status("pending_cancel", dec!(0)).unwrap(),
            OrderStatus::New
        );
        assert_eq!(
            AlpacaAdapter::map_status("canceled", dec!(0)).unwrap(),
            OrderStatus::Cancelled
        );
        assert_eq!(
            AlpacaAdapter::map_status("done_for_day", dec!(0)).unwrap(),
            OrderStatus::Cancelled
        );
        assert!(AlpacaAdapter::map_status("brand_new_status", dec!(0)).is_err());
    }

    #[tokio::test]
    async fn order_paths_fail_closed_without_credentials() {
        let a = AlpacaAdapter::new(None, None, None, None);
        let err = a
            .place_order(OrderRequest::new_market(
                VenueId::new("alpaca"),
                "BTC-USD",
                Side::Buy,
                dec!(0.01),
            ))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("credentials"));
        assert!(a
            .cancel_order(CancelRequest {
                venue: VenueId::new("alpaca"),
                symbol: "BTC-USD".into(),
                order_id: Some("x".into()),
                client_order_id: None,
            })
            .await
            .is_err());
        assert!(a.fetch_order("BTC-USD", "x", None).await.is_err());
    }

    #[tokio::test]
    async fn spot_unsupported_paths_bail_named() {
        let a = adapter("http://127.0.0.1:1");
        assert!(a.fetch_funding_rate("BTC-USD").await.is_err());
        // Bulk funding poll returns empty (never Err) so the monitor lane
        // isn't starved by a failure streak on a venue with zero funding
        // markets.
        assert!(a.fetch_funding_rates().await.unwrap().is_empty());
        assert!(a.set_leverage("BTC-USD", 2).await.is_err());
        // Post-only cannot be expressed — refuse rather than cross.
        let mut po = OrderRequest::new_limit(
            VenueId::new("alpaca"),
            "BTC-USD",
            Side::Buy,
            dec!(100),
            dec!(0.01),
        );
        po.post_only = true;
        assert!(a
            .place_order(po)
            .await
            .unwrap_err()
            .to_string()
            .contains("post-only"));
        let mut fok =
            OrderRequest::new_market(VenueId::new("alpaca"), "BTC-USD", Side::Buy, dec!(0.01));
        fok.time_in_force = TimeInForce::FillOrKill;
        assert!(a.place_order(fok).await.is_err());
    }

    #[tokio::test]
    async fn markets_parse_crypto_assets() {
        let base = stub_api(vec![(
            "/v2/assets",
            vec![(200, serde_json::json!([
                {"symbol":"BTC/USD","class":"crypto","tradable":true,"fractionable":true,"min_order_size":"0.0001","min_trade_increment":"0.0001","price_increment":"0.01","status":"active"},
                {"symbol":"ETH/USD","class":"crypto","tradable":false,"fractionable":true,"min_order_size":"0.001","min_trade_increment":"0.001","price_increment":"0.01","status":"active"}
            ]))],
        )])
        .await;
        let a = adapter(&base);
        let markets = a.fetch_markets().await.expect("markets");
        assert_eq!(markets.len(), 2);
        assert_eq!(markets[0].venue_symbol, "BTC/USD");
        assert_eq!(markets[0].symbol.canonical(), "BTC-USD");
        assert_eq!(markets[0].taker_fee_bps, dec!(25));
        assert!(markets[0].is_active);
        assert!(!markets[1].is_active, "non-tradable rows stay inactive");
    }

    #[tokio::test]
    async fn crypto_orderbook_parses_l2() {
        let base = stub_api(vec![(
            "/v1beta3/crypto/us/latest/orderbooks",
            vec![(
                200,
                serde_json::json!({"orderbooks":{"BTC/USD":{
                "b":[{"p":65986.96,"s":0.27},{"p":65980.0,"s":1.0}],
                "a":[{"p":66051.62,"s":0.28}],"t":"2024-07-24T07:31:01Z"}}}),
            )],
        )])
        .await;
        let a = adapter(&base);
        let book = a.fetch_orderbook("BTC-USD").await.expect("book");
        assert_eq!(book.bids.len(), 2);
        assert_eq!(book.bids[0].price, dec!(65986.96));
        assert_eq!(book.asks[0].price, dec!(66051.62));
    }

    #[tokio::test]
    async fn order_roundtrip_place_poll_cancel() {
        let placed = serde_json::json!({
            "id":"61e69015-8549-4bfd-b9c3-01e75843f47d","client_order_id":"cid-1",
            "status":"new","symbol":"BTC/USD","qty":"0.01","filled_qty":"0",
            "filled_avg_price":null
        });
        let cancelled = serde_json::json!({
            "id":"61e69015-8549-4bfd-b9c3-01e75843f47d","client_order_id":"cid-1",
            "status":"canceled","symbol":"BTC/USD","qty":"0.01","filled_qty":"0",
            "filled_avg_price":null
        });
        let base = stub_api(vec![
            ("/v2/orders/by_client_order_id", vec![(200, placed.clone())]),
            (
                "/v2/orders/61e69015-8549-4bfd-b9c3-01e75843f47d",
                vec![
                    (200, placed.clone()),
                    (204, serde_json::json!({})),
                    (200, cancelled),
                ],
            ),
            ("/v2/orders", vec![(200, placed.clone())]),
        ])
        .await;
        let a = adapter(&base);
        let resp = a
            .place_order(OrderRequest::new_limit(
                VenueId::new("alpaca"),
                "BTC-USD",
                Side::Buy,
                dec!(60000),
                dec!(0.01),
            ))
            .await
            .expect("place");
        assert_eq!(resp.status, OrderStatus::New);
        let polled = a
            .fetch_order("BTC-USD", "61e69015-8549-4bfd-b9c3-01e75843f47d", None)
            .await
            .expect("poll");
        assert_eq!(polled.status, OrderStatus::New);
        a.cancel_order(CancelRequest {
            venue: VenueId::new("alpaca"),
            symbol: "BTC-USD".into(),
            order_id: Some("61e69015-8549-4bfd-b9c3-01e75843f47d".into()),
            client_order_id: None,
        })
        .await
        .expect("cancel");
        let done = a
            .fetch_order("BTC-USD", "61e69015-8549-4bfd-b9c3-01e75843f47d", None)
            .await
            .expect("post-cancel poll");
        assert_eq!(done.status, OrderStatus::Cancelled);
    }

    #[tokio::test]
    async fn account_maps_equity_positions_and_balances() {
        let base = stub_api(vec![
            ("/v2/account", vec![(200, serde_json::json!({
                "equity":"10250.55","cash":"9500.00","buying_power":"9500.00"
            }))]),
            ("/v2/positions", vec![(200, serde_json::json!([
                {"asset_class":"crypto","symbol":"BTCUSD","qty":"0.011","side":"long","avg_entry_price":"65000","current_price":"66000","market_value":"726","unrealized_pl":"11"},
                {"asset_class":"us_equity","symbol":"AAPL","qty":"2","side":"long","avg_entry_price":"150","current_price":"155","market_value":"310","unrealized_pl":"10"}
            ]))]),
        ])
        .await;
        let a = adapter(&base);
        let acct = a.fetch_account().await.expect("account");
        assert_eq!(acct.equity, dec!(10250.55));
        assert_eq!(acct.free_margin, dec!(9500));
        assert_eq!(acct.positions.len(), 2);
        assert_eq!(acct.positions[0].symbol, "BTC/USD");
        assert_eq!(acct.positions[1].symbol, "AAPL");
        assert!(acct.get_balance("BTC").is_some());
        assert!(acct.get_balance("USD").is_some());
    }

    #[tokio::test]
    async fn http_error_propagates_fail_closed() {
        let base = stub_api(vec![(
            "/v2/account",
            vec![(401, serde_json::json!({"message":"unauthorized"}))],
        )])
        .await;
        let a = adapter(&base);
        let err = a.fetch_account().await.unwrap_err();
        assert!(err.to_string().contains("401"), "got: {err}");
    }
}
