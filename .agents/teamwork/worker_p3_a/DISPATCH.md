## 2026-10-01T16:27:20Z
You are worker_p3_a, an implementation worker for the Arbitrageur project.
Your assigned working directory is:
/mnt/Data/Projects/Arbitrageur/.agents/teamwork/worker_p3_a/

Read the authoritative requirements from:
/mnt/Data/Projects/Arbitrageur/.agents/teamwork/ORIGINAL_REQUEST.md
(Focus on Follow-up — 2026-10-01T16:09:57Z)

Read the technical survey report and handoff from survey_explorer_p3_2:
- /mnt/Data/Projects/Arbitrageur/.agents/teamwork/survey_explorer_p3_2/survey.md
- /mnt/Data/Projects/Arbitrageur/.agents/teamwork/survey_explorer_p3_2/handoff.md

MANDATORY INTEGRITY WARNING:
DO NOT CHEAT. All implementations must be genuine. DO NOT hardcode test results, create dummy/facade implementations, or circumvent the intended task. A teamwork_preview_auditor will independently verify your work. Integrity violations WILL be detected and your work WILL be rejected.

Your exclusive write boundaries:
- `frontend/src/components/OpportunityRadar.tsx`
- `frontend/src/components/SpotArbitrageBot.tsx`
- `frontend/src/components/FundingRadar.tsx`
- `frontend/src/components/OpportunityRadar.test.ts`
- `frontend/src/components/SpotArbitrageBot.test.ts`
- `frontend/src/components/FundingRadar.test.ts`
DO NOT edit any other source files.

Scope to implement:
1. R2. OpportunityRadar.tsx:
   - Remove `Execution Single-Way` badge from strategy status ribbon (lines ~2169–2177).
   - Add `Opportunity Scanner` header on left of table gadget (`<h3 className="text-[13px] font-bold font-sans tracking-wider text-slate-900">Opportunity Scanner</h3>`), and group filter controls (`Symbol`, `Venue`, `Min Exp`, `Min Win`, and hidden count) on the right in `<div className="flex items-center gap-2 text-xs font-sans">`. Set venue menu to `right-0`.
2. R2. SpotArbitrageBot.tsx:
   - Align opportunity monitor header to match Funding Rate layout: `Opportunity Scanner` header on left, filter controls (`Search`, `Filter Asset:`, count) right-aligned in `<div className="flex items-center gap-3 text-xs font-sans">`. Set asset menu to `right-0`.
3. R5. FundingRadar.tsx:
   - Fix individual opportunity card height to `h-[250px] overflow-hidden` (line ~1769).
   - Reduce Opportunity Scanner gadget container height from `h-[640px]` to `h-[600px]` (line ~1643) with comment `5x2 compact cards`.

Verification requirements:
- Run `cd frontend && npm test` and ensure all tests pass.
- Run `npx tsc --noEmit` in `frontend/` and ensure zero errors.
- Add unit test coverage in `OpportunityRadar.test.ts`, `SpotArbitrageBot.test.ts`, and `FundingRadar.test.ts` validating the new header layouts, dimensions, and badge removals.

Document your changes in `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/worker_p3_a/changes.md` and `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/worker_p3_a/handoff.md`.
Send a completion message back to the caller when done.
