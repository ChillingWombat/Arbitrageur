/** Historical-retention input normalization shared by the System Settings
 * Execution tab. Empty input means unlimited (`None` on the backend; `0`
 * normalizes to `None` there too). Non-finite or non-positive input clamps
 * to unlimited rather than persisting a nonsensical bound. */

export function normalizeRetentionDays(raw: string): number | null {
  const t = raw.trim();
  if (t === "") return null;
  const n = Number(t);
  if (!Number.isFinite(n) || n <= 0) return null;
  return Math.floor(n);
}

export function normalizeRetentionGb(raw: string): number | null {
  const t = raw.trim();
  if (t === "") return null;
  const n = Number(t);
  if (!Number.isFinite(n) || n <= 0) return null;
  return n;
}

export function retentionDaysToInput(value: number | null | undefined): string {
  return value != null ? String(value) : "";
}

export function retentionGbToInput(value: number | null | undefined): string {
  return value != null ? String(value) : "";
}
