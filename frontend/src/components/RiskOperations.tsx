import React from "react";
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
      <div className="bg-surface p-5 rounded-lg border border-surface-border flex flex-wrap items-center justify-between gap-4 shadow-xs">
        <div>
          <h2 className="text-sm font-bold font-mono tracking-wider text-slate-900">
            DeltaGuard & Enterprise Risk Engine Controls
          </h2>
          <p className="text-xs text-slate-500 mt-1">
            Real-time signed cross-exchange net delta tracking, drawdown tripwires, and maintenance margin proximity.
          </p>
        </div>

        <div className="flex items-center space-x-3">
          {isTripped ? (
            <button
              onClick={onResetCircuitBreaker}
              className="px-3 py-1.5 bg-warn-dim hover:bg-amber-100 text-warn border border-warn-border rounded-md text-xs font-mono font-medium flex items-center transition shadow-xs"
            >
              <span>Reset Circuit Breaker</span>
            </button>
          ) : (
            <button
              onClick={onTriggerKillswitch}
              className="px-3 py-1.5 bg-loss-dim hover:bg-red-100 text-loss border border-loss-border rounded-md text-xs font-mono font-medium flex items-center transition shadow-xs"
            >
              <span>Trigger Manual Trip</span>
            </button>
          )}
        </div>
      </div>

      {/* 3 Metric Cards */}
      <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
        {/* Card 1: Circuit Breaker */}
        <div className="bg-surface border border-surface-border rounded-lg p-5 space-y-3 shadow-xs">
          <div className="flex items-center justify-between">
            <span className="text-xs font-mono text-slate-500">Circuit Breaker</span>
            <span
              className={`px-2 py-0.5 rounded-sm text-xs font-mono font-semibold ${
                isTripped
                  ? "bg-loss-dim text-loss border border-loss-border"
                  : "bg-profit-dim text-profit border border-profit-border"
              }`}
            >
              {riskData.circuit_breaker_state}
            </span>
          </div>

          <div className="space-y-1">
            <div className="flex justify-between text-xs font-mono">
              <span className="text-slate-500">Max Drawdown:</span>
              <span className="text-slate-900 font-bold">${riskData.max_drawdown_limit_usd.toLocaleString()}</span>
            </div>
            <div className="flex justify-between text-xs font-mono">
              <span className="text-slate-500">Consecutive Errors:</span>
              <span className="text-slate-900 font-bold">{riskData.consecutive_failures} / {riskData.max_failures_allowed}</span>
            </div>
          </div>

          <div className="pt-2">
            <div className="h-1.5 w-full bg-slate-100 rounded-full overflow-hidden">
              <div
                className="h-full bg-profit transition-all duration-300"
                style={{ width: `${Math.max(5, (riskData.consecutive_failures / riskData.max_failures_allowed) * 100)}%` }}
              />
            </div>
          </div>
        </div>

        {/* Card 2: Margin Monitor */}
        <div className="bg-surface border border-surface-border rounded-lg p-5 space-y-3 shadow-xs">
          <div className="flex items-center justify-between">
            <span className="text-xs font-mono text-slate-500">Margin Proximity</span>
            <span className="px-2 py-0.5 rounded-sm text-xs font-mono font-semibold bg-profit-dim text-profit border border-profit-border">
              {margin.status}
            </span>
          </div>

          <div className="space-y-1">
            <div className="flex justify-between text-xs font-mono">
              <span className="text-slate-500">Margin Ratio:</span>
              <span className="text-slate-900 font-bold">{margin.margin_ratio_pct.toFixed(2)}%</span>
            </div>
            <div className="flex justify-between text-xs font-mono">
              <span className="text-slate-500">Liquidation Distance:</span>
              <span className="text-profit font-bold">+{margin.liquidation_distance_pct.toFixed(1)}% safe</span>
            </div>
          </div>

          <div className="pt-2">
            <div className="h-1.5 w-full bg-slate-100 rounded-full overflow-hidden">
              <div
                className="h-full bg-accent"
                style={{ width: `${margin.liquidation_distance_pct}%` }}
              />
            </div>
          </div>
        </div>

        {/* Card 3: Portfolio Capital */}
        <div className="bg-surface border border-surface-border rounded-lg p-5 space-y-3 shadow-xs">
          <div className="flex items-center justify-between">
            <span className="text-xs font-mono text-slate-500">Portfolio Capital</span>
            <span className="text-xs font-mono text-accent font-semibold">Consolidated</span>
          </div>

          <div className="space-y-1">
            <div className="flex justify-between text-xs font-mono">
              <span className="text-slate-500">Total Equity:</span>
              <span className="text-slate-900 font-bold">${riskData.total_portfolio_value_usd.toLocaleString()}</span>
            </div>
            <div className="flex justify-between text-xs font-mono">
              <span className="text-slate-500">Maint. Req:</span>
              <span className="text-slate-700 font-medium">${margin.maintenance_margin_req_usd.toLocaleString()}</span>
            </div>
          </div>

          <div className="pt-2">
            <div className="h-1.5 w-full bg-slate-100 rounded-full overflow-hidden">
              <div
                className="h-full bg-profit"
                style={{ width: "95%" }}
              />
            </div>
          </div>
        </div>
      </div>

      {/* DeltaGuard Net Exposure Table */}
      <div className="bg-surface rounded-lg border border-surface-border p-5 shadow-xs">
        <h3 className="text-xs font-bold font-mono tracking-wider text-slate-900 mb-4 flex items-center justify-between">
          <span>DeltaGuard Cross-Exchange Signed Net Exposure</span>
          <span className="text-slate-400 font-normal lowercase">enforcing strict delta-neutrality across positions</span>
        </h3>

        <div className="overflow-x-auto">
          <table className="w-full text-left text-xs font-mono">
            <thead>
              <tr className="border-b border-surface-border text-slate-500 text-xs tracking-wider">
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
                    <td className="py-3 px-3 font-bold text-slate-900">{delta.asset}</td>
                    <td className="py-3 px-3 text-slate-700">
                      {delta.net_quantity >= 0 ? `+${delta.net_quantity.toFixed(4)}` : delta.net_quantity.toFixed(4)}
                    </td>
                    <td className="py-3 px-3">
                      <span
                        className={`font-semibold ${
                          delta.net_exposure_usd >= 0 ? "text-slate-800" : "text-warn"
                        }`}
                      >
                        ${delta.net_exposure_usd.toFixed(2)}
                      </span>
                    </td>
                    <td className="py-3 px-3 text-slate-500">${delta.delta_limit_usd.toLocaleString()} limit</td>
                    <td className="py-3 px-3 text-right">
                      {isNeutral ? (
                        <span className="px-2 py-0.5 rounded-sm text-xs font-semibold bg-profit-dim text-profit border border-profit-border">
                          Delta Neutral
                        </span>
                      ) : (
                        <span className="px-2 py-0.5 rounded-sm text-xs font-semibold bg-warn-dim text-warn border border-warn-border">
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
