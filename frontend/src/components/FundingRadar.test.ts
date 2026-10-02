import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { FundingRadar, fundingAutoEligible, splitFundingLegs } from "./FundingRadar.tsx";
import type { FundingOpportunityDto } from "../types";

const baseOpp = (over: Partial<FundingOpportunityDto>): FundingOpportunityDto =>
  ({
    id: "opp-1",
    symbol: "BTC-USDC",
    long_venue: "hyperliquid",
    short_venue: "lighter",
    long_venue_id: "hyperliquid",
    short_venue_id: "lighter",
    long_symbol: "BTC-USDC",
    short_symbol: "BTC-USDC",
    long_market_type: "perpetual",
    short_market_type: "perpetual",
    net_rate_8h_pct: 0.009,
    net_apr_pct: 10,
    net_apy_pct: null,
    round_trip_fee_bps: 20,
    break_even_hours: 4,
    estimated_daily_income_usd: 1,
    trade_size_usd: 1000,
    ...over,
  }) as FundingOpportunityDto;

test("auto eligibility requires the Min APR hurdle", () => {
  assert.ok(fundingAutoEligible(baseOpp({ net_apr_pct: 10 }), 10));
  assert.ok(!fundingAutoEligible(baseOpp({ net_apr_pct: 9.99 }), 10));
  assert.ok(fundingAutoEligible(baseOpp({ net_apr_pct: 0 }), 0));
});

test("fixed-yield rows are never auto-eligible (live-only legs)", () => {
  // Boros has no testnet and autonomous entries are testnet-only — a fixed
  // row above the hurdle must still be excluded from the loop's pick set.
  assert.ok(
    !fundingAutoEligible(baseOpp({ net_apr_pct: 40, kind: "fixed_yield" }), 10),
  );
  // carry and basis rows stay eligible
  assert.ok(fundingAutoEligible(baseOpp({ net_apr_pct: 40, kind: "carry" }), 10));
  assert.ok(fundingAutoEligible(baseOpp({ net_apr_pct: 40, kind: "basis" }), 10));
  // absent kind = carry (stale backends)
  assert.ok(
    fundingAutoEligible(baseOpp({ net_apr_pct: 40, kind: undefined }), 10),
  );
});

test("splitFundingLegs: positive netQty is the long leg, negative the short, legQty is max |qty|", () => {
  const { longLegs, shortLegs, legQty } = splitFundingLegs([
    { venue: "lighter", symbol: "BTC-USDC", netQty: 0.00477 },
    { venue: "hyperliquid", symbol: "BTC-USDC", netQty: -0.00477 },
  ]);
  assert.equal(longLegs.length, 1);
  assert.equal(longLegs[0].venue, "lighter");
  assert.equal(shortLegs.length, 1);
  assert.equal(shortLegs[0].venue, "hyperliquid");
  assert.equal(legQty, 0.00477);
});

test("splitFundingLegs: single-sided exposure reports one leg and no partner", () => {
  const { longLegs, shortLegs, legQty } = splitFundingLegs([
    { venue: "hyperliquid", symbol: "BTC-USDC", netQty: -0.00477 },
  ]);
  assert.equal(longLegs.length, 0);
  assert.equal(shortLegs.length, 1);
  assert.equal(legQty, 0.00477);
});

test("splitFundingLegs: empty venues yield no legs and zero qty", () => {
  const { longLegs, shortLegs, legQty } = splitFundingLegs([]);
  assert.equal(longLegs.length + shortLegs.length, 0);
  assert.equal(legQty, 0);
});

test("FundingRadar: bottom dock container enforces fixed height across tabs", () => {
  const html = renderToStaticMarkup(
    React.createElement(FundingRadar, {
      opportunities: [],
      tradeSize: 1000,
      loading: false,
    })
  );
  assert.match(html, /class="p-3\.5 h-96 overflow-y-auto"/);
});

test("FundingRadar renders standardized status badge immediately right of Strategy Status", () => {
  const html = renderToStaticMarkup(
    React.createElement(FundingRadar, {
      opportunities: [],
      tradeSize: 1000,
      loading: false,
      isPaused: false,
    })
  );
  assert.match(
    html,
    /Strategy Status<\/span>[\s\S]*?title="Strategy bot status: Stopped"[\s\S]*?>Stopped<\/span>/
  );
  assert.doesNotMatch(html, /Engine/i);
});

test("FundingRadar status badge handles isPaused and never renders Engine", () => {
  const htmlPaused = renderToStaticMarkup(
    React.createElement(FundingRadar, {
      opportunities: [],
      tradeSize: 1000,
      loading: false,
      isPaused: true,
    })
  );
  assert.match(htmlPaused, /Strategy Status/);
  assert.doesNotMatch(htmlPaused, /Engine/i);
});

test("FundingRadar renders 5-card top telemetry metric strip with correct values and subtexts", () => {
  const opp1 = baseOpp({
    symbol: "SUI-USDC",
    long_venue: "hyperliquid",
    short_venue: "vanta",
    net_apr_pct: 38.5,
    estimated_daily_income_usd: 14.2,
  });
  const opp2 = baseOpp({
    symbol: "BTC-USDC",
    long_venue: "hyperliquid",
    short_venue: "lighter",
    net_apr_pct: 12.0,
    estimated_daily_income_usd: 5.0,
  });
  const html = renderToStaticMarkup(
    React.createElement(FundingRadar, {
      opportunities: [opp1, opp2],
      tradeSize: 1000,
      loading: false,
      executionLogs: [
        {
          id: "log-1",
          timestamp: "2026-10-01T10:00:00Z",
          trade_type: "funding",
          strategy_id: "funding",
          symbol: "BTC-USDC",
          venue_a: "hyperliquid",
          venue_b: "lighter",
          leg_a_status: "filled",
          leg_b_status: "filled",
          success: true,
          gross_profit: 25.5,
          fees: 2.0,
          net_profit: 23.5,
          latency_us: 100,
          note: "",
        },
      ],
      openPositions: [
        {
          key: "pos-1",
          tradeId: "trade-1",
          strategyId: "funding",
          symbol: "SUI-USDC",
          legSymbols: ["SUI-USDC", "SUI-USDC"],
          signalCount: 1,
          venues: [],
          expectedNetUsd: 10,
          unrealizedPnlUsd: 2,
          feesUsd: 0.5,
          notionalUsd: 2000,
          openedAt: "2026-10-01T10:00:00Z",
          reduceOnly: false,
        },
      ],
    })
  );

  // Strip grid
  assert.match(html, /grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-3\.5 shrink-0/);
  // Total Net Profit
  assert.match(html, /Total Net Profit/);
  assert.match(html, /\+\$23\.50/);
  assert.match(html, /1 completed cycles/);
  // Total APR (since start of system / first trade)
  assert.match(html, /Total APR/);
  assert.match(html, /% APR/);
  assert.match(html, /since first trade/);
  // Avg Realized Return
  assert.match(html, /Avg Realized Return/);
  assert.match(html, /\+\$23\.50/);
  assert.match(html, /\+235\.0 bps avg/);
  // Open Positions
  assert.match(html, /Open Positions/);
  assert.match(html, />1<\/div>/);
  assert.match(html, /\$2,000 USD/);
  // Est. 24h Yield
  assert.match(html, /Est\. 24h Yield/);
  assert.match(html, /\/ day/);

  // Top-right corner icons removed for clean minimal styling matching BotStatCards
  assert.doesNotMatch(html, /lucide-trending-up/i);
  assert.doesNotMatch(html, /lucide-percent/i);
  assert.doesNotMatch(html, /lucide-activity/i);
  // Ensure the card title has clean styling directly under card div
  assert.match(
    html,
    /<div[^>]*class="[^"]*bg-surface[^"]*"[^>]*><span class="text-\[11px\] font-sans tracking-wider uppercase text-slate-500 font-semibold">Total Net Profit<\/span>/,
  );
});

test("FundingRadar renders dedicated navigation tabs in bottom dock", () => {
  const html = renderToStaticMarkup(
    React.createElement(FundingRadar, {
      opportunities: [],
      tradeSize: 1000,
      loading: false,
    })
  );

  assert.match(
    html,
    /class="grid grid-cols-4 border-b border-surface-border bg-slate-50\/60 select-none pt-1 px-1 gap-1"/
  );
  assert.match(html, /Open Positions/);
  assert.match(html, /Opportunity History/);
  assert.match(html, /Execution History/);
  assert.match(html, /Log Monitor/);
  // Check active tab styling
  assert.match(
    html,
    /text-accent-strong font-bold bg-surface border-t border-x border-surface-border -mb-px shadow-2xs/
  );
  // Check inactive tab styling
  assert.match(
    html,
    /text-slate-600 hover:text-slate-900 hover:bg-slate-100\/80 font-medium border-t border-x border-transparent/
  );
});

test("FundingRadar: Total APR card handles empty trade history honestly", () => {
  const html = renderToStaticMarkup(
    React.createElement(FundingRadar, {
      opportunities: [],
      tradeSize: 1000,
      loading: false,
      executionLogs: [],
    })
  );
  assert.match(html, /Total APR/);
  assert.match(html, /No trades yet/);
});

test("FundingRadar: Opportunity Scanner container is h-[600px] and cards have h-[250px] overflow-hidden", () => {
  const opp = baseOpp({
    symbol: "ETH-USDC",
    net_apr_pct: 18.5,
  });
  const html = renderToStaticMarkup(
    React.createElement(FundingRadar, {
      opportunities: [opp],
      tradeSize: 1000,
      loading: false,
    })
  );
  assert.match(
    html,
    /<div class="bg-surface rounded-xl border border-surface-border p-4 shadow-card h-\[600px\] flex flex-col">/
  );
  assert.match(
    html,
    /<h3 class="text-\[13px\] font-bold font-sans tracking-wider text-slate-900">Opportunity Scanner<\/h3>/
  );
  assert.match(
    html,
    /class="[^"]*h-\[250px\] overflow-hidden[^"]*"/
  );
});

test("FundingRadar: scanner comment documents 5x2 compact cards", async () => {
  const { readFileSync } = await import("node:fs");
  const { resolve } = await import("node:path");
  const filePath = resolve(process.cwd(), "src/components/FundingRadar.tsx");
  const source = readFileSync(filePath, "utf8");
  assert.match(source, /Opportunity Scanner — fixed window, 5x2 compact cards, click for details/);
});


