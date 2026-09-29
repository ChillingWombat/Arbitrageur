## 2026-09-29T15:55:38Z
You are survey_explorer_1.
Your working directory: /mnt/Data/Projects/Arbitrageur/.agents/teamwork/orchestrator_2/survey_1/
Parent conversation ID: 79ca425f-52b2-482b-b97b-9d8092a384bd

MANDATORY FIRST STEP:
Read the authoritative user request at `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/ORIGINAL_REQUEST.md` (specifically `## Follow-up — 2026-09-29T15:51:57Z`).
Also consult `/mnt/Data/Projects/Arbitrageur/PROJECT.md` and `/mnt/Data/Projects/Arbitrageur/AGENTS.md`.

YOUR SPECIFIC SCOPE:
Investigate Requirement R1: Bot Status on Halt / Emergency Reset & Individual Startup.
"After halt, killswitch, emergency reset, or circuit breaker reset, all automated bot statuses across all strategies (Radar Bot, Options Bot substrategies, and frontend useBotLifecycle hooks) must be stopped (idle / off), never paused. The Header pause/resume button must not resume any automated bots and must be disabled while the circuit breaker is tripped; each strategy's bot must be started individually using its own strategy-level Start button."

INSPECTION TARGETS:
1. Backend:
   - `frontend/src-tauri/src/execution.rs`: Check `stop_all`, `killswitch`, emergency reset logic. What does it currently set bot statuses / coordinator flags to?
   - `frontend/src-tauri/src/circuit_breaker.rs`: Check circuit breaker trip and reset commands. How do they interact with bot states and `is_paused`?
   - `crates/arbitrageur-desktop/src/bots/radar/monitor.rs` and other bot controllers: What state/config controls whether bots run? Are bot statuses persisted in SQLite (e.g. `trading_bot_configs`)?
   - Dev RPC handlers in `frontend/src-tauri/src/dev_rpc.rs`: How are `stop_all`, `killswitch`, `reset_circuit_breaker`, and strategy param commands handled?
2. Frontend:
   - `frontend/src/hooks/useBotLifecycle.ts`: How is bot phase tracked (idle, starting, running, paused, stopping, failed)? What happens on halt / circuit breaker reset?
   - `frontend/src/components/Header.tsx`: How does the Header pause/resume button work? What does resume do? How does it check circuit breaker tripped status? What tooltip/disabled state currently exists or is needed?
   - Strategy-level start/stop controls across strategy views (Radar, Options, etc.): How do they start bots? Can bots start individually?
3. Verification & Tests:
   - Check existing tests in backend (`cargo test -p arbitrageur-desktop`) and frontend (`npm test` in `frontend/`).
   - Identify what test files cover bot lifecycle, circuit breaker reset, Header pause/resume, and emergency halt.

OUTPUT:
Write your comprehensive analysis and implementation plan to:
`/mnt/Data/Projects/Arbitrageur/.agents/teamwork/orchestrator_2/survey_1/handoff.md`
Report completion via `send_message` with your findings summary.
