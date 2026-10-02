# Technical Survey: R1, R6, and R8

**Surveyor**: `survey_explorer_p3_1`  
**Date**: 2026-10-01T16:25:00Z  
**Scope**:
- **R1**: Prediction Market Ribbon & Input Cleanups (`frontend/src/components/PredictionHub.tsx`)
- **R6**: Options Section Header Ribbon (`frontend/src/components/OptionsArbitrage.tsx`)
- **R8**: Ticker Explorer Symbol Quote Display (`frontend/src/components/TickerExplorer.tsx`)

---

## 1. Executive Summary

This investigation surveys the exact code implementations and test suites for Requirements R1, R6, and R8 of the follow-up request (2026-10-01T16:09:57Z). All three requirements focus on frontend UI ergonomics, eliminating legacy scaffolding warnings, and streamlining ribbon bars and ticker displays.

| Req | Target Component | Core Actions | Existing Test Seams |
|---|---|---|---|
| **R1** | `frontend/src/components/PredictionHub.tsx` | 1. Remove execution unavailable warning (`Prediction venues are scaffolds: no market data or order execution path`).<br>2. Remove `Max Slip` ribbon tag.<br>3. Change search placeholder to `Search contract...`.<br>4. Convert `Min Vol` to active parameter badge (`10,000` or `$500,000` USD), removing disabled text `no venue volume data`. | `ChallengerR3R4R5.test.ts` (lines 123–137, 189–198). Dedicated `PredictionHub.test.ts` needed. |
| **R6** | `frontend/src/components/OptionsArbitrage.tsx` | Hide top strategy ribbon bar (`Options Strategies ... Scan`) when "All Strategies" (`selected === "all"`) is selected; render it only when a specific substrategy is selected. | `OptionsArbitrage.test.ts` (lines 70–87, 330–371), `ChallengerR3R4R5.test.ts` (lines 138–152). Tests need adjustment/parameterization for `initialSelected`. |
| **R8** | `frontend/src/components/TickerExplorer.tsx` | In `TickerRow`, omit `-USD` quote suffix when `quote` is universal USD, rendering only `{base}` (e.g. `APT`), while retaining non-USD quote suffixes (`-USDC`, `-USDT`, etc.). | `TickerExplorer.test.ts` (lines 23–38). Current tests use `USDC`/`USDT`, so zero regression risk on existing assertions. |

---

## 2. Requirement R1: Prediction Market Ribbon & Input Cleanups

### 2.1 File Location & Line Analysis
- **Target File**: `frontend/src/components/PredictionHub.tsx`
- **Total Lines**: 413 lines

#### Observation 1: Execution Unavailable Scaffold Warning Text
- **Lines 276–282**:
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
- **Backend Origin**: `frontend/src-tauri/src/execution.rs:2989`:
  ```rust
  Some("Prediction venues are scaffolds: no market data or order execution path")
  ```
- **Effect in UI**: When `executeReady` is false (the current state for prediction markets), this `<span>` prints `"Prediction venues are scaffolds: no market data or order execution path"` directly in the ribbon next to the bot lifecycle buttons.
- **Action**: Completely remove lines 276–282 from `PredictionHub.tsx`.

#### Observation 2: `Max Slip` Ribbon Tag
- **Lines 226–232**:
```tsx
<div
  className="ribbon-pill opacity-60"
  title="Max slippage gate (prediction_max_slippage_bps) is defined but disabled: no execution path — prediction adapters are scaffolds"
>
  <span className="text-muted">Max Slip</span>
  <span className="font-bold text-slate-500">no execution path</span>
</div>
```
- **Effect in UI**: Displays a dimmed (`opacity-60`) pill reading `Max Slip no execution path`.
- **Action**: Completely remove lines 226–232 from `PredictionHub.tsx`.

#### Observation 3: Filter Search Placeholder
- **Line 250**:
```tsx
<input
  type="text"
  placeholder="Search contract (e.g. FED, BTC)..."
  value={searchTerm}
  onChange={(e) => setSearchTerm(e.target.value)}
  className="input-field !w-56 !pl-8 !pr-3 !py-1.5 !text-xs font-mono"
/>
```
- **Action**: Update `placeholder="Search contract (e.g. FED, BTC)..."` to `placeholder="Search contract..."`.

#### Observation 4: `Min Vol` Disabled Text vs. Active Parameter Badge
- **Lines 219–225**:
```tsx
<div
  className="ribbon-pill opacity-60"
  title="Min event volume filter (auto_min_event_volume_usd) is defined but disabled: no venue volume data — prediction adapters are scaffolds"
>
  <span className="text-muted">Min Vol</span>
  <span className="font-bold text-slate-500">no venue volume data</span>
</div>
```
- **Context in Codebase**:
  - `StrategyParamsDto` (`state.rs:297`, `strategy_config.rs:270`) carries `auto_min_event_volume_usd: Option<f64>`.
  - In `state.rs:408`, default is `None`. In `commands.rs:13475`, it is tested with `500_000.0`. In `apiRuntime.test.ts:239`, it is initialized to `500000`.
  - In `App.tsx:1196`, `strategyParams.get("futures")` is available and can be passed to `PredictionHub` as `minEventVolumeUsd`.
- **Action**:
  1. Add optional prop `minEventVolumeUsd?: number | null;` to `PredictionHubProps` (line 124).
  2. In `PredictionHub`, initialize active state or fallback:
     ```tsx
     const displayMinVol = minEventVolumeUsd ?? 10_000;
     ```
     (Optionally load via `api.getStrategyParams("futures")` in `useEffect` when prop is omitted).
  3. Replace the disabled pill with the active parameter badge:
     ```tsx
     <div
       className="ribbon-pill"
       title="Minimum event volume threshold for prediction markets"
     >
       <span className="text-muted">Min Vol</span>
       <span className="font-bold text-slate-900 tabular-nums">
         {displayMinVol.toLocaleString()}
       </span>
       <span className="text-muted">USD</span>
     </div>
     ```
  - Note: Removes `opacity-60` and `text-slate-500`. Uses active `ribbon-pill` styling matching other strategy ribbons (`Trade Size` in `SpotArbitrageBot.tsx`, `Min Net` in `PredictionHub.tsx`).

### 2.2 Proposed Diff for `PredictionHub.tsx`
```diff
--- a/frontend/src/components/PredictionHub.tsx
+++ b/frontend/src/components/PredictionHub.tsx
@@ -134,6 +134,7 @@ interface PredictionHubProps {
   /** Backend capability descriptor — prediction scan/execute are scaffolds
    *  today, so every armed control is hidden when this says not-ready. */
   capability?: StrategyCapabilityDto;
+  minEventVolumeUsd?: number | null;
 }
 
 export const PredictionHub: React.FC<PredictionHubProps> = ({
@@ -144,6 +145,7 @@ export const PredictionHub: React.FC<PredictionHubProps> = ({
   hasApiConnectivity = false,
   onOpenConfig,
   capability,
+  minEventVolumeUsd = 10000,
 }) => {
   const [searchTerm, setSearchTerm] = useState("");
   const [detailOpp, setDetailOpp] = useState<PredictionOpportunityDto | null>(null);
@@ -216,19 +218,14 @@ export const PredictionHub: React.FC<PredictionHubProps> = ({
             <span className="font-bold text-slate-900 tabular-nums">1.0</span>
             <span className="text-muted">¢</span>
           </div>
-          <div
-            className="ribbon-pill opacity-60"
-            title="Min event volume filter (auto_min_event_volume_usd) is defined but disabled: no venue volume data — prediction adapters are scaffolds"
-          >
+          <div
+            className="ribbon-pill"
+            title="Minimum event volume threshold for prediction markets"
+          >
             <span className="text-muted">Min Vol</span>
-            <span className="font-bold text-slate-500">no venue volume data</span>
+            <span className="font-bold text-slate-900 tabular-nums">{minEventVolumeUsd.toLocaleString()}</span>
+            <span className="text-muted">USD</span>
           </div>
-          <div
-            className="ribbon-pill opacity-60"
-            title="Max slippage gate (prediction_max_slippage_bps) is defined but disabled: no execution path — prediction adapters are scaffolds"
-          >
-            <span className="text-muted">Max Slip</span>
-            <span className="font-bold text-slate-500">no execution path</span>
-          </div>
         </div>
 
         <div className="flex items-center gap-3 shrink-0">
@@ -247,7 +244,7 @@ export const PredictionHub: React.FC<PredictionHubProps> = ({
           <form onSubmit={handleSearch} className="flex items-center space-x-2">
             <div className="relative">
               <input
                 type="text"
-                placeholder="Search contract (e.g. FED, BTC)..."
+                placeholder="Search contract..."
                 value={searchTerm}
                 onChange={(e) => setSearchTerm(e.target.value)}
                 className="input-field !w-56 !pl-8 !pr-3 !py-1.5 !text-xs font-mono"
@@ -273,13 +270,6 @@ export const PredictionHub: React.FC<PredictionHubProps> = ({
             onReduceOnly={bot.reduceOnly}
             onStop={() => void bot.stop()}
           />
-          {!executeReady && (
-            <span
-              className="text-xs font-mono text-muted max-w-72 truncate"
-              title={capability?.reason ?? "Execution path unavailable"}
-            >
-              {capability?.reason ?? "execution path unavailable"}
-            </span>
-          )}
           {bot.lastError && (
```

---

## 3. Requirement R6: Options Section Header Ribbon

### 3.1 File Location & Line Analysis
- **Target File**: `frontend/src/components/OptionsArbitrage.tsx`
- **Total Lines**: 1388 lines

#### Observation 1: Ribbon Rendering Logic
- **Lines 915–989**:
```tsx
{/* Strategy Status ribbon — mirrors the other strategy pages:
    read-only params + phase badge left, configurator, scan and the
    substrategy bot's lifecycle buttons right. The "All" view stays
    monitor-only — per-substrategy controls need a selection. */}
<div className="shrink-0 px-4 py-3 flex items-center justify-between gap-3 bg-surface rounded-xl border border-surface-border shadow-2xs select-none">
  <div className="flex items-center gap-2 text-xs font-mono min-w-0">
    <span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1 shrink-0">
      {selDef ? selDef.name : "Options Strategies"}
    </span>
    <BotPhaseBadge phase={botPhase} isPaused={isPaused} />
...
  <div className="flex items-center space-x-2 shrink-0">
...
    {liveVisible && (
      <button
        type="button"
        onClick={() => void scan()}
        disabled={scanning || isPaused || !optionsVenueAvailable}
...
        className="btn-primary inline-flex items-center gap-1.5 !text-xs !py-1.5 !px-2.5 font-sans shrink-0"
      >
        <RefreshCw className={`w-3.5 h-3.5 ${scanning ? "animate-spin" : ""}`} />
        {scanning ? "Scanning…" : "Scan"}
      </button>
    )}
...
  </div>
</div>
```
- **Context in Component**:
  - `selected` state is initialized to `"all"` (line 575): `const [selected, setSelected] = useState<string>("all");`
  - In `"all"` mode, `selDef` is `undefined`, so `{selDef ? selDef.name : "Options Strategies"}` printed `"Options Strategies"`, and `liveVisible` is true, so the `Scan` button rendered.
- **Requirement R6**:
  "hide the strategy status ribbon bar (`Options Strategies ... Scan`) when 'All Strategies' is selected, rendering it only when a specific substrategy is selected."
- **Action**:
  Wrap the entire ribbon container (`<div className="shrink-0 px-4 py-3 flex items-center justify-between...">...</div>`) in:
  ```tsx
  {selected !== "all" && (
    <div className="shrink-0 px-4 py-3 ...">
      ...
    </div>
  )}
  ```

#### Observation 2: Test Impact on `OptionsArbitrage.test.ts` & `ChallengerR3R4R5.test.ts`
1. `OptionsArbitrage.test.ts`:
   - `test("gate: unadmitted options venue renders the page with a disabled scan + banner")` (lines 70–87):
     Renders `<OptionsArbitrage optionsVenueAvailable={false} />` with default selection (`"all"`), and asserts `assert.match(html, /Options Strategies/);` and asserts the `Scan` button renders.
     When R6 hides the ribbon in `"all"` mode, there is no `Options Strategies` ribbon or `Scan` button in `"all"` mode!
   - `test("ribbon: header houses title, status, scan, and controls on a single line without wrapping")` (line 330):
     Renders with default selection (`"all"`).
   - `test("status badge: OptionsArbitrage renders standardized status badge and never renders Engine")` (line 345):
     Renders with default selection (`"all"`) and expects `/Options Strategies<\/span>[\s\S]*?title="Strategy bot status: Stopped"[\s\S]*?>Stopped<\/span>/`.
2. `ChallengerR3R4R5.test.ts`:
   - Lines 140–152: renders `OptionsArbitrage` with default selection (`"all"`) and asserts `/<span[^>]*>Options Strategies<\/span>\s*<span[^>]*title="Strategy bot status: (Active|Paused|Reducing|Stopped)"[^>]*>[\s\S]*?<\/span>/`.

#### Recommended Solution for Test Interoperability:
1. Add optional prop `initialSelected?: string;` to `OptionsArbitrageProps` (line 204), defaulting to `"all"`:
   ```tsx
   interface OptionsArbitrageProps {
     ...
     initialSelected?: string;
   }
   ```
   In `OptionsArbitrage`:
   ```tsx
   const [selected, setSelected] = useState<string>(initialSelected);
   ```
2. In `OptionsArbitrage.test.ts`:
   - Update tests targeting the ribbon/scan controls to pass `initialSelected: "parity"` (e.g. testing the Put–Call Parity substrategy ribbon).
   - Add a test verifying that when `initialSelected: "all"`, the ribbon is completely omitted from the HTML:
     ```ts
     test("ribbon: omitted when All Strategies is selected", () => {
       const html = renderToStaticMarkup(
         React.createElement(OptionsArbitrage, {
           executionLogs: [],
           onRefreshAll: noop,
           optionsVenueAvailable: true,
           initialSelected: "all",
         })
       );
       assert.doesNotMatch(html, /<span[^>]*>Options Strategies<\/span>/);
       assert.doesNotMatch(html, /<button[^>]*>\s*<svg[\s\S]*?Scan\s*<\/button>/);
     });
     ```
3. In `ChallengerR3R4R5.test.ts`:
   Pass `initialSelected: "parity"` so that it tests the standardized status badge on the Options strategy ribbon when a substrategy is active.

### 3.2 Proposed Diff for `OptionsArbitrage.tsx`
```diff
--- a/frontend/src/components/OptionsArbitrage.tsx
+++ b/frontend/src/components/OptionsArbitrage.tsx
@@ -203,6 +203,7 @@ interface OptionsArbitrageProps {
   premiumOutlayUsd?: number | null;
   openPositions?: OpenPositionDto[];
+  initialSelected?: string;
 }
 
 const fmtUsd = (v: number): string =>
@@ -572,7 +573,8 @@ export const OptionsArbitrage: React.FC<OptionsArbitrageProps> = ({
   premiumOutlayUsd = null,
   openPositions: propOpenPositions = [],
+  initialSelected = "all",
 }) => {
-  const [selected, setSelected] = useState<string>("all");
+  const [selected, setSelected] = useState<string>(initialSelected);
   const [dirFilter, setDirFilter] = useState<"all" | "conversion" | "reversal">("all");
   const [opps, setOpps] = useState<OptionsScannedOpp[]>([]);
@@ -914,6 +916,7 @@ export const OptionsArbitrage: React.FC<OptionsArbitrageProps> = ({
         {/* Strategy Status ribbon — mirrors the other strategy pages:
             read-only params + phase badge left, configurator, scan and the
             substrategy bot's lifecycle buttons right. The "All" view stays
             monitor-only — per-substrategy controls need a selection. */}
+        {selected !== "all" && (
         <div className="shrink-0 px-4 py-3 flex items-center justify-between gap-3 bg-surface rounded-xl border border-surface-border shadow-2xs select-none">
           <div className="flex items-center gap-2 text-xs font-mono min-w-0">
             <span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1 shrink-0">
@@ -988,6 +991,7 @@ export const OptionsArbitrage: React.FC<OptionsArbitrageProps> = ({
           </div>
         </div>
+        )}
 
         {/* Per-substrategy summary cards — filtered view of the monitor. */}
```

---

## 4. Requirement R8: Ticker Explorer Symbol Quote Display

### 4.1 File Location & Line Analysis
- **Target File**: `frontend/src/components/TickerExplorer.tsx`
- **Total Lines**: 842 lines

#### Observation 1: TickerRow Symbol Rendering
- **Lines 151–157**:
```tsx
<div className="flex items-center space-x-1.5">
  <span className={`font-bold text-xs font-sans tracking-tight ${hasIssue ? "text-loss font-black" : "text-slate-900"}`}>
    {base}
    {/* Distinct stablecoin quotes are distinct instruments (XRP-USDC
        vs XRP-USDT trade different venue sets) — the quote suffix is
        what keeps their rows visually distinguishable. */}
    <span className="text-muted font-semibold">-{quote}</span>
  </span>
  <span className="text-[9px] font-mono px-1.5 py-px rounded bg-slate-100 text-slate-600 font-bold border border-slate-200/60">
    Perp
  </span>
```
- **Context in Component**:
  - `TickerRowProps` receives `symbol`, `base`, `quote`.
  - For universal USD pairs (e.g. `APT-USD`), `base` is `"APT"` and `quote` is `"USD"`.
  - Currently it always renders `-{quote}`, resulting in `APT-USD`.
  - For non-USD quote pairs (e.g. `XRP-USDC`, `XRP-USDT`), `quote` is `"USDC"` or `"USDT"`.
- **Requirement R8**:
  "hide the `-USD` quote label when the quote is the universal USD, displaying only the base symbol (e.g. `APT` instead of `APT-USD`). Keep non-USD quote suffixes visible."
- **Action**:
  Update line 156 to conditionally render the quote suffix:
  ```tsx
  {quote && quote.toUpperCase() !== "USD" && (
    <span className="text-muted font-semibold">-{quote}</span>
  )}
  ```

#### Observation 2: Test Impact on `TickerExplorer.test.ts`
- **File**: `frontend/src/components/TickerExplorer.test.ts`
- Existing tests (lines 23–100):
  - Line 23: `quote-variant instruments render distinct rows labelled by quote`: Tests `XRP-USDC` and `XRP-USDT`. Both have non-USD quotes (`USDC` and `USDT`), which will still render `-USDC` and `-USDT`.
  - Line 40: `duplicate symbols...`: Tests `ZEC-USDC`.
  - Line 55: `net profit in dollars and bps...`: Tests `APT-USDC`.
- None of the existing tests use `quote: "USD"`. All existing tests pass completely with zero regressions.
- **New Test Addition**:
  Add an explicit test assertion:
  ```ts
  test("symbol quote display: hides -USD quote suffix for USD tickers while preserving non-USD quotes", () => {
    const html = renderToStaticMarkup(
      React.createElement(TickerExplorer, {
        ...baseProps,
        availableTickers: [
          ticker("APT-USD", "APT", "USD", ["HL", "LGT"]),
          ticker("APT-USDC", "APT", "USDC", ["HL", "LGT"]),
        ],
      }),
    );
    // USD ticker must render base 'APT' without '-USD' suffix
    assert.match(html, /<span[^>]*>APT<\/span>/);
    assert.doesNotMatch(html, />-USD</);
    // Non-USD ticker must retain its '-USDC' suffix
    assert.match(html, /APT<span[^>]*>-USDC</);
  });
  ```

### 4.2 Proposed Diff for `TickerExplorer.tsx`
```diff
--- a/frontend/src/components/TickerExplorer.tsx
+++ b/frontend/src/components/TickerExplorer.tsx
@@ -153,5 +153,7 @@ const TickerRow: React.FC<TickerRowProps> = React.memo(
               {/* Distinct stablecoin quotes are distinct instruments (XRP-USDC
                   vs XRP-USDT trade different venue sets) — the quote suffix is
-                  what keeps their rows visually distinguishable. */}
-              <span className="text-muted font-semibold">-{quote}</span>
+                  what keeps their rows visually distinguishable. Universal
+                  USD quotes omit the suffix, showing only the base symbol. */}
+              {quote && quote.toUpperCase() !== "USD" && (
+                <span className="text-muted font-semibold">-{quote}</span>
+              )}
             </span>
```

---

## 5. Verification & Test Plan

1. **Baseline Test Execution**:
   - Baseline command: `npm test` inside `frontend/`
   - Executed and verified: All 49 test suites (services, hooks, components) pass 100% currently.
2. **TypeScript Compilation Check**:
   - Run `npx tsc --noEmit` inside `frontend/`.
   - Identified pre-existing lint warning in `src/components/TickerConfigModal.tsx` (`Coins` and `Scale` unused imports). Neither `PredictionHub.tsx`, `OptionsArbitrage.tsx`, nor `TickerExplorer.tsx` has any errors.
3. **Dedicated Test Suites**:
   - `PredictionHub.test.ts`: Verify ribbon cleanups:
     1. Absence of `Prediction venues are scaffolds: no market data or order execution path`.
     2. Absence of `Max Slip` ribbon pill.
     3. Presence of `Search contract...` placeholder.
     4. Presence of active `Min Vol` pill with `10,000` or configured USD value and absence of `no venue volume data` or `opacity-60`.
   - `OptionsArbitrage.test.ts`:
     1. Verify ribbon is omitted when `selected === "all"`.
     2. Verify ribbon renders with `Put–Call Parity`, `Scan`, and controls when `selected === "parity"`.
   - `TickerExplorer.test.ts`:
     1. Verify `APT-USD` renders `APT` without `>-USD<`.
     2. Verify `APT-USDC` renders `APT` with `>-USDC<`.
