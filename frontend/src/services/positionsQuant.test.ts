import test from "node:test";
import assert from "node:assert/strict";
import { scopeAndSortPositions } from "./positionsQuant.ts";
import type { OpenPositionDto } from "../types/index.ts";

const pos = (
  key: string,
  strategyId: string,
  symbol: string,
  pnl: number | null,
  notional: number | null,
): OpenPositionDto => ({
  key,
  tradeId: key.split("|")[1] ?? key,
  strategyId,
  symbol,
  legSymbols: [symbol],
  signalCount: 1,
  venues: [
    { venue: "hyperliquid", symbol, netQty: 1 },
    { venue: "lighter", symbol, netQty: -1 },
  ],
  expectedNetUsd: null,
  unrealizedPnlUsd: pnl,
  feesUsd: 0,
  notionalUsd: notional,
  openedAt: "2026-01-01T00:00:00Z",
  reduceOnly: false,
});

const fixture: OpenPositionDto[] = [
  pos("futures|BTC-USDC|hyperliquid+lighter", "futures", "BTC-USDC", 5, 100),
  pos("futures|BTC-USDC|arcus+hyperliquid", "futures", "BTC-USDC", -3, 50),
  pos("funding|ETH-USDC|hyperliquid+lighter", "funding", "ETH-USDC", null, 200),
];

test("scope 'all' returns every strategy's positions", () => {
  const out = scopeAndSortPositions(fixture, "all", "futures", "name");
  assert.equal(out.length, 3);
});

test("scope 'current' keeps only the tab's strategy", () => {
  const out = scopeAndSortPositions(fixture, "current", "funding", "name");
  assert.equal(out.length, 1);
  assert.equal(out[0].strategyId, "funding");
});

test("scope 'current' with no mapped strategy returns all (panel forces all anyway)", () => {
  const out = scopeAndSortPositions(fixture, "current", null, "name");
  assert.equal(out.length, 3);
});

test("name sort is deterministic on the grouping key", () => {
  const out = scopeAndSortPositions(fixture, "all", null, "name");
  assert.deepEqual(
    out.map((p) => p.key),
    [
      "funding|ETH-USDC|hyperliquid+lighter",
      "futures|BTC-USDC|arcus+hyperliquid",
      "futures|BTC-USDC|hyperliquid+lighter",
    ],
  );
});

test("pnl sort descends and pushes unavailable marks to the bottom", () => {
  const out = scopeAndSortPositions(fixture, "all", null, "pnl");
  assert.deepEqual(
    out.map((p) => p.unrealizedPnlUsd),
    [5, -3, null],
  );
});

test("size sort descends on entry notional with nulls last", () => {
  const out = scopeAndSortPositions(fixture, "all", null, "size");
  assert.deepEqual(
    out.map((p) => p.notionalUsd),
    [200, 100, 50],
  );
});

test("position fixtures carry the server-side reduce-only flag", () => {
  // The durable flag rides the backend DTO now — the client no longer keeps
  // a parallel localStorage set that manual entries could bypass.
  assert.equal(fixture.every((p) => p.reduceOnly === false), true);
});

