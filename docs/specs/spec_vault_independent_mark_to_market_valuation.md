# Spec: Continuous Public Mark-to-Market Valuation and Resilient Multi-Leg Options Portfolio Pricing Independent of Vault Lock State

## Problem Statement

In the Arbitrageur desktop platform, traders operate multi-leg arbitrage strategies (including 4-leg Box Spreads, 8-leg Jelly Rolls, 3-leg Put-Call Parity, and Convexity structures) alongside spot and perpetual strategies. To ensure operational safety and protect exchange API credentials, the platform features a secure Credential Vault. When the vault is locked ("Vault locked — credentialed venue reads and order execution halted"), all private account polling and trade execution are strictly halted.

However, a critical operational friction existed in position telemetry when the vault was locked:
1. **Asymmetric and Incomplete Position Telemetry**: Some open positions displayed live current Net PnL (e.g. `-$701.35`, `-$376.07`), while others displayed an unhelpful em-dash (`—`), creating uncertainty about whether positions were still being tracked or had suffered data corruption.
2. **Defective Dependency on Authenticated Account Polling**: Position mark-to-market valuation previously relied on private `fetch_account()` polling, which cached positions with a 15-minute time-to-live (`last_good_accounts`). Once this cache expired during a locked-vault session, positions could only get marks if active strategy bots happened to be scanning those exact strikes into the market data hub (120-second TTL).
3. **Multi-Leg Poisoning and Gamma Blindspots**: Once options bots hit their maximum concurrent positions limit (`max_open_positions`), they stopped active scanning on already-filled strikes. For complex multi-leg trades (such as 8-leg Jelly Rolls spanning two future expiries or far-dated Box Spreads like `ETH-26MAR27`), if even a single leg expired from cache, the entire package's unrealized PnL poisoned to null (`None`). Operators were blinded to their real-time portfolio risk and drawdown during the very times when trading execution was locked.

## Solution

Deliver a robust architectural decoupling of private venue authentication from public market valuation, ensuring continuous, reliable, and real-time mark-to-market PnL across all open positions regardless of vault state:
- **Vault-Independent Public Valuation**: Treat mark-to-market pricing as strictly public market data. Valuation routines must never require or wait for an unlocked credential vault.
- **Proactive Public Mark Refresh**: When open derivative positions exist, proactively refresh bulk public book summaries and spot index prices directly from public venue REST endpoints with an automated 15-second cache throttle, bypassing authenticated polling entirely.
- **Deterministic Currency Conversion**: Ensure exchange adapters populate underlying spot and perpetual conversion prices during public index calls so that option premiums quoted in base crypto (BTC, ETH) reliably convert to USD without private account balance dependencies.
- **Unified IPC and Dev RPC Valuation Pipeline**: Eliminate divergent aggregation logic between the desktop graphical user interface (Tauri IPC) and automated developer tools (Unix dev socket RPC), ensuring a single, shared valuation implementation.
- **Preserved Honest Null Discipline**: Maintain strict adherence to honest nulls—if market data is genuinely unreachable due to a venue outage, render an em-dash (`—`) rather than fabricating zero, but ensure that reachable public data always yields continuous, accurate PnL.

## User Stories

1. As a risk-conscious trader, I want to see real-time unrealized Net PnL on all my open positions while my credential vault is locked, so that I can monitor portfolio health and drawdown without exposing my private trading keys to memory.
2. As an options trader holding 4-leg Box Spreads, I want all four legs to be priced simultaneously from fresh public mark data, so that my position PnL does not vanish to an em-dash when scanner bots stop looking for new box spreads.
3. As an options trader holding 8-leg Jelly Rolls across two expiration cycles, I want all eight legs to continuously receive mark-to-market prices, so that multi-month calendar spreads display continuous Net PnL.
4. As a trader running Put-Call Parity synthetic conversions, I want both the synthetic options legs and the perpetual hedge leg to have synchronized mark prices, so that synthetic basis divergence is accurately reflected in real-time PnL.
5. As an operator running in live or testnet mode, I want public market data queries to never send API credentials or signature headers, so that public reads cannot leak secrets or trigger credential rejection errors.
6. As an algorithmic trader, I want public option mark fetches to be rate-limited and throttled (e.g. 15-second cache), so that polling the UI does not trigger exchange rate limit bans or congest the network.
7. As a desktop GUI user, I want the Positions sidebar to update every polling interval with fresh dollar PnL, so that I have immediate feedback on market movements.
8. As an automated headless supervisor connecting via the Unix dev socket, I want `{"cmd":"open_positions"}` to return identical valuations and DTO shapes as the GUI IPC call, so that dev tools and monitoring scripts never diverge from what is rendered on screen.
9. As a crypto derivatives trader, I want option premiums quoted in BTC or ETH to convert to USD using live spot index prices, so that dollar PnL remains accurate even when the underlying crypto asset moves.
10. As a trader facing genuine network outages or venue downtime, I want missing leg prices to produce an honest em-dash (`—`) rather than a fabricated `0.00`, so that I am never deceived into believing an unpriced position is at break-even.
11. As a trader unlocking or locking the vault, I want the transition to be seamless with no flicker or loss of position valuations in the interface.
12. As a systems engineer, I want the venue adapter to record the underlying index price during public book summary queries, so that secondary currency conversion functions always have access to a valid conversion rate.
13. As a developer auditing open positions, I want detailed logs only when a genuine mark miss occurs, rather than recurring log spam for known public instruments.
14. As an institutional operator, I want multi-leg position calculations to aggregate net quantities per leg accurately, ensuring that partial flattens or asymmetric fills still compute honest residual PnL.
15. As a desktop user navigating between strategy views, I want position cards to display correct tone styling (green for profit, red for loss) matching the mathematical sign of the position PnL.

## Implementation Decisions

- **Decoupling Vault State from Market Data Valuation**:
  - The security boundary of the Credential Vault is strictly confined to credential storage, order signing, execution dispatch, and private account operations (balances, deposits, open orders).
  - Public market data (orderbooks, ticker prices, mark prices, and underlying indices) is treated as unauthenticated public telemetry. Valuation pipelines must function identically whether the vault is locked, unlocked, or uninitialized.
- **Proactive Public Mark Ingestion**:
  - In the position aggregation pipeline, before evaluating open position legs, the system inspects active signals for derivatives contracts requiring public mark-to-market data.
  - If open positions exist and the mark cache is stale (>15 seconds), the engine issues bulk public queries to venue summary endpoints (e.g. Deribit `/public/get_book_summary_by_currency?currency=BTC|ETH&kind=option|future`) and spot index endpoints.
  - Ingested book summaries are converted to standard orderbook structures carrying bids, asks, and exchange mark prices, and inserted into the central market data hub.
- **Deterministic Currency and Index Conversion**:
  - Exchange adapters maintain an atomic record of the latest underlying index price (`account_index_used`) populated during public index queries.
  - The currency conversion helper utilizes this index price to convert crypto-denominated option premiums into USD values without requiring authenticated account equity payloads.
- **Consolidation of Open Positions Pipeline**:
  - Extracted the core position aggregation logic into a shared, robust engine implementation function (`get_open_positions_impl`).
  - Both Tauri IPC command handlers (`get_open_positions`) and Dev Socket RPC dispatchers (`open_positions`) call this identical implementation, eliminating disparate aggregation code and behavioral drift.
- **Honest Null Discipline and Atomic Poisoning**:
  - Preserved the core architectural principle that if any leg of a multi-leg trade cannot be priced due to genuine data unavailability, the trade's `unrealizedPnlUsd` remains `None`.
  - Proactive mark ingestion ensures this condition only occurs during genuine network failures or venue outages, preventing false positives during normal operation.

## Testing Decisions

- **Testing Principles**: Tests must verify external observable behavior, API response contracts, and rendered UI states; tests must never depend on mocking transient internal states.
- **Backend Unit & Aggregation Seam**:
  - Prior art: `state_store::tests::test_aggregate_open_positions` in the desktop backend crate.
  - Unit tests verify that:
    - Multi-leg positions (Box Spreads, Jelly Rolls) compute exact dollar PnL when all leg books are present.
    - Missing marks correctly poison only the affected trade to `None` without corrupting neighboring trades.
    - Partial flattens net residual quantities and entry prices accurately.
- **Dev Socket RPC Seam**:
  - Integration verification querying `/tmp/arb-dev.sock` with `{"cmd":"open_positions"}` while the credential vault is locked.
  - Validated that 100% of open positions across all four option substrategies return populated numeric values for `unrealizedPnlUsd`.
- **Frontend Component Seam**:
  - Prior art: `frontend/src/components/PositionsPanel.test.ts` executed via Node.js test runner and Happy-DOM (`npm test`).
  - Validates that:
    - Positions with numeric PnL render formatted currency strings with matching color tones.
    - Positions with null PnL render an honest em-dash (`—`).
    - Options position cards collapse multi-leg structures cleanly without visual clipping.

## Out of Scope

- Authenticated private portfolio metrics while vault is locked (margin utilization, total cash balance, and account collateral require authenticated endpoints and remain gated by design).
- Order execution, automated position unwind, or manual emergency flattening while vault is locked (all actions modifying venue account state require master key authorization).
- Synthetic option Greeks surface calculation or Black-Scholes volatility smile generation.

## Further Notes

- The testnet binary is built with `--features custom-protocol` to embed compiled frontend assets directly into the Tauri user service (`arbitrageur-testnet.service`).
- All 18 live multi-leg testnet positions (encompassing Box Spreads, Jelly Rolls, Convexity, and Put-Call Parity) were verified live via dev RPC with the vault locked, confirming 100% telemetry coverage.
