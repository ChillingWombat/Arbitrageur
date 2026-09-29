import nodeTest from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  TradedOpportunityDetailModal,
  TradedOpportunitiesTable,
  filterTradedOpps,
  statusCategory,
  strategyCategory,
} from "./TradedOpportunitiesGadget.tsx";
import type { AtomicTransactionDto, TradedOpportunityDto } from "../types/index.ts";

const tx = (
  id: string,
  role: AtomicTransactionDto["role"],
  venue: string,
  orderSide: "buy" | "sell",
  side: "long" | "short",
  filled: number,
  intended: number,
  fillPrice: number | null,
): AtomicTransactionDto => ({
  legId: id,
  signalId: role === "entry" ? "sig-1-aa" : "flat-sig-1-aa-0001",
  role,
  venue,
  accountId: `acct-${venue}`,
  side,
  orderSide,
  symbol: "BTC-USDC",
  intendedQty: intended,
  filledQty: filled,
  intendedPrice: 100,
  fillPrice,
  feeUsd: 0.5,
  unitScale: 1,
  venueOrderId: `oid-${id}`,
  clientOrderId: `cid-${id}`,
  fillState: filled <= 0 ? "no_fill" : filled < intended ? "partial" : "filled",
  createdAt: "2026-01-01T00:00:00Z",
  updatedAt: "2026-01-01T00:00:00Z",
});

// In `--interactive-child` mode this file re-executes as a plain script: the
// DOM mount lives at the bottom and tests must not re-register (or they'd
// recurse into another child). `test` becomes a no-op there.
const INTERACTIVE_CHILD = process.argv.includes("--interactive-child");
const test = INTERACTIVE_CHILD ? (((() => {}) as unknown) as typeof nodeTest) : nodeTest;

const opp = (over: Partial<TradedOpportunityDto> = {}): TradedOpportunityDto => ({
  signalId: "sig-1-aa",
  strategyId: "futures",
  symbol: "BTC-USDC",
  environment: "testnet",
  status: "closed",
  routeId: null,
  expectedNetUsd: 4.5,
  note: null,
  venueLong: "hyperliquid",
  venueShort: "lighter",
  createdAt: "2026-01-01T00:00:00Z",
  closedAt: "2026-01-01T01:00:00Z",
  lotId: "spatial-sig-1-aa",
  lotStatus: "closed",
  entrySpreadBps: 12.5,
  closeSpreadBps: 3.0,
  realizedPnlUsd: 0.7,
  realizedPnlBps: 0.64,
  legNetFlowUsd: 0.7,
  totalFeesUsd: 1.8,
  childCount: 1,
  transactions: [
    tx("l1", "entry", "hyperliquid", "buy", "long", 1, 1, 100),
    tx("l2", "entry", "lighter", "sell", "short", 1, 1, 101),
    tx("l3", "flatten", "hyperliquid", "sell", "long", 1, 1, 102),
    tx("l4", "flatten", "lighter", "buy", "short", 0.5, 1, 100.5),
  ],
  ...over,
});

test("table renders every traded opportunity with status and realized PnL", () => {
  const html = renderToStaticMarkup(
    React.createElement(TradedOpportunitiesTable, {
      opps: [opp(), opp({ signalId: "sig-2-bb", status: "failed", realizedPnlUsd: -2.14, lotStatus: null })],
      failed: false,
      onSelect: () => {},
    }),
  );
  assert.match(html, /BTC-USDC/);
  assert.match(html, />Closed</);
  assert.match(html, />Failed</);
  assert.match(html, /\+\$0\.70/);
  assert.match(html, /-\$2\.14/);
  assert.match(html, /hyperliquid ↔ lighter/);
  // Each row exposes a real focusable button that opens the detail modal.
  assert.equal(html.match(/aria-label="Open opportunity details/g)?.length, 2);
});

test("strategyCategory collapses options substrategies under Options", () => {
  assert.equal(strategyCategory("options_parity"), "Options");
  assert.equal(strategyCategory("options_box_spread"), "Options");
  assert.equal(strategyCategory("options_jelly_roll"), "Options");
  assert.equal(strategyCategory("options_convexity"), "Options");
  assert.equal(strategyCategory("funding"), "Funding Rate");
  assert.equal(strategyCategory("futures"), "Simple Spread");
  // Unknown future ids surface verbatim rather than dropping out.
  assert.equal(strategyCategory("brand_new"), "brand_new");
});

test("statusCategory groups raw statuses into filter buckets", () => {
  assert.equal(statusCategory("in_flight"), "open");
  assert.equal(statusCategory("balanced"), "open");
  assert.equal(statusCategory("imbalanced"), "attention");
  assert.equal(statusCategory("unresolved"), "attention");
  assert.equal(statusCategory("closed"), "closed");
  assert.equal(statusCategory("failed"), "failed");
});

test("filterTradedOpps applies strategy and status categories", () => {
  const rows = [
    opp({ signalId: "s-opt", strategyId: "options_box_spread", status: "balanced" }),
    opp({ signalId: "s-fund", strategyId: "funding", status: "closed" }),
    opp({ signalId: "s-fut", strategyId: "futures", status: "failed" }),
    opp({ signalId: "s-opt2", strategyId: "options_parity", status: "unresolved" }),
  ];
  assert.equal(filterTradedOpps(rows, "all", "all").length, 4);
  // Large strategy category: both options substrategies match "Options".
  assert.deepEqual(
    filterTradedOpps(rows, "Options", "all").map((o) => o.signalId),
    ["s-opt", "s-opt2"],
  );
  // Status category: open = balanced + in_flight.
  assert.deepEqual(
    filterTradedOpps(rows, "all", "open").map((o) => o.signalId),
    ["s-opt"],
  );
  assert.deepEqual(
    filterTradedOpps(rows, "all", "attention").map((o) => o.signalId),
    ["s-opt2"],
  );
  // Combined.
  assert.equal(filterTradedOpps(rows, "Funding Rate", "closed").length, 1);
  assert.equal(filterTradedOpps(rows, "Options", "failed").length, 0);
});

test("empty ledger renders the honest empty state", () => {
  const html = renderToStaticMarkup(
    React.createElement(TradedOpportunitiesTable, { opps: [], failed: false, onSelect: () => {} }),
  );
  assert.match(html, /Nothing Traded Yet/);
  const loading = renderToStaticMarkup(
    React.createElement(TradedOpportunitiesTable, { opps: null, failed: false, onSelect: () => {} }),
  );
  assert.match(loading, /Loading traded opportunities/);
});

test("detail modal lists every atomic transaction with role, side, and fills", () => {
  const html = renderToStaticMarkup(
    React.createElement(TradedOpportunityDetailModal, { opp: opp(), onClose: () => {} }),
  );
  assert.match(html, /Traded Opportunity/);
  assert.match(html, /sig-1-aa/);
  assert.match(html, /Atomic Transactions \(4\)/);
  // Entry + flatten roles and derived order sides.
  assert.equal(html.match(/>ENTRY</g)?.length, 2);
  assert.equal(html.match(/>FLATTEN</g)?.length, 2);
  assert.match(html, />BUY</);
  assert.match(html, />SELL</);
  // Quantities, prices, fees, order ids.
  assert.match(html, /1 \/ 1/);
  assert.match(html, /0\.5 \/ 1/, "partial fill renders filled/intended");
  assert.match(html, /\$100\.00 \/ \$100\.00/);
  assert.match(html, />Partial</);
  assert.match(html, /oid-l1/);
  assert.match(html, /cid-l1/);
  // Economics.
  assert.match(html, /\+\$0\.70/);
  assert.match(html, /0\.64 bps/);
  assert.match(html, /12\.5 bps/);
  assert.match(html, /-\$1\.80/);
});

test("modal never fabricates realized PnL for open positions", () => {
  const open = opp({
    status: "balanced",
    lotStatus: "open",
    realizedPnlUsd: null,
    realizedPnlBps: null,
    closeSpreadBps: null,
    closedAt: null,
  });
  const html = renderToStaticMarkup(
    React.createElement(TradedOpportunityDetailModal, { opp: open, onClose: () => {} }),
  );
  assert.match(html, />Open</);
  assert.match(html, /Realized PnL/);
  // The realized field falls back to a dash — no invented number.
  assert.doesNotMatch(html, /\+\$0\.70.*Realized/s);
});

test("clicking a row opens the atomic-trail dialog (interactive DOM)", async (t) => {
  // React's scheduler keeps a ref'd handle alive after unmount in bare Node,
  // so the interactive mount runs in a child process that can exit cleanly.
  await t.test("click → dialog → ESC", () => {
    // The harness below is exec'd as a plain script inside the child; it
    // reuses this file's already-bundled exports through `exports.__harness`.
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
// `__filename` inside the esbuild bundle resolves to this file's own path, so
// the child re-executes the bundle and lands here.
declare const __filename: string;

async function runInteractiveChild() {
  const { Window } = await import("happy-dom");
  const { createRoot } = await import("react-dom/client");
  const { act } = await import("react");

  const window = new Window();
  for (const key of [
    "document", "window", "HTMLElement", "HTMLButtonElement", "HTMLTableRowElement",
    "Element", "Node", "MouseEvent", "KeyboardEvent", "CustomEvent", "Event",
    "SVGElement", "DocumentFragment", "MutationObserver", "getComputedStyle",
    "requestAnimationFrame", "cancelAnimationFrame",
  ]) {
    (globalThis as Record<string, unknown>)[key] =
      (window as unknown as Record<string, unknown>)[key] ??
      (globalThis as Record<string, unknown>)[key];
  }
  (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;

  const container = window.document.createElement("div");
  window.document.body.appendChild(container);
  // happy-dom elements don't satisfy React's DOM typings — they're the same
  // runtime shape, so cast through unknown.
  const reactContainer = container as unknown as HTMLElement;

  let selected: TradedOpportunityDto | null = null;
  const Harness = () => {
    const [sel, setSel] = React.useState<TradedOpportunityDto | null>(null);
    selected = sel;
    return React.createElement(
      React.Fragment,
      null,
      React.createElement(TradedOpportunitiesTable, {
        opps: [opp()],
        failed: false,
        onSelect: setSel,
      }),
      sel && React.createElement(TradedOpportunityDetailModal, { opp: sel, onClose: () => setSel(null) }),
    );
  };

  const root = createRoot(reactContainer);
  await act(async () => {
    root.render(React.createElement(Harness));
  });

  const btn = container.querySelector('button[aria-label^="Open opportunity details"]');
  if (!btn) throw new Error("row exposes no detail button");
  await act(async () => {
    btn.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true }));
  });

  const dialog = container.querySelector('[role="dialog"]');
  if (!dialog) throw new Error("clicking the row did not open the detail modal");
  if (!selected) throw new Error("selection state not set");
  const txt = dialog.textContent ?? "";
  for (const needle of ["Atomic Transactions (4)", "ENTRY", "FLATTEN", "oid-l1"]) {
    if (!txt.includes(needle)) throw new Error(`modal missing ${needle}`);
  }

  await act(async () => {
    window.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  });
  if (container.querySelector('[role="dialog"]') !== null) {
    throw new Error("ESC did not dismiss the modal");
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
