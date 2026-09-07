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
      note: "Executed atomically at spread 18.2 BPS (Dislocation +6.8 BPS over baseline)",
      direction: "Divergence Entry",
      trigger_spread_bps: 18.2,
      baseline_mean_spread_bps: 11.4,
      position_step: 1,
      max_units: 3,
      nominal_hurdle_bps: 14.8,
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
      direction: "Carry Open",
      trigger_spread_bps: 15.0,
      baseline_mean_spread_bps: 9.5,
      position_step: 1,
      max_units: 3,
      nominal_hurdle_bps: 12.0,
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
      note: "Step 2 scale-in on widening divergence",
      direction: "Divergence Entry",
      trigger_spread_bps: 22.4,
      baseline_mean_spread_bps: 13.1,
      position_step: 2,
      max_units: 3,
      nominal_hurdle_bps: 15.2,
    },
    {
      id: "exec-3b",
      timestamp: new Date(Date.now() - 210000).toISOString(),
      trade_type: "Spatial",
      symbol: "BTC-USDC",
      venue_a: "Hyperliquid",
      venue_b: "Lighter",
      leg_a_status: "FILLED",
      leg_b_status: "FILLED",
      success: true,
      gross_profit: 26.0,
      fees: 1.5,
      net_profit: 24.5,
      latency_us: 108,
      note: "Partial unwind as spread converged toward mean",
      direction: "Convergence Unwind",
      trigger_spread_bps: 4.2,
      baseline_mean_spread_bps: 12.0,
      position_step: 2,
      max_units: 3,
      nominal_hurdle_bps: 6.0,
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
      direction: "Divergence Entry",
      trigger_spread_bps: 16.4,
      baseline_mean_spread_bps: 10.2,
      position_step: 1,
      max_units: 4,
      nominal_hurdle_bps: 13.5,
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
      direction: "Carry Open",
      trigger_spread_bps: 14.2,
      baseline_mean_spread_bps: 9.0,
      position_step: 1,
      max_units: 3,
      nominal_hurdle_bps: 12.0,
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
      direction: "Divergence Entry",
      trigger_spread_bps: 24.5,
      baseline_mean_spread_bps: 14.0,
      position_step: 1,
      max_units: 3,
      nominal_hurdle_bps: 17.2,
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
      note: "AVAX perp basis captured on convergence",
      direction: "Convergence Unwind",
      trigger_spread_bps: 3.2,
      baseline_mean_spread_bps: 8.5,
      position_step: 2,
      max_units: 3,
      nominal_hurdle_bps: 5.0,
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
      direction: "Divergence Entry",
      trigger_spread_bps: 14.5,
      baseline_mean_spread_bps: 10.0,
      position_step: 2,
      max_units: 4,
      nominal_hurdle_bps: 13.5,
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
      direction: "Divergence Entry",
      trigger_spread_bps: 20.1,
      baseline_mean_spread_bps: 13.5,
      position_step: 2,
      max_units: 3,
      nominal_hurdle_bps: 16.5,
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
      direction: "Divergence Entry",
      trigger_spread_bps: 19.0,
      baseline_mean_spread_bps: 12.0,
      position_step: 1,
      max_units: 3,
      nominal_hurdle_bps: 15.0,
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
      direction: "Divergence Entry",
      trigger_spread_bps: 17.5,
      baseline_mean_spread_bps: 11.5,
      position_step: 1,
      max_units: 3,
      nominal_hurdle_bps: 14.5,
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
      direction: "Divergence Entry",
      trigger_spread_bps: 21.5,
      baseline_mean_spread_bps: 12.8,
      position_step: 1,
      max_units: 3,
      nominal_hurdle_bps: 16.0,
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

  scanSpotOpportunities(symbolFilter?: string): SpotOpportunityDto[] {
    const assets = [
      { symbol: "BTC/USDC", basePrice: 96420.0, depth: 350000, latency: 12 },
      { symbol: "ETH/USDC", basePrice: 2845.5, depth: 220000, latency: 15 },
      { symbol: "SOL/USDC", basePrice: 198.8, depth: 140000, latency: 10 },
      { symbol: "BNB/USDC", basePrice: 654.2, depth: 95000, latency: 16 },
      { symbol: "SUI/USDC", basePrice: 3.45, depth: 65000, latency: 18 },
      { symbol: "AVAX/USDC", basePrice: 34.9, depth: 70000, latency: 20 },
      { symbol: "LINK/USDC", basePrice: 18.95, depth: 55000, latency: 14 },
      { symbol: "NEAR/USDC", basePrice: 5.68, depth: 48000, latency: 17 },
    ];

    const venues = [
      { name: "Hyperliquid Spot", feeBps: 1.5, transferCost: 0.0 },
      { name: "Lighter Spot", feeBps: 0.0, transferCost: 0.0 },
      { name: "Binance Spot", feeBps: 7.5, transferCost: 0.8 },
      { name: "Coinbase Spot", feeBps: 15.0, transferCost: 1.5 },
      { name: "OKX Spot", feeBps: 8.0, transferCost: 0.8 },
      { name: "Kraken Spot", feeBps: 16.0, transferCost: 1.2 },
    ];

    const opps: SpotOpportunityDto[] = [];
    for (const a of assets) {
      if (symbolFilter && symbolFilter.trim().length > 0 && !a.symbol.toLowerCase().includes(symbolFilter.toLowerCase())) {
        continue;
      }
      for (let i = 0; i < venues.length; i++) {
        for (let j = 0; j < venues.length; j++) {
          if (i === j) continue;
          const buy = venues[i];
          const sell = venues[j];
          const skew = Math.sin(i * 3.7 + j * 7.1 + this.simCycle * 0.4) * 12.0;
          const grossBps = Math.max(buy.feeBps + sell.feeBps + 2.0 + skew, 0.5);
          const buyPrice = a.basePrice * (1.0 - skew * 0.00005);
          const sellPrice = buyPrice * (1.0 + grossBps / 10000);
          const transferCost = buy.transferCost + sell.transferCost;
          const tradeSize = 1000;
          const feeUsd = (buyPrice * tradeSize * buy.feeBps / 10000) + (sellPrice * tradeSize * sell.feeBps / 10000);
          const grossProfit = (sellPrice - buyPrice) * (tradeSize / buyPrice);
          const netProfitUsd = grossProfit - feeUsd - transferCost;
          const netBps = (netProfitUsd / tradeSize) * 10000;

          opps.push({
            id: `spot-${a.symbol.replace("/", "-")}-${buy.name.replace(/\s+/g, "")}-${sell.name.replace(/\s+/g, "")}`,
            symbol: a.symbol,
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
            available_depth_usd: a.depth,
            estimated_latency_ms: a.latency,
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
      note: `Spot Bot flash execution: Bought ${params.symbol} on ${params.buy_venue}, sold on ${params.sell_venue}`,
      direction: "Spot Cash Arb",
      trigger_spread_bps: 8.5,
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
      spread_bps: 8.5,
      realized_profit_usd: profit,
      total_cumulative_pnl_usd: this.pnl,
      message: `Spot Bot execution successful: +$${profit.toFixed(2)} USD (+8.5 bps)`,
    };
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
    const spreadBps = triggerSpreadBps ?? (14.8 + Math.random() * 5.2);
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
      direction: direction ?? "Divergence Entry",
      trigger_spread_bps: spreadBps,
      baseline_mean_spread_bps: baselineMeanSpreadBps,
      position_step: positionStep ?? 1,
      max_units: maxUnits ?? 3,
      nominal_hurdle_bps: nominalHurdleBps,
    };
    this.logs.unshift(log);

    return {
      cycle: this.simCycle,
      opportunity_found: true,
      execution_success: true,
      trade_type: "Spatial Arb",
      direction: direction ?? `Buy ${buyVenue} / Sell ${sellVenue}`,
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

  async runSimulationStep(symbol: string, targetBps: number, orderQty: number): Promise<SimStepResultDto> {
    const tauri = await getTauriCore();
    if (tauri) return tauri.invoke<SimStepResultDto>("run_simulation_step", { symbol, targetBps, orderQty });
    return mockEngine.runSimStep(symbol, targetBps, orderQty);
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


