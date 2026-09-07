import React, { useState, useEffect, useCallback } from "react";
import { Header } from "./components/Header";
import { TickerExplorer } from "./components/TickerExplorer";
import type { ActiveTab } from "./components/Navigation";
import { LiveMatrix } from "./components/LiveMatrix";
import { FundingRadar } from "./components/FundingRadar";
import { PredictionHub } from "./components/PredictionHub";
import { StatArbLab } from "./components/StatArbLab";
import { RiskOperations } from "./components/RiskOperations";
import { SimulationRunner } from "./components/SimulationRunner";
import { EngineSettings } from "./components/EngineSettings";
import { api } from "./services/api";
import type {
  SystemStatusDto,
  VenueOrderBookDto,
  SpatialOpportunityDto,
  FundingOpportunityDto,
  PredictionOpportunityDto,
  RiskOverviewDto,
  ExecutionLogEntry,
} from "./types";

import { PERP_TICKERS, getDefaultOrderQty } from "./types/tickers";

export const App: React.FC = () => {
  const [activeTab, setActiveTab] = useState<ActiveTab>("spatial");
  const [systemStatus, setSystemStatus] = useState<SystemStatusDto | null>(null);
  const [orderbooks, setOrderbooks] = useState<VenueOrderBookDto[]>([]);
  const [spatialOpps, setSpatialOpps] = useState<SpatialOpportunityDto[]>([]);
  const [fundingOpps, setFundingOpps] = useState<FundingOpportunityDto[]>([]);
  const [predictionOpps, setPredictionOpps] = useState<PredictionOpportunityDto[]>([]);
  const [riskData, setRiskData] = useState<RiskOverviewDto | null>(null);
  const [executionLogs, setExecutionLogs] = useState<ExecutionLogEntry[]>([]);

  const [selectedSymbol, setSelectedSymbol] = useState("BTC-USDC");
  const [fundingTradeSize, setFundingTradeSize] = useState(25000);
  const [refreshing, setRefreshing] = useState(false);

  // Load baseline status and telemetry
  const refreshAllData = useCallback(async (customQty?: number, customMinBps?: number) => {
    setRefreshing(true);
    try {
      const ticker = PERP_TICKERS.find((t) => t.symbol === selectedSymbol);
      const qty = customQty ?? (ticker ? getDefaultOrderQty(ticker.basePrice) : 0.5);
      const minBps = customMinBps ?? 5.0;

      const [status, books, spatial, funding, prediction, risk, logs] = await Promise.all([
        api.getSystemStatus(),
        api.fetchOrderbooks(selectedSymbol),
        api.scanSpatialOpportunities(selectedSymbol, minBps, qty),
        api.scanFundingOpportunities(selectedSymbol, fundingTradeSize),
        api.scanPredictionOpportunities(""),
        api.getRiskOverview(),
        api.getExecutionLogs(),
      ]);

      setSystemStatus(status);
      setOrderbooks(books);
      setSpatialOpps(spatial);
      setFundingOpps(funding);
      setPredictionOpps(prediction);
      setRiskData(risk);
      setExecutionLogs(logs);
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
    await api.executeSpatialArbitrage(opp.symbol, opp.buy_venue, opp.sell_venue, opp.order_qty);
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

  const handleRunSimStep = async (symbol: string, targetBps: number, qty: number) => {
    const res = await api.runSimulationStep(symbol, targetBps, qty);
    await refreshAllData();
    return res;
  };

  return (
    <div className="flex flex-col h-screen w-screen bg-background text-slate-900 overflow-hidden font-sans">
      {/* Top Header Bar with Section Switcher Navigation */}
      <Header
        status={systemStatus}
        onResetCircuitBreaker={handleResetCircuitBreaker}
        onEmergencyKillswitch={handleEmergencyKillswitch}
        onToggleLiveMode={handleToggleLiveMode}
        refreshing={refreshing}
        activeTab={activeTab}
        onSelectTab={setActiveTab}
        spatialCount={spatialOpps.filter((o) => o.is_executable).length}
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

          {activeTab === "sim" && (
            <SimulationRunner
              logs={executionLogs}
              totalPnl={systemStatus?.total_realized_pnl ?? 0}
              onRunStep={handleRunSimStep}
              symbol={selectedSymbol}
              onSymbolChange={setSelectedSymbol}
            />
          )}

          {activeTab === "settings" && <EngineSettings />}
        </main>
      </div>
    </div>
  );
};
