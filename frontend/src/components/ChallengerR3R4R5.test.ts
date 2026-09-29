import nodeTest from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { resolveBotStatus, BotPhaseBadge } from "./BotLifecycleControls.tsx";
import type { StrategyBotStatus } from "./BotLifecycleControls.tsx";
import type { BotPhase } from "../hooks/useBotLifecycle.ts";
import { OpportunityRadar } from "./OpportunityRadar.tsx";
import { FundingRadar } from "./FundingRadar.tsx";
import { LiveMatrix } from "./LiveMatrix.tsx";
import { TriangularArbitrage } from "./TriangularArbitrage.tsx";
import { PredictionHub } from "./PredictionHub.tsx";
import { OptionsArbitrage } from "./OptionsArbitrage.tsx";

const INTERACTIVE_CHILD = process.argv.includes("--interactive-child");
const test = INTERACTIVE_CHILD ? (((() => {}) as unknown) as typeof nodeTest) : nodeTest;

const VALID_STATUSES: StrategyBotStatus[] = ["Active", "Paused", "Reducing", "Stopped"];
const noop = () => {};
const asyncNoop = async () => {};

// ============================================================================
// Requirement R4: Standardized Status Badge Matrix & Integrity
// ============================================================================

test("R4: resolveBotStatus strictly returns one of the 4 standardized statuses", () => {
  const testMatrix: Array<[BotPhase | undefined, boolean, StrategyBotStatus]> = [
    ["running", false, "Active"],
    ["running", true, "Paused"],
    ["paused", false, "Paused"],
    ["paused", true, "Paused"],
    ["reduce_only", false, "Reducing"],
    ["reduce_only", true, "Reducing"],
    ["stopping", false, "Reducing"],
    ["stopping", true, "Reducing"],
    ["idle", false, "Stopped"],
    ["idle", true, "Stopped"],
    [undefined, false, "Stopped"],
    [undefined, true, "Stopped"],
  ];

  for (const [phase, isPaused, expected] of testMatrix) {
    const actual = resolveBotStatus(phase, isPaused);
    assert.equal(
      actual,
      expected,
      `Failed for phase=${phase}, isPaused=${isPaused}: expected ${expected}, got ${actual}`
    );
    assert.ok(
      VALID_STATUSES.includes(actual),
      `Status "${actual}" is not in valid status list: ${VALID_STATUSES.join(", ")}`
    );
  }
});

test("R4: BotPhaseBadge never renders the word 'Engine' in content or tooltip", () => {
  for (const status of VALID_STATUSES) {
    const html = renderToStaticMarkup(React.createElement(BotPhaseBadge, { status }));
    assert.match(html, new RegExp(`>${status}</span>`));
    assert.match(html, new RegExp(`title="Strategy bot status: ${status}"`));
    assert.doesNotMatch(html, /Engine/i);
  }

  for (const phase of ["running", "paused", "reduce_only", "stopping", "idle"] as BotPhase[]) {
    for (const isPaused of [false, true]) {
      const html = renderToStaticMarkup(React.createElement(BotPhaseBadge, { phase, isPaused }));
      assert.doesNotMatch(html, /Engine/i);
      const matched = VALID_STATUSES.some((s) => html.includes(`>${s}</span>`));
      assert.ok(matched, `Expected one of [Active, Paused, Reducing, Stopped] for phase=${phase}, isPaused=${isPaused}`);
    }
  }
});

test("R4: All 6 strategy headers render standardized status badge immediately right of title before parameter tags", () => {
  // 1. OpportunityRadar
  const htmlOpp = renderToStaticMarkup(
    React.createElement(OpportunityRadar, { executionLogs: [], isPaused: false })
  );
  assert.match(
    htmlOpp,
    /<span[^>]*>Strategy Status<\/span>\s*<span[^>]*title="Strategy bot status: (Active|Paused|Reducing|Stopped)"[^>]*>[\s\S]*?<\/span>\s*<div[^>]*title="[^"]*directional trade win probability/
  );
  assert.doesNotMatch(htmlOpp, /Engine Paused/);

  // 2. FundingRadar
  const htmlFunding = renderToStaticMarkup(
    React.createElement(FundingRadar, { opportunities: [], tradeSize: 1000, loading: false, isPaused: false })
  );
  assert.match(
    htmlFunding,
    /<span[^>]*>Strategy Status<\/span>\s*<span[^>]*title="Strategy bot status: (Active|Paused|Reducing|Stopped)"[^>]*>[\s\S]*?<\/span>\s*<div[^>]*class="ribbon-pill"[^>]*title="Notional per leg/
  );
  assert.doesNotMatch(htmlFunding, /Engine Paused/);

  // 3. LiveMatrix
  const htmlMatrix = renderToStaticMarkup(
    React.createElement(LiveMatrix, {
      symbol: null,
      orderbooks: [],
      opportunities: [],
      onRefresh: noop,
      onExecuteTrade: async () => {},
      loading: false,
      isPaused: false,
    })
  );
  assert.match(
    htmlMatrix,
    /<span[^>]*>Strategy Status<\/span>\s*<span[^>]*title="Strategy bot status: (Active|Paused|Reducing|Stopped)"[^>]*>[\s\S]*?<\/span>/
  );
  assert.doesNotMatch(htmlMatrix, /Engine Paused/);

  // 4. TriangularArbitrage
  const htmlTri = renderToStaticMarkup(
    React.createElement(TriangularArbitrage, { executionLogs: [], onRefreshAll: asyncNoop, isPaused: false })
  );
  assert.match(
    htmlTri,
    /<span[^>]*>Strategy Status<\/span>\s*<span[^>]*title="Strategy bot status: (Active|Paused|Reducing|Stopped)"[^>]*>[\s\S]*?<\/span>\s*<div[^>]*class="ribbon-pill"[^>]*title="Notional per 3-leg cycle/
  );
  assert.doesNotMatch(htmlTri, /Engine Paused/);

  // 5. PredictionHub
  const htmlPred = renderToStaticMarkup(
    React.createElement(PredictionHub, {
      opportunities: [],
      onSearch: noop,
      isPaused: false,
      capability: { strategy_id: "prediction", status: "scaffold", can_scan: false, can_execute: false, reason: "scaffold" } as any,
    })
  );
  assert.match(
    htmlPred,
    /<span[^>]*>Strategy Status<\/span>\s*<span[^>]*title="Strategy bot status: (Active|Paused|Reducing|Stopped)"[^>]*>[\s\S]*?<\/span>\s*<div[^>]*class="ribbon-pill"[^>]*title="Minimum net profit/
  );
  assert.doesNotMatch(htmlPred, /Engine Paused/);

  // 6. OptionsArbitrage
  const htmlOpt = renderToStaticMarkup(
    React.createElement(OptionsArbitrage, {
      executionLogs: [],
      onRefreshAll: asyncNoop,
      optionsVenueAvailable: true,
      isPaused: false,
    })
  );
  assert.match(
    htmlOpt,
    /<span[^>]*>Options Strategies<\/span>\s*<span[^>]*title="Strategy bot status: (Active|Paused|Reducing|Stopped)"[^>]*>[\s\S]*?<\/span>/
  );
  assert.doesNotMatch(htmlOpt, /Engine Paused/);
});

test("R4: All 6 strategy headers handle isPaused=true without rendering 'Engine Paused' badges", () => {
  const configs: Array<{ name: string; html: string }> = [
    {
      name: "OpportunityRadar",
      html: renderToStaticMarkup(
        React.createElement(OpportunityRadar, { executionLogs: [], isPaused: true })
      ),
    },
    {
      name: "FundingRadar",
      html: renderToStaticMarkup(
        React.createElement(FundingRadar, { opportunities: [], tradeSize: 1000, loading: false, isPaused: true })
      ),
    },
    {
      name: "LiveMatrix",
      html: renderToStaticMarkup(
        React.createElement(LiveMatrix, {
          symbol: null,
          orderbooks: [],
          opportunities: [],
          onRefresh: noop,
          onExecuteTrade: async () => {},
          loading: false,
          isPaused: true,
        })
      ),
    },
    {
      name: "TriangularArbitrage",
      html: renderToStaticMarkup(
        React.createElement(TriangularArbitrage, { executionLogs: [], onRefreshAll: asyncNoop, isPaused: true })
      ),
    },
    {
      name: "PredictionHub",
      html: renderToStaticMarkup(
        React.createElement(PredictionHub, {
          opportunities: [],
          onSearch: noop,
          isPaused: true,
          capability: { strategy_id: "prediction", status: "scaffold", can_scan: false, can_execute: false, reason: "scaffold" } as any,
        })
      ),
    },
    {
      name: "OptionsArbitrage",
      html: renderToStaticMarkup(
        React.createElement(OptionsArbitrage, {
          executionLogs: [],
          onRefreshAll: asyncNoop,
          optionsVenueAvailable: true,
          isPaused: true,
        })
      ),
    },
  ];

  for (const { name, html } of configs) {
    assert.doesNotMatch(
      html,
      /Engine Paused/,
      `${name} erroneously rendered 'Engine Paused' when isPaused=true`
    );
    assert.doesNotMatch(
      html,
      /badge-status-warn/,
      `${name} erroneously rendered obsolete badge-status-warn tag`
    );
  }
});

// ============================================================================
// Requirement R3: Fixed Window/Dock Height (384px) & No Inner Scrollbars
// ============================================================================

test("R3: OpportunityRadar dock container has locked h-96 without inner max-h-96 wrappers", () => {
  const html = renderToStaticMarkup(
    React.createElement(OpportunityRadar, { executionLogs: [], isPaused: false })
  );
  // Outer container is strictly locked to h-96 (384px) with overflow-y-auto
  assert.match(
    html,
    /class="p-3\.5 h-96 overflow-y-auto"/,
    "OpportunityRadar dock container must have 'p-3.5 h-96 overflow-y-auto'"
  );
  // Redundant inner max-h-96 wrappers eliminated to prevent double scrollbars
  assert.doesNotMatch(
    html,
    /max-h-96/,
    "OpportunityRadar contains forbidden inner max-h-96 wrapper"
  );
});

test("R3: FundingRadar dock container has locked h-96 without inner max-h-96 wrappers", () => {
  const html = renderToStaticMarkup(
    React.createElement(FundingRadar, { opportunities: [], tradeSize: 1000, loading: false, isPaused: false })
  );
  assert.match(
    html,
    /class="p-3\.5 h-96 overflow-y-auto"/,
    "FundingRadar dock container must have 'p-3.5 h-96 overflow-y-auto'"
  );
  assert.doesNotMatch(
    html,
    /max-h-96/,
    "FundingRadar contains forbidden inner max-h-96 wrapper"
  );
});

test("R3: LiveMatrix dock container has locked h-96", () => {
  const html = renderToStaticMarkup(
    React.createElement(LiveMatrix, {
      symbol: null,
      orderbooks: [],
      opportunities: [],
      onRefresh: noop,
      onExecuteTrade: async () => {},
      loading: false,
      isPaused: false,
    })
  );
  assert.match(
    html,
    /class="p-3\.5 h-96 overflow-y-auto"/,
    "LiveMatrix dock container must have 'p-3.5 h-96 overflow-y-auto'"
  );
  assert.doesNotMatch(
    html,
    /max-h-96/,
    "LiveMatrix contains forbidden inner max-h-96 wrapper"
  );
});

// ============================================================================
// Requirement R5: Options Arbitrage Ribbon & 5th Metric Card
// ============================================================================

test("R5: OptionsArbitrage header row is single row without flex-wrap", () => {
  const html = renderToStaticMarkup(
    React.createElement(OptionsArbitrage, {
      executionLogs: [],
      onRefreshAll: asyncNoop,
      optionsVenueAvailable: true,
      isPaused: false,
    })
  );

  // Outer ribbon header row has no flex-wrap
  assert.match(
    html,
    /class="shrink-0 px-4 py-3 flex items-center justify-between gap-3 bg-surface rounded-xl border border-surface-border shadow-2xs select-none"/
  );
  assert.doesNotMatch(
    html,
    /shrink-0 px-4 py-3[^"]*flex-wrap/
  );

  // Left side has no flex-wrap
  assert.match(
    html,
    /class="flex items-center gap-2 text-xs font-mono min-w-0"/
  );

  // Right side has no flex-wrap
  assert.match(
    html,
    /class="flex items-center space-x-2 shrink-0"/
  );
});

test("R5: OptionsArbitrage ribbon completely omits 'Premium Outlay' pill", () => {
  const html = renderToStaticMarkup(
    React.createElement(OptionsArbitrage, {
      executionLogs: [],
      onRefreshAll: asyncNoop,
      optionsVenueAvailable: true,
      isPaused: false,
    })
  );

  // Extract the header ribbon specifically:
  const match = html.match(/<div class="shrink-0 px-4 py-3 flex items-center justify-between gap-3[^"]*"[^>]*>([\s\S]*?)<\/div>\s*<div class="grid/);
  assert.ok(match, "Header ribbon div must exist");
  assert.doesNotMatch(
    match[1],
    /Premium Outlay/,
    "Header ribbon must not contain 'Premium Outlay'"
  );
});

test("R5: OptionsArbitrage renders 5th metric card for Premium Outlay in 5-column grid", () => {
  const html = renderToStaticMarkup(
    React.createElement(OptionsArbitrage, {
      executionLogs: [],
      onRefreshAll: asyncNoop,
      optionsVenueAvailable: true,
      isPaused: false,
    })
  );

  // Metric grid uses 5 columns on lg breakpoint
  assert.match(
    html,
    /class="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-4"/
  );

  // 5th metric card for Premium Outlay
  assert.match(
    html,
    /<div[^>]*title="Option premiums paid minus received at entry — financing held inside open packages, returned at close\/expiry\. Kept out of Net Profit\."/
  );
  assert.match(html, /<div class="text-xs font-medium text-muted font-mono">Premium Outlay<\/div>/);
  assert.match(html, /<div class="text-xs text-slate-500 mt-0\.5">net financing in packages<\/div>/);

  // Count number of metric card titles in the grid
  const cardTitles = html.match(/<div class="text-xs font-medium text-muted font-mono">([^<]+)<\/div>/g);
  assert.ok(cardTitles);
  assert.equal(cardTitles.length, 5, `Expected 5 metric cards, found ${cardTitles.length}`);
  assert.match(cardTitles[0], /Scanned Setups/);
  assert.match(cardTitles[1], /Best Net Edge/);
  assert.match(cardTitles[2], /Executable/);
  assert.match(cardTitles[3], /Realized PnL/);
  assert.match(cardTitles[4], /Premium Outlay/);
});

// ============================================================================
// Interactive DOM Tab Switching Stress Test (R3)
// ============================================================================

test("R3 interactive: tab switching across all 4 dock tabs in OpportunityRadar & FundingRadar (child process)", async (t) => {
  await t.test("dock tab switching maintains fixed 384px height without inner scrollbars", () => {
    const { execFileSync } = require("node:child_process");
    const out = execFileSync(
      process.execPath,
      [__filename, "--interactive-child"],
      { encoding: "utf8", timeout: 30000 }
    );
    assert.match(out, /R3-DOCK-SWITCHING-OK/);
  });
});

// Child-process runner executed under happy-dom
declare const __filename: string;

async function runInteractiveChild() {
  const { Window } = await import("happy-dom");
  const { createRoot } = await import("react-dom/client");
  const { act } = await import("react");

  const window = new Window();
  for (const key of [
    "document", "window", "HTMLElement", "HTMLButtonElement",
    "Element", "Node", "MouseEvent", "CustomEvent", "Event",
    "SVGElement", "DocumentFragment", "MutationObserver", "getComputedStyle",
    "requestAnimationFrame", "cancelAnimationFrame",
  ]) {
    (globalThis as Record<string, unknown>)[key] =
      (window as unknown as Record<string, unknown>)[key] ??
      (globalThis as Record<string, unknown>)[key];
  }
  (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;

  // Stub API calls
  const { api } = await import("../services/api.ts");
  (api as Record<string, unknown>).getMonitorSnapshot = async () => ({
    generatedTsMs: Date.now(),
    running: true,
    rows: [],
    watchlist: [],
    venues: [],
    exempted: [],
  });
  (api as Record<string, unknown>).getRadarBotStatus = async () => null;
  (api as Record<string, unknown>).listSignalExecutions = async () => [];
  (api as Record<string, unknown>).getStrategyFillPnl = async () => 0;
  (api as Record<string, unknown>).getMonitorConfig = async () => ({ minWinProbability: 0.6 });
  (api as Record<string, unknown>).getRadarBotConfig = async () => ({ enabled: false, mode: "off" });
  (api as Record<string, unknown>).listFixedYieldMaturities = async () => [];
  (api as Record<string, unknown>).listTradedOpportunities = async () => [];

  const container = window.document.createElement("div");
  window.document.body.appendChild(container);
  const root = createRoot(container as unknown as HTMLElement);

  // 1. Test OpportunityRadar interactive tab switching
  await act(async () => {
    root.render(
      React.createElement(OpportunityRadar, {
        executionLogs: [
          {
            id: "log-1",
            symbol: "BTC-USDC",
            timestamp: new Date().toISOString(),
            status: "filled",
            action: "buy",
            price: 50000,
            quantity: 1,
            strategy_id: "radar",
            details: "Sample log",
            venue_a: "hyperliquid",
            venue_b: "lighter",
            direction: "convergence",
          } as any,
        ],
        isPaused: false,
      })
    );
  });

  // Verify dock container in OpportunityRadar
  let dockContainer = container.querySelector(".h-96.overflow-y-auto");
  assert.ok(dockContainer, "OpportunityRadar dock container with h-96 overflow-y-auto must exist");
  assert.ok(dockContainer.className.includes("p-3.5"));

  // Check buttons for dock tabs
  const tabButtons = Array.from(container.querySelectorAll("button")).filter(
    (b) =>
      b.textContent?.includes("Current Positions") ||
      b.textContent?.includes("Execution History") ||
      b.textContent?.includes("Opportunity History") ||
      b.textContent?.includes("Log Monitor")
  );
  assert.equal(tabButtons.length, 4, `Expected 4 dock tab buttons, found ${tabButtons.length}`);

  // Switch tabs:
  // Tab 1: Execution History
  const historyBtn = tabButtons.find((b) => b.textContent?.includes("Execution History"));
  assert.ok(historyBtn);
  await act(async () => {
    historyBtn.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true }));
  });
  dockContainer = container.querySelector(".h-96.overflow-y-auto");
  assert.ok(dockContainer, "Dock container must maintain h-96 overflow-y-auto on Execution History tab");
  assert.equal(dockContainer.querySelectorAll(".max-h-96").length, 0, "No inner max-h-96 on Execution History tab");

  // Tab 2: Opportunity History
  const oppsBtn = tabButtons.find((b) => b.textContent?.includes("Opportunity History"));
  assert.ok(oppsBtn);
  await act(async () => {
    oppsBtn.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true }));
  });
  dockContainer = container.querySelector(".h-96.overflow-y-auto");
  assert.ok(dockContainer, "Dock container must maintain h-96 overflow-y-auto on Opportunity History tab");
  assert.equal(dockContainer.querySelectorAll(".max-h-96").length, 0, "No inner max-h-96 on Opportunity History tab");

  // Tab 3: Log Monitor
  const logsBtn = tabButtons.find((b) => b.textContent?.includes("Log Monitor"));
  assert.ok(logsBtn);
  await act(async () => {
    logsBtn.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true }));
  });
  dockContainer = container.querySelector(".h-96.overflow-y-auto");
  assert.ok(dockContainer, "Dock container must maintain h-96 overflow-y-auto on Log Monitor tab");
  assert.equal(dockContainer.querySelectorAll(".max-h-96").length, 0, "No inner max-h-96 on Log Monitor tab");

  // Tab 0: Back to Current Positions
  const posBtn = tabButtons.find((b) => b.textContent?.includes("Current Positions"));
  assert.ok(posBtn);
  await act(async () => {
    posBtn.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true }));
  });
  dockContainer = container.querySelector(".h-96.overflow-y-auto");
  assert.ok(dockContainer, "Dock container must maintain h-96 overflow-y-auto on Current Positions tab");
  assert.equal(dockContainer.querySelectorAll(".max-h-96").length, 0, "No inner max-h-96 on Current Positions tab");

  // 2. Test FundingRadar interactive tab switching
  await act(async () => {
    root.render(
      React.createElement(FundingRadar, {
        opportunities: [],
        tradeSize: 1000,
        loading: false,
        isPaused: false,
      })
    );
  });

  dockContainer = container.querySelector(".h-96.overflow-y-auto");
  assert.ok(dockContainer, "FundingRadar dock container with h-96 overflow-y-auto must exist");

  const fundingTabButtons = Array.from(container.querySelectorAll("button")).filter(
    (b) =>
      b.textContent?.includes("Current Position") ||
      b.textContent?.includes("Execution History") ||
      b.textContent?.includes("Opportunity History") ||
      b.textContent?.includes("Log Monitor")
  );
  assert.equal(fundingTabButtons.length, 4, `Expected 4 FundingRadar dock tab buttons, found ${fundingTabButtons.length}`);

  for (const tabBtn of fundingTabButtons) {
    await act(async () => {
      tabBtn.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true }));
    });
    dockContainer = container.querySelector(".h-96.overflow-y-auto");
    assert.ok(dockContainer, `Dock container must maintain h-96 overflow-y-auto on tab ${tabBtn.textContent}`);
    assert.equal(dockContainer.querySelectorAll(".max-h-96").length, 0, `No inner max-h-96 on tab ${tabBtn.textContent}`);
  }

  await act(async () => root.unmount());
  process.stdout.write("R3-DOCK-SWITCHING-OK\n");
  process.exit(0);
}

if (INTERACTIVE_CHILD) {
  runInteractiveChild().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}
