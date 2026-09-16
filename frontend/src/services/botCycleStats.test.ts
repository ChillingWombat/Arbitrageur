import test from "node:test";
import assert from "node:assert/strict";
import {
  BOT_PROFIT_UNATTRIBUTED_REASON,
  computeBotCycleStats,
} from "./botCycleStats.ts";
import type { SignalExecutionDto, SignalLegDto, StrategyFillPnl } from "../types/index.ts";

const leg = (filledQty: number): SignalLegDto => ({
  id: "leg-1",
  signalId: "sig",
  accountId: null,
  venue: "hyperliquid",
  side: "long",
  intendedQty: 1,
  filledQty,
  fillPrice: 100,
  feeUsd: 0.5,
  venueOrderId: null,
  createdAt: "2026-01-01T00:00:00Z",
  updatedAt: "2026-01-01T00:00:00Z",
});

const sig = (
  id: string,
  status: SignalExecutionDto["status"],
  strategyId = "radar",
  filledQty = 1,
): SignalExecutionDto => ({
  id,
  strategyId,
  symbol: "BTC",
  status,
  routeId: null,
  accountLongId: null,
  accountShortId: null,
  expectedNetUsd: 12,
  feesUsd: 1,
  note: null,
  legs: [leg(filledQty), { ...leg(filledQty), side: "short", venue: "lighter" }],
  createdAt: "2026-01-01T00:00:00Z",
  updatedAt: "2026-01-01T00:00:00Z",
  closedAt: status === "closed" ? "2026-01-02T00:00:00Z" : null,
});

test("open cycle counts while entered, completes on close (round trip)", () => {
  const open = computeBotCycleStats([sig("a", "balanced")]);
  assert.equal(open.openCycles, 1);
  assert.equal(open.completedCycles, 0);

  const closed = computeBotCycleStats([sig("a", "closed")]);
  assert.equal(closed.openCycles, 0);
  assert.equal(closed.completedCycles, 1);
});

test("all open ledger statuses count as open cycles", () => {
  const signals = ["in_flight", "balanced", "imbalanced", "unresolved"].map(
    (status, i) => sig(`s${i}`, status as SignalExecutionDto["status"]),
  );
  const stats = computeBotCycleStats(signals);
  assert.equal(stats.openCycles, 4);
  assert.equal(stats.completedCycles, 0);
});

test("failed and unfilled signals never count as cycles", () => {
  const stats = computeBotCycleStats([
    sig("f1", "failed"),
    sig("f2", "closed", "radar", 0),
    sig("f3", "balanced", "radar", 0),
  ]);
  assert.equal(stats.openCycles, 0);
  assert.equal(stats.completedCycles, 0);
});

test("other strategies are excluded, matching case-insensitively", () => {
  const stats = computeBotCycleStats([
    sig("a", "closed", "futures"),
    sig("b", "closed", "RADAR"),
    sig("c", "balanced", "Funding"),
  ]);
  assert.equal(stats.completedCycles, 1);
  assert.equal(stats.openCycles, 0);
});

test("net profit is honestly unattributed, never fabricated", () => {
  const stats = computeBotCycleStats([sig("a", "closed"), sig("b", "closed")]);
  assert.equal(stats.completedCycles, 2);
  assert.equal(stats.netProfitUsd, null);
  assert.equal(stats.netProfitBps, null);
  assert.equal(stats.profitUnattributedReason, BOT_PROFIT_UNATTRIBUTED_REASON);
});

test("missing ledger reads as unknown, not zero", () => {
  for (const missing of [null, undefined]) {
    const stats = computeBotCycleStats(missing);
    assert.equal(stats.unknown, true);
    assert.equal(stats.netProfitUsd, null);
  }
  assert.equal(computeBotCycleStats([]).unknown, false);
});

test("aggregation is deterministic", () => {
  const signals = [sig("b", "closed"), sig("a", "balanced"), sig("c", "failed")];
  const first = computeBotCycleStats(signals);
  const second = computeBotCycleStats([...signals].reverse());
  assert.deepEqual(second, first);
});

const fillPnl = (
  strategyId: string,
  netProfitUsd: number,
  netProfitBps: number | null = 12.5,
  unattributedClosedFamilies = 0,
): StrategyFillPnl => ({
  strategyId,
  netProfitUsd,
  netProfitBps,
  closedFamilies: 2,
  openFamilies: 1,
  unattributedClosedFamilies,
  unattributedLogEntries: 0,
});

test("attributed fill roll-up supplies real net profit with fees included", () => {
  const stats = computeBotCycleStats(
    [sig("a", "closed"), sig("b", "balanced")],
    "radar",
    fillPnl("radar", 64, 10.5),
  );
  assert.equal(stats.completedCycles, 1);
  assert.equal(stats.openCycles, 1);
  assert.equal(stats.netProfitUsd, 64);
  assert.equal(stats.netProfitBps, 10.5);
  assert.equal(stats.profitUnattributedReason, null);
  assert.equal(stats.unknown, false);
});

test("mismatched or missing roll-ups keep the honest fallback", () => {
  const signals = [sig("a", "closed")];
  const wrongStrategy = computeBotCycleStats(signals, "radar", fillPnl("futures", 64));
  assert.equal(wrongStrategy.netProfitUsd, null);
  assert.equal(wrongStrategy.profitUnattributedReason, BOT_PROFIT_UNATTRIBUTED_REASON);
  const absent = computeBotCycleStats(signals, "radar", null);
  assert.equal(absent.netProfitUsd, null);
  const nonFinite = computeBotCycleStats(signals, "radar", fillPnl("radar", NaN));
  assert.equal(nonFinite.netProfitUsd, null);
});

test("unvalued families are surfaced, never absorbed", () => {
  const stats = computeBotCycleStats([sig("a", "closed")], "radar", fillPnl("radar", 64, null, 3));
  assert.equal(stats.netProfitUsd, 64);
  assert.equal(stats.netProfitBps, null);
  assert.equal(stats.unattributedClosedFamilies, 3);
});
