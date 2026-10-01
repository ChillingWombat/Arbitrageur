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

/// Interactive Brokers (IBKR) Client Portal / TWS Gateway API adapter.
///
/// Supports both Live and Paper Trading environments:
/// - In IBKR, paper trading is account-level on the Client Portal Web API / TWS
///   gateway (standard paper account prefix `DU...`, live account `U...`).
/// - Standard Client Portal Web API gateway runs locally at `https://127.0.0.1:5000/v1/api`.
///   Self-signed TLS certificates are accepted for local gateway communication.
/// - Market data and order management route through IBKR's `/iserver` and `/portfolio`
///   REST endpoints.
pub struct IbkrAdapter {
    venue_id: VenueId,
    client: Client,
    /// Authoritative REST gateway endpoint (e.g. `https://127.0.0.1:5000/v1/api`).
    base_url: RwLock<String>,
    /// Configured account id (e.g. `DU1234567` for paper, `U1234567` for live).
    account_id: RwLock<Option<Zeroizing<String>>>,
    /// Whether this adapter instance is scoped to paper trading.
    is_paper: bool,
    /// Known / cached contract ID (conid) mapping per symbol.
    conid_cache: RwLock<HashMap<String, u64>>,
    rate_limiter: AccountRequestCoordinator,
}

impl IbkrAdapter {
    pub const DEFAULT_BASE_URL: &'static str = "https://127.0.0.1:5000/v1/api";

    pub fn new(account_id: Option<String>, base_url: Option<String>) -> Self {
        Self::with_env(account_id, base_url, false)
    }

    /// Paper trading environment: talks to the local gateway with paper account expectations.
    pub fn for_paper(account_id: Option<String>, base_url: Option<String>) -> Self {
        Self::with_env(account_id, base_url, true)
    }

    fn with_env(account_id: Option<String>, base_url: Option<String>, is_paper: bool) -> Self {
        let url = base_url
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| Self::DEFAULT_BASE_URL.to_string());
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(12))
            // IBKR Client Portal gateway generates self-signed certificates for local TLS.
            .danger_accept_invalid_certs(true)
            .user_agent("Arbitrageur/1.0")
            .build()
            .unwrap_or_default();

        let mut initial_conids = HashMap::new();
        // Seed well-known equity & crypto conids for instant resolution
        initial_conids.insert("AAPL".to_string(), 265598);
        initial_conids.insert("MSFT".to_string(), 272093);
        initial_conids.insert("GOOGL".to_string(), 208813719);
        initial_conids.insert("AMZN".to_string(), 3691937);
        initial_conids.insert("NVDA".to_string(), 4815747);
        initial_conids.insert("TSLA".to_string(), 76792991);
        initial_conids.insert("META".to_string(), 107113386);
        initial_conids.insert("SPY".to_string(), 756733);
        initial_conids.insert("QQQ".to_string(), 320227571);
        initial_conids.insert("IWM".to_string(), 9599491);
        if !is_paper {
            initial_conids.insert("BTC".to_string(), 479624278);
            initial_conids.insert("BTC-USD".to_string(), 479624278);
            initial_conids.insert("ETH".to_string(), 479624364);
            initial_conids.insert("ETH-USD".to_string(), 479624364);
        }

        Self {
            venue_id: VenueId::new("ibkr"),
            client,
            base_url: RwLock::new(url.trim_end_matches('/').to_string()),
            account_id: RwLock::new(
                account_id
                    .filter(|a| !a.trim().is_empty())
                    .map(Zeroizing::new),
            ),
            is_paper,
            conid_cache: RwLock::new(initial_conids),
            rate_limiter: AccountRequestCoordinator::new("ibkr", 100, 3000, 500, 15000),
        }
    }

    pub fn is_paper(&self) -> bool {
        self.is_paper
    }

    pub fn set_base_url(&self, url: Option<String>) {
        let u = url
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| Self::DEFAULT_BASE_URL.to_string());
        *self.base_url.write() = u.trim_end_matches('/').to_string();
    }

    pub fn update_credentials(&self, account_id: Option<String>, base_url: Option<String>) {
        *self.account_id.write() = account_id
            .filter(|a| !a.trim().is_empty())
            .map(Zeroizing::new);
        if let Some(u) = base_url.filter(|s| !s.trim().is_empty()) {
            *self.base_url.write() = u.trim_end_matches('/').to_string();
        }
    }

    pub fn account_id(&self) -> Option<String> {
        self.account_id.read().as_ref().map(|a| a.to_string())
    }

    const CRYPTO_BASES: &'static [&'static str] = &[
        "BTC", "ETH", "SOL", "AVAX", "LINK", "LTC", "BCH", "UNI", "AAVE", "DOGE", "MATIC",
    ];

    const TOP_EQUITIES: &'static [&'static str] = &[
        "AAPL", "MSFT", "GOOGL", "AMZN", "NVDA", "TSLA", "META", "SPY", "QQQ", "IWM",
    ];

    pub fn is_crypto_symbol(symbol: &str) -> bool {
        let sym_upper = symbol.to_ascii_uppercase();
        if let Some((b, _)) = sym_upper.split_once('-') {
            return Self::CRYPTO_BASES.contains(&b);
        }
        if let Some((b, _)) = sym_upper.split_once('/') {
            return Self::CRYPTO_BASES.contains(&b);
        }
        Self::CRYPTO_BASES.contains(&sym_upper.as_str())
    }

    pub fn to_venue_symbol(symbol: &str) -> String {
        let s = symbol.trim().to_ascii_uppercase();
        if let Some((b, _)) = s.split_once('-') {
            if Self::CRYPTO_BASES.contains(&b) {
                return b.to_string();
            }
        }
        if let Some((b, _)) = s.split_once('/') {
            if Self::CRYPTO_BASES.contains(&b) {
                return b.to_string();
            }
        }
        s
    }

    pub fn canonical_symbol(venue_symbol: &str) -> String {
        let s = venue_symbol.trim().to_ascii_uppercase();
        if Self::CRYPTO_BASES.contains(&s.as_str()) {
            format!("{s}-USD")
        } else {
            s
        }
    }

    /// Snap limit price according to asset class tick rule:
    /// Equities snap to whole cents ($0.01). Crypto snaps to 2 decimals.
    pub fn snap_limit_price(price: Decimal, side: Side, crypto: bool) -> Decimal {
        if crypto {
            return (price * dec!(100)).round() / dec!(100);
        }
        let cents = (price * dec!(100)).normalize();
        let rounded = match side {
            Side::Buy => cents.floor(),
            Side::Sell => cents.ceil(),
        };
        (rounded / dec!(100)).normalize()
    }

    pub fn map_status(raw: &str, filled_qty: Decimal) -> anyhow::Result<OrderStatus> {
        let clean = raw.trim().to_ascii_lowercase();
        match clean.as_str() {
            "filled" => Ok(OrderStatus::Filled),
            "cancelled" | "canceled" | "inactive" => Ok(OrderStatus::Cancelled),
            "rejected" => Ok(OrderStatus::Rejected),
            "presubmitted" | "submitted" | "pendingsubmit" | "api_pending" => {
                Ok(if filled_qty > dec!(0) {
                    OrderStatus::PartiallyFilled
                } else {
                    OrderStatus::New
                })
            }
            "pendingcancel" => Ok(if filled_qty > dec!(0) {
                OrderStatus::PartiallyFilled
            } else {
                OrderStatus::New
            }),
            other => anyhow::bail!("unmapped IBKR order status '{other}'"),
        }
    }

    /// Helper to resolve the active account id, attempting discovery from the gateway if needed.
    pub async fn resolve_account_id(&self) -> anyhow::Result<String> {
        if let Some(acc) = self.account_id() {
            return Ok(acc);
        }

        // Try discovering available accounts from the gateway
        let base = self.base_url.read().clone();
        let url = format!("{}/portfolio/accounts", base.trim_end_matches('/'));
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!(
                "IBKR account ID not configured and gateway query failed with status {}",
                resp.status()
            );
        }
        let accounts: serde_json::Value = resp.json().await?;
        if let Some(list) = accounts.as_array() {
            for item in list {
                let id = item.get("id").and_then(|x| x.as_str()).or_else(|| {
                    item.get("accountVanilla")
                        .and_then(|v| v.get("account"))
                        .and_then(|a| a.as_str())
                });
                if let Some(id_str) = id {
                    let is_paper_acct = id_str.starts_with("DU") || id_str.starts_with("DF");
                    if (self.is_paper && is_paper_acct) || (!self.is_paper && !is_paper_acct) {
                        let res = id_str.to_string();
                        *self.account_id.write() = Some(Zeroizing::new(res.clone()));
                        return Ok(res);
                    }
                }
            }
            // Fall back to first available account if prefix filter didn't match
            if let Some(first) = list
                .first()
                .and_then(|x| x.get("id"))
                .and_then(|x| x.as_str())
            {
                let res = first.to_string();
                *self.account_id.write() = Some(Zeroizing::new(res.clone()));
                return Ok(res);
            }
        }
        anyhow::bail!("IBKR account ID is not configured and no accounts found on gateway")
    }

    /// Resolve conid for a symbol, consulting cache and querying `/iserver/secdef/search` if needed.
    pub async fn resolve_conid(&self, symbol: &str) -> anyhow::Result<u64> {
        let venue_sym = Self::to_venue_symbol(symbol);
        if let Some(conid) = self.conid_cache.read().get(&venue_sym).copied() {
            return Ok(conid);
        }

        let base = self.base_url.read().clone();
        let url = format!(
            "{}/iserver/secdef/search?symbol={}",
            base.trim_end_matches('/'),
            urlencoding::encode(&venue_sym)
        );
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!(
                "Failed to search contract for {venue_sym} on IBKR (HTTP {})",
                resp.status()
            );
        }
        let list: serde_json::Value = resp.json().await?;
        if let Some(items) = list.as_array() {
            for item in items {
                if let Some(conid) = item.get("conid").and_then(|x| x.as_u64()) {
                    self.conid_cache.write().insert(venue_sym.clone(), conid);
                    return Ok(conid);
                }
            }
        }
        anyhow::bail!("No contract conid found for symbol '{venue_sym}' on IBKR")
    }

    /// Parse account summary payload into Balance.
    pub fn parse_summary_balance(&self, summary_json: &serde_json::Value) -> Option<Balance> {
        let parse_dec_str = |s: &str| -> Option<Decimal> {
            let clean = s.trim().trim_start_matches('$').replace(',', "");
            Decimal::from_str_exact(&clean).ok()
        };

        let extract_num = |v: Option<&serde_json::Value>| -> Option<Decimal> {
            match v? {
                serde_json::Value::Number(n) => Decimal::from_str_exact(&n.to_string()).ok(),
                serde_json::Value::String(s) => parse_dec_str(s),
                serde_json::Value::Object(o) => {
                    let from_field = |field: &str| -> Option<Decimal> {
                        match o.get(field)? {
                            serde_json::Value::Number(n) => {
                                Decimal::from_str_exact(&n.to_string()).ok()
                            }
                            serde_json::Value::String(s) => parse_dec_str(s),
                            _ => None,
                        }
                    };
                    from_field("amount").or_else(|| from_field("value"))
                }
                _ => None,
            }
        };

        if let Some(obj) = summary_json.as_object() {
            let total = extract_num(obj.get("netliquidation"))
                .or_else(|| extract_num(obj.get("netliquidationvalue")))
                .or_else(|| extract_num(obj.get("totalcashvalue")))
                .or_else(|| extract_num(obj.get("netliquidation-s")))
                .or_else(|| extract_num(obj.get("netliquidationvalue-s")))
                .or_else(|| extract_num(obj.get("totalcashvalue-s")))
                .or_else(|| extract_num(obj.get("cashbalance")))
                .or_else(|| extract_num(obj.get("cashbalance-s")))
                .unwrap_or(dec!(0));
            let free = extract_num(obj.get("availablefunds"))
                .or_else(|| extract_num(obj.get("buyingpower")))
                .or_else(|| extract_num(obj.get("availablefunds-s")))
                .or_else(|| extract_num(obj.get("buyingpower-s")))
                .unwrap_or(total);
            let currency = obj
                .get("netliquidation")
                .or_else(|| obj.get("totalcashvalue"))
                .or_else(|| obj.get("netliquidationvalue"))
                .or_else(|| obj.get("netliquidationvalue-s"))
                .and_then(|v| v.get("currency"))
                .and_then(|c| c.as_str())
                .unwrap_or("USD")
                .to_string();
            return Some(Balance {
                venue: self.venue_id.clone(),
                currency,
                total,
                free,
                locked: (total - free).max(dec!(0)),
            });
        }

        if let Some(arr) = summary_json.as_array() {
            let mut total = dec!(0);
            let mut free = dec!(0);
            let mut currency = "USD".to_string();
            for item in arr {
                let tag = item.get("tag").and_then(|t| t.as_str()).unwrap_or("");
                if tag.eq_ignore_ascii_case("NetLiquidation")
                    || tag.eq_ignore_ascii_case("TotalCashValue")
                {
                    if let Some(val) = extract_num(item.get("value").or_else(|| item.get("amount")))
                    {
                        total = val;
                    }
                    if let Some(curr) = item.get("currency").and_then(|c| c.as_str()) {
                        currency = curr.to_string();
                    }
                }
                if tag.eq_ignore_ascii_case("BuyingPower")
                    || tag.eq_ignore_ascii_case("AvailableFunds")
                {
                    if let Some(val) = extract_num(item.get("value").or_else(|| item.get("amount")))
                    {
                        free = val;
                    }
                }
            }
            return Some(Balance {
                venue: self.venue_id.clone(),
                currency,
                total,
                free: if free > dec!(0) { free } else { total },
                locked: (total - free).max(dec!(0)),
            });
        }

        None
    }

    /// Parse positions array from IBKR `/portfolio/{accountId}/positions/0`.
    pub fn parse_positions(&self, positions_json: &serde_json::Value) -> Vec<Position> {
        let extract_num = |v: Option<&serde_json::Value>| -> Option<Decimal> {
            match v? {
                serde_json::Value::Number(n) => Decimal::from_str_exact(&n.to_string()).ok(),
                serde_json::Value::String(s) => Decimal::from_str_exact(s).ok(),
                _ => None,
            }
        };

        let mut out = Vec::new();
        if let Some(arr) = positions_json.as_array() {
            for item in arr {
                let raw_pos = extract_num(item.get("position")).unwrap_or(dec!(0));
                if raw_pos == dec!(0) {
                    continue;
                }
                let sym_raw = item
                    .get("ticker")
                    .and_then(|x| x.as_str())
                    .or_else(|| item.get("contractDesc").and_then(|x| x.as_str()))
                    .unwrap_or("");
                if sym_raw.is_empty() {
                    continue;
                }

                let symbol = Self::canonical_symbol(sym_raw);
                let (side, qty) = if raw_pos > dec!(0) {
                    (Side::Buy, raw_pos)
                } else {
                    (Side::Sell, -raw_pos)
                };
                let entry_price = extract_num(item.get("avgCost")).unwrap_or(dec!(0));
                let current_price = extract_num(item.get("mktPrice")).unwrap_or(entry_price);
                let unrealized_pnl = extract_num(item.get("unrealizedPnl")).unwrap_or(dec!(0));

                out.push(Position {
                    venue: self.venue_id.clone(),
                    symbol,
                    side,
                    quantity: qty,
                    entry_price,
                    mark_price: current_price,
                    unrealized_pnl,
                    realized_pnl: extract_num(item.get("realizedPnl")).unwrap_or(dec!(0)),
                    liquidation_price: None,
                    leverage: dec!(1.0),
                    initial_margin: dec!(0),
                    maintenance_margin: dec!(0),
                    updated_at: Utc::now(),
                });
            }
        }
        out
    }
}

#[async_trait]
impl ExchangeAdapter for IbkrAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Interactive Brokers"
    }

    fn rest_base_url(&self) -> Option<String> {
        Some(self.base_url.read().clone())
    }

    async fn ping(&self) -> anyhow::Result<()> {
        let base = self.base_url.read().clone();
        let tickle_url = format!("{}/tickle", base.trim_end_matches('/'));
        // Modern Client Portal gateway keepalive probe is GET /tickle
        if let Ok(resp) = self.client.get(&tickle_url).send().await {
            if resp.status().is_success() {
                return Ok(());
            }
        }
        // Fallback to GET /iserver/auth/status
        let auth_url = format!("{}/iserver/auth/status", base.trim_end_matches('/'));
        let resp = self.client.get(&auth_url).send().await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            anyhow::bail!(
                "IBKR gateway connectivity probe failed (HTTP {})",
                resp.status()
            )
        }
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let mut out = Vec::new();

        // 1. Top Equities / ETFs
        for &sym in Self::TOP_EQUITIES {
            out.push(MarketMetadata {
                symbol: Symbol::spot(sym.to_string(), "USD".to_string()),
                venue: self.venue_id.clone(),
                venue_symbol: sym.to_string(),
                price_decimals: 2,
                size_decimals: 0,
                min_order_size: dec!(1),
                min_notional: dec!(0),
                maker_fee_bps: dec!(1),
                taker_fee_bps: dec!(1),
                contract_size: dec!(1),
                is_active: true,
            });
        }

        // 2. Crypto Spot Pairs (only for live trading accounts with Paxos permissions)
        if !self.is_paper {
            for &c in Self::CRYPTO_BASES {
                out.push(MarketMetadata {
                    symbol: Symbol::spot(c.to_string(), "USD".to_string()),
                    venue: self.venue_id.clone(),
                    venue_symbol: c.to_string(),
                    price_decimals: 2,
                    size_decimals: 4,
                    min_order_size: dec!(0.0001),
                    min_notional: dec!(1),
                    maker_fee_bps: dec!(1),
                    taker_fee_bps: dec!(1),
                    contract_size: dec!(1),
                    is_active: true,
                });
            }
        }

        Ok(out)
    }

    async fn fetch_market_prices(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        // Query snapshot for known conids
        let conids: Vec<String> = self
            .conid_cache
            .read()
            .values()
            .map(|id| id.to_string())
            .collect();
        if conids.is_empty() {
            return Ok(HashMap::new());
        }

        let base = self.base_url.read().clone();
        let url = format!(
            "{}/iserver/marketdata/snapshot?conids={}&fields=31,7282",
            base.trim_end_matches('/'),
            conids.join(",")
        );
        let resp = match self.client.get(&url).send().await {
            Ok(r) if r.status().is_success() => r,
            _ => return Ok(HashMap::new()),
        };

        let body: serde_json::Value = match resp.json().await {
            Ok(b) => b,
            Err(_) => return Ok(HashMap::new()),
        };

        let mut prices = HashMap::new();
        let conid_map = self.conid_cache.read();
        let reverse_map: HashMap<u64, String> =
            conid_map.iter().map(|(k, v)| (*v, k.clone())).collect();

        if let Some(arr) = body.as_array() {
            for item in arr {
                let conid = item.get("conid").and_then(|x| x.as_u64());
                let price_dec = item
                    .get("31")
                    .or_else(|| item.get("7282"))
                    .and_then(|p| match p {
                        serde_json::Value::String(s) => Decimal::from_str_exact(s).ok(),
                        serde_json::Value::Number(n) => {
                            Decimal::from_str_exact(&n.to_string()).ok()
                        }
                        _ => None,
                    });
                if let (Some(cid), Some(price)) = (conid, price_dec) {
                    if let Some(sym) = reverse_map.get(&cid) {
                        prices.insert(sym.clone(), price);
                    }
                }
            }
        }

        Ok(prices)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let conid = self.resolve_conid(symbol).await?;
        let base = self.base_url.read().clone();
        let url = format!(
            "{}/iserver/marketdata/snapshot?conids={}&fields=84,86,85,88,31",
            base.trim_end_matches('/'),
            conid
        );
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!(
                "Failed to fetch market snapshot for {symbol} (HTTP {})",
                resp.status()
            );
        }
        let list: serde_json::Value = resp.json().await?;
        let item = list
            .as_array()
            .and_then(|arr| arr.first())
            .ok_or_else(|| anyhow::anyhow!("No market data snapshot returned for {symbol}"))?;

        let extract_num = |v: Option<&serde_json::Value>| -> Option<Decimal> {
            match v? {
                serde_json::Value::Number(n) => Decimal::from_str_exact(&n.to_string()).ok(),
                serde_json::Value::String(s) => Decimal::from_str_exact(s).ok(),
                _ => None,
            }
        };

        let bid_p = extract_num(item.get("84"));
        let ask_p = extract_num(item.get("86"));
        let bid_s = extract_num(item.get("88")).unwrap_or(dec!(100));
        let ask_s = extract_num(item.get("85")).unwrap_or(dec!(100));
        let last_p = extract_num(item.get("31"));

        let (Some(bp), Some(ap)) = (bid_p, ask_p) else {
            anyhow::bail!("Snapshot for {symbol} missing bid or ask quotes");
        };

        Ok(OrderBook {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            bids: vec![BookLevel {
                price: bp,
                size: bid_s,
            }],
            asks: vec![BookLevel {
                price: ap,
                size: ask_s,
            }],
            timestamp: Utc::now(),
            sequence: 0,
            mark_price: last_p,
        })
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let book = self.fetch_orderbook(symbol).await?;
        let bid = book.best_bid_price().unwrap_or(dec!(0));
        let ask = book.best_ask_price().unwrap_or(dec!(0));
        let last = book.mark_price.unwrap_or(bid);
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
        anyhow::bail!("IBKR spot/equities does not support funding rates ({symbol})")
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        let account_id = self.resolve_account_id().await?;
        let base = self.base_url.read().clone();

        // Prime the gateway session context by querying accounts first,
        // preventing IBKR's "Please query /accounts first" portfolio session failure.
        let acct_url = format!("{}/portfolio/accounts", base.trim_end_matches('/'));
        let _ = self.client.get(&acct_url).send().await;

        // 1. Fetch Summary
        let sum_url = format!(
            "{}/portfolio/{}/summary",
            base.trim_end_matches('/'),
            urlencoding::encode(&account_id)
        );
        let sum_resp = self.client.get(&sum_url).send().await?;
        if !sum_resp.status().is_success() {
            let status = sum_resp.status();
            let body = sum_resp.text().await.unwrap_or_default();
            if status == reqwest::StatusCode::FORBIDDEN
                || status == reqwest::StatusCode::UNAUTHORIZED
            {
                anyhow::bail!(
                    "Gateway session is not authenticated. Please log in at https://localhost:5000 in your browser with your paper credentials."
                );
            }
            anyhow::bail!("IBKR account summary request failed (HTTP {status}): {body}");
        }
        let sum_val: serde_json::Value = sum_resp.json().await?;
        let balance = self.parse_summary_balance(&sum_val).unwrap_or(Balance {
            venue: self.venue_id.clone(),
            currency: "USD".to_string(),
            total: dec!(0),
            free: dec!(0),
            locked: dec!(0),
        });

        // 2. Fetch Positions
        let pos_url = format!(
            "{}/portfolio/{}/positions/0",
            base.trim_end_matches('/'),
            urlencoding::encode(&account_id)
        );
        let pos_resp = self.client.get(&pos_url).send().await?;
        if !pos_resp.status().is_success() {
            let status = pos_resp.status();
            let body = pos_resp.text().await.unwrap_or_default();
            anyhow::bail!("IBKR account positions request failed (HTTP {status}): {body}");
        }
        let pos_val: serde_json::Value = pos_resp.json().await?;
        let positions = self.parse_positions(&pos_val);

        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity: balance.total,
            free_margin: balance.free,
            maintenance_margin: dec!(0),
            margin_usage_pct: dec!(0),
            balances: vec![balance],
            positions,
            timestamp: Utc::now(),
        })
    }

    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        self.rate_limiter.acquire_signed().await?;
        let account_id = self.resolve_account_id().await?;
        let conid = self.resolve_conid(&order.symbol).await?;
        let crypto = Self::is_crypto_symbol(&order.symbol);

        let ty = match order.order_type {
            OrderType::Limit => "LMT",
            OrderType::Market => "MKT",
            OrderType::ImmediateOrCancel => "MKT",
            OrderType::PostOnly => {
                anyhow::bail!("IBKR does not support pure post-only via Web API")
            }
        };

        let tif = match order.time_in_force {
            TimeInForce::GoodTilCancelled => "GTC",
            TimeInForce::ImmediateOrCancel => "IOC",
            TimeInForce::FillOrKill => "FOK",
            TimeInForce::PostOnly => anyhow::bail!("Post-only time in force not supported on IBKR"),
        };

        let side = match order.side {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        };

        let mut order_spec = serde_json::json!({
            "conid": conid,
            "orderType": ty,
            "side": side,
            "quantity": order.quantity.normalize().to_string().parse::<f64>().unwrap_or(1.0),
            "tif": tif,
        });

        if !order.client_order_id.is_empty() {
            order_spec["cOID"] = order.client_order_id.clone().into();
        }

        if ty == "LMT" {
            let p = order
                .price
                .ok_or_else(|| anyhow::anyhow!("Limit order on IBKR requires price"))?;
            let snapped = Self::snap_limit_price(p, order.side, crypto);
            order_spec["price"] = snapped.to_string().parse::<f64>().unwrap_or(0.0).into();
        }

        let body = serde_json::json!({
            "orders": [order_spec]
        });

        let base = self.base_url.read().clone();
        let place_url = format!(
            "{}/iserver/account/{}/orders",
            base.trim_end_matches('/'),
            urlencoding::encode(&account_id)
        );

        let resp = self.client.post(&place_url).json(&body).send().await?;
        let resp_val: serde_json::Value = resp.json().await?;

        // Handle prompt reply confirmation if required by IBKR gateway
        // The gateway can return multiple sequential prompts (e.g. price warning then outside-RTH warning)
        let mut final_val = resp_val;
        for _ in 0..5 {
            let reply_id = if let Some(arr) = final_val.as_array() {
                arr.first().and_then(|item| {
                    if item.get("order_id").is_none() && item.get("orderId").is_none() {
                        item.get("id").and_then(|id| id.as_str())
                    } else {
                        None
                    }
                })
            } else if let Some(item) = final_val.as_object() {
                if item.get("order_id").is_none() && item.get("orderId").is_none() {
                    item.get("id").and_then(|id| id.as_str())
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(id) = reply_id {
                let reply_url = format!("{}/iserver/reply/{}", base.trim_end_matches('/'), id);
                let confirm_body = serde_json::json!({ "confirmed": true });
                let reply_resp = self
                    .client
                    .post(&reply_url)
                    .json(&confirm_body)
                    .send()
                    .await?;
                final_val = reply_resp.json().await.unwrap_or(final_val);
            } else {
                break;
            }
        }

        if let Some(err) = final_val.get("error").and_then(|e| e.as_str()) {
            anyhow::bail!("IBKR order placement rejected: {err}");
        }

        let first = if let Some(arr) = final_val.as_array() {
            arr.first().ok_or_else(|| {
                anyhow::anyhow!("Empty order placement array from IBKR: {final_val}")
            })?
        } else if final_val.is_object() {
            &final_val
        } else {
            anyhow::bail!("Unexpected order placement response from IBKR: {final_val}");
        };

        if let Some(err) = first.get("error").and_then(|e| e.as_str()) {
            anyhow::bail!("IBKR order placement rejected: {err}");
        }

        let order_id = first
            .get("order_id")
            .or_else(|| first.get("orderId"))
            .or_else(|| first.get("id"))
            .and_then(|x| match x {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .ok_or_else(|| {
                anyhow::anyhow!("IBKR order placement response missing order_id: {first}")
            })?;

        let status_str = first
            .get("order_status")
            .or_else(|| first.get("orderStatus"))
            .or_else(|| first.get("status"))
            .and_then(|x| x.as_str())
            .unwrap_or("Submitted");

        let parse_dec_str = |s: &str| -> Option<Decimal> {
            let clean = s.trim().trim_start_matches('$').replace(',', "");
            Decimal::from_str_exact(&clean).ok()
        };
        let extract_num = |v: Option<&serde_json::Value>| -> Option<Decimal> {
            match v? {
                serde_json::Value::Number(n) => Decimal::from_str_exact(&n.to_string()).ok(),
                serde_json::Value::String(s) => parse_dec_str(s),
                _ => None,
            }
        };

        let filled_qty = extract_num(first.get("cum_fill_qty"))
            .or_else(|| extract_num(first.get("cum_fill_quantity")))
            .or_else(|| extract_num(first.get("filled_quantity")))
            .or_else(|| extract_num(first.get("filledQuantity")))
            .unwrap_or(dec!(0));
        let remaining_qty = (order.quantity - filled_qty).max(dec!(0));
        let status = Self::map_status(status_str, filled_qty).unwrap_or(OrderStatus::New);
        let avg_price =
            extract_num(first.get("avg_price")).or_else(|| extract_num(first.get("avgPrice")));

        Ok(OrderResponse {
            order_id,
            client_order_id: order.client_order_id,
            venue: self.venue_id.clone(),
            symbol: order.symbol,
            status,
            original_qty: order.quantity,
            filled_qty,
            remaining_qty,
            avg_fill_price: avg_price,
            fee: None,
            fee_currency: String::new(),
            raw_response: Some(final_val.to_string()),
            timestamp: Utc::now(),
        })
    }

    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
        self.rate_limiter.acquire_signed().await?;
        let account_id = self.resolve_account_id().await?;
        let base = self.base_url.read().clone();
        let order_id = req
            .order_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("CancelRequest requires order_id"))?;
        let url = format!(
            "{}/iserver/account/{}/order/{}",
            base.trim_end_matches('/'),
            urlencoding::encode(&account_id),
            urlencoding::encode(order_id)
        );
        let resp = self.client.delete(&url).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("IBKR order cancel failed (HTTP {status}): {body}");
        }

        // Handle prompt reply confirmation if required by IBKR gateway on cancel
        if let Ok(val) = resp.json::<serde_json::Value>().await {
            if let Some(reply_id) = val
                .as_array()
                .and_then(|arr| arr.first())
                .and_then(|item| item.get("id"))
                .and_then(|id| id.as_str())
            {
                let reply_url =
                    format!("{}/iserver/reply/{}", base.trim_end_matches('/'), reply_id);
                let confirm_body = serde_json::json!({ "confirmed": true });
                let _ = self
                    .client
                    .post(&reply_url)
                    .json(&confirm_body)
                    .send()
                    .await;
            }
        }
        Ok(())
    }

    async fn fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        self.rate_limiter.acquire_signed().await?;
        let base = self.base_url.read().clone();
        let url = format!(
            "{}/iserver/account/order/status/{}",
            base.trim_end_matches('/'),
            urlencoding::encode(order_id)
        );

        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("IBKR order status query failed (HTTP {status}): {body}");
        }

        let val: serde_json::Value = resp.json().await?;
        let parse_dec_str = |s: &str| -> Option<Decimal> {
            let clean = s.trim().trim_start_matches('$').replace(',', "");
            Decimal::from_str_exact(&clean).ok()
        };
        let extract_num = |v: Option<&serde_json::Value>| -> Option<Decimal> {
            match v? {
                serde_json::Value::Number(n) => Decimal::from_str_exact(&n.to_string()).ok(),
                serde_json::Value::String(s) => parse_dec_str(s),
                _ => None,
            }
        };

        let raw_status = val
            .get("order_status")
            .or_else(|| val.get("orderStatus"))
            .or_else(|| val.get("status"))
            .and_then(|x| x.as_str())
            .unwrap_or("Submitted");

        let filled_qty = extract_num(val.get("cum_fill_qty"))
            .or_else(|| extract_num(val.get("cum_fill_quantity")))
            .or_else(|| extract_num(val.get("filled_quantity")))
            .or_else(|| extract_num(val.get("filledQuantity")))
            .unwrap_or(dec!(0));

        let orig_qty = extract_num(val.get("size"))
            .or_else(|| extract_num(val.get("totalSize")))
            .or_else(|| extract_num(val.get("total_size")))
            .or_else(|| extract_num(val.get("total_quantity")))
            .or_else(|| extract_num(val.get("quantity")))
            .unwrap_or(filled_qty);

        let remaining_qty = extract_num(val.get("remaining_quantity"))
            .or_else(|| extract_num(val.get("remainingQuantity")))
            .unwrap_or_else(|| (orig_qty - filled_qty).max(dec!(0)));

        let avg_price = extract_num(val.get("avg_price"))
            .or_else(|| extract_num(val.get("avgPrice")))
            .or_else(|| extract_num(val.get("price")));
        let status = Self::map_status(raw_status, filled_qty)?;

        Ok(OrderResponse {
            order_id: order_id.to_string(),
            client_order_id: client_order_id.unwrap_or_default().to_string(),
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            status,
            original_qty: orig_qty,
            filled_qty,
            remaining_qty,
            avg_fill_price: avg_price,
            fee: None,
            fee_currency: String::new(),
            raw_response: Some(val.to_string()),
            timestamp: Utc::now(),
        })
    }

    async fn wire_base_qty(&self, symbol: &str, qty: Decimal) -> anyhow::Result<Decimal> {
        if Self::is_crypto_symbol(symbol) {
            Ok((qty * dec!(10000)).floor() / dec!(10000))
        } else {
            Ok(qty.floor())
        }
    }

    fn supports_reduce_only(&self, _symbol: &str) -> bool {
        false
    }

    fn cash_session_bound(&self, symbol: &str) -> bool {
        !Self::is_crypto_symbol(symbol)
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
        self.account_id.read().is_some()
    }

    fn economic_account_key(&self) -> Option<String> {
        self.account_id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adapter_initialization() {
        let live = IbkrAdapter::new(Some("U1234567".into()), None);
        assert!(!live.is_paper());
        assert_eq!(live.venue_id(), VenueId::new("ibkr"));
        assert_eq!(live.name(), "Interactive Brokers");
        assert_eq!(live.account_id(), Some("U1234567".into()));
        assert!(live.has_live_credentials());
        assert_eq!(
            live.rest_base_url(),
            Some("https://127.0.0.1:5000/v1/api".into())
        );

        let paper = IbkrAdapter::for_paper(
            Some("DU9999999".into()),
            Some("http://localhost:5001/v1/api/".into()),
        );
        assert!(paper.is_paper());
        assert_eq!(paper.account_id(), Some("DU9999999".into()));
        assert_eq!(
            paper.rest_base_url(),
            Some("http://localhost:5001/v1/api".into())
        );
    }

    #[test]
    fn test_credential_update_and_clear() {
        let adapter = IbkrAdapter::new(None, None);
        assert!(!adapter.has_live_credentials());

        adapter.update_credentials(
            Some("DU1111111".into()),
            Some("https://ibkr-gw:5000/v1/api".into()),
        );
        assert!(adapter.has_live_credentials());
        assert_eq!(adapter.account_id(), Some("DU1111111".into()));
        assert_eq!(
            adapter.rest_base_url(),
            Some("https://ibkr-gw:5000/v1/api".into())
        );

        adapter.update_credentials(None, None);
        assert!(!adapter.has_live_credentials());
        assert_eq!(adapter.account_id(), None);
    }

    #[test]
    fn test_symbol_helpers() {
        assert!(IbkrAdapter::is_crypto_symbol("BTC"));
        assert!(IbkrAdapter::is_crypto_symbol("BTC-USD"));
        assert!(IbkrAdapter::is_crypto_symbol("ETH/USD"));
        assert!(!IbkrAdapter::is_crypto_symbol("AAPL"));
        assert!(!IbkrAdapter::is_crypto_symbol("SPY"));

        assert_eq!(IbkrAdapter::to_venue_symbol("BTC-USD"), "BTC");
        assert_eq!(IbkrAdapter::to_venue_symbol("AAPL"), "AAPL");
        assert_eq!(IbkrAdapter::canonical_symbol("BTC"), "BTC-USD");
        assert_eq!(IbkrAdapter::canonical_symbol("AAPL"), "AAPL");
    }

    #[test]
    fn test_status_mapping() {
        assert_eq!(
            IbkrAdapter::map_status("Filled", dec!(10)).unwrap(),
            OrderStatus::Filled
        );
        assert_eq!(
            IbkrAdapter::map_status("Cancelled", dec!(0)).unwrap(),
            OrderStatus::Cancelled
        );
        assert_eq!(
            IbkrAdapter::map_status("Submitted", dec!(0)).unwrap(),
            OrderStatus::New
        );
        assert_eq!(
            IbkrAdapter::map_status("PreSubmitted", dec!(5)).unwrap(),
            OrderStatus::PartiallyFilled
        );
    }

    #[test]
    fn test_parse_summary_and_positions() {
        let adapter = IbkrAdapter::new(Some("DU1234567".into()), None);
        let summary_json = serde_json::json!({
            "netliquidationvalue": 150000.0,
            "totalcashvalue": 150000.0,
            "buyingpower": 300000.0
        });
        let bal = adapter
            .parse_summary_balance(&summary_json)
            .expect("balance");
        assert_eq!(bal.total, dec!(150000.0));
        assert_eq!(bal.free, dec!(300000.0));

        let positions_json = serde_json::json!([
            {
                "acctId": "DU1234567",
                "ticker": "AAPL",
                "position": 50.0,
                "mktPrice": 220.0,
                "avgCost": 210.0,
                "unrealizedPnl": 500.0
            },
            {
                "acctId": "DU1234567",
                "ticker": "BTC",
                "position": -1.5,
                "mktPrice": 65000.0,
                "avgCost": 66000.0,
                "unrealizedPnl": 1500.0
            }
        ]);
        let positions = adapter.parse_positions(&positions_json);
        assert_eq!(positions.len(), 2);
        assert_eq!(positions[0].symbol, "AAPL");
        assert_eq!(positions[0].side, Side::Buy);
        assert_eq!(positions[0].quantity, dec!(50.0));

        assert_eq!(positions[1].symbol, "BTC-USD");
        assert_eq!(positions[1].side, Side::Sell);
        assert_eq!(positions[1].quantity, dec!(1.5));
    }

    #[test]
    fn test_parse_summary_segmented_and_formatted() {
        let adapter = IbkrAdapter::new(Some("U9876543".into()), None);
        // Segmented securities summary with formatted string numbers and custom currency
        let summary_json = serde_json::json!({
            "netliquidationvalue-s": {
                "amount": null,
                "currency": "USD",
                "value": "$250,432.10"
            },
            "buyingpower-s": {
                "amount": null,
                "currency": "USD",
                "value": "$500,864.20"
            }
        });
        let bal = adapter
            .parse_summary_balance(&summary_json)
            .expect("balance from segmented summary");
        assert_eq!(bal.total, dec!(250432.10));
        assert_eq!(bal.free, dec!(500864.20));
        assert_eq!(bal.currency, "USD");
    }

    #[test]
    fn test_economic_account_key() {
        let live = IbkrAdapter::new(Some("U1234567".into()), None);
        assert_eq!(live.economic_account_key(), Some("U1234567".into()));

        let paper = IbkrAdapter::for_paper(Some("DU7654321".into()), None);
        assert_eq!(paper.economic_account_key(), Some("DU7654321".into()));

        let empty = IbkrAdapter::new(None, None);
        assert_eq!(empty.economic_account_key(), None);
    }
}
