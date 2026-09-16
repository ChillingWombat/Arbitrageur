import { test } from "node:test";
import assert from "node:assert/strict";
import { formatWinPct } from "./winPct.ts";

// Fixtures mirroring the scan contract: Some(p) in (0,1] renders whole
// percent, None (thin history) renders the "—" fallback.
test("formatWinPct renders qualified probabilities as whole percent", () => {
  assert.equal(formatWinPct(0.93), "93%");
  assert.equal(formatWinPct(1), "100%");
  assert.equal(formatWinPct(0.005), "1%");
  assert.equal(formatWinPct(0), "0%");
});

test("formatWinPct falls back to em-dash for missing data", () => {
  assert.equal(formatWinPct(null), "—");
  assert.equal(formatWinPct(undefined), "—");
  assert.equal(formatWinPct(NaN), "—");
  assert.equal(formatWinPct(Infinity), "—");
});
