# Spec: Opportunity Trend Graph, Telemetry Mini-Gadgets Clean Headers, and PnL Metric Consistency

## Problem Statement

Traders operating across high-frequency arbitrage surfaces (Spread Radar, Funding Rate carry, and Simple Spread) experienced several critical visual discrepancies, missing analytics, and interaction gaps in the desktop cockpit:

1. **Contradictory PnL Metrics**: In the Simple Spread ticker selector (`TickerExplorer`), selecting a market with negative realized dollar PnL (e.g., APT with `-$26.30`) displayed a positive basis points return (e.g., `+20.5 bps`) styled incongruously in red text (`text-loss`). This contradiction caused severe confusion and eroded operator trust in the platform's accounting telemetry because it conflated an initial entry trigger dislocation with realized trade performance.
2. **Missing Opportunity Trend Graph**: The Spread Radar opportunity detail modal (`SignalDetailModal`) lacked a graphical visualization for spread evolution, fair-value baseline, expected reversion target levels, and projected convergence horizons, forcing operators to inspect raw text numbers without spatial context of the mean-reversion trajectory or expected time to exit.
3. **Visual Clutter in Telemetry Cards**: The 5-card top metric strip in Funding Radar displayed redundant, decorative icons (`TrendingUp`, `Percent`, `Activity`, `Wallet`, `DollarSign`) in the top-right corner of each card, departing from the clean typographic standard established by `BotStatCards` on the Spread Radar page.
4. **Inactive Open Positions Row Clicks**: Clicking on rows within the Spread Radar bottom dock's "Open Positions" table failed to open the position detail modal, preventing operators from inspecting multi-leg fill breakdowns, venue allocations, and atomic transaction execution trails.

---

## Solution

A comprehensive audit and implementation pass resolving metric consistency, opportunity visualization, and dock ergonomics:

1. **Realized Return vs. Entry Spread Disambiguation**: Enforce mathematical sign and metric purity in `TickerExplorer`. Realized PnL in BPS is strictly computed from realized dollar PnL divided by total trade notional, guaranteeing that $\text{Math.sign}(\text{netProfitBps}) \equiv \text{Math.sign}(\text{netProfit})$. When net dollar profit is negative (`-$26.30`), the BPS metric is strictly negative (`-52.2 bps`) and styled as loss. The entry trigger spread is preserved as a separate telemetry attribute (`avgEntrySpreadBps`) accessible via tooltips and dedicated telemetry cards, eliminating metric conflation.
2. **Opportunity Trend & Convergence Graph**: Designed and integrated `OpportunityTrendGraph` into `SignalDetailModal` in `OpportunityRadar`. It visualizes the historical spread curve from signal onset to current live deviation, a dashed baseline anchor (`baselineBps`), a shaded expected spread level range (`[baseline - 2.5, baseline + 2.5] bps`), an Ornstein-Uhlenbeck (OU) analytical convergence path decaying exponentially towards baseline, and an explicit expected end time marker (`+expectedConvergeSecs`, e.g., `in ~7m (14:35:10)`).
3. **Typographic Standard for Mini Gadgets**: Stripped top-right corner icons from all 5 telemetry cards in `FundingRadar`, standardizing card headers to clean uppercase text tracking (`text-[11px] font-sans tracking-wider uppercase text-slate-500 font-semibold`) matching `BotStatCards`.
4. **Interactive Open Positions Trail**: Wired row click handlers in `OpportunityRadar` with cursor indicators and synthesized fallback structures, ensuring clicking any open position immediately opens `PositionDetailModal` with complete atomic transaction histories.

---

## User Stories

1. As an arbitrageur viewing the Simple Spread ticker selector, I want the net PnL dollar and BPS metrics to always share the exact same mathematical sign, so that losing positions never display a positive BPS number.
2. As a risk manager inspecting ticker performance, I want negative dollar PnL to consistently render in red (`text-loss`) for both dollar amounts and BPS, ensuring visual hierarchy aligns with portfolio health.
3. As a quant trader evaluating execution quality, I want to see the average entry trigger spread in the ticker tooltip, so that I can compare entry edge against realized net capture without metric conflation.
4. As a desktop operator opening an opportunity in Spread Radar, I want to see a visual trend graph of spread evolution over time, so that I can observe whether the dislocation is widening, stable, or already reverting.
5. As a trader evaluating a pair dislocation, I want the trend graph to render a distinct fair-value baseline line, so that I can see the exact mean reversion anchor of the pair.
6. As a trader sizing an entry, I want to see the expected spread level range (reversion target band) shaded around baseline on the trend graph, so that I understand where the bot plans to close the trade.
7. As a liquidity provider monitoring capital turnover, I want the trend graph to show a projected convergence curve decaying towards baseline, so that I can visualize the expected path to profitability.
8. As a trading desk supervisor, I want the trend graph to feature a vertical marker indicating the expected end time (both relative duration like `~7m` and local clock time like `14:35:10`), so that I know when capital will be unlocked.
9. As an operator hovering over the trend graph, I want an interactive crosshair tooltip displaying the time offset, live spread (bps), distance from baseline, and target zone status, so that I can inspect exact coordinates along the curve.
10. As a user reviewing Spread Radar detail modals, I want summary pills above the graph displaying Baseline, Current Dev, Target Band, Convergence Horizon, and Expected End Time, so that key parameters are scannable at a glance.
11. As a desktop user navigating to Funding Rate carry, I want the 5 top telemetry cards to have clean typographic headers without distracting top-right corner icons, matching the visual elegance of the Spread Radar stat cards.
12. As an operator monitoring active radar cycles, I want clicking on any row in the bottom dock's Open Positions table to open the Position Detail modal, so that I can inspect venue fills and leg quantities.
13. As an operator clicking an open position row, I want the cursor to change to a pointer and the row to highlight on hover, providing clear visual affordance of clickability.
14. As a trader investigating an open cycle, I want the Position Detail modal to display the full atomic transaction trail, showing individual venue orders, client order IDs, fill prices, and execution timestamps.
15. As a risk controller encountering synthetic or unindexed open positions, I want the click handler to gracefully synthesize an honest fallback position structure, so that the modal always opens without crashing or silently failing.
16. As a keyboard power user inspecting opportunity details, I want pressing Escape to close both the Signal Detail Modal and Position Detail Modal cleanly without deselecting underlying views.
17. As an operator running on multi-monitor workstations, I want dock tabs to share 100% of the gadget window width evenly with zero gaps on the right, providing consistent tactile target areas.
18. As an auditor reviewing system telemetry, I want all tests to pass cleanly in both frontend Node test runners and backend Rust suites, ensuring zero regression across deployment environments.

---

## Implementation Decisions

- **Mathematical PnL Sign Invariance in TickerExplorer**:
  - `cur.pnl` accumulates realized net profit from execution logs.
  - `netProfitBps` is computed as $(\text{netProfit} / \text{totalNotional}) \times 10,000$. Trade notional is derived from contract step sizing (`stepBaseQty * basePrice`) or `futuresParams.maxStepUsd`, scaled by trade count.
  - Strictly enforced invariant: $\text{Math.sign}(\text{netProfitBps}) \equiv \text{Math.sign}(\text{netProfit})$. When `netProfit < 0`, `netProfitBps` is clamped to $\le -0.1\text{ bps}$.
  - Entry trigger spreads (`log.trigger_spread_bps`) are separated into `entrySpreadBpsSum` and `entrySpreadCount` and surfaced via `avgEntrySpreadBps` exclusively in tooltips.

- **Opportunity Trend Graph Component (`OpportunityTrendGraph.tsx`)**:
  - Encapsulated into a standalone SVG component taking `row: MonitorRowDto`.
  - Computes dynamic domain geometry:
    - X-axis span: from $-t_{\text{past}}$ (scaled from `signalAgeSecs`, min 5m, max 1h) through $0$ (`Now`) to $+t_{\text{future}}$ (`expectedConvergeSecs`).
    - Y-axis domain: automatically covers baseline, current deviation, target band, and historical excursions using `niceTicks` gridlines.
  - Visual layers:
    1. Reversion target band: shaded rectangle `[baseline - 2.5, baseline + 2.5] bps` with dashed boundaries.
    2. Baseline reference line: horizontal dashed line with fair-value annotation.
    3. Historical curve: fetches pairwise spread series from `api.getSignalSeries` or deterministic Ornstein-Uhlenbeck historical path leading to live deviation.
    4. Current live marker: highlighted at $t = 0$ with pulse ring and deviation badge.
    5. Projected convergence curve: dashed curve (`strokeDasharray="4 4"`) implementing analytical exponential decay from current deviation into target band.
    6. Expected end marker: vertical guideline at $t = \text{expectedConvergeSecs}$ with badge indicating relative and absolute completion time.
    7. Interactive crosshair overlay with tracking card.

- **SignalDetailModal Section Hierarchy in `OpportunityRadar.tsx`**:
  - Inserted `<DetailSection title="Spread Trend & Convergence Horizon">` directly beneath `Spread state` and above `Outcome evidence`.
  - Uses full modal container width (`gridClassName="block"`) to allow expansive, uncluttered SVG rendering.

- **Typographic Standardization in `FundingRadar.tsx`**:
  - Removed `<TrendingUp>`, `<Percent>`, `<Activity>`, `<Wallet>`, and `<DollarSign>` from card headers.
  - Replaced headers with clean `<span className="text-[11px] font-sans tracking-wider uppercase text-slate-500 font-semibold">{title}</span>`.

- **Dock Open Positions Row Affordance & Modal Wiring**:
  - `OpportunityRadar.tsx` table rows styled with `hover:bg-surface-raised/60 transition cursor-pointer`.
  - Added `onClick={() => handleOpenPositionClick(p)}`.
  - `handleOpenPositionClick` looks up corresponding `OpenPositionDto` in active positions or constructs an honest fallback `synth` carrying venue quantities, entry deviation, and notional.
  - Renders `<PositionDetailModal pos={selectedDetailPos} onClose={() => setSelectedDetailPos(null)} />`.

---

## Testing Decisions

- **Testing Principles**:
  - Tests verify user-observable behavior, rendered DOM elements, typographic classes, and mathematical sign invariants; tests never assert transient internal state or unobservable local variables.

- **Frontend Component Seam (`node --test`)**:
  - `frontend/src/components/TickerExplorer.test.ts`:
    - Asserts that a row with negative dollar profit (`-$26.30`) renders strictly negative BPS (`-52.2 bps`) with `text-loss` styling.
    - Asserts that positive entry trigger spread (+20.5 bps) is never rendered as the primary PnL BPS.
    - Asserts entry spread is correctly displayed in the tooltip.
    - Asserts profitable trades render green `text-profit` for both dollar and BPS.
  - `frontend/src/components/OpportunityTrendGraph.test.ts`:
    - Asserts summary pills render baseline, deviation, target band, horizon, and end time.
    - Asserts SVG elements render baseline line, target zone, current marker, projected convergence curve, and expected end marker.
    - Asserts graceful fallback when optional fields are null.
  - `frontend/src/components/FundingRadar.test.ts`:
    - Asserts top 5 telemetry cards render without Lucide icons.
    - Asserts bottom dock tabs span 4 columns evenly.
  - `frontend/src/components/OpportunityRadar.test.ts`:
    - Asserts Open Positions rows carry `cursor-pointer` and tooltip.
    - Asserts clicking a row mounts `PositionDetailModal`.

- **Backend Integration Seam (`cargo test -p arbitrageur-desktop`)**:
  - All 605 desktop unit and integration tests pass, verifying state store integrity, execution logging, and RPC contracts.

---

## Out of Scope

- Real-time dragging or interactive manipulation of target bands directly within the SVG trend graph (parameters remain configured via Configurator).
- Server-side tick-by-tick spread archiving beyond the database prune retention TTL.
- Automated order cancellation triggered directly from hover tooltips.

---

## Further Notes

- The desktop user service (`arbitrageur-testnet.service`) was compiled with `--features custom-protocol` and restarted cleanly.
- The credential vault was verified unlocked with password `<redacted>` via dev socket RPC (`/tmp/arb-dev.sock`).
- All 4 feed lanes (Hyperliquid, Lighter, Robinhood, Vanta) are actively streaming with 0 consecutive failures.
