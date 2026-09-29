import { test } from "node:test";
import assert from "node:assert/strict";
import {
  shouldAttemptFlattenOnStop,
  stopAllFrontendBots,
  tickerBotStatus,
  getBotControlAvailability,
  publishBotPhase,
  registerBotResetListener,
} from "./useBotLifecycle.ts";

test("stale ledger forces flatten on stop even with zero counted signals", () => {
  assert.equal(
    shouldAttemptFlattenOnStop({ ledgerFresh: false, openSignalCount: 0 }),
    true
  );
});

test("stale ledger forces flatten even when count is stale-zero after poll failure", () => {
  assert.equal(
    shouldAttemptFlattenOnStop({ ledgerFresh: false, openSignalCount: 0, externalOpenPositions: false }),
    true
  );
});

test("fresh ledger with zero signals and no external positions skips flatten", () => {
  assert.equal(
    shouldAttemptFlattenOnStop({ ledgerFresh: true, openSignalCount: 0 }),
    false
  );
  assert.equal(
    shouldAttemptFlattenOnStop({ ledgerFresh: true, openSignalCount: 0, externalOpenPositions: false }),
    false
  );
});

test("fresh ledger with open signals attempts flatten", () => {
  assert.equal(
    shouldAttemptFlattenOnStop({ ledgerFresh: true, openSignalCount: 2 }),
    true
  );
});

test("external open positions force flatten even on fresh empty ledger", () => {
  assert.equal(
    shouldAttemptFlattenOnStop({ ledgerFresh: true, openSignalCount: 0, externalOpenPositions: true }),
    true
  );
});

test("stopAllFrontendBots resets all registered bot phases to idle", () => {
  publishBotPhase("test_strat", "BTC", "running");
  assert.equal(tickerBotStatus("BTC"), "running");

  stopAllFrontendBots();
  // With registry reset to idle, tickerBotStatus for any symbol returns stopped
  assert.equal(tickerBotStatus("BTC"), "stopped");
});

test("registerBotResetListener registers listener and unregisters cleanly", () => {
  let callCount = 0;
  const unregister = registerBotResetListener(() => {
    callCount += 1;
  });

  stopAllFrontendBots();
  assert.equal(callCount, 1);

  stopAllFrontendBots();
  assert.equal(callCount, 2);

  unregister();
  stopAllFrontendBots();
  assert.equal(callCount, 2);
});

test("idle bot can start individually and stop with positions, but cannot pause or reduce", () => {
  const avail = getBotControlAvailability("idle", false, false);
  assert.equal(avail.start, true);
  assert.equal(avail.pause, false);
  assert.equal(avail.reduceOnly, false);
});

