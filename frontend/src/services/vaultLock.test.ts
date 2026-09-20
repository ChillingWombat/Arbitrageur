import { test } from "node:test";
import assert from "node:assert/strict";
import { scheduleVaultLock } from "./vaultLock.ts";

test("scheduleVaultLock fires exactly once after the delay", async () => {
  let calls = 0;
  await new Promise<void>((resolve) => {
    scheduleVaultLock(() => {
      calls += 1;
      resolve();
    }, 5);
  });
  await new Promise((r) => setTimeout(r, 15));
  assert.equal(calls, 1);
});

test("scheduleVaultLock cancel is idempotent and prevents the run", async () => {
  let calls = 0;
  const cancel = scheduleVaultLock(() => {
    calls += 1;
  }, 5);
  cancel();
  cancel();
  await new Promise((r) => setTimeout(r, 20));
  assert.equal(calls, 0);
});

test("scheduleVaultLock cancel after firing is a no-op", async () => {
  let calls = 0;
  const cancel = scheduleVaultLock(() => {
    calls += 1;
  }, 5);
  await new Promise((r) => setTimeout(r, 15));
  cancel();
  assert.equal(calls, 1);
});
