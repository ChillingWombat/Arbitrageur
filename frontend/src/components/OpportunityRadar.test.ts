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

test("OpportunityRadar: strategy ribbon omits Execution Single-Way badge", () => {
  const html = renderToStaticMarkup(
    React.createElement(OpportunityRadar, {
      executionLogs: [],
      isPaused: false,
    })
  );
  assert.doesNotMatch(html, /title="Execution mode — edit via Configurator"/);
  assert.doesNotMatch(html, />Execution<\/span>/);
  assert.doesNotMatch(html, />Single-Way<\/span>/);
  assert.doesNotMatch(html, />Two-Way<\/span>/);
});

test("OpportunityRadar: renders Opportunity Scanner header on left and right-aligned filter controls", () => {
  const html = renderToStaticMarkup(
    React.createElement(OpportunityRadar, {
      executionLogs: [],
      isPaused: false,
    })
  );
  assert.match(
    html,
    /<h3 class="text-\[13px\] font-bold font-sans tracking-wider text-slate-900">Opportunity Scanner<\/h3>/
  );
  assert.match(
    html,
    /class="flex items-center justify-between gap-3 px-4 py-2\.5 border-b border-surface-border text-xs font-sans shrink-0 bg-surface-raised\/40"/
  );
  assert.match(
    html,
    /<div class="flex items-center gap-2 text-xs font-sans">[\s\S]*?Symbol[\s\S]*?Venue[\s\S]*?Min Exp[\s\S]*?Min Win/
  );
});

test("OpportunityRadar: venue dropdown is anchored to right edge (right-0)", async () => {
  const { readFileSync } = await import("node:fs");
  const { resolve } = await import("node:path");
  const filePath = resolve(process.cwd(), "src/components/OpportunityRadar.tsx");
  const source = readFileSync(filePath, "utf8");
  assert.match(source, /absolute right-0 top-full mt-1\.5 w-52 bg-surface/);
});

test("OpportunityRadar: blocked rows suppress profit emphasis and render blockReason", () => {
  const mockSnapshot = {
    generatedTsMs: Date.now(),
    running: true,
    rows: [
      {
        symbol: "BTC",
        buyVenue: "binance",
        sellVenue: "coinbase",
        status: "triggered",
        executable: false,
        blockReason: "coverage below 80% floor",
        expectedProfitBps: 12.5,
        expectedProfitUsd: 150.0,
        currentEdgeBps: 15.0,
        baselineBps: 2.0,
        deviationBps: 13.0,
        pWin: 0.95,
        winProbability: 0.95,
        sessionId: "overlap",
        sessionProvisional: false,
        sessionAllow: 1.0,
        sessionSizeMult: 1.0,
        fundingCarryBps: 0,
        fundingUnknown: false,
        firstSeenMs: Date.now() - 10000,
        lastSeenMs: Date.now(),
        signalAgeSecs: 10,
        expectedConvergeSecs: 300,
        qualificationProgress: 1.0,
        observedEpisodes: 20,
        thresholdRatio: 0.9,
        coverage: 0.75,
        optimalSizeUsd: 1000,
        breakevenSizeUsd: 2000,
      },
    ],
    watchlist: ["BTC"],
    venues: [],
    exempted: [],
  };

  const html = renderToStaticMarkup(
    React.createElement(OpportunityRadar, {
      executionLogs: [],
      isPaused: false,
      initialSnapshot: mockSnapshot as any,
    })
  );

  // Exp profit for blocked row should be muted (text-slate-400 font-normal) rather than bright green (text-profit)
  assert.match(
    html,
    /class="px-2 py-2 text-right font-mono tabular-nums text-slate-400 font-normal"[^>]*title="Blocked \(coverage below 80% floor\)\. Forecast expectation: 12\.5 bps"/
  );
  // Sizing for blocked row should also be muted
  assert.match(
    html,
    /class="px-2 py-2 text-right font-mono text-slate-400 font-normal"[^>]*title="Blocked \(coverage below 80% floor\)\. Forecast optimal size: \$1\.0k"/
  );
  // Block reason should be rendered inline
  assert.match(html, /coverage below 80% floor/);
  // Status badge should reflect blocked styling rather than false green
  assert.match(html, /bg-warn-dim text-warn-strong border-warn-border/);
  // Top edge in BotStatCards should NOT claim positive profit when zero rows are executable
  assert.doesNotMatch(html, /\+12\.5 bps max/);
  assert.match(html, /monitoring orderbooks/);
  // Scanner header reflects (0 executable / 1 monitored)
  assert.match(html, /\(0 executable \/ 1 monitored\)/);
  // Executable Only filter button is rendered
  assert.match(html, /Executable Only/);
});

test("OpportunityRadar: executable rows promote topEdgeBps and display executable count", () => {
  const mockExecutableSnapshot = {
    generatedTsMs: Date.now(),
    running: true,
    rows: [
      {
        symbol: "ETH",
        buyVenue: "hyperliquid",
        sellVenue: "lighter",
        status: "ready",
        executable: true,
        blockReason: null,
        expectedProfitBps: 18.2,
        expectedProfitUsd: 250.0,
        currentEdgeBps: 20.0,
        baselineBps: 1.0,
        deviationBps: 19.0,
        pWin: 0.96,
        winProbability: 0.96,
        sessionId: "asia",
        sessionProvisional: false,
        sessionAllow: 1.0,
        sessionSizeMult: 1.0,
        fundingCarryBps: 0,
        fundingUnknown: false,
        firstSeenMs: Date.now() - 5000,
        lastSeenMs: Date.now(),
        signalAgeSecs: 5,
        expectedConvergeSecs: 180,
        qualificationProgress: 1.0,
        observedEpisodes: 50,
        thresholdRatio: 0.95,
        coverage: 0.98,
        optimalSizeUsd: 5000,
        breakevenSizeUsd: 8000,
      },
    ],
    watchlist: ["ETH"],
    venues: [],
    exempted: [],
  };

  const html = renderToStaticMarkup(
    React.createElement(OpportunityRadar, {
      executionLogs: [],
      isPaused: false,
      initialSnapshot: mockExecutableSnapshot as any,
    })
  );

  // Executable row shows bright profit styling
  assert.match(html, /text-profit font-bold/);
  // Header shows (1 executable / 1 monitored)
  assert.match(html, /\(1 executable \/ 1 monitored\)/);
  // Top edge in BotStatCards shows the true executable edge
  assert.match(html, /\+18\.2 bps max/);
});

