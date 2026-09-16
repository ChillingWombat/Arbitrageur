import test from "node:test";
import assert from "node:assert/strict";
import {
  UTILISATION_HISTORY_UNAVAILABLE_REASON,
  computeExchangeStats,
} from "./exchangeStats.ts";
import type { ExecutionLogEntry, VenueBalanceInfo } from "../types/index.ts";

const log = (
  id: string,
  venueA: string,
  venueB: string,
  netProfit: number,
  extra?: Partial<ExecutionLogEntry>,
): ExecutionLogEntry => ({
  id,
  timestamp: "2026-01-01T00:00:00Z",
  trade_type: "Spatial",
  symbol: "BTC",
  venue_a: venueA,
  venue_b: venueB,
  leg_a_status: "FILLED",
  leg_b_status: "FILLED",
  success: true,
  gross_profit: netProfit + 2,
  fees: 2,
  net_profit: netProfit,
  latency_us: 10,
  note: "paper",
  ...extra,
});

const balance = (
  venue: string,
  equityUsd: number,
  marginRatePct: number,
): VenueBalanceInfo => ({
  venue,
  equity_usd: equityUsd,
  available_margin_usd: equityUsd * (1 - marginRatePct / 100),
  margin_rate_pct: marginRatePct,
  status: "HEALTHY",
});

test("two-leg trades count on both venues with shared totals", () => {
  const rows = computeExchangeStats(
    [log("1", "hyperliquid", "lighter", 10), log("2", "hyperliquid", "lighter", -4)],
    [balance("hyperliquid", 10000, 25), balance("lighter", 8000, 10)],
  );
  assert.equal(rows.length, 2);
  const hl = rows.find((r) => r.venue === "hyperliquid")!;
  const lgt = rows.find((r) => r.venue === "lighter")!;
  assert.equal(hl.trades, 2);
  assert.equal(hl.totalPnlUsd, 6);
  assert.equal(hl.avgPnlUsd, 3);
  assert.deepEqual(lgt, { ...hl, venue: "lighter", equityUsd: 8000, utilisationPct: 10 });
});

test("totals use fee-inclusive net profit, never gross plus fees", () => {
  const rows = computeExchangeStats(
    [log("1", "hyperliquid", "lighter", 8, { gross_profit: 10, fees: 2 })],
    [],
  );
  assert.equal(rows[0].totalPnlUsd, 8);
  assert.equal(rows[0].avgPnlUsd, 8);
});

test("utilisation is the used-margin share; 24h average is honestly unavailable", () => {
  const rows = computeExchangeStats([], [balance("hyperliquid", 10000, 40)]);
  assert.equal(rows.length, 1);
  assert.equal(rows[0].trades, 0);
  assert.equal(rows[0].totalPnlUsd, 0);
  assert.equal(rows[0].avgPnlUsd, null);
  assert.equal(rows[0].equityUsd, 10000);
  assert.equal(rows[0].utilisationPct, 40);
  assert.equal(rows[0].avgUtilisationPct, null);
  assert.equal(rows[0].avgUtilisationNote, UTILISATION_HISTORY_UNAVAILABLE_REASON);
});

test("venues seen only in logs show missing equity as a dash, not zero", () => {
  const rows = computeExchangeStats([log("1", "hyperliquid", "lighter", 5)], []);
  assert.equal(rows.length, 2);
  for (const row of rows) {
    assert.equal(row.equityUsd, null);
    assert.equal(row.utilisationPct, null);
    assert.equal(row.trades, 1);
  }
});

test("venue grouping is case-insensitive and output is venue-sorted", () => {
  const rows = computeExchangeStats(
    [
      log("1", "Hyperliquid", "LIGHTER", 4),
      log("2", "lighter", "hyperliquid", 2),
      log("3", "vanta", "hyperliquid", 6),
    ],
    [],
  );
  assert.deepEqual(rows.map((r) => r.venue), ["hyperliquid", "lighter", "vanta"]);
  assert.equal(rows[0].trades, 3);
  assert.equal(rows[0].totalPnlUsd, 12);
  assert.equal(rows[1].trades, 2);
});

test("null inputs aggregate to empty, never throw", () => {
  assert.deepEqual(computeExchangeStats(null, null), []);
  assert.deepEqual(computeExchangeStats(undefined, undefined), []);
});

test("aggregation is deterministic", () => {
  const logs = [log("2", "lighter", "hyperliquid", -4), log("1", "hyperliquid", "lighter", 10)];
  const balances = [balance("lighter", 8000, 10), balance("hyperliquid", 10000, 25)];
  assert.deepEqual(computeExchangeStats(logs, balances), computeExchangeStats([...logs].reverse(), [...balances].reverse()));
});

test("24h average utilisation renders from snapshot history", () => {
  const rows = computeExchangeStats([log("1", "hyperliquid", "lighter", 5)], [], {
    hyperliquid: {
      venue: "hyperliquid",
      avgRatePct: 22.5,
      coveredMs: 86_400_000,
      windowMs: 86_400_000,
      partial: false,
      pointCount: 120,
    },
  });
  const hl = rows.find((r) => r.venue === "hyperliquid")!;
  assert.equal(hl.avgUtilisationPct, 22.5);
  assert.equal(hl.avgUtilisationNote, "24h time-weighted avg");
  const lgt = rows.find((r) => r.venue === "lighter")!;
  assert.equal(lgt.avgUtilisationPct, null);
  assert.equal(lgt.avgUtilisationNote, UTILISATION_HISTORY_UNAVAILABLE_REASON);
});

test("short snapshot windows are labeled honestly, never stretched", () => {
  const rows = computeExchangeStats([], [balance("lighter", 8000, 10)], {
    lighter: {
      venue: "lighter",
      avgRatePct: 10,
      coveredMs: 3_600_000,
      windowMs: 86_400_000,
      partial: true,
      pointCount: 4,
    },
  });
  assert.equal(rows[0].avgUtilisationPct, 10);
  assert.equal(rows[0].avgUtilisationNote, "avg over 1.0h available");
});

test("non-finite history averages fall back to unavailable", () => {
  const rows = computeExchangeStats([], [balance("lighter", 8000, 10)], {
    lighter: {
      venue: "lighter",
      avgRatePct: NaN,
      coveredMs: 0,
      windowMs: 86_400_000,
      partial: true,
      pointCount: 0,
    },
  });
  assert.equal(rows[0].avgUtilisationPct, null);
  assert.equal(rows[0].avgUtilisationNote, UTILISATION_HISTORY_UNAVAILABLE_REASON);
});
