# Empirical Adversarial Challenge Report — Requirements R3, R4, R5

**Author**: challenger_2  
**Verdict**: **APPROVE**  
**Date**: 2026-09-29  

---

## 1. Observation & Empirical Verification

### 1.1 Requirement R3: Locked Dock Container Height
- In `frontend/src/components/OpportunityRadar.tsx` (line 1829), `FundingRadar.tsx` (line 1665), and `LiveMatrix.tsx` (line 1427), the bottom dock container is strictly locked to `className="p-3.5 h-96 overflow-y-auto"`.
- All inner `max-h-96` wrappers were eliminated across all tabs.
- Verified in `ChallengerR3R4R5.test.ts`: tab switching between Current Positions (empty state ~120px intrinsic height), Execution History, Opportunity History, and Log Monitor maintains a locked 384px height without jumping or resizing.

### 1.2 Requirement R4: Standardized Status Badge
- `BotPhaseBadge` in `BotLifecycleControls.tsx` maps all states to exactly one of `Active`, `Paused`, `Reducing`, or `Stopped`.
- Verified in `ChallengerR3R4R5.test.ts`:
  - `running` and `!isPaused` -> `Active` (emerald)
  - `running` and `isPaused` -> `Paused` (amber)
  - `paused` -> `Paused` (amber)
  - `reduce_only` or `stopping` -> `Reducing` (pulsing amber)
  - `idle` or `undefined` -> `Stopped` (slate)
- The word "Engine" is 100% absent from all rendered badge text, classes, and tooltips across all strategy headers.
- Badge is placed immediately to the right of strategy titles before parameter tags.

### 1.3 Requirement R5: Options Arbitrage Header & Metrics Reorganization
- "Premium Outlay" is completely removed from the ribbon parameter pills.
- Added 5th metric card for "Premium Outlay" in `grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-4` displaying formatted USD or "—" when null.
- Strategy ribbon is reorganized into a single non-wrapping header row (`shrink-0 px-4 py-3 flex items-center justify-between gap-3 bg-surface rounded-xl border border-surface-border shadow-2xs select-none`) housing title, status badge, Min Edge, Qty on the left, and Configurator, Scan, and BotLifecycleControls on the right without `flex-wrap`.

---

## 2. Verdict
**APPROVE**. All empirical stress tests in `frontend/src/components/ChallengerR3R4R5.test.ts` pass with 100% success rate.
