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

test("symbol quote display: hides -USD quote suffix for USD tickers while preserving non-USD quotes", () => {
  const html = renderToStaticMarkup(
    React.createElement(TickerExplorer, {
      ...baseProps,
      availableTickers: [
        ticker("APT-USD", "APT", "USD", ["HL", "LGT"]),
        ticker("APT-USDC", "APT", "USDC", ["HL", "LGT"]),
        ticker("APT-USDT", "APT", "USDT", ["AST", "BNB"]),
      ],
    }),
  );
  // Universal USD ticker displays only base symbol, omitting -USD quote
  assert.match(html, /<span[^>]*>APT<\/span>/);
  assert.doesNotMatch(html, />-USD</);

  // Non-USD stablecoin tickers retain their quote suffixes
  assert.match(html, /APT<span[^>]*>-USDC</);
  assert.match(html, /APT<span[^>]*>-USDT</);
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

test("net profit in dollars and bps share matching negative mathematical sign, with dollar in red and bps in grey text-[11px]", () => {
  // Regression test: Previously, when net_profit was negative (-$26.30),
  // trigger_spread_bps (+20.5 bps entry trigger spread) was accumulated into pnlBps,
  // displaying a positive +20.5 bps in red under a negative dollar PnL.
  // The fix ensures netProfitBps strictly reflects realized return with matching sign,
  // and bps is styled in neutral grey (text-muted, text-[11px]) rather than red text-loss.
  const html = renderToStaticMarkup(
    React.createElement(TickerExplorer, {
      ...baseProps,
      availableTickers: [
        ticker("APT-USDC", "APT", "USDC", ["HL", "LGT"]),
      ],
      executionLogs: [
        {
          id: "log-apt",
          timestamp: "2026-10-01T12:00:00Z",
          trade_type: "spread",
          strategy_id: "radar",
          symbol: "APT-USDC",
          venue_a: "HL",
          venue_b: "LGT",
          leg_a_status: "filled",
          leg_b_status: "filled",
          success: true,
          gross_profit: -25.0,
          fees: 1.30,
          net_profit: -26.30,
          trigger_spread_bps: 20.5,
          latency_us: 150,
          note: "",
        },
      ] as any,
    }),
  );

  // Dollar PnL must be negative with text-loss
  assert.match(html, /-\$26\.30/);
  assert.match(html, /class="[^"]*text-loss[^"]*"[^>]*>-\$26\.30<\/span>/);

  // Bps PnL must be strictly negative, styled grey text-muted and smaller font (text-[11px])
  assert.doesNotMatch(html, />\+20\.5 bps<\/span>/, "Must never display positive entry spread as positive Net PnL bps");
  assert.match(html, /class="[^"]*text-\[11px\][^"]*text-muted[^"]*"[^>]*>-\d+(\.\d+)? bps<\/span>/);
  assert.doesNotMatch(html, /class="[^"]*text-loss[^"]*"[^>]*>-\d+(\.\d+)? bps<\/span>/, "bps must not be styled red text-loss");

  // Entry trigger spread is preserved in tooltip / title
  assert.match(html, /entry sprd: \+20\.5 bps/);
});

test("net profit in dollars and bps share matching positive mathematical sign, with dollar in green and bps in grey text-[11px]", () => {
  const html = renderToStaticMarkup(
    React.createElement(TickerExplorer, {
      ...baseProps,
      availableTickers: [
        ticker("BTC-USDC", "BTC", "USDC", ["HL", "LGT"]),
      ],
      executionLogs: [
        {
          id: "log-btc",
          timestamp: "2026-10-01T12:00:00Z",
          trade_type: "spread",
          strategy_id: "radar",
          symbol: "BTC-USDC",
          venue_a: "HL",
          venue_b: "LGT",
          leg_a_status: "filled",
          leg_b_status: "filled",
          success: true,
          gross_profit: 20.0,
          fees: 2.0,
          net_profit: 18.0,
          trigger_spread_bps: 15.0,
          latency_us: 150,
          note: "",
        },
      ] as any,
    }),
  );

  assert.match(html, /\+\$18\.00/);
  assert.match(html, /class="[^"]*text-profit[^"]*"[^>]*>\+\$18\.00<\/span>/);
  // Bps PnL is grey text-muted and text-[11px] (only the dollar Net PnL is color-coded)
  assert.match(html, /class="[^"]*text-\[11px\][^"]*text-muted[^"]*"[^>]*>\+\d+(\.\d+)? bps<\/span>/);
  assert.doesNotMatch(html, /class="[^"]*text-profit[^"]*"[^>]*>\+\d+(\.\d+)? bps<\/span>/);
});

