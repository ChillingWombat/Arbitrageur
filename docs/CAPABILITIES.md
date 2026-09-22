# Capability Matrix

**Status date:** 2026-09-20
**Authority:** This file and `README.md` define current runtime capability. Historical handoffs describe what was attempted at the time and are not capability guarantees.

## Capability vocabulary

- **Read:** The adapter can request and parse the named public/authenticated data. Individual endpoints can still be unavailable and must fail closed.
- **Submit:** The adapter can sign and transmit an order to the venue. This does **not** prove that the order filled.
- **Reconcile:** The engine can follow an accepted order through terminal fill/cancel/expiry, recover after restart, and attribute partial fills without guessing.
- **Paper strategy:** The strategy executes against authentic venue books through a local simulated account.
- **Live strategy entry:** The strategy may open real positions automatically or from a strategy button.
- **Risk-reducing submission:** A user-directed reduce-only close can be sent, but an acknowledgement may remain “working” until independently confirmed.

A venue is never “live-entry ready” merely because it has credentials and can submit.

## Strategy capability

| Strategy / path | Authentic scan | Paper execution | Live entry | Exit / reconciliation | Current operator meaning |
|---|---|---|---|---|---|
| Cross-venue perpetual spatial | Yes | Yes | **Opt-in canary** (`live_entry_enabled` + per-leg notional cap; off by default) | Live orders poll to terminal state and restart-recover; opt-in auto residual-flatten + bounded cancel retry exist, pending venue evidence | Canary entries only — keep the cap minimal |
| Spread Radar manual entry | Yes | Yes | **Disabled** | Paper bot-managed exits | Paper only |
| Spread Radar autonomous bot | Yes | Yes, opt-in | **Disabled** | Paper bot-managed exits with audit | Paper only |
| Perp/perp funding carry | Yes where both funding and books exist | Yes | **Disabled** | Manual paper flatten; no automated live lifecycle | Paper only |
| Same-venue spot/perp basis | Yes where one adapter exposes both exact books | Yes after the leg-identity remediation | **Disabled** | Exact per-leg paper flatten | Paper only |
| Cross-venue spot arbitrage | No usable scanner | No | **Disabled** | Not implemented | Roadmap |
| Single-venue triangular | Yes on exact supported books | Yes | **Disabled** | Residuals use paper flatten | Paper only |
| Cross-ticker | Research/quote path only | No | **Disabled** | Not implemented | Roadmap |
| Prediction parity | Research scan when both configured adapters return real books | No | **Disabled** | Not implemented | Research only |
| Statistical arbitrage | Analysis tools | No integrated dispatcher | **Disabled** | Not implemented | Research only |
| External-position close | Authenticated account read | N/A | User-directed risk-reducing submission only | Working orders are continuously polled (5s) and restart-recovered by the reconciliation supervisor; unresolved orders escalate after 15 min; synthetic `sim-*` paper ids are never polled | Monitor to terminal state; confirm at the venue |
| Emergency flatten | Attributed position read | Yes | User-directed risk-reducing submission only | Same continuous reconciliation; venue still authoritative | Monitor to terminal state; confirm at the venue |

## Venue adapter capability

“Unit” means deterministic parser/signing tests. “Public” means live public-read evidence recorded in the handoff. “Sandbox order” and “mainnet order” require credentialed evidence and are not implied by source code.

| Venue | Markets / books | Funding | Account read | Sign + submit | Cancel | Leverage update | Terminal/restart reconciliation | Verification ceiling in repo | Strategy live entry |

*"Strategy live entry" = the `live_entry_enabled` spatial canary path only (per-leg notional cap required); triangle/funding/spot entries remain refused regardless.*
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

## Live mode

Live mode currently means:

1. The credential vault is unlocked.
2. Authenticated venue account reads can be shown.
3. Explicit risk-reducing close submissions can be requested where an adapter supports them.
4. Live position reads are matched through adapter-owned account-position normalization (for example `BTCUSDT`, `PF_XBTUSD`, `BTC-USDT-SWAP`, `PERP_BTC_USDC`, `BTC-PERP`, and Lighter market IDs). Unknown or ambiguous mappings leave the position unresolved rather than treated as flat. Quantities and prices are kept in venue-native contract units (e.g. `1000PEPE-USDT` positions stay denominated in `1000PEPE` units with per-unit prices — no ×1000 rescaling in either direction).
5. **New strategy entries are gated, not blanket-blocked** — the spatial two-leg path dispatches live orders when the operator enables `live_entry_enabled` AND sets `live_entry_max_notional_usd` (per-leg refusal, never clamped). All other strategy paths (triangle, funding carry, spot/monitor) refuse live dispatch. Every accepted non-final leg lands in `unresolved`/`imbalanced` reconciliation with its venue order id persisted; an operator acknowledge control resolves escalated signals after manual venue review.
6. **Market data stays websocket-primary in live mode** — the feed supervisor no longer suspends WS lanes when live; orderbooks flow via WS into `MarketDataHub` and REST is fallback-only (2s freshness bound enforced at execution). The monitor keeps running as the hub's REST feeder for venues without a WS lane and as the funding refresher; its own execution stays paper-only. Engine pause still suspends both.

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
