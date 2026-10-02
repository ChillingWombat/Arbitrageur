# UI Standardization, Strategy Monitor Alignment, and Component Ergonomics Overhaul

## Problem Statement

Traders and operators navigating the Arbitrageur desktop interface across multiple active strategies (Spread Radar, Spatial Arbitrage, Funding Rate, Prediction Markets, and Options) encountered visual discrepancies, noisy scaffold warnings, and inconsistent gadget ergonomics:

1. **Prediction Market Ribbon Clutter & Dead Scaffolds**: The Prediction Market parity tab displayed an alarming execution failure warning ("Prediction venues are scaffolds...") even when running in passive monitoring mode. Additionally, the ribbon exposed an inert "Max Slip" tag and a scaffold placeholder ("no venue volume data") for minimum event volume, while the search input contained intrusive example text ("Search contract (e.g. FED, BTC)...").
2. **Strategy Monitor Layout Inconsistency**: The Opportunity Scanner in Funding Rate featured a clean, professional header bar with the title on the left and filter controls aligned on the right. In contrast, Spread Radar and Spatial Spot Arbitrage placed filters awkwardly on the left without a clear section header, leaving dead space on the right and creating cognitive discordance across cockpit views.
3. **Asymmetric Button Set Padding**: Outer container borders enclosing strategy lifecycle button sets (`Start`, `Pause`, `Reduce Only`, `Stop`) suffered from unequal padding (`px-2 py-1`), causing the horizontal gap between buttons and the border to appear twice as large as the vertical gap.
4. **Dislocation Matrix Visual Noise**: Dislocation Matrix cells rendered directional trend arrows (`↗` / `↘`) alongside delta badges (`Δ +X.X`), adding unnecessary visual clutter in a dense, fast-moving financial matrix where colour coding and mathematical signs already convey directionality.
5. **Signal Trend Gadget Footer Redundancy**: The Signal Trend telemetry gadget featured a verbose footer line reciting venue sample counts and baseline weighting descriptions, distracting operators from the core quantitative distribution analytics.
6. **Unbounded Opportunity Scanner Sizing**: The Funding Rate Opportunity Scanner gadget expanded to an excessive fixed vertical height (`640px`) with variable card heights, creating large empty voids when few opportunities were present and causing inconsistent visual flow.
7. **Redundant Options Status Ribbon**: The "All Strategies" aggregate overview in the Options module rendered a strategy status ribbon with non-functional or misleading bot control buttons, even though bot operations are scoped strictly per-substrategy.
8. **Inconsistent Security & Settings Styling**: The Security Model gadget on the Security settings page stood out with bright green accent icons on a white card rather than the neutral grey container style used for legal disclaimers on the About page. Furthermore, verbose technical explanations regarding MFA secrets and specific vendor authenticator app examples cluttered the settings interface.
9. **Universal Quote Currency Noise**: Perp ticker listings in TickerExplorer appended `-USD` to instruments traded against universal USD collateral (e.g. `APT-USD`), cluttering the ticker list with redundant quote currency labels.

## Solution

Standardize all strategy cockpits, monitor gadgets, button sets, and security settings into a unified, clean, and distraction-free desktop interface:

- **Clean Prediction Market Ribbon & Active Parameter Display**: Remove scaffold warning banners and inert Max Slip tags from `PredictionHub.tsx`. Replace disabled volume text with an active `Min Vol` parameter badge formatting the configured USD volume hurdle, and simplify the filter input placeholder to `Search contract...`.
- **Standardized Opportunity Scanner Header Across Cockpits**: Unify the Opportunity Scanner header layout across Spread Radar (`OpportunityRadar.tsx`), Spatial Arbitrage (`SpotArbitrageBot.tsx`), and Funding Rate (`FundingRadar.tsx`). Every monitor gadget now features a bold `Opportunity Scanner` title on the left and right-aligned filter inputs (`Symbol`, `Venue`, `Min Exp`, `Min Win`, and hidden count).
- **Symmetric Button Set Padding**: Update `BotLifecycleControls.tsx` outer border padding to uniform `p-1` (4px on all sides), eliminating horizontal gap discrepancies across all strategy lifecycle button clusters.
- **Minimalist Dislocation Matrix & Telemetry Cleanups**: Strip `↗` and `↘` icon symbols from matrix cell delta badges in `LiveMatrix.tsx`. Remove the verbose venue sample count footer line from `SignalTrendGadget.tsx`, focusing attention on quantitative band statistics.
- **Bounded 2-Row Funding Rate Scanner**: Constrain the Funding Rate Opportunity Scanner container height to fit exactly 2 rows of opportunity cards with smooth vertical scroll for overflow, and fix individual card heights to `250px` for layout predictability.
- **Scoped Options Strategy Status Ribbon**: Suppress the strategy status ribbon on the "All Strategies" view in `OptionsArbitrage.tsx`, revealing it only when a specific substrategy (Parity, Box Spread, Jelly Roll, Convexity) is selected.
- **Neutralized Security & Settings Ergonomics**: Style the Security Model gadget with neutral grey styling (`bg-slate-50`, `text-slate-500` icon) identical to the legal disclaimer box on the About page. Remove the redundant MFA explanation text and strip vendor app examples from TOTP labels.
- **Streamlined Ticker Symbols**: Omit the `-USD` quote suffix for universal USD pairs in `TickerExplorer.tsx`, displaying concise base tickers (`APT`, `BNB`, `SOL`) while preserving non-USD quotes (`BTC-USDC`, `ETH-USDT`).

## User Stories

1. As an arbitrage trader, I want all strategy monitor gadgets (Spread Radar, Spatial, and Funding Rate) to share an identical header layout with the title on the left and filters on the right, so that navigating between strategies is intuitive and visually consistent.
2. As a trader scanning prediction market parity opportunities, I want the Prediction Market ribbon to display the configured minimum event volume hurdle, so that I know what volume filter is applied without relying on venue liquidity data.
3. As a prediction market operator, I want inert scaffold warning banners and unused slippage tags removed from the Prediction Market ribbon, so that my view is focused strictly on actionable parity spreads.
4. As an operator searching for prediction contracts, I want a clean `Search contract...` input placeholder without parenthetical examples, so that the input is legible and uncluttered.
5. As an arbitrage operator, I want the strategy lifecycle control buttons (`Start`, `Pause`, `Reduce Only`, `Stop`) to sit symmetrically within their outer border container with equal padding on all sides, so that the button cluster looks visually balanced.
6. As a quantitative trader monitoring cross-venue dislocations in the Live Matrix, I want cell delta badges to display numerical values without arrow icons, so that the matrix presents high-density data without visual noise.
7. As an operator analyzing venue deviation trends, I want the Signal Trend gadget to omit the venue sample count footer line, so that the gadget remains compact and focused on quantitative spread distribution metrics.
8. As a funding rate trader, I want the Opportunity Scanner gadget in the Funding Rate tab to occupy a compact vertical space sized for exactly two rows of cards, so that the page remains balanced and leaves room for the bottom execution dock.
9. As a funding rate trader, I want opportunity cards in the Funding Rate scanner to have a fixed height, so that the card grid maintains a neat, uniform structure regardless of contract title length.
10. As an options trader viewing the "All Strategies" aggregate overview, I want the top strategy status ribbon hidden until I select an individual substrategy, so that I am not shown inactive or misleading bot lifecycle controls on a multi-strategy view.
11. As an options trader selecting a specific option substrategy (e.g., Put-Call Parity or Box Spread), I want the strategy status ribbon to appear with parameters and lifecycle controls specific to that substrategy, so that I can configure and run that strategy independently.
12. As a security-conscious user visiting the Security settings page, I want the Security Model explanatory box to match the muted grey appearance of the About page legal disclaimer, so that the application settings feel cohesive and professional.
13. As a user configuring Two-Factor Authentication, I want concise TOTP labels without vendor app parentheticals (`Google Authenticator, 1Password, Authy`), so that the interface remains modern and vendor-agnostic.
14. As an operator managing credentials, I want redundant explanatory paragraphs regarding MFA unlock mechanics removed, so that the security settings remain streamlined.
15. As a trader scanning perpetual swap tickers in TickerExplorer, I want standard USD instruments to display as clean base symbols (`APT`, `COIN`, `NVDA`) without an unnecessary `-USD` suffix, so that the ticker list is easier to scan.
16. As a trader analyzing multi-currency markets, I want non-USD quote suffixes (`-USDC`, `-USDT`) to remain clearly visible in TickerExplorer, so that distinct stablecoin instruments are easily distinguishable.

## Implementation Decisions

- **Prediction Market Ribbon & Input Cleanups**:
  - In `PredictionHub.tsx`, removed `{!executeReady && ...}` warning span.
  - Removed `Max Slip` ribbon pill.
  - Simplified filter input placeholder from `Search contract (e.g. FED, BTC)...` to `Search contract...`.
  - Replaced disabled scaffold volume text with an active `Min Vol` parameter badge formatting the configured USD hurdle (`params?.autoMinEventVolumeUsd ?? params?.autoMinVolumeUsd ?? 500_000`).
  - Added optional `params` prop to `PredictionHubProps` and connected `strategyParams.get("prediction")` in `App.tsx`.
- **Strategy Monitor Header Standardization**:
  - In `OpportunityRadar.tsx`, removed `Execution Single-Way` badge from the strategy ribbon.
  - Added `<h3 className="text-[13px] font-bold font-sans tracking-wider text-slate-900">Opportunity Scanner</h3>` to the table gadget header and grouped search, venue filters, min expected profit, and min win probability on the right.
  - In `SpotArbitrageBot.tsx`, unified the table gadget header with the identical left-aligned `Opportunity Scanner` title and right-aligned filter controls.
- **Button Set Border Symmetry**:
  - In `BotLifecycleControls.tsx`, adjusted outer wrapper styling from `px-2 py-1` to `p-1`, enforcing 4px symmetric padding between the border and child buttons across all strategy control sets.
- **Matrix & Telemetry Cleanups**:
  - In `LiveMatrix.tsx`, removed `TrendingUp` and `TrendingDown` icon renderings from the delta badge.
  - In `SignalTrendGadget.tsx`, removed the honesty indicator footer line (`{series.sample_count.toLocaleString()} venue samples in window...`).
- **Funding Rate Scanner Dimensions**:
  - In `FundingRadar.tsx`, sized the Opportunity Scanner container to `h-[600px]` with vertical overflow scrolling, perfectly fitting 2 rows of cards.
  - Constrained individual opportunity cards to `h-[250px] overflow-hidden` for strict dimensional uniformity.
- **Options All Strategies Ribbon Suppression**:
  - In `OptionsArbitrage.tsx`, wrapped the strategy status ribbon in `{selected !== "all" && (...)}`, ensuring it renders only when a concrete substrategy is active.
- **Security & Settings Refinements**:
  - In `EngineSettings.tsx`, changed the Security Model container from `bg-surface` to `bg-slate-50` and replaced the emerald `ShieldCheck` icon with `text-slate-500`.
  - Removed the MFA unlock explanation paragraph and removed `(Google Authenticator, 1Password, Authy)` from the TOTP label.
- **TickerExplorer Quote Currency Suffix**:
  - In `TickerExplorer.tsx`, conditionally rendered the quote suffix: `{quote !== "USD" && <span className="text-muted font-semibold">-{quote}</span>}`.

## Testing Decisions

- **Testing Principles**: Tests must verify observable rendered DOM output, layout classes, and user-facing text strings rather than internal component state.
- **Frontend Component Seam (`test-components.mjs` / Node.js test runner)**:
  - `PredictionHub`: Assert `Min Vol` renders configured numeric values in USD, assert `Max Slip` and scaffold warning text are absent, assert input placeholder equals `Search contract...`.
  - `OpportunityRadar`: Assert `Opportunity Scanner` header renders, assert filter controls appear in the header bar, assert `Execution Single-Way` tag does not render.
  - `SpotArbitrageBot`: Assert `Opportunity Scanner` header renders with right-aligned asset/search filters.
  - `BotLifecycleControls`: Assert container carries `p-1` class.
  - `LiveMatrix`: Assert delta badges render `Δ +X.X` without SVG trend icons.
  - `SignalTrendGadget`: Assert sample count footer line is absent while band analytics remain.
  - `FundingRadar`: Assert scanner container has 2-row bounded height and opportunity cards have fixed height.
  - `OptionsArbitrage`: Assert strategy status ribbon is omitted when `selected === "all"` and present when a substrategy is selected.
  - `EngineSettings`: Assert Security Model box has `bg-slate-50` and slate icon; assert MFA explanatory text and vendor examples are absent.
  - `TickerExplorer`: Assert `-USD` quote suffix does not render for USD instruments while `-USDC` renders for non-USD quotes.
- **Prior Art**:
  - Existing component test suites in `frontend/src/components/*.test.ts` (e.g. `ChallengerR3R4R5.test.ts`, `OptionsArbitrage.test.ts`, `EngineSettings.test.ts`).

## Out of Scope

- Modifying underlying backend strategy math, calculation models, or order execution routing.
- Adding new strategies or altering exchange WebSocket connection logic.
- Rewriting the settings vault encryption or TOTP generation backend.

## Further Notes

- All changes are strictly UI/UX standardizations and visual ergonomics refinements.
- Full verification passed with 400/400 checks across all 52 test suites in `frontend/` and 0 TypeScript compilation errors.
