import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { TickerExplorer } from "./TickerExplorer.tsx";
import type { PerpTicker } from "../types/tickers.ts";

const ticker = (symbol: string, base: string, quote: string, venues: string[]): PerpTicker => ({
  symbol,
  base,
  quote,
  name: base,
  venues,
  basePrice: 0,
});

const baseProps = {
  selectedSymbol: null as string | null,
  onSelectTicker: () => {},
  executionLogs: [] as never[],
};

test("quote-variant instruments render distinct rows labelled by quote", () => {
  // Regression: XRP-USDC and XRP-USDT are legitimately distinct instruments
  // (different venue sets) but the row used to render only the base, making
  // them look like duplicated tickers.
  const html = renderToStaticMarkup(
    React.createElement(TickerExplorer, {
      ...baseProps,
      availableTickers: [
        ticker("XRP-USDC", "XRP", "USDC", ["HL", "LGT"]),
        ticker("XRP-USDT", "XRP", "USDT", ["AST", "BNB"]),
      ],
    }),
  );
  assert.match(html, /XRP<span[^>]*>-USDC</, "USDC variant shows its quote");
  assert.match(html, /XRP<span[^>]*>-USDT</, "USDT variant shows its quote");
});

test("duplicate symbols from the ticker list render exactly once", () => {
  const html = renderToStaticMarkup(
    React.createElement(TickerExplorer, {
      ...baseProps,
      availableTickers: [
        ticker("ZEC-USDC", "ZEC", "USDC", ["HL", "LGT"]),
        ticker("ZEC-USDC", "ZEC", "USDC", ["HL", "LGT"]),
        ticker("zec-usdc", "ZEC", "USDC", ["HL", "LGT"]),
      ],
    }),
  );
  const rows = html.match(/role="button"/g) ?? [];
  assert.strictEqual(rows.length, 1, "same-symbol inputs collapse to a single row");
});
