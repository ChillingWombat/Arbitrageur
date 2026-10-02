# Arbitrageur System Review — 2026-09-25

- **Date:** 2026-09-25
- **Reviewer:** DeepSeek V4.1 Flash (agentic review session)
- **Review type:** Full-system, read-only audit — build/test verification, source review, cross-subsystem data-flow review, documentation-truth check
- **Repository:** `/mnt/Data/Projects/Arbitrageur`
- **Revision audited:** `9519837` ("Add USD-quote policy modes, Boros fixed-yield hedges, and Stronghold-custodied EVM AMM execution"), working tree clean
- **Status:** Review only. No source files were modified. GitHub issues were filed for the actionable findings (index in §9).

> **Authority note.** `docs/CAPABILITIES.md` remains the authoritative capability matrix. This document is a point-in-time audit and should be superseded, not maintained.

---

## 1. Executive summary

The system is in materially better shape than the 2026-09-20 review found. Verification gates are green across the board (1000 backend tests, clippy/fmt clean, frontend tests/build/audit clean), live-entry gating is consistently wired on all four manual entry paths, persistence-before-submit ordering is correct on spatial/funding/external-close, and the reconciliation supervisor is a real, well-tested component. The engineering discipline in the ledger/reconciliation core is high.

The review nevertheless found **five defects that can lose money or wedge the execution subsystem**, plus a cluster of fail-open reporting and order-truth problems:

1. **OKX and PopDEX turn a price-bounded IOC order into an unbounded market order.** The live two-leg path builds bounded IOC orders; these two adapters discard the price. The pre-trade slippage guard is therefore computed against a price the venue never enforces.
2. **Liquidation-trim `CloseBoth` self-deadlocks `execution_lock`**, wedging every entry, flatten, and residual dispatch until restart.
3. **Reconciliation can classify a partial-fill-terminal leg as `Filled`**, resolving a signal as balanced and skipping residual flatten.
4. **Triangle has no persist-before-submit and drops zero-fill working order ids**, the only entry path that can orphan a live venue order.
5. **edgeX/Extended `fetch_order` falls back to the first element of a list response**, which can report another order's fill as this order's truth.

On top of those: the daily drawdown breaker only advances when the UI polls it (and never rolls over a day), the spatial exit "stop" fires on favorable convergence instead of adverse moves, the frontend reports "Executed"/"Healthy" on failed or unknown backend state, prediction adapters fabricate orderbooks that the CLI prints as real parity candidates, CI's secret scan is red at HEAD, and the README/SECURITY docs still say live strategy entries are disabled while the code dispatches them.

**Verdict:** the fail-closed design intent is real and mostly implemented, but several fail-open seams remain precisely where money and order truth are involved. Fix the five critical items before any further live canary expansion.

## 2. Verification results

All commands run on the audited revision, Linux x86_64, Rust 1.96.0 pinned, Node 24.

| Check | Command | Result |
|---|---|---|
| Backend tests | `cargo test --workspace --locked` | **PASS** — 1000 passed, 0 failed, 8 ignored (450 desktop + 268 exchanges + 133 math + 52 strategy + 49 app + 26 core + 16 risk + 6 lib) |
| Formatting | `cargo fmt --all -- --check` | **PASS** |
| Lints | `cargo clippy --workspace --all-targets --locked` | **PASS** — 0 warnings/errors |
| Frontend tests | `cd frontend && npm test` | **PASS** — node runner + component suites, 0 failures |
| Frontend build | `cd frontend && npm run build` (`tsc && vite build`) | **PASS** (bundle warning: 896 kB main chunk) |
| Frontend audit | `npm audit --omit=dev --audit-level=high` | **PASS** — 0 production vulnerabilities |
| Rust advisories | `cargo audit` | **PASS with 13 warnings** — unmaintained/unsound transitive crates (`lru 0.12.5` unsound `pop()`/`IterMut`); exit 0, no ignores configured |
| Secret scan (CI job reproduction) | `gitleaks git . -c .gitleaks.toml` (v8.24.3, the version inside `gitleaks-action@v3.0.0`) | **FAIL** — 3 findings (`generic-api-key`), including the exact `HEAD-range` scan the workflow performs |
| Vendored signer digest | `sha256sum crates/arbitrageur-exchanges/native/lighter/liblighter_signer.so` | **PASS** — matches the CI pin `8cb2fbc…3753` |

Runtime observations:

- Live state DB `~/.arbitrageur/state.sqlite3` is **1.31 GB** (filesystem perms `0600`, dir `0700` — correct) with `auto_vacuum = 0`, `freelist_count = 0`, `spread_samples ≈ 8.0M` rows, `venue_volume_samples ≈ 2.1M`, `monitor_signal_events ≈ 1.06M` against a documented cap of 50k. See H-11.
- Existing GitHub issues were checked before filing: findings here are new or materially deeper than open issues #3, #8, #21–#27.

## 3. Findings

Severity = impact if unaddressed, weighted by reachability. Verification tags: **[V]** = personally re-read by the reviewer; **[S]** = verified by a scoped sub-review with code/numerical evidence; **[R]** = reported from code citation, not independently re-verified.

### Critical

#### C-1. OKX and PopDEX downgrade price-bounded IOC to unpriced market orders **[V]**

- `crates/arbitrageur-exchanges/src/okx.rs:992-996`:
  ```rust
  let (ord_type, px) = match order.order_type {
      OrderType::Market | OrderType::ImmediateOrCancel => ("market", None),
  ```
- `crates/arbitrageur-exchanges/src/popdex.rs:1036-1037,1086`: IOC maps to `(true, 2u8, U256::ZERO)` (market, price 0) and the slippage field is set to `U256::ZERO`; `let _ = is_market;` (line 1058) proves the price is intentionally discarded.

**Why it matters.** The live two-leg path builds IOC via `SmartOrderRouter::build_ioc_order` (`crates/arbitrageur-execution/src/smart_routing.rs:36`) used at `frontend/src-tauri/src/execution.rs:802-804` and `funding.rs:2217-2224`. The pre-trade slippage/depth guard bounds the *limit price*; OKX/PopDEX then send an unpriced market order, so the second leg can sweep arbitrarily deep into a thin book. This is direct capital-loss exposure on live paths.

**Fix.** Map IOC to the venue's limit+IOC TIF with the caller's bounded price (OKX: `ordType:"limit"` fills IOC or cancels; PopDEX: `Limit` with `tif_byte=2` and a real slippage bound). Extend the adapter wire-conformance tests to assert `px` is present for every non-`Market` order.

#### C-2. Liquidation-trim `CloseBoth` self-deadlocks the global execution lock **[V]**

- `frontend/src-tauri/src/execution.rs:3931-3937` (inside the `PairTrimDecision::CloseBoth` arm) takes `state.execution_lock.lock().await`, then calls `flatten_ticker_positions_impl`.
- `flatten_ticker_positions_impl` (`execution.rs:2762`) delegates to `flatten_ticker_positions_with_type_impl` (`:2771`), which takes the same `tokio::sync::Mutex` at `:2791`.

`tokio::sync::Mutex` is not reentrant. The task awaits its own permit **while holding the lock**, so every subsequent entry/flatten/residual dispatch blocks forever. Reachable whenever `futures_liq_trim_pct` is configured and a sub-2-step position near liquidation resolves to `CloseBoth`. There is no test covering this branch.

**Fix.** Scope the guard to the cooldown insert only, or add `flatten_ticker_positions_locked(...)` that asserts the caller holds the lock. Enforce a lock discipline: never call a public lock-taking function while holding that lock. Add a regression test for the `CloseBoth` path.

#### C-3. Reconciliation trusts terminal status without a quantity cross-check **[V]**

- `frontend/src-tauri/src/reconciliation.rs:98-119`:
  ```rust
  OrderStatus::Cancelled | OrderStatus::Rejected | OrderStatus::Expired => {
      if filled > 0 && remaining > 0 { PartialTerminal }
      else if filled > 0 { Filled }          // partial fill, remaining 0
      else { CancelledEmpty }
  }
  ```
- `derive_signal_status` (`:155-163`) resolves `Balanced`/`Closed` when all legs are `Filled`; no comparison against the persisted `intended_qty`/`original_qty` is made anywhere in the pass.
- `signals.rs:483-508` (`apply_leg_order_state`) stores `MAX(filled, new)` without rejecting `filled > intended`.

Binance/Aster validate their own acks (`binance.rs:342-382`), but the generic contract is fail-open: a venue response that reports a cancelled partial fill with `remaining_qty == 0` (or an adapter that normalizes remaining that way) resolves the signal as fully balanced, clears the ticker block, and **never triggers residual auto-flatten** — the naked leg is then unmanaged. The external-close path already validates `filled == expected` (`external_positions.rs:618-634`); that pattern is missing here.

**Fix.** Pass the persisted intended quantity into `classify_response`; require `filled >= intended - eps` for `Filled`, and `filled <= intended + eps` before applying fills. Mismatches escalate `Unresolved`. Add the partial/over-fill cases to the existing classification test matrix.

#### C-4. Triangle: no persist-before-submit; working zero-fill order ids discarded **[V]**

- `frontend/src-tauri/src/triangle.rs:749-821` submits all three legs first; durable leg rows are written only afterwards (`:854-882`, `record_triangle_leg_fill`).
- `:815-821`: a non-terminal zero-fill response (`filled_qty == 0`) is neither settled nor persisted — `leg_error` is set and the venue `order_id` is dropped.

Every other entry path persists before dispatch (spatial `execution.rs:836-871`, funding `funding.rs:2140-2170`, external close `external_positions.rs:809-840`). A crash or DB error between submit and persist orphans a live venue order with no ledger row, no block, and no reconciliation; a late fill on the dropped `New` order becomes an untracked one-sided position on a re-enterable symbol. This is the one entry path that violates the project's own promotion requirement "durable accepted-order record before submission".

**Fix.** Persist a cycle signal + leg intents with minted `client_order_id`s before the first `place_order`; settle non-terminal responses via poll→cancel; always persist the venue order id even on zero fill.

#### C-5. edgeX/Extended `fetch_order` falls back to an unrelated order's status **[V]**

- `crates/arbitrageur-exchanges/src/dex_perps.rs:9091-9103` (repeat at `:9113-9118`, `:9783-9787`):
  ```rust
  a.iter().find(|e| { /* e.id == order_id */ })
      .cloned()
      .or_else(|| a.first().cloned())     // <-- fallback
  ```
  The request sends `orderIdList` as a scalar string while the pinned HMAC vector uses an array (`orderIdList=111&222`), which is exactly a case where the venue may return a broader list.

**Why it matters.** A failed id match reports the first returned order's `status`/`cumFillSize`/`cumFillValue` as this order's truth. A never-submitted or still-working leg can be read as `Filled`, so the reconciler believes a hedge exists when it does not. This directly violates "never claim fills from acknowledgements / wrong data".

**Fix.** Delete both `.or_else(|| a.first())` fallbacks; require an exact id match and error otherwise. Send `orderIdList` as an array. Add a captured-response test where the requested order is absent from the list (the adapter must error).

### High

#### H-1. Daily drawdown breaker is UI-driven, never rolls over, and its trips are not durable **[V/S]**

- The only non-test `update_equity` call is `frontend/src-tauri/src/execution.rs:1781` inside `get_risk_overview_impl`, invoked by the UI (`frontend/src/App.tsx:425`). A background/minimized webview stops feeding equity, so `max_daily_drawdown_usd/_pct` cannot trip; entry paths only consult `is_trading_allowed()`.
- `crates/arbitrageur-risk/src/circuit_breaker.rs:142-159` compares against `day_start_equity`, written only at construction (`:52`), `reanchor_baseline` (`:87`) and `reset` (`:101`). There is no day key or rollover: a +100% run followed by a −30% day never trips (baseline ratchet).
- Operator killswitch (`execution.rs:1929-1933`), breaker trip, and engine pause (`state.rs:1248`, `:1996`) are all in-memory; a restart resumes trading.

**Fix.** Feed equity from the existing background balance poller; persist `{day_key, day_start_equity, peak, tripped, reason, tripped_at, paused}`; roll over at a configurable UTC boundary; restore trip/pause on startup. Add tests for cross-day rollover and restart-with-trip.

#### H-2. Spatial exit's "stop" fires on favorable convergence; no adverse-move stop exists **[V]**

- `crates/arbitrageur-strategy/src/spatial_exit.rs:105-121`: `Converged` fires at `live <= converge_exit_bps`; `Stopped` fires at `live <= entry - stop_drop_bps`. `entry`/`live` are the same signed cross-venue edge (persisted as `entry_net_bps`, `execution.rs:1074`; live re-priced by `live_spatial_edge`, `:3613-3622`), and mark-to-market P&L ≈ `entry − live`.

So a *falling* live edge is a **gain**, yet it triggers `Stopped` at `entry − 5 bps` (default), cutting winners before convergence; an adverse widening (`live > entry`) triggers nothing until `max_hold_secs` (default 86,400s). The unit test `stopped_on_adverse_excursion` (`:175-195`, entry 8 → live 2) encodes the inversion.

**Fix.** Express exits against inferred position P&L (`entry − live` minus closing cost): adverse stop at `live >= entry + stop_drop_bps`, take-profit separately. Add a P&L-sign table test.

#### H-3. Frontend reports success/health from failed or unknown backend state **[V]**

- **Execution results ignored.** `FundingRadar.tsx:949-955` and `App.tsx:695` discard the returned `SimStepResultDto`; `OpportunityRadar.tsx:1316-1321` prints "Executed successfully — position adopted into auto-exit watcher" unconditionally. The backend deliberately returns `Ok` with `execution_success: false` after a compensated/unwound failure (`funding.rs:2875-2884`, `execution.rs:1285-1295`). Operators are told a live entry succeeded when it failed.
- **Risk state fabricated.** `PortfolioMonitor.tsx:171` `riskData?.total_portfolio_value_usd ?? 0`, `:275-281` falls through to `Healthy`, `:398-400` "All clear — nothing needs you right now"; `App.tsx:425` converts any IPC failure/timeout (including browser mode) into `riskData = null`. A dead risk feed renders a clean bill of health with `$0.00`.
- **Pause is optimistic.** `App.tsx:599-610` flips local state before the command resolves and only logs failures — the UI can show "paused" while the engine is live.
- **Every IPC rejection is swallowed.** `api.ts:116-126` `Promise.race([...]).catch(() => fallback)`; `DESKTOP_REQUIRED_ERROR` is referenced only in `api.ts`/tests and never rendered. A timed-out `listSignalExecutions` silently clears the residual-exposure banner.

**Fix.** `assertExecutionOk(result)` everywhere, gating green state on `execution_success === true`; an explicit unknown/error state for risk data; apply pause state only after success; carry a per-lane error map and render "engine unreachable / desktop required".

#### H-4. Scheduled external exits can never fire after the 15-minute vault auto-lock **[V]**

- `external_positions.rs:669-671` refuses unless `vault.is_unlocked()`; `vault.rs:435` (and `:486,:525,:555`) always sets `unlocked_until = now + 15 min`.
- The scheduler (`run_due_scheduled_exits` → `:1159`) calls this unattended; on error the row is marked `failed`, which is terminal (only `pending` rows are due).

Any exit scheduled more than ~15 minutes ahead silently fails and is never retried — the operator believes a risk-reducing exit is armed. The reconciliation path deliberately tolerates auto-lock (`reconciliation.rs:212-227`), so this refusal is inconsistent with the rest of the system.

**Fix.** Accept `is_auto_locked()` for scheduled/reduce-only closes, or retry with backoff while the row stays `pending`; never terminal-fail on a lock error.

#### H-5. External-close dedup is not atomic with persist → double-submit race **[S]**

`external_positions.rs:726-738` checks for an in-flight close via `list_open_signals` *before* persisting a new signal; `execution_lock` is taken only at `:848`, after persistence. Two concurrent invocations (double-click; scheduled exit racing a manual click) both pass the check, both persist, then serialize and submit two full-size closes. On venues without reduce-only (Coinbase spot), the second close can flip the position.

**Fix.** Acquire `execution_lock` before the dedup read and hold through persist+submit, as `dispatch_residual_flatten` already does (`reconciliation.rs:706-755`).

#### H-6. Autotune validates on stop-free expectancy, then auto-applies **[S]**

`crates/arbitrageur-math/src/autotune.rs:668-683` scores episodes as `e_net = p_win * t - friction`, but a `Stop` (defined at `:643-644`) is scored as a non-win with **zero loss**. With friction 5, σ 10, `t = 15`, stop 30 the gate passes for `p_win > 1/3` while the stop-inclusive expectation `p·10 − (1−p)·35` is negative until `p > 0.78`. `frontend/src-tauri/src/autotune.rs:750-751` and the scheduler (`:928-936`) auto-write READY values into live strategy params.

**Fix.** Score resolved episodes with their real economics (`Win → +t − friction`, `Stop → −stop − friction`), gate on the resulting expectancy (or a lower confidence bound on its sign), and shadow-run before auto-apply. Add the counterexample as a unit test.

#### H-7. `prediction.rs` fabricates orderbooks; the CLI prints phantom parity candidates **[V]**

`crates/arbitrageur-exchanges/src/prediction.rs:61-72` and `:162-173` push hardcoded `BookLevel::new(dec!(0.41/0.42/0.56/0.57), …)` for every requested symbol with no network call. `crates/arbitrageur-app/src/main.rs:518-556` (`run_prediction_arb`) consumes them and prints "🎯 CROSS-VENUE PARITY CANDIDATE FOUND!" deterministically (0.42 + 0.57 = 0.99). This violates AGENTS.md ("silent local simulation … is not [acceptable]") and contradicts `CAPABILITIES.md` ("Research scan when both configured adapters return real books"). The Tauri scan path is stubbed (`commands.rs:1980` returns `[]`), so the CLI is the exposed surface.

**Fix.** Implement the real CLOB REST reads (Polymarket `/book`, Kalshi orderbook) and fail closed, or return `Err`/gate behind `#[cfg(test)]` until they exist. Add a CI guard that no production `fetch_orderbook` returns a book without an HTTP/RPC call.

#### H-8. CI secret scan is red at HEAD; whitespace gate is a no-op; vault/state not ignored **[V]**

- `gitleaks git . -c .gitleaks.toml` reproduces the workflow's own tool/version and finds 3 `generic-api-key` hits (`crates/arbitrageur-exchanges/src/dex_perps.rs:12382,12395,12586`; two are public test vectors, one is the sequential-hex test key whose allowlist regex requires a `0x` prefix). The HEAD-range scan exits 1 — the supply-chain job fails on every push/PR, so real secret leaks would be lost in the noise.
- `.github/workflows/ci.yml:39-40` runs `git diff --check` on a clean checkout, which compares working tree to index and gates nothing (demonstrated: a committed trailing-space file passes).
- `.gitignore` does not exclude `vault.enc`, `*.sqlite3`, `*.sqlite3-wal`, `*.sqlite3-shm`, or `*.db` (`git check-ignore` confirms), so a credential vault or full trading history can be committed from a repo-local HOME.

**Fix.** Fingerprint/allow the three exact strings (or `#gitleaks:allow`), widen the hex-key allowlist, change the whitespace gate to a rev range, and add the file patterns to `.gitignore`.

#### H-9. Authoritative docs understate live trading and contain stale numerics **[V]**

`docs/CAPABILITIES.md:4` declares README + CAPABILITIES jointly authoritative, but:

- `README.md:21` "New live strategy entries are disabled and fail closed … remain blocked", `:149` "live strategy entries are opt-in (Risk Control)", `:154` "still not live-strategy-entry ready"; `SECURITY.md:5,38-48` "new live strategy entries are disabled"/"backend must refuse"; `docs/ARCHITECTURE.md:84`; `docs/EXCHANGES.md:62,238-239`.
  Reality: `live_entry_gate` dispatches live entries on spatial, Spread Radar manual, funding carry/basis and triangle once the vault is unlocked (`commands.rs:36`, `execution.rs:415`, `funding.rs:1707/1709`, `triangle.rs:618`), and the opt-in controls were removed on 2026-09-24 (`SPEC.md:277`).
- `README.md:27` "maximum 5x leverage" (code allows 1–10, `strategy_config.rs:38`) and "only one open pair per symbol" (default cap 3, `state.rs:348`).
- `README.md:21`/`SPEC.md:13,25` "eleven venue adapters" (≥33 submission-capable today: 15 DexPerps `dex_perps.rs:628-644`, 6 live EVM AMMs `evm_amm.rs:774-780`, Boros `boros.rs:1037-1039`, plus the 11 primary adapters).
- `CAPABILITIES.md` status date 2026-09-23 omits the just-shipped USD-quote policy/depeg halt (`stablecoin_fx.rs:4-28,48`) and Stronghold-custodied EVM signing (`stronghold.rs`).
- `docs/VENUE_COVERAGE.md` still marks admitted market-data-only adapters (GMX V2, BULK, Antarctic, etc.) as `excluded-*` while `dex_perps.rs:659-686` admits 26 venue ids.

**Fix.** A single truth-alignment pass: rewrite README §"Current Safety Status" and SECURITY §"Live-order safety boundary" around `live_entry_gate`, fix the numerics, bump CAPABILITIES with the new controls, and recast VENUE_COVERAGE rows as "admitted, market-data-only".

#### H-10. `panel` of fail-open order-truth in long-tail DexPerp adapters **[S/V]**

- QFEX `fetch_order` serves the WS cache after socket death without checking `alive` (`dex_perps.rs:6777-6784`); an order filled/cancelled during a disconnect reports its last pre-disconnect status forever (contrast the correct check at `:6471-6472`).
- `map_order_status` maps `"closed" | "done"` to `Filled` unconditionally (`dex_perps.rs:2848`), even with `filled_qty = 0`.
- EVEDEX mints a fresh random order id per submit (`dex_perps.rs:7573-7576,7932`), so a retry after a lost ack creates a **second real order**; Nado/Aevo/Derive have related idempotency gaps.
- EVEDEX equity adds non-USD collateral at face value (`:8107`); Boros does the same across collateral tokens (`boros.rs:710`) despite carrying `collateral_usd_price`.
- Pacífica/Nado id parsing truncates (`as u32`, `unwrap_or(0)`) or matches `as i64` against `u64` ids (`:3687-3690,7445,7472`); StandX session expiry is hardcoded to 6 days ignoring the JWT `exp` (`:3040`).
- Boros `has_live_credentials` ignores the agent-approval gate its own module docs require (`boros.rs:1045-1047` vs `:36-37`); its batch submit ignores per-call `accountId` (`:437,466-475`) and only fails on the literal `"reverted"`; maturity is not re-checked from the 120s cache (`:517-527`).

**Fix.** Ship order-truth as a per-venue terminal-state contract: exact-match-only `fetch_order`, session-alive gating, deterministic venue ids derived from `client_order_id`, explicit terminal/fill postconditions, and unit-typed balances. Verify each with a recorded venue payload.

### Medium

#### M-1. Tauri execution-gate hardening (fail-open seams) **[V/S]**

- `normalize_symbol` only trims (`state_store/mod.rs:1674-1680`), while `blocking_signals`/`ticker_blocked`/open-count/net-position queries match exact strings: a signal stored as `btc-usdc` does not block an entry under `BTC-USDC`.
- The spatial existing-position guard uses `get_position` (case-insensitive exact match on raw snapshot symbols, `execution.rs:764-769`); adapters returning venue spellings read as flat. Funding/triangle correctly use `account_position_for` and treat errors as undetermined.
- Residual auto-flatten submits straight from the ledger residual with no venue-position check (`reconciliation.rs:849-948`), unlike the manual flatten (`execution.rs:2994-3063`); on spot it is a plain market order that can trade unrelated balances when the ledger overstates the residual.
- Funding checks `live_entry_gate` before a multi-second rescan and before taking `execution_lock` (`funding.rs:1700-1711` vs `:1716`, lock at `:1855`); a vault auto-lock in that window is not re-checked. Triangle does this correctly (gate under the lock).
- Safety guards treat SQLite read errors as "no exposure": `commands.rs:2262-2288` (env switch), `accounts.rs:1135-1155` (credential delete), `:562-582` (disable account) use `if let Ok(...)` and proceed.
- The removed canary loss-floor override is still live on the IPC surface: `execution.rs:337,632-640,3394,3411` accept `canary_max_loss_bps` and can set the profit floor as low as −50 bps; `api.ts:684,697` still declares it.
- `execute_spatial_candidates` (`execution.rs:1349-1461`) has no `auto` refusal (funding/triangle refuse `auto=true` live; the radar bot hard-blocks live internally).
- The monitor manual-entry path sizes from a 60s-stale book (`monitor_ipc.rs:197-211`) while dispatch requires 2s.

**Fix.** One shared `EntryDispatcher`/`pre_trade_gate`: canonicalize symbols at the IPC boundary, require `account_position_for` (Err = refuse), add a venue-position check + clamp to residual flatten, re-run the live gate under the lock, fail closed on DB read errors, delete the canary parameter, and add `auto` refusal to the batch command.

#### M-2. Retention defaults and row caps leave the DB unbounded; `incremental_vacuum` is a no-op **[V]**

- `commands.rs:1322-1344`: every prune except the execution log is nested under `if let Some(days) = max_days`; defaults are `None` (`state.rs:869-870`). `MONITOR_SIGNAL_EVENTS_MAX_ROWS = 50_000` (`state_store/mod.rs:767`) but the live DB holds ~1.06M rows.
- `state_store/logs.rs:109-112` runs `PRAGMA incremental_vacuum`, but no code ever sets `auto_vacuum` (live DB: `0`), so deleted pages never return to the OS.

**Fix.** Run row-cap prunes unconditionally on the hourly pass; ship a non-`None` default (e.g. 14 days); set `auto_vacuum = INCREMENTAL` on new DBs with a one-time `VACUUM` migration.

#### M-3. WS orderbook frames fsync into the execution ledger's single SQLite connection **[S]**

`feed.rs:188-193` calls `record_spread_samples_mode` on every parsed book frame (including every Lighter delta); `state_store/mod.rs:1904-1911` uses `synchronous = FULL`, WAL, one `Mutex<Connection>`; `samples.rs:25-83` runs `BEGIN IMMEDIATE`+INSERT+COMMIT per call. Durable order writes (`record_signal_execution`, leg order ids, fills) queue behind fsync-bound telemetry on the same mutex at hundreds of frames/sec.

**Fix.** Separate the telemetry DB (or an in-memory buffer flushed every 1–5s) from the execution ledger, keep `synchronous=FULL` only on the execution connection, and add a per-(venue,symbol) minimum sample interval.

#### M-4. State/durability hardening **[S]**

- `signal_executions` has no environment column (`state_store/mod.rs:876-912`), so testnet and live PnL/slippage samples blend in `performance.rs:136-185`.
- `apply_leg_fill`/`apply_leg_order_state` update the leg then recompute aggregate fees in a second statement outside a transaction (`signals.rs:300-326,483-508`); `update_signal_status` has no terminal-status guard (`:107-121`), so a late pass can reopen `closed → in_flight`.
- The unresolved escalation rewrites status/note and re-counts `escalated` every 5s (`reconciliation.rs:430-454`), churning `updated_at`.
- Stronghold `.hold` is written with umask permissions, not `0600` (`stronghold.rs:92-100`; upstream `stronghold_engine` uses plain `create_new`), unlike `vault.rs:373-408`.
- `EnvCredentials` holds raw `String` clones with no zeroize-on-drop (`state.rs:70-105`), re-cloned to adapters every 5s (`reconciliation.rs:221`).
- `unlock_vault_impl` can return `Err` after the vault is unlocked (`accounts.rs:41-50`), and unlock has no failed-attempt backoff.
- A rejected `flat-*` child parks `Unresolved`, which `counts_as_open()` blocks from re-dispatch (`reconciliation.rs:176-186,753`), so residual flatten stalls after one rejection despite the 3-attempt cap.

**Fix.** Add `environment`; wrap multi-statement updates in `BEGIN IMMEDIATE`; add terminal-status transition guards; make escalation idempotent; chmod the `.hold` file; `Zeroizing` the env credential fields; make post-unlock steps non-fatal; treat verified zero-fill terminal flatten children as retryable within the attempt bound.

#### M-5. Bedrock math/strategy corrections **[S]**

- **Bertram threshold model passage time is wrong by up to ~20×**: `optimal_thresholds.rs:56-61` gives `E[T] = πd²/(2θ) + d⁴/(12θ)`; numerical integration + 20k-path Monte Carlo give, at 1σ/2σ/4σ distances, ~0.90/1.43/2.05 θ⁻¹ versus the formula's 1.65/7.62/46.5 θ⁻¹. The objective (`:65-73`) is `(d−c)/√E[T]`, not Bertram's `(d−c)/E[T]`, so chosen thresholds and the reported `estimated_sharpe`/`estimated_trades_per_year` (surfaced in `commands.rs:2058-2074`) are misleading.
- **`OuModelParams { r_squared: 0.89 }` is hardcoded** in the calculator (`commands.rs:2049-2056`) — a fabricated goodness-of-fit next to fitted parameters.
- **Stat-arb never exits and thresholds are unvalidated** (`stat_arb_pairs.rs:14-15,33,40-93`): `exit_z_score` is dead; a negative `entry_z_score` produces a constant one-way signal.
- **`MarginMonitor::evaluate_position` returns `Healthy` when liquidation price is unknown** (`margin_monitor.rs:96-98`; `position.rs:43-52` returns `None`) — fail-open risk primitive.
- **Funding "APY" is an unrealistic compounded figure capped at 100,000×** (`funding_carry.rs:75-92`) displayed as `net_apy_pct`.
- **`account_position_for` silently skips `quantity <= 0`** (`core/src/exchange.rs:98-102`), the shared "already positioned?" primitive.
- **Dead but persuasive code**: `ArbitrageEngine` loop would stack carry entries with no position checks (`strategy/engine.rs:67-91`, unused); `spatial_price_arb` statistical baseline state is write-only; `prediction_market_arb` submits resting GTC limit orders (`prediction_market_arb.rs:159-173`) unlike every other IOC entry path.
- **Kalman covariance** is symmetrized but not projected PSD (`kalman_filter.rs:93-94`); `spread_z_score()` divides by innovation variance (documentation hazard).

**Fix.** Implement/deprecate the above; add ground-truth tests (OU MLE recovery, first-passage vs numerical integration, P&L-sign exit table, parametric VaR). Highest-value single fix: exact OU first-passage + an explicit objective.

#### M-6. Adapter wire/rate-limit hygiene **[S]**

- **Rate limiting never gates signed/order paths**: `rate_limiter.rs:55-120` is consumed only by public GETs; signed submits (`binance.rs:423-450`, `okx.rs:241-288`, `kraken.rs:188-227`, `coinbase.rs:194-235`, `aster.rs:1018-1044`, `vanta.rs:530-590`, PopDEX, Decibel) bypass it. A 429 on submit writes a backoff that blocks only market-data reads — the inverse of fail-safe.
- **Cancel returns `Ok` from submit acks without venue confirmation**: Hyperliquid ignores per-order `statuses` errors (`hyperliquid.rs:1189-1196`), Kraken ignores `cancelStatus: notFound` (`:1257-1259`), Vanta checks only `success:true` (`:1042,1051`), Lighter treats `sendTx` acceptance as cancel (`:884-892`).
- **Binance/Aster reject their own accepted orders when the request quantity is off the lot grid**: the wire qty is floored (`binance.rs:912`) but the ack is parsed against the unfloored `order.quantity` and hard-fails (`:345`); same in `aster.rs:531-537,988`. Latent because the desktop pre-snaps, but `place_order` is documented to tolerate off-grid input and a caller-level retry could duplicate the order.
- **Silent semantic downgrades**: post-only dropped on OKX/Binance/Aster/Vanta; FOK downgraded to resting limit on Kraken (`:1120-1131`) and IOC on Decibel (`:1479-1482`); OKX hardcodes isolated margin on every order and leverage change (`okx.rs:1006-1012,1141-1145`); Coinbase market buys size `funds = qty * ask` with no fee/slippage buffer (`coinbase.rs:663`), systematically under-filling the hedge.
- **Fees are fabricated** from hardcoded taker tiers (Binance 5 bps `:404-405`, Aster 3.5 bps `:585-595`; HL/Lighter report 0), corrupting PnL attribution and calibration.
- **Env/chain inferred by URL substring**: `hyperliquid.rs:1115`, `lighter.rs:1098-1107`, `decibel.rs:446-464`, `ws.rs:381-384` — a proxy/custom domain selects the wrong source/chain/package; make env an explicit constructor parameter.
- **Cancellation-proof/cancel idempotency**: Lighter order identity is a 48-bit hash with first-match lookup (`lighter.rs:1214-1226,951-965`), collision-prone at ~2^24 orders.
- **Secrets transit non-zeroizing copies** (`hyperliquid.rs:154-166,1148-1155`, `lighter_signer.rs:301-314`, header strings in aster/okx/coinbase/kraken); pre-trade `dec*(price,18)` scaling helpers can **panic** on overflow because `rust_decimal` Mul panics (`dex_perps.rs:2792-2803`; `10u64.pow(scale)` also panics for scale ≥ 20).

**Fix.** Route every signed request through a per-venue bucket + backoff gate; make cancel confirmation part of the trait contract; bind a single snapped wire quantity for request and ack; add capability flags (`supports_post_only`, `supports_fok`, `margin_mode`) and fail closed instead of downgrading; use venue fill fee data or mark unknown; audit every `Decimal` product with `checked_*`; introduce `SecretString` end-to-end.

#### M-7. Lighter signer: production packaging missing; create-order path unverified **[S]**

- `lighter_signer.rs:132-140` looks next to the executable and then at `CARGO_MANIFEST_DIR/native/...`; there is no `build.rs`, and `frontend/src-tauri/tauri.conf.json` has `bundle.active=false` with no `resources`/`externalBin`. On any machine other than the source checkout, Lighter live signing fails closed — one of the four core venues silently drops out of distributed builds.
- The CI provenance probe (`native/lighter/verify/dump_cancel.c:45-59`) only exercises `SignCancelOrder`; `SignCreateOrder` (19 args, the hot path called by `lighter_signer.rs:329-350`) is never signature-verified against the pinned source, so a tampered create path with a valid cancel path passes CI.

**Fix.** Ship the `.so` via bundle resources/externalBin or the deploy recipe, and add a create-order probe with fixed params + pinned hash/Schnorr verification.

#### M-8. Frontend destructive-action and capability-drift sweep **[V/S]**

- Spot and Prediction tabs ship full execute surfaces over permanent backend stubs: `commands.rs:1948-1965` spot always `Err`, `execution.rs:1579-1601` prediction always `Err`, scans return `[]`; yet `SpotArbitrageBot.tsx:121-131` prints "Flash fill confirmed", `PredictionHub.tsx:159-186` runs an auto-execution loop, and `App.tsx:701-707` hardcodes size 100.0.
- Emergency flatten has no confirmation and swallows failures (`PositionsPanel.tsx:165-175,524-561`); Halt Trading/Monitor stop buttons fire on one click (`PortfolioMonitor.tsx:651-658`, `BotLifecycleControls.tsx:90-100`) while the header killswitch confirms — inconsistent safety UX.
- An ordinary venue rejection on ranked-opportunity Execute becomes an unhandled rejection → the global fatal overlay (`LiveMatrix.tsx:831`, `App.tsx:657-669`, `index.html:27-30`).
- Reduce-only is a frontend `localStorage` flag enforced in exactly one auto-exec branch (`positionsQuant.ts:46-77`, `LiveMatrix.tsx:861`), not in manual entries or the backend, and not env-scoped.
- Strategy-config save fills missing fields with defaults when the pre-read fails (`TickerConfigModal.tsx:1461,1482,1500-1510`), silently resetting unrelated live parameters.
- Money formatting masks missing values as `$0.00` and can render negatives with a `+` prefix (`types/tickers.ts:51-56`, `StrategyPerformance.tsx:221-226`, `PortfolioMonitor.tsx:50-53`).
- `TickerLogMonitor` derives its own net-spread/hurdle with different fee conventions and synthesizes log rows persisted to `localStorage` (`TickerLogMonitor.tsx:152-169,207-295`).
- `LiveMatrix.tsx:826` sends `targetProfitBps` as `baseline_mean_spread_bps`, so recorded deviation/edge attribution is wrong whenever target ≠ baseline.

**Fix.** Backend capability descriptors gating Execute/auto loops; one `ConfirmAction` pattern; catch command rejections as toasts (reserve the fatal overlay for render crashes); server-side reduce-only per env+strategy+symbol; PATCH-only config saves; `—` for unknown money; derive log rows from backend telemetry; pass the real baseline.

#### M-9. Ops/release gaps for a "live mode is the product" system **[S]**

- No release pipeline: CI builds only the headless `arbitrageur` binary (`ci.yml:122-141`); the actual trading binary (`--features custom-protocol`) is built ad hoc, never archived/signed/checksummed; no tags exist.
- No headless trading mode: `arbitrageur serve` is a 503 placeholder (`arbitrageur-app/src/main.rs:651-700`); all trading lives in the Tauri desktop process, so there is no supervised/VPS deployment path.
- No alerting path: no webhook/email/PagerDuty code; systemd units have no `OnFailure=`; `unresolved`/`imbalanced`/breaker events are UI-only (`reconciliation.rs:766-771`, `RUNBOOK.md:117`).
- Deployment docs reference `/opt/arbitrageur` paths the install never creates (`ops/deploy/README.md:94`, `arbitrageur@.service:3`) and require `sqlite3` without documenting it.
- The logrotate sample sends `SIGUSR1` with no handler → default disposition kills the daemon (`arbitrageur-logrotate.conf.sample:22`); `RUST_LOG` is documented but ignored (`arbitrageur-app/src/main.rs:84-88` hardcodes INFO).
- Backups have no restore verification and per-instance naming collides when `DB_PATH` is used without `PORT` (`backup-sqlite.sh:42-53`).

**Fix.** Tagged release job (frontend build → `custom-protocol` binary → checksums/SBOM → artifacts), an alert-out hook on escalation transitions, and corrected deploy docs.

#### M-10. Test-coverage map: critical seams without tests **[V/S]**

Verified present and substantive: coordinator two-leg matrix (24 tests in `arbitrageur-execution/src/lib.rs` — identity mismatches, partial trims, late fills, unwind failures), state-store invariants (78 tests), reconciliation (24 tests), math/strategy suites.

Missing where the review found bugs:

- `monitor_ipc.rs`: **0 tests** (`execute_monitor_opportunity_impl` untested).
- Liquidation-trim `CloseBoth` re-entrant lock path (C-2) — no test.
- External close: 5 tests but none for the dedup/lock race (H-5) or the auto-lock scheduled path (H-4).
- Reconciliation partial/over-fill vs intended quantity (C-3) — the matrix tests only full-quantity cases.
- Adapter wire-conformance: no test feeds every `(OrderType, TIF, post_only, reduce_only)` combination through a fake transport (C-1, M-6).
- Absent-order `fetch_order` payloads for list endpoints (C-5).
- A test asserting `SimulatedExchange` is unreachable from production registries (it is `cfg(test)` in Tauri, but public in the crate and used by the release CLI — `arbitrageur-exchanges/src/lib.rs:18,38`, `arbitrageur-app/src/main.rs:7,209`).

## 4. Documentation vs code contradictions (verified)

| # | Document claim | Reference | Code reality |
|---|---|---|---|
| 1 | "New live strategy entries are disabled and fail closed" | `README.md:21` | `live_entry_gate` dispatches live entries (`commands.rs:36`, `execution.rs:415`, `funding.rs:1707`, `triangle.rs:618`) |
| 2 | "live strategy entries are opt-in (Risk Control)" | `README.md:149` | opt-in controls removed 2026-09-24 (`SPEC.md:277`) |
| 3 | "backend must refuse new live strategy entries" | `SECURITY.md:5,38-48` | same gate as #1 |
| 4 | "maximum 5x leverage" | `README.md:27` | 1–10 allowed (`strategy_config.rs:38`), default 5, ceiling 10 |
| 5 | "only one open pair per symbol" | `README.md:27` | default cap 3 (`state.rs:348`), enforced `execution.rs:357-397` |
| 6 | "Eleven venue adapters can sign, submit, or cancel" | `README.md:21`, `SPEC.md:13,25,74` | ≥33 submission-capable (15 DexPerp + 6 EVM AMM + Boros + 11 primary) |
| 7 | Non-spatial live dispatch still blocked (`LIVE_ENTRY_SPATIAL_ONLY`) | `SPEC.md:417` | constant no longer exists; funding/triangle dispatch |
| 8 | Auto-resolution "OPEN" | `PROJECT_REVIEW_CROSSCHECK_2026-09-20.md:373` | implemented `reconciliation.rs:695-830`, commits `805ea1e`/`e527b93` |
| 9 | "Rolling Spread Baseline… 250 BPS spike filter" | `RUNBOOK.md:40` | absolute expected-edge ceiling (`execution.rs:670-679`), not a rolling baseline |
| 10 | GMX V2/BULK/Antarctic/etc. "excluded" | `VENUE_COVERAGE.md:65-104` | admitted market-data-only (`dex_perps.rs:659-686`, `CAPABILITIES.md:71`) |
| 11 | Capability matrix current as of 2026-09-23 | `CAPABILITIES.md:3` | omits USD-quote policy/depeg halt and Stronghold custody shipped 2026-09-24 |
| 12 | "Simulated exchanges exist only under `#[cfg(test)]`" | `AGENTS.md:21` | `pub mod simulated` in the library; release CLI constructs it (`main.rs:209`) |

## 5. Potential upgrades (ranked by value)

1. **Single `EntryDispatcher` + one lock discipline.** Every entry path becomes a plan builder passing through: canonicalize → lock → live gate → blocking/position/grid/delta/margin/notional checks → persist intents → coordinator → persist results. Eliminates the C-2 deadlock class and H-5/M-1 gate-placement bugs structurally.
2. **Per-symbol locks instead of one global `execution_lock`.** A `DashMap<Symbol, Arc<Mutex>>` plus a family lock removes head-of-line blocking (one 15s venue timeout currently blocks all symbols) while making re-entrancy impossible.
3. **Venue capability flags with contract tests.** `supports_post_only`, `supports_fok`, `supports_ioc_limit`, `margin_mode`, `cancel_is_confirmable`, `supports_reduce_only`; adapters refuse rather than silently downgrade; a matrix test proves declarations match code.
4. **Wire-conformance harness per adapter.** Fake transport; feed every order-type/TIF/flag combination; assert the exact body (catches C-1, M-6, and the ack-quantity class).
5. **Order-truth contract:** exact-match-only `fetch_order`, session-alive gating, monotonically-increasing client order indexes per venue where the venue id is derived from them, recorded-payload regression tests (C-5, H-10).
6. **Backend risk state machine + alert-out.** Persist day-key equity, breaker/killswitch/pause; feed equity from the background poller; emit a webhook/journal alert on escalation. Turns H-1 from a display into a control.
7. **Split durability domains.** Execution ledger kept `synchronous=FULL`; telemetry batched in a separate connection/DB; retention caps enforced unconditionally with `auto_vacuum=INCREMENTAL` (M-2, M-3).
8. **Speech-act UI layer.** `AsyncState<T>` with explicit error/unknown states, `assertExecutionOk(result)`, one `ConfirmActionButton`, backend capability descriptors (H-3, M-8).
9. **A "no fabrication" CI gate.** Walk every adapter id, assert `fetch_orderbook` performs an HTTP/RPC call or returns `Err`; grep-deny hardcoded `BookLevel::new(dec!(...))` outside `#[cfg(test)]`. Would have caught H-7.
10. **Backtest/exit realism.** P&L-signed exit policy driven by a table test; stop-inclusive expectancy in autotune; exact OU first-passage with numerical-integration tests (H-2, H-6, M-5).

## 6. Verified-correct / non-findings (to prevent re-litigation)

- Coordinator two-leg tests exist and are substantive (24 in `crates/arbitrageur-execution/src/lib.rs`) — a first-pass claim of "zero tests" was wrong.
- Triangle **does** check free margin and `MarginMonitor` before dispatch (`triangle.rs:700-718`) — a first-pass claim of "no margin check" was wrong. It still lacks a portfolio `DeltaGuard` call, shared with funding.
- Binance/Aster validate their own ack quantities internally against the request (`binance.rs:342-382`); the C-1/M-6 issue is that the parser uses the unfloored quantity, not that validation is absent everywhere.
- Vault crypto/hygiene is sound: AES-256-GCM with fresh 12-byte nonces per write, AAD, Argon2id (64 MiB, t=3, p=1), `Zeroizing` key material, `ExchangeCredentials` zeroize-on-drop, `0600`/`0700` perms enforced and tested, field-scoped reveal with the Stronghold sentinel refused, auto-lock vs manual lock semantics consistent with reconciliation.
- Persist-before-submit ordering is correct in spatial, funding, external close; reconciliation persists venue order ids before fills; `sim-%` ids cannot be minted in production (Tauri registers simulated venues only under `#[cfg(test)]`, `state.rs:1265`).
- Cancel-attempt caps and residual-flatten attempt bounds are kv-persisted across restarts; `claim_scheduled_exit` is an atomic one-way transition.
- `open_external_url` host allowlisting resists userinfo/suffix tricks; no credential material is logged; no production `unwrap`/`expect`/`panic!` reachable from UI input in `commands.rs` production regions.
- CI gates that do work: fmt, clippy `-D warnings`, workspace tests, frontend tests/build/audit, signer digest, provenance script, systemd unit validation (both unit files exist), cargo-audit (0 vulnerabilities, warnings only).
- Frontend mock fixtures are not in the production bundle (`mockMain.tsx` unshipped; no `FIXTURES` strings in `dist/`).

## 7. Method & coverage

- **Subsystems reviewed:** `arbitrageur-core`, `arbitrageur-math`, `arbitrageur-risk`, `arbitrageur-strategy`, `arbitrageur-execution`, `arbitrageur-exchanges` (all adapters incl. vendored Lighter signer), `arbitrageur-app`, `frontend/src-tauri` (commands, execution, funding, triangle, auto_exec, reconciliation, state/state_store, vault/stronghold, accounts, external positions, monitor/feed, stablecoin FX, performance), `frontend/src` (all components/services/hooks/types/tests), `.github`, `ops/deploy`, and the doc set (`README`, `SPEC`, `CAPABILITIES`, `RUNBOOK`, `EXCHANGES`, `PARAMETER_GUIDE`, `VENUE_COVERAGE`, `ARCHITECTURE`, `SECURITY`, prior reviews).
- **Techniques:** full-suite test/clippy/fmt/build/audit runs; CI-job reproduction (gitleaks v8.24.3); numerical verification of OU first-passage time (quadrature + 20k-path Monte Carlo) and autotune expectancy arithmetic; keccak typehash verification for one pinned EIP-712 constant; read-only inspection of the live state DB; grep sweeps for `unwrap`/`expect`/`panic!`/`unsafe`/`PRAGMA`/`set_permissions`/`invoke(`/`?? 0`/`execution_success`; cross-file call-site tracing for every risk guard and entry gate.
- **Not done:** no live or testnet venue calls; no credentialed venue evidence; no PopDEX mainnet/contract dry-run; no Tauri GUI interactive session; no disassembly of the vendored signer. Claims that require venue responses are marked accordingly.

## 8. Corrections applied to first-pass review assertions

Because this review aggregated parallel sub-reviews, three first-pass claims were corrected during cross-check and are excluded from the findings: coordinator test coverage (exists), triangle margin check (exists), Binance ack validation (exists, with the grid caveat retained). The corrected claims are recorded in §6 so future reviews do not reintroduce them.

## 9. Findings filed as GitHub issues

Issues filed from this review (labels: `bug`/`enhancement`/`documentation` + `ready-for-agent`):

| Issue | Severity | Finding |
|---|---|---|
| [#28](https://github.com/ChillingWombat/Arbitrageur/issues/28) | P0 | OKX/PopDEX downgrade bounded IOC to unpriced market orders (C-1) |
| [#29](https://github.com/ChillingWombat/Arbitrageur/issues/29) | P0 | Liquidation-trim `CloseBoth` self-deadlocks `execution_lock` (C-2) |
| [#30](https://github.com/ChillingWombat/Arbitrageur/issues/30) | P0 | Reconciliation classifies partial terminal fills as `Filled` (C-3) |
| [#31](https://github.com/ChillingWombat/Arbitrageur/issues/31) | P0 | Triangle missing persist-before-submit; drops working order ids (C-4) |
| [#32](https://github.com/ChillingWombat/Arbitrageur/issues/32) | P0 | edgeX/Extended `fetch_order` first-element fallback (C-5) |
| [#33](https://github.com/ChillingWombat/Arbitrageur/issues/33) | P1 | Drawdown breaker UI-driven, never rolls over, trips not persisted (H-1) |
| [#34](https://github.com/ChillingWombat/Arbitrageur/issues/34) | P1 | Spatial exit stop fires on convergence; no adverse stop (H-2) |
| [#35](https://github.com/ChillingWombat/Arbitrageur/issues/35) | P1 | Frontend reports success/health on failed/unknown state (H-3) |
| [#36](https://github.com/ChillingWombat/Arbitrageur/issues/36) | P1 | Scheduled exits die on vault auto-lock + close dedup race (H-4/H-5) |
| [#37](https://github.com/ChillingWombat/Arbitrageur/issues/37) | P1 | Autotune stop-free expectancy auto-applied (H-6) |
| [#38](https://github.com/ChillingWombat/Arbitrageur/issues/38) | P1 | prediction.rs fabricated orderbooks / phantom CLI signals (H-7) |
| [#39](https://github.com/ChillingWombat/Arbitrageur/issues/39) | P1 | CI secret scan red; whitespace gate no-op; vault/DB unignored (H-8) |
| [#40](https://github.com/ChillingWombat/Arbitrageur/issues/40) | P1 | Docs understate live trading; stale numerics; CAPABILITIES missing features (H-9) |
| [#41](https://github.com/ChillingWombat/Arbitrageur/issues/41) | P1 | DexPerp order-truth fail-opens (QFEX cache, status map, ids, units) (H-10/M-5c) |
| [#42](https://github.com/ChillingWombat/Arbitrageur/issues/42) | P2 | Tauri execution-gate fail-open seams (M-1) |
| [#43](https://github.com/ChillingWombat/Arbitrageur/issues/43) | P2 | Retention defaults/row caps + `auto_vacuum` no-op (M-2) |
| [#44](https://github.com/ChillingWombat/Arbitrageur/issues/44) | P2 | WS telemetry fsyncs on the execution DB connection (M-3) |
| [#45](https://github.com/ChillingWombat/Arbitrageur/issues/45) | P2 | State/durability hardening (env column, transactions, perms, zeroize, flatten retry) (M-4) |
| [#46](https://github.com/ChillingWombat/Arbitrageur/issues/46) | P2 | Bertram/OU math correctness + dead-strategy cleanup (M-5) |
| [#47](https://github.com/ChillingWombat/Arbitrageur/issues/47) | P2 | Adapter wire/rate-limit/cancel-confirmation hardening (M-6) |
| [#48](https://github.com/ChillingWombat/Arbitrageur/issues/48) | P2 | Lighter signer packaging + create-order provenance (M-7) |
| [#49](https://github.com/ChillingWombat/Arbitrageur/issues/49) | P2 | Frontend destructive-action/capability drift sweep (M-8) |
| [#50](https://github.com/ChillingWombat/Arbitrageur/issues/50) | P2 | Ops: release pipeline, headless mode, alerting, deploy docs (M-9) |
| [#51](https://github.com/ChillingWombat/Arbitrageur/issues/51) | P2 | Test coverage follow-ups (M-10) |

Relations to existing issues (avoid duplication): venue-evidence gaps remain tracked by #23; params/presets by #8; adaptive analytics by #3; hardening follow-ups by #27. This review's items are distinct defects or deeper specifics.

## 10. Suggested remediation order

1. **Stop the bleeding (P0, days):** C-1 IOC price bound, C-2 deadlock, C-3 quantity cross-check, C-4 triangle persist order, C-5 fetch_order fallback.
2. **Truth and safety reporting (P1):** H-3 frontend result handling, H-1 risk state/persistence, H-8 CI secret gate, H-9 docs pass, H-4/H-5 close races, H-2 exit policy, H-7 prediction fabrication, H-6 autotune.
3. **Hardening (P2):** M-1 gate seams, M-3/M-2 DB health, M-6/M-7 adapter contracts and packaging, M-4 durability, M-5/M-8/M-9/M-10.
4. **Then** resume live canary expansion only after P0s and the H-1/H-3/H-4/H-5 exit-path fixes are in, with venue-evidence gates unchanged.

## 11. Caveats

- Severity for latent-but-unreachable defects (e.g. post-only/FOK downgrades, canary parameter) is lower than for reachable live paths; each issue notes its preconditions.
- The vendored Lighter `.so` was hash-checked and ABI-compared, not disassembled.
- The live state DB observations are from the operator's machine at review time and are cited as evidence of defaults, not as code facts.
- Sub-review assertions were cross-checked where they drove P0/P1 issues; the three corrections are recorded in §6/§8.
