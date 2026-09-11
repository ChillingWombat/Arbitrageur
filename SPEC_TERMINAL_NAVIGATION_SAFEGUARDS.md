## Problem Statement

Algorithmic arbitrageurs executing high-frequency spatial convergence, funding carry, and statistical arbitrage across decentralized perpetual exchanges (Hyperliquid, Lighter, Orderly/Vanta, PopDEX, Arcus) and prediction markets require uncompromising execution safety, zero simulated-data ambiguity, and continuous real-time risk visibility.

Prior to this specification, operators faced several operational friction points:
1. **Disconnected Navigation & Layout Drift**: Top strategy switchers used floating pill tiles over a recessed background that shifted and broke alignment with the primary ticker selection panel, adding visual noise.
2. **Fragmented Risk & Performance Governance**: Portfolio net delta neutrality (DeltaGuard), liquidation buffers, multi-strategy profit attribution, and Walk-Forward Optimization (WFO) calibration were split across separate secondary pages, impeding holistic risk assessment during volatility spikes.
3. **Constrained Configuration Viewport**: The System Setting dialog used a rigid vertical sidebar that consumed over 200px of horizontal room, compressing API key forms, venue status cards, and multi-venue balance distribution tables into cramped, wrapping containers.
4. **Lack of Non-Destructive Pause Mechanism**: Traders could only stop trading via an emergency killswitch, which forced position unwinds or required complete process termination. There was no mechanism to temporarily suspend order generation while keeping active hedge cycles and open positions intact.
5. **Uncalibrated Margin Defaults**: Initial perpetual positions defaulted to excessive leverage tiers, reducing liquidation safety buffers below institutional risk tolerance.

## Solution

A unified, high-density desktop trading interface and fail-closed execution architecture that provides:
1. **Flush Boundary-Aligned Strategy Switcher**: A minimalist, full-height top strategy bar with vertical dividers, equal column distribution across active strategies, and a crisp bottom accent underline aligned strictly with the ticker panel's right border.
2. **Unified Full-Width Portfolio Monitor**: A dedicated executive dashboard consolidating cross-exchange signed net exposure (DeltaGuard), real-time margin tripwires, multi-strategy PnL attribution, and automated WFO calibration, accessible via a persistent bottom-left control button.
3. **Top-Navigated Full-Width System Settings**: Horizontal section navigation tabs across the top with integrated AES-256-GCM vault security status and trading mode pills, unlocking the entire modal width for exchange credentials and balance monitoring.
4. **Compact Ticker Panel with Fail-Closed Guidance**: A streamlined 288px sidebar with concise guidance (`Requires at least 2 enabled exchanges`), issue-prioritized multi-criteria sorting (Alphabetical, Strategy Trades, Net PnL), and individual bot switches.
5. **Position-Preserving Pause/Continue Engine**: A non-destructive pause switch that suspends automated bot loops and rejects new dispatches while strictly preserving open lots, active positions, and virtual ledger balances.
6. **Standardized 5X Default Leverage**: Automated leverage clamping and API negotiation across all perpetual venues to maintain a minimum 18% liquidation distance.

## User Stories

1. As an arbitrage trader, I want flat, border-divided strategy tabs flush with the application header, so that switching between trading modules feels snappy, institutional, and free of visual clutter.
2. As an arbitrage trader, I want the start divider of the top strategy switcher to align strictly with the right boundary of the ticker panel, so that the terminal displays a clean, continuous vertical grid line from top to bottom.
3. As an arbitrage trader, I want equal-width distribution across all active strategy tabs (`Futures`, `Spot Arb`, `Funding`, `Prediction`, `Stat-Arb`), so that layout widths remain predictable regardless of live opportunity counts.
4. As a risk manager, I want a single unified Portfolio Monitor dashboard combining DeltaGuard net exposure and multi-strategy performance, so that I can evaluate portfolio health in one place without context switching.
5. As an arbitrage trader, I want a persistent Portfolio Monitor access button in the bottom-left panel directly above System Setting, so that I can open executive risk monitoring from any strategy view with one click.
6. As a risk manager, I want the Portfolio Monitor to expand across the full width of the screen on widescreen displays, so that no display real estate is wasted by artificial width clamps.
7. As a risk manager, I want to view a real-time table of cross-exchange signed net exposure per asset with unhedged tolerance limits ($200 default threshold), so that I can verify DeltaGuard is enforcing complete delta-neutrality across all venues.
8. As a risk manager, I want a dedicated Safety Tripwires & Margin Health card displaying margin utilization, distance to liquidation, and consecutive execution error buffers with prominent progress bars, so that I can detect margin exhaustion well before liquidation.
9. As an arbitrage trader, I want multi-strategy PnL attribution breaking down closed cycles, win rates, Sharpe ratios, and annualized APR across strategies, so that I can identify which arbitrage mechanism is generating the highest risk-adjusted yield.
10. As a quantitative researcher, I want to run Walk-Forward Optimization (WFO) parameter calibrations on demand, so that I can determine the optimal spread estimator (VWMA, EMA, SMMA, WMA, SMA) and quote update window for the active ticker.
11. As an operator, I want the System Setting modal to feature a top horizontal navigation bar instead of a left sidebar, so that configuration forms and exchange balance tables have the maximum possible horizontal space.
12. As an operator, I want the System Setting top bar to display the vault encryption status (`Locked` / `Unlocked`) and trading mode (`PAPER` / `LIVE`), so that I know immediately whether private keys are unlocked in memory.
13. As an operator, I want multi-venue balances to display in a spacious grid with health status pills and safety floor indicators, so that I can monitor collateral distribution and prevent single-leg fill failures.
14. As an operator, I want exchange API configuration cards to include venue filter tabs (`All`, `Hyperliquid`, `Lighter`, `Vanta`, `Arcus`, `PopDEX`, `Robinhood`, `Kraken`, `Polymarket`, `Kalshi`), so that I can quickly jump to specific exchange credentials.
15. As an arbitrage trader, I want the ticker explorer panel to be compact (288px width), so that more screen space is dedicated to live orderbook depth and execution matrices.
16. As an operator, I want the ticker explorer to display a concise message (`Requires at least 2 enabled exchanges`) when no exchanges are configured, so that empty states provide clear guidance without taking up excessive vertical room.
17. As an arbitrage trader, I want to sort tickers by Alphabetical (A-Z), Strategy Trades executed, or Total Net Profit/Loss, so that I can prioritize active, profitable markets.
18. As an arbitrage trader, I want tickers with active position issues or balance imbalances to automatically sort to the top of the ticker explorer, so that potential risks receive immediate operator attention.
19. As an arbitrage trader, I want individual switch toggles per ticker to enable or disable automated execution, so that bots only execute trades on markets I have explicitly authorized.
20. As an arbitrage trader, I want a non-destructive `PAUSE` / `CONTINUE` button in the header left of the `STOP` button, so that I can temporarily halt automated execution without liquidating or dumping existing positions.
21. As an arbitrage trader, I want all open lots in the virtual ledger and active exchange positions to remain untouched during engine pause, so that profitable convergence cycles can resume uninterrupted when continued.
22. As an arbitrage trader, I want all perpetual and futures arbitrage dispatches to default to safe 5X leverage, so that liquidation buffers remain protected across volatile cryptocurrency markets.
23. As an operator, I want exchange adapters that lack native leverage API endpoints to gracefully handle leverage requests without crashing, so that execution dispatches succeed across heterogeneous venues.
24. As an auditor, I want a dedicated Ticker Log Monitor tab alongside Ranked Opportunities and Execution History, so that I can review real-time telemetry and debug logs scoped to the active ticker.
25. As an institutional trader, I want 100% fail-closed market data integrity where zero synthetic prices or fake orderbooks are fabricated, so that paper trading executes strictly against real exchange orderbooks.

## Implementation Decisions

### 1. Terminal Layout & Grid Alignment
- **Top Strategy Switcher**: Replaced disconnected floating pill tiles with flush, full-height tabs separated by vertical borders (`divide-x divide-surface-border bg-surface`).
- **Active Tab Visual Indicator**: Implemented a high-contrast bottom accent underline bar (`h-[2.5px] bg-accent`) matching the institutional reference styling.
- **Strict Header-to-Sidebar Alignment**: Configured the top header's left branding container to strictly match the ticker explorer panel width (288px / `w-72`), ensuring the vertical start divider lines up with the content pane border.
- **Strategy Tab Sizing**: Allocated the remaining horizontal space evenly across the 5 actionable trading strategies (`grid grid-cols-5`) to eliminate layout jump when opportunity counters change.

### 2. Consolidated Portfolio Monitor
- **Unified Risk & Performance Scope**: Removed individual `DeltaGuard` and `Performance` tabs from top navigation and merged their telemetry into a unified `PortfolioMonitor` module.
- **Dedicated Access Seam**: Added a full-width `Portfolio Monitor` button with an `Activity` icon in the bottom-left panel directly above `System Setting`. The button highlights with an active accent background when the view is active.
- **Full-Width Canvas**: Removed restrictive `max-w-7xl` container constraints, allowing the dashboard to utilize 100% of available viewport width with generous padding (`p-5 lg:p-7`).
- **High-Density Typography**: Scaled executive KPI metric numbers to `text-xl lg:text-2xl font-black font-mono` and table cell paddings to `py-3.5 px-4`.
- **Badge Wrap Prevention**: Protected status pills (e.g. `Delta Neutral`, `Exposure Skew`) and metric headers with `whitespace-nowrap` to prevent awkward line breaks.

### 3. System Setting Modal Top Navigation
- **Elimination of Sidebar**: Removed the 224px left sidebar in `EngineSettings` in favor of a top horizontal tab bar spanning the modal header.
- **Top Tab Bar Structure**: Arranged 5 section tabs (`Exchanges`, `Balances`, `Execution`, `Network & VPS`, `About`) horizontally with active bottom border indicators.
- **Status Pills Relocation**: Shifted the `Vault: Locked / Unlocked` encryption status badge and `Mode: PAPER / LIVE` trading mode indicator into the right corner of the top section bar.
- **Full-Width Form Views**: Expanded the scrollable content area to 100% of modal width (~1040px), allowing venue filter badges, API key inputs, and multi-venue balance cards to display comfortably without horizontal truncation.

### 4. Engine Trading Pause / Continue Mechanism
- **Shared State Architecture**: Added an atomic `is_paused: RwLock<bool>` flag to the backend engine state and IPC system status payload.
- **Fail-Closed Execution Gate**: Guarded all trade execution endpoints (spatial, cross-ticker, funding carry, spot, prediction) with a fail-closed check returning `Err("Trading engine is PAUSED. Positions are preserved, but new executions are suspended.")`.
- **Automated Bot Loops**: Guarded frontend auto-bot intervals across spatial matrix, funding radar, prediction hub, and spot bot to immediately no-op when paused.
- **Position Preservation Guarantee**: Ensured that the pause mechanism does not touch, unwind, or clear open lots in `virtual_ledger` or exchange positions.
- **Header Control**: Positioned a `PAUSE` (neutral) / `CONTINUE` (amber pulse) toggle button immediately to the left of the `STOP` button in the top-right header.

### 5. Standardized 5X Default Leverage & Safeguards
- **Core Trait Contract**: Added `async fn set_leverage(&self, symbol: &str, leverage: u32) -> anyhow::Result<()>` to `ExchangeAdapter` with a default `Ok(())` no-op implementation for exchanges lacking leverage configuration APIs.
- **Perpetual Defaults**: Updated default leverage configuration across Hyperliquid, Lighter, and simulated engines from 10X to 5X.
- **Safe Leverage Calculation**: Implemented `computeSafeLeverage` in frontend contract specifications to clamp target leverage to the minimum safe ceiling across both trade legs (`Math.min(minVenueLeverage, targetLeverage)`).

## Testing Decisions

### What Makes a Good Test
Tests must verify external behavioral contracts, state transitions, and safety bounds rather than implementation details:
- Assert that entering the paused state prevents new trade dispatches while maintaining active lots in the virtual ledger.
- Assert that unconfigured environments return zero synthetic quotes and empty opportunity sets (fail-closed behavior).
- Assert that ticker filtering properly excludes symbols lacking dual-venue connectivity.
- Assert that leverage calculations clamp to safe venue thresholds.

### Test Coverage & Prior Art
- **Frontend Unit Tests**: Node `--experimental-strip-types` test runner covering:
  - Venue key normalization and lot sizing (`contractSpecs.ts`).
  - Safe leverage and margin calculation bounds.
  - Rolling spread tracking, baseline pre-seeding, and percentile ranks (`matrixQuant.ts`).
  - Real 2D matrix calculation with commission, taker fees, and sliding window deltas.
  - Pause and continue engine state transitions preserving virtual positions (`pauseContinue.test.ts`).
  - Fail-closed ticker filtering based on enabled/connected venues (`tickers.test.ts`).
- **Rust Backend Workspace Tests**: Cargo test suite covering:
  - `arbitrager_core`: Orderbook VWAP, market data hub broadcast, liquidation distance.
  - `arbitrager_exchanges`: Adapter depth consumption, rate limiting, credential validation.
  - `arbitrager_execution`: Virtual ledger cycle netting, atomic execution dispatches.
  - `arbitrager_math`: WFO estimator calibration (VWMA, EMA, SMMA, WMA, SMA), Kalman filter convergence.
  - `arbitrager_risk`: DeltaGuard delta-neutrality enforcement, circuit breaker trip conditions.
  - `arbitrager_desktop`: Tauri IPC command endpoints for status, settings, pause/continue, and risk killswitch.

## Out of Scope

- Implementing direct hardware key signing integrations (Ledger/Trezor) beyond the existing Remote HSM token interface.
- Automatic capital deposits or fiat on-ramping into exchange accounts.
- Automated liquidation of unhedged positions when the user manually closes a single leg via external third-party web interfaces (covered by manual tripwire and DeltaGuard alerts).
- Support for options volatility surface arbitrage (dedicated to future development cycles).

## Further Notes

- **Desktop Deployment**: The application is packaged as a standalone desktop binary utilizing Tauri v2 with WebKitGTK/Wayland support on Linux.
- **Local Vault Storage**: Sensitive API keys and private keys are encrypted locally using AES-256-GCM and stored only in memory while the vault is unlocked.
- **Fail-Closed Verification**: The terminal requires real credentials for all data streaming; entering paper trading mode executes against genuine live Level 2 orderbooks without risking real capital.
