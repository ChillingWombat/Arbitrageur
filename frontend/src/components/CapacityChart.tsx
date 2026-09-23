import React, { useMemo, useState } from "react";
import type { DepthCurvePointDto } from "../types";

export type { DepthCurvePointDto };

interface CapacityChartProps {
  points: DepthCurvePointDto[];
  /** Marked as a vertical guide when present. */
  optimalSizeUsd?: number | null;
  breakevenSizeUsd?: number | null;
  height?: number;
}

type Measure = "expected" | "net";
type Metric = "cumulative" | "marginal";
type Unit = "usd" | "bps";

const fmtUsd = (v: number): string =>
  v >= 1000 ? `$${(v / 1000).toFixed(1)}k` : `$${v.toFixed(0)}`;

const fmtBps = (v: number): string => `${v.toFixed(1)}`;

/** Nice-round y ticks within [lo, hi]. */
function niceTicks(lo: number, hi: number, count = 4): number[] {
  if (!(hi > lo)) return [lo];
  const span = hi - lo;
  const rough = span / count;
  const mag = Math.pow(10, Math.floor(Math.log10(rough)));
  const step = [1, 2, 5, 10].map((m) => m * mag).find((s) => span / s <= count + 0.5) ?? 10 * mag;
  const ticks: number[] = [];
  for (let v = Math.ceil(lo / step) * step; v <= hi + 1e-9; v += step) ticks.push(v);
  return ticks;
}

interface SegProps<T extends string> {
  value: T;
  options: Array<[T, string]>;
  onChange: (v: T) => void;
  title?: string;
}

/** Segmented pill switch — same visual language as the gadget filters. */
function Seg<T extends string>({ value, options, onChange, title }: SegProps<T>) {
  return (
    <div
      className="inline-flex items-center bg-surface-raised border border-surface-border rounded-lg p-0.5 gap-0.5"
      title={title}
    >
      {options.map(([v, label]) => (
        <button
          key={v}
          type="button"
          onClick={() => onChange(v)}
          className={`px-2 py-0.5 rounded-md text-[10px] font-mono font-bold transition cursor-pointer ${
            value === v
              ? "bg-accent text-white shadow-2xs"
              : "text-slate-500 hover:text-slate-800"
          }`}
        >
          {label}
        </button>
      ))}
    </div>
  );
}

/**
 * Capacity curve: profit vs position size walked through the orderbooks.
 * Three switch sets — measure (probability-weighted Expected vs raw Net),
 * metric (Cumulative vs Marginal per unit), unit ($ vs bps). X runs 0 →
 * the size where the selected measure's marginal profit stays positive.
 */
export const CapacityChart: React.FC<CapacityChartProps> = ({
  points,
  optimalSizeUsd,
  breakevenSizeUsd,
  height = 150,
}) => {
  const [measure, setMeasure] = useState<Measure>("expected");
  const [metric, setMetric] = useState<Metric>("cumulative");
  const [unit, setUnit] = useState<Unit>("usd");

  const hasExpected = useMemo(() => points.some((p) => p.cumExpBps != null), [points]);

  const view = useMemo(() => {
    // Cutoff: deepest size while the selected measure's marginal stays >0.
    const margAt = (p: DepthCurvePointDto): number | null =>
      measure === "expected" ? p.margExpBps : p.margNetBps;
    let cut = 0;
    for (const p of points) {
      const m = margAt(p);
      if (m != null && m > 0) cut = p.cumUsd;
    }
    const usable = points.filter((p) => p.cumUsd <= Math.max(cut, 1e-9) * 1.0001 || p.cumUsd === cut);
    // Fallback: nothing positive → show the first few chunks so the shape is visible.
    const shown = usable.length >= 2 ? usable : points.slice(0, Math.min(points.length, 8));

    const yOf = (p: DepthCurvePointDto): number | null => {
      const bps =
        metric === "cumulative"
          ? measure === "expected"
            ? p.cumExpBps
            : p.cumNetBps
          : measure === "expected"
            ? p.margExpBps
            : p.margNetBps;
      if (bps == null) return null;
      // USD conversion: cumulative → bps·notional/1e4; marginal → $ per $1k.
      return unit === "usd"
        ? metric === "cumulative"
          ? (bps * p.cumUsd) / 10_000
          : bps / 10
        : bps;
    };
    const series = shown
      .map((p) => ({ x: p.cumUsd, y: yOf(p) }))
      .filter((d): d is { x: number; y: number } => d.y != null);
    return { series, cutoff: cut };
  }, [points, measure, metric, unit]);

  const W = 560;
  const H = height;
  const PAD = { l: 46, r: 12, t: 14, b: 26 };
  const iw = W - PAD.l - PAD.r;
  const ih = H - PAD.t - PAD.b;

  const { series, cutoff } = view;
  const xs = series.map((d) => d.x);
  const ys = series.map((d) => d.y);
  const xMax = Math.max(cutoff, xs[xs.length - 1] ?? 1, 1e-9);
  const yMin = Math.min(0, ...ys);
  const yMax = Math.max(0, ...ys);
  const ySpan = yMax - yMin || 1;
  const sx = (x: number) => PAD.l + (x / xMax) * iw;
  const sy = (y: number) => PAD.t + (1 - (y - yMin) / ySpan) * ih;

  const path = series.map((d, i) => `${i === 0 ? "M" : "L"}${sx(d.x).toFixed(1)},${sy(d.y).toFixed(1)}`).join(" ");
  const ticks = niceTicks(yMin, yMax, 4);
  const xTicks = niceTicks(0, xMax, 4).filter((t) => t > 0);
  const last = series[series.length - 1];
  const unitLabel =
    unit === "usd" ? (metric === "cumulative" ? "$" : "$/1k") : "bps";
  const zeroY = yMin < 0 && yMax > 0 ? sy(0) : null;

  const marker = (v: number | null | undefined, color: string, label: string) =>
    v != null && v > 0 && v <= xMax * 1.02 ? (
      <g key={label}>
        <line x1={sx(v)} y1={PAD.t} x2={sx(v)} y2={PAD.t + ih} stroke={color} strokeDasharray="3 3" strokeWidth="1" opacity="0.7" />
        <text x={sx(v)} y={PAD.t - 3} textAnchor="middle" fontSize="8" fill={color} className="font-mono">
          {label}
        </text>
      </g>
    ) : null;

  return (
    <div className="space-y-1.5">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1.5">
        <Seg<Measure>
          value={measure}
          onChange={setMeasure}
          options={[
            ["expected", "Expected"],
            ["net", "Net"],
          ]}
          title="Expected = win-probability-weighted (p·edge⁺ − friction). Net = raw capture (edge − friction), unweighted."
        />
        <Seg<Metric>
          value={metric}
          onChange={setMetric}
          options={[
            ["cumulative", "Cumulative"],
            ["marginal", "Marginal"],
          ]}
          title="Cumulative = total profit at that size. Marginal = profit rate of the next unit of depth."
        />
        <Seg<Unit>
          value={unit}
          onChange={setUnit}
          options={[
            ["usd", "$"],
            ["bps", "bps"],
          ]}
          title="Profit units: USD (cumulative: total $; marginal: $ per $1k) or bps of notional."
        />
      </div>
      {!hasExpected && measure === "expected" && (
        <div className="text-[10px] font-mono text-slate-500">
          No win-rate curve — expected series unavailable; switch to Net.
        </div>
      )}
      <svg viewBox={`0 0 ${W} ${H}`} className="w-full" role="img">
        {/* gridlines + y ticks */}
        {ticks.map((t) => (
          <g key={t}>
            <line x1={PAD.l} y1={sy(t)} x2={W - PAD.r} y2={sy(t)} stroke="currentColor" strokeWidth="0.5" className="text-slate-200" />
            <text x={PAD.l - 4} y={sy(t) + 3} textAnchor="end" fontSize="9" className="fill-slate-500 font-mono">
              {unit === "usd" ? fmtUsd(t) : fmtBps(t)}
            </text>
          </g>
        ))}
        {zeroY != null && (
          <line x1={PAD.l} y1={zeroY} x2={W - PAD.r} y2={zeroY} stroke="currentColor" strokeWidth="1" strokeDasharray="2 2" className="text-muted" />
        )}
        {/* x ticks */}
        {xTicks.map((t) => (
          <g key={t}>
            <line x1={sx(t)} y1={PAD.t + ih} x2={sx(t)} y2={PAD.t + ih + 3} stroke="currentColor" className="text-slate-300" />
            <text x={sx(t)} y={H - 12} textAnchor="middle" fontSize="9" className="fill-slate-500 font-mono">
              {fmtUsd(t)}
            </text>
          </g>
        ))}
        {/* markers */}
        {marker(optimalSizeUsd, "#7c3aed", "optimal")}
        {marker(breakevenSizeUsd, "#dc2626", "breakeven")}
        {/* series */}
        {series.length >= 2 && (
          <path d={path} fill="none" stroke="var(--color-accent, #2563eb)" strokeWidth="2" strokeLinejoin="round" strokeLinecap="round" />
        )}
        {/* endpoint label */}
        {last && (
          <text x={sx(last.x)} y={sy(last.y) - 5} textAnchor="end" fontSize="9.5" fontWeight="bold" className="fill-accent font-mono">
            {unit === "usd" ? (metric === "cumulative" ? fmtUsd(last.y) : `$${last.y.toFixed(2)}/1k`) : `${fmtBps(last.y)} bps`}
          </text>
        )}
        {/* axis captions */}
        <text x={PAD.l + iw / 2} y={H - 2} textAnchor="middle" fontSize="8.5" className="fill-slate-400 font-mono">
          position size (USD)
        </text>
        <text x={6} y={PAD.t - 4} fontSize="8.5" className="fill-slate-400 font-mono">
          {unitLabel}
        </text>
      </svg>
    </div>
  );
};

export default CapacityChart;
