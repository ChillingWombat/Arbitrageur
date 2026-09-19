// Pure helpers for the External Positions gadget (grouping + roll-ups).
// WHAT: positions created outside the system (manual website trades) that the
// bot can close reduce-only. Grouped either by ticker (base asset across
// venues — the delta-neutral view) or by exchange (per-venue ops view).
// All functions are pure and unit-tested in externalPositions.test.ts.
import type { ExternalPositionDto } from "../types/index.ts";

export type ExternalGroupBy = "ticker" | "venue";

export interface ExternalGroup {
  /** Group key: base asset ("BTC") or venue id ("hyperliquid"). */
  key: string;
  /** Human label for the group header. */
  label: string;
  positions: ExternalPositionDto[];
  /** Signed net notional: longs + / shorts − (net exposure of the group). */
  netNotionalUsd: number;
  /** Gross (sum of |notional|) — capital tied up regardless of direction. */
  grossNotionalUsd: number;
  totalUnrealizedUsd: number;
  /** True when any leg in the group lacks close support. */
  hasUnsupported: boolean;
}

const signed = (p: ExternalPositionDto): number =>
  (p.side === "short" ? -1 : 1) * (Number.isFinite(p.notionalUsd) ? p.notionalUsd : 0);

function rollup(key: string, label: string, positions: ExternalPositionDto[]): ExternalGroup {
  const net = positions.reduce((a, p) => a + signed(p), 0);
  const gross = positions.reduce(
    (a, p) => a + Math.abs(Number.isFinite(p.notionalUsd) ? p.notionalUsd : 0), 0);
  return {
    key,
    label,
    positions: [...positions].sort((a, b) => Math.abs(b.notionalUsd) - Math.abs(a.notionalUsd)),
    netNotionalUsd: net,
    grossNotionalUsd: gross,
    totalUnrealizedUsd: positions.reduce(
      (a, p) => a + (Number.isFinite(p.unrealizedPnlUsd) ? p.unrealizedPnlUsd : 0), 0),
    hasUnsupported: positions.some((p) => !p.closeSupported),
  };
}

/** Group external positions by base asset or venue, biggest gross first. */
export function groupExternalPositions(
  positions: ExternalPositionDto[],
  by: ExternalGroupBy,
): ExternalGroup[] {
  const map = new Map<string, ExternalPositionDto[]>();
  for (const p of positions) {
    const k = by === "ticker" ? (p.base || "UNKNOWN") : (p.venue || "unknown");
    const arr = map.get(k);
    if (arr) arr.push(p);
    else map.set(k, [p]);
  }
  return [...map.entries()]
    .map(([k, ps]) => rollup(k, by === "ticker" ? `${k} (all venues)` : k, ps))
    .sort((a, b) => b.grossNotionalUsd - a.grossNotionalUsd);
}
