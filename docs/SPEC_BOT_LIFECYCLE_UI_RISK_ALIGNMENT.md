# Spec: Bot Lifecycle Disarmament, Strategy Dock & Header Standardization, and Risk Dust/FX Alignment

## Problem Statement

As a quantitative operator running a multi-venue arbitrage desk across perpetuals, spot, funding rate, triangular, prediction, and options strategies, several lifecycle, layout, and risk-accounting inconsistencies created operational friction and false alarms:

1. **Ambiguous Bot Lifecycle After Halts and Resets**: Triggering an emergency stop, killswitch, or circuit breaker reset left automated bots in a paused state or allowed the global Header pause/resume toggle to be mistaken for a master bot starter. While the circuit breaker was tripped, the Header resume control remained clickable, and operators lacked a strict guarantee that every strategy bot was disarmed (`Stopped` / `off`) until individually re-started on its own strategy panel.
2. **Fragmented Strategy Layouts, Jumping Dock Heights, and Inconsistent Status Badges**:
   - Switching between bottom dock tabs (`Current Positions`, `Execution History`, `Opportunity History`, `Log Monitor`) caused the dock container height to jump or rendered double nested scrollbars.
   - Spatial Strategy used a legacy two-column layout instead of the full-width scanner table + staging modal + 4-tab bottom dock architecture used by Opportunity Radar and Funding Radar, and Funding Radar's dock tabs lacked icons.
   - Strategy headers displayed inconsistent status wording (including `"Engine Paused"`) instead of a single standardized status badge (`Active` | `Paused` | `Reducing` | `Stopped`) positioned immediately right of the strategy title.
   - Options Arbitrage crowded `"Premium Outlay"` into the top ribbon pills, wrapped its header controls across multiple lines, and only displayed 4 metric cards.
3. **Portfolio Monitor & Chart Filter Fragmentation**:
   - The `P&L & Equity History` chart placed its strategy filter on a secondary row below the card header, and exposed individual options substrategies (`options_parity`, `options_box_spread`, etc.) separately rather than grouping them under a unified `"Options"` strategy entry.
   - The Capital Allocation gadget lacked a visual pie/donut breakdown and a toggle between venue allocation and strategy open-position allocation.
4. **False Risk Warnings from Coin-Collateral FX Basis and Sub-Dollar Option Dust**:
   - On coin-margined venues (such as Deribit holding large BTC and ETH collateral balances), the circuit breaker's coin-collateral beta freeze valued current FX using cross-venue perpetual orderbook mids rather than the venue adapter's own settlement index used to compute account equity. Small basis divergences (~0.24%) or paused WebSocket feeds produced phantom multi-thousand-dollar equity swings that falsely tripped the daily dollar drawdown circuit breaker.
   - Worthless deep-OTM option remnants (notional `< $1.00`, with empty bid books on the exchange) were filtered out as untradeable dust by `External Position` and `Stop All`, yet were still fed raw into `DeltaGuard` and the Perp Ticker Explorer — lighting up `BTC-USD Perp` and `ETH-USD Perp` with red unbalanced-position warning badges and `Residual` DeltaGuard rows even when the portfolio was completely flat.
   - Resetting transaction history cleared trade ledgers and realized PnL, but left cumulative option premium outlay and tripped circuit breaker snapshots latched.

## Solution

A cohesive overhaul of bot lifecycle controls, strategy workspace layouts, portfolio monitoring gadgets, and risk telemetry alignment:

1. **Strict Bot Disarmament & Individual Strategy Startup**:
   - Emergency stops, halts, killswitches, and circuit breaker resets disarm all automated bots (`enabled = false`, `mode = "off"`, frontend phase `idle` / `Stopped`) across Radar Bot, all Options Bot substrategies, and Fixed-Yield automation.
   - The global Header pause/resume button is disabled with an explanatory tooltip whenever the circuit breaker is tripped, and resuming the engine strictly unpauses execution coordination without auto-starting any strategy bots. Each strategy bot can only be started individually via its own strategy-level Start control.
2. **Standardized Strategy Headers, Single-Row Options Ribbon, and Uniform 4-Tab Bottom Docks**:
   - Every strategy header renders a standardized status badge (`Active`, `Paused`, `Reducing`, or `Stopped`, never `"Engine"`) immediately to the right of the strategy title before any parameter pills.
   - Opportunity Radar, Funding Radar, Live Matrix, and Spatial Strategy share a locked `384px` (`h-96 overflow-y-auto`) bottom dock container with icon-equipped tabs (`Current Positions`, `Execution History`, `Opportunity History`, `Log Monitor`) and no nested scrollbars.
   - Spatial Strategy uses a full-width Opportunity Scanner table with a dedicated execution staging modal and the standardized 4-tab bottom dock.
   - Options Arbitrage moves `"Premium Outlay"` out of the header ribbon into a 5th metric card in a 5-column metrics grid, keeping the title, status badge, configurator, scan, and lifecycle buttons on a single non-wrapping header line.
3. **Unified Portfolio Monitor & P&L History Filtering**:
   - The `P&L & Equity History` strategy filter sits on the top card header row directly right of the title and collapses all options substrategies into a single `"Options"` filter option across both the chart and Portfolio Monitor.
   - Capital Allocation renders as the second gadget from the top with an interactive donut/pie chart and a `By Venue` / `By Strategy` mode switcher, while External Position uses simplified header and empty-state copy (`"External Position"` / `"No external position"`).
4. **Aligned Risk Telemetry, Coin-Collateral Index Neutralization, and Sub-Dollar Dust Filtering**:
   - Coin-collateral drawdown freezing uses the venue adapter's own recorded settlement index (`last_account_index_usd`) first before falling back to cross-venue orderbook mids, eliminating phantom index-vs-perp basis drawdowns on coin-margined accounts.
   - Risk overview position aggregation and the Perp Ticker Explorer apply the same `$1.00` dust notional threshold as `External Position` and `Stop All`, preventing untradeable sub-dollar option remnants from triggering false DeltaGuard `Residual` rows or red ticker warnings.
   - Resetting transaction history also zeroes cumulative option premium outlay (`total_premium_outlay`) and resets/re-anchors the circuit breaker.

## User Stories

1. As an operator, I want emergency stops, halts, and killswitches to set every automated strategy bot to `Stopped` (`off` / `idle`) rather than `Paused`, so that no bot can silently resume trading after an emergency.
2. As an operator, I want resetting the circuit breaker to keep all automated strategy bots in the `Stopped` state, so that I must explicitly choose which individual strategy bots to re-enable.
3. As an operator, I want the Header pause/resume button to be disabled when the circuit breaker is tripped, so that I cannot accidentally attempt to resume engine coordination before resetting the breaker in Portfolio Monitor.
4. As an operator, I want hovering over the disabled Header pause/resume button while halted to show a clear tooltip explaining that trading is halted by the circuit breaker and must be reset in Portfolio Monitor first, so that the recovery workflow is self-explanatory.
5. As an operator, I want clicking `Resume Engine` in the Header to only resume execution coordination while leaving all automated bots stopped, so that global engine unpausing never triggers unintended automated order placement.
6. As an operator, I want each strategy's automated bot to be startable only via its own strategy-level Start button, so that bot activation is always scoped to a single explicitly reviewed strategy.
7. As an operator, I want every strategy panel header to display a standardized status badge (`Active`, `Paused`, `Reducing`, or `Stopped`) immediately to the right of the strategy title and before parameter pills, so that I can read strategy status at a consistent focal point across all tabs.
8. As an operator, I want the word `"Engine"` removed from strategy-level status badges, so that strategy bot status is never confused with the global engine pause state.
9. As an operator, I want switching between `Current Positions`, `Execution History`, `Opportunity History`, and `Log Monitor` in any strategy dock to maintain a fixed, uniform container height (`384px`), so that the viewport does not jump or resize when inspecting different telemetry tabs.
10. As an operator, I want inner redundant scroll wrappers removed from bottom dock tab panels, so that tables and logs scroll smoothly within a single outer scroll container rather than trapping scroll events in nested scrollbars.
11. As an operator, I want Funding Radar's bottom dock tabs to display the same semantic icons (`Wallet`, `History`, `Radar`, `Terminal`) as Opportunity Radar, so that navigation across strategy docks is visually uniform.
12. As an operator, I want Spatial Strategy to feature a full-width Opportunity Scanner table, a dedicated route staging modal, and the standardized 4-tab bottom dock (`Current Positions`, `Execution History`, `Opportunity History`, `Log Monitor`), so that spot arbitrage workflows match Opportunity Radar and Funding Radar.
13. As an operator, I want the `"Premium Outlay"` pill removed from the Options Arbitrage header ribbon and displayed as a 5th metric card in a 5-column metrics grid alongside `Scanned Setups`, `Best Net Edge`, `Executable`, and `Realized PnL`, so that options financing outlay has dedicated visibility without crowding the control ribbon.
14. As an operator, I want the Options Arbitrage header ribbon to house the strategy title, status badge, configurator, scan button, and bot lifecycle controls on a single non-wrapping row, so that vertical workspace height is preserved for the options scanner.
15. As an operator, I want the strategy filter dropdown in `P&L & Equity History` positioned on the top header row directly to the right of the card title, so that the chart header is compact and does not waste a second toolbar row.
16. As an operator, I want all options substrategies (`options_parity`, `options_box_spread`, `options_jelly_roll`, `options_convexity`, `options_calendar`, `options_butterfly`) unified under a single `"Options"` filter option in both `P&L & Equity History` and `Portfolio Monitor`, so that I can filter all options performance with one selection.
17. As an operator, I want the Capital Allocation gadget positioned as the second gadget from the top in Portfolio Monitor and upgraded to an interactive donut/pie chart with a `By Venue` / `By Strategy` switch tab, so that I can inspect both exchange collateral distribution and active strategy capital deployment.
18. As an operator, I want the External Position gadget header titled `"External Position"` with an empty state reading `"No external position"`, so that external exposure telemetry is concise and unambiguous.
19. As a quantitative trader using coin-margined venues like Deribit, I want the circuit breaker's coin-collateral FX beta adjustment to use the exact venue settlement index rate embedded in the account snapshot before falling back to cross-venue orderbook mids, so that basis differences between exchange index prices and perpetual orderbooks do not trigger false daily drawdown halts.
20. As a quantitative trader, I want locked-vault startup polls (where zero venue accounts have been read yet) to skip feeding a `$0` total into the circuit breaker baseline, so that unlocking the vault does not compare live balances against a zeroed startup anchor.
21. As a quantitative trader, I want sub-`$1.00` position remnants (such as expiring deep-OTM options with empty bid books) filtered out of Risk Overview's DeltaGuard calculation and the Perp Ticker Explorer warning check using the same `$1.00` dust threshold as `External Position` and `Stop All`, so that untradeable sub-dollar dust does not trigger false `Residual` rows or red ticker warning badges when the portfolio is flat.
22. As an operator, I want `Reset Transaction History` to also reset cumulative option premium outlay (`total_premium_outlay`) and reset/re-anchor the circuit breaker, so that a history reset restores a completely clean telemetry and risk baseline.
23. As an operator, I want circuit breaker drawdown trip reasons to format dollar and percentage losses to 2 decimal places, so that halt banners never display raw 21-decimal fixed-point strings.
24. As a developer orchestrating multi-agent workflows, I want subagent milestone state and verification handoffs persisted in a durable manifest with a recovery CLI script, so that multi-agent work can resume seamlessly after an application or server restart without repeating completed milestones.

## Implementation Decisions

### 1. Bot Lifecycle Disarmament & Header Circuit Breaker Gate
- Centralized bot disarmament into a shared backend helper invoked by emergency flatten/stop (`stop_all` / `killswitch`) and circuit breaker reset (`reset_circuit_breaker`). It persists `mode = "off"` and `enabled = false` for Radar Bot, all live Options Bot substrategies (clearing in-memory tick statuses to `"off"`), and Fixed-Yield automation (`engine.auto_fixed_yield_enabled = "0"`).
- On the frontend, the global bot lifecycle registry exposes a reset-all-to-idle transition (`idle` phase, cleared pause reason) invoked whenever an emergency stop or circuit breaker reset completes.
- The Header pause/resume control checks `status.circuit_breaker_tripped`. While tripped, the button is disabled (`disabled = true`) and wrapped in a tooltip container explaining that the circuit breaker must be reset in Portfolio Monitor before resuming the engine. Clicking `Resume Engine` only toggles engine pause state (`set_engine_paused(false)`) and never dispatches bot start commands.

### 2. Standardized Strategy Status Badge (`StrategyStatusBadge` / `BotPhaseBadge`)
- Defined a strict 4-state status type and pure mapping function:
  ```ts
  export type StrategyBotStatus = "Active" | "Paused" | "Reducing" | "Stopped";

  export function resolveBotStatus(
    phase: BotPhase | undefined,
    isPaused?: boolean,
  ): StrategyBotStatus {
    if (phase === "reduce_only" || phase === "stopping") return "Reducing";
    if (phase === "paused") return "Paused";
    if (phase === "running") return isPaused ? "Paused" : "Active";
    return "Stopped";
  }
  ```
- Rendered this badge immediately right of the strategy title header (before any parameter tags) across all 7 strategy panels (Opportunity Radar, Funding Radar, Live Matrix, Spatial Strategy, Triangular Arbitrage, Prediction Hub, and Options Arbitrage), removing all trailing `"Engine Paused"` warning pills.

### 3. Uniform Bottom Dock Container & Spatial Strategy Overhaul
- Standardized the bottom dock content wrapper across dockable strategy views to `className="p-3.5 h-96 overflow-y-auto"` (`384px` fixed height) and removed nested `max-h-96` wrappers from child tab views (`PositionsPanel`, `TickerTransactionHistory`, `TradedOpportunitiesTable`, `TickerLogMonitor`) to eliminate layout height jumping and double scrollbars.
- Upgraded Spatial Strategy (`SpotArbitrageBot`) to a full-width Opportunity Scanner table with search/asset filters, a dedicated `SpotExecutionModal` for route staging and atomic dispatch, and the standardized 4-tab icon-equipped bottom dock.

### 4. Options Arbitrage Single-Row Header & 5-Column Metrics Grid
- Removed the `"Premium Outlay"` pill from the Options Arbitrage header ribbon and expanded the metrics grid from 4 columns to 5 columns (`grid-cols-2 sm:grid-cols-3 lg:grid-cols-5`), adding a 5th `Premium Outlay` card displaying `total_premium_outlay_usd`.
- Removed `flex-wrap` from the Options Arbitrage header ribbon so the title, status badge, parameter pills, configurator, scan button, and substrategy bot lifecycle controls remain on a single horizontal row.

### 5. Strategy Filter Unification & Relocation
- Relocated the strategy filter dropdown in `P&L & Equity History` into the top header bar immediately right of the title.
- Unified all `options_*` strategy identifiers into a single `"options"` (`"Options"`) category via a shared grouping predicate (`id === "options" || id.startsWith("options_")`) across both `P&L & Equity History` and `Portfolio Monitor`.

### 6. Coin-Collateral FX Beta Neutralization, Dust Alignment & History Reset
- In `breaker_frozen_equity_usd`, non-USD collateral balances now query the venue adapter's `last_account_index_usd(ccy)` (the exact settlement index price used by `fetch_account` when computing `AccountSnapshot.equity`) before falling back to cross-venue orderbook mids (`fx_usd_now`). This guarantees exact cancellation of coin-collateral index drift on coin-margined venues.
- In `get_risk_overview_impl`, `all_positions` filters out positions with `(quantity.abs() * mark_price) < $1.00`, matching the `$1.00` dust threshold in `external_positions` and `emergency_flatten_all`. The Perp Ticker Explorer similarly requires `Math.abs(net_exposure_usd) >= 1.0` before flagging an unbalanced position warning when `is_neutral` is true.
- `reset_transaction_history_impl` now also zeroes `pnl.premium_outlay.{testnet,live}` (`total_premium_outlay`) and invokes `circuit_breaker.reset()` while clearing `last_risk_live_universe`.

## Testing Decisions

### What Makes a Good Test
- Tests must assert observable external behavior, rendered DOM contracts, and public command/RPC outputs rather than private internal state.
- Lifecycle tests must verify that halts and resets disarm persisted bot configs and frontend phases, and that resuming the engine never starts automated bots.
- Risk tests must verify that coin-collateral index moves do not trigger false drawdown trips and that sub-`$1.00` dust remnants do not pollute `net_deltas` or ticker warning states.

### Testing Seams
1. **Tauri Command / Dev RPC Seam (`*_impl` & `/tmp/arb-dev.sock`)**:
   - Exercises backend lifecycle disarmament (`execute_emergency_stop_all_impl`, `reset_circuit_breaker_impl`, `reset_transaction_history_impl`) and risk overview aggregation (`get_risk_overview_impl`, `breaker_frozen_equity_usd`) against `AppEngineState` and SQLite persistence.
2. **Frontend Component & Interactive DOM Seam (`npm test` / `scripts/test-components.mjs`)**:
   - Exercises rendered markup and interactive `happy-dom` state transitions across `Header`, `BotLifecycleControls`, `NetPnlChartGadget`, `PortfolioMonitor`, `OpportunityRadar`, `FundingRadar`, `SpotArbitrageBot`, `TriangularArbitrage`, and `OptionsArbitrage`.

### Prior Art
- Backend command & risk integration tests in `commands::tests` and `execution::tests` (`cargo test -p arbitrageur-desktop`, `cargo test -p arbitrageur-risk`).
- Frontend component & interactive DOM suites in `Header.test.ts`, `BotLifecycleControls.test.ts`, `NetPnlChartGadget.test.ts`, `PortfolioMonitor.test.ts`, `OpportunityRadar.test.ts`, `FundingRadar.test.ts`, `SpotArbitrageBot.test.ts`, `OptionsArbitrage.test.ts`, and `ChallengerR3R4R5.test.ts`.

## Out of Scope
- Modifying exchange matching engine rules on external testnet/live venues (e.g., Deribit testnet orderbooks with empty bid sides on deep-OTM expiring options).
- Adding new database schema migrations beyond existing `kv` and table structures.
- Auto-starting bots via global hotkeys or batch-resume controls (intentionally prohibited by safety design).

## Further Notes
- Verified across 600/600 Rust backend unit tests (`cargo test -p arbitrageur-desktop`), 22/22 risk crate tests (`cargo test -p arbitrageur-risk`), 23/23 frontend test suites (`cd frontend && npm test`), clean production bundle compilation (`cd frontend && npm run build`), and live RPC verification against the running `arbitrageur-testnet.service` instance.
