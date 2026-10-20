# Make environment part of durable state and PnL identity

Status: accepted.

## Context

Live and Testnet use different credentials and venue endpoints, but both submit real orders and receive real venue fills. Durable accounts, signals, legs, monitor rows, exits, cycle lots, and profit counters previously allowed rows without an environment or mixed them during reads. That can surface testnet exposure in Live, hide live exposure after an environment switch, and overwrite lifetime profit with a value derived from the current open ledger.

## Decision

`TradingEnvironment::{Testnet, Live}` is part of the identity of every environment-sensitive durable row. Writes stamp `environment`; reads filter by the active environment. Legacy rows with `unknown` remain visible in both environments so unresolved exposure still blocks and gets reconciliation. They are never guessed into Live or Testnet.

Each environment has its own adapter set, credential resolution, account admission, signal query, cycle lots, scheduled exits, breaker inputs, and `pnl.total.{environment}` counter.

The cumulative PnL counter is authoritative for lifetime realized profit. It changes only from attributed fills and fees, or an explicit environment-scoped history reset. UI totals do not recompute lifetime profit from currently open lots, and a missing value stays unavailable rather than becoming zero.

Reduce-only and reconciliation paths preserve the environment of the signal they resolve. They never adopt an adapter or ledger row from the other environment.

## Alternatives considered

- Keep one global ledger and filter only in the UI. Rejected: API callers and supervisors can still act on the wrong environment's exposure.
- Guess legacy rows from venue or credential names. Rejected: venue keys can exist in both environments, and a wrong guess can hide real exposure.
- Rebuild lifetime PnL from open and closed lots at read time. Rejected: retention, partial fills, legacy rows, and external positions make that value incomplete; presenting it as lifetime profit is false precision.
- Reintroduce a local paper environment. Rejected: Testnet is a real venue demo path with the same submission and reconciliation semantics.

## Consequences

Every environment-sensitive schema and query carries additional identity and migration complexity. `unknown` rows can appear in both environments and may block more than once, which is intentional fail-closed behavior. Cross-environment totals are not a single number by default; any future aggregate must be explicit and keep the source environments visible.
