## Problem Statement

Latency and algorithmic inefficiency are existential threats to cross-venue arbitrage and automated market-neutral strategies:

1. **Slow Emergency Shutdown and Sequential Unwind Latencies:**
   When an operator or automated risk trigger engages the STOP ALL killswitch, the prior implementation suffered from severe delays before closing orders were submitted. Resting limit cancels waited for the slowest venue, followed by an arbitrary 250ms settle pause, and a blocking `snapshot_venue_equity()` network pass querying all admitted venues sequentially. Furthermore, the position unwind loop sequentially iterated across strategies, signals, and individual legs, waiting on an async-ack polling loop with `sleep(200ms)` up to 15 times (~3.0 seconds) per leg. This caused 4 trades (8 legs) to take 8 to 15+ seconds to unwind, leaving the account dangerously exposed to adverse market moves and liquidation during market crashes.

2. **Quadratic Algorithmic Complexity on Hot Monitor Loops ($O(V^2)$):**
   The spread monitor engine evaluated pairwise crosses using nested Cartesian product loops across all admitted venues ($O(V^2 \cdot S)$). For 10+ venues and dozens of symbols, this resulted in hundreds of redundant calculations, nested hash map queries ($2V(V-1)$ lookups per tick), and lock contention on the global monitor snapshot.

3. **Memory Allocations and Clone Churn in Triangular Arbitrage:**
   The triangular arbitrage scanner deep-cloned entire `OrderBook` structs and their underlying level vectors on every triplet candidate (>2,000 deep clones per scan cycle) and performed expensive string hash lookups for graph edges during cycle extraction.

4. **Linear Scans in Position and Exposure Tracking ($O(N)$):**
   The Virtual Strategy Ledger performed linear scans over all open lots whenever computing net exposure or matching closing legs, degrading throughput as transaction history grew.

5. **Slow Numerical Solvers and Mathematical Edge-Case Failures:**
   Implied volatility solvers used slow iterative bisections (>50 iterations). Deep out-of-the-money options suffered from catastrophic floating-point cancellation in normal CDF tail evaluations ($x < -5$), and Ornstein-Uhlenbeck passage time optimization suffered from discretization step artifacts that trapped Golden Section Search in artificial local extrema.

6. **Contract Granularity Mismatches & Residual Confusion:**
   On inverse derivative venues (e.g. Deribit Perpetuals), contracts trade in discrete $10 USD lots rather than base coins, while options trade in integer base coins. Converting USD sizes to base coins creates dynamic floating values (e.g. 0.9998895 ETH displayed as +0.9999), confusing operators into suspecting unhedged risk when the position is structurally delta-neutral within venue lot limits.

## Solution

A comprehensive overhaul of execution concurrency, algorithmic complexity, numerical mathematics, and zero-allocation data structures across the entire arbitrage engine:

1. **High-Concurrency Emergency Stop and Unwind Pipeline:**
   - Eliminate sequential strategy, signal, and leg loops during emergency shutdown.
   - Dispatch non-conflicting venue/symbol unwinds concurrently via `futures::future::join_all` and `tokio::join!` while preserving cryptographic nonce ordering per individual exchange subaccount.
   - Bypass blocking pre-flight equity snapshots during emergency market exits, prioritizing execution latency over non-critical baseline telemetry.
   - Optimize async-ack polling to detect final fills with adaptive backoff.

2. **$O(V)$ Best-Bid / Best-Ask Min-Max Spread Heaps:**
   - Transform the $O(V^2)$ all-pairs pairwise scan into an $O(V)$ min/max heap structure that extracts the highest bid and lowest ask across venues directly.
   - Pre-sort crossing vectors descending by net profit yield (`net_profit_bps`), guaranteeing that execution bots process highest-yield opportunities first.
   - Replace nested hash lookups in the monitor tick loop with pre-indexed contiguous slice indexing (`Vec<Option<OrderBook>>`).

3. **Zero-Copy Triangular Graph Cycle Detection:**
   - Eliminate `OrderBook` cloning across triangular evaluation paths by using borrowed references (`Borrow<OrderBook>`).
   - Introduce integer graph node indices (`from_idx`, `to_idx: usize`) on cycle edges, replacing string hashing in Bellman-Ford / 3-cycle detection.
   - Canonicalize cyclic permutations to prevent duplicate evaluations of shifted triangles.

4. **$O(1)$ Secondary Indexing in Virtual Strategy Ledger:**
   - Implement a 3-tier secondary index `HashMap<Environment, HashMap<Symbol, HashMap<Venue, HashSet<LotId>>>>` for instantaneous $O(1)$ net exposure calculation without heap allocation.
   - Maintain index integrity across state hydration (`set_lots`) and transaction history resets (`retain_lots`).
   - Equip with a fail-closed unindexed linear fallback to prevent exposure reporting from ever understating risk.

5. **Accelerated High-Precision Numerical Solvers:**
   - Implement analytical seed approximations combined with Halley's method ($d_1 d_2 / \sigma$ curvature ratio) for Black-Scholes implied volatility, degrading to damped Newton-Raphson and bounded 40-step bisection fallback.
   - Evaluate normal CDF tails directly via $\phi(x) \cdot \text{poly}(k)$ for $x < -5$, preventing catastrophic cancellation.
   - Introduce continuous linear interpolation for discrete passage time moments in Ornstein-Uhlenbeck threshold tuning, ensuring a continuous objective function landscape for Golden Section Search.
   - Reduce matrix allocation overhead in Kalman filter covariance tracking.

6. **Full 20-Core Hardware Topology Utilization:**
   - Leverage the multi-threaded Tokio runtime to distribute compute-bound mathematical modeling and I/O-bound exchange streams concurrently across all 20 physical CPU cores.
   - Provide transparent UI telemetry explaining discrete contract units versus base coin representations.

## User Stories

1. As an operator clicking STOP ALL, I want all resting orders across all venues to cancel immediately in parallel, so that the engine enters a safe state in under 500ms.
2. As a risk manager, I want emergency position unwinds to dispatch across disjoint exchanges simultaneously, so that multi-leg positions are flattened without serial delay cascades.
3. As an automated arbitrage bot, I want market orders on separate venues to fire concurrently via `tokio::join!`, so that execution leg risk is minimized during fast-moving markets.
4. As a quantitative researcher, I want the spread scanner to compute opportunities in $O(V)$ time using best-bid/best-ask heaps, so that monitoring dozens of exchanges does not drop feed ticks.
5. As an operator watching the Opportunity Radar, I want spatial crossings sorted descending by net profit bps, so that the most profitable opportunities are always visible and traded first.
6. As a system engineer, I want triangular arbitrage book evaluation to borrow orderbooks without cloning vectors, so that CPU cache misses and heap memory fragmentation are minimized.
7. As a graph algorithm developer, I want triangular cycle extraction to use integer vertex indices rather than string hashing, so that 3-cycle path evaluation completes in sub-microsecond time.
8. As a risk supervisor, I want `VirtualStrategyLedger::compute_venue_net_exposure` to resolve in $O(1)$ time via secondary indexing, so that risk limit checks never bottleneck order submission.
9. As a compliance auditor, I want secondary ledger indexes to stay perfectly synchronized across state hydration and historical resets, so that exposure is never miscalculated after a restart.
10. As a fail-closed safety architect, I want an unindexed fallback in the virtual ledger if secondary indexes are unpopulated, so that exposure is never falsely reported as zero.
11. As an options trader, I want implied volatility calculation to converge in under 5 iterations using Halley's method with analytical seeds, so that volatility surfaces calibrate in real time.
12. As a derivatives quantitative developer, I want normal CDF evaluations for deep out-of-the-money options to avoid catastrophic floating-point cancellation, so that option deltas and Greeks remain monotonic and stable.
13. As an automated tuning scheduler, I want Ornstein-Uhlenbeck threshold optimization to use continuous linear interpolation across discrete passage time moments, so that Golden Section Search converges smoothly to true optimal parameters.
14. As an operator running on a multi-core workstation, I want the arbitrage desktop application to saturate all 20 CPU cores across background feeds, analytics, and WebKit UI rendering, so that no single thread blocks the system.
15. As a trader inspecting an options parity position, I want the UI and documentation to clearly convey why inverse USD contracts (e.g. Deribit perps) display as 0.9999 ETH rather than an exact integer, so that I understand it represents full delta neutrality within venue lot limits.
16. As an operator managing desktop stability, I want the systemd user service to enforce memory cgroup caps and auto-restart on failure, so that long-running testnet runs remain reliable.
17. As a quality engineer, I want all unit tests, clippy checks, and frontend component tests to pass cleanly before compiling release binaries, so that regressions are caught before deployment.

## Implementation Decisions

1. **Secondary Index Data Structure in Virtual Strategy Ledger:**
   - Structure: `open_lots_by_venue: HashMap<String, HashMap<String, HashMap<String, HashSet<String>>>>` representing `environment -> symbol -> venue -> Set<lot_id>`.
   - Mutation methods: added `rebuild_open_index()`, `set_lots()`, and `retain_lots()`.
   - Fallback: if `open_lots_by_venue` is empty while `lots` contains entries, execute an unindexed linear scan to guarantee fail-closed exposure truth.

2. **Spread Scanner Heap Refactoring:**
   - In `arbitrageur-math`, replaced the pairwise loop with a min/max priority heap extracting the highest bid venue and lowest ask venue in $O(V)$ time.
   - Resulting crossings are sorted descending by `net_profit_bps`.
   - In `frontend/src-tauri/src/monitor.rs`, pre-indexed contiguous slice indexing (`Vec<Option<OrderBook>>`) replaced nested map lookups.

3. **Zero-Copy Triangular Scanner:**
   - Modified `evaluate_triangle_books` and `triangle_depth_curve` signatures to accept `Borrow<OrderBook>`.
   - Added `from_idx: usize` and `to_idx: usize` to `TriangularCycleEdge`.
   - Implemented cycle shift canonicalization to prune duplicate cyclic permutations.

4. **Analytical Black-Scholes and Halley IV Solver:**
   - Normal CDF evaluated as $\phi(x) \cdot \text{poly}(k)$ for $x < -5$.
   - Implemented Halley's method using curvature ratio $d_1 d_2 / \sigma$ with analytical starting seeds.
   - Added automatic degradation to damped Newton-Raphson steps when curvature becomes degenerate, followed by a bounded 40-step bisection fallback tracking minimum absolute error.

5. **Passage Time Moment Interpolation:**
   - In `optimal_thresholds.rs`, implemented linear interpolation between adjacent discrete array nodes in `ou_passage_moments_with_buf` for expected time $\theta E[T]$ and second moment $E[T^2]$.

6. **Concurrent Execution Architecture:**
   - Preserved single-wallet nonce serialization per venue subaccount to prevent Ethereum/DEX transaction collisions.
   - Enabled concurrent multi-venue and multi-symbol execution across independent accounts via `tokio::join!` and `join_all`.
   - Eliminated blocking `snapshot_venue_equity` pre-flight calls during emergency stop paths.

## Testing Decisions

- **Testing External Behavior at the Highest Seams:**
  - Tests verify that external API outputs, calculated exposures, and mathematical Greeks match exact analytical benchmarks without coupling tests to internal loop structures.
- **Modules Tested:**
  - `arbitrageur-math`: unit tests for Black-Scholes Greeks, Newton-Raphson / Halley IV convergence, $O(V)$ spatial spread heap correctness, and Ornstein-Uhlenbeck continuous interpolation.
  - `arbitrageur-execution`: unit tests for virtual ledger lot indexing, hydration synchronization, retention pruning, and fail-closed fallback behavior.
  - `arbitrageur-desktop`: end-to-end integration tests (610 tests) verifying complete engine state, dev RPC commands, bot lifecycle transitions, and emergency stop mechanics.
  - `frontend`: 18 test suites (52 component tests) verifying UI formatting and modal displays.
- **Prior Art:**
  - Modeled after existing tests in `crates/arbitrageur-math/src/spatial_spread.rs`, `crates/arbitrageur-execution/src/virtual_ledger.rs`, and `frontend/src-tauri/src/commands.rs`.

## Out of Scope

- Modifying third-party exchange wire protocols or changing Deribit's inverse USD contract specifications.
- Deploying live trading capital on mainnet venues without explicit operator authorization.
- Rewriting the WebKit/GTK rendering engine or bypassing systemd cgroup memory enforcement.

## Further Notes

- The release binary compiles with `--features custom-protocol` to ensure proper packaging within Tauri 2.1.
- All 20 CPU cores on the host machine are utilized through Tokio's work-stealing threadpool for concurrent WebSocket feeds, spread computations, and strategy automation.
