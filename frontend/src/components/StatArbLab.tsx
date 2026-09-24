import React, { useState } from "react";
import { AlertCircle, RefreshCw } from "lucide-react";
import type { StatArbStepResultDto, BertramThresholdDto, OuFitDto, TickerMarketQuoteDto } from "../types";
import { EmptyState } from "./EmptyState";

interface StatArbLabProps {
  onRunStep: (step: number, py: number, px: number) => Promise<StatArbStepResultDto>;
  onSolveBertram: (theta: number, mu: number, sigma: number, cost: number) => Promise<BertramThresholdDto>;
  onFitOu: (prices: number[], dt: number) => Promise<OuFitDto>;
  onFetchQuotes: (symbols: string[]) => Promise<TickerMarketQuoteDto[]>;
}

export const StatArbLab: React.FC<StatArbLabProps> = ({
  onRunStep,
  onSolveBertram,
  onFitOu,
  onFetchQuotes,
}) => {
  // Kalman Tracker State
  const [step, setStep] = useState(1);
  const [priceY, setPriceY] = useState(0);
  const [priceX, setPriceX] = useState(0);
  const [isManual, setIsManual] = useState(false);
  const [history, setHistory] = useState<StatArbStepResultDto[]>([]);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Bertram Solver State
  const [theta, setTheta] = useState(0.15);
  const [mu, setMu] = useState(0.0);
  const [sigma, setSigma] = useState(0.05);
  const [cost, setCost] = useState(0.002);
  const [bertramResult, setBertramResult] = useState<BertramThresholdDto | null>(null);

  // OU Fit State
  const [ouFitResult, setOuFitResult] = useState<OuFitDto | null>(null);

  const handleStep = async () => {
    setErrorMsg(null);
    try {
      const nextStep = step + 1;
      let px = priceX;
      let py = priceY;

      if (!isManual) {
        const quotes = await onFetchQuotes(["BTC-USDC", "ETH-USDC"]);
        const btc = quotes.find((q) => q.symbol === "BTC-USDC")?.mid_price;
        const eth = quotes.find((q) => q.symbol === "ETH-USDC")?.mid_price;
        if (
          btc === undefined || eth === undefined ||
          !Number.isFinite(btc) || !Number.isFinite(eth) ||
          btc <= 0 || eth <= 0
        ) {
          setErrorMsg("Live quotes unavailable for BTC-USDC / ETH-USDC; step not run.");
          return;
        }
        py = btc;
        px = eth;
        setPriceY(py);
        setPriceX(px);
      } else if (
        !Number.isFinite(py) || !Number.isFinite(px) || py <= 0 || px <= 0
      ) {
        setErrorMsg("Manual prices must be finite positive numbers; step not run.");
        return;
      }

      setStep(nextStep);
      const res = await onRunStep(nextStep, py, px);
      setHistory((prev) => [...prev.slice(-24), res]);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      setErrorMsg(`Kalman Step Failed: ${msg}`);
    }
  };

  const handleResetFilter = () => {
    setStep(1);
    setHistory([]);
    setErrorMsg(null);
  };

  const handleSolveBertram = async () => {
    setErrorMsg(null);
    try {
      if (theta <= 0) {
        setErrorMsg("Mean reversion speed (theta) must be strictly greater than 0");
        return;
      }
      if (sigma <= 0) {
        setErrorMsg("Volatility (sigma) must be strictly greater than 0");
        return;
      }
      if (cost <= 0) {
        setErrorMsg("Transaction cost must be strictly greater than 0");
        return;
      }
      const res = await onSolveBertram(theta, mu, sigma, cost);
      setBertramResult(res);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      setErrorMsg(`Bertram Solver Error: ${msg}`);
    }
  };

  const handleFitOu = async () => {
    setErrorMsg(null);
    try {
      const series = history
        .map((h) => h.residual_spread)
        .filter((v) => Number.isFinite(v));
      if (series.length < 10) {
        setErrorMsg("Insufficient Kalman history for OU calibration: run at least 10 steps first.");
        return;
      }

      const res = await onFitOu(series, 1.0);
      setOuFitResult(res);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      setErrorMsg(`OU Fit Error: ${msg}`);
    }
  };

  const latest = history[history.length - 1];

  return (
    <div className="p-4 pr-5 space-y-4 overflow-y-auto h-full">
      {errorMsg && (
        <div className="bg-loss-dim border border-loss-border text-loss px-4 py-3 rounded-lg text-xs font-mono flex items-center justify-between">
          <div className="flex items-center space-x-2">
            <AlertCircle className="w-4 h-4 flex-shrink-0" />
            <span>{errorMsg}</span>
          </div>
          <button
            onClick={() => setErrorMsg(null)}
            className="text-slate-500 hover:text-slate-800 text-xs font-mono underline ml-4"
          >
            Dismiss
          </button>
        </div>
      )}

      {/* Grid: 2D Kalman Filter & Bertram Solver */}
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
        {/* Module 1: 2D Online Kalman Filter */}
        <div className="bg-surface border border-surface-border rounded-xl p-4 space-y-3 shadow-card">
          <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
            <div>
              <h3 className="text-[13px] font-mono font-bold text-slate-900">
                2D Online Kalman Filter (BTC vs ETH)
              </h3>
            </div>

            <div className="flex items-center space-x-2">
              <button
                onClick={handleResetFilter}
                title="Reset Filter History"
                className="btn-secondary !text-xs !py-2 !px-2 font-mono cursor-pointer"
              >
                Reset
              </button>
              <button
                onClick={handleStep}
                className="btn-primary !text-xs !py-2 !px-2 font-mono flex items-center"
              >
                <span>Step Iteration</span>
              </button>
            </div>
          </div>

          {/* Mode Switch & Manual Inputs */}
          <div className="bg-surface-raised border border-surface-border rounded-lg p-2.5 font-mono text-xs flex items-center justify-between gap-3 shadow-xs">
            <div className="flex items-center space-x-2">
              <span className="text-xs text-slate-500">Input Mode:</span>
              <button
                onClick={() => setIsManual(false)}
                className={`px-2 py-2 rounded-lg text-xs font-semibold transition cursor-pointer border ${
                  !isManual ? "bg-accent text-white border-accent shadow-xs" : "text-slate-500 hover:text-slate-900 border-surface-border bg-surface hover:border-slate-300"
                }`}
              >
                Live Quotes
              </button>
              <button
                onClick={() => setIsManual(true)}
                className={`px-2 py-2 rounded-lg text-xs font-semibold transition cursor-pointer border ${
                  isManual ? "bg-accent text-white border-accent shadow-xs" : "text-slate-500 hover:text-slate-900 border-surface-border bg-surface hover:border-slate-300"
                }`}
              >
                Manual Prices
              </button>
            </div>

            {isManual && (
              <div className="flex items-center space-x-2">
                <div className="flex items-center space-x-1">
                  <span className="text-xs text-slate-500">Y:</span>
                  <input
                    type="number"
                    value={priceY}
                    onChange={(e) => setPriceY(parseFloat(e.target.value) || 0)}
                    className="input-field !w-20 !px-1.5 !py-0.5 !text-xs font-mono"
                  />
                </div>
                <div className="flex items-center space-x-1">
                  <span className="text-xs text-slate-500">X:</span>
                  <input
                    type="number"
                    value={priceX}
                    onChange={(e) => setPriceX(parseFloat(e.target.value) || 0)}
                    className="input-field !w-16 !px-1.5 !py-0.5 !text-xs font-mono"
                  />
                </div>
              </div>
            )}
          </div>

          {/* Metrics Row */}
          <div className="grid grid-cols-3 gap-3 font-mono text-xs">
            <div className="p-3 bg-surface-raised border border-surface-border rounded-lg shadow-xs">
              <span className="text-xs text-slate-500">Hedge Ratio (Beta)</span>
              <div className="text-base font-bold text-accent-strong tabular-nums mt-0.5">
                {latest ? latest.dynamic_hedge_ratio.toFixed(4) : "—"}
              </div>
            </div>

            <div className="p-3 bg-surface-raised border border-surface-border rounded-lg shadow-xs">
              <span className="text-xs text-slate-500">Residual Spread</span>
              <div className="text-base font-bold text-slate-900 tabular-nums mt-0.5">
                {latest ? `$${latest.residual_spread.toFixed(2)}` : "—"}
              </div>
            </div>

            <div className="p-3 bg-surface-raised border border-surface-border rounded-lg shadow-xs">
              <span className="text-xs text-slate-500">Spread Z-Score</span>
              <div
                className={`text-base font-bold tabular-nums mt-0.5 ${
                  latest && Math.abs(latest.z_score) > 2
                    ? "text-loss"
                    : latest && Math.abs(latest.z_score) < 0.5
                    ? "text-profit"
                    : "text-slate-500"
                }`}
              >
                {latest ? `${latest.z_score.toFixed(2)} Z` : "—"}
              </div>
            </div>
          </div>

          {/* Interactive SVG Sparkline Chart */}
          <div className="bg-surface-raised border border-surface-border rounded-lg p-4 font-mono text-xs shadow-2xs">
            <div className="flex justify-between text-xs text-slate-500 mb-2">
              <span>Residual Z-Score Spread Walk (Last 25 Cycles)</span>
              <span className="text-accent-strong font-medium">Entry Threshold: +/- 2.0 Z</span>
            </div>

            <div className="h-32 w-full flex items-center justify-center">
              {history.length < 2 ? (
                <div className="text-muted text-xs">Click "Step Iteration" to stream Kalman filter updates...</div>
              ) : (
                <svg className="w-full h-full overflow-visible" viewBox="0 0 400 100">
                  {/* +2.0 threshold line */}
                  <line x1="0" y1="20" x2="400" y2="20" stroke="#dc2626" strokeDasharray="3,3" strokeWidth="1" opacity="0.6" />
                  {/* 0 neutral line */}
                  <line x1="0" y1="50" x2="400" y2="50" stroke="#cbd5e1" strokeWidth="1" />
                  {/* -2.0 threshold line */}
                  <line x1="0" y1="80" x2="400" y2="80" stroke="#059669" strokeDasharray="3,3" strokeWidth="1" opacity="0.6" />

                  {/* Polyline */}
                  <polyline
                    fill="none"
                    stroke="#2563eb"
                    strokeWidth="2"
                    points={history
                      .map((h, i) => {
                        const x = (i / (history.length - 1)) * 400;
                        const y = Math.max(5, Math.min(95, 50 - h.z_score * 15));
                        return `${x},${y}`;
                      })
                      .join(" ")}
                  />
                </svg>
              )}
            </div>

            <div className="flex justify-between text-xs text-muted mt-2">
              <span className="text-profit">-2.0 Z Buy</span>
              <span className="text-slate-500">0.0 Z Mean</span>
              <span className="text-loss">+2.0 Z Short</span>
            </div>
          </div>

          {/* Signal Output */}
          <div className="p-3 rounded-lg bg-surface-raised border border-surface-border flex items-center justify-between text-xs font-mono shadow-2xs">
            <span className="text-slate-500">Current Arbitrage Signal:</span>
            <span
              className={`font-semibold px-2 py-0.5 rounded-md text-xs ${
                latest?.signal === "ENTRY_LONG_SPREAD"
                  ? "bg-profit-dim text-profit border border-profit-border"
                  : latest?.signal === "ENTRY_SHORT_SPREAD"
                  ? "bg-loss-dim text-loss border border-loss-border"
                  : latest?.signal === "EXIT"
                  ? "bg-accent-dim text-accent-strong border border-accent-line"
                  : "bg-surface text-slate-600 border border-surface-border"
              }`}
            >
              {latest?.signal || "NEUTRAL"}
            </span>
          </div>
        </div>

        {/* Module 2: Bertram Optimal Analytic Thresholds */}
        <div className="bg-surface border border-surface-border rounded-xl p-4 space-y-3 shadow-card">
          <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
            <div>
              <h3 className="text-[13px] font-mono font-bold text-slate-900">
                Bertram (2010) Optimal Thresholds
              </h3>
              <span className="text-xs text-slate-500 font-mono">Analytic solver maximizing return per unit time</span>
            </div>

            <button
              onClick={handleSolveBertram}
              className="btn-primary !text-xs !py-2 !px-2 font-mono"
            >
              Optimize
            </button>
          </div>

          {/* Input Controls */}
          <div className="grid grid-cols-2 gap-3 text-xs font-mono">
            <div>
              <span className="text-slate-500 text-xs">Speed (Theta):</span>
              <input
                type="number"
                step="0.01"
                value={theta}
                onChange={(e) => setTheta(parseFloat(e.target.value) || 0.1)}
                className="input-field !w-full !mt-1 !px-2.5 !py-1 !text-xs font-mono"
              />
            </div>

            <div>
              <span className="text-slate-500 text-xs">Volatility (Sigma):</span>
              <input
                type="number"
                step="0.01"
                value={sigma}
                onChange={(e) => setSigma(parseFloat(e.target.value) || 0.05)}
                className="input-field !w-full !mt-1 !px-2.5 !py-1 !text-xs font-mono"
              />
            </div>

            <div>
              <span className="text-slate-500 text-xs">Mean (Mu):</span>
              <input
                type="number"
                step="0.01"
                value={mu}
                onChange={(e) => setMu(parseFloat(e.target.value) || 0.0)}
                className="input-field !w-full !mt-1 !px-2.5 !py-1 !text-xs font-mono"
              />
            </div>

            <div>
              <span className="text-slate-500 text-xs">Cost (c):</span>
              <input
                type="number"
                step="0.001"
                value={cost}
                onChange={(e) => setCost(parseFloat(e.target.value) || 0.002)}
                className="input-field !w-full !mt-1 !px-2.5 !py-1 !text-xs font-mono"
              />
            </div>
          </div>

          {/* Bertram Output Results */}
          {bertramResult ? (
            <div className="bg-surface-raised border border-surface-border rounded-lg p-4 space-y-2.5 font-mono text-xs shadow-xs">
              <div className="flex justify-between">
                <span className="text-slate-500">Optimal Entry (a):</span>
                <span className="text-profit font-semibold tabular-nums">{bertramResult.entry_long.toFixed(4)}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-slate-500">Optimal Exit (m):</span>
                <span className="text-slate-900 font-semibold tabular-nums">{bertramResult.exit_long.toFixed(4)}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-slate-500">Optimal Width (d = |a - m|):</span>
                <span className="text-accent-strong font-semibold tabular-nums">{bertramResult.optimal_distance.toFixed(4)}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-slate-500">Half-Life of Mean Reversion:</span>
                <span className="text-slate-900 font-semibold tabular-nums">{bertramResult.half_life.toFixed(2)} cycles</span>
              </div>
              <div className="flex justify-between">
                <span className="text-slate-500">Est. Trade Frequency:</span>
                <span className="text-slate-900 font-semibold tabular-nums">{bertramResult.estimated_trades_per_year.toFixed(1)} / year</span>
              </div>
              <div className="flex justify-between pt-2 border-t border-surface-border">
                <span className="text-slate-500">Annualized Sharpe Ratio:</span>
                <span className="text-profit font-bold text-sm tabular-nums">{bertramResult.estimated_sharpe.toFixed(2)}</span>
              </div>
            </div>
          ) : (
            <div className="border border-dashed border-surface-border rounded-lg">
              <EmptyState
                plain
                title='Click "Optimize" to calculate Bertram-style threshold levels.'
              />
            </div>
          )}

          {/* OU Process MLE Quick Fit Button */}
          <div className="pt-2">
            <button
              onClick={handleFitOu}
              className="btn-secondary w-full !py-2 font-mono !text-xs font-medium flex items-center justify-center space-x-2 cursor-pointer"
            >
              <RefreshCw className="w-3.5 h-3.5 text-accent-strong" />
              <span>Fit Ornstein-Uhlenbeck Parameters (AR(1) MLE)</span>
            </button>

            {ouFitResult && (
              <div className="mt-2 p-2.5 rounded-lg bg-surface-raised border border-surface-border text-xs font-mono text-slate-700 flex justify-between shadow-xs">
                <span>Theta: <strong className="tabular-nums">{ouFitResult.theta.toFixed(3)}</strong></span>
                <span>Mu: <strong className="tabular-nums">{ouFitResult.mu.toFixed(3)}</strong></span>
                <span>Sigma: <strong className="tabular-nums">{ouFitResult.sigma.toFixed(3)}</strong></span>
                <span>R2: <strong className="tabular-nums">{ouFitResult.r_squared != null ? `${(ouFitResult.r_squared * 100).toFixed(1)}%` : "n/a"}</strong></span>
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
};
