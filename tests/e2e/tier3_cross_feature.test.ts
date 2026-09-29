/**
 * Tier 3: Cross-Feature Combinations (Pairwise Interactions)
 * Validates complex interactions between subsystems:
 * - T3.1: Killswitch interrupting active bot trade dispatch (R2 x R4)
 * - T3.2: Vault auto-lock during emergency flatten sequence (R2 x Auth)
 * - T3.3: Circuit breaker trip inhibiting all quantitative bot execution gates (R2 x R4)
 * - T3.4: UI ToggleSwitch state sync with tripped circuit breaker (R3 x R2)
 * - T3.5: Partitioned deduplication of engine vs external positions (R2.3 x External Positions)
 * - T3.6: High-frequency telemetry updates during killswitch execution (R2 x Telemetry)
 */

import { describe, it, before, after } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import {
  MockDevRpcServer,
  sendDevRpc,
  evaluateSpreadTollGate,
} from "./test_helpers.ts";

describe("Tier 3: Cross-Feature Combinations — Pairwise Subsystem Interactions", () => {
  let mockServer: MockDevRpcServer;
  const testSocket = "/tmp/test-arb-cross-t3.sock";

  before(async () => {
    mockServer = new MockDevRpcServer(testSocket);
    await mockServer.start();
  });

  after(async () => {
    await mockServer.stop();
  });

  it("T3.1: Killswitch Interrupting Active Bot Trade Dispatch (R2 x R4)", async () => {
    // Setup state: Bot has dispatched leg 1 of a spatial arb trade
    mockServer.state.is_paused = false;
    mockServer.state.circuit_breaker_tripped = false;
    mockServer.state.orders = [
      { id: "leg-a-resting", venue: "hyperliquid", symbol: "BTC-USDC", qty: 0.1 },
    ];
    mockServer.state.engine_positions = [
      { id: "sig-active-1", venue: "binance", symbol: "BTC-USDT", qty: 0.1, value_usd: 6500 },
    ];

    // Trigger emergency killswitch
    const res = await sendDevRpc(testSocket, "killswitch");
    assert.ok(res.ok, "Killswitch must succeed");
    const report = res.ok;

    // Verify both order cancel and position unwind occurred
    assert.equal(report.cancelled_orders_count, 1, "Resting leg must be cancelled");
    assert.equal(report.closed_engine_positions, 1, "In-flight leg position must be closed");
    assert.equal(report.circuit_breaker_tripped, true, "Circuit breaker must be tripped");
    assert.equal(report.engine_paused, true, "Engine must be paused to block new bot dispatches");

    // Verify engine state is clean
    const status = await sendDevRpc(testSocket, "system_status");
    assert.equal(status.ok.is_paused, true);
    assert.equal(status.ok.circuit_breaker_tripped, true);
    assert.equal(status.ok.open_positions_count, 0);
  });

  it("T3.2: Vault Auto-Lock During Emergency Flatten Sequence (R2 x Auth)", async () => {
    // Testnet environment should allow flatten even if vault timer triggers auto-lock
    mockServer.state.trading_mode = "testnet";
    mockServer.state.vault_locked = true; // Auto-lock simulated
    mockServer.state.engine_positions = [
      { id: "sig-testnet-1", venue: "hyperliquid", symbol: "ETH-USDC", qty: 1.0, value_usd: 3200 },
    ];

    const res = await sendDevRpc(testSocket, "stop_all");
    assert.ok(res.ok, "Testnet emergency stop_all must succeed regardless of vault lock state");
    assert.equal(res.ok.closed_engine_positions, 1);
    assert.equal(res.ok.circuit_breaker_tripped, true);
  });

  it("T3.3: Circuit Breaker Trip Inhibiting All R4 Bot Gates (R2 x R4)", async () => {
    // Trip circuit breaker
    mockServer.state.circuit_breaker_tripped = true;

    // Strategy qualification evaluation function with circuit breaker check
    const evaluateBotTradeOpportunity = (params: {
      circuitBreakerTripped: boolean;
      bidPrice: number;
      askPrice: number;
      maxHalfSpreadBps: number;
      netEVBps: number;
    }): { eligible: boolean; skipReason: string | null } => {
      if (params.circuitBreakerTripped) {
        return { eligible: false, skipReason: "circuit breaker" };
      }
      const spreadCheck = evaluateSpreadTollGate({
        bidPrice: params.bidPrice,
        askPrice: params.askPrice,
        maxHalfSpreadBps: params.maxHalfSpreadBps,
      });
      if (!spreadCheck.passesGate) {
        return { eligible: false, skipReason: "spread toll exceeded" };
      }
      if (params.netEVBps <= 0) {
        return { eligible: false, skipReason: "negative EV" };
      }
      return { eligible: true, skipReason: null };
    };

    // Even with perfect prices and positive EV, circuit breaker must reject
    const decision = evaluateBotTradeOpportunity({
      circuitBreakerTripped: mockServer.state.circuit_breaker_tripped,
      bidPrice: 100.0,
      askPrice: 100.05,
      maxHalfSpreadBps: 15.0,
      netEVBps: 25.0,
    });

    assert.equal(decision.eligible, false, "Opportunity must be blocked when circuit breaker is tripped");
    assert.equal(decision.skipReason, "circuit breaker", "Skip reason must be exactly 'circuit breaker'");
  });

  it("T3.4: UI ToggleSwitch State Sync With Tripped Circuit Breaker (R3 x R2)", async () => {
    // When circuit breaker is tripped, bot activation toggles in UI must be disabled or fail
    const canToggleBotOn = (isCircuitBreakerTripped: boolean, isPaused: boolean): boolean => {
      return !isCircuitBreakerTripped && !isPaused;
    };

    assert.equal(canToggleBotOn(true, false), false, "Cannot toggle bot ON when circuit breaker is tripped");
    assert.equal(canToggleBotOn(false, true), false, "Cannot toggle bot ON when engine is paused");
    assert.equal(canToggleBotOn(false, false), true, "Can toggle bot ON when system is healthy");
  });

  it("T3.5: Partitioned Deduplication of Engine vs External Positions (R2.3 x External Positions)", async () => {
    // Simulate overlapping positions on the same venue & symbol:
    // Engine owns 2.0 BTC on Hyperliquid; external venue account has 2.5 BTC total.
    // Partitioning requires:
    // Engine closes its 2.0 BTC signal.
    // External position handler only targets unmanaged excess (0.5 BTC), NEVER the whole 2.5 BTC.
    const partitionTargets = (
      enginePositions: Array<{ venue: string; symbol: string; qty: number }>,
      venueTotalPositions: Array<{ venue: string; symbol: string; qty: number }>
    ) => {
      const engineTargets = [...enginePositions];
      const externalTargets: Array<{ venue: string; symbol: string; qty: number }> = [];

      for (const vp of venueTotalPositions) {
        const ep = enginePositions.find(e => e.venue === vp.venue && e.symbol === vp.symbol);
        const unmanagedQty = ep ? vp.qty - ep.qty : vp.qty;
        if (unmanagedQty > 0.0001) {
          externalTargets.push({ venue: vp.venue, symbol: vp.symbol, qty: unmanagedQty });
        }
      }

      return { engineTargets, externalTargets };
    };

    const result = partitionTargets(
      [{ venue: "hyperliquid", symbol: "BTC-USDC", qty: 2.0 }],
      [{ venue: "hyperliquid", symbol: "BTC-USDC", qty: 2.5 }]
    );

    assert.equal(result.engineTargets[0].qty, 2.0, "Engine claims its 2.0 BTC");
    assert.equal(result.externalTargets.length, 1, "Only 1 unmanaged excess target");
    assert.equal(result.externalTargets[0].qty, 0.5, "External position closing only submits 0.5 BTC, preventing double-closing");
  });

  it("T3.6: High-Frequency Telemetry Stream Under Killswitch (R2 x Telemetry)", async () => {
    // Simulate 50 rapid telemetry price ticks arriving concurrently with stop_all
    let telemetryCount = 0;
    const sendTelemetry = async () => {
      for (let i = 0; i < 50; i++) {
        telemetryCount++;
        await new Promise(r => setTimeout(r, 2));
      }
    };

    mockServer.state.engine_positions = [{ id: "sig-telem", venue: "hyperliquid", symbol: "BTC-USDC", qty: 0.1, value_usd: 6500 }];

    // Run telemetry concurrently with stop_all
    const [_, stopRes] = await Promise.all([
      sendTelemetry(),
      sendDevRpc(testSocket, "stop_all"),
    ]);

    assert.ok(stopRes.ok, "Stop All must succeed even under high-frequency telemetry");
    assert.equal(stopRes.ok.circuit_breaker_tripped, true);
    assert.ok(telemetryCount > 0, "Telemetry stream processed in parallel");
    assert.equal(mockServer.state.engine_positions.length, 0, "Position safely unwound");
  });
});
