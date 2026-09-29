## 2026-09-29T15:55:38Z
You are survey_explorer_2.
Your working directory: /mnt/Data/Projects/Arbitrageur/.agents/teamwork/orchestrator_2/survey_2/
Parent conversation ID: 79ca425f-52b2-482b-b97b-9d8092a384bd

MANDATORY FIRST STEP:
Read the authoritative user request at `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/ORIGINAL_REQUEST.md` (specifically `## Follow-up — 2026-09-29T15:51:57Z`).
Also consult `/mnt/Data/Projects/Arbitrageur/PROJECT.md` and `/mnt/Data/Projects/Arbitrageur/AGENTS.md`.

YOUR SPECIFIC SCOPE:
Investigate Requirement R2: Strategy Filter Relocation & Options Strategy Unification, AND Requirement R4: Standardized Status Tag Across All Strategies.

R2 DETAILS:
"In NetPnlChartGadget.tsx (and PortfolioMonitor.tsx), move the strategy filter dropdown from below the card header up into the top bar of the card directly to the right of the 'P&L & Equity History' title. Unify all options substrategies so that 'Options' appears as a single unified strategy in the dropdown."

R4 DETAILS:
"All strategy headers must display a standardized bot status badge positioned to the right of the strategy title header, before any parameter tags. Remove 'Engine' from the text; the badge must display exactly one of four statuses: Active, Paused, Reducing, or Stopped."

INSPECTION TARGETS:
1. R2 Strategy Filter:
   - `frontend/src/components/dashboard/NetPnlChartGadget.tsx` (and `PortfolioMonitor.tsx` or related): Where is the filter dropdown currently located? What are the layout containers?
   - What strategy options are currently populated? How are options substrategies (e.g. conversions, reversals, box spreads, etc.) represented in state and dropdowns? How should filtering work when "Options" is selected (grouping / mapping all options trades)?
2. R4 Standardized Status Tag:
   - Inspect ALL strategy views and panels:
     `frontend/src/components/radar/OpportunityRadar.tsx`
     `frontend/src/components/funding/FundingRadar.tsx`
     `frontend/src/components/options/OptionsArbitrage.tsx`
     `frontend/src/components/spatial/` or `Triangular` / `StatArb` / `Basis` panels.
   - How are status tags currently rendered? Do they show "Engine Active" or similar?
   - How to ensure the badge displays exactly one of: `Active`, `Paused`, `Reducing`, `Stopped` positioned immediately to the right of the strategy title header, before parameter tags?
   - Is there a shared StatusBadge component in `components/common/` that should be used or created?
3. Verification & Tests:
   - Relevant component tests in `frontend/test/` or `frontend/scripts/test-components.mjs`.

OUTPUT:
Write your comprehensive analysis and implementation plan to:
`/mnt/Data/Projects/Arbitrageur/.agents/teamwork/orchestrator_2/survey_2/handoff.md`
Report completion via `send_message` with your findings summary.
