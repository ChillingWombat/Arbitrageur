# Arbitrageur system audit — 2026-10-02

Scope: full-stack review of strategy logic, execution, reconciliation, risk, security,
architecture, performance, UI/operator workflow, tests, CI, and deployment readiness.
Source reviewed at `main` / `8148340`. Runtime evidence was collected on the installed
testnet build through the dev-RPC surface; no live orders were submitted and the engine
was left paused with the tested bots off and no open positions.

## Executive verdict

**This is a serious prototype with unusually strong fail-closed instincts, but it is not
yet evidence of a profitable real-world arbitrage system.** The codebase now contains
important production-safety mechanisms: environment separation, live-entry gates, durable
order-intent records, bounded IOC dispatch, venue-truth reconciliation, restart recovery,
durable reduce-only flags, residual-flattening attempts, conservative margin checks, and
honest “unknown” states in several UI surfaces. Those controls materially reduce the
chance that a UI click becomes an unrecorded or silently orphaned live order.

However, **none of that proves edge**. A strategy can be safe and still lose money. The
current system does not provide convincing evidence that its expected spreads survive
real queue position, asynchronous multi-leg fills, venue outages, funding drift, borrow /
transfer / gas costs, adverse selection, and out-of-sample regime change. On the observed
testnet snapshot, the Spread Radar had **720 monitored rows, 29 triggered rows, and zero
ready/executable rows**; the options-parity scan returned **zero candidates**. Those results
are consistent with fail-closed behavior, not proof of tradeable opportunity.

The highest-risk gap is therefore not “missing safety controls” — the controls are real —
but **the distance between modeled opportunity and realized executable P&L**. Until that
distance is measured with realistic replay and small-notional live canaries, positive
`expected` fields should be treated as research estimates, not profit.

## Evidence quality and limitations

Evidence used in this audit falls into three classes:

- **Confirmed source facts**: verified directly against the listed files and line ranges.
- **Measured runtime observations**: collected from the installed testnet runtime via
  dev-RPC and repository test commands on 2026-10-02.
- **Judgment / risk assessment**: inference from source and runtime behavior; these are
  marked as risks or recommendations rather than proven defects.

Important limitations:

- The runtime exercise covered testnet only. Venue testnets do not reproduce production
  liquidity, rate limits, queue position, outages, or adversarial flow.
- No live order was dispatched, so this audit cannot prove that the complete real-money
  path reconciles under venue stress.
- The workspace and frontend suites are green, but the nominal E2E suite is red and is
  mostly mock/source-shape verification rather than a real engine-level end-to-end test.
- Runtime latencies and WebSocket lane failures are point-in-time observations, not a
  statistically valid latency distribution.

## Verified repository and runtime state

### Verification performed

- `cargo fmt --all -- --check`: green.
- `cargo check --workspace --locked`: green.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: green.
- `cargo test --workspace --locked`: green — **1,276 tests passed, 0 failed**
  (6 + 34 + 610 + 340 + 55 + 154 + 22 + 55 across the reported test binaries).
- `cd frontend && npm test`: green — **415 tests passed, 0 failed**.
- `bash scripts/run_e2e_tests.sh`: **red — 43 passed, 9 failed**. Every failure was an
  `ENOENT` for `docs/AUDIT_ISSUES_32_61.md`; the file now exists at
  `docs/archive/AUDIT_ISSUES_32_61.md`. This is a stale test-path defect, not proof that
  nine product behaviors regressed, but it still means the runner is not green.

### Runtime safety snapshot

The installed release was exercised only in `testnet`. The final observed state was:

- `environment = testnet`
- `is_live_mode = false`
- `is_paused = true`
- `is_stopped = false`
- `open_positions = []`
- `total_realized_pnl = 0.0`
- `total_trades_count = 0`
- `successful_trades_count = 0`
- `circuit_breaker_status = ACTIVE`, `circuit_breaker_tripped = false`
- Radar bot disabled/off
- Options-parity scan: zero candidates
- No live orders submitted

Observed testnet venue/connectivity notes:

- Several venues reported online/executable through the runtime status surface, but many
  others were offline, unsupported, or non-admitted.
- Some WebSocket lanes were connected, while Hyperliquid and Vanta were in paused/backoff
  states; Vanta’s latest error included `Connection reset without closing handshake`.
- Measured venue latency values in the runtime status surface were approximately **177–459
  ms** for several venues. That is large enough to matter for cross-venue entry quality.
- The latest monitor snapshot contained 720 rows, 29 `triggered` rows, **0 ready /
  executable rows**. Block reasons included coverage below the configured floor, variation
  under friction, uncomputable win estimate, unknown funding, spread above session median,
  and signal-not-ready states.

## System architecture

### What the architecture actually looks like

The workspace is nominally modular: the root `Cargo.toml` lists separate crates for core,
math, exchanges, risk, execution, strategy, app, and the desktop/Tauri crate
(`Cargo.toml:1-11`). Documentation presents a clean modular picture. The implementation is
more concentrated than that picture implies:

- `frontend/src-tauri/src/commands.rs` is **18,440 lines**.
- `frontend/src-tauri/src/monitor.rs` is **6,681 lines**.
- `frontend/src-tauri/src/execution.rs` is **8,434 lines**.
- `frontend/src-tauri/src/state.rs` is **4,426 lines**.
- `crates/arbitrageur-exchanges/src/dex_perps.rs` is **13,761 lines**.
- `frontend/src/components/OpportunityRadar.tsx` is **2,732 lines**.

`AppEngineState` is effectively a broad service locator: adapters, locks, caches,
counters, telemetry, settings, runtime flags, and strategy services all live there. That
makes the code navigable by search, but not truly modular in the deep-module sense: the
object exposes too much surface and lets callers bypass narrow seams.

### Architecture strengths

- **Environment separation is real.** Adapters and persisted rows are scoped by
  `testnet` / `live`; the live gate is checked again at dispatch.
- **Fail-closed adapter capabilities.** `live_entry_gate` requires an unlocked vault,
  live order submission, order-status polling, strategy-entry admission, and loaded
  credentials (`commands.rs:32-64`). Unsupported methods default to failure in the
  `ExchangeAdapter` trait rather than pretending capability.
- **Durable intent before dispatch.** Triangle and options paths persist leg intents before
  order submission (`triangle.rs:496-549`, `options_parity.rs:807-826` region), reducing the
  crash-window between “order left process” and “ledger knew about it.”
- **Reconciliation is venue-truth based.** The reconciliation supervisor polls durable
  non-terminal legs every 5 seconds, escalates after 15 minutes, bounds cancel attempts,
  bounds residual retries, and does not assume terminal state from ambiguous responses
  (`reconciliation.rs:1-67`, `reconciliation.rs:159-260`).
- **Freshness checks and full-depth checks exist.** Route economics require full requested
  quantity on both entry books and can include two-way exit costs (`commands.rs:606-698`).
  Dispatch paths re-resolve books instead of trusting the UI’s displayed edge.
- **Persistence is deliberately durable.** The main SQLite store uses WAL, synchronous
  `FULL`, a 5s busy timeout, restricted permissions, and schema migrations; telemetry uses
  a separate `synchronous = NORMAL` sidecar for high-volume samples
  (`state_store/mod.rs:2617-2744`).

### Architecture weaknesses

1. **God-file / god-state concentration.** The most important business rules are still in
   a few oversized Tauri files. This creates high blast radius for every change and makes
   hidden coupling likely. The crate diagram should not be treated as the true module map.
2. **Command, engine, and domain logic are mixed.** IPC commands, strategy scanning,
   execution orchestration, reconciliation helpers, DTO shaping, and policy decisions share
   files and helpers. That slows safe changes because every edit touches multiple concerns.
3. **Unclear strategy ownership boundaries.** Strategy code reaches through
   `AppEngineState` to adapters, stores, settings, and shared helpers. A purer strategy
   kernel would make the economics deterministic and easier to property-test / replay.
4. **Headless deployment is a placeholder.** `arbitrageur serve` is explicitly a read-only
   health placeholder: it binds a loopback socket, returns a static `503`, and performs no
   market-data ingestion or execution (`crates/arbitrageur-app/src/main.rs:77-83`,
   `691-745`). The desktop/testnet systemd path exists, but production headless operation
   is not equivalent to a real engine service.

### Recommended architecture target

Introduce narrow runtime seams, then move code behind them:

- `MarketDataService`: admits venues, owns book/funding freshness, publishes versioned
  snapshots, and exposes lane health.
- `StrategyEngine`: pure inputs → typed opportunity/evaluation outputs; no vault, SQLite,
  or adapter calls inside the pure path.
- `ExecutionCoordinator`: converts a validated candidate into a durable execution plan,
  submits child orders, tracks in-flight state, and owns hedge/unwind policy.
- `RiskService`: single authority for pause, circuit breaker, margin, concentration,
  reduce-only, and strategy/session gating.
- `Reconciler`: consumes durable order intents and venue truth; emits terminal ledger
  transitions and operator-review events.
- `VenueRegistry`: isolates capability, credentials, and endpoint/admission checks.
- `TelemetryStore`: separated from ledger state, with explicit retention and quality flags.

This is mostly a refactoring strategy: keep the existing safety semantics, but narrow the
surfaces through which they are reached.

## Strategy and economic validity

### Spatial / spread arbitrage

The core route economics are more honest than typical “top-of-book spread” demos. The code
computes best bid/ask, walks full requested quantity through both books, counts entry
slippage, includes exit slippage and closing spread in two-way mode, and reports
`expected_net_bps = top_gross_bps - expected_friction_bps` (`commands.rs:606-698`).

That is good input hygiene. It is still not sufficient for profitability:

- The model is a **snapshot forecast**, not an executable guarantee. Two venues cannot be
  filled atomically; either leg can miss, move, partially fill, or be cancelled while the
  other executes.
- One-way mode omits exit friction by design (`commands.rs:636-660`). That is acceptable as
  an “entry economics” estimate only if the UI and downstream logic clearly label it as
  non-realized carry/convergence exposure rather than completed arbitrage profit.
- The model does not capture queue position, order-book update races, venue-side rejects,
  post-trade funding/borrow changes, transfer costs, or asymmetric margin requirements.
- Full-book VWAP is useful, but still assumes the displayed book is fillable at dispatch.
  For cross-venue arbitrage, the dominant residual risk is **leg risk**, not nominal depth.

**Judgment:** treat positive `expected_net_bps` as a candidate score, not expected realized
PnL. Promote a route only after replay shows that the modeled edge survives realistic order
latency distributions and partial-fill / cancel outcomes.

### Spread Radar / convergence monitor

The monitor is the most statistically sophisticated strategy surface. It builds pair series,
computes baseline mean/std, derives qualification windows from OU half-life or a minimum
window, requires enough observations, models fees/funding/exit costs/carry, builds empirical
win-probability curves, applies Wilson lower bounds, and blocks on stale books, unknown
funding, thin evidence, unresolved signals, reduce-only state, and paused engine
(`monitor.rs` broad implementation; UI source at `OpportunityRadar.tsx:1413-1648`).

This is a real improvement over naive z-score entry, but its statistical credibility is
still limited:

- Empirical episode counts are likely small and strongly regime-dependent.
- Multiple testing across hundreds of pairs can turn noise into apparent candidates.
- OU fits and historical qualification windows are vulnerable to nonstationarity and quote
  contamination.
- Funding availability and venue coverage gates are correctly fail-closed, but they also
  make the signal universe highly conditional; the observed 0-ready state is plausible.
- A positive expected-profit field can coexist with a blocked status. The UI partially
  handles this through tooltips and blocked reasons, but the semantics can still be
  misread by an operator under time pressure.

**Judgment:** this is the right direction, but it needs out-of-sample calibration,
multiple-testing control, and a replay harness before it deserves production trust.

### Funding / carry

The funding surface is economically richer than a simple APR sort. Its DTO includes net
rate, break-even hours, expected holding horizon, expected return, empirical backtest win
rate with Wilson bounds, persistence probability, episode counts, weekday/weekend splits,
depth curves, and optional Boros fixed-yield legs (`funding.rs:33-216`).

Strengths:

- The design acknowledges that funding carry is time-bearing, not instant arbitrage.
- It models entry spread, fees, persistence, horizon uncertainty, and capacity.
- It surfaces empirical backtest statistics rather than only point estimates.

Critical concerns:

- Funding rates can gap or reverse around settlement; carry is not locked unless the
  fixed-yield leg is real, executable, funded, and reconciled.
- The strategy still faces two-leg entry risk and asymmetric liquidation/margin terms.
- Spot/perp basis needs transfer, borrow, inventory, and withdrawal-cost realism.
- “Expected return” must be validated against realized carry over many independent regimes,
  not a single recent funding print.

### Triangular arbitrage

The triangular path is a single-venue three-leg cycle. It re-scans the opportunity id on
fresh books, serializes dispatch under `execution_lock`, re-checks pause and circuit
breaker under the lock, applies the live gate only in live mode, checks stablecoin depeg,
revalidates depth, applies leverage before margin snapshot, uses conservative margin bounds,
persists each leg intent before submit, and classifies partial cycles into reconciliation
(`triangle.rs:632-845`, `triangle.rs:936-959`, `triangle.rs:552-563`).

This is structurally better than trusting a displayed route, but the economic edge remains
fragile:

- Three sequential IOC legs create more residual paths than a two-leg trade.
- The cycle’s edge can disappear during leg 1 or leg 2; bounded IOC protects price, not
  completeness.
- Fees, book movement, and wire-grid rounding can turn a “complete” cycle into dust plus
  adverse residual inventory.

### Options parity

The options-parity evaluator uses put-call parity on Deribit: `C − P = F − K`, compares a
synthetic forward against the perpetual including expected funding drain, requires exact
executable-side quotes, computes option fee caps, and dispatches a three-leg IOC cycle under
the same durable-intent / reconciliation discipline (`options_parity.rs:1-19`,
`184-249`, `620-845`).

Strengths:

- The economics are explicit and fail closed on missing sides.
- Funding carry to expiry is included rather than ignored.
- The observed scan produced zero candidates, which is consistent with a real, selective
  scan rather than fabricated opportunities.

Risks:

- Option books are thin and discontinuous; top-level quotes can be stale or wide.
- Three legs and option contract granularity increase residual and rounding risk.
- Funding over expiry is a forecast; margin and liquidation behavior can dominate the carry.

### Prediction-market parity

This surface is currently not an executable strategy. `scan_prediction_opportunities_impl`
returns an empty vector (`commands.rs:2696-2702`), and `execute_prediction_arbitrage_impl`
is an explicit disabled stub until authentic order submission and reconciliation exist
(`execution.rs:3238-3259`). This is safer than fake simulation, but it means the prediction
feature should be described as **not implemented**, not as a live capability.

### Spot cross-venue / other strategies

The spot scan is a real scan over admitted venues and canonical spot instruments; it refuses
to merge quote families that would leave residual quote-asset basis (`commands.rs:2307-2351`).
That is the right conservative approach. Any remaining profitability claim still needs the
same replay / venue-truth validation as the futures paths.

## Execution, reconciliation, and order correctness

### Strong controls worth preserving

- `live_entry_gate` is checked at dispatch, not only at scan (`commands.rs:32-64`).
- Dispatch paths serialize through `execution_lock` and re-check pause/trip under the lock
  (`triangle.rs:684-710`, `options_parity.rs:620-634`).
- Order intents are persisted before submission (`triangle.rs:496-549`).
- Client order ids are persisted before dispatch for reconciler recovery
  (`options_parity.rs:834-843` region).
- `ExchangeAdapter` capability defaults are fail-closed; venue support must be explicit.
- Reconciliation distinguishes working, filled, cancelled-empty, partial-terminal, and
  unknown outcomes, and never resolves a signal off partial evidence
  (`reconciliation.rs:117-260`).
- Restart recovery runs a reconciliation pass immediately, then every 5 seconds
  (`reconciliation.rs:20-21`, `37-67`).
- Working orders past 15 minutes escalate to `unresolved`, bounded cancel retries prevent
  spam, and residual flattening attempts are bounded (`reconciliation.rs:37-67`).

### Remaining execution risks

- **Atomicity does not exist across venues.** The design accepts this and reconciles
  residual risk, but the economics should price the expected cost of the unwind path.
- **Partial fills are handled structurally, not yet economically.** They are detected,
  persisted, escalated, and flattened. What is not yet proven is that the realized cost of
  those events is represented in the expected-profit model.
- **Venue truth can be delayed or contradictory.** The reconciler is conservative, but a
  venue outage can still leave capital exposed longer than a market-neutral strategy can
  tolerate.
- **Autonomous exits under Off mode need explicit guarantees.** The Radar bot is designed
  to default off, block live autonomous entries, and recover state after restart
  (`auto_exec.rs` design and implementation), but the operator-facing contract should make
  clear whether “off” can strand exits and how residual supervision continues.

## Risk controls and operational safety

### What is credible

- Circuit-breaker and paused checks gate entry paths.
- Margin evaluation treats missing liquidation data as unknown, not healthy
  (`crates/arbitrageur-risk/src/margin_monitor.rs:5-120`).
- Vault lock is refused while live positions are open because a locked vault cannot
  supervise exits (`frontend/src-tauri/src/accounts.rs:153-188`).
- Testnet/live promotion requirements in `docs/CAPABILITIES.md` are appropriately strict:
  exact market/quantity conversion, terminal order handling, partial fills, reduce-only
  compensation, leverage confirmation, durable records, restart recovery, residual
  resolution, cancel retry, sandbox tests, and small-notional canary.
- The UI’s emergency and acknowledge surfaces expose unresolved/imbalanced signals rather
  than hiding them (`OpportunityRadar.tsx:2243-2294`).

### What is not yet proven

- The system’s risk limits have not been shown to hold under concurrent strategy use,
  correlated venues, missing funding, or a live outage during a multi-leg cycle.
- Margin checks are pre-dispatch; they cannot by themselves prevent adverse residual
  liquidation after a one-leg fill.
- The runtime remains dependent on operator judgement when signals are escalated. That is
  acceptable for a desk tool, but it is not autonomous production safety.

## Security and custody

### Positive findings

- Credential custody uses AES-256-GCM with Argon2id; EVM custody uses Iota Stronghold.
- Optional RFC-6238 TOTP exists for sensitive vault operations.
- Vault lock is blocked during live open positions, preventing a credential wipe that
  would disable exits.
- SQLite state and telemetry files are created with restrictive Unix permissions; the main
  store uses `synchronous = FULL`, WAL, busy timeout, and `trusted_schema = OFF`
  (`state_store/mod.rs:2617-2744`).
- The headless placeholder refuses non-loopback bind unless explicitly overridden
  (`crates/arbitrageur-app/src/main.rs:695-717`).

### Concerns and recommendations

- The UI schedules vault lock only on settings-close and `beforeunload`; this is useful,
  but a true inactivity lock policy should be explicit and tested.
- Secrets must never appear in review documents, logs, tests, or dev-RPC examples. This
  audit intentionally excludes the temporary password and runtime identifiers.
- The desktop process holds substantial authority while unlocked. Add OS-keychain /
  hardware-backed custody options where practical, and document the threat model for a
  compromised local session.
- Continue gitleaks + cargo-audit + npm audit in CI; these are already wired
  (`.github/workflows/ci.yml:112-161`).

## Performance and latency

### Measured and observed

- Venue latency values observed on testnet were approximately **177–459 ms** for several
  venues. For cross-venue arbitrage, this is material: the edge can decay between scan,
  dispatch, and hedge.
- WebSocket lanes showed instability in the runtime snapshot: Hyperliquid and Vanta had
  paused/backoff states; Vanta logged `Connection reset without closing handshake`.
- The engine’s own feed layer has bounded connects, validation before hub update, reconnect
  backoff, keepalive, and telemetry (`feed.rs`). That is good, but observed instability is
  still a production concern.

### Frontend and IPC load

The desktop UI polls aggressively:

- `App.tsx` refreshes a broad set of status, settings, books, scans, risk, logs, configs,
  positions, balances, signals, reduce-only flags, and capabilities every **3 seconds**
  (`frontend/src/App.tsx:455-598`).
- `OpportunityRadar` polls monitor snapshot, bot status, signal executions, fill PnL, and
  open positions every **5 seconds** (`OpportunityRadar.tsx:1919-1960`).
- Additional intervals poll vault status every 3s, exemptions every 15s, auto-blocked
  tickers every 30s, market universe every 30s, and volumes every 30s
  (`frontend/src/App.tsx:254-367`, `631-679`).

This is easy to reason about but expensive. It can create overlapping scans and large IPC
payloads even when nothing changed. The timeout fallbacks prevent one stalled request from
locking the UI, but they do not reduce underlying work.

### Backend/data-path concerns

- The main state store is durable but not designed as a low-latency bus. That is fine for
  ledger truth; it should not become the hot path for every market-data frame.
- Telemetry is correctly separated into a `synchronous = NORMAL` sidecar with hard row caps
  (`state_store/mod.rs:2274-2291`, `2709-2784`).
- Retention is bounded: 50,000 signal-event rows, 4,000,000 spread rows, and 1,000,000
  volume rows (`state_store/mod.rs:962-975`). That prevents unbounded growth but can still
  leave hundreds of MB of telemetry and expensive replay reads.
- There is no `[profile.release]` section in the workspace `Cargo.toml` observed at HEAD;
  consider opt-level/LTO/codegen-units tuning only after correctness and profiling justify it.

### Performance recommendations

1. **Replace broad polling with event-driven invalidation.** Push monitor snapshots,
   signal transitions, position updates, and venue-lane health through Tauri events or
   server-sent subscriptions; keep slow REST polls as fallback.
2. **Split cadences.** Risk/positions should update fast; configs, market universe,
   volumes, and account metadata should update slowly or on change.
3. **Version snapshots.** Emit a monotonically increasing market-state version so UI and
   strategy scans can compare same-time books rather than arbitrary IPC arrival order.
4. **Batch telemetry writes.** Keep WAL + sidecar, but aggregate spread frames into
   bounded micro-batches to reduce SQLite contention and fsync overhead.
5. **Cache by dependency, not by wall clock.** Cache scans keyed by venue/book/config
   versions; invalidate on those versions instead of recomputing every 3–5 seconds.
6. **Precompute pair matrices.** Incremental series caching already exists; continue
   pushing repeated history reads into incremental in-memory windows.
7. **Profile dispatch latency end-to-end.** Measure scan→dispatch, dispatch→ack,
   ack→venue-terminal, and cross-venue skew separately. Add p50/p95/p99 dashboards.
8. **Reduce clone-heavy hot paths.** Large DTOs and book structures should move as `Arc`
   or compact structs in hot loops; avoid serializing entire snapshots every poll.
9. **Keep a strict freshness contract.** Speed improvements must not weaken book age,
   venue identity, or env scoping.

## UI/UX and operator workflow

### Strengths

- `ToggleSwitch` uses accessible switch semantics (`role="switch"`, `aria-checked`, button
  behavior), which is a solid baseline.
- Radar distinguishes `ready`, `triggered`, `tracking`, `preparing`, and `no-data` styles,
  and tooltips distinguish reversion probability from directional trade win probability.
- Execute buttons are disabled when `row.executable` is false and expose the block reason
  in tooltips (`OpportunityRadar.tsx:1636-1641`, `1776-1781`).
- Unresolved/imbalanced signals are escalated visibly rather than hidden.
- The status ribbon surfaces min-win and min-expected hurdles directly.

### Weaknesses and risks

- A row can show attractive `Exp Profit` / `Opt Size` values while still non-executable.
  The UI does flag blocked state, but the visual hierarchy still invites the eye to profit
  columns first. For a live desk, blocked rows should be visually subordinated more clearly.
- Several semantics are subtle: `Win %` is directional trade win probability, `Net %` is an
  empirical P(net>0), `Reversion probability` is informational, and “optimal size” is a
  model output. This is defensible but cognitively demanding; operators need a legend and
  stricter naming.
- Manual size input and execute actions sit next to model-derived `optimal_size`; make sure
  the default cannot silently become an operator-sized order larger than the modeled
  capacity.
- The broad 3s refresh and 5s radar polling can make UI state churn even when the engine is
  idle. Event-driven updates would reduce both CPU and operator noise.

## Testing, CI, and observability

### Test posture

- Rust workspace tests are broad and green at HEAD: **1,276 passed**.
- Frontend tests are green: **415 passed**.
- The nominal E2E runner is red: **43 passed / 9 failed**, all due to the stale audit-doc
  path (`tests/e2e/tier1_feature_coverage.test.ts:25-26`,
  `tier2_boundary_corner.test.ts:24-26`).

### Important caveat about “E2E”

The Node “E2E” suite is not a true end-to-end execution harness against the live Rust
engine. It largely validates source shape, mocks, and helper behavior. Useful, but it must
not be counted as evidence that a real two-leg or three-leg order path works under venue
truth.

### CI strengths

CI enforces fmt, clippy, workspace tests, frontend tests/build, npm audit, cargo audit,
gitleaks, vendored signer provenance, and systemd template validation
(`.github/workflows/ci.yml:21-161`). This is a strong baseline.

### Operational readiness

- There is a real SQLite online-backup script using `.backup`, restrictive permissions,
  integrity verification, and instance-scoped retention (`ops/deploy/backup-sqlite.sh:1-77`).
- The headless systemd templates exist and are CI-validated, but the headless binary itself
  remains a read-only placeholder.
- Observability is strongest inside the desktop/dev-RPC surface. Production-grade alerting,
  metrics export, health SLOs, external dashboards, and paging are not yet evidenced as a
  complete operating stack.

## Findings register

Severity scale: **Critical** = can directly cause uncontrolled loss or invalidate safety;
**High** = blocks credible production/profitability use; **Medium** = important engineering
or operator risk; **Low** = cleanup / polish.

| ID | Severity | Finding | Evidence | Recommendation |
|---|---|---|---|---|
| F-01 | Critical | Profitability is unproven. The code models edge, but no realistic out-of-sample evidence shows realized profit after leg risk, latency, fees, funding drift, and partial fills. | Route economics are forecasts (`commands.rs:606-698`); runtime showed 0 executable Radar rows. | Build a replay/cost simulator and small-notional live canary program before promoting any “profit” claim. |
| F-02 | High | Cross-venue leg risk is structurally mitigated but not economically priced. | IOC bounds + reconciler exist; dispatch is still multi-leg asynchronous. | Add expected residual/unwind cost to opportunity score; tune sizing from measured hedge failure rates. |
| F-03 | High | Feed latency and lane instability can erase short-lived edge. | Observed venue latencies ~177–459 ms; WS lanes paused/backoff on Hyperliquid/Vanta. | Enforce per-route latency budgets, cross-venue skew limits, and lane-health gating in the opportunity model. |
| F-04 | High | Statistical credibility of Radar/Funding edges is fragile. | Empirical probabilities and OU/history gates exist, but sample sizes and regimes are limited. | Use walk-forward validation, multiple-testing correction, regime splits, and confidence-aware capacity caps. |
| F-05 | High | `commands.rs`, `execution.rs`, `monitor.rs`, `state.rs`, and `dex_perps.rs` concentrate too much behavior. | File sizes: 18,440 / 8,434 / 6,681 / 4,426 / 13,761 lines. | Split into narrow services and pure strategy kernels before further feature expansion. |
| F-06 | High | The nominal E2E suite is red and not a real end-to-end engine test. | `43 pass / 9 fail`; stale `AUDIT_ISSUES_32_61.md` path. | Fix the path; rename or replace the suite with real engine-level scenario tests. |
| F-07 | High | Headless `serve` is only a placeholder, not a production daemon. | `main.rs` returns static 503, no ingestion/execution. | Either build a real headless engine or remove/relabel the deployment claim. |
| F-08 | High | Prediction-market strategy is not implemented. | Scan returns `Vec::new`; execution returns disabled stub. | Remove from capability surfaces or implement real market/order support behind explicit readiness flags. |
| F-09 | High | UI may over-emphasize positive expected fields on blocked/non-executable rows. | Radar table shows profit columns alongside status/tooltips. | Add a dedicated “Executable” visual state, suppress profit emphasis for blocked rows, and make block reason first-class. |
| F-10 | Medium | Frontend polling is broad and expensive. | `App.tsx` 3s refresh-all; `OpportunityRadar` 5s multi-call poll. | Move to event-driven updates and cadence-split data domains. |
| F-11 | Medium | SQLite telemetry can grow large enough to make replay/backup operations heavy. | Row caps allow ~4M spread + ~1M volume rows. | Add rollups, configurable retention, export/archival, and replay-specific compaction. |
| F-12 | Medium | Vault locking is partly UI-scheduled rather than an enforced inactivity policy. | `scheduleVaultLock` + settings-close / `beforeunload`. | Add backend-enforced inactivity timeout and explicit unlock-session expiry. |
| F-13 | Medium | Reconciliation depends on operator acknowledgement for terminal edge cases. | `unresolved` / `imbalanced` UI requires review. | Add an operator runbook, timed escalation, and optional safe auto-remediation policies. |
| F-14 | Medium | Capability truthfulness is good but still needs automated canary proof per venue. | `live_entry_gate` and capability flags exist. | Add per-venue live canary suite that verifies submit, poll, cancel, partial fill, residual flatten, restart recovery. |
| F-15 | Medium | Backup/restore exists but scheduled deployment proof is not evidenced. | `backup-sqlite.sh` + restore script exist. | Install a timer/cron, test restore regularly, and alert on failed backups. |
| F-16 | Low | Release-performance tuning is not explicit in workspace metadata. | No `[profile.release]` in root `Cargo.toml` observed. | Profile first; then consider `lto = "thin"`, codegen-units tuning, and allocator measurements. |
| F-17 | Low | Naming overload increases operator risk. | Multiple win/probability/net fields with different definitions. | Rename UI labels around user intent: “Trade win %”, “Empirical net-positive %”, “Reversion only”. |
| F-18 | Low | Test suite breadth is good but lacks adversarial venue chaos. | Unit coverage exists; mock E2E is not adversarial enough. | Add fault-injection tests for delayed acks, duplicate fills, contradictory venue status, reset connections, stale books. |

## Prioritized remediation roadmap

### Phase 0 — stop overclaiming (immediate)

1. Fix the stale E2E path or point it at `docs/archive/AUDIT_ISSUES_32_61.md`.
2. Rename the Node suite if it remains mock/source-shape based; reserve “E2E” for tests
   against the real engine.
3. Mark prediction-market scanning/execution as unavailable in docs/UI until implemented.
4. Treat every expected-profit field as a forecast, not a realized-profit claim.

### Phase 1 — prove correctness under venue truth

1. Create an engine-level execution harness with simulated adapters that support:
   - delayed acknowledgements,
   - partial fills,
   - contradictory terminal statuses,
   - cancels rejected,
   - venue disconnects,
   - process restart mid-cycle,
   - residual flatten retries.
2. Add per-venue testnet canaries for submit → poll → terminal → cancel → residual.
3. Add a real small-notional live canary for any venue claiming live readiness.
4. Add metrics for dispatch latency, hedge skew, unwind cost, time-to-flat, and unresolved
   exposure duration.

### Phase 2 — prove economics

1. Build a replay/backtest system over recorded order books, funding prints, and lane
   latency, not just mid-price series.
2. Model queue position, adverse selection, maker/taker mix, missing depth, leg asymmetry,
   funding/borrow, transfer/gas, and partial-fill unwind costs.
3. Add walk-forward windows, non-overlapping episode accounting, bootstrap confidence,
   multiple-testing control, and regime segmentation.
4. Produce route-level expected value distributions, not only point estimates.
5. Promote strategies only if out-of-sample expected value stays positive under stressed
   cost assumptions.

### Phase 3 — architecture refactor

1. Extract a pure strategy kernel for route economics, monitor qualification, funding
   carry, triangle, and options parity.
2. Split `commands.rs` and `execution.rs` into narrower modules around IPC, orchestration,
   and domain policies.
3. Reduce `AppEngineState` to composition root + narrow service handles.
4. Make `ExecutionCoordinator` own the whole parent/child order lifecycle and remove
   duplicated leg-management logic per strategy.
5. Keep `Reconciler` independent from strategy semantics; it should consume durable order
   intents and venue truth only.

### Phase 4 — performance work

1. Replace broad frontend polling with event subscriptions and stale-time-aware queries.
2. Version market-data and monitor snapshots; render only changed rows.
3. Batch telemetry writes and add rollups for long-window analytics.
4. Measure IPC payload sizes, Rust dispatch timing, venue ack timing, and DB wait time.
5. Add route-level latency/skew budget enforcement.

### Phase 5 — production operations

1. Decide whether a real headless daemon is in scope. If yes, implement a real engine
   service instead of the current placeholder.
2. Add external metrics/alerting around circuit breaker trips, unresolved signals, feed
   lane failures, reconciliation lag, and backup failures.
3. Schedule and test backups/restores.
4. Document operator runbooks for stuck orders, imbalanced signals, venue outage, vault
   lockout, and emergency flatten.
5. Maintain a capability matrix that is generated from runtime checks, not hand-edited
   claims.

## Overall assessment

The project’s best trait is **defensive correctness**. It does not blindly trust displayed
edge, it persists intent before order submission, it refuses live entry when the
reconciliation prerequisites are missing, and it treats ambiguous venue state as unresolved
rather than inventing closure. Those are exactly the traits a real trading system needs.

The weakest trait is **evidence of edge**. The current system is much closer to a cautious
execution/control platform than to a proven profitable quant engine. The next meaningful
milestone should not be “more venues” or “more UI”; it should be a reproducible evidence
pipeline that shows the modeled opportunities still make money after realistic execution
friction and failure modes. Until then, the correct posture is: **promising, guarded, but
not production-proven.**
