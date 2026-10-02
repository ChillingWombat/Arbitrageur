import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { PredictionHub } from "./PredictionHub.tsx";
import { OpportunityRadar } from "./OpportunityRadar.tsx";
import { SpotArbitrageBot } from "./SpotArbitrageBot.tsx";
import { FundingRadar } from "./FundingRadar.tsx";
import { OptionsArbitrage, OPTIONS_SUBSTRATEGIES } from "./OptionsArbitrage.tsx";
import { TickerExplorer } from "./TickerExplorer.tsx";
import type {
  PredictionOpportunityDto,
  FundingOpportunityDto,
  StrategyCapabilityDto,
} from "../types/index.ts";
import type { PerpTicker } from "../types/tickers.ts";
import type { SpotStrategyParams } from "./TickerConfigModal.tsx";

const noop = () => {};
const asyncNoop = async () => {};

// ============================================================================
// Requirement R1: Prediction Market Ribbon & Input Cleanups
// ============================================================================

test("R1 (PredictionHub): scaffold warning text is NEVER rendered under all capability states", () => {
  const capabilityStates: Array<StrategyCapabilityDto | undefined> = [
    undefined,
    {
      strategyId: "prediction",
      scanReady: false,
      executeReady: false,
      reason: "Prediction venues are scaffolds: no market data or order execution path",
    },
    {
      strategyId: "prediction",
      scanReady: true,
      executeReady: false,
      reason: "scaffold",
    },
    {
      strategyId: "prediction",
      scanReady: true,
      executeReady: true,
      reason: null,
    },
  ];

  for (const cap of capabilityStates) {
    const html = renderToStaticMarkup(
      React.createElement(PredictionHub, {
        opportunities: [],
        onSearch: noop,
        capability: cap,
      })
    );

    assert.doesNotMatch(
      html,
      /Prediction venues are scaffolds/i,
      "Scaffold warning banner must never be rendered"
    );
    assert.doesNotMatch(
      html,
      /no market data or order execution path/i,
      "Scaffold order execution path text must never be rendered"
    );
    assert.doesNotMatch(
      html,
      /execution path unavailable/i,
      "Obsolete execution path unavailable warning must not appear"
    );
  }
});

test("R1 (PredictionHub): Max Slip ribbon tag is completely absent under all states", () => {
  for (const executeReady of [false, true, undefined]) {
    const html = renderToStaticMarkup(
      React.createElement(PredictionHub, {
        opportunities: [],
        onSearch: noop,
        capability: executeReady !== undefined ? ({
          strategyId: "prediction",
          scanReady: true,
          executeReady,
          reason: null,
        } as StrategyCapabilityDto) : undefined,
      })
    );

    assert.doesNotMatch(html, /Max Slip/i, "Max Slip ribbon pill must not be rendered");
    assert.doesNotMatch(html, /no execution path/i);
  }
});

test("R1 (PredictionHub): filter search input has exact placeholder 'Search contract...'", () => {
  const html = renderToStaticMarkup(
    React.createElement(PredictionHub, {
      opportunities: [],
      onSearch: noop,
    })
  );

  assert.match(
    html,
    /placeholder="Search contract\.\.\."/,
    "Search input must have placeholder='Search contract...'"
  );
  assert.doesNotMatch(
    html,
    /placeholder="Search contract \(e\.g\. FED, BTC\)\.\.\."/,
    "Placeholder must not contain example text '(e.g. FED, BTC)'"
  );
  assert.doesNotMatch(html, /FED, BTC/);
});

test("R1 (PredictionHub): Min Vol renders active formatted USD badge across default and custom values", () => {
  // 1. Default (10,000 USD)
  const htmlDefault = renderToStaticMarkup(
    React.createElement(PredictionHub, {
      opportunities: [],
      onSearch: noop,
    })
  );
  assert.match(
    htmlDefault,
    /<div[^>]*class="ribbon-pill"[^>]*>[\s\S]*?<span[^>]*>Min Vol<\/span>[\s\S]*?<span[^>]*>10,000<\/span>[\s\S]*?<span[^>]*>USD<\/span>[\s\S]*?<\/div>/
  );
  assert.doesNotMatch(htmlDefault, /no venue volume data/);
  assert.doesNotMatch(htmlDefault, /ribbon-pill\s+opacity-60/);

  // 2. Custom values ($500,000 USD, $1,500,000 USD, 0 USD)
  const testVolumes = [500_000, 1_500_000, 25_000, 0];
  for (const vol of testVolumes) {
    const htmlCustom = renderToStaticMarkup(
      React.createElement(PredictionHub, {
        opportunities: [],
        onSearch: noop,
        minEventVolumeUsd: vol,
      })
    );
    const formatted = vol.toLocaleString();
    assert.match(
      htmlCustom,
      new RegExp(
        `<div[^>]*class="ribbon-pill"[^>]*>[\\s\\S]*?<span[^>]*>Min Vol<\\/span>[\\s\\S]*?<span[^>]*>${formatted}<\\/span>[\\s\\S]*?<span[^>]*>USD<\\/span>[\\s\\S]*?<\\/div>`
      ),
      `Expected Min Vol to display formatted ${formatted} USD`
    );
    assert.doesNotMatch(htmlCustom, /no venue volume data/);
  }
});

test("R1 (PredictionHub): renders opportunity cards without scaffold warning text", () => {
  const sampleOpp: PredictionOpportunityDto = {
    id: "opp-fed-1",
    market_title: "Fed Interest Rate Nov 2026",
    venue_a: "Polymarket",
    venue_b: "Kalshi",
    strategy: "prediction_parity",
    buy_yes_venue: "Polymarket",
    buy_no_venue: "Kalshi",
    yes_price_cents: 48,
    no_price_cents: 50,
    total_cost_cents: 98,
    conditional_payout_cents: 100,
    net_profit_cents: 2,
    roi_pct: 2.04,
    has_arbitrage: true,
  };

  const html = renderToStaticMarkup(
    React.createElement(PredictionHub, {
      opportunities: [sampleOpp],
      onSearch: noop,
      capability: {
        strategyId: "prediction",
        scanReady: true,
        executeReady: false,
        reason: null,
      },
    })
  );

  assert.match(html, /Fed Interest Rate Nov 2026/);
  assert.match(html, /Buy Yes/);
  assert.match(html, /Buy No/);
  assert.doesNotMatch(html, /Prediction venues are scaffolds/i);
});

// ============================================================================
// Requirement R2: Strategy Monitor Header Alignment & Dropdown Boundaries
// ============================================================================

test("R2 (OpportunityRadar): Execution Single-Way badge is completely absent from ribbon", () => {
  for (const isPaused of [false, true]) {
    const html = renderToStaticMarkup(
      React.createElement(OpportunityRadar, {
        executionLogs: [],
        isPaused,
      })
    );

    assert.doesNotMatch(html, /Single-Way/i, "Execution Single-Way badge must be removed");
    assert.doesNotMatch(html, /Two-Way/i);
    assert.doesNotMatch(html, /title="Execution mode — edit via Configurator"/);
    assert.doesNotMatch(html, />Execution<\/span>/);
  }
});

test("R2 (OpportunityRadar): Opportunity Scanner header on left and right-aligned filter controls", () => {
  const html = renderToStaticMarkup(
    React.createElement(OpportunityRadar, {
      executionLogs: [],
      isPaused: false,
    })
  );

  // Outer header bar uses flex justify-between
  assert.match(
    html,
    /class="flex items-center justify-between gap-3 px-4 py-2\.5 border-b border-surface-border text-xs font-sans shrink-0 bg-surface-raised\/40"/
  );

  // Left side title
  assert.match(
    html,
    /<h3 class="text-\[13px\] font-bold font-sans tracking-wider text-slate-900">\s*Opportunity Scanner\s*<\/h3>/
  );

  // Right side filters container
  assert.match(
    html,
    /<div class="flex items-center gap-2 text-xs font-sans">[\s\S]*?Symbol[\s\S]*?Venue[\s\S]*?Min Exp[\s\S]*?Min Win/
  );
});

test("R2 (OpportunityRadar): venue dropdown is anchored to right edge (right-0) preventing screen overflow", async () => {
  const { readFileSync } = await import("node:fs");
  const { resolve } = await import("node:path");
  const filePath = resolve(process.cwd(), "src/components/OpportunityRadar.tsx");
  const source = readFileSync(filePath, "utf8");

  // Venue dropdown must have absolute right-0 top-full
  assert.match(
    source,
    /showVenueMenu\s*&&\s*\([\s\S]*?className="absolute right-0 top-full mt-1\.5 w-52 bg-surface border border-surface-border rounded-xl shadow-elevated p-1\.5 z-30/
  );
});

test("R2 (SpotArbitrageBot): Opportunity Scanner header on left and right-aligned filter controls", () => {
  const spotParams: SpotStrategyParams = {
    tradeSizeUsd: 1000,
    minNetSpreadBps: 15,
    maxSlippageBps: 5,
  };

  const html = renderToStaticMarkup(
    React.createElement(SpotArbitrageBot, {
      executionLogs: [],
      onRefreshAll: asyncNoop,
      params: spotParams,
      capability: {
        strategyId: "spot",
        scanReady: true,
        executeReady: true,
        reason: null,
      },
    })
  );

  // Filter bar has flex justify-between
  assert.match(
    html,
    /class="p-3\.5 border-b border-surface-border flex items-center justify-between gap-3 bg-surface"/
  );

  // Header title on the left
  assert.match(
    html,
    /<h3 class="text-\[13px\] font-bold font-sans tracking-wider text-slate-900">\s*Opportunity Scanner\s*<\/h3>/
  );

  // Filter controls on the right
  assert.match(
    html,
    /<div class="flex items-center gap-3 text-xs font-sans">[\s\S]*?Search[\s\S]*?Filter Asset:[\s\S]*?opportunities detected/
  );
});

test("R2 (SpotArbitrageBot): asset dropdown is anchored to right edge (right-0) preventing screen overflow", async () => {
  const { readFileSync } = await import("node:fs");
  const { resolve } = await import("node:path");
  const filePath = resolve(process.cwd(), "src/components/SpotArbitrageBot.tsx");
  const source = readFileSync(filePath, "utf8");

  // Asset dropdown menu must have right-0 top-full
  assert.match(
    source,
    /className="dropdown-menu right-0 top-full mt-1\.5 w-48 z-30"/
  );
});

// ============================================================================
// Requirement R5: Funding Rate Opportunity Scanner Dimensions
// ============================================================================

const createFundingOpp = (id: string, symbol: string, netApr: number): FundingOpportunityDto =>
  ({
    id,
    symbol,
    long_venue: "hyperliquid",
    short_venue: "lighter",
    long_venue_id: "hyperliquid",
    short_venue_id: "lighter",
    long_symbol: symbol,
    short_symbol: symbol,
    long_market_type: "perpetual",
    short_market_type: "perpetual",
    net_rate_8h_pct: 0.009,
    net_apr_pct: netApr,
    net_apy_pct: null,
    round_trip_fee_bps: 20,
    break_even_hours: 4,
    estimated_daily_income_usd: 1.5,
    trade_size_usd: 1000,
  } as FundingOpportunityDto);

test("R5 (FundingRadar): scanner gadget container has locked h-[600px] and inner overflow-y-auto", () => {
  const html = renderToStaticMarkup(
    React.createElement(FundingRadar, {
      opportunities: [createFundingOpp("opp-1", "BTC-USDC", 15.0)],
      tradeSize: 1000,
      loading: false,
    })
  );

  // Outer container is strictly h-[600px]
  assert.match(
    html,
    /<div class="bg-surface rounded-xl border border-surface-border p-4 shadow-card h-\[600px\] flex flex-col">/
  );

  // Inner scrollable wrapper is flex-1 min-h-0 overflow-y-auto
  assert.match(
    html,
    /<div class="flex-1 min-h-0 overflow-y-auto">/
  );
});

test("R5 (FundingRadar): individual opportunity cards strictly maintain fixed h-[250px] overflow-hidden across single, double, and multi-row scenarios", () => {
  const counts = [1, 2, 5, 10, 15, 25];

  for (const count of counts) {
    const opps: FundingOpportunityDto[] = Array.from({ length: count }, (_, i) =>
      createFundingOpp(`opp-${i}`, `COIN${i}-USDC`, 10 + i)
    );

    const html = renderToStaticMarkup(
      React.createElement(FundingRadar, {
        opportunities: opps,
        tradeSize: 1000,
        loading: false,
      })
    );

    // Verify outer container is h-[600px]
    assert.match(html, /h-\[600px\]/);

    // Verify each card has fixed h-[250px] overflow-hidden
    const cardMatches = html.match(/h-\[250px\]\s+overflow-hidden/g) || [];
    assert.equal(
      cardMatches.length,
      count,
      `Expected ${count} cards with 'h-[250px] overflow-hidden', found ${cardMatches.length}`
    );

    // Verify no card lacks h-[250px]
    const allOpportunityPills = html.match(/#\d+/g) || [];
    assert.equal(allOpportunityPills.length, count);
  }
});

// ============================================================================
// Requirement R6: Options Section Header Ribbon
// ============================================================================

test("R6 (OptionsArbitrage): strategy ribbon bar is hidden when selected='all' (explicit and default)", () => {
  // Case A: explicit initialSelected="all"
  const htmlExplicit = renderToStaticMarkup(
    React.createElement(OptionsArbitrage, {
      executionLogs: [],
      onRefreshAll: asyncNoop,
      optionsVenueAvailable: true,
      initialSelected: "all",
    })
  );
  assert.doesNotMatch(htmlExplicit, /<span[^>]*>Options Strategies<\/span>/);
  assert.doesNotMatch(htmlExplicit, /<button[^>]*>\s*<svg[\s\S]*?Scan\s*<\/button>/);
  assert.doesNotMatch(htmlExplicit, /shrink-0 px-4 py-3 flex items-center justify-between gap-3 bg-surface rounded-xl border border-surface-border shadow-2xs select-none/);

  // Case B: default initialSelected (omitted -> "all")
  const htmlDefault = renderToStaticMarkup(
    React.createElement(OptionsArbitrage, {
      executionLogs: [],
      onRefreshAll: asyncNoop,
      optionsVenueAvailable: true,
    })
  );
  assert.doesNotMatch(htmlDefault, /<span[^>]*>Options Strategies<\/span>/);
  assert.doesNotMatch(htmlDefault, /<button[^>]*>\s*<svg[\s\S]*?Scan\s*<\/button>/);
  assert.doesNotMatch(htmlDefault, /shrink-0 px-4 py-3 flex items-center justify-between gap-3 bg-surface rounded-xl border border-surface-border shadow-2xs select-none/);
});

test("R6 (OptionsArbitrage): strategy ribbon bar is rendered when a specific substrategy is selected", () => {
  const substrategiesToTest = OPTIONS_SUBSTRATEGIES.filter((s) => s.status === "live");

  for (const { id, name } of substrategiesToTest) {
    const html = renderToStaticMarkup(
      React.createElement(OptionsArbitrage, {
        executionLogs: [],
        onRefreshAll: asyncNoop,
        optionsVenueAvailable: true,
        initialSelected: id,
      })
    );

    // Ribbon container MUST be rendered
    assert.match(
      html,
      /shrink-0 px-4 py-3 flex items-center justify-between gap-3 bg-surface rounded-xl border border-surface-border shadow-2xs select-none/,
      `Ribbon container must be rendered for substrategy ${id}`
    );

    // Substrategy title rendered in the ribbon
    const escapedName = name.replace(/&/g, "&amp;");
    assert.match(
      html,
      new RegExp(`<span class="text-\\[13px\\] font-mono font-bold tracking-wider text-slate-900 pr-1 shrink-0">\\s*${escapedName}\\s*<\\/span>`),
      `Ribbon must display substrategy name ${name}`
    );

    // Bot status badge rendered in the ribbon
    assert.match(html, /title="Strategy bot status: (Active|Paused|Reducing|Stopped)"/);

    // Scan button rendered in the ribbon
    assert.match(html, /<button[^>]*>\s*<svg[\s\S]*?Scan\s*<\/button>/);
  }
});

// ============================================================================
// Requirement R8: Ticker Explorer Symbol Quote Display
// ============================================================================

const createTicker = (symbol: string, base: string, quote: string): PerpTicker => ({
  symbol,
  base,
  quote,
  name: base,
  venues: ["HL", "LGT"],
  basePrice: 100,
});

const tickerBaseProps = {
  selectedSymbol: null as string | null,
  onSelectTicker: noop,
  executionLogs: [] as never[],
};

test("R8 (TickerExplorer): suppresses -USD quote for USD pairs across upper, lower, and mixed case variations", () => {
  const usdVariations = [
    { symbol: "APT-USD", base: "APT", quote: "USD" },
    { symbol: "BTC-usd", base: "BTC", quote: "usd" },
    { symbol: "ETH-Usd", base: "ETH", quote: "Usd" },
    { symbol: "SOL-uSd", base: "SOL", quote: "uSd" },
  ];

  const html = renderToStaticMarkup(
    React.createElement(TickerExplorer, {
      ...tickerBaseProps,
      availableTickers: usdVariations.map((t) => createTicker(t.symbol, t.base, t.quote)),
    })
  );

  // Each base symbol must be rendered cleanly
  assert.match(html, /<span[^>]*>APT<\/span>/);
  assert.match(html, /<span[^>]*>BTC<\/span>/);
  assert.match(html, /<span[^>]*>ETH<\/span>/);
  assert.match(html, /<span[^>]*>SOL<\/span>/);

  // No -USD quote suffix (case-insensitive) should be rendered anywhere
  assert.doesNotMatch(html, />-USD</i);
  assert.doesNotMatch(html, />-usd</i);
  assert.doesNotMatch(html, />-Usd</i);
});

test("R8 (TickerExplorer): preserves non-USD quotes (-USDC, -USDT, -EUR) across upper and lowercase variations", () => {
  const nonUsdTickers = [
    { symbol: "APT-USDC", base: "APT", quote: "USDC" },
    { symbol: "SUI-usdc", base: "SUI", quote: "usdc" },
    { symbol: "BTC-USDT", base: "BTC", quote: "USDT" },
    { symbol: "DOGE-usdt", base: "DOGE", quote: "usdt" },
    { symbol: "ETH-EUR", base: "ETH", quote: "EUR" },
    { symbol: "SOL-eur", base: "SOL", quote: "eur" },
  ];

  const html = renderToStaticMarkup(
    React.createElement(TickerExplorer, {
      ...tickerBaseProps,
      availableTickers: nonUsdTickers.map((t) => createTicker(t.symbol, t.base, t.quote)),
    })
  );

  assert.match(html, />-USDC</);
  assert.match(html, />-usdc</);
  assert.match(html, />-USDT</);
  assert.match(html, />-usdt</);
  assert.match(html, />-EUR</);
  assert.match(html, />-eur</);
});

test("R8 (TickerExplorer): mixed list with USD and non-USD tickers displays only non-USD quotes", () => {
  const mixedTickers = [
    createTicker("APT-USD", "APT", "USD"),
    createTicker("APT-USDC", "APT", "USDC"),
    createTicker("APT-USDT", "APT", "USDT"),
    createTicker("SOL-USD", "SOL", "USD"),
    createTicker("SOL-EUR", "SOL", "EUR"),
  ];

  const html = renderToStaticMarkup(
    React.createElement(TickerExplorer, {
      ...tickerBaseProps,
      availableTickers: mixedTickers,
    })
  );

  // APT-USD and SOL-USD omit quote suffix
  assert.doesNotMatch(html, />-USD</);

  // Non-USD quotes are present
  assert.match(html, />-USDC</);
  assert.match(html, />-USDT</);
  assert.match(html, />-EUR</);
});
