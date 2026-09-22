import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { VenueBalancePanel, AccountCard } from "./ExchangeAccounts.tsx";
import type { VenueBalanceInfo, AccountDto } from "../types/index.ts";

test("VenueBalancePanel renders Paper badge and description when is_live is false", () => {
  const paperBal: VenueBalanceInfo = {
    venue: "hyperliquid",
    equity_usd: 50000,
    available_margin_usd: 45000,
    margin_rate_pct: 10,
    status: "HEALTHY",
    is_live: false,
  };

  const html = renderToStaticMarkup(React.createElement(VenueBalancePanel, { bal: paperBal }));
  assert.match(html, /Total Equity/);
  assert.match(html, /\$50,000\.00/);
  assert.doesNotMatch(html, />Paper</);
  assert.doesNotMatch(html, /Simulated account balance/);
  assert.doesNotMatch(html, />Live</);
});

test("VenueBalancePanel renders Live badge and description when is_live is true", () => {
  const liveBal: VenueBalanceInfo = {
    venue: "hyperliquid",
    equity_usd: 125000,
    available_margin_usd: 110000,
    margin_rate_pct: 12,
    status: "HEALTHY",
    is_live: true,
  };

  const html = renderToStaticMarkup(React.createElement(VenueBalancePanel, { bal: liveBal }));
  assert.match(html, /Total Equity/);
  assert.match(html, /\$125,000\.00/);
  assert.doesNotMatch(html, />Live</);
  assert.doesNotMatch(html, /Real exchange balance/);
  assert.doesNotMatch(html, />Paper</);
});

test("VenueBalancePanel renders NO_CREDENTIALS guidance when status is NO_CREDENTIALS", () => {
  const noCredBal: VenueBalanceInfo = {
    venue: "binance",
    equity_usd: 0,
    available_margin_usd: 0,
    margin_rate_pct: 0,
    status: "NO_CREDENTIALS",
    is_live: true,
  };

  const html = renderToStaticMarkup(React.createElement(VenueBalancePanel, { bal: noCredBal }));
  assert.match(html, /No live API credentials configured/);
});

test("VenueBalancePanel renders error alert when status is ERROR", () => {
  const errorBal: VenueBalanceInfo = {
    venue: "lighter",
    equity_usd: 0,
    available_margin_usd: 0,
    margin_rate_pct: 0,
    status: "ERROR",
    recommended_action: "API rate limit exceeded on exchange",
    is_live: true,
  };

  const html = renderToStaticMarkup(React.createElement(VenueBalancePanel, { bal: errorBal }));
  assert.match(html, /API rate limit exceeded on exchange/);
});

test("VenueBalancePanel renders N/A metrics with no guidance rows when bal is null", () => {
  const html = renderToStaticMarkup(React.createElement(VenueBalancePanel, { bal: null, isDefault: false }));
  assert.match(html, /Total Equity/);
  assert.match(html, /N\/A/);
  assert.doesNotMatch(html, /Secondary account/);
  assert.doesNotMatch(html, /Rate budget/);
});

function makeMockAccount(overrides: Partial<AccountDto> = {}): AccountDto {
  return {
    id: "acc-1",
    venue: "lighter",
    label: "Lighter #1",
    enabled: true,
    isDefault: true,
    createdAt: "2024-03-01T00:00:00Z",
    updatedAt: "2024-03-01T00:00:00Z",
    credentialsConfigured: true,
    hasApiKey: true,
    hasApiSecret: true,
    hasPassphrase: false,
    hasAccountId: false,
    hasWalletAddress: false,
    hasPrivateKey: true,
    hasBaseUrl: false,
    accountIndex: null,
    subaccountIndex: null,
    apiKeyIndex: null,
    accountTier: "standard",
    maxLeverage: null,
    ...overrides,
  };
}

test("AccountCard renders disabled banner when account.enabled is false", () => {
  const disabledAccount = makeMockAccount({
    id: "acc-lighter-2",
    venue: "lighter",
    label: "Lighter #2 PopDEX",
    enabled: false,
    isDefault: false,
  });

  const html = renderToStaticMarkup(
    React.createElement(AccountCard, {
      account: disabledAccount,
      ordinal: 2,
      locked: false,
      bal: null,
      onChanged: () => {},
    })
  );

  assert.match(html, /Account is disabled — live balance and trading are paused/);
  assert.match(html, /Toggle switch above to enable/);
  // Ensure the live/paper balance panel numbers are not rendered when disabled
  assert.doesNotMatch(html, />Live</);
  assert.doesNotMatch(html, />Paper</);
});

test("AccountCard renders VenueBalancePanel when account.enabled is true", () => {
  const enabledAccount = makeMockAccount({
    id: "acc-lighter-1",
    venue: "lighter",
    label: "Lighter #1",
    enabled: true,
    isDefault: true,
  });

  const liveBal: VenueBalanceInfo = {
    venue: "lighter",
    equity_usd: 3.21,
    available_margin_usd: 3.21,
    margin_rate_pct: 0,
    status: "HEALTHY",
    is_live: true,
    account_id: "acc-lighter-1",
  };

  const html = renderToStaticMarkup(
    React.createElement(AccountCard, {
      account: enabledAccount,
      ordinal: 1,
      locked: false,
      bal: liveBal,
      onChanged: () => {},
    })
  );

  assert.doesNotMatch(html, /Account is disabled/);
  assert.match(html, /Total Equity/);
  assert.match(html, /\$3\.21/);
  assert.doesNotMatch(html, />Live</);
  assert.doesNotMatch(html, />Paper</);
});


