import React, { useCallback, useEffect, useState } from "react";
import {
  AlertCircle,
  AlertTriangle,
  ArrowLeftRight,
  Check,
  Eye,
  EyeOff,
  KeyRound,
  Plus,
  RefreshCw,
  ShieldAlert,
  ShieldCheck,
  Terminal,
  Copy,
  Trash2,
  Wallet,
} from "lucide-react";
import { api } from "../services/api";
import type {
  AccountCredentialInput,
  AccountDto,
  AgentCheckDto,
  ApproveTxDto,
  SignApproveDto,
  EngineSettingsDto,
  EvmKeyCustodyDto,
  StrategyAccountRouteDto,
  VenueBalanceInfo,
  VenueLaneStatusDto,
} from "../types";
import { STRATEGY_DISPLAY_NAMES, venueHasTestnet } from "../types/tickers";
import { ToggleSwitch } from "./ToggleSwitch";
import { SaveButton, useSaveButton } from "./SaveButton";
import { VenueLogo } from "./VenueLogo";

/* ------------------------------------------------------------------ */
/* Per-venue credential field specifications                            */
/* ------------------------------------------------------------------ */

type CredFieldKey = keyof AccountCredentialInput;

interface CredFieldSpec {
  key: CredFieldKey;
  label: string;
  secret?: boolean;
  numeric?: boolean;
  placeholder?: string;
  optional?: boolean;
  hint?: string;
  /** secp256k1 EVM signer key — show the derived "signs as 0x…" address. */
  evmSigner?: boolean;
  /** Fixed-value dropdown instead of a free-text input (e.g. region domains). */
  options?: { value: string; label: string }[];
}

export const ACCOUNT_CREDENTIAL_FIELDS: Record<string, CredFieldSpec[]> = {
  // Arcus: Ed25519 keypair generated + registered via the exchange (docs.arcus.xyz)
  arcus: [
    { key: "apiKey", label: "API Public Key (Ed25519)", placeholder: "arc_..." },
    { key: "privateKey", label: "API Private Key (Ed25519)", secret: true },
  ],
  // Hyperliquid: account/vault address + API wallet (agent) private key
  hyperliquid: [
    { key: "walletAddress", label: "Trading EOA Wallet Address", placeholder: "0x..." },
    { key: "privateKey", label: "API Wallet / Agent Private Key", secret: true, placeholder: "0x...", evmSigner: true },
    { key: "accountId", label: "Vault Address", optional: true, placeholder: "0x..." },
  ],
  kalshi: [
    { key: "apiKey", label: "API Key ID", placeholder: "e.g. 550e8400-e29b-..." },
    { key: "apiSecret", label: "RSA Private Key", secret: true, placeholder: "-----BEGIN RSA PRIVATE KEY-----" },
  ],
  kraken_spot: [
    { key: "apiKey", label: "Spot API Key", secret: true },
    { key: "apiSecret", label: "Spot Private/Secret Key", secret: true },
  ],
  kraken_futures: [
    { key: "apiKey", label: "Futures API Key", secret: true },
    { key: "apiSecret", label: "Futures Secret Key", secret: true },
  ],
  // Lighter: api_private_keys{index} + account_index — each API key has its own index (2-254; 0-1 reserved for web/mobile)
  lighter: [
    { key: "accountIndex", label: "Account Index", numeric: true, placeholder: "Exchange-assigned account ID, e.g. 746027" },
    { key: "apiKeyIndex", label: "API Key Index", numeric: true, placeholder: "2-254" },
    { key: "privateKey", label: "API Private Key", secret: true },
  ],
  // Polymarket: L1 Polygon key signs orders; L2 key/secret/passphrase authenticate the CLOB API
  polymarket: [
    { key: "walletAddress", label: "Polygon Wallet Address", placeholder: "0x..." },
    { key: "privateKey", label: "Polygon Private Key (L1 Signing)", secret: true, placeholder: "0x...", evmSigner: true },
    { key: "apiKey", label: "API Key", secret: true },
    { key: "apiSecret", label: "API Secret", secret: true },
    { key: "passphrase", label: "Passphrase", secret: true },
  ],
  // PopDEX (Morph Tachyon EVM, native API): main account address + delegated
  // agent key. One-time approveAgent runs externally per the official guide
  // (https://popdex.xyz/docs/product-docs/onboarding/how-to-trade-via-api,
  // Step 4 — no UI exists); the bot only ever holds the agent key and the
  // main private key must never be pasted here.
  popdex: [
    { key: "walletAddress", label: "Main Account Address", placeholder: "0x...", hint: "Your PopDEX main account holding funds (public 0x...). Orders trade for this account." },
    { key: "privateKey", label: "Agent Private Key", secret: true, placeholder: "0x...", hint: "ONLY the delegated agent key approved via approveAgent. Signs orders with timestamp nonces; cannot withdraw. Never paste the main private key.", evmSigner: true },
  ],
  // Robinhood Chain is a Lighter deployment (apidocs.rh.lighter.xyz): same
  // credential scheme — account_index + api_key_index + API private key.
  robinhood: [
    { key: "accountIndex", label: "Account Index", numeric: true, placeholder: "Exchange-assigned account ID" },
    { key: "apiKeyIndex", label: "API Key Index", numeric: true, placeholder: "2-254" },
    { key: "privateKey", label: "API Private Key", secret: true },
  ],
  vanta: [
    { key: "accountId", label: "Account ID", placeholder: "Orderly account id" },
    { key: "apiKey", label: "Orderly Key", secret: true, placeholder: "ed25519:..." },
    { key: "apiSecret", label: "Orderly Secret", secret: true },
    { key: "subaccountIndex", label: "Subaccount Index", numeric: true, optional: true, placeholder: "0 = main account" },
  ],
  // Decibel (Aptos): Geomi client key for REST API, node key for optional Aptos Fullnode RPC, API wallet key signs txs
  decibel: [
    {
      key: "walletAddress",
      label: "Login Wallet Address",
      placeholder: "0x...",
      hint: "The wallet you connect at app.decibel.trade — Aptos account, or EVM/Sui/Solana wallet (its Aptos account is derived automatically).",
    },
    {
      key: "privateKey",
      label: "API Wallet Private Key",
      secret: true,
      placeholder: "0x...",
      hint: "Private key of the API wallet delegated on Decibel.",
    },
    {
      key: "apiKey",
      label: "Client API Key (Decibel REST API)",
      secret: true,
      placeholder: "Geomi Client API Key",
      hint: "Bearer API key from geomi.dev for Decibel REST queries.",
    },
    {
      key: "passphrase",
      label: "Node API Key (Aptos Fullnode RPC)",
      secret: true,
      optional: true,
      placeholder: "Optional Fullnode RPC Key",
      hint: "Optional. Only needed if using an authenticated private Aptos RPC node.",
    },
  ],
  // Interactive Brokers: API connects via the TWS/IB Gateway session — account ID is the only stored credential
  ibkr: [
    { key: "accountId", label: "Account ID", placeholder: "e.g. U1234567 / DU1234567" },
  ],
  // IG Trading: API key header + identifier/password session login
  ig: [
    { key: "apiKey", label: "API Key", secret: true },
    { key: "accountId", label: "Username / Login ID" },
    { key: "passphrase", label: "Password", secret: true },
  ],
  binance: [
    { key: "apiKey", label: "API Key", secret: true },
    { key: "apiSecret", label: "API Secret", secret: true },
  ],
  // OKX: keys live in per-region namespaces — a key created on app.okx.com
  // does not exist on www.okx.com ("API key doesn't exist", code 50119).
  // The domain must match the site where the key was issued.
  okx: [
    {
      key: "baseUrl",
      label: "Region / API Domain",
      optional: true,
      options: [
        { value: "", label: "Global — www.okx.com (default)" },
        { value: "https://app.okx.com", label: "US / AU / regional — app.okx.com" },
        { value: "https://us.okx.com", label: "US — us.okx.com" },
        { value: "https://eea.okx.com", label: "EEA — eea.okx.com" },
        { value: "https://my.okx.com", label: "EEA (legacy) — my.okx.com" },
        { value: "https://tr.okx.com", label: "Türkiye — tr.okx.com" },
      ],
      hint: "Must match the domain in your address bar where the API key was created. A mismatch returns 'API key doesn't exist' (50119) even for valid keys.",
    },
    { key: "apiKey", label: "API Key", secret: true },
    { key: "apiSecret", label: "API Secret", secret: true },
    { key: "passphrase", label: "Passphrase", secret: true },
  ],
  // Aster: V3 = user wallet + API wallet (agent) signer; V1 legacy key/secret still works for existing keys
  aster: [
    { key: "walletAddress", label: "Main Account Wallet (User)", placeholder: "0x..." },
    { key: "privateKey", label: "API Wallet / Agent Private Key", secret: true, placeholder: "0x...", evmSigner: true },
    { key: "apiKey", label: "V1 API Key", secret: true, optional: true },
    { key: "apiSecret", label: "V1 API Secret", secret: true, optional: true },
  ],
  // Coinbase Advanced Trade (CDP): API key name + EC private key (PEM)
  coinbase: [
    { key: "apiKey", label: "API Key Name", placeholder: "organizations/{org}/apiKeys/{keyId}" },
    { key: "apiSecret", label: "Private Key (EC PEM)", secret: true, placeholder: "-----BEGIN EC PRIVATE KEY-----" },
  ],
  /* ---------------- Long-tail perp venues (DexPerpAdapter) ---------------- */
  // LIVE-TRADING VENUES — credentials below are what each adapter's
  // has_live_credentials() requires; nothing else gates order paths.
  // Aevo: REST key pair + EIP-712 order signer key.
  aevo: [
    { key: "apiKey", label: "API Key", secret: true },
    { key: "apiSecret", label: "API Secret", secret: true },
    { key: "privateKey", label: "Signing Private Key", secret: true, placeholder: "0x...", evmSigner: true, hint: "EVM key that signs EIP-712 orders; must match the API key's account." },
    { key: "walletAddress", label: "Account Address", optional: true, placeholder: "0x..." },
  ],
  // Derive: session key auth — wallet address + EVM signer.
  derive: [
    { key: "walletAddress", label: "Wallet Address", placeholder: "0x..." },
    { key: "privateKey", label: "Session/Signer Private Key", secret: true, placeholder: "0x...", evmSigner: true },
    { key: "accountId", label: "Subaccount ID", optional: true, numeric: true, placeholder: "Optional Derive subaccount" },
  ],
  // GRVT: API key → gravity session cookie; EVM key signs EIP-712 orders.
  grvt: [
    { key: "apiKey", label: "API Key", secret: true },
    { key: "privateKey", label: "Signing Private Key", secret: true, placeholder: "0x...", evmSigner: true },
    { key: "accountId", label: "Sub-Account ID", optional: true, placeholder: "GRVT sub-account id" },
  ],
  // Ondo Perps: SIWE wallet login → JWT; wallet key signs orders.
  ondoperps: [
    { key: "walletAddress", label: "Wallet Address", placeholder: "0x..." },
    { key: "privateKey", label: "Wallet Private Key", secret: true, placeholder: "0x...", evmSigner: true },
  ],
  // Pacifica: Solana-native — base58 account + Ed25519 keypair (bs58).
  pacifica: [
    { key: "walletAddress", label: "Account Address (Solana)", placeholder: "base58 pubkey" },
    { key: "privateKey", label: "Ed25519 Private Key", secret: true, placeholder: "base58 (64B keypair or 32B seed)", hint: "Solana-format base58 key — not an EVM 0x key." },
  ],
  // StandX: wallet sign-in → JWT + ephemeral Ed25519 request key.
  standx: [
    { key: "walletAddress", label: "Wallet Address", placeholder: "0x... or base58" },
    { key: "privateKey", label: "Ed25519 Private Key", secret: true, placeholder: "base58 keypair/seed" },
  ],
  // Vest Markets (vest.exchange → vestmarkets.com rebrand): delegate-signer
  // model — the funded account is the "primary", orders are signed by a
  // registered signing key, and every private call carries X-API-KEY +
  // xrestservermm: restserver{accGroup}. Two ways to configure:
  //  · Primary key stored → the adapter self-registers it as a 7-day
  //    delegate signer via POST /v2/register (apiKey/accGroup auto-fetched).
  //  · Delegate key stored → paste the apiKey + accGroup that an external
  //    /register run returned for that signer.
  vest: [
    { key: "walletAddress", label: "Primary Account Address", placeholder: "0x...", hint: "The funded Vest account (holds USDC collateral on zkSync Era)." },
    { key: "privateKey", label: "Signing Key (delegate or primary)", secret: true, placeholder: "0x...", evmSigner: true, hint: "Delegate signer registered via POST /register — or the primary key itself, which self-registers a ~7-day signer on first use." },
    { key: "apiKey", label: "API Key", secret: true, optional: true, placeholder: "From POST /register", hint: "Required for delegate signers; auto-fetched when the primary key is stored." },
    { key: "accountId", label: "Account Group", optional: true, numeric: true, placeholder: "0", hint: "accGroup from /register — routes requests to restserver{group}." },
  ],
  // MARKET-DATA-ONLY VENUES — no credentials needed; an enabled account
  // admits the venue for markets/books/funding, and order paths stay
  // fail-closed regardless of what is stored here.
  alphax: [],
  antarctic: [],
  apex: [],
  astros: [],
  bulk: [],
  // dYdX Chain (Cosmos appchain): orders are signed `MsgPlaceOrder`
  // transactions broadcast over validator gRPC. The credential is the
  // account's 24-word BIP-39 mnemonic — the venue derives
  // m/44'/118'/0'/0/0, so a raw hex private key cannot work. accountId is
  // the dYdX subaccount number (default 0); walletAddress is optional and,
  // when set, must equal the mnemonic-derived dydx1… address.
  dydx: [
    { key: "privateKey", label: "Account Mnemonic", secret: true, placeholder: "24-word BIP-39 phrase", hint: "The dYdX account's recovery phrase — Cosmos key derivation needs the full mnemonic." },
    { key: "accountId", label: "Subaccount Number", optional: true, numeric: true, placeholder: "0" },
    { key: "walletAddress", label: "Account Address", optional: true, placeholder: "dydx1…", hint: "Optional — verified against the derived account when set." },
  ],
  // edgeX Perp V2: HMAC api-key trio (key/passphrase/secret from the
  // venue's API management) + the numeric accountId + the EIP-712
  // "trading" private key that signs every order. The trading key is a
  // separate credential from the wallet — it signs as its own derived
  // address, which the venue binds to the account at API-key creation.
  edgex: [
    { key: "apiKey", label: "API Key", secret: true, placeholder: "edgeX API management" },
    { key: "passphrase", label: "API Passphrase", secret: true, placeholder: "edgeX API management" },
    { key: "apiSecret", label: "API Secret", secret: true, placeholder: "edgeX API management" },
    { key: "accountId", label: "Account ID", numeric: true, placeholder: "e.g. 543429922991899150" },
    { key: "privateKey", label: "Trading Private Key", secret: true, placeholder: "0x…", evmSigner: true, hint: "EIP-712 order signer issued with the API credentials — not necessarily the deposit wallet." },
  ],
  // EVEDEX (Arbitrum One): SIWE wallet sign-in → JWT session; every order is
  // an EIP-712 signature from the same wallet. No API keys — the wallet both
  // authenticates and signs.
  evedex: [
    { key: "walletAddress", label: "Wallet Address", placeholder: "0x… funded EVEDEX account" },
    { key: "privateKey", label: "Wallet Private Key", secret: true, placeholder: "0x...", evmSigner: true, hint: "Signs the SIWE login and each EIP-712 order on Arbitrum One; must match the wallet address." },
  ],
  // Extended (StarkEx engine on Starknet): X-Api-Key auth + SNIP-12
  // Poseidon order signatures from the account's Stark key pair. The
  // Stark public key and private key are issued in the Extended UI under
  // API management; the vault id is the account's L2 position id.
  extended: [
    { key: "apiKey", label: "API Key", secret: true, placeholder: "Extended API key" },
    { key: "accountId", label: "Vault / Position ID", numeric: true, placeholder: "e.g. 123456" },
    { key: "walletAddress", label: "Stark Public Key", placeholder: "0x…" },
    { key: "privateKey", label: "Stark Private Key", secret: true, placeholder: "0x…", hint: "Stark-curve key that signs orders — must match the Stark public key above." },
  ],
  // Pendle Boros (Arbitrum): the agent wallet signs every order via
  // EIP-712 `PendleSignTx`; the root account holds funds and approved the
  // agent once in the Boros app (the agent itself can never withdraw).
  // walletAddress is the ROOT account, privateKey the approved AGENT key —
  // they are different keys by design. accountId is the Boros sub-account
  // (0–255). Needs a gas balance in the account's Boros treasury.
  boros: [
    { key: "walletAddress", label: "Root Account Address", placeholder: "0x… funded Boros account" },
    { key: "privateKey", label: "Agent Private Key", secret: true, placeholder: "0x…", evmSigner: true, hint: "The agent key approved for this root in the Boros app — signs orders; cannot withdraw funds." },
    { key: "accountId", label: "Sub-Account ID", optional: true, numeric: true, placeholder: "0" },
  ],
  gatedex: [],
  gmx: [],
  // Helix / Injective chain: the account IS an EVM key — keccak address
  // becomes the inj1… bech32 account and the bytes32 subaccount id
  // (address || zeros || nonce). No API keys; orders are Cosmos txs
  // signed keccak/secp256k1 and broadcast over the Injective LCD.
  helix: [
    { key: "privateKey", label: "Account Private Key", secret: true, placeholder: "0x… 32-byte secp256k1", evmSigner: true, hint: "The Injective account's own EVM key — signs exchange Cosmos transactions directly." },
    { key: "walletAddress", label: "Account Address", optional: true, placeholder: "0x…", hint: "Optional — verified against the derived address when set." },
    { key: "accountId", label: "Subaccount Nonce", optional: true, numeric: true, placeholder: "0" },
  ],
  iotrader: [],
  // Nado (Ink L2): EIP-712 wallet auth — no API keys. walletAddress is the
  // funded main account; privateKey may be the account key itself or a
  // linked signer already authorized for the subaccount (venue rejects an
  // unauthorized signer at execute time). accountId optionally names a
  // non-"default" subaccount (≤12 ASCII chars).
  nado: [
    { key: "walletAddress", label: "Account Address", placeholder: "0x… funded Nado account" },
    { key: "privateKey", label: "Signing Key (main or linked signer)", secret: true, placeholder: "0x… 32-byte secp256k1" },
    { key: "accountId", label: "Subaccount Name", optional: true, placeholder: "default" },
  ],
  niza: [],
  ostium: [],
  // Paradex (Starknet L2): the EVM wallet derives the venue's Stark L2
  // signing key via the "STARK Key" EIP-712 derivation — the same wallet
  // authenticates (JWT) and signs orders. The account must already be
  // onboarded at paradex.trade for this wallet.
  paradex: [
    { key: "walletAddress", label: "Wallet Address", placeholder: "0x… Paradex-onboarded account" },
    { key: "privateKey", label: "Wallet Private Key", secret: true, placeholder: "0x...", evmSigner: true, hint: "Derives the Stark L2 key and signs orders; must match the wallet address." },
  ],
  // QFEX: account-scoped API key pair (Developer Settings → Generate).
  // Order entry rides an authenticated trade WebSocket; REST handles
  // account reads. accountId selects a subaccount (UUID) — omit for
  // the primary account. Key needs Execute orders + View permissions.
  qfex: [
    { key: "apiKey", label: "API Public Key", secret: true, placeholder: "qfex_pub_..." },
    { key: "apiSecret", label: "API Secret Key", secret: true, placeholder: "qfex_secret_..." },
    { key: "accountId", label: "Subaccount ID", optional: true, placeholder: "UUID — omit for primary account" },
  ],
  sodex: [],
  /* ------------- EVM spot AMMs (EvmAmmAdapter — local-wallet swaps) ------- */
  // No per-venue schemas: all eleven share one signer — see the EVM_AMM_VENUES
  // table and EvmWalletSection below. The sidebar consolidates them under
  // the pinned "EVM Wallet" label; per-venue RPC overrides live in the
  // coverage gadget's RPC column.
};

/**
 * Venues driven by the shared EVM wallet (EvmAmmAdapter specs). One key
 * signs swaps onchain on every chain, so per-venue key entry would only
 * invite drift. `execution` mirrors the backend LIVE_EVM_AMM_VENUE_IDS set:
 * swap venues receive the private key; market-data venues receive only the
 * public address — the key never reaches a venue that cannot trade.
 * `defaultRpc` is the chain's public endpoint shown when no per-venue
 * override is stored; the backend shares a chain override across same-chain
 * venues lacking their own.
 */
export const EVM_AMM_VENUES: {
  id: string;
  chain: string;
  execution: boolean;
  defaultRpc: string;
}[] = [
  { id: "uniswap", chain: "Ethereum", execution: true, defaultRpc: "https://ethereum-rpc.publicnode.com" },
  { id: "uniswap_v2", chain: "Ethereum", execution: true, defaultRpc: "https://ethereum-rpc.publicnode.com" },
  { id: "sushiswap", chain: "Ethereum", execution: true, defaultRpc: "https://ethereum-rpc.publicnode.com" },
  { id: "pancakeswap", chain: "BNB Chain", execution: true, defaultRpc: "https://bsc-rpc.publicnode.com" },
  { id: "pancakeswap_v3", chain: "BNB Chain", execution: true, defaultRpc: "https://bsc-rpc.publicnode.com" },
  { id: "quickswap", chain: "Polygon", execution: true, defaultRpc: "https://polygon-bor-rpc.publicnode.com" },
  { id: "camelot", chain: "Arbitrum", execution: false, defaultRpc: "https://arbitrum-one-rpc.publicnode.com" },
  { id: "ramses", chain: "Arbitrum", execution: false, defaultRpc: "https://arbitrum-one-rpc.publicnode.com" },
  { id: "aerodrome", chain: "Base", execution: false, defaultRpc: "https://base-rpc.publicnode.com" },
  { id: "velodrome", chain: "Optimism", execution: false, defaultRpc: "https://optimism-rpc.publicnode.com" },
  { id: "pharaoh", chain: "Avalanche", execution: false, defaultRpc: "https://avalanche-c-chain-rpc.publicnode.com" },
];

export const EVM_AMM_VENUE_IDS = new Set(EVM_AMM_VENUES.map((v) => v.id));

/** Credential fields for the shared-wallet card — one key + an optional
 *  address cross-check, fanned out to every AMM venue's default account. */
export const EVM_WALLET_FIELDS: CredFieldSpec[] = [
  {
    key: "privateKey",
    label: "Wallet Private Key",
    secret: true,
    placeholder: "0x...",
    evmSigner: true,
    hint: "secp256k1 EOA key — derives the same address on every chain. Full custody: it can move ALL funds this wallet holds, so use a dedicated hot wallet, not your main one.",
  },
  {
    key: "walletAddress",
    label: "Wallet Address",
    optional: true,
    placeholder: "0x...",
    hint: "Optional cross-check — must equal the key's derived address or the save is refused.",
  },
];

export const ACCOUNT_VENUE_DISPLAY: Record<string, string> = {
  arcus: "Arcus",
  hyperliquid: "Hyperliquid",
  kalshi: "Kalshi",
  kraken_spot: "Kraken Spot",
  kraken_futures: "Kraken Futures",
  lighter: "Lighter",
  polymarket: "Polymarket",
  popdex: "PopDEX",
  robinhood: "Robinhood Lighter",
  vanta: "Vanta",
  decibel: "Decibel",
  ibkr: "Interactive Brokers",
  ig: "IG Trading",
  binance: "Binance",
  boros: "Pendle Boros",
  okx: "OKX",
  aster: "Aster",
  coinbase: "Coinbase",
  // Long-tail perp venues (DexPerpAdapter ids — must match the spec table).
  aevo: "Aevo",
  alphax: "AlphaX",
  antarctic: "Antarctic",
  apex: "ApeX Omni",
  astros: "Astros",
  bulk: "BULK",
  derive: "Derive",
  dydx: "dYdX Chain",
  edgex: "edgeX",
  evedex: "EVEDEX",
  extended: "Extended",
  gatedex: "Gate DEX",
  gmx: "GMX V2",
  grvt: "GRVT",
  helix: "Helix",
  iotrader: "IO Trader",
  nado: "Nado",
  niza: "Niza",
  ondoperps: "Ondo Perps",
  ostium: "Ostium",
  pacifica: "Pacifica",
  paradex: "Paradex",
  qfex: "QFEX",
  sodex: "SoDEX",
  standx: "StandX",
  vest: "Vest",
  // EVM spot AMMs (EvmAmmAdapter ids — must match the spec table).
  uniswap: "Uniswap V3",
  uniswap_v2: "Uniswap V2",
  pancakeswap: "PancakeSwap V2",
  pancakeswap_v3: "PancakeSwap V3",
  sushiswap: "SushiSwap",
  quickswap: "QuickSwap",
  camelot: "Camelot V2",
  aerodrome: "Aerodrome",
  velodrome: "Velodrome",
  ramses: "Ramses",
  pharaoh: "Pharaoh",
};

const venueDisplay = (v: string) => ACCOUNT_VENUE_DISPLAY[v] ?? v;

/** Maps a venue id to its engine-settings enable flag. */
const VENUE_SETTINGS_FLAG: Record<string, keyof EngineSettingsDto> = {
  arcus: "arcus_enabled",
  aster: "aster_enabled",
  binance: "binance_enabled",
  coinbase: "coinbase_enabled",
  decibel: "decibel_enabled",
  hyperliquid: "hyperliquid_enabled",
  ibkr: "ibkr_enabled",
  ig: "ig_enabled",
  kalshi: "kalshi_enabled",
  kraken: "kraken_enabled",
  kraken_futures: "kraken_futures_enabled",
  kraken_spot: "kraken_spot_enabled",
  lighter: "lighter_enabled",
  okx: "okx_enabled",
  polymarket: "polymarket_enabled",
  popdex: "popdex_enabled",
  robinhood: "robinhood_enabled",
  vanta: "vanta_enabled",
};

/**
 * Persist just the venue's enable flag without waiting for the full settings
 * form's Save. Reads the sanitized current settings and re-saves them with the
 * flag flipped — the sanitized payload carries no secrets, so nothing stored
 * in the vault is touched. Writes are serialized so rapid toggles across
 * different venues cannot lose each other's update.
 */
let flagWriteQueue: Promise<void> = Promise.resolve();
const persistVenueFlag = (venue: string, enabled: boolean) => {
  const key = VENUE_SETTINGS_FLAG[venue];
  if (!key) return;
  flagWriteQueue = flagWriteQueue.then(async () => {
    try {
      const current = await api.getEngineSettings();
      await api.saveEngineSettings({ ...current, [key]: enabled });
    } catch {
      /* best-effort — the form's Save remains the fallback */
    }
  });
};

const fieldIsSet = (a: AccountDto, key: CredFieldKey): boolean => {
  switch (key) {
    case "apiKey":
      return a.hasApiKey;
    case "apiSecret":
      return a.hasApiSecret;
    case "passphrase":
      return a.hasPassphrase;
    case "accountId":
      return a.hasAccountId;
    case "walletAddress":
      return a.hasWalletAddress;
    case "privateKey":
      return a.hasPrivateKey;
    case "accountIndex":
      return a.accountIndex != null;
    case "subaccountIndex":
      return a.subaccountIndex != null;
    case "apiKeyIndex":
      return a.apiKeyIndex != null;
    case "baseUrl":
      return a.hasBaseUrl;
    default:
      return false;
  }
};

const STORED_MASK = "••••••••••••";

/* ------------------------------------------------------------------ */
/* Credential field grid (shared by account card + add-account form)    */
/* ------------------------------------------------------------------ */

interface CredGridProps {
  fields: CredFieldSpec[];
  values: Record<string, string>;
  setValue: (key: string, value: string) => void;
  disabled: boolean;
  account?: AccountDto | null;
  showSecrets: Record<string, boolean>;
  toggleShow: (key: string) => void;
}

/** Derived-signer readout for EVM agent-key fields: shows which address
 *  the typed (or stored) key signs as, so a wrong/stale agent key is
 *  visible before a live order fails — e.g. HL's "API Wallet 0x… does
 *  not exist" rejection. Typed keys derive via a pure IPC call; the
 *  stored key derives vault-side and returns only the address. */
const SignerAddressHint: React.FC<{
  typedKey: string;
  accountId: string | null;
  stored: boolean;
  venue: string | null;
  isDefault: boolean;
  activeVersion: string;
}> = ({ typedKey, accountId, stored, venue, isDefault, activeVersion }) => {
  const [addr, setAddr] = useState<string | null>(null);
  const [invalid, setInvalid] = useState(false);
  const [active, setActive] = useState<string | null>(null);
  const [activeLoaded, setActiveLoaded] = useState(false);
  const [activeError, setActiveError] = useState(false);
  const trimmed = typedKey.trim();
  const looksComplete = /^(0x)?[0-9a-fA-F]{64}$/.test(trimmed);
  // get_active_signer_address is implemented for Hyperliquid and the
  // swap-capable EVM AMM adapters — other venues keep the typed/stored
  // derivation and never show an active line.
  const activeSupported =
    venue === "hyperliquid" ||
    (venue != null && EVM_AMM_VENUES.some((v) => v.id === venue && v.execution));

  useEffect(() => {
    setAddr(null);
    setInvalid(false);
    let dead = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    if (looksComplete) {
      timer = setTimeout(() => {
        api
          .deriveEvmSignerAddress(trimmed)
          .then((a) => { if (!dead) setAddr(a); })
          .catch(() => { if (!dead) setInvalid(true); });
      }, 300);
    } else if (!trimmed && stored && accountId) {
      api
        .getAccountSignerAddress(accountId)
        .then((a) => { if (!dead) setAddr(a); })
        .catch(() => {});
    }
    return () => {
      dead = true;
      if (timer) clearTimeout(timer);
    };
  }, [trimmed, looksComplete, stored, accountId]);

  // The signer the live adapter holds right now — independent of vault
  // lock state. Only the default account's credentials are provisioned;
  // re-query on save/reprovision and lock transitions via activeVersion.
  useEffect(() => {
    setActive(null);
    setActiveLoaded(false);
    setActiveError(false);
    let dead = false;
    if (activeSupported && venue && isDefault && api.isTauri) {
      api
        .getActiveSignerAddress(venue)
        .then((a) => {
          if (!dead) {
            setActive(a);
            setActiveLoaded(true);
          }
        })
        .catch(() => {
          if (!dead) {
            setActiveError(true);
            setActiveLoaded(true);
          }
        });
    }
    return () => {
      dead = true;
    };
  }, [venue, isDefault, activeVersion]);

  const typedAddr = trimmed && !invalid ? addr : null;
  const storedAddr = !trimmed && stored ? addr : null;
  const compareAddr = typedAddr ?? storedAddr;
  let activeLine: { text: string; cls: string } | null = null;
  if (activeLoaded) {
    if (activeError) {
      activeLine = {
        text: "unable to read live adapter signer",
        cls: "text-amber-700",
      };
    } else if (!active) {
      activeLine = {
        text: "live adapter has no signing key loaded",
        cls: "text-loss",
      };
    } else if (compareAddr && compareAddr.toLowerCase() === active.toLowerCase()) {
      activeLine = {
        text: `live adapter signs as ${active} (matches)`,
        cls: "text-profit",
      };
    } else if (typedAddr) {
      activeLine = {
        text: `live adapter still signs as ${active} — save this key before trading`,
        cls: "text-amber-700",
      };
    } else if (storedAddr) {
      activeLine = {
        text: `LIVE SIGNER MISMATCH: adapter signs as ${active} — save or re-unlock before trading`,
        cls: "text-loss",
      };
    } else {
      activeLine = {
        text: `live adapter currently signs as ${active}`,
        cls: "text-slate-500",
      };
    }
  }

  if (!addr && !invalid && !activeLine) return null;
  return (
    <>
      {(addr || invalid) && (
        <p
          className={`text-[10px] mt-1 leading-tight font-mono break-all ${
            invalid ? "text-amber-700" : "text-slate-500"
          }`}
        >
          {addr
            ? `${trimmed ? "typed key signs as" : "stored key signs as"} ${addr}`
            : "not a valid EVM private key"}
        </p>
      )}
      {activeLine && (
        <p
          className={`text-[10px] mt-1 leading-tight font-mono break-all ${activeLine.cls}`}
        >
          {activeLine.text}
        </p>
      )}
    </>
  );
};

const CredentialGrid: React.FC<CredGridProps> = ({
  fields,
  values,
  setValue,
  disabled,
  account,
  showSecrets,
  toggleShow,
}) => (
  <div className="grid grid-cols-1 md:grid-cols-2 gap-3 text-xs font-mono">
    {fields.map((f) => {
      const stored = account ? fieldIsSet(account, f.key) : false;
      const hasValue = (values[f.key] ?? "") !== "";
      const placeholder =
        !hasValue && stored && f.secret
          ? STORED_MASK
          : !hasValue && stored && !f.secret
          ? "(stored — leave blank to keep)"
          : f.placeholder ?? (f.optional ? "Optional" : "");
      const shown = showSecrets[f.key];
      return (
        <div key={f.key}>
          <label className="text-slate-600 block mb-1">
            {f.label}
            {f.optional ? " (Optional)" : ""}:
          </label>
          <div className="relative">
            {f.options ? (
              <select
                disabled={disabled}
                value={values[f.key] ?? ""}
                onChange={(e) => setValue(f.key, e.target.value)}
                className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono disabled:opacity-50"
              >
                {f.options.map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
                {/* A stored value outside the known set stays visible rather
                    than silently snapping to the first option. */}
                {f.options.every((o) => o.value !== (values[f.key] ?? "")) && (
                  <option value={values[f.key] ?? ""}>{values[f.key] ?? ""}</option>
                )}
              </select>
            ) : (
              <input
                type={f.secret && !shown ? "password" : f.numeric ? "number" : "text"}
                disabled={disabled}
                value={values[f.key] ?? ""}
                placeholder={placeholder}
                onChange={(e) => setValue(f.key, e.target.value)}
                autoComplete="off"
                className={`w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono disabled:opacity-50 ${f.secret ? "pr-9" : ""}`}
              />
            )}
            {f.secret && (
              <button
                type="button"
                onClick={() => toggleShow(f.key)}
                className="absolute right-2.5 top-1/2 -translate-y-1/2 text-muted hover:text-slate-700 transition cursor-pointer disabled:opacity-40"
                disabled={disabled}
                title={shown ? "Hide" : "Show"}
                aria-label={shown ? `Hide ${f.label}` : `Show ${f.label}`}
              >
                {shown ? <EyeOff className="w-4 h-4" /> : <Eye className="w-4 h-4" />}
              </button>
            )}
          </div>
          {f.hint && (
            <p className="text-[10px] text-muted mt-1 leading-tight">{f.hint}</p>
          )}
          {f.evmSigner && (
            <SignerAddressHint
              typedKey={values[f.key] ?? ""}
              accountId={account?.id ?? null}
              stored={stored}
              venue={account?.venue ?? null}
              isDefault={account?.isDefault ?? false}
              activeVersion={`${account?.updatedAt ?? ""}:${disabled ? "disabled" : "enabled"}`}
            />
          )}
        </div>
      );
    })}
  </div>
);

/* ------------------------------------------------------------------ */
/* Venue balance panel (metrics only)                                  */
/* ------------------------------------------------------------------ */

/** Venue margin/metrics block rendered inside each account card. `bal` is null when no balance data exists (no API credentials / never synced) — values render as N/A rather than fabricated zeros. */
export const VenueBalancePanel: React.FC<{
  bal: VenueBalanceInfo | null;
  lane?: VenueLaneStatusDto | null;
  accountTier?: string;
  isDefault?: boolean;
}> = ({ bal }) => {
  return (
    <div className="space-y-3">
      <div className="grid grid-cols-1 sm:grid-cols-3 gap-3 font-mono">
        {/* Metric 1: Equity */}
        <div className="p-3.5 bg-slate-50/70 border border-surface-border rounded-lg space-y-1">
          <div className="flex items-center justify-between text-muted">
            <span className="text-xs font-bold tracking-wider">Total Equity</span>
            <span className="text-xs">USD</span>
          </div>
          <div className="text-xl font-bold text-slate-900 tracking-tight">
            {bal
              ? `$${bal.equity_usd.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`
              : "N/A"}
          </div>
        </div>

        {/* Metric 2: Available Margin */}
        <div className="p-3.5 bg-slate-50/70 border border-surface-border rounded-lg space-y-1">
          <div className="flex items-center justify-between text-muted">
            <span className="text-xs font-bold tracking-wider">Available Margin</span>
            <span className="text-xs">USD</span>
          </div>
          <div className={`text-xl font-bold font-mono tracking-tight tabular-nums ${bal ? "text-profit" : "text-slate-900"}`}>
            {bal
              ? `$${bal.available_margin_usd.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`
              : "N/A"}
          </div>
        </div>

        {/* Metric 3: Margin Rate */}
        <div className="p-3.5 bg-slate-50/70 border border-surface-border rounded-lg space-y-1">
          <div className="flex items-center justify-between text-muted">
            <span className="text-xs font-bold tracking-wider">Margin Rate</span>
            <span className="text-xs">Utilization</span>
          </div>
          <div className="text-xl font-bold text-slate-900 flex items-baseline space-x-1.5 tracking-tight font-mono tabular-nums">
            {bal?.margin_rate_pct != null ? (
              <>
                <span>{bal.margin_rate_pct.toFixed(1)}%</span>
                <span className="text-xs font-normal text-muted font-sans">utilized</span>
              </>
            ) : (
              <span>N/A</span>
            )}
          </div>
          <div className="w-full bg-slate-200 h-1.5 rounded-full overflow-hidden mt-1.5">
            <div
              className={`h-full rounded-full transition-all duration-300 ${
                (bal?.margin_rate_pct ?? 0) > 80
                  ? "bg-loss"
                  : (bal?.margin_rate_pct ?? 0) > 50
                  ? "bg-warn"
                  : "bg-accent"
              }`}
              style={{ width: `${Math.min(100, bal?.margin_rate_pct ?? 0)}%` }}
            />
          </div>
        </div>
      </div>


      {bal?.status === "NO_CREDENTIALS" && (
        <div className="p-2.5 bg-slate-50 border border-slate-200 rounded-lg text-xs font-mono text-slate-600 flex items-start space-x-2">
          <span>{bal.recommended_action || "No live API credentials configured. Add credentials in the accounts section below and unlock the vault to view live balances."}</span>
        </div>
      )}

      {bal?.status === "LOCKED" && (
        <div className="p-2.5 bg-slate-50 border border-slate-200 rounded-lg text-xs font-mono text-slate-600 flex items-start space-x-2">
          <span>{bal.recommended_action || "Vault is locked — unlock to read live balances."}</span>
        </div>
      )}

      {bal?.status === "STALE" && bal?.recommended_action && (
        <div className="p-2.5 bg-slate-50 border border-slate-200 rounded-lg text-xs font-mono text-slate-500 flex items-start space-x-2">
          <span>{bal.recommended_action}</span>
        </div>
      )}

      {bal?.status === "ERROR" && (
        <div className="p-2.5 bg-loss-dim border border-loss-border rounded-lg text-xs font-mono text-loss flex items-start space-x-2">
          <AlertTriangle className="w-3.5 h-3.5 text-loss shrink-0 mt-0.5" />
          <span>{bal.recommended_action || "Failed to retrieve account balance from exchange."}</span>
        </div>
      )}

      {bal?.recommended_action && bal?.status !== "ERROR" && bal?.status !== "NO_CREDENTIALS" && bal?.status !== "EMPTY" && (
        <div className="p-2.5 bg-warn-dim border border-warn-border rounded-lg text-xs font-mono text-warn-strong flex items-start space-x-2">
          <AlertTriangle className="w-3.5 h-3.5 text-warn-strong shrink-0 mt-0.5" />
          <span>{bal.recommended_action}</span>
        </div>
      )}
    </div>
  );
};

/* ------------------------------------------------------------------ */
/* Account card                                                        */
/* ------------------------------------------------------------------ */

export interface AccountCardProps {
  account: AccountDto;
  /** 1-based position of this account within the venue, used for the title. */
  ordinal: number;
  locked: boolean;
  bal?: VenueBalanceInfo | null;
  lane?: VenueLaneStatusDto | null;
  /** Legacy flag preserved for caller backwards-compatibility */
  labelInHeader?: boolean;
  /** Legacy flag preserved for caller backwards-compatibility */
  controlsInHeader?: boolean;
  onChanged: () => void;
}

/** Parse the card's max-leverage text: "" = unlimited (null); otherwise an
 *  integer >= 1. No upper cap — effective leverage is min(strategy, account). */
function parseMaxLeverage(raw: string): number | null {
  const t = raw.trim();
  if (t === "") return null;
  const n = Number(t);
  if (!Number.isInteger(n) || n < 1) {
    throw new Error("Max leverage must be empty (unlimited) or an integer of at least 1");
  }
  return n;
}

/** Canonicalize leverage text so "3" and " 3 " compare equal in dirty checks. */
function normalizeLeverageText(raw: string): string {
  const t = raw.trim();
  if (t === "") return "";
  const n = Number(t);
  return Number.isFinite(n) ? String(n) : t;
}


/* ------------------------------------------------------------------ */
/* PopDEX agent-setup toolkit modal                                    */
/* ------------------------------------------------------------------ */

const diyCommand = (built: ApproveTxDto, agent: string, mainAddr: string): string =>
  `cast send ${built.to} "approveAgent(address,address,bytes32,uint64,uint64,bool)" ${agent} ${mainAddr} 0x0000000000000000000000000000000000000000000000000000000000000000 0 $(date +%s%3N) false --rpc-url ${built.rpcUrl} --private-key $MAIN_PRIVATE_KEY`;

/** Guided PopDEX agent setup: generate locally → approve with the main
 * key (signed once in this modal and never stored — or via a DIY shell
 * command for the cautious) → verify on-chain → save. */
const POPDEX_API_DOCS = "https://popdex.xyz/docs/product-docs/onboarding/how-to-trade-via-api";
const POPDEX_TESTNET_APP = "https://testnet-app.popdex.xyz";
const POPDEX_MAINNET_APP = "https://app.popdex.xyz";
const POPDEX_TEST_FAUCET = "https://crypto-chief.com/faucet/arbitrum-sepolia/";
const openDoc = (url: string, setError: (e: string | null) => void) => {
  api.openExternalUrl(url).catch((e) => setError(e instanceof Error ? e.message : String(e)));
};
const PopdexAgentToolkit: React.FC<{
  initialMain: string;
  onUseKey: (agentKey: string, mainAddr: string) => void;
  onClose: () => void;
}> = ({ initialMain, onUseKey, onClose }) => {
  const [network, setNetwork] = useState<"testnet" | "mainnet">("testnet");
  const [main, setMain] = useState(initialMain);
  const [agentAddr, setAgentAddr] = useState("");
  const [agentKey, setAgentKey] = useState("");
  const [check, setCheck] = useState<AgentCheckDto | null>(null);
  const [mainKey, setMainKey] = useState("");
  const [showMainKey, setShowMainKey] = useState(false);
  const [signed, setSigned] = useState<SignApproveDto | null>(null);
  const [diy, setDiy] = useState<ApproveTxDto | null>(null);
  const [copiedDiy, setCopiedDiy] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [keyUsed, setKeyUsed] = useState(false);

  const run = async (what: string, fn: () => Promise<void>) => {
    setBusy(what);
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(null);
    }
  };

  const stepCard = "p-3.5 bg-surface border border-surface-border rounded-lg space-y-2";
  const btnAccent = "btn-accent text-xs font-mono font-bold flex items-center gap-1.5 disabled:opacity-50";
  const btnGhost =
    "px-2.5 py-1.5 rounded-lg text-xs font-mono border border-surface-border text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 transition cursor-pointer disabled:opacity-50 flex items-center gap-1";

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/50 p-4"
      onClick={onClose}
      role="dialog"
      aria-label="PopDEX agent setup toolkit"
    >
      <div
        className="w-full max-w-xl max-h-[85vh] overflow-y-auto bg-background rounded-xl border border-surface-border shadow-2xl p-5 space-y-4"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between">
          <h3 className="text-sm font-bold font-mono text-slate-900 flex items-center gap-2">
            <KeyRound className="w-4 h-4 text-accent-strong" /> PopDEX Agent Setup Toolkit
          </h3>
          <button type="button" onClick={onClose} className="text-muted hover:text-slate-700 text-lg leading-none px-2" aria-label="Close">
            ×
          </button>
        </div>
        <p className="text-[11px] font-sans text-slate-500 leading-normal">
          Generate the agent key here, approve it once with the main key, verify
          on-chain, then save the account below. The main key signs a single
          transaction and is <b>never stored</b> — or run the command yourself
          instead (step 3, option B).{" "}
          <button type="button" onClick={() => openDoc(POPDEX_API_DOCS, setError)} className="text-slate-500 underline hover:text-slate-800 cursor-pointer">PopDEX API guide</button>
        </p>

        <div className="flex gap-2 items-center text-xs font-mono">
          <span className="text-slate-500">Network:</span>
          {(["mainnet", "testnet"] as const).map((n) => (
            <button
              key={n}
              type="button"
              onClick={() => { setNetwork(n); setCheck(null); setSigned(null); setDiy(null); }}
              aria-pressed={network === n}
              className={`px-2.5 py-1 rounded-lg font-bold border transition cursor-pointer ${
                network === n ? "bg-accent text-white border-accent" : "text-slate-500 border-surface-border hover:bg-slate-100/70"
              }`}
            >
              {n === "mainnet" ? "Mainnet" : "Testnet"}
            </button>
          ))}
          {network === "testnet" && (
            <span className="text-[10px] font-sans text-muted">free test USDT from the <button type="button" onClick={() => openDoc(POPDEX_TEST_FAUCET, setError)} className="text-slate-500 underline hover:text-slate-800 cursor-pointer">faucet</button></span>
          )}
        </div>

        <div className={stepCard}>
          <div className="text-xs font-mono font-bold text-slate-800">1 · Main account (holds funds, public)</div>
          <input
            value={main}
            onChange={(e) => { setMain(e.target.value); setCheck(null); }}
            placeholder="0x..."
            autoComplete="off"
            spellCheck={false}
            className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-xs font-mono text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
          />
          <p className="text-[10px] font-sans text-muted">
            Must already hold a deposit — the chain rejects every transaction (error -32000) until the first deposit.{" "}
            {network === "testnet" ? (
              <>Claim free test USDT from the <button type="button" onClick={() => openDoc(POPDEX_TEST_FAUCET, setError)} className="text-slate-500 underline hover:text-slate-800 cursor-pointer">faucet</button>, then deposit in the <button type="button" onClick={() => openDoc(POPDEX_TESTNET_APP, setError)} className="text-slate-500 underline hover:text-slate-800 cursor-pointer">testnet app</button>.</>
            ) : (
              <>Deposit USDT in the <button type="button" onClick={() => openDoc(POPDEX_MAINNET_APP, setError)} className="text-slate-500 underline hover:text-slate-800 cursor-pointer">PopDEX app</button> first, then approve here.</>
            )}
          </p>
        </div>

        <div className={stepCard}>
          <div className="text-xs font-mono font-bold text-slate-800">2 · Generate agent key (this machine only)</div>
          {!agentAddr ? (
            <button type="button" disabled={busy !== null} onClick={() => void run("gen", async () => {
              const g = await api.generatePopdexAgent();
              setAgentAddr(g.address);
              setAgentKey(g.privateKey);
              setKeyUsed(false);
              setCheck(null);
            })} className={btnAccent}>
              <KeyRound className="w-3.5 h-3.5" /> {busy === "gen" ? "Generating…" : "Generate fresh agent key"}
            </button>
          ) : (
            <div className="space-y-1.5">
              <div className="text-[11px] font-mono break-all"><span className="text-muted">Address: </span><span className="text-slate-900">{agentAddr}</span></div>
              <div className="text-[11px] font-mono break-all"><span className="text-muted">Private key: </span><span className="text-loss font-bold">{agentKey.slice(0, 10)}…{agentKey.slice(-6)} (shown once)</span></div>
              <div className="flex gap-2">
                <button type="button" onClick={() => { onUseKey(agentKey, main.trim()); setKeyUsed(true); }} className={btnAccent}>
                  <Check className="w-3.5 h-3.5" /> {keyUsed ? "Filled into account ✓" : "Use this key + main address in the account form"}
                </button>
                <button type="button" onClick={() => void run("gen", async () => {
                  const g = await api.generatePopdexAgent();
                  setAgentAddr(g.address);
                  setAgentKey(g.privateKey);
                  setKeyUsed(false);
                  setCheck(null);
                })} className={btnGhost}>Regenerate</button>
              </div>
              {!keyUsed && <p className="text-[10px] font-sans text-warn-strong">Not saved yet — press “Use this key”, then “Save Changes” on the card.</p>}
            </div>
          )}
        </div>

        <div className={stepCard}>
          <div className="text-xs font-mono font-bold text-slate-800">3 · Approve with the main key (one time)</div>
          <p className="text-[10px] font-sans text-slate-500 leading-normal">
            <b>Option A — sign here:</b> paste the <b>main</b> private key below and press Sign.
            It signs exactly one <span className="font-mono">approveAgent</span> transaction and is
            wiped from memory immediately — never saved, never sent anywhere else. Prefer not
            to paste it? Use <b>option B</b> below instead.
          </p>
          <div className="flex gap-2">
            <input
              type={showMainKey ? "text" : "password"}
              value={mainKey}
              onChange={(e) => setMainKey(e.target.value)}
              placeholder="Main private key 0x… (used once, never stored)"
              autoComplete="off"
              spellCheck={false}
              className="flex-1 px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-xs font-mono text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
            />
            <button type="button" onClick={() => setShowMainKey((s) => !s)} className={btnGhost} aria-label={showMainKey ? "Hide" : "Show"}>
              {showMainKey ? <EyeOff className="w-3.5 h-3.5" /> : <Eye className="w-3.5 h-3.5" />}
            </button>
          </div>
          <div className="flex flex-wrap gap-2 items-center">
            <button type="button" disabled={busy !== null || !main || !agentAddr || !mainKey} onClick={() => void (async () => {
              const key = mainKey;
              setMainKey("");
              await run("sign", async () => {
                setSigned(await api.signPopdexApproveWithMain(main.trim(), agentAddr, key, network));
                setCheck(null);
              });
            })()} className={btnAccent}>
              <KeyRound className="w-3.5 h-3.5" /> {busy === "sign" ? "Signing…" : "Sign approval"}
            </button>
            {signed && (
              <span className="text-[11px] font-mono break-all">
                <span className="text-profit font-bold">Submitted ✓ </span>
                <a href={signed.explorerUrl} target="_blank" rel="noreferrer" className="underline text-slate-600">{signed.txHash.slice(0, 12)}…</a>
              </span>
            )}
          </div>
          <div className="pt-1 space-y-1.5">
            <p className="text-[10px] font-sans text-slate-500 leading-normal">
              <b>Option B — run it yourself</b> (nothing secret touches this app): show the{" "}
              <span className="font-mono">cast</span> command and run it in your own terminal.{" "}
              <button type="button" onClick={() => openDoc(POPDEX_API_DOCS, setError)} className="text-slate-500 underline hover:text-slate-800 cursor-pointer">API guide, step 4</button>
            </p>
            <button type="button" disabled={busy !== null || !main || !agentAddr} onClick={() => void run("diy", async () => {
              const built = await api.buildPopdexApproveTx(main.trim(), agentAddr, network);
              setDiy(built);
              setCopiedDiy(false);
            })} className={btnGhost}>
              <Terminal className="w-3.5 h-3.5" /> {busy === "diy" ? "Building…" : diy ? "Rebuild command" : "Show DIY command"}
            </button>
            {diy && (
              <div className="space-y-1.5">
                <pre className="whitespace-pre-wrap break-all text-[10px] font-mono bg-slate-900 text-slate-100 rounded-lg p-2.5">{diyCommand(diy, agentAddr, main.trim())}</pre>
                <button type="button" onClick={() => void (async () => {
                  try {
                    await navigator.clipboard.writeText(diyCommand(diy, agentAddr, main.trim()));
                    setCopiedDiy(true);
                    setTimeout(() => setCopiedDiy(false), 1500);
                  } catch {
                    setError("Clipboard blocked — select the command manually.");
                  }
                })()} className={btnGhost}>
                  {copiedDiy ? <Check className="w-3 h-3" /> : <Copy className="w-3 h-3" />} {copiedDiy ? "Copied ✓" : "Copy command"}
                </button>
              </div>
            )}
          </div>
        </div>
        <div className={stepCard}>
          <div className="text-xs font-mono font-bold text-slate-800">4 · Verify on-chain, then save</div>
          <div className="flex gap-2 items-center">
            <button type="button" disabled={busy !== null || !main || !agentAddr} onClick={() => void run("check", async () => {
              setCheck(await api.checkPopdexAgent(main.trim(), agentAddr, network));
            })} className={btnAccent}>
              <RefreshCw className="w-3.5 h-3.5" /> {busy === "check" ? "Checking…" : "Verify approval"}
            </button>
            {check && (
              <span className={`text-xs font-mono font-bold ${check.approved ? "text-profit" : "text-loss"}`}>
                {check.approved ? `Approved ✓ (${check.agentCount} agent${check.agentCount === 1 ? "" : "s"})` : "Not approved yet"}
              </span>
            )}
          </div>
          {check?.approved && !keyUsed && <p className="text-[10px] font-sans text-warn-strong">Approved on-chain — press “Use this key” in step 2, then “Save Changes”.</p>}
        </div>

        {error && (
          <div className="p-2 bg-loss-dim border border-loss-border rounded-lg text-xs font-mono text-loss flex items-start gap-1.5">
            <AlertCircle className="w-3.5 h-3.5 shrink-0 mt-0.5" />
            <span>{error}</span>
          </div>
        )}
      </div>
    </div>
  );
};

export const AccountCard: React.FC<AccountCardProps> = ({
  account,
  ordinal,
  locked,
  bal,
  lane,
  onChanged,
}) => {
  const fields = ACCOUNT_CREDENTIAL_FIELDS[account.venue] ?? [
    { key: "apiKey" as CredFieldKey, label: "API Key", secret: true },
    { key: "apiSecret" as CredFieldKey, label: "API Secret", secret: true },
  ];
  const [label, setLabel] = useState(account.label);
  const [tier, setTier] = useState(account.accountTier || "standard");
  // Per-account leverage ceiling as text; "" = unlimited (null over IPC).
  const [maxLeverage, setMaxLeverage] = useState(
    account.maxLeverage != null ? String(account.maxLeverage) : ""
  );
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [showSecrets, setShowSecrets] = useState<Record<string, boolean>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Saved persists until the next edit — `dirty` flips the button back.
  const saveBtn = useSaveButton({ persistSaved: true });
  const [toolkitOpen, setToolkitOpen] = useState(false);
  // Decrypted credentials loaded on demand — never fetched while locked.
  // Security hardening: NEVER hold the whole credential bundle in renderer
  // memory. `revealed` keeps only fields the user explicitly revealed,
  // fetched one at a time via getAccountCredentialField; `requested` stops
  // refetch loops (requested-but-empty stays empty until next save/unlock).
  const [revealed, setRevealed] = useState<Record<string, string>>({});
  const [requested, setRequested] = useState<Record<string, boolean>>({});

  // Reset local drafts when the account identity changes (e.g. venue switch).
  useEffect(() => {
    setLabel(account.label);
    setTier(account.accountTier || "standard");
    setMaxLeverage(account.maxLeverage != null ? String(account.maxLeverage) : "");
    setDrafts({});
    setError(null);
    saveBtn.reset();
    setShowSecrets({});
    // Drop revealed secrets when switching accounts (they belong to the last one).
    setRevealed({});
    setRequested({});
  }, [account.id, account.label]);

  // Fetch ONE field on explicit reveal. Bulk prefetch was removed: pulling
  // the full bundle meant every secret sat in React state from the first
  // eye-click. Numeric index fields never fetch (prefilled from AccountDto).
  const revealField = useCallback((key: CredFieldKey) => {
    if (requested[key] || locked || !api.isTauri) return;
    if (key === "accountIndex" || key === "subaccountIndex" || key === "apiKeyIndex") return;
    setRequested((r) => ({ ...r, [key]: true }));
    api
      .getAccountCredentialField(account.id, key)
      .then((v) => setRevealed((r) => ({ ...r, [key]: v ?? "" })))
      .catch(() => {});
  }, [requested, locked, account.id]);

  // Drop decrypted material the moment the vault re-locks.
  useEffect(() => {
    if (locked) {
      // Drop decrypted material the moment the vault re-locks.
      setRevealed({});
      setRequested({});
      setShowSecrets({});
    }
  }, [locked]);

  // Public identifiers (wallet addresses, account IDs, non-secret API keys)
  // are always shown, never hidden behind the "(stored — leave blank to
  // keep)" placeholder: auto-fetch them while unlocked. Secrets stay masked
  // until the eye toggle. Numeric indexes are prefilled from the DTO.
  useEffect(() => {
    if (locked || !api.isTauri) return;
    for (const f of fields) {
      if (f.secret) continue;
      const k = f.key;
      if (k === "accountIndex" || k === "subaccountIndex" || k === "apiKeyIndex") continue;
      if (!fieldIsSet(account, k)) continue;
      if (drafts[k] !== undefined || revealed[k] !== undefined || requested[k]) continue;
      revealField(k);
    }
  }, [locked, account, fields, drafts, revealed, requested, revealField]);

  const storedValueFor = (key: CredFieldKey): string => revealed[key] ?? "";

  const valueFor = (key: CredFieldKey): string => {
    if (drafts[key] !== undefined) return drafts[key];
    const stored = storedValueFor(key);
    if (stored !== "") return stored;
    // Numeric index fields are exposed non-secretly; prefill them so a
    // credentials save round-trips the current value instead of clearing it.
    if (key === "accountIndex") return account.accountIndex?.toString() ?? "";
    if (key === "subaccountIndex") return account.subaccountIndex?.toString() ?? "";
    if (key === "apiKeyIndex") return account.apiKeyIndex?.toString() ?? "";
    return "";
  };

  const setValue = (key: string, v: string) => setDrafts((d) => ({ ...d, [key]: v }));

  // Grey-out rule: the Enabled switch stays disabled (grey, unclickable) while
  // any required account-card field is empty. Optional fields never block.
  // A stored value (presence flag) or a non-empty draft/revealed value counts
  // as filled. Empty-field venues (edgeX, long-tail venues) have no
  // required fields and are always enableable. While the vault is locked the
  // presence is unknowable, so gating is skipped (locked=false bypass above).
  const requiredFilled = fields.every(
    (f) => f.optional || (valueFor(f.key) ?? "").trim() !== "" || fieldIsSet(account, f.key),
  );

  const buildCredentials = (): { creds?: AccountCredentialInput; error?: string } => {
    const creds: AccountCredentialInput = {};
    let any = false;
    for (const f of fields) {
      const raw = (valueFor(f.key) ?? "").trim();
      if (f.key === "baseUrl") {
        // RPC endpoints are secret and often embed provider API keys: only
        // a draft decides the outcome — empty string is the explicit-clear
        // signal, a value sets. No draft means untouched (an unrevealed
        // stored URL is never accidentally cleared); a revealed non-empty
        // value still round-trips through the generic path below.
        if (Object.prototype.hasOwnProperty.call(drafts, f.key)) {
          (creds as Record<string, unknown>)[f.key] = raw;
          any = true;
        } else if (raw !== "") {
          (creds as Record<string, unknown>)[f.key] = raw;
          any = true;
        }
        continue;
      }
      if (f.numeric) {
        // Index fields always serialize: non-empty sets, empty clears.
        if (raw === "") {
          (creds as Record<string, unknown>)[f.key] = null;
          any = true;
        } else {
          const n = Number.parseInt(raw, 10);
          if (!Number.isSafeInteger(n) || n < 0) return { error: `${f.label} must be a non-negative integer below 2^53` };
          (creds as Record<string, unknown>)[f.key] = n;
          any = true;
        }
      } else if (raw !== "") {
        (creds as Record<string, unknown>)[f.key] = raw;
        any = true;
      }
    }
    return { creds: any ? creds : undefined };
  };

  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await fn();
      onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleSave = () =>
    void saveBtn.run(async () => {
      try {
        const { creds, error } = buildCredentials();
        if (error) throw new Error(error);
        if (!label.trim()) throw new Error("Account label is required");
        const cap = parseMaxLeverage(maxLeverage);
        await api.upsertAccount({
          id: account.id,
          label: label.trim(),
          enabled: account.enabled,
          makeDefault: account.isDefault,
          accountTier: tier,
          maxLeverage: cap,
          credentials: creds ?? null,
        });
        setDrafts({});
        // Drop revealed secrets after save: placeholders fall back to the
        // presence flags (STORED_MASK) until the user reveals a field again.
        setRevealed({});
        setRequested({});
        onChanged();
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        throw e; // rethrow so the save button lands on "Failed!"
      }
    });

  const handleToggleEnabled = () =>
    run(async () => {
      persistVenueFlag(account.venue, !account.enabled); // keep the venue flag in step
      await api.upsertAccount({
        id: account.id,
        label: account.label,
        enabled: !account.enabled,
        makeDefault: account.isDefault,
        accountTier: account.accountTier,
        maxLeverage: account.maxLeverage ?? null,
      });
    });

  const handleSetDefault = () =>
    run(async () => {
      await api.setDefaultAccount(account.venue, account.id);
    });

  const handleDelete = () => {
    if (!window.confirm(`Delete account "${account.label}" on ${venueDisplay(account.venue)}? Its stored credentials are removed permanently.`)) {
      return;
    }
    void run(async () => {
      await api.deleteAccount(account.id);
    });
  };

  const dirty =
    label.trim() !== account.label ||
    tier !== (account.accountTier || "standard") ||
    normalizeLeverageText(maxLeverage) !== normalizeLeverageText(
      account.maxLeverage != null ? String(account.maxLeverage) : ""
    ) ||
    Object.keys(drafts).length > 0;

  // Lighter-family venues document per-account rate-limit tiers that share
  // the IP budget — the monitor derives its read cadence from the lowest
  // tier present on the venue.
  const showTier = account.venue === "lighter" || account.venue === "robinhood";

  return (
    <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
      {/* Card header: title + inline label + default state + delete, toggle */}
      <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
        <div className="flex items-center space-x-2 min-w-0 flex-1">
          <VenueLogo venueId={account.venue.toLowerCase()} name={venueDisplay(account.venue)} size={20} />
          <h3 className="text-[13px] font-mono font-bold text-slate-900 shrink-0">
            {venueDisplay(account.venue)} #{ordinal}
          </h3>
          <input
            type="text"
            value={label}
            disabled={busy}
            onChange={(e) => setLabel(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && dirty) void handleSave();
            }}
            placeholder={`${venueDisplay(account.venue)} Account ${ordinal}`}
            title="Account label used in strategy routing"
            className="w-48 px-2 py-1 bg-surface-raised border border-surface-border rounded-lg text-xs font-mono text-slate-900 placeholder:text-muted focus:outline-none focus:ring-1 focus:ring-accent"
          />
          <span
            className={`px-1.5 py-0.5 rounded-md text-[10px] font-mono font-bold tracking-wide shrink-0 border ${
              account.environment === "live"
                ? "bg-loss-dim/40 text-loss border-loss-border/60"
                : "bg-accent/10 text-accent-strong border-accent/30"
            }`}
            title={`This account's credentials bind to the ${account.environment === "live" ? "live production" : "testnet/demo"} environment`}
          >
            {account.environment === "live" ? "LIVE" : "TESTNET"}
          </span>
          {account.isDefault ? (
            <span className="px-1.5 py-0.5 rounded-md text-xs font-mono font-bold bg-warn-dim text-warn-strong border border-warn-border tracking-wide shrink-0">
              Default Account
            </span>
          ) : (
            <button
              type="button"
              disabled={busy}
              onClick={() => void handleSetDefault()}
              className="px-2 py-2 rounded-lg border border-surface-border text-xs font-mono font-semibold text-slate-500 hover:text-warn-strong hover:border-warn-border transition cursor-pointer shrink-0 disabled:opacity-40 disabled:cursor-not-allowed disabled:hover:text-slate-500 disabled:hover:border-surface-border"
              title="Set this account as the venue default"
            >
              Set As Default
            </button>
          )}
        </div>
        <div className="flex items-center space-x-3 shrink-0">
          <button
            type="button"
            disabled={busy}
            onClick={handleDelete}
            className="p-1.5 text-muted hover:text-loss disabled:opacity-40 transition cursor-pointer"
            title="Delete this account and its stored credentials"
          >
            <Trash2 className="w-3.5 h-3.5" />
          </button>
          <ToggleSwitch
            size="sm"
            checked={account.enabled}
            onChange={() => void handleToggleEnabled()}
            disabled={busy || (!account.enabled && !requiredFilled && !locked)}
            ariaLabel={`${venueDisplay(account.venue)} #${ordinal} enabled`}
            title={
              !account.enabled && !requiredFilled && !locked
                ? "Fill required credential fields to enable"
                : `${venueDisplay(account.venue)} #${ordinal} enabled`
            }
          />
        </div>
      </div>

      {/* Per-account Balance Information Gadget */}
      {!account.enabled ? (
        <div className="p-3.5 bg-slate-50/70 border border-surface-border rounded-lg flex items-center justify-between text-xs font-mono text-slate-500">
          <div className="flex items-center space-x-2">
            <span className="w-2 h-2 rounded-full bg-slate-400" />
            <span>Account is disabled — live balance and trading are paused.</span>
          </div>
          <span className="text-muted">Toggle switch above to enable</span>
        </div>
      ) : (
        <VenueBalancePanel
          bal={bal ?? null}
          lane={lane}
          accountTier={tier}
          isDefault={account.isDefault}
        />
      )}

      {/* Account configuration and credentials */}
      <div className="space-y-3 pt-1">
        {showTier && (
          <div className="text-xs font-mono">
            <label className="text-slate-600 block mb-1">
              Account Tier{" "}
              <span className="text-muted">
                (rate-limit budget — lowest tier on this IP wins)
              </span>
              :
            </label>
            <select
              value={tier}
              disabled={busy}
              onChange={(e) => setTier(e.target.value)}
              className="w-full max-w-56 px-2 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-xs font-mono text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
            >
              <option value="standard">Standard — 60 req/min per IP</option>
              <option value="plus">Plus — high read limit, +0.5 bps fees</option>
              <option value="premium">Premium — 24k weighted req/min</option>
              <option value="builder">Builder — 240k weighted req/min</option>
            </select>
          </div>
        )}
        <div className="text-xs font-mono flex items-center justify-between gap-3">
          <label className="text-slate-600 flex-1">
            Max Leverage:
          </label>
          <input
            type="number"
            step="1"
            min="1"
            value={maxLeverage}
            disabled={busy}
            placeholder="Unlimited"
            onChange={(e) => setMaxLeverage(e.target.value)}
            className="w-28 shrink-0 px-2 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-xs font-mono text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
          />
        </div>
        <CredentialGrid
          fields={fields}
          values={Object.fromEntries(fields.map((f) => [f.key, valueFor(f.key)]))}
          setValue={setValue}
          disabled={busy || locked}
          account={account}
          showSecrets={showSecrets}
          toggleShow={(k) => {
            const opening = !showSecrets[k];
            setShowSecrets((st) => ({ ...st, [k]: !st[k] }));
            if (opening) revealField(k as CredFieldKey);
          }}
        />

        <div className="flex items-center justify-between pt-1">
          <div className="text-xs font-mono text-muted">
            {account.credentialsConfigured === null
              ? "Unlock the vault to manage credentials"
              : account.credentialsConfigured
              ? null
              : "No credentials stored for this account"}
          </div>
          {account.venue.toLowerCase() === "popdex" && (
            <button
              type="button"
              onClick={() => setToolkitOpen(true)}
              className="w-full mt-1 px-3 py-2 rounded-lg text-xs font-mono font-bold bg-accent-dim border border-accent-line text-accent-strong hover:brightness-95 transition cursor-pointer flex items-center justify-center gap-1.5"
              title="Generate an agent key, approve it in your own wallet, verify on-chain"
            >
              <KeyRound className="w-3.5 h-3.5" /> Agent Setup Toolkit — generate key & approve
            </button>
          )}
          <SaveButton
            state={saveBtn.state === "saved" && dirty ? "idle" : saveBtn.state}
            disabled={busy || locked || !dirty}
            onClick={handleSave}
          />
        </div>

        {error && (
          <div className="p-2 bg-loss-dim border border-loss-border rounded-lg text-xs font-mono text-loss flex items-start gap-1.5">
            <AlertCircle className="w-3.5 h-3.5 shrink-0 mt-0.5" />
            <span>{error}</span>
          </div>
        )}
        {toolkitOpen && account.venue.toLowerCase() === "popdex" && (
          <PopdexAgentToolkit
            initialMain={(valueFor("walletAddress" as CredFieldKey) ?? "").trim()}
            onUseKey={(k, m) => { setValue("privateKey", k); if (m) setValue("walletAddress", m); }}
            onClose={() => setToolkitOpen(false)}
          />
        )}
      </div>
    </div>
  );
};

/* ------------------------------------------------------------------ */
/* Add-account form                                                    */
/* ------------------------------------------------------------------ */

/** Label drafts keyed by venue — preserved for backwards compatibility. */
const accountLabelDrafts = new Map<string, string>();

interface AddAccountFormProps {
  venue: string;
  hasDefault: boolean;
  locked: boolean;
  bal?: VenueBalanceInfo | null;
  lane?: VenueLaneStatusDto | null;
  fallbackEnabled?: boolean;
  onFallbackChange?: (enabled: boolean) => void;
  labelFromHeader?: boolean;
  nextOrdinal?: number;
  onDone: () => void;
  onCancel?: () => void;
}

const AddAccountForm: React.FC<AddAccountFormProps> = ({
  venue,
  hasDefault,
  locked,
  bal: _bal,
  lane: _lane,
  fallbackEnabled,
  onFallbackChange,
  nextOrdinal = 1,
  onDone,
  onCancel,
}) => {
  const fields = ACCOUNT_CREDENTIAL_FIELDS[venue] ?? [
    { key: "apiKey" as CredFieldKey, label: "API Key", secret: true },
    { key: "apiSecret" as CredFieldKey, label: "API Secret", secret: true },
  ];
  const autoLabel = `${venueDisplay(venue)} Account ${nextOrdinal}`;
  const [label, setLabel] = useState("");
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [showSecrets, setShowSecrets] = useState<Record<string, boolean>>({});
  const saveBtn = useSaveButton();
  const [error, setError] = useState<string | null>(null);
  const [toolkitOpen, setToolkitOpen] = useState(false);
  // Inputs disable while the create-save is in flight.
  const busy = saveBtn.state === "saving";

  const handleCreate = async () => {
    setError(null);
    await saveBtn.run(async () => {
    try {
      const effectiveLabel = label.trim() || autoLabel;
      const creds: AccountCredentialInput = {};
      let any = false;
      for (const f of fields) {
        const raw = (drafts[f.key] ?? "").trim();
        if (raw === "") continue;
        if (f.numeric) {
          const n = Number.parseInt(raw, 10);
          if (!Number.isSafeInteger(n) || n < 0) throw new Error(`${f.label} must be a non-negative integer below 2^53`);
          (creds as Record<string, unknown>)[f.key] = n;
        } else {
          (creds as Record<string, unknown>)[f.key] = raw;
        }
        any = true;
      }
      // New accounts with missing required fields are created disabled so the
      // card's greyed-out Enabled switch matches (user must fill fields first).
      const draftFilled = fields.every(
        (f) => f.optional || (drafts[f.key] ?? "").trim() !== "",
      );
      persistVenueFlag(venue, draftFilled);
      await api.upsertAccount({
        venue,
        label: effectiveLabel,
        enabled: draftFilled,
        makeDefault: !hasDefault,
        maxLeverage: null,
        credentials: any ? creds : null,
      });
      onDone();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      throw e; // rethrow so the save button lands on "Failed!"
    }
    });
  };

  return (
    <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
      <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
        <div className="flex items-center space-x-2 min-w-0 flex-1">
          <h3 className="text-[13px] font-mono font-bold text-slate-900 shrink-0">
            {venueDisplay(venue)} #{nextOrdinal}
          </h3>
          <input
            type="text"
            value={label}
            disabled={busy}
            placeholder={autoLabel}
            onChange={(e) => setLabel(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void handleCreate();
            }}
            autoFocus={nextOrdinal > 1}
            className="w-48 px-2 py-1 bg-surface-raised border border-surface-border rounded-lg text-xs font-mono text-slate-900 placeholder:text-muted focus:outline-none focus:ring-1 focus:ring-accent"
          />
          {!hasDefault && (
            <span className="px-1.5 py-0.5 rounded-md text-xs font-mono font-bold bg-warn-dim text-warn-strong border border-warn-border tracking-wide shrink-0">
              Default
            </span>
          )}
        </div>
        <div className="flex items-center space-x-3 shrink-0">
          {onCancel && (
            <button
              type="button"
              onClick={onCancel}
              disabled={busy}
              className="px-2 py-2 rounded-lg border border-surface-border text-xs font-mono text-slate-500 hover:text-slate-800 transition cursor-pointer"
            >
              Cancel
            </button>
          )}
          {/* The fallback venue-flag toggle only exists where the venue has
              an engine-settings flag — flag-less venues (long-tail perps,
              EVM AMMs) admit purely via an enabled account, so the toggle
              would be a dead control. */}
          {onFallbackChange && VENUE_SETTINGS_FLAG[venue] !== undefined && (
            <ToggleSwitch
              size="sm"
              checked={fallbackEnabled ?? true}
              onChange={(en) => onFallbackChange(en)}
              ariaLabel={`${venueDisplay(venue)} venue enabled`}
              title={`${venueDisplay(venue)} venue enabled`}
            />
          )}
        </div>
      </div>

      <div className="space-y-3 pt-1">
        <CredentialGrid
          fields={fields}
          values={drafts}
          setValue={(k, v) => setDrafts((d) => ({ ...d, [k]: v }))}
          disabled={busy || locked}
          account={null}
          showSecrets={showSecrets}
          toggleShow={(k) => setShowSecrets((s) => ({ ...s, [k]: !s[k] }))}
        />
        {locked && (
          <p className="text-xs font-mono text-warn-strong">
            Vault is locked: the account can be created now, but credentials require an unlocked vault.
          </p>
        )}

        {error && (
          <div className="p-2 bg-loss-dim border border-loss-border rounded-lg text-xs font-mono text-loss flex items-start gap-1.5">
            <AlertCircle className="w-3.5 h-3.5 shrink-0 mt-0.5" />
            <span>{error}</span>
          </div>
        )}

        <div className="flex justify-end pt-1 gap-2">
          {venue.toLowerCase() === "popdex" && (
            <button
              type="button"
              onClick={() => setToolkitOpen(true)}
              className="px-3 py-2 rounded-lg text-xs font-mono font-bold bg-accent-dim border border-accent-line text-accent-strong hover:brightness-95 transition cursor-pointer flex items-center gap-1.5"
              title="Generate an agent key, approve it in your own wallet, verify on-chain"
            >
              <KeyRound className="w-3.5 h-3.5" /> Agent Setup Toolkit — generate key & approve
            </button>
          )}
          <SaveButton
            state={saveBtn.state}
            onClick={handleCreate}
          />
        </div>
        {toolkitOpen && venue.toLowerCase() === "popdex" && (
          <PopdexAgentToolkit
            initialMain={(drafts["walletAddress"] ?? "").trim()}
            onUseKey={(k, m) => setDrafts((d) => ({ ...d, privateKey: k, ...(m ? { walletAddress: m } : {}) }))}
            onClose={() => setToolkitOpen(false)}
          />
        )}
      </div>
    </div>
  );
};

/* ------------------------------------------------------------------ */
/* Venue accounts section (balance + account cards)                    */
/* ------------------------------------------------------------------ */

export interface VenueAccountsSectionProps {
  venue: string;
  locked: boolean;
  /** Active trading environment — badges cards and gates testnet-incapable venues. */
  environment?: "testnet" | "live";
  bal?: VenueBalanceInfo | null;
  lane?: VenueLaneStatusDto | null;
  fallbackEnabled?: boolean;
  onFallbackChange?: (enabled: boolean) => void;
  compact?: boolean;
  refreshKey?: number;
  onChanged?: () => void;
}

export const VenueAccountsSection: React.FC<VenueAccountsSectionProps> = ({
  venue,
  locked,
  environment,
  bal,
  lane,
  fallbackEnabled = false,
  onFallbackChange,
  compact = false,
  refreshKey = 0,
  onChanged,
}) => {
  const [accounts, setAccounts] = useState<AccountDto[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      setAccounts(await api.listAccounts(venue));
      setLoadError(null);
    } catch (e) {
      setLoadError(e instanceof Error ? e.message : String(e));
    }
  }, [venue]);

  useEffect(() => {
    void reload();
  }, [reload, refreshKey]);

  return (
    <div className={compact ? "space-y-3" : "space-y-4"}>
      {loadError && (
        <div className="p-2 bg-loss-dim border border-loss-border rounded-lg text-xs font-mono text-loss">
          {loadError}
        </div>
      )}

      {environment === "testnet" && !venueHasTestnet(venue) && (
        <div className="flex items-start gap-2 px-3 py-2 rounded-lg border bg-warn-dim/40 border-warn-border text-[11px] font-mono text-slate-700">
          <AlertTriangle className="w-3.5 h-3.5 shrink-0 mt-0.5 text-warn-strong" />
          <span>
            {venueDisplay(venue)} has no public testnet — accounts created now bind to Testnet
            but stay ineligible until the venue exposes a demo endpoint. Switch to Live Trading
            to manage production credentials.
          </span>
        </div>
      )}

      {accounts === null && !loadError && (
        <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs animate-pulse">
          <div className="h-4 w-40 rounded bg-slate-100" />
          <div className="h-9 w-full rounded-lg bg-slate-100" />
          <div className="h-9 w-full rounded-lg bg-slate-100" />
        </div>
      )}

      {accounts && accounts.length === 0 && (
        <AddAccountForm
          venue={venue}
          hasDefault={false}
          locked={locked}
          bal={null}
          lane={lane}
          fallbackEnabled={fallbackEnabled}
          onFallbackChange={onFallbackChange}
          nextOrdinal={1}
          onDone={() => {
            accountLabelDrafts.delete(venue);
            void reload();
            onChanged?.();
          }}
        />
      )}

      {accounts && accounts.length > 0 && (
        <div className="space-y-4">
          {accounts.map((a, i) => {
            const accountBal = (() => {
              if (!a.enabled) return null;
              if (bal?.account_id) {
                return bal.account_id === a.id ? bal : null;
              }
              return a.isDefault || accounts.length === 1 ? bal : null;
            })();
            return (
              <AccountCard
                key={a.id}
                account={a}
                ordinal={i + 1}
                locked={locked}
                bal={accountBal}
                lane={lane}
                onChanged={() => {
                  void reload();
                  onChanged?.();
                }}
              />
            );
          })}

          <AddAccountSlot
            venue={venue}
            locked={locked}
            bal={null}
            lane={lane}
            onChanged={() => {
              void reload();
              onChanged?.();
            }}
          />
        </div>
      )}
    </div>
  );
};

/* ------------------------------------------------------------------ */
/* Shared EVM wallet section — one key + per-venue coverage gadget      */
/* ------------------------------------------------------------------ */

/**
 * The "EVM Wallet" exchange page: a single credential card whose key fans
 * out to every EVM AMM venue's default account (swap venues get the key,
 * market-data venues only the address), plus a gadget listing every venue
 * the wallet can drive — per-venue enable toggles and RPC overrides.
 * The venues are live-only: in Testnet mode the section degrades to the
 * standard no-testnet banner.
 */
/** Custody-state chip for the wallet card header: reports where the
 *  shared EVM key actually lives. `stronghold`+loaded = guarded signing
 *  (no key bytes in process memory); `stronghold`+unloaded = records
 *  reference custody that cannot currently sign; `vault` = legacy
 *  encrypted-record storage; `none`/null = nothing to report. */
export const EvmCustodyChip: React.FC<{ custody: EvmKeyCustodyDto | null }> = ({ custody }) => {
  if (!custody || custody.custody === "none") return null;
  if (custody.custody === "stronghold" && custody.strongholdLoaded) {
    return (
      <span
        className="inline-flex items-center gap-1 text-[10px] font-mono px-1.5 py-0.5 rounded-md bg-profit-dim/60 text-profit border border-profit-border"
        title="The private key lives in an IOTA Stronghold snapshot — it never enters app memory; signing happens inside the guarded vault."
      >
        <ShieldCheck className="w-3 h-3" /> Stronghold custody
      </span>
    );
  }
  if (custody.custody === "stronghold") {
    return (
      <span
        className="inline-flex items-center gap-1 text-[10px] font-mono px-1.5 py-0.5 rounded-md bg-warn-dim/60 text-warn-strong border border-warn-border"
        title="Records reference Stronghold custody but no snapshot is loaded or the key record is missing — the wallet cannot sign until the vault is unlocked and the key is restored."
      >
        <ShieldAlert className="w-3 h-3" /> Stronghold key unavailable
      </span>
    );
  }
  return (
    <span
      className="inline-flex items-center gap-1 text-[10px] font-mono px-1.5 py-0.5 rounded-md bg-warn-dim/60 text-warn-strong border border-warn-border"
      title="The private key is stored in the encrypted vault (legacy/fallback path). It will migrate into Stronghold on the next unlock once a snapshot is available."
    >
      <KeyRound className="w-3 h-3" /> Key in vault
    </span>
  );
};

export const EvmWalletSection: React.FC<{
  locked: boolean;
  environment?: "testnet" | "live";
  refreshKey?: number;
  onChanged?: () => void;
}> = ({ locked, environment = "live", refreshKey = 0, onChanged }) => {
  const [accounts, setAccounts] = useState<AccountDto[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [custody, setCustody] = useState<EvmKeyCustodyDto | null>(null);
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [showSecrets, setShowSecrets] = useState<Record<string, boolean>>({});
  const [rpcDrafts, setRpcDrafts] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const saveBtn = useSaveButton();

  const reload = useCallback(async () => {
    try {
      const all = await api.listAccounts();
      setAccounts(all.filter((a) => EVM_AMM_VENUE_IDS.has(a.venue)));
      setLoadError(null);
    } catch (e) {
      setLoadError(e instanceof Error ? e.message : String(e));
    }
    if (locked) {
      setCustody(null);
      return;
    }
    try {
      setCustody(await api.getEvmKeyCustody());
    } catch {
      setCustody(null); // locked/unknown — the chip simply stays hidden
    }
  }, [locked]);

  useEffect(() => {
    void reload();
  }, [reload, refreshKey]);

  // The account a gadget row operates on: the venue's credential-resolving
  // default when present, else its first live-environment account.
  const acctFor = (vid: string): AccountDto | null =>
    (accounts ?? [])
      .filter((a) => a.venue === vid)
      .sort((a, b) => Number(b.isDefault) - Number(a.isDefault))[0] ?? null;

  // Presence flags + stored-signer readout source: any keyed venue's
  // account represents the wallet (all keyed venues share it post-save).
  // A credential-resolving default is preferred — only a default account's
  // credentials are provisioned, so its venue's adapter reflects the live
  // signer the wallet card compares against.
  const keyedExec = (accounts ?? []).filter(
    (a) =>
      a.hasPrivateKey &&
      EVM_AMM_VENUES.some((v) => v.id === a.venue && v.execution),
  );
  const representative =
    keyedExec.find((a) => a.isDefault) ??
    keyedExec[0] ??
    (accounts ?? [])[0] ??
    null;

  const enabledCount = (accounts ?? []).filter((a) => a.enabled).length;

  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleSave = () =>
    void saveBtn.run(async () => {
      try {
        setError(null);
        setNotice(null);
        const pk = (drafts.privateKey ?? "").trim();
        const wa = (drafts.walletAddress ?? "").trim();
        if (!pk) throw new Error("Wallet private key is required");
        if (!/^(0x)?[0-9a-fA-F]{64}$/.test(pk)) {
          throw new Error("Not a valid EVM private key — expected 64 hex characters");
        }
        const res = await api.saveEvmWallet(pk, wa || null);
        setDrafts({});
        setNotice(
          `Wallet saved — signs as ${res.signerAddress}. ` +
            `${res.keyedVenues.length} swap venues received the key; ${res.addressOnlyVenues.length} market-data venues received the address only. ` +
            (res.keyStorage === "stronghold"
              ? "Key held in Stronghold guarded storage — records carry only a reference."
              : "Key stored encrypted in the vault (Stronghold unavailable).") +
            (res.createdAccounts.length
              ? ` ${res.createdAccounts.length} venue account(s) created disabled — enable them below.`
              : ""),
        );
        void reload();
        onChanged?.();
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        throw e; // rethrow so the save button lands on "Failed!"
      }
    });

  const handleClear = () => {
    if (
      !window.confirm(
        "Remove the shared wallet key and address from every EVM AMM venue? Account rows, enabled flags and RPC overrides are kept — venues keep market data but cannot trade.",
      )
    ) {
      return;
    }
    void run(async () => {
      const n = await api.clearEvmWallet();
      setNotice(
        n
          ? `Wallet material cleared from ${n} credential record(s).`
          : "No stored wallet material found.",
      );
      void reload();
      onChanged?.();
    });
  };

  // Enabling always makes the wallet account the venue's default — that is
  // the account credential resolution reads, so a stale second key can
  // never keep signing while the UI shows the shared wallet.
  const toggleVenue = (v: (typeof EVM_AMM_VENUES)[number], acct: AccountDto | null) =>
    void run(async () => {
      if (acct) {
        await api.upsertAccount({
          id: acct.id,
          label: acct.label,
          enabled: !acct.enabled,
          makeDefault: !acct.enabled || acct.isDefault,
          accountTier: acct.accountTier,
          maxLeverage: acct.maxLeverage ?? null,
        });
      } else {
        await api.upsertAccount({
          venue: v.id,
          label: "EVM Wallet",
          enabled: true,
          makeDefault: true,
          environment: "live",
        });
      }
      void reload();
      onChanged?.();
    });

  // RPC override: `Some("")` clears a stored endpoint, a value sets it,
  // and an untouched draft writes nothing (baseUrl patch semantics).
  const saveRpc = (v: (typeof EVM_AMM_VENUES)[number], acct: AccountDto | null, raw: string) =>
    void run(async () => {
      const val = raw.trim();
      if (acct) {
        await api.upsertAccount({
          id: acct.id,
          label: acct.label,
          enabled: acct.enabled,
          makeDefault: acct.isDefault,
          accountTier: acct.accountTier,
          maxLeverage: acct.maxLeverage ?? null,
          credentials: { baseUrl: val },
        });
      } else {
        await api.upsertAccount({
          venue: v.id,
          label: "EVM Wallet",
          enabled: false,
          makeDefault: false,
          environment: "live",
          credentials: { baseUrl: val },
        });
      }
      setRpcDrafts((d) => {
        const n = { ...d };
        delete n[v.id];
        return n;
      });
      void reload();
      onChanged?.();
    });

  if (environment === "testnet") {
    return (
      <div className="flex items-start gap-2 px-3 py-2 rounded-lg border bg-warn-dim/40 border-warn-border text-[11px] font-mono text-slate-700">
        <AlertTriangle className="w-3.5 h-3.5 shrink-0 mt-0.5 text-warn-strong" />
        <span>
          EVM AMM venues have no public testnet — the shared wallet binds to the Live
          environment. Switch to Live Trading to configure it.
        </span>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      {loadError && (
        <div className="p-2 bg-loss-dim border border-loss-border rounded-lg text-xs font-mono text-loss">
          {loadError}
        </div>
      )}

      {/* Shared wallet credential card */}
      <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
        <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
          <div className="flex items-center space-x-2 min-w-0">
            <Wallet className="w-4 h-4 text-slate-600" />
            <h3 className="text-[13px] font-mono font-bold text-slate-900">EVM Wallet</h3>
            <span className="text-[10px] font-mono px-1.5 py-0.5 rounded-md bg-slate-100 text-slate-600 border border-surface-border">
              one signer → all EVM AMMs
            </span>
            <EvmCustodyChip custody={custody} />
          </div>
          {representative?.hasPrivateKey && (
            <button
              type="button"
              onClick={handleClear}
              disabled={busy || locked}
              className="px-2 py-1 rounded-lg border border-loss-border text-[11px] font-mono text-loss hover:bg-loss-dim transition cursor-pointer disabled:opacity-50"
              title="Strip the wallet key and address from every AMM venue (accounts and RPC overrides stay)"
            >
              Remove Wallet
            </button>
          )}
        </div>

        <p className="text-[11px] text-slate-500 font-sans leading-normal">
          DEX venues have no exchange account — this key signs swap transactions
          directly onchain. It holds <strong>full custody</strong> of the wallet on
          every chain below; use a dedicated hot wallet holding only the trading
          funds, never your main wallet key.
        </p>

        <CredentialGrid
          fields={EVM_WALLET_FIELDS}
          values={drafts}
          setValue={(k, v) => setDrafts((d) => ({ ...d, [k]: v }))}
          disabled={busy || locked}
          account={representative}
          showSecrets={showSecrets}
          toggleShow={(k) => setShowSecrets((s) => ({ ...s, [k]: !s[k] }))}
        />

        {locked && (
          <p className="text-xs font-mono text-warn-strong">
            Vault is locked: credentials require an unlocked vault.
          </p>
        )}
        {error && (
          <div className="p-2 bg-loss-dim border border-loss-border rounded-lg text-xs font-mono text-loss flex items-start gap-1.5">
            <AlertCircle className="w-3.5 h-3.5 shrink-0 mt-0.5" />
            <span>{error}</span>
          </div>
        )}
        {notice && (
          <div className="p-2 bg-profit-dim/60 border border-profit-border rounded-lg text-xs font-mono text-profit flex items-start gap-1.5">
            <Check className="w-3.5 h-3.5 shrink-0 mt-0.5" />
            <span>{notice}</span>
          </div>
        )}

        <div className="flex justify-end pt-1">
          <SaveButton state={saveBtn.state} onClick={handleSave} />
        </div>
      </div>

      {/* Coverage gadget — every venue the wallet can drive */}
      <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-2.5 shadow-2xs">
        <div className="flex items-center justify-between">
          <h4 className="text-[13px] font-mono font-bold text-slate-900">
            Venues reachable with this wallet
          </h4>
          <span className="text-[10px] font-mono text-muted">
            {enabledCount}/{EVM_AMM_VENUES.length} enabled
          </span>
        </div>
        <p className="text-[11px] text-slate-500 font-sans leading-normal">
          Enabling admits the venue for quotes — and swaps on swap-capable
          venues once the wallet key is saved. RPC overrides apply per chain:
          same-chain venues share an endpoint unless a venue sets its own.
        </p>

        <div className="divide-y divide-surface-border/70">
          {EVM_AMM_VENUES.map((v) => {
            const acct = acctFor(v.id);
            const credChip = v.execution
              ? acct?.hasPrivateKey
                ? { text: "signer key stored", cls: "text-profit" }
                : { text: "no key — market data only", cls: "text-amber-700" }
              : acct?.hasWalletAddress
                ? { text: "address stored", cls: "text-slate-500" }
                : { text: "no address", cls: "text-muted" };
            const rpcDraft = rpcDrafts[v.id];
            const rpcDirty = (rpcDraft ?? "").trim() !== "";
            return (
              <div key={v.id} className="py-2.5 flex items-center gap-3">
                <VenueLogo venueId={v.id} name={venueDisplay(v.id)} size={20} />
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2 flex-wrap">
                    <span className="text-xs font-mono font-bold text-slate-900">
                      {venueDisplay(v.id)}
                    </span>
                    <span className="text-[10px] font-mono px-1.5 py-0.5 rounded-md bg-slate-100 text-slate-600 border border-surface-border">
                      {v.chain}
                    </span>
                    <span
                      className={`text-[10px] font-mono px-1.5 py-0.5 rounded-md border ${
                        v.execution
                          ? "bg-accent-dim text-accent-strong border-accent-line"
                          : "bg-slate-50 text-slate-500 border-surface-border"
                      }`}
                    >
                      {v.execution ? "Swaps + quotes" : "Market data"}
                    </span>
                    <span className={`text-[10px] font-mono ${credChip.cls}`}>
                      {credChip.text}
                    </span>
                  </div>
                  <div className="mt-1.5 flex items-center gap-1.5">
                    <input
                      type="text"
                      value={rpcDraft ?? ""}
                      disabled={busy || locked}
                      onChange={(e) =>
                        setRpcDrafts((d) => ({ ...d, [v.id]: e.target.value }))
                      }
                      onKeyDown={(e) => {
                        if (e.key === "Enter" && rpcDirty) void saveRpc(v, acct, rpcDraft);
                      }}
                      onBlur={() => {
                        if (rpcDirty) void saveRpc(v, acct, rpcDraft);
                      }}
                      placeholder={
                        acct?.hasBaseUrl
                          ? "RPC override stored — type to replace"
                          : v.defaultRpc
                      }
                      title={
                        acct?.hasBaseUrl
                          ? "A custom RPC endpoint is stored for this venue"
                          : `Default: ${v.defaultRpc}`
                      }
                      className="w-64 px-2 py-1 bg-surface-raised border border-surface-border rounded-md text-[11px] font-mono text-slate-800 placeholder:text-muted focus:outline-none focus:ring-1 focus:ring-accent disabled:opacity-50"
                    />
                    {acct?.hasBaseUrl && !rpcDirty && (
                      <button
                        type="button"
                        onClick={() => void saveRpc(v, acct, "")}
                        disabled={busy || locked}
                        title="Clear the stored RPC override — back to the chain default"
                        className="px-1.5 py-1 rounded-md border border-surface-border text-[10px] font-mono text-muted hover:text-loss hover:border-loss-border transition cursor-pointer disabled:opacity-50"
                      >
                        reset
                      </button>
                    )}
                  </div>
                </div>
                <ToggleSwitch
                  size="sm"
                  checked={acct?.enabled ?? false}
                  disabled={busy || locked}
                  ariaLabel={`${venueDisplay(v.id)} enabled`}
                  title={
                    acct
                      ? `${venueDisplay(v.id)} enabled`
                      : `Enable ${venueDisplay(v.id)} — creates its wallet-bound account`
                  }
                  onChange={() => toggleVenue(v, acct)}
                />
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
};

/* ------------------------------------------------------------------ */
/* Add-account slot — dashed placeholder that expands into the form    */
/* ------------------------------------------------------------------ */

/** Renders below an account zone: a dashed "+ Add another account" gadget that expands into the new-account form in place. */
export const AddAccountSlot: React.FC<{
  venue: string;
  locked: boolean;
  bal?: VenueBalanceInfo | null;
  lane?: VenueLaneStatusDto | null;
  onChanged?: () => void;
}> = ({ venue, locked, bal: _bal, lane, onChanged }) => {
  const [adding, setAdding] = useState(false);
  const [accounts, setAccounts] = useState<AccountDto[] | null>(null);

  useEffect(() => {
    api
      .listAccounts(venue)
      .then(setAccounts)
      .catch(() => setAccounts(null));
  }, [venue]);

  if (adding) {
    return (
      <AddAccountForm
        venue={venue}
        hasDefault={(accounts ?? []).some((a) => a.isDefault)}
        locked={locked}
        bal={null}
        lane={lane}
        nextOrdinal={(accounts?.length ?? 0) + 1}
        onDone={() => {
          setAdding(false);
          onChanged?.();
        }}
        onCancel={() => setAdding(false)}
      />
    );
  }

  return (
    <button
      type="button"
      onClick={() => setAdding(true)}
      disabled={locked}
      className="w-full min-h-[118px] border border-dashed border-surface-border rounded-lg bg-transparent hover:bg-surface-raised/60 hover:border-accent-line flex flex-col items-center justify-center gap-1.5 text-muted hover:text-accent-strong transition cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
    >
      <Plus className="w-5 h-5" />
      <span className="text-xs font-mono font-semibold">Add Another Account</span>
    </button>
  );
};

/* ------------------------------------------------------------------ */
/* Account label control — sits in the zone header, edits account #1   */
/* ------------------------------------------------------------------ */

/** Editable label for the zone's account. When the zone has no account yet, the input holds the draft the inline credential form submits. */
export const AccountLabelControl: React.FC<{
  venue: string;
  refreshKey?: number;
  onChanged?: () => void;
}> = ({ venue, refreshKey = 0, onChanged }) => {
  const [account, setAccount] = useState<AccountDto | null | undefined>(undefined);
  const [draft, setDraft] = useState(() => accountLabelDrafts.get(venue) ?? "");
  const [editing, setEditing] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api
      .listAccounts(venue)
      .then((list) => setAccount(list[0] ?? null))
      .catch(() => setAccount(null));
  }, [venue, refreshKey]);

  const saveLabel = (value: string) => {
    setEditing(null);
    const next = value.trim();
    if (!account || !next || next === account.label) return;
    setBusy(true);
    void api
      .upsertAccount({ id: account.id, label: next, enabled: account.enabled, makeDefault: account.isDefault, accountTier: account.accountTier, maxLeverage: account.maxLeverage ?? null })
      .then(() => onChanged?.())
      .catch(() => undefined)
      .finally(() => setBusy(false));
  };

  if (account === undefined) return null;

  return (
    <input
      type="text"
      value={account ? (editing ?? account.label) : draft}
      disabled={busy}
      placeholder={`${venueDisplay(venue)} #1`}
      title="Account Label"
      onChange={(e) => {
        if (account) {
          setEditing(e.target.value);
        } else {
          setDraft(e.target.value);
          accountLabelDrafts.set(venue, e.target.value);
        }
      }}
      onBlur={(e) => account && saveLabel(e.target.value)}
      onKeyDown={(e) => {
        if (e.key === "Enter") account && saveLabel((e.target as HTMLInputElement).value);
        if (e.key === "Escape") setEditing(null);
      }}
      className="w-40 px-2 py-1 bg-surface-raised border border-surface-border rounded-lg text-xs font-mono text-slate-900 placeholder:text-muted focus:outline-none focus:ring-1 focus:ring-accent disabled:opacity-50"
    />
  );
};

/* ------------------------------------------------------------------ */
/* Venue default-account control — sits left of a zone toggle          */
/* ------------------------------------------------------------------ */

/** The account zone's first account can be promoted to the venue default. Renders a badge once set, nothing when the zone has no account. */
export const VenueDefaultControl: React.FC<{
  venue: string;
  refreshKey?: number;
  onChanged?: () => void;
}> = ({ venue, refreshKey = 0, onChanged }) => {
  const [account, setAccount] = useState<AccountDto | null>(null);
  const [busy, setBusy] = useState(false);

  const reload = useCallback(async () => {
    try {
      const list = await api.listAccounts(venue);
      setAccount(list[0] ?? null);
    } catch {
      setAccount(null);
    }
  }, [venue]);

  useEffect(() => {
    void reload();
  }, [reload, refreshKey]);

  const disabled = busy || !account || account.isDefault;

  return (
    <button
      type="button"
      disabled={disabled}
      onClick={() => {
        if (!account) return;
        setBusy(true);
        void api
          .setDefaultAccount(venue, account.id)
          .then(() => {
            void reload();
            onChanged?.();
          })
          .finally(() => setBusy(false));
      }}
      className="px-2 py-2 rounded-lg border border-surface-border text-xs font-mono font-semibold text-slate-500 hover:text-warn-strong hover:border-warn-border transition cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed disabled:hover:text-slate-500 disabled:hover:border-surface-border"
      title={
        !account
          ? "No account configured for this venue"
          : account.isDefault
            ? "This account is the venue default"
            : "Set this account as the venue default"
      }
    >
      Set As Default
    </button>
  );
};

/* ------------------------------------------------------------------ */
/* Account credentials delete — bin icon in the zone header            */
/* ------------------------------------------------------------------ */

/**
 * Permanently removes the credentials bound to the zone's account. The
 * account profile stays — only its vault secrets are deleted. Renders
 * nothing while the vault is locked or no credentials are stored.
 */
export const AccountCredentialsDelete: React.FC<{
  venue: string;
  refreshKey?: number;
  locked?: boolean;
  onChanged?: () => void;
}> = ({ venue, refreshKey = 0, locked = false, onChanged }) => {
  const [account, setAccount] = useState<AccountDto | null | undefined>(undefined);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api
      .listAccounts(venue)
      .then((list) => setAccount(list[0] ?? null))
      .catch(() => setAccount(null));
  }, [venue, refreshKey]);

  if (locked || !account?.credentialsConfigured) return null;

  return (
    <button
      type="button"
      disabled={busy}
      onClick={() => {
        if (
          !window.confirm(
            `Remove the stored credentials for "${account.label}"? The account stays; only its vault secrets are permanently deleted.`
          )
        ) {
          return;
        }
        setBusy(true);
        void api
          .deleteAccountCredentials(account.id)
          .then(() => onChanged?.())
          .catch((e) =>
            alert(`Failed to remove credentials: ${e instanceof Error ? e.message : String(e)}`)
          )
          .finally(() => setBusy(false));
      }}
      className="p-1.5 text-muted hover:text-loss disabled:opacity-40 transition cursor-pointer"
      title={`Remove stored credentials for ${account.label}`}
    >
      <Trash2 className="w-3.5 h-3.5" />
    </button>
  );
};

/* ------------------------------------------------------------------ */
/* Venue account enable toggle — sits in the zone header               */
/* ------------------------------------------------------------------ */

/**
 * Enable switch bound to the account itself — what the zone's single
 * header shows for the one account it represents. Writes the durable
 * `account.enabled` flag and mirrors it into the venue's engine-settings
 * flag so the per-venue admission check agrees. Before an account exists,
 * the switch drives the venue flag directly via `fallbackEnabled`.
 */
export const VenueAccountToggle: React.FC<{
  venue: string;
  refreshKey?: number;
  locked?: boolean;
  /** Engine flag shown/persisted while the venue has no account yet. */
  fallbackEnabled?: boolean;
  onFallbackChange?: (enabled: boolean) => void;
  onChanged?: () => void;
}> = ({ venue, refreshKey = 0, locked = false, fallbackEnabled = false, onFallbackChange, onChanged }) => {
  const [account, setAccount] = useState<AccountDto | null | undefined>(undefined);
  const [busy, setBusy] = useState(false);

  const reload = useCallback(async () => {
    try {
      const list = await api.listAccounts(venue);
      setAccount(list[0] ?? null);
    } catch {
      setAccount(null);
    }
  }, [venue]);

  useEffect(() => {
    void reload();
  }, [reload, refreshKey]);

  if (account === undefined) return null;

  const enabled = account ? account.enabled : fallbackEnabled;
  const toggleFields = ACCOUNT_CREDENTIAL_FIELDS[venue] ?? [];
  const toggleRequiredFilled =
    !account ||
    toggleFields.every((f) => f.optional || fieldIsSet(account, f.key));
  // Flag-less venues (long-tail perps, EVM AMMs) admit purely via an
  // enabled account — with no account there is nothing to switch, so the
  // toggle stays inert with an explanation rather than silently no-op.
  const flaglessNoAccount = !account && VENUE_SETTINGS_FLAG[venue] === undefined;
  const toggleDisabled =
    busy || locked || flaglessNoAccount || (!enabled && !toggleRequiredFilled);

  return (
    <ToggleSwitch
      size="sm"
      checked={enabled}
      disabled={toggleDisabled}
      ariaLabel={`${venueDisplay(venue)} enabled`}
      title={
        flaglessNoAccount
          ? "This venue is enabled by adding an account — it has no settings flag"
          : !enabled && !toggleRequiredFilled
            ? "Fill required credential fields to enable"
            : `${venueDisplay(venue)} enabled`
      }
      onChange={() => {
        const next = !enabled;
        persistVenueFlag(venue, next); // keep the venue flag in step
        if (!account) {
          onFallbackChange?.(next);
          return;
        }
        setBusy(true);
        void api
          .upsertAccount({
            id: account.id,
            label: account.label,
            enabled: next,
            makeDefault: account.isDefault,
            accountTier: account.accountTier,
            maxLeverage: account.maxLeverage ?? null,
          })
          .then(() => {
            void reload();
            onChanged?.();
          })
          .catch((e) =>
            alert(`Failed to update account: ${e instanceof Error ? e.message : String(e)}`)
          )
          .finally(() => setBusy(false));
      }}
    />
  );
};

/* ------------------------------------------------------------------ */
/* Strategy-level account routing                                      */
/* ------------------------------------------------------------------ */

export const STRATEGIES: { id: string; name: string; desc: string }[] = [
  { id: "futures", name: STRATEGY_DISPLAY_NAMES.futures, desc: "Same-ticker price dislocations across perp venues" },
  { id: "radar", name: STRATEGY_DISPLAY_NAMES.radar, desc: "Radar-dispatched spatial executions — shares futures venues, tracked separately" },
  { id: "funding", name: STRATEGY_DISPLAY_NAMES.funding, desc: "Long/short perp pairs capturing funding differentials" },
  { id: "spot", name: STRATEGY_DISPLAY_NAMES.spot, desc: "Spot basis vs. perp or spot venues" },
  { id: "triangle", name: STRATEGY_DISPLAY_NAMES.triangle, desc: "Sequential 3-leg currency cycle on a single venue" },
  { id: "prediction", name: STRATEGY_DISPLAY_NAMES.prediction, desc: "Binary market and cross-venue parity trades" },
  { id: "stat_arb", name: STRATEGY_DISPLAY_NAMES.stat_arb, desc: "Mean-reversion and basket spread strategies" },
];

interface RouteLegPickerProps {
  accounts: AccountDto[];
  venue: string;
  accountId: string;
  onVenue: (v: string) => void;
  onAccount: (id: string) => void;
  otherVenue?: string;
  disabled: boolean;
}

const RouteLegPicker: React.FC<RouteLegPickerProps> = ({
  accounts,
  venue,
  accountId,
  onVenue,
  onAccount,
  otherVenue,
  disabled,
}) => {
  const venueAccounts = accounts.filter((a) => a.venue === venue && a.enabled);
  // Only venues with at least one enabled account are routable
  const routableVenues = Object.keys(ACCOUNT_VENUE_DISPLAY).filter((v) =>
    accounts.some((a) => a.venue === v && a.enabled)
  );
  // Keep a currently-selected venue visible even if its accounts were since disabled
  const venueOptions =
    venue && !routableVenues.includes(venue) ? [...routableVenues, venue] : routableVenues;
  const selectClass =
    "w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-xs font-mono text-slate-800 focus:outline-none focus:ring-1 focus:ring-accent disabled:opacity-50 cursor-pointer";
  return (
    <div className="flex-1 min-w-0 grid grid-cols-2 gap-2">
      <select
        value={venue}
        disabled={disabled}
        onChange={(e) => onVenue(e.target.value)}
        className={selectClass}
      >
        <option value="">
          {routableVenues.length === 0 ? "No enabled accounts" : "Exchange..."}
        </option>
        {venueOptions
          .filter((v) => v !== otherVenue)
          .map((v) => (
            <option key={v} value={v}>
              {venueDisplay(v)}
            </option>
          ))}
      </select>
      <select
        value={accountId}
        disabled={disabled || !venue}
        onChange={(e) => onAccount(e.target.value)}
        className={selectClass}
      >
        <option value="">
          {venue
            ? venueAccounts.length === 0
              ? "No enabled accounts"
              : "Account..."
            : "Account..."}
        </option>
        {venueAccounts.map((a) => (
          <option key={a.id} value={a.id}>
            {a.label}
            {a.isDefault ? " (default)" : ""}
          </option>
        ))}
      </select>
    </div>
  );
};

export interface StrategyRouteCardProps {
  strategy: { id: string; name: string; desc: string };
  routes: StrategyAccountRouteDto[];
  accounts: AccountDto[];
  onChanged: () => void;
}

export const StrategyRouteCard: React.FC<StrategyRouteCardProps> = ({ strategy, routes, accounts, onChanged }) => {
  const [adding, setAdding] = useState(false);
  const [venueA, setVenueA] = useState("");
  const [acctA, setAcctA] = useState("");
  const [venueB, setVenueB] = useState("");
  const [acctB, setAcctB] = useState("");
  const [routeLabel, setRouteLabel] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const resetForm = () => {
    setAdding(false);
    setVenueA("");
    setAcctA("");
    setVenueB("");
    setAcctB("");
    setRouteLabel("");
    setError(null);
  };

  const handleAdd = async () => {
    setBusy(true);
    setError(null);
    try {
      if (!acctA || !acctB) throw new Error("Select an account on each exchange");
      await api.upsertStrategyRoute({
        strategyId: strategy.id,
        accountAId: acctA,
        accountBId: acctB,
        label: routeLabel.trim() || undefined,
        enabled: true,
      });
      resetForm();
      onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleToggle = async (r: StrategyAccountRouteDto) => {
    setBusy(true);
    setError(null);
    try {
      await api.upsertStrategyRoute({ id: r.id, strategyId: r.strategyId, accountAId: r.accountAId, accountBId: r.accountBId, enabled: !r.enabled });
      onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleDelete = async (r: StrategyAccountRouteDto) => {
    if (!window.confirm(`Delete route "${r.label}" for ${strategy.name}?`)) return;
    setBusy(true);
    setError(null);
    try {
      await api.deleteStrategyRoute(r.id);
      onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const accountName = (id: string, fallback: string) =>
    accounts.find((a) => a.id === id)?.label ?? fallback;

  return (
    <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
      <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
        <div>
          <h4 className="text-[13px] font-mono font-bold text-slate-900">{strategy.name}</h4>
          <p className="text-xs font-mono text-slate-500 mt-0.5">{strategy.desc}</p>
        </div>
        <button
          type="button"
          onClick={() => setAdding((v) => !v)}
          className="px-2 py-2 rounded-lg border border-accent-line/60 bg-blue-50/60 hover:bg-blue-100/70 text-accent-strong text-xs font-mono font-bold flex items-center gap-1 transition cursor-pointer shrink-0"
        >
          <Plus className="w-3 h-3" />
          <span>Add Route</span>
        </button>
      </div>

      {routes.length === 0 && !adding && (
        <div className="p-3 bg-surface-raised border border-dashed border-surface-border rounded-lg text-xs font-mono text-slate-500">
          No account routes. Every ticker using this strategy falls back to each exchange&apos;s
          <span className="font-bold"> default account</span>.
        </div>
      )}

      {routes.map((r) => (
        <div
          key={r.id}
          className="flex items-center justify-between gap-3 px-3 py-2 bg-surface-raised border border-surface-border rounded-lg"
        >
          <div className="flex items-center gap-2 min-w-0 text-xs font-mono">
            <span className="font-bold text-slate-800 truncate">{r.label}</span>
            <span className="text-slate-300">|</span>
            <span className="text-slate-600 truncate">
              {venueDisplay(r.accountAVenue)}:{accountName(r.accountAId, r.accountALabel)}
            </span>
            <ArrowLeftRight className="w-3 h-3 text-muted shrink-0" />
            <span className="text-slate-600 truncate">
              {venueDisplay(r.accountBVenue)}:{accountName(r.accountBId, r.accountBLabel)}
            </span>
          </div>
          <div className="flex items-center gap-2 shrink-0">
            <ToggleSwitch
              size="sm"
              checked={r.enabled}
              disabled={busy}
              onChange={() => void handleToggle(r)}
              ariaLabel={`Route ${r.label} enabled`}
              title={`Route ${r.label} enabled`}
            />
            <button
              type="button"
              disabled={busy}
              onClick={() => void handleDelete(r)}
              className="text-muted hover:text-loss disabled:opacity-40 transition cursor-pointer"
              title="Delete route"
            >
              <Trash2 className="w-3.5 h-3.5" />
            </button>
          </div>
        </div>
      ))}

      {adding && (
        <div className="p-3 bg-blue-50/40 border border-accent-line/50 rounded-lg space-y-2.5">
          <div className="flex items-center gap-2">
            <RouteLegPicker
              accounts={accounts}
              venue={venueA}
              accountId={acctA}
              otherVenue={venueB}
              disabled={busy}
              onVenue={(v) => {
                setVenueA(v);
                setAcctA("");
              }}
              onAccount={setAcctA}
            />
            <ArrowLeftRight className="w-3.5 h-3.5 text-muted shrink-0" />
            <RouteLegPicker
              accounts={accounts}
              venue={venueB}
              accountId={acctB}
              otherVenue={venueA}
              disabled={busy}
              onVenue={(v) => {
                setVenueB(v);
                setAcctB("");
              }}
              onAccount={setAcctB}
            />
          </div>
          <input
            type="text"
            value={routeLabel}
            disabled={busy}
            placeholder="Route label (optional)"
            onChange={(e) => setRouteLabel(e.target.value)}
            className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-lg text-xs font-mono text-slate-800 focus:outline-none focus:ring-1 focus:ring-accent"
          />
          {error && (
            <div className="text-xs font-mono text-loss flex items-center gap-1">
              <AlertCircle className="w-3 h-3" />
              <span>{error}</span>
            </div>
          )}
          <div className="flex justify-end gap-2">
            <button
              type="button"
              onClick={resetForm}
              disabled={busy}
              className="px-2 py-2 text-xs font-mono text-slate-500 hover:text-slate-700 cursor-pointer"
            >
              Cancel
            </button>
            <button
              type="button"
              disabled={busy || !acctA || !acctB}
              onClick={() => void handleAdd()}
              className="px-2 py-2 bg-accent hover:bg-blue-600 text-white text-xs font-mono font-bold rounded-lg transition disabled:opacity-40 cursor-pointer"
            >
              {busy ? "Adding..." : "Add Route"}
            </button>
          </div>
        </div>
      )}

      {error && !adding && (
        <div className="text-xs font-mono text-loss flex items-center gap-1">
          <AlertCircle className="w-3 h-3" />
          <span>{error}</span>
        </div>
      )}
    </div>
  );
};

export const StrategyRoutingPanel: React.FC = () => {
  const [routes, setRoutes] = useState<StrategyAccountRouteDto[] | null>(null);
  const [accounts, setAccounts] = useState<AccountDto[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      const [r, a] = await Promise.all([api.listStrategyRoutes(), api.listAccounts()]);
      setRoutes(r);
      setAccounts(a);
      setLoadError(null);
    } catch (e) {
      setLoadError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <p className="text-xs font-mono text-slate-500 max-w-2xl">
          Account routes bind an account on one exchange to an account on another for every ticker
          using the strategy. Tickers without a route fall back to each exchange&apos;s default account.
        </p>
        <button
          type="button"
          onClick={() => void reload()}
          className="p-1.5 text-muted hover:text-slate-600 transition cursor-pointer"
          title="Refresh routes"
        >
          <RefreshCw className="w-3.5 h-3.5" />
        </button>
      </div>

      {loadError && (
        <div className="p-3 bg-loss-dim border border-loss-border rounded-lg text-xs font-mono text-loss">
          {loadError}
        </div>
      )}

      {STRATEGIES.map((s) => (
        <StrategyRouteCard
          key={s.id}
          strategy={s}
          routes={(routes ?? []).filter((r) => r.strategyId === s.id)}
          accounts={accounts}
          onChanged={() => void reload()}
        />
      ))}
    </div>
  );
};
