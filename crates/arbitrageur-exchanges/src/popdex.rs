//! PopDEX perpetual DEX connector (native API: public REST + on-chain contracts).
//!
//! # Connectivity model (researched 2026-09-19, docs deploy `604061a`)
//!
//! PopDEX runs on Morph Tachyon (full EVM, gasless, instant finality) and every
//! trading action is a direct smart-contract call — there is no order-placement
//! REST and no proprietary SDK:
//!
//! - **Market data (public REST, no auth, verified live):**
//!   `GET {rest}/api/v1/public/market/tickers?category=Futures`,
//!   `.../orderbook?category=Futures&symbol=BTCUSDT&levels=25`,
//!   `GET {rest}/api/v1/market/funding-rate`, plus `fee-rate` and
//!   `config/symbol`. Envelope is `{code:"200", msg, data, ...}`.
//!   Account reads (`/api/v1/account/{wallet}/positions|orders|agents|overview`)
//!   are likewise unauthenticated.
//! - **Execution (agent-signed contract calls):** precompiles at fixed
//!   addresses — `Order` (`0x...1000`), `Account` (`0x...1008`), `UserConfig`
//!   (`0x...1009`). The operator's **agent key** signs with a millisecond
//!   **timestamp nonce**; the main wallet approves the agent once via
//!   `approveAgent` (external one-time script — the bot never holds the main
//!   key, withdrawals stay main-only).
//!
//! Docs: <https://popdex.xyz/docs/product-docs/onboarding/how-to-trade-via-api>
//!
//! # Credential model
//!
//! - `wallet_address` = main PopDEX account (public `0x…`, holds funds).
//! - `private_key_hex` = **agent private key only**, approved on-chain by the
//!   main wallet. The two addresses intentionally differ.
//! - Until an agent key is bound, order paths fail closed; market data and
//!   account reads work without any credentials.
//!
//! # Verification status
//!
//! - Market/account REST: verified live against `api.popdex.xyz` (66 futures
//!   symbols, real BTC book, 1h funding).
//! - Contract submission: implemented per the published interface
//!   (unit-tested ABI packing, nonce rules, receipt/event decoding) but not
//!   yet dry-run against testnet — validate on testnet (`chain 34952`)
//!   before mainnet size, per PopDEX's own recommendation.

use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;

use alloy::primitives::{Address, B256, Bytes, U256};
use async_trait::async_trait;
use chrono::Utc;
use parking_lot::RwLock;
use reqwest::Client;
use serde::Deserialize;
use zeroize::{Zeroize, Zeroizing};

use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::{AccountSnapshot, Balance, Position};
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{dec, Decimal, OrderStatus, OrderType, Side, TimeInForce, VenueId};

use crate::rate_limiter::AccountRequestCoordinator;

/// Mainnet REST base (docs: API Domains).
pub const POPDEX_MAINNET_REST: &str = "https://api.popdex.xyz";
/// Testnet REST base.
pub const POPDEX_TESTNET_REST: &str = "https://testnet-api.popdex.xyz";
/// Mainnet RPC (Morph Tachyon, chain 2184, gasless).
pub const POPDEX_MAINNET_RPC: &str = "https://app.popdex.xyz/api/v1/web3/rpc";
/// Testnet RPC (chain 34952).
pub const POPDEX_TESTNET_RPC: &str = "https://testnet-app.popdex.xyz/api/v1/web3/rpc";
/// Morph Tachyon mainnet chain id.
pub const POPDEX_MAINNET_CHAIN_ID: u64 = 2184;
/// Morph Tachyon testnet chain id.
pub const POPDEX_TESTNET_CHAIN_ID: u64 = 34952;
/// `Order` precompile: place/cancel/match dispatch.
pub const POPDEX_ORDER_PRECOMPILE: &str = "0x0000000000000000000000000000000000001000";
/// `Account` precompile: sub-accounts + agent authorization.
pub const POPDEX_ACCOUNT_PRECOMPILE: &str = "0x0000000000000000000000000000000000001008";
/// `UserConfig` precompile: leverage + position mode.
pub const POPDEX_USERCONFIG_PRECOMPILE: &str = "0x0000000000000000000000000000000000001009";
/// USDT on Morph Tachyon (docs: Token Config; `set_leverage` margin token).
pub const POPDEX_USDT_TACHYON: &str = "0x0000000000000000000000200000000000000001";
/// VIP0 taker fee 0.1% = 10 bps (docs: Symbol Fee Rate).
pub const POPDEX_TAKER_FEE_BPS: &str = "10.0";
/// VIP0 maker fee 0.1% = 10 bps.
pub const POPDEX_MAKER_FEE_BPS: &str = "10.0";
/// Per-account open-order cap (docs: place wisdom — cancel stale orders).
pub const POPDEX_MAX_OPEN_ORDERS: usize = 500;
/// Which PopDEX network this adapter talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PopDexNetwork {
    /// Real funds, real prices (default for scanning).
    #[default]
    Mainnet,
    /// Chain 34952 + test USDT faucet (validate automation first).
    Testnet,
}

impl PopDexNetwork {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "mainnet" | "main" | "prod" => Some(Self::Mainnet),
            "testnet" | "test" => Some(Self::Testnet),
            _ => None,
        }
    }

    fn rest_base(self) -> &'static str {
        match self {
            Self::Mainnet => POPDEX_MAINNET_REST,
            Self::Testnet => POPDEX_TESTNET_REST,
        }
    }

    fn rpc_url(self) -> &'static str {
        match self {
            Self::Mainnet => POPDEX_MAINNET_RPC,
            Self::Testnet => POPDEX_TESTNET_RPC,
        }
    }

    fn chain_id(self) -> u64 {
        match self {
            Self::Mainnet => POPDEX_MAINNET_CHAIN_ID,
            Self::Testnet => POPDEX_TESTNET_CHAIN_ID,
        }
    }
}

fn is_valid_evm_address(a: &str) -> bool {
    let s = a.trim();
    s.len() == 42 && s.starts_with("0x") && s[2..].chars().all(|c| c.is_ascii_hexdigit())
}

/// REST envelope shared by every PopDEX endpoint (`code` is a string).
#[derive(Debug, Deserialize)]
#[serde(bound(deserialize = "T: serde::Deserialize<'de>"))]
struct Envelope<T> {
    code: String,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct TickerRow {
    #[serde(default)]
    #[serde(rename = "symbolId")]
    symbol_id: String,
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    #[serde(rename = "lastPrice")]
    last_price: String,
    #[serde(default)]
    #[serde(rename = "bid1Price")]
    bid1_price: String,
    #[serde(default)]
    #[serde(rename = "ask1Price")]
    ask1_price: String,

    #[serde(default)]
    #[serde(rename = "turnover24h")]
    turnover24h: String,
    #[serde(default)]
    status: String,
}

#[derive(Debug, Deserialize)]
struct TickerPage {
    #[serde(default)]
    data: Vec<TickerRow>,
    #[serde(default)]
    cursor: String,
}

#[derive(Debug, Deserialize)]
struct BookPayload {
    #[serde(default)]
    asks: Vec<[String; 2]>,
    #[serde(default)]
    bids: Vec<[String; 2]>,
}

#[derive(Debug, Deserialize)]
struct FundingRow {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    #[serde(rename = "fundingRate")]
    funding_rate: String,
    #[serde(default)]
    #[serde(rename = "fundingRateInterval")]
    funding_rate_interval: String,
    #[serde(default)]
    #[serde(rename = "nextUpdate")]
    next_update: String,
}

#[derive(Debug, Deserialize)]
struct PositionRow {
    #[serde(default)]
    #[serde(rename = "holdQty")]
    hold_qty: String,
    #[serde(default)]
    #[serde(rename = "positionSide")]
    position_side: String,
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    #[serde(rename = "avgOpenPrice")]
    avg_open_price: String,
    #[serde(default)]
    #[serde(rename = "markPrice")]
    mark_price: String,
    #[serde(default)]
    unpnl: String,
    #[serde(default)]
    #[serde(rename = "realizedPnl")]
    realized_pnl: String,
    #[serde(default)]
    #[serde(rename = "liquidationPrice")]
    liquidation_price: String,
    #[serde(default)]
    #[serde(rename = "symbolLeverage")]
    symbol_leverage: String,
    #[serde(default)]
    #[serde(rename = "initialMargin")]
    initial_margin: String,
}

#[derive(Debug, Deserialize)]
struct BalanceRow {
    #[serde(default)]
    token: String,
    #[serde(default)]
    balance: String,
    #[serde(default)]
    frozen: String,
    #[serde(default)]
    available: String,
}

#[derive(Debug, Deserialize)]
struct OverviewPayload {
    #[serde(default)]
    #[serde(rename = "accountEquity")]
    account_equity: String,
    #[serde(default)]
    #[serde(rename = "availableMargin")]
    available_margin: String,
    #[serde(default)]
    #[serde(rename = "initialMargin")]
    initial_margin: String,
    #[serde(default)]
    balances: Vec<BalanceRow>,
}

#[derive(Debug, Deserialize)]
struct AgentRow {
    #[serde(default)]
    agent: String,
    #[serde(default)]
    #[serde(rename = "isExpired")]
    is_expired: bool,
}

#[derive(Debug, Deserialize)]
struct FeeEntry {
    #[serde(default)]
    #[serde(rename = "takerFeeRate")]
    taker_fee_rate: String,
    #[serde(default)]
    #[serde(rename = "makerFeeRate")]
    maker_fee_rate: String,
}

#[derive(Debug, Deserialize)]
struct FeeRow {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    #[serde(rename = "feeRates")]
    fee_rates: Vec<FeeEntry>,
}

fn dec_of(s: &str) -> anyhow::Result<Decimal> {
    let t = s.trim();
    if t.is_empty() {
        anyhow::bail!("empty decimal string");
    }
    Decimal::from_str(t).map_err(|e| anyhow::anyhow!("invalid decimal '{t}': {e}"))
}

/// Scale a human-unit decimal to 18-decimal wad (`U256`), truncating dust
/// past 18dp. String-based so large prices (e.g. BTC ~$81k → 8e22) never
/// overflow `Decimal`'s 96-bit mantissa the way `d * 1e18` would.
fn dec_to_wad(d: Decimal) -> anyhow::Result<U256> {
    if d.is_sign_negative() {
        anyhow::bail!("negative value cannot convert to wad: {d}");
    }
    let s = d.to_string();
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i, f),
        None => (s.as_str(), ""),
    };
    let mut frac18: String = frac.chars().take(18).collect();
    while frac18.len() < 18 {
        frac18.push('0');
    }
    let digits = format!("{int}{frac18}");
    let digits = digits.trim_start_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    U256::from_str_radix(digits, 10)
        .map_err(|e| anyhow::anyhow!("wad overflow for {d}: {e}"))
}

/// Pack `placeOrder` flags per the unified `orderParams` layout (bytes 0-7).
fn pack_order_params(
    side: Side,
    limit: bool,
    tif_gtc: bool,
    tif_ioc: bool,
    tif_fok: bool,
    post_only: bool,
    reduce_only: bool,
) -> [u8; 32] {
    let mut b = [0u8; 32];
    b[0] = 2; // Category::Futures
    b[1] = u8::from(!limit); // OrderType: Limit=0, Market=1
    b[2] = match side {
        Side::Buy => 0,
        Side::Sell => 1,
    };
    b[3] = if post_only {
        4 // PostOnly
    } else if tif_fok {
        3 // FOK
    } else if tif_ioc || !limit {
        2 // IOC (market orders always IOC)
    } else {
        1 // GTC
    };
    b[4] = 0; // MarketUnit::BaseToken — qty is base units
    b[5] = 0; // BBO::None — explicit price
    b[6] = u8::from(reduce_only);
    b[7] = 0; // PositionSide::None (one-way mode default)
    let _ = tif_gtc;
    b
}

/// Build a random 32-byte client order id (raw bytes; PopDEX right-pads).
fn random_client_oid() -> [u8; 32] {
    use rand::RngCore;
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    b[0] |= 0x01; // never the zero id (reserved for "unused")
    b
}

/// Encode a UTF-8 client id into right-padded `bytes32`.
fn client_oid_bytes(s: &str) -> [u8; 32] {
    let mut b = [0u8; 32];
    let raw = s.as_bytes();
    let n = raw.len().min(32);
    b[..n].copy_from_slice(&raw[..n]);
    if b == [0u8; 32] {
        b[0] = 0x01;
    }
    b
}

/// Compute the `OrderCreate` event topic0 (used to extract `orderId`).
fn order_create_topic() -> B256 {
    alloy::primitives::keccak256(
        "OrderCreate(address,uint16,uint128,bytes32,int256,int256,int256,uint8,bool,uint32)",
    )
}

/// One-time agent-approval instructions (the bot never holds the main key,
/// so approval runs as an external script per the official guide).
pub fn approve_agent_instructions(main: &str, agent: &str) -> String {
    format!(
        "PopDEX agent approval is a one-time on-chain call signed by the MAIN wallet \
         (cast from the official guide; needs ~test USDT on testnet / USDT deposit on mainnet):\n\
         cast send {account} \"approveAgent(address,address,bytes32,uint64,uint64,bool)\" \
         {agent} {main} 0x0000000000000000000000000000000000000000000000000000000000000000 0 $(date +%s%3N) false \
         --rpc-url {rpc} --private-key $MAIN_PRIVATE_KEY\n\
         Rules: fresh agent address only (never re-approve a revoked one), deposit before \
         approving, max 10 agents per main account. Docs: \
         https://popdex.xyz/docs/product-docs/onboarding/how-to-trade-via-api (Step 4).",
        account = POPDEX_ACCOUNT_PRECOMPILE,
        agent = agent,
        main = main,
        rpc = POPDEX_MAINNET_RPC,
    )
}
/// PopDEX connector: public REST market data + agent-signed contract trading.
pub struct PopDexAdapter {
    venue_id: VenueId,
    client: Client,
    network: RwLock<PopDexNetwork>,
    /// Main PopDEX account (public funds holder, `account` param on-chain).
    wallet_address: RwLock<Option<String>>,
    /// Agent private key hex (vault-encrypted at rest, zeroized wrapper).
    agent_key: RwLock<Option<Zeroizing<String>>>,
    /// Agent address derived from the key at bind time.
    agent_address: RwLock<Option<String>>,
    /// Last agent timestamp-nonce issued (monotonic guard; see docs Step 3).
    last_agent_nonce: RwLock<u64>,
    /// Cached venue symbol → on-chain `symbolId` (from tickers).
    symbol_ids: RwLock<HashMap<String, u16>>,
    /// Cached agent-approval check (main → (approved, at)).
    approval_cache: RwLock<Option<(bool, std::time::Instant)>>,
    rate_limiter: AccountRequestCoordinator,
}

impl PopDexAdapter {
    pub fn new(network: Option<String>) -> Self {
        let net = network
            .as_deref()
            .and_then(PopDexNetwork::parse)
            .unwrap_or_default();
        Self {
            venue_id: VenueId::new("popdex"),
            client: Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            network: RwLock::new(net),
            wallet_address: RwLock::new(None),
            agent_key: RwLock::new(None),
            agent_address: RwLock::new(None),
            last_agent_nonce: RwLock::new(0),
            symbol_ids: RwLock::new(HashMap::new()),
            approval_cache: RwLock::new(None),
            rate_limiter: AccountRequestCoordinator::new("popdex", 150, 30_000, 500, 10_000),
        }
    }

    fn rest_base(&self) -> &'static str {
        self.network.read().rest_base()
    }

    fn rpc_url(&self) -> &'static str {
        self.network.read().rpc_url()
    }

    fn chain_id(&self) -> u64 {
        self.network.read().chain_id()
    }

    /// Rebind network and/or credentials at runtime (Settings save / vault
    /// unlock). The agent key is delegation-friendly by design: it must NOT
    /// match the main address (that would be the main key itself, which the
    /// bot must never hold). An invalid key or address fails the whole update.
    pub fn update_credentials(
        &self,
        network: Option<String>,
        wallet_address: Option<String>,
        agent_key_hex: Option<String>,
    ) -> anyhow::Result<()> {
        if let Some(n) = network {
            let net = PopDexNetwork::parse(&n)
                .ok_or_else(|| anyhow::anyhow!("PopDEX network must be 'mainnet' or 'testnet', got '{n}'"))?;
            *self.network.write() = net;
        }
        let addr = wallet_address
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty());
        if let Some(a) = addr.as_deref() {
            if !is_valid_evm_address(a) {
                anyhow::bail!("PopDEX wallet address is not a valid 0x EVM address");
            }
        }
        match agent_key_hex
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
        {
            Some(hex_key) => {
                // Validate via the workspace signer (curve membership checked),
                // then derive the agent address for approval checks.
                let signer = crate::wallet::EvmWalletSigner::from_hex(&hex_key)?;
                let agent = signer.address_hex();
                drop(signer);
                if let Some(main) = addr.as_deref() {
                    if main.eq_ignore_ascii_case(&agent) {
                        anyhow::bail!(
                            "PopDEX agent key derives the main address itself — paste a DELEGATED \
                             agent key (approved via approveAgent), never the main private key"
                        );
                    }
                }
                *self.agent_key.write() = Some(Zeroizing::new(hex_key));
                *self.agent_address.write() = Some(agent);
                *self.wallet_address.write() = addr;
                Ok(())
            }
            None => {
                *self.agent_key.write() = None;
                *self.agent_address.write() = None;
                *self.wallet_address.write() = addr;
                Ok(())
            }
        }
    }

    /// Main account address (public), if bound.
    pub fn wallet_address_hex(&self) -> Option<String> {
        self.wallet_address.read().clone()
    }

    /// Derived agent address (from the delegated key), if bound.
    pub fn agent_address_hex(&self) -> Option<String> {
        self.agent_address.read().clone()
    }

    fn agent_key_guarded(&self) -> anyhow::Result<Zeroizing<String>> {
        self.agent_key
            .read()
            .clone()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "PopDEX live trading needs an approved agent key: paste the agent private key \
                     in System Settings → Exchange Accounts (PopDEX), after the one-time \
                     approveAgent setup in the official guide"
                )
            })
    }

    fn main_account(&self) -> anyhow::Result<String> {
        self.wallet_address.read().clone().ok_or_else(|| {
            anyhow::anyhow!("PopDEX main account address is not configured")
        })
    }

    // ----- REST layer (public, no auth) -----

    async fn rest_get<T: for<'de> Deserialize<'de>>(&self, path: &str) -> anyhow::Result<T> {
        // Throttle through the venue coordinator (150ms min interval, 429
        // backoff). Caches are never read here — books/funding must be live.
        let _ = self.rate_limiter.acquire_or_cache_path(path, false).await?;
        let url = format!("{}{}", self.rest_base(), path);
        let resp = self.client.get(&url).send().await.map_err(|e| {
            self.rate_limiter.record_failure(false);
            anyhow::anyhow!("PopDEX REST transport error on {path}: {e}")
        })?;
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            self.rate_limiter.record_failure(true);
            anyhow::bail!("PopDEX rate limit (429) on {path} — backing off");
        }
        if !resp.status().is_success() {
            self.rate_limiter.record_failure(false);
            anyhow::bail!("PopDEX REST error: {} {}", resp.status(), path);
        }
        let body = resp.text().await.unwrap_or_default();
        self.rate_limiter.record_success_path(path, body.clone());
        let env: Envelope<T> = serde_json::from_str(&body)
            .map_err(|e| anyhow::anyhow!("PopDEX unparseable JSON for {path}: {e}"))?;
        if env.code.trim() != "200" {
            anyhow::bail!("PopDEX API error {} on {}: {}", env.code, path, env.msg);
        }
        env.data.ok_or_else(|| anyhow::anyhow!("PopDEX API returned no data for {path}"))
    }

    /// All futures tickers (paginated; 66 symbols mainnet as of 2026-09-19).
    async fn fetch_all_tickers(&self) -> anyhow::Result<Vec<TickerRow>> {
        let mut out = Vec::new();
        let mut cursor = "0".to_string();
        for _ in 0..10 {
            let page: TickerPage = self
                .rest_get(&format!(
                    "/api/v1/public/market/tickers?category=Futures&limit=100&cursor={cursor}"
                ))
                .await?;
            if page.data.is_empty() {
                break;
            }
            // Refresh the symbol-id cache from every page.
            {
                let mut ids = self.symbol_ids.write();
                for t in &page.data {
                    if let Ok(id) = t.symbol_id.trim().parse::<u16>() {
                        ids.insert(t.symbol.trim().to_uppercase(), id);
                    }
                }
            }
            let next = page.cursor.clone();
            out.extend(page.data);
            if next.trim().is_empty() || next.trim() == "0" {
                break;
            }
            cursor = next;
        }
        Ok(out)
    }

    /// Venue pair code for an engine symbol (`BTC-USDC` → `BTCUSDT`).
    fn venue_symbol(symbol: &str) -> String {
        let base = symbol
            .split(['-', '/', ':', '_'])
            .next()
            .unwrap_or(symbol)
            .trim()
            .to_uppercase();
        format!("{base}USDT")
    }

    async fn symbol_id(&self, venue_symbol: &str) -> anyhow::Result<u16> {
        if let Some(id) = self.symbol_ids.read().get(venue_symbol) {
            return Ok(*id);
        }
        self.fetch_all_tickers().await?;
        self.symbol_ids
            .read()
            .get(venue_symbol)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("PopDEX has no futures listing for {venue_symbol}"))
    }

    async fn fetch_book(&self, venue_symbol: &str) -> anyhow::Result<BookPayload> {
        self.rest_get(&format!(
            "/api/v1/public/market/orderbook?category=Futures&symbol={venue_symbol}&levels=25"
        ))
        .await
    }

    /// Is the bound agent currently approved (and unexpired) for the main
    /// account? Fresh on-chain-backed check via REST (weight 5); cached 60s.
    pub async fn agent_approved(&self) -> anyhow::Result<bool> {
        if let Some((ok, at)) = *self.approval_cache.read() {
            if at.elapsed() < Duration::from_secs(60) {
                return Ok(ok);
            }
        }
        let main = self.main_account()?;
        let agent = self
            .agent_address_hex()
            .ok_or_else(|| anyhow::anyhow!("no agent key bound"))?;
        let agents: Vec<AgentRow> = self
            .rest_get(&format!("/api/v1/account/{main}/agents"))
            .await?;
        let ok = agents
            .iter()
            .any(|a| !a.is_expired && a.agent.eq_ignore_ascii_case(&agent));
        *self.approval_cache.write() = Some((ok, std::time::Instant::now()));
        Ok(ok)
    }

    fn require_approved_agent(&self) -> anyhow::Result<(String, Zeroizing<String>)> {
        let main = self.main_account()?;
        let key = self.agent_key_guarded()?;
        Ok((main, key))
    }
}

fn parse_book_side(levels: &[[String; 2]], venue: &VenueId, symbol: &str) -> anyhow::Result<Vec<BookLevel>> {
    let mut out = Vec::with_capacity(levels.len());
    for [p, q] in levels {
        let price = dec_of(p)?;
        let size = dec_of(q)?;
        if price.is_sign_positive() && !size.is_zero() {
            out.push(BookLevel::new(price, size));
        }
        let _ = (venue, symbol);
    }
    Ok(out)
}

#[async_trait]
impl ExchangeAdapter for PopDexAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "PopDEX"
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let tickers = self.fetch_all_tickers().await?;
        if tickers.is_empty() {
            anyhow::bail!("PopDEX tickers returned no listings");
        }
        let mut out = Vec::new();
        for t in tickers {
            let venue_symbol = t.symbol.trim().to_uppercase();
            let base = venue_symbol.strip_suffix("USDT").unwrap_or(&venue_symbol);
            if base.is_empty() {
                continue;
            }
            let active = t.status.eq_ignore_ascii_case("trading");
            // VIP0 fee schedule (docs: Symbol Fee Rate); refined per symbol
            // below when the fee endpoint answers.
            let (mut maker, mut taker) = (dec!(10.0), dec!(10.0));
            if let Ok(fees) = self
                .rest_get::<Vec<FeeRow>>(&format!(
                    "/api/v1/public/fee-rate?category=Futures&symbol={venue_symbol}&limit=2"
                ))
                .await
            {
                if let Some(row) = fees.iter().find(|r| r.symbol.eq_ignore_ascii_case(&venue_symbol)) {
                    if let Some(vip0) = row.fee_rates.first() {
                        if let (Ok(m), Ok(tk)) = (
                            Decimal::from_str(&vip0.maker_fee_rate),
                            Decimal::from_str(&vip0.taker_fee_rate),
                        ) {
                            maker = m * dec!(10000.0);
                            taker = tk * dec!(10000.0);
                        }
                    }
                }
            }
            out.push(MarketMetadata {
                symbol: Symbol::perp(base, "USDT"),
                venue: self.venue_id(),
                venue_symbol,
                price_decimals: 2,
                size_decimals: 6,
                min_order_size: dec!(0.001),
                min_notional: dec!(10.0),
                maker_fee_bps: maker,
                taker_fee_bps: taker,
                contract_size: dec!(1.0),
                is_active: active,
            });
        }
        if out.is_empty() {
            anyhow::bail!("PopDEX tickers returned no parseable listings");
        }
        Ok(out)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let venue_symbol = Self::venue_symbol(symbol);
        let payload = self.fetch_book(&venue_symbol).await?;
        let mut book = OrderBook::new(self.venue_id(), symbol.to_string());
        let mut bids = parse_book_side(&payload.bids, &book.venue, symbol)?;
        let mut asks = parse_book_side(&payload.asks, &book.venue, symbol)?;
        bids.sort_by_key(|b| std::cmp::Reverse(b.price));
        asks.sort_by_key(|a| a.price);
        book.bids = bids;
        book.asks = asks;
        book.timestamp = Utc::now();
        if book.bids.is_empty() || book.asks.is_empty() {
            anyhow::bail!("PopDEX book for {venue_symbol} has an empty side");
        }
        Ok(book)
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let venue_symbol = Self::venue_symbol(symbol);
        let rows: Vec<TickerRow> = self
            .rest_get(&format!(
                "/api/v1/public/market/tickers?category=Futures&symbol={venue_symbol}&limit=2"
            ))
            .await?;
        let t = rows
            .into_iter()
            .find(|r| r.symbol.eq_ignore_ascii_case(&venue_symbol))
            .ok_or_else(|| anyhow::anyhow!("PopDEX has no ticker for {venue_symbol}"))?;
        Ok(Ticker {
            venue: self.venue_id(),
            symbol: symbol.to_string(),
            bid: dec_of(&t.bid1_price)?,
            ask: dec_of(&t.ask1_price)?,
            last: dec_of(&t.last_price)?,
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate> {
        let venue_symbol = Self::venue_symbol(symbol);
        let rows: Vec<FundingRow> = self
            .rest_get(&format!(
                "/api/v1/market/funding-rate?symbol={venue_symbol}&limit=2"
            ))
            .await?;
        let r = rows
            .into_iter()
            .find(|x| x.symbol.eq_ignore_ascii_case(&venue_symbol))
            .ok_or_else(|| anyhow::anyhow!("PopDEX has no funding rate for {venue_symbol}"))?;
        let next_ms: i64 = r.next_update.trim().parse().unwrap_or(0);
        Ok(FundingRate {
            venue: self.venue_id(),
            symbol: symbol.to_string(),
            rate: dec_of(&r.funding_rate)?,
            predicted_rate: None,
            interval_hours: r.funding_rate_interval.trim().parse().unwrap_or(1),
            next_funding_time: chrono::DateTime::from_timestamp_millis(next_ms)
                .unwrap_or_else(Utc::now),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_volumes_24h(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        let mut out = HashMap::new();
        for t in self.fetch_all_tickers().await? {
            let venue_symbol = t.symbol.trim().to_uppercase();
            let base = venue_symbol.strip_suffix("USDT").unwrap_or(&venue_symbol);
            if let Ok(turnover) = Decimal::from_str(t.turnover24h.trim()) {
                if turnover > dec!(0.0) {
                    out.insert(base.to_string(), turnover);
                }
            }
        }
        Ok(out)
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        let main = self.main_account()?;
        let overview: OverviewPayload = self
            .rest_get(&format!("/api/v1/account/{main}/overview"))
            .await
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("11100") || msg.contains("does not exist") {
                    anyhow::anyhow!(
                        "PopDEX account {main} does not exist on-chain yet — deposit first \
                         (guide Step 2), then approve the agent (Step 4)"
                    )
                } else {
                    e
                }
            })?;
        let positions: Vec<PositionRow> = self
            .rest_get(&format!("/api/v1/account/{main}/positions"))
            .await
            .unwrap_or_default();
        let venue = self.venue_id();
        let mut account_positions = Vec::new();
        for p in positions {
            let qty = Decimal::from_str(p.hold_qty.trim()).unwrap_or(dec!(0.0));
            if qty.is_zero() {
                continue;
            }
            let base = p
                .symbol
                .trim()
                .to_uppercase()
                .strip_suffix("USDT")
                .unwrap_or(p.symbol.trim())
                .to_string();
            let side = match p.position_side.trim().to_lowercase().as_str() {
                "short" => Side::Sell,
                _ => Side::Buy,
            };
            let liq: Option<Decimal> = Decimal::from_str(p.liquidation_price.trim())
                .ok()
                .filter(|v| !v.is_zero());
            account_positions.push(Position {
                venue: venue.clone(),
                symbol: base,
                side,
                quantity: qty.abs(),
                entry_price: Decimal::from_str(p.avg_open_price.trim()).unwrap_or(dec!(0.0)),
                mark_price: Decimal::from_str(p.mark_price.trim()).unwrap_or(dec!(0.0)),
                unrealized_pnl: Decimal::from_str(p.unpnl.trim()).unwrap_or(dec!(0.0)),
                realized_pnl: Decimal::from_str(p.realized_pnl.trim()).unwrap_or(dec!(0.0)),
                liquidation_price: liq,
                leverage: Decimal::from_str(p.symbol_leverage.trim()).unwrap_or(dec!(1.0)),
                initial_margin: Decimal::from_str(p.initial_margin.trim()).unwrap_or(dec!(0.0)),
                maintenance_margin: dec!(0.0),
                updated_at: Utc::now(),
            });
        }
        let equity = Decimal::from_str(overview.account_equity.trim()).unwrap_or(dec!(0.0));
        let free = Decimal::from_str(overview.available_margin.trim()).unwrap_or(dec!(0.0));
        let locked_in = Decimal::from_str(overview.initial_margin.trim()).unwrap_or(dec!(0.0));
        let balances = overview
            .balances
            .into_iter()
            .map(|b| Balance {
                venue: venue.clone(),
                currency: b.token,
                total: Decimal::from_str(b.balance.trim()).unwrap_or(dec!(0.0)),
                free: Decimal::from_str(b.available.trim()).unwrap_or(dec!(0.0)),
                locked: Decimal::from_str(b.frozen.trim()).unwrap_or(dec!(0.0)),
            })
            .collect();
        Ok(AccountSnapshot {
            venue,
            equity,
            free_margin: free,
            maintenance_margin: locked_in,
            margin_usage_pct: if equity.is_zero() {
                dec!(0.0)
            } else {
                locked_in / equity * dec!(100.0)
            },
            balances,
            positions: account_positions,
            timestamp: Utc::now(),
        })
    }

    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        let (main, agent_key) = self.require_approved_agent()?;
        if !self.agent_approved().await? {
            let agent = self.agent_address_hex().unwrap_or_default();
            anyhow::bail!(
                "PopDEX agent {agent} is not approved for {main}. Run the one-time setup first:\n{}",
                approve_agent_instructions(&main, &agent)
            );
        }
        let venue_symbol = Self::venue_symbol(&order.symbol);
        let symbol_id = self.symbol_id(&venue_symbol).await?;
        let (is_market, tif_byte, price_wad) = match order.order_type {
            OrderType::Market | OrderType::ImmediateOrCancel => (true, 2u8, U256::ZERO),
            OrderType::PostOnly => {
                let p = order.price.ok_or_else(|| anyhow::anyhow!("post-only order needs a price"))?;
                (false, 4u8, dec_to_wad(p)?)
            }
            OrderType::Limit => {
                let tif = match order.time_in_force {
                    TimeInForce::ImmediateOrCancel => 2u8,
                    TimeInForce::FillOrKill => 3u8,
                    TimeInForce::PostOnly => 4u8,
                    TimeInForce::GoodTilCancelled => 1u8,
                };
                let tif = if order.post_only { 4u8 } else { tif };
                let p = order.price.ok_or_else(|| anyhow::anyhow!("limit order needs a price"))?;
                (false, tif, dec_to_wad(p)?)
            }
        };
        let _ = is_market;
        let limit = !is_market && !matches!(order.order_type, OrderType::ImmediateOrCancel);
        let params = pack_order_params(
            order.side,
            limit,
            matches!(order.time_in_force, TimeInForce::GoodTilCancelled) && limit,
            tif_byte == 2,
            tif_byte == 3,
            order.post_only,
            order.reduce_only,
        );
        let qty_wad = dec_to_wad(order.quantity)?;
        if qty_wad.is_zero() {
            anyhow::bail!("PopDEX order quantity converts to zero wad");
        }
        let client_oid = random_client_oid();
        let calldata = IOrder::placeOrderCall {
            account: main.parse().map_err(|e| anyhow::anyhow!("bad main address: {e}"))?,
            clientOrderId: client_oid.into(),
            symbolId: symbol_id,
            orderParams: params.into(),
            price: price_wad,
            qty: qty_wad,
            slippage: U256::ZERO,
            builder: Address::ZERO,
            builderFeeRate: U256::ZERO,
        }
        .abi_encode();
        let receipt = self
            .send_agent_tx(&agent_key, POPDEX_ORDER_PRECOMPILE, calldata)
            .await?;
        let order_id = decode_order_create(&receipt, &main, symbol_id)?.unwrap_or_else(|| {
            // Accepted but the event wasn't in the receipt logs (e.g. a node
            // anomaly) — surface the tx hash so the order can be traced via
            // the explorer instead of fabricating an id.
            format!("tx:{}", receipt.hash)
        });
        Ok(OrderResponse {
            order_id,
            client_order_id: order.client_order_id.clone(),
            venue: self.venue_id(),
            symbol: order.symbol.clone(),
            status: OrderStatus::New,
            original_qty: order.quantity,
            filled_qty: dec!(0.0),
            remaining_qty: order.quantity,
            avg_fill_price: None,
            fee: dec!(0.0),
            fee_currency: "USDT".to_string(),
            raw_response: Some(receipt.hash.clone()),
            timestamp: Utc::now(),
        })
    }

    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
        let (main, agent_key) = self.require_approved_agent()?;
        if !self.agent_approved().await? {
            anyhow::bail!("PopDEX agent is not approved for the main account — run one-time approveAgent first");
        }
        let account: Address = main.parse().map_err(|e| anyhow::anyhow!("bad main address: {e}"))?;
        let calldata = if let Some(id) = req.order_id.as_deref().filter(|s| !s.trim().is_empty()) {
            let order_id: u128 = id.trim().parse().map_err(|_| {
                anyhow::anyhow!("PopDEX order id must be a uint128 integer, got '{id}'")
            })?;
            IOrder::cancelOrderCall {
                account,
                orderId: order_id,
                clientOrderId: B256::ZERO,
            }
            .abi_encode()
        } else if let Some(coid) = req.client_order_id.as_deref().filter(|s| !s.trim().is_empty()) {
            IOrder::cancelOrderCall {
                account,
                orderId: 0,
                clientOrderId: client_oid_bytes(coid).into(),
            }
            .abi_encode()
        } else {
            // No id given: cancel everything resting for this symbol.
            let venue_symbol = Self::venue_symbol(&req.symbol);
            let symbol_id = self.symbol_id(&venue_symbol).await?;
            IOrder::cancelAllOrdersCall {
                account,
                symbolId: symbol_id,
                category: OptCat {
                    isSome: false,
                    value: 0,
                },
                orderCategory: OptOrderCat {
                    isSome: false,
                    value: 0,
                },
                isFullPositionTpsl: OptBool {
                    isSome: false,
                    value: false,
                },
            }
            .abi_encode()
        };
        let receipt = self
            .send_agent_tx(&agent_key, POPDEX_ORDER_PRECOMPILE, calldata)
            .await?;
        let _ = receipt;
        Ok(())
    }

    async fn set_leverage(&self, symbol: &str, leverage: u32) -> anyhow::Result<()> {
        let (main, agent_key) = self.require_approved_agent()?;
        if !self.agent_approved().await? {
            anyhow::bail!("PopDEX agent is not approved for the main account — run one-time approveAgent first");
        }
        let lev: u8 = u8::try_from(leverage)
            .map_err(|_| anyhow::anyhow!("leverage {leverage} exceeds u8 range"))?;
        if lev == 0 {
            anyhow::bail!("leverage must be >= 1");
        }
        let venue_symbol = Self::venue_symbol(symbol);
        let symbol_id = self.symbol_id(&venue_symbol).await?;
        let account: Address = main.parse().map_err(|e| anyhow::anyhow!("bad main address: {e}"))?;
        let token: Address = POPDEX_USDT_TACHYON
            .parse()
            .map_err(|e| anyhow::anyhow!("bad USDT address: {e}"))?;
        let calldata = IUserConfig::updateLeverageCall {
            account,
            request: LevReq {
                newLeverage: lev,
                symbolId: symbol_id,
                tokenAddress: token,
                category: 2, // Category::Futures
            },
        }
        .abi_encode();
        let receipt = self
            .send_agent_tx(&agent_key, POPDEX_USERCONFIG_PRECOMPILE, calldata)
            .await?;
        let _ = receipt;
        Ok(())
    }

    fn supports_live_execution(&self) -> bool {
        true
    }

    fn has_live_credentials(&self) -> bool {
        self.agent_key.read().is_some()
    }
}

// ----- On-chain execution (agent-signed contract calls) -----

use alloy::network::TransactionBuilder;
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::SolCall;

alloy::sol! {
    struct OptCat {
        bool isSome;
        uint8 value;
    }
    struct OptOrderCat {
        bool isSome;
        uint8 value;
    }
    struct OptBool {
        bool isSome;
        bool value;
    }
    struct LevReq {
        uint8 newLeverage;
        uint16 symbolId;
        address tokenAddress;
        uint8 category;
    }
    interface IOrder {
        function placeOrder(address account, bytes32 clientOrderId, uint16 symbolId, bytes32 orderParams, uint256 price, uint256 qty, uint256 slippage, address builder, uint256 builderFeeRate) external returns (bool success);
        function cancelOrder(address account, uint128 orderId, bytes32 clientOrderId) external returns (bool success);
        function cancelAllOrders(address account, uint16 symbolId, OptCat category, OptOrderCat orderCategory, OptBool isFullPositionTpsl) external returns (bool success);
        function closeAllPositions(address account, uint8 category, uint16 symbolId, uint8 positionSide) external returns (bool success);
    }
    interface IUserConfig {
        function updateLeverage(address account, LevReq request) external returns (bool success);
        function updatePositionMode(address account, uint8 positionMode) external returns (bool success);
    }
}

/// Minimal mined-transaction receipt (parsed from JSON-RPC, alloy-agnostic).
#[derive(Debug, Clone)]
/// Mined-transaction receipt. Construction sites must already have verified
/// `status == 1` — a `TxReceipt` value therefore always means success.
struct TxReceipt {
    hash: String,
    logs: Vec<ReceiptLog>,
}

#[derive(Debug, Clone)]
struct ReceiptLog {
    address: String,
    topics: Vec<String>,
    data: String,
}

/// Extract the `orderId` from an `OrderCreate` event for `(account, symbol)`.
/// Returns `None` when the event is absent (caller must not invent an id).
fn decode_order_create(receipt: &TxReceipt, account: &str, symbol_id: u16) -> anyhow::Result<Option<String>> {
    let topic0 = format!("0x{}", hex::encode(order_create_topic()));
    let account_topic = format!("0x000000000000000000000000{}", account.trim().trim_start_matches("0x").to_lowercase());
    let symbol_topic = format!("0x{:064x}", symbol_id);
    for log in &receipt.logs {
        if !log.address.eq_ignore_ascii_case(POPDEX_ORDER_PRECOMPILE) || log.topics.len() < 3 {
            continue;
        }
        if !log.topics[0].eq_ignore_ascii_case(&topic0) {
            continue;
        }
        if !log.topics[1].eq_ignore_ascii_case(&account_topic) {
            continue;
        }
        if !log.topics[2].eq_ignore_ascii_case(&symbol_topic) {
            continue;
        }
        let data = log.data.trim().trim_start_matches("0x");
        if data.len() < 64 {
            continue;
        }
        let order_id = U256::from_str_radix(&data[..64], 16)
            .map_err(|e| anyhow::anyhow!("bad orderId word: {e}"))?;
        let id128 = u128::try_from(order_id)
            .map_err(|_| anyhow::anyhow!("orderId overflows u128"))?;
        if id128 == 0 {
            continue;
        }
        return Ok(Some(id128.to_string()));
    }
    Ok(None)
}

impl PopDexAdapter {
    /// Next agent timestamp-nonce (docs Step 3): `Date.now()` milliseconds,
    /// strictly increasing per signer (same-ms bursts bump by 1, which still
    /// satisfies the window / minimum-bound / no-reuse rules).
    fn next_agent_nonce(&self) -> u64 {
        let now_ms = Utc::now().timestamp_millis().max(0) as u64;
        let mut guard = self.last_agent_nonce.write();
        let next = (*guard).max(now_ms);
        let next = if next <= *guard && *guard != 0 { *guard + 1 } else { next.max(*guard + (*guard != 0) as u64) };
        *guard = next;
        next
    }

    /// Build, agent-sign and submit a legacy transaction to a precompile,
    /// then wait for the receipt (PopDEX finality is instant; 30s cap).
    async fn send_agent_tx(
        &self,
        agent_key: &Zeroizing<String>,
        to: &str,
        calldata: Vec<u8>,
    ) -> anyhow::Result<TxReceipt> {
        let signer: PrivateKeySigner = agent_key
            .trim()
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid agent key: {e}"))?;
        let expected_agent = signer.address();
        if let Some(bound) = self.agent_address_hex() {
            if !bound.eq_ignore_ascii_case(&expected_agent.to_string()) {
                anyhow::bail!("bound agent address does not match the signing key — rebind credentials");
            }
        }
        let provider = ProviderBuilder::new()
            .wallet(signer)
            .connect_http(self.rpc_url().parse().map_err(|e| anyhow::anyhow!("bad RPC url: {e}"))?);
        let chain_id = provider
            .get_chain_id()
            .await
            .map_err(|e| anyhow::anyhow!("PopDEX RPC unreachable: {e}"))?;
        if chain_id != self.chain_id() {
            anyhow::bail!(
                "PopDEX chain id mismatch: node reports {chain_id}, expected {} — wrong network, refusing to sign",
                self.chain_id()
            );
        }
        let to_addr: Address = to
            .parse()
            .map_err(|e| anyhow::anyhow!("bad contract address: {e}"))?;
        let gas_price = provider
            .get_gas_price()
            .await
            .map_err(|e| anyhow::anyhow!("gas price query failed: {e}"))?;
        // Estimate first: a reverting call fails closed HERE with the node
        // error instead of burning a nonce on a doomed transaction.
        let estimate_tx = TransactionRequest::default()
            .with_from(expected_agent)
            .with_to(to_addr)
            .with_input(Bytes::from(calldata.clone()));
        let gas_limit = provider
            .estimate_gas(estimate_tx)
            .await
            .map_err(|e| anyhow::anyhow!("PopDEX estimateGas reverted (call would fail): {e}"))?;
        let gas_limit = gas_limit.saturating_mul(125).saturating_div(100).max(100_000);
        let nonce = self.next_agent_nonce();
        let tx = TransactionRequest::default()
            .with_to(to_addr)
            .with_input(Bytes::from(calldata))
            .with_nonce(nonce)
            .with_chain_id(chain_id)
            .with_gas_price(gas_price)
            .with_gas_limit(gas_limit);
        let pending = provider
            .send_transaction(tx)
            .await
            .map_err(|e| anyhow::anyhow!("PopDEX sendTransaction failed: {e}"))?;
        let tx_hash = format!("{:#x}", pending.tx_hash());
        let receipt = tokio::time::timeout(Duration::from_secs(30), pending.get_receipt())
            .await
            .map_err(|_| anyhow::anyhow!("PopDEX receipt timeout for {tx_hash} — check the explorer before retrying"))?
            .map_err(|e| anyhow::anyhow!("PopDEX receipt query failed for {tx_hash}: {e}"))?;
        if !receipt.status() {
            anyhow::bail!("PopDEX transaction reverted: {tx_hash} — see the explorer for the error code");
        }
        let logs = receipt
            .inner
            .logs()
            .iter()
            .map(|l| ReceiptLog {
                address: l.address().to_string(),
                topics: l.topics().iter().map(|t| t.to_string()).collect(),
                data: l.data().data.to_string(),
            })
            .collect();
        Ok(TxReceipt {
            hash: tx_hash,
            logs,
        })
    }
}

// ----- Agent-setup toolkit (local generation + approval helpers) -----

alloy::sol! {
    interface IAccountSetup {
        function approveAgent(address agent, address delegator, bytes32 name, uint64 expiresAt, uint64 initialNonce, bool isGlobal) external;
    }
}

/// Generate a fresh agent keypair locally (OS RNG). The address has never
/// been approved anywhere, so it carries no replay risk. NEVER re-approve a
/// revoked address — always generate a new one.
pub fn generate_agent_keypair() -> anyhow::Result<(String, Zeroizing<String>)> {
    use rand::RngCore;
    for _ in 0..8 {
        let mut b = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut b);
        let hex_key = format!("0x{}", hex::encode(b));
        b.zeroize();
        if let Ok(signer) = crate::wallet::EvmWalletSigner::from_hex(&hex_key) {
            let addr = signer.address_hex();
            drop(signer);
            return Ok((addr, Zeroizing::new(hex_key)));
        }
    }
    anyhow::bail!("RNG failed to produce a valid secp256k1 key after 8 tries")
}

/// Build the `approveAgent` calldata for an externally-signed approval
/// transaction (agent, delegator=main, default name, never expires).
/// Returns `0x`-hex calldata; the signature must come from the MAIN wallet
/// in the user's own wallet app — the bot never holds the main key.
pub fn build_approve_agent_calldata(
    agent: &str,
    main: &str,
    initial_nonce_ms: u64,
) -> anyhow::Result<String> {
    let agent_addr: Address = agent
        .trim()
        .parse()
        .map_err(|e| anyhow::anyhow!("bad agent address: {e}"))?;
    let main_addr: Address = main
        .trim()
        .parse()
        .map_err(|e| anyhow::anyhow!("bad main address: {e}"))?;
    let call = IAccountSetup::approveAgentCall {
        agent: agent_addr,
        delegator: main_addr,
        name: B256::ZERO,
        expiresAt: 0,
        initialNonce: initial_nonce_ms,
        isGlobal: false,
    };
    Ok(format!("0x{}", hex::encode(call.abi_encode())))
}

/// Sign + submit `approveAgent` with the MAIN key, entirely in-process.
///
/// The main key is transient: it arrives per-call, signs exactly one
/// transaction, and is dropped on return — it is never persisted, never bound
/// to the adapter, and never logged. Operators uncomfortable pasting any main
/// key should use the DIY `cast` command from `approve_agent_cast_command`
/// instead (same calldata, signed in their own shell).
///
/// Fail-closed checks: agent != main, the pasted key must derive the `main`
/// address (catches wrong-key pastes), and the node chain id must match the
/// selected network. Returns the mined transaction hash.
pub async fn submit_approve_agent_with_main(
    network: PopDexNetwork,
    main: &str,
    agent: &str,
    main_key: &Zeroizing<String>,
) -> anyhow::Result<String> {
    use alloy::network::TransactionBuilder;
    use alloy::providers::{Provider, ProviderBuilder};
    use alloy::rpc::types::TransactionRequest;
    use alloy::signers::local::PrivateKeySigner;

    let main_addr: Address = main
        .trim()
        .parse()
        .map_err(|e| anyhow::anyhow!("bad main address: {e}"))?;
    let agent_addr: Address = agent
        .trim()
        .parse()
        .map_err(|e| anyhow::anyhow!("bad agent address: {e}"))?;
    if main_addr == agent_addr {
        anyhow::bail!("agent address equals the main address — generate a fresh agent key instead");
    }
    let signer: PrivateKeySigner = main_key
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("bad main private key (want 0x-hex secp256k1)"))?;
    if signer.address() != main_addr {
        anyhow::bail!("the pasted private key does not match the main account address — refusing to sign");
    }
    let now_ms = Utc::now().timestamp_millis().max(0) as u64;
    let calldata_hex = build_approve_agent_calldata(
        &agent_addr.to_string(),
        &main_addr.to_string(),
        now_ms,
    )?;
    let calldata =
        hex::decode(calldata_hex.trim().trim_start_matches("0x"))
            .map_err(|e| anyhow::anyhow!("calldata encode failed: {e}"))?;

    let provider = ProviderBuilder::new().wallet(signer).connect_http(
        network
            .rpc_url()
            .parse()
            .map_err(|e| anyhow::anyhow!("bad RPC url: {e}"))?,
    );
    let chain_id = provider
        .get_chain_id()
        .await
        .map_err(|e| anyhow::anyhow!("PopDEX RPC unreachable: {e}"))?;
    if chain_id != network.chain_id() {
        anyhow::bail!(
            "PopDEX chain id mismatch: node reports {chain_id}, expected {} — wrong network, refusing to sign",
            network.chain_id()
        );
    }
    let to_addr: Address = POPDEX_ACCOUNT_PRECOMPILE
        .parse()
        .map_err(|e| anyhow::anyhow!("bad contract address: {e}"))?;
    let gas_price = provider
        .get_gas_price()
        .await
        .map_err(|e| anyhow::anyhow!("gas price query failed: {e}"))?;
    // Estimate first: a reverting call fails closed HERE with the node error
    // instead of burning a main-key signature on a doomed transaction.
    let estimate_tx = TransactionRequest::default()
        .with_from(main_addr)
        .with_to(to_addr)
        .with_input(Bytes::from(calldata.clone()));
    let gas_limit = provider
        .estimate_gas(estimate_tx)
        .await
        .map_err(|e| anyhow::anyhow!("PopDEX estimateGas reverted (call would fail): {e}"))?;
    let gas_limit = gas_limit.saturating_mul(125).saturating_div(100).max(100_000);
    let tx = TransactionRequest::default()
        .with_to(to_addr)
        .with_input(Bytes::from(calldata))
        .with_nonce(now_ms)
        .with_chain_id(chain_id)
        .with_gas_price(gas_price)
        .with_gas_limit(gas_limit);
    let pending = provider
        .send_transaction(tx)
        .await
        .map_err(|e| anyhow::anyhow!("PopDEX sendTransaction failed: {e}"))?;
    let tx_hash = format!("{:#x}", pending.tx_hash());
    let receipt = tokio::time::timeout(Duration::from_secs(30), pending.get_receipt())
        .await
        .map_err(|_| {
            anyhow::anyhow!("PopDEX receipt timeout for {tx_hash} — check the explorer before retrying")
        })?
        .map_err(|e| anyhow::anyhow!("PopDEX receipt query failed for {tx_hash}: {e}"))?;
    if !receipt.status() {
        anyhow::bail!("PopDEX transaction reverted: {tx_hash} — see the explorer for the error code");
    }
    Ok(tx_hash)
}

/// DIY `cast` command for the same approval (network-aware RPC + live nonce).
/// For operators who won't paste the main key anywhere but their own shell.
/// Docs: <https://popdex.xyz/docs/product-docs/onboarding/how-to-trade-via-api> (Step 4).
pub fn approve_agent_cast_command(
    network: PopDexNetwork,
    main: &str,
    agent: &str,
    nonce_ms: u64,
) -> String {
    format!(
        "cast send {account} \"approveAgent(address,address,bytes32,uint64,uint64,bool)\" \
  {agent} {main} 0x0000000000000000000000000000000000000000000000000000000000000000 0 {nonce} false \
  --rpc-url {rpc} --private-key $MAIN_PRIVATE_KEY",
        account = POPDEX_ACCOUNT_PRECOMPILE,
        agent = agent.trim(),
        main = main.trim(),
        nonce = nonce_ms,
        rpc = network.rpc_url(),
    )
}

/// EIP-681 request URI for the approval call (paste into a wallet app that
/// accepts contract-interaction URIs, or scan if it renders QR elsewhere).
pub fn approve_agent_eip681(main: &str, agent: &str, nonce_ms: u64, chain_id: u64) -> String {
    format!(
        "ethereum:{}@{}/approveAgent?address={}&address={}&bytes32=0x0000000000000000000000000000000000000000000000000000000000000000&uint256=0&uint256={}&bool=false",
        POPDEX_ACCOUNT_PRECOMPILE, chain_id, agent.trim(), main.trim(), nonce_ms,
    )
}

/// Check whether `agent` is currently approved and unexpired for `main`
/// (public REST, no credentials).
pub async fn check_agent_approval(
    network: PopDexNetwork,
    main: &str,
    agent: &str,
) -> anyhow::Result<(bool, usize)> {
    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_default();
    let url = format!("{}/api/v1/account/{}/agents", network.rest_base(), main.trim());
    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("PopDEX REST error: {}", resp.status());
    }
    let env: Envelope<Vec<AgentRow>> = resp.json().await?;
    if env.code.trim() != "200" {
        anyhow::bail!("PopDEX API error {}: {}", env.code, env.msg);
    }
    let agents = env.data.unwrap_or_default();
    let ok = agents
        .iter()
        .any(|a| !a.is_expired && a.agent.eq_ignore_ascii_case(agent.trim()));
    Ok((ok, agents.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_parse() {
        assert_eq!(PopDexNetwork::parse("mainnet"), Some(PopDexNetwork::Mainnet));
        assert_eq!(PopDexNetwork::parse("TESTNET"), Some(PopDexNetwork::Testnet));
        assert_eq!(PopDexNetwork::parse("beta"), None);
        assert_eq!(PopDexNetwork::default(), PopDexNetwork::Mainnet);
        assert_eq!(PopDexNetwork::Mainnet.chain_id(), 2184);
        assert_eq!(PopDexNetwork::Testnet.chain_id(), 34952);
    }

    #[tokio::test]
    async fn main_key_approval_validates_before_any_signature() {
        use zeroize::Zeroizing;
        let main = "0x1111111111111111111111111111111111111111";
        let agent = "0x2222222222222222222222222222222222222222";
        // Scalar-1 key derives 0x7e5f… — not `main`, so key/address mismatch
        // fails closed before any RPC contact.
        let wrong = Zeroizing::new(
            "0x0000000000000000000000000000000000000000000000000000000000000001".to_string(),
        );
        assert!(submit_approve_agent_with_main(PopDexNetwork::Testnet, main, agent, &wrong)
            .await
            .is_err());
        // Agent == main is refused even with a matching key shape.
        let bad = Zeroizing::new("0xnope".to_string());
        assert!(submit_approve_agent_with_main(PopDexNetwork::Testnet, main, main, &bad)
            .await
            .is_err());
        assert!(submit_approve_agent_with_main(PopDexNetwork::Testnet, "nope", agent, &bad)
            .await
            .is_err());
    }

    #[test]
    fn diy_cast_command_is_network_aware() {
        let main = "0x1111111111111111111111111111111111111111";
        let agent = "0x2222222222222222222222222222222222222222";
        let cmd = approve_agent_cast_command(PopDexNetwork::Testnet, main, agent, 42);
        for marker in [
            "approveAgent(address,address,bytes32,uint64,uint64,bool)",
            "testnet-app.popdex.xyz",
            main,
            agent,
            "MAIN_PRIVATE_KEY",
        ] {
            assert!(cmd.contains(marker), "cast command missing {marker}");
        }
        assert!(approve_agent_cast_command(PopDexNetwork::Mainnet, main, agent, 42)
            .contains("app.popdex.xyz"));
    }

    #[test]
    fn venue_symbol_mapping() {
        assert_eq!(PopDexAdapter::venue_symbol("BTC-USDC"), "BTCUSDT");
        assert_eq!(PopDexAdapter::venue_symbol("eth"), "ETHUSDT");
        assert_eq!(PopDexAdapter::venue_symbol("QQQ"), "QQQUSDT");
    }

    #[test]
    fn credentials_require_delegated_agent() {
        let a = PopDexAdapter::new(None);
        assert!(!a.has_live_credentials());
        assert!(a.update_credentials(None, Some("not-an-address".into()), None).is_err());
        // Scalar-1 agent key derives 0x7e5f…; binding it against itself
        // (i.e. the MAIN key) must be refused.
        let agent_key = "0000000000000000000000000000000000000000000000000000000000000001";
        assert!(a
            .update_credentials(
                None,
                Some("0x7e5f4552091a69125d5dfcb7b8c2659029395bdf".into()),
                Some(agent_key.into()),
            )
            .is_err());
        // Delegated binding: main differs from the agent key.
        a.update_credentials(
            None,
            Some("0x0000000000000000000000000000000000000001".into()),
            Some(agent_key.into()),
        )
        .unwrap();
        assert!(a.has_live_credentials());
        assert_eq!(
            a.agent_address_hex().as_deref(),
            Some("0x7e5f4552091a69125d5dfcb7b8c2659029395bdf")
        );
        assert_eq!(
            a.wallet_address_hex().as_deref(),
            Some("0x0000000000000000000000000000000000000001")
        );
        a.update_credentials(None, None, None).unwrap();
        assert!(!a.has_live_credentials());
    }

    #[test]
    fn pack_limit_buy_gtc() {
        let b = pack_order_params(Side::Buy, true, true, false, false, false, false);
        assert_eq!(&b[..8], &[2, 0, 0, 1, 0, 0, 0, 0]);
        assert!(b[8..].iter().all(|&x| x == 0));
    }

    #[test]
    fn pack_market_sell_ioc_reduce_only() {
        let b = pack_order_params(Side::Sell, false, false, true, false, false, true);
        assert_eq!(&b[..8], &[2, 1, 1, 2, 0, 0, 1, 0]);
    }

    #[test]
    fn pack_post_only_forces_tif() {
        let b = pack_order_params(Side::Buy, true, false, false, false, true, false);
        assert_eq!(b[3], 4);
    }

    #[test]
    fn wad_conversions() {
        assert_eq!(dec_to_wad(dec!(81289)).unwrap(), U256::from(81_289u64) * U256::from(1_000_000_000_000_000_000u64));
        assert_eq!(
            dec_to_wad(Decimal::from_str("0.008").unwrap()).unwrap(),
            U256::from(8_000_000_000_000_000u64)
        );
        assert_eq!(dec_to_wad(dec!(0.0)).unwrap(), U256::ZERO);
        assert!(dec_to_wad(dec!(-1.0)).is_err());
        // Dust past 18dp truncates rather than errors.
        assert_eq!(dec_to_wad(Decimal::from_str("1.0000000000000000009").unwrap()).unwrap(), U256::from(1_000_000_000_000_000_000u64));
    }

    #[test]
    fn client_oid_shapes() {
        let r = random_client_oid();
        assert_ne!(r, [0u8; 32]);
        let e = client_oid_bytes("");
        assert_ne!(e, [0u8; 32]);
        let s = client_oid_bytes("abc");
        assert_eq!(&s[..3], b"abc");
        assert!(s[3..].iter().all(|&x| x == 0));
    }

    #[test]
    fn order_create_topic_matches_sha3() {
        use sha3::{Digest, Keccak256};
        let mut h = Keccak256::new();
        h.update(b"OrderCreate(address,uint16,uint128,bytes32,int256,int256,int256,uint8,bool,uint32)");
        let expect = B256::from_slice(&h.finalize());
        assert_eq!(order_create_topic(), expect);
    }

    #[test]
    fn decode_order_create_synthetic() {
        let topic0 = format!("0x{}", hex::encode(order_create_topic()));
        let account = "0x1111111111111111111111111111111111111111";
        let account_topic = format!("0x000000000000000000000000{}", &account[2..]);
        let symbol_topic = format!("0x{:064x}", 20000u16);
        let mut data = format!("{:064x}", 12345u128);
        data.push_str(&"00".repeat(32 * 8));
        let receipt = TxReceipt {
            hash: "0xabc".into(),
            logs: vec![ReceiptLog {
                address: POPDEX_ORDER_PRECOMPILE.into(),
                topics: vec![topic0, account_topic, symbol_topic],
                data: format!("0x{data}"),
            }],
        };
        assert_eq!(
            decode_order_create(&receipt, account, 20000).unwrap(),
            Some("12345".to_string())
        );
        // Wrong symbol → no match, never an invented id.
        assert_eq!(decode_order_create(&receipt, account, 20001).unwrap(), None);
    }

    #[test]
    fn envelope_rejects_non_200() {
        let env: Envelope<Vec<TickerRow>> = serde_json::from_str(
            r#"{"code":"11100","msg":"Account does not exist","data":null}"#,
        )
        .unwrap();
        assert_ne!(env.code.trim(), "200");
    }

    #[test]
    fn parse_live_ticker_fixture() {
        let row: TickerRow = serde_json::from_str(
            r#"{"category":"futures","symbolId":"20000","symbol":"BTCUSDT","lastPrice":"81289","ask1Price":"81290","bid1Price":"81289","bid1Size":"0.008","ask1Size":"2.904","markPrice":"81290","fundingRate":"0.000012","volume24h":"447.1527","turnover24h":"35986114.0576","status":"Trading"}"#,
        )
        .unwrap();
        assert_eq!(row.symbol_id, "20000");
        assert_eq!(dec_of(&row.last_price).unwrap(), dec!(81289));
    }

    #[test]
    fn agent_nonce_monotonic() {
        let a = PopDexAdapter::new(None);
        let n1 = a.next_agent_nonce();
        let n2 = a.next_agent_nonce();
        assert!(n2 > n1);
        assert!(n1 > 1_700_000_000_000u64); // ms-scale timestamp nonce
    }

    #[test]
    fn agent_keygen_fresh_and_valid() {
        let (addr, key) = generate_agent_keypair().unwrap();
        assert!(is_valid_evm_address(&addr));
        let k = key.trim();
        assert!(k.len() == 66 && k.starts_with("0x") && k[2..].chars().all(|c| c.is_ascii_hexdigit()));
        // Two generations must differ (fresh randomness).
        let (addr2, _) = generate_agent_keypair().unwrap();
        assert_ne!(addr, addr2);
    }

    #[test]
    fn approve_calldata_shape() {
        let data = build_approve_agent_calldata(
            "0x0000000000000000000000000000000000000002",
            "0x0000000000000000000000000000000000000001",
            1_789_816_000_000,
        )
        .unwrap();
        assert!(data.starts_with("0x"));
        // selector(4) + 6 args × 32 bytes
        assert_eq!(data.len(), 2 + 8 + 6 * 64);
        // selector must equal keccak("approveAgent(address,address,bytes32,uint64,uint64,bool)")[..4]
        let sel = &data[2..10];
        let expect = &hex::encode(alloy::primitives::keccak256(
            b"approveAgent(address,address,bytes32,uint64,uint64,bool)",
        ))[..8];
        assert_eq!(sel, expect);
        // agent + delegator addresses land in arg slots 0 and 1.
        assert!(data[10..74].ends_with("0000000000000000000000000000000000000002"));
        assert!(data[74..138].ends_with("0000000000000000000000000000000000000001"));
        assert!(build_approve_agent_calldata("nope", "0x01", 1).is_err());
    }

    #[test]
    fn approve_eip681_shape() {
        let u = approve_agent_eip681("0xMAIN", "0xAGENT", 42, 2184);
        assert!(u.starts_with("ethereum:0x0000000000000000000000000000000000001008@2184/approveAgent?"));
        assert!(u.contains("uint256=42"));
    }

    #[test]
    fn approve_instructions_name_checks() {
        let s = approve_agent_instructions("0xmain", "0xagent");
        assert!(s.contains("approveAgent"));
        assert!(s.contains("0xagent"));
        assert!(s.contains(POPDEX_ACCOUNT_PRECOMPILE));
    }
}
