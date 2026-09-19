# Exchange Integrations Guide

> Living venue reference — includes no-API venues (Variational live market data, PopDEX beta model) and §6 transfer-rails/rebalance research. Re-check fee figures on venue pages before moving size.


This guide details the integration protocols, API specifications, and signing mechanisms for each supported market venue in the **Arbitrageur** platform.

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

### 2.3 Signing Architecture
Order submissions require native Starknet / Poseidon curve signatures generated via Lighter's native C-ABI dynamic library (`lighter-signer-linux-amd64.so`) or Taoli remote signer.

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

Paper read-only adapter (`AsterAdapter`, venue `aster`, admitted to
`get_venues`). Public REST only, no auth.

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

Paper read-only adapter (`OndoPerpsAdapter`, venue `ondoperps`, admitted to
`get_venues`). Public REST only, no auth. Poll politely: ~1 req/s venue
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
| PopDEX (beta) | TBD | TBD | unknown | unknown | No | unknown |

Notes: CEX fees vary by withdrawal network — always choose Arbitrum over ERC-20
for cents-vs-dollars gas. Swap friction default 15 bps (taker + spread),
adjustable. Estimated rows MUST be re-checked on the venue withdraw page before
moving size. Full table + estimator: `frontend/src/services/rebalanceFees.ts`.
