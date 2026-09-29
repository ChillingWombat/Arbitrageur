# BRIEFING — 2026-09-29T16:04:20Z

## Mission
Investigate R2 (Strategy Filter Relocation & Options Strategy Unification) and R4 (Standardized Status Tag Across All Strategies) to produce a comprehensive analysis and implementation plan.

## 🔒 My Identity
- Archetype: explorer
- Roles: investigator, synthesizer
- Working directory: /mnt/Data/Projects/Arbitrageur/.agents/teamwork/orchestrator_2/survey_2/
- Original parent: 79ca425f-52b2-482b-b97b-9d8092a384bd
- Milestone: survey_phase_2

## 🔒 Key Constraints
- Read-only investigation — do NOT implement
- Living mode first, fail-closed
- Only write metadata inside `.agents/teamwork/orchestrator_2/survey_2/`
- Standardized status tag: exactly one of Active, Paused, Reducing, Stopped (no 'Engine')
- NetPnlChartGadget: move filter dropdown to card header right of title, unify options substrategies under 'Options'

## Current Parent
- Conversation ID: 79ca425f-52b2-482b-b97b-9d8092a384bd
- Updated: not yet

## Investigation State
- **Explored paths**:
  - `frontend/src/components/NetPnlChartGadget.tsx` & `.test.ts`
  - `frontend/src/components/PortfolioMonitor.tsx`
  - `frontend/src/components/TradedOpportunitiesGadget.tsx` & `.test.ts`
  - `frontend/src/types/tickers.ts`
  - `frontend/src/hooks/useBotLifecycle.ts` & `.test.ts`
  - `frontend/src/components/BotLifecycleControls.tsx` & `.test.ts`
  - `frontend/src/components/OpportunityRadar.tsx`
  - `frontend/src/components/FundingRadar.tsx`
  - `frontend/src/components/LiveMatrix.tsx`
  - `frontend/src/components/TriangularArbitrage.tsx`
  - `frontend/src/components/SpotArbitrageBot.tsx`
  - `frontend/src/components/PredictionHub.tsx`
  - `frontend/src/components/OptionsArbitrage.tsx`
  - `frontend/src/components/StatArbLab.tsx`
  - `frontend/src/index.css` & `tailwind.config.js`
- **Key findings**:
  - R2: `NetPnlChartGadget.tsx` has the strategy `<select>` in a second row below the card header. Moving it into the top bar next to `P&L & Equity History` title removes the redundant row.
  - R2: `KNOWN_PNL_CATEGORIES` contains `...Object.values(STRATEGY_DISPLAY_NAMES)`, bringing in `"Options Parity"`, `"Box Spread"`, `"Jelly Roll"`, `"Convexity"`. Because `strategyCategory` in `TradedOpportunitiesGadget.tsx` maps all `options_*` to `"Options"`, filtering for substrategies results in 0 rows. Unifying options removes these substrategies and retains single `"Options"`.
  - R4: `BotPhaseBadge` in `BotLifecycleControls.tsx` currently only renders for `reduce_only` ("Reduce-Only") and `stopping` ("Stopping"), returning `null` for `running`, `paused`, and `idle`.
  - R4: `TriangularArbitrage.tsx` and `OptionsArbitrage.tsx` currently render `{isPaused && <span className="badge-status-warn">Engine Paused</span>}` at the trailing end of their ribbons.
  - R4: Standardized status badge must be placed immediately to the right of the strategy title header, before parameter tags, displaying exactly one of: `Active`, `Paused`, `Reducing`, `Stopped`. "Engine" must be removed.
- **Unexplored areas**:
  - None within R2 and R4 scope.

## Key Decisions Made
- Standardize `BotPhaseBadge` (or export alias `StrategyStatusBadge`) with 4 distinct statuses: `Active`, `Paused`, `Reducing`, `Stopped` using semantic design tokens.
- Move `<select>` in `NetPnlChartGadget.tsx` to the top header flex container immediately right of the `<h2>` title.
- Sanitize `KNOWN_PNL_CATEGORIES` so that `"Options"` is the sole options entry.

## Artifact Index
- DISPATCH.md — Initial dispatch message
- BRIEFING.md — Persistent working memory
- progress.md — Liveness heartbeat
- handoff.md — Comprehensive investigation and implementation plan
