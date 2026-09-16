import type {
  BotOpenPositionDto,
  MonitorRowDto,
  RadarBotConfigDto,
} from "../types/index.ts";

/** Normalized pair key shared by the status gadget and row markers. */
export function botPairKey(symbol: string, buyVenue: string, sellVenue: string): string {
  return `${symbol.trim().toUpperCase()}:${buyVenue.trim().toLowerCase()}:${sellVenue.trim().toLowerCase()}`;
}

/** Case-insensitive held-pair set from the bot status open positions. */
export function buildHeldSet(positions: BotOpenPositionDto[] | null | undefined): Set<string> {
  const out = new Set<string>();
  for (const p of positions ?? []) {
    if (!p) continue;
    out.add(botPairKey(p.symbol ?? "", p.buyVenue ?? "", p.sellVenue ?? ""));
  }
  return out;
}

/** Whether a radar row currently has a bot-managed convergence position. */
export function isRowHeld(row: MonitorRowDto, held: Set<string>): boolean {
  return held.has(botPairKey(row.symbol, row.buyVenue, row.sellVenue));
}

/**
 * Executable notional for a radar row: the operator override wins, else the
 * backend's optimal size. Returns `null` when neither is a positive finite
 * number — callers must block the execution, never fabricate a size.
 */
export function resolveRadarExecSize(
  override: number | undefined,
  optimalSizeUsd: number | null | undefined,
): number | null {
  const candidate = override ?? optimalSizeUsd;
  return typeof candidate === "number" && Number.isFinite(candidate) && candidate > 0
    ? candidate
    : null;
}

/** Entries fire only when explicitly armed in full-auto. */
export function botArmed(cfg: RadarBotConfigDto | null | undefined): boolean {
  return cfg?.enabled === true && cfg?.mode === "auto";
}

const MODE_LABEL: Record<string, string> = {
  off: "Off",
  auto: "Auto",
  paused: "Paused",
  reduce_only: "Reduce-Only",
};

/** Human mode label; unknown backend values render as "—", never guessed. */
export function describeBotMode(mode: string | null | undefined): string {
  if (mode == null) return "—";
  return MODE_LABEL[mode] ?? "—";
}

/** Inert factory defaults mirroring the backend (opt-in: nothing trades). */
export function defaultRadarBotConfig(): RadarBotConfigDto {
  return {
    enabled: false,
    mode: "off",
    pollIntervalSecs: 5,
    minPWin: 0.93,
    minExpectedProfitBps: 2.0,
    maxConcurrentPositions: 3,
    maxNotionalPerPositionUsd: 1000,
    maxNotionalPerVenueUsd: 3000,
    maxNotionalPerSymbolUsd: 2000,
    pairCooldownSecs: 900,
    holdTimeoutMult: 3.0,
    stopMult: 1.5,
    staleUnwindSecs: 600,
  };
}

/* ------------------------------------------------------------------ */
/* Single-control consolidation helpers (backend BotMode is the source */
/* of truth; the page-local useBotLifecycle phase was removed from the */
/* radar page because it fought the backend mode).                     */
/* ------------------------------------------------------------------ */

/** Mode buttons are inert until the backend config has loaded. */
export function botControlsDisabled(cfg: RadarBotConfigDto | null | undefined): boolean {
  return cfg == null;
}

/**
 * Emergency flatten (the old lifecycle Stop) stays armed while the bot may
 * hold risk: open managed positions exist, or the backend mode is anything
 * but off. Fully inert (off + flat) disables it — there is nothing to wind
 * down. Merged from BotLifecycleControls' stop availability, minus the
 * page-local "stopping" phase which no longer exists.
 */
export function botFlattenAvailable(
  openPositions: number,
  mode: string | null | undefined,
): boolean {
  if (!Number.isFinite(openPositions) || openPositions < 0) return false;
  if (openPositions > 0) return true;
  return mode != null && mode !== "off";
}

/** Badge tone for the single radar control: armed / draining / inert. */
export type BotBadgeTone = "armed" | "draining" | "inert";

export function botBadgeTone(cfg: RadarBotConfigDto | null | undefined): BotBadgeTone {
  if (botArmed(cfg)) return "armed";
  if (cfg?.mode === "paused" || cfg?.mode === "reduce_only") return "draining";
  return "inert";
}
