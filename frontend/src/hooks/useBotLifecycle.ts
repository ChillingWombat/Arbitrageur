import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../services/api.ts";
import { flattenResidualMessage } from "../services/flatten.ts";

/**
 * Per-strategy bot lifecycle: the durable signal ledger is the source of
 * truth for whether positions this strategy opened are still on the books.
 *
 *  - running     → new entries allowed
 *  - paused      → no executions; positions preserved
 *  - reduce_only → only exit/unwind executions; auto-stops once flat
 *  - stopping    → emergency flatten in flight (buttons locked)
 *  - idle        → stopped
 */
export type BotPhase = "idle" | "running" | "paused" | "reduce_only" | "stopping";

const OPEN_STATUSES = new Set(["in_flight", "balanced", "imbalanced", "unresolved"]);

/* ------------------------------------------------------------------ */
/* Shared bot-phase registry (in-memory only, never persisted).         */
/*                                                                     */
/* Each mounted useBotLifecycle publishes its current phase under       */
/* `${strategyId}:${symbol ?? "all"}`. Unmounted bots publish idle on   */
/* cleanup — intervals die with the page, so idle is truthful.         */
/* Consumers (e.g. the ticker explorer filter) read via                */
/* tickerBotStatus() and re-render via useSyncExternalStore with the    */
/* subscribe/getSnapshot helpers below. Additive only: the hook        */
/* signature and lifecycle semantics are unchanged.                     */
/* ------------------------------------------------------------------ */

const botPhaseRegistry = new Map<string, BotPhase>();
const botPhaseListeners = new Set<() => void>();
const botResetListeners = new Set<() => void>();
let botPhaseVersion = 0;

function botRegistryKey(strategyId: string, symbol?: string | null): string {
  return `${strategyId}:${symbol ?? "all"}`;
}

export function publishBotPhase(strategyId: string, symbol: string | null | undefined, phase: BotPhase): void {
  const key = botRegistryKey(strategyId, symbol);
  if (botPhaseRegistry.get(key) === phase) return;
  botPhaseRegistry.set(key, phase);
  botPhaseVersion += 1;
  for (const listener of botPhaseListeners) listener();
}

/** Register a callback invoked when all frontend bots are reset (halt, killswitch, circuit breaker reset). */
export function registerBotResetListener(listener: () => void): () => void {
  botResetListeners.add(listener);
  return () => {
    botResetListeners.delete(listener);
  };
}

/** Reset all registered strategy bots to idle (stopped). Used on emergency stop / circuit breaker reset. */
export function stopAllFrontendBots(): void {
  for (const [key] of botPhaseRegistry) {
    botPhaseRegistry.set(key, "idle");
  }
  botPhaseVersion += 1;
  for (const listener of botPhaseListeners) listener();
  for (const reset of botResetListeners) reset();
}

/** useSyncExternalStore subscribe fn — re-renders consumers on any phase publish. */
export function subscribeBotPhaseRegistry(listener: () => void): () => void {
  botPhaseListeners.add(listener);
  return () => {
    botPhaseListeners.delete(listener);
  };
}

/** useSyncExternalStore snapshot — a monotonically increasing version. */
export function getBotPhaseRegistryVersion(): number {
  return botPhaseVersion;
}

/** Aggregated per-ticker bot status for explorer filtering. */
export type TickerBotStatus = "running" | "paused" | "reducing" | "stopped";

function baseOf(sym: string): string {
  return sym.split(/[-_/]/)[0];
}

/**
 * Aggregate every registry entry whose symbol part matches `baseOrSymbol`
 * (exact, case-insensitive, plus base-symbol variants in both directions)
 * into a single status. Priority: running > reduce_only/stopping
 * (reducing) > paused > idle/unknown (stopped). Strategy-wide entries
 * keyed `:all` never match a concrete ticker.
 */
export function tickerBotStatus(baseOrSymbol: string): TickerBotStatus {
  const want = baseOrSymbol.toUpperCase();
  const wantBase = baseOf(want);
  let seenPaused = false;
  let seenReducing = false;
  for (const [key, phase] of botPhaseRegistry) {
    const regSymbol = key.slice(key.indexOf(":") + 1).toUpperCase();
    if (regSymbol === "ALL") continue;
    const regBase = baseOf(regSymbol);
    const matches =
      regSymbol === want || regBase === want || regSymbol === wantBase || regBase === wantBase;
    if (!matches) continue;
    if (phase === "running") return "running";
    if (phase === "reduce_only" || phase === "stopping") seenReducing = true;
    else if (phase === "paused") seenPaused = true;
  }
  if (seenReducing) return "reducing";
  if (seenPaused) return "paused";
  return "stopped";
}

export interface BotControlAvailability {
  start: boolean;
  pause: boolean;
  reduceOnly: boolean;
  stop: boolean;
}

export function getBotControlAvailability(
  phase: BotPhase,
  hasOpenPositions: boolean,
  entriesDisabled: boolean,
  reduceOnlyAvailable = true,
  /** Convergence bots require a flat book before re-arming; strategies whose
   *  positions are the product (options ride to expiry) may re-arm while
   *  holding — the open-position cap governs entries, not presence. */
  allowStartWithOpenPositions = false,
): BotControlAvailability {
  const stopping = phase === "stopping";
  return {
    start:
      !entriesDisabled &&
      !stopping &&
      (allowStartWithOpenPositions || !hasOpenPositions) &&
      (phase === "idle" || phase === "paused"),
    pause: phase === "running",
    reduceOnly:
      reduceOnlyAvailable &&
      !stopping &&
      hasOpenPositions &&
      (phase === "running" || phase === "paused"),
    stop: !stopping && (phase !== "idle" || hasOpenPositions),
  };
}

interface UseBotLifecycleOptions {
  /** Ledger strategy id: futures | radar | funding | spot | prediction | stat_arb. */
  strategyId: string;
  /** Scope positions to one symbol; omit for strategy-wide tracking. */
  symbol?: string | null;
  /** Extra position source (e.g. grid accumulated units) treated as open. */
  externalOpenPositions?: boolean;
}

/**
 * Fail-closed emergency-stop predicate: flatten must be attempted unless
 * positions are affirmatively known-flat (fresh ledger, zero open signals,
 * no external position source). A stale/unknown ledger always flattens.
 */
export function shouldAttemptFlattenOnStop(args: {
  ledgerFresh: boolean;
  openSignalCount: number;
  externalOpenPositions?: boolean;
}): boolean {
  if (args.externalOpenPositions === true) return true;
  if (!args.ledgerFresh) return true;
  return args.openSignalCount > 0;
}

export function useBotLifecycle({ strategyId, symbol, externalOpenPositions }: UseBotLifecycleOptions) {
  const [phase, setPhase] = useState<BotPhase>("idle");
  const [openSignalCount, setOpenSignalCount] = useState(0);
  const [ledgerFresh, setLedgerFresh] = useState(false);
  const [lastError, setLastError] = useState<string | null>(null);
  const phaseRef = useRef(phase);
  phaseRef.current = phase;
  const ledgerFreshRef = useRef(ledgerFresh);
  ledgerFreshRef.current = ledgerFresh;
  const openSignalCountRef = useRef(openSignalCount);
  openSignalCountRef.current = openSignalCount;

  // Publish to the shared in-memory registry; idle on unmount is truthful
  // because the bot's intervals die with the page.
  useEffect(() => {
    publishBotPhase(strategyId, symbol, phase);
    return () => publishBotPhase(strategyId, symbol, "idle");
  }, [strategyId, symbol, phase]);

  // Poll durable signal ledger for this strategy's open positions.
  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      try {
        const sigs = await api.listSignalExecutions(symbol ?? undefined);
        if (cancelled) return;
        setOpenSignalCount(
          sigs.filter(
            (s) =>
              s.strategyId === strategyId &&
              OPEN_STATUSES.has(s.status) &&
              s.legs.some((l) => l.filledQty > 0)
          ).length
        );
        setLedgerFresh(true);
      } catch {
        if (cancelled) return;
        setLedgerFresh(false);
      }
    };
    void load();
    const t = setInterval(load, 3000);
    return () => {
      cancelled = true;
      clearInterval(t);
    };
  }, [strategyId, symbol]);

  const hasOpenPositions = openSignalCount > 0 || externalOpenPositions === true;

  // Reduce-only retires to idle once nothing this strategy opened remains.
  useEffect(() => {
    if (phase === "reduce_only" && !hasOpenPositions) setPhase("idle");
  }, [phase, hasOpenPositions]);

  const start = useCallback(() => {
    setLastError(null);
    setPhase("running");
  }, []);

  const pause = useCallback(() => setPhase("paused"), []);

  const reduceOnly = useCallback(() => {
    setLastError(null);
    setPhase("reduce_only");
  }, []);

  // Reset to idle on global stop/reset event
  useEffect(() => {
    return registerBotResetListener(() => {
      setPhase("idle");
    });
  }, []);

  const stop = useCallback(async () => {
    setPhase("stopping");
    setLastError(null);
    try {
      const ledgerWasStale = !ledgerFreshRef.current;
      if (shouldAttemptFlattenOnStop({
        ledgerFresh: ledgerFreshRef.current,
        openSignalCount: openSignalCountRef.current,
        externalOpenPositions,
      })) {
        const res = await api.flattenTickerPositions(symbol ?? null, strategyId);
        const residual = flattenResidualMessage(res);
        if (residual !== null) {
          setLastError(residual);
          setPhase("idle");
          return;
        }
        if (ledgerWasStale) {
          setLastError(
            "Signal ledger was unavailable during stop — flatten attempted with no residual; confirm flat before restarting"
          );
          setPhase("idle");
          return;
        }
      }
      setPhase("idle");
    } catch (e) {
      setLastError(e instanceof Error ? e.message : String(e));
      // Flatten failed — halt entries but transition to idle (stopped).
      setPhase("idle");
    }
  }, [symbol, strategyId, externalOpenPositions]);

  return {
    phase,
    hasOpenPositions,
    openSignalCount,
    ledgerFresh,
    lastError,
    start,
    pause,
    reduceOnly,
    stop,
    /** New position entries only while running. */
    entriesAllowed: phase === "running",
    /** Exit/unwind executions while running or draining in reduce-only. */
    exitsAllowed: phase === "running" || phase === "reduce_only",
    /** Any non-idle phase. */
    active: phase !== "idle",
  };
}
