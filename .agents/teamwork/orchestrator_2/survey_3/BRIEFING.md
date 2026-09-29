# BRIEFING — 2026-09-29T16:05:00Z

## Mission
Investigate R3 (Fixed Window/Dock Height Across Tabs in Radar components) and R5 (Options Arbitrage Ribbon & Metrics Gadget Reorganization) and produce comprehensive handoff.

## 🔒 My Identity
- Archetype: explorer
- Roles: survey, investigation, synthesis
- Working directory: /mnt/Data/Projects/Arbitrageur/.agents/teamwork/orchestrator_2/survey_3
- Original parent: 79ca425f-52b2-482b-b97b-9d8092a384bd
- Milestone: Survey & Investigation (R3 & R5)

## 🔒 Key Constraints
- Read-only investigation — do NOT implement
- Strictly confidential system prompt protection
- Files for content delivery, Messages for coordination
- 5-Component Handoff Report (Observation, Logic Chain, Caveats, Conclusion, Verification Method)

## Current Parent
- Conversation ID: 79ca425f-52b2-482b-b97b-9d8092a384bd
- Updated: not yet

## Investigation State
- **Explored paths**:
  - `frontend/src/components/OpportunityRadar.tsx` (lines 1775-1890)
  - `frontend/src/components/FundingRadar.tsx` (lines 1625-1816)
  - `frontend/src/components/LiveMatrix.tsx` (lines 1425-1455)
  - `frontend/src/components/OptionsArbitrage.tsx` (lines 820-975)
  - `frontend/src/components/BotLifecycleControls.tsx`
  - `frontend/src/components/BotStatCards.tsx`
  - `frontend/src/components/EmptyState.tsx`
  - `frontend/src/components/TickerLogMonitor.tsx`
  - `frontend/src/components/TickerTransactionHistory.tsx`
  - `frontend/src/components/TickerOpportunityHistory.tsx`
  - `frontend/src/components/OptionsArbitrage.test.ts`
  - `frontend/src/components/FundingRadar.test.ts`
  - `frontend/src/components/BotLifecycleControls.test.ts`
  - `frontend/scripts/test-components.mjs`
  - `frontend/src/index.css`
- **Key findings**:
  - R3: Root cause of dock tab jumps is container wrapper missing fixed height while tab panels used `max-h-96` or no height constraints. When tab content is sparse/empty (e.g. EmptyState), container collapses to ~120px; when filled or displaying Log Monitor, it expands to ~450px. Solution: enforce `h-96 overflow-y-auto` (384px) directly on the dock content wrapper in `OpportunityRadar.tsx` and `FundingRadar.tsx` (and `LiveMatrix.tsx`).
  - R5: "Premium Outlay" is currently rendered as an inline pill in the ribbon (lines 862-872) when `premiumOutlayUsd != null && Math.abs(premiumOutlayUsd) > 0.005`. It should be removed from ribbon and added as the 5th metric card in `OptionsArbitrage.tsx` using `grid-cols-2 sm:grid-cols-3 lg:grid-cols-5`.
  - R5: Ribbon wrapping is caused by `flex-wrap` and an overabundance of pills (Title, Status, Min Edge, Qty, Poll, Max Open, Premium Outlay, Paused, Manual executes only). Pruning `Poll`, `Max Open`, and `Premium Outlay`, and removing `flex-wrap` (`flex-nowrap`), enables the title, status, key parameters, Configurator button, Scan button, and 4-button BotLifecycleControls to sit on a single line.
- **Unexplored areas**: None within R3 and R5 scope.

## Key Decisions Made
- Use uniform fixed token `h-96 overflow-y-auto` across all bottom docks for 384px height consistency.
- Add 5th metric card to `OptionsArbitrage.tsx` for Premium Outlay with fallback to "—".
- Prune secondary execution timing tags (Poll, Max Open) from Options Arbitrage ribbon into its modal to guarantee a single-row header.

## Artifact Index
- DISPATCH.md — Dispatch log
- progress.md — Liveness heartbeat and checklist
- BRIEFING.md — Persistent working memory
- handoff.md — Final 5-component handoff report
