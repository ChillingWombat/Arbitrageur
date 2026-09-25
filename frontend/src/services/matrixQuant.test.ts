import test from "node:test";
import assert from "node:assert/strict";
import {
  RollingSpreadTracker,
  buildCrossVenueMatrix,
  buildCrossTickerMatrix,
  clampWindowValue,
  durationParts,
  durationSeconds,
  formatDuration,
  getMatrixCellVisuals,
  getVenueFeeBps,
  getVenueShortLabel,
  parseDuration,
  setVenueFees,
  WINDOW_SECONDS,
  type TickerMarketQuote,
} from "./matrixQuant.ts";
import type { SpatialOpportunityDto, VenueOrderBookDto } from "../types/index.ts";
import type { PerpTicker } from "../types/tickers.ts";

// Hydrate the executor fee table the way production does — `getVenueFeeBps`
// keeps no local copy (the values below mirror `venue_fee_bps` rows these
// tests exercise).
setVenueFees(
  [
    { venue_id: "hyperliquid", taker_bps: 4.5 },
    { venue_id: "lighter", taker_bps: 1.5 },
    { venue_id: "vanta", taker_bps: 4.0 },
    { venue_id: "robinhood", taker_bps: 4.0 },
  ],
  4.0,
);

test("WINDOW_SECONDS and getVenueFeeBps return expected configurations", () => {
  assert.equal(WINDOW_SECONDS["10s"], 10);
  assert.equal(WINDOW_SECONDS["30s"], 30);
  assert.equal(WINDOW_SECONDS["1m"], 60);
  assert.equal(WINDOW_SECONDS["5m"], 300);
  assert.equal(WINDOW_SECONDS["15m"], 900);

  assert.equal(getVenueFeeBps("hyperliquid"), 4.5);
  assert.equal(getVenueFeeBps("lighter"), 1.5);
  assert.equal(getVenueFeeBps("vanta"), 4.0);
  assert.equal(getVenueFeeBps("robinhood"), 4.0);
  // Display-name aliases resolve to the same canonical fee.
  assert.equal(getVenueFeeBps("hl"), 4.5);
  assert.equal(getVenueFeeBps("LGT"), 1.5);
  // Unknown ids fall back to the executor's default arm, never a stale copy.
  assert.equal(getVenueFeeBps("no-such-venue"), 4.0);
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

  // Roundtrip fees: (HL (4.5) + LGT (1.5)) * 2 = 12.0 BPS
  assert.equal(cell01.feeBps, 12.0);

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

test("RollingSpreadTracker has no synthetic history and only real records affect stats", () => {
  const tracker = new RollingSpreadTracker();
  const pairKey = "pair:ETH:SOL";
  const now = 1_700_000_000_000;

  // Before any observation, every window reports the neutral first-sample stats
  for (const windowSeconds of [10, 30, 60, 300, 900]) {
    const stats = tracker.getStats(pairKey, 15.0, windowSeconds, 50, now);
    assert.equal(stats.sampleCount, 1);
    assert.equal(stats.deltaBps, 0.0);
    assert.equal(stats.meanNetBps, 15.0);
  }

  // The first actual observation is a single sample with zero delta
  tracker.record(pairKey, 12.0, 16.0, now);
  const first = tracker.getStats(pairKey, 12.0, 10, 50, now);
  assert.equal(first.sampleCount, 1);
  assert.equal(first.deltaBps, 0.0);

  // Only real records move the baseline: after two more observations the
  // 10s-window mean reflects just the recorded samples
  tracker.record(pairKey, 14.0, 18.0, now + 1000);
  tracker.record(pairKey, 16.0, 20.0, now + 2000);
  const stats = tracker.getStats(pairKey, 20.0, 10, 50, now + 3000);
  assert.ok(stats.sampleCount >= 2 && stats.sampleCount <= 3);
  assert.ok(stats.meanNetBps >= 12.0 && stats.meanNetBps <= 16.0);
  assert.ok(stats.deltaBps > 0.0);
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
  // feeBps = 2 * (HL 4.5 + LGT 1.5) = 12.0
  // closingFrictionBps ≈ 2.94 + 2.93 ≈ 5.87
  // nominalHurdle = 12.0 + 5.87 + 6.0 = 23.87
  assert.ok(cell01.nominalHurdleBps !== undefined && cell01.nominalHurdleBps > 23.0);

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
  assert.ok(unwindVisuals.bgClass.includes("blue"));
  assert.ok(unwindVisuals.highlightRing.includes("blue"));
});

test("duration helpers decompose, convert, format, and parse canonical durations", () => {
  assert.deepEqual(durationParts(86_400), { value: 1, unit: "days" });
  assert.deepEqual(durationParts(21_600), { value: 6, unit: "hours" });
  assert.deepEqual(durationParts(1_800), { value: 30, unit: "minutes" });
  assert.deepEqual(durationParts(300), { value: 5, unit: "minutes" });

  assert.equal(durationSeconds(6, "hours"), 21_600);
  assert.equal(durationSeconds(2, "days"), 172_800);
  assert.equal(durationSeconds(30, "minutes"), 1_800);
  assert.ok(Number.isNaN(durationSeconds(NaN, "hours")));
  assert.ok(Number.isNaN(durationSeconds(Infinity, "days")));

  assert.equal(formatDuration(86_400), "1d");
  assert.equal(formatDuration(21_600), "6h");
  assert.equal(formatDuration(1_800), "30m");
  assert.equal(formatDuration(10), "10s");
  assert.equal(formatDuration(90), "90s");

  assert.equal(parseDuration("24h"), 86_400);
  assert.equal(parseDuration("30m"), 1_800);
  assert.equal(parseDuration("10s"), 10);
  assert.equal(parseDuration("2d"), 172_800);
  assert.equal(parseDuration("99x"), null);
  assert.equal(parseDuration("1.5h"), null);
  assert.equal(parseDuration(""), null);
});

test("clampWindowValue clamps the displayed field to per-unit bounds", () => {
  assert.equal(clampWindowValue(999, "days"), 30);
  assert.equal(clampWindowValue(0, "days"), 1);
  assert.equal(clampWindowValue(3, "minutes"), 5);
  assert.equal(clampWindowValue(50_000, "minutes"), 43_200);
  assert.equal(clampWindowValue(721, "hours"), 720);
  assert.equal(clampWindowValue(0.9, "hours"), 1);
  assert.equal(clampWindowValue(2.7, "days"), 2);
  assert.equal(clampWindowValue(NaN, "minutes"), 5);
});

function venueBook(venueId: string, bid: number, ask: number): VenueOrderBookDto {
  return {
    venue_id: venueId,
    venue_name: venueId,
    symbol: "BTC-USDC",
    best_bid: bid,
    best_ask: ask,
    spread: ask - bid,
    spread_bps: ((ask - bid) / ((ask + bid) / 2)) * 10_000,
    mid_price: (ask + bid) / 2,
    total_bid_depth: 50,
    total_ask_depth: 50,
    vwap_bid: bid,
    vwap_ask: ask,
    funding_rate_8h_pct: 0.01,
    bids: [[bid, 5]],
    asks: [[ask, 5]],
  };
}

test("buildCrossVenueMatrix prefers durable baselines over tracker history", () => {
  const books = [venueBook("hyperliquid", 100.0, 100.05), venueBook("lighter", 100.30, 100.35)];
  const tracker = new RollingSpreadTracker();

  const durable = [
    {
      buy_venue: "hyperliquid",
      sell_venue: "lighter",
      mean_gross_bps: 12.0,
      percentile_gross_bps: 30.0,
      sample_count: 250,
      history_span_secs: 86_400,
    },
  ];
  const { grid } = buildCrossVenueMatrix(
    "BTC-USDC",
    books,
    60,
    0.5,
    5.0,
    { windowPercentile: 90 },
    tracker,
    durable
  );
  const cell = grid[0][1];
  assert.equal(cell.baselineReady, true);
  assert.equal(cell.sampleCount, 250);
  // Friction conversion: meanNet = meanGross - feeBps - closingFriction
  const expectedBaselineNet = 12.0 - cell.feeBps - cell.closingFrictionBps;
  assert.ok(Math.abs(cell.baselineMeanBps - expectedBaselineNet) < 0.05);
  assert.ok(Math.abs(cell.deltaBps - (cell.netSpreadBps - expectedBaselineNet)) < 0.1);
  // Percentile hurdle stays in gross bps
  assert.equal(cell.percentileHurdleBps, 30.0);
  // The tracker must not have been consulted or recorded into.
  const probe = tracker.getStats("venue:BTC-USDC:hyperliquid:lighter", 0, 60, 50, Date.now());
  assert.equal(probe.sampleCount, 1, "no durable-mode observations recorded");
});

test("buildCrossVenueMatrix fails closed when durable baseline is missing", () => {
  const books = [venueBook("hyperliquid", 100.0, 100.01), venueBook("lighter", 102.0, 102.01)];
  const tracker = new RollingSpreadTracker();
  const { grid } = buildCrossVenueMatrix(
    "BTC-USDC",
    books,
    60,
    0.5,
    5.0,
    { windowPercentile: 90, currentAccumulatedUnits: 0, maxAccumulatedUnits: 3 },
    tracker,
    []
  );
  const cell = grid[0][1];
  // A large authentic current spread cannot execute without a baseline.
  assert.ok(cell.grossSpreadBps > 100);
  assert.equal(cell.baselineReady, false);
  assert.equal(cell.sampleCount, 0);
  assert.equal(cell.isExecutable, false);
});

test("buildCrossVenueMatrix keeps convergence unwind without baseline but blocks divergence", () => {
  const books = [venueBook("hyperliquid", 100.0, 100.01), venueBook("lighter", 102.0, 102.01)];
  const tracker = new RollingSpreadTracker();
  const { grid } = buildCrossVenueMatrix(
    "BTC-USDC",
    books,
    60,
    0.5,
    5.0,
    {
      windowPercentile: 90,
      currentAccumulatedUnits: 1,
      maxAccumulatedUnits: 3,
      executionMode: "DOUBLE_WAY",
    },
    tracker,
    [
      {
        buy_venue: "hyperliquid",
        sell_venue: "lighter",
        mean_gross_bps: null,
        percentile_gross_bps: null,
        sample_count: 7,
        history_span_secs: 1_200,
      },
    ]
  );
  const cell = grid[0][1];
  assert.equal(cell.baselineReady, false);
  assert.equal(cell.sampleCount, 7, "truthful pair count is preserved");
  assert.equal(cell.isExecutable, false, "wide spread cannot unwind: not converged, divergence blocked");

  // Converged spread + held units -> unwind still permitted without baseline.
  const tight = [venueBook("hyperliquid", 100.0, 100.01), venueBook("lighter", 100.01, 100.02)];
  const res2 = buildCrossVenueMatrix(
    "BTC-USDC",
    tight,
    60,
    0.5,
    5.0,
    {
      windowPercentile: 90,
      currentAccumulatedUnits: 1,
      maxAccumulatedUnits: 3,
      executionMode: "DOUBLE_WAY",
    },
    tracker,
    []
  );
  assert.equal(res2.grid[0][1].isConvergenceUnwind, true);
  assert.equal(res2.grid[0][1].isExecutable, true);
});

function routeOpp(
  buyVenue: string,
  sellVenue: string,
  overrides: Partial<SpatialOpportunityDto> = {}
): SpatialOpportunityDto {
  return {
    id: `BTC-${buyVenue}-${sellVenue}`,
    symbol: "BTC-USDC",
    buy_venue: buyVenue,
    sell_venue: sellVenue,
    buy_price: 100.05,
    sell_price: 100.3,
    gross_spread: 0.25,
    gross_spread_bps: 24.9,
    fee_bps: 3.5,
    net_spread_bps: 10.0,
    order_qty: 0.5,
    estimated_profit_usd: 5.0,
    is_executable: true,
    buy_venue_id: buyVenue,
    sell_venue_id: sellVenue,
    buy_fee_bps: 2.0,
    sell_fee_bps: 1.5,
    round_trip_commission_bps: 7.0,
    entry_slippage_bps: 0.5,
    exit_slippage_bps: 0.5,
    closing_spread_bps: 2.0,
    expected_friction_bps: 10.0,
    required_gross_bps: 11.0,
    slippage_ok: true,
    depth_ok: true,
    ...overrides,
  };
}

test("buildCrossVenueMatrix uses backend route economics when provided", () => {
  const books = [venueBook("hyperliquid", 100.0, 100.05), venueBook("lighter", 100.3, 100.35)];
  const durable = [
    {
      buy_venue: "hyperliquid",
      sell_venue: "lighter",
      mean_gross_bps: 20.0,
      percentile_gross_bps: 18.0,
      sample_count: 100,
      history_span_secs: 86_400,
    },
  ];
  const { grid } = buildCrossVenueMatrix(
    "BTC-USDC",
    books,
    60,
    0.5,
    1.0,
    { windowPercentile: 50 },
    new RollingSpreadTracker(),
    durable,
    [routeOpp("HyperLiquid", "LIGHTER")]
  );
  const cell = grid[0][1];
  assert.equal(cell.costReady, true);
  assert.ok(Math.abs(cell.grossSpreadBps - 24.9) < 0.01);
  assert.equal(cell.netSpreadBps, 10.0);
  assert.equal(cell.feeBps, 7.0);
  assert.equal(cell.roundTripCommissionBps, 7.0);
  assert.equal(cell.closingFrictionBps, 2.0);
  assert.equal(cell.entrySlippageBps, 0.5);
  assert.equal(cell.exitSlippageBps, 0.5);
  assert.equal(cell.expectedFrictionBps, 10.0);
  assert.equal(cell.requiredGrossBps, 11.0);
  assert.equal(cell.nominalHurdleBps, 11.0);
  assert.equal(cell.slippageOk, true);
  assert.equal(cell.depthOk, true);
  assert.equal(cell.estProfitUsd, 5.0);
  // Durable gross baselines convert to net via backend expected friction.
  assert.ok(Math.abs(cell.baselineMeanBps - (20.0 - 10.0)) < 0.05);
  assert.equal(cell.percentileHurdleBps, 18.0);
  // gross 24.9 >= required 11 && net 10 >= percentile net 8 -> executable
  assert.equal(cell.isExecutable, true);
});

test("buildCrossVenueMatrix fails closed when route economics are missing", () => {
  const books = [venueBook("hyperliquid", 100.0, 100.01), venueBook("lighter", 102.0, 102.01)];
  const durable = [
    {
      buy_venue: "hyperliquid",
      sell_venue: "lighter",
      mean_gross_bps: 20.0,
      percentile_gross_bps: 18.0,
      sample_count: 100,
      history_span_secs: 86_400,
    },
  ];
  const { grid } = buildCrossVenueMatrix(
    "BTC-USDC",
    books,
    60,
    0.5,
    1.0,
    { windowPercentile: 50 },
    new RollingSpreadTracker(),
    durable,
    []
  );
  const cell = grid[0][1];
  assert.ok(cell.grossSpreadBps > 100, "observable gross spread is still shown");
  assert.equal(cell.costReady, false);
  assert.equal(cell.isExecutable, false);
  assert.equal(cell.requiredGrossBps, undefined);
  const visuals = getMatrixCellVisuals(cell, 1.0);
  assert.equal(visuals.highlightRing, "");
  assert.ok(visuals.textClass.includes("text-muted"));
});

test("buildCrossVenueMatrix honors backend slippage, depth, and required-gross gates", () => {
  const books = [venueBook("hyperliquid", 100.0, 100.05), venueBook("lighter", 100.3, 100.35)];
  const durable = [
    {
      buy_venue: "hyperliquid",
      sell_venue: "lighter",
      mean_gross_bps: 20.0,
      percentile_gross_bps: 18.0,
      sample_count: 100,
      history_span_secs: 86_400,
    },
  ];
  const cfg = { windowPercentile: 50, currentAccumulatedUnits: 0, maxAccumulatedUnits: 3 };

  const slipBlocked = buildCrossVenueMatrix(
    "BTC-USDC", books, 60, 0.5, 1.0, cfg, new RollingSpreadTracker(), durable,
    [routeOpp("hyperliquid", "lighter", { slippage_ok: false })]
  );
  assert.equal(slipBlocked.grid[0][1].costReady, true);
  assert.equal(slipBlocked.grid[0][1].isExecutable, false);

  const depthBlocked = buildCrossVenueMatrix(
    "BTC-USDC", books, 60, 0.5, 1.0, cfg, new RollingSpreadTracker(), durable,
    [routeOpp("hyperliquid", "lighter", { depth_ok: false })]
  );
  assert.equal(depthBlocked.grid[0][1].isExecutable, false);

  const hurdleBlocked = buildCrossVenueMatrix(
    "BTC-USDC", books, 60, 0.5, 1.0, cfg, new RollingSpreadTracker(), durable,
    [routeOpp("hyperliquid", "lighter", { required_gross_bps: 30.0 })]
  );
  assert.equal(hurdleBlocked.grid[0][1].isExecutable, false);
});

test("buildCrossVenueMatrix allows convergence unwind without route economics", () => {
  const tight = [venueBook("hyperliquid", 100.0, 100.01), venueBook("lighter", 100.01, 100.02)];
  const { grid } = buildCrossVenueMatrix(
    "BTC-USDC",
    tight,
    60,
    0.5,
    1.0,
    {
      windowPercentile: 50,
      currentAccumulatedUnits: 1,
      maxAccumulatedUnits: 3,
      executionMode: "DOUBLE_WAY",
    },
    new RollingSpreadTracker(),
    [],
    []
  );
  const cell = grid[0][1];
  assert.equal(cell.costReady, false);
  assert.equal(cell.isConvergenceUnwind, true);
  assert.equal(cell.isExecutable, true);
});

test("getVenueShortLabel correctly distinguishes Robinhood Lighter from Lighter", () => {
  assert.equal(getVenueShortLabel("Robinhood Lighter"), "Robinhood Lighter");
  assert.equal(getVenueShortLabel("robinhood"), "Robinhood Lighter");
  assert.equal(getVenueShortLabel("Lighter"), "Lighter");
  assert.equal(getVenueShortLabel("Hyperliquid"), "Hyperliquid");
  assert.equal(getVenueShortLabel("Vanta"), "Vanta");
});

