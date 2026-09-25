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
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha3::{Digest, Keccak256};
use std::collections::HashMap;
use std::str::FromStr;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HlOrderWire {
    pub a: u32,
    pub b: bool,
    pub p: String,
    pub s: String,
    pub r: bool,
    pub t: HlOrderTypeWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub c: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HlOrderTypeWire {
    pub limit: HlLimitWire,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HlLimitWire {
    pub tif: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HlCancelWire {
    pub a: u32,
    pub o: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum HlActionWire {
    Order {
        orders: Vec<HlOrderWire>,
        grouping: String,
    },
    Cancel {
        cancels: Vec<HlCancelWire>,
    },
}

fn dec_from_json(v: Option<&serde_json::Value>) -> Option<Decimal> {
    match v? {
        serde_json::Value::String(s) => Decimal::from_str(s).ok(),
        serde_json::Value::Number(n) => Decimal::from_str(&n.to_string()).ok(),
        _ => None,
    }
}

fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(data);
    hasher.finalize().into()
}

/// The exact EIP-712 digest a Hyperliquid L1 action signs — extracted so
/// tests can ecrecover our signatures and verify round-trip correctness.
pub fn hyperliquid_l1_digest<T: Serialize>(
    action: &T,
    nonce: u64,
    vault_address: Option<&[u8; 20]>,
    is_mainnet: bool,
) -> anyhow::Result<[u8; 32]> {
    let action_bytes = rmp_serde::to_vec_named(action)
        .map_err(|e| anyhow::anyhow!("Failed to serialize action with MessagePack: {e}"))?;
    let mut hash_input = action_bytes;
    hash_input.extend_from_slice(&nonce.to_be_bytes());
    match vault_address {
        Some(addr) => {
            hash_input.push(1);
            hash_input.extend_from_slice(addr);
        }
        None => {
            hash_input.push(0);
        }
    }
    let connection_id = keccak256(&hash_input);

    // EIP-712 Domain Separator:
    // EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)
    let domain_typehash = keccak256(
        b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
    );
    let name_hash = keccak256(b"Exchange");
    let version_hash = keccak256(b"1");
    let mut chain_id_bytes = [0u8; 32];
    chain_id_bytes[30] = 0x05;
    chain_id_bytes[31] = 0x39; // 1337
    let verifying_contract_bytes = [0u8; 32];

    let mut domain_buf = Vec::with_capacity(32 * 5);
    domain_buf.extend_from_slice(&domain_typehash);
    domain_buf.extend_from_slice(&name_hash);
    domain_buf.extend_from_slice(&version_hash);
    domain_buf.extend_from_slice(&chain_id_bytes);
    domain_buf.extend_from_slice(&verifying_contract_bytes);
    let domain_separator = keccak256(&domain_buf);

    // Agent Struct Hash:
    // Agent(string source,bytes32 connectionId)
    let agent_typehash = keccak256(b"Agent(string source,bytes32 connectionId)");
    let source_hash = if is_mainnet {
        keccak256(b"a")
    } else {
        keccak256(b"b")
    };

    let mut struct_buf = Vec::with_capacity(32 * 3);
    struct_buf.extend_from_slice(&agent_typehash);
    struct_buf.extend_from_slice(&source_hash);
    struct_buf.extend_from_slice(&connection_id);
    let struct_hash = keccak256(&struct_buf);

    // EIP-712 Digest: \x19\x01 || domain_separator || struct_hash
    let mut digest_buf = Vec::with_capacity(2 + 32 + 32);
    digest_buf.extend_from_slice(b"\x19\x01");
    digest_buf.extend_from_slice(&domain_separator);
    digest_buf.extend_from_slice(&struct_hash);
    Ok(keccak256(&digest_buf))
}

/// Computes the authentic EIP-712 signature for a Hyperliquid L1 action.
pub fn sign_hyperliquid_l1_action<T: Serialize>(
    action: &T,
    nonce: u64,
    vault_address: Option<&[u8; 20]>,
    is_mainnet: bool,
    private_key_hex: &str,
) -> anyhow::Result<(String, String, u8)> {
    let digest = hyperliquid_l1_digest(action, nonce, vault_address, is_mainnet)?;

    let key_clean = private_key_hex
        .trim()
        .strip_prefix("0x")
        .unwrap_or(private_key_hex.trim());
    let key_bytes =
        hex::decode(key_clean).map_err(|e| anyhow::anyhow!("Invalid private key hex: {e}"))?;
    if key_bytes.len() != 32 {
        anyhow::bail!(
            "Invalid private key length: expected 32 bytes, got {}",
            key_bytes.len()
        );
    }
    let key_arr: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("Failed to convert key bytes"))?;
    let signing_key = k256::ecdsa::SigningKey::from_bytes((&key_arr).into())
        .map_err(|e| anyhow::anyhow!("Failed to instantiate secp256k1 signing key: {e}"))?;
    let (sig, recid) = signing_key.sign_prehash_recoverable(&digest);

    let r_hex = format!("0x{}", hex::encode(sig.r().to_bytes()));
    let s_hex = format!("0x{}", hex::encode(sig.s().to_bytes()));
    let v = recid.to_byte() + 27;

    Ok((r_hex, s_hex, v))
}

/// How long a successful `userRole` answer stays authoritative — API
/// wallets can expire or be deregistered, so the cache must not outlive
/// the venue's ability to change its mind during multi-day operation.
const SIGNER_VERIFICATION_TTL: Duration = Duration::from_secs(300);

/// A signer identity verified via `userRole` together with the configured
/// account it was verified against — an approval for one account must never
/// be reused to skip the check after the account changes.
#[derive(Clone)]
struct VerifiedSigner {
    signer: String,
    account: String,
    verified_at: Instant,
}

/// Hyperliquid L1 Perpetual and Spot exchange adapter.
#[allow(dead_code)]
pub struct HyperliquidAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    wallet_address: RwLock<Option<String>>,
    private_key: RwLock<Option<Zeroizing<String>>>,
    /// Signer+account pair verified via `userRole` for the currently-held
    /// credentials; cleared whenever credentials actually change.
    verified_signer: RwLock<Option<VerifiedSigner>>,
    /// Which EIP-712 domain the L1 action signs. Constructor-pinned —
    /// never re-inferred from the base URL at order time, so a mistyped
    /// endpoint can only fail at the venue, not sign the wrong domain.
    is_mainnet: bool,
    rate_limiter: AccountRequestCoordinator,
}

impl HyperliquidAdapter {
    pub fn new(
        base_url: Option<String>,
        wallet_address: Option<String>,
        private_key: Option<String>,
    ) -> Self {
        let base = base_url.unwrap_or_else(|| "https://api.hyperliquid.xyz".to_string());
        Self {
            venue_id: VenueId::new("hyperliquid"),
            is_mainnet: true,
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            base_url: base,
            wallet_address: RwLock::new(wallet_address),
            private_key: RwLock::new(private_key.map(Zeroizing::new)),
            verified_signer: RwLock::new(None),
            rate_limiter: AccountRequestCoordinator::new("hyperliquid", 100, 2000, 500, 10000),
        }
    }

    /// Public testnet adapter — signs the testnet EIP-712 domain.
    pub fn for_testnet(wallet_address: Option<String>, private_key: Option<String>) -> Self {
        let mut adapter = Self::new(
            Some("https://api.hyperliquid-testnet.xyz".to_string()),
            wallet_address,
            private_key,
        );
        adapter.is_mainnet = false;
        adapter
    }

    /// Update live trading credentials dynamically (e.g. on vault unlock/lock).
    /// A no-op when nothing changed — the periodic credential sync must not
    /// wipe the verified-signer preflight cache on every pass.
    pub fn update_credentials(&self, wallet_address: Option<String>, private_key: Option<String>) {
        let mut wallet_guard = self.wallet_address.write();
        let mut key_guard = self.private_key.write();
        let unchanged = wallet_guard.as_deref() == wallet_address.as_deref()
            && key_guard.as_ref().map(|k| k.as_str()) == private_key.as_deref();
        if unchanged {
            return;
        }
        *wallet_guard = wallet_address;
        *key_guard = private_key.map(Zeroizing::new);
        *self.verified_signer.write() = None;
    }

    /// Whether valid, non-empty API credentials are held in memory.
    pub fn has_credentials(&self) -> bool {
        let w = self.wallet_address.read();
        let p = self.private_key.read();
        w.as_ref().is_some_and(|w| !w.trim().is_empty())
            && p.as_ref().is_some_and(|p| !p.trim().is_empty())
    }

    /// The EIP-55 address the currently-held private key derives to — the
    /// exact identity Hyperliquid ecrecovers from our signatures. `None`
    /// when no usable key is held. Never exposes key material.
    pub fn active_signer_address(&self) -> Option<String> {
        let guard = self.private_key.read();
        let key = guard.as_deref()?;
        let key = key.trim();
        if key.is_empty() {
            return None;
        }
        crate::wallet::EvmWalletSigner::from_hex(key)
            .ok()
            .map(|s| s.address_checksum())
    }

    /// Configured trading account (master wallet / vault the orders target).
    pub fn configured_wallet_address(&self) -> Option<String> {
        self.wallet_address.read().clone()
    }

    /// Whether a fresh `userRole` verification is cached for this exact
    /// (signer, account) pair — expiry forces a re-query because API
    /// wallets can be deregistered between orders.
    fn has_fresh_signer_verification(&self, signer: &str, configured_account: &str) -> bool {
        self.verified_signer.read().as_ref().is_some_and(|v| {
            v.signer.eq_ignore_ascii_case(signer)
                && v.account.eq_ignore_ascii_case(configured_account)
                && v.verified_at.elapsed() < SIGNER_VERIFICATION_TTL
        })
    }

    /// Pre-flight identity check: query `userRole` for the derived signer
    /// and refuse before submission when the key isn't a valid signer for
    /// the configured account. Answers are cached per (signer, account)
    /// pair for SIGNER_VERIFICATION_TTL; a transport/HTTP failure fails
    /// closed — submitting anyway would burn a nonce and strand order state.
    async fn verify_signer_registered(
        &self,
        signer: &str,
        configured_account: &str,
    ) -> anyhow::Result<()> {
        if self.has_fresh_signer_verification(signer, configured_account) {
            return Ok(());
        }
        let role_resp = self
            .post_info(json!({"type": "userRole", "user": signer}))
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "Hyperliquid: could not verify signer {signer} against account {configured_account} ({e}); no order submitted"
                )
            })?;
        validate_signer_role_response(&role_resp, signer, configured_account)?;
        *self.verified_signer.write() = Some(VerifiedSigner {
            signer: signer.to_string(),
            account: configured_account.to_string(),
            verified_at: Instant::now(),
        });
        Ok(())
    }

    async fn post_info(&self, body: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let url = format!("{}/info", self.base_url);
        let resp = self.client.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Hyperliquid /info error status: {}", resp.status());
        }
        let json_val = resp.json::<serde_json::Value>().await?;
        Ok(json_val)
    }

    async fn post_exchange(&self, body: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        self.rate_limiter.acquire_signed().await?;
        let url = format!("{}/exchange", self.base_url);
        let resp = match self.client.post(&url).json(&body).send().await {
            Ok(r) => r,
            Err(e) => {
                self.rate_limiter.record_failure(false);
                return Err(e.into());
            }
        };
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            self.rate_limiter
                .record_failure(status == reqwest::StatusCode::TOO_MANY_REQUESTS);
            anyhow::bail!("Hyperliquid /exchange error HTTP {status}: {text}");
        }
        let json_val: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            anyhow::anyhow!("Failed to parse Hyperliquid exchange response: {e}; body: {text}")
        })?;
        Ok(json_val)
    }

    /// Resolve coin name to (asset index, szDecimals) in the Hyperliquid
    /// perpetual universe — both needed to wire-format order fields.
    pub async fn resolve_asset_meta(&self, coin: &str) -> anyhow::Result<(u32, u32)> {
        let res = self.post_info(json!({"type": "meta"})).await?;
        let universe = res
            .get("universe")
            .and_then(|u| u.as_array())
            .ok_or_else(|| {
                anyhow::anyhow!("Missing universe array in Hyperliquid meta response")
            })?;
        for (idx, item) in universe.iter().enumerate() {
            if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                if name.eq_ignore_ascii_case(coin) {
                    let sz_decimals =
                        item.get("szDecimals").and_then(|s| s.as_u64()).unwrap_or(3) as u32;
                    return Ok((idx as u32, sz_decimals));
                }
            }
        }
        anyhow::bail!("Asset '{coin}' not found in Hyperliquid perpetual universe");
    }

    pub async fn resolve_asset_index(&self, coin: &str) -> anyhow::Result<u32> {
        Ok(self.resolve_asset_meta(coin).await?.0)
    }
}

/// Hyperliquid wire price rules ("Tick and lot size"): perp prices allow at
/// most 5 significant figures and at most `6 - szDecimals` decimal places;
/// integer prices always pass. The rounding direction never crosses the
/// caller's protective bound — buy caps floor (pay less), sell caps ceil
/// (receive more) — by at most one tick.
/// Canonical decimal wire string — Hyperliquid re-serializes parsed `p`/`s`
/// values before hashing the action, so the bytes we hash must already be in
/// their minimal form ("0.005", not "0.0050"; "2740", not "2740.0"). A
/// padded string signs a connection_id HL can never reproduce, and their
/// ecrecover lands on a different unregistered address per nonce — the
/// "User or API Wallet <random> does not exist" rejection.
fn hl_wire_decimal(d: Decimal) -> String {
    d.normalize().to_string()
}

fn hl_wire_price(price: Decimal, sz_decimals: u32, side: Side) -> Decimal {
    if price <= dec!(0) || price.fract().is_zero() {
        return price; // invalid or already integral — pass through
    }
    let f: f64 = price.to_string().parse().unwrap_or(0.0);
    if !f.is_finite() || f <= 0.0 {
        return price;
    }
    let digits_before = f.log10().floor() as i32 + 1;
    let sig_decimals = (5 - digits_before).max(0) as u32;
    let decimals = sig_decimals.min(6u32.saturating_sub(sz_decimals));
    let truncated = price.trunc_with_scale(decimals);
    if side == Side::Sell && truncated < price {
        truncated + Decimal::new(1, decimals)
    } else {
        truncated
    }
}

/// Append REST/WS `levels` ([bids, asks] of `{px, sz}` string entries)
/// onto a book. Malformed levels are skipped, never fabricated.
pub fn append_hyperliquid_levels(book: &mut OrderBook, levels: &serde_json::Value) {
    let Some(arr) = levels.as_array() else {
        return;
    };
    if arr.len() < 2 {
        return;
    }
    if let Some(bids) = arr[0].as_array() {
        for b in bids {
            if let (Some(px), Some(sz)) = (
                b.get("px").and_then(|p| p.as_str()),
                b.get("sz").and_then(|s| s.as_str()),
            ) {
                if let (Ok(p), Ok(s)) = (Decimal::from_str(px), Decimal::from_str(sz)) {
                    book.bids.push(BookLevel::new(p, s));
                }
            }
        }
    }
    if let Some(asks) = arr[1].as_array() {
        for a in asks {
            if let (Some(px), Some(sz)) = (
                a.get("px").and_then(|p| p.as_str()),
                a.get("sz").and_then(|s| s.as_str()),
            ) {
                if let (Ok(p), Ok(s)) = (Decimal::from_str(px), Decimal::from_str(sz)) {
                    book.asks.push(BookLevel::new(p, s));
                }
            }
        }
    }
}
/// Perp symbol normalization: Hyperliquid perp endpoints take the bare coin
/// ("BTC-USDC" -> "BTC"). A `-SPOT` market-type suffix must fail closed —
/// answering a spot query with the perp book would misattribute the market.
fn perp_coin(symbol: &str) -> anyhow::Result<String> {
    let clean = symbol.trim().replace('/', "-");
    if clean.to_uppercase().ends_with("-SPOT") {
        anyhow::bail!(
            "Hyperliquid spot instrument '{symbol}' needs spot metadata this adapter does not implement; refusing to return perp data"
        );
    }
    let parts: Vec<&str> = clean.split(['-', '_']).collect();
    if parts.len() >= 2 {
        let quote = parts[1];
        if !quote.eq_ignore_ascii_case("USDC") && !quote.eq_ignore_ascii_case("PERP") {
            anyhow::bail!(
                "Hyperliquid perpetual markets are quoted in USDC; '{symbol}' is not supported"
            );
        }
    }
    let base = parts.first().copied().unwrap_or(clean.as_str());
    if base.is_empty() {
        anyhow::bail!("Hyperliquid symbol '{symbol}' has empty coin");
    }
    Ok(base.to_string())
}

/// Pure validation of a Hyperliquid `userRole` info response for the
/// unified-account trading path. A `user` role is valid only when the
/// signer IS the configured account; an `agent` is valid only when
/// `data.user` binds it to the configured account. `missing`, `vault`,
/// `subAccount`, malformed payloads, and unknown roles all fail closed —
/// nothing here may pass on ambiguity. Address comparison is
/// case-insensitive (EIP-55 checksum vs lowercase storage).
fn validate_signer_role_response(
    response: &serde_json::Value,
    signer: &str,
    configured_account: &str,
) -> anyhow::Result<()> {
    if signer.trim().is_empty() || configured_account.trim().is_empty() {
        anyhow::bail!(
            "Hyperliquid signer verification requires non-empty signer and configured account; no order submitted"
        );
    }
    let role = response
        .get("role")
        .and_then(|r| r.as_str())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Hyperliquid userRole response for {signer} is malformed or has no role; no order submitted"
            )
        })?;
    match role {
        // The held key IS the configured account — a direct signer is
        // valid only when it would trade the configured account itself.
        "user" => {
            if signer.eq_ignore_ascii_case(configured_account) {
                Ok(())
            } else {
                anyhow::bail!(
                    "Hyperliquid signer {signer} is a direct account key, but not for configured account {configured_account} — this key would trade its own account instead; no order submitted"
                )
            }
        }
        // A registered API wallet (agent) — valid only when bound to the
        // configured account.
        "agent" => {
            let bound = response
                .get("data")
                .and_then(|d| d.get("user"))
                .and_then(|u| u.as_str())
                .unwrap_or("");
            if bound.is_empty() {
                anyhow::bail!(
                    "Hyperliquid userRole for agent {signer} carries no bound account (data.user); no order submitted"
                );
            }
            if bound.eq_ignore_ascii_case(configured_account) {
                Ok(())
            } else {
                anyhow::bail!(
                    "Hyperliquid agent {signer} is bound to account {bound}, not configured account {configured_account}; no order submitted"
                )
            }
        }
        "missing" => anyhow::bail!(
            "Hyperliquid signer {signer} is not an approved API wallet for configured account {configured_account} — approve it at app.hyperliquid.xyz/API; no order submitted"
        ),
        other => anyhow::bail!(
            "Hyperliquid signer {signer} has role '{other}', not a valid signer role for unified-account trading on configured account {configured_account}; no order submitted"
        ),
    }
}

/// Strict pure parser for the Hyperliquid `/exchange` order response.
///
/// Never synthesizes venue data: every field of the returned
/// `OrderResponse` comes from the venue payload or the original request.
/// Any missing/malformed `oid`, `totalSz`, `avgPx`, or unknown status shape
/// is a hard `Err` (fail closed).
fn parse_hl_order_response(
    resp_json: &serde_json::Value,
    order: &arbitrageur_core::order::OrderRequest,
    venue: &arbitrageur_core::types::VenueId,
    _selected_price: Decimal,
) -> anyhow::Result<arbitrageur_core::order::OrderResponse> {
    if resp_json.get("status").and_then(|s| s.as_str()) == Some("err") {
        anyhow::bail!(
            "Hyperliquid order error: {}",
            resp_json.get("response").unwrap_or(resp_json)
        );
    }

    let status_obj = resp_json
        .get("response")
        .and_then(|r| r.get("data"))
        .and_then(|d| d.get("statuses"))
        .and_then(|s| s.as_array())
        .and_then(|a| a.first())
        .ok_or_else(|| {
            anyhow::anyhow!("Hyperliquid order response has unknown shape: {resp_json}")
        })?;

    if let Some(err_msg) = status_obj.get("error").and_then(|e| e.as_str()) {
        anyhow::bail!("Hyperliquid order rejected: {err_msg}");
    }

    if let Some(filled) = status_obj.get("filled") {
        let oid = filled
            .get("oid")
            .and_then(|o| o.as_u64())
            .filter(|o| *o > 0)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Hyperliquid filled response missing positive integer oid: {resp_json}"
                )
            })?;
        let total_sz_str = filled
            .get("totalSz")
            .and_then(|s| s.as_str())
            .ok_or_else(|| {
                anyhow::anyhow!("Hyperliquid filled response missing totalSz: {resp_json}")
            })?;
        let total_sz = Decimal::from_str(total_sz_str).map_err(|e| {
            anyhow::anyhow!("Hyperliquid filled totalSz '{total_sz_str}' is not a Decimal: {e}")
        })?;
        if total_sz <= dec!(0) {
            anyhow::bail!("Hyperliquid filled totalSz must be > 0: {resp_json}");
        }
        let avg_px_str = filled
            .get("avgPx")
            .and_then(|s| s.as_str())
            .ok_or_else(|| {
                anyhow::anyhow!("Hyperliquid filled response missing avgPx: {resp_json}")
            })?;
        let avg_px = Decimal::from_str(avg_px_str).map_err(|e| {
            anyhow::anyhow!("Hyperliquid filled avgPx '{avg_px_str}' is not a Decimal: {e}")
        })?;
        if avg_px <= dec!(0) {
            anyhow::bail!("Hyperliquid filled avgPx must be > 0: {resp_json}");
        }
        if total_sz > order.quantity {
            anyhow::bail!(
                "Hyperliquid filled totalSz {total_sz} exceeds order quantity {}: {resp_json}",
                order.quantity
            );
        }
        let (status, remaining_qty) = if total_sz == order.quantity {
            (OrderStatus::Filled, dec!(0))
        } else {
            (OrderStatus::PartiallyFilled, order.quantity - total_sz)
        };
        return Ok(OrderResponse {
            order_id: oid.to_string(),
            client_order_id: order.client_order_id.clone(),
            venue: venue.clone(),
            symbol: order.symbol.clone(),
            status,
            original_qty: order.quantity,
            filled_qty: total_sz,
            remaining_qty,
            avg_fill_price: Some(avg_px),
            fee: None,
            fee_currency: "USDC".to_string(),
            raw_response: Some(resp_json.to_string()),
            timestamp: Utc::now(),
        });
    }

    if let Some(resting) = status_obj.get("resting") {
        let oid = resting
            .get("oid")
            .and_then(|o| o.as_u64())
            .filter(|o| *o > 0)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Hyperliquid resting response missing positive integer oid: {resp_json}"
                )
            })?;
        return Ok(OrderResponse {
            order_id: oid.to_string(),
            client_order_id: order.client_order_id.clone(),
            venue: venue.clone(),
            symbol: order.symbol.clone(),
            status: OrderStatus::New,
            original_qty: order.quantity,
            filled_qty: dec!(0),
            remaining_qty: order.quantity,
            avg_fill_price: None,
            fee: None,
            fee_currency: "USDC".to_string(),
            raw_response: Some(resp_json.to_string()),
            timestamp: Utc::now(),
        });
    }

    anyhow::bail!("Hyperliquid order response has unknown shape: {resp_json}")
}

#[async_trait]
impl ExchangeAdapter for HyperliquidAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Hyperliquid"
    }

    fn rest_base_url(&self) -> Option<String> {
        Some(self.base_url.clone())
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let res = self.post_info(json!({"type": "meta"})).await?;
        let universe = res
            .get("universe")
            .and_then(|u| u.as_array())
            .ok_or_else(|| {
                anyhow::anyhow!("Missing universe array in Hyperliquid meta response")
            })?;

        let mut markets = Vec::new();
        for item in universe {
            if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                let sz_decimals =
                    item.get("szDecimals").and_then(|s| s.as_u64()).unwrap_or(3) as u32;
                let symbol = Symbol::perp(name, "USDC");
                markets.push(MarketMetadata {
                    symbol,
                    venue: self.venue_id.clone(),
                    venue_symbol: name.to_string(),
                    price_decimals: 5,
                    size_decimals: sz_decimals,
                    min_order_size: dec!(0.001),
                    min_notional: dec!(10.0),
                    maker_fee_bps: dec!(1.5), // standard-tier maker 0.015%
                    taker_fee_bps: dec!(4.5), // standard-tier taker 0.045%
                    contract_size: dec!(1.0),
                    is_active: true,
                });
            }
        }
        Ok(markets)
    }

    async fn fetch_market_prices(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        let res = self.post_info(json!({"type": "metaAndAssetCtxs"})).await?;
        let arr = res.as_array().ok_or_else(|| {
            anyhow::anyhow!("Hyperliquid metaAndAssetCtxs response is not an array")
        })?;
        if arr.len() < 2 {
            anyhow::bail!("Hyperliquid metaAndAssetCtxs response is incomplete");
        }
        let universe = arr[0]
            .get("universe")
            .and_then(|u| u.as_array())
            .ok_or_else(|| anyhow::anyhow!("Hyperliquid metaAndAssetCtxs missing universe"))?;
        let contexts = arr[1].as_array().ok_or_else(|| {
            anyhow::anyhow!("Hyperliquid metaAndAssetCtxs missing asset contexts")
        })?;

        let mut out = HashMap::new();
        for (index, item) in universe.iter().enumerate() {
            let Some(name) = item.get("name").and_then(|n| n.as_str()) else {
                continue;
            };
            let Some(mark) = contexts
                .get(index)
                .and_then(|ctx| ctx.get("markPx"))
                .and_then(|value| {
                    value
                        .as_str()
                        .and_then(|s| Decimal::from_str(s).ok())
                        .or_else(|| value.as_f64().and_then(Decimal::from_f64_retain))
                })
            else {
                continue;
            };
            if mark > dec!(0) {
                out.insert(name.to_string(), mark);
            }
        }
        Ok(out)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let coin = perp_coin(symbol)?;
        let res = self
            .post_info(json!({"type": "l2Book", "coin": coin}))
            .await?;

        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());
        if let Some(levels) = res.get("levels") {
            append_hyperliquid_levels(&mut book, levels);
        }
        book.timestamp = Utc::now();
        Ok(book)
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let book = self.fetch_orderbook(symbol).await?;
        let bid = book.best_bid_price().unwrap_or(dec!(0));
        let ask = book.best_ask_price().unwrap_or(dec!(0));
        let last = (bid + ask) / dec!(2);

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
        let coin = perp_coin(symbol)?;
        self.fetch_funding_rates()
            .await?
            .into_iter()
            .find(|r| {
                r.symbol
                    .split('-')
                    .next()
                    .map(|b| b.eq_ignore_ascii_case(&coin))
                    .unwrap_or(false)
            })
            .ok_or_else(|| anyhow::anyhow!("Funding rate not found for coin: {}", coin))
    }

    /// `metaAndAssetCtxs` returns every perp's funding in one call — far
    /// cheaper than per-symbol queries for the monitor's venue sweep.
    async fn fetch_funding_rates(&self) -> anyhow::Result<Vec<FundingRate>> {
        let res = self.post_info(json!({"type": "metaAndAssetCtxs"})).await?;

        // Format is [meta, [assetCtx, ...]]
        if let Some(arr) = res.as_array() {
            if arr.len() >= 2 {
                let universe = arr[0].get("universe").and_then(|u| u.as_array());
                let contexts = arr[1].as_array();

                if let (Some(uni), Some(ctxs)) = (universe, contexts) {
                    let mut out = Vec::new();
                    for (i, item) in uni.iter().enumerate() {
                        if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                            if let Some(ctx) = ctxs.get(i) {
                                let rate_str = ctx.get("funding").and_then(|f| f.as_str());
                                let rate_opt = rate_str.and_then(|s| Decimal::from_str(s).ok());
                                let Some(rate) = rate_opt else { continue };
                                out.push(FundingRate {
                                    venue: self.venue_id.clone(),
                                    symbol: Symbol::perp(name, "USDC").canonical(),
                                    rate,
                                    predicted_rate: None,
                                    interval_hours: 1, // Hyperliquid hourly continuous funding
                                    next_funding_time: Utc::now(),
                                    timestamp: Utc::now(),
                                });
                            }
                        }
                    }
                    return Ok(out);
                }
            }
        }
        anyhow::bail!("metaAndAssetCtxs response missing universe/contexts")
    }

    async fn fetch_volumes_24h(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        let res = self.post_info(json!({"type": "metaAndAssetCtxs"})).await?;

        let arr = res
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Unexpected metaAndAssetCtxs response"))?;
        if arr.len() < 2 {
            anyhow::bail!("metaAndAssetCtxs response missing universe/contexts");
        }
        let universe = arr[0]
            .get("universe")
            .and_then(|u| u.as_array())
            .ok_or_else(|| anyhow::anyhow!("metaAndAssetCtxs missing universe"))?;
        let contexts = arr[1]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("metaAndAssetCtxs missing asset contexts"))?;

        let mut volumes = HashMap::new();
        for (i, item) in universe.iter().enumerate() {
            let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if name.is_empty() {
                continue;
            }
            if let Some(ctx) = contexts.get(i) {
                if let Some(vol_str) = ctx.get("dayNtlVlm").and_then(|v| v.as_str()) {
                    if let Ok(vol) = Decimal::from_str(vol_str) {
                        // Canonical BASE-QUOTE keys match what hub consumers
                        // look up (perp names such as kPEPE stay verbatim).
                        volumes.insert(format!("{}-USDC", name.to_uppercase()), vol);
                    }
                }
            }
        }
        Ok(volumes)
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        let user = {
            let guard = self.wallet_address.read();
            guard
                .as_deref()
                .map(|k| k.trim())
                .filter(|v| !v.is_empty())
                .map(ToString::to_string)
        }
        .ok_or_else(|| anyhow::anyhow!("Hyperliquid account fetch requires a wallet address"))?;
        let res = self
            .post_info(json!({"type": "clearinghouseState", "user": user}))
            .await?;

        let margin_summary = res.get("marginSummary");
        let account_value = margin_summary
            .and_then(|m| m.get("accountValue"))
            .and_then(|v| v.as_str())
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(dec!(0));

        let total_margin_used = margin_summary
            .and_then(|m| m.get("totalMarginUsed"))
            .and_then(|v| v.as_str())
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(dec!(0));

        let free_margin = (account_value - total_margin_used).max(dec!(0));

        // Perp-only `clearinghouseState` misses funds sitting on the spot side
        // (a funded wallet then reads $0.00). Best-effort add spot value:
        // USDC counts at face, other tokens at entry notional (~value).
        // A spot-read failure keeps perp-only figures (today's behavior).
        let mut spot_value = dec!(0);
        match self
            .post_info(json!({"type": "spotClearinghouseState", "user": user}))
            .await
        {
            Ok(spot) => {
                if let Some(bals) = spot.get("balances").and_then(|b| b.as_array()) {
                    for b in bals {
                        let coin = b.get("coin").and_then(|c| c.as_str()).unwrap_or("");
                        let num = |key: &str| {
                            b.get(key)
                                .and_then(|v| v.as_str())
                                .and_then(|s| Decimal::from_str(s).ok())
                                .unwrap_or(dec!(0))
                        };
                        let v = if coin.eq_ignore_ascii_case("USDC") {
                            num("total")
                        } else {
                            let ntl = num("entryNtl");
                            if ntl > dec!(0) {
                                ntl
                            } else {
                                num("total")
                            }
                        };
                        if v > dec!(0) {
                            spot_value += v;
                        }
                    }
                }
            }
            Err(e) => {
                tracing::debug!("Hyperliquid spot balance read failed for {user}: {e}");
            }
        }
        let account_value = account_value + spot_value;
        let free_margin = free_margin + spot_value;

        let mut positions = Vec::new();
        if let Some(pos_arr) = res.get("assetPositions").and_then(|p| p.as_array()) {
            for p in pos_arr {
                if let Some(pos_obj) = p.get("position") {
                    let coin = pos_obj.get("coin").and_then(|c| c.as_str()).unwrap_or("");
                    let szi_str = pos_obj.get("szi").and_then(|s| s.as_str()).unwrap_or("0");
                    let entry_str = pos_obj
                        .get("entryPx")
                        .and_then(|e| e.as_str())
                        .unwrap_or("0");

                    let szi = Decimal::from_str(szi_str).unwrap_or(dec!(0));
                    let entry_px = Decimal::from_str(entry_str).unwrap_or(dec!(0));

                    if szi != dec!(0) {
                        let side = if szi > dec!(0) { Side::Buy } else { Side::Sell };
                        let qty = szi.abs();
                        let parse_str = |key: &str| {
                            pos_obj
                                .get(key)
                                .and_then(|v| v.as_str())
                                .and_then(|s| Decimal::from_str(s).ok())
                        };
                        let position_value = parse_str("positionValue").unwrap_or(dec!(0));
                        // Mark-implied price from real notional; entry as last-known real price.
                        let mark_px = if position_value > dec!(0) {
                            position_value / qty
                        } else {
                            entry_px
                        };
                        let margin_used = parse_str("marginUsed").unwrap_or(dec!(0));
                        let max_leverage = pos_obj
                            .get("maxLeverage")
                            .and_then(|v| v.as_f64())
                            .and_then(Decimal::from_f64_retain)
                            .unwrap_or(dec!(0));
                        // HL maintenance margin = half the initial margin at max leverage.
                        let maintenance = if max_leverage > dec!(0) {
                            position_value / (dec!(2) * max_leverage)
                        } else {
                            margin_used
                        };
                        positions.push(Position {
                            venue: self.venue_id.clone(),
                            symbol: format!("{}-USDC", coin),
                            side,
                            quantity: qty,
                            entry_price: entry_px,
                            mark_price: mark_px,
                            unrealized_pnl: parse_str("unrealizedPnl").unwrap_or(dec!(0)),
                            realized_pnl: dec!(0),
                            liquidation_price: parse_str("liquidationPx").filter(|p| *p > dec!(0)),
                            leverage: pos_obj
                                .get("leverage")
                                .and_then(|l| l.get("value"))
                                .and_then(|v| v.as_f64())
                                .and_then(Decimal::from_f64_retain)
                                .unwrap_or(dec!(0)),
                            initial_margin: margin_used,
                            maintenance_margin: maintenance,
                            updated_at: Utc::now(),
                        });
                    }
                }
            }
        }

        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity: account_value,
            free_margin,
            maintenance_margin: total_margin_used,
            margin_usage_pct: if account_value > dec!(0) {
                (total_margin_used / account_value) * dec!(100)
            } else {
                dec!(0)
            },
            balances: vec![Balance {
                venue: self.venue_id.clone(),
                currency: "USDC".to_string(),
                total: account_value,
                free: free_margin,
                locked: total_margin_used,
            }],
            positions,
            timestamp: Utc::now(),
        })
    }

    fn normalize_account_position(&self, position: &Position) -> anyhow::Result<Option<Position>> {
        let mut position = position.clone();
        let coin = perp_coin(&position.symbol)?;
        position.symbol = format!("{}-USDC", coin.to_uppercase());
        Ok(Some(position))
    }

    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        // Snapshot credentials once: signer derivation, preflight, and
        // signing must all come from the SAME key/wallet pair — credentials
        // can be re-provisioned concurrently, and mixing snapshots could
        // verify one identity while signing with another.
        let private_key = {
            let guard = self.private_key.read();
            guard
                .as_deref()
                .map(|k| k.trim())
                .filter(|k| !k.is_empty())
                .map(|k| Zeroizing::new(k.to_string()))
        }
        .ok_or_else(|| {
            anyhow::anyhow!("Hyperliquid live order placement requires a private key")
        })?;
        let configured_wallet = self
            .configured_wallet_address()
            .map(|w| w.trim().to_string())
            .filter(|w| !w.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Hyperliquid live order placement requires a configured wallet address"
                )
            })?;

        let coin = perp_coin(&order.symbol)?;
        let (asset_index, sz_decimals) = self.resolve_asset_meta(&coin).await?;

        let raw_price = match order.price {
            Some(p) => p,
            None => {
                let ticker = self.fetch_ticker(&order.symbol).await?;
                match order.side {
                    Side::Buy => ticker.ask * dec!(1.05),
                    Side::Sell => ticker.bid * dec!(0.95),
                }
            }
        };
        let price = hl_wire_price(raw_price, sz_decimals, order.side);
        let quantity = order.quantity.trunc_with_scale(sz_decimals);
        if quantity <= dec!(0) {
            anyhow::bail!(
                "Hyperliquid order quantity {} truncates to zero at szDecimals={sz_decimals}",
                order.quantity
            );
        }

        // Hyperliquid's wire supports only Gtc/Ioc/Alo — FOK has no
        // representation and is refused rather than downgraded to a
        // resting order.
        let tif = if order.post_only
            || order.order_type == OrderType::PostOnly
            || order.time_in_force == TimeInForce::PostOnly
        {
            "Alo"
        } else if order.time_in_force == TimeInForce::ImmediateOrCancel
            || order.order_type == OrderType::ImmediateOrCancel
            || order.order_type == OrderType::Market
        {
            "Ioc"
        } else if order.time_in_force == TimeInForce::FillOrKill {
            anyhow::bail!("Hyperliquid has no FillOrKill — refusing instead of downgrading to GTC");
        } else {
            "Gtc"
        };

        let wire_order = HlOrderWire {
            a: asset_index,
            b: order.side == Side::Buy,
            p: hl_wire_decimal(price),
            s: hl_wire_decimal(quantity),
            r: order.reduce_only,
            t: HlOrderTypeWire {
                limit: HlLimitWire {
                    tif: tif.to_string(),
                },
            },
            c: None,
        };

        let action = HlActionWire::Order {
            orders: vec![wire_order],
            grouping: "na".to_string(),
        };

        // Pre-flight: refuse to submit when the held key derives an identity
        // that can't sign for the configured account — otherwise every order
        // burns a rejected nonce and surfaces a random-looking "does not
        // exist" address post-mortem.
        let signer = crate::wallet::EvmWalletSigner::from_hex(private_key.as_str())
            .map(|s| s.address_checksum())
            .map_err(|e| {
                anyhow::anyhow!("Hyperliquid stored private key does not derive a signer: {e}")
            })?;
        self.verify_signer_registered(&signer, &configured_wallet)
            .await?;
        tracing::info!(
            "hyperliquid order: signer={signer} account={configured_wallet} asset={asset_index} side={:?} px={} sz={} tif={tif}",
            order.side, price, quantity
        );

        let nonce = Utc::now().timestamp_millis() as u64;
        let (r, s, v) = sign_hyperliquid_l1_action(
            &action,
            nonce,
            None,
            self.is_mainnet,
            private_key.as_str(),
        )?;

        let body = json!({
            "action": action,
            "nonce": nonce,
            "signature": {
                "r": r,
                "s": s,
                "v": v,
            },
            "vaultAddress": null,
        });

        let resp_json = self.post_exchange(body).await?;

        parse_hl_order_response(&resp_json, &order, &self.venue_id, price)
    }

    async fn wire_base_qty(&self, symbol: &str, qty: Decimal) -> anyhow::Result<Decimal> {
        let coin = perp_coin(symbol)?;
        let (_asset_index, sz_decimals) = self.resolve_asset_meta(&coin).await?;
        let wire = qty.trunc_with_scale(sz_decimals);
        if wire <= dec!(0) {
            anyhow::bail!(
                "Hyperliquid {symbol} quantity {qty} truncates to zero at szDecimals={sz_decimals}"
            );
        }
        Ok(wire)
    }

    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
        // Same zeroizing copy discipline as place_order — key material
        // must not linger in a plain String.
        let private_key = {
            let guard = self.private_key.read();
            guard
                .as_deref()
                .map(|k| k.trim())
                .filter(|k| !k.is_empty())
                .map(|k| Zeroizing::new(k.to_string()))
        }
        .ok_or_else(|| {
            anyhow::anyhow!("Hyperliquid live order cancellation requires a private key")
        })?;

        let coin = perp_coin(&req.symbol)?;
        let asset_index = self.resolve_asset_index(&coin).await?;
        let oid_str = req.order_id.as_deref().ok_or_else(|| {
            anyhow::anyhow!("Hyperliquid order cancellation requires an order_id")
        })?;
        let oid = oid_str.parse::<u64>().map_err(|e| {
            anyhow::anyhow!("Invalid Hyperliquid order_id '{oid_str}': must be integer oid: {e}")
        })?;

        let action = HlActionWire::Cancel {
            cancels: vec![HlCancelWire {
                a: asset_index,
                o: oid,
            }],
        };
        let nonce = Utc::now().timestamp_millis() as u64;
        let (r, s, v) =
            sign_hyperliquid_l1_action(&action, nonce, None, self.is_mainnet, &private_key)?;

        let body = json!({
            "action": action,
            "nonce": nonce,
            "signature": {
                "r": r,
                "s": s,
                "v": v,
            },
            "vaultAddress": null,
        });
        let resp_json = self.post_exchange(body).await?;
        if resp_json.get("status").and_then(|s| s.as_str()) == Some("err") {
            anyhow::bail!(
                "Hyperliquid cancel error: {}",
                resp_json.get("response").unwrap_or(&resp_json)
            );
        }
        // Top-level "ok" is not a cancel confirmation — each cancel carries
        // a per-order status ("success" or {"error": ...}). Anything other
        // than success means the order may still rest.
        let statuses = resp_json
            .pointer("/response/data/statuses")
            .and_then(|s| s.as_array())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Hyperliquid cancel response missing per-order statuses: {resp_json}"
                )
            })?;
        for entry in statuses {
            match entry {
                serde_json::Value::String(s) if s == "success" => {}
                other => {
                    anyhow::bail!("Hyperliquid cancel not confirmed for oid {oid}: {other}")
                }
            }
        }
        Ok(())
    }

    /// Poll authoritative order state via `orderStatus` + `userFills`.
    /// `orderStatus` proves the terminal state and residual size; `userFills`
    /// supplies the volume-weighted price and fee. When fills fall outside
    /// the venue's recent-fill window the average price comes back `None` —
    /// never fabricated.
    async fn fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        _client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        use arbitrageur_core::types::OrderStatus;
        let user = {
            let guard = self.wallet_address.read();
            guard
                .as_deref()
                .map(str::trim)
                .filter(|w| !w.is_empty())
                .map(ToString::to_string)
        }
        .ok_or_else(|| anyhow::anyhow!("Hyperliquid order fetch requires a wallet address"))?;
        let oid = order_id.trim().parse::<u64>().map_err(|e| {
            anyhow::anyhow!("Invalid Hyperliquid order_id '{order_id}': must be integer oid: {e}")
        })?;
        let coin = perp_coin(symbol)?;

        let v = self
            .post_info(json!({"type": "orderStatus", "user": user, "oid": oid}))
            .await?;
        match v.get("status").and_then(|s| s.as_str()) {
            Some("order") => {}
            Some("unknownOid") => {
                anyhow::bail!(
                    "Hyperliquid orderStatus reports unknown oid {oid}; order state unresolvable"
                )
            }
            _ => anyhow::bail!("Hyperliquid orderStatus malformed response: {v}"),
        }
        let inner = v
            .get("order")
            .and_then(|o| o.get("order"))
            .ok_or_else(|| anyhow::anyhow!("Hyperliquid orderStatus missing order detail: {v}"))?;
        let returned_oid = inner
            .get("oid")
            .and_then(|o| o.as_u64())
            .ok_or_else(|| anyhow::anyhow!("Hyperliquid orderStatus missing oid: {v}"))?;
        if returned_oid != oid {
            anyhow::bail!(
                "Hyperliquid orderStatus returned oid {returned_oid} != queried {oid}: {v}"
            );
        }
        let returned_coin = inner
            .get("coin")
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow::anyhow!("Hyperliquid orderStatus missing coin: {v}"))?;
        if !returned_coin.eq_ignore_ascii_case(&coin) {
            anyhow::bail!(
                "Hyperliquid orderStatus coin '{returned_coin}' != expected '{coin}': {v}"
            );
        }
        let orig_sz = dec_from_json(inner.get("origSz"))
            .filter(|q| *q > dec!(0))
            .ok_or_else(|| anyhow::anyhow!("Hyperliquid orderStatus missing origSz: {v}"))?;
        let remaining_sz = dec_from_json(inner.get("sz"))
            .filter(|q| *q >= dec!(0))
            .ok_or_else(|| anyhow::anyhow!("Hyperliquid orderStatus missing sz: {v}"))?;
        if remaining_sz > orig_sz {
            anyhow::bail!(
                "Hyperliquid orderStatus sz {remaining_sz} exceeds origSz {orig_sz}: {v}"
            );
        }
        let filled_qty = orig_sz - remaining_sz;
        let raw_status = v
            .get("order")
            .and_then(|o| o.get("status"))
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow::anyhow!("Hyperliquid orderStatus missing status: {v}"))?;
        let status = match raw_status {
            "open" if filled_qty > dec!(0) => OrderStatus::PartiallyFilled,
            "open" => OrderStatus::New,
            "filled" if filled_qty == orig_sz => OrderStatus::Filled,
            "filled" => anyhow::bail!(
                "Hyperliquid orderStatus 'filled' has inconsistent sz {remaining_sz}/{orig_sz}: {v}"
            ),
            "canceled"
            | "marginCanceled"
            | "delistedCanceled"
            | "liquidatedCanceled"
            | "expired"
            | "selfTradeCanceled"
            | "siblingFilledCanceled"
            | "triggered"
            | "scheduledCancel" => {
                // These are all terminal on the venue — a partial fill does
                // not keep the order working. `filled_qty`/`remaining_qty`
                // already carry the partial truth; `PartiallyFilled` here
                // would classify as Working and deadlock resolution until
                // the reconciler's deadline.
                OrderStatus::Cancelled
            }
            "rejected" if filled_qty == dec!(0) => OrderStatus::Rejected,
            other => anyhow::bail!("Hyperliquid orderStatus unknown status '{other}': {v}"),
        };

        // Best-effort fill detail: aggregate userFills for this oid to get a
        // true vwap + fee. Rows are venue-validated on coin and oid.
        let fills_v = self
            .post_info(json!({"type": "userFills", "user": user}))
            .await
            .unwrap_or_else(|_| json!({"fills": []}));
        let mut fqty = dec!(0);
        let mut fnotional = dec!(0);
        let mut ffee = dec!(0);
        if let Some(rows) = fills_v.as_array() {
            for f in rows {
                let fid = f.get("oid").and_then(|o| o.as_u64());
                if fid != Some(oid) {
                    continue;
                }
                if f.get("coin").and_then(|s| s.as_str()) != Some(returned_coin) {
                    continue;
                }
                let sz = dec_from_json(f.get("sz")).unwrap_or_default();
                let px = dec_from_json(f.get("px")).unwrap_or_default();
                if sz > dec!(0) && px > dec!(0) {
                    fqty += sz;
                    fnotional += sz * px;
                }
                ffee += dec_from_json(f.get("fee")).unwrap_or_default().abs();
            }
        }
        let avg_px = if fqty > dec!(0) {
            Some(fnotional / fqty)
        } else {
            None
        };
        Ok(OrderResponse {
            order_id: order_id.to_string(),
            client_order_id: String::new(),
            venue: VenueId::new("hyperliquid"),
            symbol: symbol.to_string(),
            status,
            original_qty: orig_sz,
            filled_qty,
            remaining_qty: remaining_sz,
            avg_fill_price: avg_px,
            fee: Some(ffee),
            fee_currency: "USDC".to_string(),
            raw_response: Some(v.to_string()),
            timestamp: Utc::now(),
        })
    }

    async fn set_leverage(&self, _symbol: &str, _leverage: u32) -> anyhow::Result<()> {
        anyhow::bail!(
            "Leverage updates are not implemented for venue {}",
            self.name()
        )
    }

    fn supports_live_execution(&self) -> bool {
        true
    }

    fn order_status_polling_implemented(&self) -> bool {
        true
    }

    fn live_order_submission_implemented(&self) -> bool {
        true
    }

    fn has_live_credentials(&self) -> bool {
        self.has_credentials()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perp_coin_normalizes_and_rejects_spot() {
        assert_eq!(perp_coin("BTC-USDC").unwrap(), "BTC");
        assert_eq!(perp_coin("eth").unwrap(), "eth");
        assert!(perp_coin("ETH-SPOT").is_err());
        assert!(perp_coin("eth/spot").is_err());
        // A perp actually named SPOT must still resolve as a perp.
        assert_eq!(perp_coin("SPOT-USDC").unwrap(), "SPOT");
        assert!(perp_coin("BTC-USD").is_err());
        assert!(perp_coin("BTC-USDT").is_err());
        assert!(perp_coin("BTC-SOL").is_err());
        assert!(perp_coin("BTC-ETH").is_err());
    }

    #[test]
    fn hl_wire_price_respects_sigfigs_and_szdecimals() {
        // ETH perp szDecimals=4 → ≤2 decimals AND ≤5 sig figs → 1 decimal.
        // The canary failure: 2726.1 * 1.003 = 2734.2783 was rejected.
        assert_eq!(hl_wire_price(dec!(2734.2783), 4, Side::Buy), dec!(2734.2));
        // Sell bound rounds UP one tick — never below the caller's cap.
        assert_eq!(hl_wire_price(dec!(2734.2783), 4, Side::Sell), dec!(2734.3));
        // BTC perp szDecimals=5 → 5 sig figs → integer for 5-digit prices.
        assert_eq!(hl_wire_price(dec!(97123.456), 5, Side::Buy), dec!(97123));
        // Integers always pass untouched.
        assert_eq!(hl_wire_price(dec!(123456), 4, Side::Buy), dec!(123456));
        // Sub-dollar: szDecimals=1 → ≤5 decimals AND ≤5 sig figs → 5 dec.
        assert_eq!(hl_wire_price(dec!(0.01234567), 1, Side::Buy), dec!(0.01234));
        // szDecimals=0 → perp cap is 6 decimal places (decimal limit binds
        // before sig figs): 0.000012345678 → 0.000012.
        assert_eq!(
            hl_wire_price(dec!(0.000012345678), 0, Side::Buy),
            dec!(0.000012)
        );
    }

    #[tokio::test]
    async fn fetch_account_requires_wallet_address() {
        for wallet in [None, Some(String::new()), Some("   ".to_string())] {
            let adapter =
                HyperliquidAdapter::new(Some("http://127.0.0.1:1".to_string()), wallet, None);
            let err = adapter
                .fetch_account()
                .await
                .expect_err("must fail closed without identity");
            assert_eq!(
                err.to_string(),
                "Hyperliquid account fetch requires a wallet address"
            );
        }
    }

    #[test]
    fn test_k256_signing_api() {
        use k256::ecdsa::SigningKey;
        let key_bytes = [1u8; 32];
        let signing_key = SigningKey::from_bytes((&key_bytes).into()).unwrap();
        let (sig, recid) = signing_key.sign_prehash_recoverable(&[0u8; 32]);
        let _r = sig.r();
        let _s = sig.s();
        let v = recid.to_byte() + 27;
        assert!(v == 27 || v == 28);
    }

    #[test]
    fn test_hyperliquid_l1_order_signing() {
        let order = HlOrderWire {
            a: 0,
            b: true,
            p: "3000.5".to_string(),
            s: "0.1".to_string(),
            r: false,
            t: HlOrderTypeWire {
                limit: HlLimitWire {
                    tif: "Ioc".to_string(),
                },
            },
            c: None,
        };
        let action = HlActionWire::Order {
            orders: vec![order],
            grouping: "na".to_string(),
        };
        let dummy_pk = "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let (r, s, v) =
            sign_hyperliquid_l1_action(&action, 1234567890, None, true, dummy_pk).unwrap();
        assert!(r.starts_with("0x") && r.len() == 66);
        assert!(s.starts_with("0x") && s.len() == 66);
        assert!(v == 27 || v == 28);
    }

    #[test]
    fn test_hyperliquid_l1_signature_ecrecovers_to_signer() {
        // Round-trip: sign an order action with the well-known dev key
        // 0x…01 (address 0x7e5f4552…5bdf), then ecrecover (r,s,v,digest)
        // exactly as Hyperliquid's API does — the recovered address must be
        // the signer's, or HL rejects with "User or API Wallet does not
        // exist" carrying a random-looking address per message.
        let order = HlOrderWire {
            a: 0,
            b: true,
            p: "3000.5".to_string(),
            s: "0.1".to_string(),
            r: false,
            t: HlOrderTypeWire {
                limit: HlLimitWire {
                    tif: "Ioc".to_string(),
                },
            },
            c: None,
        };
        let action = HlActionWire::Order {
            orders: vec![order],
            grouping: "na".to_string(),
        };
        let pk = "0x0000000000000000000000000000000000000000000000000000000000000001";
        let nonce = 1234567890u64;
        let digest = hyperliquid_l1_digest(&action, nonce, None, true).unwrap();
        let (r_hex, s_hex, v) = sign_hyperliquid_l1_action(&action, nonce, None, true, pk).unwrap();

        let r_bytes: [u8; 32] = hex::decode(r_hex.trim_start_matches("0x"))
            .unwrap()
            .try_into()
            .unwrap();
        let s_bytes: [u8; 32] = hex::decode(s_hex.trim_start_matches("0x"))
            .unwrap()
            .try_into()
            .unwrap();
        let mut sig_bytes = [0u8; 64];
        sig_bytes[..32].copy_from_slice(&r_bytes);
        sig_bytes[32..].copy_from_slice(&s_bytes);
        let sig = k256::ecdsa::Signature::from_slice(&sig_bytes).unwrap();
        let recid = k256::ecdsa::RecoveryId::new(v == 28, false);
        let vk = k256::ecdsa::VerifyingKey::recover_from_prehash(&digest, &sig, recid)
            .expect("signature must recover");
        let pk_point = vk.to_sec1_point(false);
        let addr_hash = keccak256(&pk_point.as_bytes()[1..]);
        let recovered = format!("0x{}", hex::encode(&addr_hash[12..]));
        assert_eq!(
            recovered.to_lowercase(),
            "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf",
            "ecrecover must yield the signer's address; got {recovered}"
        );
    }

    #[test]
    fn test_hyperliquid_signature_matches_python_sdk_reference() {
        // Self-consistency isn't enough — the digest must equal what the
        // official SDK produces, or HL ecrecovers a garbage (different-per-
        // message) address and rejects "User or API Wallet does not exist".
        // Vectors produced by hyperliquid-python-sdk signing.py:
        //   action {"type":"order","orders":[{a:0,b:true,p:"3000.5",
        //     s:"0.1",r:false,t:{limit:{tif:"Ioc"}}}],"grouping":"na"},
        //   nonce 1700000000000, key 0x…01, mainnet.
        let order = HlOrderWire {
            a: 0,
            b: true,
            p: "3000.5".to_string(),
            s: "0.1".to_string(),
            r: false,
            t: HlOrderTypeWire {
                limit: HlLimitWire {
                    tif: "Ioc".to_string(),
                },
            },
            c: None,
        };
        let action = HlActionWire::Order {
            orders: vec![order],
            grouping: "na".to_string(),
        };
        let nonce = 1700000000000u64;
        let pk = "0x0000000000000000000000000000000000000000000000000000000000000001";

        // msgpack bytes must match msgpack.packb(action) exactly.
        let packed = rmp_serde::to_vec_named(&action).unwrap();
        assert_eq!(
            hex::encode(&packed),
            "83a474797065a56f72646572a66f72646572739186a16100a162c3a170a6333030302e35a173a3302e31a172c2a17481a56c696d697481a3746966a3496f63a867726f7570696e67a26e61",
            "msgpack bytes differ from Python packb — hash input mismatch"
        );
        let digest = hyperliquid_l1_digest(&action, nonce, None, true).unwrap();
        // The digest the Python SDK signed — proven by the r/s/v vectors
        // below (RFC-6979 is deterministic over key+digest).
        assert_eq!(
            hex::encode(digest),
            "cf67b3c90b4eea9c1cd6ea58b849b76cb6c5e6137aa2c1f1b5a772a101e0850b",
            "EIP-712 digest differs from Python SDK reference"
        );
        let (r, s, v) = sign_hyperliquid_l1_action(&action, nonce, None, true, pk).unwrap();
        assert_eq!(
            r, "0xa5ce1af2ef19c026adfc5dc50b5b56ad0828a8274316c52fd17cbcd9c7c6110f",
            "r differs from Python SDK reference"
        );
        assert_eq!(
            s, "0x788ded8d03eb2b5aab945748f9b9a16f98b38f385c82c5874c5333cbe3dc5eec",
            "s differs from Python SDK reference"
        );
        assert_eq!(v, 27, "v differs from Python SDK reference");
    }

    #[test]
    fn test_hyperliquid_capabilities_and_credentials() {
        let adapter = HyperliquidAdapter::new(None, None, None);
        assert!(adapter.supports_live_execution());
        assert!(!adapter.has_live_credentials());

        adapter.update_credentials(
            Some("0x1111111111111111111111111111111111111111".to_string()),
            Some("0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string()),
        );
        assert!(adapter.has_live_credentials());

        adapter.update_credentials(None, None);
        assert!(!adapter.has_live_credentials());
    }

    fn test_order_fixture() -> OrderRequest {
        OrderRequest::new_ioc(
            VenueId::new("hyperliquid"),
            "BTC-USDC",
            Side::Buy,
            dec!(50000),
            dec!(1),
        )
    }

    fn test_venue_fixture() -> VenueId {
        VenueId::new("hyperliquid")
    }

    fn hl_resp(statuses: serde_json::Value) -> serde_json::Value {
        json!({
            "status": "ok",
            "response": { "type": "order", "data": { "statuses": statuses } }
        })
    }

    #[test]
    fn parse_complete_fill() {
        let order = test_order_fixture();
        let resp =
            hl_resp(json!([{"filled": {"oid": 12345, "avgPx": "50001.5", "totalSz": "1.0"}}]));
        let out =
            parse_hl_order_response(&resp, &order, &test_venue_fixture(), dec!(50000)).unwrap();
        assert_eq!(out.status, OrderStatus::Filled);
        assert_eq!(out.order_id, "12345");
        assert_eq!(out.filled_qty, dec!(1));
        assert_eq!(out.remaining_qty, dec!(0));
        assert_eq!(out.avg_fill_price, Some(dec!(50001.5)));
    }

    #[test]
    fn parse_partial_fill() {
        let order = test_order_fixture();
        let resp = hl_resp(json!([{"filled": {"oid": 99, "avgPx": "49999.0", "totalSz": "0.4"}}]));
        let out =
            parse_hl_order_response(&resp, &order, &test_venue_fixture(), dec!(50000)).unwrap();
        assert_eq!(out.status, OrderStatus::PartiallyFilled);
        assert_eq!(out.order_id, "99");
        assert_eq!(out.filled_qty, dec!(0.4));
        assert_eq!(out.remaining_qty, dec!(0.6));
        assert_eq!(out.avg_fill_price, Some(dec!(49999.0)));
    }

    #[test]
    fn parse_resting() {
        let order = test_order_fixture();
        let resp = hl_resp(json!([{"resting": {"oid": 777}}]));
        let out =
            parse_hl_order_response(&resp, &order, &test_venue_fixture(), dec!(50000)).unwrap();
        assert_eq!(out.status, OrderStatus::New);
        assert_eq!(out.order_id, "777");
        assert_eq!(out.filled_qty, dec!(0));
        assert_eq!(out.remaining_qty, order.quantity);
        assert_eq!(out.avg_fill_price, None);
    }

    #[test]
    fn parse_missing_mandatory_fields_err() {
        let order = test_order_fixture();
        let venue = test_venue_fixture();
        let cases = vec![
            hl_resp(json!([{"filled": {"avgPx": "1.0", "totalSz": "0.5"}}])),
            hl_resp(json!([{"filled": {"oid": 1, "avgPx": "1.0"}}])),
            hl_resp(json!([{"filled": {"oid": 1, "totalSz": "0.5"}}])),
            hl_resp(json!([{"resting": {}}])),
        ];
        for resp in cases {
            let r = parse_hl_order_response(&resp, &order, &venue, dec!(50000));
            assert!(
                r.is_err(),
                "missing mandatory field must Err, never fabricate"
            );
        }
    }

    #[test]
    fn parse_unknown_shape_err() {
        let order = test_order_fixture();
        let venue = test_venue_fixture();
        let cases = vec![
            json!({"status": "ok", "response": {"type": "order", "data": {"statuses": []}}}),
            json!({"status": "ok", "response": {"type": "order"}}),
            json!({"status": "ok"}),
            hl_resp(json!([{"other": {}}])),
            json!({"status": "err", "response": "insufficient margin"}),
            hl_resp(json!([{"error": "rejected"}])),
        ];
        for resp in cases {
            assert!(parse_hl_order_response(&resp, &order, &venue, dec!(50000)).is_err());
        }
    }

    const ACCOUNT: &str = "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd";
    const SIGNER: &str = "0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf";

    /// Diagnostic: emits the exact JSON body `place_order` would POST for a
    /// live-shaped ETH order signed with the dev key at a fresh nonce. Run
    /// with `--nocapture`, then curl the body at /exchange — the address in
    /// HL's "does not exist" error is what their ecrecover sees.
    #[test]
    fn print_live_order_exchange_body() {
        // Mirror place_order's wire construction: quantity arrives at
        // szDecimals scale ("0.0050") and must be canonicalized exactly as
        // the live path does.
        let order = HlOrderWire {
            a: 1,
            b: true,
            p: dec!(2733.8).normalize().to_string(),
            s: dec!(0.0050).normalize().to_string(),
            r: false,
            t: HlOrderTypeWire {
                limit: HlLimitWire {
                    tif: "Ioc".to_string(),
                },
            },
            c: None,
        };
        let action = HlActionWire::Order {
            orders: vec![order],
            grouping: "na".to_string(),
        };
        let pk = "0x0000000000000000000000000000000000000000000000000000000000000001";
        let nonce = Utc::now().timestamp_millis() as u64;
        let (r, s, v) = sign_hyperliquid_l1_action(&action, nonce, None, true, pk).unwrap();
        let body = json!({
            "action": action,
            "nonce": nonce,
            "signature": { "r": r, "s": s, "v": v },
            "vaultAddress": null,
        });
        eprintln!("HL_CURL_BODY={}", serde_json::to_string(&body).unwrap());
        eprintln!("HL_EXPECTED_SIGNER=0x7e5f4552091a69125d5dfcb7b8c2659029395bdf");
    }

    #[test]
    fn hl_wire_decimal_emits_canonical_form() {
        // Trailing zeros are stripped — HL hashes the canonical form.
        assert_eq!(hl_wire_decimal(dec!(0.0050)), "0.005");
        assert_eq!(hl_wire_decimal(dec!(2733.80)), "2733.8");
        assert_eq!(hl_wire_decimal(dec!(2740.00)), "2740");
        assert_eq!(hl_wire_decimal(dec!(0.1)), "0.1");
        assert_eq!(hl_wire_decimal(dec!(123456)), "123456");
        assert_eq!(hl_wire_decimal(dec!(0.00001234)), "0.00001234");
        // The exact canary failure shape: szDecimals=4 quantity arrives as
        // "0.0050" and must go on the wire as "0.005".
        let q = dec!(0.005).trunc_with_scale(4);
        assert_eq!(hl_wire_decimal(q), "0.005");
    }

    #[test]
    fn signer_role_direct_user_matching_account_ok() {
        // The held key IS the configured account.
        let resp = json!({"role": "user"});
        assert!(validate_signer_role_response(&resp, ACCOUNT, ACCOUNT).is_ok());
        // Case-insensitive comparison — EIP-55 checksum vs lowercase.
        assert!(validate_signer_role_response(&resp, &ACCOUNT.to_uppercase(), ACCOUNT).is_ok());
    }

    #[test]
    fn signer_role_direct_user_mismatched_account_err() {
        let resp = json!({"role": "user"});
        let err = validate_signer_role_response(&resp, SIGNER, ACCOUNT).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains(SIGNER), "names the signer: {msg}");
        assert!(msg.contains(ACCOUNT), "names the configured account: {msg}");
        assert!(msg.contains("no order submitted"), "fails closed: {msg}");
    }

    #[test]
    fn signer_role_agent_bound_to_account_ok() {
        // Agent approved for the configured account — case-insensitive.
        let resp = json!({"role": "agent", "data": {"user": ACCOUNT}});
        assert!(validate_signer_role_response(&resp, SIGNER, ACCOUNT).is_ok());
        let resp = json!({"role": "agent", "data": {"user": ACCOUNT.to_uppercase()}});
        assert!(validate_signer_role_response(&resp, SIGNER, ACCOUNT).is_ok());
    }

    #[test]
    fn signer_role_agent_bound_to_other_account_err() {
        let resp = json!({"role": "agent", "data": {"user": "0x2222222222222222222222222222222222222222"}});
        let err = validate_signer_role_response(&resp, SIGNER, ACCOUNT).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains(SIGNER), "names the signer: {msg}");
        assert!(
            msg.contains("0x2222222222222222222222222222222222222222"),
            "names the bound account: {msg}"
        );
        assert!(msg.contains(ACCOUNT), "names the configured account: {msg}");
        assert!(msg.contains("no order submitted"), "fails closed: {msg}");
    }

    #[test]
    fn signer_role_missing_err() {
        let resp = json!({"role": "missing"});
        let err = validate_signer_role_response(&resp, SIGNER, ACCOUNT).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not an approved API wallet"), "{msg}");
        assert!(msg.contains("no order submitted"), "{msg}");
    }

    #[test]
    fn signer_role_agent_malformed_data_err() {
        for resp in [
            json!({"role": "agent"}),
            json!({"role": "agent", "data": {}}),
            json!({"role": "agent", "data": {"user": ""}}),
            json!({"role": "agent", "data": {"user": 42}}),
        ] {
            assert!(
                validate_signer_role_response(&resp, SIGNER, ACCOUNT).is_err(),
                "malformed agent data must fail closed: {resp}"
            );
        }
    }

    #[test]
    fn signer_role_malformed_or_absent_err() {
        for resp in [
            json!({}),
            json!({"role": null}),
            json!({"role": 7}),
            json!("user"),
            json!({"role": ""}),
        ] {
            assert!(
                validate_signer_role_response(&resp, SIGNER, ACCOUNT).is_err(),
                "malformed/missing role must fail closed: {resp}"
            );
        }
    }

    #[test]
    fn signer_role_vault_and_subaccount_err() {
        for role in ["vault", "subAccount", "bogon"] {
            let resp = json!({"role": role});
            let err = validate_signer_role_response(&resp, SIGNER, ACCOUNT).unwrap_err();
            assert!(
                err.to_string().contains("not a valid signer role"),
                "role {role}: {err}"
            );
        }
    }

    #[test]
    fn signer_role_empty_identities_err() {
        let resp = json!({"role": "user"});
        assert!(validate_signer_role_response(&resp, "", ACCOUNT).is_err());
        assert!(validate_signer_role_response(&resp, SIGNER, "").is_err());
        assert!(validate_signer_role_response(&resp, "  ", ACCOUNT).is_err());
    }

    #[test]
    fn active_signer_tracks_credentials_and_cache_survives_identical_update() {
        let key_a = format!("0x{}", "0".repeat(63) + "1");
        let key_b = format!("0x{}", "0".repeat(62) + "11");
        let adapter = HyperliquidAdapter::new(None, Some(ACCOUNT.to_string()), Some(key_a.clone()));
        assert_eq!(adapter.active_signer_address().as_deref(), Some(SIGNER));

        // Inject a successful verification; a byte-identical re-provision
        // (periodic sync while unlocked) must not clear it.
        *adapter.verified_signer.write() = Some(VerifiedSigner {
            signer: SIGNER.to_string(),
            account: ACCOUNT.to_string(),
            verified_at: Instant::now(),
        });
        assert!(adapter.has_fresh_signer_verification(SIGNER, ACCOUNT));

        // A stale entry past the TTL must not count as a cache hit — API
        // wallets can be deregistered between orders.
        *adapter.verified_signer.write() = Some(VerifiedSigner {
            signer: SIGNER.to_string(),
            account: ACCOUNT.to_string(),
            verified_at: Instant::now() - SIGNER_VERIFICATION_TTL - Duration::from_secs(1),
        });
        assert!(
            !adapter.has_fresh_signer_verification(SIGNER, ACCOUNT),
            "expired verification must force a fresh userRole query"
        );

        *adapter.verified_signer.write() = Some(VerifiedSigner {
            signer: SIGNER.to_string(),
            account: ACCOUNT.to_string(),
            verified_at: Instant::now(),
        });
        adapter.update_credentials(Some(ACCOUNT.to_string()), Some(key_a));
        assert!(
            adapter.verified_signer.read().is_some(),
            "identical credential update must preserve the verified-signer cache"
        );

        // A changed key invalidates the cache and moves the active signer.
        adapter.update_credentials(Some(ACCOUNT.to_string()), Some(key_b));
        assert!(adapter.verified_signer.read().is_none());
        let expected_b =
            crate::wallet::EvmWalletSigner::from_hex(&format!("0x{}", "0".repeat(62) + "11"))
                .unwrap()
                .address_checksum();
        assert_eq!(
            adapter.active_signer_address().as_deref(),
            Some(expected_b.as_str())
        );

        // Clearing credentials drops the signer entirely.
        adapter.update_credentials(None, None);
        assert_eq!(adapter.active_signer_address(), None);
    }
}
