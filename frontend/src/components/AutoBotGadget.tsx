import React from "react";
import { OctagonX } from "lucide-react";
import type { RadarBotConfigDto, RadarBotStatusDto } from "../types";
import {
  botArmed,
  botBadgeTone,
  botControlsDisabled,
  botFlattenAvailable,
  describeBotMode,
} from "../services/radarBot";

/**
 * The single radar bot control. Backend BotMode (off/auto/paused/reduce-only)
 * is the source of truth — this gadget writes it and reflects it, with no
 * page-local phase. The emergency Flatten entry point was merged in from the
 * removed BotLifecycleControls Stop button; the lifecycle Start/Pause/Reduce
 * buttons were dropped because they drove an in-memory phase that fought the
 * backend mode.
 */
const BOT_MODE_BTN: { mode: RadarBotConfigDto["mode"]; label: string; title: string }[] = [
  { mode: "auto", label: "Auto", title: "Arm entries + exits (paper-only)" },
  { mode: "paused", label: "Pause", title: "Freeze entries; exit watchers stay live" },
  { mode: "reduce_only", label: "Reduce", title: "Exits only; wind the book down" },
  { mode: "off", label: "Off", title: "Fully inert" },
];

export interface AutoBotGadgetProps {
  cfg: RadarBotConfigDto | null;
  status: RadarBotStatusDto | null;
  flattening: boolean;
  onMode: (mode: RadarBotConfigDto["mode"]) => void;
  onConfigure: () => void;
  onFlatten: () => void;
}

export const AutoBotGadget: React.FC<AutoBotGadgetProps> = ({
  cfg,
  status,
  flattening,
  onMode,
  onConfigure,
  onFlatten,
}) => {
  const armed = botArmed(cfg);
  const tone = botBadgeTone(cfg);
  const controlsOff = botControlsDisabled(cfg);
  const open = status?.openPositions ?? [];
  const flattenArmed = !flattening && botFlattenAvailable(open.length, cfg?.mode);
  return (
    <div className="px-4 py-3 flex flex-wrap items-center gap-x-4 gap-y-2 bg-surface rounded-lg border border-surface-border shadow-2xs shrink-0 font-mono text-xs">
      <span className="text-[13px] font-bold tracking-wider text-slate-900">Auto-Bot</span>
      <span
        className={`px-1.5 py-0.5 rounded-sm border font-bold ${
          tone === "armed"
            ? "bg-emerald-50 text-emerald-700 border-emerald-300"
            : tone === "draining"
              ? "bg-amber-50 text-amber-700 border-amber-200"
              : "bg-slate-100 text-slate-600 border-slate-300"
        }`}
        title={
          armed
            ? "Entries + exits armed (paper-only)"
            : cfg?.mode === "reduce_only" && open.length > 0
              ? "Auto-exit active: managing open positions until dynamic baseline convergence"
              : "Not arming entries"
        }
      >
        {armed
          ? "ARMED"
          : cfg?.mode === "reduce_only" && open.length > 0
            ? "MANAGING EXITS"
            : describeBotMode(cfg?.mode)}
      </span>
      <div className="flex items-center gap-1">
        {BOT_MODE_BTN.map((b) => (
          <button
            key={b.mode}
            type="button"
            title={b.title}
            disabled={controlsOff}
            onClick={() => onMode(b.mode)}
            className={`px-2 py-1 rounded-md border text-xs font-bold transition cursor-pointer disabled:opacity-35 disabled:cursor-not-allowed ${
              cfg?.mode === b.mode && (b.mode !== "auto" || cfg?.enabled)
                ? "bg-accent/10 text-accent border-accent-line"
                : "text-slate-600 border-surface-border hover:bg-slate-100"
            }`}
          >
            {b.label}
          </button>
        ))}
      </div>
      <button
        type="button"
        title="Emergency exit — flatten this strategy's positions to delta-neutral"
        disabled={!flattenArmed}
        onClick={onFlatten}
        className="flex items-center space-x-1 px-2 py-1 rounded-md border text-xs font-mono font-bold transition cursor-pointer disabled:opacity-35 disabled:cursor-not-allowed text-red-700 hover:bg-red-50 border-red-300"
      >
        <OctagonX className="w-3 h-3" />
        <span>{flattening ? "Flattening…" : "Flatten"}</span>
      </button>
      <span className="text-slate-500" title="Open convergence positions managed by the bot">
        Open <b className="text-slate-900">{open.length}</b>
      </span>
      <span className="text-slate-500" title="Bot decisions today (durable audit)">
        in <b className="text-slate-900">{status?.entersToday ?? "—"}</b>
        {" / "}out <b className="text-slate-900">{status?.exitsToday ?? "—"}</b>
        {" / "}skip <b className="text-slate-900">{status?.skipsToday ?? "—"}</b>
      </span>
      {open.length > 0 && (
        <span className="text-slate-500 truncate max-w-80" title={open.map((p) => `${p.symbol} ${p.buyVenue}→${p.sellVenue}`).join(", ")}>
          {open.map((p) => p.symbol).join(", ")}
        </span>
      )}
      {status?.lastDecision && (
        <span className="text-slate-500 truncate max-w-96" title={`${status.lastDecision.pair}: ${status.lastDecision.reason}`}>
          last: <b className="text-slate-700">{status.lastDecision.decision}</b> {status.lastDecision.reason}
        </span>
      )}
      {status?.lastError && (
        <span className="text-red-600 truncate max-w-72" title={status.lastError}>
          {status.lastError}
        </span>
      )}
      <button
        type="button"
        onClick={onConfigure}
        className="ml-auto px-2.5 py-1 rounded-md border border-surface-border text-slate-600 hover:bg-slate-100 text-xs font-bold transition cursor-pointer"
        title="Configure auto-bot knobs"
      >
        Configure auto-bot
      </button>
    </div>
  );
};
