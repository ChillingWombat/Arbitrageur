import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { AutoBotGadget } from "./AutoBotGadget.tsx";
import { defaultRadarBotConfig } from "../services/radarBot.ts";
import type { RadarBotStatusDto } from "../types/index.ts";

const status = (overrides?: Partial<RadarBotStatusDto>): RadarBotStatusDto => ({
  enabled: true,
  mode: "auto",
  openPositions: [],
  entersToday: 3,
  exitsToday: 1,
  skipsToday: 9,
  lastDecision: null,
  lastError: null,
  auditErrors: 0,
  monitorRunning: true,
  snapshotPresent: true,
  ...overrides,
});

const noop = () => {};

test("single control renders unified buttons and counters", () => {
  const html = renderToStaticMarkup(
    React.createElement(AutoBotGadget, {
      cfg: { ...defaultRadarBotConfig(), enabled: true, mode: "auto" },
      status: status(),
      flattening: false,
      onMode: noop,
      onFlatten: noop,
    }),
  );
  assert.match(html, /Auto-Bot/);
  assert.match(html, /Start/);
  assert.match(html, /Pause/);
  assert.match(html, /Stop/);
  assert.match(html, /ARMED/);
  assert.match(html, /in <b[^>]*>3<\/b>/);
});

test("mode buttons disable until the backend config loads", () => {
  const html = renderToStaticMarkup(
    React.createElement(AutoBotGadget, {
      cfg: null,
      status: null,
      flattening: false,
      onMode: noop,
      onFlatten: noop,
    }),
  );
  assert.match(html, /disabled/);
});

test("stop arms with open positions and disarms when inert", () => {
  const openPosition = {
    symbol: "BTC",
    buyVenue: "hyperliquid",
    sellVenue: "lighter",
    entryTsMs: 1,
    entryDevBps: 10,
    baselineBps: 2,
    sizeUsd: 100,
    deadlineMs: 2,
    liveDevBps: null,
    liveStatus: null,
    unwatched: false,
  };
  const countDisabled = (html: string) => (html.match(/disabled=""/g) ?? []).length;
  const render = (mode: "off" | "auto", open: typeof openPosition[]) =>
    renderToStaticMarkup(
      React.createElement(AutoBotGadget, {
        cfg: { ...defaultRadarBotConfig(), mode, enabled: mode === "auto" },
        status: status({ openPositions: open }),
        flattening: false,
        onMode: noop,
        onFlatten: noop,
      }),
    );
  assert.equal(countDisabled(render("off", [openPosition])), 0, "stop stays armed with open positions");
  assert.equal(countDisabled(render("off", [])), 1, "only stop disables when off and flat");
  assert.equal(countDisabled(render("auto", [])), 1, "already armed disables start");
});

test("paused backend mode shows a draining badge", () => {
  const html = renderToStaticMarkup(
    React.createElement(AutoBotGadget, {
      cfg: { ...defaultRadarBotConfig(), enabled: false, mode: "paused" },
      status: status(),
      flattening: false,
      onMode: noop,
      onFlatten: noop,
    }),
  );
  assert.match(html, /Paused/);
  assert.match(html, /bg-warn-dim/);
});

test("radar page consistency: BotLifecycleControls on top ribbon next to configurator", () => {
  const source = readFileSync(
    join(process.cwd(), "src/components/OpportunityRadar.tsx"),
    "utf-8",
  );
  assert.match(source, /BotLifecycleControls/);
  assert.doesNotMatch(source, /<AutoBotGadget/);
});
