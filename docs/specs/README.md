# Formal System Specifications & Architecture RFCs

This directory contains the formal engineering specifications, architectural RFCs, and user-story blueprints for the Arbitrageur trading terminal and autonomous execution engine.

> [!NOTE]
> All specifications listed below have been fully implemented, verified with comprehensive automated test suites (Rust desktop unit/integration tests and Node component tests), and shipped to `main`.

---

## Specification Index

### 1. Risk, Safety & Execution Lifecycle
- [`spec_probability_driven_invalidation_exit_architecture.md`](spec_probability_driven_invalidation_exit_architecture.md) — **Probability-Driven Statistical Invalidation Exit Architecture & Dynamic Risk Decoupling**: Eliminates arbitrary wall-clock timers in favor of real-time Ornstein-Uhlenbeck mean-reversion probability ($P_{\text{win}}$) decay gating (`ExitTrigger::ProbabilityDecayed`). *(GitHub Issue #100)*
- [`spec_emergency_stop_all_and_atomic_leg_unwind.md`](spec_emergency_stop_all_and_atomic_leg_unwind.md) — **Emergency Stop-All Hardening & Atomic Leg Unwind**: Terminal order cancellation, flatten deadlock elimination, and shared-symbol dust isolation. *(GitHub Issue #94)*
- [`spec_engine_stopped_vs_paused_lifecycle.md`](spec_engine_stopped_vs_paused_lifecycle.md) — **Engine Lifecycle Disambiguation**: Clear state machine separating `Stopped` (all background processes killed, requires fresh start) from `Paused` (feeds alive, order placement suspended, resumeable). *(GitHub Issue #96)*
- [`SPEC_SAFETY_UI_PROFITABILITY.md`](SPEC_SAFETY_UI_PROFITABILITY.md) — **Fail-Closed Live Safety & Route Economics**: Balance preflight gating, slippage ceilings, and unified fee deduction.

### 2. Strategy Monitors, Telemetry & Gadgets
- [`spec_opportunity_history_gadget_unification.md`](spec_opportunity_history_gadget_unification.md) — **Unified Opportunity History Gadget**: Harmonized execution timeline across Spread Radar, Funding Radar, Prediction Hub, and TickerExplorer. *(GitHub Issue #99)*
- [`spec_opportunity_trend_graph_and_pnl_consistency.md`](spec_opportunity_trend_graph_and_pnl_consistency.md) — **Opportunity Trend Graph & PnL Consistency**: Sparkline geometry upgrade and reconciliation of realized vs unrealized profit across views. *(GitHub Issue #93)*
- [`spec_strategy_cockpit_telemetry_dock.md`](spec_strategy_cockpit_telemetry_dock.md) — **Strategy Cockpit Telemetry Dock**: Multi-tab live execution stream, event logs, and real-time PnL matrix. *(GitHub Issue #91)*
- [`spec_ui_standardization_and_strategy_monitor_ergonomics.md`](spec_ui_standardization_and_strategy_monitor_ergonomics.md) — **UI Standardization & Strategy Monitor Ergonomics**: Unified ribbon headers, status badges, clean asset filters, and removal of mock scaffolding. *(GitHub Issue #97)*
- [`SPEC_BOT_LIFECYCLE_UI_RISK_ALIGNMENT.md`](SPEC_BOT_LIFECYCLE_UI_RISK_ALIGNMENT.md) — **Autonomous Bot Lifecycle & Risk Alignment**: Bot execution gating, rate limits, and risk parameters.

### 3. Strategy Engines & Carry Arbitrage
- [`spec_vault_independent_mark_to_market_valuation.md`](spec_vault_independent_mark_to_market_valuation.md) — **Continuous Public Mark-to-Market Valuation**: Vault-lock-independent mark-to-market pricing for options and perpetual positions. *(GitHub Issue #92)*
- [`spec_ui_options_bot_calibration.md`](spec_ui_options_bot_calibration.md) — **Options Strategy Page & Bot Calibration**: Put-call parity scanner, substrategy navigation rail, Deribit paper yield hurdle, and options Greeks pass-through. *(GitHub Issues #64, #81, #90)*
- [`SPEC_OPTIONS_DISPLAY_AND_PNL_RECONCILIATION.md`](SPEC_OPTIONS_DISPLAY_AND_PNL_RECONCILIATION.md) — **Options Table Ergonomics & PnL Reconciliation**: Segregated call/put columns, action labels (BOUGHT/WROTE), and leg aggregation.
- [`spec_ibkr_paper_trading_and_options_yield_hurdle.md`](spec_ibkr_paper_trading_and_options_yield_hurdle.md) — **Interactive Brokers (IBKR) Paper Trading & Equity Options**: Client Portal Gateway integration for equity/index options arbitrage.

### 4. Venue Integrations & Environmental Boundaries
- [`spec_popdex_faucet_removal_and_kraken_live_only.md`](spec_popdex_faucet_removal_and_kraken_live_only.md) — **PopDEX Teardown & Venue Realism**: Strict environment boundaries, eliminating artificial faucet toggles, and enforcing live-only Kraken execution.
