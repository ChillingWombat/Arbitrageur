import nodeTest from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  EXTERNAL_CATEGORY,
  KNOWN_PNL_CATEGORIES,
  NetPnlChartGadget,
  SeriesChart,
  cumulativeSeries,
  externalSeries,
  filterByHorizon,
  realizedSeries,
} from "./NetPnlChartGadget.tsx";
import type { TradedOpportunityDto } from "../types/index.ts";

const opp = (
  signalId: string,
  strategyId: string,
  status: TradedOpportunityDto["status"],
  realizedPnlUsd: number | null,
  createdAt = "2026-09-27T10:00:00Z",
  closedAt: string | null = null,
): TradedOpportunityDto => ({
  signalId,
  strategyId,
  symbol: "BTC-USDC",
  environment: "testnet",
  status,
  routeId: null,
  expectedNetUsd: null,
  note: null,
  venueLong: "hyperliquid",
  venueShort: "lighter",
  createdAt,
  closedAt,
  lotId: null,
  lotStatus: null,
  entrySpreadBps: null,
  closeSpreadBps: null,
  realizedPnlUsd,
  realizedPnlBps: null,
  legNetFlowUsd: 0,
  totalFeesUsd: 0,
  childCount: 0,
  transactions: [],
});

nodeTest("realizedSeries folds closed P&L cumulatively in time order", () => {
  const rows = [
    opp("b", "funding", "closed", -2, "2026-09-27T11:00:00Z", "2026-09-27T12:00:00Z"),
    opp("a", "funding", "closed", 5, "2026-09-27T09:00:00Z", "2026-09-27T10:00:00Z"),
    opp("c", "funding", "balanced", null), // open — no realized yet
  ];
  const s = realizedSeries(rows, "Funding Rate");
  assert.equal(s.length, 2);
  assert.deepEqual(s.map((p) => p.valueUsd), [5, 3]);
  // closedAt (not createdAt) carries the point
  assert.equal(s[1].tsMs, Date.parse("2026-09-27T12:00:00Z"));
});

nodeTest("realizedSeries scopes to one broad strategy category", () => {
  const rows = [
    opp("o1", "options_parity", "closed", 10, "2026-09-27T09:00:00Z"),
    opp("o2", "options_box_spread", "closed", 4, "2026-09-27T10:00:00Z"),
    opp("f1", "funding", "closed", 7, "2026-09-27T11:00:00Z"),
  ];
  // both options substrategies collapse into the "Options" family
  assert.deepEqual(realizedSeries(rows, "Options").map((p) => p.valueUsd), [10, 14]);
  assert.deepEqual(realizedSeries(rows, "all").map((p) => p.valueUsd), [10, 14, 21]);
  assert.equal(realizedSeries(rows, "Funding Rate").length, 1);
  assert.equal(realizedSeries(rows, EXTERNAL_CATEGORY).length, 0);
});

nodeTest("externalSeries cumulates account flows independently of the ledger", () => {
  const s = externalSeries([
    { tsMs: 2000, valueUsd: -25 },
    { tsMs: 1000, valueUsd: 100 },
  ]);
  assert.deepEqual(s, [
    { tsMs: 1000, valueUsd: 100 },
    { tsMs: 2000, valueUsd: 75 },
  ]);
});

nodeTest("cumulativeSeries starts from zero and tolerates empty input", () => {
  assert.deepEqual(cumulativeSeries([]), []);
  assert.deepEqual(cumulativeSeries([{ tsMs: 1, valueUsd: -4 }]), [{ tsMs: 1, valueUsd: -4 }]);
});

nodeTest("SeriesChart renders the path and zero line without an in-graph label", () => {
  const html = renderToStaticMarkup(
    React.createElement(SeriesChart, {
      points: [
        { tsMs: Date.parse("2026-09-27T10:00:00Z"), valueUsd: 5 },
        { tsMs: Date.parse("2026-09-27T12:00:00Z"), valueUsd: -3 },
      ],
      step: true,
    }),
  );
  assert.match(html, /<svg/);
  assert.match(html, /<path/);
  assert.doesNotMatch(html, /−\$3/); // last value lives in the header, not inside the plot
});

nodeTest("SeriesChart degrades honestly on one or zero points", () => {
  const single = renderToStaticMarkup(
    React.createElement(SeriesChart, { points: [{ tsMs: 1, valueUsd: 2 }] }),
  );
  assert.match(single, /<circle/); // a lone event draws a dot, not a fake trend
  const empty = renderToStaticMarkup(React.createElement(SeriesChart, { points: [] }));
  assert.match(empty, /No history yet/);
  assert.doesNotMatch(empty, /<svg/);
});

nodeTest("KNOWN_PNL_CATEGORIES unifies options under 'Options' without substrategies", () => {
  assert.ok(KNOWN_PNL_CATEGORIES.includes("Options"));
  assert.ok(!KNOWN_PNL_CATEGORIES.includes("Options Parity"));
  assert.ok(!KNOWN_PNL_CATEGORIES.includes("Box Spread"));
  assert.ok(!KNOWN_PNL_CATEGORIES.includes("Jelly Roll"));
  assert.ok(!KNOWN_PNL_CATEGORIES.includes("Convexity"));
});

nodeTest("KNOWN_PNL_CATEGORIES follows the order of top tab switchers", () => {
  assert.deepEqual(KNOWN_PNL_CATEGORIES, [
    "Simple Spread",
    "Spread Radar",
    "Funding Rate",
    "Spatial",
    "Triangle",
    "Options",
    "Prediction Market",
    "Statistical",
  ]);
});

nodeTest("gadget renders the strategy filter in the top bar header next to title and unifies options", () => {
  const html = renderToStaticMarkup(React.createElement(NetPnlChartGadget));
  assert.match(html, /Net P&amp;L/);
  assert.match(html, /Net Equity/);
  assert.match(html, /All strategies/);
  assert.match(html, /External ops/);
  assert.match(html, /Simple Spread/); // known families appear even with no realized rows yet
  assert.match(html, /Spread Radar/);
  assert.match(html, /Loading P&amp;L history/);

  // Strategy filter dropdown is in the top bar header directly next to <h2> title
  assert.match(
    html,
    /<div class="flex items-center gap-3 min-w-0 flex-wrap"><h2[^>]*>[\s\S]*?<\/h2><select[^>]*aria-label="Filter P&amp;L by strategy category"/,
  );
  // Redundant second row below divider is eliminated
  assert.doesNotMatch(html, /<div class="flex items-center gap-2 flex-wrap"><select/);

  // "Options" is present in the rendered options dropdown
  assert.match(html, /<option value="Options">Options<\/option>/);

  // Individual options substrategies are NOT present in the dropdown
  assert.doesNotMatch(html, /Options Parity/);
  assert.doesNotMatch(html, /Box Spread/);
  assert.doesNotMatch(html, /Jelly Roll/);
  assert.doesNotMatch(html, /Convexity/);
});

nodeTest("realizedSeries matches all options substrategies under 'Options'", () => {
  const rows = [
    opp("o1", "options_parity", "closed", 10, "2026-09-27T09:00:00Z"),
    opp("o2", "options_box_spread", "closed", 5, "2026-09-27T10:00:00Z"),
    opp("o3", "options_jelly_roll", "closed", 3, "2026-09-27T11:00:00Z"),
    opp("o4", "options_convexity", "closed", 2, "2026-09-27T12:00:00Z"),
    opp("f1", "funding", "closed", 7, "2026-09-27T13:00:00Z"),
  ];
  const pts = realizedSeries(rows, "Options");
  assert.equal(pts.length, 4);
  assert.deepEqual(pts.map((p) => p.valueUsd), [10, 15, 18, 20]);
});

nodeTest("filterByHorizon filters points to window and prepends anchor value", () => {
  const now = 1_000_000_000_000;
  const day = 86_400_000;
  const pts = [
    { tsMs: now - 40 * day, valueUsd: 10 },
    { tsMs: now - 20 * day, valueUsd: 20 },
    { tsMs: now - 3 * day, valueUsd: 30 },
  ];
  // 1W: only now - 3*day is in window; prepends anchor value 20 from previous point
  const w1 = filterByHorizon(pts, "1W", now);
  assert.equal(w1.length, 2);
  assert.equal(w1[0].valueUsd, 20);
  assert.equal(w1[1].valueUsd, 30);

  // 1M: 30 days window covers now - 20*day and now - 3*day; prepends anchor 10
  const m1 = filterByHorizon(pts, "1M", now);
  assert.equal(m1.length, 3);
  assert.equal(m1[0].valueUsd, 10);
  assert.equal(m1[1].valueUsd, 20);
  assert.equal(m1[2].valueUsd, 30);

  // ALL: returns all points unchanged
  const all = filterByHorizon(pts, "ALL", now);
  assert.deepEqual(all, pts);
});

nodeTest("gadget renders Performance History title and horizon buttons", () => {
  const html = renderToStaticMarkup(React.createElement(NetPnlChartGadget));
  assert.match(html, /Performance History/);
  assert.match(html, />1W<\/button>/);
  assert.match(html, />1M<\/button>/);
  assert.match(html, />1Y<\/button>/);
  assert.match(html, />ALL<\/button>/);
});

