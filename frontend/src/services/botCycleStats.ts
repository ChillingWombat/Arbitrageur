import type { SignalExecutionDto, StrategyFillPnl } from "../types/index.ts";

/**
 * Bot cycle stats for the radar stat cards. A cycle is one entered-then-
 * closed round trip of the radar strategy ledger: `closed` with filled legs
 * counts as completed, open statuses with filled legs count as open.
 * `failed` never counts — it was entered but never closed.
 *
 * Net profit comes from the backend's attributed fill roll-up
 * (`get_strategy_fill_pnl`, fees included) when supplied for the matching
 * strategy. Without it the cards render "—" with the reason below rather
 * than fabricating a number from co-mingled fills.
 */
export const BOT_CYCLE_STRATEGY_ID = "radar";

const OPEN_STATUSES: ReadonlySet<string> = new Set([
  "in_flight",
  "balanced",
  "imbalanced",
  "unresolved",
]);

export const BOT_PROFIT_UNATTRIBUTED_REASON =
  "Realized fills are not strategy-attributed (shared spatial paper path); per-cycle radar PnL is unavailable.";

export interface BotCycleStats {
  completedCycles: number;
  openCycles: number;
  /** Attributed realized net profit (fees included); null when the backend
   *  roll-up is absent — never fabricated. */
  netProfitUsd: number | null;
  /** Attributed net profit in bps; null when notional is unavailable. */
  netProfitBps: number | null;
  profitUnattributedReason: string | null;
  /** True when the ledger read failed — counts are unknown, not zero. */
  unknown: boolean;
  /** Closed families the backend could not value (no priced fills). They
   *  are excluded from net profit — never absorbed into it. */
  unattributedClosedFamilies: number;
}

function hasFills(s: SignalExecutionDto): boolean {
  return (s.legs ?? []).some((l) => (l?.filledQty ?? 0) > 0);
}

function finiteOrNull(v: number | null | undefined): number | null {
  return typeof v === "number" && Number.isFinite(v) ? v : null;
}

export function computeBotCycleStats(
  signals: SignalExecutionDto[] | null | undefined,
  strategyId: string = BOT_CYCLE_STRATEGY_ID,
  fillPnl?: StrategyFillPnl | null,
): BotCycleStats {
  const want = strategyId.trim().toLowerCase();
  const attributed =
    fillPnl != null &&
    (fillPnl.strategyId ?? "").trim().toLowerCase() === want &&
    Number.isFinite(fillPnl.netProfitUsd);
  const base = attributed
    ? {
        netProfitUsd: finiteOrNull(fillPnl!.netProfitUsd),
        netProfitBps: finiteOrNull(fillPnl!.netProfitBps),
        profitUnattributedReason: null,
        unattributedClosedFamilies:
          Number.isFinite(fillPnl!.unattributedClosedFamilies) &&
          (fillPnl!.unattributedClosedFamilies ?? 0) >= 0
            ? Math.floor(fillPnl!.unattributedClosedFamilies)
            : 0,
      }
    : {
        netProfitUsd: null,
        netProfitBps: null,
        profitUnattributedReason: BOT_PROFIT_UNATTRIBUTED_REASON,
        unattributedClosedFamilies: 0,
      };
  if (signals == null) {
    return { ...base, completedCycles: 0, openCycles: 0, unknown: true };
  }
  let completedCycles = 0;
  let openCycles = 0;
  for (const s of signals) {
    if (!s) continue;
    if ((s.strategyId ?? "").trim().toLowerCase() !== want) continue;
    if (!hasFills(s)) continue;
    if (s.status === "closed") completedCycles += 1;
    else if (OPEN_STATUSES.has(s.status)) openCycles += 1;
  }
  return { ...base, completedCycles, openCycles, unknown: false };
}
