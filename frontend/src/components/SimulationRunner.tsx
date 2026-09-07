import React, { useState, useEffect, useRef, useMemo } from "react";
import type { SimStepResultDto, ExecutionLogEntry } from "../types";
import { PERP_TICKERS, formatCurrencyPnl, getDefaultOrderQty } from "../types/tickers";
import { Cpu, Play, Pause } from "lucide-react";

interface SimulationRunnerProps {
  logs: ExecutionLogEntry[];
  totalPnl: number;
  onRunStep: (symbol: string, targetBps: number, qty: number) => Promise<SimStepResultDto>;
  symbol?: string;
  onSymbolChange?: (s: string) => void;
}

export const SimulationRunner: React.FC<SimulationRunnerProps> = ({
  logs,
  totalPnl,
  onRunStep,
  symbol: symbolProp,
  onSymbolChange,
}) => {
  const [isRunning, setIsRunning] = useState(false);
  const [speedMs, setSpeedMs] = useState(500);
  const [targetBps, setTargetBps] = useState(10.0);
  const [localSymbol, setLocalSymbol] = useState("BTC-USDC");
  const currentSymbol = symbolProp || localSymbol;
  const handleSymbolChange = onSymbolChange || setLocalSymbol;

  const activeTicker = PERP_TICKERS.find((t) => t.symbol === currentSymbol) || {
    symbol: currentSymbol,
    base: currentSymbol.split("-")[0],
    quote: currentSymbol.split("-")[1] || "USDC",
    name: currentSymbol,
    venues: ["HL", "LGT", "VNT"],
    basePrice: 100,
  };

  const [orderQty, setOrderQty] = useState(() => getDefaultOrderQty(activeTicker.basePrice));
  const [latestResult, setLatestResult] = useState<SimStepResultDto | null>(null);

  // Sync orderQty when current symbol changes
  useEffect(() => {
    setOrderQty(getDefaultOrderQty(activeTicker.basePrice));
  }, [currentSymbol, activeTicker.basePrice]);

  // Derive execution stats for the currently selected ticker
  const tickerStats = useMemo(() => {
    let count = 0;
    let pnl = 0;
    const symKey = currentSymbol.toUpperCase();
    const baseKey = symKey.split(/[-_/]/)[0];

    for (const log of logs) {
      if (!log.symbol) continue;
      const logKey = log.symbol.toUpperCase();
      const logBase = logKey.split(/[-_/]/)[0];
      if (logKey === symKey || logBase === baseKey) {
        count += 1;
        pnl += log.net_profit || 0;
      }
    }
    return { count, pnl: Math.round(pnl * 100) / 100 };
  }, [logs, currentSymbol]);

  const timerRef = useRef<NodeJS.Timeout | null>(null);

  const handleStep = async () => {
    const res = await onRunStep(currentSymbol, targetBps, orderQty);
    setLatestResult(res);
  };

  useEffect(() => {
    if (isRunning) {
      timerRef.current = setInterval(() => {
        handleStep();
      }, speedMs);
    } else {
      if (timerRef.current) clearInterval(timerRef.current);
    }
    return () => {
      if (timerRef.current) clearInterval(timerRef.current);
    };
  }, [isRunning, speedMs, currentSymbol, targetBps, orderQty]);

  const winCount = logs.filter((l) => l.success).length;
  const winRate = logs.length > 0 ? (winCount / logs.length) * 100 : 100;

  return (
    <div className="p-6 space-y-6 overflow-y-auto h-full max-w-6xl select-none">
      {/* Simulation Controls Top Bar */}
      <div className="glass-panel p-5 rounded-2xl border border-white/[0.08] flex flex-wrap items-center justify-between gap-4 shadow-xs">
        <div>
          <div className="flex items-center space-x-3 mb-1">
            <div className="w-8 h-8 rounded-lg bg-cyan-500/15 border border-cyan-500/30 flex items-center justify-center text-cyan-300 shadow-[0_0_12px_rgba(34,211,238,0.2)]">
              <Cpu className="w-4 h-4" />
            </div>
            <h2 className="text-base font-bold font-mono text-white tracking-tight">
              High-Frequency Paper Trading Simulation Studio
            </h2>
            {/* Active Ticker Tag with venues, trade count, and PnL */}
            <div className="text-xs font-mono font-bold text-white bg-white/[0.04] px-3 py-1.5 rounded-xl border border-white/[0.08] flex items-center space-x-2">
              <span>{currentSymbol}</span>
              <span className="flex space-x-1">
                {activeTicker.venues.map((v) => (
                  <span key={v} className="chip chip-info text-[9px] px-1 py-0.2">
                    {v}
                  </span>
                ))}
              </span>
              <span className="text-[10px] px-1.5 py-0.2 rounded bg-white/[0.04] border border-white/[0.06] font-medium text-white/60 tabular-nums">
                {tickerStats.count} tx{tickerStats.count === 1 ? "" : "s"}
              </span>
              <span
                className={`text-[10px] px-1.5 py-0.2 rounded font-semibold tabular-nums ${
                  tickerStats.pnl > 0
                    ? "chip chip-pos"
                    : tickerStats.pnl < 0
                    ? "chip chip-neg"
                    : "text-white/40 bg-white/[0.04] border border-white/[0.06]"
                }`}
              >
                {formatCurrencyPnl(tickerStats.pnl)}
              </span>
            </div>
          </div>
          <p className="text-xs text-white/50 font-mono mt-1">
            Simulates live cross-venue dislocations, atomic multi-leg routing, and automated emergency unwinding.
          </p>
        </div>

        {/* Action Buttons */}
        <div className="flex items-center space-x-3">
          <button
            onClick={handleStep}
            disabled={isRunning}
            className="px-3.5 py-2 bg-white/[0.04] hover:bg-white/[0.08] border border-white/[0.08] text-white/80 hover:text-white text-xs font-mono font-medium rounded-xl transition disabled:opacity-50 shadow-xs"
          >
            <span>Single Step</span>
          </button>

          <button
            onClick={() => setIsRunning(!isRunning)}
            className={`px-4 py-2 rounded-xl text-xs font-mono font-medium transition shadow-xs flex items-center space-x-1.5 ${
              isRunning
                ? "bg-rose-500 hover:bg-rose-600 text-white shadow-[0_0_15px_rgba(251,113,133,0.3)]"
                : "bg-gradient-to-r from-cyan-500 to-indigo-600 hover:from-cyan-400 hover:to-indigo-500 text-white shadow-[0_0_15px_rgba(34,211,238,0.3)]"
            }`}
          >
            {isRunning ? (
              <>
                <Pause className="w-3.5 h-3.5" />
                <span>Pause Auto-Run</span>
              </>
            ) : (
              <>
                <Play className="w-3.5 h-3.5" />
                <span>Start Continuous</span>
              </>
            )}
          </button>
        </div>
      </div>

      {/* Simulation Parameter Controls */}
      <div className="grid grid-cols-1 md:grid-cols-4 gap-4 glass-panel p-4 rounded-2xl border border-white/[0.08] font-mono text-xs shadow-xs">
        <div>
          <span className="text-white/40 text-[10px] uppercase font-semibold">Asset ({PERP_TICKERS.length} Tickers):</span>
          <select
            value={currentSymbol}
            onChange={(e) => handleSymbolChange(e.target.value)}
            className="w-full mt-1.5 px-3 py-1.5 bg-black/40 border border-white/[0.1] rounded-xl text-white focus:outline-none focus:ring-1 focus:ring-cyan-400"
          >
            {PERP_TICKERS.map((t) => (
              <option key={t.symbol} value={t.symbol} className="bg-slate-900 text-white">
                {t.symbol}
              </option>
            ))}
          </select>
        </div>

        <div>
          <span className="text-white/40 text-[10px] uppercase font-semibold">Target Spread (BPS):</span>
          <input
            type="number"
            step="1"
            value={targetBps}
            onChange={(e) => setTargetBps(parseFloat(e.target.value) || 5)}
            className="w-full mt-1.5 px-3 py-1.5 bg-black/40 border border-white/[0.1] rounded-xl text-white focus:outline-none focus:ring-1 focus:ring-cyan-400 tabular-nums"
          />
        </div>

        <div>
          <span className="text-white/40 text-[10px] uppercase font-semibold">Order Qty:</span>
          <input
            type="number"
            step="0.1"
            value={orderQty}
            onChange={(e) => setOrderQty(parseFloat(e.target.value) || 0.1)}
            className="w-full mt-1.5 px-3 py-1.5 bg-black/40 border border-white/[0.1] rounded-xl text-white focus:outline-none focus:ring-1 focus:ring-cyan-400 tabular-nums"
          />
        </div>

        <div>
          <span className="text-white/40 text-[10px] uppercase font-semibold">Speed Interval:</span>
          <select
            value={speedMs}
            onChange={(e) => setSpeedMs(parseInt(e.target.value))}
            className="w-full mt-1.5 px-3 py-1.5 bg-black/40 border border-white/[0.1] rounded-xl text-white focus:outline-none focus:ring-1 focus:ring-cyan-400"
          >
            <option value={100} className="bg-slate-900 text-white">100ms (Hyper Speed)</option>
            <option value={250} className="bg-slate-900 text-white">250ms (Fast)</option>
            <option value={500} className="bg-slate-900 text-white">500ms (Normal)</option>
            <option value={1000} className="bg-slate-900 text-white">1000ms (Slow)</option>
          </select>
        </div>
      </div>

      {/* Stats Summary Bar */}
      <div className="grid grid-cols-1 md:grid-cols-4 gap-4 font-mono text-xs tabular-nums">
        <div className="p-4 glass-panel rounded-2xl border border-white/[0.08] shadow-xs">
          <div className="text-[10px] uppercase text-white/40 font-semibold">Total Simulation Trades</div>
          <div className="text-xl font-bold text-white mt-1">{logs.length}</div>
        </div>

        <div className="p-4 glass-panel rounded-2xl border border-white/[0.08] shadow-xs">
          <div className="text-[10px] uppercase text-white/40 font-semibold">Atomic Win Rate</div>
          <div className="text-xl font-bold text-emerald-400 mt-1">{winRate.toFixed(1)}%</div>
        </div>

        <div className="p-4 glass-panel rounded-2xl border border-white/[0.08] shadow-xs">
          <div className="text-[10px] uppercase text-white/40 font-semibold">Cumulative Realized PnL</div>
          <div
            className={`text-xl font-bold mt-1 ${
              totalPnl >= 0 ? "text-emerald-400" : "text-rose-400"
            }`}
          >
            {totalPnl >= 0 ? `+$${totalPnl.toFixed(2)}` : `-$${Math.abs(totalPnl).toFixed(2)}`}
          </div>
        </div>

        <div className="p-4 glass-panel rounded-2xl border border-white/[0.08] shadow-xs">
          <div className="text-[10px] uppercase text-white/40 font-semibold">Latest Cycle Status</div>
          <div className="text-sm font-semibold text-cyan-300 mt-1 truncate">
            {latestResult ? latestResult.message : "Ready to simulate"}
          </div>
        </div>
      </div>

      {/* Real-Time Execution Trace Log Table */}
      <div className="glass-panel rounded-2xl border border-white/[0.08] p-5 shadow-xs">
        <h3 className="text-xs font-mono uppercase tracking-wider text-white/40 mb-4 flex items-center justify-between">
          <span className="text-white/90 font-bold">Atomic Order Coordination Stream</span>
          <span className="text-white/40 font-normal lowercase">tokio::join! concurrent leg execution</span>
        </h3>

        {logs.length === 0 ? (
          <div className="text-center py-12 text-white/40 font-mono text-xs border border-dashed border-white/[0.08] rounded-xl">
            No trades executed yet. Click "Single Step" or "Start Continuous".
          </div>
        ) : (
          <div className="overflow-x-auto max-h-96">
            <table className="w-full text-left text-xs font-mono">
              <thead className="sticky top-0 bg-[#080B14] border-b border-white/[0.08] text-white/40 uppercase text-[10px] tracking-wider">
                <tr>
                  <th className="py-2.5 px-3 font-semibold">Trade ID</th>
                  <th className="py-2.5 px-3 font-semibold">Type</th>
                  <th className="py-2.5 px-3 font-semibold">Venues</th>
                  <th className="py-2.5 px-3 font-semibold">Leg Status</th>
                  <th className="py-2.5 px-3 font-semibold">Gross</th>
                  <th className="py-2.5 px-3 font-semibold">Fees</th>
                  <th className="py-2.5 px-3 font-semibold">Net Profit</th>
                  <th className="py-2.5 px-3 font-semibold">Latency</th>
                  <th className="py-2.5 px-3 font-semibold">Coordination Note</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-white/[0.06]">
                {logs.map((log) => (
                  <tr key={log.id} className="hover:bg-white/[0.03] transition tabular-nums">
                    <td className="py-2 px-3 text-white/40 font-mono text-[11px]">{log.id}</td>
                    <td className="py-2 px-3 text-white font-medium">{log.trade_type}</td>
                    <td className="py-2 px-3 text-white/70">{log.venue_a} / {log.venue_b}</td>
                    <td className="py-2 px-3">
                      {log.success ? (
                        <span className="chip chip-pos text-[10px]">
                          FILLED
                        </span>
                      ) : (
                        <span className="chip chip-neg text-[10px]">
                          UNWOUND
                        </span>
                      )}
                    </td>
                    <td className="py-2 px-3 text-white/80">${log.gross_profit.toFixed(2)}</td>
                    <td className="py-2 px-3 text-white/50">${log.fees.toFixed(2)}</td>
                    <td
                      className={`py-2 px-3 font-bold ${
                        log.net_profit >= 0 ? "text-emerald-400" : "text-rose-400"
                      }`}
                    >
                      {log.net_profit >= 0 ? `+$${log.net_profit.toFixed(2)}` : `-$${Math.abs(log.net_profit).toFixed(2)}`}
                    </td>
                    <td className="py-2 px-3 text-white/50">{log.latency_us} us</td>
                    <td className="py-2 px-3 text-white/60 truncate max-w-xs">{log.note}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </div>
  );
};
