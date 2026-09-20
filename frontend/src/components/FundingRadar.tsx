import React, { useState, useEffect, useMemo, useRef } from "react";
import { SlidersHorizontal, X, ChevronDown, Check } from "lucide-react";
import type { FundingOpportunityDto, FundingTrendDto, ExecutionLogEntry, OpenPositionDto } from "../types";
import { api, describeCommandError } from "../services/api";
import type { PerpTicker } from "../types/tickers";
import { BotLifecycleControls } from "./BotLifecycleControls";
import { TickerTransactionHistory } from "./TickerTransactionHistory";
import { TickerLogMonitor } from "./TickerLogMonitor";
import { EmptyState } from "./EmptyState";
import { VenueLogo } from "./VenueLogo";
import { useBotLifecycle } from "../hooks/useBotLifecycle";

interface FundingRadarProps {
  opportunities: FundingOpportunityDto[];
  /** All shared non-hidden market tickers currently monitored */
  availableTickers?: PerpTicker[];
  /** Optional fallback / context symbol */
  symbol?: string | null;
  tradeSize: number;
  minAprPct?: number;
  onExecuteCarry?: (opp: FundingOpportunityDto) => Promise<void>;
  loading: boolean;
  executionLogs?: ExecutionLogEntry[];
  onOpenConfig?: (symbol: string, strategy?: string) => void;
  isPaused?: boolean;
  hasApiConnectivity?: boolean;
  /** Global open positions (App fan-out); the gadget filters strategy "funding". */
  openPositions?: OpenPositionDto[];
}

const CarryDetailField: React.FC<{ label: string; value: React.ReactNode; title?: string }> = ({
  label,
  value,
  title,
}) => (
  <div className="space-y-0.5" title={title}>
    <div className="text-[11px] font-sans text-slate-400 font-medium">{label}</div>
    <div className="text-sm font-bold font-sans text-slate-900 tabular-nums">{value}</div>
  </div>
);

/** Scanner card box: decay-aware expected net over the modelled hold
 * (backend value, flat fallback), replacing the unrealistic flat daily rate. */
const CarryExpectedBox: React.FC<{ opp: FundingOpportunityDto }> = ({ opp }) => {
  const v = expectedCarryUsd(opp);
  const holdH =
    opp.expected_hold_hours != null && Number.isFinite(opp.expected_hold_hours) && opp.expected_hold_hours > 0
      ? opp.expected_hold_hours
      : null;
  const title =
    holdH != null
      ? `Expected net over ~${holdH.toFixed(1)}h modelled hold with linear funding-gap decay to the min-APR exit, net of round-trip fees${opp.entry_spread_bps != null && opp.entry_spread_bps > 0 ? " + entry spread" : ""}`
      : "Expected net unavailable — hold unknown";
  return (
    <div
      className="px-1.5 py-1 bg-surface-raised rounded-lg border border-surface-border"
      title={title}
    >
      <div className="text-slate-500">Expected</div>
      <div
        className={`font-semibold tabular-nums ${v == null ? "text-slate-400" : v >= 0 ? "text-profit" : "text-loss"}`}
      >
        {v == null ? "—" : fmtSignedUsd(v)}
      </div>
    </div>
  );
};

interface CarryDetailModalProps {
  opp: FundingOpportunityDto;
  rank: number;
  executing: boolean;
  executed: boolean;
  onExecute: (opp: FundingOpportunityDto) => void;
  onClose: () => void;
  minAprPct?: number;
}

/** Funding trend for the carry detail popup: per-leg 8h-normalized prints
 *  (left axis, %/8h) plus ticker mid history (right axis, USD, venue-neutral).
 *  Dashed green = model-expected net decay from the current print to the
 *  min-APR exit threshold over the popup's expected hold. Empty series =
 *  "no history yet" (sampling just started) — current prints above still hold.
 *  Never interpolates or fabricates: only persisted samples are drawn. */
const FundingTrendGraph: React.FC<{ opp: FundingOpportunityDto; minAprPct: number }> = ({
  opp,
  minAprPct,
}) => {
  const [trend, setTrend] = useState<FundingTrendDto | null>(null);
  const [loading, setLoading] = useState(true);
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setTrend(null);
    api
      .getFundingTrend(opp.symbol, opp.long_venue, opp.short_venue, 72)
      .then((t) => {
        if (!cancelled) {
          setTrend(t);
          setLoading(false);
        }
      })
      .catch(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [opp.id, opp.symbol, opp.long_venue, opp.short_venue]);

  const W = 600;
  const H = 220;
  const PADL = 52;
  const PADR = 56;
  const PADT = 10;
  const PADB = 20;

  if (loading) {
    return (
      <div className="text-xs font-sans text-slate-400 py-6 text-center">
        Loading funding history…
      </div>
    );
  }
  const long = (trend?.long_series ?? []).filter((p) => Number.isFinite(p.v));
  const short = (trend?.short_series ?? []).filter((p) => Number.isFinite(p.v));
  const price = (trend?.price_series ?? []).filter((p) => Number.isFinite(p.v) && p.v > 0);
  if (long.length === 0 && short.length === 0) {
    return (
      <div className="text-xs font-sans text-slate-400 py-4 text-center">
        No funding history yet — sampling just started. Current leg prints are shown above.
      </div>
    );
  }
  const fundVals = [...long.map((p) => p.v), ...short.map((p) => p.v)];
  // APR = net_8h_frac * 1095 → threshold in %/8h for the min-APR exit line.
  const thrPct = Number.isFinite(minAprPct) && minAprPct > 0 ? minAprPct / 1095 : 0;
  const holdH =
    opp.expected_hold_hours != null &&
    Number.isFinite(opp.expected_hold_hours) &&
    opp.expected_hold_hours > 0
      ? Math.min(opp.expected_hold_hours, 168)
      : null;
  const curNet = opp.net_rate_8h_pct;
  const projVals = holdH != null && Number.isFinite(curNet) ? [curNet, thrPct] : [];
  const allFund = [...fundVals, thrPct, ...projVals];
  let fMin = Math.min(...allFund);
  let fMax = Math.max(...allFund);
  if (!(fMax > fMin)) {
    fMax = fMin + 1e-6;
  }
  const fPad = (fMax - fMin) * 0.15 || 1e-6;
  fMin -= fPad;
  fMax += fPad;

  const nowT = Date.now();
  const startT = Math.min(long[0]?.t ?? nowT, short[0]?.t ?? nowT);
  const endHist = Math.max(
    long.length ? long[long.length - 1].t : startT,
    short.length ? short[short.length - 1].t : startT,
  );
  const projEndT = holdH != null ? nowT + holdH * 3_600_000 : null;
  const endT = Math.max(endHist, nowT, projEndT ?? 0);
  const xSpan = Math.max(endT - startT, 1);
  const xPos = (t: number) => PADL + ((t - startT) / xSpan) * (W - PADL - PADR);
  const yFund = (v: number) => PADT + (1 - (v - fMin) / (fMax - fMin)) * (H - PADT - PADB);

  let pMin = Infinity;
  let pMax = -Infinity;
  for (const pt of price) {
    if (pt.v < pMin) pMin = pt.v;
    if (pt.v > pMax) pMax = pt.v;
  }
  const hasPrice = price.length > 1 && pMax > pMin;
  if (!hasPrice) {
    pMin = 0;
    pMax = 1;
  } else {
    const pp = (pMax - pMin) * 0.1 || 1;
    pMin -= pp;
    pMax += pp;
  }
  const yPrice = (v: number) => PADT + (1 - (v - pMin) / (pMax - pMin)) * (H - PADT - PADB);
  const line = (pts: { t: number; v: number }[], y: (v: number) => number) =>
    pts.map((pt) => `${xPos(pt.t).toFixed(1)},${y(pt.v).toFixed(1)}`).join(" ");

  // Net gap on shared timestamps (both legs minute-bucketed): thin dashed.
  const shortByT = new Map(short.map((pt) => [pt.t, pt.v]));
  const netPts = long.flatMap((pt) => {
    const s = shortByT.get(pt.t);
    return s == null ? [] : [{ t: pt.t, v: s - pt.v }];
  });
  const showProjection = holdH != null && projEndT != null && Number.isFinite(curNet);
  const fmtT = (t: number) =>
    new Date(t).toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
  const mid = trend?.current_mid_usd;

  return (
    <div>
      <svg
        viewBox={`0 0 ${W} ${H}`}
        className="w-full rounded-lg border border-surface-border bg-surface-raised"
        role="img"
        aria-label={`Funding trend for ${opp.symbol}`}
      >
        <line x1={PADL} y1={yFund(0)} x2={W - PADR} y2={yFund(0)} stroke="#cbd5e1" strokeWidth="1" strokeDasharray="2 3" />
        {thrPct > 0 && (
          <line
            x1={PADL}
            y1={yFund(thrPct)}
            x2={W - PADR}
            y2={yFund(thrPct)}
            stroke="#f43f5e"
            strokeWidth="1"
            strokeDasharray="3 3"
            opacity="0.7"
          />
        )}
        {hasPrice && (
          <polyline points={line(price, yPrice)} fill="none" stroke="#2563eb" strokeWidth="1.5" opacity="0.55" />
        )}
        {netPts.length > 1 && (
          <polyline points={line(netPts, yFund)} fill="none" stroke="#64748b" strokeWidth="1.5" strokeDasharray="5 3" />
        )}
        <polyline points={line(long, yFund)} fill="none" stroke="#0d9488" strokeWidth="2" />
        <polyline points={line(short, yFund)} fill="none" stroke="#ea580c" strokeWidth="2" />
        {showProjection && (
          <line
            x1={xPos(nowT)}
            y1={yFund(curNet)}
            x2={xPos(projEndT as number)}
            y2={yFund(thrPct)}
            stroke="#16a34a"
            strokeWidth="2"
            strokeDasharray="6 4"
          />
        )}
        <circle cx={xPos(nowT)} cy={yFund(curNet)} r="3" fill="#16a34a" />
        <text x={PADL} y={H - 6} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif">
          {fmtT(startT)}
        </text>
        <text x={W - PADR} y={H - 6} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif" textAnchor="end">
          now{holdH != null ? ` → +${holdH.toFixed(1)}h` : ""}
        </text>
        <text x={4} y={14} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif">
          {`${fMax.toFixed(4)}%`}
        </text>
        <text x={4} y={H - PADB} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif">
          {`${fMin.toFixed(4)}%`}
        </text>
        {hasPrice && (
          <text x={W - 4} y={14} fontSize="10" fill="#2563eb" fontFamily="Inter, sans-serif" textAnchor="end">
            {`$${pMax.toLocaleString(undefined, { maximumFractionDigits: 2 })}`}
          </text>
        )}
      </svg>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 mt-1.5 text-[11px] font-sans text-slate-500">
        <span className="inline-flex items-center gap-1">
          <span className="inline-block w-3 h-0.5 bg-teal-600" /> Long · {opp.long_venue}
        </span>
        <span className="inline-flex items-center gap-1">
          <span className="inline-block w-3 h-0.5 bg-orange-600" /> Short · {opp.short_venue}
        </span>
        {netPts.length > 1 && (
          <span className="inline-flex items-center gap-1">
            <span className="inline-block w-3 border-t border-dashed border-slate-500" /> Net gap
          </span>
        )}
        {hasPrice && <span className="inline-flex items-center gap-1 text-blue-700">— {opp.symbol} mid</span>}
        {showProjection && (
          <span className="inline-flex items-center gap-1 text-green-700">- - expected to min-APR exit</span>
        )}
        {mid != null && Number.isFinite(mid) && (
          <span className="tabular-nums">ref ${mid.toLocaleString(undefined, { maximumFractionDigits: 2 })}</span>
        )}
      </div>
    </div>
  );
};

/** Expected net carry in USD: backend decay-aware value when present,
 * else the flat fallback (current-rate accrual over the breakeven hold, net
 * of fees + spread — same math the modal used historically). Null when
 * neither is computable. */
function expectedCarryUsd(opp: FundingOpportunityDto): number | null {
  const e = opp.expected_return_usd;
  if (e != null && Number.isFinite(e)) return e;
  const holdH =
    opp.break_even_hours != null && Number.isFinite(opp.break_even_hours) && opp.break_even_hours > 0
      ? opp.break_even_hours
      : null;
  if (holdH == null || !(opp.trade_size_usd > 0)) return null;
  const gross = opp.trade_size_usd * (opp.net_rate_8h_pct / 100) * (holdH / 8);
  const fees = opp.trade_size_usd * (opp.round_trip_fee_bps / 10000);
  const spread =
    opp.entry_spread_bps != null && opp.entry_spread_bps > 0
      ? (opp.trade_size_usd * opp.entry_spread_bps) / 10000
      : 0;
  const net = gross - fees - spread;
  return Number.isFinite(net) ? net : null;
}

function fmtSignedUsd(v: number, digits = 0): string {
  return `${v >= 0 ? "+" : "-"}$${Math.abs(v).toFixed(digits)}`;
}

const CarryDetailModal: React.FC<CarryDetailModalProps> = React.memo(
  ({ opp, rank, executing, executed, onExecute, onClose, minAprPct = 8.0 }) => {
    // Expected hold (days): backend persistence model when present, else
    // fee breakeven fallback. Projected net subtracts fees AND entry spread
    // so early-exit holds honestly show a signed loss.
    const holdHours =
      opp.expected_hold_hours != null &&
      Number.isFinite(opp.expected_hold_hours) &&
      opp.expected_hold_hours > 0
        ? opp.expected_hold_hours
        : Number.isFinite(opp.break_even_hours) && opp.break_even_hours > 0
          ? opp.break_even_hours
          : null;
    const holdIsModelled =
      opp.expected_hold_hours != null && Number.isFinite(opp.expected_hold_hours);
    const autoH =
      holdHours != null ? Math.max(0.1, Math.round((holdHours / 24) * 10) / 10) : null;
    const size = opp.trade_size_usd;
    const fees = size * (opp.round_trip_fee_bps / 10000);
    const spreadCost =
      opp.entry_spread_bps != null && opp.entry_spread_bps > 0
        ? (size * opp.entry_spread_bps) / 10000
        : 0;
    const net = expectedCarryUsd(opp);
    useEffect(() => {
      const handleKeyDown = (e: KeyboardEvent) => {
        if (e.key === "Escape") onClose();
      };
      window.addEventListener("keydown", handleKeyDown);
      return () => window.removeEventListener("keydown", handleKeyDown);
    }, [onClose]);
    return (
      <div
        className="fixed inset-0 z-50 bg-slate-900/50 backdrop-blur-sm flex items-center justify-center p-3 sm:p-6 select-none"
        onClick={(e) => {
          if (e.target === e.currentTarget) onClose();
        }}
      >
        <div className="bg-surface border border-surface-border rounded-2xl w-[680px] max-w-[94vw] max-h-[90vh] flex flex-col shadow-2xl overflow-hidden">
          <div className="flex items-center justify-between px-6 py-3.5 border-b border-surface-border bg-surface shrink-0">
            <div className="flex items-center gap-2 min-w-0">
              <span className="px-2 py-0.5 rounded-md text-xs font-sans font-semibold bg-accent-dim text-accent border border-accent-line">
                Rank #{rank}
              </span>
              <span className="text-sm font-bold font-sans text-slate-900">{opp.symbol}</span>
              <span className="text-xs font-sans text-slate-500">
                {opp.long_venue} / {opp.short_venue}
              </span>
              <span className="text-xs font-sans font-bold text-accent tabular-nums">
                {opp.net_apy_pct.toFixed(2)}% APY
              </span>
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
            <div className="grid grid-cols-2 sm:grid-cols-3 gap-x-4 gap-y-3">
              <CarryDetailField label="Long leg" value={opp.long_venue} />
              <CarryDetailField label="Short leg" value={opp.short_venue} />
              <CarryDetailField
                label={`Long funding · ${opp.long_venue}`}
                value={
                  opp.long_rate_8h_pct != null && Number.isFinite(opp.long_rate_8h_pct)
                    ? `${opp.long_rate_8h_pct >= 0 ? "+" : ""}${opp.long_rate_8h_pct.toFixed(4)}% / 8h`
                    : "—"
                }
                title="Current funding print of the long leg, 8h-normalized. Spot legs pay none (0)."
              />
              <CarryDetailField
                label={`Short funding · ${opp.short_venue}`}
                value={
                  opp.short_rate_8h_pct != null && Number.isFinite(opp.short_rate_8h_pct)
                    ? `${opp.short_rate_8h_pct >= 0 ? "+" : ""}${opp.short_rate_8h_pct.toFixed(4)}% / 8h`
                    : "—"
                }
                title="Current funding print of the short leg, 8h-normalized."
              />
              <CarryDetailField
                label="Net 8h rate"
                value={`+${opp.net_rate_8h_pct.toFixed(4)}%`}
              />
              <CarryDetailField label="Annualized APR" value={`${opp.net_apr_pct.toFixed(2)}%`} />
              <CarryDetailField label="Compounded APY" value={`${opp.net_apy_pct.toFixed(2)}%`} />
              <CarryDetailField
                label="Win probability"
                value={opp.win_probability != null ? `${(opp.win_probability * 100).toFixed(0)}%` : "—"}
                title="Modelled-hold fee-coverage heuristic over the expected hold with gap decay (informational only — never gates entries; agrees in sign with Expected)"
              />
              <CarryDetailField
                label="Depth slope"
                value={opp.depth_slope_bps_per_1k != null ? `${opp.depth_slope_bps_per_1k.toFixed(2)} bps/$1k` : "—"}
                title="Marginal entry-cost gradient summed over both legs at trade size — how fast slippage grows per extra $1k (informational only)"
              />
              <CarryDetailField
                label="Type"
                value={opp.kind === "basis" ? "Basis (SPOT/PERP)" : "Carry (perp/perp)"}
                title="Basis = same-exchange long-spot / short-perp cash-and-carry; Carry = cross-exchange perp/perp funding"
              />
              {opp.kind === "basis" && (
                <CarryDetailField
                  label="Basis divergence"
                  value={opp.basis_bps != null ? `${opp.basis_bps >= 0 ? "+" : ""}${opp.basis_bps.toFixed(1)} bps` : "—"}
                  title="Signed live divergence (perp - spot) / spot. Positive = contango."
                />
              )}
              {opp.kind === "basis" && opp.spot_price != null && (
                <CarryDetailField
                  label="Spot ref"
                  value={`$${opp.spot_price.toLocaleString(undefined, { maximumFractionDigits: 2 })}`}
                />
              )}
              {opp.kind === "basis" && opp.perp_price != null && (
                <CarryDetailField
                  label="Perp ref"
                  value={`$${opp.perp_price.toLocaleString(undefined, { maximumFractionDigits: 2 })}`}
                />
              )}
              <CarryDetailField
                label="Opportunity capacity"
                value={
                  opp.optimal_size_usd != null && opp.optimal_size_usd > 0
                    ? `$${opp.optimal_size_usd.toLocaleString(undefined, { maximumFractionDigits: 0 })}`
                    : "—"
                }
                title="Depth-supported size from both legs' live orderbooks"
              />
              <CarryDetailField
                label="Entry spread"
                value={
                  opp.entry_spread_bps != null && opp.entry_spread_bps >= 0
                    ? `${opp.entry_spread_bps.toFixed(1)} bps`
                    : "—"
                }
                title="Measured long-ask vs short-bid cross at executable size"
              />
              <CarryDetailField
                label="Fee breakeven (min)"
                value={`${opp.break_even_hours.toFixed(1)} hrs`}
                title="Minimum hold to cover round-trip fees at the current 8h rate (excl. entry spread). Compare with Auto hold (expected) for the modelled exit."
              />
              <CarryDetailField label="Daily income" value={`+$${opp.estimated_daily_income_usd.toFixed(2)}`} />
              <CarryDetailField
                label="Round-trip fees"
                value={`-$${fees.toFixed(2)} (${opp.round_trip_fee_bps.toFixed(1)} bps)${spreadCost > 0 ? ` + spread $${spreadCost.toFixed(2)}` : ""}`}
                title="Entry friction: taker fees plus measured entry-spread cross (both deducted in Projected net carry)"
              />
              <CarryDetailField
                label={holdIsModelled ? "Auto hold (expected)" : "Auto hold"}
                value={
                  autoH != null
                    ? `${autoH} days${opp.hold_confidence != null ? ` · ${(opp.hold_confidence * 100).toFixed(0)}%` : ""}`
                    : "—"
                }
                title={
                  opp.hold_reason ??
                  "Fallback: fee break-even only (stale backend without persistence model) — not set manually"
                }
              />
              <CarryDetailField
                label="Projected net carry"
                value={
                  net != null ? `${net >= 0 ? "+" : "-"}$${Math.abs(net).toFixed(2)}` : "—"
                }
                title="Projected carry over the expected hold at the current 8h rate, net of round-trip fees and entry spread. Negative = expected loss if the gap decays before breakeven."
              />
            </div>

          <div>
            <div className="text-[11px] font-sans text-slate-400 font-medium mb-1.5">
              Funding trend — solid: venue prints (%/8h, left) · blue: {opp.symbol} mid (right) · dashed green: model-expected net to min-APR exit
            </div>
            <FundingTrendGraph opp={opp} minAprPct={minAprPct} />
          </div>
          </div>

          <div className="px-6 py-3.5 border-t border-surface-border bg-surface flex items-center justify-end gap-2 shrink-0">
            {executed ? (
              <span className="inline-flex items-center px-2 py-0.5 rounded-md bg-profit-dim text-profit text-xs font-semibold">
                Carry Active
              </span>
            ) : (
              <button
                type="button"
                onClick={() => onExecute(opp)}
                disabled={executing}
                className="btn-primary !text-xs !py-2.5 !px-2.5 font-sans cursor-pointer"
              >
                <span>{executing ? "Opening..." : "Open Carry"}</span>
              </button>
            )}
          </div>
        </div>
      </div>
    );
  },
);

interface PositionDetailModalProps {
  pos: OpenPositionDto;
  onClose: () => void;
}

/** Current-position detail: facts + net-PnL trend. Solid segment = realized
 *  (entry at -fees → now at unrealized mark, never fabricated); dashed
 *  segment = illustrative 24h run-rate projection, shown ONLY when the
 *  backend supplies expectedNetUsd. Marginal-PnL / funding-diff / spread
 *  trend graphs arrive with position telemetry (deferred). */
const PositionDetailModal: React.FC<PositionDetailModalProps> = React.memo(({ pos, onClose }) => {
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
      <div className="bg-surface border border-surface-border rounded-2xl w-[680px] max-w-[94vw] max-h-[90vh] flex flex-col shadow-2xl overflow-hidden">
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
          <div className="grid grid-cols-2 sm:grid-cols-3 gap-x-4 gap-y-3">
            <CarryDetailField label="Strategy" value={pos.strategyId} />
            <CarryDetailField
              label="Legs"
              value={pos.venues.map((v) => `${v.venue} (${v.netQty >= 0 ? "+" : ""}${v.netQty})`).join(" / ") || "—"}
            />
            <CarryDetailField
              label="Notional"
              value={pos.notionalUsd != null ? `$${pos.notionalUsd.toLocaleString(undefined, { maximumFractionDigits: 0 })}` : "—"}
            />
            <CarryDetailField label="Fees paid" value={`-$${Math.abs(fees).toFixed(2)}`} />
            <CarryDetailField
              label="Unrealized PnL"
              value={`${nowPnl >= 0 ? "+" : "-"}$${Math.abs(nowPnl).toFixed(2)}`}
            />
            <CarryDetailField
              label="Signals"
              value={String(pos.signalCount)}
              title="Signal executions still counted as open in this position"
            />
            <CarryDetailField label="Opened" value={Number.isFinite(t0) ? fmtT(t0) : pos.openedAt} />
          </div>
          <div>
            <div className="text-[11px] font-sans text-slate-400 font-medium mb-1.5">
              Net PnL since entry — solid: realized · dashed: illustrative 24h projection
            </div>
            <svg viewBox={`0 0 ${W} ${H}`} className="w-full rounded-lg border border-surface-border bg-surface-raised" role="img">
              <line x1={PAD} y1={zeroY} x2={W - PAD} y2={zeroY} stroke="#cbd5e1" strokeWidth="1" strokeDasharray="2 3" />
              <polyline
                points={`${xPos(t0safe)},${yPos(entryPnl)} ${xPos(t1)},${yPos(nowPnl)}`}
                fill="none"
                stroke={nowPnl >= 0 ? "#15803d" : "#b91c1c"}
                strokeWidth="2.5"
              />
              {hasProjection && (
                <polyline
                  points={`${xPos(t1)},${yPos(nowPnl)} ${xPos(t2)},${yPos(yEnd)}`}
                  fill="none"
                  stroke="#64748b"
                  strokeWidth="2"
                  strokeDasharray="6 4"
                />
              )}
              <circle cx={xPos(t0safe)} cy={yPos(entryPnl)} r="3.5" fill="#64748b" />
              <circle cx={xPos(t1)} cy={yPos(nowPnl)} r="3.5" fill={nowPnl >= 0 ? "#15803d" : "#b91c1c"} />
              <text x={PAD} y={H - 8} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif">{fmtT(t0safe)}</text>
              <text x={W - PAD} y={H - 8} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif" textAnchor="end">now{hasProjection ? " → +24h" : ""}</text>
              <text x={PAD} y={14} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif">
                {`entry -$${Math.abs(entryPnl).toFixed(0)} fees · now ${nowPnl >= 0 ? "+" : "-"}$${Math.abs(nowPnl).toFixed(0)}`}
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

export const FundingRadar: React.FC<FundingRadarProps> = ({
  opportunities,
  availableTickers: _availableTickers,
  symbol,
  tradeSize,
  minAprPct = 8.0,
  onExecuteCarry,
  executionLogs = [],
  onOpenConfig,
  isPaused = false,
  hasApiConnectivity = false,
  openPositions = [],
}) => {
  const [executingId, setExecutingId] = useState<string | null>(null);
  const [lastExecuted, setLastExecuted] = useState<string | null>(null);
  const [execError, setExecError] = useState<string | null>(null);
  const execErrorTimer = useRef<number | null>(null);
  const [detailOppId, setDetailOppId] = useState<string | null>(null);
  const [dockTab, setDockTab] = useState<"position" | "history" | "logs">("history");
  const [detailPosKey, setDetailPosKey] = useState<string | null>(null);
  // Display-only scanner filters (mirror the spread monitor strip): venue
  // multi-select, min net 8h edge (BPS), min win probability (%). None of
  // these reach the engine — execution and the auto loop use the full list.
  const [venueFilters, setVenueFilters] = useState<string[]>([]);
  // Default mirrors the auto-carry bot's entry hurdle (Min APR %) converted
  // to 8h BPS: APR/100/365/3*10000 — the same 365d×3 convention the backend
  // uses both ways. Tracks the live minAprPct until the user overrides it.
  // Min Win has no bot-side gate (win probability never gates execution),
  // so it defaults to off.
  // Filter inputs show at most 2 decimal places (bot-mirrored defaults are
  // normalized; user typing is never reformatted mid-edit).
  const to2dp = (n: number): string =>
    Number.isFinite(n) ? String(parseFloat(n.toFixed(2))) : "";
  const aprToBps8h = (aprPct: number) =>
    Number.isFinite(aprPct) && aprPct > 0 ? to2dp((aprPct * 10000) / 100 / 365 / 3) : "";
  const [minExpBps, setMinExpBps] = useState(() => aprToBps8h(minAprPct));
  const minExpTouched = useRef(false);
  useEffect(() => {
    if (!minExpTouched.current) setMinExpBps(aprToBps8h(minAprPct));
  }, [minAprPct]);
  const [minWinPct, setMinWinPct] = useState("");
  const [showVenueMenu, setShowVenueMenu] = useState(false);
  const venueMenuRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (venueMenuRef.current && !venueMenuRef.current.contains(e.target as Node)) {
        setShowVenueMenu(false);
      }
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, []);
  const filterVenues = useMemo(
    () => Array.from(new Set(opportunities.flatMap((o) => [o.long_venue, o.short_venue]))).sort(),
    [opportunities],
  );
  const filteredOpps = useMemo(() => {
    let out = opportunities;
    if (venueFilters.length)
      out = out.filter((o) => venueFilters.includes(o.long_venue) || venueFilters.includes(o.short_venue));
    const me = Number(minExpBps);
    if (minExpBps !== "" && Number.isFinite(me))
      out = out.filter((o) => (o.net_rate_8h_pct ?? -Infinity) * 100 >= me);
    const mp = Number(minWinPct);
    if (minWinPct !== "" && Number.isFinite(mp))
      out = out.filter((o) => ((o.win_probability ?? -Infinity) * 100) >= mp);
    return out;
  }, [opportunities, venueFilters, minExpBps, minWinPct]);
  const clearFilters = () => {
    setVenueFilters([]);
    minExpTouched.current = false;
    setMinExpBps(aprToBps8h(minAprPct));
    setMinWinPct("");
  };
  const fundingPositions = useMemo(
    () => openPositions.filter((p) => (p.strategyId ?? "").toLowerCase() === "funding"),
    [openPositions]
  );
  const fundingLogs = useMemo(
    () =>
      executionLogs.filter(
        (l) =>
          l.strategy_id === "funding" ||
          (l.trade_type ?? "").toLowerCase().includes("funding") ||
          (l.trade_type ?? "").toLowerCase().includes("carry")
      ),
    [executionLogs]
  );

  // Bot lifecycle is strategy-wide for Funding Rate carry, not locked to a single ticker
  const bot = useBotLifecycle({ strategyId: "funding" });
  const lastAutoCarryExecRef = useRef<number>(0);



  const showExecError = (message: string) => {
    setExecError(message);
    if (execErrorTimer.current != null) window.clearTimeout(execErrorTimer.current);
    execErrorTimer.current = window.setTimeout(() => setExecError(null), 8000);
  };

  // Silent auto-loop failures stay in the console: surfacing them would
  // re-banner every 8s while native execution is disabled backend-wide.
  // Manual clicks always surface the backend reason inline (never throw to
  // the error boundary — an expected fail-closed rejection is not a crash).
  const handleExecute = async (opp: FundingOpportunityDto, opts?: { silent?: boolean }) => {
    if (!onExecuteCarry) return;
    if (bot.phase === "paused" || bot.phase === "reduce_only" || bot.phase === "stopping") return;
    setExecutingId(opp.id);
    try {
      await onExecuteCarry(opp);
      setLastExecuted(opp.id);
      setTimeout(() => setLastExecuted(null), 3000);
    } catch (err: unknown) {
      const message = describeCommandError(err, "Execution failed");
      console.error(`Funding carry execution failed for ${opp.id}:`, message);
      if (!opts?.silent) showExecError(message);
    } finally {
      setExecutingId(null);
    }
  };

  // Automated Execution Loop across all monitored tickers when user enables Auto Carry
  useEffect(() => {
    if (!bot.entriesAllowed || executingId || !opportunities.length || isPaused) return;

    const now = Date.now();
    if (now - lastAutoCarryExecRef.current < 8000) return; // 8s cooldown

    // Select top funding carry opportunity with positive net yield meeting hurdle
    const topOpp = opportunities.find((o) => o.net_apr_pct >= minAprPct);
    if (topOpp) {
      lastAutoCarryExecRef.current = now;
      void handleExecute(topOpp, { silent: true });
    }
  }, [bot.entriesAllowed, executingId, opportunities, isPaused, minAprPct]);

  return (
    <div className="p-4 pr-5 space-y-4 overflow-y-auto h-full">
      {/* Strategy Ribbon — read-only params left, configurator + toggle right */}
      <div className="ribbon-container">
        <div className="flex flex-wrap items-center gap-2 text-xs font-mono">
          <span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1">
            Strategy Parameters
          </span>
          <div
            className="ribbon-pill"
            title="Notional per leg — edit via Configurator"
          >
            <span className="text-slate-400">Trade Size</span>
            <span className="font-bold text-slate-900 tabular-nums">{tradeSize.toLocaleString()}</span>
            <span className="text-slate-400">USD</span>
          </div>

          <div
            className="ribbon-pill"
            title="Ignore opportunities below this net APR — edit via Configurator"
          >
            <span className="text-slate-400">Min APR</span>
            <span className="font-bold text-slate-900 tabular-nums">{minAprPct}</span>
            <span className="text-slate-400">%</span>
          </div>

        </div>

        <div className="flex items-center space-x-3 shrink-0">
          {/* Configure Strategy Button — universal params, applies to all funding carry */}
          <button
            type="button"
            onClick={() => onOpenConfig?.(symbol || "BTC-USDC", "funding")}
            className="btn-secondary !text-xs !py-2 !px-2 font-sans cursor-pointer gap-2"
            title="Configure Funding Rate strategy parameters (applies to all tickers)"
          >
            <SlidersHorizontal className="w-3.5 h-3.5 text-accent" />
            <span>Configurator</span>
          </button>

          {/* Auto Carry Bot Switch */}
          <BotLifecycleControls
            phase={bot.phase}
            hasOpenPositions={bot.hasOpenPositions}
            disabled={isPaused || !hasApiConnectivity}
            onStart={bot.start}
            onPause={bot.pause}
            onReduceOnly={bot.reduceOnly}
            onStop={() => void bot.stop()}
          />
          {bot.lastError && (
            <span className="text-xs font-mono text-loss max-w-56 truncate" title={bot.lastError}>
              {bot.lastError}
            </span>
          )}
          {execError && (
            <span className="text-xs font-mono text-loss max-w-96 truncate" title={execError}>
              Execution blocked: {execError}
            </span>
          )}
        </div>
      </div>

      {/* Opportunity Scanner — fixed window, 5x3 compact cards, click for details */}
      <div className="bg-surface rounded-xl border border-surface-border p-4 shadow-card h-[640px] flex flex-col">
        <div className="flex items-center justify-between gap-3 mb-3 pb-3 border-b border-surface-border shrink-0">
          <h3 className="text-[13px] font-bold font-sans tracking-wider text-slate-900">
            Opportunity Scanner
          </h3>
          <div className="flex items-center gap-2 text-xs font-sans">
            <div className="relative" ref={venueMenuRef}>
              <button
                type="button"
                onClick={() => setShowVenueMenu((v) => !v)}
                className="flex items-center gap-1.5 bg-surface border border-surface-border rounded-lg px-2.5 py-1.5 shadow-2xs hover:border-slate-300 transition-all cursor-pointer"
                title="Display filter — shows only cards with a selected venue on either leg. Does not affect execution"
              >
                <span className="text-slate-400 font-medium">Venue</span>
                {venueFilters.length > 0 && (
                  <span className="flex items-center -space-x-1">
                    {venueFilters.slice(0, 4).map((v) => (
                      <VenueLogo key={v} venueId={v} name={v} size={16} />
                    ))}
                  </span>
                )}
                <span className="font-bold text-slate-900 font-mono text-xs">
                  {venueFilters.length === 0 ? "ALL" : venueFilters.join("+").toUpperCase()}
                </span>
                <ChevronDown
                  className={`w-3.5 h-3.5 text-slate-400 transition-transform ${showVenueMenu ? "rotate-180" : ""}`}
                />
              </button>
              {showVenueMenu && (
                <div className="absolute right-0 top-full mt-1.5 w-52 bg-surface border border-surface-border rounded-xl shadow-elevated p-1.5 z-30 font-sans text-xs animate-in fade-in zoom-in-95 duration-100">
                  <button
                    type="button"
                    onClick={() => setVenueFilters([])}
                    className={`w-full flex items-center justify-between px-2 py-2 text-left transition rounded-lg cursor-pointer ${
                      venueFilters.length === 0
                        ? "bg-accent/10 text-accent font-semibold"
                        : "text-slate-700 hover:bg-surface-raised hover:text-slate-900"
                    }`}
                  >
                    <span>All</span>
                    {venueFilters.length === 0 && <Check className="w-3.5 h-3.5" />}
                  </button>
                  <div className="border-t border-surface-border/60 my-1 -mx-1" />
                  {filterVenues.map((v) => {
                    const on = venueFilters.includes(v);
                    return (
                      <button
                        key={v}
                        type="button"
                        onClick={() =>
                          setVenueFilters((prev) =>
                            prev.includes(v) ? prev.filter((x) => x !== v) : [...prev, v],
                          )
                        }
                        className={`w-full flex items-center justify-between px-2 py-2 text-left transition rounded-lg cursor-pointer ${
                          on
                            ? "bg-accent/10 text-accent font-semibold"
                            : "text-slate-700 hover:bg-surface-raised hover:text-slate-900"
                        }`}
                      >
                        <span className="flex items-center gap-2">
                          <VenueLogo venueId={v} name={v} size={18} />
                          <span className="font-mono text-xs uppercase">{v}</span>
                        </span>
                        {on && <Check className="w-3.5 h-3.5" />}
                      </button>
                    );
                  })}
                </div>
              )}
            </div>
            <div
              className="flex items-center gap-1.5 bg-surface border border-surface-border rounded-lg px-2.5 py-1.5 shadow-2xs hover:border-slate-300 focus-within:border-primary focus-within:ring-2 focus-within:ring-primary/15 transition-all"
              title="Display filter — minimum net 8h funding edge. Defaults to the bot's Min APR hurdle (converted to BPS). Does not affect execution"
            >
              <span className="text-slate-400 font-medium">Min Exp</span>
              <input
                value={minExpBps}
                onChange={(e) => {
                  minExpTouched.current = true;
                  setMinExpBps(e.target.value);
                }}
                placeholder="0"
                className="bg-transparent w-14 outline-none text-right font-bold text-slate-900 font-mono text-xs"
              />
              <span className="text-slate-400 text-[11px] font-medium">BPS</span>
            </div>
            <div
              className="flex items-center gap-1.5 bg-surface border border-surface-border rounded-lg px-2.5 py-1.5 shadow-2xs hover:border-slate-300 focus-within:border-primary focus-within:ring-2 focus-within:ring-primary/15 transition-all"
              title="Display filter — minimum win probability. The bot does not gate on win probability, so this defaults to off. Does not affect execution"
            >
              <span className="text-slate-400 font-medium">Min Win</span>
              <input
                value={minWinPct}
                onChange={(e) => setMinWinPct(e.target.value)}
                placeholder="0"
                className="bg-transparent w-14 outline-none text-right font-bold text-slate-900 font-mono text-xs"
              />
              <span className="text-slate-400 text-[11px] font-medium">%</span>
            </div>
          </div>
        </div>

        {filteredOpps.length === 0 ? (
          <div className="border border-dashed border-surface-border rounded-xl">
            {opportunities.length === 0 ? (
              <EmptyState plain title="No Opportunity Detected" />
            ) : (
              <div className="flex flex-col items-center gap-2 py-8 text-xs font-sans text-slate-500">
                <span>
                  No opportunities match the current filters ({opportunities.length} scanned).
                </span>
                <button
                  type="button"
                  onClick={clearFilters}
                  className="text-accent font-semibold underline underline-offset-2 cursor-pointer"
                >
                  Clear filters
                </button>
              </div>
            )}
          </div>
        ) : (
          <div className="flex-1 min-h-0 overflow-y-auto">
            <div className="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-5 gap-3">
              {filteredOpps.map((opp, idx) => (
                <div
                  key={opp.id}
                  onClick={() => setDetailOppId(opp.id)}
                  className="bg-surface border border-surface-border hover:border-accent-line rounded-xl p-2.5 flex flex-col justify-between gap-2 transition cursor-pointer shadow-card"
                  title="Click for opportunity details"
                >
                  <div className="space-y-2">
                    <div className="flex items-center justify-between gap-1.5">
                      <div className="flex items-center gap-1 min-w-0">
                        <span className="px-1.5 py-0.5 rounded-md text-[10px] font-sans font-semibold bg-accent-dim text-accent border border-accent-line shrink-0">
                          #{idx + 1}
                        </span>
                        {opp.kind === "basis" && (
                          <span className="px-1.5 py-0.5 rounded-md text-[10px] font-sans font-bold bg-profit-dim text-profit border border-profit/30 shrink-0" title="Same-exchange long-spot / short-perp cash-and-carry">
                            BASIS
                          </span>
                        )}
                        <span className="px-1.5 py-0.5 rounded-md text-[10px] font-sans font-bold bg-slate-100 text-slate-800 border border-slate-200 truncate">
                          {opp.symbol}
                        </span>
                      </div>
                      <span className="text-[10px] font-sans font-bold text-accent tabular-nums shrink-0">
                        {opp.net_apy_pct.toFixed(1)}%
                      </span>
                    </div>

                    <div className="px-2 py-1.5 bg-surface-raised rounded-lg border border-surface-border font-sans text-[10px] space-y-1">
                      <div className="flex items-center justify-between gap-1">
                        <span className="text-slate-500">Long:</span>
                        <span className="font-semibold text-slate-900 truncate">{opp.long_venue}</span>
                      </div>
                      <div className="flex items-center justify-between gap-1">
                        <span className="text-slate-500">Short:</span>
                        <span className="font-semibold text-slate-900 truncate">{opp.short_venue}</span>
                      </div>
                      <div className="pt-1 border-t border-surface-border flex items-center justify-between text-profit">
                        <span>8h:</span>
                        <span className="font-bold tabular-nums">+{opp.net_rate_8h_pct.toFixed(3)}%</span>
                      </div>
                      {opp.kind === "basis" && opp.basis_bps != null && (
                        <div className="flex items-center justify-between gap-1">
                          <span className="text-slate-500">Basis:</span>
                          <span className="font-semibold text-slate-900 tabular-nums">{opp.basis_bps >= 0 ? "+" : ""}{opp.basis_bps.toFixed(1)} bps</span>
                        </div>
                      )}
                      <div className="flex items-center justify-between gap-1">
                        <span className="text-slate-500">Win:</span>
                        <span className="font-semibold text-slate-900 tabular-nums" title="Modelled-hold fee-coverage heuristic — informational only; sub-50% whenever Expected is negative">{opp.win_probability != null ? `${(opp.win_probability * 100).toFixed(0)}%` : "—"}</span>
                      </div>
                    </div>

                    <div className="grid grid-cols-2 gap-1.5 font-sans text-[10px]">
                      <div className="px-1.5 py-1 bg-surface-raised rounded-lg border border-surface-border">
                        <div className="text-slate-500">Breakeven</div>
                        <div className="font-semibold text-slate-900 tabular-nums">{opp.break_even_hours.toFixed(1)}h</div>
                      </div>
                      <CarryExpectedBox opp={opp} />
                    </div>
                  </div>

                  <div className="pt-2 border-t border-surface-border flex items-center justify-between font-sans text-[10px]">
                    <span className="font-semibold text-slate-800 tabular-nums">{opp.net_apr_pct.toFixed(1)}% APR</span>
                    {lastExecuted === opp.id ? (
                      <span className="inline-flex items-center px-1.5 py-0.5 rounded-md bg-profit-dim text-profit text-[10px] font-semibold">
                        Active
                      </span>
                    ) : (
                      <button
                        onClick={(e) => {
                          e.stopPropagation();
                          handleExecute(opp);
                        }}
                        disabled={executingId === opp.id}
                        className="btn-primary !text-[10px] !py-2 !px-2 font-sans cursor-pointer"
                      >
                        <span>{executingId === opp.id ? "Opening..." : "Open Carry"}</span>
                      </button>
                    )}
                  </div>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>
      {/* Bottom dock: Current Position vs Execution History vs Log Monitor */}
      <div className="bg-surface rounded-xl border border-surface-border shadow-card overflow-hidden shrink-0">
        <div className="flex items-stretch border-b border-surface-border bg-slate-50/80 select-none">
          <div className="grid grid-cols-3 flex-1 divide-x divide-surface-border">
            <button
              type="button"
              onClick={() => setDockTab("position")}
              className={`relative h-9 flex items-center justify-center gap-2 px-4 text-[11px] font-sans transition select-none cursor-pointer ${
                dockTab === "position"
                  ? "text-accent font-bold bg-surface"
                  : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
              }`}
            >
              <span>Current Position</span>
              {fundingPositions.length > 0 && (
              <span className="px-1.5 py-0.5 rounded-md text-[11px] font-sans font-bold min-w-[16px] text-center leading-tight bg-accent-dim text-accent border border-accent-line/60">
                {fundingPositions.length}
              </span>
              )}
              {dockTab === "position" && (
                <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
              )}
            </button>
            <button
              type="button"
              onClick={() => setDockTab("history")}
              className={`relative h-9 flex items-center justify-center gap-2 px-4 text-[11px] font-sans transition select-none cursor-pointer ${
                dockTab === "history"
                  ? "text-accent font-bold bg-surface"
                  : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
              }`}
            >
              <span>Execution History</span>
              {fundingLogs.length > 0 && (
              <span className="px-1.5 py-0.5 rounded-md text-[11px] font-sans font-bold min-w-[16px] text-center leading-tight bg-accent-dim text-accent border border-accent-line/60">
                {fundingLogs.length}
              </span>
              )}
              {dockTab === "history" && (
                <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
              )}
            </button>
            <button
              type="button"
              onClick={() => setDockTab("logs")}
              className={`relative h-9 flex items-center justify-center gap-2 px-4 text-[11px] font-sans transition select-none cursor-pointer ${
                dockTab === "logs"
                  ? "text-accent font-bold bg-surface"
                  : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
              }`}
            >
              <span>Log Monitor</span>
              {dockTab === "logs" && (
                <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
              )}
            </button>
          </div>
        </div>
        <div className="p-3.5 max-h-96 overflow-y-auto">
          {dockTab === "position" ? (
            fundingPositions.length === 0 ? (
              <EmptyState
                plain
                title="No Open Funding Positions"
                hint="Carry/basis fills will aggregate here per symbol with live PnL. Native execution is currently disabled, so this stays empty until authenticated order submission lands."
              />
            ) : (
              <div className="divide-y divide-surface-border">
                {fundingPositions.map((pos) => {
                  const pnl = pos.unrealizedPnlUsd;
                  return (
                    <button
                      key={pos.key}
                      type="button"
                      onClick={() => setDetailPosKey(pos.key)}
                      className="w-full flex items-center justify-between gap-3 px-2 py-2.5 rounded-lg hover:bg-slate-50 transition cursor-pointer text-left"
                      title="Click for position details and PnL trend"
                    >
                      <div className="min-w-0">
                        <div className="text-xs font-sans font-bold text-slate-900 truncate">{pos.symbol}</div>
                        <div className="text-[11px] font-sans text-slate-500 truncate">
                          {pos.venues.map((v) => v.venue).join(" / ") || pos.strategyId}
                        </div>
                      </div>
                      <div className="text-right shrink-0">
                        <div className={`text-xs font-sans font-bold tabular-nums ${pnl != null && pnl < 0 ? "text-loss" : "text-profit"}`}>
                          {pnl != null ? `${pnl >= 0 ? "+" : "-"}$${Math.abs(pnl).toFixed(2)}` : "—"}
                        </div>
                        <div className="text-[11px] font-sans text-slate-500 tabular-nums">
                          {pos.notionalUsd != null ? `$${pos.notionalUsd.toLocaleString(undefined, { maximumFractionDigits: 0 })}` : "—"}
                        </div>
                      </div>
                    </button>
                  );
                })}
              </div>
            )
          ) : dockTab === "history" ? (
            <TickerTransactionHistory symbol="" logs={fundingLogs} matchAllSymbols />
          ) : (
            <TickerLogMonitor
              symbol=""
              namespace="funding-carry"
              strategyName="Funding Carry"
              executionLogs={fundingLogs}
              matchAllSymbols
              isPaused={isPaused}
            />
          )}
        </div>
      </div>
      {(() => {
        const detailPos =
          detailPosKey != null ? fundingPositions.find((x) => x.key === detailPosKey) ?? null : null;
        if (!detailPos) return null;
        return <PositionDetailModal pos={detailPos} onClose={() => setDetailPosKey(null)} />;
      })()}
      {(() => {
        const detailOpp =
          detailOppId != null ? opportunities.find((o) => o.id === detailOppId) ?? null : null;
        if (!detailOpp) return null;
        return (
          <CarryDetailModal
            opp={detailOpp}
            rank={opportunities.findIndex((o) => o.id === detailOpp.id) + 1}
            executing={executingId === detailOpp.id}
            executed={lastExecuted === detailOpp.id}
            onExecute={handleExecute}
            onClose={() => setDetailOppId(null)}
            minAprPct={minAprPct}
          />
        );
      })()}

    </div>
  );
};
