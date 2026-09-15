export interface SystemStatusDto {
  is_live_mode: boolean;
  is_paused?: boolean;
  total_realized_pnl: number;
  total_trades_count: number;
  successful_trades_count: number;
  circuit_breaker_status: string;
  circuit_breaker_tripped: boolean;
  current_drawdown_usd: number;
  max_drawdown_usd: number;
  consecutive_failures: number;
  active_venues: VenueStatusDto[];
}

export interface VenueStatusDto {
  id: string;
  name: string;
  market_type: string;
  status: string;
  latency_ms: number | null;
  fee_bps: number;
}

export interface VenueVolumesDto {
  venue_id: string;
  /** 24h notional volume (USD) keyed by the venue's raw asset name. */
  volumes: Record<string, number>;
}

/** Backend-computed dislocation series (arbitrager-math) over persisted
 * spread samples. Deviations are per-venue bps from the selected baseline;
 * null entries are genuine gaps, never interpolated. */
export interface SignalSeriesDto {
  symbol: string;
  /** "consensus" | "leader" */
  mode: string;
  window_secs: number;
  /** Total stored venue samples inside the window. */
  sample_count: number;
  /** Earliest sample ever recorded for this symbol (history depth). */
  earliest_ts_ms: number | null;
  /** Bucket timestamps (epoch ms), ascending. */
  ts_ms: number[];
  /** Sorted venue ids; dev_bps[i][j] is venues[j] at ts_ms[i]. */
  venues: string[];
  /** venue_id -> display name. */
  venue_names: Record<string, string>;
  dev_bps: (number | null)[][];
  /** Max/min deviation across present venues per bucket — the dislocation
   * band; their difference is the gross pairwise spread. */
  band_max_bps: number[];
  band_min_bps: number[];
}

export interface VenueOrderBookDto {
  venue_id: string;
  venue_name: string;
  symbol: string;
  best_bid: number;
  best_ask: number;
  spread: number;
  spread_bps: number;
  mid_price: number;
  total_bid_depth: number;
  total_ask_depth: number;
  vwap_bid: number;
  vwap_ask: number;
  funding_rate_8h_pct: number;
  bids: [number, number][]; // [price, size]
  asks: [number, number][]; // [price, size]
}

export interface SpatialOpportunityDto {
  id: string;
  symbol: string;
  buy_venue: string;
  sell_venue: string;
  buy_price: number;
  sell_price: number;
  gross_spread: number;
  gross_spread_bps: number;
  fee_bps: number;
  net_spread_bps: number;
  order_qty: number;
  estimated_profit_usd: number;
  is_executable: boolean;
  buy_venue_id: string;
  sell_venue_id: string;
  buy_fee_bps: number;
  sell_fee_bps: number;
  round_trip_commission_bps: number;
  entry_slippage_bps: number;
  exit_slippage_bps: number;
  closing_spread_bps: number;
  expected_friction_bps: number;
  required_gross_bps: number;
  slippage_ok: boolean;
  depth_ok: boolean;
  direction?: string;
  trigger_spread_bps?: number;
  baseline_mean_spread_bps?: number;
  position_step?: number;
  max_units?: number;
  nominal_hurdle_bps?: number;
}

export type MatrixMode = "cross-venue" | "cross-ticker";
/** Legacy canonical duration string ("10s","30m","24h","7d"); arbitrary
 *  integer s|m|h|d values are valid — the fixed presets are gone. */
export type SlidingWindowOption = string;
export type ArbitrageExecutionMode = "SINGLE_WAY" | "DOUBLE_WAY";

export interface TickerStrategyConfigDto {
  symbol: string;
  targetProfitBps: number; // Target profit in BPS
  target_profit_bps?: number;
  windowPercentile: number; // Percentile of historical sliding window (50 - 99%)
  window_percentile?: number;
  maxAccumulatedUnits: number; // Maximum accumulated grid units
  max_accumulated_units?: number;
  currentAccumulatedUnits: number; // Currently filled units
  current_accumulated_units?: number;
  executionMode: ArbitrageExecutionMode; // Single-way (divergence) or Double-way (divergence + convergence)
  execution_mode?: string;
  unitSize: number; // Size per unit in base asset units
  unit_size?: number;
  userTargetLeverage: number; // User target leverage ratio (e.g. 10)
  user_target_leverage?: number;
  slidingWindow?: SlidingWindowOption; // Rolling sliding window (e.g. 1m)
  sliding_window?: string;
}

/** Universal strategy parameters — one record per strategyId ("futures" |
 * "funding" | "spot"), shared across all tickers. Per-ticker contract/runtime
 * state stays in TickerStrategyConfigDto. */
export interface StrategyParamsDto {
  strategyId: string;
  // Single Spread (futures)
  targetProfitBps: number;
  windowPercentile: number;
  slidingWindow: string;
  slidingWindowMode: "manual" | "auto";
  slidingWindowSecs: number;
  maxAccumulatedUnits: number;
  executionMode: ArbitrageExecutionMode;
  unitSizeUsd: number; // USD notional per grid unit per leg — converted to base units per ticker
  userTargetLeverage: number;
  // Funding Rate
  fundingTradeSizeUsd: number;
  fundingMinAprPct: number;
  fundingHoldingDays: number;
  // Spatial (spot)
  spotTradeSizeUsd: number;
  spotMaxSlippageBps: number;
  spotMinNetSpreadBps: number;
}

export interface MatrixCellDto {
  rowId: string;
  colId: string;
  rowLabel: string;
  colLabel: string;
  isDiagonal: boolean;
  longPrice: number;
  shortPrice: number;
  grossSpreadBps: number;
  feeBps: number;
  closingFrictionBps: number;
  netSpreadBps: number;
  baselineMeanBps: number;
  deltaBps: number;
  sampleCount: number;
  baselineReady?: boolean;
  orderQty: number;
  estProfitUsd: number;
  isExecutable: boolean;
  costReady?: boolean;
  roundTripCommissionBps?: number;
  entrySlippageBps?: number;
  exitSlippageBps?: number;
  expectedFrictionBps?: number;
  requiredGrossBps?: number;
  slippageOk?: boolean;
  depthOk?: boolean;
  nominalHurdleBps?: number;
  targetProfitBps?: number;
  percentileHurdleBps?: number;
  isConvergenceUnwind?: boolean;
  longVenue: string;
  shortVenue: string;
  longSymbol: string;
  shortSymbol: string;
}

export interface CrossTickerExecutionParams {
  longSymbol: string;
  shortSymbol: string;
  longVenue: string;
  shortVenue: string;
  orderQty: number;
  estProfitUsd: number;
  netSpreadBps: number;
}

export interface VenueMarketUniverseDto {
  venue_id: string;
  symbols: string[];
}

export interface PairBaselineDto {
  buy_venue: string;
  sell_venue: string;
  mean_gross_bps: number | null;
  percentile_gross_bps: number | null;
  sample_count: number;
  history_span_secs: number;
}

export interface MatrixBaselineSnapshotDto {
  symbol: string;
  mode: string;
  manual_window_secs: number;
  effective_window_secs: number;
  auto_window_secs: number | null;
  median_half_life_secs: number | null;
  qualifying_pair_count: number;
  max_history_span_secs: number;
  using_manual_fallback: boolean;
  pairs: PairBaselineDto[];
}

export interface TickerMarketQuoteDto {
  symbol: string;
  base: string;
  best_bid: number;
  best_ask: number;
  mid_price: number;
  spread_bps: number;
  best_buy_venue: string;
  best_sell_venue: string;
  fee_bps: number;
  base_price: number;
}

export interface FundingOpportunityDto {
  id: string;
  symbol: string;
  long_venue: string;
  short_venue: string;
  net_rate_8h_pct: number;
  net_apr_pct: number;
  net_apy_pct: number;
  round_trip_fee_bps: number;
  break_even_hours: number;
  estimated_daily_income_usd: number;
  trade_size_usd: number;
}

export interface PredictionOpportunityDto {
  id: string;
  market_title: string;
  venue_a: string;
  venue_b: string;
  strategy: string;
  buy_yes_venue: string;
  buy_no_venue: string;
  yes_price_cents: number;
  no_price_cents: number;
  total_cost_cents: number;
  guaranteed_payout_cents: number;
  net_profit_cents: number;
  roi_pct: number;
  has_arbitrage: boolean;
}

export interface StatArbStepResultDto {
  step: number;
  price_y: number;
  price_x: number;
  dynamic_hedge_ratio: number;
  intercept_alpha: number;
  residual_spread: number;
  spread_variance: number;
  z_score: number;
  signal: "ENTRY_LONG_SPREAD" | "ENTRY_SHORT_SPREAD" | "EXIT" | "NEUTRAL";
}

export interface BertramThresholdDto {
  entry_long: number;
  exit_long: number;
  entry_short: number;
  exit_short: number;
  optimal_distance: number;
  expected_return_per_cycle: number;
  estimated_trades_per_year: number;
  estimated_sharpe: number;
  half_life: number;
  theta: number;
  mu: number;
  sigma: number;
}

export interface OuFitDto {
  theta: number;
  mu: number;
  sigma: number;
  half_life: number;
  stationary_std: number;
  r_squared: number;
}

export interface RiskOverviewDto {
  circuit_breaker_state: string;
  is_halted: boolean;
  current_drawdown_usd: number;
  max_drawdown_limit_usd: number;
  drawdown_pct: number;
  consecutive_failures: number;
  max_failures_allowed: number;
  total_portfolio_value_usd: number;
  net_deltas: NetDeltaDto[];
  margin_health: MarginHealthDto;
}

export interface NetDeltaDto {
  asset: string;
  net_quantity: number;
  net_exposure_usd: number;
  delta_limit_usd: number;
  is_neutral: boolean;
}

export interface MarginHealthDto {
  maintenance_margin_req_usd: number;
  margin_balance_usd: number;
  margin_ratio_pct: number;
  liquidation_distance_pct: number;
  status: string;
}

export interface SimStepResultDto {
  cycle: number;
  opportunity_found: boolean;
  execution_success: boolean;
  trade_type: string;
  direction: string;
  venue_a: string;
  venue_b: string;
  spread_bps: number;
  realized_profit_usd: number;
  total_cumulative_pnl_usd: number;
  message: string;
}

export interface ExecutionLogEntry {
  id: string;
  timestamp: string;
  trade_type: string;
  symbol: string;
  venue_a: string;
  venue_b: string;
  direction?: string; // e.g. "Long Hyperliquid / Short Lighter"
  trigger_spread_bps?: number; // Instantaneous spread when entering/triggering
  baseline_mean_spread_bps?: number; // Average spread at that time (entry signal)
  position_step?: number; // Position step number (e.g. 1, 2, 3)
  max_units?: number; // Maximum accumulated grid units
  nominal_hurdle_bps?: number; // Nominal hurdle threshold at trigger time
  leg_a_status: string;
  leg_b_status: string;
  success: boolean;
  gross_profit: number;
  fees: number;
  net_profit: number;
  latency_us: number;
  note: string;
}

export interface EngineSettingsDto {
  // Remote HSM & Key Management
  hsm_url: string;
  hmac_token: string;

  // Smart Order Routing & Execution Guardrails
  max_slippage_bps: number;
  min_depth_ratio: number;

  // Exchange Enablement Toggles
  hyperliquid_enabled?: boolean;
  lighter_enabled?: boolean;
  vanta_enabled?: boolean;
  robinhood_enabled?: boolean;
  arcus_enabled?: boolean;
  popdex_enabled?: boolean;
  kraken_enabled?: boolean;
  kraken_spot_enabled?: boolean;
  kraken_futures_enabled?: boolean;
  polymarket_enabled?: boolean;
  kalshi_enabled?: boolean;
  decibel_enabled?: boolean;
  ibkr_enabled?: boolean;
  ig_enabled?: boolean;
  binance_enabled?: boolean;
  okx_enabled?: boolean;
  aster_enabled?: boolean;
  variational_enabled?: boolean;
  coinbase_enabled?: boolean;

  // Kraken Spot & Futures
  kraken_spot_api_key?: string;
  kraken_spot_secret?: string;
  kraken_spot_url?: string;
  kraken_futures_api_key?: string;
  kraken_futures_secret?: string;
  kraken_futures_url?: string;

  // Hyperliquid L1 Perp DEX
  hyperliquid_wallet: string;
  hyperliquid_private_key: string;
  hyperliquid_url: string;

  // Lighter zkDEX Rollup
  lighter_account_index: string;
  lighter_api_key: string;
  lighter_url: string;

  // Vanta.exchange (Orderly Network)
  vanta_account_id: string;
  vanta_orderly_key: string;
  vanta_orderly_secret: string;
  vanta_url: string;

  // Robinhood EVM
  robinhood_wallet: string;
  robinhood_private_key: string;
  robinhood_rpc_url: string;
  robinhood_router: string;

  // Arcus CLOB DEX
  arcus_api_key?: string;
  arcus_secret?: string;
  arcus_url?: string;

  // PopDEX (Arbitrage Protocol)
  popdex_account_index?: string;
  popdex_api_key?: string;
  popdex_url?: string;

  // Polymarket CTF CLOB
  polymarket_api_key: string;
  polymarket_secret: string;
  polymarket_passphrase: string;
  polymarket_url: string;

  // Kalshi CFTC Binary
  kalshi_api_key_id: string;
  kalshi_private_key: string;
  kalshi_url: string;

  // Networking & Port Settings
  engine_rpc_port: number;
  engine_ws_port: number;
  bind_address: string;

  // Execution & Warm-up Modes
  trading_mode: "paper" | "real";
  paper_starting_balance_usd?: number;
  fast_warmup_enabled: boolean;

  // Subaccount mappings
  hyperliquid_account_index?: number;
  hyperliquid_subaccount_index?: number;
  lighter_subaccount_index?: number;
  vanta_subaccount_index?: number;
  robinhood_subaccount_index?: number;

  // Vault-derived credential configuration state (venue IDs only, no secrets)
  configured_venues: string[];

  // Referrals, About & Sponsor Links
  referral_lighter: string;
  referral_robinhood: string;
  sponsor_link: string;
  version: string;
  git_repo: string;
}

export interface VaultStatus {
  is_initialized: boolean;
  is_unlocked: boolean;
  unlocked_until?: string | null;
}

export interface VenueBalanceInfo {
  venue: string;
  equity_usd: number;
  available_margin_usd: number;
  margin_rate_pct: number;
  status: "HEALTHY" | "LOW_MARGIN" | "DEPLETED" | string;
  recommended_action?: string | null;
}

// Multi-account model: one durable account profile per exchange login.
// Credential values are never returned over IPC; only presence flags.
export interface AccountDto {
  id: string;
  venue: string;
  label: string;
  enabled: boolean;
  isDefault: boolean;
  createdAt: string;
  updatedAt: string;
  /** null while the vault is locked (presence is unknowable). */
  credentialsConfigured: boolean | null;
  hasApiKey: boolean;
  hasApiSecret: boolean;
  hasPassphrase: boolean;
  hasAccountId: boolean;
  hasWalletAddress: boolean;
  hasPrivateKey: boolean;
  accountIndex: number | null;
  subaccountIndex: number | null;
  apiKeyIndex: number | null;
  /** "standard" | "plus" | "premium" | "builder" — drives the venue's
   *  shared-IP rate budget and Lighter Plus fee adjustment. */
  accountTier: string;
}

export interface AccountCredentialInput {
  apiKey?: string;
  apiSecret?: string;
  passphrase?: string | null;
  accountId?: string | null;
  walletAddress?: string | null;
  privateKey?: string | null;
  accountIndex?: number | null;
  subaccountIndex?: number | null;
  apiKeyIndex?: number | null;
}

export interface AccountUpsertInput {
  /** Existing account ID for updates; omit to create. */
  id?: string | null;
  /** Venue ID; required on create, immutable on update. */
  venue?: string;
  label?: string;
  enabled?: boolean;
  makeDefault?: boolean;
  /** Omit to preserve the stored tier on update (standard on create). */
  accountTier?: string | null;
  credentials?: AccountCredentialInput | null;
}

/** Strategy-level route: maps an enabled account on one exchange to an
 *  enabled account on another. Applies to every ticker using the strategy. */
export interface StrategyAccountRouteDto {
  id: string;
  strategyId: string;
  label: string;
  enabled: boolean;
  accountAId: string;
  accountAVenue: string;
  accountALabel: string;
  accountBId: string;
  accountBVenue: string;
  accountBLabel: string;
  createdAt: string;
  updatedAt: string;
}

export interface StrategyRouteUpsertInput {
  /** Existing route ID for updates; omit to create. */
  id?: string | null;
  strategyId: string;
  label?: string;
  accountAId: string;
  accountBId: string;
  enabled?: boolean;
}

/** One venue leg of a signal execution, with account attribution. */
export interface SignalLegDto {
  id: string;
  signalId: string;
  accountId: string | null;
  venue: string;
  side: "long" | "short";
  intendedQty: number;
  filledQty: number;
  fillPrice: number | null;
  feeUsd: number;
  venueOrderId: string | null;
  createdAt: string;
  updatedAt: string;
}

/** Durable per-signal execution record. `in_flight`, `imbalanced`, and
 *  `unresolved` block further orders on the symbol until the signal's own
 *  legs reconcile as delta-neutral. */
export interface SignalExecutionDto {
  id: string;
  strategyId: string;
  symbol: string;
  status: "in_flight" | "balanced" | "imbalanced" | "unresolved" | "closed" | "failed";
  routeId: string | null;
  accountLongId: string | null;
  accountShortId: string | null;
  expectedNetUsd: number | null;
  feesUsd: number;
  note: string | null;
  legs: SignalLegDto[];
  createdAt: string;
  updatedAt: string;
  closedAt: string | null;
}

/** One venue's signed net filled quantity inside an aggregated position.
 *  Positive = net long (buy side), negative = net short (sell side). */
export interface PositionVenueDto {
  venue: string;
  netQty: number;
}

/** Aggregated current open position for one signal-type identity
 *  (strategy + symbol/event + venue set), derived from all signal
 *  executions that still count as open. `unrealizedPnlUsd` is null when
 *  mark-to-market is impossible — never fabricated. */
export interface OpenPositionDto {
  /** Stable grouping identity: `strategyId|symbol|venueA+venueB`. */
  key: string;
  strategyId: string;
  symbol: string;
  signalCount: number;
  /** Per-venue signed net quantity; long (positive) rows first. */
  venues: PositionVenueDto[];
  expectedNetUsd: number | null;
  unrealizedPnlUsd: number | null;
  feesUsd: number;
  notionalUsd: number | null;
  openedAt: string;
}

/** One candidate in a synchronized batch — per symbol only the highest
 *  expected-profit candidate dispatches. */
export interface SpatialCandidateInput {
  symbol: string;
  buyVenue: string;
  sellVenue: string;
  orderQty: number;
  expectedNetUsd?: number | null;
  /** Explicit strategy-level account route override. */
  routeId?: string | null;
  direction?: string | null;
}

export interface CandidateOutcomeDto {
  symbol: string;
  buyVenue: string;
  sellVenue: string;
  expectedNetUsd: number | null;
  selected: boolean;
  reason: string | null;
  result: SimStepResultDto | null;
}

export interface TickerPositionState {
  symbol: string;
  blocked: boolean;
  blockingSignalIds: string[];
  netPositionQty: number;
}

/** Result of an emergency per-strategy flatten (paper venues only). */
export interface FlattenResultDto {
  strategyId: string;
  symbol: string | null;
  signalsProcessed: number;
  signalsClosed: number;
  legsClosed: number;
  /** Signals still holding residual exposure — ticker remains blocked. */
  residualSignals: number;
  notes: string[];
}

export interface StrategyMetrics {
  strategy_id: string;
  total_cycles_initiated: number;
  total_cycles_completed: number;
  winning_cycles: number;
  losing_cycles: number;
  win_rate_pct: number;
  total_pnl_usd: number;
  avg_pnl_usd: number;
  avg_pnl_bps: number;
  max_drawdown_usd: number;
  max_drawdown_pct: number;
  sharpe_ratio: number;
  annualized_apr_pct: number;
  annualized_apy_pct: number;
}

export interface WfoEvaluation {
  estimator: string;
  window_size: number;
  in_sample_sharpe: number;
  out_of_sample_sharpe: number;
  tracking_error_bps: number;
  simulated_net_profit_bps: number;
  win_rate_pct: number;
  total_trades: number;
}

export interface WfoOptimizationResult {
  optimal_estimator: string;
  optimal_window_size: number;
  in_sample_score: number;
  out_of_sample_score: number;
  candidate_evaluations: WfoEvaluation[];
  recommendation_notes: string;
}
export interface SpotOpportunityDto {
  id: string;
  symbol: string;
  buy_venue: string;
  sell_venue: string;
  buy_price: number;
  sell_price: number;
  gross_spread_bps: number;
  buy_fee_bps: number;
  sell_fee_bps: number;
  transfer_cost_usd: number;
  net_spread_bps: number;
  net_profit_usd: number;
  available_depth_usd: number;
  estimated_latency_ms: number;
  is_executable: boolean;
}

export interface SpotExecutionParams {
  symbol: string;
  buy_venue: string;
  sell_venue: string;
  trade_size_usd: number;
  max_slippage_bps: number;
  net_spread_bps?: number;
}

export interface TriangleOpportunityDto {
  id: string;
  venue: string;
  path: string; // e.g. "USDC → BTC → ETH → USDC"
  base_asset: string;
  intermediate_a: string;
  intermediate_b: string;
  leg1: {
    symbol: string;
    action: "BUY" | "SELL";
    price: number;
    fee_bps: number;
  };
  leg2: {
    symbol: string;
    action: "BUY" | "SELL";
    price: number;
    fee_bps: number;
  };
  leg3: {
    symbol: string;
    action: "BUY" | "SELL";
    price: number;
    fee_bps: number;
  };
  gross_spread_bps: number;
  total_fee_bps: number;
  net_spread_bps: number;
  net_profit_usd: number;
  trade_size_usd: number;
  estimated_latency_ms: number;
  is_executable: boolean;
}

export interface TriangleExecutionParams {
  id: string;
  venue: string;
  path: string;
  trade_size_usd: number;
  min_profit_bps: number;
  net_spread_bps?: number;
}

// ---------------------------------------------------------------------------
// Spread Range Monitor (Opportunity Radar)
// ---------------------------------------------------------------------------

export type MonitorRowStatus =
  | "collecting"
  | "watching"
  | "signaled"
  | "qualified"
  | "stale";

export interface CapacityTierDto {
  sizeUsd: number;
  filledUsd: number;
  netBps: number | null;
  expectedNetBps: number | null;
  marginalPWin: number | null;
}

export interface MonitorRowDto {
  symbol: string;
  buyVenue: string;
  sellVenue: string;
  status: MonitorRowStatus;
  currentEdgeBps: number | null;
  baselineBps: number | null;
  deviationBps: number | null;
  netCaptureBps: number | null;
  pWin: number | null;
  expectedProfitBps: number | null;
  expectedProfitUsd: number | null;
  optimalSizeUsd: number | null;
  breakevenSizeUsd: number | null;
  capacityLadder: CapacityTierDto[];
  fundingCarryBps: number | null;
  fundingUnknown: boolean;
  firstSeenMs: number | null;
  lastSeenMs: number | null;
  signalAgeSecs: number | null;
  expectedConvergeSecs: number | null;
  qualificationProgress: number;
  observedEpisodes: number;
  executable: boolean;
  blockReason: string | null;
}

export interface VenueLaneStatusDto {
  venue: string;
  readBudgetPerMin: number;
  usedLastMin: number;
  symbols: number;
  refreshIntervalSecs: number;
  backingOff: boolean;
  accountTier: string;
}

export interface MonitorSnapshotDto {
  generatedTsMs: number;
  running: boolean;
  rows: MonitorRowDto[];
  watchlist: string[];
  venues: VenueLaneStatusDto[];
  exempted: string[];
}

export interface MonitorConfigDto {
  enabled: boolean;
  watchlistRefreshSecs: number;
  qualificationMode: "auto" | "manual";
  manualWindowSecs: number;
  halfLifeMultiplier: number;
  minWindowSecs: number;
  frictionCoverMult: number;
  capacityTiersUsd: number[];
  minWinProbability: number;
  executionMode: "one_way" | "two_way";
  participationCap: number;
  txReservePct: number;
  minEpisodes: number;
  historyWindowSecs: number;
  maxSymbolIntervalSecs: number;
  minSymbolIntervalSecs: number;
  minExpectedProfitBps: number;
}

export * from "./tickers";
