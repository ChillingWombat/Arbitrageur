import React, { useState, useMemo } from "react";
import {
  ArrowRight,
  Layers,
  Search,
} from "lucide-react";
import type { ExecutionLogEntry, TickerStrategyConfigDto } from "../types";
import { formatCurrencyPnl } from "../types/tickers";
import { EmptyState } from "./EmptyState";

interface TickerTransactionHistoryProps {
  symbol: string;
  logs: ExecutionLogEntry[];
  /** Skip symbol matching (caller pre-filtered, e.g. multi-ticker strategies). */
  matchAllSymbols?: boolean;
  config?: TickerStrategyConfigDto;
  onRefresh?: () => void;
}

export const TickerTransactionHistory: React.FC<TickerTransactionHistoryProps> = ({
  symbol,
  logs,
  config,
  matchAllSymbols = false,
}) => {
  const [filterStatus, setFilterStatus] = useState<"ALL" | "FILLED" | "FAILED">("ALL");
  const [filterDirection, setFilterDirection] = useState<string>("ALL");
  const [searchQuery, setSearchQuery] = useState<string>("");

  // Filter logs specifically relevant to this ticker
  const tickerLogs = useMemo(() => {
    return logs.filter((log) => {
      // Direct match on symbol (e.g. "BTC-USDC") or pair format (e.g. "BTC-USDC/ETH-USDC")
      if (!matchAllSymbols) {
        const matchesSymbol =
          log.symbol === symbol ||
          log.symbol.startsWith(`${symbol}/`) ||
          log.symbol.endsWith(`/${symbol}`);

        if (!matchesSymbol) return false;
      }

      if (filterStatus === "FILLED" && !log.success) return false;
      if (filterStatus === "FAILED" && log.success) return false;

      if (filterDirection !== "ALL") {
        if (filterDirection === "DIVERGENCE" && log.direction && !log.direction.toLowerCase().includes("divergence")) {
          return false;
        }
        if (filterDirection === "CONVERGENCE" && log.direction && !log.direction.toLowerCase().includes("convergence")) {
          return false;
        }
      }

      if (searchQuery.trim()) {
        const q = searchQuery.toLowerCase();
        const text = `${log.venue_a} ${log.venue_b} ${log.direction || ""} ${log.note}`.toLowerCase();
        if (!text.includes(q)) return false;
      }

      return true;
    });
  }, [logs, symbol, matchAllSymbols, filterStatus, filterDirection, searchQuery]);

  return (
    <div className="bg-surface border border-surface-border rounded-xl p-4 shadow-card transition">
      {/* Header Bar */}
      <div className="flex flex-col md:flex-row md:items-center justify-between pb-2.5 border-b border-surface-border gap-3">
        <div className="flex items-center space-x-2">
          <span className="text-xs font-bold text-slate-800 font-mono tracking-wide">
            Transaction Records
          </span>
          {tickerLogs.length > 0 ? (
            tickerLogs[0].is_live ? (
              <span className="px-1.5 py-0.5 rounded-md bg-emerald-100 border border-emerald-200 text-emerald-800 text-[10px] font-mono font-bold uppercase tracking-wider">
                Live Trading
              </span>
            ) : (
              <span className="px-1.5 py-0.5 rounded-md bg-slate-100 border border-slate-200 text-slate-600 text-[10px] font-mono font-bold uppercase tracking-wider">
                Paper Trading
              </span>
            )
          ) : (
            logs.length > 0 && (
              logs[0].is_live ? (
                <span className="px-1.5 py-0.5 rounded-md bg-emerald-100 border border-emerald-200 text-emerald-800 text-[10px] font-mono font-bold uppercase tracking-wider">
                  Live Trading
                </span>
              ) : (
                <span className="px-1.5 py-0.5 rounded-md bg-slate-100 border border-slate-200 text-slate-600 text-[10px] font-mono font-bold uppercase tracking-wider">
                  Paper Trading
                </span>
              )
            )
          )}
        </div>

        {/* Filter & Search Bar */}
        <div className="flex items-center space-x-2">
          <div className="relative">
            <Search className="w-3.5 h-3.5 text-slate-400 absolute left-2.5 top-1/2 -translate-y-1/2" />
            <input
              type="text"
              placeholder="Filter venue/note..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              className="pl-8 pr-2.5 py-1 text-xs font-mono bg-surface-raised border border-surface-border rounded-lg text-slate-800 placeholder:text-slate-400 focus:outline-none focus:ring-1 focus:ring-accent w-40 transition-colors"
            />
          </div>

          <div className="flex items-center bg-surface-raised p-0.5 rounded-lg border border-surface-border text-[11px] font-mono">
            <button
              onClick={() => setFilterStatus("ALL")}
              className={`px-2 py-2 rounded-lg transition cursor-pointer ${
                filterStatus === "ALL" ? "bg-accent text-white shadow-xs font-medium" : "text-slate-500 hover:text-slate-900"
              }`}
            >
              All ({tickerLogs.length})
            </button>
            <button
              onClick={() => setFilterStatus("FILLED")}
              className={`px-2 py-2 rounded-lg transition cursor-pointer ${
                filterStatus === "FILLED" ? "bg-profit text-white shadow-xs font-medium" : "text-slate-500 hover:text-slate-900"
              }`}
            >
              Filled
            </button>
            <button
              onClick={() => setFilterStatus("FAILED")}
              className={`px-2 py-2 rounded-lg transition cursor-pointer ${
                filterStatus === "FAILED" ? "bg-loss text-white shadow-xs font-medium" : "text-slate-500 hover:text-slate-900"
              }`}
            >
              Failed
            </button>
          </div>

          <div className="flex items-center bg-surface-raised px-2.5 py-1 rounded-lg border border-surface-border text-xs font-mono">
            <select
              value={filterDirection}
              onChange={(e) => setFilterDirection(e.target.value)}
              className="bg-transparent text-slate-700 font-mono text-xs focus:outline-none cursor-pointer"
            >
              <option value="ALL">All Directions</option>
              <option value="DIVERGENCE">Divergence Entry</option>
              <option value="CONVERGENCE">Convergence Unwind</option>
            </select>
          </div>
        </div>
      </div>

      {/* History Table */}
      <div className="mt-4 overflow-x-auto">
        <table className="w-full text-left text-[11px] font-mono tabular-nums border-collapse">
          <thead>
            <tr className="border-b border-surface-border text-slate-500 text-[11px] font-semibold tracking-wider bg-surface-raised/60">
              <th className="py-2.5 px-3">DateTime</th>
              <th className="py-2.5 px-3">Direction & Action</th>
              <th className="py-2.5 px-3">Venues (Buy ➔ Sell)</th>
              <th className="py-2.5 px-3 text-right">Trigger Spread</th>
              <th className="py-2.5 px-3 text-right">Signal Baseline (Mean & Δ)</th>
              <th className="py-2.5 px-3 text-center">Position Step</th>
              <th className="py-2.5 px-3 text-right">Nominal Hurdle</th>
              <th className="py-2.5 px-3 text-right">Net PnL</th>
              <th className="py-2.5 px-3">Status / Diagnostic Note</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-surface-border">
            {tickerLogs.length === 0 ? (
              <tr>
                <td colSpan={9}>
                  <EmptyState
                    title="No Transactions Found"
                    hint={<>No execution records for <span className="font-medium text-slate-700">{symbol}</span> matching the active filters.</>}
                  />
                </td>
              </tr>
            ) : (
              tickerLogs.map((log) => {
                const date = new Date(log.timestamp);
                const dateFormatted = date.toLocaleDateString("en-US", {
                  month: "short",
                  day: "numeric",
                });
                const timeFormatted = date.toLocaleTimeString("en-US", {
                  hour: "2-digit",
                  minute: "2-digit",
                  second: "2-digit",
                  hour12: false,
                });

                const triggerSpread = log.trigger_spread_bps;
                const baselineMean = log.baseline_mean_spread_bps;
                const dislocation =
                  typeof triggerSpread === "number" && typeof baselineMean === "number"
                    ? triggerSpread - baselineMean
                    : null;

                const hurdle = log.nominal_hurdle_bps;
                const hurdleBuffer =
                  typeof triggerSpread === "number" && typeof hurdle === "number"
                    ? triggerSpread - hurdle
                    : null;

                const step = log.position_step ?? 1;
                const maxUnits = log.max_units ?? config?.maxAccumulatedUnits ?? 3;

                const isConvergence = log.direction?.toLowerCase().includes("convergence");
                const isCarry = log.trade_type === "Funding" || log.direction?.toLowerCase().includes("carry");

                return (
                  <tr key={log.id} className="hover:bg-surface-raised/40 transition">
                    {/* DateTime */}
                    <td className="py-2.5 px-3 text-slate-600 whitespace-nowrap">
                      <div className="font-semibold text-slate-900">{timeFormatted}</div>
                      <div className="text-[11px] text-slate-400">{dateFormatted}</div>
                    </td>

                    {/* Direction & Action */}
                    <td className="py-2.5 px-3 whitespace-nowrap">
                      <div className="flex items-center space-x-1.5">
                        <span
                          className={`px-2 py-0.5 rounded-md text-[11px] font-mono font-semibold border ${
                            isConvergence
                              ? "bg-profit-dim text-profit border-profit-border"
                              : isCarry
                              ? "bg-purple-50 text-purple-700 border-purple-200"
                              : "bg-blue-50 text-blue-700 border-blue-200"
                          }`}
                        >
                          {log.direction || "Divergence Entry"}
                        </span>
                      </div>
                    </td>

                    {/* Venues */}
                    <td className="py-2.5 px-3 whitespace-nowrap text-slate-800">
                      <div className="flex items-center space-x-1.5">
                        <span className="font-semibold text-slate-900">{log.venue_a}</span>
                        <ArrowRight className="w-3 h-3 text-slate-400" />
                        <span className="font-semibold text-slate-900">{log.venue_b}</span>
                      </div>
                    </td>

                    {/* Trigger Spread */}
                    <td className="py-2.5 px-3 whitespace-nowrap text-right">
                      {typeof triggerSpread === "number" ? (
                        <div>
                          <div className="font-bold text-slate-900">
                            {triggerSpread > 0 ? `+${triggerSpread.toFixed(1)}` : triggerSpread.toFixed(1)} BPS
                          </div>
                          <div className="text-[11px] text-slate-400">instantaneous</div>
                        </div>
                      ) : (
                        <span className="text-slate-400">—</span>
                      )}
                    </td>

                    {/* Average Spread Baseline Signal */}
                    <td className="py-2.5 px-3 whitespace-nowrap text-right">
                      {typeof baselineMean === "number" ? (
                        <div>
                          <div className="text-slate-700 font-medium">{baselineMean.toFixed(1)} BPS</div>
                          {dislocation !== null && (
                            <div
                              className={`text-[11px] font-semibold ${
                                dislocation >= 0 ? "text-profit" : "text-loss"
                              }`}
                            >
                              Δ {dislocation >= 0 ? `+${dislocation.toFixed(1)}` : dislocation.toFixed(1)} BPS
                            </div>
                          )}
                        </div>
                      ) : (
                        <span className="text-slate-400">—</span>
                      )}
                    </td>

                    {/* Position Step Number */}
                    <td className="py-2.5 px-3 whitespace-nowrap text-center">
                      <div className="inline-flex items-center space-x-1 px-2 py-0.5 rounded-md bg-surface-raised border border-surface-border text-[11px] font-mono font-medium text-slate-700">
                        <Layers className="w-3 h-3 text-slate-400" />
                        <span>
                          Step {step} / {maxUnits}
                        </span>
                      </div>
                    </td>

                    {/* Nominal Hurdle & Margin */}
                    <td className="py-2.5 px-3 whitespace-nowrap text-right">
                      {typeof hurdle === "number" ? (
                        <div>
                          <div className="text-slate-700">{hurdle.toFixed(1)} BPS</div>
                          {hurdleBuffer !== null && (
                            <div
                              className={`text-[11px] font-semibold ${
                                hurdleBuffer >= 0 ? "text-profit" : "text-loss"
                              }`}
                            >
                              Margin {hurdleBuffer >= 0 ? `+${hurdleBuffer.toFixed(1)}` : hurdleBuffer.toFixed(1)}
                            </div>
                          )}
                        </div>
                      ) : (
                        <span className="text-slate-400">—</span>
                      )}
                    </td>

                    {/* Net PnL */}
                    <td className="py-2.5 px-3 whitespace-nowrap text-right">
                      <div className={`font-bold tabular-nums ${log.net_profit >= 0 ? "text-profit" : "text-loss"}`}>
                        {formatCurrencyPnl(log.net_profit)}
                      </div>
                      <div className="text-[11px] text-slate-400">{log.latency_us} µs</div>
                    </td>

                    {/* Diagnostic Note / Status */}
                    <td className="py-2.5 px-3 text-slate-600 max-w-xs truncate">
                      <div className="flex items-center space-x-1.5">
                        {log.success ? (
                          <span className="w-1.5 h-1.5 rounded-full bg-profit shrink-0" />
                        ) : (
                          <span className="w-1.5 h-1.5 rounded-full bg-loss shrink-0" />
                        )}
                        <span className="truncate text-[11px] text-slate-600" title={log.note}>
                          {log.note}
                        </span>
                      </div>
                    </td>
                  </tr>
                );
              })
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
};
