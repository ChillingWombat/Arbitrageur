import React, { useEffect, useMemo, useState } from "react";
import { X, ReceiptText, ChevronRight } from "lucide-react";
import { EmptyState } from "./EmptyState";
import { DetailField, DetailSection } from "./DetailBits";
import { AtomicTransactionsTable, fmtTime, statusChip, lotChip } from "./AtomicTransactionsTable";
import { api } from "../services/api";
import { formatStrategyName, STRATEGY_DISPLAY_NAMES } from "../types/tickers";
import type { TradedOpportunityDto } from "../types";

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
            <AtomicTransactionsTable transactions={opp.transactions} />
          </DetailSection>
        </div>
      </div>
    </div>
  );
};

/** Coarse strategy family for filtering: the four options substrategies
 *  collapse into one "Options" category (the nav-tab grouping); every other
 *  id keeps its display name, so unmapped future ids surface as themselves. */
export const strategyCategory = (strategyId: string): string =>
  strategyId.startsWith("options_")
    ? "Options"
    : (STRATEGY_DISPLAY_NAMES[strategyId] ?? strategyId);

/** Status roll-up matching the chip semantics: live rows are "open",
 *  stranded ones need attention, terminal ones split closed/failed. */
export type StatusCategory = "open" | "attention" | "closed" | "failed";
export const statusCategory = (
  status: TradedOpportunityDto["status"],
): StatusCategory => {
  switch (status) {
    case "in_flight":
    case "balanced":
      return "open";
    case "imbalanced":
    case "unresolved":
      return "attention";
    case "closed":
      return "closed";
    case "failed":
      return "failed";
  }
};

/** Pure row filter — exported for tests. `all` passes everything. */
export const filterTradedOpps = (
  opps: TradedOpportunityDto[],
  strategy: string,
  status: string,
): TradedOpportunityDto[] =>
  opps.filter(
    (o) =>
      (strategy === "all" || strategyCategory(o.strategyId) === strategy) &&
      (status === "all" || statusCategory(o.status) === status),
  );

const selectClass =
  "px-2 py-1 bg-surface border border-surface-border rounded-md text-[11px] font-mono text-slate-800 focus:outline-none focus:ring-1 focus:ring-accent cursor-pointer";

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
    <div className="overflow-auto max-h-[26rem]">
      <table className="w-full text-left text-xs font-mono tabular-nums">
        <thead className="sticky top-0 z-10 bg-surface-raised">
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

/** Every opportunity the system actually dispatched (durable signal
 *  ledger — observed-but-untraded radar rows never appear). Each row opens
 *  a detail popup with the full atomic transaction trail. */
export const TradedOpportunitiesGadget: React.FC = () => {
  const [opps, setOpps] = useState<TradedOpportunityDto[] | null>(null);
  const [failed, setFailed] = useState(false);
  const [selected, setSelected] = useState<TradedOpportunityDto | null>(null);
  const [strategyFilter, setStrategyFilter] = useState("all");
  const [statusFilter, setStatusFilter] = useState("all");

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

  const totals = useMemo(() => {
    const rows = opps ?? [];
    let open = 0;
    for (const o of rows) {
      if (o.status !== "closed" && o.status !== "failed") open += 1;
    }
    return { count: rows.length, open };
  }, [opps]);

  // Filter choices derive from the data itself — a category only appears
  // once it has at least one row to show.
  const strategyCategories = useMemo(
    () =>
      [...new Set((opps ?? []).map((o) => strategyCategory(o.strategyId)))].sort(),
    [opps],
  );
  const filtered =
    opps == null ? null : filterTradedOpps(opps, strategyFilter, statusFilter);
  const filtersActive = strategyFilter !== "all" || statusFilter !== "all";

  return (
    <div className="bg-surface rounded-xl border border-surface-border p-4 shadow-card space-y-3">
      <div className="flex items-center justify-between border-b border-surface-border pb-2.5 flex-wrap gap-2">
        <div className="min-w-0">
          <h2 className="text-[13px] font-mono tracking-wider text-slate-900 font-bold flex items-center space-x-2">
            <ReceiptText className="w-5 h-5 text-accent-strong" />
            <span>Traded Opportunities</span>
          </h2>
        </div>
        {totals.open > 0 && (
          <span className="px-1.5 py-0.5 rounded-md text-[11px] font-mono font-bold bg-accent-dim text-accent-strong border border-accent-line/60 shrink-0">
            {totals.open} open
          </span>
        )}
      </div>

      <div className="flex items-center gap-2 flex-wrap">
        <select
          value={strategyFilter}
          onChange={(e) => setStrategyFilter(e.target.value)}
          className={selectClass}
          aria-label="Filter by strategy category"
          title="Filter rows by strategy family (options substrategies group under Options)"
        >
          <option value="all">All strategies</option>
          {strategyCategories.map((c) => (
            <option key={c} value={c}>
              {c}
            </option>
          ))}
        </select>
        <select
          value={statusFilter}
          onChange={(e) => setStatusFilter(e.target.value)}
          className={selectClass}
          aria-label="Filter by status"
          title="Filter rows by lifecycle state"
        >
          <option value="all">All statuses</option>
          <option value="open">Open</option>
          <option value="attention">Needs Attention</option>
          <option value="closed">Closed</option>
          <option value="failed">Failed</option>
        </select>
        {filtersActive && (
          <button
            type="button"
            onClick={() => {
              setStrategyFilter("all");
              setStatusFilter("all");
            }}
            className="px-2 py-1 rounded-md text-[11px] font-mono text-slate-500 hover:text-slate-800 hover:bg-surface-raised transition cursor-pointer"
          >
            Clear
          </button>
        )}
        {filtered != null && (
          <span className="ml-auto text-[10px] font-mono text-slate-400 tabular-nums">
            {filtersActive
              ? `${filtered.length} of ${totals.count} shown`
              : `${totals.count} rows`}
          </span>
        )}
      </div>

      {filtered != null && filtered.length === 0 && filtersActive ? (
        <div className="rounded-lg border border-surface-border bg-surface-raised/40 px-3 py-4 text-center text-xs font-mono text-slate-500">
          No opportunities match the current filters.
        </div>
      ) : (
        <TradedOpportunitiesTable opps={filtered} failed={failed} onSelect={setSelected} />
      )}

      {selectedFresh && (
        <TradedOpportunityDetailModal opp={selectedFresh} onClose={() => setSelected(null)} />
      )}
    </div>
  );
};

export default TradedOpportunitiesGadget;
