# Specification: Options Strategy Leg Nomenclature, Time-Proportional PnL Visualization, and Synthetic Financing PnL Recalibration

## Problem Statement

As a quantitative options arbitrage trader operating across centralized derivatives exchanges (Deribit) and multi-venue perpetual DEXs, I encountered three severe operational, visual, and financial accounting defects:

1. **Confusing and Redundant Leg Nomenclature**: Across options transaction history and exposure breakdown tables, orders and holdings displayed composite strings such as `BUY /long` and `SELL /short`. In the options market, this terminology is technically redundant and semantically imprecise. An operator cannot immediately distinguish whether a position represents an option write (short call/put generating upfront cash and short gamma) versus buying an option (long call/put paying premium) versus hedging via a linear perpetual. Furthermore, the absence of separate Action and Instrument Type columns forced operators to parse complex exchange ticker strings (`BTC-25DEC26-200000-C`) to deduce whether an option is a Call or a Put.

2. **Severe Net PnL Chart Geometric Distortion & Border Clipping**: In the position detail inspection modal, the "Net PnL since entry" chart used an unpartitioned linear time scale hardcoded with a 24-hour forward projection horizon (`t2 = now + 24h`). For recently opened positions (e.g. 5–15 minutes old), the historical realized trajectory (`t0` to `now`) was squashed into a 2-pixel sliver on the extreme left border (representing less than 0.5% of the chart width), while 99.5% of the chart canvas was occupied by an empty, speculative dashed projection line. Furthermore, because the current PnL data point was pinned directly to the left boundary with right-aligned text anchors, the current PnL label (e.g. `-$22.07`) extended off-canvas, suffering severe truncation and clipping on the left border.

3. **Massive Negative Net Profit Distortion via Misclassified Synthetic Bond Outlays**: The desk telemetry and bottom control bar displayed an alarming realized net loss of `-$26,325.41`, causing operators to question whether the algorithmic strategies were broken or hemorrhaging money. In reality, the desk had executed multi-leg options packages (Long Box Spreads and Jelly Rolls) which function as synthetic zero-coupon bonds. For a Long Box Spread, cash is disbursed upfront to purchase future guaranteed payout at expiration (e.g., paying $4,242.84 to receive a guaranteed $5,000.00 cashflow at expiry). Prior to the fix, the execution engine booked this upfront principal disbursement directly into `execution_log.net_profit` and accumulated it into the durable realized trading PnL counter (`pnl.total.{env}`) as an immediate 100% loss. This conflated capital financing investment with trading execution loss. True trading realized PnL was actually just `-$258.45` in exchange taker commissions and slight taker friction across 500+ automated trades.

---

## Solution

A comprehensive full-stack quantitative solution spanning frontend visualization architecture, tabular ergonomics, institutional options accounting, and durable state recalibration:

1. **Dedicated Action and Instrument Type Columns**:
   - Replaced redundant composite order strings (`BUY /long`, `SELL /short`) with clear, mutually exclusive terminology.
   - For options instruments, the action explicitly states **BOUGHT** (long premium, paid cash) or **WROTE** (short premium, received cash/short gamma). For linear perpetuals and spot, the action explicitly states **BUY** or **SELL** (or **LONG** / **SHORT** for exposures).
   - Introduced a dedicated **Type** column displaying high-visibility semantic badge pills: **CALL** (emerald badge), **PUT** (amber badge), and **PERP** (sky blue badge).

2. **Balanced Two-Zone Time-Proportional PnL Projection Geometry**:
   - Upgraded the position detail PnL chart to a balanced two-zone width partitioning model:
     - Realized History Zone: Allocates 45% of the available horizontal canvas exclusively to the elapsed lifespan of the position (`t0` to `now`), ensuring historical performance and entry fee transitions are rendered prominently over hundreds of pixels rather than squashed into an unreadable sliver.
     - Illustrative Projection Zone: Allocates 55% of the horizontal canvas to the forward projection (`now` to `target`), clearly demarcated by a subtle dashed divider line and tinted background rect.
     - Positions without forward expectations allocate 100% of the horizontal canvas to realized history.
   - Applied dynamic text anchoring and a 15% vertical safety margin (`padY`), anchoring the current PnL text label centrally over the current position marker at the partition boundary, completely eliminating edge clipping and canvas overflow.
   - Rendered explicit multi-point timeline anchors: Entry timestamp, "Now (Elapsed)", and "Target (+24h)".

3. **Institutional Accounting Separation & Historical PnL Recalibration**:
   - Architecturally segregated upfront options financing premiums from realized trading PnL: entry financing cashflows route exclusively to a dedicated financing outlay accumulator (`pnl.premium_outlay.{env}`), while only venue transaction fees (`-fee_total`) debit to the realized trading PnL counter (`pnl.total.{env}`).
   - Implemented an automated engine state recalibration migration (`migrated_v2`) that parses the execution log on boot, strips historical options package principal disbursements out of the realized PnL counter, and resets the headline Net Profit metric to the authentic realized PnL of closed trades and fees (`-$258.45`), cleanly preserving the full options financing balance (`-$56,413.15`) in the outlay register.

---

## User Stories

1. As an options arbitrageur, I want atomic transaction tables to display a dedicated Action column indicating whether an option was BOUGHT or WROTE, so that I can instantly verify whether an order bought premium or wrote short gamma.
2. As an options arbitrageur, I want atomic transaction tables to display a dedicated Type column with CALL, PUT, and PERP badges, so that I do not have to parse raw exchange ticker symbols to determine the contract type.
3. As an options trader, I want to eliminate redundant labels like `BUY /long` and `SELL /short`, so that tabular displays are clean, concise, and adhere to industry-standard options terminology.
4. As an operator inspecting an open position, I want the exposure breakdown table to display distinct Action and Type columns matching the transactions table, so that venue inventory matches trade execution logs.
5. As an operator opening the Position Detail Modal, I want the Net PnL chart to clearly display the realized price path from entry to the present, so that historical performance is visually legible regardless of how recently the position was opened.
6. As an operator viewing a position opened minutes ago, I want the realized history portion of the chart to occupy substantial visual width (45%), so that entry fee dips and mark movements are not squished against the left canvas edge.
7. As an operator, I want the current PnL numeric label to render cleanly with adequate padding on all sides, so that text is never truncated or clipped by modal boundaries.
8. As an operator, I want forward projection lines to be visually distinguished with a dashed stroke, background tint, and vertical divider line, so that speculative projections are never confused with realized marks.
9. As an operator inspecting a position without a model projection, I want the chart to allocate 100% of width to realized performance, so that no artificial forward lines are drawn.
10. As a quantitative risk manager, I want Long Box Spreads and Jelly Rolls to be accounted for as synthetic zero-coupon bonds, so that upfront capital disbursements are treated as financing assets rather than trading losses.
11. As a quantitative risk manager, I want only venue transaction fees to debit to realized PnL upon options package entry, so that trading performance metrics reflect actual execution friction.
12. As an algorithmic operator, I want the headline Net Profit telemetry in the dashboard footer to reflect authentic realized trading PnL, so that I am not misled by artificial -$26k accounting drawdowns.
13. As an operator restarting the trading engine, I want historical options principal disbursements to be automatically recalibrated out of the realized PnL counter, so that past accounting bugs are permanently rectified without manual database intervention.
14. As an auditor, I want options financing outlays to be tracked in a dedicated `total_premium_outlay` accumulator, so that capital deployed in synthetic bonds is transparently audited and expected to mature at expiration.
15. As an operator executing options parity conversions, I want linear perpetual hedge legs to be clearly labelled as PERP with BUY/SELL actions, so that derivatives and underlying instruments are cleanly distinguished.
16. As an algorithmic trader running autonomous bots, I want radar bot statistical convergence cycles to log authentic entry fees and exit deltas, so that micro-profitability per trade cycle is verifiable.
17. As an operator viewing the options scanner cards, I want opportunity legs to display distinct Call, Put, and Perp identifiers with corresponding Buy/Sell actions, so that proposed routes are unambiguous before execution.
18. As a compliance reviewer, I want all financial numbers and timestamps on the PnL chart to carry explicit units and dates, so that records are legally and operationally auditable.

---

## Implementation Decisions

### 1. Tabular Presentation Architecture
- Refactored `AtomicTransactionsTable` and `PositionDetailModal` exposure tables from monolithic composite columns into separate `Action` and `Type` columns.
- Standardized action color schemes:
  - Options Bought: Emerald profit tone (positive delta rights / premium paid).
  - Options Written: Purple accent tone (short rights / premium received / short gamma).
  - Spot/Perp Long: Emerald profit tone.
  - Spot/Perp Short: Rose loss tone.
- Type badges use pill containers with 20% opacity backgrounds and 40% borders:
  - `CALL`: Emerald theme (`bg-emerald-500/10 text-emerald-600 border-emerald-500/20`).
  - `PUT`: Amber theme (`bg-amber-500/10 text-amber-600 border-amber-500/20`).
  - `PERP`: Sky blue theme (`bg-sky-500/10 text-sky-600 border-sky-500/20`).

### 2. Two-Zone Proportional Chart Canvas
- Implemented a two-zone piecewise mapping for the time axis:
  - When `hasProjection` is true:
    - Realized domain $[t_0, t_1]$ maps to horizontal range $[X_{\text{start}}, X_{\text{mid}}]$ representing 45% of plot width.
    - Projected domain $[t_1, t_2]$ maps to horizontal range $[X_{\text{mid}}, X_{\text{end}}]$ representing 55% of plot width.
  - When `hasProjection` is false:
    - Realized domain $[t_0, t_1]$ maps to 100% of plot width.
- Compact, high-clarity chart canvas dimensions: $800 \times 140$ with calibrated margins ($P_{\text{left}} = 75$, $P_{\text{right}} = 45$, $P_{\text{top}} = 22$, $P_{\text{bottom}} = 26$).
- Zero-reference axis: Subtle `$0` label rendered on the zero breakeven dotted line with right-anchored alignment at $P_{\text{left}} - 6$, making breakeven visually distinct without clutter.
- Applied vertical domain padding: $Y_{\text{pad}} = \max(1.5, 0.15 \times (Y_{\text{hi}} - Y_{\text{lo}}))$, ensuring all peak and trough nodes maintain comfortable clearance from horizontal borders.
- Dynamic text anchoring for the current PnL text label (`start` within 40px of left margin, `end` within 40px of right margin, `middle` otherwise), completely eliminating border clipping and canvas overflow across all screen widths.

### 3. Quantitative Accounting & State Store Recalibration
- In options package execution (`options_packages.rs`), net premium financing cashflows route to `accumulate_premium_outlay`, and only venue transaction fees (`fee_total`) route to `accumulate_realized_profit`.
- In engine startup (`state.rs`), introduced migration `migrated_v2`:
  - Scans the historical execution log table.
  - Computes the true realized trading PnL by taking `net_profit` for linear strategies (radar, funding, futures, spot, triangle) and `-fees` for options strategies.
  - Re-anchors `pnl.total.testnet` and `pnl.total.live` to this true realized PnL.
  - Marks the migration complete via persistent key `pnl.premium_outlay.migrated_v2`.

### 4. Positions Sidebar Card Dual PnL Architecture
- Refactored `PositionsPanel.tsx` card footer metrics:
  - For options strategies (`options_box_spread`, `options_jelly_roll`, `options_parity`), prominently displays the locked-in expected expiry profit (`Exp: +$375.84`) in bold emerald green, paired with current testnet mark-to-market (`(MtM: -$673.07)`) in muted slate.
  - Contextual hover tooltips explain that mark-to-market fluctuations are driven by illiquid testnet bid-ask spreads, while the guaranteed expiry cashflow remains economically locked at entry.
  - For linear perpetual and spot strategies, renders color-coded realized/unrealized marks (`text-emerald-600` for profits, `text-rose-600` for drawdowns).

### 5. Deribit Venue Instrument State Filtering & Rejection Telemetry
- Deribit testnet frequently undergoes partial engine lockdowns (`"locked": "partial"`) where underlying index trading is paused and all corresponding options instruments transition to `"state": "halted"`.
- Enhanced `DeribitAdapter::fetch_markets` to parse instrument `"state"`, ensuring that instruments where `state != "open"` (e.g. `"halted"` or `"closed"`) are not registered as active tradable markets.
- In `DeribitAdapter::place_order`, added specialized detection for Deribit error `10031` (`invalid_args_for_instrument`), providing operators with unambiguous telemetry indicating venue matching engine lockdowns rather than generic syntax failures.

---

## Testing Decisions

### What Makes a Good Test
- **External Behavioral Verification**: Tests must verify observable outcomes (rendered DOM text, column headers, badge contents, SVG node attributes, and persisted database PnL values) rather than internal private variables.
- **Seam Selection**:
  1. *Frontend Presentation Seam*: Verify that rendered component HTML contains distinct `Action` and `Type` table headers, valid badge text, properly scaled SVG geometry without text clipping, and dual `Exp:` / `(MtM:)` metrics on options position cards.
  2. *Backend Engine & State Store Seam*: Verify that execution log aggregations correctly separate capital financing from transaction fees, verify that KV state persists accurate PnL totals upon startup, and verify venue connector rate limiting and retry loops.

### Modules Tested
- `AtomicTransactionsTable` and `PositionDetailModal` (`PositionDetailModal.test.ts` & `TradedOpportunitiesGadget.test.ts`).
- `PositionsPanel` (`PositionsPanel.test.ts` - includes dual options card PnL test).
- `OptionsArbitrage` (Frontend options rendering test suites).
- `state::tests` and `state_store::logs` (Backend Tauri engine and SQLite store suites).

### Prior Art
- `frontend/src/components/AtomicTransactionsTable.tsx` & `PositionDetailModal.tsx`.
- `frontend/src/components/PositionsPanel.tsx` & `PositionsPanel.test.ts`.
- `frontend/src/components/PositionDetailModal.test.ts`.
- `frontend/src/components/OptionsArbitrage.test.ts`.
- `frontend/src-tauri/src/state.rs` test suite (`cargo test -p arbitrageur-desktop --bin arbitrageur-desktop state::tests`).

---

## Out of Scope

- Modifying exchange matching engine rules or order types on external venues (Deribit, Hyperliquid, Lighter).
- Converting European options into American exercisable instruments.
- Automated tax accounting or multi-jurisdiction capital gains reporting.
- Restructuring the low-level SQLite schema beyond existing v38 migration standards.

---

## Further Notes

- Following release compilation with `--features custom-protocol` and service restart, the live testnet application immediately reflected the recalibrated metrics:
  - `total_realized_pnl`: `-$297.28` (true closed trade PnL and exchange taker fees across 600+ trades).
  - `total_premium_outlay_usd`: `-$56,413.15` (active capital deployed in synthetic zero-coupon bonds).
  - Position cards clearly distinguish `Exp:` payout from `MtM:` testnet spread noise.
  - Both Radar Bot and Options Bot continue to operate autonomously with 0 audit errors.
- The `ready-for-agent` triage label is hereby applied to this specification.
