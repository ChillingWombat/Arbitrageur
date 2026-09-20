import { test } from "node:test";
import assert from "node:assert/strict";
import {
  api,
  invokeDesktop,
  nativeCommandAvailable,
  normalizeTickerStrategyConfig,
  normalizeStrategyParams,
  normalizeStrategyTicker,
  normalizeEquityEvent,
  DESKTOP_REQUIRED_ERROR,
} from "./api.ts";

test("invokeDesktop throws DESKTOP_REQUIRED_ERROR when core is null", async () => {
  await assert.rejects(
    invokeDesktop("fetch_orderbooks", { symbol: "BTC-USDC" }, async () => null),
    (err: unknown) =>
      err instanceof Error && err.message === DESKTOP_REQUIRED_ERROR,
  );
});

test("invokeDesktop returns the desktop invoke result", async () => {
  const core = {
    invoke: async <R>(_command: string, _args?: Record<string, unknown>): Promise<R> =>
      "desktop-value" as R,
  };
  const result = await invokeDesktop(
    "fetch_orderbooks",
    { symbol: "BTC-USDC" },
    async () => core,
  );
  assert.equal(result, "desktop-value");
});

test("invokeDesktop propagates desktop invoke rejection", async () => {
  const failure = new Error("ipc invoke failed");
  const core = {
    invoke: async <R>(_command: string, _args?: Record<string, unknown>): Promise<R> => {
      throw failure;
    },
  };
  await assert.rejects(
    invokeDesktop("fetch_orderbooks", undefined, async () => core),
    (err: unknown) => err === failure,
  );
});

test("invokeDesktop propagates core loader failure", async () => {
  const failure = new Error("tauri core import failed");
  await assert.rejects(
    invokeDesktop("get_system_status", undefined, async () => {
      throw failure;
    }),
    (err: unknown) => err === failure,
  );
});

test("api.setLiveMode rejects without the desktop application", async () => {
  await assert.rejects(api.setLiveMode(true), new RegExp(DESKTOP_REQUIRED_ERROR));
  await assert.rejects(api.setLiveMode(false), new RegExp(DESKTOP_REQUIRED_ERROR));
});

test("api.deleteExchangeCredentials rejects without the desktop application", async () => {
  await assert.rejects(
    api.deleteExchangeCredentials("hyperliquid"),
    new RegExp(DESKTOP_REQUIRED_ERROR),
  );
});

const REGISTERED_NATIVE_COMMANDS = [
  "get_system_status",
  "refresh_venue_latencies",
  "get_market_universe",
  "fetch_orderbooks",
  "get_ticker_volumes",
  "get_signal_series",
  "get_matrix_baselines",
  "scan_spatial_opportunities",
  "scan_triangle_opportunities",
  "execute_triangle_arbitrage",
  "check_spatial_convergence",
  "scan_spot_opportunities",
  "execute_spot_arbitrage",
  "scan_funding_opportunities",
  "get_funding_trend",
  "scan_prediction_opportunities",
  "run_stat_arb_step",
  "compute_optimal_thresholds",
  "fit_ou_process",
  "execute_spatial_arbitrage",
  "execute_cross_ticker_arbitrage",
  "fetch_multi_market_quotes",
  "execute_funding_carry",
  "execute_prediction_arbitrage",
  "get_risk_overview",
  "reset_circuit_breaker",
  "trigger_emergency_killswitch",
  "get_execution_logs",
  "set_live_mode",
  "set_engine_paused",
  "get_engine_settings",
  "save_engine_settings",
  "get_ticker_strategy_config",
  "save_ticker_strategy_config",
  "get_all_ticker_strategy_configs",
  "get_vault_status",
  "initialize_vault",
  "unlock_vault",
  "lock_vault",
  "delete_exchange_credentials",
  "delete_account_credentials",
  "get_account_credentials",
  "get_account_credential_field",
  "execute_spatial_candidates",
  "list_signal_executions",
  "get_ticker_position_state",
  "get_venue_balances",
  "get_external_positions",
  "recommend_external_close",
  "close_external_position",
  "get_strategy_performance",
  "get_strategy_fill_pnl",
  "get_utilisation_history",
  "run_wfo_calibration",
  "apply_autotune_selected",
  "run_seasonality_promotion",
  "get_session_profile",
];

test("nativeCommandAvailable mirrors the registered desktop command set", () => {
  for (const cmd of REGISTERED_NATIVE_COMMANDS) {
    assert.equal(nativeCommandAvailable(cmd), true, `${cmd} must be a registered native command`);
  }
  assert.equal(nativeCommandAvailable("nonexistent_command_xyz"), false);
});

test("normalizeTickerStrategyConfig preserves the backend sliding window", () => {
  assert.equal(
    normalizeTickerStrategyConfig({ slidingWindow: "5m" }).slidingWindow,
    "5m",
  );
  assert.equal(
    normalizeTickerStrategyConfig({ sliding_window: "30s" }).slidingWindow,
    "30s",
  );
  assert.equal(
    normalizeTickerStrategyConfig({ slidingWindow: "6h" }).slidingWindow,
    "6h",
  );
  assert.equal(
    normalizeTickerStrategyConfig({ slidingWindow: "2d" }).slidingWindow,
    "2d",
  );
});

test("normalizeTickerStrategyConfig defaults unknown windows to 24h", () => {
  assert.equal(
    normalizeTickerStrategyConfig({ slidingWindow: "99x" }).slidingWindow,
    "24h",
  );
  assert.equal(
    normalizeTickerStrategyConfig({ slidingWindow: "1.5h" }).slidingWindow,
    "24h",
  );
  assert.equal(normalizeTickerStrategyConfig(null).slidingWindow, "24h");
  assert.equal(normalizeTickerStrategyConfig({}).slidingWindow, "24h");
});

test("triangle scan/execute and convergence check fail closed without the desktop app", async () => {
  const g = globalThis as Record<string, unknown>;
  const prev = g.window;
  delete g.window;
  try {
    await assert.rejects(
      api.scanTriangleOpportunities("BTC"),
      new RegExp(DESKTOP_REQUIRED_ERROR),
    );
    await assert.rejects(
      api.executeTriangleArbitrage({
        id: "lighter-TRI-BTC-ETH-USDC",
        venue: "Lighter",
        path: "USDC → BTC → ETH → USDC",
        trade_size_usd: 1000,
        min_profit_bps: 5,
      }),
      new RegExp(DESKTOP_REQUIRED_ERROR),
    );
    await assert.rejects(
      api.checkSpatialConvergence("BTC-USDC", "futures"),
      new RegExp(DESKTOP_REQUIRED_ERROR),
    );
  } finally {
    if (prev === undefined) delete g.window;
    else g.window = prev;
  }
});

test("normalizeStrategyParams maps the new sizing fields", () => {
  const p = normalizeStrategyParams({
    strategyId: "futures",
    targetProfitBps: 1.5,
    windowPercentile: 80,
    slidingWindow: "12h",
    slidingWindowMode: "auto",
    slidingWindowSecs: 43200,
    stepSizeMult: 5,
    maxStepUsd: 1000,
    maxTickerEquityPct: 3,
    executionMode: "DOUBLE_WAY",
    userTargetLeverage: 3,
    fundingTradeSizeUsd: 25000,
    fundingMinAprPct: 8,
    fundingHoldingDays: 14,
    spotTradeSizeUsd: 1000,
    spotMaxSlippageBps: 5,
    spotMinNetSpreadBps: 15,
  });
  assert.equal(p.stepSizeMult, 5);
  assert.equal(p.maxStepUsd, 1000);
  assert.equal(p.maxTickerEquityPct, 3);
});

test("normalizeStrategyParams defaults missing sizing fields for backward load", () => {
  const p = normalizeStrategyParams({
    strategyId: "futures",
    unitSizeUsd: 2500,
    maxAccumulatedUnits: 3,
  });
  assert.equal(p.stepSizeMult, 10);
  assert.equal(p.maxStepUsd, 2500);
  assert.equal(p.maxTickerEquityPct, 20);
  assert.equal(normalizeStrategyParams(null).stepSizeMult, 10);
});

test("normalizeStrategyParams carries the auto-filter thresholds (None/0 = off)", () => {
  const p = normalizeStrategyParams({
    strategyId: "futures",
    autoMinVolumeUsd: 250000,
    autoMinPriceUsd: 0,
    auto_min_event_volume_usd: 500000,
  });
  assert.equal(p.autoMinVolumeUsd, 250000);
  assert.equal(p.autoMinPriceUsd, null);
  assert.equal(p.autoMinEventVolumeUsd, 500000);
  const legacy = normalizeStrategyParams({ strategyId: "spot" });
  assert.equal(legacy.autoMinVolumeUsd, null);
  assert.equal(legacy.autoMinPriceUsd, null);
  assert.equal(legacy.autoMinEventVolumeUsd, null);
  assert.equal(normalizeStrategyParams(null).autoMinVolumeUsd, null);
});

test("native commands include the strategy ticker and equity event getters", () => {
  assert.equal(nativeCommandAvailable("get_strategy_tickers"), true);
  assert.equal(nativeCommandAvailable("get_account_equity_events"), true);
});

test("normalizeStrategyTicker maps pass flags and snake_case reasons", () => {
  const ok = normalizeStrategyTicker({ base: "btc", venues: ["hyperliquid"], passes: true });
  assert.deepEqual(ok, { base: "BTC", venues: ["hyperliquid"], passes: true, blockReason: null });
  const blocked = normalizeStrategyTicker({
    base: "PEPE",
    venues: [],
    passes: false,
    block_reason: "PEPE blocked by auto volume filter",
  });
  assert.equal(blocked.passes, false);
  assert.equal(blocked.blockReason, "PEPE blocked by auto volume filter");
});

test("normalizeEquityEvent maps snake_case IPC rows", () => {
  const e = normalizeEquityEvent({
    id: 7,
    account_id: "lighter",
    ts_ms: 123,
    expected: 100,
    actual: 105,
    delta: 5,
    kind: "deposit",
    note: "",
  });
  assert.deepEqual(e, {
    id: 7,
    accountId: "lighter",
    tsMs: 123,
    expected: 100,
    actual: 105,
    delta: 5,
    kind: "deposit",
    note: "",
  });
});

test("strategy ticker and equity event IPC fail closed without desktop", async () => {
  await assert.rejects(api.getStrategyTickers("futures"), new RegExp(DESKTOP_REQUIRED_ERROR));
  await assert.rejects(api.getAccountEquityEvents(), new RegExp(DESKTOP_REQUIRED_ERROR));
});
