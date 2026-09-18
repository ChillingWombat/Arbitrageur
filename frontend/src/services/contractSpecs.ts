/**
 * Cross-Exchange Contract Specifications and Compatibility Engine.
 *
 * Provides:
 * 1. Venue contract parameters (min size, step size, max leverage, contract multiplier).
 * 2. Minimum compatible position size resolution across active exchanges.
 * 3. Automatic safe leverage and margin calculation across differing futures contracts.
 */

export interface VenueContractSpec {
  venueId: string;
  minOrderSize: number;
  stepSize: number;
  maxLeverage: number;
  contractMultiplier: number;
}

export interface CompatibleSizeResult {
  minCompatibleSize: number;
  stepSize: number;
  recommendedUnitSize: number;
  venueSpecs: Record<string, VenueContractSpec>;
  explanation: string;
}

export interface LeverageComputationResult {
  effectiveLeverage: number;
  maxVenueLeverage: number;
  marginPerLegUsd: number;
  totalMarginRequiredUsd: number;
  notionalUsd: number;
  liquidationBufferPct: number;
  venueLeverages: Record<string, number>;
}

/**
 * Normalizes venue identifiers to standard keys: "hyperliquid", "lighter", "vanta", "robinhood".
 */
export function normalizeVenueKey(v: string): string {
  const s = v.toLowerCase();
  if (s.includes("hyper") || s === "hl") return "hyperliquid";
  if (s.includes("lighter") || s === "lgt") return "lighter";
  if (s.includes("vanta") || s === "vnt") return "vanta";
  if (s.includes("robin") || s === "rh") return "robinhood";
  if (s.includes("arcus") || s === "arc") return "arcus";
  if (s.includes("pop") || s === "popdex") return "popdex";
  if (s.includes("kraken") || s === "krk") return "kraken";
  if (s.includes("poly")) return "polymarket";
  if (s.includes("kalshi")) return "kalshi";
  return s;
}

/**
 * Returns the contract specification for a given venue and symbol.
 */
export function getVenueContractSpec(venueKey: string, symbol: string, basePrice: number): VenueContractSpec {
  const v = normalizeVenueKey(venueKey);
  const base = symbol.split("-")[0].toUpperCase();

  // Tier 1: BTC
  if (base === "BTC") {
    switch (v) {
      case "hyperliquid":
        return { venueId: "hyperliquid", minOrderSize: 0.0001, stepSize: 0.0001, maxLeverage: 50, contractMultiplier: 1.0 };
      case "lighter":
        return { venueId: "lighter", minOrderSize: 0.001, stepSize: 0.001, maxLeverage: 20, contractMultiplier: 1.0 };
      case "vanta":
        return { venueId: "vanta", minOrderSize: 0.001, stepSize: 0.001, maxLeverage: 20, contractMultiplier: 1.0 };
      case "arcus":
        return { venueId: "arcus", minOrderSize: 0.0005, stepSize: 0.0005, maxLeverage: 25, contractMultiplier: 1.0 };
      case "popdex":
        return { venueId: "popdex", minOrderSize: 0.001, stepSize: 0.001, maxLeverage: 20, contractMultiplier: 1.0 };
      case "robinhood":
        return { venueId: "robinhood", minOrderSize: 0.005, stepSize: 0.001, maxLeverage: 10, contractMultiplier: 1.0 };
      default:
        return { venueId: v, minOrderSize: 0.001, stepSize: 0.001, maxLeverage: 10, contractMultiplier: 1.0 };
    }
  }

  // Tier 2: ETH
  if (base === "ETH") {
    switch (v) {
      case "hyperliquid":
        return { venueId: "hyperliquid", minOrderSize: 0.001, stepSize: 0.001, maxLeverage: 50, contractMultiplier: 1.0 };
      case "lighter":
        return { venueId: "lighter", minOrderSize: 0.01, stepSize: 0.005, maxLeverage: 20, contractMultiplier: 1.0 };
      case "vanta":
        return { venueId: "vanta", minOrderSize: 0.01, stepSize: 0.005, maxLeverage: 20, contractMultiplier: 1.0 };
      case "robinhood":
        return { venueId: "robinhood", minOrderSize: 0.02, stepSize: 0.01, maxLeverage: 10, contractMultiplier: 1.0 };
      default:
        return { venueId: v, minOrderSize: 0.01, stepSize: 0.005, maxLeverage: 10, contractMultiplier: 1.0 };
    }
  }

  // Tier 3: SOL, AVAX, BNB
  if (base === "SOL" || base === "AVAX" || base === "BNB") {
    switch (v) {
      case "hyperliquid":
        return { venueId: "hyperliquid", minOrderSize: 0.01, stepSize: 0.01, maxLeverage: 20, contractMultiplier: 1.0 };
      case "lighter":
        return { venueId: "lighter", minOrderSize: 0.1, stepSize: 0.05, maxLeverage: 15, contractMultiplier: 1.0 };
      case "vanta":
        return { venueId: "vanta", minOrderSize: 0.1, stepSize: 0.05, maxLeverage: 15, contractMultiplier: 1.0 };
      case "robinhood":
        return { venueId: "robinhood", minOrderSize: 0.2, stepSize: 0.1, maxLeverage: 8, contractMultiplier: 1.0 };
      default:
        return { venueId: v, minOrderSize: 0.1, stepSize: 0.05, maxLeverage: 10, contractMultiplier: 1.0 };
    }
  }

  // Tier 4: Low-price meme tokens (PEPE, BONK, SHIB, FLOKI)
  if (basePrice < 0.001) {
    const rawMin = Math.max(10000, Math.round(15 / Math.max(0.000001, basePrice)));
    switch (v) {
      case "hyperliquid":
        return { venueId: "hyperliquid", minOrderSize: rawMin, stepSize: 1000, maxLeverage: 10, contractMultiplier: 1.0 };
      case "lighter":
        return { venueId: "lighter", minOrderSize: rawMin * 1.5, stepSize: 5000, maxLeverage: 10, contractMultiplier: 1.0 };
      case "vanta":
        return { venueId: "vanta", minOrderSize: rawMin * 1.5, stepSize: 5000, maxLeverage: 10, contractMultiplier: 1.0 };
      case "robinhood":
        return { venueId: "robinhood", minOrderSize: rawMin * 2, stepSize: 10000, maxLeverage: 5, contractMultiplier: 1.0 };
      default:
        return { venueId: v, minOrderSize: rawMin, stepSize: 1000, maxLeverage: 5, contractMultiplier: 1.0 };
    }
  }

  // General Altcoins (ARB, OP, SUI, NEAR, APT, LINK, etc.)
  const minUnitsForTenUsd = Math.max(0.1, Math.round((12 / Math.max(0.01, basePrice)) * 10) / 10);
  switch (v) {
    case "hyperliquid":
      return { venueId: "hyperliquid", minOrderSize: minUnitsForTenUsd, stepSize: 0.1, maxLeverage: 15, contractMultiplier: 1.0 };
    case "lighter":
      return { venueId: "lighter", minOrderSize: minUnitsForTenUsd * 1.2, stepSize: 0.1, maxLeverage: 10, contractMultiplier: 1.0 };
    case "vanta":
      return { venueId: "vanta", minOrderSize: minUnitsForTenUsd * 1.2, stepSize: 0.1, maxLeverage: 10, contractMultiplier: 1.0 };
    case "robinhood":
      return { venueId: "robinhood", minOrderSize: minUnitsForTenUsd * 1.5, stepSize: 0.5, maxLeverage: 5, contractMultiplier: 1.0 };
    default:
      return { venueId: v, minOrderSize: minUnitsForTenUsd, stepSize: 0.1, maxLeverage: 10, contractMultiplier: 1.0 };
  }
}

/**
 * Computes the minimum compatible position size across all participating exchanges for a ticker.
 * The compatible size must be >= the maximum of all venues' minimum order requirements,
 * rounded up to the least common step size.
 */
export function getMinCompatibleSize(
  symbol: string,
  venues: string[],
  basePrice: number
): CompatibleSizeResult {
  const activeVenues = venues.length > 0 ? venues : ["hyperliquid", "lighter", "vanta"];
  const venueSpecs: Record<string, VenueContractSpec> = {};

  let maxMinOrderSize = 0;
  let maxStepSize = 0;

  for (const v of activeVenues) {
    const norm = normalizeVenueKey(v);
    const spec = getVenueContractSpec(norm, symbol, basePrice);
    venueSpecs[norm] = spec;
    if (spec.minOrderSize > maxMinOrderSize) {
      maxMinOrderSize = spec.minOrderSize;
    }
    if (spec.stepSize > maxStepSize) {
      maxStepSize = spec.stepSize;
    }
  }

  // Round up maxMinOrderSize to the next multiple of maxStepSize
  const precision = maxStepSize < 1 ? Math.min(6, Math.ceil(-Math.log10(maxStepSize))) : 0;
  const factor = Math.pow(10, precision);
  const roundedMin = Math.ceil(maxMinOrderSize / maxStepSize) * maxStepSize;
  const minCompatibleSize = Math.round(roundedMin * factor) / factor;

  // Recommended unit size: at least 3-5x minimum compatible size or ~$1000 USD notional
  const notionalTarget = 2500;
  const targetUnits = basePrice > 0 ? notionalTarget / basePrice : minCompatibleSize * 5;
  const rawRecommended = Math.max(minCompatibleSize, targetUnits);
  const recommendedUnitSize = Math.round(Math.ceil(rawRecommended / maxStepSize) * maxStepSize * factor) / factor;

  const venueMinList = Object.entries(venueSpecs)
    .map(([vid, sp]) => `${vid.toUpperCase()}: ${sp.minOrderSize}`)
    .join(", ");

  const explanation = `Compatible across [${activeVenues.join(", ")}]. Minimum lot constraint: ${minCompatibleSize} (specs: ${venueMinList})`;

  return {
    minCompatibleSize,
    stepSize: maxStepSize,
    recommendedUnitSize,
    venueSpecs,
    explanation,
  };
}

/**
 * Single Spread step-sizing parameters (authored once per strategy).
 * Session fields are display/execution multipliers from the session
 * profile (Phase 1): each defaults to neutral 1.0 when the aggregate is
 * provisional or unknown — never a fabricated value.
 */
export interface StepSizingParams {
  stepSizeMult: number;
  maxStepUsd: number;
  maxTickerEquityPct: number;
  /** Session size-allowance (overlap 1.0 … dead 0.4, weekend x0.5, mask 0). */
  sessionAllow?: number;
  /** Inverse-vol factor clamp(sigma_med / sigma_h, 0.5, 1.5). */
  sessionInvVol?: number;
  /** Depth factor clamp(depth_h / depth_med, 0.25, 1.0). */
  sessionDepth?: number;
}

export interface StepSizingResult {
  /** Venue-compatible minimum lot, in base units. */
  minCompatBase: number;
  /** One grid step in base units: min(mult x min lot, maxStepUsd / price),
   *  quantized UP to the venue step, never below the min lot. */
  stepBaseQty: number;
  /** stepBaseQty x price. */
  stepUsd: number;
  /** max(1, floor(known equity x pct/100 / stepUsd)); 0 when equity is unknown. */
  maxSteps: number;
}

export const DEFAULT_STEP_SIZING: StepSizingParams = {
  stepSizeMult: 10,
  maxStepUsd: 2500,
  maxTickerEquityPct: 20,
};

/** Sum of live venue paper equities, or null when balances are unavailable
 *  (empty, non-finite, or zero-sum). Callers must treat null as unknown and
 *  fail closed — never substitute a hardcoded seed. */
export function resolveTotalPaperEquity(
  balances: { equity_usd: number }[]
): number | null {
  const sum = (balances ?? []).reduce(
    (acc, b) => acc + (Number.isFinite(b?.equity_usd) ? b.equity_usd : 0),
    0
  );
  return sum > 0 ? sum : null;
}

/**
 * Canonical Single Spread step sizing. Single helper consumed by BOTH the
 * LiveMatrix orderQty derivation AND the App refreshAllData scan qty so
 * scan/execute cannot diverge.
 */
function sessionFactor(value: number | undefined, lo: number, hi: number): number {
  const v = Number(value);
  if (!Number.isFinite(v)) return 1.0;
  return Math.min(hi, Math.max(lo, v));
}

export function deriveStepSizing(
  symbol: string,
  venues: string[],
  basePrice: number,
  totalEquityUsd: number | null,
  params: StepSizingParams
): StepSizingResult {
  const { minCompatibleSize, stepSize } = getMinCompatibleSize(symbol, venues, basePrice);
  const mult =
    Number.isFinite(params?.stepSizeMult) && (params.stepSizeMult as number) >= 1
      ? (params.stepSizeMult as number)
      : DEFAULT_STEP_SIZING.stepSizeMult;
  const maxStepUsd =
    Number.isFinite(params?.maxStepUsd) && (params.maxStepUsd as number) > 0
      ? (params.maxStepUsd as number)
      : DEFAULT_STEP_SIZING.maxStepUsd;
  const pct =
    Number.isFinite(params?.maxTickerEquityPct) &&
    (params.maxTickerEquityPct as number) > 0 &&
    (params.maxTickerEquityPct as number) <= 100
      ? (params.maxTickerEquityPct as number)
      : DEFAULT_STEP_SIZING.maxTickerEquityPct;

  const step = stepSize || 0.0001;
  let stepBaseQty: number;
  if (!(basePrice > 0)) {
    stepBaseQty = Math.ceil((mult * minCompatibleSize) / step) * step;
  } else {
    const target = Math.min(mult * minCompatibleSize, maxStepUsd / basePrice);
    stepBaseQty = Math.ceil(target / step) * step;
  }
  const precision = step < 1 ? Math.min(6, Math.ceil(-Math.log10(step))) : 0;
  const factor = Math.pow(10, precision);
  stepBaseQty = Math.max(minCompatibleSize, Math.round(stepBaseQty * factor) / factor);

  // Session multipliers (Phase 1): allow x inverse-vol x depth, each
  // neutral 1.0 when unknown. A zero allow (funding mask) zeroes the step;
  // the min-lot floor never fabricates size above zero.
  const sessAllow = sessionFactor(params?.sessionAllow, 0, 1);
  const sessInvVol = sessionFactor(params?.sessionInvVol, 0.5, 1.5);
  const sessDepth = sessionFactor(params?.sessionDepth, 0.25, 1);
  const sessMult = sessAllow * sessInvVol * sessDepth;
  if (sessMult <= 0) {
    stepBaseQty = 0;
  } else if (sessMult !== 1) {
    stepBaseQty = Math.max(
      minCompatibleSize,
      Math.floor((stepBaseQty * sessMult) / step) * step,
    );
  }

  const stepUsd = basePrice > 0 ? stepBaseQty * basePrice : 0;
  const equity = typeof totalEquityUsd === "number" ? totalEquityUsd : NaN;
  const equityCap =
    Number.isFinite(equity) && equity > 0 ? (equity * pct) / 100 : 0;
  const maxSteps = equityCap > 0 && stepUsd > 0 ? Math.max(1, Math.floor(equityCap / stepUsd)) : 0;

  return { minCompatBase: minCompatibleSize, stepBaseQty, stepUsd, maxSteps };
}

/**
 * Effective leverage for an arbitrage position: min(strategy target, account
 * max). The strategy target defaults to 5x; an empty account max means
 * unlimited. No upper cap and no venue component — venue contract maxima are
 * reported for reference only (see maxVenueLeverage / venueLeverages).
 */
export function computeSafeLeverage(
  symbol: string,
  venues: string[],
  basePrice: number,
  unitSize: number,
  userTargetLeverage?: number,
  accountMaxLeverage?: number | null
): LeverageComputationResult {
  const activeVenues = venues.length > 0 ? venues : ["hyperliquid", "lighter"];
  const venueLeverages: Record<string, number> = {};

  let minVenueLeverage = 100;

  for (const v of activeVenues) {
    const norm = normalizeVenueKey(v);
    const spec = getVenueContractSpec(norm, symbol, basePrice);
    venueLeverages[norm] = spec.maxLeverage;
    if (spec.maxLeverage < minVenueLeverage) {
      minVenueLeverage = spec.maxLeverage;
    }
  }

  // Effective leverage is min(strategy target, account max). Strategy target
  // defaults to 5x; empty account max means unlimited. No cap, no venue term.
  const target = userTargetLeverage && userTargetLeverage > 0 ? userTargetLeverage : 5;
  const effectiveLeverage = Math.max(
    1,
    accountMaxLeverage != null && accountMaxLeverage > 0 ? Math.min(target, accountMaxLeverage) : target,
  );

  // Compute notional and margin requirements
  const notionalUsd = Math.round(basePrice * unitSize * 100) / 100;
  const marginPerLegUsd = Math.round((notionalUsd / effectiveLeverage) * 100) / 100;
  const totalMarginRequiredUsd = Math.round(marginPerLegUsd * 2 * 100) / 100;

  // Liquidation buffer: approximate adverse excursion allowable before liquidation tripwire
  const liquidationBufferPct = Math.round(((1 / effectiveLeverage) * 100 * 0.9) * 10) / 10;

  return {
    effectiveLeverage,
    maxVenueLeverage: minVenueLeverage,
    marginPerLegUsd,
    totalMarginRequiredUsd,
    notionalUsd,
    liquidationBufferPct,
    venueLeverages,
  };
}
