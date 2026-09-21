# Arbitrageur Independent Review and DeepSeek Cross-Check

- **Review date:** 2026-09-20
- **Repository state reviewed:** `main` at `ea45503`, plus the pre-existing working-tree changes listed in §3
- **Independent reviewer:** Devin
- **Compared review:** `docs/PROJECT_REVIEW_2026-09-20.md` (the owner identifies this as the DeepSeek review)
- **Scope:** Specifications, GitHub issues, handoff history, documentation, Rust workspace, Tauri command layer, React frontend, deployment assets, tests, security posture, and repository hygiene

---

## 1. Executive Verdict

The repository contains a substantial, well-tested paper-trading and research system, but its current documents and runtime capability model do not describe one coherent product.

The most important independent conclusion is stricter than the DeepSeek review:

> An adapter that can sign and submit an order is not necessarily safe for strategy-level live execution. The present live spatial path can submit asynchronous orders whose immediate response reports zero fills, classify the attempt as cleanly failed, and stop tracking it even though the venue may fill it later.

Accordingly, the safe current product boundary is:

- **Authentic market-data research:** implemented across admitted venue adapters, subject to each adapter's actual read support.
- **Paper execution:** implemented for spatial, triangular, funding perp/perp carry, and the Spread Radar lifecycle; same-venue basis scanning exists but its paper execution contract is broken in the reviewed state.
- **Live adapter transport:** signing/submission/cancellation exists for many venues, but this is not equivalent to reconciled live strategy execution.
- **New live strategy entries:** must fail closed until both legs have terminal fill reconciliation, cancellation handling, leverage semantics, and durable recovery.
- **Risk-reducing live closes:** present, but accepted/working orders are not yet continuously reconciled into the ledger and must be described as submission workflows rather than confirmed closes.
- **Spot, cross-ticker, prediction, and live funding strategy dispatch:** disabled or unavailable.

The system should not be called production-ready until the P0/P1 items below are closed and tested with venue-specific sandbox credentials.

- **Post-remediation position:** the fail-closed boundary is internally coherent and live entries are now opt-in for canary validation. Live strategy entries default disabled and require `live_entry_enabled` + a per-leg notional cap + unlocked vault + per-venue submission/polling/credentials (spatial two-leg path only — triangle/funding live dispatch is not built and refuses honestly). Capability semantics distinguish submission, polling, and strategy-entry readiness; credential IPC and vault lifecycle gaps are closed; paper basis executes exact spot/perp legs; funding dispatch is durable before concurrent execution; close workflows preserve unresolved working orders and venue order IDs; every live-submission adapter exposes validated `fetch_order` polling and a startup+5s supervisor reconciles durable working orders to terminal state with monotonic accounting; automated residual-leg resolution (opt-in `auto_resolve_residual_exposure`, max 3 bounded flatten attempts) and cancel-retry at the working-order deadline (3 kv-persisted attempts) are implemented; signer provenance is functionally verified from pinned upstream source; CI/deployment/security controls exist. The remaining gate on unbounded live entry is venue sandbox/canary evidence.

---

## 2. Review Method and Source Authority

The audit used this precedence when sources conflict:

1. Latest explicit owner requirements captured in GitHub issues and the newest handoff entries.
2. `SPEC.md`, with newer dated decisions overriding older sections.
3. Current source code and tests.
4. README and operational/reference documentation.
5. Historical handoff claims, which are evidence of intent but not proof of current behavior.

Reviewed requirement sources include:

- `SPEC.md`
- GitHub issues #1–#8, especially open issues #3 and #8
- `docs/HANDOFF_MASTER_SESSION_HISTORY.md` through session 73
- `README.md`
- `docs/ARCHITECTURE.md`
- `docs/EXCHANGES.md`
- `docs/QUANT_RESEARCH.md`
- `docs/REBUILD_GUIDE.md`
- `docs/RUNBOOK.md`
- `docs/STRATEGY_RESEARCH.md`
- `ops/deploy/*`

Reviewed implementation surfaces include:

- `crates/arbitrageur-core`
- `crates/arbitrageur-math`
- `crates/arbitrageur-exchanges`
- `crates/arbitrageur-execution`
- `crates/arbitrageur-risk`
- `crates/arbitrageur-strategy`
- `crates/arbitrageur-app`
- `frontend/src-tauri/src`
- `frontend/src`

### Agreed regression-test seams

- Public Tauri `*_impl` command functions for end-to-end engine behavior.
- `ExchangeAdapter` for venue capability and order-response contracts.
- State-store public methods for persistence and migration behavior.
- React service functions and rendered component props for frontend behavior.

---

## 3. Initial Working Tree and Review Preservation

At review start, `main` matched `origin/main` at `ea45503`. The working tree already contained:

- Modified `frontend/src/components/EngineSettings.tsx`
- Modified `frontend/src/components/ExchangeAccounts.tsx`
- Modified `frontend/src/components/PortfolioMonitor.tsx`
- Untracked `docs/PROJECT_REVIEW_2026-09-20.md`

Those tracked changes improve locked/stale/no-credentials balance guidance, but removal of the manual **Lock Vault** action conflicts with `SPEC.md` user story 27 and weakens the operator's security control. They must be integrated deliberately rather than silently overwritten or discarded.

---

## 4. Independent Findings

## 4.1 P0 — Live strategy execution confuses submission with reconciliation

### Requirement

The system must fail closed, avoid stranded exposure, execute two legs atomically where possible, compensate partial fills, and report the true state of every order.

### Evidence

- `ExchangeAdapter::supports_live_execution` and `live_order_submission_implemented` only establish that an adapter can attempt submission; they do not establish terminal fill reconciliation.
- `get_venue_execution_capabilities_impl`, `set_live_mode_impl`, `save_engine_settings_impl`, and the settings UI derive “live ready” from `supports_live_execution + credentials`.
- `execute_spatial_arbitrage_inner` uses that same weaker gate.
- Lighter, Vanta, Kraken, Coinbase, PopDEX, and Decibel can return `OrderStatus::New`, `filled_qty = 0`, and the full quantity remaining immediately after a real submission.
- Binance and Aster do not request a guaranteed terminal response or perform a follow-up fill query in the reviewed implementation.
- OKX is the notable adapter that polls order state.
- `AtomicArbitrageCoordinator` interprets two zero-fill `Ok(New)` responses as no residual exposure. It does not cancel or continue reconciling those accepted orders.
- The live spatial path ignores both `set_leverage` errors. Hyperliquid and Lighter explicitly return “not implemented” for leverage changes.
- `EmergencyUnwindHandler` creates the compensating market order without setting `reduce_only = true`.

### Impact

A real venue can accept and later fill an order after the engine has declared the attempt failed and stopped tracking it. This can create naked or duplicated exposure while the local ledger says no position exists.

### Required correction

- Separate **submission capability** from **strategy-entry readiness**.
- Require terminal fill reconciliation, cancellation/recovery, reduce-only compensation, and explicit leverage semantics before admitting an adapter to live strategy entry.
- Until those contracts exist and are sandbox-tested, block all new live strategy entries globally.
- Keep read-only account/market data and explicitly risk-reducing workflows separate from entry readiness.

### Acceptance criteria

- No UI or backend field calls a venue “live ready” based only on credentials and submission support.
- An accepted asynchronous order remains durable and actively reconciled until filled, cancelled, expired, or escalated unresolved.
- Unknown order state always trips/blocks; it is never treated as zero exposure.
- Leverage-setting failure either blocks the order or is covered by an explicit, verified venue invariant.
- Every emergency compensation order is reduce-only where the venue supports that semantic.

**Initial status:** OPEN\
**Remediation status:** ENTRY-SAFE CLOSURE + RECONCILIATION LANDED. New live strategy entries still fail closed globally; `submission_ready`, `strategy_entry_ready`, `live_ready`, and `order_polling_ready` are distinct DTO fields. Every live-submission adapter (Binance, Aster, OKX, Kraken, Vanta, Coinbase, Hyperliquid, Lighter, Robinhood-via-Lighter, Decibel, PopDEX) now implements a validated `fetch_order`, and the desktop reconciliation supervisor polls durable working orders every 5s, re-scans them at startup, applies monotonic fill/fee updates with venue/symbol/order-id response validation, escalates orders working past 15 minutes to `unresolved`, and keeps `in_flight`/`imbalanced`/`unresolved` signals blocking the ticker (a mixed fill+empty-cancel entry maps to `imbalanced`, never `failed`). Kraken's nonzero-execution acknowledgement is resolved through `openorders` + `/fills` aggregation instead of being trusted. Emergency unwind orders are reduce-only, and the live leverage branch propagates failures. Still open for live entry: operator opt-in for automated residual-leg resolution (`auto_resolve_residual_exposure` — implemented, max 3 bounded attempts) and cancel-failure retry (implemented, 3 kv-persisted attempts at the 15-min deadline), plus sandbox/canary evidence per venue.

---

## 4.2 P0 — Whole credential bundles remain readable over IPC

### Requirement

`SPEC.md` security hardening requires field-scoped reveal only, and `README.md` claims there is no credential-read IPC.

### Evidence

- `get_account_credentials_impl` returns API key, secret, passphrase, wallet/account identifiers, and private key together.
- `get_account_credentials` is registered in `frontend/src-tauri/src/main.rs`.
- `api.getAccountCredentials` remains in `frontend/src/services/api.ts` and in the native-command allowlist.
- The current UI uses the safer `get_account_credential_field`, so the whole-bundle command is unnecessary.
- `test_no_credential_read_ipc_registered` checks several old names but omits the actual registered `get_account_credentials` command.

### Impact

Any renderer compromise while the vault is unlocked can request every secret in an account with one IPC call. This directly contradicts the documented field-scoped design.

### Required correction

Remove the whole-bundle implementation, Tauri registration, frontend service method, allowlist entry, and obsolete tests. Strengthen the registration test to reject `get_account_credentials` explicitly.

**Initial status:** OPEN\
**Remediation status:** CLOSED. The bulk credential IPC is absent from registration, frontend services, and the command allowlist; the registration test now names `get_account_credentials` as a forbidden bulk-read command.

---

## 4.3 P0 — Safety and capability documentation is materially false

This independently confirms DeepSeek finding 5.1.

### Evidence

- README, SPEC, and rebuild guide claim live submission is universally disabled.
- Multiple adapters can sign and submit real orders.
- Conversely, source and handoff language sometimes calls adapters “fully live” even though most responses are only submission acknowledgements and strategy reconciliation is incomplete.
- The current documentation therefore understates one risk while overstating another capability.

### Required correction

Publish one authoritative capability matrix with separate columns for:

- Market-data support
- Account reads
- Signing
- Submission
- Cancellation
- Terminal fill reconciliation
- Leverage updates
- Reduce-only close support
- Paper strategy support
- Live strategy-entry support
- Verification level (unit fixture, public live read, sandbox order, mainnet order)

**Initial status:** OPEN\
**Remediation status:** CLOSED. `docs/CAPABILITIES.md` is now the authoritative matrix and separates read, submission, cancellation, risk-reducing close, paper strategy, and live strategy-entry support. README, SPEC, architecture, exchange, rebuild, runbook, and security copy use the same boundary.

---

## 4.4 P0 — Same-venue spot/perp basis rows cannot execute correctly

This independently confirms DeepSeek finding 5.2.

### Evidence

- The scanner emits display labels such as `Lighter SPOT` and `Lighter PERP`.
- `canonical_venue_id` and `get_adapter` accept venue identities, not those leg labels.
- Funding execution resolves both books and both orders from one root symbol.
- Signal legs persist venue and side but not their own market symbol.
- Flattening also uses the root signal symbol for every leg.
- The auto-carry loop can select a basis row because it does not distinguish executable strategy contracts.

### Impact

The UI presents “Open Carry” for a row that cannot map to the intended spot and perpetual instruments. A superficial venue-label fix would still trade the wrong books and later flatten the wrong symbols.

### Required correction

- Add canonical venue ID, market type, and exact symbol for each opportunity leg.
- Persist a symbol per signal leg with a backward-compatible state-store migration.
- Execute the spot and perp symbols independently through the concurrent two-leg paper coordinator.
- Flatten each leg using its persisted symbol.
- Add scan → execute → position → flatten regression coverage.

**Initial status:** OPEN\
**Remediation status:** CLOSED. Funding opportunities now carry canonical venue IDs, exact leg symbols, and market types; state-store migration v22 persists/backfills exact leg symbols; same-venue `BTC-SPOT`/`BTC-USDC` basis executes both books through the paper coordinator and has a scan → execute → aggregate → flatten regression test.

---

## 4.5 P1 — Funding paper dispatch is sequential and not crash-durable before execution

### Evidence

- `execute_funding_carry_impl` places the long leg and then the short leg in a loop.
- It records the two-leg signal only after both fills succeed.
- If the process terminates after the first fill, no durable in-flight signal records the exposure.
- If the second leg fails, the first leg remains open and is only reported for manual flattening.

### Required correction

Use the existing concurrent coordinator, persist the in-flight two-leg signal before dispatch, reconcile actual fills, trim/compensate partials, and trip the breaker on unresolved exposure.

**Initial status:** OPEN\
**Remediation status:** CLOSED for paper funding. `execute_funding_carry_impl` persists the two-leg signal before dispatch, executes through `AtomicArbitrageCoordinator`, records per-leg fills/order IDs, marks unresolved exposure, and trips the breaker on failed unwind. Live funding entries remain disabled.

---

## 4.6 P1 — Hyperliquid can fabricate fill fields and order IDs

### Evidence

In `HyperliquidAdapter::place_order`:

- Missing `filled.totalSz` falls back to the requested quantity.
- Missing `filled.avgPx` falls back to the requested limit price.
- Missing order IDs and unknown response shapes can fall back to random UUIDs.

### Impact

Malformed or changed venue responses can be converted into plausible fills. This violates the fail-closed requirement and can corrupt compensation, attribution, and risk state.

### Required correction

Treat missing fill quantity, average price, or venue order ID as an error/unknown state. Never synthesize those fields in a live adapter.

**Initial status:** OPEN\
**Remediation status:** CLOSED. `parse_hl_order_response` now fails closed on missing `oid`, `totalSz`, `avgPx`, malformed quantities, unknown response shapes, and impossible fill bounds; unit tests cover fill, partial, resting, and mandatory-field rejection.

---

## 4.7 P1 — The specified 60-second vault re-lock is absent

### Requirement

GitHub issue #2 and `SPEC.md` require a lock 60 seconds after leaving System Settings, cancelled if settings are reopened.

### Evidence

- The frontend has no settings-close timer.
- `SecureVault` instead uses a general 15-minute unlock expiry.
- The historical issue-closure comment claims the 60-second behavior existed, so this is a regression.
- The pre-existing working-tree change removes the only manual Lock Vault action, contradicting `SPEC.md` user story 27.

### Required correction

Restore an explicit manual lock action and implement the 60-second post-settings timer with reopen cancellation. Keep the backend 15-minute maximum session expiry as defense in depth and document both timers.

**Initial status:** OPEN\
**Remediation status:** CLOSED. `frontend/src/services/vaultLock.ts` implements the 60-second settings-exit timer with reopen cancellation and one-shot cleanup; the manual Lock Vault action is restored, and tests cover fire-once, cancel, and post-fire no-op behavior.

---

## 4.8 P1 — Vault copy overstates encryption coverage

### Evidence

`VaultUnlockModal` says “All API keys and workspace data are encrypted at rest.” The vault encrypts credentials, but `state.sqlite3` stores settings, samples, logs, positions, and history without application-level encryption.

### Required correction

State precisely that credentials are encrypted in `vault.enc`; operational state/history is stored locally in SQLite and protected by filesystem permissions, not vault encryption.

**Initial status:** OPEN\
**Remediation status:** CLOSED. Vault copy now scopes encryption to credentials, and `SECURITY.md` documents that SQLite operational state is local plaintext protected by filesystem permissions rather than application-level encryption.

---

## 4.9 P1 — Open calibration requirements remain, while status text is stale in both directions

### Evidence

Open GitHub issues #3 and #8 are authoritative outstanding requirements.

Implemented since those issue bodies were written:

- Durable spread samples and signal series
- Session profiles and monitor coupling
- HAR anchors
- Daily promotion scheduling
- One-click application of selected READY AutoTune rows
- Apply audit events

Still missing or incomplete:

- Conservative/Balanced/Aggressive preset table and one-click preset apply
- Practical manual parameter-setting guidance
- Paper-fill/slippage feedback joined into calibration
- Explicit per-ticker manual/periodic/continuous auto-tune modes
- A complete structured data-readiness object rather than only row-level HELD rationales
- A production-quality estimator WFO command; `run_wfo_calibration_impl` remains an unconditional error and its component is dead/unmounted

Stale copy still calls AutoTune “recommend-only” and “transcribe manually” even though selected READY rows can now be applied.

### Required correction

Update issues/docs/status text to distinguish implemented Phase 2 pieces from genuine remaining scope. Add the preset/manual-guidance layer now; retain paper-feedback and autonomous tuning as explicitly gated research unless backed by enough real history.

**Initial status:** OPEN\
**Remediation status:** PARTIALLY CLOSED. `docs/PARAMETER_GUIDE.md` now provides practical manual parameter guidance and explicitly explains why named risk presets remain gated without an outcome corpus. AutoTune copy distinguishes READY-row application from HELD/manual authority. Still open: evidence-backed presets (gated on outcome history). **Update (post-review):** the WFO command is now real — `run_wfo_calibration_impl` rebuilds the executable cross-venue edge series (max bid − min ask across distinct venues) from durable `spread_samples`, prices friction from the two cheapest admitted venues' effective taker fees plus an exit allowance, takes the target from the monitor's configured expected-profit floor, and runs the walk-forward engine over it. It fails closed below 40 multi-venue observations and the fabricated fallback `EstimatorEvaluation` (hardcoded Sharpe/profit/win-rate) was removed — unevaluable or insufficient inputs now return honest empty/zero results with an explanatory note. The Strategy Performance button is enabled and surfaces the error inline. **Update (fill feedback):** the structured readiness object (`AutotuneReadinessDto`: venues seen, history span, multi-venue coverage, pair-observation floor, blockers) and the slippage feedback join have both landed. Schema v27 persists `signal_legs.intended_price` (the dispatch-time quote); `leg_slippage_samples_bps` measures direction-aware realized slippage per venue, and AutoTune adds the qty-weighted median (≥5 priced fills required) to the static fee estimate in `friction_fit_bps`/`friction_val_bps` — the tuner now prices measured execution cost instead of fee-only friction. Missing intended/fill prices stay excluded (never zero-slippage, never fabricated) and the measured value is surfaced in the report's readiness block and preview modal. **Update (tuning modes):** per-ticker AutoTune cadences landed — `autotune.mode.{SYMBOL}` ∈ manual|periodic|continuous (KV-persisted, manual default). A 60s scheduler enumerates mode keys, re-tunes due symbols (periodic default 6h, continuous default 15m), and auto-applies READY rows through the audited apply path with `detail.source` recording the mode; failures back off a full interval and manual/unknown modes never auto-run.

---

## 4.10 P1 — CLI prediction research still fabricates fallback prices

### Evidence

`crates/arbitrageur-app/src/main.rs::run_prediction_arb` substitutes fixed prices when either book side is absent.

### Impact

The desktop path may be fail-closed while the CLI still emits a plausible parity candidate from invented values, violating the repository-wide zero-synthetic requirement.

### Required correction

Return a clear unavailable-data error when any required real quote is missing. Keep synthetic data only in commands explicitly named and documented as simulation.

**Initial status:** OPEN\
**Remediation status:** CLOSED. The CLI prediction path now errors on each missing Polymarket YES bid or Kalshi NO bid/ask instead of substituting fixed prices, and exact quote-math tests cover both success and every missing-leg failure.

---

## 4.11 P1 — Live close/flatten workflows submit but do not continuously reconcile working orders

### Evidence

- External close records a zero-fill response as “submitted / working — re-check the venue.”
- No background worker later updates that signal when the order fills, cancels, or expires.
- The root signal can remain in-flight and block the symbol indefinitely.
- Live flatten uses the same broad `supports_live_execution` gate rather than terminal reconciliation capability.

### Required correction

Classify these operations as submission workflows, persist venue/client IDs before returning, and add reconciliation or provide an explicit operator resolution path. Do not label an acknowledgement as a confirmed close.

**Initial status:** OPEN\
**Remediation status:** CLOSED (detection layer) / OPEN (auto-resolution). External closes persist intent before dispatch (the generated client order ID is the durable signal ID), validate venue/symbol/client/order/quantity/status/price/fee consistency, store venue order IDs on the durable leg, distinguish filled/partial/working/not-filled/unverified, and mark `Closed` only after a fresh account reread confirms flatness. Generic flatten legs preserve accepted zero-fill and terminal zero-fill order IDs instead of collapsing them to “empty book.” Live account reads resolve venue-native position symbols through adapter-owned normalization (`BTCUSDT`, `PF_XBTUSD`, `BTC-USDT-SWAP`, `PERP_BTC_USDC`, `BTC-PERP`, Lighter `market_id` mappings, and Binance/Aster scaled contracts kept in venue-native `1000PEPE` units); unknown or ambiguous mappings remain unresolved rather than treated as flat. Continuous venue polling and restart-time reconciliation are now implemented: the `reconciliation` supervisor runs an immediate pass at startup plus a 5s poll over durable nonterminal legs, validates venue/symbol/order-id identity and USD-stable fee currency, applies monotonic cumulative updates, and escalates orders working past 15 minutes to `unresolved`. Order identity is persisted on every submission path — generic flatten legs, `reconcile_signal_legs`, and both `UnresolvedExposure` handlers now record venue and client order ids even for accepted-but-unfilled (working) orders, so no live submission can orphan its pollable handle. Synthetic `sim-*` paper order ids are excluded from the live poll set. Automatic resolution is now implemented as an opt-in: `auto_resolve_residual_exposure` dispatches reduce-only residual flattens (persisted `flat-*` children, deduplicated against open children, bounded to 3 attempts), and stale working orders get up to 3 kv-persisted cancel attempts before escalation. The mechanism is untested against real venues — sandbox/canary evidence remains the gate.

---

## 4.12 P1 — Unchecked signer binary in the live signing path

This independently confirms DeepSeek finding 5.9.

### Evidence

- `liblighter_signer.so` is loaded from an environment override, executable sidecar, or vendored path.
- No digest is checked before unsafe FFI loading.
- The reviewed binary SHA-256 is `8cb2fbcadab0b9b3fed4a524998b825818b677fcf9c49bc1f18b4ca02a233753`.
- No pinned upstream commit or reproducible build recipe is present.

### Required correction

Verify an approved SHA-256 before load, require an explicit digest for custom overrides, add a CI integrity check, and document upstream provenance/build reproduction. A checksum protects against silent drift but does not substitute for reproducible provenance.

**Initial status:** OPEN\
**Remediation status:** CLOSED (functional provenance). The vendored signer is hashed before `dlopen`, the approved SHA-256 is enforced in CI, override libraries require an explicit matching digest, and `native/lighter/README.md` documents the boundary. Provenance is now functionally verified rather than trust-based: `native/lighter/verify/verify_provenance.sh` rebuilds the signer from the pinned upstream tag (`github.com/elliottech/lighter-go` v1.0.10 @ `9d38261d1a4cc5c7211b383ba07a4d6e41604708`, Go 1.24.6, `CGO_ENABLED=1 go build -buildmode=c-shared -trimpath`), confirms both binaries derive the identical public key for a fixed private key, and cryptographically validates signatures from both libraries against the upstream `schnorr.Validate` — byte equality is not required because Go builds are not bit-reproducible and Schnorr signing is randomized. A dedicated CI job runs the same script.

---

## 4.13 P2 — Repository verification is not enforced

This independently confirms DeepSeek finding 5.5.

### Evidence

- No CI workflow exists.
- Rust toolchain is not pinned.
- Formatting currently drifts.
- Clippy has known failures in `funding_carry.rs`.
- No Rust dependency/advisory policy or security policy exists.

### Required correction

Add pinned toolchains and CI gates for format, clippy, Rust tests, frontend tests/build, npm production audit, Rust advisories, and secret scanning. Add `SECURITY.md` and dependency-update automation.

**Initial status:** OPEN\
**Remediation status:** CLOSED. `rust-toolchain.toml`, `SECURITY.md`, Dependabot, and CI now cover whitespace, signer digest, fmt, clippy `-D warnings`, locked workspace tests, frontend install/tests/build/audit, Rust advisories, Gitleaks, headless build, and systemd template checks.

---

## 4.14 P2 — Documentation has pervasive internal contradictions

### Examples

- `SPEC.md` marks seasonality partially implemented even though HAR coupling, promotion scheduling, and apply auditing exist.
- Parts 3 and 5 simultaneously say “implemented” and “implementation in progress.”
- `SPEC.md` says only three adapters genuinely submit, while later handoffs add many more.
- `docs/EXCHANGES.md` says long-tail venues were removed, then describes them as admitted in present tense.
- `docs/ARCHITECTURE.md` lists only a subset of adapters and claims remote HSM parity that is not the current execution contract.
- `docs/REBUILD_GUIDE.md` says nothing touches real funds.
- `docs/HANDOFF_MASTER_SESSION_HISTORY.md` has a stale “Open Items” section and a stale directory tree.
- `docs/QUANT_RESEARCH.md` calls carry “pure risk-free yield” and prediction parity “guaranteed,” omitting execution, settlement-equivalence, funding, custody, and counterparty risks.
- Frontend copy says native execution is disabled backend-wide.
- AutoTune copy says recommend-only despite an apply path.

### Required correction

Make README + capability matrix authoritative, label handoff sections historical, remove absolute return claims, and update every current-state document to the same capability vocabulary.

**Initial status:** OPEN\
**Remediation status:** MOSTLY CLOSED. Current-state docs now defer to the capability matrix, handoff history is labeled historical, “Real Trading” UI wording is now “Live Trading,” prediction payout is conditional, and research docs qualify carry/settlement risk. Historical claims inside the handoff remain as history rather than current authority.

---

## 4.15 P2 — Deployment instructions reference files that do not exist

### Evidence

The deployment files are still named `arbitrager*.service/env/logrotate`, while `ops/deploy/README.md` instructs users to copy `arbitrageur*` paths. The service's `ExecStart` also uses shell-style `${VAR:-default}` syntax even though systemd does not execute `ExecStart` through a shell.

### Required correction

Provide correctly named templates, use a systemd-compatible default environment, validate the unit, and make clear that the current headless daemon is only a 503 health placeholder and creates no trading state by itself.

**Initial status:** OPEN\
**Remediation status:** CLOSED. `arbitrageur@.service`, `.env`, and logrotate templates now exist alongside legacy `arbitrager*` aliases; shell-style `${...}` defaults are gone, CI verifies both units, and docs state that `serve` is a loopback `503 not_ready` placeholder only.

---

## 4.16 P2 — Tauri integration modules are too shallow at the top and too broad inside

This independently confirms DeepSeek finding 5.8.

`commands.rs` and `state_store.rs` combine unrelated domains, which increases review scope for every change. The next extraction should be domain-first:

- capabilities/live-mode policy
- vault/account commands
- spatial execution
- funding/basis execution
- portfolio/risk
- calibration/seasonality

This is a maintainability finding, not a justification for a risky behavior-neutral mega-refactor in the same safety patch.

**Initial status:** OPEN / staged refactor recommended\
**Remediation status:** IN PROGRESS — stages 1–5 landed. `commands.rs` 21625 → 18555 lines across five domain extractions: `autotune.rs` (recommend/apply/modes/scheduler), `strategy_config.rs` (ticker configs + universal params), `accounts.rs` (vault lifecycle, credential profiles, strategy routes, venue balances), `performance.rs` (strategy metrics, forecast calibration, fill P&L, WFO), `monitor_ipc.rs` (radar IPC + monitor→execution entry). All IPC command paths preserved via `pub(crate) use` glob re-exports — zero call-site churn; 371 desktop tests green per stage. Remaining stages — spatial/triangle execution core, funding scan+carry (four non-contiguous regions), capabilities/live-mode policy — continue incrementally per this finding's own caution against a behavior-neutral mega-refactor.

---

## 5. DeepSeek Review Cross-Check

| DeepSeek finding | Independent verdict | Cross-check detail |
|---|---|---|
| 5.1 Safety/capability docs stale | **Confirmed, severity retained P0** | Documentation is false in both directions. The independent audit additionally found unsafe async-order handling, making truthful capability terminology more urgent. |
| 5.2 Basis scans but cannot execute | **Confirmed** | Venue-label mismatch is real, and the deeper missing per-leg symbol persistence/flatten contract is also confirmed. |
| 5.3 Adapter live support does not equal strategy support | **Confirmed but understated** | “Spatial live path exists” is only a code-path statement. It is not safely reconciled across most adapters and should be blocked pending terminal fill handling. |
| 5.4 Uncommitted frontend work can be lost | **Confirmed** | Preserve the balance-status improvements. The Lock Vault removal conflicts with an explicit security requirement and should not land unchanged. |
| 5.5 Formatting, lint, CI absent | **Confirmed** | Add toolchain pinning, CI, advisories, secret scanning, and security policy. |
| 5.6 In-app copy stale | **Confirmed and expanded** | Funding copy, live result messages, AutoTune “recommend-only” wording, and vault encryption copy are also stale or misleading. |
| 5.7 Exchange scope ambiguous | **Confirmed** | `EXCHANGES.md` is internally contradictory; selectors and roadmap inventory need one policy. |
| 5.8 Tauri command monolith | **Confirmed** | Domain extraction is warranted, but should follow safety fixes rather than obscure them. |
| 5.9 Signer binary integrity missing | **Confirmed** | Digest measured independently; provenance/reproducibility is also missing. |

### Important findings absent from the DeepSeek review

1. Whole-account credential bundle IPC remains registered.
2. New live spatial orders can be submitted without terminal fill reconciliation.
3. Leverage update errors are ignored in live entry.
4. Emergency compensation is not explicitly reduce-only.
5. Hyperliquid fabricates missing live fill fields/order IDs.
6. Funding paper dispatch is sequential and recorded only after fills.
7. The 60-second post-settings vault lock is missing.
8. Vault UI falsely claims all workspace data is encrypted.
9. Open issues #3/#8 are partly stale but still contain unmet requirements.
10. CLI prediction mode still uses fixed fallback prices.
11. Deployment templates have broken names/systemd expansion.
12. Research docs make unjustified “risk-free” and “guaranteed” claims.
13. Binance/Aster scaled-contract (1000x) handling scaled `positionAmt` ×1000 on read and would scale `OrderRequest` ×1000 again on dispatch — a flatten would submit ~1,000,000× the intended quantity (correct semantics: venue-native `1000PEPE` units throughout; verified against live `exchangeInfo`).
14. Binance `to_venue_symbol` hardcoded `USDT` for non-1000x bases — `BTC-USDC` requests were silently routed to `BTCUSDT` (cross-quote misrouting).
15. OKX and Kraken Futures orderbook level sizes are contracts but were returned as base units — overstating depth by the contract factor (~100× on `BTC-USDT-SWAP`).
16. `fetch_volumes_24h` maps were keyed heterogeneously (`BTCUSDT`, `BTC-USDT-SWAP`, `PF_XBTUSD`, bare `BTC`) while volume consumers matched exact base names — Binance/OKX/Kraken volume was silently invisible to the auto volume filter, autotune capacity tiers, and signal-series weights. All adapters now key canonical `BASE-QUOTE`, and both matchers strip quote/instrument suffixes deterministically (deepest matching market wins).

---

## 6. Requirements Traceability Summary

| Requirement area | Reviewed state | Post-remediation verdict |
|---|---|---|
| Authentic market data / zero synthetic desktop scan | Broadly present | Aligned after CLI prediction fallback removal |
| Paper spatial execution | Implemented with extensive gates | Aligned after regression checks |
| Spread Radar + autonomous convergence | Implemented, paper-only | Aligned |
| Paper funding perp/perp carry | Implemented | Aligned after durable pre-dispatch signal + concurrent coordinator fix |
| Same-venue spot/perp basis | Scanner only | Aligned for paper after exact leg-symbol persistence and regression coverage |
| Spot cross-venue strategy | Disabled | Explicitly disabled/roadmap; not presented as executable |
| Triangle strategy | Paper scan/execute | Aligned for paper mode |
| Prediction strategy | Research/scan, execution disabled | Explicitly research/conditional payout only |
| Statistical arbitrage | Research tooling | Explicitly research tooling |
| Live adapter signing/submission | Implemented for many venues | Transport capability only; not strategy readiness |
| Live order polling/restart reconciliation | Missing | Implemented: per-adapter `fetch_order` on all 11 live-submission venues plus a startup+5s supervisor with monotonic updates, identity validation, and 15-minute unresolved escalation |
| Live strategy execution | Unsafe/incomplete | Safely blocked pending operator opt-in for the implemented residual-resolution/cancel-retry mechanisms, plus sandbox/canary evidence |
| Credential vault crypto/file permissions | Implemented | Aligned after bulk IPC removal and lifecycle fixes |
| 60-second settings-exit lock | Missing | Aligned |
| Field-scoped credential reveal | UI uses it, legacy bulk IPC remains | Aligned after bulk IPC removal |
| Dynamic ticker universe | Implemented in latest commit | Aligned |
| WebSocket feeds/retention | Implemented for primary lanes with fallbacks | Mostly aligned; adapter coverage remains venue-specific |
| Seasonality/HAR/promotion | Largely implemented | Aligned; remaining AutoTune scope is tracked separately |
| AutoTune apply/audit | Implemented for selected READY rows | Aligned for selected READY rows |
| Presets/manual parameter guidance | Missing | Partial: manual guide added; evidence-backed presets remain open |
| CI/reproducibility/security policy | Missing | Aligned; signer provenance is functionally verified from pinned upstream source in CI |
| Headless deployment | Health placeholder only | Aligned: templates repaired and placeholder scope documented |

---

## 7. CROPS Review

### Chosen default

Local-first desktop research and paper execution, with direct venue connections and new live strategy entries disabled until reconciled. This is the most security-aligned state the present code can honestly support.

### Censorship Resistance

- **Risk:** Every centralized venue API, DEX frontend/RPC, sequencer, app distribution channel, and network provider can block or degrade access.
- **Mitigation:** Open local client, direct venue endpoints, fail-closed adapters, and documented manual venue exit paths.
- **User escape:** Self-build the desktop client and close positions directly at each venue. CEX custody and API availability remain unavoidable dependencies.

### Open / Free

- **Risk:** The Rust/React source is MIT/Apache-2.0, and the vendored Lighter signer is now pinned to `lighter-go` v1.0.10 @ `9d38261d…` with a committed functional-equivalence proof (`native/lighter/verify/verify_provenance.sh`) run in CI.
- **Mitigation:** Provenance, build recipe, digest verification, and signature validation are all committed; surrounding code remains forkable.
- **User escape:** The application can be forked and self-hosted; the signer can be rebuilt from upstream source rather than trusted.

### Privacy

- **Risk:** Direct REST/WS/account requests disclose IP address, requested instruments, timing, account identifiers, and trading behavior to venues/RPC providers. SQLite history is local but plaintext at the application layer.
- **Mitigation:** No analytics SDK is present; collect only required account data; document network metadata and local-file exposure.
- **User escape:** Use operator-controlled networking/RPC where venue terms permit, disable unused venues, and delete/rotate local state and credentials through documented procedures.

### Security

- **Risk:** CEX custody, local signing keys, asynchronous fills, venue-specific leverage/reduce-only semantics, and and residual-exposure auto-resolution (now implemented but opt-in and venue-unevidenced) can cause loss or lockout.
- **Mitigation:** Least-privilege keys, encrypted vault, explicit capability matrix, digest + provenance verification, fail-closed live entry, durable in-flight state with startup/5s reconciliation, circuit breaker, and tested compensation.
- **User escape:** Direct venue access remains the ultimate recovery path; the app must never be the only way to close or withdraw.

### Accepted compromises

- Centralized venue and RPC dependencies are inherent to the selected markets.
- Local SQLite operational history is not application-encrypted.
- Risk-reducing live order submission is reconciled to terminal state by the supervisor; an order that cannot be resolved escalates to `unresolved` rather than being assumed filled or cancelled.

---

## 8. Remediation Plan and Completion Gates

### Phase A — Immediate safety

1. **Closed:** Removed bulk credential IPC and added a registration test that names it.
2. **Closed at the entry boundary:** Submission readiness and strategy-entry readiness are separate; new live strategy entries fail closed globally. Terminal/restart reconciliation is implemented for all live-submission venues (order polling + startup scan + monotonic ledger updates); what remains gated is venue evidence for the now-implemented residual-resolution and cancel-retry mechanisms (opt-in via `auto_resolve_residual_exposure`).
3. **Closed:** Hyperliquid and close/flatten response handling no longer fabricate fills or treat acknowledgements as terminal facts.
4. **Closed:** Emergency compensation and flatten paths use reduce-only orders; live leverage failures now propagate if a future entry path reaches them.
5. **Closed:** Manual lock and the 60-second settings-exit lock are restored and tested.

### Phase B — Required functionality

1. **Closed:** Exact leg market identity is persisted and used for spot/perp basis.
2. **Closed for paper:** Funding dispatch now records a two-leg signal before concurrent coordinator execution.
3. **Closed:** Live-mode strategy/auto entry paths fail closed instead of retrying submission-only adapters.
4. **Closed:** CLI prediction fallback prices were removed.

### Phase C — Integrity and operations

1. **Closed:** Vendored signer integrity is enforced and upstream provenance is functionally verified in CI (pinned tag `v1.0.10` @ `9d38261d…`; identical key derivation + signature validation via `schnorr.Validate`).
2. **Closed:** Toolchain, CI, advisories, secret scanning, Dependabot, and `SECURITY.md` are in place.
3. **Closed:** `arbitrageur*` deployment templates and systemd-compatible environment defaults exist.
4. **Closed:** The workspace is formatted and clippy-clean with warnings denied.

### Phase D — Product truth

1. **Closed:** `docs/CAPABILITIES.md` is the authoritative capability matrix.
2. **Closed:** README, SPEC statuses, architecture, exchange inventory, rebuild guide, runbook, security, and handoff framing were aligned to actual behavior.
3. **Partially closed:** Manual parameter guidance exists; evidence-backed named presets remain gated.
4. **Closed:** Current product copy avoids unsupported “risk-free,” “guaranteed,” “fully live,” and blanket “paper-only” claims.

### Final completion gates

- Rust format and clippy clean with warnings denied: **pass**.
- Full Rust workspace tests pass: **pass**.
- Frontend tests, typecheck, and production build pass: **pass**.
- Dependency and secret scans produce no unresolved high-severity finding: **pass for npm production audit and Rust advisories; Gitleaks is enforced in CI**.
- Basis scan → execute → position → flatten regression passes: **pass**.
- Bulk credential-read IPC is absent from command registration and frontend allowlists: **pass**.
- Live-entry safety tests prove submission-only adapters cannot enter a strategy path: **pass**.
- Git diff reviewed, committed, and pushed with a clean working tree: **pass** (`cf4e8b7` remediation; reconciliation work lands in the follow-up commit).

---

## 9. Verification Record

Independent post-remediation verification produced:

| Check | Result | Evidence |
|---|---|---|
| `cargo fmt --all -- --check` | PASS | Workspace formatting normalized (incl. src-tauri) |
| `git diff --check` | PASS | No whitespace errors |
| `cargo check --workspace` | PASS | Clean |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | PASS | All targets checked with warnings denied |
| `cargo test --workspace --locked` | PASS | 728 tests total: desktop 354, exchanges 155, math 102+52+16, execution 40, others 20 |
| `cargo test -p arbitrageur-desktop` | PASS | 354/354 tests |
| `cargo test -p arbitrageur-exchanges` | PASS | 154/154 tests |
| `npm test` | PASS | 210 frontend tests |
| `npm run build` | PASS | TypeScript typecheck + Vite production build |
| `npm audit --omit=dev` | PASS | 0 production vulnerabilities |
| `cargo audit` | PASS | No vulnerabilities; 12 allowed unmaintained/unsound warnings |
| Lighter signer SHA-256 | PASS | `8cb2fbcadab0b9b3fed4a524998b825818b677fcf9c49bc1f18b4ca02a233753` |
| systemd template syntax | PASS | Both `arbitrageur@` and `arbitrager@` verify (ExecStart absent locally is expected) |
| `grep -R '\${' ops/deploy/*.service` | PASS | No shell-style expansion remains |
| Secret scan (gitleaks, 154 commits) | PASS | 0 leaks after fingerprint-scoped allowlist of 7 confirmed test fixtures/dev sentinels in `.gitleaks.toml` |
| File modes | PASS | Index modes normalized (644 configs/docs, 755 `backup-sqlite.sh`); NTFS working-tree 777 display is filesystem mapping, not committed |

The static findings above were independently traced to their actual call paths rather than copied from the compared review. The only intentionally open items are venue sandbox/canary evidence for the opt-in residual-resolution/cancel-retry mechanisms, evidence-backed presets (awaiting outcome history), and the staged Tauri-module refactor.

---

## 10. Final Scope Caveat

“Full live support for every included exchange” cannot be truthfully declared from unit fixtures and public reads alone. Completion requires venue sandbox credentials, real accepted/partial/cancelled order traces, restart recovery exercises, and small-notional operator approval. The repository can and should be made fail-closed and internally aligned now; production live readiness remains a separately evidenced release gate.
