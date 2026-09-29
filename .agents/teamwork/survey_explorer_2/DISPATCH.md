## 2026-09-29T08:35:11Z
You are Survey Explorer 2.
Your working directory: /mnt/Data/Projects/Arbitrageur/.agents/teamwork/survey_explorer_2/
Orchestrator Conversation ID: 939109d4-58d1-4e45-82e4-b4d4e6ace8d9

MANDATORY READS:
1. /mnt/Data/Projects/Arbitrageur/.agents/teamwork/ORIGINAL_REQUEST.md
2. /mnt/Data/Projects/Arbitrageur/AGENTS.md

YOUR MISSION:
Perform a comprehensive survey and investigation for R2: 'Stop All' Full Emergency Kill & Flatten Overhaul.
1. Investigate the current 'Stop All' action in the UI (frontend components, buttons, state stores) and backend (desktop commands, trading coordinator, engine, dev RPC `/tmp/arb-dev.sock`).
2. Map out the full architecture needed for immediate, fail-closed emergency shutdown:
   - Immediately trip the circuit breaker and freeze all automated trading and coordinator dispatch.
   - Concurrently cancel all active resting orders across all connected trading venues.
   - Concurrently dispatch emergency market unwind orders to flatten all active positions (both engine-tracked open positions AND external venue positions).
   - Ensure venue-level fill polling confirms flat status, accurately accounts for any untradeable residuals (dust / min order size), and leaves the system in a verifiable safe state.
3. Identify existing code paths, gaps, race conditions, and error recovery needed for fail-closed safety.
4. Output:
   - Write your survey report to `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/survey_explorer_2/survey_r2.md`.
   - Write your handoff to `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/survey_explorer_2/handoff.md` with sections: Observation, Logic Chain, Caveats, Conclusion, Verification Method.
   - Regularly update `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/survey_explorer_2/progress.md` with timestamps.
5. Notify orchestrator via send_message when complete.
