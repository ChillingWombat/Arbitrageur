# Strategy Cockpit, Telemetry Gadgets, and Execution Ergonomics Overhaul

## Problem Statement

When operating high-frequency and multi-leg arbitrage bots across decentralized and centralized venues, traders face three primary operational frictions:
1. **Inefficient Strategy Configuration & Contract Filtering**: Options substrategies (Put-Call Parity, Box Spread, Jelly Roll, Convexity) previously lacked dedicated configuration controls and expiry horizon filters comparable to perp/spot strategies, forcing operators to execute without safety guardrails for contract expiration windows, sizing bounds, or account routing.
2. **Telemetry Blindspots & Visual Imbalance**: Operators monitoring complex Spread Radar and Funding Rate strategies had minimal top-level telemetry cards, obscuring key real-time performance indicators such as Net APR, Net Profit, active cycle count, and fee drag. Furthermore, the bottom gadget window left excessive trailing white space on the right, and presented bottom controls as cramped button strips rather than unified full-width navigation tabs.
3. **Execution Detail Noise & Venue Price Collar Rejections**: Detail dialogs and atomic transaction tables exposed redundant leg counters and raw internal wire order hashes to users, increasing cognitive load during live manual interventions. In multi-leg options execution, venue-imposed price collars (such as Deribit error 10007) caused atomic IOC packages to fail on un-clamped edge legs, creating unhedged leg risk.

## Solution

Deliver a comprehensive ergonomics, telemetry, and execution safety overhaul:
- **Dedicated Options Configurator Modal**: Provide an intuitive, multi-section configuration panel for all live option substrategies, featuring real-time parameter tuning (base sizing, min edge hurdle), contract filters (min hours / max days to expiry), execution cadence/capacity limits (poll intervals, max concurrent positions), and account routing overview.
- **Floating Substrategy Selector & Bordered Scanner Enclosure**: Decouple the options substrategy selector into a floating panel matching standard ticker navigation, clean up redundant subheaders, and cleanly enclose the opportunity scanner in an alert-ready bordered zone.
- **Top Metric Telemetry Gadget Strips**: Expand top-of-page telemetry strips across strategies (6 high-signal metrics for Spread Radar, 5 for Funding Rate) to provide immediate visibility into profit, APR, exposure, and daily yield.
- **Full-Width Bottom Dock Navigation Tabs**: Convert the bottom gadget switcher into a robust full-width tab navigation bar across all strategies (Spread Radar, Funding Rate, Spatial Spot, Options), distributing tabs equally (25% each) across 100% of the gadget container width without dead space on the right.
- **Streamlined Ledger & Transaction Ergonomics**: Eliminate redundant instrument/leg metrics in position detail views, remove internal audit hashes from user view, place instrument type badges inline with symbols, and reposition Opportunity History logically to the left of Execution History.
- **Automatic Deribit IOC Price Collar Clamping**: Automatically parse price collar limits from exchange rejection responses and clamp limit prices to valid collar boundaries on retry, safeguarding atomic package fills.

## User Stories

1. As an arbitrage bot operator, I want all bottom gadget tabs (Open Positions, Opportunity History, Execution History, Log Monitor) to share the entire width of the gadget container, so that there is no wasted white space on the right and the navigation feels balanced and accessible.
2. As an options trader, I want a dedicated Options Configurator modal accessible from the strategy ribbon, so that I can configure bot parameters, contract filters, cadence, and account routing for each option substrategy independently.
3. As an options trader, I want to set minimum hours to expiry and maximum days to expiry for option substrategies, so that the bot never enters illiquid far-dated contracts or contracts too close to expiration with extreme gamma risk.
4. As an options trader, I want to set the entry quantity in base asset units (e.g., BTC or ETH) and the minimum net edge in basis points, so that executions only trigger when economic profit exceeds transaction costs.
5. As an options trader, I want to configure the scanner polling interval and maximum concurrent open positions, so that I can manage API rate limits and avoid over-allocating capital.
6. As an options trader, I want the options substrategy selector to be a floating panel decoupled from window borders, so that it matches the clean visual aesthetic of the ticker selector panel.
7. As an options trader, I want the Opportunity Scanner area to be enclosed in a clear bordered zone with an empty state and status badge, so that I immediately know when the scanner is armed and ready.
8. As a spread radar operator, I want an expanded 6-metric telemetry strip at the top of the Spread Radar page, so that I can track Total Net Profit, Win Rate %, Completed Cycles, Open Cycles & Exposure, Fees Paid, and Active Signals at a glance.
9. As a funding rate trader, I want a 5-metric telemetry strip at the top of the Funding Rate page, so that I can monitor Total Net Profit, Top Net APR, Average Net APR, Open Positions & Exposure, and Estimated 24-Hour Yield in real time.
10. As a trader inspecting an open position detail modal, I want to see a single unified leg breakdown rather than duplicated instrument and leg counts, so that the dialog remains clean and legible.
11. As a trader reviewing past trades in the Traded Opportunity detail dialog, I want atomic transaction rows to omit raw audit telemetry hashes (such as internal wire order identifiers), so that I can focus on fill prices, slippage, and fee breakdowns without clutter.
12. As a trader analyzing transaction legs, I want instrument type badges (CALL, PUT, PERP, SPOT) displayed cleanly inline with the contract symbol, so that leg types are instantly recognizable.
13. As an arbitrage trader, I want Opportunity History positioned directly before Execution History in the tab bar, so that my analytical workflow flows logically from opportunity detection to trade execution.
14. As an algorithmic trader executing multi-leg options packages on Deribit, I want immediate-or-cancel (IOC) orders that violate price collars to be automatically clamped to the venue's permitted price boundary, so that multi-leg atomic executions do not fail due to transient collar boundary movements.
15. As a mobile or desktop user on varying window widths, I want tab labels to truncate gracefully with shrink-resistant icons, so that the interface never breaks on narrow displays.
16. As a security-conscious operator, I want all strategy configuration settings to persist securely in encrypted local storage and apply immediately to live engine bots without requiring a restart.

## Implementation Decisions

- **Full-Width Grid Tab Architecture**: Transformed dock navigation containers from inline flex button clusters into a strict 4-column CSS grid (`grid-cols-4`) with uniform internal padding and gutters (`pt-1 px-1 gap-1`). Each tab button is styled as a full-width centered card element (`w-full flex items-center justify-center space-x-2 py-2 px-3`) with rounded top borders and negative bottom margin (`-mb-px`) that seamlessly blends active tabs into the card container below.
- **Component Symmetry Across Strategy Views**: Applied the standardized 4-tab full-width architecture identically across all four strategy cockpit modules (Spread Radar, Funding Rate, Spatial Spot, and Options Arbitrage), guaranteeing visual and functional consistency across the entire product.
- **Tab Sequence Normalization**: Standardized tab ordering across all strategy views to: (1) Open Positions, (2) Opportunity History, (3) Execution History, (4) Log Monitor.
- **Dedicated Options Configurator (`OptionsConfigModal`)**:
  - Implemented a 2-column modal layout with a left-hand navigation rail (Sections: Bot Parameters, Contract Filters, Cadence & Limits, Account Routing) and right-hand settings form.
  - Provided substrategy switching tabs in the modal header across all four supported live substrategies: Put-Call Parity, Box Spread, Jelly Roll, and Convexity Boundary.
  - Bound form controls to the unified Options Bot Configuration DTO, supporting `qtyBase`, `minEdgeBps`, `minHoursToExpiry`, `maxDaysToExpiry`, `pollIntervalSecs`, and `maxOpenPositions`.
- **Top Telemetry Metrics Expansion**:
  - Spread Radar: 6 responsive telemetry cards aggregating live engine signals, completed cycle count, win rate percentage, net profit, total fees, and active cycle exposure.
  - Funding Rate: 5 responsive telemetry cards calculating total realized funding profit, top available positive net APR, average pool APR, open funding exposure, and estimated 24h carry yield.
- **Cleaned Position Detail & Ledger Models**:
  - Restructured position detail metrics into symmetrical 3x2 metric grids, eliminating duplicate leg count representations.
  - Refined atomic transaction tables to display role badges, side indicators, filled quantities, and execution prices while hiding verbose backend audit telemetry strings.
- **Venue Adapter Price Collar Resilience**:
  - Extended the Deribit exchange adapter's order submission pipeline to intercept rejection code 10007 (`price_too_high` or `price_too_low`).
  - Implemented automatic boundary extraction and price clamping logic to adjust IOC limit orders to the exchange-mandated collar threshold, preventing package execution failures.

## Testing Decisions

- **Testing Principles**: Only test external observable behavior, rendered UI structures, and public contract APIs; avoid coupling tests to transient internal state.
- **Frontend Component Seams**:
  - Verified with Node.js test runner and Happy-DOM (`test-components.mjs`):
    - Bottom dock container renders a 4-column grid spanning 100% width with exact classes.
    - Tab switching activates the proper views (Open Positions, Opportunity History, Execution History, Log Monitor) and verifies that inner containers maintain fixed scroll dimensions without duplicate scrollbars.
    - Options Configurator modal opens, renders all four substrategy tabs, renders all four navigation sections, and updates state without TypeScript errors.
    - Top telemetry metric cards render expected values and correct tone classes for profit/loss states.
- **Live System Desktop & Dev RPC Seam**:
  - Validated live GUI rendering using the operating system accessibility tree and coordinate frame inspection, confirming tabs occupy equal width (344px each) across the 1693px gadget window.
  - Validated live engine status and credential vault unlocking via dev RPC socket (`/tmp/arb-dev.sock`).
- **Backend Venue Adapter Wire Seam**:
  - Unit-tested Deribit price collar regex parser against actual exchange error responses for both buy (`price_too_high`) and sell (`price_too_low`) IOC orders.

## Out of Scope

- Implementing new machine-learning predictive strategies (focus is strictly on telemetry, ergonomics, and live execution safety of existing strategies).
- Modifying underlying venue WebSocket wire protocols or schema migrations beyond options bot configuration.
- Rewriting the charting canvas or depth book rendering engines.

## Further Notes

- The testnet release binary is built with `--features custom-protocol` to embed the optimized frontend assets directly into the Tauri user service (`arbitrageur-testnet.service`).
- All strategy views now adhere to the unified visual hierarchy defined in the product design guidelines.
