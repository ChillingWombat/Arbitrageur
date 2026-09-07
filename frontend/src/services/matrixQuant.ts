import type {
  VenueOrderBookDto,
  MatrixCellDto,
  SlidingWindowOption,
  TickerStrategyConfigDto,
} from "../types";
import type { PerpTicker } from "../types/tickers";

export const WINDOW_SECONDS: Record<SlidingWindowOption, number> = {
  "10s": 10,
  "30s": 30,
  "1m": 60,
  "5m": 300,
  "15m": 900,
};

export interface SpreadSample {
  timestamp: number;
  netBps: number;
  grossBps: number;
}

export interface WindowStats {
  meanNetBps: number;
  deltaBps: number;
  sampleCount: number;
  percentileNetBps: number;
  percentileGrossBps: number;
}

/**
 * Rolling spread history tracker for each matrix pair over up to 15 minutes.
 * Maintains sliding ring buffers and computes statistical baselines.
 */
export class RollingSpreadTracker {
  private history = new Map<string, SpreadSample[]>();
  private readonly maxWindowMs = 900_000; // 15 minutes in milliseconds

  constructor() {
    this.history = new Map();
  }

  /**
   * Pre-seeds initial realistic baseline samples across all sliding window scales
   * (10s, 30s, 1m, 5m, 15m) so the user immediately sees non-zero baseline stats on launch.
   */
  public seedIfEmpty(pairKey: string, initialNetBps: number, initialGrossBps: number, now = Date.now()): void {
    if (this.history.has(pairKey) && (this.history.get(pairKey)?.length ?? 0) > 0) {
      return;
    }

    const samples: SpreadSample[] = [];
    // Key historical offsets covering 10s, 30s, 1m, 5m, and 15m windows
    const offsetsMs = [
      840_000, 720_000, 600_000, 480_000, 360_000, // 6m-14m
      240_000, 180_000, 120_000, 90_000,           // 1.5m-4m
      55_000, 45_000, 35_000,                      // 30s-1m
      25_000, 20_000, 14_000,                      // 10s-30s
      8_000, 5_000, 2_000,                         // < 10s
    ];

    for (let i = 0; i < offsetsMs.length; i++) {
      const offset = offsetsMs[i];
      const t = now - offset;
      // Controlled mean-reverting sinusoidal drift (-1.2 to +1.2 BPS)
      const drift = Math.sin((i * Math.PI) / 3) * 1.1;
      samples.push({
        timestamp: t,
        netBps: Math.round((initialNetBps + drift) * 100) / 100,
        grossBps: Math.round((initialGrossBps + drift) * 100) / 100,
      });
    }

    this.history.set(pairKey, samples);
  }

  /**
   * Records a new instantaneous spread observation for a pair.
   * Includes throttling to prevent render loops from flooding duplicate timestamps.
   */
  public record(pairKey: string, netBps: number, grossBps: number, now = Date.now()): void {
    let samples = this.history.get(pairKey);
    if (!samples) {
      samples = [];
      this.history.set(pairKey, samples);
    }

    const last = samples[samples.length - 1];
    if (last && now - last.timestamp < 500) {
      // Throttle: update recent observation rather than pushing duplicate entries
      last.netBps = netBps;
      last.grossBps = grossBps;
      return;
    }

    samples.push({ timestamp: now, netBps, grossBps });

    // Prune samples older than max retention window (15m)
    const cutoff = now - this.maxWindowMs;
    const pruned = samples.filter((s) => s.timestamp >= cutoff);
    this.history.set(pairKey, pruned);
  }

  /**
   * Computes baseline mean, delta BPS, and percentile values for a given sliding window.
   * Enforces percentile >= 50% (50% is median baseline) up to 99.9%.
   */
  public getStats(
    pairKey: string,
    currentNetBps: number,
    windowSeconds: number,
    nowOrPercentile: number = Date.now(),
    percentileOrNow?: number
  ): WindowStats {
    let now = Date.now();
    let percentile = 50;

    // Support both (pairKey, currentNetBps, windowSeconds, now, percentile)
    // and (pairKey, currentNetBps, windowSeconds, percentile, now)
    if (typeof percentileOrNow === "number") {
      if (nowOrPercentile > 1000) {
        now = nowOrPercentile;
        percentile = percentileOrNow;
      } else {
        percentile = nowOrPercentile;
        now = percentileOrNow;
      }
    } else {
      if (nowOrPercentile > 1000) {
        now = nowOrPercentile;
        percentile = 50;
      } else {
        percentile = nowOrPercentile;
        now = Date.now();
      }
    }

    // Strictly clamp percentile to [50%, 99.9%]
    const clampedPercentile = Math.max(50, Math.min(99.9, percentile));

    const samples = this.history.get(pairKey);
    if (!samples || samples.length === 0) {
      return {
        meanNetBps: currentNetBps,
        deltaBps: 0.0,
        sampleCount: 1,
        percentileNetBps: currentNetBps,
        percentileGrossBps: currentNetBps,
      };
    }

    const cutoff = now - windowSeconds * 1000;
    const windowSamples = samples.filter((s) => s.timestamp >= cutoff);

    if (windowSamples.length === 0) {
      const last = samples[samples.length - 1];
      return {
        meanNetBps: last.netBps,
        deltaBps: Math.round((currentNetBps - last.netBps) * 100) / 100,
        sampleCount: 1,
        percentileNetBps: last.netBps,
        percentileGrossBps: last.grossBps,
      };
    }

    // Isolate prior baseline samples (timestamp < now) to avoid diluting baseline with the current move
    const priorSamples = windowSamples.filter((s) => s.timestamp < now);
    const baselineSamples = priorSamples.length > 0 ? priorSamples : windowSamples;
    const sum = baselineSamples.reduce((acc, s) => acc + s.netBps, 0);
    const mean = sum / baselineSamples.length;
    const delta = currentNetBps - mean;

    // Compute empirical percentile rank across the sliding window
    const sortedNet = [...baselineSamples].map((s) => s.netBps).sort((a, b) => a - b);
    const sortedGross = [...baselineSamples].map((s) => s.grossBps).sort((a, b) => a - b);
    const pIdx = Math.min(
      sortedNet.length - 1,
      Math.max(0, Math.round(((clampedPercentile / 100) * (sortedNet.length - 1))))
    );

    return {
      meanNetBps: Math.round(mean * 100) / 100,
      deltaBps: Math.round(delta * 100) / 100,
      sampleCount: windowSamples.length,
      percentileNetBps: Math.round(sortedNet[pIdx] * 100) / 100,
      percentileGrossBps: Math.round(sortedGross[pIdx] * 100) / 100,
    };
  }

  public clear(): void {
    this.history.clear();
  }
}

// Global shared singleton instance across UI renders
export const globalSpreadTracker = new RollingSpreadTracker();

/**
 * Standard taker fees in basis points for known venues.
 */
export function getVenueFeeBps(venueIdOrName: string): number {
  const v = venueIdOrName.toLowerCase();
  if (v.includes("lighter") || v.includes("lgt")) return 1.5;
  if (v.includes("hyperliquid") || v.includes("hl")) return 2.0;
  if (v.includes("vanta") || v.includes("vnt")) return 3.0;
  if (v.includes("robinhood") || v.includes("rh")) return 2.5;
  return 2.0;
}

/**
 * Clean short label for venue.
 */
export function getVenueShortLabel(name: string): string {
  if (name.includes("Hyperliquid") || name.toLowerCase() === "hl") return "Hyperliquid";
  if (name.includes("Lighter") || name.toLowerCase() === "lighter" || name.toLowerCase() === "lgt") return "Lighter";
  if (name.includes("Vanta") || name.toLowerCase() === "vanta" || name.toLowerCase() === "vnt") return "Vanta";
  if (name.includes("Robinhood") || name.toLowerCase() === "robinhood" || name.toLowerCase() === "rh") return "Robinhood";
  return name.split(" ")[0];
}

/**
 * Computes Cross-Venue Matrix (Rows = Long Venue, Cols = Short Venue) for an active ticker.
 */
export function buildCrossVenueMatrix(
  symbol: string,
  orderbooks: VenueOrderBookDto[],
  windowSeconds: number,
  orderQty: number,
  minBps: number,
  strategyConfig?: Partial<TickerStrategyConfigDto>,
  tracker: RollingSpreadTracker = globalSpreadTracker
): {
  venues: string[];
  venueLabels: string[];
  grid: MatrixCellDto[][];
} {
  const venues = orderbooks.map((b) => b.venue_id);
  const venueLabels = orderbooks.map((b) => getVenueShortLabel(b.venue_name));
  const now = Date.now();

  const targetProfitBps = strategyConfig?.targetProfitBps ?? minBps;
  const percentile = Math.max(50, strategyConfig?.windowPercentile ?? 50);
  const executionMode = strategyConfig?.executionMode ?? "SINGLE_WAY";
  const maxUnits = strategyConfig?.maxAccumulatedUnits ?? 3;
  const currentUnits = strategyConfig?.currentAccumulatedUnits ?? 0;

  const grid: MatrixCellDto[][] = [];

  for (let i = 0; i < orderbooks.length; i++) {
    const row: MatrixCellDto[] = [];
    const buyBook = orderbooks[i];

    for (let j = 0; j < orderbooks.length; j++) {
      const sellBook = orderbooks[j];
      const rowId = buyBook.venue_id;
      const colId = sellBook.venue_id;
      const isDiagonal = i === j;

      if (isDiagonal) {
        row.push({
          rowId,
          colId,
          rowLabel: venueLabels[i],
          colLabel: venueLabels[j],
          isDiagonal: true,
          longPrice: buyBook.best_ask,
          shortPrice: sellBook.best_bid,
          grossSpreadBps: 0,
          feeBps: 0,
          closingFrictionBps: 0,
          netSpreadBps: 0,
          baselineMeanBps: 0,
          deltaBps: 0,
          sampleCount: 0,
          orderQty,
          estProfitUsd: 0,
          isExecutable: false,
          nominalHurdleBps: 0,
          targetProfitBps,
          percentileHurdleBps: 0,
          isConvergenceUnwind: false,
          longVenue: buyBook.venue_name,
          shortVenue: sellBook.venue_name,
          longSymbol: symbol,
          shortSymbol: symbol,
        });
        continue;
      }

      // Long Leg: Buy at buyBook.best_ask
      // Short Leg: Sell at sellBook.best_bid
      const longPrice = buyBook.best_ask;
      const shortPrice = sellBook.best_bid;

      // Gross Spread BPS: (Sell Bid - Buy Ask) / Buy Ask * 10,000
      const grossSpread = shortPrice - longPrice;
      const grossSpreadBps = longPrice > 0 ? (grossSpread / longPrice) * 10000 : 0;

      // Round-trip taker fees: 2 * (Fee(Long) + Fee(Short)) for opening and closing both legs
      const buyFeeBps = getVenueFeeBps(buyBook.venue_id);
      const sellFeeBps = getVenueFeeBps(sellBook.venue_id);
      const feeBps = Math.round((buyFeeBps + sellFeeBps) * 2 * 100) / 100;

      // Closing spread friction: Bid-ask spread required to close/unwind both legs later
      // To close Long: Sell at buyBook.best_bid (crossing buyBook spread)
      // To close Short: Buy back at sellBook.best_ask (crossing sellBook spread)
      const buySpreadBps = buyBook.spread_bps > 0 ? buyBook.spread_bps : ((buyBook.best_ask - buyBook.best_bid) / buyBook.mid_price) * 10000;
      const sellSpreadBps = sellBook.spread_bps > 0 ? sellBook.spread_bps : ((sellBook.best_ask - sellBook.best_bid) / sellBook.mid_price) * 10000;
      const closingFrictionBps = Math.max(0.1, buySpreadBps + sellSpreadBps);

      // Nominal hurdle = Round-trip Fees + Closing Spread Friction + Target Profit
      const nominalHurdleBps = Math.round((feeBps + closingFrictionBps + targetProfitBps) * 100) / 100;

      // Net spread BPS after deducting commission and closing spread friction
      const netSpreadBps = grossSpreadBps - feeBps - closingFrictionBps;

      // Record observation and compute sliding window baseline stats with percentile filter (min 50%)
      const pairKey = `venue:${symbol}:${rowId}:${colId}`;
      tracker.seedIfEmpty(pairKey, netSpreadBps, grossSpreadBps, now);
      tracker.record(pairKey, netSpreadBps, grossSpreadBps, now);
      const stats = tracker.getStats(pairKey, netSpreadBps, windowSeconds, percentile, now);

      // Estimated dollar profit for order quantity
      const estProfitUsd = ((netSpreadBps / 10000) * longPrice * orderQty);

      // Execution decision:
      // 1. Divergence Entry: gross spread exceeds nominal hurdle AND net spread meets historical percentile rank
      //    Plus grid capacity: currentAccumulatedUnits < maxAccumulatedUnits
      const hasCapacity = currentUnits < maxUnits;
      const isDivergenceOpportunity = grossSpreadBps >= nominalHurdleBps && netSpreadBps >= stats.percentileNetBps;

      // 2. Double-Way Convergence Unwind:
      //    When in DOUBLE_WAY mode and units are held (>0), if price difference converges back toward 0 (|grossSpreadBps| <= 2.5 BPS)
      const isConvergenceUnwind = executionMode === "DOUBLE_WAY" && currentUnits > 0 && Math.abs(grossSpreadBps) <= 2.5;

      const isExecutable = (isDivergenceOpportunity && hasCapacity) || isConvergenceUnwind;

      row.push({
        rowId,
        colId,
        rowLabel: venueLabels[i],
        colLabel: venueLabels[j],
        isDiagonal: false,
        longPrice,
        shortPrice,
        grossSpreadBps: Math.round(grossSpreadBps * 100) / 100,
        feeBps: Math.round(feeBps * 100) / 100,
        closingFrictionBps: Math.round(closingFrictionBps * 100) / 100,
        netSpreadBps: Math.round(netSpreadBps * 100) / 100,
        baselineMeanBps: stats.meanNetBps,
        deltaBps: stats.deltaBps,
        sampleCount: stats.sampleCount,
        orderQty,
        estProfitUsd: Math.round(estProfitUsd * 100) / 100,
        isExecutable,
        nominalHurdleBps,
        targetProfitBps,
        percentileHurdleBps: stats.percentileGrossBps,
        isConvergenceUnwind,
        longVenue: buyBook.venue_name,
        shortVenue: sellBook.venue_name,
        longSymbol: symbol,
        shortSymbol: symbol,
      });
    }
    grid.push(row);
  }

  return { venues, venueLabels, grid };
}

export interface TickerMarketQuote {
  symbol: string;
  base: string;
  bestBid: number;
  bestAsk: number;
  midPrice: number;
  spreadBps: number;
  bestBuyVenue: string;
  bestSellVenue: string;
  feeBps: number;
  basePrice: number;
}

/**
 * Computes Cross-Ticker Matrix (Rows = Long Ticker, Cols = Short Ticker)
 * across multi-market perpetual tickers.
 */
export function buildCrossTickerMatrix(
  tickers: PerpTicker[],
  quotesMap: Map<string, TickerMarketQuote>,
  windowSeconds: number,
  tradeNotionalUsd: number,
  minBps: number,
  strategyConfig?: Partial<TickerStrategyConfigDto>,
  tracker: RollingSpreadTracker = globalSpreadTracker
): {
  symbols: string[];
  symbolLabels: string[];
  grid: MatrixCellDto[][];
} {
  const symbols = tickers.map((t) => t.symbol);
  const symbolLabels = tickers.map((t) => t.base);
  const now = Date.now();

  const targetProfitBps = strategyConfig?.targetProfitBps ?? minBps;
  const percentile = Math.max(50, strategyConfig?.windowPercentile ?? 50);
  const executionMode = strategyConfig?.executionMode ?? "SINGLE_WAY";
  const maxUnits = strategyConfig?.maxAccumulatedUnits ?? 3;
  const currentUnits = strategyConfig?.currentAccumulatedUnits ?? 0;

  const grid: MatrixCellDto[][] = [];

  for (let i = 0; i < tickers.length; i++) {
    const row: MatrixCellDto[] = [];
    const tLong = tickers[i];
    const qLong = quotesMap.get(tLong.symbol) || {
      symbol: tLong.symbol,
      base: tLong.base,
      bestBid: tLong.basePrice * 0.9998,
      bestAsk: tLong.basePrice * 1.0002,
      midPrice: tLong.basePrice,
      spreadBps: 4.0,
      bestBuyVenue: "Hyperliquid",
      bestSellVenue: "Lighter",
      feeBps: 2.0,
      basePrice: tLong.basePrice,
    };

    for (let j = 0; j < tickers.length; j++) {
      const tShort = tickers[j];
      const qShort = quotesMap.get(tShort.symbol) || {
        symbol: tShort.symbol,
        base: tShort.base,
        bestBid: tShort.basePrice * 0.9998,
        bestAsk: tShort.basePrice * 1.0002,
        midPrice: tShort.basePrice,
        spreadBps: 4.0,
        bestBuyVenue: "Hyperliquid",
        bestSellVenue: "Lighter",
        feeBps: 2.0,
        basePrice: tShort.basePrice,
      };

      const rowId = tLong.symbol;
      const colId = tShort.symbol;
      const isDiagonal = i === j;

      if (isDiagonal) {
        row.push({
          rowId,
          colId,
          rowLabel: symbolLabels[i],
          colLabel: symbolLabels[j],
          isDiagonal: true,
          longPrice: qLong.bestAsk,
          shortPrice: qShort.bestBid,
          grossSpreadBps: 0,
          feeBps: 0,
          closingFrictionBps: 0,
          netSpreadBps: 0,
          baselineMeanBps: 0,
          deltaBps: 0,
          sampleCount: 0,
          orderQty: 0,
          estProfitUsd: 0,
          isExecutable: false,
          nominalHurdleBps: 0,
          targetProfitBps,
          percentileHurdleBps: 0,
          isConvergenceUnwind: false,
          longVenue: qLong.bestBuyVenue,
          shortVenue: qShort.bestSellVenue,
          longSymbol: tLong.symbol,
          shortSymbol: tShort.symbol,
        });
        continue;
      }

      // Relative value / pair statistical spread:
      // Buying Long leg at ask relative to baseline:
      // r_ask_long = (Ask - Base) / Base * 10,000 BPS
      const baseLong = qLong.basePrice > 0 ? qLong.basePrice : qLong.midPrice;
      const rAskLong = baseLong > 0 ? ((qLong.bestAsk - baseLong) / baseLong) * 10000 : 0;
      // Selling Short leg at bid relative to baseline:
      // r_bid_short = (Bid - Base) / Base * 10,000 BPS
      const baseShort = qShort.basePrice > 0 ? qShort.basePrice : qShort.midPrice;
      const rBidShort = baseShort > 0 ? ((qShort.bestBid - baseShort) / baseShort) * 10000 : 0;

      // Gross pair spread in BPS: selling the relatively strong asset and buying the relatively cheap asset
      const grossSpreadBps = rBidShort - rAskLong;

      // Round-trip commission: Taker fees on both legs (open and close)
      const feeBps = Math.round((qLong.feeBps + qShort.feeBps) * 2 * 100) / 100;

      // Closing spread friction: Bid-ask spread required to unwind both positions later
      const closingFrictionBps = Math.max(0.1, qLong.spreadBps + qShort.spreadBps);

      // Nominal hurdle = Round-trip Fees + Closing Spread Friction + Target Profit
      const nominalHurdleBps = Math.round((feeBps + closingFrictionBps + targetProfitBps) * 100) / 100;

      // Net spread BPS
      const netSpreadBps = grossSpreadBps - feeBps - closingFrictionBps;

      // Record observation and compute sliding window stats with percentile filter (min 50%)
      const pairKey = `ticker:${rowId}:${colId}`;
      tracker.seedIfEmpty(pairKey, netSpreadBps, grossSpreadBps, now);
      tracker.record(pairKey, netSpreadBps, grossSpreadBps, now);
      const stats = tracker.getStats(pairKey, netSpreadBps, windowSeconds, percentile, now);

      // Estimated dollar profit for the trade notional size
      const estProfitUsd = (netSpreadBps / 10000) * tradeNotionalUsd;

      // Order qty for the long asset
      const orderQty = qLong.bestAsk > 0 ? tradeNotionalUsd / qLong.bestAsk : 1.0;

      const hasCapacity = currentUnits < maxUnits;
      const isDivergenceOpportunity = grossSpreadBps >= nominalHurdleBps && netSpreadBps >= stats.percentileNetBps;
      const isConvergenceUnwind = executionMode === "DOUBLE_WAY" && currentUnits > 0 && Math.abs(grossSpreadBps) <= 2.5;
      const isExecutable = (isDivergenceOpportunity && hasCapacity) || isConvergenceUnwind;

      row.push({
        rowId,
        colId,
        rowLabel: symbolLabels[i],
        colLabel: symbolLabels[j],
        isDiagonal: false,
        longPrice: qLong.bestAsk,
        shortPrice: qShort.bestBid,
        grossSpreadBps: Math.round(grossSpreadBps * 100) / 100,
        feeBps: Math.round(feeBps * 100) / 100,
        closingFrictionBps: Math.round(closingFrictionBps * 100) / 100,
        netSpreadBps: Math.round(netSpreadBps * 100) / 100,
        baselineMeanBps: stats.meanNetBps,
        deltaBps: stats.deltaBps,
        sampleCount: stats.sampleCount,
        orderQty: Math.round(orderQty * 1000) / 1000,
        estProfitUsd: Math.round(estProfitUsd * 100) / 100,
        isExecutable,
        nominalHurdleBps,
        targetProfitBps,
        percentileHurdleBps: stats.percentileGrossBps,
        isConvergenceUnwind,
        longVenue: qLong.bestBuyVenue,
        shortVenue: qShort.bestSellVenue,
        longSymbol: tLong.symbol,
        shortSymbol: tShort.symbol,
      });
    }
    grid.push(row);
  }

  return { symbols, symbolLabels, grid };
}

/**
 * Generates heatmap visual styles for a matrix cell based on Net BPS, Δ BPS, and Convergence Unwind.
 */
export function getMatrixCellVisuals(cell: MatrixCellDto, minBps: number): {
  bgClass: string;
  borderClass: string;
  textClass: string;
  deltaClass: string;
  deltaBadgeBg: string;
  highlightRing: string;
} {
  if (cell.isDiagonal) {
    return {
      bgClass: "bg-surface-raised/60",
      borderClass: "border-surface-border/60",
      textClass: "text-slate-400 font-normal",
      deltaClass: "text-slate-400",
      deltaBadgeBg: "bg-transparent",
      highlightRing: "",
    };
  }

  const net = cell.netSpreadBps;
  const delta = cell.deltaBps;

  let deltaClass = "text-white/50";
  let deltaBadgeBg = "bg-white/[0.05] text-white/60 border border-white/[0.08]";
  if (delta > 0.05) {
    deltaClass = "text-emerald-400 font-bold";
    deltaBadgeBg = "bg-emerald-500/15 text-emerald-300 border border-emerald-500/30";
  } else if (delta < -0.05) {
    deltaClass = "text-rose-400 font-bold";
    deltaBadgeBg = "bg-rose-500/15 text-rose-300 border border-rose-500/30";
  }

  // If Double-Way convergence unwind is active
  if (cell.isConvergenceUnwind) {
    return {
      bgClass: "bg-cyan-500/15 hover:bg-cyan-500/25 active:bg-cyan-500/30",
      borderClass: "border-cyan-500/40 shadow-[0_0_12px_rgba(34,211,238,0.2)]",
      textClass: "text-cyan-300 font-bold",
      deltaClass: "text-cyan-300 font-semibold",
      deltaBadgeBg: "bg-cyan-500/20 text-cyan-200 border border-cyan-500/40",
      highlightRing: "ring-1 ring-cyan-400/50",
    };
  }

  if (net >= minBps) {
    // High conviction executable arbitrage
    return {
      bgClass: "bg-emerald-500/15 hover:bg-emerald-500/25 active:bg-emerald-500/30",
      borderClass: "border-emerald-500/40 shadow-[0_0_14px_rgba(52,211,153,0.2)]",
      textClass: "text-emerald-300 font-bold",
      deltaClass,
      deltaBadgeBg,
      highlightRing: "ring-1 ring-emerald-400/50",
    };
  }

  if (net > 0) {
    // Positive spread below minBps threshold
    return {
      bgClass: "bg-cyan-500/[0.07] hover:bg-cyan-500/[0.14] active:bg-cyan-500/20",
      borderClass: "border-cyan-500/25",
      textClass: "text-cyan-200/90 font-semibold",
      deltaClass,
      deltaBadgeBg,
      highlightRing: "",
    };
  }

  if (net >= -5.0) {
    // Slight negative / friction range
    return {
      bgClass: "bg-white/[0.03] hover:bg-white/[0.06] active:bg-white/[0.08]",
      borderClass: "border-white/[0.06]",
      textClass: "text-white/60 font-medium",
      deltaClass,
      deltaBadgeBg,
      highlightRing: "",
    };
  }

  // Deep negative spread
  return {
    bgClass: "bg-rose-500/[0.08] hover:bg-rose-500/[0.14] active:bg-rose-500/20",
    borderClass: "border-rose-500/20",
    textClass: "text-rose-300/80 font-medium",
    deltaClass,
    deltaBadgeBg,
    highlightRing: "",
  };
}
