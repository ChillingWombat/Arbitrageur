export interface SystemStatusDto {
  is_live_mode: boolean;
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
  latency_ms: number;
  fee_bps: number;
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
}

export type MatrixMode = "cross-venue" | "cross-ticker";
export type SlidingWindowOption = "10s" | "30s" | "1m" | "5m" | "15m";

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
  orderQty: number;
  estProfitUsd: number;
  isExecutable: boolean;
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

  // Polymarket CTF CLOB
  polymarket_api_key: string;
  polymarket_secret: string;
  polymarket_passphrase: string;
  polymarket_url: string;

  // Kalshi CFTC Binary
  kalshi_api_key_id: string;
  kalshi_private_key: string;
  kalshi_url: string;
}
export * from "./tickers";
