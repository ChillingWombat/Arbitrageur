import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { PositionsPanel } from "./PositionsPanel.tsx";
import type { OpenPositionDto } from "../types/index.ts";

const position: OpenPositionDto = {
  key: "futures|BTC-USDC|hyperliquid+lighter",
  strategyId: "futures",
  symbol: "BTC-USDC",
  signalCount: 2,
  venues: [
    { venue: "hyperliquid", netQty: 1.5 },
    { venue: "lighter", netQty: -1.5 },
  ],
  expectedNetUsd: 25,
  unrealizedPnlUsd: 4.2,
  feesUsd: 1.0,
  notionalUsd: 90000,
  openedAt: "2026-01-01T00:00:00Z",
};

const baseProps = {
  positions: [position],
  currentStrategyId: "futures" as string | null,
  currentStrategyLabel: "Single Spread",
  selectedPositionKey: null as string | null,
  onSelectPosition: () => {},
  onOpenTickerExplorer: () => {},
};

test("position card renders identity, grey PnL, and signed venue rows", () => {
  const html = renderToStaticMarkup(React.createElement(PositionsPanel, baseProps));
  // Identity + strategy chip + PnL field below
  assert.match(html, /BTC-USDC/);
  assert.match(html, /SPREAD/);
  assert.match(html, /PnL \+\$4\.20/);
  // Venue rows: long on top (+), short below (-)
  const hl = html.indexOf(">HL<");
  const lt = html.indexOf(">LGT<");
  assert.ok(hl >= 0 && lt > hl, "long venue row renders above short venue row");
  assert.match(html, /\+1\.5000/);
  assert.match(html, /-1\.5000/);
});

test("selected card carries the light-blue accent-dim class", () => {
  const html = renderToStaticMarkup(
    React.createElement(PositionsPanel, {
      ...baseProps,
      selectedPositionKey: position.key,
    }),
  );
  assert.match(html, /bg-accent-dim/);
  assert.match(html, /border-l-accent/);
});

test("unavailable PnL renders an em dash, never a fabricated zero", () => {
  const html = renderToStaticMarkup(
    React.createElement(PositionsPanel, {
      ...baseProps,
      positions: [{ ...position, unrealizedPnlUsd: null }],
    }),
  );
  assert.match(html, /PnL —/);
  assert.doesNotMatch(html, /PnL \$0\.00/);
});

test("empty state is explicit and honest", () => {
  const html = renderToStaticMarkup(
    React.createElement(PositionsPanel, { ...baseProps, positions: [] }),
  );
  assert.match(html, /No Open Positions/);
});

test("current-strategy scope disables gracefully on strategy-less tabs", () => {
  const html = renderToStaticMarkup(
    React.createElement(PositionsPanel, {
      ...baseProps,
      currentStrategyId: null,
      currentStrategyLabel: "Portfolio",
    }),
  );
  // The scope button renders disabled with the explanatory tooltip.
  assert.match(html, /This page has no single-strategy scope/);
});
