/**
 * ExternalPositionsGadget — "WHAT DID I OPEN ELSEWHERE, AND CLOSE IT CHEAPLY."
 * ============================================================================
 * USER STORY: positions opened by hand on exchange websites are invisible to
 * the strategy monitors (those only aggregate the bot's own signal legs), yet
 * they eat margin and drift. This gadget lists every live on-exchange
 * position minus system-owned legs, grouped by ticker (net-exposure view) or
 * by venue (ops view), and closes any residual through the bot as a
 * reduce-only live order — priced with live depth (touch vs VWAP slip),
 * spread context, and an explicit limit-vs-market recommendation. One click
 * here replaces a manual multi-website close-out session.
 *
 * HOW IT WORKS (for rebuild practice):
 *   - Data comes from three IPC calls in `services/api.ts`: getExternalPositions
 *     (live account reads, system legs subtracted backend-side), then per
 *     selected position recommendExternalClose (read-only friction quote) and
 *     closeExternalPosition (live reduce-only order, Live-Trading-gated).
 *   - Polling is 15s, NOT the app's 3s rhythm: account reads are
 *     authenticated and heavy; a manual Refresh covers urgency. Grouping is
 *     the pure helper `groupExternalPositions` (tested alongside).
 *   - SAFETY UX: clicking / selecting a row only opens a details popup
 *     (read-only pricing). Nothing closes until the explicit per-position
 *     Close button inside the popup is armed and then confirmed. This avoids
 *     misclick closes.
 *   - Safety is backend-enforced (live mode + unlocked vault + capable venue
 *     + fresh re-read + dust floor); the UI additionally disables the button
 *     with the reason inline so the gates are visible, not mysterious.
 */
import React, { useCallback, useEffect, useMemo, useState } from "react";
import { ArrowDownUp, HandCoins, RefreshCw, ShieldAlert, X } from "lucide-react";
import { EmptyState } from "./EmptyState";
import { VenueLogo } from "./VenueLogo";
import { api } from "../services/api";
import { groupExternalPositions, type ExternalGroupBy } from "../services/externalPositions";
import type {
  CloseRecommendationDto,
  CloseResultDto,
  ExternalPositionDto,
  ExternalPositionsResponse,
  ScheduledExitDto,
} from "../types";

const usd = (v: number) =>
  `${v >= 0 ? "+" : "-"}$${Math.abs(v).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
const num = (v: number, d = 4) =>
  v == null || !Number.isFinite(v) ? "—" : v.toLocaleString(undefined, { maximumFractionDigits: d });
const closeStatusLabel = (status: string) =>
  status === "filled"
    ? "Closed"
    : status === "partial"
      ? "Partial close"
      : status === "submitted"
        ? "Order working"
        : status === "unverified"
          ? "Unverified"
          : "Not filled";
const closeStatusClass = (status: string) =>
  status === "filled"
    ? "bg-profit-dim/40 border-profit-border text-slate-900"
    : status === "not_filled"
      ? "bg-loss-dim/40 border-loss-border text-slate-900"
      : "bg-amber-500/10 border-amber-500/30 text-slate-900";

/** Epoch ms → `datetime-local` input value in LOCAL wall time
 *  ("YYYY-MM-DDTHH:MM") — no toISOString timezone arithmetic. */
const toLocalInputValue = (ms: number) => {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
};

export const ExternalPositionsGadget: React.FC = () => {
  // --- List state ------------------------------------------------------------
  const [data, setData] = useState<ExternalPositionsResponse | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [groupBy, setGroupBy] = useState<ExternalGroupBy>("ticker");
  const [selectedKey, setSelectedKey] = useState<string | null>(null);

  // --- Close-panel state (per selected position) ------------------------------
  const [rec, setRec] = useState<CloseRecommendationDto | null>(null);
  const [recBusy, setRecBusy] = useState(false);
  const [recError, setRecError] = useState<string | null>(null);
  const [mode, setMode] = useState<"auto" | "limit" | "market">("limit");
  const [limitPx, setLimitPx] = useState("");
  const [closePct, setClosePct] = useState("100");
  const [scheduleOn, setScheduleOn] = useState(false);
  const [scheduleAt, setScheduleAt] = useState("");
  const [schedules, setSchedules] = useState<ScheduledExitDto[]>([]);
  const [armed, setArmed] = useState(false);
  const [closing, setClosing] = useState(false);
  const [result, setResult] = useState<CloseResultDto | null>(null);
  const [closeError, setCloseError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const res = await api.getExternalPositions();
      setData(res);
      setLoadError(null);
    } catch (e) {
      setLoadError(e instanceof Error ? e.message : String(e));
    }
    try {
      setSchedules(await api.listScheduledExits());
    } catch {
      /* schedule list is informational — never block the position read */
    }
  }, []);

  // 15s cadence: account reads are authenticated + heavy, unlike the 3s
  // strategy polls. Manual refresh covers "close it now" urgency.
  useEffect(() => {
    void load();
    const t = setInterval(() => void load(), 15_000);
    return () => clearInterval(t);
  }, [load]);

  const groups = useMemo(
    () => groupExternalPositions(data?.positions ?? [], groupBy),
    [data, groupBy],
  );
  const selected: ExternalPositionDto | null = useMemo(
    () => (data?.positions ?? []).find((p) => p.key === selectedKey) ?? null,
    [data, selectedKey],
  );

  // Details popup: selection alone never closes — it only opens this modal.
  // Closing requires the explicit Arm + Confirm inside the modal.
  const openDetails = useCallback((p: ExternalPositionDto) => {
    setSelectedKey(p.key);
  }, []);
  const closeDetails = useCallback(() => {
    setSelectedKey(null);
  }, []);

  // ESC closes the popup; row selection stays harmless.
  useEffect(() => {
    if (!selected) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") closeDetails();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selected, closeDetails]);

  const loadRec = useCallback(async (p: ExternalPositionDto) => {
    setRecBusy(true);
    setRecError(null);
    setRec(null);
    try {
      const r = await api.recommendExternalClose(p.venue, p.symbol);
      setRec(r);
      setMode(r.recommendedMode === "market" ? "market" : "limit");
      setLimitPx(r.recommendedLimitPrice != null ? String(r.recommendedLimitPrice) : "");
    } catch (e) {
      setRecError(e instanceof Error ? e.message : String(e));
    } finally {
      setRecBusy(false);
    }
  }, []);

  // (Re)quote when the selection changes. The taker fee is derived
  // server-side from venue info — never hand-entered here.
  useEffect(() => {
    setArmed(false);
    setResult(null);
    setCloseError(null);
    if (!selected) {
      setRec(null);
      return;
    }
    void loadRec(selected);
  }, [selectedKey]); // eslint-disable-line react-hooks/exhaustive-deps

  const parsedPct = Number(closePct);
  const pctValid = Number.isFinite(parsedPct) && parsedPct > 0 && parsedPct <= 100;
  const estCloseQty = selected && pctValid ? (selected.externalQty * parsedPct) / 100 : null;

  const scheduleMs = scheduleAt ? new Date(scheduleAt).getTime() : NaN;
  const scheduleValid = scheduleOn && Number.isFinite(scheduleMs) && scheduleMs > Date.now();

  // One action, one button: schedule enabled → persist a one-shot row that
  // fires this same mode/pct/limit at the picked time; otherwise submit
  // the close now. Both paths require Arm + Confirm.
  const doClose = async () => {
    if (!selected || closing) return;
    if (scheduleOn && !scheduleValid) {
      setCloseError("Pick a future time for the scheduled exit");
      return;
    }
    if (!armed) {
      setArmed(true); // first click arms, second submits
      return;
    }
    setClosing(true);
    setCloseError(null);
    try {
      if (scheduleOn) {
        await api.scheduleExternalExit({
          venue: selected.venue,
          symbol: selected.symbol,
          side: selected.side,
          mode,
          limitPrice: mode === "limit" ? Number(limitPx) : null,
          closePct: pctValid ? parsedPct : 100,
          executeAtMs: scheduleMs,
        });
        setScheduleAt("");
        setScheduleOn(false);
      } else {
        const r = await api.closeExternalPosition({
          venue: selected.venue,
          symbol: selected.symbol,
          mode,
          limitPrice: mode === "limit" ? Number(limitPx) : null,
          closePct: pctValid ? parsedPct : 100,
        });
        setResult(r);
      }
      setArmed(false);
      void load(); // residual should shrink/disappear on next read
    } catch (e) {
      setCloseError(e instanceof Error ? e.message : String(e));
    } finally {
      setClosing(false);
    }
  };

  const cancelSchedule = async (id: string) => {
    try {
      await api.cancelScheduledExit(id);
      void load();
    } catch (e) {
      setCloseError(e instanceof Error ? e.message : String(e));
    }
  };

  const pendingForSelected = selected
    ? schedules.filter(
        (s) =>
          s.status === "pending" &&
          s.venue.toLowerCase() === selected.venue.toLowerCase() &&
          s.symbol.toLowerCase() === selected.symbol.toLowerCase(),
      )
    : [];

  const closeDisabledReason = !data
    ? "Loading positions…"
    : !selected
      ? "Select a position above"
      : !data.liveMode
        ? "Paper mode — switch to Live Trading to submit real closes"
        : !selected.closeSupported
          ? `${selected.venue} cannot place live orders through the bot`
          : scheduleOn && !scheduleValid
            ? "Pick a future time for the scheduled exit"
            : null;

  const tabCls = (on: boolean) =>
    `px-2.5 py-1.5 rounded-lg text-[11px] font-mono font-bold border transition cursor-pointer ${
      on
        ? "bg-accent-dim text-accent border-accent-line/60"
        : "bg-surface text-slate-500 border-surface-border hover:text-slate-800"
    }`;

  return (
    <div className="bg-surface rounded-xl border border-surface-border p-4 shadow-card space-y-3">
      {/* Header: what this answers + refresh (manual, since cadence is slow). */}
      <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
        <div>
          <h2 className="text-[13px] font-mono tracking-wider text-slate-900 font-bold flex items-center space-x-2">
            <HandCoins className="w-5 h-5 text-accent" />
            <span>External Positions — Close via Bot</span>
          </h2>
          <p className="text-xs text-slate-600 font-mono mt-1">
            Manual (non-system) holdings found on exchanges, minus the bot's own legs. Close reduce-only with live depth pricing.
          </p>
        </div>
        <div className="flex items-center gap-2 shrink-0">
          <div className="flex gap-1.5" title="Group positions by ticker (net view) or venue (ops view)">
            <button type="button" onClick={() => setGroupBy("ticker")} className={tabCls(groupBy === "ticker")}>
              By Ticker
            </button>
            <button type="button" onClick={() => setGroupBy("venue")} className={tabCls(groupBy === "venue")}>
              By Venue
            </button>
          </div>
          <button
            type="button"
            onClick={() => void load()}
            className="p-2 rounded-lg border border-surface-border text-slate-500 hover:text-accent hover:border-accent/40 transition cursor-pointer"
            title="Refresh live account reads now"
          >
            <RefreshCw className="w-3.5 h-3.5" />
          </button>
        </div>
      </div>

      {loadError && (
        <div className="px-3 py-2.5 rounded-lg border bg-loss-dim/40 border-loss-border text-xs font-mono text-slate-700">
          Position read failed: {loadError}
        </div>
      )}
      {(data?.unreadable ?? []).length > 0 && (
        <div className="flex items-start gap-2 px-3 py-2 rounded-lg border bg-warn-dim/40 border-warn-border text-[11px] font-mono text-slate-700">
          <ShieldAlert className="w-3.5 h-3.5 shrink-0 mt-0.5 text-warn" />
          <span>{data!.unreadable.join(" · ")}</span>
        </div>
      )}

      {/* Grouped position list. */}
      {groups.length === 0 ? (
        <EmptyState
          title={data ? "No external positions — everything on-exchange is bot-owned" : "Loading live positions…"}
          hint="Manual trades placed on exchange websites appear here for bot-assisted closing."
        />
      ) : (
        <div className="space-y-2.5">
          {groups.map((g) => (
            <div key={`${groupBy}-${g.key}`} className="rounded-lg border border-surface-border overflow-hidden">
              <div className="flex items-center justify-between gap-2 px-3 py-2 bg-surface-raised/70 text-xs font-mono">
                <span className="font-bold text-slate-900 flex items-center gap-1.5">
                  <ArrowDownUp className="w-3.5 h-3.5 text-slate-400" />
                  {g.label}
                  <span className="text-slate-400 font-semibold">· {g.positions.length}</span>
                </span>
                <span className="tabular-nums text-slate-600">
                  net <span className={`font-bold ${g.netNotionalUsd >= 0 ? "text-slate-900" : "text-slate-900"}`}>${Math.abs(g.netNotionalUsd).toFixed(0)}</span>
                  <span className="text-slate-400"> gross ${g.grossNotionalUsd.toFixed(0)}</span>
                  {" · "}
                  <span className={`font-bold ${g.totalUnrealizedUsd >= 0 ? "text-profit" : "text-loss"}`}>{usd(g.totalUnrealizedUsd)}</span>
                </span>
              </div>
              {g.positions.map((p) => {
                const on = p.key === selectedKey;
                return (
                  <div
                    key={p.key}
                    role="button"
                    tabIndex={0}
                    onClick={() => openDetails(p)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        e.preventDefault();
                        openDetails(p);
                      }
                    }}
                    className={`w-full flex items-center justify-between gap-2 px-3 py-2 text-xs font-mono border-t border-surface-border/60 transition cursor-pointer text-left ${
                      on ? "bg-accent-dim/50" : "hover:bg-surface-raised/50"
                    }`}
                    title="Click for details — selection alone never closes"
                  >
                    <span className="flex items-center gap-2 min-w-0">
                      <span className={`px-1.5 py-0.5 rounded-md text-[10px] font-bold ${p.side === "long" ? "bg-profit-dim text-profit" : "bg-loss-dim text-loss"}`}>
                        {p.side.toUpperCase()}
                      </span>
                      <span className="font-bold text-slate-900 truncate">{p.symbol}</span>
                      <span className="flex items-center gap-1.5 min-w-0">
                        <VenueLogo venueId={p.venue.toLowerCase()} name={p.venue} size={16} />
                        <span className="text-slate-400 truncate">{p.venue}</span>
                      </span>
                      {!p.closeSupported && <span className="text-warn text-[10px] font-bold">NO BOT CLOSE</span>}
                    </span>
                    <span className="flex items-center gap-2 shrink-0">
                      <span className="tabular-nums text-slate-700">
                        {num(p.externalQty)} @ ${num(p.markPrice, 2)}
                        {" · "}
                        <span className={`font-bold ${p.unrealizedPnlUsd >= 0 ? "text-profit" : "text-loss"}`}>{usd(p.unrealizedPnlUsd)}</span>
                      </span>
                      <button
                        type="button"
                        onClick={(e) => {
                          e.stopPropagation();
                          openDetails(p);
                        }}
                        className="px-2 py-1 rounded-lg text-[11px] font-mono font-bold border transition cursor-pointer bg-surface text-slate-600 border-surface-border hover:text-slate-900 hover:border-slate-300"
                        title={p.closeSupported ? `Close ${p.symbol} on ${p.venue} via bot — opens details popup` : `${p.venue} cannot place live orders through the bot`}
                      >
                        Details
                      </button>
                      <button
                        type="button"
                        disabled={!p.closeSupported}
                        onClick={(e) => {
                          e.stopPropagation();
                          openDetails(p);
                        }}
                        className="px-2 py-1 rounded-lg text-[11px] font-mono font-bold border transition cursor-pointer bg-accent-dim text-accent border-accent-line/60 hover:bg-accent-dim/80 disabled:cursor-not-allowed disabled:opacity-50"
                        title={p.closeSupported ? `Close ${p.symbol} on ${p.venue} via bot — opens details popup, nothing closes until Confirm` : `${p.venue} cannot place live orders through the bot`}
                      >
                        Close…
                      </button>
                    </span>
                  </div>
                );
              })}
            </div>
          ))}
        </div>
      )}

      {/* Details popup for the selected position. Selection alone never
          closes — the live close requires Arm + Confirm inside this modal. */}
      {selected && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-slate-900/50 backdrop-blur-sm animate-in fade-in duration-150"
          onClick={() => closeDetails()}
          role="dialog"
          aria-modal="true"
          aria-label={`External position details for ${selected.symbol} on ${selected.venue}`}
        >
          <div
            className="bg-surface border border-surface-border rounded-2xl w-full max-w-lg max-h-[85vh] overflow-y-auto shadow-2xl animate-in zoom-in-95 duration-150"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="p-4 space-y-3">
              <div className="flex items-center justify-between gap-2 border-b border-surface-border pb-2.5">
                <div className="min-w-0">
                  <h3 className="text-[13px] font-mono font-bold tracking-wider text-slate-900 truncate">
                    {selected.symbol} · {selected.side.toUpperCase()} ON {selected.venue.toUpperCase()}
                  </h3>
                  <p className="text-[11px] font-mono text-slate-500 mt-0.5">
                    {num(selected.externalQty)} @ ${num(selected.markPrice, 2)} ·{" "}
                    <span className={`font-bold ${selected.unrealizedPnlUsd >= 0 ? "text-profit" : "text-loss"}`}>
                      {usd(selected.unrealizedPnlUsd)}
                    </span>{" "}
                    · viewing details only — nothing closes until Confirm
                  </p>
                </div>
                <button
                  type="button"
                  onClick={() => closeDetails()}
                  className="p-1.5 rounded-lg border border-surface-border text-slate-400 hover:text-slate-700 cursor-pointer shrink-0"
                  title="Close popup (Esc)"
                >
                  <X className="w-4 h-4" />
                </button>
              </div>

          {recBusy && <div className="text-xs font-mono text-slate-500">Pricing close against live depth…</div>}
          {recError && (
            <div className="px-3 py-2 rounded-lg border bg-loss-dim/40 border-loss-border text-xs font-mono text-slate-700">
              Quote failed: {recError}
            </div>
          )}
          {rec && (
            <div className="space-y-2.5">
              {/* Recommendation receipt: touch vs VWAP, drift, friction. */}
              <div className="rounded-lg border border-surface-border overflow-hidden bg-surface">
                {[
                  [`Close ${rec.closeSide} ${num(rec.closeQty)}`, `recommended: ${rec.recommendedMode.toUpperCase()}${rec.recommendedLimitPrice != null ? ` @ $${num(rec.recommendedLimitPrice, 2)}` : ""}`],
                  ["Touch price", `$${num(rec.touchPrice, 2)} (spread ${rec.touchSpreadBps.toFixed(1)} bps)`],
                  ["Market VWAP (full size)", `$${num(rec.marketVwap, 2)} · slip ${rec.vwapSlipBps.toFixed(1)} bps${rec.fullSizeFillable ? "" : ` · only ${num(rec.depthFillableQty)} fillable`}`],
                  ["Mid vs account mark", rec.midVsMarkBps != null ? `${rec.midVsMarkBps >= 0 ? "+" : ""}${rec.midVsMarkBps.toFixed(0)} bps drift` : "—"],
                  [`Est. friction (slip + ½spread + ${rec.takerFeeBps}bps fee)`, `${rec.estFrictionBps.toFixed(1)} bps`],
                ].map(([k, v]) => (
                  <div key={k} className="flex items-center justify-between gap-2 px-3 py-1.5 text-xs font-mono odd:bg-surface-raised/50">
                    <span className="text-slate-500">{k}</span>
                    <span className="font-bold tabular-nums text-slate-900 text-right">{v}</span>
                  </div>
                ))}
              </div>
              {rec.warnings.length > 0 && (
                <div className="flex items-start gap-2 text-[11px] font-mono text-warn">
                  <ShieldAlert className="w-3.5 h-3.5 shrink-0 mt-0.5" />
                  <span>{rec.warnings.join(" ")}</span>
                </div>
              )}

              {/* Controls — every slot is always rendered (inputs disable
                  instead of unmounting) so nothing reflows between modes.
                  The taker fee is derived server-side from venue info and
                  shown in the receipt above — not hand-entered. */}
              {/* Row 1 — exit mode. Row 2 — price + size. Row 3 — schedule
                  + action. Fixed rows: nothing reflows between modes (the
                  limit input disables instead of unmounting), and the
                  taker fee is venue-derived server-side (receipt above),
                  not hand-entered. */}
              <div className="flex flex-wrap items-center gap-2">
                <div className="flex gap-1.5" role="group" aria-label="Exit mode">
                  <button
                    type="button"
                    onClick={() => { setMode("auto"); setArmed(false); }}
                    className={tabCls(mode === "auto")}
                    title="Auto Limit: works the global Passive Limit Unwind policy — pegged limit, re-priced on timeout, market fallback so the exit always completes"
                  >
                    Auto Limit
                  </button>
                  <button type="button" onClick={() => { setMode("limit"); setArmed(false); }} className={tabCls(mode === "limit")}>
                    Limit
                  </button>
                  <button type="button" onClick={() => { setMode("market"); setArmed(false); }} className={tabCls(mode === "market")}>
                    Market
                  </button>
                </div>
              </div>
              <div className="flex flex-wrap items-end gap-2">
                <label
                  className="space-y-1"
                  title={
                    mode === "limit"
                      ? "Limit price for the close order"
                      : mode === "auto"
                        ? "Auto Limit prices itself from the global policy — pegged limit → reprice on timeout → market fallback"
                        : "Market mode takes the book — no price needed"
                  }
                >
                  <span className="block text-[11px] font-mono font-bold text-slate-500">LIMIT PRICE</span>
                  <input
                    value={limitPx}
                    onChange={(e) => { setLimitPx(e.target.value.replace(/[^0-9.]/g, "")); setArmed(false); }}
                    disabled={mode !== "limit"}
                    inputMode="decimal"
                    placeholder={mode === "auto" ? "auto" : mode === "market" ? "mkt" : ""}
                    className="w-28 px-2.5 py-1.5 bg-surface border border-surface-border rounded-lg text-xs font-mono font-semibold tabular-nums focus:outline-none focus:ring-1 focus:ring-accent disabled:opacity-40 disabled:cursor-not-allowed"
                  />
                </label>
                <label className="space-y-1" title="Percentage of the external residual to close — computed from a fresh account read at submit (or fire) time">
                  <span className="block text-[11px] font-mono font-bold text-slate-500">CLOSE</span>
                  <span className="relative block w-24">
                    <input
                      value={closePct}
                      onChange={(e) => { setClosePct(e.target.value.replace(/[^0-9.]/g, "")); setArmed(false); }}
                      inputMode="decimal"
                      className={`w-full px-2.5 py-1.5 pr-6 bg-surface border rounded-lg text-xs font-mono font-semibold tabular-nums focus:outline-none focus:ring-1 focus:ring-accent ${pctValid ? "border-surface-border" : "border-loss"}`}
                    />
                    <span className="absolute right-2 top-1/2 -translate-y-1/2 text-[11px] font-mono font-bold text-slate-400 pointer-events-none">%</span>
                  </span>
                </label>
              </div>
              {estCloseQty != null && parsedPct < 100 && (
                <div className="text-[11px] font-mono text-slate-500">
                  ≈ {num(estCloseQty)} {selected.base} of {num(selected.externalQty)} — remainder stays open
                </div>
              )}
              {/* Row 3 — optional schedule + the single action button. Tick
                  SCHEDULE AT to persist a one-shot exit at a local time
                  (same mode, same %, fresh account re-read at fire); leave
                  it off to close now. */}
              <div className="flex flex-wrap items-center gap-2 pt-2 border-t border-surface-border/60">
                <label
                  className="flex items-center gap-1.5 cursor-pointer select-none"
                  title="Defer this exit to a future local time — e.g. hold through a funding timestamp, then close"
                >
                  <input
                    type="checkbox"
                    checked={scheduleOn}
                    onChange={(e) => { setScheduleOn(e.target.checked); setArmed(false); }}
                    className="w-3.5 h-3.5 accent-accent cursor-pointer"
                  />
                  <span className="text-[11px] font-mono font-bold text-slate-500">SCHEDULE AT</span>
                </label>
                <input
                  type="datetime-local"
                  value={scheduleAt}
                  min={toLocalInputValue(Date.now() + 60_000)}
                  step={60}
                  disabled={!scheduleOn}
                  onChange={(e) => { setScheduleAt(e.target.value); setArmed(false); }}
                  className="px-2.5 py-1.5 bg-surface border border-surface-border rounded-lg text-xs font-mono font-semibold focus:outline-none focus:ring-1 focus:ring-accent disabled:opacity-40 disabled:cursor-not-allowed"
                />
                <div className="flex gap-1" title="Quick schedule presets (local time)">
                  {([
                    ["+15m", Date.now() + 15 * 60_000],
                    ["+1h", Date.now() + 3_600_000],
                    ["next :00", Math.ceil((Date.now() + 60_000) / 3_600_000) * 3_600_000],
                  ] as [string, number][]).map(([label, ms]) => (
                    <button
                      key={label}
                      type="button"
                      disabled={!scheduleOn}
                      onClick={() => { setScheduleAt(toLocalInputValue(ms)); setArmed(false); }}
                      className="px-2 py-1 rounded-lg text-[10px] font-mono font-bold border bg-surface text-slate-500 border-surface-border hover:text-slate-900 hover:border-slate-300 disabled:opacity-40 disabled:cursor-not-allowed cursor-pointer"
                    >
                      {label}
                    </button>
                  ))}
                </div>
                {pendingForSelected.map((s) => (
                  <span key={s.id} className="flex items-center gap-1.5 px-2 py-1 rounded-lg bg-accent-dim/50 border border-accent-line/40 text-[10px] font-mono text-slate-700">
                    {s.mode.toUpperCase()} {s.closePct}% @ {new Date(s.executeAtMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}
                    <button
                      type="button"
                      onClick={() => void cancelSchedule(s.id)}
                      className="text-slate-400 hover:text-loss cursor-pointer"
                      title="Cancel this scheduled exit"
                    >
                      <X className="w-3 h-3" />
                    </button>
                  </span>
                ))}
                <div className="ml-auto flex gap-2">
                  <button
                    type="button"
                    onClick={() => closeDetails()}
                    className="px-3 py-2 rounded-lg text-xs font-mono font-bold border bg-surface text-slate-600 border-surface-border hover:text-slate-900 cursor-pointer"
                    title="Close this popup without doing anything"
                  >
                    Cancel
                  </button>
                  <button
                  type="button"
                  onClick={() => void doClose()}
                  disabled={closeDisabledReason != null || closing || !pctValid}
                  className={`px-3 py-2 rounded-lg text-xs font-mono font-bold transition border cursor-pointer disabled:cursor-not-allowed min-w-32 ${
                    armed
                      ? "bg-loss text-white border-loss-border"
                      : "bg-accent text-white border-accent disabled:bg-surface-raised disabled:text-slate-400 disabled:border-surface-border"
                  }`}
                  title={closeDisabledReason ?? (armed
                    ? scheduleOn
                      ? "Click again to persist the scheduled exit"
                      : "Click again to submit the live reduce-only order"
                    : "Arm, then confirm")}
                >
                  {closing
                    ? "Submitting…"
                    : armed
                      ? scheduleOn
                        ? "Confirm Schedule"
                        : "Confirm Close"
                      : scheduleOn
                        ? "Schedule Close"
                        : "Close Positions"}
                  </button>
                </div>
              </div>
              {closeDisabledReason && (
                <div className="text-[11px] font-mono text-slate-500">{closeDisabledReason}</div>
              )}
              {closeError && (
                <div className="px-3 py-2 rounded-lg border bg-loss-dim/40 border-loss-border text-xs font-mono text-slate-700">
                  Close failed: {closeError}
                </div>
              )}
              {result && (
                <div className={`px-3 py-2 rounded-lg border text-xs font-mono break-words ${closeStatusClass(result.status)}`}>
                  {closeStatusLabel(result.status)}: {result.note}
                  {result.orderId && (
                    <span className="text-slate-500" title={result.orderId}>
                      {" "}· order {result.orderId.length > 14 ? `${result.orderId.slice(0, 12)}…` : result.orderId}
                    </span>
                  )}
                </div>
              )}
            </div>
          )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
};
