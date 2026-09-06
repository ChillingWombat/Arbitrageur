import React from "react";
import { ShieldCheck, RotateCcw, AlertOctagon } from "lucide-react";
import type { RiskOverviewDto } from "../types";

interface RiskOperationsProps {
  riskData: RiskOverviewDto | null;
  onResetCircuitBreaker: () => void;
  onTriggerKillswitch: () => void;
}

export const RiskOperations: React.FC<RiskOperationsProps> = ({
  riskData,
  onResetCircuitBreaker,
  onTriggerKillswitch,
}) => {
  if (!riskData) {
    return (
      <div className="p-6 text-center text-slate-500 font-mono text-xs">
        Loading risk guardrails & DeltaGuard telemetry...
      </div>
    );
  }

  const isTripped = riskData.is_halted;
  const margin = riskData.margin_health;

  return (
    <div className="p-6 space-y-6 overflow-y-auto h-full">
      {/* Top Banner */}
      <div className="bg-surface p-5 rounded-xl border border-surface-border flex flex-wrap items-center justify-between gap-4">
        <div>
          <h2 className="text-sm font-bold font-mono text-white flex items-center space-x-2">
            <ShieldCheck className="w-4 h-4 text-profit" />
            <span>DeltaGuard & Enterprise Risk Engine Controls</span>
          </h2>
          <p className="text-xs text-slate-400 mt-1">
            Real-time signed cross-exchange net delta tracking, drawdown tripwires, and maintenance margin proximity.
          </p>
        </div>

        <div className="flex items-center space-x-3">
          {isTripped ? (
            <button
              onClick={onResetCircuitBreaker}
              className="px-3 py-1.5 bg-amber-500/20 hover:bg-amber-500/30 text-amber-400 border border-amber-500/50 rounded-lg text-xs font-mono font-bold flex items-center space-x-1.5 transition"
            >
              <RotateCcw className="w-3.5 h-3.5" />
              <span>Reset Circuit Breaker</span>
            </button>
          ) : (
            <button
              onClick={onTriggerKillswitch}
              className="px-3 py-1.5 bg-loss/20 hover:bg-loss/30 text-loss border border-loss/50 rounded-lg text-xs font-mono font-bold flex items-center space-x-1.5 transition"
            >
              <AlertOctagon className="w-3.5 h-3.5" />
              <span>Trigger Manual Trip</span>
            </button>
          )}
        </div>
      </div>

      {/* 3 Metric Cards */}
      <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
        {/* Card 1: Circuit Breaker */}
        <div className="bg-surface border border-surface-border rounded-xl p-5 space-y-3">
          <div className="flex items-center justify-between">
            <span className="text-xs font-mono uppercase text-slate-400">Circuit Breaker</span>
            <span
              className={`px-2 py-0.5 rounded text-[10px] font-mono font-bold ${
                isTripped
                  ? "bg-loss/20 text-loss border border-loss/40 animate-pulse"
                  : "bg-profit/20 text-profit border border-profit/40"
              }`}
            >
              {riskData.circuit_breaker_state}
            </span>
          </div>

          <div className="space-y-1">
            <div className="flex justify-between text-xs font-mono">
              <span className="text-slate-400">Max Drawdown:</span>
              <span className="text-white font-bold">${riskData.max_drawdown_limit_usd.toLocaleString()}</span>
            </div>
            <div className="flex justify-between text-xs font-mono">
              <span className="text-slate-400">Consecutive Errors:</span>
              <span className="text-white font-bold">{riskData.consecutive_failures} / {riskData.max_failures_allowed}</span>
            </div>
          </div>

          <div className="pt-2">
            <div className="h-1.5 w-full bg-surface-raised rounded-full overflow-hidden">
              <div
                className="h-full bg-profit transition-all duration-300"
                style={{ width: `${Math.max(5, (riskData.consecutive_failures / riskData.max_failures_allowed) * 100)}%` }}
              />
            </div>
          </div>
        </div>

        {/* Card 2: Margin Monitor */}
        <div className="bg-surface border border-surface-border rounded-xl p-5 space-y-3">
          <div className="flex items-center justify-between">
            <span className="text-xs font-mono uppercase text-slate-400">Margin Proximity</span>
            <span className="px-2 py-0.5 rounded text-[10px] font-mono font-bold bg-profit/20 text-profit border border-profit/40">
              {margin.status}
            </span>
          </div>

          <div className="space-y-1">
            <div className="flex justify-between text-xs font-mono">
              <span className="text-slate-400">Margin Ratio:</span>
              <span className="text-white font-bold">{margin.margin_ratio_pct.toFixed(2)}%</span>
            </div>
            <div className="flex justify-between text-xs font-mono">
              <span className="text-slate-400">Liquidation Distance:</span>
              <span className="text-profit font-bold">+{margin.liquidation_distance_pct.toFixed(1)}% safe</span>
            </div>
          </div>

          <div className="pt-2">
            <div className="h-1.5 w-full bg-surface-raised rounded-full overflow-hidden">
              <div
                className="h-full bg-accent"
                style={{ width: `${margin.liquidation_distance_pct}%` }}
              />
            </div>
          </div>
        </div>

        {/* Card 3: Portfolio Capital */}
        <div className="bg-surface border border-surface-border rounded-xl p-5 space-y-3">
          <div className="flex items-center justify-between">
            <span className="text-xs font-mono uppercase text-slate-400">Portfolio Capital</span>
            <span className="text-xs font-mono text-accent font-bold">Consolidated</span>
          </div>

          <div className="space-y-1">
            <div className="flex justify-between text-xs font-mono">
              <span className="text-slate-400">Total Equity:</span>
              <span className="text-white font-bold">${riskData.total_portfolio_value_usd.toLocaleString()}</span>
            </div>
            <div className="flex justify-between text-xs font-mono">
              <span className="text-slate-400">Maint. Req:</span>
              <span className="text-slate-300">${margin.maintenance_margin_req_usd.toLocaleString()}</span>
            </div>
          </div>

          <div className="pt-2">
            <div className="h-1.5 w-full bg-surface-raised rounded-full overflow-hidden">
              <div
                className="h-full bg-profit"
                style={{ width: "95%" }}
              />
            </div>
          </div>
        </div>
      </div>

      {/* DeltaGuard Net Exposure Table */}
      <div className="bg-surface rounded-xl border border-surface-border p-5">
        <h3 className="text-xs font-mono uppercase tracking-wider text-slate-400 mb-4 flex items-center justify-between">
          <span>DeltaGuard Cross-Exchange Signed Net Exposure</span>
          <span className="text-slate-500 font-normal lowercase">enforcing strict delta-neutrality across positions</span>
        </h3>

        <div className="overflow-x-auto">
          <table className="w-full text-left text-xs font-mono">
            <thead>
              <tr className="border-b border-surface-border text-slate-400 uppercase text-[10px] tracking-wider">
                <th className="py-2.5 px-3">Base Asset</th>
                <th className="py-2.5 px-3">Net Quantity</th>
                <th className="py-2.5 px-3">USD Exposure</th>
                <th className="py-2.5 px-3">Tolerance Limit</th>
                <th className="py-2.5 px-3 text-right">Status</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-surface-border">
              {riskData.net_deltas.map((delta) => {
                const isNeutral = Math.abs(delta.net_exposure_usd) < 500;
                return (
                  <tr key={delta.asset} className="hover:bg-surface-raised transition">
                    <td className="py-3 px-3 font-bold text-white">{delta.asset}</td>
                    <td className="py-3 px-3 text-slate-300">
                      {delta.net_quantity >= 0 ? `+${delta.net_quantity.toFixed(4)}` : delta.net_quantity.toFixed(4)}
                    </td>
                    <td className="py-3 px-3">
                      <span
                        className={`font-semibold ${
                          delta.net_exposure_usd >= 0 ? "text-slate-200" : "text-amber-300"
                        }`}
                      >
                        ${delta.net_exposure_usd.toFixed(2)}
                      </span>
                    </td>
                    <td className="py-3 px-3 text-slate-400">±${delta.delta_limit_usd.toLocaleString()}</td>
                    <td className="py-3 px-3 text-right">
                      {isNeutral ? (
                        <span className="px-2 py-0.5 rounded text-[10px] font-bold bg-profit/20 text-profit border border-profit/30">
                          DELTA NEUTRAL
                        </span>
                      ) : (
                        <span className="px-2 py-0.5 rounded text-[10px] font-bold bg-amber-500/20 text-amber-400 border border-amber-500/30">
                          EXPOSURE SKEW
                        </span>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  );
};
