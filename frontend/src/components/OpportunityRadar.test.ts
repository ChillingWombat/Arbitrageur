import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { OpportunityRadar } from "./OpportunityRadar.tsx";

test("OpportunityRadar: bottom dock container enforces fixed height across tabs without inner max-h-96", () => {
  const html = renderToStaticMarkup(
    React.createElement(OpportunityRadar, {
      executionLogs: [],
      isPaused: false,
    })
  );
  assert.match(html, /class="p-3\.5 h-96 overflow-y-auto"/);
  // Redundant inner wrappers removed
  assert.doesNotMatch(html, /<div class="max-h-96 overflow-y-auto">/);
});

test("OpportunityRadar renders standardized status badge immediately right of Strategy Status", () => {
  const html = renderToStaticMarkup(
    React.createElement(OpportunityRadar, {
      executionLogs: [],
      isPaused: false,
    })
  );
  assert.match(
    html,
    /Strategy Status<\/span>[\s\S]*?title="Strategy bot status: Stopped"[\s\S]*?>Stopped<\/span>/
  );
  const statusBadge = html.match(/title="Strategy bot status: [^"]*"[^>]*>[\s\S]*?<\/span>/);
  assert.ok(statusBadge);
  assert.doesNotMatch(statusBadge[0], /Engine/i);
});

test("OpportunityRadar status badge handles isPaused and never renders Engine Paused", () => {
  const htmlPaused = renderToStaticMarkup(
    React.createElement(OpportunityRadar, {
      executionLogs: [],
      isPaused: true,
    })
  );
  assert.match(htmlPaused, /Strategy Status/);
  assert.match(htmlPaused, /title="Strategy bot status: Stopped"/);
  const statusBadge = htmlPaused.match(/title="Strategy bot status: [^"]*"[^>]*>[\s\S]*?<\/span>/);
  assert.ok(statusBadge);
  assert.doesNotMatch(statusBadge[0], /Engine/i);
  assert.doesNotMatch(htmlPaused, /Engine Paused/);
  assert.doesNotMatch(htmlPaused, /badge-status-warn/);
});

test("OpportunityRadar renders Open Positions table rows with cursor-pointer, hover styling and detail tooltip", () => {
  const html = renderToStaticMarkup(
    React.createElement(OpportunityRadar, {
      executionLogs: [],
      isPaused: false,
      initialBotStatus: {
        enabled: true,
        mode: "armed",
        openPositions: [
          {
            symbol: "BTC-USDC",
            buyVenue: "hyperliquid",
            sellVenue: "lighter",
            sizeUsd: 1500,
            entryDevBps: 25.5,
            baselineBps: 0.0,
            liveDevBps: 10.2,
            entryTsMs: Date.now() - 60_000,
            deadlineMs: Date.now() + 120_000,
            unwatched: false,
            liveStatus: "active",
          },
        ],
        entersToday: 1,
        exitsToday: 0,
        skipsToday: 0,
        lastDecision: null,
        lastError: null,
        auditErrors: 0,
        monitorRunning: true,
        snapshotPresent: true,
      },
    })
  );

  assert.match(
    html,
    /<tr[^>]*class="hover:bg-surface-raised\/60 transition cursor-pointer"[^>]*title="Click to view position details, legs, and PnL trend"/,
    "Open Positions table rows must have cursor-pointer, hover styling and detail tooltip"
  );
  assert.match(html, />BTC-USDC<\/td>/);
});

test("OpportunityRadar renders PositionDetailModal when a position detail is selected", () => {
  const html = renderToStaticMarkup(
    React.createElement(OpportunityRadar, {
      executionLogs: [],
      isPaused: false,
      initialSelectedDetailPos: {
        key: "radar|BTC-USDC-HL-LGT",
        tradeId: "trade-btc-1",
        strategyId: "radar",
        symbol: "BTC-USDC",
        legSymbols: ["BTC-USDC"],
        signalCount: 2,
        venues: [
          { venue: "hyperliquid", symbol: "BTC-USDC", netQty: 0.1 },
          { venue: "lighter", symbol: "BTC-USDC", netQty: -0.1 },
        ],
        expectedNetUsd: 15.0,
        unrealizedPnlUsd: 8.5,
        feesUsd: 1.2,
        notionalUsd: 6500,
        openedAt: "2026-10-01T12:00:00Z",
        reduceOnly: false,
      },
    })
  );

  assert.match(html, /role="dialog"/);
  assert.match(html, /aria-label="Position details — BTC-USDC"/);
  assert.match(html, />BTC-USDC<\/span>/);
});

