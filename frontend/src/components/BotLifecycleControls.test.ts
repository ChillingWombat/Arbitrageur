import { test } from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  BotLifecycleControls,
  BotPhaseBadge,
  StrategyStatusBadge,
  resolveBotStatus,
} from "./BotLifecycleControls.tsx";

const noop = () => {};

test("BotLifecycleControls renders all 4 buttons including Reduce Only by default", () => {
  const html = renderToStaticMarkup(
    React.createElement(BotLifecycleControls, {
      phase: "idle",
      hasOpenPositions: false,
      onStart: noop,
      onPause: noop,
      onReduceOnly: noop,
      onStop: noop,
    })
  );

  assert.match(html, /Start/);
  assert.match(html, /Pause/);
  assert.match(html, /Reduce Only/);
  assert.match(html, /Stop/);
});

test("Reduce Only is enabled when bot is running with open positions", () => {
  const html = renderToStaticMarkup(
    React.createElement(BotLifecycleControls, {
      phase: "running",
      hasOpenPositions: true,
      onStart: noop,
      onPause: noop,
      onReduceOnly: noop,
      onStop: noop,
    })
  );

  assert.match(html, /text-warn-strong/);
});

test("Reduce Only is disabled when bot is idle without open positions", () => {
  const html = renderToStaticMarkup(
    React.createElement(BotLifecycleControls, {
      phase: "idle",
      hasOpenPositions: false,
      onStart: noop,
      onPause: noop,
      onReduceOnly: noop,
      onStop: noop,
    })
  );

  assert.match(html, /Reduce Only requires open positions to drain/);
});

test("BotLifecycleControls renders no phase label — the badge lives in the Strategy Status ribbon", () => {
  const html = renderToStaticMarkup(
    React.createElement(BotLifecycleControls, {
      phase: "reduce_only",
      hasOpenPositions: true,
      onStart: noop,
      onPause: noop,
      onReduceOnly: noop,
      onStop: noop,
    })
  );

  // Badge would render as element text; the button's "Reduce-Only mode"
  // tooltip is an attribute and stays.
  assert.doesNotMatch(html, /Reduce-Only<\/span>/);
  assert.doesNotMatch(html, />Stopping</);
});

test("resolveBotStatus resolves all phase and isPaused combinations", () => {
  assert.equal(resolveBotStatus("running", false), "Active");
  assert.equal(resolveBotStatus("running", true), "Paused");
  assert.equal(resolveBotStatus("paused", false), "Paused");
  assert.equal(resolveBotStatus("paused", true), "Paused");
  assert.equal(resolveBotStatus("reduce_only", false), "Reducing");
  assert.equal(resolveBotStatus("reduce_only", true), "Reducing");
  assert.equal(resolveBotStatus("stopping", false), "Reducing");
  assert.equal(resolveBotStatus("stopping", true), "Reducing");
  assert.equal(resolveBotStatus("idle", false), "Stopped");
  assert.equal(resolveBotStatus("idle", true), "Stopped");
  assert.equal(resolveBotStatus(undefined, false), "Stopped");
  assert.equal(resolveBotStatus(undefined, true), "Stopped");
});

test("BotPhaseBadge renders all 4 standardized statuses and never renders Engine", () => {
  const cases = [
    { props: { phase: "running" as const, isPaused: false }, expected: "Active", dotClass: "bg-profit", badgeClass: "bg-profit-dim" },
    { props: { phase: "running" as const, isPaused: true }, expected: "Paused", dotClass: "bg-warn", badgeClass: "bg-warn-dim" },
    { props: { phase: "paused" as const }, expected: "Paused", dotClass: "bg-warn", badgeClass: "bg-warn-dim" },
    { props: { phase: "reduce_only" as const }, expected: "Reducing", dotClass: "animate-pulse", badgeClass: "bg-warn-dim" },
    { props: { phase: "stopping" as const }, expected: "Reducing", dotClass: "animate-pulse", badgeClass: "bg-warn-dim" },
    { props: { phase: "idle" as const }, expected: "Stopped", dotClass: "bg-slate-400", badgeClass: "bg-surface-raised" },
    { props: {}, expected: "Stopped", dotClass: "bg-slate-400", badgeClass: "bg-surface-raised" },
    { props: { status: "Active" as const }, expected: "Active", dotClass: "bg-profit", badgeClass: "bg-profit-dim" },
    { props: { status: "Paused" as const }, expected: "Paused", dotClass: "bg-warn", badgeClass: "bg-warn-dim" },
    { props: { status: "Reducing" as const }, expected: "Reducing", dotClass: "animate-pulse", badgeClass: "bg-warn-dim" },
    { props: { status: "Stopped" as const }, expected: "Stopped", dotClass: "bg-slate-400", badgeClass: "bg-surface-raised" },
  ];

  for (const c of cases) {
    const html = renderToStaticMarkup(React.createElement(BotPhaseBadge, c.props));
    assert.match(html, new RegExp(`>${c.expected}<`));
    assert.match(html, new RegExp(c.dotClass));
    assert.match(html, new RegExp(c.badgeClass));
    assert.match(html, new RegExp(`title="Strategy bot status: ${c.expected}"`));
    assert.doesNotMatch(html, /Engine/i);
  }
});

test("StrategyStatusBadge is an alias of BotPhaseBadge", () => {
  assert.equal(StrategyStatusBadge, BotPhaseBadge);
  const html = renderToStaticMarkup(React.createElement(StrategyStatusBadge, { status: "Active" }));
  assert.match(html, />Active</);
  assert.doesNotMatch(html, /Engine/i);
});

