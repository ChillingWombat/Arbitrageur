//! DEX perpetual-venue adapters — spec-driven, fail-closed.
//!
//! # What lives here
//! One [`DexPerpAdapter`] struct serves many perp venues through a
//! per-venue [`VenueApiSpec`] table. Every venue shares verified
//! market-data paths (markets / books / tickers / funding). Six venues
//! additionally implement the authenticated order paths with
//! venue-native signing (see "Live trading" below); every other venue's
//! account / order / leverage paths fail closed with an explanatory
//! error — never a fabricated snapshot or fill.
//!
//! # Live trading (venue-native signing, audited flows)
//! - **Pacifica** — Ed25519 wallet signature over deterministic compact
//!   JSON ops (`create_order` / `create_market` / `cancel_order` /
//!   `update_leverage`), base58 signature; `account`/`positions`/
//!   `orders` reads for reconciliation.
//! - **StandX** — wallet sign-in → JWT, ephemeral Ed25519 request key;
//!   signed bodies for `new_order` / `cancel_order` / `change_leverage`,
//!   `query_order` / `query_positions` / `query_balance` reads.
//! - **Ondo Perps** — EIP-4361 (SIWE) challenge → JWT; REST
//!   `POST/GET/DELETE /v1/perps/orders`, `balance`, `positions`.
//! - **GRVT** — API-key login → `gravity` session cookie +
//!   `x-grvt-account-id`; EIP-712 `Order` signatures (legs signed with
//!   the instrument's `instrument_hash` assetID) on `trades.grvt.io`.
//! - **Derive** — `X-LyraWallet`/`X-LyraTimestamp`/`X-LyraSignature`
//!   headers plus EIP-712 action signatures against the onchain
//!   TradeModule encoding.
//! - **Aevo** — `AEVO-KEY`/`AEVO-SECRET` headers plus EIP-712 `Order`
//!   signatures (Aevo Mainnet domain, chain 1).
//!
//! # Verification status (2026-09-22, live probes from this sandbox)
//! - LIVE-VERIFIED market data (real payloads, parsers match keys):
//!   dYdX v4, Paradex, Aevo, GMX V2 (prices/tickers only — the GLV AMM
//!   has no orderbook by design), ApeX Omni, Extended, Derive, StandX,
//!   Helix via Injective LCD, Pacifica, edgeX (contract metadata),
//!   GRVT (all_instruments / book / ticker / funding), Ondo Perps
//!   (markets / depth / funding_rates), QFEX markets only
//!   (`api.qfex.com/refdata` — books/tickers/trades are auth-gated and
//!   fail closed with an explanatory error).
//! - DOC-SHAPED (admitted paper-only per operator request; parsers fail
//!   closed per entry so a wrong guess degrades to venue errors, never bad
//!   data): Vest (api host Cloudflare-530s), Antarctic, EVEDEX
//!   (`api.evedex.com` DNS-fails; `exchange.evedex.com` 404s), BULK
//!   (`api.bulk.exchange` serves an unrelated landing page), Ostium
//!   (onchain Arbitrum reads — no public REST; SDK needs key+RPC),
//!   SoDEX + AlphaX (parked `/lander` domains), Nado (app live, no public
//!   API found), IO Trader (parked page), Niza.fun (no futures product
//!   found), Astros (perp frontend live, no public REST found — DeepBook
//!   onchain), Gate DEX (no such venue found).
//! - NOT ADMITTED as venues: Orderly "Derivatives" (same rails as Vanta —
//!   enabling a second Orderly venue would double-count one orderbook; use
//!   the Vanta venue instead) and RISEx (no live mainnet found — skipped).
//!
//! # Custody model (can the venue take user funds?)
//! The user asked explicitly whether each venue can "take my money away if
//! they shutdown, skip town, or go bankrupt". Short version:
//! - LOW custodial risk (self-custody, onchain settlement — user keeps keys):
//!   dYdX v4, GMX V2, Paradex, ApeX Omni, Extended, Derive, Helix/Injective,
//!   Astros. Even here, frontend/API outages can strand positions during
//!   volatility, and smart-contract bugs are a separate risk (see Ostium).
//! - MEDIUM (hybrid: offchain matching, onchain settlement — operator runs
//!   the book but cannot move collateral without user signatures): Aevo,
//!   StandX, Vest, SoDEX, Gate DEX, QFEX, IO Trader, Niza.fun, Pacifica,
//!   AlphaX, EVEDEX, BULK, Nado, Antarctic.
//! - HIGHEST CAUTION: Ostium (oracle-manipulation exploit, ~$18M, protocol
//!   paused — trade only after audited relaunch), Pacifica (ex-FTX team,
//!   invite-only beta), AlphaX (no proof-of-reserves published), QFEX
//!   (6-person team, thin docs). Paper read-only until each case resolves.
//!
//! Full per-venue notes live in `docs/EXCHANGES.md`.

use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;

use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::AccountSnapshot;
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{dec, Decimal, VenueId};

use crate::rate_limiter::AccountRequestCoordinator;
use crate::wallet::{wallet_execution_unavailable, EvmWalletSigner};
use parking_lot::RwLock;
use rust_decimal::prelude::ToPrimitive;

// ---------------------------------------------------------------------------
// Shared parsing helpers (fail-closed per entry, never abort listings)
// ---------------------------------------------------------------------------

/// Parse a decimal from either a JSON string or number. Returns `None` for
/// anything else (null, bool, object) so callers fail closed per entry.
fn dec_from_json(v: Option<&serde_json::Value>) -> Option<Decimal> {
    match v? {
        serde_json::Value::String(s) => Decimal::from_str(s.trim()).ok(),
        serde_json::Value::Number(n) => Decimal::from_str(&n.to_string()).ok(),
        _ => None,
    }
}

/// Parse one `[price, size]` pair where each side may be a string or number
/// (the dominant perp-DEX book encoding: Paradex, Aevo, ApeX, Extended…).
fn parse_pair_level(v: &serde_json::Value) -> Option<(Decimal, Decimal)> {
    let arr = v.as_array()?;
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

/// Parse one `{price, size}` object level (dYdX v4 book encoding).
fn parse_obj_level(v: &serde_json::Value) -> Option<(Decimal, Decimal)> {
    let p = dec_from_json(v.get("price"))?;
    let q = dec_from_json(v.get("size"))?;
    if p > dec!(0) && q > dec!(0) {
        Some((p, q))
    } else {
        None
    }
}

/// Canonicalize caller-side spellings (`BTC`, `BTC-USDC`, `BTC/USDC`,
/// `BTC-PERP`, `BTCUSDT`) to a bare uppercase base (`BTC`). Venue symbol
/// builders below add the venue-specific suffix.
fn to_base(symbol: &str) -> String {
    let mut s = symbol.trim().to_uppercase().replace(['/', ':'], "-");
    for suffix in ["-PERP", "-PERPS", "-USDC", "-USDT", "-USD", "-USDP"] {
        if let Some(stripped) = s.strip_suffix(suffix) {
            s = stripped.to_string();
            break;
        }
    }
    // Unqualified `BTCUSDT`-style: split trailing quote token when present.
    for q in ["USDT", "USDC", "USDP", "USD"] {
        if s.len() > q.len() && s.ends_with(q) && !s.contains('-') {
            s = s[..s.len() - q.len()].to_string();
            break;
        }
    }
    s
}

/// Build [`MarketMetadata`] with zero per-instrument fees (venue taker rate
/// is applied downstream from the frontend fee table) and caller-supplied
/// tick sizes. Returns `None` when ticks are missing/non-positive so the
/// entry is skipped without aborting the listing.
fn meta(
    venue: &VenueId,
    base: &str,
    quote: &str,
    venue_symbol: String,
    price_tick: Option<Decimal>,
    size_tick: Option<Decimal>,
    taker_fee_bps: Decimal,
) -> Option<MarketMetadata> {
    let price_tick = price_tick.filter(|t| *t > dec!(0))?;
    let size_tick = size_tick.filter(|t| *t > dec!(0))?;
    Some(MarketMetadata {
        symbol: Symbol::perp(base, quote),
        venue: venue.clone(),
        venue_symbol,
        price_decimals: price_tick.normalize().scale(),
        size_decimals: size_tick.normalize().scale(),
        min_order_size: size_tick,
        min_notional: dec!(5.0),
        maker_fee_bps: dec!(0.0),
        taker_fee_bps,
        contract_size: dec!(1.0),
        is_active: true,
    })
}

/// Fill an [`OrderBook`] from pre-parsed level lists; bails when both sides
/// are empty so empty/maintenance books never masquerade as live quotes.
fn book_from_levels(
    venue: &VenueId,
    symbol: &str,
    bids: Vec<(Decimal, Decimal)>,
    asks: Vec<(Decimal, Decimal)>,
) -> anyhow::Result<OrderBook> {
    if bids.is_empty() && asks.is_empty() {
        anyhow::bail!(
            "No live orderbook levels returned from {} for {symbol}",
            venue.as_str()
        );
    }
    let mut book = OrderBook::new(venue.clone(), symbol.to_string());
    for (p, q) in bids {
        book.bids.push(BookLevel::new(p, q));
    }
    for (p, q) in asks {
        book.asks.push(BookLevel::new(p, q));
    }
    book.timestamp = Utc::now();
    Ok(book)
}

fn funding_now(
    venue: &VenueId,
    symbol: &str,
    rate: Decimal,
    interval_hours: u32,
    next_funding_time: Option<chrono::DateTime<Utc>>,
) -> FundingRate {
    FundingRate {
        venue: venue.clone(),
        symbol: symbol.to_string(),
        rate,
        predicted_rate: None,
        interval_hours,
        next_funding_time: next_funding_time.unwrap_or_else(Utc::now),
        timestamp: Utc::now(),
    }
}

// ---------------------------------------------------------------------------
// Venue spec table
// ---------------------------------------------------------------------------

/// Which market-data shape a venue speaks. `PricesOnly` venues (GMX V2's GLV
/// AMM) have no orderbook by construction: book/ticker-from-book paths fail
/// closed and only `fetch_ticker` (via the price feed) plus volumes work.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BookKind {
    /// `bids`/`asks` arrays of `[price, size]` pairs (Paradex, Aevo, …).
    PairLevels,
    /// `bids`/`asks` arrays of `{price, size}` objects (dYdX v4).
    ObjectLevels,
    /// No orderbook exists (GMX V2 GLV AMM).
    PricesOnly,
}

/// Static per-venue wiring: identity, public hosts, URL builders, and fee.
/// `admit` mirrors the verification section above. All 23 specs currently
/// admit (paper read-only); flipping one back to `false` returns it to
/// modeled-but-not-admitted (PopDEX precedent) without touching call sites.
pub struct VenueApiSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub markets_path: &'static str,
    pub taker_fee_bps: Decimal,
    pub book_kind: BookKind,
    pub admit: bool,
    pub verified: bool,
}

// NOTE: rust_decimal's `dec!` cannot be used in `static` initializers, so
// taker fees are constructed with `Decimal::from_str_exact` in `spec_for`.
fn spec_for(id: &str) -> Option<VenueApiSpec> {
    let fee = |s: &str| Decimal::from_str(s).unwrap_or(Decimal::ZERO);
    let spec = match id {
        // ---- LIVE-VERIFIED ----
        "dydx" => VenueApiSpec {
            id: "dydx",
            name: "dYdX Chain",
            base_url: "https://indexer.dydx.trade",
            markets_path: "/v4/perpetualMarkets",
            taker_fee_bps: fee("2.0"),
            book_kind: BookKind::ObjectLevels,
            admit: true,
            verified: true,
        },
        "paradex" => VenueApiSpec {
            id: "paradex",
            name: "Paradex",
            base_url: "https://api.prod.paradex.trade",
            markets_path: "/v1/markets",
            taker_fee_bps: fee("2.0"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: true,
        },
        "aevo" => VenueApiSpec {
            id: "aevo",
            name: "Aevo",
            base_url: "https://api.aevo.xyz",
            markets_path: "/markets",
            taker_fee_bps: fee("3.0"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: true,
        },
        "gmx" => VenueApiSpec {
            id: "gmx",
            name: "GMX V2",
            base_url: "https://arbitrum-api.gmxinfra.io",
            markets_path: "/prices/tickers",
            taker_fee_bps: fee("5.0"),
            book_kind: BookKind::PricesOnly,
            admit: true,
            verified: true,
        },
        // ---- DOC-SHAPED, admitted paper-only (public docs; sweep pending) ----
        "apex" => VenueApiSpec {
            id: "apex",
            name: "ApeX Omni",
            base_url: "https://omni.apex.exchange",
            markets_path: "/api/v3/symbols",
            taker_fee_bps: fee("2.0"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: true,
        },
        "extended" => VenueApiSpec {
            id: "extended",
            name: "Extended",
            base_url: "https://api.starknet.extended.exchange",
            markets_path: "/api/v1/info/markets",
            taker_fee_bps: fee("2.0"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: true,
        },
        "derive" => VenueApiSpec {
            id: "derive",
            name: "Derive",
            base_url: "https://api.lyra.finance",
            markets_path: "/public/get_ticker?instrument_name=ETH-PERP",
            taker_fee_bps: fee("3.0"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: true,
        },
        "standx" => VenueApiSpec {
            id: "standx",
            name: "StandX",
            base_url: "https://perps.standx.com",
            markets_path: "/api/query_symbol_info",
            taker_fee_bps: fee("4.0"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: true,
        },
        "helix" => VenueApiSpec {
            id: "helix",
            name: "Helix",
            base_url: "https://sentry.lcd.injective.network",
            markets_path: "/injective/exchange/v1beta1/derivative/markets",
            taker_fee_bps: fee("0.01"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: true,
        },
        // ---- DOC-SHAPED, admitted paper-only per operator request ----
        // Shapes come from public docs; a harness sweep from full egress
        // should still confirm each payload shape. Parsers fail closed per
        // entry, so a wrong doc guess degrades to venue errors, never bad
        // data. High-caution venues (Ostium, AlphaX) stay paper-only
        // regardless — see docs/EXCHANGES.md section 10. Pacifica and QFEX
        // graduated to LIVE-VERIFIED above (QFEX: markets only).
        "vest" => VenueApiSpec {
            id: "vest",
            name: "Vest",
            base_url: "https://api.vest.exchange",
            markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: false,
        },
        "antarctic" => VenueApiSpec {
            id: "antarctic",
            name: "Antarctic",
            base_url: "https://api.antarctic.exchange",
            markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: false,
        },
        "evedex" => VenueApiSpec {
            id: "evedex",
            name: "EVEDEX",
            base_url: "https://api.evedex.com",
            markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: false,
        },
        "bulk" => VenueApiSpec {
            id: "bulk",
            name: "BULK",
            base_url: "https://api.bulk.exchange",
            markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: false,
        },
        "qfex" => VenueApiSpec {
            id: "qfex",
            name: "QFEX",
            base_url: "https://api.qfex.com",
            markets_path: "/refdata",
            taker_fee_bps: fee("4.0"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: true,
        },
        "ostium" => VenueApiSpec {
            id: "ostium",
            name: "Ostium",
            base_url: "https://api.ostium.io",
            markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: false,
        },
        "sodex" => VenueApiSpec {
            id: "sodex",
            name: "SoDEX",
            base_url: "https://api.sodex.xyz",
            markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: false,
        },
        "alphax" => VenueApiSpec {
            id: "alphax",
            name: "AlphaX",
            base_url: "https://api.alphax.exchange",
            markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: false,
        },
        "pacifica" => VenueApiSpec {
            id: "pacifica",
            name: "Pacifica",
            base_url: "https://api.pacifica.fi",
            markets_path: "/api/v1/info",
            taker_fee_bps: fee("2.0"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: true,
        },
        // edgeX public REST exposes only contract metadata; book/ticker/
        // funding are served over the venue WebSocket, so REST read paths
        // fail closed (verified: getMetaData 200, quote endpoints empty).
        "edgex" => VenueApiSpec {
            id: "edgex",
            name: "edgeX",
            base_url: "https://pro.edgex.exchange",
            markets_path: "/api/v1/public/meta/getMetaData",
            taker_fee_bps: fee("3.8"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: true,
        },
        // GRVT market data lives on market-data.grvt.io; order entry on
        // trades.grvt.io; API-key session login on edge.grvt.io.
        "grvt" => VenueApiSpec {
            id: "grvt",
            name: "GRVT",
            base_url: "https://market-data.grvt.io",
            markets_path: "/full/v1/all_instruments",
            taker_fee_bps: fee("5.0"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: true,
        },
        // Ondo Perps (Enclave rails): markets/depth/funding are public;
        // trading is SIWE-authenticated REST.
        "ondoperps" => VenueApiSpec {
            id: "ondoperps",
            name: "Ondo Perps",
            base_url: "https://api.ondoperps.xyz",
            markets_path: "/v1/markets",
            taker_fee_bps: fee("2.5"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: true,
        },
        "nado" => VenueApiSpec {
            id: "nado",
            name: "Nado",
            base_url: "https://api.nado.xyz",
            markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: false,
        },
        "iotrader" => VenueApiSpec {
            id: "iotrader",
            name: "IO Trader",
            base_url: "https://api.io-trader.com",
            markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: false,
        },
        "niza" => VenueApiSpec {
            id: "niza",
            name: "Niza",
            base_url: "https://api.niza.fun",
            markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: false,
        },
        "astros" => VenueApiSpec {
            id: "astros",
            name: "Astros",
            base_url: "https://api.astros.ag",
            markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: false,
        },
        "gatedex" => VenueApiSpec {
            id: "gatedex",
            name: "Gate DEX",
            base_url: "https://api.gate-dex.io",
            markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: false,
        },
        _ => return None,
    };
    Some(spec)
}

/// All venue ids this module implements, in alphabetical order.
pub const DEX_PERP_VENUE_IDS: [&str; 26] = [
    "aevo",
    "alphax",
    "antarctic",
    "apex",
    "astros",
    "bulk",
    "derive",
    "dydx",
    "edgex",
    "extended",
    "evedex",
    "gatedex",
    "gmx",
    "grvt",
    "helix",
    "iotrader",
    "nado",
    "niza",
    "ondoperps",
    "ostium",
    "pacifica",
    "paradex",
    "qfex",
    "sodex",
    "standx",
    "vest",
];

/// Venue ids with implemented live order paths (signing + polling +
/// reconciliation). Every other venue in this module stays
/// market-data-only: `place_order`/`cancel_order`/`set_leverage` fail
/// closed and the capability flags stay false.
pub const LIVE_TRADING_VENUE_IDS: [&str; 6] =
    ["aevo", "derive", "grvt", "ondoperps", "pacifica", "standx"];

/// Lyra/Derive serves one instrument per call, so Derive markets fan out
/// over this static currency list (each response is cached by path, so a
/// refresh is cheap after the first sweep). Currencies with no perp return
/// an empty `result` and contribute nothing.
const DERIVE_CURRENCIES: [&str; 10] = [
    "BTC", "ETH", "SOL", "ARB", "OP", "AVAX", "LINK", "DOGE", "SUI", "NEAR",
];

/// Venue ids admitted to `get_venues`. All 26 venues in this module are
/// admitted: the curl-verified plus doc-shaped shapes. Doc-shaped venues
/// still deserve a harness sweep from full egress, but the per-entry
/// fail-closed parsers guarantee a wrong doc guess degrades to venue
/// errors, never bad data.
pub const ADMITTED_DEX_PERP_VENUE_IDS: [&str; 26] = [
    "aevo",
    "alphax",
    "antarctic",
    "apex",
    "astros",
    "bulk",
    "derive",
    "dydx",
    "edgex",
    "extended",
    "evedex",
    "gatedex",
    "gmx",
    "grvt",
    "helix",
    "iotrader",
    "nado",
    "niza",
    "ondoperps",
    "ostium",
    "pacifica",
    "paradex",
    "qfex",
    "sodex",
    "standx",
    "vest",
];

// ---------------------------------------------------------------------------
// Adapter struct (one type, driven by the spec table)
// ---------------------------------------------------------------------------

/// Paper read-only adapter for long-tail perp venues. Construct with
/// [`DexPerpAdapter::for_venue`] (returns `None` for unknown ids) or
/// the per-venue helpers (`dydx()`, `paradex()`, …) used by `state.rs`.
fn is_valid_evm_address(a: &str) -> bool {
    let s = a.trim();
    s.len() == 42 && s.starts_with("0x") && s[2..].chars().all(|c| c.is_ascii_hexdigit())
}

/// Non-secret credential fields (api key, account ids, wallet address)
/// kept for request building. Secrets live only in the signer fields.
#[derive(Default)]
struct DexPerpCreds {
    api_key: Option<String>,
    api_secret: Option<zeroize::Zeroizing<String>>,
    account_id: Option<String>,
    wallet_address: Option<String>,
}

/// Authenticated session material for venues that hand out a bearer
/// token (StandX/Ondo JWT) or cookie (GRVT `gravity` cookie).
struct DexPerpSession {
    token: String,
    /// GRVT account id echoed back by the api-key login response.
    account_id: Option<String>,
    expires_at: chrono::DateTime<Utc>,
}

pub struct DexPerpAdapter {
    venue_id: VenueId,
    name: String,
    spec_id: &'static str,
    client: Client,
    base_url: String,
    rate_limiter: AccountRequestCoordinator,
    wallet_address: RwLock<Option<String>>,
    wallet: RwLock<Option<EvmWalletSigner>>,
    /// Ed25519 signer for Pacifica ops and StandX request bodies
    /// (private_key field interpreted per venue — bs58 encoded).
    ed25519: RwLock<Option<ed25519_dalek::SigningKey>>,
    creds: RwLock<DexPerpCreds>,
    session: RwLock<Option<DexPerpSession>>,
}

impl DexPerpAdapter {
    /// Build an adapter for a venue id in [`DEX_PERP_VENUE_IDS`].
    /// Returns `None` for anything else (RISEx, Orderly-derivatives, and
    /// unknown strings fail closed here — they are documented, not modeled).
    pub fn for_venue(id: &str, base_url: Option<String>) -> Option<Self> {
        let spec = spec_for(id.trim().to_lowercase().as_str())?;
        // Public market-data only: identify ourselves and ask for JSON.
        // (Extended's edge intermittently 403s fingerprint-less clients;
        // headers keep us on the documented public API, nothing more.)
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::USER_AGENT,
            reqwest::header::HeaderValue::from_static("arbitrageur/0.1 (paper market-data)"),
        );
        headers.insert(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .default_headers(headers)
            .build()
            .unwrap_or_default();
        Some(Self {
            venue_id: VenueId::new(spec.id),
            name: spec.name.to_string(),
            spec_id: spec.id,
            base_url: base_url.unwrap_or_else(|| spec.base_url.to_string()),
            client,
            rate_limiter: AccountRequestCoordinator::new(spec.id, 100, 2000, 500, 10000),
            wallet_address: RwLock::new(None),
            wallet: RwLock::new(None),
            ed25519: RwLock::new(None),
            creds: RwLock::new(DexPerpCreds::default()),
            session: RwLock::new(None),
        })
    }

    /// Whether `get_venues` should admit this venue (spec `admit` flag).
    pub fn is_admitted(&self) -> bool {
        spec_for(self.spec_id).map(|s| s.admit).unwrap_or(false)
    }

    /// Static venue id (`'static` — usable in venue lists and settings keys).
    pub fn spec_id(&self) -> &'static str {
        self.spec_id
    }

    /// Venue id as a str for matching.
    pub fn venue_id_str(&self) -> &str {
        self.venue_id.as_str()
    }

    /// The address the currently-held key derives to — the identity the
    /// venue will recover from our signatures. `None` when no usable EVM
    /// key is held (Ed25519 venues like pacifica return None too).
    pub fn active_signer_address(&self) -> Option<String> {
        self.wallet.read().as_ref().map(|s| s.address_checksum())
    }

    /// Static display name from the spec table.
    pub fn spec_name(&self) -> &'static str {
        spec_for(self.spec_id).map(|s| s.name).unwrap_or("DEX")
    }

    /// Taker fee in bps from the spec table (status/fee display).
    pub fn taker_fee_bps(&self) -> f64 {
        spec_for(self.spec_id)
            .and_then(|s| s.taker_fee_bps.to_f64())
            .unwrap_or(0.0)
    }

    /// Book-shape label for status surfaces — AMM venues are not CLOBs.
    pub fn market_type_label(&self) -> &'static str {
        match spec_for(self.spec_id).map(|s| s.book_kind) {
            Some(BookKind::PricesOnly) => "Perpetual AMM",
            _ => "Perpetual CLOB",
        }
    }

    // Per-venue constructors (used by state.rs so call sites read clearly).
    pub fn dydx(base_url: Option<String>) -> Self {
        Self::for_venue("dydx", base_url).expect("dydx spec")
    }
    pub fn paradex(base_url: Option<String>) -> Self {
        Self::for_venue("paradex", base_url).expect("paradex spec")
    }
    pub fn aevo(base_url: Option<String>) -> Self {
        Self::for_venue("aevo", base_url).expect("aevo spec")
    }
    pub fn gmx(base_url: Option<String>) -> Self {
        Self::for_venue("gmx", base_url).expect("gmx spec")
    }
    pub fn apex(base_url: Option<String>) -> Self {
        Self::for_venue("apex", base_url).expect("apex spec")
    }
    pub fn extended(base_url: Option<String>) -> Self {
        Self::for_venue("extended", base_url).expect("extended spec")
    }
    pub fn derive(base_url: Option<String>) -> Self {
        Self::for_venue("derive", base_url).expect("derive spec")
    }
    pub fn standx(base_url: Option<String>) -> Self {
        Self::for_venue("standx", base_url).expect("standx spec")
    }
    pub fn helix(base_url: Option<String>) -> Self {
        Self::for_venue("helix", base_url).expect("helix spec")
    }
    pub fn vest(base_url: Option<String>) -> Self {
        Self::for_venue("vest", base_url).expect("vest spec")
    }
    pub fn antarctic(base_url: Option<String>) -> Self {
        Self::for_venue("antarctic", base_url).expect("antarctic spec")
    }
    pub fn evedex(base_url: Option<String>) -> Self {
        Self::for_venue("evedex", base_url).expect("evedex spec")
    }
    pub fn bulk(base_url: Option<String>) -> Self {
        Self::for_venue("bulk", base_url).expect("bulk spec")
    }
    pub fn qfex(base_url: Option<String>) -> Self {
        Self::for_venue("qfex", base_url).expect("qfex spec")
    }
    pub fn ostium(base_url: Option<String>) -> Self {
        Self::for_venue("ostium", base_url).expect("ostium spec")
    }
    pub fn sodex(base_url: Option<String>) -> Self {
        Self::for_venue("sodex", base_url).expect("sodex spec")
    }
    pub fn alphax(base_url: Option<String>) -> Self {
        Self::for_venue("alphax", base_url).expect("alphax spec")
    }
    pub fn pacifica(base_url: Option<String>) -> Self {
        Self::for_venue("pacifica", base_url).expect("pacifica spec")
    }
    pub fn nado(base_url: Option<String>) -> Self {
        Self::for_venue("nado", base_url).expect("nado spec")
    }
    pub fn iotrader(base_url: Option<String>) -> Self {
        Self::for_venue("iotrader", base_url).expect("iotrader spec")
    }
    pub fn niza(base_url: Option<String>) -> Self {
        Self::for_venue("niza", base_url).expect("niza spec")
    }
    pub fn astros(base_url: Option<String>) -> Self {
        Self::for_venue("astros", base_url).expect("astros spec")
    }
    pub fn gatedex(base_url: Option<String>) -> Self {
        Self::for_venue("gatedex", base_url).expect("gatedex spec")
    }
    pub fn edgex(base_url: Option<String>) -> Self {
        Self::for_venue("edgex", base_url).expect("edgex spec")
    }
    pub fn grvt(base_url: Option<String>) -> Self {
        Self::for_venue("grvt", base_url).expect("grvt spec")
    }
    pub fn ondoperps(base_url: Option<String>) -> Self {
        Self::for_venue("ondoperps", base_url).expect("ondoperps spec")
    }

    /// Bind (or clear) this venue's credentials from the vault.
    ///
    /// Field semantics are per venue — `requires_*` documents which the
    /// venue consumes:
    /// - `api_key`/`api_secret`: Aevo (header pair), GRVT (api_key login).
    /// - `account_id`: GRVT sub-account id (overrides the login echo).
    /// - `wallet_address`: funding/account wallet (EVM 0x for Derive,
    ///   Ondo, Aevo maker, StandX login; Solana base58 for Pacifica).
    /// - `private_key`: EVM hex for Derive/Ondo/Aevo/StandX login;
    ///   base58 Ed25519 secret for Pacifica ops.
    ///
    /// Keys are parsed into zeroizing signers immediately; raw strings
    /// are never retained. An invalid key fails the whole update so a
    /// half-bound state is impossible.
    #[allow(clippy::too_many_arguments)]
    pub fn update_credentials(
        &self,
        api_key: Option<String>,
        api_secret: Option<String>,
        _passphrase: Option<String>,
        account_id: Option<String>,
        wallet_address: Option<String>,
        private_key: Option<String>,
    ) -> anyhow::Result<()> {
        let norm = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let api_key = norm(api_key);
        let api_secret = norm(api_secret);
        let account_id = norm(account_id);
        let wallet_address = norm(wallet_address);
        let private_key = norm(private_key);

        // EVM venues validate 0x addresses; Pacifica/StandX account keys
        // are base58 pubkeys — only length-sanity-checked.
        if let Some(a) = wallet_address.as_deref() {
            match self.spec_id {
                "pacifica" | "standx" => {
                    if a.starts_with("0x") && self.spec_id == "pacifica" {
                        anyhow::bail!("Pacifica account must be a Solana base58 address, not 0x");
                    }
                }
                _ => {
                    if !is_valid_evm_address(a) {
                        anyhow::bail!("wallet address is not a valid 0x EVM address");
                    }
                }
            }
        }

        // Parse the private key into the signer type the venue uses.
        // Pacifica + StandX use Ed25519 (bs58-encoded 64-byte keypair or
        // 32-byte seed); EVM venues use hex secp256k1.
        let mut evm: Option<EvmWalletSigner> = None;
        let mut ed: Option<ed25519_dalek::SigningKey> = None;
        if let Some(key) = private_key.as_deref() {
            match self.spec_id {
                "pacifica" | "standx" => {
                    let bytes = bs58::decode(key.trim()).into_vec().map_err(|_| {
                        anyhow::anyhow!("{} private key is not valid base58", self.name)
                    })?;
                    let seed: [u8; 32] = match bytes.len() {
                        64 => bytes[..32].try_into().unwrap(),
                        32 => bytes.as_slice().try_into().unwrap(),
                        n => anyhow::bail!(
                            "{} Ed25519 key must decode to 32 or 64 bytes, got {n}",
                            self.name
                        ),
                    };
                    ed = Some(ed25519_dalek::SigningKey::from_bytes(&seed));
                }
                _ => {
                    evm = Some(EvmWalletSigner::from_hex(key)?);
                }
            }
        }

        {
            let mut c = self.creds.write();
            c.api_key = api_key;
            c.api_secret = api_secret.map(zeroize::Zeroizing::new);
            c.account_id = account_id;
            c.wallet_address = wallet_address.clone();
        }
        *self.wallet_address.write() = wallet_address;
        *self.wallet.write() = evm;
        *self.ed25519.write() = ed;
        // Credentials changed — any cached session token is stale.
        *self.session.write() = None;
        Ok(())
    }

    /// Currently bound main address, if any.
    pub fn wallet_address_hex(&self) -> Option<String> {
        self.wallet_address.read().clone()
    }

    /// Derived agent-signer address (from the delegated private key), if bound.
    pub fn agent_address_hex(&self) -> Option<String> {
        self.wallet.read().as_ref().map(|w| w.address_hex())
    }

    /// Venue's configured trading identity: wallet address for
    /// wallet-auth venues, account_id where the venue keys accounts
    /// numerically (GRVT sub-account), api key presence for Aevo.
    fn require_trading_ready(&self) -> anyhow::Result<()> {
        if !LIVE_TRADING_VENUE_IDS.contains(&self.spec_id) {
            anyhow::bail!(wallet_execution_unavailable(
                self.name(),
                "its settlement chain",
                "wallet-signed order flow"
            ));
        }
        Ok(())
    }

    /// GET `path` against the venue base, fail-closed on transport, status,
    /// and content-type errors (edge-challenge HTML must never parse as a
    /// book). Successful bodies pass through the coordinator cache.
    async fn get_json(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        if let Some(cached) = self.rate_limiter.acquire_or_cache_path(path, true).await? {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&cached) {
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
            anyhow::bail!("{} API error: {} {}", self.name, resp.status(), path);
        }
        let is_json = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| ct.contains("json"))
            .unwrap_or(false);
        let text = resp.text().await.unwrap_or_default();
        if !is_json && !text.trim_start().starts_with(['{', '[']) {
            anyhow::bail!(
                "{} returned non-JSON for {} (likely edge challenge)",
                self.name,
                path
            );
        }
        let val: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("{} unparseable JSON for {}: {e}", self.name, path))?;
        self.rate_limiter.record_success(text);
        Ok(val)
    }

    /// POST JSON to `url` (absolute), fail-closed on transport/status/
    /// content-type. Never cached — callers pass order/cancel bodies.
    async fn post_json_url(
        &self,
        url: &str,
        body: &serde_json::Value,
        headers: &[(String, String)],
    ) -> anyhow::Result<serde_json::Value> {
        let mut req = self.client.post(url).json(body);
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req.send().await.inspect_err(|_| {
            self.rate_limiter.record_failure(false);
        })?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            self.rate_limiter
                .record_failure(status == reqwest::StatusCode::TOO_MANY_REQUESTS);
            // Surface the venue's error text (it carries the rejection
            // reason) but never log secrets — bodies contain none.
            anyhow::bail!(
                "{} API error: {} — {}",
                self.name,
                status,
                text.trim().chars().take(300).collect::<String>()
            );
        }
        serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("{} unparseable JSON for POST {}: {e}", self.name, url))
    }

    /// GET `url` (absolute) with extra headers; no response caching
    /// (authenticated/private reads must never return stale state).
    async fn get_json_url(
        &self,
        url: &str,
        headers: &[(String, String)],
    ) -> anyhow::Result<serde_json::Value> {
        let mut req = self.client.get(url);
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req.send().await.inspect_err(|_| {
            self.rate_limiter.record_failure(false);
        })?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            self.rate_limiter
                .record_failure(status == reqwest::StatusCode::TOO_MANY_REQUESTS);
            anyhow::bail!(
                "{} API error: {} — {}",
                self.name,
                status,
                text.trim().chars().take(300).collect::<String>()
            );
        }
        serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("{} unparseable JSON for {}: {e}", self.name, url))
    }

    /// DELETE `url` (absolute) with headers (Ondo order cancel).
    async fn delete_json_url(
        &self,
        url: &str,
        headers: &[(String, String)],
    ) -> anyhow::Result<serde_json::Value> {
        let mut req = self.client.delete(url);
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req.send().await.inspect_err(|_| {
            self.rate_limiter.record_failure(false);
        })?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            self.rate_limiter
                .record_failure(status == reqwest::StatusCode::TOO_MANY_REQUESTS);
            anyhow::bail!(
                "{} API error: {} — {}",
                self.name,
                status,
                text.trim().chars().take(300).collect::<String>()
            );
        }
        serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("{} unparseable JSON for {}: {e}", self.name, url))
    }

    /// Venue-native market symbol for a caller-side symbol.
    fn venue_symbol(&self, symbol: &str) -> String {
        let base = to_base(symbol);
        match self.spec_id {
            "dydx" => format!("{base}-USD"),
            "paradex" => format!("{base}-USD-PERP"),
            "aevo" => format!("{base}-PERP"),
            // ApeX Omni perpetuals are `{BASE}USDT` (crossSymbolName).
            "apex" => format!("{base}USDT"),
            // Lyra/Derive instruments are `{BASE}-PERP`.
            "derive" => format!("{base}-PERP"),
            // StandX and Extended quote perps against USD (`BTC-USD`).
            "standx" | "extended" => format!("{base}-USD"),
            // Pacifica addresses markets by bare base asset (`BTC`).
            "pacifica" => base,
            // Helix/Injective books resolve through the LCD market list
            // (market_id); no static spelling exists — callers use the
            // helix helpers, never this.
            "helix" => base,
            // QFEX refdata symbols look like `BTC-USD`; books are
            // auth-gated, so this spelling is markets-listing only.
            "qfex" => format!("{base}-USD"),
            // edgeX contracts are `{BASE}USD` ("BTCUSD"); order paths
            // resolve the numeric contractId via metadata, not this.
            "edgex" => format!("{base}USD"),
            // GRVT perpetual instruments are `{BASE}_USDT_Perp`.
            "grvt" => format!("{base}_USDT_Perp"),
            // Ondo perp markets are `{BASE}-USD.P`.
            "ondoperps" => format!("{base}-USD.P"),
            _ => format!("{base}-USD-PERP"),
        }
    }

    /// Orderbook path for a venue-native symbol. Derive, Helix, and QFEX
    /// never reach this (Derive/Helix resolve dynamically, QFEX books are
    /// auth-gated and fail closed before any path is built).
    fn book_path(&self, venue_symbol: &str) -> String {
        match self.spec_id {
            "dydx" => format!("/v4/orderbooks/perpetualMarket/{venue_symbol}"),
            "paradex" => format!("/v1/orderbook/{venue_symbol}?depth=15"),
            "aevo" => format!("/orderbook?instrument_name={venue_symbol}"),
            // Verified: `data.a` = asks, `data.b` = bids (`BTCUSDT` style).
            "apex" => format!("/api/v3/depth?symbol={venue_symbol}"),
            // Verified: `/api/v1/info/markets/{name}/orderbook`.
            "extended" => format!("/api/v1/info/markets/{venue_symbol}/orderbook"),
            // Verified: `{asks: [[p, q]], bids: [[p, q]]}` (`BTC-USD`).
            "standx" => format!("/api/query_depth_book?symbol={venue_symbol}"),
            // Verified: `{success, data: {s, l: [[bids], [asks]], t}}`.
            "pacifica" => format!("/api/v1/book?symbol={venue_symbol}"),
            // Verified: `{result: {asks: [[p, s]], bids: [[p, s]]}}`.
            "ondoperps" => format!("/v1/perps/depth?market={venue_symbol}&depth=15"),
            _ => format!("/v1/orderbook?symbol={venue_symbol}"),
        }
    }

    /// Resolve a caller-side base asset (e.g. `BTC`) to its active Helix
    /// LCD market, preferring the USDC quote. Fails closed when the base
    /// has no active perp.
    async fn helix_resolve(&self, base: &str) -> anyhow::Result<HelixMarketInfo> {
        let data = self
            .get_json("/injective/exchange/v1beta1/derivative/markets")
            .await?;
        let mut fallback: Option<HelixMarketInfo> = None;
        for info in helix_market_infos(&data) {
            if info.base == base {
                if info.quote == "USDC" {
                    return Ok(info);
                }
                if fallback.is_none() {
                    fallback = Some(info);
                }
            }
        }
        fallback.ok_or_else(|| anyhow::anyhow!("Helix has no active perp for {base}"))
    }
}

// ---------------------------------------------------------------------------
// Market-listing parsers (per-entry fail-closed: bad entries are skipped,
// never abort the whole listing)
// ---------------------------------------------------------------------------

/// dYdX v4: `{"markets": {"BTC-USD": {ticker, tickSize, stepSize,
/// nextFundingRate, …}}}` — object-keyed, only ACTIVE markets admitted.
fn parse_dydx_markets(
    venue: &VenueId,
    data: &serde_json::Value,
    fee: Decimal,
) -> Vec<MarketMetadata> {
    let markets = match data.get("markets").and_then(|m| m.as_object()) {
        Some(m) => m,
        None => return Vec::new(),
    };
    markets
        .iter()
        .filter_map(|(ticker, e)| {
            if e.get("status").and_then(|s| s.as_str()) != Some("ACTIVE") {
                return None;
            }
            let (base, quote) = ticker.split_once('-')?;
            if base.is_empty() || quote.is_empty() {
                return None;
            }
            meta(
                venue,
                base,
                quote,
                ticker.clone(),
                dec_from_json(e.get("tickSize")),
                dec_from_json(e.get("stepSize")),
                fee,
            )
        })
        .collect()
}

/// Paradex: `{"results": [{symbol: "BTC-USD-PERP", base_currency,
/// quote_currency, price_tick_size, order_size_increment, …}]}` — includes
/// options; only `-PERP` symbols are admitted.
fn parse_paradex_markets(
    venue: &VenueId,
    data: &serde_json::Value,
    fee: Decimal,
) -> Vec<MarketMetadata> {
    let arr = match data.get("results").and_then(|r| r.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter_map(|e| {
            let symbol = e.get("symbol").and_then(|s| s.as_str())?;
            if !symbol.ends_with("-PERP") {
                return None;
            }
            let base = e.get("base_currency").and_then(|s| s.as_str())?;
            let quote = e
                .get("quote_currency")
                .and_then(|s| s.as_str())
                .unwrap_or("USD");
            if base.is_empty() {
                return None;
            }
            meta(
                venue,
                base,
                quote,
                symbol.to_string(),
                dec_from_json(e.get("price_tick_size")),
                dec_from_json(e.get("order_size_increment")),
                fee,
            )
        })
        .collect()
}

/// Aevo / Derive style: `[{instrument_name: "BTC-PERP", underlying_asset,
/// quote_asset, price_step, amount_step, is_active}]`.
fn parse_aevo_markets(
    venue: &VenueId,
    data: &serde_json::Value,
    fee: Decimal,
) -> Vec<MarketMetadata> {
    let arr = match data.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter_map(|e| {
            if e.get("is_active").and_then(|v| v.as_bool()) == Some(false) {
                return None;
            }
            let name = e.get("instrument_name").and_then(|s| s.as_str())?;
            if !name.ends_with("-PERP") {
                return None;
            }
            let base = e.get("underlying_asset").and_then(|s| s.as_str())?;
            let quote = e
                .get("quote_asset")
                .and_then(|s| s.as_str())
                .unwrap_or("USDC");
            if base.is_empty() {
                return None;
            }
            meta(
                venue,
                base,
                quote,
                name.to_string(),
                dec_from_json(e.get("price_step")),
                dec_from_json(e.get("amount_step")),
                fee,
            )
        })
        .collect()
}

/// GMX V2 tickers: `[{tokenSymbol, minPrice, maxPrice, …}]` — prices carry
/// chain-native decimals, so only the symbol set is admitted here; price
/// levels are normalized per-token in `parse_gmx_price` (30-decimal USD
/// feed). No orderbook exists by construction (GLV AMM).
/// ApeX Omni: `{"data": {"contractConfig": {"perpetualContract":
/// [{crossSymbolName: "BTCUSDT", symbol: "BTC-USDT", tickSize: "0.1",
/// stepSize: "0.001", enableTrade, enableDisplay, …}]}}}` — only
/// trade-enabled entries are admitted.
fn parse_apex_markets(
    venue: &VenueId,
    data: &serde_json::Value,
    fee: Decimal,
) -> Vec<MarketMetadata> {
    let arr = data
        .get("data")
        .and_then(|d| d.get("contractConfig"))
        .and_then(|c| c.get("perpetualContract"))
        .and_then(|v| v.as_array());
    let arr = match arr {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter_map(|e| {
            if e.get("enableTrade").and_then(|v| v.as_bool()) == Some(false) {
                return None;
            }
            let cross = e.get("crossSymbolName").and_then(|s| s.as_str())?;
            if cross.is_empty() {
                return None;
            }
            // `symbol` is `{BASE}-{QUOTE}` ("BTC-USDT"); fall back to splitting
            // the trailing quote token off `crossSymbolName`.
            let (base, quote) = match e
                .get("symbol")
                .and_then(|s| s.as_str())
                .and_then(|s| s.split_once('-'))
            {
                Some((b, q)) if !b.is_empty() && !q.is_empty() => (b.to_string(), q.to_string()),
                _ => {
                    let b = to_base(cross);
                    if b.is_empty() {
                        return None;
                    }
                    (b, "USDT".to_string())
                }
            };
            meta(
                venue,
                &base,
                &quote,
                cross.to_string(),
                dec_from_json(e.get("tickSize")),
                dec_from_json(e.get("stepSize")),
                fee,
            )
        })
        .collect()
}

/// Extended: `{"status": "OK", "data": [{name: "ENA-USD", type:
/// "PERPETUAL", assetName, collateralAssetName, status: "ACTIVE",
/// tradingConfig: {minPriceChange, minOrderSizeChange, minOrderSize},
/// marketStats: {…}}]}` — only ACTIVE PERPETUAL entries.
fn parse_extended_markets(
    venue: &VenueId,
    data: &serde_json::Value,
    fee: Decimal,
) -> Vec<MarketMetadata> {
    let arr = data.get("data").and_then(|v| v.as_array());
    let arr = match arr {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter_map(|e| {
            if e.get("type").and_then(|s| s.as_str()) != Some("PERPETUAL") {
                return None;
            }
            if e.get("status").and_then(|s| s.as_str()) != Some("ACTIVE") {
                return None;
            }
            let name = e.get("name").and_then(|s| s.as_str())?;
            let base = e.get("assetName").and_then(|s| s.as_str()).unwrap_or("");
            let base = if base.is_empty() {
                to_base(name)
            } else {
                base.to_string()
            };
            if base.is_empty() {
                return None;
            }
            let quote = e
                .get("collateralAssetName")
                .and_then(|s| s.as_str())
                .unwrap_or("USD");
            let cfg = e.get("tradingConfig");
            let price_tick = cfg.and_then(|c| dec_from_json(c.get("minPriceChange")));
            let size_tick = cfg
                .and_then(|c| dec_from_json(c.get("minOrderSizeChange")))
                .or_else(|| cfg.and_then(|c| dec_from_json(c.get("minOrderSize"))));
            meta(
                venue,
                &base,
                quote,
                name.to_string(),
                price_tick,
                size_tick,
                fee,
            )
        })
        .collect()
}

/// StandX: `[{symbol: "BTC-USD", base_asset, quote_asset,
/// price_tick_decimals: 2, qty_tick_decimals: 2, taker_fee: "0.0004",
/// status: "trading"}]` — only `trading` symbols; the taker fee is a
/// fraction, converted to bps per entry (spec fee is the fallback).
fn parse_standx_markets(
    venue: &VenueId,
    data: &serde_json::Value,
    fee: Decimal,
) -> Vec<MarketMetadata> {
    let arr = match data.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter_map(|e| {
            if e.get("status").and_then(|s| s.as_str()) != Some("trading") {
                return None;
            }
            let symbol = e.get("symbol").and_then(|s| s.as_str())?;
            let base = e
                .get("base_asset")
                .and_then(|s| s.as_str())
                .map(|s| s.to_uppercase())
                .unwrap_or_else(|| to_base(symbol));
            if base.is_empty() {
                return None;
            }
            let quote = e
                .get("quote_asset")
                .and_then(|s| s.as_str())
                .unwrap_or("DUSD");
            let tick = |k: &str| {
                e.get(k)
                    .and_then(|v| v.as_u64())
                    .map(|d| Decimal::new(1, d as u32))
            };
            let entry_fee = dec_from_json(e.get("taker_fee"))
                .map(|f| f * dec!(10000))
                .unwrap_or(fee);
            meta(
                venue,
                &base,
                quote,
                symbol.to_string(),
                tick("price_tick_decimals"),
                tick("qty_tick_decimals"),
                entry_fee,
            )
        })
        .collect()
}

/// Pacifica: `{"success": true, "data": [{symbol: "BTC", base_asset,
/// tick_size: "1", lot_size: "0.00001", instrument_type: "perpetual",
/// …}]}` — markets are addressed by bare base asset; quote is USD.
fn parse_pacifica_markets(
    venue: &VenueId,
    data: &serde_json::Value,
    fee: Decimal,
) -> Vec<MarketMetadata> {
    let arr = data.get("data").and_then(|v| v.as_array());
    let arr = match arr {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter_map(|e| {
            let symbol = e.get("symbol").and_then(|s| s.as_str())?;
            if symbol.is_empty() {
                return None;
            }
            let base = e
                .get("base_asset")
                .and_then(|s| s.as_str())
                .map(|s| s.to_uppercase())
                .unwrap_or_else(|| symbol.to_uppercase());
            meta(
                venue,
                &base,
                "USD",
                symbol.to_string(),
                dec_from_json(e.get("tick_size")),
                dec_from_json(e.get("lot_size")),
                fee,
            )
        })
        .collect()
}

/// edgeX: `{"code":0,"data":{"contractList":[{contractId:"10000001",
/// contractName:"BTCUSD", tickSize, stepSize, minOrderSize, status,
/// display…}], "coinList":[…]}}` — contractName is `{BASE}USD`; the
/// numeric contractId is needed by the (WS) book feed and order paths.
fn parse_edgex_markets(
    venue: &VenueId,
    data: &serde_json::Value,
    fee: Decimal,
) -> Vec<MarketMetadata> {
    let arr = data
        .get("data")
        .and_then(|d| d.get("contractList"))
        .and_then(|v| v.as_array());
    let arr = match arr {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter_map(|e| {
            let name = e.get("contractName").and_then(|s| s.as_str())?;
            // Only linear USD-quoted perpetual contracts are admitted.
            let base = name.strip_suffix("USD")?;
            if base.is_empty() {
                return None;
            }
            // Skip contracts the venue marks non-tradable when the field is
            // present (field names differ across API versions — both checked).
            if let Some(s) = e.get("status").and_then(|v| v.as_str()) {
                if !s.eq_ignore_ascii_case("enabled") && !s.eq_ignore_ascii_case("live") {
                    return None;
                }
            }
            if e.get("display").and_then(|v| v.as_bool()) == Some(false) {
                return None;
            }
            if e.get("enableTrade").and_then(|v| v.as_bool()) == Some(false) {
                return None;
            }
            meta(
                venue,
                base,
                "USD",
                name.to_string(),
                dec_from_json(e.get("tickSize")),
                dec_from_json(e.get("stepSize")),
                fee,
            )
        })
        .collect()
}

/// GRVT: `POST /full/v1/all_instruments {}` → `{"result":[{i:
/// "BTC_USDT_Perp", b:"BTC", q:"USDT", k:"PERPETUAL", ih:"0x…", ts,
/// ms, bd, qd, …}]}`. `ih` (instrument_hash) is the assetID used in
/// order signatures — kept out of MarketMetadata; order paths re-resolve
/// it from the cached instrument list.
fn parse_grvt_markets(
    venue: &VenueId,
    data: &serde_json::Value,
    fee: Decimal,
) -> Vec<MarketMetadata> {
    let arr = data
        .get("result")
        .and_then(|v| v.as_array())
        .or_else(|| data.as_array());
    let arr = match arr {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter_map(|e| {
            // Lite encoding uses single-letter keys; verbose encoding spells
            // the names out — accept both.
            let get = |k1: &str, k2: &str| e.get(k1).or_else(|| e.get(k2));
            let kind = get("k", "kind").and_then(|s| s.as_str())?;
            if !kind.eq_ignore_ascii_case("PERPETUAL") {
                return None;
            }
            let name = get("i", "instrument").and_then(|s| s.as_str())?;
            let base = get("b", "base").and_then(|s| s.as_str())?;
            let quote = get("q", "quote").and_then(|s| s.as_str()).unwrap_or("USDT");
            if base.is_empty() {
                return None;
            }
            let price_tick = dec_from_json(get("ts", "tick_size")).or_else(|| {
                get("qd", "quote_decimals")
                    .and_then(|v| v.as_u64())
                    .map(|d| Decimal::new(1, d.min(18) as u32))
            });
            let size_tick = dec_from_json(get("ms", "min_size")).or_else(|| {
                get("bd", "base_decimals")
                    .and_then(|v| v.as_u64())
                    .map(|d| Decimal::new(1, d.min(18) as u32))
            });
            meta(
                venue,
                base,
                quote,
                name.to_string(),
                price_tick,
                size_tick,
                fee,
            )
        })
        .collect()
}

/// Ondo Perps: `{"success":true,"result":{"perps":{"tradingPairs":[{
/// market:"BTC-USD.P", pair:{base:"BTC",quote:"USD"}, baseIncrement,
/// quoteIncrement, …}]}}}`.
fn parse_ondo_markets(
    venue: &VenueId,
    data: &serde_json::Value,
    fee: Decimal,
) -> Vec<MarketMetadata> {
    let arr = data
        .get("result")
        .and_then(|r| r.get("perps"))
        .and_then(|p| p.get("tradingPairs"))
        .and_then(|v| v.as_array());
    let arr = match arr {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter_map(|e| {
            let market = e.get("market").and_then(|s| s.as_str())?;
            if !market.ends_with(".P") {
                return None;
            }
            let base = e
                .get("pair")
                .and_then(|p| p.get("base"))
                .and_then(|s| s.as_str())?;
            let quote = e
                .get("pair")
                .and_then(|p| p.get("quote"))
                .and_then(|s| s.as_str())
                .unwrap_or("USD");
            if base.is_empty() {
                return None;
            }
            meta(
                venue,
                base,
                quote,
                market.to_string(),
                dec_from_json(e.get("quoteIncrement")),
                dec_from_json(e.get("baseIncrement")),
                fee,
            )
        })
        .collect()
}

/// Derive/Lyra per-currency instruments: `{"result": [{instrument_name:
/// "ETH-PERP", tick_size: "0.01", amount_step, minimum_amount,
/// base_currency, quote_currency, is_active}]}`.
fn parse_derive_markets(
    venue: &VenueId,
    data: &serde_json::Value,
    fee: Decimal,
) -> Vec<MarketMetadata> {
    let arr = data.get("result").and_then(|v| v.as_array());
    let arr = match arr {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter_map(|e| {
            if e.get("is_active").and_then(|v| v.as_bool()) == Some(false) {
                return None;
            }
            let name = e.get("instrument_name").and_then(|s| s.as_str())?;
            if !name.ends_with("-PERP") {
                return None;
            }
            let base = e.get("base_currency").and_then(|s| s.as_str())?;
            let quote = e
                .get("quote_currency")
                .and_then(|s| s.as_str())
                .unwrap_or("USD");
            if base.is_empty() {
                return None;
            }
            let entry_fee = dec_from_json(e.get("taker_fee_rate"))
                .map(|f| f * dec!(10000))
                .unwrap_or(fee);
            meta(
                venue,
                base,
                quote,
                name.to_string(),
                dec_from_json(e.get("tick_size")),
                dec_from_json(e.get("amount_step")),
                entry_fee,
            )
        })
        .collect()
}

/// A Helix/Injective LCD derivative market, decoded from the chain query
/// response. Chain prices are quote-decimal fixed-point (`p` raw units);
/// `scale = 10^quote_decimals` converts them to human prices.
pub struct HelixMarketInfo {
    pub market_id: String,
    pub ticker: String,
    pub base: String,
    pub quote: String,
    pub scale: Decimal,
    pub price_tick: Decimal,
    pub size_tick: Decimal,
    pub fee_bps: Decimal,
}

/// Decode all active perpetual LCD markets. Per-entry fail-closed: markets
/// with missing ticks, bad decimals, or non-`Active` status are skipped.
fn helix_market_infos(data: &serde_json::Value) -> Vec<HelixMarketInfo> {
    let arr = data.get("markets").and_then(|v| v.as_array());
    let arr = match arr {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter_map(|wrap| {
            let m = wrap.get("market")?;
            if m.get("status").and_then(|s| s.as_str()) != Some("Active") {
                return None;
            }
            if m.get("isPerpetual").and_then(|v| v.as_bool()) == Some(false) {
                return None;
            }
            let ticker = m.get("ticker").and_then(|s| s.as_str())?;
            // Tickers read `"BTC/USDC PERP"`; quote keeps the suffix stripped.
            let (base, qraw) = ticker.split_once('/')?;
            let quote = qraw
                .strip_suffix(" PERP")
                .or_else(|| qraw.strip_suffix("-PERP"))
                .unwrap_or(qraw);
            if base.is_empty() || quote.is_empty() {
                return None;
            }
            let qdec = m.get("quote_decimals").and_then(|v| v.as_u64())?;
            let scale =
                Decimal::new(1, 0).checked_mul(Decimal::from(10u64.pow(qdec.min(18) as u32)))?;
            let raw_tick = dec_from_json(m.get("min_price_tick_size"))?;
            let price_tick = raw_tick.checked_div(scale)?;
            let size_tick = dec_from_json(m.get("min_quantity_tick_size"))?;
            if price_tick <= dec!(0) || size_tick <= dec!(0) {
                return None;
            }
            let fee_bps = dec_from_json(m.get("taker_fee_rate"))
                .map(|f| f * dec!(10000))
                .unwrap_or(dec!(0.01));
            Some(HelixMarketInfo {
                market_id: m.get("market_id").and_then(|s| s.as_str())?.to_string(),
                ticker: ticker.to_string(),
                base: base.to_uppercase(),
                quote: quote.to_uppercase(),
                scale,
                price_tick,
                size_tick,
                fee_bps,
            })
        })
        .collect()
}

fn parse_helix_markets(venue: &VenueId, data: &serde_json::Value) -> Vec<MarketMetadata> {
    helix_market_infos(data)
        .into_iter()
        .filter_map(|info| {
            meta(
                venue,
                &info.base,
                &info.quote,
                info.ticker.clone(),
                Some(info.price_tick),
                Some(info.size_tick),
                info.fee_bps,
            )
        })
        .collect()
}

/// QFEX: `{"type": "refdata", "data": [{symbol: "BTC-USD", base_asset,
/// quote_asset, tick_size, lot_size}]}` (a bare array is also accepted).
/// Books and tickers are auth-gated, so refdata is markets-only by design.
fn parse_qfex_markets(
    venue: &VenueId,
    data: &serde_json::Value,
    fee: Decimal,
) -> Vec<MarketMetadata> {
    let arr = data
        .get("data")
        .and_then(|v| v.as_array())
        .or_else(|| data.as_array());
    let arr = match arr {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter_map(|e| {
            let symbol = e.get("symbol").and_then(|s| s.as_str())?;
            if symbol.is_empty() {
                return None;
            }
            let base = e
                .get("base_asset")
                .and_then(|s| s.as_str())
                .map(|s| s.to_uppercase())
                .unwrap_or_else(|| to_base(symbol));
            if base.is_empty() {
                return None;
            }
            let quote = e
                .get("quote_asset")
                .and_then(|s| s.as_str())
                .unwrap_or("USD");
            meta(
                venue,
                &base,
                quote,
                symbol.to_string(),
                dec_from_json(e.get("tick_size")),
                dec_from_json(e.get("lot_size")),
                fee,
            )
        })
        .collect()
}

fn parse_gmx_markets(
    venue: &VenueId,
    data: &serde_json::Value,
    fee: Decimal,
) -> Vec<MarketMetadata> {
    let arr = match data.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    let mut seen = std::collections::HashSet::new();
    arr.iter()
        .filter_map(|e| {
            let sym = e.get("tokenSymbol").and_then(|s| s.as_str())?;
            if sym.is_empty() || sym.ends_with("_deprecated") || !seen.insert(sym.to_string()) {
                return None;
            }
            // GMX tickers are spot-price feeds, not perps; expose them as
            // USD-quoted perp watchlist entries so funding/spread monitors can
            // consume the price without pretending depth exists.
            meta(
                venue,
                sym,
                "USD",
                sym.to_string(),
                Some(dec!(0.01)),
                Some(dec!(0.0001)),
                fee,
            )
        })
        .collect()
}

/// Generic `{markets|results|data: […]}` array fallback for DOC-shaped
/// venues: accepts entries with any of `symbol`/`instrument_name`/`market`
/// plus `base`/`base_currency`/`underlying_asset` spellings. Entries that
/// do not match any spelling are skipped (fail-closed per entry).
fn parse_generic_markets(
    venue: &VenueId,
    data: &serde_json::Value,
    fee: Decimal,
) -> Vec<MarketMetadata> {
    let arr = data
        .get("markets")
        .or_else(|| data.get("results"))
        .or_else(|| data.get("data"))
        .and_then(|v| v.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[]);
    let arr_owned: Vec<serde_json::Value> = if !arr.is_empty() {
        arr.to_vec()
    } else if let Some(a) = data.as_array() {
        a.clone()
    } else {
        return Vec::new();
    };
    arr_owned
        .iter()
        .filter_map(|e| {
            let raw_symbol = e
                .get("symbol")
                .or_else(|| e.get("instrument_name"))
                .or_else(|| e.get("market"))
                .and_then(|s| s.as_str())?;
            let base = e
                .get("base")
                .or_else(|| e.get("base_currency"))
                .or_else(|| e.get("underlying_asset"))
                .and_then(|s| s.as_str())
                .map(|s| s.to_uppercase())
                .unwrap_or_else(|| to_base(raw_symbol));
            if base.is_empty() {
                return None;
            }
            let quote = e
                .get("quote")
                .or_else(|| e.get("quote_currency"))
                .or_else(|| e.get("quote_asset"))
                .and_then(|s| s.as_str())
                .unwrap_or("USD");
            let price_tick = dec_from_json(e.get("price_tick_size"))
                .or_else(|| dec_from_json(e.get("tickSize")))
                .or_else(|| dec_from_json(e.get("price_step")))
                .or_else(|| dec_from_json(e.get("tick_size")))
                .or(Some(dec!(0.01)));
            let size_tick = dec_from_json(e.get("order_size_increment"))
                .or_else(|| dec_from_json(e.get("stepSize")))
                .or_else(|| dec_from_json(e.get("amount_step")))
                .or_else(|| dec_from_json(e.get("min_size")))
                .or(Some(dec!(0.0001)));
            meta(
                venue,
                &base,
                quote,
                raw_symbol.to_string(),
                price_tick,
                size_tick,
                fee,
            )
        })
        .collect()
}

fn parse_markets_for(
    spec_id: &str,
    venue: &VenueId,
    data: &serde_json::Value,
    fee: Decimal,
) -> Vec<MarketMetadata> {
    match spec_id {
        "dydx" => parse_dydx_markets(venue, data, fee),
        "paradex" => parse_paradex_markets(venue, data, fee),
        "aevo" => parse_aevo_markets(venue, data, fee),
        "apex" => parse_apex_markets(venue, data, fee),
        "extended" => parse_extended_markets(venue, data, fee),
        "standx" => parse_standx_markets(venue, data, fee),
        "pacifica" => parse_pacifica_markets(venue, data, fee),
        "derive" => parse_derive_markets(venue, data, fee),
        "helix" => parse_helix_markets(venue, data),
        "qfex" => parse_qfex_markets(venue, data, fee),
        "gmx" => parse_gmx_markets(venue, data, fee),
        "edgex" => parse_edgex_markets(venue, data, fee),
        "grvt" => parse_grvt_markets(venue, data, fee),
        "ondoperps" => parse_ondo_markets(venue, data, fee),
        _ => parse_generic_markets(venue, data, fee),
    }
}

// ---------------------------------------------------------------------------
// Orderbook parsers
// ---------------------------------------------------------------------------

/// Shared `[price, size]`-pair book parser (Paradex, Aevo, ApeX, Extended…).
/// Accepts `bids`/`asks` or `b`/`a` spellings; bails when both sides empty.
fn parse_pair_book(
    venue: &VenueId,
    symbol: &str,
    data: &serde_json::Value,
) -> anyhow::Result<OrderBook> {
    let node = data.get("result").unwrap_or(data);
    let bids = node
        .get("bids")
        .or_else(|| node.get("b"))
        .and_then(|v| v.as_array());
    let asks = node
        .get("asks")
        .or_else(|| node.get("a"))
        .and_then(|v| v.as_array());
    let mut bid_levels = Vec::new();
    let mut ask_levels = Vec::new();
    if let Some(arr) = bids {
        for l in arr {
            if let Some((p, q)) = parse_pair_level(l) {
                bid_levels.push((p, q));
            }
        }
    }
    if let Some(arr) = asks {
        for l in arr {
            if let Some((p, q)) = parse_pair_level(l) {
                ask_levels.push((p, q));
            }
        }
    }
    book_from_levels(venue, symbol, bid_levels, ask_levels)
}

/// ApeX Omni depth: `{"data": {"a": [[price, qty]…], "b": [[…]]}}`
/// where `a` is asks and `b` is bids. Bails on empty both sides so a dead
/// symbol never masquerades as a quote.
fn parse_apex_book(
    venue: &VenueId,
    symbol: &str,
    data: &serde_json::Value,
) -> anyhow::Result<OrderBook> {
    let node = data.get("data").unwrap_or(data);
    let mut bid_levels = Vec::new();
    let mut ask_levels = Vec::new();
    if let Some(arr) = node.get("b").and_then(|v| v.as_array()) {
        for l in arr {
            if let Some((p, q)) = parse_pair_level(l) {
                bid_levels.push((p, q));
            }
        }
    }
    if let Some(arr) = node.get("a").and_then(|v| v.as_array()) {
        for l in arr {
            if let Some((p, q)) = parse_pair_level(l) {
                ask_levels.push((p, q));
            }
        }
    }
    book_from_levels(venue, symbol, bid_levels, ask_levels)
}

/// Extended orderbook: `{"status": "OK", "data": {"market", "bid":
/// [{"price", "qty"}…], "ask": […]}}`.
fn parse_extended_book(
    venue: &VenueId,
    symbol: &str,
    data: &serde_json::Value,
) -> anyhow::Result<OrderBook> {
    let node = data.get("data").unwrap_or(data);
    let mut bid_levels = Vec::new();
    let mut ask_levels = Vec::new();
    if let Some(arr) = node
        .get("bid")
        .or_else(|| node.get("bids"))
        .and_then(|v| v.as_array())
    {
        for l in arr {
            let p = dec_from_json(l.get("price"));
            let q = dec_from_json(l.get("qty")).or_else(|| dec_from_json(l.get("size")));
            if let (Some(p), Some(q)) = (p, q) {
                if p > dec!(0) && q > dec!(0) {
                    bid_levels.push((p, q));
                }
            }
        }
    }
    if let Some(arr) = node
        .get("ask")
        .or_else(|| node.get("asks"))
        .and_then(|v| v.as_array())
    {
        for l in arr {
            let p = dec_from_json(l.get("price"));
            let q = dec_from_json(l.get("qty")).or_else(|| dec_from_json(l.get("size")));
            if let (Some(p), Some(q)) = (p, q) {
                if p > dec!(0) && q > dec!(0) {
                    ask_levels.push((p, q));
                }
            }
        }
    }
    book_from_levels(venue, symbol, bid_levels, ask_levels)
}

/// Pacifica book: `{"success": true, "data": {"s", "l": [[bids], [asks]],
/// "t"}}` with `{p, a}` levels — `l[0]` descends (bids), `l[1]` ascends
/// (asks). Sides are assigned by array position, then sanity-checked by
/// price order (best bid must sit below best ask).
fn parse_pacifica_book(
    venue: &VenueId,
    symbol: &str,
    data: &serde_json::Value,
) -> anyhow::Result<OrderBook> {
    let node = data.get("data").unwrap_or(data);
    let sides = node.get("l").and_then(|v| v.as_array());
    let sides = match sides {
        Some(s) => s,
        None => anyhow::bail!("Pacifica book for {symbol} has no levels array"),
    };
    let parse_side = |idx: usize| -> Vec<(Decimal, Decimal)> {
        let mut out = Vec::new();
        if let Some(arr) = sides.get(idx).and_then(|v| v.as_array()) {
            for l in arr {
                if let (Some(p), Some(q)) = (dec_from_json(l.get("p")), dec_from_json(l.get("a"))) {
                    if p > dec!(0) && q > dec!(0) {
                        out.push((p, q));
                    }
                }
            }
        }
        out
    };
    // Sides are strictly positional (verified live): l[0] descends (bids),
    // l[1] ascends (asks). A crossed book — or a venue encoding flip —
    // fails closed; no side-swapping, so a genuinely locked market can
    // never be published as a clean book with inverted sides.
    let (bids, asks) = (parse_side(0), parse_side(1));
    if bids.is_empty() && asks.is_empty() {
        anyhow::bail!("Pacifica book for {symbol} returned no live levels");
    }
    if !bids.is_empty() && !asks.is_empty() && bids[0].0 >= asks[0].0 {
        anyhow::bail!("Pacifica book for {symbol} is crossed — refusing to publish");
    }
    book_from_levels(venue, symbol, bids, asks)
}

/// Helix/Injective LCD book: `{buys_price_level: [{p, q}…],
/// sells_price_level: […]}` in quote-decimal fixed-point — divide every
/// price by `scale` (10^quote_decimals from the market entry).
fn parse_helix_book(
    venue: &VenueId,
    symbol: &str,
    data: &serde_json::Value,
    scale: Decimal,
) -> anyhow::Result<OrderBook> {
    if scale <= dec!(0) {
        anyhow::bail!("Helix book for {symbol} has a non-positive price scale");
    }
    let mut bid_levels = Vec::new();
    let mut ask_levels = Vec::new();
    if let Some(arr) = data.get("buys_price_level").and_then(|v| v.as_array()) {
        for l in arr {
            if let (Some(p), Some(q)) = (dec_from_json(l.get("p")), dec_from_json(l.get("q"))) {
                if let Some(px) = p.checked_div(scale) {
                    if px > dec!(0) && q > dec!(0) {
                        bid_levels.push((px, q));
                    }
                }
            }
        }
    }
    if let Some(arr) = data.get("sells_price_level").and_then(|v| v.as_array()) {
        for l in arr {
            if let (Some(p), Some(q)) = (dec_from_json(l.get("p")), dec_from_json(l.get("q"))) {
                if let Some(px) = p.checked_div(scale) {
                    if px > dec!(0) && q > dec!(0) {
                        ask_levels.push((px, q));
                    }
                }
            }
        }
    }
    book_from_levels(venue, symbol, bid_levels, ask_levels)
}

/// Derive/Lyra top-of-book: the public `get_ticker` result carries
/// `best_bid_price` / `best_ask_price` (+amounts) but no depth endpoint
/// exists, so the book is one real level per side — documented as
/// top-of-book only, never padded with synthetic depth.
fn parse_derive_topbook(
    venue: &VenueId,
    symbol: &str,
    result: &serde_json::Value,
) -> anyhow::Result<OrderBook> {
    let bids = match (
        dec_from_json(result.get("best_bid_price")),
        dec_from_json(result.get("best_bid_amount")),
    ) {
        (Some(p), Some(q)) if p > dec!(0) && q > dec!(0) => vec![(p, q)],
        _ => Vec::new(),
    };
    let asks = match (
        dec_from_json(result.get("best_ask_price")),
        dec_from_json(result.get("best_ask_amount")),
    ) {
        (Some(p), Some(q)) if p > dec!(0) && q > dec!(0) => vec![(p, q)],
        _ => Vec::new(),
    };
    book_from_levels(venue, symbol, bids, asks)
}

/// Parse an RFC3339 timestamp (`2026-09-19T03:00:00Z`) into `DateTime<Utc>;
/// `None` on absence/parse failure so callers fall back to `Utc::now`.
fn parse_time_opt(v: Option<&serde_json::Value>) -> Option<chrono::DateTime<Utc>> {
    v.and_then(|x| x.as_str())
        .and_then(|s| s.parse::<chrono::DateTime<Utc>>().ok())
}

/// Parse a millisecond-epoch timestamp (number or string) into
/// `DateTime<Utc>`; `None` when absent/unparseable.
fn parse_millis_opt(v: Option<&serde_json::Value>) -> Option<chrono::DateTime<Utc>> {
    let ms = match v? {
        serde_json::Value::Number(n) => n.as_i64()?,
        serde_json::Value::String(s) => s.trim().parse::<i64>().ok()?,
        _ => return None,
    };
    chrono::DateTime::from_timestamp_millis(ms)
}

/// GRVT book: `{"result": {"bids": [{"price","size"}…], "asks": […]}}`.
fn parse_grvt_book(
    venue: &VenueId,
    symbol: &str,
    data: &serde_json::Value,
) -> anyhow::Result<OrderBook> {
    let node = data.get("result").unwrap_or(data);
    let mut bid_levels = Vec::new();
    let mut ask_levels = Vec::new();
    if let Some(arr) = node.get("bids").and_then(|v| v.as_array()) {
        for l in arr {
            if let Some((p, q)) = parse_obj_level(l) {
                bid_levels.push((p, q));
            }
        }
    }
    if let Some(arr) = node.get("asks").and_then(|v| v.as_array()) {
        for l in arr {
            if let Some((p, q)) = parse_obj_level(l) {
                ask_levels.push((p, q));
            }
        }
    }
    book_from_levels(venue, symbol, bid_levels, ask_levels)
}

/// dYdX v4 book: `{"bids": [{price, size}…], "asks": […]}`.
fn parse_dydx_book(
    venue: &VenueId,
    symbol: &str,
    data: &serde_json::Value,
) -> anyhow::Result<OrderBook> {
    let mut bid_levels = Vec::new();
    let mut ask_levels = Vec::new();
    if let Some(arr) = data.get("bids").and_then(|v| v.as_array()) {
        for l in arr {
            if let Some((p, q)) = parse_obj_level(l) {
                bid_levels.push((p, q));
            }
        }
    }
    if let Some(arr) = data.get("asks").and_then(|v| v.as_array()) {
        for l in arr {
            if let Some((p, q)) = parse_obj_level(l) {
                ask_levels.push((p, q));
            }
        }
    }
    book_from_levels(venue, symbol, bid_levels, ask_levels)
}

// ---------------------------------------------------------------------------
// Funding parsers
// ---------------------------------------------------------------------------

/// dYdX v4 funding comes from the markets object: `nextFundingRate` is the
/// 1-hour predicted rate for `ticker`. Returns `None` when absent so the
/// caller fails closed for that symbol only.
fn dydx_funding_from_markets(symbol_ticker: &str, data: &serde_json::Value) -> Option<Decimal> {
    data.get("markets")?
        .get(symbol_ticker)?
        .get("nextFundingRate")
        .and_then(|v| dec_from_json(Some(v)))
}

// ---------------------------------------------------------------------------
// EIP-712 / ABI helpers (alloy primitives, explicit encoding — no
// opaque signing middleware)
// ---------------------------------------------------------------------------

use alloy::primitives::{keccak256, Address, I256, U256};
use alloy::sol_types::SolValue;

/// EIP-712 domain separator for `EIP712Domain(string name,string
/// version,uint256 chainId)` — no verifyingContract (Aevo/Derive style).
fn eip712_domain_sep(name: &str, version: &str, chain_id: u64) -> [u8; 32] {
    let type_hash = keccak256(b"EIP712Domain(string name,string version,uint256 chainId)");
    let encoded = (
        type_hash,
        keccak256(name.as_bytes()),
        keccak256(version.as_bytes()),
        U256::from(chain_id),
    )
        .abi_encode();
    keccak256(&encoded).0
}

/// `keccak256(0x1901 || domainSep || structHash)` — the EIP-712 digest.
fn eip712_digest(domain_sep: &[u8; 32], struct_hash: &[u8; 32]) -> [u8; 32] {
    let mut buf = Vec::with_capacity(66);
    buf.extend_from_slice(&[0x19, 0x01]);
    buf.extend_from_slice(domain_sep);
    buf.extend_from_slice(struct_hash);
    keccak256(&buf).0
}

/// Decimal → signed 256-bit scaled integer (`value * 10^scale`), rounded
/// half-up. `None` when the value would exceed i256 range.
fn dec_to_i256(v: Decimal, scale: u32) -> Option<I256> {
    let scaled = (v * Decimal::from(10u64.pow(scale.min(28)))).round();
    I256::from_str(&scaled.to_string()).ok()
}

fn dec_to_u256(v: Decimal, scale: u32) -> Option<U256> {
    let scaled = (v * Decimal::from(10u64.pow(scale.min(28)))).round();
    if scaled < dec!(0) {
        return None;
    }
    U256::from_str(&scaled.to_string()).ok()
}

/// Serialize a JSON value with keys sorted recursively — the
/// deterministic compact encoding Pacifica signs over. serde_json's
/// Map ordering is feature-dependent, so this never relies on it.
fn canonical_json(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap_or_default(),
                        canonical_json(&m[k])
                    )
                })
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        serde_json::Value::Array(a) => {
            let inner: Vec<String> = a.iter().map(canonical_json).collect();
            format!("[{}]", inner.join(","))
        }
        _ => serde_json::to_string(v).unwrap_or_default(),
    }
}

/// Map venue order status strings to the engine enum. Unknown strings
/// fail closed to `None` so a new venue state never masquerades as a
/// terminal fill.
fn map_order_status(
    s: &str,
    filled: Decimal,
    original: Decimal,
) -> Option<arbitrageur_core::types::OrderStatus> {
    use arbitrageur_core::types::OrderStatus::*;
    let lower = s.to_lowercase();
    match lower.as_str() {
        "new" | "open" | "live" | "untriggered" | "active" | "pending" => Some(New),
        "partially_filled" | "partiallyfilled" | "partial" | "partial_fill" => {
            Some(PartiallyFilled)
        }
        "filled" | "closed" | "done" | "fully_filled" => Some(Filled),
        "cancelled" | "canceled" | "cancel" => Some(Cancelled),
        "rejected" | "failed" | "error" => Some(Rejected),
        "expired" => Some(Expired),
        _ => {
            // Infer from quantities only when the string is absent —
            // qty-based inference is authoritative venue state.
            if s.is_empty() {
                if filled >= original && original > dec!(0) {
                    Some(Filled)
                } else if filled > dec!(0) {
                    Some(PartiallyFilled)
                } else {
                    Some(New)
                }
            } else {
                None
            }
        }
    }
}

impl DexPerpAdapter {
    // ------------------------------------------------------------------
    // Ondo market-field lookup (funding divisor, increments)
    // ------------------------------------------------------------------
    async fn ondo_market_field(&self, symbol: &str, field: &str) -> Option<serde_json::Value> {
        let vsym = self.venue_symbol(symbol);
        let data = self.get_json("/v1/markets").await.ok()?;
        let arr = data
            .get("result")?
            .get("perps")?
            .get("tradingPairs")?
            .as_array()?;
        arr.iter()
            .find(|e| e.get("market").and_then(|s| s.as_str()) == Some(vsym.as_str()))
            .and_then(|e| e.get(field).cloned())
    }

    // ------------------------------------------------------------------
    // Pacifica (Ed25519 signed ops)
    // ------------------------------------------------------------------

    /// Sign a Pacifica operation: compact JSON
    /// `{data, expiry_window, timestamp, type}` (keys sorted) → Ed25519
    /// signature, base58-encoded. Returns (timestamp_ms, sig_b58).
    fn pacifica_sign_op(
        &self,
        op_type: &str,
        data: &serde_json::Value,
    ) -> anyhow::Result<(i64, String)> {
        let key = self.ed25519.read().clone().ok_or_else(|| {
            anyhow::anyhow!("Pacifica requires a base58 Ed25519 private key (agent wallet)")
        })?;
        let ts = Utc::now().timestamp_millis();
        let op = serde_json::json!({
            "data": data,
            "expiry_window": 5000i64,
            "timestamp": ts,
            "type": op_type,
        });
        let payload = canonical_json(&op);
        use ed25519_dalek::Signer;
        let sig = key.sign(payload.as_bytes());
        Ok((ts, bs58::encode(sig.to_bytes()).into_string()))
    }

    /// POST a signed Pacifica op. `account` is the Solana wallet pubkey.
    async fn pacifica_op(
        &self,
        op_type: &str,
        data: &serde_json::Value,
        path: &str,
    ) -> anyhow::Result<serde_json::Value> {
        let account = self
            .creds
            .read()
            .wallet_address
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Pacifica requires the Solana wallet address"))?;
        let (ts, signature) = self.pacifica_sign_op(op_type, data)?;
        let mut body = data.clone();
        if let Some(obj) = body.as_object_mut() {
            obj.insert("account".to_string(), serde_json::json!(account));
            obj.insert("signature".to_string(), serde_json::json!(signature));
            obj.insert("timestamp".to_string(), serde_json::json!(ts));
            obj.insert("expiry_window".to_string(), serde_json::json!(5000i64));
        }
        let url = format!("{}{}", self.base_url, path);
        let resp = self.post_json_url(&url, &body, &[]).await?;
        if resp.get("success").and_then(|v| v.as_bool()) == Some(false) {
            let err = resp
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            anyhow::bail!("Pacifica {op_type} rejected: {err}");
        }
        Ok(resp)
    }

    async fn pacifica_account_get(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        let account = self
            .creds
            .read()
            .wallet_address
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Pacifica requires the Solana wallet address"))?;
        let sep = if path.contains('?') { "&" } else { "?" };
        let data = self
            .get_json(&format!("{path}{sep}account={account}"))
            .await?;
        if data.get("success").and_then(|v| v.as_bool()) == Some(false) {
            let err = data
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            anyhow::bail!("Pacifica {path} failed: {err}");
        }
        Ok(data)
    }

    // ------------------------------------------------------------------
    // StandX (wallet JWT + Ed25519 body signatures)
    // ------------------------------------------------------------------

    /// Ensure a live StandX JWT: EIP-191 sign the challenge message from
    /// `prepare-signin`, then `login`. The JWT payload embeds a
    /// `requestId` bound to our ephemeral Ed25519 key.
    async fn standx_jwt(&self) -> anyhow::Result<String> {
        if let Some(s) = self.session.read().as_ref() {
            if s.expires_at > Utc::now() + chrono::Duration::minutes(2) {
                return Ok(s.token.clone());
            }
        }
        if self.wallet.read().is_none() {
            anyhow::bail!("StandX requires an EVM private key for wallet sign-in");
        }
        let wallet = self
            .creds
            .read()
            .wallet_address
            .clone()
            .ok_or_else(|| anyhow::anyhow!("StandX requires the EVM wallet address"))?;
        let ed = self.ed25519.read().clone().ok_or_else(|| {
            anyhow::anyhow!("StandX requires an Ed25519 request key (bs58 private key field)")
        })?;
        // requestId = bs58(ed25519 pubkey) — binds request signatures.
        let req_id = bs58::encode(ed.verifying_key().to_bytes()).into_string();
        let prep = self
            .post_json_url(
                &format!("{}/v1/offchain/prepare-signin?chain=bsc", self.base_url),
                &serde_json::json!({"address": wallet, "requestId": req_id}),
                &[],
            )
            .await?;
        let signed_data = prep
            .get("signedData")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("StandX prepare-signin returned no signedData"))?;
        // signedData is a JWT whose payload.message is the SIWE-style
        // challenge to sign. Decode payload segment (base64url JSON).
        let payload_b64 = signed_data
            .split('.')
            .nth(1)
            .ok_or_else(|| anyhow::anyhow!("StandX signedData is not a JWT"))?;
        let payload_json = base64_decode_url(payload_b64)?;
        let payload: serde_json::Value = serde_json::from_slice(&payload_json)?;
        let message = payload
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("StandX challenge has no message"))?;
        // Scope the lock guard so it never lives across an `.await`
        // (parking_lot guards are not Send).
        let sig = {
            let guard = self.wallet.read();
            let signer = guard
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("StandX requires an EVM private key"))?;
            signer.sign_personal_message(message.as_bytes())?
        };
        let login = self
            .post_json_url(
                &format!("{}/v1/offchain/login", self.base_url),
                &serde_json::json!({"signature": format!("0x{}", hex::encode(sig)), "signedData": signed_data}),
                &[],
            )
            .await?;
        let token = login
            .get("token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("StandX login returned no token"))?
            .to_string();
        let expires_at = Utc::now() + chrono::Duration::days(6);
        *self.session.write() = Some(DexPerpSession {
            token: token.clone(),
            account_id: None,
            expires_at,
        });
        Ok(token)
    }

    /// StandX request headers: JWT bearer + Ed25519 body signature
    /// `base64(ed25519("v1,{requestId},{ts},{body}"))` + request id/ts.
    async fn standx_headers(
        &self,
        body: &serde_json::Value,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let token = self.standx_jwt().await?;
        let ed = self
            .ed25519
            .read()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("StandX requires an Ed25519 request key"))?;
        let req_id = bs58::encode(ed.verifying_key().to_bytes()).into_string();
        let ts = Utc::now().timestamp_millis().to_string();
        let body_str = serde_json::to_string(body)?;
        use ed25519_dalek::Signer;
        let sig = ed.sign(format!("v1,{req_id},{ts},{body_str}").as_bytes());
        Ok(vec![
            ("authorization".into(), format!("Bearer {token}")),
            ("x-request-id".into(), req_id),
            ("x-request-timestamp".into(), ts),
            ("x-request-signature".into(), base64_encode(&sig.to_bytes())),
        ])
    }

    // ------------------------------------------------------------------
    // Ondo Perps (SIWE → JWT)
    // ------------------------------------------------------------------

    async fn ondo_jwt(&self) -> anyhow::Result<String> {
        if let Some(s) = self.session.read().as_ref() {
            if s.expires_at > Utc::now() + chrono::Duration::minutes(2) {
                return Ok(s.token.clone());
            }
        }
        let wallet = self
            .creds
            .read()
            .wallet_address
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Ondo Perps requires the EVM wallet address"))?;
        let challenge = self
            .post_json_url(
                &format!("{}/v1/auth/erc-4361/login/get_challenge", self.base_url),
                &serde_json::json!({"walletAddress": wallet, "chainId": "1"}),
                &[],
            )
            .await?;
        let id = challenge
            .get("result")
            .and_then(|r| r.get("id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Ondo challenge returned no id"))?
            .to_string();
        let message = challenge
            .get("result")
            .and_then(|r| r.get("message"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Ondo challenge returned no message"))?
            .to_string();
        let sig = {
            let guard = self.wallet.read();
            let signer = guard.as_ref().ok_or_else(|| {
                anyhow::anyhow!("Ondo Perps requires an EVM private key for SIWE")
            })?;
            signer.sign_personal_message(message.as_bytes())?
        };
        let login = self
            .post_json_url(
                &format!(
                    "{}/v1/auth/erc-4361/login/complete_challenge",
                    self.base_url
                ),
                &serde_json::json!({"id": id, "signature": format!("0x{}", hex::encode(sig))}),
                &[],
            )
            .await?;
        let result = login
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let token = result
            .get("token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Ondo login returned no token"))?
            .to_string();
        let expires_at = result
            .get("expirationSecs")
            .and_then(|v| v.as_i64())
            .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
            .unwrap_or_else(|| Utc::now() + chrono::Duration::hours(1));
        *self.session.write() = Some(DexPerpSession {
            token: token.clone(),
            account_id: None,
            expires_at,
        });
        Ok(token)
    }

    async fn ondo_auth_headers(&self) -> anyhow::Result<Vec<(String, String)>> {
        Ok(vec![(
            "authorization".into(),
            format!("Bearer {}", self.ondo_jwt().await?),
        )])
    }

    // ------------------------------------------------------------------
    // GRVT (api-key session + EIP-712 orders)
    // ------------------------------------------------------------------

    /// GRVT session: POST api_key login on the edge host with
    /// `Cookie: rm=true` → `gravity` cookie + `x-grvt-account-id`.
    async fn grvt_session(&self) -> anyhow::Result<(String, String)> {
        if let Some(s) = self.session.read().as_ref() {
            if s.expires_at > Utc::now() + chrono::Duration::minutes(2) {
                if let Some(a) = &s.account_id {
                    return Ok((s.token.clone(), a.clone()));
                }
            }
        }
        let api_key = self
            .creds
            .read()
            .api_key
            .clone()
            .ok_or_else(|| anyhow::anyhow!("GRVT requires an API key"))?;
        let resp = self
            .client
            .post("https://edge.grvt.io/auth/api_key/login")
            .header("cookie", "rm=true")
            .json(&serde_json::json!({"api_key": api_key}))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "GRVT api-key login failed: {} — {}",
                status,
                text.trim().chars().take(200).collect::<String>()
            );
        }
        let account_hdr = resp
            .headers()
            .get("x-grvt-account-id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let cookie = resp
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|c| c.starts_with("gravity="))
            .and_then(|c| c.split(';').next().map(|s| s.to_string()))
            .ok_or_else(|| anyhow::anyhow!("GRVT login returned no gravity cookie"))?;
        // Configured account_id overrides the login echo.
        let account_id = self
            .creds
            .read()
            .account_id
            .clone()
            .or(account_hdr)
            .ok_or_else(|| {
                anyhow::anyhow!("GRVT login returned no account id — set account_id in credentials")
            })?;
        *self.session.write() = Some(DexPerpSession {
            token: cookie.clone(),
            account_id: Some(account_id.clone()),
            expires_at: Utc::now() + chrono::Duration::hours(12),
        });
        Ok((cookie, account_id))
    }

    /// GRVT EIP-712 `Order` signature. legs' `assetID` is the
    /// instrument_hash from `all_instruments`; price/size scale ×1e9.
    #[allow(clippy::too_many_arguments)]
    fn grvt_order_sig(
        &self,
        sub_account_id: u64,
        is_market: bool,
        time_in_force: u8,
        post_only: bool,
        reduce_only: bool,
        asset_id: U256,
        size_1e9: u64,
        price_1e9: u64,
        is_buy: bool,
        nonce: u32,
        expiration_ns: i64,
        chain_id: u64,
    ) -> anyhow::Result<String> {
        let signer = self.wallet.read();
        let signer = signer
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("GRVT requires an EVM private key for order signing"))?;
        let leg_type = keccak256(b"OrderLeg(uint256 assetID,uint64 contractSize,uint64 limitPrice,bool isBuyingContract)");
        let leg_hash = keccak256(
            (
                leg_type,
                asset_id,
                U256::from(size_1e9),
                U256::from(price_1e9),
                is_buy,
            )
                .abi_encode(),
        );
        let legs_hash = keccak256(leg_hash); // array: keccak(concat(per-leg hashes))
        let order_type = keccak256(b"Order(uint64 subAccountID,bool isMarket,uint8 timeInForce,bool postOnly,bool reduceOnly,OrderLeg[] legs,uint32 nonce,int64 expiration)OrderLeg(uint256 assetID,uint64 contractSize,uint64 limitPrice,bool isBuyingContract)");
        let struct_hash = keccak256(
            (
                order_type,
                U256::from(sub_account_id),
                is_market,
                U256::from(time_in_force),
                post_only,
                reduce_only,
                legs_hash,
                U256::from(nonce),
                I256::from_raw(U256::from(expiration_ns as u64)),
            )
                .abi_encode(),
        );
        let domain = eip712_domain_sep("GRVT Exchange", "0", chain_id);
        let digest = eip712_digest(&domain, &struct_hash);
        let sig = signer.sign_digest(&digest)?;
        Ok(format!("0x{}", hex::encode(sig)))
    }

    /// Resolve instrument_hash + tick metadata from cached instruments.
    async fn grvt_instrument(
        &self,
        venue_symbol: &str,
    ) -> anyhow::Result<(U256, serde_json::Value)> {
        let url = format!("{}/full/v1/all_instruments", self.base_url);
        let data = self
            .post_json_url(&url, &serde_json::json!({}), &[])
            .await?;
        let arr = data
            .get("result")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow::anyhow!("GRVT instruments missing result"))?;
        for e in arr {
            let name = e
                .get("i")
                .or_else(|| e.get("instrument"))
                .and_then(|s| s.as_str());
            if name == Some(venue_symbol) {
                let ih = e
                    .get("ih")
                    .or_else(|| e.get("instrument_hash"))
                    .and_then(|s| s.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!("GRVT instrument {venue_symbol} has no instrument_hash")
                    })?;
                let asset = U256::from_str(ih.trim_start_matches("0x"))
                    .or_else(|_| U256::from_str_radix(ih.trim_start_matches("0x"), 16))
                    .map_err(|_| anyhow::anyhow!("GRVT instrument_hash {ih} unparseable"))?;
                return Ok((asset, e.clone()));
            }
        }
        anyhow::bail!("GRVT has no instrument {venue_symbol}")
    }

    // ------------------------------------------------------------------
    // Derive (X-Lyra headers + EIP-712 TradeModule signature)
    // ------------------------------------------------------------------

    /// Derive auth headers: `X-LyraWallet`, `X-LyraTimestamp` (ms),
    /// `X-LyraSignature` = 0x + hex(personal_sign(timestamp)).
    fn derive_headers(&self) -> anyhow::Result<Vec<(String, String)>> {
        let wallet = self
            .creds
            .read()
            .wallet_address
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Derive requires the funding wallet address"))?;
        let ts = Utc::now().timestamp_millis().to_string();
        let guard = self.wallet.read();
        let signer = guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Derive requires an EVM private key (session key)"))?;
        let sig = signer.sign_personal_message(ts.as_bytes())?;
        Ok(vec![
            ("x-lyrawallet".into(), wallet),
            ("x-lyratimestamp".into(), ts),
            ("x-lyrasignature".into(), format!("0x{}", hex::encode(sig))),
        ])
    }

    /// Derive EIP-712 action signature over the TradeModule encoding.
    /// domain separator is the published Derive-chain constant.
    #[allow(clippy::too_many_arguments)]
    fn derive_order_sig(
        &self,
        asset: Address,
        sub_id: U256,
        limit_price: I256,
        amount: I256,
        max_fee: U256,
        recipient_sub_id: U256,
        is_bid: bool,
        subaccount_id: U256,
        nonce: U256,
        expiry_sec: U256,
    ) -> anyhow::Result<String> {
        const ACTION_TYPEHASH: [u8; 32] = [
            0x4d, 0x7a, 0x9f, 0x27, 0xc4, 0x03, 0xff, 0x9c, 0x0f, 0x19, 0xbc, 0xe6, 0x1d, 0x76,
            0xd8, 0x2f, 0x9a, 0xa2, 0x9f, 0x8d, 0x6d, 0x4b, 0x0c, 0x54, 0x74, 0x60, 0x7d, 0x97,
            0x70, 0xd1, 0xaf, 0x17,
        ];
        const DOMAIN_SEP: [u8; 32] = [
            0xd9, 0x6e, 0x5f, 0x90, 0x79, 0x7d, 0xa7, 0xec, 0x8d, 0xc4, 0xe2, 0x76, 0x26, 0x0c,
            0x7f, 0x3f, 0x87, 0xfe, 0xdf, 0x68, 0x77, 0x5f, 0xbe, 0x1e, 0xf1, 0x16, 0xe9, 0x96,
            0xfc, 0x60, 0x44, 0x1b,
        ];
        const TRADE_MODULE: &str = "0xB8D20c2B7a1Ad2EE33Bc50eF10876eD3035b5e7b";
        let wallet_addr = self
            .creds
            .read()
            .wallet_address
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Derive requires the funding wallet address"))?;
        let owner: Address = wallet_addr
            .parse()
            .map_err(|_| anyhow::anyhow!("Derive wallet address unparseable"))?;
        let guard = self.wallet.read();
        let signer = guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Derive requires an EVM private key (session key)"))?;
        let signer_addr: Address = signer
            .address_hex()
            .parse()
            .map_err(|_| anyhow::anyhow!("Derive signer address unparseable"))?;
        let module: Address = TRADE_MODULE
            .parse()
            .map_err(|_| anyhow::anyhow!("Derive trade module address unparseable"))?;

        let module_data = (
            asset,
            sub_id,
            limit_price,
            amount,
            max_fee,
            recipient_sub_id,
            is_bid,
        )
            .abi_encode();
        let module_data_hash = keccak256(&module_data);
        let action_hash = keccak256(
            (
                ACTION_TYPEHASH,
                subaccount_id,
                nonce,
                module,
                module_data_hash,
                expiry_sec,
                owner,
                signer_addr,
            )
                .abi_encode(),
        );
        let digest = eip712_digest(&DOMAIN_SEP, &action_hash);
        let sig = signer.sign_digest(&digest)?;
        Ok(format!("0x{}", hex::encode(sig)))
    }

    /// Fetch Derive instrument signing metadata (asset address + sub id).
    async fn derive_instrument(&self, venue_symbol: &str) -> anyhow::Result<(Address, U256)> {
        let data = self
            .get_json(&format!(
                "/public/get_instrument?instrument_name={venue_symbol}"
            ))
            .await?;
        let result = data
            .get("result")
            .ok_or_else(|| anyhow::anyhow!("Derive has no instrument {venue_symbol}"))?;
        let addr_s = result
            .get("base_asset_address")
            .and_then(|s| s.as_str())
            .ok_or_else(|| {
                anyhow::anyhow!("Derive instrument {venue_symbol} has no base_asset_address")
            })?;
        let sub = dec_from_json(result.get("base_asset_sub_id"))
            .and_then(|d| dec_to_u256(d, 0))
            .or_else(|| {
                result
                    .get("base_asset_sub_id")
                    .and_then(|v| v.as_u64())
                    .map(U256::from)
            })
            .ok_or_else(|| {
                anyhow::anyhow!("Derive instrument {venue_symbol} has no base_asset_sub_id")
            })?;
        let addr: Address = addr_s
            .parse()
            .map_err(|_| anyhow::anyhow!("Derive asset address {addr_s} unparseable"))?;
        Ok((addr, sub))
    }

    // ------------------------------------------------------------------
    // Aevo (API key headers + EIP-712 order)
    // ------------------------------------------------------------------

    fn aevo_headers(&self) -> anyhow::Result<Vec<(String, String)>> {
        let c = self.creds.read();
        let key = c
            .api_key
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Aevo requires an API key"))?;
        let secret = c
            .api_secret
            .as_ref()
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Aevo requires an API secret"))?;
        Ok(vec![
            ("aevo-key".into(), key),
            ("aevo-secret".into(), secret),
        ])
    }

    /// Aevo EIP-712 `Order` signature (domain: "Aevo Mainnet","1",chain 1).
    /// `amount`/`limit_price` are 6-decimal fixed-point integers.
    fn aevo_order_sig(
        &self,
        is_buy: bool,
        limit_price: U256,
        amount: U256,
        salt: U256,
        instrument: U256,
        timestamp: U256,
    ) -> anyhow::Result<(String, String)> {
        let maker_s = self
            .creds
            .read()
            .wallet_address
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Aevo requires the maker wallet address"))?;
        let maker: Address = maker_s
            .parse()
            .map_err(|_| anyhow::anyhow!("Aevo maker address unparseable"))?;
        let guard = self.wallet.read();
        let signer = guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Aevo requires an EVM private key for order signing"))?;
        let type_hash = keccak256(b"Order(address maker,bool isBuy,uint256 limitPrice,uint256 amount,uint256 salt,uint256 instrument,uint256 timestamp)");
        let struct_hash = keccak256(
            (
                type_hash,
                maker,
                is_buy,
                limit_price,
                amount,
                salt,
                instrument,
                timestamp,
            )
                .abi_encode(),
        );
        let domain = eip712_domain_sep("Aevo Mainnet", "1", 1);
        let digest = eip712_digest(&domain, &struct_hash);
        let sig = signer.sign_digest(&digest)?;
        Ok((format!("0x{}", hex::encode(sig)), maker_s))
    }

    /// Resolve Aevo numeric instrument id from the markets list.
    async fn aevo_instrument_id(&self, venue_symbol: &str) -> anyhow::Result<u64> {
        let data = self.get_json("/markets").await?;
        let arr = data
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Aevo markets missing array"))?;
        for e in arr {
            if e.get("instrument_name").and_then(|s| s.as_str()) == Some(venue_symbol) {
                return e
                    .get("instrument_id")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| anyhow::anyhow!("Aevo instrument {venue_symbol} has no id"));
            }
        }
        anyhow::bail!("Aevo has no instrument {venue_symbol}")
    }

    // ------------------------------------------------------------------
    // Shared order-response assembly
    // ------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn order_response(
        &self,
        order_id: String,
        client_order_id: String,
        symbol: &str,
        status: arbitrageur_core::types::OrderStatus,
        original: Decimal,
        filled: Decimal,
        avg_price: Option<Decimal>,
        raw: serde_json::Value,
    ) -> OrderResponse {
        OrderResponse {
            order_id,
            client_order_id,
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            status,
            original_qty: original,
            filled_qty: filled,
            remaining_qty: (original - filled).max(dec!(0)),
            avg_fill_price: avg_price,
            fee: dec!(0),
            fee_currency: "USD".to_string(),
            raw_response: Some(raw.to_string()),
            timestamp: Utc::now(),
        }
    }
}

/// Decode a base64url (no pad) string — StandX JWT payload segment.
fn base64_decode_url(s: &str) -> anyhow::Result<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|e| anyhow::anyhow!("base64url decode failed: {e}"))
}

fn base64_encode(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(b)
}

/// Aggregate a venue position row into the engine's Position shape.
/// `qty` is the venue-native signed/base amount; `side` is authoritative.
#[allow(clippy::too_many_arguments)]
fn position_from(
    venue: &VenueId,
    symbol: String,
    side: arbitrageur_core::types::Side,
    qty: Decimal,
    entry: Option<Decimal>,
    mark: Option<Decimal>,
    upnl: Option<Decimal>,
    liq: Option<Decimal>,
    leverage: Option<Decimal>,
) -> arbitrageur_core::position::Position {
    arbitrageur_core::position::Position {
        venue: venue.clone(),
        symbol,
        side,
        quantity: qty.abs(),
        entry_price: entry.unwrap_or_default(),
        mark_price: mark.unwrap_or_default(),
        unrealized_pnl: upnl.unwrap_or_default(),
        realized_pnl: dec!(0),
        liquidation_price: liq,
        leverage: leverage.unwrap_or_default(),
        initial_margin: dec!(0),
        maintenance_margin: dec!(0),
        updated_at: Utc::now(),
    }
}

impl DexPerpAdapter {
    /// Worst-case bound for market orders that need a limit price:
    /// explicit price wins; otherwise a 5% band off the live book top.
    async fn market_bound_price(&self, order: &OrderRequest) -> anyhow::Result<Decimal> {
        if let Some(p) = order.price {
            if p > dec!(0) {
                return Ok(p);
            }
        }
        let book = self.fetch_orderbook(&order.symbol).await?;
        match order.side {
            arbitrageur_core::types::Side::Buy => book
                .best_ask_price()
                .map(|p| p * dec!(1.05))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "{} book for {} has no ask for bound",
                        self.name,
                        order.symbol
                    )
                }),
            arbitrageur_core::types::Side::Sell => book
                .best_bid_price()
                .map(|p| p * dec!(0.95))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "{} book for {} has no bid for bound",
                        self.name,
                        order.symbol
                    )
                }),
        }
    }

    // ------------------------------------------------------------------
    // Pacifica order paths
    // ------------------------------------------------------------------

    async fn pacifica_place(&self, order: &OrderRequest) -> anyhow::Result<OrderResponse> {
        let vsym = self.venue_symbol(&order.symbol);
        let side = if order.side.is_buy() { "bid" } else { "ask" };
        let is_market = order.order_type == arbitrageur_core::types::OrderType::Market;
        let mut data = serde_json::json!({
            "symbol": vsym,
            "side": side,
            "amount": order.quantity.to_string(),
            "reduce_only": order.reduce_only,
            "client_order_id": order.client_order_id,
        });
        if is_market {
            data["slippage_percent"] = serde_json::json!("0.5");
        } else {
            let price = order
                .price
                .ok_or_else(|| anyhow::anyhow!("Pacifica limit orders require a price"))?;
            data["price"] = serde_json::json!(price.to_string());
            let tif = match order.order_type {
                arbitrageur_core::types::OrderType::ImmediateOrCancel => "IOC",
                arbitrageur_core::types::OrderType::PostOnly => "ALO",
                _ => "GTC",
            };
            data["tif"] = serde_json::json!(tif);
        }
        let op = if is_market {
            "create_market_order"
        } else {
            "create_order"
        };
        let path = if is_market {
            "/api/v1/orders/create_market"
        } else {
            "/api/v1/orders/create"
        };
        let resp = self.pacifica_op(op, &data, path).await?;
        let d = resp.get("data").cloned().unwrap_or(serde_json::Value::Null);
        let order_id = d
            .get("order_id")
            .or_else(|| d.get("orderId"))
            .and_then(|v| {
                v.as_i64()
                    .map(|i| i.to_string())
                    .or_else(|| v.as_str().map(|s| s.to_string()))
            })
            .ok_or_else(|| anyhow::anyhow!("Pacifica create_order returned no order_id: {resp}"))?;
        Ok(self.order_response(
            order_id,
            order.client_order_id.clone(),
            &order.symbol,
            arbitrageur_core::types::OrderStatus::New,
            order.quantity,
            dec!(0),
            None,
            resp,
        ))
    }

    async fn pacifica_cancel(&self, req: &CancelRequest) -> anyhow::Result<()> {
        let vsym = self.venue_symbol(&req.symbol);
        let mut data = serde_json::json!({"symbol": vsym});
        if let Some(id) = req.order_id.as_deref() {
            if let Ok(n) = id.parse::<u64>() {
                data["order_id"] = serde_json::json!(n);
            } else {
                data["client_order_id"] = serde_json::json!(id);
            }
        } else if let Some(id) = req.client_order_id.as_deref() {
            data["client_order_id"] = serde_json::json!(id);
        } else {
            anyhow::bail!("Pacifica cancel needs an order_id or client_order_id");
        }
        self.pacifica_op("cancel_order", &data, "/api/v1/orders/cancel")
            .await?;
        Ok(())
    }

    async fn pacifica_fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        let vsym = self.venue_symbol(symbol);
        // Open orders first, then history — venue truth only.
        for path in [
            format!("/api/v1/orders?symbol={vsym}"),
            format!("/api/v1/orders/history?symbol={vsym}&limit=100"),
        ] {
            let data = self.pacifica_account_get(&path).await?;
            let arr = data
                .get("data")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            for e in arr {
                let id_match = e
                    .get("order_id")
                    .and_then(|v| {
                        v.as_i64()
                            .map(|i| i.to_string())
                            .or_else(|| v.as_str().map(|s| s.to_string()))
                    })
                    .map(|s| s == order_id)
                    == Some(true);
                let cid_match = client_order_id.is_some()
                    && e.get("client_order_id").and_then(|v| v.as_str()) == client_order_id;
                if !(id_match || cid_match) {
                    continue;
                }
                let orig = dec_from_json(e.get("amount")).unwrap_or_default();
                let filled = dec_from_json(e.get("filled_amount")).unwrap_or_default();
                let status_s = e
                    .get("order_status")
                    .or_else(|| e.get("status"))
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                let status = map_order_status(status_s, filled, orig)
                    .ok_or_else(|| anyhow::anyhow!("Pacifica order {order_id} has unknown status '{status_s}' — refusing to guess"))?;
                let avg = dec_from_json(e.get("average_price"))
                    .or_else(|| dec_from_json(e.get("avg_price")));
                let cid = e
                    .get("client_order_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                return Ok(self.order_response(
                    order_id.to_string(),
                    cid,
                    symbol,
                    status,
                    orig,
                    filled,
                    avg,
                    e,
                ));
            }
        }
        anyhow::bail!(
            "Pacifica order {order_id} not found in open/history — refusing to assume a state"
        )
    }

    async fn pacifica_account(&self) -> anyhow::Result<AccountSnapshot> {
        let acct = self.pacifica_account_get("/api/v1/account").await?;
        let a = acct.get("data").cloned().unwrap_or(serde_json::Value::Null);
        let equity = dec_from_json(a.get("account_equity"))
            .or_else(|| dec_from_json(a.get("equity")))
            .ok_or_else(|| anyhow::anyhow!("Pacifica account has no equity field"))?;
        let free = dec_from_json(a.get("available_to_spend"))
            .or_else(|| dec_from_json(a.get("available")))
            .unwrap_or_default();
        let pos = self.pacifica_account_get("/api/v1/positions").await?;
        let mut positions = Vec::new();
        if let Some(arr) = pos.get("data").and_then(|v| v.as_array()) {
            for p in arr {
                let qty = dec_from_json(p.get("amount")).unwrap_or_default();
                if qty <= dec!(0) {
                    continue;
                }
                let sym = p.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
                let side_s = p.get("side").and_then(|s| s.as_str()).unwrap_or("bid");
                let side = if side_s == "ask" || side_s == "sell" {
                    arbitrageur_core::types::Side::Sell
                } else {
                    arbitrageur_core::types::Side::Buy
                };
                positions.push(position_from(
                    &self.venue_id,
                    format!("{}-USD", sym.to_uppercase()),
                    side,
                    qty,
                    dec_from_json(p.get("entry_price")),
                    dec_from_json(p.get("mark_price")),
                    dec_from_json(p.get("unrealized_pnl")).or_else(|| dec_from_json(p.get("pnl"))),
                    dec_from_json(p.get("liquidation_price")),
                    dec_from_json(p.get("leverage")),
                ));
            }
        }
        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity,
            free_margin: free,
            maintenance_margin: dec!(0),
            margin_usage_pct: dec!(0),
            balances: vec![arbitrageur_core::position::Balance {
                venue: self.venue_id.clone(),
                currency: "USDC".to_string(),
                total: equity,
                free,
                locked: (equity - free).max(dec!(0)),
            }],
            positions,
            timestamp: Utc::now(),
        })
    }

    // ------------------------------------------------------------------
    // StandX order paths
    // ------------------------------------------------------------------

    async fn standx_place(&self, order: &OrderRequest) -> anyhow::Result<OrderResponse> {
        let vsym = self.venue_symbol(&order.symbol);
        let is_market = order.order_type == arbitrageur_core::types::OrderType::Market;
        let tif = match order.order_type {
            arbitrageur_core::types::OrderType::ImmediateOrCancel => "ioc",
            arbitrageur_core::types::OrderType::PostOnly => "alo",
            _ => "gtc",
        };
        let mut body = serde_json::json!({
            "symbol": vsym,
            "side": if order.side.is_buy() { "buy" } else { "sell" },
            "order_type": if is_market { "market" } else { "limit" },
            "qty": order.quantity.to_string(),
            "time_in_force": tif,
            "reduce_only": order.reduce_only,
            "cl_ord_id": order.client_order_id,
        });
        if !is_market {
            let price = order
                .price
                .ok_or_else(|| anyhow::anyhow!("StandX limit orders require a price"))?;
            body["price"] = serde_json::json!(price.to_string());
        }
        let headers = self.standx_headers(&body).await?;
        let url = format!("{}/api/new_order", self.base_url);
        let resp = self.post_json_url(&url, &body, &headers).await?;
        if resp.get("code").and_then(|v| v.as_i64()) != Some(0) {
            anyhow::bail!("StandX new_order rejected: {}", resp);
        }
        let order_id = resp
            .get("data")
            .and_then(|d| d.get("order_id"))
            .or_else(|| resp.get("order_id"))
            .and_then(|v| {
                v.as_i64()
                    .map(|i| i.to_string())
                    .or_else(|| v.as_str().map(|s| s.to_string()))
            })
            .unwrap_or_else(|| order.client_order_id.clone());
        Ok(self.order_response(
            order_id,
            order.client_order_id.clone(),
            &order.symbol,
            arbitrageur_core::types::OrderStatus::New,
            order.quantity,
            dec!(0),
            None,
            resp,
        ))
    }

    async fn standx_cancel(&self, req: &CancelRequest) -> anyhow::Result<()> {
        let mut body = serde_json::json!({});
        if let Some(id) = req.order_id.as_deref().and_then(|s| s.parse::<i64>().ok()) {
            body["order_id"] = serde_json::json!(id);
        } else if let Some(id) = req.client_order_id.as_deref() {
            body["cl_ord_id"] = serde_json::json!(id);
        } else if let Some(id) = req.order_id.as_deref() {
            body["cl_ord_id"] = serde_json::json!(id);
        } else {
            anyhow::bail!("StandX cancel needs an order_id or client_order_id");
        }
        let headers = self.standx_headers(&body).await?;
        let url = format!("{}/api/cancel_order", self.base_url);
        let resp = self.post_json_url(&url, &body, &headers).await?;
        if resp.get("code").and_then(|v| v.as_i64()) != Some(0) {
            anyhow::bail!("StandX cancel_order rejected: {}", resp);
        }
        Ok(())
    }

    async fn standx_fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        let headers = {
            let token = self.standx_jwt().await?;
            vec![("authorization".into(), format!("Bearer {token}"))]
        };
        let param = if let Ok(n) = order_id.parse::<i64>() {
            format!("order_id={n}")
        } else if let Some(cid) = client_order_id {
            format!("cl_ord_id={cid}")
        } else {
            format!("cl_ord_id={order_id}")
        };
        let data = self
            .get_json_url(
                &format!("{}/api/query_order?{param}", self.base_url),
                &headers,
            )
            .await?;
        let e = data.get("result").unwrap_or(&data).clone();
        if e.get("id").is_none() && e.get("order_id").is_none() && e.get("cl_ord_id").is_none() {
            anyhow::bail!("StandX order {order_id} not found — refusing to assume a state");
        }
        let orig = dec_from_json(e.get("qty")).unwrap_or_default();
        let filled = dec_from_json(e.get("fill_qty")).unwrap_or_default();
        let status_s = e.get("status").and_then(|s| s.as_str()).unwrap_or("");
        let status = map_order_status(status_s, filled, orig).ok_or_else(|| {
            anyhow::anyhow!(
                "StandX order {order_id} has unknown status '{status_s}' — refusing to guess"
            )
        })?;
        let avg = dec_from_json(e.get("fill_avg_price"));
        let cid = e
            .get("cl_ord_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let oid = e
            .get("id")
            .and_then(|v| v.as_i64().map(|i| i.to_string()))
            .unwrap_or_else(|| order_id.to_string());
        Ok(self.order_response(oid, cid, symbol, status, orig, filled, avg, e))
    }

    async fn standx_account(&self) -> anyhow::Result<AccountSnapshot> {
        let headers = {
            let token = self.standx_jwt().await?;
            vec![("authorization".into(), format!("Bearer {token}"))]
        };
        let bal = self
            .get_json_url(&format!("{}/api/query_balance", self.base_url), &headers)
            .await?;
        let b = bal.get("result").cloned().unwrap_or(bal.clone());
        let equity = dec_from_json(b.get("equity"))
            .or_else(|| dec_from_json(b.get("balance")))
            .or_else(|| {
                b.get("result")
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.first())
                    .and_then(|e| dec_from_json(e.get("amount")))
            })
            .ok_or_else(|| anyhow::anyhow!("StandX balance has no equity field"))?;
        let free = dec_from_json(b.get("available"))
            .or_else(|| dec_from_json(b.get("available_balance")))
            .unwrap_or_default();
        let pos = self
            .get_json_url(&format!("{}/api/query_positions", self.base_url), &headers)
            .await?;
        let mut positions = Vec::new();
        let parr = pos
            .get("result")
            .and_then(|v| v.as_array())
            .or_else(|| pos.as_array());
        if let Some(arr) = parr {
            for p in arr {
                let qty = dec_from_json(p.get("qty"))
                    .or_else(|| dec_from_json(p.get("position")))
                    .unwrap_or_default();
                if qty == dec!(0) {
                    continue;
                }
                let sym = p.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
                let side_s = p.get("side").and_then(|s| s.as_str()).unwrap_or("buy");
                let side = if side_s == "sell" || qty < dec!(0) {
                    arbitrageur_core::types::Side::Sell
                } else {
                    arbitrageur_core::types::Side::Buy
                };
                positions.push(position_from(
                    &self.venue_id,
                    sym.to_uppercase(),
                    side,
                    qty,
                    dec_from_json(p.get("entry_price")),
                    dec_from_json(p.get("mark_price")),
                    dec_from_json(p.get("unrealized_pnl")).or_else(|| dec_from_json(p.get("upnl"))),
                    dec_from_json(p.get("liq_price"))
                        .or_else(|| dec_from_json(p.get("liquidation_price"))),
                    dec_from_json(p.get("leverage")),
                ));
            }
        }
        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity,
            free_margin: free,
            maintenance_margin: dec!(0),
            margin_usage_pct: dec!(0),
            balances: vec![arbitrageur_core::position::Balance {
                venue: self.venue_id.clone(),
                currency: "DUSD".to_string(),
                total: equity,
                free,
                locked: (equity - free).max(dec!(0)),
            }],
            positions,
            timestamp: Utc::now(),
        })
    }

    // ------------------------------------------------------------------
    // Ondo Perps order paths
    // ------------------------------------------------------------------

    async fn ondo_place(&self, order: &OrderRequest) -> anyhow::Result<OrderResponse> {
        let vsym = self.venue_symbol(&order.symbol);
        let is_market = order.order_type == arbitrageur_core::types::OrderType::Market;
        let mut body = serde_json::json!({
            "market": vsym,
            "side": if order.side.is_buy() { "buy" } else { "sell" },
            "type": if is_market { "market" } else { "limit" },
            "size": order.quantity.to_string(),
            "clientOrderId": order.client_order_id,
            "reduceOnly": order.reduce_only,
            "postOnly": order.order_type == arbitrageur_core::types::OrderType::PostOnly || order.post_only,
        });
        if !is_market {
            let price = order
                .price
                .ok_or_else(|| anyhow::anyhow!("Ondo limit orders require a price"))?;
            body["price"] = serde_json::json!(price.to_string());
            body["timeInForce"] = serde_json::json!(match order.order_type {
                arbitrageur_core::types::OrderType::ImmediateOrCancel => "IOC",
                _ => "GTC",
            });
        }
        let headers = self.ondo_auth_headers().await?;
        let url = format!("{}/v1/perps/orders", self.base_url);
        let resp = self.post_json_url(&url, &body, &headers).await?;
        let result = resp
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let order_id = result
            .get("orderID")
            .or_else(|| result.get("order_id"))
            .or_else(|| result.get("id"))
            .and_then(|v| {
                v.as_i64()
                    .map(|i| i.to_string())
                    .or_else(|| v.as_str().map(|s| s.to_string()))
            })
            .ok_or_else(|| anyhow::anyhow!("Ondo create_order returned no order id: {resp}"))?;
        Ok(self.order_response(
            order_id,
            order.client_order_id.clone(),
            &order.symbol,
            arbitrageur_core::types::OrderStatus::New,
            order.quantity,
            dec!(0),
            None,
            resp,
        ))
    }

    async fn ondo_cancel(&self, req: &CancelRequest) -> anyhow::Result<()> {
        let oid = req
            .order_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Ondo cancel needs the venue order_id"))?;
        let headers = self.ondo_auth_headers().await?;
        let url = format!("{}/v1/perps/orders/{oid}", self.base_url);
        self.delete_json_url(&url, &headers).await?;
        Ok(())
    }

    async fn ondo_fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        _client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        let headers = self.ondo_auth_headers().await?;
        let url = format!("{}/v1/perps/orders/{order_id}", self.base_url);
        let data = self.get_json_url(&url, &headers).await?;
        let e = data
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        if e.is_null() {
            anyhow::bail!("Ondo order {order_id} not found — refusing to assume a state");
        }
        let orig = dec_from_json(e.get("size")).unwrap_or_default();
        let filled = dec_from_json(e.get("filledSize"))
            .or_else(|| dec_from_json(e.get("filled_size")))
            .unwrap_or_default();
        let status_s = e.get("status").and_then(|s| s.as_str()).unwrap_or("");
        let status = map_order_status(status_s, filled, orig).ok_or_else(|| {
            anyhow::anyhow!(
                "Ondo order {order_id} has unknown status '{status_s}' — refusing to guess"
            )
        })?;
        let avg = dec_from_json(e.get("avgFillPrice"))
            .or_else(|| dec_from_json(e.get("averagePrice")))
            .or_else(|| dec_from_json(e.get("price")));
        let cid = e
            .get("clientOrderId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Ok(self.order_response(
            order_id.to_string(),
            cid,
            symbol,
            status,
            orig,
            filled,
            avg,
            e,
        ))
    }

    async fn ondo_account(&self) -> anyhow::Result<AccountSnapshot> {
        let headers = self.ondo_auth_headers().await?;
        let bal = self
            .get_json_url(&format!("{}/v1/perps/balance", self.base_url), &headers)
            .await?;
        let b = bal.get("result").cloned().unwrap_or(bal.clone());
        let equity = dec_from_json(b.get("equity"))
            .or_else(|| dec_from_json(b.get("balance")))
            .or_else(|| dec_from_json(b.get("totalEquity")))
            .ok_or_else(|| anyhow::anyhow!("Ondo balance has no equity field"))?;
        let free = dec_from_json(b.get("available"))
            .or_else(|| dec_from_json(b.get("availableBalance")))
            .or_else(|| dec_from_json(b.get("free")))
            .unwrap_or_default();
        let pos = self
            .get_json_url(&format!("{}/v1/perps/positions", self.base_url), &headers)
            .await?;
        let mut positions = Vec::new();
        if let Some(arr) = pos
            .get("result")
            .and_then(|v| v.as_array())
            .or_else(|| pos.as_array())
        {
            for p in arr {
                let qty = dec_from_json(p.get("size"))
                    .or_else(|| dec_from_json(p.get("amount")))
                    .unwrap_or_default();
                if qty == dec!(0) {
                    continue;
                }
                let sym = p.get("market").and_then(|s| s.as_str()).unwrap_or("");
                let dir = p
                    .get("direction")
                    .or_else(|| p.get("side"))
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                let side = if dir.eq_ignore_ascii_case("short")
                    || dir.eq_ignore_ascii_case("sell")
                    || qty < dec!(0)
                {
                    arbitrageur_core::types::Side::Sell
                } else {
                    arbitrageur_core::types::Side::Buy
                };
                let canon = sym.strip_suffix(".P").unwrap_or(sym).to_uppercase();
                positions.push(position_from(
                    &self.venue_id,
                    canon,
                    side,
                    qty,
                    dec_from_json(p.get("entryPrice"))
                        .or_else(|| dec_from_json(p.get("entry_price"))),
                    dec_from_json(p.get("markPrice"))
                        .or_else(|| dec_from_json(p.get("mark_price"))),
                    dec_from_json(p.get("unrealizedPnl")).or_else(|| dec_from_json(p.get("upnl"))),
                    dec_from_json(p.get("liquidationPrice")),
                    dec_from_json(p.get("leverage")),
                ));
            }
        }
        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity,
            free_margin: free,
            maintenance_margin: dec!(0),
            margin_usage_pct: dec!(0),
            balances: vec![arbitrageur_core::position::Balance {
                venue: self.venue_id.clone(),
                currency: "USD".to_string(),
                total: equity,
                free,
                locked: (equity - free).max(dec!(0)),
            }],
            positions,
            timestamp: Utc::now(),
        })
    }

    // ------------------------------------------------------------------
    // GRVT order paths (trades.grvt.io, cookie session)
    // ------------------------------------------------------------------

    fn grvt_headers(cookie: &str, account_id: &str) -> Vec<(String, String)> {
        vec![
            ("cookie".into(), cookie.to_string()),
            ("x-grvt-account-id".into(), account_id.to_string()),
        ]
    }

    async fn grvt_place(&self, order: &OrderRequest) -> anyhow::Result<OrderResponse> {
        let (cookie, account_id) = self.grvt_session().await?;
        let vsym = self.venue_symbol(&order.symbol);
        let (asset_id, _meta) = self.grvt_instrument(&vsym).await?;
        let sub_id: u64 = account_id
            .parse()
            .map_err(|_| anyhow::anyhow!("GRVT sub-account id {account_id} is not numeric"))?;
        let is_market = order.order_type == arbitrageur_core::types::OrderType::Market;
        let (tif_code, post_only) = match order.order_type {
            arbitrageur_core::types::OrderType::ImmediateOrCancel => (3u8, false),
            arbitrageur_core::types::OrderType::PostOnly => (2u8, true),
            _ => (1u8, false),
        };
        let price_bound = match order.price {
            Some(p) if !is_market => p,
            _ => self.market_bound_price(order).await?,
        };
        let price_1e9 = dec_to_u256(price_bound, 9)
            .and_then(|v| u64::try_from(v).ok())
            .ok_or_else(|| anyhow::anyhow!("GRVT price out of u64 range"))?;
        let size_1e9 = dec_to_u256(order.quantity, 9)
            .and_then(|v| u64::try_from(v).ok())
            .ok_or_else(|| anyhow::anyhow!("GRVT size out of u64 range"))?;
        let nonce: u32 = (Utc::now().timestamp_millis() as u64 % u32::MAX as u64) as u32;
        let expiration_ns: i64 = (Utc::now() + chrono::Duration::days(30))
            .timestamp_nanos_opt()
            .unwrap_or(i64::MAX);
        let chain_id: u64 = 325;
        let sig_hex = self.grvt_order_sig(
            sub_id,
            is_market,
            tif_code,
            post_only,
            order.reduce_only,
            asset_id,
            size_1e9,
            price_1e9,
            order.side.is_buy(),
            nonce,
            expiration_ns,
            chain_id,
        )?;
        // Split the 65-byte sig into r/s/v for the JSON signature block.
        let sig_bytes = hex::decode(sig_hex.trim_start_matches("0x"))?;
        if sig_bytes.len() != 65 {
            anyhow::bail!("GRVT signature must be 65 bytes");
        }
        let signer_addr = self
            .agent_address_hex()
            .ok_or_else(|| anyhow::anyhow!("GRVT requires a signing key"))?;
        let body = serde_json::json!({
            "order": {
                "sub_account_id": account_id,
                "is_market": is_market,
                "time_in_force": match tif_code { 2 => "GOOD_TILL_TIME", 3 => "IMMEDIATE_OR_CANCEL", _ => "GOOD_TILL_TIME" },
                "post_only": post_only,
                "reduce_only": order.reduce_only,
                "legs": [{
                    "instrument": vsym,
                    "size": order.quantity.to_string(),
                    "limit_price": if is_market { serde_json::Value::Null } else { serde_json::json!(price_bound.to_string()) },
                    "is_buying_asset": order.side.is_buy(),
                }],
                "signature": {
                    "signer": signer_addr,
                    "r": format!("0x{}", hex::encode(&sig_bytes[..32])),
                    "s": format!("0x{}", hex::encode(&sig_bytes[32..64])),
                    "v": sig_bytes[64],
                    "expiration": expiration_ns.to_string(),
                    "nonce": nonce,
                    "chain_id": chain_id.to_string(),
                },
                "metadata": {"client_order_id": order.client_order_id},
            }
        });
        let url = "https://trades.grvt.io/full/v1/create_order".to_string();
        let resp = self
            .post_json_url(&url, &body, &Self::grvt_headers(&cookie, &account_id))
            .await?;
        let result = resp
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let order_id = result
            .get("order_id")
            .or_else(|| result.get("orderID"))
            .and_then(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .or_else(|| v.as_i64().map(|i| i.to_string()))
            })
            .ok_or_else(|| anyhow::anyhow!("GRVT create_order returned no order_id: {resp}"))?;
        Ok(self.order_response(
            order_id,
            order.client_order_id.clone(),
            &order.symbol,
            arbitrageur_core::types::OrderStatus::New,
            order.quantity,
            dec!(0),
            None,
            resp,
        ))
    }

    async fn grvt_cancel(&self, req: &CancelRequest) -> anyhow::Result<()> {
        let (cookie, account_id) = self.grvt_session().await?;
        let oid = req
            .order_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("GRVT cancel needs the venue order_id"))?;
        let body = serde_json::json!({"sub_account_id": account_id, "order_id": oid});
        let url = "https://trades.grvt.io/full/v1/cancel_order".to_string();
        self.post_json_url(&url, &body, &Self::grvt_headers(&cookie, &account_id))
            .await?;
        Ok(())
    }

    async fn grvt_fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        _client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        let (cookie, account_id) = self.grvt_session().await?;
        let body = serde_json::json!({"sub_account_id": account_id, "order_id": order_id});
        let url = "https://trades.grvt.io/full/v1/order".to_string();
        let data = self
            .post_json_url(&url, &body, &Self::grvt_headers(&cookie, &account_id))
            .await?;
        let e = data
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        if e.is_null() {
            anyhow::bail!("GRVT order {order_id} not found — refusing to assume a state");
        }
        let legs = e.get("legs").and_then(|v| v.as_array());
        let orig = legs
            .and_then(|a| a.first())
            .and_then(|l| dec_from_json(l.get("size")))
            .unwrap_or_default();
        let state = e.get("state").cloned().unwrap_or(serde_json::Value::Null);
        let filled = dec_from_json(state.get("traded_size"))
            .or_else(|| {
                legs.and_then(|a| a.first())
                    .and_then(|l| dec_from_json(l.get("filled_size")))
            })
            .unwrap_or_default();
        let status_s = state.get("status").and_then(|s| s.as_str()).unwrap_or("");
        let status = map_order_status(status_s, filled, orig).ok_or_else(|| {
            anyhow::anyhow!(
                "GRVT order {order_id} has unknown status '{status_s}' — refusing to guess"
            )
        })?;
        let avg =
            dec_from_json(state.get("avg_price")).or_else(|| dec_from_json(e.get("avg_price")));
        Ok(self.order_response(
            order_id.to_string(),
            String::new(),
            symbol,
            status,
            orig,
            filled,
            avg,
            e,
        ))
    }

    async fn grvt_account(&self) -> anyhow::Result<AccountSnapshot> {
        let (cookie, account_id) = self.grvt_session().await?;
        let headers = Self::grvt_headers(&cookie, &account_id);
        let summary = self
            .post_json_url(
                "https://trades.grvt.io/full/v1/sub_account_summary",
                &serde_json::json!({"sub_account_id": account_id}),
                &headers,
            )
            .await?;
        let s = summary
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let equity = dec_from_json(s.get("total_equity"))
            .or_else(|| dec_from_json(s.get("equity")))
            .ok_or_else(|| anyhow::anyhow!("GRVT sub-account summary has no equity"))?;
        let free = dec_from_json(s.get("available_balance"))
            .or_else(|| dec_from_json(s.get("free")))
            .unwrap_or_default();
        let maint = dec_from_json(s.get("maintenance_margin")).unwrap_or_default();
        let pos = self
            .post_json_url(
                "https://trades.grvt.io/full/v1/positions",
                &serde_json::json!({"sub_account_id": account_id}),
                &headers,
            )
            .await?;
        let mut positions = Vec::new();
        if let Some(arr) = pos
            .get("result")
            .and_then(|v| v.as_array())
            .or_else(|| pos.as_array())
        {
            for p in arr {
                let qty = dec_from_json(p.get("size")).unwrap_or_default();
                if qty == dec!(0) {
                    continue;
                }
                let sym = p.get("instrument").and_then(|s| s.as_str()).unwrap_or("");
                let side = if qty < dec!(0) {
                    arbitrageur_core::types::Side::Sell
                } else {
                    arbitrageur_core::types::Side::Buy
                };
                positions.push(position_from(
                    &self.venue_id,
                    sym.to_uppercase(),
                    side,
                    qty,
                    dec_from_json(p.get("entry_price")),
                    dec_from_json(p.get("mark_price")),
                    dec_from_json(p.get("unrealized_pnl")).or_else(|| dec_from_json(p.get("upnl"))),
                    dec_from_json(p.get("liquidation_price")),
                    dec_from_json(p.get("leverage")),
                ));
            }
        }
        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity,
            free_margin: free,
            maintenance_margin: maint,
            margin_usage_pct: if equity > dec!(0) {
                maint / equity * dec!(100)
            } else {
                dec!(0)
            },
            balances: vec![arbitrageur_core::position::Balance {
                venue: self.venue_id.clone(),
                currency: "USDT".to_string(),
                total: equity,
                free,
                locked: (equity - free).max(dec!(0)),
            }],
            positions,
            timestamp: Utc::now(),
        })
    }

    // ------------------------------------------------------------------
    // Derive order paths
    // ------------------------------------------------------------------

    async fn derive_place(&self, order: &OrderRequest) -> anyhow::Result<OrderResponse> {
        let vsym = self.venue_symbol(&order.symbol);
        let sub_id_s = self.creds.read().account_id.clone().ok_or_else(|| {
            anyhow::anyhow!("Derive requires the subaccount id (account_id field)")
        })?;
        let sub_id: u64 = sub_id_s
            .parse()
            .map_err(|_| anyhow::anyhow!("Derive subaccount id {sub_id_s} is not numeric"))?;
        let (asset, asset_sub) = self.derive_instrument(&vsym).await?;
        let is_market = order.order_type == arbitrageur_core::types::OrderType::Market;
        let price = match order.price {
            Some(p) if !is_market => p,
            _ => self.market_bound_price(order).await?,
        };
        let qty = order.quantity;
        let limit_i =
            dec_to_i256(price, 18).ok_or_else(|| anyhow::anyhow!("Derive price out of range"))?;
        let amount_i = dec_to_i256(qty * order.side.sign(), 18)
            .ok_or_else(|| anyhow::anyhow!("Derive amount out of range"))?;
        // max_fee: conservative bound — 1% of notional, venue rejects
        // lower. Fee is charged per contract; this over-caps safely.
        let max_fee = dec_to_u256((price * qty * dec!(0.01)).max(dec!(1)), 18)
            .ok_or_else(|| anyhow::anyhow!("Derive max_fee out of range"))?;
        let nonce =
            U256::from(Utc::now().timestamp_millis() as u64 * 1000 + (nanoid_nonce() % 1000));
        let expiry = U256::from((Utc::now() + chrono::Duration::minutes(30)).timestamp() as u64);
        let sub_u = U256::from(sub_id);
        let sig = self.derive_order_sig(
            asset,
            asset_sub,
            limit_i,
            amount_i,
            max_fee,
            sub_u,
            order.side.is_buy(),
            sub_u,
            nonce,
            expiry,
        )?;
        let signer_addr = self
            .agent_address_hex()
            .ok_or_else(|| anyhow::anyhow!("Derive requires a session key"))?;
        let body = serde_json::json!({
            "instrument_name": vsym,
            "subaccount_id": sub_id,
            "direction": if order.side.is_buy() { "buy" } else { "sell" },
            "limit_price": price.to_string(),
            "amount": qty.to_string(),
            "signature_expiry_sec": expiry.to_string().parse::<u64>().unwrap_or(0),
            "max_fee": (price * qty * dec!(0.01)).max(dec!(1)).to_string(),
            "nonce": nonce.to_string().parse::<u64>().unwrap_or(0),
            "signer": signer_addr,
            "order_type": if is_market { "market" } else { "limit" },
            "time_in_force": match order.order_type {
                arbitrageur_core::types::OrderType::ImmediateOrCancel => "ioc",
                arbitrageur_core::types::OrderType::PostOnly => "post_only",
                _ => "gtc",
            },
            "mmp": false,
            "reduce_only": order.reduce_only,
            "signature": sig,
        });
        let headers = self.derive_headers()?;
        let url = format!("{}/private/order", self.base_url);
        let resp = self.post_json_url(&url, &body, &headers).await?;
        let result = resp
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let order_id = result
            .get("order")
            .and_then(|o| o.get("order_id"))
            .or_else(|| result.get("order_id"))
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .ok_or_else(|| anyhow::anyhow!("Derive order returned no order_id: {resp}"))?;
        Ok(self.order_response(
            order_id,
            order.client_order_id.clone(),
            &order.symbol,
            arbitrageur_core::types::OrderStatus::New,
            order.quantity,
            dec!(0),
            None,
            resp,
        ))
    }

    async fn derive_cancel(&self, req: &CancelRequest) -> anyhow::Result<()> {
        let sub_id: u64 = self
            .creds
            .read()
            .account_id
            .clone()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| {
                anyhow::anyhow!("Derive requires the subaccount id (account_id field)")
            })?;
        let oid = req
            .order_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Derive cancel needs the venue order_id"))?;
        let vsym = self.venue_symbol(&req.symbol);
        let body = serde_json::json!({
            "instrument_name": vsym,
            "order_id": oid,
            "subaccount_id": sub_id,
        });
        let headers = self.derive_headers()?;
        let url = format!("{}/private/cancel", self.base_url);
        self.post_json_url(&url, &body, &headers).await?;
        Ok(())
    }

    async fn derive_fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        _client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        let sub_id: u64 = self
            .creds
            .read()
            .account_id
            .clone()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| {
                anyhow::anyhow!("Derive requires the subaccount id (account_id field)")
            })?;
        let headers = self.derive_headers()?;
        let url = format!(
            "{}/private/get_order?order_id={order_id}&subaccount_id={sub_id}",
            self.base_url
        );
        let data = self.get_json_url(&url, &headers).await?;
        let e = data
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        if e.is_null() || e.get("order_id").is_none() {
            anyhow::bail!("Derive order {order_id} not found — refusing to assume a state");
        }
        let orig = dec_from_json(e.get("amount")).unwrap_or_default();
        let filled = dec_from_json(e.get("filled_amount")).unwrap_or_default();
        let status_s = e
            .get("order_status")
            .or_else(|| e.get("status"))
            .and_then(|s| s.as_str())
            .unwrap_or("");
        let status = map_order_status(status_s, filled, orig).ok_or_else(|| {
            anyhow::anyhow!(
                "Derive order {order_id} has unknown status '{status_s}' — refusing to guess"
            )
        })?;
        let avg = dec_from_json(e.get("average_price"))
            .or_else(|| dec_from_json(e.get("avg_fill_price")));
        Ok(self.order_response(
            order_id.to_string(),
            String::new(),
            symbol,
            status,
            orig,
            filled,
            avg,
            e,
        ))
    }

    async fn derive_account(&self) -> anyhow::Result<AccountSnapshot> {
        let sub_id: u64 = self
            .creds
            .read()
            .account_id
            .clone()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| {
                anyhow::anyhow!("Derive requires the subaccount id (account_id field)")
            })?;
        let headers = self.derive_headers()?;
        let url = format!(
            "{}/private/get_subaccount?subaccount_id={sub_id}",
            self.base_url
        );
        let data = self.get_json_url(&url, &headers).await?;
        let r = data
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let equity = dec_from_json(
            r.get("subaccount_value_stats")
                .and_then(|s| s.get("total_equity")),
        )
        .or_else(|| dec_from_json(r.get("total_equity")))
        .ok_or_else(|| anyhow::anyhow!("Derive subaccount has no equity field"))?;
        let free = r
            .get("collaterals")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|e| dec_from_json(e.get("amount")))
            .unwrap_or_default();
        let maint = dec_from_json(
            r.get("subaccount_value_stats")
                .and_then(|s| s.get("maintenance_margin")),
        )
        .unwrap_or_default();
        let mut positions = Vec::new();
        if let Some(arr) = r.get("positions").and_then(|v| v.as_array()) {
            for p in arr {
                let qty = dec_from_json(p.get("amount")).unwrap_or_default();
                if qty == dec!(0) {
                    continue;
                }
                let sym = p
                    .get("instrument_name")
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                let side = if qty < dec!(0) {
                    arbitrageur_core::types::Side::Sell
                } else {
                    arbitrageur_core::types::Side::Buy
                };
                positions.push(position_from(
                    &self.venue_id,
                    sym.to_uppercase(),
                    side,
                    qty,
                    dec_from_json(p.get("average_price")),
                    dec_from_json(p.get("mark_price")),
                    dec_from_json(p.get("unrealized_pnl")),
                    dec_from_json(p.get("liquidation_price")),
                    dec_from_json(p.get("leverage")),
                ));
            }
        }
        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity,
            free_margin: free,
            maintenance_margin: maint,
            margin_usage_pct: if equity > dec!(0) {
                maint / equity * dec!(100)
            } else {
                dec!(0)
            },
            balances: vec![arbitrageur_core::position::Balance {
                venue: self.venue_id.clone(),
                currency: "USDC".to_string(),
                total: equity,
                free,
                locked: (equity - free).max(dec!(0)),
            }],
            positions,
            timestamp: Utc::now(),
        })
    }

    // ------------------------------------------------------------------
    // Aevo order paths
    // ------------------------------------------------------------------

    async fn aevo_place(&self, order: &OrderRequest) -> anyhow::Result<OrderResponse> {
        let vsym = self.venue_symbol(&order.symbol);
        let instrument = self.aevo_instrument_id(&vsym).await?;
        let is_market = order.order_type == arbitrageur_core::types::OrderType::Market;
        let price = match order.price {
            Some(p) if !is_market => p,
            _ => self.market_bound_price(order).await?,
        };
        // Aevo fixed-point is 6 decimals for both price and amount.
        let amount_6 = dec_to_u256(order.quantity, 6)
            .ok_or_else(|| anyhow::anyhow!("Aevo amount out of range"))?;
        let price_6 =
            dec_to_u256(price, 6).ok_or_else(|| anyhow::anyhow!("Aevo price out of range"))?;
        let salt = U256::from(Utc::now().timestamp_millis() as u64 % u64::MAX);
        let ts = U256::from(Utc::now().timestamp() as u64);
        let (sig, maker) = self.aevo_order_sig(
            order.side.is_buy(),
            price_6,
            amount_6,
            salt,
            U256::from(instrument),
            ts,
        )?;
        let body = serde_json::json!({
            "maker": maker,
            "instrument": instrument,
            "is_buy": order.side.is_buy(),
            "amount": amount_6.to_string(),
            "limit_price": price_6.to_string(),
            "salt": salt.to_string(),
            "signature": sig,
            "post_only": order.order_type == arbitrageur_core::types::OrderType::PostOnly || order.post_only,
            "timestamp": ts.to_string().parse::<u64>().unwrap_or(0),
        });
        let headers = self.aevo_headers()?;
        let url = format!("{}/orders", self.base_url);
        let resp = self.post_json_url(&url, &body, &headers).await?;
        let order_id = resp
            .get("order_id")
            .or_else(|| resp.get("orderId"))
            .and_then(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .or_else(|| v.as_i64().map(|i| i.to_string()))
            })
            .ok_or_else(|| anyhow::anyhow!("Aevo create_order returned no order_id: {resp}"))?;
        Ok(self.order_response(
            order_id,
            order.client_order_id.clone(),
            &order.symbol,
            arbitrageur_core::types::OrderStatus::New,
            order.quantity,
            dec!(0),
            None,
            resp,
        ))
    }

    async fn aevo_cancel(&self, req: &CancelRequest) -> anyhow::Result<()> {
        let oid = req
            .order_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Aevo cancel needs the venue order_id"))?;
        let headers = self.aevo_headers()?;
        let url = format!("{}/orders/{oid}", self.base_url);
        self.delete_json_url(&url, &headers).await?;
        Ok(())
    }

    async fn aevo_fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        _client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        let headers = self.aevo_headers()?;
        let url = format!("{}/orders/{order_id}", self.base_url);
        let data = self.get_json_url(&url, &headers).await?;
        let e = data.get("result").cloned().unwrap_or(data.clone());
        if e.get("order_id").is_none() && e.get("orderId").is_none() {
            anyhow::bail!("Aevo order {order_id} not found — refusing to assume a state");
        }
        // Aevo amounts/prices are 6-decimal fixed-point.
        let scale = dec!(1000000);
        let orig = dec_from_json(e.get("amount"))
            .map(|d| d / scale)
            .unwrap_or_default();
        let filled = dec_from_json(e.get("filled_amount"))
            .map(|d| d / scale)
            .unwrap_or_default();
        let status_s = e.get("status").and_then(|s| s.as_str()).unwrap_or("");
        let status = map_order_status(status_s, filled, orig).ok_or_else(|| {
            anyhow::anyhow!(
                "Aevo order {order_id} has unknown status '{status_s}' — refusing to guess"
            )
        })?;
        let avg = dec_from_json(e.get("avg_price"))
            .or_else(|| dec_from_json(e.get("average_price")))
            .map(|d| d / scale);
        Ok(self.order_response(
            order_id.to_string(),
            String::new(),
            symbol,
            status,
            orig,
            filled,
            avg,
            e,
        ))
    }

    async fn aevo_account(&self) -> anyhow::Result<AccountSnapshot> {
        let headers = self.aevo_headers()?;
        let bal = self
            .get_json_url(&format!("{}/account", self.base_url), &headers)
            .await?;
        let b = bal.get("result").cloned().unwrap_or(bal.clone());
        let equity = dec_from_json(b.get("equity"))
            .or_else(|| dec_from_json(b.get("balance")))
            .ok_or_else(|| anyhow::anyhow!("Aevo account has no equity field"))?;
        let free = dec_from_json(b.get("available"))
            .or_else(|| dec_from_json(b.get("free")))
            .unwrap_or_default();
        let port = self
            .get_json_url(&format!("{}/portfolio", self.base_url), &headers)
            .await?;
        let mut positions = Vec::new();
        let scale = dec!(1000000);
        if let Some(arr) = port
            .get("positions")
            .and_then(|v| v.as_array())
            .or_else(|| {
                port.get("result")
                    .and_then(|r| r.get("positions"))
                    .and_then(|v| v.as_array())
            })
        {
            for p in arr {
                let qty = dec_from_json(p.get("amount"))
                    .map(|d| d / scale)
                    .unwrap_or_default();
                if qty == dec!(0) {
                    continue;
                }
                let sym = p
                    .get("instrument_name")
                    .or_else(|| p.get("asset"))
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                let side = if qty < dec!(0) {
                    arbitrageur_core::types::Side::Sell
                } else {
                    arbitrageur_core::types::Side::Buy
                };
                positions.push(position_from(
                    &self.venue_id,
                    sym.to_uppercase(),
                    side,
                    qty,
                    dec_from_json(p.get("entry_price")).map(|d| d / scale),
                    dec_from_json(p.get("mark_price")).map(|d| d / scale),
                    dec_from_json(p.get("unrealized_pnl")),
                    dec_from_json(p.get("liquidation_price")).map(|d| d / scale),
                    dec_from_json(p.get("leverage")),
                ));
            }
        }
        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity,
            free_margin: free,
            maintenance_margin: dec!(0),
            margin_usage_pct: dec!(0),
            balances: vec![arbitrageur_core::position::Balance {
                venue: self.venue_id.clone(),
                currency: "USDC".to_string(),
                total: equity,
                free,
                locked: (equity - free).max(dec!(0)),
            }],
            positions,
            timestamp: Utc::now(),
        })
    }
}

/// Pseudo-random 3-digit nonce suffix for Derive order nonces — derived
/// from the timestamp (sufficient for uniqueness; not cryptographic).
fn nanoid_nonce() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0)
        % 1000
}

// ---------------------------------------------------------------------------
// ExchangeAdapter implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl ExchangeAdapter for DexPerpAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn ping(&self) -> anyhow::Result<()> {
        // Bypass the response cache so the round-trip is a real probe.
        let spec = spec_for(self.spec_id)
            .ok_or_else(|| anyhow::anyhow!("unknown venue {}", self.spec_id))?;
        let url = format!("{}{}", self.base_url, spec.markets_path);
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("{} ping failed: {}", self.name, resp.status());
        }
        Ok(())
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let spec = spec_for(self.spec_id)
            .ok_or_else(|| anyhow::anyhow!("unknown venue {}", self.spec_id))?;
        if self.spec_id == "derive" {
            let mut out = Vec::new();
            for cur in DERIVE_CURRENCIES {
                // One currency failing must not abort the listing — the
                // error is skipped per currency, fail-closed.
                if let Ok(data) = self
                    .get_json(&format!(
                        "/public/get_instruments?currency={cur}&expired=false&instrument_type=perp"
                    ))
                    .await
                {
                    out.extend(parse_derive_markets(
                        &self.venue_id,
                        &data,
                        spec.taker_fee_bps,
                    ));
                }
            }
            return Ok(out);
        }
        // GRVT's instrument list is a POST on the market-data host.
        if self.spec_id == "grvt" {
            let url = format!("{}{}", self.base_url, spec.markets_path);
            let data = self
                .post_json_url(&url, &serde_json::json!({}), &[])
                .await?;
            return Ok(parse_grvt_markets(
                &self.venue_id,
                &data,
                spec.taker_fee_bps,
            ));
        }
        let data = self.get_json(spec.markets_path).await?;
        Ok(parse_markets_for(
            self.spec_id,
            &self.venue_id,
            &data,
            spec.taker_fee_bps,
        ))
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let spec = spec_for(self.spec_id)
            .ok_or_else(|| anyhow::anyhow!("unknown venue {}", self.spec_id))?;
        if spec.book_kind == BookKind::PricesOnly {
            anyhow::bail!(
                "{} is an AMM venue with no orderbook by construction (GLV pricing); use fetch_ticker for the index price",
                self.name
            );
        }
        // Derive exposes no depth endpoint — the book is the ticker's real
        // top-of-book level per side (documented, never padded).
        if self.spec_id == "derive" {
            let vsym = self.venue_symbol(symbol);
            let data = self
                .get_json(&format!("/public/get_ticker?instrument_name={vsym}"))
                .await?;
            let result = data
                .get("result")
                .ok_or_else(|| anyhow::anyhow!("Derive ticker for {symbol} has no result"))?;
            return parse_derive_topbook(&self.venue_id, symbol, result);
        }
        // Helix books resolve through the LCD market list (market_id) with
        // per-market quote-decimal scaling.
        if self.spec_id == "helix" {
            let base = to_base(symbol);
            let info = self.helix_resolve(&base).await?;
            let data = self
                .get_json(&format!(
                    "/injective/exchange/v1beta1/derivative/orderbook/{}",
                    info.market_id
                ))
                .await?;
            return parse_helix_book(&self.venue_id, symbol, &data, info.scale);
        }
        // QFEX books (and tickers) are auth-gated: fail closed with an
        // explanatory error rather than a fabricated quote.
        if self.spec_id == "qfex" {
            anyhow::bail!(
                "QFEX orderbooks require authentication; {} lists markets from /refdata only",
                self.name
            );
        }
        // edgeX serves the book only over WebSocket — its public REST
        // has no depth endpoint, so fail closed rather than fabricate.
        if self.spec_id == "edgex" {
            anyhow::bail!(
                "edgeX orderbooks are served over the venue WebSocket only; {} lists markets from getMetaData",
                self.name
            );
        }
        // GRVT book: `POST /full/v1/book {instrument, depth}` on the
        // market-data host — `{result: {bids: [{price, size}], asks}}`.
        if self.spec_id == "grvt" {
            let vsym = self.venue_symbol(symbol);
            let url = format!("{}/full/v1/book", self.base_url);
            let data = self
                .post_json_url(
                    &url,
                    &serde_json::json!({"instrument": vsym, "depth": 15}),
                    &[],
                )
                .await?;
            return parse_grvt_book(&self.venue_id, symbol, &data);
        }
        let vsym = self.venue_symbol(symbol);
        let data = self.get_json(&self.book_path(&vsym)).await?;
        match self.spec_id {
            "dydx" => parse_dydx_book(&self.venue_id, symbol, &data),
            "apex" => parse_apex_book(&self.venue_id, symbol, &data),
            "extended" => parse_extended_book(&self.venue_id, symbol, &data),
            "pacifica" => parse_pacifica_book(&self.venue_id, symbol, &data),
            _ => parse_pair_book(&self.venue_id, symbol, &data),
        }
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        // Prices-only venues (GMX V2) resolve the ticker from the price
        // feed; every other venue derives it from the live book so the
        // bid/ask is always a real quote, never a synthesized mid.
        if self.spec_id == "gmx" {
            let base = to_base(symbol);
            let data = self.get_json("/prices/tickers").await?;
            let arr = data
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("GMX tickers missing array"))?;
            let mut best: Option<Decimal> = None;
            for e in arr {
                if e.get("tokenSymbol").and_then(|s| s.as_str()) != Some(base.as_str()) {
                    continue;
                }
                // GMX feed prices are 30-decimal fixed-point USD values;
                // normalize min/max into a best-estimate mid.
                let lo = dec_from_json(e.get("minPrice"));
                let hi = dec_from_json(e.get("maxPrice"));
                if let (Some(l), Some(h)) = (lo, hi) {
                    let scale =
                        Decimal::from_str("1000000000000000000000000000000").unwrap_or(dec!(1));
                    if scale > dec!(0) {
                        best = Some((l + h) / (dec!(2) * scale));
                        break;
                    }
                }
            }
            let px = best.ok_or_else(|| anyhow::anyhow!("GMX has no price for {symbol}"))?;
            return Ok(Ticker {
                venue: self.venue_id.clone(),
                symbol: symbol.to_string(),
                bid: px,
                ask: px,
                last: px,
                timestamp: Utc::now(),
            });
        }
        let book = self.fetch_orderbook(symbol).await?;
        let bid = book
            .best_bid_price()
            .ok_or_else(|| anyhow::anyhow!("{} book for {symbol} has no bid", self.name))?;
        let ask = book
            .best_ask_price()
            .ok_or_else(|| anyhow::anyhow!("{} book for {symbol} has no ask", self.name))?;
        Ok(Ticker {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            bid,
            ask,
            last: (bid + ask) / dec!(2),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate> {
        // dYdX v4 carries the predicted hourly rate inside the markets
        // object — no per-symbol funding endpoint exists.
        if self.spec_id == "dydx" {
            let data = self.get_json("/v4/perpetualMarkets").await?;
            let vsym = self.venue_symbol(symbol);
            let rate = dydx_funding_from_markets(&vsym, &data)
                .ok_or_else(|| anyhow::anyhow!("dYdX has no funding rate for {symbol}"))?;
            return Ok(funding_now(&self.venue_id, symbol, rate, 1, None));
        }
        // Paradex publishes funding per market object.
        if self.spec_id == "paradex" {
            let data = self.get_json("/v1/markets").await?;
            let vsym = self.venue_symbol(symbol);
            let arr = data
                .get("results")
                .and_then(|r| r.as_array())
                .ok_or_else(|| anyhow::anyhow!("Paradex markets missing results"))?;
            for e in arr {
                if e.get("symbol").and_then(|s| s.as_str()) == Some(vsym.as_str()) {
                    // `funding_rate` is fixtures-dependent; absence fails
                    // closed for this symbol only (never a zero fill-in).
                    let rate = dec_from_json(e.get("funding_rate"))
                        .or_else(|| dec_from_json(e.get("max_funding_rate")))
                        .ok_or_else(|| {
                            anyhow::anyhow!("Paradex has no funding rate for {symbol}")
                        })?;
                    return Ok(funding_now(&self.venue_id, symbol, rate, 8, None));
                }
            }
            anyhow::bail!("Paradex has no market entry for {symbol}");
        }
        // Aevo exposes per-instrument funding.
        if self.spec_id == "aevo" {
            let vsym = self.venue_symbol(symbol);
            let data = self
                .get_json(&format!("/funding?instrument_name={vsym}"))
                .await?;
            let rate = dec_from_json(data.get("funding_rate"))
                .or_else(|| dec_from_json(data.get("rate")))
                .ok_or_else(|| anyhow::anyhow!("Aevo has no funding rate for {symbol}"))?;
            return Ok(funding_now(&self.venue_id, symbol, rate, 1, None));
        }
        // ApeX Omni tickers carry the current funding rate plus the next
        // funding timestamp (hourly grid, inferred from next-funding times).
        if self.spec_id == "apex" {
            let vsym = self.venue_symbol(symbol);
            let data = self
                .get_json(&format!("/api/v3/ticker?symbol={vsym}"))
                .await?;
            let entry = data
                .get("data")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .ok_or_else(|| anyhow::anyhow!("ApeX has no ticker for {symbol}"))?;
            let rate = dec_from_json(entry.get("fundingRate"))
                .ok_or_else(|| anyhow::anyhow!("ApeX has no funding rate for {symbol}"))?;
            return Ok(funding_now(
                &self.venue_id,
                symbol,
                rate,
                1,
                parse_time_opt(entry.get("nextFundingTime")),
            ));
        }
        // Extended funds hourly (inferred from next-funding timestamps);
        // the rate and next time ride inside the markets payload.
        if self.spec_id == "extended" {
            let vsym = self.venue_symbol(symbol);
            let data = self.get_json("/api/v1/info/markets").await?;
            let arr = data
                .get("data")
                .and_then(|v| v.as_array())
                .ok_or_else(|| anyhow::anyhow!("Extended markets missing data"))?;
            for e in arr {
                if e.get("name").and_then(|s| s.as_str()) == Some(vsym.as_str()) {
                    let stats = e
                        .get("marketStats")
                        .ok_or_else(|| anyhow::anyhow!("Extended has no stats for {symbol}"))?;
                    let rate = dec_from_json(stats.get("fundingRate")).ok_or_else(|| {
                        anyhow::anyhow!("Extended has no funding rate for {symbol}")
                    })?;
                    return Ok(funding_now(
                        &self.venue_id,
                        symbol,
                        rate,
                        1,
                        parse_millis_opt(stats.get("nextFundingRate")),
                    ));
                }
            }
            anyhow::bail!("Extended has no market entry for {symbol}");
        }
        // Derive/Lyra funds hourly (max_rate_per_hour); the live rate is
        // `perp_details.funding_rate` on the ticker.
        if self.spec_id == "derive" {
            let vsym = self.venue_symbol(symbol);
            let data = self
                .get_json(&format!("/public/get_ticker?instrument_name={vsym}"))
                .await?;
            let result = data
                .get("result")
                .ok_or_else(|| anyhow::anyhow!("Derive ticker for {symbol} has no result"))?;
            let rate = result
                .get("perp_details")
                .and_then(|d| dec_from_json(d.get("funding_rate")))
                .ok_or_else(|| anyhow::anyhow!("Derive has no funding rate for {symbol}"))?;
            return Ok(funding_now(&self.venue_id, symbol, rate, 1, None));
        }
        // StandX funds hourly; the rate and next time ride on the market
        // snapshot endpoint.
        if self.spec_id == "standx" {
            let vsym = self.venue_symbol(symbol);
            let data = self
                .get_json(&format!("/api/query_symbol_market?symbol={vsym}"))
                .await?;
            let rate = dec_from_json(data.get("funding_rate"))
                .ok_or_else(|| anyhow::anyhow!("StandX has no funding rate for {symbol}"))?;
            return Ok(funding_now(
                &self.venue_id,
                symbol,
                rate,
                1,
                parse_time_opt(data.get("next_funding_time")),
            ));
        }
        // Pacifica publishes current + next funding per symbol on
        // `/api/v1/info/prices`; the predicted rate is surfaced so the
        // funding bot can see the turn, not just the level.
        if self.spec_id == "pacifica" {
            let base = to_base(symbol);
            let data = self.get_json("/api/v1/info/prices").await?;
            let arr = data
                .get("data")
                .and_then(|v| v.as_array())
                .ok_or_else(|| anyhow::anyhow!("Pacifica prices missing data"))?;
            for e in arr {
                if e.get("symbol")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_uppercase())
                    == Some(base.clone())
                {
                    let rate = dec_from_json(e.get("funding")).ok_or_else(|| {
                        anyhow::anyhow!("Pacifica has no funding rate for {symbol}")
                    })?;
                    let mut fr = funding_now(&self.venue_id, symbol, rate, 1, None);
                    fr.predicted_rate = dec_from_json(e.get("next_funding"));
                    return Ok(fr);
                }
            }
            anyhow::bail!("Pacifica has no price entry for {symbol}");
        }
        // GRVT: `POST /full/v1/funding {instrument}` → `{result: [{
        // instrument, funding_rate (PERCENT), funding_interval_hours,
        // next_funding_time}]}` — the rate is a percent, converted to a
        // fraction here.
        if self.spec_id == "grvt" {
            let vsym = self.venue_symbol(symbol);
            let url = format!("{}/full/v1/funding", self.base_url);
            let data = self
                .post_json_url(&url, &serde_json::json!({"instrument": vsym}), &[])
                .await?;
            let arr = data
                .get("result")
                .and_then(|v| v.as_array())
                .ok_or_else(|| anyhow::anyhow!("GRVT funding for {symbol} has no result"))?;
            for e in arr {
                if e.get("instrument").and_then(|s| s.as_str()) == Some(vsym.as_str()) {
                    let pct = dec_from_json(e.get("funding_rate"))
                        .ok_or_else(|| anyhow::anyhow!("GRVT has no funding rate for {symbol}"))?;
                    let hours = e
                        .get("funding_interval_hours")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(8)
                        .min(u32::MAX as u64) as u32;
                    return Ok(funding_now(
                        &self.venue_id,
                        symbol,
                        pct / dec!(100),
                        hours.max(1),
                        parse_time_opt(e.get("next_funding_time")),
                    ));
                }
            }
            anyhow::bail!("GRVT has no funding entry for {symbol}");
        }
        // Ondo Perps: `GET /v1/perps/funding_rates?market=` →
        // `{result: {market, rate, intervalEnds}}`. Funding settles
        // `24 / fundingIntervalDivisions` hours (8 → 3h) — the divisor
        // comes from the market listing, not a guess.
        if self.spec_id == "ondoperps" {
            let vsym = self.venue_symbol(symbol);
            let data = self
                .get_json(&format!("/v1/perps/funding_rates?market={vsym}"))
                .await?;
            let result = data
                .get("result")
                .ok_or_else(|| anyhow::anyhow!("Ondo funding for {symbol} has no result"))?;
            let rate = dec_from_json(result.get("rate"))
                .ok_or_else(|| anyhow::anyhow!("Ondo has no funding rate for {symbol}"))?;
            let divisions = self
                .ondo_market_field(symbol, "fundingIntervalDivisions")
                .await
                .and_then(|v| v.as_u64())
                .unwrap_or(8);
            let hours = (24 / divisions.max(1)).max(1) as u32;
            return Ok(funding_now(
                &self.venue_id,
                symbol,
                rate,
                hours,
                parse_time_opt(result.get("intervalEnds")),
            ));
        }
        // Helix funding is NOT on the chain LCD (query returns
        // "Not Implemented") and QFEX refdata carries no funding — fail
        // closed rather than returning a zero rate that the funding bot
        // would read as "no cost to hold".
        // Every other venue: no verified per-symbol funding endpoint in this
        // harness — fail closed rather than returning a zero rate that the
        // funding bot would read as "no cost to hold".
        anyhow::bail!(
            "Live funding fetch is not verified for {}; refusing to fabricate a rate",
            self.name
        )
    }

    async fn fetch_volumes_24h(&self) -> anyhow::Result<HashMap<String, Decimal>> {
        // Best-effort volume map: dYdX (`volume24H` per ticker) and Aevo
        // (`volume_24h` per instrument) shapes are verified; anything else
        // returns an empty map (contributes nothing to aggregates).
        let mut out = HashMap::new();
        if self.spec_id == "dydx" {
            let data = self.get_json("/v4/perpetualMarkets").await?;
            if let Some(m) = data.get("markets").and_then(|v| v.as_object()) {
                for (ticker, e) in m {
                    if let Some(v) = dec_from_json(e.get("volume24H")) {
                        if let Some((base, _)) = ticker.split_once('-') {
                            out.insert(base.to_string(), v);
                        }
                    }
                }
            }
            return Ok(out);
        }
        // Extended: `marketStats.dailyVolume` keyed by asset name.
        if self.spec_id == "extended" {
            let data = self.get_json("/api/v1/info/markets").await?;
            if let Some(arr) = data.get("data").and_then(|v| v.as_array()) {
                for e in arr {
                    let base = e.get("assetName").and_then(|s| s.as_str()).unwrap_or("");
                    let vol = e
                        .get("marketStats")
                        .and_then(|s| dec_from_json(s.get("dailyVolume")));
                    if let (Some(v), true) = (vol, !base.is_empty()) {
                        out.insert(base.to_uppercase(), v);
                    }
                }
            }
            return Ok(out);
        }
        // Pacifica: `volume_24h` per symbol on the prices endpoint.
        if self.spec_id == "pacifica" {
            let data = self.get_json("/api/v1/info/prices").await?;
            if let Some(arr) = data.get("data").and_then(|v| v.as_array()) {
                for e in arr {
                    if let (Some(sym), Some(v)) = (
                        e.get("symbol").and_then(|s| s.as_str()),
                        dec_from_json(e.get("volume_24h")),
                    ) {
                        out.insert(sym.to_uppercase(), v);
                    }
                }
            }
            return Ok(out);
        }
        Ok(out)
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        match self.spec_id {
            "pacifica" => self.pacifica_account().await,
            "standx" => self.standx_account().await,
            "ondoperps" => self.ondo_account().await,
            "grvt" => self.grvt_account().await,
            "derive" => self.derive_account().await,
            "aevo" => self.aevo_account().await,
            _ => anyhow::bail!(
                "Live account fetch is not implemented for {}; refusing to fabricate an account snapshot",
                self.name
            ),
        }
    }

    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        self.require_trading_ready()?;
        match self.spec_id {
            "pacifica" => self.pacifica_place(&order).await,
            "standx" => self.standx_place(&order).await,
            "ondoperps" => self.ondo_place(&order).await,
            "grvt" => self.grvt_place(&order).await,
            "derive" => self.derive_place(&order).await,
            "aevo" => self.aevo_place(&order).await,
            _ => anyhow::bail!(wallet_execution_unavailable(
                self.name(),
                "its settlement chain",
                "wallet-signed order flow"
            )),
        }
    }

    async fn cancel_order(&self, req: CancelRequest) -> anyhow::Result<()> {
        self.require_trading_ready()?;
        match self.spec_id {
            "pacifica" => self.pacifica_cancel(&req).await,
            "standx" => self.standx_cancel(&req).await,
            "ondoperps" => self.ondo_cancel(&req).await,
            "grvt" => self.grvt_cancel(&req).await,
            "derive" => self.derive_cancel(&req).await,
            "aevo" => self.aevo_cancel(&req).await,
            _ => anyhow::bail!(wallet_execution_unavailable(
                self.name(),
                "its settlement chain",
                "wallet-signed order flow"
            )),
        }
    }

    async fn fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        self.require_trading_ready()?;
        match self.spec_id {
            "pacifica" => {
                self.pacifica_fetch_order(symbol, order_id, client_order_id)
                    .await
            }
            "standx" => {
                self.standx_fetch_order(symbol, order_id, client_order_id)
                    .await
            }
            "ondoperps" => {
                self.ondo_fetch_order(symbol, order_id, client_order_id)
                    .await
            }
            "grvt" => {
                self.grvt_fetch_order(symbol, order_id, client_order_id)
                    .await
            }
            "derive" => {
                self.derive_fetch_order(symbol, order_id, client_order_id)
                    .await
            }
            "aevo" => {
                self.aevo_fetch_order(symbol, order_id, client_order_id)
                    .await
            }
            _ => anyhow::bail!(
                "Order-status polling is not implemented for venue {}",
                self.name()
            ),
        }
    }

    async fn set_leverage(&self, symbol: &str, leverage: u32) -> anyhow::Result<()> {
        match self.spec_id {
            "pacifica" => {
                let vsym = self.venue_symbol(symbol);
                self.pacifica_op(
                    "update_leverage",
                    &serde_json::json!({"symbol": vsym, "leverage": leverage}),
                    "/api/v1/positions/leverage",
                )
                .await?;
                Ok(())
            }
            "standx" => {
                let vsym = self.venue_symbol(symbol);
                let body = serde_json::json!({"symbol": vsym, "leverage": leverage.to_string()});
                let headers = self.standx_headers(&body).await?;
                let url = format!("{}/api/change_leverage", self.base_url);
                let resp = self.post_json_url(&url, &body, &headers).await?;
                if resp.get("code").and_then(|v| v.as_i64()) != Some(0) {
                    anyhow::bail!("StandX change_leverage rejected: {}", resp);
                }
                Ok(())
            }
            _ => anyhow::bail!(
                "Leverage updates are not implemented for {}; refusing to pretend to apply it",
                self.name
            ),
        }
    }

    fn supports_live_execution(&self) -> bool {
        LIVE_TRADING_VENUE_IDS.contains(&self.spec_id)
    }

    fn live_order_submission_implemented(&self) -> bool {
        LIVE_TRADING_VENUE_IDS.contains(&self.spec_id)
    }

    fn order_status_polling_implemented(&self) -> bool {
        LIVE_TRADING_VENUE_IDS.contains(&self.spec_id)
    }

    fn has_live_credentials(&self) -> bool {
        let c = self.creds.read();
        let has_key = self.wallet.read().is_some() || self.ed25519.read().is_some();
        match self.spec_id {
            // API-key venues need the key pair (+ key for GRVT login).
            "aevo" => c.api_key.is_some() && c.api_secret.is_some() && has_key,
            "grvt" => c.api_key.is_some() && has_key,
            // Wallet-auth venues need wallet + signing key.
            "standx" => c.wallet_address.is_some() && has_key && self.ed25519.read().is_some(),
            "pacifica" | "ondoperps" | "derive" => c.wallet_address.is_some() && has_key,
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests (pure parsing + fail-closed paths — no network)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_specs_resolve_and_counts_match() {
        assert_eq!(DEX_PERP_VENUE_IDS.len(), 26);
        for id in DEX_PERP_VENUE_IDS {
            assert!(spec_for(id).is_some(), "missing spec for {id}");
            assert!(DexPerpAdapter::for_venue(id, None).is_some());
        }
        // Unknown ids (RISEx — not live; Orderly-derivatives — Vanta alias)
        // fail closed at construction time.
        assert!(DexPerpAdapter::for_venue("risex", None).is_none());
        assert!(DexPerpAdapter::for_venue("orderly", None).is_none());
        assert!(DexPerpAdapter::for_venue("", None).is_none());
        // Live-trading flags agree with the trading venue list.
        for id in LIVE_TRADING_VENUE_IDS {
            let a = DexPerpAdapter::for_venue(id, None).expect("live venue");
            assert!(
                a.supports_live_execution(),
                "{id} must report live execution"
            );
            assert!(
                a.live_order_submission_implemented(),
                "{id} must report submission"
            );
            assert!(
                a.order_status_polling_implemented(),
                "{id} must report polling"
            );
        }
        // Market-data-only venues keep all capability flags false.
        for id in [
            "dydx", "paradex", "apex", "extended", "helix", "qfex", "edgex",
        ] {
            let a = DexPerpAdapter::for_venue(id, None).expect("md venue");
            assert!(
                !a.supports_live_execution(),
                "{id} must not report live execution"
            );
            assert!(
                !a.order_status_polling_implemented(),
                "{id} must not report polling"
            );
        }
    }

    #[test]
    fn admitted_set_matches_spec_flags() {
        // Every modeled venue is admitted (paper read-only); the admitted
        // list and the spec table must agree exactly.
        assert_eq!(ADMITTED_DEX_PERP_VENUE_IDS.len(), DEX_PERP_VENUE_IDS.len());
        for id in ADMITTED_DEX_PERP_VENUE_IDS {
            let a = DexPerpAdapter::for_venue(id, None).expect("admitted venue");
            assert!(a.is_admitted(), "{id} should be admitted");
            let spec = spec_for(id).expect("spec");
            assert!(spec.admit, "{id} spec flag must be admit");
        }
        for id in DEX_PERP_VENUE_IDS {
            assert!(
                ADMITTED_DEX_PERP_VENUE_IDS.contains(&id),
                "{id} missing from admitted list"
            );
        }
        // Non-venues still fail closed at construction time.
        assert!(DexPerpAdapter::for_venue("risex", None).is_none());
        assert!(DexPerpAdapter::for_venue("orderly", None).is_none());
    }

    #[test]
    fn parse_dydx_markets_skips_non_active() {
        let venue = VenueId::new("dydx");
        let data = serde_json::json!({
            "markets": {
                "BTC-USD": {"ticker": "BTC-USD", "status": "ACTIVE", "tickSize": "1", "stepSize": "0.0001"},
                "OLD-USD": {"ticker": "OLD-USD", "status": "CANCELONLY", "tickSize": "1", "stepSize": "0.0001"},
                "BAD-USD": {"ticker": "BAD-USD", "status": "ACTIVE", "tickSize": "0", "stepSize": "0.0001"}
            }
        });
        let out = parse_dydx_markets(&venue, &data, dec!(2.0));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].venue_symbol, "BTC-USD");
    }

    #[test]
    fn parse_paradex_skips_options() {
        let venue = VenueId::new("paradex");
        let data = serde_json::json!({
            "results": [
                {"symbol": "BTC-USD-PERP", "base_currency": "BTC", "quote_currency": "USD",
                 "price_tick_size": "0.1", "order_size_increment": "0.0001"},
                {"symbol": "ZEC-USD-27NOV26-1200-P", "base_currency": "ZEC", "quote_currency": "USD",
                 "price_tick_size": "0.001", "order_size_increment": "0.1"}
            ]
        });
        let out = parse_paradex_markets(&venue, &data, dec!(2.0));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].venue_symbol, "BTC-USD-PERP");
    }

    #[test]
    fn parse_pair_book_rejects_empty_sides() {
        let venue = VenueId::new("paradex");
        let empty = serde_json::json!({"bids": [], "asks": []});
        assert!(parse_pair_book(&venue, "BTC-USDC", &empty).is_err());
        let good = serde_json::json!({"bids": [["81256.2", "0.5"]], "asks": [["81263.1", "0.25"]]});
        let book = parse_pair_book(&venue, "BTC-USDC", &good).expect("levels");
        assert_eq!(book.bids.len(), 1);
        assert_eq!(book.asks.len(), 1);
    }

    #[test]
    fn parse_apex_markets_and_book_match_live_shape() {
        let venue = VenueId::new("apex");
        let data = serde_json::json!({
            "data": {"contractConfig": {"perpetualContract": [
                {"crossSymbolName": "BTCUSDT", "symbol": "BTC-USDT",
                 "tickSize": "0.1", "stepSize": "0.001", "enableTrade": true},
                {"crossSymbolName": "OLDUSDT", "symbol": "OLD-USDT",
                 "tickSize": "0.1", "stepSize": "0.001", "enableTrade": false}
            ]}}
        });
        let out = parse_apex_markets(&venue, &data, dec!(2.0));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].venue_symbol, "BTCUSDT");
        assert_eq!(out[0].symbol.base, "BTC");
        let book = serde_json::json!({"data": {
            "b": [["81375.0", "3.735"]],
            "a": [["81388.4", "0.425"]]
        }});
        let b = parse_apex_book(&venue, "BTC", &book).expect("apex book");
        assert_eq!(b.bids.len(), 1);
        assert_eq!(b.asks.len(), 1);
        assert!(b.best_bid_price().unwrap() < b.best_ask_price().unwrap());
        // Dead symbol (null sides) fails closed.
        let dead = serde_json::json!({"data": {"a": null, "b": null}});
        assert!(parse_apex_book(&venue, "ZZZ", &dead).is_err());
    }

    #[test]
    fn parse_extended_markets_book_and_times_match_live_shape() {
        let venue = VenueId::new("extended");
        let data = serde_json::json!({
            "status": "OK", "data": [
                {"name": "BTC-USD", "type": "PERPETUAL", "assetName": "BTC",
                 "collateralAssetName": "USD", "status": "ACTIVE",
                 "tradingConfig": {"minPriceChange": "1", "minOrderSizeChange": "0.0001"},
                 "marketStats": {"fundingRate": "0.000013", "nextFundingRate": 1789786800000i64}},
                {"name": "OLD-USD", "type": "SPOT", "assetName": "OLD",
                 "collateralAssetName": "USD", "status": "ACTIVE", "tradingConfig": {}}
            ]
        });
        let out = parse_extended_markets(&venue, &data, dec!(2.0));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].venue_symbol, "BTC-USD");
        let book = serde_json::json!({"status": "OK", "data": {
            "market": "BTC-USD",
            "bid": [{"price": "81329", "qty": "0.43364"}],
            "ask": [{"price": "81332", "qty": "0.05534"}]
        }});
        let b = parse_extended_book(&venue, "BTC", &book).expect("ext book");
        assert!(b.best_bid_price().unwrap() < b.best_ask_price().unwrap());
        assert!(parse_millis_opt(Some(&serde_json::json!(1789786800000i64))).is_some());
        assert!(parse_millis_opt(Some(&serde_json::json!("x"))).is_none());
    }

    #[test]
    fn parse_standx_markets_match_live_shape() {
        let venue = VenueId::new("standx");
        let data = serde_json::json!([
            {"symbol": "BTC-USD", "base_asset": "BTC", "quote_asset": "DUSD",
             "price_tick_decimals": 2, "qty_tick_decimals": 2,
             "taker_fee": "0.0004", "status": "trading"},
            {"symbol": "OLD-USD", "base_asset": "OLD", "quote_asset": "DUSD",
             "price_tick_decimals": 2, "qty_tick_decimals": 2,
             "taker_fee": "0.0004", "status": "halted"}
        ]);
        let out = parse_standx_markets(&venue, &data, dec!(4.0));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].venue_symbol, "BTC-USD");
        assert_eq!(out[0].price_decimals, 2);
        // 0.0004 fraction -> 4.0 bps per-entry fee.
        assert_eq!(out[0].taker_fee_bps, dec!(4.0));
        // StandX depth uses plain asks/bids pairs (shared parser).
        let depth = serde_json::json!({
            "asks": [["81357", "0.0374"]], "bids": [["81346", "0.1"]]});
        let b = parse_pair_book(&venue, "BTC", &depth).expect("stx book");
        assert!(b.best_bid_price().unwrap() < b.best_ask_price().unwrap());
        assert!(parse_time_opt(Some(&serde_json::json!("2026-09-19T03:00:00Z"))).is_some());
    }

    #[test]
    fn parse_pacifica_markets_and_book_match_live_shape() {
        let venue = VenueId::new("pacifica");
        let data = serde_json::json!({"success": true, "data": [
            {"symbol": "BTC", "base_asset": "BTC", "tick_size": "1",
             "lot_size": "0.00001", "instrument_type": "perpetual"}
        ]});
        let out = parse_pacifica_markets(&venue, &data, dec!(2.0));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].venue_symbol, "BTC");
        let book = serde_json::json!({"success": true, "data": {
            "s": "BTC",
            "l": [
                [{"p": "81327", "a": "0.08203", "n": 3}],
                [{"p": "81328", "a": "0.59282", "n": 15}]
            ], "t": 1789784245931i64}});
        let b = parse_pacifica_book(&venue, "BTC", &book).expect("pac book");
        assert!(b.best_bid_price().unwrap() < b.best_ask_price().unwrap());
        // A crossed venue encoding must fail closed, never publish.
        let crossed = serde_json::json!({"success": true, "data": {
            "s": "BTC",
            "l": [
                [{"p": "81328", "a": "0.5"}],
                [{"p": "81327", "a": "0.5"}]
            ], "t": 1}});
        // First array is bids by position; prices uncrossed here only if the
        // venue keeps its encoding — this fixture is bids(81328) above
        // asks(81327), i.e. crossed, and must be rejected.
        assert!(parse_pacifica_book(&venue, "BTC", &crossed).is_err());
    }

    #[test]
    fn parse_derive_markets_and_topbook_match_live_shape() {
        let venue = VenueId::new("derive");
        let data = serde_json::json!({"result": [
            {"instrument_type": "perp", "instrument_name": "ETH-PERP",
             "is_active": true, "tick_size": "0.01", "amount_step": "0.001",
             "base_currency": "ETH", "quote_currency": "USD",
             "taker_fee_rate": "0.0003"},
            {"instrument_type": "option", "instrument_name": "ETH-260926-3000-C",
             "is_active": true, "tick_size": "0.01", "amount_step": "0.001",
             "base_currency": "ETH", "quote_currency": "USD"}
        ]});
        let out = parse_derive_markets(&venue, &data, dec!(3.0));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].venue_symbol, "ETH-PERP");
        let result = serde_json::json!({
            "best_bid_price": "2616.67", "best_bid_amount": "11.465",
            "best_ask_price": "2620.35", "best_ask_amount": "11.449"});
        let b = parse_derive_topbook(&venue, "ETH", &result).expect("drv book");
        assert_eq!(b.bids.len(), 1);
        assert_eq!(b.asks.len(), 1);
    }

    #[test]
    fn parse_helix_markets_and_scaled_book_match_live_shape() {
        let venue = VenueId::new("helix");
        let data = serde_json::json!({"markets": [
            {"market": {"ticker": "BTC/USDC PERP",
             "market_id": "0xabc", "status": "Active", "isPerpetual": true,
             "quote_decimals": 6, "min_price_tick_size": "1000000.000000000000000000",
             "min_quantity_tick_size": "0.000010000000000000",
             "taker_fee_rate": "0.000001000000000000"}},
            {"market": {"ticker": "OLD/USDC PERP",
             "market_id": "0xdef", "status": "Paused", "quote_decimals": 6,
             "min_price_tick_size": "1000000.0",
             "min_quantity_tick_size": "0.00001",
             "taker_fee_rate": "0.000001"}}
        ]});
        let out = parse_helix_markets(&venue, &data);
        assert_eq!(out.len(), 1);
        // 1e6 raw tick / 1e6 scale = 1.0 human tick.
        assert_eq!(out[0].price_decimals, 0);
        let book = serde_json::json!({
            "buys_price_level": [{"p": "81312000000.000000000000000000", "q": "0.78409"}],
            "sells_price_level": [{"p": "81332000000.000000000000000000", "q": "0.05534"}]});
        let b = parse_helix_book(&venue, "BTC", &book, dec!(1000000)).expect("helix book");
        assert_eq!(b.best_bid_price().unwrap(), dec!(81312));
        assert_eq!(b.best_ask_price().unwrap(), dec!(81332));
    }

    #[test]
    fn parse_qfex_refdata_matches_live_shape() {
        let venue = VenueId::new("qfex");
        let data = serde_json::json!({"type": "refdata", "data": [
            {"symbol": "BTC-USD", "base_asset": "BTC", "quote_asset": "USD",
             "tick_size": "0.1", "lot_size": "0.001"}
        ]});
        let out = parse_qfex_markets(&venue, &data, dec!(4.0));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].venue_symbol, "BTC-USD");
        assert_eq!(out[0].symbol.base, "BTC");
    }

    #[test]
    fn venue_symbol_mapping_is_explicit() {
        let dydx = DexPerpAdapter::dydx(None);
        assert_eq!(dydx.venue_symbol("BTC"), "BTC-USD");
        let paradex = DexPerpAdapter::paradex(None);
        assert_eq!(paradex.venue_symbol("BTC-USDC"), "BTC-USD-PERP");
        let aevo = DexPerpAdapter::aevo(None);
        assert_eq!(aevo.venue_symbol("eth"), "ETH-PERP");
        let apex = DexPerpAdapter::apex(None);
        assert_eq!(apex.venue_symbol("BTC"), "BTCUSDT");
        let extended = DexPerpAdapter::extended(None);
        assert_eq!(extended.venue_symbol("btc/usd"), "BTC-USD");
        let standx = DexPerpAdapter::standx(None);
        assert_eq!(standx.venue_symbol("BTC"), "BTC-USD");
        let pacifica = DexPerpAdapter::pacifica(None);
        assert_eq!(pacifica.venue_symbol("BTC"), "BTC");
        let derive = DexPerpAdapter::derive(None);
        assert_eq!(derive.venue_symbol("ETH"), "ETH-PERP");
        let qfex = DexPerpAdapter::qfex(None);
        assert_eq!(qfex.venue_symbol("BTC"), "BTC-USD");
        let edgex = DexPerpAdapter::edgex(None);
        assert_eq!(edgex.venue_symbol("BTC"), "BTCUSD");
        let grvt = DexPerpAdapter::grvt(None);
        assert_eq!(grvt.venue_symbol("BTC"), "BTC_USDT_Perp");
        let ondo = DexPerpAdapter::ondoperps(None);
        assert_eq!(ondo.venue_symbol("BTC"), "BTC-USD.P");
    }

    #[test]
    fn parse_new_venue_markets_match_live_shapes() {
        // edgeX contract metadata shape.
        let venue = VenueId::new("edgex");
        let data = serde_json::json!({"code": 0, "data": {"contractList": [
            {"contractId": "10000001", "contractName": "BTCUSD", "tickSize": "0.1",
             "stepSize": "0.001", "status": "enabled"},
            {"contractId": "20000001", "contractName": "EURUSD", "tickSize": "0.01",
             "stepSize": "0.001", "status": "disabled"}
        ], "coinList": []}});
        let out = parse_edgex_markets(&venue, &data, dec!(3.8));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].venue_symbol, "BTCUSD");

        // GRVT all_instruments lite shape.
        let venue = VenueId::new("grvt");
        let data = serde_json::json!({"result": [
            {"i": "BTC_USDT_Perp", "b": "BTC", "q": "USDT", "k": "PERPETUAL",
             "ih": "0xabc", "ts": "0.01", "ms": "0.0001"},
            {"i": "BTC_USDT_Fut_20Oct23", "b": "BTC", "q": "USDT", "k": "FUTURE",
             "ih": "0xdef", "ts": "0.01", "ms": "0.0001"}
        ]});
        let out = parse_grvt_markets(&venue, &data, dec!(5.0));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].venue_symbol, "BTC_USDT_Perp");
        assert_eq!(out[0].symbol.base, "BTC");

        // Ondo tradingPairs shape.
        let venue = VenueId::new("ondoperps");
        let data = serde_json::json!({"success": true, "result": {"perps": {"tradingPairs": [
            {"market": "BTC-USD.P", "pair": {"base": "BTC", "quote": "USD"},
             "baseIncrement": "0.0001", "quoteIncrement": "1",
             "fundingIntervalDivisions": 8}
        ]}}});
        let out = parse_ondo_markets(&venue, &data, dec!(2.5));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].venue_symbol, "BTC-USD.P");

        // GRVT book parser.
        let book = serde_json::json!({"result": {
            "bids": [{"price": "81327", "size": "0.08203"}],
            "asks": [{"price": "81328", "size": "0.59282"}]}});
        let b = parse_grvt_book(&venue, "BTC", &book).expect("grvt book");
        assert!(b.best_bid_price().unwrap() < b.best_ask_price().unwrap());
    }

    #[test]
    fn canonical_json_sorts_keys_recursively() {
        let v = serde_json::json!({"z": 1, "a": {"y": 2, "b": 3}, "m": [1, 2]});
        let s = canonical_json(&v);
        assert!(s.find("\"a\"").unwrap() < s.find("\"m\"").unwrap());
        assert!(s.find("\"m\"").unwrap() < s.find("\"z\"").unwrap());
        // Nested object keys sorted too.
        assert!(s.find("\"b\"").unwrap() < s.find("\"y\"").unwrap());
    }

    #[test]
    fn map_order_status_never_guesses_terminal_states() {
        use arbitrageur_core::types::OrderStatus;
        assert_eq!(
            map_order_status("filled", dec!(1), dec!(1)),
            Some(OrderStatus::Filled)
        );
        assert_eq!(
            map_order_status("OPEN", dec!(0), dec!(1)),
            Some(OrderStatus::New)
        );
        assert_eq!(
            map_order_status("cancelled", dec!(0), dec!(1)),
            Some(OrderStatus::Cancelled)
        );
        // Unknown status strings fail closed — never inferred.
        assert_eq!(
            map_order_status("partially_canceled", dec!(0.5), dec!(1)),
            None
        );
        assert_eq!(map_order_status("weird_new_state", dec!(0), dec!(1)), None);
    }

    #[tokio::test]
    async fn trading_venues_fail_closed_without_credentials() {
        for id in LIVE_TRADING_VENUE_IDS {
            let a = DexPerpAdapter::for_venue(id, None).expect("live venue");
            assert!(
                !a.has_live_credentials(),
                "{id} must not report credentials"
            );
            let req = OrderRequest {
                client_order_id: "test".into(),
                venue: a.venue_id(),
                symbol: "BTC-USD".into(),
                side: arbitrageur_core::types::Side::Buy,
                order_type: arbitrageur_core::types::OrderType::Limit,
                price: Some(dec!(50000)),
                quantity: dec!(0.01),
                time_in_force: arbitrageur_core::types::TimeInForce::GoodTilCancelled,
                post_only: false,
                reduce_only: false,
                created_at: Utc::now(),
            };
            assert!(
                a.place_order(req).await.is_err(),
                "{id} place without creds must fail"
            );
        }
    }

    #[tokio::test]
    async fn fail_closed_account_order_leverage_paths() {
        let a = DexPerpAdapter::dydx(None);
        assert!(a.fetch_account().await.is_err());
        assert!(a.set_leverage("BTC", 5).await.is_err());
        assert!(!a.supports_live_execution());
        // GMX has no book by construction.
        let gmx = DexPerpAdapter::gmx(None);
        assert!(gmx.fetch_orderbook("BTC").await.is_err());
    }
}

#[cfg(test)]
mod wallet_direct_tests {
    use super::*;

    #[test]
    fn wallet_binding_prepares_evm_signer() {
        let a = DexPerpAdapter::dydx(None);
        // Empty binding clears without error.
        assert!(a
            .update_credentials(None, None, None, None, None, None)
            .is_ok());
        assert_eq!(a.wallet_address_hex(), None);
        // Delegated-agent case: main address differs from agent key — must bind.
        assert!(a
            .update_credentials(
                None,
                None,
                None,
                None,
                Some("0x0000000000000000000000000000000000000001".to_string()),
                Some(
                    "0x0000000000000000000000000000000000000000000000000000000000000001"
                        .to_string()
                ),
            )
            .is_ok());
        assert_eq!(
            a.wallet_address_hex(),
            Some("0x0000000000000000000000000000000000000001".to_string())
        );
        assert_eq!(
            a.agent_address_hex().as_deref(),
            Some("0x7e5f4552091a69125d5dfcb7b8c2659029395bdf")
        );
        // Invalid address format fails the whole update (no half-bound state).
        assert!(a
            .update_credentials(
                None,
                None,
                None,
                None,
                Some("not-an-address".to_string()),
                Some(
                    "0x0000000000000000000000000000000000000000000000000000000000000001"
                        .to_string()
                ),
            )
            .is_err());
        // Scalar-1 test vector binds (direct-key case).
        assert!(a
            .update_credentials(
                None,
                None,
                None,
                None,
                Some("0x7E5F4552091a69125d5dfcb7b8c2659029395bdf".to_string()),
                Some(
                    "0x0000000000000000000000000000000000000000000000000000000000000001"
                        .to_string()
                ),
            )
            .is_ok());
        // dYdX stays market-data-only — bound keys never enable execution.
        assert!(!a.supports_live_execution());
    }

    #[test]
    fn pacifica_credentials_use_ed25519_and_base58_account() {
        let a = DexPerpAdapter::pacifica(None);
        // Solana keypair: 64 bytes, base58 — this is the well-known
        // [0;32] seed + pubkey layout used in tests everywhere.
        let keypair = [0u8; 64];
        let b58 = bs58::encode(keypair).into_string();
        let pubkey = bs58::encode(
            ed25519_dalek::SigningKey::from_bytes(&[0u8; 32])
                .verifying_key()
                .to_bytes(),
        )
        .into_string();
        assert!(a
            .update_credentials(None, None, None, None, Some(pubkey), Some(b58),)
            .is_ok());
        // A 0x address must be rejected for Pacifica (Solana venue).
        let a2 = DexPerpAdapter::pacifica(None);
        assert!(a2
            .update_credentials(
                None,
                None,
                None,
                None,
                Some("0x0000000000000000000000000000000000000001".to_string()),
                None,
            )
            .is_err());
        // Bound ed25519 key + wallet → live credentials present.
        assert!(a.has_live_credentials());
        assert!(a.supports_live_execution());
    }

    #[test]
    fn aevo_credentials_require_key_secret_and_signer() {
        let a = DexPerpAdapter::aevo(None);
        // API key only — no signer → not live-ready.
        assert!(a
            .update_credentials(
                Some("k".to_string()),
                Some("s".to_string()),
                None,
                None,
                Some("0x0000000000000000000000000000000000000001".to_string()),
                None,
            )
            .is_ok());
        assert!(!a.has_live_credentials());
        // Add the signing key → live-ready.
        assert!(a
            .update_credentials(
                Some("k".to_string()),
                Some("s".to_string()),
                None,
                None,
                Some("0x0000000000000000000000000000000000000001".to_string()),
                Some(
                    "0x0000000000000000000000000000000000000000000000000000000000000001"
                        .to_string()
                ),
            )
            .is_ok());
        assert!(a.has_live_credentials());
    }
}
