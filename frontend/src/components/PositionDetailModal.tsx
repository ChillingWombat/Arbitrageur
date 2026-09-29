import React, { useEffect, useMemo, useState } from "react";
import { X } from "lucide-react";
import type { AtomicTransactionDto, OpenPositionDto, TradedOpportunityDto } from "../types";
import { VenueLogo } from "./VenueLogo";
import { DetailField, DetailSection } from "./DetailBits";
import { EmptyState } from "./EmptyState";
import { api } from "../services/api";
import { formatStrategyName } from "../types/tickers";
import { formatDuration } from "../services/matrixQuant";
import {
  AtomicTransactionsTable,
  fmtTime,
  statusChip,
  roleChip,
} from "./AtomicTransactionsTable";

export interface PositionDetailModalProps {
  pos: OpenPositionDto;
  onClose: () => void;
}

const usd = (v: number, digits = 2) =>
  `${v >= 0 ? "+" : "-"}$${Math.abs(v).toLocaleString("en-US", {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  })}`;

const pnlTone = (v: number | null | undefined) =>
  v == null || !Number.isFinite(v) || Math.abs(v) < 0.005
    ? "text-slate-500"
    : v > 0
      ? "text-profit"
      : "text-loss";

const fmtQty4 = (q: number): string => {
  const abs = Math.abs(q);
  const trimmed = abs.toFixed(abs >= 100 ? 2 : 4).replace(/\.?0+$/, "");
  return `${q < 0 ? "-" : "+"}${trimmed === "" ? "0" : trimmed}`;
};

/** Backend `signal_trade_id` mirror: `-legN` siblings share the cycle
 *  prefix; the tail must be numeric so a coincidental `-leg` can't forge a
 *  shared prefix. */
const tradeIdOf = (id: string): string => id.replace(/-leg\d+$/, "");

/** `flat-{parent}-NNNN` / `trim-{parent}-NNNN` → parent signal id. */
const closeChildParentId = (id: string): string | null => {
  const rest = id.startsWith("flat-")
    ? id.slice(5)
    : id.startsWith("trim-")
      ? id.slice(5)
      : null;
  if (rest == null) return null;
  const i = rest.lastIndexOf("-");
  return i > 0 ? rest.slice(0, i) : null;
};

const signalRole = (id: string): AtomicTransactionDto["role"] =>
  id.startsWith("flat-") ? "flatten" : id.startsWith("trim-") ? "trim" : "entry";

/** True when the opportunity row belongs to this position's trade family:
 *  an entry sibling (shared trade id) or a flatten/trim child of one. */
const isFamilyMember = (o: TradedOpportunityDto, pos: OpenPositionDto): boolean => {
  if (o.strategyId.toLowerCase() !== pos.strategyId.toLowerCase()) return false;
  if (tradeIdOf(o.signalId) === pos.tradeId) return true;
  const pid = closeChildParentId(o.signalId);
  return pid != null && tradeIdOf(pid) === pos.tradeId;
};

/** Position detail: facts, live exposure, per-signal lifecycle, the merged
 *  atomic transaction trail for the whole trade family, and the net-PnL
 *  trend (solid realized → dashed illustrative 24h projection when the
 *  backend supplies expectedNetUsd — never fabricated). */
export const PositionDetailModal: React.FC<PositionDetailModalProps> = React.memo(({ pos, onClose }) => {
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  const [family, setFamily] = useState<TradedOpportunityDto[] | null>(null);
  const [familyFailed, setFamilyFailed] = useState(false);
  useEffect(() => {
    let alive = true;
    void api
      .listTradedOpportunities()
      .then((rows) => {
        if (alive) setFamily(rows.filter((o) => isFamilyMember(o, pos)));
      })
      .catch(() => {
        if (alive) setFamilyFailed(true);
      });
    return () => {
      alive = false;
    };
  }, [pos]);

  const members = useMemo(
    () =>
      (family ?? [])
        .slice()
        .sort((a, b) => a.createdAt.localeCompare(b.createdAt)),
    [family],
  );
  const transactions = useMemo(() => {
    const byLeg = new Map<string, AtomicTransactionDto>();
    for (const m of members)
      for (const tx of m.transactions) byLeg.set(tx.legId, tx);
    return [...byLeg.values()].sort((a, b) => a.createdAt.localeCompare(b.createdAt));
  }, [members]);
  const familyFees = useMemo(
    () => members.reduce((s, m) => s + m.totalFeesUsd, 0),
    [members],
  );
  const familyExpected = useMemo(() => {
    const vals = members
      .filter((m) => signalRole(m.signalId) === "entry")
      .map((m) => m.expectedNetUsd)
      .filter((v): v is number => v != null && Number.isFinite(v));
    return vals.length ? vals.reduce((s, v) => s + v, 0) : null;
  }, [members]);

  const fees = pos.feesUsd ?? 0;
  const nowPnl = pos.unrealizedPnlUsd ?? -fees;
  const entryPnl = -fees;
  const expectedNet = familyExpected ?? pos.expectedNetUsd;
  const hasProjection = expectedNet != null && Number.isFinite(expectedNet);
  // Chart geometry: x in [t0, t1, t2], y auto-scaled with padding.
  const W = 600, H = 180, PAD = 28;
  const t0 = Date.parse(pos.openedAt);
  const t1 = Date.now();
  const t2 = t1 + 24 * 3600 * 1000;
  const t0safe = Number.isFinite(t0) ? t0 : t1 - 3600 * 1000;
  const yEnd = hasProjection ? nowPnl + (expectedNet as number) : nowPnl;
  const lo = Math.min(entryPnl, nowPnl, yEnd);
  const hi = Math.max(entryPnl, nowPnl, yEnd);
  const span = hi - lo > 1e-9 ? hi - lo : 1;
  const xPos = (t: number) => PAD + ((t - t0safe) / Math.max(1, t2 - t0safe)) * (W - 2 * PAD);
  const yPos = (v: number) => H - PAD - ((v - lo) / span) * (H - 2 * PAD);
  const zeroY = yPos(0);
  const fmtT = (t: number) =>
    new Date(t).toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
  const ageSecs = Number.isFinite(t0) ? Math.max(0, (t1 - t0) / 1000) : null;

  return (
    <div
      className="fixed inset-0 z-50 bg-slate-900/50 backdrop-blur-sm flex items-center justify-center p-3 sm:p-6 select-none"
      role="dialog"
      aria-modal="true"
      aria-label={`Position details — ${pos.symbol}`}
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="bg-surface border border-surface-border rounded-2xl w-[960px] max-w-[96vw] max-h-[90vh] flex flex-col shadow-2xl overflow-hidden">
        <div className="flex items-center justify-between px-6 py-3.5 border-b border-surface-border bg-surface shrink-0">
          <div className="flex items-center gap-2 min-w-0 flex-wrap">
            <span className="px-2 py-0.5 rounded-md text-xs font-sans font-semibold bg-accent-dim text-accent-strong border border-accent-line">
              Position
            </span>
            <span className="text-sm font-bold font-sans text-slate-900">{pos.symbol}</span>
            <span className="text-xs font-sans text-slate-500">{formatStrategyName(pos.strategyId)}</span>
            <span className="px-2 py-0.5 rounded-md text-[11px] font-mono font-bold border bg-accent-dim text-accent-strong border-accent-line/60">
              Open
            </span>
            {pos.reduceOnly && (
              <span
                className="px-2 py-0.5 rounded-md text-[11px] font-mono font-bold border bg-warn-dim text-warn-strong border-warn-border"
                title="Reduce-Only: no new step increases; position closes on target profit"
              >
                Reduce-Only
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
          <DetailSection title="Position">
            <DetailField label="Trade Id" value={<span className="text-[11px]">{pos.tradeId}</span>} title={pos.tradeId} />
            <DetailField label="Strategy" value={formatStrategyName(pos.strategyId)} title={pos.strategyId} />
            <DetailField label="Opened" value={Number.isFinite(t0) ? fmtT(t0) : pos.openedAt} />
            <DetailField
              label="Age"
              value={ageSecs != null ? formatDuration(ageSecs) : "—"}
              tip="Time since the first fill in this trade family."
            />
            <DetailField
              label="Signals"
              value={String(pos.signalCount)}
              tip="Signal executions still counted as open in this position."
            />
            <DetailField
              label="Instruments"
              value={String(pos.legSymbols.length)}
              tip="Distinct leg instruments in this trade."
            />
            <DetailField
              label="Notional"
              value={pos.notionalUsd != null ? `$${pos.notionalUsd.toLocaleString(undefined, { maximumFractionDigits: 0 })}` : "—"}
              tip="USD notional at entry."
            />
          </DetailSection>

          <DetailSection title="PnL & Costs">
            <DetailField
              label="Unrealized PnL"
              value={<span className={pnlTone(nowPnl)}>{usd(nowPnl)}</span>}
              tip="Marked-to-market net PnL including fees paid; falls back to −fees when no mark is available."
            />
            <DetailField
              label="Fees Paid"
              value={`-$${Math.abs(fees).toFixed(2)}`}
              tip={`Cumulative fees on fills so far${family ? ` — ledger total across the family: $${familyFees.toFixed(2)}` : ""}.`}
            />
            <DetailField
              label="Expected Net"
              value={expectedNet != null ? usd(expectedNet) : "—"}
              tip="Entry-time expected net edge in USD (summed across entry signals when the family ledger is loaded)."
            />
            <DetailField
              label="Vs Expected"
              value={
                expectedNet != null && pos.unrealizedPnlUsd != null ? (
                  <span className={pnlTone(pos.unrealizedPnlUsd - expectedNet)}>
                    {usd(pos.unrealizedPnlUsd - expectedNet)}
                  </span>
                ) : (
                  "—"
                )
              }
              tip="Unrealized PnL minus the entry-time expectation — negative while edge hasn't matured."
            />
          </DetailSection>

          <DetailSection
            title={`Exposure (${pos.venues.length})`}
            gridClassName="block"
            tip="Net filled quantity per venue instrument — long legs positive, shorts negative."
          >
            <div className="overflow-x-auto -mx-3.5">
              <table className="w-full text-left text-xs font-mono tabular-nums">
                <thead className="bg-surface-raised/60">
                  <tr className="border-b border-surface-border text-slate-500 text-xs font-semibold tracking-wider">
                    <th className="py-2 px-3">Venue</th>
                    <th className="py-2 px-3">Instrument</th>
                    <th className="py-2 px-3">Side</th>
                    <th className="py-2 px-3 text-right">Net Qty</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-surface-border">
                  {pos.venues.map((v, i) => (
                    <tr key={`${v.venue}-${v.symbol}-${i}`} className="hover:bg-surface-raised/40 transition">
                      <td className="py-2 px-3 whitespace-nowrap">
                        <span className="inline-flex items-center gap-1.5">
                          <VenueLogo venueId={v.venue.toLowerCase()} name={v.venue} size={14} />
                          {v.venue}
                        </span>
                      </td>
                      <td className="py-2 px-3 text-slate-700">{v.symbol ?? "?"}</td>
                      <td className="py-2 px-3">
                        <span className={`font-bold ${v.netQty >= 0 ? "text-profit" : "text-loss"}`}>
                          {v.netQty >= 0 ? "LONG" : "SHORT"}
                        </span>
                      </td>
                      <td className={`py-2 px-3 text-right font-semibold ${v.netQty >= 0 ? "text-profit" : "text-loss"}`}>
                        {fmtQty4(v.netQty)}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </DetailSection>

          <div>
            <div className="text-[11px] font-sans text-muted font-medium mb-1.5">
              Net PnL since entry — solid: realized · dashed: illustrative 24h projection
            </div>
            <svg viewBox={`0 0 ${W} ${H}`} className="w-full rounded-lg border border-surface-border bg-surface-raised" role="img">
              {hasProjection && (
                <rect x={xPos(t1)} y={PAD * 0.5} width={W - PAD - xPos(t1)} height={H - PAD * 1.5} fill="#f8fafc" />
              )}
              <line x1={PAD} y1={zeroY} x2={W - PAD} y2={zeroY} stroke="#94a3b8" strokeWidth="1" strokeDasharray="2 3" />
              {hasProjection && (
                <line x1={xPos(t1)} y1={PAD * 0.5} x2={xPos(t1)} y2={H - PAD * 0.5} stroke="#94a3b8" strokeWidth="1" strokeDasharray="3 3" opacity="0.7" />
              )}
              <polygon
                points={`${xPos(t0safe)},${zeroY} ${xPos(t0safe)},${yPos(entryPnl)} ${xPos(t1)},${yPos(nowPnl)} ${xPos(t1)},${zeroY}`}
                fill={nowPnl >= 0 ? "#16a34a" : "#dc2626"}
                opacity="0.08"
              />
              <polyline
                points={`${xPos(t0safe)},${yPos(entryPnl)} ${xPos(t1)},${yPos(nowPnl)}`}
                fill="none"
                stroke={nowPnl >= 0 ? "#15803d" : "#b91c1c"}
                strokeWidth="2.5"
                strokeLinecap="round"
              />
              {hasProjection && (
                <polyline
                  points={`${xPos(t1)},${yPos(nowPnl)} ${xPos(t2)},${yPos(yEnd)}`}
                  fill="none"
                  stroke="#64748b"
                  strokeWidth="2"
                  strokeDasharray="6 4"
                  strokeLinecap="round"
                />
              )}
              <circle cx={xPos(t0safe)} cy={yPos(entryPnl)} r="3.5" fill="#64748b" />
              <circle cx={xPos(t1)} cy={yPos(nowPnl)} r="3.5" fill={nowPnl >= 0 ? "#15803d" : "#b91c1c"} />
              {hasProjection && <circle cx={xPos(t2)} cy={yPos(yEnd)} r="3" fill="#64748b" opacity="0.6" />}
              <text
                x={xPos(t1) - 8}
                y={yPos(nowPnl) + (nowPnl >= entryPnl ? -8 : 16)}
                fontSize="10"
                fontWeight="600"
                fill={nowPnl >= 0 ? "#15803d" : "#b91c1c"}
                fontFamily="Inter, sans-serif"
                textAnchor="end"
              >
                {usd(nowPnl)}
              </text>
              <text x={PAD} y={H - 8} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif">{fmtT(t0safe)}</text>
              <text x={W - PAD} y={H - 8} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif" textAnchor="end">now{hasProjection ? " → +24h" : ""}</text>
              <text x={PAD} y={14} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif">
                {`entry -$${Math.abs(entryPnl).toFixed(0)} fees`}
              </text>
            </svg>
            {!hasProjection && (
              <div className="text-[11px] font-sans text-muted mt-1.5">
                No expected-net figure from the backend, so no projection is drawn — nothing fabricated.
              </div>
            )}
          </div>

          {familyFailed && (
            <div className="rounded-lg border border-warn-border bg-warn-dim/40 px-3 py-2 text-xs font-mono text-warn-strong">
              Signal ledger unavailable — lifecycle and transaction tables could not be loaded.
            </div>
          )}

          {family == null && !familyFailed && (
            <EmptyState plain title="Loading signal lifecycle…" />
          )}

          {members.length > 0 && (
            <DetailSection
              title={`Signal Lifecycle (${members.length})`}
              gridClassName="block"
              tip="Every signal in this trade family — entry siblings plus flatten/trim children — newest lifecycle state first."
            >
              <div className="overflow-x-auto -mx-3.5">
                <table className="w-full text-left text-xs font-mono tabular-nums">
                  <thead className="bg-surface-raised/60">
                    <tr className="border-b border-surface-border text-slate-500 text-xs font-semibold tracking-wider">
                      <th className="py-2 px-3">Signal</th>
                      <th className="py-2 px-3">Role</th>
                      <th className="py-2 px-3">Status</th>
                      <th className="py-2 px-3">Dispatched</th>
                      <th className="py-2 px-3">Closed</th>
                      <th className="py-2 px-3 text-right">Fees</th>
                      <th className="py-2 px-3 text-right">Expected Net</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-surface-border">
                    {members.map((m) => {
                      const role = roleChip(signalRole(m.signalId));
                      const chip = statusChip(m.status);
                      return (
                        <tr key={m.signalId} className="hover:bg-surface-raised/40 transition">
                          <td className="py-2 px-3 text-slate-600 max-w-56 truncate" title={m.signalId}>
                            {m.signalId}
                          </td>
                          <td className="py-2 px-3">
                            <span className={`px-1.5 py-0.5 rounded-md text-[10px] font-bold border ${role.cls}`}>
                              {role.label}
                            </span>
                          </td>
                          <td className="py-2 px-3">
                            <span className={`px-1.5 py-0.5 rounded-md text-[10px] font-bold border ${chip.cls}`}>
                              {chip.label}
                            </span>
                          </td>
                          <td className="py-2 px-3 text-slate-600 whitespace-nowrap">{fmtTime(m.createdAt)}</td>
                          <td className="py-2 px-3 text-slate-600 whitespace-nowrap">
                            {m.closedAt ? fmtTime(m.closedAt) : "—"}
                          </td>
                          <td className="py-2 px-3 text-right text-slate-700">
                            {m.totalFeesUsd > 0 ? `-$${m.totalFeesUsd.toFixed(2)}` : "$0"}
                          </td>
                          <td className="py-2 px-3 text-right text-slate-700">
                            {m.expectedNetUsd != null ? usd(m.expectedNetUsd) : "—"}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            </DetailSection>
          )}

          {members.length > 0 && (
            <DetailSection
              title={`Atomic Transactions (${transactions.length})`}
              gridClassName="block"
              tip="Every dispatched venue order across the whole trade family — entry legs plus all flatten/trim children, deduplicated and chronological."
            >
              <AtomicTransactionsTable transactions={transactions} />
            </DetailSection>
          )}

          {family != null && members.length === 0 && (
            <div className="text-[11px] font-sans text-muted">
              No persisted signal rows match this position's trade family — the venue exposure above is the live truth.
            </div>
          )}
        </div>
      </div>
    </div>
  );
});
