import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { BotStatCards } from "./BotStatCards.tsx";
import {
  BOT_PROFIT_UNATTRIBUTED_REASON,
  computeBotCycleStats,
} from "../services/botCycleStats.ts";
import type { SignalExecutionDto } from "../types/index.ts";

const closed = (id: string): SignalExecutionDto => ({
  id,
  strategyId: "radar",
  symbol: "BTC",
  status: "closed",
  routeId: null,
  accountLongId: null,
  accountShortId: null,
  expectedNetUsd: 12,
  feesUsd: 1,
  note: null,
  legs: [
    {
      id: "l1",
      signalId: id,
      accountId: null,
      venue: "hyperliquid",
      side: "long",
      intendedQty: 1,
      filledQty: 1,
      fillPrice: 100,
      feeUsd: 0.5,
      venueOrderId: null,
      createdAt: "2026-01-01T00:00:00Z",
      updatedAt: "2026-01-01T00:00:00Z",
    },
  ],
  createdAt: "2026-01-01T00:00:00Z",
  updatedAt: "2026-01-01T00:00:00Z",
  closedAt: "2026-01-02T00:00:00Z",
});

test("stat cards show completed and open cycle counts", () => {
  const stats = computeBotCycleStats([
    closed("a"),
    closed("b"),
    { ...closed("c"), status: "balanced", closedAt: null },
  ]);
  const html = renderToStaticMarkup(React.createElement(BotStatCards, { stats }));
  assert.match(html, /Completed Cycles/);
  assert.match(html, /Open Cycles/);
  assert.match(html, />2</, "two completed round trips");
  assert.match(html, />1</, "one open cycle");
});

test("unattributed profit renders as a dash with the reason, never zero", () => {
  const html = renderToStaticMarkup(
    React.createElement(BotStatCards, { stats: computeBotCycleStats([closed("a")]) }),
  );
  assert.match(html, /Bot Net Profit/);
  assert.match(html, /—/);
  assert.doesNotMatch(html, /\$0\.00/);
  assert.ok(
    html.includes(BOT_PROFIT_UNATTRIBUTED_REASON),
    "reason travels in the tooltip",
  );
});

test("unknown ledger state dashes every card", () => {
  const html = renderToStaticMarkup(
    React.createElement(BotStatCards, { stats: computeBotCycleStats(null) }),
  );
  assert.doesNotMatch(html, />0</);
  assert.match(html, /—/);
});

test("attributed profit renders real numbers, not a dash", () => {
  const stats = computeBotCycleStats([closed("a")], "radar", {
    strategyId: "radar",
    netProfitUsd: 64,
    netProfitBps: 10.5,
    closedFamilies: 1,
    openFamilies: 0,
    unattributedClosedFamilies: 0,
    unattributedLogEntries: 0,
  });
  const html = renderToStaticMarkup(React.createElement(BotStatCards, { stats }));
  assert.match(html, /\+\$64\.00/);
  assert.match(html, /\+10\.5 bps/);
  assert.ok(html.includes("Attributed realized PnL"), "attributed tooltip");
});
