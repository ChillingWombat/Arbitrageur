## Problem Statement

Traders and automated desk operators were experiencing multiple usability, visual hierarchy, and execution friction issues across both the desktop GUI and autonomous trading bots:

1. **Dual / Redundant Table Header Labels ("Sell and Short, Buy and Long at the Same Time")**:
   - In the Cross-Venue Dislocation Matrix, the table axes simultaneously displayed `LONG ↓` / `SHORT →` in the corner, `Sell @ $...` in each column header, and `Buy @ $...` in each row header. This dual vocabulary created unnecessary cognitive overhead for operators evaluating arbitrage pairs.
   - In options transaction history tables, a composite column previously rendered stacked `SELL /short` and `BUY /long` badges without cleanly distinguishing the option contract type (`CALL` vs. `PUT`) or the specific trading action (`BOUGHT` vs. `WROTE`).
2. **Cramped and Clipped Chart Geometry in Position Detail Modal and Signal Trend Gadget**:
   - In `PositionDetailModal`, the Net PnL sparkline was constrained to an overly shallow height of 140px (`H = 140`), resulting in squashed trajectories, tight zero-reference lines, and overlapping projection target labels.
   - In `SignalTrendGadget`, the left padding (`PAD_L = 46`) clipped wide basis-point deviations (such as `-1077.1 bps`) against the left SVG boundary.
3. **Apparent Negative Realized PnL vs. Strategy Health & Fee Friction**:
   - The desk status displayed a cumulative negative realized PnL (`-$273.59`). Operators needed transparency into why the net profit was negative, whether options accounting was accurately capturing upfront taker fees versus expiry payoffs, and whether autonomous bots were entering sub-hurdle trades that eroded capital.
   - Radar Bot was configured with an overly narrow hurdle (`minExpectedProfitBps = 0.5`) and tight stop bands (`stopMult = 1.5`, `staleUnwindSecs = 300`), which resulted in premature stop-outs and crossing fee attrition across high-frequency cycles.
4. **Concurrent Multi-Route Execution Collisions**:
   - Multiple routes on the same ticker (e.g. `ONDO alpaca→vanta` alongside `ONDO hyperliquid→vanta`) blocked each other's unwinds with residual signal errors due to ticker-level rather than pair-level exit scoping.

---

## Solution

1. **Clean Matrix Table Vocabulary & Segregated Options Columns**:
   - Updated the Cross-Venue Dislocation Matrix top-left corner header to explicitly show `BUY ↓` and `SELL →`, while column headers display `Bid $X.XX` and row headers display `Ask $Y.YY`. This eliminates redundant "sell/short" and "buy/long" duplication while preserving precise market quote semantics.
   - Segregated options tables across `PositionDetailModal` and `AtomicTransactionsTable` into distinct **Action** (`BOUGHT`, `WROTE` for options; `BUY`, `SELL` for non-options) and **Type** (`CALL`, `PUT`, `PERP`, `SPOT`) columns with dedicated color-coded chips.
2. **Expanded Chart Geometry & Safe Plot Padding**:
   - Expanded `PositionDetailModal` sparkline height from 140px to 200px (`viewBox="0 0 800 200"`), increasing the vertical plot area to 144px with balanced top/bottom padding (`PAD_TOP = 26`, `PAD_BOTTOM = 30`, `PAD_LEFT = 75`, `PAD_RIGHT = 45`) so the `$0` zero reference line, entry fee markers, and projected target labels never crowd or clip.
   - Expanded `SignalTrendGadget` left padding to `PAD_L = 52`, ensuring multi-digit basis-point labels render with generous clearance.
3. **Forensic PnL Transparency & Radar Parameter Calibration**:
   - Audited the ledger to verify that options strategies accurately account for $208+ in upfront Deribit entry taker fees (`entry_realized = -fee`), while holding +$1,500+ in expected terminal value that matures at settlement/expiry.
   - Calibrated Radar Bot parameters: increased `minExpectedProfitBps` to 2.5 bps (guaranteeing positive net edge after all fees and crossing slippage), raised `minPWin` to 0.55, widened `stopMult` to 2.5, and extended `staleUnwindSecs` to 600s (10 minutes) to eliminate premature stop-outs.
4. **Pair-Scoped Exit Dispatch & Spot Reduce-Only Support**:
   - Refactored `BotDispatch::exit` and `VenueDispatch::exit` to accept `PairKey` (`symbol`, `buy_venue`, `sell_venue`), isolating signal unwinds to their exact route.
   - Added spot crypto universe routing and enabled spot `Side::Sell` reduce-only orders for Alpaca.

---

## User Stories

1. As a desk operator, I want the Cross-Venue Dislocation Matrix axes to clearly state `BUY ↓` and `SELL →` with clean `Bid` and `Ask` quotes, so that I do not see confusing duplicate "sell and short, buy and long" labels in the same table.
2. As an options arbitrageur, I want options transaction tables to have distinct Action (`BOUGHT` vs. `WROTE`) and Type (`CALL` vs. `PUT`) columns, so that I can immediately identify whether a leg is a long call, short call, long put, or short put.
3. As a portfolio manager, I want the Net PnL sparkline in the Position Details modal to have a 200px height with comfortable vertical padding, so that realized history and target projections are legible and do not collide with the frame.
4. As an algorithmic trader, I want the Signal Trend Gadget to allocate sufficient left padding for y-axis labels, so that large basis-point deviations like `-1077.1 bps` are never truncated.
5. As an operator reviewing desk equity, I want clear transparency into why realized PnL reflects upfront options entry fees while expected terminal edge resolves at expiry, so that I can accurately assess desk performance.
6. As a risk manager, I want the Radar Bot's expected profit hurdle to be set at 2.5 bps net of all fees, so that autonomous trades only execute when edge comfortably exceeds round-trip friction.
7. As a quant trader, I want the Radar Bot's stop multiplier and stale timeout to be calibrated to 2.5× and 600s, so that temporary spread fluctuations do not trigger premature stop-outs.
8. As a multi-venue trader, I want concurrent arbitrage cycles on the same symbol across different venue pairs to unwind independently, so that one route never blocks another route on the same asset.
9. As a spot arbitrageur, I want Alpaca spot sell orders to be accepted as reduce-only unwinds, so that spot position flattening succeeds without rejection.
10. As a desk operator, I want to click and sort open positions in the sidebar by current PnL, name, and size, so that I can quickly monitor my highest-exposure and top-performing trades.
11. As an operator using desktop automation, I want to interact with modals and close them cleanly with the ESC key or close button, so that my workflow remains unblocked.
12. As a systems auditor, I want all configuration changes made via dev RPC to persist in SQLite, so that parameter tunings survive application and service restarts.

---

## Implementation Decisions

- **Matrix Table Header Standardization**:
  - Replaced composite headers in `LiveMatrix.tsx` with unified `BUY ↓` (rows) and `SELL →` (columns) labels.
  - Formatted column headers to display `Bid {formatCurrencyPrice(sellPrice)}` and row headers to display `Ask {formatCurrencyPrice(buyPrice)}`.
- **Options Table Column Segregation**:
  - Maintained `parseInstrumentType` helper to classify contracts into `CALL`, `PUT`, `PERP`, and `SPOT`.
  - In `PositionDetailModal.tsx` and `AtomicTransactionsTable.tsx`, mapped `isLong` / `isBuy` to `BOUGHT` and `WROTE` for options, while preserving `LONG` / `SHORT` or `BUY` / `SELL` for perps and spot.
- **Sparkline Geometry**:
  - Configured `PositionDetailModal` SVG chart to `W = 800, H = 200` with `PAD_LEFT = 75, PAD_RIGHT = 45, PAD_TOP = 26, PAD_BOTTOM = 30`.
  - Increased `PAD_L` in `SignalTrendGadget.tsx` from 46 to 52.
- **Autonomous Bot Parameter Calibration**:
  - Updated `radar_bot_config` in SQLite and live executor via dev RPC:
    ```json
    {
      "enabled": true,
      "mode": "auto",
      "pollIntervalSecs": 3,
      "minPWin": 0.55,
      "minExpectedProfitBps": 2.5,
      "maxConcurrentPositions": 4,
      "maxNotionalPerPositionUsd": 50.0,
      "maxNotionalPerVenueUsd": 300.0,
      "maxNotionalPerSymbolUsd": 100.0,
      "pairCooldownSecs": 30,
      "holdTimeoutMult": 3.0,
      "stopMult": 2.5,
      "staleUnwindSecs": 600
    }
    ```
- **PairKey Route Scoping**:
  - Scoped `VenueDispatch::exit` and `BotDispatch::exit` to `PairKey { symbol, buy_venue, sell_venue }`, routing position unwinds to `flatten_signal_positions_impl` by specific signal id rather than blanket symbol cancellations.

---

## Testing Decisions

### What Makes a Good Test
- Tests must assert external behavior, rendered DOM contracts, and public dev RPC outputs rather than private internal implementation details.
- Modal tests must verify segregated table columns, distinct Action/Type badges, and non-clipping SVG viewBox geometries.
- Matrix tests must verify that table headers render `BUY ↓`, `SELL →`, `Bid`, and `Ask` without composite `/short` or `/long` substrings.

### Testing Seams
1. **Frontend Component & Interactive DOM Seam (`frontend/src/components/*`)**:
   - Tested using `node --test` via `npm test` and `scripts/test-components.mjs`.
   - Verified across `PositionDetailModal.test.ts`, `ChallengerR3R4R5.test.ts`, `OptionsArbitrage.test.ts`, and `SignalTrendGadget.test.ts`.
2. **Backend Engine & Dev RPC Seam (`frontend/src-tauri/src/*`)**:
   - Tested using `cargo test -p arbitrageur-desktop`.
   - Verified all 600 unit and integration tests covering signal persistence, state store netting, and dev RPC command handlers.
3. **Live Desktop GUI Automation Seam (`open-computer-use`)**:
   - Controlled the running binary on desktop PID 517774, opened modals, verified SVG sparklines and column headers, tested sorting by PnL, and navigated across strategy tabs.

### Prior Art
- `PositionDetailModal.test.ts`: Asserts segregated Action and Type headers and `viewBox="0 0 800 200"`.
- `OptionsArbitrage.test.ts`: Asserts options package legs, parity card details, and substrategy filtering.
- `ChallengerR3R4R5.test.ts`: Asserts locked dock heights and clean matrix table layouts.

---

## Out of Scope

- Forcing premature settlement or artificial realization of options terminal values before contract expiry.
- Rewriting third-party exchange orderbook endpoints that do not offer public testnets.
- Disabling safety gates or circuit breakers to simulate paper fills.

---

## Further Notes

- Verified across 600/600 Rust backend tests (`cargo test -p arbitrageur-desktop`), 17/17 frontend test suites (`npm test`), 0 linter warnings (`cargo clippy -- -D warnings`), clean rustfmt (`cargo fmt --check`), and release compilation (`npm run build && cargo build --release --features custom-protocol`).
- Tested live on testnet under active system service `arbitrageur-testnet.service` with unlocked vault (`<redacted>`).
- Committed as `e54ec10` and pushed to `origin/main`.
