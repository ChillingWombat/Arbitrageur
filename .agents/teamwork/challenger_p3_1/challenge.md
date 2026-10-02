# Adversarial Challenge Report — R1, R2, R5, R6, R8

## Challenge Summary

**Overall risk assessment**: LOW

All implementations for requirements R1 (`PredictionHub.tsx`), R2 (`OpportunityRadar.tsx` & `SpotArbitrageBot.tsx`), R5 (`FundingRadar.tsx`), R6 (`OptionsArbitrage.tsx`), and R8 (`TickerExplorer.tsx`) have been empirically verified and stress-tested with 17 adversarial automated tests in `frontend/src/components/ChallengerR1R2R5R6R8.test.ts`. Every adversarial scenario passed with zero regressions.

---

## Challenges

### [Low] Challenge 1: PredictionHub Scaffold Warning Leak Under Not-Ready States (R1)
- **Assumption challenged**: The removal of the scaffold warning text might only apply when execution is ready or capability is present, inadvertently leaking the banner under unconfigured or scaffold capability objects.
- **Attack scenario**: Render `PredictionHub` with `capability = { strategyId: "prediction", scanReady: false, executeReady: false, reason: "Prediction venues are scaffolds: no market data or order execution path" }` and `capability = undefined`.
- **Blast radius**: User confusion seeing obsolete scaffold warnings on what should be a clean interface.
- **Stress test result**: PASS. The JSX for the scaffold warning banner was completely eliminated. The text does not exist anywhere in rendered DOM under any capability descriptor or absence thereof.

### [Low] Challenge 2: PredictionHub Max Slip and Min Vol Badge Fallback (R1)
- **Assumption challenged**: Min Vol might display obsolete disabled text (`no venue volume data`) or disabled styling (`opacity-60`) if volume is not configured in backend params. Max Slip might remain in tooltips or secondary views.
- **Attack scenario**: Render without props, with zero volume, with large volume ($1,500,000 USD), and with custom $500,000 USD.
- **Blast radius**: Inconsistent strategy parameter reporting and misleading inactive badges.
- **Stress test result**: PASS. `Max Slip` is 100% removed. `Min Vol` renders as an active badge defaulting to `10,000 USD` and correctly formats any numerical input (e.g. `500,000 USD`, `1,500,000 USD`, `0 USD`) using `.toLocaleString()`.

### [Low] Challenge 3: OpportunityRadar Execution Mode Badge and Filter Header Alignment (R2)
- **Assumption challenged**: `Execution Single-Way` badge could persist when paused or during initial load. The table header might have misaligned filter controls on narrow viewports or push dropdown menus offscreen.
- **Attack scenario**: Render `OpportunityRadar` with paused=true/false. Inspect the table card header DOM structure and verify dropdown alignment attributes.
- **Blast radius**: Visual clutter from deprecated execution badges; dropdown menus clipped by viewport boundaries.
- **Stress test result**: PASS. `Execution Single-Way` is completely purged from the ribbon. The Opportunity Scanner table header uses `flex items-center justify-between` with the title on the left and filters on the right. The venue dropdown is explicitly pinned with `absolute right-0 top-full` to eliminate right-side screen overflow.

### [Low] Challenge 4: SpotArbitrageBot Header Parity with Funding Rate (R2)
- **Assumption challenged**: Spot Arbitrage Bot's opportunity scanner might use a different header layout or left-aligned filters.
- **Attack scenario**: Render `SpotArbitrageBot` with active capability. Inspect the Opportunity Scanner card header.
- **Blast radius**: Visual inconsistency between Spatial and Funding Rate scanner gadgets.
- **Stress test result**: PASS. Filter bar has `flex items-center justify-between` with `<h3 class="text-[13px] font-bold font-sans tracking-wider text-slate-900">Opportunity Scanner</h3>` on the left and filter controls (`Search`, `Filter Asset`, count) grouped on the right. Asset dropdown menu is anchored with `right-0 top-full`.

### [Low] Challenge 5: FundingRadar Scanner Dimensions and Card Height Uniformity (R5)
- **Assumption challenged**: With few cards (1 card) or many cards (25 cards), the gadget might resize or individual cards might expand to fill available vertical space, creating uneven card heights or breaking the 2-row layout.
- **Attack scenario**: Render `FundingRadar` with 1, 2, 5, 10, 15, and 25 opportunity cards.
- **Blast radius**: Layout jumping, vertical scrollbars within individual cards, or broken dashboard grid alignment.
- **Stress test result**: PASS. Outer container is locked to `h-[600px] flex flex-col`. Each card strictly has `h-[250px] overflow-hidden` across all card count scenarios. Multi-row scrolling is handled cleanly by the inner `flex-1 min-h-0 overflow-y-auto` container without resizing the outer gadget.

### [Low] Challenge 6: OptionsArbitrage Ribbon Visibility for 'all' vs Specific Substrategies (R6)
- **Assumption challenged**: Ribbon might render empty or partially when `selected="all"`, or fail to render the correct titles, scan buttons, and controls when specific substrategies are selected.
- **Attack scenario**: Render with `initialSelected="all"`, `initialSelected=undefined` (defaults to "all"), and across all live substrategies (`parity`, `box_spread`, `jelly_roll`, `convexity`).
- **Blast radius**: Redundant / non-functional bot controls shown in aggregate monitor view, or missing controls when inspecting specific substrategies.
- **Stress test result**: PASS. Ribbon is completely omitted when `selected === "all"`. When a live substrategy is selected, the ribbon renders with the substrategy title, `BotPhaseBadge`, `Min Edge`, `Qty`, `Scan` button, `Configurator`, and `BotLifecycleControls`.

### [Low] Challenge 7: TickerExplorer Symbol Quote Suffix Handling Across Cases (R8)
- **Assumption challenged**: Lowercase or mixed-case USD quotes (e.g. `APT-usd`, `ETH-Usd`) might fail string equality and leak `-usd` suffix, while non-USD quotes (e.g. `APT-usdc`, `ETH-eur`) might be inadvertently stripped.
- **Attack scenario**: Render tickers with quotes `"USD"`, `"usd"`, `"Usd"`, `"uSd"`, `"USDC"`, `"usdc"`, `"USDT"`, `"usdt"`, `"EUR"`, `"eur"`.
- **Blast radius**: Inconsistent ticker labels; inability to distinguish stablecoin quote variants from fiat pairs.
- **Stress test result**: PASS. `quote.toUpperCase() !== "USD"` correctly suppresses `-USD` across all casing variations (rendering only `APT`, `BTC`, `ETH`, `SOL`). All non-USD quotes retain their `-USDC`, `-usdc`, `-USDT`, `-usdt`, `-EUR`, `-eur` tags.

---

## Stress Test Results

| Test Scenario | Expected Behavior | Actual Behavior | Verdict |
|---|---|---|---|
| `PredictionHub` capability scaffold / undefined | No scaffold warning banner rendered | Text completely absent from DOM | PASS |
| `PredictionHub` Max Slip pill | Pill omitted across all execution states | Zero occurrences of Max Slip | PASS |
| `PredictionHub` search input placeholder | `placeholder="Search contract..."` | Exactly `Search contract...` (no `(e.g. FED, BTC)`) | PASS |
| `PredictionHub` Min Vol default | `10,000 USD` active badge | Rendered in active `ribbon-pill` without disabled text | PASS |
| `PredictionHub` Min Vol custom | Formatted with commas (e.g. `500,000 USD`) | Rendered tabular-nums `500,000` + `USD` | PASS |
| `OpportunityRadar` Execution Single-Way badge | Badge removed from strategy ribbon | Zero occurrences in ribbon | PASS |
| `OpportunityRadar` Opportunity Scanner header | Header on left, filters on right | `justify-between` flex layout matches spec | PASS |
| `OpportunityRadar` venue dropdown position | Anchored `right-0 top-full` | `absolute right-0 top-full mt-1.5 w-52` | PASS |
| `SpotArbitrageBot` Opportunity Scanner header | Header on left, filters on right | `p-3.5 border-b ... flex justify-between` matches Funding layout | PASS |
| `SpotArbitrageBot` asset dropdown position | Anchored `right-0 top-full` | `dropdown-menu right-0 top-full mt-1.5 w-48` | PASS |
| `FundingRadar` scanner gadget container height | Fixed `h-[600px]` | Exactly `h-[600px] flex flex-col` | PASS |
| `FundingRadar` cards (1, 2, 5, 10, 15, 25 cards) | Fixed `h-[250px] overflow-hidden` | Every card has `h-[250px] overflow-hidden` | PASS |
| `OptionsArbitrage` `selected="all"` (explicit & default) | Strategy ribbon omitted | Ribbon container completely unrendered | PASS |
| `OptionsArbitrage` live substrategies (`parity`, `box`, `jelly`, `convexity`) | Strategy ribbon rendered with title, badge & controls | Full ribbon rendered with correct names and buttons | PASS |
| `TickerExplorer` USD tickers (`USD`, `usd`, `Usd`, `uSd`) | `-USD` quote suppressed, base symbol displayed | Base symbol rendered alone; no `-USD` suffix | PASS |
| `TickerExplorer` non-USD tickers (`USDC`, `usdc`, `USDT`, `usdt`, `EUR`, `eur`) | Quote suffix preserved | Suffixes `-USDC`, `-usdc`, `-USDT`, `-usdt`, `-EUR`, `-eur` rendered | PASS |
| `TickerExplorer` mixed ticker list | USD pairs suppressed, non-USD preserved | USD clean base, non-USD with quote badges | PASS |

---

## Unchallenged Areas

- **R3 (Button Set Padding)**: Handled and challenged by peer reviewer/challenger agent `challenger_p3_2`.
- **R4 (Dislocation Matrix & Signal Trend)**: Handled and challenged by peer reviewer/challenger agent `challenger_p3_2`.
- **R7 (Security Page Explanations & MFA Wording)**: Handled and challenged by peer reviewer/challenger agent `challenger_p3_2`.
- **Backend Rust Crates**: Out of scope for this frontend component polishing milestone; backend logic was unchanged.
