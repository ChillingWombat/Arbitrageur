import test from "node:test";
import assert from "node:assert/strict";
import {
  EXCHANGES_ALPHABETICAL,
  EXCHANGE_GROUP_ORDER,
  filterSidebarExchanges,
  groupExchangesBySection,
} from "./EngineSettings.tsx";
import { EVM_AMM_VENUE_IDS } from "./ExchangeAccounts.tsx";
import { venueHasTestnet } from "../types/tickers";

test("EVM Wallet entry exists and its group pins first in the sidebar", () => {
  const wallet = EXCHANGES_ALPHABETICAL.find((ex) => ex.id === "evm_wallet");
  assert.ok(wallet, "evm_wallet metadata entry missing");
  assert.equal(wallet.group, "EVM Wallet");
  assert.equal(EXCHANGE_GROUP_ORDER[0], "EVM Wallet");

  const sections = groupExchangesBySection(EXCHANGES_ALPHABETICAL);
  assert.equal(sections[0].group, "EVM Wallet");
  assert.deepEqual(
    sections[0].venues.map((v) => v.id),
    ["evm_wallet"],
  );
});

test("individual EVM AMM venues keep metadata but leave the sidebar group list", () => {
  // The 11 AMM ids remain in EXCHANGES_ALPHABETICAL for name/type lookups
  // — they are filtered from visibleExchanges, not deleted here. (Fees are
  // not display metadata: every fee surface reads `get_venue_fees`.)
  for (const id of EVM_AMM_VENUE_IDS) {
    assert.ok(
      EXCHANGES_ALPHABETICAL.some((ex) => ex.id === id),
      `${id} metadata missing`,
    );
  }
});

test("testnet sidebar lists only venues with a real demo endpoint", () => {
  const visible = filterSidebarExchanges(EXCHANGES_ALPHABETICAL, {
    tradingMode: "testnet",
    exchangeFilter: "all",
    favoriteExchanges: [],
  });
  assert.ok(visible.length > 0, "expected at least one testnet venue");
  for (const ex of visible) {
    assert.ok(
      venueHasTestnet(ex.id),
      `${ex.id} is live-only but surfaced in Testnet mode`,
    );
    assert.ok(!EVM_AMM_VENUE_IDS.has(ex.id), `${ex.id} is an AMM leaf`);
  }
  // Spot-check known live-only venues are gone in Testnet mode.
  const ids = new Set<string>(visible.map((ex) => ex.id));
  for (const liveOnly of ["arcus", "coinbase", "aster", "evm_wallet", "variational"]) {
    assert.ok(!ids.has(liveOnly), `${liveOnly} must hide in Testnet mode`);
  }
  for (const tn of ["hyperliquid", "alpaca", "deribit", "binance"]) {
    assert.ok(ids.has(tn), `${tn} should stay visible in Testnet mode`);
  }
});

test("live sidebar lists every non-AMM venue regardless of testnet support", () => {
  const visible = filterSidebarExchanges(EXCHANGES_ALPHABETICAL, {
    tradingMode: "live",
    exchangeFilter: "all",
    favoriteExchanges: [],
  });
  const ids = new Set(visible.map((ex) => ex.id));
  assert.ok(ids.has("arcus") && ids.has("evm_wallet") && ids.has("coinbase"));
});

test("favorites filter cannot resurrect a live-only venue in testnet", () => {
  const visible = filterSidebarExchanges(EXCHANGES_ALPHABETICAL, {
    tradingMode: "testnet",
    exchangeFilter: "favorites",
    favoriteExchanges: ["arcus", "binance"],
  });
  const ids = visible.map((ex) => ex.id);
  assert.deepEqual(ids, ["binance"]);
});
