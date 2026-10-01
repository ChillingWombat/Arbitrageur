import { test } from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  OptionsConfigModal,
  LIVE_OPTIONS_SUBSTRATEGIES,
} from "./OptionsConfigModal.tsx";
import { OptionsBotConfigModal } from "./OptionsArbitrage.tsx";
import type { OptionsBotConfigDto } from "../types/index.ts";

const noop = () => {};

test("OptionsConfigModal renders nothing when isOpen is false", () => {
  const html = renderToStaticMarkup(
    React.createElement(OptionsConfigModal, {
      isOpen: false,
      onClose: noop,
    })
  );
  assert.equal(html, "");
});

test("OptionsConfigModal renders container, header title, and all 4 live substrategy tabs", () => {
  const html = renderToStaticMarkup(
    React.createElement(OptionsConfigModal, {
      isOpen: true,
      onClose: noop,
      initialSubstrategyId: "parity",
    })
  );

  // Modal container layout classes matching specification
  assert.match(html, /w-\[840px\]/);
  assert.match(html, /bg-surface/);
  assert.match(html, /rounded-2xl/);
  assert.match(html, /shadow-2xl/);

  // Header Title
  assert.match(html, /Options Strategy Configurator/);

  // All 4 live option substrategies present in header tabs
  for (const sub of LIVE_OPTIONS_SUBSTRATEGIES) {
    assert.match(html, new RegExp(sub.name));
  }
  assert.match(html, /Put–Call Parity/);
  assert.match(html, /Box Spread/);
  assert.match(html, /Jelly Roll/);
  assert.match(html, /Convexity Boundary/);

  // Action buttons
  assert.match(html, /Reset Defaults/);
  assert.match(html, /Cancel/);
  assert.match(html, /Save Changes/);
});

test("OptionsConfigModal renders left navigation rail with 4 standardized sections", () => {
  const html = renderToStaticMarkup(
    React.createElement(OptionsConfigModal, {
      isOpen: true,
      onClose: noop,
    })
  );

  // Width 220px navigation rail
  assert.match(html, /w-\[220px\]/);

  // 4 sections in the rail
  assert.match(html, /Bot Parameters/);
  assert.match(html, /Contract Filters/);
  assert.match(html, /Cadence &amp; Limits/);
  assert.match(html, /Account Routing/);
});

test("OptionsConfigModal renders Bot Parameters section with base size and edge hurdle by default", () => {
  const customCfg: OptionsBotConfigDto = {
    substrategyId: "parity",
    enabled: true,
    mode: "auto",
    qtyBase: 0.25,
    minEdgeBps: 18,
    minExpectedYieldPct: 10,
    minHoursToExpiry: 24,
    maxDaysToExpiry: 45,
    pollIntervalSecs: 15,
    maxOpenPositions: 3,
  };

  const html = renderToStaticMarkup(
    React.createElement(OptionsConfigModal, {
      isOpen: true,
      onClose: noop,
      cfg: customCfg,
    })
  );

  // Parameters Section Header
  assert.match(html, /Bot Parameters/);

  // Entry size field
  assert.match(html, /Entry Size/);
  assert.match(html, /value="0\.25"/);
  assert.match(html, /BTC\/ETH/);

  // Min edge hurdle field
  assert.match(html, /Min Net Edge Hurdle/);
  assert.match(html, /value="18"/);
  assert.match(html, /bps/);
});

test("OptionsBotConfigModal backward-compat wrapper renders OptionsConfigModal properly", () => {
  const customCfg: OptionsBotConfigDto = {
    substrategyId: "box_spread",
    enabled: false,
    mode: "off",
    qtyBase: 0.5,
    minEdgeBps: 30,
    minExpectedYieldPct: 15,
    minHoursToExpiry: 48,
    maxDaysToExpiry: 60,
    pollIntervalSecs: 30,
    maxOpenPositions: 2,
  };

  const html = renderToStaticMarkup(
    React.createElement(OptionsBotConfigModal, {
      cfg: customCfg,
      onSave: async () => {},
      onClose: noop,
    })
  );

  assert.match(html, /Options Strategy Configurator/);
  assert.match(html, /Box Spread/);
  assert.match(html, /value="0\.5"/);
  assert.match(html, /value="30"/);
});
