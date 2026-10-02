# Changes Log — worker_p3_a

**Date**: 2026-10-01T16:34:00Z  
**Worker**: `worker_p3_a`  
**Scope**: R2 (OpportunityRadar & SpotArbitrageBot header alignment & execution badge removal) and R5 (FundingRadar scanner dimensions & card height)

---

## 1. `frontend/src/components/OpportunityRadar.tsx`

### Changes Made:
1. **Removed `Execution Single-Way` badge from Strategy Status ribbon** (formerly lines 2169–2177):
   - Removed the redundant execution mode pill (`title="Execution mode — edit via Configurator"` displaying `Execution Single-Way` / `Two-Way`).
   - Execution mode remains fully configurable in the Spread Radar Configurator modal (`TickerConfigModal` / `MonitorConfigDto` Order Execution & Slippage section).
2. **Added `Opportunity Scanner` header and right-aligned filter controls in table gadget** (lines 2302–2425):
   - Changed gadget header bar to `flex items-center justify-between gap-3 px-4 py-2.5 border-b border-surface-border text-xs font-sans shrink-0 bg-surface-raised/40`.
   - Added left-aligned header title:
     ```tsx
     <h3 className="text-[13px] font-bold font-sans tracking-wider text-slate-900">
       Opportunity Scanner
     </h3>
     ```
   - Grouped all filter controls on the right inside `<div className="flex items-center gap-2 text-xs font-sans">`:
     - Symbol Search input
     - Venue dropdown selector
     - Min Exp profit hurdle input
     - Min Win empirical probability input
     - Hidden count indicator (`{(snap?.exempted.length ?? 0) > 0 && ...}`)
   - Updated Venue dropdown menu position from `left-0` to `right-0` (`absolute right-0 top-full mt-1.5 w-52 bg-surface...`) to prevent clipping past the right edge when right-aligned.

---

## 2. `frontend/src/components/SpotArbitrageBot.tsx`

### Changes Made:
1. **Aligned opportunity monitor header to match canonical Funding Rate layout** (lines 369–434):
   - Changed container to `p-3.5 border-b border-surface-border flex items-center justify-between gap-3 bg-surface`.
   - Added left-aligned header title:
     ```tsx
     <h3 className="text-[13px] font-bold font-sans tracking-wider text-slate-900">
       Opportunity Scanner
     </h3>
     ```
   - Right-aligned filter controls and count inside `<div className="flex items-center gap-3 text-xs font-sans">`:
     - Search input
     - `Filter Asset:` dropdown selector (preserving label and structure)
     - `{filteredOpportunities.length} opportunities detected` label
   - Updated Asset dropdown menu position from `left-0` to `right-0` (`dropdown-menu right-0 top-full mt-1.5 w-48 z-30`).

---

## 3. `frontend/src/components/FundingRadar.tsx`

### Changes Made:
1. **Reduced Opportunity Scanner container height to 2 rows** (lines 1642–1643):
   - Updated comment to `{/* Opportunity Scanner — fixed window, 5x2 compact cards, click for details */}`.
   - Reduced container height from `h-[640px]` to `h-[600px]`:
     `<div className="bg-surface rounded-xl border border-surface-border p-4 shadow-card h-[600px] flex flex-col">`.
2. **Fixed individual opportunity card height** (line 1769):
   - Added `h-[250px] overflow-hidden` to opportunity card wrapper:
     `className="bg-surface border border-surface-border hover:border-accent-line rounded-xl p-2.5 flex flex-col justify-between gap-2 transition cursor-pointer shadow-card h-[250px] overflow-hidden"`.
   - Ensures consistent card heights across rows and prevents stretching between standard carry and basis/fixed-yield cards.

---

## 4. Test Suite Additions

### `frontend/src/components/OpportunityRadar.test.ts`:
- Added `test("OpportunityRadar: strategy ribbon omits Execution Single-Way badge")`:
  - Asserts `title="Execution mode — edit via Configurator"` and `Execution` / `Single-Way` are absent.
- Added `test("OpportunityRadar: renders Opportunity Scanner header on left and right-aligned filter controls")`:
  - Asserts `<h3>Opportunity Scanner</h3>` exists on left.
  - Asserts outer header bar uses `justify-between`.
  - Asserts right-aligned filter wrapper contains `Symbol`, `Venue`, `Min Exp`, and `Min Win`.
- Added `test("OpportunityRadar: venue dropdown is anchored to right edge (right-0)")`:
  - Asserts `absolute right-0 top-full mt-1.5 w-52 bg-surface`.

### `frontend/src/components/SpotArbitrageBot.test.ts`:
- Added `test("SpotArbitrageBot renders Opportunity Scanner header on left and right-aligned filter controls")`:
  - Asserts `<h3>Opportunity Scanner</h3>` on left.
  - Asserts outer bar uses `justify-between`.
  - Asserts right-aligned filter wrapper contains `Search`, `Filter Asset:`, and `opportunities detected`.
- Added `test("SpotArbitrageBot: asset dropdown is anchored to right edge (right-0)")`:
  - Asserts `dropdown-menu right-0 top-full mt-1.5 w-48 z-30`.

### `frontend/src/components/FundingRadar.test.ts`:
- Added `test("FundingRadar: Opportunity Scanner container is h-[600px] and cards have h-[250px] overflow-hidden")`:
  - Asserts gadget container is `h-[600px] flex flex-col`.
  - Asserts header renders `Opportunity Scanner`.
  - Asserts rendered opportunity cards contain `h-[250px] overflow-hidden`.
- Added `test("FundingRadar: scanner comment documents 5x2 compact cards")`:
  - Asserts comment matches `Opportunity Scanner — fixed window, 5x2 compact cards, click for details`.
