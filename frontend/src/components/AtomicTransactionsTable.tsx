import React from "react";
import type { AtomicTransactionDto, TradedOpportunityDto } from "../types";
import { VenueLogo } from "./VenueLogo";
import { EmptyState } from "./EmptyState";

export const fmtTime = (ts: string) => {
  const t = Date.parse(ts);
  if (!Number.isFinite(t)) return ts || "—";
  return new Date(t).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
};

export const fmtQty = (v: number) => {
  if (!Number.isFinite(v)) return "—";
  const abs = Math.abs(v);
  const d = abs >= 1000 ? 2 : abs >= 1 ? 4 : 6;
  return v.toLocaleString("en-US", { maximumFractionDigits: d });
};

export const fmtPx = (v: number | null | undefined) =>
  v == null || !Number.isFinite(v) || v <= 0
    ? "—"
    : `$${v.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: Math.abs(v) >= 100 ? 2 : 6 })}`;

/** Signal row status — the durable ledger truth, not a derived guess. */
export const statusChip = (status: TradedOpportunityDto["status"]) => {
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

export const lotChip = (lot: TradedOpportunityDto["lotStatus"]) => {
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

export const roleChip = (role: AtomicTransactionDto["role"]) => {
  switch (role) {
    case "entry":
      return { label: "ENTRY", cls: "bg-accent-dim text-accent-strong border-accent-line/60" };
    case "flatten":
      return { label: "FLATTEN", cls: "bg-warn-dim text-warn-strong border-warn-border" };
    case "trim":
      return { label: "TRIM", cls: "bg-warn-dim text-warn-strong border-warn-border" };
  }
};

export const fillChip = (state: AtomicTransactionDto["fillState"]) => {
  switch (state) {
    case "filled":
      return { label: "Filled", cls: "bg-profit-dim text-profit border-profit-border" };
    case "partial":
      return { label: "Partial", cls: "bg-warn-dim text-warn-strong border-warn-border" };
    case "no_fill":
      return { label: "No Fill", cls: "bg-surface-hover text-slate-500 border-border" };
  }
};

/** Classify instrument into Option (Call/Put), Perpetual, or other. */
export function parseInstrumentType(symbol: string): { isOption: boolean; type: "CALL" | "PUT" | "PERP" | null } {
  const sym = (symbol || "").trim().toUpperCase();
  if (sym.endsWith("-C") || sym.includes("-C-") || sym.includes("-CALL")) {
    return { isOption: true, type: "CALL" };
  }
  if (sym.endsWith("-P") || sym.includes("-P-") || sym.includes("-PUT")) {
    return { isOption: true, type: "PUT" };
  }
  if (sym.includes("PERPETUAL") || sym.includes("PERP")) {
    return { isOption: false, type: "PERP" };
  }
  return { isOption: false, type: null };
}

/** Every dispatched venue order, chronological — entry legs plus all
 *  flatten/trim children the caller merged in. Rows are leg-level orders;
 *  venue/client order ids render as footnotes under the table. */
export const AtomicTransactionsTable: React.FC<{ transactions: AtomicTransactionDto[] }> = ({
  transactions,
}) => (
  <>
    <div className="overflow-x-auto -mx-3.5">
      <table className="w-full text-left text-xs font-mono tabular-nums">
        <thead className="bg-surface-raised/60">
          <tr className="border-b border-surface-border text-slate-500 text-xs font-semibold tracking-wider">
            <th className="py-2 px-3">Time</th>
            <th className="py-2 px-3">Role</th>
            <th className="py-2 px-3">Venue</th>
            <th className="py-2 px-3">Action</th>
            <th className="py-2 px-3">Type</th>
            <th className="py-2 px-3">Instrument</th>
            <th className="py-2 px-3 text-right">Qty Filled / Intended</th>
            <th className="py-2 px-3 text-right">Fill / Intended Px</th>
            <th className="py-2 px-3 text-right">Fee</th>
            <th className="py-2 px-3 text-right">State</th>
          </tr>
        </thead>
        <tbody className="divide-y divide-surface-border">
          {transactions.map((tx) => {
            const role = roleChip(tx.role);
            const fill = fillChip(tx.fillState);
            const scaled = Math.abs(tx.unitScale - 1) > 1e-9;
            const inst = parseInstrumentType(tx.symbol);
            const isBuy = tx.orderSide === "buy";
            const actionText = inst.isOption
              ? (isBuy ? "BOUGHT" : "WROTE")
              : (isBuy ? "BUY" : "SELL");
            const actionColor = inst.isOption
              ? (isBuy ? "text-profit font-bold" : "text-purple-600 dark:text-purple-400 font-bold")
              : (isBuy ? "text-profit font-bold" : "text-loss font-bold");

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
                <td className="py-2 px-3 whitespace-nowrap">
                  <span className={actionColor}>
                    {actionText}
                  </span>
                </td>
                <td className="py-2 px-3 whitespace-nowrap">
                  {inst.type === "CALL" ? (
                    <span className="px-1.5 py-0.5 rounded text-[10px] font-bold bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border border-emerald-500/20">
                      CALL
                    </span>
                  ) : inst.type === "PUT" ? (
                    <span className="px-1.5 py-0.5 rounded text-[10px] font-bold bg-amber-500/10 text-amber-600 dark:text-amber-400 border border-amber-500/20">
                      PUT
                    </span>
                  ) : inst.type === "PERP" ? (
                    <span className="px-1.5 py-0.5 rounded text-[10px] font-semibold bg-sky-500/10 text-sky-600 dark:text-sky-400 border border-sky-500/20">
                      PERP
                    </span>
                  ) : (
                    <span className="text-slate-400 text-[10px] font-mono">—</span>
                  )}
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
          {transactions.length === 0 && (
            <tr>
              <td colSpan={10}>
                <EmptyState plain title="No orders persisted for this signal." />
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
    <div className="space-y-1 pt-1">
      {transactions
        .filter((tx) => tx.venueOrderId || tx.clientOrderId)
        .map((tx) => (
          <div key={`${tx.legId}-ids`} className="text-[10px] font-mono text-slate-400 flex flex-wrap gap-x-3">
            <span>{tx.venue}:{tx.symbol}</span>
            {tx.venueOrderId && <span>venue {tx.venueOrderId}</span>}
            {tx.clientOrderId && <span>client {tx.clientOrderId}</span>}
          </div>
        ))}
    </div>
  </>
);
