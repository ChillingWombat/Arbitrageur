//! DEX perpetual-venue adapters — spec-driven, fail-closed.
//!
//! # What lives here
//! One [`DexPerpAdapter`] struct serves many perp venues through a
//! per-venue [`VenueApiSpec`] table. Every venue shares verified
//! market-data paths (markets / books / tickers / funding). The venues in
//! [`LIVE_TRADING_VENUE_IDS`] additionally implement the authenticated
//! order paths with venue-native signing (see "Live trading" below);
//! every other venue's account / order / leverage paths fail closed with
//! an explanatory error — never a fabricated snapshot or fill.
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
//! - **Vest** — delegate-signer (`X-API-KEY` + `xrestservermm` routing)
//!   or primary EVM key self-registering a ~7-day signer.
//! - **QFEX** — API-key HMAC REST reads + authenticated `/user/trade` WS
//!   order entry.
//! - **Nado** — Vertex-style sequencer on Ink L2; EIP-712 `Order`/
//!   `Cancellation` by main or linked signer; gateway + archive truth.
//! - **EVEDEX** — SIWE → JWT (`auth-api.evedex.com`) + per-order EIP-712
//!   signatures on Arbitrum.
//! - **Paradex** — SNIP-12 Stark-curve order signatures; L2 key derived
//!   from the EVM wallet via the `paradex` crate.
//! - **edgeX** — `X-edgeX-*` HMAC api-key trio + EIP-712
//!   `LimitOrderParams` signatures on the Perp V2 host.
//! - **Extended** — `X-Api-Key` + SNIP-12 Poseidon `Order` signatures
//!   (StarkEx engine; ported from official X10 `fast_stark_crypto`).
//! - **dYdX** — Cosmos-chain `MsgPlaceOrder`/`MsgCancelOrder` signed and
//!   broadcast by the official `dydx` gRPC client; the credential is the
//!   account's 24-word BIP-39 mnemonic; indexer `/v4/orders` is truth.
//! - **Helix / Injective** — `MsgCreateDerivative{Limit,Market}Order` /
//!   `MsgCancelDerivativeOrder` Cosmos txs: keccak256(`SignDoc`) + compact
//!   secp256k1 sig under the chain's `ethsecp256k1` pubkey, broadcast over
//!   the Injective LCD; the credential is the account's own EVM private
//!   key (`inj1…` derived); indexer gRPC `OrdersHistory` is order truth.
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
//!   data): Antarctic, BULK
//!   (`api.bulk.exchange` serves an unrelated landing page), Ostium
//!   (onchain Arbitrum reads — no public REST; SDK needs key+RPC),
//!   SoDEX + AlphaX (parked `/lander` domains), IO Trader
//!   (parked page), Niza.fun (no futures product
//!   found), Astros (perp frontend live, no public REST found — DeepBook
//!   onchain), Gate DEX (no such venue found).
//! - REBRANDED/MIGRATED: Vest Exchange → Vest Markets — every
//!   `*.vest.exchange` backend host (api/serverprod/wsprod) is gone
//!   (Cloudflare origin-DNS dead); the venue now lives at
//!   `server-prod.hz.vestmarkets.com/v2` with docs at
//!   `docs.vestmarkets.com` (verified live 2026-09-23 — exchangeInfo,
//!   ticker/latest, depth all serve real payloads incl. equity/forex
//!   perps). Nado is a live venue on Ink L2 with a full documented
//!   gateway API (docs.nado.xyz) — its earlier "no public API" note
//!   predates the docs site going up.
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
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Shared parsing helpers (fail-closed per entry, never abort listings)
// ---------------------------------------------------------------------------

/// Parse a decimal from either a JSON string or number. Returns `None` for
/// anything else (null, bool, object) so callers fail closed per entry.
pub(crate) fn dec_from_json(v: Option<&serde_json::Value>) -> Option<Decimal> {
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
    for suffix in [
        "-PERP", "-PERPS", "-USDC", "-USDT0", "-USDT", "-USD", "-USDP",
    ] {
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
        // Vest Markets (rebranded from Vest Exchange): verified live at
        // server-prod.hz.vestmarkets.com — 544 symbols incl. equity/forex
        // perps. API-transaction fee is a flat 1bp (crypto takerFee); the
        // per-symbol takerFee from exchangeInfo overrides in the parser.
        "vest" => VenueApiSpec {
            id: "vest",
            name: "Vest",
            base_url: "https://server-prod.hz.vestmarkets.com",
            markets_path: "/v2/exchangeInfo",
            taker_fee_bps: fee("1.0"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: true,
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
        // Verified live: exchange-api.evedex.com serves instruments,
        // depth, fees and the authed order surface (SIWE→JWT + EIP-712
        // order signatures; auth-api.evedex.com issues the tokens).
        "evedex" => VenueApiSpec {
            id: "evedex",
            name: "EVEDEX",
            base_url: "https://exchange-api.evedex.com",
            markets_path: "/api/market/instrument?fields=metrics",
            taker_fee_bps: fee("4.5"),
            book_kind: BookKind::ObjectLevels,
            admit: true,
            verified: true,
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
        // edgeX Perp V2 (`edgex-prod-v2` host — the SDK's documented
        // trading deployment; `pro.edgex.exchange` serves only the v1
        // public surface and 404s on every /api/v2 path). Contracts are
        // `{BASE}USDC` on numeric ids (30000001+); the EIP-712 order
        // domain is `EdgeX/1/{nativeChainId}/{global.contractAddress}`
        // and private REST auth is HMAC over `ts+method+path+canonical`.
        "edgex" => VenueApiSpec {
            id: "edgex",
            name: "edgeX",
            base_url: "https://edgex-prod-v2.edgex.exchange",
            markets_path: "/api/v2/public/meta/getMetaData",
            taker_fee_bps: fee("4.5"),
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
            // Unified gateway host (verified 2026-09-23): perp symbols via
            // the symbols query; every request needs Accept-Encoding:
            // gzip|br|deflate or the edge rejects with a compression error.
            base_url: "https://api.prod.nado.xyz",
            markets_path: "/gateway/v1/query?type=symbols&product_type=perp",
            taker_fee_bps: fee("3.5"),
            book_kind: BookKind::PairLevels,
            admit: true,
            verified: true,
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
pub const LIVE_TRADING_VENUE_IDS: [&str; 15] = [
    "aevo",
    "derive",
    "dydx",
    "edgex",
    "evedex",
    "extended",
    "grvt",
    "helix",
    "nado",
    "ondoperps",
    "pacifica",
    "paradex",
    "qfex",
    "standx",
    "vest",
];

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
    /// edgeX API passphrase (sent as `X-edgeX-Passphrase`; part of the
    /// HMAC credential trio, secret-material).
    passphrase: Option<zeroize::Zeroizing<String>>,
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
    /// QFEX trade-socket session (persisted across calls; order entry is
    /// WS-only). `None` for every other venue — constructed lazily.
    qfex_ws: RwLock<Option<Arc<QfexWsState>>>,
    /// Nado `symbol → product_id` map (from the `symbols` gateway query),
    /// cached for the process lifetime — product ids are static.
    nado_products: RwLock<Option<HashMap<String, u32>>>,
    /// EVEDEX SIWE session: `{accessToken, refreshToken}` JWT pair from
    /// `/auth/user/sign-up`. Access tokens are short-lived; on 401 the
    /// refresh token is exchanged once, then a full re-login runs.
    evedex_tokens: RwLock<Option<EvedexTokens>>,
    /// Paradex Stark L2 private key (felt hex) derived at credential-load
    /// from the EVM key via the venue's "STARK Key" EIP-712 derivation.
    paradex_l2_key: RwLock<Option<zeroize::Zeroizing<String>>>,
    /// Lazily-built Paradex SDK client — construction fetches
    /// `/v1/system/config` so it must run in async context, not in
    /// `update_credentials`.
    paradex_client: RwLock<Option<Arc<paradex::rest::Client>>>,
    /// Serializes the lazy `paradex_client` build (async guard).
    paradex_build: tokio::sync::Mutex<()>,
    /// edgeX v2 metadata cache: per-contract ids/resolutions plus the
    /// EIP-712 domain (nativeChainId + global.contractAddress) needed for
    /// order signing. Refetched when a contract id lookup misses.
    edgex_meta: RwLock<Option<Arc<EdgexMeta>>>,
    /// Extended Stark private key (felt hex) — signs SNIP-12 Poseidon
    /// `Order` messages. Stark curve, deliberately separate from the EVM
    /// `wallet` field.
    stark_key: RwLock<Option<zeroize::Zeroizing<String>>>,
    /// Extended per-market settlement config from `/info/markets`
    /// `l2Config` (asset ids + resolutions for StarkAmount packing).
    /// Refetched on a market-name miss.
    extended_meta: RwLock<Option<Arc<HashMap<String, ExtendedL2>>>>,
    /// dYdX v4 account mnemonic — the venue derives the Cosmos signing
    /// key via m/44'/118'/0'/0/0, so a raw hex key cannot be accepted.
    dydx_mnemonic: RwLock<Option<zeroize::Zeroizing<String>>>,
    /// Lazily-built dYdX gRPC session — `NodeClient::connect` and the
    /// account-number/sequence query are async, so construction happens
    /// on first trading call rather than in `update_credentials`.
    dydx_session: RwLock<Option<Arc<tokio::sync::Mutex<DydxSession>>>>,
    /// Serializes the lazy `dydx_session` build.
    dydx_build: tokio::sync::Mutex<()>,
    /// Cached `/v4/perpetualMarkets` order params (clobPairId, atomic
    /// resolution, quantum conversion, tick/step) keyed by venue symbol.
    dydx_markets: RwLock<Option<Arc<HashMap<String, dydx::node::OrderMarketParams>>>>,
    /// Lazily-connected Injective indexer gRPC channel (order/position/
    /// balance truth — the chain LCD does not expose those queries).
    helix_channel: RwLock<Option<tonic::transport::Channel>>,
    /// Serializes the lazy `helix_channel` connect.
    helix_build: tokio::sync::Mutex<()>,
}

/// EVEDEX JWT pair returned by `/auth/user/sign-up` and `/auth/refresh`.
/// Reserved for the (unimplemented) SIWE session flow — evedex is
/// market-data-only until then.
#[allow(dead_code)]
struct EvedexTokens {
    access: String,
    refresh: Option<String>,
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
            qfex_ws: RwLock::new(None),
            nado_products: RwLock::new(None),
            evedex_tokens: RwLock::new(None),
            paradex_l2_key: RwLock::new(None),
            paradex_client: RwLock::new(None),
            paradex_build: tokio::sync::Mutex::new(()),
            edgex_meta: RwLock::new(None),
            stark_key: RwLock::new(None),
            extended_meta: RwLock::new(None),
            dydx_mnemonic: RwLock::new(None),
            dydx_session: RwLock::new(None),
            dydx_build: tokio::sync::Mutex::new(()),
            dydx_markets: RwLock::new(None),
            helix_channel: RwLock::new(None),
            helix_build: tokio::sync::Mutex::new(()),
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
    /// - `api_key`/`api_secret`: Aevo (header pair), GRVT (api_key login),
    ///   Vest `X-API-KEY` (from POST /register).
    /// - `account_id`: GRVT sub-account id (overrides the login echo),
    ///   Vest `accGroup` (restserver{group} routing).
    /// - `wallet_address`: funding/account wallet (EVM 0x for Derive,
    ///   Ondo, Aevo maker, StandX login, Vest primary account; Solana
    ///   base58 for Pacifica).
    /// - `private_key`: EVM hex for Derive/Ondo/Aevo/StandX login and
    ///   Vest order signing (delegate signer or the primary key, which
    ///   self-registers a 7-day signer on first use); base58 Ed25519
    ///   secret for Pacifica ops.
    ///
    /// Keys are parsed into zeroizing signers immediately; raw strings
    /// are never retained. An invalid key fails the whole update so a
    /// half-bound state is impossible.
    #[allow(clippy::too_many_arguments)]
    pub fn update_credentials(
        &self,
        api_key: Option<String>,
        api_secret: Option<String>,
        passphrase: Option<String>,
        account_id: Option<String>,
        wallet_address: Option<String>,
        private_key: Option<String>,
    ) -> anyhow::Result<()> {
        let norm = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let api_key = norm(api_key);
        let api_secret = norm(api_secret);
        let passphrase = norm(passphrase);
        let account_id = norm(account_id);
        let wallet_address = norm(wallet_address);
        let private_key = norm(private_key);

        // EVM venues validate 0x addresses; Pacifica/StandX account keys
        // are base58 pubkeys — only length-sanity-checked. Extended's
        // "wallet" field carries the Stark public key (0x felt hex).
        if let Some(a) = wallet_address.as_deref() {
            match self.spec_id {
                "pacifica" | "standx" => {
                    if a.starts_with("0x") && self.spec_id == "pacifica" {
                        anyhow::bail!("Pacifica account must be a Solana base58 address, not 0x");
                    }
                }
                "extended" => {
                    if starknet_crypto::Felt::from_hex(a).is_err() {
                        anyhow::bail!("Extended Stark public key is not valid 0x hex");
                    }
                }
                "dydx" => {
                    // Cosmos bech32 account address (dydx1...).
                    if !a.starts_with("dydx1") {
                        anyhow::bail!("dYdX account address must be a dydx1… bech32 string");
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
        let mut stark: Option<zeroize::Zeroizing<String>> = None;
        let mut mnemonic: Option<zeroize::Zeroizing<String>> = None;
        if let Some(key) = private_key.as_deref() {
            match self.spec_id {
                "dydx" => {
                    // dYdX v4 keys are 24-word BIP-39 mnemonics — the
                    // venue derives m/44'/118'/0'/0/0 (bip32's phrase
                    // parser only accepts the 24-word length).
                    if key.split_whitespace().count() != 24 {
                        anyhow::bail!(
                            "dYdX requires the account's 24-word mnemonic in the private-key field"
                        );
                    }
                    let wallet = dydx::node::Wallet::from_mnemonic(key)
                        .map_err(|e| anyhow::anyhow!("dYdX mnemonic rejected: {e}"))?;
                    // If an address was configured it must match the
                    // derived account — otherwise trades would settle to a
                    // different wallet silently.
                    if let Some(a) = wallet_address.as_deref() {
                        let derived = wallet.account_offline(0)?.address().to_string();
                        if !derived.eq_ignore_ascii_case(a) {
                            anyhow::bail!(
                                "dYdX wallet address does not match the mnemonic-derived account"
                            );
                        }
                    }
                    mnemonic = Some(zeroize::Zeroizing::new(key.to_string()));
                }
                "extended" => {
                    let felt = starknet_crypto::Felt::from_hex(key).map_err(|_| {
                        anyhow::anyhow!("Extended Stark private key is not valid 0x hex")
                    })?;
                    if felt == starknet_crypto::Felt::ZERO {
                        anyhow::bail!("Extended Stark private key must be non-zero");
                    }
                    // The Stark public key field, when present, must match
                    // the derived key — a mismatched pair would sign into a
                    // different account silently.
                    if let Some(a) = wallet_address.as_deref() {
                        let derived = starknet_crypto::get_public_key(&felt);
                        let claimed = starknet_crypto::Felt::from_hex(a).unwrap_or_default();
                        if derived != claimed {
                            anyhow::bail!(
                                "Extended Stark public key does not match the private key"
                            );
                        }
                    }
                    stark = Some(zeroize::Zeroizing::new(felt.to_hex_string()));
                }
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
            c.passphrase = passphrase.map(zeroize::Zeroizing::new);
            c.account_id = account_id;
            c.wallet_address = wallet_address.clone();
        }
        // Paradex: derive the Stark L2 private key from the EVM key now
        // (synchronous EIP-712 "STARK Key" derivation + key grind) — the
        // API client itself builds lazily since its constructor fetches
        // system config.
        let mut l2: Option<zeroize::Zeroizing<String>> = None;
        if self.spec_id == "paradex" {
            if let Some(key) = private_key.as_deref() {
                let signer = alloy_signer_local::PrivateKeySigner::from_str(key)
                    .map_err(|e| anyhow::anyhow!("Paradex EVM private key invalid: {e}"))?;
                let felt = paradex::onboarding::get_paradex_private_key(&signer);
                l2 = Some(zeroize::Zeroizing::new(felt.to_hex_string()));
            }
        }
        *self.wallet_address.write() = wallet_address;
        *self.wallet.write() = evm;
        *self.ed25519.write() = ed;
        *self.stark_key.write() = stark;
        *self.dydx_mnemonic.write() = mnemonic;
        *self.dydx_session.write() = None;
        *self.paradex_l2_key.write() = l2;
        // Credentials changed — any cached session token is stale.
        *self.session.write() = None;
        *self.evedex_tokens.write() = None;
        *self.paradex_client.write() = None;
        // Drop the QFEX socket too — it authenticates with the old keys.
        *self.qfex_ws.write() = None;
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
        self.rate_limiter.record_success_path(path, text);
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
        self.rate_limiter.acquire_signed().await?;
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
        self.rate_limiter.acquire_signed().await?;
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
        self.rate_limiter.acquire_signed().await?;
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
            // Nado symbols keys are `{BASE}-PERP` (books/orders resolve
            // `{BASE}-PERP_USDT0` and a numeric product_id downstream).
            "nado" => format!("{base}-PERP"),
            // edgeX v2 contracts are `{BASE}USDC` ("BTCUSDC"); order
            // paths resolve the numeric contractId via metadata, not this.
            "edgex" => format!("{base}USDC"),
            // EVEDEX instruments are `{BASE}USD` ("BTCUSD") — the `name`
            // field of /api/market/instrument.
            "evedex" => format!("{base}USD"),
            // GRVT perpetual instruments are `{BASE}_USDT_Perp`.
            "grvt" => format!("{base}_USDT_Perp"),
            // Ondo perp markets are `{BASE}-USD.P`.
            "ondoperps" => format!("{base}-USD.P"),
            // Vest crypto perps are `{BASE}-PERP`; equity/index/forex
            // perps carry the USD leg inside the base (`AAPL-USD-PERP`),
            // which our canonical symbol already spells as `{T}-USD`.
            "vest" => {
                let up = symbol.trim().to_uppercase();
                if up.ends_with("-USD") {
                    format!("{up}-PERP")
                } else {
                    format!("{base}-PERP")
                }
            }
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
            // Verified: `{bids: [[p, s]], asks: [[p, s]]}` (`BTC-PERP`).
            // Vest is a taker-only AMM — depth levels are the pool's
            // quote bounds, thin by construction; never padded.
            "vest" => format!("/v2/depth?symbol={venue_symbol}&limit=15"),
            // Verified: `{bids: [[price, qty]], asks: [[price, qty]],
            // timestamp}` — numeric pairs, keyed by `{SYMBOL}_USDT0`.
            "nado" => {
                format!("/gateway/v2/orderbook?ticker_id={venue_symbol}_USDT0&depth=25")
            }
            // Verified: `{t, asks: [{price, quantity}], bids: [...]}`.
            "evedex" => format!("/api/market/{venue_symbol}/deep"),
            _ => format!("/v1/orderbook?symbol={venue_symbol}"),
        }
    }

    /// Resolve a caller-side base asset (e.g. `BTC`) to its active Helix
    /// LCD market, preferring the USDC quote. Fails closed when the base
    /// has no active perp.
    async fn helix_resolve(&self, base: &str) -> anyhow::Result<HelixMarketInfo> {
        // Callers pass whatever `to_base` produced — which keeps one
        // quote leg ("BTC-USD"); take the leftmost segment as the asset.
        let base = base.split(['-', '/']).next().unwrap_or(base).to_uppercase();
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
            // v2 contract names are `{BASE}USDC` ("BTCUSDC"); the retired
            // v1 surface used `{BASE}USD` — admit both, quote the suffix.
            let (base, quote) = if let Some(b) = name.strip_suffix("USDC") {
                (b, "USDC")
            } else if let Some(b) = name.strip_suffix("USD") {
                (b, "USD")
            } else {
                return None;
            };
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
            if e.get("display").and_then(|v| v.as_bool()) == Some(false)
                || e.get("enableDisplay").and_then(|v| v.as_bool()) == Some(false)
            {
                return None;
            }
            if e.get("enableTrade").and_then(|v| v.as_bool()) == Some(false)
                || e.get("enableOpenPosition").and_then(|v| v.as_bool()) == Some(false)
            {
                return None;
            }
            meta(
                venue,
                base,
                quote,
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
    pub quote_decimals: u32,
    pub price_tick: Decimal,
    pub size_tick: Decimal,
    pub fee_bps: Decimal,
    pub initial_margin_ratio: Decimal,
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
            let initial_margin_ratio = dec_from_json(m.get("initial_margin_ratio"))?;
            if initial_margin_ratio <= dec!(0) || initial_margin_ratio > dec!(1) {
                return None;
            }
            Some(HelixMarketInfo {
                market_id: m.get("market_id").and_then(|s| s.as_str())?.to_string(),
                ticker: ticker.to_string(),
                base: base.to_uppercase(),
                quote: quote.to_uppercase(),
                scale,
                quote_decimals: qdec.min(18) as u32,
                price_tick,
                size_tick,
                fee_bps,
                initial_margin_ratio,
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

/// Vest Markets `/v2/exchangeInfo`:
/// `{exchange: {lp, insurance, collateralDecimals}, symbols: [
/// {symbol: "BTC-PERP", displaySymbol, base: "BTC"|"AAPL-USD",
/// quote: "USDC", sizeDecimals, priceDecimals, takerFee,
/// tradingStatus, asset: "crypto"|"stock"|"forex",
/// tickSizes: [allowed price ticks], …}]}`. Verified live 2026-09-23
/// (544 symbols). Equity/index/forex bases carry the USD leg inside
/// `base` ("AAPL-USD") — split so the canonical symbol is `AAPL-USD`,
/// matching the `{T}-USD-PERP` venue spelling. Non-TRADING entries
/// are skipped per entry. Per-symbol `takerFee` (a decimal fraction —
/// currently 1bp crypto, 0 equity/forex) overrides the spec rate.
fn parse_vest_markets(
    venue: &VenueId,
    data: &serde_json::Value,
    fee: Decimal,
) -> Vec<MarketMetadata> {
    let arr = data
        .get("symbols")
        .and_then(|v| v.as_array())
        .or_else(|| data.as_array());
    let arr = match arr {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter_map(|e| {
            let vsym = e.get("symbol").and_then(|s| s.as_str())?;
            if !vsym.ends_with("-PERP") {
                return None;
            }
            let status = e.get("tradingStatus").and_then(|s| s.as_str());
            if let Some(s) = status {
                if !s.eq_ignore_ascii_case("TRADING") {
                    return None;
                }
            }
            let base_raw = e.get("base").and_then(|s| s.as_str())?;
            if base_raw.is_empty() {
                return None;
            }
            // `base` is "AAPL-USD" for equity/forex perps (canonical
            // `AAPL-USD`, quote USD); crypto bases are bare ("BTC")
            // and margin in `quote` (USDC).
            let (base, quote) = if let Some(t) = base_raw.strip_suffix("-USD") {
                (t.to_string(), "USD".to_string())
            } else {
                (
                    base_raw.to_string(),
                    e.get("quote")
                        .and_then(|s| s.as_str())
                        .unwrap_or("USDC")
                        .to_string(),
                )
            };
            // tickSizes lists every allowed price increment — take the
            // finest; decimals are the fallback grid.
            let price_tick = e
                .get("tickSizes")
                .and_then(|v| v.as_array())
                .and_then(|a| a.iter().filter_map(|t| dec_from_json(Some(t))).min())
                .or_else(|| {
                    e.get("priceDecimals")
                        .and_then(|v| v.as_u64())
                        .map(|d| dec!(1) / Decimal::from(10u64.pow(d.min(18) as u32)))
                });
            let size_tick = e
                .get("sizeDecimals")
                .and_then(|v| v.as_u64())
                .map(|d| dec!(1) / Decimal::from(10u64.pow(d.min(18) as u32)));
            // Per-symbol takerFee is a fraction ("0.0001" = 1bp).
            let sym_fee = dec_from_json(e.get("takerFee"))
                .map(|f| f * dec!(10000))
                .unwrap_or(fee);
            meta(
                venue,
                &base,
                &quote,
                vsym.to_string(),
                price_tick,
                size_tick,
                sym_fee,
            )
        })
        .collect()
}

/// Nado: `GET /gateway/v1/query?type=symbols&product_type=perp` →
/// `{status, data: {symbols: {"BTC-PERP": {product_id, symbol,
/// price_increment_x18, size_increment, min_size, taker_fee_rate_x18,
/// trading_status, …}}}}`. All numeric fields are 1e18-scaled strings;
/// only `trading_status == "live"` perps are admitted (`post_only`/
/// `reduce_only`/`not_tradable` markets must not surface as routable).
fn parse_nado_markets(
    venue: &VenueId,
    data: &serde_json::Value,
    fee: Decimal,
) -> Vec<MarketMetadata> {
    let symbols = match data
        .get("data")
        .and_then(|d| d.get("symbols"))
        .and_then(|s| s.as_object())
    {
        Some(s) => s,
        None => return Vec::new(),
    };
    symbols
        .values()
        .filter_map(|e| {
            if e.get("type").and_then(|s| s.as_str()) != Some("perp") {
                return None;
            }
            if e.get("trading_status").and_then(|s| s.as_str()) != Some("live") {
                return None;
            }
            let vsym = e.get("symbol").and_then(|s| s.as_str())?;
            let base = vsym.strip_suffix("-PERP")?;
            if base.is_empty() {
                return None;
            }
            let price_tick = nado_x18(e.get("price_increment_x18"));
            let size_tick = nado_x18(e.get("size_increment"));
            // taker_fee_rate_x18 is a fraction ("0.00035" → 3.5bps).
            let sym_fee = nado_x18(e.get("taker_fee_rate_x18"))
                .map(|f| f * dec!(10000))
                .unwrap_or(fee);
            let mut m = meta(
                venue,
                base,
                "USDT0",
                vsym.to_string(),
                price_tick,
                size_tick,
                sym_fee,
            )?;
            // min_size is quote-denominated (USDT0) scaled x18.
            if let Some(min_notional) = nado_x18(e.get("min_size")) {
                if min_notional > dec!(0) {
                    m.min_notional = min_notional;
                }
            }
            Some(m)
        })
        .collect()
}

/// EVEDEX: `GET /api/market/instrument?fields=metrics` returns a bare
/// array of instruments — `{name: "BTCUSD", type, from: {symbol},
/// to: {symbol}, priceIncrement, quantityIncrement, minVolume,
/// minQuantity, trading, marketState, visibility}`. `type` is the asset
/// class (perpetual-futures / indices / stocks / forex / commodities /
/// premarket) — all are perp contracts, so admission gates on
/// `trading=="all"`, `marketState=="OPEN"`, `visibility=="all"`.
fn parse_evedex_markets(
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
            if e.get("trading").and_then(|s| s.as_str()) != Some("all") {
                return None;
            }
            if e.get("marketState").and_then(|s| s.as_str()) != Some("OPEN") {
                return None;
            }
            if e.get("visibility").and_then(|s| s.as_str()) != Some("all") {
                return None;
            }
            let name = e.get("name").and_then(|s| s.as_str())?;
            let base = e.pointer("/from/symbol").and_then(|s| s.as_str())?;
            let quote = e
                .pointer("/to/symbol")
                .and_then(|s| s.as_str())
                .unwrap_or("USD");
            if base.is_empty() || quote.is_empty() {
                return None;
            }
            let mut m = meta(
                venue,
                base,
                quote,
                name.to_string(),
                dec_from_json(e.get("priceIncrement")),
                dec_from_json(e.get("quantityIncrement")),
                fee,
            )?;
            // minVolume is the venue's minimum notional ($5 at writing).
            if let Some(min_notional) = dec_from_json(e.get("minVolume")) {
                if min_notional > dec!(0) {
                    m.min_notional = min_notional;
                }
            }
            if let Some(min_qty) = dec_from_json(e.get("minQuantity")) {
                if min_qty > m.min_order_size {
                    m.min_order_size = min_qty;
                }
            }
            // Verified /api/market: maker 1.5bps, taker 4.5bps.
            m.maker_fee_bps = dec!(1.5);
            Some(m)
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
        "vest" => parse_vest_markets(venue, data, fee),
        "nado" => parse_nado_markets(venue, data, fee),
        "evedex" => parse_evedex_markets(venue, data, fee),
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

/// EVEDEX depth: `{t, asks: [{price, quantity}], bids: [{price,
/// quantity}]}` — object levels keyed `quantity` (not dYdX's `size`).
fn parse_evedex_book(
    venue: &VenueId,
    symbol: &str,
    data: &serde_json::Value,
) -> anyhow::Result<OrderBook> {
    let level = |v: &serde_json::Value| -> Option<(Decimal, Decimal)> {
        let p = dec_from_json(v.get("price"))?;
        let q = dec_from_json(v.get("quantity"))?;
        if p > dec!(0) && q > dec!(0) {
            Some((p, q))
        } else {
            None
        }
    };
    let mut bid_levels = Vec::new();
    let mut ask_levels = Vec::new();
    if let Some(arr) = data.get("bids").and_then(|v| v.as_array()) {
        for l in arr {
            if let Some((p, q)) = level(l) {
                bid_levels.push((p, q));
            }
        }
    }
    if let Some(arr) = data.get("asks").and_then(|v| v.as_array()) {
        for l in arr {
            if let Some((p, q)) = level(l) {
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
pub(crate) fn eip712_digest(domain_sep: &[u8; 32], struct_hash: &[u8; 32]) -> [u8; 32] {
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
    // Checked math: `10u64.pow` overflows for scale ≥ 20 and `Decimal` mul
    // panics past MAX — a venue-extreme input must fail, never panic or
    // silently wrap into a wrong order size.
    let multiplier = Decimal::from(10u128.checked_pow(scale)?);
    let scaled = v.checked_mul(multiplier)?.round();
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
        "filled" | "fully_filled" => Some(Filled),
        // "closed"/"done" mean terminal, not necessarily filled — a
        // zero-fill close must never masquerade as Filled. Verify against
        // quantities; anything short is terminal-with-partial (Cancelled —
        // the reconciler reads filled_qty and classifies PartialTerminal).
        "closed" | "done" => {
            if original > dec!(0) && filled >= original {
                Some(Filled)
            } else {
                Some(Cancelled)
            }
        }
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
        // Decode the JWT's `exp` claim rather than assuming a lifetime —
        // riding an expired token would fail every signed call. No `exp`
        // → conservative 1h refresh window, never a blind multi-day pin.
        let expires_at = token
            .split('.')
            .nth(1)
            .and_then(|p| base64_decode_url(p).ok())
            .and_then(|j| serde_json::from_slice::<serde_json::Value>(&j).ok())
            .and_then(|p| p.get("exp").and_then(|v| v.as_i64()))
            .map(|exp| {
                Utc::now()
                    + chrono::Duration::seconds(
                        (exp - Utc::now().timestamp() - 60).clamp(60, 7 * 86_400),
                    )
            })
            .unwrap_or_else(|| Utc::now() + chrono::Duration::hours(1));
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
            fee: None,
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
            data["tif"] = serde_json::json!(pacifica_limit_tif(order)?);
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
                v.as_u64()
                    .map(|i| i.to_string())
                    .or_else(|| v.as_i64().map(|i| i.to_string()))
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
                        v.as_u64()
                            .map(|i| i.to_string())
                            .or_else(|| v.as_i64().map(|i| i.to_string()))
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
        let tif = standx_tif(order)?;
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
            "postOnly": matches!(effective_exec_policy(order), ExecPolicy::PostOnly),
        });
        if !is_market {
            let price = order
                .price
                .ok_or_else(|| anyhow::anyhow!("Ondo limit orders require a price"))?;
            body["price"] = serde_json::json!(price.to_string());
            body["timeInForce"] = serde_json::json!(ondo_limit_policy(order)?.0);
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
        // GRVT time_in_force codes + postOnly flag come from the shared
        // venue policy table — every requested policy maps explicitly.
        let (tif_code, post_only) = grvt_policy(order);
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
            "time_in_force": derive_tif(order)?,
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
        // Aevo's signed-order wire carries no time-in-force — only a
        // post_only flag. IOC/FOK requests cannot be expressed; refuse
        // rather than rest a GTC order the caller never asked for.
        let aevo_post_only = aevo_policy(order)?;
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
            "post_only": aevo_post_only,
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

    /// Ondo Perps lists tokenized equity underlyings — every market is
    /// bound to US cash hours (the venue halts IOC outside the session).
    fn cash_session_bound(&self, _symbol: &str) -> bool {
        self.spec_id == "ondoperps"
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
        // edgeX v2 depth: `GET /api/v2/public/quote/getDepth` keyed by the
        // numeric contractId from metadata (level is fixed at 15 or 200).
        if self.spec_id == "edgex" {
            let vsym = self.venue_symbol(symbol);
            let meta = self.edgex_meta().await?;
            let contract = meta
                .contracts
                .get(&vsym)
                .ok_or_else(|| anyhow::anyhow!("edgeX has no contract for {vsym}"))?;
            let data = self
                .get_json(&format!(
                    "/api/v2/public/quote/getDepth?contractId={}&level=15",
                    contract.contract_id
                ))
                .await?;
            let entry = data
                .get("data")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .ok_or_else(|| anyhow::anyhow!("edgeX depth missing data for {vsym}"))?;
            let level = |key: &str| -> Vec<(Decimal, Decimal)> {
                entry
                    .get(key)
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|l| {
                                Some((
                                    dec_from_json(l.get("price"))?,
                                    dec_from_json(l.get("size"))?,
                                ))
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            };
            return book_from_levels(&self.venue_id, symbol, level("bids"), level("asks"));
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
            "evedex" => parse_evedex_book(&self.venue_id, symbol, &data),
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
        // edgeX: the per-contract ticker carries `fundingRate` plus the
        // last/next funding timestamps (interval derived from their delta).
        if self.spec_id == "edgex" {
            let vsym = self.venue_symbol(symbol);
            let meta = self.edgex_meta().await?;
            let contract = meta
                .contracts
                .get(&vsym)
                .ok_or_else(|| anyhow::anyhow!("edgeX has no contract for {vsym}"))?;
            let data = self
                .get_json(&format!(
                    "/api/v2/public/quote/getTicker?contractId={}",
                    contract.contract_id
                ))
                .await?;
            let entry = data
                .get("data")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .ok_or_else(|| anyhow::anyhow!("edgeX has no ticker for {symbol}"))?;
            let rate = dec_from_json(entry.get("fundingRate"))
                .ok_or_else(|| anyhow::anyhow!("edgeX has no funding rate for {symbol}"))?;
            let next = parse_millis_opt(entry.get("nextFundingTime"));
            let prev = parse_millis_opt(entry.get("fundingTime"));
            let interval_hours = match (prev, next) {
                (Some(p), Some(n)) if n > p => ((n - p).num_seconds() / 3600).max(1) as u32,
                _ => 4,
            };
            return Ok(funding_now(
                &self.venue_id,
                symbol,
                rate,
                interval_hours,
                next,
            ));
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
        // Nado: archive `funding_rate` query returns the hourly funding
        // rate for the product (x18 fraction). Verified live.
        if self.spec_id == "nado" {
            let pid = self.nado_product_id(symbol).await?;
            let url = format!("{}/archive/v1", self.base_url);
            let data = self
                .post_json_url(
                    &url,
                    &serde_json::json!({"funding_rate": {"product_id": pid}}),
                    &[],
                )
                .await?;
            let rate = nado_x18(data.get("funding_rate_x18"))
                .ok_or_else(|| anyhow::anyhow!("Nado has no funding rate for {symbol}"))?;
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
        // Vest: `/v2/ticker/latest` returns `oneHrFundingRate` per
        // symbol — the live hourly rate (verified field name).
        if self.spec_id == "vest" {
            let vsym = self.venue_symbol(symbol);
            let data = self
                .get_json(&format!("/v2/ticker/latest?symbols={vsym}"))
                .await?;
            let arr = data
                .get("tickers")
                .and_then(|v| v.as_array())
                .or_else(|| data.as_array())
                .ok_or_else(|| anyhow::anyhow!("Vest latest tickers missing for {symbol}"))?;
            for e in arr {
                if e.get("symbol").and_then(|s| s.as_str()) == Some(vsym.as_str()) {
                    let rate = dec_from_json(e.get("oneHrFundingRate"))
                        .or_else(|| dec_from_json(e.get("fundingRate")))
                        .ok_or_else(|| anyhow::anyhow!("Vest has no funding rate for {symbol}"))?;
                    return Ok(funding_now(&self.venue_id, symbol, rate, 1, None));
                }
            }
            anyhow::bail!("Vest has no ticker entry for {symbol}");
        }
        // EVEDEX: instrument metrics carry `fundingRate` (the live hourly
        // rate — fundingRateCreatedAt lands on the hour). Verified live.
        if self.spec_id == "evedex" {
            let vsym = self.venue_symbol(symbol);
            let data = self
                .get_json("/api/market/instrument?fields=metrics")
                .await?;
            let arr = data
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("EVEDEX instruments missing array"))?;
            for e in arr {
                if e.get("name").and_then(|s| s.as_str()) == Some(vsym.as_str()) {
                    let rate = dec_from_json(e.get("fundingRate")).ok_or_else(|| {
                        anyhow::anyhow!("EVEDEX has no funding rate for {symbol}")
                    })?;
                    return Ok(funding_now(
                        &self.venue_id,
                        symbol,
                        rate,
                        1,
                        parse_time_opt(e.get("fundingRateCreatedAt")),
                    ));
                }
            }
            anyhow::bail!("EVEDEX has no instrument entry for {symbol}");
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
        // Nado: `/archive/v2/tickers` → `{ticker_id: {base_currency,
        // quote_volume}}` — keyed by the canonical base part.
        if self.spec_id == "nado" {
            let data = self.get_json("/archive/v2/tickers").await?;
            if let Some(m) = data.as_object() {
                for (tid, e) in m {
                    if let Some(v) = dec_from_json(e.get("quote_volume")) {
                        if let Some(sym) = tid.strip_suffix("_USDT0") {
                            if let Some(base) = sym.strip_suffix("-PERP") {
                                out.insert(base.to_uppercase(), v);
                            }
                        }
                    }
                }
            }
            return Ok(out);
        }
        // EVEDEX: metrics `volume` is the 24h quote (USD) turnover per
        // instrument; key by the base asset symbol.
        if self.spec_id == "evedex" {
            let data = self
                .get_json("/api/market/instrument?fields=metrics")
                .await?;
            if let Some(arr) = data.as_array() {
                for e in arr {
                    // `:RISK` companion instruments share the base `from.symbol`
                    // with `trading:"none"`/`volume:0` — admit only tradable rows
                    // (same predicate as `parse_evedex_markets`).
                    if e.get("trading").and_then(|s| s.as_str()) != Some("all")
                        || e.get("marketState").and_then(|s| s.as_str()) != Some("OPEN")
                        || e.get("visibility").and_then(|s| s.as_str()) != Some("all")
                    {
                        continue;
                    }
                    if let (Some(base), Some(v)) = (
                        e.pointer("/from/symbol").and_then(|s| s.as_str()),
                        dec_from_json(e.get("volume")),
                    ) {
                        out.insert(base.to_uppercase(), v);
                    }
                }
            }
            return Ok(out);
        }
        // Vest: `/v2/ticker/24hr` → `{tickers: [{symbol, quoteVolume}]}`;
        // keyed by the canonical base part ("BTC-PERP"→"BTC",
        // "AAPL-USD-PERP"→"AAPL-USD").
        if self.spec_id == "vest" {
            let data = self.get_json("/v2/ticker/24hr").await?;
            if let Some(arr) = data
                .get("tickers")
                .and_then(|v| v.as_array())
                .or_else(|| data.as_array())
            {
                for e in arr {
                    if let (Some(sym), Some(v)) = (
                        e.get("symbol").and_then(|s| s.as_str()),
                        dec_from_json(e.get("quoteVolume")),
                    ) {
                        if let Some(stripped) = sym.strip_suffix("-PERP") {
                            out.insert(stripped.to_uppercase(), v);
                        }
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
            "vest" => self.vest_account().await,
            "qfex" => self.qfex_account().await,
            "nado" => self.nado_account().await,
            "evedex" => self.evedex_account().await,
            "paradex" => self.paradex_account().await,
            "edgex" => self.edgex_account().await,
            "extended" => self.extended_account().await,
            "dydx" => self.dydx_account().await,
            "helix" => self.helix_account().await,
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
            "vest" => self.vest_place(&order).await,
            "qfex" => self.qfex_place(&order).await,
            "nado" => self.nado_place(&order).await,
            "evedex" => self.evedex_place(&order).await,
            "paradex" => self.paradex_place(&order).await,
            "edgex" => self.edgex_place(&order).await,
            "extended" => self.extended_place(&order).await,
            "dydx" => self.dydx_place(&order).await,
            "helix" => self.helix_place(&order).await,
            _ => anyhow::bail!(wallet_execution_unavailable(
                self.name(),
                "its settlement chain",
                "wallet-signed order flow"
            )),
        }
    }

    async fn wire_base_qty(&self, symbol: &str, qty: Decimal) -> anyhow::Result<Decimal> {
        // GRVT serializes size as u64 at 1e9 scale; the other family venues
        // pass the decimal quantity through to the venue's own API grid.
        if self.spec_id == "grvt" {
            let wire = qty.trunc_with_scale(9);
            if wire <= dec!(0) {
                anyhow::bail!("GRVT {symbol} quantity {qty} truncates to zero at 1e9 scale");
            }
            return Ok(wire);
        }
        let _ = symbol;
        Ok(qty)
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
            "vest" => self.vest_cancel(&req).await,
            "qfex" => self.qfex_cancel(&req).await,
            "nado" => self.nado_cancel(&req).await,
            "evedex" => self.evedex_cancel(&req).await,
            "paradex" => self.paradex_cancel(&req).await,
            "edgex" => self.edgex_cancel(&req).await,
            "extended" => self.extended_cancel(&req).await,
            "dydx" => self.dydx_cancel(&req).await,
            "helix" => self.helix_cancel(&req).await,
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
            "vest" => {
                self.vest_fetch_order(symbol, order_id, client_order_id)
                    .await
            }
            "qfex" => {
                self.qfex_fetch_order(symbol, order_id, client_order_id)
                    .await
            }
            "nado" => {
                self.nado_fetch_order(symbol, order_id, client_order_id)
                    .await
            }
            "evedex" => {
                self.evedex_fetch_order(symbol, order_id, client_order_id)
                    .await
            }
            "paradex" => {
                self.paradex_fetch_order(symbol, order_id, client_order_id)
                    .await
            }
            "edgex" => {
                self.edgex_fetch_order(symbol, order_id, client_order_id)
                    .await
            }
            "extended" => {
                self.extended_fetch_order(symbol, order_id, client_order_id)
                    .await
            }
            "dydx" => {
                self.dydx_fetch_order(symbol, order_id, client_order_id)
                    .await
            }
            "helix" => {
                self.helix_fetch_order(symbol, order_id, client_order_id)
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
            "edgex" => self.edgex_set_leverage(symbol, leverage).await,
            "extended" => self.extended_set_leverage(symbol, leverage).await,
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
            // Vest: funded primary account + an EVM key (delegate signer
            // with a stored apiKey/accGroup, or the primary key itself —
            // which self-registers a 7-day signer on first use).
            "vest" => {
                c.wallet_address.is_some()
                    && self.wallet.read().is_some()
                    && (c.api_key.is_some()
                        || self
                            .wallet
                            .read()
                            .as_ref()
                            .map(|w| {
                                c.wallet_address
                                    .as_deref()
                                    .map(|p| w.address_hex().eq_ignore_ascii_case(p))
                                    .unwrap_or(false)
                            })
                            .unwrap_or(false))
            }
            "pacifica" | "ondoperps" | "derive" => c.wallet_address.is_some() && has_key,
            // QFEX: API key pair (public + secret) is the whole credential.
            "qfex" => c.api_key.is_some() && c.api_secret.is_some(),
            // Nado: funded account address + an EVM signing key. The key
            // may be the account key itself or a linked signer — venue
            // rejects unauthorized signers at execute time.
            "nado" => c.wallet_address.is_some() && self.wallet.read().is_some(),
            // EVEDEX: the SIWE message embeds the signer address, so the
            // configured walletAddress must equal the signing key's
            // derived address — a mismatch can never authenticate.
            "evedex" => {
                c.wallet_address.is_some()
                    && self
                        .wallet
                        .read()
                        .as_ref()
                        .map(|w| {
                            c.wallet_address
                                .as_deref()
                                .map(|a| w.address_hex().eq_ignore_ascii_case(a))
                                .unwrap_or(false)
                        })
                        .unwrap_or(false)
            }
            // Paradex: the L2 key is derived from the EVM key, so the
            // configured walletAddress must equal the signing key's
            // address — the derived Stark account only exists under it.
            "paradex" => {
                c.wallet_address.is_some()
                    && self.paradex_l2_key.read().is_some()
                    && self
                        .wallet
                        .read()
                        .as_ref()
                        .map(|w| {
                            c.wallet_address
                                .as_deref()
                                .map(|a| w.address_hex().eq_ignore_ascii_case(a))
                                .unwrap_or(false)
                        })
                        .unwrap_or(false)
            }
            // edgeX: HMAC api trio (key/passphrase/secret) + numeric
            // accountId + the EIP-712 trading key. The signer field in
            // the order signature is derived from the trading key, so
            // no wallet_address is required.
            "edgex" => {
                c.api_key.is_some()
                    && c.api_secret.is_some()
                    && c.passphrase.is_some()
                    && c.account_id.is_some()
                    && self.wallet.read().is_some()
            }
            // Extended: `X-Api-Key` + vault number (`account_id`) + the
            // Stark key pair (`private_key`/`wallet_address` — pair
            // consistency is enforced at credential load).
            "extended" => {
                c.api_key.is_some()
                    && c.account_id.is_some()
                    && c.wallet_address.is_some()
                    && self.stark_key.read().is_some()
            }
            // dYdX: the mnemonic is the whole credential (Cosmos key
            // derivation); `account_id` selects the subaccount (default 0)
            // and `wallet_address` — when set — is verified at load.
            "dydx" => self.dydx_mnemonic.read().is_some(),
            // Helix/Injective: the EVM key IS the account key (keccak
            // address → inj1 bech32). `wallet_address`, when configured,
            // must equal the signer's address — the subaccount derives
            // from it. `account_id` selects the subaccount nonce (0-255).
            "helix" => {
                self.wallet.read().is_some()
                    && c.wallet_address
                        .as_deref()
                        .map(|a| {
                            self.wallet
                                .read()
                                .as_ref()
                                .map(|w| w.address_hex().eq_ignore_ascii_case(a))
                                .unwrap_or(false)
                        })
                        .unwrap_or(true)
            }
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Vest Markets — delegate-signer (agent wallet) auth + account-group routing
// ---------------------------------------------------------------------------

/// EIP-712 verifying contract for Vest's `VestRouterV2` signer domain
/// (production; docs.vestmarkets.com/vest-api).
const VEST_VERIFYING_CONTRACT: &str = "0x919386306C47b2Fe1036e3B4F7C40D22D2461a23";

/// Delegate-signer validity offered at registration (~7 days — the docs
/// show `expiryTime ≈ now + 604800000` ms).
const VEST_SIGNER_EXPIRY_MS: i64 = 7 * 24 * 3600 * 1000;

/// Vest rejects stale requests — `recvWindow` is widened from the 5s
/// default so clock jitter never rejects an otherwise-valid order.
const VEST_RECV_WINDOW_MS: u64 = 60_000;

fn vest_now_ms() -> u64 {
    Utc::now().timestamp_millis().max(0) as u64
}

impl DexPerpAdapter {
    /// Sign a 32-byte venue hash with the held EVM key — Vest signs
    /// `personal_sign(keccak256(abi.encode(...)))` for orders/cancels.
    fn vest_sign(&self, hash32: &[u8; 32]) -> anyhow::Result<String> {
        let w = self.wallet.read();
        let s = w.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Vest requires an EVM signing key (privateKey field)")
        })?;
        Ok(format!(
            "0x{}",
            hex::encode(s.sign_personal_message(hash32)?)
        ))
    }

    /// Auth headers for private calls. Self-registers (primary-key held)
    /// when no apiKey/accGroup are stored yet.
    async fn vest_headers(&self) -> anyhow::Result<Vec<(String, String)>> {
        self.vest_ensure_registered().await?;
        let c = self.creds.read();
        let key = c.api_key.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "Vest needs an apiKey from POST /register — delegate signers \
                 cannot self-register; run the registration externally and \
                 store apiKey + account group on the account"
            )
        })?;
        let group = c.account_id.clone().unwrap_or_else(|| "0".to_string());
        Ok(vec![
            ("X-API-KEY".to_string(), key),
            ("xrestservermm".to_string(), format!("restserver{group}")),
        ])
    }

    /// One-time `POST /v2/register` when apiKey/accGroup are absent and
    /// the held key IS the primary account key. Registers the key itself
    /// as a delegate signer for ~7 days (EIP-712 `SignerProof` over the
    /// `VestRouterV2` domain — no chainId per the venue docs). The
    /// returned `{apiKey, accGroup}` is cached on the adapter creds for
    /// the process lifetime; persist it in the vault to skip the call.
    async fn vest_ensure_registered(&self) -> anyhow::Result<()> {
        {
            let c = self.creds.read();
            if c.api_key.is_some() && c.account_id.is_some() {
                return Ok(());
            }
        }
        let signer_addr = self
            .wallet
            .read()
            .as_ref()
            .map(|w| w.address_hex())
            .ok_or_else(|| {
                anyhow::anyhow!("Vest requires an EVM signing key (privateKey field)")
            })?;
        let primary = self.wallet_address.read().clone().ok_or_else(|| {
            anyhow::anyhow!("Vest requires the funded account address (walletAddress field)")
        })?;
        if !signer_addr.eq_ignore_ascii_case(&primary) {
            anyhow::bail!(
                "Vest signing key {signer_addr} is not the primary account {primary}; \
                 a delegate signer cannot self-register — run POST /register once \
                 externally (docs.vestmarkets.com/vest-api) and store the returned \
                 apiKey + account group on this account"
            );
        }
        let expiry = Utc::now().timestamp_millis() + VEST_SIGNER_EXPIRY_MS;
        // EIP712Domain(string name,string version,address verifyingContract)
        // — Vest's domain omits chainId per the venue docs.
        let domain_type =
            keccak256(b"EIP712Domain(string name,string version,address verifyingContract)");
        let contract: Address = VEST_VERIFYING_CONTRACT
            .parse()
            .map_err(|e| anyhow::anyhow!("Vest verifying contract parse: {e}"))?;
        let domain_sep = keccak256(
            (
                domain_type,
                keccak256(b"VestRouterV2"),
                keccak256(b"0.0.1"),
                contract,
            )
                .abi_encode(),
        );
        let struct_type = keccak256(b"SignerProof(address approvedSigner,uint256 signerExpiry)");
        let signer: Address = signer_addr
            .parse()
            .map_err(|e| anyhow::anyhow!("Vest signer address parse: {e}"))?;
        let struct_hash =
            keccak256((struct_type, signer, U256::from(expiry.max(0) as u64)).abi_encode());
        let digest = eip712_digest(&domain_sep.0, &struct_hash.0);
        let signature = self.vest_sign(&digest)?;
        let body = serde_json::json!({
            "signingAddr": signer_addr,
            "primaryAddr": primary.to_lowercase(),
            "signature": signature,
            "expiryTime": expiry,
            "networkType": 0,
        });
        let url = format!("{}/v2/register", self.base_url);
        let resp = self
            .post_json_url(
                &url,
                &body,
                &[("xrestservermm".to_string(), "restserver0".to_string())],
            )
            .await?;
        let api_key = resp
            .get("apiKey")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Vest /register returned no apiKey: {resp}"))?;
        let acc_group = resp.get("accGroup").and_then(|v| {
            v.as_u64()
                .map(|n| n.to_string())
                .or_else(|| v.as_str().map(|s| s.to_string()))
        });
        let mut c = self.creds.write();
        c.api_key = Some(api_key);
        if let Some(g) = acc_group {
            c.account_id = Some(g);
        }
        Ok(())
    }

    /// Sequential order nonce — `max(lastNonce + 1, now_ms)`; the docs
    /// bless epoch-millis nonces, so a nonce-read failure degrades to the
    /// clock rather than blocking the order.
    async fn vest_nonce(&self) -> u64 {
        let now = vest_now_ms();
        let headers = match self.vest_headers().await {
            Ok(h) => h,
            Err(_) => return now,
        };
        let url = format!("{}/v2/account/nonce?time={now}", self.base_url);
        match self.get_json_url(&url, &headers).await {
            Ok(v) => {
                let last = v.get("lastNonce").and_then(|n| {
                    n.as_u64()
                        .or_else(|| n.as_str().and_then(|s| s.parse::<u64>().ok()))
                });
                match last {
                    Some(l) => now.max(l.saturating_add(1)),
                    None => now,
                }
            }
            Err(_) => now,
        }
    }

    async fn vest_place(&self, order: &OrderRequest) -> anyhow::Result<OrderResponse> {
        use arbitrageur_core::types::OrderType;
        if order.order_type == OrderType::PostOnly
            || order.time_in_force == arbitrageur_core::types::TimeInForce::PostOnly
            || order.post_only
        {
            anyhow::bail!(
                "Vest has no post-only order type — refusing to send an order that could take"
            );
        }
        let vsym = self.venue_symbol(&order.symbol);
        let is_buy = order.side.is_buy();
        // Vest exposes MARKET plus LIMIT with GTC/FOK tifs — the shared
        // policy table maps IOC to FOK (strictly stronger, never weaker)
        // and refuses post-only outright.
        let (order_type, tif) = vest_wire_type(order)?;
        // Resolve auth first (self-registering when the primary key is
        // held) — a credential failure must not even read the book.
        let headers = self.vest_headers().await?;
        // Vest requires limitPrice even on MARKET — it is the slippage
        // bound, not a rest price.
        let limit_price = self.market_bound_price(order).await?;
        let size_str = order.quantity.normalize().to_string();
        let price_str = limit_price.normalize().to_string();
        if order.quantity <= dec!(0) {
            anyhow::bail!("Vest order size must be positive");
        }
        let time = vest_now_ms();
        let nonce = self.vest_nonce().await;
        // abi.encode(uint256 time, uint256 nonce, string orderType,
        // string symbol, bool isBuy, string size, string limitPrice,
        // bool reduceOnly) — field order per docs.
        let encoded = (
            U256::from(time),
            U256::from(nonce),
            order_type.to_string(),
            vsym.clone(),
            is_buy,
            size_str.clone(),
            price_str.clone(),
            order.reduce_only,
        )
            .abi_encode();
        let signature = self.vest_sign(&keccak256(&encoded).0)?;
        let mut order_body = serde_json::json!({
            "time": time,
            "nonce": nonce,
            "symbol": vsym,
            "isBuy": is_buy,
            "size": size_str,
            "orderType": order_type,
            "limitPrice": price_str,
            "reduceOnly": order.reduce_only,
        });
        if let Some(t) = tif {
            order_body["timeInForce"] = serde_json::json!(t);
        }
        let body = serde_json::json!({
            "order": order_body,
            "recvWindow": VEST_RECV_WINDOW_MS,
            "signature": signature,
        });
        let url = format!("{}/v2/orders", self.base_url);
        let resp = self.post_json_url(&url, &body, &headers).await?;
        let id = resp
            .get("id")
            .and_then(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .or_else(|| v.as_i64().map(|i| i.to_string()))
            })
            .ok_or_else(|| anyhow::anyhow!("Vest order returned no id: {resp}"))?;
        Ok(self.order_response(
            id,
            order.client_order_id.clone(),
            &order.symbol,
            arbitrageur_core::types::OrderStatus::New,
            order.quantity,
            dec!(0),
            None,
            resp,
        ))
    }

    async fn vest_cancel(&self, req: &CancelRequest) -> anyhow::Result<()> {
        let id = req
            .order_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Vest cancel needs the venue order id"))?;
        let headers = self.vest_headers().await?;
        let time = vest_now_ms();
        let nonce = self.vest_nonce().await;
        // abi.encode(uint256 time, uint256 nonce, string id)
        let encoded = (U256::from(time), U256::from(nonce), id.to_string()).abi_encode();
        let signature = self.vest_sign(&keccak256(&encoded).0)?;
        let body = serde_json::json!({
            "order": {"time": time, "nonce": nonce, "id": id},
            "recvWindow": VEST_RECV_WINDOW_MS,
            "signature": signature,
        });
        let url = format!("{}/v2/orders/cancel", self.base_url);
        self.post_json_url(&url, &body, &headers).await?;
        Ok(())
    }

    /// `GET /v2/orders?id=…&time=…` — authoritative venue state for the
    /// reconciler. Statuses: NEW / PARTIALLY_FILLED / FILLED /
    /// CANCELLED / REJECTED (LIQUIDATION appears only in history).
    async fn vest_fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        _client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        let headers = self.vest_headers().await?;
        let url = format!(
            "{}/v2/orders?id={order_id}&time={}",
            self.base_url,
            vest_now_ms()
        );
        let data = self.get_json_url(&url, &headers).await?;
        let arr = data
            .as_array()
            .or_else(|| data.get("orders").and_then(|v| v.as_array()))
            .ok_or_else(|| anyhow::anyhow!("Vest orders response is not an array: {data}"))?;
        let entry = arr
            .iter()
            .find(|e| {
                e.get("id")
                    .and_then(|v| {
                        v.as_str()
                            .map(|s| s.to_string())
                            .or_else(|| v.as_i64().map(|i| i.to_string()))
                    })
                    .map(|s| s.eq_ignore_ascii_case(order_id))
                    .unwrap_or(false)
            })
            .ok_or_else(|| {
                anyhow::anyhow!("Vest has no order {order_id} (not in open/history window)")
            })?;
        let size = dec_from_json(entry.get("size")).unwrap_or_default();
        let status_str = entry.get("status").and_then(|s| s.as_str()).unwrap_or("");
        // lastFilledSize is null unless FILLED per docs — a FILLED order
        // filled `size` in full; anything else reports the field or zero.
        let filled = if status_str.eq_ignore_ascii_case("FILLED") {
            size
        } else {
            dec_from_json(entry.get("lastFilledSize")).unwrap_or_default()
        };
        let status = map_order_status(status_str, filled, size).ok_or_else(|| {
            anyhow::anyhow!("Vest order {order_id} has unknown status '{status_str}'")
        })?;
        let mut resp = self.order_response(
            order_id.to_string(),
            String::new(),
            symbol,
            status,
            size,
            filled,
            dec_from_json(entry.get("avgFilledPrice")),
            entry.clone(),
        );
        if let Some(f) = dec_from_json(entry.get("fees")) {
            resp.fee = Some(f);
            resp.fee_currency = "USDC".to_string();
        }
        Ok(resp)
    }

    /// `GET /v2/account?time=…` — equity = totalAccountValue (USDC),
    /// free = withdrawable; positions carry entry/mark/liq/upnl.
    async fn vest_account(&self) -> anyhow::Result<AccountSnapshot> {
        let headers = self.vest_headers().await?;
        let url = format!("{}/v2/account?time={}", self.base_url, vest_now_ms());
        let data = self.get_json_url(&url, &headers).await?;
        let r = data.get("account").cloned().unwrap_or_else(|| data.clone());
        let equity = dec_from_json(r.get("totalAccountValue"))
            .or_else(|| dec_from_json(r.get("accountValue")))
            .or_else(|| dec_from_json(r.get("equity")))
            .ok_or_else(|| anyhow::anyhow!("Vest account has no equity field: {data}"))?;
        let free = dec_from_json(r.get("withdrawable"))
            .or_else(|| dec_from_json(r.get("availableBalance")))
            .unwrap_or_default();
        let maint = dec_from_json(r.get("totalMaintMargin"))
            .or_else(|| dec_from_json(r.get("maintenanceMargin")))
            .unwrap_or_default();
        let mut positions = Vec::new();
        if let Some(arr) = r.get("positions").and_then(|v| v.as_array()) {
            for p in arr {
                let qty = dec_from_json(p.get("size")).unwrap_or_default();
                if qty <= dec!(0) {
                    continue;
                }
                let vsym = p.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
                let side = if p.get("isLong").and_then(|v| v.as_bool()).unwrap_or(true) {
                    arbitrageur_core::types::Side::Buy
                } else {
                    arbitrageur_core::types::Side::Sell
                };
                // "BTC-PERP"→"BTC", "AAPL-USD-PERP"→"AAPL-USD" — the
                // canonical spelling the engine tracks.
                let sym = vsym.strip_suffix("-PERP").unwrap_or(vsym).to_uppercase();
                // initMarginRatio "0.125" ⇒ 8x.
                let leverage = dec_from_json(p.get("initMarginRatio"))
                    .filter(|r| *r > dec!(0))
                    .map(|r| dec!(1) / r);
                positions.push(position_from(
                    &self.venue_id,
                    sym,
                    side,
                    qty,
                    dec_from_json(p.get("entryPrice")),
                    dec_from_json(p.get("markPrice")),
                    dec_from_json(p.get("unrealizedPnl")),
                    dec_from_json(p.get("liqPrice")),
                    leverage,
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
}

// ---------------------------------------------------------------------------
// QFEX — HMAC REST reads + persistent authenticated trade WebSocket
// ---------------------------------------------------------------------------

/// QFEX trade socket endpoint (order entry is WS-only; REST covers
/// account reads and terminal order history).
const QFEX_TRADE_WS: &str = "wss://trade.qfex.com";

/// Live state of one QFEX trade-socket session. Order responses are
/// pushed (no request ids), so waiters key on `client_order_id` (add)
/// or `order_id` (cancel); the orders map keeps the latest pushed
/// state per id for `fetch_order` — QFEX has no open-orders REST.
struct QfexWsState {
    /// JSON text frames to write (forwarded by the socket task).
    out_tx: tokio::sync::mpsc::UnboundedSender<String>,
    /// order_id/client_order_id → latest `order_response` payload.
    orders: Arc<std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>>,
    /// One-shot waiters keyed by the id being awaited.
    pending:
        Arc<std::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<serde_json::Value>>>>,
    /// False once the socket task exits (disconnect → next call re-auths).
    alive: Arc<std::sync::atomic::AtomicBool>,
}

/// QFEX order_response/terminal statuses → engine status. Rejections
/// carry granular reason codes; all map to Rejected (fail-closed for
/// anything unknown → None).
fn qfex_map_status(
    status: &str,
    qty: Decimal,
    remaining: Decimal,
) -> Option<arbitrageur_core::types::OrderStatus> {
    use arbitrageur_core::types::OrderStatus::*;
    match status {
        "ACK" | "MODIFIED" => {
            if remaining < qty && remaining > dec!(0) {
                Some(PartiallyFilled)
            } else {
                Some(New)
            }
        }
        "FILLED" => {
            if remaining > dec!(0) {
                Some(PartiallyFilled)
            } else {
                Some(Filled)
            }
        }
        "IOC_PARTIALLY_FILLED" => Some(Cancelled), // terminal; remainder cancelled
        "IOC_CANCELLED" | "CANCELLED" | "CANCELLED_STP" | "NO_SUCH_ORDER" => Some(Cancelled),
        s if s.starts_with("REJECTED")
            || s.starts_with("INVALID")
            || s.starts_with("CANNOT")
            || s.starts_with("FAILED")
            || s == "USER_NOT_FOUND"
            || s == "PERMISSION_DENIED"
            || s == "RATE_LIMITED"
            || s == "USER_IN_LIQUIDATION"
            || s == "BAD_SYMBOL"
            || s == "NO_CHANGE_NO_UPDATE"
            || s == "QUANTITY_LESS_THAN_MIN_QUANTITY"
            || s == "QUANTITY_GREATER_THAN_MAX_QUANTITY"
            || s == "PRICE_LESS_THAN_MIN_PRICE"
            || s == "PRICE_GREATER_THAN_MAX_PRICE" =>
        {
            Some(Rejected)
        }
        _ => None,
    }
}

impl DexPerpAdapter {
    /// `x-qfex-*` HMAC headers for REST reads (`{nonce}:{unix_ts}` —
    /// HMAC-SHA256 hex with the secret key). Nonce is uuid-hex (unique
    /// within the 15-minute window by construction).
    fn qfex_headers(&self) -> anyhow::Result<Vec<(String, String)>> {
        let c = self.creds.read();
        let (Some(pub_key), Some(secret)) = (c.api_key.clone(), c.api_secret.clone()) else {
            anyhow::bail!("QFEX requires an API key pair (apiKey + apiSecret)");
        };
        let account = c.account_id.clone();
        drop(c);
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let ts = Utc::now().timestamp();
        let payload = format!("{nonce}:{ts}");
        use hmac::{digest::KeyInit, Mac};
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes())
            .map_err(|e| anyhow::anyhow!("QFEX secret key invalid: {e}"))?;
        mac.update(payload.as_bytes());
        let sig = hex::encode(mac.finalize().into_bytes());
        let mut headers = vec![
            ("x-qfex-public-key".to_string(), pub_key),
            ("x-qfex-hmac-signature".to_string(), sig),
            ("x-qfex-nonce".to_string(), nonce),
            ("x-qfex-timestamp".to_string(), ts.to_string()),
        ];
        if let Some(acc) = account {
            headers.push(("x-qfex-requested-account-id".to_string(), acc));
        }
        Ok(headers)
    }

    /// Connect + authenticate + subscribe `order_responses`. Reconnects
    /// when the prior session's socket task has died. The session is
    /// dropped on any auth failure (never half-authenticated).
    async fn qfex_session(&self) -> anyhow::Result<Arc<QfexWsState>> {
        if let Some(s) = self.qfex_ws.read().as_ref() {
            if s.alive.load(std::sync::atomic::Ordering::Relaxed) {
                return Ok(s.clone());
            }
        }
        // Scope the lock guard to a block — a `parking_lot` read guard is
        // not `Send`, and `let-else` scrutinee temporaries can otherwise
        // keep it live across the connect await below.
        let (pub_key, secret, account) = {
            let c = self.creds.read();
            (
                c.api_key.clone(),
                c.api_secret.clone(),
                c.account_id.clone(),
            )
        };
        let (Some(pub_key), Some(secret)) = (pub_key, secret) else {
            anyhow::bail!("QFEX requires an API key pair (apiKey + apiSecret)");
        };
        let url = format!("{QFEX_TRADE_WS}?api_key={pub_key}");
        let (ws, _) = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async(&url),
        )
        .await
        .map_err(|_| anyhow::anyhow!("QFEX trade WS connect timed out"))??;
        let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let orders = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let pending = Arc::new(std::sync::Mutex::new(HashMap::<
            String,
            tokio::sync::oneshot::Sender<serde_json::Value>,
        >::new()));
        let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let state = Arc::new(QfexWsState {
            out_tx,
            orders: orders.clone(),
            pending: pending.clone(),
            alive: alive.clone(),
        });
        tokio::spawn(async move {
            use futures::{SinkExt, StreamExt};
            use tokio_tungstenite::tungstenite::Message;
            let mut stream = ws;
            loop {
                tokio::select! {
                    out = out_rx.recv() => match out {
                        Some(s) => {
                            if stream.send(Message::Text(s)).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    },
                    inc = stream.next() => match inc {
                        Some(Ok(Message::Text(t))) => {
                            let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else {
                                continue;
                            };
                            // auth + subscribe acks resolve their waiters.
                            if v.get("type").and_then(|x| x.as_str()) == Some("auth") {
                                if let Some(tx) = pending.lock().unwrap().remove("__auth") {
                                    let _ = tx.send(v.clone());
                                }
                                continue;
                            }
                            if let Some(or) = v.get("order_response") {
                                let oid = or
                                    .get("order_id")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let coid = or
                                    .get("client_order_id")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                {
                                    let mut m = orders.lock().unwrap();
                                    if !oid.is_empty() {
                                        m.insert(oid.clone(), or.clone());
                                    }
                                    if !coid.is_empty() {
                                        m.insert(coid.clone(), or.clone());
                                    }
                                }
                                // Resolve waiters on either key.
                                let mut p = pending.lock().unwrap();
                                for key in [&coid, &oid] {
                                    if let Some(tx) = p.remove(key) {
                                        let _ = tx.send(or.clone());
                                    }
                                }
                            }
                        }
                        Some(Ok(Message::Ping(p))) => {
                            let _ = stream.send(Message::Pong(p)).await;
                        }
                        Some(Ok(Message::Close(_))) | None => break,
                        Some(Err(_)) => break,
                        _ => {}
                    }
                }
            }
            alive.store(false, std::sync::atomic::Ordering::Relaxed);
            let mut p = pending.lock().unwrap();
            for (_, tx) in p.drain() {
                let _ = tx.send(serde_json::json!({"status": "__disconnected"}));
            }
        });
        // Authenticate within the venue's 1-minute window.
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let ts = Utc::now().timestamp();
        use hmac::{digest::KeyInit, Mac};
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes())
            .map_err(|e| anyhow::anyhow!("QFEX secret key invalid: {e}"))?;
        mac.update(format!("{nonce}:{ts}").as_bytes());
        let sig = hex::encode(mac.finalize().into_bytes());
        let mut auth_params = serde_json::json!({
            "hmac": {
                "public_key": pub_key,
                "nonce": nonce,
                "unix_ts": ts,
                "signature": sig,
            }
        });
        if let Some(acc) = account.as_deref() {
            auth_params["account_id"] = serde_json::json!(acc);
        }
        let (auth_tx, auth_rx) = tokio::sync::oneshot::channel();
        state
            .pending
            .lock()
            .unwrap()
            .insert("__auth".to_string(), auth_tx);
        state
            .out_tx
            .send(serde_json::json!({"type": "auth", "params": auth_params}).to_string())
            .map_err(|_| anyhow::anyhow!("QFEX WS writer closed before auth"))?;
        let ack = tokio::time::timeout(Duration::from_secs(15), auth_rx)
            .await
            .map_err(|_| anyhow::anyhow!("QFEX auth ack timed out"))?
            .map_err(|_| anyhow::anyhow!("QFEX auth waiter dropped"))?;
        if ack.get("result").and_then(|v| v.as_str()) != Some("success") {
            anyhow::bail!("QFEX trade WS auth rejected: {ack}");
        }
        // Subscribe order_responses — fire-and-forget; the venue ack is
        // consumed by the reader loop (no waiter registered).
        let _ = state.out_tx.send(
            serde_json::json!({"type": "subscribe", "params": {"channels": ["order_responses"]}})
                .to_string(),
        );
        *self.qfex_ws.write() = Some(state.clone());
        Ok(state)
    }

    /// Send one trade-channel command and await the order_response that
    /// resolves `wait_key` (client_order_id for adds, order_id for
    /// cancels). Times out hard so a dropped ack never wedges a leg.
    async fn qfex_request(
        &self,
        wait_key: &str,
        frame: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        self.rate_limiter.acquire_signed().await?;
        let state = self.qfex_session().await?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        state
            .pending
            .lock()
            .unwrap()
            .insert(wait_key.to_string(), tx);
        if state.out_tx.send(frame.to_string()).is_err() {
            state.pending.lock().unwrap().remove(wait_key);
            anyhow::bail!("QFEX WS writer closed — reconnecting on next call");
        }
        let resp = tokio::time::timeout(Duration::from_secs(15), rx)
            .await
            .map_err(|_| {
                let mut p = state.pending.lock().unwrap();
                p.remove(wait_key);
                anyhow::anyhow!("QFEX order_response timed out for {wait_key}")
            })?
            .map_err(|_| anyhow::anyhow!("QFEX waiter dropped for {wait_key}"))?;
        if resp.get("status").and_then(|s| s.as_str()) == Some("__disconnected") {
            anyhow::bail!("QFEX trade socket disconnected mid-request");
        }
        Ok(resp)
    }

    async fn qfex_place(&self, order: &OrderRequest) -> anyhow::Result<OrderResponse> {
        let vsym = self.venue_symbol(&order.symbol);
        let (order_type, tif) = qfex_policy(order);
        let qty_f = order
            .quantity
            .to_string()
            .parse::<f64>()
            .map_err(|e| anyhow::anyhow!("QFEX quantity not a float: {e}"))?;
        let price_f = match order.price {
            Some(p) => p
                .to_string()
                .parse::<f64>()
                .map_err(|e| anyhow::anyhow!("QFEX price not a float: {e}"))?,
            None if order_type == "MARKET" => 0.0,
            None => anyhow::bail!("QFEX limit/ALO orders require a price"),
        };
        let resp = self
            .qfex_request(
                &order.client_order_id,
                serde_json::json!({
                    "type": "add_order",
                    "params": {
                        "symbol": vsym,
                        "side": if order.side.is_buy() { "BUY" } else { "SELL" },
                        "order_type": order_type,
                        "order_time_in_force": tif,
                        "quantity": qty_f,
                        "price": price_f,
                        "take_profit": 0,
                        "stop_loss": 0,
                        "reduce_only": order.reduce_only,
                        "client_order_id": order.client_order_id,
                    }
                }),
            )
            .await?;
        let status = resp.get("status").and_then(|s| s.as_str()).unwrap_or("");
        let order_id = resp
            .get("order_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("QFEX add_order returned no order_id: {resp}"))?
            .to_string();
        let qty = dec_from_json(resp.get("quantity")).unwrap_or(order.quantity);
        let remaining = dec_from_json(resp.get("quantity_remaining")).unwrap_or(qty);
        let st = qfex_map_status(status, qty, remaining)
            .ok_or_else(|| anyhow::anyhow!("QFEX order rejected with status '{status}': {resp}"))?;
        if matches!(st, arbitrageur_core::types::OrderStatus::Rejected) {
            anyhow::bail!("QFEX order rejected ({status}): {resp}");
        }
        Ok(self.order_response(
            order_id,
            order.client_order_id.clone(),
            &order.symbol,
            st,
            qty,
            qty - remaining,
            dec_from_json(resp.get("price")),
            resp,
        ))
    }

    async fn qfex_cancel(&self, req: &CancelRequest) -> anyhow::Result<()> {
        let vsym = self.venue_symbol(&req.symbol);
        let (id, id_type) = if let Some(i) = req.order_id.as_deref() {
            (i, "order_id")
        } else if let Some(i) = req.client_order_id.as_deref() {
            (i, "client_order_id")
        } else {
            anyhow::bail!("QFEX cancel needs an order_id or client_order_id");
        };
        let resp = self
            .qfex_request(
                id,
                serde_json::json!({
                    "type": "cancel_order",
                    "params": {
                        "order_id": id,
                        "symbol": vsym,
                        "cancel_order_id_type": id_type,
                    }
                }),
            )
            .await?;
        let status = resp.get("status").and_then(|s| s.as_str()).unwrap_or("");
        match status {
            // NO_SUCH_ORDER = already terminal — the reconciler confirms
            // via fetch_order; treating it as ok keeps cancels idempotent.
            "CANCELLED" | "NO_SUCH_ORDER" | "FILLED" => Ok(()),
            other => anyhow::bail!("QFEX cancel returned '{other}': {resp}"),
        }
    }

    async fn qfex_fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        // Live state: the WS cache holds every pushed order_response —
        // this is the authoritative open-order source (no REST exists).
        // A dead socket leaves stale rows that would report the last
        // pre-disconnect status forever — drop it and re-establish the
        // session so a fresh subscribe can repopulate open orders before
        // consulting the cache (fail closed: never trust a dead feed).
        let existing = {
            self.qfex_ws
                .read()
                .as_ref()
                .filter(|s| s.alive.load(std::sync::atomic::Ordering::Relaxed))
                .cloned()
        };
        let session = match existing {
            Some(s) => Some(s),
            None => self.qfex_session().await.ok(),
        };
        if let Some(s) = session {
            let cached = {
                let m = s.orders.lock().unwrap();
                m.get(order_id)
                    .or_else(|| client_order_id.and_then(|c| m.get(c)))
                    .cloned()
            };
            if let Some(or) = cached {
                let status = or.get("status").and_then(|v| v.as_str()).unwrap_or("");
                let qty = dec_from_json(or.get("quantity")).unwrap_or_default();
                let remaining = dec_from_json(or.get("quantity_remaining")).unwrap_or_default();
                let st = qfex_map_status(status, qty, remaining)
                    .ok_or_else(|| anyhow::anyhow!("QFEX order {order_id} status '{status}'"))?;
                let mut resp = self.order_response(
                    order_id.to_string(),
                    or.get("client_order_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    symbol,
                    st,
                    qty,
                    qty - remaining,
                    dec_from_json(or.get("price")),
                    or,
                );
                resp.fee_currency = "USD".to_string();
                return Ok(resp);
            }
        }
        // Terminal truth: historic orders covers filled/closed orders.
        let headers = self.qfex_headers()?;
        let vsym = self.venue_symbol(symbol);
        let url = format!(
            "{}/user/historic-orders?symbol={vsym}&limit=100",
            self.base_url
        );
        let data = self.get_json_url(&url, &headers).await?;
        let arr = data
            .get("data")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for e in arr {
            let id_match = e.get("order_id").and_then(|v| v.as_str()) == Some(order_id);
            let coid_match = client_order_id.is_some()
                && e.get("client_order_id").and_then(|v| v.as_str()) == client_order_id;
            if !id_match && !coid_match {
                continue;
            }
            let qty = dec_from_json(e.get("quantity")).unwrap_or_default();
            let filled = dec_from_json(e.get("filled_qty")).unwrap_or_default();
            let status_str = e
                .get("terminal_status")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let st = map_order_status(status_str, filled, qty).ok_or_else(|| {
                anyhow::anyhow!("QFEX historic order {order_id} status '{status_str}'")
            })?;
            return Ok(self.order_response(
                order_id.to_string(),
                client_order_id.unwrap_or("").to_string(),
                symbol,
                st,
                qty,
                filled,
                dec_from_json(e.get("avg_price")),
                e,
            ));
        }
        anyhow::bail!(
            "QFEX order {order_id} is unverifiable (not on trade socket nor in historic orders)"
        )
    }

    /// `GET /user/positions` — equity = deposit + realised + unrealised +
    /// net_funding (QFEX balance model); free = available_balance;
    /// maintenance = Σ per-position maintenance_margin.
    async fn qfex_account(&self) -> anyhow::Result<AccountSnapshot> {
        let headers = self.qfex_headers()?;
        let url = format!("{}/user/positions", self.base_url);
        let data = self.get_json_url(&url, &headers).await?;
        let bal = data
            .get("balance")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let deposit = dec_from_json(bal.get("deposit")).unwrap_or_default();
        let realised = dec_from_json(bal.get("realised_pnl")).unwrap_or_default();
        let unrealised = dec_from_json(bal.get("unrealised_pnl")).unwrap_or_default();
        let funding = dec_from_json(bal.get("net_funding")).unwrap_or_default();
        let equity = deposit + realised + unrealised + funding;
        let free = dec_from_json(bal.get("available_balance")).unwrap_or_default();
        let mut maint = dec!(0);
        let mut positions = Vec::new();
        if let Some(arr) = data.get("positions").and_then(|v| v.as_array()) {
            for p in arr {
                let qty = dec_from_json(p.get("position")).unwrap_or_default();
                if qty == dec!(0) {
                    continue;
                }
                maint += dec_from_json(p.get("maintenance_margin")).unwrap_or_default();
                let side = if qty < dec!(0) {
                    arbitrageur_core::types::Side::Sell
                } else {
                    arbitrageur_core::types::Side::Buy
                };
                let sym = p
                    .get("symbol")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_uppercase();
                positions.push(position_from(
                    &self.venue_id,
                    sym,
                    side,
                    qty,
                    dec_from_json(p.get("average_price")),
                    None,
                    dec_from_json(p.get("unrealised_pnl")),
                    None,
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
                currency: "USD".to_string(),
                total: equity,
                free,
                locked: (equity - free).max(dec!(0)),
            }],
            positions,
            timestamp: Utc::now(),
        })
    }
}

// ---------------------------------------------------------------------------
// Nado — Vertex-style off-chain sequencer on Ink L2. All executes are
// EIP-712 signed structs; order identity is the typed-data digest.
// ---------------------------------------------------------------------------

/// Ink L2 mainnet chain id (verified via `query?type=contracts`).
const NADO_CHAIN_ID: u64 = 57073;

/// Nado endpoint contract — the EIP-712 verifying contract for every
/// execute EXCEPT place_order (which uses `address(product_id)`).
/// Verified live via `query?type=contracts`.
const NADO_ENDPOINT_ADDR: &str = "0x05ec92d78ed421f3d3ada77ffde167106565974e";

/// How far ahead of now the nonce's recv-time window extends — orders
/// landing later than this are discarded by the matching engine.
const NADO_RECV_WINDOW_MS: u64 = 60_000;

/// EIP-712 domain separator with `verifyingContract`
/// (`EIP712Domain(string name,string version,uint256 chainId,address
/// verifyingContract)` — the Nado/Vertex domain shape).
fn eip712_domain_sep_vc(name: &str, version: &str, chain_id: u64, contract: Address) -> [u8; 32] {
    let type_hash = keccak256(
        b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
    );
    let encoded = (
        type_hash,
        keccak256(name.as_bytes()),
        keccak256(version.as_bytes()),
        U256::from(chain_id),
        contract,
    )
        .abi_encode();
    keccak256(&encoded).0
}

/// Nado fixed-point: all venue quantities are decimal strings scaled 1e18.
fn nado_x18(v: Option<&serde_json::Value>) -> Option<Decimal> {
    dec_from_json(v).map(|d| d / dec!(1000000000000000000))
}

fn nado_now_ms() -> u64 {
    Utc::now().timestamp_millis().max(0) as u64
}

/// Nado nonce: top 44 bits = recv-time ms (order discarded after), low
/// 20 bits = collision-avoidance random.
fn nado_nonce() -> u64 {
    let recv = nado_now_ms() + NADO_RECV_WINDOW_MS;
    (recv << 20) | u64::from(rand::random::<u32>() & 0xF_FFFF)
}

/// An order's effective execution policy, resolved once from the three
/// places a caller can express it. `order_type` is the semantic;
/// `time_in_force` refines a plain Limit; the legacy `post_only` flag
/// forces maker-only. Venues must map every variant they accept and
/// refuse the rest — nothing may silently downgrade a requested policy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ExecPolicy {
    Market,
    Ioc,
    Fok,
    PostOnly,
    Rest,
}

fn effective_exec_policy(order: &OrderRequest) -> ExecPolicy {
    use arbitrageur_core::types::{OrderType, TimeInForce};
    if order.order_type == OrderType::Market {
        ExecPolicy::Market
    } else if order.post_only
        || order.order_type == OrderType::PostOnly
        || order.time_in_force == TimeInForce::PostOnly
    {
        ExecPolicy::PostOnly
    } else if order.order_type == OrderType::ImmediateOrCancel
        || order.time_in_force == TimeInForce::ImmediateOrCancel
    {
        ExecPolicy::Ioc
    } else if order.time_in_force == TimeInForce::FillOrKill {
        ExecPolicy::Fok
    } else {
        ExecPolicy::Rest
    }
}

/// Appendix bit-layout (docs.nado.xyz order-appendix): version 1, order
/// type at bits 9-10 (0 default, 1 IOC, 2 FOK, 3 post-only), reduce-only
/// at bit 11. Market orders are IOC with a book-bounded limit price —
/// Nado has no native market type.
fn nado_appendix(order: &OrderRequest) -> u128 {
    use arbitrageur_core::types::{OrderType, TimeInForce};
    let order_type: u128 = if order.post_only
        || order.order_type == OrderType::PostOnly
        || order.time_in_force == TimeInForce::PostOnly
    {
        3
    } else if order.time_in_force == TimeInForce::FillOrKill {
        2
    } else if order.order_type == OrderType::Market
        || order.order_type == OrderType::ImmediateOrCancel
        || order.time_in_force == TimeInForce::ImmediateOrCancel
    {
        1
    } else {
        0
    };
    let mut appendix: u128 = 1 | (order_type << 9);
    if order.reduce_only {
        appendix |= 1 << 11;
    }
    appendix
}

/* ------------------------------------------------------------------ */
/* Per-venue wire policies — extracted as pure fns so the test matrix    */
/* can enumerate the full ExecPolicy space per venue and assert every   */
/* request either maps to a documented constant or refuses.             */
/* ------------------------------------------------------------------ */

/// Pacifica limit tif: IOC/ALO/GTC only — FOK and the market path are
/// refused (market orders use `create_market_order`, not this body).
fn pacifica_limit_tif(order: &OrderRequest) -> anyhow::Result<&'static str> {
    Ok(match effective_exec_policy(order) {
        ExecPolicy::Ioc => "IOC",
        ExecPolicy::PostOnly => "ALO",
        ExecPolicy::Rest => "GTC",
        ExecPolicy::Fok => {
            anyhow::bail!("Pacifica has no fill-or-kill — refusing instead of downgrading to GTC")
        }
        ExecPolicy::Market => {
            anyhow::bail!("Pacifica market orders use create_market_order")
        }
    })
}

/// StandX tif: ioc/alo/gtc — market orders submit as ioc (immediate by
/// construction), FOK is refused rather than downgraded to GTC.
fn standx_tif(order: &OrderRequest) -> anyhow::Result<&'static str> {
    Ok(match effective_exec_policy(order) {
        ExecPolicy::Ioc => "ioc",
        ExecPolicy::PostOnly => "alo",
        ExecPolicy::Rest => "gtc",
        ExecPolicy::Fok => {
            anyhow::bail!("StandX has no fill-or-kill — refusing instead of downgrading to GTC")
        }
        ExecPolicy::Market => "ioc",
    })
}

/// Ondo limit fields: (timeInForce, postOnly) — post-only rides its own
/// flag on a GTC row, IOC has a native tif, FOK refuses.
fn ondo_limit_policy(order: &OrderRequest) -> anyhow::Result<(&'static str, bool)> {
    Ok(match effective_exec_policy(order) {
        ExecPolicy::Ioc => ("IOC", false),
        ExecPolicy::PostOnly => ("GTC", true),
        ExecPolicy::Rest => ("GTC", false),
        ExecPolicy::Fok => {
            anyhow::bail!("Ondo has no fill-or-kill — refusing instead of downgrading to GTC")
        }
        ExecPolicy::Market => anyhow::bail!("unreachable: is_market checked"),
    })
}

/// GRVT time_in_force codes: 1 GOOD_TILL_TIME, 2 ALL_OR_NOTHING,
/// 3 IMMEDIATE_OR_CANCEL, 4 FILL_OR_KILL — plus a signed postOnly flag.
/// Every requested policy maps explicitly; nothing degrades.
fn grvt_policy(order: &OrderRequest) -> (u8, bool) {
    match effective_exec_policy(order) {
        ExecPolicy::Ioc => (3, false),
        ExecPolicy::Fok => (4, false),
        ExecPolicy::PostOnly => (2, true),
        ExecPolicy::Rest | ExecPolicy::Market => (1, false),
    }
}

/// Derive time_in_force: ioc/post_only/gtc — market rides a bounded
/// limit price under gtc; FOK has no wire form and refuses.
fn derive_tif(order: &OrderRequest) -> anyhow::Result<&'static str> {
    Ok(match effective_exec_policy(order) {
        ExecPolicy::Ioc => "ioc",
        ExecPolicy::PostOnly => "post_only",
        ExecPolicy::Rest | ExecPolicy::Market => "gtc",
        ExecPolicy::Fok => {
            anyhow::bail!("Derive has no fill-or-kill — refusing instead of downgrading to GTC")
        }
    })
}

/// Aevo signed orders carry only a post_only flag — no tif field at all.
/// IOC/FOK cannot be expressed and refuse; market submits as a bounded
/// limit (not post-only). Returns the post_only wire value.
fn aevo_policy(order: &OrderRequest) -> anyhow::Result<bool> {
    match effective_exec_policy(order) {
        ExecPolicy::Ioc | ExecPolicy::Fok => anyhow::bail!(
            "Aevo signed orders cannot express IOC/FOK — refusing instead of resting GTC"
        ),
        ExecPolicy::PostOnly => Ok(true),
        ExecPolicy::Rest | ExecPolicy::Market => Ok(false),
    }
}

/// Vest (order_type, tif): MARKET plus LIMIT with GTC/FOK. IOC maps to
/// FOK — strictly stronger than requested (all-or-nothing vs partial-ok),
/// never weaker, so the mapping cannot over-fill. Post-only refuses.
fn vest_wire_type(order: &OrderRequest) -> anyhow::Result<(&'static str, Option<&'static str>)> {
    Ok(match effective_exec_policy(order) {
        ExecPolicy::Market => ("MARKET", None),
        ExecPolicy::Ioc | ExecPolicy::Fok => ("LIMIT", Some("FOK")),
        ExecPolicy::Rest => ("LIMIT", Some("GTC")),
        ExecPolicy::PostOnly => anyhow::bail!("Vest post-only refused above"),
    })
}

/// QFEX (order_type, order_time_in_force): every requested policy has a
/// native pair — nothing refuses, nothing downgrades.
fn qfex_policy(order: &OrderRequest) -> (&'static str, &'static str) {
    match effective_exec_policy(order) {
        ExecPolicy::PostOnly => ("ALO", "GTC"),
        ExecPolicy::Market => ("MARKET", "GTC"),
        ExecPolicy::Ioc => ("LIMIT", "IOC"),
        ExecPolicy::Fok => ("LIMIT", "FOK"),
        ExecPolicy::Rest => ("LIMIT", "GTC"),
    }
}

/// The EIP-712 `Order` struct hash and its full typed-data digest under
/// the place-order domain (verifyingContract = `address(product_id)`).
fn nado_order_digest(
    sender: &[u8; 32],
    product_id: u32,
    price_x18: I256,
    amount_x18: I256,
    expiration: u64,
    nonce: u64,
    appendix: u128,
) -> [u8; 32] {
    let type_hash = keccak256(
        b"Order(bytes32 sender,int128 priceX18,int128 amount,uint64 expiration,uint64 nonce,uint128 appendix)",
    );
    let struct_hash = keccak256(
        (
            type_hash,
            alloy::primitives::B256::from(*sender),
            price_x18,
            amount_x18,
            U256::from(expiration),
            U256::from(nonce),
            U256::from(appendix),
        )
            .abi_encode(),
    );
    // address(product_id) — 20-byte big-endian of the numeric id.
    let mut contract_bytes = [0u8; 20];
    contract_bytes[16..].copy_from_slice(&product_id.to_be_bytes());
    let domain = eip712_domain_sep_vc(
        "Nado",
        "0.0.1",
        NADO_CHAIN_ID,
        Address::from(contract_bytes),
    );
    eip712_digest(&domain, &struct_hash.0)
}

/// The EIP-712 `Cancellation` struct hash and digest under the endpoint
/// domain. Dynamic arrays hash as `keccak256(concat(encode(elem)))` per
/// EIP-712 — built manually here.
fn nado_cancellation_digest(
    sender: &[u8; 32],
    product_ids: &[u32],
    digests: &[[u8; 32]],
    nonce: u64,
) -> [u8; 32] {
    let type_hash = keccak256(
        b"Cancellation(bytes32 sender,uint32[] productIds,bytes32[] digests,uint64 nonce)",
    );
    let mut pbuf = Vec::with_capacity(32 * product_ids.len());
    for pid in product_ids {
        pbuf.extend_from_slice(&U256::from(*pid).to_be_bytes::<32>());
    }
    let pids_hash = keccak256(&pbuf);
    let mut dbuf = Vec::with_capacity(32 * digests.len());
    for d in digests {
        dbuf.extend_from_slice(d);
    }
    let digests_hash = keccak256(&dbuf);
    let struct_hash = keccak256(
        (
            type_hash,
            alloy::primitives::B256::from(*sender),
            pids_hash,
            digests_hash,
            U256::from(nonce),
        )
            .abi_encode(),
    );
    let contract: Address = NADO_ENDPOINT_ADDR
        .parse()
        .expect("NADO_ENDPOINT_ADDR is a fixed literal");
    let domain = eip712_domain_sep_vc("Nado", "0.0.1", NADO_CHAIN_ID, contract);
    eip712_digest(&domain, &struct_hash.0)
}

impl DexPerpAdapter {
    /// `bytes32` subaccount sender: `address(20B) || subaccount_name(12B)`.
    /// `account_id` doubles as the subaccount name (≤12 ASCII bytes,
    /// null-padded; Nado's default subaccount is literally "default").
    fn nado_sender_bytes(&self) -> anyhow::Result<[u8; 32]> {
        let (addr, name) = {
            let c = self.creds.read();
            let addr = c.wallet_address.clone().ok_or_else(|| {
                anyhow::anyhow!("Nado requires the funded account address (walletAddress field)")
            })?;
            (
                addr,
                c.account_id.clone().unwrap_or_else(|| "default".into()),
            )
        };
        let addr: Address = addr
            .parse()
            .map_err(|e| anyhow::anyhow!("Nado walletAddress is not a 0x address: {e}"))?;
        let nb = name.as_bytes();
        if nb.len() > 12 || nb.is_empty() {
            anyhow::bail!("Nado subaccount name must be 1-12 ASCII bytes (got {nb:?})");
        }
        if !nb.iter().all(|b| b.is_ascii()) {
            anyhow::bail!("Nado subaccount name must be ASCII (got {name:?})");
        }
        let mut out = [0u8; 32];
        out[..20].copy_from_slice(addr.as_slice());
        out[20..20 + nb.len()].copy_from_slice(nb);
        Ok(out)
    }

    /// `symbol → product_id` for live perps, cached for the process.
    async fn nado_products(&self) -> anyhow::Result<HashMap<String, u32>> {
        if let Some(m) = self.nado_products.read().as_ref() {
            return Ok(m.clone());
        }
        let data = self
            .get_json("/gateway/v1/query?type=symbols&product_type=perp")
            .await?;
        let symbols = data
            .get("data")
            .and_then(|d| d.get("symbols"))
            .and_then(|s| s.as_object())
            .ok_or_else(|| anyhow::anyhow!("Nado symbols query missing data.symbols"))?;
        let mut map = HashMap::new();
        for e in symbols.values() {
            if e.get("type").and_then(|s| s.as_str()) != Some("perp") {
                continue;
            }
            if let (Some(sym), Some(pid)) = (
                e.get("symbol").and_then(|s| s.as_str()),
                e.get("product_id").and_then(|v| v.as_u64()),
            ) {
                map.insert(sym.to_string(), pid as u32);
            }
        }
        *self.nado_products.write() = Some(map.clone());
        Ok(map)
    }

    async fn nado_product_id(&self, symbol: &str) -> anyhow::Result<u32> {
        let vsym = self.venue_symbol(symbol);
        self.nado_products()
            .await?
            .get(&vsym)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("Nado has no perp product for {vsym}"))
    }

    /// Sign a Nado digest with the held key (main account key or a
    /// linked signer — the venue checks authorization at execute time).
    fn nado_sign(&self, digest: &[u8; 32]) -> anyhow::Result<String> {
        let w = self.wallet.read();
        let s = w.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Nado requires an EVM signing key (privateKey field)")
        })?;
        Ok(format!("0x{}", hex::encode(s.sign_digest(digest)?)))
    }

    /// `POST /gateway/v1/execute {place_order: …}` — EIP-712 `Order`
    /// under `address(product_id)` domain. The returned/local digest is
    /// the durable order id used for cancel and venue-truth polling.
    async fn nado_place(&self, order: &OrderRequest) -> anyhow::Result<OrderResponse> {
        use arbitrageur_core::types::OrderType;
        // Resolve auth material before touching the book — credential
        // failure must not even read market data.
        let sender = self.nado_sender_bytes()?;
        let product_id = self.nado_product_id(&order.symbol).await?;
        if order.quantity <= dec!(0) {
            anyhow::bail!("Nado order size must be positive");
        }
        let limit_price = match order.order_type {
            OrderType::Limit | OrderType::PostOnly => order
                .price
                .ok_or_else(|| anyhow::anyhow!("Nado limit order requires a price"))?,
            _ => self.market_bound_price(order).await?,
        };
        let price_x18 = dec_to_i256(limit_price, 18)
            .ok_or_else(|| anyhow::anyhow!("Nado price {limit_price} overflows x18"))?;
        let amount = if order.side.is_buy() {
            order.quantity
        } else {
            -order.quantity
        };
        let amount_x18 = dec_to_i256(amount, 18)
            .ok_or_else(|| anyhow::anyhow!("Nado quantity {amount} overflows x18"))?;
        // Resting orders run until u32::MAX; immediate orders get a short
        // expiry so a delayed landing can't rest on the book.
        let immediate = matches!(
            order.order_type,
            OrderType::Market | OrderType::ImmediateOrCancel
        ) || matches!(
            order.time_in_force,
            arbitrageur_core::types::TimeInForce::ImmediateOrCancel
                | arbitrageur_core::types::TimeInForce::FillOrKill
        );
        let expiration: u64 = if immediate {
            (nado_now_ms() / 1000) + 90
        } else {
            u64::from(u32::MAX)
        };
        let nonce = nado_nonce();
        let appendix = nado_appendix(order);
        let digest = nado_order_digest(
            &sender, product_id, price_x18, amount_x18, expiration, nonce, appendix,
        );
        let signature = self.nado_sign(&digest)?;
        let sender_hex = format!("0x{}", hex::encode(sender));
        let body = serde_json::json!({
            "place_order": {
                "product_id": product_id,
                "order": {
                    "sender": sender_hex,
                    "priceX18": price_x18.to_string(),
                    "amount": amount_x18.to_string(),
                    "expiration": expiration.to_string(),
                    "nonce": nonce.to_string(),
                    "appendix": appendix.to_string(),
                },
                "signature": signature,
            }
        });
        let url = format!("{}/gateway/v1/execute", self.base_url);
        let resp = self.post_json_url(&url, &body, &[]).await?;
        if resp.get("status").and_then(|s| s.as_str()) != Some("success") {
            let err = resp.get("error").and_then(|e| e.as_str()).unwrap_or("");
            let code = resp.get("error_code").and_then(|c| c.as_i64()).unwrap_or(0);
            anyhow::bail!("Nado place_order rejected: {err} (code {code})");
        }
        let local = format!("0x{}", hex::encode(digest));
        let id = resp
            .get("data")
            .and_then(|d| d.get("digest"))
            .or_else(|| resp.get("digest"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or(local);
        Ok(self.order_response(
            id,
            order.client_order_id.clone(),
            &order.symbol,
            arbitrageur_core::types::OrderStatus::New,
            order.quantity,
            dec!(0),
            Some(limit_price),
            resp,
        ))
    }

    /// `POST /gateway/v1/execute {cancel_orders: …}` — EIP-712
    /// `Cancellation` under the endpoint domain. `order_id` is the
    /// order's digest.
    async fn nado_cancel(&self, req: &CancelRequest) -> anyhow::Result<()> {
        let digest_hex = req
            .order_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Nado cancel needs the venue order digest"))?;
        let digest_bytes = hex::decode(digest_hex.trim_start_matches("0x"))
            .map_err(|e| anyhow::anyhow!("Nado order id is not a bytes32 digest: {e}"))?;
        let digest: [u8; 32] = digest_bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("Nado order digest must be 32 bytes"))?;
        let sender = self.nado_sender_bytes()?;
        let product_id = self.nado_product_id(&req.symbol).await?;
        let nonce = nado_nonce();
        let cdigest = nado_cancellation_digest(&sender, &[product_id], &[digest], nonce);
        let signature = self.nado_sign(&cdigest)?;
        let sender_hex = format!("0x{}", hex::encode(sender));
        let body = serde_json::json!({
            "cancel_orders": {
                "tx": {
                    "sender": sender_hex,
                    "productIds": [product_id],
                    "digests": [digest_hex],
                    "nonce": nonce.to_string(),
                },
                "signature": signature,
            }
        });
        let url = format!("{}/gateway/v1/execute", self.base_url);
        let resp = self.post_json_url(&url, &body, &[]).await?;
        if resp.get("status").and_then(|s| s.as_str()) != Some("success") {
            let err = resp.get("error").and_then(|e| e.as_str()).unwrap_or("");
            let code = resp.get("error_code").and_then(|c| c.as_i64()).unwrap_or(0);
            anyhow::bail!("Nado cancel_orders rejected: {err} (code {code})");
        }
        Ok(())
    }

    /// Venue truth: the gateway `order` query returns only on-book
    /// (live) orders. A missing order is then disambiguated through the
    /// archive `orders` query — `base_filled` vs `amount` decides Filled
    /// vs Cancelled. Absent from both → bail (unverifiable, stays open).
    async fn nado_fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        _client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        use arbitrageur_core::types::OrderStatus;
        let product_id = self.nado_product_id(symbol).await?;
        let digest_norm = format!("0x{}", order_id.trim_start_matches("0x"));
        let url = format!(
            "{}/gateway/v1/query?type=order&product_id={product_id}&digest={digest_norm}",
            self.base_url
        );
        let data = self.get_json_url(&url, &[]).await?;
        if data.get("status").and_then(|s| s.as_str()) == Some("success") {
            let d = data
                .get("data")
                .ok_or_else(|| anyhow::anyhow!("Nado order query missing data"))?;
            let amount = nado_x18(d.get("amount"))
                .ok_or_else(|| anyhow::anyhow!("Nado order missing amount"))?
                .abs();
            let unfilled = nado_x18(d.get("unfilled_amount"))
                .ok_or_else(|| anyhow::anyhow!("Nado order missing unfilled_amount"))?
                .abs();
            let filled = (amount - unfilled).max(dec!(0));
            let status = if unfilled <= dec!(0) {
                OrderStatus::Filled
            } else if filled > dec!(0) {
                OrderStatus::PartiallyFilled
            } else {
                OrderStatus::New
            };
            return Ok(self.order_response(
                order_id.to_string(),
                String::new(),
                symbol,
                status,
                amount,
                filled,
                nado_x18(d.get("price_x18")),
                d.clone(),
            ));
        }
        // Off-book: archive decides between filled and cancelled.
        let url = format!("{}/archive/v1", self.base_url);
        let data = self
            .post_json_url(
                &url,
                &serde_json::json!({"orders": {"digests": [digest_norm], "limit": 1}}),
                &[],
            )
            .await?;
        let arr = data
            .get("orders")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow::anyhow!("Nado archive orders missing array"))?;
        let entry = arr
            .iter()
            .find(|e| {
                e.get("digest")
                    .and_then(|v| v.as_str())
                    .map(|s| {
                        s.trim_start_matches("0x")
                            .eq_ignore_ascii_case(order_id.trim_start_matches("0x"))
                    })
                    .unwrap_or(false)
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Nado order {order_id} is neither on-book nor in archive — unverifiable"
                )
            })?;
        let amount = nado_x18(entry.get("amount"))
            .ok_or_else(|| anyhow::anyhow!("Nado archive order missing amount"))?
            .abs();
        let filled = nado_x18(entry.get("base_filled")).unwrap_or_default().abs();
        let status = if filled >= amount && amount > dec!(0) {
            OrderStatus::Filled
        } else {
            OrderStatus::Cancelled
        };
        Ok(self.order_response(
            order_id.to_string(),
            String::new(),
            symbol,
            status,
            amount,
            filled,
            nado_x18(entry.get("price_x18")),
            entry.clone(),
        ))
    }

    /// `GET /gateway/v1/query?type=subaccount_info` — equity is the
    /// unweighted health (`healths[2]`); free margin is initial health
    /// (`healths[0]`). Perp balances map back through the product map.
    async fn nado_account(&self) -> anyhow::Result<AccountSnapshot> {
        let sender = self.nado_sender_bytes()?;
        let sender_hex = format!("0x{}", hex::encode(sender));
        let url = format!(
            "{}/gateway/v1/query?type=subaccount_info&subaccount={sender_hex}",
            self.base_url
        );
        let data = self.get_json_url(&url, &[]).await?;
        let d = data
            .get("data")
            .ok_or_else(|| anyhow::anyhow!("Nado subaccount_info missing data"))?;
        let products = self.nado_products().await.unwrap_or_default();
        let pid_to_sym: HashMap<u32, String> =
            products.iter().map(|(s, p)| (*p, s.clone())).collect();
        if d.get("exists").and_then(|v| v.as_bool()) == Some(false) {
            return Ok(AccountSnapshot {
                venue: self.venue_id.clone(),
                equity: dec!(0),
                free_margin: dec!(0),
                maintenance_margin: dec!(0),
                margin_usage_pct: dec!(0),
                balances: Vec::new(),
                positions: Vec::new(),
                timestamp: Utc::now(),
            });
        }
        let health = |i: usize| -> Decimal {
            d.get("healths")
                .and_then(|h| h.as_array())
                .and_then(|a| a.get(i))
                .and_then(|h| nado_x18(h.get("health")))
                .unwrap_or_default()
        };
        let equity = health(2);
        let free = health(0);
        let maint = (equity - health(1)).max(dec!(0));
        // Oracle marks for unrealized PnL on open perp balances.
        let mut marks: HashMap<u32, Decimal> = HashMap::new();
        if let Some(arr) = d.get("perp_products").and_then(|v| v.as_array()) {
            for p in arr {
                if let (Some(pid), Some(px)) = (
                    p.get("product_id")
                        .and_then(|v| v.as_u64())
                        .and_then(|v| u32::try_from(v).ok()),
                    nado_x18(p.get("oracle_price_x18")),
                ) {
                    marks.insert(pid, px);
                }
            }
        }
        let mut balances = Vec::new();
        if let Some(arr) = d.get("spot_balances").and_then(|v| v.as_array()) {
            for b in arr {
                let amt = nado_x18(b.pointer("/balance/amount")).unwrap_or_default();
                if amt == dec!(0) {
                    continue;
                }
                // A missing/oversized product_id is not product 0 (USDT0) —
                // skip the row rather than mislabel the currency.
                let Some(pid) = b
                    .get("product_id")
                    .and_then(|v| v.as_u64())
                    .and_then(|v| u32::try_from(v).ok())
                else {
                    continue;
                };
                // Product 0 is the USDT0 quote asset itself.
                let currency = if pid == 0 {
                    "USDT0".to_string()
                } else {
                    pid_to_sym
                        .get(&pid)
                        .and_then(|s| s.strip_suffix("-PERP"))
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| format!("PID{pid}"))
                };
                balances.push(arbitrageur_core::position::Balance {
                    venue: self.venue_id.clone(),
                    currency,
                    total: amt,
                    free: amt,
                    locked: dec!(0),
                });
            }
        }
        let mut positions = Vec::new();
        if let Some(arr) = d.get("perp_balances").and_then(|v| v.as_array()) {
            for b in arr {
                let amt = nado_x18(b.pointer("/balance/amount")).unwrap_or_default();
                if amt == dec!(0) {
                    continue;
                }
                let Some(pid) = b
                    .get("product_id")
                    .and_then(|v| v.as_u64())
                    .and_then(|v| u32::try_from(v).ok())
                else {
                    continue;
                };
                let vsym = pid_to_sym.get(&pid).cloned().unwrap_or_default();
                let sym = vsym.strip_suffix("-PERP").unwrap_or(&vsym).to_uppercase();
                let side = if amt < dec!(0) {
                    arbitrageur_core::types::Side::Sell
                } else {
                    arbitrageur_core::types::Side::Buy
                };
                // v_quote_balance carries cumulative quote spent — entry
                // is |v_quote| / |amount| (both x18, ratio is plain).
                let vq = nado_x18(b.pointer("/balance/v_quote_balance")).unwrap_or_default();
                let entry = if amt != dec!(0) {
                    Some((vq / amt).abs())
                } else {
                    None
                };
                let mark = marks.get(&pid).copied();
                let upnl = match (mark, entry) {
                    (Some(m), Some(e)) => {
                        let dir = if amt > dec!(0) { dec!(1) } else { dec!(-1) };
                        Some((m - e) * amt.abs() * dir)
                    }
                    _ => None,
                };
                positions.push(position_from(
                    &self.venue_id,
                    sym,
                    side,
                    amt.abs(),
                    entry,
                    mark,
                    upnl,
                    None,
                    None,
                ));
            }
        }
        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity,
            free_margin: free.max(dec!(0)),
            maintenance_margin: maint,
            margin_usage_pct: if equity > dec!(0) {
                maint / equity * dec!(100)
            } else {
                dec!(0)
            },
            balances,
            positions,
            timestamp: Utc::now(),
        })
    }
}

// ---------------------------------------------------------------------------
// EVEDEX — SIWE→JWT session auth + per-order EIP-712 signatures
// (exchange-api.evedex.com trading host; auth-api.evedex.com mints tokens;
// signatures bind the Arbitrum-One domain)
// ---------------------------------------------------------------------------

/// Auth host: `/auth/nonce`, `/auth/user/sign-up`, `/auth/refresh`.
const EVEDEX_AUTH_URL: &str = "https://auth-api.evedex.com";

/// Signing chainId — SDK PROD params pin `arbitrum_one` (42161). It is
/// both an EIP-712 domain field and a `chainId` message field.
const EVEDEX_CHAIN_ID: u64 = 42161;

/// EIP-712 domain salt (official `exchange-crypto` constants).
const EVEDEX_DOMAIN_SALT: &str =
    "0x5792f7333c35db190e30acc144f049fd15b24f552c0010b8b3e06f9105c37c5a";

/// Matcher precision: REST order fields are ≤8dp decimal strings and
/// signatures commit to `value × 1e8` integers (SDK `toMatcherNumber` /
/// `toEthNumber`, MATCHER_PRECISION = 8).
const EVEDEX_MATCHER_SCALE: u32 = 8;

/// `generateOrderIdV2` day epoch: 2025-07-24T00:00:00Z.
const EVEDEX_ORDER_ID_EPOCH_MS: i64 = 1_753_315_200_000;

/// EIP-712 domain separator for `EIP712Domain(string name,string
/// version,uint256 chainId,bytes32 salt)` — salt, no verifyingContract.
fn evedex_domain_sep() -> [u8; 32] {
    let salt_bytes = hex::decode(EVEDEX_DOMAIN_SALT.trim_start_matches("0x"))
        .expect("EVEDEX_DOMAIN_SALT is a fixed hex literal");
    let salt: [u8; 32] = salt_bytes
        .try_into()
        .expect("EVEDEX_DOMAIN_SALT is 32 bytes");
    let type_hash =
        keccak256(b"EIP712Domain(string name,string version,uint256 chainId,bytes32 salt)");
    let encoded = (
        type_hash,
        keccak256(b"EVEDEX"),
        keccak256(b"2"),
        U256::from(EVEDEX_CHAIN_ID),
        alloy::primitives::B256::from(salt),
    )
        .abi_encode();
    keccak256(&encoded).0
}

/// Client order id `{days:05d}:{26 hex}` per SDK `generateOrderIdV2`.
/// When the caller supplies a `client_order_id` the suffix is derived
/// deterministically from it: a retry after a lost ack regenerates the
/// SAME venue id, so the matcher dedupes the resubmission instead of
/// silently opening a second real order. Callers without an id get the
/// historical random suffix.
fn evedex_order_id(client_order_id: Option<&str>) -> String {
    let days = ((Utc::now().timestamp_millis() - EVEDEX_ORDER_ID_EPOCH_MS) / 86_400_000).max(0);
    let suffix = match client_order_id.map(str::trim).filter(|c| !c.is_empty()) {
        Some(cid) => {
            let h = keccak256(cid.as_bytes());
            hex::encode(&h[..13])
        }
        None => hex::encode(rand::random::<[u8; 13]>()),
    };
    format!("{days:05}:{suffix}")
}

/// SDK `toMatcherNumber`: round half-away-from-zero at 8dp, plain
/// decimal string (what the REST body carries).
fn evedex_matcher_num(v: Decimal) -> String {
    v.round_dp_with_strategy(
        EVEDEX_MATCHER_SCALE,
        rust_decimal::RoundingStrategy::MidpointAwayFromZero,
    )
    .normalize()
    .to_string()
}

/// SDK `toEthNumber`: `round(v × 1e8)` as U256 for signed uint fields.
fn evedex_uint(v: Decimal, what: &str) -> anyhow::Result<U256> {
    dec_to_u256(v, EVEDEX_MATCHER_SCALE)
        .ok_or_else(|| anyhow::anyhow!("EVEDEX {what} {v} overflows the 1e8 matcher scale"))
}

/// EIP-4361 message exactly as siwe@3 `prepareMessage` emits for the
/// SDK params: `scheme: https` folds into the domain line, statement is
/// `Sign in to evedex.com`, no expirationTime.
fn evedex_siwe_message(address: &str, nonce: &str, issued_at: &str) -> String {
    format!(
        "https://evedex.com wants you to sign in with your Ethereum account:\n{address}\n\nSign in to evedex.com\n\nURI: https://evedex.com\nVersion: 1\nChain ID: {EVEDEX_CHAIN_ID}\nNonce: {nonce}\nIssued At: {issued_at}"
    )
}

/// `New limit order` typed-data digest (string fields hash as
/// `keccak256(utf8)` per EIP-712).
fn evedex_limit_digest(
    id: &str,
    instrument: &str,
    side: &str,
    leverage: u8,
    quantity: U256,
    limit_price: U256,
) -> [u8; 32] {
    let type_hash = keccak256(
        b"New limit order(string id,string instrument,string side,uint8 leverage,uint96 quantity,uint80 limitPrice,uint256 chainId)",
    );
    let struct_hash = keccak256(
        (
            type_hash,
            keccak256(id.as_bytes()),
            keccak256(instrument.as_bytes()),
            keccak256(side.as_bytes()),
            U256::from(leverage),
            quantity,
            limit_price,
            U256::from(EVEDEX_CHAIN_ID),
        )
            .abi_encode(),
    );
    eip712_digest(&evedex_domain_sep(), &struct_hash.0)
}

/// `New market order` typed-data digest — `cashQuantity` is USD
/// notional at 1e8 scale, `timeInForce` is IOC/FOK.
fn evedex_market_digest(
    id: &str,
    instrument: &str,
    side: &str,
    time_in_force: &str,
    leverage: u8,
    cash_quantity: U256,
) -> [u8; 32] {
    let type_hash = keccak256(
        b"New market order(string id,string instrument,string side,string timeInForce,uint8 leverage,uint96 cashQuantity,uint256 chainId)",
    );
    let struct_hash = keccak256(
        (
            type_hash,
            keccak256(id.as_bytes()),
            keccak256(instrument.as_bytes()),
            keccak256(side.as_bytes()),
            keccak256(time_in_force.as_bytes()),
            U256::from(leverage),
            cash_quantity,
            U256::from(EVEDEX_CHAIN_ID),
        )
            .abi_encode(),
    );
    eip712_digest(&evedex_domain_sep(), &struct_hash.0)
}

/// EVEDEX order status → engine status. `INTENTION` = accepted but not
/// yet processed (async pipeline) — maps to New, never Filled. Unknown
/// strings return None so callers fail closed.
fn evedex_status(s: &str) -> Option<arbitrageur_core::types::OrderStatus> {
    use arbitrageur_core::types::OrderStatus::*;
    match s {
        "INTENTION" | "NEW" => Some(New),
        "PARTIALLY_FILLED" => Some(PartiallyFilled),
        "FILLED" => Some(Filled),
        "CANCELLED" | "REPLACED" => Some(Cancelled),
        "REJECTED" | "ERROR" => Some(Rejected),
        "EXPIRED" => Some(Expired),
        _ => None,
    }
}

impl DexPerpAdapter {
    /// Validate the trading identity and return the signer's checksummed
    /// address: the SIWE message embeds it, so walletAddress must equal
    /// the key's derived address or auth recovers a different account.
    fn evedex_identity(&self) -> anyhow::Result<String> {
        let (acct, derived) = {
            let c = self.creds.read();
            let w = self.wallet.read();
            let signer = w.as_ref().ok_or_else(|| {
                anyhow::anyhow!("EVEDEX requires an EVM signing key (privateKey field)")
            })?;
            (
                c.wallet_address.clone().ok_or_else(|| {
                    anyhow::anyhow!("EVEDEX requires the account address (walletAddress field)")
                })?,
                signer.address_checksum(),
            )
        };
        if !acct.eq_ignore_ascii_case(&derived) {
            anyhow::bail!(
                "EVEDEX walletAddress {acct} does not match the signing key's address {derived}"
            );
        }
        Ok(derived)
    }

    fn evedex_sign(&self, digest: &[u8; 32]) -> anyhow::Result<String> {
        let w = self.wallet.read();
        let s = w.as_ref().ok_or_else(|| {
            anyhow::anyhow!("EVEDEX requires an EVM signing key (privateKey field)")
        })?;
        Ok(format!("0x{}", hex::encode(s.sign_digest(digest)?)))
    }

    /// Full SIWE login: nonce → EIP-4361 message → personal_sign →
    /// `POST /auth/user/sign-up` → `{token:{accessToken,refreshToken}}`.
    async fn evedex_login(&self) -> anyhow::Result<()> {
        let address = self.evedex_identity()?;
        let nonce_data = self
            .get_json_url(&format!("{EVEDEX_AUTH_URL}/auth/nonce"), &[])
            .await?;
        let nonce = nonce_data
            .get("nonce")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("EVEDEX auth nonce missing from response"))?
            .to_string();
        let issued = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let message = evedex_siwe_message(&address, &nonce, &issued);
        let signature = {
            let w = self.wallet.read();
            let s = w.as_ref().ok_or_else(|| {
                anyhow::anyhow!("EVEDEX requires an EVM signing key (privateKey field)")
            })?;
            format!(
                "0x{}",
                hex::encode(s.sign_personal_message(message.as_bytes())?)
            )
        };
        let body = serde_json::json!({
            "wallet": address,
            "message": message,
            "nonce": nonce,
            "signature": signature,
        });
        let resp = self
            .post_json_url(&format!("{EVEDEX_AUTH_URL}/auth/user/sign-up"), &body, &[])
            .await?;
        let access = resp
            .pointer("/token/accessToken")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("EVEDEX sign-up returned no accessToken"))?
            .to_string();
        let refresh = resp
            .pointer("/token/refreshToken")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        *self.evedex_tokens.write() = Some(EvedexTokens { access, refresh });
        Ok(())
    }

    /// `POST /auth/refresh` with `Bearer {refreshToken}` — rotates both
    /// tokens (the response's refreshToken replaces the old one).
    async fn evedex_refresh(&self) -> anyhow::Result<()> {
        let refresh = self
            .evedex_tokens
            .read()
            .as_ref()
            .and_then(|t| t.refresh.clone())
            .ok_or_else(|| anyhow::anyhow!("EVEDEX session has no refresh token"))?;
        let resp = self
            .post_json_url(
                &format!("{EVEDEX_AUTH_URL}/auth/refresh"),
                &serde_json::json!({}),
                &[("Authorization".to_string(), format!("Bearer {refresh}"))],
            )
            .await?;
        let access = resp
            .pointer("/token/accessToken")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("EVEDEX refresh returned no accessToken"))?
            .to_string();
        let new_refresh = resp
            .pointer("/token/refreshToken")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or(Some(refresh));
        *self.evedex_tokens.write() = Some(EvedexTokens {
            access,
            refresh: new_refresh,
        });
        Ok(())
    }

    /// Authed request against the trading host: `Bearer {accessToken}`,
    /// one 401 retry after refresh-or-relogin (per the auth docs).
    async fn evedex_request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> anyhow::Result<serde_json::Value> {
        self.rate_limiter.acquire_signed().await?;
        for attempt in 0..2u8 {
            if self.evedex_tokens.read().is_none() {
                self.evedex_login().await?;
            }
            let token = self
                .evedex_tokens
                .read()
                .as_ref()
                .map(|t| t.access.clone())
                .ok_or_else(|| anyhow::anyhow!("EVEDEX login produced no session"))?;
            let url = format!("{}{}", self.base_url, path);
            let mut req = self
                .client
                .request(method.clone(), &url)
                .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"));
            if let Some(b) = body {
                req = req.json(b);
            }
            let resp = req.send().await.inspect_err(|_| {
                self.rate_limiter.record_failure(false);
            })?;
            if resp.status() == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                if self.evedex_refresh().await.is_err() {
                    self.evedex_login().await?;
                }
                continue;
            }
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                self.rate_limiter
                    .record_failure(status == reqwest::StatusCode::TOO_MANY_REQUESTS);
                anyhow::bail!(
                    "EVEDEX API error: {} — {}",
                    status,
                    text.trim().chars().take(300).collect::<String>()
                );
            }
            return serde_json::from_str(&text)
                .map_err(|e| anyhow::anyhow!("EVEDEX unparseable JSON for {method} {path}: {e}"));
        }
        unreachable!("EVEDEX request loop exits via return or bail")
    }

    /// Mark price for `instrument` (`BTCUSD`) from the metrics listing —
    /// used to size market-order `cashQuantity` (USD notional).
    async fn evedex_mark_price(&self, instrument: &str) -> anyhow::Result<Decimal> {
        let data = self
            .get_json("/api/market/instrument?fields=metrics")
            .await?;
        let arr = data
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("EVEDEX instruments missing array"))?;
        for e in arr {
            if e.get("name").and_then(|s| s.as_str()) == Some(instrument) {
                return dec_from_json(e.get("markPrice"))
                    .or_else(|| dec_from_json(e.get("lastPrice")))
                    .ok_or_else(|| anyhow::anyhow!("EVEDEX has no mark price for {instrument}"));
            }
        }
        anyhow::bail!("EVEDEX has no instrument {instrument}")
    }

    /// Shape an EVEDEX order object (from place/fetch responses) into
    /// the engine's OrderResponse — fails closed on unknown status.
    fn evedex_parse_order(
        &self,
        resp: &serde_json::Value,
        symbol: &str,
        fallback_id: &str,
        client_order_id: &str,
        fallback_qty: Decimal,
    ) -> anyhow::Result<OrderResponse> {
        let status_s = resp
            .get("status")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("EVEDEX order response missing status"))?;
        let status = evedex_status(status_s)
            .ok_or_else(|| anyhow::anyhow!("EVEDEX returned unknown order status {status_s}"))?;
        let qty = dec_from_json(resp.get("quantity")).unwrap_or(fallback_qty);
        let unfilled = dec_from_json(resp.get("unFilledQuantity")).unwrap_or(qty);
        let filled = (qty - unfilled).max(dec!(0));
        let avg = dec_from_json(resp.get("filledAvgPrice"))
            .or_else(|| dec_from_json(resp.get("limitPrice")));
        let id = resp
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| fallback_id.to_string());
        let mut out = self.order_response(
            id,
            client_order_id.to_string(),
            symbol,
            status,
            qty,
            filled,
            avg,
            resp.clone(),
        );
        if let Some(fee) = resp
            .get("fee")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
        {
            if let (Some(coin), Some(q)) = (
                fee.get("coin").and_then(|v| v.as_str()),
                dec_from_json(fee.get("quantity")),
            ) {
                out.fee = Some(q);
                out.fee_currency = coin.to_string();
            }
        }
        Ok(out)
    }

    /// `POST /api/v2/order/limit` or `/api/v2/order/market` — per-order
    /// EIP-712 signature over the EVEDEX domain. Order processing is
    /// async (`INTENTION` → `NEW` → terminal), so the response is an
    /// acknowledgement and truth comes from `evedex_fetch_order`.
    async fn evedex_place(&self, order: &OrderRequest) -> anyhow::Result<OrderResponse> {
        use arbitrageur_core::types::{OrderType, TimeInForce};
        if order.reduce_only {
            anyhow::bail!(
                "EVEDEX order endpoints carry no reduce-only flag; refusing to submit an unflagged order"
            );
        }
        if order.quantity <= dec!(0) {
            anyhow::bail!("EVEDEX order size must be positive");
        }
        // Auth material resolves before any market-data read.
        let _ = self.evedex_identity()?;
        let instrument = self.venue_symbol(&order.symbol);
        let side = if order.side.is_buy() { "BUY" } else { "SELL" };
        let id = evedex_order_id(Some(order.client_order_id.as_str()));
        // EVEDEX orders carry leverage (1–300); the engine has no
        // leverage concept — 1x keeps orders fully margined.
        let leverage: u8 = 1;
        let post_only = order.post_only
            || order.order_type == OrderType::PostOnly
            || order.time_in_force == TimeInForce::PostOnly;
        let chain_id = EVEDEX_CHAIN_ID.to_string();

        if order.order_type == OrderType::Market {
            if post_only {
                anyhow::bail!("EVEDEX market orders cannot be post-only");
            }
            // `cashQuantity` is USD notional, not base size — derive it
            // from the live mark price (venue truth reconciles fills).
            let mark = self.evedex_mark_price(&instrument).await?;
            let cash = order.quantity * mark;
            if cash < dec!(5) {
                anyhow::bail!("EVEDEX minimum notional is $5 (order is ${cash})");
            }
            let cash_s = evedex_matcher_num(cash);
            let cash_u = dec_to_u256(
                Decimal::from_str(&cash_s)
                    .map_err(|e| anyhow::anyhow!("EVEDEX cashQuantity parse: {e}"))?,
                EVEDEX_MATCHER_SCALE,
            )
            .ok_or_else(|| anyhow::anyhow!("EVEDEX cashQuantity overflows uint96"))?;
            let tif = if order.time_in_force == TimeInForce::FillOrKill {
                "FOK"
            } else {
                "IOC"
            };
            let digest = evedex_market_digest(&id, &instrument, side, tif, leverage, cash_u);
            let signature = self.evedex_sign(&digest)?;
            let body = serde_json::json!({
                "id": id,
                "instrument": instrument,
                "side": side,
                "timeInForce": tif,
                "leverage": leverage,
                "cashQuantity": cash_s,
                "chainId": chain_id,
                "signature": signature,
            });
            let resp = self
                .evedex_request(reqwest::Method::POST, "/api/v2/order/market", Some(&body))
                .await?;
            return self.evedex_parse_order(
                &resp,
                &order.symbol,
                &id,
                &order.client_order_id,
                order.quantity,
            );
        }

        // Limit path — Limit / PostOnly / ImmediateOrCancel all carry a
        // price (IOC falls back to the book-bounded bound when absent).
        let price = match order.price {
            Some(p) if p > dec!(0) => p,
            _ if order.order_type == OrderType::ImmediateOrCancel => {
                self.market_bound_price(order).await?
            }
            _ => anyhow::bail!("EVEDEX limit order requires a positive price"),
        };
        if order.quantity * price < dec!(5) {
            anyhow::bail!(
                "EVEDEX minimum notional is $5 (order is ${})",
                order.quantity * price
            );
        }
        let qty_s = evedex_matcher_num(order.quantity);
        let px_s = evedex_matcher_num(price);
        let qty_u = evedex_uint(
            Decimal::from_str(&qty_s).map_err(|e| anyhow::anyhow!("EVEDEX quantity parse: {e}"))?,
            "quantity",
        )?;
        let px_u = evedex_uint(
            Decimal::from_str(&px_s)
                .map_err(|e| anyhow::anyhow!("EVEDEX limitPrice parse: {e}"))?,
            "limitPrice",
        )?;
        let digest = evedex_limit_digest(&id, &instrument, side, leverage, qty_u, px_u);
        let signature = self.evedex_sign(&digest)?;
        let tif = if order.order_type == OrderType::ImmediateOrCancel
            || order.time_in_force == TimeInForce::ImmediateOrCancel
        {
            Some("IOC")
        } else if order.time_in_force == TimeInForce::FillOrKill {
            Some("FOK")
        } else {
            None
        };
        let mut body = serde_json::json!({
            "id": id,
            "instrument": instrument,
            "side": side,
            "leverage": leverage,
            "quantity": qty_s,
            "limitPrice": px_s,
            "chainId": chain_id,
            "signature": signature,
        });
        if let Some(t) = tif {
            body["timeInForce"] = serde_json::Value::String(t.to_string());
        }
        if post_only {
            body["postOnly"] = serde_json::Value::String("yes".to_string());
        }
        let resp = self
            .evedex_request(reqwest::Method::POST, "/api/v2/order/limit", Some(&body))
            .await?;
        self.evedex_parse_order(
            &resp,
            &order.symbol,
            &id,
            &order.client_order_id,
            order.quantity,
        )
    }

    /// `DELETE /api/order/{orderId}` — Bearer-authed, no signature (per
    /// docs). Terminal state is confirmed via `evedex_fetch_order`.
    async fn evedex_cancel(&self, req: &CancelRequest) -> anyhow::Result<()> {
        let oid = req
            .order_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("EVEDEX cancel needs the venue order id"))?;
        self.evedex_request(reqwest::Method::DELETE, &format!("/api/order/{oid}"), None)
            .await?;
        Ok(())
    }

    /// Venue truth: `GET /api/order/{orderId}` returns the durable order
    /// record with its current status (INTENTION→NEW→terminal states).
    async fn evedex_fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        let resp = self
            .evedex_request(
                reqwest::Method::GET,
                &format!("/api/order/{order_id}"),
                None,
            )
            .await?;
        self.evedex_parse_order(
            &resp,
            symbol,
            order_id,
            client_order_id.unwrap_or_default(),
            dec!(0),
        )
    }

    /// `GET /api/user/balance` + `GET /api/position` — equity is the sum
    /// of USD balances plus unrealized PnL; maintenance margin is the
    /// per-position `maintenanceMargin` sum.
    async fn evedex_account(&self) -> anyhow::Result<AccountSnapshot> {
        let balances_resp = self
            .evedex_request(reqwest::Method::GET, "/api/user/balance", None)
            .await?;
        let positions_resp = self
            .evedex_request(reqwest::Method::GET, "/api/position", None)
            .await?;
        let mut balances = Vec::new();
        let mut equity = dec!(0);
        if let Some(arr) = balances_resp.as_array() {
            for b in arr {
                let total = dec_from_json(b.get("balance")).unwrap_or_default();
                if total == dec!(0) {
                    continue;
                }
                let currency = b
                    .get("currency")
                    .and_then(|v| v.as_str())
                    .unwrap_or("USD")
                    .to_string();
                // Equity is USD — never add a non-USD balance at face value
                // when the venue omitted its USD conversion (fail closed:
                // undercount rather than fabricate a price).
                match dec_from_json(b.get("balanceUSD")) {
                    Some(usd) => equity += usd,
                    None if arbitrageur_core::symbol::is_usd_family(&currency) => equity += total,
                    None => {}
                }
                balances.push(arbitrageur_core::position::Balance {
                    venue: self.venue_id.clone(),
                    currency,
                    total,
                    free: total,
                    locked: dec!(0),
                });
            }
        }
        let mut positions = Vec::new();
        let mut maint = dec!(0);
        let mut upnl_sum = dec!(0);
        if let Some(arr) = positions_resp.get("list").and_then(|v| v.as_array()) {
            for p in arr {
                let qty = dec_from_json(p.get("quantity")).unwrap_or_default();
                if qty == dec!(0) {
                    continue;
                }
                let inst = p
                    .get("instrument")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                // Instrument "BTCUSD" → canonical "BTC-USD".
                let sym = inst
                    .strip_suffix("USD")
                    .map(|b| format!("{b}-USD"))
                    .unwrap_or_else(|| inst.to_string());
                let side = match p.get("side").and_then(|v| v.as_str()) {
                    Some("SELL") => arbitrageur_core::types::Side::Sell,
                    _ => arbitrageur_core::types::Side::Buy,
                };
                let upnl = dec_from_json(p.get("unRealizedPnL")).unwrap_or_default();
                maint += dec_from_json(p.get("maintenanceMargin")).unwrap_or_default();
                upnl_sum += upnl;
                positions.push(position_from(
                    &self.venue_id,
                    sym,
                    side,
                    qty,
                    dec_from_json(p.get("avgPrice")),
                    None,
                    Some(upnl),
                    dec_from_json(p.get("bankruptcyPrice")),
                    dec_from_json(p.get("leverage")),
                ));
            }
        }
        equity += upnl_sum;
        let free = (equity - maint).max(dec!(0));
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
            balances,
            positions,
            timestamp: Utc::now(),
        })
    }
}

// ---------------------------------------------------------------------------
// Paradex — Starknet L2 venue via the `paradex` client crate
// (paradex-rs). Orders sign on the Stark curve with a venue-derived L2 key:
// the stored EVM key signs the "STARK Key" EIP-712 message, the signature is
// ground into a Stark scalar, and the SDK owns SNIP-12 order hashing plus
// JWT auth (`POST /v1/auth`). Truth stays with `GET /v1/orders/{id}` polled
// under the client's bearer token — submit responses are never fills.
// ---------------------------------------------------------------------------

impl DexPerpAdapter {
    /// Lazily build the Paradex SDK client. `Client::new` fetches
    /// `/v1/system/config` (chain id + account-class hashes needed to
    /// compute the L2 account address), so construction must run in async
    /// context — credentials only store the derived L2 key.
    async fn paradex_client(&self) -> anyhow::Result<Arc<paradex::rest::Client>> {
        if let Some(c) = self.paradex_client.read().clone() {
            return Ok(c);
        }
        let _build = self.paradex_build.lock().await;
        if let Some(c) = self.paradex_client.read().clone() {
            return Ok(c);
        }
        let l2 = self
            .paradex_l2_key
            .read()
            .as_ref()
            .map(|k| k.as_str().to_string())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Paradex requires the account EVM private key — the Stark L2 signing key is derived from it"
                )
            })?;
        let client = paradex::rest::Client::new(paradex::url::URL::Production, Some(l2))
            .await
            .map_err(|e| {
                anyhow::anyhow!("Paradex client init (system config fetch) failed: {e}")
            })?;
        // Prove the account authenticates now rather than at order time —
        // a wallet that never completed Paradex onboarding fails here.
        client
            .jwt()
            .await
            .map_err(|e| anyhow::anyhow!("Paradex auth failed (is this wallet onboarded?): {e}"))?;
        let client = Arc::new(client);
        *self.paradex_client.write() = Some(client.clone());
        Ok(client)
    }

    /// Map a venue `OrderUpdate` to our lifecycle status. Paradex terminal
    /// state is `CLOSED` + `cancel_reason`: empty/`USER_CANCELED` is a
    /// user cancel, `EXPIRED` expiry, anything else a venue rejection.
    fn paradex_status(o: &paradex::structs::OrderUpdate) -> arbitrageur_core::types::OrderStatus {
        use arbitrageur_core::types::OrderStatus as S;
        match o.status {
            paradex::structs::OrderStatus::NEW | paradex::structs::OrderStatus::UNTRIGGERED => {
                S::New
            }
            paradex::structs::OrderStatus::OPEN => {
                if o.remaining_size < o.size {
                    S::PartiallyFilled
                } else {
                    S::New
                }
            }
            paradex::structs::OrderStatus::CLOSED => {
                if o.remaining_size <= Decimal::ZERO {
                    S::Filled
                } else {
                    match o.cancel_reason.as_str() {
                        "" | "USER_CANCELED" => S::Cancelled,
                        "EXPIRED" => S::Expired,
                        _ => S::Rejected,
                    }
                }
            }
        }
    }

    fn paradex_order_response(
        &self,
        o: paradex::structs::OrderUpdate,
        symbol: &str,
        fallback_client_id: &str,
    ) -> anyhow::Result<OrderResponse> {
        use rust_decimal::prelude::FromPrimitive;
        let status = Self::paradex_status(&o);
        let qty = o.size;
        let filled = (o.size - o.remaining_size).max(Decimal::ZERO);
        let avg = Decimal::from_f64(o.avg_fill_price).filter(|p| *p > Decimal::ZERO);
        let client_id = if o.client_id.is_empty() {
            fallback_client_id.to_string()
        } else {
            o.client_id.clone()
        };
        let raw = serde_json::to_value(&o).unwrap_or(serde_json::Value::Null);
        Ok(self.order_response(o.id, client_id, symbol, status, qty, filled, avg, raw))
    }

    /// `POST /v1/orders` via the SDK — the crate computes the SNIP-12
    /// order hash and signs with the derived Stark key.
    async fn paradex_place(&self, order: &OrderRequest) -> anyhow::Result<OrderResponse> {
        use arbitrageur_core::types::{OrderType, TimeInForce};
        use paradex::structs as ps;
        if order.quantity <= dec!(0) {
            anyhow::bail!("Paradex order size must be positive");
        }
        if order.time_in_force == TimeInForce::FillOrKill {
            anyhow::bail!("Paradex has no FOK instruction — refusing to silently map it");
        }
        let client = self.paradex_client().await?;
        let instruction = if order.post_only
            || order.order_type == OrderType::PostOnly
            || order.time_in_force == TimeInForce::PostOnly
        {
            ps::OrderInstruction::POST_ONLY
        } else if order.order_type == OrderType::Market
            || order.order_type == OrderType::ImmediateOrCancel
            || order.time_in_force == TimeInForce::ImmediateOrCancel
        {
            ps::OrderInstruction::IOC
        } else {
            ps::OrderInstruction::GTC
        };
        let (order_type, price) = match order.order_type {
            OrderType::Market => (ps::OrderType::MARKET, None),
            _ => {
                let p = match order.price {
                    Some(p) if p > dec!(0) => p,
                    _ if order.order_type == OrderType::ImmediateOrCancel => {
                        self.market_bound_price(order).await?
                    }
                    _ => anyhow::bail!("Paradex limit order requires a positive price"),
                };
                (ps::OrderType::LIMIT, Some(p))
            }
        };
        let mut flags = Vec::new();
        if order.reduce_only {
            flags.push(ps::OrderFlags::REDUCE_ONLY);
        }
        let req = ps::OrderRequest {
            instruction,
            market: self.venue_symbol(&order.symbol),
            price,
            side: if order.side.is_buy() {
                ps::Side::BUY
            } else {
                ps::Side::SELL
            },
            size: order.quantity,
            order_type,
            client_id: Some(order.client_order_id.clone()),
            flags,
            recv_window: Some(60_000),
            stp: None,
            trigger_price: None,
        };
        let update = client
            .create_order(req)
            .await
            .map_err(|e| anyhow::anyhow!("Paradex order rejected: {e}"))?;
        self.paradex_order_response(update, &order.symbol, &order.client_order_id)
    }

    /// `DELETE /v1/orders/{id}` — venue truth confirmed via `paradex_fetch_order`.
    async fn paradex_cancel(&self, req: &CancelRequest) -> anyhow::Result<()> {
        let client = self.paradex_client().await?;
        if let Some(oid) = req.order_id.as_deref() {
            client
                .cancel_order(oid.to_string())
                .await
                .map_err(|e| anyhow::anyhow!("Paradex cancel failed: {e}"))?;
            return Ok(());
        }
        if let Some(cid) = req.client_order_id.as_deref() {
            client
                .cancel_order_by_client_id_with_market(
                    cid.to_string(),
                    Some(self.venue_symbol(&req.symbol)),
                )
                .await
                .map_err(|e| anyhow::anyhow!("Paradex cancel-by-client-id failed: {e}"))?;
            return Ok(());
        }
        anyhow::bail!("Paradex cancel needs an order id or client order id");
    }

    /// Venue truth: `GET /v1/orders/{id}` (Bearer JWT from the SDK). If the
    /// order left the open book, `GET /v1/orders-history` still carries it.
    async fn paradex_fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        let client = self.paradex_client().await?;
        let jwt = client
            .jwt()
            .await
            .map_err(|e| anyhow::anyhow!("Paradex JWT unavailable: {e}"))?;
        for path in [
            format!("/v1/orders/{order_id}"),
            format!("/v1/orders-history/{order_id}"),
        ] {
            let url = format!("{}{}", self.base_url, path);
            let resp = self
                .client
                .get(&url)
                .bearer_auth(&jwt)
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("Paradex {path} request failed: {e}"))?;
            if resp.status() == reqwest::StatusCode::NOT_FOUND {
                continue;
            }
            if !resp.status().is_success() {
                anyhow::bail!("Paradex {path} HTTP {}", resp.status());
            }
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| anyhow::anyhow!("Paradex {path} bad JSON: {e}"))?;
            // Both endpoints return the order object (history wraps it the
            // same way) — deserialize through the crate's type.
            let update: paradex::structs::OrderUpdate = serde_json::from_value(body.clone())
                .map_err(|e| anyhow::anyhow!("Paradex {path} unexpected shape: {e}"))?;
            return self.paradex_order_response(
                update,
                symbol,
                client_order_id.unwrap_or_default(),
            );
        }
        anyhow::bail!("Paradex order {order_id} not found on orders or orders-history");
    }

    /// `GET /v1/account` + `GET /v1/positions` — equity and margins come
    /// straight from the account object; positions carry venue-side
    /// liquidation price and entry cost.
    async fn paradex_account(&self) -> anyhow::Result<AccountSnapshot> {
        use rust_decimal::prelude::FromPrimitive;
        let client = self.paradex_client().await?;
        let info = client
            .account_information()
            .await
            .map_err(|e| anyhow::anyhow!("Paradex account read failed: {e}"))?;
        let positions_resp = client
            .positions()
            .await
            .map_err(|e| anyhow::anyhow!("Paradex positions read failed: {e}"))?;
        let balances_resp = client
            .balance()
            .await
            .map_err(|e| anyhow::anyhow!("Paradex balance read failed: {e}"))?;
        let f = |v: f64| Decimal::from_f64(v).unwrap_or_default();
        let equity = f(info.account_value);
        let free = f(info.free_collateral);
        let maint = f(info.maintenance_margin_requirement);
        let mut balances = Vec::new();
        for b in &balances_resp.results {
            if b.size == 0.0 {
                continue;
            }
            balances.push(arbitrageur_core::position::Balance {
                venue: self.venue_id.clone(),
                currency: b.token.clone(),
                total: f(b.size),
                free: f(b.size),
                locked: dec!(0),
            });
        }
        let mut positions = Vec::new();
        for p in &positions_resp.results {
            if p.status != paradex::structs::PositionStatus::OPEN || p.size == 0.0 {
                continue;
            }
            // Market "BTC-USD-PERP" → canonical "BTC-USD".
            let sym = p
                .market
                .strip_suffix("-PERP")
                .unwrap_or(p.market.as_str())
                .to_string();
            positions.push(arbitrageur_core::position::Position {
                venue: self.venue_id.clone(),
                symbol: sym,
                side: match p.side {
                    paradex::structs::PositionSide::LONG => arbitrageur_core::types::Side::Buy,
                    paradex::structs::PositionSide::SHORT => arbitrageur_core::types::Side::Sell,
                },
                quantity: f(p.size.abs()),
                entry_price: f(p.average_entry_price),
                mark_price: f(p.average_entry_price_usd),
                unrealized_pnl: dec!(0),
                realized_pnl: f(p.realized_positional_pnl),
                liquidation_price: Some(f(p.liquidation_price)).filter(|v| *v > dec!(0)),
                leverage: Decimal::from_str(&p.leverage).unwrap_or_else(|_| dec!(1)),
                initial_margin: dec!(0),
                maintenance_margin: dec!(0),
                updated_at: Utc::now(),
            });
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
            balances,
            positions,
            timestamp: Utc::now(),
        })
    }
}

// ---------------------------------------------------------------------------
// edgeX — Perp V2: HMAC api-key auth + EIP-712 `LimitOrderParams` orders
// ---------------------------------------------------------------------------

/// Per-contract order-routing metadata from `getMetaData`.
struct EdgexContractMeta {
    /// Numeric contract id (`"30000001"`) — doubles as `assetIdSynthetic`.
    contract_id: String,
    /// Synthetic resolution (`resolution`/`starkExResolution`, dec or 0x).
    resolution: Decimal,
    /// Quote coin id (`quoteCoinId`) — `assetIdCollateral` in the sig.
    quote_coin_id: String,
    /// Quote-coin resolution (coinList lookup, e.g. USDC → 1e6).
    quote_resolution: Decimal,
    /// `defaultTakerFeeRate` (e.g. 0.00045) — bounds the signed fee cap.
    taker_fee_rate: Decimal,
    /// `tickSize` — bounds the market-order L2 price.
    tick_size: Decimal,
}

/// Parsed `getMetaData` — the EIP-712 domain is metadata-derived (never
/// hardcoded) so a host serving a different deployment fails closed at
/// signing time rather than signing against the wrong contract.
struct EdgexMeta {
    chain_id: u64,
    verifying_contract: Address,
    /// venue symbol ("BTCUSDC") → contract metadata.
    contracts: HashMap<String, EdgexContractMeta>,
    /// contractId → venue symbol (position parsing).
    contracts_by_id: HashMap<String, String>,
    /// coinId → display name ("1000" → "USDC").
    coin_names: HashMap<String, String>,
}

/// edgeX resolution fields arrive as decimal strings or 0x-hex integers
/// (coinList uses `"0xf4240"`, contractList `"10000000000"`).
fn edgex_resolution(v: Option<&serde_json::Value>) -> Option<Decimal> {
    let s = v?.as_str()?.trim().to_lowercase();
    if s.is_empty() {
        return None;
    }
    if let Some(h) = s.strip_prefix("0x") {
        let n = u128::from_str_radix(h, 16).ok()?;
        return Some(Decimal::from_i128_with_scale(n as i128, 0));
    }
    Decimal::from_str(&s).ok()
}

/// `nativeChainId`/`chainId` may be decimal ("3343") or hex ("0xd0f").
fn edgex_chain_id(v: Option<&serde_json::Value>) -> Option<u64> {
    let s = v?.as_str()?.trim().to_lowercase();
    if let Some(h) = s.strip_prefix("0x") {
        u64::from_str_radix(h, 16).ok()
    } else {
        s.parse().ok()
    }
}

fn edgex_now_ms() -> u64 {
    Utc::now().timestamp_millis().max(0) as u64
}

/// L2 nonce = first 8 hex chars of sha256(clientOrderId) as an integer
/// (SDK `CalcNonce` — i.e. the digest's top 32 bits).
fn edgex_nonce(client_order_id: &str) -> u64 {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(client_order_id.as_bytes());
    u64::from(u32::from_be_bytes(digest[..4].try_into().unwrap()))
}

/// Canonical `k1=v1&k2=v2` form of a JSON value for the HMAC body — the
/// SDK's `get_value`: object keys sorted, scalars verbatim (bool →
/// "true"/"false"), arrays `&`-joined, null → "".
fn edgex_canonical(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(a) => a.iter().map(edgex_canonical).collect::<Vec<_>>().join("&"),
        serde_json::Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            keys.iter()
                .map(|k| format!("{k}={}", edgex_canonical(&m[*k])))
                .collect::<Vec<_>>()
                .join("&")
        }
    }
}

/// `hex(HMAC-SHA256(key = base64(secret), msg = ts + METHOD + path + canonical))`
/// — the unusual base64-wrapped key is the SDK-documented behavior.
fn edgex_hmac_sign(
    secret: &str,
    timestamp: &str,
    method: &str,
    path: &str,
    canonical: &str,
) -> String {
    use hmac::{digest::KeyInit, Mac};
    let key = base64_encode(secret.as_bytes());
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(key.as_bytes())
        .expect("hmac accepts any key length");
    mac.update(format!("{timestamp}{method}{path}{canonical}").as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// The `LimitOrderParams` EIP-712 digest (EdgeX/1/{chainId}/{verifying}).
/// `OrderBase` is nested by struct-hash per EIP-712; `parentOrderHash` is
/// always zero and `subOrderIndex` always 0 for standalone orders.
#[allow(clippy::too_many_arguments)]
fn edgex_order_digest(
    meta: &EdgexMeta,
    nonce: u64,
    signer: Address,
    account_id: u64,
    expiration_s: u64,
    amount_synthetic: I256,
    amount_collateral: I256,
    amount_fee: U256,
    asset_synthetic: u64,
    asset_collateral: u64,
    is_buying: bool,
) -> [u8; 32] {
    let base_type = keccak256(
        b"OrderBase(uint256 nonce,address signer,uint64 accountId,uint256 expirationTimestamp)",
    );
    let base_hash = keccak256(
        (
            alloy::primitives::B256::from(base_type),
            U256::from(nonce),
            signer,
            account_id,
            U256::from(expiration_s),
        )
            .abi_encode(),
    );
    let params_type = keccak256(
        b"LimitOrderParams(OrderBase base,int256 amountSynthetic,int256 amountCollateral,uint256 amountFee,uint64 assetIdSynthetic,uint64 assetIdCollateral,bool isBuyingSynthetic,bytes32 parentOrderHash,uint256 subOrderIndex)OrderBase(uint256 nonce,address signer,uint64 accountId,uint256 expirationTimestamp)",
    );
    let struct_hash = keccak256(
        (
            alloy::primitives::B256::from(params_type),
            alloy::primitives::B256::from(base_hash),
            amount_synthetic,
            amount_collateral,
            amount_fee,
            asset_synthetic,
            asset_collateral,
            is_buying,
            alloy::primitives::B256::ZERO,
            U256::ZERO,
        )
            .abi_encode(),
    );
    let domain = eip712_domain_sep_vc("EdgeX", "1", meta.chain_id, meta.verifying_contract);
    eip712_digest(&domain, &struct_hash)
}

impl DexPerpAdapter {
    /// Fetch + parse `getMetaData`, or serve the cached copy. Contract
    /// ids are static for the process lifetime; a lookup miss refetches
    /// once in case a listing was added.
    async fn edgex_meta(&self) -> anyhow::Result<Arc<EdgexMeta>> {
        if let Some(m) = self.edgex_meta.read().clone() {
            return Ok(m);
        }
        self.edgex_refresh_meta().await
    }

    async fn edgex_refresh_meta(&self) -> anyhow::Result<Arc<EdgexMeta>> {
        let data = self.get_json("/api/v2/public/meta/getMetaData").await?;
        let d = data
            .get("data")
            .ok_or_else(|| anyhow::anyhow!("edgeX getMetaData missing data"))?;
        let global = d
            .get("global")
            .ok_or_else(|| anyhow::anyhow!("edgeX metadata missing global"))?;
        let chain_id = edgex_chain_id(
            global
                .get("nativeChainId")
                .or_else(|| global.get("chainId")),
        )
        .ok_or_else(|| anyhow::anyhow!("edgeX metadata missing global.nativeChainId"))?;
        let verifying_contract = global
            .get("contractAddress")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "edgeX metadata has no global.contractAddress — refusing to sign orders against an unverified EIP-712 domain"
                )
            })?
            .parse::<Address>()
            .map_err(|e| anyhow::anyhow!("edgeX global.contractAddress invalid: {e}"))?;
        let mut coin_res: HashMap<String, Decimal> = HashMap::new();
        let mut coin_names: HashMap<String, String> = HashMap::new();
        if let Some(arr) = d.get("coinList").and_then(|v| v.as_array()) {
            for c in arr {
                if let Some(id) = c.get("coinId").and_then(|v| v.as_str()) {
                    if let Some(r) =
                        edgex_resolution(c.get("resolution").or_else(|| c.get("starkExResolution")))
                    {
                        coin_res.insert(id.to_string(), r);
                    }
                    if let Some(n) = c.get("coinName").and_then(|v| v.as_str()) {
                        coin_names.insert(id.to_string(), n.to_string());
                    }
                }
            }
        }
        let mut contracts = HashMap::new();
        let mut contracts_by_id = HashMap::new();
        if let Some(arr) = d.get("contractList").and_then(|v| v.as_array()) {
            for c in arr {
                let (Some(name), Some(cid)) = (
                    c.get("contractName").and_then(|v| v.as_str()),
                    c.get("contractId").and_then(|v| v.as_str()),
                ) else {
                    continue;
                };
                let Some(resolution) =
                    edgex_resolution(c.get("starkExResolution").or_else(|| c.get("resolution")))
                else {
                    continue;
                };
                let quote_coin_id = c
                    .get("quoteCoinId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let Some(quote_resolution) = coin_res.get(&quote_coin_id).copied() else {
                    continue;
                };
                let Some(taker_fee_rate) = dec_from_json(c.get("defaultTakerFeeRate")) else {
                    continue;
                };
                let Some(tick_size) = dec_from_json(c.get("tickSize")) else {
                    continue;
                };
                contracts.insert(
                    name.to_string(),
                    EdgexContractMeta {
                        contract_id: cid.to_string(),
                        resolution,
                        quote_coin_id,
                        quote_resolution,
                        taker_fee_rate,
                        tick_size,
                    },
                );
                contracts_by_id.insert(cid.to_string(), name.to_string());
            }
        }
        if contracts.is_empty() {
            anyhow::bail!("edgeX metadata parsed zero contracts");
        }
        let meta = Arc::new(EdgexMeta {
            chain_id,
            verifying_contract,
            contracts,
            contracts_by_id,
            coin_names,
        });
        *self.edgex_meta.write() = Some(meta.clone());
        Ok(meta)
    }

    /// Contract metadata for a canonical symbol, refetching once on a
    /// miss (new listings appear without process restart).
    async fn edgex_contract(&self, symbol: &str) -> anyhow::Result<(Arc<EdgexMeta>, String)> {
        let vsym = self.venue_symbol(symbol);
        let meta = self.edgex_meta().await?;
        if meta.contracts.contains_key(&vsym) {
            return Ok((meta, vsym));
        }
        let meta = self.edgex_refresh_meta().await?;
        if !meta.contracts.contains_key(&vsym) {
            anyhow::bail!("edgeX has no contract for {vsym}");
        }
        Ok((meta, vsym))
    }

    /// `(api_key, passphrase, account_id)` — fails closed on any miss.
    fn edgex_auth(&self) -> anyhow::Result<(String, String, String)> {
        let c = self.creds.read();
        let key = c
            .api_key
            .clone()
            .ok_or_else(|| anyhow::anyhow!("edgeX api key missing"))?;
        let pass = c
            .passphrase
            .as_ref()
            .map(|p| p.as_str().to_string())
            .ok_or_else(|| anyhow::anyhow!("edgeX api passphrase missing"))?;
        let account = c
            .account_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("edgeX account id missing"))?;
        Ok((key, pass, account))
    }

    /// Signed `X-edgeX-*` headers for `method path` over the canonical
    /// body/query string.
    fn edgex_headers(
        &self,
        method: &str,
        path: &str,
        canonical: &str,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let (key, pass, _) = self.edgex_auth()?;
        let secret = self
            .creds
            .read()
            .api_secret
            .as_ref()
            .map(|s| s.as_str().to_string())
            .ok_or_else(|| anyhow::anyhow!("edgeX api secret missing"))?;
        let ts = edgex_now_ms().to_string();
        let sig = edgex_hmac_sign(&secret, &ts, method, path, canonical);
        Ok(vec![
            ("X-edgeX-Api-Key".into(), key),
            ("X-edgeX-Passphrase".into(), pass),
            ("X-edgeX-Signature".into(), sig),
            ("X-edgeX-Timestamp".into(), ts),
        ])
    }

    /// Signed GET to a private path — `params` are both the query string
    /// and the HMAC canonical body (sorted `k=v` pairs).
    async fn edgex_get_private(
        &self,
        path: &str,
        params: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let canonical = edgex_canonical(params);
        let headers = self.edgex_headers("GET", path, &canonical)?;
        let url = format!("{}{}?{}", self.base_url, path, canonical);
        self.get_json_url(&url, &headers).await
    }

    /// Signed POST — wire body is JSON, the HMAC canonical is the sorted
    /// `k=v` form of exactly the same fields.
    async fn edgex_post_private(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let canonical = edgex_canonical(body);
        let headers = self.edgex_headers("POST", path, &canonical)?;
        let url = format!("{}{}", self.base_url, path);
        let resp = self.post_json_url(&url, body, &headers).await?;
        let code = resp.get("code").and_then(|v| v.as_str()).unwrap_or("");
        if code != "SUCCESS" {
            let msg = resp.get("msg").and_then(|v| v.as_str()).unwrap_or("");
            anyhow::bail!("edgeX {path} rejected: code={code} {msg}");
        }
        Ok(resp)
    }

    /// `POST /api/v2/private/order/createOrder` — HMAC request auth plus
    /// the EIP-712 `LimitOrderParams` signature over L2-scaled amounts.
    async fn edgex_place(&self, order: &OrderRequest) -> anyhow::Result<OrderResponse> {
        use arbitrageur_core::types::{OrderType, Side, TimeInForce};
        let (_, _, account_id) = self.edgex_auth()?;
        let account_id_u64: u64 = account_id
            .parse()
            .map_err(|_| anyhow::anyhow!("edgeX account id is not numeric: {account_id}"))?;
        let (meta, vsym) = self.edgex_contract(&order.symbol).await?;
        let contract = meta
            .contracts
            .get(&vsym)
            .ok_or_else(|| anyhow::anyhow!("edgeX has no contract for {vsym}"))?;
        let qty = order.quantity;
        if qty <= dec!(0) {
            anyhow::bail!("edgeX order size must be positive");
        }
        let is_buy = order.side == Side::Buy;
        // Venue type/TIF mapping — MARKET has no price on the wire but
        // still signs a worst-case L2 value (10× oracle cap for buys,
        // one tick for sells — the SDK recipe).
        let (vtype, tif, l2_price) = match order.order_type {
            OrderType::Market => {
                let ticker = if is_buy {
                    let data = self
                        .get_json(&format!(
                            "/api/v2/public/quote/getTicker?contractId={}",
                            contract.contract_id
                        ))
                        .await?;
                    let entry = data
                        .get("data")
                        .and_then(|v| v.as_array())
                        .and_then(|a| a.first())
                        .ok_or_else(|| anyhow::anyhow!("edgeX ticker missing for {vsym}"))?;
                    Some(
                        dec_from_json(entry.get("oraclePrice"))
                            .ok_or_else(|| anyhow::anyhow!("edgeX oracle price missing"))?,
                    )
                } else {
                    None
                };
                let cap = match (is_buy, ticker) {
                    (true, Some(oracle)) => {
                        (oracle * dec!(10) / contract.tick_size).ceil() * contract.tick_size
                    }
                    (true, None) => anyhow::bail!("edgeX market buy needs an oracle price"),
                    (false, _) => contract.tick_size,
                };
                ("MARKET", "IMMEDIATE_OR_CANCEL", cap)
            }
            _ => {
                let price = order
                    .price
                    .ok_or_else(|| anyhow::anyhow!("edgeX limit order needs a price"))?;
                if price <= dec!(0) {
                    anyhow::bail!("edgeX limit price must be positive");
                }
                let tif = if order.post_only
                    || order.order_type == OrderType::PostOnly
                    || order.time_in_force == TimeInForce::PostOnly
                {
                    "POST_ONLY"
                } else if order.time_in_force == TimeInForce::FillOrKill {
                    "FILL_OR_KILL"
                } else if order.order_type == OrderType::ImmediateOrCancel
                    || order.time_in_force == TimeInForce::ImmediateOrCancel
                {
                    "IMMEDIATE_OR_CANCEL"
                } else {
                    "GOOD_TIL_CANCEL"
                };
                ("LIMIT", tif, price)
            }
        };
        let l2_value = l2_price * qty;
        let limit_fee = (l2_value * contract.taker_fee_rate).ceil();
        let amount_synthetic = (qty * contract.resolution).trunc();
        let amount_collateral = (l2_value * contract.quote_resolution).trunc();
        let amount_fee = limit_fee * contract.quote_resolution;
        let (Some(amt_syn), Some(amt_col), Some(amt_fee)) = (
            dec_to_i256(amount_synthetic, 0),
            dec_to_i256(amount_collateral, 0),
            dec_to_u256(amount_fee, 0),
        ) else {
            anyhow::bail!("edgeX order amounts overflow the signature fields");
        };
        let asset_synthetic: u64 = contract
            .contract_id
            .parse()
            .map_err(|_| anyhow::anyhow!("edgeX contractId is not numeric"))?;
        let asset_collateral: u64 = contract
            .quote_coin_id
            .parse()
            .map_err(|_| anyhow::anyhow!("edgeX quoteCoinId is not numeric"))?;
        let nonce = edgex_nonce(&order.client_order_id);
        let now_ms = edgex_now_ms();
        // SDK expiry windows: L1 expire = now+30d, L2 expire = +8d more;
        // the signature's expirationTimestamp is seconds.
        let expire_ms = now_ms + 30 * 24 * 3600 * 1000;
        let l2_expire_ms = expire_ms + 8 * 24 * 3600 * 1000;
        let expire_s = expire_ms / 1000;
        let signer = self
            .wallet
            .read()
            .as_ref()
            .map(|w| w.address_bytes())
            .ok_or_else(|| anyhow::anyhow!("edgeX trading key missing"))?;
        let digest = edgex_order_digest(
            &meta,
            nonce,
            Address::from(signer),
            account_id_u64,
            expire_s,
            amt_syn,
            amt_col,
            amt_fee,
            asset_synthetic,
            asset_collateral,
            is_buy,
        );
        let sig = {
            let w = self.wallet.read();
            let w = w
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("edgeX trading key missing"))?;
            w.sign_digest(&digest)?
        };
        let wire_price = if vtype == "MARKET" {
            "0".to_string()
        } else {
            l2_price.normalize().to_string()
        };
        let qty_s = qty.normalize().to_string();
        let body = serde_json::json!({
            "accountId": account_id,
            "contractId": contract.contract_id,
            "price": wire_price,
            "size": qty_s,
            "type": vtype,
            "side": if is_buy { "BUY" } else { "SELL" },
            "timeInForce": tif,
            "clientOrderId": order.client_order_id,
            "expireTime": expire_ms.to_string(),
            "l2Nonce": nonce.to_string(),
            "l2Signature": format!("0x{}", hex::encode(sig)),
            "l2ExpireTime": l2_expire_ms.to_string(),
            "l2Value": l2_value.normalize().to_string(),
            "l2Size": qty_s,
            "l2LimitFee": limit_fee.normalize().to_string(),
            "reduceOnly": order.reduce_only,
        });
        let resp = self
            .edgex_post_private("/api/v2/private/order/createOrder", &body)
            .await?;
        let order_id = resp
            .pointer("/data/orderId")
            .and_then(|v| {
                v.as_str()
                    .map(str::to_string)
                    .or_else(|| v.as_i64().map(|n| n.to_string()))
            })
            .ok_or_else(|| anyhow::anyhow!("edgeX createOrder returned no orderId: {resp}"))?;
        Ok(OrderResponse {
            order_id,
            client_order_id: order.client_order_id.clone(),
            venue: self.venue_id.clone(),
            symbol: order.symbol.clone(),
            status: arbitrageur_core::types::OrderStatus::New,
            original_qty: qty,
            filled_qty: dec!(0),
            remaining_qty: qty,
            avg_fill_price: None,
            fee: None,
            fee_currency: "USDC".to_string(),
            raw_response: Some(resp.to_string()),
            timestamp: Utc::now(),
        })
    }

    /// `POST /api/v2/private/order/cancelOrderById` (or the clientOrderId
    /// variant). The ack is not terminal truth — the reconciler re-polls
    /// `getOrderById` for the CANCELED state.
    async fn edgex_cancel(&self, req: &CancelRequest) -> anyhow::Result<()> {
        let (_, _, account_id) = self.edgex_auth()?;
        let (path, body) = if let Some(id) = req.order_id.as_deref().filter(|s| !s.is_empty()) {
            (
                "/api/v2/private/order/cancelOrderById",
                serde_json::json!({"accountId": account_id, "orderIdList": [id]}),
            )
        } else if let Some(cid) = req.client_order_id.as_deref().filter(|s| !s.is_empty()) {
            (
                "/api/v2/private/order/cancelOrderByClientOrderId",
                serde_json::json!({"accountId": account_id, "clientOrderIdList": [cid]}),
            )
        } else {
            anyhow::bail!("edgeX cancel needs an order id or client order id");
        };
        let resp = self.edgex_post_private(path, &body).await?;
        // `cancelResultMap` carries per-id results; an explicit failure
        // string fails closed (absence is tolerated — the venue may
        // return a bare data object on full success).
        if let Some(map) = resp
            .pointer("/data/cancelResultMap")
            .and_then(|v| v.as_object())
        {
            for (id, res) in map {
                let s = res.as_str().unwrap_or("").to_uppercase();
                if s.contains("FAIL") || s.contains("ERROR") || s.contains("NOT") {
                    anyhow::bail!("edgeX cancel rejected for {id}: {s}");
                }
            }
        }
        Ok(())
    }

    /// `GET /api/v2/private/order/getOrderById` (clientOrderId variant as
    /// fallback) — array response; an absent order is an error, never a
    /// guessed state.
    async fn edgex_fetch_order(
        &self,
        _symbol: &str,
        order_id: &str,
        client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        use arbitrageur_core::types::OrderStatus;
        let (_, _, account_id) = self.edgex_auth()?;
        let mut entry: Option<serde_json::Value> = None;
        if !order_id.is_empty() {
            let resp = self
                .edgex_get_private(
                    "/api/v2/private/order/getOrderById",
                    &serde_json::json!({"accountId": account_id, "orderIdList": [order_id]}),
                )
                .await?;
            entry = resp.get("data").and_then(|v| v.as_array()).and_then(|a| {
                // Exact id match only — falling back to an arbitrary array
                // element would report another order's state as this one's.
                a.iter()
                    .find(|e| {
                        e.get("id").and_then(|v| {
                            v.as_str()
                                .map(|s| s == order_id)
                                .or_else(|| v.as_i64().map(|n| n.to_string() == order_id))
                                .or_else(|| v.as_u64().map(|n| n.to_string() == order_id))
                        }) == Some(true)
                    })
                    .cloned()
            });
        }
        if entry.is_none() {
            if let Some(cid) = client_order_id.filter(|s| !s.is_empty()) {
                let resp = self
                    .edgex_get_private(
                        "/api/v2/private/order/getOrderByClientOrderId",
                        &serde_json::json!({"accountId": account_id, "clientOrderIdList": [cid]}),
                    )
                    .await?;
                entry = resp.get("data").and_then(|v| v.as_array()).and_then(|a| {
                    a.iter()
                        .find(|e| e.get("clientOrderId").and_then(|v| v.as_str()) == Some(cid))
                        .cloned()
                });
            }
        }
        let e = entry
            .ok_or_else(|| anyhow::anyhow!("edgeX order {order_id} not found on the venue"))?;
        let status_raw = e.get("status").and_then(|v| v.as_str()).unwrap_or("");
        // CANCELING is transient — still live until the venue confirms.
        let status = if status_raw.eq_ignore_ascii_case("canceling") {
            OrderStatus::New
        } else {
            map_order_status(status_raw, dec!(0), dec!(0)).ok_or_else(|| {
                anyhow::anyhow!("edgeX order {order_id} has unknown status {status_raw}")
            })?
        };
        let original = dec_from_json(e.get("size")).unwrap_or_default();
        let filled = dec_from_json(e.get("cumFillSize")).unwrap_or_default();
        let fill_value = dec_from_json(e.get("cumFillValue")).unwrap_or_default();
        let avg_fill = if filled > dec!(0) {
            Some(fill_value / filled)
        } else {
            dec_from_json(e.get("avgPrice"))
        };
        let status = match status {
            OrderStatus::New if filled > dec!(0) && filled < original => {
                OrderStatus::PartiallyFilled
            }
            s => s,
        };
        let venue_oid = e
            .get("id")
            .and_then(|v| {
                v.as_str()
                    .map(str::to_string)
                    .or_else(|| v.as_u64().map(|n| n.to_string()))
                    .or_else(|| v.as_i64().map(|n| n.to_string()))
            })
            .unwrap_or_else(|| order_id.to_string());
        Ok(OrderResponse {
            order_id: venue_oid,
            client_order_id: e
                .get("clientOrderId")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            venue: self.venue_id.clone(),
            symbol: _symbol.to_string(),
            status,
            original_qty: original,
            filled_qty: filled,
            remaining_qty: (original - filled).max(dec!(0)),
            avg_fill_price: avg_fill,
            fee: dec_from_json(e.get("cumFillFee")),
            fee_currency: "USDC".to_string(),
            raw_response: Some(e.to_string()),
            timestamp: Utc::now(),
        })
    }

    /// `GET /api/v2/private/account/getAccountAsset` — equity/available
    /// from `collateralAssetModelList`, coin balances from
    /// `collateralList`, signed `openSize` positions from `positionList`
    /// with oracle marks from `oraclePriceList`.
    async fn edgex_account(&self) -> anyhow::Result<AccountSnapshot> {
        let (_, _, account_id) = self.edgex_auth()?;
        let resp = self
            .edgex_get_private(
                "/api/v2/private/account/getAccountAsset",
                &serde_json::json!({"accountId": account_id}),
            )
            .await?;
        let code = resp.get("code").and_then(|v| v.as_str()).unwrap_or("");
        if code != "SUCCESS" {
            let msg = resp.get("msg").and_then(|v| v.as_str()).unwrap_or("");
            anyhow::bail!("edgeX getAccountAsset rejected: code={code} {msg}");
        }
        let d = resp
            .get("data")
            .ok_or_else(|| anyhow::anyhow!("edgeX getAccountAsset missing data"))?;
        let meta = self.edgex_meta().await.ok();
        // Account-level asset row for the quote coin (USDC "1000").
        let assets = d
            .get("collateralAssetModelList")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let asset_row = assets
            .iter()
            .find(|a| a.get("coinId").and_then(|v| v.as_str()) == Some("1000"))
            .or_else(|| assets.first());
        let equity = asset_row
            .and_then(|a| dec_from_json(a.get("totalEquity")))
            .unwrap_or_default();
        let free = asset_row
            .and_then(|a| dec_from_json(a.get("availableAmount")))
            .unwrap_or_default();
        let maint = asset_row
            .and_then(|a| dec_from_json(a.get("initialMarginRequirement")))
            .unwrap_or_default();
        let mut balances = Vec::new();
        if let Some(arr) = d.get("collateralList").and_then(|v| v.as_array()) {
            for b in arr {
                let amt = dec_from_json(b.get("amount")).unwrap_or_default();
                if amt == dec!(0) {
                    continue;
                }
                let cid = b.get("coinId").and_then(|v| v.as_str()).unwrap_or("");
                let currency = meta
                    .as_ref()
                    .and_then(|m| m.coin_names.get(cid).cloned())
                    .unwrap_or_else(|| cid.to_string());
                let avail = assets
                    .iter()
                    .find(|a| a.get("coinId").and_then(|v| v.as_str()) == Some(cid))
                    .and_then(|a| dec_from_json(a.get("availableAmount")))
                    .unwrap_or(amt);
                balances.push(arbitrageur_core::position::Balance {
                    venue: self.venue_id.clone(),
                    currency,
                    total: amt,
                    free: avail,
                    locked: (amt - avail).max(dec!(0)),
                });
            }
        }
        // Oracle marks for position marks/uPnL.
        let mut marks: HashMap<String, Decimal> = HashMap::new();
        if let Some(arr) = d.get("oraclePriceList").and_then(|v| v.as_array()) {
            for p in arr {
                let is_oracle = p
                    .get("priceType")
                    .and_then(|v| v.as_str())
                    .map(|s| s.eq_ignore_ascii_case("ORACLE_PRICE"))
                    .unwrap_or(true);
                if !is_oracle {
                    continue;
                }
                if let (Some(cid), Some(px)) = (
                    p.get("contractId").and_then(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .or_else(|| v.as_u64().map(|n| n.to_string()))
                    }),
                    dec_from_json(p.get("priceValue")),
                ) {
                    marks.insert(cid, px);
                }
            }
        }
        let mut positions = Vec::new();
        if let Some(arr) = d.get("positionList").and_then(|v| v.as_array()) {
            for p in arr {
                let open = dec_from_json(p.get("openSize")).unwrap_or_default();
                if open == dec!(0) {
                    continue;
                }
                let cid = p
                    .get("contractId")
                    .and_then(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .or_else(|| v.as_u64().map(|n| n.to_string()))
                    })
                    .unwrap_or_default();
                // Venue name "BTCUSDC" → canonical "BTC-USDC".
                let sym = meta
                    .as_ref()
                    .and_then(|m| m.contracts_by_id.get(&cid).cloned())
                    .map(|name| {
                        name.strip_suffix("USDC")
                            .map(|b| format!("{b}-USDC"))
                            .unwrap_or_else(|| {
                                name.strip_suffix("USD")
                                    .map(|b| format!("{b}-USD"))
                                    .unwrap_or(name.clone())
                            })
                    })
                    .unwrap_or_else(|| cid.clone());
                let qty = open.abs();
                let open_value = dec_from_json(p.get("openValue")).unwrap_or_default();
                let entry = if qty > dec!(0) {
                    open_value / qty
                } else {
                    dec!(0)
                };
                let mark = marks.get(&cid).copied().unwrap_or(entry);
                let sign = if open < dec!(0) { dec!(-1) } else { dec!(1) };
                positions.push(arbitrageur_core::position::Position {
                    venue: self.venue_id.clone(),
                    symbol: sym,
                    side: if open < dec!(0) {
                        arbitrageur_core::types::Side::Sell
                    } else {
                        arbitrageur_core::types::Side::Buy
                    },
                    quantity: qty,
                    entry_price: entry,
                    mark_price: mark,
                    unrealized_pnl: (mark - entry) * qty * sign,
                    realized_pnl: dec!(0),
                    liquidation_price: None,
                    leverage: dec!(1),
                    initial_margin: dec!(0),
                    maintenance_margin: dec!(0),
                    updated_at: Utc::now(),
                });
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
            balances,
            positions,
            timestamp: Utc::now(),
        })
    }

    /// `POST /api/v2/private/account/updateLeverageSetting` — plain HMAC
    /// (no L2 signature); `leverage` is the contract-level multiplier.
    async fn edgex_set_leverage(&self, symbol: &str, leverage: u32) -> anyhow::Result<()> {
        let (_, _, account_id) = self.edgex_auth()?;
        let (meta, vsym) = self.edgex_contract(symbol).await?;
        let contract = meta
            .contracts
            .get(&vsym)
            .ok_or_else(|| anyhow::anyhow!("edgeX has no contract for {vsym}"))?;
        self.edgex_post_private(
            "/api/v2/private/account/updateLeverageSetting",
            &serde_json::json!({
                "accountId": account_id,
                "contractId": contract.contract_id,
                "leverage": leverage.to_string(),
            }),
        )
        .await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Extended — X-Api-Key auth + SNIP-12 Poseidon `Order` signatures
// ---------------------------------------------------------------------------
//
// Extended (extended.exchange) settles on a StarkEx engine reached through
// `api.starknet.extended.exchange`. REST requests authenticate with the
// account's `X-Api-Key` header; every order carries a `settlement` object
// whose signature is a Stark-curve ECDSA over the SNIP-12 typed-data hash
// of an `Order` struct (Poseidon hash chain, "StarkNet Message" prefix,
// domain {name:"Perpetuals", version:"v0", chainId:"SN_MAIN", revision:1}).
// The construction below mirrors the official X10 `fast_stark_crypto`
// Rust implementation and is verified byte-for-byte against its shipped
// test vectors.
//
// Credentials: api_key (X-Api-Key), account_id = the account's L2 vault /
// collateral position id, wallet_address = Stark public key (0x felt),
// private_key = Stark private key (0x felt). The pubkey↔privkey pairing
// is validated at credential load — a mismatch can never authenticate.

/// Per-market L2 settlement config (`l2Config` from `/info/markets`).
struct ExtendedL2 {
    synthetic_id: starknet_crypto::Felt,
    collateral_id: starknet_crypto::Felt,
    synthetic_resolution: Decimal,
    collateral_resolution: Decimal,
}

/// ASCII short string as a felt (big-endian packed bytes, ≤31 chars).
fn stark_short_felt(s: &str) -> starknet_crypto::Felt {
    debug_assert!(s.len() <= 31);
    starknet_crypto::Felt::from_bytes_be_slice(s.as_bytes())
}

/// `selector!("\"StarknetDomain\"(...)")` — value asserted against the
/// X10 `fast_stark_crypto` sdist test vector.
fn extended_domain_selector() -> starknet_crypto::Felt {
    starknet_crypto::Felt::from_hex_unchecked(
        "0x1ff2f602e42168014d405a94f75e8a93d640751d71d16311266e140d8b0a210",
    )
}

/// `selector!("\"Order\"(...)")` — asserted in tests.
fn extended_order_selector() -> starknet_crypto::Felt {
    starknet_crypto::Felt::from_hex_unchecked(
        "0x36da8d51815527cabfaa9c982f564c80fa7429616739306036f1f9b608dd112",
    )
}

/// Poseidon struct hash of the `StarknetDomain`.
fn extended_domain_hash(chain_id: &str) -> starknet_crypto::Felt {
    let mut h = starknet_crypto::PoseidonHasher::new();
    h.update(extended_domain_selector());
    h.update(stark_short_felt("Perpetuals"));
    h.update(stark_short_felt("v0"));
    h.update(stark_short_felt(chain_id));
    h.update(starknet_crypto::Felt::ONE);
    h.finalize()
}

/// Poseidon struct hash of the signed `Order` (field order matches the
/// SNIP-12 type string: position_id, base/quote asset+amount, fee
/// asset+amount, expiration, salt).
#[allow(clippy::too_many_arguments)]
fn extended_order_hash(
    position_id: u32,
    base_asset_id: starknet_crypto::Felt,
    base_amount: i64,
    quote_asset_id: starknet_crypto::Felt,
    quote_amount: i64,
    fee_asset_id: starknet_crypto::Felt,
    fee_amount: u64,
    expiration_secs: u64,
    salt: starknet_crypto::Felt,
) -> starknet_crypto::Felt {
    let mut h = starknet_crypto::PoseidonHasher::new();
    h.update(extended_order_selector());
    h.update(position_id.into());
    h.update(base_asset_id);
    h.update(base_amount.into());
    h.update(quote_asset_id);
    h.update(quote_amount.into());
    h.update(fee_asset_id);
    h.update(fee_amount.into());
    h.update(expiration_secs.into());
    h.update(salt);
    h.finalize()
}

/// `Poseidon("StarkNet Message", domain, pubkey, order_hash)`.
fn extended_message_hash(
    order_hash: starknet_crypto::Felt,
    public_key: starknet_crypto::Felt,
    chain_id: &str,
) -> starknet_crypto::Felt {
    let mut h = starknet_crypto::PoseidonHasher::new();
    h.update(stark_short_felt("StarkNet Message"));
    h.update(extended_domain_hash(chain_id));
    h.update(public_key);
    h.update(order_hash);
    h.finalize()
}

/// RFC-6979 deterministic Stark-curve ECDSA sign → (r, s).
fn extended_sign(
    msg_hash: starknet_crypto::Felt,
    private_key: starknet_crypto::Felt,
) -> anyhow::Result<(starknet_crypto::Felt, starknet_crypto::Felt)> {
    let k = starknet_crypto::rfc6979_generate_k(&msg_hash, &private_key, None);
    let sig = starknet_crypto::sign(&private_key, &msg_hash, &k)
        .map_err(|e| anyhow::anyhow!("Extended order signing failed: {e:?}"))?;
    Ok((sig.r, sig.s))
}

/// `int(value * resolution)` — ROUND_UP (away from zero) for buy-side
/// amounts and the fee, ROUND_DOWN for sell-side, per the SDK rounding
/// contexts. Positive inputs only.
fn extended_stark_amount(
    value: Decimal,
    resolution: Decimal,
    round_up: bool,
) -> anyhow::Result<i64> {
    let scaled = value * resolution;
    let int = if round_up {
        scaled.ceil()
    } else {
        scaled.floor()
    };
    i64::try_from(int)
        .map_err(|_| anyhow::anyhow!("Extended amount {value} overflows settlement range"))
}

fn felt_hex(f: &starknet_crypto::Felt) -> String {
    format!("{:#x}", f)
}

impl DexPerpAdapter {
    /// `(api_key, vault_id, stark_pubkey_hex, stark_privkey_hex)` —
    /// fails closed on any missing piece.
    fn extended_auth(&self) -> anyhow::Result<(String, u32, String, String)> {
        let c = self.creds.read();
        let key = c
            .api_key
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Extended api key missing"))?;
        let vault: u32 = c
            .account_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Extended vault/account id missing"))?
            .parse()
            .map_err(|_| anyhow::anyhow!("Extended vault/account id is not a u32"))?;
        let pubkey = c
            .wallet_address
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Extended Stark public key missing"))?;
        drop(c);
        let privk = self
            .stark_key
            .read()
            .as_ref()
            .map(|s| s.as_str().to_string())
            .ok_or_else(|| anyhow::anyhow!("Extended Stark private key missing"))?;
        Ok((key, vault, pubkey, privk))
    }

    fn extended_headers(&self) -> anyhow::Result<Vec<(String, String)>> {
        let (key, _, _, _) = self.extended_auth()?;
        Ok(vec![("X-Api-Key".into(), key)])
    }

    /// `GET /api/v1/info/markets` `l2Config` per market name — the
    /// synthetic/collateral asset ids and StarkEx resolutions needed to
    /// pack settlement amounts. Cached; refetched on a symbol miss.
    async fn extended_market_l2(&self, symbol: &str) -> anyhow::Result<(String, ExtendedL2)> {
        let vsym = self.venue_symbol(symbol);
        let data = self.get_json("/api/v1/info/markets").await?;
        let arr = data
            .get("data")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow::anyhow!("Extended markets missing data"))?;
        let mut map: HashMap<String, ExtendedL2> = HashMap::new();
        for e in arr {
            let name = e.get("name").and_then(|s| s.as_str()).unwrap_or("");
            let l2 = e.get("l2Config");
            let (Some(l2), false) = (l2, name.is_empty()) else {
                continue;
            };
            let (Some(syn), Some(col), Some(sres), Some(cres)) = (
                l2.get("syntheticId").and_then(|v| v.as_str()),
                l2.get("collateralId").and_then(|v| v.as_str()),
                dec_from_json(l2.get("syntheticResolution")),
                dec_from_json(l2.get("collateralResolution")),
            ) else {
                continue;
            };
            let (Ok(syn), Ok(col)) = (
                starknet_crypto::Felt::from_hex(syn),
                starknet_crypto::Felt::from_hex(col),
            ) else {
                continue;
            };
            map.insert(
                name.to_string(),
                ExtendedL2 {
                    synthetic_id: syn,
                    collateral_id: col,
                    synthetic_resolution: sres,
                    collateral_resolution: cres,
                },
            );
        }
        *self.extended_meta.write() = Some(Arc::new(map));
        let guard = self.extended_meta.read();
        let meta = guard
            .as_ref()
            .and_then(|m| m.get(&vsym))
            .map(|l2| ExtendedL2 {
                synthetic_id: l2.synthetic_id,
                collateral_id: l2.collateral_id,
                synthetic_resolution: l2.synthetic_resolution,
                collateral_resolution: l2.collateral_resolution,
            })
            .ok_or_else(|| anyhow::anyhow!("Extended has no L2 config for {vsym}"))?;
        Ok((vsym, meta))
    }

    /// Envelope check shared by every Extended private response.
    fn extended_data<'a>(
        resp: &'a serde_json::Value,
        what: &str,
    ) -> anyhow::Result<&'a serde_json::Value> {
        let status = resp.get("status").and_then(|v| v.as_str()).unwrap_or("");
        if !status.eq_ignore_ascii_case("OK") {
            let err = resp.get("error").map(|e| e.to_string()).unwrap_or_default();
            anyhow::bail!("Extended {what} rejected: status={status} {err}");
        }
        resp.get("data")
            .ok_or_else(|| anyhow::anyhow!("Extended {what} response missing data"))
    }

    /// `POST /api/v1/user/order` — settlement signature over the
    /// SNIP-12 `Order` hash; `id` is the client order id.
    async fn extended_place(&self, order: &OrderRequest) -> anyhow::Result<OrderResponse> {
        use arbitrageur_core::types::{OrderType, Side, TimeInForce};
        let (_, vault, pubkey_hex, priv_hex) = self.extended_auth()?;
        let (vsym, l2) = self.extended_market_l2(&order.symbol).await?;
        let is_buy = order.side == Side::Buy;
        let qty = order.quantity;
        if qty <= dec!(0) {
            anyhow::bail!("Extended order quantity must be positive");
        }
        // `reduceOnly` passes through — the venue enforces it natively.
        // Signed price bound: limit orders sign their limit; market
        // orders sign a 5% slippage bound off the live book (the SDK
        // requires MARKET + IOC + a bound price — never sign zero).
        let post_only = order.post_only || order.order_type == OrderType::PostOnly;
        let (price, otype, tif) = match order.order_type {
            OrderType::Limit | OrderType::PostOnly | OrderType::ImmediateOrCancel => {
                let p = order
                    .price
                    .ok_or_else(|| anyhow::anyhow!("Extended limit order requires a price"))?;
                let tif = if post_only {
                    // GTT + postOnly is the resting-maker form; TOB is a
                    // separate aggressive-top mode we don't emit.
                    "GTT"
                } else {
                    match order.time_in_force {
                        TimeInForce::ImmediateOrCancel => "IOC",
                        TimeInForce::FillOrKill => "FOK",
                        _ if order.order_type == OrderType::ImmediateOrCancel => "IOC",
                        _ => "GTT",
                    }
                };
                (p, "LIMIT", tif)
            }
            OrderType::Market => {
                let book = self.fetch_orderbook(&order.symbol).await?;
                let bound = if is_buy {
                    book.asks
                        .first()
                        .map(|l| l.price * dec!(1.05))
                        .ok_or_else(|| {
                            anyhow::anyhow!("Extended book has no asks to bound a market buy")
                        })?
                } else {
                    book.bids
                        .first()
                        .map(|l| l.price * dec!(0.95))
                        .ok_or_else(|| {
                            anyhow::anyhow!("Extended book has no bids to bound a market sell")
                        })?
                };
                (bound, "MARKET", "IOC")
            }
        };
        // Settlement packing: buy rounds synthetic/collateral up, sell
        // rounds down; the fee always ceils. Buy negates the collateral
        // leg; sell negates the synthetic leg.
        let notional = qty * price;
        let syn = extended_stark_amount(qty, l2.synthetic_resolution, is_buy)?;
        let col = extended_stark_amount(notional, l2.collateral_resolution, is_buy)?;
        let fee_amt =
            extended_stark_amount(notional * dec!(0.0005), l2.collateral_resolution, true)?;
        let (base_i64, quote_i64) = if is_buy { (syn, -col) } else { (-syn, col) };
        // Random-looking 32-bit salt, deterministic per client id.
        use sha2::Digest;
        let nonce_bytes = sha2::Sha256::digest(order.client_order_id.as_bytes());
        let nonce = u32::from_be_bytes(nonce_bytes[..4].try_into().unwrap());
        let expiry = Utc::now() + chrono::Duration::hours(1);
        let settle_expiry_secs = (expiry + chrono::Duration::days(14)).timestamp().max(0) as u64;
        let pubkey = starknet_crypto::Felt::from_hex(&pubkey_hex)
            .map_err(|_| anyhow::anyhow!("Extended Stark public key is not valid hex"))?;
        let privk = starknet_crypto::Felt::from_hex(&priv_hex)
            .map_err(|_| anyhow::anyhow!("Extended Stark private key is not valid hex"))?;
        let order_hash = extended_order_hash(
            vault,
            l2.synthetic_id,
            base_i64,
            l2.collateral_id,
            quote_i64,
            l2.collateral_id, // fee is always charged in the collateral asset
            fee_amt as u64,
            settle_expiry_secs,
            nonce.into(),
        );
        let msg = extended_message_hash(order_hash, pubkey, "SN_MAIN");
        let (r, s) = extended_sign(msg, privk)?;
        let body = serde_json::json!({
            "id": order.client_order_id,
            "market": vsym,
            "type": otype,
            "side": if is_buy { "BUY" } else { "SELL" },
            "qty": qty.normalize().to_string(),
            "price": price.normalize().to_string(),
            "reduceOnly": order.reduce_only,
            "postOnly": post_only,
            "timeInForce": tif,
            "expiryEpochMillis": expiry.timestamp_millis(),
            "fee": "0.0005",
            "nonce": nonce.to_string(),
            "selfTradeProtectionLevel": "ACCOUNT",
            "settlement": {
                "signature": {"r": felt_hex(&r), "s": felt_hex(&s)},
                "starkKey": felt_hex(&pubkey),
                "collateralPosition": vault.to_string(),
            },
            "debuggingAmounts": {
                "collateralAmount": quote_i64.to_string(),
                "feeAmount": fee_amt.to_string(),
                "syntheticAmount": base_i64.to_string(),
            },
        });
        let url = format!("{}/api/v1/user/order", self.base_url);
        let resp = self
            .post_json_url(&url, &body, &self.extended_headers()?)
            .await?;
        let data = Self::extended_data(&resp, "create order")?;
        let venue_oid = data
            .get("id")
            .and_then(|v| {
                v.as_str()
                    .map(str::to_string)
                    .or_else(|| v.as_u64().map(|n| n.to_string()))
                    .or_else(|| v.as_i64().map(|n| n.to_string()))
            })
            .unwrap_or_default();
        Ok(OrderResponse {
            order_id: venue_oid,
            client_order_id: order.client_order_id.clone(),
            venue: self.venue_id.clone(),
            symbol: order.symbol.clone(),
            status: arbitrageur_core::types::OrderStatus::New,
            original_qty: qty,
            filled_qty: dec!(0),
            remaining_qty: qty,
            avg_fill_price: None,
            fee: None,
            fee_currency: "USD".to_string(),
            raw_response: Some(resp.to_string()),
            timestamp: Utc::now(),
        })
    }

    /// `DELETE /api/v1/user/order/{id}` or `?externalId=` — both id forms
    /// are accepted; neither is inferred locally.
    async fn extended_cancel(&self, req: &CancelRequest) -> anyhow::Result<()> {
        let headers = self.extended_headers()?;
        let url = if let Some(oid) = req.order_id.as_deref().filter(|s| !s.is_empty()) {
            format!("{}/api/v1/user/order/{}", self.base_url, oid)
        } else if let Some(cid) = req.client_order_id.as_deref().filter(|s| !s.is_empty()) {
            format!("{}/api/v1/user/order?externalId={cid}", self.base_url)
        } else {
            anyhow::bail!("Extended cancel needs an order id or client order id");
        };
        let resp = self.delete_json_url(&url, &headers).await?;
        Self::extended_data(&resp, "cancel order")?;
        Ok(())
    }

    /// `GET /api/v1/user/orders/{id}` / `/external/{externalId}` — venue
    /// truth for order state; no terminal inference from submissions.
    async fn extended_fetch_order(
        &self,
        _symbol: &str,
        order_id: &str,
        client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        let headers = self.extended_headers()?;
        let mut entry: Option<serde_json::Value> = None;
        if !order_id.is_empty() {
            let url = format!("{}/api/v1/user/orders/{}", self.base_url, order_id);
            let resp = self.get_json_url(&url, &headers).await?;
            entry = Self::extended_data(&resp, "get order").ok().cloned();
        }
        if entry.is_none() {
            if let Some(cid) = client_order_id.filter(|s| !s.is_empty()) {
                let url = format!("{}/api/v1/user/orders/external/{}", self.base_url, cid);
                let resp = self.get_json_url(&url, &headers).await?;
                entry = Self::extended_data(&resp, "get order by external id")
                    .ok()
                    .and_then(|v| v.as_array().cloned())
                    .and_then(|a| {
                        // Exact externalId match only — an arbitrary array
                        // element is another order's state, not this one's.
                        a.iter()
                            .find(|e| e.get("externalId").and_then(|v| v.as_str()) == Some(cid))
                            .cloned()
                    });
            }
        }
        let e = entry
            .ok_or_else(|| anyhow::anyhow!("Extended order {order_id} not found on the venue"))?;
        let status_raw = e.get("status").and_then(|v| v.as_str()).unwrap_or("");
        let original = dec_from_json(e.get("qty")).unwrap_or_default();
        let filled = dec_from_json(e.get("filledQty")).unwrap_or_default();
        let status = map_order_status(status_raw, filled, original).ok_or_else(|| {
            anyhow::anyhow!("Extended order {order_id} has unknown status {status_raw}")
        })?;
        let venue_oid = e
            .get("id")
            .and_then(|v| {
                v.as_str()
                    .map(str::to_string)
                    .or_else(|| v.as_u64().map(|n| n.to_string()))
                    .or_else(|| v.as_i64().map(|n| n.to_string()))
            })
            .unwrap_or_else(|| order_id.to_string());
        let market = e.get("market").and_then(|v| v.as_str()).unwrap_or("");
        Ok(OrderResponse {
            order_id: venue_oid,
            client_order_id: e
                .get("externalId")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            venue: self.venue_id.clone(),
            symbol: if market.is_empty() {
                _symbol.to_string()
            } else {
                to_base(market)
            },
            status,
            original_qty: original,
            filled_qty: filled,
            remaining_qty: (original - filled).max(dec!(0)),
            avg_fill_price: dec_from_json(e.get("averagePrice")),
            fee: dec_from_json(e.get("payedFee")),
            fee_currency: "USD".to_string(),
            raw_response: Some(e.to_string()),
            timestamp: Utc::now(),
        })
    }

    /// `GET /api/v1/user/balance` + `GET /api/v1/user/positions`.
    async fn extended_account(&self) -> anyhow::Result<AccountSnapshot> {
        let headers = self.extended_headers()?;
        let burl = format!("{}/api/v1/user/balance", self.base_url);
        let bresp = self.get_json_url(&burl, &headers).await?;
        let b = Self::extended_data(&bresp, "balance")?;
        let equity = dec_from_json(b.get("equity")).unwrap_or_default();
        let free = dec_from_json(b.get("availableForTrade")).unwrap_or_default();
        let maint = dec_from_json(b.get("initialMargin")).unwrap_or_default();
        let mut balances = Vec::new();
        let collateral = b
            .get("collateralName")
            .and_then(|v| v.as_str())
            .unwrap_or("USD");
        let bal = dec_from_json(b.get("balance")).unwrap_or_default();
        if bal != dec!(0) || equity != dec!(0) {
            balances.push(arbitrageur_core::position::Balance {
                venue: self.venue_id.clone(),
                currency: collateral.to_string(),
                total: bal,
                free,
                locked: (bal - free).max(dec!(0)),
            });
        }
        let purl = format!("{}/api/v1/user/positions", self.base_url);
        let presp = self.get_json_url(&purl, &headers).await?;
        let plist = Self::extended_data(&presp, "positions")
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let mut positions = Vec::new();
        for p in plist {
            if p.get("status").and_then(|v| v.as_str()) != Some("OPENED") {
                continue;
            }
            let market = p.get("market").and_then(|v| v.as_str()).unwrap_or("");
            let qty = dec_from_json(p.get("size")).unwrap_or_default();
            if qty == dec!(0) {
                continue;
            }
            let side_short = p
                .get("side")
                .and_then(|v| v.as_str())
                .map(|s| s.eq_ignore_ascii_case("SHORT"))
                .unwrap_or(false);
            positions.push(arbitrageur_core::position::Position {
                venue: self.venue_id.clone(),
                symbol: to_base(market),
                side: if side_short {
                    arbitrageur_core::types::Side::Sell
                } else {
                    arbitrageur_core::types::Side::Buy
                },
                quantity: qty,
                entry_price: dec_from_json(p.get("openPrice")).unwrap_or_default(),
                mark_price: dec_from_json(p.get("markPrice")).unwrap_or_default(),
                unrealized_pnl: dec_from_json(p.get("unrealisedPnl")).unwrap_or_default(),
                realized_pnl: dec_from_json(p.get("realisedPnl")).unwrap_or_default(),
                liquidation_price: dec_from_json(p.get("liquidationPrice")),
                leverage: dec_from_json(p.get("leverage")).unwrap_or(dec!(1)),
                initial_margin: dec!(0),
                maintenance_margin: dec!(0),
                updated_at: Utc::now(),
            });
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
            balances,
            positions,
            timestamp: Utc::now(),
        })
    }

    /// `PATCH /api/v1/user/leverage` `{market, leverage}` — plain
    /// `X-Api-Key` auth, no L2 signature.
    async fn extended_set_leverage(&self, symbol: &str, leverage: u32) -> anyhow::Result<()> {
        let headers = self.extended_headers()?;
        let vsym = self.venue_symbol(symbol);
        let url = format!("{}/api/v1/user/leverage", self.base_url);
        let body = serde_json::json!({"market": vsym, "leverage": leverage.to_string()});
        self.rate_limiter.acquire_signed().await?;
        let mut req = self.client.patch(&url).json(&body);
        for (k, v) in &headers {
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
                "Extended leverage update failed: {} — {}",
                status,
                text.trim().chars().take(300).collect::<String>()
            );
        }
        let resp: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("Extended leverage update unparseable: {e}"))?;
        // Leverage update may return 200 with an empty body on success —
        // only a present-but-non-OK envelope fails closed.
        if let Some(s) = resp.get("status").and_then(|v| v.as_str()) {
            if !s.eq_ignore_ascii_case("OK") {
                anyhow::bail!("Extended leverage update rejected: status={s}");
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// dYdX v4 — Cosmos-chain orders via the official `dydx` gRPC client
// ---------------------------------------------------------------------------
//
// dYdX v4 runs its own Cosmos appchain: orders are `MsgPlaceOrder`
// protobuf transactions signed with the account's secp256k1 key and
// broadcast to a validator gRPC endpoint. The official `dydx` crate
// (v4-client-rs) owns signing, quantization, sequencing and broadcast —
// this adapter resolves the mnemonic-derived account, maps order types,
// and pins venue truth to the public indexer REST (`/v4/orders/{id}`),
// whose order id is the protobuf-marshaled `OrderId` as hex.
//
// Credentials: `private_key` = the account's BIP-39 mnemonic (the venue
// derives m/44'/118'/0'/0/0 — a raw hex key cannot express that),
// `account_id` = subaccount number (default 0), `wallet_address` =
// optional `dydx1…` address verified against the derived account.

/// Live dYdX session — connected gRPC client plus the account carrying
/// fetched account/sequence numbers (managed sequencing is enabled).
struct DydxSession {
    client: dydx::node::NodeClient,
    account: dydx::node::Account,
    subaccount: u32,
}

/// Deterministic u32 client id derived from the client order id — the
/// indexer indexes orders by it, so client-id lookups survive restarts.
fn dydx_client_id(client_order_id: &str) -> u32 {
    use sha2::Digest;
    let h = sha2::Sha256::digest(client_order_id.as_bytes());
    u32::from_be_bytes(h[..4].try_into().unwrap())
}

/// `Decimal → BigDecimal` (the dydx crate's price/quantity newtypes wrap
/// BigDecimal).
fn dydx_bd(v: Decimal) -> anyhow::Result<bigdecimal::BigDecimal> {
    use std::str::FromStr;
    bigdecimal::BigDecimal::from_str(&v.normalize().to_string())
        .map_err(|e| anyhow::anyhow!("decimal {v} rejected: {e}"))
}

impl DexPerpAdapter {
    /// `(mnemonic, subaccount_number)` — fails closed without a mnemonic.
    fn dydx_auth(&self) -> anyhow::Result<(String, u32)> {
        let c = self.creds.read();
        let sub: u32 = c
            .account_id
            .as_deref()
            .map(|s| s.parse())
            .transpose()
            .map_err(|_| anyhow::anyhow!("dYdX subaccount number is not a u32"))?
            .unwrap_or(0);
        drop(c);
        let m = self
            .dydx_mnemonic
            .read()
            .as_ref()
            .map(|s| s.as_str().to_string())
            .ok_or_else(|| anyhow::anyhow!("dYdX mnemonic missing"))?;
        Ok((m, sub))
    }

    /// The derived `dydx1…` account address — offline derivation, no
    /// network needed (used by indexer reads).
    fn dydx_address(&self) -> anyhow::Result<String> {
        let (m, _) = self.dydx_auth()?;
        let wallet = dydx::node::Wallet::from_mnemonic(&m)
            .map_err(|e| anyhow::anyhow!("dYdX mnemonic rejected: {e}"))?;
        Ok(wallet.account_offline(0)?.address().to_string())
    }

    /// Connected gRPC session — built once, reused across calls. The
    /// account carries venue-fetched account/sequence numbers.
    async fn dydx_session(&self) -> anyhow::Result<Arc<tokio::sync::Mutex<DydxSession>>> {
        if let Some(s) = self.dydx_session.read().clone() {
            return Ok(s);
        }
        let _guard = self.dydx_build.lock().await;
        if let Some(s) = self.dydx_session.read().clone() {
            return Ok(s);
        }
        let (mnemonic, subaccount) = self.dydx_auth()?;
        let wallet = dydx::node::Wallet::from_mnemonic(&mnemonic)
            .map_err(|e| anyhow::anyhow!("dYdX mnemonic rejected: {e}"))?;
        let config = dydx::node::NodeConfig {
            endpoint: "https://dydx-grpc.publicnode.com:443".to_string(),
            timeout: 15_000,
            chain_id: dydx::node::ChainId::Mainnet1,
            fee_denom: dydx::indexer::Denom::Usdc,
            manage_sequencing: true,
        };
        let mut client = dydx::node::NodeClient::connect(config)
            .await
            .map_err(|e| anyhow::anyhow!("dYdX gRPC connect failed: {e}"))?;
        let account = wallet
            .account(0, &mut client)
            .await
            .map_err(|e| anyhow::anyhow!("dYdX account fetch failed: {e}"))?;
        let sess = Arc::new(tokio::sync::Mutex::new(DydxSession {
            client,
            account,
            subaccount,
        }));
        *self.dydx_session.write() = Some(sess.clone());
        Ok(sess)
    }

    /// `OrderMarketParams` for a canonical symbol — clobPairId,
    /// atomicResolution, quantumConversionExponent, stepBaseQuantums,
    /// subticksPerTick, oraclePrice from the indexer (cached; refetched
    /// on a miss so new listings don't need a restart).
    async fn dydx_market_params(
        &self,
        symbol: &str,
    ) -> anyhow::Result<(String, dydx::node::OrderMarketParams)> {
        let vsym = self.venue_symbol(symbol);
        if let Some(p) = self
            .dydx_markets
            .read()
            .as_ref()
            .and_then(|m| m.get(&vsym).cloned())
        {
            return Ok((vsym, p));
        }
        let url = format!("{}/v4/perpetualMarkets", self.base_url);
        let data = self.get_json_url(&url, &[]).await?;
        let arr = data
            .get("markets")
            .and_then(|v| v.as_object())
            .ok_or_else(|| anyhow::anyhow!("dYdX markets response missing markets"))?;
        let mut map: HashMap<String, dydx::node::OrderMarketParams> = HashMap::new();
        for (name, m) in arr {
            let (Some(clob), Some(atomic), Some(qce), Some(step), Some(subticks)) = (
                m.get("clobPairId")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<u32>().ok()),
                m.get("atomicResolution").and_then(|v| v.as_i64()),
                m.get("quantumConversionExponent").and_then(|v| v.as_i64()),
                m.get("stepBaseQuantums").and_then(|v| v.as_u64()),
                m.get("subticksPerTick").and_then(|v| v.as_u64()),
            ) else {
                continue;
            };
            let oracle = dec_from_json(m.get("oraclePrice")).map(|p| {
                dydx::indexer::Price(
                    bigdecimal::BigDecimal::from_str(&p.normalize().to_string())
                        .unwrap_or_default(),
                )
            });
            map.insert(
                name.clone(),
                dydx::node::OrderMarketParams {
                    atomic_resolution: atomic as i32,
                    clob_pair_id: clob.into(),
                    oracle_price: oracle,
                    quantum_conversion_exponent: qce as i32,
                    step_base_quantums: step,
                    subticks_per_tick: subticks as u32,
                },
            );
        }
        let params = map
            .get(&vsym)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("dYdX has no market for {vsym}"))?;
        *self.dydx_markets.write() = Some(Arc::new(map));
        Ok((vsym, params))
    }

    /// Indexer order row for an order-id hex or a client-order-id.
    async fn dydx_order_entry(
        &self,
        order_id: &str,
        client_order_id: Option<&str>,
    ) -> anyhow::Result<serde_json::Value> {
        if !order_id.is_empty() {
            let url = format!("{}/v4/orders/{}", self.base_url, order_id);
            if let Ok(resp) = self.get_json_url(&url, &[]).await {
                if resp.get("id").is_some() {
                    return Ok(resp);
                }
            }
        }
        if let Some(cid) = client_order_id.filter(|s| !s.is_empty()) {
            let owner = self.dydx_address()?;
            let (_, sub) = self.dydx_auth()?;
            let client_id = dydx_client_id(cid);
            let url = format!(
                "{}/v4/orders?address={owner}&subaccountNumber={sub}",
                self.base_url
            );
            let resp = self.get_json_url(&url, &[]).await?;
            let arr = resp
                .as_array()
                .cloned()
                .or_else(|| resp.get("orders").and_then(|v| v.as_array()).cloned())
                .unwrap_or_default();
            if let Some(e) = arr.iter().find(|e| {
                e.get("clientId")
                    .and_then(|v| v.as_str())
                    .map(|s| s == client_id.to_string())
                    .unwrap_or(false)
            }) {
                return Ok(e.clone());
            }
        }
        anyhow::bail!("dYdX order {order_id} not found on the indexer")
    }

    /// `MsgPlaceOrder` — quantized via `OrderMarketParams`, signed by the
    /// mnemonic-derived account, broadcast over the validator gRPC.
    async fn dydx_place(&self, order: &OrderRequest) -> anyhow::Result<OrderResponse> {
        use arbitrageur_core::types::{OrderType, Side, TimeInForce};
        use dydx::node::{
            OrderBuilder, OrderGoodUntil, OrderSide, OrderTimeInForce,
            SHORT_TERM_ORDER_MAXIMUM_LIFETIME,
        };
        let sess = self.dydx_session().await?;
        let (_vsym, params) = self.dydx_market_params(&order.symbol).await?;
        let is_buy = order.side == Side::Buy;
        let post_only = order.post_only || order.order_type == OrderType::PostOnly;
        let qty = order.quantity;
        if qty <= dec!(0) {
            anyhow::bail!("dYdX order quantity must be positive");
        }
        // Destructure the guard — `g.client.f(&mut g.account)` borrows the
        // whole guard twice through DerefMut and is rejected.
        let g = &mut *sess.lock().await;
        let DydxSession {
            client,
            account,
            subaccount,
        } = g;
        let sub = account
            .subaccount(*subaccount)
            .map_err(|e| anyhow::anyhow!("dYdX subaccount {} invalid: {e}", subaccount))?;
        let height = client
            .latest_block_height()
            .await
            .map_err(|e| anyhow::anyhow!("dYdX block height fetch failed: {e}"))?;
        let until = OrderGoodUntil::Block(height.ahead(SHORT_TERM_ORDER_MAXIMUM_LIFETIME));
        let side = if is_buy {
            OrderSide::Buy
        } else {
            OrderSide::Sell
        };
        let builder = OrderBuilder::new(params, sub)
            .until(until)
            .reduce_only(order.reduce_only);
        let builder = match order.order_type {
            OrderType::Market => {
                // Bound the signed slippage price off the live book — the
                // builder falls back to oracle ± slippage when unset.
                let book = self.fetch_orderbook(&order.symbol).await?;
                let bound = if is_buy {
                    book.asks
                        .first()
                        .map(|l| l.price * dec!(1.05))
                        .ok_or_else(|| {
                            anyhow::anyhow!("dYdX book has no asks to bound a market buy")
                        })?
                } else {
                    book.bids
                        .first()
                        .map(|l| l.price * dec!(0.95))
                        .ok_or_else(|| {
                            anyhow::anyhow!("dYdX book has no bids to bound a market sell")
                        })?
                };
                builder
                    .market(side, dydx::indexer::Quantity(dydx_bd(qty)?))
                    .price(dydx::indexer::Price(dydx_bd(bound)?))
                    .time_in_force(OrderTimeInForce::Ioc)
            }
            OrderType::Limit | OrderType::PostOnly | OrderType::ImmediateOrCancel => {
                let p = order
                    .price
                    .ok_or_else(|| anyhow::anyhow!("dYdX limit order requires a price"))?;
                let tif = if post_only {
                    OrderTimeInForce::PostOnly
                } else {
                    match order.time_in_force {
                        TimeInForce::ImmediateOrCancel => OrderTimeInForce::Ioc,
                        TimeInForce::FillOrKill => OrderTimeInForce::FillOrKill,
                        _ if order.order_type == OrderType::ImmediateOrCancel => {
                            OrderTimeInForce::Ioc
                        }
                        _ => OrderTimeInForce::Unspecified,
                    }
                };
                builder
                    .limit(
                        side,
                        dydx::indexer::Price(dydx_bd(p)?),
                        dydx::indexer::Quantity(dydx_bd(qty)?),
                    )
                    .time_in_force(tif)
            }
        };
        let client_id = dydx_client_id(&order.client_order_id);
        let (order_id, built) = builder
            .build(dydx::indexer::ClientId(client_id))
            .map_err(|e| anyhow::anyhow!("dYdX order build failed: {e}"))?;
        // Venue order id = hex of the marshaled OrderId proto (the
        // indexer's own key for the order).
        use prost::Message;
        let oid_hex = hex::encode(order_id.encode_to_vec());
        let tx = client
            .place_order(account, built)
            .await
            .map_err(|e| anyhow::anyhow!("dYdX place_order broadcast failed: {e}"))?;
        Ok(OrderResponse {
            order_id: oid_hex,
            client_order_id: order.client_order_id.clone(),
            venue: self.venue_id.clone(),
            symbol: order.symbol.clone(),
            status: arbitrageur_core::types::OrderStatus::New,
            original_qty: qty,
            filled_qty: dec!(0),
            remaining_qty: qty,
            avg_fill_price: None,
            fee: None,
            fee_currency: "USD".to_string(),
            raw_response: Some(format!("{tx:?}")),
            timestamp: Utc::now(),
        })
    }

    /// `MsgCancelOrder` — order identity comes from the marshaled
    /// `OrderId` hex (decoded back into the proto) or resolved through
    /// the indexer by client id.
    async fn dydx_cancel(&self, req: &CancelRequest) -> anyhow::Result<()> {
        use dydx::node::{OrderGoodUntil, OrderId, SHORT_TERM_ORDER_MAXIMUM_LIFETIME};
        use prost::Message;
        let oid_hex = if let Some(o) = req.order_id.as_deref().filter(|s| !s.is_empty()) {
            o.to_string()
        } else if req
            .client_order_id
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false)
        {
            let e = self
                .dydx_order_entry("", req.client_order_id.as_deref())
                .await?;
            e.get("id")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .ok_or_else(|| anyhow::anyhow!("dYdX order entry has no id"))?
        } else {
            anyhow::bail!("dYdX cancel needs an order id or client order id");
        };
        let bytes = hex::decode(&oid_hex)
            .map_err(|_| anyhow::anyhow!("dYdX order id is not hex: {oid_hex}"))?;
        let order_id = OrderId::decode(bytes.as_slice())
            .map_err(|e| anyhow::anyhow!("dYdX order id decode failed: {e}"))?;
        let sess = self.dydx_session().await?;
        let g = &mut *sess.lock().await;
        let DydxSession {
            client, account, ..
        } = g;
        let height = client
            .latest_block_height()
            .await
            .map_err(|e| anyhow::anyhow!("dYdX block height fetch failed: {e}"))?;
        client
            .cancel_order(
                account,
                order_id,
                OrderGoodUntil::Block(height.ahead(SHORT_TERM_ORDER_MAXIMUM_LIFETIME)),
            )
            .await
            .map_err(|e| anyhow::anyhow!("dYdX cancel broadcast failed: {e}"))?;
        Ok(())
    }

    /// `GET /v4/orders/{id}` / `?address=&subaccountNumber=` — indexer is
    /// the venue truth; broadcast success never implies a fill.
    async fn dydx_fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        let e = self.dydx_order_entry(order_id, client_order_id).await?;
        let status_raw = e.get("status").and_then(|v| v.as_str()).unwrap_or("");
        // BEST_EFFORT_* states: the order is on the book (OPENED) or
        // already dead (CANCELED) — normalize to the mapped vocabulary.
        let normalized = status_raw
            .replace("BEST_EFFORT_", "")
            .replace("OPENED", "OPEN");
        let original = dec_from_json(e.get("size")).unwrap_or_default();
        let filled = dec_from_json(e.get("totalFilled")).unwrap_or_default();
        let mut status = map_order_status(&normalized, filled, original).ok_or_else(|| {
            anyhow::anyhow!("dYdX order {order_id} has unknown status {status_raw}")
        })?;
        // dYdX reports OPEN for partially-filled orders — restore the
        // partial state from quantities so exits see real remaining size.
        if status == arbitrageur_core::types::OrderStatus::New
            && filled > dec!(0)
            && filled < original
        {
            status = arbitrageur_core::types::OrderStatus::PartiallyFilled;
        }
        let venue_oid = e
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or(order_id)
            .to_string();
        Ok(OrderResponse {
            order_id: venue_oid,
            client_order_id: client_order_id.unwrap_or_default().to_string(),
            venue: self.venue_id.clone(),
            symbol: e
                .get("ticker")
                .and_then(|v| v.as_str())
                .map(to_base)
                .unwrap_or_else(|| symbol.to_string()),
            status,
            original_qty: original,
            filled_qty: filled,
            remaining_qty: (original - filled).max(dec!(0)),
            avg_fill_price: dec_from_json(e.get("averageFillPrice")),
            fee: None,
            fee_currency: "USD".to_string(),
            raw_response: Some(e.to_string()),
            timestamp: Utc::now(),
        })
    }

    /// `GET /v4/addresses/{addr}` — equity/free collateral from the
    /// matching subaccount; open positions from `openPerpetualPositions`.
    async fn dydx_account(&self) -> anyhow::Result<AccountSnapshot> {
        let (mnemonic, sub) = self.dydx_auth()?;
        let wallet = dydx::node::Wallet::from_mnemonic(&mnemonic)
            .map_err(|e| anyhow::anyhow!("dYdX mnemonic rejected: {e}"))?;
        let address = wallet.account_offline(0)?.address().to_string();
        let url = format!(
            "{}/v4/addresses/{}/subaccountNumber/{}",
            self.base_url, address, sub
        );
        let resp = self.get_json_url(&url, &[]).await?;
        let d = resp.get("subaccount").cloned().unwrap_or(resp.clone());
        let equity = dec_from_json(d.get("equity")).unwrap_or_default();
        let free = dec_from_json(d.get("freeCollateral")).unwrap_or_default();
        let mut balances = Vec::new();
        if equity != dec!(0) || free != dec!(0) {
            balances.push(arbitrageur_core::position::Balance {
                venue: self.venue_id.clone(),
                currency: "USDC".to_string(),
                total: equity,
                free,
                locked: (equity - free).max(dec!(0)),
            });
        }
        let mut positions = Vec::new();
        if let Some(obj) = d.get("openPerpetualPositions").and_then(|v| v.as_object()) {
            for (ticker, p) in obj {
                let qty = dec_from_json(p.get("size")).unwrap_or_default();
                if qty == dec!(0) {
                    continue;
                }
                let short = p
                    .get("side")
                    .and_then(|v| v.as_str())
                    .map(|s| s.eq_ignore_ascii_case("SHORT"))
                    .unwrap_or(false);
                positions.push(arbitrageur_core::position::Position {
                    venue: self.venue_id.clone(),
                    symbol: to_base(ticker),
                    side: if short {
                        arbitrageur_core::types::Side::Sell
                    } else {
                        arbitrageur_core::types::Side::Buy
                    },
                    quantity: qty,
                    entry_price: dec_from_json(p.get("entryPrice")).unwrap_or_default(),
                    mark_price: dec_from_json(p.get("oraclePrice")).unwrap_or_default(),
                    unrealized_pnl: dec_from_json(p.get("unrealizedPnl")).unwrap_or_default(),
                    realized_pnl: dec_from_json(p.get("realizedPnl")).unwrap_or_default(),
                    liquidation_price: None,
                    leverage: dec!(1),
                    initial_margin: dec!(0),
                    maintenance_margin: dec!(0),
                    updated_at: Utc::now(),
                });
            }
        }
        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity,
            free_margin: free,
            maintenance_margin: dec!(0),
            margin_usage_pct: dec!(0),
            balances,
            positions,
            timestamp: Utc::now(),
        })
    }
}

// ---------------------------------------------------------------------------
// Helix / Injective v2 — Cosmos txs (keccak-signed SignDoc → LCD broadcast)
// with indexer gRPC truth (orders / positions / balances). The EVM private
// key is the account key: keccak address → `inj1…` bech32, subaccount id =
// `0x{addr}{11 zero bytes}{nonce}`.
// ---------------------------------------------------------------------------

/// Injective mainnet chain id.
const HELIX_CHAIN_ID: &str = "injective-1";
/// Injective exchange indexer gRPC (order/position/balance truth — the
/// chain LCD serves markets + tx broadcast only).
const HELIX_GRPC_URL: &str = "https://sentry.exchange.grpc.injective.network:443";
/// 1M gas at the Injective minimum gas price (5e8 atto-INJ/gas ≈ 0.0005 INJ).
const HELIX_GAS_LIMIT: u64 = 1_000_000;
const HELIX_FEE_ATTO_INJ: &str = "500000000000000";
/// Cosmos Any type-url for the Injective ethsecp256k1 pubkey wrapper.
const HELIX_PUBKEY_URL: &str = "/injective.crypto.v1beta1.ethsecp256k1.PubKey";
/// Indexer gRPC method paths (InjectiveDerivativeExchangeRPC).
const HELIX_PATH_HISTORY: &str =
    "/injective_derivative_exchange_rpc.InjectiveDerivativeExchangeRPC/OrdersHistory";
const HELIX_PATH_POSITIONS: &str =
    "/injective_derivative_exchange_rpc.InjectiveDerivativeExchangeRPC/Positions";
const HELIX_PATH_BALANCES: &str =
    "/injective_accounts_rpc.InjectiveAccountsRPC/SubaccountBalancesList";

/// `OrdersHistoryRequest` — open + terminal orders. `cid` (tag 16) is a
/// server-side exact-match filter.
#[derive(Clone, PartialEq, prost::Message)]
struct InjOrdersHistoryReq {
    #[prost(string, tag = "1")]
    subaccount_id: String,
    #[prost(string, tag = "2")]
    market_id: String,
    #[prost(uint64, tag = "3")]
    skip: u64,
    #[prost(sint32, tag = "4")]
    limit: i32,
    #[prost(string, tag = "16")]
    cid: String,
}

/// `DerivativeOrderHistory` — per-order state rows.
#[derive(Clone, PartialEq, prost::Message)]
struct InjOrderHistoryEntry {
    #[prost(string, tag = "1")]
    order_hash: String,
    #[prost(string, tag = "2")]
    market_id: String,
    #[prost(string, tag = "7")]
    price: String,
    #[prost(string, tag = "9")]
    quantity: String,
    #[prost(string, tag = "10")]
    filled_quantity: String,
    #[prost(string, tag = "11")]
    state: String,
    #[prost(string, tag = "21")]
    cid: String,
}

#[derive(Clone, PartialEq, prost::Message)]
struct InjOrdersHistoryResp {
    #[prost(message, repeated, tag = "1")]
    orders: Vec<InjOrderHistoryEntry>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct InjPositionsReq {
    #[prost(string, tag = "1")]
    subaccount_id: String,
    #[prost(string, tag = "2")]
    market_id: String,
    #[prost(uint64, tag = "3")]
    skip: u64,
    #[prost(sint32, tag = "4")]
    limit: i32,
}

/// `DerivativePosition` — indexer chain-format Dec strings (prices are
/// quote-decimal scaled).
#[derive(Clone, PartialEq, prost::Message)]
struct InjDerivativePosition {
    #[prost(string, tag = "1")]
    ticker: String,
    #[prost(string, tag = "2")]
    market_id: String,
    #[prost(string, tag = "4")]
    direction: String,
    #[prost(string, tag = "5")]
    quantity: String,
    #[prost(string, tag = "6")]
    entry_price: String,
    #[prost(string, tag = "7")]
    margin: String,
    #[prost(string, tag = "8")]
    liquidation_price: String,
    #[prost(string, tag = "9")]
    mark_price: String,
}

#[derive(Clone, PartialEq, prost::Message)]
struct InjPositionsResp {
    #[prost(message, repeated, tag = "1")]
    positions: Vec<InjDerivativePosition>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct InjBalancesReq {
    #[prost(string, tag = "1")]
    subaccount_id: String,
}

#[derive(Clone, PartialEq, prost::Message)]
struct InjSubaccountDeposit {
    #[prost(string, tag = "1")]
    total_balance: String,
    #[prost(string, tag = "2")]
    available_balance: String,
    #[prost(string, tag = "3")]
    total_balance_usd: String,
    #[prost(string, tag = "4")]
    available_balance_usd: String,
}

#[derive(Clone, PartialEq, prost::Message)]
struct InjSubaccountBalance {
    #[prost(string, tag = "1")]
    subaccount_id: String,
    #[prost(string, tag = "2")]
    account_address: String,
    #[prost(string, tag = "3")]
    denom: String,
    #[prost(message, optional, tag = "4")]
    deposit: Option<InjSubaccountDeposit>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct InjBalancesResp {
    #[prost(message, repeated, tag = "1")]
    balances: Vec<InjSubaccountBalance>,
}

/// Shared response envelope of `MsgCreateDerivative{Limit,Market}Order`
/// — both carry `order_hash` (tag 1) + `cid` (tag 2).
#[derive(Clone, PartialEq, prost::Message)]
struct InjOrderResult {
    #[prost(string, tag = "1")]
    order_hash: String,
    #[prost(string, tag = "2")]
    cid: String,
}

/// Injective `sdk.Dec` wire format — plain decimal string, ≤18dp
/// (normalized so integral values carry no fractional tail).
fn helix_dec(v: Decimal) -> String {
    v.trunc_with_scale(18).normalize().to_string()
}

/// Scale a human USD/price value to Injective chain units (`×10^qdec`).
fn helix_chain_value(v: Decimal, info: &HelixMarketInfo) -> String {
    helix_dec(v * info.scale)
}

/// Normalize an indexer `state` string to a form `map_order_status` knows.
/// Injective uses `booked` (resting), `partial_filled`, `filled`,
/// `canceled`.
fn helix_state(state: &str) -> String {
    match state {
        "booked" => "open".to_string(),
        "partial_filled" => "partially_filled".to_string(),
        "canceled" => "cancelled".to_string(),
        other => other.to_string(),
    }
}

impl DexPerpAdapter {
    /// Injective identity for the loaded credential: `(inj1… bech32
    /// address, 0x-prefixed 32-byte subaccount id)`. `account_id` selects
    /// the subaccount nonce (0-255, default 0); a configured
    /// `wallet_address` must equal the signer's address.
    fn helix_identity(&self) -> anyhow::Result<(String, String)> {
        let w = self.wallet.read();
        let signer = w.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "Helix requires the Injective account's EVM private key (privateKey field)"
            )
        })?;
        let eth = signer.address_hex();
        if let Some(cfg) = self.creds.read().wallet_address.as_deref() {
            if !cfg.eq_ignore_ascii_case(&eth) {
                anyhow::bail!(
                    "Helix wallet address does not match the signing key's derived address"
                );
            }
        }
        let hrp = bech32::Hrp::parse("inj")
            .map_err(|e| anyhow::anyhow!("bech32 HRP parse failed: {e}"))?;
        let inj_addr = bech32::encode::<bech32::Bech32>(hrp, &signer.address_bytes())
            .map_err(|e| anyhow::anyhow!("bech32 encode failed: {e}"))?;
        let nonce: u8 = self
            .creds
            .read()
            .account_id
            .as_deref()
            .map(|s| s.trim().parse::<u8>())
            .transpose()
            .map_err(|_| anyhow::anyhow!("Helix accountId must be the subaccount nonce (0-255)"))?
            .unwrap_or(0);
        // bytes32: 20-byte address || 11 zero bytes || nonce byte.
        let subaccount = format!("{}0000000000000000000000{nonce:02x}", eth.to_lowercase());
        Ok((inj_addr, subaccount))
    }

    /// On-chain account number + next sequence from the LCD — fetched
    /// fresh per tx so sequence can never go stale across orders.
    async fn helix_account_state(&self, inj_addr: &str) -> anyhow::Result<(u64, u64)> {
        let url = format!("{}/cosmos/auth/v1beta1/accounts/{inj_addr}", self.base_url);
        let data = self.get_json_url(&url, &[]).await?;
        let base = data.pointer("/account/base_account").ok_or_else(|| {
            anyhow::anyhow!(
                "Injective account {inj_addr} not found on-chain — fund it with INJ/USDT first"
            )
        })?;
        let num = base
            .get("account_number")
            .and_then(|v| v.as_str()?.parse::<u64>().ok().or_else(|| v.as_u64()));
        let seq = base
            .get("sequence")
            .and_then(|v| v.as_str()?.parse::<u64>().ok().or_else(|| v.as_u64()));
        match (num, seq) {
            (Some(n), Some(s)) => Ok((n, s)),
            _ => anyhow::bail!("Injective account query missing account_number/sequence"),
        }
    }

    /// Lazily-connected TLS channel to the exchange indexer.
    async fn helix_channel(&self) -> anyhow::Result<tonic::transport::Channel> {
        if let Some(c) = self.helix_channel.read().clone() {
            return Ok(c);
        }
        let _g = self.helix_build.lock().await;
        if let Some(c) = self.helix_channel.read().clone() {
            return Ok(c);
        }
        let channel = tonic::transport::Endpoint::from_static(HELIX_GRPC_URL)
            .tls_config(tonic::transport::ClientTlsConfig::new().with_webpki_roots())
            .map_err(|e| anyhow::anyhow!("Injective indexer TLS config failed: {e}"))?
            .connect()
            .await
            .map_err(|e| anyhow::anyhow!("Injective indexer gRPC connect failed: {e}"))?;
        *self.helix_channel.write() = Some(channel.clone());
        Ok(channel)
    }

    /// Unary gRPC call against the indexer (prost-encoded).
    async fn helix_unary<Req, Resp>(&self, path: &'static str, req: Req) -> anyhow::Result<Resp>
    where
        Req: prost::Message + 'static,
        Resp: prost::Message + Default + 'static,
    {
        let channel = self.helix_channel().await?;
        let mut grpc = tonic::client::Grpc::new(channel);
        grpc.ready()
            .await
            .map_err(|e| anyhow::anyhow!("Injective indexer gRPC not ready: {e}"))?;
        let resp = grpc
            .unary(
                tonic::Request::new(req),
                tonic::codegen::http::uri::PathAndQuery::from_static(path),
                tonic::codec::ProstCodec::default(),
            )
            .await
            .map_err(|e| anyhow::anyhow!("Injective indexer {path} failed: {e}"))?;
        Ok(resp.into_inner())
    }

    /// Build → sign → broadcast one Injective message. Returns the
    /// transaction hash after CheckTx acceptance. Signing recipe
    /// (verified against injective `sdk-ts`): keccak256 over the
    /// protobuf-serialized `SignDoc`, compact 64-byte `r||s` secp256k1
    /// signature, `ethsecp256k1` pubkey, `SIGN_MODE_DIRECT`.
    async fn helix_broadcast(&self, type_url: &str, msg_value: Vec<u8>) -> anyhow::Result<String> {
        use cosmos_sdk_proto::cosmos::{
            base::v1beta1::Coin,
            crypto::secp256k1::PubKey,
            tx::{
                signing::v1beta1::SignMode,
                v1beta1::{mode_info, AuthInfo, Fee, ModeInfo, SignDoc, SignerInfo, TxBody, TxRaw},
            },
        };
        use prost::Message;

        let (inj_addr, _) = self.helix_identity()?;
        let pubkey33 = {
            let w = self.wallet.read();
            w.as_ref()
                .ok_or_else(|| anyhow::anyhow!("Helix signing key missing"))?
                .compressed_pubkey()?
        };
        let (account_number, sequence) = self.helix_account_state(&inj_addr).await?;

        let body = TxBody {
            messages: vec![cosmos_sdk_proto::Any {
                type_url: type_url.to_string(),
                value: msg_value,
            }],
            ..Default::default()
        };
        let body_bytes = body.encode_to_vec();
        let auth = AuthInfo {
            signer_infos: vec![SignerInfo {
                public_key: Some(cosmos_sdk_proto::Any {
                    type_url: HELIX_PUBKEY_URL.to_string(),
                    value: PubKey {
                        key: pubkey33.to_vec(),
                    }
                    .encode_to_vec(),
                }),
                mode_info: Some(ModeInfo {
                    sum: Some(mode_info::Sum::Single(mode_info::Single {
                        mode: SignMode::Direct as i32,
                    })),
                }),
                sequence,
            }],
            fee: Some(Fee {
                amount: vec![Coin {
                    denom: "inj".to_string(),
                    amount: HELIX_FEE_ATTO_INJ.to_string(),
                }],
                gas_limit: HELIX_GAS_LIMIT,
                payer: String::new(),
                granter: String::new(),
            }),
            ..Default::default()
        };
        let auth_bytes = auth.encode_to_vec();
        let sign_doc = SignDoc {
            body_bytes: body_bytes.clone(),
            auth_info_bytes: auth_bytes.clone(),
            chain_id: HELIX_CHAIN_ID.to_string(),
            account_number,
        };
        let sig = {
            let w = self.wallet.read();
            w.as_ref()
                .ok_or_else(|| anyhow::anyhow!("Helix signing key missing"))?
                .sign_digest(&keccak256(sign_doc.encode_to_vec().as_slice()))?
        };
        let raw = TxRaw {
            body_bytes,
            auth_info_bytes: auth_bytes,
            signatures: vec![sig[..64].to_vec()],
        };
        use base64::Engine;
        let tx_b64 = base64::engine::general_purpose::STANDARD.encode(raw.encode_to_vec());
        let url = format!("{}/cosmos/tx/v1beta1/txs", self.base_url);
        let resp = self
            .post_json_url(
                &url,
                &serde_json::json!({"tx_bytes": tx_b64, "mode": "BROADCAST_MODE_SYNC"}),
                &[],
            )
            .await?;
        let tx = resp.get("tx_response").cloned().unwrap_or_default();
        let code = tx.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            anyhow::bail!(
                "Injective CheckTx rejected (code {code}): {}",
                tx.get("raw_log").and_then(|v| v.as_str()).unwrap_or("?")
            );
        }
        tx.get("txhash")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Injective broadcast returned no txhash"))
    }

    /// Wait for a broadcast tx to commit; returns the decoded first
    /// `msg_responses` payload (order result). Bails on on-chain failure
    /// or a 15s timeout — fail-closed, never optimistically confirmed.
    async fn helix_commit(&self, txhash: &str) -> anyhow::Result<InjOrderResult> {
        use cosmos_sdk_proto::cosmos::base::abci::v1beta1::TxMsgData;
        use prost::Message;
        let url = format!("{}/cosmos/tx/v1beta1/txs/{txhash}", self.base_url);
        let mut last_err: Option<anyhow::Error> = None;
        for _ in 0..15 {
            tokio::time::sleep(Duration::from_millis(1000)).await;
            match self.client.get(&url).send().await {
                Ok(r) if r.status().is_success() => {
                    let v: serde_json::Value = match r.json().await {
                        Ok(v) => v,
                        Err(e) => {
                            last_err = Some(e.into());
                            continue;
                        }
                    };
                    let txr = v.get("tx_response").cloned().unwrap_or_default();
                    let code = txr.get("code").and_then(|c| c.as_i64());
                    match code {
                        Some(0) => {
                            let data_b64 = txr.get("data").and_then(|d| d.as_str()).unwrap_or("");
                            use base64::Engine;
                            let bytes = base64::engine::general_purpose::STANDARD
                                .decode(data_b64.as_bytes())
                                .map_err(|e| {
                                    anyhow::anyhow!("Injective tx data not base64: {e}")
                                })?;
                            let msg_data = TxMsgData::decode(bytes.as_slice())
                                .map_err(|e| anyhow::anyhow!("TxMsgData decode failed: {e}"))?;
                            let first = msg_data.msg_responses.first().ok_or_else(|| {
                                anyhow::anyhow!("Injective tx carried no msg response")
                            })?;
                            return InjOrderResult::decode(first.value.as_slice())
                                .map_err(|e| anyhow::anyhow!("order response decode failed: {e}"));
                        }
                        Some(c) => {
                            anyhow::bail!(
                                "Injective tx {txhash} failed on-chain (code {c}): {}",
                                txr.get("raw_log").and_then(|v| v.as_str()).unwrap_or("?")
                            );
                        }
                        None => continue,
                    }
                }
                Ok(r) => {
                    last_err = Some(anyhow::anyhow!("tx query HTTP {}", r.status()));
                }
                Err(e) => last_err = Some(e.into()),
            }
        }
        Err(last_err
            .unwrap_or_else(|| anyhow::anyhow!("Injective tx {txhash} not confirmed within 15s")))
        .map_err(|e| e.context(format!("Injective tx {txhash} commit poll failed")))
    }

    /// Extract the committed order hash (or bail — tx failure / timeout).
    async fn helix_place(&self, order: &OrderRequest) -> anyhow::Result<OrderResponse> {
        use arbitrageur_core::types::{OrderType, Side, TimeInForce};
        use injective_std::types::injective::exchange::v2 as injex;
        use prost::Message;

        let info = self.helix_resolve(&to_base(&order.symbol)).await?;
        let (inj_addr, subaccount) = self.helix_identity()?;
        let qty = order.quantity;
        if qty <= dec!(0) {
            anyhow::bail!("Helix order quantity must be positive");
        }
        // Injective requires exact tick multiples — floor qty (never
        // oversize) and snap price to the nearest tick.
        let qty_tick = (qty / info.size_tick).trunc() * info.size_tick;
        if qty_tick <= dec!(0) {
            anyhow::bail!(
                "Helix quantity {qty} truncates to zero at tick {}",
                info.size_tick
            );
        }
        let is_buy = order.side == Side::Buy;
        let post_only = order.post_only || order.order_type == OrderType::PostOnly;
        let cid = if order.client_order_id.is_empty() {
            format!("arb{}", Utc::now().timestamp_millis())
        } else {
            order.client_order_id.clone()
        };
        // Margin the order must carry: qty×price×IMR (reduce-only → 0).
        let margin_for = |px: Decimal| -> String {
            if order.reduce_only {
                "0".to_string()
            } else {
                helix_chain_value(qty_tick * px * info.initial_margin_ratio, &info)
            }
        };
        let order_type_of = |buy: bool, po: bool, atomic: bool| -> i32 {
            match (buy, po, atomic) {
                (_, true, _) => {
                    if buy {
                        injex::OrderType::BuyPo as i32
                    } else {
                        injex::OrderType::SellPo as i32
                    }
                }
                (_, false, true) => {
                    if buy {
                        injex::OrderType::BuyAtomic as i32
                    } else {
                        injex::OrderType::SellAtomic as i32
                    }
                }
                _ => {
                    if buy {
                        injex::OrderType::Buy as i32
                    } else {
                        injex::OrderType::Sell as i32
                    }
                }
            }
        };
        let order_info = |price: Decimal| injex::OrderInfo {
            subaccount_id: subaccount.clone(),
            fee_recipient: inj_addr.clone(),
            price: helix_chain_value(price, &info),
            quantity: helix_dec(qty_tick),
            cid: cid.clone(),
        };
        let (type_url, msg) = match order.order_type {
            OrderType::Market => {
                if matches!(order.time_in_force, TimeInForce::FillOrKill) {
                    anyhow::bail!(
                        "Helix market orders cannot be FOK — Injective has no FOK derivative order type"
                    );
                }
                if post_only {
                    anyhow::bail!("Helix market orders cannot be post-only");
                }
                let book = self.fetch_orderbook(&order.symbol).await?;
                let bound = if is_buy {
                    book.asks.first().map(|l| l.price * dec!(1.05))
                } else {
                    book.bids.first().map(|l| l.price * dec!(0.95))
                }
                .ok_or_else(|| anyhow::anyhow!("Helix book empty side for market order"))?;
                let msg = injex::MsgCreateDerivativeMarketOrder {
                    sender: inj_addr.clone(),
                    order: Some(injex::DerivativeOrder {
                        market_id: info.market_id.clone(),
                        order_info: Some(order_info(bound)),
                        order_type: order_type_of(is_buy, false, true),
                        margin: margin_for(bound),
                        trigger_price: "0".to_string(),
                        expiration_block: 0,
                    }),
                };
                (
                    "/injective.exchange.v2.MsgCreateDerivativeMarketOrder",
                    msg.encode_to_vec(),
                )
            }
            _ => {
                let raw_price = order
                    .price
                    .ok_or_else(|| anyhow::anyhow!("Helix limit orders require a price"))?;
                if raw_price <= dec!(0) {
                    anyhow::bail!("Helix order price must be positive");
                }
                let price = (raw_price / info.price_tick).round() * info.price_tick;
                if price <= dec!(0) {
                    anyhow::bail!(
                        "Helix price {raw_price} snaps to zero at tick {}",
                        info.price_tick
                    );
                }
                // order_type is the semantic — an IOC request with a stale
                // GTC tif must stay atomic, never rest on the book.
                let atomic = order.order_type == OrderType::ImmediateOrCancel
                    || matches!(
                        order.time_in_force,
                        TimeInForce::ImmediateOrCancel | TimeInForce::FillOrKill
                    );
                if matches!(order.time_in_force, TimeInForce::FillOrKill) {
                    // Injective's atomic type is IOC-style (partial fills
                    // cancel the rest); FOK all-or-nothing is not
                    // expressible — fail closed rather than approximate.
                    anyhow::bail!("Helix does not support fill-or-kill derivative orders");
                }
                let msg = injex::MsgCreateDerivativeLimitOrder {
                    sender: inj_addr.clone(),
                    order: Some(injex::DerivativeOrder {
                        market_id: info.market_id.clone(),
                        order_info: Some(order_info(price)),
                        order_type: order_type_of(is_buy, post_only, atomic),
                        margin: margin_for(price),
                        trigger_price: "0".to_string(),
                        expiration_block: 0,
                    }),
                };
                (
                    "/injective.exchange.v2.MsgCreateDerivativeLimitOrder",
                    msg.encode_to_vec(),
                )
            }
        };
        let txhash = self.helix_broadcast(type_url, msg).await?;
        let result = self.helix_commit(&txhash).await?;
        if result.order_hash.is_empty() {
            anyhow::bail!("Injective order committed without an order hash (tx {txhash})");
        }
        Ok(OrderResponse {
            order_id: result.order_hash,
            client_order_id: cid,
            venue: self.venue_id.clone(),
            symbol: order.symbol.clone(),
            status: arbitrageur_core::types::OrderStatus::New,
            original_qty: qty_tick,
            filled_qty: dec!(0),
            remaining_qty: qty_tick,
            avg_fill_price: None,
            fee: None,
            fee_currency: info.quote.clone(),
            raw_response: Some(txhash),
            timestamp: Utc::now(),
        })
    }

    async fn helix_cancel(&self, req: &CancelRequest) -> anyhow::Result<()> {
        use injective_std::types::injective::exchange::v2 as injex;
        use prost::Message;

        let info = self.helix_resolve(&to_base(&req.symbol)).await?;
        let (inj_addr, subaccount) = self.helix_identity()?;
        // Injective accepts either order_hash or cid — pass both when
        // known so the cancel resolves whichever identity the caller kept.
        if req.order_id.as_deref().unwrap_or("").is_empty()
            && req.client_order_id.as_deref().unwrap_or("").is_empty()
        {
            anyhow::bail!("Helix cancel requires an order id or client order id");
        }
        let msg = injex::MsgCancelDerivativeOrder {
            sender: inj_addr,
            market_id: info.market_id,
            subaccount_id: subaccount,
            order_hash: req
                .order_id
                .clone()
                .or_else(|| req.client_order_id.clone())
                .unwrap_or_default(),
            order_mask: 0,
            cid: req.client_order_id.clone().unwrap_or_default(),
        };
        let txhash = self
            .helix_broadcast(
                "/injective.exchange.v2.MsgCancelDerivativeOrder",
                msg.encode_to_vec(),
            )
            .await?;
        // Commit-wait: a CheckTx-accepted cancel can still be rejected
        // on-chain (unknown order) — treat non-commit as failure.
        match self.helix_commit(&txhash).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e.context(format!("Helix cancel for {:?} failed", req.order_id))),
        }
    }

    /// Order truth: the exchange indexer's order history is authoritative.
    /// Matches by `order_hash` first, then `cid`.
    async fn helix_fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        let info = self.helix_resolve(&to_base(symbol)).await?;
        let (_, subaccount) = self.helix_identity()?;
        let resp: InjOrdersHistoryResp = self
            .helix_unary(
                HELIX_PATH_HISTORY,
                InjOrdersHistoryReq {
                    subaccount_id: subaccount.clone(),
                    market_id: info.market_id.clone(),
                    skip: 0,
                    limit: 100,
                    cid: client_order_id.unwrap_or_default().to_string(),
                },
            )
            .await?;
        let entry = resp
            .orders
            .into_iter()
            .find(|o| {
                (!order_id.is_empty() && o.order_hash.eq_ignore_ascii_case(order_id))
                    || client_order_id
                        .map(|c| !c.is_empty() && o.cid == c)
                        .unwrap_or(false)
            })
            .ok_or_else(|| anyhow::anyhow!("Helix order {order_id} not found in order history"))?;
        let original = Decimal::from_str(&entry.quantity).unwrap_or_default();
        let filled = Decimal::from_str(&entry.filled_quantity).unwrap_or_default();
        let state = helix_state(&entry.state);
        let status = map_order_status(&state, filled, original).ok_or_else(|| {
            anyhow::anyhow!(
                "Helix order {} in unmapped state {}",
                entry.order_hash,
                entry.state
            )
        })?;
        Ok(OrderResponse {
            order_id: entry.order_hash,
            client_order_id: entry.cid,
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            status,
            original_qty: original,
            filled_qty: filled,
            remaining_qty: (original - filled).max(dec!(0)),
            avg_fill_price: Decimal::from_str(&entry.price).ok().map(|p| p / info.scale),
            fee: None,
            fee_currency: info.quote.clone(),
            raw_response: None,
            timestamp: Utc::now(),
        })
    }

    /// Account truth: subaccount deposit balances + derivative positions
    /// from the indexer gRPC.
    async fn helix_account(&self) -> anyhow::Result<AccountSnapshot> {
        let (_, subaccount) = self.helix_identity()?;
        let bal: InjBalancesResp = self
            .helix_unary(
                HELIX_PATH_BALANCES,
                InjBalancesReq {
                    subaccount_id: subaccount.clone(),
                },
            )
            .await?;
        let mut equity = dec!(0);
        let mut free = dec!(0);
        let mut balances = Vec::new();
        for b in &bal.balances {
            if let Some(d) = &b.deposit {
                // Indexer provides USD valuations directly; perp
                // subaccounts hold stable quotes so raw ≈ USD.
                let total_usd = Decimal::from_str(&d.total_balance_usd)
                    .or_else(|_| Decimal::from_str(&d.total_balance))
                    .unwrap_or_default();
                let avail_usd = Decimal::from_str(&d.available_balance_usd)
                    .or_else(|_| Decimal::from_str(&d.available_balance))
                    .unwrap_or_default();
                equity += total_usd;
                free += avail_usd;
                if total_usd > dec!(0) {
                    balances.push(arbitrageur_core::position::Balance {
                        venue: self.venue_id.clone(),
                        currency: b.denom.clone(),
                        total: total_usd,
                        free: avail_usd,
                        locked: (total_usd - avail_usd).max(dec!(0)),
                    });
                }
            }
        }
        let pos: InjPositionsResp = self
            .helix_unary(
                HELIX_PATH_POSITIONS,
                InjPositionsReq {
                    subaccount_id: subaccount.clone(),
                    market_id: String::new(),
                    skip: 0,
                    limit: 100,
                },
            )
            .await?;
        let markets = self
            .get_json("/injective/exchange/v1beta1/derivative/markets")
            .await
            .map(|d| helix_market_infos(&d))
            .unwrap_or_default();
        let mut positions = Vec::new();
        for p in pos.positions {
            let qty = Decimal::from_str(&p.quantity).unwrap_or_default();
            if qty == dec!(0) {
                continue;
            }
            let info = markets.iter().find(|m| m.market_id == p.market_id);
            let scale = info.map(|i| i.scale).unwrap_or(dec!(1));
            let entry = Decimal::from_str(&p.entry_price).unwrap_or_default() / scale;
            let mark = Decimal::from_str(&p.mark_price).unwrap_or_default() / scale;
            let liq_raw = Decimal::from_str(&p.liquidation_price).unwrap_or_default();
            let liq = if liq_raw > dec!(0) {
                Some(liq_raw / scale)
            } else {
                None
            };
            let is_long = !p.direction.eq_ignore_ascii_case("short");
            let upnl = if is_long {
                qty * (mark - entry)
            } else {
                qty * (entry - mark)
            };
            positions.push(arbitrageur_core::position::Position {
                venue: self.venue_id.clone(),
                symbol: info
                    .map(|i| format!("{}-USD:PERP", i.base))
                    .unwrap_or_else(|| p.ticker.clone()),
                side: if is_long {
                    arbitrageur_core::types::Side::Buy
                } else {
                    arbitrageur_core::types::Side::Sell
                },
                quantity: qty,
                entry_price: entry,
                mark_price: mark,
                unrealized_pnl: upnl,
                realized_pnl: dec!(0),
                liquidation_price: liq,
                leverage: dec!(1),
                initial_margin: Decimal::from_str(&p.margin).unwrap_or_default() / scale,
                maintenance_margin: dec!(0),
                updated_at: Utc::now(),
            });
        }
        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity,
            free_margin: free,
            maintenance_margin: dec!(0),
            margin_usage_pct: dec!(0),
            balances,
            positions,
            timestamp: Utc::now(),
        })
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
        for id in ["apex", "bulk"] {
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
             "taker_fee_rate": "0.000001000000000000",
             "initial_margin_ratio": "0.050000000000000000"}},
            {"market": {"ticker": "OLD/USDC PERP",
             "market_id": "0xdef", "status": "Paused", "quote_decimals": 6,
             "min_price_tick_size": "1000000.0",
             "min_quantity_tick_size": "0.00001",
             "taker_fee_rate": "0.000001",
             "initial_margin_ratio": "0.05"}}
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
        assert_eq!(edgex.venue_symbol("BTC"), "BTCUSDC");
        let grvt = DexPerpAdapter::grvt(None);
        assert_eq!(grvt.venue_symbol("BTC"), "BTC_USDT_Perp");
        let ondo = DexPerpAdapter::ondoperps(None);
        assert_eq!(ondo.venue_symbol("BTC"), "BTC-USD.P");
    }

    #[test]
    fn parse_new_venue_markets_match_live_shapes() {
        // edgeX v2 contract metadata shape — `{BASE}USDC` names, and the
        // disabled/display gating flags ride on `enableTrade`/`enableDisplay`.
        let venue = VenueId::new("edgex");
        let data = serde_json::json!({"code": "SUCCESS", "data": {"contractList": [
            {"contractId": "30000001", "contractName": "BTCUSDC", "tickSize": "0.1",
             "stepSize": "0.001", "enableTrade": true, "enableDisplay": true},
            {"contractId": "30000002", "contractName": "ETHUSDC", "tickSize": "0.01",
             "stepSize": "0.001", "enableTrade": false, "enableDisplay": true}
        ], "coinList": []}});
        let out = parse_edgex_markets(&venue, &data, dec!(4.5));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].venue_symbol, "BTCUSDC");
        assert_eq!(out[0].symbol.quote, "USDC");

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
        assert!(!a.has_live_credentials());
        // GMX has no book by construction.
        let gmx = DexPerpAdapter::gmx(None);
        assert!(gmx.fetch_orderbook("BTC").await.is_err());
    }

    #[test]
    fn vest_market_parser_matches_live_exchangeinfo_shape() {
        // Captured live 2026-09-23 (server-prod.hz.vestmarkets.com).
        let venue = VenueId::new("vest");
        let data = serde_json::json!({
            "exchange": {"lp": "942556.39", "insurance": "0", "collateralDecimals": 6},
            "symbols": [
                {"symbol": "BTC-PERP", "displaySymbol": "BTC-PERP",
                 "base": "BTC", "quote": "USDC", "sizeDecimals": 4,
                 "priceDecimals": 2, "takerFee": "0.0001",
                 "asset": "crypto", "tradingStatus": "TRADING",
                 "tickSizes": ["0.01", "0.1", "1", "10", "100"]},
                {"symbol": "AAPL-USD-PERP", "displaySymbol": "AAPL-USD-PERP",
                 "base": "AAPL-USD", "quote": "USDC", "sizeDecimals": 4,
                 "priceDecimals": 2, "takerFee": "0",
                 "asset": "stock", "tradingStatus": "TRADING",
                 "tickSizes": ["0.01", "0.1", "1"]},
                {"symbol": "EUR-USD-PERP", "displaySymbol": "EUR-USD-PERP",
                 "base": "EUR-USD", "quote": "USDC", "sizeDecimals": 0,
                 "priceDecimals": 6, "takerFee": "0",
                 "asset": "forex", "tradingStatus": "TRADING",
                 "tickSizes": ["0.000001", "0.00001", "0.0001", "0.001"]},
                {"symbol": "kPEPE-PERP", "base": "kPEPE", "quote": "USDC",
                 "sizeDecimals": 0, "priceDecimals": 6, "takerFee": "0.0005",
                 "asset": "crypto", "tradingStatus": "HALT",
                 "tickSizes": ["0.000001"]}
            ]
        });
        let out = parse_vest_markets(&venue, &data, dec!(1.0));
        // HALT entry is skipped per entry.
        assert_eq!(out.len(), 3);
        let btc = out.iter().find(|m| m.venue_symbol == "BTC-PERP").unwrap();
        assert_eq!(btc.symbol.base, "BTC");
        assert_eq!(btc.symbol.quote, "USDC");
        assert_eq!(btc.taker_fee_bps, dec!(1.0)); // "0.0001" → 1bp
        assert_eq!(btc.price_decimals, 2); // finest tickSize 0.01
        assert_eq!(btc.size_decimals, 4);
        let aapl = out
            .iter()
            .find(|m| m.venue_symbol == "AAPL-USD-PERP")
            .unwrap();
        assert_eq!(aapl.symbol.base, "AAPL");
        assert_eq!(aapl.symbol.quote, "USD");
        assert_eq!(aapl.taker_fee_bps, dec!(0)); // stock perps fee-free
        let eur = out
            .iter()
            .find(|m| m.venue_symbol == "EUR-USD-PERP")
            .unwrap();
        assert_eq!(eur.size_decimals, 0);
        assert_eq!(eur.price_decimals, 6); // tickSizes floor wins
        assert!(!out.iter().any(|m| m.venue_symbol == "kPEPE-PERP"));
    }

    #[test]
    fn vest_venue_symbol_covers_crypto_and_equity_families() {
        let vest = DexPerpAdapter::vest(None);
        // Crypto canonical (BTC-USDC / bare BTC) → {BASE}-PERP.
        assert_eq!(vest.venue_symbol("BTC-USDC"), "BTC-PERP");
        assert_eq!(vest.venue_symbol("btc"), "BTC-PERP");
        // Equity/forex canonical ({T}-USD) keeps the USD leg → {T}-USD-PERP.
        assert_eq!(vest.venue_symbol("AAPL-USD"), "AAPL-USD-PERP");
        assert_eq!(vest.venue_symbol("SPX-USD"), "SPX-USD-PERP");
    }

    #[tokio::test]
    async fn vest_credential_gate_and_fail_closed_order_paths() {
        let vest = DexPerpAdapter::vest(None);
        assert!(!vest.has_live_credentials());
        assert!(vest.supports_live_execution());
        // Post-only is refused before any auth/network work — Vest has
        // no maker-only order type.
        let req = OrderRequest {
            client_order_id: "t".into(),
            venue: vest.venue_id(),
            symbol: "BTC-USDC".into(),
            side: arbitrageur_core::types::Side::Buy,
            order_type: arbitrageur_core::types::OrderType::PostOnly,
            price: Some(dec!(50000)),
            quantity: dec!(0.01),
            time_in_force: arbitrageur_core::types::TimeInForce::PostOnly,
            post_only: true,
            reduce_only: false,
            created_at: Utc::now(),
        };
        let err = vest.place_order(req.clone()).await.unwrap_err().to_string();
        assert!(err.contains("post-only"), "unexpected error: {err}");
        // Limit order with no creds fails closed at auth — never reaches
        // the venue.
        let req = OrderRequest {
            order_type: arbitrageur_core::types::OrderType::Limit,
            time_in_force: arbitrageur_core::types::TimeInForce::GoodTilCancelled,
            post_only: false,
            ..req
        };
        let err = vest.place_order(req).await.unwrap_err().to_string();
        assert!(err.contains("signing key"), "unexpected error: {err}");

        // Delegate-signer config: signer != primary needs apiKey+accGroup.
        let signer_key = "0x0000000000000000000000000000000000000000000000000000000000000001";
        // secp256k1 key 0x01 → 0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf.
        vest.update_credentials(
            None,
            None,
            None,
            None,
            Some("0x0000000000000000000000000000000000000001".to_string()),
            Some(signer_key.to_string()),
        )
        .unwrap();
        // Delegate without apiKey → not live-ready (cannot self-register).
        assert!(!vest.has_live_credentials());
        // With apiKey + accGroup the delegate path is credentialed.
        vest.update_credentials(
            Some("apikey".to_string()),
            None,
            None,
            Some("0".to_string()),
            Some("0x0000000000000000000000000000000000000001".to_string()),
            Some(signer_key.to_string()),
        )
        .unwrap();
        assert!(vest.has_live_credentials());
        // Primary-key config (walletAddress == derived signer) is
        // live-ready without apiKey — self-registers on first use.
        vest.update_credentials(
            None,
            None,
            None,
            None,
            Some("0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf".to_string()),
            Some(signer_key.to_string()),
        )
        .unwrap();
        assert!(vest.has_live_credentials());
    }

    #[test]
    fn vest_signing_is_rfc6979_deterministic() {
        let vest = DexPerpAdapter::vest(None);
        vest.update_credentials(
            None,
            None,
            None,
            None,
            None,
            Some("0x0000000000000000000000000000000000000000000000000000000000000001".to_string()),
        )
        .unwrap();
        let hash = keccak256(b"vest-test-vector");
        let s1 = vest.vest_sign(&hash.0).unwrap();
        let s2 = vest.vest_sign(&hash.0).unwrap();
        assert_eq!(s1, s2, "k256 RFC6979 must be deterministic");
        assert!(s1.starts_with("0x") && s1.len() == 132);
    }

    #[test]
    fn qfex_status_mapping_covers_terminal_and_rejection_codes() {
        use arbitrageur_core::types::OrderStatus::*;
        assert_eq!(qfex_map_status("ACK", dec!(1), dec!(1)), Some(New));
        assert_eq!(
            qfex_map_status("ACK", dec!(1), dec!(0.4)),
            Some(PartiallyFilled)
        );
        assert_eq!(qfex_map_status("FILLED", dec!(1), dec!(0)), Some(Filled));
        assert_eq!(
            qfex_map_status("FILLED", dec!(1), dec!(0.3)),
            Some(PartiallyFilled)
        );
        assert_eq!(
            qfex_map_status("IOC_PARTIALLY_FILLED", dec!(1), dec!(0.3)),
            Some(Cancelled)
        );
        assert_eq!(
            qfex_map_status("NO_SUCH_ORDER", dec!(1), dec!(1)),
            Some(Cancelled)
        );
        assert_eq!(
            qfex_map_status("REJECTED_MUST_CHANGE_VALUE_MODIFY", dec!(1), dec!(1)),
            Some(Rejected)
        );
        assert_eq!(
            qfex_map_status("FAILED_MARGIN_CHECK", dec!(1), dec!(1)),
            Some(Rejected)
        );
        // Unknown strings fail closed.
        assert_eq!(qfex_map_status("SOME_NEW_STATE", dec!(1), dec!(0)), None);
    }

    #[test]
    fn qfex_credential_gate_needs_key_pair() {
        let qfex = DexPerpAdapter::qfex(None);
        assert!(!qfex.has_live_credentials());
        assert!(qfex.supports_live_execution());
        qfex.update_credentials(Some("qfex_pub_x".to_string()), None, None, None, None, None)
            .unwrap();
        assert!(!qfex.has_live_credentials(), "public key alone is not live");
        qfex.update_credentials(
            Some("qfex_pub_x".to_string()),
            Some("qfex_secret_y".to_string()),
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(qfex.has_live_credentials());
    }

    #[tokio::test]
    async fn qfex_order_paths_fail_closed_without_keys() {
        let qfex = DexPerpAdapter::qfex(None);
        let req = OrderRequest {
            client_order_id: "t".into(),
            venue: qfex.venue_id(),
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
        // No creds → session can't authenticate → bail before any order.
        let err = qfex.place_order(req).await.unwrap_err().to_string();
        assert!(err.contains("API key"), "unexpected error: {err}");
        assert!(qfex.fetch_account().await.is_err());
        assert!(qfex.fetch_order("BTC-USD", "missing", None).await.is_err());
    }

    #[test]
    fn parse_nado_markets_matches_live_symbols_shape() {
        // Trimmed live `query?type=symbols&product_type=perp` payload.
        let data = serde_json::json!({
            "status": "success",
            "data": {
                "symbols": {
                    "BTC-PERP": {
                        "type": "perp", "product_id": 2, "symbol": "BTC-PERP",
                        "price_increment_x18": "10000000000000000",
                        "size_increment": "100000000000000",
                        "min_size": "100000000000000000000",
                        "taker_fee_rate_x18": "350000000000000",
                        "trading_status": "live", "isolated_only": false
                    },
                    "WETH": {
                        "type": "spot", "product_id": 3, "symbol": "WETH",
                        "price_increment_x18": "100000000000000000",
                        "size_increment": "1000000000000000",
                        "min_size": "0", "trading_status": "live"
                    },
                    "OLD-PERP": {
                        "type": "perp", "product_id": 99, "symbol": "OLD-PERP",
                        "price_increment_x18": "10000000000000000",
                        "size_increment": "100000000000000",
                        "min_size": "0", "trading_status": "reduce_only"
                    }
                }
            }
        });
        let venue = VenueId::new("nado");
        let out = parse_nado_markets(&venue, &data, dec!(3.5));
        // Spot and non-live perp entries are skipped.
        assert_eq!(out.len(), 1);
        let btc = &out[0];
        assert_eq!(btc.venue_symbol, "BTC-PERP");
        assert_eq!(btc.symbol.base, "BTC");
        assert_eq!(btc.symbol.quote, "USDT0");
        assert_eq!(btc.price_decimals, 2); // 0.01 tick
        assert_eq!(btc.size_decimals, 4); // 0.0001 step
        assert_eq!(btc.min_notional, dec!(100));
        assert_eq!(btc.taker_fee_bps, dec!(3.5));
    }

    #[test]
    fn nado_sender_and_appendix_encoding() {
        let nado = DexPerpAdapter::nado(None);
        nado.update_credentials(
            None,
            None,
            None,
            None,
            Some("0x7a5ec2748e9065794491a8d29dcf3f9edb8d7c43".into()),
            Some(format!("{:064x}", 1)),
        )
        .unwrap();
        let sender = nado.nado_sender_bytes().unwrap();
        assert_eq!(
            hex::encode(sender),
            "7a5ec2748e9065794491a8d29dcf3f9edb8d7c4364656661756c740000000000",
            "address + ASCII 'default' null-padded to 12 bytes"
        );
        // Overlong subaccount names fail closed.
        nado.update_credentials(
            None,
            None,
            None,
            Some("waytoolongsubacct".into()),
            Some("0x7a5ec2748e9065794491a8d29dcf3f9edb8d7c43".into()),
            Some(format!("{:064x}", 1)),
        )
        .unwrap();
        assert!(nado.nado_sender_bytes().is_err());
    }

    #[test]
    fn nado_appendix_maps_engine_flags_to_wire_bits() {
        use arbitrageur_core::types::{OrderType, TimeInForce};
        let base = OrderRequest {
            client_order_id: "t".into(),
            venue: VenueId::new("nado"),
            symbol: "BTC-USDT0".into(),
            side: arbitrageur_core::types::Side::Buy,
            order_type: OrderType::Limit,
            price: Some(dec!(1)),
            quantity: dec!(1),
            time_in_force: TimeInForce::GoodTilCancelled,
            post_only: false,
            reduce_only: false,
            created_at: Utc::now(),
        };
        assert_eq!(nado_appendix(&base), 1, "GTC limit = version only");
        let mut ioc = base.clone();
        ioc.time_in_force = TimeInForce::ImmediateOrCancel;
        assert_eq!(nado_appendix(&ioc), 1 | (1 << 9));
        let mut fok = base.clone();
        fok.time_in_force = TimeInForce::FillOrKill;
        assert_eq!(nado_appendix(&fok), 1 | (2 << 9));
        let mut po = base.clone();
        po.post_only = true;
        assert_eq!(nado_appendix(&po), 1 | (3 << 9));
        let mut ro = base.clone();
        ro.reduce_only = true;
        assert_eq!(nado_appendix(&ro), 1 | (1 << 11));
        let mut mkt = base.clone();
        mkt.order_type = OrderType::Market;
        assert_eq!(
            nado_appendix(&mkt),
            1 | (1 << 9),
            "market = IOC on the wire"
        );
    }

    #[test]
    fn nado_order_digest_is_deterministic_and_domain_bound() {
        let sender = [0x11u8; 32];
        let d1 = nado_order_digest(
            &sender,
            2,
            I256::from_str("86000000000000000000000").unwrap(),
            I256::from_str("10000000000000000").unwrap(),
            4294967295,
            1757062078359666688,
            1,
        );
        let d2 = nado_order_digest(
            &sender,
            2,
            I256::from_str("86000000000000000000000").unwrap(),
            I256::from_str("10000000000000000").unwrap(),
            4294967295,
            1757062078359666688,
            1,
        );
        assert_eq!(d1, d2);
        // A different product_id changes the verifying contract → digest.
        let d3 = nado_order_digest(
            &sender,
            4,
            I256::from_str("86000000000000000000000").unwrap(),
            I256::from_str("10000000000000000").unwrap(),
            4294967295,
            1757062078359666688,
            1,
        );
        assert_ne!(d1, d3);
        // Cancellation digest over the endpoint domain differs too.
        let cd = nado_cancellation_digest(&sender, &[2], &[d1], 1);
        assert_ne!(cd, d1);
    }

    #[test]
    fn nado_credential_gate_and_fail_closed_paths() {
        let nado = DexPerpAdapter::nado(None);
        assert!(nado.supports_live_execution());
        assert!(!nado.has_live_credentials());
        // Key without the funded address is not live.
        nado.update_credentials(None, None, None, None, None, Some(format!("{:064x}", 1)))
            .unwrap();
        assert!(!nado.has_live_credentials());
        nado.update_credentials(
            None,
            None,
            None,
            None,
            Some("0x7a5ec2748e9065794491a8d29dcf3f9edb8d7c43".into()),
            Some(format!("{:064x}", 1)),
        )
        .unwrap();
        assert!(nado.has_live_credentials());
    }

    #[tokio::test]
    async fn nado_order_paths_fail_closed_without_credentials() {
        let nado = DexPerpAdapter::nado(None);
        let req = OrderRequest {
            client_order_id: "t".into(),
            venue: nado.venue_id(),
            symbol: "BTC-USDT0".into(),
            side: arbitrageur_core::types::Side::Buy,
            order_type: arbitrageur_core::types::OrderType::Limit,
            price: Some(dec!(86000)),
            quantity: dec!(0.01),
            time_in_force: arbitrageur_core::types::TimeInForce::GoodTilCancelled,
            post_only: false,
            reduce_only: false,
            created_at: Utc::now(),
        };
        // No walletAddress → sender cannot be constructed → bail before
        // any network call or book read.
        let err = nado.place_order(req).await.unwrap_err().to_string();
        assert!(err.contains("walletAddress"), "unexpected error: {err}");
        assert!(nado.fetch_account().await.is_err());
    }

    /// Live smoke against api.prod.nado.xyz — run explicitly with
    /// `--ignored`. Validates the gateway's compression gate, symbols
    /// query shape, v2 orderbook, archive tickers, and funding.
    #[tokio::test]
    #[ignore]
    async fn nado_live_market_smoke() {
        let nado = DexPerpAdapter::nado(None);
        let markets = nado.fetch_markets().await.expect("markets");
        assert!(markets.len() > 50, "expected 80+ live perps");
        assert!(markets.iter().any(|m| m.venue_symbol == "BTC-PERP"));
        let book = nado.fetch_orderbook("BTC-USDT0").await.expect("book");
        assert!(!book.bids.is_empty() && !book.asks.is_empty());
        let fr = nado.fetch_funding_rate("BTC-USDT0").await.expect("funding");
        assert_eq!(fr.interval_hours, 1);
        let vols = nado.fetch_volumes_24h().await.expect("volumes");
        assert!(vols.get("BTC").copied().unwrap_or_default() > dec!(0));
    }

    // ---- EVEDEX -------------------------------------------------------

    /// Live-shape sample from `GET /api/market/instrument?fields=metrics`
    /// (exchange-api.evedex.com, probed 2026-09-23).
    #[test]
    fn evedex_market_parser_matches_live_metrics_shape() {
        let data = serde_json::json!([
            {
                "id": "btc-usd", "name": "BTCUSD", "displayName": "BTCUSD",
                "type": "perpetual-futures", "trading": "all",
                "marketState": "OPEN", "visibility": "all",
                "from": {"symbol": "BTC"}, "to": {"symbol": "USD"},
                "priceIncrement": 0.1, "quantityIncrement": 0.001,
                "minVolume": 5, "minQuantity": 0.001, "maxLeverage": 200,
                "fundingRate": 0.00027596, "markPrice": 85847.38,
            },
            {
                "id": "closed-thing", "name": "ZZZUSD",
                "type": "perpetual-futures", "trading": "all",
                "marketState": "CLOSED", "visibility": "all",
                "from": {"symbol": "ZZZ"}, "to": {"symbol": "USD"},
                "priceIncrement": 0.1, "quantityIncrement": 0.1,
            },
            {
                // Stocks/forex perps are admitted — same contract type,
                // different asset class.
                "id": "aapl-usd", "name": "AAPLUSD",
                "type": "stocks", "trading": "all", "marketState": "OPEN",
                "visibility": "all",
                "from": {"symbol": "AAPL"}, "to": {"symbol": "USD"},
                "priceIncrement": 0.01, "quantityIncrement": 0.1,
                "minVolume": 5,
            },
            {
                // Market-maker-only instruments stay hidden.
                "id": "mm-only", "name": "MMUSD",
                "type": "perpetual-futures", "trading": "marketMakerOnly",
                "marketState": "OPEN", "visibility": "none",
                "from": {"symbol": "M"}, "to": {"symbol": "USD"},
                "priceIncrement": 0.1, "quantityIncrement": 0.1,
            },
        ]);
        let venue = VenueId::new("evedex");
        let markets = parse_evedex_markets(&venue, &data, dec!(4.5));
        assert_eq!(
            markets.len(),
            2,
            "only open, all-visible perps are admitted"
        );
        let m = &markets[0];
        assert_eq!(m.venue_symbol, "BTCUSD");
        assert_eq!(m.symbol.to_string(), "BTC-USD:PERP");
        assert_eq!(m.min_notional, dec!(5));
        assert_eq!(m.maker_fee_bps, dec!(1.5));
        assert_eq!(m.taker_fee_bps, dec!(4.5));
    }

    #[test]
    fn evedex_book_parser_reads_quantity_levels() {
        let data = serde_json::json!({
            "t": 1790156276883i64,
            "bids": [{"price": 85880.2, "quantity": 1.5}],
            "asks": [{"price": 85890.1, "quantity": 3.126}],
        });
        let book = parse_evedex_book(&VenueId::new("evedex"), "BTC-USD", &data).expect("book");
        assert_eq!(book.best_bid_price(), Some(dec!(85880.2)));
        assert_eq!(book.best_ask_price(), Some(dec!(85890.1)));
        // Both sides empty → fail closed, never an empty book.
        let empty = serde_json::json!({"t": 1, "bids": [], "asks": []});
        assert!(parse_evedex_book(&VenueId::new("evedex"), "BTC-USD", &empty).is_err());
    }

    #[test]
    fn evedex_order_id_matches_v2_shape() {
        let id = evedex_order_id(None);
        let (days, suffix) = id.split_once(':').expect("day:suffix");
        assert_eq!(days.len(), 5);
        assert_eq!(suffix.len(), 26);
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
        // Day counter is days since 2025-07-24 (≈426 on 2026-09-23).
        let d: i64 = days.parse().unwrap();
        assert!(d >= 426, "day counter behind the SDK epoch: {d}");
    }

    #[test]
    fn evedex_order_id_is_deterministic_per_client_id() {
        // Retry after a lost ack must regenerate the same venue id so the
        // matcher dedupes instead of doubling the order.
        let a = evedex_order_id(Some("sig-abc-leg0"));
        let b = evedex_order_id(Some("sig-abc-leg0"));
        assert_eq!(a, b);
        assert_ne!(
            evedex_order_id(Some("sig-abc-leg0")),
            evedex_order_id(Some("sig-abc-leg1"))
        );
        assert_ne!(evedex_order_id(None), evedex_order_id(None));
        assert_ne!(evedex_order_id(Some("x")), evedex_order_id(Some(" ")));
    }

    #[test]
    fn evedex_matcher_num_is_8dp_half_up() {
        assert_eq!(evedex_matcher_num(dec!(0.123456789)), "0.12345679");
        assert_eq!(evedex_matcher_num(dec!(86000)), "86000");
        assert_eq!(evedex_matcher_num(dec!(0.000000015)), "0.00000002");
        assert_eq!(evedex_matcher_num(dec!(1.5)), "1.5");
        // And the signed uint is the ×1e8 integer of the rounded value.
        let u = evedex_uint(Decimal::from_str("0.12345679").unwrap(), "quantity").unwrap();
        assert_eq!(u.to_string(), "12345679");
    }

    #[test]
    fn evedex_siwe_message_matches_sdk_wire_format() {
        let msg = evedex_siwe_message(
            "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf",
            "I58UsQpUW9NJEokEG",
            "2026-09-23T09:30:00.982Z",
        );
        let expected = "https://evedex.com wants you to sign in with your Ethereum account:\n\
            0x7e5f4552091a69125d5dfcb7b8c2659029395bdf\n\n\
            Sign in to evedex.com\n\n\
            URI: https://evedex.com\n\
            Version: 1\n\
            Chain ID: 42161\n\
            Nonce: I58UsQpUW9NJEokEG\n\
            Issued At: 2026-09-23T09:30:00.982Z";
        assert_eq!(msg, expected);
    }

    #[test]
    fn evedex_digests_are_deterministic_and_domain_bound() {
        let q = U256::from(123456u64);
        let p = U256::from(860000000000u64);
        let d1 = evedex_limit_digest("00001:abcdef", "BTCUSD", "BUY", 1, q, p);
        let d2 = evedex_limit_digest("00001:abcdef", "BTCUSD", "BUY", 1, q, p);
        assert_eq!(d1, d2);
        // Side, price, and id all bind into the digest.
        assert_ne!(
            d1,
            evedex_limit_digest("00001:abcdef", "BTCUSD", "SELL", 1, q, p)
        );
        assert_ne!(
            d1,
            evedex_limit_digest("00001:abcdef", "BTCUSD", "BUY", 1, q, p + U256::from(1))
        );
        assert_ne!(
            d1,
            evedex_limit_digest("00002:abcdef", "BTCUSD", "BUY", 1, q, p)
        );
        // Market schema digests differ from limit for the same fields.
        let m = evedex_market_digest("00001:abcdef", "BTCUSD", "BUY", "IOC", 1, q);
        assert_ne!(m, d1);
    }

    #[test]
    fn evedex_status_map_covers_every_documented_state() {
        use arbitrageur_core::types::OrderStatus::*;
        assert_eq!(evedex_status("INTENTION"), Some(New));
        assert_eq!(evedex_status("NEW"), Some(New));
        assert_eq!(evedex_status("PARTIALLY_FILLED"), Some(PartiallyFilled));
        assert_eq!(evedex_status("FILLED"), Some(Filled));
        assert_eq!(evedex_status("CANCELLED"), Some(Cancelled));
        assert_eq!(evedex_status("REPLACED"), Some(Cancelled));
        assert_eq!(evedex_status("REJECTED"), Some(Rejected));
        assert_eq!(evedex_status("ERROR"), Some(Rejected));
        assert_eq!(evedex_status("EXPIRED"), Some(Expired));
        // Unknown strings fail closed — never guessed.
        assert_eq!(evedex_status("SOMETHING_NEW"), None);
    }

    #[test]
    fn evedex_credential_gate_requires_matching_wallet() {
        let ev = DexPerpAdapter::evedex(None);
        assert!(ev.supports_live_execution());
        assert!(!ev.has_live_credentials());
        // Key without the matching account address is not live.
        ev.update_credentials(None, None, None, None, None, Some(format!("{:064x}", 1)))
            .unwrap();
        assert!(!ev.has_live_credentials());
        // Mismatched address fails closed (SIWE would recover elsewhere).
        ev.update_credentials(
            None,
            None,
            None,
            None,
            Some("0x0000000000000000000000000000000000000001".into()),
            Some(format!("{:064x}", 1)),
        )
        .unwrap();
        assert!(!ev.has_live_credentials());
        // Scalar-1 key derives 0x7e5f…5bdf — that pairing is live.
        ev.update_credentials(
            None,
            None,
            None,
            None,
            Some("0x7e5f4552091a69125d5dfcb7b8c2659029395bdf".into()),
            Some(format!("{:064x}", 1)),
        )
        .unwrap();
        assert!(ev.has_live_credentials());
    }

    #[tokio::test]
    async fn evedex_order_paths_fail_closed_without_credentials() {
        let ev = DexPerpAdapter::evedex(None);
        let req = OrderRequest {
            client_order_id: "t".into(),
            venue: ev.venue_id(),
            symbol: "BTC-USD".into(),
            side: arbitrageur_core::types::Side::Buy,
            order_type: arbitrageur_core::types::OrderType::Limit,
            price: Some(dec!(86000)),
            quantity: dec!(0.01),
            time_in_force: arbitrageur_core::types::TimeInForce::GoodTilCancelled,
            post_only: false,
            reduce_only: false,
            created_at: Utc::now(),
        };
        // No signing key → bail before any network call.
        let err = ev.place_order(req).await.unwrap_err().to_string();
        assert!(
            err.contains("privateKey") || err.contains("signing key"),
            "unexpected error: {err}"
        );
        assert!(ev.fetch_account().await.is_err());
    }

    /// Live smoke against exchange-api.evedex.com — run explicitly with
    /// `--ignored`. Validates the metrics listing, depth book, and
    /// funding rate against real payloads.
    #[tokio::test]
    #[ignore]
    async fn evedex_live_market_smoke() {
        let ev = DexPerpAdapter::evedex(None);
        let markets = ev.fetch_markets().await.expect("markets");
        assert!(markets.len() > 40, "expected 50+ live perps");
        assert!(markets.iter().any(|m| m.venue_symbol == "BTCUSD"));
        let book = ev.fetch_orderbook("BTC-USD").await.expect("book");
        assert!(!book.bids.is_empty() && !book.asks.is_empty());
        let fr = ev.fetch_funding_rate("BTC-USD").await.expect("funding");
        assert_eq!(fr.interval_hours, 1);
        let vols = ev.fetch_volumes_24h().await.expect("volumes");
        assert!(vols.get("BTC").copied().unwrap_or_default() > dec!(0));
    }

    // ---- Paradex (Stark L2 via paradex-rs) ----------------------------------

    #[test]
    fn paradex_l2_key_matches_official_derivation_vector() {
        // Vector from paradex-rs' own test: this EVM key must derive the
        // exact Stark L2 private key — proves our credential path feeds the
        // signer the venue expects.
        let key = "0x58d27b1d66da0dee9193105c848855b43eeceb14844f2b1de00cdcb1bdce3643";
        let signer = alloy_signer_local::PrivateKeySigner::from_str(key).unwrap();
        let felt = paradex::onboarding::get_paradex_private_key(&signer);
        assert_eq!(
            felt.to_hex_string(),
            "0x549aa9cb8328a12b1394f99f9430ba2dbc2b5c26b8a4c3b9d2b3ca3765669b2"
        );
    }

    #[test]
    fn paradex_credentials_require_matching_wallet_and_derive_l2() {
        let px = DexPerpAdapter::paradex(None);
        assert!(!px.has_live_credentials());
        let key = "0x58d27b1d66da0dee9193105c848855b43eeceb14844f2b1de00cdcb1bdce3643";
        let addr = EvmWalletSigner::from_hex(key).unwrap().address_hex();
        // Mismatched wallet address → no credentials (derived L2 account
        // would not belong to the configured address).
        px.update_credentials(
            None,
            None,
            None,
            None,
            Some("0x0000000000000000000000000000000000000001".into()),
            Some(key.into()),
        )
        .unwrap();
        assert!(px.paradex_l2_key.read().is_some());
        assert!(!px.has_live_credentials());
        // Matching wallet → credentials live.
        px.update_credentials(None, None, None, None, Some(addr), Some(key.into()))
            .unwrap();
        assert!(px.has_live_credentials());
        // Clearing drops the derived key too.
        px.update_credentials(None, None, None, None, None, None)
            .unwrap();
        assert!(px.paradex_l2_key.read().is_none());
        assert!(!px.has_live_credentials());
    }

    #[test]
    fn paradex_status_maps_closed_by_remaining_and_reason() {
        use arbitrageur_core::types::OrderStatus as S;
        use paradex::structs as ps;
        let mk = |status, remaining: &str, reason: &str| ps::OrderUpdate {
            account: String::new(),
            cancel_reason: reason.into(),
            client_id: String::new(),
            created_at: 0,
            id: "1".into(),
            instruction: ps::OrderInstruction::GTC,
            last_updated_at: 0,
            market: "BTC-USD-PERP".into(),
            price: None,
            remaining_size: Decimal::from_str(remaining).unwrap(),
            side: ps::Side::BUY,
            size: dec!(1),
            status,
            timestamp: 0,
            order_type: ps::OrderType::LIMIT,
            seq_no: 0,
            avg_fill_price: 0.0,
            received_at: 0,
            published_at: 0,
            flags: vec![],
            trigger_price: None,
        };
        use ps::OrderStatus as PS;
        assert_eq!(
            DexPerpAdapter::paradex_status(&mk(PS::NEW, "1", "")),
            S::New
        );
        assert_eq!(
            DexPerpAdapter::paradex_status(&mk(PS::OPEN, "1", "")),
            S::New
        );
        assert_eq!(
            DexPerpAdapter::paradex_status(&mk(PS::OPEN, "0.5", "")),
            S::PartiallyFilled
        );
        assert_eq!(
            DexPerpAdapter::paradex_status(&mk(PS::CLOSED, "0", "")),
            S::Filled
        );
        assert_eq!(
            DexPerpAdapter::paradex_status(&mk(PS::CLOSED, "0.5", "USER_CANCELED")),
            S::Cancelled
        );
        assert_eq!(
            DexPerpAdapter::paradex_status(&mk(PS::CLOSED, "0.5", "")),
            S::Cancelled
        );
        assert_eq!(
            DexPerpAdapter::paradex_status(&mk(PS::CLOSED, "0.5", "EXPIRED")),
            S::Expired
        );
        assert_eq!(
            DexPerpAdapter::paradex_status(&mk(PS::CLOSED, "0.5", "NOT_ENOUGH_MARGIN")),
            S::Rejected
        );
    }

    #[tokio::test]
    async fn paradex_fok_and_zero_size_fail_closed() {
        use arbitrageur_core::types::{OrderType, Side, TimeInForce};
        let px = DexPerpAdapter::paradex(None);
        let mk = |tif, ot| OrderRequest {
            client_order_id: "t".into(),
            venue: px.venue_id(),
            symbol: "BTC-USD".into(),
            side: Side::Buy,
            order_type: ot,
            price: Some(dec!(50000)),
            quantity: dec!(0.01),
            time_in_force: tif,
            post_only: false,
            reduce_only: false,
            created_at: Utc::now(),
        };
        // FOK has no Paradex instruction — must bail before any auth work.
        let err = px
            .paradex_place(&mk(TimeInForce::FillOrKill, OrderType::Limit))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("FOK"));
        // Zero size fails before touching the client.
        let mut z = mk(TimeInForce::GoodTilCancelled, OrderType::Limit);
        z.quantity = dec!(0);
        assert!(px.paradex_place(&z).await.is_err());
    }

    // ---- edgeX ----

    /// Build a minimal EdgexMeta for deterministic signing tests.
    fn edgex_test_meta() -> Arc<EdgexMeta> {
        let mut contracts = HashMap::new();
        contracts.insert(
            "BTCUSDC".to_string(),
            EdgexContractMeta {
                contract_id: "30000001".to_string(),
                resolution: dec!(10000000000),
                quote_coin_id: "1000".to_string(),
                quote_resolution: dec!(1000000),
                taker_fee_rate: dec!(0.00045),
                tick_size: dec!(0.1),
            },
        );
        let mut contracts_by_id = HashMap::new();
        contracts_by_id.insert("30000001".to_string(), "BTCUSDC".to_string());
        let mut coin_names = HashMap::new();
        coin_names.insert("1000".to_string(), "USDC".to_string());
        Arc::new(EdgexMeta {
            chain_id: 3343,
            verifying_contract: "0xeeb3fb05cca17745cbdf193a9b16537efb68fac8"
                .parse()
                .unwrap(),
            contracts,
            contracts_by_id,
            coin_names,
        })
    }

    #[test]
    fn edgex_hmac_matches_sdk_vector() {
        // Vector generated with the official edgeX python SDK's
        // build_hmac_signature (key = base64(secret), hex digest).
        let sig = edgex_hmac_sign(
            "test-secret",
            "1737254372359",
            "GET",
            "/api/v2/private/order/getOrderById",
            "accountId=123456&orderIdList=998877",
        );
        assert_eq!(
            sig,
            "e48b783bfa509a4eb656f472391baf4fdb0341b4a4695d2519edd7672bed7acb"
        );
    }

    #[test]
    fn edgex_canonical_matches_sdk_get_value() {
        let body = serde_json::json!({
            "accountId": "123456",
            "orderIdList": ["111", "222"],
            "reduceOnly": true,
            "price": "0.5",
            "x": null,
        });
        assert_eq!(
            edgex_canonical(&body),
            "accountId=123456&orderIdList=111&222&price=0.5&reduceOnly=true&x="
        );
    }

    #[test]
    fn edgex_nonce_is_sha256_top_bits() {
        // sha256("test-order-42")[..8 hex] — matches the SDK CalcNonce.
        assert_eq!(edgex_nonce("test-order-42"), 4118277392);
    }

    #[test]
    fn edgex_order_signature_matches_sdk_vector() {
        // Full EIP-712 `LimitOrderParams` signature vector produced by the
        // official SDK for the same inputs — pins domain, type schema,
        // nested OrderBase hashing, and field ordering.
        let key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let wallet = EvmWalletSigner::from_hex(key).unwrap();
        let signer = wallet.address_bytes();
        let meta = edgex_test_meta();
        let nonce = edgex_nonce("test-order-42"); // 4118277392
        let digest = edgex_order_digest(
            &meta,
            nonce,
            Address::from(signer),
            123456,
            1737254372,
            I256::from_str("10000000").unwrap(),
            I256::from_str("97393800").unwrap(),
            U256::from(1000000u64),
            30000001,
            1000,
            true,
        );
        let sig = wallet.sign_digest(&digest).unwrap();
        assert_eq!(
            format!("0x{}", hex::encode(sig)),
            "0x6959cb328ed9fc855cd9e60cfa84739428457017da480865a73e75ba13e0f002071b1f93ce18eea13439f28a69d0236d2b9543dab07c9478952749f1a9e508d41b"
        );
    }

    #[test]
    fn edgex_resolution_parses_hex_and_decimal() {
        assert_eq!(
            edgex_resolution(Some(&serde_json::json!("0xf4240"))),
            Some(dec!(1000000))
        );
        assert_eq!(
            edgex_resolution(Some(&serde_json::json!("10000000000"))),
            Some(dec!(10000000000))
        );
        assert_eq!(edgex_resolution(Some(&serde_json::json!(""))), None);
        assert_eq!(edgex_resolution(None), None);
    }

    #[test]
    fn edgex_credentials_require_full_trio_and_account() {
        let e = DexPerpAdapter::edgex(None);
        assert!(!e.has_live_credentials());
        // Missing passphrase — must stay false.
        assert!(e
            .update_credentials(
                Some("k".into()),
                Some("s".into()),
                None,
                Some("123".into()),
                None,
                Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into()),
            )
            .is_ok());
        assert!(!e.has_live_credentials());
        // Full trio + account id + trading key → live.
        assert!(e
            .update_credentials(
                Some("k".into()),
                Some("s".into()),
                Some("p".into()),
                Some("123".into()),
                None,
                Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into()),
            )
            .is_ok());
        assert!(e.has_live_credentials());
        assert!(e.supports_live_execution());
        assert!(e.live_order_submission_implemented());
        assert!(e.order_status_polling_implemented());
    }

    #[tokio::test]
    #[ignore = "live public read — run manually"]
    async fn edgex_live_public_smoke() {
        let a = DexPerpAdapter::edgex(None);
        let markets = a.fetch_markets().await.expect("markets");
        assert!(!markets.is_empty(), "edgeX returned no markets");
        let btc = markets
            .iter()
            .find(|m| m.venue_symbol == "BTCUSDC")
            .expect("BTCUSDC missing");
        let book = a.fetch_orderbook(&btc.symbol.raw).await.expect("book");
        assert!(!book.bids.is_empty() && !book.asks.is_empty());
        let funding = a
            .fetch_funding_rate(&btc.symbol.raw)
            .await
            .expect("funding");
        assert!(funding.rate != dec!(0) || funding.interval_hours > 0);
    }

    // ---------------------------------------------------------------
    // Extended — SNIP-12 Poseidon order signing
    // ---------------------------------------------------------------
    //
    // Vectors lifted verbatim from the official X10 `fast_stark_crypto`
    // sdist (`rust-crypto-lib-base/src/starknet_messages.rs` tests) — a
    // byte-for-byte match proves the port is faithful.

    #[test]
    fn extended_order_struct_hash_matches_x10_vector() {
        use starknet_crypto::Felt;
        let h = extended_order_hash(
            1,
            Felt::from_dec_str("2").unwrap(),
            3,
            Felt::from_dec_str("4").unwrap(),
            5,
            Felt::from_dec_str("6").unwrap(),
            7,
            8,
            Felt::from_dec_str("9").unwrap(),
        );
        assert_eq!(
            h,
            Felt::from_dec_str(
                "1329353150252109345267997901008558234696410103652961347079636617692652241760"
            )
            .unwrap()
        );
    }

    #[test]
    fn extended_domain_hash_matches_x10_vector() {
        use starknet_crypto::Felt;
        let h = extended_domain_hash("SN_SEPOLIA");
        assert_eq!(
            h,
            Felt::from_dec_str(
                "2788850828067604540663615870177667078542240404906059806659101905868929188327"
            )
            .unwrap()
        );
    }

    #[test]
    fn extended_message_hash_matches_x10_vector() {
        use starknet_crypto::Felt;
        let order_hash = extended_order_hash(
            1,
            Felt::from_dec_str("2").unwrap(),
            3,
            Felt::from_dec_str("4").unwrap(),
            5,
            Felt::from_dec_str("6").unwrap(),
            7,
            8,
            Felt::from_dec_str("9").unwrap(),
        );
        let user_key = Felt::from_dec_str(
            "1528491859474308181214583355362479091084733880193869257167008343298409336538",
        )
        .unwrap();
        let msg = extended_message_hash(order_hash, user_key, "SN_SEPOLIA");
        assert_eq!(
            msg,
            Felt::from_dec_str(
                "2788960362996410178586013462192086205585543858281504820767681025777602529597"
            )
            .unwrap()
        );
    }

    #[test]
    fn extended_sign_produces_verifiable_signature() {
        use starknet_crypto::Felt;
        // Known pair: privkey 0x1 → pubkey get_public_key(1).
        let privk = Felt::ONE;
        let pubkey = starknet_crypto::get_public_key(&privk);
        let msg = extended_message_hash(
            extended_order_hash(
                7,
                Felt::from_dec_str("42").unwrap(),
                100,
                Felt::from_dec_str("43").unwrap(),
                -200,
                Felt::from_dec_str("43").unwrap(),
                1,
                1_800_000_000,
                Felt::from_dec_str("99").unwrap(),
            ),
            pubkey,
            "SN_MAIN",
        );
        let (r, s) = extended_sign(msg, privk).expect("sign");
        assert!(starknet_crypto::verify(&pubkey, &msg, &r, &s).expect("verify"));
    }

    #[test]
    fn extended_stark_amount_rounds_like_sdk() {
        // Buy rounds up (away from zero), sell rounds down, fee ceils.
        assert_eq!(
            extended_stark_amount(dec!(1.001), dec!(1000), true).unwrap(),
            1001
        );
        assert_eq!(
            extended_stark_amount(dec!(1.0019), dec!(1000), true).unwrap(),
            1002
        );
        assert_eq!(
            extended_stark_amount(dec!(1.0019), dec!(1000), false).unwrap(),
            1001
        );
        assert!(extended_stark_amount(dec!(1e20), dec!(1000), true).is_err());
    }

    #[test]
    fn extended_credentials_require_api_key_vault_and_stark_pair() {
        let e = DexPerpAdapter::extended(None);
        assert!(!e.has_live_credentials());
        // api key + vault only — no Stark pair.
        assert!(e
            .update_credentials(Some("k".into()), None, None, Some("7".into()), None, None,)
            .is_ok());
        assert!(!e.has_live_credentials());
        // Stark keypair — pubkey must be derived from the private key.
        let privk = starknet_crypto::Felt::from(0x12345u64);
        let pubkey = starknet_crypto::get_public_key(&privk).to_hex_string();
        // Mismatched pair fails closed at credential load.
        assert!(e
            .update_credentials(
                Some("k".into()),
                None,
                None,
                Some("7".into()),
                Some("0x1".into()),
                Some(privk.to_hex_string()),
            )
            .is_err());
        assert!(e
            .update_credentials(
                Some("k".into()),
                None,
                None,
                Some("7".into()),
                Some(pubkey),
                Some(privk.to_hex_string()),
            )
            .is_ok());
        assert!(e.has_live_credentials());
        assert!(e.supports_live_execution());
        assert!(e.order_status_polling_implemented());
    }

    #[tokio::test]
    #[ignore = "live public read — run manually"]
    async fn extended_live_public_smoke() {
        let a = DexPerpAdapter::extended(None);
        let markets = a.fetch_markets().await.expect("markets");
        assert!(!markets.is_empty(), "Extended returned no markets");
        let btc = markets
            .iter()
            .find(|m| m.venue_symbol == "BTC-USD")
            .expect("BTC-USD missing");
        let book = a.fetch_orderbook(&btc.symbol.raw).await.expect("book");
        assert!(!book.bids.is_empty() && !book.asks.is_empty());
        // L2 config resolves for signing.
        let (_, l2) = a.extended_market_l2(&btc.symbol.raw).await.expect("l2");
        assert!(l2.collateral_resolution >= dec!(1));
    }

    // ------------------------------------------------------------------
    // dYdX
    // ------------------------------------------------------------------

    /// BIP-39 test mnemonic ("abandon"×23 + "art") — the all-zero
    /// 256-bit entropy vector; bip32 accepts only the 24-word length.
    const DYDX_TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon \
         abandon abandon abandon abandon abandon abandon abandon abandon abandon \
         abandon abandon abandon abandon abandon abandon abandon abandon art";

    #[test]
    fn dydx_credentials_require_mnemonic() {
        let a = DexPerpAdapter::dydx(None);
        // No credentials → the credential gate stays closed (the venue
        // itself is live-capable — capability ≠ credentials).
        assert!(!a.has_live_credentials());
        // A raw hex key is rejected outright (the venue needs a mnemonic).
        assert!(a
            .update_credentials(
                None,
                None,
                None,
                None,
                None,
                Some(
                    "0x0000000000000000000000000000000000000000000000000000000000000001"
                        .to_string()
                ),
            )
            .is_err());
        assert!(a.dydx_mnemonic.read().is_none());
        // A valid BIP-39 mnemonic binds and enables execution.
        a.update_credentials(
            None,
            None,
            None,
            None,
            None,
            Some(DYDX_TEST_MNEMONIC.to_string()),
        )
        .expect("mnemonic binds");
        assert!(a.dydx_mnemonic.read().is_some());
        assert!(a.has_live_credentials());
        assert!(a.supports_live_execution());
    }

    #[test]
    fn dydx_wallet_address_must_match_mnemonic() {
        let a = DexPerpAdapter::dydx(None);
        let derived = dydx::node::Wallet::from_mnemonic(DYDX_TEST_MNEMONIC)
            .unwrap()
            .account_offline(0)
            .unwrap()
            .address()
            .to_string();
        assert!(derived.starts_with("dydx1"));
        // Matching address binds fine.
        a.update_credentials(
            None,
            None,
            None,
            None,
            Some(derived.clone()),
            Some(DYDX_TEST_MNEMONIC.to_string()),
        )
        .expect("matching address binds");
        // A different dydx1 address fails the whole update.
        let other = format!("{}x", &derived[..derived.len() - 1]);
        assert!(a
            .update_credentials(
                None,
                None,
                None,
                None,
                Some(other),
                Some(DYDX_TEST_MNEMONIC.to_string()),
            )
            .is_err());
        // An EVM address is rejected at format validation.
        assert!(a
            .update_credentials(
                None,
                None,
                None,
                None,
                Some("0x7E5F4552091a69125d5dfcb7b8c2659029395bdf".to_string()),
                Some(DYDX_TEST_MNEMONIC.to_string()),
            )
            .is_err());
    }

    #[test]
    fn dydx_client_id_is_deterministic_u32() {
        let a = dydx_client_id("arb-BTC-1");
        let b = dydx_client_id("arb-BTC-1");
        let c = dydx_client_id("arb-BTC-2");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn dydx_order_id_hex_round_trips_through_protobuf() {
        use dydx::node::OrderId;
        use prost::Message;
        let oid = OrderId {
            subaccount_id: None,
            client_id: 12345,
            order_flags: 64,
            clob_pair_id: 0,
        };
        let hex_id = hex::encode(oid.encode_to_vec());
        let back = OrderId::decode(hex::decode(&hex_id).unwrap().as_slice()).unwrap();
        assert_eq!(back.client_id, 12345);
        assert_eq!(back.clob_pair_id, 0);
        assert_eq!(back.order_flags, 64);
    }

    #[test]
    fn dydx_decimal_conversion_preserves_value() {
        let v = dydx_bd(dec!(0.001)).unwrap();
        assert_eq!(v.to_string(), "0.001");
        let p = dydx_bd(dec!(64250.5)).unwrap();
        assert_eq!(p.to_string(), "64250.5");
    }

    #[tokio::test]
    #[ignore = "live public read — run manually"]
    async fn dydx_live_public_smoke() {
        let a = DexPerpAdapter::dydx(None);
        let markets = a.fetch_markets().await.expect("markets");
        assert!(!markets.is_empty(), "dYdX returned no markets");
        // Order params resolve for a real market (clobPairId etc).
        let (vsym, p) = a.dydx_market_params("BTC").await.expect("params");
        assert_eq!(vsym, "BTC-USD");
        assert!(p.step_base_quantums > 0);
        assert!(p.subticks_per_tick > 0);
        let book = a.fetch_orderbook("BTC").await.expect("book");
        assert!(!book.bids.is_empty() && !book.asks.is_empty());
    }

    // ------------------------------------------------------------------
    // Helix / Injective — identity derivation, wire formatting, gating
    // ------------------------------------------------------------------

    /// secp256k1 key 0x01 → 0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf.
    const HELIX_TEST_KEY: &str =
        "0x0000000000000000000000000000000000000000000000000000000000000001";
    const HELIX_TEST_ADDR: &str = "0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf";

    #[test]
    fn helix_identity_derives_bech32_and_subaccount() {
        let a = DexPerpAdapter::helix(None);
        a.update_credentials(
            None,
            None,
            None,
            None,
            None,
            Some(HELIX_TEST_KEY.to_string()),
        )
        .unwrap();
        let (inj, sub) = a.helix_identity().unwrap();
        assert!(inj.starts_with("inj1"), "expected inj1 bech32, got {inj}");
        // bytes32: eth addr || 11 zero bytes || nonce 0x00.
        assert_eq!(
            sub,
            "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf000000000000000000000000"
        );
        // Nonce selects the subaccount byte.
        a.update_credentials(
            None,
            None,
            None,
            Some("7".to_string()),
            Some(HELIX_TEST_ADDR.to_string()),
            Some(HELIX_TEST_KEY.to_string()),
        )
        .unwrap();
        let (inj2, sub2) = a.helix_identity().unwrap();
        assert_eq!(inj, inj2);
        assert_eq!(
            sub2,
            "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf000000000000000000000007"
        );
    }

    #[test]
    fn helix_identity_rejects_mismatched_wallet_and_bad_nonce() {
        let a = DexPerpAdapter::helix(None);
        a.update_credentials(
            None,
            None,
            None,
            None,
            Some("0x00000000000000000000000000000000000000ff".to_string()),
            Some(HELIX_TEST_KEY.to_string()),
        )
        .unwrap();
        // Key loads fine; identity fails at use time (address mismatch).
        assert!(a.helix_identity().is_err());
        assert!(!a.has_live_credentials());
        // Nonce > 255 is not a valid subaccount selector.
        a.update_credentials(
            None,
            None,
            None,
            Some("300".to_string()),
            Some(HELIX_TEST_ADDR.to_string()),
            Some(HELIX_TEST_KEY.to_string()),
        )
        .unwrap();
        assert!(a.helix_identity().is_err());
        // Correct pairing is live-ready.
        a.update_credentials(
            None,
            None,
            None,
            None,
            Some(HELIX_TEST_ADDR.to_string()),
            Some(HELIX_TEST_KEY.to_string()),
        )
        .unwrap();
        assert!(a.has_live_credentials());
    }

    #[test]
    fn helix_dec_and_state_wire_formats() {
        // sdk.Dec: integral values carry no trailing zeros, ≤18dp kept.
        assert_eq!(helix_dec(dec!(64250.5)), "64250.5");
        assert_eq!(helix_dec(dec!(100000)), "100000");
        assert_eq!(helix_dec(dec!(0.000001)), "0.000001");
        // Indexer states normalize to map_order_status vocabulary.
        assert_eq!(helix_state("booked"), "open");
        assert_eq!(helix_state("partial_filled"), "partially_filled");
        assert_eq!(helix_state("canceled"), "cancelled");
        assert_eq!(helix_state("filled"), "filled");
    }

    #[test]
    fn helix_market_info_keeps_margin_ratio() {
        let data = serde_json::json!({
            "markets": [{"market": {
                "ticker": "BTC/USDC PERP", "market_id": "0xabc",
                "status": "Active", "isPerpetual": true,
                "quote_decimals": 6,
                "min_price_tick_size": "1000000.000000000000000000",
                "min_quantity_tick_size": "0.0001",
                "taker_fee_rate": "0.0005",
                "initial_margin_ratio": "0.05"
            }}]
        });
        let infos = helix_market_infos(&data);
        assert_eq!(infos.len(), 1);
        let i = &infos[0];
        assert_eq!(i.market_id, "0xabc");
        assert_eq!(i.scale, dec!(1000000));
        assert_eq!(i.quote_decimals, 6);
        assert_eq!(i.initial_margin_ratio, dec!(0.05));
        assert_eq!(i.price_tick, dec!(1)); // 1e6 raw / 1e6 scale = $1.00
    }

    #[tokio::test]
    async fn helix_fail_closed_without_credentials() {
        let a = DexPerpAdapter::helix(None);
        assert!(a.supports_live_execution());
        assert!(!a.has_live_credentials());
        let req = OrderRequest {
            client_order_id: "t".to_string(),
            venue: VenueId::new("helix"),
            symbol: "BTC-USD:PERP".to_string(),
            side: arbitrageur_core::types::Side::Buy,
            order_type: arbitrageur_core::types::OrderType::Limit,
            price: Some(dec!(1)),
            quantity: dec!(1),
            time_in_force: arbitrageur_core::types::TimeInForce::GoodTilCancelled,
            post_only: false,
            reduce_only: false,
            created_at: Utc::now(),
        };
        assert!(a.place_order(req).await.is_err());
        assert!(a.fetch_order("BTC", "0xabc", None).await.is_err());
        assert!(a.fetch_account().await.is_err());
        // FOK is not expressible on Injective — must bail even with creds.
        a.update_credentials(
            None,
            None,
            None,
            None,
            None,
            Some(HELIX_TEST_KEY.to_string()),
        )
        .unwrap();
        let req = OrderRequest {
            client_order_id: "t".to_string(),
            venue: VenueId::new("helix"),
            symbol: "BTC-USD:PERP".to_string(),
            side: arbitrageur_core::types::Side::Buy,
            order_type: arbitrageur_core::types::OrderType::Limit,
            price: Some(dec!(1)),
            quantity: dec!(1),
            time_in_force: arbitrageur_core::types::TimeInForce::FillOrKill,
            post_only: false,
            reduce_only: false,
            created_at: Utc::now(),
        };
        let err = a.place_order(req).await.unwrap_err().to_string();
        assert!(err.contains("fill-or-kill"), "unexpected error: {err}");
    }

    #[tokio::test]
    #[ignore = "live public read — run manually"]
    async fn helix_live_public_smoke() {
        let a = DexPerpAdapter::helix(None);
        let markets = a.fetch_markets().await.expect("markets");
        assert!(!markets.is_empty(), "Helix returned no markets");
        let info = a.helix_resolve("BTC").await.expect("resolve");
        assert_eq!(info.quote_decimals, 6, "BTC/USDC PERP quote decimals");
        assert!(info.initial_margin_ratio > dec!(0));
        assert!(info.price_tick > dec!(0) && info.size_tick > dec!(0));
        let book = a.fetch_orderbook("BTC").await.expect("book");
        assert!(!book.bids.is_empty() && !book.asks.is_empty());
    }
}

#[cfg(test)]
mod wallet_direct_tests {
    use super::*;

    #[test]
    fn wallet_binding_prepares_evm_signer() {
        let a = DexPerpAdapter::apex(None);
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
        // ApeX stays market-data-only — bound keys never enable execution.
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

    #[test]
    fn exec_policy_resolution_never_downgrades() {
        use arbitrageur_core::types::{OrderType, Side, TimeInForce};
        let venue = VenueId::new("vest");
        let mk = |t: OrderType, tif: TimeInForce, po: bool| {
            let mut o = OrderRequest::new_limit(
                venue.clone(),
                "BTC-USDC",
                Side::Buy,
                dec!(50000),
                dec!(0.01),
            );
            o.order_type = t;
            o.time_in_force = tif;
            o.post_only = po;
            o
        };
        // Every request maps to exactly one policy — the semantic field
        // wins over a contradictory tif.
        assert_eq!(
            effective_exec_policy(&mk(OrderType::Market, TimeInForce::GoodTilCancelled, false)),
            ExecPolicy::Market
        );
        assert_eq!(
            effective_exec_policy(&mk(OrderType::Limit, TimeInForce::ImmediateOrCancel, false)),
            ExecPolicy::Ioc
        );
        assert_eq!(
            effective_exec_policy(&mk(OrderType::Limit, TimeInForce::FillOrKill, false)),
            ExecPolicy::Fok
        );
        assert_eq!(
            effective_exec_policy(&mk(
                OrderType::PostOnly,
                TimeInForce::GoodTilCancelled,
                false
            )),
            ExecPolicy::PostOnly
        );
        assert_eq!(
            effective_exec_policy(&mk(OrderType::Limit, TimeInForce::PostOnly, false)),
            ExecPolicy::PostOnly
        );
        assert_eq!(
            effective_exec_policy(&mk(OrderType::Limit, TimeInForce::GoodTilCancelled, true)),
            ExecPolicy::PostOnly
        );
        assert_eq!(
            effective_exec_policy(&mk(
                OrderType::ImmediateOrCancel,
                TimeInForce::GoodTilCancelled,
                false
            )),
            ExecPolicy::Ioc
        );
        assert_eq!(
            effective_exec_policy(&mk(OrderType::Limit, TimeInForce::GoodTilCancelled, false)),
            ExecPolicy::Rest
        );
    }

    /// One request per effective policy, plus the alternative spellings
    /// (`OrderType::PostOnly`/`ImmediateOrCancel`, tif `PostOnly`) that must
    /// resolve identically — the matrix proves request→policy→wire is
    /// total: every accepted policy lands on a documented constant, every
    /// unaccepted one refuses.
    fn policy_orders() -> Vec<(&'static str, OrderRequest)> {
        use arbitrageur_core::types::{OrderType, Side, TimeInForce};
        let venue = VenueId::new("vest");
        let mk = |t: OrderType, tif: TimeInForce, po: bool| {
            let mut o = OrderRequest::new_limit(
                venue.clone(),
                "BTC-USDC",
                Side::Buy,
                dec!(50000),
                dec!(0.01),
            );
            o.order_type = t;
            o.time_in_force = tif;
            o.post_only = po;
            o
        };
        let mut market = OrderRequest::new_market(venue.clone(), "BTC-USDC", Side::Buy, dec!(0.01));
        market.order_type = OrderType::Market;
        vec![
            ("market", market),
            (
                "ioc-tif",
                mk(OrderType::Limit, TimeInForce::ImmediateOrCancel, false),
            ),
            (
                "ioc-type",
                mk(
                    OrderType::ImmediateOrCancel,
                    TimeInForce::GoodTilCancelled,
                    false,
                ),
            ),
            ("fok", mk(OrderType::Limit, TimeInForce::FillOrKill, false)),
            (
                "postonly-flag",
                mk(OrderType::Limit, TimeInForce::GoodTilCancelled, true),
            ),
            (
                "postonly-type",
                mk(OrderType::PostOnly, TimeInForce::GoodTilCancelled, false),
            ),
            (
                "postonly-tif",
                mk(OrderType::Limit, TimeInForce::PostOnly, false),
            ),
            (
                "rest",
                mk(OrderType::Limit, TimeInForce::GoodTilCancelled, false),
            ),
        ]
    }

    #[test]
    fn venue_wire_policy_matrix_maps_or_refuses_every_policy() {
        // Grouped by expected ExecPolicy for readable expectations.
        let groups: &[(&[&str], ExecPolicy)] = &[
            (&["market"], ExecPolicy::Market),
            (&["ioc-tif", "ioc-type"], ExecPolicy::Ioc),
            (&["fok"], ExecPolicy::Fok),
            (
                &["postonly-flag", "postonly-type", "postonly-tif"],
                ExecPolicy::PostOnly,
            ),
            (&["rest"], ExecPolicy::Rest),
        ];
        let orders = policy_orders();
        for (names, want) in groups {
            for name in *names {
                let order = &orders.iter().find(|(n, _)| n == name).unwrap().1;
                assert_eq!(
                    effective_exec_policy(order),
                    *want,
                    "{name} must resolve to {want:?}"
                );
            }
        }
        let get = |name: &str| orders.iter().find(|(n, _)| *n == name).unwrap().1.clone();

        // Pacifica: limit path accepts IOC/ALO/GTC; FOK and market refuse.
        assert_eq!(pacifica_limit_tif(&get("ioc-tif")).unwrap(), "IOC");
        assert_eq!(pacifica_limit_tif(&get("postonly-flag")).unwrap(), "ALO");
        assert_eq!(pacifica_limit_tif(&get("rest")).unwrap(), "GTC");
        assert!(pacifica_limit_tif(&get("fok")).is_err());
        assert!(pacifica_limit_tif(&get("market")).is_err());

        // StandX: market submits as ioc; FOK refuses.
        assert_eq!(standx_tif(&get("ioc-tif")).unwrap(), "ioc");
        assert_eq!(standx_tif(&get("postonly-type")).unwrap(), "alo");
        assert_eq!(standx_tif(&get("rest")).unwrap(), "gtc");
        assert_eq!(standx_tif(&get("market")).unwrap(), "ioc");
        assert!(standx_tif(&get("fok")).is_err());

        // Ondo: (timeInForce, postOnly) — post-only rides the flag on GTC.
        assert_eq!(ondo_limit_policy(&get("ioc-tif")).unwrap(), ("IOC", false));
        assert_eq!(
            ondo_limit_policy(&get("postonly-tif")).unwrap(),
            ("GTC", true)
        );
        assert_eq!(ondo_limit_policy(&get("rest")).unwrap(), ("GTC", false));
        assert!(ondo_limit_policy(&get("fok")).is_err());
        assert!(ondo_limit_policy(&get("market")).is_err());

        // GRVT: explicit codes for every policy — nothing refuses.
        assert_eq!(grvt_policy(&get("ioc-tif")), (3, false));
        assert_eq!(grvt_policy(&get("fok")), (4, false));
        assert_eq!(grvt_policy(&get("postonly-flag")), (2, true));
        assert_eq!(grvt_policy(&get("rest")), (1, false));
        assert_eq!(grvt_policy(&get("market")), (1, false));

        // Derive: ioc/post_only/gtc strings; FOK refuses; market→gtc.
        assert_eq!(derive_tif(&get("ioc-tif")).unwrap(), "ioc");
        assert_eq!(derive_tif(&get("postonly-type")).unwrap(), "post_only");
        assert_eq!(derive_tif(&get("rest")).unwrap(), "gtc");
        assert_eq!(derive_tif(&get("market")).unwrap(), "gtc");
        assert!(derive_tif(&get("fok")).is_err());

        // Aevo: no tif field — IOC/FOK refuse; post_only flag only.
        assert!(aevo_policy(&get("ioc-tif")).is_err());
        assert!(aevo_policy(&get("fok")).is_err());
        assert!(aevo_policy(&get("postonly-flag")).unwrap());
        assert!(!aevo_policy(&get("rest")).unwrap());
        assert!(!aevo_policy(&get("market")).unwrap());

        // Vest: MARKET native; IOC and FOK both ride FOK (strictly
        // stronger); post-only refuses.
        assert_eq!(vest_wire_type(&get("market")).unwrap(), ("MARKET", None));
        assert_eq!(
            vest_wire_type(&get("ioc-tif")).unwrap(),
            ("LIMIT", Some("FOK"))
        );
        assert_eq!(vest_wire_type(&get("fok")).unwrap(), ("LIMIT", Some("FOK")));
        assert_eq!(
            vest_wire_type(&get("rest")).unwrap(),
            ("LIMIT", Some("GTC"))
        );
        assert!(vest_wire_type(&get("postonly-flag")).is_err());

        // QFEX: every policy has a native (order_type, tif) pair.
        assert_eq!(qfex_policy(&get("postonly-tif")), ("ALO", "GTC"));
        assert_eq!(qfex_policy(&get("market")), ("MARKET", "GTC"));
        assert_eq!(qfex_policy(&get("ioc-tif")), ("LIMIT", "IOC"));
        assert_eq!(qfex_policy(&get("fok")), ("LIMIT", "FOK"));
        assert_eq!(qfex_policy(&get("rest")), ("LIMIT", "GTC"));

        // The alternative spellings resolve identically on every venue —
        // no request form may bypass the policy gate.
        for (a, b) in [
            ("ioc-tif", "ioc-type"),
            ("postonly-flag", "postonly-type"),
            ("postonly-flag", "postonly-tif"),
        ] {
            let (oa, ob) = (get(a), get(b));
            assert_eq!(
                effective_exec_policy(&oa),
                effective_exec_policy(&ob),
                "{a} vs {b}"
            );
            assert_eq!(pacifica_limit_tif(&oa).ok(), pacifica_limit_tif(&ob).ok());
            assert_eq!(standx_tif(&oa).ok(), standx_tif(&ob).ok());
            assert_eq!(grvt_policy(&oa), grvt_policy(&ob));
            assert_eq!(derive_tif(&oa).ok(), derive_tif(&ob).ok());
            assert_eq!(aevo_policy(&oa).ok(), aevo_policy(&ob).ok());
            assert_eq!(vest_wire_type(&oa).ok(), vest_wire_type(&ob).ok());
            assert_eq!(qfex_policy(&oa), qfex_policy(&ob));
        }
    }
}
