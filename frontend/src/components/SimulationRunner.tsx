import React, { useState, useEffect, useRef, useMemo } from "react";
import type { SimStepResultDto, ExecutionLogEntry } from "../types";
import { PERP_TICKERS, formatCurrencyPnl, getDefaultOrderQty } from "../types/tickers";

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
    <div className="p-6 space-y-6 overflow-y-auto h-full">
      {/* Simulation Controls Top Bar */}
      <div className="bg-surface p-5 rounded-xl border border-surface-border flex flex-wrap items-center justify-between gap-4 shadow-xs">
        <div>
          <div className="flex items-center space-x-3 mb-1">
            <h2 className="text-sm font-bold font-mono text-slate-900">
              High-Frequency Paper Trading Simulation Studio
            </h2>
            {/* Active Ticker Tag with venues, trade count, and PnL */}
            <div className="text-xs font-mono font-bold text-slate-900 bg-surface-raised px-2.5 py-1 rounded-md border border-surface-border flex items-center space-x-2">
              <span>{currentSymbol}</span>
              <span className="flex space-x-1">
                {activeTicker.venues.map((v) => (
                  <span key={v} className="text-[9px] px-1 py-0.2 rounded font-semibold bg-accent-dim text-accent border border-accent-line/40">
                    {v}
                  </span>
                ))}
              </span>
              <span className="text-[10px] px-1.5 py-0.2 rounded bg-surface border border-surface-border font-medium text-slate-700">
                {tickerStats.count} tx{tickerStats.count === 1 ? "" : "s"}
              </span>
              <span
                className={`text-[10px] px-1.5 py-0.2 rounded font-semibold ${
                  tickerStats.pnl > 0
                    ? "text-profit bg-profit-dim border border-profit-border"
                    : tickerStats.pnl < 0
                    ? "text-loss bg-loss-dim border border-loss-border"
                    : "text-slate-500 bg-surface border border-surface-border"
                }`}
              >
                {formatCurrencyPnl(tickerStats.pnl)}
              </span>
            </div>
          </div>
          <p className="text-xs text-slate-500">
            Simulates live cross-venue dislocations, atomic multi-leg routing, and automated emergency unwinding.
          </p>
        </div>

        {/* Action Buttons */}
        <div className="flex items-center space-x-3">
          <button
            onClick={handleStep}
            disabled={isRunning}
            className="px-3.5 py-1.5 bg-surface hover:bg-surface-raised border border-surface-border text-slate-700 text-xs font-mono font-medium rounded-lg transition disabled:opacity-50 shadow-xs"
          >
            <span>Single Step</span>
          </button>

          <button
            onClick={() => setIsRunning(!isRunning)}
            className={`px-4 py-1.5 rounded-lg text-xs font-mono font-medium transition shadow-xs ${
              isRunning
                ? "bg-loss hover:bg-red-700 text-white"
                : "bg-accent hover:bg-accent-hover text-white"
            }`}
          >
            {isRunning ? (
              <span>Pause Auto-Run</span>
            ) : (
              <span>Start Continuous</span>
            )}
          </button>
        </div>
      </div>

      {/* Simulation Parameter Controls */}
      <div className="grid grid-cols-1 md:grid-cols-4 gap-4 bg-surface p-4 rounded-xl border border-surface-border font-mono text-xs shadow-xs">
        <div>
          <span className="text-slate-500 text-[10px] uppercase">Asset ({PERP_TICKERS.length} Tickers):</span>
          <select
            value={currentSymbol}
            onChange={(e) => handleSymbolChange(e.target.value)}
            className="w-full mt-1 px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent focus:border-accent"
          >
            {PERP_TICKERS.map((t) => (
              <option key={t.symbol} value={t.symbol}>
                {t.symbol} ({t.name})
              </option>
            ))}
          </select>
        </div>

        <div>
          <span className="text-slate-500 text-[10px] uppercase">Target Spread (BPS):</span>
          <input
            type="number"
            step="1"
            value={targetBps}
            onChange={(e) => setTargetBps(parseFloat(e.target.value) || 5)}
            className="w-full mt-1 px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent focus:border-accent"
          />
        </div>

        <div>
          <span className="text-slate-500 text-[10px] uppercase">Order Qty:</span>
          <input
            type="number"
            step="0.1"
            value={orderQty}
            onChange={(e) => setOrderQty(parseFloat(e.target.value) || 0.1)}
            className="w-full mt-1 px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent focus:border-accent"
          />
        </div>

        <div>
          <span className="text-slate-500 text-[10px] uppercase">Speed Interval:</span>
          <select
            value={speedMs}
            onChange={(e) => setSpeedMs(parseInt(e.target.value))}
            className="w-full mt-1 px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent focus:border-accent"
          >
            <option value={100}>100ms (Hyper Speed)</option>
            <option value={250}>250ms (Fast)</option>
            <option value={500}>500ms (Normal)</option>
            <option value={1000}>1000ms (Slow)</option>
          </select>
        </div>
      </div>

      {/* Stats Summary Bar */}
      <div className="grid grid-cols-1 md:grid-cols-4 gap-4 font-mono text-xs">
        <div className="p-4 bg-surface rounded-xl border border-surface-border shadow-xs">
          <div className="text-[10px] uppercase text-slate-500">Total Simulation Trades</div>
          <div className="text-xl font-bold text-slate-900 mt-1">{logs.length}</div>
        </div>

        <div className="p-4 bg-surface rounded-xl border border-surface-border shadow-xs">
          <div className="text-[10px] uppercase text-slate-500">Atomic Win Rate</div>
          <div className="text-xl font-bold text-profit mt-1">{winRate.toFixed(1)}%</div>
        </div>

        <div className="p-4 bg-surface rounded-xl border border-surface-border shadow-xs">
          <div className="text-[10px] uppercase text-slate-500">Cumulative Realized PnL</div>
          <div
            className={`text-xl font-bold mt-1 ${
              totalPnl >= 0 ? "text-profit" : "text-loss"
            }`}
          >
            {totalPnl >= 0 ? `+$${totalPnl.toFixed(2)}` : `-$${Math.abs(totalPnl).toFixed(2)}`}
          </div>
        </div>

        <div className="p-4 bg-surface rounded-xl border border-surface-border shadow-xs">
          <div className="text-[10px] uppercase text-slate-500">Latest Cycle Status</div>
          <div className="text-sm font-semibold text-accent mt-1 truncate">
            {latestResult ? latestResult.message : "Ready to simulate"}
          </div>
        </div>
      </div>

      {/* Real-Time Execution Trace Log Table */}
      <div className="bg-surface rounded-xl border border-surface-border p-5 shadow-xs">
        <h3 className="text-xs font-mono uppercase tracking-wider text-slate-500 mb-4 flex items-center justify-between">
          <span>Atomic Order Coordination Stream</span>
          <span className="text-slate-400 font-normal lowercase">tokio::join! concurrent leg execution</span>
        </h3>

        {logs.length === 0 ? (
          <div className="text-center py-12 text-slate-400 font-mono text-xs border border-dashed border-surface-border rounded-lg">
            No trades executed yet. Click "Single Step" or "Start Continuous".
          </div>
        ) : (
          <div className="overflow-x-auto max-h-96">
            <table className="w-full text-left text-xs font-mono">
              <thead className="sticky top-0 bg-surface border-b border-surface-border text-slate-500 uppercase text-[10px] tracking-wider">
                <tr>
                  <th className="py-2.5 px-3">Trade ID</th>
                  <th className="py-2.5 px-3">Type</th>
                  <th className="py-2.5 px-3">Venues</th>
                  <th className="py-2.5 px-3">Leg Status</th>
                  <th className="py-2.5 px-3">Gross</th>
                  <th className="py-2.5 px-3">Fees</th>
                  <th className="py-2.5 px-3">Net Profit</th>
                  <th className="py-2.5 px-3">Latency</th>
                  <th className="py-2.5 px-3">Coordination Note</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-surface-border">
                {logs.map((log) => (
                  <tr key={log.id} className="hover:bg-surface-raised transition">
                    <td className="py-2 px-3 text-slate-500 font-mono text-[11px]">{log.id}</td>
                    <td className="py-2 px-3 text-slate-900 font-medium">{log.trade_type}</td>
                    <td className="py-2 px-3 text-slate-700">{log.venue_a} / {log.venue_b}</td>
                    <td className="py-2 px-3">
                      {log.success ? (
                        <span className="inline-flex items-center px-1.5 py-0.5 rounded bg-profit-dim text-profit text-[10px] font-semibold">
                          FILLED
                        </span>
                      ) : (
                        <span className="inline-flex items-center px-1.5 py-0.5 rounded bg-loss-dim text-loss text-[10px] font-semibold">
                          UNWOUND
                        </span>
                      )}
                    </td>
                    <td className="py-2 px-3 text-slate-700">${log.gross_profit.toFixed(2)}</td>
                    <td className="py-2 px-3 text-slate-500">${log.fees.toFixed(2)}</td>
                    <td
                      className={`py-2 px-3 font-bold ${
                        log.net_profit >= 0 ? "text-profit" : "text-loss"
                      }`}
                    >
                      {log.net_profit >= 0 ? `+$${log.net_profit.toFixed(2)}` : `-$${Math.abs(log.net_profit).toFixed(2)}`}
                    </td>
                    <td className="py-2 px-3 text-slate-500">{log.latency_us} us</td>
                    <td className="py-2 px-3 text-slate-600 truncate max-w-xs">{log.note}</td>
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
