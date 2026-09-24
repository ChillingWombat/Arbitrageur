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
import { ExecuteButton } from "./ExecuteButton";
import { CapacityChart } from "./CapacityChart";
import { DetailField, DetailSection } from "./DetailBits";
import { PositionDetailModal } from "./PositionDetailModal";
import { InfoTooltip } from "./InfoTooltip";
import { useBotLifecycle } from "../hooks/useBotLifecycle";

interface FundingRadarProps {
  opportunities: FundingOpportunityDto[];
  /** All shared non-hidden market tickers currently monitored */
  availableTickers?: PerpTicker[];
  /** Optional fallback / context symbol */
  symbol?: string | null;
  tradeSize: number;
  minAprPct?: number;
  onExecuteCarry?: (opp: FundingOpportunityDto, auto?: boolean) => Promise<void>;
  /** Live environment — autonomous entries are testnet-only, manual stays gated. */
  isLive?: boolean;
  loading: boolean;
  executionLogs?: ExecutionLogEntry[];
  onOpenConfig?: (symbol: string, strategy?: string) => void;
  isPaused?: boolean;
  hasApiConnectivity?: boolean;
  /** Global open positions (App fan-out); the gadget filters strategy "funding". */
  openPositions?: OpenPositionDto[];
}

/** Scanner card box: decay-aware expected net over the modelled hold
 * (backend value, flat fallback), replacing the unrealistic flat daily rate. */
const CarryExpectedInline: React.FC<{ opp: FundingOpportunityDto }> = ({ opp }) => {
  // Real (backtested) expected net; "—" when history is too thin to measure.
  // The model estimate lives in the popup details, not here.
  const v =
    opp.backtest_expected_usd != null && Number.isFinite(opp.backtest_expected_usd)
      ? opp.backtest_expected_usd
      : null;
  const holdH =
    opp.expected_hold_hours != null && Number.isFinite(opp.expected_hold_hours) && opp.expected_hold_hours > 0
      ? opp.expected_hold_hours
      : null;
  const title =
    v != null && opp.backtest_trials != null
      ? `Mean net over ${opp.backtest_trials} trailing-history backtest entries${holdH != null ? ` (modelled hold ~${holdH.toFixed(1)}h)` : ""} — measured, not modelled`
      : "Expected net unmeasurable — fewer than 20 scoreable history entries";
  return (
    <span className="inline-flex items-baseline gap-1" title={title}>
      <span className="text-slate-500">Expected Profit:</span>
      <span
        className={`font-semibold tabular-nums ${v == null ? "text-muted" : v >= 0 ? "text-profit" : "text-loss"}`}
      >
        {v == null ? "—" : fmtSignedUsd(v)}
      </span>
    </span>
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
/** Evenly-rounded axis ticks: picks a 1/2/5×10ⁿ step so ~`count` ticks land
 * inside [lo, hi], then returns every multiple of the step in range. */
function niceTicks(lo: number, hi: number, count = 4): number[] {
  if (!(hi > lo) || !Number.isFinite(lo) || !Number.isFinite(hi)) return [];
  const step0 = (hi - lo) / count;
  const mag = 10 ** Math.floor(Math.log10(step0));
  const norm = step0 / mag;
  const step = (norm >= 5 ? 5 : norm >= 2 ? 2 : 1) * mag;
  const ticks: number[] = [];
  for (let v = Math.ceil(lo / step) * step; v <= hi + step * 1e-9; v += step) {
    ticks.push(Math.abs(v) < step * 1e-9 ? 0 : v);
  }
  return ticks;
}

const FundingTrendGraph: React.FC<{ opp: FundingOpportunityDto; minAprPct: number }> = ({
  opp,
  minAprPct,
}) => {
  const [trend, setTrend] = useState<FundingTrendDto | null>(null);
  const [loading, setLoading] = useState(true);
  const [mode, setMode] = useState<"gap" | "legs">("gap");
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
  const PADL = 56;
  const PADR = 60;
  const PADT = 14;
  const PADB = 20;

  if (loading) {
    return (
      <div className="text-xs font-sans text-muted py-6 text-center">
        Loading funding history…
      </div>
    );
  }
  const long = (trend?.long_series ?? []).filter((p) => Number.isFinite(p.v));
  const short = (trend?.short_series ?? []).filter((p) => Number.isFinite(p.v));
  const price = (trend?.price_series ?? []).filter((p) => Number.isFinite(p.v) && p.v > 0);
  if (long.length === 0 && short.length === 0) {
    return (
      <div className="text-xs font-sans text-muted py-4 text-center">
        No funding history yet — sampling just started. Current leg prints are shown above.
      </div>
    );
  }
  // Funding prints are standing rates: a print applies until the venue posts
  // the next one, so missing samples are carried forward (LOCF), never
  // zero-filled. `stepAt` reads the standing rate at any instant.
  const stepAt = (pts: { t: number; v: number }[], t: number): number | undefined => {
    let lo = 0;
    let hi = pts.length;
    while (lo < hi) {
      const m = (lo + hi) >> 1;
      if (pts[m].t <= t) lo = m + 1;
      else hi = m;
    }
    return lo > 0 ? pts[lo - 1].v : undefined;
  };

  const nowT = Date.now();
  const startT = Math.min(long[0]?.t ?? nowT, short[0]?.t ?? nowT);
  const endHist = Math.max(
    long.length ? long[long.length - 1].t : startT,
    short.length ? short[short.length - 1].t : startT,
    nowT,
  );

  // Net gap on a uniform time grid — both legs carried forward, so the gap
  // is defined wherever each leg has at least one earlier print.
  const GRID_N = 160;
  const netPts: { t: number; v: number }[] = [];
  if (endHist > startT && long.length > 0 && short.length > 0) {
    for (let i = 0; i <= GRID_N; i++) {
      const t = startT + (i / GRID_N) * (endHist - startT);
      const l = stepAt(long, t);
      const s = stepAt(short, t);
      if (l != null && s != null) netPts.push({ t, v: s - l });
    }
  }
  const viewMode = mode === "gap" && netPts.length === 0 ? "legs" : mode;

  // APR = net_8h_frac * 1095 → threshold in %/8h for the min-APR exit line.
  const thrPct = Number.isFinite(minAprPct) && minAprPct > 0 ? minAprPct / 1095 : 0;
  const holdH =
    opp.expected_hold_hours != null &&
    Number.isFinite(opp.expected_hold_hours) &&
    opp.expected_hold_hours > 0
      ? Math.min(opp.expected_hold_hours, 168)
      : null;
  const curNet = opp.net_rate_8h_pct;
  const projVals = viewMode === "gap" && holdH != null && Number.isFinite(curNet) ? [curNet, thrPct] : [];
  const fundVals =
    viewMode === "gap"
      ? netPts.map((p) => p.v)
      : [...long.map((p) => p.v), ...short.map((p) => p.v)];
  const allFund = [...fundVals, ...(viewMode === "gap" ? [thrPct] : []), ...projVals];
  let fMin = allFund.length ? Math.min(...allFund) : 0;
  let fMax = allFund.length ? Math.max(...allFund) : 1e-4;
  if (!(fMax > fMin)) {
    fMax = fMin + 1e-6;
  }
  const fPad = (fMax - fMin) * 0.15 || 1e-6;
  fMin -= fPad;
  fMax += fPad;

  const projEndT = viewMode === "gap" && holdH != null ? nowT + holdH * 3_600_000 : null;
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
  // Step line (post): the standing rate holds flat until the next print.
  const stepLine = (pts: { t: number; v: number }[], y: (v: number) => number) =>
    pts
      .flatMap((pt, i) =>
        i === 0
          ? [`${xPos(pt.t).toFixed(1)},${y(pt.v).toFixed(1)}`]
          : [
              `${xPos(pt.t).toFixed(1)},${y(pts[i - 1].v).toFixed(1)}`,
              `${xPos(pt.t).toFixed(1)},${y(pt.v).toFixed(1)}`,
            ],
      )
      .join(" ");

  const showProjection = viewMode === "gap" && holdH != null && projEndT != null && Number.isFinite(curNet);
  const fmtT = (t: number) =>
    new Date(t).toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
  const mid = trend?.current_mid_usd;

  const fundTicks = niceTicks(fMin, fMax, 4);
  const tickStep = fundTicks.length > 1 ? Math.abs(fundTicks[1] - fundTicks[0]) : Math.abs(fMax - fMin) || 1;
  const tickDec = Math.min(4, Math.max(0, -Math.floor(Math.log10(tickStep))));
  const fmtPct = (v: number) => `${v.toFixed(tickDec)}%`;
  const fmtUsdAxis = (v: number) =>
    `$${v >= 100 ? v.toFixed(0) : v >= 1 ? v.toFixed(2) : v.toPrecision(3)}`;
  const priceTicks = hasPrice ? niceTicks(pMin, pMax, 3) : [];
  const nowX = xPos(nowT);
  const lastLong = long.length ? long[long.length - 1] : null;
  const lastShort = short.length ? short[short.length - 1] : null;
  const lastNet = netPts.length ? netPts[netPts.length - 1] : null;
  const fmtEndPct = (v: number) => `${v >= 0 ? "+" : ""}${v.toFixed(3)}%`;

  return (
    <div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 mb-1.5 text-[11px] font-sans text-slate-500">
        {viewMode === "gap" ? (
          <span className="inline-flex items-center gap-1 text-slate-600">
            <span className="inline-block w-3 h-0.5 bg-slate-500" /> Net gap
            {lastNet != null && (
              <span className="font-semibold tabular-nums text-slate-700">{fmtEndPct(lastNet.v)}</span>
            )}
          </span>
        ) : (
          <>
            <span className="inline-flex items-center gap-1">
              <span className="inline-block w-3 h-0.5 bg-teal-600" /> Long ·
              <VenueLogo venueId={opp.long_venue} name={opp.long_venue} size={12} /> {opp.long_venue}
              {lastLong != null && (
                <span className="font-semibold tabular-nums text-teal-700">{fmtEndPct(lastLong.v)}</span>
              )}
            </span>
            <span className="inline-flex items-center gap-1">
              <span className="inline-block w-3 h-0.5 bg-orange-600" /> Short ·
              <VenueLogo venueId={opp.short_venue} name={opp.short_venue} size={12} /> {opp.short_venue}
              {lastShort != null && (
                <span className="font-semibold tabular-nums text-orange-700">{fmtEndPct(lastShort.v)}</span>
              )}
            </span>
          </>
        )}
        {hasPrice && (
          <span className="inline-flex items-center gap-1 text-blue-700">
            — {opp.symbol} mid
            {mid != null && Number.isFinite(mid) && (
              <span className="font-semibold tabular-nums">
                ${mid.toLocaleString(undefined, { maximumFractionDigits: 2 })}
              </span>
            )}
          </span>
        )}
        {viewMode === "gap" && thrPct > 0 && (
          <span className="inline-flex items-center gap-1 text-rose-600">
            - - min APR <span className="font-semibold tabular-nums">{(thrPct * 1095).toFixed(1)}%</span>
          </span>
        )}
        {showProjection && (
          <span className="inline-flex items-center gap-1 text-green-700">
            - - model exit → +{holdH != null ? holdH.toFixed(1) : "?"}h
          </span>
        )}
        <span className="ml-auto inline-flex rounded-md border border-surface-border overflow-hidden">
          {(
            [
              ["gap", "Net gap"],
              ["legs", "Per-leg"],
            ] as const
          ).map(([m, label]) => (
            <button
              key={m}
              type="button"
              onClick={() => setMode(m)}
              className={`px-2 py-0.5 text-[10px] font-semibold ${
                viewMode === m ? "bg-slate-700 text-white" : "bg-surface-raised text-slate-500 hover:text-slate-700"
              }`}
            >
              {label}
            </button>
          ))}
        </span>
      </div>
      <svg
        viewBox={`0 0 ${W} ${H}`}
        className="w-full rounded-lg border border-surface-border bg-surface-raised"
        role="img"
        aria-label={`Funding trend for ${opp.symbol}`}
      >
        {showProjection && (
          <rect x={nowX} y={PADT} width={Math.max(0, W - PADR - nowX)} height={H - PADT - PADB} fill="#f0fdf4" />
        )}
        {fundTicks.map((v) => (
          <g key={v}>
            <line
              x1={PADL}
              y1={yFund(v)}
              x2={W - PADR}
              y2={yFund(v)}
              stroke={v === 0 ? "#94a3b8" : "#e2e8f0"}
              strokeWidth="1"
              strokeDasharray={v === 0 ? "2 3" : undefined}
            />
            <text x={PADL - 6} y={yFund(v) + 3} fontSize="9.5" fill="#94a3b8" fontFamily="Inter, sans-serif" textAnchor="end">
              {fmtPct(v)}
            </text>
          </g>
        ))}
        {priceTicks.map((v) => (
          <text key={v} x={W - PADR + 6} y={yPrice(v) + 3} fontSize="9.5" fill="#93c5fd" fontFamily="Inter, sans-serif">
            {fmtUsdAxis(v)}
          </text>
        ))}
        {viewMode === "gap" && thrPct > 0 && (
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
          <polyline
            points={line(price, yPrice)}
            fill="none"
            stroke="#2563eb"
            strokeWidth="1.25"
            strokeLinejoin="round"
            strokeLinecap="round"
            opacity="0.45"
          />
        )}
        {viewMode === "gap" ? (
          <>
            {netPts.length > 1 && (
              <polyline
                points={line(netPts, yFund)}
                fill="none"
                stroke="#475569"
                strokeWidth="2"
                strokeLinejoin="round"
              />
            )}
            {lastNet != null && <circle cx={xPos(lastNet.t)} cy={yFund(lastNet.v)} r="2.5" fill="#475569" />}
          </>
        ) : (
          <>
            {long.length > 0 && (
              <polyline points={stepLine(long, yFund)} fill="none" stroke="#0d9488" strokeWidth="2" strokeLinejoin="round" strokeLinecap="round" />
            )}
            {short.length > 0 && (
              <polyline points={stepLine(short, yFund)} fill="none" stroke="#ea580c" strokeWidth="2" strokeLinejoin="round" strokeLinecap="round" />
            )}
            {lastLong != null && <circle cx={xPos(lastLong.t)} cy={yFund(lastLong.v)} r="2.5" fill="#0d9488" />}
            {lastShort != null && <circle cx={xPos(lastShort.t)} cy={yFund(lastShort.v)} r="2.5" fill="#ea580c" />}
          </>
        )}
        {showProjection && (
          <g>
            <line x1={nowX} y1={PADT} x2={nowX} y2={H - PADB} stroke="#94a3b8" strokeWidth="1" strokeDasharray="3 3" opacity="0.8" />
            <line
              x1={nowX}
              y1={yFund(curNet)}
              x2={xPos(projEndT as number)}
              y2={yFund(thrPct)}
              stroke="#16a34a"
              strokeWidth="2"
              strokeDasharray="6 4"
              strokeLinecap="round"
            />
            <circle cx={nowX} cy={yFund(curNet)} r="3" fill="#16a34a" />
          </g>
        )}
        {[0.25, 0.5, 0.75].map((f) => {
          const t = startT + f * xSpan;
          const x = xPos(t);
          return (
            <g key={f}>
              <line x1={x} y1={PADT} x2={x} y2={H - PADB} stroke="#e2e8f0" strokeWidth="1" />
              <text x={x} y={H - 6} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif" textAnchor="middle">
                {fmtT(t)}
              </text>
            </g>
          );
        })}
        <text x={PADL} y={H - 6} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif">
          {fmtT(startT)}
        </text>
        <text x={W - PADR} y={H - 6} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif" textAnchor="end">
          now{holdH != null ? ` → +${holdH.toFixed(1)}h` : ""}
        </text>
      </svg>
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
    // P(net >= projected): the target is this opportunity's own
    // projected net at scan time — never hand-entered. Empirical CDF over
    // the backtest deciles (linear interpolation, ≈ with 11 points); the
    // probability-weighted mean itself is the "Historical Avg Net" field.
    const probTarget = useMemo(() => {
      const t = net;
      const raw = opp.backtest_deciles_usd;
      if (t == null || !Number.isFinite(t) || !raw || raw.length === 0 || raw.some((v) => !Number.isFinite(v)))
        return null;
      const d = [...raw].sort((a, b) => a - b);
      const n = d.length;
      if (t <= d[0]) return 1;
      if (t > d[n - 1]) return 0;
      for (let i = 0; i < n - 1; i++) {
        const lo = d[i];
        const hi = d[i + 1];
        if (t <= hi) {
          const frac = hi > lo ? (t - lo) / (hi - lo) : 0;
          return Math.min(1, Math.max(0, 1 - (i + Math.min(1, Math.max(0, frac))) / (n - 1)));
        }
      }
      return 0;
    }, [opp.backtest_deciles_usd, net]);
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
        <div className="bg-surface border border-surface-border rounded-2xl w-[880px] max-w-[95vw] max-h-[90vh] flex flex-col shadow-2xl overflow-hidden">
          <div className="flex items-center justify-between px-6 py-3.5 border-b border-surface-border bg-surface shrink-0">
            <div className="flex items-center gap-2 min-w-0">
              <span className="px-2 py-0.5 rounded-md text-xs font-sans font-semibold bg-accent-dim text-accent-strong border border-accent-line">
                Rank #{rank}
              </span>
              <span className="text-sm font-bold font-sans text-slate-900">{opp.symbol}</span>
              <span className="inline-flex items-center gap-1 text-xs font-sans text-slate-500">
                <VenueLogo venueId={opp.long_venue} name={opp.long_venue} size={15} />
                {opp.long_venue}
                <span className="text-slate-300">/</span>
                <VenueLogo venueId={opp.short_venue} name={opp.short_venue} size={15} />
                {opp.short_venue}
              </span>
              <span
                className="text-xs font-sans font-bold text-accent-strong tabular-nums"
                title={`Compounded: ${opp.net_apy_pct.toFixed(2)}% APY`}
              >
                {opp.net_apr_pct.toFixed(2)}% APR
              </span>
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
            <DetailSection title="Route">
              <DetailField
                label="Long leg"
                value={
                  <span className="inline-flex items-center gap-1">
                    <VenueLogo venueId={opp.long_venue} name={opp.long_venue} size={14} />
                    {`${opp.long_venue} · ${opp.long_symbol ?? "?"} (${opp.long_market_type ?? "?"})`}
                  </span>
                }
                tip="Display venue plus the exact execution symbol and market type used for books and orders."
              />
              <DetailField
                label="Short leg"
                value={
                  <span className="inline-flex items-center gap-1">
                    <VenueLogo venueId={opp.short_venue} name={opp.short_venue} size={14} />
                    {`${opp.short_venue} · ${opp.short_symbol ?? "?"} (${opp.short_market_type ?? "?"})`}
                  </span>
                }
                tip="Display venue plus the exact execution symbol and market type used for books and orders."
              />
              <DetailField
                label="Type"
                value={
                  opp.kind === "basis"
                    ? "Basis (SPOT/PERP)"
                    : opp.kind === "fixed_yield"
                    ? "Fixed Yield (perp/perp + 2× Boros YU)"
                    : "Carry (perp/perp)"
                }
                tip="Basis = same-exchange long-spot / short-perp cash-and-carry; Carry = cross-exchange perp/perp funding."
              />
              {opp.kind === "basis" && (
                <DetailField
                  label="Basis divergence"
                  value={opp.basis_bps != null ? `${opp.basis_bps >= 0 ? "+" : ""}${opp.basis_bps.toFixed(1)} bps` : "—"}
                  tip="Signed live divergence (perp − spot) / spot. Positive = contango."
                />
              )}
              {opp.kind === "basis" && opp.spot_price != null && (
                <DetailField
                  label="Spot ref"
                  value={`$${opp.spot_price.toLocaleString(undefined, { maximumFractionDigits: 2 })}`}
                  tip="Spot mid used as the basis reference at scan time."
                />
              )}
              {opp.kind === "basis" && opp.perp_price != null && (
                <DetailField
                  label="Perp ref"
                  value={`$${opp.perp_price.toLocaleString(undefined, { maximumFractionDigits: 2 })}`}
                  tip="Perp mid used as the basis reference at scan time."
                />
              )}
            </DetailSection>

            <DetailSection title="Rates & Income">
              <DetailField
                label={`Long funding · ${opp.long_venue}`}
                value={
                  opp.long_rate_8h_pct != null && Number.isFinite(opp.long_rate_8h_pct)
                    ? `${opp.long_rate_8h_pct >= 0 ? "+" : ""}${opp.long_rate_8h_pct.toFixed(4)}% / 8h`
                    : "—"
                }
                tip="Current funding print of the long leg, 8h-normalized. Spot legs pay none (0). Positive = you pay; negative = you receive."
              />
              <DetailField
                label={`Short funding · ${opp.short_venue}`}
                value={
                  opp.short_rate_8h_pct != null && Number.isFinite(opp.short_rate_8h_pct)
                    ? `${opp.short_rate_8h_pct >= 0 ? "+" : ""}${opp.short_rate_8h_pct.toFixed(4)}% / 8h`
                    : "—"
                }
                tip="Current funding print of the short leg, 8h-normalized. Positive print = the short receives."
              />
              <DetailField
                label="Net 8h rate"
                value={`+${opp.net_rate_8h_pct.toFixed(4)}%`}
                tip="Short-leg funding received minus long-leg funding paid, per 8h. Positive = net income."
              />
              <DetailField
                label="Annualized APR"
                value={`${opp.net_apr_pct.toFixed(2)}%`}
                tip="Net 8h rate annualized (× 1095 intervals/yr), non-compounded."
              />
              <DetailField
                label="Compounded APY"
                value={`${opp.net_apy_pct.toFixed(2)}%`}
                tip="Net rate compounded over each 8h funding interval for a year."
              />
              <DetailField
                label="Daily income"
                value={`+$${opp.estimated_daily_income_usd.toFixed(2)}`}
                tip="Net funding accrued over 24h at the configured trade size — assumes the current print persists."
              />
            </DetailSection>

            <DetailSection title="Costs & Breakeven">
              <DetailField
                label="Round-trip fees"
                value={`-$${fees.toFixed(2)} (${opp.round_trip_fee_bps.toFixed(1)} bps)${spreadCost > 0 ? ` + spread $${spreadCost.toFixed(2)}` : ""}`}
                tip="Entry friction: taker fees plus measured entry-spread cross (both deducted in Projected Net)."
              />
              <DetailField
                label="Entry spread"
                value={
                  opp.entry_spread_bps != null && opp.entry_spread_bps >= 0
                    ? `${opp.entry_spread_bps.toFixed(1)} bps`
                    : "—"
                }
                tip="Measured long-ask vs short-bid cross at executable size."
              />
              <DetailField
                label="Fee breakeven (min)"
                value={`${opp.break_even_hours.toFixed(1)} hrs`}
                tip="Minimum hold to cover round-trip fees at the current 8h rate (excl. entry spread). Compare with Auto hold (expected) for the modelled exit."
              />
              <DetailField
                label="Depth slope"
                value={opp.depth_slope_bps_per_1k != null ? `${opp.depth_slope_bps_per_1k.toFixed(2)} bps/$1k` : "—"}
                tip="Marginal entry-cost gradient summed over both legs at trade size — how fast slippage grows per extra $1k (informational only)."
              />
              <DetailField
                label="Optimal Opportunity Capacity"
                value={
                  opp.optimal_capacity_usd != null && opp.optimal_capacity_usd > 0
                    ? `$${opp.optimal_capacity_usd.toLocaleString(undefined, { maximumFractionDigits: 0 })}`
                    : "—"
                }
                tip="Profit-maximizing size: the depth where the marginal unit's net turns non-positive — beyond it every added dollar shrinks total profit. Walked through both legs' live orderbooks; absent when book depth is unavailable."
              />
              <DetailField
                label="Maximum Opportunity Capacity"
                value={
                  opp.max_capacity_usd != null && opp.max_capacity_usd > 0
                    ? `$${opp.max_capacity_usd.toLocaleString(undefined, { maximumFractionDigits: 0 })}`
                    : "—"
                }
                tip="Largest still-profitable size: the depth where the whole position's cumulative net crosses zero. Sizing beyond it loses money overall even if earlier units earned. Absent when book depth is unavailable."
              />
            </DetailSection>

            <DetailSection title="Outcome Evidence">
              <DetailField
                label="Breakeven (model)"
                value={opp.win_probability != null ? `${(opp.win_probability * 100).toFixed(0)}%` : "—"}
                tip="Probability of covering costs over the expected hold with gap decay (informational only — never gates entries; agrees in sign with Expected)."
              />
              <DetailField
                label={holdIsModelled ? "Auto hold (expected)" : "Auto hold"}
                value={
                  autoH != null
                    ? `${autoH} days${opp.hold_confidence != null ? ` · ${(opp.hold_confidence * 100).toFixed(0)}%` : ""}`
                    : "—"
                }
                tip={
                  opp.hold_reason ??
                  "Fallback: fee break-even only (stale backend without persistence model) — not set manually."
                }
              />
              <DetailField
                label="Projected Net"
                value={
                  net != null ? `${net >= 0 ? "+" : "-"}$${Math.abs(net).toFixed(2)}` : "—"
                }
                tip="Projected net profit for THIS trade: scenario point estimate along the expected (trend) gap path with linear decay to the min-APR exit, net of round-trip fees and entry spread. A model projection, not a measurement — see the Historical rows for measured history."
              />
              <DetailField
                label="Backtest win"
                value={
                  opp.backtest_win_rate != null
                    ? `${(opp.backtest_win_rate * 100).toFixed(0)}%${opp.backtest_episodes != null ? ` (n~${opp.backtest_episodes})` : opp.backtest_trials != null ? ` (n=${opp.backtest_trials})` : ""}`
                    : "—"
                }
                tip={`Measured fraction of trailing-history entries covering costs. Unmeasurable below 20 scoreable entries.${
                  opp.backtest_win_rate_lower != null && opp.backtest_win_rate_upper != null
                    ? ` 90% CI [${(opp.backtest_win_rate_lower * 100).toFixed(0)}% – ${(opp.backtest_win_rate_upper * 100).toFixed(0)}%] on ~${opp.backtest_episodes ?? "?"} effective episode-blocks (consecutive prints overlap — raw n overstates evidence).`
                    : ""
                }${
                  opp.weekday_win_rate != null && opp.weekend_win_rate != null
                    ? ` Weekday ${(opp.weekday_win_rate * 100).toFixed(0)}% / weekend ${(opp.weekend_win_rate * 100).toFixed(0)}%.`
                    : ""
                }`}
              />
              <DetailField
                label="Gap persistence"
                value={
                  opp.persistence_probability != null
                    ? `${(opp.persistence_probability * 100).toFixed(0)}%`
                    : "—"
                }
                tip={`Measured survival probability: P(funding gap stays above the min-APR threshold through the modelled breakeven horizon).${
                  opp.persistence_probability_lower != null
                    ? ` Wilson lower bound ${(opp.persistence_probability_lower * 100).toFixed(0)}%.`
                    : ""
                } Unlike the Breakeven (model) heuristic this is estimated from actual gap lifetimes.`}
              />
              <DetailField
                label="Historical Avg Net"
                value={
                  opp.backtest_expected_usd != null
                    ? `${opp.backtest_expected_usd >= 0 ? "+" : "-"}$${Math.abs(opp.backtest_expected_usd).toFixed(2)}${opp.backtest_expected_usd_se != null ? ` ±${opp.backtest_expected_usd_se.toFixed(2)}` : ""}${opp.backtest_trials != null ? ` (n=${opp.backtest_trials})` : ""}`
                    : "—"
                }
                tip="Average net profit across similar trailing-history entries ± 1 standard error. Measured outcome, not a projection — compare with Projected Net. Unmeasurable below 20 scoreable entries."
              />
              <div className="space-y-0.5">
                <div className="text-[11px] font-sans text-muted font-medium flex items-center">
                  P(≥ projected net)
                  <InfoTooltip
                    text="Empirical probability that a trailing-history entry nets at least THIS opportunity's projected net — the target is the projected net at scan time, not hand-entered. Interpolated over the 11 backtest net-deciles; unmeasurable below 20 scoreable entries. The probability-weighted mean is 'Historical Avg Net'."
                    position="top"
                  />
                </div>
                <div className="flex items-center gap-1.5">
                  <span className="text-sm font-bold font-sans text-accent-strong tabular-nums">
                    {probTarget != null ? `≈${(probTarget * 100).toFixed(0)}%` : "—"}
                  </span>
                  {net != null && (
                    <span className="text-[10px] font-sans text-muted tabular-nums">
                      target {net >= 0 ? "+" : "-"}${Math.abs(net).toFixed(2)}
                    </span>
                  )}
                </div>
                {opp.backtest_deciles_usd != null && opp.backtest_deciles_usd.length >= 11 && (
                  <div className="text-[10px] font-sans text-muted tabular-nums">
                    P10 {opp.backtest_deciles_usd[1] >= 0 ? "+" : "-"}${Math.abs(opp.backtest_deciles_usd[1]).toFixed(0)} · P50 {opp.backtest_deciles_usd[5] >= 0 ? "+" : "-"}${Math.abs(opp.backtest_deciles_usd[5]).toFixed(0)} · P90 {opp.backtest_deciles_usd[9] >= 0 ? "+" : "-"}${Math.abs(opp.backtest_deciles_usd[9]).toFixed(0)}
                  </div>
                )}
              </div>
            </DetailSection>

          <DetailSection
            title="Capacity curve — profit vs position size"
            gridClassName="block"
            tip="Carry profit vs position size walked through both leg orderbooks. Carry income is depth-invariant (net rate × modelled hold) — the curve degrades only through the entry cross. Expected = income scaled by the Breakeven (model) probability; Net = raw. Dashed marker: depth-supported size."
          >
            {(opp.depth_curve?.length ?? 0) >= 2 ? (
              <CapacityChart
                points={opp.depth_curve ?? []}
                optimalSizeUsd={opp.optimal_capacity_usd}
              />
            ) : (
              <span className="text-muted text-xs">
                no live book depth — one or both legs' orderbooks were absent or stale at scan time
              </span>
            )}
          </DetailSection>

          <DetailSection
            title="Funding trend"
            gridClassName="block"
            tip={`Net gap view: the carry rate you earn (short print − long print, %/8h, left axis), resampled on a uniform grid — each venue's last print is carried forward as the standing rate until superseded, so missing samples are never zero-filled. Per-leg view: each venue's prints as step lines. Blue: ${opp.symbol} mid (right axis). Green dashed: model-expected path from the current net print decaying to the min-APR exit over the modelled hold. Red dotted: min-APR exit threshold.`}
          >
            <FundingTrendGraph opp={opp} minAprPct={minAprPct} />
          </DetailSection>
          </div>

          <div className="px-6 py-3.5 border-t border-surface-border bg-surface flex items-center justify-end gap-2 shrink-0">
            <ExecuteButton
              executed={executed}
              executing={executing}
              onClick={() => onExecute(opp)}
              title={executed ? "position active" : "execute carry trade"}
              className="!text-xs !py-2.5 !px-2.5 cursor-pointer"
            />
          </div>
        </div>
      </div>
    );
  },
);



export const FundingRadar: React.FC<FundingRadarProps> = ({
  opportunities,
  availableTickers: _availableTickers,
  symbol,
  tradeSize,
  minAprPct = 8.0,
  onExecuteCarry,
  isLive = false,
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
  // Min Win has no bot-side gate (measured win rate never gates execution),
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
      out = out.filter((o) => ((o.backtest_win_rate ?? -Infinity) * 100) >= mp);
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
  // re-banner every 8s while live strategy entries are disabled backend-wide.
  // Manual clicks always surface the backend reason inline (never throw to
  // the error boundary — an expected fail-closed rejection is not a crash).
  const handleExecute = async (opp: FundingOpportunityDto, opts?: { silent?: boolean; auto?: boolean }) => {
    if (!onExecuteCarry) return;
    if (bot.phase === "paused" || bot.phase === "reduce_only" || bot.phase === "stopping") return;
    setExecutingId(opp.id);
    try {
      await onExecuteCarry(opp, opts?.auto);
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

  // Automated Execution Loop across all monitored tickers when user enables Auto Carry.
  // Autonomous entries are testnet-only — in live mode the loop stays off and the
  // backend refuses auto=true dispatches (manual Execute stays live-gated).
  useEffect(() => {
    if (!bot.entriesAllowed || executingId || !opportunities.length || isPaused || isLive) return;

    const now = Date.now();
    if (now - lastAutoCarryExecRef.current < 8000) return; // 8s cooldown

    // Select the best candidate across 2-leg and 4-leg variants of the
    // same pair. Comparable economics = expected net USD per hour of the
    // row's own committed hold (fixed rows lock to maturity; carry rows
    // carry a modelled hold). Rows without an expectation fall back to
    // their daily-accrual rate.
    const hourlyUsd = (o: FundingOpportunityDto) => {
      const h = o.expected_hold_hours;
      if (o.expected_return_usd != null && h != null && h > 0)
        return o.expected_return_usd / h;
      return (o.estimated_daily_income_usd ?? 0) / 24;
    };
    // fixed_yield rows are live-only (Boros has no testnet) and the auto
    // loop is testnet-only — exclude them here so the bot never picks a
    // row the backend must refuse.
    const topOpp = opportunities
      .filter((o) => o.net_apr_pct >= minAprPct && o.kind !== "fixed_yield")
      .sort((a, b) => hourlyUsd(b) - hourlyUsd(a))[0];
    if (topOpp) {
      lastAutoCarryExecRef.current = now;
      void handleExecute(topOpp, { silent: true, auto: true });
    }
  }, [bot.entriesAllowed, executingId, opportunities, isPaused, minAprPct, isLive]);

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
            <span className="text-muted">Trade Size</span>
            <span className="font-bold text-slate-900 tabular-nums">{tradeSize.toLocaleString()}</span>
            <span className="text-muted">USD</span>
          </div>

          <div
            className="ribbon-pill"
            title="Ignore opportunities below this net APR — edit via Configurator"
          >
            <span className="text-muted">Min APR</span>
            <span className="font-bold text-slate-900 tabular-nums">{minAprPct}</span>
            <span className="text-muted">%</span>
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
            <SlidersHorizontal className="w-3.5 h-3.5 text-accent-strong" />
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
          {isLive && bot.entriesAllowed && (
            <span
              className="text-xs font-mono text-amber-700 max-w-56"
              title="Autonomous entries stay testnet-only in live mode — use manual Execute"
            >
              Live: auto-carry off, manual only
            </span>
          )}
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
                <span className="text-muted font-medium">Venue</span>
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
                  className={`w-3.5 h-3.5 text-muted transition-transform ${showVenueMenu ? "rotate-180" : ""}`}
                />
              </button>
              {showVenueMenu && (
                <div className="absolute right-0 top-full mt-1.5 w-52 bg-surface border border-surface-border rounded-xl shadow-elevated p-1.5 z-30 font-sans text-xs animate-in fade-in zoom-in-95 duration-100">
                  <button
                    type="button"
                    onClick={() => setVenueFilters([])}
                    className={`w-full flex items-center justify-between px-2 py-2 text-left transition rounded-lg cursor-pointer ${
                      venueFilters.length === 0
                        ? "bg-accent/10 text-accent-strong font-semibold"
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
                            ? "bg-accent/10 text-accent-strong font-semibold"
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
              <span className="text-muted font-medium">Min Exp</span>
              <input
                value={minExpBps}
                onChange={(e) => {
                  minExpTouched.current = true;
                  setMinExpBps(e.target.value);
                }}
                placeholder="0"
                className="bg-transparent w-14 outline-none text-right font-bold text-slate-900 font-mono text-xs"
              />
              <span className="text-muted text-[11px] font-medium">BPS</span>
            </div>
            <div
              className="flex items-center gap-1.5 bg-surface border border-surface-border rounded-lg px-2.5 py-1.5 shadow-2xs hover:border-slate-300 focus-within:border-primary focus-within:ring-2 focus-within:ring-primary/15 transition-all"
              title="Display filter — minimum measured backtest win rate. The bot does not gate on win rate, so this defaults to off. Does not affect execution"
            >
              <span className="text-muted font-medium">Min Win</span>
              <input
                value={minWinPct}
                onChange={(e) => setMinWinPct(e.target.value)}
                placeholder="0"
                className="bg-transparent w-14 outline-none text-right font-bold text-slate-900 font-mono text-xs"
              />
              <span className="text-muted text-[11px] font-medium">%</span>
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
                  className="text-accent-strong font-semibold underline underline-offset-2 cursor-pointer"
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
                        <span className="px-1.5 py-0.5 rounded-md text-[10px] font-sans font-semibold bg-accent-dim text-accent-strong border border-accent-line shrink-0">
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
                        {opp.kind === "fixed_yield" && (
                          <span className="px-1.5 py-0.5 rounded-md text-[10px] font-sans font-bold bg-profit-dim text-profit border border-profit/30 shrink-0" title="4-leg variant: perp pair + two Pendle Boros YU legs lock the funding rate to maturity">
                            Fixed Yield
                          </span>
                        )}
                      </div>
                      <span
                        className="text-[10px] font-sans font-bold text-accent-strong tabular-nums shrink-0"
                        title={`Net APR — annualized rate after fees & slippage (compounded: ${opp.net_apy_pct.toFixed(1)}% APY)`}
                      >
                        {opp.net_apr_pct.toFixed(1)}%<span className="text-[8px] font-medium text-accent-strong/70 ml-0.5">APR</span>
                      </span>
                    </div>

                    <div className="px-2 py-1.5 bg-surface-raised rounded-lg border border-surface-border font-sans text-[10px] space-y-1">
                      <div className="flex items-center justify-between gap-1">
                        <span className="text-slate-500">Long:</span>
                        <span className="inline-flex items-center gap-1 font-semibold text-slate-900 min-w-0">
                          <VenueLogo venueId={opp.long_venue} name={opp.long_venue} size={13} />
                          <span className="truncate">{opp.long_venue}</span>
                        </span>
                      </div>
                      <div className="flex items-center justify-between gap-1">
                        <span className="text-slate-500">Short:</span>
                        <span className="inline-flex items-center gap-1 font-semibold text-slate-900 min-w-0">
                          <VenueLogo venueId={opp.short_venue} name={opp.short_venue} size={13} />
                          <span className="truncate">{opp.short_venue}</span>
                        </span>
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
                      {opp.kind === "fixed_yield" && (
                        <>
                          {opp.lock_until_ms != null && (
                            <div className="flex items-center justify-between gap-1">
                              <span className="text-slate-500">Lock until:</span>
                              <span className="font-semibold text-profit tabular-nums">
                                {new Date(opp.lock_until_ms).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" })}
                              </span>
                            </div>
                          )}
                          {opp.yu_hedge_cost_bps != null && (
                            <div className="flex items-center justify-between gap-1">
                              <span className="text-slate-500">YU hedge:</span>
                              <span className="font-semibold text-slate-900 tabular-nums" title="Two Boros YU taker fees + amortized Arbitrum gas">
                                +{opp.yu_hedge_cost_bps.toFixed(1)} bps
                              </span>
                            </div>
                          )}
                          {opp.boros_margin_usd != null && (
                            <div className="flex items-center justify-between gap-1">
                              <span className="text-slate-500">Boros margin:</span>
                              <span className="font-semibold text-slate-900 tabular-nums" title="Estimated collateral lock on Boros for the two YU legs">
                                ${opp.boros_margin_usd >= 1000 ? `${(opp.boros_margin_usd / 1000).toFixed(1)}k` : opp.boros_margin_usd.toFixed(0)}
                              </span>
                            </div>
                          )}
                        </>
                      )}
                      <div className="flex items-center justify-between gap-1">
                        <span className="text-slate-500">Breakeven:</span>
                        <span className="font-semibold text-slate-900 tabular-nums">{opp.break_even_hours.toFixed(1)}h</span>
                      </div>
                      <div className="flex items-center justify-between gap-1">
                        <span className="text-slate-500">Win:</span>
                        <span
                          className={`font-semibold tabular-nums ${opp.backtest_win_rate != null && opp.backtest_win_rate < 0.5 ? "text-loss" : "text-slate-900"}`}
                          title={opp.backtest_trials != null ? `Measured over ${opp.backtest_trials} trailing-history backtest entries${opp.backtest_win_rate_lower != null && opp.backtest_win_rate_upper != null ? ` — 90% CI [${(opp.backtest_win_rate_lower * 100).toFixed(0)}–${(opp.backtest_win_rate_upper * 100).toFixed(0)}%] on ~${opp.backtest_episodes ?? "?"} effective episode-blocks` : ""} (informational only — never gates entries)` : "Win rate unmeasurable — fewer than 20 scoreable history entries"}
                        >
                          {opp.backtest_win_rate != null ? `${(opp.backtest_win_rate * 100).toFixed(0)}%` : "—"}
                        </span>
                      </div>
                      <div className="flex items-center justify-between gap-1">
                        <span className="text-slate-500">Persist:</span>
                        <span
                          className={`font-semibold tabular-nums ${opp.persistence_probability != null ? "text-slate-900" : "text-muted"}`}
                          title={opp.persistence_probability != null ? `Measured P(funding gap survives above min-APR through the breakeven horizon)${opp.persistence_probability_lower != null ? ` — lower bound ${(opp.persistence_probability_lower * 100).toFixed(0)}%` : ""}` : "Persistence unmeasurable — insufficient gap-history entries"}
                        >
                          {opp.persistence_probability != null ? `${(opp.persistence_probability * 100).toFixed(0)}%` : "—"}
                        </span>
                      </div>
                    </div>
                  </div>

                  <div className="pt-2 border-t border-surface-border flex items-center justify-between font-sans text-[10px]">
                    <CarryExpectedInline opp={opp} />
                    <span onClick={(e) => e.stopPropagation()}>
                      <ExecuteButton
                        executed={lastExecuted === opp.id}
                        executing={executingId === opp.id}
                        onClick={() => handleExecute(opp)}
                        title="execute carry trade"
                        className="!text-[10px] !py-2 !px-2 cursor-pointer"
                      />
                    </span>
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
                  ? "text-accent-strong font-bold bg-surface"
                  : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
              }`}
            >
              <span>Current Position</span>
              {fundingPositions.length > 0 && (
              <span className="px-1.5 py-0.5 rounded-md text-[11px] font-sans font-bold min-w-[16px] text-center leading-tight bg-accent-dim text-accent-strong border border-accent-line/60">
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
                  ? "text-accent-strong font-bold bg-surface"
                  : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
              }`}
            >
              <span>Execution History</span>
              {fundingLogs.length > 0 && (
              <span className="px-1.5 py-0.5 rounded-md text-[11px] font-sans font-bold min-w-[16px] text-center leading-tight bg-accent-dim text-accent-strong border border-accent-line/60">
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
                  ? "text-accent-strong font-bold bg-surface"
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
                hint="Carry/basis fills aggregate here per symbol with live PnL after execution. Live entries dispatch on venues with credentials in the unlocked vault and adapter order polling."
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
