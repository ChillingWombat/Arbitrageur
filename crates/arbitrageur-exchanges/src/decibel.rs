use crate::rate_limiter::AccountRequestCoordinator;
use aptos_sdk::{
    account::Ed25519Account,
    api::fullnode::FullnodeClient,
    transaction::{builder::TransactionBuilder, input::InputEntryFunctionData},
    types::{AccountAddress, ChainId},
    AptosConfig,
};
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
use sha3::{Digest, Sha3_256};
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
const PACKAGE_MAINNET: &str = "0x50ead22afd6ffd9769e3b3d6e0e64a2a350d68e8b102c4e72e33d0b8cfdfdb06";
const PACKAGE_TESTNET: &str = "0xe7da2794b1d8af76532ed95f38bfdf1136abfd8ea3a240189971988a83101b7f";
const FN_PLACE_ORDER: &str = "dex_accounts_entry::place_order_to_subaccount";
const FN_CANCEL_ORDER: &str = "dex_accounts_entry::cancel_order_to_subaccount";
const FN_CANCEL_CLIENT_ORDER: &str = "dex_accounts_entry::cancel_client_order_to_subaccount";
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
    let h = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .unwrap_or(t);
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

/// BCS encoding of a `vector<u8>`/`String`: ULEB128 length prefix + bytes.
fn bcs_len_prefixed(b: &[u8]) -> Vec<u8> {
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

/// BCS encoding of a Move `String`: ULEB128 length prefix + UTF-8 bytes.
fn bcs_move_string_bytes(s: &str) -> Vec<u8> {
    bcs_len_prefixed(s.as_bytes())
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
fn market_addr_for(perp_engine_global_hex: &str, market_name: &str) -> anyhow::Result<String> {
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

// ---------------------------------------------------------------------------
// Derivable-account (SIWA) login translation — mirrors the official SDK's
// `src/derivable-account.ts`. A wallet that connects at app.decibel.trade is
// often not an Aptos account at all: EVM/Sui/Solana sign-ins derive an Aptos
// account via `0x1::{ethereum,solana,sui}_derivable_account::authenticate`,
// and that derived address — not the connected wallet — owns the subaccount.
// ---------------------------------------------------------------------------

/// SIWA domain the Decibel app signs login messages under; the derived
/// account is domain-scoped, so testnet derives under its own hostname.
const SIWA_DOMAIN_MAINNET: &str = "app.decibel.trade";
const SIWA_DOMAIN_TESTNET: &str = "testnet-app.decibel.trade";

/// Aptos `DerivableAbstractedAccount::computeAccountAddress`:
/// `sha3_256(bcs(FunctionInfo 0x1::{module}::{name}) || bcs_bytes(
/// AbstractPublicKey{ bcs_str(identity), bcs_str(domain) }) || 0x05)`.
fn derivable_account_address(
    module_name: &str,
    fn_name: &str,
    identity: &str,
    domain: &str,
) -> String {
    let mut function_info = vec![0u8; 31];
    function_info.push(1); // module address 0x1, BCS-serialized address = 32 bytes
    function_info.extend_from_slice(&bcs_move_string_bytes(module_name));
    function_info.extend_from_slice(&bcs_move_string_bytes(fn_name));
    let mut public_key = bcs_move_string_bytes(identity);
    public_key.extend_from_slice(&bcs_move_string_bytes(domain));
    let mut h = Sha3_256::new();
    h.update(&function_info);
    h.update(bcs_len_prefixed(&public_key));
    h.update([0x05u8]);
    format!("0x{}", hex::encode(h.finalize()))
}

/// Aptos account an EVM wallet's Decibel login derives to. The identity is
/// the EIP-55 checksummed address string — input case is normalized.
fn evm_login_account(evm_addr: &str, domain: &str) -> anyhow::Result<String> {
    let a = alloy::primitives::Address::from_str(evm_addr.trim())
        .map_err(|e| anyhow::anyhow!("'{evm_addr}' is not an EVM address: {e}"))?;
    Ok(derivable_account_address(
        "ethereum_derivable_account",
        "authenticate",
        &a.to_checksum(None),
        domain,
    ))
}

/// Aptos account a Sui wallet's Decibel login derives to. The identity is the
/// `0x`-prefixed lowercase 64-hex address exactly as Sui wallets report it.
fn sui_login_account(sui_addr: &str, domain: &str) -> anyhow::Result<String> {
    let h = sui_addr.trim().to_lowercase();
    let h = h.strip_prefix("0x").unwrap_or(&h).to_string();
    if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!("'{sui_addr}' is not a Sui address");
    }
    Ok(derivable_account_address(
        "sui_derivable_account",
        "authenticate",
        &format!("0x{h}"),
        domain,
    ))
}

/// Aptos account a Solana wallet's Decibel login derives to. The identity is
/// the base58 pubkey string as-is.
fn solana_login_account(sol_addr: &str, domain: &str) -> anyhow::Result<String> {
    let s = sol_addr.trim();
    let raw = bs58::decode(s)
        .into_vec()
        .map_err(|_| anyhow::anyhow!("'{sol_addr}' is not a base58 Solana address"))?;
    if raw.len() != 32 {
        anyhow::bail!("'{sol_addr}' is not a Solana pubkey");
    }
    Ok(derivable_account_address(
        "solana_derivable_account",
        "authenticate",
        s,
        domain,
    ))
}

/// Candidate on-chain owner addresses for a configured login wallet string,
/// most likely first. A 40-hex EVM address maps to its derivable Aptos
/// account; a 64-hex value is tried as a native Aptos account then as a Sui
/// login; non-hex input is tried as a Solana pubkey. Probing the live
/// subaccounts listing picks whichever candidate actually owns one.
fn owner_candidates(configured: &str, testnet: bool) -> Vec<String> {
    let t = configured.trim();
    let domain = if testnet {
        SIWA_DOMAIN_TESTNET
    } else {
        SIWA_DOMAIN_MAINNET
    };
    let hex_part = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .unwrap_or(t);
    let is_hex = !hex_part.is_empty() && hex_part.chars().all(|c| c.is_ascii_hexdigit());
    let mut out: Vec<String> = Vec::new();
    if is_hex && hex_part.len() == 40 {
        if let Ok(d) = evm_login_account(t, domain) {
            out.push(d);
        }
    }
    if is_hex && hex_part.len() <= 64 {
        if let Ok(c) = canonical_address(t) {
            if !out.contains(&c) {
                out.push(c);
            }
        }
        if hex_part.len() == 64 {
            if let Ok(d) = sui_login_account(t, domain) {
                if !out.contains(&d) {
                    out.push(d);
                }
            }
        }
    }
    if !(is_hex && hex_part.len() == 64) {
        // Anything that isn't already a full-width Aptos address may still be
        // a base58 Solana pubkey (short all-digit strings are valid base58).
        if let Ok(d) = solana_login_account(t, domain) {
            if !out.contains(&d) {
                out.push(d);
            }
        }
    }
    out
}

/// Map an Arbitrageur venue symbol to a Decibel perp market name.
/// Decibel perp markets are USDC-quoted and named like `BTC-PERP`; a foreign
/// quote must never silently resolve onto the USDC market.
fn resolve_market_name(symbol: &str) -> anyhow::Result<String> {
    let s = symbol.trim().to_uppercase().replace(['/', ':'], "-");
    if let Some(base) = s.strip_suffix("-PERP") {
        if base.is_empty() || base.contains('-') {
            anyhow::bail!("Decibel market name '{symbol}' is malformed");
        }
        return Ok(s);
    }
    if s.contains('-') {
        let parts: Vec<&str> = s.split('-').collect();
        let [base, quote] = parts.as_slice() else {
            anyhow::bail!("Decibel symbol '{symbol}' is not BASE-QUOTE");
        };
        if !quote.eq_ignore_ascii_case("USDC") {
            anyhow::bail!("Decibel perpetual markets are USDC-quoted; '{symbol}' is not supported");
        }
        if base.is_empty() {
            anyhow::bail!("Decibel symbol '{symbol}' has an empty base");
        }
        return Ok(format!("{base}-PERP"));
    }
    if s.is_empty() {
        anyhow::bail!("Decibel symbol is empty");
    }
    Ok(format!("{s}-PERP"))
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
    /// On-chain owner resolved from the configured login wallet via the live
    /// subaccounts listing (cross-chain wallets derive an Aptos account).
    /// Cached for the credential lifetime; cleared on `update_credentials`.
    resolved_owner: RwLock<Option<String>>,
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
        let base =
            base_url.unwrap_or_else(|| "https://api.mainnet.aptoslabs.com/decibel".to_string());
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
                Client::builder()
                    .timeout(std::time::Duration::from_secs(15))
                    .default_headers(headers)
                    .build()
                    .unwrap_or_default()
            },
            base_url: base,
            api_key: RwLock::new(api_key.map(Zeroizing::new)),
            node_api_key: RwLock::new(api_secret.map(Zeroizing::new)),
            wallet_address: RwLock::new(None),
            private_key: RwLock::new(None::<Zeroizing<String>>),
            resolved_owner: RwLock::new(None),
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
        *self.resolved_owner.write() = None;
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
        let h = t
            .strip_prefix("0x")
            .or_else(|| t.strip_prefix("0X"))
            .unwrap_or(t);
        let clean: String = h.chars().filter(|c| !c.is_whitespace()).collect();
        Ed25519Account::from_private_key_hex(&clean)
            .map_err(|e| anyhow::anyhow!("Decibel private key is not a valid Ed25519 key: {e}"))
    }

    /// The configured login wallet exactly as entered (any chain's address
    /// format), falling back to the signing key's own Aptos account when
    /// unset. Shape translation + on-chain verification happen in
    /// `resolve_owner` — this value is NOT canonicalized so an EVM wallet
    /// keeps its 40-hex shape.
    fn configured_owner(&self, account: &Ed25519Account) -> String {
        if let Some(w) = self.wallet_address.read().clone() {
            if !w.trim().is_empty() {
                return w.trim().to_string();
            }
        }
        account.address().to_string()
    }

    /// Resolve a configured login wallet string to the on-chain owner that
    /// actually holds a subaccount. The wallet a user connects at
    /// app.decibel.trade may be EVM/Sui/Solana — those derive an Aptos
    /// account under the deployment's SIWA domain — so every candidate shape
    /// is probed against `subaccounts?owner=` and the first non-empty wins.
    /// The result is cached for the credential lifetime.
    async fn resolve_owner(&self, configured: &str) -> anyhow::Result<String> {
        let configured = configured.trim();
        if let Some(o) = self.resolved_owner.read().clone() {
            return Ok(o);
        }
        let candidates = owner_candidates(configured, self.is_testnet());
        if candidates.is_empty() {
            anyhow::bail!(
                "Decibel login wallet '{configured}' is not a recognizable Aptos, EVM, Sui, or Solana address"
            );
        }
        let mut last_err = String::new();
        for cand in &candidates {
            match self.subaccount_rows(cand).await {
                Ok(rows) if !rows.is_empty() => {
                    *self.resolved_owner.write() = Some(cand.clone());
                    return Ok(cand.clone());
                }
                Ok(_) => {}
                Err(e) => last_err = e.to_string(),
            }
        }
        if !last_err.is_empty() {
            anyhow::bail!("Decibel subaccounts lookup failed for '{configured}': {last_err}");
        }
        let canonical = canonical_address(configured).unwrap_or_default();
        let detail = candidates
            .iter()
            .find(|c| **c != canonical)
            .map(|c| format!(" (its Aptos login account is {c})"))
            .unwrap_or_default();
        anyhow::bail!(
            "Decibel login wallet {configured}{detail} has no trading account yet — create one at https://app.decibel.trade, then re-read"
        );
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
        let want = resolve_market_name(symbol)?;
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
                            r.get(k)
                                .and_then(|v| v.as_f64())
                                .and_then(Decimal::from_f64_retain)
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

    /// Raw `GET {trading}/api/v1/subaccounts?owner=` rows for an owner.
    /// Checksum-cased wallets have been observed to miss; retry lowercase.
    /// An owner with no subaccounts returns an empty vec (not an error).
    async fn subaccount_rows(&self, owner: &str) -> anyhow::Result<Vec<serde_json::Value>> {
        let owner = owner.trim();
        for candidate in [owner.to_string(), owner.to_lowercase()] {
            let val = self
                .get_json(&format!("/api/v1/subaccounts?owner={candidate}"))
                .await?;
            let rows = as_array(&val).ok_or_else(|| {
                anyhow::anyhow!("Decibel subaccounts response has no list for owner {owner}")
            })?;
            if !rows.is_empty() {
                return Ok(rows.clone());
            }
        }
        Ok(Vec::new())
    }

    /// Resolve the owner's primary subaccount object address via the official
    /// subaccounts listing (primary first, else first active, else first
    /// row). Fails closed when the owner has no subaccount — the fix is
    /// creating one in the Decibel app, not guessing an address.
    async fn primary_subaccount(&self, owner: &str) -> anyhow::Result<String> {
        let rows = self.subaccount_rows(owner).await?;
        if rows.is_empty() {
            anyhow::bail!(
                "Decibel owner {owner} has no subaccount yet — create one at https://app.decibel.trade, then re-read"
            );
        }
        let pick = rows
            .iter()
            .find(|r| {
                r.get("is_primary")
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false)
            })
            .or_else(|| {
                rows.iter()
                    .find(|r| r.get("is_active").and_then(|b| b.as_bool()).unwrap_or(true))
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

    /// Recent transactions touching our primary subaccount, ascending by
    /// ledger version and strictly AFTER `after_version` — the post-commit
    /// window where a resting order's fills/cancels actually land.
    async fn recent_subaccount_txs(
        &self,
        subaccount: &str,
        after_version: u64,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let url = format!(
            "{}/accounts/{subaccount}/transactions?limit=50",
            self.fullnode_url()
        );
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!(
                "Decibel fullnode account-transactions status {} for {subaccount}",
                resp.status()
            );
        }
        let mut txs: Vec<serde_json::Value> = resp.json().await?;
        let ver = |t: &serde_json::Value| {
            t.get("version")
                .and_then(|v| v.as_str())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0)
        };
        txs.retain(|t| ver(t) > after_version);
        txs.sort_by_key(ver);
        Ok(txs)
    }

    fn fullnode_url(&self) -> &'static str {
        if self.is_testnet() {
            "https://fullnode.testnet.aptoslabs.com/v1"
        } else {
            "https://fullnode.mainnet.aptoslabs.com/v1"
        }
    }
}

/// Authoritative outcome decoded from a committed Decibel transaction's
/// events. `OrderEvent` carries the order lifecycle (`orig_size`,
/// `remaining_size`, `status.__variant__`); `TradeEvent` carries per-fill
/// price/size/fee. All chain-unit quantities are de-scaled with the market's
/// `sz_decimals`/`px_decimals`; fees are balance-sheet (USDC, 6 decimals).
#[derive(Debug, Default)]
struct DecibelCommitState {
    venue_order_id: Option<String>,
    client_order_id: Option<String>,
    status_variant: Option<String>,
    /// Order limit price echoed by the venue (human units), when emitted.
    order_price: Option<Decimal>,
    orig_qty: Decimal,
    remaining_qty: Decimal,
    filled_qty: Decimal,
    avg_fill_price: Option<Decimal>,
    fee_usd: Decimal,
}

fn dec_unit(v: &serde_json::Value, key: &str, scale: u32) -> Option<Decimal> {
    let raw = v.get(key)?;
    let units = raw
        .as_str()
        .and_then(|s| Decimal::from_str_exact(s).ok())
        .or_else(|| raw.as_u64().map(Decimal::from))?;
    let factor = Decimal::from(10u64.pow(scale.min(18)));
    Some(units / factor)
}

/// `OptionType<T>` fields serialise as `{"vec": [...]}`; plain strings and
/// numbers also appear in the wild.
fn option_str(v: Option<&serde_json::Value>) -> Option<String> {
    let v = v?;
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    if let Some(n) = v.as_u64() {
        return Some(n.to_string());
    }
    v.get("vec")
        .and_then(|x| x.as_array())
        .and_then(|a| a.first())
        .map(|inner| {
            inner
                .as_str()
                .map(str::to_string)
                .or_else(|| inner.as_u64().map(|n| n.to_string()))
                .unwrap_or_else(|| inner.to_string())
        })
}

/// Address-valued event fields arrive either as a plain `"0x.."` string or
/// as a Move `Object` (`{"inner": "0x.."}`).
fn event_addr(v: Option<&serde_json::Value>) -> String {
    v.and_then(|a| {
        a.as_str()
            .map(str::to_string)
            .or_else(|| a.get("inner").and_then(|i| i.as_str()).map(str::to_string))
    })
    .unwrap_or_default()
    .to_lowercase()
}

/// Raw event accumulator — latest OrderEvent lifecycle fields plus summed
/// TradeEvent fill/fee units across ANY number of transactions. Scanning
/// the placement tx alone is blind to resting-order fills that land in
/// later transactions; the accumulator lets a scan span them.
#[derive(Default)]
struct OrderScan {
    venue_order_id: Option<String>,
    client_order_id: Option<String>,
    status_variant: Option<String>,
    order_price: Option<Decimal>,
    orig_qty: Decimal,
    remaining_qty: Decimal,
    fill_sz: Decimal,
    fill_notional_units: Decimal,
    fee_units: Decimal,
}

/// Does this event row belong to the order we're scanning? `cid`/`oid` are
/// the persisted identity; `None, None` (the commit tx) accepts every row
/// for the subaccount — our own placement tx emits only our order's rows.
fn order_event_matches(data: &serde_json::Value, cid: Option<&str>, oid: Option<&str>) -> bool {
    if cid.is_none() && oid.is_none() {
        return true;
    }
    let ev_cid = option_str(data.get("client_order_id"));
    let ev_oid = data.get("order_id").and_then(|id| {
        id.as_str()
            .map(str::to_string)
            .or_else(|| id.as_u64().map(|n| n.to_string()))
            .or_else(|| option_str(id.get("order_id")))
    });
    if let (Some(w), Some(g)) = (cid, ev_cid.as_deref()) {
        if g == w {
            return true;
        }
    }
    if let (Some(w), Some(g)) = (oid, ev_oid.as_deref()) {
        if g == w {
            return true;
        }
    }
    false
}

/// Fold one transaction's events into `acc`. OrderEvent rows update the
/// lifecycle fields (later txs overwrite earlier ones — scan in ascending
/// version order); TradeEvent rows add to the cumulative fill/fee sums.
fn scan_tx_events(
    acc: &mut OrderScan,
    tx: &serde_json::Value,
    subaccount: &str,
    cid: Option<&str>,
    oid: Option<&str>,
    sz_decimals: u32,
    px_decimals: u32,
) {
    let Some(events) = tx.get("events").and_then(|e| e.as_array()) else {
        return;
    };
    let sub = subaccount.trim().to_lowercase();
    for ev in events {
        let Some(ty) = ev.get("type").and_then(|t| t.as_str()) else {
            continue;
        };
        let Some(data) = ev.get("data") else {
            continue;
        };
        if ty.contains("OrderEvent") {
            let user = event_addr(data.get("user"));
            if user != sub || !order_event_matches(data, cid, oid) {
                continue;
            }
            if let Some(v) = data.get("order_id").and_then(|id| {
                id.as_str()
                    .map(str::to_string)
                    .or_else(|| id.as_u64().map(|n| n.to_string()))
                    .or_else(|| option_str(id.get("order_id")))
            }) {
                acc.venue_order_id = Some(v);
            }
            if let Some(v) = option_str(data.get("client_order_id")) {
                acc.client_order_id = Some(v);
            }
            if let Some(v) = data
                .get("status")
                .and_then(|s| {
                    s.get("__variant__")
                        .and_then(|v| v.as_str())
                        .or_else(|| s.as_str())
                })
                .map(str::to_string)
            {
                acc.status_variant = Some(v);
            }
            if let Some(v) = dec_unit(data, "price", px_decimals).filter(|p| *p > dec!(0)) {
                acc.order_price = Some(v);
            }
            if let Some(o) = dec_unit(data, "orig_size", sz_decimals) {
                acc.orig_qty = o;
            }
            if let Some(r) = dec_unit(data, "remaining_size", sz_decimals) {
                acc.remaining_qty = r;
            }
        } else if ty.contains("TradeEvent") {
            let acct = event_addr(data.get("account"));
            if acct != sub || !order_event_matches(data, cid, oid) {
                continue;
            }
            let sz = data
                .get("size")
                .and_then(|s| s.as_str())
                .and_then(|s| Decimal::from_str_exact(s).ok())
                .unwrap_or(dec!(0));
            let px = data
                .get("price")
                .and_then(|s| s.as_str())
                .and_then(|s| Decimal::from_str_exact(s).ok())
                .unwrap_or(dec!(0));
            acc.fill_sz += sz;
            acc.fill_notional_units += sz * px;
            acc.fee_units += data
                .get("fee")
                .and_then(|s| s.as_str())
                .and_then(|s| Decimal::from_str_exact(s).ok())
                .unwrap_or(dec!(0));
        }
    }
}

/// Scale raw accumulators into ledger units and apply the sanity gates.
fn finalize_order_scan(acc: &OrderScan, sz_decimals: u32, px_decimals: u32) -> DecibelCommitState {
    let mut st = DecibelCommitState {
        venue_order_id: acc.venue_order_id.clone(),
        client_order_id: acc.client_order_id.clone(),
        status_variant: acc.status_variant.clone(),
        order_price: acc.order_price,
        orig_qty: acc.orig_qty,
        remaining_qty: acc.remaining_qty,
        ..Default::default()
    };
    let sz_factor = Decimal::from(10u64.pow(sz_decimals.min(18)));
    let px_factor = Decimal::from(10u64.pow(px_decimals.min(18)));
    if acc.fill_sz > dec!(0) {
        st.filled_qty = acc.fill_sz / sz_factor;
        let vwap_units = acc.fill_notional_units / acc.fill_sz;
        let vwap = vwap_units / px_factor;
        // Scale sanity: the documented px unit is the collateral balance
        // sheet's; a fill VWAP that deviates wildly from the order's own
        // limit price means the assumed scale is wrong — report no price
        // rather than write a 10^k-scaled figure into accounting.
        let plausible = match st.order_price {
            Some(limit) if limit > dec!(0) => {
                let ratio = vwap / limit;
                ratio > dec!(0.5) && ratio < dec!(2.0)
            }
            _ => true,
        };
        if plausible {
            st.avg_fill_price = Some(vwap);
        }
    } else if st.orig_qty > st.remaining_qty {
        st.filled_qty = st.orig_qty - st.remaining_qty;
    }
    if st.filled_qty < dec!(0) {
        st.filled_qty = dec!(0);
    }
    // Collateral balance-sheet amounts are USDC-denominated (6 decimals).
    // Negative fee = rebate; expose cost only and never a negative credit
    // that would inflate computed PnL. Sanity-gate against the fill
    // notional so a wrong-scale assumption cannot corrupt accounting.
    let fee = acc.fee_units / Decimal::from(10u64.pow(6));
    let notional = st
        .avg_fill_price
        .or(st.order_price)
        .map(|p| st.filled_qty * p)
        .unwrap_or(dec!(0));
    if fee > dec!(0) && (notional <= dec!(0) || fee <= notional * dec!(0.10)) {
        st.fee_usd = fee;
    }
    st
}

/// Scan `tx.events` for our subaccount's `OrderEvent` (last one wins — the
/// final lifecycle state) plus every `TradeEvent` booked to our account
/// (each fill emits one maker + one taker row; ours is `account == sub`).
fn parse_commit_order_state(
    tx: &serde_json::Value,
    subaccount: &str,
    sz_decimals: u32,
    px_decimals: u32,
) -> DecibelCommitState {
    let mut acc = OrderScan::default();
    scan_tx_events(
        &mut acc,
        tx,
        subaccount,
        None,
        None,
        sz_decimals,
        px_decimals,
    );
    finalize_order_scan(&acc, sz_decimals, px_decimals)
}

/// Map a Decibel `OrderEvent` status variant onto `OrderStatus` using the
/// decoded quantities — `SIZE_REDUCED`/`OPEN` with zero remaining is still a
/// full fill, and a cancelled order that took a partial fill first reports
/// truthfully as `Cancelled` with `filled_qty > 0`.
fn decibel_order_status(variant: Option<&str>, filled: Decimal, remaining: Decimal) -> OrderStatus {
    let v = variant.unwrap_or("").to_uppercase();
    match v.as_str() {
        "FILLED" => OrderStatus::Filled,
        "CANCELLED" | "CANCELED" => OrderStatus::Cancelled,
        "REJECTED" => OrderStatus::Rejected,
        "EXPIRED" => OrderStatus::Expired,
        _ => {
            if remaining <= dec!(0) && filled > dec!(0) {
                OrderStatus::Filled
            } else if filled > dec!(0) {
                OrderStatus::PartiallyFilled
            } else {
                OrderStatus::New
            }
        }
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
        let resp = self
            .client
            .get("https://fullnode.mainnet.aptoslabs.com/v1")
            .send()
            .await?;
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
        let market_name = resolve_market_name(symbol)?;
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
                            serde_json::Value::Array(a) => (dec_f64(a.first()), dec_f64(a.get(1))),
                            serde_json::Value::Object(o) => (
                                dec_f64(o.get("price").or_else(|| o.get("px"))),
                                dec_f64(
                                    o.get("size")
                                        .or_else(|| o.get("sz"))
                                        .or_else(|| o.get("quantity")),
                                ),
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
        let market_name = resolve_market_name(symbol)?;
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
        let rate_8h =
            Decimal::from_f64_retain(bps / 10_000.0 * (8.0 / period_h)).unwrap_or(dec!(0));
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
        let configured = match wallet.filter(|s| !s.trim().is_empty()) {
            Some(w) => w.trim().to_string(),
            // No login wallet configured: a native-Aptos login's API key can
            // be the account key itself — fall back to the signer's address.
            None if self
                .private_key
                .read()
                .as_ref()
                .is_some_and(|k| !k.trim().is_empty()) =>
            {
                self.wallet_key()?.address().to_string()
            }
            None => anyhow::bail!(
                "Decibel account query requires a wallet address — set Login Wallet Address in Settings > Exchange Accounts to the Aptos account, or the EVM/Sui/Solana wallet you signed in with"
            ),
        };

        // 1. Owner -> primary subaccount (official `/api/v1/subaccounts`).
        let owner = self.resolve_owner(&configured).await?;
        let subaccount = self.primary_subaccount(&owner).await?;

        // 2. Balances via the official `/api/v1/account_overviews`.
        let overview = self
            .get_json(&format!("/api/v1/account_overviews?account={subaccount}"))
            .await
            .map(|v| envelope_data(&v).clone())
            .unwrap_or(serde_json::Value::Null);
        let total_equity = dec_f64(overview.get("perp_equity_balance")).unwrap_or(dec!(0));
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
                        (mark - entry) * qty * if size > 0.0 { dec!(1) } else { dec!(-1) }
                    } else {
                        dec!(0)
                    };
                    let liq =
                        dec_f64(r.get("estimated_liquidation_price")).filter(|p| *p > dec!(0));
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

    fn normalize_account_position(&self, position: &Position) -> anyhow::Result<Option<Position>> {
        let mut position = position.clone();
        if let Some(base) = position.symbol.strip_suffix("-PERP") {
            if base.is_empty() {
                anyhow::bail!(
                    "Decibel account symbol '{}' has an empty base",
                    position.symbol
                );
            }
            position.symbol = format!("{}-USDC", base.to_uppercase());
        } else if position.symbol.contains('-') {
            position.symbol = position.symbol.to_uppercase();
            let market = resolve_market_name(&position.symbol)?;
            position.symbol = market
                .strip_suffix("-PERP")
                .map(|base| format!("{base}-USDC"))
                .unwrap_or(position.symbol);
        } else {
            anyhow::bail!(
                "Decibel account symbol '{}' is not a *-PERP market",
                position.symbol
            );
        }
        Ok(Some(position))
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
        // Refuse non-exact lot sizes rather than silently under-submitting a
        // hedge or risk-reducing close.
        let qty = order.quantity;
        if row.lot_size > dec!(0) {
            let lots = qty / row.lot_size;
            if lots < dec!(1) || !lots.fract().is_zero() {
                anyhow::bail!(
                    "Decibel order size {} is not an exact lot multiple {} for {}",
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
        let owner = self.resolve_owner(&self.configured_owner(&signer)).await?;
        let subaccount = primary_subaccount_addr(&owner, package)?;
        let sub_addr = AccountAddress::from_str(&subaccount)
            .map_err(|e| anyhow::anyhow!("Decibel subaccount address invalid: {e}"))?;
        let mkt_addr = AccountAddress::from_str(&market_addr)
            .map_err(|e| anyhow::anyhow!("Decibel market address invalid: {e}"))?;

        if order.client_order_id.trim().is_empty() {
            anyhow::bail!("Decibel order requires a non-empty client_order_id");
        }
        let client_id = Some(order.client_order_id.clone());
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
        if tx.get("success").and_then(|s| s.as_bool()) == Some(false) {
            let vm = tx
                .get("vm_status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            anyhow::bail!("Decibel order transaction {hash} reverted on-chain: {vm}");
        }
        let st = parse_commit_order_state(&tx, &subaccount, row.sz_decimals, row.px_decimals);
        let status = decibel_order_status(st.status_variant.as_deref(), st.filled_qty, {
            if st.orig_qty > dec!(0) {
                st.remaining_qty
            } else {
                qty - st.filled_qty
            }
        });
        // The persisted order id is the committed transaction hash — the only
        // handle that resolves authoritatively on-chain after a restart. The
        // numeric venue order id is preserved in raw_response for support.
        let venue_order_id = hash.clone();
        Ok(OrderResponse {
            order_id: venue_order_id,
            client_order_id: order.client_order_id.clone(),
            venue: self.venue_id.clone(),
            symbol: order.symbol.clone(),
            status,
            original_qty: if st.orig_qty > dec!(0) {
                st.orig_qty
            } else {
                qty
            },
            filled_qty: st.filled_qty,
            remaining_qty: if st.orig_qty > dec!(0) {
                st.remaining_qty
            } else {
                (qty - st.filled_qty).max(dec!(0))
            },
            avg_fill_price: st.avg_fill_price,
            fee: st.fee_usd,
            fee_currency: "USDC".to_string(),
            raw_response: Some(
                serde_json::json!({
                    "tx_hash": hash,
                    "market": row.market_name,
                    "subaccount": subaccount,
                    "venue_order_id_num": st.venue_order_id,
                    "order_status": st.status_variant,
                    "price_chain_units": price_chain,
                    "size_chain_units": size_chain,
                })
                .to_string(),
            ),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        let hash = order_id.trim();
        if !hash.starts_with("0x") || hash.len() < 10 {
            anyhow::bail!(
                "Decibel order status requires the committed transaction hash; '{hash}' is not resolvable on-chain"
            );
        }
        let signer = self.wallet_key()?;
        let owner = self.resolve_owner(&self.configured_owner(&signer)).await?;
        let subaccount = primary_subaccount_addr(&owner, self.deployment_package())?;
        let url = format!("{}/transactions/by_hash/{hash}", self.fullnode_url());
        let resp = self.client.get(&url).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            anyhow::bail!("Decibel transaction {hash} not found on the fullnode");
        }
        if !resp.status().is_success() {
            anyhow::bail!(
                "Decibel fullnode status {} for transaction {hash}",
                resp.status()
            );
        }
        let tx: serde_json::Value = resp.json().await?;
        if tx.get("success").and_then(|s| s.as_bool()) == Some(false) {
            return Ok(OrderResponse {
                order_id: order_id.to_string(),
                client_order_id: client_order_id.unwrap_or_default().to_string(),
                venue: self.venue_id.clone(),
                symbol: symbol.to_string(),
                status: OrderStatus::Rejected,
                original_qty: dec!(0),
                filled_qty: dec!(0),
                remaining_qty: dec!(0),
                avg_fill_price: None,
                fee: dec!(0),
                fee_currency: "USDC".to_string(),
                raw_response: Some(tx.to_string()),
                timestamp: Utc::now(),
            });
        }
        let row = self.fetch_market_row(symbol).await?;
        let mut st = parse_commit_order_state(&tx, &subaccount, row.sz_decimals, row.px_decimals);
        if st.status_variant.is_none() && st.venue_order_id.is_none() {
            anyhow::bail!(
                "Decibel transaction {hash} carries no OrderEvent for subaccount {subaccount}"
            );
        }
        // Identity: the on-chain client_order_id must match the persisted one
        // when both are known — a mismatched transaction is not our order.
        if let (Some(want), Some(got)) = (
            client_order_id.filter(|s| !s.trim().is_empty()),
            st.client_order_id.as_deref(),
        ) {
            if got != want {
                anyhow::bail!(
                    "Decibel transaction {hash} client_order_id '{got}' != persisted '{want}'"
                );
            }
        }
        let status = decibel_order_status(
            st.status_variant.as_deref(),
            st.filled_qty,
            st.remaining_qty,
        );
        // Stage 2 — resting-order fills land in LATER transactions the
        // placement tx cannot see. While the order is still working, scan
        // post-commit subaccount transactions for events keyed to our order
        // identity. Venue lifecycle fields (status/orig/remaining) from the
        // latest OrderEvent are authoritative for cumulative state and
        // outrank the additive TradeEvent sum, which covers windows where no
        // lifecycle row was emitted.
        if matches!(status, OrderStatus::New | OrderStatus::PartiallyFilled) {
            let cid_key = st
                .client_order_id
                .clone()
                .or_else(|| client_order_id.map(str::to_string))
                .filter(|v| !v.is_empty());
            let oid_key = st.venue_order_id.clone();
            if cid_key.is_some() || oid_key.is_some() {
                let commit_ver = tx
                    .get("version")
                    .and_then(|v| v.as_str())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0);
                match self.recent_subaccount_txs(&subaccount, commit_ver).await {
                    Ok(recent) => {
                        let mut acc = OrderScan::default();
                        for rtx in &recent {
                            scan_tx_events(
                                &mut acc,
                                rtx,
                                &subaccount,
                                cid_key.as_deref(),
                                oid_key.as_deref(),
                                row.sz_decimals,
                                row.px_decimals,
                            );
                        }
                        let later = finalize_order_scan(&acc, row.sz_decimals, row.px_decimals);
                        let has_lifecycle =
                            later.status_variant.is_some() || later.orig_qty > dec!(0);
                        if let Some(v) = later.status_variant {
                            st.status_variant = Some(v);
                        }
                        if later.orig_qty > dec!(0) {
                            st.orig_qty = later.orig_qty;
                        }
                        let commit_filled = st.filled_qty;
                        let later_filled_portion;
                        if has_lifecycle {
                            st.remaining_qty = later.remaining_qty;
                            if st.orig_qty > st.remaining_qty {
                                st.filled_qty = st.orig_qty - st.remaining_qty;
                            }
                            later_filled_portion = (st.filled_qty - commit_filled).max(dec!(0));
                        } else {
                            st.filled_qty += later.filled_qty;
                            later_filled_portion = later.filled_qty;
                        }
                        st.fee_usd += later.fee_usd;
                        // Blend the two fill windows' VWAPs by quantity —
                        // overwriting with the later VWAP alone would
                        // misstate the blended entry in accounting.
                        if let Some(p2) = later.avg_fill_price {
                            st.avg_fill_price = Some(match st.avg_fill_price {
                                Some(p1) if commit_filled + later_filled_portion > dec!(0) => {
                                    (p1 * commit_filled + p2 * later_filled_portion)
                                        / (commit_filled + later_filled_portion)
                                }
                                _ => p2,
                            });
                        }
                    }
                    Err(e) => {
                        // Scan failure must not downgrade commit-time truth —
                        // the order simply stays at commit state.
                        tracing::debug!("Decibel post-commit scan for {hash} unavailable: {e}");
                    }
                }
            }
        }
        let status = decibel_order_status(
            st.status_variant.as_deref(),
            st.filled_qty,
            st.remaining_qty,
        );
        let orig = if st.orig_qty > dec!(0) {
            st.orig_qty
        } else {
            st.filled_qty + st.remaining_qty
        };
        Ok(OrderResponse {
            order_id: order_id.to_string(),
            client_order_id: client_order_id.unwrap_or_default().to_string(),
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            status,
            original_qty: orig,
            filled_qty: st.filled_qty,
            remaining_qty: st.remaining_qty,
            avg_fill_price: st.avg_fill_price,
            fee: st.fee_usd,
            fee_currency: "USDC".to_string(),
            raw_response: Some(tx.to_string()),
            timestamp: Utc::now(),
        })
    }

    fn order_status_polling_implemented(&self) -> bool {
        true
    }

    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
        let _guard = self.order_lock.lock().await;
        self.trading_key()?;
        let signer = self.wallet_key()?;
        let package = self.deployment_package();
        let engine = perp_engine_global_addr(package)?;
        let market_name = resolve_market_name(&req.symbol)?;
        let market_addr = market_addr_for(&engine, &market_name)?;
        let owner = self.resolve_owner(&self.configured_owner(&signer)).await?;
        let subaccount = primary_subaccount_addr(&owner, package)?;
        let sub_addr = AccountAddress::from_str(&subaccount)
            .map_err(|e| anyhow::anyhow!("Decibel subaccount address invalid: {e}"))?;
        let mkt_addr = AccountAddress::from_str(&market_addr)
            .map_err(|e| anyhow::anyhow!("Decibel market address invalid: {e}"))?;

        // The persisted order id is a committed tx hash (non-numeric); venue
        // order ids remain numeric. A non-numeric id falls through to the
        // client-order-id path rather than erroring.
        let numeric_id = req
            .order_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|id| id.parse::<u128>().ok());
        let payload = if let Some(num) = numeric_id {
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
        if tx
            .get("hash")
            .and_then(|h| h.as_str())
            .unwrap_or("")
            .is_empty()
        {
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
    fn test_decibel_derivable_accounts() {
        // Verified live: the app.decibel.trade SIWE login for this Arbitrum
        // wallet owns primary subaccount 0x00fa32…1cf0 under this derived
        // account (ObjectCore.owner + primary_subaccount() view agree).
        assert_eq!(
            evm_login_account(
                "0x2b9B8dE0bd391F364e595010C4ce70073467b04D",
                SIWA_DOMAIN_MAINNET
            )
            .unwrap(),
            "0xd97b59b49cf58faf09ef2810dbea539a3ed6c949c6efe256dcd2d2f13d776da0"
        );
        // EIP-55 normalization: input case must not change the result.
        assert_eq!(
            evm_login_account(
                "0x2b9b8de0bd391f364e595010c4ce70073467b04d",
                SIWA_DOMAIN_MAINNET
            )
            .unwrap(),
            "0xd97b59b49cf58faf09ef2810dbea539a3ed6c949c6efe256dcd2d2f13d776da0"
        );
        // Official SDK vector (derivable-account.test.ts): domain-scoped.
        assert_eq!(
            evm_login_account(
                "0x55BcEcf60eED4e66CF6515C92B386A993140339f",
                SIWA_DOMAIN_TESTNET
            )
            .unwrap(),
            "0xbb42ddec781413ce1bb7334be5df2061f232ad821da96d41706ab7d0934127d8"
        );
        // Official SDK vector: a Sui login derives a different account.
        assert_eq!(
            sui_login_account(
                "0x7cf81ab1c5752b8f430a2839a4b9034f8133c489cfbfe3f5f541d4d787f87915",
                SIWA_DOMAIN_MAINNET
            )
            .unwrap(),
            "0x686d2ddf7e7c531ce525856816cdc5e6783e3f5830a708f49126cd48413cb1ae"
        );
        // Solana pubkey → a 66-char Aptos address; non-base58 input fails.
        let sol =
            solana_login_account("11111111111111111111111111111111", SIWA_DOMAIN_MAINNET).unwrap();
        assert!(sol.starts_with("0x") && sol.len() == 66);
        assert!(solana_login_account(
            "0x2b9B8dE0bd391F364e595010C4ce70073467b04D",
            SIWA_DOMAIN_MAINNET
        )
        .is_err()); // '0' is not in the base58 alphabet
    }

    #[test]
    fn test_decibel_owner_candidates() {
        // 40-hex EVM wallet → derived Aptos account first, zero-padded
        // reading as last-ditch fallback.
        let c = owner_candidates("0x2b9B8dE0bd391F364e595010C4ce70073467b04D", false);
        assert_eq!(
            c[0],
            "0xd97b59b49cf58faf09ef2810dbea539a3ed6c949c6efe256dcd2d2f13d776da0"
        );
        assert!(c.contains(
            &"0x0000000000000000000000002b9b8de0bd391f364e595010c4ce70073467b04d".to_string()
        ));
        // 64-hex → native Aptos first, then the Sui-login derivation.
        let c = owner_candidates(
            "0x7cf81ab1c5752b8f430a2839a4b9034f8133c489cfbfe3f5f541d4d787f87915",
            false,
        );
        assert_eq!(
            c[0],
            "0x7cf81ab1c5752b8f430a2839a4b9034f8133c489cfbfe3f5f541d4d787f87915"
        );
        assert_eq!(
            c[1],
            "0x686d2ddf7e7c531ce525856816cdc5e6783e3f5830a708f49126cd48413cb1ae"
        );
        // A real Solana pubkey (non-hex letters) → Solana-derived only.
        let c = owner_candidates("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA", false);
        assert_eq!(c.len(), 1);
        assert!(c[0].starts_with("0x") && c[0].len() == 66);
        // Garbage → no candidates.
        assert!(owner_candidates("not-an-address", false).is_empty());
    }

    #[test]
    fn test_decibel_symbol_and_scaling() {
        assert_eq!(resolve_market_name("BTC-PERP").unwrap(), "BTC-PERP");
        assert_eq!(resolve_market_name("btc/usdc").unwrap(), "BTC-PERP");
        assert_eq!(resolve_market_name("ETH-USDC").unwrap(), "ETH-PERP");
        assert_eq!(resolve_market_name("SOL").unwrap(), "SOL-PERP");
        assert!(resolve_market_name("BTC-USDT").is_err());
        assert!(resolve_market_name("BTC-USD").is_err());
        assert!(resolve_market_name("BTC-ETH").is_err());
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
        assert_eq!(
            dec_f64(ov.get("perp_equity_balance")),
            Decimal::from_str_exact("123.5").ok()
        );
        assert_eq!(
            dec_f64(ov.get("usdc_cross_withdrawable_balance")),
            Decimal::from_str_exact("100.25").ok()
        );
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

    #[test]
    fn account_position_for_maps_market_name() {
        let adapter = DecibelAdapter::new(None, None, None);
        let account = AccountSnapshot {
            venue: VenueId::new("decibel"),
            equity: dec!(100),
            free_margin: dec!(100),
            maintenance_margin: dec!(0),
            margin_usage_pct: dec!(0),
            balances: Vec::new(),
            positions: vec![Position {
                venue: VenueId::new("decibel"),
                symbol: "BTC-PERP".to_string(),
                side: Side::Sell,
                quantity: dec!(0.2),
                entry_price: dec!(50000),
                mark_price: dec!(49000),
                unrealized_pnl: dec!(0),
                realized_pnl: dec!(0),
                liquidation_price: None,
                leverage: dec!(1),
                initial_margin: dec!(0),
                maintenance_margin: dec!(0),
                updated_at: Utc::now(),
            }],
            timestamp: Utc::now(),
        };
        let pos = adapter
            .account_position_for(&account, "BTC-USDC")
            .unwrap()
            .expect("BTC-PERP should resolve");
        assert_eq!(pos.symbol, "BTC-USDC");
        assert_eq!(pos.quantity, dec!(0.2));
        assert!(adapter
            .account_position_for(&account, "ETH-USDC")
            .unwrap()
            .is_none());
    }

    #[test]
    fn commit_state_parses_fill_events() {
        let sub = "0xabc";
        let tx = serde_json::json!({
            "hash": "0xdeadbeef",
            "success": true,
            "events": [
                {
                    "type": "0xpkg::perp_positions::OrderEvent",
                    "data": {
                        "user": "0xABC",
                        "order_id": "42",
                        "client_order_id": {"vec": ["cid-1"]},
                        "orig_size": "2000000",
                        "remaining_size": "0",
                        "price": "50000000000",
                        "status": {"__variant__": "FILLED"},
                    }
                },
                {
                    "type": "0xpkg::perp_positions::TradeEvent",
                    "data": {
                        "account": "0xabc",
                        "size": "2000000",
                        "price": "50050000000",
                        "fee": "1001000",
                        "is_taker": true,
                    }
                },
                // Maker's copy of the same fill — must not be counted twice.
                {
                    "type": "0xpkg::perp_positions::TradeEvent",
                    "data": {
                        "account": "0xother",
                        "size": "2000000",
                        "price": "50050000000",
                        "fee": "500",
                    }
                }
            ]
        });
        // px_decimals = 6, sz_decimals = 6 → 2.0 units filled at 50050.0.
        let st = parse_commit_order_state(&tx, sub, 6, 6);
        assert_eq!(st.venue_order_id.as_deref(), Some("42"));
        assert_eq!(st.client_order_id.as_deref(), Some("cid-1"));
        assert_eq!(st.status_variant.as_deref(), Some("FILLED"));
        assert_eq!(st.filled_qty, dec!(2));
        assert_eq!(st.avg_fill_price, Some(dec!(50050)));
        // fee 1,001,000 balance-sheet units → 1.001 USDC.
        assert_eq!(st.fee_usd, dec!(1.001));
        assert_eq!(
            decibel_order_status(st.status_variant.as_deref(), st.filled_qty, dec!(0)),
            OrderStatus::Filled
        );
    }

    #[test]
    fn post_commit_scan_matches_only_our_order() {
        let sub = "0xabc";
        // Later engine tx: our resting order's fill + another order's fill
        // for the same subaccount — only ours must count.
        let later_tx = serde_json::json!({
            "version": "101",
            "events": [
                {
                    "type": "0xpkg::perp_positions::OrderEvent",
                    "data": {
                        "user": "0xABC",
                        "order_id": "42",
                        "client_order_id": {"vec": ["cid-1"]},
                        "orig_size": "2000000",
                        "remaining_size": "0",
                        "status": {"__variant__": "FILLED"},
                    }
                },
                {
                    "type": "0xpkg::perp_positions::TradeEvent",
                    "data": {
                        "account": "0xabc",
                        "order_id": "42",
                        "size": "2000000",
                        "price": "50050000000",
                        "fee": "1001000",
                    }
                },
                // Someone else's order filling in the same engine tx.
                {
                    "type": "0xpkg::perp_positions::OrderEvent",
                    "data": {
                        "user": "0xabc",
                        "order_id": "77",
                        "status": {"__variant__": "FILLED"},
                        "orig_size": "9000000",
                        "remaining_size": "0",
                    }
                },
                {
                    "type": "0xpkg::perp_positions::TradeEvent",
                    "data": {
                        "account": "0xabc",
                        "order_id": "77",
                        "size": "9000000",
                        "price": "50000000000",
                        "fee": "9000000",
                    }
                },
                // Anonymous fill (no order identity) — cannot be safely
                // attributed in a batch tx, must be skipped.
                {
                    "type": "0xpkg::perp_positions::TradeEvent",
                    "data": {
                        "account": "0xabc",
                        "size": "5000000",
                        "price": "1",
                        "fee": "1",
                    }
                }
            ]
        });
        let mut acc = OrderScan::default();
        scan_tx_events(&mut acc, &later_tx, sub, Some("cid-1"), Some("42"), 6, 6);
        let st = finalize_order_scan(&acc, 6, 6);
        assert_eq!(st.status_variant.as_deref(), Some("FILLED"));
        assert_eq!(st.venue_order_id.as_deref(), Some("42"));
        // Only our 2.0-unit fill — the other order's 9.0 and the anonymous
        // 5.0 are excluded.
        assert_eq!(st.filled_qty, dec!(2));
        assert_eq!(st.avg_fill_price, Some(dec!(50050)));
        assert_eq!(st.fee_usd, dec!(1.001));
        // Wrong keys → nothing matches.
        let mut acc2 = OrderScan::default();
        scan_tx_events(&mut acc2, &later_tx, sub, Some("cid-zzz"), None, 6, 6);
        let st2 = finalize_order_scan(&acc2, 6, 6);
        assert!(st2.status_variant.is_none());
        assert_eq!(st2.filled_qty, dec!(0));
    }

    #[test]
    fn commit_state_rejected_and_unknown_scales() {
        let sub = "0xabc";
        // REJECTED order, no fills.
        let tx = serde_json::json!({
            "events": [{
                "type": "0xpkg::perp_positions::OrderEvent",
                "data": {
                    "user": "0xabc",
                    "order_id": "7",
                    "status": {"__variant__": "REJECTED"},
                    "orig_size": "0", "remaining_size": "0", "price": "0"
                }
            }]
        });
        let st = parse_commit_order_state(&tx, sub, 6, 6);
        assert_eq!(st.filled_qty, dec!(0));
        assert_eq!(
            decibel_order_status(
                st.status_variant.as_deref(),
                st.filled_qty,
                st.remaining_qty
            ),
            OrderStatus::Rejected
        );
        // Wrong-scale price (100x the limit) → avg_fill_price withheld.
        let tx2 = serde_json::json!({
            "events": [
                {
                    "type": "0xpkg::perp_positions::OrderEvent",
                    "data": {
                        "user": "0xabc",
                        "order_id": "9",
                        "status": {"__variant__": "FILLED"},
                        "orig_size": "1000000", "remaining_size": "0",
                        "price": "50000000000"
                    }
                },
                {
                    "type": "0xpkg::perp_positions::TradeEvent",
                    "data": {
                        "account": "0xabc",
                        "size": "1000000",
                        "price": "5000000000000",
                        "fee": "0"
                    }
                }
            ]
        });
        let st2 = parse_commit_order_state(&tx2, sub, 6, 6);
        assert_eq!(st2.filled_qty, dec!(1));
        assert!(st2.avg_fill_price.is_none());
    }

    #[test]
    fn commit_state_partial_then_cancelled() {
        let sub = "0xabc";
        let tx = serde_json::json!({
            "events": [{
                "type": "0xpkg::perp_positions::OrderEvent",
                "data": {
                    "user": "0xabc",
                    "order_id": "11",
                    "status": {"__variant__": "CANCELLED"},
                    "orig_size": "4000000", "remaining_size": "3000000",
                    "price": "50000000000"
                }
            }]
        });
        let st = parse_commit_order_state(&tx, sub, 6, 6);
        assert_eq!(st.filled_qty, dec!(1));
        assert_eq!(
            decibel_order_status(
                st.status_variant.as_deref(),
                st.filled_qty,
                st.remaining_qty
            ),
            OrderStatus::Cancelled
        );
    }
}
