export type SessionId = "asia" | "europe" | "overlap" | "us_late" | "dead";

export function sessionOf(tsMs: number): SessionId {
  if (!Number.isFinite(tsMs)) return "dead";
  const hour = new Date(tsMs).getUTCHours();
  if (hour < 0 || hour > 23 || !Number.isInteger(hour)) return "dead";
  if (hour < 8) return "asia";
  if (hour < 13) return "europe";
  if (hour < 17) return "overlap";
  if (hour < 22) return "us_late";
  return "dead";
}

export function isWeekendUtc(tsMs: number): boolean {
  if (!Number.isFinite(tsMs)) return false;
  const day = new Date(tsMs).getUTCDay();
  return day === 0 || day === 6;
}

export const SESSION_LABEL: Record<SessionId, string> = {
  asia: "ASIA",
  europe: "EU",
  overlap: "OVERLAP",
  us_late: "US LATE",
  dead: "DEAD",
};

export const SESSION_BAND_COLOR: Record<SessionId, string> = {
  asia: "#e0e7ff",
  europe: "#bae6fd",
  overlap: "#a7f3d0",
  us_late: "#fde68a",
  dead: "#fecaca",
};

export function sessionLabel(sessionId: string | null | undefined): string {
  if (sessionId == null) return "—";
  const key = sessionId.toLowerCase() as SessionId;
  return SESSION_LABEL[key] ?? "—";
}

export interface SessionSegment {
  session: SessionId;
  fromMs: number;
  toMs: number;
}

export function sessionSegments(t0Ms: number, t1Ms: number): SessionSegment[] {
  if (!Number.isFinite(t0Ms) || !Number.isFinite(t1Ms) || t1Ms <= t0Ms) return [];
  const out: SessionSegment[] = [];
  let cursor = t0Ms;
  let guard = 0;
  while (cursor < t1Ms && guard < 240) {
    guard += 1;
    const d = new Date(cursor);
    d.setUTCMinutes(0, 0, 0);
    const nextHour = d.getTime() + 3_600_000;
    const end = Math.min(nextHour, t1Ms);
    const session = sessionOf(cursor);
    const last = out[out.length - 1];
    if (last != null && last.session === session) {
      last.toMs = end;
    } else {
      out.push({ session, fromMs: cursor, toMs: end });
    }
    cursor = end;
  }
  return out;
}
