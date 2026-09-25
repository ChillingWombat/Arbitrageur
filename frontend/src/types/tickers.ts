import type { VenueMarketUniverseDto } from "./index";

export interface PerpTicker {
  symbol: string;         // e.g. "BTC-USDC"
  base: string;           // e.g. "BTC"
  quote: string;          // e.g. "USDC"
  name: string;           // e.g. "Bitcoin"
  venues: string[];       // e.g. ["HL", "LGT", "VNT", "RH"] - all have length >= 2
  basePrice: number;      // Reference benchmark price
}

export type TickerSortOption = "alpha" | "trades" | "pnl" | "liquidity";

export interface TickerWithStats extends PerpTicker {
  tradeCount: number;
  netProfit: number;
  netProfitBps?: number;
  hasIssue?: boolean;
  issueDescription?: string;
  lastTradeTime?: string;
  /** Mean 24h notional volume (USD) across venues that report it. */
  avgVolumeUsd?: number;
}

export function formatCurrencyPrice(price: number): string {
  if (price === 0) return "$0.00";
  const abs = Math.abs(price);
  if (abs >= 1000) {
    return `$${price.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
  }
  if (abs >= 1) {
    return `$${price.toFixed(2)}`;
  }
  if (abs >= 0.01) {
    return `$${price.toFixed(4)}`;
  }
  if (abs >= 0.0001) {
    return `$${price.toFixed(6)}`;
  }
  return `$${price.toFixed(8)}`;
}

export function formatSpreadPrice(spread: number): string {
  const abs = Math.abs(spread);
  if (abs >= 1) return `$${spread.toFixed(2)}`;
  if (abs >= 0.01) return `$${spread.toFixed(4)}`;
  if (abs >= 0.0001) return `$${spread.toFixed(6)}`;
  return `$${spread.toFixed(8)}`;
}

export function formatCurrencyPnl(pnl: number | null | undefined): string {
  // Unknown stays unknown — a null/NaN PnL must never read as "$0.00".
  if (pnl === null || pnl === undefined || !Number.isFinite(pnl)) return "—";
  const rounded = Math.round(pnl * 100) / 100;
  if (rounded > 0) return `+$${rounded.toFixed(2)}`;
  if (rounded < 0) return `-$${Math.abs(rounded).toFixed(2)}`;
  return "$0.00";
}

export function getDefaultOrderQty(basePrice: number): number {
  if (!basePrice || basePrice <= 0) return 1.0;
  const raw = 25000 / basePrice;
  if (raw >= 100000) return Math.round(raw / 10000) * 10000;
  if (raw >= 1000) return Math.round(raw / 100) * 100;
  if (raw >= 10) return Math.round(raw);
  if (raw >= 1) return Math.round(raw * 10) / 10;
  return Math.round(raw * 100) / 100;
}

// Venue Identifiers & Short Code Mappings
/** Canonical strategy display names — keyed by the persisted strategy_id
 *  values the backend allows (ALLOWED_STRATEGY_IDS), plus "triangle" for the
 *  intra-venue path that never persists signal_executions. Keep these aligned
 *  with the Navigation tab labels; they are the single vocabulary used across
 *  panels, monitors, and the positions scope toggle. */
export const STRATEGY_DISPLAY_NAMES: Record<string, string> = {
  futures: "Simple Spread",
  radar: "Spread Radar",
  funding: "Funding Rate",
  spot: "Spatial",
  prediction: "Prediction Market",
  stat_arb: "Statistical",
  triangle: "Triangle",
  options_parity: "Options Parity",
  external: "External (manual)",
};

/** Display name for a persisted strategy id; falls back to the raw id. */
export const formatStrategyName = (id: string): string =>
  STRATEGY_DISPLAY_NAMES[id] ?? id;

export const VENUE_ID_TO_CODE: Record<string, string> = {
  hyperliquid: "HL",
  lighter: "LGT",
  vanta: "VNT",
  robinhood: "RH",
  arcus: "ARC",
  aster: "AST",
  popdex: "POP",
  kraken: "KRK",
  binance: "BNB",
  okx: "OKX",
  coinbase: "CB",
  alpaca: "ALP",
  decibel: "DCB",
  deribit: "DBT",
};

export const VENUE_CODE_TO_ID: Record<string, string> = {
  HL: "hyperliquid",
  LGT: "lighter",
  VNT: "vanta",
  RH: "robinhood",
  ARC: "arcus",
  AST: "aster",
  POP: "popdex",
  KRK: "kraken",
  BNB: "binance",
  OKX: "okx",
  CB: "coinbase",
  ALP: "alpaca",
  DCB: "decibel",
  DBT: "deribit",
};

/**
 * Minimal settings contract needed to determine venue enablement and connectivity.
 */
export interface VenueSettingsFilter {
  trading_mode?: string;
  hyperliquid_enabled?: boolean;
  hyperliquid_wallet?: string;
  hyperliquid_private_key?: string;
  lighter_enabled?: boolean;
  lighter_api_key?: string;
  lighter_account_index?: string;
  vanta_enabled?: boolean;
  vanta_orderly_key?: string;
  vanta_account_id?: string;
  robinhood_enabled?: boolean;
  robinhood_wallet?: string;
  robinhood_private_key?: string;
  arcus_enabled?: boolean;
  arcus_api_key?: string;
  aster_enabled?: boolean;
  popdex_enabled?: boolean;
  kraken_enabled?: boolean;
  kraken_spot_enabled?: boolean;
  kraken_futures_enabled?: boolean;
  kraken_spot_api_key?: string;
  kraken_futures_api_key?: string;
  binance_enabled?: boolean;
  okx_enabled?: boolean;
  coinbase_enabled?: boolean;
  alpaca_enabled?: boolean;
  decibel_enabled?: boolean;
  deribit_enabled?: boolean;
  polymarket_enabled?: boolean;
  polymarket_api_key?: string;
  polymarket_secret?: string;
  kalshi_enabled?: boolean;
  kalshi_api_key?: string;
  kalshi_email?: string;
  configured_venues?: string[];
}

/**
 * Venue codes with public testnet/demo endpoints — the only venues the
 * engine admits in the Testnet environment. Mirrors the `tn_*` adapter set
 * in venues_for_env; venues without a demo endpoint (Robinhood, Aster,
 * Coinbase, long-tail perps, EVM AMMs) are live-only.
 */
const TESTNET_CAPABLE_CODES = new Set(["HL", "LGT", "VNT", "POP", "BNB", "OKX", "KRK", "ALP", "DCB", "DBT"]);

/**
 * Venue IDs with public testnet/demo endpoints — mirrors the backend `tn_*`
 * adapter set in venues_for_env. Used to badge account cards when the
 * active environment is Testnet: an account created for a venue without a
 * demo endpoint can never be admitted while the engine runs testnet.
 */
export const TESTNET_CAPABLE_VENUE_IDS = new Set([
  "hyperliquid",
  "lighter",
  "vanta",
  "popdex",
  "binance",
  "okx",
  "kraken",
  "kraken_futures",
  "alpaca",
  "decibel",
  "deribit",
]);

export function venueHasTestnet(venueId: string): boolean {
  return TESTNET_CAPABLE_VENUE_IDS.has(venueId.trim().toLowerCase());
}

/**
 * Determines the set of natively connected venue codes from sanitized engine settings.
 * Public read-only feeds require the venue's enable toggle, not secrets; connectivity
 * is never inferred from credential-looking strings. In Live mode this returns an
 * empty set — flag state never proves connectivity. In Testnet mode only
 * testnet-capable venues count.
 *
 * Prefer getAdmittedVenueCodes(systemStatus) — the backend's own admission report —
 * over this flag-based approximation; it cannot drift from what get_venues admits.
 */
export function getEnabledConnectedVenueCodes(
  settings?: VenueSettingsFilter | null,
  isLiveMode?: boolean
): Set<string> {
  const active = new Set<string>();
  // Fail closed on any mode other than an explicit "testnet" — this helper
  // only speaks for testnet admission; live, legacy, or unrecognized values
  // return empty (the backend's admission report is authoritative anyway).
  if (
    !settings ||
    isLiveMode === true ||
    settings.trading_mode !== "testnet"
  ) {
    return active;
  }

  const admit = (code: string, enabled?: boolean) => {
    if (enabled === true && TESTNET_CAPABLE_CODES.has(code)) {
      active.add(code);
    }
  };

  admit("HL", settings.hyperliquid_enabled);
  admit("LGT", settings.lighter_enabled);
  admit("VNT", settings.vanta_enabled);
  admit("POP", settings.popdex_enabled);
  admit("BNB", settings.binance_enabled);
  admit("OKX", settings.okx_enabled);
  admit(
    "KRK",
    settings.kraken_enabled === true ||
      settings.kraken_futures_enabled === true ||
      settings.kraken_spot_enabled === true
  );
  admit("ALP", settings.alpaca_enabled);
  admit("DCB", settings.decibel_enabled);
  admit("DBT", settings.deribit_enabled);
  return active;
}

/** Minimal shape of the venue status entries this helper reads. */
export interface VenueStatusLike {
  id: string;
  status: string;
}

/**
 * Venue codes currently admitted by the engine, derived from the backend's
 * own system status — the same flag-OR-enabled-account rule `get_venues`
 * applies — instead of stale frontend flag state. Only READ_ONLY venues
 * count: OFFLINE means disabled, UNSUPPORTED means enabled without a real
 * market-data adapter.
 */
export function getAdmittedVenueCodes(
  status?: { active_venues?: VenueStatusLike[] } | null
): Set<string> {
  const active = new Set<string>();
  for (const v of status?.active_venues ?? []) {
    if (v.status === "READ_ONLY") {
      const code = VENUE_ID_TO_CODE[v.id.toLowerCase()];
      if (code) active.add(code);
    }
  }
  return active;
}

/**
 * Returns the set of active venue codes that support spot trading (e.g. Kraken Spot, Arcus, Robinhood).
 */
export function getEnabledSpotVenueCodes(
  settings?: VenueSettingsFilter | null,
  isLiveMode?: boolean
): Set<string> {
  const allActive = getEnabledConnectedVenueCodes(settings, isLiveMode);
  const spotCodes = new Set<string>();
  if (allActive.has("KRK") && settings?.kraken_spot_enabled && settings?.kraken_spot_api_key?.trim()) {
    spotCodes.add("KRK");
  }
  if (allActive.has("ARC")) {
    spotCodes.add("ARC");
  }
  if (allActive.has("RH")) {
    spotCodes.add("RH");
  }
  return spotCodes;
}

/**
 * Returns the subset of venue codes in ticker.venues that are currently enabled & connected.
 */
export function getTickerActiveVenues(
  ticker: PerpTicker,
  activeVenueCodes: Set<string>
): string[] {
  return ticker.venues.filter((v) => activeVenueCodes.has(v));
}

/**
 * Honest explanation for an empty ticker universe, used by the explorer
 * empty state. Distinguishes the real causes instead of always blaming
 * venue enablement: unreachable engine, live mode (no read-only lanes),
 * enabled-but-unadmitted venues, single-venue admission, no overlapping
 * markets across the venues' actual reported universes, or all tickers
 * hidden via exemptions.
 *
 * `hasMarketUniverseRows` is true once at least one venue row has arrived
 * from get_market_universe; `hasMarketUniverseIntersection` is the
 * pre-exemption `applyMarketUniverse` result being non-empty. Both are
 * needed because admitted-venue count alone cannot tell "no overlap" from
 * "all hidden".
 */
export function tickerUnavailabilityReason(
  status: { is_live_mode?: boolean; active_venues?: VenueStatusLike[] } | null | undefined,
  activeVenueCodes: Set<string>,
  hasMarketUniverseRows = false,
  hasMarketUniverseIntersection = false
): string {
  if (!status) {
    return "Engine status unavailable — start the desktop app so venue lanes can report.";
  }
  if (status.is_live_mode) {
    return "Live Trading mode has no read-only market lanes — switch to Testnet mode to stream venue quotes.";
  }
  if (activeVenueCodes.size === 0) {
    const enabled = (status.active_venues ?? []).filter(
      (v) => v.status !== "OFFLINE"
    );
    if (enabled.length > 0) {
      return "Enabled venues are not providing an overlapping market universe yet.";
    }
    return "Enable markets in System Settings to stream live venue quotes.";
  }
  if (activeVenueCodes.size === 1) {
    const code = [...activeVenueCodes][0];
    return `Only ${code} is streaming — a cross-venue spread needs quotes from at least 2 admitted venues.`;
  }
  if (hasMarketUniverseIntersection) {
    return "All tickers are hidden — reveal them in System Settings → Hidden Tickers.";
  }
  if (hasMarketUniverseRows) {
    return "Enabled venues do not currently report any overlapping catalog markets.";
  }
  return "Enabled venues are not providing an overlapping market universe yet.";
}

function canonicalMarketParts(
  raw: string
): { symbol: string; base: string; quote: string } | null {
  const symbol = raw.trim().toUpperCase();
  const parts = symbol.split("-");
  if (parts.length !== 2 || !parts[0] || !parts[1]) return null;
  return { symbol, base: parts[0], quote: parts[1] };
}

function medianPositive(prices: number[]): number | undefined {
  const valid = prices
    .filter((price) => Number.isFinite(price) && price > 0)
    .sort((a, b) => a - b);
  if (valid.length === 0) return undefined;
  const middle = Math.floor(valid.length / 2);
  return valid.length % 2 === 1
    ? valid[middle]
    : (valid[middle - 1] + valid[middle]) / 2;
}

/**
 * Builds the live ticker universe from the venues' actual reported markets.
 * The static catalog is metadata/price fallback only; it is no longer an
 * allowlist. Any active perpetual listed by at least 2 admitted venues is
 * admitted, including symbols absent from the catalog.
 *
 * `mergeUsdQuotes` mirrors the engine's `usd_quote_mode` (backend
 * `pair_key`): under `separate` each `BASE-QUOTE` stays a distinct
 * instrument; under the merged modes (`parity`, `fx_adjusted`) all quote
 * variants of a base collapse into one instrument keyed by the bare base —
 * the merged row reports quote "USD" so cards render "BTC-USD".
 */
export function applyMarketUniverse(
  catalog: PerpTicker[],
  universe: VenueMarketUniverseDto[],
  mergeUsdQuotes = false
): PerpTicker[] {
  const catalogBySymbol = new Map(
    catalog.map((ticker) => [ticker.symbol.toUpperCase(), ticker])
  );
  const catalogByBase = new Map<string, PerpTicker>();
  for (const ticker of catalog) {
    const base = ticker.base.toUpperCase();
    if (!catalogByBase.has(base)) catalogByBase.set(base, ticker);
  }
  const symbolVenues = new Map<string, Set<string>>();
  const symbolPrices = new Map<string, number[]>();

  for (const entry of universe) {
    const code = VENUE_ID_TO_CODE[entry.venue_id.toLowerCase()];
    if (!code) continue;

    for (const rawSymbol of entry.symbols) {
      const parts = canonicalMarketParts(rawSymbol);
      if (!parts) continue;
      const key = mergeUsdQuotes ? parts.base : parts.symbol;

      const venues = symbolVenues.get(key) ?? new Set<string>();
      venues.add(code);
      symbolVenues.set(key, venues);

      const rawPrice = entry.base_prices?.[rawSymbol] ?? entry.base_prices?.[parts.symbol];
      const price = Number(rawPrice);
      if (Number.isFinite(price) && price > 0) {
        const prices = symbolPrices.get(key) ?? [];
        prices.push(price);
        symbolPrices.set(key, prices);
      }
    }
  }

  const discovered: PerpTicker[] = [];
  for (const [key, venues] of symbolVenues) {
    if (venues.size < 2) continue;
    const parts = mergeUsdQuotes
      ? { symbol: key, base: key, quote: "USD" }
      : canonicalMarketParts(key);
    if (!parts) continue;
    const known = catalogBySymbol.get(key) ?? catalogByBase.get(key);
    const livePrice = medianPositive(symbolPrices.get(key) ?? []);
    const fallbackPrice = known?.basePrice && known.basePrice > 0 ? known.basePrice : 0;
    discovered.push({
      symbol: parts.symbol,
      base: parts.base,
      quote: parts.quote,
      name: known?.name ?? parts.base,
      venues: Array.from(venues).sort(),
      basePrice: livePrice ?? fallbackPrice,
    });
  }

  discovered.sort((a, b) => a.symbol.localeCompare(b.symbol));
  return discovered;
}

/**
 * Filters the list of tickers to only those that have at least minVenues (default 2)
 * enabled and connected exchanges.
 */
export function filterAvailableTickers(
  tickers: PerpTicker[],
  activeVenueCodes: Set<string>,
  minVenues = 2
): PerpTicker[] {
  return tickers.filter((t) => {
    const activeVenues = getTickerActiveVenues(t, activeVenueCodes);
    return activeVenues.length >= minVenues;
  });
}

const VOLUME_KEY_QUOTE_SUFFIXES = [
  "-USDC",
  "-USDT",
  "-USD1",
  "-USD",
  "-PERP",
  "-SWAP",
  "-U",
  "/USDC",
  "/USDT",
  "/USD",
  "USDC",
  "USDT",
  "USD",
];

function stripVolumeQuoteSuffix(key: string): string {
  // Strip instrument-class and quote suffixes until stable so compound
  // spellings (`BTC-USDT-SWAP`, `BTC-USD-PERP`) fully reduce to the base.
  let cur = key;
  for (;;) {
    let next: string | undefined;
    for (const suf of VOLUME_KEY_QUOTE_SUFFIXES) {
      if (cur.length > suf.length && cur.endsWith(suf)) {
        const rest = cur.slice(0, cur.length - suf.length).replace(/[-/]+$/, "");
        if (rest.length >= 2) next = rest;
        break;
      }
    }
    if (next === undefined || next === cur) return cur;
    cur = next;
  }
}

/**
 * 24h notional volume (USD) for a base asset within one venue's volume map.
 * Keys are canonical `BASE-QUOTE` venue symbols (bare-base keys are also
 * tolerated); quote suffixes are stripped and "K"-/"1000"-prefix variants
 * like Hyperliquid's kPEPE map to the plain base. Returns undefined when
 * the venue does not report the asset.
 */
export function getVenueAssetVolumeUsd(
  volumes: Record<string, number>,
  base: string
): number | undefined {
  const b = base.toUpperCase();
  const candidates = [b, `K${b}`, `1000${b}`];
  // A venue may list the base under several quote assets (e.g. `BTC-USDT`
  // and `BTC-USD1`); take the deepest matching market — it bounds what the
  // venue can source for the base and keeps the pick deterministic.
  let best: number | undefined;
  for (const [rawKey, v] of Object.entries(volumes)) {
    if (typeof v !== "number" || !Number.isFinite(v) || v <= 0) continue;
    const key = rawKey.trim().toUpperCase();
    const stripped = stripVolumeQuoteSuffix(key);
    if (candidates.some((w) => w === key || w === stripped)) {
      best = best === undefined ? v : Math.max(best, v);
    }
  }
  return best;
}

/**
 * Mean 24h notional volume (USD) for a ticker's base asset across the venues
 * that report it. Returns undefined when no venue reports a volume for the asset.
 */
export function getTickerAvgVolumeUsd(
  base: string,
  venueVolumes: { volumes: Record<string, number> }[]
): number | undefined {
  const found: number[] = [];
  for (const venue of venueVolumes) {
    const v = getVenueAssetVolumeUsd(venue.volumes, base);
    if (v !== undefined) found.push(v);
  }
  if (found.length === 0) return undefined;
  return found.reduce((a, c) => a + c, 0) / found.length;
}

/**
 * MIN 24h notional volume (USD) across the ticker's in-scope venues.
 * Returns undefined when ANY in-scope venue is missing from the volume maps
 * or reports no volume for the asset (fail-closed). MIN bounds executable
 * size and defeats single-venue wash inflation.
 */
export function getTickerMinVolumeUsd(
  base: string,
  inScopeVenueIds: string[],
  venueVolumes: { venue_id?: string; venueId?: string; volumes: Record<string, number> }[]
): number | undefined {
  const byId = new Map<string, Record<string, number>>();
  for (const v of venueVolumes) {
    const id = String(v.venue_id ?? v.venueId ?? "").toLowerCase();
    if (id) byId.set(id, v.volumes);
  }
  let min: number | undefined;
  for (const vid of inScopeVenueIds) {
    const vols = byId.get(vid.toLowerCase());
    if (!vols) continue;
    const v = getVenueAssetVolumeUsd(vols, base);
    if (v === undefined) continue;
    min = min === undefined ? v : Math.min(min, v);
  }
  // If none of the in-scope venues had reported volume, check across all venue volumes
  if (min === undefined) {
    for (const v of venueVolumes) {
      const vol = getVenueAssetVolumeUsd(v.volumes, base);
      if (vol !== undefined) {
        min = min === undefined ? vol : Math.min(min, vol);
      }
    }
  }
  return min;
}

export interface AutoFilterThresholds {
  minVolumeUsd: number | null | undefined;
  minPriceUsd: number | null | undefined;
}

const thresholdOn = (v: number | null | undefined): v is number =>
  typeof v === "number" && Number.isFinite(v) && v > 0;

/**
 * Frontend mirror of the backend auto-filter for configurator previews:
 * every active threshold must pass on the MIN across in-scope venues that report.
 * Returns the first block reason, or null when the ticker passes.
 */
export function autoFilterBlockReason(
  base: string,
  inScopeVenueIds: string[],
  venueVolumes: { venue_id?: string; venueId?: string; volumes: Record<string, number> }[],
  priceByVenueId: Record<string, number>,
  thresholds: AutoFilterThresholds
): string | null {
  if (thresholdOn(thresholds.minVolumeUsd)) {
    const floor = thresholds.minVolumeUsd as number;
    const min = getTickerMinVolumeUsd(base, inScopeVenueIds, venueVolumes);
    if (min === undefined) {
      return `${base} blocked by auto volume filter: no venue reports 24h volume`;
    }
    if (min < floor) {
      return `${base} blocked by auto volume filter: min venue volume below floor`;
    }
  }
  if (thresholdOn(thresholds.minPriceUsd)) {
    const floor = thresholds.minPriceUsd as number;
    let min: number | undefined;
    for (const vid of inScopeVenueIds) {
      const p = priceByVenueId[vid.toLowerCase()] ?? priceByVenueId[vid];
      if (typeof p === "number" && Number.isFinite(p) && p > 0) {
        min = min === undefined ? p : Math.min(min, p);
      }
    }
    if (min === undefined) {
      const benchmark = getTickerBenchmarkPrice(base);
      if (benchmark !== undefined && benchmark > 0) {
        min = benchmark;
      }
    }
    if (min === undefined) {
      return `${base} blocked by auto price filter: no mid price available`;
    }
    if (min < floor) {
      return `${base} blocked by auto price filter: min venue mid below floor`;
    }
  }
  return null;
}

/**
 * Ticker visibility pipeline: universe minus the active strategy's
 * auto-filtered set, minus the global manual exemption set. Manual applies
 * AFTER auto and is managed separately. `autoBlocked` null means no
 * auto-filter is active (or not yet loaded) — only manual applies.
 */
export function applyTickerVisibility<T extends { symbol: string; base: string }>(
  universe: T[],
  autoBlocked: Set<string> | null,
  exempted: Set<string>
): T[] {
  return universe.filter((t) => {
    const base = t.base.toUpperCase();
    if (autoBlocked?.has(base)) return false;
    if (exempted.has(base)) return false;
    return true;
  });
}

// Curated metadata and benchmark-price fallback for well-known perpetuals.
// This is NOT the runtime allowlist: applyMarketUniverse() discovers the live
// universe from venue-reported markets and uses these rows only to enrich
// names and prices when a symbol is present.
export const PERP_TICKERS: PerpTicker[] = [
  { symbol: "BTC-USDC", base: "BTC", quote: "USDC", name: "Bitcoin", venues: ["HL", "LGT", "VNT", "RH", "KRK"], basePrice: 78550.0 },
  { symbol: "ETH-USDC", base: "ETH", quote: "USDC", name: "Ethereum", venues: ["HL", "LGT", "VNT", "RH", "KRK"], basePrice: 2470.0 },
  { symbol: "SOL-USDC", base: "SOL", quote: "USDC", name: "Solana", venues: ["HL", "LGT", "VNT", "RH", "KRK"], basePrice: 103.0 },
  { symbol: "AVAX-USDC", base: "AVAX", quote: "USDC", name: "Avalanche", venues: ["HL", "LGT", "VNT", "RH", "KRK"], basePrice: 8.07 },
  { symbol: "BNB-USDC", base: "BNB", quote: "USDC", name: "BNB", venues: ["HL", "LGT", "VNT"], basePrice: 751.0 },
  { symbol: "ARB-USDC", base: "ARB", quote: "USDC", name: "Arbitrum", venues: ["HL", "LGT", "VNT", "RH"], basePrice: 0.58 },
  { symbol: "OP-USDC", base: "OP", quote: "USDC", name: "Optimism", venues: ["HL", "LGT", "VNT", "RH"], basePrice: 1.45 },
  { symbol: "SUI-USDC", base: "SUI", quote: "USDC", name: "Sui", venues: ["HL", "LGT", "VNT"], basePrice: 0.82 },
  { symbol: "NEAR-USDC", base: "NEAR", quote: "USDC", name: "NEAR Protocol", venues: ["HL", "LGT", "VNT"], basePrice: 2.29 },
  { symbol: "APT-USDC", base: "APT", quote: "USDC", name: "Aptos", venues: ["HL", "LGT", "VNT"], basePrice: 5.40 },
  { symbol: "LINK-USDC", base: "LINK", quote: "USDC", name: "Chainlink", venues: ["HL", "LGT", "VNT", "RH", "KRK"], basePrice: 12.67 },
  { symbol: "DOGE-USDC", base: "DOGE", quote: "USDC", name: "Dogecoin", venues: ["HL", "LGT", "VNT", "RH", "KRK"], basePrice: 0.125 },
  { symbol: "PEPE-USDC", base: "PEPE", quote: "USDC", name: "Pepe", venues: ["HL", "LGT", "VNT"], basePrice: 0.0000095 },
  { symbol: "WIF-USDC", base: "WIF", quote: "USDC", name: "dogwifhat", venues: ["HL", "LGT", "VNT"], basePrice: 1.75 },
  { symbol: "POL-USDC", base: "POL", quote: "USDC", name: "Polygon / POL", venues: ["HL", "LGT", "VNT", "RH"], basePrice: 0.38 },
  { symbol: "TIA-USDC", base: "TIA", quote: "USDC", name: "Celestia", venues: ["HL", "LGT", "VNT"], basePrice: 5.20 },
  { symbol: "SEI-USDC", base: "SEI", quote: "USDC", name: "Sei", venues: ["HL", "LGT", "VNT"], basePrice: 0.32 },
  { symbol: "INJ-USDC", base: "INJ", quote: "USDC", name: "Injective", venues: ["HL", "LGT", "VNT"], basePrice: 21.50 },
  { symbol: "RENDER-USDC", base: "RENDER", quote: "USDC", name: "Render", venues: ["HL", "LGT", "VNT"], basePrice: 5.80 },
  { symbol: "FTM-USDC", base: "FTM", quote: "USDC", name: "Fantom / Sonic", venues: ["HL", "LGT", "VNT"], basePrice: 0.68 },
  { symbol: "ATOM-USDC", base: "ATOM", quote: "USDC", name: "Cosmos", venues: ["HL", "LGT", "VNT"], basePrice: 4.85 },
  { symbol: "ADA-USDC", base: "ADA", quote: "USDC", name: "Cardano", venues: ["HL", "LGT", "VNT", "RH", "KRK"], basePrice: 0.36 },
  { symbol: "XRP-USDC", base: "XRP", quote: "USDC", name: "XRP", venues: ["HL", "LGT", "VNT", "RH", "KRK"], basePrice: 0.58 },
  { symbol: "DOT-USDC", base: "DOT", quote: "USDC", name: "Polkadot", venues: ["HL", "LGT", "VNT"], basePrice: 4.40 },
  { symbol: "LTC-USDC", base: "LTC", quote: "USDC", name: "Litecoin", venues: ["HL", "LGT", "VNT", "RH", "KRK"], basePrice: 65.50 },
  { symbol: "BCH-USDC", base: "BCH", quote: "USDC", name: "Bitcoin Cash", venues: ["HL", "LGT", "VNT", "RH", "KRK"], basePrice: 340.0 },
  { symbol: "UNI-USDC", base: "UNI", quote: "USDC", name: "Uniswap", venues: ["HL", "LGT", "VNT", "RH", "KRK"], basePrice: 7.20 },
  { symbol: "AAVE-USDC", base: "AAVE", quote: "USDC", name: "Aave", venues: ["HL", "LGT", "VNT", "RH", "KRK"], basePrice: 145.0 },
  { symbol: "CRV-USDC", base: "CRV", quote: "USDC", name: "Curve DAO", venues: ["HL", "LGT", "VNT"], basePrice: 0.28 },
  { symbol: "MKR-USDC", base: "MKR", quote: "USDC", name: "Maker", venues: ["HL", "LGT", "VNT"], basePrice: 1650.0 },
  { symbol: "LDO-USDC", base: "LDO", quote: "USDC", name: "Lido DAO", venues: ["HL", "LGT", "VNT"], basePrice: 1.15 },
  { symbol: "JUP-USDC", base: "JUP", quote: "USDC", name: "Jupiter", venues: ["HL", "LGT", "VNT"], basePrice: 0.85 },
  { symbol: "PYTH-USDC", base: "PYTH", quote: "USDC", name: "Pyth Network", venues: ["HL", "LGT", "VNT"], basePrice: 0.34 },
  { symbol: "ONDO-USDC", base: "ONDO", quote: "USDC", name: "Ondo Finance", venues: ["HL", "LGT", "VNT"], basePrice: 0.72 },
  { symbol: "KAS-USDC", base: "KAS", quote: "USDC", name: "Kaspa", venues: ["HL", "LGT"], basePrice: 0.16 },
  { symbol: "TON-USDC", base: "TON", quote: "USDC", name: "Toncoin", venues: ["HL", "LGT", "VNT"], basePrice: 5.40 },
  { symbol: "NOT-USDC", base: "NOT", quote: "USDC", name: "Notcoin", venues: ["HL", "LGT"], basePrice: 0.0082 },
  { symbol: "PENDLE-USDC", base: "PENDLE", quote: "USDC", name: "Pendle", venues: ["HL", "LGT", "VNT"], basePrice: 4.10 },
  { symbol: "FET-USDC", base: "FET", quote: "USDC", name: "FET / ASI", venues: ["HL", "LGT", "VNT"], basePrice: 1.35 },
  { symbol: "TAO-USDC", base: "TAO", quote: "USDC", name: "Bittensor", venues: ["HL", "LGT", "VNT"], basePrice: 320.0 },
  { symbol: "WLD-USDC", base: "WLD", quote: "USDC", name: "Worldcoin", venues: ["HL", "LGT", "VNT"], basePrice: 1.65 },
  { symbol: "BLUR-USDC", base: "BLUR", quote: "USDC", name: "Blur", venues: ["HL", "LGT", "VNT"], basePrice: 0.22 },
  { symbol: "STX-USDC", base: "STX", quote: "USDC", name: "Stacks", venues: ["HL", "LGT", "VNT"], basePrice: 1.75 },
  { symbol: "RUNE-USDC", base: "RUNE", quote: "USDC", name: "THORChain", venues: ["HL", "LGT", "VNT"], basePrice: 4.90 },
  { symbol: "DYDX-USDC", base: "DYDX", quote: "USDC", name: "dYdX", venues: ["HL", "LGT", "VNT"], basePrice: 1.05 },
  { symbol: "ENA-USDC", base: "ENA", quote: "USDC", name: "Ethena", venues: ["HL", "LGT", "VNT"], basePrice: 0.35 },
  { symbol: "STRK-USDC", base: "STRK", quote: "USDC", name: "Starknet", venues: ["HL", "LGT", "VNT"], basePrice: 0.42 },
  { symbol: "JTO-USDC", base: "JTO", quote: "USDC", name: "Jito", venues: ["HL", "LGT", "VNT"], basePrice: 2.30 },
  { symbol: "BONK-USDC", base: "BONK", quote: "USDC", name: "Bonk", venues: ["HL", "LGT", "VNT"], basePrice: 0.000021 },
  { symbol: "FLOKI-USDC", base: "FLOKI", quote: "USDC", name: "Floki", venues: ["HL", "LGT", "VNT"], basePrice: 0.00014 },
  { symbol: "SHIB-USDC", base: "SHIB", quote: "USDC", name: "Shiba Inu", venues: ["HL", "LGT", "VNT", "RH"], basePrice: 0.000014 },
  { symbol: "AR-USDC", base: "AR", quote: "USDC", name: "Arweave", venues: ["HL", "LGT"], basePrice: 20.50 },
];

export function getTickerBenchmarkPrice(base: string): number | undefined {
  const b = base.trim().toUpperCase();
  const t = PERP_TICKERS.find((item) => item.base.toUpperCase() === b);
  return t?.basePrice;
}
