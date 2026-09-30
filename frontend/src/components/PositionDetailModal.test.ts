import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { PositionDetailModal } from "./PositionDetailModal.tsx";
import type { OpenPositionDto } from "../types/index.ts";

const samplePosition: OpenPositionDto = {
  key: "obox-BTC-25DEC26",
  tradeId: "obox-1790752547998-a677e923",
  symbol: "BTC-25DEC26",
  strategyId: "options_box_spread",
  openedAt: "2026-09-30T17:15:00.000Z",
  signalCount: 4,
  notionalUsd: 13212,
  unrealizedPnlUsd: 308.54,
  feesUsd: 9.96,
  expectedNetUsd: 375.84,
  venues: [
    { venue: "deribit", symbol: "BTC-25DEC26-125000-C", netQty: 0.1 },
    { venue: "deribit", symbol: "BTC-25DEC26-160000-P", netQty: 0.1 },
    { venue: "deribit", symbol: "BTC-25DEC26-125000-P", netQty: -0.1 },
    { venue: "deribit", symbol: "BTC-25DEC26-160000-C", netQty: -0.1 },
  ],
  legSymbols: [
    "BTC-25DEC26-125000-C",
    "BTC-25DEC26-160000-P",
    "BTC-25DEC26-125000-P",
    "BTC-25DEC26-160000-C",
  ],
  reduceOnly: false,
};

test("PositionDetailModal renders exposure table with segregated Action (BOUGHT/WROTE) and Type (CALL/PUT) columns", () => {
  const html = renderToStaticMarkup(
    React.createElement(PositionDetailModal, {
      pos: samplePosition,
      onClose: () => {},
    })
  );

  // Table headers must have distinct Action and Type columns
  assert.match(html, /<th[^>]*>Action<\/th>/);
  assert.match(html, /<th[^>]*>Type<\/th>/);

  // Must render BOUGHT for positive netQty options legs and WROTE for negative netQty options legs
  assert.match(html, /BOUGHT/);
  assert.match(html, /WROTE/);

  // Must render CALL and PUT badges
  assert.match(html, />CALL<\/span>/);
  assert.match(html, />PUT<\/span>/);

  // Must NEVER render composite BUY /long or SELL /short strings
  assert.doesNotMatch(html, /BUY\s*\/long/);
  assert.doesNotMatch(html, /SELL\s*\/short/);
  assert.doesNotMatch(html, /\/short/);
  assert.doesNotMatch(html, /\/long/);
});

test("PositionDetailModal renders Net PnL sparkline with safe dimensions and $0 zero reference line", () => {
  const html = renderToStaticMarkup(
    React.createElement(PositionDetailModal, {
      pos: samplePosition,
      onClose: () => {},
    })
  );

  // Chart SVG must exist with 800x140 viewBox
  assert.match(html, /viewBox="0 0 800 140"/);

  // Must render $0 zero line label
  assert.match(html, />\$0<\/text>/);

  // Must render entry fees
  assert.match(html, /entry -\$9\.96 fees/);

  // Must render target label
  assert.match(html, /target:/);
});
