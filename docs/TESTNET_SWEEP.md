# Testnet Validation Sweep (Issue #27e)

> Manual validation pass across every testnet-capable venue the desktop
> admits, against real venue demo endpoints. This is a runbook, not an
> automated suite — the point is credentialed, signed-order evidence on
> real venue infrastructure. Automated coverage (`#[cfg(test)]` sims,
> stub-socket adapters) proves the code paths; this sweep proves the
> venues.

## Scope

| Venue | Testnet endpoint | Key acquisition | Notes |
|---|---|---|---|
| Binance USDⓈ-M Futures | `testnet.binancefuture.com` | testnet.binancefuture.com account → API Management | Full REST; thin books on some symbols |
| Hyperliquid | `api.hyperliquid-testnet.xyz` | Testnet wallet (app.hyperliquid-testnet.xyz), builder fee | Wallet-signature auth |
| Lighter | `testnet.zklighter.elliot.ai` | Lighter testnet app → API keys (account/api key index) | Key-index scoped |
| OKX demo | `www.okx.com` + `x-simulated-trading: 1` | OKX account → demo trading → demo API key | **Regional keys**: key minted under `us.okx.com` etc. only authenticates on that domain. Demo keys only, never mainnet keys. |
| Vanta (Orderly) | `testnet-api.orderly.org` | Orderly testnet app → orderly key + account | Orderly network account |
| Decibel | testnet via `for_testnet` | Decibel testnet app | Aptos-based |
| PopDEX | testnet profile | PopDEX testnet app | Alpha-stage venue |
| Deribit | `test.deribit.com` | test.deribit.com account → API keys | Options + perp; very thin testnet books — expect empty/one-sided depth |
| Alpaca | `paper-api.alpaca.markets` | Alpaca dashboard → Paper Trading → API keys | Real venue sandbox: separate host + separate key namespace. Crypto spot legs only. |

Not in scope (no public testnet — must never be simulated locally):
Coinbase, Aster, Kraken, Robinhood, the DexPerp long-tail (GRVT, EdgeX,
Variational, dYdX, etc.), EVM AMMs, Boros (mainnet-only protocol).

## Preconditions

1. Release binary built with `cargo build --release --bin arbitrageur-desktop --features custom-protocol`.
2. Environment = **testnet** (Settings → Trading Environment). Verify the
   header badge reads TESTNET.
3. Credential vault initialized + unlocked.
4. Per venue: account row added under Exchange Accounts with the venue's
   testnet credentials; the account's environment must be `testnet`.

## Per-venue sweep

For each venue, in order — record evidence per step:

1. **Admission** — enable the venue flag (Engine Settings) or rely on the
   enabled testnet account profile; `get_system_status` shows the venue
   `READ_ONLY` or better, never `UNSUPPORTED`.
2. **Market data** — Ticker Log / matrix shows live books for at least one
   liquid symbol (BTC-USD*). Funding rate panel reports the venue's real
   rate + interval (HL 1h, others 8h) — check the 8h normalization on the
   Funding tab (`net_rate_8h_pct` must differ correctly across intervals).
3. **Credentials** — account probe (`fetch_account`) returns real demo
   equity; a deliberately wrong key fails visibly with an auth error, not
   a generic timeout.
4. **Order identity** — manual small-size entry on a carry/spatial pair
   where this venue is a leg: the persisted signal leg must carry the
   venue's real order id (not a synthetic one) and `fetch_order` must
   resolve it to a terminal state.
5. **Reconciliation** — if any leg acks non-terminal: verify the +5s
   supervisor polls it to terminal, or the leg is cancelled + re-polled,
   and the signal resolves Balanced/Imbalanced/Unresolved — never silently
   Success.
6. **Partial-cycle** — with one leg's book pulled thin (pick a stale
   symbol), confirm the executor unwinds the filled leg rather than
   leaving naked exposure; the signal row must read Imbalanced/Failed with
   the residual surfaced, and `flatten` must clear it.

## Cross-cutting checks

- **USD quote modes** (`usd_quote_mode` setting): run one scan each under
  `parity`, `separate`, and `fx_adjusted` on a pair where both venues
  quote BTC in different stablecoins (e.g. USDT-margined vs USDC-margined).
  `separate` must not pair them; `parity` pairs at face value;
  `fx_adjusted` pairs and applies the FX factor. The matrix/`hub_key`
  slot separation is observable via differing book rows under `separate`.
- **Depeg gate**: with a stablecoin quote mode active, confirm that a
  halted/depegged stablecoin quote blocks entry for that leg
  (`stablecoin_gate` refusal surfaces as an explicit error, not a skip).
  USDC/USDT-vs-USD legs stay exempt (they are the depeg arb).
- **Fee truth**: `get_venue_fees` IPC values must match what fills report
  in `fee`/`fee_currency`; scan-time `round_trip_fee_bps` must equal the
  sum of both legs' IPC taker bps.
- **Vault rotation**: after `change_vault_password`, every EVM-signing
  venue (Boros/AMM paths) still signs — or the wallet is visibly marked
  unprovisioned. No silent signer failure.
- **Restart recovery**: leave a working (non-IOC) order open, restart the
  app — the startup reconciler must pick it up from persisted order ids.

## Evidence

Per venue record: venue, endpoint, account label (never the keys), order
ids for each dispatched leg, signal ids + final statuses, screenshots of
the venue's own UI showing the position/order, and any refusal messages.
File under `docs/testnet-sweep/<venue>-<date>.md` or equivalent.

## Exit criteria

A venue graduates from "pending venue evidence" when at least one
signed, reconciled round-trip exists on its real demo endpoint with order
ids that resolve at the venue's own API. Venues without demo endpoints
stay gated out of live strategy entry regardless — no amount of local
simulation substitutes.
