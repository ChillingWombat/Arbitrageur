import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { OpportunityTrendGraph } from "./OpportunityTrendGraph.tsx";
import { SignalDetailModal } from "./OpportunityRadar.tsx";
import type { MonitorRowDto } from "../types/index.ts";

const createMockRow = (overrides?: Partial<MonitorRowDto>): MonitorRowDto => ({
  symbol: "BTC-USDC",
  buyVenue: "binance",
  sellVenue: "hyperliquid",
  status: "ready",
  currentEdgeBps: 18.5,
  baselineBps: 2.0,
  deviationBps: 14.5,
  netCaptureBps: 12.0,
  pWin: 0.85,
  winProbability: 0.85,
  expectedProfitBps: 10.2,
  expectedProfitUsd: 15.3,
  optimalSizeUsd: 1500,
  breakevenSizeUsd: 3000,
  fundingCarryBps: 0.5,
  fundingUnknown: false,
  firstSeenMs: 1774000000000,
  lastSeenMs: 1774000600000,
  signalAgeSecs: 600,
  expectedConvergeSecs: 420,
  qualificationProgress: 1.0,
  observedEpisodes: 120,
  executable: true,
  blockReason: null,
  thresholdRatio: 0.95,
  coverage: 0.99,
  sessionId: "asia",
  sessionProvisional: false,
  sessionAllow: 1.0,
  sessionSizeMult: 1.0,
  ...overrides,
});

test("OpportunityTrendGraph renders summary pills with baseline, dev, target zone, horizon, and end time", () => {
  const row = createMockRow({
    baselineBps: 1.5,
    deviationBps: 12.0,
    expectedConvergeSecs: 420,
    signalAgeSecs: 300,
  });

  const html = renderToStaticMarkup(
    React.createElement(OpportunityTrendGraph, { row })
  );

  // Summary pills
  assert.match(html, /Baseline:/);
  assert.match(html, /\+1\.5 bps/);
  assert.match(html, /Current Dev:/);
  assert.match(html, /\+12\.0 bps/);
  assert.match(html, /Target Zone:/);
  assert.match(html, /\[-1\.0, \+4\.0\] bps/);
  assert.match(html, /Convergence Horizon:/);
  assert.match(html, /~7m left/);
  assert.match(html, /Expected End Time:/);
});

test("OpportunityTrendGraph renders SVG with baseline, target band, now marker, convergence line, and legend", () => {
  const row = createMockRow({
    buyVenue: "binance",
    sellVenue: "hyperliquid",
    baselineBps: 0.0,
    deviationBps: 15.0,
    expectedConvergeSecs: 300,
  });

  const html = renderToStaticMarkup(
    React.createElement(OpportunityTrendGraph, { row })
  );

  // SVG elements
  assert.match(html, /<svg[^>]*aria-label="Opportunity Spread Trend &amp; Convergence Graph"/);
  // Target zone band
  assert.match(html, /Target Zone \(±2\.5 bps\)/);
  // Baseline guideline
  assert.match(html, /Baseline \(\+0\.0 bps\)/);
  // Now marker
  assert.match(html, />Now</);
  assert.match(html, /\+15\.0 bps/);
  // Convergence marker
  assert.match(html, /in ~5m \(End\)/);
  // Legend
  assert.match(html, /Historical spread/);
  assert.match(html, /Projected OU reversion/);
  assert.match(html, /Target zone \(±2\.5 bps\)/);
  assert.match(html, /Route: binance → hyperliquid/);
});

test("OpportunityTrendGraph gracefully handles null fields and negative deviation", () => {
  const row = createMockRow({
    baselineBps: null,
    deviationBps: -8.4,
    signalAgeSecs: null,
    expectedConvergeSecs: null,
    lastSeenMs: null,
  });

  const html = renderToStaticMarkup(
    React.createElement(OpportunityTrendGraph, { row })
  );

  // Defaults baseline to 0.0
  assert.match(html, /\+0\.0 bps/);
  // Negative deviation renders loss styling and sign
  assert.match(html, /-8\.4 bps/);
  assert.match(html, /bg-loss-dim/);
  // Fallback convergence horizon
  assert.match(html, /Convergence Horizon:/);
  assert.match(html, /~7m left/);
});

test("SignalDetailModal integrates OpportunityTrendGraph under Spread Trend & Convergence Horizon", () => {
  const row = createMockRow({
    symbol: "ETH-USDC",
    baselineBps: 1.0,
    deviationBps: 8.5,
  });

  const html = renderToStaticMarkup(
    React.createElement(SignalDetailModal, {
      row,
      held: false,
      sizeValue: 1000,
      executing: false,
      onSizeChange: () => {},
      onExecute: () => {},
      onClose: () => {},
    })
  );

  assert.match(html, /Spread Trend &amp; Convergence Horizon/);
  assert.match(html, /data-testid="opportunity-trend-graph"/);
  assert.match(html, />Now</);
  assert.match(html, /Target Zone \(±2\.5 bps\)/);
});

