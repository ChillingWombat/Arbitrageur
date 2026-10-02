# Arbitrageur System Comprehensive Review & Deep-Dive Audit — 2026-09-25

- **Date:** 2026-09-25
- **Auditor:** Gemini 3.8 Flash (High) / Antigravity Agentic Pair
- **Audit Scope:** Full system review: codebase verification, architectural safety, execution atomicity, mathematical foundations, venue adapters, risk state machines, database durability, frontend resilience, and operational deployment.
- **Repository:** `/mnt/Data/Projects/Arbitrageur`
- **Revision Audited:** `9519837` ("Add USD-quote policy modes, Boros fixed-yield hedges, and Stronghold-custodied EVM AMM execution")
- **Actionable Tracking:** 34 GitHub issues indexed and filed (#28 through #61).

---

## 1. Executive Summary

Arbitrageur is an ambitious, high-performance cross-venue arbitrage and carry system combining Rust (Tauri desktop backend + multi-crate workspace) and TypeScript / React. The architecture exhibits strong engineering discipline in several mission-critical areas: durable order state tracking before submit on spatial and funding paths, Argon2id/AES-256-GCM vault security with field-level reveal protection, extensive mock and wire test coverage (1000 workspace tests passing, clippy clean, 0 frontend test failures), and fail-closed reconciliation.

However, an exhaustive multi-dimensional audit of code, logic, mathematics, execution dynamics, and venue interactions uncovered **critical vulnerabilities that threaten capital, violate execution safety rules, or produce completely incorrect mathematical results**.

### The Most Severe Findings:
1. **Triangular Arbitrage Math Double-Counts Slippage (P0 - #52):** In `triangular.rs`, `end_notional` already simulates chained order execution at VWAP prices (absorbing depth slippage). The code then subtracts `total_slippage_bps` again from `gross_bps`, double-counting the slippage penalty and converting profitable trades into phantom losses.
2. **Triangular Arbitrage Multi-Currency Cashflow Summation (P0 - #53):** In `triangle.rs:830-838`, `attempt_pnl` calculates signed cashflows across all 3 legs and calls `.sum()`. In a USDC → BTC → ETH → USDC cycle, it literally sums -USDC + ETH + USDC directly without converting the cross-pair intermediate cashflow (e.g. adding 2.0 ETH as \$2.00 to USDC), corrupting cumulative P&L and audit logs.
3. **Completed Triangular Cycles Persisted as Imbalanced Signals (P0 - #54):** In `triangle.rs:467-537`, even when all 3 legs of a triangular cycle execute 100% cleanly (leaving zero position inventory), every leg is recorded as a 1-leg `SignalStatus::Imbalanced` signal. This permanently blocks the three traded symbols from future trading and prompts the operator / auto-flatten system to execute unhedged flattens on non-existent inventory.
4. **Triangular Arbitrage Dispatches Unbounded Market Orders (P0 - #55):** In `triangle.rs:792-797`, all three legs are dispatched as `OrderRequest::new_market` with no limit price and no IOC constraint, allowing naked market orders to sweep deeply into thin cross-currency books (e.g. ETH-BTC).
5. **OKX and PopDEX Downgrade Price-Bounded IOC to Unpriced Market Orders (P0 - #28):** The two-leg dispatch builds price-bounded IOC orders, but OKX and PopDEX adapters discard the limit price and submit unpriced market orders.
6. **Liquidation-Trim `CloseBoth` Self-Deadlocks Global `execution_lock` (P0 - #29):** Re-entrant acquisition of `tokio::sync::Mutex` in `execution.rs:3931` and `:2791` permanently freezes the entire execution engine.
7. **Emergency Unwind Lacks Fill Polling, Tripping Breakers on DEXes (P1 - #56):** `EmergencyUnwindHandler::unwind_leg` requires `OrderStatus::Filled` immediately on submit. Asynchronous venues (Lighter, Decibel, Aster) return `OrderStatus::New` on placement, causing `unwind_leg` to immediately panic and trip the circuit breaker even though the unwind order filled onchain milliseconds later.
8. **Passive Unwind Live Fallback Submits Market Orders While Limits Are Working (P1 - #57):** In `passive_unwind.rs:674-706`, when order cancellation is unconfirmed or unverifiable via `fetch_order`, it falls through to `finish_live`, which submits an immediate market order, stacking orders and over-selling inventory.
9. **Convergence Auto-Exit Stop Logic Misclassifies Overshoots as Stop Losses (P1 - #58):** In `convergence_auto.rs:601-606`, the exit rule checks `dev.abs() >= stop_at`. If a trade enters at +10 bps and snaps into massive profit at -16 bps (spread inversion), `|-16| >= 15` triggers an adverse stop-loss, closing the winner and applying a punitive blackout cooldown.
10. **EVM AMM Synchronous Exact-Amount Approvals Block Atomic Dispatch for 30s (P1 - #59):** In `evm_amm.rs:1625-1656`, `place_order` performs onchain ERC-20 approvals with a 30-second block mining timeout while holding atomic execution locks. Its sibling CEX leg fills in 20ms, leaving capital unhedged for 15–30 seconds.

---

## 2. Verification Gate Results

All verification suites were run on the current working tree (`x86_64-unknown-linux-gnu`, Rust 1.96.0, Node v24.13.0):

| Suite | Command | Result | Details |
|---|---|---|---|
| **Workspace Unit Tests** | `cargo test --workspace --locked` | **PASS (100%)** | 1000 passed, 0 failed, 8 ignored |
| **Desktop Crate Tests** | `cargo test -p arbitrageur-desktop` | **PASS (100%)** | 450 passed, 0 failed in 70.85s |
| **Code Formatting** | `cargo fmt --all -- --check` | **PASS** | 0 formatting violations |
| **Static Analysis / Lints** | `cargo clippy --workspace --all-targets --locked` | **PASS** | 0 warnings / 0 errors with `-D warnings` |
| **Frontend Tests** | `npm test` (in `frontend/`) | **PASS (100%)** | 169 component & service tests passed |
| **Frontend Build** | `npm run build` | **PASS** | TypeScript + Vite compiled cleanly (bundle 896 kB) |
| **Dependency Audits** | `npm audit --omit=dev --audit-level=high` | **PASS** | 0 high/critical production vulnerabilities |

---

## 3. Detailed Audit Findings by Subsystem

### A. Mathematical Foundations & Strategy Logic

#### 1. Triangular Arbitrage Double-Counts Depth Slippage (P0 — Issue #52)
- **Location:** [`crates/arbitrageur-math/src/triangular.rs:118, 138, 146-150`](file:///mnt/Data/Projects/Arbitrageur/crates/arbitrageur-math/src/triangular.rs#L118-L150)
- **Mechanics:**
  In `evaluate_triangular_cycle`:
  - Buy legs chain proceeds via: `amount /= vwap;`
  - Sell legs chain proceeds via: `amount *= vwap;`
  - At the end of 3 legs, `end_notional = amount`.
  - Gross return is computed as:
    $$\text{gross\_bps} = \left(\frac{\text{end\_notional}}{\text{start\_notional}} - 1\right) \times 10000$$
    Because `end_notional` was already realized at VWAP, `gross_bps` is **already net of orderbook depth slippage**.
  - Line 149 then computes:
    $$\text{net\_bps} = \text{gross\_bps} - \text{total\_fee\_bps} - \text{total\_slippage\_bps}$$
  - **Slippage is deducted twice.**
- **Impact:** If top-of-book return is +60 bps, depth slippage is 25 bps, and fees are 6 bps: true net profit is $+29\text{ bps}$. Arbitrageur computes $\text{gross\_bps} = +35\text{ bps}$, and $\text{net\_bps} = 35 - 6 - 25 = +4\text{ bps}$. If slippage is 35 bps (true net $+19\text{ bps}$), the code computes $\text{net\_bps} = 25 - 6 - 35 = -16\text{ bps}$, discarding profitable trades and generating false negative radar signals.

#### 2. Cross-Currency Cashflow Direct Summation in Triangular P&L (P0 — Issue #53)
- **Location:** [`frontend/src-tauri/src/triangle.rs:830-841`](file:///mnt/Data/Projects/Arbitrageur/frontend/src-tauri/src/triangle.rs#L830-L841)
- **Mechanics:**
  ```rust
  let attempt_pnl: Decimal = fills
      .iter()
      .map(|(idx, r)| {
          let px = r.avg_fill_price.unwrap_or(dec!(0));
          let signed = match plan.order_sides[*idx] {
              Side::Sell => r.filled_qty * px,
              Side::Buy => -r.filled_qty * px,
          };
          signed - r.fee
      })
      .sum();
  ```
  In a triangle `USDC -> BTC -> ETH -> USDC`:
  - Leg 0: Buy `BTC-USDC` (quote USDC) $\rightarrow$ cashflow is $-Q_0 \times P_0\text{ [USDC]}$.
  - Leg 1: Sell `BTC-ETH` (quote ETH) $\rightarrow$ cashflow is $+Q_1 \times P_1\text{ [ETH]}$!
  - Leg 2: Sell `ETH-USDC` (quote USDC) $\rightarrow$ cashflow is $+Q_2 \times P_2\text{ [USDC]}$.
  - Summing them directly adds $\text{ETH} + \text{USDC}$ 1:1. 2.0 ETH received on Leg 1 is recorded as $\$2.00$ USDC!
- **Impact:** Realized P&L is mathematically nonsensical. Realized cycle profit for a closed triangle starting and ending in USDC is simply $\text{Cashflow}_{\text{Leg 2 [USDC]}} - \text{Cashflow}_{\text{Leg 0 [USDC]}} - \sum \text{Fees}_{\text{[USD]}}$.

#### 3. Inverted Convergence Stop Logic Kills Profitable Overshoots (P1 — Issue #58)
- **Location:** [`crates/arbitrageur-strategy/src/convergence_auto.rs:601-606`](file:///mnt/Data/Projects/Arbitrageur/crates/arbitrageur-strategy/src/convergence_auto.rs#L601-L606)
- **Mechanics:**
  `evaluate_exit` tests:
  ```rust
  let stop_at = (cfg.stop_mult * entry_dev_bps.abs()).max(friction_band_bps);
  if dev.abs() >= stop_at {
      return ExitEval::Exit {
          trigger: ExitTrigger::Stopped,
          reason: format!("deviation crossed stop band (|dev| {dev:.1} >= {stop_at:.1} bps)"),
      };
  }
  ```
  Taking `.abs()` discards trade direction. If a mean-reversion trade enters at deviation $+12\text{ bps}$ and snaps past zero to $-18\text{ bps}$, the trader earned $+30\text{ bps}$ of profit. But because $|-18| \ge 15$, `evaluate_exit` evaluates this as `ExitTrigger::Stopped`, records a stop loss, cuts the winner, and triggers `pair_cooldown_secs` blackout.

#### 4. Spatial Exit Stop Drop Inversion (P1 — Issue #34)
- **Location:** [`crates/arbitrageur-strategy/src/spatial_exit.rs:105-121`](file:///mnt/Data/Projects/Arbitrageur/crates/arbitrageur-strategy/src/spatial_exit.rs#L105-L121)
- **Mechanics:**
  `Stopped` fires when `live <= entry - stop_drop_bps`. Since mark-to-market P&L is approximately $\text{entry} - \text{live}$, a *falling* live edge represents a gain, cutting winners at $+5\text{ bps}$ while allowing adverse widening ($\text{live} > \text{entry}$) to run unstopped until max hold timeout ($86,400\text{s}$).

#### 5. Bertram First-Passage Time Formula Mismatch (P2 — Issue #46)
- **Location:** [`crates/arbitrageur-math/src/optimal_thresholds.rs:56-73`](file:///mnt/Data/Projects/Arbitrageur/crates/arbitrageur-math/src/optimal_thresholds.rs#L56-L73)
- **Mechanics:**
  The analytic formula implemented for Ornstein-Uhlenbeck expected passage time diverges by up to $20\times$ from numerical integration and Monte Carlo truth at $2\sigma$–$4\sigma$ boundaries. Furthermore, the objective function optimizes $(d - c)/\sqrt{E[T]}$ instead of Bertram's rate-of-return objective $(d - c)/E[T]$.

---

### B. Execution Safety & Order Truth

#### 1. Permanent Symbol Lockout from Completed Triangular Cycles (P0 — Issue #54)
- **Location:** [`frontend/src-tauri/src/triangle.rs:467-537, 854-882, 934`](file:///mnt/Data/Projects/Arbitrageur/frontend/src-tauri/src/triangle.rs#L467-L537)
- **Mechanics:**
  A completed 3-leg cycle is flat. Yet `record_triangle_leg_fill` marks every filled leg as `SignalStatus::Imbalanced`.
- **Impact:**
  - `blocking_signals` flags all 3 symbols (`BTC-USDC`, `BTC-ETH`, `ETH-USDC`) as locked.
  - The reconciliation supervisor (`reconciliation.rs`) and UI prompt the user to unwind positions that do not exist, causing accidental naked shorting or spot liquidation.

#### 2. Unbounded Market Orders Dispatched on Triangular Arbitrage (P0 — Issue #55)
- **Location:** [`frontend/src-tauri/src/triangle.rs:792-797`](file:///mnt/Data/Projects/Arbitrageur/frontend/src-tauri/src/triangle.rs#L792-L797)
- **Mechanics:**
  Orders are constructed via `OrderRequest::new_market(adapter.venue_id(), sym, *order_side, qty)` with no price bound and no IOC TIF. Cross-currency books with thin liquidity are swept at arbitrary market prices.

#### 3. Premature Emergency Unwind Failure on Asynchronous DEX Venues (P1 — Issue #56)
- **Location:** [`crates/arbitrageur-execution/src/emergency_unwind.rs:36-88`](file:///mnt/Data/Projects/Arbitrageur/crates/arbitrageur-execution/src/emergency_unwind.rs#L36-L88)
- **Mechanics:**
  `EmergencyUnwindHandler::unwind_leg` synchronously inspects `resp.status == OrderStatus::Filled`. On DEXes where order submission returns `OrderStatus::New` (e.g. Lighter, Decibel), it fails instantly without polling `fetch_order`. This trips the circuit breaker and dumps state into `UnresolvedExposure` despite the order filling onchain immediately afterwards.

#### 4. Passive Unwind Live Fallback Places Stacking Market Orders (P1 — Issue #57)
- **Location:** [`crates/arbitrageur-execution/src/passive_unwind.rs:660-706, 763-771`](file:///mnt/Data/Projects/Arbitrageur/crates/arbitrageur-execution/src/passive_unwind.rs#L660-L706)
- **Mechanics:**
  When `cancel_order` is issued and `fetch_order` fails or returns `!cur.status.is_final()`, the code routes to `finish_live`, which submits an immediate market order. If the limit order was still working or already filled, this results in double-fills and position inversion.

#### 5. Coordinator IOC Partial Trim Discard (P2 — Issue #60)
- **Location:** [`crates/arbitrageur-execution/src/coordinator.rs:180-199, 341-363`](file:///mnt/Data/Projects/Arbitrageur/crates/arbitrageur-execution/src/coordinator.rs#L180-L199)
- **Mechanics:**
  `is_trimmable_partial` requires `resp.status == OrderStatus::PartiallyFilled`. When IOC orders partially fill, venues report `OrderStatus::Cancelled` or `OrderStatus::Expired` with `remaining_qty = 0`. This causes the coordinator to reject the partial fill as unsound and execute an emergency unwind of **both legs**, liquidating the matched delta-neutral position at full market cost.

#### 6. Missing Partial Trim Fees in Virtual Ledger Cycle Opening (P2 — Issue #61)
- **Location:** [`frontend/src-tauri/src/execution.rs:951-1031`](file:///mnt/Data/Projects/Arbitrageur/frontend/src-tauri/src/execution.rs#L951-L1031)
- **Mechanics:**
  When `PartialPairKept` is resolved, `execution.rs` ignores `realized_profit` and records `open_cycle` fees using only the initial leg fees, completely omitting the trim order's fee and any slippage incurred during the trim.

---

### C. Venue Integrations & EVM AMM Execution

#### 1. EVM AMM Synchronous Exact-Amount Onchain Approvals (P1 — Issue #59)
- **Location:** [`crates/arbitrageur-exchanges/src/evm_amm.rs:1625-1656, 2281-2285`](file:///mnt/Data/Projects/Arbitrageur/crates/arbitrageur-exchanges/src/evm_amm.rs#L1625-L1656)
- **Mechanics:**
  `place_order` executes onchain ERC-20 approvals with a 30-second block confirmation timeout before sending swap transactions. In atomic two-leg execution with a CEX or Perp DEX, the sibling leg fills in 20ms, leaving the trader unhedged for 15–30 seconds.

#### 2. OKX / PopDEX IOC Price Stripping (P0 — Issue #28)
- **Location:** [`crates/arbitrageur-exchanges/src/okx.rs:992-996`](file:///mnt/Data/Projects/Arbitrageur/crates/arbitrageur-exchanges/src/okx.rs#L992-L996), [`popdex.rs:1036-1037`](file:///mnt/Data/Projects/Arbitrageur/crates/arbitrageur-exchanges/src/popdex.rs#L1036-L1037)
- **Mechanics:**
  `OrderType::ImmediateOrCancel` maps to `("market", None)` on OKX and unpriced market swap with zero slippage on PopDEX, stripping the pre-trade limit price bound.

#### 3. edgeX / Extended First-Element Fallback in `fetch_order` (P0 — Issue #32)
- **Location:** [`crates/arbitrageur-exchanges/src/dex_perps.rs:9091-9103`](file:///mnt/Data/Projects/Arbitrageur/crates/arbitrageur-exchanges/src/dex_perps.rs#L9091-L9103)
- **Mechanics:**
  When searching the order list response for `order_id`, it falls back to `.or_else(|| a.first().cloned())`, which can adopt an unrelated order's fill status as venue truth.

#### 4. Multi-Collateral Valuation Distortions in `normalize_account_usd` (P2)
- **Location:** [`frontend/src-tauri/src/state.rs:2086-2112`](file:///mnt/Data/Projects/Arbitrageur/frontend/src-tauri/src/state.rs#L2086-L2112)
- **Mechanics:**
  Under `fx_adjusted` mode, `normalize_account_usd` scales `acc.equity`, `free_margin`, and `maintenance_margin` by `sf` (the single settlement quote factor). For accounts holding multiple collaterals (e.g. USDC, USDT, BTC) or venues that already report unified USD equity (Hyperliquid, Decibel), this distorts portfolio equity and creates contradictions with the sum of individual balances.

---

### D. User Interface & Error Handling

#### 1. Discarded Execution Results and Silent Failures (P1 — Issue #35)
- **Locations:** `FundingRadar.tsx:949-955`, `App.tsx:695`, `OpportunityRadar.tsx:1316-1321`
- **Mechanics:**
  Frontend components discard returned `SimStepResultDto` and unconditionally display "Executed successfully — position adopted into auto-exit watcher", even when the backend returns `execution_success: false` after a compensated failure.

#### 2. Risk State Masking and Dead Feeds (P1 — Issue #35)
- **Locations:** `PortfolioMonitor.tsx:171, 275-281`, `App.tsx:425`
- **Mechanics:**
  IPC timeout or failure falls back to `riskData = null`, which `PortfolioMonitor` displays as `$0.00` total value and a green "Healthy / All clear" status instead of an explicit error banner.

#### 3. Unhandled Rejection Fatal Overlays on Ordinary Venue Errors (P2 — Issue #49)
- **Locations:** `LiveMatrix.tsx:831`, `App.tsx:657-669`, `index.html:27-30`
- **Mechanics:**
  Ordinary venue rejections (e.g. insufficient margin, post-only cross) reject the IPC promise and trigger the global fatal crash overlay rather than displaying a transient toast or inline error.

---

## 4. GitHub Issues Master Index

A total of 34 GitHub issues document the actionable findings across the repository:

### Critical (P0) — Immediate Capital Loss / System Wedge
| Issue | Component | Title |
|---|---|---|
| [#28](https://github.com/ChillingWombat/Arbitrageur/issues/28) | Adapters | OKX and PopDEX submit price-bounded IOC orders as unpriced market orders |
| [#29](https://github.com/ChillingWombat/Arbitrageur/issues/29) | Execution | Liquidation-trim `CloseBoth` self-deadlocks `execution_lock` |
| [#30](https://github.com/ChillingWombat/Arbitrageur/issues/30) | Reconciler | Reconciliation can classify a partial-fill-terminal leg as `Filled` |
| [#31](https://github.com/ChillingWombat/Arbitrageur/issues/31) | Triangle | Triangle: persist leg intents before submit and never drop working order ids |
| [#32](https://github.com/ChillingWombat/Arbitrageur/issues/32) | Adapters | edgeX/Extended `fetch_order` falls back to the first element of a list response |
| [#52](https://github.com/ChillingWombat/Arbitrageur/issues/52) | Math | Triangular arbitrage evaluation double-counts slippage in net return and profit |
| [#53](https://github.com/ChillingWombat/Arbitrageur/issues/53) | Triangle | Triangular arbitrage realized PnL sums cross-currency cashflows without conversion |
| [#54](https://github.com/ChillingWombat/Arbitrageur/issues/54) | Triangle | Completed triangular cycles record `Imbalanced` signals that block symbols and prompt phantom flattens |
| [#55](https://github.com/ChillingWombat/Arbitrageur/issues/55) | Triangle | Triangular arbitrage dispatches unbounded, unpriced market orders with zero slippage limit |

### High (P1) — Safety Guards, Strategy Logic & Execution Truth
| Issue | Component | Title |
|---|---|---|
| [#33](https://github.com/ChillingWombat/Arbitrageur/issues/33) | Risk | Make the drawdown circuit breaker persistent, background-fed, and day-rolling |
| [#34](https://github.com/ChillingWombat/Arbitrageur/issues/34) | Strategy | Fix inverted spatial exit stop: stop fires on convergence; no adverse-move stop exists |
| [#35](https://github.com/ChillingWombat/Arbitrageur/issues/35) | Frontend | Frontend: never report success/health on failed or unknown backend state |
| [#36](https://github.com/ChillingWombat/Arbitrageur/issues/36) | Exits | External exits: survive vault auto-lock and close the dedup/submit race |
| [#37](https://github.com/ChillingWombat/Arbitrageur/issues/37) | Math | Autotune: include stop losses in candidate expectancy; shadow-run before auto-apply |
| [#38](https://github.com/ChillingWombat/Arbitrageur/issues/38) | Adapters | Remove fabricated prediction orderbooks and phantom CLI parity candidates |
| [#39](https://github.com/ChillingWombat/Arbitrageur/issues/39) | CI / CD | CI: fix red gitleaks step at HEAD, un-break whitespace gate, gitignore state/vault files |
| [#40](https://github.com/ChillingWombat/Arbitrageur/issues/40) | Docs | Docs truth pass: live trading reality, stale numerics, CAPABILITIES update |
| [#41](https://github.com/ChillingWombat/Arbitrageur/issues/41) | Adapters | DexPerp order-truth hardening: QFEX cache, status mapping, id truncation, unit pricing |
| [#56](https://github.com/ChillingWombat/Arbitrageur/issues/56) | Execution | Emergency unwind lacks fill polling, causing false-alarm circuit breaker trips on DEXes |
| [#57](https://github.com/ChillingWombat/Arbitrageur/issues/57) | Execution | Passive unwind live fallback submits market orders while limit orders may still be working |
| [#58](https://github.com/ChillingWombat/Arbitrageur/issues/58) | Strategy | Convergence auto-exit stop logic uses absolute deviation, misclassifying profitable overshoots as stop losses |
| [#59](https://github.com/ChillingWombat/Arbitrageur/issues/59) | EVM AMM | EVM AMM exact-amount approvals inside `place_order` create 30s unhedged exposure in atomic execution |

### Medium (P2) — Durability, Ops, UI Polish & Edge-Case Robustness
| Issue | Component | Title |
|---|---|---|
| [#42](https://github.com/ChillingWombat/Arbitrageur/issues/42) | Desktop | Close Tauri execution-gate fail-open seams |
| [#43](https://github.com/ChillingWombat/Arbitrageur/issues/43) | Storage | Enforce retention caps unconditionally; enable `auto_vacuum` on the state DB |
| [#44](https://github.com/ChillingWombat/Arbitrageur/issues/44) | Storage | Decouple WS telemetry persistence from the execution ledger's SQLite connection |
| [#45](https://github.com/ChillingWombat/Arbitrageur/issues/45) | State | State/durability hardening: env column, transactional multi-updates, zeroize credentials |
| [#46](https://github.com/ChillingWombat/Arbitrageur/issues/46) | Math | Math/strategy corrections: exact OU first-passage time, PSD Kalman, stat-arb cleanup |
| [#47](https://github.com/ChillingWombat/Arbitrageur/issues/47) | Adapters | Adapter hardening: rate limiting on signed endpoints, cancel confirmation, decimal overflows |
| [#48](https://github.com/ChillingWombat/Arbitrageur/issues/48) | Packaging | Ship the Lighter signer library via Tauri bundling; verify `SignCreateOrder` in CI |
| [#49](https://github.com/ChillingWombat/Arbitrageur/issues/49) | Frontend | Frontend destructive-action and capability drift sweep |
| [#50](https://github.com/ChillingWombat/Arbitrageur/issues/50) | Ops | Ops: release pipeline for the custom-protocol desktop binary, headless mode, alerting |
| [#51](https://github.com/ChillingWombat/Arbitrageur/issues/51) | Tests | Test coverage follow-ups for critical uncovered seams |
| [#60](https://github.com/ChillingWombat/Arbitrageur/issues/60) | Coordinator| Coordinator partial-fill trim rejects IOC cancellations, forcing unnecessary full market unwinds |
| [#61](https://github.com/ChillingWombat/Arbitrageur/issues/61) | Ledger | Partial-fill trim fees and price PnL dropped from virtual ledger cycle opening |

---

## 5. Architectural Recommendations & Remediation Roadmap

### Phase 1: Capital Safety & Execution Correctness (Days 1–3)
1. **Fix IOC Wire Encoding (#28):** Ensure OKX sends `ordType: "limit"` with IOC TIF, and PopDEX encodes limit price and slippage bounds.
2. **Fix Re-entrant Deadlock (#29):** Remove `state.execution_lock.lock().await` from `PairTrimDecision::CloseBoth`.
3. **Fix Triangular Arbitrage Math & State (#52, #53, #54, #55):**
   - Correct `evaluate_triangular_cycle` to avoid double-deducting slippage.
   - Calculate cycle realized P&L purely in the quote asset: `USDC_out - USDC_in - fees`.
   - Record completed 3/3 cycles as `SignalStatus::Closed` or `Balanced`, clearing symbol blocks.
   - Price-bound triangular orders using `SmartOrderRouter::build_ioc_order`.
4. **Fix Emergency Unwind & Passive Fallback (#56, #57):**
   - Add bounded `fetch_order` polling to `EmergencyUnwindHandler::unwind_leg`.
   - Fail closed when passive limit cancels are unconfirmed; never blindly submit market orders.

### Phase 2: Strategy Realism & Risk Continuity (Days 4–7)
1. **Fix Inverted Exits (#34, #58):** Correct spatial and convergence stop logic to track signed deviation relative to entry side.
2. **Durable Risk & Breaker Engine (#33):** Feed equity from the background poller, persist daily baselines with UTC rollover, and restore pause/tripped state across restarts.
3. **Decouple EVM AMM Approvals (#59):** Pre-approve router allowances during wallet setup; verify allowances before atomic dispatch.
4. **Frontend Result Truth (#35):** Enforce `assertExecutionOk(result)` and render explicit error banners on dead risk/account feeds.

### Phase 3: Hardening & Performance (Week 2)
1. **Storage Separation (#43, #44):** Move high-frequency orderbook spread telemetry off the primary execution SQLite connection.
2. **Adapter Contracts (#41, #47):** Route signed requests through rate-limiter buckets and enforce strict ID matching on list responses (#32).
3. **Release Pipeline (#48, #50):** Automate Tauri custom-protocol release builds with bundled Lighter native libraries and checksum verification.
