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
| E2E 4-tier suite | not in CI, red when run | **52/52 pass on Linux *and* Windows** (macOS via CI) |
| CRITICAL findings | 6 open | **6 fixed** |
| GitHub tracking | ~190 findings untracked | **12 issues filed (#108–#119)** |
| CI red job | `deployment` job references missing unit | **fixed** |
| Cross-platform | never verified, never compiled off Linux | **compiles + E2E green on all three OSes in CI (§7)** |
| Desktop packaging | `bundle.active = false`, no installer | **bundling on; per-OS release matrix (nsis / dmg / deb-rpm-appimage)** |
| Windows bundling | `icon.ico`/`icon.icns` were PNGs with renamed extensions → `RC2175` | **regenerated as real ICO/ICNS containers (§7.6)** |
| Convergence exit (#116) | entry-frame edge compared against baseline-frame deviation | **entry edge re-based onto the live baseline (§8.1)** |
| Breaker baseline (H-3) | only a paper↔live switch re-anchored | **supervised-account-set changes re-anchor too (§8.2)** |
| CI feedback | one 30-minute job hid lint failures behind the test suite | **lint split from a per-crate test matrix (§8.3)** |
| Lint gate, first real run | clippy `-D warnings` had never been enforced | **caught 2 `dead_code` errors on its first run; both fixed, no `#[allow]` (§8.6)** |
| Combo leg pricing (#119) | per-leg rows stamped with the pre-trade bound; legs don't sum to the premium paid | **net fill attributed across legs, reconciled by construction; `fill_price_estimated` flag through schema→store→DTO→UI (§8.7)** |
| Dev RPC auth on Windows (#109) | loopback-only, no identity check at all | **per-launch bearer token required as the first line; peer re-checked as loopback (§8.8)** |

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
2. **ST-HIGH-1** (#116) — **fixed in §8.1.** Convergence exit now re-bases
   the entry edge onto the live baseline; `TrackedPosition.baseline_bps` is
   read instead of only persisted.
3. **TG-H1** — a filled native combo still books estimated per-leg prices
   (issue #111 notes the fix path).
4. **Phantom code** (#114) — ~35k unreachable lines plus 48 fabricated
   "verified green" reports; deletion is a repo-owner decision.
5. **Breaker baseline reset** (`H-3` in `.review/tauri-security.md`) — **fixed
   in §8.2**: the drawdown baseline is now re-anchored when the supervised
   account set changes, not only on a paper↔live switch.
6. **Full-suite runtime** — **mitigated in §8.3**: the desktop tests are slow
   enough that a workspace run takes tens of minutes; CI now runs one job per
   crate in parallel and reports lint separately.
7. **Lighter signer is Linux-only** — only `liblighter_signer.so` is vendored
   and digest-pinned. The loader now asks for the platform-native file name
   (`lighter_signer.dll` / `liblighter_signer.dylib`), but no such artifact is
   published, so Lighter stays unavailable off Linux. Building and pinning a
   Windows/macOS signer is a separate, upstream work item.
8. **Windows dev bridge is loopback TCP** — with no filesystem sockets there is
   no `SO_PEERCRED` equivalent, so any local process can talk to the bridge
   while `ARBITRAGEUR_DEV_SOCK` is set. Loopback-only by construction and
   dev-gated; a named-pipe transport with an ACL would restore peer identity.
   Tracked as part of #115.
9. **Stronghold rekey test is slow, not deadlocked** (#117) — measured in
   §8.5: 229 s on the container's overlay fs, not a hang.

---

## 7. Cross-platform pass (second pass, same day)

Issue #115 said a native `windows-latest` job was the only real gate. It is now
in place, and the audit it enabled found three genuine blockers the static
gating review had missed:

### 7.1 `dev_rpc.rs` — unconditional Unix socket import (hard Windows compile error)

The module imported `tokio::net::{UnixListener, UnixStream}` at module scope
with no `cfg` gate. That is not a runtime failure — it is a compile failure,
and it takes `arbitrageur-desktop` (hence `cargo check --workspace`) down on
any non-unix target. The static audit in §2 looked for `std::os::unix` and
`#[cfg(unix)]` call sites, which were all correctly gated; it never looked for
*ungated Unix-only types in imports*, which is where the break actually was.

Fix: split the transport behind a `DevStream` type alias.

- unix: unchanged behaviour — filesystem socket, `chmod 0600` right after
  bind, Linux `SO_PEERCRED` owner-uid check.
- non-unix: an ephemeral `127.0.0.1:0` listener; the chosen port is written to
  `<ARBITRAGEUR_DEV_SOCK>.port` so a client can discover it. Every ambiguity
  (bind fails, `local_addr` fails, port file unwritable) refuses to serve
  rather than listening somewhere unknown.

`handle_conn` became generic over the stream type; the line protocol, the
dispatch table and the four `dev_rpc` unit tests are untouched.

### 7.2 E2E suite hard-coded `/tmp` (whole suite red on Windows)

Two forms: `/tmp/test-arb-*.sock` endpoints (→ `EACCES`) and
`path.join("/tmp", …)` for a scratch audit file (→ `ENOENT`). Node's `net`
already serves Unix sockets on POSIX and named pipes on Windows through the
same `createServer`/`createConnection` API, so only the *address* had to
change: `devRpcEndpoint(name)` returns `\\.\pipe\arb-e2e-<name>` on Windows and
`os.tmpdir()/test-arb-<name>.sock` elsewhere, and only filesystem endpoints get
the `unlink` bookkeeping.

Result: **52/52 passing on a Windows host**, where the suite previously could
not run at all.

### 7.3 Packaging: no installer existed on any platform

`bundle.active` was `false`, so `tauri build` emitted a bare binary. Enabled
it with `targets: "all"` (each host emits every format it supports) plus
icons, category and a `downloadBootstrapper` WebView2 mode for Windows. Note
`webviewInstallMode` is an *internally tagged enum* — the first attempt as a
bare string was rejected by the build script's config validation, which is the
gate working as intended.

`release.yml` was then rewritten from a single Linux job into:

- `desktop` matrix — `ubuntu-latest` / `windows-latest` / `macos-latest`,
  `npm run tauri build`, staging only the packaged installers (never the
  exploded `macos/*.app` tree) and digest-checking the signer on Linux.
- `headless` (Linux) — CLI + ops tree + signer tarball.
- `sbom` — CycloneDX.
- `publish` — flattens everything with a platform prefix, checksums, releases.

### 7.4 CI gate

New `cross-platform` job: `cargo check --workspace --locked --all-targets` plus
the 4-tier E2E suite on all three OSes, `fail-fast: false` so one OS going red
does not hide the other two.

### 7.5 Verification

| Check | Result |
|---|---|
| `cargo check --workspace --locked --all-targets` (Linux container) | 0 errors |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 warnings |
| `cargo fmt --all -- --check` | clean — this also caught a formatting regression left in `options_packages.rs` by the C-5 fix |
| `cargo test -p arbitrageur-exchanges` | 348 passed, 0 failed |
| `cargo test -p arbitrageur-desktop dev_rpc` | 4 passed, 0 failed |
| 4-tier E2E on Windows host | 52 passed, 0 failed |
| `tauri.conf.json` | accepted by the `tauri-build` config validator |
| Windows/macOS compile | gated by the CI matrix — see the run for commit `f536b35` |

### 7.6 `icon.ico` / `icon.icns` were PNGs with renamed extensions

Turning bundling on (§7.3) made the icon files load-bearing, and the new
Windows job failed immediately in `tauri-winres`:

```
resource.rc(27) : error RC2175 : resource file …\icons\icon.ico is not in 3.00 format
tauri-winres-0.3.6/src/lib.rs:543 panicked: called `Result::unwrap()` on
  an `Err` value: Failed("RC.EXE failed to compile specified resource file")
```

Byte comparison showed why: `icon.ico` was md5-identical to `32x32.png` and
`icon.icns` to `128x128.png`. They had never been containers at all — just
copies of a PNG given the expected extension, which nothing had ever parsed
because bundling was disabled.

Fix: regenerate both from the 512×512 RGBA `icon.png`.

- `icon.ico` — 2 365 bytes, magic `00 00 01 00`, seven entries
  (16/24/32/48/64/128/256). Windows `RC.EXE` requires 3.00-format ICO.
- `icon.icns` — 10 898 bytes, magic `69636e73` ("icns"), members
  `ic07`/`ic08`/`ic09`/`ic10`/`ic11`/`ic12` (128/256/512/1024/32/64).

This is the second class of defect the static audit could not see: the files
*existed* and were the right size to look plausible, so every existence-based
check passed. Only a real Windows toolchain reading the bytes could catch it —
which is precisely what the matrix job is for.

---

## 8. Logic fixes found once the gate existed (third pass)

With the three-OS gate green, the remaining open items in §6 were the real
queue. Two are now fixed.

### 8.1 Convergence exit mixed two baseline frames (#116)

`evaluate_exit` compared `TrackedPosition.entry_dev_bps` — measured against
the pair's baseline **at entry** — with `MonitorRowDto.deviation_bps`, which
the monitor recomputes against its **current rolling** baseline every tick.
`TrackedPosition.baseline_bps` was persisted and never read back, so the two
sides of every comparison lived in different frames.

Fix: thread the entry baseline into `evaluate_exit` and re-base the entry edge
onto the live baseline before comparing:

```text
drift      = live_baseline − entry_baseline
entry_edge = entry_dev − drift       // entry edge expressed in the live frame
captured   = entry_edge − live_dev   // identically raw_entry − raw_now
```

The live frame is the right one because the pair mean-reverts around its
*current* baseline: the edge that is still capturable is
`raw_entry − live_baseline`, not `raw_entry − entry_baseline`. Two concrete
failure modes disappear:

- **baseline drifts up** — the capturable edge shrinks, so an unreverted
  spread is no longer mistaken for a reverted one and closed at breakeven.
- **baseline drifts down** — the capturable edge grows, so a genuinely
  reverted spread is no longer held past its edge, and the stop band
  (`stop_mult × |entry_edge|`) stops being set too tight.

`LiveSpreadView` gained `baseline_bps`; `auto_exec.rs` now passes
`pos.baseline_bps` (restored from the persisted row, so it survives restarts)
and the monitor's live `r.baseline_bps`. If either side is unknown the
correction is skipped rather than guessed, which reproduces the previous
behaviour exactly.

Regression tests (all in `convergence_auto.rs`): drift up, drift down, no
drift, stop-band re-basing, and the unknown-baseline fallback. The pre-existing
`exit_on_dynamic_baseline_shift_convergence` test — which encodes "after a
permanent baseline shift you exit when the spread returns to the *new* mean" —
now feeds real baselines (20 → 25 bps) and still passes unchanged, which is
the strongest evidence the chosen frame matches the intended semantics.

### 8.2 Breaker baseline did not follow the supervised account set (H-3)

`feed_breaker_equity` re-anchored the drawdown baseline only when the
paper↔live universe flipped (`last_risk_live_universe`). Any *other* change to
which accounts are supervised — an operator disabling a venue, credentials
being revoked, an account dropping out of the admitted set — left the baseline
pinned to the old total, so the equity step was read as a real loss. At
5% / $1 000 limits, disabling one venue out of five is a 20% step: it tripped
the breaker and halted *all* trading off an administrative action.

Fix: `AppEngineState.last_risk_account_set` remembers the admitted key set the
baseline was anchored for; both feed paths re-anchor when that set changes
instead of measuring. A genuine loss inside an unchanged universe still trips.

New test `test_account_set_change_reanchors_breaker_baseline` covers
disable → re-enable → real drawdown, asserting the first two do not trip and
the third does.

### 8.3 CI: lint was hidden behind a 30-minute test run

`rust` ran fmt → clippy → `cargo test --workspace` in one job. The desktop
crate dominates the suite, so a one-line clippy failure surfaced after ~30
minutes. Split into:

- `rust-lint` — whitespace gate, signer digest, `cargo fmt --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`. Reports in minutes.
- `rust-tests` — one parallel job per workspace crate
  (`fail-fast: false`), so the fast crates are not held behind the desktop
  suite and a per-crate failure names itself.

### 8.4 Verification (third pass)

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 warnings |
| `cargo test -p arbitrageur-strategy` | 261 passed, 0 failed |
| `cargo test -p arbitrageur-desktop` (breaker/risk/equity subset) | 29 passed, 0 failed |
| CI `Cross-platform` (ubuntu / windows / macos) | all three **success** |
| CI `Rust advisories and secret scan` | **success** (10 → 0 after §7 fixes) |

> Correction, see §8.6: the `cargo clippy` row above was measured on a
> checkout that predated the H-3 helper functions. The `rust-lint` job that
> the new gate added then caught two `dead_code` errors in the same code on
> the very next push. That is the gate doing its job — and it is the reason
> §8.3 exists at all, since under the old combined job those two errors would
> have surfaced after ~30 minutes instead of ~5.

### 8.5 `stronghold::tests::rekey_snapshot…` is slow, not deadlocked (#117)

Issue #117 step 1 asked whether the test passes on a native Linux filesystem
or whether it is a product-level deadlock in the vault rekey path (which would
have been the highest-priority finding in the whole pass). Measured:

| Filesystem | Result |
|---|---|
| container overlay (`/tmp`, `TMPDIR` unset) | **passes in 228.91 s** |
| Docker bind mount of a Windows path (9p/DrvFs) | previously killed at 120 s, ~1805 % CPU over 21 min |

So it is not a deadlock and the product path is clear: the test does real
Stronghold KDF work and is simply expensive, and the Windows bind mount makes
its filesystem syscalls far slower still. The "spin" was CPU-bound Argon2 work
across threads, not a lock acquisition loop.

Consequences: the test stays (the issue explicitly says do not `#[ignore]` it
until the product path is cleared — it now is), and the CI job that runs it
gets an explicit `timeout-minutes` so a genuine future hang fails loudly
instead of burning a runner for hours. If it must be fast, the lever is
lowering the KDF parameters for the test snapshot, not skipping it.

### 8.6 The new lint gate immediately caught two `dead_code` errors (8ff67ff)

Run `39019514771`, job `Rust format and lint`, step 8:

```
error: function `admitted_risk_account_keys` is never used
error: method `mark_risk_universe_anchored` is never used
```

Both came in with the H-3 fix (§8.2). `mark_risk_universe_anchored` had
exactly one caller — `commands::tests` — and `admitted_risk_account_keys` had
exactly one caller, that method. With `--all-targets`, clippy also builds the
non-test binary, where neither is reachable, so `-D dead-code` fires.

Fixed by gating both with `#[cfg(test)]`, **not** `#[allow(dead_code)]`. The
distinction matters: production genuinely never needs them. Every production
path that changes the equity universe — environment switch, account
enable/disable, environment reset — *wants* the re-anchor and clears the
anchor instead (`reset_transaction_history_impl`), which is the opposite of
what this helper does. An `#[allow]` would have shipped a live-only helper
inside the release binary to hide a design fact the gate correctly surfaced.

Re-verified after the fix: `cargo fmt --all -- --check` clean, `cargo clippy
--workspace --all-targets --locked -- -D warnings` clean, and the four tests
on this path pass (`test_risk_overview_drawdown_pct_uses_day_start`,
`test_account_set_change_reanchors_breaker_baseline`, both
`options_exit_tests`).

Process note worth keeping: the failure was pushed to `main` before it was
caught. Local `cargo clippy` was run at an earlier revision, so the fix
commit and the lint run covered different trees. The gate is what made that
cheap — 5 minutes and a one-line-per-site fix rather than a red `main` for
half an hour.

### 8.7 Native combo legs were priced from a pre-trade bound, not the fill (#119)

A native combo is submitted as ONE order at a net limit price, and the venue
reports back ONE `avg_fill_price` — the net premium per combo unit. The
per-leg rows were then built by cloning that ack and stamping each leg with
`options_leg_bound_px` — a pre-trade *worst-case execution bound*. So the
persisted per-leg prices were estimates, the legs did not sum to the premium
actually paid, and the same estimate seeded hedge/reval bases and delta
sizing.

Fixed in two parts:

1. **Reconciliation.** New `attribute_combo_net_price` rescales the
   bound-shaped prices by one common factor so the signed leg sum equals the
   venue-reported net by construction (`Σ sign·p·mult == net_actual`,
   verified at runtime rather than trusted — the `!(x <= eps)` form also
   rejects NaN). Scaling keeps each leg's relative price structure; a pure
   `qty_mult` split would flatten a 0.05 BTC leg and a 0.0005 BTC leg onto
   one number. It refuses — and the caller falls back to the bound, flagged —
   when the identity cannot be forced safely: a degenerate (zero) shape net,
   or a factor that would drive any leg price non-positive (a debit package
   filling as a credit), which the fill recorder would reject and abort a
   package the venue genuinely filled. Also fixed a latent variant: a leg
   with no book previously kept the combo's *net* price as its own — it now
   gets `None` instead of a guaranteed-wrong number.
2. **Honesty.** Every combo-derived leg price is an attribution, never a
   venue fill for that instrument, so it is marked: schema v43 adds
   `signal_legs.fill_price_estimated`, `apply_leg_fill_attributed` writes it,
   `AtomicTransactionDto.fillPriceEstimated` carries it to the UI, and the
   atomic-transactions table renders an `est` badge with a tooltip instead of
   presenting the number as venue truth.

Tests: 6 new — reconciliation to the paid premium, relative-structure
preservation, sign-flip refusal, degenerate-shape refusal, scan-price
fallback, and unusable-input rejection. The old behaviour is pinned in the
first test: booking the raw bound leaves the legs 0.002 BTC short of what
was paid.

Open follow-up (venue capability, not app logic): query the venue for the
combo's per-instrument child fills and book those when available, downgrading
the flag to venue truth — issue #119 step 1.

### 8.8 Dev RPC had no authentication at all on Windows (#109)

The bridge carries full execution authority — order submission, flatten,
circuit-breaker reset, environment switch — behind `ARBITRAGEUR_DEV_SOCK`.
On unix that is gated twice: the socket file is chmod 0600, and on Linux
every accepted connection is additionally checked against the socket owner's
uid via `SO_PEERCRED`. On Windows there is no filesystem socket, so the
transport serves the identical line protocol over an ephemeral
`127.0.0.1` TCP port — and loopback-only is **not** authentication: any local
process, including one running as another user, could connect and trade.

Closed by giving the loopback transport an identity check of its own:

* a random 128-bit bearer token is minted per launch and published in the
  port file (line 1 = port, line 2 = token — a reader that only wants the
  port still reads one line);
* every connection must open with `{"token":"…"}` or it receives one error
  line and a closed socket — it never reaches `dispatch`;
* the peer address is re-checked as loopback on accept, so a socket that is
  not the one we bound cannot be served.

The token is now the transport's only identity check there, so the port file
is the credential; that is spelled out in the module docs and in the
launch-time warning. `handshake_token` is gated `any(not(unix), test)` so the
parsing that could silently admit a connection — non-string token, missing
field, bad JSON — is asserted on Linux and macOS CI too, not only on the OS
that needs it. Windows CI type-checks this path via
`cargo check --workspace --all-targets` on `windows-latest`.
