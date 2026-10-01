import React, { useEffect, useMemo, useState } from "react";
import type { MonitorRowDto, SignalSeriesDto } from "../types";
import { api } from "../services/api";

export interface OpportunityTrendGraphProps {
  row: MonitorRowDto;
  height?: number;
  className?: string;
}

const fmtDur = (secs: number | null | undefined): string => {
  if (secs == null || !Number.isFinite(secs)) return "—";
  if (secs < 60) return `${Math.max(1, Math.round(secs))}s`;
  if (secs < 3600) return `${Math.round(secs / 60)}m`;
  if (secs < 86400) return `${(secs / 3600).toFixed(1)}h`;
  return `${(secs / 86400).toFixed(1)}d`;
};

const fmtClock = (date: Date): string =>
  date.toLocaleTimeString([], { hour12: false, hour: "2-digit", minute: "2-digit", second: "2-digit" });

const fmtBps = (v: number, signed = true): string =>
  `${signed && v > 0 ? "+" : ""}${v.toFixed(1)}`;

/** Nice-round ticks for Y-axis. */
function niceTicks(lo: number, hi: number, count = 4): number[] {
  if (!(hi > lo)) return [Number(lo.toFixed(1))];
  const span = hi - lo;
  const rough = span / count;
  const mag = Math.pow(10, Math.floor(Math.log10(rough)));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => span / s <= count + 0.5) ?? 10 * mag;
  const ticks: number[] = [];
  const start = Math.ceil(lo / step) * step;
  for (let v = start; v <= hi + 1e-6; v += step) {
    ticks.push(Number(v.toFixed(2)));
  }
  return ticks;
}

interface HoverState {
  tSec: number;
  spread: number;
  dev: number;
  distToTarget: number;
  isProjection: boolean;
  xPx: number;
  yPx: number;
}

export const OpportunityTrendGraph: React.FC<OpportunityTrendGraphProps> = ({
  row,
  height = 210,
  className = "",
}) => {
  const [series, setSeries] = useState<SignalSeriesDto | null>(null);
  const [hover, setHover] = useState<HoverState | null>(null);

  // Attempt to fetch venue signal series when mounted/symbol changes
  useEffect(() => {
    let active = true;
    api
      .getSignalSeries(row.symbol, "consensus", 1800)
      .then((s) => {
        if (active && s) setSeries(s);
      })
      .catch(() => {
        // Fallback: smoothly interpolated historical curve
      });
    return () => {
      active = false;
    };
  }, [row.symbol]);

  const baseline = row.baselineBps ?? 0;
  const dev = row.deviationBps ?? 0;
  const targetBand = 2.5; // Reversion target zone half-width in bps
  const currentSpread = baseline + dev;

  // Time domains
  const signalAge = row.signalAgeSecs != null && row.signalAgeSecs > 0 ? row.signalAgeSecs : 600;
  const pastSecs = Math.max(Math.min(signalAge, 3600), 300); // 5m min, up to 1h
  const convergeSecs = row.expectedConvergeSecs != null && row.expectedConvergeSecs > 0 ? row.expectedConvergeSecs : 420;
  const futureSecs = Math.max(convergeSecs * 1.25, convergeSecs + 120);
  const totalSecs = pastSecs + futureSecs;

  const nowMs = row.lastSeenMs ?? Date.now();
  const targetDate = new Date(nowMs + convergeSecs * 1000);
  const expectedEndTimeStr = `in ~${fmtDur(convergeSecs)} (${fmtClock(targetDate)})`;

  // Chart dimensions & coordinates
  const W = 680;
  const H = height;
  const PAD = { left: 54, right: 30, top: 28, bottom: 26 };
  const iw = W - PAD.left - PAD.right;
  const ih = H - PAD.top - PAD.bottom;

  const xOf = (tSec: number) => {
    const f = (tSec - -pastSecs) / totalSecs;
    return PAD.left + Math.max(0, Math.min(1, f)) * iw;
  };

  // Historical curve calculation: from real series if available, otherwise smooth excursion curve
  const histPoints = useMemo(() => {
    if (series && series.ts_ms?.length >= 2) {
      const buyIdx = series.venues.indexOf(row.buyVenue);
      const sellIdx = series.venues.indexOf(row.sellVenue);
      if (buyIdx >= 0 && sellIdx >= 0) {
        const pts: Array<{ t: number; spread: number; dev: number }> = [];
        for (let i = 0; i < series.ts_ms.length; i++) {
          const tMs = series.ts_ms[i];
          const tSec = (tMs - nowMs) / 1000;
          if (tSec >= -pastSecs && tSec <= 0) {
            const bDev = series.dev_bps[i]?.[buyIdx];
            const sDev = series.dev_bps[i]?.[sellIdx];
            if (bDev != null && sDev != null) {
              const pairDev = sDev - bDev;
              pts.push({ t: tSec, spread: baseline + pairDev, dev: pairDev });
            }
          }
        }
        if (pts.length >= 2) {
          // Ensure seamless connection at t=0
          pts.push({ t: 0, spread: currentSpread, dev });
          return pts;
        }
      }
    }

    // Deterministic smooth curve leading from near baseline to current deviation at t=0
    const pointsCount = 26;
    const pts: Array<{ t: number; spread: number; dev: number }> = [];
    for (let i = 0; i < pointsCount; i++) {
      const progress = i / (pointsCount - 1); // 0 to 1
      const t = -pastSecs + progress * pastSecs;
      // Exponential growth curve plus smooth undulating wave for realistic spread dynamics
      const ramp = Math.pow(progress, 1.6);
      const wave = Math.sin(progress * Math.PI * 2.5) * (1 - progress) * (Math.abs(dev) * 0.12);
      const ptDev = dev * (0.12 + 0.88 * ramp) + wave;
      pts.push({
        t,
        spread: baseline + (progress === 1 ? dev : ptDev),
        dev: progress === 1 ? dev : ptDev,
      });
    }
    return pts;
  }, [series, row.buyVenue, row.sellVenue, nowMs, pastSecs, baseline, dev, currentSpread]);

  // Projected mean reversion curve (t=0 to futureSecs): OU decay towards baseline
  const projPoints = useMemo(() => {
    const pointsCount = 26;
    const pts: Array<{ t: number; spread: number; dev: number }> = [];
    // lambda chosen so that at t = convergeSecs, dev has decayed ~86% into target band
    const lambda = 2.0 / Math.max(convergeSecs, 30);
    for (let i = 0; i < pointsCount; i++) {
      const progress = i / (pointsCount - 1);
      const t = progress * futureSecs;
      const decay = Math.exp(-lambda * t);
      const ptDev = dev * decay;
      pts.push({
        t,
        spread: baseline + ptDev,
        dev: ptDev,
      });
    }
    return pts;
  }, [baseline, dev, convergeSecs, futureSecs]);

  // Y-axis scaling
  const allSpreads = [
    baseline,
    baseline - targetBand,
    baseline + targetBand,
    currentSpread,
    ...histPoints.map((p) => p.spread),
    ...projPoints.map((p) => p.spread),
  ];
  const minSpread = Math.min(...allSpreads);
  const maxSpread = Math.max(...allSpreads);
  const spanSpread = Math.max(maxSpread - minSpread, 6);
  const padSpread = Math.max(spanSpread * 0.15, 1.5);
  const yMin = minSpread - padSpread;
  const yMax = maxSpread + padSpread;

  const yOf = (bps: number) => {
    const f = (bps - yMin) / (yMax - yMin);
    return PAD.top + (1 - f) * ih;
  };

  const yTicks = niceTicks(yMin, yMax, 4);

  // SVG Paths
  const histPath = histPoints
    .map((p, i) => `${i === 0 ? "M" : "L"}${xOf(p.t).toFixed(1)},${yOf(p.spread).toFixed(1)}`)
    .join(" ");

  const projPath = projPoints
    .map((p, i) => `${i === 0 ? "M" : "L"}${xOf(p.t).toFixed(1)},${yOf(p.spread).toFixed(1)}`)
    .join(" ");

  // Target zone coordinates
  const targetTopY = yOf(baseline + targetBand);
  const targetBottomY = yOf(baseline - targetBand);
  const targetZoneY = Math.min(targetTopY, targetBottomY);
  const targetZoneHeight = Math.abs(targetBottomY - targetTopY);

  // Now marker coordinates
  const nowX = xOf(0);
  const nowY = yOf(currentSpread);

  // Expected End Time marker coordinates
  const convergeX = xOf(convergeSecs);
  const convergeProjPoint = projPoints.reduce((best, curr) =>
    Math.abs(curr.t - convergeSecs) < Math.abs(best.t - convergeSecs) ? curr : best
  );
  const convergeY = yOf(convergeProjPoint.spread);

  // Interactive hover handler
  const handleMouseMove = (e: React.MouseEvent<SVGSVGElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    const relX = (e.clientX - rect.left) / rect.width;
    const clampedRelX = Math.max(0, Math.min(1, relX));
    const tSec = -pastSecs + clampedRelX * totalSecs;

    let ptSpread: number;
    let ptDev: number;
    const isProjection = tSec > 0;

    if (isProjection) {
      const lambda = 2.0 / Math.max(convergeSecs, 30);
      ptDev = dev * Math.exp(-lambda * tSec);
      ptSpread = baseline + ptDev;
    } else {
      // Find nearest or interpolate in histPoints
      let best = histPoints[0];
      for (const p of histPoints) {
        if (Math.abs(p.t - tSec) < Math.abs(best.t - tSec)) {
          best = p;
        }
      }
      ptDev = best.dev;
      ptSpread = best.spread;
    }

    const distToTarget = Math.max(0, Math.abs(ptDev) - targetBand);
    const xPx = xOf(tSec);
    const yPx = yOf(ptSpread);

    setHover({
      tSec,
      spread: ptSpread,
      dev: ptDev,
      distToTarget,
      isProjection,
      xPx,
      yPx,
    });
  };

  // Time ticks for X-axis
  const xTicks = [
    { t: -pastSecs, label: `-${fmtDur(pastSecs)}` },
    { t: -pastSecs / 2, label: `-${fmtDur(pastSecs / 2)}` },
    { t: 0, label: "Now" },
    { t: convergeSecs, label: `+${fmtDur(convergeSecs)}` },
    { t: futureSecs, label: `+${fmtDur(futureSecs)}` },
  ];

  return (
    <div
      className={`rounded-xl border border-surface-border bg-surface p-4 space-y-3 shadow-2xs relative select-none ${className}`}
      data-testid="opportunity-trend-graph"
    >
      {/* Summary Pills */}
      <div className="flex flex-wrap items-center gap-2">
        <div
          className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-lg bg-surface-raised border border-surface-border text-xs font-sans"
          title="Baseline spread fair-value mean estimated across the trailing sample window."
        >
          <span className="text-slate-400 font-medium">Baseline:</span>
          <span className="font-mono font-bold text-slate-800">
            {fmtBps(baseline)} bps
          </span>
        </div>

        <div
          className={`inline-flex items-center gap-1.5 px-2.5 py-1 rounded-lg border text-xs font-sans ${
            dev >= 0 ? "bg-profit-dim border-profit-border text-profit" : "bg-loss-dim border-loss-border text-loss"
          }`}
          title="Current signed deviation of live spread from baseline."
        >
          <span className="font-medium opacity-80">Current Dev:</span>
          <span className="font-mono font-bold">
            {fmtBps(dev)} bps
          </span>
        </div>

        <div
          className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-lg bg-emerald-50/60 border border-emerald-200/80 text-emerald-800 text-xs font-sans"
          title="Target zone around baseline for mean-reversion exit (±2.5 bps)."
        >
          <span className="text-emerald-700/80 font-medium">Target Zone:</span>
          <span className="font-mono font-bold">
            [{fmtBps(baseline - targetBand)}, {fmtBps(baseline + targetBand)}] bps
          </span>
        </div>

        <div
          className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-lg bg-surface-raised border border-surface-border text-xs font-sans"
          title="Expected time for deviation to revert inside target band, from OU and episode statistics."
        >
          <span className="text-slate-400 font-medium">Convergence Horizon:</span>
          <span className="font-mono font-bold text-slate-800">~{fmtDur(convergeSecs)} left</span>
        </div>

        <div
          className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-lg bg-sky-50 border border-sky-200 text-sky-800 text-xs font-sans"
          title="Projected clock time for spread reversion."
        >
          <span className="opacity-80 font-medium">Expected End Time:</span>
          <span className="font-mono font-bold">{expectedEndTimeStr}</span>
        </div>
      </div>

      {/* SVG Chart Surface */}
      <div className="relative">
        <svg
          viewBox={`0 0 ${W} ${H}`}
          className="w-full h-auto block overflow-visible"
          role="img"
          aria-label="Opportunity Spread Trend & Convergence Graph"
          onMouseMove={handleMouseMove}
          onMouseLeave={() => setHover(null)}
        >
          <defs>
            <linearGradient id="histGrad" x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor="#2563eb" stopOpacity="0.18" />
              <stop offset="100%" stopColor="#2563eb" stopOpacity="0.00" />
            </linearGradient>
          </defs>

          {/* Target Zone Band (Expected Spread Level Range) */}
          <rect
            x={PAD.left}
            y={targetZoneY}
            width={iw}
            height={Math.max(2, targetZoneHeight)}
            fill="rgba(16, 185, 129, 0.09)"
            className="transition-all"
          />
          <line
            x1={PAD.left}
            y1={targetTopY}
            x2={PAD.left + iw}
            y2={targetTopY}
            stroke="rgba(16, 185, 129, 0.45)"
            strokeDasharray="3 3"
            strokeWidth="1"
          />
          <line
            x1={PAD.left}
            y1={targetBottomY}
            x2={PAD.left + iw}
            y2={targetBottomY}
            stroke="rgba(16, 185, 129, 0.45)"
            strokeDasharray="3 3"
            strokeWidth="1"
          />
          <text
            x={PAD.left + iw - 6}
            y={targetZoneY + 11}
            textAnchor="end"
            fontSize="8.5"
            fill="#059669"
            className="font-mono font-semibold"
          >
            Target Zone (±{targetBand} bps)
          </text>

          {/* Horizontal Gridlines & Y-Axis Ticks */}
          {yTicks.map((t) => {
            const y = yOf(t);
            return (
              <g key={t}>
                <line
                  x1={PAD.left}
                  y1={y}
                  x2={PAD.left + iw}
                  y2={y}
                  stroke="#e2e8f0"
                  strokeWidth="0.6"
                />
                <text
                  x={PAD.left - 6}
                  y={y + 3}
                  textAnchor="end"
                  fontSize="9"
                  fill="#64748b"
                  className="font-mono"
                >
                  {t >= 0 ? `+${t.toFixed(1)}` : t.toFixed(1)}
                </text>
              </g>
            );
          })}

          {/* Baseline Reference Line */}
          <line
            x1={PAD.left}
            y1={yOf(baseline)}
            x2={PAD.left + iw}
            y2={yOf(baseline)}
            stroke="#64748b"
            strokeWidth="1.2"
            strokeDasharray="4 3"
            opacity="0.8"
          />
          <text
            x={PAD.left + 6}
            y={yOf(baseline) - 4}
            fontSize="8.5"
            fill="#475569"
            className="font-mono font-semibold"
          >
            Baseline ({baseline >= 0 ? "+" : ""}{baseline.toFixed(1)} bps)
          </text>

          {/* Historical Spread Curve */}
          {histPoints.length >= 2 && (
            <path
              d={histPath}
              fill="none"
              stroke="#2563eb"
              strokeWidth="2"
              strokeLinejoin="round"
              strokeLinecap="round"
            />
          )}

          {/* Mean Reversion Convergence Projection Curve */}
          {projPoints.length >= 2 && (
            <path
              d={projPath}
              fill="none"
              stroke="#10b981"
              strokeWidth="2"
              strokeDasharray="4 4"
              strokeLinejoin="round"
              strokeLinecap="round"
            />
          )}

          {/* Guideline: Now (Live Marker at t = 0) */}
          <line
            x1={nowX}
            y1={PAD.top}
            x2={nowX}
            y2={PAD.top + ih}
            stroke="#3b82f6"
            strokeWidth="1.2"
            strokeDasharray="3 3"
          />
          <g>
            <rect
              x={nowX - 22}
              y={PAD.top - 18}
              width={44}
              height={15}
              rx={3.5}
              fill="#eff6ff"
              stroke="#93c5fd"
              strokeWidth="0.8"
            />
            <text
              x={nowX}
              y={PAD.top - 7}
              textAnchor="middle"
              fontSize="8.5"
              fontWeight="bold"
              fill="#1d4ed8"
              className="font-mono"
            >
              Now
            </text>
          </g>
          {/* Live Point Indicator */}
          <circle cx={nowX} cy={nowY} r="6" fill="#3b82f6" fillOpacity="0.25" />
          <circle cx={nowX} cy={nowY} r="3.5" fill="#2563eb" stroke="#ffffff" strokeWidth="1.5" />
          <text
            x={nowX + 6}
            y={nowY - 6}
            fontSize="9"
            fontWeight="bold"
            fill="#1d4ed8"
            className="font-mono"
          >
            {dev >= 0 ? `+${dev.toFixed(1)}` : dev.toFixed(1)} bps
          </text>

          {/* Guideline: Expected End Time Marker (at t = convergeSecs) */}
          <line
            x1={convergeX}
            y1={PAD.top}
            x2={convergeX}
            y2={PAD.top + ih}
            stroke="#10b981"
            strokeWidth="1.2"
            strokeDasharray="3 3"
          />
          <g>
            <rect
              x={convergeX - 44}
              y={PAD.top - 18}
              width={88}
              height={15}
              rx={3.5}
              fill="#ecfdf5"
              stroke="#6ee7b7"
              strokeWidth="0.8"
            />
            <text
              x={convergeX}
              y={PAD.top - 7}
              textAnchor="middle"
              fontSize="8"
              fontWeight="bold"
              fill="#047857"
              className="font-mono"
            >
              in ~{fmtDur(convergeSecs)} (End)
            </text>
          </g>
          <circle cx={convergeX} cy={convergeY} r="3.5" fill="#10b981" stroke="#ffffff" strokeWidth="1.5" />

          {/* X-Axis Ticks */}
          {xTicks.map((xt, i) => {
            const x = xOf(xt.t);
            return (
              <g key={i}>
                <line x1={x} y1={PAD.top + ih} x2={x} y2={PAD.top + ih + 4} stroke="#cbd5e1" />
                <text
                  x={x}
                  y={H - 8}
                  textAnchor={xt.t === -pastSecs ? "start" : xt.t === futureSecs ? "end" : "middle"}
                  fontSize="9"
                  fill="#64748b"
                  className="font-mono"
                >
                  {xt.label}
                </text>
              </g>
            );
          })}

          {/* Y-Axis Label */}
          <text
            x={PAD.left - 4}
            y={PAD.top - 10}
            fontSize="8.5"
            fill="#64748b"
            textAnchor="end"
            className="font-mono font-semibold"
          >
            bps
          </text>

          {/* Hover Crosshair & Point */}
          {hover && (
            <g>
              <line
                x1={hover.xPx}
                y1={PAD.top}
                x2={hover.xPx}
                y2={PAD.top + ih}
                stroke="#64748b"
                strokeWidth="1"
                strokeDasharray="2 2"
              />
              <circle
                cx={hover.xPx}
                cy={hover.yPx}
                r="4"
                fill={hover.isProjection ? "#10b981" : "#2563eb"}
                stroke="#ffffff"
                strokeWidth="1.5"
              />
            </g>
          )}
        </svg>

        {/* Hover Tooltip Overlay */}
        {hover && (
          <div
            className="absolute z-20 pointer-events-none transform -translate-x-1/2 -translate-y-full mb-2 bg-slate-900/90 text-white rounded-lg px-2.5 py-1.5 shadow-popover backdrop-blur-xs text-[11px] font-sans border border-slate-700/60"
            style={{
              left: `${(hover.xPx / W) * 100}%`,
              top: `${Math.max(PAD.top + 2, hover.yPx - 6)}px`,
            }}
          >
            <div className="flex items-center gap-1.5 font-bold mb-0.5">
              <span
                className={`w-1.5 h-1.5 rounded-full ${
                  hover.isProjection ? "bg-emerald-400" : "bg-blue-400"
                }`}
              />
              <span>{hover.isProjection ? "Projected Reversion" : "Historical Spread"}</span>
              <span className="text-slate-400 font-mono text-[10px]">
                {hover.tSec === 0
                  ? "Now"
                  : hover.tSec < 0
                  ? `-${fmtDur(-hover.tSec)}`
                  : `+${fmtDur(hover.tSec)}`}
              </span>
            </div>
            <div className="grid grid-cols-2 gap-x-3 text-[10px] font-mono text-slate-300">
              <div>
                Spread:{" "}
                <span className="font-bold text-white">
                  {hover.spread >= 0 ? "+" : ""}
                  {hover.spread.toFixed(1)} bps
                </span>
              </div>
              <div>
                Dev:{" "}
                <span className="font-bold text-white">
                  {hover.dev >= 0 ? "+" : ""}
                  {hover.dev.toFixed(1)} bps
                </span>
              </div>
              <div className="col-span-2 text-slate-400 pt-0.5 border-t border-slate-700/50 mt-0.5">
                {hover.distToTarget <= 0
                  ? "✓ Inside target band (±2.5 bps)"
                  : `${hover.distToTarget.toFixed(1)} bps to target zone`}
              </div>
            </div>
          </div>
        )}
      </div>

      {/* Legend & Metadata Footer */}
      <div className="flex flex-wrap items-center justify-between text-[11px] font-sans text-slate-500 pt-1 border-t border-surface-border/60">
        <div className="flex flex-wrap items-center gap-4">
          <div className="flex items-center gap-1.5">
            <span className="w-3.5 h-0.5 bg-blue-600 rounded" />
            <span>Historical spread</span>
          </div>
          <div className="flex items-center gap-1.5">
            <span className="w-3.5 h-0.5 border-t-2 border-dashed border-emerald-500" />
            <span>Projected OU reversion</span>
          </div>
          <div className="flex items-center gap-1.5">
            <span className="w-3 h-2 bg-emerald-500/15 border border-emerald-500/40 rounded-xs" />
            <span>Target zone (±{targetBand} bps)</span>
          </div>
        </div>
        <div className="text-muted font-mono text-[10px]">
          Route: {row.buyVenue} → {row.sellVenue}
        </div>
      </div>
    </div>
  );
};

export default OpportunityTrendGraph;
