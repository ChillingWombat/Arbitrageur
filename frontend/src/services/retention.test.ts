import { test } from "node:test";
import assert from "node:assert";
import {
  normalizeRetentionDays,
  normalizeRetentionGb,
  retentionDaysToInput,
  retentionGbToInput,
} from "./retention.ts";

test("normalizeRetentionDays: empty means unlimited", () => {
  assert.strictEqual(normalizeRetentionDays(""), null);
  assert.strictEqual(normalizeRetentionDays("   "), null);
});

test("normalizeRetentionDays: zero and invalid clamp to unlimited", () => {
  assert.strictEqual(normalizeRetentionDays("0"), null);
  assert.strictEqual(normalizeRetentionDays("-5"), null);
  assert.strictEqual(normalizeRetentionDays("NaN"), null);
  assert.strictEqual(normalizeRetentionDays("abc"), null);
});

test("normalizeRetentionDays: positive values pass through floored", () => {
  assert.strictEqual(normalizeRetentionDays("30"), 30);
  assert.strictEqual(normalizeRetentionDays(" 7 "), 7);
  assert.strictEqual(normalizeRetentionDays("2.9"), 2);
});

test("normalizeRetentionGb: empty means unlimited", () => {
  assert.strictEqual(normalizeRetentionGb(""), null);
  assert.strictEqual(normalizeRetentionGb("   "), null);
});

test("normalizeRetentionGb: zero and invalid clamp to unlimited", () => {
  assert.strictEqual(normalizeRetentionGb("0"), null);
  assert.strictEqual(normalizeRetentionGb("-1.5"), null);
  assert.strictEqual(normalizeRetentionGb("Infinity"), null);
  assert.strictEqual(normalizeRetentionGb("NaN"), null);
});

test("normalizeRetentionGb: positive values pass through", () => {
  assert.strictEqual(normalizeRetentionGb("2.5"), 2.5);
  assert.strictEqual(normalizeRetentionGb(" 10 "), 10);
});

test("retention inputs hydrate from settings (null stays empty)", () => {
  assert.strictEqual(retentionDaysToInput(null), "");
  assert.strictEqual(retentionDaysToInput(undefined), "");
  assert.strictEqual(retentionDaysToInput(30), "30");
  assert.strictEqual(retentionGbToInput(null), "");
  assert.strictEqual(retentionGbToInput(2.5), "2.5");
});
