import type { PassiveUnwindDto, StrategyParamsDto } from "../types";

/** Client-side mirror of the backend `save_strategy_params` guards, so the
 * configurator surfaces validation messages without a failed IPC round-trip.
 * Message strings intentionally match the backend errors field-for-field.
 * The backend remains authoritative: every save still validates server-side.
 */
export function validateStrategyParams(p: StrategyParamsDto): string[] {
  const errors: string[] = [];
  const positive = (v: unknown, label: string): void => {
    if (typeof v !== "number" || !Number.isFinite(v) || v <= 0) {
      errors.push(`${label} must be finite and positive`);
    }
  };
  const pctOpt = (v: unknown, label: string): void => {
    if (v === null || v === undefined) return;
    if (typeof v !== "number" || !Number.isFinite(v) || v <= 0 || v > 100) {
      errors.push(`${label} must be within (0, 100] when set`);
    }
  };
  positive(p.futuresMaxSlippageBps, "futuresMaxSlippageBps");
  positive(p.fundingMaxSlippageBps, "fundingMaxSlippageBps");
  positive(p.predictionMaxSlippageBps, "predictionMaxSlippageBps");
  positive(p.fundingMaxEntrySpreadBps, "fundingMaxEntrySpreadBps");
  pctOpt(p.fundingAutoExitSpreadPct, "fundingAutoExitSpreadPct");
  pctOpt(p.fundingLiqTrimPct, "fundingLiqTrimPct");
  pctOpt(p.futuresLiqTrimPct, "futuresLiqTrimPct");
  pctOpt(p.spotLiqTrimPct, "spotLiqTrimPct");
  pctOpt(p.predictionLiqTrimPct, "predictionLiqTrimPct");
  if (p.fundingRotationThresholdBps !== null && p.fundingRotationThresholdBps !== undefined) {
    positive(p.fundingRotationThresholdBps, "fundingRotationThresholdBps");
  }
  if (p.triangleTradeSizeUsd !== undefined) {
    positive(p.triangleTradeSizeUsd, "triangleTradeSizeUsd");
  }
  if (p.triangleMinProfitBps !== undefined) {
    positive(p.triangleMinProfitBps, "triangleMinProfitBps");
  }
  if (p.triangleMaxSlippageBps !== undefined) {
    positive(p.triangleMaxSlippageBps, "triangleMaxSlippageBps");
  }
  validatePassiveUnwind(p.futuresPassiveUnwind, "futuresPassiveUnwind", errors);
  validatePassiveUnwind(p.fundingPassiveUnwind, "fundingPassiveUnwind", errors);
  validatePassiveUnwind(p.spotPassiveUnwind, "spotPassiveUnwind", errors);
  return errors;
}

/** Client-side mirror of `PassiveUnwindConfig::validate` plus the
 * save-path floor (timeoutSecs >= 1 when enabled). Absent blocks pass:
 * the backend fills defaults for rows persisted before this block. */
export function validatePassiveUnwind(
  v: PassiveUnwindDto | null | undefined,
  label: string,
  errors: string[],
): void {
  if (v === null || v === undefined) return;
  if (
    typeof v.spreadPercentile !== "number" ||
    !Number.isFinite(v.spreadPercentile) ||
    v.spreadPercentile < 0 ||
    v.spreadPercentile > 100
  ) {
    errors.push(`${label}: spreadPercentile must be within [0, 100]`);
  }
  if (
    typeof v.timeoutSecs !== "number" ||
    !Number.isInteger(v.timeoutSecs) ||
    v.timeoutSecs > 300 ||
    (v.enabled ? v.timeoutSecs < 1 : v.timeoutSecs < 0)
  ) {
    errors.push(`${label}: timeoutSecs must be within 0..=300 (at least 1 when enabled)`);
  }
  if (
    typeof v.maxTries !== "number" ||
    !Number.isInteger(v.maxTries) ||
    v.maxTries < 1 ||
    v.maxTries > 10
  ) {
    errors.push(`${label}: maxTries must be within 1..=10`);
  }
  if (
    typeof v.maxSpreadBps !== "number" ||
    !Number.isFinite(v.maxSpreadBps) ||
    v.maxSpreadBps <= 0
  ) {
    errors.push(`${label}: maxSpreadBps must be finite and positive`);
  }
  if (
    typeof v.maxFastVolBps !== "number" ||
    !Number.isFinite(v.maxFastVolBps) ||
    v.maxFastVolBps <= 0
  ) {
    errors.push(`${label}: maxFastVolBps must be finite and positive`);
  }
}

/** Parse an optional percent/bps text input: "" (or nonpositive) = off. */
export function parseOptThreshold(raw: string): number | null {
  const v = parseFloat(raw);
  return Number.isFinite(v) && v > 0 ? v : null;
}
