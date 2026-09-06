import React, { useState } from "react";
import { Zap, ShieldAlert, AlertOctagon, RotateCcw, Activity } from "lucide-react";
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
        {/* Left: Branding & Tag */}
        <div className="flex items-center space-x-3">
          <div className="w-10 h-10 rounded-lg bg-gradient-to-br from-accent/20 to-profit/20 border border-accent/40 flex items-center justify-center shadow-lg shadow-accent/10">
            <Zap className="w-5 h-5 text-accent" />
          </div>
          <div>
            <div className="flex items-center space-x-2">
              <span className="font-bold tracking-wider text-base font-mono text-white">ARBITRAGER</span>
              <span className="text-[10px] uppercase font-semibold px-2 py-0.5 rounded bg-accent/10 text-accent border border-accent/30 tracking-widest">
                QUANT DESK
              </span>
              {refreshing && (
                <Activity className="w-3.5 h-3.5 text-accent animate-spin" />
              )}
            </div>
            <p className="text-xs text-slate-400 font-mono tracking-tight">
              Hyperliquid • Lighter • Vanta • Robinhood • Polymarket • Kalshi
            </p>
          </div>
        </div>

        {/* Center: Live Venue Telemetry Chips */}
        <div className="hidden xl:flex items-center space-x-2">
          {status?.active_venues.map((v) => (
            <div
              key={v.id}
              className="flex items-center space-x-1.5 px-2.5 py-1 rounded-md bg-surface-raised border border-surface-border text-xs font-mono"
            >
              <span className="w-2 h-2 rounded-full bg-profit animate-pulse" />
              <span className="text-slate-300 font-medium">{v.name.split(" ")[0]}</span>
              <span className="text-slate-500 text-[10px]">{v.latency_ms}ms</span>
            </div>
          ))}
        </div>

        {/* Right: Metrics & Risk Actions */}
        <div className="flex items-center space-x-4">
          {/* PnL Card */}
          <div className="px-3.5 py-1.5 rounded-lg bg-surface-raised border border-surface-border flex flex-col items-end">
            <span className="text-[10px] uppercase font-mono tracking-wider text-slate-400">Total Net PnL</span>
            <span
              className={`font-mono text-sm font-bold ${
                pnl >= 0 ? "text-profit" : "text-loss"
              }`}
            >
              {pnl >= 0 ? `+$${pnl.toFixed(2)}` : `-$${Math.abs(pnl).toFixed(2)}`}
            </span>
          </div>

          {/* Circuit Breaker Status */}
          <div
            className={`px-3 py-1.5 rounded-lg border flex items-center space-x-2 text-xs font-mono ${
              isTripped
                ? "bg-loss/10 border-loss text-loss animate-pulse"
                : "bg-profit/10 border-profit/30 text-profit"
            }`}
          >
            <ShieldAlert className="w-4 h-4" />
            <span className="font-semibold">{isTripped ? "CB: TRIPPED" : "CB: ACTIVE"}</span>
          </div>

          {/* Reset CB if tripped */}
          {isTripped && (
            <button
              onClick={onResetCircuitBreaker}
              className="px-2.5 py-1.5 bg-amber-500/10 hover:bg-amber-500/20 text-amber-400 border border-amber-500/40 rounded-lg text-xs font-mono flex items-center space-x-1 transition"
              title="Reset Circuit Breaker"
            >
              <RotateCcw className="w-3.5 h-3.5" />
              <span>Reset</span>
            </button>
          )}

          {/* Mode Selector */}
          <div className="flex items-center bg-surface-raised border border-surface-border rounded-lg p-1 text-xs font-mono">
            <button
              onClick={() => onToggleLiveMode(false)}
              className={`px-2.5 py-1 rounded transition ${
                !isLive ? "bg-accent/20 text-accent font-semibold" : "text-slate-400 hover:text-white"
              }`}
            >
              Simulated
            </button>
            <button
              onClick={() => onToggleLiveMode(true)}
              className={`px-2.5 py-1 rounded transition ${
                isLive ? "bg-profit/20 text-profit font-semibold" : "text-slate-400 hover:text-white"
              }`}
            >
              Live
            </button>
          </div>

          {/* Emergency Kill Switch */}
          <button
            onClick={() => setShowKillModal(true)}
            className="px-3 py-1.5 bg-loss/20 hover:bg-loss/30 text-loss border border-loss/50 rounded-lg text-xs font-mono font-bold flex items-center space-x-1.5 transition glow-rose shadow-md"
            title="Emergency Killswitch: Halt All Trading"
          >
            <AlertOctagon className="w-4 h-4" />
            <span>KILL SWITCH</span>
          </button>
        </div>
      </header>

      {/* Kill Switch Modal */}
      {showKillModal && (
        <div className="fixed inset-0 z-50 bg-black/80 backdrop-blur-sm flex items-center justify-center p-4">
          <div className="bg-surface border-2 border-loss rounded-xl p-6 max-w-md w-full shadow-2xl space-y-4">
            <div className="flex items-center space-x-3 text-loss">
              <AlertOctagon className="w-8 h-8" />
              <h3 className="text-lg font-bold font-mono">EMERGENCY KILL SWITCH</h3>
            </div>
            <p className="text-sm text-slate-300">
              This will immediately halt all trading coordination, trip the risk circuit breaker, and cancel any pending orders across all venues.
            </p>
            <div className="flex justify-end space-x-3 pt-2">
              <button
                onClick={() => setShowKillModal(false)}
                className="px-4 py-2 bg-surface-raised hover:bg-surface-border text-slate-300 rounded-lg text-sm font-mono transition"
              >
                Cancel
              </button>
              <button
                onClick={() => {
                  onEmergencyKillswitch();
                  setShowKillModal(false);
                }}
                className="px-4 py-2 bg-loss hover:bg-rose-600 text-white font-bold rounded-lg text-sm font-mono transition"
              >
                CONFIRM HALT
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
};
