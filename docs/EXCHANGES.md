# Exchange Integrations Guide

> Living venue reference — native-API venues only. 2026-09-22: 26 long-tail
> perp venues re-admitted through the spec-driven `DexPerpAdapter`
> (`crates/arbitrageur-exchanges/src/dex_perps.rs`) — all serve market data;
> six (Pacifica, StandX, Ondo Perps, GRVT, Derive, Aevo) additionally
> implement authenticated order paths with venue-native signing, gated by
> `has_live_credentials()` until credentialed venue evidence exists.
> Also 2026-09-22/23: 11 EVM spot AMMs admitted through the spec-driven
> `EvmAmmAdapter` (`crates/arbitrageur-exchanges/src/evm_amm.rs`) — Uniswap
> V3/V2, PancakeSwap V2/V3, SushiSwap, QuickSwap, Camelot V2, Aerodrome,
> Velodrome, Ramses (legacy), Pharaoh (legacy) — all serve onchain quoter/
> factory market data; only the six standard-router venues (Uniswap V3/V2,
> PancakeSwap V2/V3, SushiSwap, QuickSwap) additionally implement local-
> wallet swap submission with receipt/two-leg-Transfer verification.
> Camelot V2 (custom referrer router ABI) and Aerodrome/Velodrome/
> Ramses/Pharaoh (no exact-output buy primitive) are market-data-only.
> (see `docs/VENUE_COVERAGE.md` §2b for the probe-verified spec table).
> §6 transfer-rails / rebalance research. Re-check fee figures on venue
> pages before moving size.

> NOTE: `docs/CAPABILITIES.md` is the authoritative runtime matrix. Sections
> below mentioning Variational / edgeX / GRVT / Ondo Perps / dYdX / Paradex /
> long-tail venues are historical research notes kept for context. Any legacy
> “Admitted” status in those notes means a parser was once registered for
> research, **not** that `get_venues` admits the venue today. Restoring one
> means adding and admitting a native-API adapter.


This guide details the integration protocols, API specifications, and signing mechanisms for each supported market venue in the **Arbitrageur** platform.

## Current runtime inventory

The runtime admits only enabled adapters in `get_venues`; capability remains endpoint-specific and is tracked in `docs/CAPABILITIES.md`:

- Hyperliquid, Lighter, Robinhood Lighter, Vanta/Orderly
- Aster, PopDEX, Binance, OKX, Coinbase, Kraken, Decibel
- Long-tail perp venues via `DexPerpAdapter` (account-enabled admission;
  market data for all, implemented signed order paths on Pacifica /
  StandX / Ondo Perps / GRVT / Derive / Aevo): dYdX, Paradex, GMX V2
  (prices only), ApeX Omni, Extended, Helix, edgeX, QFEX (markets only),
  Vest, Antarctic, EVEDEX, BULK, Ostium, SoDEX, AlphaX, Nado, IO Trader,
  Niza, Astros, Gate DEX
- EVM spot AMMs via `EvmAmmAdapter` (account-enabled admission; onchain
  quotes for all 11, local-wallet swap execution on the six standard
  routers only): Uniswap V3/V2, PancakeSwap V2/V3, SushiSwap, QuickSwap
  implement signed `swapExact*`/`exact*Single` wallet transactions;
  Camelot V2, Aerodrome, Velodrome, Ramses (legacy), Pharaoh (legacy)
  are **market-data-only** — Camelot's V2 router uses a custom referrer
  ABI and the Solidly routers expose no exact-output buy primitive, so
  buys via exact-in headroom could overfill the requested base quantity.
  Market-data-only venues load no private key (an optional wallet
  address enables balance reads). Blackhole is
  spec'd but not admitted — its legacy deployment holds no live pools.
- Arcus plus Polymarket/Kalshi remain research/scaffold surfaces outside the admitted strategy path

Submission transport is not terminal fill reconciliation. Every live-submission adapter now exposes `fetch_order` and the desktop reconciliation supervisor polls persisted working orders every 5s and re-scans them at startup; automated residual-leg resolution (opt-in) and bounded cancel retry are implemented, but without venue sandbox/canary evidence no listed venue is live-strategy-entry ready.

Account-position matching is adapter-owned. Live flattening asks the adapter to resolve the venue-native position back to the signal leg's exact symbol before sizing or verifying a reduce-only close. Examples: Binance/Aster/PopDEX `BTCUSDT`, Kraken `PF_XBTUSD`, OKX `BTC-USDT-SWAP`, Vanta `PERP_BTC_USDC`, Decibel `BTC-PERP`, and Lighter `market_id`/spot-vs-perp resolution. Unsupported, unknown, or ambiguous mappings fail closed as unresolved exposure; they are never treated as flat.

**Venue-native contract units (Binance/Aster scaled symbols):** exchangeInfo lists scaled contracts with their own base asset — `1000PEPEUSDT` has `baseAsset: "1000PEPE"`, `quantityPrecision: 0`, `LOT_SIZE stepSize: "1"` (verified live on both venues). The canonical engine symbol `1000PEPE-USDT` is therefore denominated in venue-native `1000PEPE` units: `positionAmt`, `origQty`, order `quantity`, and book level sizes are all already in those units and prices are quoted per unit. Adapters must NOT rescale by 1000 in either direction — a `PEPE-USDT` request is a different (nonexistent) market, not an alias.

---

## 1. Hyperliquid (L1 AppChain DEX)

### 1.1 Network Architecture
Hyperliquid is an optimized standalone L1 blockchain utilizing Tendermint-inspired consensus with an EVM-compatible bridge on Arbitrum.

- **Mainnet REST API:** `https://api.hyperliquid.xyz`
- **Mainnet WebSocket:** `wss://api.hyperliquid.xyz/ws`
- **Testnet REST API:** `https://api.hyperliquid-testnet.xyz`

### 1.2 Key Endpoints & Payload Formats
1. **Public Meta Information:**
   ```bash
   POST /info
   Content-Type: application/json
   {"type": "meta"}
   ```
2. **Oracle Prices, Funding Rates & Open Interest:**
   ```bash
   POST /info
   Content-Type: application/json
   {"type": "metaAndAssetCtxs"}
   ```
   *Note: Hyperliquid funding rates are continuous and calculated/settled every 1 hour.*
3. **L2 Orderbook:**
   ```bash
   POST /info
   Content-Type: application/json
   {"type": "l2Book", "coin": "BTC"}
   ```
4. **User Clearinghouse State (Positions, Equity, Balances):**
   ```bash
   POST /info
   Content-Type: application/json
   {"type": "clearinghouseState", "user": "0xYourWalletAddress"}
   ```

### 1.3 Order Signing & EIP-712
Orders are dispatched to `POST /exchange` with an action payload containing:
- `asset`: Integer asset index
- `isBuy`: Boolean
- `limitPx`: Formatted price string
- `sz`: Formatted quantity string
- `orderType`: `{"limit": {"tif": "Ioc"}}` or `{"limit": {"tif": "Gtc"}}`
- `signature`: EIP-712 typed structured data signature (`r`, `s`, `v`) from the wallet or approved API agent wallet.

---

## 2. Lighter (zk-Rollup DEX)

### 2.1 Network Architecture
Lighter operates a high-throughput orderbook DEX secured by zero-knowledge rollups on Ethereum and Arbitrum.

- **Mainnet REST API:** `https://mainnet.zklighter.elliot.ai`
- **Mainnet WebSocket:** `wss://mainnet.zklighter.elliot.ai/stream`
- **Testnet REST API:** `https://testnet.zklighter.elliot.ai`

### 2.2 Key Endpoints
1. **Orderbook Details & Market IDs:**
   ```bash
   GET /api/v1/orderBookDetails
   ```
   Maps symbols to integer `market_id` (e.g. `ETH` $\to 0$, `BTC` $\to 1$, `SOL` $\to 2$, `DOGE` $\to 3$, `1000PEPE` $\to 4$).
2. **Orderbook Depth:**
   ```bash
   GET /api/v1/orderBookOrders?market_id=1&limit=50
   ```
3. **Account Balances & Positions:**
   ```bash
   GET /api/v1/account?by=index&value=<ACCOUNT_INDEX>
   ```
4. **Funding Rates:**
   ```bash
   GET /api/v1/funding-rates
   ```
   Only rows with `exchange: "lighter"` are the venue's own print (same on the
   Robinhood Chain deployment); sibling rows (binance/bybit/hyperliquid) are
   reference context. The venue `rate` is an **8-hour aggregated print** (tracks
   the venue UI's "8hr" figure; hourly ~= rate / 8) — adapters must record it
   with `interval_hours = 8` or carry quotes inflate ~8x.

### 2.3 Signing Architecture (IMPLEMENTED 2026-09-20)
Orders use the ZK-friendly Schnorr scheme (Goldilocks-quintic + Poseidon2) for
which no Rust crate exists, so signing is delegated to the reference Go
implementation (`github.com/elliottech/lighter-go`, Apache-2.0) compiled as a
C-shared library and vendored at
`crates/arbitrageur-exchanges/native/lighter/liblighter_signer.so`
(`libloading` runtime load: `LIGHTER_SIGNER_LIB` env → sidecar next to the
executable → vendored copy; a missing library is a fail-closed error, never a
fabricated signature).
- Nonce: fresh `GET {base}/api/v1/nextNonce?account_index=&api_key_index=`
  (uncached — a cached nonce would double-submit).
- Sign: `SignCreateOrder` / `SignCancelOrder` over the L2 payload
  (tx_type create=14 / cancel=15; chain id from base URL: mainnet 304,
  testnet 300, `rh.lighter` 466324; unknown → 304).
- Submit: form `POST {base}/api/v1/sendTx` (`tx_type` + urlencoded `tx_info`);
  success requires HTTP-ok + `code==200` + echoed `tx_hash` match.
- Identity: account index (>0) + API private key + **API Key Index (0–254,
  UI collects 2–254; the Go validator rejects 255, so there is no wildcard
  default — orders fail closed until `set_api_key_index()` is configured,
  now plumbed from the vault through `state.rs` for both `lighter` and the
  `robinhood` deployment). `live_order_submission_implemented() == true`;
  the Robinhood adapter forwards it automatically.

---

## 3. Vanta Exchange (`vanta.exchange`)

### 3.1 Network Architecture
Vanta is an onchain perpetual DEX for crypto, equities, and commodities powered by the **Orderly Network** broker protocol. Vanta does not host a separate public API — per Vanta's docs its perpetual API is Orderly's.

- **REST API:** `https://api.orderly.org` (Orderly-hosted mainnet)
- **WebSocket:** `wss://ws.orderly.org` (Orderly-hosted)

### 3.2 Key Endpoints & Conventions
1. **Symbol Formatting:** All perpetual instruments follow the format `PERP_{BASE}_USDC` (e.g. `PERP_BTC_USDC`, `PERP_ETH_USDC`).
2. **Public read-only queries** go through a single endpoint, `POST /v1/public/query`, unauthenticated:
   ```bash
   POST /v1/public/query
   {"type": "marketSummary"}
   ```
   returns `data.markets` with decimal-string `symbol`, `bid_price`, `ask_price`, `mark_price`, `last_funding_rate`, `est_funding_rate`, `next_funding_time`, `min_notional`, `quote_tick`, `base_tick`.
3. **Orderbook Snapshot:**
   ```bash
   POST /v1/public/query
   {"type": "orderbook", "symbol": "PERP_BTC_USDC", "max_level": 100}
   ```
   returns `data.asks`/`data.bids` with decimal-string `price`/`quantity`. Empty sides are valid.
4. **Lightweight probe:** `{"type": "rateLimitStatus"}` (weight 0).

### 3.3 Signing Protocol
Orderly Network uses Ed25519 authentication keys. Requests require the headers:
- `orderly-account-id`: Account ID string
- `orderly-key`: Public Ed25519 key (base58 encoded)
- `orderly-signature`: Ed25519 signature over `timestamp + method + path + body`
- `orderly-timestamp`: Unix timestamp in milliseconds

---

## 4. Robinhood EVM / Web3 Router

### 4.1 Architecture
Robinhood Web3 connects to the Robinhood EVM chain with multi-DEX router support (Uniswap v3, OKX DEX, PancakeSwap v3).

- **Router Contract:** `0x89e5DB8B5aA49aA85AC63f691524311AEB649eba`
- **Supported Chains:** Robinhood EVM, Arbitrum, Base, Monad.
- **Role in Arbitrageur:** Facilitates atomic spot purchases and token bridging into Lighter / PopDEX pools.

---

## 5. Prediction Markets (Polymarket & Kalshi)

### 5.1 Polymarket (Polygon CTF CLOB)
- **API URL:** `https://clob.polymarket.com`
- **Contract Standard:** ERC-1155 Conditional Tokens Framework (CTF)
- **Order Mechanism:** EIP-712 Order struct signed by Polygon wallet.

### 5.2 Kalshi (CFTC-Regulated Exchange)
- **API URL:** `https://trading-api.kalshi.com`
- **Contract Format:** Standardized $0.01 - $0.99 binary event contracts.
- **Settlement:** Cash-settled at $1.00 USD upon market resolution.

---

## 6. Aster (Binance-Style Futures DEX)

Native adapter (`AsterAdapter`, venue `aster`, admitted to `get_venues`).
Public market/account reads, authenticated submission transport, and
order-status polling (`GET /fapi/v3/order`) are implemented; automated
residual-leg resolution and cancel retry now exist as opt-in mechanisms, but strategy live entry
remains disabled.

- **REST API:** `https://fapi.asterdex.com` (`/fapi/v3/*` recommended,
  `/fapi/v1/*` legacy)
- **WebSocket (recorded, stubbed for v1):** `wss://fstream.asterdex.com`

### 6.1 Key Endpoints
1. **Connectivity:** `GET /fapi/v3/ping` -> `{}` (live-verified 2026-09-15)
2. **Orderbook:**
   ```bash
   GET /fapi/v3/depth?symbol=BTCUSDT&limit=100
   ```
   returns `{lastUpdateId, E, T, bids: [[px, qty]...], asks: ...}` with
   decimal-string numbers. Valid limits: 5, 10, 20, 50, 100, 500, 1000.
3. **Markets:** `GET /fapi/v3/exchangeInfo` — 594 symbols (582 USDT-margined,
   10 USD1-margined, 2 U-stablecoin; NO USDC perps). Entries carry
   `baseAsset`/`quoteAsset` plus `PRICE_FILTER`/`LOT_SIZE`/`MIN_NOTIONAL`
   filters. The adapter never assumes a USDT suffix: the quote always comes
   from the entry's own `quoteAsset`, and bare UI bases resolve through the
   live listing (USDT > USD1 > U preference).
4. **Funding:** `GET /fapi/v3/premiumIndex?symbol=...` (`markPrice`,
   `lastFundingRate`, `nextFundingTime`, 8h interval) and
   `GET /fapi/v3/fundingRate` history. Bulk `fetch_funding_rates` joins the
   all-symbols premiumIndex to the exchangeInfo quote map.
5. **24h volumes:** `GET /fapi/v3/ticker/24hr` (`quoteVolume`).

### 6.2 Rate Limits
`REQUEST_WEIGHT` 2400/min/IP; depth weight 2-20 by limit. HTTP 429 feeds the
coordinator backoff; HTTP 418 means IP ban. The adapter polls at a 100ms
minimum interval.

---

## 7. Ondo Perps (Tokenized Equities + Crypto)

Historical paper read-only research (`OndoPerpsAdapter`, venue `ondoperps`,
not admitted by `get_venues` in the current build). Public REST only, no
auth. Poll politely: ~1 req/s venue
guidance, so the adapter's coordinator minimum interval is 1100ms.

- **REST API:** `https://api.ondoperps.xyz`
- **WebSocket (recorded, stubbed for v1):** `wss://api.ondoperps.xyz/ws`
  (`topOfBooksPerps` / `depthBooksPerps` public channels)

### 7.1 Key Endpoints
1. **Status:** `GET /status` ->
   `{"success": true, "result": {"marketStatus": "open"}}` (live-verified
   2026-09-15). Every response is enveloped `{"success": bool, "result": ..}`;
   anything without `success: true` fails closed.
2. **Markets:** `GET /v1/markets` -> `result.perps.tradingPairs` (81 markets,
   live-verified). Entries carry `market` (`BTC-USD.P`), `pair: {base, quote}`,
   `baseIncrement`/`quoteIncrement` ticks, decimal-fraction `makerFee`/
   `takerFee` (BTC: 0.0001 / 0.00025), and `fundingIntervalDivisions` (8 ->
   3h interval).
3. **Orderbook:**
   ```bash
   GET /v1/perps/depth?market=BTC-USD.P&depth=10
   ```
   returns `{success, result: {market, time, asks: [[px, qty]], bids}}`
   (live-verified for crypto and equity markets, e.g. `AAPL-USD.P`).
4. **Funding:** `GET /v1/perps/funding_rates?market=...` -> `{market, rate,
   intervalEnds, premiums}` (live-verified).

### 7.2 Symbol Mapping
UI canonical bases map to `{BASE}-USD.P` (`BTC` -> `BTC-USD.P`,
`AAPL` -> `AAPL-USD.P`).

### 7.3 Ondo Perps vs Ondo Finance — Do Not Confuse Them
**Ondo Finance proper** (USDY/OUSG tokenized T-bills, yield-bearing RWA
mint-redeem at `api.gm.ondo.finance`) is **not an exchange** and nothing in
this repo integrates with it. The venue built here is **Ondo Perps**
(`api.ondoperps.xyz`), a perpetual-futures venue for tokenized equities and
crypto. Never send RWA mint/redeem flows through the perps adapter.

---

## 8. GRVT (JSON POST Market Data)

Registered-but-**unadmitted** adapter (`GrvtAdapter`, venue `grvt`):
constructor, probe, and fee wiring exist, but `get_venues` does not admit it
until a full end-to-end sweep (markets + book + funding + volumes) is
confirmed in the desktop harness. Public POST only, no auth.

- **REST API:** `https://market-data.grvt.io` (`/full/v1/*`)
- **WebSocket (recorded, stubbed for v1):** `wss://market-data.grvt.io/ws/full`
  (JSONRPC `subscribe`, e.g. `v1.book.s` /
  `BTC_USDT_Perp@500-10` selectors)

### 8.1 Key Endpoints (All POST, Live-Verified 2026-09-15)
1. **Orderbook:**
   ```bash
   POST /full/v1/book  {"instrument": "BTC_USDT_Perp", "depth": 50}
   ```
   returns `{result: {instrument, bids: [{price, size, num_orders}], asks,
   event_time}}`. **Depth allowlist is exactly {10, 50, 100}** — probed
   depths 1/5/15/20/25/30/200 all return code 3031 "Depth is invalid", and
   the adapter fails closed on anything outside the allowlist.
2. **Markets:** `POST /full/v1/all_instruments
   {"is_active": true, "kinds": ["PERPETUAL"]}` -> `{result: [{instrument,
   base, quote, kind, tick_size, min_size, min_notional,
   funding_interval_hours}]}`. Instruments are `BASE_QUOTE_Perp`
   (`BTC_USDT_Perp`).
3. **Funding:** `POST /full/v1/funding {"instrument": ..., "limit": 1}` ->
   `{result: [{funding_rate, funding_time (ns), funding_interval_hours}]}`.

### 8.2 Symbol Mapping
`BTC` / `BTC-USDT` / `BTC_USDT_Perp` all map to `BTC_USDT_Perp` (bare bases
default to USDT — every live GRVT perp is USDT-quoted).

---

## 9. edgeX (StarkEx Perp DEX)

Registered-but-**unadmitted** adapter (`EdgeXAdapter`, venue `edgex`): same
status as GRVT — registered with probe + fee, admission withheld pending a
confirmed harness sweep. Public REST only, no auth. All responses are
enveloped `{"code": "SUCCESS", "data": ...}`; any other code fails closed.

- **REST API:** `https://edgex-prod-v2.edgex.exchange` — this is the docs'
  production api-domain (mainnet, live-verified 2026-09-15).
  `https://pro.edgex.exchange` serves the web app and 404s
  `/api/v2/public/*`; `https://api.edgex.exchange` is unreachable. Contract
  ids are never hardcoded: the legacy docs example `10000001` (BTCUSDT)
  already returns empty data; the live catalog is 173 USDC-settled contracts
  (`30000001` = BTCUSDC).
- **WebSocket (recorded, stubbed for v1):** `wss://quote.edgex.exchange`
  (`{"type": "subscribe", "channel": "depth.{id}.{15|200}"}`, SNAPSHOT then
  Changed deltas).

### 9.1 Key Endpoints (Live-Verified 2026-09-15)
1. **Metadata (docs § Metadata API):**
   `GET /api/v2/public/meta/getMetaData` -> `data.contractList` with
   `contractId`, `contractName` (`BTCUSDC`), `tickSize`, `stepSize`,
   `minOrderSize`, decimal-fraction `defaultTakerFeeRate` (0.00045 = 4.5 bps)
   / `defaultMakerFeeRate` (0.0004 = 4 bps), `fundingRateIntervalMin` (240 =
   4h), `enableTrade`. Contract names split as BASE + quote suffix
   (USDC/USDT/USD, longest first).
2. **Depth (docs § Quote API):**
   ```bash
   GET /api/v2/public/quote/getDepth?contractId=30000001&level=15
   ```
   Level must be 15 or 200. Returns `data: [{contractId, contractName,
   asks: [{price, size}], bids, depthType}]`.
3. **Ticker:** `GET /api/v2/public/quote/getTicker?contractId=...`
   (omit the id for the full catalog) -> `lastPrice`, `markPrice`,
   `fundingRate`, `fundingTime`, `nextFundingTime`, 24h `value` (notional,
   feeds `fetch_volumes_24h`). Bulk `fetch_funding_rates` uses the
   full-catalog ticker.
4. **Funding:** `GET
   /api/v2/public/funding/getLatestFundingRate?contractId=...` ->
   `{fundingRate, forecastFundingRate, fundingTime, ...}`.

### 9.2 Symbol Mapping
`BTC`, `BTC-USDC`, `BTCUSDC` all resolve through live metadata to the
numeric contract id (USDC-settled preferred, then USDT). Raw numeric ids pass
through only after metadata confirms they name a tradable contract.

---

## 10. Long-tail perp venues — historical research, not admitted

The 2026-09-19 `extended_venues.rs` experiment is no longer part of the
current workspace. The table below is retained only as API/custody research;
none of these venues is admitted by `get_venues`, listed in the runtime
catalog, or available for strategy execution. Any restoration requires a new
adapter, current endpoint verification, and a capability-matrix update. The
original user asked for two
things per venue: (a) a working market-data adapter, and (b) a custody
verdict — "can they take my money away if they shutdown, skip town, or go
bankrupt". The table below is (b); the adapter status column is (a).

**Custody reading guide.** LOW = self-custody onchain settlement (user keeps
keys; the operator cannot move collateral — but frontend/API outages can still
strand positions in volatility, and contract bugs are a separate risk).
MEDIUM = hybrid (offchain matching, onchain settlement — the operator runs the
book but cannot sign for your collateral). CAUTION = trade paper-only until the
named issue resolves.

| Venue | Venue id | Custody | Can they take funds? | Historical adapter status |
|---|---|---|---|---|
| dYdX Chain | `dydx` | LOW (L1, self-custody) | No — onchain positions, user keys | Historical research (curl-verified: markets + book) |
| Paradex | `paradex` | LOW (Starknet, self-custody) | No — Starknet settlement, user keys | Historical research (curl-verified: markets + book) |
| GMX V2 | `gmx` | LOW (GLV vaults, self-custody) | No — vault shares, user keys; prices-only feed (no orderbook by design) | Historical research (curl-verified tickers; excluded from spread pairs) |
| Aevo | `aevo` | LOW–MED (offchain book, onchain settlement) | Cannot move collateral without user signatures | Historical research (curl-verified: markets + book) |
| ApeX Omni | `apex` | LOW (self-custody modules) | No — user keeps keys | Historical research (LIVE 2026-09-19: 88 markets, 25-level books, hourly funding) |
| Extended | `extended` | LOW (StarkEx, self-custody) | No — user keeps keys | Historical research (LIVE 2026-09-19: 326 markets, full-depth books, funding in marketStats; client sends UA+Accept — edge 403s fingerprint-less clients) |
| Derive | `derive` | LOW (L2, self-custody) | No — user keeps keys | Historical research (LIVE 2026-09-19: per-currency instruments via `api.lyra.finance`, top-of-book + funding from `get_ticker`; no depth endpoint exists) |
| StandX | `standx` | MEDIUM (hybrid matching) | Operator runs book, cannot sign collateral | Historical research (LIVE 2026-09-19: 13 markets, 130-level books, hourly funding; per-entry 4.0 bps taker fee) |
| Helix (Injective) | `helix` | LOW (Cosmos L1, self-custody) | No — onchain, user keys | Historical research (LIVE 2026-09-19: 286 LCD derivative markets, scaled onchain books; funding NOT on LCD — fails closed) |
| Vest | `vest` | MEDIUM (hybrid) | Operator cannot sign collateral | Historical research (UNVERIFIED 2026-09-19: `api.vest.exchange` Cloudflare-530s — origin DNS dead; fails closed) |
| Antarctic | `antarctic` | MEDIUM (thin docs) | Unverified — treat as custodial until docs confirm | Historical research (UNVERIFIED 2026-09-19: site is a CloudFront shell, no public API found; fails closed) |
| EVEDEX | `evedex` | MEDIUM (hybrid) | Operator cannot sign collateral (docs claim) | Historical research (UNVERIFIED 2026-09-19: `api.evedex.com` DNS-fails, `exchange.evedex.com` 404s; fails closed) |
| BULK | `bulk` | MEDIUM (thin docs) | Unverified — treat as custodial until docs confirm | Historical research (UNVERIFIED 2026-09-19: `api.bulk.exchange` serves an unrelated landing page; real app at `app.bulk.trade` has no public API found; fails closed) |
| QFEX | `qfex` | CAUTION (6-person team, thin docs) | Small-team risk — paper only | Historical research (LIVE 2026-09-19: 216 markets via `/refdata`; books/tickers auth-gated — fail closed) |
| Ostium | `ostium` | CAUTION (oracle exploit ~$18M, paused) | Exploit history — paper only until audited relaunch | Historical research (UNVERIFIED 2026-09-19: onchain Arbitrum reads only — no public REST; Python SDK needs key+RPC; fails closed) |
| SoDEX | `sodex` | MEDIUM (hybrid) | Operator cannot sign collateral (docs claim) | Historical research (UNVERIFIED 2026-09-19: `sodex.xyz` is a parked `/lander` domain; fails closed) |
| AlphaX | `alphax` | CAUTION (no proof-of-reserves) | No PoR — paper only until published | Historical research (UNVERIFIED 2026-09-19: `alphax.exchange` is a parked `/lander` domain; fails closed) |
| Pacifica | `pacifica` | CAUTION (ex-FTX team, invite beta) | Team history + beta — paper only | Historical research (LIVE 2026-09-19: 77 markets via `/v1/info`, books via `/v1/book`, funding+predicted via `/v1/info/prices`; crossed books rejected) |
| Nado | `nado` | MEDIUM (thin docs) | Unverified — treat as custodial until docs confirm | Historical research (UNVERIFIED 2026-09-19: `api.nado.xyz` is a dead Vercel deployment; app live but no public API found; fails closed) |
| IO Trader | `iotrader` | MEDIUM (thin docs) | Unverified — treat as custodial until docs confirm | Historical research (UNVERIFIED 2026-09-19: `io-trader.com` is a parked page — no such venue found; fails closed) |
| Niza.fun | `niza` | MEDIUM (thin docs) | Unverified — treat as custodial until docs confirm | Historical research (UNVERIFIED 2026-09-19: `niza.fun` is a memecoin site, no futures product found; fails closed) |
| Astros | `astros` | LOW (Sui, onchain via DeepBook) | No — user keeps keys (docs claim) | Historical research (UNVERIFIED 2026-09-19: perp frontend live at `perp.astros.ag`, no public REST found; fails closed) |
| Gate DEX | `gatedex` | MEDIUM (thin docs) | Unverified — treat as custodial until docs confirm | Historical research (UNVERIFIED 2026-09-19: `api.gate-dex.io` DNS-fails — no such venue found; fails closed) |

**Deliberately not venues.** Orderly "Derivatives" is the same Orderly rails
as Vanta — a second Orderly venue would double-count one orderbook, so use the
Vanta venue. RISEx has no live mainnet (nothing to integrate) — skipped, will
revisit after launch.

**Verification.** dYdX (`indexer.dydx.trade/v4/perpetualMarkets` +
`/v4/orderbooks/perpetualMarket/BTC-USD`), Paradex (`api.prod.paradex.trade`
`/v1/markets` + `/v1/orderbook/BTC-USD-PERP`), Aevo (`api.aevo.xyz/markets` +
`/orderbook`), and GMX (`arbitrum-api.gmxinfra.io/prices/tickers`) returned
live payloads in this sandbox. A second sweep (2026-09-19, network
available) verified 7 more end-to-end through the Rust adapters
(`live_probe` integration test): ApeX Omni (88 markets / 25-level books /
hourly funding), Extended (326 markets / full-depth books / marketStats
funding), Derive (5 perps across 10 currencies / top-of-book / ticker
funding), StandX (13 markets / 130-level books / hourly funding), Helix
(286 LCD markets / scaled onchain books; funding not on LCD — fails
closed), Pacifica (77 markets / books / funding + predicted rate), and
QFEX (216 markets via `/refdata`; books auth-gated — fails closed). The
remaining 12 were UNVERIFIED in that research sweep (dead/parked hosts or no
public REST found — see table). These notes do not admit any venue today;
each parser was designed to fail closed per entry (bad
shapes skip entries, empty books never masquerade as quotes, crossed
Pacifica books are rejected), so a wrong guess degrades to venue errors,
never bad data.

---

## 11. PopDEX (Morph Tachyon EVM)

Native-API adapter (`PopDexAdapter`, venue `popdex`, admitted to `get_venues`).
Trading is entirely on-chain via precompile contracts; market/account data is
public REST. Guide: <https://popdex.xyz/docs/product-docs/onboarding/how-to-trade-via-api>
(docs deploy `604061a`, researched 2026-09-20).

- **Chain:** Morph Tachyon, full EVM, gasless, instant HotStuff-BFT finality.
  Mainnet chain id `2184`, RPC `https://app.popdex.xyz/api/v1/web3/rpc`;
  testnet chain id `34952`, RPC `https://testnet-app.popdex.xyz/api/v1/web3/rpc`
  (test USDT faucet; validate automation there first per PopDEX's own advice).
- **Contracts (fixed precompiles, never change):** `Order` `0x…1000`
  (place/cancel/match), `Account` `0x…1008` (sub-accounts, agent auth),
  `UserConfig` `0x…1009` (leverage, position mode).
- ** REST (`https://api.popdex.xyz`, testnet `https://testnet-api.popdex.xyz`,
  no auth, `{code:"200",…}` envelope):** `GET /api/v1/public/market/tickers`
  (66 futures symbols live, carries `symbolId`, top-of-book, mark/index,
  funding, OI, 24h turnover), `.../orderbook` (levels 1/5/25/50/100/200),
  `GET /api/v1/market/funding-rate` (rate, hour interval, next update),
  `.../fee-rate` (VIP0 taker/maker 0.1% = 10 bps), `.../config/symbol`
  (tick/lot sizes, min qty/notional), account `.../{wallet}/overview`
  (equity, margins, balances), `.../positions`, `.../orders`, `.../agents`.
  Rate limit 1200 weight/min/IP sliding window (tickers/book/funding weight 2,
  account reads weight 5); 429 feeds the coordinator backoff.
- **Agent model:** the Agent Key (`Agent Account` on-chain) is PopDEX's API
  key. Generated fresh locally, approved once by the main wallet via
  `approveAgent(agent, delegator, 0, 0, initialNonce, false)` (max 10 agents
  per main account; never re-approve a revoked address — replay risk).
  Orders are agent-signed with millisecond **timestamp nonces** (window
  now−1h…now+1d, above the smallest of the last 100, never reused); the
  adapter enforces monotonicity. Withdrawals are main-wallet-only; a deposit
  must land before approval. Max 500 open orders per account.
- **Order encoding:** `placeOrder(account, clientOrderId, symbolId, orderParams,
  price, qty, slippage, 0, 0)` with unified `orderParams` bytes
  (Futures=2, Limit/Market, Buy/Sell, GTC/IOC/FOK/PostOnly, BaseToken qty,
  reduce-only, one-way default), 18-decimal price/qty, `cancelOrder` by
  numeric id or client oid, `cancelAllOrders` per symbol, `updateLeverage`
  via `UserConfig` (USDT margin token). Legacy txs (chain-id bound, node gas
  price, `estimateGas`-gated so reverts fail closed pre-send); receipts waited
  ≤30s and `OrderCreate` events decoded for the `orderId` — never invented.
- **Credentials (vault):** main account address (public) + agent private key
  only. The main key is never held — one-time approval runs externally
  (exact `cast`/ethers commands in the adapter error text + guide Step 4).
  Binding an agent key that derives the main address is refused.
- **Verification:** market/account REST verified live 2026-09-20 (real BTC
  book, 1h funding, empty-array reads for unknown accounts). Contract
  submission paths are implemented per the published interface with
  unit-tested packing/nonce/receipt logic but **not yet dry-run on testnet**
  — fund testnet first, then mainnet size.

---
## 12. Decibel (Aptos CLOB Perp DEX) — live submission IMPLEMENTED 2026-09-20

Native-API adapter (`DecibelAdapter`, venue `decibel`). Reads go through the
Geomi trading API (`{base}/api/v1/...`, `Authorization: Bearer <client key>`);
writes are genuine on-chain Aptos transactions signed with the vault-held
wallet key via the `aptos-sdk` Rust crate (v0.7.1, `ed25519` feature) plus the
`bcs` crate — non-Rust crypto was considered but the audited Rust SDK covers
the whole flow, so no sidecar was needed.

- **Deployment (official `decibeltrade/sdk` `PACKAGE`):**
  mainnet `0x50ead22afd6ffd9769e3b3d6e0e64a2a350d68e8b102c4e72e33d0b8cfdfdb06`
  (chain id 1), testnet
  `0xe7da2794b1d8af76532ed95f38bfdf1136abfd8ea3a240189971988a83101b7f`
  (chain id 2). Network is resolved from the adapter base URL (`testnet`
  substring → testnet package + testnet fullnode).
- **Entry functions** (`<package>::dex_accounts_entry::…`, sender = the
  vault-held ed25519 signer — normally the API wallet delegated
  `TradePerpsAllMarkets` on the owner's subaccount):
  `place_order_to_subaccount(subaccount, market, price:u64, size:u64,
  isBuy:bool, tif:u8, reduceOnly:bool, Option<String> clientId,
  Option<u64>×5 stops/TP/SL, Option<address> builder, Option<u64> fee)`,
  `cancel_order_to_subaccount(subaccount, orderId:u128, market)`,
  `cancel_client_order_to_subaccount(subaccount, clientId:String, market)`.
  Arg shapes verified against the SDK `mainnet.json` ABI.
- **Address derivation** (`object::create_object_address` =
  `sha3_256(creator || seed || 0xFE)`):
  `GlobalPerpEngine` named object under the package (mainnet
  `0x4d8cc47e…928098370` — verified live on the fullnode, owned by the
  package), market = named object under the engine keyed by BCS(MoveString)
  of the market name (mirrors SDK `getMarketAddr`), primary subaccount via
  the `GlobalSubaccountManager` deriver with seed
  `owner_bytes ++ BCS("primary_subaccount")` (mirrors
  `getPrimarySubaccountAddr`). Pure-Rust unit tests pin the mainnet vectors.
- **Owner resolution** (2026-09-23): the configured Login Wallet Address may
  be an Aptos account, an EVM/Sui address, or a Solana pubkey — Decibel's
  SIWE login derives an Aptos *abstract account* per chain
  (`0x1::{ethereum,sui,solana}_derivable_account::authenticate`, domain
  `app.decibel.trade` / `testnet-app.decibel.trade`, EVM identity is the
  EIP-55 checksummed string). `resolve_owner` probes `subaccounts?owner=`
  for each shape-derived candidate (EVM-derived → zero-padded for 40-hex;
  native → Sui-derived for 64-hex; Solana-derived otherwise) and caches the
  first owner with a non-empty listing. Reads and writes share the
  resolution; the derived address is invisible in Decibel's UI, so it must
  be computed — the subaccount object address itself is never valid input.
- **Scaling:** price/size chain units come from the LIVE `/api/v1/markets`
  row (`px_decimals`/`sz_decimals`); unresolvable markets fail closed.
  Prices side-safe tick-aligned (buy floor / sell ceil), sizes floored to the
  lot multiple, minima enforced. TIF mapping GTC→0 / PostOnly→1 / IOC·FOK→2.
  Limit orders fully supported; market orders become a bounded
  marketable-limit (live touch ±0.5% collar, IOC) — a blind cross is refused.
- **Submission:** per-adapter async order lock (one sequence number per
  account), live sequence/chain-id/gas-price from the fullnode, 120s expiry,
  50k-unit gas cap, `submit_and_wait` ≤30s; vm failures are errors, never
  fills. The venue order id is parsed best-effort from commit `OrderEvent`s
  (subaccount-matched), falling back to the tx hash. Status is `New` with
  full remaining qty — fills are never invented.
- **Order-status reconciliation:** `fetch_order` polls the committed tx via
  `transactions/by_hash`, then — while the order is still working — scans
  post-commit `accounts/{subaccount}/transactions` (last 50, ascending
  version) for `OrderEvent`/`TradeEvent` rows identity-matched on
  `client_order_id`/`order_id`. Venue lifecycle rows are authoritative for
  cumulative fills; anonymous batch-tx fills without order identity are
  never attributed. Window truncation fails closed to `unresolved`.
- **Credentials (vault):** Geomi client API key (reads) + Aptos wallet
  private key (signer) + optional wallet address (defaults to the key's own
  account). `has_live_credentials` keys on the private key; everything
  without it fails closed with an actionable message.
- **Reads (rebuilt 2026-09-20 on official SDK paths after a guessed
  `/v1/subaccounts` 404):** `GET /api/v1/subaccounts?owner=` → primary
  subaccount (lowercase retry); `GET /api/v1/account_overviews?account=`
  (`perp_equity_balance`, `usdc_cross_withdrawable_balance`,
  `maintenance_margin`); `GET /api/v1/account_positions` (rows labelled via
  the live markets map, marked via `/api/v1/prices` mids with entry
  fallback); **funding via `/api/v1/prices?market=`** (`funding_rate_bps` +
  `funding_period_s` → 8h print, Decibel legs join funding scans).
- **Still fail-closed by design:** orderbook depth (REST removed upstream —
  WebSocket-only, tried once then refused, never synthesized), ticker (no
  REST bid/ask); matching is async on-venue.

---

## 6. Transfer Rails & Rebalance Friction (researched 2026-09-19)

| Venue | Deposit rails | Withdraw rails | Withdraw fee | Deposit fee | Direct venue→venue? | Confidence |
|---|---|---|---|---|---|---|
| Hyperliquid | USDC (Arbitrum) | USDC (Arbitrum) | $1.00 flat | Free | Yes | verified |
| Lighter | USDC (ETH/ARB bridge) | USDC | $0 protocol | Free | Yes | documented |
| Variational | USDC | USDC | $0.10 flat | $0.10 flat | Yes | documented |
| Vanta (Orderly) | USDC (ARB/OP/Base) | USDC | ~$1.00 | Free | Yes | estimated |
| Aster / edgeX / GRVT / Decibel / Ondo | USDC(/USDT) | USDC(/USDT) | ~$0.50–1.00 | Free | Yes | estimated |
| Binance / OKX | USDC + multis | USDC + multis | ~$0.40–0.50 (ARB) | Free | Yes | estimated |
| Coinbase | USDC/USD | USDC/USD | network only | Free | Yes | estimated |
| Kraken | USDC/USD | USDC/USD | ~$2.50 | Free | Yes | estimated |
| Robinhood | USD (crypto limited) | USD | unknown | Free | No | unknown |
| PopDEX | USDT (ARB/Morph bridge, top-up addr) | USDT (main-wallet signed) | unknown | Free | Yes | documented |

Notes: CEX fees vary by withdrawal network — always choose Arbitrum over ERC-20
for cents-vs-dollars gas. Swap friction default 15 bps (taker + spread),
adjustable. Estimated rows MUST be re-checked on the venue withdraw page before
moving size. Full table + estimator: `frontend/src/services/rebalanceFees.ts`.
