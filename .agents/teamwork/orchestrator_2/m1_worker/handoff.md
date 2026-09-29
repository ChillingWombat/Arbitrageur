# Hard Handoff Report: Milestone M1 — Bot Lifecycle & Header Controls

## 1. Observation

1. **Backend Pause & Resume Safeguards**:
   - In `frontend/src-tauri/src/commands.rs` line 3178 (`set_engine_paused_impl`), unpausing the engine (`paused == false`) previously did not verify whether `state.circuit_breaker.is_tripped()`. Furthermore, pausing the engine (`paused == true`) did not invoke `crate::execution::stop_all_bots(state)`.
   - Now updated in `frontend/src-tauri/src/commands.rs`:
     ```rust
     pub fn set_engine_paused_impl(paused: bool, state: &AppEngineState) -> Result<bool, String> {
         if !paused && state.circuit_breaker.is_tripped() {
             return Err(
                 "Cannot resume engine while circuit breaker is tripped. Reset circuit breaker first."
                     .into(),
             );
         }
         state
             .state_store
             .kv_set("engine.paused", if paused { "1" } else { "0" })
             .map_err(|e| format!("Failed to persist pause state: {e}"))?;
         *state.is_paused.write() = paused;
         if paused {
             crate::execution::stop_all_bots(state);
         }
         Ok(paused)
     }
     ```

2. **Autonomous Fixed Yield Disarmament**:
   - In `frontend/src-tauri/src/execution.rs` line 3533 (`stop_all_bots`), Radar and Options bots were disarmed, but `state.settings.read().auto_fixed_yield_enabled` was not reset or persisted.
   - Now updated in `frontend/src-tauri/src/execution.rs`:
     ```rust
     // 3. Fixed yield bot: disable auto_fixed_yield_enabled and persist to KV
     state.settings.write().auto_fixed_yield_enabled = false;
     if let Err(e) = state
         .state_store
         .kv_set("engine.auto_fixed_yield_enabled", "0")
     {
         tracing::warn!("Failed to persist auto_fixed_yield_enabled = 0: {e}");
     }
     ```

3. **Arming Rejection when Circuit Breaker is Tripped**:
   - In `frontend/src-tauri/src/auto_exec.rs` line 402 (`OpportunityExecutor::save_config`), arming Radar bot (`mode == BotMode::Auto || dto.enabled`) when `state.circuit_breaker.is_tripped()` is now rejected with: `"Cannot enable or arm Radar bot while circuit breaker is tripped. Reset circuit breaker first."`.
   - In `frontend/src-tauri/src/options_bot.rs` line 243 (`OptionsBotExecutor::save_config`), arming Options bot (`mode == BotMode::Auto || dto.enabled`) when `state.circuit_breaker.is_tripped()` is now rejected with: `"Cannot enable or arm Options bot for '{substrategy}' while circuit breaker is tripped. Reset circuit breaker first."`.

4. **Frontend Lifecycle Hooks**:
   - In `frontend/src/hooks/useBotLifecycle.ts`, exported `registerBotResetListener(listener: () => void): () => void` and `publishBotPhase(strategyId: string, symbol: string | null | undefined, phase: BotPhase): void`. Refactored the internal reset `useEffect` to use `registerBotResetListener`.

5. **Header Tooltips & Interactive Controls**:
   - In `frontend/src/components/Header.tsx`, wrapped the pause/resume `<button>` in `<div className="relative inline-flex" title={...}>` with `pointer-events-none` on the disabled button when `isTripped`.
   - Tooltip messages verified:
     - Tripped: `"Trading halted by circuit breaker — reset in Portfolio Monitor before resuming engine"`
     - Paused: `"Resume Engine: Resume execution coordination. Automated bots remain stopped and must be started individually on each strategy page."`
     - Running: `"Pause Engine: Temporarily suspend executions. Open positions are preserved."`
   - Verified `disabled={isTripped}`. Resuming toggles only the engine pause state; no bots are started.

6. **Application-level Bot Reset Synchronization**:
   - In `frontend/src/App.tsx`:
     - In `handleTogglePause`: when pausing (`applied === true`), invoked `stopAllFrontendBots()`.
     - In `handleResetCircuitBreaker`: verified `stopAllFrontendBots()` is called.
     - In circuit breaker effect: ensured `stopAllFrontendBots()` runs whenever `circuit_breaker_tripped` changes (`systemStatus?.circuit_breaker_tripped !== undefined`).

7. **Tests**:
   - In `frontend/src/components/EngineSettings.test.ts`: removed unused `fileURLToPath` import; verified line 125 uses `__filename`.
   - In `frontend/src/hooks/useBotLifecycle.test.ts`: added test coverage for `publishBotPhase`, `registerBotResetListener` registration/unregistration, and `stopAllFrontendBots`.
   - Created `frontend/src/components/Header.test.ts`: verified tripped tooltip & disabled state, paused tooltip & Resume text, running tooltip & Pause text, and interactive click tests (click toggles pause without starting bots; click when tripped does not toggle pause).
   - In `frontend/src-tauri/src/commands.rs`: added `test_engine_pause_circuit_breaker_and_stop_all_bots` testing pause engine disarms fixed yield and radar bots, unpause rejection while circuit breaker is tripped, and arming rejection while circuit breaker is tripped.

---

## 2. Logic Chain

1. **Safety Invariant**: Under Requirement R1, halting, emergency stop, killswitch, and circuit breaker reset must guarantee that all bots transition to off / stopped / idle, never paused or running.
2. **Backend Engine Pause**: When an operator pauses the engine, automated trading must stop. Adding `crate::execution::stop_all_bots(state)` inside `set_engine_paused_impl` guarantees that pausing the coordinator disarms Radar bots, Options bots, and the autonomous fixed yield loop, persisting `0` / `off` to KV and SQLite.
3. **Breaker Gating on Resume**: An operator must not be able to bypass a tripped circuit breaker simply by clicking "Resume Engine" in the Header. Enforcing `if !paused && state.circuit_breaker.is_tripped() { return Err(...); }` closes this hole.
4. **Arming Gating**: Similarly, an operator or automated script calling `set_radar_bot_config` or `set_options_bot_config` to re-arm bots while the circuit breaker is still tripped must fail closed. Adding this check in `save_config` guarantees that arming cannot succeed until the breaker is explicitly reset in Portfolio Monitor.
5. **Header Tooltip Accessibility**: Native HTML `<button disabled>` suppresses mouse enter/hover events in several browsers. Wrapping the button in a styled container (`<div className="relative inline-flex" title={...}>`) guarantees that the explanatory tooltip remains readable when the button is disabled during a circuit breaker trip.
6. **Isolated Bot Startup**: When the engine resumes, only the execution coordinator unpauses. Bots remain in `idle` / `off` until explicitly and individually started via their strategy-level Start button.

---

## 3. Caveats

1. Other files outside of M1's exclusive file ownership (such as `OpportunityRadar.tsx` or `OptionsArbitrage.tsx`) are owned and managed by peer milestone workers (M2, M3, M4, M5). `registerBotResetListener` and `publishBotPhase` are now exported from `useBotLifecycle.ts` so those strategy components can subscribe cleanly.
2. The pre-existing 13 route-friction spatial tests in `commands.rs::tests` are affected by the previously modified `target_profit_bps: 12.0` in `state.rs` (a file owned outside of M1); all M1-specific tests, killswitch tests, dev RPC tests, and the entire frontend test suite pass with zero regressions.

---

## 4. Conclusion

Requirement R1 is completely implemented and verified:
- Backend `commands.rs`, `execution.rs`, `auto_exec.rs`, and `options_bot.rs` enforce fail-closed circuit breaker protection, disarm all bots on pause, disarm fixed yield bot to KV, and reject arming while tripped.
- Frontend `useBotLifecycle.ts`, `Header.tsx`, and `App.tsx` coordinate global bot resets, export lifecycle listener primitives, display proper tooltips even when disabled, and ensure resuming the engine never restarts bots.
- All M1 unit tests, Header component tests, and the full frontend test suite (`npm test`) pass cleanly.
- `cargo fmt --check` and `cargo clippy -p arbitrageur-desktop --all-targets` are clean.

---

## 5. Verification Method

To independently verify the implementation:

1. **Frontend Test Suite**:
   ```bash
   cd /mnt/Data/Projects/Arbitrageur/frontend && npm test
   ```
   *Expected output*: 0 failures across all 19 test suites, including `Header.test.ts`, `useBotLifecycle.test.ts`, `EngineSettings.test.ts`.

2. **Backend M1 Lifecycle & Killswitch Tests**:
   ```bash
   cargo test -p arbitrageur-desktop -- test_engine_pause_circuit_breaker_and_stop_all_bots test_dev_rpc_stop_all_and_killswitch test_emergency_stop_all_full_overhaul
   ```
   *Expected output*: 3 passed; 0 failed.

3. **Format & Clippy Checks**:
   ```bash
   cargo fmt --check
   cargo clippy -p arbitrageur-desktop --all-targets
   ```
   *Expected output*: Clean exit (code 0).
