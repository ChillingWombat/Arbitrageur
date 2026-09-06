# System Architecture & Modular Design

The **Arbitrager** system is engineered in Rust with high-performance concurrency, zero-cost abstractions, and strict modular separation across multiple financial market domains.

---

## 1. High-Level Workspace Architecture

```
                                  +-----------------------+
                                  |     arbitrager-app    |
                                  |   (CLI, TUI, Daemon)  |
                                  +-----------+-----------+
                                              |
                     +------------------------+------------------------+
                     |                                                 |
         +-----------v------------+                       +------------v-----------+
         |   arbitrager-strategy  |                       |    arbitrager-risk     |
         |  - Spatial Price Arb   |                       |  - Circuit Breaker     |
         |  - Funding Carry Arb   |<----------------------+  - Delta Guard         |
         |  - Stat-Arb Pairs (OU) |                       |  - Margin Monitor      |
         |  - Prediction Market   |                       +------------+-----------+
         +-----------+------------+                                    |
                     |                                                 |
         +-----------v------------+                                    |
         |  arbitrager-execution  |<-----------------------------------+
         |  - Atomic Coordinator  |
         |  - Emergency Unwind    |
         |  - Smart Order Router  |
         +-----------+------------+
                     |
         +-----------v------------+                       +------------------------+
         |  arbitrager-exchanges  |                       |    arbitrager-math     |
         |  - Hyperliquid L1      |                       |  - Kalman Filter (2D)  |
         |  - Lighter (zk-DEX)    |                       |  - OU Process MLE      |
         |  - Vanta.exchange      |                       |  - Bertram Thresholds  |
         |  - Robinhood EVM       |                       |  - Cointegration (ADF) |
         |  - Prediction Markets  |                       |  - Synthetic Parity    |
         |  - Simulated Engine    |                       |  - Welford Online      |
         +-----------+------------+                       +-----------+------------+
                     |                                                |
                     +------------------------+-----------------------+
                                              |
                                  +-----------v-----------+
                                  |    arbitrager-core    |
                                  | (Types, Domain, Order)|
                                  +-----------------------+
```

---

## 2. Modular Crate Decomposition

| Crate | Responsibility | Key Abstractions |
|---|---|---|
| `arbitrager-core` | Core domain types, precision decimal arithmetic, market specifications, and unified interfaces. | `OrderBook`, `Ticker`, `FundingRate`, `OrderRequest`, `Position`, `ExchangeAdapter` |
| `arbitrager-math` | Pure quantitative finance algorithms, time series models, statistical arbitrage solvers. | `HedgeRatioKalmanFilter`, `OuModelParams`, `compute_bertram_thresholds`, `evaluate_funding_carry` |
| `arbitrager-exchanges` | High-throughput asynchronous venue connectors with rate-limiting and session management. | `HyperliquidAdapter`, `LighterAdapter`, `VantaAdapter`, `RobinhoodAdapter`, `SimulatedExchange` |
| `arbitrager-risk` | Real-time circuit breakers, liquidation buffers, and portfolio delta neutrality enforcement. | `CircuitBreaker`, `DeltaGuard`, `MarginMonitor` |
| `arbitrager-execution` | Atomic multi-leg concurrent execution, smart routing, and automated rollback/unwind handlers. | `AtomicArbitrageCoordinator`, `EmergencyUnwindHandler`, `SmartOrderRouter` |
| `arbitrager-strategy` | Autonomous trading strategies consuming normalized market events and generating actionable signals. | `SpatialPriceArbStrategy`, `FundingRateArbStrategy`, `StatArbPairsStrategy`, `ArbitrageEngine` |
| `arbitrager-app` | Command-line interface, benchmarking suite, telemetry, and live interactive runners. | `Cli`, `run_monitor`, `run_simulate`, `run_funding_scan`, `run_benchmarks` |

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

1. **Environment Isolation:** Secrets and API keys are loaded strictly via environment variables or encrypted keychain files (`.env` is strictly gitignored).
2. **Permission Hardening:** Sensitive configuration files are saved with POSIX `0o600` permissions (read/write by owner only).
3. **Remote Signer Compatibility:** Full architectural parity with `taoli-tools-signer`, allowing the engine to offload order signing to a private hardware security module or container.
