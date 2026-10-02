import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { PredictionHub } from "./PredictionHub.tsx";
import type { PredictionOpportunityDto, StrategyCapabilityDto } from "../types";

const noop = () => {};

const scaffoldCapability: StrategyCapabilityDto = {
  strategyId: "prediction",
  scanReady: false,
  executeReady: false,
  reason: "Prediction venues are scaffolds: no market data or order execution path",
};

const sampleOpp: PredictionOpportunityDto = {
  id: "fed-rate-2026",
  market_title: "Fed Interest Rate Decision Nov 2026",
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

test("R1: PredictionHub removes scaffold execution unavailable warning text", () => {
  const html = renderToStaticMarkup(
    React.createElement(PredictionHub, {
      opportunities: [],
      onSearch: noop,
      capability: scaffoldCapability,
    })
  );

  // Verifies the scaffold warning message is completely removed
  assert.doesNotMatch(
    html,
    /Prediction venues are scaffolds: no market data or order execution path/
  );
  assert.doesNotMatch(html, /execution path unavailable/i);
});

test("R1: PredictionHub removes Max Slip ribbon tag", () => {
  const html = renderToStaticMarkup(
    React.createElement(PredictionHub, {
      opportunities: [],
      onSearch: noop,
      capability: scaffoldCapability,
    })
  );

  // Max Slip ribbon pill must not be rendered
  assert.doesNotMatch(html, /Max Slip/);
  assert.doesNotMatch(html, /no execution path/);
});

test("R1: PredictionHub cleans search input placeholder to 'Search contract...'", () => {
  const html = renderToStaticMarkup(
    React.createElement(PredictionHub, {
      opportunities: [],
      onSearch: noop,
      capability: scaffoldCapability,
    })
  );

  // Placeholder must be "Search contract..." without the "(e.g. FED, BTC)" example
  assert.match(html, /placeholder="Search contract\.\.\."/);
  assert.doesNotMatch(html, /Search contract \(e\.g\. FED, BTC\)\.\.\./);
});

test("R1: PredictionHub renders active Min Vol parameter badge with default 10,000 USD", () => {
  const html = renderToStaticMarkup(
    React.createElement(PredictionHub, {
      opportunities: [],
      onSearch: noop,
      capability: scaffoldCapability,
    })
  );

  // Min Vol should render as an active parameter badge with default 10,000 USD
  assert.match(
    html,
    /<div[^>]*class="ribbon-pill"[^>]*>[\s\S]*?<span[^>]*>Min Vol<\/span>[\s\S]*?<span[^>]*>10,000<\/span>[\s\S]*?<span[^>]*>USD<\/span>[\s\S]*?<\/div>/
  );

  // Disabled styling and text must be removed
  assert.doesNotMatch(html, /no venue volume data/);
  assert.doesNotMatch(html, /ribbon-pill\s+opacity-60/);
});

test("R1: PredictionHub renders configured minEventVolumeUsd parameter (e.g. 500,000 USD)", () => {
  const html = renderToStaticMarkup(
    React.createElement(PredictionHub, {
      opportunities: [],
      onSearch: noop,
      capability: scaffoldCapability,
      minEventVolumeUsd: 500000,
    })
  );

  // Min Vol should show 500,000 USD
  assert.match(
    html,
    /<div[^>]*class="ribbon-pill"[^>]*>[\s\S]*?<span[^>]*>Min Vol<\/span>[\s\S]*?<span[^>]*>500,000<\/span>[\s\S]*?<span[^>]*>USD<\/span>[\s\S]*?<\/div>/
  );
  assert.doesNotMatch(html, /no venue volume data/);
});

test("R1: PredictionHub renders opportunities card when available", () => {
  const html = renderToStaticMarkup(
    React.createElement(PredictionHub, {
      opportunities: [sampleOpp],
      onSearch: noop,
      capability: scaffoldCapability,
    })
  );

  assert.match(html, /Fed Interest Rate Decision Nov 2026/);
  assert.match(html, /Polymarket/);
  assert.match(html, /Kalshi/);
});
