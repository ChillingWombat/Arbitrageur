# Capability Matrix

**Status date:** 2026-09-23
**Authority:** This file and `README.md` define current runtime capability. Historical handoffs describe what was attempted at the time and are not capability guarantees.

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
| Cross-venue perpetual spatial | Yes | Yes (testnet-capable venues) | **Opt-in** (`live_entry_enabled` + per-leg notional cap; off by default) — venue-proven by the HL↔Lighter live canary | Orders poll to terminal state and restart-recover; unconditional residual auto-flatten + bounded cancel retry | Keep the cap minimal — residual one-sided-exposure risk remains on venue outage |
| Spread Radar manual entry | Yes | Yes | Via `live_entry_enabled` gate — dispatches through the spatial two-leg path, so the same vault + per-leg cap + adapter-eligibility checks apply | Bot-managed exits | Opt-in (off by default) |
| Spread Radar autonomous bot | Yes | Yes, opt-in | **Testnet-only** (GateContext hard-blocks live autonomous entries) | Bot-managed exits with audit | Testnet only |
| Perp/perp funding carry | Yes where both funding and books exist | Yes | Via `live_entry_enabled` gate — same vault + per-leg cap + adapter-eligibility checks as spatial | Orders poll to terminal; unconditional residual auto-flatten; **min-APR exit supervisor** flattens the position when EMA-smoothed net carry decays below the entry hurdle (60s cadence, fresh-rate-only decisions) | Manual opt-in (off by default); the Auto Carry loop is **testnet-only** — `auto=true` dispatches refuse in live |
| Same-venue spot/perp basis | Yes where one adapter exposes both exact books | Yes | Via `live_entry_enabled` gate — spot legs submit without `reduce_only` (the venue itself refuses oversells); if a venue still rejects the flatten the residual escalates `unresolved` for manual resolution | Exact per-leg flatten; same min-APR exit supervisor (spot legs contribute 0 rate) | Manual opt-in (off by default); autonomous entries testnet-only |
| Cross-venue spot arbitrage | No usable scanner | No | **Disabled** | Not implemented | Roadmap |
| Single-venue triangular | Yes on exact supported books | Yes | Via `live_entry_enabled` gate — three sequential legs on one venue; each priced leg must clear the per-leg notional cap | Residual flatten | Opt-in (off by default) |
| Cross-ticker | Research/quote path only | No | **Disabled** | Not implemented | Roadmap |
| Prediction parity | Research scan when both configured adapters return real books | No | **Disabled** | Not implemented | Research only |
| Statistical arbitrage | Analysis tools | No integrated dispatcher | **Disabled** | Not implemented | Research only |
| External-position close | Authenticated account read | N/A | User-directed risk-reducing submission (works in both environments — testnet submits real demo-venue orders) | Working orders are continuously polled (5s) and restart-recovered by the reconciliation supervisor; unresolved orders escalate after 15 min | Monitor to terminal state; confirm at the venue |
| Emergency flatten | Attributed position read | Yes | User-directed risk-reducing submission only | Same continuous reconciliation; venue still authoritative | Monitor to terminal state; confirm at the venue |

## Venue adapter capability

“Unit” means deterministic parser/signing tests. “Public” means live public-read evidence recorded in the handoff. “Sandbox order” and “mainnet order” require credentialed evidence and are not implied by source code.

*"Strategy live entry" = the `live_entry_enabled` opt-in gate (per-leg notional cap required, refused never clamped). Spatial, Spread Radar manual, funding carry/basis, and triangle entries all dispatch through it; autonomous entries (Radar bot, Funding Auto Carry loop) remain testnet-only — `auto=true` dispatches refuse in live regardless of the opt-in; cross-venue spot and prediction parity have no live dispatch.*

| Venue | Markets / books | Funding | Account read | Sign + submit | Cancel | Leverage update | Terminal/restart reconciliation | Verification ceiling in repo | Strategy live entry |
|---|---|---|---|---|---|---|---|---|---|
| Hyperliquid | Perpetual; spot suffix refused | Yes | Yes | Yes | Yes | No | Order-status poll + restart supervisor | Unit + public reads | **Canary-gated** (`live_entry_enabled` + cap) |
| Lighter | Perpetual and exact `-SPOT` resolution | Yes | Yes | Yes, vendored signer | Yes | No | `accountOrders` poll by client order index + restart supervisor | Unit + public reads | **Canary-gated** (`live_entry_enabled` + cap) |
| Robinhood Lighter | Lighter deployment | Yes | Yes | Yes, same signer family | Yes | No | Delegates to Lighter polling | Unit + public reads | **Canary-gated** (`live_entry_enabled` + cap) |
| Vanta / Orderly | Perpetual | Yes | Yes | Yes | Yes | No explicit implementation | `GET /v1/order/{id}` poll + restart supervisor | Unit + public reads | **Canary-gated** (`live_entry_enabled` + cap) |
| Aster | Perpetual | Yes | Yes | Yes | Yes | Yes | `GET /fapi/v3/order` poll + restart supervisor | Unit + public reads | **Canary-gated** (`live_entry_enabled` + cap) |
| PopDEX | Perpetual | Yes | Yes | Yes, agent-signed on-chain tx | Yes | Yes | Open + `history/orders` poll + restart supervisor | Unit + public reads; no documented sandbox order proof | **Canary-gated** (`live_entry_enabled` + cap) |
| Binance | USDT-M perpetual | Yes | Yes | Yes | Yes | Yes | `GET /fapi/v1/order` poll + restart supervisor | Unit + public reads | **Canary-gated** (`live_entry_enabled` + cap) |
| OKX | Perpetual swaps | Yes | Yes | Yes | Yes | Yes | `GET /api/v5/trade/order` poll + restart supervisor | Unit + public reads | **Canary-gated** (`live_entry_enabled` + cap) |
| Coinbase | Spot | No perpetual funding | Yes | Yes | Yes | N/A; reduce-only unsupported | `GET /orders/{id}` poll + restart supervisor | Unit + public reads | **Canary-gated** (`live_entry_enabled` + cap) |
| Kraken | Linear futures in current adapter | Yes | Yes | Yes | Yes | No explicit implementation | `openorders` + `/fills` aggregation poll + restart supervisor | Unit + public reads | **Canary-gated** (`live_entry_enabled` + cap) |
| Decibel | Markets/prices; REST depth unavailable upstream | Yes through prices endpoint | Yes | Yes, Aptos transaction | Yes | No explicit implementation | Commit-event decode + tx-hash poll + post-commit subaccount-tx scan (last 50 txs, identity-matched OrderEvent/TradeEvent; anonymous fills unattributed, window truncation fails closed) | Unit + selected public reads | **Canary-gated** (`live_entry_enabled` + cap) |
| Arcus | Adapter scaffold, not admitted | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | None | Scaffold only | **No** |
| Polymarket | Separate prediction research adapter | N/A | Not an admitted portfolio venue | No integrated execution | No integrated execution | N/A | None | Research parser | **No** |
| Kalshi | Separate prediction research adapter | N/A | Not an admitted portfolio venue | No integrated execution | No integrated execution | N/A | None | Research parser | **No** |
| Pacifica (DexPerpAdapter) | Perpetual | Yes | Yes (Ed25519 session) | Yes, Ed25519 signed ops | Yes | Yes | `orders` read + restart supervisor | Unit + live public reads; signing implemented, no credentialed order evidence yet | **No — pending venue evidence** |
| StandX (DexPerpAdapter) | Perpetual | Yes | Yes (JWT session) | Yes, Ed25519 signed bodies | Yes (async; WS confirm) | Yes | `query_order` poll + restart supervisor | Unit + live public reads; no credentialed order evidence yet | **No — pending venue evidence** |
| Ondo Perps (DexPerpAdapter) | Perpetual | Yes | Yes (SIWE → JWT) | Yes, wallet-signed | Yes | No explicit implementation | `GET /v1/perps/orders` poll + restart supervisor | Unit + live public reads; no credentialed order evidence yet | **No — pending venue evidence** |
| GRVT (DexPerpAdapter) | Perpetual | Yes | Yes (`gravity` cookie session) | Yes, EIP-712 legs | Yes | No explicit implementation | Order read poll + restart supervisor | Unit + live public reads; no credentialed order evidence yet | **No — pending venue evidence** |
| Derive (DexPerpAdapter) | Perpetual (top-of-book only) | Yes | Yes (`X-Lyra*` headers) | Yes, EIP-712 action sigs | Yes (`private/cancel`) | No explicit implementation | Order read poll + restart supervisor | Unit + live public reads; no credentialed order evidence yet | **No — pending venue evidence** |
| Aevo (DexPerpAdapter) | Perpetual | Yes | Yes (`AEVO-KEY`/`AEVO-SECRET`) | Yes, EIP-712 `Order` | Yes | No explicit implementation | Order read poll + restart supervisor | Unit + live public reads; no credentialed order evidence yet | **No — pending venue evidence** |
| dYdX, Paradex, GMX V2, ApeX Omni, Extended, Helix, edgeX, QFEX (markets only), Vest, Antarctic, EVEDEX, BULK, Ostium, SoDEX, AlphaX, Nado, IO Trader, Niza, Astros, Gate DEX (DexPerpAdapter, market-data-only) | Verified or doc-shaped public reads; GMX V2 is prices-only (GLV AMM has no CLOB), Derive top-of-book only, QFEX books auth-gated → fail closed | Yes where the venue publishes it | **No — fails closed** | **No — fails closed** | **No — fails closed** | **No — fails closed** | None | Unit + live public reads where marked verified; doc-shaped venues degrade to per-entry errors | **No** |
| Uniswap V3, Uniswap V2, PancakeSwap V2, PancakeSwap V3, SushiSwap, QuickSwap (EvmAmmAdapter, spot AMM — standard routers) | On-chain quoter/factory reads — probe-verified live; one-level book synthesized from real executable quotes (bid = exact-in 1 base, ask = exact-out cost of 1 base; both sides required, crossed/empty books fail) | N/A — spot pools | Yes — local EVM wallet account read | Yes — wallet-signed `swapExact*`/`exact*Single` txs; exact-in sells and exact-out buys; limit orders map to min-out/max-in bounds, post-only refused | Atomic swap — nothing rests to cancel | Spot pools — N/A | Submit returns tx hash as `New` immediately; `eth_getTransactionReceipt` polling is the authority — Filled/Rejected require BOTH input-spent and output-received `Transfer` proof; mined gas reported in native fee; restart reconciliation | Unit + live `eth_call` smoke test (`live_market_smoke`, `--ignored`); no credentialed swap evidence yet | **No — pending gas-aware profitability integration and credentialed venue evidence** (`live_strategy_entry_implemented` = false) |
| Camelot V2, Aerodrome, Velodrome, Ramses (legacy), Pharaoh (legacy) (EvmAmmAdapter, spot AMM — market data only) | Live public market data — probe-verified factory/quoter reads; one-level book with real quoted depth (ask via reverse exact-in) | N/A — spot pools | Optional address-only account read — no private key requested or loaded | **No** — Camelot router uses a custom referrer ABI; Solidly routers lack exact-output buys | **No** | Spot pools — N/A | **No** — no order submission, cancel, or status polling | Unit + live `eth_call` smoke test (`live_market_smoke`, `--ignored`) | **No** |

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
5. **New strategy entries are gated, not blanket-blocked** — spatial two-leg, Spread Radar manual, funding carry/basis, and single-venue triangle paths dispatch live orders when the operator enables `live_entry_enabled` AND sets `live_entry_max_notional_usd` (per-leg refusal, never clamped). Cross-venue spot, prediction parity, and the autonomous Radar bot do not dispatch live. Every accepted non-final leg lands in `unresolved`/`imbalanced` reconciliation with its venue order id persisted; an operator acknowledge control resolves escalated signals after manual venue review. Residual auto-flatten submits reduce-only closes only where the venue supports the flag (`ExchangeAdapter::supports_reduce_only`) — spot instruments flatten via plain market orders since the venue itself refuses oversells.
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
- Operator opt-in: implemented — `live_entry_enabled` + `live_entry_max_notional_usd` gate the spatial entry path (vault unlocked, per-venue submission+polling+credentials, per-leg notional refusal — never clamped); a Safety-tab canary panel submits one minimal pair with a bounded loss acceptance
- Sandbox tests for success, partial fill, one-leg failure, timeout, cancel race, and process restart
- Small-notional owner-approved live canary — IN PROGRESS
- Updated capability row and release notes
