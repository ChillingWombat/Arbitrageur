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
import { api } from "./services/api";
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
} from "./types";

import {
  PERP_TICKERS,
  getAdmittedVenueCodes,
  applyMarketUniverse,
  tickerUnavailabilityReason,
  type PerpTicker,
  STRATEGY_DISPLAY_NAMES,
} from "./types/tickers";
import { deriveUnitSize } from "./services/contractSpecs";

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
  triangle: { id: null, label: STRATEGY_DISPLAY_NAMES.triangle },
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
  const [tickerPositionState, setTickerPositionState] = useState<TickerPositionState | null>(null);
  const [openPositions, setOpenPositions] = useState<OpenPositionDto[]>([]);
  const [selectedPositionKey, setSelectedPositionKey] = useState<string | null>(null);
  const [enabledTickers, setEnabledTickers] = useState<Set<string>>(() => new Set());
  const [isPaused, setIsPaused] = useState(false);

  const handleToggleTicker = (symbol: string, enabled?: boolean) => {
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
  };

  const [selectedSymbol, setSelectedSymbol] = useState<string | null>(null);
  // Funding/Spatial params come from the universal per-strategy store —
  // defaults apply until a saved record exists.
  const fundingParamsDto = strategyParams.get("funding");
  const spotParamsDto = strategyParams.get("spot");
  const fundingTradeSize = fundingParamsDto?.fundingTradeSizeUsd ?? 25000;
  const fundingParams = {
    minAprPct: fundingParamsDto?.fundingMinAprPct ?? 8.0,
    holdingDays: fundingParamsDto?.fundingHoldingDays ?? 14,
  };
  const spotParams = {
    tradeSizeUsd: spotParamsDto?.spotTradeSizeUsd ?? 1000,
    minNetSpreadBps: spotParamsDto?.spotMinNetSpreadBps ?? 15.0,
    maxSlippageBps: spotParamsDto?.spotMaxSlippageBps ?? 5.0,
  };
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

  // Auto-lock vault 1 minute after exiting the settings modal, and when the program exits
  useEffect(() => {
    let timer: NodeJS.Timeout | null = null;
    if (!isSettingsOpen) {
      // Exited settings modal: start 60-second auto-lock timer
      timer = setTimeout(() => {
        api.lockVault().catch((err) => {
          console.warn("Auto-lock vault failed:", err);
        });
      }, 60000);
    }
    return () => {
      if (timer) clearTimeout(timer);
    };
  }, [isSettingsOpen]);

  useEffect(() => {
    const handleBeforeUnload = () => {
      api.lockVault().catch(() => {});
    };
    window.addEventListener("beforeunload", handleBeforeUnload);
    return () => {
      window.removeEventListener("beforeunload", handleBeforeUnload);
    };
  }, []);

  // Calculate enabled & connected venues and dynamically available tickers.
  // Derived from the backend's own admission report (READ_ONLY venues) so
  // account-level enablement and real adapter support are authoritative —
  // frontend flag state can be stale.
  const activeVenueCodes = useMemo(() => {
    return getAdmittedVenueCodes(systemStatus);
  }, [systemStatus]);

  // Tickers hidden via their configurator drop out of the explorer and all
  // strategy selection; System Settings → Reveal All clears the list.
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

  // Pre-exemption intersection — needed so the empty-state copy can tell
  // "no overlapping markets" apart from "everything hidden".
  const universeTickers: PerpTicker[] = useMemo(
    () => applyMarketUniverse(PERP_TICKERS, marketUniverse),
    [marketUniverse],
  );

  const availableTickers: PerpTicker[] = useMemo(() => {
    return universeTickers.filter(
      (t) => !exemptedTickers.has(t.base.toUpperCase()),
    );
  }, [universeTickers, exemptedTickers]);

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
  useEffect(() => {
    if (selectedSymbol !== null && availableTickers.length > 0) {
      const isAvailable = availableTickers.some((t: PerpTicker) => t.symbol === selectedSymbol);
      if (!isAvailable) {
        setSelectedSymbol(null);
      }
    }
  }, [availableTickers, selectedSymbol]);

  // Load baseline status and telemetry
  const refreshAllData = useCallback(async (customQty?: number, customMinBps?: number) => {
    setRefreshing(true);
    try {
      const ticker = selectedSymbol
        ? (availableTickers.length > 0 ? availableTickers : PERP_TICKERS).find((t: PerpTicker) => t.symbol === selectedSymbol)
        : undefined;
      const futuresParams = strategyParams.get("futures");
      const qty =
        customQty ??
        (ticker
          ? deriveUnitSize(
              ticker.symbol,
              ticker.venues,
              ticker.basePrice,
              futuresParams?.unitSizeUsd ?? 2500.0
            )
          : 0.5);
      const minBps = customMinBps ?? (futuresParams?.targetProfitBps ?? 1.0);

      const [status, settings, books, spatial, funding, prediction, risk, logs, configs, allParams, spot, triangle, tickerPos, positions] = await Promise.all([
        api.getSystemStatus(),
        api.getEngineSettings(),
        selectedSymbol ? api.fetchOrderbooks(selectedSymbol) : Promise.resolve([]),
        selectedSymbol ? api.scanSpatialOpportunities(selectedSymbol, minBps, qty) : Promise.resolve([]),
        selectedSymbol ? api.scanFundingOpportunities(selectedSymbol, fundingTradeSize) : Promise.resolve([]),
        api.scanPredictionOpportunities(""),
        api.getRiskOverview(),
        api.getExecutionLogs(),
        api.getAllTickerStrategyConfigs(),
        api.getAllStrategyParams().catch(() => [] as StrategyParamsDto[]),
        api.scanSpotOpportunities(),
        api.scanTriangleOpportunities(),
        selectedSymbol ? api.getTickerPositionState(selectedSymbol) : Promise.resolve(null),
        api.getOpenPositions().catch(() => []),
      ]);

      setSystemStatus(status);
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

      if (configs && configs.length > 0) {
        setStrategyConfigs((prev) => {
          const next = new Map(prev);
          for (const c of configs) {
            next.set(c.symbol, c);
          }
          return next;
        });
      }

      if (allParams && allParams.length > 0) {
        setStrategyParams((prev) => {
          const next = new Map(prev);
          for (const p of allParams) {
            next.set(p.strategyId, p);
          }
          return next;
        });
      }
    } catch (err) {
      console.error("Failed to refresh market data:", err);
    } finally {
      setRefreshing(false);
    }
  }, [selectedSymbol, fundingTradeSize, availableTickers, strategyParams]);

  useEffect(() => {
    refreshAllData();
    const interval = setInterval(() => {
      refreshAllData();
    }, 3000);
    return () => clearInterval(interval);
  }, [refreshAllData]);

  // Stable callbacks for the settings modal — inline closures change
  // identity every render and defeat EngineSettings' memo.
  const handleSettingsClose = useCallback(() => setIsSettingsOpen(false), []);
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
  const handleResetCircuitBreaker = async () => {
    await api.resetCircuitBreaker();
    await refreshAllData();
  };

  const handleEmergencyKillswitch = async () => {
    await api.triggerKillswitch();
    await refreshAllData();
  };

  const handleTogglePause = async (paused: boolean) => {
    setIsPaused(paused);
    try {
      await api.setEnginePaused(paused);
      await refreshAllData();
    } catch (err) {
      console.error("Failed to toggle pause status:", err);
    }
  };

  const handleToggleLiveMode = async (enabled: boolean) => {
    await api.setLiveMode(enabled);
    await refreshAllData();
  };

  const handleExecuteTrade = async (opp: SpatialOpportunityDto) => {
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
  };

  const handleExecuteCarry = async (opp: FundingOpportunityDto) => {
    await api.executeFundingCarry(opp.symbol, opp.long_venue, opp.short_venue, opp.trade_size_usd);
    await refreshAllData();
  };

  const handleExecutePrediction = async (opp: PredictionOpportunityDto) => {
    await api.executePredictionArbitrage(opp.market_title, opp.buy_yes_venue, opp.buy_no_venue, 100.0);
    await refreshAllData();
  };

  const handleOpenTickerConfig = (symbol: string, strategy?: string) => {
    setTickerConfigModal({
      isOpen: true,
      // Context ticker for contract-spec previews — strategy params themselves
      // are universal across tickers.
      symbol: symbol || "BTC-USDC",
      strategy: strategy || (activeTab === "funding" ? "funding" : activeTab === "spot_arb" ? "spot" : "futures"),
    });
  };

  const handleSaveStrategyParams = async (params: StrategyParamsDto) => {
    setStrategyParams((prev) => {
      const next = new Map(prev);
      next.set(params.strategyId, params);
      return next;
    });
    await refreshAllData();
  };

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
        onResetCircuitBreaker={handleResetCircuitBreaker}
        onEmergencyKillswitch={handleEmergencyKillswitch}
        onToggleLiveMode={handleToggleLiveMode}
        onOpenSettings={() => setIsSettingsOpen(true)}
        refreshing={refreshing}
        activeTab={activeTab}
        onSelectTab={handleSelectTab}
        spatialCount={spatialOpps.filter((o) => o.is_executable).length}
        fundingCount={fundingOpps.length}
        spotCount={spotOpps.filter((o) => o.is_executable).length}
        triangleCount={triangleOpps.filter((t) => t.is_executable).length}
        predictionCount={predictionOpps.filter((p) => p.has_arbitrage).length}
      />

      {/* Main Workspace Body */}
      <div className="relative flex flex-1 overflow-hidden">
        {/* Left Aggregated Positions Panel — symbol selection is
            strategy-specific, so the ticker explorer lives inside the
            Single Spread page rather than the universal sidebar. */}
        <PositionsPanel
          positions={openPositions}
          currentStrategyId={TAB_STRATEGY[activeTab].id}
          currentStrategyLabel={TAB_STRATEGY[activeTab].label}
          selectedPositionKey={selectedPositionKey}
          onSelectPosition={(pos) => {
            setSelectedPositionKey(pos.key);
            const ticker = (availableTickers.length > 0 ? availableTickers : PERP_TICKERS)
              .find((t: PerpTicker) => t.symbol === pos.symbol);
            if (ticker) setSelectedSymbol(ticker.symbol);
          }}
          status={systemStatus}
          isPaused={isPaused}
          onResetCircuitBreaker={handleResetCircuitBreaker}
          onOpenPortfolioMonitor={handleTogglePortfolio}
          isPortfolioActive={activeTab === "portfolio_monitor"}
          onOpenSettings={() => setIsSettingsOpen(true)}
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
              futuresParams={strategyParams.get("futures") ?? null}
              tickerExplorer={
                <TickerExplorer
                  selectedSymbol={selectedSymbol}
                  onSelectTicker={setSelectedSymbol}
                  executionLogs={executionLogs}
                  strategyConfigs={strategyConfigs}
                  futuresMaxUnits={strategyParams.get("futures")?.maxAccumulatedUnits}
                  riskData={riskData}
                  enabledTickers={enabledTickers}
                  onToggleTicker={handleToggleTicker}
                  onOpenSettings={() => setIsSettingsOpen(true)}
                  onOpenTickerConfig={handleOpenTickerConfig}
                  availableTickers={availableTickers}
                  activeVenueCodes={activeVenueCodes}
                  tickerVolumes={tickerVolumes}
                  systemStatus={systemStatus}
                  tickerUnavailableReason={tickerUnavailableReason}
                />
              }
            />
          )}

          {activeTab === "funding" && (
            <FundingRadar
              opportunities={fundingOpps}
              symbol={selectedSymbol}
              tradeSize={fundingTradeSize}
              minAprPct={fundingParams.minAprPct}
              holdingDays={fundingParams.holdingDays}
              onExecuteCarry={handleExecuteCarry}
              loading={refreshing}
              executionLogs={executionLogs}
              onOpenConfig={handleOpenTickerConfig}
              isPaused={isPaused}
              hasApiConnectivity={activeVenueCodes.size >= 2}
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
              isPaused={isPaused}
              hasApiConnectivity={activeVenueCodes.size >= 2}
            />
          )}

          {activeTab === "prediction" && (
            <PredictionHub
              opportunities={predictionOpps}
              onSearch={async (query) => {
                const res = await api.scanPredictionOpportunities(query);
                setPredictionOpps(res);
              }}
              onExecutePrediction={handleExecutePrediction}
              loading={refreshing}
              isPaused={isPaused}
              hasApiConnectivity={activeVenueCodes.size >= 2}
            />
          )}

          {activeTab === "statarb" && (
            <StatArbLab
              onRunStep={(step, py, px) => api.runStatArbStep(step, py, px)}
              onSolveBertram={(theta, mu, sigma, cost) =>
                api.computeOptimalThresholds(theta, mu, sigma, cost)
              }
              onFitOu={(prices, dt) => api.fitOuProcess(prices, dt)}
              onFetchQuotes={(symbols) => api.fetchMultiMarketQuotes(symbols)}
            />
          )}

          {activeTab === "radar" && <OpportunityRadar />}

          {activeTab === "portfolio_monitor" && (
            <PortfolioMonitor
              activeSymbol={selectedSymbol}
              riskData={riskData}
              status={systemStatus}
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
        onClose={() => setTickerConfigModal((prev) => ({ ...prev, isOpen: false }))}
        tickerSymbol={tickerConfigModal.symbol}
        initialStrategy={tickerConfigModal.strategy}
        onSaveConfig={handleSaveStrategyParams}
      />
        </div>
      </div>
    </div>
  );
};
