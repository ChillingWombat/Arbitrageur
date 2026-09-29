use crate::rate_limiter::AccountRequestCoordinator;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::{AccountSnapshot, Balance, Position};
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::symbol::canonical_base;
use arbitrageur_core::types::{dec, Decimal, OrderStatus, OrderType, Side, TimeInForce, VenueId};
use async_trait::async_trait;
use chrono::Utc;
use ed25519_dalek::Signer;
use parking_lot::RwLock;
use reqwest::Client;
use rust_decimal::prelude::ToPrimitive;
use serde_json::json;
use std::collections::HashMap;
use zeroize::Zeroizing;

/// Arcus perpetuals connector — a real CLOB on its own appchain.
///
/// Endpoints (documented at docs.arcus.xyz): live `https://api.arcus.xyz`,
/// testnet `https://api.testnet.arcus.xyz` — identical paths on both hosts,
/// environment selects by base URL. Market data and account/order reads are
/// unauthenticated (`/v1/markets`, `/v1/l2OrderBook/{market}`, `/v1/mids`,
/// `/v1/account`, `/v1/order/{id}`). Order routing signs an Ed25519
/// "ordersign" typed canonical payload per request (Scheme 1 in the venue
/// docs): `X-API-Key` + `X-Timestamp` (nanoseconds) + `X-Signature`.
/// Placement is asynchronous — a `202` ACK carries no terminal state, so
/// order truth comes from `fetch_order` polling.
pub struct ArcusAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    api_key: RwLock<Option<Zeroizing<String>>>,
    api_secret: RwLock<Option<Zeroizing<String>>>,
    /// Parsed Ed25519 signing key derived from `api_secret` at credential
    /// load; `None` means the secret was absent or malformed.
    ed25519: RwLock<Option<ed25519_dalek::SigningKey>>,
    /// Master Ethereum address — `GET /v1/account` scopes by it. Not
    /// secret (public key material) but stored alongside credentials.
    wallet_address: RwLock<Option<String>>,
    /// Arcus subaccount index (0–9); default 0.
    account_index: RwLock<Option<u64>>,
    /// marketDisplayName → order-routing params, filled from `/v1/markets`.
    market_specs: RwLock<HashMap<String, ArcusMarketSpec>>,
    rate_limiter: AccountRequestCoordinator,
}

/// Order-routing parameters for one Arcus market — the `marketId` plus the
/// tick/step grid needed to express price and quantity as the signed
/// integer ticks/quantums the venue requires.
///
/// Arcus markets carry *dynamic* price tiers (`tickTiers`): the applicable
/// tick depends on the wire price's band. Each entry is
/// `(upToPrice, tick)` with `upToPrice` an EXCLUSIVE upper bound; the last
/// tier omits the bound (applies to every higher price). `tick_size`
/// retains the top-level `tickSize` only as a legacy fallback when the
/// venue omits `tickTiers` entirely.
#[derive(Clone, Debug)]
struct ArcusMarketSpec {
    market_id: u64,
    tick_size: Decimal,
    step_size: Decimal,
    /// Ascending by `upToPrice` (`None` last = unbounded). Empty → always
    /// the top-level `tick_size`.
    tick_tiers: Vec<(Option<Decimal>, Decimal)>,
}

impl ArcusMarketSpec {
    /// Tick for the band containing `price`. `upToPrice` is exclusive:
    /// a price exactly on a bound belongs to the NEXT tier.
    fn tick_size_at(&self, price: Decimal) -> Decimal {
        for (bound, tick) in &self.tick_tiers {
            match bound {
                Some(up) if price >= *up => continue,
                _ => return *tick,
            }
        }
        self.tick_size
    }
}

impl ArcusAdapter {
    pub fn new(
        base_url: Option<String>,
        api_key: Option<String>,
        api_secret: Option<String>,
    ) -> Self {
        // api.arcus.exchange does not resolve; the live host is
        // api.arcus.xyz (verified via /health, 2026-09). Persisted settings
        // may still carry the dead hostname — normalize rather than pin a
        // broken endpoint.
        let mut base = base_url.unwrap_or_else(|| "https://api.arcus.xyz".to_string());
        if base.contains("api.arcus.exchange") {
            base = "https://api.arcus.xyz".to_string();
        }
        Self {
            venue_id: VenueId::new("arcus"),
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            base_url: base,
            api_key: RwLock::new(api_key.map(Zeroizing::new)),
            ed25519: RwLock::new(api_secret.as_deref().and_then(parse_signing_key)),
            api_secret: RwLock::new(api_secret.map(Zeroizing::new)),
            wallet_address: RwLock::new(None),
            account_index: RwLock::new(None),
            market_specs: RwLock::new(HashMap::new()),
            rate_limiter: AccountRequestCoordinator::new("arcus", 100, 2000, 500, 10000),
        }
    }

    /// Rotate or clear credentials. `api_secret` is the venue-issued
    /// Ed25519 private key (hex, optionally `0x`-prefixed, 32-byte seed or
    /// 64-byte seed||pubkey); `wallet_address` + `account_index` scope both
    /// the public account reads and the signed `ad`/`ai` payload fields.
    pub fn update_credentials(
        &self,
        api_key: Option<String>,
        api_secret: Option<String>,
        wallet_address: Option<String>,
        account_index: Option<u64>,
    ) {
        *self.api_key.write() = api_key.map(Zeroizing::new);
        *self.ed25519.write() = api_secret.as_deref().and_then(parse_signing_key);
        *self.api_secret.write() = api_secret.map(Zeroizing::new);
        *self.wallet_address.write() = wallet_address;
        *self.account_index.write() = account_index;
    }

    /// Credentials required for signed order routes: a parseable Ed25519
    /// secret, the public key for `X-API-Key`, the master wallet address
    /// (lowercased — `ad` is the payload's only case-folded field), and the
    /// subaccount index (default 0). When a public key is configured it must
    /// match the secret's derived key — a mismatch would authenticate the
    /// signature as a different key, so fail closed.
    fn order_creds(&self) -> anyhow::Result<(ed25519_dalek::SigningKey, String, String, u64)> {
        let key = self.ed25519.read().clone().ok_or_else(|| {
            anyhow::anyhow!(
                "Arcus order routing requires a valid Ed25519 private key (Exchange Accounts)"
            )
        })?;
        let derived = verifying_key_hex(&key);
        let api_key = {
            let guard = self.api_key.read();
            guard
                .as_deref()
                .map(|k| k.trim())
                .filter(|k| !k.is_empty())
                .map(|k| k.trim_start_matches("0x").to_ascii_lowercase())
        };
        if let Some(k) = &api_key {
            if *k != derived {
                anyhow::bail!(
                    "Arcus API public key does not match the Ed25519 private key \
                     (derived {derived}) — refusing to sign"
                );
            }
        }
        let wallet = {
            let guard = self.wallet_address.read();
            guard
                .as_deref()
                .map(str::trim)
                .filter(|w| !w.is_empty())
                .map(|w| w.to_ascii_lowercase())
        }
        .ok_or_else(|| anyhow::anyhow!("Arcus order routing requires the master wallet address"))?;
        let ai = self.account_index.read().unwrap_or(0);
        Ok((key, api_key.unwrap_or(derived), wallet, ai))
    }

    /// Order-routing params for a market, loading `/v1/markets` on a cache
    /// miss (the same fetch populates `market_specs` for all markets).
    async fn market_spec(&self, market: &str) -> anyhow::Result<ArcusMarketSpec> {
        if let Some(spec) = self.market_specs.read().get(market) {
            return Ok(spec.clone());
        }
        self.fetch_markets().await?;
        self.market_specs
            .read()
            .get(market)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Arcus market {market} not listed on /v1/markets"))
    }

    /// POST with the venue's three auth headers. Returns the HTTP status
    /// and raw body — `202 Accepted` is success (async ack), so callers
    /// decide which statuses carry terminal state.
    async fn signed_post(
        &self,
        path: &str,
        body: serde_json::Value,
        api_key: &str,
        timestamp_nanos: u64,
        signature_hex: &str,
    ) -> anyhow::Result<(reqwest::StatusCode, serde_json::Value, String)> {
        self.rate_limiter.acquire_signed().await?;
        let url = format!("{}{}", self.base_url, path);
        let resp = match self
            .client
            .post(&url)
            .header("X-API-Key", api_key)
            .header("X-Timestamp", timestamp_nanos.to_string())
            .header("X-Signature", signature_hex)
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                self.rate_limiter.record_failure(false);
                return Err(e.into());
            }
        };
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !(status.is_success() || status == reqwest::StatusCode::ACCEPTED) {
            let is_429 = status == reqwest::StatusCode::TOO_MANY_REQUESTS;
            self.rate_limiter.record_failure(is_429);
            anyhow::bail!("Arcus {path} error: {status} {text}");
        }
        self.rate_limiter.record_success(text.clone());
        let json = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
        Ok((status, json, text))
    }

    fn sign(&self, key: &ed25519_dalek::SigningKey, payload: &str) -> String {
        hex::encode(key.sign(payload.as_bytes()).to_bytes())
    }

    /// Snap a price onto the market's tick grid. `loosen=true` widens the
    /// bound (buy up / sell down — used for MARKET protective bounds so the
    /// guard never rejects our own intent); `loosen=false` preserves the
    /// caller's limit (buy down / sell up — the wire price never exceeds
    /// the limit).
    fn snap_to_tick(price: Decimal, tick_size: Decimal, side: Side, loosen: bool) -> Decimal {
        let ticks = price / tick_size;
        let snapped = match (side, loosen) {
            (Side::Buy, true) | (Side::Sell, false) => ticks.ceil(),
            (Side::Sell, true) | (Side::Buy, false) => ticks.floor(),
        };
        snapped * tick_size
    }

    /// Snap `price` using the tick of the band the SNAPPED result lands in
    /// — the tier is resolved from the wire price, not the request price.
    /// Iterates because a snap can cross a tier bound (e.g. a looser market
    /// bound pushed up into a coarser band); bounded at 4 passes, and
    /// `exact_units` downstream still fails closed if a malformed tier list
    /// never converges.
    fn snap_price_tiered(
        price: Decimal,
        spec: &ArcusMarketSpec,
        side: Side,
        loosen: bool,
    ) -> Decimal {
        let mut p = price;
        for _ in 0..4 {
            let snapped = Self::snap_to_tick(p, spec.tick_size_at(p), side, loosen);
            if snapped == p {
                return p;
            }
            p = snapped;
        }
        Self::snap_to_tick(p, spec.tick_size_at(p), side, loosen)
    }

    /// Exact integer ticks/quantums: `value / step` must be integral or the
    /// signed payload would not match what the venue validates.
    fn exact_units(value: Decimal, step: Decimal, what: &str, market: &str) -> anyhow::Result<u64> {
        let units = value / step;
        if units.fract() != dec!(0) || units <= dec!(0) {
            anyhow::bail!("Arcus {what} {value} is not a positive multiple of {step} on {market}");
        }
        units.to_u64().ok_or_else(|| {
            anyhow::anyhow!("Arcus {what} {value} overflows integer units on {market}")
        })
    }

    /// Build an `OrderResponse` from the venue's order shape (place/cancel
    /// `200` bodies and `GET /v1/order/{id}` all share it). Missing fields
    /// needed to reconcile fail closed rather than report a guessed state.
    fn order_response(
        &self,
        data: &serde_json::Value,
        symbol: &str,
        fallback_qty: Decimal,
        raw: &str,
    ) -> anyhow::Result<OrderResponse> {
        let dec_of = |key: &str| -> Option<Decimal> {
            data.get(key)
                .and_then(|v| v.as_str())
                .and_then(|s| Decimal::from_str_exact(s).ok())
        };
        let order_id = data
            .get("orderId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Arcus order response missing orderId: {raw}"))?
            .to_string();
        let status_raw = data
            .get("status")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Arcus order response missing status: {raw}"))?;
        let original = dec_of("originalSize")
            .or_else(|| dec_of("quantity"))
            .unwrap_or(fallback_qty);
        let filled = dec_of("filledSize").unwrap_or(dec!(0));
        let remaining = dec_of("remainingSize").unwrap_or_else(|| (original - filled).max(dec!(0)));
        let status = arcus_order_status(status_raw, filled, original)?;
        Ok(OrderResponse {
            order_id,
            client_order_id: data
                .get("clientId")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            status,
            original_qty: original,
            filled_qty: filled,
            remaining_qty: remaining,
            avg_fill_price: dec_of("avgFillPrice"),
            fee: None,
            fee_currency: "USDG".to_string(),
            raw_response: Some(raw.to_string()),
            timestamp: Utc::now(),
        })
    }

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
            anyhow::bail!("Arcus API error: {} {}", resp.status(), path);
        }

        let text = resp.text().await?;
        self.rate_limiter.record_success_path(path, text.clone());
        let data = serde_json::from_str::<serde_json::Value>(&text)?;
        Ok(data)
    }
}

/// Engine symbol → Arcus market name. Arcus perps quote USD exclusively
/// and spell markets `BASE-USD`; the engine's canonical forms (`BTC`,
/// `BTC-USD`, `BTC-USDC`, `BTC-PERP`) all reduce through the shared
/// canonical-base normalizer.
fn arcus_market_name(symbol: &str) -> String {
    format!("{}-USD", canonical_base(symbol))
}

/// Decimal places implied by a string step/tick size ("0.1" → 1,
/// "0.00000001" → 8, "5" → 0). The venue publishes grid sizes as strings;
/// scale is the wire-truthful precision.
fn decimals_of(value: &str) -> u32 {
    Decimal::from_str_exact(value.trim())
        .map(|d| d.scale())
        .unwrap_or(0)
}

/// Ed25519 secret in hex (optionally `0x`-prefixed). Accepts a 32-byte seed
/// or a 64-byte seed||pubkey encoding (first 32 bytes are the seed, matching
/// the venue SDK's export formats). Returns `None` on malformed input so a
/// bad key fails closed at credential load rather than mid-order.
fn parse_signing_key(secret: &str) -> Option<ed25519_dalek::SigningKey> {
    let s = secret.trim().trim_start_matches("0x");
    let bytes = hex::decode(s).ok()?;
    let seed: [u8; 32] = match bytes.len() {
        32 => bytes.as_slice().try_into().ok()?,
        64 => bytes[..32].try_into().ok()?,
        _ => return None,
    };
    Some(ed25519_dalek::SigningKey::from_bytes(&seed))
}

/// Hex-encoded Ed25519 public key as the venue expects it in `X-API-Key`
/// (64 lowercase hex chars, no `0x`).
fn verifying_key_hex(key: &ed25519_dalek::SigningKey) -> String {
    hex::encode(key.verifying_key().to_bytes())
}

/// Client ids the venue accepts: `[A-Za-z0-9_-]{1,36}` so the signed `c`
/// field is byte-identical across SDK languages. Engine ids (UUIDs) already
/// conform; anything else is dropped — the order is still tracked by the
/// server order id.
fn arcus_client_id(client_order_id: &str) -> Option<String> {
    let id = client_order_id.trim();
    if id.is_empty() || id.len() > 36 {
        return None;
    }
    if id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Some(id.to_string())
    } else {
        None
    }
}

/// Engine TIF → Arcus `timeInForce` string and the `t` code in the signed
/// payload (0 GTT, 1 FOK, 2 IOC, 3 ALO).
fn arcus_tif(tif: TimeInForce, post_only: bool) -> (&'static str, u64) {
    if post_only {
        return ("ALO", 3);
    }
    match tif {
        TimeInForce::GoodTilCancelled => ("GTT", 0),
        TimeInForce::FillOrKill => ("FOK", 1),
        TimeInForce::ImmediateOrCancel => ("IOC", 2),
        TimeInForce::PostOnly => ("ALO", 3),
    }
}

/// Canonical `placeOrder` signing payload (Scheme 1, `op=1`): key-sorted
/// JSON, no whitespace, `ad` lowercased, `c` omitted when absent.
#[allow(clippy::too_many_arguments)]
fn place_payload(
    wallet: &str,
    account_index: u64,
    client_id: Option<&str>,
    ct: u64,
    g: u64,
    market_id: u64,
    price_ticks: u64,
    qty_quantums: u64,
    reduce_only: bool,
    side: u64,
    tif: u64,
) -> String {
    let mut p = format!("{{\"ad\":\"{wallet}\",\"ai\":{account_index}");
    if let Some(c) = client_id {
        p.push_str(&format!(",\"c\":\"{c}\""));
    }
    p.push_str(&format!(
        ",\"ct\":{ct},\"g\":{g},\"m\":{market_id},\"op\":1,\"p\":{price_ticks},\"q\":{qty_quantums},\"r\":{},\"s\":{side},\"t\":{tif},\"v\":1}}",
        u64::from(reduce_only)
    ));
    p
}

/// Canonical `cancelOrder` signing payload (`op=2`): exactly one of
/// `order_id` / `client_id`.
fn cancel_payload(
    wallet: &str,
    account_index: u64,
    client_id: Option<&str>,
    ct: u64,
    order_id: Option<&str>,
    market_id: u64,
) -> String {
    let mut p = format!("{{\"ad\":\"{wallet}\",\"ai\":{account_index}");
    if order_id.is_none() {
        if let Some(c) = client_id {
            p.push_str(&format!(",\"c\":\"{c}\""));
        }
    }
    p.push_str(&format!(",\"ct\":{ct}"));
    if let Some(id) = order_id {
        p.push_str(&format!(",\"id\":\"{id}\""));
    }
    p.push_str(&format!(",\"m\":{market_id},\"op\":2,\"v\":1}}"));
    p
}

/// Map the venue order-status enum onto the engine's lifecycle. Arcus
/// reports richer states (margin cancels, TPSL lifecycle, acks); anything
/// unrecognised fails closed so reconciliation keeps polling rather than
/// trusting a guessed state.
fn arcus_order_status(
    status: &str,
    filled: Decimal,
    original: Decimal,
) -> anyhow::Result<OrderStatus> {
    Ok(match status {
        "FILLED" if filled >= original && original > dec!(0) => OrderStatus::Filled,
        "FILLED" => anyhow::bail!(
            "Arcus order status FILLED but filledSize {filled} < originalSize {original}"
        ),
        "CANCELED" | "MARGIN_CANCELED" | "TPSL_CANCELED" | "LIQUIDATED" | "ADL" => {
            if filled > dec!(0) && filled < original {
                OrderStatus::PartiallyFilled
            } else {
                OrderStatus::Cancelled
            }
        }
        "REJECTED" | "ERROR" => OrderStatus::Rejected,
        "EXPIRED" => OrderStatus::Expired,
        "PARTIALLY_FILLED" => OrderStatus::PartiallyFilled,
        "PENDING"
        | "ACK"
        | "OPEN"
        | "UNTRIGGERED"
        | "TPSL_PLACED"
        | "TPSL_TRIGGERED"
        | "CANCEL_PENDING"
        | "CANCEL_ACKNOWLEDGED"
        | "CANCEL_ALL_ACKNOWLEDGED" => {
            if filled > dec!(0) {
                OrderStatus::PartiallyFilled
            } else {
                OrderStatus::New
            }
        }
        other => anyhow::bail!("Arcus order returned unknown status '{other}'"),
    })
}

#[async_trait]
impl ExchangeAdapter for ArcusAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Arcus"
    }

    fn rest_base_url(&self) -> Option<String> {
        Some(self.base_url.clone())
    }

    /// `/health` is a dedicated liveness endpoint — cheaper than a markets
    /// pull and bypasses the response cache the way a probe must.
    async fn ping(&self) -> anyhow::Result<()> {
        let url = format!("{}/health", self.base_url);
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Arcus health probe: HTTP {}", resp.status());
        }
        Ok(())
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let res = self.get_json("/v1/markets").await?;
        let rows = res
            .get("markets")
            .and_then(|m| m.as_array())
            .ok_or_else(|| anyhow::anyhow!("Arcus /v1/markets response missing markets array"))?;
        let mut markets = Vec::new();
        for m in rows {
            // Only perpetuals ride this adapter — the spot router is a
            // separate API surface (router.spot.arcus.xyz) not wired here.
            if m.get("type").and_then(|t| t.as_str()) != Some("PERPETUAL") {
                continue;
            }
            let display = m
                .get("marketDisplayName")
                .and_then(|d| d.as_str())
                .unwrap_or("");
            let Some((base, quote)) = display.rsplit_once('-') else {
                continue;
            };
            if base.is_empty() || quote.is_empty() {
                continue;
            }
            let str_dec = |key: &str| -> Option<Decimal> {
                m.get(key)
                    .and_then(|v| v.as_str())
                    .and_then(|s| Decimal::from_str_exact(s).ok())
            };
            if let (Some(id), Some(tick), Some(step)) = (
                m.get("marketId").and_then(|v| v.as_u64()),
                str_dec("tickSize").filter(|t| *t > dec!(0)),
                str_dec("stepSize").filter(|s| *s > dec!(0)),
            ) {
                // Dynamic price bands: `tickTiers` entries carry a `tick`
                // plus an optional `upToPrice` (EXCLUSIVE upper bound; the
                // final tier omits it). Sort ascending with the unbounded
                // tier last; keep only positive ticks — a malformed entry
                // narrows to the top-level tick rather than misquantizing.
                let mut tick_tiers: Vec<(Option<Decimal>, Decimal)> = m
                    .get("tickTiers")
                    .and_then(|t| t.as_array())
                    .map(|tiers| {
                        tiers
                            .iter()
                            .filter_map(|t| {
                                let tick = t
                                    .get("tick")
                                    .and_then(|v| v.as_str())
                                    .and_then(|s| Decimal::from_str_exact(s).ok())
                                    .filter(|x| *x > dec!(0))?;
                                let up = t
                                    .get("upToPrice")
                                    .and_then(|v| v.as_str())
                                    .and_then(|s| Decimal::from_str_exact(s).ok())
                                    .filter(|x| *x > dec!(0));
                                Some((up, tick))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                tick_tiers.sort_by(|(a, _), (b, _)| match (a, b) {
                    (None, None) => std::cmp::Ordering::Equal,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (Some(x), Some(y)) => x.cmp(y),
                });
                self.market_specs.write().insert(
                    display.to_string(),
                    ArcusMarketSpec {
                        market_id: id,
                        tick_size: tick,
                        step_size: step,
                        tick_tiers,
                    },
                );
            }
            markets.push(MarketMetadata {
                symbol: Symbol::perp(base, quote),
                venue: self.venue_id.clone(),
                venue_symbol: display.to_string(),
                price_decimals: m
                    .get("tickSize")
                    .and_then(|t| t.as_str())
                    .map(decimals_of)
                    .unwrap_or(2),
                size_decimals: m
                    .get("stepSize")
                    .and_then(|s| s.as_str())
                    .map(decimals_of)
                    .unwrap_or(3),
                min_order_size: str_dec("minOrderSize").unwrap_or(dec!(0.0001)),
                min_notional: str_dec("minOrderNotional").unwrap_or(dec!(5.0)),
                // Base fee tier, verified against GET /v1/feetiers (level 0:
                // 150ppm maker / 450ppm taker). Higher-volume tiers improve
                // on this — using the base tier keeps route economics
                // conservative rather than flattering.
                maker_fee_bps: dec!(1.5),
                taker_fee_bps: dec!(4.5),
                contract_size: dec!(1.0),
                is_active: m.get("status").and_then(|s| s.as_str()) == Some("ONLINE"),
            });
        }
        Ok(markets)
    }

    /// `/v1/mids` is the venue's bulk mid-price map keyed by market name —
    /// exactly what the ticker universe seeds from.
    async fn fetch_market_prices(
        &self,
    ) -> anyhow::Result<std::collections::HashMap<String, Decimal>> {
        let res = self.get_json("/v1/mids").await?;
        let mut out = std::collections::HashMap::new();
        if let Some(mids) = res.get("mids").and_then(|m| m.as_object()) {
            for (market, price) in mids {
                if let Some(p) = price.as_str().and_then(|s| Decimal::from_str_exact(s).ok()) {
                    if p > dec!(0) {
                        out.insert(market.clone(), p);
                    }
                }
            }
        }
        Ok(out)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let market = arcus_market_name(symbol);
        let path = format!("/v1/l2OrderBook/{}?nLevels=50", market);
        let res = self.get_json(&path).await?;

        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());
        book.timestamp = Utc::now();

        let parse_side = |side: Option<&serde_json::Value>, dst: &mut Vec<BookLevel>| {
            if let Some(levels) = side.and_then(|s| s.as_array()) {
                for lvl in levels {
                    let Some(pair) = lvl.as_array() else { continue };
                    let p = pair
                        .first()
                        .and_then(|v| v.as_str())
                        .and_then(|s| Decimal::from_str_exact(s).ok());
                    let s = pair
                        .get(1)
                        .and_then(|v| v.as_str())
                        .and_then(|s| Decimal::from_str_exact(s).ok());
                    if let (Some(p), Some(s)) = (p, s) {
                        if p > dec!(0) && s > dec!(0) {
                            dst.push(BookLevel::new(p, s));
                        }
                    }
                }
            }
        };
        parse_side(res.get("bids"), &mut book.bids);
        parse_side(res.get("asks"), &mut book.asks);

        if book.bids.is_empty() && book.asks.is_empty() {
            anyhow::bail!(
                "No live orderbook data returned from Arcus API for {}",
                symbol
            );
        }

        Ok(book)
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let book = self.fetch_orderbook(symbol).await?;
        let bid = book.best_bid_price().unwrap_or(dec!(0));
        let ask = book.best_ask_price().unwrap_or(dec!(0));
        Ok(Ticker {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            bid,
            ask,
            last: (bid + ask) / dec!(2),
            timestamp: Utc::now(),
        })
    }

    /// Arcus funding is hourly; `GET /v1/markets` carries the current
    /// `fundingRate`, the projected `nextFundingRate`, and the epoch-second
    /// `nextFundingAt` per market — the venue's own print, not a default.
    async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate> {
        let market = arcus_market_name(symbol);
        let res = self.get_json("/v1/markets").await?;
        let rows = res
            .get("markets")
            .and_then(|m| m.as_array())
            .ok_or_else(|| anyhow::anyhow!("Arcus /v1/markets response missing markets array"))?;
        let m = rows
            .iter()
            .find(|r| r.get("marketDisplayName").and_then(|d| d.as_str()) == Some(market.as_str()))
            .ok_or_else(|| anyhow::anyhow!("Arcus market {} not listed", market))?;
        // Fail-closed: a missing print is an outage, not a 0% print — a
        // silent zero would mint phantom carry against any nonzero leg.
        let rate = m
            .get("fundingRate")
            .and_then(|r| r.as_str())
            .and_then(|s| Decimal::from_str_exact(s).ok())
            .ok_or_else(|| {
                anyhow::anyhow!("Arcus funding response missing fundingRate for {symbol}")
            })?;
        let predicted = m
            .get("nextFundingRate")
            .and_then(|r| r.as_str())
            .and_then(|s| Decimal::from_str_exact(s).ok());
        let next_at = m
            .get("nextFundingAt")
            .and_then(|t| t.as_i64())
            .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0))
            .unwrap_or_else(|| Utc::now() + chrono::Duration::hours(1));
        Ok(FundingRate {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            rate,
            predicted_rate: predicted,
            interval_hours: 1,
            next_funding_time: next_at,
            timestamp: Utc::now(),
        })
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        // `GET /v1/account` is a public endpoint scoped by the master
        // wallet address — only the stored address + subaccount index are
        // needed, no signature.
        let wallet = {
            let guard = self.wallet_address.read();
            guard
                .as_deref()
                .map(str::trim)
                .filter(|w| !w.is_empty())
                .map(str::to_string)
        }
        .ok_or_else(|| anyhow::anyhow!("Arcus account fetch requires the master wallet address"))?;
        let idx = self.account_index.read().unwrap_or(0);
        let path = format!("/v1/account?address={wallet}&accountIndex={idx}");

        let empty_snapshot = |venue: VenueId| AccountSnapshot {
            venue,
            equity: dec!(0),
            free_margin: dec!(0),
            maintenance_margin: dec!(0),
            margin_usage_pct: dec!(0),
            balances: Vec::new(),
            positions: Vec::new(),
            timestamp: Utc::now(),
        };

        // Status-aware fetch (the shared `get_json` bails on non-2xx and
        // the venue returns 404 for an address that has never deposited —
        // a truthful empty account, not an error).
        let data: serde_json::Value =
            match self.rate_limiter.acquire_or_cache_path(&path, true).await? {
                Some(cached) => serde_json::from_str(&cached)?,
                None => {
                    let url = format!("{}{}", self.base_url, path);
                    let resp = match self.client.get(&url).send().await {
                        Ok(r) => r,
                        Err(e) => {
                            self.rate_limiter.record_failure(false);
                            return Err(e.into());
                        }
                    };
                    if resp.status() == reqwest::StatusCode::NOT_FOUND {
                        return Ok(empty_snapshot(self.venue_id.clone()));
                    }
                    if !resp.status().is_success() {
                        let is_429 = resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS;
                        self.rate_limiter.record_failure(is_429);
                        anyhow::bail!("Arcus API error: {} {}", resp.status(), path);
                    }
                    let text = resp.text().await?;
                    self.rate_limiter.record_success_path(&path, text.clone());
                    serde_json::from_str(&text)?
                }
            };
        // Defensive: some gateway builds echo {"error": ...} on 200 for
        // the same never-deposited case — treat it like the 404.
        if data.get("accountIndex").is_none() && data.get("equity").is_none() {
            return Ok(empty_snapshot(self.venue_id.clone()));
        }

        let dec_of = |v: Option<&serde_json::Value>| -> Decimal {
            v.and_then(|v| v.as_str())
                .and_then(|s| Decimal::from_str_exact(s).ok())
                .unwrap_or(dec!(0))
        };
        let equity = dec_of(data.get("equity"));
        let free = dec_of(data.get("freeCollateral"));
        let used = (equity - free).max(dec!(0));

        let mut positions = Vec::new();
        if let Some(map) = data.get("positions").and_then(|p| p.as_object()) {
            for (_market_id, p) in map {
                let size = dec_of(p.get("size"));
                if size == dec!(0) {
                    continue;
                }
                let side = match p
                    .get("side")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_ascii_uppercase()
                    .as_str()
                {
                    "SHORT" => Side::Sell,
                    _ => Side::Buy,
                };
                let sym = p
                    .get("marketDisplayName")
                    .and_then(|s| s.as_str())
                    .unwrap_or("UNKNOWN")
                    .to_string();
                positions.push(Position {
                    symbol: sym,
                    venue: self.venue_id.clone(),
                    side,
                    quantity: size.abs(),
                    entry_price: dec_of(p.get("averageEntryPrice")),
                    mark_price: dec_of(p.get("markPx")),
                    unrealized_pnl: dec_of(p.get("unrealizedPnl")),
                    realized_pnl: dec!(0),
                    liquidation_price: None,
                    leverage: dec_of(p.get("leverage")),
                    initial_margin: dec_of(p.get("marginUsed")),
                    maintenance_margin: dec_of(p.get("marginUsed")),
                    updated_at: Utc::now(),
                });
            }
        }

        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity,
            free_margin: free,
            maintenance_margin: used,
            margin_usage_pct: if equity > dec!(0) {
                (used / equity) * dec!(100)
            } else {
                dec!(0)
            },
            balances: vec![Balance {
                venue: self.venue_id.clone(),
                currency: "USDG".to_string(),
                total: equity,
                free,
                locked: used,
            }],
            positions,
            timestamp: Utc::now(),
        })
    }

    /// Wire grid = the market's `stepSize`, floored — the same snap
    /// `place_order` applies before signing, so pair sizing sees the true
    /// tradeable size instead of discovering the grid via a 400.
    async fn wire_base_qty(&self, symbol: &str, qty: Decimal) -> anyhow::Result<Decimal> {
        let market = arcus_market_name(symbol);
        let spec = self.market_spec(&market).await?;
        Ok((qty / spec.step_size).floor() * spec.step_size)
    }

    /// `POST /v1/placeOrder` — Ed25519-signed typed payload (Scheme 1).
    /// The response is asynchronous: `202`/`ACK` carries no terminal state,
    /// so this maps to `New` and truth comes from `fetch_order` polling.
    /// `goodTilTime` is mandatory on every order (even IOC/FOK — it acts as
    /// a replay-protection expiry); we send now+40d, comfortably past the
    /// venue's one-month minimum.
    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        let (key, api_key, wallet, ai) = self.order_creds()?;
        let market = arcus_market_name(&order.symbol);
        let spec = self.market_spec(&market).await?;

        let is_market = order.order_type == OrderType::Market;
        let post_only = order.post_only || order.order_type == OrderType::PostOnly;
        if is_market && post_only {
            anyhow::bail!("Arcus post-only market order is contradictory");
        }
        let (mut tif_str, mut t_code) = arcus_tif(order.time_in_force, post_only);
        if is_market && t_code == 0 {
            // MARKET orders only accept IOC/FOK — an immediate order with
            // a resting TIF is a schema error, not intent, so coerce.
            tif_str = "IOC";
            t_code = 2;
        }
        if is_market && t_code == 3 {
            anyhow::bail!("Arcus MARKET orders cannot be ALO");
        }

        // Venue prices must land exactly on tick multiples. LIMIT prices
        // snap inward (buy down / sell up) so the wire price never exceeds
        // the caller's bound. MARKET orders still carry a mandatory
        // protective price bound within 10% of mark; derive one from the
        // mid when the caller didn't supply a bound — snapped looser
        // (buy up / sell down) so the signed tick integer stays aligned.
        let price = match order.price {
            Some(p) if !is_market => Self::snap_price_tiered(p, &spec, order.side, false),
            Some(p) => Self::snap_price_tiered(p, &spec, order.side, true),
            None if is_market => {
                let mids = self.fetch_market_prices().await?;
                let mid = *mids.get(&market).ok_or_else(|| {
                    anyhow::anyhow!(
                        "Arcus has no mid price for {market}; cannot bound market order"
                    )
                })?;
                let bound = match order.side {
                    Side::Buy => mid * dec!(1.05),
                    Side::Sell => mid * dec!(0.95),
                };
                Self::snap_price_tiered(bound, &spec, order.side, true)
            }
            None => anyhow::bail!("Arcus limit order requires a price"),
        };

        let p = Self::exact_units(price, spec.tick_size_at(price), "price", &market)?;
        // Quantities floor to the step grid — never size up on the wire.
        let qty = (order.quantity / spec.step_size).floor() * spec.step_size;
        let q = Self::exact_units(qty, spec.step_size, "quantity", &market)?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| anyhow::anyhow!("system clock before epoch: {e}"))?;
        let ct = now.as_nanos() as u64;
        let gtt_micros = now.as_micros() as u64 + 40 * 24 * 60 * 60 * 1_000_000u64;
        let g = gtt_micros * 1000;
        let s = match order.side {
            Side::Buy => 0u64,
            Side::Sell => 1,
        };
        let client_id = arcus_client_id(&order.client_order_id);
        let payload = place_payload(
            &wallet,
            ai,
            client_id.as_deref(),
            ct,
            g,
            spec.market_id,
            p,
            q,
            order.reduce_only,
            s,
            t_code,
        );
        let signature = self.sign(&key, &payload);

        let mut body = json!({
            "address": wallet,
            "marketId": spec.market_id,
            "accountIndex": ai,
            "orderSide": if s == 0 { "BUY" } else { "SELL" },
            "orderType": if is_market { "MARKET" } else { "LIMIT" },
            // The wire quantity is the step-floored value the signature
            // covers — posting the raw request would 400 on the market's
            // size increment while the signed payload claims the snap.
            "quantity": qty.to_string(),
            "price": price.to_string(),
            "timeInForce": tif_str,
            "goodTilTime": gtt_micros.to_string(),
            "clientTime": ct.to_string(),
            "reduceOnly": order.reduce_only,
        });
        if let Some(c) = &client_id {
            body["clientId"] = json!(c);
        }

        let path = format!("/v1/placeOrder?address={wallet}");
        let (status_code, data, raw) = self
            .signed_post(&path, body, &api_key, ct, &signature)
            .await?;
        let mut resp = self.order_response(&data, &order.symbol, order.quantity, &raw)?;
        if status_code == reqwest::StatusCode::ACCEPTED {
            // Async ack: no terminal state yet — report New and let
            // reconciliation poll `fetch_order` for the truth.
            resp.status = OrderStatus::New;
            resp.filled_qty = dec!(0);
            resp.remaining_qty = order.quantity;
        }
        if resp.client_order_id.is_empty() {
            resp.client_order_id = order.client_order_id.clone();
        }
        Ok(resp)
    }

    /// `POST /v1/cancelOrder` — cancels by server `order_id` or `client_id`
    /// (exactly one; server id preferred). A `200` reporting `FILLED` means
    /// the order completed before the cancel landed — terminal either way,
    /// so it is not an error; `fetch_order` reports the truth.
    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
        let (key, api_key, wallet, ai) = self.order_creds()?;
        let market = arcus_market_name(&req.symbol);
        let spec = self.market_spec(&market).await?;

        let by_id = req
            .order_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let by_client = req
            .client_order_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(arcus_client_id);
        if by_id.is_none() && by_client.is_none() {
            anyhow::bail!("Arcus cancel requires an order_id or client_order_id");
        }

        let ct = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| anyhow::anyhow!("system clock before epoch: {e}"))?
            .as_nanos() as u64;

        let payload = cancel_payload(&wallet, ai, by_client.as_deref(), ct, by_id, spec.market_id);
        let signature = self.sign(&key, &payload);

        let mut body = json!({
            "address": wallet,
            "marketId": spec.market_id,
            "accountIndex": ai,
            "timestamp": ct,
        });
        if let Some(id) = by_id {
            body["orderId"] = json!(id);
        } else if let Some(c) = by_client {
            body["clientId"] = json!(c);
        }

        let path = format!("/v1/cancelOrder?address={wallet}");
        let (status_code, data, raw) = self
            .signed_post(&path, body, &api_key, ct, &signature)
            .await?;
        if status_code == reqwest::StatusCode::ACCEPTED {
            return Ok(());
        }
        match data.get("status").and_then(|v| v.as_str()) {
            Some("CANCELED") | Some("FILLED") => Ok(()),
            Some("REJECTED") => {
                let reason = data
                    .get("rejectionReason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                anyhow::bail!("Arcus cancel rejected ({reason}): {raw}")
            }
            other => anyhow::bail!("Arcus cancel returned unexpected status {other:?}: {raw}"),
        }
    }

    /// `GET /v1/order/{orderId}?address=…` — public read, so no signature
    /// is needed; the wallet address + subaccount scope the lookup. Order
    /// truth for the reconciliation supervisor.
    async fn fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        _client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        let wallet = {
            let guard = self.wallet_address.read();
            guard
                .as_deref()
                .map(str::trim)
                .filter(|w| !w.is_empty())
                .map(|w| w.to_ascii_lowercase())
        }
        .ok_or_else(|| anyhow::anyhow!("Arcus order fetch requires the master wallet address"))?;
        let ai = self.account_index.read().unwrap_or(0);
        let oid = order_id.trim();
        if oid.is_empty() {
            anyhow::bail!("Arcus order fetch requires a server order id");
        }
        let path = format!("/v1/order/{oid}?address={wallet}&accountIndex={ai}");
        if self
            .rate_limiter
            .acquire_or_cache_path(&path, false)
            .await?
            .is_some()
        {
            anyhow::bail!("Arcus order fetch unexpectedly served from cache");
        }
        let url = format!("{}{}", self.base_url, path);
        let resp = match self.client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                self.rate_limiter.record_failure(false);
                return Err(e.into());
            }
        };
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            anyhow::bail!("Arcus order {oid} not found for {wallet}#{ai}");
        }
        if !resp.status().is_success() {
            let is_429 = resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS;
            self.rate_limiter.record_failure(is_429);
            anyhow::bail!("Arcus API error: {} {}", resp.status(), path);
        }
        let raw = resp.text().await?;
        self.rate_limiter.record_success_path(&path, raw.clone());
        let data = serde_json::from_str::<serde_json::Value>(&raw)?;

        // Identity check — the response must describe the order we asked
        // about, on the market the caller expects.
        let returned_id = data
            .get("orderId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Arcus order response missing orderId: {raw}"))?;
        if returned_id != oid {
            anyhow::bail!("Arcus order fetch returned {returned_id} for query {oid}");
        }
        let market = arcus_market_name(symbol);
        if let Some(name) = data.get("marketDisplayName").and_then(|v| v.as_str()) {
            if !name.eq_ignore_ascii_case(&market) {
                anyhow::bail!("Arcus order {oid} is on {name}, expected {market}");
            }
        }
        self.order_response(&data, symbol, dec!(0), &raw)
    }

    /// Venue position rows carry `marketDisplayName` (`BTC-USD`); engine
    /// signal legs and `account_position_for` queries speak the canonical
    /// `BASE-USDC` family — map or flatten/residual checks read a real
    /// position as flat (observed 2026-09 on a live flatten).
    fn normalize_account_position(&self, position: &Position) -> anyhow::Result<Option<Position>> {
        let mut position = position.clone();
        let base = canonical_base(&position.symbol);
        if base.is_empty() {
            anyhow::bail!(
                "Arcus account position symbol '{}' is not a standard BASE-USD market",
                position.symbol
            );
        }
        position.symbol = format!("{base}-USDC");
        Ok(Some(position))
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
        // Signing material must parse — a malformed secret that only
        // exists as a string cannot authenticate anything.
        let has_key = self.ed25519.read().is_some();
        let has_wallet = self
            .wallet_address
            .read()
            .as_deref()
            .map(str::trim)
            .is_some_and(|w| !w.is_empty());
        has_key && has_wallet
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_name_reduces_engine_symbols_to_venue_spelling() {
        // Arcus quotes USD on every perp; all engine spellings reduce to
        // the BASE-USD venue name.
        assert_eq!(arcus_market_name("BTC-USD"), "BTC-USD");
        assert_eq!(arcus_market_name("BTC-USDC"), "BTC-USD");
        assert_eq!(arcus_market_name("BTC"), "BTC-USD");
        assert_eq!(arcus_market_name("amd"), "AMD-USD");
    }

    #[test]
    fn decimals_of_parses_string_grids() {
        assert_eq!(decimals_of("0.1"), 1);
        assert_eq!(decimals_of("0.00000001"), 8);
        assert_eq!(decimals_of("5"), 0);
        assert_eq!(decimals_of("junk"), 0);
    }

    #[tokio::test]
    async fn order_paths_fail_closed_without_credentials() {
        let a = ArcusAdapter::new(
            Some("https://api.testnet.arcus.xyz".to_string()),
            None,
            None,
        );
        let order = OrderRequest::new_limit(
            VenueId::new("arcus"),
            "BTC-USD",
            arbitrageur_core::types::Side::Buy,
            dec!(80000.0),
            dec!(0.001),
        );
        let err = a.place_order(order).await.unwrap_err().to_string();
        assert!(err.contains("Ed25519"), "unexpected refusal: {err}");
        assert!(!a.has_live_credentials());
    }

    #[test]
    fn signing_key_parses_seed_and_keypair_hex() {
        let seed = [7u8; 32];
        let hex_seed = hex::encode(seed);
        let parsed = parse_signing_key(&hex_seed).expect("32-byte seed parses");
        assert_eq!(parsed.to_bytes(), seed);

        // 0x-prefixed and 64-byte seed||pubkey encodings both accepted.
        assert!(parse_signing_key(&format!("0x{hex_seed}")).is_some());
        let mut kp = [0u8; 64];
        kp[..32].copy_from_slice(&seed);
        kp[32..].copy_from_slice(&parsed.verifying_key().to_bytes());
        let parsed64 = parse_signing_key(&hex::encode(kp)).expect("64-byte keypair parses");
        assert_eq!(parsed64.to_bytes(), seed);

        assert!(parse_signing_key("zz").is_none());
        assert!(parse_signing_key(&hex::encode([0u8; 16])).is_none());
    }

    #[test]
    fn derived_public_key_matches_secret() {
        // ed25519-dalek test vector shape: pubkey is derivable from seed.
        let seed = [9u8; 32];
        let key = ed25519_dalek::SigningKey::from_bytes(&seed);
        assert_eq!(
            verifying_key_hex(&key),
            hex::encode(key.verifying_key().to_bytes())
        );
        assert_eq!(verifying_key_hex(&key).len(), 64);
    }

    #[test]
    fn place_payload_is_canonical_key_sorted_json() {
        let p = place_payload(
            "0xabc",
            0,
            Some("cid-1"),
            1713825891591000000,
            1717279891591000000,
            1,
            836725,
            10000000,
            false,
            0,
            2,
        );
        assert_eq!(
            p,
            "{\"ad\":\"0xabc\",\"ai\":0,\"c\":\"cid-1\",\"ct\":1713825891591000000,\"g\":1717279891591000000,\"m\":1,\"op\":1,\"p\":836725,\"q\":10000000,\"r\":0,\"s\":0,\"t\":2,\"v\":1}"
        );
        // `c` omitted entirely when no usable client id.
        let p2 = place_payload("0xabc", 2, None, 1, 2, 3, 4, 5, true, 1, 0);
        assert_eq!(
            p2,
            "{\"ad\":\"0xabc\",\"ai\":2,\"ct\":1,\"g\":2,\"m\":3,\"op\":1,\"p\":4,\"q\":5,\"r\":1,\"s\":1,\"t\":0,\"v\":1}"
        );
    }

    #[test]
    fn cancel_payload_targets_exactly_one_id() {
        let p = cancel_payload("0xabc", 0, Some("cid-1"), 42, Some("oid-9"), 1);
        // Server id wins; client id stays out of the signed payload.
        assert_eq!(
            p,
            "{\"ad\":\"0xabc\",\"ai\":0,\"ct\":42,\"id\":\"oid-9\",\"m\":1,\"op\":2,\"v\":1}"
        );
        let p = cancel_payload("0xabc", 0, Some("cid-1"), 42, None, 1);
        assert_eq!(
            p,
            "{\"ad\":\"0xabc\",\"ai\":0,\"c\":\"cid-1\",\"ct\":42,\"m\":1,\"op\":2,\"v\":1}"
        );
    }

    #[test]
    fn signature_verifies_against_derived_pubkey() {
        use ed25519_dalek::Verifier;
        let key = ed25519_dalek::SigningKey::from_bytes(&[3u8; 32]);
        let payload = place_payload("0xabc", 0, None, 1, 2, 1, 100, 50, false, 0, 0);
        let sig_hex = {
            let a = ArcusAdapter::new(None, None, None);
            a.sign(&key, &payload)
        };
        assert_eq!(sig_hex.len(), 128);
        let sig_bytes = hex::decode(&sig_hex).expect("hex sig");
        let sig = ed25519_dalek::Signature::from_slice(&sig_bytes).expect("sig");
        key.verifying_key()
            .verify(payload.as_bytes(), &sig)
            .expect("signature verifies over canonical payload");
    }

    #[test]
    fn client_id_charset_enforced() {
        assert_eq!(arcus_client_id("abc-DEF_123"), Some("abc-DEF_123".into()));
        // UUIDs the engine generates are 36 chars — the venue max.
        assert_eq!(
            arcus_client_id("123e4567-e89b-12d3-a456-426614174000").map(|s| s.len()),
            Some(36)
        );
        assert_eq!(arcus_client_id(""), None);
        assert_eq!(arcus_client_id("bad id!"), None);
        assert_eq!(arcus_client_id(&"x".repeat(37)), None);
    }

    #[test]
    fn order_status_mapping_covers_terminal_states() {
        assert!(matches!(
            arcus_order_status("FILLED", dec!(1), dec!(1)).unwrap(),
            OrderStatus::Filled
        ));
        assert!(matches!(
            arcus_order_status("PARTIALLY_FILLED", dec!(0.5), dec!(1)).unwrap(),
            OrderStatus::PartiallyFilled
        ));
        assert!(matches!(
            arcus_order_status("MARGIN_CANCELED", dec!(0), dec!(1)).unwrap(),
            OrderStatus::Cancelled
        ));
        // A cancel that landed after partial fills keeps the fill truth.
        assert!(matches!(
            arcus_order_status("CANCELED", dec!(0.5), dec!(1)).unwrap(),
            OrderStatus::PartiallyFilled
        ));
        assert!(matches!(
            arcus_order_status("REJECTED", dec!(0), dec!(1)).unwrap(),
            OrderStatus::Rejected
        ));
        assert!(matches!(
            arcus_order_status("ACK", dec!(0), dec!(1)).unwrap(),
            OrderStatus::New
        ));
        assert!(matches!(
            arcus_order_status("CANCEL_PENDING", dec!(0), dec!(1)).unwrap(),
            OrderStatus::New
        ));
        // Unknown states fail closed — reconciliation keeps polling.
        assert!(arcus_order_status("BOGUS", dec!(0), dec!(1)).is_err());
        // FILLED with less than full size is inconsistent — fail closed.
        assert!(arcus_order_status("FILLED", dec!(0.5), dec!(1)).is_err());
    }

    #[tokio::test]
    async fn creds_reject_pubkey_mismatch() {
        let a = ArcusAdapter::new(None, None, None);
        let key = ed25519_dalek::SigningKey::from_bytes(&[5u8; 32]);
        let wrong_pub = hex::encode([0u8; 32]);
        a.update_credentials(
            Some(wrong_pub),
            Some(hex::encode(key.to_bytes())),
            Some("0xE3abF3D7E567A17D3f6Ae4Ebd4896654ac544eb8".into()),
            None,
        );
        let err = a.order_creds().unwrap_err().to_string();
        assert!(err.contains("does not match"), "unexpected error: {err}");
        // With matching pubkey the same secret resolves.
        a.update_credentials(
            Some(verifying_key_hex(&key)),
            Some(hex::encode(key.to_bytes())),
            Some("0xE3abF3D7E567A17D3f6Ae4Ebd4896654ac544eb8".into()),
            None,
        );
        let (_k, api, wallet, ai) = a.order_creds().expect("creds resolve");
        assert_eq!(api, verifying_key_hex(&key));
        // `ad` is the only case-folded payload field — always lowercase.
        assert_eq!(wallet, "0xe3abf3d7e567a17d3f6ae4ebd4896654ac544eb8");
        assert_eq!(ai, 0);
        assert!(a.has_live_credentials());
    }

    #[test]
    fn snap_to_tick_preserves_limit_bounds_and_loosens_market_guards() {
        let tick = dec!(0.1);
        // The real testnet failure: engine-derived sell at 84567.99485 is
        // off-grid. Limit snap keeps the bound — sell goes UP to 84568.0
        // (never sells below intent); buy goes DOWN to 84567.9.
        assert_eq!(
            ArcusAdapter::snap_to_tick(dec!(84567.99485), tick, Side::Sell, false),
            dec!(84568.0)
        );
        assert_eq!(
            ArcusAdapter::snap_to_tick(dec!(84567.99485), tick, Side::Buy, false),
            dec!(84567.9)
        );
        // Market protective bounds snap looser — the opposite direction.
        assert_eq!(
            ArcusAdapter::snap_to_tick(dec!(84567.99485), tick, Side::Sell, true),
            dec!(84567.9)
        );
        assert_eq!(
            ArcusAdapter::snap_to_tick(dec!(84567.99485), tick, Side::Buy, true),
            dec!(84568.0)
        );
        // Already-aligned prices are untouched.
        assert_eq!(
            ArcusAdapter::snap_to_tick(dec!(84568.0), tick, Side::Buy, false),
            dec!(84568.0)
        );
    }

    #[test]
    fn tick_size_at_resolves_exclusive_price_bands() {
        let spec = ArcusMarketSpec {
            market_id: 1,
            tick_size: dec!(0.01),
            step_size: dec!(0.001),
            tick_tiers: vec![
                (Some(dec!(100)), dec!(0.01)),
                (Some(dec!(1000)), dec!(0.1)),
                (None, dec!(1)),
            ],
        };
        // Below the first bound -> base-band tick.
        assert_eq!(spec.tick_size_at(dec!(0.0004)), dec!(0.01));
        assert_eq!(spec.tick_size_at(dec!(99.999)), dec!(0.01));
        // `upToPrice` is EXCLUSIVE: exactly on the bound -> next tier.
        assert_eq!(spec.tick_size_at(dec!(100)), dec!(0.1));
        assert_eq!(spec.tick_size_at(dec!(999.9)), dec!(0.1));
        assert_eq!(spec.tick_size_at(dec!(1000)), dec!(1));
        assert_eq!(spec.tick_size_at(dec!(50_000)), dec!(1));
    }

    #[test]
    fn tick_size_at_falls_back_to_top_level_tick_without_tiers() {
        let spec = ArcusMarketSpec {
            market_id: 1,
            tick_size: dec!(0.05),
            step_size: dec!(0.01),
            tick_tiers: vec![],
        };
        assert_eq!(spec.tick_size_at(dec!(3.1415)), dec!(0.05));
    }

    #[test]
    fn snap_price_tiered_uses_the_snapped_price_band() {
        let spec = ArcusMarketSpec {
            market_id: 1,
            tick_size: dec!(0.01),
            step_size: dec!(0.001),
            tick_tiers: vec![
                (Some(dec!(100)), dec!(0.01)),
                (Some(dec!(1000)), dec!(0.1)),
                (None, dec!(1)),
            ],
        };
        // Limit buy just under the 100 bound: ticks at 0.01, stays in-band.
        assert_eq!(
            ArcusAdapter::snap_price_tiered(dec!(99.996), &spec, Side::Buy, false),
            dec!(99.99)
        );
        // Loosened buy bound crossing the bound: 99.995 -> ceil to 100.00,
        // re-resolves to the 0.1 band and stays aligned on it.
        assert_eq!(
            ArcusAdapter::snap_price_tiered(dec!(99.995), &spec, Side::Buy, true),
            dec!(100.0)
        );
        // Loosened buy bound just under 1000: lands on 1000.0, re-resolves
        // to the 1.0 band.
        assert_eq!(
            ArcusAdapter::snap_price_tiered(dec!(999.95), &spec, Side::Buy, true),
            dec!(1000)
        );
        // Limit sell above 1000 must never go below the caller bound:
        // 1050.3 already aligns on the 1.0 grid.
        assert_eq!(
            ArcusAdapter::snap_price_tiered(dec!(1050.3), &spec, Side::Sell, false),
            dec!(1051)
        );
        // Market-guard sell loosens DOWN onto the coarser band.
        assert_eq!(
            ArcusAdapter::snap_price_tiered(dec!(1050.3), &spec, Side::Sell, true),
            dec!(1050)
        );
    }

    #[test]
    fn normalize_account_position_maps_venue_usd_to_engine_usdc() {
        let a = ArcusAdapter::new(None, None, None);
        let pos = Position {
            venue: a.venue_id.clone(),
            symbol: "BTC-USD".to_string(),
            side: Side::Sell,
            quantity: dec!(0.001),
            entry_price: dec!(84541.3),
            mark_price: dec!(83998.3),
            unrealized_pnl: dec!(0.543),
            realized_pnl: dec!(0),
            liquidation_price: None,
            leverage: dec!(40),
            initial_margin: dec!(2.1),
            maintenance_margin: dec!(2.1),
            updated_at: Utc::now(),
        };
        let out = a
            .normalize_account_position(&pos)
            .expect("standard market maps")
            .expect("row managed");
        assert_eq!(out.symbol, "BTC-USDC");
        // A real position row must resolve for the engine symbol a signal
        // leg carries — the flatten path depends on this exact match.
        let account = AccountSnapshot {
            venue: a.venue_id.clone(),
            equity: dec!(1000),
            free_margin: dec!(1000),
            maintenance_margin: dec!(0),
            margin_usage_pct: dec!(0),
            balances: vec![],
            positions: vec![pos],
            timestamp: Utc::now(),
        };
        let resolved = a
            .account_position_for(&account, "BTC-USDC")
            .expect("resolves")
            .expect("position found");
        assert_eq!(resolved.quantity, dec!(0.001));
        assert_eq!(resolved.side, Side::Sell);
    }
}
