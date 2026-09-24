import type { OpenPositionDto } from "../types/index.ts";

export type PositionSortOption = "name" | "pnl" | "size";
export type PositionScope = "all" | "current";

/**
 * Scope-filter and sort aggregated open positions for the sidebar gadget.
 * "current" keeps only rows whose strategyId matches the active tab's
 * strategy; null id or "all" scope returns everything.
 *
 * Sort keys:
 * - name: stable identity key ascending
 * - pnl:  unrealized PnL descending; unavailable (null) sorts last — a
 *   missing mark never masquerades as zero PnL
 * - size: entry notional descending; unavailable sorts last
 */
export function scopeAndSortPositions(
  positions: OpenPositionDto[],
  scope: PositionScope,
  currentStrategyId: string | null,
  sortOption: PositionSortOption,
): OpenPositionDto[] {
  const list =
    scope === "current" && currentStrategyId
      ? positions.filter((p) => p.strategyId === currentStrategyId)
      : positions;

  const sorted = [...list];
  if (sortOption === "name") {
    sorted.sort((a, b) => a.key.localeCompare(b.key));
  } else {
    const metric = (p: OpenPositionDto): number | null =>
      sortOption === "pnl" ? p.unrealizedPnlUsd : p.notionalUsd;
    sorted.sort((a, b) => {
      const ma = metric(a);
      const mb = metric(b);
      if (ma !== null && mb === null) return -1;
      if (ma === null && mb !== null) return 1;
      if (ma !== null && mb !== null && ma !== mb) return mb - ma;
      return a.key.localeCompare(b.key);
    });
  }
  return sorted;
}

// Reduce-only lives server-side (`ticker_reduce_only` table, enforced by
// every backend entry gate) — the old localStorage flag could be bypassed
// by any manual execute path and was never environment-scoped.
