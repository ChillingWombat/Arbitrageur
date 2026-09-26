import test from "node:test";
import assert from "node:assert/strict";
import {
  botArmed,
  botBadgeTone,
  botControlsDisabled,
  botFlattenAvailable,
  botPairKey,
  buildHeldSet,
  defaultRadarBotConfig,
  describeBotMode,
  isRowHeld,
  resolveRadarExecSize,
} from "./radarBot.ts";
import type { BotOpenPositionDto, MonitorRowDto } from "../types/index.ts";

const pos = (symbol: string, buyVenue: string, sellVenue: string): BotOpenPositionDto => ({
  symbol,
  buyVenue,
  sellVenue,
  entryTsMs: 1,
  entryDevBps: 10,
  baselineBps: 2,
  sizeUsd: 100,
  deadlineMs: 2,
  liveDevBps: 9,
  liveStatus: "ready",
  unwatched: false,
});

const row = (symbol: string, buyVenue: string, sellVenue: string): MonitorRowDto => ({
  symbol,
  buyVenue,
  sellVenue,
  status: "ready",
  currentEdgeBps: 12,
  baselineBps: 2,
  deviationBps: 10,
  netCaptureBps: 6,
  pWin: 0.95,
  expectedProfitBps: 5.7,
  expectedProfitUsd: 57,
  optimalSizeUsd: 1000,
  breakevenSizeUsd: 800,
  
  fundingCarryBps: 0.1,
  fundingUnknown: false,
  firstSeenMs: 1,
  lastSeenMs: 2,
  signalAgeSecs: 60,
  expectedConvergeSecs: 120,
  qualificationProgress: 1,
  observedEpisodes: 8,
  executable: true,
  blockReason: null,
  thresholdRatio: 0.8,
  coverage: 1.0,
  sessionId: "overlap",
  sessionProvisional: false,
  sessionAllow: 1,
  sessionSizeMult: 1,
});

test("botPairKey normalizes case and whitespace", () => {
  assert.equal(botPairKey("btc", "Hyperliquid", "LIGHTER"), "BTC:hyperliquid:lighter");
  assert.equal(botPairKey("  BTC ", " hyperliquid ", " lighter "), "BTC:hyperliquid:lighter");
});

test("held markers match only real open positions, case-insensitively", () => {
  const held = buildHeldSet([pos("BTC", "hyperliquid", "lighter")]);
  assert.equal(isRowHeld(row("BTC", "hyperliquid", "lighter"), held), true);
  assert.equal(isRowHeld(row("btc", "HYPERLIQUID", "Lighter"), held), true);
  assert.equal(isRowHeld(row("BTC", "lighter", "hyperliquid"), held), false);
  assert.equal(isRowHeld(row("ETH", "hyperliquid", "lighter"), held), false);
  assert.equal(isRowHeld(row("BTC", "hyperliquid", "lighter"), buildHeldSet([])), false);
  assert.equal(isRowHeld(row("BTC", "hyperliquid", "lighter"), buildHeldSet(null)), false);
});

test("botArmed requires explicit opt-in to full-auto", () => {
  const base = defaultRadarBotConfig();
  assert.equal(botArmed(base), false);
  assert.equal(botArmed({ ...base, enabled: true, mode: "paused" }), false);
  assert.equal(botArmed({ ...base, enabled: true, mode: "reduce_only" }), false);
  assert.equal(botArmed({ ...base, enabled: false, mode: "auto" }), false);
  assert.equal(botArmed({ ...base, enabled: true, mode: "auto" }), true);
  assert.equal(botArmed(null), false);
});

test("default config ships inert", () => {
  const d = defaultRadarBotConfig();
  assert.equal(d.enabled, false);
  assert.equal(d.mode, "off");
});

test("describeBotMode never fabricates labels", () => {
  assert.equal(describeBotMode("auto"), "Auto");
  assert.equal(describeBotMode("reduce_only"), "Reduce-Only");
  assert.equal(describeBotMode("bogus"), "—");
  assert.equal(describeBotMode(null), "—");
  assert.equal(describeBotMode(undefined), "—");
});

test("single control: mode buttons inert until backend config loads", () => {
  assert.equal(botControlsDisabled(null), true);
  assert.equal(botControlsDisabled(undefined), true);
  assert.equal(botControlsDisabled(defaultRadarBotConfig()), false);
});

test("single control: flatten armed while risk may exist, off when inert", () => {
  // Open managed positions always arm the emergency exit.
  assert.equal(botFlattenAvailable(2, "off"), true);
  assert.equal(botFlattenAvailable(1, "auto"), true);
  // No positions but a live backend mode may still hold ledger risk.
  assert.equal(botFlattenAvailable(0, "auto"), true);
  assert.equal(botFlattenAvailable(0, "paused"), true);
  assert.equal(botFlattenAvailable(0, "reduce_only"), true);
  // Fully inert: off + flat means nothing to wind down.
  assert.equal(botFlattenAvailable(0, "off"), false);
  assert.equal(botFlattenAvailable(0, null), false);
  // Garbage counts never arm.
  assert.equal(botFlattenAvailable(NaN, "auto"), false);
  assert.equal(botFlattenAvailable(-1, "auto"), false);
});

test("single control: badge tone follows backend mode, not page-local phase", () => {
  const base = defaultRadarBotConfig();
  assert.equal(botBadgeTone({ ...base, enabled: true, mode: "auto" }), "armed");
  assert.equal(botBadgeTone({ ...base, enabled: false, mode: "auto" }), "inert");
  assert.equal(botBadgeTone({ ...base, enabled: true, mode: "paused" }), "draining");
  assert.equal(botBadgeTone({ ...base, enabled: false, mode: "reduce_only" }), "draining");
  assert.equal(botBadgeTone({ ...base, enabled: false, mode: "off" }), "inert");
  assert.equal(botBadgeTone(null), "inert");
});

test("radar exec size never fabricates a notional when optimal size is unknown", () => {
  assert.equal(resolveRadarExecSize(undefined, null), null);
  assert.equal(resolveRadarExecSize(undefined, undefined), null);
  assert.equal(resolveRadarExecSize(undefined, 0), null);
  assert.equal(resolveRadarExecSize(undefined, -50), null);
  assert.equal(resolveRadarExecSize(undefined, NaN), null);
  assert.equal(resolveRadarExecSize(undefined, Infinity), null);
  assert.equal(resolveRadarExecSize(0, null), null);
  assert.equal(resolveRadarExecSize(undefined, 2500), 2500);
  assert.equal(resolveRadarExecSize(750, null), 750);
});
