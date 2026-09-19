// Unit tests for external-position grouping (node:test style, matching repo).
import test from "node:test";
import assert from "node:assert/strict";
import { groupExternalPositions } from "./externalPositions.ts";
import type { ExternalPositionDto } from "../types/index.ts";

const mk = (over: Partial<ExternalPositionDto>): ExternalPositionDto => ({
  key: "k", venue: "hyperliquid", symbol: "BTC-USDC", base: "BTC", side: "long",
  quantity: 1, entryPrice: 100, markPrice: 110, notionalUsd: 110,
  unrealizedPnlUsd: 10, liquidationPrice: null, liquidationDistancePct: 50,
  leverage: 1, systemQty: 0, externalQty: 1, closeSupported: true,
  updatedAt: "2026-09-19T00:00:00Z", ...over,
});

test("groups by ticker across venues with signed net", () => {
  const ps = [
    mk({ venue: "hyperliquid", side: "long", notionalUsd: 100, unrealizedPnlUsd: 5 }),
    mk({ venue: "lighter", side: "short", notionalUsd: 40, unrealizedPnlUsd: -2 }),
  ];
  const g = groupExternalPositions(ps, "ticker");
  assert.equal(g.length, 1);
  assert.equal(g[0].netNotionalUsd, 60);
  assert.equal(g[0].grossNotionalUsd, 140);
  assert.equal(g[0].totalUnrealizedUsd, 3);
});

test("groups by venue and flags unsupported legs", () => {
  const ps = [
    mk({ venue: "hyperliquid", base: "BTC", notionalUsd: 100 }),
    mk({ venue: "lighter", base: "ETH", symbol: "ETH-USDC", notionalUsd: 50, closeSupported: false }),
  ];
  const g = groupExternalPositions(ps, "venue");
  assert.equal(g.length, 2);
  assert.equal(g[0].key, "hyperliquid"); // biggest gross first
  assert.equal(g[1].hasUnsupported, true);
  assert.equal(g[0].hasUnsupported, false);
});

test("empty input yields no groups", () => {
  assert.deepEqual(groupExternalPositions([], "ticker"), []);
});
