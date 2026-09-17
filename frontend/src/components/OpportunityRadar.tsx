import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Activity,
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
import { AutoBotGadget } from "./AutoBotGadget";
import { BotStatCards } from "./BotStatCards";
import { InfoTooltip } from "./InfoTooltip";
import { SessionBadge } from "./SessionBadge";
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
  defaultRadarBotConfig,
  isRowHeld,
  resolveRadarExecSize,
} from "../services/radarBot";
import { computeBotCycleStats } from "../services/botCycleStats";
import { flattenResidualMessage } from "../services/flatten";
import { EmptyState } from "./EmptyState";

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

const STATUS_STYLE: Record<string, string> = {
  qualified: "bg-emerald-50 text-emerald-700 border-emerald-300",
  signaled: "bg-amber-50 text-amber-700 border-amber-300",
  watching: "bg-sky-50 text-sky-700 border-sky-300",
  collecting: "bg-slate-100 text-slate-600 border-slate-300",
  stale: "bg-rose-50 text-rose-700 border-rose-300",
};

const VENUE_LABEL: Record<string, string> = {
  hyperliquid: "HL",
  lighter: "LGT",
  robinhood: "RH",
  vanta: "VNT",
  aster: "AST",
  ondoperps: "ONDO",
  grvt: "GRVT",
  edgex: "EDGX",
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
  variational: "VAR",
};

const LIVE_CAPABLE_VENUES = new Set(["aster", "hyperliquid"]);
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
  { id: "signal", label: "Signal thresholds", tooltip: "Configure the edge, capacity, and win-probability floors a deviation must clear to surface.", icon: TrendingUp },
  { id: "execution", label: "Execution", tooltip: "Configure one-way versus two-way execution and the transaction reserve haircut.", icon: Layers },
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
  minCoverage: 0.5,
};

const INPUT_INLINE_CLS =
  "w-28 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold text-xs focus:outline-none focus:ring-1 focus:ring-accent h-[38px]";
const SELECT_INLINE_CLS =
  "w-44 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-medium text-xs focus:outline-none focus:ring-1 focus:ring-accent h-[38px]";
const CARD_CLS =
  "bg-surface p-3.5 rounded-lg border border-surface-border space-y-1.5 shadow-2xs";
const LABEL_CLS = "text-xs text-slate-700 font-medium flex items-center";

const ConfigPanel: React.FC<{
  cfg: MonitorConfigDto;
  onSave: (c: MonitorConfigDto) => Promise<void>;
  onClose: () => void;
}> = ({ cfg, onSave, onClose }) => {
  const [draft, setDraft] = useState<MonitorConfigDto>(cfg);
  const [saving, setSaving] = useState(false);
  const [saveSuccess, setSaveSuccess] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [activeSection, setActiveSection] = useState<string>(MONITOR_SECTIONS[0].id);
  const set = <K extends keyof MonitorConfigDto>(k: K, v: MonitorConfigDto[K]) => {
    setError(null);
    setDraft((d) => ({ ...d, [k]: v }));
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
  };

  const handleSave = async () => {
    setSaving(true);
    setSaveSuccess(false);
    setError(null);
    try {
      await onSave(draft);
      setSaveSuccess(true);
      setTimeout(onClose, 400);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 bg-slate-900/40 backdrop-blur-xs flex items-center justify-center p-3 sm:p-6 select-none animate-in fade-in duration-150"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="bg-surface border border-surface-border rounded-xl w-[960px] h-[680px] max-w-[96vw] max-h-[92vh] flex flex-col shadow-2xl overflow-hidden animate-in zoom-in-95 duration-150">
        {/* Modal Top Header Bar */}
        <div className="flex items-center justify-between px-6 py-3.5 border-b border-surface-border bg-surface shrink-0">
          <div className="flex items-center space-x-3">
            <div className="w-8 h-8 rounded-lg bg-accent-dim border border-accent-line flex items-center justify-center text-accent shadow-2xs">
              <SlidersHorizontal className="w-4 h-4" />
            </div>
            <div>
              <h2 className="text-[13px] font-bold font-mono tracking-wider text-slate-900 leading-tight">
                Spread Radar Configurator
              </h2>
            </div>
          </div>
          <div className="flex items-center space-x-3">
            <button
              type="button"
              onClick={onClose}
              className="p-1.5 rounded-md text-slate-400 hover:text-slate-700 hover:bg-surface-raised transition border border-transparent hover:border-surface-border cursor-pointer"
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
                      className={`w-full flex items-center space-x-2.5 px-3 py-2.5 rounded-lg font-mono text-[11px] transition cursor-pointer text-left ${
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
          <div className="flex-1 overflow-y-auto p-4 space-y-4 bg-surface">
            {activeSection === "monitor" && (
              <div className="space-y-3">
                <div className={CARD_CLS}>
                  <label className={`${LABEL_CLS} justify-between`}>
                    <span className="flex items-center">
                      Monitor enabled
                      <InfoTooltip text="Master switch for the spread monitor. When off, no pairs are watched and the radar table stays empty." />
                    </span>
                    <SwitchToggle
                      size="sm"
                      checked={draft.enabled}
                      onChange={(v) => set("enabled", v)}
                    />
                  </label>
                </div>
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
                      <InfoTooltip text="Minimum empirical convergence probability a pair must show to surface in the table." />
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
                      Min expected profit (bps)
                      <InfoTooltip text="Radar table floor — rows show only when expected profit exceeds this (0 = positive-only; negative shows all)." />
                    </span>
                  </label>
                  <input
                    className={INPUT_INLINE_CLS}
                    type="number"
                    step={0.5}
                    value={draft.minExpectedProfitBps}
                    onChange={(e) => set("minExpectedProfitBps", num(e.target.value, 0))}
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
                      <InfoTooltip text="One-way opens the spread only; two-way also prices the closing leg into expected profit." />
                    </span>
                  </label>
                  <select
                    className={SELECT_INLINE_CLS}
                    value={draft.executionMode}
                    onChange={(e) => set("executionMode", e.target.value as "one_way" | "two_way")}
                  >
                    <option value="one_way">One-way (open)</option>
                    <option value="two_way">Two-way (+close)</option>
                  </select>
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
          </div>
        </div>

        {/* Modal Bottom Footer Actions */}
        <div className="px-6 py-3.5 border-t border-surface-border bg-surface flex items-center justify-between shrink-0 font-mono text-xs">
          <button
            type="button"
            onClick={handleResetDefaults}
            className="flex items-center space-x-1.5 px-3 py-1.5 bg-surface hover:bg-surface-raised text-slate-600 hover:text-slate-900 rounded-md border border-surface-border transition cursor-pointer"
            title="Reset to default monitor parameters"
          >
            <RotateCcw className="w-3.5 h-3.5 text-slate-500" />
            <span>Reset Defaults</span>
          </button>
          <div className="flex items-center space-x-3">
            {error && (
              <span className="text-xs font-mono text-red-600 max-w-72 truncate" title={error}>
                {error}
              </span>
            )}
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-1.5 bg-surface hover:bg-surface-raised text-slate-700 border border-surface-border rounded-md font-medium transition cursor-pointer"
            >
              Cancel
            </button>
            <button
              type="button"
              onClick={handleSave}
              disabled={saving}
              className={`flex items-center space-x-1.5 px-4 py-1.5 rounded-md font-semibold transition shadow-2xs cursor-pointer ${
                saveSuccess
                  ? "bg-profit text-white border border-profit-border"
                  : "bg-accent hover:bg-blue-600 text-white"
              }`}
            >
              {saveSuccess ? (
                <>
                  <Check className="w-4 h-4" />
                  <span>Saved!</span>
                </>
              ) : (
                <span>{saving ? "Saving..." : "Save & Apply"}</span>
              )}
            </button>
          </div>
      </div>
      </div>
    </div>
  );
};

/* ------------------------------------------------------------------ */
/* Capacity ladder expansion                                           */
/* ------------------------------------------------------------------ */

const LadderRow: React.FC<{ row: MonitorRowDto }> = React.memo(({ row }) => (
  <tr className="bg-surface-raised/70">
    <td colSpan={18} className="px-6 py-2">
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
                className={`w-10 rounded-sm ${good ? "bg-accent/70" : "bg-slate-300"}`}
                style={{ height: `${h}px` }}
                title={`net ${fmtBps(t.netBps)}bp · E ${fmtBps(t.expectedNetBps)}bp · p ${fmtPct(t.marginalPWin)}`}
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
    </td>
  </tr>
));

interface RadarRowProps {
  row: MonitorRowDto;
  rowKey: string;
  open: boolean;
  held: boolean;
  sizeValue: number | undefined;
  executing: boolean;
  onToggle: (rowKey: string) => void;
  onSizeChange: (rowKey: string, value: number) => void;
  onExecute: (row: MonitorRowDto) => void;
}

const RadarRow: React.FC<RadarRowProps> = React.memo(
  ({ row: r, rowKey: k, open, held, sizeValue, executing, onToggle, onSizeChange, onExecute }) => {
    return (
      <React.Fragment>
        <tr
          className="border-b border-surface-border/60 hover:bg-slate-50 cursor-pointer"
          onClick={() => onToggle(k)}
        >
          <td className="px-2 py-1.5 text-slate-400">
            {open ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
          </td>
          <td className="px-2 py-1.5 font-bold text-slate-900">
            {r.symbol}
            {held && (
              <span
                className="ml-1.5 px-1 py-px rounded-sm border border-emerald-300 bg-emerald-50 text-emerald-700 text-xs font-mono"
                title="Bot-managed convergence position open on this pair"
              >
                bot
              </span>
            )}
          </td>
          <td className="px-2 py-1.5">
            <span className="text-slate-700 font-mono">
              {VENUE_LABEL[r.buyVenue] ?? r.buyVenue}→
              {VENUE_LABEL[r.sellVenue] ?? r.sellVenue}
            </span>
            {isLiveCapableRoute(r.buyVenue, r.sellVenue) ? (
              <span
                className="ml-1.5 px-1 py-px rounded-xs border border-emerald-400 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 text-[10px] font-mono font-bold"
                title="Both venues support live API execution (Aster ↔ Hyperliquid)"
              >
                LIVE
              </span>
            ) : (
              <span
                className="ml-1.5 px-1 py-px rounded-xs border border-surface-border bg-slate-100 dark:bg-slate-800 text-slate-400 text-[10px] font-mono"
                title="Route involves a read-only venue — paper execution only"
              >
                PAPER
              </span>
            )}
          </td>
          <td className="px-2 py-1.5 text-right font-bold text-accent">
            {fmtBpsU(r.expectedProfitBps)}
          </td>
          <td className="px-2 py-1.5 text-right">{fmtBpsU(r.netCaptureBps)}</td>
          <td className="px-2 py-1.5 text-right text-slate-500">
            {fmtBpsU(r.baselineBps)}
          </td>
          <td
            className={`px-2 py-1.5 text-right ${
              (r.deviationBps ?? 0) >= 0 ? "text-emerald-600" : "text-rose-600"
            }`}
          >
            {fmtBpsU(r.deviationBps)}
          </td>
          <td className="px-2 py-1.5 text-right">{fmtPct(r.pWin)}</td>
          <td className="px-2 py-1.5 text-right" title="Fraction of in-window buckets at/above entry (persistence gate)">
            {fmtPct(r.thresholdRatio)}
          </td>
          <td className="px-2 py-1.5 text-right" title="Fraction of expected window buckets with both venues observed (coverage gate)">
            {fmtPct(r.coverage)}
          </td>
          <td className="px-2 py-1.5 text-right">{fmtUsd(r.expectedProfitUsd)}</td>
          <td className="px-2 py-1.5 text-right">{fmtUsd(r.optimalSizeUsd)}</td>
          <td className="px-2 py-1.5 text-right">{fmtUsd(r.breakevenSizeUsd)}</td>
          <td className="px-2 py-1.5 text-right" title={`first seen ${fmtTs(r.firstSeenMs)}`}>
            {fmtDur(r.signalAgeSecs)}
          </td>
          <td className="px-2 py-1.5 text-right">{fmtDur(r.expectedConvergeSecs)}</td>
          <td className="px-2 py-1.5">
            <span
              className={`px-1.5 py-0.5 rounded-sm border text-xs ${STATUS_STYLE[r.status]}`}
              title={
                r.status === "collecting"
                  ? `history ${(r.qualificationProgress * 100).toFixed(0)}% of window · ${r.observedEpisodes} episodes`
                  : r.blockReason ?? r.status
              }
            >
              {r.status}
              {r.status === "collecting" &&
                ` ${(r.qualificationProgress * 100).toFixed(0)}%`}
            </span>
          </td>
          <td className="px-2 py-1.5" title="UTC session regime and its size-allowance multiplier">
            <SessionBadge
              sessionId={r.sessionId}
              provisional={r.sessionProvisional}
              allow={r.sessionAllow}
              sizeMult={r.sessionSizeMult}
            />
          </td>
          <td className="px-2 py-1.5 text-right" onClick={(e) => e.stopPropagation()}>
            <div className="flex items-center justify-end gap-1">
              <input
                className="w-14 bg-surface border border-surface-border rounded-md px-1 py-0.5 text-right text-xs text-slate-800"
                type="number"
                min={0}
                placeholder={fmtUsd(r.optimalSizeUsd ?? 1000)}
                value={sizeValue ?? ""}
                onChange={(e) => onSizeChange(k, Number(e.target.value))}
              />
              <button
                className="px-2 py-1 rounded-md bg-accent-dim border border-accent-line text-accent text-xs hover:bg-blue-100 disabled:opacity-30 disabled:cursor-not-allowed"
                disabled={!r.executable || executing}
                title={r.executable ? "execute paper trade" : r.blockReason ?? "blocked"}
                onClick={() => onExecute(r)}
              >
                {executing ? "…" : "Exec"}
              </button>
            </div>
          </td>
        </tr>
        {open && <LadderRow row={r} />}
      </React.Fragment>
    );
  },
);

/* ------------------------------------------------------------------ */
/* Auto-bot config modal (separate DTO from the monitor configurator)   */
/* ------------------------------------------------------------------ */

const BOT_NUM_FIELDS: { key: keyof RadarBotConfigDto; label: string; tooltip: string; step?: string }[] = [
  { key: "minExpectedProfitBps", label: "Min expected profit (bps)", tooltip: "Entry hurdle on probability-weighted expected profit after friction." },
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

const BotConfigPanel: React.FC<{
  cfg: RadarBotConfigDto;
  onSave: (c: RadarBotConfigDto) => Promise<void>;
  onClose: () => void;
}> = ({ cfg, onSave, onClose }) => {
  const [draft, setDraft] = useState<RadarBotConfigDto>(cfg);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const set = <K extends keyof RadarBotConfigDto>(k: K, v: RadarBotConfigDto[K]) => {
    setError(null);
    setDraft((d) => ({ ...d, [k]: v }));
  };
  const num = (v: string, fallback: number) => {
    const n = Number(v);
    return Number.isFinite(n) ? n : fallback;
  };
  const handleSave = async () => {
    setSaving(true);
    setError(null);
    try {
      await onSave(draft);
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };
  return (
    <div
      className="fixed inset-0 z-50 bg-slate-900/40 backdrop-blur-xs flex items-center justify-center p-3 sm:p-6 select-none animate-in fade-in duration-150"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="bg-surface border border-surface-border rounded-xl w-[560px] max-w-[94vw] max-h-[90vh] flex flex-col shadow-2xl overflow-hidden">
        <div className="flex items-center justify-between px-5 py-3 border-b border-surface-border shrink-0">
          <h2 className="text-[13px] font-bold font-mono tracking-wider text-slate-900">
            Auto-Bot Configurator
          </h2>
          <button
            type="button"
            onClick={onClose}
            className="p-1.5 rounded-md text-slate-400 hover:text-slate-700 hover:bg-surface-raised transition cursor-pointer"
            title="Close"
          >
            <X className="w-4 h-4" />
          </button>
        </div>
        <div className="flex-1 overflow-y-auto p-4 space-y-3 font-mono">
          <div className={CARD_CLS}>
            <label className={`${LABEL_CLS} justify-between`}>
              <span className="flex items-center">
                Mode
                <InfoTooltip text="auto trades entries+exits; paused/reduce_only run exits only; off is fully inert. Arming requires enabled + auto." />
              </span>
              <select
                className="px-2 py-1 bg-surface-raised border border-surface-border rounded-md text-xs font-bold"
                value={draft.mode}
                onChange={(e) => set("mode", e.target.value as RadarBotConfigDto["mode"])}
              >
                <option value="off">off</option>
                <option value="auto">auto</option>
                <option value="paused">paused</option>
                <option value="reduce_only">reduce_only</option>
              </select>
            </label>
            <label className={`${LABEL_CLS} justify-between mt-2`}>
              <span className="flex items-center">
                Arm auto entries
                <InfoTooltip text="Master opt-in. Entries fire only when armed AND mode is auto. Ships off." />
              </span>
              <SwitchToggle size="sm" checked={draft.enabled} onChange={(v) => set("enabled", v)} />
            </label>
          </div>
          <div className={`${CARD_CLS} flex items-center justify-between gap-3`}>
            <label className={`${LABEL_CLS} flex-1`}>
              <span className="flex items-center">
                Min win probability (%)
                <InfoTooltip text="Entry conviction floor — demand higher P_win than the monitor display threshold." />
              </span>
            </label>
            <input
              className={INPUT_INLINE_CLS}
              type="number"
              min={50}
              max={99.9}
              step="0.1"
              value={(draft.minPWin * 100).toFixed(1)}
              onChange={(e) => set("minPWin", num(e.target.value, 93) / 100)}
            />
          </div>
          {BOT_NUM_FIELDS.map((f) => (
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
                value={Number(draft[f.key])}
                onChange={(e) =>
                  set(f.key, num(e.target.value, Number(draft[f.key])) as never)
                }
              />
            </div>
          ))}
          {error && <div className="text-xs font-mono text-red-600">{error}</div>}
        </div>
        <div className="flex items-center justify-end gap-2 px-5 py-3 border-t border-surface-border shrink-0">
          <button
            type="button"
            onClick={handleSave}
            disabled={saving}
            className="px-3 py-1.5 rounded-md bg-accent text-white text-xs font-mono font-bold disabled:opacity-40 cursor-pointer"
          >
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
};

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
  const [minExpBps, setMinExpBps] = useState("");
  const [minPWin, setMinPWin] = useState("");
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
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [executing, setExecuting] = useState<string | null>(null);
  const [execError, setExecError] = useState<string | null>(null);
  const [execSuccess, setExecSuccess] = useState<string | null>(null);
  const [sizeUsd, setSizeUsd] = useState<Record<string, number>>({});
  // Autonomous convergence bot (paper-only, opt-in). Backend BotMode is the
  // single source of truth — no page-local lifecycle phase.
  const [botCfg, setBotCfg] = useState<RadarBotConfigDto | null>(null);
  const [botStatus, setBotStatus] = useState<RadarBotStatusDto | null>(null);
  const [showBotConfig, setShowBotConfig] = useState(false);
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

  const heldSet = useMemo(
    () => buildHeldSet(botStatus?.openPositions),
    [botStatus],
  );

  const venues = useMemo(
    () => Array.from(new Set((snap?.rows ?? []).flatMap((r) => [r.buyVenue, r.sellVenue]))).sort(),
    [snap],
  );

  const rows = useMemo(() => {
    let out = snap?.rows ?? [];
    const floor = cfg?.minExpectedProfitBps ?? 0;
    out = out.filter((r) => (r.expectedProfitBps ?? -Infinity) > floor);
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
      out = out.filter((r) => (r.pWin ?? -Infinity) >= mp / 100);
    return out;
  }, [snap, search, venueFilters, minExpBps, minPWin, cfg]);

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

  const handleToggleRow = useCallback((k: string) => {
    setExpanded((s) => {
      const n = new Set(s);
      if (n.has(k)) n.delete(k);
      else n.add(k);
      return n;
    });
  }, []);

  const handleSizeChange = useCallback((k: string, value: number) => {
    setSizeUsd((m) => ({ ...m, [k]: value }));
  }, []);

  return (
    <div className="h-full flex flex-col bg-background text-slate-700 overflow-hidden p-4 space-y-4">
      {showConfig && cfg && (
        <ConfigPanel
          cfg={cfg}
          onSave={async (c) => {
            await api.setMonitorConfig(c);
            setCfg(c);
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
            className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-md"
            title="Master monitor switch — edit via Configure Strategy"
          >
            <span className="text-slate-400">Status</span>
            <span className="font-bold text-slate-900">{cfg ? (cfg.enabled ? "ON" : "OFF") : "—"}</span>
          </div>
          <div
            className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-md"
            title="Qualification window mode — edit via Configure Strategy"
          >
            <span className="text-slate-400">Qualification</span>
            <span className="font-bold text-slate-900">{cfg?.qualificationMode ?? "—"}</span>
          </div>
          <div
            className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-md"
            title="Minimum convergence probability floor — edit via Configure Strategy"
          >
            <span className="text-slate-400">Min Win</span>
            <span className="font-bold text-slate-900">
              {cfg ? `${(cfg.minWinProbability * 100).toFixed(0)}` : "—"}
            </span>
            <span className="text-slate-400">%</span>
          </div>
          <div
            className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-md"
            title="Radar table expected-profit floor — edit via Configure Strategy"
          >
            <span className="text-slate-400">Min Exp</span>
            <span className="font-bold text-slate-900">{cfg?.minExpectedProfitBps ?? "—"}</span>
            <span className="text-slate-400">BPS</span>
          </div>
          <div
            className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-md"
            title="Execution mode — edit via Configure Strategy"
          >
            <span className="text-slate-400">Execution</span>
            <span className="font-bold text-slate-900">
              {cfg ? (cfg.executionMode === "two_way" ? "two-way" : "one-way") : "—"}
            </span>
          </div>
        </div>

        <div className="flex items-center space-x-3 shrink-0">
          {(execError || flattenError) && (
            <span
              className="text-xs font-mono text-red-600 max-w-72 truncate"
              title={execError ?? flattenError ?? ""}
            >
              {execError ?? flattenError}
            </span>
          )}
          {execSuccess && !execError && !flattenError && (
            <span
              className="text-xs font-mono text-emerald-600 max-w-96 truncate"
              title={execSuccess}
            >
              {execSuccess}
            </span>
          )}
          <button
            type="button"
            onClick={() => setShowConfig(true)}
            className="flex items-center space-x-1.5 px-3 py-1.5 bg-surface hover:bg-slate-100 text-accent font-semibold text-xs font-mono rounded-md border border-accent-line/60 hover:border-accent transition shadow-2xs cursor-pointer"
            title="Configure Spread Radar monitor parameters"
          >
            <SlidersHorizontal className="w-3.5 h-3.5 text-accent" />
            <span>Configure Strategy</span>
          </button>
        </div>
      </div>

      {showBotConfig && botCfg && (
        <BotConfigPanel
          cfg={botCfg}
          onSave={async (c) => {
            await api.setRadarBotConfig(c);
            setBotCfg(c);
          }}
          onClose={() => setShowBotConfig(false)}
        />
      )}

      <AutoBotGadget
        cfg={botCfg}
        status={botStatus}
        flattening={flattening}
        onMode={(m) => void handleBotMode(m)}
        onConfigure={() => setShowBotConfig(true)}
        onFlatten={() => void handleFlatten()}
      />
      <BotStatCards stats={cycleStats} />
      {(botError) && (
        <div className="px-4 py-2 bg-red-50 border border-red-200 rounded-lg text-xs font-mono text-red-600 shrink-0">
          {botError}
        </div>
      )}

      {/* Radar gadget — filter strip + table in a single card.
          Filters are display-only; none of these affect the monitor or strategy. */}
      <div className="flex-1 min-h-0 flex flex-col bg-surface rounded-lg border border-surface-border shadow-2xs overflow-hidden">
      <div className="flex items-center gap-3 px-4 py-3 border-b border-surface-border text-xs font-mono shrink-0">
        <div
          className="flex items-center gap-1.5 bg-surface-raised border border-surface-border rounded-md px-2.5 py-1.5"
          title="Display filter — symbol contains"
        >
          <Search size={11} className="text-slate-400" />
          <span className="text-slate-400">Symbol</span>
          <input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="bg-transparent w-28 outline-none font-bold text-slate-900 uppercase"
          />
        </div>
        <div className="relative" ref={venueMenuRef}>
          <button
            type="button"
            onClick={() => setShowVenueMenu((v) => !v)}
            className="flex items-center gap-1.5 bg-surface-raised border border-surface-border rounded-md px-2.5 py-1.5 font-mono transition cursor-pointer hover:border-slate-300"
            title="Display filter — shows only rows with a selected venue on either leg. Does not affect the monitor or strategy"
          >
            <span className="text-slate-400">Venue</span>
            <span className="font-bold text-slate-900">
              {venueFilters.length === 0
                ? "ALL"
                : venueFilters.map((v) => VENUE_LABEL[v] ?? v).join("+")}
            </span>
            <ChevronDown
              className={`w-3 h-3 text-slate-400 transition-transform ${showVenueMenu ? "rotate-180" : ""}`}
            />
          </button>
          {showVenueMenu && (
            <div className="absolute left-0 top-full mt-1.5 w-52 bg-surface border border-surface-border rounded-md shadow-lg p-1 z-30 font-mono text-xs animate-in fade-in zoom-in-95 duration-100">
              <button
                type="button"
                onClick={() => setVenueFilters([])}
                className={`w-full flex items-center justify-between px-2.5 py-1.5 text-left transition rounded-sm ${
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
                    className={`w-full flex items-center justify-between px-2.5 py-1.5 text-left transition rounded-sm ${
                      on
                        ? "bg-accent/10 text-accent font-semibold"
                        : "text-slate-700 hover:bg-surface-raised hover:text-slate-900"
                    }`}
                  >
                    <span>{VENUE_LABEL[v] ?? v}</span>
                    {on && <Check className="w-3.5 h-3.5" />}
                  </button>
                );
              })}
            </div>
          )}
        </div>
        <div
          className="flex items-center gap-1.5 bg-surface-raised border border-surface-border rounded-md px-2.5 py-1.5"
          title="Display filter — minimum expected profit"
        >
          <span className="text-slate-400">Min Exp</span>
          <input
            value={minExpBps}
            onChange={(e) => setMinExpBps(e.target.value)}
            className="bg-transparent w-16 outline-none text-right font-bold text-slate-900"
          />
          <span className="text-slate-400">BPS</span>
        </div>
        <div
          className="flex items-center gap-1.5 bg-surface-raised border border-surface-border rounded-md px-2.5 py-1.5"
          title="Display filter — minimum win probability"
        >
          <span className="text-slate-400">Min Win</span>
          <input
            value={minPWin}
            onChange={(e) => setMinPWin(e.target.value)}
            className="bg-transparent w-16 outline-none text-right font-bold text-slate-900"
          />
          <span className="text-slate-400">%</span>
        </div>
        <div className="ml-auto flex items-center gap-2">
          {(snap?.exempted.length ?? 0) > 0 && (
            <span className="text-slate-500 flex items-center gap-1">
              <EyeOff size={11} /> {snap?.exempted.length} hidden
            </span>
          )}
        </div>
      </div>

      <div className="flex-1 overflow-auto min-h-0">
        <table className="w-full text-xs font-mono">
          <thead className="sticky top-0 z-10 bg-surface-raised text-slate-500 text-xs tracking-wider">
            <tr className="text-slate-600 font-semibold tracking-wider text-left border-b border-surface-border">
              <th className="px-2 py-1.5 w-6" />
              <th className="px-2 py-1.5">Symbol</th>
              <th className="px-2 py-1.5" title="Buy venue → sell venue">Route</th>
              <th className="px-2 py-1.5 text-right" title="Probability-weighted expected profit, basis points">
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
              <th className="px-2 py-1.5 text-right" title="Empirical probability the deviation converges before the stop band">
                Win %
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
              <th className="px-2 py-1.5" title="UTC session regime and its size-allowance multiplier">
                Session
              </th>
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
                  open={expanded.has(k)}
                  held={isRowHeld(r, heldSet)}
                  sizeValue={sizeUsd[k]}
                  executing={executing === k}
                  onToggle={handleToggleRow}
                  onSizeChange={handleSizeChange}
                  onExecute={handleExecute}
                />
              );
            })}
          </tbody>
        </table>
      </div>
      </div>
    </div>
  );
};

export const OpportunityRadar: React.FC = React.memo(OpportunityRadarInner);
