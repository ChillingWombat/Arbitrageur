import React, { useEffect } from "react";
import { X } from "lucide-react";
import type { OpenPositionDto } from "../types";
import { VenueLogo } from "./VenueLogo";
import { DetailField, DetailSection } from "./DetailBits";

export interface PositionDetailModalProps {
  pos: OpenPositionDto;
  onClose: () => void;
}

/** Current-position detail: facts + net-PnL trend. Solid segment = realized
 *  (entry at -fees → now at unrealized mark, never fabricated); dashed
 *  segment = illustrative 24h run-rate projection, shown ONLY when the
 *  backend supplies expectedNetUsd. Marginal-PnL / funding-diff / spread
 *  trend graphs arrive with position telemetry (deferred). */
export const PositionDetailModal: React.FC<PositionDetailModalProps> = React.memo(({ pos, onClose }) => {
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);
  const fees = pos.feesUsd ?? 0;
  const nowPnl = pos.unrealizedPnlUsd ?? -fees;
  const entryPnl = -fees;
  const hasProjection = pos.expectedNetUsd != null && Number.isFinite(pos.expectedNetUsd);
  // Chart geometry: x in [t0, t1, t2], y auto-scaled with padding.
  const W = 600, H = 180, PAD = 28;
  const t0 = Date.parse(pos.openedAt);
  const t1 = Date.now();
  const t2 = t1 + 24 * 3600 * 1000;
  const t0safe = Number.isFinite(t0) ? t0 : t1 - 3600 * 1000;
  const yEnd = hasProjection ? nowPnl + (pos.expectedNetUsd as number) : nowPnl;
  const lo = Math.min(entryPnl, nowPnl, yEnd);
  const hi = Math.max(entryPnl, nowPnl, yEnd);
  const span = hi - lo > 1e-9 ? hi - lo : 1;
  const xPos = (t: number) => PAD + ((t - t0safe) / Math.max(1, t2 - t0safe)) * (W - 2 * PAD);
  const yPos = (v: number) => H - PAD - ((v - lo) / span) * (H - 2 * PAD);
  const zeroY = yPos(0);
  const fmtT = (t: number) =>
    new Date(t).toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
  return (
    <div
      className="fixed inset-0 z-50 bg-slate-900/50 backdrop-blur-sm flex items-center justify-center p-3 sm:p-6 select-none"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="bg-surface border border-surface-border rounded-2xl w-[880px] max-w-[95vw] max-h-[90vh] flex flex-col shadow-2xl overflow-hidden">
        <div className="flex items-center justify-between px-6 py-3.5 border-b border-surface-border bg-surface shrink-0">
          <div className="flex items-center gap-2 min-w-0">
            <span className="px-2 py-0.5 rounded-md text-xs font-sans font-semibold bg-accent-dim text-accent border border-accent-line">
              Position
            </span>
            <span className="text-sm font-bold font-sans text-slate-900">{pos.symbol}</span>
            <span className="text-xs font-sans text-slate-500">{pos.strategyId}</span>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="p-1.5 rounded-lg text-slate-400 hover:text-slate-700 hover:bg-slate-100 transition cursor-pointer"
            title="Close details (ESC)"
          >
            <X className="w-4 h-4" />
          </button>
        </div>
        <div className="flex-1 overflow-y-auto p-5 space-y-5">
          <DetailSection title="Position">
            <DetailField label="Strategy" value={pos.strategyId} />
            <DetailField label="Opened" value={Number.isFinite(t0) ? fmtT(t0) : pos.openedAt} />
            <DetailField
              label="Signals"
              value={String(pos.signalCount)}
              tip="Signal executions still counted as open in this position."
            />
            <DetailField
              label="Legs"
              value={
                pos.venues.length ? (
                  <span className="inline-flex flex-wrap items-center gap-x-2 gap-y-0.5">
                    {pos.venues.map((v, i) => (
                      <span key={`${v.venue}-${i}`} className="inline-flex items-center gap-1">
                        <VenueLogo venueId={v.venue} name={v.venue} size={13} />
                        {`${v.venue}:${v.symbol ?? "?"} (${v.netQty >= 0 ? "+" : ""}${v.netQty})`}
                      </span>
                    ))}
                  </span>
                ) : (
                  "—"
                )
              }
              tip="Net filled quantity per venue/symbol in this position."
            />
            <DetailField
              label="Notional"
              value={pos.notionalUsd != null ? `$${pos.notionalUsd.toLocaleString(undefined, { maximumFractionDigits: 0 })}` : "—"}
              tip="USD notional at entry."
            />
          </DetailSection>
          <DetailSection title="PnL & Costs">
            <DetailField
              label="Fees paid"
              value={`-$${Math.abs(fees).toFixed(2)}`}
              tip="Cumulative taker fees charged on fills so far."
            />
            <DetailField
              label="Unrealized PnL"
              value={`${nowPnl >= 0 ? "+" : "-"}$${Math.abs(nowPnl).toFixed(2)}`}
              tip="Marked-to-market net PnL including fees paid; falls back to −fees when no mark is available."
            />
          </DetailSection>
          <div>
            <div className="text-[11px] font-sans text-slate-400 font-medium mb-1.5">
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
                {`${nowPnl >= 0 ? "+" : "-"}$${Math.abs(nowPnl).toFixed(2)}`}
              </text>
              <text x={PAD} y={H - 8} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif">{fmtT(t0safe)}</text>
              <text x={W - PAD} y={H - 8} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif" textAnchor="end">now{hasProjection ? " → +24h" : ""}</text>
              <text x={PAD} y={14} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif">
                {`entry -$${Math.abs(entryPnl).toFixed(0)} fees`}
              </text>
            </svg>
            {!hasProjection && (
              <div className="text-[11px] font-sans text-slate-400 mt-1.5">
                No expected-net figure from the backend, so no projection is drawn — nothing fabricated.
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
});
