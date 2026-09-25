export interface SystemStatusDto {
  is_live_mode: boolean;
  environment: string;
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
  can_submit?: boolean;
  can_enter_strategy?: boolean;
}

/** Backend-reported order capability per venue id, built from
 *  `SystemStatusDto.active_venues`. */
export type VenueCaps = Map<string, { canSubmit: boolean; canEnterStrategy: boolean }>;

export interface VenueCapabilityDto {
  venue: string;
  name: string;
  supportsLive: boolean;
  hasCredentials: boolean;
  liveReady: boolean;
  submissionReady: boolean;
  strategyEntryReady: boolean;
  /** Adapter-level order-status polling is wired (reconciler primitive). */
  orderPollingReady: boolean;
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
  /** venue_id -> newest raw-sample ts (epoch ms) in the fetched range —
   *  lets the legend badge stale REST lanes honestly. */
  venue_last_ts_ms: Record<string, number>;
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
  /** Wilson 90% lower bound of win_probability on the episode bucket's
   *  effective count — how much confidence the evidence supports. */
  win_probability_lower?: number | null;
  /** Wilson 90% upper bound of win_probability — interval width shows how
   *  thin the episode evidence is. */
  win_probability_upper?: number | null;
  position_step?: number;
  max_units?: number;
  nominal_hurdle_bps?: number;
  /** Depth-supported size maximizing expected USD (monitor's lockstep
   *  capacity walk over both leg books). Null when no win-rate curve. */
  optimal_size_usd?: number | null;
  /** Largest fillable size whose marginal level clears the win floor
   *  with positive expected net. Null when unmeasurable. */
  breakeven_size_usd?: number | null;
  /** Probability-weighted fillable notional over levels with positive
   *  marginal expected net. Null when no win-rate curve exists. */
  expected_capacity_usd?: number | null;
  /** Fillable notional at each marginal expected-net floor. */
  bps_capacity_ladder?: BpsCapacityTierDto[];
  /** Per-chunk capacity curve for the detail-modal chart. */
  depth_curve?: DepthCurvePointDto[];
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

/** Passive limit unwind/rebalance controls (fail-closed).
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
  // Simple Spread (futures)
  targetProfitBps: number;
  windowPercentile: number;
  slidingWindow: string;
  slidingWindowMode: "manual" | "auto";
  slidingWindowSecs: number;
  // Simple Spread sizing: one grid step = min(mult x venue min lot,
  // maxStepUsd / price); max steps derive per ticker from venue equity x pct.
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
  // How each strategy resolves its limit-exit config:
  //   "inherit" → the global EngineSettings.passive_unwind policy;
  //   "custom"  → this strategy's own PassiveUnwindDto;
  //   "off"     → never passive, always market.
  // "" (legacy rows) auto-resolves: enabled config ⇒ "custom", else "inherit".
  futuresPassiveMode?: "inherit" | "custom" | "off" | "";
  fundingPassiveMode?: "inherit" | "custom" | "off" | "";
  spotPassiveMode?: "inherit" | "custom" | "off" | "";
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
  /** Best-effort reference price keyed by canonical BASE-QUOTE symbol. */
  base_prices?: Record<string, number>;
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

/** AutoTune preview DTOs. `recommended` is null exactly when
 * `status === "HELD"` — the manual value stays authoritative unless the user
 * explicitly applies a READY row. */
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

export interface AutotuneReadinessDto {
  venues_seen: string[];
  history_span_secs: number;
  multi_venue_coverage: number;
  pair_observations: number;
  total_pair_observations: number;
  min_observations_required: number;
  obs_floor_met: boolean;
  /** Qty-weighted median realized slippage (bps) summed over the selected
   * venues' recent filled legs — null until ≥5 legs carry both intended
   * and fill prices. Negative means fills beat the dispatch quote. */
  measured_slippage_bps: number | null;
  blockers: string[];
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
  readiness: AutotuneReadinessDto;
}

/** One-click apply result: only READY rows are ever written; HELD rows are
 * reported in skippedHeld and never touch the DTOs. */
export interface AutotuneApplyResultDto {
  symbol: string;
  applied: string[];
  skipped_held: string[];
  audit_id: number | null;
}

/** Per-ticker AutoTune cadence. "manual" (default) never auto-runs;
 * "periodic" re-tunes every period_secs; "continuous" every
 * continuous_min_secs. Non-manual modes auto-apply READY rows with the
 * same audit trail as the manual apply path. */
export interface TickerAutotuneModeDto {
  symbol: string;
  mode: "manual" | "periodic" | "continuous";
  last_run_ms: number | null;
  period_secs: number;
  continuous_min_secs: number;
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
  /** Canonical venue ID of the long leg (adapter key, never a display label). */
  long_venue_id: string;
  /** Canonical venue ID of the short leg. */
  short_venue_id: string;
  /** Exact execution symbol of the long leg (requested perp, or discovered *-SPOT for basis). */
  long_symbol: string;
  /** Exact execution symbol of the short leg (always the requested perp). */
  short_symbol: string;
  /** Market type of the long leg: "perpetual" (carry) or "spot" (basis). */
  long_market_type: string;
  /** Market type of the short leg: always "perpetual". */
  short_market_type: string;
  net_rate_8h_pct: number;
  net_apr_pct: number;
  /** Compounded-if-reinvested APY; `null` when the backend cannot express a
   * finite compounded figure — render as unavailable, never as 0. */
  net_apy_pct: number | null;
  round_trip_fee_bps: number;
  break_even_hours: number;
  estimated_daily_income_usd: number;
  trade_size_usd: number;
  /** Depth-supported executable size (absent on stale backends). */
  optimal_size_usd?: number;
  /** Measured entry cross as a positive cost in bps; negative = unmeasured. */
  entry_spread_bps?: number;
  /** Opportunity kind: "carry" (cross-exchange perp/perp), "basis" (same-exchange spot/perp),
   *  or "fixed_yield" (perp pair + two Pendle Boros YU legs locking the rate to maturity).
   *  Absent = carry (stale backends). */
  kind?: string;
  /** Signed live divergence (perp - spot) / spot * 10000 in bps. Only for basis opps. */
  basis_bps?: number;
  /** Spot reference price. Only for basis opps. */
  spot_price?: number;
  /** Perp reference price. Only for basis opps. */
  perp_price?: number;
  /** Informational win probability 0..1 (modelled-hold fee-coverage heuristic, sign-consistent with expected_return_usd). Absent = unusable edge. Never gates execution. */
  win_probability?: number;
  /** Marginal entry-cost slope in bps per extra $1k notional (both legs). Absent = book missing/thin. Informational only. */
  depth_slope_bps_per_1k?: number;
  /** Modelled expected holding horizon in hours (funding-gap persistence to
   * min-APR threshold + fee+spread breakeven). Absent on stale backends —
   * clients fall back to break_even_hours. */
  expected_hold_hours?: number;
  /** Human-readable hold basis (decay slope, threshold, spread inclusion). */
  hold_reason?: string;
  /** Heuristic hold confidence 0..1. */
  hold_confidence?: number;
  /** Empirical backtest win rate (fraction of simulated trailing-history entries covering costs). Absent below 20 trials. */
  backtest_win_rate?: number;
  /** Wilson 90% lower bound of backtest_win_rate on the effective
   *  (episode-block) trial count — consecutive prints overlap, so this is
   *  far below the raw trial count's implied confidence. */
  backtest_win_rate_lower?: number;
  /** Wilson 90% upper bound of backtest_win_rate. */
  backtest_win_rate_upper?: number;
  /** Effective trial count: contiguous above-threshold episode blocks in
   *  the backtest window — the statistically honest sample size. */
  backtest_episodes?: number;
  /** Win rate restricted to weekday trials (UTC), absent below 5 trials. */
  weekday_win_rate?: number;
  /** Win rate restricted to weekend trials (UTC), absent below 5 trials. */
  weekend_win_rate?: number;
  /** Empirical P(funding gap persists above the min-APR threshold through
   *  the modelled breakeven horizon) — a measured survival probability,
   *  unlike the heuristic win_probability. */
  persistence_probability?: number;
  /** Wilson 90% lower bound of persistence_probability. */
  persistence_probability_lower?: number;
  /** Empirical backtest mean net USD per entry. Absent below 20 trials. */
  backtest_expected_usd?: number;
  /** Standard error of backtest_expected_usd across trials. */
  backtest_expected_usd_se?: number;
  /** Backtest entry trials behind the two figures above. */
  backtest_trials?: number;
  /** Ascending net-USD deciles (P0…P100, 11 points) for P(net >= any target). Absent below 20 trials. */
  backtest_deciles_usd?: number[];
  /** Expected net carry in USD over the modelled hold with linear
   * funding-gap decay to the min-APR exit threshold, net of round-trip
   * fees + measured entry spread (signed). Absent on stale backends. */
  expected_return_usd?: number;
  /** Current funding print of the LONG leg, 8h-normalized percent.
   *  Basis opps: long leg is spot (pays none) so 0. Absent on stale backends. */
  long_rate_8h_pct?: number;
  /** Current funding print of the SHORT leg, 8h-normalized percent.
   *  Absent on stale backends. */
  short_rate_8h_pct?: number;
  /** Depth curve: cumulative/marginal net & expected carry profit per
   * unit walking both leg books in lockstep. Empty when a book is
   * missing. Income is depth-invariant; the curve degrades through the
   * entry cross only. (snake_case field, camelCase point fields.) */
  depth_curve?: DepthCurvePointDto[];
  /** Profit-maximizing size in USD — depth where marginal net turns
   * non-positive. Absent when no live book depth exists. */
  optimal_capacity_usd?: number;
  /** Largest still-profitable size in USD — cumulative net breakeven
   * bound. Absent when no live book depth exists. */
  max_capacity_usd?: number;

  // --- Fixed-yield (Boros YU hedge) fields — present when kind === "fixed_yield" ---
  /** Locked net APR in percent (F_short − F_long), executable at the YU book top. */
  fixed_apr_pct?: number;
  /** The two Boros YU hedge legs converting floating funding into a locked rate. */
  yu_legs?: YuLegDto[];
  /** Extra entry cost vs the 2-leg variant, in bps of notional (YU taker fees + gas). */
  yu_hedge_cost_bps?: number;
  /** Estimated Boros collateral lock for the two YU legs, USD. */
  boros_margin_usd?: number;
  /** Estimated Arbitrum gas for the YU batch, USD. */
  boros_gas_usd?: number;
  /** Unix-ms timestamp of the shared YU maturity — the locked rate's end. */
  lock_until_ms?: number;
}

/** One Boros Yield-Unit hedge leg of a fixed-yield opportunity. */
export interface YuLegDto {
  /** Canonical venue id whose funding this leg swaps (e.g. "binance"). */
  venue_id: string;
  yu_market_id: number;
  /** Boros market symbol, e.g. "BINANCE-BTCUSDT-30OCT2026". */
  yu_symbol: string;
  /** "long" = pay fixed / receive floating; "short" = receive fixed / pay floating. */
  yu_side: string;
  /** Executable fixed APR in percent. */
  fixed_apr_pct: number;
  /** YU taker fee in bps of notional. */
  fee_bps: number;
  /** Unix seconds — the maturity this leg locks to. */
  maturity_unix: number;
}

export interface FundingTrendPointDto {
  t: number;
  v: number;
}

export interface FundingTrendDto {
  symbol: string;
  long_venue: string;
  short_venue: string;
  /** Per-leg 8h-normalized funding history (percent), ascending by time. */
  long_series: FundingTrendPointDto[];
  short_series: FundingTrendPointDto[];
  /** Ticker mid-price history (USD), ascending by time. */
  price_series: FundingTrendPointDto[];
  current_mid_usd?: number;
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
  conditional_payout_cents: number;
  net_profit_cents: number;
  roi_pct: number;
  has_arbitrage: boolean;
  /** Per-chunk capacity curve for the detail-modal chart. Empty until
   *  the prediction scan resolves real books (impl currently stubbed). */
  depth_curve?: DepthCurvePointDto[];
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
  /** Real regression R² when a fit produced enough variance; `null` when
   * the fit quality is unavailable — never display as a number. */
  r_squared: number | null;
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

  // USD-stablecoin quote policy: "separate" keeps BASE-QUOTE identity,
  // "parity" merges USD-family quotes at 1:1, "fx_adjusted" merges and
  // converts through the live stablecoin rate table (default).
  usd_quote_mode?: string;
  // Depeg thresholds, |deviation| from $1.00 in bps: warn displays an
  // alert; past halt, pairs quoted in the coin are blocked.
  stablecoin_warn_bps?: number;
  stablecoin_halt_bps?: number;

  // Exchange Enablement Toggles
  hyperliquid_enabled?: boolean;
  lighter_enabled?: boolean;
  vanta_enabled?: boolean;
  robinhood_enabled?: boolean;
  arcus_enabled?: boolean;
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
  popdex_enabled?: boolean;
  popdex_network?: string;
  coinbase_enabled?: boolean;
  alpaca_enabled?: boolean;
  deribit_enabled?: boolean;

  // Deribit (OAuth2 client credentials pair — mint on test.deribit.com for
  // testnet, www.deribit.com for live; never mix).
  deribit_client_id?: string;
  deribit_client_secret?: string;

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
  trading_mode: "testnet" | "live";
  paper_starting_balance_usd?: number;
  fast_warmup_enabled: boolean;

  // Overall circuit-breaker + safety-net limits (Safety settings section)
  cb_max_daily_drawdown_usd?: number;
  cb_max_daily_drawdown_pct?: number;
  cb_max_consecutive_errors?: number;
  cb_max_spread_divergence_bps?: number;
  delta_max_unhedged_usd_per_asset?: number;
  margin_warning_usage_pct?: number;
  margin_critical_usage_pct?: number;
  margin_min_liquidation_distance_pct?: number;
  max_leverage?: number;
  /** Global Auto Position Exit policy (Execution section). Strategies on
   * `*PassiveMode = "inherit"` use this; the popup's Auto Limit mode runs
   * it too. Serialized camelCase like PassiveUnwindDto. */
  passive_unwind?: PassiveUnwindDto;
  /** Hold deadline (seconds) before the auto-exit supervisor unwinds a
   * still-open futures pair position, regardless of the live edge.
   * Backend-validated 60–604800; default 86400 (24h). */
  auto_exit_max_hold_secs?: number;
  /** Opt-in autonomous fixed-yield entries (Boros YU locks, live-only).
   * Default off — gates additionally on approval validity, gas treasury,
   * margin, lock term, and post-cost return. */
  auto_fixed_yield_enabled?: boolean;
  /** Minimum lock term (days) for an autonomous fixed-yield entry. */
  auto_fixed_yield_min_lock_days?: number;
  /** Minimum Boros gas-treasury USD for an autonomous fixed-yield entry. */
  auto_fixed_yield_min_gas_usd?: number;
  balance_stale_warn_secs?: number;

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

/** Live storage footprint behind the Historical Price Data retention
 *  inputs: whole-DB bytes (what the GB budget enforces) plus the
 *  top-of-book sample span (what the day limit enforces). */
export interface StorageStatsDto {
  db_bytes: number;
  sample_rows: number;
  oldest_sample_ts_ms: number | null;
  newest_sample_ts_ms: number | null;
}

/** One tracked USD-family quote's live health row (camelCase on the wire). */
export interface StablecoinQuoteDto {
  quote: string;
  effectiveRate: number;
  deviationBps: number;
  /** "ok" | "warn" | "halted" | "unobserved" */
  status: string;
  lastUpdateMs: number | null;
  source: string | null;
  stale: boolean;
}

export interface StablecoinHealthDto {
  mode: string;
  quotes: StablecoinQuoteDto[];
  /** Quotes past the halt threshold — pairs quoted in these are blocked. */
  haltedQuotes: string[];
  /** True while every external rate source is unreachable. */
  feedStale: boolean;
  generatedTsMs: number;
}

export interface VaultStatus {
  is_initialized: boolean;
  is_unlocked: boolean;
  /** True only for inactivity-timer locks: provisioned venue credentials
      keep working. False after a manual lock, which wipes them. */
  auto_locked?: boolean;
  unlocked_until?: string | null;
  /** Whether TOTP MFA is enrolled — while locked this is the plaintext kv
      flag (the secret itself lives inside the vault); while unlocked it is
      the authoritative vault content. */
  mfa_enabled?: boolean;
}

/** Material returned once by `begin_mfa_enrollment` — shown to the operator
    for authenticator-app enrollment, then confirmed with a live code. */
export interface MfaEnrollment {
  secretBase32: string;
  otpauthUri: string;
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

/** A persisted one-shot scheduled exit (Portfolio Monitor → Close →
 *  Schedule). status: pending | firing | fired | failed | cancelled. */
export interface ScheduledExitDto {
  id: string;
  venue: string;
  symbol: string;
  side: "buy" | "sell";
  closePct: number;
  mode: "auto" | "limit" | "market";
  limitPrice: number | null;
  executeAtMs: number;
  status: string;
  note: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface ExternalPositionsResponse {
  positions: ExternalPositionDto[];
  unreadable: string[];
  /** Active trading environment — "testnet" | "live" (display context only;
   * closes are gated per-position by closeSupported). */
  environment: string;
  venueNotes?: string[];
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

/** One row of the executor's venue taker-fee table (`get_venue_fees`). */
export interface VenueFeeDto {
  venue_id: string;
  taker_bps: number;
}

/** A curated parameter bundle served by the backend preset table
 *  (strategy_config.rs). `description` states the intended regime and risk
 *  posture verbatim; `version` is recorded in the apply audit row so history
 *  can distinguish which table revision was applied. */
export interface StrategyPresetDto {
  id: string;
  label: string;
  description: string;
  version: number;
  targetProfitBps: number;
  windowPercentile: number;
  slidingWindowSecs: number;
  executionMode: ArbitrageExecutionMode;
  stepSizeMult: number;
  maxStepUsd: number;
  maxTickerEquityPct: number;
  userTargetLeverage: number;
}

export interface VenueFeesDto {
  fees: VenueFeeDto[];
  /** Executor fallback for ids absent from `fees` — UI lookups must use
   *  this, not a local constant. */
  default_bps: number;
}

/** Distribution stats over a spread/deviation series — every metric is
 *  null on empty input rather than a fabricated zero. */
export interface SpreadStatsDto {
  sample_count: number;
  history_span_secs: number;
  mean_bps: number | null;
  std_bps: number | null;
  p50_bps: number | null;
  p90_bps: number | null;
  p99_bps: number | null;
  max_bps: number | null;
  frac_at_or_above_hurdle: number | null;
}

export interface SpreadPairAnalyticsDto {
  buy_venue: string;
  sell_venue: string;
  stats: SpreadStatsDto;
  half_life_secs: number | null;
}

export interface SpreadAnalyticsDto {
  symbol: string;
  window_secs: number;
  hurdle_bps: number;
  venues_seen: string[];
  /** Per-ticker dislocation band stats (max–min venue spread per poll). */
  band: SpreadStatsDto;
  pairs: SpreadPairAnalyticsDto[];
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
  /** A venue endpoint override is stored (REST entity domain or JSON-RPC). */
  hasBaseUrl: boolean;
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
  /** "testnet" | "live" — which environment this account's credentials bind
   *  to. Immutable once created. */
  environment: string;
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
  /** Venue endpoint patch: omitted preserves, empty clears, nonempty sets.
   *  Supports REST entity domains and secret JSON-RPC URLs. */
  baseUrl?: string;
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
  /** "testnet" | "live" — which environment this account's credentials bind
   *  to. Omit on create to use the active environment; immutable after. */
  environment?: string;
  credentials?: AccountCredentialInput | null;
}

/** Result of the shared-EVM-wallet fan-out (`save_evm_wallet`): the derived
 *  signer plus which AMM venue accounts received key vs. address-only. */
export interface EvmWalletSaveDto {
  /** EIP-55 checksum address derived from the submitted private key. */
  signerAddress: string;
  /** Swap-capable venues whose accounts now hold the private key. */
  keyedVenues: string[];
  /** Market-data venues whose accounts hold only the wallet address. */
  addressOnlyVenues: string[];
  /** `{venue}-default` account ids created by this save (start disabled). */
  createdAccounts: string[];
  /** Where the key bytes landed: "stronghold" (guarded signer storage —
   *  credential records carry only a sentinel) or "vault" (encrypted
   *  credential record — legacy/fallback path). */
  keyStorage: string;
}

/** Custody status of the shared EVM wallet key (`get_evm_key_custody`). */
export interface EvmKeyCustodyDto {
  /** "stronghold" | "vault" | "none" — where stored key material lives. */
  custody: string;
  /** A stronghold snapshot is open AND holds the key record. */
  strongholdLoaded: boolean;
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
  /** Exact per-leg execution symbol (funding legs can diverge from the
   *  signal root); backfilled from the root for pre-v22 rows. */
  symbol: string;
  intendedQty: number;
  filledQty: number;
  fillPrice: number | null;
  feeUsd: number;
  venueOrderId: string | null;
  clientOrderId: string | null;
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

/** Durable margin-utilisation observation for a venue account. */
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
  /** Exact leg symbol behind this row's net quantity. */
  symbol: string;
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
  /** Durable server-side exits-only flag for (env, strategy, symbol). */
  reduceOnly: boolean;
}

/** Per-strategy capability descriptor — Execute controls and auto-loops
 *  must gate on this; a permanently-stubbed path never gets an armed
 *  Execute button. */
export interface StrategyCapabilityDto {
  strategyId: string;
  scanReady: boolean;
  executeReady: boolean;
  reason: string | null;
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
  /** Scan-time reversion probability — persisted to the backend
   * calibration log when the candidate dispatches (kind "reversion"). */
  predictedWinProbability?: number | null;
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

/** Result of an emergency per-strategy flatten. */
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

/** One Boros YU lock's maturity tracker. `maturityState` is
 *  venue-truth-derived: `locked` → `due` → `settled` |
 *  `settlement_unverified`; `closed_early` = exited via the priced path.
 *  `resolvedAction` records the operator's explicit choice. */
export interface FixedYieldMaturityDto {
  signalId: string;
  parentSignalId: string | null;
  parentStatus: string | null;
  symbol: string;
  lockUntilMs: number;
  maturityState: "locked" | "due" | "settled" | "settlement_unverified" | "closed_early";
  resolvedAction: "close_pair" | "keep_carry" | null;
  venueObservation: string | null;
  resolvedAt: string | null;
}

export interface FixedYieldExitQuoteLegDto {
  legSide: "long" | "short";
  yuSymbol: string;
  marketId: number;
  openQty: number;
  entryRate: number;
  /** `null` = no executable rate on the book — the leg cannot be priced. */
  exitRate: number | null;
  estExitPnlUsd: number | null;
}

export interface FixedYieldExitQuoteDto {
  signalId: string;
  parentSignalId: string | null;
  maturityState: string;
  lockUntilMs: number;
  yearsRemaining: number;
  legs: FixedYieldExitQuoteLegDto[];
  /** `null` when any leg is unquotable — never a partial sum. */
  estTotalPnlUsd: number | null;
  notes: string[];
}

/** Boros account ops status for the Exchange Accounts card — every field
 *  is venue-read or honestly `null`/`unknown`, never fabricated. */
export interface BorosStatusDto {
  /** False when no Boros credentials are configured at all. */
  configured: boolean;
  /** Whether the agent key is approved AND unexpired right now. */
  agentApproved: boolean;
  /** Agent-approval expiry (unix seconds) — null when unreadable. */
  agentExpiryUnix: number | null;
  /** Sponsored-gas treasury in USD — null when the venue doesn't report. */
  gasTreasuryUsd: number | null;
  /** Named reason when approval is absent/expired, for UI remediation. */
  approvalNote: string | null;
  /** Named reason when the gas read failed, for UI remediation. */
  gasNote: string | null;
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
  /** Per-chunk capacity curve for the detail-modal chart. Empty until
   *  the spot scan resolves real books (impl currently stubbed). */
  depth_curve?: DepthCurvePointDto[];
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
  /** Per-size capacity curve from the depth-aware cycle evaluation —
   *  net fields only; atomic cycles carry no win-rate model. */
  depth_curve?: DepthCurvePointDto[];
}

export type ParityDirection = "conversion" | "reversal";

export interface OptionsParityLegDto {
  instrument: string;
  role: "call" | "put" | "perp";
  action: "buy" | "sell";
  /** Venue-unit quote (BTC for options, USD for the perp). */
  venue_price: number;
  /** USD translation (option prices × index). */
  usd_price: number;
}

export interface OptionsParityOpportunityDto {
  id: string;
  venue: string;
  currency: string;
  expiry_ms: number;
  expiry_iso: string;
  strike: number;
  direction: ParityDirection;
  call_leg: OptionsParityLegDto;
  put_leg: OptionsParityLegDto;
  perp_leg: OptionsParityLegDto;
  synthetic_forward_usd: number;
  perp_equiv_usd: number;
  funding_8h: number;
  funding_drain_usd: number;
  gross_edge_usd: number;
  est_fees_usd: number;
  net_edge_usd: number;
  edge_bps: number;
  index_price_usd: number;
  qty_base_suggested: number;
  is_executable: boolean;
  reason?: string;
}

export interface OptionsParityScanParams {
  currency: string;
  qty_base?: number;
  min_hours_to_expiry?: number;
  max_days_to_expiry?: number;
  min_edge_bps?: number;
}

export interface OptionsParityExecutionParams {
  id: string;
  currency: string;
  qty_base: number;
  min_edge_bps?: number;
  auto?: boolean;
}

export interface TriangleExecutionParams {
  id: string;
  venue: string;
  path: string;
  trade_size_usd: number;
  min_profit_bps: number;
  net_spread_bps?: number;
  /** Autonomous bot dispatch — autonomous entries are testnet-only. */
  auto?: boolean;
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
  | "preparing"
  | "tracking"
  | "triggered"
  | "ready"
  | "no-data";

export interface CapacityTierDto {
  sizeUsd: number;
  filledUsd: number;
  netBps: number | null;
  expectedNetBps: number | null;
  marginalPWin: number | null;
}

/** Fillable notional while marginal expected net stays >= `minNetBps`. */
export interface BpsCapacityTierDto {
  minNetBps: number;
  capacityUsd: number;
}

/** One capacity-curve sample emitted by the backend book walk. USD values
 * derive as `bps · cumUsd / 1e4`. */
export interface DepthCurvePointDto {
  /** Cumulative fillable notional through this chunk, USD (x-axis). */
  cumUsd: number;
  /** Cumulative raw net capture per unit, bps. */
  cumNetBps: number;
  /** Cumulative probability-weighted expected net per unit, bps. */
  cumExpBps: number | null;
  /** Marginal raw net per unit at this depth, bps. */
  margNetBps: number;
  /** Marginal expected net per unit at this depth, bps. */
  margExpBps: number | null;
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
  /** Deprecated alias of reversionProbability (old backend). Prefer winProbability. */
  pWin: number | null;
  /** Direction-insensitive mean-reversion P(|dev| reverts). Detail-only as "Reversion Probability". */
  reversionProbability?: number | null;
  /** Directional trade win probability after all gates. Table "Win %" column. */
  winProbability?: number | null;
  /** Wilson 90% lower bound of winProbability — the confidence-aware
   * floor the executable gate checks against minWinLowerBound. */
  winProbabilityLower?: number | null;
  /** Wilson 90% upper bound of winProbability — width shows evidence depth. */
  winProbabilityUpper?: number | null;
  /** Kish-effective episode count behind the win estimate (shrinks under
   * recency weighting). 0 when the OU fallback produced the estimate. */
  pWinNEffective?: number | null;
  /** Still-open (censored) episodes inside the win bucket — high share
   * means the rate is understated, not confirmed. */
  pWinCensored?: number | null;
  /** True when the win estimate came from the OU first-passage fallback
   * rather than empirical episode evidence. */
  pWinFromModel?: boolean | null;
  /** Empirical P(net > 0) for a fresh entry at the current deviation —
   * historical excursions replayed through the exit policy and scored on
   * real net bps. THE P&L-defined win rate (reversionProbability only
   * answers "does the spread normalize"). */
  netWinProbability?: number | null;
  /** Wilson lower bound of netWinProbability counting censored episodes
   * as non-wins — the fail-closed reading. */
  netWinLower?: number | null;
  /** Recency-weighted mean net bps per simulated entry at this deviation. */
  netExpectedBps?: number | null;
  /** Ascending weighted net-bps deciles (P0…P100, 11 points) of simulated
   * entries — enables P(net >= any bps target) client-side. */
  netDecilesBps?: number[] | null;
  /** Simulated entry trials / still-open share at this deviation. */
  outcomeTrials?: number | null;
  outcomeCensored?: number | null;
  /** Weighted median entry→exit seconds across simulated episodes, and
   * among profitable ones — capital-turnover context. */
  medianHoldSecs?: number | null;
  medianWinSecs?: number | null;
  expectedProfitBps: number | null;
  expectedProfitUsd: number | null;
  optimalSizeUsd: number | null;
  breakevenSizeUsd: number | null;
  capacityLadder: CapacityTierDto[];
  /** Total expected capacity — probability-weighted fillable notional
   * over book levels with positive marginal expected net (backend). */
  expectedCapacityUsd?: number | null;
  /** Depth capacity at each marginal expected-net floor (bps). */
  bpsCapacityLadder?: BpsCapacityTierDto[];
  /** Per-chunk capacity curve for the detail-modal chart. */
  depthCurve?: DepthCurvePointDto[];
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

/** One persisted pair-signal lifecycle event (monitor_signal_events table).
 * Append-only audit: every status transition plus minute-bucketed heartbeats
 * while a signal stays active. `economicsJson` carries the point-in-time
 * friction/exit snapshot behind the transition. */
export interface MonitorSignalEventDto {
  id: number;
  symbol: string;
  buyVenue: string;
  sellVenue: string;
  tsMs: number;
  oldStatus: string | null;
  newStatus: string;
  edgeBps: number | null;
  devBps: number | null;
  pWin: number | null;
  economicsJson: string | null;
  trigger: string;
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
  /** Episode recency half-life in seconds — older episodes weigh
   * exponentially less in win/outcome estimates. 0 disables weighting. */
  episodeHalfLifeSecs: number;
  /** Confidence floor on the win probability's Wilson lower bound
   * (z=1.645): a row is not executable when its lower bound falls below
   * this even if the point estimate passes minWinProbability. 0 disables. */
  minWinLowerBound: number;
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
/** PopDEX agent toolkit (ExchangeAccounts modal, desktop only). */
export interface GeneratedAgentDto {
  address: string;
  privateKey: string;
}
export interface ApproveTxDto {
  to: string;
  chainId: number;
  rpcUrl: string;
  explorerUrl: string;
  data: string;
  initialNonceMs: number;
  eip681Uri: string;
}
export interface AgentCheckDto {
  approved: boolean;
  agentCount: number;
}
/** One reconciliation-supervisor pass over persisted working orders. */
export interface ReconcileSummaryDto {
  pendingLegs: number;
  polled: number;
  advanced: number;
  resolved: number;
  escalated: number;
  pollErrors: number;
  skipped: number;
}

export interface SignApproveDto {
  txHash: string;
  explorerUrl: string;
}
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

/** One reliability-diagram bucket from the forecast calibration report. */
export interface CalibrationBin {
  lo: number;
  hi: number;
  n: number;
  meanPred: number;
  observed: number;
}

/** Aggregate calibration stats for one scored forecast stream. */
export interface CalibrationReport {
  scored: number;
  /** Brier score (mean squared forecast error); 0.25 = always-0.5 baseline. */
  brier: number | null;
  /** OLS calibration intercept — >0 with slope<1 is the overconfidence
   * signature. Null under 3 scored pairs or zero prediction variance. */
  intercept: number | null;
  slope: number | null;
  bins: CalibrationBin[];
}

/** Per (strategy, kind) calibration cut. */
export interface CalibrationCutDto {
  strategyId: string;
  kind: string;
  report: CalibrationReport;
}

/** Forecast-calibration report over the durable prediction log. */
export interface CalibrationReportDto {
  /** Predictions scored against a reconciled realized outcome. */
  scored: number;
  /** Predictions waiting on a reconciled close. */
  unresolved: number;
  /** Aggregate calibration over every scored net-P&L prediction. */
  overall: CalibrationReport;
  cuts: CalibrationCutDto[];
  /** Logged-but-not-P&L-scored predictions (reversion/persist/heuristic). */
  auditOnly: number;
}

export * from "./tickers.ts";
