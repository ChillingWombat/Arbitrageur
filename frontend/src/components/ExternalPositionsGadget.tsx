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
 *   - Safety is backend-enforced (live mode + unlocked vault + capable venue
 *     + fresh re-read + dust floor); the UI additionally disables the button
 *     with the reason inline so the gates are visible, not mysterious.
 */
import React, { useCallback, useEffect, useMemo, useState } from "react";
import { ArrowDownUp, HandCoins, RefreshCw, ShieldAlert, X } from "lucide-react";
import { EmptyState } from "./EmptyState";
import { api } from "../services/api";
import { groupExternalPositions, type ExternalGroupBy } from "../services/externalPositions";
import type {
  CloseRecommendationDto,
  CloseResultDto,
  ExternalPositionDto,
  ExternalPositionsResponse,
} from "../types";

const usd = (v: number) =>
  `${v >= 0 ? "+" : "-"}$${Math.abs(v).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
const num = (v: number, d = 4) =>
  v == null || !Number.isFinite(v) ? "—" : v.toLocaleString(undefined, { maximumFractionDigits: d });

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
  const [mode, setMode] = useState<"limit" | "market">("limit");
  const [limitPx, setLimitPx] = useState("");
  const [takerBps, setTakerBps] = useState("5");
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

  const loadRec = useCallback(async (p: ExternalPositionDto, feeBps: number) => {
    setRecBusy(true);
    setRecError(null);
    setRec(null);
    try {
      const r = await api.recommendExternalClose(p.venue, p.symbol, feeBps);
      setRec(r);
      setMode(r.recommendedMode === "market" ? "market" : "limit");
      setLimitPx(r.recommendedLimitPrice != null ? String(r.recommendedLimitPrice) : "");
    } catch (e) {
      setRecError(e instanceof Error ? e.message : String(e));
    } finally {
      setRecBusy(false);
    }
  }, []);

  // (Re)quote when the selection or taker-fee assumption changes.
  useEffect(() => {
    setArmed(false);
    setResult(null);
    setCloseError(null);
    if (!selected) {
      setRec(null);
      return;
    }
    const fee = Number(takerBps);
    void loadRec(selected, Number.isFinite(fee) && fee >= 0 ? fee : 5);
  }, [selectedKey, takerBps]); // eslint-disable-line react-hooks/exhaustive-deps

  const doClose = async () => {
    if (!selected || closing) return;
    if (!armed) {
      setArmed(true); // first click arms, second submits
      return;
    }
    setClosing(true);
    setCloseError(null);
    try {
      const r = await api.closeExternalPosition({
        venue: selected.venue,
        symbol: selected.symbol,
        mode,
        limitPrice: mode === "limit" ? Number(limitPx) : null,
      });
      setResult(r);
      setArmed(false);
      void load(); // residual should shrink/disappear on next read
    } catch (e) {
      setCloseError(e instanceof Error ? e.message : String(e));
    } finally {
      setClosing(false);
    }
  };

  const closeDisabledReason = !data
    ? "Loading positions…"
    : !selected
      ? "Select a position above"
      : !data.liveMode
        ? "Paper mode — switch to Live Trading to submit real closes"
        : !selected.closeSupported
          ? `${selected.venue} cannot place live orders through the bot`
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
              By ticker
            </button>
            <button type="button" onClick={() => setGroupBy("venue")} className={tabCls(groupBy === "venue")}>
              By venue
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
                  <button
                    key={p.key}
                    type="button"
                    onClick={() => setSelectedKey(on ? null : p.key)}
                    className={`w-full flex items-center justify-between gap-2 px-3 py-2 text-xs font-mono border-t border-surface-border/60 transition cursor-pointer text-left ${
                      on ? "bg-accent-dim/50" : "hover:bg-surface-raised/50"
                    }`}
                    title={p.closeSupported ? "Select to price a bot close" : "Venue cannot place live orders through the bot"}
                  >
                    <span className="flex items-center gap-2 min-w-0">
                      <span className={`px-1.5 py-0.5 rounded-md text-[10px] font-bold ${p.side === "long" ? "bg-profit-dim text-profit" : "bg-loss-dim text-loss"}`}>
                        {p.side.toUpperCase()}
                      </span>
                      <span className="font-bold text-slate-900 truncate">{p.symbol}</span>
                      <span className="text-slate-400">{p.venue}</span>
                      {!p.closeSupported && <span className="text-warn text-[10px] font-bold">NO BOT CLOSE</span>}
                    </span>
                    <span className="tabular-nums text-slate-700 shrink-0">
                      {num(p.externalQty)} @ ${num(p.markPrice, 2)}
                      {" · "}
                      <span className={`font-bold ${p.unrealizedPnlUsd >= 0 ? "text-profit" : "text-loss"}`}>{usd(p.unrealizedPnlUsd)}</span>
                    </span>
                  </button>
                );
              })}
            </div>
          ))}
        </div>
      )}

      {/* Close panel for the selected position. */}
      {selected && (
        <div className="rounded-xl border border-accent-line/60 bg-accent-dim/30 p-3.5 space-y-3">
          <div className="flex items-center justify-between gap-2">
            <h3 className="text-xs font-mono font-bold tracking-wider text-slate-900">
              CLOSE {selected.base} {selected.side.toUpperCase()} ON {selected.venue.toUpperCase()} — {num(selected.externalQty)} @ ${num(selected.markPrice, 2)}
            </h3>
            <button
              type="button"
              onClick={() => setSelectedKey(null)}
              className="p-1 rounded-md text-slate-400 hover:text-slate-700 cursor-pointer"
              title="Deselect"
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

              {/* Mode + limit + fee controls, then the guarded submit. */}
              <div className="flex flex-wrap items-end gap-2">
                <div className="flex gap-1.5">
                  <button type="button" onClick={() => { setMode("limit"); setArmed(false); }} className={tabCls(mode === "limit")}>
                    Limit
                  </button>
                  <button type="button" onClick={() => { setMode("market"); setArmed(false); }} className={tabCls(mode === "market")}>
                    Market
                  </button>
                </div>
                {mode === "limit" && (
                  <label className="space-y-1">
                    <span className="text-[11px] font-mono font-bold text-slate-500">LIMIT PRICE</span>
                    <input
                      value={limitPx}
                      onChange={(e) => { setLimitPx(e.target.value.replace(/[^0-9.]/g, "")); setArmed(false); }}
                      inputMode="decimal"
                      className="w-28 px-2.5 py-1.5 bg-surface border border-surface-border rounded-lg text-xs font-mono font-semibold tabular-nums focus:outline-none focus:ring-1 focus:ring-accent"
                    />
                  </label>
                )}
                <label className="space-y-1" title="Taker-fee assumption for the friction estimate (verify on the venue fee schedule)">
                  <span className="text-[11px] font-mono font-bold text-slate-500">TAKER FEE (BPS)</span>
                  <input
                    value={takerBps}
                    onChange={(e) => setTakerBps(e.target.value.replace(/[^0-9.]/g, ""))}
                    inputMode="decimal"
                    className="w-20 px-2.5 py-1.5 bg-surface border border-surface-border rounded-lg text-xs font-mono font-semibold tabular-nums focus:outline-none focus:ring-1 focus:ring-accent"
                  />
                </label>
                <button
                  type="button"
                  onClick={() => void doClose()}
                  disabled={closeDisabledReason != null || closing}
                  className={`px-3 py-2 rounded-lg text-xs font-mono font-bold transition border cursor-pointer disabled:cursor-not-allowed ${
                    armed
                      ? "bg-loss text-white border-loss-border"
                      : "bg-accent text-white border-accent disabled:bg-surface-raised disabled:text-slate-400 disabled:border-surface-border"
                  }`}
                  title={closeDisabledReason ?? (armed ? "Click again to submit the live reduce-only order" : "Arm the close, then confirm")}
                >
                  {closing ? "Submitting…" : armed ? `Confirm ${mode.toUpperCase()} close` : `Close ${mode.toUpperCase()} via bot`}
                </button>
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
                <div className="px-3 py-2 rounded-lg border bg-profit-dim/40 border-profit-border text-xs font-mono text-slate-900">
                  {result.status === "filled" ? "Closed" : "Submitted"}: {result.note}
                  {result.orderId && <span className="text-slate-500"> · order {result.orderId}</span>}
                </div>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
};
