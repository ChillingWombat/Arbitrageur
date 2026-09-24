# Security Policy

## Project status

Arbitrageur is a research and trading system. Adapter-level signing, order submission, and reconciled live strategy entries exist for many venues — `live_entry_gate` dispatches entries whenever the vault is unlocked and each leg's adapter passes submission/polling/strategy-entry/credential checks. Trading Mode = Live is the operator opt-in. See `docs/CAPABILITIES.md`.

Do not assume that an accepted order acknowledgement means a fill, and do not treat the reconciliation machinery as a substitute for venue-side verification.

## Reporting a vulnerability

Report vulnerabilities privately through GitHub's private security-advisory mechanism for this repository when available. Include:

- Affected commit and file/symbol
- Reproduction steps with secrets removed
- Expected and observed behavior
- Potential impact
- A minimal proof of concept or failing test
- Suggested mitigation, if known

Do not include private keys, API secrets, vault files, authentication headers, funded account identifiers, or unredacted production logs. If private reporting is unavailable, open a minimal public issue asking for a private contact channel without disclosing exploit details.

## Supported code

Security fixes target the current `main` branch. Tagged releases
(`v*`) are built by `.github/workflows/release.yml` — each publishes the
trading binary, headless CLI, digest-pinned Lighter signer sidecar,
SHA-256 checksums, and a CycloneDX SBOM. Verify downloads against
`SHA256SUMS.txt` before installing.

## Credential model

- Credentials are encrypted locally in `~/.arbitrageur/vault.enc` with AES-256-GCM and an Argon2id-derived key.
- On Unix, the application enforces `0700` on the data directory and `0600` on the vault and SQLite files.
- The operational SQLite database is local but is not application-encrypted.
- Secret reveal is field-scoped and requires an unlocked vault.
- Use API keys with the minimum required permissions and no withdrawal permission.
- Prefer delegated/agent keys over a main wallet key where the venue supports them.
- Rotate a credential immediately if it appears in source control, terminal output, screenshots, logs, shell history, or a bug report.

## Live-order safety boundary

Signing and submission capability do not by themselves establish live strategy readiness. `live_entry_gate` admits a live strategy entry only when all of the following hold for every leg's adapter:

- Durable in-flight persistence before submission
- Terminal fill/partial/cancel/expiry reconciliation via `fetch_order` polling
- Idempotent order identity (persisted client order IDs)
- Reduce-only compensation on residual legs
- Restart recovery of durable working orders
- The adapter's automated strategy-entry transport is implemented
- Live credentials are loaded for the active environment

When any requirement fails, the backend refuses the entry at `live_entry_gate`. Operator approval and canary evidence remain the operator's responsibility — the gate checks capability, not judgment. Authenticated account monitoring and explicitly risk-reducing submissions are separate capabilities; the operator must confirm their terminal state directly at the venue.

## Vendored signer

The Lighter signing path loads a vendored native library through FFI. The loader verifies the approved SHA-256 before loading. A custom `LIGHTER_SIGNER_LIB` requires an explicit matching `LIGHTER_SIGNER_SHA256`.

A checksum detects drift; it does not establish provenance. Updating the library requires:

1. Pinning and recording the upstream source commit.
2. Rebuilding from documented source in an isolated environment.
3. Reviewing exported symbols and dependency metadata.
4. Updating the approved digest and CI integrity check in the same reviewed change.
5. Re-running signer vectors and sandbox order tests.

## Local and network exposure

- Public and authenticated venue requests disclose the operator's IP address, requested instruments, timing, and possibly account identifiers to venues and infrastructure providers.
- The frontend contains no analytics SDK, but venue, RPC, operating-system, and network providers remain observable dependencies.
- The headless `serve` command is a read-only `503 not_ready` health placeholder. It does not ingest data or trade.
- Non-loopback daemon binding requires an explicit override and should be protected by host firewalling and an authenticated reverse proxy if ever exposed.

## Operational response

If unexplained live exposure is detected:

1. Pause new execution and leave market/account reads running if safe.
2. Inspect the venue directly; do not trust a local zero-fill acknowledgement as proof of no order.
3. Cancel working orders and flatten residual exposure using the venue's native interface when necessary.
4. Record venue order IDs, fills, timestamps, and redacted errors.
5. Rotate affected credentials if compromise is possible.
6. Preserve the SQLite database and logs for diagnosis; do not publish them unredacted.

## Security verification

Before merging a security-sensitive change, run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd frontend && npm test && npm run build && npm audit --omit=dev
```

CI additionally checks Rust advisories, secret leakage, and the approved native-signer digest.
