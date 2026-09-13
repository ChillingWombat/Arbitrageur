import { test } from "node:test";
import assert from "node:assert/strict";
import {
  api,
  invokeDesktop,
  nativeCommandAvailable,
  normalizeTickerStrategyConfig,
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
  "fetch_orderbooks",
  "get_ticker_volumes",
  "get_signal_series",
  "scan_spatial_opportunities",
  "scan_spot_opportunities",
  "execute_spot_arbitrage",
  "scan_funding_opportunities",
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
  "execute_spatial_candidates",
  "list_signal_executions",
  "get_ticker_position_state",
  "get_venue_balances",
  "get_strategy_performance",
  "run_wfo_calibration",
];

test("nativeCommandAvailable mirrors the registered desktop command set", () => {
  for (const cmd of REGISTERED_NATIVE_COMMANDS) {
    assert.equal(nativeCommandAvailable(cmd), true, `${cmd} must be a registered native command`);
  }
  assert.equal(nativeCommandAvailable("scan_triangle_opportunities"), false);
  assert.equal(nativeCommandAvailable("execute_triangle_arbitrage"), false);
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
});

test("normalizeTickerStrategyConfig defaults unknown windows to 1m", () => {
  assert.equal(
    normalizeTickerStrategyConfig({ slidingWindow: "99x" }).slidingWindow,
    "1m",
  );
  assert.equal(normalizeTickerStrategyConfig(null).slidingWindow, "1m");
  assert.equal(normalizeTickerStrategyConfig({}).slidingWindow, "1m");
});

test("triangle scan and execution fail closed in desktop mode", async () => {
  const g = globalThis as Record<string, unknown>;
  const prev = g.window;
  g.window = { __TAURI_INTERNALS__: {} };
  try {
    const scan = await api.scanTriangleOpportunities("BTC");
    assert.deepEqual(scan, []);
    await assert.rejects(
      api.executeTriangleArbitrage({
        id: "tri-1",
        venue: "hyperliquid",
        path: "BTC>ETH>USDC",
        trade_size_usd: 1000,
        min_profit_bps: 5,
      }),
      /three-leg coordinator/,
    );
  } finally {
    if (prev === undefined) delete g.window;
    else g.window = prev;
  }
});
