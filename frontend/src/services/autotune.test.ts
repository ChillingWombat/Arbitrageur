import test from "node:test";
import assert from "node:assert/strict";
import {
  AUTOTUNE_PARAMS,
  normalizeAutotuneRecommendation,
  normalizeSessionProfile,
} from "./api.ts";

function fixtureRow(param: string, status: "READY" | "HELD", recommended: number | null) {
  return {
    param,
    current: 1.0,
    recommended,
    status,
    fit_metric: "fit",
    validated_metric: "val",
    n: 200,
    rationale: "why",
    formula_id: "f",
    confidence: "high",
  };
}

test("normalize keeps 7 canonical rows and READY/HELD invariants", () => {
  const rec = normalizeAutotuneRecommendation(
    {
      symbol: "BTC-USDC",
      buy_venue: "hyperliquid",
      sell_venue: "lighter",
      pair_observations: 400,
      rows: [
        fixtureRow("targetProfitBps", "READY", 12.34),
        fixtureRow("windowPercentile", "HELD", 80),
      ],
      banner: {
        fit_from_ms: 1,
        fit_to_ms: 2,
        validate_from_ms: 3,
        validate_to_ms: 4,
        regime_verdict: "STABLE",
        ready_count: 99,
        total: 99,
      },
    },
    "BTC-USDC",
  );
  assert.equal(rec.rows.length, 7);
  assert.deepEqual(rec.rows.map((r) => r.param), [...AUTOTUNE_PARAMS]);
  const target = rec.rows.find((r) => r.param === "targetProfitBps");
  assert.equal(target?.status, "READY");
  assert.equal(target?.recommended, 12.34);
  // HELD rows force recommended=null even when the backend sent a value.
  const pct = rec.rows.find((r) => r.param === "windowPercentile");
  assert.equal(pct?.status, "HELD");
  assert.equal(pct?.recommended, null);
  // Missing rows default to HELD with null recommendation.
  const window = rec.rows.find((r) => r.param === "slidingWindowSecs");
  assert.equal(window?.status, "HELD");
  assert.equal(window?.recommended, null);
  // Banner ready_count/total reflect the normalized rows.
  assert.equal(rec.banner.ready_count, 1);
  assert.equal(rec.banner.total, 7);
});

test("normalize is honest on empty/garbage input", () => {
  const rec = normalizeAutotuneRecommendation(null, "ETH-USDC");
  assert.equal(rec.symbol, "ETH-USDC");
  assert.equal(rec.rows.length, 7);
  assert.ok(rec.rows.every((r) => r.status === "HELD" && r.recommended === null));
  assert.equal(rec.banner.regime_verdict, "INSUFFICIENT");
  assert.equal(rec.banner.ready_count, 0);
  assert.equal(rec.pair_observations, 0);
});

test("normalize demotes READY rows with non-numeric recommendations to HELD", () => {
  const rec = normalizeAutotuneRecommendation(
    {
      symbol: "BTC-USDC",
      rows: [fixtureRow("maxStepUsd", "READY", Number.NaN)],
      banner: {},
    },
    "BTC-USDC",
  );
  const row = rec.rows.find((r) => r.param === "maxStepUsd");
  assert.equal(row?.status, "HELD");
  assert.equal(row?.recommended, null);
});

test("normalize carries session coupling scales with 1.0 fallback", () => {
  const coupled = normalizeAutotuneRecommendation(
    {
      symbol: "BTC-USDC",
      rows: [],
      banner: {},
      session_tau_scale: 1.2,
      session_sigma_scale: 0.9,
    },
    "BTC-USDC",
  );
  assert.equal(coupled.session_tau_scale, 1.2);
  assert.equal(coupled.session_sigma_scale, 0.9);
  const fallback = normalizeAutotuneRecommendation(
    { symbol: "BTC-USDC", rows: [], banner: {} },
    "BTC-USDC",
  );
  assert.equal(fallback.session_tau_scale, 1.0);
  assert.equal(fallback.session_sigma_scale, 1.0);
  const garbage = normalizeAutotuneRecommendation(
    { symbol: "BTC-USDC", rows: [], banner: {}, session_tau_scale: -3 },
    "BTC-USDC",
  );
  assert.equal(garbage.session_tau_scale, 1.0);
});

test("normalizeSessionProfile is fail-closed without promotion", () => {
  const p = normalizeSessionProfile(null, "BTC-USDC");
  assert.equal(p.symbol, "BTC-USDC");
  assert.equal(p.promotion, null);
  assert.equal(p.provisional, true);
  assert.deepEqual(p.sessions, []);
  const promoted = normalizeSessionProfile(
    {
      symbol: "BTC-USDC",
      currentSession: "overlap",
      isWeekend: false,
      inFundingMask: false,
      usCashOpen: true,
      sessionAllow: 1.0,
      provisional: false,
      sessions: [],
      promotion: {
        status: "promoted",
        fitDeltaBps: 2.0,
        valDeltaBps: 1.5,
        reason: "agree",
        harR2: 0.9,
        rescaled: true,
      },
    },
    "BTC-USDC",
  );
  assert.equal(promoted.promotion?.status, "promoted");
  assert.equal(promoted.promotion?.rescaled, true);
  assert.equal(promoted.promotion?.harR2, 0.9);
});
