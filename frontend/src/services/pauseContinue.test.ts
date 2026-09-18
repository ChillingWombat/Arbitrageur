import { test } from "node:test";
import assert from "node:assert/strict";
import { api, DESKTOP_REQUIRED_ERROR } from "./api.ts";
import { getBotControlAvailability } from "../hooks/useBotLifecycle.ts";

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

test("idle bot with open positions can stop but not start", () => {
  const avail = getBotControlAvailability("idle", true, false);
  assert.equal(avail.stop, true);
  assert.equal(avail.start, false);
});

test("running bot with disabled entries can pause and stop but not start", () => {
  const avail = getBotControlAvailability("running", true, true);
  assert.equal(avail.pause, true);
  assert.equal(avail.stop, true);
  assert.equal(avail.start, false);
});

test("reduce-only control is available by default when running with open positions", () => {
  const avail = getBotControlAvailability("running", true, false);
  assert.equal(avail.reduceOnly, true);
});

test("reduce-only control stays unavailable when flat, stopping, or explicitly disabled", () => {
  const flatAvail = getBotControlAvailability("running", false, false);
  assert.equal(flatAvail.reduceOnly, false);
  const stoppingAvail = getBotControlAvailability("stopping", true, false);
  assert.equal(stoppingAvail.reduceOnly, false);
  const disabledAvail = getBotControlAvailability("running", true, false, false);
  assert.equal(disabledAvail.reduceOnly, false);
});
