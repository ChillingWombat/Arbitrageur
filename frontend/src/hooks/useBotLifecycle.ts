import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../services/api";

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

interface UseBotLifecycleOptions {
  /** Ledger strategy id: futures | funding | spot | prediction | stat_arb. */
  strategyId: string;
  /** Scope positions to one symbol; omit for strategy-wide tracking. */
  symbol?: string | null;
  /** Extra position source (e.g. grid accumulated units) treated as open. */
  externalOpenPositions?: boolean;
}

export function useBotLifecycle({ strategyId, symbol, externalOpenPositions }: UseBotLifecycleOptions) {
  const [phase, setPhase] = useState<BotPhase>("idle");
  const [openSignalCount, setOpenSignalCount] = useState(0);
  const [lastError, setLastError] = useState<string | null>(null);
  const phaseRef = useRef(phase);
  phaseRef.current = phase;

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
      } catch {
        // Outside Tauri / backend down — ledger stays empty, never fabricate.
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

  const stop = useCallback(async () => {
    setPhase("stopping");
    setLastError(null);
    try {
      if (openSignalCount > 0) {
        const res = await api.flattenTickerPositions(symbol ?? null, strategyId);
        if (res.residualSignals > 0) {
          setLastError(
            `Flatten incomplete: ${res.residualSignals} signal(s) still hold residual exposure`
          );
          setPhase("paused");
          return;
        }
      }
      setPhase("idle");
    } catch (e) {
      setLastError(e instanceof Error ? e.message : String(e));
      // Flatten failed — halt entries but keep the incident visible.
      setPhase("paused");
    }
  }, [openSignalCount, symbol, strategyId]);

  return {
    phase,
    hasOpenPositions,
    openSignalCount,
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
