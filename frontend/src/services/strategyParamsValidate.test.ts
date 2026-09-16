import { test } from "node:test";
import assert from "node:assert/strict";
import { normalizeStrategyParams } from "./api.ts";
import {
  parseOptThreshold,
  validateStrategyParams,
} from "./strategyParamsValidate.ts";

test("normalizeStrategyParams defaults the per-strategy slippage and funding guards", () => {
  const p = normalizeStrategyParams(null);
  assert.equal(p.futuresMaxSlippageBps, 15.0);
  assert.equal(p.fundingMaxSlippageBps, 15.0);
  assert.equal(p.predictionMaxSlippageBps, 15.0);
  assert.equal(p.fundingMaxEntrySpreadBps, 50.0);
  assert.equal(p.fundingAutoExitSpreadPct, null);
  assert.equal(p.fundingLiqTrimPct, null);
  assert.equal(p.fundingAutoRebalance, false);
  assert.equal(p.fundingRotationThresholdBps, null);
  assert.equal(p.futuresLiqTrimPct, null);
  assert.equal(p.spotLiqTrimPct, null);
  assert.equal(p.predictionLiqTrimPct, null);
});

test("normalizeStrategyParams maps snake_case guard fields and preserves each trim block", () => {
  const p = normalizeStrategyParams({
    strategyId: "funding",
    futures_max_slippage_bps: 12.5,
    funding_max_slippage_bps: 20.0,
    prediction_max_slippage_bps: 30.0,
    funding_max_entry_spread_bps: 40.0,
    funding_auto_exit_spread_pct: 5.0,
    funding_liq_trim_pct: 15.0,
    funding_auto_rebalance: true,
    funding_rotation_threshold_bps: 10.0,
    futures_liq_trim_pct: 12.5,
    spot_liq_trim_pct: 8.0,
    prediction_liq_trim_pct: 9.0,
  });
  assert.equal(p.futuresMaxSlippageBps, 12.5);
  assert.equal(p.fundingMaxSlippageBps, 20.0);
  assert.equal(p.predictionMaxSlippageBps, 30.0);
  assert.equal(p.fundingMaxEntrySpreadBps, 40.0);
  assert.equal(p.fundingAutoExitSpreadPct, 5.0);
  assert.equal(p.fundingLiqTrimPct, 15.0);
  assert.equal(p.fundingAutoRebalance, true);
  assert.equal(p.fundingRotationThresholdBps, 10.0);
  assert.equal(p.futuresLiqTrimPct, 12.5);
  assert.equal(p.spotLiqTrimPct, 8.0);
  assert.equal(p.predictionLiqTrimPct, 9.0);
});

test("normalizeStrategyParams defaults the passive unwind blocks", () => {
  const p = normalizeStrategyParams(null);
  for (const field of [
    "futuresPassiveUnwind",
    "fundingPassiveUnwind",
    "spotPassiveUnwind",
  ] as const) {
    assert.deepEqual(p[field], {
      enabled: false,
      spreadPercentile: 50,
      timeoutSecs: 10,
      maxTries: 3,
      maxSpreadBps: 15,
      maxFastVolBps: 10,
    });
  }
  const legacy = normalizeStrategyParams({ strategyId: "spot" });
  assert.equal(legacy.spotPassiveUnwind?.enabled, false);
});

test("validateStrategyParams accepts default passive unwind blocks", () => {
  assert.deepEqual(validateStrategyParams(normalizeStrategyParams(null)), []);
});

test("validateStrategyParams rejects bad passive unwind fields", () => {
  const base = normalizeStrategyParams(null);
  const badPct = {
    ...base,
    futuresPassiveUnwind: { ...base.futuresPassiveUnwind!, spreadPercentile: 101 },
  };
  assert.deepEqual(validateStrategyParams(badPct), [
    "futuresPassiveUnwind: spreadPercentile must be within [0, 100]",
  ]);
  const badTries = {
    ...base,
    spotPassiveUnwind: { ...base.spotPassiveUnwind!, maxTries: 11 },
  };
  assert.deepEqual(validateStrategyParams(badTries), [
    "spotPassiveUnwind: maxTries must be within 1..=10",
  ]);
  const zeroTimeoutEnabled = {
    ...base,
    fundingPassiveUnwind: { ...base.fundingPassiveUnwind!, enabled: true, timeoutSecs: 0 },
  };
  assert.deepEqual(validateStrategyParams(zeroTimeoutEnabled), [
    "fundingPassiveUnwind: timeoutSecs must be within 0..=300 (at least 1 when enabled)",
  ]);
});

test("validateStrategyParams rejects nonpositive per-strategy slippage", () => {
  for (const field of [
    "futuresMaxSlippageBps",
    "fundingMaxSlippageBps",
    "predictionMaxSlippageBps",
  ] as const) {
    const bad = { ...normalizeStrategyParams(null), [field]: 0 };
    assert.deepEqual(validateStrategyParams(bad), [`${field} must be finite and positive`]);
    const nan = { ...normalizeStrategyParams(null), [field]: Number.NaN };
    assert.deepEqual(validateStrategyParams(nan), [`${field} must be finite and positive`]);
  }
});

test("validateStrategyParams rejects bad funding entry spread", () => {
  const bad = { ...normalizeStrategyParams(null), fundingMaxEntrySpreadBps: -1 };
  assert.deepEqual(validateStrategyParams(bad), [
    "fundingMaxEntrySpreadBps must be finite and positive",
  ]);
});

test("validateStrategyParams enforces the 0..100 window on optional percents", () => {
  for (const field of [
    "fundingAutoExitSpreadPct",
    "fundingLiqTrimPct",
    "futuresLiqTrimPct",
    "spotLiqTrimPct",
    "predictionLiqTrimPct",
  ] as const) {
    const zero = { ...normalizeStrategyParams(null), [field]: 0 };
    assert.deepEqual(validateStrategyParams(zero), [`${field} must be within (0, 100] when set`]);
    const over = { ...normalizeStrategyParams(null), [field]: 100.5 };
    assert.deepEqual(validateStrategyParams(over), [`${field} must be within (0, 100] when set`]);
    const ok = { ...normalizeStrategyParams(null), [field]: 5 };
    assert.deepEqual(validateStrategyParams(ok), []);
    const off = { ...normalizeStrategyParams(null), [field]: null };
    assert.deepEqual(validateStrategyParams(off), []);
  }
});

test("validateStrategyParams rejects nonpositive rotation threshold when set", () => {
  const bad = { ...normalizeStrategyParams(null), fundingRotationThresholdBps: 0 };
  assert.deepEqual(validateStrategyParams(bad), [
    "fundingRotationThresholdBps must be finite and positive",
  ]);
  const off = { ...normalizeStrategyParams(null), fundingRotationThresholdBps: null };
  assert.deepEqual(validateStrategyParams(off), []);
});

test("parseOptThreshold treats empty/nonpositive as off", () => {
  assert.equal(parseOptThreshold(""), null);
  assert.equal(parseOptThreshold("0"), null);
  assert.equal(parseOptThreshold("-3"), null);
  assert.equal(parseOptThreshold("abc"), null);
  assert.equal(parseOptThreshold("5"), 5);
  assert.equal(parseOptThreshold("12.5"), 12.5);
});
