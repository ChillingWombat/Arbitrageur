//! Generic EVM spot-DEX (AMM) adapter — one spec-driven implementation for
//! the three dominant onchain-AMM ABI families:
//!
//! * `UniV2` — constant-product forks. `getPair(a,b)` factory lookup,
//!   `getAmountsOut`/`getAmountsIn` router quotes,
//!   `swapExactTokensForTokens`/`swapTokensForExactTokens`.
//!   Covers Uniswap V2, PancakeSwap V2, SushiSwap, QuickSwap, Camelot V2.
//! * `UniV3` — concentrated liquidity. `getPool(a,b,fee)` factory lookup,
//!   `QuoterV2.quoteExact{Input,Output}Single`, SwapRouter
//!   `exact{Input,Output}Single` (deadline-in-params variant, which
//!   Uniswap's canonical `0xE592…` SwapRouter and PancakeSwap's V3
//!   router share).
//! * `Solidly` — volatile/stable pool routers. Pool lookup via
//!   `getPair(a,b,stable)` (Ramses/Pharaoh legacy) or
//!   `getPool(a,b,stable)` (Aerodrome/Velodrome V2), routes encoded
//!   `{from,to,stable}` or `{from,to,stable,factory}`.
//!
//! ## Venue truth
//!
//! There are no resting orders on an AMM: `place_order` submits a swap
//! transaction and returns `OrderStatus::New` keyed by the transaction
//! hash — the durable order id — without waiting for the receipt, so the
//! id reaches durable state before any receipt timeout or process crash.
//! `fetch_order` re-fetches and re-decodes the same receipt (`status == 1`,
//! ERC-20 `Transfer` logs of both legs to/from the signer, gas fee), so
//! reconciliation never trusts a submit acknowledgement.
//!
//! Only the six standard UniV2/UniV3-router venues execute
//! (`LIVE_EVM_AMM_VENUE_IDS`). Camelot's router uses a custom referrer
//! ABI, not the standard UniV2 swap ABI, and the Solidly-family routers
//! (Aerodrome/Velodrome/Ramses/Pharaoh) have no exact-output buy primitive
//! — an exact-in estimate can overfill the requested base quantity — so
//! all five stay admitted for market data but refuse order submission.
//!
//! ## Scope / known deferrals
//!
//! * Single-hop swaps only — no multi-hop routing (configured pairs are
//!   direct pools, so a hop path is never fabricated).
//! * ERC-20 <-> ERC-20 only — no native-wrap/unwrap leg. The wrapped native
//!   token (WETH/WBNB/WMATIC/WAVAX) is the configured base.
//! * Ramses V3 / Pharaoh CL / Blackhole Algebra concentrated liquidity —
//!   different ABIs (tickSpacing-keyed pools, Algebra Swap events) — are
//!   deferred; those venues run on their legacy Solidly routers where they
//!   exist. Blackhole's legacy deployment has no live major pools, so the
//!   venue stays spec'd-but-unadmitted (`admit = false`).
//! * `fetch_orderbook` synthesizes a single level from the live quote —
//!   documented approximation of "executable at this price for at least
//!   this size"; an AMM has no L2 depth to report.

use std::str::FromStr;
use std::time::Duration;

use alloy::network::TransactionBuilder;
use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::SolCall;
use async_trait::async_trait;
use chrono::Utc;
use parking_lot::RwLock;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use zeroize::Zeroizing;

use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, MarketType, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::{AccountSnapshot, Balance};
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{OrderStatus, OrderType, Side, VenueId};

// ---------------------------------------------------------------------------
// ABI — minimal surface per family, verified against live eth_call probes.
// ---------------------------------------------------------------------------

alloy::sol! {
    #[derive(Debug)]
    interface IERC20 {
        function balanceOf(address account) external view returns (uint256);
        function allowance(address owner, address spender) external view returns (uint256);
        function approve(address spender, uint256 amount) external returns (bool);
        function decimals() external view returns (uint8);
        function symbol() external view returns (string);
    }

    interface IUniV2Factory {
        function getPair(address tokenA, address tokenB) external view returns (address pair);
    }

    interface IUniV2Router {
        function getAmountsOut(uint256 amountIn, address[] memory path) external view returns (uint256[] memory amounts);
        function getAmountsIn(uint256 amountOut, address[] memory path) external view returns (uint256[] memory amounts);
        function swapExactTokensForTokens(uint256 amountIn, uint256 amountOutMin, address[] calldata path, address to, uint256 deadline) external returns (uint256[] memory amounts);
        function swapTokensForExactTokens(uint256 amountOut, uint256 amountInMax, address[] calldata path, address to, uint256 deadline) external returns (uint256[] memory amounts);
    }

    interface IUniV3Factory {
        function getPool(address tokenA, address tokenB, uint24 fee) external view returns (address pool);
    }

    /// QuoterV2 (Uniswap 0x61fFE014…, PancakeSwap 0xB048Bbc1…) — reverts on
    /// missing pool; a successful static call IS venue truth for quoting.
    interface IQuoterV2 {
        struct QuoteExactInputSingleParams {
            address tokenIn;
            address tokenOut;
            uint256 amountIn;
            uint24 fee;
            uint160 sqrtPriceLimitX96;
        }
        function quoteExactInputSingle(QuoteExactInputSingleParams memory params)
            external returns (uint256 amountOut, uint160 sqrtPriceX96After, uint32 initializedTicksCrossed, uint256 gasEstimate);
        struct QuoteExactOutputSingleParams {
            address tokenIn;
            address tokenOut;
            uint256 amount;
            uint24 fee;
            uint160 sqrtPriceLimitX96;
        }
        function quoteExactOutputSingle(QuoteExactOutputSingleParams memory params)
            external returns (uint256 amountIn, uint160 sqrtPriceX96After, uint32 initializedTicksCrossed, uint256 gasEstimate);
    }

    /// Original SwapRouter ABI (deadline inside params) — Uniswap 0xE592…,
    /// PancakeSwap V3 0x1b81D678… both use this shape.
    interface ISwapRouterV3 {
        struct ExactInputSingleParams {
            address tokenIn;
            address tokenOut;
            uint24 fee;
            address recipient;
            uint256 deadline;
            uint256 amountIn;
            uint256 amountOutMinimum;
            uint160 sqrtPriceLimitX96;
        }
        function exactInputSingle(ExactInputSingleParams calldata params) external payable returns (uint256 amountOut);
        struct ExactOutputSingleParams {
            address tokenIn;
            address tokenOut;
            uint24 fee;
            address recipient;
            uint256 deadline;
            uint256 amountOut;
            uint256 amountInMaximum;
            uint160 sqrtPriceLimitX96;
        }
        function exactOutputSingle(ExactOutputSingleParams calldata params) external payable returns (uint256 amountIn);
    }

    /// Ramses/Pharaoh legacy factory (Solidly getPair).
    interface ISolidlyFactoryGetPair {
        function getPair(address tokenA, address tokenB, bool stable) external view returns (address pair);
    }

    /// Aerodrome/Velodrome V2 PoolFactory (renamed lookup).
    interface ISolidlyFactoryGetPool {
        function getPool(address tokenA, address tokenB, bool stable) external view returns (address pool);
    }

    /// 3-field route router (Ramses legacy, Pharaoh RouterV2, Blackhole V2).
    interface ISolidlyRouter3 {
        struct Route { address from; address to; bool stable; }
        function getAmountsOut(uint256 amountIn, Route[] memory routes) external view returns (uint256[] memory amounts);
        function swapExactTokensForTokens(uint256 amountIn, uint256 amountOutMin, Route[] calldata routes, address to, uint256 deadline) external returns (uint256[] memory amounts);
    }

    /// 4-field route router (Velodrome V2 / Aerodrome — factory is a route field).
    interface ISolidlyRouter4 {
        struct Route { address from; address to; bool stable; address factory; }
        function getAmountsOut(uint256 amountIn, Route[] memory routes) external view returns (uint256[] memory amounts);
        function swapExactTokensForTokens(uint256 amountIn, uint256 amountOutMin, Route[] calldata routes, address to, uint256 deadline) external returns (uint256[] memory amounts);
    }
}

// ---------------------------------------------------------------------------
// Venue specifications
// ---------------------------------------------------------------------------

/// AMM ABI family — determines pool lookup, quote, and swap encodings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AmmKind {
    UniV2,
    UniV3,
    /// `solidly_getpool` chooses the factory lookup fn; `route_has_factory`
    /// chooses the 3- vs 4-field Route encoding.
    Solidly {
        getpool_lookup: bool,
        route_has_factory: bool,
    },
}

/// ERC-20 token on a venue's chain — static, verified addresses only.
#[derive(Debug, Clone, Copy)]
struct TokenSpec {
    /// Canonical symbol used in `BASE-QUOTE` pair names (no separators).
    symbol: &'static str,
    address: &'static str,
    decimals: u8,
}

/// One configured direct pair. `fee` is the UniV3 fee tier in hundredths of a
/// bips (500 = 0.05%) and ignored elsewhere; `stable` selects the Solidly
/// pool flavour and is ignored elsewhere.
#[derive(Debug, Clone, Copy)]
struct PairSpec {
    base: &'static str,
    quote: &'static str,
    fee: u32,
    stable: bool,
}

struct AmmVenueSpec {
    id: &'static str,
    name: &'static str,
    kind: AmmKind,
    chain_id: u64,
    /// Human label for status surfaces.
    chain_label: &'static str,
    /// Symbol for the chain's native currency in account snapshots.
    native_symbol: &'static str,
    /// Wrapped-native token symbol (must exist in `tokens`) — used to price
    /// the native balance for equity.
    native_wrapped: &'static str,
    factory: &'static str,
    router: &'static str,
    /// UniV3 only.
    quoter: Option<&'static str>,
    /// Taker fee in bps for UniV2/Solidly pools (UniV3 derives per-pair).
    taker_fee_bps: u32,
    /// Solidly stable-pool fee in bps.
    stable_fee_bps: u32,
    default_rpc: &'static str,
    tokens: &'static [TokenSpec],
    pairs: &'static [PairSpec],
    /// Whether `get_venues` may admit this spec. Blackhole is spec'd but not
    /// admitted: its legacy deployment holds no major pools (verified), and
    /// its Algebra CL needs a dedicated ABI.
    admit: bool,
}

// Ethereum
const ETH_TOKENS: &[TokenSpec] = &[
    TokenSpec {
        symbol: "WETH",
        address: "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2",
        decimals: 18,
    },
    TokenSpec {
        symbol: "USDC",
        address: "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
        decimals: 6,
    },
    TokenSpec {
        symbol: "USDT",
        address: "0xdAC17F958D2ee523A2206206994597C13D831ec7",
        decimals: 6,
    },
    TokenSpec {
        symbol: "DAI",
        address: "0x6B175474E89094C44Da98b954EedeAC495271d0f",
        decimals: 18,
    },
    TokenSpec {
        symbol: "WBTC",
        address: "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599",
        decimals: 8,
    },
];
// BNB Chain
const BSC_TOKENS: &[TokenSpec] = &[
    TokenSpec {
        symbol: "WBNB",
        address: "0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c",
        decimals: 18,
    },
    TokenSpec {
        symbol: "USDT",
        address: "0x55d398326f99059fF775485246999027B3197955",
        decimals: 18,
    },
    TokenSpec {
        symbol: "USDC",
        address: "0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d",
        decimals: 18,
    },
    TokenSpec {
        symbol: "ETH",
        address: "0x2170Ed0880ac9A755fd29B2688956BD959F933F8",
        decimals: 18,
    },
    TokenSpec {
        symbol: "BTCB",
        address: "0x7130d2A12B9BCbFAe4f2634d864A1Ee1Ce3Ead9c",
        decimals: 18,
    },
    TokenSpec {
        symbol: "CAKE",
        address: "0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82",
        decimals: 18,
    },
];
// Polygon (USDC.e is the bridged canonical — native USDC pools are thin)
const POLYGON_TOKENS: &[TokenSpec] = &[
    TokenSpec {
        symbol: "WMATIC",
        address: "0x0d500B1d8E8eF31E21C99d1Db9A6444d3ADf1270",
        decimals: 18,
    },
    TokenSpec {
        symbol: "USDCE",
        address: "0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174",
        decimals: 6,
    },
    TokenSpec {
        symbol: "USDT",
        address: "0xc2132D05D31c914a87C6611C10748AEb04B58e8F",
        decimals: 6,
    },
    TokenSpec {
        symbol: "WETH",
        address: "0x7ceB23fD6bC0adD59E62ac25578270cFf1b9f619",
        decimals: 18,
    },
];
// Arbitrum
const ARB_TOKENS: &[TokenSpec] = &[
    TokenSpec {
        symbol: "WETH",
        address: "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1",
        decimals: 18,
    },
    TokenSpec {
        symbol: "USDC",
        address: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831",
        decimals: 6,
    },
    TokenSpec {
        symbol: "USDT",
        address: "0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9",
        decimals: 6,
    },
    TokenSpec {
        symbol: "ARB",
        address: "0x912CE59144191C1204E64559FE8253a0e49E6548",
        decimals: 18,
    },
    TokenSpec {
        symbol: "WBTC",
        address: "0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f",
        decimals: 8,
    },
];
// Base
const BASE_TOKENS: &[TokenSpec] = &[
    TokenSpec {
        symbol: "WETH",
        address: "0x4200000000000000000000000000000000000006",
        decimals: 18,
    },
    TokenSpec {
        symbol: "USDC",
        address: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
        decimals: 6,
    },
    TokenSpec {
        symbol: "DAI",
        address: "0x50c5725949A6F0c72E6C4a641F24049A917DB0Cb",
        decimals: 18,
    },
    TokenSpec {
        symbol: "AERO",
        address: "0x940181a94A35A4569E4529A3CDfB74e38FD98631",
        decimals: 18,
    },
];
// Optimism
const OP_TOKENS: &[TokenSpec] = &[
    TokenSpec {
        symbol: "WETH",
        address: "0x4200000000000000000000000000000000000006",
        decimals: 18,
    },
    TokenSpec {
        symbol: "USDC",
        address: "0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85",
        decimals: 6,
    },
    TokenSpec {
        symbol: "OP",
        address: "0x4200000000000000000000000000000000000042",
        decimals: 18,
    },
    TokenSpec {
        symbol: "VELO",
        address: "0x9560e827aF36c94D2Ac33a39bCE1Fe78631088Db",
        decimals: 18,
    },
    TokenSpec {
        symbol: "DAI",
        address: "0xDA10009cBd5D07dd0CeCc66161FC93D7c9000da1",
        decimals: 18,
    },
];
// Avalanche
const AVAX_TOKENS: &[TokenSpec] = &[
    TokenSpec {
        symbol: "WAVAX",
        address: "0xB31f66AA3C1e785363F0875A1B74E27b85FD66c7",
        decimals: 18,
    },
    TokenSpec {
        symbol: "USDC",
        address: "0xB97EF9Ef8734C71904D8002F8b6Bc66Dd9c48a6E",
        decimals: 6,
    },
    TokenSpec {
        symbol: "USDT",
        address: "0x9702230A8Ea53601f5cD2dc00fDBc13d4dF4A8c7",
        decimals: 6,
    },
    TokenSpec {
        symbol: "WETHE",
        address: "0x49D5c2BdFfac6CE2BFdB6640F4F80f226bc10bAB",
        decimals: 18,
    },
];

const fn u2(base: &'static str, quote: &'static str) -> PairSpec {
    PairSpec {
        base,
        quote,
        fee: 0,
        stable: false,
    }
}
const fn u3(base: &'static str, quote: &'static str, fee: u32) -> PairSpec {
    PairSpec {
        base,
        quote,
        fee,
        stable: false,
    }
}
const fn sd(base: &'static str, quote: &'static str, stable: bool) -> PairSpec {
    PairSpec {
        base,
        quote,
        fee: 0,
        stable,
    }
}

static SPECS: &[AmmVenueSpec] = &[
    AmmVenueSpec {
        id: "uniswap",
        name: "Uniswap V3",
        kind: AmmKind::UniV3,
        chain_id: 1,
        chain_label: "Ethereum",
        native_symbol: "ETH",
        native_wrapped: "WETH",
        factory: "0x1F98431c8aD98523631AE4a59f267346ea31F984",
        router: "0xE592427A0AEce92De3Edee1F18E0157C05861564",
        quoter: Some("0x61fFE014bA17989E743c5F6cB21bF9697530B21e"),
        taker_fee_bps: 0,
        stable_fee_bps: 0,
        default_rpc: "https://ethereum-rpc.publicnode.com",
        tokens: ETH_TOKENS,
        pairs: &[
            u3("WETH", "USDC", 500),
            u3("WETH", "USDT", 500),
            u3("WBTC", "WETH", 3000),
            u3("USDC", "USDT", 100),
            u3("DAI", "USDC", 100),
        ],
        admit: true,
    },
    AmmVenueSpec {
        id: "uniswap_v2",
        name: "Uniswap V2",
        kind: AmmKind::UniV2,
        chain_id: 1,
        chain_label: "Ethereum",
        native_symbol: "ETH",
        native_wrapped: "WETH",
        factory: "0x5C69bEe701ef814a2B6a3EDD4B1652CB9cc5aA6f",
        router: "0x7a250d5630B4cF539739dF2C5dAcb4c659F2488D",
        quoter: None,
        taker_fee_bps: 30,
        stable_fee_bps: 0,
        default_rpc: "https://ethereum-rpc.publicnode.com",
        tokens: ETH_TOKENS,
        pairs: &[
            u2("WETH", "USDC"),
            u2("WETH", "USDT"),
            u2("WETH", "DAI"),
            u2("WBTC", "WETH"),
        ],
        admit: true,
    },
    AmmVenueSpec {
        id: "pancakeswap",
        name: "PancakeSwap V2",
        kind: AmmKind::UniV2,
        chain_id: 56,
        chain_label: "BNB Chain",
        native_symbol: "BNB",
        native_wrapped: "WBNB",
        factory: "0xcA143Ce32Fe78f1f7019d7d551a6402fC5350c73",
        router: "0x10ED43C718714eb63d5aA57B78B54704E256024E",
        quoter: None,
        taker_fee_bps: 25,
        stable_fee_bps: 0,
        default_rpc: "https://bsc-rpc.publicnode.com",
        tokens: BSC_TOKENS,
        pairs: &[
            u2("WBNB", "USDT"),
            u2("WBNB", "USDC"),
            u2("BTCB", "WBNB"),
            u2("ETH", "WBNB"),
            u2("CAKE", "WBNB"),
        ],
        admit: true,
    },
    AmmVenueSpec {
        id: "pancakeswap_v3",
        name: "PancakeSwap V3",
        kind: AmmKind::UniV3,
        chain_id: 56,
        chain_label: "BNB Chain",
        native_symbol: "BNB",
        native_wrapped: "WBNB",
        factory: "0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865",
        router: "0x1b81D678ffb9C0263b24A97847620C99d213eB14",
        quoter: Some("0xB048Bbc1Ee6b733FFfCFb9e9CeF7375518e25997"),
        taker_fee_bps: 0,
        stable_fee_bps: 0,
        default_rpc: "https://bsc-rpc.publicnode.com",
        tokens: BSC_TOKENS,
        pairs: &[
            u3("WBNB", "USDT", 2500),
            u3("WBNB", "USDC", 2500),
            u3("USDC", "USDT", 100),
            u3("ETH", "WBNB", 500),
            u3("BTCB", "WBNB", 500),
        ],
        admit: true,
    },
    AmmVenueSpec {
        id: "sushiswap",
        name: "SushiSwap",
        kind: AmmKind::UniV2,
        chain_id: 1,
        chain_label: "Ethereum",
        native_symbol: "ETH",
        native_wrapped: "WETH",
        factory: "0xC0AEe478e3658e2610c5F7A4A2E1777cE9e4f2Ac",
        router: "0xd9e1cE17f2641f24aE83637ab66a2cca9C378B9F",
        quoter: None,
        taker_fee_bps: 30,
        stable_fee_bps: 0,
        default_rpc: "https://ethereum-rpc.publicnode.com",
        tokens: ETH_TOKENS,
        pairs: &[u2("WETH", "USDC"), u2("WETH", "USDT"), u2("WETH", "DAI")],
        admit: true,
    },
    AmmVenueSpec {
        id: "quickswap",
        name: "QuickSwap",
        kind: AmmKind::UniV2,
        chain_id: 137,
        chain_label: "Polygon",
        native_symbol: "POL",
        native_wrapped: "WMATIC",
        factory: "0x5757371414417b8C6CAad45bAeF941aBc7d3Ab32",
        router: "0xa5E0829CaCEd8fFDD4De3c43696c57F7D7A678ff",
        quoter: None,
        taker_fee_bps: 30,
        stable_fee_bps: 0,
        default_rpc: "https://polygon-bor-rpc.publicnode.com",
        tokens: POLYGON_TOKENS,
        pairs: &[
            u2("WMATIC", "USDCE"),
            u2("WMATIC", "USDT"),
            u2("WETH", "USDCE"),
        ],
        admit: true,
    },
    AmmVenueSpec {
        id: "camelot",
        name: "Camelot V2",
        kind: AmmKind::UniV2,
        chain_id: 42161,
        chain_label: "Arbitrum",
        native_symbol: "ETH",
        native_wrapped: "WETH",
        factory: "0x6EcCab422D763aC031210895C81787E87B43A652",
        router: "0xc873fEcbd354f5A56E00E710B90EF4201db2448d",
        quoter: None,
        taker_fee_bps: 30,
        stable_fee_bps: 0,
        default_rpc: "https://arbitrum-one-rpc.publicnode.com",
        tokens: ARB_TOKENS,
        pairs: &[u2("WETH", "USDC"), u2("ARB", "WETH"), u2("WETH", "USDT")],
        admit: true,
    },
    AmmVenueSpec {
        id: "aerodrome",
        name: "Aerodrome",
        kind: AmmKind::Solidly {
            getpool_lookup: true,
            route_has_factory: true,
        },
        chain_id: 8453,
        chain_label: "Base",
        native_symbol: "ETH",
        native_wrapped: "WETH",
        factory: "0x420DD381b31aEf6683db6B902084cB0FFECe40Da",
        router: "0xcF77a3Ba9A5CA399B7c97c74d54e5b1Beb874E43",
        quoter: None,
        taker_fee_bps: 30,
        stable_fee_bps: 5,
        default_rpc: "https://base-rpc.publicnode.com",
        tokens: BASE_TOKENS,
        pairs: &[
            sd("WETH", "USDC", false),
            sd("USDC", "DAI", true),
            sd("AERO", "WETH", false),
        ],
        admit: true,
    },
    AmmVenueSpec {
        id: "velodrome",
        name: "Velodrome",
        kind: AmmKind::Solidly {
            getpool_lookup: true,
            route_has_factory: true,
        },
        chain_id: 10,
        chain_label: "Optimism",
        native_symbol: "ETH",
        native_wrapped: "WETH",
        factory: "0xF1046053aa5682b4F9a81b5481394DA16BE5FF5a",
        router: "0xa062aE8A9c5e11aaA026fc2670B0D65cCc8B2858",
        quoter: None,
        taker_fee_bps: 30,
        stable_fee_bps: 5,
        default_rpc: "https://optimism-rpc.publicnode.com",
        tokens: OP_TOKENS,
        pairs: &[
            sd("WETH", "USDC", false),
            sd("OP", "WETH", false),
            sd("USDC", "DAI", true),
            sd("VELO", "WETH", false),
        ],
        admit: true,
    },
    AmmVenueSpec {
        id: "ramses",
        name: "Ramses (legacy)",
        kind: AmmKind::Solidly {
            getpool_lookup: false,
            route_has_factory: false,
        },
        chain_id: 42161,
        chain_label: "Arbitrum",
        native_symbol: "ETH",
        native_wrapped: "WETH",
        factory: "0xADd32480630A16dfAcEe6eeFcB3ab2181449Dc3B",
        router: "0x1614a7e1fe63960B4684867a62080acd2404757f",
        quoter: None,
        taker_fee_bps: 30,
        stable_fee_bps: 5,
        default_rpc: "https://arbitrum-one-rpc.publicnode.com",
        tokens: ARB_TOKENS,
        pairs: &[
            sd("WETH", "USDC", false),
            sd("ARB", "WETH", false),
            sd("WETH", "USDT", false),
        ],
        admit: true,
    },
    AmmVenueSpec {
        id: "pharaoh",
        name: "Pharaoh (legacy)",
        kind: AmmKind::Solidly {
            getpool_lookup: false,
            route_has_factory: false,
        },
        chain_id: 43114,
        chain_label: "Avalanche",
        native_symbol: "AVAX",
        native_wrapped: "WAVAX",
        factory: "0x85448bF2F589ab1F56225DF5167c63f57758f8c1",
        router: "0x9CEE04bDcE127DA7E448A333f006DEFb3d5e38cC",
        quoter: None,
        taker_fee_bps: 30,
        stable_fee_bps: 5,
        default_rpc: "https://avalanche-c-chain-rpc.publicnode.com",
        tokens: AVAX_TOKENS,
        pairs: &[
            sd("WAVAX", "USDC", false),
            sd("WAVAX", "USDT", false),
            sd("WETHE", "WAVAX", false),
        ],
        admit: true,
    },
    AmmVenueSpec {
        id: "blackhole",
        name: "Blackhole (legacy)",
        kind: AmmKind::Solidly {
            getpool_lookup: false,
            route_has_factory: false,
        },
        chain_id: 43114,
        chain_label: "Avalanche",
        native_symbol: "AVAX",
        native_wrapped: "WAVAX",
        factory: "0xfE926062Fb99CA5653080d6C14fE945Ad68c265C",
        router: "0x9EED160D7D8253DeC1A2A512e504DE5E7ff3C111",
        quoter: None,
        taker_fee_bps: 30,
        stable_fee_bps: 5,
        default_rpc: "https://avalanche-c-chain-rpc.publicnode.com",
        tokens: AVAX_TOKENS,
        pairs: &[sd("WAVAX", "USDC", false), sd("WAVAX", "USDT", false)],
        // Legacy deployment holds no major pools (verified 2026-09): the live
        // liquidity sits in the Algebra CL deployment, which needs a dedicated
        // ABI family. Kept in the spec table for documentation; not admitted.
        admit: false,
    },
];

/// Every venue id with a spec — admitted or not (for alias/canonical maps).
pub const EVM_AMM_VENUE_IDS: &[&str] = &[
    "uniswap",
    "uniswap_v2",
    "pancakeswap",
    "pancakeswap_v3",
    "sushiswap",
    "quickswap",
    "camelot",
    "aerodrome",
    "velodrome",
    "ramses",
    "pharaoh",
    "blackhole",
];

/// Ids that may be instantiated and admitted into `get_venues()`.
pub const ADMITTED_EVM_AMM_VENUE_IDS: &[&str] = &[
    "uniswap",
    "uniswap_v2",
    "pancakeswap",
    "pancakeswap_v3",
    "sushiswap",
    "quickswap",
    "camelot",
    "aerodrome",
    "velodrome",
    "ramses",
    "pharaoh",
];

/// Ids whose router ABIs are validated for live swap submission. Camelot's
/// V2 router uses a custom referrer ABI (not the standard UniV2 swap ABI),
/// and the Solidly-family routers (Aerodrome/Velodrome/Ramses/Pharaoh)
/// lack an exact-output buy — an exact-in estimate with headroom can
/// overfill the requested base quantity. Both stay admitted for market
/// data only.
pub const LIVE_EVM_AMM_VENUE_IDS: &[&str] = &[
    "uniswap",
    "uniswap_v2",
    "pancakeswap",
    "pancakeswap_v3",
    "sushiswap",
    "quickswap",
];

fn spec_for(id: &str) -> Option<&'static AmmVenueSpec> {
    SPECS.iter().find(|s| s.id == id)
}

// ---------------------------------------------------------------------------
// Units & helpers
// ---------------------------------------------------------------------------

fn parse_addr(s: &str) -> anyhow::Result<Address> {
    Address::from_str(s.trim()).map_err(|e| anyhow::anyhow!("bad EVM address {s}: {e}"))
}

/// Decimal → integer base units at `decimals` precision. Rejects negative,
/// fractional-base-unit, and overflowing inputs (fail-closed conversions).
fn dec_to_u256(v: Decimal, decimals: u8) -> anyhow::Result<U256> {
    if v <= Decimal::ZERO {
        anyhow::bail!("amount must be positive, got {v}");
    }
    let scaled = v
        .checked_mul(Decimal::from(10u128.pow(decimals as u32)))
        .ok_or_else(|| anyhow::anyhow!("amount {v} overflows at {decimals} decimals"))?;
    if scaled.fract() != Decimal::ZERO {
        anyhow::bail!("amount {v} has sub-wei precision at {decimals} decimals");
    }
    let n: u128 = scaled
        .to_u128()
        .ok_or_else(|| anyhow::anyhow!("amount {v} exceeds u128"))?;
    Ok(U256::from(n))
}

/// Integer base units → Decimal at `decimals` precision.
fn u256_to_dec(v: U256, decimals: u8) -> Decimal {
    let raw = Decimal::from_str(&v.to_string()).unwrap_or(Decimal::ZERO);
    raw / Decimal::from(10u128.pow(decimals as u32))
}

/// Stable-looking symbols priced at face value for equity purposes.
fn is_stable_symbol(sym: &str) -> bool {
    matches!(
        sym.to_ascii_uppercase().as_str(),
        "USDC" | "USDT" | "USDCE" | "USDC.E" | "DAI" | "USDTE" | "USDT.E" | "FRAX" | "USDS"
    )
}

/// Market-order slippage headroom in bps applied when no limit price exists.
const DEFAULT_SLIPPAGE_BPS: u32 = 50;
/// Swap deadline horizon in seconds — a mined-after-deadline tx reverts.
const SWAP_DEADLINE_SECS: i64 = 180;
/// ERC-20 Transfer topic0.
const TRANSFER_TOPIC: &str = "ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

fn deadline_ts() -> U256 {
    U256::from((Utc::now().timestamp() + SWAP_DEADLINE_SECS).max(0) as u64)
}

// ---------------------------------------------------------------------------
// Adapter
// ---------------------------------------------------------------------------

pub struct EvmAmmAdapter {
    venue_id: VenueId,
    name: &'static str,
    spec: &'static AmmVenueSpec,
    /// Constructor-set RPC endpoint (validated injected URL or spec
    /// default) — the restore target when an account override is cleared.
    /// Never mutated after construction. Zeroized: provider URLs can embed
    /// API keys.
    default_rpc_url: Zeroizing<String>,
    /// RPC endpoint — `base_url` injection point (per-account override or
    /// constructor default). Validated to http(s) on set.
    rpc_url: RwLock<Zeroizing<String>>,
    /// RPC URL whose chain id was last verified to match `spec.chain_id`.
    /// Cleared whenever the active RPC changes so a swapped endpoint can
    /// never ride on a stale verification.
    verified_rpc_url: RwLock<Option<Zeroizing<String>>>,
    /// Loaded signing key (agent/local wallet). Never leaves process memory.
    signer: RwLock<Option<PrivateKeySigner>>,
    /// Configured account address — when present it must equal the signer's
    /// address or credentials are refused (fail closed on key/wallet mismatch).
    account: RwLock<Option<Address>>,
    /// Serializes nonce assignment across concurrent swaps.
    swap_lock: tokio::sync::Mutex<()>,
}

impl EvmAmmAdapter {
    pub fn for_venue(id: &str, base_url: Option<String>) -> Option<Self> {
        let spec = spec_for(id.trim().to_lowercase().as_str())?;
        let rpc = base_url
            .map(|u| u.trim().trim_end_matches('/').to_string())
            .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
            .unwrap_or_else(|| spec.default_rpc.to_string());
        Some(Self {
            venue_id: VenueId::new(spec.id),
            name: spec.name,
            spec,
            default_rpc_url: Zeroizing::new(rpc.clone()),
            rpc_url: RwLock::new(Zeroizing::new(rpc)),
            verified_rpc_url: RwLock::new(None),
            signer: RwLock::new(None),
            account: RwLock::new(None),
            swap_lock: tokio::sync::Mutex::new(()),
        })
    }

    pub fn is_admitted(&self) -> bool {
        self.spec.admit
    }
    /// Whether this venue's router ABI is validated for live swap
    /// submission — a strict subset of admitted (market-data) venues.
    fn execution_supported(&self) -> bool {
        self.spec.admit && LIVE_EVM_AMM_VENUE_IDS.contains(&self.spec.id)
    }
    pub fn spec_id(&self) -> &'static str {
        self.spec.id
    }
    /// EVM chain id this venue's contracts live on — used to share a
    /// chain-level RPC override across same-chain venues.
    pub fn chain_id(&self) -> u64 {
        self.spec.chain_id
    }
    pub fn spec_name(&self) -> &'static str {
        self.spec.name
    }
    pub fn market_type_label(&self) -> &'static str {
        "Spot AMM"
    }
    pub fn taker_fee_bps(&self) -> f64 {
        match self.spec.kind {
            AmmKind::UniV3 => self
                .spec
                .pairs
                .first()
                .map(|p| p.fee as f64 / 100.0)
                .unwrap_or(30.0),
            _ => self.spec.taker_fee_bps as f64,
        }
    }

    /// Update the RPC endpoint (per-account override). Blank/`None`
    /// restores the constructor's endpoint; non-http(s) values return
    /// `Err` and keep the current endpoint — the submitted URL is never
    /// echoed (it may embed a provider API key). Any actual change clears
    /// the chain-verification cache.
    pub fn set_base_url(&self, base_url: Option<String>) -> anyhow::Result<()> {
        let next = base_url
            .map(|u| u.trim().trim_end_matches('/').to_string())
            .filter(|u| !u.is_empty());
        let resolved: Zeroizing<String> = match next {
            None => self.default_rpc_url.clone(),
            Some(u) if u.starts_with("http://") || u.starts_with("https://") => Zeroizing::new(u),
            Some(_) => {
                anyhow::bail!("{} endpoint override rejected: must use http(s)", self.name);
            }
        };
        let mut guard = self.rpc_url.write();
        if guard.as_str() != resolved.as_str() {
            *guard = resolved;
            *self.verified_rpc_url.write() = None;
        }
        Ok(())
    }

    /// Same 6-field signature as `DexPerpAdapter::update_credentials` so the
    /// generic credential-sync loop treats both uniformly. Only
    /// `wallet_address` + `private_key` are meaningful for an EVM AMM.
    pub fn update_credentials(
        &self,
        _api_key: Option<String>,
        _api_secret: Option<String>,
        _passphrase: Option<String>,
        _account_id: Option<String>,
        wallet_address: Option<String>,
        private_key: Option<String>,
    ) -> anyhow::Result<()> {
        let norm = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let wallet_address = norm(wallet_address);
        let private_key = norm(private_key).map(Zeroizing::new);

        // Any rejection clears live credentials first — a failed update must
        // never leave a stale signer loaded.
        let clear = || {
            *self.signer.write() = None;
            *self.account.write() = None;
        };
        let signer = match private_key.as_deref() {
            Some(k) => match k.trim().parse::<PrivateKeySigner>() {
                Ok(s) => Some(s),
                Err(e) => {
                    clear();
                    anyhow::bail!("{} invalid EVM private key: {e}", self.name);
                }
            },
            None => None,
        };
        let acct = match wallet_address.as_deref() {
            Some(a) => match parse_addr(a) {
                Ok(a) => Some(a),
                Err(e) => {
                    clear();
                    anyhow::bail!("{} invalid wallet address: {e}", self.name);
                }
            },
            None => None,
        };
        // Key/wallet mismatch is a misconfiguration — refuse both rather
        // than sign from an unexpected address.
        if let (Some(s), Some(a)) = (&signer, &acct) {
            if s.address() != *a {
                clear();
                anyhow::bail!(
                    "{} wallet address {a} does not match the private key's signer {} — refusing credentials",
                    self.name,
                    s.address()
                );
            }
        }
        *self.signer.write() = signer;
        *self.account.write() = acct;
        Ok(())
    }

    pub fn wallet_address_hex(&self) -> Option<String> {
        self.account
            .read()
            .map(|a| a.to_string())
            .or_else(|| self.signer.read().as_ref().map(|s| s.address().to_string()))
    }

    // -- RPC plumbing ------------------------------------------------------

    fn rpc(&self) -> anyhow::Result<alloy::transports::http::reqwest::Url> {
        self.rpc_url
            .read()
            .parse()
            .map_err(|e| anyhow::anyhow!("{} bad RPC url: {e}", self.name))
    }

    fn http_provider(&self) -> anyhow::Result<impl Provider> {
        Ok(ProviderBuilder::new().connect_http(self.rpc()?))
    }

    /// Verify the active RPC serves the expected chain. The result is
    /// cached per endpoint URL — a changed RPC always re-verifies, an
    /// unchanged one skips the `eth_chainId` round-trip.
    async fn check_chain(&self) -> anyhow::Result<()> {
        let url = self.rpc_url.read().clone();
        if self.verified_rpc_url.read().as_ref().map(|v| v.as_str()) == Some(url.as_str()) {
            return Ok(());
        }
        let provider = self.http_provider()?;
        let chain_id = provider
            .get_chain_id()
            .await
            .map_err(|e| anyhow::anyhow!("{} RPC unreachable: {e}", self.name))?;
        if chain_id != self.spec.chain_id {
            anyhow::bail!(
                "{} chain id mismatch: node reports {chain_id}, expected {} ({}) — wrong network, refusing",
                self.name,
                self.spec.chain_id,
                self.spec.chain_label
            );
        }
        *self.verified_rpc_url.write() = Some(url);
        Ok(())
    }

    /// `eth_call` at the latest block. Revert → Err (callers decide whether
    /// a missing pool is skippable or fatal).
    async fn call(&self, to: Address, calldata: Vec<u8>) -> anyhow::Result<Bytes> {
        let provider = self.http_provider()?;
        let tx = TransactionRequest::default()
            .with_to(to)
            .with_input(Bytes::from(calldata));
        provider
            .call(tx)
            .latest()
            .await
            .map_err(|e| anyhow::anyhow!("{} eth_call to {to} reverted: {e}", self.name))
    }

    // -- Spec lookups ------------------------------------------------------

    fn token(&self, symbol: &str) -> anyhow::Result<&'static TokenSpec> {
        self.spec
            .tokens
            .iter()
            .find(|t| t.symbol.eq_ignore_ascii_case(symbol))
            .ok_or_else(|| anyhow::anyhow!("{} has no configured token {symbol}", self.name))
    }

    fn pair_spec(&self, base: &str, quote: &str) -> Option<&'static PairSpec> {
        self.spec
            .pairs
            .iter()
            .find(|p| p.base.eq_ignore_ascii_case(base) && p.quote.eq_ignore_ascii_case(quote))
    }

    /// Canonical "BASE-QUOTE" → pair + token specs. Rejects unknown pairs.
    fn resolve_symbol(
        &self,
        symbol: &str,
    ) -> anyhow::Result<(&'static PairSpec, &'static TokenSpec, &'static TokenSpec)> {
        let (base, quote) = symbol
            .split_once('-')
            .ok_or_else(|| anyhow::anyhow!("{} symbol {symbol} is not BASE-QUOTE", self.name))?;
        let pair = self
            .pair_spec(base, quote)
            .ok_or_else(|| anyhow::anyhow!("{symbol} is not a configured {} pair", self.name))?;
        Ok((pair, self.token(pair.base)?, self.token(pair.quote)?))
    }

    fn fee_bps_for(&self, pair: &PairSpec) -> u32 {
        match self.spec.kind {
            AmmKind::UniV3 => pair.fee / 100,
            AmmKind::UniV2 => self.spec.taker_fee_bps,
            AmmKind::Solidly { .. } => {
                if pair.stable {
                    self.spec.stable_fee_bps
                } else {
                    self.spec.taker_fee_bps
                }
            }
        }
    }

    // -- Onchain reads ------------------------------------------------------

    /// Factory pool lookup → pair/pool contract address, or zero.
    async fn pool_address(
        &self,
        pair: &PairSpec,
        base: &TokenSpec,
        quote: &TokenSpec,
    ) -> anyhow::Result<Address> {
        let factory = parse_addr(self.spec.factory)?;
        let a = parse_addr(base.address)?;
        let b = parse_addr(quote.address)?;
        let (calldata, to) = match self.spec.kind {
            AmmKind::UniV2 => (
                IUniV2Factory::getPairCall {
                    tokenA: a,
                    tokenB: b,
                }
                .abi_encode(),
                factory,
            ),
            AmmKind::UniV3 => (
                IUniV3Factory::getPoolCall {
                    tokenA: a,
                    tokenB: b,
                    fee: alloy::primitives::Uint::<24, 1>::from(pair.fee),
                }
                .abi_encode(),
                factory,
            ),
            AmmKind::Solidly { getpool_lookup, .. } => {
                if getpool_lookup {
                    (
                        ISolidlyFactoryGetPool::getPoolCall {
                            tokenA: a,
                            tokenB: b,
                            stable: pair.stable,
                        }
                        .abi_encode(),
                        factory,
                    )
                } else {
                    (
                        ISolidlyFactoryGetPair::getPairCall {
                            tokenA: a,
                            tokenB: b,
                            stable: pair.stable,
                        }
                        .abi_encode(),
                        factory,
                    )
                }
            }
        };
        let out = self.call(to, calldata).await?;
        let addr = match self.spec.kind {
            AmmKind::UniV2 => IUniV2Factory::getPairCall::abi_decode_returns(&out)?,
            AmmKind::UniV3 => IUniV3Factory::getPoolCall::abi_decode_returns(&out)?,
            AmmKind::Solidly { getpool_lookup, .. } => {
                if getpool_lookup {
                    ISolidlyFactoryGetPool::getPoolCall::abi_decode_returns(&out)?
                } else {
                    ISolidlyFactoryGetPair::getPairCall::abi_decode_returns(&out)?
                }
            }
        };
        Ok(addr)
    }

    /// Exact-input quote: `amount_in` of `in_tok` → `out_tok` units.
    async fn quote_exact_in(
        &self,
        pair: &PairSpec,
        in_tok: &TokenSpec,
        out_tok: &TokenSpec,
        amount_in: U256,
    ) -> anyhow::Result<U256> {
        if amount_in.is_zero() {
            anyhow::bail!("quote amount must be positive");
        }
        let router = parse_addr(self.spec.router)?;
        let tin = parse_addr(in_tok.address)?;
        let tout = parse_addr(out_tok.address)?;
        match self.spec.kind {
            AmmKind::UniV2 => {
                let call = IUniV2Router::getAmountsOutCall {
                    amountIn: amount_in,
                    path: vec![tin, tout],
                };
                let out = self.call(router, call.abi_encode()).await?;
                let amounts = IUniV2Router::getAmountsOutCall::abi_decode_returns(&out)?;
                amounts
                    .last()
                    .copied()
                    .filter(|v| !v.is_zero())
                    .ok_or_else(|| anyhow::anyhow!("{} quote returned empty/zero", self.name))
            }
            AmmKind::UniV3 => {
                let quoter =
                    parse_addr(self.spec.quoter.ok_or_else(|| {
                        anyhow::anyhow!("{} has no quoter configured", self.name)
                    })?)?;
                let call = IQuoterV2::quoteExactInputSingleCall {
                    params: IQuoterV2::QuoteExactInputSingleParams {
                        tokenIn: tin,
                        tokenOut: tout,
                        amountIn: amount_in,
                        fee: alloy::primitives::Uint::<24, 1>::from(pair.fee),
                        sqrtPriceLimitX96: alloy::primitives::Uint::<160, 3>::ZERO,
                    },
                };
                let out = self.call(quoter, call.abi_encode()).await?;
                let ret = IQuoterV2::quoteExactInputSingleCall::abi_decode_returns(&out)?;
                if ret.amountOut.is_zero() {
                    anyhow::bail!("{} quote returned zero", self.name);
                }
                Ok(ret.amountOut)
            }
            AmmKind::Solidly {
                route_has_factory, ..
            } => {
                let factory = parse_addr(self.spec.factory)?;
                let out = if route_has_factory {
                    let call = ISolidlyRouter4::getAmountsOutCall {
                        amountIn: amount_in,
                        routes: vec![ISolidlyRouter4::Route {
                            from: tin,
                            to: tout,
                            stable: pair.stable,
                            factory,
                        }],
                    };
                    self.call(router, call.abi_encode()).await?
                } else {
                    let call = ISolidlyRouter3::getAmountsOutCall {
                        amountIn: amount_in,
                        routes: vec![ISolidlyRouter3::Route {
                            from: tin,
                            to: tout,
                            stable: pair.stable,
                        }],
                    };
                    self.call(router, call.abi_encode()).await?
                };
                let amounts = if route_has_factory {
                    ISolidlyRouter4::getAmountsOutCall::abi_decode_returns(&out)?
                } else {
                    ISolidlyRouter3::getAmountsOutCall::abi_decode_returns(&out)?
                };
                amounts
                    .last()
                    .copied()
                    .filter(|v| !v.is_zero())
                    .ok_or_else(|| anyhow::anyhow!("{} quote returned empty/zero", self.name))
            }
        }
    }

    /// Exact-output quote (UniV2/UniV3 only — Solidly routers lack it).
    async fn quote_exact_out(
        &self,
        pair: &PairSpec,
        in_tok: &TokenSpec,
        out_tok: &TokenSpec,
        amount_out: U256,
    ) -> anyhow::Result<U256> {
        let router = parse_addr(self.spec.router)?;
        let tin = parse_addr(in_tok.address)?;
        let tout = parse_addr(out_tok.address)?;
        match self.spec.kind {
            AmmKind::UniV2 => {
                let call = IUniV2Router::getAmountsInCall {
                    amountOut: amount_out,
                    path: vec![tin, tout],
                };
                let out = self.call(router, call.abi_encode()).await?;
                IUniV2Router::getAmountsInCall::abi_decode_returns(&out)?
                    .first()
                    .copied()
                    .filter(|v| !v.is_zero())
                    .ok_or_else(|| anyhow::anyhow!("{} getAmountsIn empty/zero", self.name))
            }
            AmmKind::UniV3 => {
                let quoter =
                    parse_addr(self.spec.quoter.ok_or_else(|| {
                        anyhow::anyhow!("{} has no quoter configured", self.name)
                    })?)?;
                let call = IQuoterV2::quoteExactOutputSingleCall {
                    params: IQuoterV2::QuoteExactOutputSingleParams {
                        tokenIn: tin,
                        tokenOut: tout,
                        amount: amount_out,
                        fee: alloy::primitives::Uint::<24, 1>::from(pair.fee),
                        sqrtPriceLimitX96: alloy::primitives::Uint::<160, 3>::ZERO,
                    },
                };
                let out = self.call(quoter, call.abi_encode()).await?;
                let ret = IQuoterV2::quoteExactOutputSingleCall::abi_decode_returns(&out)?;
                if ret.amountIn.is_zero() {
                    anyhow::bail!("{} quote returned zero", self.name);
                }
                Ok(ret.amountIn)
            }
            AmmKind::Solidly { .. } => {
                anyhow::bail!("{} router has no exact-output primitive", self.name)
            }
        }
    }

    async fn erc20_balance(&self, token: &TokenSpec, owner: Address) -> anyhow::Result<U256> {
        let call = IERC20::balanceOfCall { account: owner };
        let out = self
            .call(parse_addr(token.address)?, call.abi_encode())
            .await?;
        Ok(IERC20::balanceOfCall::abi_decode_returns(&out)?)
    }

    async fn erc20_allowance(
        &self,
        token: &TokenSpec,
        owner: Address,
        spender: Address,
    ) -> anyhow::Result<U256> {
        let call = IERC20::allowanceCall { owner, spender };
        let out = self
            .call(parse_addr(token.address)?, call.abi_encode())
            .await?;
        Ok(IERC20::allowanceCall::abi_decode_returns(&out)?)
    }

    /// One-unit reference price for a pair: `1 base` → quote units.
    async fn pair_mid_price(
        &self,
        pair: &PairSpec,
        base: &TokenSpec,
        quote: &TokenSpec,
    ) -> anyhow::Result<Decimal> {
        let one = U256::from(10u128.pow(base.decimals as u32));
        let out = self.quote_exact_in(pair, base, quote, one).await?;
        Ok(u256_to_dec(out, quote.decimals))
    }

    // -- Swap calldata -------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    /// Build the swap calldata for `side`. Sell = exact-in `amount_in`
    /// producing ≥ `bound`; Buy = exact-out `qty` spending ≤ `bound`
    /// (UniV2/UniV3) or exact-in `bound`-capped producing ≥ `qty` (Solidly —
    /// `amount_in` is then the spend estimate and `bound` the max spend).
    fn build_swap_calldata(
        &self,
        pair: &PairSpec,
        side: Side,
        in_tok: &TokenSpec,
        out_tok: &TokenSpec,
        amount: U256,
        bound: U256,
        recipient: Address,
    ) -> anyhow::Result<Vec<u8>> {
        let router_to = recipient;
        let deadline = deadline_ts();
        let tin = parse_addr(in_tok.address)?;
        let tout = parse_addr(out_tok.address)?;
        let data = match self.spec.kind {
            AmmKind::UniV2 => match side {
                Side::Sell => IUniV2Router::swapExactTokensForTokensCall {
                    amountIn: amount,
                    amountOutMin: bound,
                    path: vec![tin, tout],
                    to: router_to,
                    deadline,
                }
                .abi_encode(),
                Side::Buy => IUniV2Router::swapTokensForExactTokensCall {
                    amountOut: amount,
                    amountInMax: bound,
                    path: vec![tin, tout],
                    to: router_to,
                    deadline,
                }
                .abi_encode(),
            },
            AmmKind::UniV3 => {
                let fee = alloy::primitives::Uint::<24, 1>::from(pair.fee);
                let zero = alloy::primitives::Uint::<160, 3>::ZERO;
                match side {
                    Side::Sell => ISwapRouterV3::exactInputSingleCall {
                        params: ISwapRouterV3::ExactInputSingleParams {
                            tokenIn: tin,
                            tokenOut: tout,
                            fee,
                            recipient,
                            deadline,
                            amountIn: amount,
                            amountOutMinimum: bound,
                            sqrtPriceLimitX96: zero,
                        },
                    }
                    .abi_encode(),
                    Side::Buy => ISwapRouterV3::exactOutputSingleCall {
                        params: ISwapRouterV3::ExactOutputSingleParams {
                            tokenIn: tin,
                            tokenOut: tout,
                            fee,
                            recipient,
                            deadline,
                            amountOut: amount,
                            amountInMaximum: bound,
                            sqrtPriceLimitX96: zero,
                        },
                    }
                    .abi_encode(),
                }
            }
            AmmKind::Solidly {
                route_has_factory, ..
            } => {
                // Buy collapses to exact-in here: `amount` is the spend
                // estimate and `bound` is `amountOutMin` (target qty).
                let (amt_in, min_out) = match side {
                    Side::Sell => (amount, bound),
                    Side::Buy => (bound, amount),
                };
                if route_has_factory {
                    ISolidlyRouter4::swapExactTokensForTokensCall {
                        amountIn: amt_in,
                        amountOutMin: min_out,
                        routes: vec![ISolidlyRouter4::Route {
                            from: tin,
                            to: tout,
                            stable: pair.stable,
                            factory: parse_addr(self.spec.factory)?,
                        }],
                        to: router_to,
                        deadline,
                    }
                    .abi_encode()
                } else {
                    ISolidlyRouter3::swapExactTokensForTokensCall {
                        amountIn: amt_in,
                        amountOutMin: min_out,
                        routes: vec![ISolidlyRouter3::Route {
                            from: tin,
                            to: tout,
                            stable: pair.stable,
                        }],
                        to: router_to,
                        deadline,
                    }
                    .abi_encode()
                }
            }
        };
        Ok(data)
    }

    // -- Receipt truth --------------------------------------------------------

    /// Sum ERC-20 `Transfer` amounts in a receipt where `token` moves
    /// `from` → `to` (either side may be `None` = wildcard).
    fn transfer_sum(
        logs: &[alloy::rpc::types::Log],
        token: Address,
        from: Option<Address>,
        to: Option<Address>,
    ) -> U256 {
        let topic0 = B256::from_str(TRANSFER_TOPIC).unwrap_or_default();
        let mut sum = U256::ZERO;
        for log in logs {
            if log.address() != token {
                continue;
            }
            let topics = log.topics();
            if topics.len() != 3 || topics[0] != topic0 {
                continue;
            }
            let src = Address::from_slice(&topics[1][12..]);
            let dst = Address::from_slice(&topics[2][12..]);
            if let Some(f) = from {
                if src != f {
                    continue;
                }
            }
            if let Some(t) = to {
                if dst != t {
                    continue;
                }
            }
            let data = log.data().data.as_ref();
            if data.len() == 32 {
                sum = sum.saturating_add(U256::from_be_slice(data));
            }
        }
        sum
    }

    /// Decode (input spent, output received) for a swap receipt. Fails
    /// closed unless BOTH legs are proven: an output-token transfer to the
    /// signer AND an input-token transfer from the signer — a "successful"
    /// status alone never counts as a fill.
    fn decode_swap_fill(
        logs: &[alloy::rpc::types::Log],
        signer: Address,
        in_tok: &TokenSpec,
        out_tok: &TokenSpec,
    ) -> anyhow::Result<(U256, U256)> {
        let tin = parse_addr(in_tok.address)?;
        let tout = parse_addr(out_tok.address)?;
        let out_received = Self::transfer_sum(logs, tout, None, Some(signer));
        if out_received.is_zero() {
            anyhow::bail!(
                "receipt has no {} transfer to signer — cannot prove a fill",
                out_tok.symbol
            );
        }
        let in_spent = Self::transfer_sum(logs, tin, Some(signer), None);
        if in_spent.is_zero() {
            anyhow::bail!(
                "receipt has no {} transfer from signer — cannot prove the spend",
                in_tok.symbol
            );
        }
        Ok((in_spent, out_received))
    }

    /// Ensure `spender` may move `needed` of `token`, submitting an approval
    /// tx when short. Exact-amount approvals (no unlimited grants); USDT-style
    /// tokens that reject nonzero→nonzero changes get a reset-then-set.
    async fn ensure_allowance(
        &self,
        provider: &impl Provider,
        token: &TokenSpec,
        owner: Address,
        needed: U256,
    ) -> anyhow::Result<()> {
        let spender = parse_addr(self.spec.router)?;
        let current = self.erc20_allowance(token, owner, spender).await?;
        if current >= needed {
            return Ok(());
        }
        let approve = |amount: U256| IERC20::approveCall { spender, amount }.abi_encode();
        // Reset first when a nonzero allowance exists (USDT policy).
        if !current.is_zero() {
            self.send_signed(
                provider,
                parse_addr(token.address)?,
                approve(U256::ZERO),
                "approve-reset",
            )
            .await?;
        }
        self.send_signed(
            provider,
            parse_addr(token.address)?,
            approve(needed),
            "approve",
        )
        .await?;
        Ok(())
    }

    /// Estimate gas (from the loaded signer address) and build the
    /// transaction request: +25% gas headroom, gas price, chain id.
    /// Revert at estimate time fails closed before any nonce burns.
    async fn prepare_signed_transaction(
        &self,
        provider: &impl Provider,
        to: Address,
        calldata: Vec<u8>,
        what: &str,
    ) -> anyhow::Result<TransactionRequest> {
        let from = self
            .signer
            .read()
            .as_ref()
            .map(|s| s.address())
            .ok_or_else(|| anyhow::anyhow!("{} has no signing wallet", self.name))?;
        let gas_price = provider
            .get_gas_price()
            .await
            .map_err(|e| anyhow::anyhow!("{} gas price query failed: {e}", self.name))?;
        let estimate_tx = TransactionRequest::default()
            .with_from(from)
            .with_to(to)
            .with_input(Bytes::from(calldata.clone()));
        let gas_limit = provider.estimate_gas(estimate_tx).await.map_err(|e| {
            anyhow::anyhow!(
                "{} estimateGas reverted for {what} (would fail): {e}",
                self.name
            )
        })?;
        let gas_limit = gas_limit
            .saturating_mul(125)
            .saturating_div(100)
            .max(60_000);
        Ok(TransactionRequest::default()
            .with_to(to)
            .with_input(Bytes::from(calldata))
            .with_chain_id(self.spec.chain_id)
            .with_gas_price(gas_price)
            .with_gas_limit(gas_limit))
    }

    /// Approvals only: prepare → send → await mined receipt (30s) →
    /// require success. A reverted or timed-out approval fails closed.
    async fn send_signed(
        &self,
        provider: &impl Provider,
        to: Address,
        calldata: Vec<u8>,
        what: &str,
    ) -> anyhow::Result<alloy::rpc::types::TransactionReceipt> {
        let tx = self
            .prepare_signed_transaction(provider, to, calldata, what)
            .await?;
        let pending = provider
            .send_transaction(tx)
            .await
            .map_err(|e| anyhow::anyhow!("{} sendTransaction failed for {what}: {e}", self.name))?;
        let tx_hash = format!("{:#x}", pending.tx_hash());
        let receipt = tokio::time::timeout(Duration::from_secs(30), pending.get_receipt())
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "{} receipt timeout for {tx_hash} — check the explorer before retrying; do NOT resubmit blindly",
                    self.name
                )
            })?
            .map_err(|e| anyhow::anyhow!("{} receipt query failed for {tx_hash}: {e}", self.name))?;
        if !receipt.status() {
            anyhow::bail!("{} {what} reverted onchain: {tx_hash}", self.name);
        }
        Ok(receipt)
    }
}

// ---------------------------------------------------------------------------
// ExchangeAdapter
// ---------------------------------------------------------------------------

#[async_trait]
impl ExchangeAdapter for EvmAmmAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        self.name
    }

    async fn ping(&self) -> anyhow::Result<()> {
        self.check_chain().await
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        self.check_chain().await?;
        let mut out = Vec::new();
        for pair in self.spec.pairs {
            let (base_tok, quote_tok) = (self.token(pair.base)?, self.token(pair.quote)?);
            // A missing/empty pool skips the pair — never invent a market.
            let pool = match self.pool_address(pair, base_tok, quote_tok).await {
                Ok(a) => a,
                Err(_) => continue,
            };
            if pool == Address::ZERO {
                continue;
            }
            let fee = Decimal::from(self.fee_bps_for(pair));
            out.push(MarketMetadata {
                symbol: Symbol::new(pair.base, pair.quote, MarketType::Spot),
                venue: self.venue_id.clone(),
                venue_symbol: format!("{}-{}", pair.base, pair.quote),
                price_decimals: 8,
                size_decimals: base_tok.decimals as u32,
                min_order_size: Decimal::ZERO,
                min_notional: Decimal::ZERO,
                maker_fee_bps: fee,
                taker_fee_bps: fee,
                contract_size: Decimal::ONE,
                is_active: true,
            });
        }
        Ok(out)
    }

    async fn fetch_market_prices(
        &self,
    ) -> anyhow::Result<std::collections::HashMap<String, Decimal>> {
        self.check_chain().await?;
        let mut map = std::collections::HashMap::new();
        for pair in self.spec.pairs {
            let (Ok(base_tok), Ok(quote_tok)) = (self.token(pair.base), self.token(pair.quote))
            else {
                continue;
            };
            if let Ok(price) = self.pair_mid_price(pair, base_tok, quote_tok).await {
                if price > Decimal::ZERO {
                    map.insert(format!("{}-{}", pair.base, pair.quote), price);
                }
            }
        }
        Ok(map)
    }

    /// AMMs have no L2 book — this synthesizes a single level per side from
    /// the live router/quoter quote (price at which the probe size executes
    /// *right now*). Documented approximation, not fabricated depth: both
    /// sides must quote, zero/crossed results fail instead of clamping.
    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        self.check_chain().await?;
        let (pair, base_tok, quote_tok) = self.resolve_symbol(symbol)?;
        let probe = U256::from(10u128.pow(base_tok.decimals as u32)); // 1 base unit
                                                                      // Bid: exact-in one base → quote out.
        let out = self
            .quote_exact_in(pair, base_tok, quote_tok, probe)
            .await?;
        let bid = u256_to_dec(out, quote_tok.decimals);
        if bid <= Decimal::ZERO {
            anyhow::bail!("{} bid quote for {symbol} returned zero", self.name);
        }
        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());
        book.bids.push(BookLevel::new(bid, Decimal::ONE));
        if self.execution_supported() {
            // Standard routers: exact-out — quote cost to buy exactly 1 base.
            let in_amt = self
                .quote_exact_out(pair, quote_tok, base_tok, probe)
                .await?;
            let ask = u256_to_dec(in_amt, quote_tok.decimals);
            if ask <= Decimal::ZERO {
                anyhow::bail!("{} ask quote for {symbol} returned zero", self.name);
            }
            if ask < bid {
                anyhow::bail!(
                    "{} crossed book for {symbol}: ask {ask} < bid {bid}",
                    self.name
                );
            }
            book.asks.push(BookLevel::new(ask, Decimal::ONE));
        } else {
            // Market-data-only routers (Camelot referrer ABI, Solidly family)
            // have no exact-out primitive: reverse exact-in of one quote unit
            // → base out; ask price = 1/base_out and the quoted base amount
            // is the actual ask depth, never fabricated as 1.
            let qprobe = U256::from(10u128.pow(quote_tok.decimals as u32));
            let back = self
                .quote_exact_in(pair, quote_tok, base_tok, qprobe)
                .await?;
            let base_out = u256_to_dec(back, base_tok.decimals);
            if base_out <= Decimal::ZERO {
                anyhow::bail!("{} ask quote for {symbol} returned zero", self.name);
            }
            let ask = Decimal::ONE / base_out;
            if ask < bid {
                anyhow::bail!(
                    "{} crossed book for {symbol}: ask {ask} < bid {bid}",
                    self.name
                );
            }
            book.asks.push(BookLevel::new(ask, base_out));
        }
        Ok(book)
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let book = self.fetch_orderbook(symbol).await?;
        let bid = book
            .bids
            .first()
            .map(|l| l.price)
            .ok_or_else(|| anyhow::anyhow!("{} book for {symbol} has no bid", self.name))?;
        let ask = book
            .asks
            .first()
            .map(|l| l.price)
            .ok_or_else(|| anyhow::anyhow!("{} book for {symbol} has no ask", self.name))?;
        Ok(Ticker {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            bid,
            ask,
            last: bid,
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate> {
        anyhow::bail!("{} is a spot AMM — {symbol} has no funding rate", self.name)
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        self.check_chain().await?;
        let owner = self
            .signer
            .read()
            .as_ref()
            .map(|s| s.address())
            .or_else(|| *self.account.read())
            .ok_or_else(|| {
                anyhow::anyhow!("{} account snapshot needs a wallet address", self.name)
            })?;
        let provider = self.http_provider()?;
        let mut balances = Vec::new();
        // Native balance.
        let native = provider
            .get_balance(owner)
            .await
            .map_err(|e| anyhow::anyhow!("{} native balance query failed: {e}", self.name))?;
        if !native.is_zero() {
            balances.push(Balance {
                venue: self.venue_id.clone(),
                currency: self.spec.native_symbol.to_string(),
                total: u256_to_dec(native, 18),
                free: u256_to_dec(native, 18),
                locked: Decimal::ZERO,
            });
        }
        // Configured tokens.
        for tok in self.spec.tokens {
            if let Ok(bal) = self.erc20_balance(tok, owner).await {
                if bal.is_zero() {
                    continue;
                }
                let amt = u256_to_dec(bal, tok.decimals);
                balances.push(Balance {
                    venue: self.venue_id.clone(),
                    currency: tok.symbol.to_string(),
                    total: amt,
                    free: amt,
                    locked: Decimal::ZERO,
                });
            }
        }
        // Equity: stables at face; everything else priced via a configured
        // pair into a stable quote when one exists. Unquotable holdings are
        // still listed but excluded from equity — never invent a price.
        let mut equity = Decimal::ZERO;
        for b in &balances {
            if is_stable_symbol(&b.currency) {
                equity += b.total;
                continue;
            }
            let sym = if b.currency == self.spec.native_symbol {
                self.spec.native_wrapped
            } else {
                b.currency.as_str()
            };
            let priced = self
                .spec
                .pairs
                .iter()
                .find(|p| p.base.eq_ignore_ascii_case(sym) && is_stable_symbol(p.quote));
            if let Some(pair) = priced {
                if let (Ok(bt), Ok(qt)) = (self.token(pair.base), self.token(pair.quote)) {
                    if let Ok(px) = self.pair_mid_price(pair, bt, qt).await {
                        equity += b.total * px;
                    }
                }
            }
        }
        Ok(AccountSnapshot {
            venue: self.venue_id.clone(),
            equity,
            free_margin: equity,
            maintenance_margin: Decimal::ZERO,
            margin_usage_pct: Decimal::ZERO,
            balances,
            positions: Vec::new(),
            timestamp: Utc::now(),
        })
    }

    async fn place_order(&self, order: OrderRequest) -> anyhow::Result<OrderResponse> {
        if !self.execution_supported() {
            anyhow::bail!(
                "{} is admitted for market data only — its router ABI is not validated for live swaps; no transaction submitted",
                self.name
            );
        }
        let signer_addr = match self.signer.read().as_ref().map(|s| s.address()) {
            Some(a) => a,
            None => anyhow::bail!(
                "{} live swap needs an EVM private key credential",
                self.name
            ),
        };
        if order.post_only || order.order_type == OrderType::PostOnly {
            anyhow::bail!(
                "{} cannot post resting orders — AMM swaps execute atomically",
                self.name
            );
        }
        if order.quantity <= Decimal::ZERO {
            anyhow::bail!("{} order quantity must be positive", self.name);
        }
        let (pair, base_tok, quote_tok) = self.resolve_symbol(&order.symbol)?;
        let (in_tok, out_tok) = match order.side {
            Side::Sell => (base_tok, quote_tok),
            Side::Buy => (quote_tok, base_tok),
        };
        let _swap_guard = self.swap_lock.lock().await;
        self.check_chain().await?;

        // Quote + bounds. `swap_amount`/`swap_bound` semantics differ per
        // family — see build_swap_calldata.
        let (swap_amount, swap_bound) = match order.side {
            Side::Sell => {
                let amount_in = dec_to_u256(order.quantity, base_tok.decimals)?;
                let quoted_out = self
                    .quote_exact_in(pair, in_tok, out_tok, amount_in)
                    .await?;
                let quoted_px = u256_to_dec(quoted_out, quote_tok.decimals) / order.quantity;
                let min_out = match order.price {
                    Some(limit) => {
                        // IOC limit: refuse if the live quote can't fill at limit.
                        if quoted_px < limit {
                            anyhow::bail!(
                                "{} {symbol} quote {quoted_px} is below sell limit {limit} — not marketable",
                                self.name, symbol = order.symbol
                            );
                        }
                        dec_to_u256(order.quantity * limit, quote_tok.decimals)?
                    }
                    None => {
                        quoted_out.saturating_mul(U256::from(10_000 - DEFAULT_SLIPPAGE_BPS))
                            / U256::from(10_000u32)
                    }
                };
                (amount_in, min_out)
            }
            Side::Buy => {
                // Exact-out only — every live-admitted router implements it
                // (the Solidly-family venues without it never reach here).
                let qty_out = dec_to_u256(order.quantity, base_tok.decimals)?;
                let quoted_in = self.quote_exact_out(pair, in_tok, out_tok, qty_out).await?;
                let max_in = match order.price {
                    Some(limit) => {
                        let cap = dec_to_u256(order.quantity * limit, quote_tok.decimals)?;
                        if quoted_in > cap {
                            anyhow::bail!(
                                "{} buy quote exceeds limit {} — not marketable",
                                self.name,
                                limit
                            );
                        }
                        cap
                    }
                    None => {
                        quoted_in.saturating_mul(U256::from(10_000 + DEFAULT_SLIPPAGE_BPS))
                            / U256::from(10_000u32)
                    }
                };
                // exact-out swap: amount = desired out, bound = max in
                return self
                    .execute_swap(&order, pair, in_tok, out_tok, qty_out, max_in, signer_addr)
                    .await;
            }
        };
        self.execute_swap(
            &order,
            pair,
            in_tok,
            out_tok,
            swap_amount,
            swap_bound,
            signer_addr,
        )
        .await
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        anyhow::bail!(
            "{} swaps settle atomically onchain — nothing rests to cancel",
            self.name
        )
    }

    async fn fetch_order(
        &self,
        symbol: &str,
        order_id: &str,
        client_order_id: Option<&str>,
    ) -> anyhow::Result<OrderResponse> {
        self.check_chain().await?;
        // Side isn't persisted in the order_id — derive direction from which
        // leg moved: try sell first (base→quote), fall back to buy.
        let (_, base_tok, quote_tok) = self.resolve_symbol(symbol)?;
        let signer = self
            .signer
            .read()
            .as_ref()
            .map(|s| s.address())
            .or_else(|| *self.account.read())
            .ok_or_else(|| anyhow::anyhow!("{} fetch_order needs the wallet", self.name))?;
        let hash: B256 = order_id
            .parse()
            .map_err(|e| anyhow::anyhow!("{} bad order/tx id {order_id}: {e}", self.name))?;
        let provider = self.http_provider()?;
        let receipt = provider
            .get_transaction_receipt(hash)
            .await
            .map_err(|e| anyhow::anyhow!("{} receipt query failed for {order_id}: {e}", self.name))?
            .ok_or_else(|| {
                anyhow::anyhow!("{} tx {order_id} not yet mined — still pending", self.name)
            })?;
        // A mined tx always paid gas — report the real native fee.
        let gas_fee_wei =
            U256::from(receipt.gas_used).saturating_mul(U256::from(receipt.effective_gas_price));
        let gas_fee = u256_to_dec(gas_fee_wei, 18);
        if !receipt.status() {
            return Ok(OrderResponse {
                order_id: order_id.to_string(),
                client_order_id: client_order_id.unwrap_or_default().to_string(),
                venue: self.venue_id.clone(),
                symbol: symbol.to_string(),
                status: OrderStatus::Rejected,
                original_qty: Decimal::ZERO,
                filled_qty: Decimal::ZERO,
                remaining_qty: Decimal::ZERO,
                avg_fill_price: None,
                fee: gas_fee,
                fee_currency: self.spec.native_symbol.to_string(),
                raw_response: Some(format!(
                    "tx {order_id} reverted onchain (gas {gas_fee} {})",
                    self.spec.native_symbol
                )),
                timestamp: Utc::now(),
            });
        }
        let logs = receipt.inner.logs();
        // Direction detection: output token received by signer decides.
        let got_base = Self::transfer_sum(logs, parse_addr(base_tok.address)?, None, Some(signer));
        let got_quote =
            Self::transfer_sum(logs, parse_addr(quote_tok.address)?, None, Some(signer));
        let (side, in_tok, out_tok) = if !got_base.is_zero() && got_quote.is_zero() {
            (Side::Buy, quote_tok, base_tok)
        } else if !got_quote.is_zero() && got_base.is_zero() {
            (Side::Sell, base_tok, quote_tok)
        } else {
            anyhow::bail!(
                "{} tx {order_id} receipt transfers are ambiguous for {symbol} — refusing to guess the fill",
                self.name
            );
        };
        let (in_spent, out_received) = Self::decode_swap_fill(logs, signer, in_tok, out_tok)?;
        let (base_units, quote_units) = match side {
            Side::Sell => (in_spent, out_received),
            Side::Buy => (out_received, in_spent),
        };
        let filled_base = u256_to_dec(base_units, base_tok.decimals);
        let quote_amt = u256_to_dec(quote_units, quote_tok.decimals);
        Ok(OrderResponse {
            order_id: order_id.to_string(),
            client_order_id: client_order_id.unwrap_or_default().to_string(),
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            status: OrderStatus::Filled,
            original_qty: filled_base,
            filled_qty: filled_base,
            remaining_qty: Decimal::ZERO,
            avg_fill_price: if filled_base > Decimal::ZERO {
                Some(quote_amt / filled_base)
            } else {
                None
            },
            fee: gas_fee,
            fee_currency: self.spec.native_symbol.to_string(),
            raw_response: Some(format!(
                "tx {order_id}: spent {} {} → received {} {} (gas {gas_fee} {})",
                u256_to_dec(in_spent, in_tok.decimals),
                in_tok.symbol,
                u256_to_dec(out_received, out_tok.decimals),
                out_tok.symbol,
                self.spec.native_symbol
            )),
            timestamp: Utc::now(),
        })
    }

    async fn set_leverage(&self, symbol: &str, _leverage: u32) -> anyhow::Result<()> {
        anyhow::bail!(
            "{} is a spot AMM — leverage is not meaningful for {symbol}",
            self.name
        )
    }

    fn supports_live_execution(&self) -> bool {
        self.execution_supported()
    }
    fn live_order_submission_implemented(&self) -> bool {
        self.execution_supported()
    }
    fn order_status_polling_implemented(&self) -> bool {
        self.execution_supported()
    }
    // Submission transport + receipt polling exist for explicit adapter
    // canaries, but automated multi-leg strategy entry is not admitted:
    // gas-aware profitability is not integrated and no credentialed swap
    // evidence exists.
    fn live_strategy_entry_implemented(&self) -> bool {
        false
    }
    fn has_live_credentials(&self) -> bool {
        self.signer.read().is_some()
    }
}

impl EvmAmmAdapter {
    /// Durable identity for a broadcast swap: the tx hash IS the order id,
    /// returned immediately as `New` so it reaches durable state before any
    /// receipt timeout or process crash. `fetch_order` is the only
    /// authority for the final fill state.
    fn submitted_order_response(&self, order: &OrderRequest, tx_hash: &str) -> OrderResponse {
        OrderResponse {
            order_id: tx_hash.to_string(),
            client_order_id: order.client_order_id.clone(),
            venue: self.venue_id.clone(),
            symbol: order.symbol.clone(),
            status: OrderStatus::New,
            original_qty: order.quantity,
            filled_qty: Decimal::ZERO,
            remaining_qty: order.quantity,
            avg_fill_price: None,
            fee: Decimal::ZERO,
            fee_currency: self.spec.native_symbol.to_string(),
            raw_response: Some(format!(
                "tx {tx_hash} submitted; receipt pending venue truth"
            )),
            timestamp: Utc::now(),
        }
    }

    /// Shared tail of `place_order`: allowance → calldata → prepare →
    /// broadcast → return the tx hash as a `New` order. The receipt is
    /// deliberately NOT awaited here.
    /// `amount`/`bound` meaning per side is set by the caller.
    #[allow(clippy::too_many_arguments)]
    async fn execute_swap(
        &self,
        order: &OrderRequest,
        pair: &PairSpec,
        in_tok: &TokenSpec,
        out_tok: &TokenSpec,
        amount: U256,
        bound: U256,
        signer_addr: Address,
    ) -> anyhow::Result<OrderResponse> {
        let router = parse_addr(self.spec.router)?;
        // Spend-side balance check (fail closed before any approval tx).
        let spend_needed = match order.side {
            Side::Sell => amount,
            Side::Buy => bound, // exact-out max-in bound
        };
        let bal = self.erc20_balance(in_tok, signer_addr).await?;
        if bal < spend_needed {
            anyhow::bail!(
                "{} wallet has {} {} — needs {} for this swap",
                self.name,
                u256_to_dec(bal, in_tok.decimals),
                in_tok.symbol,
                u256_to_dec(spend_needed, in_tok.decimals)
            );
        }
        let signer: PrivateKeySigner = self
            .signer
            .read()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("{} has no signing wallet", self.name))?;
        let provider = ProviderBuilder::new()
            .wallet(signer)
            .connect_http(self.rpc()?);
        self.ensure_allowance(&provider, in_tok, signer_addr, spend_needed)
            .await?;
        let calldata = self.build_swap_calldata(
            pair,
            order.side,
            in_tok,
            out_tok,
            amount,
            bound,
            signer_addr,
        )?;
        let tx = self
            .prepare_signed_transaction(&provider, router, calldata, "swap")
            .await?;
        let pending = provider
            .send_transaction(tx)
            .await
            .map_err(|e| anyhow::anyhow!("{} sendTransaction failed for swap: {e}", self.name))?;
        let tx_hash = format!("{:#x}", pending.tx_hash());
        Ok(self.submitted_order_response(order, &tx_hash))
    }
}

// ---------------------------------------------------------------------------
// Tests — pure logic; live RPC paths are covered by probe-verified encodings.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::hex;
    use arbitrageur_core::types::dec;

    fn adapter(id: &str) -> EvmAmmAdapter {
        EvmAmmAdapter::for_venue(id, None).expect(id)
    }

    #[test]
    fn spec_table_integrity() {
        let mut seen = std::collections::HashSet::new();
        for spec in SPECS {
            assert!(seen.insert(spec.id), "dup spec {}", spec.id);
            assert!(spec.chain_id > 0);
            parse_addr(spec.factory).unwrap();
            parse_addr(spec.router).unwrap();
            if let Some(q) = spec.quoter {
                parse_addr(q).unwrap();
            }
            assert!(spec.default_rpc.starts_with("https://"), "{} rpc", spec.id);
            assert!(!spec.tokens.is_empty(), "{} tokens", spec.id);
            assert!(!spec.pairs.is_empty(), "{} pairs", spec.id);
            for t in spec.tokens {
                parse_addr(t.address).unwrap();
                assert!(t.decimals <= 24, "{} {}", spec.id, t.symbol);
                assert!(!t.symbol.contains('-'));
            }
            for p in spec.pairs {
                assert!(
                    spec.tokens.iter().any(|t| t.symbol == p.base),
                    "{} pair base {} missing token",
                    spec.id,
                    p.base
                );
                assert!(
                    spec.tokens.iter().any(|t| t.symbol == p.quote),
                    "{} pair quote {} missing token",
                    spec.id,
                    p.quote
                );
                if matches!(spec.kind, AmmKind::UniV3) {
                    assert!(p.fee > 0 && p.fee <= 10000, "{} bad v3 fee", spec.id);
                }
            }
            // Wrapped native must be a configured token.
            assert!(spec.tokens.iter().any(|t| t.symbol == spec.native_wrapped));
        }
        // Admitted ids ⊆ spec ids; every admitted spec has admit=true.
        for id in ADMITTED_EVM_AMM_VENUE_IDS {
            assert!(spec_for(id).map(|s| s.admit).unwrap_or(false), "{id}");
        }
        // Live-execution ids ⊆ admitted ids.
        for id in LIVE_EVM_AMM_VENUE_IDS {
            assert!(ADMITTED_EVM_AMM_VENUE_IDS.contains(id), "{id}");
        }
        // Blackhole documented-deferred.
        assert!(!spec_for("blackhole").unwrap().admit);
    }

    #[test]
    fn dec_conversions() {
        assert_eq!(
            dec_to_u256(dec!(1.5), 18).unwrap(),
            U256::from(1_500_000_000_000_000_000u128)
        );
        assert_eq!(
            dec_to_u256(dec!(2500.123456), 6).unwrap(),
            U256::from(2_500_123_456u64)
        );
        assert!(dec_to_u256(dec!(0), 18).is_err());
        assert!(dec_to_u256(dec!(-1), 18).is_err());
        assert!(dec_to_u256(dec!(0.0000001), 6).is_err()); // sub-wei
        assert_eq!(u256_to_dec(U256::from(1_500_000u64), 6), dec!(1.5));
        assert_eq!(u256_to_dec(U256::ZERO, 18), Decimal::ZERO);
    }

    #[test]
    fn univ2_calldata_shape() {
        let a = adapter("uniswap_v2");
        let (pair, base, quote) = a.resolve_symbol("WETH-USDC").unwrap();
        let data = a
            .build_swap_calldata(
                pair,
                Side::Sell,
                base,
                quote,
                U256::from(1_000_000_000_000_000_000u128),
                U256::from(2_000_000u64),
                Address::ZERO,
            )
            .unwrap();
        // selector swapExactTokensForTokens(uint256,uint256,address[],address,uint256)
        assert_eq!(&data[..4], &hex!("38ed1739"));
        let buy = a
            .build_swap_calldata(
                pair,
                Side::Buy,
                quote,
                base,
                U256::from(1u64),
                U256::from(2u64),
                Address::ZERO,
            )
            .unwrap();
        assert_eq!(&buy[..4], &hex!("8803dbee"));
    }

    #[test]
    fn univ3_calldata_shape() {
        let a = adapter("uniswap");
        let (pair, base, quote) = a.resolve_symbol("WETH-USDC").unwrap();
        let data = a
            .build_swap_calldata(
                pair,
                Side::Sell,
                base,
                quote,
                U256::from(1u64),
                U256::from(2u64),
                Address::ZERO,
            )
            .unwrap();
        assert_eq!(&data[..4], &hex!("414bf389")); // exactInputSingle
        let buy = a
            .build_swap_calldata(
                pair,
                Side::Buy,
                quote,
                base,
                U256::from(1u64),
                U256::from(2u64),
                Address::ZERO,
            )
            .unwrap();
        assert_eq!(&buy[..4], &hex!("db3e2198")); // exactOutputSingle
    }

    #[test]
    fn solidly_calldata_shape() {
        let r3 = adapter("ramses");
        let (pair, base, quote) = r3.resolve_symbol("WETH-USDC").unwrap();
        let d3 = r3
            .build_swap_calldata(
                pair,
                Side::Sell,
                base,
                quote,
                U256::from(1u64),
                U256::from(2u64),
                Address::ZERO,
            )
            .unwrap();
        assert_eq!(&d3[..4], &hex!("f41766d8")); // 3-field route swap
        let r4 = adapter("aerodrome");
        let (pair, base, quote) = r4.resolve_symbol("WETH-USDC").unwrap();
        let d4 = r4
            .build_swap_calldata(
                pair,
                Side::Sell,
                base,
                quote,
                U256::from(1u64),
                U256::from(2u64),
                Address::ZERO,
            )
            .unwrap();
        assert_eq!(&d4[..4], &hex!("cac88ea9")); // 4-field route swap
    }

    #[test]
    fn symbol_resolution() {
        let a = adapter("camelot");
        assert!(a.resolve_symbol("ARB-WETH").is_ok());
        assert!(a.resolve_symbol("USDC-WETH").is_err()); // reversed pair not configured
        assert!(a.resolve_symbol("DOGE-USDC").is_err());
        assert!(a.resolve_symbol("WETH").is_err());
    }

    #[test]
    fn credentials_gate() {
        let a = adapter("uniswap");
        assert!(!a.has_live_credentials());
        // random valid secp256k1 scalar
        a.update_credentials(
            None,
            None,
            None,
            None,
            None,
            Some("0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d".into()),
        )
        .unwrap();
        assert!(a.has_live_credentials());
        assert_eq!(
            a.wallet_address_hex().unwrap().to_lowercase(),
            "0x70997970c51812dc3a010c7d01b50e0d17dc79c8"
        );
        // A failed re-provision clears the previously loaded signer — a
        // rejected update must never leave stale live credentials.
        assert!(a
            .update_credentials(None, None, None, None, None, Some("notakey".into()))
            .is_err());
        assert!(!a.has_live_credentials());
        assert!(a.wallet_address_hex().is_none());
        // Mismatched wallet clears a previously valid signer too.
        a.update_credentials(
            None,
            None,
            None,
            None,
            None,
            Some("0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d".into()),
        )
        .unwrap();
        assert!(a.has_live_credentials());
        assert!(a
            .update_credentials(
                None,
                None,
                None,
                None,
                Some("0x000000000000000000000000000000000000dead".into()),
                Some("0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d".into())
            )
            .is_err());
        assert!(!a.has_live_credentials());
        // mismatched wallet address → refuses everything
        let b = adapter("uniswap_v2");
        assert!(b
            .update_credentials(
                None,
                None,
                None,
                None,
                Some("0x000000000000000000000000000000000000dead".into()),
                Some("0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d".into())
            )
            .is_err());
        assert!(!b.has_live_credentials());
        // garbage key rejected
        let c = adapter("sushiswap");
        assert!(c
            .update_credentials(None, None, None, None, None, Some("notakey".into()))
            .is_err());
        // Market-data-only venue: an account address alone enables balance
        // reads but never reports live credentials (no signer is loaded).
        let m = adapter("aerodrome");
        m.update_credentials(
            None,
            None,
            None,
            None,
            Some("0x70997970C51812dc3A010C7d01b50e0d17dc79C8".into()),
            None,
        )
        .unwrap();
        assert!(!m.has_live_credentials());
        assert_eq!(
            m.wallet_address_hex().unwrap().to_lowercase(),
            "0x70997970c51812dc3a010c7d01b50e0d17dc79c8"
        );
    }

    #[test]
    fn transfer_sum_decode() {
        // Synthetic receipt: signer receives 2500 USDC, sends 1 WETH.
        let signer = Address::from_slice(&[0x11u8; 20]);
        let usdc = parse_addr("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48").unwrap();
        let weth = parse_addr("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2").unwrap();
        let t0 = B256::from_str(TRANSFER_TOPIC).unwrap();
        let mk = |token: Address, from: Address, to: Address, amt: u128| {
            let mut t1 = [0u8; 32];
            t1[12..].copy_from_slice(from.as_slice());
            let mut t2 = [0u8; 32];
            t2[12..].copy_from_slice(to.as_slice());
            let data = U256::from(amt).to_be_bytes::<32>();
            alloy::rpc::types::Log {
                inner: alloy::primitives::Log::new(
                    token,
                    vec![t0, B256::from(t1), B256::from(t2)],
                    alloy::primitives::Bytes::from(data.to_vec()),
                )
                .unwrap(),
                ..Default::default()
            }
        };
        let logs = vec![
            mk(
                weth,
                signer,
                Address::from_slice(&[0x22u8; 20]),
                1_000_000_000_000_000_000,
            ),
            mk(
                usdc,
                Address::from_slice(&[0x22u8; 20]),
                signer,
                2_500_000_000,
            ),
        ];
        let got = EvmAmmAdapter::transfer_sum(&logs, usdc, None, Some(signer));
        assert_eq!(got, U256::from(2_500_000_000u64));
        let spent = EvmAmmAdapter::transfer_sum(&logs, weth, Some(signer), None);
        assert_eq!(spent, U256::from(1_000_000_000_000_000_000u128));
        // Missing output → fail closed
        let a = adapter("uniswap");
        let (_, base, quote) = a.resolve_symbol("WETH-USDC").unwrap();
        let err = EvmAmmAdapter::decode_swap_fill(&logs[..1], signer, base, quote);
        assert!(err.is_err());
        // Output-only (input spend not proven) → fail closed
        let err = EvmAmmAdapter::decode_swap_fill(&logs[1..], signer, base, quote);
        assert!(err.is_err(), "received-without-spend must not count");
        // Full decode
        let (sp, rcv) = EvmAmmAdapter::decode_swap_fill(&logs, signer, base, quote).unwrap();
        assert_eq!(sp, U256::from(1_000_000_000_000_000_000u128));
        assert_eq!(rcv, U256::from(2_500_000_000u64));
    }

    #[test]
    fn market_metadata_shape() {
        // Offline shape check — fetch_markets hits RPC, so construct directly.
        let a = adapter("quickswap");
        let (pair, base, _q) = a.resolve_symbol("WMATIC-USDCE").unwrap();
        let fee = Decimal::from(a.fee_bps_for(pair));
        let m = MarketMetadata {
            symbol: Symbol::new(pair.base, pair.quote, MarketType::Spot),
            venue: a.venue_id().clone(),
            venue_symbol: "WMATIC-USDCE".into(),
            price_decimals: 8,
            size_decimals: base.decimals as u32,
            min_order_size: Decimal::ZERO,
            min_notional: Decimal::ZERO,
            maker_fee_bps: fee,
            taker_fee_bps: fee,
            contract_size: Decimal::ONE,
            is_active: true,
        };
        assert_eq!(m.symbol.market_type, MarketType::Spot);
        assert_eq!(m.taker_fee_bps, dec!(30));
        assert_eq!(a.market_type_label(), "Spot AMM");
    }

    #[test]
    fn capability_flags() {
        // Standard UniV2/UniV3-router venues support live execution.
        for id in LIVE_EVM_AMM_VENUE_IDS {
            let a = adapter(id);
            assert!(a.supports_live_execution(), "{id}");
            assert!(a.live_order_submission_implemented(), "{id}");
            assert!(a.order_status_polling_implemented(), "{id}");
            // Submission transport is available for explicit canaries, but
            // automated strategy entry stays refused pending gas-aware
            // profitability + credentialed evidence.
            assert!(!a.live_strategy_entry_implemented(), "{id}");
            assert!(a.is_admitted(), "{id}");
        }
        // Camelot (custom referrer ABI) and the Solidly family (no
        // exact-out buy) are admitted for market data but refuse execution.
        for id in ["camelot", "aerodrome", "velodrome", "ramses", "pharaoh"] {
            let m = adapter(id);
            assert!(m.is_admitted(), "{id}");
            assert!(!m.supports_live_execution(), "{id}");
            assert!(!m.live_order_submission_implemented(), "{id}");
            assert!(!m.order_status_polling_implemented(), "{id}");
        }
        // Non-admitted spec fails closed even if instantiated directly.
        let bh = EvmAmmAdapter::for_venue("blackhole", None).unwrap();
        assert!(!bh.supports_live_execution());
        assert!(!bh.is_admitted());
    }

    #[test]
    fn submitted_response_is_new_and_durable() {
        // A broadcast swap returns the tx hash as a New order immediately —
        // no fill fields may be claimed before receipt reconciliation.
        let a = adapter("uniswap");
        let order =
            OrderRequest::new_market(VenueId::new("uniswap"), "WETH-USDC", Side::Sell, dec!(0.5));
        let resp = a.submitted_order_response(&order, "0xabc123");
        assert_eq!(resp.status, OrderStatus::New);
        assert_eq!(resp.order_id, "0xabc123");
        assert_eq!(resp.original_qty, dec!(0.5));
        assert_eq!(resp.filled_qty, Decimal::ZERO);
        assert_eq!(resp.remaining_qty, dec!(0.5));
        assert!(resp.avg_fill_price.is_none());
        assert_eq!(resp.fee, Decimal::ZERO);
        assert_eq!(resp.fee_currency, "ETH");
        assert!(resp
            .raw_response
            .as_deref()
            .unwrap_or_default()
            .contains("0xabc123"));
        assert_eq!(resp.client_order_id, order.client_order_id);
    }

    /// Live-network smoke test — hits real public RPCs; run explicitly:
    /// `cargo test -p arbitrageur-exchanges evm_amm -- --ignored`
    #[tokio::test]
    #[ignore]
    async fn live_market_smoke() {
        for (id, sym) in [
            ("uniswap", "WETH-USDC"),
            ("uniswap_v2", "WETH-USDC"),
            ("pancakeswap", "WBNB-USDT"),
            ("pancakeswap_v3", "WBNB-USDT"),
            ("sushiswap", "WETH-USDC"),
            ("quickswap", "WMATIC-USDCE"),
            ("camelot", "WETH-USDC"),
            ("aerodrome", "WETH-USDC"),
            ("velodrome", "WETH-USDC"),
            ("ramses", "WETH-USDC"),
            ("pharaoh", "WAVAX-USDC"),
        ] {
            let a = adapter(id);
            a.ping().await.unwrap_or_else(|e| panic!("{id} ping: {e}"));
            let markets = a
                .fetch_markets()
                .await
                .unwrap_or_else(|e| panic!("{id} markets: {e}"));
            assert!(!markets.is_empty(), "{id} produced no markets");
            let tick = a
                .fetch_ticker(sym)
                .await
                .unwrap_or_else(|e| panic!("{id} ticker {sym}: {e}"));
            assert!(tick.bid > Decimal::ZERO, "{id} zero bid");
            assert!(tick.ask >= tick.bid, "{id} ask < bid");
            println!(
                "{id} {sym}: bid={} ask={} markets={}",
                tick.bid,
                tick.ask,
                markets.len()
            );
        }
        // Blackhole is intentionally not admitted.
        assert!(EvmAmmAdapter::for_venue("blackhole", None).is_some());
    }

    #[test]
    fn base_url_injection() {
        let a =
            EvmAmmAdapter::for_venue("velodrome", Some("http://localhost:8545".into())).unwrap();
        assert_eq!(a.rpc_url.read().as_str(), "http://localhost:8545");
        assert_eq!(a.default_rpc_url.as_str(), "http://localhost:8545");
        // garbage rejected → Err and current value kept
        assert!(a.set_base_url(Some("ftp://nope".into())).is_err());
        assert_eq!(a.rpc_url.read().as_str(), "http://localhost:8545");
        // blank restores the constructor's endpoint, not the spec default
        a.set_base_url(Some("  ".into())).unwrap();
        assert_eq!(a.rpc_url.read().as_str(), "http://localhost:8545");
        a.set_base_url(Some("https://custom.example/rpc/".into()))
            .unwrap();
        assert_eq!(a.rpc_url.read().as_str(), "https://custom.example/rpc");
        a.set_base_url(None).unwrap();
        assert_eq!(a.rpc_url.read().as_str(), "http://localhost:8545");
        // An actual URL change clears the chain-verification cache.
        *a.verified_rpc_url.write() = Some(Zeroizing::new("http://localhost:8545".to_string()));
        a.set_base_url(Some("http://localhost:9545".into()))
            .unwrap();
        assert!(a.verified_rpc_url.read().is_none());
        // Setting the same URL keeps the cache.
        *a.verified_rpc_url.write() = Some(Zeroizing::new("http://localhost:9545".to_string()));
        a.set_base_url(Some("http://localhost:9545".into()))
            .unwrap();
        assert_eq!(
            a.verified_rpc_url.read().as_ref().map(|v| v.as_str()),
            Some("http://localhost:9545")
        );
    }
}
