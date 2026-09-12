import React, { useState, useEffect, useCallback, useMemo } from "react";
import { Header } from "./components/Header";
import { TickerExplorer } from "./components/TickerExplorer";
import type { ActiveTab } from "./components/Navigation";
import { LiveMatrix } from "./components/LiveMatrix";
import { FundingRadar } from "./components/FundingRadar";
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
  SpatialOpportunityDto,
  SpotOpportunityDto,
  TriangleOpportunityDto,
  FundingOpportunityDto,
  PredictionOpportunityDto,
  RiskOverviewDto,
  ExecutionLogEntry,
  TickerStrategyConfigDto,
  EngineSettingsDto,
} from "./types";

import {
  PERP_TICKERS,
  getDefaultOrderQty,
  getEnabledConnectedVenueCodes,
  filterAvailableTickers,
  type PerpTicker,
} from "./types/tickers";

export const App: React.FC = () => {
  const [activeTab, setActiveTab] = useState<ActiveTab>("spatial");
  const [systemStatus, setSystemStatus] = useState<SystemStatusDto | null>(null);
  const [engineSettings, setEngineSettings] = useState<EngineSettingsDto | null>(null);
  const [orderbooks, setOrderbooks] = useState<VenueOrderBookDto[]>([]);
  const [spatialOpps, setSpatialOpps] = useState<SpatialOpportunityDto[]>([]);
  const [spotOpps, setSpotOpps] = useState<SpotOpportunityDto[]>([]);
  const [triangleOpps, setTriangleOpps] = useState<TriangleOpportunityDto[]>([]);
  const [fundingOpps, setFundingOpps] = useState<FundingOpportunityDto[]>([]);
  const [predictionOpps, setPredictionOpps] = useState<PredictionOpportunityDto[]>([]);
  const [riskData, setRiskData] = useState<RiskOverviewDto | null>(null);
  const [executionLogs, setExecutionLogs] = useState<ExecutionLogEntry[]>([]);
  const [strategyConfigs, setStrategyConfigs] = useState<Map<string, TickerStrategyConfigDto>>(new Map());
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

  const handleToggleAllTickers = (enableAll: boolean) => {
    setEnabledTickers(
      enableAll ? new Set(availableTickers.map((t) => t.symbol)) : new Set()
    );
  };

  const [selectedSymbol, setSelectedSymbol] = useState("BTC-USDC");
  const [fundingTradeSize, setFundingTradeSize] = useState(25000);
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

  // Calculate enabled & connected venues and dynamically available tickers
  const activeVenueCodes = useMemo(() => {
    return getEnabledConnectedVenueCodes(engineSettings, systemStatus?.is_live_mode);
  }, [engineSettings, systemStatus?.is_live_mode]);

  const availableTickers: PerpTicker[] = useMemo(() => {
    return filterAvailableTickers(PERP_TICKERS, activeVenueCodes, 2);
  }, [activeVenueCodes]);

  // Fallback selectedSymbol to first available ticker if current selection loses availability
  useEffect(() => {
    if (availableTickers.length > 0) {
      const isAvailable = availableTickers.some((t: PerpTicker) => t.symbol === selectedSymbol);
      if (!isAvailable) {
        setSelectedSymbol(availableTickers[0].symbol);
      }
    }
  }, [availableTickers, selectedSymbol]);

  // Load baseline status and telemetry
  const refreshAllData = useCallback(async (customQty?: number, customMinBps?: number) => {
    setRefreshing(true);
    try {
      const ticker = (availableTickers.length > 0 ? availableTickers : PERP_TICKERS).find((t: PerpTicker) => t.symbol === selectedSymbol);
      const qty = customQty ?? (ticker ? getDefaultOrderQty(ticker.basePrice) : 0.5);
      const minBps = customMinBps ?? 5.0;

      const [status, settings, books, spatial, funding, prediction, risk, logs, configs, spot, triangle] = await Promise.all([
        api.getSystemStatus(),
        api.getEngineSettings(),
        api.fetchOrderbooks(selectedSymbol),
        api.scanSpatialOpportunities(selectedSymbol, minBps, qty),
        api.scanFundingOpportunities(selectedSymbol, fundingTradeSize),
        api.scanPredictionOpportunities(""),
        api.getRiskOverview(),
        api.getExecutionLogs(),
        api.getAllTickerStrategyConfigs(),
        api.scanSpotOpportunities(),
        api.scanTriangleOpportunities(),
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

      if (configs && configs.length > 0) {
        setStrategyConfigs((prev) => {
          const next = new Map(prev);
          for (const c of configs) {
            next.set(c.symbol, c);
          }
          return next;
        });
      }
    } catch (err) {
      console.error("Failed to refresh market data:", err);
    } finally {
      setRefreshing(false);
    }
  }, [selectedSymbol, fundingTradeSize, availableTickers]);

  useEffect(() => {
    refreshAllData();
    const interval = setInterval(() => {
      refreshAllData();
    }, 3000);
    return () => clearInterval(interval);
  }, [refreshAllData]);

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
    await api.executeSpatialArbitrage(
      opp.symbol,
      opp.buy_venue,
      opp.sell_venue,
      opp.order_qty,
      opp.direction,
      opp.trigger_spread_bps ?? opp.net_spread_bps,
      opp.baseline_mean_spread_bps,
      opp.position_step,
      opp.max_units,
      opp.nominal_hurdle_bps
    );
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
      symbol,
      strategy: strategy || (activeTab === "funding" ? "funding" : activeTab === "spot_arb" ? "spot" : "futures"),
    });
  };

  const handleSaveTickerConfig = async (newConfig: TickerStrategyConfigDto) => {
    await api.saveTickerStrategyConfig(newConfig);
    setStrategyConfigs((prev) => {
      const next = new Map(prev);
      next.set(newConfig.symbol, newConfig);
      return next;
    });
    await refreshAllData();
  };

  return (
    <div className="flex flex-col h-screen w-screen bg-background text-slate-900 overflow-hidden font-sans">
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
        onSelectTab={setActiveTab}
        spatialCount={spatialOpps.filter((o) => o.is_executable).length}
        fundingCount={fundingOpps.length}
        spotCount={spotOpps.filter((o) => o.is_executable).length}
        triangleCount={triangleOpps.filter((t) => t.is_executable).length}
        predictionCount={predictionOpps.filter((p) => p.has_arbitrage).length}
      />

      {!api.isTauri && (
        <div
          role="status"
          className="px-4 py-1.5 text-xs font-medium text-center bg-amber-100 text-amber-900 border-b border-amber-300"
        >
          Browser demo mode — execution, balances, PnL, risk, and calibration are synthetic; public quote views may use unauthenticated market data. Desktop IPC and exchange order submission are unavailable.
        </div>
      )}

      {/* Main Workspace Body */}
      <div className="flex flex-1 overflow-hidden">
        {/* Left Ticker Explorer Panel (Union of Perp DEX Tickers >= 2 Venues) */}
        <TickerExplorer
          selectedSymbol={selectedSymbol}
          onSelectTicker={setSelectedSymbol}
          executionLogs={executionLogs}
          strategyConfigs={strategyConfigs}
          status={systemStatus}
          riskData={riskData}
          enabledTickers={enabledTickers}
          onToggleTicker={handleToggleTicker}
          onToggleAllTickers={handleToggleAllTickers}
          onOpenSettings={() => setIsSettingsOpen(true)}
          onOpenTickerConfig={handleOpenTickerConfig}
          availableTickers={availableTickers}
          activeVenueCodes={activeVenueCodes}
          isPaused={isPaused}
          onResetCircuitBreaker={handleResetCircuitBreaker}
          onOpenPortfolioMonitor={() => setActiveTab("portfolio_monitor")}
          isPortfolioActive={activeTab === "portfolio_monitor"}
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
              isTickerEnabled={enabledTickers.has(selectedSymbol)}
              onToggleTicker={handleToggleTicker}
              isPaused={isPaused}
            />
          )}

          {activeTab === "funding" && (
            <FundingRadar
              opportunities={fundingOpps}
              symbol={selectedSymbol}
              tradeSize={fundingTradeSize}
              onTradeSizeChange={setFundingTradeSize}
              onExecuteCarry={handleExecuteCarry}
              loading={refreshing}
              executionLogs={executionLogs}
              onOpenConfig={handleOpenTickerConfig}
              isPaused={isPaused}
            />
          )}

          {activeTab === "spot_arb" && (
            <SpotArbitrageBot
              executionLogs={executionLogs}
              onRefreshAll={refreshAllData}
              onOpenConfig={handleOpenTickerConfig}
              isPaused={isPaused}
            />
          )}

          {activeTab === "triangle" && (
            <TriangularArbitrage
              executionLogs={executionLogs}
              onRefreshAll={refreshAllData}
              onOpenConfig={handleOpenTickerConfig}
              isPaused={isPaused}
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
            />
          )}

          {activeTab === "statarb" && (
            <StatArbLab
              onRunStep={(step, py, px) => api.runStatArbStep(step, py, px)}
              onSolveBertram={(theta, mu, sigma, cost) =>
                api.computeOptimalThresholds(theta, mu, sigma, cost)
              }
              onFitOu={(prices, dt) => api.fitOuProcess(prices, dt)}
            />
          )}

          {activeTab === "portfolio_monitor" && (
            <PortfolioMonitor
              activeSymbol={selectedSymbol}
              riskData={riskData}
              status={systemStatus}
              onResetCircuitBreaker={handleResetCircuitBreaker}
              onTriggerKillswitch={handleEmergencyKillswitch}
              onRefreshAll={refreshAllData}
            />
          )}
        </main>
      </div>

      {/* Pop-Out Engine Settings Modal */}
      <SettingsModal
        isOpen={isSettingsOpen}
        onClose={() => setIsSettingsOpen(false)}
        systemStatus={systemStatus}
        onSettingsSaved={(newSettings) => {
          setEngineSettings(newSettings);
          refreshAllData();
        }}
      />

      {/* Pop-Out Ticker Strategy Configurator Modal */}
      <TickerConfigModal
        isOpen={tickerConfigModal.isOpen}
        onClose={() => setTickerConfigModal((prev) => ({ ...prev, isOpen: false }))}
        tickerSymbol={tickerConfigModal.symbol}
        initialStrategy={tickerConfigModal.strategy}
        strategyConfig={strategyConfigs.get(tickerConfigModal.symbol)}
        isEnabled={enabledTickers.has(tickerConfigModal.symbol)}
        onToggleEnabled={(enabled) => handleToggleTicker(tickerConfigModal.symbol, enabled)}
        onSaveConfig={handleSaveTickerConfig}
      />
    </div>
  );
};
