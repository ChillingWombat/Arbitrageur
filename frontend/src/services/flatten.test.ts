import { test } from "node:test";
import assert from "node:assert/strict";
import { flattenResidualMessage } from "./flatten.ts";

test("zero residual flatten reports success", () => {
  assert.equal(
    flattenResidualMessage({ strategyId: "radar", symbol: null, residualSignals: 0, signalsProcessed: 3, signalsClosed: 3, legsClosed: 6, notes: [] }),
    null
  );
});

test("nonzero residual flatten surfaces exposure instead of success", () => {
  const msg = flattenResidualMessage({ strategyId: "radar", symbol: null, residualSignals: 2, signalsProcessed: 3, signalsClosed: 1, legsClosed: 2, notes: [] });
  assert.ok(msg?.includes("2 signal(s)"));
  assert.ok(msg?.includes("residual exposure"));
});
