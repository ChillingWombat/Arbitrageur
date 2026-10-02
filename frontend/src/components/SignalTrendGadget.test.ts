import nodeTest from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { SignalTrendGadget } from "./SignalTrendGadget.tsx";
import type { SignalSeriesDto, SpreadAnalyticsDto } from "../types/index.ts";

const INTERACTIVE_CHILD = process.argv.includes("--interactive-child");
const test = INTERACTIVE_CHILD ? (((() => {}) as unknown) as typeof nodeTest) : nodeTest;

test("SignalTrendGadget renders header, window options and empty state initially", () => {
  const html = renderToStaticMarkup(
    React.createElement(SignalTrendGadget, {
      symbol: "BTC-USDC",
      botWindowSec: 3600,
      hurdleBps: 2.5,
    })
  );

  assert.match(html, /Venue deviation from baseline/);
  assert.match(html, /Consensus/);
  assert.match(html, /Leader/);
  assert.match(html, /Collecting Signal Samples…/);
  assert.match(html, /rounded-lg border border-surface-border bg-surface overflow-hidden shadow-2xs relative/);
  assert.doesNotMatch(html, /venue samples in window/);
  assert.doesNotMatch(html, /baseline = /);
});

test("SignalTrendGadget with loaded series omits venue samples footer while preserving analytics row", async (t) => {
  await t.test("series loaded -> footer omitted", () => {
    const out = execFileSync(
      process.execPath,
      [__filename, "--interactive-child"],
      { encoding: "utf8", timeout: 30000 },
    );
    assert.match(out, /INTERACTIVE-SIGNAL-TREND-OK/);
  });
});

// ── child-process harness (not executed under node:test) ────────────────────
declare const __filename: string;

async function runInteractiveChild() {
  const { Window } = await import("happy-dom");
  const { createRoot } = await import("react-dom/client");
  const { act } = await import("react");
  const { api } = await import("../services/api.ts");

  const window = new Window();
  for (const key of [
    "document", "window", "HTMLElement", "HTMLButtonElement",
    "Element", "Node", "MouseEvent", "CustomEvent", "Event",
    "SVGElement", "DocumentFragment", "MutationObserver", "getComputedStyle",
    "requestAnimationFrame", "cancelAnimationFrame",
  ]) {
    (globalThis as Record<string, unknown>)[key] =
      (window as unknown as Record<string, unknown>)[key] ??
      (globalThis as Record<string, unknown>)[key];
  }
  (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;

  const mockSeries: SignalSeriesDto = {
    symbol: "BTC-USDC",
    mode: "leader",
    window_secs: 3600,
    sample_count: 1420,
    earliest_ts_ms: Date.now() - 3600000,
    ts_ms: [Date.now() - 1800000, Date.now()],
    venues: ["binance", "hyperliquid"],
    venue_names: { binance: "Binance", hyperliquid: "Hyperliquid" },
    venue_last_ts_ms: { binance: Date.now(), hyperliquid: Date.now() },
    dev_bps: [[0, 2.5], [0, 3.1]],
    band_max_bps: [2.5, 3.1],
    band_min_bps: [0, 0],
  };

  const mockAnalytics: SpreadAnalyticsDto = {
    symbol: "BTC-USDC",
    window_secs: 3600,
    hurdle_bps: 2.5,
    venues_seen: ["binance", "hyperliquid"],
    band: {
      sample_count: 50,
      mean_bps: 2.8,
      p90_bps: 3.5,
      max_bps: 4.2,
      frac_at_or_above_hurdle: 0.75,
    } as any,
    pairs: [],
  };

  api.getSignalSeries = async () => mockSeries;
  api.getSpreadAnalytics = async () => mockAnalytics;

  const container = window.document.createElement("div");
  window.document.body.appendChild(container);
  const reactContainer = container as unknown as HTMLElement;

  const root = createRoot(reactContainer);
  await act(async () => {
    root.render(
      React.createElement(SignalTrendGadget, {
        symbol: "BTC-USDC",
        botWindowSec: 3600,
        hurdleBps: 2.5,
      }),
    );
  });

  // Give promises a tick to resolve and state to flush
  await act(async () => {
    await new Promise((r) => setTimeout(r, 50));
  });

  const html = container.innerHTML;
  if (html.includes("venue samples in window")) {
    throw new Error("Footer still rendered 'venue samples in window'");
  }
  if (html.includes("baseline = most-liquid venue") || html.includes("baseline = ")) {
    throw new Error("Footer still rendered baseline description");
  }
  if (!html.includes("band μ")) {
    throw new Error("Analytics row 'band μ' missing from DOM");
  }
  if (!html.includes("≥2.5 bps hurdle")) {
    throw new Error("Analytics row hurdle info missing from DOM");
  }

  await act(async () => root.unmount());
  process.stdout.write("INTERACTIVE-SIGNAL-TREND-OK\n");
  process.exit(0);
}

if (INTERACTIVE_CHILD) {
  runInteractiveChild().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}
