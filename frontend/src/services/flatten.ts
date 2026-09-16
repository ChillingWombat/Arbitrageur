import type { FlattenResultDto } from "../types/index.ts";

export function flattenResidualMessage(res: FlattenResultDto): string | null {
  return res.residualSignals > 0
    ? `Flatten incomplete: ${res.residualSignals} signal(s) still hold residual exposure`
    : null;
}
