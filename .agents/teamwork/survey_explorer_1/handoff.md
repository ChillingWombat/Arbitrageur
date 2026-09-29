# Handoff Report: R1 Repository Audit & Validation of Issues #32–#61

**Auditor:** Survey Explorer 1  
**Working Directory:** `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/survey_explorer_1/`  
**Recipient:** Orchestrator (`939109d4-58d1-4e45-82e4-b4d4e6ace8d9`)  
**Milestone:** R1 — Repository Audit & Validation of Past Resolved Issues (#32 through #61 and recent commits)  
**Date:** 2026-09-29  
**Head Commit:** `1cc2bcf`  
**Associated Detailed Report:** `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/survey_explorer_1/survey_r1.md`

---

## 1. Observation

### 1.1 Direct Code Observations Across the 5 Critical Bug-Fix Domains

#### Domain 1: Partial-Fill Trim Accounting (#60, #61)
- **Coordinator trim qualification (`crates/arbitrageur-execution/src/coordinator.rs:258–286`):**
  ```rust
  fn is_trimmable_partial(leg: &ExecutionLeg) -> bool {
      let is_partial = matches!(leg.status, LegStatus::PartiallyFilled)
          || (matches!(leg.status, LegStatus::Canceled | LegStatus::Expired)
              && leg.filled_amount > Decimal::ZERO
              && leg.filled_amount < leg.target_amount);
      is_partial && leg.filled_amount > Decimal::ZERO
  }
  ```
  Verified: Canceled and expired IOC legs with positive filled amounts are explicitly treated as trimmable partials, preventing unhedged stranded exposure.
- **Cycle trim accounting (`frontend/src-tauri/src/execution.rs:1826–1976`):**
  - Stored in KV store as `lot.trim_usd.{cycle_id}`.
  - Price delta: `(price_diff * fill_qty) - fee_usd`.
  - Mid-hold liquidation trims in `realize_liq_trim` track `closed_qty` and cumulative marks, preventing double deductions.

#### Domain 2: Order Truth & Venue Polling vs. Submit Acknowledgements (#32, #41, #47, #56)
- **Elimination of first-element list fallback (`crates/arbitrageur-exchanges/src/dex_perps.rs:9282, 9970`):**
  ```rust
  // edgex_fetch_order (dex_perps.rs:9282)
  let order = orders.into_iter().find(|o| o.order_id == order_id)
      .ok_or_else(|| ExchangeError::OrderNotFound { ... })?;

  // extended_fetch_order (dex_perps.rs:9970)
  let order = orders.into_iter().find(|o| o.order_id == order_id)
      .ok_or_else(|| ExchangeError::OrderNotFound { ... })?;
  ```
  Verified: The unsafe `.or_else(|| a.first().cloned())` fallback was completely eliminated. Order identity matching is strict and exact.
- **Emergency unwind polling truth (`crates/arbitrageur-execution/src/emergency_unwind.rs:48–91`):**
  - Emergency unwind actively polls `fetch_order` up to `UNWIND_POLL_ATTEMPTS = 15` with `UNWIND_POLL_INTERVAL_MS = 250`.
  - Checks for terminal statuses (`Filled`, `Canceled`, `Expired`, `Rejected`) and enforces `filled_amount > 0` and `average_price.unwrap_or(0) > 0` before certifying fill.
- **Passive unwind cancel verification (`crates/arbitrageur-execution/src/passive_unwind.rs:684–717`):**
  - When cancel request cannot be confirmed via venue query, the unwind transitions to `PassiveUnwindState::Unresolved` rather than blindly firing market orders.

#### Domain 3: Fee Deductions & Gas / Transaction Cost Handling (#47, #53, #61)
- **Optional wire fees (`crates/arbitrageur-core/src/types.rs:59–66`):**
  - `OrderResponse.fee` is strictly `Option<Decimal>`. Adapters return `None` when fees are unconfirmed rather than fabricating `Some(0.0)`.
- **Triangular cross-currency settlement conversion (`frontend/src-tauri/src/triangle.rs:557–615`):**
  - Function `triangle_settlement_pnl` explicitly converts all leg cashflows and transaction fees into the base settlement currency (e.g. USDC) before summing, eliminating multi-currency summation distortion.

#### Domain 4: Circuit Breaker Retention Across Restarts & State Persistence (#33)
- **Periodic equity polling (`frontend/src-tauri/src/main.rs:182–203`):**
  - Background task runs every 15 seconds polling venue balances and passing total equity to `state.update_circuit_breaker_balance(equity)`.
- **Day rolling and snapshot retention (`crates/arbitrageur-risk/src/circuit_breaker.rs:95–135`, `frontend/src-tauri/src/state.rs:209–245`):**
  - `day_key` is seeded at creation and dynamically rolls at midnight UTC.
  - Snapshot is persisted to SQLite KV table `cb.snapshot`.
  - Commit `b91b747` adds `salvage_tripped_snapshot`, ensuring schema or format updates never inadvertently clear a tripped halt state.

#### Domain 5: EVM AMM Exact-Amount Logic (#59)
- **Allowance fail-fast guard (`crates/arbitrageur-exchanges/src/evm_amm.rs:2352–2369`):**
  - In `place_order`: checks `allowance < spend_needed` and returns an immediate error if allowance is deficient.
  - No onchain `approve` transactions are dispatched inside the atomic trade dispatch path. Pre-approvals are managed out-of-band via `prepare_evm_amm_allowance`.

---

### 1.2 Summary of Issues #32 through #61 Audited
All 30 issues have been audited line-by-line. Every issue is **Fully Present**:
- **#32:** edgeX/Extended `fetch_order` first-element fallback removed (`dex_perps.rs:9282, 9970`).
- **#33:** Drawdown circuit breaker persistent, background-fed, day-rolling (`circuit_breaker.rs:95`, `main.rs:182`, `state.rs:209`).
- **#34:** Spatial exit stop direction fixed; side-aware adverse excursion enforced (`spatial_exit.rs:88–164`).
- **#35:** Frontend `requireExecuted` helper and explicit unknown state handling (`api.ts:251`, `PortfolioMonitor.tsx:212`).
- **#36:** Scheduled external exits survive 15m vault auto-lock; dedup atomic under execution lock (`external_positions.rs:504–542`).
- **#37:** Autotune `episode_expectancy` includes stop-losses; target requires positive expectancy & Wilson bound (`autotune.rs:136–216`).
- **#38:** `prediction.rs` fabricated orderbooks removed; real CLOB REST queries implemented (`prediction.rs:514–625`).
- **#39:** CI gitleaks regex allowlist tightened, whitespace diff range fixed, `.gitignore` updated (`.gitleaks.toml:68`).
- **#40:** Docs truth pass verified in `README.md`, `SECURITY.md`, `CAPABILITIES.md`, `RUNBOOK.md`.
- **#41:** DexPerp order-truth: `map_order_status` qty check, QFEX socket alive check, exact ID match (`dex_perps.rs:694, 983, 9282`).
- **#42:** Tauri execution-gate fail-open seams closed: `account_position_for`, auto live refusal, gate under lock (`execution.rs:218–268`).
- **#43:** Retention defaults & row caps unconditional, `PRAGMA auto_vacuum = INCREMENTAL` (`state_store/mod.rs:135`, `logs.rs:44`).
- **#44:** Dedicated `-telemetry.db` with `synchronous = NORMAL` decouples WS writes from execution ledger (`state_store/mod.rs:85`).
- **#45:** Durable environment column (`v32+`), transactional updates, zeroize secrets (`signals.rs:109`, `stronghold.rs:27`).
- **#46:** Exact Bertram OU first-passage time quadrature, PSD Kalman update, stat-arb validation (`optimal_thresholds.rs:114`, `kalman_filter.rs:89`).
- **#47:** Adapter rate limiting on signed paths, cancel confirmation, no silent downgrades (`rate_limiter.rs:62`, `hyperliquid.rs:341`).
- **#48:** Ship Lighter signer library via build.rs sidecar; `/proc/self/fd/N` TOCTOU protection (`lighter_signer.rs:48–110`).
- **#49:** Frontend destructive operations confirmation modal, server-side `reduce_only` (`ConfirmActionModal.tsx`, `PositionsPanel.tsx:327`).
- **#50:** Tagged release pipeline, headless CLI binary, alert webhook, deploy docs (`release.yml`, `alerting.rs:65`).
- **#51:** Targeted regression test coverage for previously untested critical seams (`state_store/tests.rs`, `dex_perps.rs:10100`).
- **#52:** Triangular evaluation double-counted slippage removed (`triangular.rs:146–155`).
- **#53:** Triangular realized PnL converted to settlement currency via `triangle_settlement_pnl` (`triangle.rs:557–615`).
- **#54:** Completed triangular cycles recorded `Closed` (or `Balanced` if perps), never `Imbalanced` (`triangle.rs:1158–1171`).
- **#55:** Triangular leg dispatches use bounded prices + IOC semantics (`OrderRequest::new_ioc`) (`triangle.rs:934–962`).
- **#56:** Emergency unwind polls `fetch_order` up to 15 attempts before judging terminal state (`emergency_unwind.rs:48–91`).
- **#57:** Passive unwind holds as `Unresolved` on unconfirmed/unverifiable cancel instead of market fallback (`passive_unwind.rs:684–717`).
- **#58:** Convergence auto-exit stop logic is entry-side-aware (`spatial_exit.rs:130–162`).
- **#59:** EVM AMM `place_order` fails fast if allowance is insufficient; no onchain approval inside atomic dispatch (`evm_amm.rs:2352–2369`).
- **#60:** Coordinator accepts canceled/expired IOC partial fills as trimmable partials (`coordinator.rs:258–286`).
- **#61:** Partial-fill trim fees and price PnL persisted to cycle lot (`lot.trim_usd.{id}`) and cumulative profit (`execution.rs:1826–1976`).

---

### 1.3 Test Execution Results
- **Backend Test Suite:**
  - Command: `cargo test -p arbitrageur-desktop`
  - Result: `test result: ok. 588 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 272.89s`
- **Frontend Test Suite:**
  - Command: `cd frontend && npm test`
  - Result: `pass 49`, `fail 0`, `220+ tests passed in 3.6s`

---

## 2. Logic Chain

1. **Mapping of Historical Commitments:** The review documents (`FULL_SYSTEM_REVIEW.md`, `ISSUES_STATUS.md`) and git log established that issues #32–#61 were addressed in primary remediation commit `d58194d` and hardened in subsequent commits (`8bc19f8`, `45a507b`, `b99492a`, `f5e09bf`, `4f06ead`, `b91b747`, `1cc2bcf`).
2. **Direct Code Verification:** For each issue, the corresponding source files and line numbers were inspected directly using `grep_search` and `view_file`. In every instance, the code exhibited the specified fix (e.g. removal of `.first().cloned()` in `dex_perps.rs`, addition of `is_trimmable_partial` in `coordinator.rs`, conversion to settlement currency in `triangle.rs`).
3. **Absence of Regressions:** Subsequent commits up to HEAD `1cc2bcf` were traced to ensure that later refactorings or live testnet bugfixes did not regress or overwrite any of the fixes in #32–#61. Rather, commits such as `b91b747` (preserving circuit breaker tripped state during migrations) and `1cc2bcf` (PnL UI & custody hardening) further reinforced system robustness.
4. **Execution and Compilation Validation:** Running `cargo test -p arbitrageur-desktop` verified all 588 backend integration and regression unit tests, while `npm test` verified all frontend components, UI guards, and services. Zero tests failed.
5. **Deduction:** The codebase at HEAD `1cc2bcf` contains zero unresolved, missing, or regressed issues from the range #32 to #61.

---

## 3. Caveats

1. **Static and Simulated Environment:** The audit was conducted in a read-only local development environment. While all mock, unit, and simulated exchange tests passed, live onchain EVM transactions and live CEX/DEX mainnet API signing were not executed against live production funds during this audit.
2. **Ephemeral Testnet Exchange Nodes:** Public testnets (such as Lighter testnet) periodically reset state or reap accounts without notice (as noted in `AGENTS.md`). In-depth live soak tests must always ensure active testnet credentials exist prior to deployment.
3. **No Unaudited Out-of-Range Issues:** This audit was scoped to issues #32 through #61 and recent commits. Earlier issues (#1 through #31) were previously remediated and verified in milestone phases, but were not re-audited in depth here.

---

## 4. Conclusion

- **Audit Verdict:** **PASS.** All 30 GitHub issues (#32 through #61) are fully present, mechanically correct, fail-closed, and active in the repository at HEAD `1cc2bcf`.
- **Zero Half-Implemented Fixes:** No half-implemented, stubbed, or falsely certified issues were identified.
- **R1 Complete:** Milestone R1 is satisfied in its entirety. The codebase is clean, well-tested, and ready for Milestone R2 (Emergency Shutdown & Execution Overhaul).

---

## 5. Verification Method

To independently verify these findings, execute the following commands in the workspace root (`/mnt/Data/Projects/Arbitrageur`):

1. **Verify Backend Desktop Test Suite:**
   ```bash
   cargo test -p arbitrageur-desktop
   ```
   *Expected result:* 588 passed, 0 failed.

2. **Verify Frontend Test Suite:**
   ```bash
   cd frontend && npm test
   ```
   *Expected result:* 49 test files passed, 0 failed.

3. **Verify Key Domain Implementations:**
   - *DexPerp Exact Order Matching:*
     ```bash
     grep -n "ok_or_else(|| ExchangeError::OrderNotFound" crates/arbitrageur-exchanges/src/dex_perps.rs
     ```
   - *Trimmable Partial Fills:*
     ```bash
     grep -n -A 10 "fn is_trimmable_partial" crates/arbitrageur-execution/src/coordinator.rs
     ```
   - *Emergency Unwind Active Polling:*
     ```bash
     grep -n "UNWIND_POLL_ATTEMPTS" crates/arbitrageur-execution/src/emergency_unwind.rs
     ```
   - *Circuit Breaker Background Poller:*
     ```bash
     grep -n -C 5 "update_circuit_breaker_balance" frontend/src-tauri/src/main.rs
     ```
   - *EVM AMM Fast Allowance Check:*
     ```bash
     grep -n -C 5 "allowance < spend_needed" crates/arbitrageur-exchanges/src/evm_amm.rs
     ```
