import { test } from "node:test";
import assert from "node:assert";
import {
  PERP_TICKERS,
  getAdmittedVenueCodes,
  getEnabledConnectedVenueCodes,
  getTickerActiveVenues,
  getTickerAvgVolumeUsd,
  getTickerMinVolumeUsd,
  getVenueAssetVolumeUsd,
  getTickerBenchmarkPrice,
  autoFilterBlockReason,
  applyTickerVisibility,
  filterAvailableTickers,
  applyMarketUniverse,
  tickerUnavailabilityReason,
  type PerpTicker,
  type VenueSettingsFilter,
} from "./tickers.ts";

test("getEnabledConnectedVenueCodes returns empty set when settings are undefined (no synthetic connectivity)", () => {
  const defaultCodes = getEnabledConnectedVenueCodes(undefined);
  assert.strictEqual(defaultCodes.size, 0);
});

test("getEnabledConnectedVenueCodes returns enabled supported venues in paper mode and ignores unsupported", () => {
  const settings: VenueSettingsFilter = {
    trading_mode: "paper",
    hyperliquid_enabled: true,
    lighter_enabled: true,
    lighter_api_key: "", // connectivity is never inferred from key strings
    vanta_enabled: true,
    vanta_orderly_key: "ed25519:validkey",
    robinhood_enabled: true,
    arcus_enabled: true, // scaffold only, unsupported
    aster_enabled: true,
    popdex_enabled: true,
    kraken_enabled: true, // admitted to paper read-only
    polymarket_enabled: true, // unsupported
    kalshi_enabled: true, // unsupported
  };
  const codes = getEnabledConnectedVenueCodes(settings, false);
  assert.deepStrictEqual(Array.from(codes).sort(), ["AST", "HL", "KRK", "LGT", "POP", "RH", "VNT"]);
});

test("getEnabledConnectedVenueCodes returns newly added supported venues (BNB, OKX, CB, DCB)", () => {
  const settings: VenueSettingsFilter = {
    trading_mode: "paper",
    binance_enabled: true,
    okx_enabled: true,
    coinbase_enabled: true,
    decibel_enabled: true,
  };
  const codes = getEnabledConnectedVenueCodes(settings, false);
  assert.deepStrictEqual(Array.from(codes).sort(), ["BNB", "CB", "DCB", "OKX"]);
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

test("PERP_TICKERS catalog has unique canonical symbols", () => {
  const seen = new Set<string>();
  for (const t of PERP_TICKERS) {
    const key = t.symbol.toUpperCase();
    assert.ok(!seen.has(key), `duplicate catalog symbol: ${t.symbol}`);
    seen.add(key);
  }
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

test("getVenueAssetVolumeUsd resolves canonical BASE-QUOTE venue keys", () => {
  // Adapters key volume maps by canonical symbol; the matcher must strip
  // quote suffixes so every venue's spelling resolves to the plain base.
  const volumes: Record<string, number> = {
    "BTC-USDT": 3_000_000_000, // binance/aster/okx/popdex style
    "ETH-USD": 700_000_000, // kraken style
    "SOL-USDC": 500_000_000, // hyperliquid/lighter/vanta style
    "1000PEPE-USDT": 40_000_000,
    "BTC-USD1": 1,
  };
  assert.strictEqual(getVenueAssetVolumeUsd(volumes, "BTC"), 3_000_000_000);
  assert.strictEqual(getVenueAssetVolumeUsd(volumes, "ETH"), 700_000_000);
  assert.strictEqual(getVenueAssetVolumeUsd(volumes, "sol"), 500_000_000);
  assert.strictEqual(getVenueAssetVolumeUsd(volumes, "PEPE"), 40_000_000);
  // A bare base never matches an unrelated quote row.
  assert.strictEqual(getVenueAssetVolumeUsd(volumes, "TAO"), undefined);
  // Bare-base keys (legacy/venue-native) still resolve.
  assert.strictEqual(getVenueAssetVolumeUsd({ BTC: 5 }, "BTC"), 5);
  assert.strictEqual(getVenueAssetVolumeUsd({ kPEPE: 7 }, "PEPE"), 7);
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

test("getAdmittedVenueCodes counts only READ_ONLY venues — UNSUPPORTED and OFFLINE are not admitted", () => {
  const status = {
    active_venues: [
      { id: "hyperliquid", status: "OFFLINE" },
      { id: "lighter", status: "READ_ONLY" },
      { id: "robinhood", status: "READ_ONLY" },
      { id: "vanta", status: "UNSUPPORTED" },
      { id: "binance", status: "UNSUPPORTED" },
    ],
  };
  const codes = getAdmittedVenueCodes(status);
  assert.deepStrictEqual(Array.from(codes).sort(), ["LGT", "RH"]);
  // Two admitted venues unlock the RH-eligible ticker universe.
  assert.ok(filterAvailableTickers(PERP_TICKERS, codes, 2).length > 0);
});

test("getAdmittedVenueCodes admits nothing when the backend marks every venue UNSUPPORTED", () => {
  const status = {
    active_venues: [
      { id: "lighter", status: "UNSUPPORTED" },
      { id: "robinhood", status: "UNSUPPORTED" },
    ],
  };
  assert.strictEqual(getAdmittedVenueCodes(status).size, 0);
  assert.strictEqual(getAdmittedVenueCodes(null).size, 0);
  assert.strictEqual(getAdmittedVenueCodes(undefined).size, 0);
});

test("tickerUnavailabilityReason names the real cause of an empty universe", () => {
  const mk = (venues: { id: string; status: string }[], live = false) => ({
    is_live_mode: live,
    active_venues: venues,
  });

  // Engine unreachable / no status at all.
  assert.match(tickerUnavailabilityReason(null, new Set()), /Engine status unavailable/);
  assert.match(tickerUnavailabilityReason(undefined, new Set()), /Engine status unavailable/);

  // Live mode: read-only lanes are not admitted.
  assert.match(
    tickerUnavailabilityReason(mk([{ id: "lighter", status: "UNSUPPORTED" }], true), new Set()),
    /Live Trading mode/
  );

  // Venues enabled but none admitted (UNSUPPORTED ≠ stream-capable).
  assert.match(
    tickerUnavailabilityReason(
      mk([
        { id: "binance", status: "UNSUPPORTED" },
        { id: "decibel", status: "UNSUPPORTED" },
      ]),
      new Set()
    ),
    /not providing an overlapping market universe/
  );

  // Nothing enabled at all — original guidance.
  assert.match(
    tickerUnavailabilityReason(
      mk([{ id: "lighter", status: "OFFLINE" }]),
      new Set()
    ),
    /Enable markets in System Settings/
  );

  // Exactly one admitted venue — cross-venue spreads need two.
  assert.match(
    tickerUnavailabilityReason(
      mk([
        { id: "lighter", status: "READ_ONLY" },
        { id: "robinhood", status: "OFFLINE" },
      ]),
      new Set(["LGT"])
    ),
    /Only LGT is streaming.*at least 2/
  );

  // Two+ admitted, universe has venue rows but no overlapping catalog market.
  assert.match(
    tickerUnavailabilityReason(
      mk([
        { id: "lighter", status: "READ_ONLY" },
        { id: "robinhood", status: "READ_ONLY" },
      ]),
      new Set(["LGT", "RH"]),
      true,
      false
    ),
    /do not currently report any overlapping catalog markets/
  );

  // Two+ admitted but the market-universe response has not arrived yet.
  assert.match(
    tickerUnavailabilityReason(
      mk([
        { id: "lighter", status: "READ_ONLY" },
        { id: "robinhood", status: "READ_ONLY" },
      ]),
      new Set(["LGT", "RH"]),
      false,
      false
    ),
    /not providing an overlapping market universe yet/
  );

  // Two+ admitted, intersection non-empty but every row exempted — hidden.
  assert.match(
    tickerUnavailabilityReason(
      mk([
        { id: "lighter", status: "READ_ONLY" },
        { id: "robinhood", status: "READ_ONLY" },
      ]),
      new Set(["LGT", "RH"]),
      true,
      true
    ),
    /All tickers are hidden/
  );
});

test("applyMarketUniverse replaces stale catalog venue claims with actual reporting venues", () => {
  const catalog: PerpTicker[] = [
    { symbol: "BTC-USDC", base: "BTC", quote: "USDC", name: "Bitcoin", venues: ["HL", "LGT", "VNT", "RH"], basePrice: 70000 },
    { symbol: "KAS-USDC", base: "KAS", quote: "USDC", name: "Kaspa", venues: ["HL", "LGT"], basePrice: 0.15 },
  ];
  const universe = [
    { venue_id: "hyperliquid", symbols: ["BTC-USDC", "KAS-USDC"] },
    { venue_id: "vanta", symbols: ["BTC-USDC"] },
  ];
  const out = applyMarketUniverse(catalog, universe);
  assert.strictEqual(out.length, 1);
  assert.strictEqual(out[0].symbol, "BTC-USDC");
  assert.deepStrictEqual(out[0].venues.sort(), ["HL", "VNT"]);
});

test("applyMarketUniverse drops a ticker when only one venue really reports it", () => {
  const catalog: PerpTicker[] = [
    { symbol: "BTC-USDC", base: "BTC", quote: "USDC", name: "Bitcoin", venues: ["HL", "LGT"], basePrice: 70000 },
  ];
  const out = applyMarketUniverse(catalog, [
    { venue_id: "hyperliquid", symbols: ["BTC-USDC"] },
  ]);
  assert.strictEqual(out.length, 0);
});

test("applyMarketUniverse ignores unknown venue ids and single-venue non-catalog symbols", () => {
  const catalog: PerpTicker[] = [
    { symbol: "BTC-USDC", base: "BTC", quote: "USDC", name: "Bitcoin", venues: ["HL", "LGT"], basePrice: 70000 },
  ];
  const out = applyMarketUniverse(catalog, [
    { venue_id: "mysterydex", symbols: ["BTC-USDC", "FAKE-USDC"] },
    { venue_id: "lighter", symbols: ["BTC-USDC", "NOTREAL-USDC"] },
  ]);
  assert.strictEqual(out.length, 0);
});

test("applyMarketUniverse discovers catalog-absent symbols and uses median live price", () => {
  const out = applyMarketUniverse([], [
    { venue_id: "hyperliquid", symbols: ["ZEC-USDC"], base_prices: { "ZEC-USDC": 1440 } },
    { venue_id: "lighter", symbols: ["ZEC-USDC"], base_prices: { "ZEC-USDC": 1444 } },
    { venue_id: "robinhood", symbols: ["ZEC-USDC"], base_prices: { "ZEC-USDC": 1443 } },
  ]);

  assert.strictEqual(out.length, 1);
  assert.strictEqual(out[0].symbol, "ZEC-USDC");
  assert.strictEqual(out[0].base, "ZEC");
  assert.strictEqual(out[0].quote, "USDC");
  assert.strictEqual(out[0].name, "ZEC");
  assert.strictEqual(out[0].basePrice, 1443);
  assert.deepStrictEqual(out[0].venues, ["HL", "LGT", "RH"]);
});

test("applyMarketUniverse keeps exact stablecoin quotes distinct", () => {
  const out = applyMarketUniverse([], [
    { venue_id: "binance", symbols: ["MSTR-USDT"], base_prices: { "MSTR-USDT": 151 } },
    { venue_id: "okx", symbols: ["MSTR-USDT"], base_prices: { "MSTR-USDT": 151.5 } },
    { venue_id: "aster", symbols: ["MSTR-USDT"], base_prices: { "MSTR-USDT": 151.2 } },
    { venue_id: "lighter", symbols: ["MSTR-USDC"], base_prices: { "MSTR-USDC": 151.4 } },
  ]);

  assert.deepStrictEqual(out.map((ticker) => ticker.symbol), ["MSTR-USDT"]);
  assert.deepStrictEqual(out[0].venues, ["AST", "BNB", "OKX"]);
});

test("applyMarketUniverse admits a ticker listed by two real venues", () => {
  const catalog: PerpTicker[] = [
    { symbol: "ETH-USDC", base: "ETH", quote: "USDC", name: "Ethereum", venues: ["HL", "LGT"], basePrice: 3000 },
  ];
  const out = applyMarketUniverse(catalog, [
    { venue_id: "hyperliquid", symbols: ["eth-usdc"] },
    { venue_id: "robinhood", symbols: ["ETH-USDC"] },
  ]);
  assert.strictEqual(out.length, 1);
  assert.deepStrictEqual(out[0].venues.sort(), ["HL", "RH"]);
});

test("getTickerMinVolumeUsd takes the MIN across in-scope venues", () => {
  const vols = [
    { venue_id: "hyperliquid", volumes: { BTC: 1_000_000_000 } },
    { venue_id: "lighter", volumes: { BTC: 50_000 } },
  ];
  assert.strictEqual(getTickerMinVolumeUsd("BTC", ["hyperliquid", "lighter"], vols), 50_000);
  assert.strictEqual(getTickerMinVolumeUsd("BTC", ["hyperliquid"], vols), 1_000_000_000);
});

test("getTickerMinVolumeUsd evaluates reporting venues and excludes when completely unreported", () => {
  const vols = [{ venue_id: "hyperliquid", volumes: { BTC: 1_000_000 } }];
  // When hyperliquid reports but vanta has no volume feed, reporting venue volume is returned
  assert.strictEqual(getTickerMinVolumeUsd("BTC", ["hyperliquid", "vanta"], vols), 1_000_000);
  assert.strictEqual(getTickerMinVolumeUsd("BTC", ["hyperliquid", "lighter"], vols), 1_000_000);
  // When no venue reports volume for this asset, it returns undefined
  assert.strictEqual(
    getTickerMinVolumeUsd("BTC", ["hyperliquid"], [{ venue_id: "hyperliquid", volumes: { ETH: 5 } }]),
    undefined
  );
  assert.strictEqual(
    getTickerMinVolumeUsd("UNKNOWN", ["hyperliquid", "vanta"], vols),
    undefined
  );
});

test("getTickerMinVolumeUsd honors K/1000 normalization like the mean helper", () => {
  const vols: { venue_id: string; volumes: Record<string, number> }[] = [
    { venue_id: "hyperliquid", volumes: { kPEPE: 12_000_000 } },
    { venue_id: "lighter", volumes: { PEPE: 8_000_000 } },
  ];
  assert.strictEqual(getTickerMinVolumeUsd("PEPE", ["hyperliquid", "lighter"], vols), 8_000_000);
});

test("autoFilterBlockReason enforces MIN volume and MIN mid with reporting venues and benchmark fallback", () => {
  const vols = [
    { venue_id: "hyperliquid", volumes: { BTC: 1_000_000 } },
    { venue_id: "lighter", volumes: { BTC: 900_000 } },
  ];
  const scope = ["hyperliquid", "lighter"];
  assert.strictEqual(
    autoFilterBlockReason("BTC", scope, vols, { hyperliquid: 100, lighter: 101 }, { minVolumeUsd: 500_000, minPriceUsd: 50 }),
    null
  );
  assert.ok(
    (autoFilterBlockReason("BTC", scope, vols, { hyperliquid: 100, lighter: 101 }, { minVolumeUsd: 950_000, minPriceUsd: null }) ?? "").includes("volume"),
    "MIN aggregation must fail on the smaller venue"
  );
  assert.ok(
    (autoFilterBlockReason("BTC", scope, vols, { hyperliquid: 100, lighter: 40 }, { minVolumeUsd: null, minPriceUsd: 50 }) ?? "").includes("price"),
    "price uses the minimum mid"
  );
  // Reporting venue volume allows liquid tickers to pass even when one venue reports no volume
  assert.strictEqual(
    autoFilterBlockReason("BTC", ["hyperliquid", "vanta"], vols, { hyperliquid: 100, vanta: 100 }, { minVolumeUsd: 500_000, minPriceUsd: null }),
    null,
    "reporting venue volume >= floor passes even if another venue has no volume feed"
  );
  // But if reported volume is below floor, it fails
  assert.ok(
    (autoFilterBlockReason("BTC", ["hyperliquid", "vanta"], vols, { hyperliquid: 100, vanta: 100 }, { minVolumeUsd: 2_000_000, minPriceUsd: null }) ?? "").includes("volume"),
    "reported volume below floor blocks"
  );
  // Benchmark fallback price is used when live mid is not in cache
  assert.strictEqual(getTickerBenchmarkPrice("BTC"), 78550.0);
  assert.strictEqual(
    autoFilterBlockReason("BTC", scope, vols, {}, { minVolumeUsd: null, minPriceUsd: 50 }),
    null,
    "benchmark price allows BTC ($78k) to pass minPrice 50"
  );
  // Sub-floor benchmark price blocks penny tokens
  assert.ok(
    (autoFilterBlockReason("PEPE", scope, vols, {}, { minVolumeUsd: null, minPriceUsd: 0.5 }) ?? "").includes("price"),
    "PEPE benchmark price < 0.5 blocks"
  );
  assert.strictEqual(
    autoFilterBlockReason("BTC", scope, vols, {}, { minVolumeUsd: null, minPriceUsd: null }),
    null,
    "disabled filters always pass"
  );
  assert.strictEqual(
    autoFilterBlockReason("BTC", scope, vols, {}, { minVolumeUsd: 0, minPriceUsd: -1 }),
    null,
    "zero/negative thresholds are off"
  );
});

test("autoFilterBlockReason ignores the inert prediction event-volume field", () => {
  const vols = [{ venue_id: "hyperliquid", volumes: { BTC: 1 } }];
  assert.strictEqual(
    autoFilterBlockReason("BTC", ["hyperliquid"], vols, { hyperliquid: 1 }, { minVolumeUsd: null, minPriceUsd: null }),
    null
  );
});

test("applyTickerVisibility applies manual AFTER auto and manages them separately", () => {
  const universe: PerpTicker[] = [
    { symbol: "BTC-USDC", base: "BTC", quote: "USDC", name: "Bitcoin", venues: ["HL", "LGT"], basePrice: 70000 },
    { symbol: "ETH-USDC", base: "ETH", quote: "USDC", name: "Ethereum", venues: ["HL", "LGT"], basePrice: 3000 },
    { symbol: "SOL-USDC", base: "SOL", quote: "USDC", name: "Solana", venues: ["HL", "LGT"], basePrice: 100 },
  ];
  const auto = new Set(["ETH"]);
  const manual = new Set(["SOL"]);
  const out = applyTickerVisibility(universe, auto, manual);
  assert.deepStrictEqual(out.map((t) => t.base), ["BTC"]);
  assert.deepStrictEqual(applyTickerVisibility(universe, null, manual).map((t) => t.base), ["BTC", "ETH"]);
  assert.deepStrictEqual(applyTickerVisibility(universe, auto, new Set()).map((t) => t.base), ["BTC", "SOL"]);
  assert.deepStrictEqual(applyTickerVisibility(universe, new Set(["BTC", "ETH", "SOL"]), new Set()).length, 0);
});

test("manual hide UI lives in the Simple Spread configurator, not EngineSettings", async () => {
  const { readFile } = await import("node:fs/promises");
  const modal = await readFile(new URL("../components/TickerConfigModal.tsx", import.meta.url), "utf8");
  const settings = await readFile(new URL("../components/EngineSettings.tsx", import.meta.url), "utf8");
  for (const token of ["getExemptedTickers", "revealAllTickers", "Reveal All", "Hidden Tickers"]) {
    assert.ok(modal.includes(token), `configurator must own ${token}`);
    assert.ok(!settings.includes(token), `EngineSettings must not reference ${token}`);
  }
});
