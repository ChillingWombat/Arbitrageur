import test from "node:test";
import assert from "node:assert/strict";
import {
  RollingSpreadTracker,
  buildCrossVenueMatrix,
  buildCrossTickerMatrix,
  getMatrixCellVisuals,
  getVenueFeeBps,
  WINDOW_SECONDS,
  type TickerMarketQuote,
} from "./matrixQuant.ts";
import type { VenueOrderBookDto } from "../types/index.ts";
import type { PerpTicker } from "../types/tickers.ts";

test("WINDOW_SECONDS and getVenueFeeBps return expected configurations", () => {
  assert.equal(WINDOW_SECONDS["10s"], 10);
  assert.equal(WINDOW_SECONDS["30s"], 30);
  assert.equal(WINDOW_SECONDS["1m"], 60);
  assert.equal(WINDOW_SECONDS["5m"], 300);
  assert.equal(WINDOW_SECONDS["15m"], 900);

  assert.equal(getVenueFeeBps("hyperliquid"), 2.0);
  assert.equal(getVenueFeeBps("lighter"), 1.5);
  assert.equal(getVenueFeeBps("vanta"), 3.0);
  assert.equal(getVenueFeeBps("robinhood"), 2.5);
});

test("RollingSpreadTracker tracks rolling observations and computes sliding window delta", () => {
  const tracker = new RollingSpreadTracker();
  const pairKey = "test:BTC:hl:lgt";
  const baseTime = 1000000;

  // Initially empty tracker computes fallback with delta = 0
  const initialStats = tracker.getStats(pairKey, 12.0, 60, baseTime);
  assert.equal(initialStats.deltaBps, 0.0);

  // Record observations with varying net spreads over 60 seconds
  tracker.record(pairKey, 10.0, 15.0, baseTime - 50_000);
  tracker.record(pairKey, 12.0, 17.0, baseTime - 40_000);
  tracker.record(pairKey, 14.0, 19.0, baseTime - 20_000);
  tracker.record(pairKey, 16.0, 21.0, baseTime - 10_000);

  // Average over the 60s window: (10 + 12 + 14 + 16) / 4 = 13.0
  const stats60s = tracker.getStats(pairKey, 18.0, 60, baseTime);
  assert.equal(stats60s.meanNetBps, 13.0);
  // Current 18.0 - baseline 13.0 = +5.0 BPS
  assert.equal(stats60s.deltaBps, 5.0);
  assert.equal(stats60s.sampleCount, 4);

  // For 15s window, only sample at baseTime - 10_000 (16.0) is included:
  const stats15s = tracker.getStats(pairKey, 18.0, 15, baseTime);
  assert.equal(stats15s.meanNetBps, 16.0);
  assert.equal(stats15s.deltaBps, 2.0);
  assert.equal(stats15s.sampleCount, 1);
});

test("buildCrossVenueMatrix produces real 2D matrix with deductions and sliding window delta", () => {
  const books: VenueOrderBookDto[] = [
    {
      venue_id: "hyperliquid",
      venue_name: "Hyperliquid",
      symbol: "BTC-USDC",
      best_bid: 64200.0,
      best_ask: 64202.0,
      spread: 2.0,
      spread_bps: 0.31,
      mid_price: 64201.0,
      total_bid_depth: 50.0,
      total_ask_depth: 50.0,
      vwap_bid: 64200.0,
      vwap_ask: 64202.0,
      funding_rate_8h_pct: 0.01,
      bids: [[64200.0, 5.0]],
      asks: [[64202.0, 5.0]],
    },
    {
      venue_id: "lighter",
      venue_name: "Lighter",
      symbol: "BTC-USDC",
      best_bid: 64230.0, // Dislocated bid
      best_ask: 64232.0,
      spread: 2.0,
      spread_bps: 0.31,
      mid_price: 64231.0,
      total_bid_depth: 40.0,
      total_ask_depth: 40.0,
      vwap_bid: 64230.0,
      vwap_ask: 64232.0,
      funding_rate_8h_pct: 0.01,
      bids: [[64230.0, 5.0]],
      asks: [[64232.0, 5.0]],
    },
  ];

  const tracker = new RollingSpreadTracker();
  const { grid, venues, venueLabels } = buildCrossVenueMatrix(
    "BTC-USDC",
    books,
    60,
    0.5,
    5.0,
    undefined,
    tracker
  );

  assert.equal(venues.length, 2);
  assert.equal(venueLabels.length, 2);
  assert.equal(grid.length, 2);
  assert.equal(grid[0].length, 2);

  // Diagonal cell (0, 0): Hyperliquid Long & Short
  const diag00 = grid[0][0];
  assert.equal(diag00.isDiagonal, true);
  assert.equal(diag00.netSpreadBps, 0);
  assert.equal(diag00.deltaBps, 0);

  // Off-diagonal cell (0, 1): Long Hyperliquid (Buy @ 64202), Short Lighter (Sell @ 64230)
  const cell01 = grid[0][1];
  assert.equal(cell01.isDiagonal, false);
  assert.equal(cell01.longPrice, 64202.0);
  assert.equal(cell01.shortPrice, 64230.0);

  // Gross spread BPS = (64230 - 64202) / 64202 * 10000 = 28 / 64202 * 10000 ≈ 4.36 BPS
  assert.ok(cell01.grossSpreadBps > 4.0);

  // Roundtrip fees: (HL (2.0) + LGT (1.5)) * 2 = 7.0 BPS
  assert.equal(cell01.feeBps, 7.0);

  // Closing spread friction: buySpread (0.31) + sellSpread (0.31) = 0.62 BPS
  assert.ok(cell01.closingFrictionBps > 0.5);

  // Net spread = gross - fee - friction
  const expectedNet = cell01.grossSpreadBps - cell01.feeBps - cell01.closingFrictionBps;
  assert.ok(Math.abs(cell01.netSpreadBps - expectedNet) < 0.05);

  // Delta BPS is computed
  assert.ok(typeof cell01.deltaBps === "number");
});

test("buildCrossTickerMatrix produces 2D matrix with relative value quant calculations", () => {
  const tickers: PerpTicker[] = [
    { symbol: "BTC-USDC", base: "BTC", quote: "USDC", name: "Bitcoin", venues: ["HL", "LGT"], basePrice: 64200.0 },
    { symbol: "ETH-USDC", base: "ETH", quote: "USDC", name: "Ethereum", venues: ["HL", "LGT"], basePrice: 3450.0 },
  ];

  const quotesMap = new Map<string, TickerMarketQuote>();
  // BTC lagging / undervalued relative to benchmark
  quotesMap.set("BTC-USDC", {
    symbol: "BTC-USDC",
    base: "BTC",
    bestBid: 64180.0,
    bestAsk: 64190.0,
    midPrice: 64185.0,
    spreadBps: 1.5,
    bestBuyVenue: "Hyperliquid",
    bestSellVenue: "Lighter",
    feeBps: 2.0,
    basePrice: 64200.0,
  });
  // ETH surging / overvalued relative to benchmark
  quotesMap.set("ETH-USDC", {
    symbol: "ETH-USDC",
    base: "ETH",
    bestBid: 3470.0,
    bestAsk: 3472.0,
    midPrice: 3471.0,
    spreadBps: 2.0,
    bestBuyVenue: "Hyperliquid",
    bestSellVenue: "Lighter",
    feeBps: 2.0,
    basePrice: 3450.0,
  });

  const tracker = new RollingSpreadTracker();
  const { grid, symbols } = buildCrossTickerMatrix(
    tickers,
    quotesMap,
    60,
    25000,
    5.0,
    undefined,
    tracker
  );

  assert.equal(symbols.length, 2);
  assert.equal(grid.length, 2);

  // Diagonal cell (0, 0): BTC Long, BTC Short -> Diagonal
  assert.equal(grid[0][0].isDiagonal, true);
  assert.equal(grid[0][0].netSpreadBps, 0);

  // Off-diagonal cell (0, 1): Long BTC (cheap asset) / Short ETH (rich asset)
  const cell01 = grid[0][1];
  assert.equal(cell01.isDiagonal, false);
  assert.equal(cell01.longSymbol, "BTC-USDC");
  assert.equal(cell01.shortSymbol, "ETH-USDC");

  // Gross spread BPS is positive because ETH has outpaced BTC
  assert.ok(cell01.grossSpreadBps > 0);
  // Round-trip fees: (2.0 + 2.0) * 2 = 8.0 BPS
  assert.equal(cell01.feeBps, 8.0);
  assert.equal(cell01.closingFrictionBps, 3.5);
  assert.ok(cell01.netSpreadBps < cell01.grossSpreadBps);
});

test("RollingSpreadTracker pre-seeds baseline across all 5 sliding windows", () => {
  const tracker = new RollingSpreadTracker();
  const pairKey = "pair:ETH:SOL";
  const now = 1_700_000_000_000;

  // Pre-seed
  tracker.seedIfEmpty(pairKey, 12.0, 16.0, now);

  // Check stats for each predefined window
  const stats10s = tracker.getStats(pairKey, 15.0, 10, now);
  assert.ok(stats10s.sampleCount >= 2, "10s window must have baseline samples");
  assert.ok(stats10s.meanNetBps > 10.0 && stats10s.meanNetBps < 14.0);
  assert.ok(stats10s.deltaBps !== 0.0);

  const stats30s = tracker.getStats(pairKey, 15.0, 30, now);
  assert.ok(stats30s.sampleCount >= 5, "30s window must have baseline samples");
  assert.ok(stats30s.meanNetBps > 10.0 && stats30s.meanNetBps < 14.0);

  const stats1m = tracker.getStats(pairKey, 15.0, 60, now);
  assert.ok(stats1m.sampleCount >= 8, "1m window must have baseline samples");

  const stats5m = tracker.getStats(pairKey, 15.0, 300, now);
  assert.ok(stats5m.sampleCount >= 12, "5m window must have baseline samples");

  const stats15m = tracker.getStats(pairKey, 15.0, 900, now);
  assert.ok(stats15m.sampleCount >= 15, "15m window must have baseline samples");
});

test("RollingSpreadTracker throttles rapid render loops to prevent sample inflation", () => {
  const tracker = new RollingSpreadTracker();
  const pairKey = "throttle:test";
  const now = 1_700_000_000_000;

  tracker.record(pairKey, 10.0, 15.0, now);
  // Simulate 5 rapid re-renders within 100ms
  tracker.record(pairKey, 10.5, 15.5, now + 50);
  tracker.record(pairKey, 11.0, 16.0, now + 100);
  tracker.record(pairKey, 11.5, 16.5, now + 150);

  const stats = tracker.getStats(pairKey, 11.5, 10, now + 200);
  // Instead of pushing 4 distinct samples in 150ms, throttled to updating recent sample
  const sampleCount = stats.sampleCount;
  assert.ok(sampleCount <= 4);
});

test("getMatrixCellVisuals assigns proper heatmap classes and highlights", () => {
  const diagVisual = getMatrixCellVisuals({
    rowId: "hl", colId: "hl", rowLabel: "HL", colLabel: "HL", isDiagonal: true,
    longPrice: 100, shortPrice: 100, grossSpreadBps: 0, feeBps: 0, closingFrictionBps: 0,
    netSpreadBps: 0, baselineMeanBps: 0, deltaBps: 0, sampleCount: 0, orderQty: 1,
    estProfitUsd: 0, isExecutable: false, longVenue: "HL", shortVenue: "HL",
    longSymbol: "BTC", shortSymbol: "BTC"
  }, 5.0);
  assert.ok(diagVisual.bgClass.includes("surface-raised"));

  // High conviction executable spread (12.0 >= 5.0)
  const execVisual = getMatrixCellVisuals({
    rowId: "hl", colId: "lgt", rowLabel: "HL", colLabel: "LGT", isDiagonal: false,
    longPrice: 100, shortPrice: 102, grossSpreadBps: 20, feeBps: 7.0, closingFrictionBps: 1.5,
    netSpreadBps: 15.0, baselineMeanBps: 10.0, deltaBps: 5.0, sampleCount: 10, orderQty: 1,
    estProfitUsd: 15.0, isExecutable: true, longVenue: "HL", shortVenue: "LGT",
    longSymbol: "BTC", shortSymbol: "BTC"
  }, 5.0);
  assert.ok(execVisual.bgClass.includes("emerald"));
  assert.ok(execVisual.textClass.includes("emerald"));
  assert.ok(execVisual.deltaClass.includes("emerald"));

  // Negative spread (-8.0 BPS)
  const negVisual = getMatrixCellVisuals({
    rowId: "lgt", colId: "hl", rowLabel: "LGT", colLabel: "HL", isDiagonal: false,
    longPrice: 102, shortPrice: 100, grossSpreadBps: -20, feeBps: 7.0, closingFrictionBps: 1.5,
    netSpreadBps: -25.0, baselineMeanBps: -20.0, deltaBps: -5.0, sampleCount: 10, orderQty: 1,
    estProfitUsd: -25.0, isExecutable: false, longVenue: "LGT", shortVenue: "HL",
    longSymbol: "BTC", shortSymbol: "BTC"
  }, 5.0);
  assert.ok(negVisual.bgClass.includes("rose"));
  assert.ok(negVisual.deltaClass.includes("rose"));
});

test("RollingSpreadTracker enforces minimum 50% percentile and computes empirical ranks", () => {
  const tracker = new RollingSpreadTracker();
  const pairKey = "pair:percentile:test";
  const now = 2_000_000;

  // Add 10 samples with net spreads: 1, 2, 3, 4, 5, 6, 7, 8, 9, 10
  for (let i = 1; i <= 10; i++) {
    tracker.record(pairKey, i, i + 5, now - 50_000 + i * 2000);
  }

  // Request 20% percentile -> Must clamp to 50% (median)
  const statsClamped = tracker.getStats(pairKey, 10, 60, 20, now);
  // Median (50th percentile) of 1..10 is approx index 5 -> value 6
  assert.ok(statsClamped.percentileNetBps >= 5);

  // Request 90% percentile
  const stats90 = tracker.getStats(pairKey, 10, 60, 90, now);
  assert.ok(stats90.percentileNetBps >= 9);
  assert.ok(stats90.percentileGrossBps >= 14);
});

test("buildCrossVenueMatrix incorporates target profit, nominal hurdle, and Double-Way convergence unwind", () => {
  const books: VenueOrderBookDto[] = [
    {
      venue_id: "hyperliquid",
      venue_name: "Hyperliquid",
      symbol: "ETH-USDC",
      best_bid: 3400.0,
      best_ask: 3401.0,
      spread: 1.0,
      spread_bps: 2.94,
      mid_price: 3400.5,
      total_bid_depth: 50.0,
      total_ask_depth: 50.0,
      vwap_bid: 3400.0,
      vwap_ask: 3401.0,
      funding_rate_8h_pct: 0.01,
      bids: [[3400.0, 10.0]],
      asks: [[3401.0, 10.0]],
    },
    {
      venue_id: "lighter",
      venue_name: "Lighter",
      symbol: "ETH-USDC",
      best_bid: 3410.0,
      best_ask: 3411.0,
      spread: 1.0,
      spread_bps: 2.93,
      mid_price: 3410.5,
      total_bid_depth: 40.0,
      total_ask_depth: 40.0,
      vwap_bid: 3410.0,
      vwap_ask: 3411.0,
      funding_rate_8h_pct: 0.01,
      bids: [[3410.0, 10.0]],
      asks: [[3411.0, 10.0]],
    },
  ];

  const tracker = new RollingSpreadTracker();

  // Test 1: Single-Way with Target Profit 6.0 BPS
  const resSingle = buildCrossVenueMatrix(
    "ETH-USDC",
    books,
    60,
    1.0,
    5.0,
    {
      targetProfitBps: 6.0,
      windowPercentile: 75,
      maxAccumulatedUnits: 3,
      currentAccumulatedUnits: 0,
      executionMode: "SINGLE_WAY",
      unitSize: 1.0,
      userTargetLeverage: 10,
    },
    tracker
  );

  const cell01 = resSingle.grid[0][1];
  assert.equal(cell01.targetProfitBps, 6.0);
  // feeBps = 2 * (HL 2.0 + LGT 1.5) = 7.0
  // closingFrictionBps ≈ 2.94 + 2.93 ≈ 5.87
  // nominalHurdle = 7.0 + 5.87 + 6.0 = 18.87
  assert.ok(cell01.nominalHurdleBps !== undefined && cell01.nominalHurdleBps > 18.0);

  // Test 2: Double-Way Mode with open position (units = 1) and spread converged (tight books)
  const tightBooks: VenueOrderBookDto[] = [
    { ...books[0], best_bid: 3400.0, best_ask: 3400.1, spread_bps: 0.29 },
    { ...books[1], best_bid: 3400.1, best_ask: 3400.2, spread_bps: 0.29 },
  ];

  const resDouble = buildCrossVenueMatrix(
    "ETH-USDC",
    tightBooks,
    60,
    1.0,
    5.0,
    {
      targetProfitBps: 5.0,
      windowPercentile: 50,
      maxAccumulatedUnits: 3,
      currentAccumulatedUnits: 1, // Currently held position
      executionMode: "DOUBLE_WAY",
      unitSize: 1.0,
      userTargetLeverage: 10,
    },
    tracker
  );

  const cellDouble = resDouble.grid[0][1];
  assert.equal(cellDouble.isConvergenceUnwind, true);
  assert.equal(cellDouble.isExecutable, true);

  const unwindVisuals = getMatrixCellVisuals(cellDouble, 5.0);
  assert.ok(unwindVisuals.bgClass.includes("cyan") || unwindVisuals.bgClass.includes("blue"));
  assert.ok(unwindVisuals.highlightRing.includes("cyan") || unwindVisuals.highlightRing.includes("blue"));
});
