import React from "react";
import { Play, Pause, OctagonX } from "lucide-react";
import type { RadarBotConfigDto, RadarBotStatusDto } from "../types";
import {
  botArmed,
  botBadgeTone,
  botControlsDisabled,
  botFlattenAvailable,
  describeBotMode,
} from "../services/radarBot";

export interface AutoBotGadgetProps {
  cfg: RadarBotConfigDto | null;
  status: RadarBotStatusDto | null;
  flattening: boolean;
  onMode: (mode: RadarBotConfigDto["mode"]) => void;
  onFlatten: () => void;
}

export const AutoBotGadget: React.FC<AutoBotGadgetProps> = ({
  cfg,
  status,
  flattening,
  onMode,
  onFlatten,
}) => {
  const armed = botArmed(cfg);
  const tone = botBadgeTone(cfg);
  const controlsOff = botControlsDisabled(cfg);
  const open = status?.openPositions ?? [];
  const flattenArmed = !flattening && botFlattenAvailable(open.length, cfg?.mode);

  const handleStop = () => {
    onMode("off");
    onFlatten();
  };

  return (
    <div className="px-4 py-2.5 flex flex-wrap items-center gap-x-4 gap-y-2 bg-surface rounded-xl border border-surface-border shadow-card shrink-0 text-xs">
      <span className="text-xs font-bold font-sans tracking-wider text-slate-900 uppercase">Auto-Bot</span>
      <span
        className={`px-2.5 py-0.5 rounded-full border text-[11px] font-sans font-bold shadow-xs transition-all ${
          tone === "armed"
            ? "bg-profit-dim text-profit border-profit-border"
            : tone === "draining"
              ? "bg-warn-dim text-warn border-warn-border"
              : "bg-surface-raised text-slate-600 border-surface-border"
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

      {/* Unified Bot Lifecycle Controls: Start / Pause / Stop */}
      <div className="flex items-center gap-1 p-0.5 bg-surface-raised border border-surface-border rounded-lg">
        <button
          type="button"
          title="Start the bot — monitor, enter, and automatically rebalance on convergence"
          disabled={controlsOff || armed}
          onClick={() => onMode("auto")}
          className={`flex items-center space-x-1 px-2.5 py-1 rounded-lg text-xs font-sans font-bold transition cursor-pointer disabled:opacity-35 disabled:cursor-not-allowed ${
            armed
              ? "bg-profit text-white shadow-xs font-bold"
              : "text-profit hover:bg-profit-dim border border-transparent hover:border-profit-border"
          }`}
        >
          <Play className="w-3 h-3" />
          <span>Start</span>
        </button>

        <button
          type="button"
          title="Pause new entries; open positions remain managed until target convergence"
          disabled={controlsOff || cfg?.mode === "paused"}
          onClick={() => onMode("paused")}
          className={`flex items-center space-x-1 px-2.5 py-1 rounded-lg text-xs font-sans font-semibold transition cursor-pointer disabled:opacity-35 disabled:cursor-not-allowed ${
            cfg?.mode === "paused"
              ? "bg-accent text-white shadow-xs font-bold"
              : "text-slate-700 hover:bg-slate-200/50"
          }`}
        >
          <Pause className="w-3 h-3" />
          <span>Pause</span>
        </button>

        <button
          type="button"
          title="Emergency exit — stop bot and immediately flatten this strategy's positions to delta-neutral"
          disabled={!flattenArmed}
          onClick={handleStop}
          className="flex items-center space-x-1 px-2.5 py-1 rounded-lg text-xs font-sans font-semibold transition cursor-pointer disabled:opacity-35 disabled:cursor-not-allowed text-loss hover:bg-loss-dim border border-transparent hover:border-loss-border bg-loss-dim/40 shadow-xs active:scale-[0.98]"
        >
          <OctagonX className="w-3 h-3" />
          <span>{flattening ? "Stopping…" : "Stop"}</span>
        </button>
      </div>
      <span className="text-slate-500 font-sans" title="Open convergence positions managed by the bot">
        Open <b className="font-mono tabular-nums text-slate-900">{open.length}</b>
      </span>
      <span className="text-slate-500 font-sans" title="Bot decisions today (durable audit)">
        in <b className="font-mono tabular-nums text-slate-900">{status?.entersToday ?? "—"}</b>
        {" / "}out <b className="font-mono tabular-nums text-slate-900">{status?.exitsToday ?? "—"}</b>
        {" / "}skip <b className="font-mono tabular-nums text-slate-900">{status?.skipsToday ?? "—"}</b>
      </span>
      {open.length > 0 && (
        <span className="text-slate-500 font-mono text-[11px] truncate max-w-80" title={open.map((p) => `${p.symbol} ${p.buyVenue}→${p.sellVenue}`).join(", ")}>
          {open.map((p) => p.symbol).join(", ")}
        </span>
      )}
      {status?.lastDecision && (
        <span className="text-slate-500 font-sans text-xs truncate max-w-96" title={`${status.lastDecision.pair}: ${status.lastDecision.reason}`}>
          last: <b className="text-slate-700">{status.lastDecision.decision}</b> {status.lastDecision.reason}
        </span>
      )}
      {status?.lastError && (
        <span className="text-loss font-sans text-xs truncate max-w-72" title={status.lastError}>
          {status.lastError}
        </span>
      )}
    </div>
  );
};
