import nodeTest from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import {
  BotLifecycleControls,
  BotPhaseBadge,
  resolveBotStatus,
  type StrategyBotStatus,
} from "./BotLifecycleControls.tsx";
import { getBotControlAvailability, type BotPhase } from "../hooks/useBotLifecycle.ts";
import { LiveMatrix } from "./LiveMatrix.tsx";
import { SignalTrendGadget } from "./SignalTrendGadget.tsx";
import { EngineSettings } from "./EngineSettings.tsx";
import { getMatrixCellVisuals } from "../services/matrixQuant.ts";
import type {
  MatrixCellDto,
  SignalSeriesDto,
  SpreadAnalyticsDto,
  SystemStatusDto,
  VaultStatus,
} from "../types/index.ts";

const INTERACTIVE_CHILD = process.argv.includes("--interactive-child");
const test = INTERACTIVE_CHILD ? (((() => {}) as unknown) as typeof nodeTest) : nodeTest;

const noop = () => {};

function getComponentSource(filename: string): string {
  const candidates = [
    path.resolve(process.cwd(), "src/components", filename),
    path.resolve(__dirname, filename),
    path.resolve(__dirname, "../../src/components", filename),
    path.resolve(__dirname, "../../../src/components", filename),
  ];
  for (const c of candidates) {
    if (fs.existsSync(c)) return fs.readFileSync(c, "utf8");
  }
  throw new Error(`Component source file not found: ${filename}`);
}

// ============================================================================
// REQUIREMENT R3: BotLifecycleControls Outer Wrapper Symmetric Padding & States
// ============================================================================

test("R3 [Adversarial]: BotLifecycleControls outer border padding is strictly symmetric p-1 (4px all sides)", () => {
  const html = renderToStaticMarkup(
    React.createElement(BotLifecycleControls, {
      phase: "idle",
      hasOpenPositions: false,
      onStart: noop,
      onPause: noop,
      onReduceOnly: noop,
      onStop: noop,
    })
  );

  // Extract the outer wrapper <div> class attribute
  const wrapperClassMatch = html.match(/^<div\s+class="([^"]*)"/);
  assert.ok(wrapperClassMatch, "Must have outer wrapper div");
  const outerClasses = wrapperClassMatch[1];

  // 1. Must contain p-1
  assert.match(outerClasses, /\bp-1\b/, "Outer wrapper must have p-1 padding");

  // 2. Must NOT contain asymmetric padding classes on the wrapper
  assert.doesNotMatch(outerClasses, /\bpx-\d+\b/, "Outer wrapper must not contain asymmetric px- padding");
  assert.doesNotMatch(outerClasses, /\bpy-\d+\b/, "Outer wrapper must not contain asymmetric py- padding");
  assert.doesNotMatch(outerClasses, /\bpt-\d+\b/, "Outer wrapper must not contain asymmetric pt- padding");
  assert.doesNotMatch(outerClasses, /\bpb-\d+\b/, "Outer wrapper must not contain asymmetric pb- padding");
  assert.doesNotMatch(outerClasses, /\bpl-\d+\b/, "Outer wrapper must not contain asymmetric pl- padding");
  assert.doesNotMatch(outerClasses, /\bpr-\d+\b/, "Outer wrapper must not contain asymmetric pr- padding");
  assert.doesNotMatch(outerClasses, /px-2\s+py-1/, "Outer wrapper must not contain legacy px-2 py-1");

  // 3. Verify exact wrapper styling classes
  assert.equal(
    outerClasses,
    "flex items-center space-x-1.5 p-1 bg-surface border border-surface-border rounded-lg shadow-xs",
    "Outer wrapper must have standard flex, border, bg-surface, and p-1 classes"
  );

  // 4. Verify inner button container spacing
  assert.match(
    html,
    /class="flex items-center gap-1"/,
    "Inner button group must have gap-1"
  );
});

test("R3 [Adversarial]: Inner buttons maintain uniform button padding and sizing across all states", () => {
  const phases: BotPhase[] = ["idle", "running", "paused", "reduce_only", "stopping"];

  for (const phase of phases) {
    const html = renderToStaticMarkup(
      React.createElement(BotLifecycleControls, {
        phase,
        hasOpenPositions: true,
        onStart: noop,
        onPause: noop,
        onReduceOnly: noop,
        onStop: noop,
      })
    );

    // Every button rendered must have px-2 py-2 rounded-lg border text-xs font-mono font-bold
    const buttonMatches = html.match(/<button[^>]*class="([^"]*)"/g);
    assert.ok(buttonMatches && buttonMatches.length >= 3, `Expected at least 3 buttons for phase ${phase}`);

    for (const btnTag of buttonMatches) {
      assert.match(btnTag, /\bpx-2\b/, "Button must have px-2");
      assert.match(btnTag, /\bpy-2\b/, "Button must have py-2");
      assert.match(btnTag, /\brounded-lg\b/, "Button must have rounded-lg");
      assert.match(btnTag, /\btext-xs\b/, "Button must have text-xs");
      assert.match(btnTag, /\bfont-mono\b/, "Button must have font-mono");
    }
  }
});

test("R3 [Adversarial]: BotPhaseBadge renders all 4 standardized statuses without 'Engine'", () => {
  const statuses: StrategyBotStatus[] = ["Active", "Paused", "Reducing", "Stopped"];
  for (const s of statuses) {
    const html = renderToStaticMarkup(React.createElement(BotPhaseBadge, { status: s }));
    assert.match(html, new RegExp(`>${s}<`));
    assert.doesNotMatch(html, /Engine/i);
  }
});

test("R3 [Adversarial]: Exhaustive lifecycle state machine and button availability matrix", () => {
  // Test matrix covers all valid combinations of phase, hasOpenPositions, disabled, allowStartWithOpenPositions
  const scenarios: Array<{
    phase: BotPhase;
    hasOpenPositions: boolean;
    disabled?: boolean;
    allowStart?: boolean;
    reduceOnlyAvailable?: boolean;
    expected: {
      start: boolean;
      pause: boolean;
      reduceOnly: boolean;
      stop: boolean;
      status: StrategyBotStatus;
    };
  }> = [
    // Idle states
    {
      phase: "idle",
      hasOpenPositions: false,
      expected: { start: true, pause: false, reduceOnly: false, stop: false, status: "Stopped" },
    },
    {
      phase: "idle",
      hasOpenPositions: true,
      allowStart: false,
      expected: { start: false, pause: false, reduceOnly: false, stop: true, status: "Stopped" },
    },
    {
      phase: "idle",
      hasOpenPositions: true,
      allowStart: true,
      expected: { start: true, pause: false, reduceOnly: false, stop: true, status: "Stopped" },
    },
    // Running states
    {
      phase: "running",
      hasOpenPositions: false,
      expected: { start: false, pause: true, reduceOnly: false, stop: true, status: "Active" },
    },
    {
      phase: "running",
      hasOpenPositions: true,
      expected: { start: false, pause: true, reduceOnly: true, stop: true, status: "Active" },
    },
    // Paused states
    {
      phase: "paused",
      hasOpenPositions: false,
      expected: { start: true, pause: false, reduceOnly: false, stop: true, status: "Paused" },
    },
    {
      phase: "paused",
      hasOpenPositions: true,
      allowStart: false,
      expected: { start: false, pause: false, reduceOnly: true, stop: true, status: "Paused" },
    },
    {
      phase: "paused",
      hasOpenPositions: true,
      allowStart: true,
      expected: { start: true, pause: false, reduceOnly: true, stop: true, status: "Paused" },
    },
    // Reduce Only states
    {
      phase: "reduce_only",
      hasOpenPositions: true,
      expected: { start: false, pause: false, reduceOnly: false, stop: true, status: "Reducing" },
    },
    {
      phase: "reduce_only",
      hasOpenPositions: false,
      expected: { start: false, pause: false, reduceOnly: false, stop: true, status: "Reducing" },
    },
    // Stopping states
    {
      phase: "stopping",
      hasOpenPositions: true,
      expected: { start: false, pause: false, reduceOnly: false, stop: false, status: "Reducing" },
    },
    // Disabled safety override checks
    {
      phase: "running",
      hasOpenPositions: true,
      disabled: true,
      // Pause, ReduceOnly, and Stop remain armed even when entries are disabled (safety invariant)
      expected: { start: false, pause: true, reduceOnly: true, stop: true, status: "Active" },
    },
    {
      phase: "idle",
      hasOpenPositions: true,
      disabled: true,
      // Emergency stop remains available to flatten open positions
      expected: { start: false, pause: false, reduceOnly: false, stop: true, status: "Stopped" },
    },
    // reduceOnlyAvailable = false (e.g. strategy does not support reduce-only)
    {
      phase: "running",
      hasOpenPositions: true,
      reduceOnlyAvailable: false,
      expected: { start: false, pause: true, reduceOnly: false, stop: true, status: "Active" },
    },
  ];

  for (const sc of scenarios) {
    const avail = getBotControlAvailability(
      sc.phase,
      sc.hasOpenPositions,
      sc.disabled ?? false,
      sc.reduceOnlyAvailable ?? true,
      sc.allowStart ?? false
    );

    assert.equal(
      avail.start,
      sc.expected.start,
      `Start mismatch for phase=${sc.phase} pos=${sc.hasOpenPositions} dis=${sc.disabled} allow=${sc.allowStart}`
    );
    assert.equal(
      avail.pause,
      sc.expected.pause,
      `Pause mismatch for phase=${sc.phase} pos=${sc.hasOpenPositions} dis=${sc.disabled}`
    );
    assert.equal(
      avail.reduceOnly,
      sc.expected.reduceOnly,
      `ReduceOnly mismatch for phase=${sc.phase} pos=${sc.hasOpenPositions} dis=${sc.disabled}`
    );
    assert.equal(
      avail.stop,
      sc.expected.stop,
      `Stop mismatch for phase=${sc.phase} pos=${sc.hasOpenPositions} dis=${sc.disabled}`
    );

    const st = resolveBotStatus(sc.phase, false);
    assert.equal(st, sc.expected.status, `Status mismatch for phase=${sc.phase}`);
  }
});

test("R3 [Adversarial]: All strategy components in codebase import and render BotLifecycleControls with p-1", () => {
  const strategyFiles = [
    "PredictionHub.tsx",
    "FundingRadar.tsx",
    "OpportunityRadar.tsx",
    "OptionsArbitrage.tsx",
    "TriangularArbitrage.tsx",
    "LiveMatrix.tsx",
    "SpotArbitrageBot.tsx",
  ];

  for (const filename of strategyFiles) {
    const content = getComponentSource(filename);

    assert.ok(
      content.includes("<BotLifecycleControls"),
      `${filename} must render <BotLifecycleControls`
    );

    // Verify no strategy file has custom padding override wrapper around BotLifecycleControls
    assert.doesNotMatch(
      content,
      /<div[^>]*\b(p-[02-9]|px-[1-9]|py-[2-9])\b[^>]*>\s*<BotLifecycleControls/s,
      `${filename} must not wrap BotLifecycleControls in non-standard outer padding`
    );
  }
});

// ============================================================================
// REQUIREMENT R4: LiveMatrix Cell Delta Badges & SignalTrendGadget Footers
// ============================================================================

test("R4 [Adversarial]: LiveMatrix component renders without TrendingUp/TrendingDown icons in DOM", () => {
  const html = renderToStaticMarkup(
    React.createElement(LiveMatrix, {
      symbol: null,
      orderbooks: [],
      opportunities: [],
      onRefresh: noop,
      onExecuteTrade: async () => {},
      loading: false,
      isPaused: false,
    })
  );
  assert.match(html, /Cross-Venue|No Ticker Selected/);
  assert.doesNotMatch(html, /TrendingUp/);
  assert.doesNotMatch(html, /TrendingDown/);
});

test("R4 [Adversarial]: LiveMatrix cell delta badges render numerical delta string with Δ + or Δ - without any Trending icons or arrow symbols", () => {
  const content = getComponentSource("LiveMatrix.tsx");

  // 1. TrendingUp and TrendingDown must not be imported
  assert.doesNotMatch(content, /TrendingUp/, "LiveMatrix.tsx must not import or reference TrendingUp");
  assert.doesNotMatch(content, /TrendingDown/, "LiveMatrix.tsx must not import or reference TrendingDown");

  // 2. No unicode arrow symbols in matrix cell rendering
  assert.doesNotMatch(
    content,
    /Δ.*[↗↘↑↓]/,
    "LiveMatrix.tsx delta badge must not contain arrow characters"
  );

  // 3. Exact delta badge rendering template in LiveMatrix
  assert.match(
    content,
    /Δ\s*\{cell\.deltaBps\s*>\s*0\s*\?\s*"\+"\s*:\s*""\}\s*\{cell\.deltaBps\.toFixed\(1\)\}/,
    "LiveMatrix must render Δ followed by optional + and formatted decimal delta"
  );
});

test("R4 [Adversarial]: Matrix cell delta badge format across positive, negative, zero, and extreme values", () => {
  // Test simulated rendering helper directly reflecting LiveMatrix.tsx cell delta badge
  function renderDeltaBadge(cell: {
    deltaBps: number;
    costReady?: boolean;
    baselineReady?: boolean;
    sampleCount?: number;
  }): string | null {
    if (cell.costReady === false || cell.baselineReady === false || cell.sampleCount === 0) {
      return null;
    }
    return `Δ ${cell.deltaBps > 0 ? "+" : ""}${cell.deltaBps.toFixed(1)}`;
  }

  // 1. Positive values
  assert.equal(renderDeltaBadge({ deltaBps: 5.23 }), "Δ +5.2");
  assert.equal(renderDeltaBadge({ deltaBps: 0.1 }), "Δ +0.1");
  assert.equal(renderDeltaBadge({ deltaBps: 120.0 }), "Δ +120.0");

  // 2. Negative values
  assert.equal(renderDeltaBadge({ deltaBps: -4.89 }), "Δ -4.9");
  assert.equal(renderDeltaBadge({ deltaBps: -0.1 }), "Δ -0.1");
  assert.equal(renderDeltaBadge({ deltaBps: -999.5 }), "Δ -999.5");

  // 3. Zero and boundary values
  assert.equal(renderDeltaBadge({ deltaBps: 0 }), "Δ 0.0");
  assert.equal(renderDeltaBadge({ deltaBps: -0.0 }), "Δ 0.0");

  // 4. Missing economics or baseline suppresses badge
  assert.equal(renderDeltaBadge({ deltaBps: 5.0, costReady: false }), null);
  assert.equal(renderDeltaBadge({ deltaBps: 5.0, baselineReady: false }), null);
  assert.equal(renderDeltaBadge({ deltaBps: 5.0, sampleCount: 0 }), null);
});

test("R4 [Adversarial]: getMatrixCellVisuals assigns proper delta badge background classes", () => {
  const dummyCell = (deltaBps: number): MatrixCellDto =>
    ({
      rowId: "binance",
      colId: "hyperliquid",
      grossSpreadBps: 10,
      netSpreadBps: 5,
      deltaBps,
      costReady: true,
      baselineReady: true,
      sampleCount: 100,
      closingFrictionBps: 2,
      isDiagonal: false,
      pairReady: true,
    } as any);

  const posVisual = getMatrixCellVisuals(dummyCell(4.5), 2.0);
  assert.match(posVisual.deltaBadgeBg, /bg-emerald-100 text-emerald-800/, "Positive delta must have emerald styling");

  const negVisual = getMatrixCellVisuals(dummyCell(-3.2), 2.0);
  assert.match(negVisual.deltaBadgeBg, /bg-rose-100 text-rose-800/, "Negative delta must have rose styling");

  const zeroVisual = getMatrixCellVisuals(dummyCell(0.0), 2.0);
  assert.match(zeroVisual.deltaBadgeBg, /bg-slate-100 text-slate-600/, "Zero delta must have neutral slate styling");
});

test("R4 [Adversarial]: SignalTrendGadget source code omits venue samples footer and baseline descriptions", () => {
  const content = getComponentSource("SignalTrendGadget.tsx");

  // 1. Venue samples footer line removed
  assert.doesNotMatch(content, /venue samples in window/, "Must omit 'venue samples in window'");
  assert.doesNotMatch(content, /historyLabel/, "Must omit historyLabel");
  assert.doesNotMatch(content, /history recorded/, "Must omit 'history recorded'");

  // 2. Baseline descriptions removed from footer
  assert.doesNotMatch(content, /baseline\s*=\s*most-liquid venue/, "Must omit 'baseline = most-liquid venue'");
  assert.doesNotMatch(content, /baseline\s*=\s*volume-weighted consensus/, "Must omit 'baseline = volume-weighted consensus'");
  assert.doesNotMatch(content, /pairwise spread needs ≥2 venues/, "Must omit 'pairwise spread needs ≥2 venues'");

  // 3. Analytics row preserved
  assert.match(content, /band μ/, "Must preserve band mean (μ) in analytics row");
  assert.match(content, /p90/, "Must preserve p90 in analytics row");
  assert.match(content, /max/, "Must preserve max in analytics row");
  assert.match(content, /bps hurdle/, "Must preserve hurdle information in analytics row");
});

// ============================================================================
// REQUIREMENT R7: EngineSettings Security Model Box Styling & MFA Copy Cleanups
// ============================================================================

test("R7 [Adversarial]: EngineSettings Security Model card uses neutral slate palette matching Legal Disclaimer", () => {
  const content = getComponentSource("EngineSettings.tsx");

  // 1. Security Model box styling: bg-slate-50 border border-surface-border rounded-lg p-3.5 shadow-2xs space-y-2
  assert.match(
    content,
    /<div className="bg-slate-50 border border-surface-border rounded-lg p-3\.5 shadow-2xs space-y-2">\s*<div className="flex items-center space-x-1\.5 font-bold text-slate-800 text-xs font-mono">\s*<ShieldCheck className="w-3\.5 h-3\.5 text-slate-500"/,
    "Security Model box must use bg-slate-50 and text-slate-500 ShieldCheck icon"
  );

  // 2. Must not use green / emerald styling
  assert.doesNotMatch(
    content,
    /<ShieldCheck[^>]*text-emerald-600/,
    "Security Model ShieldCheck icon must not use text-emerald-600"
  );
  assert.doesNotMatch(
    content,
    /<ShieldCheck[^>]*text-green-/,
    "Security Model ShieldCheck icon must not use text-green-*"
  );

  // 3. Verify Legal & Financial Risk Disclaimer uses matching bg-slate-50 and text-slate-500
  assert.match(
    content,
    /<div className="p-4 bg-slate-50 border border-surface-border rounded-lg space-y-2 text-xs font-mono text-slate-600 leading-relaxed">\s*<div className="flex items-center space-x-1\.5 font-bold text-slate-800 text-xs">\s*<Scale className="w-3\.5 h-3\.5 text-slate-500"/,
    "Legal & Financial Risk Disclaimer must use bg-slate-50 and text-slate-500 Scale icon"
  );
});

test("R7 [Adversarial]: EngineSettings completely omits MFA unlock explanation paragraph and authenticator app examples", () => {
  const content = getComponentSource("EngineSettings.tsx");

  // 1. MFA unlock explanation paragraph completely absent
  assert.doesNotMatch(
    content,
    /MFA gates the unlock path in this app/,
    "Must not contain 'MFA gates the unlock path in this app'"
  );
  assert.doesNotMatch(
    content,
    /an operator needs the code, not just the password/,
    "Must not contain 'an operator needs the code, not just the password'"
  );
  assert.doesNotMatch(
    content,
    /The TOTP secret is stored inside the encrypted vault/,
    "Must not contain 'The TOTP secret is stored inside the encrypted vault'"
  );
  assert.doesNotMatch(
    content,
    /it does not add encryption strength against theft/,
    "Must not contain 'it does not add encryption strength against theft'"
  );

  // 2. Authenticator app examples completely absent
  assert.doesNotMatch(
    content,
    /\(Google Authenticator, 1Password, Authy\)/,
    "Must not contain '(Google Authenticator, 1Password, Authy)'"
  );
  assert.doesNotMatch(content, /Google Authenticator/, "Must not contain 'Google Authenticator'");
  assert.doesNotMatch(content, /1Password/, "Must not contain '1Password'");
  assert.doesNotMatch(content, /Authy/, "Must not contain 'Authy'");

  // 3. Clean TOTP copy must be present
  assert.match(
    content,
    /Optionally require a 6-digit TOTP code at vault unlock/,
    "Must contain clean TOTP description"
  );
});

// ============================================================================
// Interactive DOM Sub-process Tests (DOM assertions via Happy-DOM)
// ============================================================================

test("Interactive DOM verification: LiveMatrix cell deltas, SignalTrendGadget footer, and EngineSettings Security tab", async (t) => {
  await t.test("run interactive child process", () => {
    const out = execFileSync(
      process.execPath,
      [__filename, "--interactive-child"],
      { encoding: "utf8", timeout: 35000 }
    );
    assert.match(out, /CHALLENGER-R3-R4-R7-ALL-OK/);
  });
});

declare const __filename: string;

async function runInteractiveChild() {
  const { Window } = await import("happy-dom");
  const { createRoot } = await import("react-dom/client");
  const { act } = await import("react");
  const { api } = await import("../services/api.ts");

  const window = new Window();
  for (const key of [
    "document",
    "window",
    "HTMLElement",
    "HTMLButtonElement",
    "Element",
    "Node",
    "MouseEvent",
    "CustomEvent",
    "Event",
    "SVGElement",
    "DocumentFragment",
    "MutationObserver",
    "getComputedStyle",
    "requestAnimationFrame",
    "cancelAnimationFrame",
  ]) {
    (globalThis as Record<string, unknown>)[key] =
      (window as unknown as Record<string, unknown>)[key] ??
      (globalThis as Record<string, unknown>)[key];
  }
  (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;

  // --------------------------------------------------------------------------
  // 1. SignalTrendGadget Interactive Verification
  // --------------------------------------------------------------------------
  const mockSeries: SignalSeriesDto = {
    symbol: "BTC-USDC",
    mode: "leader",
    window_secs: 3600,
    sample_count: 5000,
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
      sample_count: 85,
      mean_bps: 3.14,
      p90_bps: 4.5,
      max_bps: 6.2,
      frac_at_or_above_hurdle: 0.88,
    } as any,
    pairs: [],
  };

  api.getSignalSeries = async () => mockSeries;
  api.getSpreadAnalytics = async () => mockAnalytics;

  const container1 = window.document.createElement("div");
  window.document.body.appendChild(container1);
  const root1 = createRoot(container1 as unknown as HTMLElement);

  await act(async () => {
    root1.render(
      React.createElement(SignalTrendGadget, {
        symbol: "BTC-USDC",
        botWindowSec: 3600,
        hurdleBps: 2.5,
      })
    );
  });

  await act(async () => {
    await new Promise((r) => setTimeout(r, 60));
  });

  const trendHtml = container1.innerHTML;
  if (trendHtml.includes("venue samples in window")) {
    throw new Error("SignalTrendGadget unexpectedly rendered 'venue samples in window'");
  }
  if (trendHtml.includes("baseline = most-liquid venue") || trendHtml.includes("baseline = ")) {
    throw new Error("SignalTrendGadget unexpectedly rendered baseline footer text");
  }
  if (!trendHtml.includes("band μ 3.1")) {
    throw new Error("SignalTrendGadget failed to render preserved analytics row 'band μ 3.1'");
  }
  if (!trendHtml.includes("88% of polls")) {
    throw new Error("SignalTrendGadget failed to render preserved hurdle percentage '88% of polls'");
  }

  await act(async () => root1.unmount());
  container1.remove();

  // --------------------------------------------------------------------------
  // 2. EngineSettings Interactive Verification
  // --------------------------------------------------------------------------
  const mockSystemStatus: SystemStatusDto = {
    trading_mode: "testnet",
    circuit_breaker_tripped: false,
    killswitch_tripped: false,
    live_unlocked: true,
    venues: [],
  } as any;

  const mockVaultStatus: VaultStatus = {
    initialized: true,
    unlocked: true,
    mfa_enabled: false,
    password_hash_exists: true,
  } as any;

  api.getSystemStatus = async () => mockSystemStatus;
  api.getVaultStatus = async () => mockVaultStatus;
  (api as any).getExchangeAccounts = async () => [];
  api.getStrategyParams = async () => ({} as any);

  const container2 = window.document.createElement("div");
  window.document.body.appendChild(container2);
  const root2 = createRoot(container2 as unknown as HTMLElement);

  await act(async () => {
    root2.render(
      React.createElement(EngineSettings, {
        systemStatus: mockSystemStatus,
        onClose: noop,
      })
    );
  });

  // Switch to Security tab
  const buttons = Array.from(container2.querySelectorAll("button"));
  const secTabBtn = buttons.find((b) => b.textContent?.includes("Security"));
  if (!secTabBtn) throw new Error("Security tab button not found in EngineSettings");

  await act(async () => {
    secTabBtn.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true }));
  });

  const secHtml = container2.innerHTML;

  // Verify Security Model box neutral styling
  if (!secHtml.includes("bg-slate-50 border border-surface-border rounded-lg p-3.5 shadow-2xs space-y-2")) {
    throw new Error("Security Model box did not render with bg-slate-50 styling in DOM");
  }
  if (!secHtml.includes("text-slate-500")) {
    throw new Error("Security Model ShieldCheck icon missing text-slate-500 in DOM");
  }
  if (secHtml.includes("text-emerald-600")) {
    throw new Error("Security Model icon still contains text-emerald-600 in DOM");
  }

  // Verify copy omissions
  if (secHtml.includes("MFA gates the unlock path in this app")) {
    throw new Error("EngineSettings still contains MFA explanation paragraph");
  }
  if (secHtml.includes("Google Authenticator") || secHtml.includes("1Password") || secHtml.includes("Authy")) {
    throw new Error("EngineSettings still contains authenticator app examples");
  }
  if (!secHtml.includes("Optionally require a 6-digit TOTP code at vault unlock")) {
    throw new Error("EngineSettings missing clean TOTP copy");
  }

  await act(async () => root2.unmount());
  container2.remove();

  process.stdout.write("CHALLENGER-R3-R4-R7-ALL-OK\n");
  process.exit(0);
}

if (INTERACTIVE_CHILD) {
  runInteractiveChild().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}
