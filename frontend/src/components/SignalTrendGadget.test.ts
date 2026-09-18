import { test } from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { SignalTrendGadget } from "./SignalTrendGadget.tsx";

test("SignalTrendGadget renders header, window options and empty state initially", () => {
  const html = renderToStaticMarkup(
    React.createElement(SignalTrendGadget, {
      symbol: "BTC-USDC",
      botWindowSec: 3600,
      hurdleBps: 2.5,
    })
  );

  assert.match(html, /Venue deviation from baseline/);
  assert.match(html, /Consensus/);
  assert.match(html, /Leader/);
  assert.match(html, /Collecting Signal Samples…/);
  assert.match(html, /rounded-lg border border-surface-border bg-surface overflow-hidden shadow-2xs relative/);
});
