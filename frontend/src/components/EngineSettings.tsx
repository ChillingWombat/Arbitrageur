import React, { useState, useEffect } from "react";
import {
  AlertCircle,
  CheckCircle2,
  Key,
  Lock,
  Sliders,
  ExternalLink,
  Globe,
  AlertTriangle,
  Scale,
  RefreshCw,
  Info,
  Trash2,
  ArrowLeftRight,
} from "lucide-react";
import { api } from "../services/api";
import {
  normalizeRetentionDays,
  normalizeRetentionGb,
  retentionDaysToInput,
  retentionGbToInput,
} from "../services/retention";
import type { EngineSettingsDto, SystemStatusDto, VenueStatusDto, VaultStatus, VenueBalanceInfo, AccountDto, VenueLaneStatusDto, VenueCapabilityDto } from "../types";
import { VaultUnlockModal } from "./VaultUnlockModal";
import { InfoTooltip } from "./InfoTooltip";
import { ArbitragerLogo } from "./ArbitragerLogo";
import { ToggleSwitch } from "./ToggleSwitch";
import { VenueAccountsSection, StrategyRoutingPanel, AddAccountSlot, VenueDefaultControl, VenueAccountToggle, AccountCredentialsDelete, AccountLabelControl } from "./ExchangeAccounts";

/** Venue margin/metrics block rendered inside the venue's account zone. `bal` is null when no balance data exists (no API credentials / never synced) — values render as N/A rather than fabricated zeros. */
const VenueBalancePanel: React.FC<{
  bal: VenueBalanceInfo | null;
  lane?: VenueLaneStatusDto | null;
}> = ({ bal, lane }) => (
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
        <div className={`text-xl font-bold tracking-tight ${bal ? "text-emerald-700" : "text-slate-900"}`}>
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
        <div className="text-xl font-bold text-slate-900 flex items-baseline space-x-1.5 tracking-tight">
          {bal?.margin_rate_pct != null ? (
            <>
              <span>{bal.margin_rate_pct.toFixed(1)}%</span>
              <span className="text-xs font-normal text-slate-400">utilized</span>
            </>
          ) : (
            <span>N/A</span>
          )}
        </div>
        <div className="w-full bg-slate-200 h-1.5 rounded-full overflow-hidden mt-1.5">
          <div
            className={`h-full rounded-full transition-all duration-300 ${
              (bal?.margin_rate_pct ?? 0) > 80
                ? "bg-red-500"
                : (bal?.margin_rate_pct ?? 0) > 50
                ? "bg-amber-500"
                : "bg-accent"
            }`}
            style={{ width: `${Math.min(100, bal?.margin_rate_pct ?? 0)}%` }}
          />
        </div>
      </div>
    </div>

    <div className="text-xs font-mono text-slate-400 flex items-center space-x-1.5 flex-wrap gap-y-1">
      {lane ? (
        <>
          <span
            className={`px-1.5 py-0.5 rounded-sm border font-bold tracking-wider ${
              lane.backingOff
                ? "bg-rose-50 border-rose-200 text-rose-600"
                : "bg-slate-100 border-slate-200 text-slate-600"
            }`}
            title="Read requests used in the last minute vs the venue's per-minute budget"
          >
            {lane.usedLastMin}/{lane.readBudgetPerMin.toFixed(0)}/min{lane.backingOff ? " backoff" : ""}
          </span>
          <span>
            Rate budget · tier {lane.accountTier} · refresh {lane.refreshIntervalSecs.toFixed(0)}s · {lane.symbols} symbols
          </span>
        </>
      ) : (
        <span>No monitor lane — venue is not streaming market data.</span>
      )}
    </div>

    <div className="text-xs font-mono text-slate-400 flex items-center space-x-1.5">
      {bal ? (
        <>
          <span className="px-1.5 py-0.5 rounded-sm bg-slate-100 border border-slate-200 font-bold tracking-wider">
            Paper
          </span>
          <span>Simulated account balance — virtual funds from the paper-trading engine, not your real exchange balance.</span>
        </>
      ) : (
        <span>No paper account — paper execution is provisioned only for venues with a real market-data adapter (Hyperliquid, Lighter, Robinhood Lighter, Vanta).</span>
      )}
    </div>

    {bal?.recommended_action && (
      <div className="p-2.5 bg-amber-50 border border-amber-300 rounded-lg text-xs font-mono text-amber-900 flex items-start space-x-2">
        <AlertTriangle className="w-3.5 h-3.5 text-amber-700 shrink-0 mt-0.5" />
        <span>{bal.recommended_action}</span>
      </div>
    )}
  </div>
);

export type SettingsTab = "execution" | "exchanges" | "strategies" | "network" | "about";
export type ExchangeId =
  | "arcus"
  | "aster"
  | "binance"
  | "coinbase"
  | "decibel"
  | "edgex"
  | "grvt"
  | "hyperliquid"
  | "ibkr"
  | "ig"
  | "kalshi"
  | "kraken"
  | "lighter"
  | "okx"
  | "ondoperps"
  | "polymarket"
  | "popdex"
  | "robinhood"
  | "vanta"
  | "variational";

export interface ExchangeMetadata {
  id: ExchangeId;
  name: string;
  type: string;
  feeBps: number;
}

export const EXCHANGES_ALPHABETICAL: ExchangeMetadata[] = [
  { id: "arcus", name: "Arcus", type: "CLOB DEX", feeBps: 1.5 },
  { id: "aster", name: "Aster", type: "Multi-Chain Perp DEX", feeBps: 3.5 },
  { id: "binance", name: "Binance", type: "CEX Spot & Perp", feeBps: 7.5 },
  { id: "coinbase", name: "Coinbase", type: "CEX Spot & Perp", feeBps: 8.0 },
  { id: "decibel", name: "Decibel", type: "Aptos Perp DEX", feeBps: 2.5 },
  { id: "edgex", name: "edgeX", type: "StarkEx Perp DEX", feeBps: 4.5 },
  { id: "grvt", name: "GRVT", type: "Perp DEX", feeBps: 2.0 },
  { id: "hyperliquid", name: "Hyperliquid", type: "L1 Perp DEX", feeBps: 2.0 },
  { id: "ibkr", name: "Interactive Brokers", type: "TradFi Broker", feeBps: 1.0 },
  { id: "ig", name: "IG Trading", type: "TradFi Broker", feeBps: 5.0 },
  { id: "kalshi", name: "Kalshi", type: "CFTC Regulated", feeBps: 10.0 },
  { id: "kraken", name: "Kraken", type: "CEX Spot & Perp", feeBps: 16.0 },
  { id: "lighter", name: "Lighter", type: "zkDEX Rollup", feeBps: 1.5 },
  { id: "okx", name: "OKX", type: "CEX Spot & Perp", feeBps: 8.0 },
  { id: "ondoperps", name: "Ondo Perps", type: "Equity + Crypto Perps", feeBps: 3.5 },
  { id: "polymarket", name: "Polymarket", type: "CTF Binary Market", feeBps: 0.0 },
  { id: "popdex", name: "PopDEX", type: "Arbitrage Protocol", feeBps: 2.0 },
  { id: "robinhood", name: "Robinhood Lighter", type: "EVM DEX Aggregator", feeBps: 2.5 },
  { id: "vanta", name: "Vanta", type: "Omnichain Perps", feeBps: 3.0 },
  { id: "variational", name: "Variational", type: "Perp DEX (Read-Only)", feeBps: 0.0 },
];

interface EngineSettingsProps {
  systemStatus?: SystemStatusDto | null;
  onClose?: () => void;
  onSettingsSaved?: (newSettings: EngineSettingsDto) => void;
}

// Custom equality for React.memo: the component only reads
// systemStatus.active_venues, so skip re-rendering when the 3s status poll
// produces identical venue data (the common case — latency probes update
// only every 30s). Without this the whole settings tree re-renders on
// every poll and scrolling stutters.
function areEngineSettingsPropsEqual(p: EngineSettingsProps, n: EngineSettingsProps): boolean {
  if (p.onClose !== n.onClose || p.onSettingsSaved !== n.onSettingsSaved) return false;
  if (p.systemStatus === n.systemStatus) return true;
  return (
    JSON.stringify(p.systemStatus?.active_venues ?? null) ===
    JSON.stringify(n.systemStatus?.active_venues ?? null)
  );
}

export const EngineSettings = React.memo<EngineSettingsProps>(({ systemStatus, onClose, onSettingsSaved }) => {
  const [liveVenues, setLiveVenues] = useState<VenueStatusDto[]>([]);
  const [pinging, setPinging] = useState(false);

  // Remote HSM
  const [hsmUrl, setHsmUrl] = useState("http://127.0.0.1:8787/sign");
  const [hmacToken, setHmacToken] = useState("");

  // Guardrails & Execution Modes (slippage is per-strategy now: each bot
  // configurator carries its own max_slippage_bps gate)
  const [minDepthRatio, setMinDepthRatio] = useState(3.0);
  // Historical retention: empty string = unlimited (None on the backend)
  const [maxHistoryDays, setMaxHistoryDays] = useState("");
  const [maxStorageGb, setMaxStorageGb] = useState("");
  const [tradingMode, setTradingMode] = useState<"paper" | "real">("paper");
  const [paperStartingBalance, setPaperStartingBalance] = useState(100000);
  const [fastWarmupEnabled, setFastWarmupEnabled] = useState(true);
  // Networking & VPS
  const [engineRpcPort, setEngineRpcPort] = useState(8787);
  const [engineWsPort, setEngineWsPort] = useState(8789);
  const [bindAddress, setBindAddress] = useState("127.0.0.1");

  // Exchange Enablement Toggles
  const [hlEnabled, setHlEnabled] = useState(false);
  const [lighterEnabled, setLighterEnabled] = useState(false);
  const [vantaEnabled, setVantaEnabled] = useState(false);
  const [robinhoodEnabled, setRobinhoodEnabled] = useState(false);
  const [krakenEnabled, setKrakenEnabled] = useState(false);
  const [krakenSpotEnabled, setKrakenSpotEnabled] = useState(false);
  const [krakenFuturesEnabled, setKrakenFuturesEnabled] = useState(false);
  const [polymarketEnabled, setPolymarketEnabled] = useState(false);
  const [kalshiEnabled, setKalshiEnabled] = useState(false);

  // Hyperliquid
  const [hyperliquidWallet, setHyperliquidWallet] = useState("");
  const [hyperliquidPrivateKey, setHyperliquidPrivateKey] = useState("");
  const [hyperliquidUrl, setHyperliquidUrl] = useState("https://api.hyperliquid.xyz");
  const [hyperliquidAccountIndex, setHyperliquidAccountIndex] = useState<string>("");
  const [hyperliquidSubaccountIndex, setHyperliquidSubaccountIndex] = useState<string>("");

  // Lighter
  const [lighterAccountIndex, setLighterAccountIndex] = useState("0");
  const [lighterApiKey, setLighterApiKey] = useState("");
  const [lighterUrl, setLighterUrl] = useState("https://mainnet.zklighter.elliot.ai");
  const [lighterSubaccountIndex, setLighterSubaccountIndex] = useState<string>("");

  // Vanta (Orderly)
  const [vantaAccountId, setVantaAccountId] = useState("");
  const [vantaOrderlyKey, setVantaOrderlyKey] = useState("");
  const [vantaOrderlySecret, setVantaOrderlySecret] = useState("");
  const [vantaUrl, setVantaUrl] = useState("https://api.orderly.org");
  const [vantaSubaccountIndex, setVantaSubaccountIndex] = useState<string>("");

  // Robinhood EVM
  const [robinhoodWallet, setRobinhoodWallet] = useState("");
  const [robinhoodPrivateKey, setRobinhoodPrivateKey] = useState("");
  const [robinhoodRpcUrl, setRobinhoodRpcUrl] = useState("https://rpc.robinhood.network");
  const [robinhoodRouter, setRobinhoodRouter] = useState("0x89e5DB8B5aA49aA85AC63f691524311AEB649eba");
  const [robinhoodSubaccountIndex, setRobinhoodSubaccountIndex] = useState<string>("");

  // Arcus CLOB DEX
  const [arcusEnabled, setArcusEnabled] = useState(false);
  const [asterEnabled, setAsterEnabled] = useState(false);
  const [edgexEnabled, setEdgexEnabled] = useState(false);
  const [grvtEnabled, setGrvtEnabled] = useState(false);
  const [ondoperpsEnabled, setOndoperpsEnabled] = useState(false);
  const [binanceEnabled, setBinanceEnabled] = useState(false);
  const [coinbaseEnabled, setCoinbaseEnabled] = useState(false);
  const [decibelEnabled, setDecibelEnabled] = useState(false);
  const [ibkrEnabled, setIbkrEnabled] = useState(false);
  const [igEnabled, setIgEnabled] = useState(false);
  const [okxEnabled, setOkxEnabled] = useState(false);
  const [variationalEnabled, setVariationalEnabled] = useState(false);
  const [arcusApiKey, setArcusApiKey] = useState("");
  const [arcusSecret, setArcusSecret] = useState("");
  const [arcusUrl, setArcusUrl] = useState("https://api.arcus.exchange");

  // PopDEX (Arbitrage Protocol)
  const [popdexEnabled, setPopdexEnabled] = useState(false);
  const [popdexAccountIndex, setPopdexAccountIndex] = useState("0");
  const [popdexApiKey, setPopdexApiKey] = useState("");
  const [popdexUrl, setPopdexUrl] = useState("https://api.popdex.trade");

  // Kraken (Spot & Futures)
  const [krakenSpotApiKey, setKrakenSpotApiKey] = useState("");
  const [krakenSpotSecret, setKrakenSpotSecret] = useState("");
  const [krakenSpotUrl, setKrakenSpotUrl] = useState("https://api.kraken.com");
  const [krakenFuturesApiKey, setKrakenFuturesApiKey] = useState("");
  const [krakenFuturesSecret, setKrakenFuturesSecret] = useState("");
  const [krakenFuturesUrl, setKrakenFuturesUrl] = useState("https://futures.kraken.com");

  // Polymarket
  const [polymarketApiKey, setPolymarketApiKey] = useState("");
  const [polymarketSecret, setPolymarketSecret] = useState("");
  const [polymarketPassphrase, setPolymarketPassphrase] = useState("");
  const [polymarketUrl, setPolymarketUrl] = useState("https://clob.polymarket.com");

  // Kalshi
  const [kalshiApiKeyId, setKalshiApiKeyId] = useState("");
  const [kalshiPrivateKey, setKalshiPrivateKey] = useState("");
  const [kalshiUrl, setKalshiUrl] = useState("https://trading-api.kalshi.com");

  // Referrals, About & Links
  const [referralLighter, setReferralLighter] = useState("https://app.lighter.xyz/?referral=SOLO&source=none");
  const [referralRobinhood, setReferralRobinhood] = useState("https://robinhoodchain.lighter.xyz/?referral=SOLO&source=none");
  const [sponsorLink, setSponsorLink] = useState("https://buymeacoffee.com/dav1d1010");
  const [version, setVersion] = useState("v1.2.0");
  const [gitRepo, setGitRepo] = useState("https://github.com/arbitrager/arbitrager");

  // UI state
  const [activeTab, setActiveTab] = useState<SettingsTab>("execution");
  const [selectedExchange, setSelectedExchange] = useState<ExchangeId>("arcus");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Vault state
  const [vaultStatus, setVaultStatus] = useState<VaultStatus | null>(null);
  const [showVaultModal, setShowVaultModal] = useState(false);
  const [inlinePassword, setInlinePassword] = useState("");
  const [inlineConfirmPassword, setInlineConfirmPassword] = useState("");
  const [inlineUnlocking, setInlineUnlocking] = useState(false);
  const [inlineVaultError, setInlineVaultError] = useState<string | null>(null);
  const [configuredVenues, setConfiguredVenues] = useState<string[]>([]);

  // Venue execution capabilities & live trading risk gating
  const [venueCapabilities, setVenueCapabilities] = useState<VenueCapabilityDto[]>([]);
  const [showLiveRiskModal, setShowLiveRiskModal] = useState(false);
  const [liveRiskAgreed, setLiveRiskAgreed] = useState(false);
  const [liveRiskConfirmationInput, setLiveRiskConfirmationInput] = useState("");

  const handleInlineVaultSubmit = async () => {
    if (inlineUnlocking) return;
    setInlineVaultError(null);

    if (!vaultStatus) {
      setInlineVaultError("Vault status is still loading. Please try again.");
      return;
    }
    if (!inlinePassword) return;

    const isInitializing = !vaultStatus.is_initialized;
    if (isInitializing && Array.from(inlinePassword).length < 12) {
      setInlineVaultError("Password must be at least 12 characters long.");
      return;
    }
    if (isInitializing && inlinePassword !== inlineConfirmPassword) {
      setInlineVaultError("Passwords do not match. Please re-enter.");
      return;
    }

    setInlineUnlocking(true);
    try {
      const updatedStatus = isInitializing
        ? await api.initializeVault(inlinePassword)
        : await api.unlockVault(inlinePassword);
      setVaultStatus(updatedStatus);
      setInlinePassword("");
      setInlineConfirmPassword("");
      const sanitized = await api.getEngineSettings();
      setConfiguredVenues(sanitized.configured_venues || []);
    } catch (err) {
      setInlineVaultError(err instanceof Error ? err.message : "Vault operation failed");
    } finally {
      setInlineUnlocking(false);
    }
  };

  const handleInlineVaultKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key !== "Enter") return;
    e.preventDefault();
    e.stopPropagation();
    void handleInlineVaultSubmit();
  };

  // Venue Balances
  const [venueBalances, setVenueBalances] = useState<VenueBalanceInfo[]>([]);
  const [venueLanes, setVenueLanes] = useState<VenueLaneStatusDto[]>([]);
  const [accountsEpoch, setAccountsEpoch] = useState(0);
  // All account profiles — drives sidebar enable/configured badges so they
  // reflect account-level state, not just the legacy venue flags.
  const [accounts, setAccounts] = useState<AccountDto[]>([]);

  useEffect(() => {
    api.listAccounts().then(setAccounts).catch(() => setAccounts([]));
  }, [accountsEpoch]);


  // Update check
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [updateMsg, setUpdateMsg] = useState<string | null>(null);

  const venueFlagEnabled = (id: ExchangeId): boolean => {
    switch (id) {
      case "arcus": return arcusEnabled;
      case "aster": return asterEnabled;
      case "binance": return binanceEnabled;
      case "coinbase": return coinbaseEnabled;
      case "decibel": return decibelEnabled;
      case "edgex": return edgexEnabled;
      case "grvt": return grvtEnabled;
      case "hyperliquid": return hlEnabled;
      case "ibkr": return ibkrEnabled;
      case "ig": return igEnabled;
      case "kalshi": return kalshiEnabled;
      case "kraken": return krakenEnabled;
      case "lighter": return lighterEnabled;
      case "okx": return okxEnabled;
      case "ondoperps": return ondoperpsEnabled;
      case "polymarket": return polymarketEnabled;
      case "popdex": return popdexEnabled;
      case "robinhood": return robinhoodEnabled;
      case "vanta": return vantaEnabled;
      case "variational": return variationalEnabled;
      default: return false;
    }
  };

  // Kraken's two account zones are separate venues upstream of the sidebar id.
  const accountsForVenue = (id: string): AccountDto[] =>
    id === "kraken"
      ? accounts.filter((a) => a.venue === "kraken" || a.venue === "kraken_spot" || a.venue === "kraken_futures")
      : accounts.filter((a) => a.venue === id);

  // Same admission rule as the backend: venue flag OR any enabled account.
  const isExchangeEnabled = (id: ExchangeId): boolean =>
    venueFlagEnabled(id) || accountsForVenue(id).some((a) => a.enabled);

  const hasConfiguredAccount = (id: string): boolean =>
    accountsForVenue(id).some((a) => a.credentialsConfigured === true);

  const getVenueBalance = (id: string): VenueBalanceInfo | null => {
    return (
      venueBalances.find((b) => b.venue.toLowerCase() === id.toLowerCase()) ?? null
    );
  };

  // Spread-monitor rate lane for this venue (read budget usage), when streaming.
  const getVenueLane = (id: string): VenueLaneStatusDto | null => {
    return (
      venueLanes.find((l) => l.venue.toLowerCase() === id.toLowerCase()) ?? null
    );
  };

  // Measured round-trip to the venue API, from the live system status poll.
  const getVenueLatency = (id: string): number | null => {
    return (
      liveVenues.find((v) => v.id.toLowerCase() === id.toLowerCase())?.latency_ms ?? null
    );
  };

  // Backend admission status for this venue — OFFLINE (not enabled),
  // READ_ONLY (admitted, streaming market data) or UNSUPPORTED (enabled but
  // no market-data adapter; contributes nothing to ticker availability).
  const getVenueStatus = (id: string): string | null => {
    return (
      liveVenues.find((v) => v.id.toLowerCase() === id.toLowerCase())?.status ?? null
    );
  };

  const streamingCount = liveVenues.filter((v) => v.status === "READ_ONLY").length;

  const latencyClass = (ms: number): string =>
    ms < 100 ? "text-emerald-600" : ms < 300 ? "text-amber-500" : "text-red-500";

  const loadVaultStatus = async () => {
    try {
      const status = await api.getVaultStatus();
      setVaultStatus(status);
    } catch (err) {
      console.warn("Could not fetch vault status:", err);
    }
  };

  const loadVenueBalances = async () => {
    try {
      const data = await api.getVenueBalances();
      setVenueBalances(data);
    } catch (err) {
      console.error("Failed to load balances:", err);
    }
  };

  useEffect(() => {
    const loadSettings = async () => {
      try {
        const s: EngineSettingsDto = await api.getEngineSettings();
        setConfiguredVenues(s.configured_venues || []);
        api.getVenueExecutionCapabilities().then(setVenueCapabilities).catch(console.error);
        setHsmUrl(s.hsm_url || "http://127.0.0.1:8787/sign");
        setHmacToken(s.hmac_token || "");
        setMinDepthRatio(s.min_depth_ratio ?? 3.0);
        setMaxHistoryDays(retentionDaysToInput(s.spread_samples_max_days));
        setMaxStorageGb(retentionGbToInput(s.spread_samples_max_gb));

        setEngineRpcPort(s.engine_rpc_port ?? 8787);
        setEngineWsPort(s.engine_ws_port ?? 8789);
        setBindAddress(s.bind_address || "127.0.0.1");

        setTradingMode(s.trading_mode === "real" ? "real" : "paper");
        setPaperStartingBalance(s.paper_starting_balance_usd ?? 100000);
        setFastWarmupEnabled(s.fast_warmup_enabled ?? true);

        setHyperliquidWallet(s.hyperliquid_wallet || "");
        setHyperliquidPrivateKey(s.hyperliquid_private_key || "");
        setHyperliquidUrl(s.hyperliquid_url || "https://api.hyperliquid.xyz");
        setHyperliquidAccountIndex(s.hyperliquid_account_index?.toString() ?? "");
        setHyperliquidSubaccountIndex(s.hyperliquid_subaccount_index?.toString() ?? "");

        setLighterAccountIndex(s.lighter_account_index || "0");
        setLighterApiKey(s.lighter_api_key || "");
        setLighterUrl(s.lighter_url || "https://mainnet.zklighter.elliot.ai");
        setLighterSubaccountIndex(s.lighter_subaccount_index?.toString() ?? "");

        setVantaAccountId(s.vanta_account_id || "");
        setVantaOrderlyKey(s.vanta_orderly_key || "");
        setVantaOrderlySecret(s.vanta_orderly_secret || "");
        setVantaUrl(s.vanta_url || "https://api.orderly.org");
        setVantaSubaccountIndex(s.vanta_subaccount_index?.toString() ?? "");

        setRobinhoodWallet(s.robinhood_wallet || "");
        setRobinhoodPrivateKey(s.robinhood_private_key || "");
        setRobinhoodRpcUrl(s.robinhood_rpc_url || "https://rpc.robinhood.network");
        setRobinhoodRouter(s.robinhood_router || "0x89e5DB8B5aA49aA85AC63f691524311AEB649eba");
        setRobinhoodSubaccountIndex(s.robinhood_subaccount_index?.toString() ?? "");

        setPolymarketApiKey(s.polymarket_api_key || "");
        setPolymarketSecret(s.polymarket_secret || "");
        setPolymarketPassphrase(s.polymarket_passphrase || "");
        setPolymarketUrl(s.polymarket_url || "https://clob.polymarket.com");

        setKalshiApiKeyId(s.kalshi_api_key_id || "");
        setKalshiPrivateKey(s.kalshi_private_key || "");
        setKalshiUrl(s.kalshi_url || "https://trading-api.kalshi.com");

        setHlEnabled(s.hyperliquid_enabled ?? false);
        setLighterEnabled(s.lighter_enabled ?? false);
        setVantaEnabled(s.vanta_enabled ?? false);
        setRobinhoodEnabled(s.robinhood_enabled ?? false);
        setArcusEnabled(s.arcus_enabled ?? false);
        setPopdexEnabled(s.popdex_enabled ?? false);
        setKrakenEnabled(s.kraken_enabled ?? false);
        setKrakenSpotEnabled(s.kraken_spot_enabled ?? false);
        setKrakenFuturesEnabled(s.kraken_futures_enabled ?? false);
        setPolymarketEnabled(s.polymarket_enabled ?? false);
        setKalshiEnabled(s.kalshi_enabled ?? false);
        setDecibelEnabled(s.decibel_enabled ?? false);
        setIbkrEnabled(s.ibkr_enabled ?? false);
        setIgEnabled(s.ig_enabled ?? false);
        setBinanceEnabled(s.binance_enabled ?? false);
        setOkxEnabled(s.okx_enabled ?? false);
        setAsterEnabled(s.aster_enabled ?? false);
        setEdgexEnabled(s.edgex_enabled ?? false);
        setGrvtEnabled(s.grvt_enabled ?? false);
        setOndoperpsEnabled(s.ondoperps_enabled ?? false);
        setVariationalEnabled(s.variational_enabled ?? false);
        setCoinbaseEnabled(s.coinbase_enabled ?? false);

        setArcusApiKey(s.arcus_api_key || "");
        setArcusSecret(s.arcus_secret || "");
        setArcusUrl(s.arcus_url || "https://api.arcus.exchange");

        setPopdexAccountIndex(s.popdex_account_index || "0");
        setPopdexApiKey(s.popdex_api_key || "");
        setPopdexUrl(s.popdex_url || "https://api.popdex.trade");

        setKrakenSpotApiKey(s.kraken_spot_api_key || "");
        setKrakenSpotSecret(s.kraken_spot_secret || "");
        setKrakenSpotUrl(s.kraken_spot_url || "https://api.kraken.com");
        setKrakenFuturesApiKey(s.kraken_futures_api_key || "");
        setKrakenFuturesSecret(s.kraken_futures_secret || "");
        setKrakenFuturesUrl(s.kraken_futures_url || "https://futures.kraken.com");

        setReferralLighter(s.referral_lighter || "https://app.lighter.xyz/?referral=SOLO&source=none");
        setReferralRobinhood(s.referral_robinhood || "https://robinhoodchain.lighter.xyz/?referral=SOLO&source=none");
        setSponsorLink(s.sponsor_link || "https://buymeacoffee.com/dav1d1010");
        setVersion(s.version || "v1.2.0");
        setGitRepo(s.git_repo || "https://github.com/arbitrager/arbitrager");

        if (systemStatus?.active_venues && systemStatus.active_venues.length > 0) {
          setLiveVenues(systemStatus.active_venues);
        } else {
          api.getSystemStatus().then((sys) => {
            if (sys?.active_venues) setLiveVenues(sys.active_venues);
          }).catch(() => {});
        }
      } catch (err) {
        console.error("Failed to load engine settings:", err);
      } finally {
        setLoading(false);
      }
    };
    loadSettings();
    loadVaultStatus();
    loadVenueBalances();
  }, []);

  // Venue read-budget lanes stream from the spread monitor — poll lightly
  // while the settings page is mounted.
  useEffect(() => {
    let dead = false;
    const tick = async () => {
      try {
        const s = await api.getMonitorSnapshot();
        if (!dead && s) setVenueLanes(s.venues);
      } catch {
        /* monitor engine not running — lanes stay empty */
      }
    };
    tick();
    const id = setInterval(tick, 5000);
    return () => {
      dead = true;
      clearInterval(id);
    };
  }, []);

  useEffect(() => {
    if (systemStatus?.active_venues && systemStatus.active_venues.length > 0) {
      setLiveVenues(systemStatus.active_venues);
    }
  }, [systemStatus]);

  // A venue only counts as "API connected" when enabled and backed by a
  // working adapter (READ_ONLY). OFFLINE/UNSUPPORTED venues provide no data.
  const hasApiConnectivity = liveVenues.some((v) => v.status === "READ_ONLY");

  useEffect(() => {
    api.getVenueExecutionCapabilities().then(setVenueCapabilities).catch(console.error);
  }, [vaultStatus]);

  const liveReadyVenues = venueCapabilities.filter((c) => c.liveReady);
  const canSwitchToLive = (vaultStatus?.is_unlocked ?? false) && liveReadyVenues.length >= 2;

  const handleSave = async (e: React.FormEvent) => {
    e.preventDefault();
    setErrorMsg(null);
    setSaving(true);
    try {
      const payload: EngineSettingsDto = {
        hsm_url: hsmUrl,
        hmac_token: hmacToken,
        min_depth_ratio: minDepthRatio,
        engine_rpc_port: engineRpcPort,
        engine_ws_port: engineWsPort,
        bind_address: bindAddress,
        trading_mode: tradingMode,
        paper_starting_balance_usd: paperStartingBalance,
        fast_warmup_enabled: fastWarmupEnabled,
        spread_samples_max_days: normalizeRetentionDays(maxHistoryDays),
        spread_samples_max_gb: normalizeRetentionGb(maxStorageGb),
        configured_venues: configuredVenues,
        hyperliquid_enabled: hlEnabled,
        lighter_enabled: lighterEnabled,
        vanta_enabled: vantaEnabled,
        robinhood_enabled: robinhoodEnabled,
        arcus_enabled: arcusEnabled,
        popdex_enabled: popdexEnabled,
        kraken_enabled: krakenEnabled,
        kraken_spot_enabled: krakenSpotEnabled,
        kraken_futures_enabled: krakenFuturesEnabled,
        polymarket_enabled: polymarketEnabled,
        kalshi_enabled: kalshiEnabled,
        decibel_enabled: decibelEnabled,
        ibkr_enabled: ibkrEnabled,
        ig_enabled: igEnabled,
        binance_enabled: binanceEnabled,
        okx_enabled: okxEnabled,
        aster_enabled: asterEnabled,
        edgex_enabled: edgexEnabled,
        grvt_enabled: grvtEnabled,
        ondoperps_enabled: ondoperpsEnabled,
        variational_enabled: variationalEnabled,
        coinbase_enabled: coinbaseEnabled,
        arcus_api_key: arcusApiKey.trim(),
        arcus_secret: arcusSecret.trim(),
        arcus_url: arcusUrl.trim(),
        popdex_account_index: popdexAccountIndex.trim(),
        popdex_api_key: popdexApiKey.trim(),
        popdex_url: popdexUrl.trim(),
        kraken_spot_api_key: krakenSpotApiKey.trim(),
        kraken_spot_secret: krakenSpotSecret.trim(),
        kraken_spot_url: krakenSpotUrl.trim(),
        kraken_futures_api_key: krakenFuturesApiKey.trim(),
        kraken_futures_secret: krakenFuturesSecret.trim(),
        kraken_futures_url: krakenFuturesUrl.trim(),
        hyperliquid_wallet: hyperliquidWallet.trim(),
        hyperliquid_private_key: hyperliquidPrivateKey.trim(),
        hyperliquid_url: hyperliquidUrl.trim(),
        hyperliquid_account_index: hyperliquidAccountIndex ? parseInt(hyperliquidAccountIndex, 10) : undefined,
        hyperliquid_subaccount_index: hyperliquidSubaccountIndex ? parseInt(hyperliquidSubaccountIndex, 10) : undefined,
        lighter_account_index: lighterAccountIndex.trim(),
        lighter_api_key: lighterApiKey.trim(),
        lighter_url: lighterUrl.trim(),
        lighter_subaccount_index: lighterSubaccountIndex ? parseInt(lighterSubaccountIndex, 10) : undefined,
        vanta_account_id: vantaAccountId.trim(),
        vanta_orderly_key: vantaOrderlyKey.trim(),
        vanta_orderly_secret: vantaOrderlySecret.trim(),
        vanta_url: vantaUrl.trim(),
        vanta_subaccount_index: vantaSubaccountIndex ? parseInt(vantaSubaccountIndex, 10) : undefined,
        robinhood_wallet: robinhoodWallet.trim(),
        robinhood_private_key: robinhoodPrivateKey.trim(),
        robinhood_rpc_url: robinhoodRpcUrl.trim(),
        robinhood_router: robinhoodRouter.trim(),
        robinhood_subaccount_index: robinhoodSubaccountIndex ? parseInt(robinhoodSubaccountIndex, 10) : undefined,
        polymarket_api_key: polymarketApiKey.trim(),
        polymarket_secret: polymarketSecret.trim(),
        polymarket_passphrase: polymarketPassphrase.trim(),
        polymarket_url: polymarketUrl.trim(),
        kalshi_api_key_id: kalshiApiKeyId.trim(),
        kalshi_private_key: kalshiPrivateKey.trim(),
        kalshi_url: kalshiUrl.trim(),
        referral_lighter: referralLighter,
        referral_robinhood: referralRobinhood,
        sponsor_link: sponsorLink,
        version: version,
        git_repo: gitRepo,
      };

      await api.saveEngineSettings(payload);

      // Live execution is disabled until authenticated adapters exist; always report paper mode
      await api.setLiveMode(false);

      const sanitized = await api.getEngineSettings();
      setConfiguredVenues(sanitized.configured_venues || []);
      clearCredentialInputs();

      onSettingsSaved?.(sanitized);

      setSaved(true);
      setTimeout(() => setSaved(false), 3000);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      setErrorMsg(msg);
    } finally {
      setSaving(false);
    }
  };

  const clearCredentialInputs = () => {
    setHmacToken("");
    setHyperliquidWallet("");
    setHyperliquidPrivateKey("");
    setHyperliquidAccountIndex("");
    setHyperliquidSubaccountIndex("");
    setLighterAccountIndex("0");
    setLighterApiKey("");
    setLighterSubaccountIndex("");
    setVantaAccountId("");
    setVantaOrderlyKey("");
    setVantaOrderlySecret("");
    setVantaSubaccountIndex("");
    setRobinhoodWallet("");
    setRobinhoodPrivateKey("");
    setRobinhoodSubaccountIndex("");
    setArcusApiKey("");
    setArcusSecret("");
    setPopdexAccountIndex("0");
    setPopdexApiKey("");
    setKrakenSpotApiKey("");
    setKrakenSpotSecret("");
    setKrakenFuturesApiKey("");
    setKrakenFuturesSecret("");
    setPolymarketApiKey("");
    setPolymarketSecret("");
    setPolymarketPassphrase("");
    setKalshiApiKeyId("");
    setKalshiPrivateKey("");
    setInlinePassword("");
    setInlineConfirmPassword("");
  };

  const handleLockVault = async () => {
    try {
      const s = await api.lockVault();
      setVaultStatus(s);
      setConfiguredVenues([]);
      clearCredentialInputs();
    } catch (err) {
      console.error("Lock error:", err);
    }
  };

  /** Reloads account lists and venue credential-presence after any account-level mutation (toggle, default, credential delete). */
  const handleAccountsChanged = () => {
    setAccountsEpoch((e) => e + 1);
    void api
      .getEngineSettings()
      .then((s) => setConfiguredVenues(s.configured_venues || []))
      .catch(() => undefined);
  };

  const handleRemoveHsmToken = async () => {
    if (!configuredVenues.includes("remote_hsm")) return;
    if (
      !window.confirm("Remove the encrypted HSM token? This cannot be undone.")
    ) {
      return;
    }
    setErrorMsg(null);
    try {
      await api.deleteExchangeCredentials("remote_hsm");
      const sanitized = await api.getEngineSettings();
      setConfiguredVenues(sanitized.configured_venues || []);
      clearCredentialInputs();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleCheckForUpdates = () => {
    setCheckingUpdate(true);
    setUpdateMsg(null);
    setTimeout(() => {
      setCheckingUpdate(false);
      setUpdateMsg("System up to date. Arbitrager v1.2.0 is currently the latest stable release.");
    }, 1200);
  };

  const isVenueConfigured = (venue: string): boolean => {
    const configured = (id: string) => configuredVenues.includes(id);
    switch (venue) {
      case "hyperliquid":
        return isExchangeEnabled("hyperliquid") && (configured("hyperliquid") || Boolean(hyperliquidWallet || hyperliquidPrivateKey) || hasConfiguredAccount("hyperliquid"));
      case "lighter":
        return isExchangeEnabled("lighter") && (configured("lighter") || Boolean(lighterApiKey) || hasConfiguredAccount("lighter"));
      case "vanta":
        return isExchangeEnabled("vanta") && (configured("vanta") || Boolean(vantaOrderlyKey || vantaAccountId) || hasConfiguredAccount("vanta"));
      case "arcus":
        return isExchangeEnabled("arcus") && (configured("arcus") || Boolean(arcusApiKey) || hasConfiguredAccount("arcus"));
      case "popdex":
        return isExchangeEnabled("popdex") && (configured("popdex") || Boolean(popdexApiKey || (popdexAccountIndex && popdexAccountIndex !== "0")) || hasConfiguredAccount("popdex"));
      case "robinhood":
        return isExchangeEnabled("robinhood") && (configured("robinhood") || Boolean(robinhoodWallet || robinhoodPrivateKey) || hasConfiguredAccount("robinhood"));
      case "kraken":
        return isExchangeEnabled("kraken") && (
          configured("kraken_spot") || configured("kraken_futures") ||
          (krakenSpotEnabled && Boolean(krakenSpotApiKey)) ||
          (krakenFuturesEnabled && Boolean(krakenFuturesApiKey)) ||
          hasConfiguredAccount("kraken")
        );
      case "polymarket":
        return isExchangeEnabled("polymarket") && (configured("polymarket") || Boolean(polymarketApiKey || polymarketSecret) || hasConfiguredAccount("polymarket"));
      case "kalshi":
        return isExchangeEnabled("kalshi") && (configured("kalshi") || Boolean(kalshiApiKeyId || kalshiPrivateKey) || hasConfiguredAccount("kalshi"));
      default:
        return false;
    }
  };

  const configuredCount = [
    isVenueConfigured("hyperliquid"),
    isVenueConfigured("lighter"),
    isVenueConfigured("vanta"),
    isVenueConfigured("arcus"),
    isVenueConfigured("popdex"),
    isVenueConfigured("robinhood"),
    isVenueConfigured("kraken"),
    isVenueConfigured("polymarket"),
    isVenueConfigured("kalshi"),
  ].filter(Boolean).length;

  const activeVenuesCount = (
    ["hyperliquid", "lighter", "vanta", "arcus", "popdex", "robinhood", "kraken", "polymarket", "kalshi"] as ExchangeId[]
  ).filter((id) => isExchangeEnabled(id)).length;

  const isVaultLocked = !api.isTauri || !vaultStatus?.is_unlocked;
  const isVaultInitializing = vaultStatus?.is_initialized === false;
  const isVaultStatusLoading = api.isTauri && vaultStatus === null;

  return (
    <div className="flex flex-col h-full min-h-0 overflow-hidden select-none bg-surface">
      {/* Top Section Navigation Bar */}
      <div className="border-b border-surface-border bg-slate-50/80 shrink-0 flex items-stretch justify-between select-none">
        {/* Navigation Tabs with dividers: Execution, Exchanges, Network & VPS, About */}
        <div className="flex items-stretch divide-x divide-surface-border border-r border-surface-border">
          <button
            type="button"
            onClick={() => setActiveTab("execution")}
            className={`relative h-10 flex items-center space-x-2 px-4 sm:px-5 text-[11px] font-mono transition select-none group cursor-pointer ${
              activeTab === "execution"
                ? "text-accent font-bold bg-surface"
                : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
            }`}
          >
            <Sliders
              className={`w-3 h-3 transition shrink-0 ${
                activeTab === "execution" ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
              }`}
            />
            <span>Execution</span>
            {activeTab === "execution" && (
              <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
            )}
          </button>

          <button
            type="button"
            onClick={() => setActiveTab("exchanges")}
            className={`relative h-10 flex items-center space-x-2 px-4 sm:px-5 text-[11px] font-mono transition select-none group cursor-pointer ${
              activeTab === "exchanges"
                ? "text-accent font-bold bg-surface"
                : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
            }`}
          >
            <Key
              className={`w-3 h-3 transition shrink-0 ${
                activeTab === "exchanges" ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
              }`}
            />
            <span>Exchanges</span>
            <span
              className={`px-1.5 py-0.2 rounded-sm text-[11px] font-bold font-mono min-w-[16px] text-center leading-tight transition ${
                activeTab === "exchanges"
                  ? "bg-accent text-white shadow-2xs"
                  : "bg-accent-dim text-accent border border-accent-line/60"
              }`}
            >
              {activeVenuesCount}
            </span>
            {activeTab === "exchanges" && (
              <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
            )}
          </button>

          <button
            type="button"
            onClick={() => setActiveTab("strategies")}
            className={`relative h-10 flex items-center space-x-2 px-4 sm:px-5 text-[11px] font-mono transition select-none group cursor-pointer ${
              activeTab === "strategies"
                ? "text-accent font-bold bg-surface"
                : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
            }`}
          >
            <ArrowLeftRight
              className={`w-3 h-3 transition shrink-0 ${
                activeTab === "strategies" ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
              }`}
            />
            <span>Strategies</span>
            {activeTab === "strategies" && (
              <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
            )}
          </button>

          <button
            type="button"
            onClick={() => setActiveTab("network")}
            className={`relative h-10 flex items-center space-x-2 px-4 sm:px-5 text-[11px] font-mono transition select-none group cursor-pointer ${
              activeTab === "network"
                ? "text-accent font-bold bg-surface"
                : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
            }`}
          >
            <Globe
              className={`w-3 h-3 transition shrink-0 ${
                activeTab === "network" ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
              }`}
            />
            <span>Network &amp; VPS</span>
            {activeTab === "network" && (
              <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
            )}
          </button>

          <button
            type="button"
            onClick={() => setActiveTab("about")}
            className={`relative h-10 flex items-center space-x-2 px-4 sm:px-5 text-[11px] font-mono transition select-none group cursor-pointer ${
              activeTab === "about"
                ? "text-accent font-bold bg-surface"
                : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
            }`}
          >
            <Info
              className={`w-3 h-3 transition shrink-0 ${
                activeTab === "about" ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
              }`}
            />
            <span>About</span>
            {activeTab === "about" && (
              <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
            )}
          </button>
        </div>

        {/* Right Header: Lock Vault action when unlocked on Exchanges Tab */}
        <div className="flex items-center space-x-2 py-2 pr-5 sm:pr-6">
          {activeTab === "exchanges" && !isVaultLocked && (
            <>
              <button
                type="button"
                onClick={handleLockVault}
                className="px-2.5 py-1 rounded-md border border-slate-200 bg-surface hover:bg-slate-100 text-slate-600 text-xs font-mono flex items-center space-x-1.5 transition shadow-2xs cursor-pointer"
                title="Lock sensitive exchange credentials now"
              >
                <Lock className="w-3.5 h-3.5 text-slate-500" />
                <span>Lock Vault</span>
              </button>
            </>
          )}
          {activeTab === "network" &&
            !isVaultLocked &&
            configuredVenues.includes("remote_hsm") && (
              <button
                type="button"
                onClick={handleRemoveHsmToken}
                className="px-2.5 py-1 rounded-md border border-red-200 bg-surface hover:bg-red-50 text-red-600 text-xs font-mono flex items-center space-x-1.5 transition shadow-2xs cursor-pointer"
                title="Permanently remove the stored HSM token from the vault"
              >
                <Trash2 className="w-3.5 h-3.5 text-red-500" />
                <span>Remove HSM Token</span>
              </button>
            )}
        </div>
      </div>

      {/* Main Content Form Area */}
      <div className="flex-1 flex flex-col min-w-0 h-full overflow-hidden bg-surface">
        <form onSubmit={handleSave} className="flex flex-col h-full min-h-0">
          {/* Active Tab Subheader */}
          <div className="px-6 py-3 border-b border-surface-border bg-surface shrink-0 flex items-center justify-between">
            <div>
              <h3 className="text-[13px] font-bold font-mono tracking-wider text-slate-900 flex items-center space-x-1.5">
                <span>
                  {activeTab === "execution" && "Execution Modes & Safety Guardrails"}
                  {activeTab === "exchanges" && "Exchange Accounts, API Credentials & Margins"}
                  {activeTab === "strategies" && "Strategy Account Routing"}
                  {activeTab === "network" && "Network Ports & HSM Signer Configuration"}
                  {activeTab === "about" && "About Arbitrager & Release Information"}
                </span>
                <InfoTooltip
                  text={
                    activeTab === "execution"
                      ? "Calibrate the minimum depth ratio and paper/live trading modes. Slippage tolerances live in each strategy's configurator."
                      : activeTab === "exchanges"
                      ? "Manage per-exchange accounts: credentials in the encrypted vault, enable switches, and default selection."
                      : activeTab === "strategies"
                      ? "Bind which account on one exchange trades against which account on another for each strategy. Applies to all tickers using the strategy."
                      : activeTab === "network"
                      ? "Manage local daemon RPC/WS ports and remote Hardware Security Module tokens."
                      : "Version status, open-source repository, sponsorship, and compliance notices."
                  }
                />
              </h3>
            </div>
          </div>


          {activeTab === "exchanges" ? (
            <div className="relative flex-1 flex flex-row min-h-0 overflow-hidden">
              {/* Left Panel: Exchanges Selector in Alphabetical Order */}
              <div className="w-56 sm:w-64 shrink-0 border-r border-surface-border bg-slate-50/50 flex flex-col min-h-0">
                <div className="p-3 border-b border-surface-border bg-slate-100/60 flex items-center justify-between shrink-0">
                  <span className="text-xs font-mono font-bold text-slate-700 tracking-wider">
                    Exchanges ({EXCHANGES_ALPHABETICAL.length})
                  </span>
                  <button
                    type="button"
                    onClick={async () => {
                      setPinging(true);
                      try {
                        const status = await api.refreshVenueLatencies();
                        if (status.active_venues) setLiveVenues(status.active_venues);
                      } catch (err: unknown) {
                        setErrorMsg(err instanceof Error ? err.message : String(err));
                      } finally {
                        setPinging(false);
                      }
                    }}
                    disabled={pinging}
                    title="Measure HTTP round-trip to all enabled venue API endpoints now"
                    className="px-2 py-1 bg-surface hover:bg-surface-raised border border-surface-border text-slate-600 hover:text-slate-900 rounded-md text-[10px] font-mono transition flex items-center space-x-1 disabled:opacity-50"
                  >
                    <RefreshCw className={`w-3 h-3 ${pinging ? "animate-spin" : ""}`} />
                    <span>Ping</span>
                  </button>
                </div>

                <div className="flex-1 overflow-y-auto p-2 space-y-1">
                  {EXCHANGES_ALPHABETICAL.map((ex) => {
                    const isSelected = selectedExchange === ex.id;
                    const isEnabled = isExchangeEnabled(ex.id);
                    const bal = getVenueBalance(ex.id);
                    const latency = getVenueLatency(ex.id);
                    // Backend admission status is authoritative — an enabled
                    // venue without an adapter shows an amber dot, not a
                    // green "connected" dot.
                    const vStatus = getVenueStatus(ex.id);
                    const isStreaming = vStatus === "READ_ONLY";
                    const noAdapter = vStatus === "UNSUPPORTED";
                    const statusLabel = !isEnabled
                      ? "Off"
                      : latency != null
                      ? `${latency} ms`
                      : "No response";
                    return (
                      <button
                        key={ex.id}
                        type="button"
                        onClick={() => setSelectedExchange(ex.id)}
                        className={`w-full text-left p-2.5 rounded-lg text-xs font-mono transition flex items-center justify-between group cursor-pointer border ${
                          isSelected
                            ? "bg-accent text-white border-accent shadow-2xs"
                            : "bg-surface hover:bg-slate-100/80 text-slate-700 border-surface-border/60 hover:border-surface-border"
                        }`}
                      >
                        <div className="min-w-0 pr-2">
                          <div className="flex items-center space-x-2">
                            <span
                              className={`w-2 h-2 rounded-full shrink-0 ${
                                isSelected
                                  ? isStreaming || noAdapter
                                    ? "bg-white ring-2 ring-white/30"
                                    : "bg-white/40"
                                  : isStreaming
                                  ? "bg-emerald-500"
                                  : noAdapter
                                  ? "bg-amber-400"
                                  : "bg-slate-300"
                              }`}
                              title={
                                isStreaming
                                  ? "Market quotes are integrated and active"
                                  : noAdapter
                                  ? "Enabled, but market quotes are not integrated"
                                  : vStatus === "OFFLINE"
                                  ? "Not enabled"
                                  : undefined
                              }
                            />
                            <span className={`font-semibold truncate ${isSelected ? "text-white font-bold" : "text-slate-900"}`}>
                              {ex.name}
                            </span>
                          </div>
                        </div>
                        <div className="text-right shrink-0">
                          <span
                            className={`text-[11px] font-mono block font-semibold ${
                              isSelected
                                ? "text-blue-100"
                                : noAdapter
                                ? "text-amber-500"
                                : statusLabel === "Off" || statusLabel === "No response"
                                ? "text-slate-400"
                                : latency != null
                                ? latencyClass(latency)
                                : "text-slate-400"
                            }`}
                            title={
                              !isEnabled
                                ? "Not enabled"
                                : isStreaming
                                ? latency != null
                                  ? `Market data active · measured API round-trip: ${latency} ms`
                                  : "Market data active"
                                : noAdapter
                                ? latency != null
                                  ? `API reachable (${latency} ms), but market quotes are not integrated`
                                  : "Market quotes are not integrated and the API probe returned no response"
                                : undefined
                            }
                          >
                            {statusLabel}
                          </span>
                          <span
                            className={`text-[11px] font-mono block font-bold ${isSelected ? "text-white" : "text-slate-800"}`}
                            title={
                              bal
                                ? "Simulated paper equity, not live exchange equity"
                                : "No paper execution account for this venue"
                            }
                          >
                            {bal
                              ? `Paper $${bal.equity_usd.toLocaleString("en-US", { maximumFractionDigits: 0 })}`
                              : "No paper"}
                          </span>
                        </div>
                      </button>
                    );
                  })}
                </div>

                {/* Left Panel Summary */}
                <div className="p-3 border-t border-surface-border bg-slate-100/60 text-xs font-mono text-slate-500 flex items-center justify-between shrink-0">
                  <span>Streaming: <strong className="text-emerald-600 font-bold">{streamingCount}</strong></span>
                  <span>Active: <strong className="text-slate-800 font-bold">{activeVenuesCount}</strong>/{EXCHANGES_ALPHABETICAL.length}</span>
                  <span>Configured: <strong className="text-slate-800 font-bold">{configuredCount}</strong>/{EXCHANGES_ALPHABETICAL.length}</span>
                </div>
              </div>

              {/* Right Content Pane: Single Selected Exchange */}
              <div className="flex-1 overflow-y-auto p-4 space-y-4 min-w-0">
                {errorMsg && (
                  <div className="p-4 bg-red-50 border border-red-200 text-red-700 rounded-lg text-xs font-mono flex items-center space-x-2">
                    <AlertCircle className="w-4 h-4 shrink-0" />
                    <span>{errorMsg}</span>
                  </div>
                )}

                {saved && (
                  <div className="p-4 bg-emerald-50 border border-emerald-200 text-emerald-700 rounded-lg text-xs font-mono flex items-center space-x-2 animate-in fade-in duration-200">
                    <CheckCircle2 className="w-4 h-4 shrink-0" />
                    <span>Configuration saved successfully and updated in live atomic execution coordinator.</span>
                  </div>
                )}


            {/* Exchange: Arcus DEX */}
            {selectedExchange === "arcus" && (
              <div className="space-y-4">
              <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-[13px] font-mono font-bold text-slate-900">
                      Arcus #1
                    </h3>
                    <AccountLabelControl venue="arcus" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                  </div>
                  <div className="flex items-center space-x-3">
                    <AccountCredentialsDelete
                      venue="arcus"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      onChanged={handleAccountsChanged}
                    />
                    <VenueDefaultControl venue="arcus" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                    <VenueAccountToggle
                      venue="arcus"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      fallbackEnabled={arcusEnabled}
                      onFallbackChange={setArcusEnabled}
                      onChanged={() => setAccountsEpoch((e) => e + 1)}
                    />
                  </div>
                </div>

                <VenueBalancePanel
                  bal={getVenueBalance("arcus")}
                  lane={getVenueLane("arcus")}
                />                  <VenueAccountsSection
                    venue="arcus"
                    locked={isVaultLocked}
                    refreshKey={accountsEpoch}
                    onChanged={() => setAccountsEpoch((e) => e + 1)}
                  />
              </div>

              <AddAccountSlot
                venue="arcus"
                locked={isVaultLocked}
                onChanged={() => setAccountsEpoch((e) => e + 1)}
              />
              </div>
            )}



            {/* Exchange 1: Hyperliquid */}
            {selectedExchange === "hyperliquid" && (
              <div className="space-y-4">
              <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-[13px] font-mono font-bold text-slate-900">
                      Hyperliquid #1
                    </h3>
                    <AccountLabelControl venue="hyperliquid" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                  </div>
                  <div className="flex items-center space-x-3">
                    <AccountCredentialsDelete
                      venue="hyperliquid"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      onChanged={handleAccountsChanged}
                    />
                    <VenueDefaultControl venue="hyperliquid" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                    <VenueAccountToggle
                      venue="hyperliquid"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      fallbackEnabled={hlEnabled}
                      onFallbackChange={setHlEnabled}
                      onChanged={() => setAccountsEpoch((e) => e + 1)}
                    />
                  </div>
                </div>

                <VenueBalancePanel
                  bal={getVenueBalance("hyperliquid")}
                  lane={getVenueLane("hyperliquid")}
                />

                <div className="space-y-3">
                  <VenueAccountsSection
                    venue="hyperliquid"
                    locked={isVaultLocked}
                    refreshKey={accountsEpoch}
                    onChanged={() => setAccountsEpoch((e) => e + 1)}
                  />
                </div>
              </div>

              <AddAccountSlot
                venue="hyperliquid"
                locked={isVaultLocked}
                onChanged={() => setAccountsEpoch((e) => e + 1)}
              />
              </div>
            )}



            {/* Exchange 7: Kalshi */}
            {selectedExchange === "kalshi" && (
              <div className="space-y-4">
              <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-[13px] font-mono font-bold text-slate-900">
                      Kalshi #1
                    </h3>
                    <AccountLabelControl venue="kalshi" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                  </div>
                  <div className="flex items-center space-x-3">
                    <AccountCredentialsDelete
                      venue="kalshi"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      onChanged={handleAccountsChanged}
                    />
                    <VenueDefaultControl venue="kalshi" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                    <VenueAccountToggle
                      venue="kalshi"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      fallbackEnabled={kalshiEnabled}
                      onFallbackChange={setKalshiEnabled}
                      onChanged={() => setAccountsEpoch((e) => e + 1)}
                    />
                  </div>
                </div>

                <VenueBalancePanel
                  bal={getVenueBalance("kalshi")}
                  lane={getVenueLane("kalshi")}
                />                  <VenueAccountsSection
                    venue="kalshi"
                    locked={isVaultLocked}
                    refreshKey={accountsEpoch}
                    onChanged={() => setAccountsEpoch((e) => e + 1)}
                  />
              </div>

              <AddAccountSlot
                venue="kalshi"
                locked={isVaultLocked}
                onChanged={() => setAccountsEpoch((e) => e + 1)}
              />
              </div>
            )}


            {/* Exchange 5: Kraken (Spot & Futures Multi-Market) */}
            {selectedExchange === "kraken" && (
              <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-[13px] font-mono font-bold text-slate-900">
                      Kraken #1
                    </h3>
                  </div>
                  <div className="flex items-center space-x-3">
                    <ToggleSwitch
                      checked={krakenEnabled}
                      onChange={setKrakenEnabled}
                      label={krakenEnabled ? "Active" : undefined}
                    />
                  </div>
                </div>

                <VenueBalancePanel
                  bal={getVenueBalance("kraken")}
                  lane={getVenueLane("kraken")}
                />

                <div className="space-y-4">
                  <div className="p-2.5 bg-blue-50/70 border border-blue-200 rounded-lg text-xs font-mono text-slate-600">
                    💡 Kraken maintains separate API infrastructure for Spot and Futures markets. Toggle each option independently to enter its respective credentials.
                  </div>

                  {/* Option A: Kraken Spot Market */}
                  <div className="p-4 bg-surface-raised border border-surface-border rounded-lg space-y-3">
                    <div className="flex items-center justify-between border-b border-surface-border/60 pb-2">
                      <div className="flex items-center space-x-2">
                        <span className="text-xs font-bold font-mono text-slate-900">1. Kraken Spot Trading API</span>
                        <span className="text-xs font-mono px-1.5 py-0.2 rounded-sm bg-slate-200 text-slate-700">api.kraken.com</span>
                        <AccountLabelControl venue="kraken_spot" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                      </div>
                      <AccountCredentialsDelete
                        venue="kraken_spot"
                        refreshKey={accountsEpoch}
                        locked={isVaultLocked}
                        onChanged={handleAccountsChanged}
                      />
                      <VenueDefaultControl venue="kraken_spot" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                      <VenueAccountToggle
                        venue="kraken_spot"
                        refreshKey={accountsEpoch}
                        locked={isVaultLocked}
                        fallbackEnabled={krakenSpotEnabled}
                        onFallbackChange={setKrakenSpotEnabled}
                        onChanged={() => setAccountsEpoch((e) => e + 1)}
                      />
                    </div>

                    <div className="space-y-3 pt-1">
                      <VenueAccountsSection
                        venue="kraken_spot"
                        locked={isVaultLocked}
                        refreshKey={accountsEpoch}
                        compact
                        onChanged={() => setAccountsEpoch((e) => e + 1)}
                      />
                    </div>
                  </div>

                  <AddAccountSlot
                    venue="kraken_spot"
                    locked={isVaultLocked}
                    onChanged={() => setAccountsEpoch((e) => e + 1)}
                  />

                  {/* Option B: Kraken Futures Market */}
                  <div className="p-4 bg-surface-raised border border-surface-border rounded-lg space-y-3">
                    <div className="flex items-center justify-between border-b border-surface-border/60 pb-2">
                      <div className="flex items-center space-x-2">
                        <span className="text-xs font-bold font-mono text-slate-900">2. Kraken Futures Trading API</span>
                        <span className="text-xs font-mono px-1.5 py-0.2 rounded-sm bg-indigo-50 text-indigo-700 border border-indigo-200">futures.kraken.com</span>
                        <AccountLabelControl venue="kraken_futures" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                      </div>
                      <AccountCredentialsDelete
                        venue="kraken_futures"
                        refreshKey={accountsEpoch}
                        locked={isVaultLocked}
                        onChanged={handleAccountsChanged}
                      />
                      <VenueDefaultControl venue="kraken_futures" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                      <VenueAccountToggle
                        venue="kraken_futures"
                        refreshKey={accountsEpoch}
                        locked={isVaultLocked}
                        fallbackEnabled={krakenFuturesEnabled}
                        onFallbackChange={setKrakenFuturesEnabled}
                        onChanged={() => setAccountsEpoch((e) => e + 1)}
                      />
                    </div>

                    <div className="space-y-3 pt-1">
                      <VenueAccountsSection
                        venue="kraken_futures"
                        locked={isVaultLocked}
                        refreshKey={accountsEpoch}
                        compact
                        onChanged={() => setAccountsEpoch((e) => e + 1)}
                      />
                    </div>
                  </div>

                  <AddAccountSlot
                    venue="kraken_futures"
                    locked={isVaultLocked}
                    onChanged={() => setAccountsEpoch((e) => e + 1)}
                  />
                </div>
              </div>
            )}



            {/* Exchange 2: Lighter */}
            {selectedExchange === "lighter" && (
              <div className="space-y-4">
              <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-[13px] font-mono font-bold text-slate-900">
                      Lighter #1
                    </h3>
                    <AccountLabelControl venue="lighter" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                  </div>
                  <div className="flex items-center space-x-3">
                    <AccountCredentialsDelete
                      venue="lighter"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      onChanged={handleAccountsChanged}
                    />
                    <VenueDefaultControl venue="lighter" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                    <VenueAccountToggle
                      venue="lighter"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      fallbackEnabled={lighterEnabled}
                      onFallbackChange={setLighterEnabled}
                      onChanged={() => setAccountsEpoch((e) => e + 1)}
                    />
                  </div>
                </div>

                <VenueBalancePanel
                  bal={getVenueBalance("lighter")}
                  lane={getVenueLane("lighter")}
                />

                <div className="space-y-3">
                  <VenueAccountsSection
                    venue="lighter"
                    locked={isVaultLocked}
                    refreshKey={accountsEpoch}
                    onChanged={() => setAccountsEpoch((e) => e + 1)}
                  />

                </div>
              </div>

              <AddAccountSlot
                venue="lighter"
                locked={isVaultLocked}
                onChanged={() => setAccountsEpoch((e) => e + 1)}
              />
              </div>
            )}



            {/* Exchange 6: Polymarket */}
            {selectedExchange === "polymarket" && (
              <div className="space-y-4">
              <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-[13px] font-mono font-bold text-slate-900">
                      Polymarket #1
                    </h3>
                    <AccountLabelControl venue="polymarket" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                  </div>
                  <div className="flex items-center space-x-3">
                    <AccountCredentialsDelete
                      venue="polymarket"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      onChanged={handleAccountsChanged}
                    />
                    <VenueDefaultControl venue="polymarket" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                    <VenueAccountToggle
                      venue="polymarket"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      fallbackEnabled={polymarketEnabled}
                      onFallbackChange={setPolymarketEnabled}
                      onChanged={() => setAccountsEpoch((e) => e + 1)}
                    />
                  </div>
                </div>

                <VenueBalancePanel
                  bal={getVenueBalance("polymarket")}
                  lane={getVenueLane("polymarket")}
                />                  <VenueAccountsSection
                    venue="polymarket"
                    locked={isVaultLocked}
                    refreshKey={accountsEpoch}
                    onChanged={() => setAccountsEpoch((e) => e + 1)}
                  />
              </div>

              <AddAccountSlot
                venue="polymarket"
                locked={isVaultLocked}
                onChanged={() => setAccountsEpoch((e) => e + 1)}
              />
              </div>
            )}



            {/* Exchange: PopDEX */}
            {selectedExchange === "popdex" && (
              <div className="space-y-4">
              <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-[13px] font-mono font-bold text-slate-900">
                      PopDEX #1
                    </h3>
                    <AccountLabelControl venue="popdex" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                  </div>
                  <div className="flex items-center space-x-3">
                    <AccountCredentialsDelete
                      venue="popdex"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      onChanged={handleAccountsChanged}
                    />
                    <VenueDefaultControl venue="popdex" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                    <VenueAccountToggle
                      venue="popdex"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      fallbackEnabled={popdexEnabled}
                      onFallbackChange={setPopdexEnabled}
                      onChanged={() => setAccountsEpoch((e) => e + 1)}
                    />
                  </div>
                </div>

                <VenueBalancePanel
                  bal={getVenueBalance("popdex")}
                  lane={getVenueLane("popdex")}
                />                  <VenueAccountsSection
                    venue="popdex"
                    locked={isVaultLocked}
                    refreshKey={accountsEpoch}
                    onChanged={() => setAccountsEpoch((e) => e + 1)}
                  />
              </div>

              <AddAccountSlot
                venue="popdex"
                locked={isVaultLocked}
                onChanged={() => setAccountsEpoch((e) => e + 1)}
              />
              </div>
            )}



            {/* Exchange 4: Robinhood EVM */}
            {selectedExchange === "robinhood" && (
              <div className="space-y-4">
              <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-[13px] font-mono font-bold text-slate-900">
                      Robinhood Lighter #1
                    </h3>
                    <AccountLabelControl venue="robinhood" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                  </div>
                  <div className="flex items-center space-x-3">
                    <AccountCredentialsDelete
                      venue="robinhood"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      onChanged={handleAccountsChanged}
                    />
                    <VenueDefaultControl venue="robinhood" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                    <VenueAccountToggle
                      venue="robinhood"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      fallbackEnabled={robinhoodEnabled}
                      onFallbackChange={setRobinhoodEnabled}
                      onChanged={() => setAccountsEpoch((e) => e + 1)}
                    />
                  </div>
                </div>

                <VenueBalancePanel
                  bal={getVenueBalance("robinhood")}
                  lane={getVenueLane("robinhood")}
                />                  <VenueAccountsSection
                    venue="robinhood"
                    locked={isVaultLocked}
                    refreshKey={accountsEpoch}
                    onChanged={() => setAccountsEpoch((e) => e + 1)}
                  />
              </div>

              <AddAccountSlot
                venue="robinhood"
                locked={isVaultLocked}
                onChanged={() => setAccountsEpoch((e) => e + 1)}
              />
              </div>
            )}



            {/* Exchange 3: Vanta */}
            {selectedExchange === "vanta" && (
              <div className="space-y-4">
              <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-[13px] font-mono font-bold text-slate-900">
                      Vanta #1
                    </h3>
                    <AccountLabelControl venue="vanta" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                  </div>
                  <div className="flex items-center space-x-3">
                    <AccountCredentialsDelete
                      venue="vanta"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      onChanged={handleAccountsChanged}
                    />
                    <VenueDefaultControl venue="vanta" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                    <VenueAccountToggle
                      venue="vanta"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      fallbackEnabled={vantaEnabled}
                      onFallbackChange={setVantaEnabled}
                      onChanged={() => setAccountsEpoch((e) => e + 1)}
                    />
                  </div>
                </div>

                <VenueBalancePanel
                  bal={getVenueBalance("vanta")}
                  lane={getVenueLane("vanta")}
                />

                <VenueAccountsSection
                  venue="vanta"
                  locked={isVaultLocked}
                  refreshKey={accountsEpoch}
                  onChanged={() => setAccountsEpoch((e) => e + 1)}
                />
              </div>

              <AddAccountSlot
                venue="vanta"
                locked={isVaultLocked}
                onChanged={() => setAccountsEpoch((e) => e + 1)}
              />
              </div>
            )}

            {/* Exchange: Aster */}
            {selectedExchange === "aster" && (
              <div className="space-y-4">
              <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-[13px] font-mono font-bold text-slate-900">
                      Aster #1
                    </h3>
                    <AccountLabelControl venue="aster" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                  </div>
                  <div className="flex items-center space-x-3">
                    <AccountCredentialsDelete
                      venue="aster"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      onChanged={handleAccountsChanged}
                    />
                    <VenueDefaultControl venue="aster" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                    <VenueAccountToggle
                      venue="aster"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      fallbackEnabled={asterEnabled}
                      onFallbackChange={setAsterEnabled}
                      onChanged={() => setAccountsEpoch((e) => e + 1)}
                    />
                  </div>
                </div>

                <VenueBalancePanel
                  bal={getVenueBalance("aster")}
                  lane={getVenueLane("aster")}
                />

                <VenueAccountsSection
                  venue="aster"
                  locked={isVaultLocked}
                  refreshKey={accountsEpoch}
                  onChanged={() => setAccountsEpoch((e) => e + 1)}
                />
              </div>

              <AddAccountSlot
                venue="aster"
                locked={isVaultLocked}
                onChanged={() => setAccountsEpoch((e) => e + 1)}
              />
              </div>
            )}

            {/* Exchange: Binance */}
            {selectedExchange === "binance" && (
              <div className="space-y-4">
              <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-[13px] font-mono font-bold text-slate-900">
                      Binance #1
                    </h3>
                    <AccountLabelControl venue="binance" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                  </div>
                  <div className="flex items-center space-x-3">
                    <AccountCredentialsDelete
                      venue="binance"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      onChanged={handleAccountsChanged}
                    />
                    <VenueDefaultControl venue="binance" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                    <VenueAccountToggle
                      venue="binance"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      fallbackEnabled={binanceEnabled}
                      onFallbackChange={setBinanceEnabled}
                      onChanged={() => setAccountsEpoch((e) => e + 1)}
                    />
                  </div>
                </div>

                <VenueBalancePanel
                  bal={getVenueBalance("binance")}
                  lane={getVenueLane("binance")}
                />

                <VenueAccountsSection
                  venue="binance"
                  locked={isVaultLocked}
                  refreshKey={accountsEpoch}
                  onChanged={() => setAccountsEpoch((e) => e + 1)}
                />
              </div>

              <AddAccountSlot
                venue="binance"
                locked={isVaultLocked}
                onChanged={() => setAccountsEpoch((e) => e + 1)}
              />
              </div>
            )}

            {/* Exchange: Coinbase */}
            {selectedExchange === "coinbase" && (
              <div className="space-y-4">
              <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-[13px] font-mono font-bold text-slate-900">
                      Coinbase #1
                    </h3>
                    <AccountLabelControl venue="coinbase" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                  </div>
                  <div className="flex items-center space-x-3">
                    <AccountCredentialsDelete
                      venue="coinbase"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      onChanged={handleAccountsChanged}
                    />
                    <VenueDefaultControl venue="coinbase" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                    <VenueAccountToggle
                      venue="coinbase"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      fallbackEnabled={coinbaseEnabled}
                      onFallbackChange={setCoinbaseEnabled}
                      onChanged={() => setAccountsEpoch((e) => e + 1)}
                    />
                  </div>
                </div>

                <VenueBalancePanel
                  bal={getVenueBalance("coinbase")}
                  lane={getVenueLane("coinbase")}
                />

                <VenueAccountsSection
                  venue="coinbase"
                  locked={isVaultLocked}
                  refreshKey={accountsEpoch}
                  onChanged={() => setAccountsEpoch((e) => e + 1)}
                />
              </div>

              <AddAccountSlot
                venue="coinbase"
                locked={isVaultLocked}
                onChanged={() => setAccountsEpoch((e) => e + 1)}
              />
              </div>
            )}

            {/* Exchange: Decibel */}
            {selectedExchange === "decibel" && (
              <div className="space-y-4">
              <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-[13px] font-mono font-bold text-slate-900">
                      Decibel #1
                    </h3>
                    <AccountLabelControl venue="decibel" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                  </div>
                  <div className="flex items-center space-x-3">
                    <AccountCredentialsDelete
                      venue="decibel"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      onChanged={handleAccountsChanged}
                    />
                    <VenueDefaultControl venue="decibel" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                    <VenueAccountToggle
                      venue="decibel"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      fallbackEnabled={decibelEnabled}
                      onFallbackChange={setDecibelEnabled}
                      onChanged={() => setAccountsEpoch((e) => e + 1)}
                    />
                  </div>
                </div>

                <VenueBalancePanel
                  bal={getVenueBalance("decibel")}
                  lane={getVenueLane("decibel")}
                />

                <VenueAccountsSection
                  venue="decibel"
                  locked={isVaultLocked}
                  refreshKey={accountsEpoch}
                  onChanged={() => setAccountsEpoch((e) => e + 1)}
                />
              </div>

              <AddAccountSlot
                venue="decibel"
                locked={isVaultLocked}
                onChanged={() => setAccountsEpoch((e) => e + 1)}
              />
              </div>
            )}

            {/* Exchange: Interactive Brokers */}
            {/* Exchange: edgeX */}
            {selectedExchange === "edgex" && (
              <div className="space-y-4">
              <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-[13px] font-mono font-bold text-slate-900">
                      edgeX #1
                    </h3>
                    <AccountLabelControl venue="edgex" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                  </div>
                  <div className="flex items-center space-x-3">
                    <AccountCredentialsDelete
                      venue="edgex"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      onChanged={handleAccountsChanged}
                    />
                    <VenueDefaultControl venue="edgex" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                    <VenueAccountToggle
                      venue="edgex"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      fallbackEnabled={edgexEnabled}
                      onFallbackChange={setEdgexEnabled}
                      onChanged={() => setAccountsEpoch((e) => e + 1)}
                    />
                  </div>
                </div>

                <VenueBalancePanel
                  bal={getVenueBalance("edgex")}
                  lane={getVenueLane("edgex")}
                />

                <VenueAccountsSection
                  venue="edgex"
                  locked={isVaultLocked}
                  refreshKey={accountsEpoch}
                  onChanged={() => setAccountsEpoch((e) => e + 1)}
                />
              </div>

              <AddAccountSlot
                venue="edgex"
                locked={isVaultLocked}
                onChanged={() => setAccountsEpoch((e) => e + 1)}
              />
              </div>
            )}

            {/* Exchange: GRVT */}
            {selectedExchange === "grvt" && (
              <div className="space-y-4">
              <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-[13px] font-mono font-bold text-slate-900">
                      GRVT #1
                    </h3>
                    <AccountLabelControl venue="grvt" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                  </div>
                  <div className="flex items-center space-x-3">
                    <AccountCredentialsDelete
                      venue="grvt"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      onChanged={handleAccountsChanged}
                    />
                    <VenueDefaultControl venue="grvt" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                    <VenueAccountToggle
                      venue="grvt"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      fallbackEnabled={grvtEnabled}
                      onFallbackChange={setGrvtEnabled}
                      onChanged={() => setAccountsEpoch((e) => e + 1)}
                    />
                  </div>
                </div>

                <VenueBalancePanel
                  bal={getVenueBalance("grvt")}
                  lane={getVenueLane("grvt")}
                />

                <VenueAccountsSection
                  venue="grvt"
                  locked={isVaultLocked}
                  refreshKey={accountsEpoch}
                  onChanged={() => setAccountsEpoch((e) => e + 1)}
                />
              </div>

              <AddAccountSlot
                venue="grvt"
                locked={isVaultLocked}
                onChanged={() => setAccountsEpoch((e) => e + 1)}
              />
              </div>
            )}

            {selectedExchange === "ibkr" && (
              <div className="space-y-4">
              <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-[13px] font-mono font-bold text-slate-900">
                      Interactive Brokers #1
                    </h3>
                    <AccountLabelControl venue="ibkr" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                  </div>
                  <div className="flex items-center space-x-3">
                    <AccountCredentialsDelete
                      venue="ibkr"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      onChanged={handleAccountsChanged}
                    />
                    <VenueDefaultControl venue="ibkr" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                    <VenueAccountToggle
                      venue="ibkr"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      fallbackEnabled={ibkrEnabled}
                      onFallbackChange={setIbkrEnabled}
                      onChanged={() => setAccountsEpoch((e) => e + 1)}
                    />
                  </div>
                </div>

                <VenueBalancePanel
                  bal={getVenueBalance("ibkr")}
                  lane={getVenueLane("ibkr")}
                />

                <VenueAccountsSection
                  venue="ibkr"
                  locked={isVaultLocked}
                  refreshKey={accountsEpoch}
                  onChanged={() => setAccountsEpoch((e) => e + 1)}
                />
              </div>

              <AddAccountSlot
                venue="ibkr"
                locked={isVaultLocked}
                onChanged={() => setAccountsEpoch((e) => e + 1)}
              />
              </div>
            )}

            {/* Exchange: IG Trading */}
            {selectedExchange === "ig" && (
              <div className="space-y-4">
              <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-[13px] font-mono font-bold text-slate-900">
                      IG Trading #1
                    </h3>
                    <AccountLabelControl venue="ig" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                  </div>
                  <div className="flex items-center space-x-3">
                    <AccountCredentialsDelete
                      venue="ig"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      onChanged={handleAccountsChanged}
                    />
                    <VenueDefaultControl venue="ig" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                    <VenueAccountToggle
                      venue="ig"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      fallbackEnabled={igEnabled}
                      onFallbackChange={setIgEnabled}
                      onChanged={() => setAccountsEpoch((e) => e + 1)}
                    />
                  </div>
                </div>

                <VenueBalancePanel
                  bal={getVenueBalance("ig")}
                  lane={getVenueLane("ig")}
                />

                <VenueAccountsSection
                  venue="ig"
                  locked={isVaultLocked}
                  refreshKey={accountsEpoch}
                  onChanged={() => setAccountsEpoch((e) => e + 1)}
                />
              </div>

              <AddAccountSlot
                venue="ig"
                locked={isVaultLocked}
                onChanged={() => setAccountsEpoch((e) => e + 1)}
              />
              </div>
            )}

            {/* Exchange: OKX */}
            {selectedExchange === "okx" && (
              <div className="space-y-4">
              <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-[13px] font-mono font-bold text-slate-900">
                      OKX #1
                    </h3>
                    <AccountLabelControl venue="okx" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                  </div>
                  <div className="flex items-center space-x-3">
                    <AccountCredentialsDelete
                      venue="okx"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      onChanged={handleAccountsChanged}
                    />
                    <VenueDefaultControl venue="okx" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                    <VenueAccountToggle
                      venue="okx"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      fallbackEnabled={okxEnabled}
                      onFallbackChange={setOkxEnabled}
                      onChanged={() => setAccountsEpoch((e) => e + 1)}
                    />
                  </div>
                </div>

                <VenueBalancePanel
                  bal={getVenueBalance("okx")}
                  lane={getVenueLane("okx")}
                />

                <VenueAccountsSection
                  venue="okx"
                  locked={isVaultLocked}
                  refreshKey={accountsEpoch}
                  onChanged={() => setAccountsEpoch((e) => e + 1)}
                />
              </div>

              <AddAccountSlot
                venue="okx"
                locked={isVaultLocked}
                onChanged={() => setAccountsEpoch((e) => e + 1)}
              />
              </div>
            )}

            {/* Exchange: Variational */}
            {/* Exchange: Ondo Perps */}
            {selectedExchange === "ondoperps" && (
              <div className="space-y-4">
              <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-[13px] font-mono font-bold text-slate-900">
                      Ondo Perps #1
                    </h3>
                    <AccountLabelControl venue="ondoperps" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                  </div>
                  <div className="flex items-center space-x-3">
                    <AccountCredentialsDelete
                      venue="ondoperps"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      onChanged={handleAccountsChanged}
                    />
                    <VenueDefaultControl venue="ondoperps" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                    <VenueAccountToggle
                      venue="ondoperps"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      fallbackEnabled={ondoperpsEnabled}
                      onFallbackChange={setOndoperpsEnabled}
                      onChanged={() => setAccountsEpoch((e) => e + 1)}
                    />
                  </div>
                </div>

                <VenueBalancePanel
                  bal={getVenueBalance("ondoperps")}
                  lane={getVenueLane("ondoperps")}
                />

                <VenueAccountsSection
                  venue="ondoperps"
                  locked={isVaultLocked}
                  refreshKey={accountsEpoch}
                  onChanged={() => setAccountsEpoch((e) => e + 1)}
                />
              </div>

              <AddAccountSlot
                venue="ondoperps"
                locked={isVaultLocked}
                onChanged={() => setAccountsEpoch((e) => e + 1)}
              />
              </div>
            )}

            {selectedExchange === "variational" && (
              <div className="space-y-4">
              <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-[13px] font-mono font-bold text-slate-900">
                      Variational #1
                    </h3>
                    <AccountLabelControl venue="variational" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                  </div>
                  <div className="flex items-center space-x-3">
                    <AccountCredentialsDelete
                      venue="variational"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      onChanged={handleAccountsChanged}
                    />
                    <VenueDefaultControl venue="variational" refreshKey={accountsEpoch} onChanged={() => setAccountsEpoch((e) => e + 1)} />
                    <VenueAccountToggle
                      venue="variational"
                      refreshKey={accountsEpoch}
                      locked={isVaultLocked}
                      fallbackEnabled={variationalEnabled}
                      onFallbackChange={setVariationalEnabled}
                      onChanged={() => setAccountsEpoch((e) => e + 1)}
                    />
                  </div>
                </div>

                <VenueBalancePanel
                  bal={getVenueBalance("variational")}
                  lane={getVenueLane("variational")}
                />

                <VenueAccountsSection
                  venue="variational"
                  locked={isVaultLocked}
                  refreshKey={accountsEpoch}
                  onChanged={() => setAccountsEpoch((e) => e + 1)}
                />
              </div>

              <AddAccountSlot
                venue="variational"
                locked={isVaultLocked}
                onChanged={() => setAccountsEpoch((e) => e + 1)}
              />
              </div>
            )}


              </div>

              {/* Locked Vault Overlay: Completely Greys Out the Exchange Configuration */}
              {isVaultLocked && (
                <div className="absolute inset-0 z-40 bg-slate-100 flex items-center justify-center p-4 animate-in fade-in duration-150">
                  <div className="bg-white rounded-xl p-6 shadow-2xl w-80 text-center space-y-4 border border-slate-200 animate-in zoom-in-95 duration-150">
                    <div className="w-12 h-12 rounded-lg bg-blue-50 border border-blue-100 flex items-center justify-center mx-auto text-accent shadow-2xs">
                      {api.isTauri && isVaultInitializing ? (
                        <Key className="w-5 h-5 text-accent" />
                      ) : (
                        <Lock className="w-5 h-5 text-accent" />
                      )}
                    </div>
                    {api.isTauri ? (
                      <>
                        <div>
                          <h4 className="text-[13px] font-bold font-mono tracking-wider text-slate-900">
                            {isVaultStatusLoading
                              ? "Checking Secure Vault"
                              : isVaultInitializing
                              ? "Create Password"
                              : "Vault Locked"}
                          </h4>
                          <p className="text-xs text-slate-500 font-sans mt-1">
                            {isVaultStatusLoading
                              ? "Loading local vault status..."
                              : isVaultInitializing
                              ? "Create a password to encrypt credentials"
                              : "Enter your password to unlock credentials"}
                          </p>
                        </div>

                        {inlineVaultError && (
                          <div className="p-2 bg-red-50 border border-red-200 rounded-md text-xs font-mono text-red-600">
                            {inlineVaultError}
                          </div>
                        )}

                        {!isVaultStatusLoading && (
                          <div className="space-y-3">
                            <input
                              type="password"
                              value={inlinePassword}
                              onChange={(e) => setInlinePassword(e.target.value)}
                              onKeyDown={handleInlineVaultKeyDown}
                              placeholder={isVaultInitializing ? "Create password..." : "Enter password..."}
                              autoComplete={isVaultInitializing ? "new-password" : "current-password"}
                              autoFocus
                              className="w-full px-3 py-2 bg-slate-50 border border-slate-200 rounded-md text-xs font-mono text-center focus:outline-none focus:ring-1 focus:ring-accent"
                            />
                            {isVaultInitializing && (
                              <>
                                <input
                                  type="password"
                                  value={inlineConfirmPassword}
                                  onChange={(e) => setInlineConfirmPassword(e.target.value)}
                                  onKeyDown={handleInlineVaultKeyDown}
                                  placeholder="Confirm password..."
                                  autoComplete="new-password"
                                  className="w-full px-3 py-2 bg-slate-50 border border-slate-200 rounded-md text-xs font-mono text-center focus:outline-none focus:ring-1 focus:ring-accent"
                                />
                                <p className="text-xs text-slate-500 font-sans leading-relaxed">
                                  Use at least 12 characters. Letters, numbers, and symbols are accepted. Longer passwords are safer, and this password cannot be recovered.
                                </p>
                              </>
                            )}
                            <button
                              type="button"
                              onClick={() => void handleInlineVaultSubmit()}
                              disabled={inlineUnlocking || !vaultStatus || !inlinePassword || (isVaultInitializing && !inlineConfirmPassword)}
                              className="w-full py-2 bg-accent hover:bg-blue-600 text-white font-semibold text-xs font-mono rounded-md transition shadow-2xs disabled:opacity-50 cursor-pointer"
                            >
                              {inlineUnlocking
                                ? isVaultInitializing ? "Creating Vault..." : "Unlocking..."
                                : isVaultInitializing ? "Create & Unlock Vault" : "Unlock Vault"}
                            </button>
                          </div>
                        )}
                      </>
                    ) : (
                      <div>
                        <h4 className="text-[13px] font-bold font-mono tracking-wider text-slate-900">Desktop Application Required</h4>
                        <p className="text-xs text-slate-500 font-sans mt-1">
                          Secure credential storage is available only in the desktop application.
                        </p>
                      </div>
                    )}
                  </div>
                </div>
              )}
            </div>
          ) : (
            <div className="flex-1 overflow-y-auto p-4 space-y-4">
              {errorMsg && (
                <div className="p-4 bg-red-50 border border-red-200 text-red-700 rounded-lg text-xs font-mono flex items-center space-x-2">
                  <AlertCircle className="w-4 h-4 shrink-0" />
                  <span>{errorMsg}</span>
                </div>
              )}

              {saved && (
                <div className="p-4 bg-emerald-50 border border-emerald-200 text-emerald-700 rounded-lg text-xs font-mono flex items-center space-x-2 animate-in fade-in duration-200">
                  <CheckCircle2 className="w-4 h-4 shrink-0" />
                  <span>Configuration saved successfully and updated in live atomic execution coordinator.</span>
                </div>
              )}
        {/* =================================================================== */}
        {/* SECTION 2B: STRATEGY ACCOUNT ROUTING                               */}
        {/* =================================================================== */}
        {activeTab === "strategies" && (
          <div className="max-w-4xl mx-auto w-full">
            <StrategyRoutingPanel />
          </div>
        )}

        {/* =================================================================== */}
        {/* SECTION 3: EXECUTION MODES & WARM-UP                               */}
        {/* =================================================================== */}
        {activeTab === "execution" && (
          <div className="max-w-4xl mx-auto w-full space-y-4 text-xs font-mono">
            {/* Trading Mode */}
            <div className="p-3.5 bg-surface border border-surface-border rounded-lg shadow-2xs space-y-3">
              <div className="space-y-2 min-w-0">
                <label className="text-slate-800 font-bold text-xs flex items-center">
                  Trading Mode
                  <InfoTooltip
                    position="bottom"
                    text={"Paper Trading executes simulated fills against live market data — no real orders are sent and balances are virtual.\n\nReal Trading submits signed orders to your connected exchange accounts on supported API venues (Aster & Hyperliquid). Live execution requires an unlocked credential vault and at least two live-ready venues."}
                  />
                </label>
                <p className="text-xs text-slate-500">
                  {hasApiConnectivity
                    ? "Both modes require exchange API connectivity for market data."
                    : "Unavailable — no exchange API is connected. Enable a venue with working connectivity in the Exchanges tab first."}
                </p>

                {/* Venue Execution Capability Indicators */}
                {venueCapabilities.length > 0 && (
                  <div className="flex flex-wrap gap-1.5 pt-1">
                    {venueCapabilities.map((cap) => (
                      <span
                        key={cap.venue}
                        className={`px-2 py-0.5 rounded border font-mono text-[10px] flex items-center gap-1.5 ${
                          cap.liveReady
                            ? "bg-emerald-500/10 border-emerald-500/30 text-emerald-700 dark:text-emerald-400"
                            : cap.supportsLive
                            ? "bg-amber-500/10 border-amber-500/30 text-amber-700 dark:text-amber-400"
                            : "bg-slate-100 dark:bg-slate-800/40 border-surface-border text-slate-400"
                        }`}
                        title={
                          cap.liveReady
                            ? `${cap.name}: Live execution ready (API keys active in unlocked vault)`
                            : cap.supportsLive
                            ? `${cap.name}: Live execution supported, but credentials missing or vault is locked`
                            : `${cap.name}: Read-only market data venue (no live order placement API)`
                        }
                      >
                        <span
                          className={`w-1.5 h-1.5 rounded-full ${
                            cap.liveReady
                              ? "bg-emerald-500 animate-pulse"
                              : cap.supportsLive
                              ? "bg-amber-500"
                              : "bg-slate-400"
                          }`}
                        />
                        <span className="font-semibold">{cap.name}</span>:{" "}
                        {cap.liveReady ? "LIVE READY" : cap.supportsLive ? "CREDS NEEDED" : "READ ONLY"}
                      </span>
                    ))}
                  </div>
                )}

                <div className="flex items-center space-x-4 pt-1">
                  <label
                    className={`flex items-center space-x-2 ${hasApiConnectivity ? "cursor-pointer" : "cursor-not-allowed opacity-60"}`}
                    title={hasApiConnectivity ? undefined : "Requires at least one connected exchange API for market data"}
                  >
                    <input
                      type="radio"
                      name="trading_mode"
                      value="paper"
                      checked={tradingMode === "paper"}
                      onChange={() => setTradingMode("paper")}
                      disabled={!hasApiConnectivity}
                      className="text-accent focus:ring-accent"
                    />
                    <span className="font-semibold text-slate-800">Paper Trading</span>
                  </label>
                  <label
                    className={`flex items-center space-x-2 ${
                      canSwitchToLive ? "cursor-pointer" : "cursor-not-allowed opacity-60"
                    }`}
                    title={
                      !vaultStatus?.is_unlocked
                        ? "Vault is locked. Unlock the vault to enable real trading."
                        : liveReadyVenues.length < 2
                        ? `Requires at least 2 live-ready venues (currently ${liveReadyVenues.length}: ${liveReadyVenues.map(v => v.name).join(", ") || "none"}). Aster & Hyperliquid support live API orders.`
                        : "Switch to Real Trading"
                    }
                  >
                    <input
                      type="radio"
                      name="trading_mode"
                      value="real"
                      checked={tradingMode === "real"}
                      disabled={!canSwitchToLive}
                      onChange={() => {
                        if (canSwitchToLive) {
                          if (!liveRiskAgreed && tradingMode !== "real") {
                            setShowLiveRiskModal(true);
                          } else {
                            setTradingMode("real");
                          }
                        }
                      }}
                      className="text-red-600 focus:ring-red-600"
                    />
                    <span className="font-semibold text-slate-800 flex items-center gap-1.5">
                      Real Trading
                      {tradingMode === "real" && (
                        <span className="text-[10px] bg-red-600 text-white font-mono px-1.5 py-0.5 rounded font-bold uppercase tracking-wider animate-pulse">
                          LIVE
                        </span>
                      )}
                    </span>
                  </label>
                </div>

                {!canSwitchToLive && (
                  <p className="text-[11px] text-amber-600 dark:text-amber-400 font-mono mt-0.5">
                    {!vaultStatus?.is_unlocked
                      ? "🔒 Real trading locked: Unlock the secure credential vault to enable live orders."
                      : `⚠️ Real trading requires at least 2 live-ready venues with API keys enabled (${liveReadyVenues.length}/2 ready: ${liveReadyVenues.map(v => v.name).join(", ") || "none"}). Aster & Hyperliquid support live order execution.`}
                  </p>
                )}
              </div>

              <div className="pt-3 border-t border-surface-border flex items-center justify-between gap-3">
                <div className="flex-1 min-w-0">
                  <label className="text-slate-600 block font-mono text-xs">
                    Paper Starting Balance (USD)
                  </label>
                  <p className="text-xs text-slate-400 leading-snug mt-0.5">
                    Virtual equity per paper account. Saving resets paper accounts and clears open positions.
                  </p>
                </div>
                <input
                  type="number"
                  min="1"
                  step="any"
                  disabled={loading}
                  value={paperStartingBalance}
                  onChange={(e) => setPaperStartingBalance(parseFloat(e.target.value) || 0)}
                  className="w-28 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>
            </div>

            {/* Orderbook Fast Warm-up */}
            <div className="p-3.5 bg-surface border border-surface-border rounded-lg shadow-2xs flex items-center justify-between gap-4">
              <div className="space-y-1 min-w-0">
                <label className="text-slate-800 font-bold block text-xs">Orderbook Cache Warm-Up</label>
                <p className="text-xs text-slate-500">
                  {fastWarmupEnabled
                    ? "Fast Warm-Up: Persists sliding window orderbook quotes across app restarts for zero-lag instant signal readiness."
                    : "Cold Start: Starts orderbook buffer from scratch on every launch (requires 30-60s warm-up window)."}
                </p>
              </div>
              <ToggleSwitch
                checked={fastWarmupEnabled}
                onChange={setFastWarmupEnabled}
                disabled={loading}
              />
            </div>

            {/* Historical Price Data */}
            <div className="p-3.5 bg-surface border border-surface-border rounded-lg shadow-2xs space-y-3">
              <div className="space-y-1 min-w-0">
                <label className="text-slate-800 font-bold block text-xs">Historical Price Data</label>
                <p className="text-xs text-slate-500">
                  How much top-of-book history to keep. Empty means no limit. Time and
                  size limits compose — whichever binds first wins.
                </p>
              </div>
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                <div className="flex items-center justify-between gap-3">
                  <div className="flex-1 min-w-0">
                    <label className="text-slate-600 block font-mono text-xs">
                      Max history days
                    </label>
                    <p className="text-xs text-slate-400 leading-snug mt-0.5">
                      {maxHistoryDays.trim() === "" ? "No limit — oldest data accumulates." : "Samples older than this are pruned hourly."}
                    </p>
                  </div>
                  <input
                    type="number"
                    min="0"
                    step="1"
                    disabled={loading}
                    value={maxHistoryDays}
                    placeholder="No limit"
                    onChange={(e) => setMaxHistoryDays(e.target.value)}
                    className="w-24 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
                  />
                </div>
                <div className="flex items-center justify-between gap-3">
                  <div className="flex-1 min-w-0">
                    <label className="text-slate-600 block font-mono text-xs">
                      Max storage GB
                    </label>
                    <p className="text-xs text-slate-400 leading-snug mt-0.5">
                      {maxStorageGb.trim() === "" ? "No limit — oldest data accumulates." : "Oldest samples are evicted hourly until under budget."}
                    </p>
                  </div>
                  <input
                    type="number"
                    min="0"
                    step="any"
                    disabled={loading}
                    value={maxStorageGb}
                    placeholder="No limit"
                    onChange={(e) => setMaxStorageGb(e.target.value)}
                    className="w-24 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
                  />
                </div>
              </div>
            </div>

          </div>
        )}

        {/* =================================================================== */}
        {/* SECTION 4: NETWORKING & VPS 24/7 DEPLOYMENT                         */}
        {/* =================================================================== */}
        {activeTab === "network" && (
          <div className="bg-surface border border-surface-border rounded-lg p-3.5 space-y-3 shadow-2xs">
            <div className="border-b border-surface-border pb-2.5">
              <h3 className="text-[13px] font-mono font-bold text-slate-900 flex items-center space-x-2">
                <Globe className="w-4 h-4 text-accent" />
                <span>Engine Ports & 24/7 VPS Background Daemon</span>
              </h3>
              <p className="text-xs text-slate-500 font-mono mt-0.5">
                Configure RPC / WebSocket listener ports and deploy headlessly on cloud servers
              </p>
            </div>

            <div className="grid grid-cols-1 md:grid-cols-3 gap-4 text-xs font-mono">
              <div className="flex items-center justify-between gap-3">
                <label className="text-slate-600 flex-1">Engine RPC Port:</label>
                <input
                  type="number"
                  disabled={loading}
                  value={engineRpcPort}
                  onChange={(e) => setEngineRpcPort(parseInt(e.target.value, 10) || 8787)}
                  className="w-24 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>

              <div className="flex items-center justify-between gap-3">
                <label className="text-slate-600 flex-1">Engine WebSocket Port:</label>
                <input
                  type="number"
                  disabled={loading}
                  value={engineWsPort}
                  onChange={(e) => setEngineWsPort(parseInt(e.target.value, 10) || 8789)}
                  className="w-24 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>

              <div className="flex items-center justify-between gap-3">
                <label className="text-slate-600 flex-1">Bind Address:</label>
                <input
                  type="text"
                  disabled={loading}
                  value={bindAddress}
                  placeholder="127.0.0.1 or 0.0.0.0"
                  onChange={(e) => setBindAddress(e.target.value)}
                  className="w-32 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                />
              </div>
            </div>

            {/* VPS Daemon Deployment Guide */}
            <div className="p-4 bg-slate-50 border border-surface-border rounded-lg space-y-2 text-xs font-mono text-slate-700">
              <h4 className="font-bold text-slate-900 flex items-center space-x-1.5">
                <span>🖥️ Headless VPS 24/7 Deployment Instructions</span>
              </h4>
              <p className="text-xs text-slate-600">
                To run Arbitrager continuously 24/7 without keeping your local laptop or desktop open, deploy the standalone Rust binary to an Ubuntu/Debian VPS:
              </p>
              <div className="p-2.5 bg-slate-900 text-emerald-400 rounded-md text-xs overflow-x-auto select-text font-mono">
                <code>./target/release/arbitrager serve --bind 0.0.0.0 --port 8787</code>
              </div>
              <p className="text-xs text-slate-500">
                The serve command exposes a read-only health endpoint only — no market data ingestion or order execution runs. You can create a systemd service (e.g. <code className="bg-slate-200 px-1 py-0.5 rounded-sm">systemctl start arbitrager</code>) to auto-restart on server reboot.
              </p>
            </div>
          </div>
        )}

        {/* =================================================================== */}
        {/* SECTION 5: ABOUT, CHECK FOR UPDATES & BUY ME A SOLO                 */}
        {/* =================================================================== */}
        {activeTab === "about" && (
          <div className="space-y-4">
            <div className="bg-surface border border-surface-border rounded-lg p-3.5 shadow-2xs space-y-3">
              <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4 border-b border-surface-border pb-2.5">
                <div className="flex items-center space-x-3">
                  <div className="w-10 h-10 rounded-lg bg-accent-dim border border-accent-line flex items-center justify-center text-accent">
                    <ArbitragerLogo size={22} className="text-accent" />
                  </div>
                  <div>
                    <h3 className="text-[13px] font-bold font-mono tracking-wider text-slate-900">
                      About Arbitrager ({version})
                    </h3>
                  </div>
                </div>

                <div className="flex items-center space-x-2">
                  <button
                    type="button"
                    onClick={handleCheckForUpdates}
                    disabled={checkingUpdate}
                    className="px-3.5 py-1.5 bg-surface-raised hover:bg-slate-200 border border-surface-border text-slate-800 rounded-md text-xs font-mono font-medium transition flex items-center space-x-1.5 shadow-2xs"
                  >
                    <RefreshCw className={`w-3.5 h-3.5 ${checkingUpdate ? "animate-spin text-accent" : ""}`} />
                    <span>{checkingUpdate ? "Checking..." : "Check for Updates"}</span>
                  </button>

                  <a
                    href={gitRepo}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="px-3.5 py-1.5 bg-surface hover:bg-surface-raised border border-surface-border text-slate-800 rounded-lg text-xs font-mono font-medium transition flex items-center space-x-1.5 shadow-2xs"
                  >
                    <span>Git Repository</span>
                    <ExternalLink className="w-3.5 h-3.5" />
                  </a>
                </div>
              </div>

              {updateMsg && (
                <div className="p-3 bg-emerald-50 border border-emerald-200 text-emerald-800 rounded-lg text-xs font-mono flex items-center space-x-2">
                  <CheckCircle2 className="w-4 h-4 text-emerald-600 shrink-0" />
                  <span>{updateMsg}</span>
                </div>
              )}

              {/* Sponsor Card: Buy me a SOLO */}
              <div className="p-5 bg-gradient-to-r from-amber-50/80 via-yellow-50/50 to-amber-50/80 border border-amber-200 rounded-lg flex flex-col sm:flex-row items-center justify-between gap-4 shadow-2xs">
                <div className="flex items-center space-x-4">
                  <img
                    src="/solo_can.jpg"
                    alt="Can of Australian SOLO Lemon Soda"
                    className="w-14 h-14 object-contain rounded-lg shadow-2xs border border-amber-200 bg-white"
                  />
                  <div>
                    <h4 className="text-[13px] font-bold font-mono tracking-wider text-slate-900">
                      Enjoying The Tool? Buy Me A SOLO!
                    </h4>
                  </div>
                </div>

                <a
                  href={sponsorLink}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="px-5 py-2.5 bg-amber-500 hover:bg-amber-600 text-white font-bold text-xs font-mono rounded-lg transition shadow-2xs flex items-center space-x-2 shrink-0"
                >
                  <span>Buy me a SOLO ($5)</span>
                  <ExternalLink className="w-3.5 h-3.5" />
                </a>
              </div>

              {/* Legal & Financial Disclaimer */}
              <div className="p-4 bg-slate-50 border border-surface-border rounded-lg space-y-2 text-xs font-mono text-slate-600 leading-relaxed">
                <div className="flex items-center space-x-1.5 font-bold text-slate-800 text-xs">
                  <Scale className="w-3.5 h-3.5 text-slate-500" />
                  <span>Legal & Financial Risk Disclaimer</span>
                </div>
                <p>
                  This software is provided for research, simulation, and educational quantitative finance purposes. Cross-market arbitrage and algorithmic trading involve inherent execution risks, including market volatility, network latency spikes, smart contract bugs, API rate limits, and liquidity slippage. Past simulation performance does not guarantee future financial returns.
                </p>
                <p>
                  No private keys or seed phrases are stored remotely or accessed by any third party; all credentials remain encrypted locally on your system using AES-256-GCM with Argon2id key derivation. Users are solely responsible for compliance with their local jurisdiction regulations and exchange terms of service.
                </p>
              </div>
            </div>
          </div>
        )}

            </div>
          )}

      {/* Pinned Save Bar with Trading Mode Indicator on Bottom-Left */}
      <div className="p-4 border-t border-surface-border bg-slate-50/80 shrink-0 flex items-center justify-between">
        {/* Relocated Trading Mode Indicator on bottom-left */}
        <div
          className="px-2.5 py-1.5 rounded-md border border-surface-border bg-surface text-slate-700 text-xs font-mono flex items-center space-x-1.5 shadow-2xs"
          title={`Execution mode: ${tradingMode.toUpperCase()}`}
        >
          <span className="text-slate-400 font-normal">Mode:</span>
          <span className="font-bold text-accent">{tradingMode.charAt(0).toUpperCase() + tradingMode.slice(1)}</span>
        </div>

        <div className="flex items-center space-x-2.5">
          {onClose && (
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-2 bg-surface-raised hover:bg-slate-200 text-slate-700 border border-surface-border font-medium text-xs font-mono rounded-md transition shadow-2xs cursor-pointer"
            >
              Cancel
            </button>
          )}
          <button
            type="submit"
            disabled={loading || saving}
            className="px-5 py-2 bg-accent hover:bg-blue-600 text-white font-medium text-xs font-mono rounded-md transition flex items-center space-x-2 shadow-2xs disabled:opacity-50 cursor-pointer"
          >
            {saving ? (
              <span>Saving...</span>
            ) : saved ? (
              <>
                <CheckCircle2 className="w-3.5 h-3.5" />
                <span>Saved to Engine</span>
              </>
            ) : (
              <span>Save Configuration</span>
            )}
          </button>
        </div>
      </div>
    </form>
  </div>

  {/* Vault Unlock Modal (desktop only) */}
  <VaultUnlockModal
    isOpen={showVaultModal && api.isTauri}
    vaultStatus={vaultStatus}
    onClose={() => setShowVaultModal(false)}
    onSuccess={(status) => {
      setVaultStatus(status);
      setShowVaultModal(false);
    }}
  />

  {/* Live Trading Risk Confirmation Modal */}
  {showLiveRiskModal && (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/75 backdrop-blur-xs">
      <div className="bg-surface-raised border-2 border-red-500/50 rounded-xl shadow-2xl max-w-lg w-full p-6 text-slate-100 animate-in fade-in zoom-in-95 duration-150">
        <div className="flex items-center space-x-3 text-red-400 mb-4 pb-3 border-b border-surface-border">
          <div className="p-2 bg-red-500/10 rounded-lg border border-red-500/30">
            <AlertTriangle className="w-6 h-6 text-red-500" />
          </div>
          <div>
            <h3 className="text-lg font-bold text-slate-100">Institutional Risk Warning</h3>
            <p className="text-xs text-red-400/90 font-mono">Live Automated Execution Activation</p>
          </div>
        </div>

        <div className="space-y-3 text-xs text-slate-300 mb-6 leading-relaxed">
          <div className="p-3 bg-red-950/30 border border-red-800/40 rounded-lg text-red-200 space-y-1.5 font-mono text-[11px]">
            <p className="font-bold text-red-300">⚠️ CAPITAL AT RISK — TWO SIGMA EXECUTION DISCIPLINE</p>
            <p>
              You are enabling real-money automated order submission. Live orders will be placed directly to connected exchange order books.
            </p>
          </div>

          <div className="space-y-2 text-slate-300">
            <div className="flex items-start gap-2">
              <span className="font-bold text-accent font-mono">1.</span>
              <p>
                <strong className="text-slate-100">Selective Live Execution:</strong> Only venue pairs where <span className="text-emerald-400 font-semibold">BOTH venues support live API execution</span> (currently Aster & Hyperliquid) will submit live orders. Routes involving read-only venues will fail closed to prevent naked unhedged legs.
              </p>
            </div>
            <div className="flex items-start gap-2">
              <span className="font-bold text-accent font-mono">2.</span>
              <p>
                <strong className="text-slate-100">Market Risk & Latency:</strong> Slippage, network jitter, and orderbook changes can result in price deviation or partial fills.
              </p>
            </div>
            <div className="flex items-start gap-2">
              <span className="font-bold text-accent font-mono">3.</span>
              <p>
                <strong className="text-slate-100">Kill Switch:</strong> Use the <span className="font-mono text-amber-400 font-semibold">PAUSE</span> button in the header at any time to immediately halt new order submissions.
              </p>
            </div>
          </div>

          <div className="pt-2 border-t border-surface-border">
            <label className="text-[11px] font-mono text-slate-400 block mb-1.5">
              To confirm activation, please type <span className="text-red-400 font-bold select-all">CONFIRM LIVE TRADING</span> below:
            </label>
            <input
              type="text"
              value={liveRiskConfirmationInput}
              onChange={(e) => setLiveRiskConfirmationInput(e.target.value)}
              placeholder="CONFIRM LIVE TRADING"
              className="w-full px-3 py-2 bg-surface border border-surface-border rounded-md text-slate-100 font-mono text-xs focus:outline-none focus:ring-1 focus:ring-red-500 placeholder-slate-600"
            />
          </div>
        </div>

        <div className="flex items-center justify-end space-x-3 pt-3 border-t border-surface-border">
          <button
            type="button"
            onClick={() => {
              setShowLiveRiskModal(false);
              setLiveRiskConfirmationInput("");
            }}
            className="px-4 py-2 text-xs font-semibold text-slate-400 hover:text-slate-200 transition-colors"
          >
            Cancel (Keep Paper)
          </button>
          <button
            type="button"
            disabled={liveRiskConfirmationInput !== "CONFIRM LIVE TRADING"}
            onClick={() => {
              if (liveRiskConfirmationInput === "CONFIRM LIVE TRADING") {
                setLiveRiskAgreed(true);
                setTradingMode("real");
                setShowLiveRiskModal(false);
                setLiveRiskConfirmationInput("");
              }
            }}
            className={`px-4 py-2 text-xs font-bold rounded-lg uppercase tracking-wider transition-all ${
              liveRiskConfirmationInput === "CONFIRM LIVE TRADING"
                ? "bg-red-600 hover:bg-red-500 text-white shadow-lg shadow-red-900/30 cursor-pointer"
                : "bg-red-950/40 text-red-500/40 border border-red-900/30 cursor-not-allowed"
            }`}
          >
            Activate Live Trading
          </button>
        </div>
      </div>
    </div>
  )}
</div>
  );
}, areEngineSettingsPropsEqual);
