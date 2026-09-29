/**
 * E2E Test Suite Helpers & Public Interface Contracts
 * Arbitrageur Algorithmic Trading Platform
 */

import fs from "node:fs";
import path from "node:path";
import net from "node:net";

export interface EmergencyShutdownReportDto {
  circuit_breaker_tripped: boolean;
  engine_paused: boolean;
  cancelled_orders_count: number;
  cancelled_orders_failed: number;
  closed_engine_positions: number;
  closed_external_positions: number;
  untradeable_residuals: UntradeableResidualDto[];
  unresolved_positions: UnresolvedPositionDto[];
  timestamp_ms: number;
}

export interface UntradeableResidualDto {
  venue: string;
  symbol: string;
  raw_qty: string;
  reason: string;
}

export interface UnresolvedPositionDto {
  venue: string;
  symbol: string;
  qty: string;
  reason: string;
}

export interface ToggleSwitchProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  label?: string;
  size?: 'sm' | 'md';
  variant?: 'primary' | 'emerald';
  id?: string;
}

export interface AuditRecord {
  issueNumber: number;
  severity: string;
  component: string;
  summary: string;
  commit: string;
  keyFiles: string[];
  status: string;
}

export interface SystemStatusDto {
  circuit_breaker_tripped: boolean;
  is_paused: boolean;
  trading_mode: 'testnet' | 'live';
  vault_locked: boolean;
  active_venues_count: number;
  open_positions_count: number;
}

/**
 * Parses docs/AUDIT_ISSUES_32_61.md and extracts all issue entries
 */
export function parseAuditReport(filePath: string): {
  issues: Map<number, AuditRecord>;
  criticalDomainsVerified: string[];
  verdictSummary: { totalAudited: number; fullyPresent: number; regressed: number };
} {
  const content = fs.readFileSync(filePath, "utf-8");
  const issues = new Map<number, AuditRecord>();

  // Extract table rows: | **#32** | P0 | Adapters / DexPerp | ... | `d58194d` | ... | **PASS (Fully Present)** |
  for (const line of content.split("\n")) {
    const trimmed = line.trim();
    if (trimmed.startsWith("| **#")) {
      const parts = trimmed.split("|").map(p => p.trim());
      // parts[0] is empty, parts[1] is "**#32**", parts[2] is "P0", parts[3] is component, etc.
      const numMatch = parts[1].match(/#(\d+)/);
      if (numMatch && parts.length >= 8) {
        const num = parseInt(numMatch[1], 10);
        issues.set(num, {
          issueNumber: num,
          severity: parts[2],
          component: parts[3],
          summary: parts[4],
          commit: parts[5].replace(/`/g, ""),
          keyFiles: parts[6].split(",").map(s => s.trim().replace(/`/g, "")),
          status: parts[7].replace(/\*\*/g, ""),
        });
      }
    }
  }

  // Detect critical domains
  const criticalDomains: string[] = [];
  if (content.includes("Partial-fill trim accounting") || content.includes("Partial-Fill Trim")) {
    criticalDomains.push("partial_fill_trim");
  }
  if (content.includes("Order truth") || content.includes("order truth")) {
    criticalDomains.push("order_truth_polling");
  }
  if (content.includes("Fee deductions") || content.includes("fee deductions")) {
    criticalDomains.push("fee_deductions_gas");
  }
  if (content.includes("Circuit breaker durability") || content.includes("circuit breaker durability")) {
    criticalDomains.push("circuit_breaker_retention");
  }
  if (content.includes("EVM AMM exact-amount") || content.includes("EVM AMM")) {
    criticalDomains.push("evm_amm_exact_amount");
  }

  // Detect verdict summary counts
  const totalAuditedMatch = content.match(/Total Issues Audited.*:\*\*\s*(\d+)/i);
  const fullyPresentMatch = content.match(/Fully Implemented & Verified Present.*:\*\*\s*(\d+)/i);
  const regressedMatch = content.match(/Partially Implemented \/ Regressed.*:\*\*\s*(\d+)/i);

  return {
    issues,
    criticalDomainsVerified: criticalDomains,
    verdictSummary: {
      totalAudited: totalAuditedMatch ? parseInt(totalAuditedMatch[1], 10) : issues.size,
      fullyPresent: fullyPresentMatch ? parseInt(fullyPresentMatch[1], 10) : issues.size,
      regressed: regressedMatch ? parseInt(regressedMatch[1], 10) : 0,
    },
  };
}

/**
 * Mock Dev RPC Server for isolated, self-contained E2E testing
 */
export class MockDevRpcServer {
  public server: net.Server;
  public socketPath: string;
  public state: {
    circuit_breaker_tripped: boolean;
    is_paused: boolean;
    vault_locked: boolean;
    trading_mode: 'testnet' | 'live';
    orders: Array<{ id: string; venue: string; symbol: string; qty: number }>;
    engine_positions: Array<{ id: string; venue: string; symbol: string; qty: number; value_usd: number }>;
    external_positions: Array<{ venue: string; symbol: string; qty: number; value_usd: number }>;
  };

  constructor(socketPath: string) {
    this.socketPath = socketPath;
    this.state = {
      circuit_breaker_tripped: false,
      is_paused: false,
      vault_locked: false,
      trading_mode: 'testnet',
      orders: [],
      engine_positions: [],
      external_positions: [],
    };

    if (fs.existsSync(socketPath)) {
      try {
        fs.unlinkSync(socketPath);
      } catch {}
    }

    this.server = net.createServer((socket) => {
      let buffer = "";
      socket.on("data", (chunk) => {
        buffer += chunk.toString();
        const lines = buffer.split("\n");
        buffer = lines.pop() || "";
        for (const line of lines) {
          if (!line.trim()) continue;
          try {
            const req = JSON.parse(line);
            const res = this.handleCommand(req);
            socket.write(JSON.stringify(res) + "\n");
          } catch (e: any) {
            socket.write(JSON.stringify({ err: `bad json: ${e.message}` }) + "\n");
          }
        }
      });
    });
  }

  public start(): Promise<void> {
    return new Promise((resolve, reject) => {
      this.server.listen(this.socketPath, () => {
        resolve();
      });
      this.server.on("error", reject);
    });
  }

  public stop(): Promise<void> {
    return new Promise((resolve) => {
      this.server.close(() => {
        if (fs.existsSync(this.socketPath)) {
          try {
            fs.unlinkSync(this.socketPath);
          } catch {}
        }
        resolve();
      });
    });
  }

  public handleCommand(req: { id?: any; cmd: string; args?: any }): any {
    const id = req.id ?? null;
    const cmd = req.cmd;
    const args = req.args || {};

    switch (cmd) {
      case "system_status": {
        return {
          id,
          ok: {
            circuit_breaker_tripped: this.state.circuit_breaker_tripped,
            is_paused: this.state.is_paused,
            trading_mode: this.state.trading_mode,
            vault_locked: this.state.vault_locked,
            active_venues_count: 5,
            open_positions_count: this.state.engine_positions.length + this.state.external_positions.length,
          },
        };
      }
      case "unlock_vault": {
        const pw = args.password || "";
        if (pw === "000000") {
          this.state.vault_locked = false;
          return { id, ok: "vault_unlocked" };
        }
        return { id, err: "invalid password" };
      }
      case "lock_vault": {
        this.state.vault_locked = true;
        return { id, ok: "vault_locked" };
      }
      case "reset_circuit_breaker": {
        this.state.circuit_breaker_tripped = false;
        return { id, ok: "circuit_breaker_reset" };
      }
      case "open_positions": {
        return { id, ok: this.state.engine_positions };
      }
      case "external_positions": {
        return { id, ok: this.state.external_positions };
      }
      case "stop_all":
      case "killswitch": {
        // 1. Trip circuit breaker and pause engine
        this.state.circuit_breaker_tripped = true;
        this.state.is_paused = true;

        if (this.state.trading_mode === "live" && this.state.vault_locked) {
          return {
            id,
            err: "vault locked: cannot dispatch order cancels or position unwinds in live mode",
          };
        }

        // 2. Concurrently cancel resting orders
        const cancelledCount = this.state.orders.length;
        this.state.orders = [];

        // 3. Partition and unwind open positions
        const untradeable_residuals: UntradeableResidualDto[] = [];
        let closedEngineCount = 0;
        let closedExternalCount = 0;

        for (const pos of this.state.engine_positions) {
          if (pos.value_usd < 1.0 || pos.qty < 0.0001) {
            untradeable_residuals.push({
              venue: pos.venue,
              symbol: pos.symbol,
              raw_qty: pos.qty.toString(),
              reason: "sub-dollar notional under $1.00",
            });
          } else {
            closedEngineCount++;
          }
        }
        this.state.engine_positions = [];

        for (const pos of this.state.external_positions) {
          if (pos.value_usd < 1.0 || pos.qty < 0.0001) {
            untradeable_residuals.push({
              venue: pos.venue,
              symbol: pos.symbol,
              raw_qty: pos.qty.toString(),
              reason: "sub-lot increment residual",
            });
          } else {
            closedExternalCount++;
          }
        }
        this.state.external_positions = [];

        const report: EmergencyShutdownReportDto = {
          circuit_breaker_tripped: true,
          engine_paused: true,
          cancelled_orders_count: cancelledCount,
          cancelled_orders_failed: 0,
          closed_engine_positions: closedEngineCount,
          closed_external_positions: closedExternalCount,
          untradeable_residuals,
          unresolved_positions: [],
          timestamp_ms: Date.now(),
        };

        return { id, ok: report };
      }
      default:
        return { id, err: `unknown command '${cmd}'` };
    }
  }
}

/**
 * Dev RPC Client using Unix Domain Sockets
 */
export async function sendDevRpc(
  socketPath: string,
  cmd: string,
  args: Record<string, any> = {}
): Promise<any> {
  return new Promise((resolve, reject) => {
    const client = net.createConnection(socketPath);
    let buffer = "";

    client.on("connect", () => {
      client.write(JSON.stringify({ id: 1, cmd, args }) + "\n");
    });

    client.on("data", (chunk) => {
      buffer += chunk.toString();
      if (buffer.includes("\n")) {
        try {
          const res = JSON.parse(buffer.trim());
          client.end();
          resolve(res);
        } catch (e) {
          client.destroy();
          reject(e);
        }
      }
    });

    client.on("error", (err) => {
      reject(err);
    });

    client.setTimeout(2500, () => {
      client.destroy();
      reject(new Error("Dev RPC socket timeout"));
    });
  });
}

// ── Quantitative Math Models (R4 Bot Profitability) ──────────────────────────

/**
 * Correct mean reversion convergence exit evaluator.
 * Premature exit when |dev| <= friction_band is incorrect because it exits before the spread
 * returns to mean. True convergence exit requires dev to revert to mean (dev.abs() <= target_tolerance
 * or cross zero).
 */
export function evaluateConvergenceExit(params: {
  currentDevBps: number;
  entryDevBps: number;
  frictionBandBps: number;
  targetToleranceBps: number;
  algorithm: 'legacy_friction_band' | 'fixed_mean_reversion';
}): { shouldExit: boolean; exitReason: string | null } {
  const { currentDevBps, entryDevBps, frictionBandBps, targetToleranceBps, algorithm } = params;

  if (algorithm === 'legacy_friction_band') {
    // Buggy implementation in convergence_auto.rs line 188
    if (Math.abs(currentDevBps) <= frictionBandBps) {
      return { shouldExit: true, exitReason: 'ConvergedPremature' };
    }
    return { shouldExit: false, exitReason: null };
  } else {
    // Fixed implementation: exit only when dev reverts past the target tolerance towards 0
    // or when the sign flips (crosses the mean)
    const crossedZero = (entryDevBps > 0 && currentDevBps <= 0) || (entryDevBps < 0 && currentDevBps >= 0);
    const reachedTarget = Math.abs(currentDevBps) <= targetToleranceBps;

    if (crossedZero || reachedTarget) {
      return { shouldExit: true, exitReason: 'ConvergedToMean' };
    }
    return { shouldExit: false, exitReason: null };
  }
}

/**
 * Spread-toll ceiling gate:
 * Checks whether bid-ask half-spread is within safe bounds (< max_half_spread_bps)
 */
export function evaluateSpreadTollGate(params: {
  bidPrice: number;
  askPrice: number;
  maxHalfSpreadBps: number;
}): { passesGate: boolean; halfSpreadBps: number } {
  const mid = (params.bidPrice + params.askPrice) / 2.0;
  if (mid <= 0 || params.askPrice < params.bidPrice) {
    return { passesGate: false, halfSpreadBps: Infinity };
  }
  const halfSpreadBps = ((params.askPrice - params.bidPrice) / (2.0 * mid)) * 10000;
  return {
    passesGate: halfSpreadBps <= params.maxHalfSpreadBps,
    halfSpreadBps,
  };
}

/**
 * One-way mode exit cost estimator
 */
export function calculateOneWayExitFriction(params: {
  perpQty: number;
  perpPrice: number;
  estimatedTakerFeeBps: number;
  halfSpreadBps: number;
}): number {
  const notional = params.perpQty * params.perpPrice;
  const frictionBps = params.estimatedTakerFeeBps + params.halfSpreadBps;
  return notional * (frictionBps / 10000);
}

/**
 * Options premium cashflow classifier:
 * Debit outflow is asset inventory purchase, NOT instant realized loss
 */
export function classifyOptionsPremiumCashflow(debitAmountUsd: number): {
  realizedPnlDelta: number;
  inventoryBookValue: number;
  isRealizedLoss: boolean;
} {
  return {
    realizedPnlDelta: 0.0, // Should NOT impact realized PnL on open
    inventoryBookValue: debitAmountUsd, // Booked as asset acquisition
    isRealizedLoss: false,
  };
}
