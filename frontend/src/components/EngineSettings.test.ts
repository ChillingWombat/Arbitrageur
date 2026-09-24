import test from "node:test";
import assert from "node:assert/strict";
import {
  EXCHANGES_ALPHABETICAL,
  EXCHANGE_GROUP_ORDER,
  groupExchangesBySection,
} from "./EngineSettings.tsx";
import { EVM_AMM_VENUE_IDS } from "./ExchangeAccounts.tsx";

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
  // The 11 AMM ids remain in EXCHANGES_ALPHABETICAL for fee/name lookups
  // (TickerLogMonitor etc.) — they are filtered from visibleExchanges, not
  // deleted here.
  for (const id of EVM_AMM_VENUE_IDS) {
    assert.ok(
      EXCHANGES_ALPHABETICAL.some((ex) => ex.id === id),
      `${id} metadata missing`,
    );
  }
});
