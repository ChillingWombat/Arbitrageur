# Arbitrageur — Agent Rules

## Live mode is the product

- **Every feature request means live mode first.** Real venues, real orders,
  real money. Paper mode is secondary and its existence never counts as the
  feature being "done."
- Never ship a capability that works in paper but not live unless the user
  explicitly scoped it that way. If a live path is unsafe or blocked, say so
  and gate it — don't silently deliver paper-only.
- Paper mode is slated to move to **venue testnets** (real order flow against
  testnet endpoints). Keep the mode abstraction clean for this: `is_live_mode`
  will eventually become a venue-environment selection, not a boolean. Every
  exchange adapter already accepts `base_url: Option<String>` — preserve that
  injection point; do not hardcode mainnet URLs into new code.
- Testnet availability per venue is uneven (Binance/Hyperliquid/Lighter/OKX
  have public testnets; Robinhood crypto does not). Venue-specific gaps are
  acceptable — silent simulation in place of testnet is not.

## Audit discipline

- Before answering "does X exist / how does X work," **enumerate the module
  contents first** (`ls` the crate/dir, read `lib.rs` mod lists), then grep.
  Feature names in this codebase are idiosyncratic ("passive unwind" not
  "limit exit") — vocabulary-based searches produce false negatives.
- When reporting "no X exists," state what was searched and what would have
  been found if it existed.

## Venue quirks

- **OKX regional entities.** OKX runs separate API-key namespaces per entity
  domain: `www.okx.com` (global), `app.okx.com` / `us.okx.com` (US & AU),
  `eea.okx.com` / `my.okx.com` (EEA), `tr.okx.com` (TR). A key issued on one
  returns `50119 "API key doesn't exist"` on every other domain — check which
  host the user's browser shows before debugging credentials. Per-account
  `baseUrl` (Region / API Domain dropdown) overrides the adapter's host;
  stored on `ExchangeCredentials.base_url`, applied to public and signed
  calls alike. OKX demo keys additionally need `x-simulated-trading: 1` —
  not implemented.

## Execution safety (standing)

- Fail-closed on live paths: unverifiable order state → stop, don't stack
  resting orders; never claim fills from submit acknowledgements.
- Venue truth via `fetch_order` polling; order identity is durable state.
- Emergency exits stay market. Everything else follows the configured policy.

## Verification

- Frontend tests: `cd frontend && npm test` (node --test + scripts/test-components.mjs). NOT vitest.
- Backend: `cargo test -p arbitrageur-desktop`, `cargo clippy`, `cargo fmt --check`.
- Release binary: `cargo build --release --bin arbitrageur-desktop --features custom-protocol`; restart via `setsid nohup`.
  Without `custom-protocol`, Tauri compiles a dev-mode binary whose webview loads
  `devUrl` (localhost:1420) → "Could not connect to localhost" unless vite is running.
