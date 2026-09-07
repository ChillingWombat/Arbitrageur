import { test } from "node:test";
import assert from "node:assert/strict";
import {
  getMinCompatibleSize,
  computeSafeLeverage,
  normalizeVenueKey,
} from "./contractSpecs.ts";

test("normalizeVenueKey correctly normalizes variations", () => {
  assert.equal(normalizeVenueKey("Hyperliquid (L1)"), "hyperliquid");
  assert.equal(normalizeVenueKey("HL"), "hyperliquid");
  assert.equal(normalizeVenueKey("Lighter (zkDEX)"), "lighter");
  assert.equal(normalizeVenueKey("Vanta.exchange"), "vanta");
  assert.equal(normalizeVenueKey("Robinhood EVM"), "robinhood");
});

test("getMinCompatibleSize finds maximum minimum lot size across venues for BTC", () => {
  const res = getMinCompatibleSize("BTC-USDC", ["Hyperliquid", "Lighter", "Vanta", "Robinhood"], 64200);
  // HL: 0.0001, LGT: 0.001, VNT: 0.001, RH: 0.005 -> Max is 0.005
  assert.equal(res.minCompatibleSize, 0.005);
  assert.ok(res.recommendedUnitSize >= res.minCompatibleSize);
  assert.equal(res.stepSize, 0.001);
});

test("getMinCompatibleSize works for ETH across venues", () => {
  const res = getMinCompatibleSize("ETH-USDC", ["Hyperliquid", "Lighter"], 3450);
  // HL: 0.001, LGT: 0.01 -> Max is 0.01
  assert.equal(res.minCompatibleSize, 0.01);
  assert.ok(res.recommendedUnitSize >= res.minCompatibleSize);
});

test("computeSafeLeverage calculates minimum allowable leverage and margin", () => {
  // HL has 50x, LGT has 20x, RH has 10x
  const res = computeSafeLeverage("BTC-USDC", ["Hyperliquid", "Lighter", "Robinhood"], 64200, 0.1, 15);
  // Minimum venue leverage is 10x from Robinhood. Target was 15x, so effective is min(10, 15) = 10x.
  assert.equal(res.effectiveLeverage, 10);
  assert.equal(res.maxVenueLeverage, 10);
  assert.equal(res.notionalUsd, 6420);
  assert.equal(res.marginPerLegUsd, 642);
  assert.equal(res.totalMarginRequiredUsd, 1284);
  assert.ok(res.liquidationBufferPct >= 8.0 && res.liquidationBufferPct <= 10.0);
});

test("computeSafeLeverage respects user lower leverage target", () => {
  // HL has 50x, LGT has 20x. User targets 5x.
  const res = computeSafeLeverage("BTC-USDC", ["Hyperliquid", "Lighter"], 64200, 0.05, 5);
  assert.equal(res.effectiveLeverage, 5);
  assert.equal(res.maxVenueLeverage, 20);
  assert.equal(res.notionalUsd, 3210);
  assert.equal(res.marginPerLegUsd, 642);
  assert.equal(res.totalMarginRequiredUsd, 1284);
});
