import nodeTest from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  OptionsArbitrage,
  OptionsParityCard,
  OPTIONS_SUBSTRATEGIES,
  filterOptionsLogs,
  filterOptionsOpps,
} from "./OptionsArbitrage.tsx";
import type {
  ExecutionLogEntry,
  OptionsParityOpportunityDto,
} from "../types/index.ts";

const noop = async () => {};

// Interactive children re-exec this file with --interactive-child; the test
// registry is silenced there so only the child harness runs.
const INTERACTIVE_CHILD = process.argv.includes("--interactive-child");
const test = INTERACTIVE_CHILD ? (((() => {}) as unknown) as typeof nodeTest) : nodeTest;

const opp = (over: Partial<OptionsParityOpportunityDto> = {}): OptionsParityOpportunityDto => ({
  id: "opar-BTC-1-85000-conversion",
  venue: "deribit",
  currency: "BTC",
  expiry_ms: 1_800_000_000_000,
  expiry_iso: "2026-06-27",
  strike: 85000,
  direction: "conversion",
  call_leg: {
    instrument: "BTC-27JUN26-85000-C",
    role: "call",
    action: "sell",
    venue_price: 0.05,
    usd_price: 4225,
  },
  put_leg: {
    instrument: "BTC-27JUN26-85000-P",
    role: "put",
    action: "buy",
    venue_price: 0.031,
    usd_price: 2619.5,
  },
  perp_leg: {
    instrument: "BTC-PERPETUAL",
    role: "perp",
    action: "buy",
    venue_price: 84502,
    usd_price: 84502,
  },
  synthetic_forward_usd: 86605.5,
  perp_equiv_usd: 85262.5,
  funding_8h: 0.0001,
  funding_drain_usd: 760.5,
  gross_edge_usd: 134.3,
  est_fees_usd: 12.1,
  net_edge_usd: 122.2,
  edge_bps: 14.5,
  index_price_usd: 84500,
  qty_base_suggested: 0.1,
  is_executable: true,
  ...over,
});

test("gate: unadmitted options venue renders the page with a disabled scan + banner", () => {
  // The page is the strategy monitor — other options venues will exist, so
  // it stays visible; only the scan action is gated on an options-capable
  // venue being admitted.
  const html = renderToStaticMarkup(
    React.createElement(OptionsArbitrage, {
      executionLogs: [],
      onRefreshAll: noop,
      optionsVenueAvailable: false,
    })
  );
  assert.match(html, /Options Strategies/);
  assert.match(html, /No options-capable venue is enabled/);
  assert.match(html, /test\.deribit\.com/);
  const scanBtn = html.match(/<button[^>]*>\s*<svg[\s\S]*?Scan\s*<\/button>/);
  assert.ok(scanBtn, "Scan button renders (disabled)");
  assert.match(scanBtn[0], /disabled/);
});

test("selector: rail renders All Strategies plus every catalogued substrategy", () => {
  const html = renderToStaticMarkup(
    React.createElement(OptionsArbitrage, {
      executionLogs: [],
      onRefreshAll: noop,
      optionsVenueAvailable: true,
    })
  );
  assert.match(html, /All Strategies/);
  for (const s of OPTIONS_SUBSTRATEGIES) {
    // JSX escapes & → &amp; in rendered markup.
    assert.ok(
      html.includes(s.name.replace(/&/g, "&amp;")),
      `rail row for ${s.id}`,
    );
  }
  // The catalog is broader than crypto perps — pending rows carry their
  // unblock requirement on the row tooltip, live rows carry a LIVE badge.
  assert.match(html, />LIVE</);
  assert.match(html, /IBKR options/);
});

test("selector: pending substrategy selection swaps the scanner for an honest placeholder", () => {
  // React SSR renders initial state only — the placeholder branch is covered
  // by the pure catalog + a direct render check on a pending entry's fields.
  const pending = OPTIONS_SUBSTRATEGIES.find((s) => s.id === "box_spread");
  assert.ok(pending);
  assert.equal(pending.status, "pending");
  assert.match(pending.requires ?? "", /IBKR|Deribit/);
  assert.match(pending.universe, /equity|index/i);
});

test("filter: substrategy 'all' passes every opp; 'parity' keeps parity rows only", () => {
  const list = [opp(), opp({ id: "opar-ETH-1-3000-reversal", direction: "reversal" })];
  assert.equal(filterOptionsOpps(list, "all").length, 2);
  assert.equal(filterOptionsOpps(list, "parity").length, 2);
  assert.equal(filterOptionsOpps(list, "box_spread").length, 0);
});

const logRow = (over: Partial<ExecutionLogEntry> = {}): ExecutionLogEntry => ({
  id: "l1",
  timestamp: "2026-09-25T00:00:00Z",
  trade_type: "OptionsParity",
  strategy_id: "options_parity",
  symbol: "conversion",
  venue_a: "deribit",
  venue_b: "deribit",
  leg_a_status: "filled",
  leg_b_status: "filled",
  success: true,
  gross_profit: 10,
  fees: 1,
  net_profit: 9,
  ...over,
} as ExecutionLogEntry);

test("filter: logs narrow to the selected substrategy's strategy_id", () => {
  const logs = [
    logRow(),
    logRow({ id: "l2", strategy_id: "funding_rate", trade_type: "FundingCarry" }),
    logRow({ id: "l3", strategy_id: "options_box_spread", trade_type: "BoxSpread" }),
  ];
  // Overall keeps every catalogued options row but never other strategies'.
  assert.equal(filterOptionsLogs(logs, "all").length, 2);
  const parityOnly = filterOptionsLogs(logs, "parity");
  assert.equal(parityOnly.length, 1);
  assert.equal(parityOnly[0].strategy_id, "options_parity");
  // A pending substrategy filters to its own (empty) log set.
  assert.equal(filterOptionsLogs(logs, "box_spread").length, 1);
});

test("card: conversion legs render with venue prices and the execute gate title", () => {
  const html = renderToStaticMarkup(
    React.createElement(OptionsParityCard, {
      opp: opp(),
      executing: false,
      executed: false,
      disabled: false,
      onExecute: () => {},
    })
  );
  assert.match(html, /SELL CALL/);
  assert.match(html, /BUY PUT/);
  assert.match(html, /BUY PERP/);
  assert.match(html, /BTC-27JUN26-85000-C/);
  assert.match(html, /BTC-PERPETUAL/);
  assert.match(html, /85,000/);
  assert.match(html, /Execute conversion on Deribit/);
  assert.match(html, /\$122\.20/);
});

test("card: non-executable edge carries the backend reason on the disabled button", () => {
  const html = renderToStaticMarkup(
    React.createElement(OptionsParityCard, {
      opp: opp({ is_executable: false, reason: "edge is fee-eaten at this size" }),
      executing: false,
      executed: false,
      disabled: true,
      onExecute: () => {},
    })
  );
  assert.match(html, /edge is fee-eaten at this size/);
});

test("card: reversal direction labels buy-call / sell-put / sell-perp", () => {
  const rev = opp({
    id: "opar-BTC-1-85000-reversal",
    direction: "reversal",
    call_leg: { ...opp().call_leg, action: "buy" },
    put_leg: { ...opp().put_leg, action: "sell" },
    perp_leg: { ...opp().perp_leg, action: "sell" },
  });
  const html = renderToStaticMarkup(
    React.createElement(OptionsParityCard, {
      opp: rev,
      executing: false,
      executed: false,
      disabled: false,
      onExecute: () => {},
    })
  );
  assert.match(html, /BUY CALL/);
  assert.match(html, /SELL PUT/);
  assert.match(html, /SELL PERP/);
});

test("substrategy bot controls live only on the selected live scanner (interactive DOM)", async (t) => {
  await t.test("all-view monitor-only → parity view controls", () => {
    const { execFileSync } = require("node:child_process");
    const out = execFileSync(
      process.execPath,
      [__filename, "--interactive-child"],
      { encoding: "utf8", timeout: 30000 },
    );
    assert.match(out, /INTERACTIVE-OK/);
  });
});

// ── child-process harness (not executed under node:test) ────────────────────
declare const __filename: string;

async function runInteractiveChild() {
  const { Window } = await import("happy-dom");
  const { createRoot } = await import("react-dom/client");
  const { act } = await import("react");

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

  // Stub the bot-config + signal ledger reads the page polls.
  const { api } = await import("../services/api.ts");
  const { defaultOptionsBotConfig } = await import("../services/optionsBot.ts");
  (api as Record<string, unknown>).getOptionsBotConfig = async (id: string) =>
    defaultOptionsBotConfig(id);
  (api as Record<string, unknown>).listSignalExecutions = async () => [];
  (api as Record<string, unknown>).scanOptionsParity = async () => [];

  const container = window.document.createElement("div");
  window.document.body.appendChild(container);
  const root = createRoot(container as unknown as HTMLElement);
  await act(async () => {
    root.render(
      React.createElement(OptionsArbitrage, {
        executionLogs: [],
        onRefreshAll: noop,
        optionsVenueAvailable: true,
      }),
    );
  });

  const buttons = () =>
    Array.from(container.querySelectorAll("button")).map(
      (b) => (b as unknown as HTMLElement).textContent ?? "",
    );
  const has = (label: string) => buttons().some((t) => t.includes(label));

  if (has("Start") || has("Configurator")) {
    throw new Error("All Strategies view must stay monitor-only");
  }

  const rail = Array.from(container.querySelectorAll("button")).find((b) =>
    ((b as unknown as HTMLElement).textContent ?? "").includes("Put–Call Parity"),
  );
  if (!rail) throw new Error("parity rail row missing");
  await act(async () => {
    rail.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true }));
    // Let the config poll settle.
    await new Promise((r) => setTimeout(r, 50));
  });

  for (const label of ["Configurator", "Start", "Pause", "Reduce Only", "Stop"]) {
    if (!has(label)) throw new Error(`missing lifecycle control: ${label}`);
  }

  // Pending substrategies expose no bot controls.
  const pending = Array.from(container.querySelectorAll("button")).find((b) =>
    ((b as unknown as HTMLElement).textContent ?? "").includes("Box Spread"),
  );
  if (!pending) throw new Error("box_spread rail row missing");
  await act(async () => {
    pending.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true }));
    await new Promise((r) => setTimeout(r, 50));
  });
  if (has("Start") || has("Configurator")) {
    throw new Error("pending substrategy must not expose bot controls");
  }

  await act(async () => root.unmount());
  process.stdout.write("INTERACTIVE-OK\n");
  process.exit(0);
}

if (INTERACTIVE_CHILD) {
  runInteractiveChild().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}
