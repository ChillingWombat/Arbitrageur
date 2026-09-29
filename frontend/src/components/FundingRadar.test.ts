import test from "node:test";
import assert from "node:assert/strict";
import { fundingAutoEligible, splitFundingLegs } from "./FundingRadar.tsx";
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
