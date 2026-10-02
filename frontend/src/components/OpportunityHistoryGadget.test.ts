import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  OpportunityHistoryGadget,
  UntradedOpportunityModal,
  parseEconomics,
  fmtTs,
  pnlTone,
  type UnifiedOpportunityItem,
} from "./OpportunityHistoryGadget.tsx";
import type {
  TradedOpportunityDto,
  FundingOpportunityDto,
  ExecutionLogEntry,
} from "../types/index.ts";

const sampleTradedOpp = (over: Partial<TradedOpportunityDto> = {}): TradedOpportunityDto => ({
  signalId: "sig-test-01",
  strategyId: "futures",
  symbol: "BTC-USDC",
  environment: "testnet",
  status: "closed",
  routeId: null,
  expectedNetUsd: 15.2,
  note: null,
  venueLong: "hyperliquid",
  venueShort: "lighter",
  createdAt: "2026-10-02T10:00:00Z",
  closedAt: "2026-10-02T10:05:00Z",
  lotId: "lot-1",
  lotStatus: "closed",
  entrySpreadBps: 18.5,
  closeSpreadBps: 2.1,
  realizedPnlUsd: 14.8,
  realizedPnlBps: 16.4,
  legNetFlowUsd: 14.8,
  totalFeesUsd: 0.4,
  childCount: 1,
  transactions: [],
  ...over,
});

const sampleFundingOpp = (over: Partial<FundingOpportunityDto> = {}): FundingOpportunityDto => ({
  id: "fund-test-01",
  symbol: "ETH-USDC",
  long_venue: "hyperliquid",
  short_venue: "lighter",
  long_venue_id: "hyperliquid",
  short_venue_id: "lighter",
  long_symbol: "ETH-USDC",
  short_symbol: "ETH-USDC",
  long_market_type: "perpetual",
  short_market_type: "perpetual",
  net_rate_8h_pct: 0.012,
  net_apr_pct: 13.14,
  net_apy_pct: 14.05,
  round_trip_fee_bps: 10.0,
  break_even_hours: 6.8,
  estimated_daily_income_usd: 5.2,
  trade_size_usd: 1000,
  persistence_probability: 0.88,
  ...over,
});

const sampleBlockedLog = (over: Partial<ExecutionLogEntry> = {}): ExecutionLogEntry => ({
  id: "log-01",
  timestamp: "2026-10-02T10:00:00Z",
  trade_type: "Funding",
  strategy_id: "funding",
  symbol: "SOL-USDC",
  venue_a: "lighter",
  venue_b: "hyperliquid",
  direction: "Long Lighter / Short Hyperliquid",
  leg_a_status: "blocked",
  leg_b_status: "blocked",
  success: false,
  gross_profit: 0,
  fees: 0,
  net_profit: 0,
  latency_us: 100,
  note: "Execution blocked: Live funding execution is not available",
  ...over,
});

const sampleRadarFailedLog = (over: Partial<ExecutionLogEntry> = {}): ExecutionLogEntry => ({
  id: "rad-log-01",
  timestamp: "2026-10-02T10:02:00Z",
  trade_type: "Spread",
  strategy_id: "radar",
  symbol: "AVAX-USDC",
  venue_a: "hyperliquid",
  venue_b: "lighter",
  direction: "Convergence Entry",
  trigger_spread_bps: 24.5,
  leg_a_status: "failed",
  leg_b_status: "blocked",
  success: false,
  gross_profit: 0,
  fees: 0,
  net_profit: 0,
  latency_us: 120,
  note: "Execution failed: Margin requirement exceeded on lighter",
  ...over,
});

test("OpportunityHistoryGadget renders switch buttons for All, Traded, and Not Traded", () => {
  const html = renderToStaticMarkup(
    React.createElement(OpportunityHistoryGadget, {
      strategy: "futures",
      tradedOpps: [sampleTradedOpp()],
    })
  );

  // Switch buttons must exist
  assert.match(html, /<span>All<\/span>/);
  assert.match(html, /<span>Traded<\/span>/);
  assert.match(html, /<span>Not Traded<\/span>/);
});

test("OpportunityHistoryGadget switch button counters use valid Tailwind spacing py-px (no py-0.2)", () => {
  const html = renderToStaticMarkup(
    React.createElement(OpportunityHistoryGadget, {
      strategy: "futures",
      tradedOpps: [sampleTradedOpp()],
    })
  );

  assert.doesNotMatch(html, /py-0\.2/, "Must not contain invalid Tailwind spacing step py-0.2");
  assert.match(html, /py-px rounded-full text-\[10px\]/);
});

test("OpportunityHistoryGadget NEVER renders 'All tickers' or 'This Ticker' button", () => {
  const htmlFutures = renderToStaticMarkup(
    React.createElement(OpportunityHistoryGadget, {
      strategy: "futures",
      tradedOpps: [sampleTradedOpp()],
    })
  );

  assert.doesNotMatch(htmlFutures, /All tickers/i, "Must not contain 'All tickers' button");
  assert.doesNotMatch(htmlFutures, /This ticker/i, "Must not contain 'This ticker' button");

  const htmlFunding = renderToStaticMarkup(
    React.createElement(OpportunityHistoryGadget, {
      strategy: "funding",
      fundingOpps: [sampleFundingOpp()],
    })
  );

  assert.doesNotMatch(htmlFunding, /All tickers/i, "Must not contain 'All tickers' button in funding mode");
  assert.doesNotMatch(htmlFunding, /This ticker/i, "Must not contain 'This ticker' button in funding mode");
});

test("OpportunityHistoryGadget displays Traded opportunities with correct badges and realized PnL", () => {
  const html = renderToStaticMarkup(
    React.createElement(OpportunityHistoryGadget, {
      strategy: "futures",
      tradedOpps: [sampleTradedOpp({ symbol: "BTC-USDC", realizedPnlUsd: 14.8 })],
    })
  );

  assert.match(html, />BTC-USDC<\/td>/);
  assert.match(html, />TRADED<\/span>/);
  assert.match(html, />CLOSED<\/span>/);
  assert.match(html, /class="font-semibold text-profit">\+\$14\.80/);
});

test("OpportunityHistoryGadget colors negative realized PnL as text-loss and NEVER text-profit", () => {
  const html = renderToStaticMarkup(
    React.createElement(OpportunityHistoryGadget, {
      strategy: "futures",
      tradedOpps: [sampleTradedOpp({ symbol: "ETH-USDC", realizedPnlUsd: -5.5, realizedPnlBps: -6.2 })],
    })
  );

  assert.match(html, />ETH-USDC<\/td>/);
  assert.match(html, /-\$5\.50/);
  assert.match(html, /class="font-semibold text-loss">-\$5\.50/);
  assert.doesNotMatch(html, /class="font-semibold text-profit">-\$5\.50/);
});

test("OpportunityHistoryGadget displays Funding Rate opportunities with Net APR and 8h rate", () => {
  const html = renderToStaticMarkup(
    React.createElement(OpportunityHistoryGadget, {
      strategy: "funding",
      fundingOpps: [sampleFundingOpp({ symbol: "ETH-USDC", net_apr_pct: 13.14 })],
      executionLogs: [sampleBlockedLog({ symbol: "SOL-USDC" })],
    })
  );

  assert.match(html, />ETH-USDC<\/td>/);
  assert.match(html, /\+13\.14% APR/);
  assert.match(html, />SOL-USDC<\/td>/);
  assert.match(html, />UNTRADED<\/span>/);
  assert.match(html, />BLOCKED<\/span>/);
  assert.match(html, /Execution blocked/);
});

test("OpportunityHistoryGadget surfaces Spread Radar execution failure logs from radarLogs", () => {
  const html = renderToStaticMarkup(
    React.createElement(OpportunityHistoryGadget, {
      strategy: "futures",
      radarLogs: [sampleRadarFailedLog()],
    })
  );

  assert.match(html, />AVAX-USDC<\/td>/);
  assert.match(html, />UNTRADED<\/span>/);
  assert.match(html, />BLOCKED<\/span>/);
  assert.match(html, /Margin requirement exceeded on lighter/);
});

test("OpportunityHistoryGadget accepts strategy='radar' and normalizes to futures", () => {
  const html = renderToStaticMarkup(
    React.createElement(OpportunityHistoryGadget, {
      strategy: "radar",
      tradedOpps: [sampleTradedOpp({ strategyId: "radar", symbol: "LINK-USDC" })],
    })
  );

  assert.match(html, />LINK-USDC<\/td>/);
  assert.match(html, />TRADED<\/span>/);
});

test("parseEconomics parses valid JSON correctly", () => {
  const jsonStr = JSON.stringify({
    skipReason: "entry rejected: friction too high",
    pWin: 0.82,
    deviationBps: 15.4,
  });

  const parsed = parseEconomics(jsonStr);
  assert.ok(parsed);
  assert.equal(parsed?.skipReason, "entry rejected: friction too high");
  assert.equal(parsed?.pWin, 0.82);
  assert.equal(parsed?.deviationBps, 15.4);
});

test("pnlTone assigns text-profit, text-loss, and text-slate-600 honestly", () => {
  assert.equal(pnlTone(10.5), "text-profit");
  assert.equal(pnlTone(-4.2), "text-loss");
  assert.equal(pnlTone(0), "text-slate-600");
  assert.equal(pnlTone(null), "text-slate-600");
  assert.equal(pnlTone(undefined), "text-slate-600");
  assert.equal(pnlTone(0.001), "text-slate-600");
});

test("UntradedOpportunityModal renders skip reason, route, and economics snapshot", () => {
  const item: UnifiedOpportunityItem = {
    id: "skip-123",
    kind: "futures",
    tradeStatus: "not_traded",
    timestamp: Date.now(),
    symbol: "BTC-USDC",
    venueBuy: "hyperliquid",
    venueSell: "lighter",
    edgeDisplay: "+22.5 bps",
    edgeValue: 22.5,
    subEdgeDisplay: "Dev: +18.2 bps",
    winProbDisplay: "85%",
    statusLabel: "SKIPPED",
    statusBadgeCls: "bg-warn-dim text-warn-strong border-warn-border",
    reasonOrOutcome: "entry rejected: Irreconcilable wire grids",
    rawJson: JSON.stringify({
      skipReason: "entry rejected: Irreconcilable wire grids",
      pWin: 0.85,
      deviationBps: 18.2,
      baselineBps: -4.3,
      sizeUsd: 250,
      optimalSizeUsd: 5000,
    }),
  };

  const html = renderToStaticMarkup(
    React.createElement(UntradedOpportunityModal, {
      item,
      onClose: () => {},
    })
  );

  assert.match(html, /Reason Not Traded \/ Blocked/);
  assert.match(html, /entry rejected: Irreconcilable wire grids/);
  assert.match(html, /Spread Radar Belief Snapshot/);
  assert.match(html, /\+22\.5 bps/);
  assert.match(html, /85%/);
  assert.match(html, /\$5,000/);
});

test("UntradedOpportunityModal renders funding carry route with long ↔ short", () => {
  const item: UnifiedOpportunityItem = {
    id: "fund-modal-01",
    kind: "funding",
    tradeStatus: "not_traded",
    timestamp: Date.now(),
    symbol: "ETH-USDC",
    venueBuy: "hyperliquid",
    venueSell: "lighter",
    edgeDisplay: "+14.2% APR",
    statusLabel: "ACTIVE SCAN",
    statusBadgeCls: "bg-profit-dim text-profit border-profit-border",
    reasonOrOutcome: "Exp net: $10.50 (4.5h b/e)",
    fundingOppDetail: sampleFundingOpp(),
  };

  const html = renderToStaticMarkup(
    React.createElement(UntradedOpportunityModal, {
      item,
      onClose: () => {},
    })
  );

  assert.match(html, /Funding Carry Snapshot/);
  assert.match(html, /long ↔/);
  assert.match(html, /short/);
});

test("OpportunityHistoryGadget displays honest empty state when no opportunities exist", () => {
  const html = renderToStaticMarkup(
    React.createElement(OpportunityHistoryGadget, {
      strategy: "futures",
      tradedOpps: [],
    })
  );

  assert.match(html, /No opportunities recorded yet/);
});

test("TickerOpportunityHistory backward-compatible wrapper renders unified gadget without 'This ticker'", () => {
  const { TickerOpportunityHistory } = require("./TickerOpportunityHistory.tsx");
  const html = renderToStaticMarkup(
    React.createElement(TickerOpportunityHistory, { symbol: "" })
  );

  assert.match(html, /<span>All<\/span>/);
  assert.match(html, /<span>Traded<\/span>/);
  assert.match(html, /<span>Not Traded<\/span>/);
  assert.doesNotMatch(html, /All tickers/i);
  assert.doesNotMatch(html, /This ticker/i);
});

test("fmtTs handles invalid, zero, and valid timestamps", () => {
  assert.equal(fmtTs(0), "—");
  assert.equal(fmtTs(-100), "—");
  assert.equal(fmtTs(NaN), "—");
  assert.ok(fmtTs(Date.now()).length > 0);
});
