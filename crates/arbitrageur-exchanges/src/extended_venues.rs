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
//! # Verification status (2026-09-19, this sandbox)
//! - LIVE-VERIFIED (curl returned real payloads): dYdX v4, Paradex, Aevo,
//!   GMX V2 (prices/tickers only — the GLV AMM has no orderbook by design).
//! - DOC-SHAPED (public docs + API references; admitted paper-only per
//!   operator request — a harness sweep from full egress should still confirm
//!   each shape): ApeX Omni, Extended, Derive, StandX, Helix/Injective, Vest,
//!   Antarctic, EVEDEX, BULK, QFEX, Ostium, SoDEX, AlphaX, Pacifica, Nado,
//!   IO Trader, Niza.fun, Astros, Gate DEX.
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
    let mut s = symbol.trim().to_uppercase().replace('/', "-").replace(':', "-");
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
            base_url: "https://api.apex.exchange", markets_path: "/api/v3/symbols",
            taker_fee_bps: fee("2.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: false,
        },
        "extended" => VenueApiSpec {
            id: "extended", name: "Extended",
            base_url: "https://api.extended.exchange", markets_path: "/api/v1/info/markets",
            taker_fee_bps: fee("2.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: false,
        },
        "derive" => VenueApiSpec {
            id: "derive", name: "Derive",
            base_url: "https://api.derive.xyz", markets_path: "/instruments?instrument_type=perp",
            taker_fee_bps: fee("2.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: false,
        },
        "standx" => VenueApiSpec {
            id: "standx", name: "StandX",
            base_url: "https://perps.standx.com", markets_path: "/api/v1/markets",
            taker_fee_bps: fee("3.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: false,
        },
        "helix" => VenueApiSpec {
            id: "helix", name: "Helix",
            base_url: "https://sentry.chain.grpc-web.injective.exchange",
            markets_path: "/api/chuck/v1/derivative/markets",
            taker_fee_bps: fee("2.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: false,
        },
        // ---- DOC-SHAPED, admitted paper-only per operator request ----
        // Shapes come from public docs; a harness sweep from full egress
        // should still confirm each payload shape. Parsers fail closed per
        // entry, so a wrong doc guess degrades to venue errors, never bad
        // data. High-caution venues (Ostium, Pacifica, AlphaX, QFEX) stay
        // paper-only regardless — see docs/EXCHANGES.md section 10.
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
            base_url: "https://api.qfex.com", markets_path: "/v1/markets",
            taker_fee_bps: fee("4.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: false,
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
            base_url: "https://api.pacifica.fi", markets_path: "/api/v1/markets",
            taker_fee_bps: fee("2.0"), book_kind: BookKind::PairLevels,
            admit: true, verified: false,
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
}

impl ExtendedVenueAdapter {
    /// Build an adapter for a venue id in [`EXTENDED_VENUE_IDS`].
    /// Returns `None` for anything else (RISEx, Orderly-derivatives, and
    /// unknown strings fail closed here — they are documented, not modeled).
    pub fn for_venue(id: &str, base_url: Option<String>) -> Option<Self> {
        let spec = spec_for(id.trim().to_lowercase().as_str())?;
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        Some(Self {
            venue_id: VenueId::new(spec.id),
            name: spec.name.to_string(),
            spec_id: spec.id,
            base_url: base_url.unwrap_or_else(|| spec.base_url.to_string()),
            client,
            rate_limiter: AccountRequestCoordinator::new(spec.id, 100, 2000, 500, 10000),
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
            "apex" => format!("{base}-USDT"),
            "derive" => format!("{base}-PERP"),
            _ => format!("{base}-USD-PERP"),
        }
    }

    /// Orderbook path for a venue-native symbol.
    fn book_path(&self, venue_symbol: &str) -> String {
        match self.spec_id {
            "dydx" => format!("/v4/orderbooks/perpetualMarket/{venue_symbol}"),
            "paradex" => format!("/v1/orderbook/{venue_symbol}?depth=15"),
            "aevo" => format!("/orderbook?instrument_name={venue_symbol}"),
            "apex" => format!("/api/v3/depth?symbol={venue_symbol}&limit=50"),
            "extended" => format!("/api/v1/info/orderbook?market={venue_symbol}"),
            "derive" => format!("/orderbook?instrument_name={venue_symbol}"),
            "standx" => format!("/api/v1/orderbook?symbol={venue_symbol}"),
            "helix" => format!("/api/chuck/v1/derivative/orderbook?marketId={venue_symbol}"),
            _ => format!("/v1/orderbook?symbol={venue_symbol}"),
        }
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
        "aevo" | "derive" => parse_aevo_markets(venue, data, fee),
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
        let vsym = self.venue_symbol(symbol);
        let data = self.get_json(&self.book_path(&vsym)).await?;
        match self.spec_id {
            "dydx" => parse_dydx_book(&self.venue_id, symbol, &data),
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
        Ok(out)
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        anyhow::bail!(
            "Live account fetch is not implemented for {}; refusing to fabricate an account snapshot",
            self.name
        )
    }

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        anyhow::bail!(
            "Live order execution is not implemented for {}; refusing to fabricate a fill",
            self.name
        )
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        anyhow::bail!("Live order cancellation is not implemented for {}", self.name)
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
    fn venue_symbol_mapping_is_explicit() {
        let dydx = ExtendedVenueAdapter::dydx(None);
        assert_eq!(dydx.venue_symbol("BTC"), "BTC-USD");
        let paradex = ExtendedVenueAdapter::paradex(None);
        assert_eq!(paradex.venue_symbol("BTC-USDC"), "BTC-USD-PERP");
        let aevo = ExtendedVenueAdapter::aevo(None);
        assert_eq!(aevo.venue_symbol("eth"), "ETH-PERP");
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
