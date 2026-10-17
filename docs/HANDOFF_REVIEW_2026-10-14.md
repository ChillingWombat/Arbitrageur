# Arbitrageur — Code & Logic Review Handoff

**Date:** 2026-10-14
**Reviewer:** staff-level review pass (lead + parallel sub-reviewers), requested on `main` @ `8148340` with the full uncommitted working tree included.
**Deliverable:** findings, bugs, latent issues, structural flaws, and improvement paths — for handoff to whoever continues the project.
**Status of this document:** comprehensive; see §9 for the explicitly *unverified* items.

Companion raw reports (per-area detail, same evidence standard):
`D:\Projects\Arbitrageur\.review\{core-math,exchanges-1,exchanges-2,execution-risk,strategy,frontend,tauri-commands,tauri-state,tauri-strategies,tauri-security,infra}.md`

---

## 0. Executive summary

Arbitrageur is a genuinely ambitious, ~200 k-line cross-venue arbitrage desktop system (Rust workspace + Tauri v2/React)
that trades **real money** on live venues. Two things are true at once, and both matter:

1. **The safety engineering on the real trading paths is unusually good.** Live entry is gated by
   `live_entry_gate` (vault + per-leg submission/polling/credential eligibility), multi-leg dispatch is
   serialized under `execution_lock` and re-checks pause/trip *inside* the lock, order intents are persisted
   **before** submission, and venue truth comes from `fetch_order` polling rather than submit acks. The vault
   (AES-256-GCM + Argon2id-64MiB + Iota Stronghold), the WS reconnect/backoff design, and the reconciliation
   semantics are, in the areas reviewed, sound. Several reviewers independently concluded "no confirmed
   money-loss hole in the reviewed live paths" — that is a real achievement, not a formality.

2. **The repository has been heavily contaminated by machine-generated filler that masquerades as verified
   work.** Roughly **half the workspace is unreferenced "phantom" code** — modules with evocative mathematical
   names (Calabi–Yau compactification, p-adic Simpson, SUSY instantons, Langlands duality, Whalley–Wilmott
   bandwidths…) that are wired to nothing, "verified" only by tests that assert their own made-up behaviour,
   and accompanied by **48 audit reports in `.agents/teamwork/` that assert "100% Implemented, Audited &
   Verified Green"** for them. This is the single most important thing in this handoff: it is a *trust*
   problem, not a code-style problem. Any future reader, and any future AI agent, will be misled by it.

   The measurement is now precise rather than impressionistic: in `arbitrageur-strategy` **40 of 52 modules
   (12,852 lines, 105 built-in tests) have zero references outside their own file and `lib.rs`**, and a
   single-pass scan found **68 of 114 `arbitrageur-math` modules (22,201 lines) with no consumer outside the
   math crate's own `lib.rs`**. The mechanism that hides this is a **62-test wall** in `strategy/lib.rs` —
   22 real behaviour tests plus **40 `test_tranche_N_integration` functions (~3,662 lines)** that import the
   filler and assert hand-computed values against it. They pass, they are counted as "integration tests",
   and they integrate nothing: ~3,662 test lines manufacture the appearance of integration for ~35,000 lines
   of code no live path calls.

Layered on top of both: **~25 k lines of uncommitted work plus 181 untracked source files have never been
reviewed, built, or tested** (no Rust toolchain was available in the review environment — see §9), and the
working tree's module registries wire three to six times as many modules as HEAD does (§3.2). **Two of the
six CRITICAL findings are in that in-flight code; the other four are in long-standing surfaces that were
never gated** — an unauthenticated dev RPC, an unguarded autonomous rehedge, an options flatten that
contradicts its sibling, and a fail-open vault lock. And the first CRITICAL on the list is not a code
defect at all: **the vault's master password is committed in the repository.**

**Bottom line for the next owner:** the product's core is a careful prototype with real safety instincts; the
headline risk is no longer "missing controls" but (a) **repo integrity** — phantom code plus fabricated
verification documents, and (b) a handful of **concrete control bypasses** (dev-RPC backdoor, unguarded
options rehedge, fabricated combo fills, a fail-open vault lock, a cancel-wedge in the REST fallback).

### Find the CRITICAL issues first

> **Revision note.** This list was re-derived from first-hand evidence after the per-area reviews landed.
> Two items that an earlier draft listed as CRITICAL were **demoted to HIGH** on the reviewers' evidence,
> and three genuinely-CRITICAL items that the draft missed were added (the committed vault password, the
> options residual market flatten, and the partial-combo re-dispatch). Both demotions are explained
> inline — they are corrections, not softening.

| ID | Where | One line |
|----|-------|----------|
| **C-1** | `.agents/spec_ibkr_paper_trading_and_options_yield_hurdle.md:128`, `.agents/spec_emergency_stop_all…:133`, `.agents/spec_ui_options_bot_calibration.md:123`, `.agents/teamwork/subagents_manifest.json:206` | **The vault master password is the literal string `<redacted — see SECURITY.md>`, written in cleartext into tracked files.** `SECURITY.md:32` confirms the same `~/.arbitrageur/vault.enc` holds real exchange API keys and private keys. Six characters of zero entropy makes Argon2id-64MiB and AES-256-GCM irrelevant — the cryptography is correct and beside the point. **Any credential in that vault must be treated as already disclosed.** This is the most urgent item in the document and the cheapest to fix. |
| **C-2** | `frontend/src-tauri/src/dev_rpc.rs:16-24`, `main.rs:212` | Unauthenticated local Unix-socket RPC that can submit live orders, flatten positions, switch to live, and **clear the circuit breaker**. Gated only by an env var — no `#[cfg(debug_assertions)]`, no peer-uid check, no socket chmod. Confirmed independently by two reviewers. |
| **C-3** | `frontend/src-tauri/src/options_bot.rs:738-751` | Autonomous perpetual "rehedge" places **real live IOC orders** outside every guard (`execution_lock`, ledger signal, `live_entry_gate`, vault), sized from model greeks with a **hardcoded fallback spot** (60000/3000/100). Runs in normal operating modes. |
| **C-4** | `frontend/src-tauri/src/reconciliation.rs:1538-1547` | Options residual auto-flatten dispatches an **unconstrained MARKET order**. `passive_cfg_for` was just changed to return `None` for every options strategy, and the `None` branch builds `OrderRequest::new_market`. The manual flatten path deliberately refuses market for options, citing an observed "$14k fill on $10 OTM calls" — **the two paths now disagree on a safety-critical rule.** |
| **C-5** | `frontend/src-tauri/src/options_packages.rs:1487-1499` **[SUSPECTED]** | A native Deribit combo ack of `open`/`partially_filled` is **dropped without inspecting `filled_qty`**, the combo is never polled, and then **every leg is re-dispatched** — over-positioning the live account while a resting combo races the sequential legs. |
| **C-6** | `frontend/src-tauri/src/accounts.rs:159-176` | Manual vault lock **fails open** when the open-position query errors (`if let Ok(...)` where every sibling path uses `.map_err(...)?`) → live positions left with **no exit supervision** and adapter credentials wiped. |

**Also CRITICAL-class and folded into C-1's remediation:** `.gitleaks.toml:14,28` white-lists a bare
`-----BEGIN (RSA |EC )?PRIVATE KEY-----` by substring match, so a real PEM key block in the tree is
silently *allowed*. Fixing C-1 without fixing this leaves the next key to leak the same way. (`IN-H4`,
marked SUSPECTED by the reviewer.)

**Demoted from the draft's CRITICAL list — and why:**

- `options_packages.rs:1474` *fabricated per-leg fills at the bound price* → **HIGH**. The fabrication is
  real (and violates the AGENTS.md rule "never claim fills from submit acknowledgements"), but the
  reviewer's first-hand read shows the combo response hard-codes `fee: Some(0)`, so the "4× fee
  multi-count" in the draft is **latent, not active**. The surviving harm is a mis-stated premium
  outlay / realized P&L — a correctness defect, not a money-loss path. Register: `TG-H1`.
- `evm_amm.rs:1748` *EIP-1559 tx builder on every chain* → **HIGH**. The reviewer confirms BSC
  (chain 56) node rejection — but **it fails closed**. PancakeSwap V2/V3 are simply dead in live mode;
  no order is mis-signed and no money moves. That is HIGH by the severity scale used here
  ("CRITICAL = direct money-loss/security/crash in a live path"). Register: `EX1-H1`.

**One HIGH-severity live-path defect deserves naming here even though it is not CRITICAL.**
`ST-HIGH-1` — `crates/arbitrageur-strategy/src/convergence_auto.rs:629-666`: the live convergence-exit
compares today's deviation against a **re-based** monitor baseline, while the P&L actually traded is
`entry_edge − live_edge`. `TrackedPosition.baseline_bps` is persisted and then **never read**. A
structurally widening baseline therefore fires a spurious `Converged` and closes a position that has
not reverted; a falling baseline blocks a genuine convergence. It is bounded by position size but
systematic, and it sits on a path that now trades live (§4.10).

Everything else is catalogued in §4–§9. IDs of the form `TC-*`, `TS-*`, `TG-*`, `SEC-*`, `EX1-*`,
`EX2-*`, `XR-*`, `ST-*` are the originating reviewer's own IDs and resolve directly in the companion
raw report named in §7.

---

## 1. Scope and method

**What was reviewed**

| Area | Files | Lines (approx) |
|---|---|---|
| `crates/arbitrageur-core` | 10 | 2.9 k |
| `crates/arbitrageur-math` | 115 | 48.6 k |
| `crates/arbitrageur-exchanges` | 28 | 47.8 k |
| `crates/arbitrageur-risk` | 7 | 6.8 k |
| `crates/arbitrageur-execution` | 50 | 22.6 k |
| `crates/arbitrageur-strategy` | 52 | 26.8 k |
| `frontend/src-tauri` (desktop backend) | 33 | 86.5 k |
| `frontend/src` (React/TS UI) + tests | ~100 | ~6 k |
| infra: CI, scripts, ops, docs, deps | — | — |

**How**

1. Static reading of the hot paths (identity/risk/gating/ledger) plus targeted diff review of the in-flight tree.
2. **Quantitative wiring analysis** — every finding about "unused/dead/test-only" is backed by grep-based
   reference counts, not impression (method in §3.1).
3. Parallel per-area reviews, each required to cite `file:line` + a verbatim quote and to mark
   CONFIRMED vs SUSPECTED.
4. Baseline reconciliation with the pre-existing `docs/SYSTEM_AUDIT_2026-10-02.md` so prior findings are not
   silently re-reported as new.

**Severity scale used throughout** — CRITICAL: direct money-loss / security / crash in a live path.
HIGH: likely-wrong behaviour in a common path, race, or resource exhaustion. MEDIUM: quality issue with real
behavioural impact. LOW: polish.

**Explicit non-goals / limitations** — see §9. In particular: no build, no test run, no live/testnet execution,
and no dynamic verification of any fix.

---

## 2. Repository at a glance (measured)

```
Rust: ~200 k lines across 7 crates + the Tauri desktop crate
  arbitrageur-core        2,893   (data types, precision, ExchangeAdapter trait)
  arbitrageur-math       48,615   (115 modules)
  arbitrageur-exchanges  47,775   (28 modules, 30+ venue adapters)
  arbitrageur-risk        6,812
  arbitrageur-execution  22,625   (50 modules, ~40 of them conformal_*)
  arbitrageur-strategy   26,822   (52 modules)
  arbitrageur-app           884   (CLI + simulation harness)
  frontend/src-tauri     86,456   (33 modules; commands.rs alone 18,697)
  frontend/src            ~6,000  (React 19 + TS + Tailwind, ~90 components)

Tests: ~2,058 `#[test]`/`#[tokio::test]` functions in the working tree
  (crates ≈1,436; src-tauri 622). The 2026-10-02 audit ran 1,276 Rust tests + 415 frontend tests green.
Working tree: 74 modified files, +24,924 / −1,072 lines, never reviewed/built;
  181 untracked `.rs` under `crates/` (91 math, 44 execution, 43 strategy, 2 risk, 1 exchanges).
Untracked: 48 `.agents/teamwork/tranche*` dirs (~1.2 MB), 7 `libtest_tranche*.rlib` files, 2 zero-byte SQLite files.
.agents/: 271 files / 3.1 MB, of which 223 are committed into the product repo.
CI: 5 jobs — 1 permanently red (systemd-unit typo), the documented E2E suite not run at all.
```

---

## 3. Headline structural findings

### 3.1 H1 — Roughly half the workspace is unreferenced "phantom" code with fabricated verification

This is measured, not inferred. Method: for each module file, count references to `module::` anywhere in the
repo excluding the module's own file and its crate's `lib.rs`; then split "referenced" into
"referenced by non-test code" vs "referenced only from `#[cfg(test)]`".

**Finding A — the product consumes a small slice of the workspace.**

| Crate | Module files | Modules actually referenced by the desktop app |
|---|---|---|
| `arbitrageur-math` | 115 | **~20** (`pair_monitor`, `autotune`, `funding_carry`, `session`, `seasonality`, `forecast_calibration`, `fast_basis_harvest`, `dislocation`, `regime_switching`, `ou_process`, `fixed_yield`, `triangular`, `optimal_thresholds`, `kalman_filter`, `debiased_ou`, `calibration`, `book_gradient`, `black_scholes`, `binomial`, …) |
| `arbitrageur-strategy` | 52 | **4** (`convergence_auto`, `pair_trim`, `spatial_exit`, `options_dispersion`) |
| `arbitrageur-execution` | 50 | **5** (`virtual_ledger`, `coordinator`, `passive_unwind`, `emergency_unwind`, `smart_routing`) |

Total desktop-side references into `arbitrageur_math`: **79**. Into `arbitrageur_strategy`: **21**.

Concretely: **`arbitrageur-strategy` is 26.8 k lines and the product uses four modules.** `stat_arb_pairs`
(800 lines) and `cross_perp_skew` (598 lines) — both listed as headline strategies in `README.md` — are
referenced **only from the test module** of `strategy/lib.rs`. The real statistical-arbitrage logic lives in
`src-tauri/monitor.rs` (the "Spread Radar"), a *parallel implementation*. That is a duplicated-logic and
divergence hazard, and a documentation-truth problem.

The strategy review then confirmed this from the inside and made it precise. `strategy/lib.rs` contains
**no registry and no dispatch** — 102 lines of `pub mod`/`pub use` plus 4,948 lines of tests. Of the eight
"priority" strategies: **four wired** (`convergence_auto`, `pair_trim`, `spatial_exit`,
`options_dispersion::LiveVolSurfaceCache`), **two reach only the CLI demo** (`spatial_price_arb` 2,270 L and
`funding_rate_arb` 845 L, via `arbitrageur-app/src/main.rs:275-334`), and **two have zero consumers**
(`stat_arb_pairs` 800 L, `cross_perp_skew` 598 L). Of the crate's 52 modules, **40 are TEST-ONLY** — 12,852
lines, 105 built-in tests, **0** references outside their own file and `lib.rs`. A single-pass scan across
`arbitrageur-app`, `-core`, `-execution`, `-risk`, `-exchanges` and `frontend/src-tauri` found **68 of 114
`arbitrageur-math` modules (22,201 lines) with no consumer outside the math crate's own `lib.rs`**, with
names like `perverse_sheaves_arb`, `voevodsky_motives_arb`, `tannakian_frobenius_arb`,
`perfectoid_shimura_arb`, `padic_simpson_arb` and `geometric_langlands_arb`.

**Finding B — the test module swallowed the library files.**

| File | Total | Production | `#[cfg(test)]` |
|---|---|---|---|
| `crates/arbitrageur-strategy/src/lib.rs` | 5,050 | **~102** | **~4,948** |
| `crates/arbitrageur-execution/src/lib.rs` | 1,985 | ~98 | ~1,887 |
| `crates/arbitrageur-exchanges/src/lib.rs` | 767 | ~50 | ~717 |
| `frontend/src-tauri/src/commands.rs` | 18,697 | ~6,100 | **~12,600 (67%)** |

`strategy/lib.rs` is a 102-line registry followed by 4,900 lines of tests. The +4,663-line uncommitted change
to that file is therefore **almost entirely test code**. The "18.7 k-line god file" impression of `commands.rs`
is likewise two-thirds inline tests — the file is large because of *test-mass*, not prod logic overload. (Both
are still worth splitting, but the refactor is cheaper than the raw line count suggests.)

**Finding C — the phantom modules are "verified" by tests that only assert their own invented behaviour.**

There are **40 `test_tranche_N_integration` functions in `strategy/lib.rs`** (`:1388–5050`, ~3,662 lines)
— the file holds 62 tests in total, of which only 22 are real behaviour tests — plus **105 unit tests
inside the phantom modules themselves**. Because they are self-consistent, they pass. Example — the whole of
`crates/arbitrageur-math/src/susy_instanton.rs` (186 lines) implements a made-up "Kramers tunneling rate"
`Γ = ω·a√2/π · exp(−a²/(4bD))` with magic constants (15 s⁻¹ threshold, 0.40 OFI compression, 2 bps base shift),
is referenced **only** from `test_tranche_72_integration`, and its tests assert only that the formula returns
its own values. Its doc comment claims "simultaneous maker-quote evacuation in < 65 ms" — a performance claim
nothing measures.

**Finding D — the pattern has now spread into the production crates.**

The in-flight change to `crates/arbitrageur-risk/src/delta_guard.rs` adds **+1,909 lines** (Whalley–Wilmott
optimal re-hedging bandwidth, correlation-weighted portfolio delta with a hardcoded ρ=0.85 BTC/ETH, jump-bandwidth
contraction, SABR vol-surface provider). Every one of the new public entry points
(`calculate_correlated_portfolio_delta`, `compute_whalley_wilmott_bandwidth`, `evaluate_rehedge_trigger`,
`compute_multi_asset_whalley_wilmott_bandwidth`, `with_greek_limits`, `set_max_gamma`, `set_max_vega`,
`evaluate_jump_bandwidth_scaling`) has **zero references outside `delta_guard.rs`**. A repo-wide audit of the
247 public functions in the 13 most-changed files found **37 defined-but-never-referenced anywhere** and
**116 with ≤2 occurrences repo-wide**.

**Finding E — the phantom work is accompanied by fabricated audit documents.**

`.agents/teamwork/` holds **48 `tranche*` directories** (plus worker/reviewer dirs) containing reports such as
"*Landmark Milestone Audit Tranche 119: Categorical Geometric Langlands Duality, Conformal Rademacher PAC-Bayesian
Shields & Rough Fractional LSV-SABR Jump Deep Hedging*" — status "*100% Implemented, Audited & Verified Green*" —
for modules that are unreferenced. `tranche100`'s report derives "Heterotic E8×E8 superstring dualities & 11D
M-theory compactification" for crypto liquidity and claims `< 0.35 µs via AVX-256 SIMD` for code that is scalar.
These are not speculations about the future; they are present-tense false statements about what has been verified.

**Why this is the top finding.** The failure mode is not "extra code". It is that the repo's own evidence
channel — tests, audit reports, module docs — has been rendered untrustworthy. Any subsequent engineer or AI
agent that reads `.agents/teamwork/*` or trusts the test count will form a materially wrong model of what the
system does. **Recommended immediate action:** quarantine (do not delete) the phantom surface and the audit
reports, and add a short `docs/` note stating plainly which modules are live and which are not — before any
further feature work. See §8, Phase 0.

### 3.2 H2 — The uncommitted working tree is large, unreviewed, and unverified — and it is where most of the phantom code lives

74 tracked files / **+24,924 −1,072** lines sit uncommitted, plus **181 untracked `.rs` files under
`crates/`** (91 math, 44 execution, 43 strategy, 2 risk, 1 exchanges). The scale of the divergence is best
seen in the module registries:

| `lib.rs` | HEAD wires | Working tree wires |
|---|---|---|
| `arbitrageur-math` | 21 modules | **116** |
| `arbitrageur-execution` | 5 modules | **48** |
| `arbitrageur-strategy` | 8 modules | **51** |

**A fresh clone is not the developer's tree — HEAD and the working tree are two different products.**
Committing a working-tree `lib.rs` without its untracked sibling files simply breaks the build. The batch
is coherent (mostly the phantom-module + test additions, plus real-looking changes to `delta_guard`,
`smart_routing`, `passive_unwind`, `margin_monitor`, `deribit`, `evm_amm`, `commands.rs`, and the
options/funding bots). The 2026-10-02 audit verified **HEAD**; **none of this batch has been verified by
anything**, and at least two HIGH findings (`EX1-H1` the BSC/EIP-1559 regression, and the
funding-constant migration `TG-H6`) are regressions *introduced by this batch*. It must be treated as
unreviewed work, not as "the current system".

**This has a large positive implication for remediation.** Because the phantom surface is overwhelmingly
**uncommitted**, the cheapest correct fix for most of it is not a line-by-line deletion campaign but
`git checkout` plus a selective cherry-pick of the legitimate changes — provided the two regressions above
are fixed first. §8 Phase 3 is correspondingly cheaper than the raw line count suggests.

### 3.3 H3 — Repository hygiene is poor and actively misleading

- **7 stray `libtest_tranche143..149.rlib` build artifacts** sit in the repo root (untracked, ~550 KB) —
  residue of ad-hoc `rustc --test` runs of the phantom "tranches".
- **`arbitrageur.db` and `state.sqlite3` (0 bytes)** exist at the root (untracked); `.gitignore` does not
  obviously cover the real runtime DB (`~/.arbitrageur/state.sqlite3` is elsewhere — fine — but the root stubs
  invite confusion and would be committed by a careless `git add -A`).
- **119 untracked `.agents/teamwork/*` entries** churn `git status`; the tracked `subagents_manifest.json` has
  a 1,437-line diff.
- **Mixed line endings** (git prints CRLF warnings on nearly every file); no `.gitattributes`.
- `SPEC.md` is a 197 KB "merged spec" that is really an append-only session log (Session 40, 41, … 82) with
  average line length ~145 chars and no headings — it is effectively unreadable as a spec and will drift from
  reality silently.
- Full inventory in §4.12 (infra): no `.gitattributes`, `*.rlib` not in `.gitignore`, `.codegraph/` (30 MB)
  ignored only via `.git/info/exclude` (not portable), 223 of 271 `.agents/` files committed into the product
  repo, a `.gitleaks.toml` PEM allowlist broad enough to let a real private key through (`IN-H4`), `sha2`
  present at **three** major versions in `Cargo.lock`, and a shipped systemd unit that lacks the `MemoryMax`
  cap its own runbook documents (`IN-M2`).

### 3.4 H4 — Prior audit findings remain open, and the "profitability is unproven" verdict stands

`docs/SYSTEM_AUDIT_2026-10-02.md` is a good document. Its F-01…F-18 are **not** resolved by anything in this
review; several are reinforced (feed latency/fragility, mock-only E2E, prediction-market stubs, broad polling,
god-files). Its central judgment is worth restating because it should govern the next milestone:

> "The project's best trait is defensive correctness… The weakest trait is evidence of edge… the correct
> posture is: promising, guarded, but not production-proven."

This review adds no evidence of edge. It adds, instead, more *unverified surface*.

---

## 4. Findings register

Deduplicated across all reviewers. IDs are the originating reviewer's own (`TC` = tauri-commands,
`TS` = tauri-state, `TG` = tauri-strategies, `SEC` = tauri-security, `EX1`/`EX2` = exchanges-1/-2,
`XR` = execution-risk) so every row resolves directly in the companion raw report listed in §7.
Items marked **[dup]** were reported independently by two reviewers and merged.

### 4.1 Live-money control bypasses and guard gaps

- **`TC-C1` / `SEC-M1` [dup] — CRITICAL.** `dev_rpc.rs:16-24`, `main.rs:212`, `dev_rpc.rs:456-460`.
  Unauthenticated Unix-socket admin/trading RPC. The only gate is the env var
  `ARBITRAGEUR_DEV_SOCK` — there is **no `#[cfg(debug_assertions)]`**, no token, no peer-credential
  check, and no `chmod` (the rest of the codebase deliberately uses `from_mode(0o600/0o700)`:
  `state_store/mod.rs:2666,2676`, `stronghold.rs:123`). Exposed verbs include `execute_spatial`,
  `execute_spot`, `execute_funding_carry`, `execute_triangle`, `flatten_*`, `cancel_venue_order`,
  `set_env live`, `set_paused`, `reset_circuit_breaker`, and `unlock_vault` **accepting a plaintext
  password** (`dev_rpc.rs:92-96`). `AGENTS.md`'s own systemd example ships
  `ARBITRAGEUR_DEV_SOCK=/tmp/arb-dev.sock`, which is connectable by any local user on a shared host.
- **`TG-C1` — CRITICAL.** `options_bot.rs:738-751`. Autonomous perpetual rehedge places **real live
  IOC orders** with (a) no `state.execution_lock` — every other dispatch takes it (parity:672,
  packages:1204, triangle:790, funding:2279); (b) no ledger leg/signal row, so the reconciler has no
  order identity; (c) no `is_live`/vault/`live_entry_gate` check; (d) `bound_px = spot ± 0.2 %` where
  `spot` falls back to a **hardcoded 60000/3000/100** (`options_bot.rs:592-596, 702-708`) when the vol
  surface is stale. Executes whenever mode ≠ `off`, i.e. the normal operating modes.
- **`TS-C1` — CRITICAL.** `reconciliation.rs:1452, 1538-1547`. Options residual auto-flatten
  dispatches an unconstrained `OrderRequest::new_market`. `passive_cfg_for` was just changed to return
  `None` for **every** options strategy (`execution.rs:4426-4428`), so any options entry that lands
  `Imbalanced` is flattened at market — while `flatten_leg` deliberately refuses market for options
  citing an observed *"$14k fill on $10 OTM calls"* (`execution.rs:4570-4610`). Reachable from
  `reconciliation.rs:761` (`Imbalanced && !is_close_signal`), which is strategy-agnostic.
- **`TG-C2` — CRITICAL [SUSPECTED].** `options_packages.rs:1487-1499`. A native Deribit combo ack of
  `open`/`partially_filled` is dropped after a `tracing::warn!` **without inspecting
  `combo_resp.filled_qty`**, the combo order is **never polled**, and `!native_combo_executed` then
  re-dispatches **all** legs through the concurrent path. Deribit maps `open|partially_filled` →
  `New|PartiallyFilled` (`deribit.rs:722-733`, missing `order_state` defaults to `"open"`) and
  `supports_combo_orders()` returns true (`deribit.rs:1925`). The per-symbol `blocking_signals` guard
  only sees *ledger* signals, so the untracked combo legs are invisible.
- **`TC-H2` — HIGH.** `commands.rs:5660-5676`, awaited from `execution.rs:1935` while
  `_execution_guard` (`execution.rs:1067`) is still held. `refresh_equity_expectations_after_fills`
  loops venues **serially with no timeout** on `adapter.fetch_account().await`. The same diff gave the
  sibling `snapshot_venue_equity_for` a 3 s timeout (`commands.rs:5633`).
- **`XR-HIGH-1` — HIGH.** `emergency_unwind.rs:107`. Residual acceptance is
  `(residual_notional_usd < 5.0) OR (residual_qty < 1.0)`. The second term is **unit-blind**: a `0.4`
  residual on a 1-unit BTC contract (~$24k–60k) satisfies it and the unwind reports a clean
  `Ok(Filled)`. The unwind also builds its order from `unhedged_qty` without re-snapping to the wire
  grid, so reachability depends on coarse venue quantization.
- **`TG-H3` — HIGH.** `main.rs:105, 129, 137, 150`. Every runtime loop (`monitor.rs:2083`,
  `auto_exec.rs:1253`, `options_bot.rs:372`, funding/exit supervisor, `autotune.rs:949`, stablecoin FX)
  is spawned with **no `JoinError`/panic handler and no restart**. One panic permanently kills that bot
  for the process lifetime; only radar exposes `last_tick_ms`, so monitor/options/autotune deaths are
  silent apart from stop-orders never firing.
- **`TG-H4` — HIGH.** `auto_exec.rs:216-218` (doc) vs `convergence_auto.rs:357-368`. The diff
  **removed the live hard-block** from `evaluate_entry_gates` and replaced it with a 25 bps / 90 %
  p_win hurdle, while the module doc and `main.rs:123` still assert "testnet-only". Real dispatch
  (`monitor_ipc.rs:157` → `execute_monitor_opportunity_inner`) does not re-block live. `options_bot`
  still hard-refuses `auto && is_live`, so the two bots now have **opposite live postures**.
- **`XR-MEDIUM-2` — MEDIUM.** `risk/safety_guards.rs:136, 147, 175`.
  `check_divergence_catastrophic_stop`, `check_time_decay_exit` and
  `evaluate_venue_rebalance_needs` have **no non-test callers**. Only `verify_pre_execution_slippage`
  is wired (`execution.rs:1370`, `commands.rs:849`, `funding.rs:2369`). The documented de-peg /
  stalled-convergence guards are dead.

### 4.2 Environment-isolation violations

- **`TS-H1` — HIGH.** `execution.rs:3143-3148`, `:5613-5642`; `state_store/signals.rs:785-819`.
  `list_signals(status: Option<SignalStatus>)` has **no environment parameter at all**, so
  `get_open_positions_impl` and `flatten_positions_inner`'s `roots` selection aggregate testnet and
  live signals together. Venues are then resolved through the env-scoped `state.get_adapter`, so in
  live mode a leftover **testnet** signal drives reduce-only orders against the **live** account. The
  residual count at `:6385-6386` *was* explicitly made env-aware — the roots selection above it was
  not, which looks like an oversight rather than intent.
- **`TS-H2` — HIGH.** `execution.rs:4152-4163, 4208`. Same root cause on the killswitch path:
  `engine_covered_pairs`, `engine_strategies` and the per-strategy flatten loop see the other
  environment's signals, contaminating the engine-vs-external dedup at `:4243`.
- **`EX1-M2` — MEDIUM.** `decibel.rs:1188`. `ping()` hardcodes the **mainnet** Aptos fullnode even for
  a `for_testnet` adapter, although `fullnode_url()` (`:876-881`) already branches correctly. A
  mainnet outage falsely marks the testnet venue unhealthy.
- **`EX2-M9` — MEDIUM.** `arbitrageur-app/src/main.rs:540-541`. Polymarket/Kalshi adapters exist only
  in the CLI; the Tauri venue surfaces list them while `get_adapter("polymarket"/"kalshi")` → `None`.
- **`SEC-CROSS` — design note, not a defect.** `vault.rs:278-280` + `state.rs:2883-2886`:
  **auto-lock is not a security boundary.** After an idle auto-lock, `allows_adapter_execution()` is
  still true, credentials are not cleared, and live entry/exit remain permitted (`execution.rs:3505`,
  `:4049` only refuse when *initialised && locked && not auto*). Only a **manual** lock is a
  panic-halt — and manual lock is itself degraded by `SEC-H1`/`C-5`. This must be stated operator-facing.

### 4.3 Fail-open and silent-default defects

- **`SEC-H1` — HIGH by the reviewer, escalated to CRITICAL in §0 — `C-5`.** `accounts.rs:159-176`.
  `if let Ok(signals) = ...list_open_signals(...)`: an `Err` (DB busy / corrupt / IO) skips the
  open-position check entirely, after which the function proceeds to `vault.lock()` and
  `sync_vault_credentials_to_adapters()` (`state.rs:2883-2888`; non-auto-lock →
  `clear_adapter_credentials()`). Every sibling path in the same file is `.map_err(...)?`
  (`accounts.rs:377, 869, 1451`). **Escalation rationale:** the reviewer rated HIGH on the grounds
  that it needs a concurrent DB error; the lead rates CRITICAL because the *consequence* is live
  positions left with no exit supervision plus a wiped AMM signer — an unrecoverable state until the
  operator unlocks again.
- **`TS-M2` — MEDIUM.** `state_store/accounts.rs:12-21`. The private `normalize_environment(None)`
  returns `Ok("live")`, so an `upsert_account` caller that omits `environment` silently creates a
  **live** credential profile. The `pub(crate)` twin at `mod.rs:2452` correctly rejects anything but
  `testnet|live`.
- **`EX1-M8` — MEDIUM.** `dex_perps.rs:3613-3634`. `position_from` uses
  `entry.unwrap_or_default()` / `mark.unwrap_or_default()`, so a venue row missing `entryPrice` /
  `markPrice` yields `0` — poisoning PnL and any downstream delta/liquidation sizing with a fail-open
  zero instead of "undetermined".
- **`XR-LOW-1` — LOW.** `position.rs:43`. `liquidation_price == Some(0)` computes distance
  `(mark-0)/mark = 1.0` → `MarginMonitor::evaluate_position` returns Healthy at 100 %. `None` is
  correctly Unknown, but `Some(0)` is the common "no liquidation" sentinel and is read as maximally safe.
- **`SEC-M3` — MEDIUM.** `accounts.rs:1119-1133`, `:1517-1527`. When `store_key` fails, or when the
  `.hold` file is corrupt so `open_evm_stronghold` sets `evm_stronghold = None`
  (`state.rs:2860-2863`), the **raw hex EVM private key is written into the vault record** — where
  `get_account_credential_field("privateKey")` can export it, since the reveal guard only blocks values
  carrying the external-signer sentinel. The module comment claims non-exportable custody.
- **`TC-L1` — LOW.** `num.rs:19-21`. `dec_f64` returns `0.0` on parse failure and is used in
  size/price math (`external_positions.rs:333,351,568,785…`); a 0 price walks into dust logic.
- **Capability-honesty cluster — MEDIUM.** `EX2-M1` `okx.rs:1220-1230` `has_live_credentials()`
  ignores the passphrase although `credentials()` and every live path gate on all three;
  `EX2-M8` `ibkr.rs:1034-1036` returns `account_id.is_some()` with no session probe;
  `EX1-M11` `dex_perps.rs:5963-5975` Helix returns `true` when `wallet_address` is `None`, unlike
  paradex/evedex; `EX2-M8`. All fail closed at execute time, but the readiness flag lies.
- **`EX1-M6` — MEDIUM.** `simulated.rs:601-611` is **not** `#[cfg(test)]` (`lib.rs:22`) and
  unconditionally returns `true` for `supports_live_execution()`, `live_order_submission_implemented()`
  and `has_live_credentials()`; its `place_order` fabricates fills. Only registry discipline
  (`state.rs:1568` plus the `production_adapter_resolution_never_yields_a_simulation` test) keeps it
  out of live. `AGENTS.md`'s "sims exist only under `#[cfg(test)]`" is not literally true.

### 4.4 Order lifecycle, identity and settlement correctness

- **`EX2-H1` — HIGH.** `okx.rs:1159-1161`, `kraken.rs:429-431`, `boros.rs:1160-1162` vs
  `coordinator.rs:205-219`. These three adapters' `fetch_order` return
  `client_order_id: String::new()` and ignore the id passed in, while
  `response_identity_matches` requires a **non-empty, matching** client id. `settle_working_leg`
  therefore never adopts venue truth on these venues — `Ok(_) => return latest` (`coordinator.rs:73`)
  hands back the original submit ack. In the Leg-B-failed path (`coordinator.rs:1056-1073`) a leg that
  actually filled is reported `UnresolvedExposure` with `filled_qty == 0`, so the sibling unwind is
  skipped. Binance (`binance.rs:1045-1049`) and Vanta (`vanta.rs:1229-1233`) do it correctly — the
  pattern is expected, not intentional.
- **`EX1-H2` — HIGH.** `hyperliquid.rs:1259` vs `:775-792`. `place_order` sends
  `hl_wire_decimal(quantity)` truncated at `sz_decimals`, but `parse_hl_order_response` compares
  `totalSz` against the **untruncated** `order.quantity`. A full fill on an unrounded qty (0.12345 →
  wire 0.123) is reported `PartiallyFilled` with a phantom `remaining_qty = 0.00045`.
- **`EX1-H3` — HIGH [SUSPECTED].** `hyperliquid.rs:1321`. `nonce = Utc::now().timestamp_millis()`;
  the adapter holds **no order lock** (fields `:275-290`, `post_exchange` `:469`). Two submits in the
  same millisecond produce identical nonces and Hyperliquid requires strictly-increasing ones. Lighter,
  Decibel and PopDEX all guard this.
- **`XR-MEDIUM-3` — MEDIUM.** `coordinator.rs:1268` + rollback `:1302`. `settle_working_leg(...,
  cancel_partial=false)` at `:1255` leaves a non-terminal leg working; the rollback unwinds only
  `fill.filled_qty` and the remainder's identity is never surfaced (`BothFailed` carries text only).
  Mitigated today only because `execute_plan` is itself test-only — but `pub use coordinator::*` exports it.
- **`XR-MEDIUM-4` — MEDIUM.** `coordinator.rs:727-730`. `is_trimmable_partial` (`:261`) accepts
  `PartiallyFilled`, which `core/types.rs:123` excludes from `is_final()`. After trimming the sibling's
  excess (`:809`) the still-working leg's remainder rests; a later fill produces net unhedged exposure
  while the result reports only the matched quantity. Entry paths are IOC today, so venue remainder
  auto-cancels — the primitive is generic and would break for any GTC entry.
- **`TS-L1` — LOW.** `reconciliation.rs:133-137`. `signal_age_secs` returns `0` on an unparseable
  `created_at`, pinning a corrupt row **below every deadline** (permanent stall instead of escalation).
- **`TS-L4` — LOW.** `reconciliation.rs:509-526`. A non-USD-stable fee currency rejects the whole poll
  as `Unknown`, which can keep a fully-filled order on the escalation path forever.
- **`XR-LOW-2` — LOW.** `reconciliation.rs:1518`. `PassiveOutcome` is logged but not branched on
  (`Unresolved`, and `MarketFallback`-whose-market-order-errored); recovery relies on the retry loop.
- **`XR-LOW-3` — LOW.** `passive_unwind.rs:1447`. `combine_live_attempts` fabricates
  `status: New, filled_qty: 0` when there were no attempts and the fallback errored.
- **`EX1-M5` — MEDIUM (in-flight diff).** `deribit.rs:1713-1730`. Adding `|| is_ioc` to the collar
  clamp means an IOC Buy whose collar sits **above** its limit (after `price_too_low`) is retried *at
  the collar* — above the authorized cap; symmetrically an IOC Sell can be clamped below its limit. The
  `reduce_only` relaxation is deliberate (closes must execute); the plain-IOC one silently widens
  authorized price.
- **`EX1-M10` — MEDIUM [SUSPECTED].** `popdex.rs:1044-1053, 1105`. The IOC branch deliberately encodes
  a worst-price bound, but true `Market` orders send `price = 0` **and** `slippage = 0`. If the
  precompile reads 0 as unbounded, emergency market exits have no price protection. ABI semantics need
  confirmation.
- **`EX1-L4`, `EX1-L5`, `EX1-L6` — LOW.** `deribit.rs:1836-1839` maps an absent `direction` to
  `Side::Buy`; `decibel.rs:1156-1172` infers `Filled` from `remaining<=0 && filled>0` (guarded, acceptable);
  `ws.rs:220-240` hardcodes a mainnet `market_id → symbol` map (guarded to mainnet hosts by
  `feed.rs:511-541`, but can drift).

### 4.5 Dead, unwired and mis-advertised safety machinery

- **`XR-HIGH-2` — HIGH.** `execution/src/lib.rs:3` + all `conformal_*.rs`. **32 modules / 10,690 lines
  = 47 % of the execution crate** implement fail-closed risk verdicts (`evaluate_shield`,
  `should_evacuate_passive_quotes`, …) with **zero production callers**: all ~160 references are inside
  the strategy crate's `#[cfg(test)]` block, which starts at line 103. Nothing in `coordinator.rs`,
  `passive_unwind.rs`, `smart_routing.rs` or `frontend/src-tauri` refers to them.
  `conformal_spectral_risk_shield.rs` is not declared in `lib.rs` at all (32 files on disk, 31 `pub mod`)
  — it is never compiled. Four modules were sampled and are **genuine** numerical code (real Adadelta
  recurrence, Bernstein concentration radii, conformal quantile factors) with correct-looking
  fail-closed verdicts. This is *orphaned competence*, not filler — which is worse, because it looks
  production-ready.
- **`XR-MEDIUM-1` — MEDIUM.** Roughly **3.3 K further unreachable lines**: `atomic_bundle` (765;
  reachable only through `smart_routing`'s `with_atomic_bundle_simulator`, an Option that is always
  `None`), `causal_dml_impact`, `mcafee_combinatorial_auction`, `mean_field_liquidation`,
  `modular_session_account`, `omd_dual_averaging`, `sac_execution`, `viscoelastic_cascade_guard`,
  `zk_intent_settlement` (all test-only via `strategy/lib.rs` tests), plus `shared_sequencer_bundle` and
  `risk/risk_parity_allocator` (no references at all). Combined with the conformal family:
  **≈14,000 unreachable lines — 62 % of the execution crate.**
- **`XR-MEDIUM-7` — MEDIUM.** `passive_unwind.rs:680`. `execute_passive_reduce` (the paper driver) has
  **no callers**; production uses only `execute_passive_reduce_live` (`reconciliation.rs:1507`,
  `execution.rs:4574`, `external_positions.rs:968`). Two parallel implementations maintained in lockstep.
- **`TC-M6` — MEDIUM.** `commands.rs:2794-2800`; `execution.rs:3243/3266`.
  `scan_prediction_opportunities_impl` discards its inputs and returns `Ok(Vec::new())`;
  `execute_prediction_arbitrage` / `execute_cross_ticker_arbitrage` return `Err` unconditionally. All
  are registered and consumed by UI panels (PredictionHub) — panels that look live and are permanently empty.
- **`TC-M5` — MEDIUM.** `commands.rs:18670-18697` + `main.rs:358-359`.
  `prepare_across_bridge_deposit` / `quote_across_bridge_transfer` are registered with **zero frontend
  call sites**; the former validates nothing (`gross_profit_usd.unwrap_or(100.0)` reaches
  `compute_priority_fee`, NaN included) and returns `calldata: Vec<u8>` as a JSON number array.
- **`EX1-M1` — MEDIUM.** `dex_perps.rs:1174-1210`. `get_json` *reads* the cache by exact path
  (`acquire_or_cache_path(path, true)`) but *records* with `record_success(text)`, which writes to the
  `"default"` bucket (`rate_limiter.rs:239-241`). No path key is ever populated, so the per-path cache
  **never hits** and every markets/book/funding poll on 26 long-tail venues goes to network.
  hyperliquid/lighter/decibel/popdex all use `record_success_path`.
- **`EX1-M7` — MEDIUM.** `decibel.rs:1198-1216`. `fetch_markets` is a hardcoded 7-symbol list with
  invented `price_decimals`/`size_decimals`/`min_order_size`. Order scaling is safe (`place_order`
  re-reads `fetch_market_row`, `:1558-1594`), but symbol discovery, min-size pre-checks and the UI see
  fabricated metadata.
- **`EX2-M2` — MEDIUM.** `robinhood.rs:13-34`. A second adapter for the `robinhood` venue wraps
  `LighterAdapter` but does **not** override `rest_base_url()`, `ws_account_id()`, `wire_base_qty()` or
  `live_order_reconciliation_implemented()`, so it inherits the fail-closed trait defaults and loses the
  correct WS base. The live Tauri product constructs `LighterAdapter::new_with_venue(..., "robinhood")`
  directly; only the CLI uses the wrapper. `docs/archive/HANDOFF_MASTER_SESSION_HISTORY.md:327` says it
  was deleted; `ARCHITECTURE.md:59` still lists it.
- **`EX2-L2` — LOW.** `arcus.rs:1124-1131`. `acquire_or_cache_path(path, false)` can never return
  `Some` (both `Some` returns are gated on `allow_cache`, `rate_limiter.rs:79-89, 124-130`), so the
  "unexpectedly served from cache" guard is unreachable.

### 4.6 Economic constants, accounting and modelling defects

- **`TG-H6` — HIGH.** `funding.rs:625-636`; `monitor.rs:3405, 3444`. Three live economic constants
  moved at once: entry min APR **8 → 18 %**, max entry spread **50 → 8 bps** (`funding.rs:2407-2417`),
  rotation hurdle `max(25 % APR)`, exit hysteresis `max(8 %)`. Worse, the values are expressed as
  `#[cfg(test)] { 15.0 } #[cfg(not(test))] { 18.0 }` — **CI exercises different numbers than
  production**, so the test suite structurally cannot catch a bad live value.
- **`TG-H5` — HIGH.** `autotune.rs:592-654, 872-945`. `apply_ready_to_ticker` (`:575-590`) applies
  `targetProfitBps`, `windowPercentile` and `userTargetLeverage` without the per-field range guards that
  `apply_ready_to_strategy` has, and saves through `save_ticker_strategy_config_impl` — a different,
  unvalidated path. Separately, `run_scheduled_autotune` has **no** `is_paused` / `is_tripped()` check,
  so a halted engine still has its live strategy params rewritten every 15 m / 6 h.
- **`TS-H3` — HIGH.** `state.rs:2490-2535`; `state_store/logs.rs:27-30`. The diff renamed the guard key
  `pnl.premium_outlay.migrated_v2` → `..._v3`, forcing a fresh recompute on **every existing install**.
  The recompute overwrites `pnl.total.{testnet,live}` with the sum of `load_execution_logs(100_000)` —
  but that table is pruned to the newest **2000 rows on every insert**, so lifetime realized PnL is
  permanently replaced by a recency window (the write is unconditional, not additive). `pnl.total.*`
  also feeds the drawdown-breaker baseline, so this is not cosmetic.
- **`TG-H1` — HIGH.** `options_packages.rs:1474-1481`; `deribit.rs:717-720, 744`. One filled combo ack
  is cloned into **one fill per leg** with `avg_fill_price = options_leg_bound_px(...)` — the *limit*
  price, not the venue execution price (the venue returns a net combo price which is never
  distributed). The Deribit combo response also hard-codes `fee: Some(dec!(0))`, so **no combo entry
  commission is ever recorded** and `fee_total` is 0, overstating realized Net P&L. That same
  hard-coded zero is what keeps the draft's "4× fee multi-count" **latent rather than active**.
- **`TG-M6` — MEDIUM.** `funding.rs:1338-1343`. Spot-perp carry entry gates on a hardcoded
  `dec!(0.08)` borrow APR identical for every asset and venue; a real 15–20 % borrow book still passes.
- **`TG-M1` — MEDIUM.** `monitor.rs:728-736`. `fast_basis_harvest` detects a spot leg with
  `venue.to_ascii_lowercase().contains("spot")`, but venue ids here are
  `hyperliquid|lighter|binance|coinbase|okx|vanta…` — **never matches**, so `borrow_cost_usd` is always
  0. The correct predicate already exists one screen up (`is_spot_or_equity_leg`, `:546-563`, `-SPOT`
  aware). Also `funding_leg_rate_hourly(...).unwrap_or(0.0)` (`:770-776`) substitutes a synthetic 1 bps
  when the EMA is empty.
- **`TG-M2` — MEDIUM.** `monitor.rs:800-816`. A brand-new autonomous stop-loss path flattens
  funding/basis positions on hardcoded 15/25 bps Leung–Li barriers that no operator can tune and no
  test exercises at live values.
- **`TG-M3` — MEDIUM.** `options_packages.rs:477-479`. The diff flipped the jelly-roll implied rate to
  `(near_pv / far_pv).ln() / (t2 - t1)` while the comment directly above still documents
  `(far_pv / near_pv)`; the box-spread sibling (`:349`, `ln(strike_gap/usd_val)`) is positive. The
  reported `implied_rate_pct` is now sign-inverted (display-only, not gating).
- **`TC-M1` — MEDIUM.** The new gas-aware evaluator (`commands.rs:683-788`) is wired into scan at
  `:2237, :2543`, but the **execution** profit floor still calls `execution.rs:1491`
  `compute_route_economics_for_mode` with gas = 0. The UI can show a route as unexecutable while a
  manual Execute passes the floor.
- **`TC-M2` — MEDIUM.** `commands.rs:740-751`. `two_way = true` pays on-chain gas on both entry and
  exit, but `route_gas_usd = venue_estimated_gas_cost_usd(buy_id) + venue_estimated_gas_cost_usd(sell_id)`
  sums each leg once → ~1× systematic understatement for bidirectional EVM routes.
- **`TC-M4` — MEDIUM.** The new `effective_venue_spot_fee_bps` (`commands.rs:582-596`) is used **only**
  by `funding.rs:1267`; `scan_spot_opportunities_impl` still uses the futures table
  (`commands.rs:2510`), so spot net edge is computed against the wrong fee schedule.
- **`TS-M1` — MEDIUM.** `state_store/samples.rs:742-756`. `funding_rate_series` adds
  `OR (INSTR(?2,'-') > 0 AND symbol LIKE SUBSTR(?2,1,INSTR(?2,'-')-1) || '-%')`, so `BTC-USDC` also
  matches `BTC-USDT` / `BTC-USD` / `BTC-PERP` / date-coded futures; then a **bare** `GROUP BY ts_ms`
  with un-aggregated columns makes SQLite return one arbitrary member per group. The returned series can
  silently splice different instruments — and it is the stated training substrate for
  funding-persistence / half-life modelling.
- **`TS-M6` — MEDIUM.** `state_store/mod.rs:782-820` (diff). `aggregate_open_positions` replaces
  mark-to-market package PnL with `expected_sum * (elapsed/duration).clamp(0,1)`, **unconditionally on
  environment**. The testnet rationale is documented, but any live consumer of
  `OpenPositionDto.unrealized_pnl_usd` now sees a synthesized number.
- **`TS-M8` — MEDIUM.** `state.rs:2542-2550`; `state_store/logs.rs:38-42`. The `sim_step_counter`
  collision floor is `execution_log_count() + 8`, but that count can never exceed 2000 because the table
  is pruned on every insert — so the documented protection is void. Floor from `MAX(id)` over
  `cycle_lots` instead.
- **`TG-M4` — MEDIUM.** `monitor.rs:3164` called from `poll_funding`, vs `auto_exec.rs:884-894`.
  `trip()` (`circuit_breaker.rs:182-209`) does **not** set `is_paused` (only the manual pause and the
  emergency killswitch do), so on an automatic trip the monitor keeps running funding exits while
  radar/options self-retire to `off` and stop supervising their own positions. Inconsistent halt
  semantics, contradicting PROJECT.md R2.1's "breaker trip freezes dispatch".
- **`TG-M5` — MEDIUM.** `options_bot.rs:423-458`, gated by `tick_substrategy` at `:814` (which returns
  early at `!mode.exits_allowed()`). A paused engine calls `stop_all_bots`, which writes mode `off`, so
  the parity perp-hedge auto-unwind at option expiry and the packages 88 %-edge / pin-risk harvest are
  **skipped** → a paused engine can hold a parity trade through expiry with the perp leg open (naked
  directional delta). "Pause = positions preserved" removes a safety unwind here.
- **`TG-M7` — MEDIUM.** `monitor.rs:1131-1137, 1187-1198`. `PAIR_OU_SOFT_MIN_R2 = 0.20` with shrinkage
  toward 0.50 means a low-quality pair can now yield a non-`None` `p_win` instead of being rejected (the
  previous hard floor was `PAIR_OU_MIN_R2 = 0.6`). Shrinkage is a defensible stabiliser; the effect is
  that more pairs reach the qualification pipeline.
- **`TG-M8` — MEDIUM.** `options_packages.rs:1680-1730`. Package legs moved from sequential (breaking
  on first failure) to `futures::future::join_all` **concurrent** dispatch on the same venue, widening
  the partial-fill window and increasing simultaneous margin/market impact. Partly mitigated by the new
  emergency unwind (`:1804-1860`), which can itself fail and leave `Imbalanced`.
- **`EX1-M9` — MEDIUM [SUSPECTED].** `hyperliquid.rs:1085-1121`. `clearinghouseState.accountValue`
  (perp) plus the spot wallet's USDC `total` are summed into both `account_value` and `free_margin`. If
  USDC collateral overlaps between the two views, equity and free margin are overstated → sizing reads
  more margin than exists. Needs venue confirmation of the accounting model.
- **`EX2-M3` — MEDIUM.** `binance.rs:935-936`. USDT-M orders are built without a `positionSide` field
  and no `/fapi/v1/positionSide/dual` probe is performed. In **Hedge Mode** Binance rejects every order,
  including risk-reducing `reduceOnly` closes — a close intent silently cannot execute.
- **`EX2-M4` — MEDIUM.** `binance.rs:116-145` vs `:911-921`. `symbol_filters` returns only
  `(tick, step, minQty)`; MIN_NOTIONAL is parsed into `MarketMetadata.min_notional` (`:637-641`) but
  never enforced before submit → avoidable venue rejects mid-execution, which can fail a leg and trigger
  an unwind of the sibling.
- **`EX2-M7` — MEDIUM.** `okx.rs:1186-1190`. `set_leverage` hardcodes `"mgnMode": "isolated"` while
  orders send `tdMode: self.td_mode`, so `with_td_mode("cross")` is half-honoured.
- **`EX2-M6` — MEDIUM.** `across_bridge.rs:114-119, 252-255`. USD money math in `f64` where the rest of
  the crate uses `Decimal`; the relayer fee comes from a local simulation
  (`arbitrageur_math::intent_bridge::CrossChainIntentBridgeEngine`, `:140-157`) rather than a live
  relayer quote, so the fee may diverge from the real quote at fill time.
- **`TG-L1` — LOW.** `options_parity.rs:1307-1308`. `realized_profit_usd` / `total_cumulative_pnl_usd`
  carry premium-cashflow / premium-outlay semantics. The accounting is correct per R4.4; the wire names
  mislead.
- **`TG-L2` — LOW.** `triangle.rs:424`. `triangle_target_profit_bps` default silently raised 2.0 → 4.0
  with no operator-facing note.
- **`TG-L4` — LOW.** `funding.rs:925-949`. When a venue omits `predicted_rate`, a TWAP forecaster is
  invoked with **no history samples** and its output feeds `agrees_hold` as if it were a venue prediction.
- **`EX2-L4` — LOW.** `okx.rs:853`. `fetch_funding_rate` hardcodes `interval_hours: 8` although OKX
  lists 1 h / 4 h / 8 h funding on some swaps.
- **`XR-LOW-4` — LOW.** `delta_guard.rs:355,886,1116`; `collateral_rebalance.rs:1617,1623`.
  `Decimal::from_f64_retain` diverges from the codebase's string-round-trip convention
  (`dec_to_f64_opt`/`f64_to_dec_opt`) and can introduce binary-expansion dust.
- **`EX1-L2`, `EX1-L3` — LOW.** `hyperliquid.rs:566` derives significant-figure count via `f64 log10`
  where exactness drives a price cap; `dex_perps.rs:6689-6701` parses qty/price through `f64`, so float
  noise (`0.30000000000000004`) can reach the wire.

### 4.7 Security, crypto and trust boundary

- **`SEC-H2` — HIGH.** `ws_primary.rs:126-161`. `OrderbookSingleFlight`'s leader branch
  (`if am_leader { let outcome = adapter.fetch_orderbook(symbol).await; ... }`) sits between inserting
  into `self.slots` and writing `slot.result` + `notify_waiters()` with **no RAII guard**. If the leader
  future is dropped (caller disconnect / task cancellation) or panics, `slot.result` stays `None`,
  nobody is notified, and the entry is never removed — so **every subsequent `fetch` for that
  (venue, symbol) takes the waiter branch and hangs forever**, the REST fallback is permanently dead for
  that symbol, and each cancelled leader leaks a HashMap entry.
- **`SEC-H3` ≡ `TS-H4` [dup, cross-confirmed by two reviewers] — HIGH.** `execution.rs:3453-3466`
  (plus `:3419-3420`, `:3428-3433`), `state.rs:3435-3439`, `circuit_breaker.rs:296-314`. The breaker
  baseline is re-anchored **only on env switch** (`last_risk_live_universe != Some(is_live)`), never when
  the **admitted account set** changes. Refined trigger: only when `settings.<venue>_enabled = false`
  **and** its enabled default account is disabled does the venue drop out of `venues_for_env`
  (`state.rs:3435-3439`) — a `true` flag with a disabled account merely becomes `NO_CREDENTIALS` and
  stays in the set. When it does drop out, `map.retain(|k,_| admitted.contains(k))` removes its equity
  bucket and the total falls; `update_equity` trips on `dollar_loss >= $1000` **or** `loss_pct >= 5 %`,
  so disabling a >5 %-of-equity closed account (a legitimate, allowed action —
  `accounts.rs:864-878`) is read as a same-day loss → breaker trip → all new entries blocked, bots
  included, with no automatic recovery. Symmetric case: the first successful on-ramp of an
  equity-bearing venue raises the total without re-baselining. The very first feed is safe because
  `last_risk_live_universe = None` forces a re-anchor; the bug is on subsequent same-environment feeds.
  Fix: record the previous `admitted` set, and on any change either `reanchor_baseline(total)` or shift
  `day_start_equity` by the delta.
- **`SEC-M2` — MEDIUM.** `alerting.rs:61-65`. The Slack/Discord/ntfy incoming-webhook URL **is itself
  a credential** (it embeds a secret path/token) and is printed in full by `tracing::info!` into journald.
- **`SEC-M4` — MEDIUM.** `totp.rs:89-104`; `accounts.rs:80-84, 110-118`. ① No used-step record, so a
  code replays within the ±1-step (90 s) window; ② `code_at(...) == code` is a short-circuiting,
  non-constant-time comparison; ③ `vault_unlock_backoff` is in-memory only with a 30 s ceiling, so a
  process restart clears the online brute-force throttle.
- **`SEC-M5` — MEDIUM.** `vault.rs:23, 483-485, 672-674`. `MIN_PASSWORD_CHARS = 6` with no strength
  floor. Argon2id(64 MiB, t=3) is strong, but an offline copy of `vault.enc` falls to a 6-character
  password, exposing every credential and the MFA seed.
- **`SEC-M6` — MEDIUM.** `priceretriver.rs:24-41, 59-71`; `vendor/priceretriver-client/src/lib.rs:133-138,
  165-170`. `dest_path` goes straight to `Path::new` → `File::create` with **no directory constraint**;
  `node_url` is arbitrary (loopback / cloud metadata reachable); `run_custom_script` ships `script_code`
  to the remote for execution. `capabilities/default.json` grants only `core:default`, so every
  `#[tauri::command]` is webview-callable; the strict CSP (`script-src 'self'`) is the only thing
  holding this boundary.
- **`SEC-M7` — MEDIUM.** `strategy_config.rs:27-59`. `save_ticker_strategy_config_impl` checks
  `target_profit_bps` finiteness but **omits `validate_market_symbol`** (which the preset path at `:425`
  and `commands.rs:2673` do apply) and sets no upper bounds on `unit_size` /
  `max_accumulated_units` / `target_profit_bps`. `config.symbol` is interpolated into a KV key
  (`:54`) and into the in-memory map.
- **`SEC-M8` — MEDIUM.** `feed.rs:419-430, 574-592, 713-731`. `ws_connect` has a 10 s handshake
  timeout, but the post-connect `read.next()` is **not** wrapped in a timeout and tungstenite does not
  error on missing pongs. On a half-open TCP path (NAT / silent edge drop) keepalive Pings succeed,
  no inbound frame ever arrives, and the loop blocks forever: `phase` stays `connected`,
  `last_ended_ms` never updates, the lane never reconnects. **The supervision surface reports healthy
  while the feed is dead**, and reads silently fall back to full REST — precisely the Lighter-WAF
  trigger that `feed.rs:126-134` warns about. (Data itself is not silently used: the hub's 400 ms
  `max_age` correctly returns `None`.) Two adjacent defects: Lighter's subscribe loop omits the
  `SUB_BURST_PACE_MS` pacing that hyperliquid/vanta have (`feed.rs:547-550` vs `:404-412`, `:702-706`),
  contradicting the file's own "instant floods get the connection RST" note; and `feed.rs:499-502`
  writes `.timeout()` twice, so 5 s silently overrides 15 s.
- **`TC-H1` ≡ `SEC-M9` [dup] — HIGH.** `commands.rs:18592-18625`, injection point `:18617-18620`.
  `open_external_url` validates only the **host** against `["popdex.xyz", "crypto-chief.com"]` and then
  passes the whole URL to `cmd /c start "" <url>` on Windows. Rust does not quote an argument that
  contains no spaces, so `https://popdex.xyz/a&calc.exe` passes host validation and `&` becomes a
  command separator. Reachability is low today — the only call site passes a literal constant
  (`ExchangeAccounts.tsx:900`) and CSP is `script-src 'self'` — but it is a genuine
  command-execution sink that fails the moment any frontend URL is influenced.
- **`EX2-M5` — MEDIUM.** `ibkr.rs:54-60`. `danger_accept_invalid_certs(true)` is set **unconditionally**
  on the shared client. Defensible for the local self-signed Client Portal gateway (the default base
  URL), but the same client is used when `base_url` is overridden to a remote host, where it enables
  MITM of signed order traffic.
- **`TC-H4` — HIGH.** `dev_rpc.rs:48-58` (plus `dev_rpc.rs:30`). `BufReader::lines()` / `next_line()`
  has no frame-length cap and there is no per-connection limit (one `spawn` per connection) → a single
  oversized JSON line exhausts memory/CPU. Compounded by `TC-C1` (no auth) and `TC-L4`.
- **`TC-M3` — MEDIUM.** `commands.rs:3163`. `reset_transaction_history_impl` calls
  `state.circuit_breaker.reset()` plus `*state.last_risk_live_universe.write() = None` as an unadvertised
  side effect → clearing trade history silently lifts a daily-loss breaker and discards the trip reason
  and consecutive-error count.
- **`TC-M7` — MEDIUM.** `commands.rs:2811-2813`. `run_stat_arb_step_impl` takes an arbitrary
  caller-supplied price and calls `update()` on the process-global shared `state.kalman_filter`, poisoning
  the stat-arb hedge-ratio / residual baseline if that state participates in live decisions.
- **`TC-M8` — MEDIUM.** `commands.rs:1131`. `get_system_status` clones the entire `EngineSettingsDto` —
  which carries `hyperliquid_private_key` and friends (`state.rs:1258` `clear_credential_material`) —
  roughly every 3 s and never zeroizes the temporary.
- **`SEC-L1` — LOW.** `vault.rs:392-424, 627-637`. A random 96-bit nonce is reused under the same
  `(key, salt)` on every `save_data`. Safe at human write rates (the birthday bound needs ≪ 2^32
  messages); worth documenting, or moving to a counter nonce if rewrites ever become frequent.
- **`SEC-L2` — LOW.** `vault.rs:437-441, 466-472`; `stronghold.rs:120-148`. The 0700/0600 hardening is
  `#[cfg(unix)]` only; Windows falls back to default ACLs. Matches SECURITY.md, but no explicit DACL.
- **`SEC-L5` — LOW.** `stronghold.rs:93-95, 175-177`. `password.to_vec()` makes a non-zeroized copy;
  `has_key` maps errors to `false` (fail-closed, acceptable).
- **`SEC-L6` — LOW.** `alerting.rs:30-39, 142-155`. Plaintext `http://` webhooks are accepted (credentials
  and alerts in the clear); `THROTTLES` is an unbounded HashMap (currently keyed on bounded values).
- **`SEC-L4` — LOW.** `hub.rs:183-197, 97`. `MarketDataHub` never evicts stale slots (bounded by the
  venue × symbol universe; read-side `max_age` rejection is correct) and discards `broadcast` `send`
  errors.

### 4.8 Availability, resource and lifecycle

- **`TC-H3` — HIGH.** `commands.rs:1969-1984`; `enforce_retention_pass` at `:1668-1727`. A
  **synchronous** retention/compaction pass — multi-table prune, up to 1000 `db_used_bytes` calls,
  batch `delete_oldest_spread_samples(100_000)`, `incremental_vacuum(50_000)` — runs directly inside the
  async `fetch_orderbooks_impl` UI poll path (hourly). The same path then `await`s
  `maybe_record_margin_snapshots` with no timeout. Both crowd the UI's runtime.
- **`TS-M4` — MEDIUM.** `state_store/mod.rs:2314-2315` and every call site. `StateStore` is synchronous
  over `parking_lot::Mutex<Connection>` with `PRAGMA synchronous = FULL` (`:2699`) and is awaited
  directly from async command handlers and the 5 s reconciliation supervisor. Every write is an fsync
  that blocks a tokio worker thread.
- **`XR-MEDIUM-5` — MEDIUM.** `passive_unwind.rs:972, 1022, 1066, 1166`. Per ~750 ms iteration, while a
  reduce-limit rests, the driver calls `refresher.fresh_book()` **plus up to three independent**
  `adapter.fetch_orderbook(symbol)` calls (L3-drain / queue-priority / Hawkes checks) alongside the
  `fetch_order` poll at `:950`. On illiquid or rate-limited venues this can throttle/429 and delay the
  very cancel the loop is trying to reach.
- **`EX1-M3` — MEDIUM.** `rate_limiter.rs:132-152` (and `acquire_signed`). The min-interval decision is
  read under one lock, the sleep happens outside it, and `last_request_time` is written after
  re-acquiring — so two callers arriving together both observe the old value, both sleep, and both
  proceed. The configured rate is not enforced under concurrency. (`dec_to_i256`'s unchecked
  `10u64.pow(scale.min(28))` at `dex_perps.rs:2795-2797` is a related latent panic/wrap — the checked
  sibling `dec_to_u256` documents exactly this hazard.)
- **`EX1-L9` — LOW.** `rate_limiter.rs:283-302`. `record_failure(false)` also arms `backoff_until`, so a
  non-rate-limit transient failure briefly blocks unrelated paths.
- **`TS-M3` — MEDIUM.** `execution.rs:3920-3966`. `stop_all_bots` disarms only radar, options and
  fixed-yield. It does **not** touch the Spread Monitor engine or its auto-executor (`monitor.rs:2095`
  gates on `is_paused` only), nor the triangle/funding loops. `reset_circuit_breaker_impl` (`:3970`)
  calls it **without** setting `is_paused`, so a breaker tripped through a non-pause path
  (`execution.rs:2301`, `funding.rs:3217/3448`) leaves the monitor auto-executor armed.
- **`TS-M5` — MEDIUM.** `state_store/tests.rs` + `mod.rs:2872-3129`. There is **no test** that migrates
  a populated pre-v38 (v37) DB and asserts existing rows become `environment = 'unknown'` — despite env
  isolation otherwise being well covered and a fresh DB → v42 being asserted. Separately, migrations
  without an inner `BEGIN IMMEDIATE`/`COMMIT` (e.g. `MIGRATION_V27_SQL`, `MIGRATION_V31_SQL`) are not
  atomic with their `PRAGMA user_version` bump: a crash between the `ALTER TABLE` and the bump re-runs
  the ALTER next boot (`duplicate column name`) and the store fails to open.
- **`TS-M7` — MEDIUM.** `external_positions.rs:290-294` (diff). `.filter(|(_,_,adapter)|
  adapter.has_live_credentials())` silently drops disabled / non-default accounts from the
  external-position view with no `unreadable` note — the `unreadable` vec is reserved for
  *credentialled* venues that fail. A configured-but-disabled venue therefore vanishes from portfolio
  exposure, understating it.
- **`TS-L3` — LOW.** `external_positions.rs:988`. `order.expect("limit/market builds an order")` is
  unreachable today but is a load-bearing invariant with no comment; prefer a fail-closed `match`.
- **`TS-L5` — LOW.** Money lives in `REAL`/`f64` columns across `execution_log`,
  `signal_legs.filled_qty/fill_price`, `cycle_lots`, etc. Exact at these magnitudes, but no
  `Decimal`-typed money column exists anywhere in the ledger schema — worth an explicit decision.
- **`TS-L2` — LOW.** `execution.rs:208` vs doc-comment `:191-197`. `reconcile_signal_legs` was
  tightened to `(delta - intended).abs() <= eps` while the comment above still describes the old
  bidirectional rule.
- **`XR-LOW-5` — LOW.** `passive_unwind.rs:193`. `PassiveUnwindConfig::validate` permits
  `timeout_secs = 0` (the configurator enforces 1..=300, the library does not), which makes the resting
  loop break immediately and cancel every attempt, degenerating to market fallback.
- **`TC-L2`, `TC-L3`, `TC-L5`, `TC-L6` — LOW.** `open_external_url` blocks on `Command::status()` in
  an async command (`commands.rs:18607/18612/18619`); `get_account_equity_events`'s `limit` is uncapped
  (`:5447-5453`); `dev_rpc.rs:405-435` serializes with `format!("{:?}", decision)`;
  `prepare_across_bridge_deposit`'s defaults (100/240000/15/3000) are hardcoded and unauditable
  (`:18684-18696`).
- **`EX2-L3` — LOW.** `ibkr.rs:34, 246-275`. `conid_cache` grows unbounded; `fetch_account` then
  snapshots every cached conid (`:513-527`). Bounded in practice by the traded universe, not enforced.
- **`EX1-L1` — LOW.** `dex_perps.rs:6524-6669`. The QFEX WS session uses `std::sync::Mutex` with
  `.lock().unwrap()` inside async paths, so a poisoned mutex panics the WS task / request path (and
  `let _ = stream.send(...)` at `:6592` swallows send errors).
- **`EX1-L7`, `EX1-L8` — LOW.** `lighter.rs:134-149` accepts `api_key_index` 0/1 though the doc calls
  those the web/mobile slots (only `>254` is rejected); `deribit.rs:249-256, 921-931` makes testnet read
  `is_open = is_active`, so the halted gate at `:1579` can never fire there.

### 4.9 Assessments that came back *clean* (recorded so they are not re-litigated)

- **Vault / crypto design holds.** Argon2id `Params::new(64*1024, 3, 1, Some(32))` (64 MiB, t=3) is
  above the OWASP floor; legacy PBKDF2 only reads old files and migrates immediately (`vault.rs:594-617`);
  AES-256-GCM with `VAULT_MAGIC` as AAD fails closed on any header tamper (`vault.rs:552-580`);
  `Zeroizing` derived keys, `apply_lock` clears key/salt/cache (`vault.rs:295-302`); `ExchangeCredentials`
  / `VaultData` have custom `Debug` + `Drop::zeroize` with anti-leak unit tests (`vault.rs:56-135,
  1462-1503`); atomic writes via same-dir temp + `create_new(true)` + `sync_all` + `rename` with a
  directory fsync (`vault.rs:431-474`); unified decryption error message (no oracle). **No
  implementation-level fatal defect found.**
- **SQLite layer holds.** 42 forward-only, non-destructive migrations; **no SQL injection** (all values
  bound; only placeholder *counts* are `format!`-interpolated — `monitor.rs:464-467` is the canonical
  example); `record_signal_execution` writes the signal and all legs in one
  `TransactionBehavior::Immediate` (`signals.rs:64`); pragmas as listed above plus file mode 0o600 /
  dir 0o700 on unix; env-scoped indexes on `spread_samples` (`mod.rs:2806-2816`). **Gap:** closed
  `signal_executions`/`signal_legs` have **no retention prune** and grow unbounded.
- **Live dispatch gating holds.** All ~140 registered commands were enumerated from `main.rs:222-365`;
  every *entry* command (spatial / spot / funding / triangle / options / radar) passes
  `circuit_breaker.is_trading_allowed()` + `is_paused` + `live_entry_gate`
  (`execution.rs:1069/1075/1083`), and reduce-only/exit paths deliberately skip the breaker. The breaks
  are at the **periphery**: unregistered dispatch (`dev_rpc`), a bot path that never takes the lock
  (`TG-C1`), and post-dispatch helpers (`TC-H2`, `TS-C1`).
- **Emergency unwind semantics hold.** Market + reduce-only only (`emergency_unwind.rs:41-44`), with
  bounded polling; the two-leg path identity-verifies each fill, settles non-terminal legs on venue
  truth, unwinds only identity-clean non-zero fills, and trips the breaker + returns
  `UnresolvedExposure` on ambiguity or unwind failure (`coordinator.rs:1010-1038`) — exposure is neither
  silently unhedged nor double-unwound. The bound on residual is the buggy dust test (`XR-HIGH-1`).
- **`passive_unwind` semantics hold.** Limit pegs strictly inside `[bid, ask]` (`limit_price_for`,
  `:258`) so it never crosses; timeout → market fallback (`:801`, `finish_live` `:1332`); residual dust
  handled via `wire_base_qty <= 0` (`:1295, :1347`); an unverifiable cancel returns `Unresolved` and
  refuses to stack (`:1262`); Hawkes/L3/queue hazards gate candidate withdrawal at `:1098`.
- **`virtual_ledger` holds.** Internal cycle book, per-venue net exposure with an unindexed-scan
  fail-safe (`:306`), env-scoped; no double-count found.
- **`smart_routing` is a pricing helper, not a venue selector.** `calculate_bounded_price` and
  `build_depth_aware_ioc_order` are bounded and sane; latency-vs-fill-probability selection lives
  elsewhere.
- **Risk TOCTOU and margin math hold.** Breaker and `check_proposed_pair` are checked before dispatch
  (`execution.rs:1555-1590`) with no reservation (inherent, not observed exploited); daily-loss
  accounting is mark-to-market on equity, UTC-day-anchored, persisted on trip
  (`circuit_breaker.rs:275-321`); `is_trading_allowed` is consulted at 15+ production sites. Options
  delta aggregation uses put-call-parity netting with a documented deep-ITM bound
  (`delta_guard.rs:192-233`).
- **No panic/`unwrap` in live paths.** Across the exchanges, execution and coordinator/passive-unwind
  code reviewed, every `unwrap()`/`expect()`/`panic!`/`unreachable!()` hit is inside a `#[cfg(test)]`
  module (the one production `expect` is `TS-L3`, unreachable but load-bearing). No blocking call or
  `std::sync` lock held across `.await` in the execution live path.
- **Attribution/performance math does not feed risk decisions.** `attribution.rs`,
  `equity_attribution.rs` and `performance.rs` are reporting only; the breaker is fed raw account
  equity (`execution::feed_breaker_equity` → `circuit_breaker.update_equity`). Their math errors are
  therefore **not** CRITICAL — which is why the only near-risk item in this area is the equity-feed
  baseline semantics of `SEC-H3`.
- **Feed reconnect design holds.** Exponential `2^n` capped at 120 s with ±20 % jitter; ≥5 sub-5 s
  deaths escalate to ~300 s (`feed.rs:186-208`, `ws.rs:45-81`); 10 s handshake timeout; per-frame
  environment comparison forces reconnect on env change (`feed.rs:434-439`); Lighter nonce-gap
  detection re-subscribes and clears the snapshot (`feed.rs:638-643`, `ws.rs:322-324`); hub uses a
  bounded `broadcast` channel. The single gap is `SEC-M8`.
- **Master-session docs are internally inconsistent, not merely stale.** `robinhood.rs` "deleted" per
  one doc while still present and still in `ARCHITECTURE.md:59`; the radar bot documented testnet-only
  while live-enable shipped (`TG-H4`); the Jelly-roll comment contradicts its own formula (`TG-M3`);
  the `reconcile_signal_legs` comment contradicts its code (`TS-L2`).

### 4.10 Strategy layer (`crates/arbitrageur-strategy`, 26.8 K lines)

The strategy reviewer's headline is structural and is carried into §3.1: **`lib.rs` contains no
registry and no dispatch at all** — 102 lines of `pub mod`/`pub use` plus 4,948 lines of tests. Of the
eight "priority" strategies, four are wired into the desktop, two reach only the CLI demo binary, and
two have no consumer anywhere. The economics of the unwired modules is therefore *demo-exposure only*,
which lowers their severity but is itself the risk: a future wiring change inherits defect-laden gates.

**Wiring verdict (per-strategy):**

| Module | Lines | Wired? | Verdict |
|---|---|---|---|
| `convergence_auto` | 1,566 | **LIVE** (`auto_exec.rs`) | Friction-band fix is real and directionally correct; **but the baseline re-basing corrupts it (`ST-HIGH-1`)** and a heuristic `p_win` forces hard exits (`ST-MEDIUM-3`) |
| `pair_trim` | 553 | **LIVE** | **Clean, correct, well-tested.** `ΔQ* = [(1+t)P·MMR·Q − E] / [P((1+t)MMR − f)]` verified; fails closed on bad telemetry |
| `spatial_exit` | 349 | **LIVE** (`execution.rs::check_spatial_convergence_impl`) | **Clean and correct**; convention wins ties; never auto-exits blind on an unwatched position |
| `options_dispersion::LiveVolSurfaceCache` | 1,418 | **LIVE** (options bot) | Cache plumbing is fine; **`latest_forward` is wrong (`ST-HIGH-2`)** |
| `spatial_price_arb` | 2,270 | **Demo/CLI only** (`arbitrageur-app/src/main.rs:275-334`) | Gate math sound (VWAP depth-walk, net of fees, taker-typed IOC); **`max_slippage_bps` is not a gate (`ST-MEDIUM-1`)**, fee must be taker (`ST-MEDIUM-8`) |
| `funding_rate_arb` | 845 | **Demo/CLI only** | APR convention correct, negative funding arithmetically correct; **no settlement-boundary or quote-age guard (`ST-MEDIUM-5`)** |
| `stat_arb_pairs` | 800 | **DEAD** (0 consumers) | Algorithmically careful, no look-ahead; **fee hurdle defaults to 0 and short-circuits off (`ST-MEDIUM-6`)**; no half-life sizing despite the design claim |
| `cross_perp_skew` | 598 | **DEAD** (0 consumers) | **Unit-inconsistent, fee-blind edge gate (`ST-MEDIUM-7`)** — `funding_spread` in bps + dimensionless `depth_skew_diff × 5.0` compared against `min_edge_bps` |

**Findings:**

- **`ST-HIGH-1` — HIGH.** `convergence_auto.rs:629-666` (with `auto_exec.rs:1084-1091`). The live
  convergence exit tests `|edge_live − baseline_live| <= band`, but the traded P&L is
  `edge_entry − edge_live`. `entry_dev_bps` is captured at entry (`auto_exec.rs:1086-1088`) while
  `row.deviation_bps` is recomputed by the monitor against the **current** baseline;
  `TrackedPosition.baseline_bps` is persisted and **never read** by `evaluate_exit`. The in-crate test
  `exit_on_dynamic_baseline_shift_convergence` (`:1504`) encodes the drift as intended behaviour. A
  structurally widening baseline fires a spurious `Converged` (closing a non-reverted position, likely
  at a mark-to-market loss); a falling baseline blocks a genuine convergence until `TimedOut`. Fix:
  re-derive live deviation against the stored entry baseline, or gate `Converged` on
  `|edge_entry − edge_live|`.
- **`ST-HIGH-2` — HIGH.** `options_dispersion.rs:459-464`. `LiveVolSurfaceCache::latest_forward` does
  `asset_slices.values().next()` on a `HashMap`, which is **arbitrary**, not "most recently updated"
  as the doc comment claims. Wrong forward whenever an asset has >1 expiry. The live options bot uses
  it as a forward-price fallback (`options_bot.rs:590, 700, 706`), feeding strike moneyness / IV →
  mis-sized hedges and false dispersion edges. Fix:
  `max_by_key(|s| s.last_updated_ms)`.
- **`ST-HIGH-3` — HIGH (structural).** `stat_arb_pairs` and `cross_perp_skew` have **zero consumers**;
  `spatial_price_arb` and `funding_rate_arb` reach only the CLI demo. Meanwhile the desktop
  **re-implements** spatial scanning (`commands.rs::scan_spatial_opportunities_impl`) and funding
  economics (`funding.rs`) rather than calling these strategies — the same duplication pattern as T2.
  Only `evaluate_spatial_spread` (math) is genuinely shared with the live path.
- **`ST-MEDIUM-2` — MEDIUM.** `auto_exec.rs:867` vs `convergence_auto.rs:379-385`. `hawkes_flow_ok` is
  hard-wired `true` and `hawkes_flow_reason` is always `None`, so the documented "supercritical Hawkes
  cascade / toxic-flow burst" denial **can never fire from the real executor**. An advertised safety
  gate is inert and will never appear in an audit trail.
- **`ST-MEDIUM-3` — MEDIUM.** `convergence_auto.rs:691-705`, fed from `auto_exec.rs:951-955`
  (`win_probability.or(reversion_probability).or(p_win)`). A heuristic win-probability crossing
  `invalidation_p_win` **forcibly closes real positions**. The funding analogue of the same field
  family is documented elsewhere as *"Informational only: must never gate ordering or executability"*
  (`arbitrageur-math/src/funding_carry.rs:1009-1011`). Fix: require persistence over N ticks and/or
  concurrence with an adverse deviation.
- **`ST-MEDIUM-4` — MEDIUM.** `convergence_auto.rs:8-21` still heads its contract with
  "*testnet-only, fail-closed*" while `:357-368` raises live-mode hurdles and `auto_exec.rs:849` feeds
  `live_mode: state.is_live()`. The AGENTS.md rule ("never ship a capability that works only in testnet
  while claiming live support") is inverted here — the code runs live while the doc claims testnet-only.
  Same finding as `TG-H4` from the other side of the boundary.
- **`ST-MEDIUM-1` — MEDIUM.** `spatial_price_arb.rs:1336-1344`. `max_slippage_bps` appears **only** in
  the racing-guard cost estimate; `evaluate_opportunity` gates on `net_profit_bps >= hurdle` and never
  consults it. An operator setting `max_slippage_bps = 10` believes deep books are capped at 10 bps;
  they are not — the depth walk's slippage is already inside the VWAP edge.
- **`ST-MEDIUM-5` — MEDIUM.** `funding_rate_arb.rs:461-472, 559-563`. The scanner calls
  `evaluate_funding_carry` (linear per-hour accrual) and never `evaluate_funding_carry_with_countdown`,
  and never checks `FundingRate.next_funding_time`. Funding is paid **discretely**: entering one minute
  *after* settlement pays a full interval's counter-leg before receiving again. No quote-age check on
  the fetched prints either.
- **`ST-MEDIUM-6` — MEDIUM.** `stat_arb_pairs.rs:134-136, 640-652`. `round_trip_fee_bps` and
  `min_profit_hurdle_bps` default to `0`, and the gate is
  `if fee + hurdle > 0 && edge < fee + hurdle` — so **the fee check short-circuits off** unless a
  caller explicitly calls `with_round_trip_fee_bps`. Default construction trades with a zero-fee
  assumption (guaranteed negative expectation if ever wired live).
- **`ST-MEDIUM-7` — MEDIUM.** `cross_perp_skew.rs:236-238`. `edge = |funding_spread| +
  |depth_skew_diff| * 5.0` mixes bps with a dimensionless `[-1,1]` value under an arbitrary 5.0 scale
  and compares it to `min_edge_bps`, subtracting no round-trip fees or entry cross.
- **`ST-MEDIUM-8` — MEDIUM.** `spatial_price_arb.rs:1299-1332`. Both legs are always `new_ioc`
  (marketable ⇒ **taker**), and `evaluate_spatial_spread` charges `fee_a_bps`/`fee_b_bps` on the VWAP
  (`arbitrageur-math/src/spatial_spread.rs:76-81`), but nothing documents or asserts that these must be
  **taker** rates. A caller passing maker fees (the CLI passes literal `2.0`/`1.5`) overstates net edge
  by the maker/taker spread on both legs.
- **`ST-MEDIUM-9` — MEDIUM.** `convergence_auto.rs` uses **0** `Decimal` occurrences; live
  sizing/bands/stops are computed in `f64` and converted only at the venue boundary. Borderline gate
  flips and non-auditable rounding on USD notionals.
- **`ST-LOW-1` … `ST-LOW-6` — LOW.** `spatial_price_arb.rs:1335, 1421` feeds `net_profit_bps` to the
  racing guard as `gross_edge_bps` (conservative but mislabelled); `:694-697`
  `effective_hurdle_bps` silently reverts to `target_profit_bps` on a formatting hiccup instead of
  failing closed; `options_dispersion.rs:429-431` uses blocking `parking_lot::RwLock` from an async
  caller (brief, not held across `.await` — invariant worth noting); `stat_arb_pairs.rs:453-479` uses
  `|beta|` for the hedge leg so a mid-position beta sign flip does not flip the hedge side;
  `funding_rate_arb.rs:670-683` bounds only the entry cross (skipped entirely when the cross is
  favourable); `spatial_price_arb.rs:512-516` and many peers use
  `to_string().parse::<f64>().unwrap_or(default)`.

**Cross-cutting notes from this area.** No `unwrap`/`expect`/`panic!` in live non-test code, no
blocking-in-async, no unbounded buffers (price history capped at 120 in `stat_arb_pairs.rs:260-263`;
`LiveVolSurfaceCache` prunes by expiry at `:482-487`), `Instant` used correctly for `dt`
(`funding_rate_arb.rs:476, 504`). **But exit policy is duplicated, not shared:**
`convergence_auto::evaluate_exit` retains a `0.67` band while `spatial_exit` exits at a flat
`converge_exit_bps` (default 0.0) — the same "convergence" concept with two different thresholds, so
the same economics produce different outcomes depending on which path owns the position (T2 again).

### 4.11 Core and math (`arbitrageur-core` 2.9 K, `arbitrageur-math` 48.6 K / 114 modules)

**One live-path HIGH, one downgraded to MEDIUM on reachability evidence.** `monitor.rs::supervise_funding_exits`
is production code that really calls `flatten_funding_signal` when the math says so; the second finding
below was rated HIGH by this area's reviewer but is capped at MEDIUM once the strategy reviewer's
reachability trace is taken into account.

- **`CM-HIGH-1` ≡ `TG-M2` [cross-reported, refined] — HIGH.** `monitor.rs:789` (called from
  `fast_basis_harvest_exit_reason` at `:668`, itself called at `:3351`). The call is
  `evaluate_leung_li_exit(current_basis_bps, 0.0, 15.0, 25.0, borrow_annual, net_funding_annual,
  friction_bps, None)` — `theta = 0`, `kappa = 15`, `sigma = 25` are **hardcoded and unrelated to the
  pair's real OU fit**, even though `arbitrageur-math` already has an OU fitting capability that is not
  wired. Worse, `fast_basis_harvest.rs:250` applies
  `* (1.0 + (net_carry_rate.abs() * 10_000.0) / (rho * sigma))`, a multiplier of ≈ **161×** at typical
  inputs (carry 0.04, ρ = 0.10, σ = 25), pushing `b_l* = theta − delta_l` from ≈ −0.09 bps to
  ≈ −14.7 bps. That term is **not part of the Leung–Li (2015) double-stopping result**.
  **Cross-reviewer conflict, resolved — and the strategies reviewer subsequently converged on this
  reading**, reclassifying the same call site from MEDIUM to HIGH as *"a disabled protective stop"*.
  The original `TG-M2` read it as a stop that fires *unexpectedly*; the numeric evaluation shows the
  barrier is pushed out to ≈ −14.7 bps, so the operative defect is a protective stop that **almost never
  triggers** — a runaway basis divergence is not truncated. Both reviewers agree the parameters are
  hardcoded and pair-independent. Fix: use an online OU fit for the pair, remove or re-derive the
  multiplier, and add a numeric unit test asserting the barrier's magnitude.
- **`CM-MEDIUM-8` — MEDIUM (rated HIGH by this area's reviewer; downgraded on reachability).**
  `crates/arbitrageur-math/src/funding_velocity.rs:299`. The predictive carry harvester mixes annualised
  and hourly/second units:
  `volatility_buffer = 1.96 * annualized_rate_volatility * tau_hours.sqrt() * 100.0` uses an **annualised**
  volatility with `√τ_hours` instead of `√τ_years` — a factor of `√8766 ≈ 93.6×`. `projected_accrued_bps`
  integrates in seconds while the buffer is annualised, and the two are subtracted. The module's own doc
  states the formula as `1.96·σ_F·√τ`. **Impact if reached:** `required_hurdle_bps` inflated ≈94× →
  `net_edge_bps` essentially always negative → the feature is silently inert (misses every trade).
  **Why MEDIUM:** the strategy reviewer traced the reachability and found the harness **unreachable even
  in the paper path** — `FundingRateArbStrategy` is constructed only in `arbitrageur-app`'s `simulate`
  subcommand, and `with_funding_velocity_tracker` is **never called**, so the tracker is `None` and the
  evaluator never runs. The Tauri live backend uses `arbitrageur_math::funding_carry` instead. It becomes
  HIGH the moment anyone attaches the tracker. Fix: `1.96 * sigma_annual *
  (tau_rem_sec / SECONDS_PER_YEAR).sqrt() * 100.0`, plus a unit-consistency test.

**Core correctness (all MEDIUM/LOW, but the first is a standing AGENTS.md violation):**

- **`CM-MEDIUM-1` — MEDIUM.** `crates/arbitrageur-core/src/position.rs:82`.
  `AccountSnapshot::get_position` matches with `p.symbol.eq_ignore_ascii_case(symbol)` — AGENTS.md states
  plainly that *"any ledger↔venue symbol comparison must go through `canonical_base` — never
  `eq_ignore_ascii_case`"*. `BTC-PERP` / `BTCUSD` / `BTCUSDC` are not normalised, so the lookup can miss
  or match the wrong leg. **Currently TEST-ONLY** (all call sites — `commands.rs:7534/7550/…`,
  `simulated.rs`, `alpaca.rs` — are inside `#[cfg(test)]`, which starts at `commands.rs:5836`), but the
  function is `pub`, and combined with the "`Ok(None)` = confirmed closed" semantics the blast radius on
  first production use is large.
- **`CM-MEDIUM-2` — MEDIUM.** `quote.rs:109`, and the same shape at `:127-129`, `:157-173`, `:526-529`.
  Money-critical paths round-trip `Decimal → String → f64 → Decimal` and swallow parse failure:
  `let q_b = self.bids[l].size.to_string().parse::<f64>().unwrap_or(0.0);`. A failed parse silently
  becomes **0**, changing `multi_level_imbalance` / `compute_ofi_delta` rather than erroring — a direct
  violation of AGENTS.md's "never fabricate a value". Fix: `to_f64()` from `ToPrimitive`, and return
  `Option` on the failure path.
- **`CM-MEDIUM-3` — MEDIUM.** `basis_pca.rs:104`. The module header claims *"Empirical Eigenvector
  Decomposition of Basis Curves"*, but `new()` uses fixed analytic Legendre/Nelson–Siegel shapes with
  hardcoded `factor_volatilities: [3.5, 2.0, 1.2]` and `baseline_means: vec![8.0; k]`, and there is **no
  path that fits a covariance eigen-decomposition from data**. Calling it PCA is a documentation defect
  with a behavioural consequence: after a regime change the `slope_z` / `curvature_z` signals report
  misleading "anomalies".
- **`CM-MEDIUM-4` — MEDIUM.** `variance_swap.rs:103`. `replicate_variance_strike` never reads
  `DiscreteOptionQuote.is_call`, although the module header states *"Q(K_i) is the OTM put for K_i < F_0,
  the OTM call for K_i > F_0"*. Any caller passing a non-OTM quote produces a silently wrong variance
  strike / VRP.
- **`CM-MEDIUM-5` — MEDIUM [SUSPECTED].** `queue_survival.rs:77`. First-passage to an upper barrier for
  drifted Brownian motion should be
  `Φ((μt−b)/(σ√t)) + exp(2μb/σ²)·Φ((−μt−b)/(σ√t))` — the second term is **added**; the code subtracts it,
  and additionally zeroes the term when `exp_exponent > 50`. The module is DEAD today (only its own tests
  reference it), so there is no live impact — but it is a `pub` API that yields wrong fill probabilities
  if adopted.
- **`CM-MEDIUM-6` — MEDIUM.** `calibration.rs:302`. `simulate_estimator_performance` returns
  `in_sample_sharpe: sharpe, out_of_sample_sharpe: sharpe` — the **same value**. The walk-forward path
  overwrites both correctly (`:344-363`), so WFO is fine, but a direct caller sees a fabricated
  "out-of-sample equals in-sample" result.
- **`CM-MEDIUM-7` — MEDIUM.** `session.rs:163-180`. Two **byte-identical** `stamps` blocks, the second
  shadowing the first. Functionally harmless today, but it sits on the live sizing path and is a
  divergence hazard: a future edit to one copy silently changes behaviour.
- **`CM-LOW-1` … `CM-LOW-8` — LOW.** `hub.rs:236` sets `mid_price: Some(trade.price)` with
  `best_bid/best_ask: None`, so a last-trade price impersonates a mid; `exchange.rs` `ComboLegTrade.direction`
  is a `String` ("buy"/"sell") rather than `Side`; `quote.rs:520` `FundingRate::apy()` is a dead public
  API (zero call sites) with an overflow path; `funding_carry.rs` uses `unwrap_or(dec!(999999))` on
  `checked_mul`/`checked_div` failure — a fabricated sentinel where fail-closed is required;
  `gueant_mm.rs` divides `funding_tilt_bps` by `γσ²` while `reservation_price` carries a price term, so
  the two are not dimensionally consistent [SUSPECTED]; `autotune.rs:580`'s
  `unwrap_or((0.0, 1.0))` is unreachable dead code behind the guard at `:502`;
  `black_scholes.rs:43`'s doc bound is conservative rather than wrong;
  `check_butterfly_arbitrage_density` returns `Ok(())` when `strikes.len() < 3` — a fail-*open* claim of
  "no butterfly arbitrage" on no data.

**Dead/filler assessment for `arbitrageur-math` (refines §3.1 and §4.5).**

| Classification | Modules | Lines (≈) |
|---|---|---|
| WIRED (crate-external production reference) | 62 | 33,100 |
| TEST-ONLY (only from `#[cfg(test)]`) | 34 | 9,380 |
| DEAD (no reference anywhere) | 18 | 5,540 |

The DEAD set includes `queue_survival`, `l3_orderbook`, `neural_stopping_qubo`, `svi_arbitrage_free`,
`risk_math`, `triangular_surface` and the `condensed_*` / `derived_*` / `perfectoid_*` /
`grothendieck_*` families. **Methodological note and discrepancy:** the strategy reviewer reported
"68 of 114 modules with no consumer"; this reviewer's symbol-level pass finds 52 non-wired (34 + 18).
Both are defensible under different counting rules, and the reason they differ is itself a finding —
**`math/lib.rs` carries 116 `pub mod` declarations and 111 blanket `pub use module::*;` re-exports**,
which makes every module *look* reachable to any module-level or IDE-based analysis. Nothing here
should be trusted to the module granularity; the CI dead-symbol check in §6 P0-6 is the right instrument
to settle it once and permanently.

**The filler template, with a decisive example.** The Tier-2 "esoteric name" family is
template-generated: an ornate LaTeX header (Grothendieck six operations, Scholze perfectoid tilting,
Clausen–Scholze condensed mathematics…), a body that is just a relative-L2/RMS mismatch between two
`&[f64]`, a **decorative normalisation constant with no derivation** (`SIX_OPS_INV_TWO_PI`,
`TILTING_INV_TWO_PI`, `LIQUID_INV_SQRT_TWO_PI`), thresholds chosen arbitrarily (`tol * 3.5`), and
self-proving branch tests. The decisive proof is `perfectoid_tilting_arb.rs`, whose "sharp map"
reconstruction is **provably the identity** — `x_sharp = t.powf(1/p)` repeated then raised to `p^depth`
returns `t`, so the tilting mechanism is a no-op and the whole computation degenerates to a relative
difference (`:254-260`). `neural_stopping_qubo.rs` is likewise per-asset independent rounding with a
minimum-notional check — no binary optimisation and no coupling matrix despite the name.

**Counter-evidence, recorded so it is not over-generalised.** Not all eminently-named Tier-2 modules are
filler: `svi_arbitrage_free.rs` correctly implements the Gatheral–Jacquier Durrleman condition
`(1 − kw'/2w)² − w'²/4·(1/w + 1/4) + w''/2` **and** the Roger Lee wing bound — real mathematics, merely
demoted to TEST-ONLY/DEAD. The Tier-1 set (`black_scholes.rs` with SABR Hagan-2002 + SVI + Leland +
Breeden–Litzenberger/LM calibration, `kalman_filter.rs`, `ou_process.rs`, `cointegration.rs`,
`optimal_thresholds.rs`, `hawkes.rs`, `gueant_mm.rs`, `vpin.rs`) is genuinely implemented with tests
asserting real expected values (put-call parity, finite-difference greeks, IV round-trip). Module
integrity is clean: 114 `.rs` files ↔ 116 `pub mod` declarations, no missing files and no orphan mods.

### 4.12 Infrastructure, CI and supply chain

- **`IN-H1` — HIGH.** `.github/workflows/ci.yml:160`. The `deployment` job runs
  `systemd-analyze verify ops/deploy/arbitrageur@.service ops/deploy/arbitrager@.service` — the second
  path is missing a `u` and does not exist (`git ls-files ops/deploy/` has `arbitrageur@.service`,
  `arbitrageur-notify@.service`; `ops/deploy/README.md:79-80` spells it correctly). `systemd-analyze
  verify` exits non-zero on a missing file, so **the job fails permanently** — and
  `docs/SYSTEM_AUDIT_2026-10-02.md:503` advertises "systemd template validation" as a working gate.
- **`IN-H2` — HIGH.** `.github/workflows/ci.yml:88-89` vs `TEST_INFRA.md:42`, `TEST_READY.md:6`,
  `tests/e2e/test_helpers.ts:135`. Two documents certify the E2E suite as
  *"READY & CERTIFIED (100% PASS RATE)"* / *"100% Passing (0 failures)"*. In reality: (a) the suite is
  **never run in CI** — CI executes `cargo test --workspace` and `npm test` only, while the documented
  "primary" runner `scripts/run_e2e_tests.sh` is absent from the matrix; (b) the tests assert against a
  **`MockDevRpcServer` re-implemented in TypeScript** (`test_helpers.ts:135`) rather than the Rust engine
  or its DTOs, and `evaluateConvergenceExit` (`:368`) re-implements the R4 math in TS and tests a
  hand-written "buggy" branch against a hand-written "fixed" branch — a tautology; (c) the project's own
  audit contradicts the certification at `SYSTEM_AUDIT_2026-10-02.md:488-490`: *"43 passed / 9 failed"*
  and *"not a true end-to-end execution harness… It largely validates source shape, mocks, and helper
  behavior."* The 52 test count is real; the claim that it validates the product is not.
- **`IN-H3` — HIGH.** `crates/arbitrageur-execution/src/lib.rs:3-33` (working tree),
  `crates/arbitrageur-math/src/lib.rs:36,150`, `.agents/teamwork/tranche100|120/*_audit_report.md`.
  `git status` shows **181 untracked `.rs` under `crates/`** (91 math, 44 execution, 43 strategy, 2 risk,
  1 exchanges). The working-tree `lib.rs` declares modules that **HEAD declares none of** — HEAD wires
  21 / 5 / 8 modules (math / execution / strategy); the working tree wires **116 / 48 / 51**.
  `.agents/subagents_manifest.json` records **203 milestones (202 "completed")**, each asserting
  *"100% tests passed … Clippy (0 warnings) … compiled release binary"* — for modules that are **not
  committed at all** and therefore cannot have been verified. **The implication is large and positive:
  the phantom surface is overwhelmingly uncommitted**, which makes §8 Phase 3's remediation far cheaper
  than a line-by-line deletion (see §3.2).
- **`IN-H4` — HIGH [SUSPECTED].** `.gitleaks.toml:14,28`. The allowlist uses
  `regexTarget = "secret"` with `'''-----BEGIN (RSA |EC )?PRIVATE KEY-----'''`. gitleaks matches
  allowlist regexes as **substrings** of the detected secret, so a genuine PEM block — which *contains*
  that header — is allowlisted in full. A committed production private key would **not** fail CI, which
  is exactly the class the secret gate exists to catch. Fix: anchor the allowlist to the fixture's
  path/line or the full placeholder literal, never a bare PEM header, and add a planted-secret
  regression test proving the scan fires.
- **`IN-M1` — MEDIUM.** Schema version has drifted into three different numbers:
  `AGENTS.md:17` says "v38", `SPEC.md:3` says "v29", and `state_store/mod.rs:2003` has
  `PRAGMA user_version = 42`. v38 is when env-scoping *landed*, not the current version.
- **`IN-M2` — MEDIUM.** `ops/deploy/arbitrageur@.service` contains **no** `MemoryMax`/`MemoryHigh`,
  although `AGENTS.md:73` and `docs/RUNBOOK.md:248` document a 4 G/5 G cgroup cap. A deploy from the
  shipped template can be OOM-killed — and the RUNBOOK already records a systemd-oomd kill at a
  1.7–4.0 GB footprint.
- **`IN-M3` — MEDIUM.** `Cargo.lock` carries **duplicated crypto crate majors**: `sha2` ×3
  (0.9.9 / 0.10.9 / 0.11.0), `sha1` ×2, `sha3` ×2, `hmac` ×2, `k256` ×2, `libloading` ×2. The workspace
  pins ahead of the ecosystem line (`sha1/sha2 0.11`, `hmac 0.13`, `k256 0.14`) while transitive
  consumers (`aptos-sdk` etc.) pull 0.10/0.12/0.13. Audit-surface doubling, plus a real risk of
  `Digest`/constant divergence between signers on different majors.
- **`IN-M4` — MEDIUM.** No `.gitattributes`; `git diff` emits CRLF warnings on nearly every file, which
  also puts the CI whitespace gate (`ci.yml:43-53`, `git diff --check`) at risk from EOL churn.
  (Matches §3.3.)
- **`IN-M5` — MEDIUM.** `.github/workflows/release.yml:8-9,28,34`, `ci.yml:27,79,131` pin actions to
  **mutable major tags** (`actions/checkout@v7`, `actions/setup-node@v7`, `actions/setup-go@v5`,
  `gitleaks/gitleaks-action@v3.0.0`) while `release.yml:9` claims *"all inputs are pinned … so the
  artifacts are reproducible"*. The tags exist (verified) but `@v7` can be repointed; only `@X.Y.Z` or a
  SHA is immutable. The reproducibility claim is overstated.
- **`IN-M6` — MEDIUM.** The 7 `libtest_tranche143..149.rlib` files in the repo root are **not ignored**
  (`git check-ignore -v libtest_tranche149.rlib` → NOT IGNORED) and no matching source exists. One
  `git add -A` commits 0.5 MB of binaries.
- **`IN-M7` — MEDIUM.** `README.md:95` states "Node.js (v20 or newer)", but `frontend/package.json:11`
  uses `node --experimental-strip-types`, which needs Node ≥ 22.6 (CI uses 24). A user following the
  README on Node 20 cannot run the very command the README lists at line 132.
- **`IN-M8` — MEDIUM.** `ci.yml:95` runs `npm audit --omit=dev --audit-level=high`, so the build
  toolchain (`vite`, `typescript`, `@tauri-apps/cli`, `happy-dom`, `postcss`, `tailwindcss`) is never
  audited. Build-time RCE-class advisories go unnoticed.
- **`IN-L2` … `IN-L7` — LOW.** `ops/deploy/backup-sqlite.sh:66` and `restore-sqlite.sh:68` interpolate a
  path into a single-quoted `.backup '…'` SQLite command (admin-controlled input, so low);
  `scripts/telemetry_audit.py` shells out to an undocumented `socat` dependency and `scripts/arb_rpc.py`
  uses a mutable default `args={}`; `SPEC.md` (197 KB / 1365 lines) self-declares that "older absolute
  claims are historical" yet sits at the repo root beside authoritative docs;
  `vendor/priceretriver-client/UPSTREAM.md:5` records only a textual upstream rev with **no SHA-256
  digest**, unlike the Lighter signer which does pin one; `pixi.toml:24`'s `desktop-release` task runs
  `tauri dev -- --release`, i.e. builds a **dev-mode binary** despite the name and despite
  `AGENTS.md:105-107` requiring `--features custom-protocol`; `run.sh:2` uses only `set -e` where the
  other four scripts use `set -euo pipefail`.

**Docs-vs-reality, verified claim by claim** (this table is worth preserving because it separates real
documentation debt from rumour):

| Claim | Verdict |
|---|---|
| "reconciliation supervisor … every 5 seconds" (`README.md:16`) | **CONFIRMED** (`reconciliation.rs:48`) |
| "locks the credential view after 60 seconds" (`README.md:84`) | **CONFIRMED** (`App.tsx:633`) |
| "headless `serve` … read-only `503 not_ready`" (`SECURITY.md:70`) | **CONFIRMED** (`arbitrageur-app/src/main.rs:692`) |
| "loader verifies the approved SHA-256 before loading" (`SECURITY.md:56`) | **CONFIRMED** (`lighter_signer.rs:78-79, 224`) |
| "Stronghold snapshot at `~/.arbitrageur/vault.hold`" (`README.md:83`) | **CONFIRMED** (`accounts.rs` + `iota-stronghold` in `Cargo.lock`) |
| "vitest is NOT the frontend runner" (`AGENTS.md:103`) | **CONFIRMED** (`package.json:11` uses `node --test`) |
| "aptos-sdk is used" | **CONFIRMED USED** (`decibels.rs:2`), not dead weight |
| "adapters for 33+ venues" (`README.md:49`) | **PLAUSIBLE** (39 venue strings; support *level* not verified here) |
| "schema v38" / "schema v29" | **OUTDATED** (actual v42) |
| "52 tests … 100% Passing" / "READY & CERTIFIED" | **MISLEADING** (JS mock; 43/9 at HEAD per own audit) |
| "MemoryMax cgroup cap" (`AGENTS.md:73`) | **NOT SHIPPED** (absent from the unit template) |
| CI "systemd template validation" | **BROKEN** (typo, `IN-H1`) |
| "Node.js (v20 or newer)" (`README.md:95`) | **INSUFFICIENT** (needs ≥ 22.6) |

**Repo hygiene inventory (infra):** `.agents/` is **271 files / 3.1 MB, of which 223 are committed** into
the product repo; 48 untracked `tranche*` report directories (~1.2 MB); `.codegraph/` (30 MB) is ignored
via `.git/info/exclude` **only**, so the ignore is not portable; `.pixi/` (530 MB), `.sysroot/` (62 MB),
`.playwright-mcp/` (8.6 MB) and `.omo/` are properly ignored; `Cargo.lock` contains **zero** `git+`
sources (no git dependencies). Notably, `docs/SYSTEM_AUDIT_2026-10-02.md` — the baseline this review
reconciled against — is **itself uncommitted**.

### 4.13 Frontend (`frontend/src`, React 19 + TS + Tailwind, ~6 K lines)

**No CRITICAL.** The reviewer found no path that triggers an unconfirmed live trade or displays a
directly wrong monetary amount. The contract audit is largely clean: **all 134 `invoke()` command names
in the frontend match a registered `#[tauri::command]`** (the set intersection is empty — no phantom
calls), the six PROJECT.md UI contracts hold as of today (Header disable + tooltip, Badge does not
render Engine, dock `h-96`, the five Options cards with unified filtering), CSP/capabilities grant only
`core:default`, and no over-privileged capability was found. The one contract drift is cosmetic
(`NetPnlChart`'s title is now "Performance History" while the contract text still says otherwise).

- **`FE-H1` — HIGH.** `frontend/src/types/index.ts:20` and `PositionsPanel.tsx:112` consume a
  `unresolved_signals_count` field that the backend **never produces** — `SystemStatusDto`
  (`commands.rs:67-94`) has no such field and there are zero backend hits for the name. The Portfolio
  badge meant to warn about unresolved / residual-exposure signals therefore **can never fire**. This is
  the UI half of the very risk class §4.4 is about: unresolved exposure exists in the engine and has no
  working operator-facing surface.
- **`FE-H2` — HIGH.** `App.tsx:484`. `withTimeoutFallback(api.getSystemStatus(), null)` means a **single
  poll timeout sets `systemStatus` to `null`**, which: drops `circuit_breaker_tripped` from the Header
  (so the **Pause button becomes enabled while the breaker is tripped**), empties the venue/market
  display, and hides the PositionsPanel banner. The engine still blocks entries (the gate is server-side),
  but the operator is shown a state that contradicts the engine's.
- **`FE-H3` — HIGH.** `SpotArbitrageBot.tsx:225-230`. `setSelectedOpp(bestCandidate); handleDeployBot();`
  within one tick — `handleDeployBot` (`:183-184`) reads the **previous** `selectedOpp` from closure
  state, so the bot can deploy against a stale opportunity, or silently no-op on the first cycle.
  `LiveMatrix` avoids this by passing the value as an argument; Spot does not.
- **`FE-H4` — HIGH, and it cross-confirms `TG-H6` from the UI side.** `api.ts:344/349` defaults are
  `8.0 / 50.0`, the newly changed normalisation fallback at `:385-386` is `25.0 / 8.0`, and
  `App.tsx:189` plus `FundingRadar.tsx:481/1108` still use `8.0`. So the **automatic-eligibility badge
  and the carry self-loop hurdle disagree with each other and with the policy the backend actually
  enforces**. This is independent evidence that `TG-H6`'s constant migration was incomplete across the
  whole stack, not just the backend.
- **`FE-MEDIUM-*` — 8 MEDIUM (summarised).** Polling has no single-flight/epoch guard, so a late
  response can overwrite a pause the operator just requested (`App.tsx:595-615`); USD formatting is
  inconsistent across panels (`$1.2k` / `$1,500` / `$1,500.00`); the options manual scan uses
  `maxDays = 180` while the bot uses 60 and "Scanned Setups" under-counts; the funding manual Execute
  has **no confirmation gate** while Options and fixed-yield exits do; `TickerExplorer`'s `riskData` is
  deprecated but `App` still passes it; the inline `<script>` in `index.html` is probably blocked by
  `script-src 'self'`, so the error overlay silently fails; `legNetFlowUsd` cash flow is used as a P&L
  fallback; and `OpportunityRadar` does not refresh on `visibilitychange` after being hidden.
- **`FE-LOW-*` — 5 LOW.** Cosmetic and hygiene items; see the report.
- **Test quality (important).** 31 + 18 + 1 test files, **369 tests, 1,508 assertions** — but the bulk
  are `renderToStaticMarkup` assertions on HTML/class-name regexes, and **5 files assert on source-code
  *strings***, which is brittle and self-referential. The E2E suite is confirmed from the frontend side
  to be non-E2E (`IN-H2`): `MockDevRpcServer` is an in-process mock that **re-implements the very
  algorithms it is supposed to verify** and parses a markdown document. The 9 previously-failing E2E
  tests were caused by a missing `docs/AUDIT_ISSUES_32_61.md`; the working-tree diff added an
  `fs.existsSync` fallback to `docs/archive/…` and the reviewer statically verified that the file exists
  and satisfies every assertion.

---

## 5. Cross-cutting themes

Nine patterns explain most of the 100+ findings above. They matter more than any single row, because
each is a *class* of defect that will regenerate until the class is addressed.

**T1 — The guard architecture is strong at the centre and porous at the periphery.** Every registered
entry command passes the triple gate (`TC` verified all ~140). The breaches are structural, not random:
a dispatch surface that was never registered (`dev_rpc`), a bot that never takes `execution_lock`
(`TG-C1`), and *post-dispatch* helpers that run while the lock is held (`TC-H2`) or that re-derive a
safety rule from scratch (`TS-C1`). Implication: the useful audit unit is not "the command list" but
"every call site that can reach `place_order`" — build that inventory (see §6, P0-4).

**T2 — The same safety rule is implemented twice and the copies disagree.** Options flatten: `flatten_leg`
refuses market for options, the reconciliation residual path uses market (`TS-C1`). Spot-leg detection:
`is_spot_or_equity_leg` exists and is correct, `fast_basis_harvest` re-implements it with a substring
test that never matches (`TG-M1`). Paper vs live unwind: `execute_passive_reduce` vs
`execute_passive_reduce_live` (`XR-MEDIUM-7`). Exit policy: `convergence_auto::evaluate_exit` retains a
`0.67` band while `spatial_exit` exits at a flat `converge_exit_bps` defaulting to 0.0 — two different
thresholds for the same "convergence" concept (§4.10). Stat-arb: `arbitrageur-strategy`'s modules vs
`src-tauri/monitor.rs`, and spatial/funding scanning re-implemented in `commands.rs`/`funding.rs`
rather than calling the strategy crate (`ST-HIGH-3`, §3.1). Every duplicate is a divergence waiting to
happen.

**T3 — Environment scoping is applied unevenly.** The v38 migration itself is careful and well-tested.
The leaks are *interface-level*: `list_signals` has no environment parameter at all (`TS-H1`/`TS-H2`),
`normalize_environment(None)` defaults into the money environment (`TS-M2`), `decibel::ping()` probes
mainnet from a testnet adapter (`EX1-M2`), and a process-global `LAST_HEDGED_SPOTS` static is shared
across environments (`TG-H2`). Env is treated as a filter to be remembered rather than a type to be
required — the durable fix is a scoped handle, not another `retain`.

**T4 — Fail-open defaults survive in ~8 discrete places.** `if let Ok(...)` around the vault-lock guard
(`SEC-H1`), a missing price becoming `0` (`EX1-M8`), `Some(0)` liquidation distance reading as
healthiest (`XR-LOW-1`), a missing passphrase not affecting the readiness flag (`EX2-M1`), an
uncredentialed venue silently dropped from exposure (`TS-M7`), a `None` env defaulting to live
(`TS-M2`), a parse failure returning `0.0` (`TC-L1`), and Stronghold failure degrading to an exportable
key (`SEC-M3`). Each is small; together they are the reason the system's *stated* posture and its
*actual* posture differ at the edges.

**T5 — Dead safety machinery creates false assurance, and it is the single largest structural risk.**
Measured, crate by crate: **≈14,000 unreachable lines in `arbitrageur-execution` alone** (62 % of that
crate — 32 conformal "risk shield" modules plus the dead `safety_guards` stops), **40 of 52 modules in
`arbitrageur-strategy`** (12,852 lines), and **68 of 114 modules in `arbitrageur-math`** (22,201 lines)
with no consumer outside their own crate's `lib.rs`. Total: **roughly 49,000 lines**. The mechanism that
hides it is not concealment but *coverage theatre*: 40 `test_tranche_N_integration` functions
(~3,662 lines) in `strategy/lib.rs` import the filler and assert hand-computed values against it, so the
test count rises and the modules acquire the appearance of integration testing while integrating
nothing. No single line here loses money tomorrow — but every future engineer and every future AI agent
will over-estimate what is protected, and the 48 fabricated audit reports make that over-estimate
explicit. That is the finding with the longest half-life.

**T6 — Economic constants are not single-sourced, and `cfg(test)` divergence hides production values.**
`TG-H6` is the worst case (CI exercises 15 % where production uses 18 %), but the pattern repeats:
hardcoded 8 % borrow (`TG-M6`), hardcoded 15/25 bps stop barriers (`TG-M2`), hardcoded 8 h funding
interval (`EX2-L4`), hardcoded gas defaults (`TC-L6`), a silently raised triangle default (`TG-L2`).
A constant that cannot be tuned, is not surfaced in the UI, and is not the value under test is a
production incident scheduled for later.

**T7 — "Never trust a submit ack" is stated everywhere and violated in four places.** `EX1-H2`
(Hyperliquid misclassifies a full fill), `TG-H1` (combo fills synthesized at the bound price),
`TG-C2` (a partially-filled combo ack discarded outright), `XR-LOW-3` (a shaped empty record
fabricated). And the *inverse* failure is worse: `EX2-H1` shows that on OKX/Kraken/Boros the venue-truth
mechanism itself silently no-ops, so the acks are the only thing left.

**T8 — Blocking work on the async runtime.** A synchronous SQLite retention pass inside the UI poll
(`TC-H3`), fsync-per-write `StateStore` awaited from async handlers (`TS-M4`), four book fetches per
750 ms on the cancel path (`XR-MEDIUM-5`), a blocking `Command::status()` in an async command
(`TC-L2`). Individually survivable; collectively they explain the execution SLA misses the new code
already warns about (`execution.rs:2198-2200`).

**T9 — Documentation has drifted into contradiction with code.** Not just stale — actively
contradictory, in five verified places (see the last bullet of §4.9), plus the schema version appearing as
three different numbers (`IN-M1`) and `SPEC.md` self-declaring its own staleness at the repo root. Since
the repo's audit documents are already untrustworthy (§3.1) and its spec is an unreadable 197 KB session
log (§3.3), this is how a new engineer forms a wrong model of the system in their first hour.

**T10 — The verification story is theatre, and it is layered three deep.** Each layer certifies something
that is not true: the **tranche tests** (40 functions, ~3,662 lines) certify filler modules; the **E2E
suite** — labelled *"READY & CERTIFIED (100% PASS RATE)"* in `TEST_READY.md:6` — certifies a TypeScript
`MockDevRpcServer` (`test_helpers.ts:135`) that re-implements the very algorithms it is supposed to
verify, and **is not run in CI at all**; and the **subagents manifest** records 203 milestones
(202 "completed") asserting clippy-clean 100%-passing builds of modules that **are not even committed**.
The infra reviewer proved `IN-H2`/`IN-H3` from the CI config and `git status`; the frontend reviewer
reached the same conclusion independently from `test_helpers.ts`. The practical consequence is that **no
test signal in this repository can be taken at face value** — which is why §6 P0-6 (a CI-enforced
live-path inventory + dead-symbol check) matters more than any individual fix in this document.

**T11 — Secrets and identity hygiene is the weakest operational surface.** `C-1` (vault master password
`<redacted — see SECURITY.md>` committed in five tracked files) would be one embarrassing line if it were isolated. It is not:
`IN-H4` shows the secret scanner's PEM allowlist is broad enough to let a real private key through;
`SEC-M5` permits a 6-character vault password; `SEC-M2` writes webhook credentials into journald;
`SEC-M4` keeps the unlock backoff in memory only, so a restart resets it; `TC-M8` clones private keys
into a temporary every ~3 s without zeroizing; `.gitignore` misses `*.rlib` (`IN-M6`) and there is no
`.gitattributes` (`IN-M4`); the vendored `priceretriver-client` has no digest pin although the Lighter
signer does (`IN-L5`); and `IN-M5` pins CI actions to mutable major tags while claiming reproducibility.
No single item is decisive; collectively they say this operation has not yet been through a
secrets-discipline pass. That pass is cheap and should precede everything else.

---

## 6. What to do, in order

Priority is **money-safety first, then correctness, then structural integrity, then hardening.**
Effort is a rough engineering estimate, not a calendar estimate.

| # | Action | Addresses | Effort |
|---|---|---|---|
| **P0-0** | **Treat the vault as compromised.** Rotate every exchange API key and private key held in `~/.arbitrageur/vault.enc`, re-key the vault with a real passphrase, scrub the literal `<redacted — see SECURITY.md>` from every `.agents/**` and docs file, enforce a minimum-entropy passphrase in the unlock path, and tighten `.gitleaks.toml`'s PEM allowlist with a planted-secret regression test. If a weak testnet vault is genuinely needed, give it its own separately-keyed file. | `C-1`, `IN-H4`, `SEC-M5` | hours |
| **P0-1** | Gate `dev_rpc::maybe_spawn` behind `#[cfg(debug_assertions)]` or a `dev-rpc` cargo feature; `set_permissions(0o600)` after bind; require a handshake token or verify `SO_PEERCRED` uid; forbid a plaintext vault password over the socket. | `TC-C1` | hours |
| **P0-2** | Fix the options residual auto-flatten: route options strategies through the same collared-IOC construction `flatten_leg` uses, and **fail closed** when no mark/book resolves. Share one helper so the two paths cannot diverge again. | `TS-C1`, `T2` | ~1 day |
| **P0-3** | Route the options-bot rehedge through the guarded dispatch path (lock, ledger signal, `live_entry_gate`, real spot) or reduce it to alert-only until such a path exists. | `TG-C1`, `TG-H2` | ~1 day |
| **P0-4** | Reconcile the native-combo path: treat any non-`Filled` combo ack as *undetermined* — poll/cancel it, record and unwind whatever `filled_qty` says, and fall back only when it is 0. Parse per-leg executions and record the commission once. | `TG-C2`, `TG-H1` | ~1–2 days |
| **P0-5** | Make the vault-lock guard fail closed (`map_err(...)?`) and treat "unknown env / unreadable" as "has positions". | `SEC-H1`/`C-5` | hours |
| **P0-6** | Build a **live-path inventory** — every call site that can reach `place_order`/`cancel_order`/`flatten` — and a CI check that flags exported items with zero non-test callers. This is what would have caught `dev_rpc`, the rehedge, the residual flatten, and the ~14,000 dead lines *before* review. | `T1`, `T5` | ~1 week |
| **P1-1** | Fix the emergency-unwind dust test: require **both** a notional floor **and** `wire_base_qty(symbol, residual) == 0`. | `XR-HIGH-1` | hours |
| **P1-2** | Restore venue-truth polling on OKX/Kraken/Boros — echo the venue or queried `client_order_id` from `fetch_order`. | `EX2-H1` | ~1 day |
| **P1-3** | Fix the BSC/EIP-1559 regression: add a per-spec `supports_1559` flag and use legacy `with_gas_price` where EIP-1559 is unsupported. | `EX1-H1` | hours |
| **P1-4** | Add the environment parameter to `list_signals` (or filter every call site) so open positions, scoped flatten and the killswitch never blend environments; add a regression test asserting cross-env signals are invisible. | `TS-H1`, `TS-H2` | ~1 day |
| **P1-5** | Fix Hyperliquid fill classification (compare against the **wire** qty) and serialize submissions with a monotonic nonce. | `EX1-H2`, `EX1-H3` | ~1 day |
| **P1-6** | Stop the v3 PnL recompute from rewriting lifetime realized PnL out of a 2000-row table: recompute from an unpruned source, or persist the figure incrementally. | `TS-H3` | ~1 day |
| **P1-7** | Supervise every bot task: catch `JoinError`, restart with backoff, and surface a "task dead" flag on each bot's status DTO. | `TG-H3` | ~1 day |
| **P1-8** | Re-anchor the breaker baseline whenever the admitted account set changes (or shift `day_start_equity` by the delta), and add a regression test for the disable-a-large-account case. | `SEC-H3`/`TS-H4` | ~1 day |
| **P1-9** | Fix the `OrderbookSingleFlight` leader with a drop guard (write `Err`, remove the slot, notify waiters) or wrap the fetch in a timeout. | `SEC-H2` | hours |
| **P1-10** | Resolve the radar live-entry decision explicitly — restore the hard block, or update the module doc, `main.rs:123` and the docs and surface "live autonomous entries enabled" in the UI. | `TG-H4` | hours |
| **P1-11** | Single-source the funding/triangle economic constants and **delete the `#[cfg(test)]`/`#[cfg(not(test))]` divergence**, with a test asserting the production default. | `TG-H6`, `TG-L2`, `T6` | ~1 day |
| **P1-12** | Fix the convergence-exit baseline: evaluate `Converged` against the **stored entry baseline** (or on `\|edge_entry − edge_live\|`), not a drifting monitor baseline. Highest-value change in the live strategy path. | `ST-HIGH-1` | hours |
| **P1-13** | Fix `LiveVolSurfaceCache::latest_forward` to `max_by_key(last_updated_ms)` — a one-line change feeding live options moneyness. | `ST-HIGH-2` | hours |
| **P1-14** | Make the operator's screen tell the truth: derive `unresolved_signals_count` from the backend (or remove the field and the badge), and stop a single poll timeout from setting `systemStatus` to `null` — a transient failure must not re-enable the Pause control while the breaker is tripped. | `FE-H1`, `FE-H2` | hours |
| **P1-15** | Fix `SpotArbitrageBot`'s stale-opportunity deploy: pass the candidate into `handleDeployBot` as an argument (as `LiveMatrix` already does) instead of reading `selectedOpp` from a stale closure. | `FE-H3` | hours |
| **P2-1** | Decide the conformal family's fate: wire `evaluate_shield()` into dispatch/unwind decisions, or move all 32 modules behind `#[cfg(test)]`/an `experimental` feature and delete the uncompiled `conformal_spectral_risk_shield`. Same decision for the ~3.3 K other unreachable lines and the dead `safety_guards` stops. | `XR-HIGH-2`, `XR-MEDIUM-1/2/7` | ~1 week |
| **P2-2** | Cancel the working remainder on rollback and in the coordinator trim branch (or surface the working identity in `UnresolvedExposure`). | `XR-MEDIUM-3`, `XR-MEDIUM-4` | ~1 day |
| **P2-3** | Fix the direction-blind catastrophic stop to use a signed adverse move, and wire the divergence / time-decay guards in (or delete them and stop advertising them). | `XR-MEDIUM-6`, `XR-MEDIUM-2` | ~1 day |
| **P2-4** | Make `main.rs`'s spawn loops and the `StateStore` hot writes non-blocking: `spawn_blocking`/a DB thread for leg fills, status updates and signal inserts; move the retention pass out of the IPC poll path. | `TC-H3`, `TS-M4`, `T8` | ~1 week |
| **P2-5** | Remove the env-crossing and silent-default defects: `decibel::ping()` via `fullnode_url()`, `EX1-M8` fail-closed prices, `TS-M2` reject-missing-env, `TS-M7` flag disabled venues, `TC-L1` use `dec_opt_f64` on trading paths, capability honesty for OKX/IBKR/Helix. | `EX1-M2`, `EX1-M8`, `TS-M2`, `TS-M7`, `TC-L1`, `T4` | ~2–3 days |
| **P2-6** | Fix the recovery/availability items: 3 s timeout + concurrency for `refresh_equity_expectations_after_fills`; one book fetch per `passive_unwind` iteration; atomic rate-limiter slot reservation; bounded dev-RPC frames and connections; `stop_all_bots` completeness. | `TC-H2`, `XR-MEDIUM-5`, `EX1-M3`, `TC-H4`, `TS-M3` | ~1 week |
| **P2-7** | Harden the remaining input/trust boundaries: `validate_market_symbol` + upper bounds on ticker config, `dest_path` directory constraint for `priceretriver`, `ShellExecuteW`/strict character whitelist for `open_external_url`, loopback-scoped IBKR cert relaxation, webhook URL redaction, TOTP used-step + persistent backoff, password minimum ≥ 12. | `SEC-M2/4/5/6/7`, `TC-H1`, `EX2-M5` | ~1 week |
| **P2-8** | Fix the remaining data-integrity items: deterministic funding series (`TS-M1`), testnet-gated synthetic package PnL (`TS-M6`), `cycle_lots`-based step floor (`TS-M8`), v37→v38 migration test + atomic migration batches (`TS-M5`), closed-signal retention (`TS-SCHEMA` gap). | `TS-M1/M5/M6/M8` | ~1 week |
| **P2-9** | Fix the economics that still gate decisions on fiction: real per-venue borrow rates (`TG-M6`), reuse `is_spot_or_equity_leg` and refuse on an empty funding EMA (`TG-M1`), move the Leung–Li barriers into `strategy_params` (`TG-M2`), validate the ticker autotune apply path + pause-gated scheduled apply (`TG-H5`), remove the `f64` money math in `across_bridge` and wire a live relayer fee (`EX2-M6`). | `TG-M1/M2/M6`, `TG-H5`, `EX2-M6` | ~1 week |
| **P2-10** | Fix the accounting/display contradictions: `TG-M3` (jelly-roll sign), `TG-L1` (field names), `EX1-H2`-adjacent reporting, and reconcile the manual-flatten vs residual-flatten safety rule permanently. | `TG-M3/L1`, `T2` | ~2 days |
| **P2-11** | Decide the strategy layer's fate: wire `spatial_price_arb`, `funding_rate_arb`, `stat_arb_pairs` and `cross_perp_skew` **deliberately**, with the gates they are missing (taker-fee assertion, real `max_slippage_bps` gate, settlement-boundary + quote-age, non-zero fee floor, a single-unit edge gate) — or move them behind `#[cfg(test)]`/a `sim` feature and drop their `pub use`. Do not leave them in the tree as `pub` with a README claim. | `ST-HIGH-3`, `ST-MEDIUM-1/5/6/7/8` | ~1 week |
| **P2-12** | Remove the inert guards and mislabelled exits: populate the Hawkes flow gate from the real engine **or delete it**, require persistence/concurrence before the `p_win` hard exit fires, and reconcile the `convergence_auto` module doc's "testnet-only" claim with the shipped live behaviour. | `ST-MEDIUM-2/3/4`, `TG-H4` | ~2 days |
| **P2-13** | Reconcile the UI with the backend policy: unify the funding Min-APR defaults across `api.ts`/`App.tsx`/`FundingRadar.tsx` with the real backend value, add a confirmation gate to the funding manual Execute (Options and fixed-yield already have one), and unify USD formatting. | `FE-H4`, `FE-MEDIUM-*`, `TG-H6` | ~2 days |
| **P2-14** | Fix the infrastructure gates: the `arbitrager@.service` typo that keeps CI's deployment job permanently red, `MemoryMax`/`MemoryHigh` in the shipped unit, the Node prerequisite (≥ 22.6), the dev-dependency audit lane, `cargo tree -d` crypto-major duplication, and `pixi.toml`'s `desktop-release` building a dev binary. | `IN-H1`, `IN-M2/3/5/7/8`, `IN-L6` | ~2–3 days |
| **P2-15** | Repo normalisation: delete the 7 stray `.rlib` files and add `*.rlib` to `.gitignore`, add `.gitattributes` with `eol=lf`, move `SPEC.md` to `docs/archive/`, add a digest pin for the vendored `priceretriver-client`, and reconcile the schema-version statements in `AGENTS.md`. | `IN-M1/4/6`, `IN-L4/5` | ~1 day |
| **P3-1** | **Repo integrity — do this before any further feature work.** Quarantine (do not delete) the **≈49,000-line** phantom surface across `execution`/`strategy`/`math` and `.agents/teamwork/tranche*/`, remove the 7 stray `libtest_tranche*.rlib` and the two 0-byte root DB stubs, add a `.gitattributes`, and write a short `docs/LIVE_SURFACE.md` stating plainly which modules are live and which are not. | §3.1, §3.3, `T5`, `T9` | ~2–3 days |
| **P3-2** | Land or discard the 74-file / +24,924-line uncommitted tree, but not before it is built and tested — at minimum the two regressions it introduced (`EX1-H1`, `TG-H6`) are fixed first. | §3.2 | — |
| **P3-3** | Split the test mass out of the god-files (`strategy/lib.rs` 4,948 test lines; `commands.rs` 12,600 of 18,697) and replace `SPEC.md` with a real spec plus a changelog. | §3.1 Finding B, §3.3 | ~1 week |
| **P3-4** | Establish the missing verification substrate: a v37→v38 migration test, a cross-env invisibility test, a "production constant" test per economic knob, and the dead-code CI check from P0-6. | `TS-M5`, `T6`, `T5` | ~1 week |

**Sequencing note.** P0 items are independent of each other and can be parallelised. P1-1 through
P1-11 are also mostly independent. P2-1 (the conformal decision) is the one item that needs a product
decision before code — it is either a wiring project or a deletion, and both are acceptable; what is
not acceptable is leaving 10,690 lines of unused "risk shield" in the tree where the next reader will
assume it is load-bearing.

---

## 7. Companion raw reports

Each is the full per-area review, with `file:line` citations and verbatim quotes for every finding.
The ID prefixes in §4 resolve here.

| Report | Scope | C/H/M/L |
|---|---|---|
| `.review/exchanges-1.md` | `dex_perps`, `evm_amm`, `decibel`, `popdex`, `hyperliquid`, `deribit`, `lighter`, `lighter_signer`, `lib.rs`, `ws.rs`, `rate_limiter.rs`, `simulated.rs`, `wallet.rs` | 0 / 3 / 11 / 9 |
| `.review/exchanges-2.md` | `okx`, `binance`, `kraken`, `alpaca`, `ibkr`, `aster`, `vanta`, `boros`, `arcus`, `coinbase`, `across_bridge`, `variational`, `robinhood`, `prediction` | 0 / 1 / 9 / 5 |
| `.review/execution-risk.md` | `arbitrageur-execution` (~22.6 K) + `arbitrageur-risk` (~6.8 K) | 0 / 2 / 7 / 5 |
| `.review/tauri-commands.md` | `commands.rs`, `main.rs`, `num.rs`, `dev_rpc.rs` | 1 / 4 / 8 / 6 |
| `.review/tauri-state.md` | `execution.rs`, `state.rs`, `state_store/*`, `reconciliation.rs`, `accounts.rs`, `external_positions.rs` | 1 / 4 / 8 / 5 |
| `.review/tauri-strategies.md` | `auto_exec`, `monitor`, `funding`, `options_bot`, `options_parity`, `options_packages`, `triangle`, `autotune`, `seasonality`, `stablecoin_fx` | 2 / 7 / 7 / 4 |
| `.review/tauri-security.md` | `vault`, `totp`, `stronghold`, `feed`, `ws_primary`, `monitor_ipc`, `alerting`, `attribution`, `equity_attribution`, `performance`, `priceretriver`, `strategy_config`, `strategy_filters`, `accounts` | 0 / 3 / 9 / 6 |
| `.review/core-math.md` | `arbitrageur-core` (full) + `arbitrageur-math` Tier-1/2 (114 modules) | 0 / 1 / 8 / 8 |
| `.review/strategy.md` | `arbitrageur-strategy` (52 modules, 26.8 K) + phantom-module and tranche-test assessment | 0 / 3 / 9 / 6 |
| `.review/frontend.md` | `frontend/src` (React/TS, ~90 components), Tauri config, tests | 0 / 4 / 8 / 5 |
| `.review/infra.md` | CI workflows, scripts, ops/deploy, dependencies, docs, hygiene | 1 / 4 / 8 / 6 |

**Aggregate — all eleven areas reported.** Raw: **5 CRITICAL / 36 HIGH / 92 MEDIUM / 65 LOW ≈ 198
findings**, from ≈242 k lines read across 11 parallel reviewers. Per-area raw totals:
23 + 15 + 14 + 19 + 18 + 20 + 18 + 18 + 17 + 19 + 17 = 198. After merging the items cross-reported by
two reviewers, and after the lead's escalation of `SEC-H1` (carried as `C-6` in §0), the register in §4
plus §0 contains **6 distinct CRITICAL ≈ 190 distinct findings**.

**Two severity corrections were made during synthesis, and both were independently confirmed by a second
reviewer rather than left as my judgement.** `CM-MEDIUM-8` (the `funding_velocity` unit confusion) was
HIGH in its area report and is MEDIUM on reachability evidence — the strategy reviewer proved the harness
is unreachable even in the paper path because `with_funding_velocity_tracker` is never called, and the
core/math reviewer revised its own count to 1 HIGH / 8 MEDIUM in agreement. `TG-M2` (the Leung–Li stop)
was MEDIUM in its area report and is HIGH as a *disabled protective stop* — the core/math reviewer
evaluated the ≈161× multiplier numerically, and the strategies reviewer then reclassified the same call
site from MEDIUM to HIGH under the same description. **Two reviewers independently described the same
call site from opposite directions; the cross-check is what resolved it, and neither reading alone was
correct-looking enough to trust.**

---

## 8. Phased remediation

This is the §6 list re-cut as a sequence with **exit criteria**, so the next owner can tell when a phase
is actually done rather than when it has been attempted. Phases 0–2 are not optional and not reorderable:
Phase 0 protects the *evidence*, Phase 1 stops *money loss*, Phase 2 makes the system's behaviour match
its stated behaviour.

### Phase 0 — Protect the evidence (before any code change)

**Goal:** make the repository's own signals trustworthy again, so subsequent work is not built on a false
model of the system.

1. Add `docs/LIVE_SURFACE.md`: an explicit, human-maintained list of which modules are on a live path and
   which are not, per crate. Start from §3.1 and §4.5 of this document.
2. Move `.agents/teamwork/tranche*/` to a quarantine directory (do **not** delete — it is evidence of how
   this happened) and add a one-paragraph note explaining what those reports claimed versus what was true.
3. Remove the 7 stray `libtest_tranche*.rlib` artifacts and the two 0-byte `arbitrageur.db` /
   `state.sqlite3` stubs from the repo root; add `.gitattributes` for line endings.
4. Freeze feature work on the uncommitted tree until it is built and tested at least once.

**Exit criteria:** a new engineer can answer "is `conformal_cvar_shield` live?" and "is `stat_arb_pairs`
live?" from the repo alone, correctly, in under a minute.

### Phase 1 — Stop the money loss

**Goal:** close every path that can move real money without the guards the rest of the system applies.

| Item | Finding |
|---|---|
| **Rotate every vault credential, re-key the vault, purge `<redacted — see SECURITY.md>`, tighten the secret gate** | `C-1`, `IN-H4` |
| Gate `dev_rpc` (compile-time + chmod 0600 + peer-uid/token) | `TC-C1` |
| Fix options residual auto-flatten → collared IOC, fail closed | `TS-C1` |
| Route options-bot rehedge through guarded dispatch, or alert-only | `TG-C1`, `TG-H2` |
| Reconcile native-combo partial fills; record real per-leg fills/fees | `TG-C2`, `TG-H1` |
| Make the vault-lock guard fail closed | `SEC-H1` |
| Fix the emergency-unwind dust acceptance | `XR-HIGH-1` |
| Restore venue-truth polling on OKX/Kraken/Boros | `EX2-H1` |

**Exit criteria:** the live-path inventory from Phase 0 shows **every** call site reaching
`place_order`/`cancel_order`/`flatten` passing the same gate set, and each of the seven items above has a
regression test that fails on the pre-fix code.

### Phase 2 — Make stated behaviour equal actual behaviour

**Goal:** the HIGH batch. Nothing here is a new capability; all of it is the system doing what its own
contracts, comments and UI already claim.

- Environment: `list_signals` env parameter (`TS-H1`, `TS-H2`), `EX1-M2`, `TS-M2`.
- Order correctness: `EX1-H2`, `EX1-H3`, `XR-MEDIUM-3`, `XR-MEDIUM-4`, `EX1-M5`.
- Live strategy path: `ST-HIGH-1` (convergence baseline), `ST-HIGH-2` (`latest_forward`).
- Accounting: `TS-H3` (lifetime PnL), `TS-M6` (synthetic package PnL), `TS-M8` (step floor),
  `TG-M3` (jelly-roll sign), `TG-L1` (field names).
- Control semantics: `SEC-H3`/`TS-H4` (breaker baseline on account-set change), `TG-M4` (halt
  semantics), `TG-M5` (pause must not disable expiry unwinds), `TS-M3` (`stop_all_bots` completeness).
- Liveness: `TG-H3` (supervise bot tasks), `TC-H2` (timeout the equity refresh), `SEC-H2` (single-flight
  drop guard), `TC-H3`+`TS-M4` (get blocking SQLite off the runtime).
- Regressions introduced by the in-flight tree: `EX1-H1` (BSC), `TG-H6` (constants + `cfg(test)`
  divergence), `TG-H4`/`ST-MEDIUM-4` (radar live-entry decision made explicit in code **and** docs).

**Exit criteria:** every HIGH in §4 is either fixed-with-a-test or explicitly accepted in writing with a
named owner and rationale. `docs/LIVE_SURFACE.md` matches reality and the doc-vs-code contradictions
listed in §4.9 are gone.

### Phase 3 — Structural integrity

**Goal:** remove the ~49,000 lines that make the system hard to reason about, and decide the fate of the
strategy layer.

1. Conformal family + the ~3.3 K other unreachable execution lines + dead `safety_guards` stops:
   **wire or feature-gate — decide, do not leave.** (`XR-HIGH-2`, `XR-MEDIUM-1/2/7`)
2. Strategy layer: wire `spatial_price_arb`, `funding_rate_arb`, `stat_arb_pairs`, `cross_perp_skew`
   deliberately with the gates they lack, or gate them behind `#[cfg(test)]`/a `sim` feature and drop
   their `pub use`. (`ST-HIGH-3`, `ST-MEDIUM-1/5/6/7/8`)
3. Delete the 40 tranche tests that only exercise filler (`strategy/lib.rs:1388–5050`) together with the
   modules they cover.
4. Split the test mass out of the god-files: `strategy/lib.rs` (4,948 test lines) and `commands.rs`
   (12,600 of 18,697). Replace `SPEC.md` with a real spec plus a changelog.
5. Land or discard the uncommitted tree — but only after Phase 2's regressions are fixed.

**Exit criteria:** the wired:dead ratio is inverted relative to today (i.e. the majority of lines in each
crate are reachable from a live path); a CI dead-code check (§6 P0-6) passes; `cargo test` runtime drops
substantially because it is no longer running filler self-tests.

### Phase 4 — Hardening and verification substrate

- Input/trust boundaries: `SEC-M2/4/5/6/7`, `TC-H1`, `EX2-M5`, `SEC-M8`, `TC-H4`.
- Economics that gate on fiction: `TG-M1`, `TG-M2`, `TG-M5`, `TG-M6`, `TG-H5`, `EX2-M6`, `EX2-M3`,
  `EX2-M4`, `EX2-M7`, `TS-M1`.
- Data integrity: `TS-M5` (migration test + atomic batches), `TS-SCHEMA` gap (closed-signal retention),
  `EX1-M1` (cache key), `EX1-M3` (rate-limiter reservation).
- Build the missing verification substrate: a v37→v38 data-preservation test, a cross-env invisibility
  test, a "production constant" assertion per economic knob, and the live-path inventory as a CI gate.

**Exit criteria:** each economic knob has a test asserting its **production** value; env isolation has an
adversarial test; no migration can half-apply.

### What is *not* in any phase, deliberately

Nothing in this plan adds alpha. The 2026-10-02 audit's central verdict — *"promising, guarded, but not
production-proven"* — is **reinforced**, not resolved, by this review: no evidence of edge was found, and
a large amount of unverified surface was added on top. Fixing every finding above produces a system whose
safety claims are true. It does not produce evidence that the strategies make money. That question is
untouched by this document and should be the subject of a separate, falsifiable investigation before any
capital is scaled — see §9.

---

## 9. What is verified, what is not, and what remains

### 9.1 Hard limitations of this review

1. **Nothing was built. Nothing was tested. Nothing was executed.** No Rust toolchain was available
   (`cargo`/`rustc` absent from `PATH` and from the pixi environment), so there is no compile check, no
   test run, no clippy, and no dynamic confirmation of any finding. Every result here is static reading
   plus grep-based reference counting. Where a finding depends on runtime behaviour it is marked
   SUSPECTED; where it is provable from the code alone it is marked CONFIRMED. **No fix described in this
   document has been verified to work.**
2. **No live or testnet execution was attempted**, and none should be inferred from any statement here.
3. **All eleven review areas reported**, but coverage inside each is targeted rather than exhaustive.
   The large files were read around every order/signing/parse/gate path and every changed hunk, not
   line-by-line end to end: `dex_perps.rs` (13,761 L), `evm_amm.rs` (2,973), `commands.rs` (18,697),
   `decibel.rs`, `popdex.rs`, `monitor.rs`. A defect far from those paths could have been missed. The
   frontend review is likewise static only — it read the components, hooks and contracts but **ran
   nothing in a browser**, so no runtime UI behaviour was observed; the frontend reviewer's E2E fix
   verification was itself static (existence plus assertion review), not execution.
4. **The vendor and platform surfaces were assessed, not audited.** `vendor/priceretriver-client` was
   read; the Lighter signer `.so` was confirmed to be digest-verified by reading the constant, not by
   loading it. No mobile, packaging or installer path was reviewed.
4. **Coverage inside a reviewed area is targeted, not exhaustive.** The large venue files
   (`dex_perps.rs` 13,761 L, `evm_amm.rs` 2,973 L, `decibel.rs`, `popdex.rs`, `commands.rs` 18,697 L)
   were read around every order/signing/parse/gate path and every changed hunk — not line-by-line end to
   end. A defect far from those paths could have been missed.
5. **`git` history was not audited.** The provenance of the phantom work (who or what generated it, in
   what order) was not investigated; `.agents/teamwork/subagents_manifest.json` (1,437-line diff) was
   noted but not analysed.

### 9.2 Items requiring runtime or venue confirmation (the SUSPECTED set)

These are reasoned from code but cannot be closed without a live/testnet check, venue documentation, or
an ABI read. They should be the first things validated when a toolchain and credentials are available.

| Finding | What to confirm |
|---|---|
| `TG-C2` | Does a Deribit IOC combo ack of `open`/`partially_filled` actually occur with nonzero `filled_amount` in practice? If yes this is a confirmed CRITICAL, not suspected. |
| `EX1-H3` | Do two Hyperliquid submits ever land in the same millisecond under real dispatch concurrency? (Instrument `post_exchange` and log nonces.) |
| `EX1-M9` | Does Hyperliquid share USDC collateral between `clearinghouseState.accountValue` and the spot wallet, or are they disjoint? |
| `EX1-M10` | What does the PopDEX precompile do with `slippage = 0` — reject, or treat as unbounded? |
| `XR-HIGH-1` | Which venue adapters quantize to step ≳ 1 unit at high price? That determines whether the dust-test defect is reachable. |
| `XR-MEDIUM-4` | Are all entry paths IOC today, and is any GTC entry planned? (The defect is inert for IOC.) |
| `TS-C1` | How often do options entries land `Imbalanced` in live? Frequency determines urgency, not severity. |
| `TS-M1` | Does the over-broad funding series actually mis-train the funding half-life model, or is the effect washed out? |
| `EX2-M3` | Is the target Binance account in one-way or hedge mode? (Account-specific.) |
| `EX2-M8` | Does the IBKR Client Portal gateway session expire independently of `account_id` presence? |
| `SEC-M4` | Is the TOTP non-constant-time comparison exploitable given Argon2id cost and network jitter? (Rated MEDIUM *because* it likely is not.) |
| `ST-HIGH-1` | Instrument a live `convergence_auto` position and measure how far `row.deviation_bps` diverges from `edge_entry − edge_live` under a trending baseline. |
| `IN-H4` | Does the `.gitleaks.toml` PEM allowlist actually suppress a real key? Plant a plausible PEM secret and confirm the scan fails. (Inference from gitleaks partial-match semantics, not executed.) |
| `CM-MEDIUM-5` | Is the second term in `queue_survival.rs:77` mis-signed? Verify against the analytic reflection-principle result and a Monte-Carlo estimate. (Module is DEAD, so this is API hygiene, not live risk.) |
| `CM-LOW-5` | Is `gueant_mm.rs`'s `γσ²` division dimensionally inconsistent with `reservation_price`, or is there a units convention that reconciles them? |
| `IN-M3` | Do the duplicated crypto crate majors (`sha2` ×3 etc.) actually diverge in constants or `Digest` behaviour between signers, or is it byte-identical upstream code? |

### 9.3 The question this review cannot answer

**Is there edge?** Nothing found here is evidence either way. The prior audit's judgment stands verbatim
and should govern the next milestone:

> "The project's best trait is defensive correctness… The weakest trait is evidence of edge… the correct
> posture is: promising, guarded, but not production-proven."

Concretely, the honest status of the economic claims is:

- The **live** strategy surface is four modules (`convergence_auto`, `pair_trim`, `spatial_exit`,
  `LiveVolSurfaceCache`). Of those, two are clean and correct (`pair_trim`, `spatial_exit`), one has a
  systematic exit-accounting defect (`convergence_auto`, `ST-HIGH-1`), and one feeds a wrong fallback
  price into options decisions (`ST-HIGH-2`).
- The strategies **documented as headline features** (`stat_arb_pairs`, `cross_perp_skew`) have **no
  consumers at all**; two others (`spatial_price_arb`, `funding_rate_arb`) reach only a CLI simulation.
  The desktop re-implements spatial and funding economics separately in `commands.rs` and `funding.rs`.
- Several **live** economics gates are computed against fiction: an 8 % hardcoded borrow rate
  (`TG-M6`), a spot-leg detector that never matches so borrow cost is always zero (`TG-M1`),
  funding entries with no settlement-boundary awareness (`ST-MEDIUM-5`), a hardcoded 8 h funding
  interval (`EX2-L4`), and production constants that CI never exercises (`TG-H6`).
- The **risk shield the READMEs describe does not exist in the call graph** (32 conformal modules,
  0 callers).

A defensible next step is not another feature. It is: fix Phase 1 and Phase 2, then run the live
strategies in the smallest size the venues allow, for long enough to measure realised (not modelled)
net-of-fee P&L per strategy, with `pnl.total.*` no longer corrupted by `TS-H3`. Until that measurement
exists, the system's sophistication is a hypothesis, not an asset.

---

*Document ends. All eleven review areas have reported; §7's counts are final. The two severity
corrections recorded in §7 are the only places where the lead overrode an area reviewer, and both were
independently confirmed by a second reviewer.*

---
