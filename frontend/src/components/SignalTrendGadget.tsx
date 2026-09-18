import React, { useEffect, useMemo, useRef, useState } from "react";
import { Activity } from "lucide-react";
import type { SignalSeriesDto } from "../types";
import { api } from "../services/api";
import { formatDuration } from "../services/matrixQuant";
import { sessionSegments, SESSION_BAND_COLOR } from "../services/session.ts";
import { InfoTooltip } from "./InfoTooltip";
import { EmptyState } from "./EmptyState";

interface SignalTrendGadgetProps {
  symbol: string;
  /** The bot's configured trailing window (seconds) — the default horizon. */
  botWindowSec: number;
  /** Minimum net spread (bps) required for an executable pair — tints the band. */
  hurdleBps: number;
}

type BaselineMode = "consensus" | "leader";

const VENUE_COLORS = ["#2563eb", "#f59e0b", "#8b5cf6", "#10b981", "#ef4444", "#06b6d4", "#ec4899"];

const shortVenueName = (name: string) => name.split("(")[0].trim() || name;

const fmtBps = (v: number, signed = true) =>
  `${signed && v > 0 ? "+" : ""}${v.toFixed(1)}`;

const fmtWindow = formatDuration;

/** Span-aware x-axis labels: sub-hour -> clock, multi-day -> date+time. */
const fmtTick = (t: number, spanMs: number) => {
  const d = new Date(t);
  if (spanMs >= 2 * 86_400_000)
    return d.toLocaleDateString([], { month: "numeric", day: "numeric" }) +
      " " + d.toLocaleTimeString([], { hour12: false, hour: "2-digit", minute: "2-digit" });
  if (spanMs >= 3_600_000)
    return d.toLocaleTimeString([], { hour12: false, hour: "2-digit", minute: "2-digit" });
  return d.toLocaleTimeString([], { hour12: false });
};

export const SignalTrendGadget: React.FC<SignalTrendGadgetProps> = React.memo(SignalTrendGadgetInner);

function SignalTrendGadgetInner({
  symbol,
  botWindowSec,
  hurdleBps,
}: SignalTrendGadgetProps) {
  const [baselineMode, setBaselineMode] = useState<BaselineMode>("consensus");
  const [windowSec, setWindowSec] = useState(botWindowSec);
  const [series, setSeries] = useState<SignalSeriesDto | null>(null);
  const [failed, setFailed] = useState(false);
  const [hoverIdx, setHoverIdx] = useState<number | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [chartWidth, setChartWidth] = useState<number>(0);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const update = () => {
      const w = el.clientWidth;
      if (w > 0) {
        setChartWidth((prev) => (prev === w ? prev : w));
      }
    };
    update();
    if (typeof ResizeObserver !== "undefined") {
      const ro = new ResizeObserver((entries) => {
        for (const entry of entries) {
          const w = Math.round(entry.contentRect.width);
          if (w > 0) {
            setChartWidth((prev) => (prev === w ? prev : w));
          }
        }
      });
      ro.observe(el);
      return () => ro.disconnect();
    }
    window.addEventListener("resize", update);
    return () => window.removeEventListener("resize", update);
  }, []);

  const windowOptions = useMemo(() => {
    const opts = [{ s: botWindowSec, label: fmtWindow(botWindowSec) }];
    for (const s of [300, 1800, 3600, 14400, 86400, 259200, 604800]) {
      if (!opts.some((o) => o.s === s)) opts.push({ s, label: fmtWindow(s) });
    }
    opts.sort((a, b) => a.s - b.s);
    return opts;
  }, [botWindowSec]);

  useEffect(() => {
    setWindowSec(botWindowSec);
    setSeries(null);
    setFailed(false);
  }, [symbol, botWindowSec]);

  useEffect(() => {
    let stopped = false;
    const cadence = windowSec <= 900 ? 3000 : 15000;
    const load = async () => {
      try {
        const s = await api.getSignalSeries(symbol, baselineMode, windowSec);
        if (!stopped) {
          setSeries(s);
          setFailed(false);
        }
      } catch {
        if (!stopped) setFailed(true);
      }
    };
    load();
    const iv = setInterval(load, cadence);
    return () => {
      stopped = true;
      clearInterval(iv);
    };
  }, [symbol, baselineMode, windowSec]);

  const venueIds = series?.venues ?? [];
  const venueColor = (vid: string) =>
    VENUE_COLORS[venueIds.indexOf(vid) % VENUE_COLORS.length];
  const venueName = (vid: string) =>
    shortVenueName(series?.venue_names?.[vid] ?? vid);

  const pointCount = series?.ts_ms.length ?? 0;
  const t0 = series?.ts_ms[0] ?? 0;
  const t1 = series?.ts_ms[pointCount - 1] ?? 1;
  const spanMs = Math.max(1, t1 - t0);

  // Buckets where at least two venues were observed — the band only has
  // meaning (a pairwise spread) there.
  const bandIdx = useMemo(() => {
    if (!series) return [] as number[];
    const out: number[] = [];
    series.dev_bps.forEach((row, i) => {
      if (row.filter((v) => v !== null).length >= 2) out.push(i);
    });
    return out;
  }, [series]);

  // Gap threshold: if consecutive timestamps differ by more than ~3 nominal
  // bucket steps (min 60s), consider it an engine downtime gap so we don't
  // draw fake interpolation lines or a false dislocation band across it.
  const nominalBucketMs = Math.max(1000, Math.floor((windowSec * 1000) / 500));
  const gapThreshold = Math.max(nominalBucketMs * 3, 60_000);

  const latestBandIdx = bandIdx[bandIdx.length - 1];
  // Only treat the band as "current" when its newest ≥2-venue bucket is at
  // the head of the window — otherwise the badge would quote a stale spread.
  const bandIsFresh =
    latestBandIdx !== undefined &&
    pointCount - 1 - latestBandIdx <= 2 &&
    (pointCount === 0 || Date.now() - (series?.ts_ms[pointCount - 1] ?? 0) <= gapThreshold);
  const dislocationBps =
    bandIsFresh && series
      ? series.band_max_bps[latestBandIdx] - series.band_min_bps[latestBandIdx]
      : 0;
  const isExecutableSpread = dislocationBps >= hurdleBps;

  // Latest deviation per venue (last non-null observation), A→Z by venue name.
  const legend = useMemo(() => {
    if (!series) return [];
    const rows: { vid: string; dev: number }[] = [];
    series.venues.forEach((vid, j) => {
      for (let i = series.dev_bps.length - 1; i >= 0; i--) {
        const v = series.dev_bps[i][j];
        if (v !== null) {
          rows.push({ vid, dev: v });
          break;
        }
      }
    });
    rows.sort((a, b) =>
      shortVenueName(series.venue_names?.[a.vid] ?? a.vid).localeCompare(
        shortVenueName(series.venue_names?.[b.vid] ?? b.vid),
        undefined,
        { sensitivity: "base" },
      ),
    );
    return rows;
  }, [series]);

  // Chart geometry — W tracks the measured container width so lines and
  // gridlines stretch across the full available space without empty margins.
  const PAD_L = 46;
  const PAD_R = 10;
  const PAD_T = 10;
  const PAD_B = 20;
  const W = Math.max(320, chartWidth || 800);
  const H = 176;

  const maxAbsDev = useMemo(() => {
    let m = 0;
    if (series) {
      for (const row of series.dev_bps) {
        for (const v of row) if (v !== null) m = Math.max(m, Math.abs(v));
      }
    }
    return Math.max(1, m * 1.15);
  }, [series]);

  const xOf = (t: number): number =>
    PAD_L + ((t - t0) / spanMs) * (W - PAD_L - PAD_R);
  const yOf = (dev: number): number =>
    PAD_T + (1 - (dev + maxAbsDev) / (2 * maxAbsDev)) * (H - PAD_T - PAD_B);

  // Per-venue paths: a null bucket or downtime gap breaks the line (gaps stay gaps).
  const venuePaths = useMemo(() => {
    if (!series) return new Map<string, string>();
    const map = new Map<string, string>();
    series.venues.forEach((vid, j) => {
      let d = "";
      let pen = false; // false after a gap -> next point starts a new segment
      series.ts_ms.forEach((t, i) => {
        const v = series.dev_bps[i][j];
        if (v === null) {
          pen = false;
          return;
        }
        if (i > 0 && t - series.ts_ms[i - 1] > gapThreshold) {
          pen = false;
        }
        d += `${pen ? "L" : "M"}${xOf(t).toFixed(1)},${yOf(v).toFixed(1)}`;
        pen = true;
      });
      map.set(vid, d);
    });
    return map;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [series, maxAbsDev, t0, spanMs, W, H, gapThreshold]);

  // Isolated observations (no adjacent non-null bucket within gapThreshold)
  // draw no visible line — render them as dots so sparse history doesn't vanish.
  const isolatedDots = useMemo(() => {
    if (!series) return [] as { vid: string; x: number; y: number }[];
    const n = series.ts_ms.length;
    const dots: { vid: string; x: number; y: number }[] = [];
    series.venues.forEach((vid, j) => {
      for (let i = 0; i < n; i++) {
        const v = series.dev_bps[i][j];
        if (v === null) continue;
        const prevNullOrGap =
          i === 0 ||
          series.dev_bps[i - 1][j] === null ||
          series.ts_ms[i] - series.ts_ms[i - 1] > gapThreshold;
        const nextNullOrGap =
          i === n - 1 ||
          series.dev_bps[i + 1][j] === null ||
          series.ts_ms[i + 1] - series.ts_ms[i] > gapThreshold;
        if (prevNullOrGap && nextNullOrGap) {
          dots.push({ vid, x: xOf(series.ts_ms[i]), y: yOf(v) });
        }
      }
    });
    return dots;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [series, maxAbsDev, t0, spanMs, W, H, gapThreshold]);

  // Dislocation band: partitioned into contiguous segments so downtime gaps aren't shaded.
  const bandPath = useMemo(() => {
    if (!series || bandIdx.length < 2) return "";
    const segments: number[][] = [];
    let current: number[] = [];
    for (let k = 0; k < bandIdx.length; k++) {
      const idx = bandIdx[k];
      if (current.length > 0) {
        const prevIdx = current[current.length - 1];
        if (series.ts_ms[idx] - series.ts_ms[prevIdx] > gapThreshold) {
          segments.push(current);
          current = [];
        }
      }
      current.push(idx);
    }
    if (current.length > 0) {
      segments.push(current);
    }

    const paths: string[] = [];
    for (const seg of segments) {
      if (seg.length < 2) continue;
      const top = seg.map(
        (i, k) =>
          `${k === 0 ? "M" : "L"}${xOf(series.ts_ms[i]).toFixed(1)},${yOf(series.band_max_bps[i]).toFixed(1)}`,
      );
      const bottom = [...seg]
        .reverse()
        .map(
          (i) =>
            `L${xOf(series.ts_ms[i]).toFixed(1)},${yOf(series.band_min_bps[i]).toFixed(1)}`,
        );
      paths.push(`${top.join(" ")} ${bottom.join(" ")} Z`);
    }
    return paths.join(" ");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [series, bandIdx, maxAbsDev, t0, spanMs, W, H, gapThreshold]);

  const hoverDevs = useMemo(() => {
    if (hoverIdx === null || !series || !series.ts_ms[hoverIdx]) return null;
    const map = new Map<string, number | null>();
    series.venues.forEach((vid, j) => {
      map.set(vid, series.dev_bps[hoverIdx][j]);
    });
    return map;
  }, [hoverIdx, series]);

  const historyLabel = useMemo(() => {
    if (!series?.earliest_ts_ms) return null;
    const hrs = (Date.now() - series.earliest_ts_ms) / 3_600_000;
    return hrs >= 48
      ? `${(hrs / 24).toFixed(1)}d history`
      : hrs >= 1
        ? `${hrs.toFixed(1)}h history`
        : `${Math.max(1, Math.round(hrs * 60))}m history`;
  }, [series?.earliest_ts_ms]);

  const hasSeries = pointCount >= 2;

  // Session overlay: UTC regime bands behind the series, derived from wall
  // time so they render even before any profile calibrates.
  const sessionRects = useMemo(
    () => (hasSeries ? sessionSegments(t0, t1) : []),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [hasSeries, t0, t1],
  );

  return (
    <div className="space-y-3 font-mono text-xs select-none">
      {/* Header: title, dislocation badge, controls */}
      <div className="flex flex-wrap items-center justify-between gap-3 pb-2.5 border-b border-surface-border">
        <div className="flex items-center space-x-2">
          <span className="text-sm text-slate-900 flex items-center">
            Venue deviation from baseline
            <InfoTooltip
              position="bottom"
              text="Each line is a venue's mid-price deviation from the baseline, in basis points. Above 0 = expensive (sell candidate); below 0 = cheap (buy candidate). The shaded band spans the cheapest-to-richest gap — its height is the gross pairwise spread and it tints green when it exceeds the hurdle. Computed in Rust over persisted samples; gaps are real gaps."
            />
          </span>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          {/* Baseline selector */}
          <div className="flex items-center space-x-1 bg-surface-raised p-0.5 rounded-lg border border-surface-border text-[11px]">
            {(["consensus", "leader"] as const).map((m) => (
              <button
                key={m}
                type="button"
                onClick={() => setBaselineMode(m)}
                title={
                  m === "consensus"
                    ? "Baseline = volume-weighted mean of venue mids"
                    : "Baseline = most-liquid venue's mid (anchored at 0)"
                }
                className={`px-2 py-1 rounded font-medium transition cursor-pointer ${
                  baselineMode === m
                    ? "bg-accent text-white shadow-2xs"
                    : "text-slate-500 hover:text-slate-900"
                }`}
              >
                {m === "consensus" ? "Consensus" : "Leader"}
              </button>
            ))}
          </div>
          {/* Window selector — default mirrors the bot's trailing window */}
          <div className="flex items-center space-x-1 bg-surface-raised p-0.5 rounded-lg border border-surface-border text-[11px]">
            {windowOptions.map((w) => (
              <button
                key={w.s}
                type="button"
                onClick={() => setWindowSec(w.s)}
                title={
                  w.s === botWindowSec
                    ? `Bot's configured trailing window (${fmtWindow(w.s)})`
                    : `${fmtWindow(w.s)} of recorded history`
                }
                className={`px-2 py-1 rounded font-medium transition cursor-pointer ${
                  windowSec === w.s
                    ? "bg-accent text-white shadow-2xs"
                    : "text-slate-500 hover:text-slate-900"
                }`}
              >
                {w.label}
              </button>
            ))}
          </div>
        </div>
      </div>

      {/* Legend: current deviation per venue, A→Z (hover updates values) */}
      {legend.length > 0 && (
        <div className="flex flex-wrap items-center gap-1.5">
          {legend.map((row) => {
            const displayDev: number | null = hoverDevs
              ? (hoverDevs.get(row.vid) ?? null)
              : row.dev;
            return (
              <span
                key={row.vid}
                className="inline-flex items-center space-x-1.5 px-2 py-0.5 rounded-md border border-surface-border bg-surface-raised text-xs"
                title={`${venueName(row.vid)} mid vs baseline`}
              >
                <span
                  className="w-2 h-2 rounded-full inline-block"
                  style={{ backgroundColor: venueColor(row.vid) }}
                />
                <span className="font-semibold text-slate-700">{venueName(row.vid)}</span>
                {displayDev === null ? (
                  <span className="tabular-nums font-bold text-slate-400">—</span>
                ) : (
                  <span
                    className={`tabular-nums font-bold ${
                      displayDev > 0 ? "text-loss" : displayDev < 0 ? "text-profit" : "text-slate-500"
                    }`}
                  >
                    {fmtBps(displayDev)}
                  </span>
                )}
              </span>
            );
          })}
        </div>
      )}

      {/* Chart */}
      <div
        ref={containerRef}
        className="rounded-lg border border-surface-border bg-surface overflow-hidden shadow-2xs relative"
      >
        {failed ? (
          <EmptyState
            icon={Activity}
            title="Signal Series Unavailable"
            hint={symbol ? `No series data for ${symbol}.` : "No series data available."}
          />
        ) : !hasSeries ? (
          <EmptyState
            icon={Activity}
            pulse
            title={
              series && series.sample_count === 0
                ? "No Recorded Samples"
                : "Collecting Signal Samples…"
            }
            hint={
              series && series.sample_count === 0
                ? `No samples for ${symbol} in this window — history accumulates while this ticker's order books are polled.`
                : "History accumulates while this ticker's order books are polled."
            }
          />
        ) : (
          <svg
            width={W}
            height={H}
            viewBox={`0 0 ${W} ${H}`}
            className="w-full h-auto block"
            onMouseLeave={() => setHoverIdx(null)}
            onMouseMove={(e) => {
              if (!series) return;
              const rect = e.currentTarget.getBoundingClientRect();
              const f = (e.clientX - rect.left) / rect.width;
              const xPx = Math.min(W - PAD_R, Math.max(PAD_L, f * W));
              let idx = 0;
              let best = Infinity;
              series.ts_ms.forEach((t, i) => {
                const d = Math.abs(xOf(t) - xPx);
                if (d < best) {
                  best = d;
                  idx = i;
                }
              });
              setHoverIdx(idx);
            }}
          >
            {/* Horizontal gridlines + y labels */}
            {[-1, -0.5, 0, 0.5, 1].map((f) => {
              const dev = f * maxAbsDev;
              const y = yOf(dev);
              const isZero = f === 0;
              return (
                <g key={f}>
                  <line
                    x1={PAD_L}
                    x2={W - PAD_R}
                    y1={y}
                    y2={y}
                    stroke={isZero ? "#94a3b8" : "#e2e8f0"}
                    strokeWidth={isZero ? 1.4 : 1}
                    strokeDasharray={isZero ? "5 4" : "3 4"}
                  />
                  <text
                    x={PAD_L - 5}
                    y={y + 3}
                    textAnchor="end"
                    className="fill-slate-400"
                    fontSize="9"
                    fontFamily="'Inter Variable', 'Inter', sans-serif"
                  >
                    {isZero ? "0" : fmtBps(dev)}
                  </text>
                </g>
              );
            })}

            {/* Session regime bands (UTC) behind the series */}
            {sessionRects.map((s, i) => (
              <rect
                key={i}
                x={xOf(s.fromMs)}
                width={Math.max(1, xOf(s.toMs) - xOf(s.fromMs))}
                y={PAD_T}
                height={H - PAD_T - PAD_B}
                fill={SESSION_BAND_COLOR[s.session]}
                opacity={0.3}
              />
            ))}

            {/* Dislocation band (max-min venue envelope, ≥2 venues only) */}
            {bandPath && (
              <path
                d={bandPath}
                fill={isExecutableSpread ? "rgba(16,185,129,0.14)" : "rgba(100,116,139,0.10)"}
                stroke="none"
              />
            )}

            {/* Per-venue deviation lines */}
            {[...venuePaths.entries()].map(([vid, d]) => (
              <path
                key={vid}
                d={d}
                fill="none"
                stroke={venueColor(vid)}
                strokeWidth={1.6}
                strokeLinejoin="round"
                strokeLinecap="round"
              />
            ))}

            {/* Isolated single-bucket observations */}
            {isolatedDots.map((p, k) => (
              <circle
                key={k}
                cx={p.x}
                cy={p.y}
                r={2}
                fill={venueColor(p.vid)}
              />
            ))}

            {/* Hover crosshair + timestamp badge */}
            {hoverIdx !== null && series && series.ts_ms[hoverIdx] !== undefined && (
              <g>
                <line
                  x1={xOf(series.ts_ms[hoverIdx])}
                  x2={xOf(series.ts_ms[hoverIdx])}
                  y1={PAD_T}
                  y2={H - PAD_B}
                  stroke="#64748b"
                  strokeWidth={1}
                  strokeDasharray="3 3"
                />
                <text
                  x={Math.min(W - PAD_R - 24, Math.max(PAD_L + 24, xOf(series.ts_ms[hoverIdx])))}
                  y={PAD_T + 9}
                  textAnchor="middle"
                  className="fill-slate-600 font-mono font-bold"
                  fontSize="9"
                >
                  {fmtTick(series.ts_ms[hoverIdx], spanMs)}
                </text>
              </g>
            )}

            {/* X-axis time ticks — adaptive density based on chart width */}
            {series &&
              (W >= 640 ? [0, 0.25, 0.5, 0.75, 1] : [0, 0.5, 1]).map((f) => {
                const t = t0 + f * spanMs;
                return (
                  <text
                    key={f}
                    x={xOf(t)}
                    y={H - 6}
                    textAnchor={f === 0 ? "start" : f === 1 ? "end" : "middle"}
                    className="fill-slate-400"
                    fontSize="9"
                    fontFamily="'Inter Variable', 'Inter', sans-serif"
                  >
                    {fmtTick(t, spanMs)}
                  </text>
                );
              })}
          </svg>
        )}
      </div>

      {/* Footer: honesty indicators — how much real history backs this view */}
      {series && (
        <div className="flex items-center justify-between text-xs text-slate-400 px-0.5">
          <span>
            {series.sample_count.toLocaleString()} venue samples in window
            {historyLabel ? ` · ${historyLabel} recorded` : ""}
          </span>
          <span>
            {series.venues.length === 1
              ? "1 venue — pairwise spread needs ≥2 venues"
              : series.mode === "leader"
                ? "baseline = most-liquid venue"
                : "baseline = volume-weighted consensus"}
          </span>
        </div>
      )}
    </div>
  );
};
