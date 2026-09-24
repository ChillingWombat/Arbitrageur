import React, { useState } from "react";
import { Play, Pause, ArrowDown, OctagonX } from "lucide-react";
import { getBotControlAvailability } from "../hooks/useBotLifecycle";
import type { BotPhase } from "../hooks/useBotLifecycle";
import { ConfirmActionModal } from "./ConfirmActionModal";

interface BotLifecycleControlsProps {
  phase: BotPhase;
  hasOpenPositions: boolean;
  disabled?: boolean;
  reduceOnlyAvailable?: boolean;
  onStart: () => void;
  onPause: () => void;
  onReduceOnly?: () => void;
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
  reduceOnlyAvailable = true,
  onStart,
  onPause,
  onReduceOnly,
  onStop,
}) => {
  const [showStopConfirm, setShowStopConfirm] = useState(false);
  const stopping = phase === "stopping";
  const availability = getBotControlAvailability(
    phase,
    hasOpenPositions,
    disabled,
    reduceOnlyAvailable,
  );

  const buttons: BtnDef[] = [
    {
      key: "start",
      label: "Start",
      title: phase === "paused" ? "Resume entries" : "Start the bot",
      icon: <Play className="w-3 h-3" />,
      enabled: availability.start,
      onClick: onStart,
      activeClass: "text-profit hover:bg-profit-dim border-profit-border",
    },
    {
      key: "pause",
      label: "Pause",
      title: "Suspend new executions; positions preserved",
      icon: <Pause className="w-3 h-3" />,
      // Suspending is safe — stays available even when entries are disabled
      enabled: availability.pause,
      onClick: onPause,
      activeClass: "text-slate-700 hover:bg-surface-raised border-surface-border",
    },
    ...(reduceOnlyAvailable !== false
      ? [
          {
            key: "reduce",
            label: "Reduce Only",
            title:
              phase === "reduce_only"
                ? "Currently in Reduce-Only mode (exits only)"
                : !hasOpenPositions
                ? "Reduce Only requires open positions to drain"
                : phase !== "running" && phase !== "paused"
                ? "Reduce Only requires bot to be running or paused"
                : "Exit-only mode — stops automatically once this strategy is flat",
            icon: <ArrowDown className="w-3 h-3" />,
            enabled: availability.reduceOnly,
            onClick: onReduceOnly ?? (() => {}),
            activeClass: "text-warn-strong hover:bg-warn-dim border-warn-border",
          },
        ]
      : []),
    {
      key: "stop",
      label: stopping ? "Stopping…" : "Stop",
      title:
        "Emergency exit — immediately flatten this strategy's positions to delta-neutral",
      icon: <OctagonX className="w-3 h-3" />,
      // Emergency exit stays armed while a phase is active or positions remain
      enabled: availability.stop,
      onClick: () => setShowStopConfirm(true),
      activeClass: "text-loss hover:bg-loss-dim border-loss-border",
    },
  ];

  const phaseLabel =
    phase === "reduce_only"
      ? "Reduce-Only"
      : phase === "stopping"
      ? "Stopping"
      : null;

  return (
    <div className="flex items-center space-x-1.5 px-2 py-1 bg-surface border border-surface-border rounded-lg shadow-xs">
      <div className="flex items-center gap-1">
        {buttons.map((b) => (
          <button
            key={b.key}
            type="button"
            title={b.title}
            disabled={!b.enabled}
            onClick={b.onClick}
            className={`flex items-center space-x-1 px-2 py-2 rounded-lg border text-xs font-mono font-bold transition cursor-pointer disabled:opacity-35 disabled:cursor-not-allowed ${
              b.enabled ? b.activeClass : "text-muted border-surface-border bg-surface-raised/40"
            }`}
          >
            {b.icon}
            <span>{b.label}</span>
          </button>
        ))}
      </div>
      {phaseLabel && (
        <span className="text-xs font-mono font-bold px-2 py-0.5 rounded-md bg-warn-dim text-warn-strong border border-warn-border">
          {phaseLabel}
        </span>
      )}
      <ConfirmActionModal
        spec={
          showStopConfirm
            ? {
                title: "Emergency Stop",
                subtitle: "Flatten to delta-neutral",
                body: "This immediately dispatches close orders for every open position this strategy holds — market orders where the venue allows them. Unfilled residuals stay open and escalate for operator review.",
                confirmLabel: "Flatten Positions",
              }
            : null
        }
        onConfirm={onStop}
        onCancel={() => setShowStopConfirm(false)}
      />
    </div>
  );
};
