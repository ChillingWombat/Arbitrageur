import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { ExchangeVenueGadget } from "./ExchangeVenueGadget.tsx";
import { computeExchangeStats } from "../services/exchangeStats.ts";
import type { ExecutionLogEntry, VenueBalanceInfo } from "../types/index.ts";

const log = (id: string, netProfit: number): ExecutionLogEntry => ({
  id,
  timestamp: "2026-01-01T00:00:00Z",
  trade_type: "Spatial",
  symbol: "BTC",
  venue_a: "hyperliquid",
  venue_b: "lighter",
  leg_a_status: "FILLED",
  leg_b_status: "FILLED",
  success: true,
  gross_profit: netProfit + 2,
  fees: 2,
  net_profit: netProfit,
  latency_us: 10,
  note: "paper",
});

const balance = (venue: string): VenueBalanceInfo => ({
  venue,
  equity_usd: 10000,
  available_margin_usd: 7500,
  margin_rate_pct: 25,
  status: "HEALTHY",
});

test("venue rows show trades, average, total, equity, and utilisation", () => {
  const rows = computeExchangeStats([log("1", 10), log("2", -4)], [balance("hyperliquid")]);
  const html = renderToStaticMarkup(React.createElement(ExchangeVenueGadget, { rows }));
  assert.match(html, /Per-Exchange Stats/);
  assert.match(html, /hyperliquid/);
  assert.match(html, />2</, "two trade events");
  assert.match(html, /\+\$3\.00/, "average PnL per trade");
  assert.match(html, /\+\$6\.00/, "total PnL");
  assert.match(html, /\$10000\.00/, "current equity");
  assert.match(html, /25\.0%/, "current utilisation");
});

test("24h average utilisation dashes with the honest reason", () => {
  const rows = computeExchangeStats([log("1", 10)], [balance("hyperliquid")]);
  const html = renderToStaticMarkup(React.createElement(ExchangeVenueGadget, { rows }));
  assert.match(html, /24h avg util/);
  assert.ok(html.includes("No utilisation history is retained"), "reason travels in the tooltip");
});

test("missing balances dash equity and utilisation, never zero", () => {
  const rows = computeExchangeStats([log("1", 10)], []);
  const html = renderToStaticMarkup(React.createElement(ExchangeVenueGadget, { rows }));
  assert.match(html, /—/);
  assert.ok(html.includes("No balance snapshot for this venue."));
});

test("empty exchange data is explicit", () => {
  const html = renderToStaticMarkup(React.createElement(ExchangeVenueGadget, { rows: [] }));
  assert.match(html, /No Exchange Data/);
});
