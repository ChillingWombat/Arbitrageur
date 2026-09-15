# Exchange Integrations Guide

This guide details the integration protocols, API specifications, and signing mechanisms for each supported market venue in the **Arbitrager** platform.

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
- **Role in Arbitrager:** Facilitates atomic spot purchases and token bridging into Lighter / PopDEX pools.

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
