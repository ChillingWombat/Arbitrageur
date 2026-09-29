import React, { useEffect, useMemo, useState } from "react";
import { LineChart } from "lucide-react";
import { EmptyState } from "./EmptyState";
import { strategyCategory } from "./TradedOpportunitiesGadget";
import { STRATEGY_DISPLAY_NAMES } from "../types/tickers";
import { api } from "../services/api";
import type { PnlHistoryDto, TradedOpportunityDto } from "../types";

/** Filter bucket for account-level flows no strategy order produced
 *  (deposits/withdrawals/venue transfers). Kept distinct from the
 *  `external` strategy id — that one is a manual-trade ledger row, this
 *  one is everything outside the ledger entirely. */
export const EXTERNAL_CATEGORY = "External ops";

/** Every broad family the ledger can produce — shown in the filter even
 *  before a family has realized rows, so the control doesn't lie about
 *  which strategies exist. Order follows the product nav. */
export const KNOWN_PNL_CATEGORIES: string[] = [
  "Options",
  ...Object.values(STRATEGY_DISPLAY_NAMES),
];

interface Pt {
  tsMs: number;
  valueUsd: number;
}

/** Cumulative-sum fold over time-sorted points. */
export const cumulativeSeries = (pts: Pt[]): Pt[] => {
  let acc = 0;
  return pts.map((p) => {
    acc += p.valueUsd;
    return { tsMs: p.tsMs, valueUsd: acc };
  });
};

/** Realized P&L walk for one strategy category (or all). A signal's
 *  realized P&L lands at its close time; createdAt is the fallback for
 *  terminal rows that never stamped a close. Open rows contribute nothing
 *  — unrealized marks are not profit. */
export const realizedSeries = (
  opps: TradedOpportunityDto[],
  category: string,
): Pt[] =>
  cumulativeSeries(
    opps
      .filter(
        (o) =>
          o.realizedPnlUsd != null &&
          (category === "all" || strategyCategory(o.strategyId) === category),
      )
      .map((o) => ({
        tsMs: Date.parse(o.closedAt ?? o.createdAt),
        valueUsd: o.realizedPnlUsd as number,
      }))
      .filter((p) => Number.isFinite(p.tsMs))
      .sort((a, b) => a.tsMs - b.tsMs),
  );

/** Cumulative walk over the external-operation deltas from the backend. */
export const externalSeries = (external: Pt[]): Pt[] =>
  cumulativeSeries([...external].sort((a, b) => a.tsMs - b.tsMs));

const fmtUsdAxis = (v: number): string =>
  Math.abs(v) >= 1000 ? `$${(v / 1000).toFixed(1)}k` : `$${v.toFixed(0)}`;

const fmtUsdVal = (v: number): string =>
  `${v >= 0 ? "+" : "−"}$${Math.abs(v).toLocaleString("en-US", { maximumFractionDigits: 2 })}`;

const fmtTs = (ts: number): string =>
  new Date(ts).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });

/** Nice-round ticks within [lo, hi] — same rule as CapacityChart. */
const niceTicks = (lo: number, hi: number, count = 4): number[] => {
  if (!(hi > lo)) return [lo];
  const span = hi - lo;
  const rough = span / count;
  const mag = Math.pow(10, Math.floor(Math.log10(rough)));
  const step = [1, 2, 5, 10].map((m) => m * mag).find((s) => span / s <= count + 0.5) ?? 10 * mag;
  const ticks: number[] = [];
  for (let v = Math.ceil(lo / step) * step; v <= hi + 1e-9; v += step) ticks.push(v);
  return ticks;
};

/** Small SVG time chart — step line for event-driven cumulative series,
 *  plain polyline for level series (equity). Same visual language as
 *  CapacityChart: mono ticks, slate grid, accent stroke. */
export const SeriesChart: React.FC<{
  points: Pt[];
  step?: boolean;
  height?: number;
}> = ({ points, step = false, height = 160 }) => {
  // Wide viewBox: the svg stretches to container width, so 8px glyphs only
  // stay small if the coordinate space is near the rendered width.
  const W = 760;
  const H = height;
  const PAD = { l: 46, r: 12, t: 12, b: 22 };
  const iw = W - PAD.l - PAD.r;
  const ih = H - PAD.t - PAD.b;

  if (points.length === 0) {
    return <EmptyState plain title="No history yet" />;
  }
  const xs = points.map((p) => p.tsMs);
  const ys = points.map((p) => p.valueUsd);
  const xMin = xs[0];
  const xMax = xs[xs.length - 1];
  const xSpan = Math.max(xMax - xMin, 1);
  const yMin = Math.min(0, ...ys);
  const yMax = Math.max(0, ...ys);
  const ySpan = yMax - yMin || 1;
  const sx = (x: number) => PAD.l + ((x - xMin) / xSpan) * iw;
  const sy = (y: number) => PAD.t + (1 - (y - yMin) / ySpan) * ih;

  const path = points
    .map((p, i) =>
      i === 0
        ? `M${sx(p.tsMs).toFixed(1)},${sy(p.valueUsd).toFixed(1)}`
        : step
          ? `H${sx(p.tsMs).toFixed(1)}V${sy(p.valueUsd).toFixed(1)}`
          : `L${sx(p.tsMs).toFixed(1)},${sy(p.valueUsd).toFixed(1)}`,
    )
    .join(" ");

  const yTicks = niceTicks(yMin, yMax, 4);
  const xTicks = niceTicks(xMin, xMax, 4).filter((t) => t > xMin && t < xMax);
  const last = points[points.length - 1];
  const zeroY = yMin < 0 && yMax > 0 ? sy(0) : null;
  const up = last.valueUsd >= 0;

  return (
    <svg viewBox={`0 0 ${W} ${H}`} className="w-full" role="img">
      {yTicks.map((t) => (
        <g key={t}>
          <line x1={PAD.l} y1={sy(t)} x2={W - PAD.r} y2={sy(t)} stroke="currentColor" strokeWidth="0.5" className="text-slate-200" />
          <text x={PAD.l - 4} y={sy(t) + 2.5} textAnchor="end" fontSize="8" className="fill-slate-500 font-mono">
            {fmtUsdAxis(t)}
          </text>
        </g>
      ))}
      {zeroY != null && (
        <line x1={PAD.l} y1={zeroY} x2={W - PAD.r} y2={zeroY} stroke="currentColor" strokeWidth="1" strokeDasharray="2 2" className="text-muted" />
      )}
      {xTicks.map((t) => (
        <text key={t} x={sx(t)} y={H - 7} textAnchor="middle" fontSize="8" className="fill-slate-500 font-mono">
          {fmtTs(t)}
        </text>
      ))}
      {points.length === 1 ? (
        <circle cx={sx(last.tsMs)} cy={sy(last.valueUsd)} r="2.5" className={up ? "fill-profit" : "fill-loss"} />
      ) : (
        <path d={path} fill="none" strokeWidth="1.6" strokeLinejoin="round" className={up ? "stroke-profit" : "stroke-loss"} />
      )}
    </svg>
  );
};

/** Segmented tab switch — same visual language as the CapacityChart Seg. */
const Tab = <T extends string>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: Array<[T, string]>;
  onChange: (v: T) => void;
}) => (
  <div className="inline-flex items-center bg-surface-raised border border-surface-border rounded-lg p-0.5 gap-0.5">
    {options.map(([v, label]) => (
      <button
        key={v}
        type="button"
        onClick={() => onChange(v)}
        className={`px-2.5 py-0.5 rounded-md text-[10px] font-mono font-bold transition cursor-pointer ${
          value === v ? "bg-accent text-white shadow-2xs" : "text-slate-500 hover:text-slate-800"
        }`}
      >
        {label}
      </button>
    ))}
  </div>
);

const selectClass =
  "px-2 py-1 bg-surface border border-surface-border rounded-md text-[11px] font-mono text-slate-800 focus:outline-none focus:ring-1 focus:ring-accent cursor-pointer";

/** Net P&L through time (realized, per strategy category, plus the
 *  external-ops bucket) and the total net equity walk — the account-level
 *  money story beside the traded-opportunity ledger. */
export const NetPnlChartGadget: React.FC = () => {
  const [opps, setOpps] = useState<TradedOpportunityDto[] | null>(null);
  const [hist, setHist] = useState<PnlHistoryDto | null>(null);
  const [failed, setFailed] = useState(false);
  const [tab, setTab] = useState<"pnl" | "equity">("pnl");
  const [category, setCategory] = useState("all");

  useEffect(() => {
    let alive = true;
    const load = async () => {
      try {
        const [rows, h] = await Promise.all([
          api.listTradedOpportunities(),
          api.getPnlHistory(),
        ]);
        if (!alive) return;
        setOpps(Array.isArray(rows) ? rows : []);
        setHist(h ?? null);
        setFailed(false);
      } catch (err) {
        if (!alive) return;
        console.error("Failed to load P&L history:", err);
        setFailed(true);
      }
    };
    load();
    const interval = setInterval(load, 5000);
    return () => {
      alive = false;
      clearInterval(interval);
    };
  }, []);

  const categories = useMemo(() => {
    const fromData = new Set(
      (opps ?? []).map((o) => strategyCategory(o.strategyId)),
    );
    const list = KNOWN_PNL_CATEGORIES.filter((c, i) => KNOWN_PNL_CATEGORIES.indexOf(c) === i);
    for (const c of [...fromData].sort()) if (!list.includes(c)) list.push(c);
    return list;
  }, [opps]);

  const pnlPoints = useMemo(
    () =>
      category === EXTERNAL_CATEGORY
        ? externalSeries(hist?.external ?? [])
        : realizedSeries(opps ?? [], category),
    [opps, hist, category],
  );
  const equityPoints = hist?.equity ?? [];

  const active = tab === "pnl" ? pnlPoints : equityPoints;
  const lastVal = active.length ? active[active.length - 1].valueUsd : null;

  return (
    <div className="bg-surface rounded-xl border border-surface-border p-4 shadow-card space-y-3">
      <div className="flex items-center justify-between border-b border-surface-border pb-2.5 flex-wrap gap-2">
        <div className="min-w-0">
          <h2 className="text-[13px] font-mono tracking-wider text-slate-900 font-bold flex items-center space-x-2">
            <LineChart className="w-4 h-4 text-accent-strong" />
            <span>P&amp;L &amp; Equity History</span>
          </h2>
        </div>
        <div className="flex items-center gap-3 shrink-0">
          <Tab<"pnl" | "equity">
            value={tab}
            onChange={setTab}
            options={[
              ["pnl", "Net P&L"],
              ["equity", "Net Equity"],
            ]}
          />
          {lastVal != null && (
            <div className="text-right">
              <div className="text-[9px] font-mono uppercase tracking-wider text-slate-500">
                {tab === "pnl" ? (category === "all" ? "All strategies" : category) : "Net equity"}
              </div>
              <div className={`text-sm font-bold font-mono tabular-nums leading-tight ${lastVal >= 0 ? "text-profit" : "text-loss"}`}>
                {fmtUsdVal(lastVal)}
              </div>
            </div>
          )}
        </div>
      </div>

      {tab === "pnl" && (
        <div className="flex items-center gap-2 flex-wrap">
          <select
            value={category}
            onChange={(e) => setCategory(e.target.value)}
            className={selectClass}
            aria-label="Filter P&L by strategy category"
            title="Realized P&L per strategy family; External ops = deposits/withdrawals/transfers no strategy produced"
          >
            <option value="all">All strategies</option>
            {categories.map((c) => (
              <option key={c} value={c}>
                {c}
              </option>
            ))}
            <option value={EXTERNAL_CATEGORY}>{EXTERNAL_CATEGORY}</option>
          </select>
          <span className="ml-auto text-[10px] font-mono text-slate-400 tabular-nums">
            {pnlPoints.length} point{pnlPoints.length === 1 ? "" : "s"}
          </span>
        </div>
      )}

      {failed && (
        <div className="rounded-lg border border-warn-border bg-warn-dim/40 px-3 py-2 text-xs font-mono text-warn-strong">
          P&amp;L history read failed — check backend.
        </div>
      )}

      {tab === "pnl" ? (
        opps == null && !failed ? (
          <EmptyState plain title="Loading P&L history…" />
        ) : (
          <SeriesChart points={pnlPoints} step />
        )
      ) : equityPoints.length === 0 && !failed ? (
        <EmptyState plain title="No equity history yet — margin snapshots accumulate while the engine runs." />
      ) : (
        <SeriesChart points={equityPoints} />
      )}
    </div>
  );
};

export default NetPnlChartGadget;
