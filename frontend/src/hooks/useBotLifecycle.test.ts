import { test } from "node:test";
import assert from "node:assert/strict";
import { shouldAttemptFlattenOnStop } from "./useBotLifecycle.ts";

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
