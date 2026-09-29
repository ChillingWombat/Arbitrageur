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
  /** Let Start re-arm while positions are open (options bots, whose managed
   *  positions ride to expiry — their cap, not flatness, gates entries). */
  allowStartWithOpenPositions?: boolean;
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

export type StrategyBotStatus = "Active" | "Paused" | "Reducing" | "Stopped";

export function resolveBotStatus(
  phase?: BotPhase,
  isPaused = false,
): StrategyBotStatus {
  if (phase === "running") {
    return isPaused ? "Paused" : "Active";
  }
  if (phase === "paused") {
    return "Paused";
  }
  if (phase === "reduce_only" || phase === "stopping") {
    return "Reducing";
  }
  return "Stopped";
}

export interface BotPhaseBadgeProps {
  phase?: BotPhase;
  isPaused?: boolean;
  status?: StrategyBotStatus;
}

/** Standardized strategy bot status badge — rendered by each strategy
 * page inside its header ribbon immediately to the right of the strategy
 * title header, before parameter tags. Displays exactly one of:
 * Active, Paused, Reducing, or Stopped. Never renders "Engine". */
export const BotPhaseBadge: React.FC<BotPhaseBadgeProps> = ({
  phase,
  isPaused = false,
  status,
}) => {
  const currentStatus = status ?? resolveBotStatus(phase, isPaused);

  const config: Record<
    StrategyBotStatus,
    { label: string; dotClass: string; badgeClass: string }
  > = {
    Active: {
      label: "Active",
      dotClass: "bg-profit",
      badgeClass: "bg-profit-dim text-profit border-profit-border",
    },
    Paused: {
      label: "Paused",
      dotClass: "bg-warn",
      badgeClass: "bg-warn-dim text-warn-strong border-warn-border",
    },
    Reducing: {
      label: "Reducing",
      dotClass: "bg-warn animate-pulse",
      badgeClass: "bg-warn-dim text-warn-strong border-warn-border",
    },
    Stopped: {
      label: "Stopped",
      dotClass: "bg-slate-400",
      badgeClass: "bg-surface-raised text-slate-500 border-surface-border",
    },
  };

  const { label, dotClass, badgeClass } = config[currentStatus];

  return (
    <span
      className={`inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full text-xs font-mono font-bold border shadow-2xs ${badgeClass}`}
      title={`Strategy bot status: ${label}`}
    >
      <span className={`w-1.5 h-1.5 rounded-full ${dotClass}`} />
      {label}
    </span>
  );
};

export const StrategyStatusBadge = BotPhaseBadge;


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
  allowStartWithOpenPositions = false,
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
    allowStartWithOpenPositions,
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
