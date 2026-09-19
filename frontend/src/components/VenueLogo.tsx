import React, { useMemo } from "react";

interface VenueLogoProps {
  /** Canonical venue id (e.g. "hyperliquid", "standx"). */
  venueId: string;
  /** Display name used to derive initials (e.g. "dYdX Chain"). */
  name: string;
  /** Diameter in px. Defaults to 22 (settings tab size). */
  size?: number;
}

/** Curated brand hues (0-360) for major venues; everything else hashes
 *  deterministically from the venue id so logos are stable across reloads. */
const BRAND_HUE: Record<string, number> = {
  hyperliquid: 160,
  lighter: 210,
  binance: 45,
  okx: 220,
  coinbase: 215,
  kraken: 260,
  dydx: 230,
  paradex: 190,
  gmx: 200,
  aevo: 250,
  apex: 20,
  grvt: 150,
  edgex: 180,
  polymarket: 265,
  kalshi: 110,
  hyperliquid_evm: 160,
};

function hashHue(id: string): number {
  let h = 0;
  for (let i = 0; i < id.length; i++) {
    h = (h * 31 + id.charCodeAt(i)) >>> 0;
  }
  return h % 360;
}

function initialsOf(name: string): string {
  const tokens = name.trim().split(/[\s\-_/]+/).filter(Boolean);
  if (tokens.length >= 2) {
    return (tokens[0][0] + tokens[1][0]).toUpperCase();
  }
  const t = tokens[0] ?? "?";
  return (t.slice(0, 2) || "?").toUpperCase();
}

/**
 * Round venue badge: initials on a soft tinted disc. Offline-safe (no
 * remote images) and deterministic per venue, so all 43 exchange tabs
 * share one consistent logo language. Real brand SVGs can replace
 * individual venues later by extending BRAND_HUE or this component.
 */
export const VenueLogo: React.FC<VenueLogoProps> = ({ venueId, name, size = 22 }) => {
  const { initials, bg, fg } = useMemo(() => {
    const hue = BRAND_HUE[venueId] ?? hashHue(venueId);
    return {
      initials: initialsOf(name),
      bg: `hsl(${hue}, 45%, 88%)`,
      fg: `hsl(${hue}, 45%, 26%)`,
    };
  }, [venueId, name]);

  return (
    <span
      aria-hidden="true"
      title={name}
      className="rounded-full shrink-0 inline-flex items-center justify-center font-semibold select-none"
      style={{
        width: size,
        height: size,
        fontSize: Math.max(8, Math.round(size * 0.42)),
        lineHeight: 1,
        backgroundColor: bg,
        color: fg,
        fontFamily: "Inter, system-ui, sans-serif",
      }}
    >
      {initials}
    </span>
  );
};
