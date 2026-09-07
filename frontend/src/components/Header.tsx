import React, { useState } from "react";
import type { SystemStatusDto } from "../types";
import { Navigation, type ActiveTab } from "./Navigation";

interface HeaderProps {
  status: SystemStatusDto | null;
  onResetCircuitBreaker: () => void;
  onEmergencyKillswitch: () => void;
  onToggleLiveMode: (enabled: boolean) => void;
  refreshing: boolean;
  activeTab: ActiveTab;
  onSelectTab: (tab: ActiveTab) => void;
  spatialCount?: number;
  fundingCount?: number;
  predictionCount?: number;
}

export const Header: React.FC<HeaderProps> = ({
  status,
  onResetCircuitBreaker,
  onEmergencyKillswitch,
  onToggleLiveMode,
  refreshing,
  activeTab,
  onSelectTab,
  spatialCount = 0,
  fundingCount = 0,
  predictionCount = 0,
}) => {
  const [showKillModal, setShowKillModal] = useState(false);

  const pnl = status?.total_realized_pnl ?? 0;
  const isTripped = status?.circuit_breaker_tripped ?? false;
  const isLive = status?.is_live_mode ?? false;

  return (
    <>
      <header className="h-16 border-b border-surface-border bg-surface px-4 lg:px-6 flex items-center justify-between select-none gap-3">
        {/* Left: Minimalist Branding */}
        <div className="flex items-center space-x-3 shrink-0">
          <div className="flex items-center space-x-2">
            <span className="w-2.5 h-2.5 rounded-full bg-accent" />
            <span className="font-bold tracking-tight text-sm font-mono text-slate-900">
              ARBITRAGER
            </span>
            {refreshing && (
              <span className="w-1.5 h-1.5 rounded-full bg-accent animate-ping" title="Syncing telemetry" />
            )}
          </div>
        </div>

        {/* Center: Top Section Switcher Navigation Bar */}
        <div className="flex-1 flex justify-center min-w-0 px-2 overflow-x-auto">
          <Navigation
            activeTab={activeTab}
            onSelectTab={onSelectTab}
            spatialCount={spatialCount}
            fundingCount={fundingCount}
            predictionCount={predictionCount}
          />
        </div>

        {/* Right: Metrics & Risk Actions */}
        <div className="flex items-center space-x-2.5 shrink-0">
          {/* Active Venues Micro-Chips */}
          <div className="hidden 2xl:flex items-center space-x-1.5">
            {status?.active_venues.slice(0, 4).map((v) => (
              <div
                key={v.id}
                className="flex items-center space-x-1 px-2 py-1 rounded bg-surface-raised border border-surface-border text-[10px] font-mono text-slate-600"
                title={`${v.name} (${v.latency_ms}ms)`}
              >
                <span className="w-1.5 h-1.5 rounded-full bg-profit" />
                <span className="font-semibold">{v.name.split(" ")[0]}</span>
              </div>
            ))}
          </div>

          {/* PnL Card */}
          <div className="px-3 py-1 rounded-lg bg-surface-raised border border-surface-border flex flex-col items-end">
            <span className="text-[9px] uppercase font-mono tracking-wider text-slate-600 font-semibold">Net PnL</span>
            <span
              className={`font-mono text-xs font-bold ${
                pnl >= 0 ? "text-profit" : "text-loss"
              }`}
            >
              {pnl >= 0 ? `+$${pnl.toFixed(2)}` : `-$${Math.abs(pnl).toFixed(2)}`}
            </span>
          </div>

          {/* Circuit Breaker Status */}
          <div
            className={`px-2.5 py-1 rounded-lg border flex items-center space-x-1.5 text-xs font-mono font-medium ${
              isTripped
                ? "bg-loss-dim border-loss-border text-loss"
                : "bg-profit-dim border-profit-border text-profit"
            }`}
          >
            <span className={`w-1.5 h-1.5 rounded-full ${isTripped ? "bg-loss animate-pulse" : "bg-profit"}`} />
            <span className="hidden sm:inline">{isTripped ? "CB: Tripped" : "Risk: Normal"}</span>
          </div>

          {/* Reset CB if tripped */}
          {isTripped && (
            <button
              onClick={onResetCircuitBreaker}
              className="px-2 py-1 bg-warn-dim hover:bg-amber-100 text-warn border border-warn-border rounded-lg text-xs font-mono font-medium transition"
              title="Reset Circuit Breaker"
            >
              Reset
            </button>
          )}

          {/* Mode Selector */}
          <div className="flex items-center bg-surface-raised border border-surface-border rounded-lg p-0.5 text-xs font-mono">
            <button
              onClick={() => onToggleLiveMode(false)}
              className={`px-2 py-0.5 rounded transition ${
                !isLive ? "bg-surface text-accent font-semibold shadow-xs" : "text-slate-600 hover:text-slate-900"
              }`}
            >
              Sim
            </button>
            <button
              onClick={() => onToggleLiveMode(true)}
              className={`px-2 py-0.5 rounded transition ${
                isLive ? "bg-surface text-profit font-semibold shadow-xs" : "text-slate-600 hover:text-slate-900"
              }`}
            >
              Live
            </button>
          </div>

          {/* Emergency Kill Switch */}
          <button
            onClick={() => setShowKillModal(true)}
            className="px-2.5 py-1 bg-loss hover:bg-red-700 text-white rounded-lg text-xs font-mono font-medium transition shadow-xs"
            title="Emergency Killswitch: Halt All Trading"
          >
            Halt
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
