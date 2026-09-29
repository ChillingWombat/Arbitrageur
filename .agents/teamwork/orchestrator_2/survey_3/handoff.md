# Survey & Investigation Report: R3 (Fixed Dock Heights) & R5 (Options Arbitrage Ribbon & Metrics)

**Author**: survey_explorer_3  
**Working Directory**: `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/orchestrator_2/survey_3/`  
**Parent Agent**: `79ca425f-52b2-482b-b97b-9d8092a384bd`  
**Date**: 2026-09-29  

---

## 1. Observation

### 1.1 R3 Dock Tab Heights in `OpportunityRadar.tsx`, `FundingRadar.tsx`, and `LiveMatrix.tsx`

#### A. `OpportunityRadar.tsx` (Lines 1776–1889)
```tsx
1779: <div className="bg-surface rounded-lg border border-surface-border shadow-2xs overflow-hidden">
1780:   <div className="flex items-stretch border-b border-surface-border bg-slate-50/80 select-none">
1781:     <div className="grid grid-cols-4 flex-1 divide-x divide-surface-border">
1782:       {(
1783:         [
1784:           { id: "position", label: "Current Positions", icon: Wallet, count: openPos.length },
1785:           { id: "history", label: "Execution History", icon: History, count: radarLogs.length },
1786:           { id: "opps", label: "Opportunity History", icon: Radar, count: null },
1787:           { id: "logs", label: "Log Monitor", icon: Terminal, count: radarLogs.length },
1788:         ] as const
1789:       ).map(({ id, label, icon: Icon, count }) => (
...
1817:   </div>
1818: 
1819:   <div className="p-3.5">
1820:     {dockTab === "position" ? (
1821:       <div className="max-h-96 overflow-y-auto">
1822:         {openPos.length === 0 ? (
1823:           <EmptyState
1824:             plain
1825:             title="No Open Radar Positions"
1826:             hint="Bot-managed and manual radar entries aggregate here with live deviation and unwind deadline."
1827:           />
1828:         ) : (
1829:           <div className="divide-y divide-surface-border">
...
1867:           </div>
1868:         )}
1869:       </div>
1870:     ) : dockTab === "history" ? (
1871:       <div className="max-h-96 overflow-y-auto">
1872:         <TickerTransactionHistory symbol="" logs={radarLogs} matchAllSymbols />
1873:       </div>
1874:     ) : dockTab === "opps" ? (
1875:       <div className="max-h-96 overflow-y-auto">
1876:         <TickerOpportunityHistory symbol="" />
1877:       </div>
1878:     ) : (
1879:       <div className="overflow-y-auto">
1880:         <TickerLogMonitor
1881:           symbol=""
1882:           namespace="spread-radar"
1883:           strategyName="Spread Radar"
1884:           executionLogs={radarLogs}
1885:           matchAllSymbols
1886:           isPaused={isPaused}
1887:         />
1888:       </div>
1889:     )}
1890:   </div>
1891: </div>
```
- **Direct observation of height jumping mechanism**:
  1. The container element at line 1819 is `<div className="p-3.5">`. It specifies **no fixed height** (`h-...`) and **no min-height** (`min-h-...`).
  2. Tab 1 (`position`): Uses `max-h-96 overflow-y-auto`. When `openPos.length === 0`, `<EmptyState>` has intrinsic height of ~120px. The dock container shrinks down to ~150px total.
  3. Tab 2 (`history`): Uses `max-h-96 overflow-y-auto`. If empty, renders EmptyState inside `TickerTransactionHistory` (~160px). If partially filled, it adopts intrinsic content height up to 384px.
  4. Tab 3 (`opps`): Uses `max-h-96 overflow-y-auto`. EmptyState or table adopts intrinsic height.
  5. Tab 4 (`logs`): Uses `<div className="overflow-y-auto">` with **no `max-h-96` at all**! Inside `TickerLogMonitor.tsx` (lines 350–465), there is a control bar (~42px), a table container with `max-h-[380px]`, and a summary footer (~28px). The total component height easily reaches 450px+.
  6. **Result**: Switching between tabs causes the dock container to abruptly jump between ~150px and ~450px, forcing the whole viewport and scroll position to jump and resize.

#### B. `FundingRadar.tsx` (Lines 1700–1816)
```tsx
1702: <div className="p-3.5 max-h-96 overflow-y-auto">
1703:   {dockTab === "position" ? (
1704:     <>
1705:     <FixedYieldPanel isLive={isLive} />
1706:     {fundingPositions.length === 0 ? (
1707:       <EmptyState ... />
...
1797:   ) : dockTab === "history" ? (
1798:     <TickerTransactionHistory symbol="" logs={fundingLogs} matchAllSymbols />
1799:   ) : dockTab === "opps" ? (
1800:     <TradedOpportunitiesTable ... />
1801:   ) : (
1802:     <TickerLogMonitor ... />
1803:   )}
1804: </div>
```
- Line 1702 applies `max-h-96 overflow-y-auto`. Because it is `max-height` and not fixed `height`, when `fundingPositions` is empty, or when switching to `history` with no logs, the container collapses to ~140px. Switching to `position` (with `FixedYieldPanel`) or `logs` expands it to 384px, causing the same layout jump.

#### C. `LiveMatrix.tsx` (Lines 1426–1454)
```tsx
1427: <div className="p-3.5">
1428:   {bottomTab === "history" ? (
1429:     <div className="max-h-96 overflow-y-auto">
...
1437:   ) : bottomTab === "opps" ? (
1438:     <div className="max-h-96 overflow-y-auto">
...
1441:   ) : (
1442:     <div className="overflow-y-auto">
1443:       <TickerLogMonitor ... />
```
- Demonstrates the identical pattern as `OpportunityRadar.tsx`.

---

### 1.2 R5 Options Arbitrage Ribbon & Metrics Reorganization in `OptionsArbitrage.tsx`

#### A. Existing Strategy Ribbon (Lines 826–929)
```tsx
826: <div className="ribbon-container">
827:   <div className="flex flex-wrap items-center gap-2 text-xs font-mono">
828:     <span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1">
829:       {selDef ? selDef.name : "Options Strategies"}
830:     </span>
831:     {botSubstrategy && <BotPhaseBadge phase={botPhase} />}
832:     {liveVisible && (
833:       <>
834:         <div className="ribbon-pill" title="Net-edge hurdle for reported setups">
835:           <span className="text-muted">Min Edge</span>
836:           <span className="font-bold text-slate-900 tabular-nums">{scanEdge}</span>
837:           <span className="text-muted">BPS</span>
838:         </div>
839:         <div className="ribbon-pill" title="Scan/entry size in base units">
840:           <span className="text-muted">Qty</span>
841:           <span className="font-bold text-slate-900 tabular-nums">{scanQty}</span>
842:         </div>
843:         {botSubstrategy && botCfg && (
844:           <>
845:             <div className="ribbon-pill" title="Bot poll cadence">
846:               <span className="text-muted">Poll</span>
847:               <span className="font-bold text-slate-900 tabular-nums">
848:                 {botCfg.pollIntervalSecs}
849:               </span>
850:               <span className="text-muted">s</span>
851:             </div>
852:             <div className="ribbon-pill" title="Max simultaneous managed positions">
853:               <span className="text-muted">Max Open</span>
854:               <span className="font-bold text-slate-900 tabular-nums">
855:                 {botCfg.maxOpenPositions}
856:               </span>
857:             </div>
858:           </>
859:         )}
860:       </>
861:     )}
862:     {premiumOutlayUsd != null && Math.abs(premiumOutlayUsd) > 0.005 && (
863:       <div
864:         className="ribbon-pill"
865:         title="Option premiums paid minus received at entry — financing held inside open packages, returned at close/expiry. Kept out of Net Profit."
866:       >
867:         <span className="text-muted">Premium Outlay</span>
868:         <span className="font-bold text-slate-900 tabular-nums">
869:           {fmtUsd(premiumOutlayUsd)}
870:         </span>
871:       </div>
872:     )}
873:     {isPaused && <span className="badge-status-warn">Engine Paused</span>}
874:     {isLive && (
875:       <span className="text-xs text-amber-400 border border-amber-500/40 rounded px-1.5 py-0.5 font-sans">
876:         Manual executes only
877:       </span>
878:     )}
879:   </div>
880:   <div className="flex items-center space-x-3 shrink-0">
881:     {(botError || flattenError) && (
882:       <span
883:         className="text-xs font-mono text-loss max-w-72 truncate"
884:         title={botError ?? flattenError ?? ""}
885:       >
886:         {botError ?? flattenError}
887:       </span>
888:     )}
889:     {botSubstrategy && (
890:       <button
891:         type="button"
892:         onClick={() => setShowBotConfig(true)}
893:         className="btn-secondary !text-xs !py-2 !px-2 font-sans cursor-pointer gap-2"
894:         title={`Configure the ${botSubstrategy.name} bot`}
895:       >
896:         <SlidersHorizontal className="w-3.5 h-3.5 text-accent-strong" />
897:         <span>Configurator</span>
898:       </button>
899:     )}
900:     {liveVisible && (
901:       <button
902:         type="button"
903:         onClick={() => void scan()}
904:         disabled={scanning || isPaused || !optionsVenueAvailable}
...
911:         className="btn-primary inline-flex items-center gap-1.5 !text-xs !py-2 !px-2.5 font-sans"
912:       >
913:         <RefreshCw className={`w-3.5 h-3.5 ${scanning ? "animate-spin" : ""}`} />
914:         {scanning ? "Scanning…" : "Scan"}
915:       </button>
916:     )}
917:     {botSubstrategy && (
918:       <BotLifecycleControls
919:         phase={botPhase}
920:         hasOpenPositions={openPositions > 0}
921:         disabled={botCfg == null}
922:         allowStartWithOpenPositions
923:         onStart={() => void handleBotMode("auto")}
924:         onPause={() => void handleBotMode("paused")}
925:         onReduceOnly={() => void handleBotMode("reduce_only")}
926:         onStop={() => void handleFlatten()}
927:       />
928:     )}
929:   </div>
930: </div>
```

- **Direct observations**:
  1. "Premium Outlay" is currently rendered at lines 862–872 as an inline parameter chip inside the strategy ribbon:
     `{premiumOutlayUsd != null && Math.abs(premiumOutlayUsd) > 0.005 && (<div className="ribbon-pill">...</div>)}`.
     It formats the value using `fmtUsd(premiumOutlayUsd)`.
  2. The ribbon container has class `ribbon-container`, defined in `frontend/src/index.css` line 181 as `@apply shrink-0 px-4 py-3 flex flex-wrap items-center justify-between gap-3 ...`.
  3. The left container uses `flex flex-wrap items-center gap-2`.
  4. There are up to 7 distinct chips on the left side: Title, Status Badge, Min Edge, Qty, Poll, Max Open, Premium Outlay, Paused badge, and "Manual executes only" pill.
  5. The right side has: Error message, Configurator button, Scan button, and the 4-button `BotLifecycleControls`.
  6. Because the total horizontal width exceeds standard viewport widths (e.g. 1024px–1280px once the 256px `SubstrategyRail` is subtracted), the `flex-wrap` triggers, causing the lifecycle controls or parameter tags to wrap onto a 2nd or 3rd line.

#### B. Existing 4 Metric Cards (Lines 932–975)
```tsx
932: <div className="grid grid-cols-2 sm:grid-cols-4 gap-4">
933:   {/* Card 1: Scanned Setups */}
934:   <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
935:     <div className="text-xs font-medium text-muted font-mono">Scanned Setups</div>
936:     <div className="text-lg font-bold text-slate-900 font-mono tabular-nums mt-0.5">
937:       {filteredOpps.length}
938:     </div>
939:     <div className="text-xs text-slate-500 mt-0.5">{scanned ? "latest scan" : "no scan yet"}</div>
940:   </div>
941:   {/* Card 2: Best Net Edge */}
942:   <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
943:     <div className="text-xs font-medium text-muted font-mono">Best Net Edge</div>
944:     <div className={`text-lg font-bold font-mono tabular-nums mt-0.5 ${bestEdge > 0 ? "text-profit" : "text-slate-900"}`}>
945:       {bestEdge === -Infinity ? "—" : fmtUsd(bestEdge)}
946:     </div>
947:     <div className="text-xs text-slate-500 mt-0.5">{filteredOpps.length > 0 ? "after fees + funding" : "no setups"}</div>
948:   </div>
949:   {/* Card 3: Executable */}
950:   <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
951:     <div className="text-xs font-medium text-muted font-mono">Executable</div>
952:     <div className="text-lg font-bold text-slate-900 font-mono tabular-nums mt-0.5">
953:       {executableCount}
954:     </div>
955:     <div className="text-xs text-slate-500 mt-0.5">passes hurdle at scan size</div>
956:   </div>
957:   {/* Card 4: Realized PnL */}
958:   <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
959:     <div className="text-xs font-medium text-muted font-mono">Realized PnL</div>
960:     <div className={`text-lg font-bold font-mono tabular-nums mt-0.5 ${realizedPnl >= 0 ? "text-profit" : "text-loss"}`}>
961:       {filteredLogs.length > 0 ? fmtUsd(realizedPnl) : "—"}
962:     </div>
963:     <div className="text-xs text-slate-500 mt-0.5">{filteredLogs.length} execution log{filteredLogs.length === 1 ? "" : "s"}</div>
964:   </div>
965: </div>
```
- **Direct observations**:
  1. The metric grid uses `grid grid-cols-2 sm:grid-cols-4 gap-4`.
  2. Exactly 4 metric cards exist: Scanned Setups, Best Net Edge, Executable, Realized PnL.
  3. Premium Outlay is not represented here, even though it is a portfolio cashflow/capital metric rather than an execution hurdle parameter.

---

## 2. Logic Chain

### 2.1 R3: Establishing Fixed Uniform Height for Bottom Dock Tabs

1. **Height jump diagnosis**:
   - In `OpportunityRadar.tsx`, `<div className="p-3.5">` allows the container to flex freely based on inner contents.
   - Inner tabs use `max-h-96` (`max-height: 24rem` = 384px) or unconstrained `overflow-y-auto`.
   - Empty state height: 120px; small transaction history: ~160px; full transaction history: 384px; log monitor: 450px+.
   - Switching tabs triggers browser reflow and changes document height, resulting in layout shift.

2. **Selecting the uniform height token**:
   - The codebase has consistently standardized on `h-96` (24rem / 384px) across `OpportunityRadar.tsx`, `FundingRadar.tsx`, and `LiveMatrix.tsx`.
   - Fixing the container to `h-96` (384px) with `overflow-y-auto` provides ample vertical height to view 6–8 positions or log entries, while keeping the overall viewport stable.
   - When content is smaller than 384px (such as `EmptyState`), `h-96` prevents collapsing.
   - When content is larger than 384px, `overflow-y-auto` provides smooth scrolling within the fixed boundary.

3. **Eliminating redundant nested scroll containers**:
   - Applying `h-96 overflow-y-auto` to the outer `<div className="p-3.5 h-96 overflow-y-auto">` means inner wrappers in each tab must NOT have redundant `max-h-96 overflow-y-auto`. Removing redundant inner wrappers prevents double scrollbar bugs and margin clipping.
   - Total dock panel height will be strictly:
     `36px (tab bar) + 1px (border) + 384px (content) + 28px (p-3.5 vertical padding) = 449px` across all tabs at all times.

---

### 2.2 R5: Options Arbitrage Reorganization & Single-Line Header

1. **Why "Premium Outlay" belongs in the Metrics Gadget**:
   - Premium Outlay represents net options financing (premiums paid minus received at entry) held in open options packages and returned at expiry or closing.
   - It is a financial performance/capital utilization metric, identical in character to "Realized PnL" and "Best Net Edge", rather than a trade configuration parameter like "Min Edge" or "Qty".
   - Removing it from the ribbon eliminates a long, variable-width parameter pill (`~140px`) from the header.

2. **Creating the 5th Metric Card**:
   - Design matching the existing 4 cards:
     - Header: `Premium Outlay` (mono font, muted)
     - Value: `premiumOutlayUsd != null && Math.abs(premiumOutlayUsd) > 0.005 ? fmtUsd(premiumOutlayUsd) : "—"` (bold mono, tabular nums)
     - Subtitle/Hint: `net financing in packages`
     - Tooltip: Explains that premium outlay is financing held in open packages, kept out of net profit.
   - Update grid container:
     `className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-4"`
     On wide desktop viewports (`lg:`), all 5 cards align neatly on 1 horizontal row. On tablet/mobile, they wrap gracefully (3 and 2).

3. **Single-Line Header Row Reorganization**:
   - Replace `flex flex-wrap` with `flex items-center justify-between gap-3` (with `flex-nowrap` or explicit single-row layout).
   - Pruning less critical parameter tags:
     - **Min Edge** (`Min Edge {scanEdge} BPS`): **CRITICAL** (trade qualification hurdle). Keep.
     - **Qty** (`Qty {scanQty}`): **CRITICAL** (order/scan size). Keep.
     - **Poll** (`Poll {botCfg.pollIntervalSecs} s`): **LESS CRITICAL**. It is an internal background loop timer. It is already fully viewable and editable inside the `OptionsBotConfigModal`. **Prune from ribbon**.
     - **Max Open** (`Max Open {botCfg.maxOpenPositions}`): **LESS CRITICAL**. It is a risk cap, already editable in `OptionsBotConfigModal`. Current open position count is already displayed in `BotLifecycleControls` and position trackers. **Prune from ribbon**.
     - **Manual executes only**: Compact to `Manual only` to save 50px.
   - Right-side action controls:
     - Error messages are clamped with `truncate max-w-48`.
     - Button layout: `Configurator` (`!py-1.5 !px-2.5`), `Scan` (`!py-1.5 !px-2.5`), `BotLifecycleControls` (`Start`, `Pause`, `Reduce Only`, `Stop`).
   - Space budget analysis:
     - Desktop container width (after 256px rail): ~1024px to 1200px.
     - Left elements: Title (~140px) + Status Badge (~70px) + Min Edge pill (~95px) + Qty pill (~65px) = ~370px.
     - Right elements: Configurator (~100px) + Scan (~70px) + BotLifecycleControls (~200px) = ~370px.
     - Total width: ~740px + gap (~24px) = ~764px.
     - Result: Entire header fits with ample headroom (>250px margin) on a single row without any wrapping.

---

## 3. Caveats

1. **Interaction with Requirement R4 (Standardized Status Tag)**:
   - R4 standardizes bot status tags to four explicit states: `Active`, `Paused`, `Reducing`, `Stopped`.
   - In `OptionsArbitrage.tsx`, `{botSubstrategy && <BotPhaseBadge phase={botPhase} />}` is placed immediately after the strategy title and before any parameter pills, which complies with R4.
   - When R4 updates `BotPhaseBadge` (or introduces `StrategyStatusBadge`), `OptionsArbitrage.tsx` will inherit it without any architectural conflict.
2. **Substrategy Selection State ("all" vs live substrategy)**:
   - When `selected === "all"`, `botSubstrategy` is `null`. Per existing architectural design and unit tests (`OptionsArbitrage.test.ts` line 364), the "All Strategies" view is strictly monitor-only: Configurator and BotLifecycleControls are intentionally hidden.
   - When a specific live substrategy (such as `parity`, `box_spread`, `jelly_roll`, `convexity`) is selected, `botSubstrategy` is populated, revealing Configurator and BotLifecycleControls. Both views maintain single-line header row alignment.
3. **Internal Scrolling of `TickerLogMonitor`**:
   - `TickerLogMonitor` has an internal log table with `max-h-[380px]`. When housed inside the `h-96` (384px) dock container, the header filter bar and table fit smoothly. If the log table grows, the outer container scrolls cleanly.

---

## 4. Conclusion & Concrete Implementation Proposal

### 4.1 Changes for Requirement R3 (Dock Tab Heights)

#### File 1: `frontend/src/components/OpportunityRadar.tsx`
Replace lines 1819–1890:

```diff
-        <div className="p-3.5">
-          {dockTab === "position" ? (
-            <div className="max-h-96 overflow-y-auto">
-              {openPos.length === 0 ? (
-                <EmptyState
-                  plain
-                  title="No Open Radar Positions"
-                  hint="Bot-managed and manual radar entries aggregate here with live deviation and unwind deadline."
-                />
-              ) : (
-                <div className="divide-y divide-surface-border">
-                  {openPos.map((p, i) => {
-                    const remainingSecs = (p.deadlineMs - Date.now()) / 1000;
-                    return (
-                      <div
-                        key={`${p.symbol}-${p.buyVenue}-${p.sellVenue}-${i}`}
-                        className="flex items-center justify-between gap-3 px-2 py-2.5"
-                      >
-                        <div className="min-w-0">
-                          <div className="flex items-center gap-2">
-                            <span className="text-xs font-sans font-bold text-slate-900 truncate">{p.symbol}</span>
-                            {p.unwatched ? (
-                              <span className="px-1.5 py-px rounded-md text-[10px] font-bold border bg-warn-dim text-warn-strong border-warn-border">
-                                unwatched
-                              </span>
-                            ) : p.liveStatus ? (
-                              <span className="px-1.5 py-px rounded-md text-[10px] font-bold border bg-slate-100 text-slate-600 border-slate-200">
-                                {p.liveStatus}
-                              </span>
-                            ) : null}
-                          </div>
-                          <div className="text-[11px] font-sans text-slate-500 truncate">
-                            {p.buyVenue} → {p.sellVenue} · entry {fmtBpsU(p.entryDevBps)} · {fmtTs(p.entryTsMs)}
-                          </div>
-                        </div>
-                        <div className="text-right shrink-0">
-                          <div className="text-xs font-sans font-bold tabular-nums text-slate-900">
-                            {p.liveDevBps != null ? <>{fmtBpsU(p.liveDevBps)} live</> : "—"}
-                          </div>
-                          <div className="text-[11px] font-sans text-slate-500 tabular-nums">
-                            {fmtUsd(p.sizeUsd)} ·{" "}
-                            {remainingSecs > 0 ? `${fmtDur(remainingSecs)} left` : "past deadline"}
-                          </div>
-                        </div>
-                      </div>
-                    );
-                  })}
-                </div>
-              )}
-            </div>
-          ) : dockTab === "history" ? (
-            <div className="max-h-96 overflow-y-auto">
-              <TickerTransactionHistory symbol="" logs={radarLogs} matchAllSymbols />
-            </div>
-          ) : dockTab === "opps" ? (
-            <div className="max-h-96 overflow-y-auto">
-              <TickerOpportunityHistory symbol="" />
-            </div>
-          ) : (
-            <div className="overflow-y-auto">
-              <TickerLogMonitor
-                symbol=""
-                namespace="spread-radar"
-                strategyName="Spread Radar"
-                executionLogs={radarLogs}
-                matchAllSymbols
-                isPaused={isPaused}
-              />
-            </div>
-          )}
-        </div>
+        <div className="p-3.5 h-96 overflow-y-auto">
+          {dockTab === "position" ? (
+            openPos.length === 0 ? (
+              <EmptyState
+                plain
+                title="No Open Radar Positions"
+                hint="Bot-managed and manual radar entries aggregate here with live deviation and unwind deadline."
+              />
+            ) : (
+              <div className="divide-y divide-surface-border">
+                {openPos.map((p, i) => {
+                  const remainingSecs = (p.deadlineMs - Date.now()) / 1000;
+                  return (
+                    <div
+                      key={`${p.symbol}-${p.buyVenue}-${p.sellVenue}-${i}`}
+                      className="flex items-center justify-between gap-3 px-2 py-2.5"
+                    >
+                      <div className="min-w-0">
+                        <div className="flex items-center gap-2">
+                          <span className="text-xs font-sans font-bold text-slate-900 truncate">{p.symbol}</span>
+                          {p.unwatched ? (
+                            <span className="px-1.5 py-px rounded-md text-[10px] font-bold border bg-warn-dim text-warn-strong border-warn-border">
+                              unwatched
+                            </span>
+                          ) : p.liveStatus ? (
+                            <span className="px-1.5 py-px rounded-md text-[10px] font-bold border bg-slate-100 text-slate-600 border-slate-200">
+                              {p.liveStatus}
+                            </span>
+                          ) : null}
+                        </div>
+                        <div className="text-[11px] font-sans text-slate-500 truncate">
+                          {p.buyVenue} → {p.sellVenue} · entry {fmtBpsU(p.entryDevBps)} · {fmtTs(p.entryTsMs)}
+                        </div>
+                      </div>
+                      <div className="text-right shrink-0">
+                        <div className="text-xs font-sans font-bold tabular-nums text-slate-900">
+                          {p.liveDevBps != null ? <>{fmtBpsU(p.liveDevBps)} live</> : "—"}
+                        </div>
+                        <div className="text-[11px] font-sans text-slate-500 tabular-nums">
+                          {fmtUsd(p.sizeUsd)} ·{" "}
+                          {remainingSecs > 0 ? `${fmtDur(remainingSecs)} left` : "past deadline"}
+                        </div>
+                      </div>
+                    </div>
+                  );
+                })}
+              </div>
+            )
+          ) : dockTab === "history" ? (
+            <TickerTransactionHistory symbol="" logs={radarLogs} matchAllSymbols />
+          ) : dockTab === "opps" ? (
+            <TickerOpportunityHistory symbol="" />
+          ) : (
+            <TickerLogMonitor
+              symbol=""
+              namespace="spread-radar"
+              strategyName="Spread Radar"
+              executionLogs={radarLogs}
+              matchAllSymbols
+              isPaused={isPaused}
+            />
+          )}
+        </div>
```

#### File 2: `frontend/src/components/FundingRadar.tsx`
Change line 1702:

```diff
-        <div className="p-3.5 max-h-96 overflow-y-auto">
+        <div className="p-3.5 h-96 overflow-y-auto">
```

#### File 3: `frontend/src/components/LiveMatrix.tsx`
Replace lines 1426–1454:

```diff
-        <div className="p-3.5">
-          {bottomTab === "history" ? (
-            <div className="max-h-96 overflow-y-auto">
-              <TickerTransactionHistory
-                symbol={sym}
-                logs={executionLogs || []}
-                config={currentStrategyConfig}
-                onRefresh={handleHistoryRefresh}
-              />
-            </div>
-          ) : bottomTab === "opps" ? (
-            <div className="max-h-96 overflow-y-auto">
-              <TickerOpportunityHistory symbol={sym} />
-            </div>
-          ) : (
-            <div className="overflow-y-auto">
-              <TickerLogMonitor
-                symbol={sym}
-                strategyName="Simple Spread"
-                config={currentStrategyConfig}
-                executionLogs={executionLogs || []}
-                orderbooks={orderbooks}
-                autoBotEnabled={bot.active}
-                isPaused={isPaused}
-              />
-            </div>
-          )}
-        </div>
+        <div className="p-3.5 h-96 overflow-y-auto">
+          {bottomTab === "history" ? (
+            <TickerTransactionHistory
+              symbol={sym}
+              logs={executionLogs || []}
+              config={currentStrategyConfig}
+              onRefresh={handleHistoryRefresh}
+            />
+          ) : bottomTab === "opps" ? (
+            <TickerOpportunityHistory symbol={sym} />
+          ) : (
+            <TickerLogMonitor
+              symbol={sym}
+              strategyName="Simple Spread"
+              config={currentStrategyConfig}
+              executionLogs={executionLogs || []}
+              orderbooks={orderbooks}
+              autoBotEnabled={bot.active}
+              isPaused={isPaused}
+            />
+          )}
+        </div>
```

---

### 4.2 Changes for Requirement R5 (Options Arbitrage Ribbon & Metrics Reorganization)

#### File: `frontend/src/components/OptionsArbitrage.tsx`
Replace lines 826–975:

```diff
-        <div className="ribbon-container">
-          <div className="flex flex-wrap items-center gap-2 text-xs font-mono">
-            <span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1">
+        <div className="shrink-0 px-4 py-3 flex items-center justify-between gap-3 bg-surface rounded-xl border border-surface-border shadow-2xs select-none">
+          <div className="flex items-center gap-2 text-xs font-mono min-w-0">
+            <span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1 shrink-0">
               {selDef ? selDef.name : "Options Strategies"}
             </span>
             {botSubstrategy && <BotPhaseBadge phase={botPhase} />}
             {liveVisible && (
               <>
-                <div className="ribbon-pill" title="Net-edge hurdle for reported setups">
+                <div className="ribbon-pill shrink-0" title="Net-edge hurdle for reported setups">
                   <span className="text-muted">Min Edge</span>
                   <span className="font-bold text-slate-900 tabular-nums">{scanEdge}</span>
                   <span className="text-muted">BPS</span>
                 </div>
-                <div className="ribbon-pill" title="Scan/entry size in base units">
+                <div className="ribbon-pill shrink-0" title="Scan/entry size in base units">
                   <span className="text-muted">Qty</span>
                   <span className="font-bold text-slate-900 tabular-nums">{scanQty}</span>
                 </div>
-                {botSubstrategy && botCfg && (
-                  <>
-                    <div className="ribbon-pill" title="Bot poll cadence">
-                      <span className="text-muted">Poll</span>
-                      <span className="font-bold text-slate-900 tabular-nums">
-                        {botCfg.pollIntervalSecs}
-                      </span>
-                      <span className="text-muted">s</span>
-                    </div>
-                    <div className="ribbon-pill" title="Max simultaneous managed positions">
-                      <span className="text-muted">Max Open</span>
-                      <span className="font-bold text-slate-900 tabular-nums">
-                        {botCfg.maxOpenPositions}
-                      </span>
-                    </div>
-                  </>
-                )}
               </>
             )}
-            {premiumOutlayUsd != null && Math.abs(premiumOutlayUsd) > 0.005 && (
-              <div
-                className="ribbon-pill"
-                title="Option premiums paid minus received at entry — financing held inside open packages, returned at close/expiry. Kept out of Net Profit."
-              >
-                <span className="text-muted">Premium Outlay</span>
-                <span className="font-bold text-slate-900 tabular-nums">
-                  {fmtUsd(premiumOutlayUsd)}
-                </span>
-              </div>
-            )}
-            {isPaused && <span className="badge-status-warn">Engine Paused</span>}
+            {isPaused && <span className="badge-status-warn shrink-0">Engine Paused</span>}
             {isLive && (
-              <span className="text-xs text-amber-400 border border-amber-500/40 rounded px-1.5 py-0.5 font-sans">
-                Manual executes only
+              <span className="text-xs text-amber-400 border border-amber-500/40 rounded px-1.5 py-0.5 font-sans shrink-0">
+                Manual only
               </span>
             )}
           </div>
-          <div className="flex items-center space-x-3 shrink-0">
+          <div className="flex items-center space-x-2 shrink-0">
             {(botError || flattenError) && (
               <span
-                className="text-xs font-mono text-loss max-w-72 truncate"
+                className="text-xs font-mono text-loss max-w-48 truncate"
                 title={botError ?? flattenError ?? ""}
               >
                 {botError ?? flattenError}
               </span>
             )}
             {botSubstrategy && (
               <button
                 type="button"
                 onClick={() => setShowBotConfig(true)}
-                className="btn-secondary !text-xs !py-2 !px-2 font-sans cursor-pointer gap-2"
+                className="btn-secondary !text-xs !py-1.5 !px-2.5 font-sans cursor-pointer gap-1.5 shrink-0"
                 title={`Configure the ${botSubstrategy.name} bot`}
               >
                 <SlidersHorizontal className="w-3.5 h-3.5 text-accent-strong" />
                 <span>Configurator</span>
               </button>
             )}
             {liveVisible && (
               <button
                 type="button"
                 onClick={() => void scan()}
                 disabled={scanning || isPaused || !optionsVenueAvailable}
                 title={
                   !optionsVenueAvailable
                     ? "No options-capable venue is enabled in this environment"
                     : undefined
                 }
-                className="btn-primary inline-flex items-center gap-1.5 !text-xs !py-2 !px-2.5 font-sans"
+                className="btn-primary inline-flex items-center gap-1.5 !text-xs !py-1.5 !px-2.5 font-sans shrink-0"
               >
                 <RefreshCw className={`w-3.5 h-3.5 ${scanning ? "animate-spin" : ""}`} />
                 {scanning ? "Scanning…" : "Scan"}
               </button>
             )}
             {botSubstrategy && (
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
             )}
           </div>
         </div>

         {/* Per-substrategy summary cards — filtered view of the monitor. */}
-        <div className="grid grid-cols-2 sm:grid-cols-4 gap-4">
+        <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-4">
           <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
             <div className="text-xs font-medium text-muted font-mono">Scanned Setups</div>
             <div className="text-lg font-bold text-slate-900 font-mono tabular-nums mt-0.5">
               {filteredOpps.length}
             </div>
             <div className="text-xs text-slate-500 mt-0.5">
               {scanned ? "latest scan" : "no scan yet"}
             </div>
           </div>
           <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
             <div className="text-xs font-medium text-muted font-mono">Best Net Edge</div>
             <div
               className={`text-lg font-bold font-mono tabular-nums mt-0.5 ${
                 bestEdge > 0 ? "text-profit" : "text-slate-900"
               }`}
             >
               {bestEdge === -Infinity ? "—" : fmtUsd(bestEdge)}
             </div>
             <div className="text-xs text-slate-500 mt-0.5">
               {filteredOpps.length > 0 ? "after fees + funding" : "no setups"}
             </div>
           </div>
           <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
             <div className="text-xs font-medium text-muted font-mono">Executable</div>
             <div className="text-lg font-bold text-slate-900 font-mono tabular-nums mt-0.5">
               {executableCount}
             </div>
             <div className="text-xs text-slate-500 mt-0.5">passes hurdle at scan size</div>
           </div>
           <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
             <div className="text-xs font-medium text-muted font-mono">Realized PnL</div>
             <div
               className={`text-lg font-bold font-mono tabular-nums mt-0.5 ${
                 realizedPnl >= 0 ? "text-profit" : "text-loss"
               }`}
             >
               {filteredLogs.length > 0 ? fmtUsd(realizedPnl) : "—"}
             </div>
             <div className="text-xs text-slate-500 mt-0.5">
               {filteredLogs.length} execution log{filteredLogs.length === 1 ? "" : "s"}
             </div>
           </div>
+          <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
+            <div className="text-xs font-medium text-muted font-mono">Premium Outlay</div>
+            <div
+              className={`text-lg font-bold font-mono tabular-nums mt-0.5 ${
+                premiumOutlayUsd != null && Math.abs(premiumOutlayUsd) > 0.005
+                  ? "text-slate-900"
+                  : "text-muted"
+              }`}
+              title="Option premiums paid minus received at entry — financing held inside open packages, returned at close/expiry. Kept out of Net Profit."
+            >
+              {premiumOutlayUsd != null && Math.abs(premiumOutlayUsd) > 0.005
+                ? fmtUsd(premiumOutlayUsd)
+                : "—"}
+            </div>
+            <div className="text-xs text-slate-500 mt-0.5">net financing in packages</div>
+          </div>
         </div>
```

---

## 5. Verification Method

### 5.1 Test Execution Command
From workspace root:
```bash
cd /mnt/Data/Projects/Arbitrageur/frontend && npm test
```
This executes `node:test` across `src/components/*.test.ts` via `scripts/test-components.mjs`.

### 5.2 Unit Test Specifications

#### Test A: `frontend/src/components/OptionsArbitrage.test.ts`
Add assertions to verify R5 deliverable:
```typescript
test("metrics: 5th card renders Premium Outlay and ribbon omits outlay pill", () => {
  // Test with non-zero premiumOutlayUsd
  const htmlWithOutlay = renderToStaticMarkup(
    React.createElement(OptionsArbitrage, {
      executionLogs: [],
      onRefreshAll: noop,
      optionsVenueAvailable: true,
      premiumOutlayUsd: 1450.5,
    })
  );
  // Verify 5th card renders
  assert.match(htmlWithOutlay, /Premium Outlay<\/div>/);
  assert.match(htmlWithOutlay, /\$1,450\.50/);
  assert.match(htmlWithOutlay, /net financing in packages/);
  // Verify ribbon does NOT contain "Premium Outlay" as a ribbon pill
  assert.doesNotMatch(htmlWithOutlay, /ribbon-pill[^>]*>[\s\S]*?Premium Outlay/);

  // Test with null premiumOutlayUsd -> renders em dash
  const htmlNullOutlay = renderToStaticMarkup(
    React.createElement(OptionsArbitrage, {
      executionLogs: [],
      onRefreshAll: noop,
      optionsVenueAvailable: true,
      premiumOutlayUsd: null,
    })
  );
  assert.match(htmlNullOutlay, /Premium Outlay<\/div>[\s\S]*?>—<\/div>/);
});

test("ribbon: header houses title, status, scan, and controls on a single line", () => {
  const html = renderToStaticMarkup(
    React.createElement(OptionsArbitrage, {
      executionLogs: [],
      onRefreshAll: noop,
      optionsVenueAvailable: true,
    })
  );
  // Header container does NOT use flex-wrap
  const headerContainer = html.match(/<div class="[^"]*justify-between[^"]*">/);
  assert.ok(headerContainer);
  assert.doesNotMatch(headerContainer[0], /flex-wrap/);
});
```

#### Test B: `frontend/src/components/OpportunityRadar.test.ts`
Create new test file `frontend/src/components/OpportunityRadar.test.ts`:
```typescript
import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { OpportunityRadar } from "./OpportunityRadar.tsx";

test("OpportunityRadar: bottom dock container enforces fixed height across tabs", () => {
  const html = renderToStaticMarkup(React.createElement(OpportunityRadar, {}));
  // Assert dock content container has fixed uniform height
  assert.match(html, /class="p-3\.5 h-96 overflow-y-auto"/);
  // Assert all 4 bottom tabs are rendered in the dock header
  assert.match(html, /Current Positions/);
  assert.match(html, /Execution History/);
  assert.match(html, /Opportunity History/);
  assert.match(html, /Log Monitor/);
});
```

#### Test C: `frontend/src/components/FundingRadar.test.ts`
Add assertion in `frontend/src/components/FundingRadar.test.ts`:
```typescript
test("FundingRadar: bottom dock container enforces fixed height", () => {
  const html = renderToStaticMarkup(
    React.createElement(FundingRadar, {
      symbol: "BTC-USDC",
      executionLogs: [],
      onOpenConfig: () => {},
    })
  );
  assert.match(html, /class="p-3\.5 h-96 overflow-y-auto"/);
});
```

### 5.3 Invalidation Conditions
This investigation and proposal would be invalidated if:
1. A global strategy layout overhaul replaces `h-96` with a full-screen vertical flex layout (`flex-1 min-h-0`).
2. Options Arbitrage replaces Deribit with a multi-venue options book requiring per-venue outlay breakdowns.
