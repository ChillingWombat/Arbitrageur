import { test } from "node:test";
import assert from "node:assert/strict";
import { api, DESKTOP_REQUIRED_ERROR } from "./api.ts";

// The synthetic browser demo engine was removed: outside the desktop app
// every engine call must fail closed rather than fabricate state.
test("engine calls fail closed without the desktop application", async () => {
  const expected = new RegExp(DESKTOP_REQUIRED_ERROR);

  await assert.rejects(api.getSystemStatus(), expected);
  await assert.rejects(api.setEnginePaused(true), expected);
  await assert.rejects(api.setEnginePaused(false), expected);
  await assert.rejects(
    api.executeSpatialArbitrage("BTC-USDC", "hyperliquid", "lighter", 0.01),
    expected,
  );
  await assert.rejects(api.getRiskOverview(), expected);
  await assert.rejects(api.getExecutionLogs(), expected);
});
