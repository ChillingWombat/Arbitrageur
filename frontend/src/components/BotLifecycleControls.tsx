import React from "react";
import { Play, Pause, TrendingDown, OctagonX } from "lucide-react";
import type { BotPhase } from "../hooks/useBotLifecycle";

interface BotLifecycleControlsProps {
  phase: BotPhase;
  hasOpenPositions: boolean;
  disabled?: boolean;
  onStart: () => void;
  onPause: () => void;
  onReduceOnly: () => void;
  onStop: () => void;
}

interface BtnDef {
  key: string;
  label: string;
  title: string;
  icon: React.ReactNode;
  enabled: boolean;
  onClick: () => void;
  activeClass: string;
}

/**
 * Start / Pause / Reduce Only / Stop control group for strategy bots.
 * Stop is the emergency exit: flattens this strategy's positions for the
 * scope until delta-neutral. Buttons grey out when not applicable.
 */
export const BotLifecycleControls: React.FC<BotLifecycleControlsProps> = ({
  phase,
  hasOpenPositions,
  disabled = false,
  onStart,
  onPause,
  onReduceOnly,
  onStop,
}) => {
  const stopping = phase === "stopping";
  const idle = phase === "idle";

  const buttons: BtnDef[] = [
    {
      key: "start",
      label: "Start",
      title: phase === "paused" ? "Resume entries" : "Start the bot",
      icon: <Play className="w-3 h-3" />,
      enabled: !disabled && !stopping && (idle || phase === "paused"),
      onClick: onStart,
      activeClass: "text-emerald-700 hover:bg-emerald-50 border-emerald-300",
    },
    {
      key: "pause",
      label: "Pause",
      title: "Suspend new executions; positions preserved",
      icon: <Pause className="w-3 h-3" />,
      // Suspending is safe — stays available even when entries are disabled
      enabled: phase === "running",
      onClick: onPause,
      activeClass: "text-slate-700 hover:bg-slate-100 border-slate-300",
    },
    {
      key: "reduce",
      label: "Reduce Only",
      title: "Exit-only mode — stops automatically once this strategy is flat",
      icon: <TrendingDown className="w-3 h-3" />,
      // Risk-reducing: never gated by connectivity — must drain positions
      // even if the feed dropped mid-session
      enabled:
        (phase === "running" || phase === "paused") &&
        hasOpenPositions,
      onClick: onReduceOnly,
      activeClass: "text-amber-700 hover:bg-amber-50 border-amber-300",
    },
    {
      key: "stop",
      label: stopping ? "Stopping…" : "Stop",
      title:
        "Emergency exit — immediately flatten this strategy's positions to delta-neutral",
      icon: <OctagonX className="w-3 h-3" />,
      // Emergency exit must never be greyed out while a phase is active
      enabled: !idle && !stopping,
      onClick: onStop,
      activeClass: "text-red-700 hover:bg-red-50 border-red-300",
    },
  ];

  const phaseLabel =
    phase === "reduce_only"
      ? "REDUCE-ONLY"
      : phase === "stopping"
      ? "STOPPING"
      : null;

  return (
    <div className="flex items-center space-x-1.5 px-2 py-1 bg-surface border border-surface-border rounded-md shadow-2xs">
      <div className="flex items-center gap-1">
        {buttons.map((b) => (
          <button
            key={b.key}
            type="button"
            title={b.title}
            disabled={!b.enabled}
            onClick={b.onClick}
            className={`flex items-center space-x-1 px-2 py-1 rounded border text-[10px] font-mono font-bold uppercase tracking-wide transition cursor-pointer disabled:opacity-35 disabled:cursor-not-allowed ${
              b.enabled ? b.activeClass : "text-slate-400 border-surface-border bg-surface-raised/40"
            }`}
          >
            {b.icon}
            <span>{b.label}</span>
          </button>
        ))}
      </div>
      {phaseLabel && (
        <span className="text-[9px] font-mono font-bold px-1.5 py-0.5 rounded-sm bg-amber-50 text-amber-700 border border-amber-200 uppercase">
          {phaseLabel}
        </span>
      )}
    </div>
  );
};
