import type {
  SystemStatusDto,
  VenueMarketUniverseDto,
  VenueOrderBookDto,
  VenueVolumesDto,
  SignalSeriesDto,
  MatrixBaselineSnapshotDto,
  AutotuneRecommendationDto,
  AutotuneApplyResultDto,
  SeasonalityRunSummaryDto,
  SpatialOpportunityDto,
  SpotOpportunityDto,
  SpotExecutionParams,
  TriangleOpportunityDto,
  TriangleExecutionParams,
  ConvergenceCheckDto,
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
  PassiveUnwindDto,
  VaultStatus,
  VenueBalanceInfo,
  ExternalPositionsResponse,
  CloseRecommendationDto,
  CloseResultDto,
  StrategyMetrics,
  StrategyFillPnl,
  UtilisationHistory,
  WfoOptimizationResult,
  AccountDto,
  AccountUpsertInput,
  AccountCredentialInput,
  StrategyAccountRouteDto,
  StrategyRouteUpsertInput,
  StrategyTickerDto,
  AccountEquityEventDto,
  SpatialCandidateInput,
  CandidateOutcomeDto,
  SignalExecutionDto,
  OpenPositionDto,
  TickerPositionState,
  FlattenResultDto,
  MonitorSnapshotDto,
  MonitorConfigDto,
  RadarBotConfigDto,
  RadarBotStatusDto,
  SessionProfileDto,
  VenueCapabilityDto,
} from "../types/index.ts";
import { DEFAULT_PASSIVE_UNWIND } from "../types/index.ts";

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

/** Race an IPC call against a timeout, resolving the fallback instead of
 *  hanging or throwing — keeps refresh fan-outs settling when a venue stalls.
 *  Backend REST clients carry their own 15s timeouts; this bounds the rest. */
export function withTimeoutFallback<T>(promise: Promise<T>, fallback: T, ms = 30000): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<T>((_, reject) => {
    timer = setTimeout(() => reject(new Error(`request timed out after ${ms}ms`)), ms);
  });
  return Promise.race([promise, timeout])
    .catch(() => fallback)
    .finally(() => {
      if (timer !== undefined) clearTimeout(timer);
    });
}

// Commands registered in the desktop invoke handler (src-tauri/src/main.rs).
// Triangle scan/execute and the spatial convergence check are native
// paper-only commands; in the browser they fail closed with the
// desktop-required error instead of serving synthetic data.
const NATIVE_COMMANDS: ReadonlySet<string> = new Set([
  "get_system_status",
  "refresh_venue_latencies",
  "get_market_universe",
  "fetch_orderbooks",
  "get_ticker_volumes",
  "get_signal_series",
  "get_matrix_baselines",
  "scan_spatial_opportunities",
  "scan_triangle_opportunities",
  "execute_triangle_arbitrage",
  "check_spatial_convergence",
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
  "get_venue_execution_capabilities",
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
  "get_strategy_tickers",
  "get_account_equity_events",
  "get_autotune_recommendation",
  "apply_autotune_selected",
  "run_seasonality_promotion",
  "get_session_profile",
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
  "get_account_credential_field",
  "set_default_account",
  "list_strategy_routes",
  "upsert_strategy_route",
  "delete_strategy_route",
  "eligible_strategy_routes",
  "get_venue_balances",
  "get_strategy_performance",
  "get_strategy_fill_pnl",
  "get_utilisation_history",
  "run_wfo_calibration",
  "flatten_ticker_positions",
  "execute_spatial_candidates",
  "get_monitor_snapshot",
  "get_monitor_config",
  "set_monitor_config",
  "set_ticker_exempted",
  "get_exempted_tickers",
  "reveal_all_tickers",
  "execute_monitor_opportunity",
  "get_radar_bot_config",
  "set_radar_bot_config",
  "get_radar_bot_status",
  "list_signal_executions",
  "get_ticker_position_state",
  "get_open_positions",
  "get_external_positions",
  "recommend_external_close",
  "close_external_position",
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

export function normalizeStrategyParams(p: any): StrategyParamsDto {
  const num = (v: unknown, fb: number): number => {
    const n = Number(v);
    return Number.isFinite(n) ? n : fb;
  };
  const optThreshold = (v: unknown): number | null => {
    const n = Number(v);
    return Number.isFinite(n) && n > 0 ? n : null;
  };
  const normalizePassive = (v: any): PassiveUnwindDto => {
    if (!v || typeof v !== "object") return { ...DEFAULT_PASSIVE_UNWIND };
    const b = { ...DEFAULT_PASSIVE_UNWIND };
    return {
      enabled: (v.enabled ?? false) === true,
      spreadPercentile: num(v.spreadPercentile ?? v.spread_percentile, b.spreadPercentile),
      timeoutSecs: Math.round(num(v.timeoutSecs ?? v.timeout_secs, b.timeoutSecs)),
      maxTries: Math.round(num(v.maxTries ?? v.max_tries, b.maxTries)),
      maxSpreadBps: num(v.maxSpreadBps ?? v.max_spread_bps, b.maxSpreadBps),
      maxFastVolBps: num(v.maxFastVolBps ?? v.max_fast_vol_bps, b.maxFastVolBps),
    };
  };
  if (!p) {
    return {
      strategyId: "futures",
      targetProfitBps: 1.0,
      windowPercentile: 50,
      slidingWindow: "24h",
      slidingWindowMode: "manual",
      slidingWindowSecs: 86400,
      stepSizeMult: 10,
      maxStepUsd: 2500,
      maxTickerEquityPct: 20,
      executionMode: "SINGLE_WAY",
      userTargetLeverage: 5,
      autoMinVolumeUsd: null,
      autoMinPriceUsd: null,
      autoMinEventVolumeUsd: null,
      fundingTradeSizeUsd: 2000,
      fundingMinAprPct: 8.0,
      fundingHoldingDays: 14,
      futuresMaxSlippageBps: 15.0,
      fundingMaxSlippageBps: 15.0,
      predictionMaxSlippageBps: 15.0,
      fundingMaxEntrySpreadBps: 50.0,
      fundingAutoExitSpreadPct: null,
      fundingLiqTrimPct: null,
      fundingAutoRebalance: false,
      fundingRotationThresholdBps: null,
      fundingEnableBasis: true,
      fundingMinBasisBps: null,
      futuresLiqTrimPct: null,
      spotLiqTrimPct: null,
      predictionLiqTrimPct: null,
      spotTradeSizeUsd: 1000,
      spotMaxSlippageBps: 5.0,
      spotMinNetSpreadBps: 15.0,
      futuresPassiveUnwind: { ...DEFAULT_PASSIVE_UNWIND },
      fundingPassiveUnwind: { ...DEFAULT_PASSIVE_UNWIND },
      spotPassiveUnwind: { ...DEFAULT_PASSIVE_UNWIND },
    };
  }
  return {
    strategyId: p.strategyId ?? p.strategy_id ?? "futures",
    targetProfitBps: num(p.targetProfitBps ?? p.target_profit_bps, 1.0),
    windowPercentile: num(p.windowPercentile ?? p.window_percentile, 50),
    slidingWindow: p.slidingWindow ?? p.sliding_window ?? "24h",
    slidingWindowMode: p.slidingWindowMode === "auto" ? "auto" : "manual",
    slidingWindowSecs: num(p.slidingWindowSecs ?? p.sliding_window_secs, 86400),
    stepSizeMult: num(p.stepSizeMult ?? p.step_size_mult, 10),
    maxStepUsd: num(p.maxStepUsd ?? p.max_step_usd, 2500),
    maxTickerEquityPct: num(p.maxTickerEquityPct ?? p.max_ticker_equity_pct, 20),
    executionMode: p.executionMode || p.execution_mode || "SINGLE_WAY",
    userTargetLeverage: num(p.userTargetLeverage ?? p.user_target_leverage, 5),
    autoMinVolumeUsd: optThreshold(p.autoMinVolumeUsd ?? p.auto_min_volume_usd),
    autoMinPriceUsd: optThreshold(p.autoMinPriceUsd ?? p.auto_min_price_usd),
    autoMinEventVolumeUsd: optThreshold(p.autoMinEventVolumeUsd ?? p.auto_min_event_volume_usd),
    fundingTradeSizeUsd: num(p.fundingTradeSizeUsd ?? p.funding_trade_size_usd, 2000),
    fundingMinAprPct: num(p.fundingMinAprPct ?? p.funding_min_apr_pct, 8.0),
    fundingHoldingDays: num(p.fundingHoldingDays ?? p.funding_holding_days, 14),
    futuresMaxSlippageBps: num(p.futuresMaxSlippageBps ?? p.futures_max_slippage_bps, 15.0),
    fundingMaxSlippageBps: num(p.fundingMaxSlippageBps ?? p.funding_max_slippage_bps, 15.0),
    predictionMaxSlippageBps: num(p.predictionMaxSlippageBps ?? p.prediction_max_slippage_bps, 15.0),
    fundingMaxEntrySpreadBps: num(p.fundingMaxEntrySpreadBps ?? p.funding_max_entry_spread_bps, 50.0),
    fundingAutoExitSpreadPct: optThreshold(p.fundingAutoExitSpreadPct ?? p.funding_auto_exit_spread_pct),
    fundingLiqTrimPct: optThreshold(p.fundingLiqTrimPct ?? p.funding_liq_trim_pct),
    fundingAutoRebalance: (p.fundingAutoRebalance ?? p.funding_auto_rebalance) === true,
    fundingRotationThresholdBps: optThreshold(p.fundingRotationThresholdBps ?? p.funding_rotation_threshold_bps),
    fundingEnableBasis: (p.fundingEnableBasis ?? p.funding_enable_basis) !== false,
    fundingMinBasisBps: optThreshold(p.fundingMinBasisBps ?? p.funding_min_basis_bps),
    futuresLiqTrimPct: optThreshold(p.futuresLiqTrimPct ?? p.futures_liq_trim_pct),
    spotLiqTrimPct: optThreshold(p.spotLiqTrimPct ?? p.spot_liq_trim_pct),
    predictionLiqTrimPct: optThreshold(p.predictionLiqTrimPct ?? p.prediction_liq_trim_pct),
    spotTradeSizeUsd: num(p.spotTradeSizeUsd ?? p.spot_trade_size_usd, 1000),
    spotMaxSlippageBps: num(p.spotMaxSlippageBps ?? p.spot_max_slippage_bps, 5.0),
    spotMinNetSpreadBps: num(p.spotMinNetSpreadBps ?? p.spot_min_net_spread_bps, 15.0),
    futuresPassiveUnwind: normalizePassive(p.futuresPassiveUnwind ?? p.futures_passive_unwind),
    fundingPassiveUnwind: normalizePassive(p.fundingPassiveUnwind ?? p.funding_passive_unwind),
    spotPassiveUnwind: normalizePassive(p.spotPassiveUnwind ?? p.spot_passive_unwind),
  };
}

export function normalizeStrategyTicker(r: any): StrategyTickerDto {
  return {
    base: String(r?.base ?? r?.symbol ?? "").toUpperCase(),
    venues: Array.isArray(r?.venues) ? r.venues.filter((v: unknown) => typeof v === "string") : [],
    passes: r?.passes !== false,
    blockReason: typeof (r?.blockReason ?? r?.block_reason) === "string"
      ? (r.blockReason ?? r.block_reason)
      : null,
  };
}

export function normalizeEquityEvent(e: any): AccountEquityEventDto {
  return {
    id: Number(e?.id ?? 0),
    accountId: String(e?.accountId ?? e?.account_id ?? ""),
    tsMs: Number(e?.tsMs ?? e?.ts_ms ?? 0),
    expected: Number(e?.expected ?? 0),
    actual: Number(e?.actual ?? 0),
    delta: Number(e?.delta ?? 0),
    kind: String(e?.kind ?? "unknown"),
    note: typeof e?.note === "string" ? e.note : "",
  };
}

export const AUTOTUNE_PARAMS = [  "slidingWindowSecs",
  "targetProfitBps",
  "windowPercentile",
  "stepSizeMult",
  "maxStepUsd",
  "maxTickerEquityPct",
  "userTargetLeverage",
] as const;

export function normalizeAutotuneRecommendation(r: any, symbol: string): AutotuneRecommendationDto {
  const num = (v: unknown, fb: number): number => {
    const n = Number(v);
    return Number.isFinite(n) ? n : fb;
  };
  const rows = Array.isArray(r?.rows) ? r.rows : [];
  const byParam = new Map<string, any>(rows.map((row: any) => [row?.param, row]));
  const normRows = AUTOTUNE_PARAMS.map((param) => {
    const row: any = byParam.get(param) ?? {};
    const rawRec = Number(row.recommended);
    // READY with a non-numeric recommendation is a backend bug — demote to
    // HELD rather than render a fabricated row.
    const status: "READY" | "HELD" =
      row.status === "READY" && Number.isFinite(rawRec) ? "READY" : "HELD";
    const recommended = status === "READY" ? rawRec : null;
    return {
      param,
      current: num(row.current, 0),
      recommended,
      status,
      fit_metric: typeof row.fit_metric === "string" ? row.fit_metric : "n/a",
      validated_metric:
        typeof row.validated_metric === "string" ? row.validated_metric : "n/a",
      n: Math.max(0, Math.floor(num(row.n, 0))),
      rationale: typeof row.rationale === "string" ? row.rationale : "held",
      formula_id: typeof row.formula_id === "string" ? row.formula_id : param,
      confidence: typeof row.confidence === "string" ? row.confidence : "low",
    };
  });
  const banner = r?.banner ?? {};
  const readyCount = normRows.filter((row) => row.status === "READY").length;
  const scale = (v: unknown): number => {
    const n = Number(v);
    return Number.isFinite(n) && n > 0 ? n : 1.0;
  };
  return {
    symbol: typeof r?.symbol === "string" ? r.symbol : symbol,
    buy_venue: typeof r?.buy_venue === "string" ? r.buy_venue : null,
    sell_venue: typeof r?.sell_venue === "string" ? r.sell_venue : null,
    pair_observations: Math.max(0, Math.floor(num(r?.pair_observations, 0))),
    rows: normRows,
    banner: {
      fit_from_ms: Math.floor(num(banner.fit_from_ms, 0)),
      fit_to_ms: Math.floor(num(banner.fit_to_ms, 0)),
      validate_from_ms: Math.floor(num(banner.validate_from_ms, 0)),
      validate_to_ms: Math.floor(num(banner.validate_to_ms, 0)),
      regime_verdict:
        typeof banner.regime_verdict === "string" ? banner.regime_verdict : "INSUFFICIENT",
      ready_count: readyCount,
      total: normRows.length,
    },
    session_tau_scale: scale(r?.session_tau_scale ?? r?.sessionTauScale),
    session_sigma_scale: scale(r?.session_sigma_scale ?? r?.sessionSigmaScale),
  };
}

export function normalizeSessionProfile(p: any, symbol: string): SessionProfileDto {
  const sessions = Array.isArray(p?.sessions) ? p.sessions : [];
  const promo = p?.promotion;
  return {
    symbol: typeof p?.symbol === "string" ? p.symbol : symbol,
    currentSession: typeof p?.currentSession === "string" ? p.currentSession : "dead",
    isWeekend: p?.isWeekend === true,
    inFundingMask: p?.inFundingMask === true,
    usCashOpen: p?.usCashOpen === true,
    sessionAllow: Number.isFinite(Number(p?.sessionAllow)) ? Number(p.sessionAllow) : 1.0,
    provisional: p?.provisional !== false,
    sessions: sessions.map((s: any) => ({
      sessionId: typeof s?.sessionId === "string" ? s.sessionId : "?",
      nEpisodes: Math.max(0, Math.floor(Number(s?.nEpisodes) || 0)),
      medAbsSpreadBps: Number.isFinite(Number(s?.medAbsSpreadBps)) ? Number(s.medAbsSpreadBps) : null,
      p90SpreadBps: Number.isFinite(Number(s?.p90SpreadBps)) ? Number(s.p90SpreadBps) : null,
      rvBps: Number.isFinite(Number(s?.rvBps)) ? Number(s.rvBps) : null,
      winRate: Number.isFinite(Number(s?.winRate)) ? Number(s.winRate) : null,
      meanHoldS: Number.isFinite(Number(s?.meanHoldS)) ? Number(s.meanHoldS) : null,
      provisional: s?.provisional !== false,
    })),
    promotion:
      promo != null && typeof promo === "object"
        ? {
            status: typeof promo.status === "string" ? promo.status : "held",
            fitDeltaBps: Number.isFinite(Number(promo.fitDeltaBps)) ? Number(promo.fitDeltaBps) : 0,
            valDeltaBps: Number.isFinite(Number(promo.valDeltaBps)) ? Number(promo.valDeltaBps) : 0,
            reason: typeof promo.reason === "string" ? promo.reason : "",
            harR2: Number.isFinite(Number(promo.harR2)) ? Number(promo.harR2) : null,
            rescaled: promo.rescaled === true,
          }
        : null,
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
    return invokeDesktop<TriangleOpportunityDto[]>("scan_triangle_opportunities", {
      filterAsset: filterAsset ?? "",
    });
  },

  async executeTriangleArbitrage(params: TriangleExecutionParams): Promise<SimStepResultDto> {
    return invokeDesktop<SimStepResultDto>("execute_triangle_arbitrage", { params });
  },

  async checkSpatialConvergence(
    symbol: string | null,
    strategyId: string,
  ): Promise<ConvergenceCheckDto[]> {
    return invokeDesktop<ConvergenceCheckDto[]>("check_spatial_convergence", {
      symbol,
      strategyId,
    });
  },

  async scanFundingOpportunities(
    symbolOrSymbols?: string | string[],
    tradeSizeUsd: number = 2000
  ): Promise<FundingOpportunityDto[]> {
    if (Array.isArray(symbolOrSymbols)) {
      return invokeDesktop("scan_funding_opportunities", {
        symbols: symbolOrSymbols,
        tradeSizeUsd,
      });
    }
    return invokeDesktop("scan_funding_opportunities", {
      symbol: symbolOrSymbols ?? "",
      tradeSizeUsd,
    });
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

  async getExecutionLogs(isLive?: boolean): Promise<ExecutionLogEntry[]> {
    return invokeDesktop<ExecutionLogEntry[]>("get_execution_logs", isLive !== undefined ? { isLive } : {});
  },

  async getVenueExecutionCapabilities(): Promise<VenueCapabilityDto[]> {
    return invokeDesktop<VenueCapabilityDto[]>("get_venue_execution_capabilities");
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
    const res = await invokeDesktop<StrategyParamsDto>("get_strategy_params", { strategyId });
    return normalizeStrategyParams(res);
  },

  async saveStrategyParams(params: StrategyParamsDto): Promise<void> {
    return invokeDesktop<void>("save_strategy_params", { params });
  },

  async getAllStrategyParams(): Promise<StrategyParamsDto[]> {
    const res = await invokeDesktop<StrategyParamsDto[]>("get_all_strategy_params");
    return (res || []).map(normalizeStrategyParams);
  },

  async getAutotuneRecommendation(symbol: string): Promise<AutotuneRecommendationDto> {
    const res = await invokeDesktop<AutotuneRecommendationDto>("get_autotune_recommendation", {
      symbol,
    });
    return normalizeAutotuneRecommendation(res, symbol);
  },

  async applyAutotuneSelected(symbol: string, selected: string[]): Promise<AutotuneApplyResultDto> {
    const res = await invokeDesktop<AutotuneApplyResultDto>("apply_autotune_selected", {
      symbol,
      selected,
    });
    const applied = Array.isArray(res?.applied) ? res.applied.filter((p) => typeof p === "string") : [];
    const skipped = Array.isArray(res?.skipped_held)
      ? res.skipped_held.filter((p) => typeof p === "string")
      : [];
    return {
      symbol: typeof res?.symbol === "string" ? res.symbol : symbol,
      applied,
      skipped_held: skipped,
      audit_id: Number.isFinite(Number(res?.audit_id)) ? Number(res.audit_id) : null,
    };
  },

  async runSeasonalityPromotion(): Promise<SeasonalityRunSummaryDto> {
    const res = await invokeDesktop<Record<string, number | null | undefined>>(
      "run_seasonality_promotion",
    );
    const num = (v: unknown): number => {
      const n = Number(v);
      return Number.isFinite(n) ? n : 0;
    };
    return {
      symbolsScanned: Math.max(0, Math.floor(num(res?.symbolsScanned ?? res?.symbols_scanned))),
      anchorsWritten: Math.max(0, Math.floor(num(res?.anchorsWritten ?? res?.anchors_written))),
      promoted: Math.max(0, Math.floor(num(res?.promoted))),
      demoted: Math.max(0, Math.floor(num(res?.demoted))),
      held: Math.max(0, Math.floor(num(res?.held))),
      skipped: Math.max(0, Math.floor(num(res?.skipped))),
      runTsMs: Math.max(0, Math.floor(num(res?.runTsMs ?? res?.run_ts_ms))),
    };
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
  /** Field-scoped reveal: fetches ONE secret field on explicit user reveal,
   *  so the full credential bundle never sits in renderer memory. */
  async getAccountCredentialField(accountId: string, field: string): Promise<string> {
    return invokeDesktop<string>("get_account_credential_field", { accountId, field });
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
    strategyId: string,
    executionType?: "emergency_market" | "non_emergency_limit"
  ): Promise<FlattenResultDto> {
    return invokeDesktop<FlattenResultDto>("flatten_ticker_positions", {
      symbol,
      strategyId,
      executionType: executionType ?? null,
    });
  },

  async getVenueBalances(): Promise<VenueBalanceInfo[]> {
    return invokeDesktop<VenueBalanceInfo[]>("get_venue_balances");
  },

  async getStrategyPerformance(strategyId?: string): Promise<StrategyMetrics[]> {
    return invokeDesktop<StrategyMetrics[]>("get_strategy_performance", { strategyId });
  },

  async getStrategyFillPnl(strategyId: string): Promise<StrategyFillPnl> {
    return invokeDesktop<StrategyFillPnl>("get_strategy_fill_pnl", { strategyId });
  },

  async getExternalPositions(): Promise<ExternalPositionsResponse> {
    return invokeDesktop<ExternalPositionsResponse>("get_external_positions");
  },
  async recommendExternalClose(venue: string, symbol: string, takerFeeBps?: number): Promise<CloseRecommendationDto> {
    return invokeDesktop<CloseRecommendationDto>("recommend_external_close", { venue, symbol, takerFeeBps: takerFeeBps ?? null });
  },
  async closeExternalPosition(input: { venue: string; symbol: string; mode: string; limitPrice?: number | null }): Promise<CloseResultDto> {
    return invokeDesktop<CloseResultDto>("close_external_position", { input });
  },
  async getUtilisationHistory(venue: string, windowHours?: number): Promise<UtilisationHistory> {
    return invokeDesktop<UtilisationHistory>("get_utilisation_history", {
      venue,
      windowHours: windowHours ?? null,
    });
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

  async getStrategyTickers(strategyId: string): Promise<StrategyTickerDto[]> {
    const res = await invokeDesktop<any[]>("get_strategy_tickers", { strategyId });
    return (res || []).map(normalizeStrategyTicker);
  },

  async getAccountEquityEvents(accountId?: string, limit?: number): Promise<AccountEquityEventDto[]> {
    const res = await invokeDesktop<any[]>("get_account_equity_events", {
      accountId: accountId ?? null,
      limit: limit ?? null,
    });
    return (res || []).map(normalizeEquityEvent);
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

  // ---- Autonomous convergence bot (opt-in, paper-only) ----
  async getRadarBotConfig(): Promise<RadarBotConfigDto> {
    return invokeDesktop<RadarBotConfigDto>("get_radar_bot_config");
  },

  async setRadarBotConfig(config: RadarBotConfigDto): Promise<void> {
    return invokeDesktop<void>("set_radar_bot_config", { config });
  },

  async getRadarBotStatus(): Promise<RadarBotStatusDto> {
    return invokeDesktop<RadarBotStatusDto>("get_radar_bot_status");
  },

  // ---- Session seasonality (Phase 1, display only) ----
  async getSessionProfile(symbol: string): Promise<SessionProfileDto> {
    const res = await invokeDesktop<SessionProfileDto>("get_session_profile", { symbol });
    return normalizeSessionProfile(res, symbol);
  },
};


