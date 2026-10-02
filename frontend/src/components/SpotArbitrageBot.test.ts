import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { SpotArbitrageBot } from "./SpotArbitrageBot.tsx";
import type { StrategyCapabilityDto } from "../types/index.ts";

const noop = async () => {};

const params = {
  tradeSizeUsd: 1000,
  minNetSpreadBps: 15,
  maxSlippageBps: 5,
};

const render = (capability?: StrategyCapabilityDto) =>
  renderToStaticMarkup(
    React.createElement(SpotArbitrageBot, {
      executionLogs: [],
      onRefreshAll: noop,
      params,
      capability,
    }),
  );

test("renders the live page when the backend reports spot ready", () => {
  const html = render({
    strategyId: "spot",
    scanReady: true,
    executeReady: true,
    reason: null,
  });
  assert.match(html, /Filter Asset/);
  assert.match(html, /No Opportunity Detected/);
  assert.doesNotMatch(html, /Spot Arbitrage Unavailable/);
});

test("gates the page off when the backend reports spot unwired", () => {
  const html = render({
    strategyId: "spot",
    scanReady: false,
    executeReady: false,
    reason: "stub",
  });
  assert.match(html, /Spot Arbitrage Unavailable/);
  assert.doesNotMatch(html, /Filter Asset/);
});

test("fail-closed on absent capability descriptor", () => {
  const html = render(undefined);
  assert.match(html, /Spot Arbitrage Unavailable/);
});

test("renders bottom dock container with 4 tabs and fixed height", () => {
  const html = render({
    strategyId: "spot",
    scanReady: true,
    executeReady: true,
    reason: null,
  });
  assert.match(html, /Open Positions/);
  assert.match(html, /Execution History/);
  assert.match(html, /Opportunity History/);
  assert.match(html, /Log Monitor/);
  assert.match(html, /class="p-3\.5 h-96 overflow-y-auto"/);
});

test("SpotArbitrageBot renders standardized status badge and passes isPaused", () => {
  const htmlActive = renderToStaticMarkup(
    React.createElement(SpotArbitrageBot, {
      executionLogs: [],
      onRefreshAll: noop,
      params,
      isPaused: false,
      capability: {
        strategyId: "spot",
        scanReady: true,
        executeReady: true,
        reason: null,
      },
    }),
  );
  assert.match(htmlActive, /Strategy Status/);
  assert.match(htmlActive, /title="Strategy bot status: Stopped"/);

  const htmlPaused = renderToStaticMarkup(
    React.createElement(SpotArbitrageBot, {
      executionLogs: [],
      onRefreshAll: noop,
      params,
      isPaused: true,
      capability: {
        strategyId: "spot",
        scanReady: true,
        executeReady: true,
        reason: null,
      },
    }),
  );
  assert.match(htmlPaused, /title="Strategy bot status: Stopped"/);
  assert.doesNotMatch(htmlPaused, /Engine Paused/);
});

test("SpotArbitrageBot renders Opportunity Scanner header on left and right-aligned filter controls", () => {
  const html = render({
    strategyId: "spot",
    scanReady: true,
    executeReady: true,
    reason: null,
  });
  assert.match(
    html,
    /<h3 class="text-\[13px\] font-bold font-sans tracking-wider text-slate-900">Opportunity Scanner<\/h3>/
  );
  assert.match(
    html,
    /class="p-3\.5 border-b border-surface-border flex items-center justify-between gap-3 bg-surface"/
  );
  assert.match(
    html,
    /<div class="flex items-center gap-3 text-xs font-sans">[\s\S]*?Search[\s\S]*?Filter Asset:[\s\S]*?opportunities detected/
  );
});

test("SpotArbitrageBot: asset dropdown is anchored to right edge (right-0)", async () => {
  const { readFileSync } = await import("node:fs");
  const { resolve } = await import("node:path");
  const filePath = resolve(process.cwd(), "src/components/SpotArbitrageBot.tsx");
  const source = readFileSync(filePath, "utf8");
  assert.match(source, /dropdown-menu right-0 top-full mt-1\.5 w-48 z-30/);
});
