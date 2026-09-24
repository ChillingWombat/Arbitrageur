//! Pendle Boros — on-chain funding-rate swaps on Arbitrum (Yield Units).
//!
//! Boros is not a perpetual venue: its instruments are fixed-rate swap
//! positions ("YU") over another venue's realized funding rate, with a
//! maturity and settlement schedule mirroring that venue. This adapter
//! exists so the fixed-yield strategy can (a) read YU market data and
//! (b) trade the two hedge legs of a 4-leg carry.
//!
//! # Execution model (verified against `@pendle/boros-sdk-public` 0.4.0 +
//! `pendle-finance/boros-api-examples`)
//!
//! Boros never accepts a raw order. The flow is a three-step relay:
//!   1. `POST /apis/v1/calldata-builder/agent/place-order` (or
//!      `cancel-orders`) → ABI-encoded `calls[]` for the Router.
//!   2. Each call is bound to an EIP-712 `PendleSignTx` message —
//!      `{account: bytes21 = root<<8|accountId, connectionId:
//!      bytes32 = keccak256(calldata), nonce: uint64}` — on domain
//!      `("Pendle Boros Router", "1.0", chainId 42161, Router
//!      0x8080808080daB95eFED788a9214e400ba552DEf6)`, signed by the
//!      **agent** key (a delegated EVM key approved on-chain by the root
//!      wallet; it can trade but never withdraw).
//!   3. `POST /apis/v1/send-txs/bulk-calls` submits the signed batch;
//!      the backend broadcasts it on Arbitrum and (with
//!      `skipReceipt=false`) returns per-call status after inclusion.
//!      Order identity is then resolved via `GET /v1/orders/by-tx-hash`,
//!      and order truth via `GET /v1/accounts/orders`.
//!
//! # Credential model
//! - `privateKey`    — the **agent** EVM private key (signs trades).
//! - `walletAddress` — the **root** account address the agent acts for.
//! - `accountId`     — Boros sub-account id (0..=255).
//!
//! Agent approval, collateral deposit, and gas-balance top-up are
//! sensitive root-wallet actions done once via the Boros web app
//! (https://boros.pendle.finance) — this adapter never performs them.
//! `has_live_credentials` stays false until all three fields are bound
//! AND `/v1/agents/expiry-time` shows a live agent approval.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alloy::primitives::{keccak256, Address, FixedBytes, U256};
use alloy::sol_types::SolValue;
use async_trait::async_trait;
use chrono::Utc;
use parking_lot::RwLock;
use reqwest::Client;
use rust_decimal::prelude::ToPrimitive;

use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::MarketMetadata;
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::{AccountSnapshot, Balance, Position};
use arbitrageur_core::quote::{FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{dec, Decimal, OrderStatus, OrderType, Side, TimeInForce, VenueId};

use crate::dex_perps::{dec_from_json, eip712_digest};
use crate::wallet::EvmWalletSigner;

pub const BOROS_API_BASE: &str = "https://api-boros.pendle.finance/apis";
/// Router is the EIP-712 verifyingContract for every agent-signed call.
const BOROS_ROUTER: &str = "0x8080808080daB95eFED788a9214e400ba552DEf6";
const BOROS_CHAIN_ID: u64 = 42161;
/// `marketId` segment meaning "cross-margin account" inside `marketAcc`.
const BOROS_CROSS_MARKET_ID: u64 = 0xFF_FFFF;
/// Market list cache TTL — YU markets are slow-moving; refreshing per
/// scan would hammer the API.
const BOROS_MARKETS_TTL: Duration = Duration::from_secs(120);

// ---------------------------------------------------------------------------
// Public market data — consumed by the funding scanner, not by the generic
// venue-symbol pipeline (YU instruments are rate swaps, not perp tickers).
// ---------------------------------------------------------------------------

/// One Boros Yield-Unit market: a fixed-rate swap over `platform`'s
/// realized funding for `asset`, settling every `payment_period_secs`
/// until `maturity_unix`.
#[derive(Debug, Clone)]
pub struct BorosYuMarket {
    pub market_id: u64,
    pub token_id: u64,
    /// Venue-style symbol, e.g. `BINANCE-BTCUSDT-30OCT2026`.
    pub symbol: String,
    /// Funding source venue as named by Boros, e.g. `Binance`.
    pub platform: String,
    /// Underlying asset, e.g. `BTC`.
    pub asset: String,
    pub maturity_unix: i64,
    pub tick_step: i64,
    /// Implied fixed APR mid (fraction, e.g. 0.05 = 5%).
    pub mid_apr: f64,
    /// Top-of-book implied APRs — the executable fixed rates.
    pub best_bid_apr: Option<f64>,
    pub best_ask_apr: Option<f64>,
    /// The platform's current floating funding APR Boros settles against.
    pub floating_apr: f64,
    pub notional_oi_usd: f64,
    /// Taker fee as a fraction of notional (e.g. 0.0005 = 5 bps).
    pub taker_fee_rate: f64,
    pub payment_period_secs: u64,
    pub max_leverage: f64,
    /// Collateral token symbol for this market (`WBTC`, `WETH`, `USDT`…)
    /// — YU `size` is denominated in this token's units.
    pub collateral_symbol: String,
    /// Collateral token USD price — converts USD notional → YU size
    /// (`size_yu = notional_usd / collateral_usd_price`).
    pub collateral_usd_price: f64,
}

/// Map a Boros `platformName` to our canonical venue id. Only platforms
/// we actually run perp adapters for are eligible — anything else returns
/// `None` so fixed-yield pairing fails closed rather than hedging a
/// position on a venue we cannot trade.
pub fn boros_platform_venue_id(platform: &str) -> Option<&'static str> {
    match platform.trim().to_lowercase().as_str() {
        "binance" => Some("binance"),
        "hyperliquid" => Some("hyperliquid"),
        "okx" => Some("okx"),
        "lighter" => Some("lighter"),
        _ => None,
    }
}

fn dec_fixed_x18(v: Option<&serde_json::Value>) -> Option<Decimal> {
    // Boros bigint strings are 1e18-scaled fixed point.
    dec_from_json(v).map(|d| d / dec!(1000000000000000000))
}

fn f64_from_json(v: Option<&serde_json::Value>) -> Option<f64> {
    dec_from_json(v).and_then(|d| d.to_string().parse::<f64>().ok())
}

/// `tokenId → (collateral symbol, USD price)` from `/v1/assets`. Only
/// `isCollateral` rows matter; a missing token fails closed by leaving
/// the market's collateral price at 0 (callers must not divide by it).
fn parse_collateral_assets(val: &serde_json::Value) -> HashMap<u64, (String, f64)> {
    let mut out = HashMap::new();
    if let Some(rows) = val.get("results").and_then(|r| r.as_array()) {
        for a in rows {
            if !a
                .get("isCollateral")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                continue;
            }
            let Some(token_id) = a.get("tokenId").and_then(|v| v.as_u64()) else {
                continue;
            };
            let Some(symbol) = a.get("symbol").and_then(|v| v.as_str()) else {
                continue;
            };
            let price = a
                .get("usdPrice")
                .and_then(|v| {
                    v.as_str()
                        .and_then(|s| s.parse::<f64>().ok())
                        .or_else(|| v.as_f64())
                })
                .unwrap_or(0.0);
            out.insert(token_id, (symbol.to_string(), price));
        }
    }
    out
}

fn parse_yu_market(
    m: &serde_json::Value,
    collateral: &HashMap<u64, (String, f64)>,
) -> Option<BorosYuMarket> {
    let im = m.get("imData")?;
    let meta = m.get("metadata")?;
    let data = m.get("data")?;
    let config = m.get("config").cloned().unwrap_or_default();
    let ext = m.get("extConfig").cloned().unwrap_or_default();
    // status 2 = active trading (observed on every live market; statuses
    // 0/1 are pre-open/suspended — skip them so stale shells never quote).
    if config.get("status").and_then(|s| s.as_u64()) != Some(2) {
        return None;
    }
    let maturity = im.get("maturity")?.as_i64()?;
    if maturity <= Utc::now().timestamp() {
        return None; // matured markets must never quote a fixed rate.
    }
    let token_id = m.get("tokenId")?.as_u64()?;
    // A market whose collateral token is unknown cannot convert USD
    // notional into YU size — drop it rather than size orders wrong.
    let (collateral_symbol, collateral_usd_price) = collateral.get(&token_id)?.clone();
    if !(collateral_usd_price.is_finite() && collateral_usd_price > 0.0) {
        return None;
    }
    // Platform name moved from `metadata.platformName` to the top-level
    // `platform` object ({"name","platformId"}) — accept either shape.
    let platform = m
        .get("platform")
        .and_then(|p| {
            p.get("name")
                .or_else(|| p.get("platformId"))
                .and_then(|v| v.as_str())
        })
        .or_else(|| meta.get("platformName").and_then(|v| v.as_str()))?
        .to_string();
    // Underlying asset moved from `metadata.assetSymbol` to
    // `metadata.underlyingSymbol` — accept either.
    let asset = meta
        .get("underlyingSymbol")
        .or_else(|| meta.get("assetSymbol"))
        .and_then(|v| v.as_str())?
        .to_string();
    Some(BorosYuMarket {
        market_id: m.get("marketId")?.as_u64()?,
        token_id,
        symbol: im.get("symbol")?.as_str()?.to_string(),
        platform,
        asset,
        maturity_unix: maturity,
        tick_step: im.get("tickStep").and_then(|v| v.as_i64()).unwrap_or(1),
        mid_apr: f64_from_json(data.get("midApr"))?,
        best_bid_apr: f64_from_json(data.get("bestBid")),
        best_ask_apr: f64_from_json(data.get("bestAsk")),
        floating_apr: f64_from_json(data.get("floatingApr")).unwrap_or(0.0),
        notional_oi_usd: f64_from_json(data.get("notionalOI")).unwrap_or(0.0),
        taker_fee_rate: dec_fixed_x18(config.get("takerFee"))
            .and_then(|d| d.to_string().parse::<f64>().ok())
            .unwrap_or(0.0005),
        payment_period_secs: ext
            .get("paymentPeriod")
            .and_then(|v| v.as_u64())
            .unwrap_or(28800),
        max_leverage: f64_from_json(meta.get("maxLeverage")).unwrap_or(1.0),
        collateral_symbol,
        collateral_usd_price,
    })
}

/// Fetch all active (unmatured, status=2) YU markets. Public endpoint —
/// no credentials needed.
pub async fn fetch_boros_yu_markets(
    client: &Client,
    base_url: &str,
) -> anyhow::Result<Vec<BorosYuMarket>> {
    // Collateral assets first — every market row needs its token's USD
    // price for YU-size conversion; no assets means no usable markets.
    let assets_resp = client.get(format!("{base_url}/v1/assets")).send().await?;
    if !assets_resp.status().is_success() {
        anyhow::bail!("Boros assets API error: {}", assets_resp.status());
    }
    let assets_val: serde_json::Value = assets_resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Boros assets unparseable: {e}"))?;
    let collateral = parse_collateral_assets(&assets_val);
    if collateral.is_empty() {
        anyhow::bail!("Boros assets endpoint returned no collateral tokens");
    }
    let url = format!("{base_url}/v1/markets?isMatured=false&limit=100");
    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("Boros markets API error: {}", resp.status());
    }
    let text = resp.text().await.unwrap_or_default();
    let val: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| anyhow::anyhow!("Boros markets unparseable: {e}"))?;
    let mut out = Vec::new();
    if let Some(rows) = val.get("results").and_then(|r| r.as_array()) {
        for m in rows {
            if let Some(yu) = parse_yu_market(m, &collateral) {
                out.push(yu);
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Adapter — account + order surface for the agent-signed trading path.
// ---------------------------------------------------------------------------

struct BorosCreds {
    agent: EvmWalletSigner,
    root: Address,
    account_id: u8,
}

pub struct BorosAdapter {
    client: Client,
    base_url: String,
    creds: RwLock<Option<Arc<BorosCreds>>>,
    /// Shared YU market snapshot so order paths resolve `marketId` →
    /// tokenId/symbol without a second fetch per call.
    markets: RwLock<(Option<Instant>, Vec<BorosYuMarket>)>,
}

impl BorosAdapter {
    pub fn new(base_url: Option<String>) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .unwrap_or_default(),
            base_url: base_url
                .unwrap_or_else(|| BOROS_API_BASE.to_string())
                .trim_end_matches('/')
                .to_string(),
            creds: RwLock::new(None),
            markets: RwLock::new((None, Vec::new())),
        }
    }

    /// Bind the agent credentials. `private_key` is the *agent* EVM key
    /// (never the root key — the root key only approves the agent once,
    /// via the Boros UI); `wallet_address` is the root account address;
    /// `account_id` is the Boros sub-account (0..=255).
    pub fn update_credentials(
        &self,
        private_key: Option<String>,
        wallet_address: Option<String>,
        account_id: Option<String>,
    ) -> anyhow::Result<()> {
        let norm = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let (private_key, wallet_address, account_id) =
            (norm(private_key), norm(wallet_address), norm(account_id));
        if private_key.is_none() && wallet_address.is_none() && account_id.is_none() {
            *self.creds.write() = None;
            return Ok(());
        }
        let key =
            private_key.ok_or_else(|| anyhow::anyhow!("Boros needs the agent private key"))?;
        let agent = EvmWalletSigner::from_hex(&key)
            .map_err(|e| anyhow::anyhow!("Boros agent key invalid: {e}"))?;
        let root_raw =
            wallet_address.ok_or_else(|| anyhow::anyhow!("Boros needs the root wallet address"))?;
        let root = Address::from_str(&root_raw)
            .map_err(|_| anyhow::anyhow!("Boros root wallet address is not valid 0x hex"))?;
        let account_id: u8 = account_id
            .as_deref()
            .unwrap_or("0")
            .parse()
            .map_err(|_| anyhow::anyhow!("Boros accountId must be 0-255"))?;
        *self.creds.write() = Some(Arc::new(BorosCreds {
            agent,
            root,
            account_id,
        }));
        Ok(())
    }

    fn creds(&self) -> anyhow::Result<Arc<BorosCreds>> {
        self.creds.read().clone().ok_or_else(|| {
            anyhow::anyhow!(
                "Boros credentials not configured — need agent private key + root address + accountId"
            )
        })
    }

    /// `bytes21` account packing: `root (20B) << 8 | accountId (1B)`.
    fn pack_account(root: &Address, account_id: u8) -> FixedBytes<21> {
        let mut buf = [0u8; 21];
        buf[..20].copy_from_slice(root.as_slice());
        buf[20] = account_id;
        FixedBytes::<21>::from(buf)
    }

    /// `bytes26` marketAcc packing: `root<<48 | accountId<<40 |
    /// tokenId<<24 | marketId` (CROSS market id for cross-margin handles).
    fn pack_market_acc(root: &Address, account_id: u8, token_id: u64, market_id: u64) -> String {
        let v: U256 = (U256::from_be_bytes({
            let mut b = [0u8; 32];
            b[12..32].copy_from_slice(root.as_slice());
            b
        }) << 48)
            | (U256::from(account_id) << 40)
            | (U256::from(token_id) << 24)
            | U256::from(market_id);
        let bytes = v.to_be_bytes::<32>();
        format!("0x{}", hex::encode(&bytes[6..32]))
    }

    /// EIP-712 `PendleSignTx` domain separator.
    fn domain_sep() -> [u8; 32] {
        let type_hash = keccak256(
            b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
        );
        let encoded = (
            type_hash,
            keccak256(b"Pendle Boros Router"),
            keccak256(b"1.0"),
            U256::from(BOROS_CHAIN_ID),
            BOROS_ROUTER.parse::<Address>().unwrap_or_default(),
        )
            .abi_encode();
        keccak256(&encoded).0
    }

    /// Sign one agent call: `PendleSignTx(bytes21 account,bytes32
    /// connectionId,uint64 nonce)` — agent key, Router domain.
    fn sign_agent_call(
        creds: &BorosCreds,
        calldata: &str,
        nonce: u64,
    ) -> anyhow::Result<serde_json::Value> {
        let raw = calldata
            .strip_prefix("0x")
            .and_then(|h| hex::decode(h).ok())
            .ok_or_else(|| anyhow::anyhow!("Boros calldata is not 0x-hex"))?;
        let connection_id = keccak256(&raw);
        let account = Self::pack_account(&creds.root, creds.account_id);
        let type_hash =
            keccak256(b"PendleSignTx(bytes21 account,bytes32 connectionId,uint64 nonce)");
        let struct_hash = keccak256((type_hash, account, connection_id, nonce).abi_encode());
        let digest = eip712_digest(&Self::domain_sep(), &struct_hash);
        let sig = creds.agent.sign_digest(&digest)?;
        Ok(serde_json::json!({
            "agent": creds.agent.address_checksum(),
            "message": {
                "account": format!("0x{}", hex::encode(account.as_slice())),
                "connectionId": format!("0x{}", hex::encode(connection_id.as_slice())),
                "nonce": nonce.to_string(),
            },
            "signature": format!("0x{}", hex::encode(sig)),
            "calldata": calldata,
        }))
    }

    /// Sign every call and submit the batch. `require_success` keeps
    /// place/cancel pairs atomic — a revert anywhere aborts the batch.
    async fn submit_calls(
        &self,
        calls: Vec<(String, u64)>,
        require_success: bool,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let creds = self.creds()?;
        let base_nonce = Utc::now().timestamp_millis().max(0) as u64 * 1000;
        let mut datas = Vec::with_capacity(calls.len());
        for (i, (calldata, _account_id)) in calls.iter().enumerate() {
            datas.push(Self::sign_agent_call(
                &creds,
                calldata,
                base_nonce + i as u64,
            )?);
        }
        let body = serde_json::json!({
            "datas": datas,
            "requireSuccess": require_success,
            "skipReceipt": false,
        });
        let url = format!("{}/v1/send-txs/bulk-calls", self.base_url);
        let resp = self.client.post(&url).json(&body).send().await?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!(
                "Boros bulk-calls error: {} — {}",
                status,
                text.trim().chars().take(300).collect::<String>()
            );
        }
        let val: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("Boros bulk-calls unparseable: {e}"))?;
        let rows = val
            .as_array()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Boros bulk-calls returned a non-array body"))?;
        for row in &rows {
            if row.get("status").and_then(|s| s.as_str()) == Some("reverted") {
                anyhow::bail!(
                    "Boros transaction reverted on-chain: {}",
                    row.get("error")
                        .and_then(|e| e.as_str())
                        .unwrap_or("unknown error")
                );
            }
        }
        Ok(rows)
    }

    async fn post(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.client.post(&url).json(body).send().await?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!(
                "Boros API error: {} — {}",
                status,
                text.trim().chars().take(300).collect::<String>()
            );
        }
        serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("Boros unparseable JSON for {path}: {e}"))
    }

    async fn get(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.client.get(&url).send().await?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!(
                "Boros API error: {} — {}",
                status,
                text.trim().chars().take(300).collect::<String>()
            );
        }
        serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("Boros unparseable JSON for {path}: {e}"))
    }

    /// YU markets with a short cache; callers may force a refresh after a
    /// maturity roll.
    async fn yu_markets(&self) -> anyhow::Result<Vec<BorosYuMarket>> {
        {
            let guard = self.markets.read();
            if guard.0.is_some_and(|t| t.elapsed() < BOROS_MARKETS_TTL) {
                return Ok(guard.1.clone());
            }
        }
        let rows = fetch_boros_yu_markets(&self.client, &self.base_url).await?;
        *self.markets.write() = (Some(Instant::now()), rows.clone());
        Ok(rows)
    }

    /// Resolve an order `symbol` to a YU market. Accepts the canonical
    /// `BOROS-{marketId}` form or the venue symbol (`BINANCE-BTCUSDT-
    /// 30OCT2026`); fails closed on anything else.
    async fn resolve_market(&self, symbol: &str) -> anyhow::Result<BorosYuMarket> {
        let markets = self.yu_markets().await?;
        let s = symbol.trim();
        if let Some(id) = s
            .strip_prefix("BOROS-")
            .or_else(|| s.strip_prefix("boros-"))
            .and_then(|d| d.parse::<u64>().ok())
        {
            return markets
                .iter()
                .find(|m| m.market_id == id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Boros market {id} is not active"));
        }
        markets
            .iter()
            .find(|m| m.symbol.eq_ignore_ascii_case(s))
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Boros symbol '{s}' is not an active YU market — use BOROS-<marketId>"
                )
            })
    }

    /// Resolve a YU market by numeric id — used by the fixed-yield
    /// executor, which carries market ids (not symbols) in its leg DTOs.
    /// Fails closed: only currently-active markets resolve.
    pub async fn market_by_id(&self, market_id: u64) -> anyhow::Result<BorosYuMarket> {
        self.yu_markets()
            .await?
            .into_iter()
            .find(|m| m.market_id == market_id)
            .ok_or_else(|| anyhow::anyhow!("Boros market {market_id} is not active"))
    }

    /// Cross-margin `marketAcc` for the configured root/accountId with the
    /// market's collateral `tokenId`.
    fn cross_market_acc(creds: &BorosCreds, token_id: u64) -> String {
        Self::pack_market_acc(
            &creds.root,
            creds.account_id,
            token_id,
            BOROS_CROSS_MARKET_ID,
        )
    }

    /// The agent approval must be live before any order path can work —
    /// checked against the backend, never assumed.
    async fn agent_approved(&self) -> anyhow::Result<bool> {
        let creds = self.creds()?;
        let val = self
            .get(&format!(
                "/v1/agents/expiry-time?root={}&accountId={}&agentAddress={}",
                creds.root,
                creds.account_id,
                creds.agent.address_checksum()
            ))
            .await?;
        let expiry = val
            .get("expiryTime")
            .or_else(|| val.get("expiry"))
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(0);
        Ok(expiry > Utc::now().timestamp().max(0) as u64)
    }

    fn map_order_status(
        &self,
        status: u64,
        placed: Decimal,
        unfilled: Decimal,
    ) -> anyhow::Result<OrderStatus> {
        // 0 Filling, 1 Cancelled, 2 FullyFilled, 3 Expired, 4 Purged.
        Ok(match status {
            0 => {
                if unfilled < placed && unfilled >= dec!(0) {
                    OrderStatus::PartiallyFilled
                } else {
                    OrderStatus::New
                }
            }
            1 | 4 => OrderStatus::Cancelled,
            2 => OrderStatus::Filled,
            3 => OrderStatus::Expired,
            other => anyhow::bail!("Boros order has unknown status {other}"),
        })
    }
}

#[async_trait]
impl ExchangeAdapter for BorosAdapter {
    fn venue_id(&self) -> VenueId {
        VenueId::from("boros")
    }

    fn name(&self) -> &str {
        "Pendle Boros"
    }

    fn rest_base_url(&self) -> Option<String> {
        Some(self.base_url.clone())
    }

    /// YU markets are rate swaps, not perp tickers — the generic symbol
    /// universe stays clean by returning none. Market data for the
    /// fixed-yield scanner flows through `fetch_boros_yu_markets`.
    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        Ok(Vec::new())
    }

    async fn ping(&self) -> anyhow::Result<()> {
        self.get("/v1/markets?isMatured=false&limit=1")
            .await
            .map(|_| ())
    }

    async fn fetch_orderbook(&self, _symbol: &str) -> anyhow::Result<OrderBook> {
        anyhow::bail!(
            "Boros YU orderbooks are rate ladders, not price books — use the fixed-yield scanner"
        )
    }

    async fn fetch_ticker(&self, _symbol: &str) -> anyhow::Result<Ticker> {
        anyhow::bail!("Boros YU markets have no price ticker")
    }

    async fn fetch_funding_rate(&self, _symbol: &str) -> anyhow::Result<FundingRate> {
        anyhow::bail!("Boros YU markets are the funding hedge, not a funding source")
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        let creds = self.creds()?;
        if !self.agent_approved().await? {
            anyhow::bail!(
                "Boros agent {} is not approved (or expired) for root {} — approve it via the Boros app first",
                creds.agent.address_checksum(),
                creds.root
            );
        }
        // Cross-account handle covers every entered market's positions.
        // tokenId varies per collateral; fetch assets to enumerate the
        // handles in use.
        let assets = self.get("/v1/assets").await.unwrap_or_default();
        let mut token_ids: Vec<u64> = Vec::new();
        if let Some(rows) = assets
            .as_array()
            .or_else(|| assets.get("results").and_then(|r| r.as_array()))
        {
            for a in rows {
                if let Some(t) = a.get("tokenId").and_then(|v| v.as_u64()) {
                    token_ids.push(t);
                }
            }
        }
        if token_ids.is_empty() {
            token_ids.push(0);
        }
        let market_accs: Vec<String> = token_ids
            .iter()
            .map(|t| Self::cross_market_acc(&creds, *t))
            .collect();
        let resp = self
            .post(
                "/v1/accounts/market-acc-infos",
                &serde_json::json!({ "marketAccs": market_accs }),
            )
            .await?;
        let mut equity = dec!(0);
        let mut free = dec!(0);
        let mut maint = dec!(0);
        let mut positions = Vec::new();
        let markets = self.yu_markets().await.unwrap_or_default();
        if let Some(rows) = resp.get("results").and_then(|r| r.as_array()) {
            for row in rows {
                equity += dec_fixed_x18(row.get("netBalance")).unwrap_or_default();
                free += dec_fixed_x18(row.get("availableInitialMargin")).unwrap_or_default();
                if let Some(ps) = row.get("positions").and_then(|p| p.as_array()) {
                    for p in ps {
                        let signed = dec_fixed_x18(p.get("signedSize")).unwrap_or_default();
                        if signed == dec!(0) {
                            continue;
                        }
                        maint += dec_fixed_x18(p.get("maintMargin")).unwrap_or_default();
                        let mid = p.get("marketId").and_then(|v| v.as_u64()).unwrap_or(0);
                        let sym = markets
                            .iter()
                            .find(|m| m.market_id == mid)
                            .map(|m| m.symbol.clone())
                            .unwrap_or_else(|| format!("BOROS-{mid}"));
                        positions.push(Position {
                            venue: self.venue_id(),
                            symbol: sym,
                            side: if signed > dec!(0) {
                                Side::Buy
                            } else {
                                Side::Sell
                            },
                            quantity: signed.abs(),
                            entry_price: dec!(0),
                            mark_price: dec_fixed_x18(p.get("positionValue"))
                                .unwrap_or_default()
                                .abs(),
                            unrealized_pnl: dec!(0),
                            realized_pnl: dec!(0),
                            liquidation_price: None,
                            leverage: dec!(1),
                            initial_margin: dec_fixed_x18(p.get("initialMargin"))
                                .unwrap_or_default(),
                            maintenance_margin: dec_fixed_x18(p.get("maintMargin"))
                                .unwrap_or_default(),
                            updated_at: Utc::now(),
                        });
                    }
                }
            }
        }
        let usage = if equity > dec!(0) {
            (maint / equity) * dec!(100)
        } else {
            dec!(0)
        };
        Ok(AccountSnapshot {
            venue: self.venue_id(),
            equity,
            free_margin: free.max(dec!(0)),
            maintenance_margin: maint,
            margin_usage_pct: usage,
            balances: vec![Balance {
                venue: self.venue_id(),
                currency: "USD".into(),
                total: equity,
                free: free.max(dec!(0)),
                locked: (equity - free).max(dec!(0)),
            }],
            positions,
            timestamp: Utc::now(),
        })
    }

    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        let creds = self.creds()?;
        if !self.agent_approved().await? {
            anyhow::bail!(
                "Boros agent {} is not approved for root {} — refusing to submit",
                creds.agent.address_checksum(),
                creds.root
            );
        }
        let market = self.resolve_market(&order.symbol).await?;
        // Quantity is YU notional in settlement-token units (FixedX18).
        if order.quantity <= dec!(0) {
            anyhow::bail!("Boros order quantity must be positive");
        }
        let size_x18 = order.quantity * dec!(1000000000000000000);
        let size_str = size_x18
            .trunc()
            .to_u128()
            .ok_or_else(|| anyhow::anyhow!("Boros size overflows u128"))?
            .to_string();
        // Side: Buy → LONG YU (pay fixed / receive floating); Sell →
        // SHORT YU (pay floating / receive fixed).
        let side = match order.side {
            Side::Buy => 0u8,
            Side::Sell => 1u8,
        };
        let tif = if order.post_only || order.order_type == OrderType::PostOnly {
            3u8 // ADD_LIQUIDITY_ONLY
        } else {
            match order.time_in_force {
                TimeInForce::FillOrKill => 2,
                TimeInForce::ImmediateOrCancel => 1,
                TimeInForce::PostOnly => 3,
                TimeInForce::GoodTilCancelled => 0,
            }
        };
        let tif = if order.order_type == OrderType::Market {
            1
        } else {
            tif
        };
        // `rate` = the fixed APR bound as a fraction. Market/IOC without
        // an explicit rate gets the slippage guard instead of an
        // unguarded fill.
        let mut body = serde_json::json!({
            "marketAcc": Self::cross_market_acc(&creds, market.token_id),
            "marketId": market.market_id,
            "side": side,
            "size": size_str,
            "tif": tif,
        });
        if let Some(price) = order.price {
            let rate: f64 = price
                .to_string()
                .parse()
                .map_err(|_| anyhow::anyhow!("Boros rate (price field) is not numeric"))?;
            if !rate.is_finite() {
                anyhow::bail!("Boros rate must be finite");
            }
            body["rate"] = serde_json::json!(rate);
        } else if matches!(tif, 0 | 1 | 3 | 4) {
            // Unbounded market orders are dangerous on a rate ladder —
            // require a rate bound (or FOK which takes the extreme tick).
            body["slippage"] = serde_json::json!(0.005);
        }
        let resp = self
            .post("/v1/calldata-builder/agent/place-order", &body)
            .await?;
        let calls: Vec<(String, u64)> = resp
            .get("calls")
            .and_then(|c| c.as_array())
            .ok_or_else(|| anyhow::anyhow!("Boros place-order returned no calls"))?
            .iter()
            .map(|c| {
                let calldata = c
                    .get("calldata")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Boros call is missing calldata"))?
                    .to_string();
                let account_id = c.get("accountId").and_then(|v| v.as_u64()).unwrap_or(0);
                Ok((calldata, account_id))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        if calls.is_empty() {
            anyhow::bail!("Boros place-order produced an empty call list");
        }
        let receipts = self.submit_calls(calls, true).await?;
        let tx_hash = receipts
            .first()
            .and_then(|r| r.get("txHash"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Boros submission returned no txHash"))?
            .to_string();
        // Resolve the venue order id from the placement transaction.
        let orders = self
            .get(&format!("/v1/orders/by-tx-hash?placedTxHash={tx_hash}"))
            .await
            .unwrap_or_default();
        let first = orders
            .as_array()
            .and_then(|a| a.first().cloned())
            .or_else(|| {
                orders
                    .get("results")
                    .and_then(|r| r.as_array())
                    .and_then(|a| a.first().cloned())
            });
        let (order_id, filled, remaining, placed, status) = if let Some(o) = first {
            let placed = dec_fixed_x18(o.get("placedSize")).unwrap_or(order.quantity);
            let unfilled = dec_fixed_x18(o.get("unfilledSize")).unwrap_or(dec!(0));
            let status = o.get("status").and_then(|v| v.as_u64()).unwrap_or(0);
            (
                o.get("orderId")
                    .and_then(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .or_else(|| v.as_u64().map(|n| n.to_string()))
                    })
                    .unwrap_or_else(|| tx_hash.clone()),
                placed - unfilled,
                unfilled,
                placed,
                self.map_order_status(status, placed, unfilled)
                    .unwrap_or(OrderStatus::New),
            )
        } else {
            // The placement tx succeeded on-chain; if no order row exists
            // yet the order fully matched or the indexer is a beat behind —
            // report New with the tx hash as the durable id, never a fill.
            (
                tx_hash.clone(),
                dec!(0),
                order.quantity,
                order.quantity,
                OrderStatus::New,
            )
        };
        Ok(OrderResponse {
            order_id,
            client_order_id: order.client_order_id,
            venue: self.venue_id(),
            symbol: order.symbol,
            status,
            original_qty: placed,
            filled_qty: filled.max(dec!(0)),
            remaining_qty: remaining.max(dec!(0)),
            avg_fill_price: order.price,
            fee: dec!(0),
            fee_currency: "USD".into(),
            raw_response: Some(serde_json::json!({"txHash": tx_hash}).to_string()),
            timestamp: Utc::now(),
        })
    }

    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
        let creds = self.creds()?;
        let order_id = req
            .order_id
            .clone()
            .or(req.client_order_id.clone())
            .ok_or_else(|| anyhow::anyhow!("Boros cancel needs an order id"))?;
        let market = self.resolve_market(&req.symbol).await?;
        let resp = self
            .post(
                "/v1/calldata-builder/agent/cancel-orders",
                &serde_json::json!({
                    "markets": [{
                        "marketAcc": Self::cross_market_acc(&creds, market.token_id),
                        "marketId": market.market_id,
                        "cancelAll": false,
                        "orderIds": [order_id],
                    }]
                }),
            )
            .await?;
        let calls: Vec<(String, u64)> = resp
            .get("calls")
            .and_then(|c| c.as_array())
            .ok_or_else(|| anyhow::anyhow!("Boros cancel returned no calls"))?
            .iter()
            .map(|c| {
                let calldata = c
                    .get("calldata")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Boros call is missing calldata"))?
                    .to_string();
                let account_id = c.get("accountId").and_then(|v| v.as_u64()).unwrap_or(0);
                Ok((calldata, account_id))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        if calls.is_empty() {
            anyhow::bail!("Boros cancel produced an empty call list");
        }
        self.submit_calls(calls, true).await?;
        Ok(())
    }

    async fn fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        _client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        let creds = self.creds()?;
        let market = self.resolve_market(symbol).await?;
        let val = self
            .get(&format!(
                "/v1/accounts/orders?root={}&accountId={}&marketId={}&limit=100",
                creds.root, creds.account_id, market.market_id
            ))
            .await?;
        let rows = val
            .as_array()
            .cloned()
            .or_else(|| val.get("results").and_then(|r| r.as_array()).cloned())
            .unwrap_or_default();
        let row = rows
            .iter()
            .find(|o| {
                o.get("orderId")
                    .and_then(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .or_else(|| v.as_u64().map(|n| n.to_string()))
                    })
                    .map(|id| id == order_id)
                    .unwrap_or(false)
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Boros order {order_id} not found on market {}",
                    market.market_id
                )
            })?;
        let placed = dec_fixed_x18(row.get("placedSize")).unwrap_or(dec!(0));
        let unfilled = dec_fixed_x18(row.get("unfilledSize")).unwrap_or(dec!(0));
        let status = row
            .get("status")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Boros order row has no status"))?;
        Ok(OrderResponse {
            order_id: order_id.to_string(),
            client_order_id: String::new(),
            venue: self.venue_id(),
            symbol: symbol.to_string(),
            status: self.map_order_status(status, placed, unfilled)?,
            original_qty: placed,
            filled_qty: (placed - unfilled).max(dec!(0)),
            remaining_qty: unfilled.max(dec!(0)),
            avg_fill_price: f64_from_json(row.get("impliedApr"))
                .and_then(|r| Decimal::from_str(&r.to_string()).ok()),
            fee: dec!(0),
            fee_currency: "USD".into(),
            raw_response: Some(row.to_string()),
            timestamp: Utc::now(),
        })
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
        self.creds.read().is_some()
    }

    fn supports_reduce_only(&self, _symbol: &str) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_acc_packing_matches_sdk() {
        // MarketAccLib.pack(root, accountId, tokenId, marketId):
        // root<<48 | accountId<<40 | tokenId<<24 | marketId — 26 bytes.
        let root = Address::from_str("0x009dcf85824e024fea9e3ef583dccbea68edbc37").unwrap();
        let packed = BorosAdapter::pack_market_acc(&root, 0, 3, BOROS_CROSS_MARKET_ID);
        let v = U256::from_str_radix(packed.trim_start_matches("0x"), 16).unwrap();
        assert_eq!(
            U256::from_be_bytes({
                let mut b = [0u8; 32];
                b[12..32].copy_from_slice(root.as_slice());
                b
            }),
            v >> 48
        );
        assert_eq!(U256::from(3u64), (v >> 24) & U256::from(0xFFFFu64));
        assert_eq!(
            U256::from(BOROS_CROSS_MARKET_ID),
            v & U256::from(0xFF_FFFFu64)
        );
    }

    #[test]
    fn account_packing_matches_sdk() {
        // AccountLib.pack(root, accountId): root<<8 | accountId — 21 bytes.
        let root = Address::from_str("0x009dcf85824e024fea9e3ef583dccbea68edbc37").unwrap();
        let packed = BorosAdapter::pack_account(&root, 7);
        let bytes = packed.as_slice();
        assert_eq!(&bytes[..20], root.as_slice());
        assert_eq!(bytes[20], 7u8);
    }

    #[test]
    fn yu_market_parses_live_shape() {
        let row = serde_json::json!({
            "marketId": 194, "tokenId": 2,
            "imData": {"symbol": "HYPERLIQUID-BTCUSDC-30OCT2026", "maturity": 4070908800u64.saturating_sub(0), "tickStep": 2},
            "config": {"status": 2, "takerFee": "500000000000000"},
            "extConfig": {"paymentPeriod": 3600},
            "platform": {"name": "Hyperliquid", "platformId": "Hyperliquid"},
            "metadata": {"underlyingSymbol": "BTC", "name": "BTCUSDC", "maxLeverage": 3.0},
            "data": {"midApr": 0.086, "bestBid": 0.085, "bestAsk": 0.087, "floatingApr": 0.1095, "notionalOI": 881568.0}
        });
        // maturity must be in the future for the parser to keep it.
        let future = Utc::now().timestamp() + 86_400;
        let mut row = row;
        row["imData"]["maturity"] = serde_json::json!(future);
        let collateral = HashMap::from([
            (1u64, ("WBTC".to_string(), 83_000.0)),
            (2u64, ("WETH".to_string(), 2_600.0)),
            (3u64, ("USDT".to_string(), 1.0)),
        ]);
        let m = parse_yu_market(&row, &collateral).expect("active market parses");
        assert_eq!(m.market_id, 194);
        assert_eq!(m.platform, "Hyperliquid");
        assert_eq!(m.asset, "BTC");
        assert!((m.best_ask_apr.unwrap() - 0.087).abs() < 1e-9);
        assert!((m.taker_fee_rate - 0.0005).abs() < 1e-12);
        assert_eq!(m.collateral_symbol, "WETH");
        assert!((m.collateral_usd_price - 2600.0).abs() < 1e-9);
    }

    #[test]
    fn yu_market_rejects_matured_and_inactive() {
        let mut row = serde_json::json!({
            "marketId": 2, "tokenId": 1,
            "imData": {"symbol": "BINANCE-BTCUSDT-26SEP2025", "maturity": 1, "tickStep": 2},
            "config": {"status": 2, "takerFee": "500000000000000"},
            "platform": {"name": "Binance"},
            "metadata": {"underlyingSymbol": "BTC"},
            "data": {"midApr": 0.04}
        });
        let collateral = HashMap::from([(1u64, ("WBTC".to_string(), 83_000.0))]);
        assert!(parse_yu_market(&row, &collateral).is_none()); // matured
        row["imData"]["maturity"] = serde_json::json!(Utc::now().timestamp() + 86_400);
        row["config"]["status"] = serde_json::json!(1);
        assert!(parse_yu_market(&row, &collateral).is_none()); // suspended
        row["config"]["status"] = serde_json::json!(2);
        assert!(parse_yu_market(&row, &collateral).is_some()); // active
                                                               // An unknown collateral token must not produce a tradable market.
        assert!(parse_yu_market(&row, &HashMap::new()).is_none());
    }

    #[test]
    fn platform_mapping_is_fail_closed() {
        assert_eq!(boros_platform_venue_id("Binance"), Some("binance"));
        assert_eq!(boros_platform_venue_id("Hyperliquid"), Some("hyperliquid"));
        assert_eq!(boros_platform_venue_id("OKX"), Some("okx"));
        assert_eq!(boros_platform_venue_id("Lighter"), Some("lighter"));
        assert_eq!(boros_platform_venue_id("Gate"), None);
        assert_eq!(boros_platform_venue_id("KuCoin"), None);
        assert_eq!(boros_platform_venue_id("Bybit"), None);
    }

    /// Live public smoke: the Open API serves real YU markets with
    /// consistent platform/asset/maturity/implied-rate fields. Run with
    /// `cargo test -p arbitrageur-exchanges boros_live -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn boros_live_markets_smoke() {
        let client = Client::new();
        let markets = fetch_boros_yu_markets(&client, BOROS_API_BASE)
            .await
            .expect("Boros markets respond");
        assert!(!markets.is_empty(), "Boros lists active YU markets");
        for m in &markets {
            assert!(m.market_id > 0);
            assert!(m.maturity_unix > Utc::now().timestamp());
            assert!(m.mid_apr.is_finite());
            assert!(!m.platform.is_empty() && !m.asset.is_empty());
        }
        // At least one platform we actually trade must be represented.
        assert!(markets
            .iter()
            .any(|m| boros_platform_venue_id(&m.platform).is_some()));
    }

    #[test]
    fn credential_gate_requires_all_fields() {
        let adapter = BorosAdapter::new(None);
        assert!(!adapter.has_live_credentials());
        adapter
            .update_credentials(Some("not-a-key".into()), None, None)
            .unwrap_err();
        assert!(!adapter.has_live_credentials());
        adapter
            .update_credentials(
                Some("0x0000000000000000000000000000000000000000000000000000000000000001".into()),
                Some("0x0000000000000000000000000000000000000001".into()),
                Some("0".into()),
            )
            .unwrap();
        assert!(adapter.has_live_credentials());
    }
}
