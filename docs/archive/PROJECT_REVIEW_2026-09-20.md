# Arbitrageur Project Review

- **Date:** 2026-09-20
- **Reviewer:** DeepSeek (provenance supplied by the repository owner)
- **Review type:** Read-only alignment, documentation, code, and verification audit
- **Repository:** `/mnt/Data/Projects/Arbitrageur`
- **Status:** Review only. No fixes were applied by this document.

---

## 1. Purpose

This document compares the system's stated intent and historical user requirements against the current repository state.

It is intended to be reviewed and cross-checked before any remediation work begins.

The review covers:

- All project-linked Codex conversations
- The consolidated 72-entry handoff history
- Repository documentation
- Rust and Tauri backend code
- React frontend code
- Build, test, formatting, lint, and dependency-audit results

---

## 2. Sources Reviewed

### Conversations and history

- 7 project-linked Codex session rollouts
- 185 user prompts extracted from those sessions
- `docs/HANDOFF_MASTER_SESSION_HISTORY.md`, covering 72 session entries

### Documentation

- `README.md`
- `SPEC.md`
- `docs/ARCHITECTURE.md`
- `docs/EXCHANGES.md`
- `docs/HANDOFF_MASTER_SESSION_HISTORY.md`
- `docs/QUANT_RESEARCH.md`
- `docs/REBUILD_GUIDE.md`
- `docs/REFERENCE_PROJECTS.md`
- `docs/RUNBOOK.md`
- `docs/STRATEGY_RESEARCH.md`
- `ops/deploy/README.md`

### Code

- Rust workspace crates under `crates/`
- Tauri backend under `frontend/src-tauri/src/`
- React frontend under `frontend/src/`
- Deployment files under `ops/`

### Repository history

- Current `main` and `origin/main`
- Commit history and reflog
- Recent session work and uncommitted changes

---

## 3. Verification Results

| Check | Result | Notes |
|---|---|---|
| `cargo test --workspace` | PASS | All workspace tests passed; 317 desktop tests reported |
| `npm test` | PASS | Frontend service, type, hook, and component tests passed |
| `npm run build` | PASS | TypeScript and Vite production build passed |
| `npm audit --omit=dev` | PASS | 0 known production vulnerabilities reported |
| `git diff --check` | PASS | No whitespace errors in the current diff |
| `cargo fmt --all -- --check` | FAIL | Large formatting drift across the repository |
| `cargo clippy --workspace --all-targets -- -D warnings` | FAIL | 2 lint errors in `arbitrageur-math` |
| CI workflow present | NO | No `.github` workflow found |
| Rust toolchain pinned | NO | No `rust-toolchain.toml` found |
| Rust formatting config | NO | No `rustfmt.toml` found |
| Dependency policy | NO | No `deny.toml` or equivalent found |
| Security policy | NO | No `SECURITY.md` found |

### Clippy failures

File: `crates/arbitrageur-math/src/funding_carry.rs`

1. Negated comparison on a partially ordered type at line 568.
2. Unnecessary parentheses around an index expression at line 592.

The same unused-parentheses warning appears during `cargo test`.

---

## 4. Executive Conclusion

The overall architectural direction matches the user's intent:

- Rust core with modular crates
- Multi-venue adapters
- Tauri desktop terminal
- Credential vault
- Risk controls
- Historical persistence
- Fail-closed behavior
- Honest handling of missing data
- Funding, basis, spatial, triangular, prediction, and statistical-arbitrage research paths

However, the repository is not internally aligned today.

The system is currently a hybrid:

- Some venue adapters can genuinely sign, submit, and cancel live orders.
- Spatial execution has a live two-leg path.
- Funding, spot, triangular, cross-ticker, prediction, and autonomous bot paths are paper-only or disabled.
- Several docs still describe a paper-only four-venue system.
- Basis opportunities are scanned and displayed, but their execution path is not wired to the actual spot and perpetual legs.

The most important correction is not a feature build. It is a **truth and capability alignment pass**, followed by a targeted basis-execution fix.

---

## 5. Findings

## 5.1 [P0] Safety and capability documentation is materially stale

### Evidence

`README.md` currently states:

> “Live trading is disabled and fails closed. Order signing, submission, and cancellation are not implemented for any venue.”

Reference: `README.md:21`

This is no longer true. Adapter-level live submission is implemented for:

- Hyperliquid
- Lighter
- Robinhood Lighter deployment
- Vanta / Orderly
- Aster
- PopDEX
- Binance
- OKX
- Kraken
- Coinbase
- Decibel

`README.md` also says the desktop app admits only Hyperliquid, Lighter,
Robinhood Lighter, and Vanta, and that PopDEX is still a scaffold.

References:

- `README.md:22`
- `README.md:23`
- `README.md:152`

`SPEC.md` repeats the same stale position:

- `SPEC.md:9`
- `SPEC.md:19`
- `SPEC.md:227`
- `SPEC.md:482`
- `SPEC.md:811`

`docs/REBUILD_GUIDE.md` also says nothing touches real funds and that native
execution is disabled backend-wide.

Reference: `docs/REBUILD_GUIDE.md:24`

### Why this matters

This is the highest-risk documentation defect because it makes the system look
less capable of spending real money than it actually is. The documentation is
not conservatively overselling safety; it is factually understating live
capability.

### Required correction

Replace the current status language with an explicit capability matrix covering:

- Whether the venue adapter can sign
- Whether it can submit
- Whether it can cancel
- Whether the strategy path can actually use it live
- Which paths remain paper-only
- Which paths fail closed and why

### Acceptance criteria

- No document claims that live execution is universally disabled.
- No document claims that live order submission is implemented for no venues.
- Every venue and strategy is classified as live-capable, paper-only, disabled,
  unsupported, or roadmap-only.
- The README, SPEC, handoff backlog, rebuild guide, and in-app copy agree.

---

## 5.2 [P0] Basis trades can be scanned but cannot be executed

### Evidence

The funding scanner emits same-exchange basis opportunities with display labels:

```rust
long_venue: format!("{venue_label} SPOT"),
short_venue: format!("{venue_label} PERP"),
```

Reference: `frontend/src-tauri/src/commands.rs:3098`

Execution later resolves venue IDs through `canonical_venue_id`.

Reference: `frontend/src-tauri/src/state.rs:1065`

That function only accepts canonical IDs and supported aliases such as
`lighter`. It does not accept labels such as `Lighter SPOT` or `Lighter PERP`.

The execution path then calls `get_adapter(long_venue)` and fails with:

> `Venue '... SPOT' is not admitted for paper execution`

Reference: `frontend/src-tauri/src/commands.rs:4963`

There is a second, deeper execution defect: the funding executor resolves both
legs from the same symbol. It does not select the `BTC-SPOT` market for the
spot leg and the perp market for the perpetual leg.

Spot execution is also independently hard-disabled.

Reference: `frontend/src-tauri/src/commands.rs:1786`

The auto-carry loop selects the first qualifying opportunity without excluding
basis rows.

Reference: `frontend/src/components/FundingRadar.tsx:834`

If a basis row is selected first, the loop can repeatedly retry an
unexecutable opportunity and delay or starve real carry opportunities.

### Why this matters

The user explicitly requested same-exchange spot/perpetual basis trading.
Scanning and displaying the opportunity without a correct two-leg execution
path creates a false impression that the request is complete.

### Required correction

Separate the following concepts in the execution contract:

- Venue identity
- Market type
- Spot symbol
- Perpetual symbol
- Account / margin domain
- Direction

The execution path must resolve the correct orderbook and paper or live leg for
each side of the basis trade.

### Acceptance criteria

- A basis opportunity can be executed end-to-end in paper mode.
- The spot leg uses the spot market and symbol.
- The perpetual leg uses the perpetual market and symbol.
- The same-venue basis flow does not mis-map to the perp book twice.
- The execution log records both legs correctly.
- A regression test covers basis scanning and execution as one workflow.
- Auto-carry does not repeatedly select an unexecutable basis row.

---

## 5.3 [P1] Live support exists at the adapter layer but not across all strategies

### Current strategy-level state

| Strategy / path | Current state |
|---|---|
| Spatial futures | Live path exists |
| Funding carry | Paper-only; live explicitly refused |
| Spot | Hard-disabled |
| Triangular | Paper-only |
| Cross-ticker | Hard-disabled |
| Prediction | Hard-disabled |
| Autonomous convergence bot | Hard-blocked in live mode |
| Passive unwind | Paper-only |
| External position close | Requires separate review; do not assume live-safe |

### Evidence

- Spatial live path: `frontend/src-tauri/src/commands.rs:3772`
- Funding live refusal: `frontend/src-tauri/src/commands.rs:4935`
- Spot disabled: `frontend/src-tauri/src/commands.rs:1786`
- Cross-ticker disabled: `frontend/src-tauri/src/commands.rs:4866`
- Prediction disabled: `frontend/src-tauri/src/commands.rs:5362`
- Bot live hard block: `crates/arbitrageur-strategy/src/convergence_auto.rs:343`
- Passive unwind paper-only rationale: `docs/HANDOFF_MASTER_SESSION_HISTORY.md:1476`

### Why this matters

The user repeatedly asked to keep working until full support existed for all
included exchanges and to add non-Rust libraries when needed.

The adapter work is substantial and genuine, but adapter capability alone does
not equal strategy support. A user can enable live mode and still find most
strategy paths unavailable.

### Required decision

Decide explicitly whether the intended product state is:

1. **Live spatial-only for now**, with every other path clearly labelled
   paper-only or disabled, or
2. **Live across funding, spatial, basis, and automation**, requiring a unified
   two-leg dispatch and reconciliation path.

### Acceptance criteria

- A single capability matrix exists for strategy × venue × mode.
- Every UI execute button matches the backend's actual capability.
- Live mode cannot select or imply a strategy path that will always fail.
- Any remaining live gap is named in the UI and in the docs.

---

## 5.4 [P1] Uncommitted frontend work can be lost

### Current working tree

Three files are modified but not committed:

- `frontend/src/components/EngineSettings.tsx`
- `frontend/src/components/ExchangeAccounts.tsx`
- `frontend/src/components/PortfolioMonitor.tsx`

The changes include:

- Removal of the manual Lock Vault button
- Improved NO_CREDENTIALS / LOCKED / STALE balance guidance
- STALE and LOCKED status handling in Portfolio Monitor

The frontend tests and production build pass with these changes, but they are
not present in `HEAD` or `origin/main`.

### Why this matters

A branch switch, reset, or future cleanup can discard work that the user may
believe is already committed.

### Required correction

Review the diff, then either commit it with an intentional message or revert it
if it is no longer wanted.

### Acceptance criteria

- No unexplained modified tracked files remain.
- The final state matches the last documented handoff.
- Commit history contains the reviewed UI changes.

---

## 5.5 [P2] Formatting, lint, and CI are not enforced

### Evidence

- `cargo fmt --all -- --check` fails across many files.
- `cargo clippy --workspace --all-targets -- -D warnings` fails with two
  warnings in `crates/arbitrageur-math/src/funding_carry.rs`.
- No `.github` CI workflow exists.
- No `rust-toolchain.toml` exists.
- No `rustfmt.toml` exists.
- No dependency-policy configuration exists.

### Why this matters

The project has a complex live-trading codebase, but nothing prevents future
commits from arriving unformatted, warning-producing, untested, or dependent on
an unpinned Rust version.

### Required correction

Add CI gates for:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `npm test`
- `npm run build`
- Dependency and secret scanning where practical

Pin the Rust toolchain and add a formatting configuration.

### Acceptance criteria

- A clean checkout passes all gates.
- CI runs on every pull request and push to `main`.
- Formatting and lint failures block merge.
- The toolchain version is explicit and reproducible.

---

## 5.6 [P2] In-app copy still describes the old paper-only state

`FundingRadar.tsx` contains comments and user-facing hints saying native
execution is disabled backend-wide.

References:

- `frontend/src/components/FundingRadar.tsx:806`
- `frontend/src/components/FundingRadar.tsx:1185`

This is misleading in a hybrid live/paper system.

### Required correction

Replace blanket statements with context-specific text such as:

- “Live spatial execution is available when the vault is unlocked and live
  mode is enabled.”
- “Funding execution is paper-only until two-leg live dispatch and
  reconciliation are implemented.”
- “Prediction execution is not available.”

### Acceptance criteria

- No user-facing text implies a capability state that the backend contradicts.
- Empty-state and tooltip text names the exact reason for unavailability.

---

## 5.7 [P2] Exchange scope remains ambiguous

The user previously requested removal of exchanges without native API support.
Current `main` excludes the 23 long-tail venues and does not admit Arcus,
Polymarket, Kalshi, IBKR, or IG for execution.

However, the venue-status surface still contains entries for several of these
venues as disabled or scaffolded options.

### Why this matters

The user cannot tell whether these are:

- Intentionally removed
- Roadmap placeholders
- Future integrations
- Unsupported but retained for display

### Required decision

Choose one policy:

1. Remove them from the UI and status surface entirely.
2. Keep them visible only in a clearly labelled “Roadmap / not supported”
   section.
3. Re-admit specific venues only after a native API and execution path exist.

### Acceptance criteria

- The venue list has one unambiguous meaning.
- No disabled venue appears in an execution selector.
- The docs and UI use the same venue inventory.

---

## 5.8 [P2] The Tauri command layer is becoming a maintainability bottleneck

### Largest files

| File | Lines |
|---|---:|
| `frontend/src-tauri/src/commands.rs` | 18,559 |
| `frontend/src-tauri/src/state_store.rs` | 6,079 |
| `frontend/src-tauri/src/monitor.rs` | 3,773 |
| `frontend/src-tauri/src/state.rs` | 2,070 |

### Why this matters

The Rust crates are modular, but the Tauri integration layer is a monolith.
This increases:

- Review difficulty
- Merge-conflict risk
- Test isolation difficulty
- Risk of cross-cutting regressions
- Onboarding cost for the owner's rebuild practice goal

### Recommended direction

Split by domain, not by arbitrary size:

- Funding commands
- Spatial commands
- Triangle / spot commands
- Portfolio and account commands
- Risk and settings commands
- Execution-log and retention commands

No immediate behavior change is required.

### Acceptance criteria

- New features can be added to a domain module without editing one very large
  command file.
- Cross-domain tests remain co-located with their behavior.

---

## 5.9 [P3] Vendored signer binary has no automated integrity gate

### Evidence

- `crates/arbitrageur-exchanges/native/lighter/liblighter_signer.so`
- Approximately 13 MB
- ELF shared object
- Not stripped
- Loaded through `libloading`
- Search path allows `LIGHTER_SIGNER_LIB`
- Sidecar next to the executable is also allowed

### Why this matters

The binary is part of the live signing path. Without an automated hash or
signature check, a modified binary could be loaded without detection.

### Required correction

- Publish the expected SHA-256 in source or deployment metadata.
- Verify the hash before loading.
- Pin the upstream `lighter-go` version or commit.
- Consider stripping debug symbols for release builds.
- Add a CI check that the vendored artifact matches the approved hash.

### Acceptance criteria

- The signer library cannot silently change.
- CI fails if the binary differs from the approved artifact.
- The provenance and build instructions are documented.

---

## 6. Alignment Against the User's Stated Goals

| User goal | Current state | Alignment verdict |
|---|---|---|
| Multi-venue Rust arbitrage engine | Implemented | Aligned |
| Spatial price arbitrage | Live path exists | Mostly aligned |
| Funding-rate arbitrage | Scan and paper execution exist | Partially aligned; live missing |
| Same-exchange spot/perp basis | Scanner exists | Not aligned; execution broken |
| Prediction-market arbitrage | Math and adapters exist | Research/scaffold only |
| Traditional statistical arbitrage | Math and strategy code exist | Research/partial |
| Live support across included venues | Submittable adapters exist | Adapter-aligned, strategy-incomplete |
| Honest fail-closed behavior | Broadly implemented | Aligned |
| Risk controls and circuit breaker | Implemented | Aligned |
| Credential vault | Implemented | Aligned |
| Logging and failure persistence | Implemented | Aligned |
| Inter font and consistent UI tokens | Implemented | Aligned |
| Detailed rebuild guidance | Guide exists | Partially aligned; legacy comments incomplete |
| Remove venues without native API support | Current main excludes long-tail venues | Mostly aligned, but scaffold entries remain |
| Full support for all included exchanges | Mixed by strategy | Not yet achieved |

---

## 7. Recommended Remediation Order

### Phase 0 — Cross-check this document

Do not implement fixes until this review has been checked against the intended
product scope.

### Phase 1 — Truth and capability alignment

- Rewrite README safety status.
- Rewrite SPEC status sections.
- Update rebuild guide.
- Update handoff backlog/open items.
- Correct stale in-app copy.
- Publish one capability matrix for venue × strategy × mode.

### Phase 2 — Basis execution

- Add explicit market-type and symbol fields for each basis leg.
- Resolve spot and perpetual adapters/books correctly.
- Make same-venue basis execution work in paper mode.
- Add an end-to-end regression test.
- Prevent auto-carry from retrying unexecutable basis rows.

### Phase 3 — Live-scope decision

- Choose either live spatial-only or live across funding/spatial/automation.
- Wire remaining live paths through one reconciled dispatcher if full coverage
  is the target.
- Otherwise, label every remaining path as paper-only or disabled.

### Phase 4 — Engineering hygiene

- Commit reviewed uncommitted work.
- Fix formatting.
- Fix the two clippy errors.
- Add CI.
- Pin the Rust toolchain.
- Add dependency and secret scanning.

### Phase 5 — Exchange and security cleanup

- Decide the final venue inventory.
- Remove or label roadmap-only venues.
- Add signer-binary integrity verification.
- Add a security policy and operational threat model.

---

## 8. Open Decisions for the Owner

1. Is live spatial execution the intended current scope, with all other paths
   explicitly paper-only?
2. Should same-exchange basis trading be paper-only first, or live-capable in
   the same release?
3. Should funding and the autonomous convergence bot become live-capable?
4. Should Arcus, Polymarket, Kalshi, IBKR, and IG be removed or retained as
   clearly labelled roadmap entries?
5. Is prediction-market implementation a current priority or a future phase?
6. Should the next work package focus on correctness, live coverage, or
   documentation truth first?

---

## 9. Review Status

This document is intentionally read-only.

No source files were modified while producing this review.

The next step is owner cross-check and approval of:

- Finding severity
- Intended live/paper scope
- Venue inventory
- Remediation order

After that, fixes can be implemented in small, reviewable commits.
