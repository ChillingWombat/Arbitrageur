import test from "node:test";
import assert from "node:assert/strict";
import { actionLabel } from "./TickerTransactionHistory.tsx";
import type { ExecutionLogEntry } from "../types";

const baseLog = (over: Partial<ExecutionLogEntry>): ExecutionLogEntry =>
  ({
    id: "l1",
    timestamp: "2026-09-29T02:00:00Z",
    trade_type: "Funding",
    symbol: "BTC-USDC",
    venue_a: "lighter",
    venue_b: "hyperliquid",
    leg_a_status: "filled",
    leg_b_status: "filled",
    success: true,
    gross_profit: 0,
    fees: 0,
    net_profit: 0,
    latency_us: 0,
    note: "",
    ...over,
  }) as ExecutionLogEntry;

test("actionLabel strips @venue spellings and ticker, keeps leg direction", () => {
  const log = baseLog({
    direction: "LONG BTC-USDC@lighter / SHORT BTC-USDC@hyperliquid",
  });
  assert.equal(actionLabel(log), "LONG / SHORT");
});

test("actionLabel strips bare venue names too", () => {
  const log = baseLog({ direction: "Long lighter / Short hyperliquid" });
  assert.equal(actionLabel(log), "Long / Short");
});

test("actionLabel leaves clean action text untouched", () => {
  assert.equal(actionLabel(baseLog({ direction: "Emergency Flatten" })), "Emergency Flatten");
  assert.equal(actionLabel(baseLog({ direction: "Convergence Unwind" })), "Convergence Unwind");
});

test("actionLabel falls back to raw when stripping would empty the badge", () => {
  const log = baseLog({ direction: "BTC-USDC" });
  assert.equal(actionLabel(log), "BTC-USDC");
});

test("actionLabel defaults when direction is absent", () => {
  assert.equal(actionLabel(baseLog({ direction: undefined })), "Divergence Entry");
});
