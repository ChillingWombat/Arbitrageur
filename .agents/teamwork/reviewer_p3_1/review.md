# Review Report: Phase 3 Follow-up (R1, R2, R5, R6, R8)

**Reviewer**: `reviewer_p3_1` (Reviewer & Adversarial Critic)  
**Date**: 2026-10-01T16:50:00Z  
**Verdict**: **APPROVE**  
**Assigned Scope**: 
- **R1**: Prediction Market Ribbon & Input Cleanups (`frontend/src/components/PredictionHub.tsx`)
- **R2**: Strategy Monitor Header Alignment (`frontend/src/components/OpportunityRadar.tsx`, `frontend/src/components/SpotArbitrageBot.tsx`)
- **R5**: Funding Rate Opportunity Scanner Dimensions (`frontend/src/components/FundingRadar.tsx`)
- **R6**: Options Section Header Ribbon (`frontend/src/components/OptionsArbitrage.tsx`)
- **R8**: Ticker Explorer Symbol Quote Display (`frontend/src/components/TickerExplorer.tsx`)

---

## 1. Review Summary

All requirements assigned to `worker_p3_a` (R2, R5) and `worker_p3_b` (R1, R6, R8) have been implemented genuinely, correctly, and with thorough unit and adversarial test coverage.
- Zero integrity violations were detected.
- Real component markup, props, and lifecycle mechanisms are employed without facade shims or hardcoded test overrides.
- TypeScript compilation is completely clean (`npx tsc --noEmit` exits code 0).
- All 8 unit and challenger test suites in this scope pass 100% (84 individual tests passing).

---

## 2. Requirement-by-Requirement Verification

### R1. Prediction Market Ribbon & Input Cleanups (`PredictionHub.tsx`)
**Requirements**:
1. Remove execution unavailable warning text (`Prediction venues are scaffolds: no market data or order execution path`).
2. Remove `Max Slip` ribbon tag.
3. Clean filter search input placeholder from `Search contract (e.g. FED, BTC)...` to `Search contract...`.
4. Make `Min Vol` parameter value visible as an active parameter badge (`10,000` or configured USD) replacing disabled text `no venue volume data`.

**Observations & Code Inspection**:
- Lines 275–282 formerly rendering `{!executeReady && <span title={capability?.reason}>{capability?.reason}</span>}` were completely deleted.
- The `Max Slip` ribbon pill (`no execution path`) was deleted.
- The search input placeholder is now exactly `"Search contract..."`.
- An active parameter badge was implemented:
  ```tsx
  <div className="ribbon-pill" title="Minimum event volume threshold for prediction markets">
    <span className="text-muted">Min Vol</span>
    <span className="font-bold text-slate-900 tabular-nums">{displayMinVol.toLocaleString()}</span>
    <span className="text-muted">USD</span>
  </div>
  ```
- `displayMinVol` resolves via `minEventVolumeUsd ?? loadedMinVol ?? 10_000`, with asynchronous loading from `api.getStrategyParams("prediction")` (`p.autoMinEventVolumeUsd`) when the prop is omitted.
- **Safety Assessment**: Order execution gating (`executeReady === true`) remains strictly enforced on the actual execution handlers (`handleExecute`), automated parity loops, and individual card action buttons (`ExecuteButton`). Removing the static banner text did not weaken execution safety in any way.

### R2. Strategy Monitor Header Alignment (`OpportunityRadar.tsx` & `SpotArbitrageBot.tsx`)
**Requirements**:
1. In `OpportunityRadar.tsx`, remove the `Execution Single-Way` badge from the strategy status ribbon.
2. In `OpportunityRadar.tsx`, add an `Opportunity Scanner` header to the table gadget and align filter controls (`Symbol`, `Venue`, `Min Exp`, `Min Win`, hidden count) to the right end of the bar, matching Funding Rate layout.
3. In `SpotArbitrageBot.tsx`, align the opportunity monitor header to match the Funding Rate layout: `Opportunity Scanner` on the left and filter controls right-aligned.

**Observations & Code Inspection**:
- In `OpportunityRadar.tsx`, lines 2169–2177 rendering the execution mode badge were cleanly removed from the ribbon. Execution mode remains fully configurable via `TickerConfigModal`.
- In `OpportunityRadar.tsx`, the table gadget header was restructured to `flex items-center justify-between gap-3 px-4 py-2.5 ...`, rendering:
  - Left: `<h3 className="text-[13px] font-bold font-sans tracking-wider text-slate-900">Opportunity Scanner</h3>`
  - Right: `<div className="flex items-center gap-2 text-xs font-sans">` grouping `Symbol`, `Venue`, `Min Exp`, `Min Win`, and the hidden counter.
- In `SpotArbitrageBot.tsx`, the header was restructured to `p-3.5 border-b border-surface-border flex items-center justify-between gap-3 bg-surface`, rendering:
  - Left: `<h3 className="text-[13px] font-bold font-sans tracking-wider text-slate-900">Opportunity Scanner</h3>`
  - Right: `<div className="flex items-center gap-3 text-xs font-sans">` with `Search`, `Filter Asset:` dropdown, and `{filteredOpportunities.length} opportunities detected`.
- **Ergonomics & Overflow Defense**: Both components updated their dropdown popup positions from `left-0` to `right-0` (`absolute right-0 top-full ...`). Because the trigger buttons are positioned near the right viewport boundary, `right-0` anchors the menu to the right edge and expands inward to the left, preventing off-screen overflow.

### R5. Funding Rate Opportunity Scanner Dimensions (`FundingRadar.tsx`)
**Requirements**:
1. Reduce height of Opportunity Scanner gadget so it accommodates exactly 2 rows of opportunity cards, with vertical scrolling for additional cards.
2. Fix height of individual opportunity cards to maintain consistent card sizing.

**Observations & Code Inspection**:
- Container height in `FundingRadar.tsx` line 1643 was reduced from `h-[640px]` to `h-[600px] flex flex-col`.
- Individual opportunity cards on line 1769 were given fixed height: `h-[250px] overflow-hidden`.
- The child container uses `<div className="flex-1 min-h-0 overflow-y-auto">` enclosing `<div className="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-5 gap-3">`.
- **Dimensional Verification**:
  - 2 rows of 250px cards = 500px.
  - Grid row gap (`gap-3` = 0.75rem = 12px) = 12px.
  - Gadget header, padding (`p-4` = 32px), and borders = ~88px.
  - Total vertical height for 2 rows = ~600px.
  - Exactly 2 rows (up to 10 cards on xl displays) fit without scrolling. For > 10 cards, `overflow-y-auto` smoothly engages.
  - `overflow-hidden` on cards prevents Basis cards (with extra badges) or differing metric lines from stretching individual grid cells.

### R6. Options Section Header Ribbon (`OptionsArbitrage.tsx`)
**Requirements**:
1. Hide strategy status ribbon bar (`Options Strategies ... Scan`) when "All Strategies" is selected, rendering it only when a specific substrategy is selected.

**Observations & Code Inspection**:
- The strategy status ribbon container in `OptionsArbitrage.tsx` lines 917–993 was wrapped in `{selected !== "all" && ( ... )}`.
- Added `initialSelected?: string` (defaulting to `"all"`) in `OptionsArbitrageProps`, and initialized state via `useState<string>(initialSelected)`.
- When "All Strategies" is viewed (default), the ribbon bar is completely unrendered.
- Clicking any specific substrategy on the rail sets `selected` to that substrategy id (`parity`, `box`, `calendar`, etc.), rendering that substrategy's bot status, controls, and Scan button.
- Re-selecting "All Strategies" restores `selected === "all"` and omits the ribbon.

### R8. Ticker Explorer Symbol Quote Display (`TickerExplorer.tsx`)
**Requirements**:
1. Hide `-USD` quote label when the quote is universal USD, displaying only the base symbol (e.g. `APT`). Keep non-USD quote suffixes visible (e.g. `APT-USDC`, `APT-USDT`).

**Observations & Code Inspection**:
- In `TickerRow` (lines 157–159):
  ```tsx
  {quote && quote.toUpperCase() !== "USD" && (
    <span className="text-muted font-semibold">-{quote}</span>
  )}
  ```
- When `quote` is `"USD"` or `"usd"`, the condition evaluates to false; only `{base}` is rendered.
- When `quote` is `"USDC"` or `"USDT"`, the suffix `-{quote}` is rendered with `text-muted font-semibold`.
- When `quote` is empty or undefined, the guard prevents rendering `-`.
- Full symbol keys (`t.symbol`) remain preserved for selection, context menus, and telemetry lookup.

---

## 3. Adversarial Analysis & Stress-Testing

| Vector | Scenario Tested | Outcome | Verdict |
|---|---|---|---|
| **R1 Gating** | User views PredictionHub with `executeReady: false` after banner removal | Manual execute button and auto execution loops remain strictly disabled; no order can dispatch | **PASS** |
| **R1 Volume Fallback** | `minEventVolumeUsd` prop passed as `0`, `null`, `undefined`, or unconfigured | `0` renders `"0"`, `500000` renders `"500,000"`, null/error falls back gracefully to `"10,000"` | **PASS** |
| **R2 Menu Overflow** | User clicks Venue menu in `OpportunityRadar` or Asset menu in `SpotArbitrageBot` on viewport right edge | `right-0` anchors menu to right side and opens inward; zero horizontal viewport overflow | **PASS** |
| **R5 Card Geometry** | Single row (3 cards), exactly 2 rows (10 cards), and overflow rows (15 cards) | 0–10 cards render cleanly within 600px; 11+ cards trigger smooth vertical scrolling without expanding parent | **PASS** |
| **R6 Substrategy State** | User toggles from "all" to "parity" to "box" and back to "all" | Ribbon dynamically appears with relevant substrategy name & controls, then completely disappears on "all" | **PASS** |
| **R8 Quote Normalization** | Mixed case quotes (`"usd"`, `"USD"`, `"Usd"`, `"USDC"`, `"usdc"`) | Universal USD is cleanly hidden across all casings; non-USD quotes (`-USDC`, `-USDT`, `-EUR`) remain visible | **PASS** |

---

## 4. Integrity Violation Check

- **Hardcoded test results or fake implementations**: None.
- **Dummy or facade components**: None.
- **Shortcuts bypassing required work**: None.
- **Fabricated verification logs**: None. Independent test runs confirmed.

---

## 5. Test Suite Verification Results

### TypeScript Typecheck
```bash
cd frontend && npx tsc --noEmit
# Exit code: 0 (Zero errors)
```

### In-Scope Component & Challenger Test Suites
Executed via `node scripts/test-components.mjs`:
1. `PredictionHub.test.ts`: **6/6 pass**
2. `OpportunityRadar.test.ts`: **8/8 pass**
3. `SpotArbitrageBot.test.ts`: **7/7 pass**
4. `FundingRadar.test.ts`: **13/13 pass**
5. `OptionsArbitrage.test.ts`: **16/16 pass**
6. `TickerExplorer.test.ts`: **5/5 pass**
7. `ChallengerR1R2R5R6R8.test.ts`: **16/16 pass**
8. `ChallengerR3R4R5.test.ts`: **13/13 pass**

**Total In-Scope Tests**: **84/84 PASS (100%)**

### Non-Scope Note
A single failure was observed in `ChallengerR3R4R7.test.ts` (`Pause mismatch for phase=reduce_only`), which tests `BotLifecycleControls.tsx` for requirements R3, R4, and R7 (assigned to `worker_p3_c` and `reviewer_p3_2`). This does not affect R1, R2, R5, R6, or R8.

---

## 6. Conclusion

The deliverables for R1, R2, R5, R6, and R8 fully meet the specification, adhere to project conventions, maintain fail-closed execution safety, and provide robust test coverage.

**Verdict**: **APPROVE**
