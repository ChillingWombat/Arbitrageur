import React, { useState } from "react";
import type { SystemStatusDto } from "../types";

interface HeaderProps {
  status: SystemStatusDto | null;
  onResetCircuitBreaker: () => void;
  onEmergencyKillswitch: () => void;
  onToggleLiveMode: (enabled: boolean) => void;
  refreshing: boolean;
}

export const Header: React.FC<HeaderProps> = ({
  status,
  onResetCircuitBreaker,
  onEmergencyKillswitch,
  onToggleLiveMode,
  refreshing,
}) => {
  const [showKillModal, setShowKillModal] = useState(false);

  const pnl = status?.total_realized_pnl ?? 0;
  const isTripped = status?.circuit_breaker_tripped ?? false;
  const isLive = status?.is_live_mode ?? false;

  return (
    <>
      <header className="h-16 border-b border-surface-border bg-surface px-6 flex items-center justify-between select-none">
        {/* Left: Clean Minimalist Branding */}
        <div className="flex items-center space-x-3">
          <div className="flex items-center space-x-2">
            <span className="w-2 h-2 rounded-full bg-accent" />
            <span className="font-bold tracking-tight text-sm font-mono text-slate-900">ARBITRAGER</span>
            {refreshing && (
              <span className="w-1.5 h-1.5 rounded-full bg-accent animate-ping" title="Syncing" />
            )}
          </div>
        </div>

        {/* Center: Live Venue Telemetry Chips */}
        <div className="hidden xl:flex items-center space-x-2">
          {status?.active_venues.map((v) => (
            <div
              key={v.id}
              className="flex items-center space-x-1.5 px-2.5 py-1 rounded-md bg-surface-raised border border-surface-border text-xs font-mono"
            >
              <span className="w-1.5 h-1.5 rounded-full bg-profit" />
              <span className="text-slate-700 font-medium">{v.name.split(" ")[0]}</span>
              <span className="text-slate-400 text-[10px]">{v.latency_ms}ms</span>
            </div>
          ))}
        </div>

        {/* Right: Metrics & Risk Actions */}
        <div className="flex items-center space-x-3">
          {/* PnL Card */}
          <div className="px-3.5 py-1.5 rounded-lg bg-surface-raised border border-surface-border flex flex-col items-end">
            <span className="text-[10px] uppercase font-mono tracking-wider text-slate-500">Total Net PnL</span>
            <span
              className={`font-mono text-sm font-semibold ${
                pnl >= 0 ? "text-profit" : "text-loss"
              }`}
            >
              {pnl >= 0 ? `+$${pnl.toFixed(2)}` : `-$${Math.abs(pnl).toFixed(2)}`}
            </span>
          </div>

          {/* Circuit Breaker Status */}
          <div
            className={`px-3 py-1.5 rounded-lg border flex items-center space-x-1.5 text-xs font-mono font-medium ${
              isTripped
                ? "bg-loss-dim border-loss-border text-loss"
                : "bg-profit-dim border-profit-border text-profit"
            }`}
          >
            <span className={`w-1.5 h-1.5 rounded-full ${isTripped ? "bg-loss animate-pulse" : "bg-profit"}`} />
            <span>{isTripped ? "Circuit Breaker: Tripped" : "Risk: Normal"}</span>
          </div>

          {/* Reset CB if tripped */}
          {isTripped && (
            <button
              onClick={onResetCircuitBreaker}
              className="px-2.5 py-1.5 bg-warn-dim hover:bg-amber-100 text-warn border border-warn-border rounded-lg text-xs font-mono font-medium transition"
              title="Reset Circuit Breaker"
            >
              Reset
            </button>
          )}

          {/* Mode Selector */}
          <div className="flex items-center bg-surface-raised border border-surface-border rounded-lg p-0.5 text-xs font-mono">
            <button
              onClick={() => onToggleLiveMode(false)}
              className={`px-2.5 py-1 rounded transition ${
                !isLive ? "bg-surface text-accent font-semibold shadow-xs" : "text-slate-500 hover:text-slate-900"
              }`}
            >
              Simulated
            </button>
            <button
              onClick={() => onToggleLiveMode(true)}
              className={`px-2.5 py-1 rounded transition ${
                isLive ? "bg-surface text-profit font-semibold shadow-xs" : "text-slate-500 hover:text-slate-900"
              }`}
            >
              Live
            </button>
          </div>

          {/* Emergency Kill Switch */}
          <button
            onClick={() => setShowKillModal(true)}
            className="px-3 py-1.5 bg-loss hover:bg-red-700 text-white rounded-lg text-xs font-mono font-medium transition shadow-xs"
            title="Emergency Killswitch: Halt All Trading"
          >
            Halt All
          </button>
        </div>
      </header>

      {/* Kill Switch Modal */}
      {showKillModal && (
        <div className="fixed inset-0 z-50 bg-slate-900/40 backdrop-blur-xs flex items-center justify-center p-4">
          <div className="bg-surface border border-surface-border rounded-xl p-6 max-w-md w-full shadow-xl space-y-4">
            <div>
              <h3 className="text-base font-bold font-mono text-slate-900">Emergency Halt</h3>
            </div>
            <p className="text-sm text-slate-600">
              This will immediately halt all trading coordination, trip the risk circuit breaker, and cancel pending orders across all venues.
            </p>
            <div className="flex justify-end space-x-3 pt-2">
              <button
                onClick={() => setShowKillModal(false)}
                className="px-4 py-2 bg-surface-raised hover:bg-slate-200 text-slate-700 rounded-lg text-sm font-mono transition"
              >
                Cancel
              </button>
              <button
                onClick={() => {
                  onEmergencyKillswitch();
                  setShowKillModal(false);
                }}
                className="px-4 py-2 bg-loss hover:bg-red-700 text-white font-medium rounded-lg text-sm font-mono transition shadow-xs"
              >
                Confirm Halt
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
};
