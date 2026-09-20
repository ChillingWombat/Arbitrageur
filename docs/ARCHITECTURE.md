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
| `arbitrageur-exchanges` | Venue connectors with capability-specific public/account reads, submission transports, and the simulator. | `HyperliquidAdapter`, `LighterAdapter`, `VantaAdapter`, `RobinhoodAdapter`, `AsterAdapter`, `BinanceAdapter`, `OkxAdapter`, `CoinbaseAdapter`, `KrakenAdapter`, `DecibelAdapter`, `PopDexAdapter`, `SimulatedExchange` |
| `arbitrageur-risk` | Real-time circuit breakers, liquidation buffers, and portfolio delta neutrality enforcement. | `CircuitBreaker`, `DeltaGuard`, `MarginMonitor` |
| `arbitrageur-execution` | Atomic multi-leg concurrent execution, smart routing, and automated rollback/unwind handlers. | `AtomicArbitrageCoordinator`, `EmergencyUnwindHandler`, `SmartOrderRouter`, `PassiveUnwindEngine` |
| `arbitrageur-strategy` | Autonomous trading strategies consuming normalized market events and generating actionable signals. | `SpatialPriceArbStrategy`, `FundingRateArbStrategy`, `StatArbPairsStrategy`, `ConvergenceAuto`, `ArbitrageEngine` |
| `arbitrageur-app` | CLI research commands, benchmarks, and a loopback-only headless placeholder daemon. | `Cli`, `run_monitor`, `run_simulate`, `run_funding_scan`, `run_benchmarks`, `run_serve` |
| `arbitrageur-desktop` (`frontend/src-tauri`) | Tauri command surface, durable SQLite state, vault, live/paper mode policy, feed supervisors, and UI DTO mapping. | `AppEngineState`, `StateStore`, `SecureVault`, `execute_*_impl`, `SignalExecution` |

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

1. **Encrypted credential vault:** Exchange secrets are entered through the desktop vault and stored in `~/.arbitrageur/vault.enc` using AES-256-GCM with an Argon2id-derived key. Reveal is field-scoped; there is no whole-account credential-read IPC.
2. **Permission hardening:** On Unix the data directory is `0700`; vault and SQLite files are `0600`. SQLite operational state is local plaintext at the application layer and must be protected by filesystem permissions/backups.
3. **Live-order boundary:** Adapter signing/submission is transport capability only. Terminal fill polling and restart recovery are implemented — a reconciliation supervisor re-scans durable working orders at startup and every 5s through per-adapter `fetch_order`, with identity validation and monotonic ledger updates. Automated residual-leg resolution (opt-in `auto_resolve_residual_exposure` — reduce-only flatten of the filled leg) and bounded cancel-retry at the working-order deadline are implemented; new live strategy entries remain disabled pending operator opt-in and sandbox/canary evidence.
4. **Vendored signer integrity:** `liblighter_signer.so` is hashed before FFI load; a custom `LIGHTER_SIGNER_LIB` requires an explicit `LIGHTER_SIGNER_SHA256`. Remote HSM signing remains a design direction, not an implemented runtime backend.
