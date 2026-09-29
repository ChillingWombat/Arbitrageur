# Dispatch Assignments

## 2026-09-29T15:53:46Z

You are orchestrator_2, the Project Orchestrator for Arbitrageur Desktop.
Working directory: /mnt/Data/Projects/Arbitrageur/.agents/teamwork/orchestrator_2/
Project root: /mnt/Data/Projects/Arbitrageur

Your authoritative instructions and requirements are documented in:
`/mnt/Data/Projects/Arbitrageur/.agents/teamwork/ORIGINAL_REQUEST.md`
under `## Follow-up — 2026-09-29T15:51:57Z`.

Overview of Requirements:
1. R1: Bot Status on Halt / Emergency Reset & Individual Startup
   - After halt, killswitch, emergency reset, or circuit breaker reset, all automated bot statuses across all strategies (Radar Bot, Options Bot substrategies, and frontend useBotLifecycle hooks) must be stopped (idle / off), never paused.
   - The Header pause/resume button must not resume any automated bots and must be disabled while the circuit breaker is tripped.
   - Each strategy's bot must be started individually using its own strategy-level Start button.
2. R2: Strategy Filter Relocation & Options Strategy Unification
   - In NetPnlChartGadget.tsx (and PortfolioMonitor.tsx), move the strategy filter dropdown from below the card header up into the top bar of the card directly to the right of the "P&L & Equity History" title.
   - Unify all options substrategies so that "Options" appears as a single unified strategy in the dropdown.
3. R3: Fixed Window/Dock Height Across Tabs
   - In OpportunityRadar.tsx (and related dockable strategy panels such as FundingRadar.tsx), fix the container height across all bottom dock tabs (Current Positions, Execution History, Opportunity History, Log Monitor) so that switching tabs maintains a consistent window height without layout jumping or resizing.
4. R4: Standardized Status Tag Across All Strategies
   - All strategy headers must display a standardized bot status badge positioned to the right of the strategy title header, before any parameter tags.
   - Remove "Engine" from the text; the badge must display exactly one of four statuses: Active, Paused, Reducing, or Stopped.
5. R5: Options Arbitrage Ribbon & Metrics Gadget Reorganization
   - In OptionsArbitrage.tsx, remove the "Premium Outlay" parameter tag from the strategy ribbon and add a 5th metric card for Premium Outlay alongside the existing 4 metrics gadgets.
   - Reorganize the configurator, scan, and bot lifecycle controls to sit on the same single header row as the strategy title and status.
   - If space is tight, deprioritize or prune less critical parameter tags so the entire header fits on a single line.

Verification:
- Frontend test suite: `cd frontend && npm test` (node --test + scripts/test-components.mjs)
- Desktop backend test suite: `cargo test -p arbitrageur-desktop`
- Both must pass with zero failures.

Maintain your `BRIEFING.md` and `progress.md` in `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/orchestrator_2/`.
Decompose into milestones, spawn specialized subagents (explorers, workers, reviewers, challengers), execute with rigorous verification, and notify parent when complete.

## 2026-09-29T16:29:33Z

The server just restarted. Please revive your active workers / subagents or dispatch fresh agents as needed and continue executing your remaining milestones (M1, M3, M4, M5).

Note the coordination scope from parent agent:
1) The parent agent is directly handling:
   - Adding dock tab icons/symbols to FundingRadar.tsx (Wallet, History, Radar, Terminal) matching OpportunityRadar.
   - Updating the bottom half gadgets and layout of SpotArbitrageBot.tsx (Spatial strategy) to match FundingRadar/OpportunityRadar (full-width scanner + 4-tab docked container for Current Positions, Execution History, Opportunity History, Log Monitor).
2) Your team is responsible for:
   - R1: Bot status on halt/emergency reset set to stopped (never paused), Header button disabled while circuit breaker is tripped and not auto-resuming bots, individual strategy-level start buttons.
   - R4: Standardized status badges (Active | Paused | Reducing | Stopped) across all remaining strategies.
   - R5: Options Arbitrage ribbon and metrics reorganization (Premium Outlay 5th card in grid, controls on same single header row as strategy title and status).
   - Milestone M5: Full test suite verification (cd frontend && npm test and cargo test -p arbitrageur-desktop must pass with zero failures).

## 2026-09-29T16:48:20Z

The server just restarted. Please resume your orchestration for the remaining tasks and milestones.

Status summary:
- Completed: Survey, M2, M3.
- In Progress: M1, M4 (Options Arbitrage ribbon & 5th metric card for Premium Outlay, single header row for controls, status badges across strategies).
- Backend note: Parent agent resolved the StrategyParamsDto default target_profit_bps backend test issue and completed FundingRadar dock icons & SpotArbitrageBot bottom half layout.

Please re-establish your heartbeat cron and revive active workers for M1, M4, and proceed through M5 for final regression verification (cd frontend && npm test and cargo test -p arbitrageur-desktop). Report back when victory is ready for audit.

## 2026-09-29T17:06:33Z

The server just restarted. Please resume orchestrator_2 immediately.

Exact M5 Gate Status from disk (.agents/teamwork/orchestrator_2/):
- Milestones M1, M2, M3, M4: DONE.
- M5 Gate Status:
  - challenger_1/handoff.md: DONE (APPROVED)
  - reviewer_1/handoff.md: DONE
  - reviewer_2/handoff.md: DONE
  - challenger_2: progress.md exists, handoff.md was in-flight at restart (ChallengerR3R4R5.test.ts already created and passing 100%)
  - auditor_1: progress.md exists, handoff.md was in-flight at restart

Action: Revive challenger_2 and auditor_1 (or write the verified approvals into handoff.md if completed), synthesize GATE_STATUS.md for Milestone M5, verify dual-track test results (npm test and cargo test), and submit your final completion handoff so we can trigger the independent Victory Audit.
