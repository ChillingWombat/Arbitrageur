import nodeTest from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  EXCHANGES_ALPHABETICAL,
  EXCHANGE_GROUP_ORDER,
  filterSidebarExchanges,
  groupExchangesBySection,
  TransactionHistoryCard,
} from "./EngineSettings.tsx";
import type { ConfirmActionSpec } from "./ConfirmActionModal.tsx";

// In `--interactive-child` mode this file re-executes as a plain script: the
// DOM mount lives at the bottom and tests must not re-register (or they'd
// recurse into another child). `test` becomes a no-op there.
const INTERACTIVE_CHILD = process.argv.includes("--interactive-child");
const test = INTERACTIVE_CHILD ? (((() => {}) as unknown) as typeof nodeTest) : nodeTest;
import { EVM_AMM_VENUE_IDS } from "./ExchangeAccounts.tsx";
import { venueHasTestnet } from "../types/tickers";

test("EVM Wallet entry exists and its group pins first in the sidebar", () => {
  const wallet = EXCHANGES_ALPHABETICAL.find((ex) => ex.id === "evm_wallet");
  assert.ok(wallet, "evm_wallet metadata entry missing");
  assert.equal(wallet.group, "EVM Wallet");
  assert.equal(EXCHANGE_GROUP_ORDER[0], "EVM Wallet");

  const sections = groupExchangesBySection(EXCHANGES_ALPHABETICAL);
  assert.equal(sections[0].group, "EVM Wallet");
  assert.deepEqual(
    sections[0].venues.map((v) => v.id),
    ["evm_wallet"],
  );
});

test("individual EVM AMM venues keep metadata but leave the sidebar group list", () => {
  // The 11 AMM ids remain in EXCHANGES_ALPHABETICAL for name/type lookups
  // — they are filtered from visibleExchanges, not deleted here. (Fees are
  // not display metadata: every fee surface reads `get_venue_fees`.)
  for (const id of EVM_AMM_VENUE_IDS) {
    assert.ok(
      EXCHANGES_ALPHABETICAL.some((ex) => ex.id === id),
      `${id} metadata missing`,
    );
  }
});

test("testnet sidebar lists only venues with a real demo endpoint", () => {
  const visible = filterSidebarExchanges(EXCHANGES_ALPHABETICAL, {
    tradingMode: "testnet",
    exchangeFilter: "all",
    favoriteExchanges: [],
  });
  assert.ok(visible.length > 0, "expected at least one testnet venue");
  for (const ex of visible) {
    assert.ok(
      venueHasTestnet(ex.id),
      `${ex.id} is live-only but surfaced in Testnet mode`,
    );
    assert.ok(!EVM_AMM_VENUE_IDS.has(ex.id), `${ex.id} is an AMM leaf`);
  }
  // Spot-check known live-only venues are gone in Testnet mode.
  const ids = new Set<string>(visible.map((ex) => ex.id));
  for (const liveOnly of ["coinbase", "aster", "evm_wallet", "variational"]) {
    assert.ok(!ids.has(liveOnly), `${liveOnly} must hide in Testnet mode`);
  }
  for (const tn of ["hyperliquid", "alpaca", "deribit", "binance", "robinhood", "arcus"]) {
    assert.ok(ids.has(tn), `${tn} should stay visible in Testnet mode`);
  }
});

test("live sidebar lists every non-AMM venue regardless of testnet support", () => {
  const visible = filterSidebarExchanges(EXCHANGES_ALPHABETICAL, {
    tradingMode: "live",
    exchangeFilter: "all",
    favoriteExchanges: [],
  });
  const ids = new Set(visible.map((ex) => ex.id));
  assert.ok(ids.has("arcus") && ids.has("evm_wallet") && ids.has("coinbase"));
});

test("favorites filter cannot resurrect a live-only venue in testnet", () => {
  const visible = filterSidebarExchanges(EXCHANGES_ALPHABETICAL, {
    tradingMode: "testnet",
    exchangeFilter: "favorites",
    favoriteExchanges: ["coinbase", "binance"],
  });
  const ids = visible.map((ex) => ex.id);
  assert.deepEqual(ids, ["binance"]);
});

test("transaction history card scopes the wipe to the active environment", () => {
  const html = renderToStaticMarkup(
    React.createElement(TransactionHistoryCard, {
      tradingMode: "testnet",
      disabled: false,
      onRequestReset: () => {},
      resetMsg: null,
      resetErr: null,
    }),
  );
  assert.match(html, /Reset Transaction History/);
  assert.match(html, /Cannot be undone/);
  assert.match(html, />Reset</);

  const live = renderToStaticMarkup(
    React.createElement(TransactionHistoryCard, {
      tradingMode: "live",
      disabled: true,
      onRequestReset: () => {},
      resetMsg: "Removed 3 signal(s)",
      resetErr: null,
    }),
  );
  assert.match(live, /disabled=""/, "disabled state reaches the button");
  assert.match(live, /Removed 3 signal\(s\)/);
});

test("reset button hands the parent an explicit confirm spec (interactive DOM)", async (t) => {
  await t.test("click → spec", () => {
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

  const container = window.document.createElement("div");
  window.document.body.appendChild(container);
  const reactContainer = container as unknown as HTMLElement;

  let spec: ConfirmActionSpec | null = null;
  const root = createRoot(reactContainer);
  await act(async () => {
    root.render(
      React.createElement(TransactionHistoryCard, {
        tradingMode: "testnet",
        disabled: false,
        onRequestReset: (s) => { spec = s; },
        resetMsg: null,
        resetErr: null,
      }),
    );
  });

  const btn = container.querySelector("button");
  if (!btn) throw new Error("no reset button");
  await act(async () => {
    btn.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true }));
  });
  if (!spec) throw new Error("click did not request the reset spec");
  const s: ConfirmActionSpec = spec;
  if (s.title !== "Reset Transaction History") throw new Error(`bad title ${s.title}`);
  if (!s.subtitle?.includes("Testnet")) throw new Error(`bad subtitle ${s.subtitle}`);
  if (!s.body.includes("Net Profit") || !s.body.includes("cannot be undone")) {
    throw new Error("confirm body missing blast-radius wording");
  }
  if (s.confirmLabel !== "Delete Transaction History") throw new Error(`bad label ${s.confirmLabel}`);

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
