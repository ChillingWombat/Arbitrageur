# System Architecture & Modular Design

> Current as of the Tauri workspace (`crates/*` + `frontend/src-tauri`). If crate list drifts (e.g. headless `arbitrageur-app`, `wallet.rs`, Variational/PopDEX adapters), update §2 first.


The **Arbitrageur** system is engineered in Rust with high-performance concurrency, zero-cost abstractions, and strict modular separation across multiple financial market domains.

---

## 1. High-Level Workspace Architecture

```
                                  +-----------------------+
                                  |     arbitrageur-app    |
                                  |   (CLI, TUI, Daemon)  |
                                  +-----------+-----------+
                                              |
                     +------------------------+------------------------+
                     |                                                 |
         +-----------v------------+                       +------------v-----------+
         |   arbitrageur-strategy  |                       |    arbitrageur-risk     |
         |  - Spatial Price Arb   |                       |  - Circuit Breaker     |
         |  - Funding Carry Arb   |<----------------------+  - Delta Guard         |
         |  - Stat-Arb Pairs (OU) |                       |  - Margin Monitor      |
         |  - Prediction Market   |                       +------------+-----------+
         +-----------+------------+                                    |
                     |                                                 |
         +-----------v------------+                                    |
         |  arbitrageur-execution  |<-----------------------------------+
         |  - Atomic Coordinator  |
         |  - Emergency Unwind    |
         |  - Smart Order Router  |
         +-----------+------------+
                     |
         +-----------v------------+                       +------------------------+
         |  arbitrageur-exchanges  |                       |    arbitrageur-math     |
         |  - CEX + DEX adapters |                       |  - Kalman Filter (2D) |
         |  - Lighter + RH lane  |                       |  - OU Process MLE     |
         |  - Prediction research|                       |  - Bertram Thresholds |
         |  - Simulated Engine   |                       |  - Carry / Parity     |
         +-----------+------------+                       +-----------+------------+
                     |                                                |
                     +------------------------+-----------------------+
                                              |
                                  +-----------v-----------+
                                  |    arbitrageur-core    |
                                  | (Types, Domain, Order)|
                                  +-----------------------+
```

---

## 2. Modular Crate Decomposition

| Crate | Responsibility | Key Abstractions |
|---|---|---|
| `arbitrageur-core` | Core domain types, precision decimal arithmetic, market specifications, and unified interfaces. | `OrderBook`, `Ticker`, `FundingRate`, `OrderRequest`, `Position`, `ExchangeAdapter` |
| `arbitrageur-math` | Pure quantitative finance algorithms, time series models, statistical arbitrage solvers. | `HedgeRatioKalmanFilter`, `OuModelParams`, `compute_bertram_thresholds`, `evaluate_funding_carry` |
| `arbitrageur-exchanges` | Venue connectors with capability-specific public/account reads, submission transports, and the simulator. | `HyperliquidAdapter`, `LighterAdapter`, `VantaAdapter`, `RobinhoodAdapter`, `AsterAdapter`, `BinanceAdapter`, `OkxAdapter`, `CoinbaseAdapter`, `KrakenAdapter`, `DecibelAdapter`, `AlpacaAdapter`, `IbkrAdapter`, `BorosAdapter`, `SimulatedExchange` |
| `arbitrageur-risk` | Real-time circuit breakers, liquidation buffers, and portfolio delta neutrality enforcement. | `CircuitBreaker`, `DeltaGuard`, `MarginMonitor` |
| `arbitrageur-execution` | Atomic multi-leg concurrent execution, smart routing, and automated rollback/unwind handlers. | `AtomicArbitrageCoordinator`, `EmergencyUnwindHandler`, `SmartOrderRouter`, `PassiveUnwindEngine` |
| `arbitrageur-strategy` | Autonomous trading strategies consuming normalized market events and generating actionable signals. | `SpatialPriceArbStrategy`, `FundingRateArbStrategy`, `StatArbPairsStrategy`, `ConvergenceAuto`, `SpatialExitConfig` (probability-driven invalidation exit $P_{\text{win}}$) |
| `arbitrageur-app` | CLI research commands, benchmarks, and a loopback-only headless placeholder daemon. | `Cli`, `run_monitor`, `run_simulate`, `run_funding_scan`, `run_benchmarks`, `run_serve` |
| `arbitrageur-desktop` (`frontend/src-tauri`) | Tauri command surface, durable SQLite state, vault, live/paper mode policy, feed supervisors, and UI DTO mapping. | `AppEngineState`, `StateStore`, `SecureVault`, `dev_rpc`, `execute_*_impl`, `SignalExecution` |

---

## 3. Concurrency & Execution Model

### 3.1 Non-Blocking Asynchronous Tokio Runtime
- Execution coordinates multiple asynchronous streams concurrently without blocking threads.
- Market orderbook feeds are ingested via non-blocking channels (`tokio::sync::broadcast` / `mpsc`).
- Order placement executes both legs in parallel via `tokio::join!(exchange_a.place_order(), exchange_b.place_order())`, minimizing execution latency skew.

### 3.2 Thread-Safe State & Lock-Free Data Structures
- Heavy use of `parking_lot::RwLock`, atomic flags (`AtomicBool`, `AtomicU32`), and `dashmap::DashMap` ensures safe concurrent updates across strategy worker threads without mutex contention.

---

## 4. Security & Key Management

1. **Encrypted credential vault:** Exchange secrets are entered through the desktop vault and stored in `~/.arbitrageur/vault.enc` using AES-256-GCM with an Argon2id-derived key. Private keys for on-chain execution are protected in an Iota Stronghold snapshot (`~/.arbitrageur/vault.hold`).
2. **Session auto-lock & optional MFA:** The desktop session automatically locks after 60 seconds of inactivity or navigation away from settings. Optional RFC-6238 TOTP provides second-factor authentication for vault access.
3. **Permission hardening:** On Unix the data directory is `0700`; vault and SQLite files are `0600`. SQLite operational state is local plaintext at the application layer and must be protected by filesystem permissions/backups.
4. **Live-order boundary:** Adapter signing/submission is transport capability only. Terminal fill polling and restart recovery are implemented — a reconciliation supervisor re-scans durable working orders at startup and every 5s through per-adapter `fetch_order`, with identity validation and monotonic ledger updates. Automated residual-leg resolution (unconditional — reduce-only flatten of the filled leg, routed through the resolved Auto Position Exit policy) and bounded cancel-retry at the working-order deadline are implemented; live strategy entries dispatch once `live_entry_gate` passes — Trading Mode = Live is the operator opt-in, gated per-leg on vault-unlocked + submission + order-polling + strategy-entry transport + credentials.
5. **Vendored signer integrity:** `liblighter_signer.so` is hashed before FFI load; a custom `LIGHTER_SIGNER_LIB` requires an explicit `LIGHTER_SIGNER_SHA256`.

---

## 5. Operator Dev RPC & Engine Lifecycle

1. **Operator Dev RPC Unix Socket (`/tmp/arb-dev.sock`):** Provides authenticated IPC inspection for operators, automated daemons, and system health monitors without opening external network ports. Supports commands such as `unlock_vault`, `system_status`, `open_positions`, `external_positions`, `reconcile_orders`, `flatten_ticker`, `cancel_venue_order`, `radar_bot_status`, and `monitor_snapshot`.
2. **Decoupled Engine Lifecycle State Machine:**
   - **`Running`**: Market feeds active, automated bots executing, manual trades enabled.
   - **`Paused`**: Market data streams and reconciliation remain live; automated order dispatch is suspended. Can be resumed instantly without re-initializing network connections.
   - **`Stopped`**: Triggered by Emergency Stop-All or explicit operator shutdown. All working orders are cancelled, active positions are flattened at market, background supervisors are terminated, and order dispatch is locked until a fresh start.
3. **Probability-Driven Statistical Invalidation Exit:** Spread convergence positions are monitored continuously against real-time Ornstein-Uhlenbeck mean-reversion probability ($P_{\text{win}}$). If $P_{\text{win}}$ drops below the invalidation hurdle (default 0.50), the position is unwound dynamically (`ExitTrigger::ProbabilityDecayed`), eliminating artificial wall-clock churn.
