# Handoff Report: Survey of R1, R6, and R8

**Agent**: `survey_explorer_p3_1`  
**Working Directory**: `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/survey_explorer_p3_1/`  
**Timestamp**: 2026-10-01T16:28:00Z  
**Type**: Hard (Investigation complete)

---

## 1. Observation

1. **R1 PredictionHub Ribbon Cleanups (`frontend/src/components/PredictionHub.tsx`)**:
   - **Execution unavailable text** is rendered at lines 276–282:
     ```tsx
     {!executeReady && (
       <span
         className="text-xs font-mono text-muted max-w-72 truncate"
         title={capability?.reason ?? "Execution path unavailable"}
       >
         {capability?.reason ?? "execution path unavailable"}
       </span>
     )}
     ```
     In `frontend/src-tauri/src/execution.rs:2989`, the reason returned for prediction capability is `Some("Prediction venues are scaffolds: no market data or order execution path")`. This directly produces the scaffold warning text in the ribbon.
   - **`Max Slip` ribbon tag** is rendered at lines 226–232:
     ```tsx
     <div
       className="ribbon-pill opacity-60"
       title="Max slippage gate (prediction_max_slippage_bps) is defined but disabled: no execution path — prediction adapters are scaffolds"
     >
       <span className="text-muted">Max Slip</span>
       <span className="font-bold text-slate-500">no execution path</span>
     </div>
     ```
   - **Search placeholder** is at line 250:
     ```tsx
     placeholder="Search contract (e.g. FED, BTC)..."
     ```
   - **`Min Vol` disabled pill** is rendered at lines 219–225:
     ```tsx
     <div
       className="ribbon-pill opacity-60"
       title="Min event volume filter (auto_min_event_volume_usd) is defined but disabled: no venue volume data — prediction adapters are scaffolds"
     >
       <span className="text-muted">Min Vol</span>
       <span className="font-bold text-slate-500">no venue volume data</span>
     </div>
     ```
     `auto_min_event_volume_usd` is defined on `StrategyParamsDto` (`state.rs:297`, `api.ts:383`) with tested values `500_000.0` (`commands.rs:13475`, `apiRuntime.test.ts:239`) and `10,000` in user requirements.

2. **R6 Options Section Header Ribbon (`frontend/src/components/OptionsArbitrage.tsx`)**:
   - Strategy ribbon bar spans lines 915–989:
     ```tsx
     <div className="shrink-0 px-4 py-3 flex items-center justify-between gap-3 bg-surface rounded-xl border border-surface-border shadow-2xs select-none">
       <div className="flex items-center gap-2 text-xs font-mono min-w-0">
         <span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1 shrink-0">
           {selDef ? selDef.name : "Options Strategies"}
         </span>
         <BotPhaseBadge phase={botPhase} isPaused={isPaused} />
         ...
       </div>
       <div className="flex items-center space-x-2 shrink-0">
         ...
         {liveVisible && (
           <button ...>
             <RefreshCw ... />
             {scanning ? "Scanning…" : "Scan"}
           </button>
         )}
         ...
       </div>
     </div>
     ```
   - State `selected` is initialized to `"all"` (line 575). In `"all"` mode, `selDef` is `undefined`, so title displays `"Options Strategies"` and `Scan` button is rendered.
   - In `OptionsArbitrage.test.ts` (lines 70–87, 330–371) and `ChallengerR3R4R5.test.ts` (lines 140–152), tests render `OptionsArbitrage` with default props (`selected="all"`) and assert `assert.match(html, /Options Strategies/)` or check for the `Scan` button and status badge.

3. **R8 Ticker Explorer Symbol Quote Display (`frontend/src/components/TickerExplorer.tsx`)**:
   - TickerRow symbol rendering at lines 151–157:
     ```tsx
     <span className={`font-bold text-xs font-sans tracking-tight ${hasIssue ? "text-loss font-black" : "text-slate-900"}`}>
       {base}
       <span className="text-muted font-semibold">-{quote}</span>
     </span>
     ```
   - `base` and `quote` come from `PerpTicker` (`types/tickers.ts:485-486`).
   - In `TickerExplorer.test.ts` (lines 23–38), existing tests assert `-USDC` and `-USDT` for `XRP-USDC` and `XRP-USDT`. No existing test asserts `-USD`.

4. **Baseline Test Execution**:
   - `npm test` inside `frontend/` exited code `0`, running 49 test files with all passing.
   - `npx tsc --noEmit` flagged 2 pre-existing unused import warnings in `TickerConfigModal.tsx` (`Coins`, `Scale`). No errors in `PredictionHub.tsx`, `OptionsArbitrage.tsx`, or `TickerExplorer.tsx`.

---

## 2. Logic Chain

1. **R1 Execution Unavailable & Max Slip Removal**:
   - Lines 276–282 directly check `!executeReady` and render `capability?.reason`, which contains `"Prediction venues are scaffolds: no market data or order execution path"`. Deleting lines 276–282 completely removes this warning banner from the ribbon.
   - Lines 226–232 render the `Max Slip` pill. Deleting lines 226–232 completely removes the tag from the ribbon.
2. **R1 Search Placeholder & Active Min Vol Badge**:
   - In line 250, modifying `placeholder="Search contract (e.g. FED, BTC)..."` to `placeholder="Search contract..."` removes the example text cleanly without changing input semantics or handlers.
   - Lines 219–225 render a disabled badge with `opacity-60` and `"no venue volume data"`. Replacing it with `<div className="ribbon-pill" title="Minimum event volume threshold for prediction markets"><span className="text-muted">Min Vol</span><span className="font-bold text-slate-900 tabular-nums">{(minEventVolumeUsd ?? 10_000).toLocaleString()}</span><span className="text-muted">USD</span></div>` satisfies the active parameter badge requirement, matching the styling pattern of `Min Net` and `Trade Size`.
   - Adding `minEventVolumeUsd?: number | null;` as an optional prop on `PredictionHubProps` (defaulting to 10,000) ensures synchronous availability during SSR / `renderToStaticMarkup` without requiring an async network round-trip.
3. **R6 Options Section Header Ribbon Conditional Display**:
   - Wrapping lines 915–989 with `{selected !== "all" && (...)}` guarantees that when "All Strategies" is viewed, no top ribbon bar (`Options Strategies ... Scan`) is rendered.
   - Adding `initialSelected?: string;` (defaulting to `"all"`) to `OptionsArbitrageProps` preserves the default user experience in the desktop app while allowing tests and deep links to select a specific substrategy (e.g. `"parity"`).
   - Because existing tests in `OptionsArbitrage.test.ts` and `ChallengerR3R4R5.test.ts` tested the ribbon under default props, updating them to pass `initialSelected: "parity"` ensures test suite continuity, while a dedicated new test asserts that `selected="all"` renders no ribbon.
4. **R8 Ticker Explorer Symbol Quote Display**:
   - In `TickerRow` (line 156), replacing `<span className="text-muted font-semibold">-{quote}</span>` with:
     ```tsx
     {quote && quote.toUpperCase() !== "USD" && (
       <span className="text-muted font-semibold">-{quote}</span>
     )}
     ```
     conditionally omits the suffix when `quote` is `"USD"`.
   - For `APT-USD`, this renders `APT`.
   - For `XRP-USDC`, `quote` is `"USDC"` (`!== "USD"`), so `-USDC` renders.
   - For `XRP-USDT`, `quote` is `"USDT"` (`!== "USD"`), so `-USDT` renders.
   - All existing tests in `TickerExplorer.test.ts` assert `USDC` and `USDT`, so they pass with zero modification.

---

## 3. Caveats

1. **Pre-existing `tsc` warning in unrelated file**: `TickerConfigModal.tsx` has 2 unused import warnings (`Coins`, `Scale`) when running `npx tsc --noEmit`. The files targeted in this scope (`PredictionHub.tsx`, `OptionsArbitrage.tsx`, `TickerExplorer.tsx`) are completely clean.
2. **OptionsArbitrage test backward compatibility**: If an implementer applies `{selected !== "all" && ...}` without adding `initialSelected` or updating `OptionsArbitrage.test.ts` and `ChallengerR3R4R5.test.ts`, the existing tests in those files will fail because they look for `Options Strategies` and the `Scan` button in the default view. The proposed `initialSelected?: string` prop solves this cleanly.
3. **No existing dedicated test file for PredictionHub**: `PredictionHub` was previously only spot-checked in `ChallengerR3R4R5.test.ts`. Implementers should introduce `src/components/PredictionHub.test.ts` to explicitly lock in the cleanups.

---

## 4. Conclusion

The technical requirements for R1, R6, and R8 are concise, localized, and well-isolated.
- **R1** requires 4 changes in `PredictionHub.tsx`: deleting the execution unavailable span (lines 276–282), deleting the `Max Slip` pill (lines 226–232), cleaning the search input placeholder (line 250), and transforming `Min Vol` into an active `ribbon-pill` (lines 219–225).
- **R6** requires wrapping the ribbon bar in `OptionsArbitrage.tsx` (lines 915–989) with `{selected !== "all" && (...)}` and exposing `initialSelected?: string` for testability.
- **R8** requires wrapping the quote suffix in `TickerExplorer.tsx:156` with `{quote && quote.toUpperCase() !== "USD" && (...)}`.

All required edits, diffs, and test specifications have been documented in `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/survey_explorer_p3_1/survey.md`.

---

## 5. Verification Method

1. **Run full frontend test suite**:
   ```bash
   cd /mnt/Data/Projects/Arbitrageur/frontend && npm test
   ```
   *Expected*: All tests pass 100%.
2. **Run TypeScript check**:
   ```bash
   cd /mnt/Data/Projects/Arbitrageur/frontend && npx tsc --noEmit
   ```
   *Expected*: Zero errors in `PredictionHub.tsx`, `OptionsArbitrage.tsx`, and `TickerExplorer.tsx`.
3. **Inspect component test output**:
   - `PredictionHub.test.ts`: Verify absence of scaffold text, absence of `Max Slip`, placeholder text equals `Search contract...`, and active `Min Vol` pill.
   - `OptionsArbitrage.test.ts`: Verify `initialSelected="all"` renders no ribbon bar, and `initialSelected="parity"` renders `Put–Call Parity` ribbon with `Scan`.
   - `TickerExplorer.test.ts`: Verify `APT-USD` renders `APT` without `>-USD<`, and `APT-USDC` renders `APT` with `>-USDC<`.
