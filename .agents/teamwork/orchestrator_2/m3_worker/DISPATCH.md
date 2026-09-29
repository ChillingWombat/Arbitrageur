## 2026-09-29T16:21:14Z
You are m3_worker.
Your working directory: /mnt/Data/Projects/Arbitrageur/.agents/teamwork/orchestrator_2/m3_worker/
Parent conversation ID: 79ca425f-52b2-482b-b97b-9d8092a384bd

MANDATORY FIRST STEP:
Read the authoritative user request at `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/ORIGINAL_REQUEST.md` (specifically `## Follow-up — 2026-09-29T15:51:57Z`).
Also read `/mnt/Data/Projects/Arbitrageur/PROJECT.md`, `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/orchestrator_2/survey_2/handoff.md`, and `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/orchestrator_2/survey_3/handoff.md`.

MANDATORY INTEGRITY WARNING:
DO NOT CHEAT. All implementations must be genuine. DO NOT hardcode test results, create dummy/facade implementations, or circumvent the intended task. A teamwork_preview_auditor will independently verify your work. Integrity violations WILL be detected and your work WILL be rejected.

SCOPE & EXCLUSIVE FILE OWNERSHIP:
You exclusively own and may edit the following files:
- `frontend/src/components/BotLifecycleControls.tsx`
- `frontend/src/components/BotLifecycleControls.test.ts`
- `frontend/src/components/FundingRadar.tsx`
- `frontend/src/components/FundingRadar.test.ts`
- `frontend/src/components/LiveMatrix.tsx`

TASKS:
1. `BotLifecycleControls.tsx` (Requirement R4):
   - Define:
     ```tsx
     export type StrategyBotStatus = "Active" | "Paused" | "Reducing" | "Stopped";

     export function resolveBotStatus(
       phase?: BotPhase,
       isPaused = false,
     ): StrategyBotStatus {
       if (phase === "running") {
         return isPaused ? "Paused" : "Active";
       }
       if (phase === "paused") {
         return "Paused";
       }
       if (phase === "reduce_only" || phase === "stopping") {
         return "Reducing";
       }
       return "Stopped";
     }
     ```
   - Update `BotPhaseBadge` (and alias `export const StrategyStatusBadge = BotPhaseBadge;`):
     - Props: `{ phase?: BotPhase; isPaused?: boolean; status?: StrategyBotStatus }`
     - Status resolved via `status ?? resolveBotStatus(phase, isPaused)`.
     - Displays exactly one of: `"Active"` (emerald), `"Paused"` (amber), `"Reducing"` (pulsing amber), `"Stopped"` (neutral slate).
     - NEVER render "Engine" in text or tooltip.
   - Update `BotLifecycleControls.test.ts` to test all 4 statuses under different phase and isPaused combinations, ensuring "Engine" is never present.

2. `FundingRadar.tsx` (Requirements R3 & R4):
   - At line 1702: Lock bottom dock tabs container to `className="p-3.5 h-96 overflow-y-auto"`.
   - In strategy ribbon (line 1307): Render `<BotPhaseBadge phase={bot.phase} isPaused={isPaused} />` immediately to the right of `Strategy Status`, before parameter tags.
   - Update `FundingRadar.test.ts` to verify the badge and dock height container.

3. `LiveMatrix.tsx` (Requirements R3 & R4):
   - Lock bottom dock tabs container to `className="p-3.5 h-96 overflow-y-auto"`, removing redundant inner `max-h-96` wrappers.
   - In strategy ribbon (line 953): Render `<BotPhaseBadge phase={bot.phase} isPaused={isPaused} />` immediately to the right of `Strategy Status`, before parameter tags.

4. Verification:
   - Run `cd frontend && npm test`
   - Must pass with zero failures.

OUTPUT:
Write your full completion handoff to `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/orchestrator_2/m3_worker/handoff.md`.
Include test run outputs, files modified, and verification commands.
Send a completion message via `send_message`.
