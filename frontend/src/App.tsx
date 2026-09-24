import React, { useState, useEffect, useCallback, useMemo, useRef } from "react";
import { Header } from "./components/Header";
import { TickerExplorer } from "./components/TickerExplorer";
import { PositionsPanel } from "./components/PositionsPanel";
import type { ActiveTab } from "./components/Navigation";
import { LiveMatrix } from "./components/LiveMatrix";
import { FundingRadar } from "./components/FundingRadar";
import { OpportunityRadar } from "./components/OpportunityRadar";
import { PredictionHub } from "./components/PredictionHub";
import { StatArbLab } from "./components/StatArbLab";
import { PortfolioMonitor } from "./components/PortfolioMonitor";
import { SettingsModal } from "./components/SettingsModal";
import { SpotArbitrageBot } from "./components/SpotArbitrageBot";
import { TriangularArbitrage } from "./components/TriangularArbitrage";
import { TickerConfigModal } from "./components/TickerConfigModal";
import { VaultUnlockModal } from "./components/VaultUnlockModal";
import { api, withTimeoutFallback } from "./services/api";
import { scheduleVaultLock } from "./services/vaultLock";
import { Lock, Unlock, ShieldCheck, AlertTriangle } from "lucide-react";
import type {
  SystemStatusDto,
  VenueOrderBookDto,
  VenueVolumesDto,
  VenueMarketUniverseDto,
  SpatialOpportunityDto,
  SpotOpportunityDto,
  TriangleOpportunityDto,
  FundingOpportunityDto,
  PredictionOpportunityDto,
  RiskOverviewDto,
  ExecutionLogEntry,
  TickerStrategyConfigDto,
  StrategyParamsDto,
  EngineSettingsDto,
  SpatialCandidateInput,
  TickerPositionState,
  OpenPositionDto,
  VaultStatus,
  SignalExecutionDto,
} from "./types";

import {
  PERP_TICKERS,
  getAdmittedVenueCodes,
  applyMarketUniverse,
  applyTickerVisibility,
  tickerUnavailabilityReason,
  type PerpTicker,
  STRATEGY_DISPLAY_NAMES,
} from "./types/tickers";
import { deriveStepSizing, resolveTotalVenueEquity } from "./services/contractSpecs";

// Fixed design canvas — the app always renders at this size and is uniformly
// scaled to fit the window. Ratio ~1.88:1 sits just below a maximized 16:9
// window's usable area (~1.93:1 after the title bar and OS taskbar), so height
// binds the scale and the surplus becomes a small symmetric side margin rather
// than letterboxing or full-bleed width.
const DESIGN_WIDTH = 1920;
const DESIGN_HEIGHT = 1005;

/** Which persisted strategy each page's executions belong to — the sidebar's
 *  "current strategy" scope keys off this. Radar executions dispatch through
 *  the spatial pipeline but persist under the separate `radar` strategy so
 *  the two bots' positions stay separable. Tabs with no single-strategy
 *  identity (portfolio monitor, triangle — which has no persisted execution
 *  path) map to null and can only show "all". */
const TAB_STRATEGY: Record<ActiveTab, { id: string | null; label: string }> = {
  spatial: { id: "futures", label: STRATEGY_DISPLAY_NAMES.futures },
  radar: { id: "radar", label: STRATEGY_DISPLAY_NAMES.radar },
  funding: { id: "funding", label: STRATEGY_DISPLAY_NAMES.funding },
  spot_arb: { id: "spot", label: STRATEGY_DISPLAY_NAMES.spot },
  prediction: { id: "prediction", label: STRATEGY_DISPLAY_NAMES.prediction },
  statarb: { id: "stat_arb", label: STRATEGY_DISPLAY_NAMES.stat_arb },
  triangle: { id: "triangle", label: STRATEGY_DISPLAY_NAMES.triangle },
  portfolio_monitor: { id: null, label: "Portfolio" },
};

const useFitScale = () => {
  const [scale, setScale] = useState(() =>
    Math.min(window.innerWidth / DESIGN_WIDTH, window.innerHeight / DESIGN_HEIGHT)
  );
  useEffect(() => {
    const update = () =>
      setScale(Math.min(window.innerWidth / DESIGN_WIDTH, window.innerHeight / DESIGN_HEIGHT));
    update();
    window.addEventListener("resize", update);
    return () => window.removeEventListener("resize", update);
  }, []);
  return scale;
};

export const App: React.FC = () => {
  const canvasScale = useFitScale();
  const [activeTab, setActiveTab] = useState<ActiveTab>("spatial");
  const prevTabRef = useRef<ActiveTab>("spatial");
  const handleSelectTab = useCallback((tab: ActiveTab) => {
    setActiveTab((cur) => {
      if (cur !== tab) prevTabRef.current = cur;
      return tab;
    });
  }, []);
  const handleTogglePortfolio = useCallback(() => {
    setActiveTab((cur) => {
      if (cur === "portfolio_monitor") return prevTabRef.current;
      prevTabRef.current = cur;
      return "portfolio_monitor";
    });
  }, []);
  const [systemStatus, setSystemStatus] = useState<SystemStatusDto | null>(null);
  const [, setEngineSettings] = useState<EngineSettingsDto | null>(null);
  const [orderbooks, setOrderbooks] = useState<VenueOrderBookDto[]>([]);
  const [tickerVolumes, setTickerVolumes] = useState<VenueVolumesDto[]>([]);
  const [marketUniverse, setMarketUniverse] = useState<VenueMarketUniverseDto[]>([]);
  const [spatialOpps, setSpatialOpps] = useState<SpatialOpportunityDto[]>([]);
  const [spotOpps, setSpotOpps] = useState<SpotOpportunityDto[]>([]);
  const [triangleOpps, setTriangleOpps] = useState<TriangleOpportunityDto[]>([]);
  const [fundingOpps, setFundingOpps] = useState<FundingOpportunityDto[]>([]);
  const [predictionOpps, setPredictionOpps] = useState<PredictionOpportunityDto[]>([]);
  const [riskData, setRiskData] = useState<RiskOverviewDto | null>(null);
  const [executionLogs, setExecutionLogs] = useState<ExecutionLogEntry[]>([]);
  const [strategyConfigs, setStrategyConfigs] = useState<Map<string, TickerStrategyConfigDto>>(new Map());
  // Universal per-strategy parameters (shared across tickers), keyed by strategy id.
  const [strategyParams, setStrategyParams] = useState<Map<string, StrategyParamsDto>>(new Map());
  const [totalEquityUsd, setTotalEquityUsd] = useState<number | null>(null);
  // Ref mirrors so refreshAllData can read the latest values without
  // listing fresh-identity state (new Map every save) in its deps — that
  // refires the polling effect continuously.
  const strategyParamsRef = useRef(strategyParams);
  useEffect(() => {
    strategyParamsRef.current = strategyParams;
  }, [strategyParams]);
  const totalEquityUsdRef = useRef(totalEquityUsd);
  useEffect(() => {
    totalEquityUsdRef.current = totalEquityUsd;
  }, [totalEquityUsd]);
  const [tickerPositionState, setTickerPositionState] = useState<TickerPositionState | null>(null);
  const [openPositions, setOpenPositions] = useState<OpenPositionDto[]>([]);
  // Reconciler-escalated signals — each may hold residual venue exposure and
  // must stay visible on every tab, not only where the acknowledge control
  // lives (Spread Radar).
  const [attentionSignals, setAttentionSignals] = useState<SignalExecutionDto[]>([]);
  const [selectedPositionKey, setSelectedPositionKey] = useState<string | null>(null);
  const [enabledTickers, setEnabledTickers] = useState<Set<string>>(() => new Set());
  const [isPaused, setIsPaused] = useState(false);

  const handleToggleTicker = useCallback((symbol: string, enabled?: boolean) => {
    setEnabledTickers((prev) => {
      const next = new Set(prev);
      const shouldEnable = enabled !== undefined ? enabled : !next.has(symbol);
      if (shouldEnable) {
        next.add(symbol);
      } else {
        next.delete(symbol);
      }
      return next;
    });
  }, []);

  const [selectedSymbol, setSelectedSymbol] = useState<string | null>(null);
  // Funding/Spatial params come from the universal per-strategy store —
  // defaults apply until a saved record exists.
  const fundingParamsDto = strategyParams.get("funding");
  const spotParamsDto = strategyParams.get("spot");
  const fundingTradeSize = fundingParamsDto?.fundingTradeSizeUsd ?? 2000;
  const fundingParams = useMemo(
    () => ({
      minAprPct: fundingParamsDto?.fundingMinAprPct ?? 8.0,
    }),
    [fundingParamsDto?.fundingMinAprPct],
  );
  const spotParams = useMemo(
    () => ({
      tradeSizeUsd: spotParamsDto?.spotTradeSizeUsd ?? 1000,
      minNetSpreadBps: spotParamsDto?.spotMinNetSpreadBps ?? 15.0,
      maxSlippageBps: spotParamsDto?.spotMaxSlippageBps ?? 5.0,
    }),
    [
      spotParamsDto?.spotTradeSizeUsd,
      spotParamsDto?.spotMinNetSpreadBps,
      spotParamsDto?.spotMaxSlippageBps,
    ],
  );
  const [refreshing, setRefreshing] = useState(false);
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [tickerConfigModal, setTickerConfigModal] = useState<{
    isOpen: boolean;
    symbol: string;
    strategy?: string;
  }>({
    isOpen: false,
    symbol: "BTC-USDC",
    strategy: "futures",
  });

  // Vault state & startup unlock prompt
  const [vaultStatus, setVaultStatus] = useState<VaultStatus | null>(null);
  const [showVaultModal, setShowVaultModal] = useState(false);
  const vaultLockCancelRef = useRef<(() => void) | null>(null);

  // Check vault status on app startup: if running in desktop app and vault is locked,
  // prompt user immediately to unlock or create master password.
  useEffect(() => {
    if (!api.isTauri) return;
    api
      .getVaultStatus()
      .then((status) => {
        setVaultStatus(status);
        if (status && !status.is_unlocked) {
          setShowVaultModal(true);
        }
      })
      .catch((err) => {
        console.warn("Startup vault status check failed:", err);
      });
  }, []);

  useEffect(() => {
    const handleBeforeUnload = () => {
      api.lockVault().catch(() => {});
    };
    window.addEventListener("beforeunload", handleBeforeUnload);
    return () => {
      window.removeEventListener("beforeunload", handleBeforeUnload);
    };
  }, []);

  useEffect(() => {
    return () => {
      vaultLockCancelRef.current?.();
      vaultLockCancelRef.current = null;
    };
  }, []);

  // Calculate enabled & connected venues and dynamically available tickers.
  // Derived from the backend's own admission report (READ_ONLY venues) so
  // account-level enablement and real adapter support are authoritative —
  // frontend flag state can be stale.
  const activeVenueCodes = useMemo(() => {
    return getAdmittedVenueCodes(systemStatus);
  }, [systemStatus]);

  // Backend-reported order capability per venue id — the authoritative
  // source for READ-ONLY badges (replaces the static frontend lists that
  // drifted from the adapters' real flags).
  const venueCaps = useMemo(() => {
    if (!systemStatus) return undefined;
    const map = new Map<string, { canSubmit: boolean; canEnterStrategy: boolean }>();
    for (const v of systemStatus.active_venues ?? []) {
      map.set(v.id, {
        canSubmit: v.can_submit === true,
        canEnterStrategy: v.can_enter_strategy === true,
      });
    }
    return map;
  }, [systemStatus]);

  // Tickers hidden via their configurator drop out of the explorer and all
  // strategy selection; the Simple Spread configurator's Reveal All clears
  // the manual list (right-click Hide re-adds).
  const [exemptedTickers, setExemptedTickers] = useState<Set<string>>(new Set());
  useEffect(() => {
    let dead = false;
    const load = () =>
      api
        .getExemptedTickers()
        .then((l) => !dead && setExemptedTickers(new Set(l)))
        .catch(() => {});
    load();
    const id = setInterval(load, 15000);
    return () => {
      dead = true;
      clearInterval(id);
    };
  }, []);

  // Active strategy's auto-filtered set (bases failing MIN aggregation).
  // Radar shares the futures perp universe, so it reads the futures record;
  // tabs without a filtered strategy apply no auto-filter. Manual hiding
  // always applies after auto-filtering (see applyTickerVisibility).
  const filterStrategyId = useMemo(() => {
    const id = TAB_STRATEGY[activeTab].id;
    if (id === "radar") return "futures";
    if (id === "futures" || id === "funding" || id === "spot") return id;
    return null;
  }, [activeTab]);
  const [autoBlockedTickers, setAutoBlockedTickers] = useState<Set<string> | null>(null);
  const [strategyParamsVersion, setStrategyParamsVersion] = useState(0);
  useEffect(() => {
    if (!filterStrategyId) {
      setAutoBlockedTickers(null);
      return;
    }
    let dead = false;
    const load = () =>
      api
        .getStrategyTickers(filterStrategyId)
        .then((rows) => {
          if (!dead) {
            setAutoBlockedTickers(
              new Set(
                rows.filter((r) => !r.passes).map((r) => r.base.toUpperCase()),
              ),
            );
          }
        })
        .catch(() => {});
    load();
    const id = setInterval(load, 30000);
    return () => {
      dead = true;
      clearInterval(id);
    };
  }, [filterStrategyId, strategyParamsVersion]);

  // Pre-exemption live universe — needed so the empty-state copy can tell
  // "no overlapping markets" apart from "everything hidden".
  const universeTickers: PerpTicker[] = useMemo(
    () => applyMarketUniverse(PERP_TICKERS, marketUniverse),
    [marketUniverse],
  );

  const availableTickers: PerpTicker[] = useMemo(() => {
    return applyTickerVisibility(universeTickers, autoBlockedTickers, exemptedTickers);
  }, [universeTickers, autoBlockedTickers, exemptedTickers]);

  const tickerUnavailableReason = tickerUnavailabilityReason(
    systemStatus,
    activeVenueCodes,
    marketUniverse.length > 0,
    universeTickers.length > 0,
  );

  // Selection is only ever an explicit user click in the explorer — never
  // auto-pick a ticker. A stale selection is cleared once real alternatives
  // exist; when nothing is available, keep it (the remaining venue's book is
  // still real data).
  // Keep a live selection so the per-ticker data loop (orderbooks, venue
  // deviation samples, opportunity scan — all keyed on selectedSymbol) runs
  // continuously from boot instead of idling until a manual click. Prefer
  // BTC-USDC (densest cross-venue coverage); fall back to the first listing.
  useEffect(() => {
    if (availableTickers.length === 0) return;
    if (
      selectedSymbol !== null &&
      availableTickers.some((t: PerpTicker) => t.symbol === selectedSymbol)
    ) {
      return;
    }
    const fallback =
      availableTickers.find((t: PerpTicker) => t.symbol === "BTC-USDC") ??
      availableTickers[0];
    setSelectedSymbol(fallback.symbol);
  }, [availableTickers, selectedSymbol]);

  // Per-ticker gating was removed with the explorer toggle: every available
  // ticker is enabled so bots can run without manual per-ticker activation.
  useEffect(() => {
    setEnabledTickers((prev) => {
      const next = new Set(prev);
      let changed = false;
      for (const t of availableTickers) {
        if (!next.has(t.symbol)) {
          next.add(t.symbol);
          changed = true;
        }
      }
      return changed ? next : prev;
    });
  }, [availableTickers]);

  // Load baseline status and telemetry
  const refreshAllData = useCallback(async (customQty?: number, customMinBps?: number) => {
    setRefreshing(true);
    try {
      const params = strategyParamsRef.current;
      const equityBaseline = totalEquityUsdRef.current;
      const ticker = selectedSymbol
        ? (availableTickers.length > 0 ? availableTickers : PERP_TICKERS).find((t: PerpTicker) => t.symbol === selectedSymbol)
        : undefined;
      const futuresParams = params.get("futures");
      const sizing = deriveStepSizing(
        ticker?.symbol ?? "",
        ticker?.venues ?? [],
        ticker?.basePrice ?? 0,
        equityBaseline,
        {
          stepSizeMult: futuresParams?.stepSizeMult ?? 10,
          maxStepUsd: futuresParams?.maxStepUsd ?? 2500,
          maxTickerEquityPct: futuresParams?.maxTickerEquityPct ?? 20,
        }
      );
      const qty = customQty ?? (ticker ? sizing.stepBaseQty : 0.5);
      const minBps = customMinBps ?? (futuresParams?.targetProfitBps ?? 1.0);

      // Every leg races a timeout into its empty fallback, so one stalled
      // venue can never stick the whole refresh (or a configurator save).
      const [status, settings, books, spatial, funding, prediction, risk, logs, configs, allParams, spot, triangle, tickerPos, positions, balances, signals] = await Promise.all([
        withTimeoutFallback(api.getSystemStatus(), null),
        withTimeoutFallback(api.getEngineSettings(), null),
        withTimeoutFallback(
          selectedSymbol ? api.fetchOrderbooks(selectedSymbol) : Promise.resolve([]),
          []
        ),
        withTimeoutFallback(
          selectedSymbol ? api.scanSpatialOpportunities(selectedSymbol, minBps, qty) : Promise.resolve([]),
          []
        ),
        withTimeoutFallback(
          // Scan only the resolved visible list. While the strategy's
          // auto-block list is still loading (null), pass [] rather than
          // the unfiltered universe — otherwise the first paint flashes
          // 100+ pairs that the $5M-volume/$0.50-price gates strip a
          // second later. An empty array scans nothing; never pass
          // undefined here (the backend's "" path scans its own full
          // venue universe, bypassing the frontend filter set).
          api.scanFundingOpportunities(
            filterStrategyId !== null && autoBlockedTickers === null
              ? []
              : availableTickers.map((t) => t.symbol),
            fundingTradeSize
          ),
          []
        ),
        withTimeoutFallback(api.scanPredictionOpportunities(""), []),
        withTimeoutFallback(api.getRiskOverview(), null),
        withTimeoutFallback(api.getExecutionLogs(), []),
        withTimeoutFallback(api.getAllTickerStrategyConfigs(), []),
        withTimeoutFallback(api.getAllStrategyParams(), [] as StrategyParamsDto[]),
        withTimeoutFallback(api.scanSpotOpportunities(), []),
        withTimeoutFallback(api.scanTriangleOpportunities(), []),
        withTimeoutFallback(
          selectedSymbol ? api.getTickerPositionState(selectedSymbol) : Promise.resolve(null),
          null
        ),
        withTimeoutFallback(api.getOpenPositions(), []),
        withTimeoutFallback(api.getVenueBalances(), []),
        withTimeoutFallback(api.listSignalExecutions(), [] as SignalExecutionDto[]),
      ]);

      setSystemStatus(status);
      if (api.isTauri) {
        api.getVaultStatus().then(setVaultStatus).catch(() => {});
      }
      if (status && typeof status.is_paused === "boolean") {
        setIsPaused(status.is_paused);
      }
      setEngineSettings(settings);
      setOrderbooks(books);
      setSpatialOpps(spatial);
      setFundingOpps(funding);
      setPredictionOpps(prediction);
      setRiskData(risk);
      setExecutionLogs(logs);
      setSpotOpps(spot);
      setTriangleOpps(triangle);
      setTickerPositionState(tickerPos);
      setOpenPositions(positions);
      setAttentionSignals(
        signals.filter((s) => s.status === "unresolved" || s.status === "imbalanced")
      );
      setTotalEquityUsd(resolveTotalVenueEquity(balances));

      if (configs && configs.length > 0) {
        setStrategyConfigs((prev) => {
          let changed = false;
          const next = new Map(prev);
          for (const c of configs) {
            const cur = next.get(c.symbol);
            if (!cur || JSON.stringify(cur) !== JSON.stringify(c)) {
              next.set(c.symbol, c);
              changed = true;
            }
          }
          return changed ? next : prev;
        });
      }

      if (allParams && allParams.length > 0) {
        setStrategyParams((prev) => {
          let changed = false;
          const next = new Map(prev);
          for (const p of allParams) {
            const cur = next.get(p.strategyId);
            if (!cur || JSON.stringify(cur) !== JSON.stringify(p)) {
              next.set(p.strategyId, p);
              changed = true;
            }
          }
          return changed ? next : prev;
        });
      }
    } catch (err) {
      console.error("Failed to refresh market data:", err);
    } finally {
      setRefreshing(false);
    }
  }, [selectedSymbol, fundingTradeSize, availableTickers, autoBlockedTickers, filterStrategyId]);

  useEffect(() => {
    refreshAllData();
    const interval = setInterval(() => {
      refreshAllData();
    }, 3000);
    return () => clearInterval(interval);
  }, [refreshAllData]);

  // Stable callbacks for the settings modal — inline closures change
  // identity every render and defeat EngineSettings' memo.
  const handleSettingsClose = useCallback(() => {
    setIsSettingsOpen(false);
    vaultLockCancelRef.current?.();
    vaultLockCancelRef.current = scheduleVaultLock(() => {
      vaultLockCancelRef.current = null;
      if (!api.isTauri) return;
      api
        .lockVault()
        .then((status) => setVaultStatus(status))
        .catch((e) => {
          // Refused while live positions are open — a locked vault cannot
          // flatten or supervise exits. Vault stays unlocked by design.
          console.info("Vault auto-lock skipped:", e);
        });
    }, 60_000);
  }, []);
  const handleSettingsSaved = useCallback(
    (newSettings: EngineSettingsDto) => {
      setEngineSettings(newSettings);
      refreshAllData();
    },
    [refreshAllData]
  );

  const venueKey = useMemo(() => [...activeVenueCodes].sort().join(","), [activeVenueCodes]);

  // Actual market universe per admitted venue — the catalog's static venue
  // claims are not trusted; availability is the live intersection. Reloads
  // immediately when the admitted venue set changes, then every 30s.
  useEffect(() => {
    if (venueKey === "") {
      setMarketUniverse([]);
      return;
    }
    let cancelled = false;
    const load = () => {
      api
        .getMarketUniverse()
        .then((u) => {
          if (!cancelled) setMarketUniverse(u);
        })
        .catch(() => {
          if (!cancelled) setMarketUniverse([]);
        });
    };
    load();
    const interval = setInterval(load, 30000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [venueKey]);

  // 24h volume stats per venue — slow-moving data, polled on a 30s cadence
  // and refetched whenever the connected venue set changes.
  useEffect(() => {
    if (activeVenueCodes.size === 0) {
      setTickerVolumes([]);
      return;
    }
    let cancelled = false;
    const load = () => {
      api
        .getTickerVolumes()
        .then((v) => {
          if (!cancelled) setTickerVolumes(v);
        })
        .catch(() => {
          if (!cancelled) setTickerVolumes([]);
        });
    };
    load();
    const interval = setInterval(load, 30000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [venueKey]);

  // Risk actions
  const handleResetCircuitBreaker = useCallback(async () => {
    await api.resetCircuitBreaker();
    await refreshAllData();
  }, [refreshAllData]);

  const handleEmergencyKillswitch = useCallback(async () => {
    await api.triggerKillswitch();
    await refreshAllData();
  }, [refreshAllData]);

  const handleTogglePause = useCallback(
    async (paused: boolean) => {
      setIsPaused(paused);
      try {
        await api.setEnginePaused(paused);
        await refreshAllData();
      } catch (err) {
        console.error("Failed to toggle pause status:", err);
      }
    },
    [refreshAllData],
  );


  const handleExecuteTrade = useCallback(
    async (opp: SpatialOpportunityDto) => {
    // Same-time arbitration: every currently-scanned executable candidate for
    // this symbol goes into one batch; the backend dispatches only the one
    // with the highest expected net profit and blocks the ticker until that
    // signal's own legs reconcile as delta-neutral.
    const candidates: SpatialCandidateInput[] = spatialOpps
      .filter((o) => o.symbol === opp.symbol && o.is_executable)
      .map((o) => ({
        symbol: o.symbol,
        buyVenue: o.buy_venue,
        sellVenue: o.sell_venue,
        orderQty: o.order_qty,
        expectedNetUsd: o.estimated_profit_usd,
        direction: o.direction ?? null,
        routeId: null,
        positionStep: o.position_step ?? null,
        maxUnits: o.max_units ?? null,
        predictedWinProbability: o.win_probability ?? null,
      }));
    // The clicked opportunity may come from the matrix grid rather than the
    // scan snapshot — always include it in its ticker's batch.
    if (
      !candidates.some(
        (c) =>
          c.buyVenue === opp.buy_venue &&
          c.sellVenue === opp.sell_venue &&
          c.orderQty === opp.order_qty
      )
    ) {
      candidates.push({
        symbol: opp.symbol,
        buyVenue: opp.buy_venue,
        sellVenue: opp.sell_venue,
        orderQty: opp.order_qty,
        expectedNetUsd: opp.estimated_profit_usd,
        direction: opp.direction ?? null,
        routeId: null,
        positionStep: opp.position_step ?? null,
        maxUnits: opp.max_units ?? null,
        predictedWinProbability: opp.win_probability ?? null,
      });
    }

    const outcomes = await api.executeSpatialCandidates(candidates);
    const selected = outcomes.find((o) => o.selected);
    if (selected) {
      if (selected.reason) throw new Error(selected.reason);
      if (selected.result && !selected.result.execution_success) {
        throw new Error(selected.result.message);
      }
    } else {
      // Nothing dispatched (e.g. ticker blocked by an unreconciled signal) —
      // surface the arbitration reason so the click isn't silently ignored.
      const reason = outcomes.find((o) => o.reason)?.reason;
      if (reason) throw new Error(reason);
    }
    await refreshAllData();
  }, [spatialOpps, refreshAllData]);

  const handleExecuteCarry = useCallback(
    async (opp: FundingOpportunityDto, auto = false) => {
      // Never deploy more than either leg's venue can fund — cap the
      // configured trade size at the lower of the two venue balances.
      // Depth-aware optimal size (when measured) caps it further.
      let size = opp.trade_size_usd;
      if (opp.optimal_size_usd != null && opp.optimal_size_usd > 0 && opp.optimal_size_usd < size) {
        size = opp.optimal_size_usd;
      }
      try {
        const balances = await api.getVenueBalances().catch(() => []);
        const fundsFor = (venue: string): number | null => {
          const b = balances.find((x) => x.venue.toLowerCase() === venue.toLowerCase());
          const avail = b != null && b.available_margin_usd > 0 ? b.available_margin_usd : b?.equity_usd ?? null;
          return avail != null && avail > 0 ? avail : null;
        };
        for (const leg of [fundsFor(opp.long_venue), fundsFor(opp.short_venue)]) {
          if (leg != null && leg < size) size = leg;
        }
      } catch {
        /* balance lookup failed — fall back to the configured trade size */
      }
      await api.executeFundingCarry(opp.symbol, opp.long_venue, opp.short_venue, size, auto, opp.kind);
      await refreshAllData();
    },
    [refreshAllData],
  );

  const handleExecutePrediction = useCallback(
    async (opp: PredictionOpportunityDto) => {
      await api.executePredictionArbitrage(opp.market_title, opp.buy_yes_venue, opp.buy_no_venue, 100.0);
      await refreshAllData();
    },
    [refreshAllData],
  );

  const handleOpenTickerConfig = useCallback(
    (symbol: string, strategy?: string) => {
      setTickerConfigModal({
        isOpen: true,
        // Context ticker for contract-spec previews — strategy params themselves
        // are universal across tickers.
        symbol: symbol || "BTC-USDC",
        strategy:
          strategy ||
          (activeTab === "funding"
            ? "funding"
            : activeTab === "spot_arb"
            ? "spot"
            : activeTab === "triangle"
            ? "triangle"
            : "futures"),
      });
    },
    [activeTab],
  );

  const handleSaveStrategyParams = useCallback(
    async (params: StrategyParamsDto) => {
      setStrategyParams((prev) => {
        const next = new Map(prev);
        next.set(params.strategyId, params);
        return next;
      });
      setStrategyParamsVersion((v) => v + 1);
      await refreshAllData();
    },
    [refreshAllData],
  );

  // Stable child callbacks — inline closures below would defeat memo on the
  // hot panels re-rendered by the 3s fan-out.
  const handleOpenSettings = useCallback(() => {
    vaultLockCancelRef.current?.();
    vaultLockCancelRef.current = null;
    setIsSettingsOpen(true);
  }, []);
  const handleSelectPosition = useCallback(
    (pos: OpenPositionDto) => {
      setSelectedPositionKey(pos.key);
      const ticker = (availableTickers.length > 0 ? availableTickers : PERP_TICKERS).find(
        (t: PerpTicker) => t.symbol === pos.symbol,
      );
      if (ticker) setSelectedSymbol(ticker.symbol);
    },
    [availableTickers],
  );
  const handleHideTicker = useCallback(() => {
    api
      .getExemptedTickers()
      .then((l) => setExemptedTickers(new Set(l)))
      .catch(() => {});
  }, []);
  const handleTickerConfigModalClose = useCallback(
    () => setTickerConfigModal((prev) => ({ ...prev, isOpen: false })),
    [],
  );
  const handlePredictionSearch = useCallback(async (query: string) => {
    const res = await api.scanPredictionOpportunities(query);
    setPredictionOpps(res);
  }, []);
  const handleStatArbRunStep = useCallback(
    (step: number, py: number, px: number) => api.runStatArbStep(step, py, px),
    [],
  );
  const handleStatArbSolveBertram = useCallback(
    (theta: number, mu: number, sigma: number, cost: number) =>
      api.computeOptimalThresholds(theta, mu, sigma, cost),
    [],
  );
  const handleStatArbFitOu = useCallback(
    (prices: number[], dt: number) => api.fitOuProcess(prices, dt),
    [],
  );
  const handleStatArbFetchQuotes = useCallback(
    (symbols: string[]) => api.fetchMultiMarketQuotes(symbols),
    [],
  );
  const futuresParams = useMemo(
    () => strategyParams.get("futures") ?? null,
    [strategyParams],
  );

  // The explorer node is composed into LiveMatrix — memoize the element so
  // the 3s refresh doesn't hand LiveMatrix a fresh child identity.
  const tickerExplorerNode = useMemo(
    () => (
      <TickerExplorer
        selectedSymbol={selectedSymbol}
        onSelectTicker={setSelectedSymbol}
        executionLogs={executionLogs}
        strategyConfigs={strategyConfigs}
        futuresParams={futuresParams}
        totalEquityUsd={totalEquityUsd}
        riskData={riskData}
        enabledTickers={enabledTickers}
        onToggleTicker={handleToggleTicker}
        onOpenSettings={handleOpenSettings}
        onOpenTickerConfig={handleOpenTickerConfig}
        availableTickers={availableTickers}
        activeVenueCodes={activeVenueCodes}
        tickerVolumes={tickerVolumes}
        systemStatus={systemStatus}
        tickerUnavailableReason={tickerUnavailableReason}
        onHideTicker={handleHideTicker}
      />
    ),
    [
      selectedSymbol,
      executionLogs,
      strategyConfigs,
      futuresParams,
      totalEquityUsd,
      riskData,
      enabledTickers,
      handleToggleTicker,
      handleOpenSettings,
      handleOpenTickerConfig,
      availableTickers,
      activeVenueCodes,
      tickerVolumes,
      systemStatus,
      tickerUnavailableReason,
      handleHideTicker,
    ],
  );

  return (
    <div className="w-screen h-screen overflow-hidden bg-background flex items-center justify-center">
      <div
        style={{
          width: DESIGN_WIDTH * canvasScale,
          height: DESIGN_HEIGHT * canvasScale,
          position: "relative",
          flexShrink: 0,
        }}
      >
        <div
          className="flex flex-col bg-background text-slate-900 overflow-hidden font-sans border border-surface-border"
          style={{
            width: DESIGN_WIDTH,
            height: DESIGN_HEIGHT,
            transform: `scale(${canvasScale})`,
            transformOrigin: "top left",
            position: "absolute",
          }}
        >
      {/* Top Header Bar with Section Switcher Navigation */}
      <Header
        status={systemStatus}
        isPaused={isPaused}
        onTogglePause={handleTogglePause}
        onEmergencyKillswitch={handleEmergencyKillswitch}
        refreshing={refreshing}
        activeTab={activeTab}
        onSelectTab={handleSelectTab}
        spatialCount={spatialOpps.filter((o) => o.is_executable).length}
        fundingCount={fundingOpps.length}
        spotCount={spotOpps.filter((o) => o.is_executable).length}
        triangleCount={triangleOpps.filter((t) => t.is_executable).length}
        predictionCount={predictionOpps.filter((p) => p.has_arbitrage).length}
      />

      {/* Vault lock banner — auto-lock (timer) keeps provisioned venue
          credentials working; a manual lock wipes them and halts all live
          venue access until re-unlock. */}
      {api.isTauri &&
        vaultStatus &&
        vaultStatus.is_initialized &&
        !vaultStatus.is_unlocked && (
          <div
            className={`shrink-0 px-4 py-2 flex items-center justify-between border-b ${
              vaultStatus.auto_locked
                ? "bg-amber-500/10 border-amber-500/30"
                : "bg-loss-dim border-loss-border"
            }`}
          >
            <div className="flex items-center space-x-2.5">
              {vaultStatus.auto_locked ? (
                <ShieldCheck className="w-4 h-4 text-amber-700 shrink-0" />
              ) : (
                <Lock className="w-4 h-4 text-loss shrink-0" />
              )}
              <span
                className={`text-[11px] font-mono ${
                  vaultStatus.auto_locked ? "text-amber-700" : "text-loss"
                }`}
              >
                {vaultStatus.auto_locked
                  ? "Vault auto-locked — trading continues on provisioned credentials; unlock to change API keys."
                  : "Vault locked — credentialed venue reads and order execution halted. Unlock to resume."}
              </span>
            </div>
            <button
              type="button"
              onClick={() => setShowVaultModal(true)}
              className="btn-accent px-2.5 py-1 text-[11px] font-mono font-bold flex items-center space-x-1.5 shrink-0 cursor-pointer"
            >
              <Unlock className="w-3 h-3" />
              <span>Unlock Vault</span>
            </button>
          </div>
        )}

      {/* Reconciler attention banner — escalated or imbalanced signals may
          still carry venue exposure, so the alert lives on every tab. The
          acknowledge/flatten controls stay on Spread Radar. */}
      {attentionSignals.length > 0 && (
        <div
          role="alert"
          className="shrink-0 px-4 py-2 flex items-center justify-between border-b bg-loss-dim border-loss-border"
        >
          <div className="flex items-center space-x-2.5 min-w-0">
            <AlertTriangle className="w-4 h-4 text-loss shrink-0" />
            <span className="text-[11px] font-mono text-loss truncate">
              {attentionSignals.length} signal{attentionSignals.length === 1 ? " needs" : "s need"}{" "}
              operator review — residual exposure may be open (
              {attentionSignals
                .slice(0, 4)
                .map((s) => `${s.symbol} ${s.status}`)
                .join(", ")}
              {attentionSignals.length > 4 ? ", …" : ""})
            </span>
          </div>
          <button
            type="button"
            onClick={() => handleSelectTab("radar")}
            className="px-2.5 py-1 text-[11px] font-mono font-bold rounded border border-loss-border text-loss hover:bg-loss/10 transition shrink-0 cursor-pointer"
          >
            Review in Spread Radar
          </button>
        </div>
      )}

      {/* Main Workspace Body */}
      <div className="relative flex flex-1 overflow-hidden">
        {/* Left Aggregated Positions Panel — symbol selection is
            strategy-specific, so the ticker explorer lives inside the
            Simple Spread page rather than the universal sidebar. */}
        <PositionsPanel
          positions={openPositions}
          currentStrategyId={TAB_STRATEGY[activeTab].id}
          currentStrategyLabel={TAB_STRATEGY[activeTab].label}
          selectedPositionKey={selectedPositionKey}
          onSelectPosition={handleSelectPosition}
          status={systemStatus}
          isPaused={isPaused}
          onOpenPortfolioMonitor={handleTogglePortfolio}
          isPortfolioActive={activeTab === "portfolio_monitor"}
          onOpenSettings={handleOpenSettings}
        />

        {/* Workspace Content Pane */}
        <main className="flex-1 overflow-hidden bg-background">
          {activeTab === "spatial" && (
            <LiveMatrix
              symbol={selectedSymbol}
              orderbooks={orderbooks}
              opportunities={spatialOpps}
              onRefresh={refreshAllData}
              onExecuteTrade={handleExecuteTrade}
              loading={refreshing}
              executionLogs={executionLogs}
              onOpenTickerConfig={handleOpenTickerConfig}
              activeVenueCodes={activeVenueCodes}
              availableTickers={availableTickers}
              isTickerEnabled={selectedSymbol !== null && enabledTickers.has(selectedSymbol)}
              onToggleTicker={handleToggleTicker}
              isPaused={isPaused}
              positionState={tickerPositionState}
              futuresParams={futuresParams}
              totalEquityUsd={totalEquityUsd}
              tickerExplorer={tickerExplorerNode}
              venueCaps={venueCaps}
            />
          )}

          {activeTab === "funding" && (
            <FundingRadar
              opportunities={fundingOpps}
              availableTickers={availableTickers}
              symbol={selectedSymbol}
              tradeSize={fundingTradeSize}
              minAprPct={fundingParams.minAprPct}
              onExecuteCarry={handleExecuteCarry}
              isLive={systemStatus?.is_live_mode === true}
              loading={refreshing}
              executionLogs={executionLogs}
              onOpenConfig={handleOpenTickerConfig}
              isPaused={isPaused}
              hasApiConnectivity={activeVenueCodes.size >= 2}
              openPositions={openPositions}
            />
          )}

          {activeTab === "spot_arb" && (
            <SpotArbitrageBot
              executionLogs={executionLogs}
              onRefreshAll={refreshAllData}
              onOpenConfig={handleOpenTickerConfig}
              isPaused={isPaused}
              hasApiConnectivity={activeVenueCodes.size >= 2}
              params={spotParams}
            />
          )}

          {activeTab === "triangle" && (
            <TriangularArbitrage
              executionLogs={executionLogs}
              onRefreshAll={refreshAllData}
              onOpenConfig={handleOpenTickerConfig}
              isPaused={isPaused}
              isLive={systemStatus?.is_live_mode === true}
              hasApiConnectivity={activeVenueCodes.size >= 2}
              params={strategyParams.get("triangle")}
            />
          )}

          {activeTab === "prediction" && (
            <PredictionHub
              opportunities={predictionOpps}
              onSearch={handlePredictionSearch}
              onExecutePrediction={handleExecutePrediction}
              loading={refreshing}
              isPaused={isPaused}
              hasApiConnectivity={activeVenueCodes.size >= 2}
              onOpenConfig={handleOpenTickerConfig}
            />
          )}

          {activeTab === "statarb" && (
            <StatArbLab
              onRunStep={handleStatArbRunStep}
              onSolveBertram={handleStatArbSolveBertram}
              onFitOu={handleStatArbFitOu}
              onFetchQuotes={handleStatArbFetchQuotes}
            />
          )}

          {activeTab === "radar" && <OpportunityRadar venueCaps={venueCaps} />}

          {activeTab === "portfolio_monitor" && (
            <PortfolioMonitor
              riskData={riskData}
              status={systemStatus}
              openPositions={openPositions}
              onResetCircuitBreaker={handleResetCircuitBreaker}
              onTriggerKillswitch={handleEmergencyKillswitch}
            />
          )}
        </main>
      </div>

      {/* Pop-Out Engine Settings Modal */}
      <SettingsModal
        isOpen={isSettingsOpen}
        onClose={handleSettingsClose}
        systemStatus={systemStatus}
        onSettingsSaved={handleSettingsSaved}
      />

      {/* Pop-Out Strategy Configurator Modal — universal per-strategy params */}
      <TickerConfigModal
        isOpen={tickerConfigModal.isOpen}
        onClose={handleTickerConfigModalClose}
        tickerSymbol={tickerConfigModal.symbol}
        tickerCatalog={availableTickers}
        initialStrategy={tickerConfigModal.strategy}
        onSaveConfig={handleSaveStrategyParams}
      />

      {/* Pop-Out Vault Unlock / Password Creation Modal */}
      <VaultUnlockModal
        isOpen={showVaultModal && api.isTauri}
        vaultStatus={vaultStatus}
        onClose={() => setShowVaultModal(false)}
        onSuccess={(status) => {
          setVaultStatus(status);
          setShowVaultModal(false);
          refreshAllData();
        }}
      />
        </div>
      </div>
    </div>
  );
};
