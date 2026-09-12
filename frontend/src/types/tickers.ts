export interface PerpTicker {
  symbol: string;         // e.g. "BTC-USDC"
  base: string;           // e.g. "BTC"
  quote: string;          // e.g. "USDC"
  name: string;           // e.g. "Bitcoin"
  venues: string[];       // e.g. ["HL", "LGT", "VNT", "RH"] - all have length >= 2
  basePrice: number;      // Reference benchmark price
}

export type TickerSortOption = "alpha" | "trades" | "pnl";

export interface TickerWithStats extends PerpTicker {
  tradeCount: number;
  netProfit: number;
  netProfitBps?: number;
  hasIssue?: boolean;
  issueDescription?: string;
  lastTradeTime?: string;
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

export function formatCurrencyPnl(pnl: number): string {
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
export const VENUE_ID_TO_CODE: Record<string, string> = {
  hyperliquid: "HL",
  lighter: "LGT",
  vanta: "VNT",
  robinhood: "RH",
  arcus: "ARC",
  popdex: "POP",
  kraken: "KRK",
};

export const VENUE_CODE_TO_ID: Record<string, string> = {
  HL: "hyperliquid",
  LGT: "lighter",
  VNT: "vanta",
  RH: "robinhood",
  ARC: "arcus",
  POP: "popdex",
  KRK: "kraken",
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
  popdex_enabled?: boolean;
  popdex_api_key?: string;
  kraken_enabled?: boolean;
  kraken_spot_enabled?: boolean;
  kraken_futures_enabled?: boolean;
  kraken_spot_api_key?: string;
  kraken_futures_api_key?: string;
  polymarket_enabled?: boolean;
  polymarket_api_key?: string;
  polymarket_secret?: string;
  kalshi_enabled?: boolean;
  kalshi_api_key?: string;
  kalshi_email?: string;
  configured_venues?: string[];
}

/**
 * Determines the set of natively connected venue codes from sanitized engine settings.
 * Only Hyperliquid ("HL") and Lighter ("LGT") expose read-only native market-data
 * adapters in this phase, and only outside live trading mode. Public read-only
 * feeds require the venue's enable toggle, not secrets; connectivity is never
 * inferred from credential-looking strings.
 */
export function getEnabledConnectedVenueCodes(
  settings?: VenueSettingsFilter | null,
  isLiveMode?: boolean
): Set<string> {
  const active = new Set<string>();
  if (!settings || isLiveMode === true || settings.trading_mode === "real") {
    return active;
  }

  if (settings.hyperliquid_enabled === true) {
    active.add("HL");
  }
  if (settings.lighter_enabled === true) {
    active.add("LGT");
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

// Complete list of the union of available tickers across perp DEXes (Hyperliquid, Lighter, Vanta, Robinhood, Kraken)
// Every ticker is available on at least 2 exchanges (52 tickers total, exceeding the >40 requirement).
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
