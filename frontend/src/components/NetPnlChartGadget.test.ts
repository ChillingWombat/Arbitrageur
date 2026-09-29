import nodeTest from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  EXTERNAL_CATEGORY,
  NetPnlChartGadget,
  SeriesChart,
  cumulativeSeries,
  externalSeries,
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

nodeTest("gadget renders the two tabs and the strategy filter", () => {
  const html = renderToStaticMarkup(React.createElement(NetPnlChartGadget));
  assert.match(html, /Net P&amp;L/);
  assert.match(html, /Net Equity/);
  assert.match(html, /All strategies/);
  assert.match(html, /External ops/);
  assert.match(html, /Simple Spread/); // known families appear even with no realized rows yet
  assert.match(html, /Spread Radar/);
  assert.match(html, /Loading P&amp;L history/);
});
