import { test } from "node:test";
import assert from "node:assert";
import {
  PERP_TICKERS,
  getEnabledConnectedVenueCodes,
  getTickerActiveVenues,
  filterAvailableTickers,
  type PerpTicker,
  type VenueSettingsFilter,
} from "./tickers.ts";

test("getEnabledConnectedVenueCodes returns empty set when settings are undefined (no synthetic connectivity)", () => {
  const defaultCodes = getEnabledConnectedVenueCodes(undefined);
  assert.strictEqual(defaultCodes.size, 0);
});

test("getEnabledConnectedVenueCodes requires API credentials in paper mode (no synthetic data)", () => {
  const settings: VenueSettingsFilter = {
    trading_mode: "paper",
    hyperliquid_enabled: true,
    hyperliquid_wallet: "0x1234567890abcdef",
    lighter_enabled: true,
    lighter_api_key: "", // missing credentials
    vanta_enabled: true,
    vanta_orderly_key: "ed25519:validkey",
    robinhood_enabled: false,
    arcus_enabled: false,
    popdex_enabled: false,
    kraken_enabled: false,
  };
  const codes = getEnabledConnectedVenueCodes(settings, false);
  assert.deepStrictEqual(Array.from(codes).sort(), ["HL", "VNT"]);
});

test("getEnabledConnectedVenueCodes requires credentials in live mode", () => {
  const settings: VenueSettingsFilter = {
    trading_mode: "real",
    hyperliquid_enabled: true,
    hyperliquid_wallet: "0x1234567890abcdef",
    lighter_enabled: true,
    lighter_api_key: "", // missing credentials
    vanta_enabled: true,
    vanta_orderly_key: "ed25519:validkey",
    robinhood_enabled: true,
    robinhood_wallet: "", // missing credentials
  };
  const codes = getEnabledConnectedVenueCodes(settings, true);
  assert.ok(codes.has("HL"));
  assert.ok(!codes.has("LGT"));
  assert.ok(codes.has("VNT"));
  assert.ok(!codes.has("RH"));
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
