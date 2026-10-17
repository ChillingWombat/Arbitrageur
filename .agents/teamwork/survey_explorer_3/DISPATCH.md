## 2026-09-29T08:35:11Z
You are Survey Explorer 3.
Your working directory: /mnt/Data/Projects/Arbitrageur/.agents/teamwork/survey_explorer_3/
Orchestrator Conversation ID: 939109d4-58d1-4e45-82e4-b4d4e6ace8d9

MANDATORY READS:
1. /mnt/Data/Projects/Arbitrageur/.agents/teamwork/ORIGINAL_REQUEST.md
2. /mnt/Data/Projects/Arbitrageur/AGENTS.md

YOUR MISSION:
Perform a comprehensive survey and investigation for R3 (Dashboard Gadgets & Windows Harmonization) and R4 (Quantitative Bot Profitability & Execution Tuning on Testnet).
1. For R3 (Dashboard UI Harmonization):
   - Enumerate all dashboard gadgets (`*Gadget.tsx`), modal windows (`*Modal.tsx`), panels, and controls in `frontend/src/`.
   - Identify duplicate components (e.g. `SwitchToggle` vs `ToggleSwitch` or similar).
   - Audit visual styling: cards, borders, typography, status badges, padding, colors, interaction states.
   - Propose unification strategy and design token harmonization.
2. For R4 (Quantitative Bot Profitability & Execution Tuning):
   - Enumerate all trading bots/strategies in the codebase (Radar, Spatial Arbitrage, Triangular Arbitrage, StatArb, Basis).
   - Inspect hurdle rates, slippage modeling, fee structures, and gas/network costs.
   - Inspect quote freshness checks, funding accrual attribution, and qualification criteria.
   - Identify why bots are not achieving positive returns and determine exact fixes for profitable testnet execution.
   - Inspect dev RPC (`/tmp/arb-dev.sock`) and testnet vault unlocking with password `<redacted — see SECURITY.md>`.
3. Output:
   - Write your survey report to `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/survey_explorer_3/survey_r3_r4.md`.
   - Write your handoff to `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/survey_explorer_3/handoff.md` with sections: Observation, Logic Chain, Caveats, Conclusion, Verification Method.
   - Regularly update `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/survey_explorer_3/progress.md` with timestamps.
5. Notify orchestrator via send_message when complete.
