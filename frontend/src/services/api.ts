import { invoke } from "@tauri-apps/api/core";
import type {
  SystemStatusDto,
  VenueOrderBookDto,
  SpatialOpportunityDto,
  SpotOpportunityDto,
  SpotExecutionParams,
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
  VaultStatus,
  ExchangeCredentials,
  VenueBalanceInfo,
  StrategyMetrics,
  WfoOptimizationResult,
} from "../types";

import { PERP_TICKERS } from "../types/tickers";

// Check if running inside Tauri IPC environment
function isTauriEnvironment(): boolean {
  return typeof window !== "undefined" && ("__TAURI_INTERNALS__" in window || "__TAURI__" in window);
}

// Lighter perp dex market IDs
const LIGHTER_MARKET_MAP: Record<string, number> = {
  BTC: 1,
  ETH: 0,
  SOL: 2,
  DOGE: 3,
  AVAX: 9,
  SUI: 16,
  BNB: 25,
  ARB: 50,
  OP: 55,
  LINK: 8,
  NEAR: 10,
  APT: 11,
  PEPE: 12,
  WIF: 13,
  AAVE: 14,
  RENDER: 15,
  INJ: 17,
  TIA: 18,
  SEI: 19,
};

// Cache for live quotes to prevent rate limits
let cachedAllMids: { timestamp: number; mids: Record<string, number> } | null = null;
let cachedFundingRates: { timestamp: number; rates: Record<string, number> } | null = null;
let cachedBinanceTickers: { timestamp: number; tickers: Map<string, { bid: number; ask: number; bidQty: number; askQty: number }> } | null = null;

async function getLiveHyperliquidMids(): Promise<Record<string, number>> {
  const now = Date.now();
  if (cachedAllMids && now - cachedAllMids.timestamp < 1000) {
    return cachedAllMids.mids;
  }
  try {
    const resp = await fetch("https://api.hyperliquid.xyz/info", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ type: "allMids" }),
    });
    if (resp.ok) {
      const data = await resp.json();
      const mids: Record<string, number> = {};
      for (const [k, v] of Object.entries(data)) {
        const num = parseFloat(v as string);
        if (!isNaN(num)) mids[k.toUpperCase()] = num;
      }
      cachedAllMids = { timestamp: now, mids };
      return mids;
    }
  } catch {
    // network fallback
  }
  return cachedAllMids?.mids || {};
}

async function getLiveHyperliquidFunding(): Promise<Record<string, number>> {
  const now = Date.now();
  if (cachedFundingRates && now - cachedFundingRates.timestamp < 5000) {
    return cachedFundingRates.rates;
  }
  try {
    const resp = await fetch("https://api.hyperliquid.xyz/info", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ type: "metaAndAssetCtxs" }),
    });
    if (resp.ok) {
      const data = await resp.json();
      if (Array.isArray(data) && data.length >= 2) {
        const universe = data[0].universe || [];
        const ctxs = data[1] || [];
        const rates: Record<string, number> = {};
        for (let i = 0; i < universe.length; i++) {
          const coin = universe[i]?.name?.toUpperCase();
          const ctx = ctxs[i];
          if (coin && ctx && ctx.funding) {
            rates[coin] = parseFloat(ctx.funding) * 800;
          }
        }
        cachedFundingRates = { timestamp: now, rates };
        return rates;
      }
    }
  } catch {
    // network fallback
  }
  return cachedFundingRates?.rates || {};
}

async function getLiveBinanceBookTickers(): Promise<Map<string, { bid: number; ask: number; bidQty: number; askQty: number }>> {
  const now = Date.now();
  if (cachedBinanceTickers && now - cachedBinanceTickers.timestamp < 1000) {
    return cachedBinanceTickers.tickers;
  }
  try {
    const resp = await fetch("https://api.binance.com/api/v3/ticker/bookTicker");
    if (resp.ok) {
      const data = await resp.json();
      const map = new Map<string, { bid: number; ask: number; bidQty: number; askQty: number }>();
      if (Array.isArray(data)) {
        for (const item of data) {
          map.set(item.symbol, {
            bid: parseFloat(item.bidPrice) || 0,
            ask: parseFloat(item.askPrice) || 0,
            bidQty: parseFloat(item.bidQty) || 0,
            askQty: parseFloat(item.askQty) || 0,
          });
        }
      }
      cachedBinanceTickers = { timestamp: now, tickers: map };
      return map;
    }
  } catch {
    // network fallback
  }
  return cachedBinanceTickers?.tickers || new Map();
}

// Fallback live market paper engine for browser mode
class BrowserMockEngine {
  private pnl = 0.0;
  private simCycle = 0;
  private isLive = false;
  private cbTripped = false;
  private settings: EngineSettingsDto = {
    hsm_url: "http://127.0.0.1:8787/sign",
    hmac_token: "••••••••••••••••••••••••",
    max_slippage_bps: 15.0,
    min_depth_ratio: 3.0,
    hyperliquid_enabled: true,
    hyperliquid_spot_enabled: true,
    lighter_enabled: true,
    lighter_spot_enabled: true,
    vanta_enabled: true,
    robinhood_enabled: true,
    kraken_enabled: false,
    kraken_spot_enabled: false,
    kraken_futures_enabled: false,
    polymarket_enabled: false,
    kalshi_enabled: false,
    kraken_spot_api_key: "",
    kraken_spot_secret: "",
    kraken_spot_url: "https://api.kraken.com",
    kraken_futures_api_key: "",
    kraken_futures_secret: "",
    kraken_futures_url: "https://futures.kraken.com",
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
    engine_rpc_port: 8787,
    engine_ws_port: 8789,
    bind_address: "127.0.0.1",
    trading_mode: "paper",
    fast_warmup_enabled: true,
    venue_rebalance_threshold_usd: 500.0,
    referral_lighter: "https://app.lighter.xyz/?referral=SOLO&source=none",
    referral_robinhood: "https://robinhoodchain.lighter.xyz/?referral=SOLO&source=none",
    sponsor_link: "https://buymeacoffee.com/dav1d1010",
    version: "v1.2.0",
    git_repo: "https://github.com/arbitrager/arbitrager",
  };
  private logs: ExecutionLogEntry[] = [];

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
        { id: "kraken", name: "Kraken", market_type: "Spot & Futures CLOB", status: "STANDBY", latency_ms: 25, fee_bps: 16.0 },
        { id: "polymarket", name: "Polymarket (CTF CLOB)", market_type: "Prediction Market", status: "ONLINE", latency_ms: 12, fee_bps: 0.0 },
        { id: "kalshi", name: "Kalshi (CFTC)", market_type: "Prediction Market", status: "ONLINE", latency_ms: 15, fee_bps: 1.0 },
      ],
    };
  }

  async fetchOrderbooks(symbol: string): Promise<VenueOrderBookDto[]> {
    const baseCoin = symbol.split("-")[0].toUpperCase();
    const ticker = PERP_TICKERS.find((t) => t.symbol === symbol);
    const benchmark = ticker ? ticker.basePrice : 100.0;

    let hlBid = benchmark * 0.9998;
    let hlAsk = benchmark * 1.0002;
    let hlBids: [number, number][] = [];
    let hlAsks: [number, number][] = [];

    try {
      const resp = await fetch("https://api.hyperliquid.xyz/info", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ type: "l2Book", coin: baseCoin }),
      });
      if (resp.ok) {
        const data = await resp.json();
        if (data && data.levels && data.levels.length >= 2) {
          const rawBids = data.levels[0] || [];
          const rawAsks = data.levels[1] || [];
          if (rawBids.length > 0) {
            hlBid = parseFloat(rawBids[0].px);
            hlBids = rawBids.slice(0, 5).map((l: any) => [parseFloat(l.px), parseFloat(l.sz)]);
          }
          if (rawAsks.length > 0) {
            hlAsk = parseFloat(rawAsks[0].px);
            hlAsks = rawAsks.slice(0, 5).map((l: any) => [parseFloat(l.px), parseFloat(l.sz)]);
          }
        }
      }
    } catch {
      // network fallback
    }

    if (hlBids.length === 0) {
      hlBids = [
        [hlBid, 5.0],
        [hlBid * 0.9998, 8.5],
        [hlBid * 0.9995, 12.0],
        [hlBid * 0.9992, 15.0],
      ];
    }
    if (hlAsks.length === 0) {
      hlAsks = [
        [hlAsk, 5.2],
        [hlAsk * 1.0002, 9.0],
        [hlAsk * 1.0005, 13.5],
        [hlAsk * 1.0008, 16.0],
      ];
    }

    let lgtBid = hlBid;
    let lgtAsk = hlAsk;
    let lgtBids: [number, number][] = [];
    let lgtAsks: [number, number][] = [];
    const marketId = LIGHTER_MARKET_MAP[baseCoin];
    if (marketId !== undefined) {
      try {
        const resp = await fetch(`https://mainnet.zklighter.elliot.ai/api/v1/orderBookOrders?market_id=${marketId}&limit=10`);
        if (resp.ok) {
          const data = await resp.json();
          if (data && data.code === 200) {
            if (data.bids && data.bids.length > 0) {
              lgtBid = parseFloat(data.bids[0].price);
              lgtBids = data.bids.slice(0, 5).map((b: any) => [parseFloat(b.price), parseFloat(b.remaining_base_amount || b.initial_base_amount || "1.0")]);
            }
            if (data.asks && data.asks.length > 0) {
              lgtAsk = parseFloat(data.asks[0].price);
              lgtAsks = data.asks.slice(0, 5).map((a: any) => [parseFloat(a.price), parseFloat(a.remaining_base_amount || a.initial_base_amount || "1.0")]);
            }
          }
        }
      } catch {
        // network fallback
      }
    }
    if (lgtBids.length === 0) {
      lgtBids = hlBids.map(([p, s]) => [p * 1.0002, s * 0.8]);
      lgtBid = lgtBids[0][0];
    }
    if (lgtAsks.length === 0) {
      lgtAsks = hlAsks.map(([p, s]) => [p * 1.0002, s * 0.8]);
      lgtAsk = lgtAsks[0][0];
    }

    let binanceBid = hlBid;
    let binanceAsk = hlAsk;
    try {
      const resp = await fetch(`https://api.binance.com/api/v3/ticker/bookTicker?symbol=${baseCoin}USDT`);
      if (resp.ok) {
        const b = await resp.json();
        if (b.bidPrice && b.askPrice) {
          binanceBid = parseFloat(b.bidPrice);
          binanceAsk = parseFloat(b.askPrice);
        }
      }
    } catch {
      // fallback
    }

    const vntBid = binanceBid > 0 ? binanceBid * 0.9999 : hlBid * 0.9999;
    const vntAsk = binanceAsk > 0 ? binanceAsk * 1.0001 : hlAsk * 1.0001;
    const rhBid = binanceBid > 0 ? binanceBid : hlBid;
    const rhAsk = binanceAsk > 0 ? binanceAsk : hlAsk;

    const fundingRates = await getLiveHyperliquidFunding();
    const hlFunding = fundingRates[baseCoin] ?? 0.01;

    const makeDto = (
      vid: string,
      vname: string,
      bestBid: number,
      bestAsk: number,
      bids: [number, number][],
      asks: [number, number][],
      _feeBps: number,
      fundingPct: number
    ): VenueOrderBookDto => {
      const spread = Math.max(0, bestAsk - bestBid);
      const mid = bestBid > 0 && bestAsk > 0 ? (bestBid + bestAsk) / 2 : benchmark;
      const spreadBps = mid > 0 ? (spread / mid) * 10000 : 0;
      const totalBidDepth = bids.reduce((acc, curr) => acc + curr[1], 0);
      const totalAskDepth = asks.reduce((acc, curr) => acc + curr[1], 0);

      return {
        venue_id: vid,
        venue_name: vname,
        symbol,
        best_bid: bestBid,
        best_ask: bestAsk,
        spread,
        spread_bps: spreadBps,
        mid_price: mid,
        total_bid_depth: totalBidDepth,
        total_ask_depth: totalAskDepth,
        vwap_bid: bestBid * 0.9998,
        vwap_ask: bestAsk * 1.0002,
        funding_rate_8h_pct: fundingPct,
        bids,
        asks,
      };
    };

    return [
      makeDto("hyperliquid", "Hyperliquid (L1)", hlBid, hlAsk, hlBids, hlAsks, 2.0, hlFunding),
      makeDto("lighter", "Lighter (zkDEX)", lgtBid, lgtAsk, lgtBids, lgtAsks, 1.5, hlFunding * 0.75),
      makeDto("vanta", "Vanta.exchange", vntBid, vntAsk, [[vntBid, 12.0], [vntBid * 0.9995, 20.0]], [[vntAsk, 14.0], [vntAsk * 1.0005, 22.0]], 3.0, hlFunding * 0.9),
      makeDto("robinhood", "Robinhood EVM", rhBid, rhAsk, [[rhBid, 8.0], [rhBid * 0.9996, 15.0]], [[rhAsk, 9.0], [rhAsk * 1.0004, 16.0]], 2.5, hlFunding * 0.8),
    ];
  }

  async fetchMultiMarketQuotes(symbols: string[]): Promise<TickerMarketQuoteDto[]> {
    const mids = await getLiveHyperliquidMids();
    const binanceTickers = await getLiveBinanceBookTickers();
    const results: TickerMarketQuoteDto[] = [];

    for (const sym of symbols) {
      const baseCoin = sym.split("-")[0].toUpperCase();
      const ticker = PERP_TICKERS.find((t) => t.symbol === sym);
      const benchmark = ticker ? ticker.basePrice : 100.0;

      const hlMid = mids[baseCoin];
      const bTicker = binanceTickers.get(`${baseCoin}USDT`) || binanceTickers.get(`${baseCoin}USDC`);

      let bestBid: number;
      let bestAsk: number;
      let bestBidVenue = "Hyperliquid";
      let bestAskVenue = "Binance/Orderly";

      if (bTicker && bTicker.bid > 0 && bTicker.ask > 0) {
        if (hlMid && hlMid > 0) {
          bestBid = Math.max(hlMid * 0.9998, bTicker.bid);
          bestAsk = Math.min(hlMid * 1.0002, bTicker.ask);
          bestBidVenue = hlMid * 0.9998 >= bTicker.bid ? "Hyperliquid" : "Binance/Orderly";
          bestAskVenue = hlMid * 1.0002 <= bTicker.ask ? "Hyperliquid" : "Binance/Orderly";
        } else {
          bestBid = bTicker.bid;
          bestAsk = bTicker.ask;
        }
      } else if (hlMid && hlMid > 0) {
        bestBid = hlMid * 0.9998;
        bestAsk = hlMid * 1.0002;
      } else {
        bestBid = benchmark * 0.9998;
        bestAsk = benchmark * 1.0002;
      }

      const mid = (bestBid + bestAsk) / 2;
      const spread = Math.max(0, bestAsk - bestBid);
      const spreadBps = mid > 0 ? (spread / mid) * 10000 : 2.0;

      results.push({
        symbol: sym,
        base: baseCoin,
        best_bid: bestBid,
        best_ask: bestAsk,
        mid_price: mid,
        spread_bps: Math.max(0.5, spreadBps),
        best_buy_venue: bestAskVenue,
        best_sell_venue: bestBidVenue,
        fee_bps: 2.0,
        base_price: benchmark,
      });
    }

    return results;
  }

  async scanSpatialOpportunities(symbol: string, minBps: number, orderQty: number): Promise<SpatialOpportunityDto[]> {
    const books = await this.fetchOrderbooks(symbol);
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

  async scanSpotOpportunities(symbolFilter?: string): Promise<SpotOpportunityDto[]> {
    const spotSymbols = ["BTC", "ETH", "SOL", "BNB", "SUI", "AVAX", "LINK", "NEAR"];
    const binanceTickers = await getLiveBinanceBookTickers();
    const hlMids = await getLiveHyperliquidMids();

    const venues = [
      { name: "Hyperliquid Spot", feeBps: 1.5, transferCost: 0.0 },
      { name: "Lighter Spot", feeBps: 0.0, transferCost: 0.0 },
      { name: "Binance Spot", feeBps: 7.5, transferCost: 0.8 },
      { name: "Coinbase Spot", feeBps: 15.0, transferCost: 1.5 },
      { name: "OKX Spot", feeBps: 8.0, transferCost: 0.8 },
      { name: "Kraken Spot", feeBps: 16.0, transferCost: 1.2 },
    ];

    const opps: SpotOpportunityDto[] = [];
    for (const coin of spotSymbols) {
      const pairSymbol = `${coin}/USDC`;
      if (symbolFilter && symbolFilter.trim().length > 0 && !pairSymbol.toLowerCase().includes(symbolFilter.toLowerCase())) {
        continue;
      }

      const ticker = PERP_TICKERS.find((t) => t.base === coin);
      const benchmark = ticker ? ticker.basePrice : 100.0;
      const bTicker = binanceTickers.get(`${coin}USDT`) || binanceTickers.get(`${coin}USDC`);
      const hlMid = hlMids[coin];

      const refPrice = bTicker && bTicker.bid > 0 ? (bTicker.bid + bTicker.ask) / 2 : (hlMid || benchmark);

      for (let i = 0; i < venues.length; i++) {
        for (let j = 0; j < venues.length; j++) {
          if (i === j) continue;
          const buy = venues[i];
          const sell = venues[j];

          const buyPrice = buy.name.includes("Binance") && bTicker
            ? bTicker.ask
            : buy.name.includes("Hyperliquid") && hlMid
            ? hlMid * 0.9998
            : refPrice * (1.0 - (buy.feeBps * 0.0001));

          const sellPrice = sell.name.includes("Binance") && bTicker
            ? bTicker.bid
            : sell.name.includes("Hyperliquid") && hlMid
            ? hlMid * 1.0002
            : refPrice * (1.0 + (sell.feeBps * 0.0001));

          const grossSpread = sellPrice - buyPrice;
          const grossBps = (grossSpread / buyPrice) * 10000;
          const transferCost = buy.transferCost + sell.transferCost;
          const tradeSize = 1000;
          const feeUsd = (buyPrice * tradeSize * buy.feeBps / 10000) + (sellPrice * tradeSize * sell.feeBps / 10000);
          const grossProfit = (sellPrice - buyPrice) * (tradeSize / buyPrice);
          const netProfitUsd = grossProfit - feeUsd - transferCost;
          const netBps = (netProfitUsd / tradeSize) * 10000;

          opps.push({
            id: `spot-${coin}-USDC-${buy.name.replace(/\s+/g, "")}-${sell.name.replace(/\s+/g, "")}`,
            symbol: pairSymbol,
            buy_venue: buy.name,
            sell_venue: sell.name,
            buy_price: Math.round(buyPrice * 100) / 100,
            sell_price: Math.round(sellPrice * 100) / 100,
            gross_spread_bps: Math.round(grossBps * 10) / 10,
            buy_fee_bps: buy.feeBps,
            sell_fee_bps: sell.feeBps,
            transfer_cost_usd: transferCost,
            net_spread_bps: Math.round(netBps * 10) / 10,
            net_profit_usd: Math.round(netProfitUsd * 100) / 100,
            available_depth_usd: 150000,
            estimated_latency_ms: 12,
            is_executable: netBps > 2.0,
          });
        }
      }
    }

    return opps.sort((a, b) => b.net_spread_bps - a.net_spread_bps);
  }

  executeSpotArbitrage(params: SpotExecutionParams): SimStepResultDto {
    this.simCycle += 1;
    const profit = Math.max(params.trade_size_usd * 0.00085, 4.5);
    this.pnl += profit;

    const logEntry: ExecutionLogEntry = {
      id: `exec-spot-${Date.now()}`,
      timestamp: new Date().toISOString(),
      trade_type: "Spot Cross-Exchange",
      symbol: params.symbol,
      venue_a: params.buy_venue,
      venue_b: params.sell_venue,
      leg_a_status: "FILLED",
      leg_b_status: "FILLED",
      success: true,
      gross_profit: Math.round((profit + 2.0) * 100) / 100,
      fees: 2.0,
      net_profit: Math.round(profit * 100) / 100,
      latency_us: 14200,
      note: `Spot paper flash execution: Bought ${params.symbol} on ${params.buy_venue}, sold on ${params.sell_venue}`,
      direction: "Spot Cash Arb",
      trigger_spread_bps: params.net_spread_bps ?? 8.5,
      baseline_mean_spread_bps: 4.0,
      position_step: 1,
      max_units: 1,
      nominal_hurdle_bps: 5.0,
    };
    this.logs.unshift(logEntry);
    if (this.logs.length > 100) this.logs.pop();

    return {
      cycle: this.simCycle,
      opportunity_found: true,
      execution_success: true,
      trade_type: "Spot Cross-Exchange",
      direction: "Spot Cash Arb",
      venue_a: params.buy_venue,
      venue_b: params.sell_venue,
      spread_bps: params.net_spread_bps ?? 8.5,
      realized_profit_usd: Math.round(profit * 100) / 100,
      total_cumulative_pnl_usd: Math.round(this.pnl * 100) / 100,
      message: `Spot Bot execution successful: +$${profit.toFixed(2)} USD`,
    };
  }

  async scanFundingOpportunities(symbol: string, tradeSizeUsd: number): Promise<FundingOpportunityDto[]> {
    const baseCoin = symbol.split("-")[0].toUpperCase();
    const fundingRates = await getLiveHyperliquidFunding();
    const hlRate8h = fundingRates[baseCoin] ?? 0.015;

    const venues = [
      { long: "Lighter", short: "Hyperliquid", rateMultiplier: 0.85 },
      { long: "Robinhood EVM", short: "Hyperliquid", rateMultiplier: 0.70 },
      { long: "Vanta.exchange", short: "Hyperliquid", rateMultiplier: 0.55 },
    ];

    return venues.map((v) => {
      const netRate8h = Math.abs(hlRate8h * (1.0 - v.rateMultiplier));
      const netApr = netRate8h * 3 * 365;
      const netApy = ((1 + netRate8h / 100) ** (3 * 365) - 1) * 100;
      const dailyIncome = tradeSizeUsd * (netRate8h / 100) * 3;
      const breakEvenHours = netRate8h > 0 ? (3.5 / (netRate8h * 10000)) * 8 : 24.0;

      return {
        id: `${symbol}-${v.long.replace(/\s+/g, "")}-${v.short}`,
        symbol,
        long_venue: v.long,
        short_venue: v.short,
        net_rate_8h_pct: Math.round(netRate8h * 10000) / 10000,
        net_apr_pct: Math.round(netApr * 100) / 100,
        net_apy_pct: Math.round(netApy * 100) / 100,
        round_trip_fee_bps: 3.5,
        break_even_hours: Math.round(breakEvenHours * 10) / 10,
        estimated_daily_income_usd: Math.round(dailyIncome * 100) / 100,
        trade_size_usd: tradeSizeUsd,
      };
    });
  }

  async scanPredictionOpportunities(market: string): Promise<PredictionOpportunityDto[]> {
    const opps: PredictionOpportunityDto[] = [];
    try {
      const resp = await fetch("https://gamma-api.polymarket.com/events?closed=false&limit=15");
      if (resp.ok) {
        const events = await resp.json();
        if (Array.isArray(events)) {
          for (const ev of events) {
            const m = ev.markets?.[0];
            if (!m) continue;
            let prices: number[] = [];
            if (typeof m.outcomePrices === "string") {
              try { prices = JSON.parse(m.outcomePrices).map(Number); } catch { /* ignore */ }
            } else if (Array.isArray(m.outcomePrices)) {
              prices = m.outcomePrices.map(Number);
            }
            if (prices.length >= 2) {
              const yesCents = Math.round((prices[0] || 0.5) * 1000) / 10;
              const noCents = Math.round((prices[1] || 0.5) * 1000) / 10;
              const totalCost = yesCents + noCents;
              const netProfit = Math.max(0, 100.0 - totalCost);
              const roi = totalCost > 0 ? (netProfit / totalCost) * 100 : 0;
              const title = ev.title || m.question || "Prediction Market";

              if (!market || title.toLowerCase().includes(market.toLowerCase())) {
                opps.push({
                  id: `poly-${ev.id || m.id}`,
                  market_title: title,
                  venue_a: "Polymarket",
                  venue_b: "Kalshi",
                  strategy: "Synthetic Binary Parity",
                  buy_yes_venue: "Polymarket",
                  buy_no_venue: "Kalshi",
                  yes_price_cents: yesCents,
                  no_price_cents: noCents,
                  total_cost_cents: totalCost,
                  guaranteed_payout_cents: 100.0,
                  net_profit_cents: Math.round(netProfit * 10) / 10,
                  roi_pct: Math.round(roi * 100) / 100,
                  has_arbitrage: netProfit > 0,
                });
              }
            }
          }
        }
      }
    } catch {
      // network fallback
    }

    if (opps.length === 0) {
      const fallbackList: PredictionOpportunityDto[] = [
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
      ];
      return market ? fallbackList.filter((p) => p.market_title.toLowerCase().includes(market.toLowerCase())) : fallbackList;
    }

    return opps;
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

  executeSpatialArbitrage(
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
  ): SimStepResultDto {
    this.simCycle += 1;
    const ticker = PERP_TICKERS.find((t) => t.symbol === symbol);
    const base = ticker ? ticker.basePrice : 100.0;
    const spreadBps = triggerSpreadBps ?? 14.8;
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
      gross_profit: Math.round(grossProfit * 100) / 100,
      fees,
      net_profit: Math.round(netProfit * 100) / 100,
      latency_us: 115,
      note: `Paper spatial fill: Buy ${buyVenue} / Sell ${sellVenue} (+${netProfit.toFixed(2)} USD)`,
      direction: direction ?? "Divergence Entry",
      trigger_spread_bps: spreadBps,
      baseline_mean_spread_bps: baselineMeanSpreadBps,
      position_step: positionStep ?? 1,
      max_units: maxUnits ?? 3,
      nominal_hurdle_bps: nominalHurdleBps,
    };
    this.logs.unshift(log);
    if (this.logs.length > 100) this.logs.pop();

    return {
      cycle: this.simCycle,
      opportunity_found: true,
      execution_success: true,
      trade_type: "Spatial Arb",
      direction: direction ?? `Buy ${buyVenue} / Sell ${sellVenue}`,
      venue_a: buyVenue,
      venue_b: sellVenue,
      spread_bps: spreadBps,
      realized_profit_usd: Math.round(netProfit * 100) / 100,
      total_cumulative_pnl_usd: Math.round(this.pnl * 100) / 100,
      message: `Paper executed atomic spatial arb on ${symbol} between ${buyVenue} and ${sellVenue}. Net profit: $${netProfit.toFixed(2)}`,
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
    const spreadBps = netSpreadBps ?? 15.5;
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
      gross_profit: Math.round(grossProfit * 100) / 100,
      fees,
      net_profit: Math.round(netProfit * 100) / 100,
      latency_us: 108,
      note: `Paper cross-ticker relative arb: Buy ${longSymbol} (${longVenue}) / Sell ${shortSymbol} (${shortVenue}) (+${netProfit.toFixed(2)} USD)`,
    };
    this.logs.unshift(log);
    if (this.logs.length > 100) this.logs.pop();

    return {
      cycle: this.simCycle,
      opportunity_found: true,
      execution_success: true,
      trade_type: "Cross-Ticker Arb",
      direction: `Long ${longSymbol} (${longVenue}) + Short ${shortSymbol} (${shortVenue})`,
      venue_a: `${longSymbol} (${longVenue})`,
      venue_b: `${shortSymbol} (${shortVenue})`,
      spread_bps: spreadBps,
      realized_profit_usd: Math.round(netProfit * 100) / 100,
      total_cumulative_pnl_usd: Math.round(this.pnl * 100) / 100,
      message: `Paper executed cross-ticker pair arb! Long ${longSymbol} on ${longVenue} / Short ${shortSymbol} on ${shortVenue}. Net profit: $${netProfit.toFixed(2)}`,
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
      gross_profit: Math.round((netProfit + 1.8) * 100) / 100,
      fees: 1.8,
      net_profit: Math.round(netProfit * 100) / 100,
      latency_us: 88,
      note: `Paper carry opened: Long ${longVenue} / Short ${shortVenue} for ${symbol}`,
    };
    this.logs.unshift(log);
    if (this.logs.length > 100) this.logs.pop();

    return {
      cycle: this.simCycle,
      opportunity_found: true,
      execution_success: true,
      trade_type: "Funding Carry",
      direction: `Long ${longVenue} + Short ${shortVenue}`,
      venue_a: longVenue,
      venue_b: shortVenue,
      spread_bps: 18.5,
      realized_profit_usd: Math.round(netProfit * 100) / 100,
      total_cumulative_pnl_usd: Math.round(this.pnl * 100) / 100,
      message: `Paper opened delta-neutral funding carry position of $${tradeSizeUsd.toFixed(2)} on ${symbol}`,
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
      gross_profit: Math.round(netProfit * 100) / 100,
      fees: 0.0,
      net_profit: Math.round(netProfit * 100) / 100,
      latency_us: 125,
      note: `Paper synthetic parity filled: Buy YES (${buyYesVenue}) + Buy NO (${buyNoVenue})`,
    };
    this.logs.unshift(log);
    if (this.logs.length > 100) this.logs.pop();

    return {
      cycle: this.simCycle,
      opportunity_found: true,
      execution_success: true,
      trade_type: "Prediction Parity",
      direction: "ArrowDebreu",
      venue_a: buyYesVenue,
      venue_b: buyNoVenue,
      spread_bps: 450.0,
      realized_profit_usd: Math.round(netProfit * 100) / 100,
      total_cumulative_pnl_usd: Math.round(this.pnl * 100) / 100,
      message: `Paper executed atomic parity trade on ${marketTitle}. Locked in payout with profit $${netProfit.toFixed(2)}`,
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

  private strategyConfigs = new Map<string, TickerStrategyConfigDto>();

  getTickerStrategyConfig(symbol: string): TickerStrategyConfigDto {
    const existing = this.strategyConfigs.get(symbol);
    if (existing) return existing;
    return {
      symbol,
      targetProfitBps: 5.0,
      windowPercentile: 50,
      maxAccumulatedUnits: 3,
      currentAccumulatedUnits: 0,
      executionMode: "SINGLE_WAY",
      unitSize: 0.005,
      userTargetLeverage: 10,
    };
  }

  saveTickerStrategyConfig(config: TickerStrategyConfigDto): void {
    this.strategyConfigs.set(config.symbol, config);
  }

  getAllTickerStrategyConfigs(): TickerStrategyConfigDto[] {
    return Array.from(this.strategyConfigs.values());
  }

  private vaultStatus: VaultStatus = {
    is_initialized: true,
    is_unlocked: true,
    unlocked_until: new Date(Date.now() + 15 * 60 * 1000).toISOString(),
  };
  private exchangeCredentials: ExchangeCredentials[] = [
    { venue: "hyperliquid", api_key: "0x1a8f9c...4b", api_secret: "••••••••••••", passphrase: null, account_index: 0, subaccount_index: null, enabled: true },
    { venue: "lighter", api_key: "0x3e71d8...c2", api_secret: "••••••••••••", passphrase: null, account_index: 0, subaccount_index: 1, enabled: true },
    { venue: "vanta", api_key: "ord_key_99812", api_secret: "••••••••••••", passphrase: null, account_index: null, subaccount_index: null, enabled: true },
    { venue: "robinhood", api_key: "0x89e5db...ba", api_secret: "••••••••••••", passphrase: null, account_index: null, subaccount_index: null, enabled: true },
  ];

  getVaultStatus(): VaultStatus {
    return this.vaultStatus;
  }

  initializeVault(_password: string): VaultStatus {
    this.vaultStatus = {
      is_initialized: true,
      is_unlocked: true,
      unlocked_until: new Date(Date.now() + 15 * 60 * 1000).toISOString(),
    };
    return this.vaultStatus;
  }

  unlockVault(_password: string): VaultStatus {
    this.vaultStatus.is_unlocked = true;
    this.vaultStatus.unlocked_until = new Date(Date.now() + 15 * 60 * 1000).toISOString();
    return this.vaultStatus;
  }

  lockVault(): VaultStatus {
    this.vaultStatus.is_unlocked = false;
    this.vaultStatus.unlocked_until = null;
    return this.vaultStatus;
  }

  getExchangeCredentials(): ExchangeCredentials[] {
    if (!this.vaultStatus.is_unlocked) throw new Error("Vault is locked. Enter password to view credentials.");
    return this.exchangeCredentials;
  }

  saveExchangeCredentials(_password: string, credentials: ExchangeCredentials[]): void {
    this.exchangeCredentials = credentials;
  }

  getVenueBalances(): VenueBalanceInfo[] {
    return [
      { venue: "hyperliquid", balance_usd: 84250.0, min_required_usd: 500.0, needs_rebalance: false, status: "HEALTHY" },
      { venue: "lighter", balance_usd: 62100.0, min_required_usd: 500.0, needs_rebalance: false, status: "HEALTHY" },
      { venue: "vanta", balance_usd: 48900.0, min_required_usd: 500.0, needs_rebalance: false, status: "HEALTHY" },
      { venue: "robinhood", balance_usd: 35400.0, min_required_usd: 500.0, needs_rebalance: false, status: "HEALTHY" },
    ];
  }

  getStrategyPerformance(strategyId?: string): StrategyMetrics[] {
    const list: StrategyMetrics[] = [
      {
        strategy_id: "spatial_spread",
        total_cycles_initiated: 24,
        total_cycles_completed: 22,
        winning_cycles: 20,
        losing_cycles: 2,
        win_rate_pct: 90.9,
        total_pnl_usd: 1420.5,
        avg_pnl_usd: 64.57,
        avg_pnl_bps: 12.4,
        max_drawdown_usd: 85.0,
        max_drawdown_pct: 0.85,
        sharpe_ratio: 3.42,
        annualized_apr_pct: 172.8,
        annualized_apy_pct: 320.4,
      },
      {
        strategy_id: "stat_arb",
        total_cycles_initiated: 16,
        total_cycles_completed: 15,
        winning_cycles: 13,
        losing_cycles: 2,
        win_rate_pct: 86.7,
        total_pnl_usd: 890.2,
        avg_pnl_usd: 59.35,
        avg_pnl_bps: 9.8,
        max_drawdown_usd: 120.0,
        max_drawdown_pct: 1.2,
        sharpe_ratio: 2.85,
        annualized_apr_pct: 108.3,
        annualized_apy_pct: 195.1,
      },
      {
        strategy_id: "funding_rate",
        total_cycles_initiated: 12,
        total_cycles_completed: 12,
        winning_cycles: 11,
        losing_cycles: 1,
        win_rate_pct: 91.7,
        total_pnl_usd: 640.8,
        avg_pnl_usd: 53.4,
        avg_pnl_bps: 14.1,
        max_drawdown_usd: 45.0,
        max_drawdown_pct: 0.45,
        sharpe_ratio: 3.8,
        annualized_apr_pct: 78.0,
        annualized_apy_pct: 118.2,
      },
      {
        strategy_id: "prediction_parity",
        total_cycles_initiated: 8,
        total_cycles_completed: 8,
        winning_cycles: 8,
        losing_cycles: 0,
        win_rate_pct: 100.0,
        total_pnl_usd: 310.0,
        avg_pnl_usd: 38.75,
        avg_pnl_bps: 22.5,
        max_drawdown_usd: 0.0,
        max_drawdown_pct: 0.0,
        sharpe_ratio: 6.5,
        annualized_apr_pct: 37.7,
        annualized_apy_pct: 45.8,
      },
    ];
    if (strategyId) return list.filter((s) => s.strategy_id === strategyId);
    return list;
  }

  runWfoCalibration(_symbol: string): WfoOptimizationResult {
    return {
      optimal_estimator: "VWMA",
      optimal_window_size: 30,
      in_sample_score: 3.15,
      out_of_sample_score: 2.84,
      recommendation_notes: "Depth-weighted VWMA(30) demonstrated highest out-of-sample Sharpe (2.84) with lowest tracking error across orderbook quote updates.",
      candidate_evaluations: [
        { estimator: "VWMA", window_size: 30, in_sample_sharpe: 3.15, out_of_sample_sharpe: 2.84, tracking_error_bps: 1.1, simulated_net_profit_bps: 48.2, win_rate_pct: 88.5, total_trades: 26 },
        { estimator: "EMA", window_size: 20, in_sample_sharpe: 2.92, out_of_sample_sharpe: 2.45, tracking_error_bps: 1.4, simulated_net_profit_bps: 41.0, win_rate_pct: 82.0, total_trades: 28 },
        { estimator: "SMA", window_size: 50, in_sample_sharpe: 2.65, out_of_sample_sharpe: 2.15, tracking_error_bps: 1.8, simulated_net_profit_bps: 36.5, win_rate_pct: 78.0, total_trades: 23 },
        { estimator: "WMA", window_size: 30, in_sample_sharpe: 2.80, out_of_sample_sharpe: 2.30, tracking_error_bps: 1.5, simulated_net_profit_bps: 38.2, win_rate_pct: 80.0, total_trades: 25 },
        { estimator: "SMMA", window_size: 50, in_sample_sharpe: 2.40, out_of_sample_sharpe: 1.95, tracking_error_bps: 2.1, simulated_net_profit_bps: 31.0, win_rate_pct: 74.0, total_trades: 20 },
      ],
    };
  }
}

const mockEngine = new BrowserMockEngine();

// Tauri core IPC using statically imported invoke
async function getTauriCore() {
  if (isTauriEnvironment()) {
    return { invoke };
  }
  return null;
}

function normalizeTickerStrategyConfig(c: any): TickerStrategyConfigDto {
  if (!c) {
    return {
      symbol: "BTC-USDC",
      targetProfitBps: 5.0,
      windowPercentile: 50,
      maxAccumulatedUnits: 3,
      currentAccumulatedUnits: 0,
      executionMode: "SINGLE_WAY",
      unitSize: 0.005,
      userTargetLeverage: 10,
    };
  }
  return {
    symbol: c.symbol || "BTC-USDC",
    targetProfitBps: Number(c.targetProfitBps ?? c.target_profit_bps ?? 5.0),
    windowPercentile: Number(c.windowPercentile ?? c.window_percentile ?? 50),
    maxAccumulatedUnits: Number(c.maxAccumulatedUnits ?? c.max_accumulated_units ?? 3),
    currentAccumulatedUnits: Number(c.currentAccumulatedUnits ?? c.current_accumulated_units ?? 0),
    executionMode: (c.executionMode ?? c.execution_mode ?? "SINGLE_WAY"),
    unitSize: Number(c.unitSize ?? c.unit_size ?? 0.005),
    userTargetLeverage: Number(c.userTargetLeverage ?? c.user_target_leverage ?? 10),
  };
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
    if (isTauriEnvironment()) {
      try {
        return await invoke<SpatialOpportunityDto[]>("scan_spatial_opportunities", { symbol, minBps, orderQty });
      } catch (e) {
        console.warn("Tauri invoke scan_spatial_opportunities error:", e);
      }
    }
    return mockEngine.scanSpatialOpportunities(symbol, minBps, orderQty);
  },

  async scanSpotOpportunities(symbol?: string): Promise<SpotOpportunityDto[]> {
    if (isTauriEnvironment()) {
      try {
        return await invoke<SpotOpportunityDto[]>("scan_spot_opportunities", { symbol: symbol || "" });
      } catch (e) {
        console.warn("Tauri invoke scan_spot_opportunities error:", e);
      }
    }
    return mockEngine.scanSpotOpportunities(symbol);
  },

  async executeSpotArbitrage(params: SpotExecutionParams): Promise<SimStepResultDto> {
    if (isTauriEnvironment()) {
      try {
        return await invoke<SimStepResultDto>("execute_spot_arbitrage", { params });
      } catch (e) {
        console.warn("Tauri invoke execute_spot_arbitrage error:", e);
      }
    }
    return mockEngine.executeSpotArbitrage(params);
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
    const tauri = await getTauriCore();
    if (tauri)
      return tauri.invoke<SimStepResultDto>("execute_spatial_arbitrage", {
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
    return mockEngine.executeSpatialArbitrage(
      symbol,
      buyVenue,
      sellVenue,
      orderQty,
      direction,
      triggerSpreadBps,
      baselineMeanSpreadBps,
      positionStep,
      maxUnits,
      nominalHurdleBps
    );
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

  async getTickerStrategyConfig(symbol: string): Promise<TickerStrategyConfigDto> {
    const tauri = await getTauriCore();
    if (tauri) {
      const res = await tauri.invoke<TickerStrategyConfigDto>("get_ticker_strategy_config", { symbol });
      return normalizeTickerStrategyConfig(res);
    }
    return normalizeTickerStrategyConfig(mockEngine.getTickerStrategyConfig(symbol));
  },

  async saveTickerStrategyConfig(config: TickerStrategyConfigDto): Promise<void> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<void>("save_ticker_strategy_config", { config });
    return mockEngine.saveTickerStrategyConfig(config);
  },

  async getAllTickerStrategyConfigs(): Promise<TickerStrategyConfigDto[]> {
    const tauri = await getTauriCore();
    if (tauri) {
      const res = await tauri.invoke<TickerStrategyConfigDto[]>("get_all_ticker_strategy_configs");
      return (res || []).map(normalizeTickerStrategyConfig);
    }
    return mockEngine.getAllTickerStrategyConfigs().map(normalizeTickerStrategyConfig);
  },

  async getVaultStatus(): Promise<VaultStatus> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<VaultStatus>("get_vault_status");
    return mockEngine.getVaultStatus();
  },

  async initializeVault(password: string): Promise<VaultStatus> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<VaultStatus>("initialize_vault", { password });
    return mockEngine.initializeVault(password);
  },

  async unlockVault(password: string): Promise<VaultStatus> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<VaultStatus>("unlock_vault", { password });
    return mockEngine.unlockVault(password);
  },

  async lockVault(): Promise<VaultStatus> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<VaultStatus>("lock_vault");
    return mockEngine.lockVault();
  },

  async getExchangeCredentials(): Promise<ExchangeCredentials[]> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<ExchangeCredentials[]>("get_exchange_credentials");
    return mockEngine.getExchangeCredentials();
  },

  async saveExchangeCredentials(password: string, credentials: ExchangeCredentials[]): Promise<void> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<void>("save_exchange_credentials", { password, credentials });
    return mockEngine.saveExchangeCredentials(password, credentials);
  },

  async getVenueBalances(): Promise<VenueBalanceInfo[]> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<VenueBalanceInfo[]>("get_venue_balances");
    return mockEngine.getVenueBalances();
  },

  async getStrategyPerformance(strategyId?: string): Promise<StrategyMetrics[]> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<StrategyMetrics[]>("get_strategy_performance", { strategyId });
    return mockEngine.getStrategyPerformance(strategyId);
  },

  async runWfoCalibration(symbol: string): Promise<WfoOptimizationResult> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<WfoOptimizationResult>("run_wfo_calibration", { symbol });
    return mockEngine.runWfoCalibration(symbol);
  },
};


