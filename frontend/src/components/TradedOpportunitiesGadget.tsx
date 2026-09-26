import React, { useEffect, useMemo, useState } from "react";
import { X, ReceiptText, ChevronRight } from "lucide-react";
import { EmptyState } from "./EmptyState";
import { VenueLogo } from "./VenueLogo";
import { DetailField, DetailSection } from "./DetailBits";
import { api } from "../services/api";
import { formatStrategyName } from "../types/tickers";
import type {
  AtomicTransactionDto,
  SystemStatusDto,
  TradedOpportunityDto,
} from "../types";

const usd = (v: number, digits = 2) => {
  if (!Number.isFinite(v)) return "—";
  if (Math.abs(v) < 0.005) return "$0.00";
  return `${v >= 0 ? "+" : "-"}$${Math.abs(v).toLocaleString("en-US", { minimumFractionDigits: digits, maximumFractionDigits: digits })}`;
};

const pnlTone = (v: number | null | undefined) =>
  v == null || !Number.isFinite(v) || Math.abs(v) < 0.005
    ? "text-slate-500"
    : v > 0
      ? "text-profit"
      : "text-loss";

const fmtTime = (ts: string) => {
  const t = Date.parse(ts);
  if (!Number.isFinite(t)) return ts || "—";
  return new Date(t).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
};

const fmtQty = (v: number) => {
  if (!Number.isFinite(v)) return "—";
  const abs = Math.abs(v);
  const d = abs >= 1000 ? 2 : abs >= 1 ? 4 : 6;
  return v.toLocaleString("en-US", { maximumFractionDigits: d });
};

const fmtPx = (v: number | null | undefined) =>
  v == null || !Number.isFinite(v) || v <= 0
    ? "—"
    : `$${v.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: Math.abs(v) >= 100 ? 2 : 6 })}`;

/** Signal row status — the durable ledger truth, not a derived guess. */
const statusChip = (status: TradedOpportunityDto["status"]) => {
  switch (status) {
    case "closed":
      return { label: "Closed", cls: "bg-profit-dim text-profit border-profit-border" };
    case "balanced":
      return { label: "Open", cls: "bg-accent-dim text-accent-strong border-accent-line/60" };
    case "in_flight":
      return { label: "In Flight", cls: "bg-accent-dim text-accent-strong border-accent-line/60" };
    case "imbalanced":
      return { label: "Imbalanced", cls: "bg-warn-dim text-warn-strong border-warn-border" };
    case "unresolved":
      return { label: "Unresolved", cls: "bg-loss-dim text-loss border-loss-border" };
    case "failed":
      return { label: "Failed", cls: "bg-loss-dim text-loss border-loss-border" };
  }
};

const lotChip = (lot: TradedOpportunityDto["lotStatus"]) => {
  switch (lot) {
    case "open":
      return { label: "Lot Open", cls: "bg-accent-dim text-accent-strong border-accent-line/60" };
    case "closed":
      return { label: "Lot Closed", cls: "bg-profit-dim text-profit border-profit-border" };
    case "stopped_out":
      return { label: "Stopped Out", cls: "bg-warn-dim text-warn-strong border-warn-border" };
    case "timed_out":
      return { label: "Timed Out", cls: "bg-warn-dim text-warn-strong border-warn-border" };
    default:
      return null;
  }
};

const roleChip = (role: AtomicTransactionDto["role"]) => {
  switch (role) {
    case "entry":
      return { label: "ENTRY", cls: "bg-accent-dim text-accent-strong border-accent-line/60" };
    case "flatten":
      return { label: "FLATTEN", cls: "bg-warn-dim text-warn-strong border-warn-border" };
    case "trim":
      return { label: "TRIM", cls: "bg-warn-dim text-warn-strong border-warn-border" };
  }
};

const fillChip = (state: AtomicTransactionDto["fillState"]) => {
  switch (state) {
    case "filled":
      return { label: "Filled", cls: "bg-profit-dim text-profit border-profit-border" };
    case "partial":
      return { label: "Partial", cls: "bg-warn-dim text-warn-strong border-warn-border" };
    case "no_fill":
      return { label: "No Fill", cls: "bg-surface-hover text-slate-500 border-border" };
  }
};

/** Detail popup: opportunity facts, expected-vs-realized economics, and the
 *  full atomic transaction trail (entry orders + every flatten/trim order). */
export const TradedOpportunityDetailModal: React.FC<{
  opp: TradedOpportunityDto;
  onClose: () => void;
}> = ({ opp, onClose }) => {
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  const chip = statusChip(opp.status);
  const lot = lotChip(opp.lotStatus);
  const realized = opp.realizedPnlUsd;
  const route =
    opp.venueLong != null || opp.venueShort != null
      ? `${opp.venueLong ?? "?"} long ↔ ${opp.venueShort ?? "?"} short`
      : "—";

  return (
    <div
      className="fixed inset-0 z-50 bg-slate-900/50 backdrop-blur-sm flex items-center justify-center p-3 sm:p-6 select-none"
      role="dialog"
      aria-modal="true"
      aria-label={`Opportunity details — ${opp.symbol}`}
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="bg-surface border border-surface-border rounded-2xl w-[960px] max-w-[96vw] max-h-[90vh] flex flex-col shadow-2xl overflow-hidden">
        <div className="flex items-center justify-between px-6 py-3.5 border-b border-surface-border bg-surface shrink-0">
          <div className="flex items-center gap-2 min-w-0 flex-wrap">
            <span className="px-2 py-0.5 rounded-md text-xs font-sans font-semibold bg-accent-dim text-accent-strong border border-accent-line">
              Traded Opportunity
            </span>
            <span className="text-sm font-bold font-sans text-slate-900">{opp.symbol}</span>
            <span className="text-xs font-sans text-slate-500">{formatStrategyName(opp.strategyId)}</span>
            <span className={`px-2 py-0.5 rounded-md text-[11px] font-mono font-bold border ${chip.cls}`}>
              {chip.label}
            </span>
            {lot && (
              <span className={`px-2 py-0.5 rounded-md text-[11px] font-mono font-bold border ${lot.cls}`}>
                {lot.label}
              </span>
            )}
          </div>
          <button
            type="button"
            onClick={onClose}
            className="p-1.5 rounded-lg text-muted hover:text-slate-700 hover:bg-slate-100 transition cursor-pointer"
            title="Close details (ESC)"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto p-5 space-y-5">
          <DetailSection title="Opportunity">
            <DetailField label="Signal" value={<span className="text-xs">{opp.signalId}</span>} title={opp.signalId} />
            <DetailField label="Route" value={route} />
            <DetailField label="Environment" value={opp.environment || "unknown"} />
            <DetailField label="Dispatched" value={fmtTime(opp.createdAt)} />
            <DetailField
              label="Closed"
              value={opp.closedAt ? fmtTime(opp.closedAt) : "—"}
            />
            <DetailField
              label="Dispatches"
              value={String(1 + opp.childCount)}
              tip="The entry signal plus its close-family dispatches (flatten/trim)."
            />
          </DetailSection>

          <DetailSection title="Economics">
            <DetailField
              label="Expected Net"
              value={opp.expectedNetUsd != null ? usd(opp.expectedNetUsd) : "—"}
              tip="Dispatch-time expected net edge in USD."
            />
            <DetailField
              label="Realized PnL"
              value={
                <span className={pnlTone(realized)}>{realized != null ? usd(realized) : "—"}</span>
              }
              tip="Closed cycle-lot P&L when a lot exists; leg cash-flow for terminal lotless families; blank while the position may still be open."
            />
            <DetailField
              label="Realized (bps)"
              value={opp.realizedPnlBps != null ? `${opp.realizedPnlBps.toFixed(2)} bps` : "—"}
            />
            <DetailField
              label="Entry Spread"
              value={opp.entrySpreadBps != null ? `${opp.entrySpreadBps.toFixed(1)} bps` : "—"}
            />
            <DetailField
              label="Close Spread"
              value={opp.closeSpreadBps != null ? `${opp.closeSpreadBps.toFixed(1)} bps` : "—"}
            />
            <DetailField
              label="Total Fees"
              value={`-$${opp.totalFeesUsd.toFixed(2)}`}
              tip="Sum of fees charged on every family leg fill."
            />
            <DetailField
              label="Leg Cash-Flow"
              value={<span className={pnlTone(opp.legNetFlowUsd)}>{usd(opp.legNetFlowUsd)}</span>}
              tip="Signed cash-flow across all atomic transactions (sells − buys − fees)."
            />
          </DetailSection>

          {opp.note && (
            <div className="rounded-lg border border-warn-border bg-warn-dim/40 px-3 py-2 text-xs font-mono text-warn-strong">
              {opp.note}
            </div>
          )}

          <DetailSection
            title={`Atomic Transactions (${opp.transactions.length})`}
            gridClassName="block"
            tip="Every dispatched venue order for this opportunity — entry legs plus all flatten/trim children, in chronological order."
          >
            <div className="overflow-x-auto -mx-3.5">
              <table className="w-full text-left text-xs font-mono tabular-nums">
                <thead className="bg-surface-raised/60">
                  <tr className="border-b border-surface-border text-slate-500 text-xs font-semibold tracking-wider">
                    <th className="py-2 px-3">Time</th>
                    <th className="py-2 px-3">Role</th>
                    <th className="py-2 px-3">Venue</th>
                    <th className="py-2 px-3">Order</th>
                    <th className="py-2 px-3">Instrument</th>
                    <th className="py-2 px-3 text-right">Qty Filled / Intended</th>
                    <th className="py-2 px-3 text-right">Fill / Intended Px</th>
                    <th className="py-2 px-3 text-right">Fee</th>
                    <th className="py-2 px-3 text-right">State</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-surface-border">
                  {opp.transactions.map((tx) => {
                    const role = roleChip(tx.role);
                    const fill = fillChip(tx.fillState);
                    const scaled = Math.abs(tx.unitScale - 1) > 1e-9;
                    return (
                      <tr key={tx.legId} className="hover:bg-surface-raised/40 transition">
                        <td className="py-2 px-3 text-slate-600 whitespace-nowrap">{fmtTime(tx.createdAt)}</td>
                        <td className="py-2 px-3">
                          <span className={`px-1.5 py-0.5 rounded-md text-[10px] font-bold border ${role.cls}`}>
                            {role.label}
                          </span>
                        </td>
                        <td className="py-2 px-3 whitespace-nowrap">
                          <span className="inline-flex items-center gap-1">
                            <VenueLogo venueId={(tx.venue ?? "").toLowerCase()} name={tx.venue} size={13} />
                            {tx.venue}
                          </span>
                        </td>
                        <td className="py-2 px-3">
                          <span className={tx.orderSide === "buy" ? "text-profit font-bold" : "text-loss font-bold"}>
                            {tx.orderSide.toUpperCase()}
                          </span>
                          <span className="text-slate-400"> /{tx.side}</span>
                        </td>
                        <td className="py-2 px-3 text-slate-700 whitespace-nowrap">
                          {tx.symbol}
                          {scaled && (
                            <span className="text-slate-400" title={`${tx.unitScale} venue-units per parent unit`}>
                              ×{tx.unitScale.toPrecision(3)}
                            </span>
                          )}
                        </td>
                        <td className="py-2 px-3 text-right text-slate-700 whitespace-nowrap">
                          {fmtQty(tx.filledQty)} / {fmtQty(tx.intendedQty)}
                        </td>
                        <td className="py-2 px-3 text-right text-slate-700 whitespace-nowrap">
                          {fmtPx(tx.fillPrice)} / {fmtPx(tx.intendedPrice)}
                        </td>
                        <td className="py-2 px-3 text-right text-slate-700 whitespace-nowrap">
                          {tx.feeUsd > 0 ? `-$${tx.feeUsd.toFixed(3)}` : "$0"}
                        </td>
                        <td className="py-2 px-3 text-right whitespace-nowrap">
                          <span className={`px-1.5 py-0.5 rounded-md text-[10px] font-bold border ${fill.cls}`}>
                            {fill.label}
                          </span>
                        </td>
                      </tr>
                    );
                  })}
                  {opp.transactions.length === 0 && (
                    <tr>
                      <td colSpan={9}>
                        <EmptyState plain title="No orders persisted for this signal." />
                      </td>
                    </tr>
                  )}
                </tbody>
              </table>
            </div>
            <div className="space-y-1 pt-1">
              {opp.transactions
                .filter((tx) => tx.venueOrderId || tx.clientOrderId)
                .map((tx) => (
                  <div key={`${tx.legId}-ids`} className="text-[10px] font-mono text-slate-400 flex flex-wrap gap-x-3">
                    <span>{tx.venue}:{tx.symbol}</span>
                    {tx.venueOrderId && <span>venue {tx.venueOrderId}</span>}
                    {tx.clientOrderId && <span>client {tx.clientOrderId}</span>}
                  </div>
                ))}
            </div>
          </DetailSection>
        </div>
      </div>
    </div>
  );
};

/** The traded-opportunity rows table — exported for static render tests.
 *  `opps === null` renders the loading state. */
export const TradedOpportunitiesTable: React.FC<{
  opps: TradedOpportunityDto[] | null;
  failed: boolean;
  onSelect: (opp: TradedOpportunityDto) => void;
}> = ({ opps, failed, onSelect }) => (
  <>
    {failed && (
      <div className="rounded-lg border border-warn-border bg-warn-dim/40 px-3 py-2 text-xs font-mono text-warn-strong">
        Traded-opportunity ledger read failed — showing last state is not truthful here; check backend.
      </div>
    )}
    <div className="overflow-x-auto">
      <table className="w-full text-left text-xs font-mono tabular-nums">
        <thead className="bg-surface-raised/60">
          <tr className="border-b border-surface-border text-slate-500 text-xs font-semibold tracking-wider">
            <th className="py-2.5 px-3">Dispatched</th>
            <th className="py-2.5 px-3">Opportunity</th>
            <th className="py-2.5 px-3">Route</th>
            <th className="py-2.5 px-3 text-right">Expected</th>
            <th className="py-2.5 px-3 text-right">Realized</th>
            <th className="py-2.5 px-3 text-right">Orders</th>
            <th className="py-2.5 px-3 text-right">Status</th>
          </tr>
        </thead>
        <tbody className="divide-y divide-surface-border">
          {(opps ?? []).map((o) => {
            const chip = statusChip(o.status);
            const route =
              o.venueLong != null || o.venueShort != null
                ? `${o.venueLong ?? "?"} ↔ ${o.venueShort ?? "?"}`
                : "—";
            return (
              <tr
                key={o.signalId}
                onClick={() => onSelect(o)}
                className="hover:bg-surface-raised/60 transition cursor-pointer"
                title="Show opportunity detail and atomic transactions"
              >
                <td className="py-2.5 px-3 text-slate-600 whitespace-nowrap">{fmtTime(o.createdAt)}</td>
                <td className="py-2.5 px-3 whitespace-nowrap">
                  <button
                    type="button"
                    onClick={(e) => {
                      e.stopPropagation();
                      onSelect(o);
                    }}
                    className="flex items-center gap-2 min-w-0 text-left rounded-md focus:outline-none focus-visible:ring-2 focus-visible:ring-accent-line cursor-pointer group"
                    aria-label={`Open opportunity details — ${o.symbol} ${formatStrategyName(o.strategyId)}`}
                    title="Show opportunity detail and atomic transactions"
                  >
                    <span className="font-bold text-slate-900 group-hover:underline underline-offset-2">{o.symbol}</span>
                    <span className="text-slate-500">{formatStrategyName(o.strategyId)}</span>
                    <ChevronRight className="w-3.5 h-3.5 text-slate-400 group-hover:text-accent-strong transition shrink-0" />
                  </button>
                </td>
                <td className="py-2.5 px-3 text-slate-700 whitespace-nowrap">{route}</td>
                <td className="py-2.5 px-3 text-right text-slate-700">
                  {o.expectedNetUsd != null ? usd(o.expectedNetUsd) : "—"}
                </td>
                <td className={`py-2.5 px-3 text-right font-bold ${pnlTone(o.realizedPnlUsd)}`}>
                  {o.realizedPnlUsd != null ? usd(o.realizedPnlUsd) : "—"}
                </td>
                <td className="py-2.5 px-3 text-right text-slate-600">
                  {o.transactions.length}
                  {o.childCount > 0 && (
                    <span className="text-slate-400" title={`${o.childCount} close-family dispatch(es)`}>
                      {" "}·{o.childCount + 1}×
                    </span>
                  )}
                </td>
                <td className="py-2.5 px-3 text-right whitespace-nowrap">
                  <span className={`px-2 py-0.5 rounded-full text-xs font-bold border ${chip.cls}`}>
                    {chip.label}
                  </span>
                </td>
              </tr>
            );
          })}
          {opps == null && !failed && (
            <tr>
              <td colSpan={7}>
                <EmptyState plain title="Loading traded opportunities…" />
              </td>
            </tr>
          )}
          {opps != null && opps.length === 0 && (
            <tr>
              <td colSpan={7}>
                <EmptyState
                  title="Nothing Traded Yet"
                  hint="Signals the engine dispatches land here with their full order trail."
                />
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  </>
);

interface TradedOpportunitiesGadgetProps {
  status: SystemStatusDto | null;
}

/** Net Profit + every opportunity the system actually dispatched (durable
 *  signal ledger — observed-but-untraded radar rows never appear). Each row
 *  opens a detail popup with the full atomic transaction trail. */
export const TradedOpportunitiesGadget: React.FC<TradedOpportunitiesGadgetProps> = ({
  status,
}) => {
  const [opps, setOpps] = useState<TradedOpportunityDto[] | null>(null);
  const [failed, setFailed] = useState(false);
  const [selected, setSelected] = useState<TradedOpportunityDto | null>(null);

  useEffect(() => {
    let alive = true;
    const load = async () => {
      try {
        const rows = await api.listTradedOpportunities();
        if (!alive) return;
        setOpps(Array.isArray(rows) ? rows : []);
        setFailed(false);
      } catch (err) {
        if (!alive) return;
        console.error("Failed to load traded opportunities:", err);
        setFailed(true);
      }
    };
    load();
    const interval = setInterval(load, 3000);
    return () => {
      alive = false;
      clearInterval(interval);
    };
  }, []);

  // Keep the open popup tracking the freshest copy of its opportunity.
  const selectedFresh = useMemo(
    () =>
      selected == null
        ? null
        : (opps ?? []).find((o) => o.signalId === selected.signalId) ?? selected,
    [selected, opps],
  );

  const netProfit = status?.total_realized_pnl;
  const totals = useMemo(() => {
    const rows = opps ?? [];
    let realized = 0;
    let realizedKnown = false;
    let open = 0;
    for (const o of rows) {
      if (o.realizedPnlUsd != null) {
        realized += o.realizedPnlUsd;
        realizedKnown = true;
      }
      if (o.status !== "closed" && o.status !== "failed") open += 1;
    }
    return { count: rows.length, realized, realizedKnown, open };
  }, [opps]);

  return (
    <div className="bg-surface rounded-xl border border-surface-border p-4 shadow-card space-y-3">
      <div className="flex items-center justify-between border-b border-surface-border pb-2.5 flex-wrap gap-2">
        <div className="min-w-0">
          <h2 className="text-[13px] font-mono tracking-wider text-slate-900 font-bold flex items-center space-x-2">
            <ReceiptText className="w-5 h-5 text-accent-strong" />
            <span>Traded Opportunities</span>
          </h2>
          <p className="text-xs text-slate-600 font-mono mt-1">
            Every signal the engine dispatched — click a row for the atomic order trail.
          </p>
        </div>
        <div className="flex items-center gap-4 shrink-0">
          {totals.open > 0 && (
            <span className="px-1.5 py-0.5 rounded-md text-[11px] font-mono font-bold bg-accent-dim text-accent-strong border border-accent-line/60">
              {totals.open} open
            </span>
          )}
          <div className="text-right">
            <div className="text-[10px] font-mono uppercase tracking-wider text-slate-500">Net Profit</div>
            <div className={`text-lg font-bold font-mono tabular-nums leading-tight ${pnlTone(netProfit)}`}>
              {netProfit != null ? usd(netProfit) : "—"}
            </div>
            {totals.realizedKnown && (
              <div className="text-[10px] font-mono text-slate-400 tabular-nums" title="Sum of realized P&L over the listed opportunities. Header Net Profit is the engine counter and includes unattributed trims.">
                rows {usd(totals.realized)}
              </div>
            )}
          </div>
        </div>
      </div>

      <TradedOpportunitiesTable opps={opps} failed={failed} onSelect={setSelected} />

      {selectedFresh && (
        <TradedOpportunityDetailModal opp={selectedFresh} onClose={() => setSelected(null)} />
      )}
    </div>
  );
};

export default TradedOpportunitiesGadget;
