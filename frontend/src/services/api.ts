import type {
  SystemStatusDto,
  VenueMarketUniverseDto,
  VenueOrderBookDto,
  VenueVolumesDto,
  SignalSeriesDto,
  MatrixBaselineSnapshotDto,
  SpatialOpportunityDto,
  SpotOpportunityDto,
  SpotExecutionParams,
  TriangleOpportunityDto,
  TriangleExecutionParams,
  FundingOpportunityDto,
  PredictionOpportunityDto,
  StatArbStepResultDto,
  BertramThresholdDto,
  OuFitDto,
  RiskOverviewDto,
  SimStepResultDto,
  ExecutionLogEntry,
  EngineSettingsDto,
  CrossTickerExecutionParams,
  TickerMarketQuoteDto,
  TickerStrategyConfigDto,
  StrategyParamsDto,
  VaultStatus,
  VenueBalanceInfo,
  StrategyMetrics,
  WfoOptimizationResult,
  AccountDto,
  AccountUpsertInput,
  AccountCredentialInput,
  StrategyAccountRouteDto,
  StrategyRouteUpsertInput,
  SpatialCandidateInput,
  CandidateOutcomeDto,
  SignalExecutionDto,
  OpenPositionDto,
  TickerPositionState,
  FlattenResultDto,
  MonitorSnapshotDto,
  MonitorConfigDto,
} from "../types/index.ts";

// Check if running inside Tauri IPC environment
function isTauriEnvironment(): boolean {
  return typeof window !== "undefined" && ("__TAURI_INTERNALS__" in window || "__TAURI__" in window);
}


// Tauri core IPC dynamically loaded when inside desktop environment
async function getTauriCore() {
  if (!isTauriEnvironment()) return null;
  return import("@tauri-apps/api/core");
}

export const DESKTOP_REQUIRED_ERROR =
  "This feature requires the desktop application";

export async function invokeDesktop<T>(
  command: string,
  args?: Record<string, unknown>,
  loadCore: () => Promise<{ invoke: <R>(command: string, args?: Record<string, unknown>) => Promise<R> } | null> = getTauriCore,
): Promise<T> {
  const core = await loadCore();
  if (!core) throw new Error(DESKTOP_REQUIRED_ERROR);
  return core.invoke<T>(command, args);
}

// Commands registered in the desktop invoke handler (src-tauri/src/main.rs).
// Triangle scan/execute are intentionally absent: no authenticated three-leg
// coordinator exists, so those API methods fail closed on desktop.
const NATIVE_COMMANDS: ReadonlySet<string> = new Set([
  "get_system_status",
  "refresh_venue_latencies",
  "get_market_universe",
  "fetch_orderbooks",
  "get_ticker_volumes",
  "get_signal_series",
  "get_matrix_baselines",
  "scan_spatial_opportunities",
  "scan_spot_opportunities",
  "execute_spot_arbitrage",
  "scan_funding_opportunities",
  "scan_prediction_opportunities",
  "run_stat_arb_step",
  "compute_optimal_thresholds",
  "fit_ou_process",
  "execute_spatial_arbitrage",
  "execute_cross_ticker_arbitrage",
  "fetch_multi_market_quotes",
  "execute_funding_carry",
  "execute_prediction_arbitrage",
  "get_risk_overview",
  "reset_circuit_breaker",
  "trigger_emergency_killswitch",
  "get_execution_logs",
  "set_live_mode",
  "set_engine_paused",
  "get_engine_settings",
  "save_engine_settings",
  "get_ticker_strategy_config",
  "save_ticker_strategy_config",
  "get_all_ticker_strategy_configs",
  "get_strategy_params",
  "save_strategy_params",
  "get_all_strategy_params",
  "get_vault_status",
  "initialize_vault",
  "unlock_vault",
  "lock_vault",
  "delete_exchange_credentials",
  "list_accounts",
  "upsert_account",
  "delete_account",
  "delete_account_credentials",
  "get_account_credentials",
  "set_default_account",
  "list_strategy_routes",
  "upsert_strategy_route",
  "delete_strategy_route",
  "eligible_strategy_routes",
  "get_venue_balances",
  "get_strategy_performance",
  "run_wfo_calibration",
  "flatten_ticker_positions",
  "execute_spatial_candidates",
  "list_signal_executions",
  "get_ticker_position_state",
  "get_open_positions",
]);

export function nativeCommandAvailable(command: string): boolean {
  return NATIVE_COMMANDS.has(command);
}

export function normalizeTickerStrategyConfig(c: any): TickerStrategyConfigDto {
  const rawWindow = c?.slidingWindow ?? c?.sliding_window;
  const slidingWindow =
    typeof rawWindow === "string" && /^\d+[smhd]$/.test(rawWindow) ? rawWindow : "24h";
  if (!c) {
    return {
      symbol: "BTC-USDC",
      targetProfitBps: 1.0,
      windowPercentile: 50,
      maxAccumulatedUnits: 3,
      currentAccumulatedUnits: 0,
      executionMode: "SINGLE_WAY",
      unitSize: 0.005,
      userTargetLeverage: 5,
      slidingWindow,
    };
  }
  return {
    symbol: c.symbol || "BTC-USDC",
    targetProfitBps: Number(c.targetProfitBps ?? c.target_profit_bps ?? 1.0),
    windowPercentile: Number(c.windowPercentile ?? c.window_percentile ?? 50),
    maxAccumulatedUnits: Number(c.maxAccumulatedUnits ?? c.max_accumulated_units ?? 3),
    currentAccumulatedUnits: Number(c.currentAccumulatedUnits ?? c.current_accumulated_units ?? 0),
    executionMode: c.executionMode || c.execution_mode || "SINGLE_WAY",
    unitSize: Number(c.unitSize ?? c.unit_size ?? 0.005),
    userTargetLeverage: Number(c.userTargetLeverage ?? c.user_target_leverage ?? 5),
    slidingWindow,
  };
}

export const api = {
  get isTauri(): boolean {
    return isTauriEnvironment();
  },

  async getSystemStatus(): Promise<SystemStatusDto> {
    return invokeDesktop<SystemStatusDto>("get_system_status");
  },

  async refreshVenueLatencies(): Promise<SystemStatusDto> {
    return invokeDesktop<SystemStatusDto>("refresh_venue_latencies");
  },

  async getMarketUniverse(): Promise<VenueMarketUniverseDto[]> {
    return invokeDesktop<VenueMarketUniverseDto[]>("get_market_universe");
  },

  async fetchOrderbooks(symbol: string): Promise<VenueOrderBookDto[]> {
    return invokeDesktop("fetch_orderbooks", { symbol });
  },

  async getTickerVolumes(): Promise<VenueVolumesDto[]> {
    return invokeDesktop("get_ticker_volumes");
  },

  async getSignalSeries(
    symbol: string,
    mode: "consensus" | "leader",
    windowSecs: number,
  ): Promise<SignalSeriesDto> {
    return invokeDesktop("get_signal_series", { symbol, mode, windowSecs });
  },

  async getMatrixBaselines(
    symbol: string,
    mode: "manual" | "auto",
    manualWindowSecs: number,
    percentile: number,
  ): Promise<MatrixBaselineSnapshotDto> {
    return invokeDesktop<MatrixBaselineSnapshotDto>("get_matrix_baselines", {
      symbol,
      mode,
      manualWindowSecs,
      percentile,
    });
  },

  async scanSpatialOpportunities(symbol: string, minBps: number, orderQty: number): Promise<SpatialOpportunityDto[]> {
    return invokeDesktop("scan_spatial_opportunities", { symbol, minBps, orderQty });
  },

  async scanSpotOpportunities(symbol?: string): Promise<SpotOpportunityDto[]> {
    return invokeDesktop("scan_spot_opportunities", { symbol: symbol || "" });
  },

  async executeSpotArbitrage(params: SpotExecutionParams): Promise<SimStepResultDto> {
    return invokeDesktop("execute_spot_arbitrage", { params });
  },

  async scanTriangleOpportunities(filterAsset?: string): Promise<TriangleOpportunityDto[]> {
    const tauri = await getTauriCore();
    if (!tauri) throw new Error(DESKTOP_REQUIRED_ERROR);
    if (!nativeCommandAvailable("scan_triangle_opportunities")) return [];
    return tauri.invoke<TriangleOpportunityDto[]>("scan_triangle_opportunities", { filterAsset });
  },

  async executeTriangleArbitrage(params: TriangleExecutionParams): Promise<SimStepResultDto> {
    const tauri = await getTauriCore();
    if (!tauri) throw new Error(DESKTOP_REQUIRED_ERROR);
    if (!nativeCommandAvailable("execute_triangle_arbitrage")) {
      throw new Error("Native triangular execution is unavailable until an authenticated three-leg coordinator is implemented");
    }
    return tauri.invoke<SimStepResultDto>("execute_triangle_arbitrage", { params });
  },

  async scanFundingOpportunities(symbol: string, tradeSizeUsd: number): Promise<FundingOpportunityDto[]> {
    return invokeDesktop("scan_funding_opportunities", { symbol, tradeSizeUsd });
  },

  async scanPredictionOpportunities(market: string): Promise<PredictionOpportunityDto[]> {
    return invokeDesktop("scan_prediction_opportunities", { market });
  },

  async runStatArbStep(step: number, priceY: number, priceX: number): Promise<StatArbStepResultDto> {
    return invokeDesktop("run_stat_arb_step", { step, priceY, priceX });
  },

  async computeOptimalThresholds(
    theta: number,
    mu: number,
    sigma: number,
    transactionCost: number
  ): Promise<BertramThresholdDto> {
    return invokeDesktop("compute_optimal_thresholds", { theta, mu, sigma, transactionCost });
  },

  async fitOuProcess(prices: number[], dt: number): Promise<OuFitDto> {
    return invokeDesktop("fit_ou_process", { prices, dt });
  },


  async executeSpatialArbitrage(
    symbol: string,
    buyVenue: string,
    sellVenue: string,
    orderQty: number,
    direction?: string,
    triggerSpreadBps?: number,
    baselineMeanSpreadBps?: number,
    positionStep?: number,
    maxUnits?: number,
    nominalHurdleBps?: number
  ): Promise<SimStepResultDto> {
    return invokeDesktop<SimStepResultDto>("execute_spatial_arbitrage", {
      symbol,
      buyVenue,
      sellVenue,
      orderQty,
      direction,
      triggerSpreadBps,
      baselineMeanSpreadBps,
      positionStep,
      maxUnits,
      nominalHurdleBps,
    });
  },

  async executeCrossTickerArbitrage(
    paramsOrLongSymbol: CrossTickerExecutionParams | string,
    shortSymbol?: string,
    longVenue?: string,
    shortVenue?: string,
    orderQty?: number,
    netSpreadBps?: number
  ): Promise<SimStepResultDto> {
    const p: CrossTickerExecutionParams =
      typeof paramsOrLongSymbol === "string"
        ? {
            longSymbol: paramsOrLongSymbol,
            shortSymbol: shortSymbol!,
            longVenue: longVenue!,
            shortVenue: shortVenue!,
            orderQty: orderQty!,
            estProfitUsd: 0,
            netSpreadBps: netSpreadBps ?? 0,
          }
        : paramsOrLongSymbol;

    return invokeDesktop<SimStepResultDto>("execute_cross_ticker_arbitrage", {
      longSymbol: p.longSymbol,
      shortSymbol: p.shortSymbol,
      longVenue: p.longVenue,
      shortVenue: p.shortVenue,
      orderQty: p.orderQty,
      netSpreadBps: p.netSpreadBps,
    });
  },

  async fetchMultiMarketQuotes(symbols: string[]): Promise<TickerMarketQuoteDto[]> {
    return invokeDesktop<TickerMarketQuoteDto[]>("fetch_multi_market_quotes", { symbols });
  },

  async executeFundingCarry(symbol: string, longVenue: string, shortVenue: string, tradeSizeUsd: number): Promise<SimStepResultDto> {
    return invokeDesktop<SimStepResultDto>("execute_funding_carry", { symbol, longVenue, shortVenue, tradeSizeUsd });
  },

  async executePredictionArbitrage(marketTitle: string, buyYesVenue: string, buyNoVenue: string, quantity: number): Promise<SimStepResultDto> {
    return invokeDesktop<SimStepResultDto>("execute_prediction_arbitrage", { marketTitle, buyYesVenue, buyNoVenue, quantity });
  },

  async getEngineSettings(): Promise<EngineSettingsDto> {
    return invokeDesktop<EngineSettingsDto>("get_engine_settings");
  },

  async saveEngineSettings(settings: EngineSettingsDto): Promise<string> {
    return invokeDesktop<string>("save_engine_settings", { settings });
  },

  async getRiskOverview(): Promise<RiskOverviewDto> {
    return invokeDesktop<RiskOverviewDto>("get_risk_overview");
  },

  async resetCircuitBreaker(): Promise<string> {
    return invokeDesktop<string>("reset_circuit_breaker");
  },

  async triggerKillswitch(): Promise<string> {
    return invokeDesktop<string>("trigger_emergency_killswitch");
  },

  async getExecutionLogs(): Promise<ExecutionLogEntry[]> {
    return invokeDesktop<ExecutionLogEntry[]>("get_execution_logs");
  },

  async setLiveMode(enabled: boolean): Promise<boolean> {
    return invokeDesktop<boolean>("set_live_mode", { enabled });
  },

  async setEnginePaused(paused: boolean): Promise<boolean> {
    return invokeDesktop<boolean>("set_engine_paused", { paused });
  },

  async getTickerStrategyConfig(symbol: string): Promise<TickerStrategyConfigDto> {
    const res = await invokeDesktop<TickerStrategyConfigDto>("get_ticker_strategy_config", { symbol });
    return normalizeTickerStrategyConfig(res);
  },

  async saveTickerStrategyConfig(config: TickerStrategyConfigDto): Promise<void> {
    return invokeDesktop<void>("save_ticker_strategy_config", { config });
  },

  async getAllTickerStrategyConfigs(): Promise<TickerStrategyConfigDto[]> {
    const res = await invokeDesktop<TickerStrategyConfigDto[]>("get_all_ticker_strategy_configs");
    return (res || []).map(normalizeTickerStrategyConfig);
  },

  async getStrategyParams(strategyId: string): Promise<StrategyParamsDto> {
    return invokeDesktop<StrategyParamsDto>("get_strategy_params", { strategyId });
  },

  async saveStrategyParams(params: StrategyParamsDto): Promise<void> {
    return invokeDesktop<void>("save_strategy_params", { params });
  },

  async getAllStrategyParams(): Promise<StrategyParamsDto[]> {
    return invokeDesktop<StrategyParamsDto[]>("get_all_strategy_params");
  },

  async getVaultStatus(): Promise<VaultStatus> {
    return invokeDesktop<VaultStatus>("get_vault_status");
  },

  async initializeVault(password: string): Promise<VaultStatus> {
    return invokeDesktop<VaultStatus>("initialize_vault", { password });
  },

  async unlockVault(password: string): Promise<VaultStatus> {
    return invokeDesktop<VaultStatus>("unlock_vault", { password });
  },

  async lockVault(): Promise<VaultStatus> {
    return invokeDesktop<VaultStatus>("lock_vault");
  },

  async deleteExchangeCredentials(venue: string): Promise<boolean> {
    return invokeDesktop<boolean>("delete_exchange_credentials", { venue });
  },

  async listAccounts(venue?: string): Promise<AccountDto[]> {
    return invokeDesktop<AccountDto[]>("list_accounts", { venue: venue ?? null });
  },

  async upsertAccount(input: AccountUpsertInput): Promise<AccountDto> {
    return invokeDesktop<AccountDto>("upsert_account", { input });
  },

  async deleteAccount(accountId: string): Promise<boolean> {
    return invokeDesktop<boolean>("delete_account", { accountId });
  },

  async deleteAccountCredentials(accountId: string): Promise<boolean> {
    return invokeDesktop<boolean>("delete_account_credentials", { accountId });
  },

  /** Full decrypted credentials for one account. Requires unlocked vault; desktop only. */
  async getAccountCredentials(accountId: string): Promise<AccountCredentialInput> {
    return invokeDesktop<AccountCredentialInput>("get_account_credentials", { accountId });
  },

  async setDefaultAccount(venue: string, accountId: string): Promise<void> {
    return invokeDesktop<void>("set_default_account", { venue, accountId });
  },

  async listStrategyRoutes(strategyId?: string): Promise<StrategyAccountRouteDto[]> {
    return invokeDesktop<StrategyAccountRouteDto[]>("list_strategy_routes", { strategyId: strategyId ?? null });
  },

  async upsertStrategyRoute(input: StrategyRouteUpsertInput): Promise<StrategyAccountRouteDto> {
    return invokeDesktop<StrategyAccountRouteDto>("upsert_strategy_route", { input });
  },

  async deleteStrategyRoute(routeId: string): Promise<boolean> {
    return invokeDesktop<boolean>("delete_strategy_route", { routeId });
  },

  async eligibleStrategyRoutes(strategyId: string, venueA: string, venueB: string): Promise<StrategyAccountRouteDto[]> {
    return invokeDesktop<StrategyAccountRouteDto[]>("eligible_strategy_routes", { strategyId, venueA, venueB });
  },

  async executeSpatialCandidates(
    candidates: SpatialCandidateInput[]
  ): Promise<CandidateOutcomeDto[]> {
    return invokeDesktop<CandidateOutcomeDto[]>("execute_spatial_candidates", { candidates });
  },

  async listSignalExecutions(
    symbol?: string,
    status?: string
  ): Promise<SignalExecutionDto[]> {
    return invokeDesktop<SignalExecutionDto[]>("list_signal_executions", {
      symbol: symbol ?? null,
      status: status ?? null,
    });
  },

  async getTickerPositionState(symbol: string): Promise<TickerPositionState> {
    return invokeDesktop<TickerPositionState>("get_ticker_position_state", { symbol });
  },

  async getOpenPositions(strategyId?: string | null): Promise<OpenPositionDto[]> {
    return invokeDesktop<OpenPositionDto[]>("get_open_positions", {
      strategyId: strategyId ?? null,
    });
  },

  async flattenTickerPositions(
    symbol: string | null,
    strategyId: string
  ): Promise<FlattenResultDto> {
    return invokeDesktop<FlattenResultDto>("flatten_ticker_positions", {
      symbol,
      strategyId,
    });
  },

  async getVenueBalances(): Promise<VenueBalanceInfo[]> {
    return invokeDesktop<VenueBalanceInfo[]>("get_venue_balances");
  },

  async getStrategyPerformance(strategyId?: string): Promise<StrategyMetrics[]> {
    return invokeDesktop<StrategyMetrics[]>("get_strategy_performance", { strategyId });
  },

  async runWfoCalibration(symbol: string): Promise<WfoOptimizationResult> {
    return invokeDesktop<WfoOptimizationResult>("run_wfo_calibration", { symbol });
  },

  // ---- Spread Range Monitor (Opportunity Radar) ----
  async getMonitorSnapshot(): Promise<MonitorSnapshotDto | null> {
    return invokeDesktop<MonitorSnapshotDto | null>("get_monitor_snapshot");
  },

  async getMonitorConfig(): Promise<MonitorConfigDto> {
    return invokeDesktop<MonitorConfigDto>("get_monitor_config");
  },

  async setMonitorConfig(config: MonitorConfigDto): Promise<void> {
    return invokeDesktop<void>("set_monitor_config", { config });
  },

  async setTickerExempted(symbol: string, exempt: boolean): Promise<void> {
    return invokeDesktop<void>("set_ticker_exempted", { symbol, exempt });
  },

  async getExemptedTickers(): Promise<string[]> {
    return invokeDesktop<string[]>("get_exempted_tickers");
  },

  async revealAllTickers(): Promise<number> {
    return invokeDesktop<number>("reveal_all_tickers");
  },

  async executeMonitorOpportunity(
    symbol: string,
    buyVenue: string,
    sellVenue: string,
    sizeUsd: number,
  ): Promise<SimStepResultDto> {
    return invokeDesktop<SimStepResultDto>("execute_monitor_opportunity", {
      symbol,
      buyVenue,
      sellVenue,
      sizeUsd,
    });
  },
};


