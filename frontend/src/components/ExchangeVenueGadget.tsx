import React from "react";
import { Building2 } from "lucide-react";
import type { ExchangeRowStats } from "../services/exchangeStats";
import { EmptyState } from "./EmptyState";

function fmtMoney(v: number | null): string {
  if (v == null || !Number.isFinite(v)) return "—";
  return v >= 0 ? `+$${v.toFixed(2)}` : `-$${Math.abs(v).toFixed(2)}`;
}

function fmtPct(v: number | null): string {
  if (v == null || !Number.isFinite(v)) return "—";
  return `${v.toFixed(1)}%`;
}

/**
 * Per-exchange stats gadget for the Portfolio Monitor. Every figure reuses
 * backend IPC (`get_execution_logs`, `get_venue_balances`,
 * `get_utilisation_history`): a two-leg trade counts on both of its venues,
 * totals sum fee-inclusive net profit, and the 24h average utilisation is
 * the time-weighted mean of retained margin snapshots — "—" with the reason
 * in the tooltip while history is absent, never a fabricated window.
 */
export const ExchangeVenueGadget: React.FC<{ rows: ExchangeRowStats[] | null }> = ({ rows }) => (
  <div className="bg-surface rounded-xl border border-surface-border p-4 shadow-card space-y-3">
    <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
      <div>
        <h2 className="text-[13px] font-mono tracking-wider text-slate-900 font-bold flex items-center space-x-2">
          <Building2 className="w-5 h-5 text-accent" />
          <span>Per-Exchange Stats</span>
        </h2>
        <p className="text-xs text-slate-600 font-mono mt-1">
          Two-leg trades count on both venues · totals sum fee-inclusive net profit · utilisation is used margin / equity.
        </p>
      </div>
      <span className="text-xs font-mono font-bold text-slate-600">
        {rows == null ? "—" : `${rows.length} ${rows.length === 1 ? "Venue" : "Venues"}`}
      </span>
    </div>

    {rows == null || rows.length === 0 ? (
      <EmptyState
        title="No Exchange Data"
        hint="Venue balances and execution logs are unavailable."
      />
    ) : (
      <div className="overflow-x-auto">
        <table className="w-full text-left text-xs font-mono tabular-nums">
          <thead className="bg-surface-raised/60">
            <tr className="border-b border-surface-border text-slate-500 text-xs font-semibold tracking-wider">
              <th className="py-2.5 px-3">Venue</th>
              <th className="py-2.5 px-3 text-right" title="Execution events involving this venue">Trades</th>
              <th className="py-2.5 px-3 text-right" title="Total net profit / trades">Avg PnL / trade</th>
              <th className="py-2.5 px-3 text-right" title="Sum of fee-inclusive net profit">Total PnL</th>
              <th className="py-2.5 px-3 text-right" title="Current venue equity">Equity</th>
              <th className="py-2.5 px-3 text-right" title="Used margin / equity, current snapshot">Utilisation</th>
              <th className="py-2.5 px-3 text-right" title="Time-averaged utilisation over 24h">24h avg util</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-surface-border">
            {rows.map((r) => (
              <tr key={r.venue} className="hover:bg-surface-raised/40 transition">
                <td className="py-2.5 px-3 font-bold text-sm text-slate-900">{r.venue}</td>
                <td className="py-2.5 px-3 text-right text-slate-700">{r.trades}</td>
                <td className="py-2.5 px-3 text-right">
                  <span className={`font-bold ${r.avgPnlUsd == null ? "text-slate-400" : r.avgPnlUsd >= 0 ? "text-profit" : "text-loss"}`}>
                    {r.avgPnlUsd == null ? "—" : fmtMoney(r.avgPnlUsd)}
                  </span>
                </td>
                <td className="py-2.5 px-3 text-right">
                  <span className={`font-bold ${r.totalPnlUsd >= 0 ? "text-profit" : "text-loss"}`}>
                    {fmtMoney(r.totalPnlUsd)}
                  </span>
                </td>
                <td className="py-2.5 px-3 text-right text-slate-800" title={r.equityUsd == null ? "No balance snapshot for this venue." : undefined}>
                  {r.equityUsd == null ? "—" : `$${r.equityUsd.toFixed(2)}`}
                </td>
                <td className="py-2.5 px-3 text-right text-slate-800" title={r.utilisationPct == null ? "No balance snapshot for this venue." : undefined}>
                  {fmtPct(r.utilisationPct)}
                </td>
                <td className="py-2.5 px-3 text-right text-slate-400" title={r.avgUtilisationNote ?? undefined}>
                  {r.avgUtilisationPct == null ? "—" : fmtPct(r.avgUtilisationPct)}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    )}
  </div>
);
