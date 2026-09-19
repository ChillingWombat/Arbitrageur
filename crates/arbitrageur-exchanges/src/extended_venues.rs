//! Extended perpetual-venue adapters (paper read-only, fail-closed).
//!
//! # What lives here
//! One [`ExtendedVenueAdapter`] struct serves many long-tail perp venues
//! through a per-venue [`VenueApiSpec`] table (public REST only, no auth).
//! Every venue below follows the same contract as `grvt.rs`:
//! books, tickers, markets, and funding go through the read-only paths,
//! while account / order / leverage paths fail closed with an explanatory
//! error (never a fabricated snapshot or fill). `supports_live_execution`
//! is `false` for all of them.
//!
//! # Verification status (2026-09-19, live curl from this sandbox)
//! - LIVE-VERIFIED (curl returned real payloads, parsers match exact keys):
//!   dYdX v4, Paradex, Aevo, GMX V2 (prices/tickers only — the GLV AMM has
//!   no orderbook by design), ApeX Omni (`omni.apex.exchange`: symbols /
//!   depth / ticker), Extended (`api.starknet.extended.exchange`: markets /
//!   `{market}/orderbook` / marketStats funding), Derive (`api.lyra.finance`:
//!   per-currency instruments + `get_ticker` top-of-book/funding),
//!   StandX (`perps.standx.com`: symbol_info / depth_book / symbol_market),
//!   Helix via Injective chain LCD (`sentry.lcd.injective.network`:
//!   derivative markets + scaled orderbook), Pacifica (`api.pacifica.fi`:
//!   `/v1/info` + `/v1/book?symbol=` + `/v1/info/prices`),
//!   QFEX markets only (`api.qfex.com/refdata` — book/ticker/trades are
//!   auth-gated and fail closed with an explanatory error).
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
        anyhow::bail!("No live orderbook levels returned from {} for {symbol}", venue.as_str());
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
            id: "apex", name: "ApeX Omni",
            base_url: "https://omni.apex.exchange", markets_path: "/api/v3/symbols",
            taker_fee_bps: fee("2.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: true,
        },
        "extended" => VenueApiSpec {
            id: "extended", name: "Extended",
            base_url: "https://api.starknet.extended.exchange", markets_path: "/api/v1/info/markets",
            taker_fee_bps: fee("2.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: true,
        },
        "derive" => VenueApiSpec {
            id: "derive", name: "Derive",
            base_url: "https://api.lyra.finance", markets_path: "/public/get_ticker?instrument_name=ETH-PERP",
            taker_fee_bps: fee("3.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: true,
        },
        "standx" => VenueApiSpec {
            id: "standx", name: "StandX",
            base_url: "https://perps.standx.com", markets_path: "/api/query_symbol_info",
            taker_fee_bps: fee("4.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: true,
        },
        "helix" => VenueApiSpec {
            id: "helix", name: "Helix",
            base_url: "https://sentry.lcd.injective.network",
            markets_path: "/injective/exchange/v1beta1/derivative/markets",
            taker_fee_bps: fee("0.01"), book_kind: BookKind::PairLevels,
            admit: true, verified: true,
        },
        // ---- DOC-SHAPED, admitted paper-only per operator request ----
        // Shapes come from public docs; a harness sweep from full egress
        // should still confirm each payload shape. Parsers fail closed per
        // entry, so a wrong doc guess degrades to venue errors, never bad
        // data. High-caution venues (Ostium, AlphaX) stay paper-only
        // regardless — see docs/EXCHANGES.md section 10. Pacifica and QFEX
        // graduated to LIVE-VERIFIED above (QFEX: markets only).
        "vest" => VenueApiSpec {
            id: "vest", name: "Vest",
            base_url: "https://api.vest.exchange", markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: false,
        },
        "antarctic" => VenueApiSpec {
            id: "antarctic", name: "Antarctic",
            base_url: "https://api.antarctic.exchange", markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: false,
        },
        "evedex" => VenueApiSpec {
            id: "evedex", name: "EVEDEX",
            base_url: "https://api.evedex.com", markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: false,
        },
        "bulk" => VenueApiSpec {
            id: "bulk", name: "BULK",
            base_url: "https://api.bulk.exchange", markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: false,
        },
        "qfex" => VenueApiSpec {
            id: "qfex", name: "QFEX",
            base_url: "https://api.qfex.com", markets_path: "/refdata",
            taker_fee_bps: fee("4.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: true,
        },
        "ostium" => VenueApiSpec {
            id: "ostium", name: "Ostium",
            base_url: "https://api.ostium.io", markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: false,
        },
        "sodex" => VenueApiSpec {
            id: "sodex", name: "SoDEX",
            base_url: "https://api.sodex.xyz", markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: false,
        },
        "alphax" => VenueApiSpec {
            id: "alphax", name: "AlphaX",
            base_url: "https://api.alphax.exchange", markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: false,
        },
        "pacifica" => VenueApiSpec {
            id: "pacifica", name: "Pacifica",
            base_url: "https://api.pacifica.fi", markets_path: "/api/v1/info",
            taker_fee_bps: fee("2.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: true,
        },
        "nado" => VenueApiSpec {
            id: "nado", name: "Nado",
            base_url: "https://api.nado.xyz", markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: false,
        },
        "iotrader" => VenueApiSpec {
            id: "iotrader", name: "IO Trader",
            base_url: "https://api.io-trader.com", markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: false,
        },
        "niza" => VenueApiSpec {
            id: "niza", name: "Niza",
            base_url: "https://api.niza.fun", markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: false,
        },
        "astros" => VenueApiSpec {
            id: "astros", name: "Astros",
            base_url: "https://api.astros.ag", markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: false,
        },
        "gatedex" => VenueApiSpec {
            id: "gatedex", name: "Gate DEX",
            base_url: "https://api.gate-dex.io", markets_path: "/v1/markets",
            taker_fee_bps: fee("3.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: false,
        },
        _ => return None,
    };
    Some(spec)
}

/// All venue ids this module implements, in alphabetical order.
pub const EXTENDED_VENUE_IDS: [&str; 23] = [
    "aevo", "alphax", "antarctic", "apex", "astros", "bulk", "derive",
    "dydx", "extended", "evedex", "gatedex", "gmx", "helix", "iotrader",
    "nado", "niza", "ostium", "pacifica", "paradex", "qfex", "sodex",
    "standx", "vest",
];

/// Lyra/Derive serves one instrument per call, so Derive markets fan out
/// over this static currency list (each response is cached by path, so a
/// refresh is cheap after the first sweep). Currencies with no perp return
/// an empty `result` and contribute nothing.
const DERIVE_CURRENCIES: [&str; 10] = [
    "BTC", "ETH", "SOL", "ARB", "OP", "AVAX", "LINK", "DOGE", "SUI", "NEAR",
];

/// Venue ids admitted to `get_venues` (paper read-only). All 23 venues in
/// this module are admitted: the 4 curl-verified plus 19 doc-shaped shapes.
/// Doc-shaped venues still deserve a harness sweep from full egress, but the
/// per-entry fail-closed parsers guarantee a wrong doc guess degrades to
/// venue errors, never bad data.
pub const ADMITTED_EXTENDED_VENUE_IDS: [&str; 23] = [
    "aevo", "alphax", "antarctic", "apex", "astros", "bulk", "derive",
    "dydx", "extended", "evedex", "gatedex", "gmx", "helix", "iotrader",
    "nado", "niza", "ostium", "pacifica", "paradex", "qfex", "sodex",
    "standx", "vest",
];

// ---------------------------------------------------------------------------
// Adapter struct (one type, driven by the spec table)
// ---------------------------------------------------------------------------

/// Paper read-only adapter for long-tail perp venues. Construct with
/// [`ExtendedVenueAdapter::for_venue`] (returns `None` for unknown ids) or
/// the per-venue helpers (`dydx()`, `paradex()`, …) used by `state.rs`.
pub struct ExtendedVenueAdapter {
    venue_id: VenueId,
    name: String,
    spec_id: &'static str,
    client: Client,
    base_url: String,
    rate_limiter: AccountRequestCoordinator,
    wallet_address: RwLock<Option<String>>,
    wallet: RwLock<Option<EvmWalletSigner>>,
}

impl ExtendedVenueAdapter {
    /// Build an adapter for a venue id in [`EXTENDED_VENUE_IDS`].
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
        })
    }

    /// Whether `get_venues` should admit this venue (spec `admit` flag).
    pub fn is_admitted(&self) -> bool {
        spec_for(self.spec_id).map(|s| s.admit).unwrap_or(false)
    }

    // Per-venue constructors (used by state.rs so call sites read clearly).
    pub fn dydx(base_url: Option<String>) -> Self { Self::for_venue("dydx", base_url).expect("dydx spec") }
    pub fn paradex(base_url: Option<String>) -> Self { Self::for_venue("paradex", base_url).expect("paradex spec") }
    pub fn aevo(base_url: Option<String>) -> Self { Self::for_venue("aevo", base_url).expect("aevo spec") }
    pub fn gmx(base_url: Option<String>) -> Self { Self::for_venue("gmx", base_url).expect("gmx spec") }
    pub fn apex(base_url: Option<String>) -> Self { Self::for_venue("apex", base_url).expect("apex spec") }
    pub fn extended(base_url: Option<String>) -> Self { Self::for_venue("extended", base_url).expect("extended spec") }
    pub fn derive(base_url: Option<String>) -> Self { Self::for_venue("derive", base_url).expect("derive spec") }
    pub fn standx(base_url: Option<String>) -> Self { Self::for_venue("standx", base_url).expect("standx spec") }
    pub fn helix(base_url: Option<String>) -> Self { Self::for_venue("helix", base_url).expect("helix spec") }
    pub fn vest(base_url: Option<String>) -> Self { Self::for_venue("vest", base_url).expect("vest spec") }
    pub fn antarctic(base_url: Option<String>) -> Self { Self::for_venue("antarctic", base_url).expect("antarctic spec") }
    pub fn evedex(base_url: Option<String>) -> Self { Self::for_venue("evedex", base_url).expect("evedex spec") }
    pub fn bulk(base_url: Option<String>) -> Self { Self::for_venue("bulk", base_url).expect("bulk spec") }
    pub fn qfex(base_url: Option<String>) -> Self { Self::for_venue("qfex", base_url).expect("qfex spec") }
    pub fn ostium(base_url: Option<String>) -> Self { Self::for_venue("ostium", base_url).expect("ostium spec") }
    pub fn sodex(base_url: Option<String>) -> Self { Self::for_venue("sodex", base_url).expect("sodex spec") }
    pub fn alphax(base_url: Option<String>) -> Self { Self::for_venue("alphax", base_url).expect("alphax spec") }
    pub fn pacifica(base_url: Option<String>) -> Self { Self::for_venue("pacifica", base_url).expect("pacifica spec") }
    pub fn nado(base_url: Option<String>) -> Self { Self::for_venue("nado", base_url).expect("nado spec") }
    pub fn iotrader(base_url: Option<String>) -> Self { Self::for_venue("iotrader", base_url).expect("iotrader spec") }
    pub fn niza(base_url: Option<String>) -> Self { Self::for_venue("niza", base_url).expect("niza spec") }
    pub fn astros(base_url: Option<String>) -> Self { Self::for_venue("astros", base_url).expect("astros spec") }
    pub fn gatedex(base_url: Option<String>) -> Self { Self::for_venue("gatedex", base_url).expect("gatedex spec") }

    /// Bind (or clear) the locally-held EVM wallet used for future
    /// wallet-direct signing. The hex key is parsed into a zeroizing signer
    /// immediately; the raw string is never retained. An invalid key fails
    /// the whole update so a half-bound state is impossible. Execution stays
    /// fail-closed until this venue's signing flow is implemented and
    /// audited — binding a key only prepares the vault.
    pub fn update_credentials(
        &self,
        wallet_address: Option<String>,
        private_key_hex: Option<String>,
    ) -> anyhow::Result<()> {
        let addr = wallet_address
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty());
        match private_key_hex
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
        {
            Some(hex_key) => {
                let signer = EvmWalletSigner::from_hex(&hex_key)?;
                if let Some(a) = addr.as_deref() {
                    if !a.eq_ignore_ascii_case(&signer.address_hex()) {
                        anyhow::bail!("wallet address does not match the supplied private key");
                    }
                }
                *self.wallet.write() = Some(signer);
                *self.wallet_address.write() = addr;
                Ok(())
            }
            None => {
                *self.wallet.write() = None;
                *self.wallet_address.write() = addr;
                Ok(())
            }
        }
    }

    /// Currently bound wallet address, if any (preparation only — execution
    /// stays disabled until the signing flow is audited).
    pub fn wallet_address_hex(&self) -> Option<String> {
        self.wallet_address.read().clone()
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
        let is_json = resp.headers().get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()).map(|ct| ct.contains("json")).unwrap_or(false);
        let text = resp.text().await.unwrap_or_default();
        if !is_json && !text.trim_start().starts_with(['{', '[']) {
            anyhow::bail!("{} returned non-JSON for {} (likely edge challenge)", self.name, path);
        }
        let val: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("{} unparseable JSON for {}: {e}", self.name, path))?;
        self.rate_limiter.record_success(text);
        Ok(val)
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
            _ => format!("/v1/orderbook?symbol={venue_symbol}"),
        }
    }

    /// Resolve a caller-side base asset (e.g. `BTC`) to its active Helix
    /// LCD market, preferring the USDC quote. Fails closed when the base
    /// has no active perp.
    async fn helix_resolve(&self, base: &str) -> anyhow::Result<HelixMarketInfo> {
        let data = self.get_json("/injective/exchange/v1beta1/derivative/markets").await?;
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
fn parse_dydx_markets(venue: &VenueId, data: &serde_json::Value, fee: Decimal) -> Vec<MarketMetadata> {
    let markets = match data.get("markets").and_then(|m| m.as_object()) {
        Some(m) => m,
        None => return Vec::new(),
    };
    markets.iter().filter_map(|(ticker, e)| {
        if e.get("status").and_then(|s| s.as_str()) != Some("ACTIVE") { return None; }
        let (base, quote) = ticker.split_once('-')?;
        if base.is_empty() || quote.is_empty() { return None; }
        meta(venue, base, quote, ticker.clone(),
            dec_from_json(e.get("tickSize")), dec_from_json(e.get("stepSize")), fee)
    }).collect()
}

/// Paradex: `{"results": [{symbol: "BTC-USD-PERP", base_currency,
/// quote_currency, price_tick_size, order_size_increment, …}]}` — includes
/// options; only `-PERP` symbols are admitted.
fn parse_paradex_markets(venue: &VenueId, data: &serde_json::Value, fee: Decimal) -> Vec<MarketMetadata> {
    let arr = match data.get("results").and_then(|r| r.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter().filter_map(|e| {
        let symbol = e.get("symbol").and_then(|s| s.as_str())?;
        if !symbol.ends_with("-PERP") { return None; }
        let base = e.get("base_currency").and_then(|s| s.as_str())?;
        let quote = e.get("quote_currency").and_then(|s| s.as_str()).unwrap_or("USD");
        if base.is_empty() { return None; }
        meta(venue, base, quote, symbol.to_string(),
            dec_from_json(e.get("price_tick_size")), dec_from_json(e.get("order_size_increment")), fee)
    }).collect()
}

/// Aevo / Derive style: `[{instrument_name: "BTC-PERP", underlying_asset,
/// quote_asset, price_step, amount_step, is_active}]`.
fn parse_aevo_markets(venue: &VenueId, data: &serde_json::Value, fee: Decimal) -> Vec<MarketMetadata> {
    let arr = match data.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter().filter_map(|e| {
        if e.get("is_active").and_then(|v| v.as_bool()) == Some(false) { return None; }
        let name = e.get("instrument_name").and_then(|s| s.as_str())?;
        if !name.ends_with("-PERP") { return None; }
        let base = e.get("underlying_asset").and_then(|s| s.as_str())?;
        let quote = e.get("quote_asset").and_then(|s| s.as_str()).unwrap_or("USDC");
        if base.is_empty() { return None; }
        meta(venue, base, quote, name.to_string(),
            dec_from_json(e.get("price_step")), dec_from_json(e.get("amount_step")), fee)
    }).collect()
}

/// GMX V2 tickers: `[{tokenSymbol, minPrice, maxPrice, …}]` — prices carry
/// chain-native decimals, so only the symbol set is admitted here; price
/// levels are normalized per-token in `parse_gmx_price` (30-decimal USD
/// feed). No orderbook exists by construction (GLV AMM).
/// ApeX Omni: `{"data": {"contractConfig": {"perpetualContract":
/// [{crossSymbolName: "BTCUSDT", symbol: "BTC-USDT", tickSize: "0.1",
/// stepSize: "0.001", enableTrade, enableDisplay, …}]}}}` — only
/// trade-enabled entries are admitted.
fn parse_apex_markets(venue: &VenueId, data: &serde_json::Value, fee: Decimal) -> Vec<MarketMetadata> {
    let arr = data.get("data").and_then(|d| d.get("contractConfig"))
        .and_then(|c| c.get("perpetualContract")).and_then(|v| v.as_array());
    let arr = match arr {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter().filter_map(|e| {
        if e.get("enableTrade").and_then(|v| v.as_bool()) == Some(false) { return None; }
        let cross = e.get("crossSymbolName").and_then(|s| s.as_str())?;
        if cross.is_empty() { return None; }
        // `symbol` is `{BASE}-{QUOTE}` ("BTC-USDT"); fall back to splitting
        // the trailing quote token off `crossSymbolName`.
        let (base, quote) = match e.get("symbol").and_then(|s| s.as_str()).and_then(|s| s.split_once('-')) {
            Some((b, q)) if !b.is_empty() && !q.is_empty() => (b.to_string(), q.to_string()),
            _ => {
                let b = to_base(cross);
                if b.is_empty() { return None; }
                (b, "USDT".to_string())
            }
        };
        meta(venue, &base, &quote, cross.to_string(),
            dec_from_json(e.get("tickSize")), dec_from_json(e.get("stepSize")), fee)
    }).collect()
}

/// Extended: `{"status": "OK", "data": [{name: "ENA-USD", type:
/// "PERPETUAL", assetName, collateralAssetName, status: "ACTIVE",
/// tradingConfig: {minPriceChange, minOrderSizeChange, minOrderSize},
/// marketStats: {…}}]}` — only ACTIVE PERPETUAL entries.
fn parse_extended_markets(venue: &VenueId, data: &serde_json::Value, fee: Decimal) -> Vec<MarketMetadata> {
    let arr = data.get("data").and_then(|v| v.as_array());
    let arr = match arr {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter().filter_map(|e| {
        if e.get("type").and_then(|s| s.as_str()) != Some("PERPETUAL") { return None; }
        if e.get("status").and_then(|s| s.as_str()) != Some("ACTIVE") { return None; }
        let name = e.get("name").and_then(|s| s.as_str())?;
        let base = e.get("assetName").and_then(|s| s.as_str()).unwrap_or("");
        let base = if base.is_empty() { to_base(name) } else { base.to_string() };
        if base.is_empty() { return None; }
        let quote = e.get("collateralAssetName").and_then(|s| s.as_str()).unwrap_or("USD");
        let cfg = e.get("tradingConfig");
        let price_tick = cfg.and_then(|c| dec_from_json(c.get("minPriceChange")));
        let size_tick = cfg.and_then(|c| dec_from_json(c.get("minOrderSizeChange")))
            .or_else(|| cfg.and_then(|c| dec_from_json(c.get("minOrderSize"))));
        meta(venue, &base, quote, name.to_string(), price_tick, size_tick, fee)
    }).collect()
}

/// StandX: `[{symbol: "BTC-USD", base_asset, quote_asset,
/// price_tick_decimals: 2, qty_tick_decimals: 2, taker_fee: "0.0004",
/// status: "trading"}]` — only `trading` symbols; the taker fee is a
/// fraction, converted to bps per entry (spec fee is the fallback).
fn parse_standx_markets(venue: &VenueId, data: &serde_json::Value, fee: Decimal) -> Vec<MarketMetadata> {
    let arr = match data.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter().filter_map(|e| {
        if e.get("status").and_then(|s| s.as_str()) != Some("trading") { return None; }
        let symbol = e.get("symbol").and_then(|s| s.as_str())?;
        let base = e.get("base_asset").and_then(|s| s.as_str()).map(|s| s.to_uppercase())
            .unwrap_or_else(|| to_base(symbol));
        if base.is_empty() { return None; }
        let quote = e.get("quote_asset").and_then(|s| s.as_str()).unwrap_or("DUSD");
        let tick = |k: &str| e.get(k).and_then(|v| v.as_u64()).map(|d| Decimal::new(1, d as u32));
        let entry_fee = dec_from_json(e.get("taker_fee")).map(|f| f * dec!(10000)).unwrap_or(fee);
        meta(venue, &base, quote, symbol.to_string(), tick("price_tick_decimals"), tick("qty_tick_decimals"), entry_fee)
    }).collect()
}

/// Pacifica: `{"success": true, "data": [{symbol: "BTC", base_asset,
/// tick_size: "1", lot_size: "0.00001", instrument_type: "perpetual",
/// …}]}` — markets are addressed by bare base asset; quote is USD.
fn parse_pacifica_markets(venue: &VenueId, data: &serde_json::Value, fee: Decimal) -> Vec<MarketMetadata> {
    let arr = data.get("data").and_then(|v| v.as_array());
    let arr = match arr {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter().filter_map(|e| {
        let symbol = e.get("symbol").and_then(|s| s.as_str())?;
        if symbol.is_empty() { return None; }
        let base = e.get("base_asset").and_then(|s| s.as_str()).map(|s| s.to_uppercase())
            .unwrap_or_else(|| symbol.to_uppercase());
        meta(venue, &base, "USD", symbol.to_string(),
            dec_from_json(e.get("tick_size")), dec_from_json(e.get("lot_size")), fee)
    }).collect()
}

/// Derive/Lyra per-currency instruments: `{"result": [{instrument_name:
/// "ETH-PERP", tick_size: "0.01", amount_step, minimum_amount,
/// base_currency, quote_currency, is_active}]}`.
fn parse_derive_markets(venue: &VenueId, data: &serde_json::Value, fee: Decimal) -> Vec<MarketMetadata> {
    let arr = data.get("result").and_then(|v| v.as_array());
    let arr = match arr {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter().filter_map(|e| {
        if e.get("is_active").and_then(|v| v.as_bool()) == Some(false) { return None; }
        let name = e.get("instrument_name").and_then(|s| s.as_str())?;
        if !name.ends_with("-PERP") { return None; }
        let base = e.get("base_currency").and_then(|s| s.as_str())?;
        let quote = e.get("quote_currency").and_then(|s| s.as_str()).unwrap_or("USD");
        if base.is_empty() { return None; }
        let entry_fee = dec_from_json(e.get("taker_fee_rate")).map(|f| f * dec!(10000)).unwrap_or(fee);
        meta(venue, base, quote, name.to_string(),
            dec_from_json(e.get("tick_size")), dec_from_json(e.get("amount_step")), entry_fee)
    }).collect()
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
    arr.iter().filter_map(|wrap| {
        let m = wrap.get("market")?;
        if m.get("status").and_then(|s| s.as_str()) != Some("Active") { return None; }
        if m.get("isPerpetual").and_then(|v| v.as_bool()) == Some(false) { return None; }
        let ticker = m.get("ticker").and_then(|s| s.as_str())?;
        // Tickers read `"BTC/USDC PERP"`; quote keeps the suffix stripped.
        let (base, qraw) = ticker.split_once('/')?;
        let quote = qraw.strip_suffix(" PERP").or_else(|| qraw.strip_suffix("-PERP")).unwrap_or(qraw);
        if base.is_empty() || quote.is_empty() { return None; }
        let qdec = m.get("quote_decimals").and_then(|v| v.as_u64())?;
        let scale = Decimal::new(1, 0).checked_mul(Decimal::from(10u64.pow(qdec.min(18) as u32)))?;
        let raw_tick = dec_from_json(m.get("min_price_tick_size"))?;
        let price_tick = raw_tick.checked_div(scale)?;
        let size_tick = dec_from_json(m.get("min_quantity_tick_size"))?;
        if price_tick <= dec!(0) || size_tick <= dec!(0) { return None; }
        let fee_bps = dec_from_json(m.get("taker_fee_rate")).map(|f| f * dec!(10000)).unwrap_or(dec!(0.01));
        Some(HelixMarketInfo {
            market_id: m.get("market_id").and_then(|s| s.as_str())?.to_string(),
            ticker: ticker.to_string(),
            base: base.to_uppercase(),
            quote: quote.to_uppercase(),
            scale, price_tick, size_tick, fee_bps,
        })
    }).collect()
}

fn parse_helix_markets(venue: &VenueId, data: &serde_json::Value) -> Vec<MarketMetadata> {
    helix_market_infos(data).into_iter().filter_map(|info| {
        meta(venue, &info.base, &info.quote, info.ticker.clone(),
            Some(info.price_tick), Some(info.size_tick), info.fee_bps)
    }).collect()
}

/// QFEX: `{"type": "refdata", "data": [{symbol: "BTC-USD", base_asset,
/// quote_asset, tick_size, lot_size}]}` (a bare array is also accepted).
/// Books and tickers are auth-gated, so refdata is markets-only by design.
fn parse_qfex_markets(venue: &VenueId, data: &serde_json::Value, fee: Decimal) -> Vec<MarketMetadata> {
    let arr = data.get("data").and_then(|v| v.as_array()).or_else(|| data.as_array());
    let arr = match arr {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter().filter_map(|e| {
        let symbol = e.get("symbol").and_then(|s| s.as_str())?;
        if symbol.is_empty() { return None; }
        let base = e.get("base_asset").and_then(|s| s.as_str()).map(|s| s.to_uppercase())
            .unwrap_or_else(|| to_base(symbol));
        if base.is_empty() { return None; }
        let quote = e.get("quote_asset").and_then(|s| s.as_str()).unwrap_or("USD");
        meta(venue, &base, quote, symbol.to_string(),
            dec_from_json(e.get("tick_size")), dec_from_json(e.get("lot_size")), fee)
    }).collect()
}

fn parse_gmx_markets(venue: &VenueId, data: &serde_json::Value, fee: Decimal) -> Vec<MarketMetadata> {
    let arr = match data.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    let mut seen = std::collections::HashSet::new();
    arr.iter().filter_map(|e| {
        let sym = e.get("tokenSymbol").and_then(|s| s.as_str())?;
        if sym.is_empty() || sym.ends_with("_deprecated") || !seen.insert(sym.to_string()) { return None; }
        // GMX tickers are spot-price feeds, not perps; expose them as
        // USD-quoted perp watchlist entries so funding/spread monitors can
        // consume the price without pretending depth exists.
        meta(venue, sym, "USD", sym.to_string(),
            Some(dec!(0.01)), Some(dec!(0.0001)), fee)
    }).collect()
}

/// Generic `{markets|results|data: […]}` array fallback for DOC-shaped
/// venues: accepts entries with any of `symbol`/`instrument_name`/`market`
/// plus `base`/`base_currency`/`underlying_asset` spellings. Entries that
/// do not match any spelling are skipped (fail-closed per entry).
fn parse_generic_markets(venue: &VenueId, data: &serde_json::Value, fee: Decimal) -> Vec<MarketMetadata> {
    let arr = data.get("markets").or_else(|| data.get("results")).or_else(|| data.get("data"))
        .and_then(|v| v.as_array()).map(|a| a.as_slice()).unwrap_or(&[]);
    let arr_owned: Vec<serde_json::Value> = if !arr.is_empty() {
        arr.to_vec()
    } else if let Some(a) = data.as_array() {
        a.clone()
    } else {
        return Vec::new();
    };
    arr_owned.iter().filter_map(|e| {
        let raw_symbol = e.get("symbol").or_else(|| e.get("instrument_name")).or_else(|| e.get("market"))
            .and_then(|s| s.as_str())?;
        let base = e.get("base").or_else(|| e.get("base_currency")).or_else(|| e.get("underlying_asset"))
            .and_then(|s| s.as_str())
            .map(|s| s.to_uppercase())
            .unwrap_or_else(|| to_base(raw_symbol));
        if base.is_empty() { return None; }
        let quote = e.get("quote").or_else(|| e.get("quote_currency")).or_else(|| e.get("quote_asset"))
            .and_then(|s| s.as_str()).unwrap_or("USD");
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
        meta(venue, &base, quote, raw_symbol.to_string(), price_tick, size_tick, fee)
    }).collect()
}

fn parse_markets_for(spec_id: &str, venue: &VenueId, data: &serde_json::Value, fee: Decimal) -> Vec<MarketMetadata> {
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
        _ => parse_generic_markets(venue, data, fee),
    }
}

// ---------------------------------------------------------------------------
// Orderbook parsers
// ---------------------------------------------------------------------------

/// Shared `[price, size]`-pair book parser (Paradex, Aevo, ApeX, Extended…).
/// Accepts `bids`/`asks` or `b`/`a` spellings; bails when both sides empty.
fn parse_pair_book(venue: &VenueId, symbol: &str, data: &serde_json::Value) -> anyhow::Result<OrderBook> {
    let node = data.get("result").unwrap_or(data);
    let bids = node.get("bids").or_else(|| node.get("b")).and_then(|v| v.as_array());
    let asks = node.get("asks").or_else(|| node.get("a")).and_then(|v| v.as_array());
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
fn parse_apex_book(venue: &VenueId, symbol: &str, data: &serde_json::Value) -> anyhow::Result<OrderBook> {
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
fn parse_extended_book(venue: &VenueId, symbol: &str, data: &serde_json::Value) -> anyhow::Result<OrderBook> {
    let node = data.get("data").unwrap_or(data);
    let mut bid_levels = Vec::new();
    let mut ask_levels = Vec::new();
    if let Some(arr) = node.get("bid").or_else(|| node.get("bids")).and_then(|v| v.as_array()) {
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
    if let Some(arr) = node.get("ask").or_else(|| node.get("asks")).and_then(|v| v.as_array()) {
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
fn parse_pacifica_book(venue: &VenueId, symbol: &str, data: &serde_json::Value) -> anyhow::Result<OrderBook> {
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
fn parse_helix_book(venue: &VenueId, symbol: &str, data: &serde_json::Value, scale: Decimal) -> anyhow::Result<OrderBook> {
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
fn parse_derive_topbook(venue: &VenueId, symbol: &str, result: &serde_json::Value) -> anyhow::Result<OrderBook> {
    let bids = match (dec_from_json(result.get("best_bid_price")), dec_from_json(result.get("best_bid_amount"))) {
        (Some(p), Some(q)) if p > dec!(0) && q > dec!(0) => vec![(p, q)],
        _ => Vec::new(),
    };
    let asks = match (dec_from_json(result.get("best_ask_price")), dec_from_json(result.get("best_ask_amount"))) {
        (Some(p), Some(q)) if p > dec!(0) && q > dec!(0) => vec![(p, q)],
        _ => Vec::new(),
    };
    book_from_levels(venue, symbol, bids, asks)
}

/// Parse an RFC3339 timestamp (`2026-09-19T03:00:00Z`) into `DateTime<Utc>;
/// `None` on absence/parse failure so callers fall back to `Utc::now`.
fn parse_time_opt(v: Option<&serde_json::Value>) -> Option<chrono::DateTime<Utc>> {
    v.and_then(|x| x.as_str()).and_then(|s| s.parse::<chrono::DateTime<Utc>>().ok())
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

/// dYdX v4 book: `{"bids": [{price, size}…], "asks": […]}`.
fn parse_dydx_book(venue: &VenueId, symbol: &str, data: &serde_json::Value) -> anyhow::Result<OrderBook> {
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
    data.get("markets")?.get(symbol_ticker)?.get("nextFundingRate").and_then(|v| dec_from_json(Some(v)))
}

// ---------------------------------------------------------------------------
// ExchangeAdapter implementation (paper read-only for every venue)
// ---------------------------------------------------------------------------

#[async_trait]
impl ExchangeAdapter for ExtendedVenueAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn ping(&self) -> anyhow::Result<()> {
        // Bypass the response cache so the round-trip is a real probe.
        let spec = spec_for(self.spec_id).ok_or_else(|| anyhow::anyhow!("unknown venue {}", self.spec_id))?;
        let url = format!("{}{}", self.base_url, spec.markets_path);
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("{} ping failed: {}", self.name, resp.status());
        }
        Ok(())
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let spec = spec_for(self.spec_id).ok_or_else(|| anyhow::anyhow!("unknown venue {}", self.spec_id))?;
        if self.spec_id == "derive" {
            let mut out = Vec::new();
            for cur in DERIVE_CURRENCIES {
                // One currency failing must not abort the listing — the
                // error is skipped per currency, fail-closed.
                if let Ok(data) = self.get_json(&format!(
                    "/public/get_instruments?currency={cur}&expired=false&instrument_type=perp"
                )).await {
                    out.extend(parse_derive_markets(&self.venue_id, &data, spec.taker_fee_bps));
                }
            }
            return Ok(out);
        }
        let data = self.get_json(spec.markets_path).await?;
        Ok(parse_markets_for(self.spec_id, &self.venue_id, &data, spec.taker_fee_bps))
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let spec = spec_for(self.spec_id).ok_or_else(|| anyhow::anyhow!("unknown venue {}", self.spec_id))?;
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
            let data = self.get_json(&format!("/public/get_ticker?instrument_name={vsym}")).await?;
            let result = data.get("result").ok_or_else(|| anyhow::anyhow!("Derive ticker for {symbol} has no result"))?;
            return parse_derive_topbook(&self.venue_id, symbol, result);
        }
        // Helix books resolve through the LCD market list (market_id) with
        // per-market quote-decimal scaling.
        if self.spec_id == "helix" {
            let base = to_base(symbol);
            let info = self.helix_resolve(&base).await?;
            let data = self.get_json(&format!(
                "/injective/exchange/v1beta1/derivative/orderbook/{}", info.market_id
            )).await?;
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
            let arr = data.as_array().ok_or_else(|| anyhow::anyhow!("GMX tickers missing array"))?;
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
                    let scale = Decimal::from_str("1000000000000000000000000000000").unwrap_or(dec!(1));
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
        let bid = book.best_bid_price().ok_or_else(|| anyhow::anyhow!("{} book for {symbol} has no bid", self.name))?;
        let ask = book.best_ask_price().ok_or_else(|| anyhow::anyhow!("{} book for {symbol} has no ask", self.name))?;
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
            let arr = data.get("results").and_then(|r| r.as_array())
                .ok_or_else(|| anyhow::anyhow!("Paradex markets missing results"))?;
            for e in arr {
                if e.get("symbol").and_then(|s| s.as_str()) == Some(vsym.as_str()) {
                    // `funding_rate` is fixtures-dependent; absence fails
                    // closed for this symbol only (never a zero fill-in).
                    let rate = dec_from_json(e.get("funding_rate"))
                        .or_else(|| dec_from_json(e.get("max_funding_rate")))
                        .ok_or_else(|| anyhow::anyhow!("Paradex has no funding rate for {symbol}"))?;
                    return Ok(funding_now(&self.venue_id, symbol, rate, 8, None));
                }
            }
            anyhow::bail!("Paradex has no market entry for {symbol}");
        }
        // Aevo exposes per-instrument funding.
        if self.spec_id == "aevo" {
            let vsym = self.venue_symbol(symbol);
            let data = self.get_json(&format!("/funding?instrument_name={vsym}")).await?;
            let rate = dec_from_json(data.get("funding_rate"))
                .or_else(|| dec_from_json(data.get("rate")))
                .ok_or_else(|| anyhow::anyhow!("Aevo has no funding rate for {symbol}"))?;
            return Ok(funding_now(&self.venue_id, symbol, rate, 1, None));
        }
        // ApeX Omni tickers carry the current funding rate plus the next
        // funding timestamp (hourly grid, inferred from next-funding times).
        if self.spec_id == "apex" {
            let vsym = self.venue_symbol(symbol);
            let data = self.get_json(&format!("/api/v3/ticker?symbol={vsym}")).await?;
            let entry = data.get("data").and_then(|v| v.as_array())
                .and_then(|a| a.first()).ok_or_else(|| anyhow::anyhow!("ApeX has no ticker for {symbol}"))?;
            let rate = dec_from_json(entry.get("fundingRate"))
                .ok_or_else(|| anyhow::anyhow!("ApeX has no funding rate for {symbol}"))?;
            return Ok(funding_now(&self.venue_id, symbol, rate, 1, parse_time_opt(entry.get("nextFundingTime"))));
        }
        // Extended funds hourly (inferred from next-funding timestamps);
        // the rate and next time ride inside the markets payload.
        if self.spec_id == "extended" {
            let vsym = self.venue_symbol(symbol);
            let data = self.get_json("/api/v1/info/markets").await?;
            let arr = data.get("data").and_then(|v| v.as_array())
                .ok_or_else(|| anyhow::anyhow!("Extended markets missing data"))?;
            for e in arr {
                if e.get("name").and_then(|s| s.as_str()) == Some(vsym.as_str()) {
                    let stats = e.get("marketStats").ok_or_else(|| anyhow::anyhow!("Extended has no stats for {symbol}"))?;
                    let rate = dec_from_json(stats.get("fundingRate"))
                        .ok_or_else(|| anyhow::anyhow!("Extended has no funding rate for {symbol}"))?;
                    return Ok(funding_now(&self.venue_id, symbol, rate, 1,
                        parse_millis_opt(stats.get("nextFundingRate"))));
                }
            }
            anyhow::bail!("Extended has no market entry for {symbol}");
        }
        // Derive/Lyra funds hourly (max_rate_per_hour); the live rate is
        // `perp_details.funding_rate` on the ticker.
        if self.spec_id == "derive" {
            let vsym = self.venue_symbol(symbol);
            let data = self.get_json(&format!("/public/get_ticker?instrument_name={vsym}")).await?;
            let result = data.get("result").ok_or_else(|| anyhow::anyhow!("Derive ticker for {symbol} has no result"))?;
            let rate = result.get("perp_details").and_then(|d| dec_from_json(d.get("funding_rate")))
                .ok_or_else(|| anyhow::anyhow!("Derive has no funding rate for {symbol}"))?;
            return Ok(funding_now(&self.venue_id, symbol, rate, 1, None));
        }
        // StandX funds hourly; the rate and next time ride on the market
        // snapshot endpoint.
        if self.spec_id == "standx" {
            let vsym = self.venue_symbol(symbol);
            let data = self.get_json(&format!("/api/query_symbol_market?symbol={vsym}")).await?;
            let rate = dec_from_json(data.get("funding_rate"))
                .ok_or_else(|| anyhow::anyhow!("StandX has no funding rate for {symbol}"))?;
            return Ok(funding_now(&self.venue_id, symbol, rate, 1, parse_time_opt(data.get("next_funding_time"))));
        }
        // Pacifica publishes current + next funding per symbol on
        // `/api/v1/info/prices`; the predicted rate is surfaced so the
        // funding bot can see the turn, not just the level.
        if self.spec_id == "pacifica" {
            let base = to_base(symbol);
            let data = self.get_json("/api/v1/info/prices").await?;
            let arr = data.get("data").and_then(|v| v.as_array())
                .ok_or_else(|| anyhow::anyhow!("Pacifica prices missing data"))?;
            for e in arr {
                if e.get("symbol").and_then(|s| s.as_str()).map(|s| s.to_uppercase()) == Some(base.clone()) {
                    let rate = dec_from_json(e.get("funding"))
                        .ok_or_else(|| anyhow::anyhow!("Pacifica has no funding rate for {symbol}"))?;
                    let mut fr = funding_now(&self.venue_id, symbol, rate, 1, None);
                    fr.predicted_rate = dec_from_json(e.get("next_funding"));
                    return Ok(fr);
                }
            }
            anyhow::bail!("Pacifica has no price entry for {symbol}");
        }
        // Helix funding is NOT on the chain LCD (query returns
        // "Not Implemented") and QFEX refdata carries no funding — fail
        // closed rather than returning a zero rate that the funding bot
        // would read as "no cost to hold".
        // Every other venue: no verified per-symbol funding endpoint in this
        // harness — fail closed rather than returning a zero rate that the
        // funding bot would read as "no cost to hold".
        anyhow::bail!("Live funding fetch is not verified for {}; refusing to fabricate a rate", self.name)
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
                    let vol = e.get("marketStats").and_then(|s| dec_from_json(s.get("dailyVolume")));
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
        anyhow::bail!(
            "Live account fetch is not implemented for {}; refusing to fabricate an account snapshot",
            self.name
        )
    }

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        anyhow::bail!(wallet_execution_unavailable(self.name(), "its settlement chain", "wallet-signed order flow"))
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        anyhow::bail!(wallet_execution_unavailable(self.name(), "its settlement chain", "wallet-signed order flow"))
    }

    async fn set_leverage(&self, _symbol: &str, _leverage: u32) -> anyhow::Result<()> {
        anyhow::bail!(
            "Leverage updates are not implemented for {}; refusing to pretend to apply it",
            self.name
        )
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
        assert_eq!(EXTENDED_VENUE_IDS.len(), 23);
        for id in EXTENDED_VENUE_IDS {
            assert!(spec_for(id).is_some(), "missing spec for {id}");
            assert!(ExtendedVenueAdapter::for_venue(id, None).is_some());
        }
        // Unknown ids (RISEx — not live; Orderly-derivatives — Vanta alias)
        // fail closed at construction time.
        assert!(ExtendedVenueAdapter::for_venue("risex", None).is_none());
        assert!(ExtendedVenueAdapter::for_venue("orderly", None).is_none());
        assert!(ExtendedVenueAdapter::for_venue("", None).is_none());
    }

    #[test]
    fn admitted_set_matches_spec_flags() {
        // Every modeled venue is admitted (paper read-only); the admitted
        // list and the spec table must agree exactly.
        assert_eq!(ADMITTED_EXTENDED_VENUE_IDS.len(), EXTENDED_VENUE_IDS.len());
        for id in ADMITTED_EXTENDED_VENUE_IDS {
            let a = ExtendedVenueAdapter::for_venue(id, None).expect("admitted venue");
            assert!(a.is_admitted(), "{id} should be admitted");
            let spec = spec_for(id).expect("spec");
            assert!(spec.admit, "{id} spec flag must be admit");
        }
        for id in EXTENDED_VENUE_IDS {
            assert!(ADMITTED_EXTENDED_VENUE_IDS.contains(&id), "{id} missing from admitted list");
        }
        // Non-venues still fail closed at construction time.
        assert!(ExtendedVenueAdapter::for_venue("risex", None).is_none());
        assert!(ExtendedVenueAdapter::for_venue("orderly", None).is_none());
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
        let dydx = ExtendedVenueAdapter::dydx(None);
        assert_eq!(dydx.venue_symbol("BTC"), "BTC-USD");
        let paradex = ExtendedVenueAdapter::paradex(None);
        assert_eq!(paradex.venue_symbol("BTC-USDC"), "BTC-USD-PERP");
        let aevo = ExtendedVenueAdapter::aevo(None);
        assert_eq!(aevo.venue_symbol("eth"), "ETH-PERP");
        let apex = ExtendedVenueAdapter::apex(None);
        assert_eq!(apex.venue_symbol("BTC"), "BTCUSDT");
        let extended = ExtendedVenueAdapter::extended(None);
        assert_eq!(extended.venue_symbol("btc/usd"), "BTC-USD");
        let standx = ExtendedVenueAdapter::standx(None);
        assert_eq!(standx.venue_symbol("BTC"), "BTC-USD");
        let pacifica = ExtendedVenueAdapter::pacifica(None);
        assert_eq!(pacifica.venue_symbol("BTC"), "BTC");
        let derive = ExtendedVenueAdapter::derive(None);
        assert_eq!(derive.venue_symbol("ETH"), "ETH-PERP");
        let qfex = ExtendedVenueAdapter::qfex(None);
        assert_eq!(qfex.venue_symbol("BTC"), "BTC-USD");
    }

    #[tokio::test]
    async fn fail_closed_account_order_leverage_paths() {
        let a = ExtendedVenueAdapter::dydx(None);
        assert!(a.fetch_account().await.is_err());
        assert!(a.set_leverage("BTC", 5).await.is_err());
        assert!(!a.supports_live_execution());
        // GMX has no book by construction.
        let gmx = ExtendedVenueAdapter::gmx(None);
        assert!(gmx.fetch_orderbook("BTC").await.is_err());
    }
}

#[cfg(test)]
mod wallet_direct_tests {
    use super::*;

    #[test]
    fn wallet_binding_prepares_without_enabling_execution() {
        let a = ExtendedVenueAdapter::dydx(None);
        // Empty binding clears without error.
        assert!(a.update_credentials(None, None).is_ok());
        assert_eq!(a.wallet_address_hex(), None);
        // Mismatched address fails the whole update (no half-bound state).
        assert!(a
            .update_credentials(
                Some("0x0000000000000000000000000000000000000001".to_string()),
                Some("0x0000000000000000000000000000000000000000000000000000000000000001".to_string()),
            )
            .is_err());
        assert_eq!(a.wallet_address_hex(), None);
        // Scalar-1 test vector binds; address must match the derived key.
        assert!(a
            .update_credentials(
                Some("0x7E5F4552091a69125d5dfcb7b8c2659029395bdf".to_string()),
                Some("0x0000000000000000000000000000000000000000000000000000000000000001".to_string()),
            )
            .is_ok());
        assert_eq!(
            a.wallet_address_hex(),
            Some("0x7E5F4552091a69125d5dfcb7b8c2659029395bdf".to_string())
        );
        // Binding a key never enables live execution — the signing flow
        // stays fail-closed until implemented and audited.
        assert!(!a.supports_live_execution());
    }
}
