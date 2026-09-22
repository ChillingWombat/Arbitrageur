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
| 35 | BULK | perp DEX (early) | med | API docs thin; endpoint serves unrelated page | excluded-unverified |
| 36 | Bitfinex | CEX | cex | yes | excluded-cex |
| 37 | EVEDEX | perp DEX | med | `api.evedex.com` DNS-fails | excluded-dead |
| 38 | KCEX | CEX | cex | yes | excluded-cex |
| 39 | Hotcoin | CEX | cex | yes | excluded-cex |
| 40 | Zoomex | CEX | cex | yes | excluded-cex |

### Genuine perp DEXes below the top-40 (same list, continued)

| CG rank | Venue | Custody | API / agent-wallet trading? | Verdict |
| --- | --- | --- | --- | --- |
| 41 | edgeX | med (StarkEx-style L2) | REST market data + HMAC auth; orders need Stark L2 sig; **depth is WS-only (REST returns empty)** | added (`edgex`) — markets live; book/funding + trading fail closed pending WS-depth & Stark signing |
| 43 | GRVT | low (ZKsync hyperchain, trustless orders) | REST + session cookie + EIP-712 order sig | added (`grvt`) — trading implemented |
| 44 | Robinhood Chain Lighter | low | same rails as `lighter` | supported (`robinhood` = Lighter adapter) |
| 45 | Antarctic | med | no verified public API | excluded-unverified |
| 48 | QFEX | med (6-person team) | `/refdata` live; books auth-gated | added (`qfex`) — markets only, caution flag |
| 50 | GMX V2 (Solana) | low | contract calls only, no orderbook | excluded-no-orderbook (AMM pricing) |
| 51 | Extended | low | REST + Stark signature orders | added (`extended`) — market data live; trading fail-closed pending Stark signing |
| 52 | Ostium | **high** — ~$18M oracle exploit, paused | onchain only | excluded-exploit |
| 54 | ApeX Omni | low (zkLink settle) | REST + apiKey HMAC + zkLink L2 sig | added (`apex`) — market data live; trading fail-closed pending zkLink signing |
| 55 | SoDEX | med | parked domain | excluded-dead |
| 56 | Pacifica | med (ex-FTX team) | REST + ed25519-signed ops | added (`pacifica`) — trading implemented |
| 58 | AlphaX | **high** (no PoR) | parked domain | excluded-dead |
| 60 | Derive | low | REST + `X-Lyra*` auth + EIP-712 Action sig | added (`derive`) — trading implemented |
| 61 | Nado | med | no public API found | excluded-no-api |
| 62 | Ondo Perps | med | full REST (SIWE→JWT→orders) | added (`ondoperps`) — trading implemented |
| 63 | IO Trader | med | parked domain | excluded-dead |
| 64 | Orderly (Derivatives) | low | same rails as `vanta` | supported (`vanta`) — dedup: one Orderly venue |
| 65 | Vest Exchange | med | Cloudflare-530s | excluded-dead |
| 69 | StandX | med | REST + wallet-sig JWT + ed25519 body sig | added (`standx`) — trading implemented |
| 70 | RISEx | med | no live mainnet | excluded-unverified |
| 72 | dYdX Chain | low | indexer reads + cosmos-tx writes | added (`dydx`) — market data live; trading fail-closed pending cosmos signer |
| 73 | Niza.fun | med | no futures product found | excluded-dead |
| 74 | Arcus | med | research venue | supported (`arcus`, scaffold) |
| 75 | Flipster | CEX-hybrid | yes | excluded-cex |
| 76 | GMX V2 (Arbitrum) | low | contract only, no orderbook | excluded-no-orderbook |
| 77 | Paradex | low | REST + Stark SNIP-12 sig + JWT | added (`paradex`) — market data live; trading fail-closed pending Stark signing |
| 79 | Astros | low | DeepBook onchain, no REST | excluded-no-api |
| 80 | Aevo | med | REST + AEVO-KEY/SECRET + EIP-712 order sig | added (`aevo`) — trading implemented |
| 82 | Gate DEX | med | DNS/endpoint unverified | excluded-unverified |
| 83 | Decibel | low | REST + Aptos-signed txs | supported (`decibel`) |
| 85 | Helix (Injective) | low | chain LCD reads live; orders need injective-chain sig | added (`helix`) — markets/book live; funding + trading fail-closed |
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
  enablement): `pacifica`, `ondoperps`, `standx`, `derive`, `grvt`, `aevo`
  (full REST trading implemented — credentials still required before
  `has_live_credentials()` opens order paths);
  `edgex`, `extended`, `paradex`, `apex`, `dydx`, `helix`, `gmx`,
  `qfex`, `vest`, `antarctic`, `evedex`, `bulk`, `ostium`, `sodex`,
  `alphax`, `nado`, `iotrader`, `niza`, `astros`, `gatedex`
  (market data live; order paths fail closed — see CAPABILITIES.md for the
  exact per-venue capability bits).
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
| **Aerodrome** | 7,18,60,78 | Base | low | Slipstream quoter (UniV3-compatible) + classic Solidly router | added (`aerodrome`) |
| **Velodrome** | 65 | Optimism | low | same family as Aerodrome | added (`velodrome`) |
| **Orca / Raydium / Meteora / PumpSwap / Humidifi / ZeroFi / Byreal / Manifest** | 5,10,19,8,37,35,45,41,15 | Solana | low | Solana tx signing — needs solana SDK integration | excluded-deferred (non-EVM; single biggest missing cluster — ~$1.4B/day combined) |
| **Curve** | 12 | ETH | low | custom AMM math/router | excluded-custom-abi (v2: StableSwapNG quoter) |
| **Fluid** | 14,72 | ETH, Arb | low | custom liquidity-layer ABI | excluded-custom-abi |
| **Ramses** | 31,64,66 | Robinhood-chain, HyperEVM, Polygon | low | Solidly fork | added (`ramses`) |
| **Pharaoh / Blackhole** | 21,27,79 | Avalanche | low | Solidly forks (same team/lineage) | added (`pharaoh`, `blackhole`) |
| **Dexalot** | 33 | Avalanche subnet CLOB | low | REST CLOB API exists | excluded-unverified (subnet RPC needed) |
| **THORChain / Chainflip / Near Intents / Bron Intents** | 34,84,22,100 | cross-chain | low | swap intents, not orderbooks | excluded-model-mismatch |
| **Kuru** | 23 | Monad CLOB | low | CLOB API | excluded-unverified |
| **Project X / Nest / Topaz / Saphyre / GIGA / Hydrex / Up / De¹ / Pons / Alandale / Bankr / Temple / Native Core / Subnet Tokens** | various | HyperEVM / Monad / misc L2s | low–med | mostly EVM AMMs on new chains | excluded-unverified (thin docs/endpoints) |
| **DODO** | 26 | BSC | low | PMM — custom ABI | excluded-custom-abi |
| **SunSwap** | 40 | Tron | low | Tron tx signing | excluded-deferred (non-EVM) |
| **Cetus / Turbos / Bluefin / Aftermath** | 43,59,56,71 | Sui | low | Sui tx signing | excluded-deferred (non-EVM) |
| **Ekubo** | 76 (Starknet), 89 (ETH) | Starknet/ETH | low | custom pool math | excluded-custom-abi |
| **Camelot** | 86 | Arbitrum | low | UniV2-style router (V2) | added (`camelot`) |
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

- **Target list** (`evm_amm.rs`, one generic adapter over venue specs,
  EVM agent-key execution): `uniswap` (UniV3, default Arbitrum),
  `uniswap_v2` (UniV2, Ethereum), `pancakeswap` (UniV2-style, BSC),
  `pancakeswap_v3` (UniV3-style, BSC), `aerodrome` (Solidly-classic
  routes, Base), `velodrome` (Solidly-classic, Optimism), `ramses`,
  `pharaoh`, `blackhole`, `camelot` (Algebra), `quickswap` (UniV2,
  Polygon), `sushiswap` (UniV2, Ethereum).
- **Deferred (documented)**: non-EVM clusters (Solana/Sui/Tron/Aptos/XRPL/
  Hedera/Stellar/NEAR/Injective) need per-chain signers; custom-ABI AMMs
  (Curve, Fluid, DODO, LFJ, Ekubo) need per-venue quoter work.
- **Excluded**: aggregators/intents (no venue orderbook), RFQ venues, dead
  endpoints, non-venue entries.

## 2b. `evm_amm` implementation spec

One `EvmAmmAdapter` struct driven by a per-venue `AmmSpec` table — same
shape as `DexPerpAdapter`, but all I/O is on-chain JSON-RPC instead of
venue REST.

### ABI families

| Kind | Pool discovery | Quote | Swap entrypoint |
| --- | --- | --- | --- |
| `UniV2` | `factory.getPair(a,b)` | `router.getAmountsOut(amountIn,[a,b])` | `swapExactTokensForTokens` / `swapTokensForExactTokens` |
| `UniV3` | `factory.getPool(a,b,fee)` | `quoter.quoteExactInputSingle(params)` | `SwapRouter02.exactInputSingle` / `exactOutputSingle` |
| `Solidly` | `factory.getPool(a,b,stable)` | `router.getAmountsOut(amountIn,Route[])` | `swapExactTokensForTokens(...,Route[])` |

Ramses-lineage V3 pools are keyed by `tickSpacing` (not UniV3 `fee`), and
Algebra pools (Blackhole CL, Camelot V3, QuickSwap V3) drop the fee param
entirely — both need their own quoter/router ABI, so v1 of `evm_amm`
supports the three uniform families above and reaches Ramses / Pharaoh /
Blackhole through their verified **legacy Solidly** routers and Camelot
through its UniV2-compatible V2 router. RamsesV3/Algebra CL liquidity is
deferred to a follow-up adapter family.

### Interface contract (per venue)

- `base_url: Option<String>` carries the **RPC endpoint** override —
  spec default is a public RPC for the venue's chain; Engine Settings can
  point at an operator's node. Chain id is verified against the spec
  before any signing (wrong-network RPC refuses).
- `fetch_markets` resolves each spec pair's pool on-chain
  (`getPool`/`getPair`/`poolByPair`/`getPool(stable)`) and returns only
  pools that resolve non-zero — markets are *proven*, never assumed.
- `fetch_ticker` quotes one base unit through the venue's quoter
  (`quoteExactInputSingle` / `getAmountsOut`) → `last`; the reverse quote
  gives `bid`. AMM venues have no orderbook → `fetch_orderbook` fails
  closed (prices-only, same honesty rule as GMX in `dex_perps`).
- `fetch_account` reads ERC-20 `balanceOf` for the spec token list plus
  native balance → `balances`; `positions` stays empty (spot).
- `fetch_funding_rate` / `set_leverage` fail closed — spot venues have
  neither.
- `place_order` — **market and limit-bound orders only**: a swap is
  atomic, so `Limit`/`PostOnly` is honored as a price bound — the quoter
  result must satisfy the limit or the order is refused *without
  submitting* (never rests, never violates the bound). Sell = exact-in
  base→quote; Buy = exact-out quote→base (`swapTokensForExactTokens` /
  `exactOutputSingle`). Slippage bound = `minAmountOut`/`maxAmountIn`
  derived from the limit price (or the quote for market orders with the
  router tolerance). ERC-20 `approve(router)` is issued lazily per
  (token, spender) when allowance is insufficient.
- `cancel_order` fails closed — nothing rests on an AMM.
- `fetch_order` maps `eth_getTransactionReceipt(hash)` →
  `Filled` (status 1) / `Rejected` (status 0) / pending-missing → error
  (never invents state). Order identity is the tx hash, durable by
  construction.
- Capabilities: `live_order_submission_implemented` /
  `order_status_polling_implemented` true (atomic swap + receipt poll);
  `has_live_credentials` requires the EVM agent key (`privateKey` field).
  No API key exists — the agent key *is* the credential.

### Venue spec table (contract addresses — canonical deployments)

| id | chain | kind | factory / quoter / router |
| --- | --- | --- | --- |
| `uniswap` | Arbitrum 42161 | UniV3 | F `0x1F98431c8aD98523631AE4a59f267346ea31F984` / Q `0x61fFE014bA17989E743c5F6cB21bF9697530B21e` / R `0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45` |
| `uniswap_v2` | Ethereum 1 | UniV2 | F `0x5C69bEe701ef814a2B6a3EDD4B1652CB9cc5aA6f` / R `0x7a250d5630B4cF539739dF2C5dAcb4c659F2488D` |
| `pancakeswap` | BSC 56 | UniV2 | F `0xcA143Ce32Fe78f1f7019d7d551a6402fC5350c73` / R `0x10ED43C718714eb63d5aA57B78B54704E256024E` |
| `pancakeswap_v3` | BSC 56 | UniV3 | F `0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865` / Q `0xB048Bbc1Ee6b733FFfCFb9e9CeF7375518e25997` / R `0x1b81D678ffb9C0263b24A97847620C99d213eB14` |
| `aerodrome` | Base 8453 | Solidly | F `0x420DD381b31aEf6683db6B902084cB0FFECe40Da` / R `0xcF77a3Ba9A5CA399B7c97c74d54e5b1Beb874E43` |
| `velodrome` | Optimism 10 | Solidly | F `0xF1046053aa5682b4F9a81b5481394DA16BE5FF5a` / R `0xa062aE8A9c5e11aaA026fc2670B0D65cCc8B2858` |
| `camelot` | Arbitrum 42161 | UniV2 (V2 pools) | F `0x6EcCab422D763aC031210895C81787E87B43A652` / R `0xc873fEcbd354f5A56E00E710B90EF4201db2448d` |
| `quickswap` | Polygon 137 | UniV2 | F `0x5757371414417b8C6CAad45bAeF941aBc7d3Ab32` / R `0xa5E0829CaCEd8fFDD4De3c43696c57F7D7A678ff` |
| `sushiswap` | Ethereum 1 | UniV2 | F `0xC0AEe478e3658e2610c5F7A4A2E1777cE9e4f2Ac` / R `0xd9e1cE17f2641f24aE83637ab66a2cca9C378B9F` |
| `ramses` | Arbitrum 42161 | Solidly (legacy) | F `0xADd32480630A16dfAcEe6eeFcB3ab2181449Dc3B` / R `0x1614a7e1fe63960B4684867a62080acd2404757f` |
| `pharaoh` | Avalanche 43114 | Solidly (PairFactory+Router) | F `0x85448bF2F589ab1F56225DF5167c63f57758f8c1` / R `0x9CEE04bDcE127DA7E448A333f006DEFb3d5e38cC` |
| `blackhole` | Avalanche 43114 | Solidly (PairFactory+RouterV2) | F `0xfE926062Fb99CA5653080d6C14fE945Ad68c265C` / R `0x9EED160D7D8253DeC1A2A512e504DE5E7ff3C111` |

PancakeSwap V3 keeps `deadline` inside its params struct (old SwapRouter
interface); Uniswap SwapRouter02 drops it — the spec carries a
`deadline_in_params` flag and the adapter encodes the right shape.

(Default public RPCs: `arb1.arbitrum.io/rpc`, `eth.llamarpc.com`,
`bsc-dataseed.binance.org`, `mainnet.base.org`, `mainnet.optimism.io`,
`polygon-rpc.com`, `api.avax.network/ext/bc/C/rpc` — all injectable.)

Market sets are the spec pair tables verified on-chain at
`fetch_markets` — majors only per chain (WETH/WBTC/ARB/OP/BNB/AVAX/POL
vs USDC/USDT where the pool exists). Adding a pair is a spec-row change,
never new code.

## 3. What "added" means (capability contract)

Every added adapter reports honest capability flags:

- Market data (markets/book/ticker/funding) — live-verified REST.
- `supports_live_execution()` / `live_order_submission_implemented()` /
  `order_status_polling_implemented()` — true **only** where order submit,
  cancel, and status polling are implemented end-to-end; fail-closed
  otherwise.
- No adapter claims fills from submit acknowledgements; `fetch_order` polls
  venue truth (REST order status or chain receipt for AMM swaps).
- Live strategy entry still requires the operator's `live_entry_enabled`
  canary gate and venue sandbox evidence — see `docs/CAPABILITIES.md`.
