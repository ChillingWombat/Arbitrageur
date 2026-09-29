## 2026-09-29T16:32:59Z
You are m4_worker.
Your working directory: /mnt/Data/Projects/Arbitrageur/.agents/teamwork/orchestrator_2/m4_worker/
Parent conversation ID: 79ca425f-52b2-482b-b97b-9d8092a384bd

MANDATORY FIRST STEP:
Read the authoritative user request at `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/ORIGINAL_REQUEST.md` (specifically `## Follow-up — 2026-09-29T15:51:57Z`).
Also read `/mnt/Data/Projects/Arbitrageur/PROJECT.md`, `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/orchestrator_2/survey_2/handoff.md`, and `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/orchestrator_2/survey_3/handoff.md`.

MANDATORY INTEGRITY WARNING:
DO NOT CHEAT. All implementations must be genuine. DO NOT hardcode test results, create dummy/facade implementations, or circumvent the intended task. A teamwork_preview_auditor will independently verify your work. Integrity violations WILL be detected and your work WILL be rejected.

SCOPE & EXCLUSIVE FILE OWNERSHIP:
You exclusively own and may edit the following files:
- `frontend/src/components/OpportunityRadar.tsx`
- `frontend/src/components/OpportunityRadar.test.ts`
- `frontend/src/components/OptionsArbitrage.tsx`
- `frontend/src/components/OptionsArbitrage.test.ts`
- `frontend/src/components/TriangularArbitrage.tsx`
- `frontend/src/components/TriangularArbitrage.test.ts`
- `frontend/src/components/PredictionHub.tsx`

TASKS:
1. `OpportunityRadar.tsx`:
   - R3: In the bottom dock container (lines ~1819-1889), lock the outer container to `className="p-3.5 h-96 overflow-y-auto"` and remove the redundant inner `max-h-96` wrappers so that switching tabs maintains a locked 384px height across all 4 tabs (Current Positions, Execution History, Opportunity History, Log Monitor).
   - R4: In the strategy status header ribbon (lines ~1403-1405), pass `isPaused={isPaused}` to `<BotPhaseBadge phase={botPhase} isPaused={isPaused} />` positioned immediately right of the strategy title header (`Strategy Status`), before parameter tags.
   - R1: In `handleFlatten`, also set `handleBotMode("off")` so the bot is stopped (idle/off) upon stopping, never left running or paused. Register with `registerBotResetListener` (or listen to bot phase version/resets) to transition `botCfg.mode` to `"off"` on global halt/reset.

2. `OptionsArbitrage.tsx`:
   - R5: Remove the "Premium Outlay" parameter tag pill from the strategy ribbon.
   - R5: Add a 5th metric card for "Premium Outlay" in the summary metrics grid alongside Scanned Setups, Best Net Edge, Executable, and Realized PnL:
     `<div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-4">`
     Card displays title `"Premium Outlay"`, formatted value `{premiumOutlayUsd != null && Math.abs(premiumOutlayUsd) > 0.005 ? fmtUsd(premiumOutlayUsd) : "—"}` (bold font-mono), subtitle `"net financing in packages"`, and explanatory tooltip.
   - R5: Reorganize the ribbon into a single header row (`shrink-0 px-4 py-3 flex items-center justify-between gap-3 bg-surface rounded-xl border border-surface-border shadow-2xs select-none`). Left side houses title, `<BotPhaseBadge phase={botPhase} isPaused={isPaused} />`, and critical parameter tags (`Min Edge`, `Qty`). Secondary internal loop parameters (`Poll`, `Max Open`) are pruned from the ribbon (available in Configurator modal). Right side houses Configurator button, Scan button, and `BotLifecycleControls` on the same row without wrapping.
   - R4: Ensure `<BotPhaseBadge phase={botPhase} isPaused={isPaused} />` displays immediately right of `{selDef ? selDef.name : "Options Strategies"}`. Remove the trailing `{isPaused && <span className="badge-status-warn">Engine Paused</span>}` tag.
   - R1: In `handleFlatten`, also set `handleBotMode("off")`. Register with `registerBotResetListener` to reset to `"off"` on global reset.

3. `TriangularArbitrage.tsx`:
   - R4: Pass `isPaused={isPaused}` to `<BotPhaseBadge phase={bot.phase} isPaused={isPaused} />` immediately right of `Strategy Status`, before parameter tags. Remove trailing `{isPaused && <span className="badge-status-warn">Engine Paused</span>}`.

4. `PredictionHub.tsx`:
   - R4: Pass `isPaused={isPaused}` to `<BotPhaseBadge phase={bot.phase} isPaused={isPaused} />` immediately right of `Strategy Status`, before parameter tags.

5. Tests & Verification:
   - Add/update tests in `OpportunityRadar.test.ts`, `OptionsArbitrage.test.ts`, and `TriangularArbitrage.test.ts`.
   - Run verification command:
     `cd frontend && npm test`
     Must pass with zero failures.

OUTPUT:
Write your full completion handoff to `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/orchestrator_2/m4_worker/handoff.md`.
Include test run outputs, files modified, and verification commands.
Send a completion message via `send_message`.
