import React, { useState } from "react";
import { OctagonX, Pause, Play, ShieldAlert } from "lucide-react";
import type { SystemStatusDto } from "../types";
import { Navigation, type ActiveTab } from "./Navigation";
import { ArbitrageurLogo } from "./ArbitrageurLogo";

interface HeaderProps {
  status: SystemStatusDto | null;
  isPaused?: boolean;
  onTogglePause?: (paused: boolean) => void;
  onEmergencyKillswitch: () => void;
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

  const isPaused = propIsPaused ?? status?.is_paused ?? false;

  return (
    <>
      <header className="h-12 border-b border-surface-border bg-surface flex items-stretch select-none">
        {/* Left: Institutional Branding (w-72 aligned with PositionsPanel's right border) */}
        <div className="w-72 shrink-0 border-r border-surface-border flex items-center px-4 lg:px-5">
          <div
            className="flex items-center space-x-2.5 cursor-default group"
            title={refreshing ? "ARBITRAGEUR — Syncing telemetry..." : "ARBITRAGEUR Quant Platform"}
          >
            <div className="relative flex items-center justify-center">
              <ArbitrageurLogo className="w-5 h-5 text-accent shrink-0 drop-shadow-xs" />
            </div>
            <div className="flex items-center">
              <span className="font-black tracking-tight text-sm font-sans text-slate-900">
                ARBITRAGEUR
              </span>
            </div>
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

        {/* Right: Metrics & Risk Actions — circuit-breaker reset lives only in Portfolio Monitor */}
        <div className="border-l border-surface-border flex items-center space-x-2 px-3 lg:px-4 shrink-0">
          {/* Pause / Continue Button (To the left of STOP, preserves positions) */}
          <button
            onClick={() => onTogglePause?.(!isPaused)}
            className={isPaused ? "btn-warning-subtle animate-pulse" : "btn-secondary"}
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
            className="btn-danger px-2 py-2 text-xs font-sans font-bold !gap-1 shadow-xs"
            title="Emergency Killswitch: Stop All Trading"
          >
            <OctagonX className="w-3.5 h-3.5" />
            <span>Stop All</span>
          </button>
        </div>
      </header>

      {/* Kill Switch Modal */}
      {showKillModal && (
        <div className="fixed inset-0 z-50 bg-slate-900/50 backdrop-blur-sm flex items-center justify-center p-4 animate-in fade-in duration-150">
          <div className="bg-surface border border-surface-border rounded-2xl p-6 max-w-md w-full shadow-2xl space-y-4 animate-in zoom-in-95 duration-150">
            <div className="flex items-center space-x-3">
              <div className="w-10 h-10 rounded-xl bg-loss-dim border border-loss/30 flex items-center justify-center text-loss">
                <ShieldAlert className="w-5 h-5" />
              </div>
              <div>
                <h3 className="text-base font-bold font-sans text-slate-900 leading-snug">Emergency Stop</h3>
                <p className="text-xs text-slate-500">Trip circuit breaker &amp; halt all trading</p>
              </div>
            </div>
            <p className="text-sm text-slate-600 leading-relaxed">
              This will immediately stop all trading coordination, trip the risk circuit breaker, and cancel pending orders across all venues.
            </p>
            <div className="flex justify-end space-x-2.5 pt-3 border-t border-surface-border">
              <button
                onClick={() => setShowKillModal(false)}
                className="btn-secondary px-2.5 py-2.5 text-sm"
              >
                Cancel
              </button>
              <button
                onClick={() => {
                  onEmergencyKillswitch();
                  setShowKillModal(false);
                }}
                className="btn-danger px-2.5 py-2.5 text-sm font-bold shadow-sm"
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
