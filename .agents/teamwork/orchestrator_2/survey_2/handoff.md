# Investigation & Implementation Plan: R2 (Strategy Filter Relocation & Options Unification) and R4 (Standardized Status Tag Across All Strategies)

## Executive Summary
This investigation covers Requirement R2 (relocating the strategy filter dropdown into the top header row of `NetPnlChartGadget.tsx` next to "P&L & Equity History", and unifying all options substrategies under a single "Options" entry) and Requirement R4 (implementing a standardized bot status badge displaying exactly one of `Active`, `Paused`, `Reducing`, or `Stopped` placed immediately right of the strategy title header across all 7 automated strategy views, with "Engine" removed from all status texts).

---

## 1. Observation

### 1.1 Requirement R2: Strategy Filter in NetPnlChartGadget & PortfolioMonitor
1. **Dropdown Location & Layout Container in `NetPnlChartGadget.tsx`**:
   - `frontend/src/components/NetPnlChartGadget.tsx`, lines 251-278:
     ```tsx
     <div className="flex items-center justify-between border-b border-surface-border pb-2.5 flex-wrap gap-2">
       <div className="min-w-0">
         <h2 className="text-[13px] font-mono tracking-wider text-slate-900 font-bold flex items-center space-x-2">
           <LineChart className="w-4 h-4 text-accent-strong" />
           <span>P&amp;L &amp; Equity History</span>
         </h2>
       </div>
       <div className="flex items-center gap-3 shrink-0">
         <Tab<"pnl" | "equity"> ... />
         {lastVal != null && ( ... )}
       </div>
     </div>
     ```
   - Lines 280-301: The filter dropdown is currently isolated in an extra second row *below* the card header:
     ```tsx
     {tab === "pnl" && (
       <div className="flex items-center gap-2 flex-wrap">
         <select
           value={category}
           onChange={(e) => setCategory(e.target.value)}
           className={selectClass}
           aria-label="Filter P&L by strategy category"
           title="Realized P&L per strategy family; External ops = deposits/withdrawals/transfers no strategy produced"
         >
           <option value="all">All strategies</option>
           {categories.map((c) => (
             <option key={c} value={c}>
               {c}
             </option>
           ))}
           <option value={EXTERNAL_CATEGORY}>{EXTERNAL_CATEGORY}</option>
         </select>
         <span className="ml-auto text-[10px] font-mono text-slate-400 tabular-nums">
           {pnlPoints.length} point{pnlPoints.length === 1 ? "" : "s"}
         </span>
       </div>
     )}
     ```
   - Observation: When `tab === "pnl"`, a full extra row is rendered underneath the header divider, causing unnecessary vertical space usage. Moving `<select>` into the top bar next to `<h2>` places it directly on the card header.

2. **Options Substrategies in `NetPnlChartGadget.tsx`**:
   - Lines 18-21:
     ```tsx
     export const KNOWN_PNL_CATEGORIES: string[] = [
       "Options",
       ...Object.values(STRATEGY_DISPLAY_NAMES),
     ];
     ```
   - In `frontend/src/types/tickers.ts`, lines 76-96:
     `STRATEGY_DISPLAY_NAMES` contains:
     - `options_parity: "Options Parity"`
     - `options_box_spread: "Box Spread"`
     - `options_jelly_roll: "Jelly Roll"`
     - `options_convexity: "Convexity"`
   - Consequently, `KNOWN_PNL_CATEGORIES` populates the dropdown with:
     `"Options"`, `"Simple Spread"`, `"Spread Radar"`, `"Funding Rate"`, `"Spatial"`, `"Prediction Market"`, `"Statistical"`, `"Triangle"`, `"Options Parity"`, `"Box Spread"`, `"Jelly Roll"`, `"Convexity"`, `"Unattributed"`, `"External (manual)"`.
   - In `NetPnlChartGadget.tsx`, lines 41-58, filtering uses:
     ```tsx
     (category === "all" || strategyCategory(o.strategyId) === category)
     ```
   - In `frontend/src/components/TradedOpportunitiesGadget.tsx`, lines 160-163:
     ```tsx
     export const strategyCategory = (strategyId: string): string =>
       strategyId.startsWith("options_")
         ? "Options"
         : (STRATEGY_DISPLAY_NAMES[strategyId] ?? strategyId);
     ```
   - Observation: Because `strategyCategory` maps all `options_*` to `"Options"`, any selection in the dropdown of `"Options Parity"`, `"Box Spread"`, `"Jelly Roll"`, or `"Convexity"` yields 0 matching points (`strategyCategory(o.strategyId)` is `"Options"`, never `"Box Spread"`). The dropdown thus displays non-functional, ununified substrategies.

3. **Strategy Selector in `PortfolioMonitor.tsx`**:
   - `frontend/src/components/PortfolioMonitor.tsx`, lines 217-220 & lines 436-452:
     ```tsx
     const strategyOptions = useMemo(
       () => [...new Set(metrics.map((m) => m.strategy_id))].sort(),
       [metrics],
     );
     ```
     Renders:
     ```tsx
     <select value={strategyFilter} onChange={(e) => setStrategyFilter(e.target.value)}>
       <option value="all">All</option>
       {strategyOptions.map((id) => (
         <option key={id} value={id}>{formatStrategyName(id)}</option>
       ))}
     </select>
     ```
   - When options metrics exist in `metrics`, individual substrategy IDs (e.g. `options_parity`) appear separately.

---

### 1.2 Requirement R4: Standardized Status Tag Across All Strategies
1. **Existing `BotPhaseBadge` Implementation**:
   - `frontend/src/components/BotLifecycleControls.tsx`, lines 35-44:
     ```tsx
     export const BotPhaseBadge: React.FC<{ phase: BotPhase }> = ({ phase }) => {
       const label =
         phase === "reduce_only" ? "Reduce-Only" : phase === "stopping" ? "Stopping" : null;
       if (!label) return null;
       return (
         <span className="text-xs font-mono font-bold px-2 py-0.5 rounded-md bg-warn-dim text-warn-strong border border-warn-border">
           {label}
         </span>
       );
     };
     ```
   - `frontend/src/components/BotLifecycleControls.test.ts`, lines 75-89 verifies that `BotPhaseBadge` only labels `reduce_only` and `stopping`, and returns `""` (empty) for `running` and `idle`.
   - Observation: `BotPhaseBadge` does NOT render when the bot is `running`, `paused`, or `idle`.

2. **Ad-Hoc Status Tags and "Engine" Mentions in Strategy Ribbons**:
   - `frontend/src/components/TriangularArbitrage.tsx`, lines 391-395:
     ```tsx
     {isPaused && (
       <span className="badge-status-warn">
         Engine Paused
       </span>
     )}
     ```
     This tag is located *after* all parameter tags (Trade Size, Min Profit, Max Slippage), not immediately to the right of the strategy title.
   - `frontend/src/components/OptionsArbitrage.tsx`, line 873:
     ```tsx
     {isPaused && <span className="badge-status-warn">Engine Paused</span>}
     ```
     Also placed *after* all parameter tags.
   - Observation: Status badges contain the word "Engine", are conditionally appended at the far end of ribbons rather than after the header title, and fail to report `Active` or `Stopped`.

3. **Status Tag Locations across All 7 Strategy Pages**:
   - **Spread Radar** (`frontend/src/components/OpportunityRadar.tsx:1403-1405`):
     `<span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1">Strategy Status</span>`
     `<BotPhaseBadge phase={botPhase} />`
     Followed by `Min Win`, `Min Exp`, `Execution` parameter tags.
   - **Funding Rate** (`frontend/src/components/FundingRadar.tsx:1304-1307`):
     `<span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1">Strategy Status</span>`
     `<BotPhaseBadge phase={bot.phase} />`
     Followed by `Trade Size`, `Min APR` parameter tags.
   - **Simple Spread / Spatial** (`frontend/src/components/LiveMatrix.tsx:950-953`):
     `<span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1">Strategy Status</span>`
     `<BotPhaseBadge phase={bot.phase} />`
     Followed by reconciling check and parameter tags.
   - **Triangular Arbitrage** (`frontend/src/components/TriangularArbitrage.tsx:360-363` & `391-395`):
     `<span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1">Strategy Status</span>`
     `<BotPhaseBadge phase={bot.phase} />`
     Trailing: `{isPaused && <span className="badge-status-warn">Engine Paused</span>}`.
   - **Spatial Arbitrage** (`frontend/src/components/SpotArbitrageBot.tsx:187-190`):
     `<span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1">Strategy Status</span>`
     `<BotPhaseBadge phase={bot.phase} />`
     Followed by `Trade Size`, `Min Net` parameter tags.
   - **Prediction Market** (`frontend/src/components/PredictionHub.tsx:207-210`):
     `<span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1">Strategy Status</span>`
     `<BotPhaseBadge phase={bot.phase} />`
     Followed by `Min Net`, `Min Vol`, `Max Slip` parameter tags.
   - **Options Arbitrage** (`frontend/src/components/OptionsArbitrage.tsx:828-831` & `873`):
     `<span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1">{selDef ? selDef.name : "Options Strategies"}</span>`
     `{botSubstrategy && <BotPhaseBadge phase={botPhase} />}`
     Trailing: `{isPaused && <span className="badge-status-warn">Engine Paused</span>}`.
   - All 7 strategy components receive `isPaused: boolean` as a prop from `App.tsx` (`isPaused = systemStatus?.is_paused ?? false`).

---

## 2. Logic Chain

### 2.1 R2 Logic Chain: Filter Relocation & Options Unification
1. *From Obs 1.1.1*: The filter dropdown in `NetPnlChartGadget.tsx` sits inside `{tab === "pnl" && (<div className="flex items-center gap-2 flex-wrap">...</div>)}` on a separate line below the card header.
2. *From Obs 1.1.1*: The card header already has a left flex container `<div className="min-w-0"><h2 ...>P&amp;L &amp; Equity History</h2></div>`.
3. *Inference*: Changing the left container to `<div className="flex items-center gap-3 min-w-0 flex-wrap">` and moving `{tab === "pnl" && (<select ... />)}` immediately to the right of the `<h2>` title directly fulfills: *"move the strategy filter dropdown from below the card header up into the top bar of the card directly to the right of the 'P&L & Equity History' title"*. The second row can then be deleted, saving vertical layout space.
4. *From Obs 1.1.2*: `KNOWN_PNL_CATEGORIES` contains `...Object.values(STRATEGY_DISPLAY_NAMES)`, which includes `"Options Parity"`, `"Box Spread"`, `"Jelly Roll"`, `"Convexity"`.
5. *From Obs 1.1.2*: Filtering by any of these four substrategies produces 0 matches because `realizedSeries` checks `strategyCategory(o.strategyId) === category`, and `strategyCategory` turns all `options_*` into `"Options"`.
6. *Inference*: Filtering `KNOWN_PNL_CATEGORIES` so that keys starting with `options_` are excluded ensures `"Options"` is the sole options entry. When "Options" is selected, `realizedSeries(opps, "Options")` matches all options trades across all substrategies.
7. *From Obs 1.1.3*: In `PortfolioMonitor.tsx`, `strategyOptions` and the strategy filter should also unify any `options_*` into `"options"` (with label `"Options"`) and filter metrics where `m.strategy_id === "options" || m.strategy_id.startsWith("options_")`, ensuring consistent unified options behavior across the portfolio dashboard.

### 2.2 R4 Logic Chain: Standardized Status Tag
1. *From Obs 1.2.1*: `BotPhaseBadge` returns `null` for `running`, `paused`, and `idle`.
2. *From Obs 1.2.2*: "Engine Paused" appears ad-hoc in `TriangularArbitrage.tsx` and `OptionsArbitrage.tsx` after the parameter tags.
3. *From Requirement R4*: All strategy headers must display a standardized bot status badge positioned to the right of the strategy title header, before any parameter tags; the word "Engine" must be removed; and the badge must display exactly one of four statuses: `Active`, `Paused`, `Reducing`, or `Stopped`.
4. *Inference for Status Resolution*:
   - If `phase === "running"`: if `isPaused`, return `Paused`; else `Active`.
   - If `phase === "paused"`: return `Paused`.
   - If `phase === "reduce_only"` or `phase === "stopping"`: return `Reducing`.
   - If `phase === "idle"` (or stopped): return `Stopped`. (Per R1, stopped bots remain stopped, never paused, on halt/emergency reset).
5. *Inference for UI Tokens*:
   - Pill radius: `rounded-full text-xs font-mono font-bold px-2 py-0.5 shadow-2xs inline-flex items-center gap-1.5`.
   - `Active`: `bg-profit-dim text-profit border-profit-border` with `<span className="w-1.5 h-1.5 rounded-full bg-profit" />`.
   - `Paused`: `bg-warn-dim text-warn-strong border-warn-border` with `<span className="w-1.5 h-1.5 rounded-full bg-warn" />`.
   - `Reducing`: `bg-warn-dim text-warn-strong border-warn-border` with `<span className="w-1.5 h-1.5 rounded-full bg-warn animate-pulse" />`.
   - `Stopped`: `bg-surface-raised text-slate-500 border-surface-border` with `<span className="w-1.5 h-1.5 rounded-full bg-slate-400" />`.
6. *Inference for Component Placement*:
   - In all 7 strategy headers, place `<BotPhaseBadge phase={...} isPaused={isPaused} />` (or `<StrategyStatusBadge ... />`) immediately after the strategy title header, before any parameter pills.
   - Delete the trailing `{isPaused && <span className="badge-status-warn">Engine Paused</span>}` in `TriangularArbitrage.tsx` and `OptionsArbitrage.tsx`.

---

## 3. Caveats
1. **StatArbLab Tab**:
   `StatArbLab.tsx` is an interactive analytical research workspace (2D Kalman filter & Bertram solver) with no automated execution bot or `useBotLifecycle` hook. It does not possess a strategy status ribbon or automated bot parameters. No status tag is needed there.
2. **Options Arbitrage "All" Rail Tab**:
   In `OptionsArbitrage.tsx`, when "All" is selected in the rail (`selected === "all"`), `botSubstrategy` is `null`. The header should evaluate whether any substrategy bot is running/reducing/paused: if any bot is running, show `Active` (or `Paused` if `isPaused`); if any is reducing, show `Reducing`; if any is paused, show `Paused`; else show `Stopped`. This ensures the status badge is always present and never disappears.
3. **External Positions / Traded Opportunities**:
   `TradedOpportunitiesGadget.tsx` already uses `strategyCategory`, so its dropdown already groups all options under "Options". No regression or change is required there.

---

## 4. Conclusion & Implementation Plan

### 4.1 Plan for Requirement R2: Strategy Filter Relocation & Options Unification

#### File 1: `frontend/src/types/tickers.ts`
- Ensure `STRATEGY_DISPLAY_NAMES` includes `options: "Options"`.

#### File 2: `frontend/src/components/NetPnlChartGadget.tsx`
1. Update `KNOWN_PNL_CATEGORIES`:
   ```tsx
   export const KNOWN_PNL_CATEGORIES: string[] = [
     "Options",
     ...Object.keys(STRATEGY_DISPLAY_NAMES)
       .filter((k) => !k.startsWith("options_") && k !== "unattributed" && k !== "external")
       .map((k) => STRATEGY_DISPLAY_NAMES[k]),
   ].filter((c, i, arr) => arr.indexOf(c) === i);
   ```
2. Move `<select>` inside the top bar flex container directly beside `<h2>`:
   ```tsx
   <div className="flex items-center justify-between border-b border-surface-border pb-2.5 flex-wrap gap-2">
     <div className="flex items-center gap-3 min-w-0 flex-wrap">
       <h2 className="text-[13px] font-mono tracking-wider text-slate-900 font-bold flex items-center space-x-2 shrink-0">
         <LineChart className="w-4 h-4 text-accent-strong" />
         <span>P&amp;L &amp; Equity History</span>
       </h2>
       {tab === "pnl" && (
         <select
           value={category}
           onChange={(e) => setCategory(e.target.value)}
           className={selectClass}
           aria-label="Filter P&L by strategy category"
           title="Realized P&L per strategy family; External ops = deposits/withdrawals/transfers no strategy produced"
         >
           <option value="all">All strategies</option>
           {categories.map((c) => (
             <option key={c} value={c}>
               {c}
             </option>
           ))}
           <option value={EXTERNAL_CATEGORY}>{EXTERNAL_CATEGORY}</option>
         </select>
       )}
     </div>
     <div className="flex items-center gap-3 shrink-0">
       <Tab<"pnl" | "equity"> ... />
       {lastVal != null && ( ... )}
     </div>
   </div>
   ```
3. Remove lines 280-301 (`{tab === "pnl" && (<div className="flex items-center gap-2 flex-wrap">...</div>)}`).

#### File 3: `frontend/src/components/PortfolioMonitor.tsx`
1. Update `strategyOptions` to group any `id.startsWith("options_")` into `"options"`.
2. Update `visibleMetrics` and `visibleStrategyRows` so that when `strategyFilter === "options"`, all metrics and rows where `id.startsWith("options_") || id === "options"` match.

#### File 4: `frontend/src/components/NetPnlChartGadget.test.ts`
- Add assertions that:
  - `"Options"` is present in the rendered markup.
  - Substrategies (`"Options Parity"`, `"Box Spread"`, `"Jelly Roll"`, `"Convexity"`) are NOT present in the dropdown.
  - Dropdown markup is located within the header row.

---

### 4.2 Plan for Requirement R4: Standardized Status Tag Across All Strategies

#### File 5: `frontend/src/components/BotLifecycleControls.tsx`
1. Define the 4 standard statuses:
   ```tsx
   export type StrategyBotStatus = "Active" | "Paused" | "Reducing" | "Stopped";

   export function resolveBotStatus(
     phase?: BotPhase,
     isPaused = false,
   ): StrategyBotStatus {
     if (phase === "running") {
       return isPaused ? "Paused" : "Active";
     }
     if (phase === "paused") {
       return "Paused";
     }
     if (phase === "reduce_only" || phase === "stopping") {
       return "Reducing";
     }
     return "Stopped";
   }
   ```
2. Upgrade `BotPhaseBadge`:
   ```tsx
   export interface BotPhaseBadgeProps {
     phase?: BotPhase;
     isPaused?: boolean;
     status?: StrategyBotStatus;
   }

   export const BotPhaseBadge: React.FC<BotPhaseBadgeProps> = ({
     phase,
     isPaused = false,
     status,
   }) => {
     const currentStatus = status ?? resolveBotStatus(phase, isPaused);

     const config: Record<
       StrategyBotStatus,
       { label: string; dotClass: string; badgeClass: string }
     > = {
       Active: {
         label: "Active",
         dotClass: "bg-profit",
         badgeClass: "bg-profit-dim text-profit border-profit-border",
       },
       Paused: {
         label: "Paused",
         dotClass: "bg-warn",
         badgeClass: "bg-warn-dim text-warn-strong border-warn-border",
       },
       Reducing: {
         label: "Reducing",
         dotClass: "bg-warn animate-pulse",
         badgeClass: "bg-warn-dim text-warn-strong border-warn-border",
       },
       Stopped: {
         label: "Stopped",
         dotClass: "bg-slate-400",
         badgeClass: "bg-surface-raised text-slate-500 border-surface-border",
       },
     };

     const { label, dotClass, badgeClass } = config[currentStatus];

     return (
       <span
         className={`inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full text-xs font-mono font-bold border shadow-2xs ${badgeClass}`}
         title={`Strategy bot status: ${label}`}
       >
         <span className={`w-1.5 h-1.5 rounded-full ${dotClass}`} />
         {label}
       </span>
     );
   };

   export const StrategyStatusBadge = BotPhaseBadge;
   ```

#### File 6: `frontend/src/components/BotLifecycleControls.test.ts`
- Update unit tests to verify:
  - `phase="running"` without `isPaused` renders `Active` with `bg-profit-dim`.
  - `phase="running"` with `isPaused=true` renders `Paused` with `bg-warn-dim`.
  - `phase="paused"` renders `Paused`.
  - `phase="reduce_only"` and `phase="stopping"` render `Reducing`.
  - `phase="idle"` renders `Stopped`.
  - Text contains exactly one of the 4 words and NEVER contains `"Engine"`.

#### Strategy Views Header Upgrades:
1. `OpportunityRadar.tsx` (line 1405):
   `<BotPhaseBadge phase={botPhase} isPaused={isPaused} />`
2. `FundingRadar.tsx` (line 1307):
   `<BotPhaseBadge phase={bot.phase} isPaused={isPaused} />`
3. `LiveMatrix.tsx` (line 953):
   `<BotPhaseBadge phase={bot.phase} isPaused={isPaused} />`
4. `TriangularArbitrage.tsx` (line 363 & 391):
   `<BotPhaseBadge phase={bot.phase} isPaused={isPaused} />` immediately after `Strategy Status`. Remove lines 391-395 (`Engine Paused`).
5. `SpotArbitrageBot.tsx` (line 190):
   `<BotPhaseBadge phase={bot.phase} isPaused={isPaused} />`
6. `PredictionHub.tsx` (line 210):
   `<BotPhaseBadge phase={bot.phase} isPaused={isPaused} />`
7. `OptionsArbitrage.tsx` (line 831 & 873):
   Always render `<BotPhaseBadge phase={botPhase} isPaused={isPaused} status={...} />` right after `{selDef ? selDef.name : "Options Strategies"}`. Remove line 873 (`Engine Paused`).

---

## 5. Verification Method

### 5.1 Automated Test Execution Commands
1. **Frontend Tests**:
   ```bash
   cd /mnt/Data/Projects/Arbitrageur/frontend && npm test
   ```
   Must pass all tests (node --test with `scripts/test-components.mjs`).
2. **Backend Regression**:
   ```bash
   cargo test -p arbitrageur-desktop
   ```
   Ensures Tauri commands and backend integration remain pristine.
3. **Linter & Formatting**:
   ```bash
   cargo clippy --all-targets && cargo fmt --check
   ```

### 5.2 Specific Assertions to Verify
1. **R2 Invalidation Conditions**:
   - If the `<select>` in `NetPnlChartGadget.tsx` renders below the border divider, or on a second row when inspecting static HTML, R2 fails.
   - If selecting "Options" in the dropdown leaves `realizedSeries` empty when options trades exist, R2 fails.
   - If `"Options Parity"` or `"Box Spread"` appears in the dropdown options, R2 fails.
2. **R4 Invalidation Conditions**:
   - If any strategy header displays `"Engine Paused"` or `"Engine Active"`, R4 fails.
   - If any strategy header status badge displays anything other than `Active`, `Paused`, `Reducing`, or `Stopped`, R4 fails.
   - If any strategy header places the badge after parameter pills (e.g. at the ribbon tail), R4 fails.
   - If an unstarted bot displays no badge (empty element), R4 fails.
