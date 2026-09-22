import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Activity,
  ShieldCheck,
  Check,
  Search,
  SlidersHorizontal,
  ChevronDown,
  ChevronRight,
  Clock,
  EyeOff,
  Layers,
  RotateCcw,
  TrendingUp,
  X,
} from "lucide-react";
import { BotLifecycleControls } from "./BotLifecycleControls";
import { BotStatCards } from "./BotStatCards";
import { InfoTooltip } from "./InfoTooltip";
import { SaveButton, useSaveButton } from "./SaveButton";
import { SwitchToggle } from "./SwitchToggle";
import type {
  MonitorSnapshotDto,
  MonitorRowDto,
  MonitorConfigDto,
  RadarBotConfigDto,
  RadarBotStatusDto,
  SignalExecutionDto,
  StrategyFillPnl,
} from "../types";
import { api } from "../services/api";
import {
  buildHeldSet,
  botControlsDisabled,
  defaultRadarBotConfig,
  isRowHeld,
  resolveRadarExecSize,
} from "../services/radarBot";
import { computeBotCycleStats } from "../services/botCycleStats";
import { flattenResidualMessage } from "../services/flatten";
import { EmptyState } from "./EmptyState";
import { VenueLogo } from "./VenueLogo";

/* ------------------------------------------------------------------ */
/* Formatting helpers — missing data always renders as "—", never 0    */
/* ------------------------------------------------------------------ */

const fmtBps = (v: number | null | undefined, digits = 1) =>
  v == null || !Number.isFinite(v) ? "—" : `${v.toFixed(digits)}`;

// Table cells carry the unit; headers stay unitless.
const bpsUnit = <span className="text-slate-400 font-normal"> bps</span>;
const fmtBpsU = (v: number | null | undefined, digits = 1) => (
  <>
    {fmtBps(v, digits)}
    {v != null && Number.isFinite(v) ? bpsUnit : null}
  </>
);

const fmtUsd = (v: number | null | undefined) =>
  v == null || !Number.isFinite(v)
    ? "—"
    : v >= 1000
      ? `$${(v / 1000).toFixed(1)}k`
      : `$${v.toFixed(0)}`;

const fmtPct = (v: number | null | undefined) =>
  v == null || !Number.isFinite(v) ? "—" : `${(v * 100).toFixed(0)}%`;

const fmtDur = (secs: number | null | undefined) => {
  if (secs == null || !Number.isFinite(secs)) return "—";
  if (secs < 60) return `${secs.toFixed(0)}s`;
  if (secs < 3600) return `${(secs / 60).toFixed(0)}m`;
  if (secs < 86400) return `${(secs / 3600).toFixed(1)}h`;
  return `${(secs / 86400).toFixed(1)}d`;
};

const fmtTs = (ms: number | null | undefined) =>
  ms == null ? "—" : new Date(ms).toLocaleTimeString();

/* ------------------------------------------------------------------ */
/* Win probability helpers — reversion (detail-only) vs trade win (table) */
/* ------------------------------------------------------------------ */

/** Direction-insensitive mean reversion P(|dev| reverts). Detail-only. */
const getReversion = (r: MonitorRowDto): number | null | undefined =>
  (r as { reversionProbability?: number | null }).reversionProbability ?? r.pWin;

/** Directional trade win probability after all gates. Table Win % + Min Win filter. */
const getWin = (r: MonitorRowDto): number | null | undefined =>
  (r as { winProbability?: number | null }).winProbability ?? r.pWin;

const STATUS_STYLE: Record<string, string> = {
  ready: "bg-profit-dim text-profit border-profit-border",
  triggered: "bg-warn-dim text-warn border-warn-border",
  tracking: "bg-sky-50 text-sky-700 border-sky-300",
  preparing: "bg-surface-raised text-slate-600 border-surface-border",
  "no-data": "bg-loss-dim text-loss border-loss-border",
};

const VENUE_NAME: Record<string, string> = {
  hyperliquid: "Hyperliquid",
  lighter: "Lighter",
  robinhood: "Robinhood",
  vanta: "Vanta",
  aster: "Aster",
  arcus: "Arcus",
  popdex: "PopDEX",
  kraken_spot: "Kraken Spot",
  kraken_futures: "Kraken Futures",
  polymarket: "Polymarket",
  kalshi: "Kalshi",
  binance: "Binance",
  coinbase: "Coinbase",
  decibel: "Decibel",
  ibkr: "IBKR",
  ig: "IG",
};

const VENUE_LABEL: Record<string, string> = {
  hyperliquid: "HL",
  lighter: "LGT",
  robinhood: "RH",
  vanta: "VNT",
  aster: "AST",
  arcus: "ARC",
  popdex: "POP",
  kraken_spot: "KRK",
  kraken_futures: "KFUT",
  polymarket: "POLY",
  kalshi: "KLSH",
  binance: "BNB",
  coinbase: "CB",
  decibel: "DCB",
  ibkr: "IBKR",
  ig: "IG",
  okx: "OKX",
};

const LIVE_CAPABLE_VENUES = new Set([
  "aster",
  "popdex",
  "hyperliquid",
  "lighter",
  "robinhood",
  "vanta",
  "binance",
  "okx",
  "coinbase",
  "kraken",
]);
const isLiveCapableRoute = (v1: string, v2: string): boolean =>
  LIVE_CAPABLE_VENUES.has(v1.toLowerCase()) && LIVE_CAPABLE_VENUES.has(v2.toLowerCase());

/* ------------------------------------------------------------------ */
/* Config modal (TickerConfigModal canon)                                */
/* ------------------------------------------------------------------ */

type SectionIcon = React.ComponentType<{ className?: string }>;

/** Settings sections — the left panel navigates these. */
const MONITOR_SECTIONS: { id: string; label: string; tooltip: string; icon: SectionIcon }[] = [
  { id: "monitor", label: "Monitor", tooltip: "Master enable switch plus background refresh cadences for the watchlist and history.", icon: Activity },
  { id: "qualification", label: "Qualification window", tooltip: "Configure how much spread history qualifies a pair: auto from mean-reversion half-life or an explicit manual window.", icon: Clock },
  { id: "signal", label: "Signal thresholds", tooltip: "Engine gates for qualification (edge, capacity, win-probability, persistence, coverage) plus bot entry hurdles. Table display filters live on the gadget strip, not here.", icon: TrendingUp },
  { id: "execution", label: "Execution", tooltip: "Configure Single-Way versus Two-Way execution, auto-entry arming, and the transaction reserve haircut.", icon: Layers },
  { id: "risk", label: "Risk & Limits", tooltip: "Position caps, per-venue and per-symbol notional limits, cooldowns, and stop/unwind guardrails shared by auto and manual entries.", icon: ShieldCheck },
];

const RISK_NUM_FIELDS: { key: keyof RadarBotConfigDto; label: string; tooltip: string; step?: string }[] = [
  { key: "maxConcurrentPositions", label: "Max concurrent positions", tooltip: "Cap on simultaneously managed convergence positions.", step: "1" },
  { key: "maxNotionalPerPositionUsd", label: "Max $ per position", tooltip: "Per-entry notional cap in USD." },
  { key: "maxNotionalPerVenueUsd", label: "Max $ per venue", tooltip: "Open bot notional cap summed per venue leg in USD." },
  { key: "maxNotionalPerSymbolUsd", label: "Max $ per symbol", tooltip: "Open bot notional cap per ticker in USD." },
  { key: "pairCooldownSecs", label: "Pair cooldown (s)", tooltip: "Re-entry blackout per pair after an exit or stop.", step: "1" },
  { key: "holdTimeoutMult", label: "Hold timeout ×", tooltip: "Max-hold deadline as a multiple of the signal's expected convergence time." },
  { key: "stopMult", label: "Stop × entry dev", tooltip: "Stop band as a multiple of entry |deviation|, floored at the friction band." },
  { key: "staleUnwindSecs", label: "Stale unwind (s)", tooltip: "Grace while a pair is unwatched before reduce-only unwind.", step: "1" },
  { key: "pollIntervalSecs", label: "Poll interval (s)", tooltip: "Executor tick cadence.", step: "1" },
];

/** Factory defaults for Reset Defaults — untouched backend fields are preserved from the opened config. */
const DEFAULT_MONITOR_CONFIG: MonitorConfigDto = {
  enabled: true,
  watchlistRefreshSecs: 3600,
  qualificationMode: "auto",
  manualWindowSecs: 86400,
  halfLifeMultiplier: 2,
  minWindowSecs: 1800,
  frictionCoverMult: 1,
  capacityTiersUsd: [100, 500, 1000, 5000, 10000],
  minWinProbability: 0.9,
  executionMode: "one_way",
  participationCap: 1,
  txReservePct: 0.25,
  minEpisodes: 3,
  historyWindowSecs: 86400,
  maxSymbolIntervalSecs: 300,
  minSymbolIntervalSecs: 5,
  minExpectedProfitBps: 0,
  minThresholdRatio: 0.2,
  episodeHalfLifeSecs: 259200,
  minWinLowerBound: 0.5,
  minCoverage: 0.5,
};

const INPUT_INLINE_CLS =
  "w-28 shrink-0 px-3 py-2 bg-surface border border-surface-border rounded-lg text-slate-900 font-bold text-xs focus:outline-none focus:ring-2 focus:ring-accent/20 focus:border-accent transition-all h-[38px] font-mono tabular-nums shadow-2xs";
const SELECT_INLINE_CLS =
  "w-44 shrink-0 px-3 py-2 bg-surface border border-surface-border rounded-lg text-slate-900 font-medium text-xs focus:outline-none focus:ring-2 focus:ring-accent/20 focus:border-accent transition-all h-[38px] shadow-2xs";
const CARD_CLS =
  "bg-surface p-4 rounded-xl border border-surface-border space-y-1.5 shadow-card hover:border-slate-300 transition-all";
const LABEL_CLS = "text-xs text-slate-700 font-medium flex items-center font-sans";

const ConfigPanel: React.FC<{
  cfg: MonitorConfigDto;
  botCfg: RadarBotConfigDto;
  onSave: (c: MonitorConfigDto, b: RadarBotConfigDto) => Promise<void>;
  onClose: () => void;
}> = ({ cfg, botCfg, onSave, onClose }) => {
  const [draft, setDraft] = useState<MonitorConfigDto>(cfg);
  const [botDraft, setBotDraft] = useState<RadarBotConfigDto>(botCfg);
  const saveBtn = useSaveButton();
  const [error, setError] = useState<string | null>(null);
  const [activeSection, setActiveSection] = useState<string>(MONITOR_SECTIONS[0].id);
  const set = <K extends keyof MonitorConfigDto>(k: K, v: MonitorConfigDto[K]) => {
    setError(null);
    setDraft((d) => ({ ...d, [k]: v }));
  };
  const setBot = <K extends keyof RadarBotConfigDto>(k: K, v: RadarBotConfigDto[K]) => {
    setError(null);
    setBotDraft((d) => ({ ...d, [k]: v }));
  };

  const num = (v: string, fallback: number) => {
    const n = Number(v);
    return Number.isFinite(n) ? n : fallback;
  };

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  const handleResetDefaults = () => {
    setDraft((d) => ({
      ...DEFAULT_MONITOR_CONFIG,
      participationCap: d.participationCap,
      minEpisodes: d.minEpisodes,
      maxSymbolIntervalSecs: d.maxSymbolIntervalSecs,
      minSymbolIntervalSecs: d.minSymbolIntervalSecs,
    }));
    // Bot mode is driven from the main-page lifecycle buttons — Reset Defaults
    // must not silently flip it.
    setBotDraft((d) => ({ ...defaultRadarBotConfig(), mode: d.mode }));
  };

  const handleSave = async () => {
    setError(null);
    const ok = await saveBtn.run(async () => {
      try {
        await onSave({ ...draft, enabled: true }, botDraft);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        throw e; // rethrow so the save button lands on "Failed!"
      }
    });
    if (ok) setTimeout(onClose, 400);
  };

  return (
    <div
      className="fixed inset-0 z-50 bg-slate-900/50 backdrop-blur-sm flex items-center justify-center p-3 sm:p-6 select-none animate-in fade-in duration-150"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="bg-surface border border-surface-border rounded-2xl w-[960px] h-[680px] max-w-[96vw] max-h-[92vh] flex flex-col shadow-2xl overflow-hidden animate-in zoom-in-95 duration-150">
        {/* Modal Top Header Bar */}
        <div className="flex items-center justify-between px-6 py-3.5 border-b border-surface-border bg-surface shrink-0">
          <div className="flex items-center space-x-3">
            <div className="w-8 h-8 rounded-lg bg-accent-dim border border-accent-line flex items-center justify-center text-accent shadow-2xs">
              <SlidersHorizontal className="w-4 h-4" />
            </div>
            <div>
              <h2 className="text-sm font-bold font-sans tracking-tight text-slate-900 leading-tight">
                Spread Radar Configurator
              </h2>
              <p className="text-[11px] font-sans text-slate-500">Monitor thresholds, probability gates, execution and risk limits</p>
            </div>
          </div>
          <div className="flex items-center space-x-3">
            <button
              type="button"
              onClick={onClose}
              className="p-1.5 rounded-lg text-slate-400 hover:text-slate-700 hover:bg-slate-100 transition border border-transparent hover:border-surface-border cursor-pointer"
              title="Close Configurator (ESC)"
            >
              <X className="w-4 h-4" />
            </button>
          </div>
        </div>

        {/* Modal Middle Body: Left Sidebar + Right Content Area */}
        <div className="flex-1 flex min-h-0 overflow-hidden font-mono">
          {/* Left Vertical Section Selector Panel */}
          <div className="w-56 shrink-0 border-r border-surface-border bg-slate-50/70 p-3 flex flex-col justify-between select-none overflow-y-auto">
            <div className="space-y-3">
              <div className="px-2 pt-1 flex items-center justify-between">
                <span className="text-xs font-mono font-bold tracking-wider text-slate-400">
                  Settings
                </span>
              </div>
              <div className="space-y-1">
                {MONITOR_SECTIONS.map((s) => {
                  const Icon = s.icon;
                  return (
                    <button
                      key={s.id}
                      type="button"
                      onClick={() => setActiveSection(s.id)}
                      className={`w-full flex items-center space-x-2.5 px-2.5 py-2.5 rounded-lg font-mono text-[11px] transition cursor-pointer text-left ${
                        activeSection === s.id
                          ? "bg-accent text-white font-bold shadow-2xs border border-accent"
                          : "text-slate-600 hover:text-slate-900 bg-surface hover:bg-slate-100/80 border border-surface-border/60 hover:border-surface-border"
                      }`}
                    >
                      <Icon className={`w-3.5 h-3.5 shrink-0 ${activeSection === s.id ? "text-white" : "text-slate-400"}`} />
                      <div className="min-w-0 flex-1 flex items-center">
                        <span className="truncate">{s.label}</span>
                        <InfoTooltip
                          text={s.tooltip}
                          iconClassName={activeSection === s.id ? "text-blue-100 hover:text-white" : undefined}
                        />
                      </div>
                    </button>
                  );
                })}
              </div>
            </div>
          </div>

          {/* Right Main Content Area */}
          <div className="flex-1 overflow-y-auto p-4 pr-5 space-y-4 bg-surface">
            {activeSection === "monitor" && (
              <div className="space-y-3">
                <div className={`${CARD_CLS} flex items-center justify-between gap-3`}>
                  <label className={`${LABEL_CLS} flex-1`}>
                    <span className="flex items-center">
                      Watchlist refresh (h)
                      <InfoTooltip text="How often the eligible watchlist is rebuilt from venue market coverage, in hours." />
                    </span>
                  </label>
                  <input
                    className={INPUT_INLINE_CLS}
                    type="number"
                    min={1}
                    value={draft.watchlistRefreshSecs / 3600}
                    onChange={(e) => set("watchlistRefreshSecs", num(e.target.value, 1) * 3600)}
                  />
                </div>
                <div className={`${CARD_CLS} flex items-center justify-between gap-3`}>
                  <label className={`${LABEL_CLS} flex-1`}>
                    <span className="flex items-center">
                      History window (h)
                      <InfoTooltip text="Trailing duration of persisted spread history retained for baselines, in hours." />
                    </span>
                  </label>
                  <input
                    className={INPUT_INLINE_CLS}
                    type="number"
                    min={1}
                    value={draft.historyWindowSecs / 3600}
                    onChange={(e) => set("historyWindowSecs", num(e.target.value, 24) * 3600)}
                  />
                </div>
              </div>
            )}

            {activeSection === "qualification" && (
              <div className="space-y-3">
                <div className={`${CARD_CLS} flex items-center justify-between gap-3`}>
                  <label className={`${LABEL_CLS} flex-1`}>
                    <span className="flex items-center">
                      Qualification
                      <InfoTooltip text="Auto derives the qualification window per pair from mean-reversion half-life; manual uses the explicit window below." />
                    </span>
                  </label>
                  <select
                    className={SELECT_INLINE_CLS}
                    value={draft.qualificationMode}
                    onChange={(e) => set("qualificationMode", e.target.value as "auto" | "manual")}
                  >
                    <option value="auto">Auto (2× half-life)</option>
                    <option value="manual">Manual</option>
                  </select>
                </div>
                <div className={`${CARD_CLS} flex items-center justify-between gap-3`}>
                  <label className={`${LABEL_CLS} flex-1`}>
                    <span className="flex items-center">
                      Manual window (h)
                      <InfoTooltip text="Explicit qualification window used when qualification mode is manual, in hours." />
                    </span>
                  </label>
                  <input
                    className={INPUT_INLINE_CLS}
                    type="number"
                    min={1}
                    value={draft.manualWindowSecs / 3600}
                    onChange={(e) => set("manualWindowSecs", num(e.target.value, 24) * 3600)}
                  />
                </div>
                <div className={`${CARD_CLS} flex items-center justify-between gap-3`}>
                  <label className={`${LABEL_CLS} flex-1`}>
                    <span className="flex items-center">
                      Half-life multiplier
                      <InfoTooltip text="Qualification window equals this multiple of the pair's mean-reversion half-life in auto mode." />
                    </span>
                  </label>
                  <input
                    className={INPUT_INLINE_CLS}
                    type="number"
                    step={0.5}
                    min={0.5}
                    value={draft.halfLifeMultiplier}
                    onChange={(e) => set("halfLifeMultiplier", num(e.target.value, 2))}
                  />
                </div>
                <div className={`${CARD_CLS} flex items-center justify-between gap-3`}>
                  <label className={`${LABEL_CLS} flex-1`}>
                    <span className="flex items-center">
                      Min window (min)
                      <InfoTooltip text="Floor for the auto-derived qualification window, in minutes." />
                    </span>
                  </label>
                  <input
                    className={INPUT_INLINE_CLS}
                    type="number"
                    min={5}
                    value={draft.minWindowSecs / 60}
                    onChange={(e) => set("minWindowSecs", num(e.target.value, 30) * 60)}
                  />
                </div>
              </div>
            )}

            {activeSection === "signal" && (
              <div className="space-y-3">
                <div className={`${CARD_CLS} flex items-center justify-between gap-3`}>
                  <label className={`${LABEL_CLS} flex-1`}>
                    <span className="flex items-center">
                      Friction cover multiplier
                      <InfoTooltip text="Deviation must cover at least this multiple of crossing friction before a pair can qualify." />
                    </span>
                  </label>
                  <input
                    className={INPUT_INLINE_CLS}
                    type="number"
                    step={0.1}
                    min={0.1}
                    value={draft.frictionCoverMult}
                    onChange={(e) => set("frictionCoverMult", num(e.target.value, 1))}
                  />
                </div>
                <div className={`${CARD_CLS} flex items-center justify-between gap-3`}>
                  <label className={`${LABEL_CLS} flex-1`}>
                    <span className="flex items-center">
                      Capacity tiers ($)
                      <InfoTooltip text="Comma-separated notional tiers used to build the per-row capacity ladder, in USD." />
                    </span>
                  </label>
                  <input
                    className={SELECT_INLINE_CLS}
                    value={draft.capacityTiersUsd.join(",")}
                    onChange={(e) =>
                      set(
                        "capacityTiersUsd",
                        e.target.value
                          .split(",")
                          .map((s) => Number(s.trim()))
                          .filter((n) => Number.isFinite(n) && n > 0),
                      )
                    }
                  />
                </div>
                <div className={`${CARD_CLS} flex items-center justify-between gap-3`}>
                  <label className={`${LABEL_CLS} flex-1`}>
                    <span className="flex items-center">
                      Min win probability
                      <InfoTooltip text="Engine gate — backend marks pairs Qualified only above this and sizes capacity off it. To only change what the table shows, use the gadget's Min Win display filter instead." />
                    </span>
                  </label>
                  <input
                    className={INPUT_INLINE_CLS}
                    type="number"
                    step={0.01}
                    min={0.5}
                    max={1}
                    value={draft.minWinProbability}
                    onChange={(e) => set("minWinProbability", num(e.target.value, 0.9))}
                  />
                </div>
                <div className={`${CARD_CLS} flex items-center justify-between gap-3`}>
                  <label className={`${LABEL_CLS} flex-1`}>
                    <span className="flex items-center">
                      Min win lower bound
                      <InfoTooltip text="Confidence floor on the Wilson 90% lower bound of the win estimate — a shrunken point estimate can clear Min Win on thin evidence; this floor refuses it. 0 disables." />
                    </span>
                  </label>
                  <input
                    className={INPUT_INLINE_CLS}
                    type="number"
                    step={0.05}
                    min={0}
                    max={0.99}
                    value={draft.minWinLowerBound}
                    onChange={(e) => set("minWinLowerBound", num(e.target.value, 0.5))}
                  />
                </div>
                <div className={`${CARD_CLS} flex items-center justify-between gap-3`}>
                  <label className={`${LABEL_CLS} flex-1`}>
                    <span className="flex items-center">
                      Episode half-life (s)
                      <InfoTooltip text="Recency half-life for episode statistics — older excursions weigh exponentially less in win/outcome estimates. 0 disables weighting (all history counts equally)." />
                    </span>
                  </label>
                  <input
                    className={INPUT_INLINE_CLS}
                    type="number"
                    step={3600}
                    min={0}
                    value={draft.episodeHalfLifeSecs}
                    onChange={(e) => set("episodeHalfLifeSecs", num(e.target.value, 259200))}
                  />
                </div>
                <div className={`${CARD_CLS} flex items-center justify-between gap-3`}>
                  <label className={`${LABEL_CLS} flex-1`}>
                    <span className="flex items-center">
                      Min persistence ratio
                      <InfoTooltip text="Fraction of in-window buckets at/above entry that a pair must hold to qualify (0–1; 0 disables the gate)." />
                    </span>
                  </label>
                  <input
                    className={INPUT_INLINE_CLS}
                    type="number"
                    step={0.05}
                    min={0}
                    max={1}
                    value={draft.minThresholdRatio}
                    onChange={(e) => set("minThresholdRatio", Math.min(1, Math.max(0, num(e.target.value, 0.2))))}
                  />
                </div>
                <div className={`${CARD_CLS} flex items-center justify-between gap-3`}>
                  <label className={`${LABEL_CLS} flex-1`}>
                    <span className="flex items-center">
                      Min coverage
                      <InfoTooltip text="Fraction of expected window buckets with both venues observed required to qualify (0–1; 0 disables the gate)." />
                    </span>
                  </label>
                  <input
                    className={INPUT_INLINE_CLS}
                    type="number"
                    step={0.05}
                    min={0}
                    max={1}
                    value={draft.minCoverage}
                    onChange={(e) => set("minCoverage", Math.min(1, Math.max(0, num(e.target.value, 0.5))))}
                  />
                </div>
                <div className={`${CARD_CLS} flex items-center justify-between gap-3`}>
                  <label className={`${LABEL_CLS} flex-1`}>
                    <span className="flex items-center">
                      Entry min expected profit (bps)
                      <InfoTooltip text="Bot entry hurdle on probability-weighted expected profit after friction — stricter than the monitor display floor above." />
                    </span>
                  </label>
                  <input
                    className={INPUT_INLINE_CLS}
                    type="number"
                    step={0.5}
                    value={Number(botDraft.minExpectedProfitBps)}
                    onChange={(e) => setBot("minExpectedProfitBps", num(e.target.value, 0))}
                  />
                </div>
              </div>
            )}

            {activeSection === "execution" && (
              <div className="space-y-3">
                <div className={`${CARD_CLS} flex items-center justify-between gap-3`}>
                  <label className={`${LABEL_CLS} flex-1`}>
                    <span className="flex items-center">
                      Execution
                      <InfoTooltip text="Single-Way opens the spread only; Two-Way also prices the closing leg into expected profit." />
                    </span>
                  </label>
                  <select
                    className={SELECT_INLINE_CLS}
                    value={draft.executionMode}
                    onChange={(e) => set("executionMode", e.target.value as "one_way" | "two_way")}
                  >
                    <option value="one_way">Single-Way (entry only)</option>
                    <option value="two_way">Two-Way (+ close)</option>
                  </select>
                </div>
                <div className={`${CARD_CLS} flex items-center justify-between gap-3`}>
                  <label className={`${LABEL_CLS} flex-1`}>
                    <span className="flex items-center">
                      Arm auto entries
                      <InfoTooltip text="Master opt-in for automatic entries on filtered signals — auto entries fire only when armed AND the main-page bot is Started. Manual monitor-table entries always place through the bot with automatic exits. Ships off." />
                    </span>
                  </label>
                  <SwitchToggle size="sm" checked={botDraft.enabled} onChange={(v) => setBot("enabled", v)} />
                </div>
                <div className={`${CARD_CLS} flex items-center justify-between gap-3`}>
                  <label className={`${LABEL_CLS} flex-1`}>
                    <span className="flex items-center">
                      Tx reserve
                      <InfoTooltip text="Fraction of notional reserved for transaction costs, from 0 to 0.9." />
                    </span>
                  </label>
                  <input
                    className={INPUT_INLINE_CLS}
                    type="number"
                    step={0.05}
                    min={0}
                    max={0.9}
                    value={draft.txReservePct}
                    onChange={(e) => set("txReservePct", num(e.target.value, 0.25))}
                  />
                </div>
              </div>
            )}

            {activeSection === "risk" && (
              <div className="space-y-3">
                {RISK_NUM_FIELDS.map((f) => (
                  <div className={`${CARD_CLS} flex items-center justify-between gap-3`} key={f.key}>
                    <label className={`${LABEL_CLS} flex-1`}>
                      <span className="flex items-center">
                        {f.label}
                        <InfoTooltip text={f.tooltip} />
                      </span>
                    </label>
                    <input
                      className={INPUT_INLINE_CLS}
                      type="number"
                      min={0}
                      step={f.step ?? "any"}
                      value={Number(botDraft[f.key])}
                      onChange={(e) =>
                        setBot(f.key, num(e.target.value, Number(botDraft[f.key])) as never)
                      }
                    />
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>

        {/* Modal Bottom Footer Actions */}
        <div className="px-6 py-3.5 border-t border-surface-border bg-surface flex items-center justify-between shrink-0 font-mono text-xs">
          <button
            type="button"
            onClick={handleResetDefaults}
            className="flex items-center space-x-1.5 px-2 py-2 bg-surface hover:bg-surface-raised text-slate-600 hover:text-slate-900 rounded-lg border border-surface-border transition cursor-pointer"
            title="Reset to default monitor parameters"
          >
            <RotateCcw className="w-3.5 h-3.5 text-slate-500" />
            <span>Reset Defaults</span>
          </button>
          <div className="flex items-center space-x-3">
            {error && (
              <span className="text-xs font-mono text-loss max-w-72 truncate" title={error}>
                {error}
              </span>
            )}
            <button
              type="button"
              onClick={onClose}
              className="px-2.5 py-2.5 bg-surface hover:bg-surface-raised text-slate-700 border border-surface-border rounded-lg font-medium transition cursor-pointer"
            >
              Cancel
            </button>
            <SaveButton state={saveBtn.state} onClick={handleSave} />
          </div>
      </div>
      </div>
    </div>
  );
};

/* ------------------------------------------------------------------ */
/* Signal detail popup (capacity ladder + full signal fields)            */
/* ------------------------------------------------------------------ */

const CapacityLadder: React.FC<{ row: MonitorRowDto }> = React.memo(({ row }) => (
  <div className="flex items-end gap-3">
    {row.capacityLadder.length === 0 && (
      <span className="text-slate-400">no depth data</span>
    )}
    {row.capacityLadder.map((t) => {
      const h = Math.max(
        8,
        Math.min(48, ((t.expectedNetBps ?? 0) / 15) * 48 + 8),
      );
      const good = (t.expectedNetBps ?? -1) > 0;
      return (
        <div key={t.sizeUsd} className="flex flex-col items-center gap-1">
          <div
            className={`w-10 rounded-lg ${good ? "bg-accent/70" : "bg-slate-300"}`}
            style={{ height: `${h}px` }}
            title={`net ${fmtBps(t.netBps)}bp · signed E ${fmtBps(t.expectedNetBps)}bp · reversion p ${fmtPct(t.marginalPWin)}`}
          />
          <span className="text-xs text-slate-500">{fmtUsd(t.sizeUsd)}</span>
          <span className={`text-xs ${good ? "text-accent" : "text-slate-400"}`}>
            {t.filledUsd < t.sizeUsd - 0.5
              ? `${fmtUsd(t.filledUsd)} fill`
              : fmtBps(t.expectedNetBps)}
          </span>
        </div>
      );
    })}
    <div className="ml-4 text-xs text-slate-500 leading-4">
      <div>
        optimal <span className="text-slate-700 font-semibold">{fmtUsd(row.optimalSizeUsd)}</span>
      </div>
      <div>
        breakeven <span className="text-slate-700 font-semibold">{fmtUsd(row.breakevenSizeUsd)}</span>
      </div>
    </div>
  </div>
));

const DetailField: React.FC<{
  label: string;
  value: React.ReactNode;
  title?: string;
  tip?: string;
}> = ({ label, value, title, tip }) => (
  <div className="space-y-0.5" title={title}>
    <div className="text-[11px] font-sans text-slate-400 font-medium flex items-center">
      {label}
      {tip && <InfoTooltip text={tip} position="top" />}
    </div>
    <div className="text-sm font-bold font-sans text-slate-900 tabular-nums">{value}</div>
  </div>
);

const DetailSection: React.FC<{ title: string; children: React.ReactNode }> = ({
  title,
  children,
}) => (
  <div className="space-y-2">
    <div className="text-[11px] font-sans font-bold uppercase tracking-wider text-slate-400">
      {title}
    </div>
    <div className="grid grid-cols-2 sm:grid-cols-3 gap-x-4 gap-y-3">{children}</div>
  </div>
);

interface SignalDetailModalProps {
  row: MonitorRowDto;
  held: boolean;
  sizeValue: number | undefined;
  executing: boolean;
  onSizeChange: (rowKey: string, value: number) => void;
  onExecute: (row: MonitorRowDto) => void;
  onClose: () => void;
}

const SignalDetailModal: React.FC<SignalDetailModalProps> = React.memo(
  ({ row: r, held, sizeValue, executing, onSizeChange, onExecute, onClose }) => {
    const rowKey = `${r.symbol}:${r.buyVenue}:${r.sellVenue}`;
    const live = isLiveCapableRoute(r.buyVenue, r.sellVenue);
    useEffect(() => {
      const handleKeyDown = (e: KeyboardEvent) => {
        if (e.key === "Escape") onClose();
      };
      window.addEventListener("keydown", handleKeyDown);
      return () => window.removeEventListener("keydown", handleKeyDown);
    }, [onClose]);
    return (
      <div
        className="fixed inset-0 z-50 bg-slate-900/50 backdrop-blur-sm flex items-center justify-center p-3 sm:p-6 select-none"
        onClick={(e) => {
          if (e.target === e.currentTarget) onClose();
        }}
      >
        <div className="bg-surface border border-surface-border rounded-2xl w-[720px] max-w-[94vw] max-h-[90vh] flex flex-col shadow-2xl overflow-hidden">
          <div className="flex items-center justify-between px-6 py-3.5 border-b border-surface-border bg-surface shrink-0">
            <div className="flex items-center gap-2 min-w-0">
              <span className="text-sm font-bold font-sans text-slate-900">{r.symbol}</span>
              <span className="inline-flex items-center gap-1 text-xs font-sans text-slate-500">
                <VenueLogo venueId={r.buyVenue} name={VENUE_NAME[r.buyVenue] ?? r.buyVenue} size={15} />
                {VENUE_LABEL[r.buyVenue] ?? r.buyVenue}→
                <VenueLogo venueId={r.sellVenue} name={VENUE_NAME[r.sellVenue] ?? r.sellVenue} size={15} />
                {VENUE_LABEL[r.sellVenue] ?? r.sellVenue}
              </span>
              {held && (
                <span
                  className="px-1.5 py-0.5 rounded-full border border-profit-border bg-profit-dim text-profit text-[10px] font-sans font-bold"
                  title="Bot-managed convergence position open on this pair"
                >
                  bot
                </span>
              )}
              <span
                className={`px-1.5 py-0.5 rounded-full border text-[10px] font-sans font-bold ${
                  live
                    ? "border-profit-border bg-profit-dim text-profit"
                    : "border-surface-border bg-surface-raised text-slate-500"
                }`}
                title={live ? "Both venues support live API execution" : "Route involves a read-only venue — paper execution only"}
              >
                {live ? "LIVE" : "PAPER"}
              </span>
              <span
                className={`px-2 py-0.5 rounded-full border text-[11px] font-sans font-semibold ${STATUS_STYLE[r.status]}`}
                title={
                  r.status === "preparing"
                    ? `history ${(r.qualificationProgress * 100).toFixed(0)}% of window · ${r.observedEpisodes} episodes`
                    : r.blockReason ?? r.status
                }
              >
                {r.status}
                {r.status === "preparing" && ` ${(r.qualificationProgress * 100).toFixed(0)}%`}
              </span>
            </div>
            <button
              type="button"
              onClick={onClose}
              className="p-1.5 rounded-lg text-slate-400 hover:text-slate-700 hover:bg-slate-100 transition cursor-pointer"
              title="Close details (ESC)"
            >
              <X className="w-4 h-4" />
            </button>
          </div>

          <div className="flex-1 overflow-y-auto p-5 space-y-5">
            <DetailSection title="Spread state">
              <DetailField
                label="Baseline"
                value={fmtBpsU(r.baselineBps)}
                tip="Fair-value mean the pair spread reverts to, estimated from the sample window (bps)."
              />
              <DetailField
                label="Deviation"
                value={fmtBpsU(r.deviationBps)}
                tip="Current signed distance of the live spread from baseline (bps). Sign sets trade direction."
              />
              <DetailField
                label="Net capture"
                value={fmtBpsU(r.netCaptureBps)}
                tip="Current spread minus fees and modeled slippage at the displayed size (bps)."
              />
              <DetailField
                label="Persistence"
                value={fmtPct(r.thresholdRatio)}
                tip="Fraction of in-window buckets where the deviation stayed at/above the entry level — the persistence gate."
              />
              <DetailField
                label="Coverage"
                value={fmtPct(r.coverage)}
                tip="Fraction of expected window buckets where both venues were actually observed — the data-coverage gate."
              />
              <DetailField
                label="Signal age"
                value={fmtDur(r.signalAgeSecs)}
                title={`first seen ${fmtTs(r.firstSeenMs)}`}
              />
              <DetailField
                label="Time to converge"
                value={fmtDur(r.expectedConvergeSecs)}
                tip="Expected time for the deviation to revert inside the band, from OU / episode statistics."
              />
            </DetailSection>

            <DetailSection title="Outcome evidence">
              <DetailField
                label="Win probability"
                value={fmtPct(getWin(r))}
                tip={`Directional P(trade is profitable) after edge direction, profitable capacity, funding, exit depth, persistence, coverage, variation and session gates. 0% when this direction has no edge or is hard-blocked.${
                  r.winProbabilityLower != null && r.winProbabilityUpper != null
                    ? ` 90% CI [${fmtPct(r.winProbabilityLower)} – ${fmtPct(r.winProbabilityUpper)}]` +
                      (r.pWinNEffective != null ? ` on ~${r.pWinNEffective.toFixed(0)} effective episodes` : "") +
                      (r.pWinCensored != null && r.pWinCensored > 0 ? `; ${r.pWinCensored} episodes still open (censored)` : "") +
                      (r.pWinFromModel ? "; OU model estimate, not empirical" : "")
                    : ""
                }`}
              />
              <DetailField
                label="Net win probability"
                value={fmtPct(r.netWinProbability)}
                tip={`Empirical P(net > 0): every historical excursion at this deviation replayed through the exit policy and scored on real net bps.${
                  r.netWinLower != null ? ` Wilson lower bound ${fmtPct(r.netWinLower)} (censored = losses)` : ""
                }${r.outcomeTrials != null ? ` · ${r.outcomeTrials} trials` : ""}${
                  r.outcomeCensored != null && r.outcomeCensored > 0 ? ` · ${r.outcomeCensored} censored` : ""
                }`}
              />
              <DetailField
                label="Expected net"
                value={r.netExpectedBps != null ? fmtBpsU(r.netExpectedBps) : "—"}
                tip={
                  r.netDecilesBps && r.netDecilesBps.length === 11
                    ? `Net-bps deciles of simulated entries at this deviation: P10 ${fmtBpsU(
                        r.netDecilesBps[1]
                      )} · P50 ${fmtBpsU(r.netDecilesBps[5])} · P90 ${fmtBpsU(r.netDecilesBps[9])}`
                    : "Recency-weighted mean net bps per simulated entry"
                }
              />
              <DetailField
                label="Reversion probability"
                value={fmtPct(getReversion(r))}
                tip="Direction-insensitive P(|deviation| reverts before the stop band). Informational only — high reversion with negative deviation still means 0% trade win for this direction."
              />
              <DetailField
                label="Median hold"
                value={r.medianHoldSecs != null ? fmtDur(r.medianHoldSecs) : "—"}
                tip={
                  r.medianWinSecs != null
                    ? `Median hold across simulated episodes; ${fmtDur(r.medianWinSecs)} among profitable ones — capital-turnover context`
                    : "Median hold across simulated episodes"
                }
              />
            </DetailSection>

            <DetailSection title="Sizing & PnL">
              <DetailField
                label="Expected profit (signed PnL)"
                value={fmtBpsU(r.expectedProfitBps)}
                tip="Signed probability-weighted PnL per unit. Negative = expected loss — never floored to 0."
              />
              <DetailField label="Exp profit $" value={fmtUsd(r.expectedProfitUsd)} />
              <DetailField
                label="Optimal size"
                value={fmtUsd(r.optimalSizeUsd)}
                tip="Size that maximizes expected net edge given visible book depth on both legs."
              />
              <DetailField
                label="Breakeven size"
                value={fmtUsd(r.breakevenSizeUsd)}
                tip="Largest size before expected net bps turns negative — depth ceiling for this route."
              />
            </DetailSection>

            {r.blockReason && (
              <div className="p-3 bg-warn-dim border border-warn-border rounded-lg text-xs font-sans text-warn">
                {r.blockReason}
              </div>
            )}

            <div className="space-y-2">
              <div className="text-[11px] font-sans font-bold uppercase tracking-wider text-slate-400">
                Capacity ladder
              </div>
              <CapacityLadder row={r} />
            </div>
          </div>

          <div className="px-6 py-3.5 border-t border-surface-border bg-surface flex items-center justify-end gap-2 shrink-0">
            <input
              className="w-28 bg-surface border border-surface-border rounded-lg px-2 py-1.5 text-right text-xs text-slate-800 focus:outline-none focus:border-primary focus:ring-2 focus:ring-primary/15 transition-all shadow-2xs font-sans tabular-nums"
              type="number"
              min={0}
              placeholder={fmtUsd(r.optimalSizeUsd ?? 1000)}
              value={sizeValue ?? ""}
              onChange={(e) => onSizeChange(rowKey, Number(e.target.value))}
            />
            <button
              className="btn-primary px-2.5 py-2.5 text-xs rounded-lg shadow-2xs cursor-pointer"
              disabled={!r.executable || executing}
              title={r.executable ? "execute paper trade" : r.blockReason ?? "blocked"}
              onClick={() => onExecute(r)}
            >
              {executing ? "…" : "Exec"}
            </button>
          </div>
        </div>
      </div>
    );
  },
);

interface RadarRowProps {
  row: MonitorRowDto;
  rowKey: string;
  held: boolean;
  sizeValue: number | undefined;
  executing: boolean;
  onOpenDetail: (rowKey: string) => void;
  onSizeChange: (rowKey: string, value: number) => void;
  onExecute: (row: MonitorRowDto) => void;
}

const RadarRow: React.FC<RadarRowProps> = React.memo(
  ({ row: r, rowKey: k, held, sizeValue, executing, onOpenDetail, onSizeChange, onExecute }) => {
    return (
      <React.Fragment>
        <tr
          className="border-b border-surface-border/60 hover:bg-blue-50/25 transition-colors cursor-pointer"
          onClick={() => onOpenDetail(k)}
          title="Open signal details"
        >
          <td className="px-2 py-2 text-slate-400">
            <ChevronRight size={12} />
          </td>
          <td className="px-2 py-2 font-bold font-sans text-xs text-slate-900">
            {r.symbol}
            {held && (
              <span
                className="ml-1.5 px-1.5 py-0.2 rounded-full border border-profit-border bg-profit-dim text-profit text-[10px] font-mono font-bold"
                title="Bot-managed convergence position open on this pair"
              >
                bot
              </span>
            )}
          </td>
          <td className="px-2 py-2">
            <span className="inline-flex items-center gap-1 text-slate-700 font-mono text-xs whitespace-nowrap">
              <VenueLogo venueId={r.buyVenue} name={VENUE_NAME[r.buyVenue] ?? r.buyVenue} size={14} />
              {VENUE_LABEL[r.buyVenue] ?? r.buyVenue}→
              <VenueLogo venueId={r.sellVenue} name={VENUE_NAME[r.sellVenue] ?? r.sellVenue} size={14} />
              {VENUE_LABEL[r.sellVenue] ?? r.sellVenue}
            </span>
            {!isLiveCapableRoute(r.buyVenue, r.sellVenue) && (
              <span
                className="ml-1.5 px-1.5 py-0.2 rounded-full border border-surface-border bg-surface-raised text-slate-500 text-[10px] font-mono"
                title="Route involves a read-only venue — paper execution only"
              >
                PAPER
              </span>
            )}
          </td>
          <td
            className={`px-2 py-2 text-right font-mono tabular-nums font-bold ${
              (r.expectedProfitBps ?? 0) >= 0 ? "text-profit" : "text-loss"
            }`}
            title="Signed probability-weighted PnL expectation (negative = expected loss)"
          >
            {fmtBpsU(r.expectedProfitBps)}
          </td>
          <td className="px-2 py-2 text-right font-mono tabular-nums">{fmtBpsU(r.netCaptureBps)}</td>
          <td className="px-2 py-2 text-right text-slate-500 font-mono tabular-nums">
            {fmtBpsU(r.baselineBps)}
          </td>
          <td
            className={`px-2 py-2 text-right font-mono tabular-nums ${
              (r.deviationBps ?? 0) >= 0 ? "text-profit font-bold" : "text-loss font-bold"
            }`}
          >
            {fmtBpsU(r.deviationBps)}
          </td>
          <td
            className="px-2 py-2 text-right font-mono"
            title={`Directional trade win probability — 0% when this direction has no edge or is blocked.${
              r.winProbabilityLower != null && r.winProbabilityUpper != null
                ? ` 90% CI [${fmtPct(r.winProbabilityLower)} – ${fmtPct(r.winProbabilityUpper)}]` +
                  (r.pWinNEffective != null ? ` on ~${r.pWinNEffective.toFixed(0)} eff. episodes` : "") +
                  (r.pWinFromModel ? " (OU model fallback — no empirical episodes)" : "")
                : ""
            } See Reversion Probability in details.`}
          >
            {fmtPct(getWin(r))}
          </td>
          <td
            className="px-2 py-2 text-right font-mono"
            title={`P(net > 0) for entry at the current deviation — excursions replayed through the exit policy.${
              r.netWinLower != null ? ` Wilson lower ${fmtPct(r.netWinLower)} (censored count as losses)` : ""
            }${r.outcomeTrials != null ? ` · ${r.outcomeTrials} trials` : ""}${
              r.outcomeCensored != null && r.outcomeCensored > 0 ? ` · ${r.outcomeCensored} still open` : ""
            }${r.netExpectedBps != null ? ` · E[net] ${fmtBpsU(r.netExpectedBps)}` : ""}`}
          >
            {fmtPct(r.netWinProbability)}
          </td>
          <td className="px-2 py-2 text-right font-mono" title="Fraction of in-window buckets at/above entry (persistence gate)">
            {fmtPct(r.thresholdRatio)}
          </td>
          <td className="px-2 py-2 text-right font-mono" title="Fraction of expected window buckets with both venues observed (coverage gate)">
            {fmtPct(r.coverage)}
          </td>
          <td className="px-2 py-2 text-right font-mono">{fmtUsd(r.expectedProfitUsd)}</td>
          <td className="px-2 py-2 text-right font-mono">{fmtUsd(r.optimalSizeUsd)}</td>
          <td className="px-2 py-2 text-right font-mono">{fmtUsd(r.breakevenSizeUsd)}</td>
          <td className="px-2 py-2 text-right font-mono" title={`first seen ${fmtTs(r.firstSeenMs)}`}>
            {fmtDur(r.signalAgeSecs)}
          </td>
          <td className="px-2 py-2 text-right font-mono">{fmtDur(r.expectedConvergeSecs)}</td>
          <td className="px-2 py-2">
            <span
              className={`px-2 py-0.5 rounded-full border text-[11px] font-sans font-semibold ${STATUS_STYLE[r.status]}`}
              title={
                r.status === "preparing"
                  ? `history ${(r.qualificationProgress * 100).toFixed(0)}% of window · ${r.observedEpisodes} episodes`
                  : r.blockReason ?? r.status
              }
            >
              {r.status}
              {r.status === "preparing" &&
                ` ${(r.qualificationProgress * 100).toFixed(0)}%`}
            </span>
          </td>
          <td className="px-2 py-2 text-right" onClick={(e) => e.stopPropagation()}>
            <div className="flex items-center justify-end gap-1.5">
              <input
                className="w-16 bg-surface border border-surface-border rounded-lg px-2 py-1 text-right text-xs text-slate-800 focus:outline-none focus:border-primary focus:ring-2 focus:ring-primary/15 transition-all shadow-2xs font-mono tabular-nums"
                type="number"
                min={0}
                placeholder={fmtUsd(r.optimalSizeUsd ?? 1000)}
                value={sizeValue ?? ""}
                onChange={(e) => onSizeChange(k, Number(e.target.value))}
              />
              <button
                className="btn-primary px-2 py-2 text-xs rounded-lg shadow-2xs cursor-pointer"
                disabled={!r.executable || executing}
                title={r.executable ? "execute paper trade" : r.blockReason ?? "blocked"}
                onClick={() => onExecute(r)}
              >
                {executing ? "…" : "Exec"}
              </button>
            </div>
          </td>
        </tr>
      </React.Fragment>
    );
  },
);

/* Auto-bot status gadget lives in ./AutoBotGadget (single radar bot
   control; backend BotMode is the source of truth). */

/* ------------------------------------------------------------------ */
/* Main component                                                      */
/* ------------------------------------------------------------------ */

const OpportunityRadarInner: React.FC = () => {
  const [snap, setSnap] = useState<MonitorSnapshotDto | null>(null);
  const [cfg, setCfg] = useState<MonitorConfigDto | null>(null);
  const [showConfig, setShowConfig] = useState(false);
  const [search, setSearch] = useState("");
  const [venueFilters, setVenueFilters] = useState<string[]>([]);
  // Filter inputs show at most 2 decimal places (bot-mirrored defaults are
  // normalized; user typing is never reformatted mid-edit).
  const to2dpStr = (n: number): string =>
    Number.isFinite(n) ? String(parseFloat(n.toFixed(2))) : "";
  const [minExpBps, setMinExpBps] = useState(() => to2dpStr(defaultRadarBotConfig().minExpectedProfitBps)); // default mirrors the bot entry hurdle (bot minExpectedProfitBps)
  const minExpTouched = useRef(false);
  const [minPWin, setMinPWin] = useState("95"); // default mirrors the monitor ready floor (minWinProbability 0.95)
  const [showVenueMenu, setShowVenueMenu] = useState(false);
  const venueMenuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (venueMenuRef.current && !venueMenuRef.current.contains(event.target as Node)) {
        setShowVenueMenu(false);
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);
  const [detailKey, setDetailKey] = useState<string | null>(null);
  const [executing, setExecuting] = useState<string | null>(null);
  const [execError, setExecError] = useState<string | null>(null);
  const [execSuccess, setExecSuccess] = useState<string | null>(null);
  const [sizeUsd, setSizeUsd] = useState<Record<string, number>>({});
  // Autonomous convergence bot (paper-only, opt-in). Backend BotMode is the
  // single source of truth — no page-local lifecycle phase.
  const [botCfg, setBotCfg] = useState<RadarBotConfigDto | null>(null);
  const [botStatus, setBotStatus] = useState<RadarBotStatusDto | null>(null);
  const [botError, setBotError] = useState<string | null>(null);
  const [flattening, setFlattening] = useState(false);
  const [flattenError, setFlattenError] = useState<string | null>(null);
  // Durable radar signal ledger backing the bot stat cards (null = unread).
  const [signals, setSignals] = useState<SignalExecutionDto[] | null>(null);
  const [fillPnl, setFillPnl] = useState<StrategyFillPnl | null>(null);

  // Poll the snapshot while the tab is mounted.
  useEffect(() => {
    let dead = false;
    const tick = async () => {
      try {
        const s = await api.getMonitorSnapshot();
        if (!dead) setSnap(s);
      } catch {
        /* engine not running yet */
      }
      try {
        const st = await api.getRadarBotStatus();
        if (!dead) setBotStatus(st);
      } catch {
        /* bot task not installed / backend down — gadget keeps last state */
      }
      try {
        const sigs = await api.listSignalExecutions();
        if (!dead) setSignals(sigs);
      } catch {
        /* ledger unreadable — stat cards render unknown, never zero */
      }
      try {
        const pnl = await api.getStrategyFillPnl("radar");
        if (!dead) setFillPnl(pnl);
      } catch {
        /* attributed PnL unavailable — profit cell keeps the "—" fallback */
      }
    };
    tick();
    const id = setInterval(tick, 5000);
    return () => {
      dead = true;
      clearInterval(id);
    };
  }, []);

  useEffect(() => {
    api.getMonitorConfig().then(setCfg).catch(() => {});
    api.getRadarBotConfig().then(setBotCfg).catch(() => {});
  }, []);

  // Keep the table Min Exp display filter aligned with the bot's live
  // minExpectedProfitBps until the user overrides it locally.
  useEffect(() => {
    if (!minExpTouched.current && botCfg && Number.isFinite(botCfg.minExpectedProfitBps)) {
      setMinExpBps(to2dpStr(botCfg.minExpectedProfitBps));
    }
  }, [botCfg]);

  const handleBotMode = useCallback(
    async (mode: RadarBotConfigDto["mode"]) => {
      const base = botCfg ?? defaultRadarBotConfig();
      const next: RadarBotConfigDto = {
        ...base,
        enabled: mode === "auto",
        mode,
      };
      setBotError(null);
      try {
        await api.setRadarBotConfig(next);
        setBotCfg(next);
      } catch (e) {
        setBotError(e instanceof Error ? e.message : String(e));
      }
    },
    [botCfg],
  );

  const handleFlatten = useCallback(async () => {
    setFlattening(true);
    setFlattenError(null);
    try {
      const res = await api.flattenTickerPositions(null, "radar");
      const residual = flattenResidualMessage(res);
      if (residual !== null) setFlattenError(residual);
    } catch (e) {
      setFlattenError(e instanceof Error ? e.message : String(e));
    } finally {
      setFlattening(false);
    }
  }, []);

  const cycleStats = useMemo(() => computeBotCycleStats(signals, "radar", fillPnl), [signals, fillPnl]);

  // Signals the reconciler escalated or could not prove flat — surfaced
  // prominently because each one may hold residual venue exposure and
  // blocks its ticker until an operator reviews it.
  const attentionSignals = useMemo(
    () =>
      (signals ?? []).filter(
        (s) => s.status === "unresolved" || s.status === "imbalanced",
      ),
    [signals],
  );

  const heldSet = useMemo(
    () => buildHeldSet(botStatus?.openPositions),
    [botStatus],
  );

  const venues = useMemo(
    () => Array.from(new Set((snap?.rows ?? []).flatMap((r) => [r.buyVenue, r.sellVenue]))).sort(),
    [snap],
  );

  const rows = useMemo(() => {
    // NOTE (display-vs-engine split): this table is filtered ONLY by the gadget
    // strip controls below (search / venues / Min Exp / Min Win). The old
    // persisted `minExpectedProfitBps` table floor was removed from the
    // Configurator because it never reached the engine — backend only clamps
    // it — so it was a display filter in disguise. One owner, one place.
    let out = snap?.rows ?? [];
    const q = search.trim().toUpperCase();
    if (q) out = out.filter((r) => r.symbol.includes(q));
    if (venueFilters.length)
      out = out.filter(
        (r) => venueFilters.includes(r.buyVenue) || venueFilters.includes(r.sellVenue),
      );
    const me = Number(minExpBps);
    if (Number.isFinite(me) && minExpBps !== "")
      out = out.filter((r) => (r.expectedProfitBps ?? -Infinity) >= me);
    const mp = Number(minPWin);
    if (Number.isFinite(mp) && minPWin !== "")
      out = out.filter((r) => (getWin(r) ?? -Infinity) >= mp / 100);
    return out;
  }, [snap, search, venueFilters, minExpBps, minPWin]);

  const key = useCallback((r: MonitorRowDto) => `${r.symbol}:${r.buyVenue}:${r.sellVenue}`, []);

  const handleExecute = useCallback(
    async (r: MonitorRowDto) => {
      const k = key(r);
      const size = resolveRadarExecSize(sizeUsd[k], r.optimalSizeUsd);
      if (size === null) {
        setExecError(
          `${r.symbol} ${r.buyVenue}→${r.sellVenue}: no executable size — backend optimal size unavailable`,
        );
        return;
      }
      setExecuting(k);
      setExecError(null);
      setExecSuccess(null);
      try {
        await api.executeMonitorOpportunity(r.symbol, r.buyVenue, r.sellVenue, size);
        setExecSuccess(
          `${r.symbol} ${r.buyVenue}→${r.sellVenue}: Executed successfully — position adopted into auto-exit watcher (targeting dynamic baseline)`,
        );
        api.getRadarBotConfig().then(setBotCfg).catch(() => {});
        api.getRadarBotStatus().then(setBotStatus).catch(() => {});
      } catch (e) {
        setExecError(`${r.symbol} ${r.buyVenue}→${r.sellVenue}: ${String(e)}`);
      } finally {
        setExecuting(null);
      }
    },
    [key, sizeUsd],
  );

  const handleOpenDetail = useCallback((k: string) => {
    setDetailKey(k);
  }, []);

  const handleCloseDetail = useCallback(() => {
    setDetailKey(null);
  }, []);

  const handleSizeChange = useCallback((k: string, value: number) => {
    setSizeUsd((m) => ({ ...m, [k]: value }));
  }, []);

  return (
    <div className="h-full flex flex-col bg-background text-slate-700 overflow-hidden p-4 space-y-4">
      {showConfig && cfg && (
        <ConfigPanel
          cfg={cfg}
          botCfg={botCfg ?? defaultRadarBotConfig()}
          onSave={async (c, b) => {
            await Promise.all([
              api.setMonitorConfig(c),
              api.setRadarBotConfig(b),
            ]);
            setCfg(c);
            setBotCfg(b);
          }}
          onClose={() => setShowConfig(false)}
        />
      )}

      {/* Strategy Ribbon — read-only params left, configurator + lifecycle right */}
      <div className="px-4 py-3 flex flex-wrap items-center justify-between gap-3 bg-surface rounded-lg border border-surface-border shadow-2xs shrink-0">
        <div className="flex flex-wrap items-center gap-2 text-xs font-mono">
          <span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1">
            Strategy Parameters
          </span>
          <div
            className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-lg"
            title="Qualification window mode — edit via Configurator"
          >
            <span className="text-slate-400">Qualification</span>
            <span className="font-bold text-slate-900">{cfg?.qualificationMode ?? "—"}</span>
          </div>
          <div
            className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-lg"
            title="Engine floor on directional trade win probability — the executable gate also requires the Wilson lower bound to clear a confidence floor. Edit via Configurator."
          >
            <span className="text-slate-400">Min Win</span>
            <span className="font-bold text-slate-900">
              {cfg ? `${(cfg.minWinProbability * 100).toFixed(0)}` : "—"}
            </span>
            <span className="text-slate-400">%</span>
          </div>
          <div
            className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-lg"
            title="Execution mode — edit via Configurator"
          >
            <span className="text-slate-400">Execution</span>
            <span className="font-bold text-slate-900">
              {cfg ? (cfg.executionMode === "two_way" ? "Two-Way" : "Single-Way") : "—"}
            </span>
          </div>
        </div>

        <div className="flex items-center space-x-3 shrink-0">
          {(execError || flattenError) && (
            <span
              className="text-xs font-mono text-loss max-w-72 truncate"
              title={execError ?? flattenError ?? ""}
            >
              {execError ?? flattenError}
            </span>
          )}
          {execSuccess && !execError && !flattenError && (
            <span
              className="text-xs font-mono text-profit max-w-96 truncate"
              title={execSuccess}
            >
              {execSuccess}
            </span>
          )}
          <button
            type="button"
            onClick={() => setShowConfig(true)}
            className="btn-secondary !text-xs !py-2 !px-2 font-sans cursor-pointer gap-2"
            title="Configure Spread Radar monitor parameters"
          >
            <SlidersHorizontal className="w-3.5 h-3.5 text-accent" />
            <span>Configurator</span>
          </button>

          <BotLifecycleControls
            phase={
              flattening
                ? "stopping"
                : botCfg?.mode === "auto" && botCfg.enabled
                ? "running"
                : botCfg?.mode === "paused"
                ? "paused"
                : botCfg?.mode === "reduce_only"
                ? "reduce_only"
                : "idle"
            }
            hasOpenPositions={(botStatus?.openPositions ?? []).length > 0}
            disabled={botControlsDisabled(botCfg)}
            onStart={() => void handleBotMode("auto")}
            onPause={() => void handleBotMode("paused")}
            onReduceOnly={() => void handleBotMode("reduce_only")}
            onStop={() => void handleFlatten()}
          />
          {botError && (
            <span className="text-xs font-mono text-loss max-w-56 truncate" title={botError}>
              {botError}
            </span>
          )}
        </div>
      </div>

      <BotStatCards stats={cycleStats} />

      {attentionSignals.length > 0 && (
        <div
          className="bg-loss-dim border border-loss-border rounded-xl px-4 py-3 space-y-1.5 shrink-0"
          role="alert"
        >
          <div className="text-xs font-mono font-bold text-loss">
            {attentionSignals.length} signal{attentionSignals.length === 1 ? "" : "s"} need
            operator review — exposure may be open
          </div>
          {attentionSignals.slice(0, 5).map((s) => (
            <div key={s.id} className="space-y-0.5">
              <div className="flex items-center gap-2 text-[11px] font-mono">
                <span
                  className={`px-1.5 py-0.5 rounded border text-[10px] font-bold uppercase ${
                    s.status === "imbalanced"
                      ? "bg-loss/15 text-loss border-loss-border"
                      : "bg-warn-dim text-warn border-warn-border"
                  }`}
                >
                  {s.status}
                </span>
                <span className="text-slate-700 font-bold">{s.symbol}</span>
                <span className="text-slate-400">{s.id}</span>
                {s.note && (
                  <span className="text-slate-500 truncate max-w-md" title={s.note}>
                    {s.note}
                  </span>
                )}
                <button
                  className="ml-auto px-1.5 py-0.5 rounded border border-slate-600 text-slate-400 hover:bg-slate-700/40 transition cursor-pointer"
                  title="After checking both venues yourself: unresolved → mark failed (nothing executed); imbalanced → mark closed (you flattened the residual manually)"
                  onClick={async () => {
                    const resolution = s.status === "imbalanced" ? "closed" : "failed";
                    const ok = window.confirm(
                      s.status === "imbalanced"
                        ? "Mark this imbalanced signal CLOSED? Only do this after verifying the residual exposure has been flattened on the venue."
                        : "Mark this unresolved signal FAILED? Only do this after verifying no working orders or open exposure remain on either venue.",
                    );
                    if (!ok) return;
                    try {
                      await api.acknowledgeSignal(s.id, resolution);
                    } catch (e) {
                      console.error("Acknowledge signal failed:", e);
                    }
                  }}
                >
                  acknowledge
                </button>
              </div>
              {s.legs
                .filter((l) => l.venueOrderId && l.filledQty < l.intendedQty)
                .map((l) => (
                  <div key={l.id} className="flex items-center gap-2 pl-4 text-[10px] font-mono">
                    <span className="text-slate-500">
                      {l.venue} {l.side} {l.filledQty}/{l.intendedQty} · order {l.venueOrderId}
                    </span>
                    <button
                      className="px-1.5 py-0.5 rounded border border-warn-border text-warn hover:bg-warn-dim transition cursor-pointer"
                      title="Send a venue cancel for this order, then re-poll — risk-reducing only"
                      onClick={async () => {
                        try {
                          await api.cancelVenueOrder(
                            l.venue,
                            l.symbol,
                            l.venueOrderId as string,
                            l.clientOrderId ?? undefined,
                          );
                          await api.reconcileOrdersNow();
                        } catch (e) {
                          console.error("Cancel venue order failed:", e);
                        }
                      }}
                    >
                      cancel order
                    </button>
                  </div>
                ))}
            </div>
          ))}
          {attentionSignals.length > 5 && (
            <div className="text-[10px] font-mono text-slate-500">
              …and {attentionSignals.length - 5} more in the signal ledger
            </div>
          )}
        </div>
      )}

      {/* Radar gadget — filter strip + table in a single card.
          Filters are display-only; none of these affect the monitor or strategy. */}
      <div className="flex-1 min-h-0 flex flex-col bg-surface rounded-xl border border-surface-border shadow-card overflow-hidden">
        <div className="flex items-center gap-2.5 px-4 py-2.5 border-b border-surface-border text-xs font-sans shrink-0 bg-surface-raised/40">
          <div
            className="flex items-center gap-1.5 bg-surface border border-surface-border rounded-lg px-2.5 py-1.5 shadow-2xs hover:border-slate-300 focus-within:border-primary focus-within:ring-2 focus-within:ring-primary/15 transition-all"
            title="Display filter — symbol contains"
          >
            <Search size={13} className="text-slate-400 shrink-0" />
            <span className="text-slate-400 font-medium">Symbol</span>
            <input
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="ALL"
              className="bg-transparent w-24 outline-none font-bold text-slate-900 uppercase font-mono text-xs placeholder:text-slate-300"
            />
          </div>
          <div className="relative" ref={venueMenuRef}>
            <button
              type="button"
              onClick={() => setShowVenueMenu((v) => !v)}
              className="flex items-center gap-1.5 bg-surface border border-surface-border rounded-lg px-2 py-2 font-sans text-xs transition-all cursor-pointer shadow-2xs hover:border-slate-300 hover:bg-slate-50/80"
              title="Display filter — shows only rows with a selected venue on either leg. Does not affect the monitor or strategy"
            >
              <span className="text-slate-400 font-medium">Venue</span>
              {venueFilters.length > 0 && (
                <span className="flex items-center -space-x-1">
                  {venueFilters.slice(0, 4).map((v) => (
                    <VenueLogo key={v} venueId={v} name={VENUE_NAME[v] ?? v} size={16} />
                  ))}
                </span>
              )}
              <span className="font-bold text-slate-900 font-mono text-xs">
                {venueFilters.length === 0
                  ? "ALL"
                  : venueFilters.map((v) => VENUE_LABEL[v] ?? v).join("+")}
              </span>
              <ChevronDown
                className={`w-3.5 h-3.5 text-slate-400 transition-transform ${showVenueMenu ? "rotate-180" : ""}`}
              />
            </button>
            {showVenueMenu && (
              <div className="absolute left-0 top-full mt-1.5 w-52 bg-surface border border-surface-border rounded-xl shadow-elevated p-1.5 z-30 font-sans text-xs animate-in fade-in zoom-in-95 duration-100">
                <button
                  type="button"
                  onClick={() => setVenueFilters([])}
                  className={`w-full flex items-center justify-between px-2 py-2 text-left transition rounded-lg cursor-pointer ${
                    venueFilters.length === 0
                      ? "bg-accent/10 text-accent font-semibold"
                      : "text-slate-700 hover:bg-surface-raised hover:text-slate-900"
                  }`}
                >
                  <span>All</span>
                  {venueFilters.length === 0 && <Check className="w-3.5 h-3.5" />}
                </button>
                <div className="border-t border-surface-border/60 my-1 -mx-1" />
                {venues.map((v) => {
                  const on = venueFilters.includes(v);
                  return (
                    <button
                      key={v}
                      type="button"
                      onClick={() =>
                        setVenueFilters((prev) =>
                          prev.includes(v) ? prev.filter((x) => x !== v) : [...prev, v],
                        )
                      }
                      className={`w-full flex items-center justify-between px-2 py-2 text-left transition rounded-lg cursor-pointer ${
                        on
                          ? "bg-accent/10 text-accent font-semibold"
                          : "text-slate-700 hover:bg-surface-raised hover:text-slate-900"
                      }`}
                    >
                      <span className="flex items-center gap-2">
                        <VenueLogo venueId={v} name={VENUE_NAME[v] ?? v} size={18} />
                        <span className="font-mono text-xs">{VENUE_LABEL[v] ?? v}</span>
                      </span>
                      {on && <Check className="w-3.5 h-3.5" />}
                    </button>
                  );
                })}
              </div>
            )}
          </div>
          <div
            className="flex items-center gap-1.5 bg-surface border border-surface-border rounded-lg px-2.5 py-1.5 shadow-2xs hover:border-slate-300 focus-within:border-primary focus-within:ring-2 focus-within:ring-primary/15 transition-all"
            title="Display filter — minimum expected profit"
          >
            <span className="text-slate-400 font-medium">Min Exp</span>
            <input
              value={minExpBps}
              onChange={(e) => {
                minExpTouched.current = true;
                setMinExpBps(e.target.value);
              }}
              placeholder={String(defaultRadarBotConfig().minExpectedProfitBps)}
              className="bg-transparent w-14 outline-none text-right font-bold text-slate-900 font-mono text-xs"
            />
            <span className="text-slate-400 text-[11px] font-medium">BPS</span>
          </div>
          <div
            className="flex items-center gap-1.5 bg-surface border border-surface-border rounded-lg px-2.5 py-1.5 shadow-2xs hover:border-slate-300 focus-within:border-primary focus-within:ring-2 focus-within:ring-primary/15 transition-all"
            title="Display filter — minimum directional trade win probability (reversion-based, gated). Net % column shows the P&L-defined rate; both render '—' when history is thin."
          >
            <span className="text-slate-400 font-medium">Min Win</span>
            <input
              value={minPWin}
              onChange={(e) => setMinPWin(e.target.value)}
              placeholder="0"
              className="bg-transparent w-14 outline-none text-right font-bold text-slate-900 font-mono text-xs"
            />
            <span className="text-slate-400 text-[11px] font-medium">%</span>
          </div>
          <div className="ml-auto flex items-center gap-2">
            {(snap?.exempted.length ?? 0) > 0 && (
              <span className="text-slate-500 flex items-center gap-1 text-[11px] font-sans">
                <EyeOff size={12} /> {snap?.exempted.length} hidden
              </span>
            )}
          </div>
        </div>

        <div className="flex-1 overflow-auto min-h-0">
          <table className="w-full text-xs">
            <thead className="sticky top-0 z-10 bg-slate-50/95 backdrop-blur-xs text-slate-500 font-sans text-[11px] uppercase tracking-wider font-semibold">
              <tr className="border-b border-surface-border text-left">
                <th className="px-2 py-2 w-6" />
                <th className="px-2 py-2">Symbol</th>
                <th className="px-2 py-2" title="Buy venue → sell venue">Route</th>
              <th
                className="px-2 py-1.5 text-right"
                title="Signed probability-weighted PnL expectation, bps (negative = expected loss)"
              >
                Exp Profit
              </th>
              <th className="px-2 py-1.5 text-right" title="Edge after fees and crossing friction">
                Net Profit
              </th>
              <th className="px-2 py-1.5 text-right" title="Historical baseline (mean) of the pair edge">
                Baseline
              </th>
              <th className="px-2 py-1.5 text-right" title="Signed deviation of current edge from baseline">
                Deviation
              </th>
              <th
                className="px-2 py-1.5 text-right"
                title="Directional trade win probability after all gates (0% when this direction has no edge or is blocked). Measured on spread reversion, not P&L — Net% is the P&L-defined rate. Reversion Probability lives in the detail card."
              >
                Win %
              </th>
              <th
                className="px-2 py-1.5 text-right"
                title="Empirical P(net > 0): historical excursions at this deviation replayed through the exit policy and scored on real net bps. Tooltip shows the Wilson 90% interval and censored share — wide interval = thin evidence."
              >
                Net %
              </th>
              <th className="px-2 py-1.5 text-right" title="Share of in-window buckets holding at/above entry (persistence)">
                Persist
              </th>
              <th className="px-2 py-1.5 text-right" title="Share of expected window buckets with both venues observed (coverage)">
                Cover
              </th>
              <th className="px-2 py-1.5 text-right" title="Probability-weighted expected profit, USD">
                Exp Profit $
              </th>
              <th className="px-2 py-1.5 text-right" title="Size maximizing expected USD profit given both books' depth">
                Opt Size $
              </th>
              <th className="px-2 py-1.5 text-right" title="Largest size still clearing the minimum win-probability floor">
                Max Size $
              </th>
              <th className="px-2 py-1.5 text-right" title="Time since the signal was first observed">
                Signal Age
              </th>
              <th className="px-2 py-1.5 text-right" title="Expected time for the spread to revert to baseline">
                Time to Converge
              </th>
              <th className="px-2 py-1.5">Status</th>
              <th className="px-2 py-1.5 text-right">Action</th>
            </tr>
          </thead>
          <tbody>
            {rows.length === 0 && (
              <tr>
                <td colSpan={18}>
                  {snap == null ? (
                    <EmptyState pulse icon={Activity} title="Monitor Warming Up" hint="Collecting spread samples…" />
                  ) : snap.watchlist.length === 0 ? (
                    <EmptyState title="No Spread-Eligible Tickers" hint="Requires ≥2 venues per ticker." />
                  ) : (
                    <EmptyState plain title="No Opportunity Detected" />
                  )}
                </td>
              </tr>
            )}
            {rows.map((r) => {
              const k = key(r);
              return (
                <RadarRow
                  key={k}
                  row={r}
                  rowKey={k}
                  held={isRowHeld(r, heldSet)}
                  sizeValue={sizeUsd[k]}
                  executing={executing === k}
                  onOpenDetail={handleOpenDetail}
                  onSizeChange={handleSizeChange}
                  onExecute={handleExecute}
                />
              );
            })}
          </tbody>
        </table>
      </div>
      </div>
      {(() => {
        const detailRow = detailKey != null ? rows.find((r) => key(r) === detailKey) ?? null : null;
        if (!detailRow) return null;
        const k = key(detailRow);
        return (
          <SignalDetailModal
            row={detailRow}
            held={isRowHeld(detailRow, heldSet)}
            sizeValue={sizeUsd[k]}
            executing={executing === k}
            onSizeChange={handleSizeChange}
            onExecute={handleExecute}
            onClose={handleCloseDetail}
          />
        );
      })()}
    </div>
  );
};

export const OpportunityRadar: React.FC = React.memo(OpportunityRadarInner);
