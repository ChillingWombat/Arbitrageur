import { test } from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { BotLifecycleControls, BotPhaseBadge } from "./BotLifecycleControls.tsx";

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

test("BotPhaseBadge labels drain phases and renders nothing otherwise", () => {
  const reduceOnly = renderToStaticMarkup(
    React.createElement(BotPhaseBadge, { phase: "reduce_only" })
  );
  const stopping = renderToStaticMarkup(
    React.createElement(BotPhaseBadge, { phase: "stopping" })
  );
  const running = renderToStaticMarkup(
    React.createElement(BotPhaseBadge, { phase: "running" })
  );

  assert.match(reduceOnly, /Reduce-Only/);
  assert.match(stopping, /Stopping/);
  assert.equal(running, "");
});
