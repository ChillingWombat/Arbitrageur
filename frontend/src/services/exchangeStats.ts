import type { ExecutionLogEntry, UtilisationHistory, VenueBalanceInfo } from "../types/index.ts";

/**
 * Per-venue aggregates for the Portfolio Monitor exchange gadget, built only
 * from existing IPC surfaces (`get_execution_logs`, `get_venue_balances`)
 * plus the durable margin-snapshot history (`get_utilisation_history`).
 *
 * Attribution rules (documented in the UI subtitle as well):
 * - Every log entry involving the venue counts as one trade event on that
 *   venue, so a two-leg trade counts on both of its venues.
 * - Totals sum `net_profit`, which the backend records net of fees — gross
 *   and fee legs are never re-added.
 * - Utilisation is the venue account's used-margin share
 *   (`margin_rate_pct` = initial margin / equity).
 * - The 24h average is the time-weighted mean of retained margin snapshots;
 *   when snapshots cover less than the full window the note names the
 *   shorter window, and with no history the cell renders "—".
 */
export const UTILISATION_HISTORY_UNAVAILABLE_REASON =
  "No utilisation history is retained — spot margin snapshot only.";

export interface ExchangeRowStats {
  venue: string;
  trades: number;
  totalPnlUsd: number;
  avgPnlUsd: number | null;
  equityUsd: number | null;
  utilisationPct: number | null;
  avgUtilisationPct: number | null;
  avgUtilisationNote: string | null;
}

function venueKey(raw: string | null | undefined): string | null {
  const k = (raw ?? "").trim().toLowerCase();
  return k === "" ? null : k;
}

function finiteOrNull(v: number | null | undefined): number | null {
  return typeof v === "number" && Number.isFinite(v) ? v : null;
}

export function formatUtilisationWindow(coveredMs: number, windowMs: number): string {
  const hours = coveredMs / 3_600_000;
  if (!Number.isFinite(hours) || hours <= 0) return "no snapshot coverage";
  if (coveredMs >= windowMs) {
    const windowHours = windowMs / 3_600_000;
    return windowHours === 24 ? "24h time-weighted avg" : "time-weighted avg over window";
  }
  if (hours < 1) return `avg over ${Math.max(1, Math.round(coveredMs / 60_000))}m available`;
  return `avg over ${hours.toFixed(hours < 10 ? 1 : 0)}h available`;
}

export function computeExchangeStats(
  logs: ExecutionLogEntry[] | null | undefined,
  balances: VenueBalanceInfo[] | null | undefined,
  utilisation?: Record<string, UtilisationHistory | null | undefined> | null,
): ExchangeRowStats[] {
  const byVenue = new Map<string, { trades: number; totalPnlUsd: number }>();
  const touch = (venue: string) => {
    let row = byVenue.get(venue);
    if (!row) {
      row = { trades: 0, totalPnlUsd: 0 };
      byVenue.set(venue, row);
    }
    return row;
  };

  for (const entry of logs ?? []) {
    if (!entry) continue;
    const pnl = finiteOrNull(entry.net_profit) ?? 0;
    const venues = new Set<string>();
    for (const raw of [entry.venue_a, entry.venue_b]) {
      const k = venueKey(raw);
      if (k) venues.add(k);
    }
    for (const venue of venues) {
      const row = touch(venue);
      row.trades += 1;
      row.totalPnlUsd += pnl;
    }
  }

  const equity = new Map<string, { equityUsd: number | null; utilisationPct: number | null }>();
  for (const b of balances ?? []) {
    if (!b) continue;
    const k = venueKey(b.venue);
    if (!k || equity.has(k)) continue;
    equity.set(k, {
      equityUsd: finiteOrNull(b.equity_usd),
      utilisationPct: finiteOrNull(b.margin_rate_pct),
    });
  }

  const venues = new Set<string>([...byVenue.keys(), ...equity.keys()]);
  return [...venues]
    .sort()
    .map((venue) => {
      const agg = byVenue.get(venue);
      const bal = equity.get(venue);
      const hist = utilisation?.[venue] ?? null;
      const histAvg = hist != null ? finiteOrNull(hist.avgRatePct) : null;
      const trades = agg?.trades ?? 0;
      const totalPnlUsd = agg?.totalPnlUsd ?? 0;
      return {
        venue,
        trades,
        totalPnlUsd,
        avgPnlUsd: trades > 0 ? totalPnlUsd / trades : null,
        equityUsd: bal?.equityUsd ?? null,
        utilisationPct: bal?.utilisationPct ?? null,
        avgUtilisationPct: histAvg,
        avgUtilisationNote:
          hist == null
            ? UTILISATION_HISTORY_UNAVAILABLE_REASON
            : histAvg == null
              ? UTILISATION_HISTORY_UNAVAILABLE_REASON
              : formatUtilisationWindow(hist.coveredMs, hist.windowMs),
      };
    });
}
