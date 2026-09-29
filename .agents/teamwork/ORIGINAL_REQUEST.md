# Original User Request

## 2026-09-29T08:32:11Z

# Teamwork Project Prompt

Requested team: Multi-agent team with diverse perspectives: Gemini 3.8, DeepSeek-V4.1-Flash (with max available efforts) offered by OpenCode Go from opencode through ACP, and SWE-2 MAX from Devin through ACP.

A multi-agent codebase scan, verification of past closed issues, safety overhaul ensuring 'Stop All' halts operations, cancels all resting orders, and flattens all open positions, dashboard UI/gadget harmonization, and quantitative bot profitability optimization on testnet with temp password 000000.

Working directory: /mnt/Data/Projects/Arbitrageur
Integrity mode: development

## Requirements

### R1. Repository Audit & Validation of Past Resolved Issues (Phase 1)
Scan the entire repository including crates, frontend, documentation, and GitHub issues (#32 through #61 and recent commits). Verify whether closed bug fixes and features were truthfully and fully implemented without regression (such as partial-fill trim accounting, order truth, fee deductions, circuit breaker retention, and EVM AMM exact-amount logic). Produce an explicit audit assessment flagging any half-implemented or falsely certified issues.

### R2. 'Stop All' Full Emergency Kill & Flatten Overhaul (Phase 1)
Upgrade the 'Stop All' action in the UI and backend into an immediate, fail-closed emergency shutdown:
- Immediately trip the circuit breaker and freeze all automated trading and coordinator dispatch.
- Concurrently cancel all active resting orders across all connected trading venues.
- Concurrently dispatch emergency market unwind orders to flatten all active positions (both engine-tracked and external venue positions).
- Ensure venue-level fill polling confirms flat status, accurately accounts for any untradeable residuals, and leaves the system in a verifiable safe state.

### R3. Dashboard Gadgets & Windows Design Harmonization (Phase 2)
Audit and unify all dashboard gadgets and modal windows (e.g., `*Gadget.tsx`, `*Modal.tsx`, panels, and duplicate toggle implementations like `SwitchToggle` vs `ToggleSwitch`). Establish consistent design tokens across cards, borders, typography, status badges, and interaction states so that all components adhere to a unified visual language and ergonomics.

### R4. Quantitative Bot Profitability & Execution Tuning on Testnet (Phase 2)
Investigate why automated trading bots (Radar, Spatial Arbitrage, Triangular Arbitrage, StatArb, Basis) are not achieving positive returns, and optimize their execution on testnet:
- Re-evaluate hurdle rates, slippage modeling, fee structures, and gas/network transaction costs to ensure bots only trigger trades with positive net expected value.
- Inspect and fix quote freshness, funding accrual attribution, and qualification criteria.
- Test against live testnet venues using vault unlock password `000000` to verify reliable, profitable paper/testnet trade execution.

## Verification Resources
- Desktop backend test suite: `cargo test -p arbitrageur-desktop`
- Frontend test suite: `cd frontend && npm test`
- Rust linting & formatting: `cargo clippy --all-targets` and `cargo fmt --check`
- Local dev RPC socket: `/tmp/arb-dev.sock` for live status inspection (`open_positions`, `external_positions`, `system_status`)
- Testnet environment credentials unlocked via password: `000000`

## Acceptance Criteria

### Phase 1: Audit & Critical Safety
- [ ] Audit assessment documented covering closed issues #32-#61 with clear pass/fail/regression status for each.
- [ ] Clicking 'Stop All' trips circuit breaker, cancels resting orders on all active venues, and flattens all open positions down to zero or documented untradeable residual.
- [ ] System status confirms flat positions and tripped circuit breaker via `/tmp/arb-dev.sock` dev RPC.
- [ ] All existing backend and frontend test suites pass with zero regressions.

### Phase 2: UI Consistency & Bot Profitability
- [ ] Redundant components (e.g., duplicate switch controls) consolidated into single standard implementations.
- [ ] Visual consistency verified across all dashboard gadgets and modals (borders, typography, headers, padding).
- [ ] Bot trade qualification rejects non-profitable opportunities after full accounting of venue fees and slippage.
- [ ] Successful testnet execution cycle executed and validated with positive or non-negative net PnL under testnet conditions.

## Follow-up — 2026-09-29T15:51:57Z

Comprehensive UI/UX, Bot Lifecycle, and Strategy Controls Overhaul for Arbitrageur Desktop.

Working directory: /mnt/Data/Projects/Arbitrageur
Integrity mode: development

## Requirements

### R1. Bot Status on Halt / Emergency Reset & Individual Startup
After halt, killswitch, emergency reset, or circuit breaker reset, all automated bot statuses across all strategies (Radar Bot, Options Bot substrategies, and frontend useBotLifecycle hooks) must be stopped (idle / off), never paused. The Header pause/resume button must not resume any automated bots and must be disabled while the circuit breaker is tripped; each strategy's bot must be started individually using its own strategy-level Start button.

### R2. Strategy Filter Relocation & Options Strategy Unification
In NetPnlChartGadget.tsx (and PortfolioMonitor.tsx), move the strategy filter dropdown from below the card header up into the top bar of the card directly to the right of the "P&L & Equity History" title. Unify all options substrategies so that "Options" appears as a single unified strategy in the dropdown.

### R3. Fixed Window/Dock Height Across Tabs
In OpportunityRadar.tsx (and related dockable strategy panels such as FundingRadar.tsx), fix the container height across all bottom dock tabs (Current Positions, Execution History, Opportunity History, Log Monitor) so that switching tabs maintains a consistent window height without layout jumping or resizing.

### R4. Standardized Status Tag Across All Strategies
All strategy headers must display a standardized bot status badge positioned to the right of the strategy title header, before any parameter tags. Remove "Engine" from the text; the badge must display exactly one of four statuses: Active, Paused, Reducing, or Stopped.

### R5. Options Arbitrage Ribbon & Metrics Gadget Reorganization
In OptionsArbitrage.tsx, remove the "Premium Outlay" parameter tag from the strategy ribbon and add a 5th metric card for Premium Outlay alongside the existing 4 metrics gadgets. Reorganize the configurator, scan, and bot lifecycle controls to sit on the same single header row as the strategy title and status. If space is tight, deprioritize or prune less critical parameter tags so the entire header fits on a single line.

## Acceptance Criteria

### Bot Lifecycle & Controls
- [ ] Emergency stop and circuit breaker reset set all bot configs to disabled/off and frontend phases to idle (stopped).
- [ ] The Header button does not start any bots when resumed, and is disabled with an explanatory tooltip while the circuit breaker is tripped.
- [ ] Each strategy's bot can only be started individually via its own Start button.

### Strategy Filter & Options Grouping
- [ ] The strategy filter in the P&L & Equity History gadget is positioned on the top header row next to the title.
- [ ] The strategy filter displays a single "Options" entry rather than separate options substrategies.

### Layout & Sizing Consistency
- [ ] Switching between Current Positions, Execution History, Opportunity History, and Log Monitor in Opportunity Radar maintains a fixed, uniform height.
- [ ] All strategy headers display the standardized status tag (Active | Paused | Reducing | Stopped) before parameter tags.
- [ ] Options Arbitrage displays the Premium Outlay metric card in the metrics grid, and houses the configurator, scan, and lifecycle buttons on the same header row as the strategy title.
- [ ] cd frontend && npm test and cargo test -p arbitrageur-desktop pass with zero failures.

## Follow-up — 2026-09-29T16:28:56Z

The server just restarted. Please continue executing your remaining tasks and milestones (M1, M3, M4) as planned. Note that the parent agent is handling the user's specific request for:
1) adding the dock tab icons/symbols to FundingRadar.tsx (Wallet, History, Radar, Terminal) matching OpportunityRadar (spread radar);
2) updating the bottom half gadgets and layout of SpotArbitrageBot.tsx (Spatial strategy) to match FundingRadar/OpportunityRadar (full-width scanner + 4-tab docked container for Current Positions, Execution History, Opportunity History, Log Monitor).
Please resume your work on your assigned requirements (R1 bot lifecycle/circuit breaker startup, R4 status badges across remaining strategies, and R5 Options Arbitrage ribbon & metrics reorganization).

## Follow-up — 2026-09-29T16:47:41Z

The server just restarted. Please continue executing your remaining tasks and milestones.
Status summary:
- Completed: Survey, M2, M3.
- In Progress: M1, M4 (Options Arbitrage ribbon & 5th metric card for Premium Outlay, single header row for controls, status badges).
- Note: Parent agent resolved the StrategyParamsDto default target_profit_bps backend test issue and completed FundingRadar dock icons & SpotArbitrageBot bottom half layout.
Please re-establish any stopped crons and resume orchestrator_2 and active workers for M1, M4, and M5.

## Follow-up — 2026-09-29T17:05:53Z

The server just restarted. Please continue executing your remaining tasks and milestones.
Exact M5 Gate Status from disk (.agents/teamwork/orchestrator_2/):
- Milestones M1, M2, M3, M4: DONE.
- M5 Gate Status:
  - challenger_1/handoff.md: DONE (APPROVED)
  - reviewer_1/handoff.md: DONE
  - reviewer_2/handoff.md: DONE
  - challenger_2: progress.md exists, handoff.md was in-flight at restart (ChallengerR3R4R5.test.ts already created and passing 100%)
  - auditor_1: progress.md exists, handoff.md was in-flight at restart
Please re-schedule your monitoring crons and instruct orchestrator_2 to revive challenger_2 and auditor_1 to finish their handoff.md files, synthesize the M5 gate, and complete the Victory Audit.
