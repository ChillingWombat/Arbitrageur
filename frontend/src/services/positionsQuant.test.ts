import test from "node:test";
import assert from "node:assert/strict";
import {
  scopeAndSortPositions,
  getReduceOnlyPositions,
  isPositionReduceOnly,
  setPositionReduceOnly,
} from "./positionsQuant.ts";
import type { OpenPositionDto } from "../types/index.ts";

const pos = (
  key: string,
  strategyId: string,
  symbol: string,
  pnl: number | null,
  notional: number | null,
): OpenPositionDto => ({
  key,
  strategyId,
  symbol,
  signalCount: 1,
  venues: [
    { venue: "hyperliquid", netQty: 1 },
    { venue: "lighter", netQty: -1 },
  ],
  expectedNetUsd: null,
  unrealizedPnlUsd: pnl,
  feesUsd: 0,
  notionalUsd: notional,
  openedAt: "2026-01-01T00:00:00Z",
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

test("reduce-only state management persists symbols and handles window environment", () => {
  // Mock window and localStorage
  const storage = new Map<string, string>();
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
    assert.equal(isPositionReduceOnly("BTC-USDC"), false);
    assert.deepEqual(getReduceOnlyPositions(), []);

    setPositionReduceOnly("BTC-USDC", true);
    assert.equal(isPositionReduceOnly("BTC-USDC"), true);
    assert.equal(isPositionReduceOnly("btc-usdc"), true);
    assert.deepEqual(getReduceOnlyPositions(), ["BTC-USDC"]);

    setPositionReduceOnly("ETH-USDC", true);
    assert.equal(isPositionReduceOnly("ETH-USDC"), true);
    assert.equal(getReduceOnlyPositions().length, 2);

    setPositionReduceOnly("BTC-USDC", false);
    assert.equal(isPositionReduceOnly("BTC-USDC"), false);
    assert.equal(isPositionReduceOnly("ETH-USDC"), true);
  } finally {
    if (origWindow === undefined) {
      delete (globalThis as any).window;
    } else {
      (globalThis as any).window = origWindow;
    }
  }
});

