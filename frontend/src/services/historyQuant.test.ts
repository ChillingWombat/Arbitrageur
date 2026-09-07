import { test } from "node:test";
import assert from "node:assert/strict";
import type { ExecutionLogEntry } from "../types/index.ts";

test("filter and compute telemetry metrics for selected ticker", () => {
  const sampleLogs: ExecutionLogEntry[] = [
    {
      id: "exec-1",
      timestamp: "2026-09-07T02:00:00Z",
      trade_type: "Spatial",
      symbol: "BTC-USDC",
      venue_a: "Hyperliquid",
      venue_b: "Lighter",
      leg_a_status: "FILLED",
      leg_b_status: "FILLED",
      success: true,
      gross_profit: 48.5,
      fees: 2.1,
      net_profit: 46.4,
      latency_us: 114,
      note: "Executed atomically",
      direction: "Divergence Entry",
      trigger_spread_bps: 18.2,
      baseline_mean_spread_bps: 11.4,
      position_step: 1,
      max_units: 3,
      nominal_hurdle_bps: 14.8,
    },
    {
      id: "exec-2",
      timestamp: "2026-09-07T02:05:00Z",
      trade_type: "Spatial",
      symbol: "BTC-USDC",
      venue_a: "Hyperliquid",
      venue_b: "Robinhood EVM",
      leg_a_status: "FILLED",
      leg_b_status: "FILLED",
      success: true,
      gross_profit: 41.0,
      fees: 1.9,
      net_profit: 39.1,
      latency_us: 120,
      note: "Scale in step 2",
      direction: "Divergence Entry",
      trigger_spread_bps: 22.4,
      baseline_mean_spread_bps: 13.1,
      position_step: 2,
      max_units: 3,
      nominal_hurdle_bps: 15.2,
    },
    {
      id: "exec-3",
      timestamp: "2026-09-07T02:10:00Z",
      trade_type: "Spatial",
      symbol: "ETH-USDC",
      venue_a: "Hyperliquid",
      venue_b: "Vanta.exchange",
      leg_a_status: "FILLED",
      leg_b_status: "FILLED",
      success: true,
      gross_profit: 32.0,
      fees: 1.8,
      net_profit: 30.2,
      latency_us: 98,
      note: "ETH trade",
      direction: "Carry Open",
      trigger_spread_bps: 15.0,
      baseline_mean_spread_bps: 9.5,
      position_step: 1,
      max_units: 3,
      nominal_hurdle_bps: 12.0,
    },
    {
      id: "exec-4",
      timestamp: "2026-09-07T02:15:00Z",
      trade_type: "Spatial",
      symbol: "BTC-USDC",
      venue_a: "Hyperliquid",
      venue_b: "Lighter",
      leg_a_status: "FILLED",
      leg_b_status: "FILLED",
      success: true,
      gross_profit: 26.0,
      fees: 1.5,
      net_profit: 24.5,
      latency_us: 108,
      note: "Partial unwind",
      direction: "Convergence Unwind",
      trigger_spread_bps: 4.2,
      baseline_mean_spread_bps: 12.0,
      position_step: 2,
      max_units: 3,
      nominal_hurdle_bps: 6.0,
    },
  ];

  // 1. Filter specifically for BTC-USDC
  const btcLogs = sampleLogs.filter(
    (l) => l.symbol === "BTC-USDC" || l.symbol.startsWith("BTC-USDC/") || l.symbol.endsWith("/BTC-USDC")
  );
  assert.equal(btcLogs.length, 3);

  // 2. Realized Net PnL
  const totalPnl = btcLogs.reduce((acc, l) => acc + l.net_profit, 0);
  assert.equal(Math.round(totalPnl * 10) / 10, 110.0); // 46.4 + 39.1 + 24.5 = 110.0

  // 3. Win rate
  const winRate = (btcLogs.filter((l) => l.success).length / btcLogs.length) * 100;
  assert.equal(winRate, 100);

  // 4. Average trigger spread
  const avgTrigger = btcLogs.reduce((acc, l) => acc + (l.trigger_spread_bps || 0), 0) / btcLogs.length;
  // (18.2 + 22.4 + 4.2) / 3 = 44.8 / 3 = 14.933 BPS
  assert.ok(Math.abs(avgTrigger - 14.93) < 0.05);

  // 5. Average baseline signal
  const avgBaseline = btcLogs.reduce((acc, l) => acc + (l.baseline_mean_spread_bps || 0), 0) / btcLogs.length;
  // (11.4 + 13.1 + 12.0) / 3 = 36.5 / 3 = 12.167 BPS
  assert.ok(Math.abs(avgBaseline - 12.17) < 0.05);

  // 6. Max position step reached
  const maxStep = btcLogs.reduce((max, l) => Math.max(max, l.position_step || 1), 0);
  assert.equal(maxStep, 2);

  // 7. Dislocation Delta calculation for Step 1
  const step1 = btcLogs[0];
  const dislocation = step1.trigger_spread_bps! - step1.baseline_mean_spread_bps!;
  assert.equal(Math.round(dislocation * 10) / 10, 6.8); // 18.2 - 11.4 = +6.8 BPS

  // 8. Hurdle clearance for Step 1
  const clearance = step1.trigger_spread_bps! - step1.nominal_hurdle_bps!;
  assert.equal(Math.round(clearance * 10) / 10, 3.4); // 18.2 - 14.8 = +3.4 BPS
});
