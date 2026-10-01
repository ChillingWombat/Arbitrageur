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
  parseInstrumentType,
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

/** Expected time or condition when the trade's positions will be closed to take profit. */
export const formatExpectedClose = (pos: OpenPositionDto): string => {
  const strat = (pos.strategyId ?? "").toLowerCase();
  if (strat.startsWith("options_") || strat === "options") {
    const expiries = new Set<string>();
    const allSymbols = [pos.symbol, ...(pos.legSymbols ?? [])];
    for (const s of allSymbols) {
      const match = s.match(/\b\d{1,2}[A-Z]{3}\d{2}\b/i);
      if (match) expiries.add(match[0].toUpperCase());
    }
    if (expiries.size === 1) {
      return `At Expiry (${[...expiries][0]})`;
    } else if (expiries.size > 1) {
      const sorted = [...expiries];
      return `Front: ${sorted[0]} · Back: ${sorted[1] ?? sorted[sorted.length - 1]}`;
    }
    return "At Contract Expiration";
  }
  if (strat.includes("funding")) {
    return "Next Funding Settlement (~8h)";
  }
  if (strat.includes("radar") || strat.includes("futures") || strat.includes("spread") || strat.includes("stat_arb")) {
    return "Mean Reversion Target (~1h - 4h)";
  }
  if (strat.includes("triangle")) {
    return "Instant / Sub-second";
  }
  return "Target Profit Reversion";
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
  // Forward projected target at settlement/expiry is the expected terminal net profit (not nowPnl + expectedNet)
  const targetPnl = expectedNet as number;
  const yEnd = hasProjection ? targetPnl : nowPnl;
  const capital = pos.notionalUsd && pos.notionalUsd > 0 ? pos.notionalUsd : null;
  const currentExpectedNet = expectedNet;
  const currentYieldPct =
    currentExpectedNet != null && capital != null && capital > 0
      ? (currentExpectedNet / capital) * 100
      : null;

  // Chart geometry: x in [t0, t1, t2], y auto-scaled with safe padding.
  const W = 800, H = 200;
  const PAD_LEFT = 75, PAD_RIGHT = 45, PAD_TOP = 26, PAD_BOTTOM = 30;
  const plotW = W - PAD_LEFT - PAD_RIGHT;
  const plotH = H - PAD_TOP - PAD_BOTTOM;
  const t0 = Date.parse(pos.openedAt);
  const t1 = Date.now();
  const t2 = t1 + 24 * 3600 * 1000;
  const t0safe = Number.isFinite(t0) ? t0 : t1 - 3600 * 1000;

  const rawLo = Math.min(entryPnl, nowPnl, yEnd);
  const rawHi = Math.max(entryPnl, nowPnl, yEnd);
  const padY = Math.max(1.5, (rawHi - rawLo) * 0.15);
  const lo = rawLo - padY;
  const hi = rawHi + padY;
  const span = hi - lo > 1e-9 ? hi - lo : 1;

  // Balanced 2-zone width partitioning:
  // If there's a forward projection, allocate 45% of plot width to realized history (t0 -> t1),
  // and 55% to the projected horizon (t1 -> t2).
  // If no projection, 100% of plot width is allocated to realized history.
  const realizedRatio = hasProjection ? 0.45 : 1.0;
  const realizedW = plotW * realizedRatio;
  const projW = plotW - realizedW;
  const elapsed = Math.max(1000, t1 - t0safe);
  const projDuration = 24 * 3600 * 1000;

  const xPos = (t: number) => {
    if (t <= t1) {
      const frac = Math.max(0, Math.min(1, (t - t0safe) / elapsed));
      return PAD_LEFT + frac * realizedW;
    } else {
      const frac = Math.max(0, Math.min(1, (t - t1) / projDuration));
      return PAD_LEFT + realizedW + frac * projW;
    }
  };
  const yPos = (v: number) => H - PAD_BOTTOM - ((v - lo) / span) * plotH;
  const zeroY = Math.max(PAD_TOP, Math.min(H - PAD_BOTTOM, yPos(0)));
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
              label="Expected Close"
              value={formatExpectedClose(pos)}
              tip="Estimated time or condition when positions in this trade will be closed to take profit."
            />
            <DetailField
              label="Legs"
              value={`${pos.signalCount} legs`}
              tip="Execution legs in this trade package (e.g. 8 option contracts across 2 expiry cycles)."
            />
          </DetailSection>

          <DetailSection title="Performance">
            <DetailField
              label="Unrealized PnL"
              value={<span className={pnlTone(nowPnl)}>{usd(nowPnl)}</span>}
              tip="Marked-to-market net PnL including fees paid; falls back to −fees when no mark is available."
            />
            <DetailField
              label="Initial Expected Net Profit"
              value={expectedNet != null ? usd(expectedNet) : "—"}
              tip="Entry-time locked or targeted net profit upon settlement/reversion."
            />
            <DetailField
              label="Current Expected Net Profit"
              value={
                currentExpectedNet != null ? (
                  <span className={pnlTone(currentExpectedNet)}>{usd(currentExpectedNet)}</span>
                ) : (
                  "—"
                )
              }
              tip="Current expected net profit taking into account updated information, settlement payout, and current market conditions."
            />
            <DetailField
              label="Entry Notional"
              value={pos.notionalUsd != null ? `$${pos.notionalUsd.toLocaleString(undefined, { maximumFractionDigits: 0 })}` : "—"}
              tip="Total USD capital / notional put into entering these positions."
            />
            <DetailField
              label="Current Expected Yield"
              value={
                currentYieldPct != null ? (
                  <span className={currentYieldPct >= 0 ? "text-profit font-semibold" : "text-loss font-semibold"}>
                    {currentYieldPct >= 0 ? "+" : ""}{currentYieldPct.toFixed(2)}%
                  </span>
                ) : (
                  "—"
                )
              }
              tip="Expected return on capital put into entering these positions (Current Expected Net Profit ÷ Entry Notional)."
            />
            <DetailField
              label="Fees Paid"
              value={`-$${Math.abs(fees).toFixed(2)}`}
              tip={`Cumulative fees on fills so far${family ? ` — ledger total across the family: $${familyFees.toFixed(2)}` : ""}.`}
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
                    <th className="py-2 px-3">Action</th>
                    <th className="py-2 px-3">Type</th>
                    <th className="py-2 px-3 text-right">Net Qty</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-surface-border">
                  {pos.venues.map((v, i) => {
                    const inst = parseInstrumentType(v.symbol ?? "");
                    const isLong = v.netQty >= 0;
                    const actionText = inst.isOption
                      ? (isLong ? "BOUGHT" : "WROTE")
                      : (isLong ? "LONG" : "SHORT");
                    const actionColor = inst.isOption
                      ? (isLong ? "text-profit font-bold" : "text-purple-600 dark:text-purple-400 font-bold")
                      : (isLong ? "text-profit font-bold" : "text-loss font-bold");
                    return (
                      <tr key={`${v.venue}-${v.symbol}-${i}`} className="hover:bg-surface-raised/40 transition">
                        <td className="py-2 px-3 whitespace-nowrap">
                          <span className="inline-flex items-center gap-1.5">
                            <VenueLogo venueId={v.venue.toLowerCase()} name={v.venue} size={14} />
                            {v.venue}
                          </span>
                        </td>
                        <td className="py-2 px-3 text-slate-700">{v.symbol ?? "?"}</td>
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
                          ) : inst.type === "SPOT" ? (
                            <span className="px-1.5 py-0.5 rounded text-[10px] font-semibold bg-blue-500/10 text-blue-600 dark:text-blue-400 border border-blue-500/20">
                              SPOT
                            </span>
                          ) : (
                            <span className="text-slate-400 text-[10px] font-mono">—</span>
                          )}
                        </td>
                        <td className={`py-2 px-3 text-right font-semibold ${v.netQty >= 0 ? "text-profit" : "text-loss"}`}>
                          {fmtQty4(v.netQty)}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          </DetailSection>

          <div>
            <div className="text-[11px] font-sans text-muted font-medium mb-1.5">
              Net PnL since entry — solid: realized · dashed: {pos.strategyId.toLowerCase().includes("options") ? "illustrative target at expiry" : "illustrative 24h projection"}
            </div>
            <svg viewBox={`0 0 ${W} ${H}`} className="w-full rounded-lg border border-surface-border bg-surface-raised" role="img">
              {hasProjection && (
                <rect x={xPos(t1)} y={PAD_TOP * 0.5} width={W - PAD_RIGHT - xPos(t1)} height={H - PAD_BOTTOM - PAD_TOP * 0.5} fill="#f8fafc" />
              )}
              {/* Zero line */}
              <line x1={PAD_LEFT} y1={zeroY} x2={W - PAD_RIGHT} y2={zeroY} stroke="#94a3b8" strokeWidth="1" strokeDasharray="2 3" />
              <text x={PAD_LEFT - 6} y={zeroY + 3.5} fontSize="9" fill="#94a3b8" fontFamily="Inter, sans-serif" textAnchor="end">
                $0
              </text>
              {/* Projection divider line at t1 */}
              {hasProjection && (
                <line x1={xPos(t1)} y1={PAD_TOP * 0.5} x2={xPos(t1)} y2={H - PAD_BOTTOM} stroke="#94a3b8" strokeWidth="1" strokeDasharray="3 3" opacity="0.7" />
              )}
              {/* Realized shaded area */}
              <polygon
                points={`${xPos(t0safe)},${zeroY} ${xPos(t0safe)},${yPos(entryPnl)} ${xPos(t1)},${yPos(nowPnl)} ${xPos(t1)},${zeroY}`}
                fill={nowPnl >= 0 ? "#16a34a" : "#dc2626"}
                opacity="0.08"
              />
              {/* Realized line */}
              <polyline
                points={`${xPos(t0safe)},${yPos(entryPnl)} ${xPos(t1)},${yPos(nowPnl)}`}
                fill="none"
                stroke={nowPnl >= 0 ? "#15803d" : "#b91c1c"}
                strokeWidth="2.5"
                strokeLinecap="round"
              />
              {/* Projected line */}
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
              {/* Dots */}
              <circle cx={xPos(t0safe)} cy={yPos(entryPnl)} r="3.5" fill="#64748b" />
              <circle cx={xPos(t1)} cy={yPos(nowPnl)} r="4" fill={nowPnl >= 0 ? "#15803d" : "#b91c1c"} />
              {hasProjection && <circle cx={xPos(t2)} cy={yPos(yEnd)} r="3.5" fill="#64748b" opacity="0.6" />}

              {/* Current PnL label at t1 */}
              {(() => {
                const px1 = xPos(t1);
                const nearLeft = px1 - PAD_LEFT < 36;
                const nearRight = W - PAD_RIGHT - px1 < 36;
                const anchor = nearLeft ? "start" : nearRight ? "end" : "middle";
                const labelX = nearLeft ? PAD_LEFT + 4 : nearRight ? W - PAD_RIGHT - 4 : px1;
                return (
                  <text
                    x={labelX}
                    y={yPos(nowPnl) + (nowPnl >= entryPnl ? -9 : 16)}
                    fontSize="11"
                    fontWeight="700"
                    fill={nowPnl >= 0 ? "#15803d" : "#b91c1c"}
                    fontFamily="Inter, sans-serif"
                    textAnchor={anchor}
                  >
                    {usd(nowPnl)}
                  </text>
                );
              })()}

              {/* Projected target label at t2 */}
              {hasProjection && (
                <text
                  x={xPos(t2)}
                  y={yPos(yEnd) + (yEnd >= nowPnl ? -9 : 16)}
                  fontSize="10"
                  fontWeight="600"
                  fill="#64748b"
                  fontFamily="Inter, sans-serif"
                  textAnchor="end"
                >
                  {`target: ${usd(yEnd)}`}
                </text>
              )}

              {/* Time axis labels */}
              <text x={PAD_LEFT} y={H - 8} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif">
                {`${fmtT(t0safe)} (Entry)`}
              </text>
              {hasProjection && (
                <text x={Math.max(PAD_LEFT + 55, Math.min(W - PAD_RIGHT - 55, xPos(t1)))} y={H - 8} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif" textAnchor="middle">
                  Now
                </text>
              )}
              <text x={W - PAD_RIGHT} y={H - 8} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif" textAnchor="end">
                {hasProjection ? (pos.strategyId.toLowerCase().includes("options") ? "Target (At Expiry)" : "Target (+24h)") : "Now"}
              </text>

              {/* Entry fees indicator */}
              <text x={PAD_LEFT} y={16} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif">
                {`entry -$${Math.abs(entryPnl).toFixed(2)} fees`}
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
