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
  Terminal,
  Copy,
  Trash2,
} from "lucide-react";
import { api } from "../services/api";
import type {
  AccountCredentialInput,
  AccountDto,
  AgentCheckDto,
  ApproveTxDto,
  SignApproveDto,
  EngineSettingsDto,
  StrategyAccountRouteDto,
  VenueBalanceInfo,
  VenueLaneStatusDto,
} from "../types";
import { STRATEGY_DISPLAY_NAMES } from "../types/tickers";
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
      label: "Login Wallet Address (Aptos)",
      placeholder: "0x...",
      hint: "Your Aptos account / wallet address.",
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
  okx: [
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
};

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
  okx: "OKX",
  aster: "Aster",
  coinbase: "Coinbase",
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
}> = ({ typedKey, accountId, stored }) => {
  const [addr, setAddr] = useState<string | null>(null);
  const [invalid, setInvalid] = useState(false);
  const trimmed = typedKey.trim();
  const looksComplete = /^(0x)?[0-9a-fA-F]{64}$/.test(trimmed);

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

  if (!addr && !invalid) return null;
  return (
    <p
      className={`text-[10px] mt-1 leading-tight font-mono break-all ${
        invalid ? "text-amber-600" : "text-slate-500"
      }`}
    >
      {addr
        ? `${trimmed ? "signs as" : "stored key signs as"} ${addr}`
        : "not a valid EVM private key"}
    </p>
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
            <input
              type={f.secret && !shown ? "password" : f.numeric ? "number" : "text"}
              disabled={disabled}
              value={values[f.key] ?? ""}
              placeholder={placeholder}
              onChange={(e) => setValue(f.key, e.target.value)}
              autoComplete="off"
              className={`w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono disabled:opacity-50 ${f.secret ? "pr-9" : ""}`}
            />
            {f.secret && (
              <button
                type="button"
                onClick={() => toggleShow(f.key)}
                className="absolute right-2.5 top-1/2 -translate-y-1/2 text-slate-400 hover:text-slate-700 transition cursor-pointer disabled:opacity-40"
                disabled={disabled}
                title={shown ? "Hide" : "Show"}
                aria-label={shown ? `Hide ${f.label}` : `Show ${f.label}`}
              >
                {shown ? <EyeOff className="w-4 h-4" /> : <Eye className="w-4 h-4" />}
              </button>
            )}
          </div>
          {f.hint && (
            <p className="text-[10px] text-slate-400 mt-1 leading-tight">{f.hint}</p>
          )}
          {f.evmSigner && (
            <SignerAddressHint
              typedKey={values[f.key] ?? ""}
              accountId={account?.id ?? null}
              stored={stored}
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
          <div className="flex items-center justify-between text-slate-400">
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
          <div className="flex items-center justify-between text-slate-400">
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
          <div className="flex items-center justify-between text-slate-400">
            <span className="text-xs font-bold tracking-wider">Margin Rate</span>
            <span className="text-xs">Utilization</span>
          </div>
          <div className="text-xl font-bold text-slate-900 flex items-baseline space-x-1.5 tracking-tight font-mono tabular-nums">
            {bal?.margin_rate_pct != null ? (
              <>
                <span>{bal.margin_rate_pct.toFixed(1)}%</span>
                <span className="text-xs font-normal text-slate-400 font-sans">utilized</span>
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
        <div className="p-2.5 bg-warn-dim border border-warn-border rounded-lg text-xs font-mono text-warn flex items-start space-x-2">
          <AlertTriangle className="w-3.5 h-3.5 text-warn shrink-0 mt-0.5" />
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
            <KeyRound className="w-4 h-4 text-accent" /> PopDEX Agent Setup Toolkit
          </h3>
          <button type="button" onClick={onClose} className="text-slate-400 hover:text-slate-700 text-lg leading-none px-2" aria-label="Close">
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
            <span className="text-[10px] font-sans text-slate-400">free test USDT from the <button type="button" onClick={() => openDoc(POPDEX_TEST_FAUCET, setError)} className="text-slate-500 underline hover:text-slate-800 cursor-pointer">faucet</button></span>
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
          <p className="text-[10px] font-sans text-slate-400">
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
              <div className="text-[11px] font-mono break-all"><span className="text-slate-400">Address: </span><span className="text-slate-900">{agentAddr}</span></div>
              <div className="text-[11px] font-mono break-all"><span className="text-slate-400">Private key: </span><span className="text-loss font-bold">{agentKey.slice(0, 10)}…{agentKey.slice(-6)} (shown once)</span></div>
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
              {!keyUsed && <p className="text-[10px] font-sans text-warn">Not saved yet — press “Use this key”, then “Save Changes” on the card.</p>}
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
          {check?.approved && !keyUsed && <p className="text-[10px] font-sans text-warn">Approved on-chain — press “Use this key” in step 2, then “Save Changes”.</p>}
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
  // as filled. Empty-field venues (edgeX, long-tail paper venues) have no
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
            className="w-48 px-2 py-1 bg-surface-raised border border-surface-border rounded-lg text-xs font-mono text-slate-900 placeholder:text-slate-400 focus:outline-none focus:ring-1 focus:ring-accent"
          />
          {account.isDefault ? (
            <span className="px-1.5 py-0.5 rounded-md text-xs font-mono font-bold bg-warn-dim text-warn border border-warn-border tracking-wide shrink-0">
              Default Account
            </span>
          ) : (
            <button
              type="button"
              disabled={busy}
              onClick={() => void handleSetDefault()}
              className="px-2 py-2 rounded-lg border border-surface-border text-xs font-mono font-semibold text-slate-500 hover:text-warn hover:border-warn-border transition cursor-pointer shrink-0 disabled:opacity-40 disabled:cursor-not-allowed disabled:hover:text-slate-500 disabled:hover:border-surface-border"
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
            className="p-1.5 text-slate-400 hover:text-loss disabled:opacity-40 transition cursor-pointer"
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
          <span className="text-slate-400">Toggle switch above to enable</span>
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
              <span className="text-slate-400">
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
          <div className="text-xs font-mono text-slate-400">
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
              className="w-full mt-1 px-3 py-2 rounded-lg text-xs font-mono font-bold bg-accent-dim border border-accent-line text-accent hover:brightness-95 transition cursor-pointer flex items-center justify-center gap-1.5"
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
            className="w-48 px-2 py-1 bg-surface-raised border border-surface-border rounded-lg text-xs font-mono text-slate-900 placeholder:text-slate-400 focus:outline-none focus:ring-1 focus:ring-accent"
          />
          {!hasDefault && (
            <span className="px-1.5 py-0.5 rounded-md text-xs font-mono font-bold bg-warn-dim text-warn border border-warn-border tracking-wide shrink-0">
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
          {onFallbackChange && (
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
          <p className="text-xs font-mono text-warn">
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
              className="px-3 py-2 rounded-lg text-xs font-mono font-bold bg-accent-dim border border-accent-line text-accent hover:brightness-95 transition cursor-pointer flex items-center gap-1.5"
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
      className="w-full min-h-[118px] border border-dashed border-surface-border rounded-lg bg-transparent hover:bg-surface-raised/60 hover:border-accent-line flex flex-col items-center justify-center gap-1.5 text-slate-400 hover:text-accent transition cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
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
      className="w-40 px-2 py-1 bg-surface-raised border border-surface-border rounded-lg text-xs font-mono text-slate-900 placeholder:text-slate-400 focus:outline-none focus:ring-1 focus:ring-accent disabled:opacity-50"
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
      className="px-2 py-2 rounded-lg border border-surface-border text-xs font-mono font-semibold text-slate-500 hover:text-warn hover:border-warn-border transition cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed disabled:hover:text-slate-500 disabled:hover:border-surface-border"
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
      className="p-1.5 text-slate-400 hover:text-loss disabled:opacity-40 transition cursor-pointer"
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
  const toggleDisabled =
    busy || locked || (!enabled && !toggleRequiredFilled);

  return (
    <ToggleSwitch
      size="sm"
      checked={enabled}
      disabled={toggleDisabled}
      ariaLabel={`${venueDisplay(venue)} enabled`}
      title={
        !enabled && !toggleRequiredFilled
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
          className="px-2 py-2 rounded-lg border border-accent-line/60 bg-blue-50/60 hover:bg-blue-100/70 text-accent text-xs font-mono font-bold flex items-center gap-1 transition cursor-pointer shrink-0"
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
            <ArrowLeftRight className="w-3 h-3 text-slate-400 shrink-0" />
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
              className="text-slate-400 hover:text-loss disabled:opacity-40 transition cursor-pointer"
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
            <ArrowLeftRight className="w-3.5 h-3.5 text-slate-400 shrink-0" />
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
          className="p-1.5 text-slate-400 hover:text-slate-600 transition cursor-pointer"
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
