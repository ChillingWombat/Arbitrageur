# Handoff Report: Requirement R1 — Bot Status on Halt / Emergency Reset & Individual Startup

## Executive Summary
This investigation analyzes Requirement R1: ensuring that after halt, killswitch, emergency reset, or circuit breaker reset, all automated bot statuses across all strategies (Radar Bot, Options Bot substrategies, and frontend `useBotLifecycle` hooks) transition to stopped (`idle` / `off`), never paused. Furthermore, the Header pause/resume button must not resume any automated bots, must be disabled while the circuit breaker is tripped, and each strategy's bot must only be startable individually via its own strategy-level Start button.

---

## 1. Observations

### 1.1 Backend: Emergency Stop, Killswitch & Circuit Breaker Reset
1. **`frontend/src-tauri/src/execution.rs` lines 3532–3570 (`stop_all_bots`)**:
   ```rust
   pub fn stop_all_bots(state: &AppEngineState) {
       // 1. Radar bot: persist mode = off and enabled = false
       let (cfg, _) = crate::auto_exec::OpportunityExecutor::load_config(state);
       let disabled_cfg = crate::auto_exec::RadarBotConfigDto::from_policy(
           &cfg,
           arbitrageur_strategy::convergence_auto::BotMode::Off,
       );
       if let Err(e) = crate::auto_exec::OpportunityExecutor::save_config(state, &disabled_cfg) {
           tracing::warn!("Failed to persist radar bot off mode: {e}");
       }

       // 2. Options bots: persist mode = off and enabled = false for all live substrategies
       for sub in crate::options_bot::LIVE_SUBSTRATEGIES {
           let (cfg, _) = crate::options_bot::OptionsBotExecutor::load_config(state, sub);
           let disabled_cfg = crate::options_bot::OptionsBotConfigDto::from_policy(
               sub,
               &cfg,
               arbitrageur_strategy::convergence_auto::BotMode::Off,
           );
           if let Err(e) = crate::options_bot::OptionsBotExecutor::save_config(state, &disabled_cfg) {
               tracing::warn!("Failed to persist options bot off mode for {sub}: {e}");
           }
           state.options_tick_status.lock().insert(
               sub.to_string(),
               crate::options_bot::OptionsBotTickStatus {
                   substrategy: sub.to_string(),
                   mode: "off".to_string(),
                   open_positions: 0,
                   max_open_positions: cfg.max_open_positions,
                   last_scan_ms: chrono::Utc::now().timestamp_millis(),
                   rows_scanned: 0,
                   candidates: 0,
                   best_edge_usd: None,
                   last_refusal: None,
               },
           );
       }
   }
   ```
2. **`frontend/src-tauri/src/execution.rs` lines 3572–3576 (`reset_circuit_breaker_impl`)**:
   ```rust
   pub fn reset_circuit_breaker_impl(state: &AppEngineState) -> Result<String, String> {
       state.circuit_breaker.reset();
       stop_all_bots(state);
       Ok("Circuit Breaker reset successfully. Automated bots remain stopped — start each strategy individually.".into())
   }
   ```
3. **`frontend/src-tauri/src/execution.rs` lines 3630–3641 (`execute_emergency_stop_all_impl`)**:
   ```rust
   pub async fn execute_emergency_stop_all_impl(
       state: &AppEngineState,
   ) -> Result<EmergencyShutdownReportDto, String> {
       // 1. Immediate Fail-Closed Freeze
       state
           .circuit_breaker
           .trip("EMERGENCY KILLSWITCH ENGAGED BY OPERATOR");
       *state.is_paused.write() = true;
       let _ = state.state_store.kv_set("engine.paused", "1");
       let _ = state.state_store.kv_set("engine.is_paused", "true");
       stop_all_bots(state);
       ...
   ```
4. **`frontend/src-tauri/src/commands.rs` lines 3178–3188 (`set_engine_paused_impl`)**:
   ```rust
   pub fn set_engine_paused_impl(paused: bool, state: &AppEngineState) -> Result<bool, String> {
       state
           .state_store
           .kv_set("engine.paused", if paused { "1" } else { "0" })
           .map_err(|e| format!("Failed to persist pause state: {e}"))?;
       *state.is_paused.write() = paused;
       Ok(paused)
   }
   ```
   *Observation*: `set_engine_paused_impl` does NOT reject unpausing (`paused == false`) when `state.circuit_breaker.is_tripped()`. Furthermore, it does NOT invoke `stop_all_bots` when `paused == true`.
5. **`frontend/src-tauri/src/auto_exec.rs` lines 778–788 & `options_bot.rs` lines 340–358**:
   Both bot tick loops inspect `state.circuit_breaker.is_tripped() && mode != BotMode::Off` and autonomously retire persisted mode to `BotMode::Off`.
6. **`frontend/src-tauri/src/dev_rpc.rs` lines 261–266**:
   Both `"killswitch"`/`"stop_all"` and `"reset_circuit_breaker"` dispatch directly to `execute_emergency_stop_all_impl` and `reset_circuit_breaker_impl`.

### 1.2 Frontend: Hook & Component Implementations
1. **`frontend/src/hooks/useBotLifecycle.ts` lines 48–56 & lines 232–240**:
   ```typescript
   export function stopAllFrontendBots(): void {
     for (const [key] of botPhaseRegistry) {
       botPhaseRegistry.set(key, "idle");
     }
     botPhaseVersion += 1;
     for (const listener of botPhaseListeners) listener();
     for (const reset of botResetListeners) reset();
   }
   ```
   `botResetListeners` is currently a module-private `Set<() => void>` that is not exported for non-hook components to register with.
2. **`frontend/src/components/Header.tsx` lines 77–106**:
   ```tsx
   <button
     onClick={() => onTogglePause?.(!isPaused)}
     disabled={isTripped}
     className={
       isTripped
         ? "btn-secondary opacity-50 cursor-not-allowed"
         : isPaused
           ? "btn-warning-subtle animate-pulse"
           : "btn-secondary"
     }
     title={
       isTripped
         ? "Trading halted by circuit breaker — reset in Portfolio Monitor before resuming engine"
         : isPaused
           ? "Resume Engine: Resume execution coordination. Automated bots remain stopped and must be started individually on each strategy page."
           : "Pause Engine: Temporarily suspend executions. Open positions are preserved."
     }
   >
   ```
   *Observation*: The button is disabled when `isTripped`. However, native browser behavior frequently suppresses `title` tooltips on `<button disabled>`, meaning the explanatory tooltip is hidden unless wrapped in an interactive container.
3. **`frontend/src/components/OpportunityRadar.tsx` lines 1267–1279 & 1464–1472**:
   ```tsx
   const handleFlatten = useCallback(async () => {
     setFlattening(true);
     setFlattenError(null);
     try {
       const res = await api.flattenTickerPositions(null, "radar");
       const residual = flattenResidualMessage(res);
       if (residual !== null) setFlattenError(residual);
     } catch (e) {
       setFlattenError(e instanceof Error ? e.message : String(e));
     } finally {
       setFlattening(false);
     }
   }, []);
   ...
   <BotLifecycleControls
     phase={botPhase}
     hasOpenPositions={(botStatus?.openPositions ?? []).length > 0}
     disabled={botControlsDisabled(botCfg)}
     onStart={() => void handleBotMode("auto")}
     onPause={() => void handleBotMode("paused")}
     onReduceOnly={() => void handleBotMode("reduce_only")}
     onStop={() => void handleFlatten()}
   />
   ```
   *Observation*:
   - `handleFlatten` calls `api.flattenTickerPositions(null, "radar")`, but NEVER calls `handleBotMode("off")`! Once flattening is complete, `botCfg.mode` remains whatever it was before (e.g. `"auto"` or `"paused"`), and `botPhase` remains active/paused.
   - `OpportunityRadar.tsx` does NOT listen to `botResetListeners`. When `stopAllFrontendBots()` runs, `botCfg` in `OpportunityRadar.tsx` is not reset to `off`.
   - `OpportunityRadar.tsx` does not publish its phase to `botPhaseRegistry`.
   - `disabled={botControlsDisabled(botCfg)}` fails to include `isPaused`.
4. **`frontend/src/components/OptionsArbitrage.tsx` lines 668–681 & 917–927**:
   ```tsx
   const handleFlatten = useCallback(async () => {
     if (!botSubstrategy) return;
     setFlattening(true);
     setFlattenError(null);
     try {
       const res = await api.flattenTickerPositions(null, botSubstrategy.logStrategyId);
       const residual = flattenResidualMessage(res);
       if (residual !== null) setFlattenError(residual);
     } catch (e) {
       setFlattenError(e instanceof Error ? e.message : String(e));
     } finally {
       setFlattening(false);
     }
   }, [botSubstrategy]);
   ...
   <BotLifecycleControls
     phase={botPhase}
     hasOpenPositions={openPositions > 0}
     disabled={botCfg == null}
     allowStartWithOpenPositions
     onStart={() => void handleBotMode("auto")}
     onPause={() => void handleBotMode("paused")}
     onReduceOnly={() => void handleBotMode("reduce_only")}
     onStop={() => void handleFlatten()}
   />
   ```
   *Observation*:
   - Same defect: `handleFlatten` flattens positions but never sets `botCfg.mode = "off"`.
   - `OptionsArbitrage.tsx` does not listen to `botResetListeners`.
   - `disabled={botCfg == null}` fails to include `isPaused`.
5. **`frontend/src/App.tsx` lines 677–681**:
   ```tsx
   useEffect(() => {
     if (systemStatus?.circuit_breaker_tripped) {
       stopAllFrontendBots();
     }
   }, [systemStatus?.circuit_breaker_tripped]);
   ```
   *Observation*: If `circuit_breaker_tripped` transitions from `true` to `false` (e.g. after a reset performed via Dev RPC), `stopAllFrontendBots()` is NOT called because `if (systemStatus?.circuit_breaker_tripped)` is false.

---

## 2. Logic Chain

1. **Premise 1 (Safety Requirement)**: Requirement R1 mandates that after halt, killswitch, emergency reset, or circuit breaker reset, all automated bot statuses across all strategies (Radar Bot, Options Bot substrategies, and frontend `useBotLifecycle` hooks) must be stopped (`idle` / `off`), never paused.
2. **Backend Execution**:
   - In `execution.rs`, `stop_all_bots(state)` successfully sets `BotMode::Off` for Radar and Options bots in SQLite and in-memory caches (Obs 1.1.1).
   - In `reset_circuit_breaker_impl` and `execute_emergency_stop_all_impl`, `stop_all_bots(state)` is already invoked (Obs 1.1.2, 1.1.3).
   - However, fixed-yield autonomous entries in `funding.rs` can be triggered if `state.settings.read().auto_fixed_yield_enabled` remains true. Calling `state.settings.write().auto_fixed_yield_enabled = false` in `stop_all_bots` guarantees all automated backend bots are stopped.
   - In `commands.rs`, `set_engine_paused_impl` lacks a guard blocking unpause while the breaker is tripped. Adding `if !paused && state.circuit_breaker.is_tripped() { return Err(...); }` guarantees fail-closed safety (Obs 1.1.4).
   - Calling `stop_all_bots(state)` when `paused == true` in `set_engine_paused_impl` guarantees that pausing the engine from Header or RPC stops automated bot execution.
3. **Frontend Strategy Isolation & State Synchronization**:
   - In `useBotLifecycle.ts`, `stopAllFrontendBots()` transitions all hook-managed bots to `"idle"` (Obs 1.2.1).
   - However, `OpportunityRadar.tsx` and `OptionsArbitrage.tsx` manage bot states via local React state (`botCfg`), polling config/status, and deriving `botPhase` (Obs 1.2.3, 1.2.4).
   - Because they do not listen to `botResetListeners`, calling `stopAllFrontendBots()` leaves their local `botPhase` unchanged if the components remain mounted.
   - When an operator clicks "Stop" in `OpportunityRadar` or `OptionsArbitrage`, the component calls `api.flattenTickerPositions` but never sets `botCfg.mode = "off"` or calls `api.setRadarBotConfig` / `api.setOptionsBotConfig` to persist `off` mode (Obs 1.2.3, 1.2.4). When flattening completes, the bot remains in its prior phase (running or paused).
   - Exporting `registerBotResetListener` from `useBotLifecycle.ts` allows `OpportunityRadar.tsx` and `OptionsArbitrage.tsx` to subscribe. When reset/halt occurs, they can immediately set their local `botCfg` mode to `"off"`, `enabled` to `false`, and update the backend.
   - Updating `handleFlatten` in both `OpportunityRadar.tsx` and `OptionsArbitrage.tsx` to explicitly set `mode = "off"` before or during flatten ensures local and persisted states become `off` / `idle`.
4. **Header Pause/Resume Controls**:
   - The Header button toggles `isPaused` via `api.setEnginePaused(paused)`.
   - When resumed (`paused = false`), it only unpauses coordinator dispatch. It does NOT invoke any bot start functions. All bots stay in `idle`/`off` until their strategy-level Start button is clicked (Obs 1.2.2).
   - To guarantee the tooltip is accessible while disabled, wrapping the `<button>` in a `<div title={...}>` ensures browsers display the explanatory message when hovered, even while `disabled={isTripped}` is active.

---

## 3. Caveats

1. **Autonomous Live Options Entries**: In live mode (`state.is_live()`), autonomous options entries are already rejected at the execute gate (`OptionsBotExecutor::load_config` refuses live auto). The UI surfaces "Manual executes only" in live mode.
2. **Stale Signal Ledgers**: If the durable signal ledger is unreachable during a stop, `shouldAttemptFlattenOnStop` fails closed and attempts flatten anyway. The bot phase transitions to `idle` (stopped), but sets an error message indicating the ledger was unavailable.
3. **External Positions**: External positions detected via `external_positions.rs` are flattened during emergency stop all, but are not owned by any single bot lifecycle hook.

---

## 4. Conclusion & Concrete Implementation Plan

### Conclusion
Requirement R1 can be achieved cleanly without breaking existing interface contracts. The core defects identified are:
1. `OpportunityRadar.tsx` and `OptionsArbitrage.tsx` do not transition bot configs to `off` when the strategy-level "Stop" button is clicked (only flattening positions).
2. `OpportunityRadar.tsx` and `OptionsArbitrage.tsx` are decoupled from `stopAllFrontendBots()` and `botResetListeners`.
3. `set_engine_paused_impl` in `commands.rs` does not reject unpausing while `circuit_breaker.is_tripped()`, and does not stop bots on engine pause.
4. `Header.tsx` needs a wrapper container around the disabled pause/resume button so that hover tooltips remain visible while `disabled={isTripped}`.
5. `App.tsx` only calls `stopAllFrontendBots()` when `circuit_breaker_tripped` is true, but should also ensure reset events (including transition from tripped to clean) stop all bots.

### Concrete Implementation Plan

#### Step 1: Backend Refinements
1. **`frontend/src-tauri/src/commands.rs`**:
   - In `set_engine_paused_impl(paused: bool, state: &AppEngineState)`:
     - Check: `if !paused && state.circuit_breaker.is_tripped() { return Err("Cannot resume engine while circuit breaker is tripped. Reset circuit breaker first.".into()); }`
     - If `paused`, invoke `crate::execution::stop_all_bots(state);`.
2. **`frontend/src-tauri/src/execution.rs`**:
   - In `stop_all_bots(state)`:
     - Add `state.settings.write().auto_fixed_yield_enabled = false;` and persist to KV `state.state_store.kv_set("engine.auto_fixed_yield_enabled", "0")`.
3. **`frontend/src-tauri/src/auto_exec.rs` & `options_bot.rs`**:
   - In `OpportunityExecutor::save_config` and `OptionsBotExecutor::save_config`:
     - Reject arming (`mode == BotMode::Auto || enabled == true`) if `state.circuit_breaker.is_tripped()`.

#### Step 2: Frontend Refinements
1. **`frontend/src/hooks/useBotLifecycle.ts`**:
   - Export `registerBotResetListener(listener: () => void): () => void`.
   - Export `publishBotPhase(strategyId: string, symbol: string | null | undefined, phase: BotPhase): void`.
2. **`frontend/src/components/OpportunityRadar.tsx`**:
   - In `handleFlatten`: set `handleBotMode("off")` (and update backend config `api.setRadarBotConfig`) so the bot is stopped.
   - Register listener with `registerBotResetListener`: on global reset, set `botCfg` mode to `"off"`, `enabled` to `false`, and publish `"idle"`.
   - Pass `isPaused` into `disabled={botControlsDisabled(botCfg) || isPaused}`.
   - Publish `botPhase` to `botPhaseRegistry` via `publishBotPhase("radar", null, botPhase)`.
3. **`frontend/src/components/OptionsArbitrage.tsx`**:
   - In `handleFlatten`: set `handleBotMode("off")` (and update backend config `api.setOptionsBotConfig`) so the bot is stopped.
   - Register listener with `registerBotResetListener`: on global reset, set `botCfg` mode to `"off"`, `enabled` to `false`.
   - Pass `isPaused` into `disabled={botCfg == null || isPaused}`.
   - Publish `botPhase` to `botPhaseRegistry`.
4. **`frontend/src/components/Header.tsx`**:
   - Wrap the pause/resume button in a `div` or `span` carrying the `title` attribute so the tooltip renders reliably even when `disabled={isTripped}`.
5. **`frontend/src/App.tsx`**:
   - In `handleResetCircuitBreaker`: ensure `stopAllFrontendBots()` is called and state is refreshed.
   - In `handleTogglePause`: when pausing (`applied === true`), call `stopAllFrontendBots()`.
   - Monitor `systemStatus?.circuit_breaker_tripped` transition: if it changes (whether tripped or reset), call `stopAllFrontendBots()`.
6. **`frontend/src/components/EngineSettings.test.ts`**:
   - Fix line 125 from `fileURLToPath(import.meta.url)` to `__filename` to fix the test suite regression.
7. **Add Header test file `frontend/src/components/Header.test.ts`**:
   - Test Header pause/resume button rendering, disabled state when `isTripped`, tooltip display, and ensure resume does not trigger bots.

---

## 5. Verification Method

### 5.1 Backend Verification Commands
1. **Desktop Backend Unit & Integration Tests**:
   ```bash
   cargo test -p arbitrageur-desktop
   ```
2. **Specific Bot Lifecycle & Killswitch Tests**:
   ```bash
   cargo test -p arbitrageur-desktop test_risk_overview_and_killswitch
   cargo test -p arbitrageur-desktop test_emergency_stop_all_full_overhaul
   cargo test -p arbitrageur-desktop test_dev_rpc_stop_all_and_killswitch
   ```
3. **Formatting & Linting**:
   ```bash
   cargo fmt --check
   cargo clippy -p arbitrageur-desktop --all-targets -- -D warnings
   ```

### 5.2 Frontend Verification Commands
1. **Frontend Test Suite**:
   ```bash
   cd /mnt/Data/Projects/Arbitrageur/frontend && npm test
   ```
2. **Specific Lifecycle & Component Tests**:
   ```bash
   cd /mnt/Data/Projects/Arbitrageur/frontend && node --experimental-strip-types --test "src/hooks/useBotLifecycle.test.ts"
   cd /mnt/Data/Projects/Arbitrageur/frontend && node scripts/test-components.mjs
   ```

### 5.3 Invalidation Conditions
- Any automated bot entering or remaining in `paused` mode instead of `off` / `idle` after halt, killswitch, emergency reset, or circuit breaker reset.
- The Header pause/resume button initiating any trade executions or starting any bots upon resume.
- The Header pause/resume button being clickable while `status.circuit_breaker_tripped` is true.
- A bot on any strategy page starting without explicit user interaction on that strategy's Start button.
