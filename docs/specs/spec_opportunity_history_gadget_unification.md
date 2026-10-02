# Spec: Unified Opportunity History Gadget & Lifecycle Switch Controls

## Problem Statement

Traders and risk operators monitoring arbitrage opportunities across Spread Radar and Funding Rate encountered fragmented, inconsistent, and misleading opportunity history tools:

1. **Inconsistent Implementations Across Strategies**: Spread Radar and Funding Rate featured completely different gadget implementations with divergent layouts, schemas, and capabilities, forcing operators to adapt to different user interfaces between strategy tabs.
2. **Misleading "This Ticker" Filter**: Both Spread Radar and Funding Rate monitors operate across the entire active ticker universe. The presence of an "All Tickers" vs "This Ticker" toggle was redundant and misleading, as there is no single isolated ticker context in a universe-wide monitor.
3. **Lack of Lifecycle Opportunity Segregation**: Operators had no intuitive way to switch between executed trades and missed/untraded opportunities. Bot skips (e.g. hurdle rejections, min notional bounds, friction bands) and execution blocks (e.g. gateway rejections, margin limits) were either buried in verbose raw execution logs or completely omitted.
4. **Misleading Telemetry & PnL Color Coding**: Traded opportunities with negative realized PnL were unconditionally styled in green (`text-profit`), disguising losing trades as profitable.
5. **Fragile Data Ingestion**: When retrieving traded opportunities and monitor signal events via `Promise.all`, a temporary network error or endpoint failure from one service caused the entire gadget to crash, preventing traders from viewing even available historical data.
6. **Omission of Blocked Radar Executions**: Dispatched futures radar signals that failed execution (e.g. margin exceeded or gateway timeout) were silently dropped from the opportunity history table.

## Solution

Unify the opportunity history interface across both Spread Radar and Funding Rate into a single, high-performance, standardized `OpportunityHistoryGadget`:

- **Complete Elimination of "This Ticker"**: Remove the confusing "All Tickers" vs "This Ticker" toggle entirely, reflecting that monitors oversee the entire admitted market universe.
- **Segmented Switch Buttons for Opportunity Lifecycle**: Anchor a prominent three-state switch toolbar (`All`, `Traded`, `Not Traded`) with live count badges:
  - **All**: Unified chronological ledger of all detections, tagged with explicit `TRADED` vs `UNTRADED` status badges.
  - **Traded**: Dedicated view of executed and closed positions displaying entry/exit legs, realized PnL, duration, and fill counts.
  - **Not Traded**: Dedicated view surfacing bot skip reasons (insufficient edge, probability decay), gate rejections, and execution failures with actionable context.
- **Tone-Aware PnL Formatting**: Apply truthful PnL coloring where positive returns render in profit green, negative returns in loss red, and flat/unclosed returns in neutral tones.
- **Resilient Data Loading**: Transition to `Promise.allSettled` to gracefully handle partial endpoint degradation with informative status banners.
- **Ingestion of Blocked Signals**: Ingest radar and funding execution logs with blocked or failed statuses directly into the "Not Traded" view with failure diagnostic notes.
- **Dual-Track Drill-Down Modals**:
  - Clicking a traded row opens the atomic transaction trail modal displaying order legs, fills, and fees.
  - Clicking an untraded row opens the untraded audit modal displaying hurdle analysis, friction bands, and model economics beliefs.

## User Stories

1. As an arbitrage trader, I want a single unified Opportunity History gadget across Spread Radar and Funding Rate, so that my workflow and muscle memory remain consistent regardless of which strategy I am monitoring.
2. As an operator monitoring the full ticker universe, I want the confusing "All Tickers" vs "This Ticker" toggle removed, so that the gadget honestly reflects that the monitor covers all admitted symbols.
3. As a trader evaluating execution performance, I want a "Traded" switch button with an active counter pill, so that I can immediately inspect only executed trades with realized PnL and trade duration.
4. As a quantitative analyst diagnosing bot decisions, I want a "Not Traded" switch button with an active counter pill, so that I can inspect all detected opportunities that the bot skipped or were blocked at execution.
5. As an operator wanting a comprehensive chronological record, I want an "All" switch button showing both traded and untraded signals with explicit `TRADED` and `UNTRADED` badges, so that I can review total signal flow.
6. As a trader analyzing trade outcomes, I want losing trades with negative realized PnL to be rendered in red (`text-loss`), so that I am never misled by losing trades styled in green.
7. As an operator filtering a high-volume opportunity table, I want a text search filter matching symbol, venues, status, or skip reason, so that I can quickly pinpoint specific events.
8. As a multi-exchange trader, I want a venue filter dropdown, so that I can inspect opportunities involving specific trading venues.
9. As an operator during volatile markets, I want an "Actionable only" toggle to filter out routine background heartbeats in Spread Radar and sub-hurdle yields in Funding Rate, so that the table highlights high-conviction events.
10. As a trader reviewing a failed order, I want Spread Radar execution blocks (e.g., gateway timeout or margin rejection) to appear in the "Not Traded" list with a `BLOCKED` badge, so that execution issues are immediately apparent.
11. As an operator investigating an untraded signal, I want to click the row to open an Untraded Opportunity modal displaying skip reasons, route direction, and economics JSON, so that I can understand why the algorithm rejected the trade.
12. As an operator investigating an executed trade, I want to click the row to open the Traded Opportunity modal showing atomic legs and transaction fills, so that I can verify execution accuracy.
13. As a developer maintaining backward compatibility, I want legacy references to the history gadget to delegate cleanly to the new component without regression, so that existing code remains stable.
14. As an operator experiencing intermittent network connectivity, I want the gadget to load partially available data via `Promise.allSettled`, so that a failure in one query does not break the entire view.

## Implementation Decisions

- **Unified Component Architecture**:
  - Replace the separate implementations with a single configurable `OpportunityHistoryGadget` component supporting strategy modes (`futures`, `radar`, `funding`, and `all`).
  - Normalize strategy identifiers (`radar` normalized to `futures`).
- **Elimination of Per-Ticker Scoping**:
  - Remove all buttons, toggles, and state variables for switching between "All Tickers" and "This Ticker".
  - Scope queries to the entire universe by default, with optional symbol prop filtering when embedded in a single-ticker explorer context.
- **Segmented Switch Controls**:
  - Implement a three-way switch (`all` | `traded` | `not_traded`) with badge count pills showing real-time item counts matching the active search and venue filters.
  - Use standard Tailwind utility classes (`py-px` vertical padding) for clean badge pill rendering.
- **PnL Tone-Aware Formatting**:
  - Implement a `pnlTone` evaluation helper mapping positive values (> +0.005) to `text-profit`, negative values (< -0.005) to `text-loss`, and zero/flat/open states to `text-slate-600`.
- **Fault-Tolerant Asynchronous Ingestion**:
  - Query trade ledger entries and monitor signal events using `Promise.allSettled`, handling partial rejections by displaying available rows alongside an amber warning banner.
- **Comprehensive Failure Log Integration**:
  - Ingest execution failure logs from both `radarLogs` (Spread Radar) and `executionLogs` (Funding Rate) when status indicates failure or notes mention blocked execution, rendering them as `BLOCKED` items in the untraded list.
- **Sticky Gadget Toolbar**:
  - Position the filter and switch button toolbar with `sticky top-0 bg-surface z-20` so controls remain accessible during table scrolling.
- **Backward-Compatible Facade**:
  - Retain `TickerOpportunityHistory` as a thin wrapper forwarding props to `OpportunityHistoryGadget`.

## Testing Decisions

- **Testing Principles**: Tests must verify observable user-facing behavior and external DOM output (rendered switch buttons, active badge counts, presence of `TRADED` / `UNTRADED` labels, correct PnL text colors, modal content) rather than internal React state.
- **Primary Testing Seam**:
  - Single high-level test seam at the frontend component layer using the Node.js test runner (`scripts/test-components.mjs`) bundled with esbuild.
  - Component test suite in `OpportunityHistoryGadget.test.ts` covering:
    - Switch buttons for `All`, `Traded`, and `Not Traded` render with accurate count badges.
    - Verification that no button with text "All tickers" or "This Ticker" exists anywhere in the rendered markup.
    - Honest PnL coloring: verifying that negative PnL applies `text-loss` and never `text-profit`.
    - Spread Radar execution failures from `radarLogs` correctly populate the "Not Traded" list.
    - Modals for traded transactions and untraded skips render appropriate diagnostic data.
    - Backward-compatible wrapper renders cleanly without errors.
- **Secondary Verification Seams**:
  - Frontend type safety and production bundling: `npm run build` (`tsc && vite build`).
  - Desktop backend test suite: `cargo test -p arbitrageur-desktop`.
  - Backend linting and formatting: `cargo clippy -p arbitrageur-desktop` and `cargo fmt --check`.
- **Prior Art**:
  - `TradedOpportunitiesGadget.test.ts`, `FundingRadar.test.ts`, `OpportunityRadar.test.ts`.

## Out of Scope

- Modifying the underlying SQLite database schema (`signal_executions`, `monitor_signal_events`).
- Altering the engine's core trading algorithms, mathematical hurdle functions, or execution dispatch pipelines.
- Changing WebSocket streaming protocols or exchange adapter transports.

## Further Notes

- All changes have been implemented, tested, and verified on Linux.
- Frontend test suite passed 100% (15/15 tests in `OpportunityHistoryGadget.test.ts` plus all existing component suites).
- Production build succeeded with zero TypeScript errors.
- Backend desktop test suite passed 100% (610 passed, 0 failed).
