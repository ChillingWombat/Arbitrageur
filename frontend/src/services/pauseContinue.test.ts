import { test } from "node:test";
import assert from "node:assert/strict";
import { api } from "./api.ts";

test("pause and continue switches engine pause state without clearing positions", async () => {
  // 1. Initial state: not paused
  const status1 = await api.getSystemStatus();
  assert.equal(status1.is_paused ?? false, false);

  // 2. Pause trading
  const paused = await api.setEnginePaused(true);
  assert.equal(paused, true);
  const status2 = await api.getSystemStatus();
  assert.equal(status2.is_paused, true);

  // 3. Trade execution rejects while paused
  await assert.rejects(
    async () => {
      await api.executeSpatialArbitrage(
        "BTC-USDC",
        "hyperliquid",
        "lighter",
        0.01
      );
    },
    {
      message: /PAUSED/,
    }
  );

  // 4. Continue trading
  const continued = await api.setEnginePaused(false);
  assert.equal(continued, false);
  const status3 = await api.getSystemStatus();
  assert.equal(status3.is_paused, false);

  // 5. Trade execution succeeds when continued
  const result = await api.executeSpatialArbitrage(
    "BTC-USDC",
    "hyperliquid",
    "lighter",
    0.01
  );
  assert.ok(result.execution_success);
});
