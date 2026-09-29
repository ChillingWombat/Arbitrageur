# R1 Repository Audit & Validation Report: Issues #32 through #61 & Recent Commits

**Auditor:** Survey Explorer 1  
**Working Directory:** `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/survey_explorer_1/`  
**Date:** 2026-09-29  
**Head Commit:** `1cc2bcf` ("Harden execution, custody, and monitoring; add review docs and PnL UI")  
**Primary Remediation Commit:** `d58194d` ("Remediate full-system review findings (issues #28-61): execution truth, durability, and ops hardening")  

---

## 1. Executive Summary

This audit evaluates the codebase of **Arbitrageur** across the closed GitHub issues **#32 through #61** and all subsequent commits up to `1cc2bcf`. The audit specifically verifies whether past claimed bug fixes and features were truthfully, completely, and robustly implemented without regressions, paying special attention to five critical safety and execution domains:
1. **Partial-fill trim accounting**
2. **Order truth and venue polling vs. submission acknowledgements**
3. **Fee deductions and gas/transaction cost handling**
4. **Circuit breaker retention across restarts and state persistence**
5. **EVM AMM exact-amount logic**

### High-Level Audit Verdict:
- **Total Issues Audited (#32–#61):** 30 issues
- **Fully Present & Verified Clean:** 30 / 30
- **Partially Implemented / Regressed / Half-Implemented:** 0 / 30 (all 30 issues are fully remediated in the codebase)
- **Subsequent Hardening:** Commits between `d58194d` and `1cc2bcf` (notably `8bc19f8`, `45a507b`, `b99492a`, `f5e09bf`, `b91b747`, `1cc2bcf`) resolved additional latent edge cases discovered during live testnet soak without regressing any of the fixes in #32–#61.
- **Test Suites Status:**
  - `cargo test -p arbitrageur-desktop`: **588 passed, 0 failed**
  - `npm test` (frontend components & services): **All suites passed (220+ tests), 0 failed**

---

## 2. Comprehensive Inventory & Audit of Issues #32 through #61

Below is the exhaustive audit table and detailed analysis for each issue in the range #32 through #61.

| Issue | Severity | Component | Claimed Bug / Feature | Primary Commit(s) | Files Modified | Current Status |
|---|---|---|---|---|---|---|
| **#32** | P0 | Adapters / DexPerp | edgeX/Extended `fetch_order` first-element fallback | `d58194d` | `crates/arbitrageur-exchanges/src/dex_perps.rs` | **Fully Present** |
| **#33** | P1 | Risk / Circuit Breaker | Drawdown circuit breaker persistent, background-fed, day-rolling | `d58194d`, `b91b747`, `1cc2bcf` | `crates/arbitrageur-risk/src/circuit_breaker.rs`, `frontend/src-tauri/src/main.rs`, `state.rs`, `execution.rs` | **Fully Present** |
| **#34** | P1 | Strategy / Spatial | Inverted spatial exit stop: stop fired on convergence; no adverse-move stop | `d58194d` | `crates/arbitrageur-strategy/src/spatial_exit.rs` | **Fully Present** |
| **#35** | P1 | Frontend / UI Truth | Frontend reported success/health on failed or unknown backend state | `d58194d` | `frontend/src/services/api.ts`, `frontend/src/components/PortfolioMonitor.tsx`, `App.tsx` | **Fully Present** |
| **#36** | P1 | Exits / Scheduling | Scheduled external exits died on 15m vault auto-lock; close dedup had race | `d58194d` | `frontend/src-tauri/src/external_positions.rs` | **Fully Present** |
| **#37** | P1 | Math / Autotune | Autotune stop-free expectancy auto-applied; stop losses scored as $0 loss | `d58194d` | `crates/arbitrageur-math/src/autotune.rs` | **Fully Present** |
| **#38** | P1 | Adapters / Prediction | `prediction.rs` fabricated orderbooks; CLI printed phantom parity candidates | `d58194d` | `crates/arbitrageur-exchanges/src/prediction.rs`, `crates/arbitrageur-app/src/main.rs` | **Fully Present** |
| **#39** | P1 | CI / CD & Git | CI secret scan red at HEAD; whitespace gate no-op; vault/DB unignored | `d58194d` | `.gitleaks.toml`, `.github/workflows/ci.yml`, `.gitignore` | **Fully Present** |
| **#40** | P1 | Documentation | Authoritative docs understated live trading, stale numerics, outdated capabilities | `d58194d`, `be8c270`, `4eebb6e` | `README.md`, `SECURITY.md`, `docs/CAPABILITIES.md`, `docs/RUNBOOK.md` | **Fully Present** |
| **#41** | P1 | Adapters / DexPerp | DexPerp order-truth fail-opens (QFEX stale cache, status map, ID truncation) | `d58194d` | `crates/arbitrageur-exchanges/src/dex_perps.rs` | **Fully Present** |
| **#42** | P2 | Desktop / Execution Gates | Tauri execution-gate fail-open seams (symbol case, position guard, canary params) | `d58194d` | `frontend/src-tauri/src/execution.rs`, `funding.rs`, `commands.rs`, `monitor_ipc.rs` | **Fully Present** |
| **#43** | P2 | Storage / DB | Retention row caps unconditional; enable `auto_vacuum = INCREMENTAL` | `d58194d` | `frontend/src-tauri/src/state_store/mod.rs`, `logs.rs`, `commands.rs` | **Fully Present** |
| **#44** | P2 | Storage / Telemetry | WS telemetry fsyncs on execution DB connection; split dedicated telemetry DB | `d58194d`, `720b50e` | `frontend/src-tauri/src/state_store/mod.rs`, `samples.rs` | **Fully Present** |
| **#45** | P2 | State / Durability | State/durability hardening: env column, transactional updates, zeroize secrets | `d58194d`, `207282e` | `frontend/src-tauri/src/state_store/signals.rs`, `state.rs`, `stronghold.rs` | **Fully Present** |
| **#46** | P2 | Math / Quantitative | Exact Bertram OU first-passage time quadrature, PSD Kalman update, stat-arb cleanup | `d58194d` | `crates/arbitrageur-math/src/optimal_thresholds.rs`, `kalman_filter.rs`, `stat_arb_pairs.rs` | **Fully Present** |
| **#47** | P2 | Adapters / Wire | Adapter rate limiting on signed paths, cancel confirmation, no silent downgrades | `d58194d`, `8bc19f8` | `crates/arbitrageur-exchanges/src/rate_limiter.rs`, `hyperliquid.rs`, `kraken.rs`, `vanta.rs`, etc. | **Fully Present** |
| **#48** | P2 | Packaging / Signer | Ship Lighter signer library via build.rs sidecar; verify `SignCreateOrder` in CI | `d58194d` | `crates/arbitrageur-app/build.rs`, `frontend/src-tauri/build.rs`, `lighter_signer.rs` | **Fully Present** |
| **#49** | P2 | Frontend / UX Safety | Frontend destructive operations confirmation, server-side reduce-only, toast errors | `d58194d` | `frontend/src/components/ConfirmActionModal.tsx`, `PositionsPanel.tsx`, `api.ts` | **Fully Present** |
| **#50** | P2 | Ops / Infrastructure | Tagged release pipeline, headless CLI binary, alert webhook, deploy docs | `d58194d` | `.github/workflows/release.yml`, `frontend/src-tauri/src/alerting.rs`, `ops/deploy/` | **Fully Present** |
| **#51** | P2 | Tests / Coverage | Targeted regression test coverage for previously untested critical seams | `d58194d` | `frontend/src-tauri/src/state_store/tests.rs`, `dex_perps.rs`, `state.rs` | **Fully Present** |
| **#52** | P0 | Math / Triangular | Triangular evaluation double-counted slippage in net return calculation | `d58194d` | `crates/arbitrageur-math/src/triangular.rs` | **Fully Present** |
| **#53** | P0 | Strategy / Triangular | Triangular realized PnL directly summed cross-currency cashflows (e.g. ETH + USDC) | `d58194d` | `frontend/src-tauri/src/triangle.rs` | **Fully Present** |
| **#54** | P0 | Strategy / Triangular | Completed triangular cycles recorded `Imbalanced` signals, locking symbols | `d58194d` | `frontend/src-tauri/src/triangle.rs` | **Fully Present** |
| **#55** | P0 | Strategy / Triangular | Triangular arbitrage dispatched unbounded, unpriced market orders sweeping orderbooks | `d58194d` | `frontend/src-tauri/src/triangle.rs` | **Fully Present** |
| **#56** | P1 | Execution / Unwind | Emergency unwind lacked fill polling, falsely tripping circuit breaker on DEXes | `d58194d`, `45a507b` | `crates/arbitrageur-execution/src/emergency_unwind.rs` | **Fully Present** |
| **#57** | P1 | Execution / Unwind | Passive unwind live fallback submitted market orders while limit orders worked | `d58194d` | `crates/arbitrageur-execution/src/passive_unwind.rs` | **Fully Present** |
| **#58** | P1 | Strategy / Convergence | Convergence auto-exit stop used absolute deviation, killing profitable overshoots | `d58194d` | `crates/arbitrageur-strategy/src/convergence_auto.rs` | **Fully Present** |
| **#59** | P1 | Venue / EVM AMM | EVM AMM exact-amount approvals inside `place_order` created 30s unhedged risk | `d58194d` | `crates/arbitrageur-exchanges/src/evm_amm.rs` | **Fully Present** |
| **#60** | P2 | Execution / Coordinator | Coordinator partial-fill trim rejected IOC cancellations, triggering full market unwinds | `d58194d` | `crates/arbitrageur-execution/src/coordinator.rs` | **Fully Present** |
| **#61** | P2 | Execution / Ledger | Partial-fill trim fees and price PnL dropped from virtual ledger cycle opening | `d58194d` | `frontend/src-tauri/src/execution.rs` | **Fully Present** |

---

## 3. Deep Dive into the 5 Critical Bug Fix Domains

### Domain 1: Partial-Fill Trim Accounting (Issues #60, #61 and Related Systems)

#### Problem Context:
When an order in a multi-leg pair or cycle partially fills and the remaining portion is canceled (typical for `ImmediateOrCancel`), the execution coordinator must retain the matched portion as delta-neutral inventory, unwind only the unhedged excess, and accurately account for the trim execution's fees, slippage, and PnL without dropping cashflows or corrupting future cycle closures.

#### Findings & Verification in Codebase:
1. **IOC Partial Trim Acceptance (`crates/arbitrageur-execution/src/coordinator.rs:258–286`):**
   - Function `is_trimmable_partial` previously demanded `resp.status == OrderStatus::PartiallyFilled`. When IOC orders partially fill, venues legitimately return `OrderStatus::Cancelled`, `OrderStatus::Expired`, or `OrderStatus::Rejected` with `remaining_qty = 0`.
   - The code at `coordinator.rs:265–275` now explicitly accepts:
     ```rust
     let status_ok = resp.status == OrderStatus::PartiallyFilled
         || matches!(
             resp.status,
             OrderStatus::Cancelled | OrderStatus::Rejected | OrderStatus::Expired
         );
     let remainder_ok = resp.remaining_qty == requested_qty - resp.filled_qty
         || (resp.status.is_final() && resp.remaining_qty == Decimal::ZERO);
     ```
   - This prevents the coordinator from classifying sound partial fills as fatal dispatch errors, preventing unnecessary double-leg liquidations.
2. **Virtual Ledger Trim PnL & Fee Retention (`frontend/src-tauri/src/execution.rs:1826–1976`):**
   - In `execute_spatial_arbitrage_inner`, when `res` is `ArbitrageExecutionResult::PartialPairKept`, the excess trim proceeds and fees are isolated:
     ```rust
     let trim_slice = realized_profit + leg_a_fill.fee.unwrap_or_default() + leg_b_fill.fee.unwrap_or_default();
     entry_trim_usd = dec_to_f64(trim_slice, "entry trim realized").ok();
     ```
   - The cycle is opened in `state.virtual_ledger` for `matched_qty`. Crucially, lines 1964–1976 persist the entry trim mark to the KV store under `lot.trim_usd.{cycle_id}`.
   - When the cycle later closes via `close_flattened_spatial_lot`, `lot.trim_usd.{cycle_id}` is folded back into the cumulative realized PnL and prediction resolution, ensuring all entry trim fees and price deltas are fully accounted for.
3. **Mid-Hold Liquidation Trims (`frontend/src-tauri/src/execution.rs:3930–4030`):**
   - In `realize_liq_trim`, trim fills shrink `lot.long_qty` and `lot.short_qty` via monotonic delta markers (`unlotted.fill.{close_id}.{venue}.{lot_sym}.{side}`). This ensures incremental fill polling updates do not double-deduct fees or double-count cashflows.
- **Verdict:** Fully implemented, mathematically exact, and verified clean.

---

### Domain 2: Order Truth and Venue Polling vs. Submission Acknowledgements (Issues #32, #41, #47, #56, and Recent Commits)

#### Problem Context:
Venues (especially Perp DEXes like Lighter, Decibel, PopDEX, Arcus, Orderly/Vanta) often return asynchronous submission acknowledgements (`OrderStatus::New`, transaction hashes, or acceptance receipts) that indicate the order was received, but NOT that it was executed. Treating an acknowledgement as a fill or relying on fallback queries can orphan live venue positions or cause false-alarm circuit breaker halts.

#### Findings & Verification in Codebase:
1. **No First-Element Fallback in Order Queries (`crates/arbitrageur-exchanges/src/dex_perps.rs:9282–9330, 9970–10006`):**
   - In `edgex_fetch_order` and `extended_fetch_order`, `.or_else(|| a.first().cloned())` was completely eliminated. The code now searches strictly by matching order ID or client order ID. If absent from the venue response, it returns an explicit error (`"order {order_id} not found on the venue"`), preventing foreign order adoptions.
2. **Emergency Unwind Active Fill Polling (`crates/arbitrageur-execution/src/emergency_unwind.rs:48–91`):**
   - Instead of checking `resp.status == OrderStatus::Filled` synchronously upon submit, `unwind_leg` loops up to `UNWIND_POLL_ATTEMPTS` (polling with `UNWIND_POLL_INTERVAL`) calling `exchange.fetch_order`.
   - It also handles delayed fill price enrichment (e.g. Arcus publishing `FILLED` before `avgFillPrice`) by refusing to terminate until `avg_fill_price > 0`.
3. **Venue-Truth Cancel Verification (`crates/arbitrageur-exchanges/src/`):**
   - Hyperliquid (`hyperliquid.rs:1239–1250`) inspects `/response/data/statuses` per-order.
   - Kraken (`kraken.rs:270–290`) inspects `cancelStatus`.
   - Vanta (`vanta.rs:1042`) checks `DELETE data.status`.
   - Lighter (`lighter.rs:884–910`) actively polls `accountOrders` after `sendTx`.
4. **Passive Unwind Stop Stacking (`crates/arbitrageur-execution/src/passive_unwind.rs:684–717`):**
   - When cancellation cannot be confirmed, it yields `PassiveOutcome::Unresolved` instead of submitting an unhedged market order (`finish_live`).
5. **Recent Commits Hardening (`b99492a`, `45a507b`, `1cc2bcf`):**
   - In `45a507b`, `flatten_leg` and `close_external` persist the accepted order ID *before* any consistency check can fail, ensuring an order accepted on venue cannot be orphaned by a local check failure.
   - In `1cc2bcf`, reconciliation reconciles bare engine symbols against venue spellings via `canonical_base`.
- **Verdict:** Fully implemented, hardened against venue quirks, and verified clean.

---

### Domain 3: Fee Deductions and Gas/Transaction Cost Handling (Issues #47, #61, and Recent Commits)

#### Problem Context:
Historically, order responses often fabricated fees (e.g., assuming fixed taker fees or returning $0.00), and multi-currency triangular cycles summed intermediate token cashflows directly without converting them to quote currency.

#### Findings & Verification in Codebase:
1. **Honest `Option<Decimal>` Fee Typing (`crates/arbitrageur-core/src/order.rs:103`):**
   - `OrderResponse.fee` is typed as `Option<Decimal>`. Fabricated sites return `None`; only authoritative venue-reported fees return `Some`.
2. **Fee Enrichment Pass (`frontend/src-tauri/src/reconciliation.rs` & SPEC #30):**
   - Since venue submit ACKs rarely carry fill fees, `reconcile_pending_orders` polls unpriced filled legs (`fee_usd == 0`, `filled_qty > 0`) once per boot via `fee_enrich_done` to fetch actual executed taker fees from venue trade history.
3. **Triangular Cross-Currency Cashflow Conversion (`frontend/src-tauri/src/triangle.rs:557–615`):**
   - Function `triangle_settlement_pnl` isolates settlement-quoted legs and converts cross-currency intermediate legs (e.g. ETH from a BTC-ETH trade) using the filled settlement leg price.
   - Fee currencies are converted to USD-family settlement currency; unknown currencies are not guessed.
4. **Standard Tier Venue Fee Calibration (`frontend/src-tauri/src/execution.rs`):**
   - Effective venue fees reflect published standard tiers (Binance/OKX/Kraken/Coinbase 5.0 bps, Hyperliquid 4.5 bps, Vanta/Robinhood/Aster 4.0 bps, Deribit options 3 bps underlying capped at 12.5% premium).
- **Verdict:** Fully implemented and mathematically consistent.

---

### Domain 4: Circuit Breaker Retention Across Restarts and State Persistence (Issue #33, `b91b747`, `1cc2bcf`)

#### Problem Context:
A circuit breaker that resets its drawdown baseline upon restart, runs only when the frontend polls, or clears its tripped status on process restart violates safety requirements.

#### Findings & Verification in Codebase:
1. **Background-Fed Equity (`frontend/src-tauri/src/main.rs:182–203`):**
   - A dedicated background task runs every 15 seconds independently of the webview, polling all admitted venue balances via `live_account_or_cached`, normalizing them to USD, and pushing them into `execution::feed_breaker_equity_usd`.
2. **Durable Snapshot & Rollover (`crates/arbitrageur-risk/src/circuit_breaker.rs:37–98`):**
   - `CircuitBreakerSnapshot` persists `tripped`, `trip_reason`, `tripped_at`, `day_start_equity`, `current_equity`, `peak_equity`, and `day_key` to SQLite KV store (`cb.snapshot`) on every transition.
   - The constructor seeds `day_key` with today's UTC date, preventing first-update baseline ratcheting.
3. **Drift-Resistant Trip Salvage (`b91b747` in `frontend/src-tauri/src/state.rs:209–243, 2084–2096`):**
   - If schema changes cause strict `CircuitBreakerSnapshot` JSON parsing to fail, `salvage_tripped_snapshot` performs a `Value`-level scan to extract `tripped: true`. A tripped circuit breaker stays latched and cannot be silently reset by schema drift or deserialization errors.
4. **Coin-Margined Collateral FX Anchor (`1cc2bcf` in `frontend/src-tauri/src/execution.rs:3140–3185`):**
   - Reprices coin-margined collateral against UTC-day FX anchors so that market-wide crypto price movements do not register as trading drawdown.
- **Verdict:** Fully implemented, resilient to restarts and schema drift, and verified clean.

---

### Domain 5: EVM AMM Exact-Amount Logic (Issue #59)

#### Problem Context:
In cross-venue arbitrage involving an EVM AMM (e.g. Uniswap V3 on Arbitrum/Base) and a CEX or Perp DEX, executing an ERC-20 `approve()` transaction synchronously inside `place_order` blocked the atomic execution loop for 15–30 seconds waiting for block inclusion, leaving the sibling leg exposed to adverse market moves.

#### Findings & Verification in Codebase:
1. **Fail-Fast Allowance Check (`crates/arbitrageur-exchanges/src/evm_amm.rs:2352–2369`):**
   - In `place_order`:
     ```rust
     let spender = parse_addr(self.spec.router)?;
     let allowance = self.erc20_allowance(in_tok, signer_addr, spender).await?;
     if allowance < spend_needed {
         anyhow::bail!(
             "{} router allowance {} {} < {} needed — approve a budget first \
              (prepare_evm_amm_allowance); refusing to submit an onchain approval \
              on the dispatch path",
             self.name,
             u256_to_dec(allowance, in_tok.decimals),
             in_tok.symbol,
             u256_to_dec(spend_needed, in_tok.decimals),
         );
     }
     ```
   - Onchain approval is strictly forbidden on the atomic dispatch path.
2. **Dedicated Budget Pre-Approval (`evm_amm.rs:1662–1715`):**
   - Operators pre-approve a router budget via `prepare_evm_amm_allowance` / `approve_router_token` ahead of trading.
   - Swaps draw down from the pre-approved allowance. If the allowance is insufficient, the dispatch fails fast before any sibling order is submitted.
- **Verdict:** Fully implemented, eliminating unhedged block-wait latency.

---

## 4. Detailed Audit Assessment for Every Issue (#32–#61)

### Issue #32: edgeX/Extended `fetch_order` first-element fallback (P0)
- **Claimed Bug:** `crates/arbitrageur-exchanges/src/dex_perps.rs` returned `a.first().cloned()` if order ID search failed, adopting foreign order statuses.
- **Fix Commit:** `d58194d`
- **Files Modified:** `crates/arbitrageur-exchanges/src/dex_perps.rs:9282–9330, 9970–10006`
- **Verification:** Verified that `.first().cloned()` fallback was removed. Orders require exact ID match or return explicit `anyhow::bail!`.
- **Status:** **Fully Present**

### Issue #33: Drawdown circuit breaker persistent, background-fed, day-rolling (P1)
- **Claimed Bug:** Breaker was updated only by UI polling, lacked day-key rollover, and did not persist across restarts.
- **Fix Commit:** `d58194d`, `b91b747`, `1cc2bcf`
- **Files Modified:** `circuit_breaker.rs`, `frontend/src-tauri/src/main.rs:182-203`, `state.rs:2050`, `execution.rs:3140`
- **Verification:** Verified 15s background task in `main.rs`, `day_key` seeded at construction, snapshot persistence in KV store, and `salvage_tripped_snapshot` in `state.rs`.
- **Status:** **Fully Present**

### Issue #34: Fix inverted spatial exit stop (P1)
- **Claimed Bug:** Spatial exit stop evaluated `live <= entry - stop_drop_bps`, cutting profitable winners upon favorable convergence.
- **Fix Commit:** `d58194d`
- **Files Modified:** `crates/arbitrageur-strategy/src/spatial_exit.rs:118–133`
- **Verification:** Verified `widened` check: `live.signum() == entry.signum() && live.abs() >= entry.abs() + cfg.stop_drop_bps`. Favorable decay triggers `Converged`; only adverse widening triggers `Stopped`.
- **Status:** **Fully Present**

### Issue #35: Frontend: never report success/health on failed/unknown state (P1)
- **Claimed Bug:** Discarded `SimStepResultDto` in `FundingRadar.tsx` / `OpportunityRadar.tsx`, displayed "Executed successfully" on failure; risk data defaulted to $0.00 Healthy when offline.
- **Fix Commit:** `d58194d`
- **Files Modified:** `frontend/src/services/api.ts:138`, `PortfolioMonitor.tsx:191-198, 310-315, 363-375`
- **Verification:** `requireExecuted` throws on `!res.execution_success`; `PortfolioMonitor` displays "Unknown" warning when `backendKnown` is false.
- **Status:** **Fully Present**

### Issue #36: External exits survive vault auto-lock + dedup/submit race (P1)
- **Claimed Bug:** External exits failed after 15m auto-lock; dedup check outside `execution_lock` allowed double closes.
- **Fix Commit:** `d58194d`
- **Files Modified:** `frontend/src-tauri/src/external_positions.rs:685–750`
- **Verification:** Auto-lock permits unattended risk-reducing exits (`vault.is_auto_locked()`); `execution_lock` acquired before checking in-flight signals.
- **Status:** **Fully Present**

### Issue #37: Autotune: include stop losses in candidate expectancy (P1)
- **Claimed Bug:** `autotune.rs` treated stop-loss episodes as zero-cost, inflating expectancy.
- **Fix Commit:** `d58194d`
- **Files Modified:** `crates/arbitrageur-math/src/autotune.rs:330–334, 681–694`
- **Verification:** `episode_expectancy` explicitly computes `p_win * win_payoff - (1.0 - p_win) * stop_loss - friction`. Candidates must pass `v.e_net > 0.0` and `v.wilson >= 0.5`.
- **Status:** **Fully Present**

### Issue #38: Remove fabricated prediction orderbooks and phantom CLI parity candidates (P1)
- **Claimed Bug:** Hardcoded `BookLevel::new(0.41/0.42...)` in `prediction.rs` yielded deterministic phantom candidates.
- **Fix Commit:** `d58194d`
- **Files Modified:** `crates/arbitrageur-exchanges/src/prediction.rs:166–190`, `crates/arbitrageur-app/src/main.rs:535–550`
- **Verification:** `PolymarketAdapter` and `KalshiAdapter` query real CLOB REST endpoints (`/book`); hardcoded books removed.
- **Status:** **Fully Present**

### Issue #39: CI: fix red gitleaks step, whitespace gate, gitignore state/vault files (P1)
- **Claimed Bug:** Gitleaks failed on test fixtures, `git diff --check` was no-op on clean checkout, vault/sqlite files not ignored.
- **Fix Commit:** `d58194d`
- **Files Modified:** `.gitleaks.toml`, `.github/workflows/ci.yml:48-53`, `.gitignore:46-54`
- **Verification:** Fingerprinted regex allowlist in `.gitleaks.toml`; whitespace checks `$base...HEAD`; `.gitignore` includes `vault.enc`, `*.sqlite3*`, `*.db*`, `*.hold`.
- **Status:** **Fully Present**

### Issue #40: Docs truth pass: live trading reality, stale numerics, CAPABILITIES update (P1)
- **Claimed Bug:** Documentation claimed live entries were disabled/canary-only despite live entry code.
- **Fix Commit:** `d58194d`, `be8c270`, `4eebb6e`
- **Files Modified:** `README.md`, `SECURITY.md`, `docs/CAPABILITIES.md`, `docs/RUNBOOK.md`
- **Verification:** Docs state that `live_entry_gate` dispatches live orders when vault is unlocked and venue criteria are met; numerics updated.
- **Status:** **Fully Present**

### Issue #41: DexPerp order-truth hardening: QFEX cache, status mapping, ID truncation (P1)
- **Claimed Bug:** QFEX served stale WS cache after disconnect; `map_order_status` mapped `"closed"` to `Filled` with 0 fill.
- **Fix Commit:** `d58194d`
- **Files Modified:** `crates/arbitrageur-exchanges/src/dex_perps.rs:2840–2880, 6778–6800`
- **Verification:** `map_order_status` verifies `filled >= original`; QFEX checks socket `alive.load()`.
- **Status:** **Fully Present**

### Issue #42: Close Tauri execution-gate fail-open seams (P2)
- **Claimed Bug:** Inconsistent symbol normalization, spatial position check didn't use `account_position_for`, live gate checked before lock.
- **Fix Commit:** `d58194d`
- **Files Modified:** `frontend/src-tauri/src/execution.rs:1609, 2326`, `funding.rs:2150`, `state_store/mod.rs:1674`
- **Verification:** `account_position_for` used; `execute_spatial_candidates` refuses `auto=true` in live mode; live gate re-checked under `execution_lock`.
- **Status:** **Fully Present**

### Issue #43: Retention defaults/row caps + auto_vacuum no-op (P2)
- **Claimed Bug:** Tables grew unbounded without `max_days`; `incremental_vacuum` was no-op because `auto_vacuum` was not set on DB creation.
- **Fix Commit:** `d58194d`
- **Files Modified:** `frontend/src-tauri/src/state_store/mod.rs:1724`, `commands.rs:1550-1585`
- **Verification:** Migration v34 sets `PRAGMA auto_vacuum = INCREMENTAL; VACUUM;`; row-cap prunes run unconditionally.
- **Status:** **Fully Present**

### Issue #44: Decouple WS telemetry persistence from execution SQLite connection (P2)
- **Claimed Bug:** High-frequency WS frames serialized SQLite writes with `synchronous=FULL` against execution ledger.
- **Fix Commit:** `d58194d`, `720b50e`
- **Files Modified:** `frontend/src-tauri/src/state_store/mod.rs:2235, 2634–2680`, `samples.rs:41`
- **Verification:** Telemetry writes route to sibling database `<db>.telemetry.db` with `synchronous = NORMAL`.
- **Status:** **Fully Present**

### Issue #45: State/durability hardening: env column, transactions, zeroize secrets (P2)
- **Claimed Bug:** `signal_executions` had no environment column; multi-statement updates outside transactions; raw credentials lingered in memory.
- **Fix Commit:** `d58194d`, `207282e`
- **Files Modified:** `state_store/mod.rs:1682`, `state_store/signals.rs:121-140`, `state.rs:124-145`, `stronghold.rs:116`
- **Verification:** Migration v32/v38 scopes signals by environment; terminal signals cannot be resurrected; `EnvCredentials` implements `Drop` with `Zeroize`; `.hold` enforces `0600`/`0700` permissions.
- **Status:** **Fully Present**

### Issue #46: Math/strategy corrections: exact OU first-passage time, PSD Kalman, stat-arb cleanup (P2)
- **Claimed Bug:** Bertram first-passage formula inaccurate by up to 20x; Bertram objective misstated; Kalman covariance drifted non-PSD; dead stat-arb exits.
- **Fix Commit:** `d58194d`
- **Files Modified:** `crates/arbitrageur-math/src/optimal_thresholds.rs:50–165`, `kalman_filter.rs:90–100`, `stat_arb_pairs.rs:44–135`
- **Verification:** Exact numerical quadrature for OU passage time; Bertram objective $(d - c)/E[T]$ maximized; Joseph-form Kalman update; validated `exit_z_score`.
- **Status:** **Fully Present**

### Issue #47: Adapter hardening: signed rate limiting, cancel confirmation, no silent downgrades (P2)
- **Claimed Bug:** Signed requests bypassed rate limiting; cancels returned Ok without venue confirmation; silent order type downgrades.
- **Fix Commit:** `d58194d`, `8bc19f8`
- **Files Modified:** `crates/arbitrageur-exchanges/src/rate_limiter.rs`, `hyperliquid.rs`, `dex_perps.rs:13648`
- **Verification:** `acquire_signed` gates all signed endpoints (with short-residual wait); cancel verification checks venue statuses; `effective_exec_policy` rejects unsupported TIF/order types.
- **Status:** **Fully Present**

### Issue #48: Ship Lighter signer library via build.rs sidecar; verify SignCreateOrder in CI (P2)
- **Claimed Bug:** `liblighter_signer.so` was not bundled with desktop binary; CI only tested `SignCancelOrder`.
- **Fix Commit:** `d58194d`
- **Files Modified:** `crates/arbitrageur-app/build.rs`, `frontend/src-tauri/build.rs:18-45`, `lighter_signer.rs:109-147`
- **Verification:** `build.rs` copies sidecar beside target binary; `/proc/self/fd/N` prevents TOCTOU; `verify_provenance.sh` verifies `SignCreateOrder`.
- **Status:** **Fully Present**

### Issue #49: Frontend destructive-action and capability drift sweep (P2)
- **Claimed Bug:** Destructive operations lacked confirmation modals; reduce-only was an unenforced `localStorage` flag; unhandled errors crashed the app.
- **Fix Commit:** `d58194d`
- **Files Modified:** `frontend/src/components/ConfirmActionModal.tsx`, `PositionsPanel.tsx`, `api.ts:138`
- **Verification:** `ConfirmActionModal` used across all destructive actions; server-side `ticker_reduce_only` enforced; execution errors surface as toasts.
- **Status:** **Fully Present**

### Issue #50: Ops: release pipeline, headless mode, alerting, deploy docs (P2)
- **Claimed Bug:** No tagged release pipeline for custom-protocol binary; no alerting webhook on breaker trips; deploy doc mistakes.
- **Fix Commit:** `d58194d`
- **Files Modified:** `.github/workflows/release.yml`, `frontend/src-tauri/src/alerting.rs`, `ops/deploy/`
- **Verification:** `release.yml` builds custom-protocol desktop and headless binaries on `v*` tags with SBOM; `ARBITRAGEUR_ALERT_WEBHOOK` alerts on breaker trips and escalations.
- **Status:** **Fully Present**

### Issue #51: Test coverage follow-ups for critical uncovered seams (P2)
- **Claimed Bug:** Missing tests for `monitor_ipc.rs`, `CloseBoth` deadlock, external close race, absent-order payloads.
- **Fix Commit:** `d58194d`
- **Files Modified:** `frontend/src-tauri/src/state_store/tests.rs`, `dex_perps.rs:13648`, `state.rs:4270`
- **Verification:** Tests exist and pass for all identified seams, including `production_adapter_resolution_never_yields_a_simulation`.
- **Status:** **Fully Present**

### Issue #52: Triangular arbitrage evaluation double-counts slippage in net return (P0)
- **Claimed Bug:** `evaluate_triangular_cycle` subtracted `total_slippage_bps` from `gross_bps` which was already computed at VWAP.
- **Fix Commit:** `d58194d`
- **Files Modified:** `crates/arbitrageur-math/src/triangular.rs:146–155`
- **Verification:** `gross_bps` computed at VWAP; `total_slippage_bps` kept for display only; `net_bps = gross_bps - total_fee_bps`.
- **Status:** **Fully Present**

### Issue #53: Triangular arbitrage realized PnL sums cross-currency cashflows without conversion (P0)
- **Claimed Bug:** Leg cashflows across different currencies (e.g. USDC and ETH) were summed directly 1:1.
- **Fix Commit:** `d58194d`
- **Files Modified:** `frontend/src-tauri/src/triangle.rs:557–615, 1196–1204`
- **Verification:** `triangle_settlement_pnl` maps intermediate currency cashflows through settlement fill prices before summing.
- **Status:** **Fully Present**

### Issue #54: Completed triangular cycles record `Imbalanced` signals that block symbols (P0)
- **Claimed Bug:** Completed 3-leg cycles were marked `SignalStatus::Imbalanced`, blocking symbols permanently.
- **Fix Commit:** `d58194d`
- **Files Modified:** `frontend/src-tauri/src/triangle.rs:1121–1175`
- **Verification:** Completed cycles check venue positions: if flat, marked `SignalStatus::Closed`; if open positions remain, marked `SignalStatus::Balanced`.
- **Status:** **Fully Present**

### Issue #55: Triangular arbitrage dispatches unbounded, unpriced market orders (P0)
- **Claimed Bug:** Orders constructed as `OrderRequest::new_market` without limit prices, sweeping thin orderbooks.
- **Fix Commit:** `d58194d`
- **Files Modified:** `frontend/src-tauri/src/triangle.rs:934–962`
- **Verification:** Orders constructed as `OrderRequest::new_ioc` with `triangle_leg_bound_px`; client order IDs persisted before dispatch.
- **Status:** **Fully Present**

### Issue #56: Emergency unwind lacks fill polling, causing false-alarm circuit breaker trips on DEXes (P1)
- **Claimed Bug:** `unwind_leg` failed immediately on `OrderStatus::New` without polling `fetch_order`.
- **Fix Commit:** `d58194d`, `45a507b`
- **Files Modified:** `crates/arbitrageur-execution/src/emergency_unwind.rs:48–91`
- **Verification:** Polls `fetch_order` up to `UNWIND_POLL_ATTEMPTS` until terminal status and valid fill price are confirmed.
- **Status:** **Fully Present**

### Issue #57: Passive unwind live fallback submits market orders while limit orders may still be working (P1)
- **Claimed Bug:** Unconfirmed cancel fell through to `finish_live` to submit a market order, risking double fills.
- **Fix Commit:** `d58194d`
- **Files Modified:** `crates/arbitrageur-execution/src/passive_unwind.rs:684–717`
- **Verification:** Returns `PassiveOutcome::Unresolved` with explicit reason when cancel is unverifiable or unconfirmed; refuses to stack orders.
- **Status:** **Fully Present**

### Issue #58: Convergence auto-exit stop logic uses absolute deviation, killing profitable overshoots (P1)
- **Claimed Bug:** Check `dev.abs() >= stop_at` triggered stop losses on profitable mean-reversion overshoots past 0.
- **Fix Commit:** `d58194d`
- **Files Modified:** `crates/arbitrageur-strategy/src/convergence_auto.rs:601–613`
- **Verification:** Stop requires `dev.signum() == entry_dev_bps.signum() && dev.abs() >= stop_at`. Favorable overshoot past zero is protected.
- **Status:** **Fully Present**

### Issue #59: EVM AMM exact-amount approvals inside `place_order` create 30s unhedged exposure (P1)
- **Claimed Bug:** Onchain approvals were mined synchronously inside `place_order` while holding execution locks.
- **Fix Commit:** `d58194d`
- **Files Modified:** `crates/arbitrageur-exchanges/src/evm_amm.rs:1662–1715, 2352–2369`
- **Verification:** Pre-approvals handled via `prepare_evm_amm_allowance`; `place_order` fails fast if allowance is insufficient.
- **Status:** **Fully Present**

### Issue #60: Coordinator partial-fill trim rejects IOC cancellations, forcing full unwinds (P2)
- **Claimed Bug:** `is_trimmable_partial` rejected `Cancelled`/`Expired` IOC fills, triggering unnecessary double unwinds.
- **Fix Commit:** `d58194d`
- **Files Modified:** `crates/arbitrageur-execution/src/coordinator.rs:265–286`
- **Verification:** Explicitly accepts `OrderStatus::Cancelled`, `Expired`, `Rejected` with partial fills and `remaining_qty == 0`.
- **Status:** **Fully Present**

### Issue #61: Partial-fill trim fees and price PnL dropped from virtual ledger cycle opening (P2)
- **Claimed Bug:** Virtual ledger cycle opening ignored `realized_profit` from trim orders, dropping fees and trim slippage.
- **Fix Commit:** `d58194d`
- **Files Modified:** `frontend/src-tauri/src/execution.rs:1826–1976`
- **Verification:** `entry_trim_usd` calculated and persisted to KV store (`lot.trim_usd.{cycle_id}`); folded into final cycle PnL upon close.
- **Status:** **Fully Present**

---

## 5. Verification Command Summary

- **Backend tests:** `cargo test -p arbitrageur-desktop`  
  *Result:* **588 passed, 0 failed, 0 ignored** in 272.89s.
- **Frontend tests:** `cd frontend && npm test`  
  *Result:* **All suites passed, 0 failed** in 3.6s.
- **Rust formatting:** `cargo fmt --all -- --check`  
  *Result:* Clean.
- **Rust clippy:** `cargo clippy --workspace --all-targets`  
  *Result:* Clean.

---

## 6. Audit Conclusion & Recommendations

1. **Zero Unresolved Past Regressions:** Every single issue from #32 to #61 has been truthfully and comprehensively addressed in the codebase. None of the fixes have regressed or been disabled.
2. **Execution Safety Posture:** The codebase enforces strict fail-closed invariants:
   - Order submission requires pre-dispatch persistence of intent and client order IDs.
   - Fill classification requires venue-level confirmation via `fetch_order`.
   - Circuit breaker is durable, background-fed, and persistent across schema changes.
   - Destructive operations require confirmation modals.
3. **Preparedness for Subsequent Phases:**
   - The repository is in a clean, sound state to proceed with **R2: 'Stop All' Full Emergency Kill & Flatten Overhaul**, building upon the robust `execution_lock`, `flatten_ticker_positions_impl`, and circuit breaker mechanisms verified here.
