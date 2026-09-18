import React, { useState, useMemo, useEffect, useRef, useCallback } from "react";
import {
  Search,
  Trash2,
  Activity,
} from "lucide-react";
import type { ExecutionLogEntry, TickerStrategyConfigDto, VenueOrderBookDto } from "../types";
import { formatCurrencyPnl, formatCurrencyPrice } from "../types/tickers";
import { EXCHANGES_ALPHABETICAL } from "./EngineSettings";
import { EmptyState } from "./EmptyState";

export interface LogItem {
  id: string;
  timestamp: string;
  epochMs: number;
  level: "EXEC" | "TRIGGER" | "EVAL" | "SCAN" | "WARN" | "ERROR";
  source: string;
  message: string;
  route?: string;
  spreadBps?: number;
  netPnl?: number;
  status: "SUCCESS" | "INFO" | "REJECTED" | "WARNING" | "TRIPPED";
}

interface TickerLogMonitorProps {
  symbol: string;
  strategyName?: string;
  config?: TickerStrategyConfigDto;
  executionLogs?: ExecutionLogEntry[];
  orderbooks?: VenueOrderBookDto[];
  autoBotEnabled?: boolean;
  isPaused?: boolean;
  /** Skip symbol matching (caller pre-filtered, e.g. multi-ticker strategies). */
  matchAllSymbols?: boolean;
  /** Storage/display namespace override (defaults to symbol). */
  namespace?: string;
}

const getLevelBadgeClass = (level: LogItem["level"]) => {
  switch (level) {
    case "EXEC":
      return "bg-profit-dim text-profit border-profit-border";
    case "TRIGGER":
      return "bg-blue-50 text-accent border-blue-200";
    case "EVAL":
      return "bg-indigo-50 text-indigo-700 border-indigo-200";
    case "SCAN":
      return "bg-slate-100 text-slate-700 border-slate-200";
    case "WARN":
      return "bg-warn-dim text-warn border-warn-border";
    case "ERROR":
      return "bg-loss-dim text-loss border-loss-border";
    default:
      return "bg-surface-raised text-slate-600 border-surface-border";
  }
};

interface LogRowProps {
  id: string;
  timestamp: string;
  level: LogItem["level"];
  source: string;
  route: string | undefined;
  message: string;
  netPnl: number | undefined;
  status: LogItem["status"];
}

const LogRow: React.FC<LogRowProps> = React.memo(
  ({ timestamp, level, source, route, message, netPnl, status }) => (
    <tr className="hover:bg-surface-raised/60 transition">
      <td className="py-2 px-3 text-slate-500 whitespace-nowrap text-[11px] tabular-nums">
        {timestamp}
      </td>
      <td className="py-2 px-2.5 whitespace-nowrap">
        <span
          className={`px-2 py-0.5 rounded-md text-[11px] font-bold border ${getLevelBadgeClass(
            level
          )}`}
        >
          {level.charAt(0) + level.slice(1).toLowerCase()}
        </span>
      </td>
      <td className="py-2 px-3 font-semibold text-slate-900 whitespace-nowrap">
        {route ? (
          <span className="flex items-center space-x-1 text-slate-800">
            <span>{route}</span>
          </span>
        ) : (
          <span className="text-slate-600">{source}</span>
        )}
      </td>
      <td className="py-2 px-3 text-slate-700 leading-relaxed">
        <span>{message}</span>
        {netPnl !== undefined && (
          <span
            className={`ml-1.5 font-bold tabular-nums ${
              netPnl >= 0 ? "text-profit" : "text-loss"
            }`}
          >
            ({formatCurrencyPnl(netPnl)})
          </span>
        )}
      </td>
      <td className="py-2 px-3 text-right whitespace-nowrap">
        <span
          className={`text-[11px] font-semibold ${
            status === "SUCCESS"
              ? "text-profit"
              : status === "WARNING"
              ? "text-warn"
              : status === "REJECTED"
              ? "text-loss"
              : "text-slate-500"
          }`}
        >
          {status}
        </span>
      </td>
    </tr>
  ),
);

export const TickerLogMonitor: React.FC<TickerLogMonitorProps> = React.memo(TickerLogMonitorInner);

function TickerLogMonitorInner({
  symbol,
  strategyName = "Single Spread",
  config,
  executionLogs = [],
  orderbooks = [],
  autoBotEnabled = false,
  isPaused = false,
  matchAllSymbols = false,
  namespace,
}: TickerLogMonitorProps) {
  const [levelFilter, setLevelFilter] = useState<string>("ALL");
  const [logDepth, setLogDepth] = useState<string>("100");
  const [searchQuery, setSearchQuery] = useState<string>("");
  const [autoScroll, setAutoScroll] = useState<boolean>(true);
  const [runtimeLogs, setRuntimeLogs] = useState<LogItem[]>([]);
  const scrollBottomRef = useRef<HTMLDivElement>(null);
  const storageKey = `ticker-log-monitor:${namespace ?? symbol}`;
  // Heartbeat entries buffer here and flush to state at most once per
  // second — the 3s orderbook poll must not setState per tick.
  const pendingRef = useRef<LogItem[]>([]);

  // Load persisted telemetry stream for this ticker (survives remounts/restarts)
  useEffect(() => {
    pendingRef.current = [];
    try {
      const raw = localStorage.getItem(storageKey);
      const parsed = raw ? (JSON.parse(raw) as LogItem[]) : [];
      parsed.sort((a, b) => a.epochMs - b.epochMs);
      setRuntimeLogs(parsed.slice(-500));
    } catch {
      setRuntimeLogs([]);
    }
  }, [storageKey]);

  // Persist the telemetry stream automatically (capped) — the engine creates
  // and stores logs continuously; there is no manual scanning mode.
  useEffect(() => {
    try {
      localStorage.setItem(storageKey, JSON.stringify(runtimeLogs.slice(-500)));
    } catch {
      // Storage quota exceeded — telemetry remains visible but unpersisted
    }
  }, [runtimeLogs, storageKey]);

  // Convert backend execution logs into structured log items for this ticker
  const executionItems = useMemo<LogItem[]>(() => {
    const symKey = symbol.toUpperCase();
    const baseKey = symKey.split(/[-_/]/)[0];

    return executionLogs
      .filter((log) => {
        if (matchAllSymbols) return true;
        if (!log.symbol) return false;
        const logKey = log.symbol.toUpperCase();
        const logBase = logKey.split(/[-_/]/)[0];
        return logKey === symKey || logBase === baseKey;
      })
      .map((log) => {
        const isConvergence = Boolean(log.direction && log.direction.toLowerCase().includes("convergence"));
        const level = log.success ? "EXEC" : "ERROR";
        const status = log.success ? "SUCCESS" : "REJECTED";
        const route = `${log.venue_a || "Venue A"} ➔ ${log.venue_b || "Venue B"}`;
        const pnlStr = log.net_profit !== undefined ? ` Net PnL: ${formatCurrencyPnl(log.net_profit)}` : "";
        const spreadStr = log.trigger_spread_bps !== undefined ? ` [Spread: +${log.trigger_spread_bps.toFixed(1)} BPS]` : "";

        return {
          id: `exec-${log.id}`,
          timestamp: log.timestamp || new Date().toISOString(),
          epochMs: new Date(log.timestamp || Date.now()).getTime(),
          level,
          source: isConvergence ? "Convergence Unwind" : "Atomic Arbitrage",
          message: `${isConvergence ? "Unwound convergence leg" : "Filled atomic arbitrage order"}.${spreadStr}${pnlStr} — ${log.note || "Order executed successfully"}`,
          route,
          spreadBps: log.trigger_spread_bps,
          netPnl: log.net_profit,
          status,
        };
      });
  }, [executionLogs, symbol, matchAllSymbols]);

  // Generate live strategy telemetry heartbeat logs on ticker / orderbook updates
  useEffect(() => {
    if (orderbooks.length === 0) return;
    if (typeof document !== "undefined" && document.hidden) return;

    const now = new Date();
    const timestamp = now.toISOString().replace("T", " ").substring(0, 23);
    const epochMs = now.getTime();

    const newEntries: LogItem[] = [];

    // Find best bid and best ask for live spread evaluation
    let bestBid = -Infinity;
    let bestAsk = Infinity;
    let bestBidVenue = "";
    let bestAskVenue = "";

    for (const b of orderbooks) {
      if (b.best_bid > bestBid) {
        bestBid = b.best_bid;
        bestBidVenue = b.venue_name;
      }
      if (b.best_ask > 0 && b.best_ask < bestAsk) {
        bestAsk = b.best_ask;
        bestAskVenue = b.venue_name;
      }
    }

    const grossSpreadBps = bestAsk > 0 && bestBid > 0 ? ((bestBid - bestAsk) / bestAsk) * 10000 : 0;
    // Round-trip fees come from venue metadata; unresolved routes report gross only.
    const feeBpsFor = (venueName: string): number | null => {
      const key = venueName.toLowerCase();
      const meta = EXCHANGES_ALPHABETICAL.find(
        (e) => key.includes(e.name.toLowerCase()) || e.name.toLowerCase().includes(key),
      );
      return meta ? meta.feeBps : null;
    };
    const askFee = feeBpsFor(bestAskVenue);
    const bidFee = feeBpsFor(bestBidVenue);
    const estFees = askFee !== null && bidFee !== null ? askFee + bidFee : null;
    const netSpreadBps = estFees !== null ? grossSpreadBps - estFees : grossSpreadBps;
    const hurdleBps = (config?.targetProfitBps ?? 1.0) + (estFees ?? 0);
    const spreadNote =
      estFees !== null
        ? `Net spread: ${netSpreadBps.toFixed(1)} BPS (hurdle: ${hurdleBps.toFixed(1)} BPS)`
        : `Gross spread: ${grossSpreadBps.toFixed(1)} BPS (route fees unresolved)`;

    if (isPaused) {
      newEntries.push({
        id: `paused-${epochMs}`,
        timestamp,
        epochMs,
        level: "WARN",
        source: "Engine Core",
        message: `Trading currently PAUSED. Strategy executions suspended. All positions and margin preserved safely.`,
        status: "WARNING",
      });
    } else if (estFees !== null && netSpreadBps >= (config?.targetProfitBps ?? 1.0)) {
      newEntries.push({
        id: `eval-${epochMs}`,
        timestamp,
        epochMs,
        level: autoBotEnabled ? "TRIGGER" : "EVAL",
        source: autoBotEnabled ? "AutoBot" : "Signal Evaluator",
        message: `Hurdle breached! Net spread +${netSpreadBps.toFixed(1)} BPS exceeded target +${(config?.targetProfitBps ?? 1.0).toFixed(1)} BPS (Hurdle: ${hurdleBps.toFixed(1)} BPS). ${
          autoBotEnabled ? "Evaluating order execution." : "Entries disabled — monitoring only."
        }`,
        route: `${bestAskVenue} ➔ ${bestBidVenue}`,
        spreadBps: netSpreadBps,
        status: "INFO",
      });
    } else {
      newEntries.push({
        id: `scan-${epochMs}`,
        timestamp,
        epochMs,
        level: "SCAN",
        source: "MarketDataHub",
        message: `Polled ${orderbooks.length} venues. Best: Buy ${bestAskVenue} @ ${formatCurrencyPrice(bestAsk)} / Sell ${bestBidVenue} @ ${formatCurrencyPrice(bestBid)}. ${spreadNote}.`,
        route: `${bestAskVenue} ➔ ${bestBidVenue}`,
        spreadBps: netSpreadBps,
        status: "INFO",
      });
    }

    if (newEntries.length > 0) {
      pendingRef.current.push(...newEntries);
    }
  }, [orderbooks, symbol, autoBotEnabled, isPaused, config]);

  // Flush buffered heartbeats at most once per second; no pending entries
  // means no state update and no downstream re-sort/reconciliation.
  useEffect(() => {
    const id = setInterval(() => {
      if (typeof document !== "undefined" && document.hidden) return;
      const pending = pendingRef.current;
      if (pending.length === 0) return;
      pendingRef.current = [];
      setRuntimeLogs((prev) => {
        const next = [...prev];
        for (const entry of pending) {
          const last = next[next.length - 1];
          if (last && entry.epochMs < last.epochMs) {
            let lo = 0;
            let hi = next.length;
            while (lo < hi) {
              const mid = (lo + hi) >> 1;
              if (next[mid].epochMs <= entry.epochMs) lo = mid + 1;
              else hi = mid;
            }
            next.splice(lo, 0, entry);
          } else {
            next.push(entry);
          }
        }
        return next.length > 500 ? next.slice(next.length - 500) : next;
      });
    }, 1000);
    return () => clearInterval(id);
  }, []);

  // Both inputs arrive newest-last; merge descending in O(n) instead of a
  // full O(n log n) re-sort on every append.
  const sortedExecutionItems = useMemo<LogItem[]>(() => {
    return [...executionItems].sort((a, b) => b.epochMs - a.epochMs);
  }, [executionItems]);

  // Combine execution history + runtime telemetry, sorted descending (newest first)
  const allLogs = useMemo<LogItem[]>(() => {
    const out: LogItem[] = [];
    let i = 0;
    let j = runtimeLogs.length - 1;
    while (i < sortedExecutionItems.length && j >= 0) {
      if (sortedExecutionItems[i].epochMs >= runtimeLogs[j].epochMs) {
        out.push(sortedExecutionItems[i]);
        i += 1;
      } else {
        out.push(runtimeLogs[j]);
        j -= 1;
      }
    }
    while (i < sortedExecutionItems.length) {
      out.push(sortedExecutionItems[i]);
      i += 1;
    }
    while (j >= 0) {
      out.push(runtimeLogs[j]);
      j -= 1;
    }
    return out;
  }, [sortedExecutionItems, runtimeLogs]);

  // Filter logs by level, search query, and depth/time window
  const filteredLogs = useMemo<LogItem[]>(() => {
    let out = allLogs.filter((item) => {
      if (levelFilter === "TRADES" && item.level !== "EXEC") return false;
      if (levelFilter === "TRIGGER" && item.level !== "TRIGGER") return false;
      if (levelFilter === "EVAL" && item.level !== "EVAL" && item.level !== "TRIGGER") return false;
      if (levelFilter === "SCAN" && item.level !== "SCAN") return false;
      if (levelFilter === "ERRORS" && item.level !== "ERROR" && item.level !== "WARN") return false;

      if (searchQuery.trim()) {
        const q = searchQuery.toLowerCase();
        const text = `${item.source} ${item.message} ${item.route || ""} ${item.level}`.toLowerCase();
        if (!text.includes(q)) return false;
      }

      return true;
    });

    const now = Date.now();
    if (logDepth === "1h") {
      out = out.filter((item) => item.epochMs >= now - 3_600_000);
    } else if (logDepth === "24h") {
      out = out.filter((item) => item.epochMs >= now - 86_400_000);
    } else if (logDepth !== "all") {
      // Logs are sorted newest-first — count presets take the head
      out = out.slice(0, Number(logDepth));
    }
    return out;
  }, [allLogs, levelFilter, searchQuery, logDepth]);

  // Scroll to top/bottom when logs change if auto-scroll is enabled
  useEffect(() => {
    if (autoScroll && scrollBottomRef.current) {
      scrollBottomRef.current.scrollIntoView({ behavior: "smooth" });
    }
  }, [filteredLogs.length, autoScroll]);

  const handleClearLogs = useCallback(() => {
    pendingRef.current = [];
    setRuntimeLogs([]);
    try {
      localStorage.removeItem(storageKey);
    } catch {
      // ignore
    }
  }, [storageKey]);

  return (
    <div className="space-y-3 font-mono text-xs select-none">
      {/* 1. Control Header: Strategy Pill, Filters, Search & Clear */}
      <div className="flex flex-wrap items-center justify-end gap-3 pb-2.5 border-b border-surface-border">
        {/* Right: Controls (Level Filters, Search, Clear) */}
        <div className="flex flex-wrap items-center gap-2">
          {/* Level Filter Chips */}
          <div className="flex items-center space-x-1 bg-surface-raised p-0.5 rounded-lg border border-surface-border text-[11px]">
            {(["ALL", "TRADES", "TRIGGER", "SCAN", "ERRORS"] as const).map((lvl) => (
              <button
                key={lvl}
                type="button"
                onClick={() => setLevelFilter(lvl)}
                className={`px-2 py-2 rounded-lg font-medium transition cursor-pointer ${
                  levelFilter === lvl
                    ? "bg-accent text-white shadow-xs font-semibold"
                    : "text-slate-500 hover:text-slate-900"
                }`}
              >
                {lvl.charAt(0) + lvl.slice(1).toLowerCase()}
              </button>
            ))}
          </div>

          {/* Depth / Time-Window Filter */}
          <div className="flex items-center bg-surface-raised px-2.5 py-1 rounded-lg border border-surface-border text-xs">
            <select
              value={logDepth}
              onChange={(e) => setLogDepth(e.target.value)}
              className="bg-transparent text-slate-700 font-mono text-xs focus:outline-none cursor-pointer"
              title="How many / how far back logs to display"
            >
              <option value="50">Last 50</option>
              <option value="100">Last 100</option>
              <option value="200">Last 200</option>
              <option value="500">Last 500</option>
              <option value="1h">Last 1h</option>
              <option value="24h">Last 24h</option>
              <option value="all">All</option>
            </select>
          </div>

          {/* Search Bar */}
          <div className="relative">
            <Search className="w-3 h-3 text-slate-400 absolute left-2.5 top-1/2 -translate-y-1/2" />
            <input
              type="text"
              placeholder="Search logs..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              className="pl-7 pr-2.5 py-1 w-32 sm:w-44 bg-surface border border-surface-border rounded-lg text-xs text-slate-900 placeholder:text-slate-400 focus:outline-none focus:ring-1 focus:ring-accent transition-colors"
            />
          </div>

          {/* Clear Button */}
          <button
            type="button"
            onClick={handleClearLogs}
            className="p-1.5 rounded-lg text-slate-400 hover:text-slate-700 hover:bg-surface-raised border border-transparent hover:border-surface-border transition cursor-pointer"
            title="Clear runtime telemetry logs"
          >
            <Trash2 className="w-3.5 h-3.5" />
          </button>
        </div>
      </div>

      {/* 2. Log Stream Table */}
      <div className="rounded-xl border border-surface-border bg-surface overflow-hidden shadow-card">
        {filteredLogs.length === 0 ? (
          <EmptyState
            icon={Activity}
            pulse
            title="No Telemetry Logs"
            hint={`No logs recorded for ${namespace ?? symbol ?? "this ticker"} yet.`}
          />
        ) : (
          <div className="overflow-x-auto max-h-[380px] overflow-y-auto divide-y divide-surface-border">
            <table className="w-full text-left font-mono text-[11px] tabular-nums">
              <thead className="sticky top-0 bg-surface-raised border-b border-surface-border text-[11px] text-slate-500 font-semibold tracking-wider">
                <tr>
                  <th className="py-2.5 px-3 w-40">Timestamp</th>
                  <th className="py-2.5 px-2.5 w-20">Level</th>
                  <th className="py-2.5 px-3 w-36">Source / Route</th>
                  <th className="py-2.5 px-3">Telemetry Details</th>
                  <th className="py-2.5 px-3 text-right w-24">Status</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-surface-border text-[11px]">
                {filteredLogs.map((item) => (
                  <LogRow
                    key={item.id}
                    id={item.id}
                    timestamp={item.timestamp}
                    level={item.level}
                    source={item.source}
                    route={item.route}
                    message={item.message}
                    netPnl={item.netPnl}
                    status={item.status}
                  />
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

      {/* 3. Footer Bar: Summary Count & Auto-Scroll Toggle */}
      <div className="flex items-center justify-between text-xs text-slate-500 px-1 pt-0.5">
        <span>
          Showing <strong className="text-slate-800">{filteredLogs.length}</strong> of{" "}
          <strong className="text-slate-800">{allLogs.length}</strong> log events for {namespace ?? symbol} ({strategyName})
        </span>

        <div className="flex items-center space-x-2">
          <label className="flex items-center space-x-1.5 cursor-pointer text-slate-600">
            <input
              type="checkbox"
              checked={autoScroll}
              onChange={(e) => setAutoScroll(e.target.checked)}
              className="rounded-lg border-surface-border text-accent focus:ring-accent"
            />
            <span className="text-xs">Follow Live Stream</span>
          </label>
        </div>
      </div>
    </div>
  );
};
