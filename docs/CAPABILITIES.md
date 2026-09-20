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
| Cross-venue perpetual spatial | Yes | Yes | **Disabled** | Paper convergence/stop/timeout; live terminal reconciliation incomplete | Use Paper Trading for entries |
| Spread Radar manual entry | Yes | Yes | **Disabled** | Paper bot-managed exits | Paper only |
| Spread Radar autonomous bot | Yes | Yes, opt-in | **Disabled** | Paper bot-managed exits with audit | Paper only |
| Perp/perp funding carry | Yes where both funding and books exist | Yes | **Disabled** | Manual paper flatten; no automated live lifecycle | Paper only |
| Same-venue spot/perp basis | Yes where one adapter exposes both exact books | Yes after the leg-identity remediation | **Disabled** | Exact per-leg paper flatten | Paper only |
| Cross-venue spot arbitrage | No usable scanner | No | **Disabled** | Not implemented | Roadmap |
| Single-venue triangular | Yes on exact supported books | Yes | **Disabled** | Residuals use paper flatten | Paper only |
| Cross-ticker | Research/quote path only | No | **Disabled** | Not implemented | Roadmap |
| Prediction parity | Research scan when both configured adapters return real books | No | **Disabled** | Not implemented | Research only |
| Statistical arbitrage | Analysis tools | No integrated dispatcher | **Disabled** | Not implemented | Research only |
| External-position close | Authenticated account read | N/A | User-directed risk-reducing submission only | Working orders are not yet continuously reconciled | Confirm terminal state at the venue |
| Emergency flatten | Attributed position read | Yes | User-directed risk-reducing submission only | Live acknowledgements require venue confirmation | Confirm terminal state at the venue |

## Venue adapter capability

“Unit” means deterministic parser/signing tests. “Public” means live public-read evidence recorded in the handoff. “Sandbox order” and “mainnet order” require credentialed evidence and are not implied by source code.

| Venue | Markets / books | Funding | Account read | Sign + submit | Cancel | Leverage update | Terminal/restart reconciliation | Verification ceiling in repo | Strategy live entry |
|---|---|---|---|---|---|---|---|---|---|
| Hyperliquid | Perpetual; spot suffix refused | Yes | Yes | Yes | Yes | No | No durable worker | Unit + public reads | **No** |
| Lighter | Perpetual and exact `-SPOT` resolution | Yes | Yes | Yes, vendored signer | Yes | No | Submission acknowledgement only | Unit + public reads | **No** |
| Robinhood Lighter | Lighter deployment | Yes | Yes | Yes, same signer family | Yes | No | Submission acknowledgement only | Unit + public reads | **No** |
| Vanta / Orderly | Perpetual | Yes | Yes | Yes | Yes | No explicit implementation | Submission acknowledgement only | Unit + public reads | **No** |
| Aster | Perpetual | Yes | Yes | Yes | Yes | Yes | Immediate response only; no restart worker | Unit + public reads | **No** |
| PopDEX | Perpetual | Yes | Yes | Yes, agent-signed on-chain tx | Yes | Yes | Transaction/order acknowledgement only | Unit + public reads; no documented sandbox order proof | **No** |
| Binance | USDT-M perpetual | Yes | Yes | Yes | Yes | Yes | No durable fill worker | Unit + public reads | **No** |
| OKX | Perpetual swaps | Yes | Yes | Yes | Yes | Yes | Short synchronous poll only; no restart worker | Unit + public reads | **No** |
| Coinbase | Spot | No perpetual funding | Yes | Yes | Yes | N/A; reduce-only unsupported | Submission acknowledgement only | Unit + public reads | **No** |
| Kraken | Linear futures in current adapter | Yes | Yes | Yes | Yes | No explicit implementation | Submission acknowledgement only | Unit + public reads | **No** |
| Decibel | Markets/prices; REST depth unavailable upstream | Yes through prices endpoint | Yes | Yes, Aptos transaction | Yes | No explicit implementation | Transaction acknowledgement only | Unit + selected public reads | **No** |
| Arcus | Adapter scaffold, not admitted | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | None | Scaffold only | **No** |
| Polymarket | Separate prediction research adapter | N/A | Not an admitted portfolio venue | No integrated execution | No integrated execution | N/A | None | Research parser | **No** |
| Kalshi | Separate prediction research adapter | N/A | Not an admitted portfolio venue | No integrated execution | No integrated execution | N/A | None | Research parser | **No** |

## Live mode

Live mode currently means:

1. The credential vault is unlocked.
2. Authenticated venue account reads can be shown.
3. Explicit risk-reducing close submissions can be requested where an adapter supports them.
4. Live position reads are matched through adapter-owned account-position normalization (for example `BTCUSDT`, `PF_XBTUSD`, `BTC-USDT-SWAP`, `PERP_BTC_USDC`, `BTC-PERP`, and Lighter market IDs). Unknown or ambiguous mappings leave the position unresolved rather than treated as flat. Quantities and prices are kept in venue-native contract units (e.g. `1000PEPE-USDT` positions stay denominated in `1000PEPE` units with per-unit prices — no ×1000 rescaling in either direction).
5. **New strategy entries remain blocked.**

Before enabling Live mode:

- Use minimum-permission API keys without withdrawal rights.
- Verify venue account identity and network.
- Confirm every submitted close at the venue until reconciliation is implemented.
- Keep a direct venue login and manual close/withdraw path available.

## Promotion requirements for live strategy entry

A venue/strategy pair can move to live entry only after all of these have evidence:

- Exact market and quantity conversion
- Idempotent client-order identity
- Terminal fill, partial-fill, reject, cancel, and expiry handling
- Reduce-only compensation
- Leverage/margin-mode confirmation or an explicit no-change invariant
- Durable accepted-order record before submission
- Restart recovery and reconciliation
- Sandbox tests for success, partial fill, one-leg failure, timeout, cancel race, and process restart
- Small-notional owner-approved live canary
- Updated capability row and release notes
