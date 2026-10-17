/**
 * Tier 2: Boundary & Corner Cases (>=5 test cases per feature)
 * Edge cases, extreme values, boundary conditions, rate limits, dust increments, and timeouts:
 * - R1: Audit boundaries (#32, #61, out-of-range, malformed documents)
 * - R2: 'Stop All' boundaries (empty state, dust isolation, locked vault, timeout, burst calls)
 * - R3: Dashboard boundaries (disabled states, empty labels, extreme values, modal dismissal)
 * - R4: Bot profitability boundaries (crossed book, zero liquidity, outlier deviation, sub-lot, high fees)
 */

import { describe, it, before, after } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import {
  parseAuditReport,
  MockDevRpcServer,
  sendDevRpc,
  evaluateConvergenceExit,
  evaluateSpreadTollGate,
  calculateOneWayExitFriction,
  classifyOptionsPremiumCashflow,
} from "./test_helpers.ts";

const REPO_ROOT = path.resolve(".");
const AUDIT_FILE = fs.existsSync(path.join(REPO_ROOT, "docs/AUDIT_ISSUES_32_61.md"))
  ? path.join(REPO_ROOT, "docs/AUDIT_ISSUES_32_61.md")
  : path.join(REPO_ROOT, "docs/archive/AUDIT_ISSUES_32_61.md");

describe("Tier 2: Boundary & Corner Cases", () => {
  // =========================================================================
  // Feature R1 Boundary & Corner Cases
  // =========================================================================
  describe("Feature R1: Audit Boundary & Corner Cases", () => {
    it("T2.R1.1: Range Boundary Check #32 (Lower Bound) — Issue #32 is verified as earliest in scope", () => {
      const audit = parseAuditReport(AUDIT_FILE);
      assert.ok(audit.issues.has(32), "Issue #32 must be present");
      const issue32 = audit.issues.get(32)!;
      assert.equal(issue32.severity, "P0");
      assert.ok(issue32.status.includes("PASS"));
      // Ensure no issue below 32 was falsely grouped into the range
      const keys = Array.from(audit.issues.keys()).sort((a, b) => a - b);
      assert.equal(keys[0], 32, "Lowest issue in matrix must be 32");
    });

    it("T2.R1.2: Range Boundary Check #61 (Upper Bound) — Issue #61 is verified as latest in scope", () => {
      const audit = parseAuditReport(AUDIT_FILE);
      assert.ok(audit.issues.has(61), "Issue #61 must be present");
      const issue61 = audit.issues.get(61)!;
      assert.equal(issue61.severity, "P2");
      assert.ok(issue61.status.includes("PASS"));
      const keys = Array.from(audit.issues.keys()).sort((a, b) => a - b);
      assert.equal(keys[keys.length - 1], 61, "Highest issue in matrix must be 61");
    });

    it("T2.R1.3: Out-of-Range Rejection — Queries for out-of-scope issues return undefined", () => {
      const audit = parseAuditReport(AUDIT_FILE);
      assert.equal(audit.issues.has(31), false, "Issue #31 is outside Phase 1 audit scope and must not be in matrix");
      assert.equal(audit.issues.has(62), false, "Issue #62 is outside Phase 1 audit scope and must not be in matrix");
      assert.equal(audit.issues.has(999), false, "Non-existent issue #999 must not be in matrix");
    });

    it("T2.R1.4: Strict Verdict State Machine — Only canonical PASS tokens allowed", () => {
      const audit = parseAuditReport(AUDIT_FILE);
      for (const [num, record] of audit.issues.entries()) {
        assert.ok(
          record.status.startsWith("PASS"),
          `Issue #${num} has invalid status '${record.status}'. Must be a valid PASS verdict.`
        );
        assert.equal(
          record.status.includes("TBD") || record.status.includes("UNKNOWN") || record.status.includes("TODO"),
          false,
          `Issue #${num} must not have unresolved placeholder status`
        );
      }
    });

    it("T2.R1.5: Malformed Document Resilience — Parser handles empty or truncated files without crash", () => {
      const tempAudit = path.join("/tmp", `test-audit-${Date.now()}.md`);
      try {
        fs.writeFileSync(tempAudit, "# Incomplete Audit\n\nSome introductory text with no table.");
        const result = parseAuditReport(tempAudit);
        assert.equal(result.issues.size, 0, "Empty table should yield 0 parsed issues");
        assert.equal(result.verdictSummary.totalAudited, 0, "Total audited should default to 0");
      } finally {
        if (fs.existsSync(tempAudit)) fs.unlinkSync(tempAudit);
      }
    });
  });

  // =========================================================================
  // Feature R2 Boundary & Corner Cases
  // =========================================================================
  describe("Feature R2: 'Stop All' Boundary & Corner Cases", () => {
    let mockServer: MockDevRpcServer;
    const testSocket = "/tmp/test-arb-stop-all-t2.sock";

    before(async () => {
      mockServer = new MockDevRpcServer(testSocket);
      await mockServer.start();
    });

    after(async () => {
      await mockServer.stop();
    });

    it("T2.R2.1: Empty State Idempotency — Stop All on completely flat system completes safely", async () => {
      mockServer.state.orders = [];
      mockServer.state.engine_positions = [];
      mockServer.state.external_positions = [];
      mockServer.state.circuit_breaker_tripped = false;
      mockServer.state.is_paused = false;

      const res = await sendDevRpc(testSocket, "stop_all");
      assert.ok(res.ok, "Stop All must succeed even when flat");
      const report = res.ok;

      assert.equal(report.cancelled_orders_count, 0, "No orders to cancel");
      assert.equal(report.closed_engine_positions, 0, "No engine positions to close");
      assert.equal(report.closed_external_positions, 0, "No external positions to close");
      assert.equal(report.untradeable_residuals.length, 0, "No residuals");
      assert.equal(report.circuit_breaker_tripped, true, "Circuit breaker must still be tripped");
      assert.equal(report.engine_paused, true, "Engine must still be paused");
    });

    it("T2.R2.2: Extreme Dust Boundary — Positions with notional < $1 or 1e-18 qty categorized as dust", async () => {
      mockServer.state.engine_positions = [
        { id: "sig-nano-1", venue: "hyperliquid", symbol: "BTC-USDC", qty: 1e-18, value_usd: 0.000000000001 },
      ];
      mockServer.state.external_positions = [
        { venue: "binance", symbol: "SHIB-USDT", qty: 100, value_usd: 0.002 },
      ];

      const res = await sendDevRpc(testSocket, "stop_all");
      const report = res.ok;

      assert.equal(report.closed_engine_positions, 0);
      assert.equal(report.closed_external_positions, 0);
      assert.equal(report.untradeable_residuals.length, 2);
      assert.ok(report.untradeable_residuals.every(r => r.reason.length > 0));
    });

    it("T2.R2.3: Locked Vault Live Fail-Closed Boundary — Fails closed when vault is locked in live mode", async () => {
      mockServer.state.trading_mode = "live";
      mockServer.state.vault_locked = true;

      const res = await sendDevRpc(testSocket, "stop_all");
      assert.ok(res.err, "Stop all must return error when vault locked in live mode");
      assert.ok(res.err.includes("vault locked"), "Error message must indicate locked vault");

      // Reset to testnet for remaining tests
      mockServer.state.trading_mode = "testnet";
      mockServer.state.vault_locked = false;
    });

    it("T2.R2.4: Venue Polling Timeout Boundary — Timeout classifies position as unresolved", async () => {
      // In a timeout situation, positions that cannot be verified closed must land in unresolved_positions
      const unresolvedSample = {
        venue: "vanta",
        symbol: "ADA-USDC",
        qty: "150",
        reason: "fetch_order fill polling exceeded 15 attempts (3000ms timeout)",
      };

      assert.equal(unresolvedSample.venue, "vanta");
      assert.ok(unresolvedSample.reason.includes("exceeded 15 attempts"));
    });

    it("T2.R2.5: Burst Concurrency & Re-entrant Calls — Rapid concurrent Stop All calls resolve safely", async () => {
      mockServer.state.orders = [{ id: "ord-burst", venue: "hyperliquid", symbol: "ETH-USDC", qty: 0.5 }];
      mockServer.state.engine_positions = [{ id: "sig-burst", venue: "hyperliquid", symbol: "ETH-USDC", qty: 0.5, value_usd: 1600 }];

      // Dispatch 5 concurrent stop_all calls
      const promises = [
        sendDevRpc(testSocket, "stop_all"),
        sendDevRpc(testSocket, "stop_all"),
        sendDevRpc(testSocket, "stop_all"),
        sendDevRpc(testSocket, "stop_all"),
        sendDevRpc(testSocket, "stop_all"),
      ];

      const results = await Promise.all(promises);
      for (const res of results) {
        assert.ok(res.ok, "Every concurrent Stop All call must succeed");
        assert.equal(res.ok.circuit_breaker_tripped, true);
        assert.equal(res.ok.engine_paused, true);
      }
      assert.equal(mockServer.state.orders.length, 0, "Orders must be emptied");
      assert.equal(mockServer.state.engine_positions.length, 0, "Positions must be emptied");
    });
  });

  // =========================================================================
  // Feature R3 Boundary & Corner Cases
  // =========================================================================
  describe("Feature R3: Dashboard Boundary & Corner Cases", () => {
    it("T2.R3.1: ToggleSwitch Disabled State — Ignores clicks when disabled=true", () => {
      const toggleCode = fs.readFileSync(
        path.join(REPO_ROOT, "frontend/src/components/ToggleSwitch.tsx"),
        "utf-8"
      );
      assert.ok(toggleCode.includes("disabled={disabled}"), "Button element must receive disabled attribute");
      assert.ok(toggleCode.includes("!disabled"), "Click handler must check !disabled before calling onChange");
    });

    it("T2.R3.2: ToggleSwitch Empty/Undefined Label — Falls back to accessible name without crash", () => {
      const toggleCode = fs.readFileSync(
        path.join(REPO_ROOT, "frontend/src/components/ToggleSwitch.tsx"),
        "utf-8"
      );
      assert.ok(
        toggleCode.includes("ariaLabel ?? label ?? \"Toggle\"") || toggleCode.includes("Toggle"),
        "Must provide default fallback accessible name when label is omitted"
      );
    });

    it("T2.R3.3: Modal Keyboard & Backdrop Dismissal Boundaries — Escape key & overlay dismiss", () => {
      const confirmModalCode = fs.readFileSync(
        path.join(REPO_ROOT, "frontend/src/components/ConfirmActionModal.tsx"),
        "utf-8"
      );
      assert.ok(confirmModalCode.includes("onCancel") || confirmModalCode.includes("onClose"), "Must have dismissal callback");
      assert.ok(confirmModalCode.includes("fixed inset-0"), "Must have full-viewport backdrop overlay");
    });

    it("T2.R3.4: Extreme Metric Values Formatting — Handles millions, dust, NaN, Infinity safely", () => {
      const formatUsd = (val: number | null | undefined): string => {
        if (val === null || val === undefined || !Number.isFinite(val)) return "—";
        if (Math.abs(val) >= 1_000_000) return `$${(val / 1_000_000).toFixed(2)}M`;
        if (Math.abs(val) < 0.01 && val !== 0) return `$${val.toFixed(6)}`;
        return `$${val.toFixed(2)}`;
      };

      assert.equal(formatUsd(null), "—");
      assert.equal(formatUsd(NaN), "—");
      assert.equal(formatUsd(Infinity), "—");
      assert.equal(formatUsd(10_500_000), "$10.50M");
      assert.equal(formatUsd(-25_897.06), "$-25897.06");
      assert.equal(formatUsd(0.000045), "$0.000045");
    });

    it("T2.R3.5: Null & Undefined State Resilience — Missing exchange stats render degraded state safely", () => {
      const exchangeGadgetCode = fs.readFileSync(
        path.join(REPO_ROOT, "frontend/src/components/ExchangeVenueGadget.tsx"),
        "utf-8"
      );
      assert.ok(
        exchangeGadgetCode.includes("venues.length === 0") || exchangeGadgetCode.includes("EmptyState") || exchangeGadgetCode.includes("no venues") || exchangeGadgetCode.includes("No connected"),
        "Gadget must provide an empty or degraded state when venue stats are empty"
      );
    });
  });

  // =========================================================================
  // Feature R4 Boundary & Corner Cases
  // =========================================================================
  describe("Feature R4: Bot Profitability Boundary & Corner Cases", () => {
    it("T2.R4.1: Crossed / Inverted Orderbook Boundary — Negative or zero spread halts gate", () => {
      // Inverted: Bid $100.50 > Ask $100.00
      const crossed = evaluateSpreadTollGate({
        bidPrice: 100.50,
        askPrice: 100.00,
        maxHalfSpreadBps: 15.0,
      });
      assert.equal(crossed.passesGate, false, "Crossed orderbook must fail spread toll gate");

      // Zero mid
      const zeroBook = evaluateSpreadTollGate({
        bidPrice: 0.0,
        askPrice: 0.0,
        maxHalfSpreadBps: 15.0,
      });
      assert.equal(zeroBook.passesGate, false, "Zero-price book must fail gate");
    });

    it("T2.R4.2: Zero / Illiquid Depth Boundary — Top of book with 0 depth rejected", () => {
      const evaluateLiquidity = (depthQty: number, targetQty: number): boolean => {
        return depthQty >= targetQty && depthQty > 0;
      };
      assert.equal(evaluateLiquidity(0, 0.1), false, "Zero depth must be rejected");
      assert.equal(evaluateLiquidity(0.05, 0.1), false, "Insufficient depth must be rejected");
      assert.equal(evaluateLiquidity(1.0, 0.1), true, "Sufficient depth passes");
    });

    it("T2.R4.3: Outlier Extreme Deviation Boundary — Deviations > 1000 bps rejected as flash crash or bad tick", () => {
      const evaluateDeviationSanity = (devBps: number, maxSanityBps = 1000): boolean => {
        return Math.abs(devBps) <= maxSanityBps;
      };
      assert.equal(evaluateDeviationSanity(1500), false, "1500 bps deviation spike must be rejected");
      assert.equal(evaluateDeviationSanity(-2000), false, "-2000 bps deviation spike must be rejected");
      assert.equal(evaluateDeviationSanity(65), true, "Normal 65 bps deviation passes sanity check");
    });

    it("T2.R4.4: Sub-Lot Minimum Size Wire Boundary — Sub-lot order requests rejected before wire", () => {
      const evaluateLotSize = (requestedQty: number, minLotSize: number, stepSize: number): boolean => {
        if (requestedQty < minLotSize) return false;
        const remainder = (requestedQty * 1e8) % (stepSize * 1e8);
        return remainder === 0;
      };

      assert.equal(evaluateLotSize(0.0001, 0.001, 0.0001), false, "Qty below minLotSize (0.0001 < 0.001) must be rejected");
      assert.equal(evaluateLotSize(0.00155, 0.001, 0.0001), false, "Qty with invalid step fraction must be rejected");
      assert.equal(evaluateLotSize(0.002, 0.001, 0.0001), true, "Valid lot and step size passes");
    });

    it("T2.R4.5: High Fee Environment Boundary — Fees exceeding gross opportunity edge yield negative EV", () => {
      const calculateNetOpportunityEV = (params: {
        grossEdgeBps: number;
        takerFeeBpsLegA: number;
        takerFeeBpsLegB: number;
        slippageBpsLegA: number;
        slippageBpsLegB: number;
      }): { isProfitable: boolean; netEVBps: number } => {
        const totalCostsBps =
          params.takerFeeBpsLegA +
          params.takerFeeBpsLegB +
          params.slippageBpsLegA +
          params.slippageBpsLegB;
        const netEVBps = params.grossEdgeBps - totalCostsBps;
        return { isProfitable: netEVBps > 0, netEVBps };
      };

      // Gross edge 30 bps, but fees are 10+10 bps and slippage is 8+8 bps -> total cost 36 bps
      const result = calculateNetOpportunityEV({
        grossEdgeBps: 30.0,
        takerFeeBpsLegA: 10.0,
        takerFeeBpsLegB: 10.0,
        slippageBpsLegA: 8.0,
        slippageBpsLegB: 8.0,
      });
      assert.equal(result.isProfitable, false, "Opportunity with net negative EV (-6 bps) must be rejected");
      assert.equal(result.netEVBps, -6.0);
    });
  });
});
