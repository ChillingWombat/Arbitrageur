import nodeTest from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  PortfolioMonitor,
  STRATEGY_BUTTONS,
  groupStrategyId,
  matchStrategy,
} from "./PortfolioMonitor.tsx";
import type { StrategyMetrics } from "../types/index.ts";

nodeTest("groupStrategyId unifies any options substrategy into 'options'", () => {
  assert.equal(groupStrategyId("options_parity"), "options");
  assert.equal(groupStrategyId("options_box_spread"), "options");
  assert.equal(groupStrategyId("options_jelly_roll"), "options");
  assert.equal(groupStrategyId("options_convexity"), "options");
  assert.equal(groupStrategyId("options"), "options");

  // Non-options strategies stay unchanged
  assert.equal(groupStrategyId("spatial_spread"), "spatial_spread");
  assert.equal(groupStrategyId("funding_rate"), "funding_rate");
  assert.equal(groupStrategyId("radar"), "radar");
  assert.equal(groupStrategyId("spot"), "spot");
});

nodeTest("matchStrategy with 'options' filter matches all options substrategies", () => {
  // Matches options and all options_*
  assert.equal(matchStrategy("options", "options"), true);
  assert.equal(matchStrategy("options_parity", "options"), true);
  assert.equal(matchStrategy("options_box_spread", "options"), true);
  assert.equal(matchStrategy("options_jelly_roll", "options"), true);
  assert.equal(matchStrategy("options_convexity", "options"), true);

  // Does NOT match non-options strategies
  assert.equal(matchStrategy("spatial_spread", "options"), false);
  assert.equal(matchStrategy("funding_rate", "options"), false);
  assert.equal(matchStrategy("radar", "options"), false);
  assert.equal(matchStrategy("spot", "options"), false);
  assert.equal(matchStrategy("prediction_parity", "options"), false);
  assert.equal(matchStrategy("stat_arb", "options"), false);
});

nodeTest("matchStrategy with other filters does not match options", () => {
  assert.equal(matchStrategy("options_parity", "funding_rate"), false);
  assert.equal(matchStrategy("options_box_spread", "spatial_spread"), false);
  assert.equal(matchStrategy("options", "radar"), false);

  // "all" matches everything
  assert.equal(matchStrategy("options_parity", "all"), true);
  assert.equal(matchStrategy("funding_rate", "all"), true);
});

nodeTest("STRATEGY_BUTTONS provides unified 'Options' and excludes substrategies", () => {
  const optionsButton = STRATEGY_BUTTONS.find((b) => b.id === "options");
  assert.ok(optionsButton, "Options button must exist");
  assert.equal(optionsButton.label, "Options");

  // No separate options substrategies in strategy buttons
  const substrategyIds = STRATEGY_BUTTONS.filter((b) => b.id.startsWith("options_"));
  assert.equal(substrategyIds.length, 0);

  const substrategyLabels = STRATEGY_BUTTONS.filter(
    (b) =>
      b.label === "Options Parity" ||
      b.label === "Box Spread" ||
      b.label === "Jelly Roll" ||
      b.label === "Convexity",
  );
  assert.equal(substrategyLabels.length, 0);
});

nodeTest("PortfolioMonitor renders unified Options button and excludes options substrategies", () => {
  const html = renderToStaticMarkup(
    React.createElement(PortfolioMonitor, {
      riskData: null,
      status: null,
      openPositions: [],
      onResetCircuitBreaker: () => {},
      onTriggerKillswitch: () => {},
    }),
  );

  // Verify Options button is rendered
  assert.match(html, /<span>Options<\/span>/);

  // Verify individual options substrategies are NOT rendered as strategy buttons
  assert.doesNotMatch(html, /<span>Options Parity<\/span>/);
  assert.doesNotMatch(html, /<span>Box Spread<\/span>/);
  assert.doesNotMatch(html, /<span>Jelly Roll<\/span>/);
  assert.doesNotMatch(html, /<span>Convexity<\/span>/);
});

nodeTest("metrics filtering with options matches all options substrategies", () => {
  const metrics: StrategyMetrics[] = [
    {
      strategy_id: "options_parity",
      total_pnl_usd: 120.5,
      total_cycles_completed: 10,
      winning_cycles: 8,
      sharpe_ratio: 1.8,
      annualized_apr_pct: 22.4,
    } as StrategyMetrics,
    {
      strategy_id: "options_box_spread",
      total_pnl_usd: 45.0,
      total_cycles_completed: 5,
      winning_cycles: 4,
      sharpe_ratio: 1.5,
      annualized_apr_pct: 18.0,
    } as StrategyMetrics,
    {
      strategy_id: "funding_rate",
      total_pnl_usd: 200.0,
      total_cycles_completed: 20,
      winning_cycles: 18,
      sharpe_ratio: 2.1,
      annualized_apr_pct: 35.0,
    } as StrategyMetrics,
  ];

  const filtered = metrics.filter(
    (m) => m.strategy_id === "options" || m.strategy_id.startsWith("options_"),
  );
  assert.equal(filtered.length, 2);
  assert.equal(
    filtered.reduce((sum, m) => sum + m.total_pnl_usd, 0),
    165.5,
  );
  assert.equal(
    filtered.reduce((sum, m) => sum + m.total_cycles_completed, 0),
    15,
  );
});

nodeTest("PortfolioMonitor renders Capital Allocation with By Venue and By Strategy switch tabs", () => {
  const html = renderToStaticMarkup(
    React.createElement(PortfolioMonitor, {
      riskData: null,
      status: null,
      openPositions: [],
      onResetCircuitBreaker: () => {},
      onTriggerKillswitch: () => {},
    }),
  );

  assert.match(html, /<span>Capital Allocation<\/span>/);
  assert.match(html, />By Venue<\/button>/);
  assert.match(html, />By Strategy<\/button>/);
});

nodeTest("PortfolioMonitor renders Stopped rather than Paused when engine is stopped after emergency exit", () => {
  const stoppedStatus = {
    is_live_mode: false,
    environment: "testnet",
    is_paused: true,
    is_stopped: true,
    total_realized_pnl: 0,
    total_trades_count: 0,
    successful_trades_count: 0,
    circuit_breaker_status: "ACTIVE",
    circuit_breaker_tripped: false,
    current_drawdown_usd: 0,
    max_drawdown_usd: 1000,
    consecutive_failures: 0,
    active_venues: [],
  };
  const html = renderToStaticMarkup(
    React.createElement(PortfolioMonitor, {
      riskData: null,
      status: stoppedStatus,
      openPositions: [],
      onResetCircuitBreaker: () => {},
      onTriggerKillswitch: () => {},
    }),
  );

  assert.match(html, />Stopped<\/span>/, "verdict badge must render Stopped");
  assert.match(html, /Engine stopped/, "subtext must render Engine stopped");
  assert.doesNotMatch(html, />Paused<\/span>/, "verdict badge must never render Paused when stopped");
  assert.doesNotMatch(html, /Engine paused/, "subtext must never render Engine paused when stopped");
});
