# R2: 'Stop All' Full Emergency Kill & Flatten Overhaul — Comprehensive Survey Report

## Executive Summary
This report presents a thorough investigation of the current "Stop All" action across the UI, desktop backend commands, trading coordinator, autonomous strategy bots, reconciliation engine, and Dev RPC (`/tmp/arb-dev.sock`).

**Current Status:** The existing "Stop All" action in `frontend/src/components/Header.tsx` and `frontend/src-tauri/src/execution.rs::trigger_emergency_killswitch_impl` is **purely a circuit-breaker trip**. It does **not** cancel resting venue orders, does **not** pause the trading engine, does **not** flatten open positions, does **not** poll fills, and does **not** verify venue flat state. In fact, the confirmation modal in `Header.tsx` explicitly warns:
> *"This trips the risk circuit breaker and halts new trading coordination. It does not flatten positions or cancel resting venue orders; use explicit position close actions and verify state on the venue."*

This violates the Phase 1 Acceptance Criteria of the original user prompt:
> *"Clicking 'Stop All' trips circuit breaker, cancels resting orders on all active venues, and flattens all open positions down to zero or documented untradeable residual. System status confirms flat positions and tripped circuit breaker via `/tmp/arb-dev.sock` dev RPC."*

This survey maps out the existing code paths, identifies architectural gaps, uncovers subtle race conditions (notably double-closing and position-flipping hazards on unhedged excess), and specifies the complete end-to-end architecture required to upgrade "Stop All" into an immediate, fail-closed emergency shutdown pipeline.

---

## 1. Current State Inventory

### 1.1 UI & Frontend Components
1. **Header Killswitch Button (`frontend/src/components/Header.tsx:99-106`)**:
   - Red button labelled "Stop All" with `OctagonX` icon.
   - Opens `showKillModal` state.
   - Confirmation modal lines 123–125:
     ```tsx
     <p className="text-sm text-slate-600 leading-relaxed">
       This trips the risk circuit breaker and halts new trading coordination. It does not flatten positions or cancel resting venue orders; use explicit position close actions and verify state on the venue.
     </p>
     ```
   - On confirmation, calls prop `onEmergencyKillswitch()`.
2. **App Coordinator (`frontend/src/App.tsx:664-671`)**:
   - `handleEmergencyKillswitch` calls `api.triggerKillswitch()` followed by `refreshAllData()`.
3. **Frontend API Bridge (`frontend/src/services/api.ts:845-847`)**:
   - `triggerKillswitch(): Promise<string>` calls Tauri desktop command `invokeDesktop<string>("trigger_emergency_killswitch")`.
4. **Position-Level Flatten Controls (`frontend/src/components/PositionsPanel.tsx`)**:
   - Right-click context menu on individual position cards offers "Emergency Clear" (market) or "Clear Position" (limit).
   - Calls `api.flattenTradePositions(pos.tradeId, pos.strategyId, mode)`.
   - Clears only **one specific trade/symbol at a time**.
5. **External Positions Controls (`frontend/src/components/ExternalPositionsGadget.tsx`)**:
   - Lists external non-system positions.
   - Offers "Close" button calling `api.closeExternalPosition(...)`.
   - Clears only **one venue position at a time**.

### 1.2 Backend Commands & Tauri IPC
1. **Command Registration (`frontend/src-tauri/src/main.rs:250`)**:
   - Registers `commands::trigger_emergency_killswitch`.
2. **Tauri Handler (`frontend/src-tauri/src/execution.rs:5457-5461`)**:
   ```rust
   #[tauri::command]
   pub async fn trigger_emergency_killswitch(
       state: tauri::State<'_, Arc<AppEngineState>>,
   ) -> Result<String, String> {
       trigger_emergency_killswitch_impl(&state)
   }
   ```
3. **Core Implementation (`frontend/src-tauri/src/execution.rs:3535-3540`)**:
   ```rust
   pub fn trigger_emergency_killswitch_impl(state: &AppEngineState) -> Result<String, String> {
       state
           .circuit_breaker
           .trip("EMERGENCY KILLSWITCH ENGAGED BY OPERATOR");
       Ok("EMERGENCY KILLSWITCH ENGAGED. All trading operations halted immediately.".into())
   }
   ```
   *Analysis:* Completely synchronous, zero interaction with adapters, orders, or positions.

### 1.3 Dev RPC Socket (`/tmp/arb-dev.sock`)
1. **Socket Dispatcher (`frontend/src-tauri/src/dev_rpc.rs:255`)**:
   - `"killswitch" => crate::execution::trigger_emergency_killswitch_impl(st).map(|v| json!(v))`
   - Exactly wraps `trigger_emergency_killswitch_impl`.
2. **Status Commands (`frontend/src-tauri/src/dev_rpc.rs`)**:
   - `"system_status"`: returns `SystemStatusDto` with `circuit_breaker_tripped: true`, but `is_paused` remains unchanged.
   - `"open_positions"`: still returns all active system positions after killswitch!
   - `"external_positions"`: still returns all external positions after killswitch!

### 1.4 Existing Flatten Infrastructure
1. **Signal-Level Flatten (`frontend/src-tauri/src/execution.rs:4597-5235` — `flatten_positions_inner`)**:
   - Takes `strategy_id: &str`.
   - Queries `list_signals(symbol, None)`.
   - Filters roots: `s.strategy_id.eq_ignore_ascii_case(&sid) && s.status.counts_as_open()`.
   - Submits `flat-{sig_id}-{rand}` child signal with market or limit orders.
   - **Limitation:** Only operates on a single `strategy_id` per invocation.
   - **Limitation:** Dispatches leg unwinds **serially** in a `for t in &targets { match flatten_leg(...) }` loop.
   - **Limitation:** Does not poll fills on async-ack venues; relies on single subsequent `adapter.fetch_account().await`. If account hasn't settled yet, marks signal `Unresolved`.
2. **External Position Close (`frontend/src-tauri/src/external_positions.rs:679-990` — `close_external_position_impl`)**:
   - Takes single `CloseExternalPositionInput { venue, symbol, mode, ... }`.
   - Re-reads single venue account.
   - Submits `extclose-{venue}-{ms}-{rand}` signal and single order.
   - **Limitation:** Single venue and symbol only.
3. **Atomic Unwind Handler (`crates/arbitrageur-execution/src/emergency_unwind.rs`)**:
   - Used by `AtomicArbitrageCoordinator` during execution rollback when Leg B fails.
   - Has built-in bounded polling:
     ```rust
     const UNWIND_POLL_ATTEMPTS: u32 = 10;
     const UNWIND_POLL_INTERVAL: Duration = Duration::from_millis(300);
     ```
   - Verifies fill before considering unwind complete.

---

## 2. Identified Gaps, Critical Hazards, and Race Conditions

### 2.1 Critical Hazard: Double-Closing and Position Inversion on Shared Instruments
- **Observation:** `execution.rs` (lines 4880–4920) implements a "Venue-excess sweep":
  If an engine leg has a ledger residual of `0.01` BTC, but the live venue position is `0.05` BTC (e.g., due to an external trade or untracked fill), and no other signal claims the venue+symbol, `flatten_positions_inner` closes the **entire venue position** (`0.05` BTC).
- **Hazard:** If an emergency shutdown simultaneously dispatches an engine flatten AND an external position close on the same `(venue, symbol)`:
  - Engine flatten places a market SELL for `0.05` BTC.
  - External flattener calculates `external_qty = 0.05 - 0.01 = 0.04` BTC and places a market SELL for `0.04` BTC.
  - Total market orders submitted: `0.09` BTC SELL!
  - **Outcome:** The account is inverted from `+0.05` BTC Long to `-0.04` BTC Short! On spot venues or venues lacking native `reduce_only` enforcement, this creates an unhedged catastrophic naked short exposure.
- **Fail-Closed Resolution:**
  A single coordinated emergency flatten orchestrator must partition positions cleanly by `(venue, canonical_base)`:
  1. For any `(venue, canonical_base)` where engine-tracked signals exist, the engine flatten path claims and unwinds the full venue exposure.
  2. The external flatten path is **strictly restricted** to venue positions that have **zero open engine signals** on that `(venue, canonical_base)`.
  3. All closing orders must be submitted with `reduce_only = true` (where supported by venue/contract).

### 2.2 Critical Gap: Resting Order Cancellation Before Market Unwind
- **Observation:** Currently, `reconciliation.rs` cancels orders only after `NON_FINAL_DEADLINE_SECS` (15 minutes), one order at a time sequentially.
- **Hazard:** If an emergency market unwind order is dispatched while a resting limit order (entry, peg, or grid step) remains working on the book:
  - If a resting BUY order fills immediately after a market SELL unwind, the account immediately re-opens a position!
  - If a resting limit SELL order was working to exit, placing a market SELL order causes a double fill and position inversion.
- **Requirement:** **Resting order cancellation must execute FIRST and concurrently across all connected venues before dispatching market unwind orders.** Once cancellations are acknowledged and confirmed dead, market unwind orders can safely execute.

### 2.3 Critical Gap: Lack of Fill Polling on Async-Ack Venues
- **Observation:** As observed in Issue #56 and `emergency_unwind.rs`, DEX perps and L2 venues (Lighter, Decibel, Aster, Arcus, PopDEX) acknowledge market orders with status `New` or `PartiallyFilled`, with fill records populated milliseconds or seconds later on-chain.
- **Hazard:** `flatten_positions_inner` currently checks `adapter.fetch_account().await` immediately after `place_order`. If the account cache or on-chain state hasn't updated within those milliseconds, the position still shows open, and the engine flags the signal as `Unresolved` and gives up!
- **Requirement:** Implement bounded `fetch_order` polling (10 attempts @ 300ms) with `enrich_fill_stats` on all dispatched market unwind orders, followed by fresh `fetch_account()` re-reads to confirm flat status.

### 2.4 Accounting for Untradeable Residuals (Dust & Minimum Lot Sizes)
- **Observation:** Not all positions can be reduced to exact zero on live venues:
  - Minimum notional limits (e.g. Binance $5 min order size, Lighter min notional).
  - Lot size step increments (e.g. Deribit $10 contract increments, Alpaca share roundings).
  - In `arbitrageur-core/src/exchange.rs:180`, every adapter implements `wire_base_qty(&self, symbol: &str, qty: Decimal) -> Result<Decimal>`. If `qty < step_size`, `wire_base_qty` returns `0`.
- **Requirement:** When `wire_base_qty` returns zero or residual notional is `< $1.00`, the system cannot trade the remainder on-wire without exchange rejection.
- **Fail-Closed Resolution:** Accurately classify and document these remnants as `UntradeableResidualDto { venue, symbol, quantity, notional_usd, reason: "sub_min_wire_lot" }`. The system is in a verified safe state when all tradable exposure is flat and all remnants are documented dust.

### 2.5 Multi-Strategy Engine Coverage
- **Observation:** `flatten_positions_inner` takes a single `strategy_id`. In live operation, open positions can span:
  - `futures` (spatial arbitrage / basis carry)
  - `radar` (spread radar autonomous bot)
  - `triangle` (triangular arbitrage)
  - `options_parity` / `options_packages`
  - `statarb`
- **Requirement:** The emergency shutdown orchestrator must query `list_signals(None, None)` to aggregate and unwind roots across **all strategies**, not just one.

---

## 3. Target Architecture: Fail-Closed Emergency Shutdown Pipeline

```
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                        'STOP ALL' EMERGENCY TRIGGER                                   │
│            (Header UI Button [confirmed] OR Dev RPC {"cmd":"stop_all"})               │
└───────────────────────────────────┬────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────────────────────┐
│ PHASE 1: IMMEDIATE FAIL-CLOSED FREEZE                                                 │
│ • state.circuit_breaker.trip("EMERGENCY STOP ALL ENGAGED BY OPERATOR")                 │
│ • *state.is_paused.write() = true                                                      │
│ • state.state_store.kv_set("engine.is_paused", "true")                                  │
│ • Freeze autonomous bot loops (radar_bot, options_bot, monitor)                        │
│ • Acquire state.execution_lock                                                         │
└───────────────────────────────────┬────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────────────────────┐
│ PHASE 2: CONCURRENT RESTING ORDER CANCELLATION                                         │
│ • Query state_store.list_pending_order_legs(Some(state.env().as_str()))                 │
│ • Query open orders from all connected, authenticated venue adapters                   │
│ • Discard duplicates across order_id / client_order_id                                 │
│ • Concurrently dispatch cancel_order() across all active venues via tokio/join_all     │
│ • Settle / poll cancellation confirmations (short bounded poll)                        │
└───────────────────────────────────┬────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────────────────────┐
│ PHASE 3: POSITION INVENTORY & DEDUPLICATION                                            │
│ • Query all open root signals across all strategies (futures, radar, triangle, etc.)   │
│ • Query all external positions across connected venue adapters (fetch_account)         │
│ • Map each position by (venue, canonical_base)                                         │
│ • Dedup: If engine signal exists on (venue, base), engine unwind claims full exposure; │
│   external close only claims non-engine instruments                                    │
└───────────────────────────────────┬────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────────────────────┐
│ PHASE 4: CONCURRENT EMERGENCY MARKET UNWIND DISPATCH                                   │
│ • For each target position, snap closable size to wire_base_qty                        │
│ • Filter out untradeable dust (shortfall < min_step -> log as untradeable residual)    │
│ • Pre-persist client_order_id and close signal (flat-... / extclose-...)               │
│ • Set reduce_only = true                                                               │
│ • Concurrently dispatch market orders across disjoint (venue, symbol) lanes            │
└───────────────────────────────────┬────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────────────────────┐
│ PHASE 5: FILL POLLING & VENUE FLAT CONFIRMATION                                        │
│ • Poll fetch_order on each dispatched order (up to 10 attempts @ 300ms) until final    │
│ • Enrich fill stats (avg_fill_price, fee) & persist fills to signal_legs               │
│ • Perform fresh venue account re-read (fetch_account) across all connected venues      │
│ • Verify venue positions:                                                              │
│   - position == 0 -> Confirmed Flat                                                    │
│   - position <= min_lot / < $1 dust -> Documented Untradeable Residual                 │
│   - position > min_lot -> Flag as Unresolved Residual & alert operator                 │
└───────────────────────────────────┬────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────────────────────┐
│ PHASE 6: RETURN STRUCTURED EMERGENCY SHUTDOWN REPORT                                   │
│ • Return EmergencyShutdownReportDto to caller (UI / Dev RPC)                           │
│ • System leaves in verifiable safe state:                                              │
│   tripped circuit breaker, paused engine, 0 resting orders, verified flat / dust       │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 4. Detailed Specification of Implementation Components

### 4.1 Backend Data Structures (`frontend/src-tauri/src/execution.rs`)
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UntradeableResidualDto {
    pub venue: String,
    pub symbol: String,
    pub side: String,
    pub quantity: f64,
    pub notional_usd: f64,
    pub reason: String, // "sub_wire_lot_step" | "dust_under_1_usd" | "zero_orderbook_liquidity"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmergencyShutdownReportDto {
    pub success: bool,
    pub circuit_breaker_tripped: bool,
    pub engine_paused: bool,
    pub resting_orders_cancelled: usize,
    pub cancel_failures: Vec<String>,
    pub engine_signals_processed: usize,
    pub engine_signals_closed: usize,
    pub external_positions_closed: usize,
    pub untradeable_residuals: Vec<UntradeableResidualDto>,
    pub unresolved_positions: Vec<String>,
    pub safe_state_verified: bool,
    pub timestamp: String,
    pub notes: Vec<String>,
}
```

### 4.2 Core Backend Engine Function: `execute_emergency_stop_all_impl`
Location: `frontend/src-tauri/src/execution.rs` (callable by both Tauri IPC and Dev RPC):
```rust
pub async fn execute_emergency_stop_all_impl(
    state: &AppEngineState,
) -> Result<EmergencyShutdownReportDto, String>
```
1. **Trip & Freeze:**
   - Call `state.circuit_breaker.trip("EMERGENCY STOP ALL ENGAGED BY OPERATOR")`.
   - Set `*state.is_paused.write() = true`.
   - Persist kv `engine.is_paused = "true"`.
   - Acquire `_exec_guard = state.execution_lock.lock().await`.
2. **Resting Order Cancellation:**
   - Gather pending legs via `state.state_store.list_pending_order_legs(Some(state.env().as_str()))`.
   - Gather open orders from adapters where supported.
   - Run concurrent `cancel_order` with `futures::future::join_all`.
   - Short sleep (~400ms) and bounded check.
3. **Position Aggregation & Partitioning:**
   - Query all open root signals across all strategies from `state.state_store.list_signals(None, None)`.
   - Query external positions via `get_external_positions_impl(state)`.
   - Partition: any `(venue, base)` claimed by an engine signal is closed exclusively through the engine flatten path. Unclaimed external positions are closed via `close_external_position_impl`.
4. **Concurrent Market Unwind:**
   - Concurrently execute engine signal flattens and external position closes using `join_all`.
   - Market orders dispatched with `reduce_only = true`.
   - Bounded polling via `fetch_order` until final.
5. **Venue Truth Reconciliation:**
   - Re-read all venue accounts via `fetch_account()`.
   - Calculate residuals. If `wire_base_qty` <= 0 or notional < $1, record in `untradeable_residuals`.
   - Verify `safe_state_verified = unresolved_positions.is_empty()`.

### 4.3 Dev RPC Wiring (`frontend/src-tauri/src/dev_rpc.rs`)
In `dev_rpc.rs`:
- Map both `"killswitch"` and `"stop_all"` to `crate::execution::execute_emergency_stop_all_impl(st).await.map(|v| json!(v))`.
- Keep existing commands (`system_status`, `open_positions`, `external_positions`) so verification scripts can inspect:
  - `system_status`: verifies `circuit_breaker_tripped: true`, `is_paused: true`.
  - `open_positions`: verifies empty list.
  - `external_positions`: verifies empty or dust-only list.

### 4.4 UI Updates (`frontend/src/components/Header.tsx` & `App.tsx`)
1. **Header Confirmation Modal (`Header.tsx`)**:
   - Update title: `Emergency Kill & Flatten: Stop All Trading`
   - Update explanation text:
     ```tsx
     <p className="text-sm text-slate-600 leading-relaxed">
       This immediately trips the circuit breaker, freezes trading coordinator dispatch, cancels all resting orders across all connected venues, and executes emergency market unwinds to flatten all open positions (both system-tracked and external).
     </p>
     ```
   - Update button label: `Confirm Stop All & Flatten`
2. **Execution Feedback**:
   - Show loading/busy state while the stop-all pipeline executes.
   - On completion, trigger `refreshAllData()` and display notification summary (e.g. `Emergency stop complete: X orders cancelled, Y positions flattened`).

---

## 5. Summary Matrix: Existing vs Upgraded Path

| Feature / Behavior | Current Implementation | Upgraded R2 Implementation |
|---|---|---|
| **Circuit Breaker** | Tripped synchronously via `trip()` | Tripped immediately, durable snapshot saved |
| **Engine Trading Pause** | Untouched (`is_paused` stays false) | Atomically paused (`is_paused = true`), persisted |
| **Autonomous Bots** | Keep running in background | Frozen immediately |
| **Resting Venue Orders** | Left untouched on orderbooks | Concurrently cancelled across all active venues |
| **Engine-Tracked Positions** | Left open | Concurrently flattened via market reduce-only orders |
| **External Venue Positions** | Left open | Concurrently flattened via market reduce-only orders |
| **Multi-Strategy Coverage** | Single strategy (`strategy_id`) only | Comprehensive across all strategies and instruments |
| **Double-Close Hazard** | Unprotected (can flip positions) | Partitioned by `(venue, base)` — zero collision risk |
| **Fill Polling** | None (instant single account read) | Bounded `fetch_order` polling (~3s) + fill enrichment |
| **Dust / Residual Handling** | Treated as failure / Unresolved | Accurately classified as documented untradeable residual |
| **Safe State Verification** | None | Full post-unwind venue account re-read & audit report |
| **Dev RPC Command** | `"killswitch"` trips breaker only | `"killswitch"` / `"stop_all"` runs full flatten pipeline |
| **UI Header Modal** | Warns that it does NOT flatten | Confirms full emergency kill & flatten action |

---

## 6. Verification and Test Strategy
1. **Unit & Integration Tests in Desktop Backend (`cargo test -p arbitrageur-desktop`)**:
   - Test `execute_emergency_stop_all_impl` against simulated/test venue adapters:
     - Verify circuit breaker is tripped.
     - Verify engine pause flag is set.
     - Verify resting limit orders are cancelled.
     - Verify open positions across multiple strategies (`futures`, `radar`, `triangle`) are flattened.
     - Verify external positions are flattened without double-close.
     - Verify sub-step residuals are classified as `UntradeableResidualDto`.
     - Verify `safe_state_verified` returns true.
   - Ensure existing tests (e.g. `test_risk_overview_and_killswitch`) pass with zero regression.
2. **Frontend Test Suite (`cd frontend && npm test`)**:
   - Verify `Header.tsx` modal renders new text and triggers API call.
   - Verify `api.ts` maps `triggerKillswitch` and parses new DTO structure.
3. **Live Testnet Dev RPC Verification (`/tmp/arb-dev.sock`)**:
   - Testnet launch with `ARBITRAGEUR_DEV_SOCK=/tmp/arb-dev.sock`.
   - Unlock vault via `unlock_vault` with password `<redacted>`.
   - Verify `{"cmd":"stop_all"}` or `{"cmd":"killswitch"}` executes.
   - Inspect `system_status`: confirms `circuit_breaker_tripped: true` and `is_paused: true`.
   - Inspect `open_positions`: confirms zero open positions.
   - Inspect `external_positions`: confirms zero or documented dust.
