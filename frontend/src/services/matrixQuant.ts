import type {
  VenueOrderBookDto,
  VenueFeesDto,
  MatrixCellDto,
  PairBaselineDto,
  SlidingWindowOption,
  SpatialOpportunityDto,
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

export type DurationUnit = "minutes" | "hours" | "days";

export function durationParts(seconds: number): { value: number; unit: DurationUnit } {
  const s = Number.isFinite(seconds) ? Math.max(0, Math.round(seconds)) : 0;
  if (s >= 86_400 && s % 86_400 === 0) return { value: s / 86_400, unit: "days" };
  if (s >= 3_600 && s % 3_600 === 0) return { value: s / 3_600, unit: "hours" };
  if (s >= 60 && s % 60 === 0) return { value: s / 60, unit: "minutes" };
  return { value: Math.max(1, Math.ceil(s / 60)), unit: "minutes" };
}

export function durationSeconds(value: number, unit: DurationUnit): number {
  if (!Number.isFinite(value)) return NaN;
  const mult = unit === "days" ? 86_400 : unit === "hours" ? 3_600 : 60;
  return Math.round(value) * mult;
}

export function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return "0s";
  const s = Math.round(seconds);
  if (s % 86_400 === 0) return `${s / 86_400}d`;
  if (s % 3_600 === 0) return `${s / 3_600}h`;
  if (s % 60 === 0) return `${s / 60}m`;
  return `${s}s`;
}

export function parseDuration(value: string): number | null {
  const m = /^\s*(\d+)\s*([smhd])\s*$/i.exec(value ?? "");
  if (!m) return null;
  const mult = m[2].toLowerCase() === "d" ? 86_400 : m[2].toLowerCase() === "h" ? 3_600 : m[2].toLowerCase() === "m" ? 60 : 1;
  const secs = parseInt(m[1], 10) * mult;
  return secs > 0 && Number.isFinite(secs) ? secs : null;
}

export const WINDOW_UNIT_BOUNDS: Record<DurationUnit, { min: number; max: number }> = {
  minutes: { min: 5, max: 43_200 },
  hours: { min: 1, max: 720 },
  days: { min: 1, max: 30 },
};

export function clampWindowValue(value: number, unit: DurationUnit): number {
  const b = WINDOW_UNIT_BOUNDS[unit];
  if (!Number.isFinite(value)) return b.min;
  return Math.min(Math.max(Math.floor(value), b.min), b.max);
}

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
 * Standard taker fee in basis points for a venue — hydrated from the
 * backend's `venue_fee_bps` executor table via `get_venue_fees` so this
 * surface can never drift from executed cost (the previous hand-maintained
 * copy did, twice). `setVenueFees` is fed by `api.getVenueFees()` at mount
 * (LiveMatrix); until hydration lands, ids fall back to the executor's
 * default arm. EVM AMM rows are the venue's documented display default —
 * per-pair truth rides pool metadata.
 */
let venueFees: Map<string, number> | null = null;
let venueFeeDefault = 4.0; // executor default arm; corrected on hydrate

/** Replace the fee table — called from the `get_venue_fees` IPC response
 *  and from tests. */
export function setVenueFees(
  fees: ReadonlyArray<{ venue_id: string; taker_bps: number }> | VenueFeesDto["fees"],
  defaultBps: number,
): void {
  venueFees = new Map(fees.map((f) => [f.venue_id.toLowerCase(), f.taker_bps]));
  venueFeeDefault = defaultBps;
}

/** Display-name shorthands → canonical venue ids used by the fee table. */
const VENUE_FEE_ALIASES: Record<string, string> = {
  hl: "hyperliquid",
  lgt: "lighter",
  vnt: "vanta",
  rh: "robinhood",
  "robinhood lighter": "robinhood",
  arc: "arcus",
  pop: "popdex",
};

export function getVenueFeeBps(venueIdOrName: string): number {
  const v = venueIdOrName.toLowerCase().trim();
  const id = VENUE_FEE_ALIASES[v] ?? v;
  return venueFees?.get(id) ?? venueFeeDefault;
}

/**
 * Clean short label for venue.
 */
export function getVenueShortLabel(name: string): string {
  if (name.includes("Robinhood") || name.toLowerCase() === "robinhood" || name.toLowerCase() === "rh") return "Robinhood Lighter";
  if (name.includes("Hyperliquid") || name.toLowerCase() === "hl") return "Hyperliquid";
  if (name.includes("Lighter") || name.toLowerCase() === "lighter" || name.toLowerCase() === "lgt") return "Lighter";
  if (name.includes("Vanta") || name.toLowerCase() === "vanta" || name.toLowerCase() === "vnt") return "Vanta";
  if (name.includes("Arcus") || name.toLowerCase() === "arcus" || name.toLowerCase() === "arc") return "Arcus";
  if (name.includes("PopDEX") || name.toLowerCase() === "popdex" || name.toLowerCase() === "pop") return "PopDEX";
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
  tracker: RollingSpreadTracker = globalSpreadTracker,
  durableBaselines?: PairBaselineDto[],
  routeEconomics?: SpatialOpportunityDto[]
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
          baselineReady: false,
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

      if (longPrice <= 0 || shortPrice <= 0) {
        row.push({
          rowId,
          colId,
          rowLabel: venueLabels[i],
          colLabel: venueLabels[j],
          isDiagonal: false,
          longPrice: 0,
          shortPrice: 0,
          grossSpreadBps: 0,
          feeBps: 0,
          closingFrictionBps: 0,
          netSpreadBps: 0,
          baselineMeanBps: 0,
          deltaBps: 0,
          sampleCount: 0,
          baselineReady: false,
          orderQty: 0,
          estProfitUsd: 0,
          isExecutable: false,
          costReady: false,
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

      const route = routeEconomics?.find(
        (r) =>
          r.buy_venue_id.toLowerCase() === rowId.toLowerCase() &&
          r.sell_venue_id.toLowerCase() === colId.toLowerCase()
      );
      const costReady = routeEconomics !== undefined && route != null;

      // Gross Spread BPS: (Sell Bid - Buy Ask) / Buy Ask * 10,000
      const grossSpread = shortPrice - longPrice;
      const localGrossSpreadBps = longPrice > 0 ? (grossSpread / longPrice) * 10000 : 0;

      let grossSpreadBps: number;
      let feeBps: number;
      let closingFrictionBps: number;
      let netSpreadBps: number;
      let expectedFrictionBps: number;
      let nominalHurdleBps: number | undefined;
      let requiredGrossBps: number | undefined;
      let entrySlippageBps: number | undefined;
      let exitSlippageBps: number | undefined;
      let slippageOk = false;
      let depthOk = false;
      let estProfitUsd: number;

      if (route != null) {
        grossSpreadBps = route.gross_spread_bps;
        netSpreadBps = route.net_spread_bps;
        feeBps = route.round_trip_commission_bps;
        closingFrictionBps = route.closing_spread_bps;
        expectedFrictionBps = route.expected_friction_bps;
        requiredGrossBps = route.required_gross_bps;
        nominalHurdleBps = route.required_gross_bps;
        entrySlippageBps = route.entry_slippage_bps;
        exitSlippageBps = route.exit_slippage_bps;
        slippageOk = route.slippage_ok;
        depthOk = route.depth_ok;
        estProfitUsd = route.estimated_profit_usd;
      } else if (routeEconomics !== undefined) {
        grossSpreadBps = localGrossSpreadBps;
        netSpreadBps = localGrossSpreadBps;
        feeBps = 0;
        closingFrictionBps = 0;
        expectedFrictionBps = 0;
        estProfitUsd = 0;
      } else {
        grossSpreadBps = localGrossSpreadBps;

        // Round-trip taker fees: 2 * (Fee(Long) + Fee(Short)) for opening and closing both legs
        const buyFeeBps = getVenueFeeBps(buyBook.venue_id);
        const sellFeeBps = getVenueFeeBps(sellBook.venue_id);
        feeBps = Math.round((buyFeeBps + sellFeeBps) * 2 * 100) / 100;

        // Closing spread friction: Bid-ask spread required to close/unwind both legs later
        const buySpreadBps = buyBook.spread_bps > 0 ? buyBook.spread_bps : ((buyBook.best_ask - buyBook.best_bid) / buyBook.mid_price) * 10000;
        const sellSpreadBps = sellBook.spread_bps > 0 ? sellBook.spread_bps : ((sellBook.best_ask - sellBook.best_bid) / sellBook.mid_price) * 10000;
        closingFrictionBps = Math.max(0.1, buySpreadBps + sellSpreadBps);

        expectedFrictionBps = feeBps + closingFrictionBps;
        nominalHurdleBps = Math.round((expectedFrictionBps + targetProfitBps) * 100) / 100;
        netSpreadBps = grossSpreadBps - expectedFrictionBps;
        estProfitUsd = (netSpreadBps / 10000) * longPrice * orderQty;
      }

      let stats: WindowStats;
      let baselineReady: boolean;
      if (durableBaselines !== undefined) {
        const durable = durableBaselines.find(
          (b) =>
            b.buy_venue.toLowerCase() === rowId.toLowerCase() &&
            b.sell_venue.toLowerCase() === colId.toLowerCase()
        );
        baselineReady =
          durable != null &&
          durable.mean_gross_bps != null &&
          durable.percentile_gross_bps != null;
        if (baselineReady && durable) {
          const meanNet = durable.mean_gross_bps! - expectedFrictionBps;
          const pctNet = durable.percentile_gross_bps! - expectedFrictionBps;
          stats = {
            meanNetBps: Math.round(meanNet * 100) / 100,
            deltaBps: Math.round((netSpreadBps - meanNet) * 100) / 100,
            sampleCount: durable.sample_count,
            percentileNetBps: Math.round(pctNet * 100) / 100,
            percentileGrossBps: durable.percentile_gross_bps!,
          };
        } else {
          stats = {
            meanNetBps: 0,
            deltaBps: 0,
            sampleCount: durable?.sample_count ?? 0,
            percentileNetBps: 0,
            percentileGrossBps: 0,
          };
        }
      } else {
        baselineReady = true;
        const pairKey = `venue:${symbol}:${rowId}:${colId}`;
        tracker.record(pairKey, netSpreadBps, grossSpreadBps, now);
        stats = tracker.getStats(pairKey, netSpreadBps, windowSeconds, percentile, now);
      }

      const hasCapacity = currentUnits < maxUnits;
      const isDivergenceOpportunity = routeEconomics !== undefined
        ? costReady &&
          baselineReady &&
          slippageOk &&
          depthOk &&
          grossSpreadBps >= (requiredGrossBps ?? Infinity) &&
          netSpreadBps >= stats.percentileNetBps
        : baselineReady &&
          grossSpreadBps >= (nominalHurdleBps ?? Infinity) &&
          netSpreadBps >= stats.percentileNetBps;

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
        baselineReady,
        orderQty: route != null ? route.order_qty : orderQty,
        estProfitUsd: Math.round(estProfitUsd * 100) / 100,
        isExecutable,
        costReady,
        roundTripCommissionBps: route?.round_trip_commission_bps,
        entrySlippageBps,
        exitSlippageBps,
        expectedFrictionBps: route?.expected_friction_bps,
        requiredGrossBps,
        slippageOk: costReady ? slippageOk : undefined,
        depthOk: costReady ? depthOk : undefined,
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
    const qLong = quotesMap.get(tLong.symbol);

    for (let j = 0; j < tickers.length; j++) {
      const tShort = tickers[j];
      const qShort = quotesMap.get(tShort.symbol);

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
          longPrice: qLong?.bestAsk ?? 0,
          shortPrice: qShort?.bestBid ?? 0,
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
          longVenue: qLong?.bestBuyVenue ?? "-",
          shortVenue: qShort?.bestSellVenue ?? "-",
          longSymbol: tLong.symbol,
          shortSymbol: tShort.symbol,
        });
        continue;
      }

      if (!qLong || !qShort || qLong.bestAsk <= 0 || qShort.bestBid <= 0 || qLong.bestBid <= 0 || qShort.bestAsk <= 0) {
        row.push({
          rowId,
          colId,
          rowLabel: symbolLabels[i],
          colLabel: symbolLabels[j],
          isDiagonal: false,
          longPrice: qLong?.bestAsk ?? 0,
          shortPrice: qShort?.bestBid ?? 0,
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
          longVenue: qLong?.bestBuyVenue ?? "-",
          shortVenue: qShort?.bestSellVenue ?? "-",
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
      textClass: "text-muted font-normal",
      deltaClass: "text-muted",
      deltaBadgeBg: "bg-transparent",
      highlightRing: "",
    };
  }

  const net = cell.netSpreadBps;
  const delta = cell.deltaBps;

  let deltaClass = "text-slate-500";
  let deltaBadgeBg = "bg-slate-100 text-slate-600 border border-slate-200";
  if (delta > 0.05) {
    deltaClass = "text-emerald-700 font-bold";
    deltaBadgeBg = "bg-emerald-100 text-emerald-800 border border-emerald-200";
  } else if (delta < -0.05) {
    deltaClass = "text-rose-700 font-bold";
    deltaBadgeBg = "bg-rose-100 text-rose-800 border border-rose-200";
  }

  // If Double-Way convergence unwind is active
  if (cell.isConvergenceUnwind) {
    return {
      bgClass: "bg-blue-50/90 hover:bg-blue-100 active:bg-blue-200/80",
      borderClass: "border-blue-400 shadow-xs",
      textClass: "text-blue-700 font-bold",
      deltaClass: "text-blue-600 font-semibold",
      deltaBadgeBg: "bg-blue-100 text-blue-800 border border-blue-200",
      highlightRing: "ring-2 ring-blue-500/50",
    };
  }

  if (cell.costReady === false) {
    return {
      bgClass: "bg-surface-raised/40 hover:bg-surface-raised/60",
      borderClass: "border-surface-border/60",
      textClass: "text-muted font-normal",
      deltaClass,
      deltaBadgeBg,
      highlightRing: "",
    };
  }

  if (net >= minBps) {
    // High conviction executable arbitrage
    return {
      bgClass: "bg-emerald-50/90 hover:bg-emerald-100 active:bg-emerald-200/80",
      borderClass: "border-emerald-400 shadow-xs",
      textClass: "text-emerald-700 font-bold",
      deltaClass,
      deltaBadgeBg,
      highlightRing: "ring-2 ring-emerald-500/50",
    };
  }

  if (net > 0) {
    // Positive spread below minBps threshold
    return {
      bgClass: "bg-sky-50/80 hover:bg-sky-100/80 active:bg-sky-200/70",
      borderClass: "border-sky-300",
      textClass: "text-sky-700 font-semibold",
      deltaClass,
      deltaBadgeBg,
      highlightRing: "",
    };
  }

  if (net >= -5.0) {
    // Slight negative / friction range
    return {
      bgClass: "bg-surface hover:bg-surface-raised active:bg-surface-raised",
      borderClass: "border-surface-border",
      textClass: "text-slate-600 font-medium",
      deltaClass,
      deltaBadgeBg,
      highlightRing: "",
    };
  }

  // Deep negative spread
  return {
    bgClass: "bg-rose-50/50 hover:bg-rose-100/60 active:bg-rose-100",
    borderClass: "border-rose-200",
    textClass: "text-rose-600 font-medium",
    deltaClass,
    deltaBadgeBg,
    highlightRing: "",
  };
}
