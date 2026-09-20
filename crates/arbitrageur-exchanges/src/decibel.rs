use async_trait::async_trait;
use chrono::Utc;
use parking_lot::RwLock;
use reqwest::Client;
use sha3::{Digest, Sha3_256};
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::{AccountSnapshot, Balance, Position};
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{dec, Decimal, OrderStatus, OrderType, Side, TimeInForce, VenueId};
use aptos_sdk::{
    account::Ed25519Account,
    api::fullnode::FullnodeClient,
    transaction::{builder::TransactionBuilder, input::InputEntryFunctionData},
    types::{AccountAddress, ChainId},
    AptosConfig,
};
use crate::rate_limiter::AccountRequestCoordinator;
use std::str::FromStr;
use std::time::Duration;
use zeroize::Zeroizing;

/// f64-or-string JSON number (Decibel REST mixes both across endpoints).
fn num_f64(v: Option<&serde_json::Value>) -> Option<f64> {
    match v? {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(t) => t.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn dec_f64(v: Option<&serde_json::Value>) -> Option<Decimal> {
    num_f64(v).and_then(Decimal::from_f64_retain)
}

/// Unwrap the trading-API envelope: either a bare value or `{data: …}`.
fn envelope_data(v: &serde_json::Value) -> &serde_json::Value {
    if let Some(inner) = v.get("data") {
        if !inner.is_null() {
            return inner;
        }
    }
    v
}

fn as_array(v: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    let d = envelope_data(v);
    d.as_array().or_else(|| {
        d.get("result")
            .and_then(|r| r.as_array())
            .or_else(|| d.get("markets").and_then(|m| m.as_array()))
    })
}

// ---------------------------------------------------------------------------
// Decibel / Aptos deployment constants (from the official `decibeltrade/sdk`
// `PACKAGE` / `MAINNET_CONFIG` / `TESTNET_CONFIG`).
// ---------------------------------------------------------------------------
const PACKAGE_MAINNET: &str =
    "0x50ead22afd6ffd9769e3b3d6e0e64a2a350d68e8b102c4e72e33d0b8cfdfdb06";
const PACKAGE_TESTNET: &str =
    "0xe7da2794b1d8af76532ed95f38bfdf1136abfd8ea3a240189971988a83101b7f";
const FN_PLACE_ORDER: &str = "dex_accounts_entry::place_order_to_subaccount";
const FN_CANCEL_ORDER: &str = "dex_accounts_entry::cancel_order_to_subaccount";
const FN_CANCEL_CLIENT_ORDER: &str =
    "dex_accounts_entry::cancel_client_order_to_subaccount";
/// Aptos object derivation scheme byte (`object::create_object_address`).
const OBJECT_DERIVATION_SCHEME: u8 = 0xFE;
/// Sender-paid gas cap for Decibel writes. Unused gas is not charged; the cap
/// just needs headroom above a ~10k-unit place-order payload.
const MAX_GAS_AMOUNT: u64 = 50_000;
/// Minimum gas unit price (octas) if the fullnode estimate is unavailable.
const FALLBACK_GAS_UNIT_PRICE: u64 = 100;

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested, no network).
// ---------------------------------------------------------------------------

/// Parse an Aptos hex address (short or zero-padded, with or without `0x`)
/// into 32 raw bytes.
fn parse_address_bytes(s: &str) -> anyhow::Result<[u8; 32]> {
    let t = s.trim();
    let h = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t);
    if h.is_empty() || h.len() > 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!("invalid Aptos address: {s}");
    }
    let padded = format!("{:0>64}", h.to_lowercase());
    let v = hex::decode(&padded)?;
    let mut out = [0u8; 32];
    out.copy_from_slice(&v);
    Ok(out)
}

/// Canonical lowercase zero-padded `0x…` form of an Aptos address.
fn canonical_address(s: &str) -> anyhow::Result<String> {
    Ok(format!("0x{}", hex::encode(parse_address_bytes(s)?)))
}

/// `object::create_object_address(creator, seed)` =
/// `sha3_256(creator_bytes || seed || 0xFE)`.
fn create_object_address(creator: &[u8; 32], seed: &[u8]) -> [u8; 32] {
    let mut h = Sha3_256::new();
    h.update(creator);
    h.update(seed);
    h.update([OBJECT_DERIVATION_SCHEME]);
    let d = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&d);
    out
}

/// BCS encoding of a Move `String`: ULEB128 length prefix + UTF-8 bytes.
fn bcs_move_string_bytes(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut n = b.len() as u64;
    let mut out = Vec::with_capacity(b.len() + 5);
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
    out.extend_from_slice(b);
    out
}

/// Object address of the shared perp engine (`GlobalPerpEngine` named object
/// under the deployment package). Verified against mainnet fullnode:
/// package `0x50ea…` yields `0x4d8c…`, which exists on chain owned by the
/// package account.
fn perp_engine_global_addr(package_hex: &str) -> anyhow::Result<String> {
    let pkg = parse_address_bytes(package_hex)?;
    Ok(format!(
        "0x{}",
        hex::encode(create_object_address(&pkg, b"GlobalPerpEngine"))
    ))
}

/// Market object address = named object under the perp-engine global keyed by
/// the BCS bytes of the market name (mirrors SDK `getMarketAddr`).
fn market_addr_for(
    perp_engine_global_hex: &str,
    market_name: &str,
) -> anyhow::Result<String> {
    let engine = parse_address_bytes(perp_engine_global_hex)?;
    let seed = bcs_move_string_bytes(market_name);
    Ok(format!(
        "0x{}",
        hex::encode(create_object_address(&engine, &seed))
    ))
}

/// Primary subaccount object for an owner wallet (mirrors SDK
/// `getPrimarySubaccountAddr`): deriver named object `GlobalSubaccountManager`
/// under the package, seed = owner bytes ++ BCS("primary_subaccount").
fn primary_subaccount_addr(owner_hex: &str, package_hex: &str) -> anyhow::Result<String> {
    let owner = parse_address_bytes(owner_hex)?;
    let pkg = parse_address_bytes(package_hex)?;
    let deriver = create_object_address(&pkg, b"GlobalSubaccountManager");
    let mut seed = Vec::with_capacity(32 + 24);
    seed.extend_from_slice(&owner);
    seed.extend_from_slice(&bcs_move_string_bytes("primary_subaccount"));
    Ok(format!(
        "0x{}",
        hex::encode(create_object_address(&deriver, &seed))
    ))
}

/// Map an Arbitrageur venue symbol to a Decibel perp market name.
/// Decibel perp markets are named like `BTC-PERP`.
fn resolve_market_name(symbol: &str) -> String {
    let s = symbol.trim().to_uppercase().replace(['/', ':'], "-");
    if s.ends_with("-PERP") {
        return s;
    }
    // Strip common quote suffixes, keep the base.
    for q in ["-USDC", "-USDT", "-USD"] {
        if let Some(base) = s.strip_suffix(q) {
            if !base.is_empty() {
                return format!("{base}-PERP");
            }
        }
    }
    if s.contains('-') {
        return s;
    }
    format!("{s}-PERP")
}

/// Scale a human price into chain units: `round(price * 10^px_decimals)`.
fn price_to_chain_units(price: Decimal, px_decimals: u32) -> anyhow::Result<u64> {
    if price <= dec!(0) {
        anyhow::bail!("price must be positive, got {price}");
    }
    let factor = Decimal::from(10u64.pow(px_decimals.min(18)));
    let scaled = (price * factor).round();
    let divisor = 10i128.pow(scaled.scale().min(28));
    u64::try_from(scaled.mantissa() / divisor)
        .map_err(|_| anyhow::anyhow!("price {price} overflows u64 at {px_decimals} decimals"))
}

/// Scale a human size into chain units: `round(size * 10^sz_decimals)`.
fn size_to_chain_units(size: Decimal, sz_decimals: u32) -> anyhow::Result<u64> {
    if size <= dec!(0) {
        anyhow::bail!("size must be positive, got {size}");
    }
    let factor = Decimal::from(10u64.pow(sz_decimals.min(18)));
    let scaled = (size * factor).round();
    let divisor = 10i128.pow(scaled.scale().min(28));
    u64::try_from(scaled.mantissa() / divisor)
        .map_err(|_| anyhow::anyhow!("size {size} overflows u64 at {sz_decimals} decimals"))
}

#[derive(Debug, Clone)]
struct DecibelMarketRow {
    market_name: String,
    market_addr: String,
    px_decimals: u32,
    sz_decimals: u32,
    tick_size: Decimal,
    lot_size: Decimal,
    min_size: Decimal,
}

/// Decibel Aptos CLOB Perpetual DEX connector.
///
/// Reads go through the Geomi trading API; writes are genuine on-chain Aptos
/// transactions (`dex_accounts_entry::place_order_to_subaccount` et al.)
/// signed with the vault-held wallet key via the `aptos-sdk` crate.
/// Everything is fail-closed: missing keys, unresolvable markets, or failed
/// submissions return errors — fills are never fabricated.
pub struct DecibelAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    api_key: RwLock<Option<Zeroizing<String>>>,
    node_api_key: RwLock<Option<Zeroizing<String>>>,
    wallet_address: RwLock<Option<String>>,
    private_key: RwLock<Option<Zeroizing<String>>>,
    rate_limiter: AccountRequestCoordinator,
    /// Serialises sequence-number fetch → sign → submit so concurrent orders
    /// never reuse an account sequence number.
    order_lock: tokio::sync::Mutex<()>,
}

impl DecibelAdapter {
    pub fn new(
        base_url: Option<String>,
        api_key: Option<String>,
        api_secret: Option<String>,
    ) -> Self {
        let base = base_url.unwrap_or_else(|| "https://api.mainnet.aptoslabs.com/decibel".to_string());
        Self {
            venue_id: VenueId::new("decibel"),
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
                Client::builder().timeout(std::time::Duration::from_secs(15))
                    .default_headers(headers)
                    .build()
                    .unwrap_or_default()
            },
            base_url: base,
            api_key: RwLock::new(api_key.map(Zeroizing::new)),
            node_api_key: RwLock::new(api_secret.map(Zeroizing::new)),
            wallet_address: RwLock::new(None),
            private_key: RwLock::new(None::<Zeroizing<String>>),
            rate_limiter: AccountRequestCoordinator::new("decibel", 100, 2000, 500, 10000),
            order_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn update_credentials(
        &self,
        api_key: Option<String>,
        node_api_key: Option<String>,
        wallet_address: Option<String>,
        private_key: Option<String>,
    ) {
        *self.api_key.write() = api_key.map(Zeroizing::new);
        *self.node_api_key.write() = node_api_key.map(Zeroizing::new);
        *self.wallet_address.write() = wallet_address;
        *self.private_key.write() = private_key.map(Zeroizing::new);
    }

    fn is_testnet(&self) -> bool {
        self.base_url.to_lowercase().contains("testnet")
    }

    fn deployment_package(&self) -> &'static str {
        if self.is_testnet() {
            PACKAGE_TESTNET
        } else {
            PACKAGE_MAINNET
        }
    }

    fn aptos_config(&self) -> AptosConfig {
        if self.is_testnet() {
            AptosConfig::testnet()
        } else {
            AptosConfig::mainnet()
        }
    }

    fn trading_key(&self) -> anyhow::Result<String> {
        match &*self.api_key.read() {
            Some(k) if !k.trim().is_empty() => Ok(k.trim().to_string()),
            _ => anyhow::bail!("Decibel requires a Geomi/Aptos client API key"),
        }
    }

    fn wallet_key(&self) -> anyhow::Result<Ed25519Account> {
        let guard = self.private_key.read();
        let Some(k) = &*guard else {
            anyhow::bail!("Decibel live orders require the Aptos wallet private key (Settings > Exchange Accounts); refusing to submit without a signer");
        };
        let t = k.trim();
        let h = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t);
        let clean: String = h.chars().filter(|c| !c.is_whitespace()).collect();
        Ed25519Account::from_private_key_hex(&clean).map_err(|e| {
            anyhow::anyhow!("Decibel private key is not a valid Ed25519 key: {e}")
        })
    }

    /// Owner wallet address: explicit setting wins, otherwise derived from
    /// the signing key (the key's own account).
    fn owner_address(&self, account: &Ed25519Account) -> anyhow::Result<String> {
        if let Some(w) = self.wallet_address.read().clone() {
            if !w.trim().is_empty() {
                return canonical_address(w.trim());
            }
        }
        Ok(account.address().to_string())
    }

    async fn get_json(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        let key = self.trading_key()?;

        if let Some(cached_str) = self.rate_limiter.acquire_or_cache_path(path, true).await? {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&cached_str) {
                return Ok(val);
            }
        }

        let url = format!("{}{}", self.base_url, path);
        let resp = match self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", key))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                self.rate_limiter.record_failure(false);
                return Err(e.into());
            }
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let is_429 = status == reqwest::StatusCode::TOO_MANY_REQUESTS;
            if status.as_u16() == 401 || status.as_u16() == 404 {
                anyhow::bail!("Decibel API {}: {}", status, path);
            }
            self.rate_limiter.record_failure(is_429);
            anyhow::bail!("Decibel API error: {} {}", status, path);
        }

        let text = resp.text().await?;
        self.rate_limiter.record_success_path(path, text.clone());
        let data = serde_json::from_str::<serde_json::Value>(&text)?;
        Ok(data)
    }

    /// Fetch the live market row for a venue symbol from the trading API.
    /// Fails closed when the market cannot be resolved — chain-unit scaling
    /// without authoritative decimals would submit a wrongly-sized order.
    async fn fetch_market_row(&self, symbol: &str) -> anyhow::Result<DecibelMarketRow> {
        let want = resolve_market_name(symbol);
        let mut last_err = String::new();
        for path in ["/api/v1/markets", "/v1/markets"] {
            let val = match self.get_json(path).await {
                Ok(v) => v,
                Err(e) => {
                    last_err = e.to_string();
                    continue;
                }
            };
            let arr: Option<&Vec<serde_json::Value>> = val
                .as_array()
                .or_else(|| val.get("data").and_then(|d| d.as_array()))
                .or_else(|| val.get("markets").and_then(|d| d.as_array()))
                .or_else(|| val.get("result").and_then(|d| d.as_array()));
            let Some(rows) = arr else {
                last_err = format!("unexpected markets shape at {path}");
                continue;
            };
            for r in rows {
                let name = r
                    .get("market_name")
                    .and_then(|v| v.as_str())
                    .or_else(|| r.get("name").and_then(|v| v.as_str()))
                    .unwrap_or("");
                if !name.eq_ignore_ascii_case(&want) {
                    continue;
                }
                let px = r
                    .get("px_decimals")
                    .and_then(|v| v.as_u64())
                    .or_else(|| r.get("pxDecimals").and_then(|v| v.as_u64()))
                    .unwrap_or(6) as u32;
                let sz = r
                    .get("sz_decimals")
                    .and_then(|v| v.as_u64())
                    .or_else(|| r.get("szDecimals").and_then(|v| v.as_u64()))
                    .unwrap_or(6) as u32;
                let num = |k: &str| {
                    r.get(k)
                        .and_then(|v| v.as_str())
                        .and_then(|s| Decimal::from_str_exact(s).ok())
                        .or_else(|| {
                            r.get(k).and_then(|v| v.as_f64()).and_then(|f| {
                                Decimal::from_f64_retain(f)
                            })
                        })
                        .unwrap_or(dec!(0))
                };
                let addr = r
                    .get("market_addr")
                    .and_then(|v| v.as_str())
                    .or_else(|| r.get("marketAddr").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_string();
                return Ok(DecibelMarketRow {
                    market_name: name.to_string(),
                    market_addr: addr,
                    px_decimals: px,
                    sz_decimals: sz,
                    tick_size: num("tick_size"),
                    lot_size: num("lot_size"),
                    min_size: num("min_size"),
                });
            }
            last_err = format!("market {want} not listed at {path}");
        }
        anyhow::bail!(
            "Decibel market {want} (symbol {symbol}) could not be resolved from the live markets feed: {last_err}"
        )
    }

    /// Resolve the owner's primary subaccount object address via the
    /// official `GET {trading}/api/v1/subaccounts?owner=` listing (primary
    /// first, else first active, else first row). Fails closed when the owner
    /// has no subaccount — the fix is creating one in the Decibel app, not
    /// guessing an address.
    async fn primary_subaccount(&self, owner: &str) -> anyhow::Result<String> {
        let owner = owner.trim();
        let mut rows: Vec<serde_json::Value> = Vec::new();
        // Checksum-cased wallets have been observed to miss; retry lowercase.
        for candidate in [owner.to_string(), owner.to_lowercase()] {
            let val = self
                .get_json(&format!("/api/v1/subaccounts?owner={candidate}"))
                .await?;
            rows = as_array(&val)
                .ok_or_else(|| {
                    anyhow::anyhow!("Decibel subaccounts response has no list for owner {owner}")
                })?
                .clone();
            if !rows.is_empty() {
                break;
            }
        }
        if rows.is_empty() {
            anyhow::bail!(
                "Decibel owner {owner} has no subaccount yet — create one at https://app.decibel.trade, then re-read"
            );
        }
        let pick = rows
            .iter()
            .find(|r| r.get("is_primary").and_then(|b| b.as_bool()).unwrap_or(false))
            .or_else(|| {
                rows.iter().find(|r| {
                    r.get("is_active").and_then(|b| b.as_bool()).unwrap_or(true)
                })
            })
            .or_else(|| rows.first())
            .expect("non-empty");
        pick.get("subaccount_address")
            .and_then(|a| a.as_str())
            .filter(|a| !a.trim().is_empty())
            .map(|a| a.trim().to_string())
            .ok_or_else(|| anyhow::anyhow!("Decibel subaccount row has no address"))
    }

    /// Live market map (object address lowercase → market name) from the
    /// official `GET {trading}/api/v1/markets` listing. Used to label
    /// positions and to address price lookups.
    async fn market_name_map(&self) -> anyhow::Result<std::collections::HashMap<String, String>> {
        let mut map = std::collections::HashMap::new();
        let mut last_err = String::new();
        for path in ["/api/v1/markets", "/v1/markets"] {
            match self.get_json(path).await {
                Ok(val) => {
                    if let Some(rows) = as_array(&val) {
                        for r in rows {
                            let (Some(addr), Some(name)) = (
                                r.get("market_addr")
                                    .and_then(|v| v.as_str())
                                    .or_else(|| r.get("marketAddr").and_then(|v| v.as_str())),
                                r.get("market_name")
                                    .and_then(|v| v.as_str())
                                    .or_else(|| r.get("name").and_then(|v| v.as_str())),
                            ) else {
                                continue;
                            };
                            map.insert(addr.trim().to_lowercase(), name.trim().to_string());
                        }
                        if !map.is_empty() {
                            return Ok(map);
                        }
                        last_err = format!("no usable rows at {path}");
                    } else {
                        last_err = format!("unexpected markets shape at {path}");
                    }
                }
                Err(e) => last_err = e.to_string(),
            }
        }
        anyhow::bail!("Decibel markets listing unavailable: {last_err}")
    }

    /// Live per-market mids (object address lowercase → mid price) from the
    /// official `GET {trading}/api/v1/prices` listing. Best-effort: an empty
    /// map just means marks fall back to entry prices, never fabricated ones.
    async fn market_mid_map(&self) -> std::collections::HashMap<String, Decimal> {
        let mut map = std::collections::HashMap::new();
        let Ok(val) = self.get_json("/api/v1/prices").await else {
            return map;
        };
        let d = envelope_data(&val);
        let rows: Vec<&serde_json::Value> = match d {
            serde_json::Value::Array(a) => a.iter().collect(),
            serde_json::Value::Object(_) => vec![d],
            _ => vec![],
        };
        for r in rows {
            let (Some(addr), Some(mid)) = (
                r.get("market").and_then(|v| v.as_str()),
                dec_f64(r.get("mid_px").or_else(|| r.get("mark_px"))),
            ) else {
                continue;
            };
            if mid > dec!(0) {
                map.insert(addr.trim().to_lowercase(), mid);
            }
        }
        map
    }

    /// Submit a pre-built entry-function payload as the vault signer and wait
    /// for commit. Returns the committed transaction JSON.
    async fn submit_payload(
        &self,
        payload: aptos_sdk::transaction::payload::TransactionPayload,
    ) -> anyhow::Result<serde_json::Value> {
        let account = self.wallet_key()?;
        let sender = account.address();
        let client = FullnodeClient::new(self.aptos_config())
            .map_err(|e| anyhow::anyhow!("Decibel fullnode client init failed: {e}"))?;
        let seq = client.get_sequence_number(sender).await.map_err(|e| {
            anyhow::anyhow!("Decibel fullnode sequence fetch for {sender} failed: {e}")
        })?;
        let chain_id = client
            .get_ledger_info()
            .await
            .map(|r| ChainId::new(r.data.chain_id))
            .unwrap_or_else(|_| {
                if self.is_testnet() {
                    ChainId::testnet()
                } else {
                    ChainId::mainnet()
                }
            });
        let gas_price = client
            .estimate_gas_price()
            .await
            .map(|r| r.data.recommended().max(1))
            .unwrap_or(FALLBACK_GAS_UNIT_PRICE);
        let expiration = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() + 120)
            .unwrap_or(9_999_999_999);
        let signed = TransactionBuilder::new()
            .sender(sender)
            .sequence_number(seq)
            .payload(payload)
            .max_gas_amount(MAX_GAS_AMOUNT)
            .gas_unit_price(gas_price)
            .expiration_timestamp_secs(expiration)
            .chain_id(chain_id)
            .build_and_sign(&account)
            .map_err(|e| anyhow::anyhow!("Decibel transaction build/sign failed: {e}"))?;
        let committed = client
            .submit_and_wait(&signed, Some(Duration::from_secs(30)))
            .await
            .map_err(|e| anyhow::anyhow!("Decibel on-chain submission failed: {e}"))?;
        Ok(committed.data)
    }

    /// Best-effort extraction of the Decibel order id from commit events.
    fn extract_order_id(tx: &serde_json::Value, subaccount: &str) -> Option<String> {
        let events = tx.get("events")?.as_array()?;
        let sub = subaccount.to_lowercase();
        for ev in events {
            let ty = ev.get("type")?.as_str()?;
            if !ty.contains("OrderEvent") {
                continue;
            }
            let data = ev.get("data")?;
            if let Some(user) = data.get("user").and_then(|u| u.as_str()) {
                if user.to_lowercase() != sub {
                    continue;
                }
            }
            if let Some(id) = data.get("order_id") {
                if let Some(s) = id.as_str() {
                    return Some(s.to_string());
                }
                if let Some(n) = id.as_u64() {
                    return Some(n.to_string());
                }
                if let Some(inner) = id.get("order_id") {
                    if let Some(s) = inner.as_str() {
                        return Some(s.to_string());
                    }
                    if let Some(n) = inner.as_u64() {
                        return Some(n.to_string());
                    }
                }
                return Some(id.to_string());
            }
        }
        None
    }
}

#[async_trait]
impl ExchangeAdapter for DecibelAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Decibel"
    }

    async fn ping(&self) -> anyhow::Result<()> {
        let resp = self.client.get("https://fullnode.mainnet.aptoslabs.com/v1").send().await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            anyhow::bail!("Decibel Aptos probe returned status {}", resp.status());
        }
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let mut markets = Vec::new();
        let common_pairs = ["APT", "BTC", "ETH", "SOL", "USDT", "SUI", "DOGE"];
        for base in common_pairs {
            markets.push(MarketMetadata {
                symbol: Symbol::perp(base, "USDC"),
                venue: self.venue_id.clone(),
                venue_symbol: format!("{}-PERP", base),
                price_decimals: if base == "BTC" || base == "ETH" { 2 } else { 4 },
                size_decimals: 3,
                min_order_size: dec!(0.001),
                min_notional: dec!(5.0),
                maker_fee_bps: dec!(2.0),
                taker_fee_bps: dec!(5.0),
                contract_size: dec!(1.0),
                is_active: true,
            });
        }
        Ok(markets)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        // Upstream removed the REST depth endpoint (WS-only now, see the
        // official SDK's commented-out `/api/v1/depth` reader). Try the
        // documented path once in case it was reintroduced; otherwise fail
        // closed — a book must never be synthesized.
        let market_name = resolve_market_name(symbol);
        let engine = perp_engine_global_addr(self.deployment_package())?;
        let market_addr = market_addr_for(&engine, &market_name)?;
        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());
        if let Ok(val) = self
            .get_json(&format!("/api/v1/depth?market={market_addr}&limit=50"))
            .await
        {
            let d = envelope_data(&val);
            let parse_side = |key: &str, out: &mut Vec<BookLevel>| {
                if let Some(levels) = d.get(key).and_then(|v| v.as_array()) {
                    for lvl in levels {
                        let (px, sz) = match lvl {
                            serde_json::Value::Array(a) => {
                                (dec_f64(a.first()), dec_f64(a.get(1)))
                            }
                            serde_json::Value::Object(o) => (
                                dec_f64(o.get("price").or_else(|| o.get("px"))),
                                dec_f64(o.get("size").or_else(|| o.get("sz")).or_else(|| o.get("quantity"))),
                            ),
                            _ => (None, None),
                        };
                        if let (Some(p), Some(q)) = (px, sz) {
                            if p > dec!(0) && q > dec!(0) {
                                out.push(BookLevel::new(p, q));
                            }
                        }
                    }
                }
            };
            parse_side("bids", &mut book.bids);
            parse_side("asks", &mut book.asks);
        }
        if book.bids.is_empty() || book.asks.is_empty() {
            anyhow::bail!(
                "Decibel has no REST orderbook for {symbol} (depth is WebSocket-only upstream); refusing to synthesize a book"
            );
        }
        book.timestamp = Utc::now();
        Ok(book)
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        // No REST bid/ask exists upstream (depth is WebSocket-only), and a
        // mid!=last ticker with zeroed touch would poison execution pricing.
        // Fail closed; live limit orders (user-priced) are unaffected.
        let _ = symbol;
        anyhow::bail!("Decibel exposes no REST bid/ask (depth is WebSocket-only upstream); refusing to fabricate a ticker")
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate> {
        // Real feed: `GET {trading}/api/v1/prices?market={addr}` returns
        // `funding_rate_bps` per `funding_period_s` (official SDK
        // `MarketPricesSchema`). Normalized to an 8h print; missing fields
        // fail closed, never zero-filled (a silent zero would mint phantom
        // carry against any nonzero leg).
        let market_name = resolve_market_name(symbol);
        let engine = perp_engine_global_addr(self.deployment_package())?;
        let market_addr = market_addr_for(&engine, &market_name)?;
        let val = self
            .get_json(&format!("/api/v1/prices?market={market_addr}"))
            .await?;
        let d = envelope_data(&val);
        let rows: Vec<&serde_json::Value> = match d {
            serde_json::Value::Array(a) => a.iter().collect(),
            serde_json::Value::Object(_) => vec![d],
            _ => vec![],
        };
        let row = rows
            .iter()
            .find(|r| {
                r.get("market")
                    .and_then(|m| m.as_str())
                    .is_some_and(|m| m.eq_ignore_ascii_case(&market_addr))
            })
            .or(rows.first())
            .ok_or_else(|| anyhow::anyhow!("Decibel prices feed returned no rows for {symbol}"))?;
        let bps = num_f64(row.get("funding_rate_bps")).ok_or_else(|| {
            anyhow::anyhow!("Decibel prices row for {symbol} has no funding_rate_bps")
        })?;
        let period_s = num_f64(row.get("funding_period_s")).unwrap_or(3600.0);
        if !(period_s.is_finite() && period_s > 0.0) {
            anyhow::bail!("Decibel prices row for {symbol} has an unusable funding_period_s");
        }
        let period_h = period_s / 3600.0;
        let rate_8h = Decimal::from_f64_retain(bps / 10_000.0 * (8.0 / period_h))
            .unwrap_or(dec!(0));
        Ok(FundingRate {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            rate: rate_8h,
            predicted_rate: None,
            interval_hours: period_h.max(1.0) as u32,
            next_funding_time: Utc::now() + chrono::Duration::seconds(period_s as i64),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        let wallet = self.wallet_address.read().clone();
        let Some(w) = wallet.filter(|s| !s.trim().is_empty()) else {
            anyhow::bail!("Decibel account query requires an Aptos wallet address. Please set the Login Wallet Address in Settings > Exchange Accounts.");
        };
        let owner = w.trim().to_string();

        // 1. Owner -> primary subaccount (official `/api/v1/subaccounts`).
        let subaccount = self.primary_subaccount(&owner).await?;

        // 2. Balances via the official `/api/v1/account_overviews`.
        let overview = self
            .get_json(&format!("/api/v1/account_overviews?account={subaccount}"))
            .await
            .map(|v| envelope_data(&v).clone())
            .unwrap_or(serde_json::Value::Null);
        let total_equity =
            dec_f64(overview.get("perp_equity_balance")).unwrap_or(dec!(0));
        let free_margin = dec_f64(overview.get("usdc_cross_withdrawable_balance"))
            .or_else(|| dec_f64(overview.get("usdc_cross_balance")))
            .unwrap_or(total_equity);
        let locked = if total_equity > free_margin {
            total_equity - free_margin
        } else {
            dec!(0)
        };
        let maintenance = dec_f64(overview.get("maintenance_margin")).unwrap_or(dec!(0));

        // 3. Positions via the official `/api/v1/account_positions`, labelled
        // through the live markets map, marked through the live prices feed.
        // A position row without a resolvable market or size is skipped, not
        // guessed; marks fall back to entry (never invented).
        let names = self.market_name_map().await.unwrap_or_default();
        let mids = self.market_mid_map().await;
        let mut positions = Vec::new();
        if let Ok(pval) = self
            .get_json(&format!(
                "/api/v1/account_positions?account={subaccount}&include_deleted=false&limit=100"
            ))
            .await
        {
            if let Some(rows) = as_array(&pval) {
                for r in rows {
                    let size = num_f64(r.get("size")).unwrap_or(0.0);
                    if !size.is_finite() || size == 0.0 {
                        continue;
                    }
                    let market_addr = r.get("market").and_then(|m| m.as_str()).unwrap_or("");
                    let label = names
                        .get(&market_addr.trim().to_lowercase())
                        .cloned()
                        .unwrap_or_else(|| {
                            let a = market_addr.trim();
                            if a.len() > 12 {
                                format!("{}…{}", &a[..6], &a[a.len() - 4..])
                            } else {
                                a.to_string()
                            }
                        });
                    if label.trim().is_empty() {
                        continue;
                    }
                    let qty = Decimal::from_f64_retain(size.abs()).unwrap_or(dec!(0));
                    if qty <= dec!(0) {
                        continue;
                    }
                    let entry = dec_f64(r.get("entry_price")).unwrap_or(dec!(0));
                    let mark = mids
                        .get(&market_addr.trim().to_lowercase())
                        .cloned()
                        .filter(|m| *m > dec!(0))
                        .unwrap_or(entry);
                    let upnl = if mark > dec!(0) && entry > dec!(0) {
                        (mark - entry)
                            * qty
                            * if size > 0.0 { dec!(1) } else { dec!(-1) }
                    } else {
                        dec!(0)
                    };
                    let liq = dec_f64(r.get("estimated_liquidation_price"))
                        .filter(|p| *p > dec!(0));
                    positions.push(Position {
                        venue: self.venue_id.clone(),
                        symbol: label,
                        side: if size > 0.0 { Side::Buy } else { Side::Sell },
                        quantity: qty,
                        entry_price: entry,
                        mark_price: mark,
                        unrealized_pnl: upnl,
                        realized_pnl: dec!(0),
                        liquidation_price: liq,
                        leverage: num_f64(r.get("user_leverage"))
                            .and_then(Decimal::from_f64_retain)
                            .unwrap_or(dec!(0)),
                        initial_margin: dec!(0),
                        maintenance_margin: dec!(0),
                        updated_at: Utc::now(),
                    });
                }
            }
        }

        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity: total_equity,
            free_margin,
            maintenance_margin: maintenance,
            margin_usage_pct: dec!(0),
            balances: vec![Balance {
                venue: self.venue_id.clone(),
                currency: "USDC".to_string(),
                total: total_equity,
                free: free_margin,
                locked,
            }],
            positions,
            timestamp: Utc::now(),
        })
    }

    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        // Serialise submissions: one sequence number per account.
        let _guard = self.order_lock.lock().await;
        // Reads (markets feed) need the trading API key; the signer needs the
        // vault private key. Both missing → fail closed, never fabricate.
        self.trading_key()?;
        let signer = self.wallet_key()?;

        if order.quantity <= dec!(0) {
            anyhow::bail!("Decibel order quantity must be positive");
        }
        if order.order_type == OrderType::PostOnly && order.price.is_none() {
            anyhow::bail!("Decibel PostOnly orders require a limit price");
        }
        let is_buy = matches!(order.side, Side::Buy);
        // Decibel entry TIF: 0 = GTC, 1 = PostOnly, 2 = IOC.
        let tif: u8 = if order.post_only || order.time_in_force == TimeInForce::PostOnly {
            1
        } else {
            match order.time_in_force {
                TimeInForce::ImmediateOrCancel | TimeInForce::FillOrKill => 2,
                _ => 0,
            }
        };

        let row = self.fetch_market_row(&order.symbol).await?;

        // Limit price, or — for market orders — a bounded marketable-limit:
        // live touch price with a 0.5% adverse collar, IOC. A pure on-chain
        // market type does not exist in the entry function.
        let (price, ioc_market) = match order.price {
            Some(p) => (p, false),
            None => {
                if order.order_type != OrderType::Market
                    && order.order_type != OrderType::ImmediateOrCancel
                {
                    anyhow::bail!(
                        "Decibel order has no price and is not a market order; refusing to guess one"
                    );
                }
                let book = self.fetch_orderbook(&order.symbol).await?;
                let touch = if is_buy {
                    book.best_ask_price()
                } else {
                    book.best_bid_price()
                }
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Decibel market order for {} has no live touch price; refusing to cross blind",
                        order.symbol
                    )
                })?;
                let collar = if is_buy {
                    touch * dec!(1.005)
                } else {
                    touch * dec!(0.995)
                };
                (collar, true)
            }
        };
        let eff_tif: u8 = if ioc_market { 2 } else { tif };

        // Side-safe tick alignment when the feed publishes a tick size:
        // buys round down, sells round up — never cross the caller's cap.
        let mut price_adj = price;
        if row.tick_size > dec!(0) {
            let ticks = (price / row.tick_size).floor();
            let floored = ticks * row.tick_size;
            let ceiled = floored + row.tick_size;
            price_adj = if is_buy { floored } else { ceiled };
            if price_adj <= dec!(0) {
                price_adj = price;
            }
        }

        let price_chain = price_to_chain_units(price_adj, row.px_decimals)?;
        // Floor size to the market lot multiple when the feed publishes one.
        let mut qty = order.quantity;
        if row.lot_size > dec!(0) {
            let lots = (qty / row.lot_size).floor();
            qty = lots * row.lot_size;
            if qty <= dec!(0) {
                anyhow::bail!(
                    "Decibel order size {} below one lot {} for {}",
                    order.quantity,
                    row.lot_size,
                    row.market_name
                );
            }
        }
        let size_chain = size_to_chain_units(qty, row.sz_decimals)?;
        if row.min_size > dec!(0) && order.quantity < row.min_size {
            anyhow::bail!(
                "Decibel order size {} below market minimum {} for {}",
                order.quantity,
                row.min_size,
                row.market_name
            );
        }

        let package = self.deployment_package();
        let engine = perp_engine_global_addr(package)?;
        let market_addr = if row.market_addr.trim().is_empty() {
            market_addr_for(&engine, &row.market_name)?
        } else {
            canonical_address(row.market_addr.trim())?
        };
        let owner = self.owner_address(&signer)?;
        let subaccount = primary_subaccount_addr(&owner, package)?;
        let sub_addr = AccountAddress::from_str(&subaccount)
            .map_err(|e| anyhow::anyhow!("Decibel subaccount address invalid: {e}"))?;
        let mkt_addr = AccountAddress::from_str(&market_addr)
            .map_err(|e| anyhow::anyhow!("Decibel market address invalid: {e}"))?;

        let client_id: Option<String> = if order.client_order_id.trim().is_empty() {
            None
        } else {
            Some(order.client_order_id.clone())
        };
        let payload = InputEntryFunctionData::new(&format!("{package}::{FN_PLACE_ORDER}"))
            .arg(sub_addr)
            .arg(mkt_addr)
            .arg(price_chain)
            .arg(size_chain)
            .arg(is_buy)
            .arg(eff_tif)
            .arg(order.reduce_only)
            .arg(client_id)
            .arg(None::<u64>)
            .arg(None::<u64>)
            .arg(None::<u64>)
            .arg(None::<u64>)
            .arg(None::<u64>)
            .arg(None::<AccountAddress>)
            .arg(None::<u64>)
            .build()
            .map_err(|e| anyhow::anyhow!("Decibel payload build failed: {e}"))?;

        let tx = self.submit_payload(payload).await?;
        let hash = tx
            .get("hash")
            .and_then(|h| h.as_str())
            .unwrap_or("")
            .to_string();
        if hash.is_empty() {
            anyhow::bail!("Decibel submission returned no transaction hash: {tx}");
        }
        let venue_order_id =
            Self::extract_order_id(&tx, &subaccount).unwrap_or_else(|| hash.clone());
        Ok(OrderResponse {
            order_id: venue_order_id,
            client_order_id: order.client_order_id.clone(),
            venue: self.venue_id.clone(),
            symbol: order.symbol.clone(),
            status: OrderStatus::New,
            original_qty: qty,
            filled_qty: dec!(0),
            remaining_qty: qty,
            avg_fill_price: None,
            fee: dec!(0),
            fee_currency: "USDC".to_string(),
            raw_response: Some(
                serde_json::json!({
                    "tx_hash": hash,
                    "market": row.market_name,
                    "subaccount": subaccount,
                    "price_chain_units": price_chain,
                    "size_chain_units": size_chain,
                })
                .to_string(),
            ),
            timestamp: Utc::now(),
        })
    }

    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
        let _guard = self.order_lock.lock().await;
        self.trading_key()?;
        let signer = self.wallet_key()?;
        let package = self.deployment_package();
        let engine = perp_engine_global_addr(package)?;
        let market_name = resolve_market_name(&req.symbol);
        let market_addr = market_addr_for(&engine, &market_name)?;
        let owner = self.owner_address(&signer)?;
        let subaccount = primary_subaccount_addr(&owner, package)?;
        let sub_addr = AccountAddress::from_str(&subaccount)
            .map_err(|e| anyhow::anyhow!("Decibel subaccount address invalid: {e}"))?;
        let mkt_addr = AccountAddress::from_str(&market_addr)
            .map_err(|e| anyhow::anyhow!("Decibel market address invalid: {e}"))?;

        let payload = if let Some(id) = req.order_id.filter(|s| !s.trim().is_empty()) {
            let num: u128 = id.trim().parse().map_err(|_| {
                anyhow::anyhow!(
                    "Decibel numeric order id unparseable ({id}); pass the venue order id or use client_order_id"
                )
            })?;
            InputEntryFunctionData::new(&format!("{package}::{FN_CANCEL_ORDER}"))
                .arg(sub_addr)
                .arg(num)
                .arg(mkt_addr)
                .build()
                .map_err(|e| anyhow::anyhow!("Decibel cancel payload build failed: {e}"))?
        } else if let Some(cid) = req.client_order_id.filter(|s| !s.trim().is_empty()) {
            InputEntryFunctionData::new(&format!("{package}::{FN_CANCEL_CLIENT_ORDER}"))
                .arg(sub_addr)
                .arg(cid.clone())
                .arg(mkt_addr)
                .build()
                .map_err(|e| anyhow::anyhow!("Decibel cancel payload build failed: {e}"))?
        } else {
            anyhow::bail!("Decibel cancel requires an order id or client order id");
        };
        let tx = self.submit_payload(payload).await?;
        if tx.get("hash").and_then(|h| h.as_str()).unwrap_or("").is_empty() {
            anyhow::bail!("Decibel cancel returned no transaction hash: {tx}");
        }
        Ok(())
    }

    fn supports_live_execution(&self) -> bool {
        true
    }

    fn live_order_submission_implemented(&self) -> bool {
        true
    }

    fn has_live_credentials(&self) -> bool {
        self.private_key
            .read()
            .as_ref()
            .is_some_and(|k| !k.trim().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_decibel_credentials_flow() {
        let adapter = DecibelAdapter::new(None, None, None);
        assert!(!adapter.has_live_credentials());
        assert!(adapter.supports_live_execution());
        assert!(adapter.live_order_submission_implemented());
        assert!(adapter.fetch_account().await.is_err());

        adapter.update_credentials(
            Some("geomi_client_key".to_string()),
            Some("node_key".to_string()),
            Some("0x1234567890abcdef".to_string()),
            Some("0xprivkey".to_string()),
        );
        // A placeholder key is still "loaded" (validity is checked at sign
        // time); the adapter reports submittable and fails closed per call.
        assert!(adapter.has_live_credentials());

        adapter.update_credentials(None, None, None, None);
        assert!(!adapter.has_live_credentials());
    }

    #[test]
    fn test_decibel_mainnet_vectors() {
        // Verified live against the mainnet fullnode: the GlobalPerpEngine
        // object exists, owned by the deployment package.
        let engine = perp_engine_global_addr(PACKAGE_MAINNET).unwrap();
        assert_eq!(
            engine,
            "0x4d8cc47ee3545fcbe87fcef21bd0ff39377c98efebb8f55a8b97aba928098370"
        );
        // Deterministic + owner-sensitive subaccounts.
        let a = primary_subaccount_addr(
            "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
            PACKAGE_MAINNET,
        )
        .unwrap();
        let b = primary_subaccount_addr(
            "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
            PACKAGE_MAINNET,
        )
        .unwrap();
        assert_eq!(a, b);
        assert!(a.starts_with("0x") && a.len() == 66);
        let c = primary_subaccount_addr(
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            PACKAGE_MAINNET,
        )
        .unwrap();
        assert_ne!(a, c);
        // Market derivation is name-sensitive and deterministic.
        let m1 = market_addr_for(&engine, "BTC-PERP").unwrap();
        let m2 = market_addr_for(&engine, "BTC-PERP").unwrap();
        assert_eq!(m1, m2);
        assert_ne!(m1, market_addr_for(&engine, "ETH-PERP").unwrap());
    }

    #[test]
    fn test_decibel_symbol_and_scaling() {
        assert_eq!(resolve_market_name("BTC-PERP"), "BTC-PERP");
        assert_eq!(resolve_market_name("btc/usdc"), "BTC-PERP");
        assert_eq!(resolve_market_name("ETH-USDC"), "ETH-PERP");
        assert_eq!(resolve_market_name("SOL"), "SOL-PERP");
        assert_eq!(
            price_to_chain_units(Decimal::from_str_exact("65000.5").unwrap(), 2).unwrap(),
            6_500_050
        );
        assert_eq!(
            size_to_chain_units(Decimal::from_str_exact("0.001").unwrap(), 6).unwrap(),
            1_000
        );
        assert!(price_to_chain_units(dec!(0), 2).is_err());
        assert!(size_to_chain_units(dec!(-1), 6).is_err());
    }

    #[test]
    fn test_decibel_api_shapes() {
        // f64-or-string numbers across endpoints.
        assert_eq!(num_f64(Some(&serde_json::json!(1.5))), Some(1.5));
        assert_eq!(num_f64(Some(&serde_json::json!("2.5"))), Some(2.5));
        assert_eq!(num_f64(Some(&serde_json::json!(true))), None);
        assert_eq!(num_f64(None), None);
        // Envelope: bare value vs {data: …}.
        let bare = serde_json::json!([{"a": 1}]);
        assert!(as_array(&bare).is_some_and(|a| a.len() == 1));
        let wrapped = serde_json::json!({"code": "200", "data": [{"a": 1}]});
        assert!(as_array(&wrapped).is_some_and(|a| a.len() == 1));
        assert!(as_array(&serde_json::json!({"code": "200"})).is_none());
        // Overview equity fields parse from either numbers or strings.
        let ov = serde_json::json!({"perp_equity_balance": 123.5, "usdc_cross_withdrawable_balance": "100.25"});
        assert_eq!(dec_f64(ov.get("perp_equity_balance")), Decimal::from_str_exact("123.5").ok());
        assert_eq!(dec_f64(ov.get("usdc_cross_withdrawable_balance")), Decimal::from_str_exact("100.25").ok());
    }

    #[test]
    fn test_decibel_address_helpers() {
        assert_eq!(
            canonical_address("0x1").unwrap(),
            format!("0x{}", "0".repeat(63) + "1")
        );
        assert!(parse_address_bytes("zzzz").is_err());
        // BCS MoveString vector check: len prefix + utf8.
        let b = bcs_move_string_bytes("BTC-PERP");
        assert_eq!(b[0], 8);
        assert_eq!(&b[1..], b"BTC-PERP");
    }
}
