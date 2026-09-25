import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { PresetPanel } from "./TickerConfigModal.tsx";
import type { StrategyPresetDto } from "../types/index.ts";

const presets: StrategyPresetDto[] = [
  {
    id: "conservative",
    label: "Conservative",
    description: "Quiet-regime posture: only deep, rare dislocations clear the hurdle.",
    version: 1,
    targetProfitBps: 8.0,
    windowPercentile: 90,
    slidingWindowSecs: 21_600,
    executionMode: "SINGLE_WAY",
    stepSizeMult: 5.0,
    maxStepUsd: 1_000.0,
    maxTickerEquityPct: 10.0,
    userTargetLeverage: 2,
  },
  {
    id: "balanced",
    label: "Balanced",
    description: "Normal-regime posture: moderate hurdle, 20% equity cap, 5x leverage.",
    version: 1,
    targetProfitBps: 3.0,
    windowPercentile: 75,
    slidingWindowSecs: 43_200,
    executionMode: "SINGLE_WAY",
    stepSizeMult: 10.0,
    maxStepUsd: 2_500.0,
    maxTickerEquityPct: 20.0,
    userTargetLeverage: 5,
  },
  {
    id: "aggressive",
    label: "Aggressive",
    description: "Opportunity-rich posture: thin hurdle, double-way entries, 8x leverage.",
    version: 1,
    targetProfitBps: 1.5,
    windowPercentile: 60,
    slidingWindowSecs: 86_400,
    executionMode: "DOUBLE_WAY",
    stepSizeMult: 15.0,
    maxStepUsd: 5_000.0,
    maxTickerEquityPct: 30.0,
    userTargetLeverage: 8,
  },
];

test("PresetPanel renders every preset with label, posture note, and param summary", () => {
  const html = renderToStaticMarkup(
    React.createElement(PresetPanel, {
      presets,
      busyId: null,
      notice: null,
      onApply: () => {},
    })
  );
  for (const p of presets) {
    assert.ok(html.includes(p.label), `missing label ${p.label}`);
    assert.ok(html.includes(p.description), `missing description for ${p.id}`);
    assert.ok(html.includes(`v${p.version}`), `missing version for ${p.id}`);
    assert.ok(
      html.includes(`${p.targetProfitBps} bps`),
      `missing hurdle for ${p.id}`
    );
  }
  // Risk ladder readable: conservative 2x/10%, aggressive 8x/30%.
  assert.ok(html.includes("2x"));
  assert.ok(html.includes("8x"));
  assert.ok(html.includes("30%"));
});

test("PresetPanel shows the busy state on the applying card only", () => {
  const html = renderToStaticMarkup(
    React.createElement(PresetPanel, {
      presets,
      busyId: "balanced",
      notice: null,
      onApply: () => {},
    })
  );
  assert.equal((html.match(/Applying…/g) ?? []).length, 1);
  assert.ok(html.includes("Applying…"));
});

test("PresetPanel surfaces the apply notice verbatim", () => {
  const html = renderToStaticMarkup(
    React.createElement(PresetPanel, {
      presets,
      busyId: null,
      notice: "Applied Conservative (v1).",
      onApply: () => {},
    })
  );
  assert.ok(html.includes("Applied Conservative (v1)."));
  assert.ok(html.includes('role="status"'));
});
