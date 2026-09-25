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
