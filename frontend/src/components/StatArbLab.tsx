import React, { useState } from "react";
import { FlaskConical, Play, RefreshCw, Activity, Cpu, AlertCircle, RotateCcw } from "lucide-react";
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
    <div className="p-6 space-y-6 overflow-y-auto h-full">
      {/* Top Banner */}
      <div className="bg-surface p-5 rounded-xl border border-surface-border flex items-center justify-between">
        <div>
          <h2 className="text-sm font-bold font-mono text-white flex items-center space-x-2">
            <FlaskConical className="w-4 h-4 text-accent" />
            <span>Quantitative Analytics & Statistical Arbitrage Lab</span>
          </h2>
          <p className="text-xs text-slate-400 mt-1">
            2D Online Kalman Filter dynamic hedge ratio estimation, Ornstein-Uhlenbeck MLE fitting, and Bertram analytic threshold solver.
          </p>
        </div>

        <div className="flex items-center space-x-2 text-xs font-mono">
          <span className="px-2.5 py-1 rounded bg-surface-raised border border-surface-border text-slate-400">
            Latency: <span className="text-profit font-bold">19.15 ns</span> / update
          </span>
        </div>
      </div>

      {errorMsg && (
        <div className="bg-loss/10 border border-loss/30 text-loss px-4 py-3 rounded-xl text-xs font-mono flex items-center justify-between">
          <div className="flex items-center space-x-2">
            <AlertCircle className="w-4 h-4 flex-shrink-0" />
            <span>{errorMsg}</span>
          </div>
          <button
            onClick={() => setErrorMsg(null)}
            className="text-slate-400 hover:text-white text-xs font-mono underline ml-4"
          >
            Dismiss
          </button>
        </div>
      )}

      {/* Grid: 2D Kalman Filter & Bertram Solver */}
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        {/* Module 1: 2D Online Kalman Filter */}
        <div className="bg-surface border border-surface-border rounded-xl p-5 space-y-4">
          <div className="flex items-center justify-between border-b border-surface-border pb-3">
            <div>
              <h3 className="text-xs font-mono font-bold uppercase text-white flex items-center space-x-2">
                <Cpu className="w-4 h-4 text-accent" />
                <span>2D Online Kalman Filter (BTC vs ETH)</span>
              </h3>
              <span className="text-[10px] text-slate-400 font-mono">
                Recursive O(1) updates • BTC: ${priceY.toFixed(2)} vs ETH: ${priceX.toFixed(2)}
              </span>
            </div>

            <div className="flex items-center space-x-2">
              <button
                onClick={handleResetFilter}
                title="Reset Filter History"
                className="p-1.5 bg-surface-raised hover:bg-surface-border border border-surface-border text-slate-400 hover:text-white rounded-lg transition"
              >
                <RotateCcw className="w-3.5 h-3.5" />
              </button>
              <button
                onClick={handleStep}
                className="px-3 py-1.5 bg-accent hover:bg-cyan-400 text-background font-bold text-xs font-mono rounded-lg transition flex items-center space-x-1"
              >
                <Play className="w-3 h-3" />
                <span>Step Iteration</span>
              </button>
            </div>
          </div>

          {/* Mode Switch & Manual Inputs */}
          <div className="bg-surface-raised border border-surface-border rounded-lg p-2.5 font-mono text-xs flex items-center justify-between gap-3">
            <div className="flex items-center space-x-2">
              <span className="text-[10px] text-slate-400 uppercase">Input Mode:</span>
              <button
                onClick={() => setIsManual(false)}
                className={`px-2 py-0.5 rounded text-[11px] transition ${
                  !isManual ? "bg-accent text-background font-bold" : "text-slate-400 hover:text-white"
                }`}
              >
                Auto Drift
              </button>
              <button
                onClick={() => setIsManual(true)}
                className={`px-2 py-0.5 rounded text-[11px] transition ${
                  isManual ? "bg-accent text-background font-bold" : "text-slate-400 hover:text-white"
                }`}
              >
                Manual Prices
              </button>
            </div>

            {isManual && (
              <div className="flex items-center space-x-2">
                <div className="flex items-center space-x-1">
                  <span className="text-[10px] text-slate-400">Y:</span>
                  <input
                    type="number"
                    value={priceY}
                    onChange={(e) => setPriceY(parseFloat(e.target.value) || 0)}
                    className="w-20 px-1.5 py-0.5 bg-surface border border-surface-border rounded text-white text-[11px]"
                  />
                </div>
                <div className="flex items-center space-x-1">
                  <span className="text-[10px] text-slate-400">X:</span>
                  <input
                    type="number"
                    value={priceX}
                    onChange={(e) => setPriceX(parseFloat(e.target.value) || 0)}
                    className="w-16 px-1.5 py-0.5 bg-surface border border-surface-border rounded text-white text-[11px]"
                  />
                </div>
              </div>
            )}
          </div>

          {/* Metrics Row */}
          <div className="grid grid-cols-3 gap-3 font-mono text-xs">
            <div className="p-3 bg-surface-raised border border-surface-border rounded-lg">
              <span className="text-[10px] text-slate-400 uppercase">Hedge Ratio (β)</span>
              <div className="text-base font-bold text-accent mt-0.5">
                {latest ? latest.dynamic_hedge_ratio.toFixed(4) : "18.6200"}
              </div>
            </div>

            <div className="p-3 bg-surface-raised border border-surface-border rounded-lg">
              <span className="text-[10px] text-slate-400 uppercase">Residual Spread</span>
              <div className="text-base font-bold text-white mt-0.5">
                {latest ? `$${latest.residual_spread.toFixed(2)}` : "$0.00"}
              </div>
            </div>

            <div className="p-3 bg-surface-raised border border-surface-border rounded-lg">
              <span className="text-[10px] text-slate-400 uppercase">Spread Z-Score</span>
              <div
                className={`text-base font-bold mt-0.5 ${
                  Math.abs(latest?.z_score || 0) > 2
                    ? "text-loss"
                    : Math.abs(latest?.z_score || 0) < 0.5
                    ? "text-profit"
                    : "text-amber-400"
                }`}
              >
                {latest ? latest.z_score.toFixed(2) : "0.00"}σ
              </div>
            </div>
          </div>

          {/* Interactive SVG Sparkline Chart */}
          <div className="bg-surface-raised border border-surface-border rounded-lg p-4 font-mono text-xs">
            <div className="flex justify-between text-[10px] text-slate-400 mb-2">
              <span>Residual Z-Score Spread Walk (Last 25 Cycles)</span>
              <span className="text-accent">Entry Threshold: ±2.0σ</span>
            </div>

            <div className="h-32 w-full flex items-center justify-center">
              {history.length < 2 ? (
                <div className="text-slate-500 text-xs">Click "Step Iteration" to stream Kalman filter updates...</div>
              ) : (
                <svg className="w-full h-full overflow-visible" viewBox="0 0 400 100">
                  {/* +2.0 threshold line */}
                  <line x1="0" y1="20" x2="400" y2="20" stroke="#f43f5e" strokeDasharray="3,3" strokeWidth="1" />
                  {/* 0 neutral line */}
                  <line x1="0" y1="50" x2="400" y2="50" stroke="#475569" strokeWidth="1" />
                  {/* -2.0 threshold line */}
                  <line x1="0" y1="80" x2="400" y2="80" stroke="#10b981" strokeDasharray="3,3" strokeWidth="1" />

                  {/* Polyline */}
                  <polyline
                    fill="none"
                    stroke="#00f0ff"
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

            <div className="flex justify-between text-[9px] text-slate-500 mt-2">
              <span className="text-profit">-2.0σ Buy Spread</span>
              <span className="text-slate-400">0.0σ Mean Equilibrium</span>
              <span className="text-loss">+2.0σ Short Spread</span>
            </div>
          </div>

          {/* Signal Output */}
          <div className="p-3 rounded-lg bg-surface-raised border border-surface-border flex items-center justify-between text-xs font-mono">
            <span className="text-slate-400">Current Arbitrage Signal:</span>
            <span
              className={`font-bold px-2 py-0.5 rounded ${
                latest?.signal === "ENTRY_LONG_SPREAD"
                  ? "bg-profit/20 text-profit border border-profit/30"
                  : latest?.signal === "ENTRY_SHORT_SPREAD"
                  ? "bg-loss/20 text-loss border border-loss/30"
                  : latest?.signal === "EXIT"
                  ? "bg-accent/20 text-accent border border-accent/30"
                  : "bg-surface text-slate-400"
              }`}
            >
              {latest?.signal || "NEUTRAL"}
            </span>
          </div>
        </div>

        {/* Module 2: Bertram Optimal Analytic Thresholds */}
        <div className="bg-surface border border-surface-border rounded-xl p-5 space-y-4">
          <div className="flex items-center justify-between border-b border-surface-border pb-3">
            <div>
              <h3 className="text-xs font-mono font-bold uppercase text-white flex items-center space-x-2">
                <Activity className="w-4 h-4 text-profit" />
                <span>Bertram (2010) Optimal Thresholds</span>
              </h3>
              <span className="text-[10px] text-slate-400 font-mono">Analytic solver maximizing return / unit time</span>
            </div>

            <button
              onClick={handleSolveBertram}
              className="px-3 py-1.5 bg-profit hover:bg-emerald-400 text-background font-bold text-xs font-mono rounded-lg transition"
            >
              Solve Optimal
            </button>
          </div>

          {/* Input Controls */}
          <div className="grid grid-cols-2 gap-3 text-xs font-mono">
            <div>
              <span className="text-slate-400 text-[10px]">Speed (θ):</span>
              <input
                type="number"
                step="0.01"
                value={theta}
                onChange={(e) => setTheta(parseFloat(e.target.value) || 0.1)}
                className="w-full mt-1 px-2.5 py-1 bg-surface-raised border border-surface-border rounded text-white font-mono text-xs focus:outline-none focus:border-profit"
              />
            </div>

            <div>
              <span className="text-slate-400 text-[10px]">Volatility (σ):</span>
              <input
                type="number"
                step="0.01"
                value={sigma}
                onChange={(e) => setSigma(parseFloat(e.target.value) || 0.05)}
                className="w-full mt-1 px-2.5 py-1 bg-surface-raised border border-surface-border rounded text-white font-mono text-xs focus:outline-none focus:border-profit"
              />
            </div>

            <div>
              <span className="text-slate-400 text-[10px]">Equilibrium Mean (μ):</span>
              <input
                type="number"
                step="0.01"
                value={mu}
                onChange={(e) => setMu(parseFloat(e.target.value) || 0.0)}
                className="w-full mt-1 px-2.5 py-1 bg-surface-raised border border-surface-border rounded text-white font-mono text-xs focus:outline-none focus:border-profit"
              />
            </div>

            <div>
              <span className="text-slate-400 text-[10px]">Transaction Cost (c):</span>
              <input
                type="number"
                step="0.001"
                value={cost}
                onChange={(e) => setCost(parseFloat(e.target.value) || 0.002)}
                className="w-full mt-1 px-2.5 py-1 bg-surface-raised border border-surface-border rounded text-white font-mono text-xs focus:outline-none focus:border-profit"
              />
            </div>
          </div>

          {/* Bertram Output Results */}
          {bertramResult ? (
            <div className="bg-surface-raised border border-surface-border rounded-lg p-4 space-y-2.5 font-mono text-xs">
              <div className="flex justify-between">
                <span className="text-slate-400">Optimal Entry (a):</span>
                <span className="text-profit font-bold">{bertramResult.entry_long.toFixed(4)}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-slate-400">Optimal Exit (m):</span>
                <span className="text-white font-bold">{bertramResult.exit_long.toFixed(4)}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-slate-400">Optimal Width (d = |a - m|):</span>
                <span className="text-accent font-bold">{bertramResult.optimal_distance.toFixed(4)}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-slate-400">Half-Life of Mean Reversion:</span>
                <span className="text-white font-bold">{bertramResult.half_life.toFixed(2)} cycles</span>
              </div>
              <div className="flex justify-between">
                <span className="text-slate-400">Est. Trade Frequency:</span>
                <span className="text-white font-bold">{bertramResult.estimated_trades_per_year.toFixed(1)} / year</span>
              </div>
              <div className="flex justify-between pt-2 border-t border-surface-border">
                <span className="text-slate-400">Annualized Sharpe Ratio:</span>
                <span className="text-profit font-bold text-sm">{bertramResult.estimated_sharpe.toFixed(2)}</span>
              </div>
            </div>
          ) : (
            <div className="text-center py-6 text-slate-500 font-mono text-xs border border-dashed border-surface-border rounded-lg">
              Click "Solve Optimal" to calculate Bertram analytic levels.
            </div>
          )}

          {/* OU Process MLE Quick Fit Button */}
          <div className="pt-2">
            <button
              onClick={handleFitOu}
              className="w-full py-2 bg-surface-raised hover:bg-surface-border border border-surface-border text-slate-300 font-mono text-xs rounded-lg transition flex items-center justify-center space-x-2"
            >
              <RefreshCw className="w-3.5 h-3.5 text-accent" />
              <span>Fit Ornstein-Uhlenbeck Parameters (AR(1) MLE)</span>
            </button>

            {ouFitResult && (
              <div className="mt-2 p-2.5 rounded bg-surface border border-surface-border text-[11px] font-mono text-slate-300 flex justify-between">
                <span>θ: {ouFitResult.theta.toFixed(3)}</span>
                <span>μ: {ouFitResult.mu.toFixed(3)}</span>
                <span>σ: {ouFitResult.sigma.toFixed(3)}</span>
                <span>R²: {(ouFitResult.r_squared * 100).toFixed(1)}%</span>
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
};
