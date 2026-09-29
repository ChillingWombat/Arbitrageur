# Specification: Arbitrageur Emergency Killswitch, UI Harmonization, and Bot Profitability Overhaul

## Problem Statement

As a quantitative trader operating an algorithmic multi-venue arbitrage desk, I faced three critical operational and financial hazards:
1. **Incomplete Emergency Shutdown**: Triggering "Stop All" halted automated signal generation but failed to cancel resting orders and left live positions active across external venues. In volatile or adverse market conditions, unhedged positions remained exposed to market risk, risking capital destruction.
2. **Inconsistent User Interface**: UI controls, toggle switches, modal dialogs, and dashboard telemetry gadgets featured divergent visual styles, inconsistent padding and borders, duplicate switch implementations with incompatible props, and varying backdrop blur layers, leading to operator friction and accidental misconfiguration during live trading.
3. **Severe Bot Profitability Degradation & Accounting Distortions**: Automated strategies (Radar Convergence, Options Parity, Spatial Arbitrage) suffered consistent negative net returns on testnet. Options parity conversion trades erroneously booked upfront options financing premiums as instant realized trading losses, distorting PnL telemetry by tens of thousands of dollars. Furthermore, razor-thin hurdle rate defaults allowed trades to execute where round-trip crossing spreads and exchange taker fees completely overwhelmed gross edge.

## Solution

A comprehensive multi-subsystem overhaul delivering:
1. **Immediate, Fail-Closed 'Stop All' Emergency Shutdown**: An atomic, fail-closed killswitch that instantly trips the risk circuit breaker, freezes coordinator trading dispatch, concurrently cancels active resting orders across all connected venues, concurrently unwinds all engine-managed and external positions with canonical venue alias deduplication, and isolates sub-lot/sub-dollar remnants into untradeable residuals while enforcing fail-closed vault access guards.
2. **Harmonized UI Component & Modal Design**: A unified toggle switch component architecture supporting standard visual and semantic variants, a backward-compatible alias layer for legacy imports, standardized gadget container styling tokens, and unified modal backdrop containers that maintain cryptographic security blur guarantees.
3. **Quantitative Bot Execution & Profitability Corrections**:
   - Accurate options parity PnL accounting that books only realized transaction fees on entry while tracking asset financing cash flows in a dedicated outlay accumulator until expiration/settlement.
   - Calibrated hurdle rates requiring net expected edge to exceed crossing spreads and taker fee tiers before trade admission.
   - Verified convergence exit models preventing premature liquidation prior to statistical equilibrium.

## User Stories

1. As an operator, I want clicking "Stop All" to immediately trip the risk circuit breaker, so that no new automated trading signals can be dispatched.
2. As an operator, I want clicking "Stop All" to freeze the coordinator loop, so that in-flight evaluations cease immediately.
3. As an operator, I want clicking "Stop All" to cancel all active resting orders across all connected venues concurrently, so that resting limit exposure is eliminated without waiting on serial API round trips.
4. As an operator, I want clicking "Stop All" to discover and unwind engine-managed positions using reduce-only market orders, so that open arbitrage pairs are collapsed safely.
5. As an operator, I want clicking "Stop All" to discover and unwind external venue positions, so that discretionary or out-of-band positions are also flattened during an emergency.
6. As an operator, I want the emergency shutdown to deduplicate positions between engine ledgers and venue accounts using canonical venue identifiers, so that positions are not double-closed or reversed into opposite exposure.
7. As an operator, I want sub-dollar or sub-lot position remnants to be classified as untradeable residuals with clear audit explanations, so that sub-minimum exchange order rejections do not stall the shutdown process.
8. As an operator, I want the emergency shutdown to fail closed with an explicit error if the vault is locked in live mode, so that unauthenticated operations cannot silently fail to cancel live exposure.
9. As an operator, I want the emergency shutdown to return a structured report indicating order cancellation counts, position closure counts, untradeable residuals, and any unresolvable venue positions, so that I have complete situational awareness.
10. As an automated watchdog or monitoring script, I want to trigger the emergency shutdown via a local Unix domain socket RPC command (`stop_all` or `killswitch`), so that headless headless automation can halt operations during anomalies.
11. As an operator, I want the UI header to display an emergency shutdown summary modal following a "Stop All" invocation, so that I can inspect the results and acknowledge the halted state.
12. As a frontend developer, I want a single standardized `ToggleSwitch` component with consistent props (`checked`, `onChange`, `label`, `size`, `variant`, `disabled`), so that switches across all settings and gadgets behave identically.
13. As a frontend developer, I want `SwitchToggle` to remain available as a backward-compatible alias of `ToggleSwitch`, so that legacy imports do not cause build regressions.
14. As an operator, I want toggle controls to provide visual distinction between general settings (`accent` blue) and safety-critical execution toggles (`emerald` green), so that high-risk switches are easily identifiable.
15. As an operator, I want all dashboard gadget cards to share identical borders, background surface elevation, padding, and corner radiuses, so that the dashboard presents a cohesive, professional workspace.
16. As an operator, I want all modal dialogs to use standardized backdrop overlays and card elevation, so that dialog focus and dismissal behaviors are predictable.
17. As a security reviewer, I want the vault unlock modal to maintain high-strength cryptographic background blur, so that sensitive wallet and credential data cannot be viewed behind transparent overlays.
18. As an operator, I want dashboard telemetry to display honest em dashes for unavailable metrics rather than fabricated zeros, so that missing market feeds are never mistaken for zero-risk flat values.
19. As a quantitative trader, I want options conversion and reversal trades to record true realized trading fees on entry rather than gross options premium cash flow, so that execution logs and net profit metrics reflect genuine trading performance.
20. As a quantitative trader, I want options premium cash outlays to be tracked in a dedicated outlay accumulator, so that asset financing capital is accounted for separately from trading PnL.
21. As an algorithmic trader, I want the default futures hurdle rate to be calibrated to at least 12.0 bps, so that bots do not execute trades where orderbook crossing spreads and exchange taker fees exceed gross opportunity edge.
22. As an algorithmic trader, I want route execution gating to enforce minimum net profit hurdles net of taker fees, so that non-profitable routes are rejected before reaching exchange wire adapters.
23. As an algorithmic trader, I want route execution gating to enforce a maximum spread divergence ceiling, so that corrupted orderbook feeds or mismatched ticker symbols are rejected as toxic quotes.
24. As an algorithmic trader, I want radar convergence bot positions to exit only when spread deviation reverts to statistical equilibrium, so that positions are not prematurely liquidated while still inside the friction band.
25. As an algorithmic trader, I want testnet operations to authenticate using master password `000000`, so that automated testnet verification runs smoothly without manual intervention.

## Implementation Decisions

### 1. Emergency Shutdown Core & Dev RPC Interface
- An emergency shutdown workflow was established in the execution engine to orchestrate four sequential safety stages:
  1. Circuit breaker trip and coordinator pause flag engagement (`is_paused = true`).
  2. Concurrent cancellation of all active resting venue orders.
  3. Concurrent unwind of open positions across engine-managed and external venue positions, with deduplication using canonical venue identifiers.
  4. Isolation of sub-lot/sub-dollar balances into untradeable residual records.
- In live trading mode, the emergency sequence verifies vault unlock state before dispatching wire cancels; if locked, it fails closed with an explicit error to prevent unauthenticated execution drift.
- The Dev RPC socket exposes `stop_all` and `killswitch` commands returning the following report schema:

```json
{
  "circuitBreakerTripped": true,
  "enginePaused": true,
  "cancelledOrdersCount": 4,
  "cancelledOrdersFailed": 0,
  "closedEnginePositions": 2,
  "closedExternalPositions": 0,
  "untradeableResiduals": [],
  "unresolvedPositions": [],
  "timestampMs": 1727610000000
}
```

### 2. UI Component Standardization & Design Tokens
- Consolidated all toggle switches into a unified component interface supporting `size` (`sm` | `md`) and `variant` (`accent` | `emerald`), accompanied by an alias re-export preserving backward compatibility for legacy switch imports.
- Replaced raw HTML checkbox inputs and inline button toggles across settings and estimator panels with the unified toggle component.
- Standardized gadget card container styling tokens to use elevated surface backgrounds, tokenized surface borders, rounded card corners, and uniform padding.
- Standardized modal backdrop containers to enforce centralized z-index management, click-outside dismissal, and ESC key handling, while preserving mandatory security blur styling in credential dialogs.

### 3. Quantitative Bot Accounting & Hurdle Calibration
- In options put-call parity conversions, the opening trade entry logs `net_profit = -fees` instead of deducting the upfront options premium outlay. Upfront premium financing is routed to a dedicated outlay accumulator until expiry/settlement.
- Calibrated default strategy hurdle rates (`target_profit_bps`) from 1.0 to 12.0 bps in strategy configuration defaults and execution fallback logic to guarantee trades clear crossing spreads and exchange fee schedules.
- Verified convergence exit condition logic to ensure mean-reversion trades hold until spread deviation converges toward equilibrium rather than exiting prematurely upon reaching the entry friction threshold.

## Testing Decisions

### What Makes a Good Test
- Tests must verify external behavioral contracts, state transitions, and safety invariants rather than private internal implementation details.
- Safety tests must assert fail-closed behaviors under fault conditions (e.g. locked vault, missing orderbook depth, corrupt quotes, venue timeouts).
- Numerical tests must assert exact decimal bounds and fee deductions without floating-point precision loss.

### Testing Seams & Prior Art
1. **Dev RPC Socket Seam**:
   - The primary test seam for emergency shutdown and system control. External test clients dispatch JSON commands over `/tmp/arb-dev.sock` and assert the resulting system state and reports.
   - *Prior Art*: System status and open position verification tests in `dev_rpc.rs`.
2. **Backend Execution & Risk Seam**:
   - Unit and integration tests in the desktop and strategy crates verifying circuit breaker trips, route economics computation, hurdle enforcement, and options parity logging.
   - *Prior Art*: Existing test suites in `execution.rs`, `options_parity.rs`, `state_store.rs`, and `convergence_auto.rs`.
3. **Frontend Component & Interaction Seam**:
   - Node-based component tests asserting rendering, accessibility attributes, variant classes, event dispatching, and security blur properties.
   - *Prior Art*: `test-components.mjs` running via Node test runner.
4. **4-Tier End-to-End Test Suite**:
   - Automated test runner covering Tier 1 (feature coverage), Tier 2 (boundary & corner cases), Tier 3 (cross-feature interactions), and Tier 4 (real-world application scenarios).

## Out of Scope
- Modifying underlying venue exchange API signature algorithms (e.g. Deribit, Lighter, Hyperliquid, OKX).
- Redesigning database schema versions beyond existing v38/v39 migrations.
- Live mainnet capital deployment (all execution verification conducted in testnet environment using testnet credentials and vault password `000000`).

## Further Notes
- All verification checks across Rust backend (`cargo test`), strategy engine (`arbitrageur-strategy`), frontend unit suites (`npm test`), 4-tier E2E tests (`runner.mjs`), and static linters (`cargo clippy`, `cargo fmt`) pass with 100% success rate and zero regressions.
