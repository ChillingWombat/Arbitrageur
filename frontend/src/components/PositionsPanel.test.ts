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
    { venue: "hyperliquid", symbol: "BTC-USDC", netQty: 1.5 },
    { venue: "lighter", symbol: "BTC-USDC", netQty: -1.5 },
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
  currentStrategyLabel: "Simple Spread",
  selectedPositionKey: null as string | null,
  onSelectPosition: () => {},
  onOpenTickerExplorer: () => {},
};

test("position card renders identity, strategy chip under the name, and PnL under the sizes", () => {
  const html = renderToStaticMarkup(React.createElement(PositionsPanel, baseProps));
  // Strategy chip renders AFTER (below) the ticker, PnL sits under the
  // long/short sums in the right column.
  const chip = html.indexOf(">SPREAD<");
  const sym = html.indexOf(">BTC-USDC<");
  assert.ok(sym >= 0 && chip > sym, "strategy chip renders below the symbol");
  const longIdx = html.indexOf(">Long<");
  const shortIdx = html.indexOf(">Short<");
  const pnlIdx = html.indexOf("PnL +$4.20");
  assert.ok(longIdx >= 0 && shortIdx > longIdx, "long sum renders above short sum");
  assert.ok(pnlIdx > shortIdx, "PnL renders under the position sizes");
  assert.match(html, /PnL \+\$4\.20/);
  assert.match(html, /\+1\.5/);
  assert.match(html, /-1\.5/);
  assert.doesNotMatch(html, /1\.5000/);
  assert.doesNotMatch(html, /HL BTC-USDC/);
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

test("position card has right-click context menu hint", () => {
  const html = renderToStaticMarkup(React.createElement(PositionsPanel, baseProps));
  assert.match(html, /Left-click: position details · Right-click: Reduce-Only \/ Clear actions/);
});

test("position card renders Reduce-Only badge when marked reduce-only", () => {
  const storage = new Map<string, string>();
  storage.set("arbitrageur:reduce_only_positions", JSON.stringify(["BTC-USDC"]));
  const mockLocalStorage = {
    getItem: (key: string) => storage.get(key) ?? null,
    setItem: (key: string, val: string) => {
      storage.set(key, val);
    },
    removeItem: (key: string) => {
      storage.delete(key);
    },
    clear: () => {
      storage.clear();
    },
  };

  const origWindow = (globalThis as any).window;
  (globalThis as any).window = {
    localStorage: mockLocalStorage,
    dispatchEvent: () => true,
  };

  try {
    const html = renderToStaticMarkup(React.createElement(PositionsPanel, baseProps));
    assert.match(html, /Reduce-Only/);
    assert.match(html, /No new step increases; position closes on target profit/);
  } finally {
    if (origWindow === undefined) {
      delete (globalThis as any).window;
    } else {
      (globalThis as any).window = origWindow;
    }
  }
});

