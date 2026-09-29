# Project: Arbitrageur Safety, UI & Profitability Overhaul

## Architecture
Arbitrageur is a multi-venue algorithmic trading platform combining a Rust backend engine (`arbitrageur-core`, `arbitrageur-desktop` with Tauri) and a React/TypeScript frontend.
- **Backend Architecture**:
  - `execution.rs`: Orchestrates order execution, signal lifecycle, emergency unwinds, and `stop_all_bots`.
  - `commands.rs`: Handles Tauri commands including engine pause/unpause, reset, and bot configurations.
  - `coordinator.rs`: Coordinates atomic cross-venue arbitrage, quote analysis, and trading gate checks.
  - `circuit_breaker.rs`: Monitors portfolio equity, drawdowns, and enforces system-wide trading halts.
  - `external_positions.rs`: Tracks and reconciles non-engine venue positions.
  - `reconciliation.rs`: Detects order status discrepancies and classifies fills / residuals.
  - `dev_rpc.rs`: Unix domain socket server (`/tmp/arb-dev.sock`) handling administrative RPC commands.
  - Strategies: Radar mean reversion (`auto_exec.rs`, `convergence_auto.rs`, `monitor.rs`), Spatial arbitrage, Triangular arbitrage, StatArb cointegration, Funding rate carry (`funding.rs`), Options parity (`options_bot.rs`).
- **Frontend Architecture**:
  - `App.tsx`: Central desktop shell, status polling, pause/resume, and state management.
  - `components/Header.tsx`: Header bar with emergency halt, circuit breaker status, and pause/resume button.
  - `components/BotLifecycleControls.tsx`: Standardized lifecycle buttons and `BotPhaseBadge` / `StrategyStatusBadge`.
  - `components/dashboard/`: Gadgets (`ExchangeVenueGadget`, `ExternalPositionsGadget`, `NetPnlChartGadget`, `PortfolioMonitor`, `RebalanceEstimatorGadget`, `SignalTrendGadget`, `TickerTelemetryGadget`, `TradedOpportunitiesGadget`).
  - `components/modals/`: Modal dialogs (`AutotunePreviewModal`, `ConfirmActionModal`, `PositionDetailModal`, `SettingsModal`, `TickerConfigModal`, `VaultUnlockModal`).
  - `components/common/`: Shared atomic controls (`ToggleSwitch`, buttons, badges).
  - Strategy Panels: `OpportunityRadar.tsx`, `FundingRadar.tsx`, `LiveMatrix.tsx`, `TriangularArbitrage.tsx`, `SpotArbitrageBot.tsx`, `PredictionHub.tsx`, `OptionsArbitrage.tsx`.

## Feature Inventory
| # | Feature | Description | Milestone | Source |
|---|---------|-------------|-----------|--------|
| 1 | R1.1 Issues #32-#61 Audit | Audit and document verification of closed issues #32-#61 and recent commits | M1_Legacy | survey_1 |
| 2 | R1.2 Critical Bug Domain Verification | Validate partial-fill trim, order truth, fee deductions, circuit breaker retention, EVM AMM logic | M1_Legacy | survey_1 |
| 3 | R2.1 Circuit Breaker & Coordinator Freeze | Instant trip of circuit breaker and `is_paused = true` freeze of all trading dispatch | M2_Legacy | survey_2 |
| 4 | R2.2 Concurrent Venue Order Cancellation | Concurrently cancel all active resting orders across all connected trading venues | M2_Legacy | survey_2 |
| 5 | R2.3 Partitioned Emergency Unwind Dispatch | Concurrently dispatch emergency market unwinds across engine & external positions with deduplication | M2_Legacy | survey_2 |
| 6 | R2.4 Fill Polling & Untradeable Residual Accounting | Bounded fill polling via `fetch_order` and classification of sub-step dust into `untradeable_residuals` | M2_Legacy | survey_2 |
| 7 | R2.5 Dev RPC & UI Stop All Integration | Wire `stop_all` to Dev RPC and update `Header.tsx` modal and confirmation action | M2_Legacy | survey_2 |
| 8 | R3.1 Toggle Switch Consolidation | Consolidate duplicate `SwitchToggle` vs `ToggleSwitch` and inline button switches into unified `ToggleSwitch` | M3_Legacy | survey_3 |
| 9 | R3.2 Modal & Gadget Styling Harmonization | Harmonize backdrop blurs, z-index, card borders, typography, and status badges across all gadgets & modals | M3_Legacy | survey_3 |
| 10 | R4.1 Convergence Exit Condition Fix | Fix premature convergence exit in `convergence_auto.rs` line 188 where `friction_band` causes early close | M4_Legacy | survey_3 |
| 11 | R4.2 Slippage & Testnet Spread Toll Protection | Enforce spread-toll ceiling gate and passive exit limit unwinds to prevent crossing wide testnet spreads | M4_Legacy | survey_3 |
| 12 | R4.3 One-Way Exit Cost Accounting | Account for unwind friction in one-way mode rather than assuming zero exit cost | M4_Legacy | survey_3 |
| 13 | R4.4 Options Premium Cashflow Accounting | Correct options conversion accounting so premium cash outflows are not booked as instant realized losses | M4_Legacy | survey_3 |
| 14 | R4.5 Testnet Trade Execution & Validation | Validate profitable or non-negative trade execution against testnet venues using vault password `000000` | M4_Legacy | survey_3 |
| 15 | R1: Bot Status on Halt/Reset & Startup | Transition all bots to stopped (`off`/`idle`), reject unpause while tripped, individual strategy start | M1 | survey_1 |
| 16 | R1: Header Pause/Resume Tooltip & Disablement | Header pause button disabled with tooltip when breaker tripped; resuming engine never starts bots | M1 | survey_1 |
| 17 | R2: Strategy Filter Relocation in NetPnlChart | Move filter dropdown into card top bar next to "P&L & Equity History" title; remove redundant row | M2 | survey_2 |
| 18 | R2: Options Strategy Unification | Unify all options substrategies under single "Options" entry in NetPnlChart and PortfolioMonitor | M2 | survey_2 |
| 19 | R4: Standardized Status Badge Component | Implement `StrategyBotStatus` (Active, Paused, Reducing, Stopped) without "Engine" in BotPhaseBadge | M3 | survey_2 |
| 20 | R3: Fixed Uniform Dock Container Height | Lock bottom dock tabs to uniform `h-96 overflow-y-auto` in FundingRadar and LiveMatrix | M3 | survey_3 |
| 21 | R1/R3/R4/R5: Strategy Panels Integration | Integrate stop-to-off, fixed dock height, and status badge in OpportunityRadar and remaining panels | M4 | survey_1,2,3 |
| 22 | R5: Options Ribbon & 5th Metric Card | Move Premium Outlay to 5th metric card; reorganize configurator, scan, lifecycle to single header row | M4 | survey_3 |
| 23 | E2E & Full Regression Verification | Verify zero failures on `cd frontend && npm test` and `cargo test -p arbitrageur-desktop` | M5 | all surveys |

## Milestones
| # | Name | Scope | Dependencies | Status |
|---|------|-------|-------------|--------|
| M1 | Bot Status on Halt/Reset & Header Controls | Backend `commands.rs`, `execution.rs`, `auto_exec.rs`, `options_bot.rs`; Frontend `useBotLifecycle.ts`, `Header.tsx`, `App.tsx` | none | DONE |
| M2 | Strategy Filter Relocation & Options Unification | `NetPnlChartGadget.tsx`, `PortfolioMonitor.tsx`, `tickers.ts`, tests | none | DONE |
| M3 | Standardized Status Badge & Dock Heights | `BotLifecycleControls.tsx`, `FundingRadar.tsx`, `LiveMatrix.tsx`, tests | none | DONE |
| M4 | Strategy Panels Integration & Options Ribbon Reorganization | `OpportunityRadar.tsx`, `OptionsArbitrage.tsx`, `TriangularArbitrage.tsx`, `SpotArbitrageBot.tsx`, `PredictionHub.tsx`, tests | M1, M3 | DONE |
| M5 | Full Regression & Dual-Track Verification | `EngineSettings.test.ts` fix, full frontend `npm test`, backend `cargo test -p arbitrageur-desktop` | M1, M2, M3, M4 | DONE |

## Interface Contracts

### 1. Bot Lifecycle State Contract (`useBotLifecycle.ts`)
```typescript
export type StrategyBotStatus = "Active" | "Paused" | "Reducing" | "Stopped";

export function registerBotResetListener(listener: () => void): () => void;
export function publishBotPhase(strategyId: string, symbol: string | null | undefined, phase: BotPhase): void;
export function stopAllFrontendBots(): void;
```

### 2. Standardized Status Badge Contract (`BotLifecycleControls.tsx`)
```typescript
export interface BotPhaseBadgeProps {
  phase?: BotPhase;
  isPaused?: boolean;
  status?: StrategyBotStatus;
}
export function resolveBotStatus(phase?: BotPhase, isPaused?: boolean): StrategyBotStatus;
export const BotPhaseBadge: React.FC<BotPhaseBadgeProps>;
export const StrategyStatusBadge = BotPhaseBadge;
```
Badge rules:
- Displays exactly one of: `"Active"` (emerald), `"Paused"` (amber), `"Reducing"` (pulsing amber), `"Stopped"` (neutral slate).
- NEVER renders `"Engine"` or `"Engine Paused"`.
- Positioned immediately to the right of the strategy title header, before any parameter tags.

### 3. Header Pause/Resume Contract (`Header.tsx`)
- When `isTripped`: button is `disabled={true}`, wrapped in `<div title="Trading halted by circuit breaker — reset in Portfolio Monitor before resuming engine">`.
- When resumed: toggles `isPaused` to false. DOES NOT start any bots.
- Backend `set_engine_paused_impl`: rejects `paused = false` with error if circuit breaker is tripped. Calls `stop_all_bots(state)` when `paused = true`.

### 4. Strategy Filter Contract (`NetPnlChartGadget.tsx` & `PortfolioMonitor.tsx`)
- Dropdown is placed inside the card header `<div className="flex items-center gap-3 min-w-0 flex-wrap">` directly to the right of the `<h2>P&L & Equity History</h2>` title.
- Dropdown options list contains `"Options"`, which maps and aggregates all `options_*` substrategies. Substrategy keys (`options_parity`, `options_box_spread`, `options_jelly_roll`, `options_convexity`) are excluded from the dropdown options.

### 5. Fixed Dock Tab Height Contract (`OpportunityRadar.tsx`, `FundingRadar.tsx`, `LiveMatrix.tsx`)
- Outer container around bottom dock tab contents: `className="p-3.5 h-96 overflow-y-auto"`.
- Removes redundant inner `max-h-96` wrappers. Content height is locked to 384px, eliminating layout jumps across all 4 tabs.

### 6. Options Ribbon & Metrics Contract (`OptionsArbitrage.tsx`)
- Single header row: `className="shrink-0 px-4 py-3 flex items-center justify-between gap-3 bg-surface rounded-xl border border-surface-border shadow-2xs select-none"`.
- "Premium Outlay" removed from the ribbon pills.
- 5th metric card added to the metrics grid:
  `className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-4"`.
  Card displays "Premium Outlay" with formatted USD or "—" when null.

## Code Layout
- Backend:
  - `frontend/src-tauri/src/commands.rs`: `set_engine_paused_impl` circuit breaker checks & stop_all_bots.
  - `frontend/src-tauri/src/execution.rs`: `stop_all_bots` & `reset_circuit_breaker_impl`.
  - `frontend/src-tauri/src/auto_exec.rs`: Radar bot mode rejection when tripped.
  - `frontend/src-tauri/src/options_bot.rs`: Options bot mode rejection when tripped.
- Frontend Core:
  - `frontend/src/hooks/useBotLifecycle.ts`: Global lifecycle hooks & reset listeners.
  - `frontend/src/components/Header.tsx`: Header pause/resume button with tooltip.
  - `frontend/src/components/BotLifecycleControls.tsx`: Standardized `BotPhaseBadge` & `StrategyStatusBadge`.
  - `frontend/src/App.tsx`: Global event orchestration & reset calls.
- Strategy Views:
  - `frontend/src/components/OpportunityRadar.tsx`: Dock height `h-96`, status badge, stop-to-off.
  - `frontend/src/components/FundingRadar.tsx`: Dock height `h-96`, status badge.
  - `frontend/src/components/LiveMatrix.tsx`: Dock height `h-96`, status badge.
  - `frontend/src/components/OptionsArbitrage.tsx`: Single row header, 5th metric card, status badge, stop-to-off.
  - `frontend/src/components/TriangularArbitrage.tsx`: Status badge, remove Engine Paused.
  - `frontend/src/components/SpotArbitrageBot.tsx`: Status badge.
  - `frontend/src/components/PredictionHub.tsx`: Status badge.
- Dashboard Gadgets:
  - `frontend/src/components/NetPnlChartGadget.tsx`: Relocated filter dropdown & options unification.
  - `frontend/src/components/PortfolioMonitor.tsx`: Options unification.
- Tests:
  - `frontend/src/components/Header.test.ts`
  - `frontend/src/components/BotLifecycleControls.test.ts`
  - `frontend/src/components/NetPnlChartGadget.test.ts`
  - `frontend/src/components/FundingRadar.test.ts`
  - `frontend/src/components/OpportunityRadar.test.ts`
  - `frontend/src/components/OptionsArbitrage.test.ts`
  - `frontend/src/components/EngineSettings.test.ts`
