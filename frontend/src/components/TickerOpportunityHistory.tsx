import React, { useEffect, useMemo, useState } from "react";
import { Radar, Search } from "lucide-react";
import { api } from "../services/api";
import type { MonitorSignalEventDto } from "../types";
import { VenueLogo } from "./VenueLogo";
import { EmptyState } from "./EmptyState";
import { InfoTooltip } from "./InfoTooltip";

/**
 * Opportunity history gadget for the Simple Spread page: every detection
 * the monitor engine persisted for this ticker (or all tickers), newest
 * first. Backed by the durable `monitor_signal_events` audit table — the
 * same rows the engine writes on every lifecycle transition and each
 * minute-bucketed heartbeat while a signal stays active. Nothing here is
 * simulated; if the monitor never saw an edge, the list stays empty.
 */

interface TickerOpportunityHistoryProps {
  /** Canonical symbol of the selected ticker (e.g. "BTC-USDC"). */
  symbol: string;
  /** Poll cadence — matches the log monitor's near-real-time feel. */
  pollMs?: number;
}

const POLL_MS = 5_000;
const PAGE_LIMIT = 200;

/** Statuses that mean "an actionable opportunity exists right now". */
const ACTIONABLE = new Set(["triggered", "ready"]);

const STATUS_STYLE: Record<string, string> = {
  ready: "bg-profit-dim text-profit border-profit-border",
  triggered: "bg-blue-50 text-accent border-blue-200",
  tracking: "bg-slate-100 text-slate-700 border-slate-200",
  preparing: "bg-warn-dim text-warn border-warn-border",
  "no-data": "bg-slate-50 text-slate-400 border-slate-200",
};

const statusChip = (s: string | null) => {
  if (s == null) return <span className="text-slate-300">—</span>;
  const cls = STATUS_STYLE[s] ?? "bg-slate-100 text-slate-600 border-slate-200";
  return (
    <span className={`px-1.5 py-0.2 rounded-md text-[10px] font-bold border ${cls}`}>{s}</span>
  );
};

const fmtBps = (v: number | null, signed = true) =>
  v == null || !Number.isFinite(v)
    ? "—"
    : `${signed && v >= 0 ? "+" : ""}${v.toFixed(1)}`;

const fmtTs = (ms: number) => {
  const d = new Date(ms);
  const today = new Date();
  const sameDay = d.toDateString() === today.toDateString();
  const time = d.toLocaleTimeString(undefined, { hour12: false });
  return sameDay
    ? time
    : `${d.toLocaleDateString(undefined, { month: "short", day: "numeric" })} ${time}`;
};

export const TickerOpportunityHistory: React.FC<TickerOpportunityHistoryProps> = ({
  symbol,
  pollMs = POLL_MS,
}) => {
  const [events, setEvents] = useState<MonitorSignalEventDto[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [allTickers, setAllTickers] = useState(false);
  const [detectedOnly, setDetectedOnly] = useState(true);
  const [query, setQuery] = useState("");

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      try {
        const rows = await api.getMonitorSignalEvents(
          allTickers ? undefined : symbol,
          PAGE_LIMIT,
        );
        if (cancelled) return;
        setEvents(rows);
        setError(null);
        setLoaded(true);
      } catch (e) {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : String(e));
        setLoaded(true);
      }
    };
    void load();
    const t = setInterval(load, pollMs);
    return () => {
      cancelled = true;
      clearInterval(t);
    };
  }, [symbol, allTickers, pollMs]);

  const filtered = useMemo(() => {
    let rows = events;
    if (detectedOnly) {
      rows = rows.filter((e) => ACTIONABLE.has(e.newStatus));
    }
    const q = query.trim().toLowerCase();
    if (q) {
      rows = rows.filter(
        (e) =>
          e.symbol.toLowerCase().includes(q) ||
          e.buyVenue.toLowerCase().includes(q) ||
          e.sellVenue.toLowerCase().includes(q),
      );
    }
    return rows;
  }, [events, detectedOnly, query]);

  const actionableCount = useMemo(
    () => events.filter((e) => ACTIONABLE.has(e.newStatus)).length,
    [events],
  );

  return (
    <div className="font-sans">
      {/* Toolbar */}
      <div className="flex flex-wrap items-center gap-2 mb-2.5">
        <div className="relative flex-1 min-w-[140px] max-w-[240px]">
          <Search className="w-3.5 h-3.5 absolute left-2.5 top-1/2 -translate-y-1/2 text-slate-400" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Filter symbol / venue…"
            className="w-full pl-8 pr-2 py-1.5 text-xs bg-surface-raised border border-surface-border rounded-lg text-slate-900 placeholder:text-slate-400 focus:outline-none focus:ring-2 focus:ring-accent/20 focus:border-accent transition"
          />
        </div>
        <button
          type="button"
          onClick={() => setDetectedOnly((v) => !v)}
          className={`px-2 py-1.5 rounded-lg border text-[11px] font-semibold transition cursor-pointer ${
            detectedOnly
              ? "bg-accent-dim text-accent border-accent-line"
              : "bg-surface text-slate-600 border-surface-border hover:bg-surface-raised"
          }`}
          title="Detected only = transitions into triggered/ready. Off = the full lifecycle audit including heartbeats and drop-outs."
        >
          Detected only · {actionableCount}
        </button>
        <button
          type="button"
          onClick={() => setAllTickers((v) => !v)}
          className={`px-2 py-1.5 rounded-lg border text-[11px] font-semibold transition cursor-pointer ${
            allTickers
              ? "bg-accent-dim text-accent border-accent-line"
              : "bg-surface text-slate-600 border-surface-border hover:bg-surface-raised"
          }`}
          title={allTickers ? "Showing every ticker's detections" : `Scoped to ${symbol || "the selected ticker"}`}
        >
          {allTickers ? "All tickers" : symbol || "This ticker"}
        </button>
        <span className="ml-auto inline-flex items-center text-[10px] text-slate-400">
          durable audit log
          <InfoTooltip text="Every row is a persisted monitor event: status transitions plus a heartbeat each minute while a signal stays active. 'Detected only' keeps transitions into triggered/ready — the moments an actionable spread edge appeared." />
        </span>
      </div>

      {/* Body */}
      {!loaded ? (
        <div className="py-8 text-center text-xs text-slate-400">Loading opportunity history…</div>
      ) : error ? (
        <EmptyState
          icon={Radar}
          title="Opportunity history unavailable"
          hint={error}
        />
      ) : filtered.length === 0 ? (
        <EmptyState
          icon={Radar}
          plain
          title={
            events.length === 0
              ? `No detected opportunities yet${allTickers ? "" : ` for ${symbol}`}`
              : "No events match the current filters"
          }
          hint={
            events.length === 0
              ? "The monitor records a row the moment a spread edge qualifies — detections will appear here as they happen."
              : "Try widening the filters or clearing the search."
          }
        />
      ) : (
        <div className="overflow-x-auto rounded-lg border border-surface-border">
          <table className="w-full text-xs">
            <thead>
              <tr className="bg-slate-50/80 text-left text-[10px] font-semibold uppercase tracking-wider text-slate-500 border-b border-surface-border">
                <th className="py-2 px-3">Time</th>
                {allTickers && <th className="py-2 px-3">Symbol</th>}
                <th className="py-2 px-3">Route</th>
                <th className="py-2 px-3">Transition</th>
                <th className="py-2 px-3 text-right">
                  <span className="inline-flex items-center gap-1">
                    Edge
                    <InfoTooltip text="Gross spread edge vs. the rolling baseline at detection, in basis points." />
                  </span>
                </th>
                <th className="py-2 px-3 text-right">
                  <span className="inline-flex items-center gap-1">
                    Dev
                    <InfoTooltip text="Signed deviation of the current spread from baseline — what the strategy would trade." />
                  </span>
                </th>
                <th className="py-2 px-3 text-right">
                  <span className="inline-flex items-center gap-1">
                    Win%
                    <InfoTooltip text="Mean-reversion win probability at detection time (direction-insensitive)." />
                  </span>
                </th>
                <th className="py-2 px-3 text-right">Kind</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-surface-border/60">
              {filtered.map((e) => (
                <tr key={e.id} className="hover:bg-surface-raised/60 transition">
                  <td className="py-1.5 px-3 text-slate-500 whitespace-nowrap text-[11px] tabular-nums">
                    {fmtTs(e.tsMs)}
                  </td>
                  {allTickers && (
                    <td className="py-1.5 px-3 font-semibold text-slate-900 whitespace-nowrap">
                      {e.symbol}
                    </td>
                  )}
                  <td className="py-1.5 px-3 whitespace-nowrap">
                    <span className="inline-flex items-center gap-1 text-slate-700">
                      <VenueLogo venueId={e.buyVenue} name={e.buyVenue} size={14} />
                      {e.buyVenue}
                      <span className="text-slate-400">→</span>
                      <VenueLogo venueId={e.sellVenue} name={e.sellVenue} size={14} />
                      {e.sellVenue}
                    </span>
                  </td>
                  <td className="py-1.5 px-3 whitespace-nowrap">
                    <span className="inline-flex items-center gap-1">
                      {statusChip(e.oldStatus)}
                      <span className="text-slate-400">→</span>
                      {statusChip(e.newStatus)}
                    </span>
                  </td>
                  <td className="py-1.5 px-3 text-right font-mono tabular-nums text-slate-700">
                    {fmtBps(e.edgeBps)}
                  </td>
                  <td
                    className={`py-1.5 px-3 text-right font-mono tabular-nums font-semibold ${
                      (e.devBps ?? 0) >= 0 ? "text-profit" : "text-loss"
                    }`}
                  >
                    {fmtBps(e.devBps)}
                  </td>
                  <td className="py-1.5 px-3 text-right font-mono tabular-nums text-slate-700">
                    {e.pWin == null ? "—" : `${(e.pWin * 100).toFixed(0)}%`}
                  </td>
                  <td className="py-1.5 px-3 text-right whitespace-nowrap">
                    <span
                      className={`text-[10px] font-semibold ${
                        e.trigger === "transition" ? "text-accent" : "text-slate-400"
                      }`}
                      title={
                        e.trigger === "transition"
                          ? "Status changed — a detection event"
                          : "Minute-bucketed heartbeat while the signal stayed active"
                      }
                    >
                      {e.trigger === "transition" ? "signal" : e.trigger}
                    </span>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
};
