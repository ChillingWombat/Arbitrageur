# Hard Handoff Report: Independent Review & Adversarial Quality Assessment

**Reviewer**: reviewer_1 (Reviewer & Adversarial Critic)  
**Parent Agent**: 79ca425f-52b2-482b-b97b-9d8092a384bd  
**Directory**: `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/orchestrator_2/reviewer_1/`  
**Date**: 2026-09-29  

---

## Review Summary

**Verdict**: **REQUEST_CHANGES**

| Requirement | Implementation Assessment | Status | Notes |
|---|---|---|---|
| **R1**: Bot Status on Halt/Reset | Fully implemented in backend & frontend | **PASS** | Resume does not start bots; tripped breaker locks resume & arming; tooltips accessible. |
| **R2**: NetPnlChart Strategy Filter | Top bar relocation & Options unification | **PASS** | Dropdown moved to top bar right of title; `options_*` unified into single "Options" entry. |
| **R3**: Locked Dock Height | Enforced `p-3.5 h-96 overflow-y-auto` | **PASS** (with minor note) | OpportunityRadar & FundingRadar locked at 384px; inner wrappers removed. SpotArbitrageBot uses `h-80`. |
| **R4**: Standardized Status Badge | Active / Paused / Reducing / Stopped | **DEFECT FOUND** | Standardized across 6 of 7 strategy views. `SpotArbitrageBot.tsx` omits `isPaused={isPaused}`. |
| **R5**: Options Arbitrage Layout | Single-line header & 5th Metric Card | **PASS** | Outlay pill removed; 5th metric card added; header fits on single line. |
| **Regression Suite** | Backend `cargo test -p arbitrageur-desktop` | **FAIL** | 599 passed; 1 failed (`test_emergency_flatten_retries_partial_close`). |

---

## 1. Observation

### 1.1 Automated Verification Commands Executed
1. **Frontend Unit Tests**:
   - Command: `cd /mnt/Data/Projects/Arbitrageur/frontend && npm test`
   - Result: **PASS** — 19 test files, 23 test suites, 100% pass (0 failures).
2. **Formatting Check**:
   - Command: `cargo fmt --check`
   - Result: **PASS** — Clean exit code 0.
3. **Rust Linter**:
   - Command: `cargo clippy -p arbitrageur-desktop --all-targets`
   - Result: **PASS** — Clean exit code 0 (Finished dev profile, 0 warnings/errors).
4. **Backend Test Suite**:
   - Command: `cargo test -p arbitrageur-desktop`
   - Result: **FAIL** — `599 passed; 1 failed; finished in 333.98s`
   - Verbatim error:
     ```
     ---- commands::tests::test_emergency_flatten_retries_partial_close stdout ----
     thread 'commands::tests::test_emergency_flatten_retries_partial_close' (205335) panicked at frontend/src-tauri/src/commands.rs:16779:9:
     assertion `left == right` failed: no duplicate close child; notes: ["sig-1790700720298-8061e2d7: hyperliquid leg passive skipped, market exit (unfilled after 3 limit tries)", "sig-1790700720298-8061e2d7: hyperliquid leg got no fill (empty book or still working)", "sig-1790700720298-8061e2d7: lighter leg passive skipped, market exit (unfilled after 3 limit tries)", "sig-1790700720298-8061e2d7: lighter leg got no fill (empty book or still working)"]
       left: 1
      right: 2
     note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

     failures:
         commands::tests::test_emergency_flatten_retries_partial_close

     test result: FAILED. 599 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 333.98s
     ```

### 1.2 Inspection of Source Code by Requirement

#### Requirement R1: Bot Status on Halt / Emergency Reset
- `frontend/src-tauri/src/commands.rs` (lines 3178–3197, `set_engine_paused_impl`):
  - Line 3179: Checks `if !paused && state.circuit_breaker.is_tripped() { return Err("Cannot resume engine while circuit breaker is tripped. Reset circuit breaker first.".into()); }`
  - Line 3192: Sets `*state.is_paused.write() = paused;`
  - Line 3194: If `paused == true`, invokes `crate::execution::stop_all_bots(state);`. Resuming engine does NOT start any bots.
- `frontend/src-tauri/src/auto_exec.rs` (line 402, `save_config`):
  - Checks `if (mode == BotMode::Auto || dto.enabled) && state.circuit_breaker.is_tripped()` -> returns Err.
- `frontend/src-tauri/src/options_bot.rs` (line 243, `save_config`):
  - Checks `if (mode == BotMode::Auto || dto.enabled) && state.circuit_breaker.is_tripped()` -> returns Err.
- `frontend/src-tauri/src/execution.rs` (lines 3532–3579, `stop_all_bots`):
  - Disarms Radar bot (`BotMode::Off`, `enabled = false`).
  - Disarms all live Options bots (`BotMode::Off`, `enabled = false`).
  - Disarms Fixed Yield bot: `state.settings.write().auto_fixed_yield_enabled = false` and persists `kv_set("engine.auto_fixed_yield_enabled", "0")`.
- `frontend/src/components/Header.tsx` (lines 78–116):
  - Button wrapped in `<div className="relative inline-flex" title={...}>`.
  - Tooltip on tripped breaker: `"Trading halted by circuit breaker — reset in Portfolio Monitor before resuming engine"`.
  - Tooltip on paused: `"Resume Engine: Resume execution coordination. Automated bots remain stopped and must be started individually on each strategy page."`.
  - Disabled state: `disabled={isTripped}`, with `pointer-events-none cursor-not-allowed opacity-50`.
- `frontend/src/App.tsx` (lines 656–693):
  - `handleResetCircuitBreaker`, `handleEmergencyKillswitch`, `handleTogglePause(applied === true)` all invoke `stopAllFrontendBots()`.
  - `useEffect` on `systemStatus?.circuit_breaker_tripped` triggers `stopAllFrontendBots()`.

#### Requirement R2: NetPnlChartGadget Strategy Filter & Options Unification
- `frontend/src/components/NetPnlChartGadget.tsx`:
  - Lines 259–281: `<select>` relocated directly inside `<div className="flex items-center gap-3 min-w-0 flex-wrap">` alongside `<h2>P&L & Equity History</h2>`.
  - Redundant second row `{tab === "pnl" && (<div className="flex items-center gap-2 flex-wrap">...</div>)}` completely removed.
  - Lines 18–29: `KNOWN_PNL_CATEGORIES` explicitly places `"Options"` first and filters out all `options_*` keys.
  - Line 58: `realizedSeries` checks `(category === "all" || strategyCategory(o.strategyId) === category)`.
- `frontend/src/components/TradedOpportunitiesGadget.tsx` (lines 160–163):
  - `strategyCategory` maps any `strategyId.startsWith("options_")` to `"Options"`.
- `frontend/src/components/PortfolioMonitor.tsx` (lines 108–139, 495–504):
  - `groupStrategyId(strategyId)` maps `options_*` to `"options"`.
  - `STRATEGY_BUTTONS` includes `{ id: "options", label: "Options" }` and omits individual substrategies.
  - `matchStrategy` matches `options_*` when filter is `"options"`.
  - `visibleMetrics` groups all `options_*` rows under `"options"`.

#### Requirement R3: Fixed Dock Height Across Tabs
- `frontend/src/components/OpportunityRadar.tsx` (line 1829):
  - Dock container locked to `<div className="p-3.5 h-96 overflow-y-auto">`.
  - Redundant inner `max-h-96` and `overflow-y-auto` wrappers removed from `position`, `history`, `opps`, and `logs` tabs.
- `frontend/src/components/FundingRadar.tsx` (line 1665):
  - Dock container locked to `<div className="p-3.5 h-96 overflow-y-auto">`.
- `frontend/src/components/LiveMatrix.tsx` (line 1427):
  - Dock container locked to `<div className="p-3.5 h-96 overflow-y-auto">`.
- `frontend/src/components/SpotArbitrageBot.tsx` (line 571):
  - Dock container set to `<div className="p-3.5 h-80 overflow-y-auto">`. (Fixed height, but uses `h-80` rather than `h-96`).

#### Requirement R4: Standardized Status Badge
- `frontend/src/components/BotLifecycleControls.tsx` (lines 31–105):
  - `StrategyBotStatus = "Active" | "Paused" | "Reducing" | "Stopped"`.
  - `resolveBotStatus(phase, isPaused)` correctly resolves `running` to `Paused` if `isPaused`, `Active` if not; `paused` to `Paused`; `reduce_only`/`stopping` to `Reducing`; and `idle` to `Stopped`.
  - `BotPhaseBadge`: Renders dot and label; never renders the word "Engine".
- Strategy view placements:
  - `OpportunityRadar.tsx` (line 1415): `<BotPhaseBadge phase={botPhase} isPaused={isPaused} />` positioned right of `Strategy Status`, before parameter tags.
  - `FundingRadar.tsx` (line 1307): `<BotPhaseBadge phase={bot.phase} isPaused={isPaused} />` positioned right of `Strategy Status`, before parameter tags.
  - `OptionsArbitrage.tsx` (line 841): `<BotPhaseBadge phase={botPhase} isPaused={isPaused} />` positioned right of `{selDef ? selDef.name : "Options Strategies"}`. Redundant `Engine Paused` tag removed.
  - `TriangularArbitrage.tsx` (line 363): `<BotPhaseBadge phase={bot.phase} isPaused={isPaused} />` positioned right of `Strategy Status`. Redundant `Engine Paused` tag removed.
  - `PredictionHub.tsx` (line 210): `<BotPhaseBadge phase={bot.phase} isPaused={isPaused} />` positioned right of `Strategy Status`.
  - `LiveMatrix.tsx` (line 953): `<BotPhaseBadge phase={bot.phase} isPaused={isPaused} />` positioned right of `Strategy Status`.
  - **DEFECT**: `SpotArbitrageBot.tsx` (line 267): `<BotPhaseBadge phase={bot.phase} />` **OMITS** `isPaused={isPaused}`!
    Line 56 declares `isPaused = false` in component props, and line 313 passes `isPaused` to `BotLifecycleControls`. However, line 267 does not pass `isPaused` to `BotPhaseBadge`. When the engine is paused, `SpotArbitrageBot` continues displaying `Active` instead of `Paused`.

#### Requirement R5: Options Arbitrage Ribbon & Metrics Reorganization
- `frontend/src/components/OptionsArbitrage.tsx`:
  - Lines 913–968: Metric cards container is `<div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-4">`.
  - 5th card renders title `"Premium Outlay"`, formatted value `{premiumOutlayUsd != null && Math.abs(premiumOutlayUsd) > 0.005 ? fmtUsd(premiumOutlayUsd) : "—"}`, subtitle `"net financing in packages"`, and tooltip.
  - Lines 836–910: Ribbon is `<div className="shrink-0 px-4 py-3 flex items-center justify-between gap-3 bg-surface rounded-xl border border-surface-border shadow-2xs select-none">`.
  - "Premium Outlay" pill removed from ribbon.
  - Secondary internal loop parameters (`Poll` and `Max Open`) pruned so the entire header row fits on a single line without wrapping.

---

## 2. Logic Chain

1. *From Observation 1.1*: Running the mandatory backend test suite `cargo test -p arbitrageur-desktop` produces a panic failure in `commands::tests::test_emergency_flatten_retries_partial_close` at line 16779 (`assertion left == right failed: no duplicate close child; left: 1, right: 2`).
2. *From Observation 1.1*: Acceptance criteria in `ORIGINAL_REQUEST.md` (lines 48 & 95) explicitly mandate:
   `- [ ] cd frontend && npm test and cargo test -p arbitrageur-desktop pass with zero failures.`
3. *From Investigation of `test_emergency_flatten_retries_partial_close`*:
   - In `frontend/src-tauri/src/execution.rs` (lines 5216 & 5776), recent modifications changed `flatten_positions_inner`:
     - Line 5216: `pending_for_family` was restricted to `if child.status != SignalStatus::InFlight { continue; }`.
     - Line 5776: A partial close child with fills was changed from `SignalStatus::Unresolved` to `SignalStatus::Closed`.
   - When a user or automated procedure executes a second flatten while an incomplete flatten attempt exists, `pending_for_family` no longer recognizes the pending remainder of the first child.
   - Consequently, the second flatten attempt generates a duplicate close child (`children_after` increases from 1 to 2), violating duplicate suppression.
4. *From Observation 1.2 (R4)*: Requirement R4 states:
   `"All strategy headers must display a standardized bot status badge positioned to the right of the strategy title header, before any parameter tags. Remove 'Engine' from the text; the badge must display exactly one of four statuses: Active, Paused, Reducing, or Stopped."`
5. *From Observation 1.2 (R4)*: In `frontend/src/components/SpotArbitrageBot.tsx` line 267:
   `<BotPhaseBadge phase={bot.phase} />` omits `isPaused={isPaused}`.
   Because `resolveBotStatus(phase, isPaused)` defaults `isPaused` to `false`, when the coordinator engine is paused, `bot.phase` remains `"running"`, causing `SpotArbitrageBot` to render `Active` (green) rather than `Paused` (amber). This violates truthful status reporting across strategy headers.
6. *From Steps 1-5*: Because both the backend test suite fails and a strategy header status badge fails truthfulness on pause, the review verdict must be **REQUEST_CHANGES**.

---

## 3. Findings

### Finding 1: [Critical / Regression] Backend Test Failure in `test_emergency_flatten_retries_partial_close`
- **Location**: `frontend/src-tauri/src/commands.rs:16779:9` / `frontend/src-tauri/src/execution.rs:5216, 5776`
- **Problem**: Running `cargo test -p arbitrageur-desktop -- test_emergency_flatten_retries_partial_close` fails:
  ```
  assertion `left == right` failed: no duplicate close child; left: 1, right: 2
  ```
- **Cause**: In `flatten_positions_inner`, `pending_for_family` only considers `SignalStatus::InFlight`, and line 5776 immediately sets non-fully-flat children to `SignalStatus::Closed`. This causes repeat clicks on flatten to bypass pending-quantity suppression and generate duplicate close children.
- **Suggestion**: In `flatten_positions_inner`, ensure that open/non-terminal close children properly suppress duplicate dispatch until the child resolves or reaches a terminal state, matching the safety invariant expected by `test_emergency_flatten_retries_partial_close`.

### Finding 2: [Major / Interface Conformance] `SpotArbitrageBot.tsx` Status Badge Omits `isPaused`
- **Location**: `frontend/src/components/SpotArbitrageBot.tsx:267`
- **Problem**: `SpotArbitrageBot` renders `<BotPhaseBadge phase={bot.phase} />` without passing `isPaused={isPaused}`.
- **Impact**: When the engine is paused, all other strategies accurately transition their badge from `Active` to `Paused`. `SpotArbitrageBot` continues displaying `Active` with a green indicator, misrepresenting execution readiness.
- **Suggestion**: Update line 267 in `SpotArbitrageBot.tsx` to:
  ```tsx
  <BotPhaseBadge phase={bot.phase} isPaused={isPaused} />
  ```

### Finding 3: [Minor / Consistency] Dock Height Discrepancy in `SpotArbitrageBot.tsx`
- **Location**: `frontend/src/components/SpotArbitrageBot.tsx:571`
- **Problem**: While `OpportunityRadar.tsx`, `FundingRadar.tsx`, and `LiveMatrix.tsx` lock the dock container height to `h-96` (384px), `SpotArbitrageBot.tsx` uses `h-80` (320px).
- **Suggestion**: Standardize `SpotArbitrageBot.tsx` dock container to `p-3.5 h-96 overflow-y-auto` for complete visual consistency across all docked strategy views.

---

## 4. Adversarial Challenge & Stress-Testing

### Challenge 1: Circuit Breaker Reset vs. Bot Auto-Start Race Condition
- **Assumption Challenged**: Does resetting the circuit breaker in Portfolio Monitor or via API ever inadvertently re-enable automated trading?
- **Attack Scenario**: An operator trips the circuit breaker due to an abnormal market move. In Portfolio Monitor, the operator clicks "Reset Circuit Breaker".
- **Stress-Test Analysis**:
  - In `frontend/src-tauri/src/execution.rs:3581` (`reset_circuit_breaker_impl`):
    `state.circuit_breaker.reset(); stop_all_bots(state);`
  - In `frontend/src/App.tsx:656` (`handleResetCircuitBreaker`):
    Invokes `stopAllFrontendBots()`.
  - In `frontend/src-tauri/src/auto_exec.rs` and `options_bot.rs`:
    Bots cannot arm until an explicit, individual user action updates the configuration to `Auto` or `enabled: true`.
- **Result**: **PASS (Robust)**. The reset path is strictly disarming. Resuming execution coordination preserves stopped bot states.

### Challenge 2: Client-Side Engine Resume Bypass
- **Assumption Challenged**: Can an operator bypass a tripped circuit breaker by clicking "Resume Engine" in the Header?
- **Attack Scenario**: With circuit breaker tripped, inspect the DOM, remove `disabled` or `pointer-events-none` from the Header button, and dispatch a click event.
- **Stress-Test Analysis**:
  - In `frontend/src-tauri/src/commands.rs:3179` (`set_engine_paused_impl`):
    `if !paused && state.circuit_breaker.is_tripped() { return Err("Cannot resume engine while circuit breaker is tripped...".into()); }`
  - The backend returns an error without altering `*state.is_paused`.
  - In `frontend/src/App.tsx:689`, `setIsPaused(applied)` uses the backend return value, never optimistic intent.
- **Result**: **PASS (Fail-Closed)**. The backend strongly defends against client-side tampering or out-of-order UI events.

### Challenge 3: Options Substrategy PnL Isolation & Unification
- **Assumption Challenged**: If a user selects "Options" in NetPnlChartGadget, will all options substrategies correctly roll up into the cumulative series?
- **Attack Scenario**: Traded opportunities contain records from `options_parity`, `options_box_spread`, `options_jelly_roll`, `options_convexity`, and future options namespaces.
- **Stress-Test Analysis**:
  - `strategyCategory` in `TradedOpportunitiesGadget.tsx:161` maps any `strategyId.startsWith("options_")` to `"Options"`.
  - `KNOWN_PNL_CATEGORIES` sanitizes categories so `"Options"` is the sole options entry.
  - `realizedSeries` filters by `strategyCategory(o.strategyId) === category`, which matches every options record.
- **Result**: **PASS (Robust)**. Substrategy PnLs roll up faithfully without fragmentation or missing points.

### Challenge 4: Dock Container Viewport Leaks & Nested Scrolling
- **Assumption Challenged**: Does locking dock height to `p-3.5 h-96 overflow-y-auto` prevent nested scrollbars when tabs contain large logs or tabular lists?
- **Attack Scenario**: Switch to `logs` tab (thousands of entries) or `history` tab (hundreds of executed cycles).
- **Stress-Test Analysis**:
  - Worker m4 cleanly removed redundant inner `<div className="max-h-96 overflow-y-auto">` from `OpportunityRadar.tsx`.
  - Worker m3 removed redundant inner scroll wrappers from `FundingRadar.tsx` and `LiveMatrix.tsx`.
- **Result**: **PASS (Clean)**. Outer container cleanly owns the vertical scrollbar; zero viewport jumping occurs on tab switch.

---

## 5. Integrity Verification Assessment

- **Hardcoded Test Results**: None found. Tests in `Header.test.ts`, `BotLifecycleControls.test.ts`, `FundingRadar.test.ts`, `OpportunityRadar.test.ts`, `OptionsArbitrage.test.ts`, etc., assert actual DOM structure, class names, and state machines.
- **Facade Implementations**: None found. The circuit breaker checks, KV persistence, and bot disarming routines execute real logic against real engine state.
- **Shortcuts / Task Bypasses**: None found.
- **Verdict Integrity**: The findings above are based solely on direct execution of the test suites and source code inspection.

---

## 6. Caveats

- `StatArbLab.tsx` is an interactive analytical calculator for Kalman/Bertram pairs and does not instantiate automated bots or a strategy lifecycle ribbon.
- Testing of live exchange execution relies on simulated adapters (`test_sims`) and mock orderbooks, which is appropriate for unit/integration testing under testnet development mode.

---

## 7. Conclusion

While the vast majority of the work across Milestones M1–M5 is exemplary, well-tested, and adheres to high engineering standards, **REQUEST_CHANGES** is issued due to:
1. **Critical backend test regression**: `cargo test -p arbitrageur-desktop` fails on `commands::tests::test_emergency_flatten_retries_partial_close`.
2. **Strategy badge status defect**: `SpotArbitrageBot.tsx` omits `isPaused={isPaused}` on `<BotPhaseBadge phase={bot.phase} />`.

Once these two items are addressed, the entire test suite will pass and all requirements will be 100% satisfied.

---

## 8. Verification Method

To independently verify the reported findings:

1. **Reproduce Backend Test Failure**:
   ```bash
   cargo test -p arbitrageur-desktop -- test_emergency_flatten_retries_partial_close --nocapture
   ```
   *Expected result*: Panics at `commands.rs:16779:9` with `assertion left == right failed: left: 1, right: 2`.

2. **Verify Frontend Tests**:
   ```bash
   cd /mnt/Data/Projects/Arbitrageur/frontend && npm test
   ```
   *Expected result*: 19 test files pass.

3. **Inspect SpotArbitrageBot Badge**:
   Inspect line 267 of `frontend/src/components/SpotArbitrageBot.tsx`:
   Observe `<BotPhaseBadge phase={bot.phase} />` lacks `isPaused={isPaused}`.
