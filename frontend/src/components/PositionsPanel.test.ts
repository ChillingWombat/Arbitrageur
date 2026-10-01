import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { PositionsPanel } from "./PositionsPanel.tsx";
import type { OpenPositionDto } from "../types/index.ts";

const position: OpenPositionDto = {
  key: "futures|s1",
  tradeId: "s1",
  strategyId: "futures",
  symbol: "BTC-USDC",
  legSymbols: ["BTC-USDC"],
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
  reduceOnly: false,
};

const baseProps = {
  positions: [position],
  currentStrategyId: "futures" as string | null,
  currentStrategyLabel: "Simple Spread",
  selectedPositionKey: null as string | null,
  onSelectPosition: () => {},
  onOpenTickerExplorer: () => {},
};

test("position card renders identity, strategy chip, per-leg nets, and PnL", () => {
  const html = renderToStaticMarkup(React.createElement(PositionsPanel, baseProps));
  // Strategy chip renders AFTER (below) the ticker — the card footer pairs
  // it with the bare PnL figure (no "PnL" prefix).
  const chip = html.indexOf(">SPREAD<");
  const sym = html.indexOf(">BTC-USDC<");
  assert.ok(sym >= 0 && chip > sym, "strategy chip renders below the symbol");
  assert.match(html, /\+\$4\.20/);
  assert.doesNotMatch(html, /PnL \+\$/);
  // Per-(venue, side) rows carry the venue name on the left and the bare
  // signed qty on the right — no "@venue" suffix, no per-instrument
  // labels. Anchor on element boundaries — "-1.5" alone collides with
  // the `space-x-1.5` utility class earlier in the markup.
  const longLeg = html.indexOf(">+1.5<");
  const shortLeg = html.indexOf(">-1.5<");
  assert.ok(longLeg >= 0 && shortLeg > longLeg, "long leg renders above short leg");
  assert.match(html, /hyperliquid/);
  assert.match(html, /lighter/);
  assert.doesNotMatch(html, /@hyperliquid/);
  assert.doesNotMatch(html, /@lighter/);
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
  // The footer P&L figure is an em dash, never a fabricated zero — anchored
  // to the card footer (strategy chip → PnL span), not panel-wide markup.
  assert.match(html, /SPREAD<\/span><span[^>]*>—<\/span>/);
  assert.doesNotMatch(html, /SPREAD<\/span><span[^>]*>[+-]?\$0\.00/);
  assert.doesNotMatch(html, />PnL —/);
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

test("options card collapses legs into Σ Long / Σ Short rows and hides instruments", () => {
  const optPos: OpenPositionDto = {
    ...position,
    key: "options_jelly_roll|s2",
    strategyId: "options_jelly_roll",
    symbol: "BTC",
    venues: [
      { venue: "deribit", symbol: "BTC-30OCT26-91000-C", netQty: 0.1 },
      { venue: "deribit", symbol: "BTC-30OCT26-92000-P", netQty: 0.1 },
      { venue: "deribit", symbol: "BTC-30SEP26-91000-P", netQty: -0.1 },
      { venue: "deribit", symbol: "BTC-30SEP26-92000-C", netQty: -0.1 },
      { venue: "deribit", symbol: "BTC-30OCT26-91000-P", netQty: 0.1 },
      { venue: "deribit", symbol: "BTC-30SEP26-92000-P", netQty: -0.1 },
      { venue: "deribit", symbol: "BTC-30OCT26-92000-C", netQty: 0.1 },
      { venue: "deribit", symbol: "BTC-30SEP26-91000-C", netQty: -0.1 },
    ],
  };
  const html = renderToStaticMarkup(
    React.createElement(PositionsPanel, { ...baseProps, positions: [optPos] }),
  );
  // Venue named on the left of each aggregate row, bare signed totals on
  // the right — no "@venue" suffix and no Σ Long/Σ Short labels.
  assert.match(html, /deribit/);
  assert.match(html, />JELLY ROLL</);
  assert.doesNotMatch(html, /@deribit/);
  assert.doesNotMatch(html, />Σ Long<|>Σ Short</);
  // Two aggregate rows only — no strike-level instrument names on the card.
  assert.doesNotMatch(html, /30OCT26-91000-C/);
  assert.doesNotMatch(html, /more legs/);
});

test("position card has right-click context menu hint", () => {
  const html = renderToStaticMarkup(React.createElement(PositionsPanel, baseProps));
  assert.match(html, /Left-click: position details · Right-click: Reduce-Only \/ Clear actions/);
});

test("position card renders Reduce-Only badge when the DTO flag is set", () => {
  // The durable flag rides the backend DTO — no client-side storage.
  const flagged: OpenPositionDto = { ...position, reduceOnly: true };
  const html = renderToStaticMarkup(
    React.createElement(PositionsPanel, { ...baseProps, positions: [flagged] }),
  );
  assert.match(html, /Reduce-Only/);
  assert.match(html, /No new step increases; position closes on target profit/);
});

test("options position card renders simple current Net Profit without Exp/MtM", () => {
  const optionsPos: OpenPositionDto = {
    ...position,
    key: "options_box_spread|box1",
    strategyId: "options_box_spread",
    expectedNetUsd: 375.84,
    unrealizedPnlUsd: -673.07,
  };
  const html = renderToStaticMarkup(
    React.createElement(PositionsPanel, {
      ...baseProps,
      positions: [optionsPos],
    }),
  );
  assert.match(html, />BOX</);
  assert.match(html, /[-−]\$673\.07/);
  assert.doesNotMatch(html, /Exp:/);
  assert.doesNotMatch(html, /MtM:/);
});

test("position cards render SPREAD for radar and FUNDING RATE for funding", () => {
  const radarPos: OpenPositionDto = {
    ...position,
    key: "radar|rad1",
    strategyId: "radar",
  };
  const fundingPos: OpenPositionDto = {
    ...position,
    key: "funding|fund1",
    strategyId: "funding",
  };
  const html = renderToStaticMarkup(
    React.createElement(PositionsPanel, {
      ...baseProps,
      positions: [radarPos, fundingPos],
    }),
  );
  assert.match(html, />SPREAD</);
  assert.match(html, />FUNDING RATE</);
  assert.doesNotMatch(html, />RADAR</);
});

test("PositionsPanel renders Stopped rather than Paused when engine is stopped after emergency exit", () => {
  const stoppedStatus: SystemStatusDto = {
    is_live_mode: false,
    environment: "testnet",
    is_paused: true,
    is_stopped: true,
    total_realized_pnl: 0,
    total_trades_count: 0,
    successful_trades_count: 0,
    circuit_breaker_status: "ACTIVE",
    circuit_breaker_tripped: false,
    current_drawdown_usd: 0,
    max_drawdown_usd: 1000,
    consecutive_failures: 0,
    active_venues: [],
  };
  const html = renderToStaticMarkup(
    React.createElement(PositionsPanel, {
      ...baseProps,
      status: stoppedStatus,
      positions: [],
    }),
  );
  assert.match(html, /<span>Stopped<\/span>/, "badge must render Stopped");
  assert.doesNotMatch(html, /<span>Paused<\/span>/, "badge must never render Paused when stopped");
});
