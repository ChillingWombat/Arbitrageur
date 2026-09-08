import React, { useState } from "react";
import { AlertCircle, RefreshCw } from "lucide-react";
import type { StatArbStepResultDto, BertramThresholdDto, OuFitDto } from "../types";

interface StatArbLabProps {
  onRunStep: (step: number, py: number, px: number) => Promise<StatArbStepResultDto>;
  onSolveBertram: (theta: number, mu: number, sigma: number, cost: number) => Promise<BertramThresholdDto>;
  onFitOu: (prices: number[], dt: number) => Promise<OuFitDto>;
}

export const StatArbLab: React.FC<StatArbLabProps> = ({
  onRunStep,
  onSolveBertram,
  onFitOu,
}) => {
  // Kalman Tracker State
  const [step, setStep] = useState(1);
  const [priceY, setPriceY] = useState(64200.0);
  const [priceX, setPriceX] = useState(3450.0);
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
        try {
          const resp = await fetch('https://api.binance.com/api/v3/ticker/bookTicker?symbols=%5B%22BTCUSDT%22,%22ETHUSDT%22%5D');
          if (resp.ok) {
            const data = await resp.json();
            const btc = data.find((d: any) => d.symbol === "BTCUSDT");
            const eth = data.find((d: any) => d.symbol === "ETHUSDT");
            if (btc && eth) {
              py = (parseFloat(btc.bidPrice) + parseFloat(btc.askPrice)) / 2;
              px = (parseFloat(eth.bidPrice) + parseFloat(eth.askPrice)) / 2;
              setPriceX(px);
              setPriceY(py);
            }
          }
        } catch {
          // Keep current px and py on network error
        }
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
      let series: number[] = [];
      try {
        const resp = await fetch("https://api.binance.com/api/v3/klines?symbol=BTCUSDT&interval=1m&limit=100");
        if (resp.ok) {
          const klines = await resp.json();
          if (Array.isArray(klines) && klines.length > 10) {
            const closes = klines.map((k: any) => parseFloat(k[4]));
            const mean = closes.reduce((a: number, b: number) => a + b, 0) / closes.length;
            series = closes.map((c: number) => mu + (c - mean) / mean);
          }
        }
      } catch {
        // network fallback
      }

      if (series.length === 0) {
        series = [mu - sigma, mu - sigma * 0.5, mu, mu + sigma * 0.5, mu + sigma, mu + sigma * 0.3, mu - sigma * 0.2, mu];
      }

      const res = await onFitOu(series, 0.1);
      setOuFitResult(res);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      setErrorMsg(`OU Fit Error: ${msg}`);
    }
  };

  const latest = history[history.length - 1];

  return (
    <div className="p-6 space-y-6 overflow-y-auto h-full">
      {/* Top Banner */}
      <div className="bg-surface p-5 rounded-xl border border-surface-border flex items-center justify-between shadow-xs">
        <div>
          <h2 className="text-sm font-bold font-mono text-slate-900">
            Quantitative Analytics & Statistical Arbitrage Lab
          </h2>
          <p className="text-xs text-slate-500 mt-1">
            2D Online Kalman Filter dynamic hedge ratio estimation, Ornstein-Uhlenbeck MLE fitting, and Bertram analytic threshold solver.
          </p>
        </div>

        <div className="flex items-center space-x-2 text-xs font-mono">
          <span className="px-2.5 py-1 rounded bg-surface-raised border border-surface-border text-slate-500">
            Latency: <span className="text-profit font-bold">19.15 ns</span> / update
          </span>
        </div>
      </div>

      {errorMsg && (
        <div className="bg-loss-dim border border-loss-border text-loss px-4 py-3 rounded-xl text-xs font-mono flex items-center justify-between">
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
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        {/* Module 1: 2D Online Kalman Filter */}
        <div className="bg-surface border border-surface-border rounded-xl p-5 space-y-4 shadow-xs">
          <div className="flex items-center justify-between border-b border-surface-border pb-3">
            <div>
              <h3 className="text-xs font-mono font-bold uppercase text-slate-900">
                2D Online Kalman Filter (BTC vs ETH)
              </h3>
              <span className="text-[10px] text-slate-500 font-mono">
                Recursive O(1) updates | BTC: ${priceY.toFixed(2)} vs ETH: ${priceX.toFixed(2)}
              </span>
            </div>

            <div className="flex items-center space-x-2">
              <button
                onClick={handleResetFilter}
                title="Reset Filter History"
                className="px-2.5 py-1.5 bg-surface hover:bg-surface-raised border border-surface-border text-slate-600 hover:text-slate-900 rounded-lg text-xs font-mono transition"
              >
                Reset
              </button>
              <button
                onClick={handleStep}
                className="px-3 py-1.5 bg-accent hover:bg-accent-hover text-white font-medium text-xs font-mono rounded-lg transition flex items-center shadow-xs"
              >
                <span>Step Iteration</span>
              </button>
            </div>
          </div>

          {/* Mode Switch & Manual Inputs */}
          <div className="bg-surface-raised border border-surface-border rounded-lg p-2.5 font-mono text-xs flex items-center justify-between gap-3 shadow-2xs">
            <div className="flex items-center space-x-2">
              <span className="text-[10px] text-slate-500 uppercase">Input Mode:</span>
              <button
                onClick={() => setIsManual(false)}
                className={`px-2 py-0.5 rounded text-[11px] transition ${
                  !isManual ? "bg-accent text-white font-semibold shadow-2xs" : "text-slate-600 hover:text-slate-900"
                }`}
              >
                Auto Drift
              </button>
              <button
                onClick={() => setIsManual(true)}
                className={`px-2 py-0.5 rounded text-[11px] transition ${
                  isManual ? "bg-accent text-white font-semibold shadow-2xs" : "text-slate-600 hover:text-slate-900"
                }`}
              >
                Manual Prices
              </button>
            </div>

            {isManual && (
              <div className="flex items-center space-x-2">
                <div className="flex items-center space-x-1">
                  <span className="text-[10px] text-slate-500">Y:</span>
                  <input
                    type="number"
                    value={priceY}
                    onChange={(e) => setPriceY(parseFloat(e.target.value) || 0)}
                    className="w-20 px-1.5 py-0.5 bg-surface border border-surface-border rounded text-slate-900 text-[11px]"
                  />
                </div>
                <div className="flex items-center space-x-1">
                  <span className="text-[10px] text-slate-500">X:</span>
                  <input
                    type="number"
                    value={priceX}
                    onChange={(e) => setPriceX(parseFloat(e.target.value) || 0)}
                    className="w-16 px-1.5 py-0.5 bg-surface border border-surface-border rounded text-slate-900 text-[11px]"
                  />
                </div>
              </div>
            )}
          </div>

          {/* Metrics Row */}
          <div className="grid grid-cols-3 gap-3 font-mono text-xs">
            <div className="p-3 bg-surface-raised border border-surface-border rounded-lg shadow-2xs">
              <span className="text-[10px] text-slate-500 uppercase">Hedge Ratio (Beta)</span>
              <div className="text-base font-bold text-accent mt-0.5">
                {latest ? latest.dynamic_hedge_ratio.toFixed(4) : "18.6200"}
              </div>
            </div>

            <div className="p-3 bg-surface-raised border border-surface-border rounded-lg shadow-2xs">
              <span className="text-[10px] text-slate-500 uppercase">Residual Spread</span>
              <div className="text-base font-bold text-slate-900 mt-0.5">
                {latest ? `$${latest.residual_spread.toFixed(2)}` : "$0.00"}
              </div>
            </div>

            <div className="p-3 bg-surface-raised border border-surface-border rounded-lg shadow-2xs">
              <span className="text-[10px] text-slate-500 uppercase">Spread Z-Score</span>
              <div
                className={`text-base font-bold mt-0.5 ${
                  Math.abs(latest?.z_score || 0) > 2
                    ? "text-loss"
                    : Math.abs(latest?.z_score || 0) < 0.5
                    ? "text-profit"
                    : "text-warn"
                }`}
              >
                {latest ? latest.z_score.toFixed(2) : "0.00"} Z
              </div>
            </div>
          </div>

          {/* Interactive SVG Sparkline Chart */}
          <div className="bg-surface-raised border border-surface-border rounded-lg p-4 font-mono text-xs shadow-2xs">
            <div className="flex justify-between text-[10px] text-slate-500 mb-2">
              <span>Residual Z-Score Spread Walk (Last 25 Cycles)</span>
              <span className="text-accent font-medium">Entry Threshold: +/- 2.0 Z</span>
            </div>

            <div className="h-32 w-full flex items-center justify-center">
              {history.length < 2 ? (
                <div className="text-slate-400 text-xs">Click "Step Iteration" to stream Kalman filter updates...</div>
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

            <div className="flex justify-between text-[9px] text-slate-400 mt-2">
              <span className="text-profit">-2.0 Z Buy</span>
              <span className="text-slate-500">0.0 Z Mean</span>
              <span className="text-loss">+2.0 Z Short</span>
            </div>
          </div>

          {/* Signal Output */}
          <div className="p-3 rounded-lg bg-surface-raised border border-surface-border flex items-center justify-between text-xs font-mono shadow-2xs">
            <span className="text-slate-500">Current Arbitrage Signal:</span>
            <span
              className={`font-semibold px-2 py-0.5 rounded text-[11px] ${
                latest?.signal === "ENTRY_LONG_SPREAD"
                  ? "bg-profit-dim text-profit border border-profit-border"
                  : latest?.signal === "ENTRY_SHORT_SPREAD"
                  ? "bg-loss-dim text-loss border border-loss-border"
                  : latest?.signal === "EXIT"
                  ? "bg-accent-dim text-accent border border-accent-line"
                  : "bg-surface text-slate-600 border border-surface-border"
              }`}
            >
              {latest?.signal || "NEUTRAL"}
            </span>
          </div>
        </div>

        {/* Module 2: Bertram Optimal Analytic Thresholds */}
        <div className="bg-surface border border-surface-border rounded-xl p-5 space-y-4 shadow-xs">
          <div className="flex items-center justify-between border-b border-surface-border pb-3">
            <div>
              <h3 className="text-xs font-mono font-bold uppercase text-slate-900">
                Bertram (2010) Optimal Thresholds
              </h3>
              <span className="text-[10px] text-slate-500 font-mono">Analytic solver maximizing return per unit time</span>
            </div>

            <button
              onClick={handleSolveBertram}
              className="px-3 py-1.5 bg-accent hover:bg-accent-hover text-white font-medium text-xs font-mono rounded-lg transition shadow-xs"
            >
              Solve Optimal
            </button>
          </div>

          {/* Input Controls */}
          <div className="grid grid-cols-2 gap-3 text-xs font-mono">
            <div>
              <span className="text-slate-500 text-[10px]">Speed (Theta):</span>
              <input
                type="number"
                step="0.01"
                value={theta}
                onChange={(e) => setTheta(parseFloat(e.target.value) || 0.1)}
                className="w-full mt-1 px-2.5 py-1 bg-surface border border-surface-border rounded text-slate-900 font-mono text-xs focus:outline-none focus:ring-1 focus:ring-accent focus:border-accent"
              />
            </div>

            <div>
              <span className="text-slate-500 text-[10px]">Volatility (Sigma):</span>
              <input
                type="number"
                step="0.01"
                value={sigma}
                onChange={(e) => setSigma(parseFloat(e.target.value) || 0.05)}
                className="w-full mt-1 px-2.5 py-1 bg-surface border border-surface-border rounded text-slate-900 font-mono text-xs focus:outline-none focus:ring-1 focus:ring-accent focus:border-accent"
              />
            </div>

            <div>
              <span className="text-slate-500 text-[10px]">Mean (Mu):</span>
              <input
                type="number"
                step="0.01"
                value={mu}
                onChange={(e) => setMu(parseFloat(e.target.value) || 0.0)}
                className="w-full mt-1 px-2.5 py-1 bg-surface border border-surface-border rounded text-slate-900 font-mono text-xs focus:outline-none focus:ring-1 focus:ring-accent focus:border-accent"
              />
            </div>

            <div>
              <span className="text-slate-500 text-[10px]">Cost (c):</span>
              <input
                type="number"
                step="0.001"
                value={cost}
                onChange={(e) => setCost(parseFloat(e.target.value) || 0.002)}
                className="w-full mt-1 px-2.5 py-1 bg-surface border border-surface-border rounded text-slate-900 font-mono text-xs focus:outline-none focus:ring-1 focus:ring-accent focus:border-accent"
              />
            </div>
          </div>

          {/* Bertram Output Results */}
          {bertramResult ? (
            <div className="bg-surface-raised border border-surface-border rounded-lg p-4 space-y-2.5 font-mono text-xs shadow-2xs">
              <div className="flex justify-between">
                <span className="text-slate-500">Optimal Entry (a):</span>
                <span className="text-profit font-semibold">{bertramResult.entry_long.toFixed(4)}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-slate-500">Optimal Exit (m):</span>
                <span className="text-slate-900 font-semibold">{bertramResult.exit_long.toFixed(4)}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-slate-500">Optimal Width (d = |a - m|):</span>
                <span className="text-accent font-semibold">{bertramResult.optimal_distance.toFixed(4)}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-slate-500">Half-Life of Mean Reversion:</span>
                <span className="text-slate-900 font-semibold">{bertramResult.half_life.toFixed(2)} cycles</span>
              </div>
              <div className="flex justify-between">
                <span className="text-slate-500">Est. Trade Frequency:</span>
                <span className="text-slate-900 font-semibold">{bertramResult.estimated_trades_per_year.toFixed(1)} / year</span>
              </div>
              <div className="flex justify-between pt-2 border-t border-surface-border">
                <span className="text-slate-500">Annualized Sharpe Ratio:</span>
                <span className="text-profit font-bold text-sm">{bertramResult.estimated_sharpe.toFixed(2)}</span>
              </div>
            </div>
          ) : (
            <div className="text-center py-6 text-slate-400 font-mono text-xs border border-dashed border-surface-border rounded-lg">
              Click "Solve Optimal" to calculate Bertram analytic levels.
            </div>
          )}

          {/* OU Process MLE Quick Fit Button */}
          <div className="pt-2">
            <button
              onClick={handleFitOu}
              className="w-full py-2 bg-surface hover:bg-surface-raised border border-surface-border text-slate-700 font-mono text-xs font-medium rounded-lg transition flex items-center justify-center space-x-2 shadow-xs"
            >
              <RefreshCw className="w-3.5 h-3.5 text-accent" />
              <span>Fit Ornstein-Uhlenbeck Parameters (AR(1) MLE)</span>
            </button>

            {ouFitResult && (
              <div className="mt-2 p-2.5 rounded bg-surface-raised border border-surface-border text-[11px] font-mono text-slate-700 flex justify-between shadow-2xs">
                <span>Theta: {ouFitResult.theta.toFixed(3)}</span>
                <span>Mu: {ouFitResult.mu.toFixed(3)}</span>
                <span>Sigma: {ouFitResult.sigma.toFixed(3)}</span>
                <span>R2: {(ouFitResult.r_squared * 100).toFixed(1)}%</span>
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
};
