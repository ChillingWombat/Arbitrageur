import React, { useState } from "react";
import type { SystemStatusDto } from "../types";
import { Navigation, type ActiveTab } from "./Navigation";

interface HeaderProps {
  status: SystemStatusDto | null;
  onResetCircuitBreaker: () => void;
  onEmergencyKillswitch: () => void;
  onToggleLiveMode?: (enabled: boolean) => void;
  onOpenSettings?: () => void;
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

  const isTripped = status?.circuit_breaker_tripped ?? false;

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
            spotCount={spotCount}
            fundingCount={fundingCount}
            predictionCount={predictionCount}
          />
        </div>

        {/* Right: Metrics & Risk Actions */}
        <div className="flex items-center space-x-2.5 shrink-0">
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
              className="px-2 py-1 bg-warn-dim hover:bg-amber-100 text-warn border border-warn-border rounded-lg text-xs font-mono font-medium transition cursor-pointer"
              title="Reset Circuit Breaker"
            >
              Reset
            </button>
          )}

          {/* Emergency Kill Switch */}
          <button
            onClick={() => setShowKillModal(true)}
            className="px-2.5 py-1 bg-loss hover:bg-red-700 text-white rounded-lg text-xs font-mono font-medium transition shadow-xs cursor-pointer"
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
