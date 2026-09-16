import { test } from "node:test";
import assert from "node:assert/strict";
import {
  getMinCompatibleSize,
  computeSafeLeverage,
  normalizeVenueKey,
  deriveStepSizing,
  resolveTotalPaperEquity,
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

test("deriveStepSizing multiplier binds below the USD cap", () => {
  // HL min 0.0001 / LGT min 0.001 -> compat 0.001, step 0.001.
  const res = deriveStepSizing("BTC-USDC", ["Hyperliquid", "Lighter"], 64200, 600_000, {
    stepSizeMult: 10,
    maxStepUsd: 2500,
    maxTickerEquityPct: 2,
  });
  assert.equal(res.minCompatBase, 0.001);
  assert.equal(res.stepBaseQty, 0.01);
  assert.equal(res.stepUsd, 642);
  // floor(600000 * 2% / 642) = floor(12000 / 642) = 18
  assert.equal(res.maxSteps, 18);
});

test("deriveStepSizing USD cap binds above the multiplier lot", () => {
  const res = deriveStepSizing("BTC-USDC", ["Hyperliquid", "Lighter"], 64200, 600_000, {
    stepSizeMult: 10,
    maxStepUsd: 100,
    maxTickerEquityPct: 2,
  });
  // min(0.01, 100/64200 = 0.0015576) quantized UP to step 0.001 -> 0.002
  assert.equal(res.stepBaseQty, 0.002);
  assert.ok(Math.abs(res.stepUsd - 128.4) < 1e-9);
  assert.equal(res.maxSteps, Math.floor(12000 / res.stepUsd));
});

test("deriveStepSizing never drops below the compatible min lot", () => {
  const res = deriveStepSizing(
    "BTC-USDC",
    ["Hyperliquid", "Lighter", "Vanta", "Robinhood"],
    64200,
    600_000,
    { stepSizeMult: 10, maxStepUsd: 1, maxTickerEquityPct: 2 }
  );
  // RH min 0.005 dominates; the $1 cap would price 0.0000156 BTC.
  assert.equal(res.minCompatBase, 0.005);
  assert.equal(res.stepBaseQty, 0.005);
  assert.ok(res.stepBaseQty >= res.minCompatBase);
});

test("deriveStepSizing maxSteps math floors to a minimum of 1 on known equity", () => {
  const params = { stepSizeMult: 10, maxStepUsd: 2500, maxTickerEquityPct: 2 };
  const full = deriveStepSizing("BTC-USDC", ["Hyperliquid", "Lighter"], 64200, 600_000, params);
  assert.equal(full.maxSteps, 18);
  const tiny = deriveStepSizing("BTC-USDC", ["Hyperliquid", "Lighter"], 64200, 100, params);
  assert.equal(tiny.maxSteps, 1);
});

test("deriveStepSizing maxSteps fails closed (0 steps) on unknown equity", () => {
  const params = { stepSizeMult: 10, maxStepUsd: 2500, maxTickerEquityPct: 2 };
  const noEquity = deriveStepSizing("BTC-USDC", ["Hyperliquid", "Lighter"], 64200, 0, params);
  assert.equal(noEquity.maxSteps, 0);
  const nullEquity = deriveStepSizing("BTC-USDC", ["Hyperliquid", "Lighter"], 64200, null, params);
  assert.equal(nullEquity.maxSteps, 0);
  const noPrice = deriveStepSizing("BTC-USDC", ["Hyperliquid", "Lighter"], 0, 600_000, params);
  assert.equal(noPrice.maxSteps, 0);
  assert.ok(noPrice.stepBaseQty >= noPrice.minCompatBase);
});

test("resolveTotalPaperEquity returns null instead of a fabricated seed", () => {
  assert.equal(resolveTotalPaperEquity([]), null);
  assert.equal(resolveTotalPaperEquity([{ equity_usd: NaN }]), null);
  assert.equal(resolveTotalPaperEquity([{ equity_usd: 0 }]), null);
  assert.equal(resolveTotalPaperEquity([{ equity_usd: 100_000 }, { equity_usd: 200_000 }]), 300_000);
});

test("deriveStepSizing session multipliers default to neutral and scale size", () => {
  const base = { stepSizeMult: 10, maxStepUsd: 2500, maxTickerEquityPct: 2 };
  const plain = deriveStepSizing("BTC-USDC", ["Hyperliquid", "Lighter"], 64200, 600_000, base);
  assert.equal(plain.stepBaseQty, 0.01);
  const neutral = deriveStepSizing("BTC-USDC", ["Hyperliquid", "Lighter"], 64200, 600_000, {
    ...base,
    sessionAllow: 1,
    sessionInvVol: 1,
    sessionDepth: 1,
  });
  assert.equal(neutral.stepBaseQty, plain.stepBaseQty);
  const unknown = deriveStepSizing("BTC-USDC", ["Hyperliquid", "Lighter"], 64200, 600_000, {
    ...base,
    sessionAllow: NaN,
    sessionInvVol: undefined,
  });
  assert.equal(unknown.stepBaseQty, plain.stepBaseQty);
  const asia = deriveStepSizing("BTC-USDC", ["Hyperliquid", "Lighter"], 64200, 600_000, {
    ...base,
    sessionAllow: 0.5,
  });
  assert.equal(asia.stepBaseQty, 0.005);
  assert.ok(asia.stepBaseQty >= asia.minCompatBase);
  const masked = deriveStepSizing("BTC-USDC", ["Hyperliquid", "Lighter"], 64200, 600_000, {
    ...base,
    sessionAllow: 0,
  });
  assert.equal(masked.stepBaseQty, 0);
  assert.equal(masked.stepUsd, 0);
});
