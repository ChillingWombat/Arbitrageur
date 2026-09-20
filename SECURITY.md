# Security Policy

## Project status

Arbitrageur is a research and paper-trading system. Adapter-level signing and order submission exist for several venues, but new live strategy entries are disabled until terminal fill and restart reconciliation are implemented and validated. See `docs/CAPABILITIES.md`.

Do not use this repository as a production trading system or assume that an accepted order acknowledgement means a fill.

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

Security fixes target the current `main` branch. There are no supported release tags yet.

## Credential model

- Credentials are encrypted locally in `~/.arbitrageur/vault.enc` with AES-256-GCM and an Argon2id-derived key.
- On Unix, the application enforces `0700` on the data directory and `0600` on the vault and SQLite files.
- The operational SQLite database is local but is not application-encrypted.
- Secret reveal is field-scoped and requires an unlocked vault.
- Use API keys with the minimum required permissions and no withdrawal permission.
- Prefer delegated/agent keys over a main wallet key where the venue supports them.
- Rotate a credential immediately if it appears in source control, terminal output, screenshots, logs, shell history, or a bug report.

## Live-order safety boundary

Signing and submission capability do not establish live strategy readiness. Live strategy entry requires:

- Durable in-flight persistence before submission
- Terminal fill/partial/cancel/expiry reconciliation
- Idempotent order identity
- Reduce-only compensation
- Correct leverage and margin mode
- Restart recovery
- Venue sandbox and owner-approved canary evidence

Until all requirements are met, the backend must refuse new live strategy entries. Authenticated account monitoring and explicitly risk-reducing submissions are separate capabilities; the operator must confirm their terminal state directly at the venue.

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
