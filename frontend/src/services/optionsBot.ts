import type { OptionsBotConfigDto } from "../types/index.ts";

/** Inert factory defaults mirroring the backend — nothing trades until a
 *  substrategy is explicitly armed. */
export function defaultOptionsBotConfig(substrategyId: string): OptionsBotConfigDto {
  return {
    substrategyId,
    enabled: false,
    mode: "off",
    qtyBase: 0.1,
    minEdgeBps: 5,
    minExpectedYieldPct: 30,
    minHoursToExpiry: 48,
    maxDaysToExpiry: 60,
    pollIntervalSecs: 30,
    maxOpenPositions: 2,
  };
}
