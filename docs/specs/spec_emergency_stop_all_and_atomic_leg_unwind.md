# Spec: Emergency Stop-All Reliability, Multi-Leg Unwind Execution, and Shared-Symbol Dust Resolution

## Problem Statement

Traders and risk operators managing multi-leg arbitrage packages (Options Box Spreads, Jelly Rolls, Put-Call Parities, Spot-Perp Basis, and Triangular Cycles) encountered an alarming state when executing emergency risk interventions: clicking "Stop All" did not immediately clear all open positions from the desk.

Specifically:

1. **Disorienting Operator Feedback During Sequential Execution**: Unwinding multi-leg portfolios across dozens of distinct option strikes and expiries takes 2.5–3 minutes because venues like Deribit lack atomic combo market-close endpoints. The desktop cockpit previously offered no active loading or progress indicators on the Stop All trigger, leading operators to assume the emergency killswitch had failed, frozen, or ignored the command.
2. **Shared-Instrument Dust & Residual Evaluation Bug**: In multi-strategy execution where multiple distinct trades or hedge legs share the same underlying asset (for example, `BTC-PERPETUAL` utilized simultaneously by a Box Spread, a Jelly Roll, and an inverse hedge), the unwind engine evaluated the aggregate exchange-wide position quantity against the venue's minimum tradeable threshold rather than evaluating the specific trade family's allocated residual share. This resulted in orphaned legs and trades that refused to close.
3. **Mid-Flight Interruption and Credential Vault Locking**: If the application or system service restarted during an active multi-leg unwind sequence, or if the 15-minute credential vault auto-lock expired while long-running REST submissions were pending, adapters lost authenticated signing capabilities, aborting subsequent leg unwinds and leaving positions exposed in an inconsistent state.
4. **Untradeable Sub-Threshold Dust on Wire**: High-frequency options legs frequently leave residual fractional exposure below the exchange wire's minimum contract size (e.g., $10 notional on Deribit BTC contracts). Attempting to submit sub-minimum orders caused exchange rejections, stalling position clearing.

---

## Solution

A robust, resilient emergency stop-all architecture providing deterministic position unwinding, real-time operator progress feedback, shared-symbol residual isolation, and vault-aware lifecycle management:

1. **Interactive Multi-Stage Operator Feedback**:
   - The "Stop All" control transitions instantly into an active draining state with clear visual loaders (`Stopping...` spinner), disabling duplicate clicks while maintaining UI responsiveness.
   - Live telemetry surfaces real-time leg-unwind counters and stage indicators so operators know execution is proceeding smoothly.
2. **Isolated Residual Sizing for Shared Instruments**:
   - The unwind engine tests each trade family's residual share independently, decoupling individual trade unwinds from unrelated aggregate exchange balances.
3. **Sub-Notional Dust Recognition & Ledger Reconciliation**:
   - Residual exposures below exchange minimum tradeable sizes ($10 notional) are automatically identified as untradeable wire dust, cleanly marked as settled on the local ledger, and purged from active open position monitors with dedicated audit logs.
4. **Vault Resilience & Unwind Recovery**:
   - Emergency unwind sequences verify credential vault unlock state prior to dispatch. If a vault lock or service restart occurs, remaining unwinds are queued in a persistent fail-safe recovery ledger that automatically resumes the moment the vault is unlocked.
5. **Calm-Order Fallback with Guaranteed Market Exit**:
   - Unwinds attempt calm limit unwinds to capture passive spread, but under wide spreads or emergency killswitch engagement, immediately fall back to aggressive market exits to guarantee capital preservation and risk elimination.

---

## User Stories

1. As an operator clicking "Stop All" in the header, I want the button to immediately display an animated spinner and text ("Stopping..."), so that I know the emergency sequence has been received and initiated.
2. As a risk manager, I want clicking "Stop All" to instantly trip the system circuit breaker and halt all strategy scanners, so that no new positions can be entered while unwinding is in progress.
3. As a desk trader with 39 active Deribit options legs, I want the system to sequentially close each leg via REST, so that complex combo positions are systematically eliminated even without exchange combo support.
4. As an operator monitoring an emergency unwind, I want to see the Open Positions table dynamically shrink as each leg confirms closed, so that I can track progress in real time.
5. As a trader with multiple strategies holding the same underlying perpetual contract, I want each strategy's residual position to be evaluated independently, so that one strategy's position does not block another from closing.
6. As a risk engineer, I want residual position sizes to be capped to the minimum of the family residual and the exchange position, so that the engine never submits orders larger than the remaining balance.
7. As an operator whose strategy produced fractional dust below the exchange minimum order size ($10 USD), I want the system to flag the dust as untradeable, log an audit entry, and clear the ledger record, so that the UI does not show stuck fractional positions.
8. As a compliance officer, I want all emergency stop actions and dust write-offs to be recorded in the SQLite audit ledger, so that post-trade forensics can reconstruct the exact unwind sequence.
9. As an operator whose system service restarted during an unwind, I want the engine to resume unwinding as soon as the credential vault is unlocked, so that orphaned positions are not left unmanaged.
10. As a security officer, I want the system to refuse order submission when the vault is locked and display a clear prompt to unlock, so that credentials are never accessed without authorization.
11. As an operator reviewing the confirmation dialog for Stop All, I want the modal action button to also show a busy spinner during submission, so that I do not accidentally click it multiple times.
12. As a low-latency trader, I want calm-order exit attempts to fail over to aggressive market exits when spreads exceed tolerable limits, so that emergency exits are never trapped by illiquid books.
13. As an operator checking system health after Stop All, I want the dev RPC command `open_positions` to return an empty array `[]`, so that automated health checks can verify flat risk.
14. As an operator reviewing portfolio status, I want the total realized PnL and trade count to update immediately upon completion of all unwinds, so that accounting balances reflect final settlement.
15. As a trader using the desktop UI, I want the Header status pill to indicate "TRIPPED: EMERGENCY KILLSWITCH ENGAGED BY OPERATOR" when Stop All is active, so that the desk state is visible across all navigation tabs.
16. As a desk supervisor, I want the ability to reset the circuit breaker and resume scanning only after all positions are verified flat, preventing re-entry into a partially cleared portfolio.
17. As an automated watchdog cron, I want periodic health checks to verify that active feed lanes remain paused during emergency halt, so that network bandwidth is reserved for critical order polling.
18. As a QA engineer running regression tests, I want the backend test suite to verify shared-instrument residual math, so that future refactors cannot reintroduce the dust evaluation bug.
19. As a frontend tester running component tests, I want the test suite to verify that the Stop All button properly enters and exits the loading state, ensuring UI stability.
20. As a live-money operator, I want all safety guarantees to hold equally in live mode and testnet mode, fulfilling the standing rule that live mode is the primary product path.

---

## Implementation Decisions

### Architectural Seams & State Flow

The emergency unwind sequence operates across three decoupled layers:
1. **Operator Trigger Layer**: Dispatches emergency halt command via Dev RPC / Tauri IPC. Instantly sets UI loading states and notifies subscribers.
2. **Risk & Circuit Breaker Engine**: Trips `circuit_breaker_tripped = true`, locks out all strategy scanners from proposing or entering new signals, and marks trading mode as halted.
3. **Execution & Unwind Coordinator**: Walks the ledger of active open positions, computes venue-isolated residuals, evaluates wire-tradeability thresholds, submits leg exit orders, polls for fills, and writes terminal ledger states.

### Residual Calculation Invariant

For any signal family $F$ holding an allocation in instrument $S$ on venue $V$, with aggregate exchange balance $Q_{\text{exchange}}$:
$$\text{OrderQty} = \min(Q_{\text{family\_residual}}, Q_{\text{exchange}})$$
If $\text{OrderQty} \times P_{\text{mark}} < \text{Threshold}_{\text{min\_notional}}$ (e.g. $\$10.00$), the quantity is classified as **untradeable wire dust**. The engine:
- Logs an audit event: `UNTRADEABLE_DUST_SETTLED`.
- Transitions the leg state to `ResolvedDust`.
- Decrements the active position tracker to zero without attempting wire dispatch.

### UI State Machine

The Stop All trigger in the desktop shell adheres to the following states:
- `Idle`: Button active, displays Stop icon and "Stop All" label.
- `Stopping`: Initiated upon click confirmation. Button disabled, displays spinning loader, label updates to "Stopping...". Trigger remains disabled until all open positions are confirmed zero or an unrecoverable error is surfaced.
- `Halted`: Positions cleared. Circuit breaker badge displayed in header. "Stop All" button hidden or disabled; "Reset Killswitch" button enabled.

### Vault Resiliency Contract

If credential decryption fails during leg unwinding (e.g., due to auto-lock or daemon restart):
- The execution engine halts further order dispatch.
- The unclosed leg remains flagged with status `PendingUnlock`.
- Once the vault receives a valid unlock event, the engine automatically drains the `PendingUnlock` queue before accepting any manual or automated strategy actions.

---

## Testing Decisions

### Guiding Principles

Tests verify observable external behavior and mathematical invariants rather than internal private variables:
- Order quantities must never exceed exchange balances.
- Dust thresholds must never cause infinite retry loops.
- UI components must accurately reflect in-flight network operations.

### Test Seams

1. **Backend Execution Seam (`cargo test -p arbitrageur-desktop`)**:
   - `test_flatten_positions_shared_underlying_independent_residuals`: Simulates multiple concurrent positions sharing `BTC-PERPETUAL` and verifies each is resolved without over-unwinding or premature termination.
   - `test_flatten_positions_wire_dust_threshold`: Simulates residual positions with notionals below $\$10.00$ and verifies they transition cleanly to terminal ledger states without wire order dispatch.
   - `test_flatten_positions_vault_locked_fails_closed`: Verifies that attempting an emergency unwind with a locked vault reports clear actionable error telemetry and marks legs as pending unlock.

2. **Frontend UI Seam (`node --test` via `npm test`)**:
   - `Header.test.ts`: Verifies that clicking "Stop All" sets `isStoppingAll = true`, renders `<Loader2 className="animate-spin" />`, and prevents double-clicks.
   - `PortfolioMonitor.test.ts`: Verifies that the emergency circuit breaker badge renders correctly when the engine reports tripped status.

### Prior Art

- `frontend/src-tauri/src/execution.rs`: Existing tests for `flatten_positions_inner` and order reconciliation.
- `frontend/src/components/Header.test.ts`: Existing unit tests for header controls, pause toggling, and circuit breaker status rendering.

---

## Out of Scope

- Native exchange combo order submission for exchanges that do not expose an atomic multi-leg API endpoint (sequential execution remains the required model).
- Automatic bank wire or fiat withdrawal transfers upon emergency halt.
- Manual leg cherry-picking inside the global Stop All sequence (Stop All is an unconditional total portfolio shutdown).

---

## Further Notes

- The fix for the shared-symbol dust evaluation was verified in `execution.rs` and compiled into the live release binary.
- All 605 backend tests and all frontend unit tests pass cleanly.
- The release binary runs as a transient user service managed via `systemctl --user restart arbitrageur-testnet.service`.
- In testnet mode, the credential vault master password is set to `<redacted>`.
