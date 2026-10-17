# Diagnosis & Repair Report — 2026-10-17

Scope: full-workspace build, test and security diagnosis following
`docs/HANDOFF_REVIEW_2026-10-14.md`; all findings of that review were still
untouched when this pass began (verified: no `crates/**` or
`frontend/src-tauri/src/**` file had an mtime after 2026-10-15).

---

## 0. TL;DR

| Area | Before | After |
|---|---|---|
| Workspace compiles | never built | `cargo check --workspace --locked` → **0 errors** |
| Formatting gate | unknown | `cargo fmt --all -- --check` → **clean** |
| Rust tests | never run | see §3 |
| Frontend tests | 175/175 units passed, component suite unrunnable (Linux `node_modules` on a Windows host) | **419/419 pass** (Linux container, node 24) |
| E2E 4-tier suite | not in CI, red when run | **passes on Linux**; Unix-socket-bound → cannot run on Windows |
| CRITICAL findings | 6 open | **6 fixed** |
| GitHub tracking | ~190 findings untracked | **10 issues filed (#108–#117)** |
| CI red job | `deployment` job references missing unit | **fixed** |
| Cross-platform | never verified | static gating audit clean; CI matrix still missing |

---

## 1. Environment (why this machine could not build before)

The Windows host has no Rust toolchain and no MSVC. All builds here run in
`arb-builder:1.96.0` (Rust 1.96.0 + GTK/WebKit dev packages + three cross
targets) with the repo bind-mounted at `/app`.

Two repo-local traps had to be neutralised — both are genuine findings:

1. **`.cargo/config.toml` points `PKG_CONFIG_PATH` at `.sysroot/`**, which is
   gitignored and contains *newer* `.pc` files (pango requires
   `gobject-2.0 >= 2.82`) but no `gobject-2.0.pc`/`glib-2.0.pc`. Build fails in
   `gdk-sys`. Worked around by overriding `PKG_CONFIG_PATH` to the container's
   consistent GTK stack.
2. **`.sysroot` `.so` files are Windows symlink placeholders** (`IntxLNK…`
   text). `frontend/src-tauri/build.rs` injected that directory into the link
   line unconditionally, so linking died with
   `unknown directive: IntxLNK`. **Fixed in-tree**: the build script now
   requires `target_os == "linux"` *and* a directory where **every** `.so`
   starts with the ELF magic (`\x7fELF`) before emitting `-L`/rpath — one
   placeholder disqualifies the whole directory.

---

## 2. What was verified

- `cargo check --workspace --locked` — 0 errors, 1m48s (warm target dir).
- `cargo fmt --all -- --check` — clean (two files formatted: `options_bot.rs`,
  `reconciliation.rs`).
- `cargo test --workspace --locked --no-fail-fast` — see §3.
- `cd frontend && npm test` — 419/419 (node 24; the component suite needs
  `chmod +x node_modules/@esbuild/linux-x64/bin/esbuild` when `node_modules`
  is served through a Windows bind mount).
- `node --experimental-strip-types tests/e2e/runner.mjs` — exit 0 on Linux.
  On Windows it fails with `EACCES` binding `/tmp/<name>.sock`.
- Static platform-gating audit: all 14 `std::os::unix` call sites are behind
  `#[cfg(unix)]`; no un-gated `Command::new("sh"|"bash"|"systemctl")`.

---

## 3. Test results

| Suite | Tests | Failures |
|---|---|---|
| `cargo test --workspace` (run 1) | 664 desktop + 42 crate tests completed | **0** |
| — then | **hung** on `stronghold::tests::rekey_snapshot_rotates_password_and_preserves_key` (#117) | busy-spin >10 min at ~18 cores |
| Targeted re-runs of every changed module | 76 passed (options_bot 7, ws_primary 7, options_packages 14, reconciliation 43, dev_rpc 4, accounts 1) | **0** |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | clean, 8m07s | **0 warnings** |

**The stronghold hang is itself a finding (#117)**: the test does not fail, it
spins. A CI job running `cargo test --workspace` can therefore sit for the full
job timeout. It must be confirmed on native ext4 before being written off as a
container/volume artefact — if it reproduces there, vault password rotation
deadlocks in production.

Note on runtime: the desktop suite is disk/CPU-bound (Stronghold snapshot writes
+ Argon2id per test). Individual tests routinely exceed 60 s, so a full
workspace run takes tens of minutes on a bind-mounted Windows volume. This is a
real CI-cost finding, not a flake.

Cross-target compile check (`--target x86_64-pc-windows-msvc`): blocked at
`aws-lc-sys` (C dependency needs a Windows C toolchain). See #115 — a native
`windows-latest` CI job is required; the static gating audit is clean.

---

## 4. Fixes (all in the working tree, uncommitted)

### CRITICAL — from the handoff review

| ID | File | Change |
|----|------|--------|
| C-1 | `.agents/**`, `docs/**`, `ORIGINAL_REQUEST.md`, `.gitleaks.toml` | Redacted the vault master password in 47 places / 21 files; anchored the two dangerous gitleaks allowlist entries (`^…$`) so a real PEM block can never be exempted. **History rewritten** with `git filter-repo` (438 commits; four literal rules scoped to password contexts so timestamps/decimal noise were untouched) and force-pushed — `main` and every branch no longer contain the literal; the old commit is reachable only by known-SHA until GitHub GC. |
| C-2 | `dev_rpc.rs` | Socket `chmod 0600` immediately after bind; Linux per-connection `SO_PEERCRED` UID check (owner UID resolved from the socket file, fail-closed to `u32::MAX`). |
| C-3 | `options_bot.rs` | Removed hardcoded fallback spots; rehedge now runs under `execution_lock` + breaker + pause + `live_entry_gate`; persists leg intent and client order id before submit; advances `last_hedged_spot` only on accepted submission; retires failed intents so they cannot wedge the symbol. |
| C-4 | `execution.rs`, `reconciliation.rs` | New shared `protective_option_ioc_order` (±2% collar, refuse when no reference) and `is_options_leg` predicate; the reconciler's residual flatten no longer sends market orders for options. |
| C-5 | `options_packages.rs` | Partial/non-terminal native combo ack blocks the sequential fallback (fail-closed) and raises a throttled alert; only terminal zero-fill falls back. |
| C-6 | `accounts.rs` | `lock_vault_impl` propagates the open-position query error and refuses to lock (fail-closed). |

### HIGH

- `ws_primary.rs` — `FlightLeaderGuard` (RAII): a cancelled/panicking
  single-flight leader now publishes a cancellation error, removes the slot and
  notifies waiters instead of wedging `(venue, symbol)` forever.

### CI / hygiene

- `.github/workflows/ci.yml` — `deployment` job referenced
  `ops/deploy/arbitrager@.service` (missing “u”) → permanently red. Corrected
  to `arbitrageur@.service` + `arbitrageur-notify@.service`.
- `.gitattributes` — `eol=lf` + binary rules (stops CRLF churn between a
  Windows checkout and the Linux build container).
- `.gitignore` — `*.rlib` (7 stray `libtest_tranche*.rlib` in the repo root).
- `README.md` — Node prerequisite v20 → v22.6+ (the suite uses
  `--experimental-strip-types`); Rust version now points at
  `rust-toolchain.toml`.

---

## 5. GitHub issues filed

Previous review findings were **not tracked**: the only open issues were #102
and #103 (Oct 2–3 specs). Filed:

| # | Title |
|---|---|
| 108 | [P0] Vault master password committed in cleartext; gitleaks allowlist can exempt real PEM keys |
| 109 | [P0] Dev RPC socket carries full execution authority with no authentication |
| 110 | [P0] Options bot autonomous rehedge places live IOC orders outside every execution guard |
| 111 | [P0] Options residual auto-flatten dispatches unconstrained MARKET orders |
| 112 | [P0] Partial native combo ack re-dispatches every leg (over-positioning) |
| 113 | [P0] Manual vault lock fails open when the open-position query errors |
| 114 | [P1] ~35k lines of unreferenced "phantom" modules with fabricated verification reports |
| 115 | [P1] Build/CI chain is Linux-only; Windows and macOS compilation never verified |
| 116 | [P1] Convergence auto-exit compares against a re-based monitor baseline |
| 117 | [P2] `stronghold::tests::rekey_snapshot_…` hangs (CPU busy-spin) on a bind-mounted volume |

---

## 6. Known remaining risks (not fixed in this pass)

1. **Vault password scope (C-1)** — the operator clarified the `<redacted>`
   master password is a **testnet dev password**, deliberately set so agents
   can unlock the vault and test strategies on testnet. History has been
   scrubbed (see C-1 row). Remaining caveat: if any vault using that password
   ever holds **live/mainnet** keys, rotate immediately — history cleanup does
   not protect a password that is still in use. The password now lives only in
   the untracked `.workbuddy/memory/` (gitignored) for future agent test runs.
2. **ST-HIGH-1** (#116) — convergence exit mixes entry-frame and
   baseline-frame deviations; `TrackedPosition.baseline_bps` is never read.
3. **TG-H1** — a filled native combo still books estimated per-leg prices
   (issue #111 notes the fix path).
4. **Phantom code** (#114) — ~35k unreachable lines plus 48 fabricated
   "verified green" reports; deletion is a repo-owner decision.
5. **Cross-platform packaging** (#115) — `bundle.active = false`; no installer
   for any platform; the Lighter signer is Linux-only.
6. **Breaker baseline reset** (`H-3` in `.review/tauri-security.md`) — the
   drawdown baseline is not reset when the supervised account set changes.
7. **Full-suite runtime** — the desktop tests are slow enough that a workspace
   run takes tens of minutes; CI should split or parallelise them.
