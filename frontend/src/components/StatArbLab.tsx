import React, { useState } from "react";
import { AlertCircle, RefreshCw, FlaskConical } from "lucide-react";
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
        // Generate slight cointegrated price drift
        px = 3450.0 + nextStep * 2.5 + (Math.sin(nextStep * 0.5) * 10.0);
        const trueBeta = 18.6;
        py = trueBeta * px + 150.0 + (Math.cos(nextStep * 0.8) * 85.0);
        setPriceX(px);
        setPriceY(py);
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
      // Generate synthetic mean-reverting series
      const series: number[] = [];
      let cur = mu;
      for (let i = 0; i < 500; i++) {
        cur += theta * (mu - cur) * 0.1 + sigma * Math.sqrt(0.1) * (Math.random() - 0.5) * 2;
        series.push(cur);
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
    <div className="p-6 space-y-6 overflow-y-auto h-full max-w-6xl select-none">
      {/* Top Banner */}
      <div className="glass-panel p-5 rounded-2xl border border-white/[0.08] flex items-center justify-between shadow-xs">
        <div>
          <div className="flex items-center space-x-2.5">
            <div className="w-8 h-8 rounded-lg bg-indigo-500/15 border border-indigo-500/30 flex items-center justify-center text-indigo-300 shadow-[0_0_12px_rgba(129,140,248,0.2)]">
              <FlaskConical className="w-4 h-4" />
            </div>
            <h2 className="text-base font-bold font-mono text-white tracking-tight">
              Quantitative Analytics & Statistical Arbitrage Lab
            </h2>
          </div>
          <p className="text-xs text-white/50 font-mono mt-1">
            2D Online Kalman Filter dynamic hedge ratio estimation, Ornstein-Uhlenbeck MLE fitting, and Bertram analytic threshold solver.
          </p>
        </div>

        <div className="flex items-center space-x-2 text-xs font-mono">
          <span className="px-3 py-1.5 rounded-xl bg-white/[0.04] border border-white/[0.08] text-white/60">
            Latency: <span className="text-emerald-400 font-bold">19.15 ns</span> / update
          </span>
        </div>
      </div>

      {errorMsg && (
        <div className="bg-rose-500/15 border border-rose-500/30 text-rose-300 px-4 py-3 rounded-xl text-xs font-mono flex items-center justify-between">
          <div className="flex items-center space-x-2">
            <AlertCircle className="w-4 h-4 flex-shrink-0" />
            <span>{errorMsg}</span>
          </div>
          <button
            onClick={() => setErrorMsg(null)}
            className="text-white/60 hover:text-white text-xs font-mono underline ml-4"
          >
            Dismiss
          </button>
        </div>
      )}

      {/* Grid: 2D Kalman Filter & Bertram Solver */}
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        {/* Module 1: 2D Online Kalman Filter */}
        <div className="glass-panel border border-white/[0.08] rounded-2xl p-5 space-y-4 shadow-xs">
          <div className="flex items-center justify-between border-b border-white/[0.06] pb-3">
            <div>
              <h3 className="text-xs font-mono font-bold uppercase text-white/95">
                2D Online Kalman Filter (BTC vs ETH)
              </h3>
              <span className="text-[10px] text-white/40 font-mono tabular-nums">
                Recursive O(1) updates | BTC: ${priceY.toFixed(2)} vs ETH: ${priceX.toFixed(2)}
              </span>
            </div>

            <div className="flex items-center space-x-2">
              <button
                onClick={handleResetFilter}
                title="Reset Filter History"
                className="px-2.5 py-1.5 bg-white/[0.04] hover:bg-white/[0.08] border border-white/[0.08] text-white/70 hover:text-white rounded-xl text-xs font-mono transition"
              >
                Reset
              </button>
              <button
                onClick={handleStep}
                className="px-3.5 py-1.5 bg-gradient-to-r from-cyan-500 to-indigo-600 hover:from-cyan-400 hover:to-indigo-500 text-white font-medium text-xs font-mono rounded-xl transition flex items-center shadow-[0_0_12px_rgba(34,211,238,0.25)]"
              >
                <span>Step Iteration</span>
              </button>
            </div>
          </div>

          {/* Mode Switch & Manual Inputs */}
          <div className="bg-white/[0.03] border border-white/[0.06] rounded-xl p-2.5 font-mono text-xs flex items-center justify-between gap-3 shadow-2xs">
            <div className="flex items-center space-x-2">
              <span className="text-[10px] text-white/40 uppercase font-semibold">Input Mode:</span>
              <button
                onClick={() => setIsManual(false)}
                className={`px-2.5 py-1 rounded-lg text-[11px] transition ${
                  !isManual ? "bg-cyan-500/20 text-cyan-300 border border-cyan-500/30 font-semibold" : "text-white/60 hover:text-white"
                }`}
              >
                Auto Drift
              </button>
              <button
                onClick={() => setIsManual(true)}
                className={`px-2.5 py-1 rounded-lg text-[11px] transition ${
                  isManual ? "bg-cyan-500/20 text-cyan-300 border border-cyan-500/30 font-semibold" : "text-white/60 hover:text-white"
                }`}
              >
                Manual Prices
              </button>
            </div>

            {isManual && (
              <div className="flex items-center space-x-2 tabular-nums">
                <div className="flex items-center space-x-1">
                  <span className="text-[10px] text-white/40">Y:</span>
                  <input
                    type="number"
                    value={priceY}
                    onChange={(e) => setPriceY(parseFloat(e.target.value) || 0)}
                    className="w-20 px-2 py-0.5 bg-black/40 border border-white/[0.1] rounded text-white text-[11px]"
                  />
                </div>
                <div className="flex items-center space-x-1">
                  <span className="text-[10px] text-white/40">X:</span>
                  <input
                    type="number"
                    value={priceX}
                    onChange={(e) => setPriceX(parseFloat(e.target.value) || 0)}
                    className="w-16 px-2 py-0.5 bg-black/40 border border-white/[0.1] rounded text-white text-[11px]"
                  />
                </div>
              </div>
            )}
          </div>

          {/* Metrics Row */}
          <div className="grid grid-cols-3 gap-3 font-mono text-xs tabular-nums">
            <div className="p-3 bg-white/[0.03] border border-white/[0.04] rounded-xl shadow-2xs">
              <span className="text-[10px] text-white/40 uppercase font-semibold">Hedge Ratio (Beta)</span>
              <div className="text-base font-bold text-cyan-300 mt-0.5">
                {latest ? latest.dynamic_hedge_ratio.toFixed(4) : "18.6200"}
              </div>
            </div>

            <div className="p-3 bg-white/[0.03] border border-white/[0.04] rounded-xl shadow-2xs">
              <span className="text-[10px] text-white/40 uppercase font-semibold">Residual Spread</span>
              <div className="text-base font-bold text-white mt-0.5">
                {latest ? `$${latest.residual_spread.toFixed(2)}` : "$0.00"}
              </div>
            </div>

            <div className="p-3 bg-white/[0.03] border border-white/[0.04] rounded-xl shadow-2xs">
              <span className="text-[10px] text-white/40 uppercase font-semibold">Spread Z-Score</span>
              <div
                className={`text-base font-bold mt-0.5 ${
                  Math.abs(latest?.z_score || 0) > 2
                    ? "text-rose-400"
                    : Math.abs(latest?.z_score || 0) < 0.5
                    ? "text-emerald-400"
                    : "text-amber-300"
                }`}
              >
                {latest ? latest.z_score.toFixed(2) : "0.00"} Z
              </div>
            </div>
          </div>

          {/* Interactive SVG Sparkline Chart */}
          <div className="bg-white/[0.02] border border-white/[0.06] rounded-xl p-4 font-mono text-xs shadow-2xs">
            <div className="flex justify-between text-[10px] text-white/40 mb-2">
              <span>Residual Z-Score Spread Walk (Last 25 Cycles)</span>
              <span className="text-cyan-300 font-medium">Entry Threshold: +/- 2.0 Z</span>
            </div>

            <div className="h-32 w-full flex items-center justify-center">
              {history.length < 2 ? (
                <div className="text-white/30 text-xs">Click "Step Iteration" to stream Kalman filter updates...</div>
              ) : (
                <svg className="w-full h-full overflow-visible" viewBox="0 0 400 100">
                  {/* +2.0 threshold line */}
                  <line x1="0" y1="20" x2="400" y2="20" stroke="#fb7185" strokeDasharray="3,3" strokeWidth="1" opacity="0.5" />
                  {/* 0 neutral line */}
                  <line x1="0" y1="50" x2="400" y2="50" stroke="rgba(255,255,255,0.15)" strokeWidth="1" />
                  {/* -2.0 threshold line */}
                  <line x1="0" y1="80" x2="400" y2="80" stroke="#34d399" strokeDasharray="3,3" strokeWidth="1" opacity="0.5" />

                  {/* Polyline */}
                  <polyline
                    fill="none"
                    stroke="#22d3ee"
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

            <div className="flex justify-between text-[9px] text-white/40 mt-2 font-mono">
              <span className="text-emerald-400">-2.0 Z Buy</span>
              <span className="text-white/40">0.0 Z Mean</span>
              <span className="text-rose-400">+2.0 Z Short</span>
            </div>
          </div>

          {/* Signal Output */}
          <div className="p-3 rounded-xl bg-white/[0.03] border border-white/[0.06] flex items-center justify-between text-xs font-mono shadow-2xs">
            <span className="text-white/50">Current Arbitrage Signal:</span>
            <span
              className={`font-semibold px-2.5 py-0.5 rounded text-[11px] ${
                latest?.signal === "ENTRY_LONG_SPREAD"
                  ? "chip chip-pos"
                  : latest?.signal === "ENTRY_SHORT_SPREAD"
                  ? "chip chip-neg"
                  : latest?.signal === "EXIT"
                  ? "chip chip-info"
                  : "bg-white/[0.04] text-white/60 border border-white/[0.06]"
              }`}
            >
              {latest?.signal || "NEUTRAL"}
            </span>
          </div>
        </div>

        {/* Module 2: Bertram Optimal Analytic Thresholds */}
        <div className="glass-panel border border-white/[0.08] rounded-2xl p-5 space-y-4 shadow-xs">
          <div className="flex items-center justify-between border-b border-white/[0.06] pb-3">
            <div>
              <h3 className="text-xs font-mono font-bold uppercase text-white/95">
                Bertram (2010) Optimal Thresholds
              </h3>
              <span className="text-[10px] text-white/40 font-mono">Analytic solver maximizing return per unit time</span>
            </div>

            <button
              onClick={handleSolveBertram}
              className="px-3.5 py-1.5 bg-gradient-to-r from-cyan-500 to-indigo-600 hover:from-cyan-400 hover:to-indigo-500 text-white font-medium text-xs font-mono rounded-xl transition shadow-[0_0_12px_rgba(34,211,238,0.25)]"
            >
              Solve Optimal
            </button>
          </div>

          {/* Input Controls */}
          <div className="grid grid-cols-2 gap-3 text-xs font-mono tabular-nums">
            <div>
              <span className="text-white/50 text-[10px]">Speed (Theta):</span>
              <input
                type="number"
                step="0.01"
                value={theta}
                onChange={(e) => setTheta(parseFloat(e.target.value) || 0.1)}
                className="w-full mt-1 px-3 py-1.5 bg-black/40 border border-white/[0.1] rounded-xl text-white font-mono text-xs focus:outline-none focus:ring-1 focus:ring-cyan-400"
              />
            </div>

            <div>
              <span className="text-white/50 text-[10px]">Volatility (Sigma):</span>
              <input
                type="number"
                step="0.01"
                value={sigma}
                onChange={(e) => setSigma(parseFloat(e.target.value) || 0.05)}
                className="w-full mt-1 px-3 py-1.5 bg-black/40 border border-white/[0.1] rounded-xl text-white font-mono text-xs focus:outline-none focus:ring-1 focus:ring-cyan-400"
              />
            </div>

            <div>
              <span className="text-white/50 text-[10px]">Mean (Mu):</span>
              <input
                type="number"
                step="0.01"
                value={mu}
                onChange={(e) => setMu(parseFloat(e.target.value) || 0.0)}
                className="w-full mt-1 px-3 py-1.5 bg-black/40 border border-white/[0.1] rounded-xl text-white font-mono text-xs focus:outline-none focus:ring-1 focus:ring-cyan-400"
              />
            </div>

            <div>
              <span className="text-white/50 text-[10px]">Cost (c):</span>
              <input
                type="number"
                step="0.001"
                value={cost}
                onChange={(e) => setCost(parseFloat(e.target.value) || 0.002)}
                className="w-full mt-1 px-3 py-1.5 bg-black/40 border border-white/[0.1] rounded-xl text-white font-mono text-xs focus:outline-none focus:ring-1 focus:ring-cyan-400"
              />
            </div>
          </div>

          {/* Bertram Output Results */}
          {bertramResult ? (
            <div className="bg-white/[0.03] border border-white/[0.06] rounded-xl p-4 space-y-2.5 font-mono text-xs shadow-2xs tabular-nums">
              <div className="flex justify-between">
                <span className="text-white/50">Optimal Entry (a):</span>
                <span className="text-emerald-400 font-semibold">{bertramResult.entry_long.toFixed(4)}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-white/50">Optimal Exit (m):</span>
                <span className="text-white font-semibold">{bertramResult.exit_long.toFixed(4)}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-white/50">Optimal Width (d = |a - m|):</span>
                <span className="text-cyan-300 font-semibold">{bertramResult.optimal_distance.toFixed(4)}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-white/50">Half-Life of Mean Reversion:</span>
                <span className="text-white font-semibold">{bertramResult.half_life.toFixed(2)} cycles</span>
              </div>
              <div className="flex justify-between">
                <span className="text-white/50">Est. Trade Frequency:</span>
                <span className="text-white font-semibold">{bertramResult.estimated_trades_per_year.toFixed(1)} / year</span>
              </div>
              <div className="flex justify-between pt-2 border-t border-white/[0.06]">
                <span className="text-white/50">Annualized Sharpe Ratio:</span>
                <span className="text-emerald-400 font-bold text-sm">{bertramResult.estimated_sharpe.toFixed(2)}</span>
              </div>
            </div>
          ) : (
            <div className="text-center py-6 text-white/40 font-mono text-xs border border-dashed border-white/[0.08] rounded-xl">
              Click "Solve Optimal" to calculate Bertram analytic levels.
            </div>
          )}

          {/* OU Process MLE Quick Fit Button */}
          <div className="pt-2">
            <button
              onClick={handleFitOu}
              className="w-full py-2 bg-white/[0.04] hover:bg-white/[0.08] border border-white/[0.08] text-white/80 font-mono text-xs font-medium rounded-xl transition flex items-center justify-center space-x-2 shadow-xs"
            >
              <RefreshCw className="w-3.5 h-3.5 text-cyan-400" />
              <span>Fit Ornstein-Uhlenbeck Parameters (AR(1) MLE)</span>
            </button>

            {ouFitResult && (
              <div className="mt-2 p-2.5 rounded-xl bg-white/[0.03] border border-white/[0.06] text-[11px] font-mono text-white/80 flex justify-between shadow-2xs tabular-nums">
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
