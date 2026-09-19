import React, { useMemo, useState } from "react";

interface VenueLogoProps {
  /** Canonical venue id (e.g. "hyperliquid", "standx"). */
  venueId: string;
  /** Display name used to derive fallback initials (e.g. "dYdX Chain"). */
  name: string;
  /** Diameter in px. Defaults to 22 (settings tab size). */
  size?: number;
}

/**
 * Venues with no verifiable official mark. alphax/iotrader/sodex resolve to
 * parked domains; gatedex has no such venue (DNS-fails, unverified); niza
 * points at a memecoin site with no futures product. They render the
 * initials badge instead of an <img> rather than a fabricated logo.
 */
const NO_LOGO_VENUES = new Set(["alphax", "iotrader", "sodex", "gatedex", "niza"]);

/** Curated brand hues (0-360) for the initials fallback badges. */
const BRAND_HUE: Record<string, number> = {
  alphax: 210,
  iotrader: 200,
  sodex: 190,
  gatedex: 150,
  niza: 45,
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
 * Round venue logo. Real brand marks live in `public/logos/{venueId}.png`
 * (96px round-cropped PNGs fetched from official sites/favicons and
 * circular-masked; see /tmp for the fetch/crop scripts). Venues without a
 * recoverable mark — or any logo that fails to load — fall back to a
 * deterministic initials badge, so no tab ever renders broken.
 */
export const VenueLogo: React.FC<VenueLogoProps> = ({ venueId, name, size = 22 }) => {
  const [failed, setFailed] = useState(false);
  const fallback = useMemo(() => {
    const hue = BRAND_HUE[venueId] ?? hashHue(venueId);
    return {
      initials: initialsOf(name),
      bg: `hsl(${hue}, 45%, 88%)`,
      fg: `hsl(${hue}, 45%, 26%)`,
    };
  }, [venueId, name]);

  const style: React.CSSProperties = {
    width: size,
    height: size,
    borderRadius: 9999,
    flexShrink: 0,
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    userSelect: "none",
    overflow: "hidden",
    boxShadow: "inset 0 0 0 1px rgba(15, 23, 42, 0.08)",
    border: "1px solid rgba(15, 23, 42, 0.06)",
    backgroundColor: "#fff",
  };

  if (!failed && !NO_LOGO_VENUES.has(venueId)) {
    // `contain` + inset padding keeps the mark centered, fully visible, and
    // clear of the round edge (gap = white bed). `cover` cropped edge-to-edge
    // marks and clipped oversized art — the source of the "too large / not
    // centered" reports.
    const pad = Math.max(2, Math.round(size * 0.18));
    return (
      <img
        src={`logos/${venueId}.png`}
        alt=""
        aria-hidden="true"
        title={`${name} — official brand mark`}
        width={size}
        height={size}
        style={{ ...style, objectFit: "contain", padding: pad, boxSizing: "border-box" }}
        onError={() => setFailed(true)}
        draggable={false}
      />
    );
  }

  const noMark = NO_LOGO_VENUES.has(venueId);
  return (
    <span
      aria-hidden="true"
      title={noMark ? `${name} — no official mark published, initials badge` : name}
      className="font-semibold"
      style={{
        ...style,
        fontSize: Math.max(8, Math.round(size * 0.4)),
        lineHeight: 1,
        letterSpacing: "0.02em",
        backgroundColor: fallback.bg,
        color: fallback.fg,
        fontFamily: "Inter, system-ui, sans-serif",
      }}
    >
      {fallback.initials}
    </span>
  );
};
