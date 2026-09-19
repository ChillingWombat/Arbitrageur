// Unit tests for the rebalance-friction estimator (node:test style, matching repo).
// WHAT they prove: the arithmetic a user relies on before moving real money —
// direct beats via-wallet, unknown fees fail closed (null, never 0), same-venue
// is free, swap friction scales with size, worth-it verdicts respect the margin.
import test from "node:test";
import assert from "node:assert/strict";
import {
  estimateRebalance,
  getVenueProfile,
  listRebalanceVenues,
} from "./rebalanceFees.ts";

test("fee table covers the venues the app trades on", () => {
  const venues = listRebalanceVenues();
  for (const v of ["hyperliquid", "lighter", "vanta", "binance"]) {
    assert.ok(venues.includes(v), `missing ${v}`);
  }
});

test("hyperliquid encodes the verified $1 withdrawal fee", () => {
  assert.equal(getVenueProfile("HyperLiquid")?.withdrawFeeUsd, 1.0);
  assert.equal(getVenueProfile("hyperliquid")?.confidence, "verified");
});

test("direct HL->Lighter move prices known legs", () => {
  const r = estimateRebalance({ from: "hyperliquid", to: "lighter", amountUsd: 2000, viaDirect: true, swapNeeded: false });
  assert.ok(r.totalCostUsd != null);
  assert.ok(Math.abs(r.totalCostUsd! - 1.27) < 0.02, `got ${r.totalCostUsd}`);
  assert.ok(Math.abs(r.netReceivedUsd! - (2000 - 1.27)) < 0.02);
  assert.equal(r.verdict, "unknown");
});

test("direct path is cheaper than via-wallet (saves the hop gas)", () => {
  const direct = estimateRebalance({ from: "hyperliquid", to: "lighter", amountUsd: 2000, viaDirect: true, swapNeeded: false });
  const viaWallet = estimateRebalance({ from: "hyperliquid", to: "lighter", amountUsd: 2000, viaDirect: false, swapNeeded: false });
  assert.ok(viaWallet.totalCostUsd! > direct.totalCostUsd!);
});

test("same venue costs nothing", () => {
  const r = estimateRebalance({ from: "lighter", to: "lighter", amountUsd: 500, viaDirect: true, swapNeeded: false });
  assert.equal(r.totalCostUsd, 0);
});

test("unknown venue fails closed (null total, unknown verdict)", () => {
  const r = estimateRebalance({ from: "removedvenue", to: "lighter", amountUsd: 500, viaDirect: true, swapNeeded: false });
  assert.equal(r.totalCostUsd, null);
  assert.equal(r.verdict, "unknown");
  assert.ok(r.warnings.length > 0);
});

test("swap friction scales with size (15bps default)", () => {
  const r = estimateRebalance({ from: "binance", to: "lighter", amountUsd: 10_000, viaDirect: true, swapNeeded: true });
  assert.ok(r.totalCostUsd! > 15);
});

test("worth-it verdict respects the 2x safety margin", () => {
  const worth = estimateRebalance({ from: "hyperliquid", to: "lighter", amountUsd: 2000, viaDirect: true, swapNeeded: false, opportunityProfitUsd: 50 });
  assert.equal(worth.verdict, "worth_it");
  const notWorth = estimateRebalance({ from: "hyperliquid", to: "lighter", amountUsd: 2000, viaDirect: true, swapNeeded: false, opportunityProfitUsd: 1 });
  assert.equal(notWorth.verdict, "not_worth_it");
});
