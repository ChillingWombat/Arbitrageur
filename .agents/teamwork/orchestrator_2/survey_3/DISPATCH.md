## 2026-09-29T15:55:38Z
You are survey_explorer_3.
Your working directory: /mnt/Data/Projects/Arbitrageur/.agents/teamwork/orchestrator_2/survey_3/
Parent conversation ID: 79ca425f-52b2-482b-b97b-9d8092a384bd

MANDATORY FIRST STEP:
Read the authoritative user request at `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/ORIGINAL_REQUEST.md` (specifically `## Follow-up — 2026-09-29T15:51:57Z`).
Also consult `/mnt/Data/Projects/Arbitrageur/PROJECT.md` and `/mnt/Data/Projects/Arbitrageur/AGENTS.md`.

YOUR SPECIFIC SCOPE:
Investigate Requirement R3: Fixed Window/Dock Height Across Tabs, AND Requirement R5: Options Arbitrage Ribbon & Metrics Gadget Reorganization.

R3 DETAILS:
"In OpportunityRadar.tsx (and related dockable strategy panels such as FundingRadar.tsx), fix the container height across all bottom dock tabs (Current Positions, Execution History, Opportunity History, Log Monitor) so that switching tabs maintains a consistent window height without layout jumping or resizing."

R5 DETAILS:
"In OptionsArbitrage.tsx, remove the 'Premium Outlay' parameter tag from the strategy ribbon and add a 5th metric card for Premium Outlay alongside the existing 4 metrics gadgets. Reorganize the configurator, scan, and bot lifecycle controls to sit on the same single header row as the strategy title and status. If space is tight, deprioritize or prune less critical parameter tags so the entire header fits on a single line."

INSPECTION TARGETS:
1. R3 Dock Tab Heights:
   - `frontend/src/components/radar/OpportunityRadar.tsx` (and `FundingRadar.tsx`):
   - Inspect the bottom dock tab components: Current Positions, Execution History, Opportunity History, Log Monitor.
   - Why does switching tabs cause height jumps? Check flex layout, min-height, max-height, overflow-y-auto, fixed dock height classes (e.g. `h-[300px]` or `flex-1`).
   - How can uniform fixed height or consistent scroll container be established across all 4 dock tabs?
2. R5 Options Arbitrage Reorganization:
   - `frontend/src/components/options/OptionsArbitrage.tsx`:
   - Inspect the existing ribbon, parameter tags, metric cards (what are the existing 4 cards?), and header row.
   - Where is "Premium Outlay" currently? How is it computed/formatted?
   - How to create the 5th metric card for Premium Outlay alongside the 4 metrics gadgets?
   - How to reorganize the header row so strategy title, status tag, configurator, scan, and bot lifecycle controls all fit on a single row?
   - Which parameter tags can be pruned or compacted to ensure a single-line header?
3. Verification & Tests:
   - Check existing tests in `frontend/test/` and `frontend/scripts/test-components.mjs`.

OUTPUT:
Write your comprehensive analysis and implementation plan to:
`/mnt/Data/Projects/Arbitrageur/.agents/teamwork/orchestrator_2/survey_3/handoff.md`
Report completion via `send_message` with your findings summary.
