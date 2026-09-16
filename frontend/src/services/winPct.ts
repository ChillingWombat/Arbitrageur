/** Ranked win-% formatting for spatial scan results.
 *
 * The backend populates `SpatialOpportunityDto.win_probability` (0-1) when
 * the pair's episode history qualifies, and leaves it null otherwise.
 * Missing or non-finite values render as "—", never 0%.
 */
export function formatWinPct(v: number | null | undefined): string {
  if (v == null || !Number.isFinite(v)) return "—";
  return `${(v * 100).toFixed(0)}%`;
}
