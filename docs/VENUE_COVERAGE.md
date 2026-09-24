# Venue Coverage Matrix — CoinGecko top-40 perp DEX + top-100 spot DEX

Snapshot: **2026-09-22**. Sources: CoinGecko derivatives exchange list (API
snapshot, ordered by open interest) and `coingecko.com/en/exchanges/decentralized`
(spot DEX, ordered by 24h volume). CoinGecko's "decentralized" derivatives page
lists **all** derivatives venues (CEX futures included), so the perp table below
marks which entries are actually DEX/hybrid vs centralized.

Legend — **Verdict**:
- `supported` — adapter exists in this repo already
- `added` — new adapter added by this work
- `excluded-*` — not integrated; reason stated (dead endpoint, no trading API,
  custody risk, duplicate platform, CEX out of scope)

Legend — **Custody** (can the venue take user funds if it shuts down):
- `low` — self-custody / on-chain settlement, user keeps keys
- `med` — hybrid: operator matches orders but cannot move collateral without a
  user signature
- `high` — custodial or exploit/opacity concerns
- `cex` — centralized exchange (full custody)

## 1. Perpetual DEX review (CoinGecko derivatives list, by OI)

The "decentralized" derivatives page is dominated by centralized futures venues.
Every row below is the full top-40 list plus the genuine perp DEXes that appear
later in the ranking. CEX futures entries are listed for completeness but are
out of scope (this engine's strategy venues are DEX/hybrid first).

| CG rank | Venue | Type | Custody | API / agent-wallet trading? | Verdict |
| --- | --- | --- | --- | --- | --- |
| 1 | Binance (Futures) | CEX | cex | yes (REST, HMAC) | supported (`binance`) |
| 2 | CoinW (Futures) | CEX | cex | yes | excluded-cex |
| 3 | Hyperliquid | perp DEX (L1) | low | yes (signed REST actions + agent keys) | supported (`hyperliquid`) |
| 4 | Bybit | CEX | cex | yes | excluded-cex |
| 5 | MEXC (Futures) | CEX | cex | yes | excluded-cex |
| 6 | Gate (Futures) | CEX | cex | yes | excluded-cex |
| 7 | WEEX (Futures) | CEX | cex | yes | excluded-cex |
| 8 | XT.COM | CEX | cex | yes | excluded-cex |
| 9 | Tapbit | CEX | cex | yes | excluded-cex |
| 10 | Bitget | CEX | cex | yes | excluded-cex |
| 11 | OKX (Futures) | CEX | cex | yes | supported (`okx`) |
| 12 | Toobit | CEX | cex | yes | excluded-cex |
| 13 | OrangeX | CEX | cex | yes | excluded-cex |
| 14 | WhiteBIT | CEX | cex | yes | excluded-cex |
| 15 | CoinUp.io | CEX | cex | yes | excluded-cex |
| 16 | BingX | CEX | cex | yes | excluded-cex |
| 17 | HTX | CEX | cex | yes | excluded-cex |
| 18 | LBank | CEX | cex | yes | excluded-cex |
| 19 | KuCoin | CEX | cex | yes | excluded-cex |
| 20 | Ourbit | CEX | cex | yes | excluded-cex |
| 21 | Bitmart | CEX | cex | yes | excluded-cex |
| 22 | Deribit | CEX (options/fut) | cex | yes | excluded-cex |
| 23 | Aster | perp DEX (hybrid) | med | yes (REST + agent wallet) | supported (`aster`) |
| 24 | Deepcoin | CEX | cex | yes | excluded-cex |
| 25 | Phemex | CEX | cex | yes | excluded-cex |
| 26 | Bitunix | CEX | cex | yes | excluded-cex |
| 27 | BTSE | CEX | cex | yes | excluded-cex |
| 28 | Crypto.com | CEX | cex | yes | excluded-cex |
| 29 | Variational Omni | perp DEX (RFQ) | low | **no orderbook API** — RFQ/quote flow only | excluded-no-api |
| 30 | PrimeXBT | CEX | cex | yes | excluded-cex |
| 31 | BVOX | CEX | cex | yes | excluded-cex |
| 32 | BitKan | CEX (aggregator) | cex | yes | excluded-cex |
| 33 | Lighter | perp DEX (zk) | low | yes (signed tx API) | supported (`lighter`) |
| 34 | BloFin | CEX | cex | yes | excluded-cex |
| 35 | BULK | perp DEX (early) | med | API docs thin; endpoint serves unrelated page | added (`bulk`) — **market-data-only**, fail-closed parsers; trading excluded |
| 36 | Bitfinex | CEX | cex | yes | excluded-cex |
| 37 | EVEDEX | perp DEX | med | SIWE→JWT auth (`auth-api.evedex.com`) + EIP-712 order sigs on Arbitrum One (`exchange-api.evedex.com`) | added (`evedex`) — trading implemented |
| 38 | KCEX | CEX | cex | yes | excluded-cex |
| 39 | Hotcoin | CEX | cex | yes | excluded-cex |
| 40 | Zoomex | CEX | cex | yes | excluded-cex |

### Genuine perp DEXes below the top-40 (same list, continued)

| CG rank | Venue | Custody | API / agent-wallet trading? | Verdict |
| --- | --- | --- | --- | --- |
| 41 | edgeX | med (StarkEx-style L2) | Perp V2 REST on `edgex-prod-v2.edgex.exchange`: HMAC api-key auth + EIP-712 `LimitOrderParams` order sigs (secp256k1 — no Stark curve) | added (`edgex`) — trading implemented |
| 43 | GRVT | low (ZKsync hyperchain, trustless orders) | REST + session cookie + EIP-712 order sig | added (`grvt`) — trading implemented |
| 44 | Robinhood Chain Lighter | low | same rails as `lighter` | supported (`robinhood` = Lighter adapter) |
| 45 | Antarctic | med | no verified public API | added (`antarctic`) — **market-data-only**, doc-shaped parsers fail closed; trading excluded |
| 48 | QFEX | med (6-person team) | API-key HMAC REST + authenticated trade WS order entry | added (`qfex`) — trading implemented |
| 50 | GMX V2 (Solana) | low | contract calls only, no orderbook | added (`gmx`) — **market-data-only** (prices-only: GLV AMM has no CLOB); trading excluded |
| 51 | Extended | low | REST + SNIP-12 Poseidon `Order` signature + X-Api-Key | added (`extended`) — trading implemented (Stark-curve signing ported from the official X10 fast_stark_crypto spec, verified against its test vectors) |
| 52 | Ostium | **high** — ~$18M oracle exploit, paused | onchain only | added (`ostium`) — **market-data-only**, fail-closed parsers; trading excluded |
| 54 | ApeX Omni | low (zkLink settle) | REST + apiKey HMAC + zkLink L2 sig | added (`apex`) — market data live; trading fail-closed pending zkLink signing |
| 55 | SoDEX | med | parked domain | added (`sodex`) — **market-data-only**, fail-closed parsers; trading excluded |
| 56 | Pacifica | med (ex-FTX team) | REST + ed25519-signed ops | added (`pacifica`) — trading implemented |
| 58 | AlphaX | **high** (no PoR) | parked domain | added (`alphax`) — **market-data-only**, fail-closed parsers; trading excluded |
| 60 | Derive | low | REST + `X-Lyra*` auth + EIP-712 Action sig | added (`derive`) — trading implemented |
| 61 | Nado | med | Ink L2 gateway (Vertex-style sequencer) + EIP-712 orders; linked signers supported | added (`nado`) — trading implemented |
| 62 | Ondo Perps | med | full REST (SIWE→JWT→orders) | added (`ondoperps`) — trading implemented |
| 63 | IO Trader | med | parked domain | added (`iotrader`) — **market-data-only**, fail-closed parsers; trading excluded |
| 64 | Orderly (Derivatives) | low | same rails as `vanta` | supported (`vanta`) — dedup: one Orderly venue |
| 65 | Vest Markets (prev. Vest Exchange) | med | rebranded → `server-prod.hz.vestmarkets.com/v2`; delegate-signer (agent wallet) + X-API-KEY | added (`vest`) — trading implemented |
| 69 | StandX | med | REST + wallet-sig JWT + ed25519 body sig | added (`standx`) — trading implemented |
| 70 | RISEx | med | no live mainnet | excluded-unverified |
| 72 | dYdX Chain | low | indexer reads + cosmos-tx writes | added (`dydx`) — trading implemented (official `dydx` gRPC client, mnemonic credential) |
| 73 | Niza.fun | med | no futures product found | added (`niza`) — **market-data-only**, fail-closed parsers; trading excluded |
| 74 | Arcus | med | research venue | supported (`arcus`, scaffold) |
| 75 | Flipster | CEX-hybrid | yes | excluded-cex |
| 76 | GMX V2 (Arbitrum) | low | contract only, no orderbook | excluded-no-orderbook |
| 77 | Paradex | low | REST + Stark SNIP-12 sig + JWT; L2 key derived from EVM key (paradex-rs crate) | added (`paradex`) — trading implemented |
| 79 | Astros | low | DeepBook onchain, no REST | added (`astros`) — **market-data-only**, fail-closed parsers; trading excluded |
| 80 | Aevo | med | REST + AEVO-KEY/SECRET + EIP-712 order sig | added (`aevo`) — trading implemented |
| 82 | Gate DEX | med | DNS/endpoint unverified | added (`gatedex`) — **market-data-only**, fail-closed parsers; trading excluded |
| 83 | Decibel | low | REST + Aptos-signed txs | supported (`decibel`) |
| 85 | Helix (Injective) | low | Cosmos txs (keccak SignDoc) via LCD broadcast + indexer gRPC truth | added (`helix`) — trading implemented |
| 89 | Katana Perps | med | app shell only, no public API | excluded-unverified |
| 90 | Synthetix | low | API root unresolvable; frontend-only | excluded-unverified |
| 92 | SynFutures | low | contract calls + auth-gated REST | excluded-contract-only (EVM wallet path possible — tracked) |
| 93 | Strike Finance | med | API 404 | excluded-dead |
| 94 | GMX V2 (Avalanche) | low | dup of GMX platform | excluded-duplicate |
| 95/97/99 | KiloEx (BSC/Base/opBNB) | low | contract calls only | excluded-contract-only (one platform, three deployments) |
| 98 | SparkDEX | med | API live (api.sparkdex.ai) but trading is contract-based on Flare | excluded-contract-only |
| 100 | DeriW | med | API root live, thin docs | excluded-unverified |

### Perp DEX summary

- **Already supported**: Hyperliquid, Lighter (+Robinhood deployment), Aster,
  Vanta/Orderly, PopDEX, Decibel, Arcus (scaffold).
- **Added this pass** (`dex_perps.rs`, wired into `state.rs` via account
  enablement): `pacifica`, `ondoperps`, `standx`, `derive`, `grvt`, `aevo`,
  `vest` (delegate-signer + X-API-KEY on vestmarkets.com), `qfex`
  (HMAC REST + authed trade WS), `nado` (EIP-712 orders on Ink L2 gateway),
  `evedex` (SIWE→JWT + EIP-712 orders on Arbitrum One),
  `paradex` (Stark L2 key derivation + SNIP-12 orders via paradex-rs),
  `edgex` (HMAC api trio + EIP-712 `LimitOrderParams` orders on the
  Perp V2 host),
  `extended` (X-Api-Key + SNIP-12 Poseidon `Order` signatures on the
  StarkEx engine — signing ported from the official X10 Rust crypto and
  verified against its shipped test vectors),
  `dydx` (Cosmos-chain `MsgPlaceOrder`/`MsgCancelOrder` signed and
  broadcast via the official `dydx` gRPC client — credential is the
  account's 24-word BIP-39 mnemonic; indexer `/v4/orders` is order truth),
  `helix` (Injective v2 derivative msgs — `MsgCreateDerivative*Order`/
  `MsgCancelDerivativeOrder` as Cosmos txs signed keccak/`ethsecp256k1`
  over `SignDoc`, broadcast on the chain LCD; indexer gRPC `OrdersHistory`
  is order truth; credential is the account's own EVM private key)
  (full trading implemented — credentials still required before
  `has_live_credentials()` opens order paths);
  `apex`, `gmx`,
  `antarctic`, `bulk`, `ostium`, `sodex`,
  `alphax`, `iotrader`, `niza`, `astros`, `gatedex`
  (market data live; order paths fail closed — see CAPABILITIES.md for the
  exact per-venue capability bits).
- **Hedge venue**: `boros` (Pendle Boros, Arbitrum — YU fixed-rate swaps
  over Binance/Hyperliquid/OKX/Lighter funding; agent-key EIP-712
  `PendleSignTx` via calldata-builder + `/v1/send-txs/bulk-calls`; not a
  perp venue — it powers the funding strategy's 4-leg `fixed_yield`
  variant only. See EXCHANGES.md §13).
- **Excluded**: every centralized futures venue (custody + out of scope),
  Variational (RFQ/no orderbook), KiloEx / SynFutures / SparkDEX
  (contract-only swap UX, no CLOB REST), Orderly-derivative listings
  (same rails as Vanta), RISEx (no live mainnet), and a dozen
  dead/parked/unverifiable venues.

## 2. Spot DEX review (CoinGecko decentralized exchanges, top 100, deduplicated)

CoinGecko lists each *chain deployment and AMM version* as a separate exchange.
Deduplicated by underlying platform, the top-100 rows collapse to roughly **45
platforms**, of which ~10 dominate volume.

### Deduped platform table

| Platform | CG rows (rank) | Chain(s) | Custody | Trading path | Verdict |
| --- | --- | --- | --- | --- | --- |
| **Uniswap** | 2,3,4,11,13,25,28,38,39,42,50,51,52,54,57,73,74,80,90,91,92,95 (≈22 rows) | ETH, Arb, Base, OP, Polygon, BSC, AVAX, Unichain, Robinhood-chain, Monad, Arc, xLayer | low | on-chain swap via Quoter/Router — agent wallet signs | added (`uniswap`, `uniswap_v2`) — UniV3 QuoterV2/SwapRouter02 + UniV2 Router; default chain Arbitrum, RPC injectable |
| **PancakeSwap** | 1,16,17,32,47,62,91 | BSC, ETH, Arb, Base (Solana variant separate) | low | same UniV3/V2 interfaces | added (`pancakeswap`, `pancakeswap_v3`) — BSC + Base + Arb |
| **Aerodrome** | 7,18,60,78 | Base | low | Slipstream quoter (UniV3-compatible) + classic Solidly router | added (`aerodrome`) — market data only |
| **Velodrome** | 65 | Optimism | low | same family as Aerodrome | added (`velodrome`) — market data only |
| **Orca / Raydium / Meteora / PumpSwap / Humidifi / ZeroFi / Byreal / Manifest** | 5,10,19,8,37,35,45,41,15 | Solana | low | Solana tx signing — needs solana SDK integration | excluded-deferred (non-EVM; single biggest missing cluster — ~$1.4B/day combined) |
| **Curve** | 12 | ETH | low | custom AMM math/router | excluded-custom-abi (v2: StableSwapNG quoter) |
| **Fluid** | 14,72 | ETH, Arb | low | custom liquidity-layer ABI | excluded-custom-abi |
| **Ramses** | 31,64,66 | Robinhood-chain, HyperEVM, Polygon | low | Solidly fork | added (`ramses`) — market data only |
| **Pharaoh / Blackhole** | 21,27,79 | Avalanche | low | Solidly forks (same team/lineage) | added (`pharaoh` — market data only; `blackhole` spec'd, not admitted) |
| **Dexalot** | 33 | Avalanche subnet CLOB | low | REST CLOB API exists | excluded-unverified (subnet RPC needed) |
| **THORChain / Chainflip / Near Intents / Bron Intents** | 34,84,22,100 | cross-chain | low | swap intents, not orderbooks | excluded-model-mismatch |
| **Kuru** | 23 | Monad CLOB | low | CLOB API | excluded-unverified |
| **Project X / Nest / Topaz / Saphyre / GIGA / Hydrex / Up / De¹ / Pons / Alandale / Bankr / Temple / Native Core / Subnet Tokens** | various | HyperEVM / Monad / misc L2s | low–med | mostly EVM AMMs on new chains | excluded-unverified (thin docs/endpoints) |
| **DODO** | 26 | BSC | low | PMM — custom ABI | excluded-custom-abi |
| **SunSwap** | 40 | Tron | low | Tron tx signing | excluded-deferred (non-EVM) |
| **Cetus / Turbos / Bluefin / Aftermath** | 43,59,56,71 | Sui | low | Sui tx signing | excluded-deferred (non-EVM) |
| **Ekubo** | 76 (Starknet), 89 (ETH) | Starknet/ETH | low | custom pool math | excluded-custom-abi |
| **Camelot** | 86 | Arbitrum | low | UniV2-style router (V2) | added (`camelot`) — market data only |
| **Quickswap** | 87 | Polygon | low | UniV2-style router | added (`quickswap`) |
| **Sushiswap** | (below 100 cutoff) | multi | low | UniV2/V3-style | added (`sushiswap`) |
| **LFJ (TraderJoe)** | 96 | Avalanche | low | Liquidity-Book bins — custom math | excluded-custom-abi |
| **Tokenlon** | 88 | ETH | med | RFQ | excluded-rfq |
| **Oku Trade** | 83 | Plasma | low | frontend for UniV3-style pools | excluded-duplicate |
| **Hyperliquid spot / Lighter spot / Aster spot / Helix spot** | 6,85,69,68 | native | low/med | same platform adapters | supported (existing venue adapters cover their spot books where listed) |
| **First Ledger / Sologenic** | 67,81 | XRPL | low | XRPL tx signing | excluded-deferred (non-EVM) |
| **Rhea / Near** | 49 | NEAR | low | NEAR tx signing | excluded-deferred |
| **SaucerSwap** | 97 | Hedera | low | Hedera tx signing | excluded-deferred |
| **ThalaSwap** | 98 | Aptos | low | Aptos tx signing | excluded-deferred (`aptos-sdk` already vendored — candidate for next pass) |
| **Aquarius** | 99 | Stellar | low | Stellar tx signing | excluded-deferred |
| **AlphaX / ChangeNOW / Origin ARM** | 20,46,36 | misc | med/high | — | excluded-dead/cex-adjacent |
| **Qυestions… (Subnet Tokens)** | 30 | Bittensor | n/a | not a trading venue | excluded-not-a-venue |

### Spot DEX summary

- **Landed** (`evm_amm.rs`, one generic `EvmAmmAdapter` over a static
  `AmmVenueSpec` table): `uniswap` (UniV3,
  Ethereum), `uniswap_v2` (UniV2, Ethereum), `pancakeswap` (UniV2, BSC),
  `pancakeswap_v3` (UniV3, BSC), `sushiswap` (UniV2, Ethereum),
  `quickswap` (UniV2, Polygon), `camelot` (UniV2-compatible V2 pools,
  Arbitrum), `aerodrome` (Solidly V2 4-field routes, Base), `velodrome`
  (Solidly V2 4-field, Optimism), `ramses` (Solidly legacy 3-field,
  Arbitrum), `pharaoh` (Solidly legacy 3-field, Avalanche). **11 admitted
  for onchain market data.** Of these, the six standard-router venues —
  `uniswap`, `uniswap_v2`, `pancakeswap`, `pancakeswap_v3`, `sushiswap`,
  `quickswap` (`LIVE_EVM_AMM_VENUE_IDS`) — also implement local-wallet
  swap submission; `camelot`, `aerodrome`, `velodrome`, `ramses`,
  `pharaoh` are **market-data-only** (execution-gating rationale below)
  and never load a private key — an optional wallet address enables
  balance reads.
  `blackhole` stays in the spec table with `admit = false` — its legacy
  deployment holds no live major pools (verified on-chain); its Algebra
  CL liquidity needs a dedicated ABI family.
- **Deferred (documented)**: non-EVM clusters (Solana/Sui/Tron/Aptos/XRPL/
  Hedera/Stellar/NEAR/Injective) need per-chain signers; custom-ABI AMMs
  (Curve, Fluid, DODO, LFJ, Ekubo) and Ramses-lineage/Algebra CL pools
  need per-venue quoter work.
- **Excluded**: aggregators/intents (no venue orderbook), RFQ venues, dead
  endpoints, non-venue entries.

## 2b. `evm_amm` implementation (LANDED)

One `EvmAmmAdapter` struct driven by a per-venue `AmmVenueSpec` table —
same shape as `DexPerpAdapter`, but all I/O is on-chain JSON-RPC instead
of venue REST. Every address below was re-verified by live `eth_call`
probes during implementation (factory lookup + a real quote) — see
`live_market_smoke` test (`--ignored`).

### ABI families (all probe-verified)

| Kind | Pool discovery | Quote | Swap entrypoint |
| --- | --- | --- | --- |
| `UniV2` | `factory.getPair(a,b)` | `router.getAmountsOut(amountIn,[a,b])` / `getAmountsIn` | `swapExactTokensForTokens` / `swapTokensForExactTokens` |
| `UniV3` | `factory.getPool(a,b,fee)` | `QuoterV2.quoteExact{Input,Output}Single` (params: `tokenIn,tokenOut,amount,fee,sqrtLimit`) | `SwapRouter.exact{Input,Output}Single` — deadline-in-params variant |
| `Solidly` | `factory.getPool(a,b,stable)` (Aero/Velo) or `getPair(a,b,stable)` (Ramses/Pharaoh) | `router.getAmountsOut(amountIn,Route[])` | `swapExactTokensForTokens(...,Route[])` |

Solidly `Route` has two encodings, probed live: `{from,to,stable}` (3-field,
Ramses/Pharaoh legacy routers) and `{from,to,stable,factory}` (4-field,
Aerodrome/Velodrome V2). The spec carries both flags.

Ramses-lineage V3 pools are keyed by `tickSpacing` (not UniV3 `fee`), and
Algebra pools (Blackhole CL, Camelot V3, QuickSwap V3) drop the fee param
entirely — both need their own quoter/router ABI, so v1 of `evm_amm`
reaches Ramses / Pharaoh through their verified **legacy Solidly** routers
and Camelot through its UniV2-compatible V2 router. RamsesV3/Algebra CL
liquidity is deferred to a follow-up adapter family.

**Execution gating** — only the six standard-router venues
(`LIVE_EVM_AMM_VENUE_IDS`: `uniswap`, `uniswap_v2`, `pancakeswap`,
`pancakeswap_v3`, `sushiswap`, `quickswap`) may submit swaps. Solidly
routers have no exact-output primitive (`getAmountsIn`/
`swapTokensForExactTokens` absent — probed), so a buy could only run as an
exact-in estimate with input headroom, which can overfill the requested
base quantity; Camelot's V2 router uses a custom referrer ABI rather than
the standard UniV2 swap signature. Camelot/Aerodrome/Velodrome/Ramses/
Pharaoh are therefore market-data-only: `place_order` rejects them before
any signer, chain, or quote work, and they never request or load a
private key.

### Interface contract (per venue) — as implemented

- `base_url: Option<String>` carries the **RPC endpoint** override
  (account credential `baseUrl` field → `set_base_url`); spec default is a
  public `publicnode.com` RPC. Endpoint URLs are secrets (provider API
  keys can embed in them): the credential field is masked in Debug and
  zeroized on drop/in adapter storage, and follows the patch protocol —
  an omitted property preserves the stored endpoint, an explicit empty
  string clears it back to the adapter's constructor endpoint, a
  non-empty value sets it. Invalid (non-http(s)) values return `Err`,
  keep the active endpoint, and withhold credentials. Chain id is
  verified against the spec before any read or signing, cached per
  endpoint URL so an endpoint change always re-verifies (wrong-network
  RPC refuses).
- `fetch_markets` resolves each spec pair's pool on-chain and returns only
  pools that resolve non-zero — markets are *proven*, never assumed.
- `fetch_orderbook` synthesizes a documented one-level book from real
  executable quotes (an AMM has no L2 depth): the bid is the exact-in
  quote for one base unit; the ask is the exact-out cost of one base unit
  on the six standard routers, or the reverse exact-in quote (price
  `1/base_out`, ask quantity = the actual quoted `base_out` — never
  normalized to 1) on the five market-data-only venues. Both sides are
  required — a failed side, a zero/nonpositive quote, or a crossed book
  (`ask < bid`) fails the read rather than fabricating a side.
  `fetch_ticker` delegates to this book and uses `bid` as `last`; there
  is no ask→bid fallback.
- `fetch_account` reads native + configured ERC-20 `balanceOf`; `equity`
  prices stables at face and other tokens via venue quotes — unquotable
  holdings stay listed but excluded (no invented prices). `positions`
  empty. Market-data-only venues hold only an optional public wallet
  address for this read — no private key is requested or loaded.
- `fetch_funding_rate` / `set_leverage` fail closed — spot venues have
  neither.
- `place_order` — admitted only on the six standard-router venues
  (`execution_supported()`); the Camelot/Solidly venues reject before any
  signer, chain, or quote work. Market and limit-bound orders only: the
  quoter result must satisfy the limit or the order is refused *without
  submitting* (never rests). Sell = exact-in base→quote; Buy = exact-out
  quote→base. Slippage bound =
  limit price, or quote ±50bps for market orders. ERC-20 `approve(router)`
  issues lazily at the exact needed amount (nonzero→nonzero reset handled
  for USDT-style tokens; no unlimited grants). `estimate_gas` runs before
  every send — a revert fails closed with no nonce burned. On broadcast
  the call returns `New` immediately, keyed by the transaction hash, with
  zero fill and full remaining — the durable order id reaches persisted
  state before any receipt wait or process crash.
- `cancel_order` fails closed — nothing rests on an AMM.
- `fetch_order` is the sole status authority:
  `eth_getTransactionReceipt(hash)` → `Filled` / `Rejected`, decoding
  `Transfer` logs — BOTH the input token spent and the output token
  received must be proven to the signer, and the mined gas fee
  (`gas_used * effective_gas_price`) is reported in the chain's native
  currency. The submit ack alone never counts. Missing receipt = pending
  (error, so reconciliation keeps polling). Order identity is the tx
  hash.
- Capabilities: `supports_live_execution` /
  `live_order_submission_implemented` /
  `order_status_polling_implemented` = `execution_supported()` — true
  only for the six standard-router venues; the five market-data-only
  venues report false on all three.
  `live_strategy_entry_implemented()` is **false for all EVM AMM
  venues** — the submission transports are for explicit canaries only;
  automated multi-leg strategy entry stays backend-refused pending
  gas-aware profitability modeling and credentialed swap evidence.
  `has_live_credentials` requires the
  EVM wallet key (`privateKey` field) on live venues. No API key exists —
  the wallet key *is* the credential.

### Venue spec table (contract addresses — all probe-verified 2026-09-22)

| id | chain | kind | factory / quoter / router |
| --- | --- | --- | --- |
| `uniswap` | Ethereum 1 | UniV3 | F `0x1F98431c8aD98523631AE4a59f267346ea31F984` / Q `0x61fFE014bA17989E743c5F6cB21bF9697530B21e` / R `0xE592427A0AEce92De3Edee1F18E0157C05861564` |
| `uniswap_v2` | Ethereum 1 | UniV2 | F `0x5C69bEe701ef814a2B6a3EDD4B1652CB9cc5aA6f` / R `0x7a250d5630B4cF539739dF2C5dAcb4c659F2488D` |
| `pancakeswap` | BSC 56 | UniV2 | F `0xcA143Ce32Fe78f1f7019d7d551a6402fC5350c73` / R `0x10ED43C718714eb63d5aA57B78B54704E256024E` |
| `pancakeswap_v3` | BSC 56 | UniV3 | F `0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865` / Q `0xB048Bbc1Ee6b733FFfCFb9e9CeF7375518e25997` / R `0x1b81D678ffb9C0263b24A97847620C99d213eB14` |
| `aerodrome` | Base 8453 | Solidly-4 | F `0x420DD381b31aEf6683db6B902084cB0FFECe40Da` / R `0xcF77a3Ba9A5CA399B7c97c74d54e5b1Beb874E43` |
| `velodrome` | Optimism 10 | Solidly-4 | F `0xF1046053aa5682b4F9a81b5481394DA16BE5FF5a` / R `0xa062aE8A9c5e11aaA026fc2670B0D65cCc8B2858` |
| `camelot` | Arbitrum 42161 | UniV2 (V2 pools) | F `0x6EcCab422D763aC031210895C81787E87B43A652` / R `0xc873fEcbd354f5A56E00E710B90EF4201db2448d` |
| `quickswap` | Polygon 137 | UniV2 | F `0x5757371414417b8C6CAad45bAeF941aBc7d3Ab32` / R `0xa5E0829CaCEd8fFDD4De3c43696c57F7D7A678ff` |
| `sushiswap` | Ethereum 1 | UniV2 | F `0xC0AEe478e3658e2610c5F7A4A2E1777cE9e4f2Ac` / R `0xd9e1cE17f2641f24aE83637ab66a2cca9C378B9F` |
| `ramses` | Arbitrum 42161 | Solidly-3 (legacy) | F `0xADd32480630A16dfAcEe6eeFcB3ab2181449Dc3B` / R `0x1614a7e1fe63960B4684867a62080acd2404757f` |
| `pharaoh` | Avalanche 43114 | Solidly-3 (legacy) | F `0x85448bF2F589ab1F56225DF5167c63f57758f8c1` / R `0x9CEE04bDcE127DA7E448A333f006DEFb3d5e38cC` |
| `blackhole` | Avalanche 43114 | Solidly-3 (legacy, **not admitted**) | F `0xfE926062Fb99CA5653080d6C14fE945Ad68c265C` / R `0x9EED160D7D8253DeC1A2A512e504DE5E7ff3C111` |

Both UniV3 venues use the original SwapRouter ABI (deadline inside
params) — Uniswap's canonical `0xE592…` router and PancakeSwap's
`0x1b81D678…` share it, so no per-venue deadline flag is needed.

(Default public RPCs: `*-rpc.publicnode.com` per chain — ethereum / bsc /
polygon-bor / arbitrum-one / base / optimism / avalanche-c-chain — all
overridable per account via the credential `baseUrl` field.)

Market sets are the spec pair tables verified on-chain at
`fetch_markets` — majors only per chain (WETH/WBTC/ARB/OP/BNB/AVAX/POL
vs USDC/USDT where the pool exists). Adding a pair is a spec-row change,
never new code.

**Honest liquidity caveat**: legacy AMM deployments carry thin books where
the venue migrated liquidity to CL pools — Ramses WETH/USDC quoted
~$0.000061/ETH, Camelot V2 ~$313/ETH, Velodrome volatile ~$1,586/ETH at
probe time (the 2026-09-23 live smoke reproduced the same pattern).
The adapter reports those quotes faithfully. These dead/thin legacy pools
are exactly why the Camelot/Solidly venues are gated market-data-only —
the quotes explain the gating, they do not make execution safe.

## 3. What "added" means (capability contract)

Every added adapter reports honest capability flags:

- Market data (markets/book/ticker/funding) — live-verified REST.
- `supports_live_execution()` / `live_order_submission_implemented()` /
  `order_status_polling_implemented()` — true **only** where order submit,
  cancel, and status polling are implemented end-to-end; fail-closed
  otherwise.
- No adapter claims fills from submit acknowledgements; `fetch_order` polls
  venue truth (REST order status or chain receipt for AMM swaps).
- Live strategy entry dispatches through `live_entry_gate` — unlocked
  vault plus per-venue submission/polling/strategy-entry/credential
  eligibility — and venue sandbox evidence still distinguishes
  canary-proven routes from implemented-but-unproven ones; see
  `docs/CAPABILITIES.md`.
