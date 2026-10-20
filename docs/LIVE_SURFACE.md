# Live Surface

Last reviewed: 2026-10-21 against `feature/146-quant-system-integration`.

This file lists the top-level modules that compile into each product crate and separates desktop live/testnet paths from CLI research and simulation helpers. The module registries in `lib.rs` and `frontend/src-tauri/src/main.rs` are authoritative. A module being present here does not mean it has an execution call site.

## Runtime rule

`TradingEnvironment` has two values:

- `Live` selects the `live_*` adapter set, production credentials, and live durable-state scope.
- `Testnet` selects the `tn_*` adapter set, venue demo endpoints, and testnet durable-state scope.

The desktop has no simulated trading environment. A venue without a demo endpoint is ineligible in Testnet. Test-only simulated adapters exist only under `#[cfg(test)]`.

## Module inventory

### `arbitrageur-core`

```
exchange, hub, market, order, position, quote, symbol, tracking, types
```

These modules define the shared domain types, market-data bus, normalized symbols, order and position models, and the `ExchangeAdapter` contract.

### `arbitrageur-math`

```
atomic_mev, autotune, basis_pca, basis_term_structure, binomial,
black_scholes, book_gradient, bouchaud_propagator, calibration,
cointegration, collateral_barrier, collateral_dag, concentrated_liquidity,
cross_chain_intent_dag, cross_hawkes, debiased_ou, delta_gamma_rehedge,
dislocation, epoch_jump, factor_stat_arb, fast_basis_harvest, fixed_yield,
forecast_calibration, funding_carry, funding_epoch_countdown, funding_spline,
funding_velocity, gueant_mm, hawkes, hf_vecm, hmm_liquidity, intent_bridge,
inventory_skew, kalman_filter, l3_hazard, l3_orderbook, l3_tensor_ofi,
latency_racing, lead_lag, margin_deleveraging, matrix_propagator,
micro_price_surface, multi_asset_avellaneda, multi_asset_impact,
multi_commodity_flow, optimal_thresholds, ou_process, pair_monitor,
portfolio_greeks_opt, power_law_propagator, prediction_synthetic,
private_relay, queue_drift, queue_priority, queue_survival,
regime_switching, resilience_pacer, risk_math, rolling_stats, rough_ou,
seasonality, session, spatial_spread, stochastic_depth, svi_arbitrage_free,
tail_copula, triangular, triangular_surface, tropical_arbitrage,
tsrv_volatility, unified_margin_warning, variance_swap, volatility_jump, vpin
```

This is a quantitative library surface. The desktop and CLI import models directly; strategy and execution modules import others internally. A module that has no desktop call site is still compiled into the library, but it is not an execution entry point.

### `arbitrageur-risk`

```
circuit_breaker, collateral_rebalance, delta_guard, margin_monitor,
risk_parity_allocator, safety_guards
```

`circuit_breaker`, `delta_guard`, `margin_monitor`, `collateral_rebalance`, and `safety_guards` are imported by desktop or execution code. `risk_parity_allocator` has no desktop consumer on this branch.

### `arbitrageur-execution`

```
atomic_bundle, conformal_cvar_shield, conformal_risk_guard, coordinator,
emergency_unwind, l2_sequencer_guard, passive_unwind, queue_priority,
smart_routing, virtual_ledger
```

`CompositeExecutionShield` composes `conformal_risk_guard` and
`conformal_cvar_shield` and is wired into desktop execution preflight.
`atomic_bundle` is optional inside `SmartOrderRouter`; `l2_sequencer_guard`
is optional inside `SpatialPriceArbStrategy`. ADR 0001 defines the
consolidation seam.

### `arbitrageur-strategy`

| Module | Current consumer | Runtime status |
|---|---|---|
| `convergence_auto` | `auto_exec`, `monitor_ipc`, options bot mode | Live/Testnet policy |
| `engine` | Strategy crate composition | No desktop or CLI constructor |
| `funding_rate_arb` | `arbitrageur-app funding` | CLI only |
| `hawkes_flow` | `state`, `feed`, `auto_exec` | Live/Testnet flow gate |
| `options_dispersion` | Desktop `LiveVolSurfaceCache` and options bots | Live/Testnet |
| `pair_trim` | `execution`, `triangle`, `dev_rpc` | Live/Testnet reduce paths |
| `prediction_market_arb` | `commands::scan_prediction_opportunities_impl` | Live/Testnet read-only scan |
| `spatial_exit` | `execution::check_spatial_convergence_impl` | Live/Testnet |
| `spatial_price_arb` | `arbitrageur-app monitor` and `simulate` | CLI only |
| `stat_arb_pairs` | `state`, `commands` | Live/Testnet analysis |

### `arbitrageur-exchanges`

```
across_bridge, alpaca, arcus, aster, binance, boros, coinbase, decibel,
deribit, dex_perps, evm_amm, hyperliquid, ibkr, kraken, lighter,
lighter_signer, okx, popdex, prediction, rate_limiter, robinhood,
simulated, vanta, variational, wallet, ws
```

`simulated` is instantiated by the CLI and `#[cfg(test)]` desktop helpers only. `prediction` exposes read-only Polymarket and Kalshi research adapters. `across_bridge` is a transfer helper, not a trading adapter. `lighter_signer`, `rate_limiter`, `wallet`, and `ws` are shared support modules.

### `frontend/src-tauri`

```
accounts, alerting, attribution, auto_exec, autotune, commands, dev_rpc,
equity_attribution, execution, external_positions, feed, funding, monitor,
monitor_ipc, num, options_bot, options_packages, options_parity, performance,
priceretriver, reconciliation, seasonality, stablecoin_fx, state, state_store,
strategy_config, strategy_filters, stronghold, totp, triangle, vault,
ws_primary
```

`main.rs` starts the monitor, stablecoin-FX refresh, Radar executor, options bot, volatility-surface worker, convergence supervisor, reconciliation supervisor, feed supervisors, scheduled-exit dispatcher, AutoTune scheduler, drawdown-breaker feed, and optional dev RPC.

## Active desktop paths

| Surface | Environment | Entry modules | Venue writes |
|---|---|---|---|
| Market data and spread monitor | Live and Testnet | `monitor`, `feed`, `ws_primary` | None |
| Simple Spread manual entry | Live after `live_entry_gate`; Testnet | `execution`, `commands` | Two-leg entry |
| Spread Radar row Execute | Live after `live_entry_gate`; Testnet | `monitor_ipc`, `execution` | Two-leg entry |
| Spread Radar autonomous executor | Active environment | `auto_exec`, `monitor_ipc` | Two-leg entry and managed exits |
| Funding carry and spot/perp basis | Live manual, Testnet manual or auto | `funding` | Two-leg entry |
| Fixed-yield Boros carry | Live only | `funding`, `commands` | Four-leg entry and exit |
| Triangular arbitrage | Live manual, Testnet manual or auto | `triangle` | Three-leg entry |
| Spot cross-venue arbitrage | Live after `live_entry_gate`; Testnet | `commands`, `execution` | Two-leg spot entry |
| Options parity and packages | Live manual, Testnet manual or auto | `options_parity`, `options_packages`, `options_bot` | Multi-leg entry and exit |
| Reconciliation and residual flatten | Active environment | `reconciliation`, `execution` | Reduce-only writes |
| External-position exits | Active environment | `external_positions`, `execution` | Reduce-only writes |
| Emergency Stop All | Active environment | `execution`, `execution::emergency_unwind` | Cancel and market flatten |
| Dev RPC | Active environment; requires `ARBITRAGEUR_DEV_SOCK` | `dev_rpc` | Operator-only commands |

All new-risk entries pass the circuit breaker, pause/stop guard, environment-scoped adapter lookup, and strategy-specific gates. Live entries also require an unlocked vault and `live_entry_gate`: submission support, order-status polling, strategy-entry eligibility, and loaded live credentials for every leg.

Reduce-only exits do not require a new-entry gate. They still use the environment-scoped adapter, verify venue order identity, and remain supervised until terminal state.

## Venue sets

Live named adapters:

```
hyperliquid, lighter, robinhood, vanta, arcus, aster, popdex, binance, okx,
coinbase, kraken, alpaca, ibkr, decibel, deribit, boros, variational
```

Live also admits configured long-tail `dex_perps`, configured `evm_amm` venues, and enabled account-bound adapters. Arcus and Variational expose market data but fail closed on unsupported order paths. Boros is live-only.

Testnet adapters:

```
hyperliquid, lighter, robinhood, vanta, arcus, popdex, binance, okx,
alpaca, ibkr, decibel, deribit
```

Aster, Coinbase, Kraken, Boros, Variational, `dex_perps`, and `evm_amm` are not admitted in Testnet on this branch. IBKR paper and Alpaca paper are real venue account environments, not local simulations.

## CLI-only helpers

| Command | Data or execution class |
|---|---|
| `arbitrageur monitor` | Real public venue reads |
| `arbitrageur simulate` | `SimulatedExchange`; no venue writes |
| `arbitrageur funding` | Real public venue reads plus pure funding math |
| `arbitrageur stat-arb` | Real public venue reads plus local Kalman math |
| `arbitrageur prediction` | Read-only Polymarket and Kalshi quotes |
| `arbitrageur benchmark` | Local quantitative micro-benchmarks |
| `arbitrageur serve` | Read-only 503 placeholder daemon |

The CLI has no vault, durable signal ledger, reconciliation supervisor, or order submission path. `SimulatedExchange` is a CLI and test helper, not a desktop production adapter.
