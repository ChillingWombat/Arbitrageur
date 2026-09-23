// Dev-only design-review harness. Serves the real App in a plain browser by
// installing a fixture-backed __TAURI_INTERNALS__.invoke before main.tsx
// mounts. Loaded via /mock.html only — never shipped in the desktop bundle.
export {};

const now = Date.now();

const VENUES = [
  { id: "hyperliquid", name: "Hyperliquid", market_type: "perp", status: "CONNECTED", latency_ms: 41, fee_bps: 2.5, can_submit: true, can_enter_strategy: true },
  { id: "lighter", name: "Lighter", market_type: "perp", status: "CONNECTED", latency_ms: 63, fee_bps: 0, can_submit: true, can_enter_strategy: true },
  { id: "vanta", name: "Vanta", market_type: "perp", status: "CONNECTED", latency_ms: 88, fee_bps: 3, can_submit: true, can_enter_strategy: true },
  { id: "binance", name: "Binance Futures", market_type: "perp", status: "CONNECTED", latency_ms: 35, fee_bps: 4, can_submit: true, can_enter_strategy: true },
  { id: "okx", name: "OKX", market_type: "perp", status: "READ_ONLY", latency_ms: 52, fee_bps: 5, can_submit: false, can_enter_strategy: false },
  { id: "kraken", name: "Kraken Futures", market_type: "perp", status: "READ_ONLY", latency_ms: 120, fee_bps: 5, can_submit: false, can_enter_strategy: false },
  { id: "decibel", name: "Decibel", market_type: "perp", status: "STALE", latency_ms: null, fee_bps: 2, can_submit: true, can_enter_strategy: true },
];

const book = (venue_id: string, venue_name: string, symbol: string, mid: number, spreadBps = 1.2) => {
  const half = mid * (spreadBps / 20000);
  const bids: [number, number][] = [];
  const asks: [number, number][] = [];
  for (let i = 0; i < 8; i++) {
    bids.push([mid - half - i * mid * 0.0002, 1.5 + i * 0.8]);
    asks.push([mid + half + i * mid * 0.0002, 1.4 + i * 0.7]);
  }
  return {
    venue_id, venue_name, symbol,
    best_bid: mid - half, best_ask: mid + half,
    spread: half * 2, spread_bps: spreadBps, mid_price: mid,
    total_bid_depth: 420000, total_ask_depth: 390000,
    vwap_bid: mid - half * 1.4, vwap_ask: mid + half * 1.4,
    funding_rate_8h_pct: 0.01, bids, asks,
  };
};

const FIXTURES: Record<string, (args?: Record<string, unknown>) => unknown> = {
  get_system_status: () => ({
    is_live_mode: true,
    environment: "live",
    is_paused: false,
    total_realized_pnl: 1842.55,
    total_trades_count: 64,
    successful_trades_count: 16,
    circuit_breaker_status: "ARMED",
    circuit_breaker_tripped: false,
    current_drawdown_usd: 412.3,
    max_drawdown_usd: 25000,
    consecutive_failures: 0,
    active_venues: VENUES,
  }),
  get_engine_settings: () => ({
    hsm_url: "", hmac_token: "",
    min_depth_ratio: 1.2,
    hyperliquid_enabled: true, lighter_enabled: true, vanta_enabled: true,
    binance_enabled: true, okx_enabled: true, kraken_futures_enabled: true,
    decibel_enabled: true, popdex_enabled: false, aster_enabled: false,
    robinhood_enabled: false, coinbase_enabled: false,
    hyperliquid_url: "https://api.hyperliquid.xyz",
    vanta_url: "https://api.orderly.org",
    lighter_url: "https://mainnet.zklighter.elliot.ai",
    kalshi_url: "https://trading-api.kalshi.com",
    kraken_spot_url: "https://api.kraken.com",
    engine_rpc_port: 9111, engine_ws_port: 9112, bind_address: "127.0.0.1",
    trading_mode: "live",
    fast_warmup_enabled: true,
    cb_max_daily_drawdown_usd: 25000, cb_max_daily_drawdown_pct: 5,
    cb_max_consecutive_errors: 5, cb_max_spread_divergence_bps: 250,
    delta_max_unhedged_usd_per_asset: 2500,
    margin_warning_usage_pct: 70, margin_critical_usage_pct: 85,
    margin_min_liquidation_distance_pct: 15,
    max_leverage: 10,
    live_entry_enabled: true, live_entry_max_notional_usd: 2000,
    auto_exit_max_hold_secs: 86400, balance_stale_warn_secs: 300,
    configured_venues: ["hyperliquid", "lighter", "vanta", "binance", "okx", "decibel"],
    referral_lighter: "", referral_robinhood: "", sponsor_link: "",
    version: "0.1.0", git_repo: "https://github.com/Dav1D1010/Arbitrageur",
    hyperliquid_wallet: "0xabc…def", hyperliquid_private_key: "",
    lighter_account_index: "281474976710654", lighter_api_key: "",
    vanta_account_id: "", vanta_orderly_key: "", vanta_orderly_secret: "",
    robinhood_wallet: "", robinhood_private_key: "", robinhood_rpc_url: "", robinhood_router: "",
    polymarket_api_key: "", polymarket_secret: "", polymarket_passphrase: "", polymarket_url: "",
    kalshi_api_key_id: "", kalshi_private_key: "",
  }),
  get_vault_status: () => ({ is_initialized: true, is_unlocked: true, auto_locked: false }),
  get_market_universe: () => VENUES.slice(0, 4).map((v) => ({
    venue_id: v.id,
    symbols: ["BTC-USDC", "ETH-USDC", "SOL-USDC", "DOGE-USDC", "AVAX-USDC", "LINK-USDC", "ARB-USDC", "SUI-USDC"],
    base_prices: { "BTC-USDC": 118420, "ETH-USDC": 4412, "SOL-USDC": 218, "DOGE-USDC": 0.24, "AVAX-USDC": 32, "LINK-USDC": 22.4, "ARB-USDC": 0.51, "SUI-USDC": 3.9 },
  })),
  fetch_orderbooks: (a) => {
    const sym = String(a?.symbol ?? "BTC-USDC");
    const px = sym.startsWith("ETH") ? 4412 : sym.startsWith("SOL") ? 218 : 118420;
    return [
      book("hyperliquid", "Hyperliquid", sym, px),
      book("lighter", "Lighter", sym, px * 1.0004),
      book("vanta", "Vanta", sym, px * 0.9997),
      book("binance", "Binance Futures", sym, px * 1.0001),
    ];
  },
  get_ticker_volumes: () => VENUES.map((v) => ({
    venue_id: v.id,
    volumes: { BTC: 4.2e9, ETH: 1.8e9, SOL: 9.1e8, DOGE: 2.4e8, AVAX: 1.1e8, LINK: 9e7, ARB: 4e7, SUI: 3e8 },
  })),
  scan_spatial_opportunities: (a) => {
    const sym = String(a?.symbol ?? "BTC-USDC");
    const px = sym.startsWith("ETH") ? 4412 : sym.startsWith("SOL") ? 218 : 118420;
    return [
      { id: "opp1", symbol: sym, buy_venue: "Vanta", sell_venue: "Hyperliquid", buy_price: px * 0.9997, sell_price: px * 1.0004, gross_spread: px * 0.0007, gross_spread_bps: 7.0, fee_bps: 5.5, net_spread_bps: 1.5, order_qty: 0.05, estimated_profit_usd: 0.88, is_executable: true, buy_venue_id: "vanta", sell_venue_id: "hyperliquid", buy_fee_bps: 3, sell_fee_bps: 2.5, round_trip_commission_bps: 5.5, entry_slippage_bps: 1.1, exit_slippage_bps: 1.1, closing_spread_bps: 0.8, expected_friction_bps: 8.5, required_gross_bps: 9.5, slippage_ok: true, depth_ok: true },
      { id: "opp2", symbol: sym, buy_venue: "Lighter", sell_venue: "Binance Futures", buy_price: px * 0.9999, sell_price: px * 1.0003, gross_spread: px * 0.0004, gross_spread_bps: 4.0, fee_bps: 4.0, net_spread_bps: 0.4, order_qty: 0.05, estimated_profit_usd: 0.24, is_executable: false, buy_venue_id: "lighter", sell_venue_id: "binance", buy_fee_bps: 0, sell_fee_bps: 4, round_trip_commission_bps: 4, entry_slippage_bps: 0.9, exit_slippage_bps: 0.9, closing_spread_bps: 0.6, expected_friction_bps: 6.4, required_gross_bps: 7.4, slippage_ok: true, depth_ok: true },
    ];
  },
  scan_spot_opportunities: () => [],
  scan_triangle_opportunities: () => [],
  scan_funding_opportunities: () => [
    { symbol: "ETH-USDC", long_venue: "lighter", short_venue: "hyperliquid", long_venue_id: "lighter", short_venue_id: "hyperliquid", funding_apr_pct: 14.2, funding_spread_bps_8h: 1.8, mark_price: 4412, is_executable: true },
  ],
  scan_prediction_opportunities: () => [],
  get_risk_overview: () => ({
    circuit_breaker_state: "ARMED", is_halted: false,
    current_drawdown_usd: 412.3, max_drawdown_limit_usd: 25000, drawdown_pct: 1.6,
    consecutive_failures: 0, max_failures_allowed: 5,
    total_portfolio_value_usd: 287_400,
    net_deltas: [
      { asset: "BTC", net_quantity: 0.002, net_exposure_usd: 236, delta_limit_usd: 2500, is_neutral: true },
      { asset: "ETH", net_quantity: -0.41, net_exposure_usd: -1808, delta_limit_usd: 2500, is_neutral: true },
    ],
    margin_health: {
      maintenance_margin_req_usd: 18400, margin_balance_usd: 287400,
      margin_ratio_pct: 6.4, liquidation_distance_pct: 78, status: "HEALTHY",
    },
  }),
  get_execution_logs: () => [
    { id: "ex1", timestamp: new Date(now - 3600_000).toISOString(), trade_type: "spatial", strategy_id: "futures", symbol: "ETH-USDC", venue_a: "hyperliquid", venue_b: "lighter", direction: "Long Lighter / Short Hyperliquid", trigger_spread_bps: 6.8, baseline_mean_spread_bps: 1.2, position_step: 1, max_units: 3, nominal_hurdle_bps: 5.0, leg_a_status: "filled", leg_b_status: "filled", success: true, gross_profit: 3.42, fees: 1.1, net_profit: 2.32, latency_us: 91200, note: "", is_live: true },
    { id: "ex2", timestamp: new Date(now - 7200_000).toISOString(), trade_type: "spatial", strategy_id: "futures", symbol: "BTC-USDC", venue_a: "vanta", venue_b: "binance", direction: "Long Vanta / Short Binance", trigger_spread_bps: 5.4, baseline_mean_spread_bps: 0.8, position_step: 1, max_units: 3, nominal_hurdle_bps: 5.0, leg_a_status: "filled", leg_b_status: "rejected", success: false, gross_profit: 0, fees: 0.4, net_profit: -0.4, latency_us: 141000, note: "leg_b invalid price", is_live: true },
  ],
  get_all_ticker_strategy_configs: () => [],
  get_all_strategy_params: () => [],
  get_ticker_position_state: () => null,
  get_open_positions: () => [
    {
      key: "futures|ETH-USDC|lighter+hyperliquid", strategyId: "futures", symbol: "ETH-USDC",
      signalCount: 2,
      venues: [
        { venue: "lighter", symbol: "ETH-USDC", netQty: 0.25 },
        { venue: "hyperliquid", symbol: "ETH-USDC", netQty: -0.25 },
      ],
      expectedNetUsd: 1.1, unrealizedPnlUsd: 0.26, feesUsd: 0.84, notionalUsd: 2206,
      openedAt: new Date(now - 5400_000).toISOString(),
    },
  ],
  get_venue_balances: () => VENUES.slice(0, 4).map((v, i) => ({
    venue: v.id, equity_usd: [82000, 61400, 71000, 73000][i],
    available_margin_usd: [79000, 60000, 70500, 71200][i],
    margin_rate_pct: [3.7, 2.3, 0.8, 2.5][i], status: "HEALTHY",
    recommended_action: null, is_live: true, account_id: `live-${v.id}-main`,
  })),
  list_accounts: () => VENUES.slice(0, 4).map((v) => ({
    id: `live-${v.id}-main`, venue: v.id, label: "Main", enabled: true,
    isDefault: true, createdAt: new Date(now - 86400_000 * 30).toISOString(),
    updatedAt: new Date(now - 86400_000).toISOString(),
    credentialsConfigured: true, hasApiKey: true, hasApiSecret: true,
    hasPassphrase: false, hasAccountId: v.id === "vanta",
    hasWalletAddress: v.id === "hyperliquid", hasPrivateKey: v.id !== "binance",
    hasBaseUrlOverride: false, environment: "live",
  })),
  get_monitor_snapshot: () => ({
    generatedTsMs: now, running: true,
    watchlist: ["BTC-USDC", "ETH-USDC", "SOL-USDC"],
    exempted: [],
    venues: VENUES.slice(0, 4).map((v) => ({
      venue: v.id, readBudgetPerMin: 600, usedLastMin: 120, symbols: 8,
      refreshIntervalSecs: 5, backingOff: false, accountTier: "standard",
    })),
    rows: [
      { symbol: "ETH-USDC", buyVenue: "lighter", sellVenue: "hyperliquid", status: "triggered", currentEdgeBps: 6.9, baselineBps: 1.1, deviationBps: 5.8, netCaptureBps: 3.1, pWin: 0.94, reversionProbability: 0.78, winProbability: 0.94, winProbabilityLower: 0.88, winProbabilityUpper: 0.97, pWinNEffective: 24, pWinCensored: 0.05, pWinFromModel: false, netWinProbability: 0.9, netWinLower: 0.83, netExpectedBps: 2.4, netDecilesBps: null, outcomeTrials: 31, outcomeCensored: 2, medianHoldSecs: 210, medianWinSecs: 180, expectedProfitBps: 3.1, expectedProfitUsd: 4.2, optimalSizeUsd: 1200, breakevenSizeUsd: 3800, capacityLadder: [], expectedCapacityUsd: 2400, bpsCapacityLadder: [], depthCurve: [], fundingCarryBps: 0.4, fundingUnknown: false, firstSeenMs: now - 120_000, lastSeenMs: now, signalAgeSecs: 120, expectedConvergeSecs: 300, qualificationProgress: 1, observedEpisodes: 24, executable: true, blockReason: null, thresholdRatio: 1.8, coverage: 0.93, sessionId: "us_late", sessionProvisional: false, sessionAllow: 0.8, sessionSizeMult: 0.8 },
      { symbol: "SOL-USDC", buyVenue: "vanta", sellVenue: "binance", status: "tracking", currentEdgeBps: 3.2, baselineBps: 0.9, deviationBps: 2.3, netCaptureBps: 1.1, pWin: 0.95, reversionProbability: 0.64, winProbability: 0.95, winProbabilityLower: 0.86, winProbabilityUpper: 0.98, pWinNEffective: 11, pWinCensored: 0.12, pWinFromModel: false, netWinProbability: 0.88, netWinLower: 0.8, netExpectedBps: 0.9, netDecilesBps: null, outcomeTrials: 14, outcomeCensored: 2, medianHoldSecs: 420, medianWinSecs: 360, expectedProfitBps: 2.6, expectedProfitUsd: 1.4, optimalSizeUsd: 800, breakevenSizeUsd: 2100, capacityLadder: [], expectedCapacityUsd: 1300, bpsCapacityLadder: [], depthCurve: [], fundingCarryBps: 0.1, fundingUnknown: false, firstSeenMs: now - 600_000, lastSeenMs: now, signalAgeSecs: 600, expectedConvergeSecs: 900, qualificationProgress: 0.7, observedEpisodes: 11, executable: false, blockReason: "cooldown 4m", thresholdRatio: 1.2, coverage: 0.87, sessionId: "us_late", sessionProvisional: false, sessionAllow: 0.8, sessionSizeMult: 0.8 },
      { symbol: "BTC-USDC", buyVenue: "hyperliquid", sellVenue: "lighter", status: "preparing", currentEdgeBps: 1.4, baselineBps: 0.6, deviationBps: 0.8, netCaptureBps: 0.2, pWin: 0.52, reversionProbability: 0.55, winProbability: 0.52, winProbabilityLower: 0.33, winProbabilityUpper: 0.7, pWinNEffective: 8, pWinCensored: 0, pWinFromModel: true, netWinProbability: 0.48, netWinLower: 0.3, netExpectedBps: 0.3, netDecilesBps: null, outcomeTrials: 8, outcomeCensored: 0, medianHoldSecs: 300, medianWinSecs: 280, expectedProfitBps: 0.3, expectedProfitUsd: 0.6, optimalSizeUsd: 500, breakevenSizeUsd: 1500, capacityLadder: [], expectedCapacityUsd: 900, bpsCapacityLadder: [], depthCurve: [], fundingCarryBps: 0.2, fundingUnknown: false, firstSeenMs: now - 300_000, lastSeenMs: now, signalAgeSecs: 300, expectedConvergeSecs: 600, qualificationProgress: 0.4, observedEpisodes: 8, executable: false, blockReason: null, thresholdRatio: 0.8, coverage: 0.91, sessionId: "us_late", sessionProvisional: true, sessionAllow: 0.8, sessionSizeMult: 0.8 },
    ],
  }),
  get_monitor_config: () => ({
    enabled: true, watchlistRefreshSecs: 30, qualificationMode: "auto",
    manualWindowSecs: 86400, halfLifeMultiplier: 4, minWindowSecs: 3600,
    frictionCoverMult: 1.2, capacityTiersUsd: [500, 1000, 2500, 5000],
    minWinProbability: 0.5, executionMode: "two_way", participationCap: 0.1,
    txReservePct: 0.05, minEpisodes: 8, historyWindowSecs: 604800,
    maxSymbolIntervalSecs: 300, minSymbolIntervalSecs: 30,
    minExpectedProfitBps: 1.0, minThresholdRatio: 1.0, minCoverage: 0.8,
    episodeHalfLifeSecs: 86400, minWinLowerBound: 0.5,
  }),
  get_radar_bot_config: () => ({ enabled: false, size_usd: 500, max_concurrent: 3 }),
  get_radar_bot_status: () => ({ running: false, open_positions: 0, executed_today: 0, pnl_today_usd: 0 }),
  list_signal_executions: () => [],
  get_strategy_fill_pnl: () => ({ realized_usd: 0, fees_usd: 0, trades: 0 }),
  get_exempted_tickers: () => [],
  get_matrix_baselines: (a) => ({
    symbol: String(a?.symbol ?? "BTC-USDC"), mode: "auto",
    manual_window_secs: 86400, effective_window_secs: 43200, auto_window_secs: 43200,
    median_half_life_secs: 240, qualifying_pair_count: 6, max_history_span_secs: 604800,
    using_manual_fallback: false,
    pairs: [
      { buy_venue: "hyperliquid", sell_venue: "lighter", mean_gross_bps: 1.8, percentile_gross_bps: 4.2, sample_count: 12400, history_span_secs: 604800 },
      { buy_venue: "vanta", sell_venue: "binance", mean_gross_bps: 2.4, percentile_gross_bps: 5.1, sample_count: 9800, history_span_secs: 604800 },
      { buy_venue: "lighter", sell_venue: "hyperliquid", mean_gross_bps: 1.5, percentile_gross_bps: 3.8, sample_count: 11200, history_span_secs: 604800 },
    ],
  }),
  get_signal_series: (a) => {
    const pts = 60;
    const ts = Array.from({ length: pts }, (_, i) => now - (pts - i) * 60_000);
    return {
      symbol: String(a?.symbol ?? "BTC-USDC"), mode: "consensus", window_secs: 3600,
      sample_count: pts * 4, earliest_ts_ms: now - 7 * 86400_000, ts_ms: ts,
      venues: ["hyperliquid", "lighter", "vanta", "binance"],
      venue_names: { hyperliquid: "Hyperliquid", lighter: "Lighter", vanta: "Vanta", binance: "Binance Futures" },
      venue_last_ts_ms: { hyperliquid: now, lighter: now, vanta: now, binance: now },
      dev_bps: ts.map((t) => [Math.sin(t / 9e5) * 2, Math.sin(t / 9e5 + 1) * 2.4 - 1.2, Math.sin(t / 9e5 + 2) * 1.6 + 0.8, Math.sin(t / 9e5 + 3) * 1.1 - 0.4]),
      band_max_bps: ts.map((t) => Math.sin(t / 9e5) * 2 + 1),
      band_min_bps: ts.map((t) => Math.sin(t / 9e5 + 1) * 2.4 - 1.5),
    };
  },
  get_venue_execution_capabilities: () => VENUES.map((v) => ({
    venue: v.id, name: v.name, supportsLive: v.can_submit, hasCredentials: v.can_submit,
    liveReady: v.can_submit, submissionReady: v.can_submit,
    strategyEntryReady: v.can_enter_strategy, orderPollingReady: v.can_submit,
  })),
  get_storage_stats: () => ({ db_bytes: 685_000_000, sample_rows: 4_315_763, oldest_sample_ts_ms: now - 14 * 86400_000, newest_sample_ts_ms: now }),
  get_strategy_tickers: () => ["BTC", "ETH", "SOL", "DOGE", "AVAX", "LINK", "ARB", "SUI"].map((base) => ({ base, venues: ["hyperliquid", "lighter", "vanta", "binance"], passes: true, blockReason: null })),
  get_account_equity_events: () => [],
  get_external_positions: () => ({ positions: [], errors: [] }),
  get_ticker_strategy_config: () => null,
  get_strategy_params: () => null,
  get_utilisation_history: () => ({ points: [] }),
  get_strategy_performance: () => [
    { strategy_id: "futures", total_cycles_initiated: 20, total_cycles_completed: 16, winning_cycles: 9, losing_cycles: 7, win_rate_pct: 56.3, total_pnl_usd: 1842.55, avg_pnl_usd: 115.2, avg_pnl_bps: 2.1, max_drawdown_usd: 412.3, max_drawdown_pct: 1.6, sharpe_ratio: 1.84, annualized_apr_pct: 11.2, annualized_apy_pct: 11.8 },
    { strategy_id: "radar", total_cycles_initiated: 5, total_cycles_completed: 3, winning_cycles: 2, losing_cycles: 1, win_rate_pct: 66.7, total_pnl_usd: 7.4, avg_pnl_usd: 2.5, avg_pnl_bps: 1.9, max_drawdown_usd: 2.1, max_drawdown_pct: 0.3, sharpe_ratio: 2.2, annualized_apr_pct: 9.4, annualized_apy_pct: 9.9 },
  ],
  get_session_profile: (a) => ({
    symbol: String(a?.symbol ?? "BTC-USDC"), currentSession: "us_late",
    isWeekend: false, inFundingMask: false, usCashOpen: true,
    sessionAllow: 0.8, provisional: false, sessions: [], promotion: null,
  }),
};

const w = window as unknown as Record<string, unknown>;
w.__TAURI__ = { core: { invoke: true } };
w.__TAURI_INTERNALS__ = {
  invoke: async <T,>(cmd: string, args?: Record<string, unknown>): Promise<T> => {
    const f = FIXTURES[cmd];
    if (f) return f(args) as T;
    // Unmocked commands resolve to null — components already tolerate empty
    // data; logging keeps the missing fixture obvious during review.
    console.warn(`[mock] no fixture for ${cmd}`, args);
    return null as T;
  },
  transformCallback: () => 0,
  unregisterCallback: () => {},
  convertFileSrc: (p: string) => p,
  metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } },
};

await import("./main");
