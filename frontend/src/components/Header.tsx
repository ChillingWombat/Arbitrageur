import React, { useState } from "react";
import type { SystemStatusDto } from "../types";
import { Navigation, type ActiveTab } from "./Navigation";

interface HeaderProps {
  status: SystemStatusDto | null;
  onResetCircuitBreaker: () => void;
  onEmergencyKillswitch: () => void;
  onToggleLiveMode?: (enabled: boolean) => void;
  refreshing: boolean;
  activeTab: ActiveTab;
  onSelectTab: (tab: ActiveTab) => void;
  spatialCount?: number;
  spotCount?: number;
  fundingCount?: number;
  predictionCount?: number;
}

export const Header: React.FC<HeaderProps> = ({
  status,
  onResetCircuitBreaker,
  onEmergencyKillswitch,
  refreshing,
  activeTab,
  onSelectTab,
  spatialCount = 0,
  spotCount = 0,
  fundingCount = 0,
  predictionCount = 0,
}) => {
  const [showKillModal, setShowKillModal] = useState(false);

  const pnl = status?.total_realized_pnl ?? 0;
  const isTripped = status?.circuit_breaker_tripped ?? false;
  const isLive = status?.is_live_mode ?? false;

  return (
    <>
      <header className="h-16 border-b border-white/[0.08] bg-[#080B14]/80 backdrop-blur-xl px-4 lg:px-6 flex items-center justify-between select-none gap-3 sticky top-0 z-40">
        {/* Left: Holographic / Iridescent Branding */}
        <div className="flex items-center space-x-3 shrink-0">
          <div className="flex items-center space-x-2.5">
            <span className="w-2.5 h-2.5 rounded-full bg-accent-cyan shadow-[0_0_10px_#22D3EE]" />
            <span className="font-bold tracking-tight text-sm font-mono text-white/95 bg-gradient-to-r from-accent-cyan via-accent-indigo to-accent-magenta bg-clip-text text-transparent">
              ARBITRAGER
            </span>
            {refreshing && (
              <span className="w-1.5 h-1.5 rounded-full bg-accent-cyan animate-ping" title="Syncing telemetry" />
            )}
          </div>
        </div>

        {/* Center: Top Section Switcher Navigation Bar */}
        <div className="flex-1 flex justify-center min-w-0 px-2 overflow-x-auto">
          <Navigation
            activeTab={activeTab}
            onSelectTab={onSelectTab}
            spatialCount={spatialCount}
            spotCount={spotCount}
            fundingCount={fundingCount}
            predictionCount={predictionCount}
          />
        </div>

        {/* Right: Metrics & Risk Actions */}
        <div className="flex items-center space-x-2.5 shrink-0">
          {/* PnL Card */}
          <div className="px-3 py-1 rounded-xl bg-white/[0.04] border border-white/[0.08] backdrop-blur-md flex flex-col items-end">
            <span className="text-[9px] uppercase font-mono tracking-wider text-white/50 font-semibold">Net PnL</span>
            <span
              className={`font-mono text-xs font-bold tabular-numbers ${
                pnl >= 0 ? "text-pos text-shadow-sm" : "text-neg"
              }`}
            >
              {pnl >= 0 ? `+$${pnl.toFixed(2)}` : `-$${Math.abs(pnl).toFixed(2)}`}
            </span>
          </div>

          {/* Circuit Breaker Status */}
          <div
            className={`px-2.5 py-1 rounded-xl border flex items-center space-x-1.5 text-xs font-mono font-medium backdrop-blur-md ${
              isTripped
                ? "bg-neg/10 border-neg/30 text-neg"
                : "bg-pos/10 border-pos/30 text-pos"
            }`}
          >
            <span className={`w-1.5 h-1.5 rounded-full ${isTripped ? "bg-neg animate-pulse shadow-[0_0_8px_#FB7185]" : "bg-pos shadow-[0_0_8px_#34D399]"}`} />
            <span className="hidden sm:inline">{isTripped ? "CB: Tripped" : "Risk: Normal"}</span>
          </div>

          {/* Reset CB if tripped */}
          {isTripped && (
            <button
              onClick={onResetCircuitBreaker}
              className="px-2 py-1 bg-warn/15 hover:bg-warn/25 text-warn border border-warn/30 rounded-xl text-xs font-mono font-medium transition"
              title="Reset Circuit Breaker"
            >
              Reset
            </button>
          )}

          {/* Trading Mode Status Badge */}
          <div
            className={`px-2.5 py-1 rounded-xl border flex items-center space-x-1.5 text-xs font-mono font-medium backdrop-blur-md ${
              isLive
                ? "bg-amber-500/10 border-amber-500/30 text-amber-300"
                : "bg-accent-indigo/10 border-accent-indigo/30 text-accent-indigo"
            }`}
            title={isLive ? "Real Trading (Live Execution Active)" : "Paper Trading (Live Feeds & Simulated Orderbooks)"}
          >
            <span className={`w-1.5 h-1.5 rounded-full ${isLive ? "bg-amber-400 animate-pulse shadow-[0_0_8px_#FBBF24]" : "bg-accent-cyan shadow-[0_0_8px_#22D3EE]"}`} />
            <span>{isLive ? "Real Trading" : "Paper Trading"}</span>
          </div>

          {/* Emergency Kill Switch */}
          <button
            onClick={() => setShowKillModal(true)}
            className="px-3 py-1 bg-neg/20 hover:bg-neg/30 text-neg border border-neg/40 hover:border-neg/60 rounded-xl text-xs font-mono font-semibold transition shadow-glow-loss"
            title="Emergency Killswitch: Halt All Trading"
          >
            Halt
          </button>
        </div>
      </header>

      {/* Kill Switch Modal */}
      {showKillModal && (
        <div className="fixed inset-0 z-50 bg-black/70 backdrop-blur-md flex items-center justify-center p-4">
          <div className="bg-[#0C1019] border border-white/[0.14] rounded-2xl p-6 max-w-md w-full shadow-2xl space-y-4">
            <div>
              <h3 className="text-base font-bold font-mono text-white/95">Emergency Halt</h3>
            </div>
            <p className="text-sm text-white/70">
              This will immediately halt all trading coordination, trip the risk circuit breaker, and cancel pending orders across all venues.
            </p>
            <div className="flex justify-end space-x-3 pt-2">
              <button
                onClick={() => setShowKillModal(false)}
                className="px-4 py-2 bg-white/[0.06] hover:bg-white/[0.12] text-white/80 rounded-xl text-sm font-mono transition"
              >
                Cancel
              </button>
              <button
                onClick={() => {
                  onEmergencyKillswitch();
                  setShowKillModal(false);
                }}
                className="px-4 py-2 bg-neg hover:bg-rose-600 text-white font-medium rounded-xl text-sm font-mono transition shadow-lg shadow-neg/30"
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
