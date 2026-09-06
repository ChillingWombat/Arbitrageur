import type {
  SystemStatusDto,
  VenueOrderBookDto,
  SpatialOpportunityDto,
  FundingOpportunityDto,
  PredictionOpportunityDto,
  StatArbStepResultDto,
  BertramThresholdDto,
  OuFitDto,
  RiskOverviewDto,
  SimStepResultDto,
  ExecutionLogEntry,
  EngineSettingsDto,
} from "../types";

// Check if running inside Tauri IPC environment
function isTauriEnvironment(): boolean {
  return typeof window !== "undefined" && ("__TAURI_INTERNALS__" in window || "__TAURI__" in window);
}

// Fallback simulated engine state for browser mode
class BrowserMockEngine {
  private pnl = 1420.5;
  private simCycle = 10;
  private isLive = false;
  private cbTripped = false;
  private settings: EngineSettingsDto = {
    hsm_url: "https://hsm-vault.internal.net:8443",
    hmac_token: "enc-vault-a9f82d-k92",
    max_slippage_bps: 10.0,
    min_depth_ratio: 2.5,
    robinhood_router: "0x1b44f3514812d835eb1bdb0acb33d3ff33f13353",
  };
  private logs: ExecutionLogEntry[] = [
    {
      id: "exec-1",
      timestamp: new Date(Date.now() - 60000).toISOString(),
      trade_type: "Spatial",
      symbol: "BTC-USDC",
      venue_a: "Hyperliquid",
      venue_b: "Lighter",
      leg_a_status: "FILLED",
      leg_b_status: "FILLED",
      success: true,
      gross_profit: 48.5,
      fees: 2.1,
      net_profit: 46.4,
      latency_us: 114,
      note: "Executed atomically at spread 18.2 BPS",
    },
    {
      id: "exec-2",
      timestamp: new Date(Date.now() - 30000).toISOString(),
      trade_type: "Funding",
      symbol: "ETH-USDC",
      venue_a: "Hyperliquid",
      venue_b: "Vanta.exchange",
      leg_a_status: "FILLED",
      leg_b_status: "FILLED",
      success: true,
      gross_profit: 32.0,
      fees: 1.8,
      net_profit: 30.2,
      latency_us: 98,
      note: "Carry opened: Short HL (0.03%/1h) + Long Vanta (0.015%/8h)",
    },
  ];

  // Kalman filter internal state
  private kfState = [0.0, 20.0]; // [alpha, beta]
  private kfVariance = 1.0;

  getSystemStatus(): SystemStatusDto {
    return {
      is_live_mode: this.isLive,
      total_realized_pnl: this.pnl,
      total_trades_count: this.logs.length,
      successful_trades_count: this.logs.filter((l) => l.success).length,
      circuit_breaker_status: this.cbTripped ? "TRIPPED: Drawdown limit reached" : "ACTIVE",
      circuit_breaker_tripped: this.cbTripped,
      current_drawdown_usd: 0.0,
      max_drawdown_usd: 250000.0,
      consecutive_failures: 0,
      active_venues: [
        { id: "hyperliquid", name: "Hyperliquid (L1 DEX)", market_type: "Perpetual CLOB", status: "ONLINE", latency_ms: 2, fee_bps: 2.0 },
        { id: "lighter", name: "Lighter (zkDEX)", market_type: "zk-Rollup CLOB", status: "ONLINE", latency_ms: 5, fee_bps: 1.5 },
        { id: "vanta", name: "Vanta.exchange", market_type: "Omnichain Perps", status: "ONLINE", latency_ms: 4, fee_bps: 3.0 },
        { id: "robinhood", name: "Robinhood EVM", market_type: "EVM DEX Aggregator", status: "ONLINE", latency_ms: 8, fee_bps: 2.5 },
        { id: "polymarket", name: "Polymarket (CTF CLOB)", market_type: "Prediction Market", status: "ONLINE", latency_ms: 12, fee_bps: 0.0 },
        { id: "kalshi", name: "Kalshi (CFTC)", market_type: "Prediction Market", status: "ONLINE", latency_ms: 15, fee_bps: 1.0 },
      ],
    };
  }

  fetchOrderbooks(symbol: string): VenueOrderBookDto[] {
    const basePrices: Record<string, number> = {
      "BTC-USDC": 64200.0,
      "ETH-USDC": 3450.0,
      "SOL-USDC": 152.2,
    };
    const base = basePrices[symbol] || 64200.0;
    const jitter = (Math.random() - 0.5) * (base * 0.0004);

    return [
      {
        venue_id: "hyperliquid",
        venue_name: "Hyperliquid (L1)",
        symbol,
        best_bid: base + jitter - 2.0,
        best_ask: base + jitter + 2.5,
        spread: 4.5,
        spread_bps: (4.5 / base) * 10000,
        mid_price: base + jitter + 0.25,
        total_bid_depth: 18.5,
        total_ask_depth: 22.0,
        vwap_bid: base + jitter - 2.8,
        vwap_ask: base + jitter + 3.2,
        funding_rate_8h_pct: 0.024,
        bids: [
          [base + jitter - 2.0, 2.5],
          [base + jitter - 4.5, 4.2],
          [base + jitter - 7.0, 5.8],
          [base + jitter - 10.0, 6.0],
        ],
        asks: [
          [base + jitter + 2.5, 3.1],
          [base + jitter + 5.0, 5.5],
          [base + jitter + 8.5, 6.4],
          [base + jitter + 12.0, 7.0],
        ],
      },
      {
        venue_id: "lighter",
        venue_name: "Lighter (zkDEX)",
        symbol,
        best_bid: base + jitter + 12.0,
        best_ask: base + jitter + 16.5,
        spread: 4.5,
        spread_bps: (4.5 / base) * 10000,
        mid_price: base + jitter + 14.25,
        total_bid_depth: 14.2,
        total_ask_depth: 19.8,
        vwap_bid: base + jitter + 11.2,
        vwap_ask: base + jitter + 17.5,
        funding_rate_8h_pct: 0.008,
        bids: [
          [base + jitter + 12.0, 1.8],
          [base + jitter + 9.5, 3.4],
          [base + jitter + 7.0, 4.5],
          [base + jitter + 4.0, 4.5],
        ],
        asks: [
          [base + jitter + 16.5, 2.9],
          [base + jitter + 19.0, 4.8],
          [base + jitter + 22.0, 5.6],
          [base + jitter + 25.0, 6.5],
        ],
      },
      {
        venue_id: "vanta",
        venue_name: "Vanta.exchange",
        symbol,
        best_bid: base + jitter + 3.0,
        best_ask: base + jitter + 8.0,
        spread: 5.0,
        spread_bps: (5.0 / base) * 10000,
        mid_price: base + jitter + 5.5,
        total_bid_depth: 25.0,
        total_ask_depth: 27.5,
        vwap_bid: base + jitter + 2.1,
        vwap_ask: base + jitter + 9.2,
        funding_rate_8h_pct: 0.012,
        bids: [
          [base + jitter + 3.0, 3.5],
          [base + jitter + 0.5, 6.0],
          [base + jitter - 2.0, 7.5],
          [base + jitter - 5.0, 8.0],
        ],
        asks: [
          [base + jitter + 8.0, 4.2],
          [base + jitter + 11.0, 6.8],
          [base + jitter + 14.5, 7.5],
          [base + jitter + 18.0, 9.0],
        ],
      },
      {
        venue_id: "robinhood",
        venue_name: "Robinhood EVM",
        symbol,
        best_bid: base + jitter + 1.0,
        best_ask: base + jitter + 6.5,
        spread: 5.5,
        spread_bps: (5.5 / base) * 10000,
        mid_price: base + jitter + 3.75,
        total_bid_depth: 12.0,
        total_ask_depth: 15.0,
        vwap_bid: base + jitter + 0.2,
        vwap_ask: base + jitter + 7.8,
        funding_rate_8h_pct: 0.007,
        bids: [
          [base + jitter + 1.0, 1.5],
          [base + jitter - 1.5, 3.2],
          [base + jitter - 4.0, 4.3],
          [base + jitter - 7.0, 3.0],
        ],
        asks: [
          [base + jitter + 6.5, 2.2],
          [base + jitter + 9.5, 3.8],
          [base + jitter + 13.0, 4.5],
          [base + jitter + 17.0, 4.5],
        ],
      },
    ];
  }

  scanSpatialOpportunities(symbol: string, minBps: number, orderQty: number): SpatialOpportunityDto[] {
    const books = this.fetchOrderbooks(symbol);
    const opps: SpatialOpportunityDto[] = [];

    for (let i = 0; i < books.length; i++) {
      for (let j = 0; j < books.length; j++) {
        if (i === j) continue;
        const buy = books[i];
        const sell = books[j];

        if (sell.best_bid > buy.best_ask) {
          const spread = sell.best_bid - buy.best_ask;
          const grossBps = (spread / buy.best_ask) * 10000;
          const feeBps = 3.5;
          const netBps = grossBps - feeBps;
          const estProfit = spread * orderQty - (buy.best_ask * orderQty * feeBps) / 10000;

          opps.push({
            id: `${symbol}-${buy.venue_id}-${sell.venue_id}`,
            symbol,
            buy_venue: buy.venue_name,
            sell_venue: sell.venue_name,
            buy_price: buy.best_ask,
            sell_price: sell.best_bid,
            gross_spread: spread,
            gross_spread_bps: grossBps,
            fee_bps: feeBps,
            net_spread_bps: netBps,
            order_qty: orderQty,
            estimated_profit_usd: estProfit,
            is_executable: netBps >= minBps,
          });
        }
      }
    }

    return opps.sort((a, b) => b.net_spread_bps - a.net_spread_bps);
  }

  scanFundingOpportunities(symbol: string, tradeSizeUsd: number): FundingOpportunityDto[] {
    return [
      {
        id: `${symbol}-Lighter-Hyperliquid`,
        symbol,
        long_venue: "Lighter",
        short_venue: "Hyperliquid",
        net_rate_8h_pct: 0.021,
        net_apr_pct: 22.99,
        net_apy_pct: 25.84,
        round_trip_fee_bps: 3.5,
        break_even_hours: 13.3,
        estimated_daily_income_usd: (tradeSizeUsd * 0.00021 * 3),
        trade_size_usd: tradeSizeUsd,
      },
      {
        id: `${symbol}-Robinhood-Hyperliquid`,
        symbol,
        long_venue: "Robinhood EVM",
        short_venue: "Hyperliquid",
        net_rate_8h_pct: 0.017,
        net_apr_pct: 18.61,
        net_apy_pct: 20.45,
        round_trip_fee_bps: 3.5,
        break_even_hours: 16.5,
        estimated_daily_income_usd: (tradeSizeUsd * 0.00017 * 3),
        trade_size_usd: tradeSizeUsd,
      },
      {
        id: `${symbol}-Vanta-Hyperliquid`,
        symbol,
        long_venue: "Vanta.exchange",
        short_venue: "Hyperliquid",
        net_rate_8h_pct: 0.012,
        net_apr_pct: 13.14,
        net_apy_pct: 14.04,
        round_trip_fee_bps: 3.5,
        break_even_hours: 23.3,
        estimated_daily_income_usd: (tradeSizeUsd * 0.00012 * 3),
        trade_size_usd: tradeSizeUsd,
      },
    ];
  }

  scanPredictionOpportunities(market: string): PredictionOpportunityDto[] {
    const list: PredictionOpportunityDto[] = [
      {
        id: "FED-RATE-CUT-MAY-Poly-Kalshi",
        market_title: "FED-RATE-CUT-MAY",
        venue_a: "Polymarket",
        venue_b: "Kalshi",
        strategy: "Synthetic Binary Parity",
        buy_yes_venue: "Polymarket",
        buy_no_venue: "Kalshi",
        yes_price_cents: 44.0,
        no_price_cents: 51.0,
        total_cost_cents: 95.0,
        guaranteed_payout_cents: 100.0,
        net_profit_cents: 5.0,
        roi_pct: 5.26,
        has_arbitrage: true,
      },
      {
        id: "BTC-OVER-70K-Q2-Poly-Kalshi",
        market_title: "BTC-OVER-70K-Q2",
        venue_a: "Polymarket",
        venue_b: "Kalshi",
        strategy: "Synthetic Binary Parity",
        buy_yes_venue: "Kalshi",
        buy_no_venue: "Polymarket",
        yes_price_cents: 58.0,
        no_price_cents: 38.0,
        total_cost_cents: 96.0,
        guaranteed_payout_cents: 100.0,
        net_profit_cents: 4.0,
        roi_pct: 4.17,
        has_arbitrage: true,
      },
      {
        id: "ETH-ETF-INFLOW-Poly-Kalshi",
        market_title: "ETH-ETF-NET-INFLOW",
        venue_a: "Polymarket",
        venue_b: "Kalshi",
        strategy: "Synthetic Binary Parity",
        buy_yes_venue: "Polymarket",
        buy_no_venue: "Kalshi",
        yes_price_cents: 35.0,
        no_price_cents: 61.0,
        total_cost_cents: 96.0,
        guaranteed_payout_cents: 100.0,
        net_profit_cents: 4.0,
        roi_pct: 4.17,
        has_arbitrage: true,
      },
    ];

    if (!market) return list;
    return list.filter((p) => p.market_title.toLowerCase().includes(market.toLowerCase()));
  }

  runStatArbStep(step: number, priceY: number, priceX: number): StatArbStepResultDto {
    const yHat = this.kfState[0] + this.kfState[1] * priceX;
    const residual = priceY - yHat;
    this.kfVariance = 0.95 * this.kfVariance + 0.05 * (residual * residual);
    const z = residual / Math.sqrt(this.kfVariance || 1.0);

    // Minor update to beta
    this.kfState[1] += 0.0001 * (residual / (priceX || 1.0));

    let signal: StatArbStepResultDto["signal"] = "NEUTRAL";
    if (z > 2.0) signal = "ENTRY_SHORT_SPREAD";
    else if (z < -2.0) signal = "ENTRY_LONG_SPREAD";
    else if (Math.abs(z) < 0.5) signal = "EXIT";

    return {
      step,
      price_y: priceY,
      price_x: priceX,
      dynamic_hedge_ratio: this.kfState[1],
      intercept_alpha: this.kfState[0],
      residual_spread: residual,
      spread_variance: this.kfVariance,
      z_score: z,
      signal,
    };
  }

  computeBertramThresholds(theta: number, mu: number, sigma: number, transactionCost: number): BertramThresholdDto {
    const halfLife = theta > 0 ? Math.log(2) / theta : 0;
    const statStd = theta > 0 ? Math.sqrt((sigma * sigma) / (2 * theta)) : 0.01;
    const optDist = Math.max(transactionCost * 1.5, statStd * 1.6);

    return {
      entry_long: mu - optDist,
      exit_long: mu + 0.05 * optDist,
      entry_short: mu + optDist,
      exit_short: mu - 0.05 * optDist,
      optimal_distance: optDist,
      expected_return_per_cycle: (optDist - transactionCost) * 100,
      estimated_trades_per_year: theta * 45,
      estimated_sharpe: 2.85,
      half_life: halfLife,
      theta,
      mu,
      sigma,
    };
  }

  fitOuProcess(prices: number[], dt: number): OuFitDto {
    const n = prices.length;
    let sumX = 0, sumY = 0, sumXX = 0, sumXY = 0;
    for (let i = 0; i < n - 1; i++) {
      const x = prices[i];
      const y = prices[i + 1];
      sumX += x; sumY += y; sumXX += x * x; sumXY += x * y;
    }
    const m = n - 1;
    const a = (m * sumXY - sumX * sumY) / (m * sumXX - sumX * sumX);
    const theta = -Math.log(Math.max(0.001, a)) / dt;
    const mu = (sumY - a * sumX) / (m * (1 - a));

    return {
      theta,
      mu,
      sigma: 0.045,
      half_life: Math.log(2) / (theta || 0.1),
      stationary_std: 0.082,
      r_squared: 0.91,
    };
  }

  runSimStep(symbol: string, targetBps: number, orderQty: number): SimStepResultDto {
    this.simCycle += 1;
    const basePrices: Record<string, number> = {
      "BTC-USDC": 64200.0,
      "ETH-USDC": 3450.0,
      "SOL-USDC": 152.2,
    };
    const base = basePrices[symbol] || 64200.0;
    const spreadBps = targetBps + 4.5 + Math.random() * 5.0;
    const profit = (spreadBps / 10000) * base * orderQty - 2.1;
    this.pnl += profit;

    const log: ExecutionLogEntry = {
      id: `exec-${this.simCycle}`,
      timestamp: new Date().toISOString(),
      trade_type: "Spatial",
      symbol,
      venue_a: "Hyperliquid",
      venue_b: "Lighter",
      leg_a_status: "FILLED",
      leg_b_status: "FILLED",
      success: true,
      gross_profit: profit + 2.1,
      fees: 2.1,
      net_profit: profit,
      latency_us: Math.floor(100 + Math.random() * 40),
      note: `Atomically captured spread of ${spreadBps.toFixed(1)} BPS (+${profit.toFixed(2)})`,
    };
    this.logs.unshift(log);

    return {
      cycle: this.simCycle,
      opportunity_found: true,
      execution_success: true,
      trade_type: "Spatial Arb",
      direction: "BuyASellB",
      venue_a: "Hyperliquid",
      venue_b: "Lighter",
      spread_bps: spreadBps,
      realized_profit_usd: profit,
      total_cumulative_pnl_usd: this.pnl,
      message: `Atomically captured spread of ${spreadBps.toFixed(2)} BPS (+$${profit.toFixed(2)})`,
    };
  }

  executeSpatialArbitrage(symbol: string, buyVenue: string, sellVenue: string, orderQty: number): SimStepResultDto {
    this.simCycle += 1;
    const basePrices: Record<string, number> = {
      "BTC-USDC": 64200.0,
      "ETH-USDC": 3450.0,
      "SOL-USDC": 152.2,
    };
    const base = basePrices[symbol] || 64200.0;
    const spreadBps = 14.8 + Math.random() * 5.2;
    const grossProfit = (spreadBps / 10000) * base * orderQty;
    const fees = 2.1;
    const netProfit = grossProfit - fees;
    this.pnl += netProfit;

    const log: ExecutionLogEntry = {
      id: `exec-${this.simCycle}`,
      timestamp: new Date().toISOString(),
      trade_type: "Spatial",
      symbol,
      venue_a: buyVenue,
      venue_b: sellVenue,
      leg_a_status: "FILLED",
      leg_b_status: "FILLED",
      success: true,
      gross_profit: grossProfit,
      fees,
      net_profit: netProfit,
      latency_us: Math.floor(95 + Math.random() * 30),
      note: `Spatial captured: Buy ${buyVenue} / Sell ${sellVenue} (+${netProfit.toFixed(2)} USD)`,
    };
    this.logs.unshift(log);

    return {
      cycle: this.simCycle,
      opportunity_found: true,
      execution_success: true,
      trade_type: "Spatial Arb",
      direction: `Buy ${buyVenue} -> Sell ${sellVenue}`,
      venue_a: buyVenue,
      venue_b: sellVenue,
      spread_bps: spreadBps,
      realized_profit_usd: netProfit,
      total_cumulative_pnl_usd: this.pnl,
      message: `Executed atomic spatial arb on ${symbol} between ${buyVenue} and ${sellVenue}. Net profit: $${netProfit.toFixed(2)}`,
    };
  }

  executeFundingCarry(symbol: string, longVenue: string, shortVenue: string, tradeSizeUsd: number): SimStepResultDto {
    this.simCycle += 1;
    const netProfit = tradeSizeUsd * 0.00018 * 3;
    this.pnl += netProfit;

    const log: ExecutionLogEntry = {
      id: `exec-${this.simCycle}`,
      timestamp: new Date().toISOString(),
      trade_type: "Funding",
      symbol,
      venue_a: longVenue,
      venue_b: shortVenue,
      leg_a_status: "FILLED",
      leg_b_status: "FILLED",
      success: true,
      gross_profit: netProfit + 1.8,
      fees: 1.8,
      net_profit: netProfit,
      latency_us: Math.floor(80 + Math.random() * 25),
      note: `Carry opened: Long ${longVenue} / Short ${shortVenue} for ${symbol}`,
    };
    this.logs.unshift(log);

    return {
      cycle: this.simCycle,
      opportunity_found: true,
      execution_success: true,
      trade_type: "Funding Carry",
      direction: `Long ${longVenue} + Short ${shortVenue}`,
      venue_a: longVenue,
      venue_b: shortVenue,
      spread_bps: 18.5,
      realized_profit_usd: netProfit,
      total_cumulative_pnl_usd: this.pnl,
      message: `Opened delta-neutral funding carry position of $${tradeSizeUsd.toFixed(2)} on ${symbol}`,
    };
  }

  executePredictionArbitrage(marketTitle: string, buyYesVenue: string, buyNoVenue: string, quantity: number): SimStepResultDto {
    this.simCycle += 1;
    const profitPerContract = 0.045;
    const netProfit = profitPerContract * quantity;
    this.pnl += netProfit;

    const log: ExecutionLogEntry = {
      id: `exec-${this.simCycle}`,
      timestamp: new Date().toISOString(),
      trade_type: "Prediction",
      symbol: marketTitle,
      venue_a: buyYesVenue,
      venue_b: buyNoVenue,
      leg_a_status: "FILLED",
      leg_b_status: "FILLED",
      success: true,
      gross_profit: netProfit,
      fees: 0.0,
      net_profit: netProfit,
      latency_us: Math.floor(120 + Math.random() * 40),
      note: `Synthetic parity filled: Buy YES (${buyYesVenue}) + Buy NO (${buyNoVenue})`,
    };
    this.logs.unshift(log);

    return {
      cycle: this.simCycle,
      opportunity_found: true,
      execution_success: true,
      trade_type: "Prediction Parity",
      direction: "ArrowDebreu",
      venue_a: buyYesVenue,
      venue_b: buyNoVenue,
      spread_bps: 450.0,
      realized_profit_usd: netProfit,
      total_cumulative_pnl_usd: this.pnl,
      message: `Executed atomic parity trade on ${marketTitle}. Locked in payout with profit $${netProfit.toFixed(2)}`,
    };
  }

  getRiskOverview(): RiskOverviewDto {
    return {
      circuit_breaker_state: this.cbTripped ? "TRIPPED: Drawdown halt" : "ACTIVE",
      is_halted: this.cbTripped,
      current_drawdown_usd: 0.0,
      max_drawdown_limit_usd: 25000.0,
      drawdown_pct: 0.0,
      consecutive_failures: 0,
      max_failures_allowed: 5,
      total_portfolio_value_usd: 400000.0 + this.pnl,
      net_deltas: [
        { asset: "BTC", net_quantity: 0.0, net_exposure_usd: 0.0, delta_limit_usd: 2500.0, is_neutral: true },
        { asset: "ETH", net_quantity: 0.0, net_exposure_usd: 0.0, delta_limit_usd: 2500.0, is_neutral: true },
        { asset: "SOL", net_quantity: 0.0, net_exposure_usd: 0.0, delta_limit_usd: 2500.0, is_neutral: true },
        { asset: "USDC", net_quantity: 400000.0, net_exposure_usd: 400000.0, delta_limit_usd: 500000.0, is_neutral: true },
      ],
      margin_health: {
        maintenance_margin_req_usd: 12500.0,
        margin_balance_usd: 387500.0,
        margin_ratio_pct: 3.22,
        liquidation_distance_pct: 78.5,
        status: "HEALTHY",
      },
    };
  }

  getEngineSettings(): EngineSettingsDto {
    return { ...this.settings };
  }

  saveEngineSettings(settings: EngineSettingsDto): string {
    if (settings.max_slippage_bps <= 0 || settings.max_slippage_bps > 500) {
      throw new Error("Max slippage tolerance must be between 0.1 and 500 BPS");
    }
    if (settings.min_depth_ratio < 1.0) {
      throw new Error("Min depth ratio must be at least 1.0");
    }
    this.settings = { ...settings };
    return "Configuration successfully applied and persistent in runtime engine";
  }

  resetCircuitBreaker(): string {
    this.cbTripped = false;
    return "Circuit Breaker reset successfully. Trading resumed.";
  }

  triggerKillswitch(): string {
    this.cbTripped = true;
    return "EMERGENCY KILLSWITCH ENGAGED. All trading operations halted immediately.";
  }

  getExecutionLogs(): ExecutionLogEntry[] {
    return this.logs;
  }

  setLiveMode(enabled: boolean): boolean {
    this.isLive = enabled;
    return this.isLive;
  }
}

const mockEngine = new BrowserMockEngine();

// Dynamic loader for Tauri core IPC
let tauriCore: { invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T> } | null = null;

async function getTauriCore() {
  if (tauriCore) return tauriCore;
  if (isTauriEnvironment()) {
    try {
      const core = await import("@tauri-apps/api/core");
      tauriCore = core;
      return tauriCore;
    } catch (e) {
      console.warn("Could not load @tauri-apps/api/core, falling back to mock:", e);
    }
  }
  return null;
}

export const api = {
  get isTauri(): boolean {
    return isTauriEnvironment();
  },

  async getSystemStatus(): Promise<SystemStatusDto> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<SystemStatusDto>("get_system_status");
    return mockEngine.getSystemStatus();
  },

  async fetchOrderbooks(symbol: string): Promise<VenueOrderBookDto[]> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<VenueOrderBookDto[]>("fetch_orderbooks", { symbol });
    return mockEngine.fetchOrderbooks(symbol);
  },

  async scanSpatialOpportunities(symbol: string, minBps: number, orderQty: number): Promise<SpatialOpportunityDto[]> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<SpatialOpportunityDto[]>("scan_spatial_opportunities", { symbol, minBps, orderQty });
    return mockEngine.scanSpatialOpportunities(symbol, minBps, orderQty);
  },

  async scanFundingOpportunities(symbol: string, tradeSizeUsd: number): Promise<FundingOpportunityDto[]> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<FundingOpportunityDto[]>("scan_funding_opportunities", { symbol, tradeSizeUsd });
    return mockEngine.scanFundingOpportunities(symbol, tradeSizeUsd);
  },

  async scanPredictionOpportunities(market: string): Promise<PredictionOpportunityDto[]> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<PredictionOpportunityDto[]>("scan_prediction_opportunities", { market });
    return mockEngine.scanPredictionOpportunities(market);
  },

  async runStatArbStep(step: number, priceY: number, priceX: number): Promise<StatArbStepResultDto> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<StatArbStepResultDto>("run_stat_arb_step", { step, priceY, priceX });
    return mockEngine.runStatArbStep(step, priceY, priceX);
  },

  async computeOptimalThresholds(
    theta: number,
    mu: number,
    sigma: number,
    transactionCost: number
  ): Promise<BertramThresholdDto> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<BertramThresholdDto>("compute_optimal_thresholds", { theta, mu, sigma, transactionCost });
    return mockEngine.computeBertramThresholds(theta, mu, sigma, transactionCost);
  },

  async fitOuProcess(prices: number[], dt: number): Promise<OuFitDto> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<OuFitDto>("fit_ou_process", { prices, dt });
    return mockEngine.fitOuProcess(prices, dt);
  },

  async runSimulationStep(symbol: string, targetBps: number, orderQty: number): Promise<SimStepResultDto> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<SimStepResultDto>("run_simulation_step", { symbol, targetBps, orderQty });
    return mockEngine.runSimStep(symbol, targetBps, orderQty);
  },

  async executeSpatialArbitrage(symbol: string, buyVenue: string, sellVenue: string, orderQty: number): Promise<SimStepResultDto> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<SimStepResultDto>("execute_spatial_arbitrage", { symbol, buyVenue, sellVenue, orderQty });
    return mockEngine.executeSpatialArbitrage(symbol, buyVenue, sellVenue, orderQty);
  },

  async executeFundingCarry(symbol: string, longVenue: string, shortVenue: string, tradeSizeUsd: number): Promise<SimStepResultDto> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<SimStepResultDto>("execute_funding_carry", { symbol, longVenue, shortVenue, tradeSizeUsd });
    return mockEngine.executeFundingCarry(symbol, longVenue, shortVenue, tradeSizeUsd);
  },

  async executePredictionArbitrage(marketTitle: string, buyYesVenue: string, buyNoVenue: string, quantity: number): Promise<SimStepResultDto> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<SimStepResultDto>("execute_prediction_arbitrage", { marketTitle, buyYesVenue, buyNoVenue, quantity });
    return mockEngine.executePredictionArbitrage(marketTitle, buyYesVenue, buyNoVenue, quantity);
  },

  async getEngineSettings(): Promise<EngineSettingsDto> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<EngineSettingsDto>("get_engine_settings");
    return mockEngine.getEngineSettings();
  },

  async saveEngineSettings(settings: EngineSettingsDto): Promise<string> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<string>("save_engine_settings", { settings });
    return mockEngine.saveEngineSettings(settings);
  },

  async getRiskOverview(): Promise<RiskOverviewDto> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<RiskOverviewDto>("get_risk_overview");
    return mockEngine.getRiskOverview();
  },

  async resetCircuitBreaker(): Promise<string> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<string>("reset_circuit_breaker");
    return mockEngine.resetCircuitBreaker();
  },

  async triggerKillswitch(): Promise<string> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<string>("trigger_emergency_killswitch");
    return mockEngine.triggerKillswitch();
  },

  async getExecutionLogs(): Promise<ExecutionLogEntry[]> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<ExecutionLogEntry[]>("get_execution_logs");
    return mockEngine.getExecutionLogs();
  },

  async setLiveMode(enabled: boolean): Promise<boolean> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<boolean>("set_live_mode", { enabled });
    return mockEngine.setLiveMode(enabled);
  },
};

