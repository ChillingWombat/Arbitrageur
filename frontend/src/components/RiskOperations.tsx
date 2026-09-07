import React from "react";
import type { RiskOverviewDto } from "../types";
import { Shield, AlertTriangle, RefreshCw } from "lucide-react";

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
      <div className="p-8 text-center text-white/40 font-mono text-xs flex items-center justify-center space-x-2">
        <RefreshCw className="w-4 h-4 animate-spin text-cyan-400" />
        <span>Loading risk guardrails & DeltaGuard telemetry...</span>
      </div>
    );
  }

  const isTripped = riskData.is_halted;
  const margin = riskData.margin_health;

  return (
    <div className="p-6 space-y-6 overflow-y-auto h-full max-w-6xl select-none">
      {/* Top Banner */}
      <div className="glass-panel p-5 rounded-2xl flex flex-wrap items-center justify-between gap-4 border border-white/[0.08]">
        <div>
          <div className="flex items-center space-x-2.5">
            <div className="w-8 h-8 rounded-lg bg-cyan-500/15 border border-cyan-500/30 flex items-center justify-center text-cyan-300 shadow-[0_0_12px_rgba(34,211,238,0.2)]">
              <Shield className="w-4 h-4" />
            </div>
            <h2 className="text-base font-bold font-mono text-white tracking-tight">
              DeltaGuard & Enterprise Risk Engine Controls
            </h2>
          </div>
          <p className="text-xs text-white/50 font-mono mt-1">
            Real-time signed cross-exchange net delta tracking, drawdown tripwires, and maintenance margin proximity.
          </p>
        </div>

        <div className="flex items-center space-x-3">
          {isTripped ? (
            <button
              onClick={onResetCircuitBreaker}
              className="px-4 py-2 bg-amber-500/20 hover:bg-amber-500/30 text-amber-300 border border-amber-500/40 rounded-xl text-xs font-mono font-medium flex items-center space-x-1.5 transition shadow-[0_0_15px_rgba(245,158,11,0.2)]"
            >
              <RefreshCw className="w-3.5 h-3.5" />
              <span>Reset Circuit Breaker</span>
            </button>
          ) : (
            <button
              onClick={onTriggerKillswitch}
              className="px-4 py-2 bg-rose-500/20 hover:bg-rose-500/30 text-rose-300 border border-rose-500/40 rounded-xl text-xs font-mono font-medium flex items-center space-x-1.5 transition shadow-[0_0_15px_rgba(251,113,133,0.2)]"
            >
              <AlertTriangle className="w-3.5 h-3.5" />
              <span>Trigger Manual Trip</span>
            </button>
          )}
        </div>
      </div>

      {/* 3 Metric Cards */}
      <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
        {/* Card 1: Circuit Breaker */}
        <div className="glass-panel border border-white/[0.08] rounded-2xl p-5 space-y-3.5">
          <div className="flex items-center justify-between">
            <span className="text-xs font-mono uppercase text-white/40 font-semibold">Circuit Breaker</span>
            <span
              className={`chip text-[10px] font-mono font-semibold ${
                isTripped ? "chip-neg" : "chip-pos"
              }`}
            >
              {riskData.circuit_breaker_state}
            </span>
          </div>

          <div className="space-y-1.5">
            <div className="flex justify-between text-xs font-mono tabular-nums">
              <span className="text-white/50">Max Drawdown:</span>
              <span className="text-white font-bold">${riskData.max_drawdown_limit_usd.toLocaleString()}</span>
            </div>
            <div className="flex justify-between text-xs font-mono tabular-nums">
              <span className="text-white/50">Consecutive Errors:</span>
              <span className="text-white font-bold">{riskData.consecutive_failures} / {riskData.max_failures_allowed}</span>
            </div>
          </div>

          <div className="pt-2">
            <div className="h-1.5 w-full bg-white/[0.06] rounded-full overflow-hidden">
              <div
                className={`h-full ${isTripped ? "bg-rose-400" : "bg-emerald-400"} transition-all duration-300`}
                style={{ width: `${Math.max(5, (riskData.consecutive_failures / riskData.max_failures_allowed) * 100)}%` }}
              />
            </div>
          </div>
        </div>

        {/* Card 2: Margin Monitor */}
        <div className="glass-panel border border-white/[0.08] rounded-2xl p-5 space-y-3.5">
          <div className="flex items-center justify-between">
            <span className="text-xs font-mono uppercase text-white/40 font-semibold">Margin Proximity</span>
            <span className="chip chip-pos text-[10px] font-mono font-semibold">
              {margin.status}
            </span>
          </div>

          <div className="space-y-1.5">
            <div className="flex justify-between text-xs font-mono tabular-nums">
              <span className="text-white/50">Margin Ratio:</span>
              <span className="text-white font-bold">{margin.margin_ratio_pct.toFixed(2)}%</span>
            </div>
            <div className="flex justify-between text-xs font-mono tabular-nums">
              <span className="text-white/50">Liquidation Distance:</span>
              <span className="text-emerald-400 font-bold">+{margin.liquidation_distance_pct.toFixed(1)}% safe</span>
            </div>
          </div>

          <div className="pt-2">
            <div className="h-1.5 w-full bg-white/[0.06] rounded-full overflow-hidden">
              <div
                className="h-full bg-cyan-400"
                style={{ width: `${Math.min(100, margin.liquidation_distance_pct)}%` }}
              />
            </div>
          </div>
        </div>

        {/* Card 3: Portfolio Capital */}
        <div className="glass-panel border border-white/[0.08] rounded-2xl p-5 space-y-3.5">
          <div className="flex items-center justify-between">
            <span className="text-xs font-mono uppercase text-white/40 font-semibold">Portfolio Capital</span>
            <span className="text-xs font-mono text-cyan-300 font-semibold">Consolidated</span>
          </div>

          <div className="space-y-1.5">
            <div className="flex justify-between text-xs font-mono tabular-nums">
              <span className="text-white/50">Total Equity:</span>
              <span className="text-white font-bold">${riskData.total_portfolio_value_usd.toLocaleString()}</span>
            </div>
            <div className="flex justify-between text-xs font-mono tabular-nums">
              <span className="text-white/50">Maint. Req:</span>
              <span className="text-white/80 font-medium">${margin.maintenance_margin_req_usd.toLocaleString()}</span>
            </div>
          </div>

          <div className="pt-2">
            <div className="h-1.5 w-full bg-white/[0.06] rounded-full overflow-hidden">
              <div
                className="h-full bg-emerald-400"
                style={{ width: "95%" }}
              />
            </div>
          </div>
        </div>
      </div>

      {/* DeltaGuard Net Exposure Table */}
      <div className="glass-panel rounded-2xl border border-white/[0.08] p-5 shadow-xs">
        <h3 className="text-xs font-mono uppercase tracking-wider text-white/40 mb-4 flex items-center justify-between">
          <span className="text-white/90 font-bold">DeltaGuard Cross-Exchange Signed Net Exposure</span>
          <span className="text-white/40 font-normal lowercase">enforcing strict delta-neutrality across positions</span>
        </h3>

        <div className="overflow-x-auto">
          <table className="w-full text-left text-xs font-mono">
            <thead>
              <tr className="border-b border-white/[0.06] text-white/40 uppercase text-[10px] tracking-wider">
                <th className="py-2.5 px-3 font-semibold">Base Asset</th>
                <th className="py-2.5 px-3 font-semibold">Net Quantity</th>
                <th className="py-2.5 px-3 font-semibold">USD Exposure</th>
                <th className="py-2.5 px-3 font-semibold">Tolerance Limit</th>
                <th className="py-2.5 px-3 text-right font-semibold">Status</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-white/[0.06]">
              {riskData.net_deltas.map((delta) => {
                const isNeutral = Math.abs(delta.net_exposure_usd) < 500;
                return (
                  <tr key={delta.asset} className="hover:bg-white/[0.03] transition tabular-nums">
                    <td className="py-3 px-3 font-bold text-white">{delta.asset}</td>
                    <td className="py-3 px-3 text-white/80">
                      {delta.net_quantity >= 0 ? `+${delta.net_quantity.toFixed(4)}` : delta.net_quantity.toFixed(4)}
                    </td>
                    <td className="py-3 px-3">
                      <span
                        className={`font-semibold ${
                          delta.net_exposure_usd >= 0 ? "text-white/90" : "text-amber-300"
                        }`}
                      >
                        ${delta.net_exposure_usd.toFixed(2)}
                      </span>
                    </td>
                    <td className="py-3 px-3 text-white/50">${delta.delta_limit_usd.toLocaleString()} limit</td>
                    <td className="py-3 px-3 text-right">
                      {isNeutral ? (
                        <span className="chip chip-pos text-[10px]">
                          Delta Neutral
                        </span>
                      ) : (
                        <span className="chip chip-warn text-[10px]">
                          Exposure Skew
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
