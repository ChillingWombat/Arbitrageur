import React, { useEffect, useMemo, useRef, useState } from "react";
import {
  Activity,
  Check,
  Search,
  SlidersHorizontal,
  ChevronDown,
  ChevronRight,
  EyeOff,
  X,
} from "lucide-react";
import { BotLifecycleControls } from "./BotLifecycleControls";
import type {
  MonitorSnapshotDto,
  MonitorRowDto,
  MonitorConfigDto,
} from "../types";
import { api } from "../services/api";
import { useBotLifecycle } from "../hooks/useBotLifecycle";
import { EmptyState } from "./EmptyState";

/* ------------------------------------------------------------------ */
/* Formatting helpers — missing data always renders as "—", never 0    */
/* ------------------------------------------------------------------ */

const fmtBps = (v: number | null | undefined, digits = 1) =>
  v == null || !Number.isFinite(v) ? "—" : `${v.toFixed(digits)}`;

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
};

/* ------------------------------------------------------------------ */
/* Config drawer                                                       */
/* ------------------------------------------------------------------ */

const ConfigPanel: React.FC<{
  cfg: MonitorConfigDto;
  onSave: (c: MonitorConfigDto) => Promise<void>;
  onClose: () => void;
}> = ({ cfg, onSave, onClose }) => {
  const [draft, setDraft] = useState<MonitorConfigDto>(cfg);
  const [saving, setSaving] = useState(false);
  const set = <K extends keyof MonitorConfigDto>(k: K, v: MonitorConfigDto[K]) =>
    setDraft((d) => ({ ...d, [k]: v }));

  const num = (v: string, fallback: number) => {
    const n = Number(v);
    return Number.isFinite(n) ? n : fallback;
  };

  return (
    <div className="fixed inset-0 z-50 bg-slate-900/40 backdrop-blur-xs flex items-center justify-center p-4 select-none animate-in fade-in duration-150">
      <div className="bg-surface rounded-xl border border-surface-border shadow-xl w-full max-w-2xl max-h-[85vh] flex flex-col overflow-hidden">
        <div className="flex items-center justify-between px-4 py-3 border-b border-surface-border">
          <h2 className="text-[13px] font-bold font-mono tracking-wider text-slate-900 flex items-center space-x-2">
            <SlidersHorizontal size={13} className="text-accent" />
            <span>Radar Monitor Configuration</span>
          </h2>
          <button
            className="p-1 rounded-md hover:bg-slate-100 text-slate-400 hover:text-slate-700 transition"
            onClick={onClose}
            title="Close"
          >
            <X size={14} />
          </button>
        </div>
        <div className="px-4 py-4 text-xs font-mono text-slate-700 overflow-y-auto">
          <div className="grid grid-cols-2 md:grid-cols-3 gap-x-6 gap-y-3">
        <label className="flex items-center justify-between gap-2">
          <span className="text-slate-500">Monitor enabled</span>
          <input
            type="checkbox"
            checked={draft.enabled}
            onChange={(e) => set("enabled", e.target.checked)}
          />
        </label>
        <label className="flex items-center justify-between gap-2">
          <span className="text-slate-500">Qualification</span>
          <select
            className="bg-surface border border-surface-border text-slate-800 rounded-md px-1 py-0.5"
            value={draft.qualificationMode}
            onChange={(e) => set("qualificationMode", e.target.value as "auto" | "manual")}
          >
            <option value="auto">auto (2× half-life)</option>
            <option value="manual">manual</option>
          </select>
        </label>
        <label className="flex items-center justify-between gap-2">
          <span className="text-slate-500">Manual window (h)</span>
          <input
            className="w-20 bg-surface border border-surface-border text-slate-800 rounded-md px-1 py-0.5 text-right"
            type="number"
            min={1}
            value={draft.manualWindowSecs / 3600}
            onChange={(e) => set("manualWindowSecs", num(e.target.value, 24) * 3600)}
          />
        </label>
        <label className="flex items-center justify-between gap-2">
          <span className="text-slate-500">Half-life ×</span>
          <input
            className="w-16 bg-surface border border-surface-border text-slate-800 rounded-md px-1 py-0.5 text-right"
            type="number"
            step={0.5}
            min={0.5}
            value={draft.halfLifeMultiplier}
            onChange={(e) => set("halfLifeMultiplier", num(e.target.value, 2))}
          />
        </label>
        <label className="flex items-center justify-between gap-2">
          <span className="text-slate-500">Min window (min)</span>
          <input
            className="w-16 bg-surface border border-surface-border text-slate-800 rounded-md px-1 py-0.5 text-right"
            type="number"
            min={5}
            value={draft.minWindowSecs / 60}
            onChange={(e) => set("minWindowSecs", num(e.target.value, 30) * 60)}
          />
        </label>
        <label className="flex items-center justify-between gap-2">
          <span className="text-slate-500">Variation ≥ friction ×</span>
          <input
            className="w-16 bg-surface border border-surface-border text-slate-800 rounded-md px-1 py-0.5 text-right"
            type="number"
            step={0.1}
            min={0.1}
            value={draft.frictionCoverMult}
            onChange={(e) => set("frictionCoverMult", num(e.target.value, 1))}
          />
        </label>
        <label className="flex items-center justify-between gap-2">
          <span className="text-slate-500">Capacity tiers ($)</span>
          <input
            className="w-36 bg-surface border border-surface-border text-slate-800 rounded-md px-1 py-0.5 text-right"
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
        </label>
        <label className="flex items-center justify-between gap-2">
          <span className="text-slate-500">Min win prob</span>
          <input
            className="w-16 bg-surface border border-surface-border text-slate-800 rounded-md px-1 py-0.5 text-right"
            type="number"
            step={0.01}
            min={0.5}
            max={1}
            value={draft.minWinProbability}
            onChange={(e) => set("minWinProbability", num(e.target.value, 0.9))}
          />
        </label>
        <label className="flex items-center justify-between gap-2">
          <span className="text-slate-500">Min exp profit (BPS)</span>
          <input
            className="w-16 bg-surface border border-surface-border text-slate-800 rounded-md px-1 py-0.5 text-right"
            type="number"
            step={0.5}
            value={draft.minExpectedProfitBps}
            onChange={(e) => set("minExpectedProfitBps", num(e.target.value, 0))}
            title="Radar table floor — rows show only when expected profit exceeds this (0 = positive-only; negative shows all)"
          />
        </label>
        <label className="flex items-center justify-between gap-2">
          <span className="text-slate-500">Execution</span>
          <select
            className="bg-surface border border-surface-border text-slate-800 rounded-md px-1 py-0.5"
            value={draft.executionMode}
            onChange={(e) => set("executionMode", e.target.value as "one_way" | "two_way")}
          >
            <option value="one_way">one-way (open)</option>
            <option value="two_way">two-way (+close)</option>
          </select>
        </label>
        <label className="flex items-center justify-between gap-2">
          <span className="text-slate-500">TX reserve</span>
          <input
            className="w-16 bg-surface border border-surface-border text-slate-800 rounded-md px-1 py-0.5 text-right"
            type="number"
            step={0.05}
            min={0}
            max={0.9}
            value={draft.txReservePct}
            onChange={(e) => set("txReservePct", num(e.target.value, 0.25))}
          />
        </label>
        <label className="flex items-center justify-between gap-2">
          <span className="text-slate-500">Watchlist refresh (h)</span>
          <input
            className="w-16 bg-surface border border-surface-border text-slate-800 rounded-md px-1 py-0.5 text-right"
            type="number"
            min={1}
            value={draft.watchlistRefreshSecs / 3600}
            onChange={(e) => set("watchlistRefreshSecs", num(e.target.value, 1) * 3600)}
          />
        </label>
        <label className="flex items-center justify-between gap-2">
          <span className="text-slate-500">History window (h)</span>
          <input
            className="w-16 bg-surface border border-surface-border text-slate-800 rounded-md px-1 py-0.5 text-right"
            type="number"
            min={1}
            value={draft.historyWindowSecs / 3600}
            onChange={(e) => set("historyWindowSecs", num(e.target.value, 24) * 3600)}
          />
        </label>
          </div>
        </div>
        <div className="flex justify-end gap-2 px-4 py-3 border-t border-surface-border bg-surface-raised">
          <button
            className="px-3 py-1.5 rounded-md border border-surface-border text-slate-500 hover:text-slate-800 hover:bg-slate-100 text-xs font-mono"
            onClick={onClose}
          >
            Cancel
          </button>
          <button
            className="px-3 py-1.5 rounded-md bg-accent-dim border border-accent-line text-accent hover:bg-blue-100 disabled:opacity-40 text-xs font-mono font-bold"
            disabled={saving}
            onClick={async () => {
              setSaving(true);
              try {
                await onSave(draft);
                onClose();
              } finally {
                setSaving(false);
              }
            }}
          >
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
};

/* ------------------------------------------------------------------ */
/* Capacity ladder expansion                                           */
/* ------------------------------------------------------------------ */

const LadderRow: React.FC<{ row: MonitorRowDto }> = ({ row }) => (
  <tr className="bg-surface-raised/70">
    <td colSpan={15} className="px-6 py-2">
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
);

/* ------------------------------------------------------------------ */
/* Main component                                                      */
/* ------------------------------------------------------------------ */

export const OpportunityRadar: React.FC = () => {
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
  // Radar executions persist under the "radar" strategy — tracked and
  // flattened separately from Single Spread ("futures") positions.
  const bot = useBotLifecycle({ strategyId: "radar" });
  const [execError, setExecError] = useState<string | null>(null);
  const [sizeUsd, setSizeUsd] = useState<Record<string, number>>({});

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
  }, []);

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

  const key = (r: MonitorRowDto) => `${r.symbol}:${r.buyVenue}:${r.sellVenue}`;

  const handleExecute = async (r: MonitorRowDto) => {
    const k = key(r);
    const size = sizeUsd[k] ?? r.optimalSizeUsd ?? 1000;
    setExecuting(k);
    setExecError(null);
    try {
      await api.executeMonitorOpportunity(r.symbol, r.buyVenue, r.sellVenue, size);
    } catch (e) {
      setExecError(`${r.symbol} ${r.buyVenue}→${r.sellVenue}: ${String(e)}`);
    } finally {
      setExecuting(null);
    }
  };

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

      {(execError || bot.lastError) && (
        <div className="px-4 py-1.5 text-xs font-mono text-rose-700 bg-rose-50 rounded-lg border border-rose-200 shrink-0">
          {execError ?? bot.lastError}
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
          <button
            type="button"
            onClick={() => setShowConfig(true)}
            className="flex items-center space-x-1.5 px-3 py-1.5 bg-surface hover:bg-slate-100 text-accent font-semibold text-xs font-mono rounded-md border border-accent-line/60 hover:border-accent transition shadow-2xs cursor-pointer"
            title="Configure Spread Radar monitor parameters"
          >
            <SlidersHorizontal className="w-3.5 h-3.5 text-accent" />
            <span>Configure Strategy</span>
          </button>
          <BotLifecycleControls
            phase={bot.phase}
            hasOpenPositions={bot.hasOpenPositions}
            onStart={bot.start}
            onPause={bot.pause}
            onReduceOnly={bot.reduceOnly}
            onStop={() => void bot.stop()}
          />
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
                Baseline BPS
              </th>
              <th className="px-2 py-1.5 text-right" title="Signed deviation of current edge from baseline">
                Deviation BPS
              </th>
              <th className="px-2 py-1.5 text-right" title="Empirical probability the deviation converges before the stop band">
                Win %
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
                <td colSpan={15}>
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
              const open = expanded.has(k);
              const posEdge = (r.currentEdgeBps ?? 0) > 0;
              return (
                <React.Fragment key={k}>
                  <tr
                    className="border-b border-surface-border/60 hover:bg-slate-50 cursor-pointer"
                    onClick={() =>
                      setExpanded((s) => {
                        const n = new Set(s);
                        if (n.has(k)) n.delete(k);
                        else n.add(k);
                        return n;
                      })
                    }
                  >
                    <td className="px-2 py-1.5 text-slate-400">
                      {open ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
                    </td>
                    <td className="px-2 py-1.5 font-bold text-slate-900">{r.symbol}</td>
                    <td className="px-2 py-1.5">
                      <span className={posEdge ? "text-emerald-600" : "text-rose-600"}>
                        {VENUE_LABEL[r.buyVenue] ?? r.buyVenue}→
                        {VENUE_LABEL[r.sellVenue] ?? r.sellVenue}
                      </span>
                    </td>
                    <td className="px-2 py-1.5 text-right font-bold text-accent">
                      {fmtBps(r.expectedProfitBps)}
                    </td>
                    <td className="px-2 py-1.5 text-right">{fmtBps(r.netCaptureBps)}</td>
                    <td className="px-2 py-1.5 text-right text-slate-500">
                      {fmtBps(r.baselineBps)}
                    </td>
                    <td
                      className={`px-2 py-1.5 text-right ${
                        (r.deviationBps ?? 0) >= 0 ? "text-emerald-600" : "text-rose-600"
                      }`}
                    >
                      {fmtBps(r.deviationBps)}
                    </td>
                    <td className="px-2 py-1.5 text-right">{fmtPct(r.pWin)}</td>
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
                    <td className="px-2 py-1.5 text-right" onClick={(e) => e.stopPropagation()}>
                      <div className="flex items-center justify-end gap-1">
                        <input
                          className="w-14 bg-surface border border-surface-border rounded-md px-1 py-0.5 text-right text-xs text-slate-800"
                          type="number"
                          min={0}
                          placeholder={fmtUsd(r.optimalSizeUsd ?? 1000)}
                          value={sizeUsd[k] ?? ""}
                          onChange={(e) =>
                            setSizeUsd((m) => ({ ...m, [k]: Number(e.target.value) }))
                          }
                        />
                        <button
                          className="px-2 py-1 rounded-md bg-accent-dim border border-accent-line text-accent text-xs hover:bg-blue-100 disabled:opacity-30 disabled:cursor-not-allowed"
                          disabled={!r.executable || executing === k}
                          title={r.executable ? "execute paper trade" : r.blockReason ?? "blocked"}
                          onClick={() => handleExecute(r)}
                        >
                          {executing === k ? "…" : "Exec"}
                        </button>
                      </div>
                    </td>
                  </tr>
                  {open && <LadderRow row={r} />}
                </React.Fragment>
              );
            })}
          </tbody>
        </table>
      </div>
      </div>
    </div>
  );
};
