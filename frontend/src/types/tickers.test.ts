import { test } from "node:test";
import assert from "node:assert";
import {
  PERP_TICKERS,
  getEnabledConnectedVenueCodes,
  getTickerActiveVenues,
  getTickerAvgVolumeUsd,
  filterAvailableTickers,
  type PerpTicker,
  type VenueSettingsFilter,
} from "./tickers.ts";

test("getEnabledConnectedVenueCodes returns empty set when settings are undefined (no synthetic connectivity)", () => {
  const defaultCodes = getEnabledConnectedVenueCodes(undefined);
  assert.strictEqual(defaultCodes.size, 0);
});

test("getEnabledConnectedVenueCodes returns only enabled HL/LGT/RH in paper mode", () => {
  const settings: VenueSettingsFilter = {
    trading_mode: "paper",
    hyperliquid_enabled: true,
    lighter_enabled: true,
    lighter_api_key: "", // connectivity is never inferred from key strings
    vanta_enabled: true,
    vanta_orderly_key: "ed25519:validkey",
    robinhood_enabled: true,
    arcus_enabled: true,
    popdex_enabled: true,
    kraken_enabled: true,
    polymarket_enabled: true,
    kalshi_enabled: true,
  };
  const codes = getEnabledConnectedVenueCodes(settings, false);
  assert.deepStrictEqual(Array.from(codes).sort(), ["HL", "LGT", "RH"]);
});

test("getEnabledConnectedVenueCodes does not infer connectivity from credential strings", () => {
  const settings: VenueSettingsFilter = {
    trading_mode: "paper",
    hyperliquid_enabled: false,
    hyperliquid_private_key: "0xdeadbeef",
    lighter_enabled: false,
    lighter_api_key: "some-key",
    configured_venues: ["hyperliquid", "lighter"],
  };
  const codes = getEnabledConnectedVenueCodes(settings, false);
  assert.strictEqual(codes.size, 0);
});

test("getEnabledConnectedVenueCodes returns no venues in live mode", () => {
  const settings: VenueSettingsFilter = {
    trading_mode: "real",
    hyperliquid_enabled: true,
    lighter_enabled: true,
  };
  assert.strictEqual(getEnabledConnectedVenueCodes(settings, true).size, 0);
  assert.strictEqual(getEnabledConnectedVenueCodes(settings, false).size, 0);

  const paperSettings: VenueSettingsFilter = {
    trading_mode: "paper",
    hyperliquid_enabled: true,
    lighter_enabled: true,
  };
  assert.strictEqual(getEnabledConnectedVenueCodes(paperSettings, true).size, 0);
});

test("filterAvailableTickers retains only tickers with at least 2 active venues", () => {
  const sampleTickers: PerpTicker[] = [
    { symbol: "BTC-USDC", base: "BTC", quote: "USDC", name: "Bitcoin", venues: ["HL", "LGT", "VNT", "RH"], basePrice: 70000 },
    { symbol: "BNB-USDC", base: "BNB", quote: "USDC", name: "BNB", venues: ["HL", "LGT", "VNT"], basePrice: 600 },
    { symbol: "KAS-USDC", base: "KAS", quote: "USDC", name: "Kaspa", venues: ["HL", "LGT"], basePrice: 0.15 },
  ];

  // When all 4 DEXes enabled: all 3 tickers have >= 2 venues
  const allCodes = new Set(["HL", "LGT", "VNT", "RH"]);
  const allAvailable = filterAvailableTickers(sampleTickers, allCodes, 2);
  assert.strictEqual(allAvailable.length, 3);

  // When LGT and RH are disabled: only HL and VNT are active
  const hlVntOnly = new Set(["HL", "VNT"]);
  const filtered = filterAvailableTickers(sampleTickers, hlVntOnly, 2);
  // BTC has ["HL", "VNT"] (2 >= 2) -> available
  // BNB has ["HL", "VNT"] (2 >= 2) -> available
  // KAS has ["HL"] (1 < 2) -> NOT available!
  assert.strictEqual(filtered.length, 2);
  assert.ok(filtered.some((t) => t.symbol === "BTC-USDC"));
  assert.ok(filtered.some((t) => t.symbol === "BNB-USDC"));
  assert.ok(!filtered.some((t) => t.symbol === "KAS-USDC"));

  // When only 1 venue is active: 0 tickers available
  const singleVenue = new Set(["HL"]);
  const noneAvailable = filterAvailableTickers(sampleTickers, singleVenue, 2);
  assert.strictEqual(noneAvailable.length, 0);
});

test("filterAvailableTickers on full PERP_TICKERS universe returns 0 when unconfigured and 52 when venues configured", () => {
  const unconfiguredCodes = getEnabledConnectedVenueCodes(null);
  assert.strictEqual(filterAvailableTickers(PERP_TICKERS, unconfiguredCodes, 2).length, 0);

  const configuredCodes = new Set(["HL", "LGT", "VNT", "RH", "ARC", "POP"]);
  const available = filterAvailableTickers(PERP_TICKERS, configuredCodes, 2);
  assert.strictEqual(available.length, 52);
});

test("getTickerAvgVolumeUsd averages across reporting venues and normalizes venue asset names", () => {
  const venueVolumes: { venue_id: string; volumes: Record<string, number> }[] = [
    { venue_id: "hyperliquid", volumes: { BTC: 3_000_000_000, kPEPE: 12_000_000, SOL: 800_000_000 } },
    { venue_id: "lighter", volumes: { BTC: 1_000_000_000, PEPE: 8_000_000 } },
  ];

  // Mean of reporting venues
  assert.strictEqual(getTickerAvgVolumeUsd("BTC", venueVolumes), 2_000_000_000);
  // k-prefixed HL asset matches plain base; averaged with Lighter's plain name
  assert.strictEqual(getTickerAvgVolumeUsd("PEPE", venueVolumes), 10_000_000);
  // Case-insensitive base lookup
  assert.strictEqual(getTickerAvgVolumeUsd("sol", venueVolumes), 800_000_000);
  // No venue reports the asset -> undefined (never a fabricated zero)
  assert.strictEqual(getTickerAvgVolumeUsd("TAO", venueVolumes), undefined);
  // Zero/invalid venue entries are ignored rather than dragging the mean down
  const withZero: { venue_id: string; volumes: Record<string, number> }[] = [
    { venue_id: "hyperliquid", volumes: { BTC: 0, ETH: 5_000_000 } },
  ];
  assert.strictEqual(getTickerAvgVolumeUsd("BTC", withZero), undefined);
  assert.strictEqual(getTickerAvgVolumeUsd("ETH", withZero), 5_000_000);
});

test("getTickerActiveVenues returns only active venue subset", () => {
  const ticker: PerpTicker = {
    symbol: "BTC-USDC",
    base: "BTC",
    quote: "USDC",
    name: "Bitcoin",
    venues: ["HL", "LGT", "VNT", "RH"],
    basePrice: 70000,
  };
  const activeCodes = new Set(["HL", "RH"]);
  const activeVenues = getTickerActiveVenues(ticker, activeCodes);
  assert.deepStrictEqual(activeVenues, ["HL", "RH"]);
});
