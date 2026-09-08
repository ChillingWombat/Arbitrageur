import React, { useState, useEffect, useCallback } from "react";
import { Header } from "./components/Header";
import { TickerExplorer } from "./components/TickerExplorer";
import type { ActiveTab } from "./components/Navigation";
import { LiveMatrix } from "./components/LiveMatrix";
import { FundingRadar } from "./components/FundingRadar";
import { PredictionHub } from "./components/PredictionHub";
import { StatArbLab } from "./components/StatArbLab";
import { RiskOperations } from "./components/RiskOperations";
import { StrategyPerformance } from "./components/StrategyPerformance";
import { SettingsModal } from "./components/SettingsModal";
import { SpotArbitrageBot } from "./components/SpotArbitrageBot";
import { TickerConfigModal } from "./components/TickerConfigModal";
import { api } from "./services/api";
import type {
  SystemStatusDto,
  VenueOrderBookDto,
  SpatialOpportunityDto,
  SpotOpportunityDto,
  FundingOpportunityDto,
  PredictionOpportunityDto,
  RiskOverviewDto,
  ExecutionLogEntry,
  CrossTickerExecutionParams,
  TickerStrategyConfigDto,
} from "./types";

import { PERP_TICKERS, getDefaultOrderQty } from "./types/tickers";

export const App: React.FC = () => {
  const [activeTab, setActiveTab] = useState<ActiveTab>("spatial");
  const [systemStatus, setSystemStatus] = useState<SystemStatusDto | null>(null);
  const [orderbooks, setOrderbooks] = useState<VenueOrderBookDto[]>([]);
  const [spatialOpps, setSpatialOpps] = useState<SpatialOpportunityDto[]>([]);
  const [spotOpps, setSpotOpps] = useState<SpotOpportunityDto[]>([]);
  const [fundingOpps, setFundingOpps] = useState<FundingOpportunityDto[]>([]);
  const [predictionOpps, setPredictionOpps] = useState<PredictionOpportunityDto[]>([]);
  const [riskData, setRiskData] = useState<RiskOverviewDto | null>(null);
  const [executionLogs, setExecutionLogs] = useState<ExecutionLogEntry[]>([]);
  const [strategyConfigs, setStrategyConfigs] = useState<Map<string, TickerStrategyConfigDto>>(new Map());

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

  // Load baseline status and telemetry
  const refreshAllData = useCallback(async (customQty?: number, customMinBps?: number) => {
    setRefreshing(true);
    try {
      const ticker = PERP_TICKERS.find((t) => t.symbol === selectedSymbol);
      const qty = customQty ?? (ticker ? getDefaultOrderQty(ticker.basePrice) : 0.5);
      const minBps = customMinBps ?? 5.0;

      const [status, books, spatial, funding, prediction, risk, logs, configs, spot] = await Promise.all([
        api.getSystemStatus(),
        api.fetchOrderbooks(selectedSymbol),
        api.scanSpatialOpportunities(selectedSymbol, minBps, qty),
        api.scanFundingOpportunities(selectedSymbol, fundingTradeSize),
        api.scanPredictionOpportunities(""),
        api.getRiskOverview(),
        api.getExecutionLogs(),
        api.getAllTickerStrategyConfigs(),
        api.scanSpotOpportunities(),
      ]);

      setSystemStatus(status);
      setOrderbooks(books);
      setSpatialOpps(spatial);
      setFundingOpps(funding);
      setPredictionOpps(prediction);
      setRiskData(risk);
      setExecutionLogs(logs);
      setSpotOpps(spot);

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
  }, [selectedSymbol, fundingTradeSize]);

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

  const handleExecuteCrossTicker = async (params: CrossTickerExecutionParams) => {
    await api.executeCrossTickerArbitrage(params);
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
        onResetCircuitBreaker={handleResetCircuitBreaker}
        onEmergencyKillswitch={handleEmergencyKillswitch}
        onToggleLiveMode={handleToggleLiveMode}
        onOpenSettings={() => setIsSettingsOpen(true)}
        refreshing={refreshing}
        activeTab={activeTab}
        onSelectTab={setActiveTab}
        spatialCount={spatialOpps.filter((o) => o.is_executable).length}
        spotCount={spotOpps.filter((o) => o.is_executable).length}
        fundingCount={fundingOpps.length}
        predictionCount={predictionOpps.filter((p) => p.has_arbitrage).length}
      />

      {/* Main Workspace Body */}
      <div className="flex flex-1 overflow-hidden">
        {/* Left Ticker Explorer Panel (Union of Perp DEX Tickers >= 2 Venues) */}
        <TickerExplorer
          selectedSymbol={selectedSymbol}
          onSelectTicker={setSelectedSymbol}
          executionLogs={executionLogs}
          strategyConfigs={strategyConfigs}
          status={systemStatus}
          onOpenSettings={() => setIsSettingsOpen(true)}
          onOpenTickerConfig={handleOpenTickerConfig}
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
              onExecuteCrossTicker={handleExecuteCrossTicker}
              loading={refreshing}
              executionLogs={executionLogs}
              onOpenTickerConfig={handleOpenTickerConfig}
            />
          )}

          {activeTab === "spot_arb" && (
            <SpotArbitrageBot
              executionLogs={executionLogs}
              onRefreshAll={refreshAllData}
              onOpenConfig={handleOpenTickerConfig}
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

          {activeTab === "risk" && (
            <RiskOperations
              riskData={riskData}
              onResetCircuitBreaker={handleResetCircuitBreaker}
              onTriggerKillswitch={handleEmergencyKillswitch}
            />
          )}

          {activeTab === "performance" && <StrategyPerformance activeSymbol={selectedSymbol} />}
        </main>
      </div>

      {/* Pop-Out Engine Settings Modal */}
      <SettingsModal
        isOpen={isSettingsOpen}
        onClose={() => setIsSettingsOpen(false)}
        systemStatus={systemStatus}
      />

      {/* Pop-Out Ticker Strategy Configurator Modal */}
      <TickerConfigModal
        isOpen={tickerConfigModal.isOpen}
        onClose={() => setTickerConfigModal((prev) => ({ ...prev, isOpen: false }))}
        tickerSymbol={tickerConfigModal.symbol}
        initialStrategy={tickerConfigModal.strategy}
        strategyConfig={strategyConfigs.get(tickerConfigModal.symbol)}
        onSaveConfig={handleSaveTickerConfig}
      />
    </div>
  );
};
