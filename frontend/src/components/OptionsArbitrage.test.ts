import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  OptionsArbitrage,
  OptionsParityCard,
} from "./OptionsArbitrage.tsx";
import type { OptionsParityOpportunityDto } from "../types/index.ts";

const noop = async () => {};

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
      deribitAvailable: false,
    })
  );
  assert.match(html, /Options ↔ Perp Parity/);
  assert.match(html, /No options-capable venue is enabled/);
  assert.match(html, /test\.deribit\.com/);
  const scanBtn = html.match(/<button[^>]*>\s*<svg[\s\S]*?Scan\s*<\/button>/);
  assert.ok(scanBtn, "Scan button renders (disabled)");
  assert.match(scanBtn[0], /disabled/);
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
