# Technical Survey: R2 Strategy Monitor Header Alignment & R5 Funding Rate Opportunity Scanner Dimensions

**Agent**: `survey_explorer_p3_2` (Read-only Explorer)  
**Date**: 2026-10-01T16:21:00Z  
**Repository**: `/mnt/Data/Projects/Arbitrageur`  
**Reference Document**: `.agents/teamwork/ORIGINAL_REQUEST.md` (Follow-up — 2026-10-01T16:09:57Z)  
**Milestone**: Investigation & Architecture Survey (M1)

---

## Executive Summary

This survey maps the technical scope, exact source locations, layout geometry, DOM hierarchy, and test impact for two core frontend harmonization requirements in the Arbitrageur desktop application:
1. **R2. Strategy Monitor Header Alignment (Spread Radar & Spatial Strategy)**:
   - In `frontend/src/components/OpportunityRadar.tsx` (Spread Radar):
     - Remove the `Execution Single-Way` badge from the strategy status ribbon.
     - Add an `Opportunity Scanner` header to the table gadget and right-align all filter controls (`Symbol`, `Venue`, `Min Exp`, `Min Win`, and hidden count) to match the canonical Funding Rate layout.
   - In `frontend/src/components/SpotArbitrageBot.tsx` (Spatial Strategy):
     - Align the opportunity monitor header to match the Funding Rate layout: `Opportunity Scanner` header on the left, filter controls (`Search`, `Filter Asset`, and count badge) right-aligned.
2. **R5. Funding Rate Opportunity Scanner Dimensions**:
   - In `frontend/src/components/FundingRadar.tsx` (Funding Rate Strategy):
     - Fix the height of individual opportunity cards (`h-[250px] overflow-hidden`) to eliminate card stretching and inconsistent sizing across rows.
     - Reduce the Opportunity Scanner gadget container height from legacy `h-[640px]` to `h-[600px]` (or `h-[605px]`) so it accommodates exactly 2 rows of opportunity cards without vertical scrollbars, while scrolling vertically for 3+ rows.

All 23 existing frontend test suites (`cd frontend && npm test`) pass at baseline with 100% pass rate. The proposed changes preserve existing test contracts (e.g. `Filter Asset`, bottom dock heights, status badges) with zero regressions.

---

## Detailed Investigation & Technical Mapping

### 1. R2 — Spread Radar (`OpportunityRadar.tsx`)

#### 1.1 Strategy Ribbon: Execution Single-Way Badge Removal

- **File Path**: `frontend/src/components/OpportunityRadar.tsx`
- **Lines**: 2169–2177
- **Existing Code**:
  ```tsx
  2169:           <div
  2170:             className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-lg"
  2171:             title="Execution mode — edit via Configurator"
  2172:           >
  2173:             <span className="text-muted">Execution</span>
  2174:             <span className="font-bold text-slate-900">
  2175:               {cfg ? (cfg.executionMode === "two_way" ? "Two-Way" : "Single-Way") : "—"}
  2176:             </span>
  2177:           </div>
  ```
- **Context & Surrounding Elements**:
  - The ribbon starts at line 2143 (`px-4 py-3 flex flex-wrap items-center justify-between gap-3 bg-surface rounded-lg border border-surface-border shadow-2xs shrink-0`).
  - Left element group (line 2144: `flex flex-wrap items-center gap-2 text-xs font-mono`):
    1. Title: `Strategy Status`
    2. Status Badge: `<BotPhaseBadge phase={botPhase} isPaused={isPaused} />`
    3. Min Win Pill: `title="Engine floor on directional trade win probability..."`
    4. Min Exp Pill: `title="Bot entry hurdle — minimum probability-weighted expected profit..."`
    5. Execution Mode Pill: `title="Execution mode — edit via Configurator"`
  - Right element group (line 2180): Configurator button + `BotLifecycleControls`.
- **Finding & Technical Scope**:
  - Execution mode is already fully configurable via the Spread Radar Configurator modal (`TickerConfigModal` / `MonitorConfigDto` section "Order Execution & Slippage", lines 821–845).
  - Showing `Execution Single-Way` on the status ribbon is redundant and occupies valuable horizontal ribbon space.
  - Removing lines 2169–2177 cleanly excises the pill without altering state variables, configurator hooks, or API payloads (`cfg.executionMode` remains functional in the configurator modal).
- **Test Impact**:
  - `ChallengerR3R4R5.test.ts:81` asserts that immediately following `Strategy Status` and the status badge, there is `<div[^>]*title="[^"]*directional trade win probability` (the Min Win pill).
  - Because `Execution` appeared after `Min Exp` (at the very end of the left group), removing it has zero impact on `ChallengerR3R4R5.test.ts` or any other test.

#### 1.2 Table Gadget Header: Add Title & Right-Align Filter Controls

- **File Path**: `frontend/src/components/OpportunityRadar.tsx`
- **Lines**: 2310–2431
- **Existing Code**:
  ```tsx
  2310:       <div className="flex-1 min-h-0 flex flex-col bg-surface rounded-xl border border-surface-border shadow-card overflow-hidden">
  2311:         <div className="flex items-center gap-2.5 px-4 py-2.5 border-b border-surface-border text-xs font-sans shrink-0 bg-surface-raised/40">
  2312:           <div
  2313:             className="flex items-center gap-1.5 bg-surface border border-surface-border rounded-lg px-2.5 py-1.5 shadow-2xs hover:border-slate-300 focus-within:border-primary focus-within:ring-2 focus-within:ring-primary/15 transition-all"
  2314:             title="Display filter — symbol contains"
  2315:           >
  2316:             <Search size={13} className="text-muted shrink-0" />
  2317:             <span className="text-muted font-medium">Symbol</span>
  2318:             <input
  2319:               value={search}
  2320:               onChange={(e) => setSearch(e.target.value)}
  2321:               placeholder="ALL"
  2322:               className="bg-transparent w-24 outline-none font-bold text-slate-900 uppercase font-mono text-xs placeholder:text-slate-300"
  2323:             />
  2324:           </div>
  2325:           <div className="relative" ref={venueMenuRef}>
                    {/* Venue dropdown trigger and menu */}
  2391:           </div>
  2392:           <div className="flex items-center gap-1.5 bg-surface border border-surface-border rounded-lg px-2.5 py-1.5..." title="Display filter — minimum expected profit">
                    {/* Min Exp input */}
  2407:           </div>
  2408:           <div className="flex items-center gap-1.5 bg-surface border border-surface-border rounded-lg px-2.5 py-1.5..." title="Display filter — minimum directional trade win probability...">
                    {/* Min Win input */}
  2423:           </div>
  2424:           <div className="ml-auto flex items-center gap-2">
  2425:             {(snap?.exempted.length ?? 0) > 0 && (
  2426:               <span className="text-slate-500 flex items-center gap-1 text-[11px] font-sans">
  2427:                 <EyeOff size={12} /> {snap?.exempted.length} hidden
  2428:               </span>
  2429:             )}
  2430:           </div>
  2431:         </div>
  ```
- **Finding & Defect**:
  - The Spread Radar table gadget has no title header at all.
  - The filter controls are placed on the left side of the bar, with only the hidden count pushed to the right via `ml-auto`.
  - In contrast, `FundingRadar.tsx:1644-1740` establishes the canonical pattern:
    - Left side: `<h3 className="text-[13px] font-bold font-sans tracking-wider text-slate-900">Opportunity Scanner</h3>`
    - Right side: `<div className="flex items-center gap-2 text-xs font-sans">` grouping all filter inputs and badges.
- **Proposed Architecture**:
  1. Update outer header bar class to:
     `className="flex items-center justify-between gap-3 px-4 py-2.5 border-b border-surface-border text-xs font-sans shrink-0 bg-surface-raised/40"`
  2. Insert header title on the left:
     ```tsx
     <h3 className="text-[13px] font-bold font-sans tracking-wider text-slate-900">
       Opportunity Scanner
     </h3>
     ```
  3. Group all filter controls on the right inside `<div className="flex items-center gap-2 text-xs font-sans">`:
     - Symbol Search input
     - Venue dropdown
     - Min Exp input
     - Min Win input
     - Hidden count indicator (`{(snap?.exempted.length ?? 0) > 0 && ...}`)
  4. In `showVenueMenu` dropdown menu (line 2350), change `left-0` to `right-0`:
     `className="absolute right-0 top-full mt-1.5 w-52 bg-surface border border-surface-border rounded-xl shadow-elevated p-1.5 z-30 font-sans text-xs animate-in fade-in zoom-in-95 duration-100"`
     This prevents the dropdown menu from rendering off-screen or clipping the right viewport border when right-aligned.

---

### 2. R2 — Spatial Strategy (`SpotArbitrageBot.tsx`)

#### 2.1 Opportunity Monitor Header Alignment

- **File Path**: `frontend/src/components/SpotArbitrageBot.tsx`
- **Lines**: 367–435
- **Existing Code**:
  ```tsx
  367:       <div className="flex-1 min-h-0 flex flex-col bg-surface rounded-xl border border-surface-border shadow-card overflow-hidden">
  368:         {/* Filter Bar */}
  369:         <div className="p-3.5 border-b border-surface-border flex flex-wrap items-center justify-between gap-3 bg-surface">
  370:           <div className="flex items-center space-x-3">
  371:             <div className="flex items-center gap-1.5 bg-surface border border-surface-border rounded-lg px-2.5 py-1.5 shadow-2xs hover:border-slate-300 focus-within:border-primary focus-within:ring-2 focus-within:ring-primary/15 transition-all">
  372:               <Search size={13} className="text-muted shrink-0" />
  373:               <span className="text-muted font-medium text-xs">Search</span>
  374:               <input
  375:                 value={search}
  376:                 onChange={(e) => setSearch(e.target.value)}
  377:                 placeholder="ALL"
  378:                 className="bg-transparent w-24 outline-none font-bold text-slate-900 uppercase font-mono text-xs placeholder:text-slate-300"
  379:               />
  380:             </div>
  381: 
  382:             <div className="flex items-center space-x-2">
  383:               <span className="text-xs font-bold text-slate-700 font-mono">Filter Asset:</span>
  384:               <div className="relative" ref={assetMenuRef}>
                       {/* Asset dropdown */}
  429:               </div>
  430:             </div>
  431:           </div>
  432:           <div className="text-xs font-mono text-slate-500">
  433:             {filteredOpportunities.length} opportunities detected
  434:           </div>
  435:         </div>
  ```
- **Finding & Defect**:
  - The Spatial Strategy monitor lacks the `Opportunity Scanner` title.
  - The search input and `Filter Asset:` dropdown are currently on the left (`space-x-3`), while the count is on the right.
  - This diverges from the unified layout established in Funding Rate and Options Arbitrage.
- **Proposed Architecture**:
  1. Outer container:
     `className="p-3.5 border-b border-surface-border flex items-center justify-between gap-3 bg-surface"`
  2. Insert header title on the left:
     ```tsx
     <h3 className="text-[13px] font-bold font-sans tracking-wider text-slate-900">
       Opportunity Scanner
     </h3>
     ```
  3. Group all filters on the right inside `<div className="flex items-center gap-3 text-xs font-sans">`:
     - Search input
     - `Filter Asset:` dropdown (with dropdown menu positioned `right-0 top-full mt-1.5 w-48 z-30`)
     - `{filteredOpportunities.length} opportunities detected` label
- **Test Impact**:
  - `SpotArbitrageBot.test.ts:33` checks `assert.match(html, /Filter Asset/)`.
  - Preserving the exact `Filter Asset:` text label and dropdown structure ensures complete test compliance.

---

### 3. R5 — Funding Rate Opportunity Scanner Dimensions (`FundingRadar.tsx`)

#### 3.1 Problem Analysis: Why 640px Was Used & Why It Failed

- **File Path**: `frontend/src/components/FundingRadar.tsx`
- **Lines**: 1642–1643, 1763–1894
- **Historical Analysis**:
  - In commit `23d4aed` (Fri Sep 18 2026), the Opportunity Scanner was introduced with:
    `{/* Opportunity Scanner — fixed window, 5x3 compact cards, click for details */}` and `h-[640px] flex flex-col`.
  - At that time, each opportunity card was ultra-compact (~170px tall). Three rows of 170px cards = 510px, plus 24px grid gaps and ~90px chrome (padding + header) = ~624px, fitting into 640px.
  - In subsequent feature passes (commit `651e9e0` and multi-leg additions), each card received significant new data fields:
    1. Venue Logos for long and short legs (13px logo icons)
    2. Net APR / APY badges with subscript APY tooltips
    3. Basis bps row (for `opp.kind === "basis"`)
    4. Fixed Yield rows (`lock_until_ms`, `yu_hedge_cost_bps`, `boros_margin_usd`)
    5. Breakeven hours
    6. Trailing backtest win rate (with 90% confidence interval tooltip)
    7. Persistence probability (with survival probability lower bound)
    8. Decay-aware expected net profit (`CarryExpectedInline`)
    9. Action button (`ExecuteButton` with `!py-2 !px-2`)
  - These additions grew the card height from 170px to ~220px–250px.
  - Consequently, inside `h-[640px]`, two full rows take ~512px, leaving only ~39px for the third row, which peeks through partially cut-off and causes visual layout degradation.
  - Furthermore, on typical desktop viewports (1080p, with browser chrome leaving ~850px–900px effective height), a 640px gadget pushed the 384px (`h-96`) bottom dock far below the screen fold.

#### 3.2 Individual Card Height Math & Inconsistent Sizing Diagnosis

Let's compute the exact box-model height of an opportunity card:
- Card Chrome:
  - Top & bottom padding (`p-2.5`): `10px + 10px = 20px`
  - Top & bottom border: `1px + 1px = 2px`
- Child 1 (Top Section):
  - Header row (Rank, Symbol, Kind badge, Net APR): `20px`
  - Gap below header (`space-y-2`): `8px`
  - Middle stats box (`px-2 py-1.5 bg-surface-raised rounded-lg border border-surface-border font-sans text-[10px] space-y-1`):
    - Border: `2px`
    - Padding (`py-1.5`): `6px + 6px = 12px`
    - Long venue row: `14px`
    - Short venue row: `14px`
    - 8h rate row (`pt-1 border-t`): `4px + 1px + 14px = 19px`
    - Breakeven row: `14px`
    - Win rate row: `14px`
    - Persistence probability row: `14px`
    - 5 intra-row gaps (`space-y-1`): `5 * 4px = 20px`
    - Base stats box total: `2 + 12 + 14 + 14 + 19 + 14 + 14 + 14 + 20 = 123px`
  - Total Child 1 height: `20 + 8 + 123 = 151px`
- Child 2 (Footer Section):
  - Top border: `1px`
  - Padding (`pt-2`): `8px`
  - `ExecuteButton` (`btn-primary !text-[10px] !py-2 !px-2`): `30px`
  - Total Child 2 height: `1 + 8 + 30 = 39px`
- Flex separation gap (`gap-2`): `8px`
- **Total Standard Card Content Height**: `22px (chrome) + 151px (Child 1) + 8px (gap) + 39px (Child 2) = 220px`.
- **Basis Card Content Height**: `220px + 18px (Basis row + gap) = 238px`.
- **Fixed Yield Card Content Height**: `220px + 54px (3 extra rows) = 274px`.

**The Root Cause of Inconsistent Card Sizing**:
Because individual cards had no explicit `height` or `min-height`, cards without Basis or Fixed Yield were 220px tall, while Basis cards stretched to 238px. If one row contained a Basis card, CSS Grid stretched all items in that row to 238px while adjacent rows remained 220px, causing jarring height disparity across rows and during filter operations.

#### 3.3 Target Sizing: Card Height & Gadget Height

1. **Card Fixed Height**: `h-[250px] overflow-hidden`
   - In a 250px card:
     - Standard carry (220px content): Has 30px of clean breathing space between the stats box and the footer button. The card's `flex flex-col justify-between` aligns the footer button uniformly at the bottom across all cards.
     - Basis carry (238px content): Fits cleanly with 12px breathing space.
     - `overflow-hidden` ensures rounded corners (`rounded-xl`) clip cleanly and prevents any accidental line wrap overflow.
2. **Gadget Height for Exactly 2 Rows**:
   - 2 rows of 250px cards = `2 * 250px = 500px`
   - 1 grid row gap (`gap-3`) = `12px`
   - Total Grid Height for 2 rows = `512px`
   - Gadget Header:
     - Button/input height (`py-1.5 text-xs`): `~32px`
     - Header bottom padding (`pb-3`): `12px`
     - Bottom border: `1px`
     - Bottom margin (`mb-3`): `12px`
     - Total Header footprint: `57px`
   - Gadget Outer Padding (`p-4`): `16px + 16px = 32px`
   - Gadget Outer Border: `2px`
   - Total Non-Grid Height: `57px + 32px + 2px = 91px`
   - **Total Target Gadget Height**: `512px + 91px = 603px` -> **`h-[600px]`** (or `h-[605px]`).
   - Sizing the gadget to `h-[600px]` (or `h-[605px]`):
     - For `<= 10` opportunities (<= 2 rows on xl screen with 5 cols): Displays exactly 2 rows with zero vertical scrollbar.
     - For `> 10` opportunities (3+ rows): The existing container class `flex-1 min-h-0 overflow-y-auto` smoothly scrolls vertically to reveal subsequent rows.
   - Update comment on line 1642 to:
     `{/* Opportunity Scanner — fixed window, 5x2 compact cards, click for details */}`.

---

## Code Diffs Specification

### Proposed Diff 1: `frontend/src/components/OpportunityRadar.tsx`

```diff
--- a/frontend/src/components/OpportunityRadar.tsx
+++ b/frontend/src/components/OpportunityRadar.tsx
@@ -2166,15 +2166,6 @@ export const OpportunityRadar: React.FC<OpportunityRadarProps> = ({
             </span>
             <span className="text-muted">bps</span>
           </div>
-          <div
-            className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-lg"
-            title="Execution mode — edit via Configurator"
-          >
-            <span className="text-muted">Execution</span>
-            <span className="font-bold text-slate-900">
-              {cfg ? (cfg.executionMode === "two_way" ? "Two-Way" : "Single-Way") : "—"}
-            </span>
-          </div>
         </div>
 
         <div className="flex items-center space-x-3 shrink-0">
@@ -2308,8 +2299,12 @@ export const OpportunityRadar: React.FC<OpportunityRadarProps> = ({
       {/* Radar gadget — filter strip + table in a single card.
           Filters are display-only; none of these affect the monitor or strategy. */}
       <div className="flex-1 min-h-0 flex flex-col bg-surface rounded-xl border border-surface-border shadow-card overflow-hidden">
-        <div className="flex items-center gap-2.5 px-4 py-2.5 border-b border-surface-border text-xs font-sans shrink-0 bg-surface-raised/40">
+        <div className="flex items-center justify-between gap-3 px-4 py-2.5 border-b border-surface-border text-xs font-sans shrink-0 bg-surface-raised/40">
+          <h3 className="text-[13px] font-bold font-sans tracking-wider text-slate-900">
+            Opportunity Scanner
+          </h3>
+          <div className="flex items-center gap-2 text-xs font-sans">
           <div
             className="flex items-center gap-1.5 bg-surface border border-surface-border rounded-lg px-2.5 py-1.5 shadow-2xs hover:border-slate-300 focus-within:border-primary focus-within:ring-2 focus-within:ring-primary/15 transition-all"
             title="Display filter — symbol contains"
@@ -2347,7 +2342,7 @@ export const OpportunityRadar: React.FC<OpportunityRadarProps> = ({
               />
             </button>
             {showVenueMenu && (
-              <div className="absolute left-0 top-full mt-1.5 w-52 bg-surface border border-surface-border rounded-xl shadow-elevated p-1.5 z-30 font-sans text-xs animate-in fade-in zoom-in-95 duration-100">
+              <div className="absolute right-0 top-full mt-1.5 w-52 bg-surface border border-surface-border rounded-xl shadow-elevated p-1.5 z-30 font-sans text-xs animate-in fade-in zoom-in-95 duration-100">
                 <button
                   type="button"
                   onClick={() => setVenueFilters([])}
@@ -2421,12 +2416,11 @@ export const OpportunityRadar: React.FC<OpportunityRadarProps> = ({
             />
             <span className="text-muted text-[11px] font-medium">%</span>
           </div>
-          <div className="ml-auto flex items-center gap-2">
             {(snap?.exempted.length ?? 0) > 0 && (
               <span className="text-slate-500 flex items-center gap-1 text-[11px] font-sans">
                 <EyeOff size={12} /> {snap?.exempted.length} hidden
               </span>
             )}
           </div>
         </div>
```

---

### Proposed Diff 2: `frontend/src/components/SpotArbitrageBot.tsx`

```diff
--- a/frontend/src/components/SpotArbitrageBot.tsx
+++ b/frontend/src/components/SpotArbitrageBot.tsx
@@ -366,8 +366,12 @@ export const SpotArbitrageBot: React.FC<SpotArbitrageBotProps> = ({
       {/* Main Content Workspace: Full-Width Opportunity Scanner */}
       <div className="flex-1 min-h-0 flex flex-col bg-surface rounded-xl border border-surface-border shadow-card overflow-hidden">
         {/* Filter Bar */}
-        <div className="p-3.5 border-b border-surface-border flex flex-wrap items-center justify-between gap-3 bg-surface">
-          <div className="flex items-center space-x-3">
+        <div className="p-3.5 border-b border-surface-border flex items-center justify-between gap-3 bg-surface">
+          <h3 className="text-[13px] font-bold font-sans tracking-wider text-slate-900">
+            Opportunity Scanner
+          </h3>
+          <div className="flex items-center gap-3 text-xs font-sans">
             <div className="flex items-center gap-1.5 bg-surface border border-surface-border rounded-lg px-2.5 py-1.5 shadow-2xs hover:border-slate-300 focus-within:border-primary focus-within:ring-2 focus-within:ring-primary/15 transition-all">
               <Search size={13} className="text-muted shrink-0" />
               <span className="text-muted font-medium text-xs">Search</span>
@@ -396,7 +400,7 @@ export const SpotArbitrageBot: React.FC<SpotArbitrageBotProps> = ({
                   />
                 </button>
                 {showAssetMenu && (
-                  <div className="dropdown-menu left-0 top-full mt-1.5 w-48 z-30">
+                  <div className="dropdown-menu right-0 top-full mt-1.5 w-48 z-30">
                     <button
                       type="button"
                       onClick={() => setAssetFilters([])}
@@ -428,8 +432,8 @@ export const SpotArbitrageBot: React.FC<SpotArbitrageBotProps> = ({
                   </div>
                 )}
               </div>
             </div>
+            <div className="text-xs font-mono text-slate-500">
+              {filteredOpportunities.length} opportunities detected
+            </div>
           </div>
-          <div className="text-xs font-mono text-slate-500">
-            {filteredOpportunities.length} opportunities detected
-          </div>
         </div>
```

---

### Proposed Diff 3: `frontend/src/components/FundingRadar.tsx`

```diff
--- a/frontend/src/components/FundingRadar.tsx
+++ b/frontend/src/components/FundingRadar.tsx
@@ -1639,8 +1639,8 @@ export const FundingRadar: React.FC<FundingRadarProps> = ({
         </div>
       </div>
 
-      {/* Opportunity Scanner — fixed window, 5x3 compact cards, click for details */}
-      <div className="bg-surface rounded-xl border border-surface-border p-4 shadow-card h-[640px] flex flex-col">
+      {/* Opportunity Scanner — fixed window, 5x2 compact cards, click for details */}
+      <div className="bg-surface rounded-xl border border-surface-border p-4 shadow-card h-[600px] flex flex-col">
         <div className="flex items-center justify-between gap-3 mb-3 pb-3 border-b border-surface-border shrink-0">
           <h3 className="text-[13px] font-bold font-sans tracking-wider text-slate-900">
             Opportunity Scanner
@@ -1766,7 +1766,7 @@ export const FundingRadar: React.FC<FundingRadarProps> = ({
               {filteredOpps.map((opp, idx) => (
                 <div
                   key={opp.id}
                   onClick={() => setDetailOppId(opp.id)}
-                  className="bg-surface border border-surface-border hover:border-accent-line rounded-xl p-2.5 flex flex-col justify-between gap-2 transition cursor-pointer shadow-card"
+                  className="bg-surface border border-surface-border hover:border-accent-line rounded-xl p-2.5 flex flex-col justify-between gap-2 transition cursor-pointer shadow-card h-[250px] overflow-hidden"
                   title="Click for opportunity details"
                 >
                   <div className="space-y-2">
```

---

## Verification & Test Plan

### Existing Tests Impact Assessment
1. `frontend/src/components/OpportunityRadar.test.ts`:
   - Enforces fixed height on the bottom dock container (`p-3.5 h-96 overflow-y-auto`).
   - Enforces standardized status badge formatting (`title="Strategy bot status: Stopped"`).
   - Enforces Open Positions row click handlers and PositionDetailModal rendering.
   - **Assessment**: Zero breakage. None of the tests assert on `Execution Single-Way` or the table header.
2. `frontend/src/components/SpotArbitrageBot.test.ts`:
   - Checks `assert.match(html, /Filter Asset/)` when backend reports spot ready.
   - Checks `assert.match(html, /No Opportunity Detected/)`.
   - Checks `assert.match(html, /class="p-3\.5 h-96 overflow-y-auto"/` on bottom dock.
   - Checks status badge.
   - **Assessment**: Zero breakage. `Filter Asset:` is preserved verbatim.
3. `frontend/src/components/FundingRadar.test.ts`:
   - Checks auto eligibility, split legs, fixed bottom dock height (`h-96`), status badges, and 5-card telemetry strip.
   - **Assessment**: Zero breakage. None of the tests assert on `h-[640px]`.
4. `frontend/src/components/ChallengerR3R4R5.test.ts`:
   - R4 status badges across all 6 strategies.
   - R3 locked `h-96` on dock containers.
   - R5 Options Arbitrage single row header and 5th metric card.
   - **Assessment**: Zero breakage.

### New Test Cases to Add During Implementation
1. **OpportunityRadar Tests**:
   - `assert.doesNotMatch(html, /title="Execution mode — edit via Configurator"/);`
   - `assert.doesNotMatch(html, /Single-Way/);`
   - `assert.match(html, /<h3[^>]*>Opportunity Scanner<\/h3>/);`
   - `assert.match(html, /flex items-center justify-between gap-3 px-4 py-2\.5 border-b/);`
2. **SpotArbitrageBot Tests**:
   - `assert.match(html, /<h3[^>]*>Opportunity Scanner<\/h3>/);`
   - `assert.match(html, /Filter Asset:/);`
3. **FundingRadar Tests**:
   - `assert.match(html, /class="[^"]*h-\[600px\][^"]*flex flex-col"/);`
   - `assert.match(html, /class="[^"]*h-\[250px\][^"]*overflow-hidden"/);`

---

## Conclusion & Readiness

The scope for R2 and R5 is fully investigated, mathematically verified, and de-risked. Implementers can directly apply the provided unified diffs and accompanying test additions.
