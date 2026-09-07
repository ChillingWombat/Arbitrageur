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
  CrossTickerExecutionParams,
  TickerMarketQuoteDto,
} from "../types";

import { PERP_TICKERS } from "../types/tickers";

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
    hsm_url: "http://127.0.0.1:8787/sign",
    hmac_token: "••••••••••••••••••••••••",
    max_slippage_bps: 15.0,
    min_depth_ratio: 3.0,
    hyperliquid_wallet: "",
    hyperliquid_private_key: "",
    hyperliquid_url: "https://api.hyperliquid.xyz",
    lighter_account_index: "0",
    lighter_api_key: "",
    lighter_url: "https://mainnet.zklighter.elliot.ai",
    vanta_account_id: "",
    vanta_orderly_key: "",
    vanta_orderly_secret: "",
    vanta_url: "https://api-evm.orderly.org",
    robinhood_wallet: "",
    robinhood_private_key: "",
    robinhood_rpc_url: "https://rpc.robinhood.network",
    robinhood_router: "0x89e5DB8B5aA49aA85AC63f691524311AEB649eba",
    polymarket_api_key: "",
    polymarket_secret: "",
    polymarket_passphrase: "",
    polymarket_url: "https://clob.polymarket.com",
    kalshi_api_key_id: "",
    kalshi_private_key: "",
    kalshi_url: "https://trading-api.kalshi.com",
  };
  private logs: ExecutionLogEntry[] = [
    {
      id: "exec-1",
      timestamp: new Date(Date.now() - 360000).toISOString(),
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
      timestamp: new Date(Date.now() - 300000).toISOString(),
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
    {
      id: "exec-3",
      timestamp: new Date(Date.now() - 240000).toISOString(),
      trade_type: "Spatial",
      symbol: "BTC-USDC",
      venue_a: "Hyperliquid",
      venue_b: "Robinhood EVM",
      leg_a_status: "FILLED",
      leg_b_status: "FILLED",
      success: true,
      gross_profit: 41.0,
      fees: 1.9,
      net_profit: 39.1,
      latency_us: 120,
      note: "Atomic multi-leg routing on BTC basis dislocation",
    },
    {
      id: "exec-4",
      timestamp: new Date(Date.now() - 180000).toISOString(),
      trade_type: "Spatial",
      symbol: "SOL-USDC",
      venue_a: "Hyperliquid",
      venue_b: "Lighter",
      leg_a_status: "FILLED",
      leg_b_status: "FILLED",
      success: true,
      gross_profit: 20.0,
      fees: 1.8,
      net_profit: 18.2,
      latency_us: 105,
      note: "Executed atomically at spread 16.4 BPS",
    },
    {
      id: "exec-5",
      timestamp: new Date(Date.now() - 150000).toISOString(),
      trade_type: "Funding",
      symbol: "ETH-USDC",
      venue_a: "Hyperliquid",
      venue_b: "Lighter",
      leg_a_status: "FILLED",
      leg_b_status: "FILLED",
      success: true,
      gross_profit: 13.9,
      fees: 1.8,
      net_profit: 12.1,
      latency_us: 88,
      note: "Carry opened: Short HL + Long LGT",
    },
    {
      id: "exec-6",
      timestamp: new Date(Date.now() - 120000).toISOString(),
      trade_type: "Spatial",
      symbol: "SUI-USDC",
      venue_a: "Hyperliquid",
      venue_b: "Lighter",
      leg_a_status: "FILLED",
      leg_b_status: "FILLED",
      success: true,
      gross_profit: 12.0,
      fees: 1.7,
      net_profit: 10.3,
      latency_us: 92,
      note: "Sui perp dislocation captured",
    },
    {
      id: "exec-7",
      timestamp: new Date(Date.now() - 90000).toISOString(),
      trade_type: "Spatial",
      symbol: "AVAX-USDC",
      venue_a: "Hyperliquid",
      venue_b: "Vanta.exchange",
      leg_a_status: "FILLED",
      leg_b_status: "FILLED",
      success: true,
      gross_profit: 7.5,
      fees: 1.1,
      net_profit: 6.4,
      latency_us: 110,
      note: "AVAX perp basis captured",
    },
    {
      id: "exec-8",
      timestamp: new Date(Date.now() - 70000).toISOString(),
      trade_type: "Spatial",
      symbol: "SOL-USDC",
      venue_a: "Hyperliquid",
      venue_b: "Vanta.exchange",
      leg_a_status: "FILLED",
      leg_b_status: "FILLED",
      success: true,
      gross_profit: 7.7,
      fees: 1.8,
      net_profit: 5.9,
      latency_us: 102,
      note: "Solana cross-venue basis capture",
    },
    {
      id: "exec-9",
      timestamp: new Date(Date.now() - 50000).toISOString(),
      trade_type: "Spatial",
      symbol: "SUI-USDC",
      venue_a: "Hyperliquid",
      venue_b: "Vanta.exchange",
      leg_a_status: "FILLED",
      leg_b_status: "FILLED",
      success: true,
      gross_profit: 6.8,
      fees: 1.3,
      net_profit: 5.5,
      latency_us: 95,
      note: "Sui liquidity arb captured",
    },
    {
      id: "exec-10",
      timestamp: new Date(Date.now() - 30000).toISOString(),
      trade_type: "Spatial",
      symbol: "ARB-USDC",
      venue_a: "Hyperliquid",
      venue_b: "Lighter",
      leg_a_status: "FILLED",
      leg_b_status: "REJECTED",
      success: false,
      gross_profit: 0.0,
      fees: 0.5,
      net_profit: -0.5,
      latency_us: 130,
      note: "DeltaGuard safety unwind on Leg B rejection",
    },
    {
      id: "exec-11",
      timestamp: new Date(Date.now() - 20000).toISOString(),
      trade_type: "Spatial",
      symbol: "ARB-USDC",
      venue_a: "Hyperliquid",
      venue_b: "Vanta.exchange",
      leg_a_status: "FILLED",
      leg_b_status: "REJECTED",
      success: false,
      gross_profit: 0.0,
      fees: 0.3,
      net_profit: -0.3,
      latency_us: 125,
      note: "Slippage tolerance boundary reached, position cleanly unwound",
    },
    {
      id: "exec-12",
      timestamp: new Date(Date.now() - 10000).toISOString(),
      trade_type: "Spatial",
      symbol: "DOGE-USDC",
      venue_a: "Hyperliquid",
      venue_b: "Lighter",
      leg_a_status: "FILLED",
      leg_b_status: "REJECTED",
      success: false,
      gross_profit: 0.0,
      fees: 1.5,
      net_profit: -1.5,
      latency_us: 145,
      note: "Emergency unwind on venue latency spike",
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
    const ticker = PERP_TICKERS.find((t) => t.symbol === symbol);
    const base = ticker ? ticker.basePrice : 100.0;
    const jitter = (Math.random() - 0.5) * (base * 0.0003);

    const hlSpreadHalf = Math.max(0.000001, base * 0.00012);
    const hlBid = base + jitter - hlSpreadHalf;
    const hlAsk = base + jitter + hlSpreadHalf;

    const lgtDisloc = base * 0.00026;
    const lgtBid = base + jitter + lgtDisloc - hlSpreadHalf;
    const lgtAsk = base + jitter + lgtDisloc + hlSpreadHalf;

    const vntDisloc = base * 0.00008;
    const vntBid = base + jitter + vntDisloc - hlSpreadHalf * 1.1;
    const vntAsk = base + jitter + vntDisloc + hlSpreadHalf * 1.1;

    const rhDisloc = base * 0.00005;
    const rhBid = base + jitter + rhDisloc - hlSpreadHalf * 1.2;
    const rhAsk = base + jitter + rhDisloc + hlSpreadHalf * 1.2;

    const mkBids = (best: number, stepBps: number): [number, number][] => [
      [best, 5.0],
      [best * (1 - stepBps * 0.0001), 8.5],
      [best * (1 - stepBps * 0.0002), 12.0],
      [best * (1 - stepBps * 0.0003), 15.0],
    ];

    const mkAsks = (best: number, stepBps: number): [number, number][] => [
      [best, 5.2],
      [best * (1 + stepBps * 0.0001), 9.0],
      [best * (1 + stepBps * 0.0002), 13.5],
      [best * (1 + stepBps * 0.0003), 16.0],
    ];

    return [
      {
        venue_id: "hyperliquid",
        venue_name: "Hyperliquid (L1)",
        symbol,
        best_bid: hlBid,
        best_ask: hlAsk,
        spread: hlAsk - hlBid,
        spread_bps: ((hlAsk - hlBid) / base) * 10000,
        mid_price: (hlBid + hlAsk) / 2,
        total_bid_depth: 40.5,
        total_ask_depth: 43.7,
        vwap_bid: hlBid * 0.9998,
        vwap_ask: hlAsk * 1.0002,
        funding_rate_8h_pct: 0.024,
        bids: mkBids(hlBid, 1.5),
        asks: mkAsks(hlAsk, 1.5),
      },
      {
        venue_id: "lighter",
        venue_name: "Lighter (zkDEX)",
        symbol,
        best_bid: lgtBid,
        best_ask: lgtAsk,
        spread: lgtAsk - lgtBid,
        spread_bps: ((lgtAsk - lgtBid) / base) * 10000,
        mid_price: (lgtBid + lgtAsk) / 2,
        total_bid_depth: 35.0,
        total_ask_depth: 38.0,
        vwap_bid: lgtBid * 0.9998,
        vwap_ask: lgtAsk * 1.0002,
        funding_rate_8h_pct: 0.008,
        bids: mkBids(lgtBid, 1.5),
        asks: mkAsks(lgtAsk, 1.5),
      },
      {
        venue_id: "vanta",
        venue_name: "Vanta.exchange",
        symbol,
        best_bid: vntBid,
        best_ask: vntAsk,
        spread: vntAsk - vntBid,
        spread_bps: ((vntAsk - vntBid) / base) * 10000,
        mid_price: (vntBid + vntAsk) / 2,
        total_bid_depth: 45.0,
        total_ask_depth: 48.0,
        vwap_bid: vntBid * 0.9997,
        vwap_ask: vntAsk * 1.0003,
        funding_rate_8h_pct: 0.012,
        bids: mkBids(vntBid, 1.8),
        asks: mkAsks(vntAsk, 1.8),
      },
      {
        venue_id: "robinhood",
        venue_name: "Robinhood EVM",
        symbol,
        best_bid: rhBid,
        best_ask: rhAsk,
        spread: rhAsk - rhBid,
        spread_bps: ((rhAsk - rhBid) / base) * 10000,
        mid_price: (rhBid + rhAsk) / 2,
        total_bid_depth: 28.0,
        total_ask_depth: 32.0,
        vwap_bid: rhBid * 0.9996,
        vwap_ask: rhAsk * 1.0004,
        funding_rate_8h_pct: 0.007,
        bids: mkBids(rhBid, 2.0),
        asks: mkAsks(rhAsk, 2.0),
      },
    ];
  }

  fetchMultiMarketQuotes(symbols: string[]): TickerMarketQuoteDto[] {
    const results: TickerMarketQuoteDto[] = [];
    for (const sym of symbols) {
      const books = this.fetchOrderbooks(sym);
      const ticker = PERP_TICKERS.find((t) => t.symbol === sym);
      const basePrice = ticker ? ticker.basePrice : 100.0;
      const baseStr = sym.split("-")[0];

      let bestBid = -Infinity;
      let bestBidVenue = "Hyperliquid";
      let bestAsk = Infinity;
      let bestAskVenue = "Hyperliquid";
      let bestBuySpreadBps = 2.0;

      for (const b of books) {
        if (b.best_bid > bestBid) {
          bestBid = b.best_bid;
          bestBidVenue = b.venue_name;
        }
        if (b.best_ask < bestAsk && b.best_ask > 0) {
          bestAsk = b.best_ask;
          bestAskVenue = b.venue_name;
          bestBuySpreadBps = b.spread_bps > 0 ? b.spread_bps : ((b.best_ask - b.best_bid) / b.mid_price) * 10000;
        }
      }

      const mid = (bestBid + bestAsk) / 2;
      results.push({
        symbol: sym,
        base: baseStr,
        best_bid: bestBid,
        best_ask: bestAsk,
        mid_price: mid,
        spread_bps: Math.max(0.5, bestBuySpreadBps),
        best_buy_venue: bestAskVenue,
        best_sell_venue: bestBidVenue,
        fee_bps: 2.0,
        base_price: basePrice,
      });
    }
    return results;
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
    const ticker = PERP_TICKERS.find((t) => t.symbol === symbol);
    const base = ticker ? ticker.basePrice : 100.0;
    const spreadBps = targetBps + 4.5 + Math.random() * 5.0;
    const profit = Math.max(0.5, (spreadBps / 10000) * base * orderQty - 2.1);
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
    const ticker = PERP_TICKERS.find((t) => t.symbol === symbol);
    const base = ticker ? ticker.basePrice : 100.0;
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
      direction: `Buy ${buyVenue} / Sell ${sellVenue}`,
      venue_a: buyVenue,
      venue_b: sellVenue,
      spread_bps: spreadBps,
      realized_profit_usd: netProfit,
      total_cumulative_pnl_usd: this.pnl,
      message: `Executed atomic spatial arb on ${symbol} between ${buyVenue} and ${sellVenue}. Net profit: $${netProfit.toFixed(2)}`,
    };
  }

  executeCrossTickerArbitrage(
    longSymbol: string,
    shortSymbol: string,
    longVenue: string,
    shortVenue: string,
    orderQty: number,
    netSpreadBps?: number
  ): SimStepResultDto {
    this.simCycle += 1;
    const tickerLong = PERP_TICKERS.find((t) => t.symbol === longSymbol);
    const baseLong = tickerLong ? tickerLong.basePrice : 100.0;
    const spreadBps = netSpreadBps ?? (15.5 + Math.random() * 6.0);
    const grossProfit = Math.max(0.5, (spreadBps / 10000) * baseLong * orderQty);
    const fees = 2.1;
    const netProfit = grossProfit - fees;
    this.pnl += netProfit;

    const log: ExecutionLogEntry = {
      id: `exec-${this.simCycle}`,
      timestamp: new Date().toISOString(),
      trade_type: "PairStatArb",
      symbol: `${longSymbol}/${shortSymbol}`,
      venue_a: `${longSymbol} (${longVenue})`,
      venue_b: `${shortSymbol} (${shortVenue})`,
      leg_a_status: "FILLED",
      leg_b_status: "FILLED",
      success: true,
      gross_profit: grossProfit,
      fees,
      net_profit: netProfit,
      latency_us: Math.floor(105 + Math.random() * 30),
      note: `Cross-ticker relative arb: Buy ${longSymbol} (${longVenue}) / Sell ${shortSymbol} (${shortVenue}) (+${netProfit.toFixed(2)} USD)`,
    };
    this.logs.unshift(log);

    return {
      cycle: this.simCycle,
      opportunity_found: true,
      execution_success: true,
      trade_type: "Cross-Ticker Arb",
      direction: `Long ${longSymbol} (${longVenue}) + Short ${shortSymbol} (${shortVenue})`,
      venue_a: `${longSymbol} (${longVenue})`,
      venue_b: `${shortSymbol} (${shortVenue})`,
      spread_bps: spreadBps,
      realized_profit_usd: netProfit,
      total_cumulative_pnl_usd: this.pnl,
      message: `Executed cross-ticker pair arb! Long ${longSymbol} on ${longVenue} / Short ${shortSymbol} on ${shortVenue}. Net profit: $${netProfit.toFixed(2)}`,
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

    const tauri = await getTauriCore();
    if (tauri) {
      return tauri.invoke<SimStepResultDto>("execute_cross_ticker_arbitrage", {
        longSymbol: p.longSymbol,
        shortSymbol: p.shortSymbol,
        longVenue: p.longVenue,
        shortVenue: p.shortVenue,
        orderQty: p.orderQty,
        netSpreadBps: p.netSpreadBps,
      });
    }
    return mockEngine.executeCrossTickerArbitrage(
      p.longSymbol,
      p.shortSymbol,
      p.longVenue,
      p.shortVenue,
      p.orderQty,
      p.netSpreadBps
    );
  },

  async fetchMultiMarketQuotes(symbols: string[]): Promise<TickerMarketQuoteDto[]> {
    const tauri = await getTauriCore();
    if (tauri) {
      return tauri.invoke<TickerMarketQuoteDto[]>("fetch_multi_market_quotes", { symbols });
    }
    return mockEngine.fetchMultiMarketQuotes(symbols);
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

