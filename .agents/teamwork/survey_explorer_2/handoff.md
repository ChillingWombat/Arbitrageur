# Handoff Report — Survey Explorer 2: R2 'Stop All' Full Emergency Kill & Flatten Overhaul

## 1. Observation

1. **Current UI Stop All Button & Modal**
   - File: `/mnt/Data/Projects/Arbitrageur/frontend/src/components/Header.tsx`, lines 99–125:
     - Button `onClick={() => setShowKillModal(true)}`, text `<span>Stop All</span>`.
     - Modal explanation lines 123–125 verbatim:
       > *"This trips the risk circuit breaker and halts new trading coordination. It does not flatten positions or cancel resting venue orders; use explicit position close actions and verify state on the venue."*
     - Confirmation button lines 134–137 calls `onEmergencyKillswitch()`.
   - File: `/mnt/Data/Projects/Arbitrageur/frontend/src/App.tsx`, lines 664–671:
     - `handleEmergencyKillswitch` calls `await api.triggerKillswitch()` then `await refreshAllData()`.
   - File: `/mnt/Data/Projects/Arbitrageur/frontend/src/services/api.ts`, lines 845–847:
     - `triggerKillswitch()` invokes desktop command `"trigger_emergency_killswitch"`.

2. **Current Backend Killswitch Handler**
   - File: `/mnt/Data/Projects/Arbitrageur/frontend/src-tauri/src/execution.rs`, lines 3535–3540:
     ```rust
     pub fn trigger_emergency_killswitch_impl(state: &AppEngineState) -> Result<String, String> {
         state
             .circuit_breaker
             .trip("EMERGENCY KILLSWITCH ENGAGED BY OPERATOR");
         Ok("EMERGENCY KILLSWITCH ENGAGED. All trading operations halted immediately.".into())
     }
     ```
   - Lines 5457–5461:
     ```rust
     #[tauri::command]
     pub async fn trigger_emergency_killswitch(
         state: tauri::State<'_, Arc<AppEngineState>>,
     ) -> Result<String, String> {
         trigger_emergency_killswitch_impl(&state)
     }
     ```
   - File: `/mnt/Data/Projects/Arbitrageur/frontend/src-tauri/src/dev_rpc.rs`, line 255:
     ```rust
     "killswitch" => crate::execution::trigger_emergency_killswitch_impl(st).map(|v| json!(v)),
     ```
   - Observations:
     - Does **not** set `*state.is_paused.write() = true`.
     - Does **not** cancel any resting orders on any exchange.
     - Does **not** flatten any open positions (internal or external).
     - Does **not** poll fills or verify flat state.

3. **Current Flatten Implementations**
   - File: `/mnt/Data/Projects/Arbitrageur/frontend/src-tauri/src/execution.rs`, lines 4597–5235 (`flatten_positions_inner`):
     - Takes a single `strategy_id: &str`.
     - Flattens root signals for that one strategy only.
     - Loops over targets serially (`for t in &targets { match flatten_leg(...) }`).
     - Contains a "Venue-excess sweep" (lines 4880–4920): if the venue position exceeds the ledger residual and the symbol is exclusive, it closes the entire venue quantity.
     - Line 5158: calls `adapter.fetch_account().await` once without polling; if not yet indexed, marks signal `Unresolved`.
   - File: `/mnt/Data/Projects/Arbitrageur/frontend/src-tauri/src/external_positions.rs`, lines 679–990 (`close_external_position_impl`):
     - Takes single `CloseExternalPositionInput { venue, symbol, ... }`.
     - Subtracts `system_qty` from `live_qty` to get `external_qty`.
     - Dispatches one market or limit order for that single position.

4. **Resting Order Tracking**
   - File: `/mnt/Data/Projects/Arbitrageur/frontend/src-tauri/src/state_store/signals.rs`, lines 549–582:
     - `list_pending_order_legs(Some(state.env().as_str()))` queries legs from `signal_executions` where `status IN ('in_flight', 'imbalanced', 'unresolved')` or recent settle-gap fills with non-empty `venue_order_id` or `client_order_id`.
   - File: `/mnt/Data/Projects/Arbitrageur/frontend/src-tauri/src/commands.rs`, lines 4786–4828:
     - `cancel_venue_order` cancels a single order by `order_id` and `client_order_id` via `adapter.cancel_order(...)`.
   - There is currently **no concurrent bulk cancel mechanism** across all connected venues.

5. **Untradeable Residuals (Dust & Step Limits)**
   - File: `/mnt/Data/Projects/Arbitrageur/crates/arbitrageur-core/src/exchange.rs`, line 180:
     - `wire_base_qty(&self, symbol: &str, qty: Decimal) -> Result<Decimal>` is implemented by all adapters to snap quantity to valid lot increments. If `qty < min_lot`, `wire_base_qty` returns zero.
   - File: `/mnt/Data/Projects/Arbitrageur/frontend/src-tauri/src/external_positions.rs`, line 31:
     - `const DUST_NOTIONAL_USD: f64 = 1.0;` filters positions under $1.
   - File: `/mnt/Data/Projects/Arbitrageur/frontend/src-tauri/src/reconciliation.rs`, lines 529–536:
     - If `shortfall > Decimal::ZERO` and `wire_base_qty == 0`, classifies as sub-wire complete.

---

## 2. Logic Chain

1. **Safety Deficit Analysis:**
   - From (1) and (2), when an operator clicks "Stop All" or sends `{"cmd":"killswitch"}` via Dev RPC, the system only trips `CircuitBreaker`.
   - All open positions across all connected venues (Hyperliquid, Binance, OKX, Lighter, Deribit, etc.) remain open and subject to market risk, margin calls, or liquidations.
   - All resting orders on orderbooks remain live; if market prices move, resting orders will fill, increasing risk despite the killswitch.
   - Therefore, the current "Stop All" violates the Phase 1 Acceptance Criteria and must be overhauled into a full emergency kill & flatten operation.

2. **Order of Operations (Fail-Closed Prerequisite Chain):**
   - Step 1: **Trip & Freeze Immediately.** Tripping the circuit breaker (`circuit_breaker.trip`) and setting `is_paused = true` instantly prevents new entries from `AtomicArbitrageCoordinator`, `OpportunityExecutor` (radar bot), `options_bot`, `triangle.rs`, and manual UI submissions.
   - Step 2: **Concurrently Cancel All Resting Orders FIRST.** If market unwind orders are placed before resting orders are cancelled, a resting limit order filling seconds later will immediately re-open unwanted exposure, or a resting exit order will double-fill and flip the position. Thus, resting order cancellation must precede position unwinds.
   - Step 3: **Partition and Dedup Unwind Targets.** From (3), `flatten_positions_inner` closes venue excess when exclusive. If external position closing runs simultaneously on the same `(venue, base)`, both will submit market orders, causing a double-close and flipping the position into a naked short. Hence, targets must be strictly partitioned: engine signals claim their `(venue, base)`, while external position closes only target unmanaged `(venue, base)` pairs.
   - Step 4: **Concurrently Dispatch Emergency Market Unwinds.** Market orders with `reduce_only = true` must be dispatched concurrently across disjoint lanes. Client order IDs and child signals must be persisted before dispatch to guarantee post-crash recoverability.
   - Step 5: **Bounded Fill Polling & Flat Confirmation.** From (3) and (5), async-ack venues require bounded `fetch_order` polling to verify terminal fill. Once fills settle, fresh `fetch_account()` calls must verify each venue position is zero or within documented untradeable dust limits (`wire_base_qty == 0` or notional < $1).
   - Step 6: **Verifiable Safe State Output.** Return a structured `EmergencyShutdownReportDto` to the UI and Dev RPC, leaving the circuit breaker tripped and engine paused.

---

## 3. Caveats

1. **Vault State Requirement in Live Mode:**
   - In live trading mode (`state.is_live() == true`), order cancellation and submission on external venues require authenticated credentials loaded from the secure vault. If the vault is locked and credentials have been cleared from memory, venue API calls cannot authenticate. In testnet mode or when credentials remain loaded (e.g. auto-locked state), calls proceed normally.
2. **Venue Rate Limits during Concurrent Cancellation:**
   - Certain venues (e.g., Orderly / Vanta or Aster) enforce strict rate limits on order cancellations. Concurrently dispatching dozens of cancels must route through adapter rate limiters to avoid HTTP 429 rejections.
3. **Severe Orderbook Illiquidity:**
   - If an exchange has an empty orderbook on a specific contract (e.g., an illiquid altcoin or deep OTM option), a market close order may be rejected by the venue. Such positions must be surfaced in `unresolved_positions` in the final report rather than assumed flat.

---

## 4. Conclusion

The "Stop All" overhaul requires:
1. Adding a new core engine function: `execute_emergency_stop_all_impl(state: &AppEngineState) -> Result<EmergencyShutdownReportDto, String>`.
2. Wiring this function to:
   - Tauri command `trigger_emergency_killswitch` (invoked by `Header.tsx`).
   - Dev RPC commands `"killswitch"` and `"stop_all"` on `/tmp/arb-dev.sock`.
3. Updating `Header.tsx` modal text and confirmation button to accurately reflect the full emergency shutdown and flattening action.
4. Ensuring concurrent cancellation of all resting orders from `state_store.list_pending_order_legs` and venue order queries.
5. Implementing concurrent emergency market unwinds across all strategies (`futures`, `radar`, `triangle`, `options_parity`, `options_packages`, `statarb`) and external venue positions, with clean deduplication to prevent double-closing.
6. Implementing bounded `fetch_order` fill polling and venue account re-reads, accurately classifying sub-step remnants into `untradeable_residuals`.

---

## 5. Verification Method

1. **Desktop Backend Test Suite:**
   ```bash
   cargo test -p arbitrageur-desktop --bin arbitrageur-desktop
   ```
   Add new unit and integration tests verifying:
   - `execute_emergency_stop_all_impl` trips circuit breaker and pauses engine.
   - Resting order cancellation across simulated adapters.
   - Concurrent flattening of multiple open positions across different strategies.
   - Clean partition preventing double-closing of external positions.
   - Correct classification of sub-step / dust remnants as `untradeable_residuals`.
   - Invalidation condition: any regression in existing 588 tests or failure to confirm flat state.

2. **Frontend Test Suite:**
   ```bash
   cd /mnt/Data/Projects/Arbitrageur/frontend && npm test
   ```
   Verify all frontend components, modals, and tests pass without errors.

3. **Dev RPC Integration Test (`/tmp/arb-dev.sock`):**
   ```bash
   echo '{"cmd":"stop_all"}' | socat - UNIX-CONNECT:/tmp/arb-dev.sock
   echo '{"cmd":"system_status"}' | socat - UNIX-CONNECT:/tmp/arb-dev.sock
   echo '{"cmd":"open_positions"}' | socat - UNIX-CONNECT:/tmp/arb-dev.sock
   echo '{"cmd":"external_positions"}' | socat - UNIX-CONNECT:/tmp/arb-dev.sock
   ```
   Verification criteria:
   - `system_status` returns `"circuit_breaker_tripped": true` and `"is_paused": true`.
   - `open_positions` returns `[]`.
   - `external_positions` returns `[]` or documented dust only.
