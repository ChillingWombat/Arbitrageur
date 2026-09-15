import React, { useState } from "react";
import { Pause, Play } from "lucide-react";
import type { SystemStatusDto } from "../types";
import { Navigation, type ActiveTab } from "./Navigation";
import { ArbitragerLogo } from "./ArbitragerLogo";

interface HeaderProps {
  status: SystemStatusDto | null;
  isPaused?: boolean;
  onTogglePause?: (paused: boolean) => void;
  onResetCircuitBreaker: () => void;
  onEmergencyKillswitch: () => void;
  onToggleLiveMode?: (enabled: boolean) => void;
  onOpenSettings?: () => void;
  refreshing: boolean;
  activeTab: ActiveTab;
  onSelectTab: (tab: ActiveTab) => void;
  spatialCount?: number;
  fundingCount?: number;
  spotCount?: number;
  triangleCount?: number;
  predictionCount?: number;
}

export const Header: React.FC<HeaderProps> = ({
  status,
  isPaused: propIsPaused,
  onTogglePause,
  onResetCircuitBreaker,
  onEmergencyKillswitch,
  refreshing,
  activeTab,
  onSelectTab,
  spatialCount = 0,
  fundingCount = 0,
  spotCount = 0,
  triangleCount = 0,
  predictionCount = 0,
}) => {
  const [showKillModal, setShowKillModal] = useState(false);

  const isTripped = status?.circuit_breaker_tripped ?? false;
  const isPaused = propIsPaused ?? status?.is_paused ?? false;

  return (
    <>
      <header className="h-12 border-b border-surface-border bg-surface flex items-stretch select-none">
        {/* Left: Minimalist Branding (w-72 aligned with TickerExplorer's right border) */}
        <div className="w-72 shrink-0 border-r border-surface-border flex items-center px-4 lg:px-5">
          <div
            className="flex items-center space-x-2.5 cursor-default"
            title={refreshing ? "ARBITRAGER — Syncing telemetry..." : "ARBITRAGER"}
          >
            <ArbitragerLogo className="w-5 h-5 text-accent shrink-0" />
            <span className="font-bold tracking-tight text-sm font-mono text-slate-900">
              ARBITRAGER
            </span>
          </div>
        </div>

        {/* Center: Top Section Switcher Navigation Bar (flush with dividers) */}
        <div className="flex-1 flex items-stretch min-w-0">
          <Navigation
            activeTab={activeTab}
            onSelectTab={onSelectTab}
            spatialCount={spatialCount}
            fundingCount={fundingCount}
            spotCount={spotCount}
            triangleCount={triangleCount}
            predictionCount={predictionCount}
          />
        </div>

        {/* Right: Metrics & Risk Actions */}
        <div className="border-l border-surface-border flex items-center space-x-2.5 px-4 shrink-0">
          {/* Reset CB if tripped */}
          {isTripped && (
            <button
              onClick={onResetCircuitBreaker}
              className="px-2 py-1 bg-warn-dim hover:bg-amber-100 text-warn border border-warn-border rounded-md text-xs font-mono font-medium transition cursor-pointer"
              title="Reset Circuit Breaker"
            >
              Reset CB
            </button>
          )}

          {/* Pause / Continue Button (To the left of STOP, preserves positions) */}
          <button
            onClick={() => onTogglePause?.(!isPaused)}
            className={`px-2 py-1 rounded-md text-xs font-mono font-medium transition flex items-center space-x-1.5 cursor-pointer shadow-2xs ${
              isPaused
                ? "bg-amber-500 hover:bg-amber-600 text-white animate-pulse"
                : "bg-surface-raised hover:bg-slate-200 text-slate-700 border border-surface-border"
            }`}
            title={
              isPaused
                ? "Continue Trading: Resume automated and manual executions (Positions Preserved)"
                : "Pause Trading: Temporarily suspend executions (Positions are NOT cleared)"
            }
          >
            {isPaused ? (
              <>
                <Play className="w-3.5 h-3.5 fill-current" />
                <span>Continue All</span>
              </>
            ) : (
              <>
                <Pause className="w-3.5 h-3.5" />
                <span>Pause All</span>
              </>
            )}
          </button>

          {/* Emergency Kill Switch */}
          <button
            onClick={() => setShowKillModal(true)}
            className="px-2 py-1 bg-loss hover:bg-red-700 text-white rounded-md text-xs font-mono font-medium transition shadow-2xs cursor-pointer"
            title="Emergency Killswitch: Stop All Trading"
          >
            Stop All
          </button>
        </div>
      </header>

      {/* Kill Switch Modal */}
      {showKillModal && (
        <div className="fixed inset-0 z-50 bg-slate-900/40 backdrop-blur-xs flex items-center justify-center p-4">
          <div className="bg-surface border border-surface-border rounded-lg p-6 max-w-md w-full shadow-xl space-y-4">
            <div>
              <h3 className="text-base font-bold font-mono tracking-wider text-slate-900">Emergency Stop</h3>
            </div>
            <p className="text-sm text-slate-600">
              This will immediately stop all trading coordination, trip the risk circuit breaker, and cancel pending orders across all venues.
            </p>
            <div className="flex justify-end space-x-3 pt-2">
              <button
                onClick={() => setShowKillModal(false)}
                className="px-4 py-2 bg-surface-raised hover:bg-slate-200 text-slate-700 rounded-md text-sm font-mono transition"
              >
                Cancel
              </button>
              <button
                onClick={() => {
                  onEmergencyKillswitch();
                  setShowKillModal(false);
                }}
                className="px-4 py-2 bg-loss hover:bg-red-700 text-white font-medium rounded-md text-sm font-mono transition shadow-2xs"
              >
                Confirm Stop
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
};
