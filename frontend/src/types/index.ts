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

export interface VenueCapabilityDto {
  venue: string;
  name: string;
  supportsLive: boolean;
  hasCredentials: boolean;
  liveReady: boolean;
}

export interface VenueVolumesDto {
  venue_id: string;
  /** 24h notional volume (USD) keyed by the venue's raw asset name. */
  volumes: Record<string, number>;
}

/** Backend-computed dislocation series (arbitrageur-math) over persisted
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
  /** Empirical convergence probability (0-1) from the pair's episode
   *  history, scored to the monitor's standard. Null when history is thin
   *  — the Ranked Opportunities Win % column renders "—" until then. */
  win_probability?: number | null;
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

/** Passive limit unwind/rebalance controls (paper-only, fail-closed).
 * Peg `spreadPercentile` 0 = aggressive touch … 50 = mid … 100 = passive
 * edge; `timeoutSecs` bounds every limit attempt; `maxTries` bounds retries
 * before market fallback; the calm gate skips passive when the spread or
 * fast vol exceeds its cap. Prediction carries no instance (inert). */
export interface PassiveUnwindDto {
  enabled: boolean;
  spreadPercentile: number;
  timeoutSecs: number;
  maxTries: number;
  maxSpreadBps: number;
  maxFastVolBps: number;
}

export const DEFAULT_PASSIVE_UNWIND: PassiveUnwindDto = {
  enabled: false,
  spreadPercentile: 50,
  timeoutSecs: 10,
  maxTries: 3,
  maxSpreadBps: 15,
  maxFastVolBps: 10,
};
export interface StrategyParamsDto {
  strategyId: string;
  // Single Spread (futures)
  targetProfitBps: number;
  windowPercentile: number;
  slidingWindow: string;
  slidingWindowMode: "manual" | "auto";
  slidingWindowSecs: number;
  // Single Spread sizing: one grid step = min(mult x venue min lot,
  // maxStepUsd / price); max steps derive per ticker from paper equity x pct.
  stepSizeMult: number;
  maxStepUsd: number;
  maxTickerEquityPct: number;
  executionMode: ArbitrageExecutionMode;
  userTargetLeverage: number;
  // Per-strategy auto-hide filters (None/0 = off). MIN aggregation across
  // the ticker's in-scope venues; any missing venue excludes the ticker.
  autoMinVolumeUsd: number | null;
  autoMinPriceUsd: number | null;
  // Prediction event-volume threshold: defined but inert (no venue volume
  // data — prediction adapters are scaffolds).
  autoMinEventVolumeUsd: number | null;
  // Funding Rate
  fundingTradeSizeUsd: number;
  fundingMinAprPct: number;
  fundingHoldingDays: number;
  // Per-strategy max slippage (bps): each strategy enforces its own gate.
  // No global setting. Prediction carries the field for schema parity but
  // it is inert (no execution path — prediction adapters are scaffolds),
  // like autoMinEventVolumeUsd.
  futuresMaxSlippageBps: number;
  fundingMaxSlippageBps: number;
  predictionMaxSlippageBps: number;
  // Funding configurator: max allowed gap (bps) between venues' execution
  // prices at entry; auto-exit when live inter-venue spread (percent) drops
  // below threshold; trim both legs when either nears liquidation; optional
  // rebalance-to-neutral; rotation on net-of-friction improvement (bps).
  // Null = gate off.
  fundingMaxEntrySpreadBps: number;
  fundingAutoExitSpreadPct: number | null;
  fundingLiqTrimPct: number | null;
  fundingAutoRebalance: boolean;
  fundingRotationThresholdBps: number | null;
  // Same-exchange spot-vs-perp basis (cash-and-carry) switch + signed
  // min-basis floor in bps (null = no floor; APR still gates downstream).
  fundingEnableBasis: boolean;
  fundingMinBasisBps: number | null;
  // Universal trim block: every bot configurator surfaces the same
  // threshold (percent, null = off) with identical trim-both/close-both
  // semantics. Prediction is defined-but-disabled.
  futuresLiqTrimPct: number | null;
  spotLiqTrimPct: number | null;
  predictionLiqTrimPct: number | null;
  // Spatial (spot)
  spotTradeSizeUsd: number;
  spotMaxSlippageBps: number;
  spotMinNetSpreadBps: number;
  // Triangular Arbitrage
  triangleTradeSizeUsd?: number;
  triangleMinProfitBps?: number;
  triangleMaxSlippageBps?: number;
  // Passive limit unwinds: one control set per strategy with a live reduce
  // path. Optional in persisted rows (older saves) — callers must fall back
  // to DEFAULT_PASSIVE_UNWIND when absent.
  futuresPassiveUnwind?: PassiveUnwindDto;
  fundingPassiveUnwind?: PassiveUnwindDto;
  spotPassiveUnwind?: PassiveUnwindDto;
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

/** Auto-tune Phase 1 (recommend-only) preview DTOs. `recommended` is null
 * exactly when `status === "HELD"` — the manual value stays authoritative. */
export interface AutotuneRowDto {
  param: string;
  current: number;
  recommended: number | null;
  status: "READY" | "HELD";
  fit_metric: string;
  validated_metric: string;
  n: number;
  rationale: string;
  formula_id: string;
  confidence: string;
}

export interface AutotuneBannerDto {
  fit_from_ms: number;
  fit_to_ms: number;
  validate_from_ms: number;
  validate_to_ms: number;
  regime_verdict: "STABLE" | "SHIFT" | "INSUFFICIENT" | string;
  ready_count: number;
  total: number;
}

export interface AutotuneRecommendationDto {
  symbol: string;
  buy_venue: string | null;
  sell_venue: string | null;
  pair_observations: number;
  rows: AutotuneRowDto[];
  banner: AutotuneBannerDto;
  session_tau_scale: number;
  session_sigma_scale: number;
}

/** One-click apply result: only READY rows are ever written; HELD rows are
 * reported in skippedHeld and never touch the DTOs. */
export interface AutotuneApplyResultDto {
  symbol: string;
  applied: string[];
  skipped_held: string[];
  audit_id: number | null;
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
  /** Depth-supported executable size (absent on stale backends). */
  optimal_size_usd?: number;
  /** Measured entry cross as a positive cost in bps; negative = unmeasured. */
  entry_spread_bps?: number;
  /** Opportunity kind: "carry" (cross-exchange perp/perp) or "basis" (same-exchange spot/perp). Absent = carry (stale backends). */
  kind?: string;
  /** Signed live divergence (perp - spot) / spot * 10000 in bps. Only for basis opps. */
  basis_bps?: number;
  /** Spot reference price. Only for basis opps. */
  spot_price?: number;
  /** Perp reference price. Only for basis opps. */
  perp_price?: number;
  /** Informational win probability 0..1 (experimental fee-coverage heuristic). Absent = unusable edge. Never gates execution. */
  win_probability?: number;
  /** Marginal entry-cost slope in bps per extra $1k notional (both legs). Absent = book missing/thin. Informational only. */
  depth_slope_bps_per_1k?: number;
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
  /** Owning strategy; absent or "unattributed" rows predate attribution and
   *  are excluded from per-strategy sums, counted separately. */
  strategy_id?: string;
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
  is_live?: boolean;
}

export interface EngineSettingsDto {
  // Remote HSM & Key Management
  hsm_url: string;
  hmac_token: string;

  // Smart Order Routing & Execution Guardrails (no global slippage:
  // each strategy enforces its own per-strategy max_slippage_bps gate)
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
  ondoperps_enabled?: boolean;
  grvt_enabled?: boolean;
  edgex_enabled?: boolean;
  variational_enabled?: boolean;
  coinbase_enabled?: boolean;
  // Long-tail perp venues (extended_venues.rs, paper read-only, keyless).
  dydx_enabled?: boolean;
  paradex_enabled?: boolean;
  gmx_enabled?: boolean;
  apex_enabled?: boolean;
  extended_enabled?: boolean;
  derive_enabled?: boolean;
  standx_enabled?: boolean;
  helix_enabled?: boolean;
  aevo_enabled?: boolean;
  vest_enabled?: boolean;
  antarctic_enabled?: boolean;
  evedex_enabled?: boolean;
  bulk_enabled?: boolean;
  qfex_enabled?: boolean;
  ostium_enabled?: boolean;
  sodex_enabled?: boolean;
  alphax_enabled?: boolean;
  pacifica_enabled?: boolean;
  nado_enabled?: boolean;
  iotrader_enabled?: boolean;
  niza_enabled?: boolean;
  astros_enabled?: boolean;
  gatedex_enabled?: boolean;

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

  // Historical retention (None/null = unlimited; 0 normalizes to unlimited)
  spread_samples_max_days?: number | null;
  spread_samples_max_gb?: number | null;

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

// External (non-system) positions: live on-exchange holdings minus every
// filled leg the system opened itself. Closed through the bot as reduce-only
// live orders (Live Trading + unlocked vault + execution-capable venue).
export interface ExternalPositionDto {
  key: string;
  venue: string;
  symbol: string;
  base: string;
  side: "long" | "short";
  quantity: number;
  entryPrice: number;
  markPrice: number;
  notionalUsd: number;
  unrealizedPnlUsd: number;
  liquidationPrice: number | null;
  liquidationDistancePct: number | null;
  leverage: number;
  systemQty: number;
  externalQty: number;
  closeSupported: boolean;
  updatedAt: string;
}

export interface ExternalPositionsResponse {
  positions: ExternalPositionDto[];
  unreadable: string[];
  liveMode: boolean;
}

export interface CloseRecommendationDto {
  venue: string;
  symbol: string;
  closeSide: string;
  closeQty: number;
  touchPrice: number;
  touchSpreadBps: number;
  marketVwap: number;
  vwapSlipBps: number;
  depthFillableQty: number;
  fullSizeFillable: boolean;
  recommendedMode: string;
  recommendedLimitPrice: number | null;
  takerFeeBps: number;
  estFrictionBps: number;
  midVsMarkBps: number | null;
  bookAgeMs: number;
  warnings: string[];
}

export interface CloseResultDto {
  venue: string;
  symbol: string;
  mode: string;
  orderId: string | null;
  filledQty: number | null;
  avgFillPrice: number | null;
  status: string;
  note: string;
}

export interface VenueBalanceInfo {
  venue: string;
  equity_usd: number;
  available_margin_usd: number;
  margin_rate_pct: number;
  status: "HEALTHY" | "LOW_MARGIN" | "DEPLETED" | string;
  recommended_action?: string | null;
  is_live?: boolean;
  account_id?: string | null;
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
  /** Per-account leverage ceiling 1..=5; null = unlimited (backend hard
   *  ceiling of 5 still applies). Effective leverage is
   *  min(strategy, account, venue). */
  maxLeverage: number | null;
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
  /** Per-account leverage ceiling 1..=5; null = unlimited. Updates are
   *  authoritative: sending null clears a stored cap. Partial updaters must
   *  echo the current value (see AccountDto.maxLeverage). */
  maxLeverage?: number | null;
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

/** Attributed realized PnL for one strategy, from durable signal-leg fills
 *  (fees included). Unattributed rows are excluded from the sums and
 *  reported in the unattributed counters — never absorbed. */
export interface StrategyFillPnl {
  strategyId: string;
  netProfitUsd: number;
  netProfitBps: number | null;
  closedFamilies: number;
  openFamilies: number;
  unattributedClosedFamilies: number;
  unattributedLogEntries: number;
}

/** Time-weighted average utilisation for one venue over a trailing window.
 *  `partial` means snapshots cover less than the full window — the UI must
 *  label the shorter window honestly. */
export interface UtilisationHistory {
  venue: string;
  avgRatePct: number | null;
  coveredMs: number;
  windowMs: number;
  partial: boolean;
  pointCount: number;
}

/** Durable margin-utilisation observation for a paper venue. */
export interface MarginSnapshot {
  venue: string;
  accountId: string;
  tsMs: number;
  equity: number;
  usedMargin: number;
  ratePct: number;
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
  /** Grid-cap context for the backend execution gate. */
  positionStep?: number | null;
  maxUnits?: number | null;
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

/** One position-aware exit verdict from check_spatial_convergence. */
export interface ConvergenceCheckDto {
  signal_id: string;
  symbol: string;
  verdict: "hold" | "unwatched" | "exit" | string;
  trigger: string | null;
  reason: string;
  live_net_bps: number | null;
  entry_net_bps: number | null;
  unwound: boolean;
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
  thresholdRatio: number | null;
  coverage: number | null;
  /** Current UTC session ("asia" | "europe" | "overlap" | "us_late" | "dead"). */
  sessionId: string;
  /** True when the session aggregate is missing/provisional: all session
   * multipliers on the row are neutral x1.0, flagged here. */
  sessionProvisional: boolean;
  /** Session size-allowance factor (overlap 1.0 … dead 0.4, weekend x0.5,
   * funding mask 0.0). */
  sessionAllow: number;
  /** Combined session sizing multiplier applied to optimal/breakeven sizes. */
  sessionSizeMult: number;
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

/** One row of the get_strategy_tickers IPC: the ticker's in-scope venues,
 *  whether it passes the strategy's auto-filters, and the explicit block
 *  reason when it does not. Manual hiding applies after this auto-filter. */
export interface StrategyTickerDto {
  base: string;
  venues: string[];
  passes: boolean;
  blockReason: string | null;
}

/** One external-delta balance attribution event: actual equity disagreed
 *  with the fills-derived baseline, so the move was classified and the
 *  baseline rebased to actual. */
export interface AccountEquityEventDto {
  id: number;
  accountId: string;
  tsMs: number;
  expected: number;
  actual: number;
  delta: number;
  kind: "deposit" | "withdrawal" | "transfer" | "funding" | string;
  note: string;
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
  minThresholdRatio: number;
  minCoverage: number;
}

/** One shrunk per-session aggregate (get_session_profile row). Provisional
 * rows read as neutral x1.0 everywhere — never as tuned multipliers. */
export interface SessionProfileEntryDto {
  sessionId: string;
  nEpisodes: number;
  medAbsSpreadBps: number | null;
  p90SpreadBps: number | null;
  rvBps: number | null;
  winRate: number | null;
  meanHoldS: number | null;
  provisional: boolean;
}

/** Snapshot behind get_session_profile: per-session rows, the current
 * session, and the active multipliers. Display only in Phase 1. */
export interface SessionProfileDto {
  symbol: string;
  currentSession: string;
  isWeekend: boolean;
  inFundingMask: boolean;
  usCashOpen: boolean;
  sessionAllow: number;
  provisional: boolean;
  sessions: SessionProfileEntryDto[];
  promotion: SessionPromotionDto | null;
}

/** Walk-forward promotion verdict. Null (absent) means the Phase-1
 * baseline is served: profile RV is unscaled and nothing is promoted. */
export interface SessionPromotionDto {
  status: "promoted" | "held" | "demoted" | string;
  fitDeltaBps: number;
  valDeltaBps: number;
  reason: string;
  harR2: number | null;
  rescaled: boolean;
}

/** Shared nightly/manual seasonality run summary. */
export interface SeasonalityRunSummaryDto {
  symbolsScanned: number;
  anchorsWritten: number;
  promoted: number;
  demoted: number;
  held: number;
  skipped: number;
  runTsMs: number;
}

/** Autonomous convergence bot config (get/set_radar_bot_config). Opt-in:
 * entries only when enabled && mode === "auto". Ships inert. */
export interface RadarBotConfigDto {
  enabled: boolean;
  mode: "off" | "auto" | "paused" | "reduce_only";
  pollIntervalSecs: number;
  minPWin: number;
  minExpectedProfitBps: number;
  maxConcurrentPositions: number;
  maxNotionalPerPositionUsd: number;
  maxNotionalPerVenueUsd: number;
  maxNotionalPerSymbolUsd: number;
  pairCooldownSecs: number;
  holdTimeoutMult: number;
  stopMult: number;
  staleUnwindSecs: number;
}

export interface BotOpenPositionDto {
  symbol: string;
  buyVenue: string;
  sellVenue: string;
  entryTsMs: number;
  entryDevBps: number;
  baselineBps: number | null;
  sizeUsd: number;
  deadlineMs: number;
  liveDevBps: number | null;
  liveStatus: string | null;
  unwatched: boolean;
}

export interface BotDecisionDto {
  tsMs: number;
  pair: string;
  decision: string;
  reason: string;
}

/** Status surface for the Spread Radar auto-bot gadget. Missing backend
 * data renders as "—", never fabricated. */
export interface RadarBotStatusDto {
  enabled: boolean;
  mode: string;
  openPositions: BotOpenPositionDto[];
  entersToday: number;
  exitsToday: number;
  skipsToday: number;
  lastDecision: BotDecisionDto | null;
  lastError: string | null;
  auditErrors: number;
  monitorRunning: boolean;
  snapshotPresent: boolean;
}

export * from "./tickers.ts";
