# Capability Matrix

**Status date:** 2026-09-25
**Authority:** This file and `README.md` define current runtime capability. Historical handoffs describe what was attempted at the time and are not capability guarantees.

**Since 2026-09-23:** USD-quote policy (`usd_quote_mode` merge/separate + `stablecoin_fx` global rate feed with depeg gates), Stronghold custody for the shared EVM wallet key (`vault.hold` snapshot next to `vault.enc`), durable circuit-breaker/ pause state across restarts, bounded-IOC dispatch on OKX/PopDEX, and fail-closed prediction-market order books (real CLOB reads — fabricated book levels removed).

## Capability vocabulary

- **Read:** The adapter can request and parse the named public/authenticated data. Individual endpoints can still be unavailable and must fail closed.
- **Submit:** The adapter can sign and transmit an order to the venue. This does **not** prove that the order filled.
- **Reconcile:** The engine can follow an accepted order through terminal fill/cancel/expiry, recover after restart, and attribute partial fills without guessing.
- **Testnet strategy:** The strategy executes real orders against the venue's public demo/test endpoints — same signing, dispatch, and reconciliation path as live, against demo balances. Venues without a public testnet are simply ineligible in this environment; nothing is simulated.
- **Live strategy entry:** The strategy may open real positions automatically or from a strategy button.
- **Risk-reducing submission:** A user-directed reduce-only close can be sent, but an acknowledgement may remain “working” until independently confirmed.

A venue is never “live-entry ready” merely because it has credentials and can submit.

## Strategy capability

| Strategy / path | Authentic scan | Testnet execution | Live entry | Exit / reconciliation | Current operator meaning |
|---|---|---|---|---|---|
| Cross-venue perpetual spatial | Yes | Yes (testnet-capable venues) | Live mode + venue readiness (vault unlocked, submission + order polling + strategy-entry + credentials) — venue-proven by the HL↔Lighter live canary | Orders poll to terminal state and restart-recover; unconditional residual auto-flatten + bounded cancel retry | Residual one-sided-exposure risk remains on venue outage |
| Spread Radar manual entry | Yes | Yes | Dispatches through the spatial two-leg path — the same vault + adapter-eligibility + credential checks apply | Bot-managed exits | Manual entries live-ready |
| Spread Radar autonomous bot | Yes | Yes, opt-in | **Testnet-only** (GateContext hard-blocks live autonomous entries) | Bot-managed exits with audit | Testnet only |
| Perp/perp funding carry | Yes where both funding and books exist | Yes | Same vault + adapter-eligibility + credential checks as spatial | Orders poll to terminal; unconditional residual auto-flatten; **min-APR exit supervisor** flattens the position when EMA-smoothed net carry decays below the entry hurdle (60s cadence, fresh-rate-only decisions) | Manual entries live-ready; the Auto Carry loop is **testnet-only** — `auto=true` dispatches refuse in live |
| Same-venue spot/perp basis | Yes where one adapter exposes both exact books | Yes | Spot legs submit without `reduce_only` (the venue itself refuses oversells); if a venue still rejects the flatten the residual escalates `unresolved` for manual resolution | Exact per-leg flatten; same min-APR exit supervisor (spot legs contribute 0 rate) | Manual entries live-ready; autonomous entries testnet-only |
| Cross-venue spot arbitrage | No usable scanner | No | **Disabled** | Not implemented | Roadmap |
| Single-venue triangular | Yes on exact supported books | Yes | Three sequential legs on one venue under the same vault + adapter-eligibility + credential gate | Residual flatten | Manual entries live-ready; the auto-deploy loop is **testnet-only** (`params.auto` refuses in live) |
| Cross-ticker | Research/quote path only | No | **Disabled** | Not implemented | Roadmap |
| Prediction parity | Research scan when both configured adapters return real books | No | **Disabled** | Not implemented | Research only |
| Statistical arbitrage | Analysis tools | No integrated dispatcher | **Disabled** | Not implemented | Research only |
| External-position close | Authenticated account read | N/A | User-directed risk-reducing submission (works in both environments — testnet submits real demo-venue orders) | Working orders are continuously polled (5s) and restart-recovered by the reconciliation supervisor; unresolved orders escalate after 15 min | Monitor to terminal state; confirm at the venue |
| Emergency flatten | Attributed position read | Yes | User-directed risk-reducing submission only | Same continuous reconciliation; venue still authoritative | Monitor to terminal state; confirm at the venue |

## Venue adapter capability

“Unit” means deterministic parser/signing tests. “Public” means live public-read evidence recorded in the handoff. “Sandbox order” and “mainnet order” require credentialed evidence and are not implied by source code.

*"Strategy live entry" = the `live_entry_gate`: Trading Mode = Live plus an unlocked credential vault, and every leg venue must implement order submission, order-status polling, and strategy-entry eligibility and hold live credentials. Spatial, Spread Radar manual, funding carry/basis, and triangle entries all dispatch through it; autonomous entries (Radar bot, Funding Auto Carry loop) remain testnet-only — `auto=true` dispatches refuse in live; cross-venue spot and prediction parity have no live dispatch.*

| Venue | Markets / books | Funding | Account read | Sign + submit | Cancel | Leverage update | Terminal/restart reconciliation | Verification ceiling in repo | Strategy live entry |
|---|---|---|---|---|---|---|---|---|---|
| Hyperliquid | Perpetual; spot suffix refused | Yes | Yes | Yes | Yes | No | Order-status poll + restart supervisor | Unit + public reads | Gated by vault + venue readiness |
| Lighter | Perpetual and exact `-SPOT` resolution | Yes | Yes | Yes, vendored signer | Yes | No | `accountOrders` poll by client order index + restart supervisor | Unit + public reads | Gated by vault + venue readiness |
| Robinhood Lighter | Lighter deployment | Yes | Yes | Yes, same signer family | Yes | No | Delegates to Lighter polling | Unit + public reads | Gated by vault + venue readiness |
| Vanta / Orderly | Perpetual | Yes | Yes | Yes | Yes | No explicit implementation | `GET /v1/order/{id}` poll + restart supervisor | Unit + public reads | Gated by vault + venue readiness |
| Aster | Perpetual | Yes | Yes | Yes | Yes | Yes | `GET /fapi/v3/order` poll + restart supervisor | Unit + public reads | Gated by vault + venue readiness |
| PopDEX | Perpetual | Yes | Yes | Yes, agent-signed on-chain tx | Yes | Yes | Open + `history/orders` poll + restart supervisor | Unit + public reads; no documented sandbox order proof | Gated by vault + venue readiness |
| Binance | USDT-M perpetual | Yes | Yes | Yes | Yes | Yes | `GET /fapi/v1/order` poll + restart supervisor | Unit + public reads | Gated by vault + venue readiness |
| OKX | Perpetual swaps | Yes | Yes | Yes | Yes | Yes | `GET /api/v5/trade/order` poll + restart supervisor | Unit + public reads | Gated by vault + venue readiness |
| Coinbase | Spot | No perpetual funding | Yes | Yes | Yes | N/A; reduce-only unsupported | `GET /orders/{id}` poll + restart supervisor | Unit + public reads | Gated by vault + venue readiness |
| Kraken | Linear futures in current adapter | Yes | Yes | Yes | Yes | No explicit implementation | `openorders` + `/fills` aggregation poll + restart supervisor | Unit + public reads | Gated by vault + venue readiness |
| Decibel | Markets/prices; REST depth unavailable upstream | Yes through prices endpoint | Yes | Yes, Aptos transaction | Yes | No explicit implementation | Commit-event decode + tx-hash poll + post-commit subaccount-tx scan (last 50 txs, identity-matched OrderEvent/TradeEvent; anonymous fills unattributed, window truncation fails closed) | Unit + selected public reads | Gated by vault + venue readiness |
| Arcus | Adapter scaffold, not admitted | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | None | Scaffold only | **No** |
| Polymarket | Separate prediction research adapter | N/A | Not an admitted portfolio venue | No integrated execution | No integrated execution | N/A | None | Research parser | **No** |
| Kalshi | Separate prediction research adapter | N/A | Not an admitted portfolio venue | No integrated execution | No integrated execution | N/A | None | Research parser | **No** |
| Pacifica (DexPerpAdapter) | Perpetual | Yes | Yes (Ed25519 session) | Yes, Ed25519 signed ops | Yes | Yes | `orders` read + restart supervisor | Unit + live public reads; signing implemented, no credentialed order evidence yet | **No — pending venue evidence** |
| StandX (DexPerpAdapter) | Perpetual | Yes | Yes (JWT session) | Yes, Ed25519 signed bodies | Yes (async; WS confirm) | Yes | `query_order` poll + restart supervisor | Unit + live public reads; no credentialed order evidence yet | **No — pending venue evidence** |
| Ondo Perps (DexPerpAdapter) | Perpetual | Yes | Yes (SIWE → JWT) | Yes, wallet-signed | Yes | No explicit implementation | `GET /v1/perps/orders` poll + restart supervisor | Unit + live public reads; no credentialed order evidence yet | **No — pending venue evidence** |
| GRVT (DexPerpAdapter) | Perpetual | Yes | Yes (`gravity` cookie session) | Yes, EIP-712 legs | Yes | No explicit implementation | Order read poll + restart supervisor | Unit + live public reads; no credentialed order evidence yet | **No — pending venue evidence** |
| Derive (DexPerpAdapter) | Perpetual (top-of-book only) | Yes | Yes (`X-Lyra*` headers) | Yes, EIP-712 action sigs | Yes (`private/cancel`) | No explicit implementation | Order read poll + restart supervisor | Unit + live public reads; no credentialed order evidence yet | **No — pending venue evidence** |
| Aevo (DexPerpAdapter) | Perpetual | Yes | Yes (`AEVO-KEY`/`AEVO-SECRET`) | Yes, EIP-712 `Order` | Yes | No explicit implementation | Order read poll + restart supervisor | Unit + live public reads; no credentialed order evidence yet | **No — pending venue evidence** |
| Vest Markets (DexPerpAdapter) | Perpetual (crypto + equity/index/forex perps, zkSync Era) | Yes | Yes (`X-API-KEY` + `xrestservermm` routing) | Yes, delegate-signer `eth_sign` order sigs; primary key self-registers a ~7-day signer via `POST /v2/register` | Yes | No explicit implementation (leverage rides each order body) | `GET /v2/orders` poll + restart supervisor | Unit + live public reads; no credentialed order evidence yet | **No — pending venue evidence** |
| QFEX (DexPerpAdapter) | Perpetual | Yes | Yes (API-key HMAC) | Yes, authenticated `/user/trade` WS order entry | Yes | No explicit implementation | WS order-status subscription + REST order read + restart supervisor | Unit + live public reads; no credentialed order evidence yet | **No — pending venue evidence** |
| Nado (DexPerpAdapter) | Perpetual (Ink L2, chain 57073) | Yes | Yes (EIP-712 `Order`/`Cancellation` by main or linked signer) | Yes, signed execute payloads to `gateway.prod.nado.xyz` | Yes | No explicit implementation | Gateway `order` query + archive `orders` digest reconciliation + restart supervisor | Unit + live public reads (`nado_live_market_smoke`); no credentialed order evidence yet | **No — pending venue evidence** |
| EVEDEX (DexPerpAdapter) | Perpetual (multi-asset perps, Arbitrum One) | Yes | Yes (SIWE → JWT Bearer via `auth-api.evedex.com`, refresh + re-login) | Yes, EIP-712 signed orders (domain `EVEDEX`/`2`, chainId 42161, ×1e8 matcher ints, client-generated ids) | Yes (`DELETE /api/order/{id}`) | No explicit implementation | `GET /api/order/{id}` status poll + restart supervisor | Unit + live public reads (`evedex_live_market_smoke`); no credentialed order evidence yet | **No — pending venue evidence** |
| Paradex (DexPerpAdapter + paradex-rs) | Perpetual | Yes | Yes (JWT via Stark-key auth) | Yes, SNIP-12 Stark-curve order sigs (L2 key derived from EVM wallet) | Yes (`DELETE /v1/orders/{id}`) | No explicit implementation | `GET /v1/orders/{id}` + `orders-history` poll + restart supervisor | Unit + live public reads; no credentialed order evidence yet | **No — pending venue evidence** |
| edgeX (DexPerpAdapter, Perp V2) | Perpetual | Yes | Yes (`X-edgeX-*` HMAC api-key trio) | Yes, EIP-712 `LimitOrderParams` secp256k1 sigs (domain `EdgeX`/`1`/chain 3343/metadata contractAddress) | Yes (`cancelOrderById`/`cancelOrderByClientOrderId`) | Yes (`updateLeverageSetting`) | `getOrderById`/`getOrderByClientOrderId` poll + restart supervisor | Unit (SDK-verified HMAC + EIP-712 vectors) + live public reads (`edgex_live_public_smoke`); no credentialed order evidence yet | **No — pending venue evidence** |
| Extended (DexPerpAdapter, StarkEx engine on `api.starknet.extended.exchange`) | Perpetual | Yes | Yes (`X-Api-Key`) | Yes, SNIP-12 Poseidon `Order` struct sigs on the Stark curve (domain `Perpetuals`/`v0`/`SN_MAIN`/rev 1; vault = collateral position id; signing ported from official X10 `fast_stark_crypto` and vector-verified) | Yes (`DELETE /user/order/{id}` + `?externalId=`) | Yes (`PATCH /user/leverage`) | `GET /user/orders/{id}` + `/orders/external/{id}` poll + restart supervisor | Unit (official hash vectors + sign/verify roundtrip) + live public reads (`extended_live_public_smoke`); no credentialed order evidence yet | **No — pending venue evidence** |
| dYdX Chain (DexPerpAdapter + official `dydx` gRPC client) | Perpetual | Yes | Yes (mnemonic-derived Cosmos key — 24-word BIP-39 phrase stored as the private-key credential; `accountId` = subaccount, `walletAddress` verified against derivation) | Yes, `MsgPlaceOrder` signed + broadcast over validator gRPC | Yes (`MsgCancelOrder`, order id resolved from indexer) | No (cross-margin — no leverage endpoint) | Indexer `/v4/orders/{id}` + `?address=&subaccountNumber=` client-id poll + restart supervisor | Unit (mnemonic gate, proto order-id round-trip) + live public reads (`dydx_live_public_smoke`); no credentialed order evidence yet | **No — pending venue evidence** |
| Helix / Injective (DexPerpAdapter + `injective-std` protos) | Perpetual | Yes | Yes (the account's own EVM key — keccak address → `inj1…` bech32; `accountId` = subaccount nonce 0-255, `walletAddress` verified against derivation) | Yes, `MsgCreateDerivative{Limit,Market}Order` Cosmos txs — keccak256(`SignDoc`) + compact secp256k1 sig + `ethsecp256k1` pubkey, broadcast `BROADCAST_MODE_SYNC` on the chain LCD | Yes (`MsgCancelDerivativeOrder` by order_hash/cid) | No (per-order margin — no leverage endpoint) | LCD tx commit poll (order hash from `TxMsgData`) + indexer gRPC `OrdersHistory` truth + restart supervisor | Unit (bech32/subaccount derivation, wire formats, credential gate) + live public reads (`helix_live_public_smoke`); no credentialed order evidence yet | **No — pending venue evidence** |
| GMX V2, ApeX Omni, Antarctic, BULK, Ostium, SoDEX, AlphaX, IO Trader, Niza, Astros, Gate DEX (DexPerpAdapter, market-data-only) | Verified or doc-shaped public reads; GMX V2 is prices-only (GLV AMM has no CLOB), Derive top-of-book only | Yes where the venue publishes it | **No — fails closed** | **No — fails closed** | **No — fails closed** | **No — fails closed** | None | Unit + live public reads where marked verified; doc-shaped venues degrade to per-entry errors | **No** |
| Uniswap V3, Uniswap V2, PancakeSwap V2, PancakeSwap V3, SushiSwap, QuickSwap (EvmAmmAdapter, spot AMM — standard routers) | On-chain quoter/factory reads — probe-verified live; one-level book synthesized from real executable quotes (bid = exact-in 1 base, ask = exact-out cost of 1 base; both sides required, crossed/empty books fail) | N/A — spot pools | Yes — local EVM wallet account read | Yes — wallet-signed `swapExact*`/`exact*Single` txs; exact-in sells and exact-out buys; limit orders map to min-out/max-in bounds, post-only refused | Atomic swap — nothing rests to cancel | Spot pools — N/A | Submit returns tx hash as `New` immediately; `eth_getTransactionReceipt` polling is the authority — Filled/Rejected require BOTH input-spent and output-received `Transfer` proof; mined gas reported in native fee; restart reconciliation | Unit + live `eth_call` smoke test (`live_market_smoke`, `--ignored`); no credentialed swap evidence yet | **No — pending gas-aware profitability integration and credentialed venue evidence** (`live_strategy_entry_implemented` = false) |
| Camelot V2, Aerodrome, Velodrome, Ramses (legacy), Pharaoh (legacy) (EvmAmmAdapter, spot AMM — market data only) | Live public market data — probe-verified factory/quoter reads; one-level book with real quoted depth (ask via reverse exact-in) | N/A — spot pools | Optional address-only account read — no private key requested or loaded | **No** — Camelot router uses a custom referrer ABI; Solidly routers lack exact-output buys | **No** | Spot pools — N/A | **No** — no order submission, cancel, or status polling | Unit + live `eth_call` smoke test (`live_market_smoke`, `--ignored`) | **No** |
|| Pendle Boros (`BorosAdapter`, Arbitrum — YU rate swaps, hedge venue only) | `/v1/markets?isMatured=false` + `/v1/assets` (collateral price join) + `/v1/markets/order-book` rate ladders — no ticker/price book/funding feed by design | N/A — YU markets ARE the funding hedge | Yes (`accounts/active-positions` + `market-acc-infos`; agent-approval gate on every read) | Yes — calldata-builder → EIP-712 `PendleSignTx` (agent key, domain `Pendle Boros Router`/chainId 42161) → `/v1/send-txs/bulk-calls` | Yes (`calldata-builder/agent/cancel-orders`, same signed path) | No (per-market `maxLeverage`; no leverage endpoint) | `orders/by-tx-hash` id resolution + `/v1/accounts/orders` status poll + restart supervisor | Unit (account/marketAcc packing vs SDK, credential gate, market parse/maturity reject) + live public reads (`boros_live_markets_smoke`); no credentialed order evidence yet | **No — pending venue evidence** (fixed-yield entries are live-manual only) |

## Environments

The engine runs two real execution environments (`TradingEnvironment`); there is no simulated execution in production.

**Testnet** dispatches real signed orders against venue demo endpoints — the same submission/reconciliation pipeline as live, against demo balances:

| Venue | Testnet endpoint |
|---|---|
| Hyperliquid | `api.hyperliquid-testnet.xyz` |
| Lighter | `testnet.zklighter.elliot.ai` (chain 300) |
| Binance (USDT-M perp) | `testnet.binancefuture.com` |
| OKX | main host + `x-simulated-trading: 1` demo header |
| Kraken | `demo-futures.kraken.com` |
| Vanta / Orderly | `testnet-api.orderly.org` |
| Decibel | Aptos testnet (`testnet-app.decibel.trade` domain) |
| PopDEX | Morph testnet (chain 34952, `testnet-api.popdex.xyz`) |

Venues without a public demo endpoint — Coinbase, Aster, Robinhood, Arcus, Polymarket, all DexPerp long-tail venues, and all EVM AMMs — are ineligible in testnet; they are never simulated. Testnet account profiles are a separate credential set (`exchange_accounts.environment = 'testnet'`) managed while Testnet Trading is the active mode; fund them at each venue's own faucet.

## Live mode

Live mode currently means:

1. The credential vault is unlocked.
2. Authenticated venue account reads can be shown.
3. Explicit risk-reducing close submissions can be requested where an adapter supports them.
4. Live position reads are matched through adapter-owned account-position normalization (for example `BTCUSDT`, `PF_XBTUSD`, `BTC-USDT-SWAP`, `PERP_BTC_USDC`, `BTC-PERP`, and Lighter market IDs). Unknown or ambiguous mappings leave the position unresolved rather than treated as flat. Quantities and prices are kept in venue-native contract units (e.g. `1000PEPE-USDT` positions stay denominated in `1000PEPE` units with per-unit prices — no ×1000 rescaling in either direction).
5. **New strategy entries are gated, not blanket-blocked** — spatial two-leg, Spread Radar manual, funding carry/basis, and single-venue triangle paths dispatch live orders in Live mode once the vault is unlocked and every leg venue passes submission + order-status-polling + strategy-entry + credential checks (`live_entry_gate`). Cross-venue spot, prediction parity, and the autonomous Radar bot do not dispatch live. Every accepted non-final leg lands in `unresolved`/`imbalanced` reconciliation with its venue order id persisted; an operator acknowledge control resolves escalated signals after manual venue review. Residual auto-flatten submits reduce-only closes only where the venue supports the flag (`ExchangeAdapter::supports_reduce_only`) — spot instruments flatten via plain market orders since the venue itself refuses oversells.
6. **Market data stays websocket-primary in live mode** — the feed supervisor no longer suspends WS lanes when live; orderbooks flow via WS into `MarketDataHub` and REST is fallback-only (2s freshness bound enforced at execution). The monitor keeps running as the hub's REST feeder for venues without a WS lane and as the funding refresher. Engine pause still suspends both.
7. **Accounts are environment-scoped** — live credential profiles never bind to testnet adapters and vice versa; strategy routes are same-environment only.

Before enabling Live mode:

- Use minimum-permission API keys without withdrawal rights.
- Verify venue account identity and network.
- Watch the signals/positions view until every submitted close reaches a terminal reconciled state; unresolved escalations mean venue truth could not be confirmed.
- Keep a direct venue login and manual close/withdraw path available.

## Promotion requirements for live strategy entry

A venue/strategy pair can move to live entry only after all of these have evidence:

- Exact market and quantity conversion
- Idempotent client-order identity
- Terminal fill, partial-fill, reject, cancel, and expiry handling
- Reduce-only compensation
- Leverage/margin-mode confirmation or an explicit no-change invariant
- Durable accepted-order record before submission
- Restart recovery and reconciliation (implemented via the reconciliation supervisor)
- Automated residual-exposure resolution: implemented unconditionally — an imbalanced entry auto-submits reduce-only flattens (max 3 attempts) persisted as `flat-*` children the supervisor reconciles; requires venue evidence before it can be trusted live
- Cancel-failure retry: implemented — orders working past the 15-minute deadline get up to 3 cancel attempts (kv-persisted across restarts) before escalation; needs venue evidence
- Live entry gate: implemented — `live_entry_gate` admits entries on the spatial/radar/funding/triangle paths when the vault is unlocked and every leg venue passes submission+polling+strategy-entry+credential checks; unproven routes are validated with a minimal manual entry through the normal strategy paths
- Sandbox tests for success, partial fill, one-leg failure, timeout, cancel race, and process restart
- Small-notional owner-approved live canary — IN PROGRESS
- Updated capability row and release notes
