# Project: Arbitrageur Desktop Application UI/UX Standardization & Polish (R1–R8)

## Architecture
Standardize and polish strategy monitor components, ribbon tags, button set borders, and security widgets across the Arbitrageur desktop application.

## Feature Inventory
| # | Feature | Description | Milestone | Source |
|---|---------|-------------|-----------|--------|
| 1 | R1.1 PredictionHub Warning Cleanup | Remove scaffold warning text (`Prediction venues are scaffolds: no market data or order execution path`) from ribbon | Block_B | survey_explorer_p3_1 |
| 2 | R1.2 PredictionHub Max Slip Removal | Remove disabled `Max Slip` ribbon pill | Block_B | survey_explorer_p3_1 |
| 3 | R1.3 PredictionHub Search Placeholder | Update search placeholder to `Search contract...` | Block_B | survey_explorer_p3_1 |
| 4 | R1.4 PredictionHub Min Vol Badge | Convert `Min Vol` to active parameter badge showing value in USD (`10,000` / `$500,000`), removing disabled text | Block_B | survey_explorer_p3_1 |
| 5 | R2.1 OpportunityRadar Single-Way Removal | Remove `Execution Single-Way` badge from strategy status ribbon | Block_A | survey_explorer_p3_2 |
| 6 | R2.2 OpportunityRadar Header Alignment | Add `Opportunity Scanner` header on left and align filter controls (`Symbol`, `Venue`, `Min Exp`, `Min Win`, hidden count) to right end of bar | Block_A | survey_explorer_p3_2 |
| 7 | R2.3 SpotArbitrageBot Header Alignment | Align Spatial opportunity monitor header with `Opportunity Scanner` title on left and filter controls right-aligned | Block_A | survey_explorer_p3_2 |
| 8 | R3.1 BotLifecycleControls Symmetric Padding | Update outer border wrapper padding from `px-2 py-1` to `p-1` for equal symmetric padding across all strategy lifecycle groups | Block_C | survey_explorer_p3_3 |
| 9 | R4.1 LiveMatrix Delta Trend Arrow Removal | Remove `TrendingUp` / `TrendingDown` trend arrow icons (`↗` / `↘`) from cell delta badges and prune unused imports | Block_C | survey_explorer_p3_3 |
| 10 | R4.2 SignalTrendGadget Footer Line Removal | Remove footer text line displaying venue samples and baseline description, prune unused `historyLabel` | Block_C | survey_explorer_p3_3 |
| 11 | R5.1 FundingRadar Scanner Gadget Height | Reduce Opportunity Scanner gadget height from `h-[640px]` to `h-[600px]` to cleanly fit 2 rows of cards with vertical scroll for 3+ | Block_A | survey_explorer_p3_2 |
| 12 | R5.2 FundingRadar Card Height Consistency | Lock individual opportunity card height to `h-[250px] overflow-hidden` for consistent sizing across all cards | Block_A | survey_explorer_p3_2 |
| 13 | R6.1 Options Section Header Ribbon | Hide strategy status ribbon bar (`Options Strategies ... Scan`) when "All Strategies" is viewed, rendering only for specific substrategies | Block_B | survey_explorer_p3_1 |
| 14 | R7.1 EngineSettings Security Model Styling | Refine Security Model explanatory box to use `bg-slate-50` and neutral slate icon (`text-slate-500`) | Block_C | survey_explorer_p3_3 |
| 15 | R7.2 EngineSettings MFA Paragraph Removal | Remove MFA unlock explanation paragraph (`MFA gates the unlock path in this app...`) | Block_C | survey_explorer_p3_3 |
| 16 | R7.3 EngineSettings TOTP Label Wording | Remove authenticator app examples `(Google Authenticator, 1Password, Authy)` from TOTP description label | Block_C | survey_explorer_p3_3 |
| 17 | R8.1 TickerExplorer Quote Display | Hide `-USD` quote label when quote is universal USD (e.g. `APT`), while retaining non-USD quotes (`-USDC`, `-USDT`) | Block_B | survey_explorer_p3_1 |

## Milestones
| # | Name | Scope | Dependencies | Status |
|---|------|-------|-------------|--------|
| Block_A | Strategy Scanners & Dimensions | R2 (`OpportunityRadar.tsx`, `SpotArbitrageBot.tsx`), R5 (`FundingRadar.tsx`) | Phase 0 Survey | DONE |
| Block_B | Strategy Ribbons & Ticker Display | R1 (`PredictionHub.tsx`), R6 (`OptionsArbitrage.tsx`), R8 (`TickerExplorer.tsx`) | Phase 0 Survey | DONE |
| Block_C | Controls, Cleanups & Security Settings | R3 (`BotLifecycleControls.tsx`), R4 (`LiveMatrix.tsx`, `SignalTrendGadget.tsx`), R7 (`EngineSettings.tsx`) | Phase 0 Survey | DONE |
| Verification | Review, Challenge & Forensic Audit | Full verification across all modified files, `cd frontend && npm test`, `npx tsc --noEmit` | Block_A, Block_B, Block_C | DONE |

## Code Layout & Exclusive File Boundaries
### Block A (Worker A)
- Exclusively owns:
  - `frontend/src/components/OpportunityRadar.tsx`
  - `frontend/src/components/SpotArbitrageBot.tsx`
  - `frontend/src/components/FundingRadar.tsx`
  - `frontend/src/components/OpportunityRadar.test.ts`
  - `frontend/src/components/SpotArbitrageBot.test.ts`
  - `frontend/src/components/FundingRadar.test.ts`

### Block B (Worker B)
- Exclusively owns:
  - `frontend/src/components/PredictionHub.tsx`
  - `frontend/src/components/OptionsArbitrage.tsx`
  - `frontend/src/components/TickerExplorer.tsx`
  - `frontend/src/components/PredictionHub.test.ts`
  - `frontend/src/components/OptionsArbitrage.test.ts`
  - `frontend/src/components/TickerExplorer.test.ts`
  - `frontend/src/components/ChallengerR3R4R5.test.ts`

### Block C (Worker C)
- Exclusively owns:
  - `frontend/src/components/BotLifecycleControls.tsx`
  - `frontend/src/components/LiveMatrix.tsx`
  - `frontend/src/components/SignalTrendGadget.tsx`
  - `frontend/src/components/EngineSettings.tsx`
  - `frontend/src/components/BotLifecycleControls.test.ts`
  - `frontend/src/components/SignalTrendGadget.test.ts`
  - `frontend/src/components/EngineSettings.test.ts`
