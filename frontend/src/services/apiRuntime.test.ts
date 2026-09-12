import { test } from "node:test";
import assert from "node:assert/strict";
import { api, invokeDesktopOrDemo, nativeCommandAvailable } from "./api.ts";

test("invokeDesktopOrDemo calls demo when core is null and returns its value", async () => {
  let demoCalled = false;
  const result = await invokeDesktopOrDemo(
    "fetch_orderbooks",
    { symbol: "BTC-USDC" },
    () => {
      demoCalled = true;
      return "demo-value";
    },
    async () => null,
  );
  assert.equal(result, "demo-value");
  assert.equal(demoCalled, true);
});

test("invokeDesktopOrDemo propagates desktop invoke rejection and never calls demo", async () => {
  const failure = new Error("ipc invoke failed");
  let demoCalled = false;
  const core = {
    invoke: async <R>(_command: string, _args?: Record<string, unknown>): Promise<R> => {
      throw failure;
    },
  };
  await assert.rejects(
    invokeDesktopOrDemo(
      "fetch_orderbooks",
      undefined,
      () => {
        demoCalled = true;
        return "demo";
      },
      async () => core,
    ),
    (err: unknown) => err === failure,
  );
  assert.equal(demoCalled, false);
});

test("invokeDesktopOrDemo propagates core loader failure", async () => {
  const failure = new Error("tauri core import failed");
  let demoCalled = false;
  await assert.rejects(
    invokeDesktopOrDemo(
      "get_system_status",
      undefined,
      () => {
        demoCalled = true;
        return "demo";
      },
      async () => {
        throw failure;
      },
    ),
    (err: unknown) => err === failure,
  );
  assert.equal(demoCalled, false);
});

test("browser demo setLiveMode(true) rejects and setLiveMode(false) returns false", async () => {
  await assert.rejects(api.setLiveMode(true), /Live execution is unavailable in browser demo mode/);
  assert.equal(await api.setLiveMode(false), false);
});

test("browser demo deleteExchangeCredentials rejects", async () => {
  await assert.rejects(
    api.deleteExchangeCredentials("hyperliquid"),
    /Secure credential storage is unavailable in browser demo mode/,
  );
});

const REGISTERED_NATIVE_COMMANDS = [
  "get_system_status",
  "fetch_orderbooks",
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
