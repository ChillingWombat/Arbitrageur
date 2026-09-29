# Empirical Adversarial Challenge Report — Requirements R1 & R2

**Verdict: APPROVE**

---

## 1. Observation

### R1: Bot Lifecycle, Circuit Breaker Gating & Header Controls

1. **Header Pause/Resume Tooltip and Disablement (`frontend/src/components/Header.tsx:77-117`)**:
   - The button is enclosed in a wrapper with a dynamic tooltip:
     ```tsx
     <div
       className="relative inline-flex"
       title={
         isTripped
           ? "Trading halted by circuit breaker — reset in Portfolio Monitor before resuming engine"
           : isPaused
             ? "Resume Engine: Resume execution coordination. Automated bots remain stopped and must be started individually on each strategy page."
             : "Pause Engine: Temporarily suspend executions. Open positions are preserved."
       }
     >
       <button
         onClick={() => onTogglePause?.(!isPaused)}
         disabled={isTripped}
         className={
           isTripped
             ? "btn-secondary opacity-50 cursor-not-allowed pointer-events-none"
             : isPaused
               ? "btn-warning-subtle animate-pulse"
               : "btn-secondary"
         }
         ...
     ```
   - When `isTripped === true`: `disabled={true}`, `pointer-events-none` prevents clicks and passes hover to wrapper tooltip, `cursor-not-allowed` styles the disabled state.

2. **Header Resuming Does Not Start Bots (`frontend/src/App.tsx:683-699`)**:
   - `handleTogglePause` implementation:
     ```typescript
     const handleTogglePause = useCallback(
       async (paused: boolean) => {
         try {
           const applied = await api.setEnginePaused(paused);
           setIsPaused(applied);
           if (applied) {
             stopAllFrontendBots();
           }
           await refreshAllData();
         } catch (err) {
           notifyCommandError(describeCommandError(err, "Failed to toggle pause status"));
         }
       },
       [refreshAllData, notifyCommandError],
     );
     ```
   - Resuming (`paused = false`, `applied = false`) sets `isPaused` to false and refreshes data; it never invokes any bot start calls.

3. **Backend Circuit Breaker Gating on Resume (`frontend/src-tauri/src/commands.rs:3178-3197`)**:
   - `set_engine_paused_impl` strictly rejects resuming while tripped:
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

4. **Engine Pause Disarms All Bots (`frontend/src-tauri/src/execution.rs:3533-3579`)**:
   - `stop_all_bots`:
     - Radar bot: persists `BotMode::Off` and `enabled = false` (`execution.rs:3535-3542`).
     - Options bots: persists `BotMode::Off` and `enabled = false` for all live substrategies in `LIVE_SUBSTRATEGIES`, resets `options_tick_status` mode to `"off"` (`execution.rs:3545-3569`).
     - Fixed yield bot: sets `state.settings.write().auto_fixed_yield_enabled = false` and persists `engine.auto_fixed_yield_enabled = "0"` (`execution.rs:3572-3578`).

5. **Saving Bot Config Rejects Auto/Enabled While Tripped**:
   - Radar bot (`frontend/src-tauri/src/auto_exec.rs:402-407`):
     ```rust
     if (mode == BotMode::Auto || dto.enabled) && state.circuit_breaker.is_tripped() {
         return Err(
             "Cannot enable or arm Radar bot while circuit breaker is tripped. Reset circuit breaker first."
                 .into(),
         );
     }
     ```
   - Options bot (`frontend/src-tauri/src/options_bot.rs:243-248`):
     ```rust
     if (mode == BotMode::Auto || dto.enabled) && state.circuit_breaker.is_tripped() {
         return Err(format!(
             "Cannot enable or arm Options bot for '{}' while circuit breaker is tripped. Reset circuit breaker first.",
             dto.substrategy_id
         ));
     }
     ```

6. **Strategy Panel Stop & Flatten Transitions to Off (`OpportunityRadar.tsx` & `OptionsArbitrage.tsx`)**:
   - In `OpportunityRadar.tsx:1276-1289` & `1481`: `onStop` triggers `handleFlatten()`, which begins with `void handleBotMode("off")`, saving `mode: "off"` and `enabled: false`. Additionally, `registerBotResetListener` listener sets `mode: "off"` and `enabled: false`.
   - In `OptionsArbitrage.tsx:677-691` & `906`: `onStop` triggers `handleFlatten()`, which begins with `void handleBotMode("off")`, saving `mode: "off"` and `enabled: false`. Additionally, `registerBotResetListener` sets `mode: "off"` and `enabled: false`.

---

### R2: Strategy Filter Relocation & Options Unification

1. **NetPnlChartGadget Filter Placement (`frontend/src/components/NetPnlChartGadget.tsx:259-282`)**:
   - Dropdown is placed inside the top bar `<div className="flex items-center gap-3 min-w-0 flex-wrap">` directly to the right of `<h2>P&L & Equity History</h2>`.
   - The former second row (`<div className="flex items-center gap-2 flex-wrap"><select ...>`) was removed completely.

2. **NetPnlChartGadget Options Unification (`frontend/src/components/NetPnlChartGadget.tsx:18-29, 236-250`)**:
   - `KNOWN_PNL_CATEGORIES` explicitly defines `"Options"` and filters out all keys starting with `"options_"`:
     ```tsx
     export const KNOWN_PNL_CATEGORIES: string[] = [
       "Options",
       ...Object.keys(STRATEGY_DISPLAY_NAMES)
         .filter(
           (k) =>
             !k.startsWith("options_") &&
             k !== "options" &&
             k !== "unattributed" &&
             k !== "external",
         )
         .map((k) => STRATEGY_DISPLAY_NAMES[k]),
     ].filter((c, i, arr) => arr.indexOf(c) === i);
     ```
   - In `TradedOpportunitiesGadget.tsx:160-163`:
     ```typescript
     export const strategyCategory = (strategyId: string): string =>
       strategyId.startsWith("options_")
         ? "Options"
         : (STRATEGY_DISPLAY_NAMES[strategyId] ?? strategyId);
     ```
   - In `realizedSeries(opps, category)` (`NetPnlChartGadget.tsx:58`):
     `strategyCategory(o.strategyId) === category` maps all options substrategies (`options_parity`, `options_box_spread`, `options_jelly_roll`, `options_convexity`, `options`) to `"Options"`.

3. **PortfolioMonitor Options Unification (`frontend/src/components/PortfolioMonitor.tsx:108-139, 469-585`)**:
   - `STRATEGY_BUTTONS` defines `{ id: "options", label: "Options" }` and contains zero substrategy buttons (`PortfolioMonitor.tsx:111-120`).
   - `groupStrategyId` returns `"options"` for any string starting with `"options_"` (`PortfolioMonitor.tsx:108-109`).
   - `matchStrategy(rawId, "options")` matches `"options"` and all `"options_*"` (`PortfolioMonitor.tsx:126-133`).
   - `visibleMetrics` and `visibleStrategyRows` filter down to `m.strategy_id === "options" || m.strategy_id.startsWith("options_")` and `r.ledgerId === "options" || r.ledgerId.startsWith("options_")`.
   - All high-level cards (completed cycles, win rate, total PnL, open positions, deployed capital) correctly compute over only the filtered options items.

---

## 2. Logic Chain

1. **Header Pause/Resume Invariance**:
   - Observation 1.1 shows `<button>` has `disabled={isTripped}` and `pointer-events-none`.
   - Observation 1.2 shows `handleTogglePause(false)` only writes `isPaused = false` in React and does not call any bot start API or lifecycle method.
   - Observation 1.3 shows that even if an attacker or automated script bypasses the UI and calls `set_engine_paused(false)`, backend `set_engine_paused_impl` returns `Err` because `state.circuit_breaker.is_tripped()` is checked before unpausing.
   - Therefore, clicking or calling resume can NEVER start any bot, and unpausing is strictly prevented while the circuit breaker is tripped.

2. **Comprehensive Disarming on Halt / Pause**:
   - Observation 1.4 confirms that `stop_all_bots(state)` is triggered on `set_engine_paused_impl(true)`, `reset_circuit_breaker_impl`, and `execute_emergency_stop_all_impl`.
   - Radar bot is persisted to `BotMode::Off` and `enabled = false`.
   - Options bot is persisted to `BotMode::Off` and `enabled = false` across all live substrategies.
   - Fixed yield bot is persisted to `auto_fixed_yield_enabled = false` and KV `0`.
   - Observation 1.5 shows that attempting to save bot configs with `Auto` mode or `enabled: true` while tripped is rejected with fail-closed errors.
   - Observation 1.6 confirms clicking Stop or flatten in `OpportunityRadar` and `OptionsArbitrage` calls `handleBotMode("off")`.
   - Therefore, all bots are reliably disarmed on halt, pause, and stop.

3. **Options Grouping & Chart Top Bar Alignment**:
   - Observation 2.1 confirms `<select>` is nested directly in `<div className="flex items-center gap-3 min-w-0 flex-wrap">` adjacent to `<h2>P&L & Equity History</h2>`, eliminating the previous second filter row.
   - Observation 2.2 confirms individual options substrategies ("Options Parity", "Box Spread", "Jelly Roll", "Convexity") are filtered out of the dropdown options, leaving a single unified `"Options"` option.
   - Observation 2.2 confirms `realizedSeries` aggregates all options trades under `"Options"`.
   - Observation 2.3 confirms `PortfolioMonitor` displays unified `"Options"` filter button and filters open positions, metrics, and strategy rows accurately.
   - Therefore, Requirement R2 is fully satisfied.

---

## 3. Caveats

No caveats. All investigated areas have been empirically tested through both existing component/Rust test suites and an adversarial DOM/logic test script (`/tmp/test_r1_r2_adversarial.mjs`).

---

## 4. Conclusion

Requirement R1 (Bot status on halt/reset, Header controls, circuit breaker gating, individual startup) and Requirement R2 (Strategy filter relocation in NetPnlChartGadget, Options substrategy unification in NetPnlChartGadget and PortfolioMonitor) are **fully and robustly satisfied without regressions**.

- **Header pause/resume button**: Correctly disabled when breaker is tripped, displays informative tooltip, never starts any bots on unpause.
- **Circuit breaker gating**: Backend strictly rejects unpausing while tripped; bot config arming is blocked while tripped.
- **Bot lifecycle**: Engine pause disarms all bots (Radar, Options, Fixed Yield) to off/idle. Strategy panels transition to off on Stop/flatten. Individual manual startup is required on each panel.
- **NetPnlChartGadget**: Dropdown moved to top bar next to title; redundant second row removed; options unified under single "Options" entry.
- **PortfolioMonitor**: Unified "Options" button filters all options metrics, rows, and open positions.

**Final Verdict: APPROVE**.

---

## 5. Verification Method

### Test Commands Executed

1. **Frontend Test Suite**:
   ```bash
   cd /mnt/Data/Projects/Arbitrageur/frontend && npm test
   ```
   *Result*: 9 suites passed, 0 failed, 60+ component tests passed (including `Header.test.ts`, `NetPnlChartGadget.test.ts`, `PortfolioMonitor.test.ts`, `BotLifecycleControls.test.ts`).

2. **Targeted Backend Test Commands**:
   ```bash
   cargo test -p arbitrageur-desktop -- test_engine_pause_circuit_breaker_and_stop_all_bots
   cargo test -p arbitrageur-desktop -- options_bot::tests
   cargo test -p arbitrageur-desktop -- auto_exec::tests
   ```
   *Result*: All passed (0 failed).

3. **Adversarial Empirical Script**:
   ```bash
   cd /mnt/Data/Projects/Arbitrageur/frontend
   NODE_PATH=./node_modules npx esbuild /tmp/test_r1_r2_adversarial.mjs --bundle --platform=node --outfile=/tmp/test_r1_r2_adversarial.cjs && node /tmp/test_r1_r2_adversarial.cjs
   ```
   *Result*:
   - `Header button attributes & tooltips pass.`
   - `useBotLifecycle disarm & stopAllFrontendBots pass.`
   - `NetPnlChartGadget layout and options filtering pass.`
   - `PortfolioMonitor options unification & matchStrategy pass.`
   - `ALL EMPIRICAL ADVERSARIAL CHECKS PASSED.`
