import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  AutotunePreviewModal,
  buildAutotuneCopyText,
} from "./AutotunePreviewModal.tsx";
import type { AutotuneRecommendationDto } from "../types/index.ts";

const rec: AutotuneRecommendationDto = {
  symbol: "BTC-USDC",
  buy_venue: "hyperliquid",
  sell_venue: "lighter",
  pair_observations: 400,
  rows: [
    {
      param: "targetProfitBps",
      current: 1.0,
      recommended: 12.34,
      status: "READY",
      fit_metric: "fit",
      validated_metric: "val",
      n: 200,
      rationale: "validated",
      formula_id: "target=friction+k*sigma+carry",
      confidence: "high",
    },
    {
      param: "userTargetLeverage",
      current: 5,
      recommended: null,
      status: "HELD",
      fit_metric: "fit",
      validated_metric: "val",
      n: 200,
      rationale: "kept manual",
      formula_id: "leverage=decrease-only",
      confidence: "low",
    },
  ],
  banner: {
    fit_from_ms: 1_700_000_000_000,
    fit_to_ms: 1_700_010_000_000,
    validate_from_ms: 1_700_010_000_000,
    validate_to_ms: 1_700_020_000_000,
    regime_verdict: "STABLE",
    ready_count: 1,
    total: 2,
  },
  session_tau_scale: 1.0,
  session_sigma_scale: 1.0,
  readiness: {
    venues_seen: ["hyperliquid", "lighter"],
    history_span_secs: 86_400,
    multi_venue_coverage: 1.0,
    pair_observations: 400,
    total_pair_observations: 400,
    min_observations_required: 120,
    obs_floor_met: true,
    blockers: [],
  },
};

test("preview renders banner, rows, and Copy values + Close only", () => {
  const html = renderToStaticMarkup(
    React.createElement(AutotunePreviewModal, {
      symbol: "BTC-USDC",
      recommendation: rec,
      loading: false,
      error: null,
      onClose: () => {},
    }),
  );
  assert.match(html, /AutoTune Preview/);
  assert.match(html, /READY 1\/2/);
  assert.match(html, /STABLE/);
  assert.match(html, /12\.34 bps/);
  assert.match(html, /HELD/);
  assert.match(html, /Copy values/);
  assert.match(html, /Close/);
  assert.match(html, /Apply selected \(0\)/);
  assert.match(html, /Session coupling/);
  assert.doesNotMatch(html, /Save &amp; Apply/);
  assert.doesNotMatch(html, /Save-Assess/);
});

test("preview renders an Apply checkbox per row, HELD disabled", () => {
  const html = renderToStaticMarkup(
    React.createElement(AutotunePreviewModal, {
      symbol: "BTC-USDC",
      recommendation: rec,
      loading: false,
      error: null,
      onClose: () => {},
    }),
  );
  const checkboxes = html.match(/type="checkbox"/g) ?? [];
  assert.equal(checkboxes.length, 2);
  const disabled = html.match(/disabled=""/g) ?? [];
  assert.ok(disabled.length >= 1, "HELD row checkbox must be disabled");
});

test("preview shows loading and error states", () => {
  const loading = renderToStaticMarkup(
    React.createElement(AutotunePreviewModal, {
      symbol: "BTC-USDC",
      recommendation: null,
      loading: true,
      error: null,
      onClose: () => {},
    }),
  );
  assert.match(loading, /Computing walk-forward/);
  const failed = renderToStaticMarkup(
    React.createElement(AutotunePreviewModal, {
      symbol: "BTC-USDC",
      recommendation: null,
      loading: false,
      error: "boom",
      onClose: () => {},
    }),
  );
  assert.match(failed, /AutoTune failed: boom/);
});

test("copy text covers READY rows only", () => {
  const text = buildAutotuneCopyText(rec);
  assert.match(text, /targetProfitBps=12\.34/);
  assert.doesNotMatch(text, /userTargetLeverage/);
});
