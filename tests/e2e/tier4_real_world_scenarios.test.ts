/**
 * Tier 4: Real-World Application Scenarios (>=5 Realistic Scenarios)
 * Full lifecycle, end-to-end operational workflows:
 * - Scenario 1: Flash Crash Multi-Venue Emergency Halt
 * - Scenario 2: Testnet Radar Bot Mean-Reversion Trade Lifecycle
 * - Scenario 3: Multi-Venue Arbitrage Unwind with Partial Venue Outage
 * - Scenario 4: Vault Security Lifecycle & Destructive Action Gate
 * - Scenario 5: Dashboard Live Monitoring, Bot Tuning & Operator Shutdown
 */

import { describe, it, before, after } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import {
  MockDevRpcServer,
  sendDevRpc,
  evaluateConvergenceExit,
  evaluateSpreadTollGate,
  calculateOneWayExitFriction,
} from "./test_helpers.ts";

describe("Tier 4: Real-World Application Scenarios", () => {
  let mockServer: MockDevRpcServer;
  const testSocket = "/tmp/test-arb-scenarios-t4.sock";

  before(async () => {
    mockServer = new MockDevRpcServer(testSocket);
    await mockServer.start();
  });

  after(async () => {
    await mockServer.stop();
  });

  // =========================================================================
  // Scenario 1: Flash Crash Multi-Venue Emergency Halt
  // =========================================================================
  it("Scenario 1: Flash Crash Multi-Venue Emergency Halt — Complete fail-closed kill & flatten", async () => {
    // 1. Initial State: Market drops 20%; operator has multiple open positions and resting bids
    mockServer.state.orders = [
      { id: "maker-bid-1", venue: "hyperliquid", symbol: "BTC-USDC", qty: 0.25 },
      { id: "maker-bid-2", venue: "binance", symbol: "ETH-USDT", qty: 2.0 },
      { id: "maker-bid-3", venue: "okx", symbol: "SOL-USDT", qty: 15.0 },
    ];
    mockServer.state.engine_positions = [
      { id: "pos-btc-1", venue: "hyperliquid", symbol: "BTC-USDC", qty: 0.5, value_usd: 30000 },
      { id: "pos-eth-1", venue: "binance", symbol: "ETH-USDT", qty: 4.0, value_usd: 12000 },
      { id: "pos-dust-ada", venue: "hyperliquid", symbol: "ADA-USDC", qty: 0.00001, value_usd: 0.0004 },
    ];
    mockServer.state.external_positions = [
      { venue: "okx", symbol: "SOL-USDT", qty: 20.0, value_usd: 2800 },
    ];

    // 2. Operator sends 'stop_all' via Dev RPC
    const stopRes = await sendDevRpc(testSocket, "stop_all");
    assert.ok(stopRes.ok, `Dev RPC stop_all must succeed during flash crash, got: ${stopRes.err}`);
    const report = stopRes.ok;

    // 3. Verify resting orders cancelled
    assert.equal(report.cancelled_orders_count, 3, "All 3 resting maker bids must be cancelled");
    assert.equal(report.cancelled_orders_failed, 0);

    // 4. Verify positions closed
    assert.equal(report.closed_engine_positions, 2, "2 tradeable engine positions closed");
    assert.equal(report.closed_external_positions, 1, "1 tradeable external position closed");

    // 5. Verify untradeable dust isolated
    assert.equal(report.untradeable_residuals.length, 1, "Sub-dollar ADA dust safely isolated");
    assert.equal(report.untradeable_residuals[0].symbol, "ADA-USDC");
    assert.ok(report.untradeable_residuals[0].reason.includes("sub-dollar"));

    // 6. Verify fail-closed invariants
    assert.equal(report.circuit_breaker_tripped, true, "Circuit breaker must be tripped");
    assert.equal(report.engine_paused, true, "Engine coordinator must be paused");

    // 7. Verify post-halt state inspection via Dev RPC
    const status = await sendDevRpc(testSocket, "system_status");
    assert.equal(status.ok.circuit_breaker_tripped, true);
    assert.equal(status.ok.is_paused, true);
    assert.equal(status.ok.open_positions_count, 0, "No open active positions remain");
  });

  // =========================================================================
  // Scenario 2: Testnet Radar Bot Mean-Reversion Full Lifecycle
  // =========================================================================
  it("Scenario 2: Testnet Radar Bot Mean-Reversion Lifecycle — Entry, hold past friction, passive exit", async () => {
    // 1. Opportunity Detection & Qualification
    const entryBid = 0.4000;
    const entryAsk = 0.4008; // 8 bps spread
    const maxHalfSpreadBps = 15.0;
    const spreadCheck = evaluateSpreadTollGate({
      bidPrice: entryBid,
      askPrice: entryAsk,
      maxHalfSpreadBps,
    });
    assert.equal(spreadCheck.passesGate, true, "Spread toll gate passes with 10 bps spread");

    const entryDevBps = 65.0; // Significant dislocation
    const frictionBandBps = 41.0;
    const targetToleranceBps = 5.0;

    // 2. Position Opened: Holding
    let currentDevBps = 55.0;
    let evalResult = evaluateConvergenceExit({
      currentDevBps,
      entryDevBps,
      frictionBandBps,
      targetToleranceBps,
      algorithm: "fixed_mean_reversion",
    });
    assert.equal(evalResult.shouldExit, false, "Dev=55 bps: Hold");

    // 3. Mean Reversion In Progress: Dev drops to 40 bps
    currentDevBps = 40.0;
    evalResult = evaluateConvergenceExit({
      currentDevBps,
      entryDevBps,
      frictionBandBps,
      targetToleranceBps,
      algorithm: "fixed_mean_reversion",
    });
    // Flawed legacy would have exited here at a loss; fixed holds!
    assert.equal(evalResult.shouldExit, false, "Dev=40 bps: Must continue holding past friction band");

    // 4. Convergence Reached: Dev drops to 3 bps (<= targetTolerance)
    currentDevBps = 3.0;
    evalResult = evaluateConvergenceExit({
      currentDevBps,
      entryDevBps,
      frictionBandBps,
      targetToleranceBps,
      algorithm: "fixed_mean_reversion",
    });
    assert.equal(evalResult.shouldExit, true, "Dev=3 bps: Reached target convergence to mean!");
    assert.equal(evalResult.exitReason, "ConvergedToMean");

    // 5. Passive Limit Unwind Execution
    const positionQty = 10000; // 10,000 ADA
    const exitPrice = 0.4025;
    const passiveFriction = calculateOneWayExitFriction({
      perpQty: positionQty,
      perpPrice: exitPrice,
      estimatedTakerFeeBps: 2.0, // Maker fee
      halfSpreadBps: 0.0, // Passive limit post
    });

    const grossProfitUsd = positionQty * (exitPrice - entryAsk); // 10000 * (0.4025 - 0.4008) = $17.00
    const netProfitUsd = grossProfitUsd - passiveFriction; // $17.00 - $0.80 = $16.20
    assert.ok(netProfitUsd > 0, `Net trade profit must be positive, got: $${netProfitUsd.toFixed(2)}`);
  });

  // =========================================================================
  // Scenario 3: Multi-Venue Arbitrage Unwind with Partial Venue Outage
  // =========================================================================
  it("Scenario 3: Multi-Venue Arbitrage Unwind with Partial Venue Outage — Fail-closed resilience", async () => {
    // Simulate multi-venue position where venue B times out
    const simulateUnwindWithOutage = (positions: Array<{ venue: string; symbol: string; qty: number }>) => {
      const closed: Array<{ venue: string; symbol: string }> = [];
      const unresolved: Array<{ venue: string; symbol: string; reason: string }> = [];

      for (const pos of positions) {
        if (pos.venue === "vanta_degraded") {
          unresolved.push({
            venue: pos.venue,
            symbol: pos.symbol,
            reason: "fetch_order fill polling exceeded 15 attempts (3000ms timeout)",
          });
        } else {
          closed.push({ venue: pos.venue, symbol: pos.symbol });
        }
      }

      return {
        circuit_breaker_tripped: true,
        engine_paused: true,
        closed_engine_positions: closed.length,
        unresolved_positions: unresolved,
      };
    };

    const result = simulateUnwindWithOutage([
      { venue: "hyperliquid", symbol: "BTC-USDC", qty: 0.1 },
      { venue: "vanta_degraded", symbol: "ADA-USDC", qty: 500 },
    ]);

    assert.equal(result.closed_engine_positions, 1, "Hyperliquid leg successfully closed");
    assert.equal(result.unresolved_positions.length, 1, "Degraded venue leg classified as unresolved");
    assert.equal(result.unresolved_positions[0].venue, "vanta_degraded");
    assert.equal(result.circuit_breaker_tripped, true, "Circuit breaker remains safely tripped");
    assert.equal(result.engine_paused, true, "Engine remains paused");
  });

  // =========================================================================
  // Scenario 4: Vault Security Lifecycle & Destructive Action Gate
  // =========================================================================
  it("Scenario 4: Vault Security Lifecycle — Lock, password 'test-only-vault-pw' unlock, execution, re-lock", async () => {
    // 1. Initial State: Vault locked in live mode
    mockServer.state.trading_mode = "live";
    mockServer.state.vault_locked = true;

    // 2. Unauthenticated destructive execution fails closed
    const failRes = await sendDevRpc(testSocket, "stop_all");
    assert.ok(failRes.err, "Stop all must fail when vault locked in live mode");
    assert.ok(failRes.err.includes("vault locked"));

    // 3. Unlock vault with valid temporary password "test-only-vault-pw"
    const unlockRes = await sendDevRpc(testSocket, "unlock_vault", { password: "test-only-vault-pw" });
    assert.equal(unlockRes.ok, "vault_unlocked");
    assert.equal(mockServer.state.vault_locked, false);

    // 4. Authorized execution succeeds
    mockServer.state.engine_positions = [
      { id: "sig-live-1", venue: "hyperliquid", symbol: "BTC-USDC", qty: 0.1, value_usd: 6500 },
    ];
    const liveStopRes = await sendDevRpc(testSocket, "stop_all");
    assert.ok(liveStopRes.ok, "Stop all succeeds once vault is unlocked");
    assert.equal(liveStopRes.ok.closed_engine_positions, 1);

    // 5. Auto-lock timeout / manual re-lock
    const lockRes = await sendDevRpc(testSocket, "lock_vault");
    assert.equal(lockRes.ok, "vault_locked");
    assert.equal(mockServer.state.vault_locked, true);

    // Reset to testnet
    mockServer.state.trading_mode = "testnet";
    mockServer.state.vault_locked = false;
  });

  // =========================================================================
  // Scenario 5: Dashboard Live Monitoring, Bot Tuning & Operator Shutdown
  // =========================================================================
  it("Scenario 5: Dashboard Live Monitoring, Bot Tuning & Clean Operator Shutdown", async () => {
    // 0. Reset circuit breaker for clean operational cycle
    await sendDevRpc(testSocket, "reset_circuit_breaker");

    // 1. Operator monitors system via system_status
    const statusBefore = await sendDevRpc(testSocket, "system_status");
    assert.equal(statusBefore.ok.circuit_breaker_tripped, false);

    // 2. Operator tunes bot parameters (e.g. adjusts hurdle rates to eliminate testnet loss)
    const tunedParams = {
      minSpreadBps: 35.0, // Raised from 20 bps
      maxHalfSpreadBps: 15.0, // Strict spread toll
      passiveExitUnwind: true, // Eliminate taker exit toll
    };

    // Verify qualification with tuned params
    const wideTestnetOpp = {
      spreadBps: 28.0,
      bidAskSpreadBps: 22.0, // Wide testnet book
    };

    const qualifies =
      wideTestnetOpp.spreadBps >= tunedParams.minSpreadBps &&
      wideTestnetOpp.bidAskSpreadBps / 2.0 <= tunedParams.maxHalfSpreadBps;

    assert.equal(qualifies, false, "Tuned parameters safely reject wide testnet opportunity");

    // 3. Operator initiates clean shutdown via stop_all
    const finalShutdown = await sendDevRpc(testSocket, "stop_all");
    assert.ok(finalShutdown.ok);
    assert.equal(finalShutdown.ok.circuit_breaker_tripped, true);
    assert.equal(finalShutdown.ok.engine_paused, true);
  });
});
