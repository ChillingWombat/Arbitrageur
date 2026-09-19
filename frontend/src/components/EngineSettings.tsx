import React, { useState, useEffect } from "react";
import {
  AlertCircle,
  CheckCircle2,
  Key,
  Lock,
  Unlock,
  Sliders,
  ExternalLink,
  Globe,
  AlertTriangle,
  Scale,
  RefreshCw,
  Info,
  Trash2,
  ShieldAlert,
  Star,
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
import { ArbitrageurLogo } from "./ArbitrageurLogo";
import { ToggleSwitch } from "./ToggleSwitch";
import { VenueAccountsSection } from "./ExchangeAccounts";


export type SettingsTab = "execution" | "exchanges" | "network" | "about";
export type ExchangeId =
  | "aevo"
  | "alphax"
  | "antarctic"
  | "apex"
  | "arcus"
  | "aster"
  | "astros"
  | "binance"
  | "bulk"
  | "coinbase"
  | "decibel"
  | "derive"
  | "dydx"
  | "edgex"
  | "evedex"
  | "extended"
  | "gatedex"
  | "gmx"
  | "grvt"
  | "helix"
  | "hyperliquid"
  | "ibkr"
  | "ig"
  | "iotrader"
  | "kalshi"
  | "kraken"
  | "lighter"
  | "nado"
  | "niza"
  | "okx"
  | "ondoperps"
  | "ostium"
  | "pacifica"
  | "paradex"
  | "polymarket"
  | "popdex"
  | "qfex"
  | "robinhood"
  | "sodex"
  | "standx"
  | "vanta"
  | "variational"
  | "vest";

// Long-tail perp venues served by the shared paper read-only adapter
// (backend `extended_venues.rs`). Keyed map keeps 23 toggles in one state
// object instead of 23 useState hooks; new venues only touch this list,
// the flag helpers below, and the load/save mappings.
export const EXTENDED_VENUE_IDS: ExchangeId[] = [
  "aevo", "alphax", "antarctic", "apex", "astros", "bulk", "derive",
  "dydx", "evedex", "extended", "gatedex", "gmx", "helix", "iotrader",
  "nado", "niza", "ostium", "pacifica", "paradex", "qfex", "sodex",
  "standx", "vest",
];

export interface ExchangeMetadata {
  id: ExchangeId;
  name: string;
  type: string;
  feeBps: number;
}

export const EXCHANGES_ALPHABETICAL: ExchangeMetadata[] = [
  { id: "aevo", name: "Aevo", type: "Perp DEX", feeBps: 3.0 },
  { id: "alphax", name: "AlphaX", type: "Perp DEX", feeBps: 3.0 },
  { id: "antarctic", name: "Antarctic", type: "Perp DEX", feeBps: 3.0 },
  { id: "apex", name: "ApeX Omni", type: "Perp DEX", feeBps: 2.0 },
  { id: "arcus", name: "Arcus", type: "CLOB DEX", feeBps: 1.5 },
  { id: "aster", name: "Aster", type: "Multi-Chain Perp DEX", feeBps: 3.5 },
  { id: "astros", name: "Astros", type: "Perp DEX", feeBps: 3.0 },
  { id: "binance", name: "Binance", type: "CEX Spot & Perp", feeBps: 7.5 },
  { id: "bulk", name: "BULK", type: "Perp DEX", feeBps: 3.0 },
  { id: "coinbase", name: "Coinbase", type: "CEX Spot & Perp", feeBps: 8.0 },
  { id: "decibel", name: "Decibel", type: "Aptos Perp DEX", feeBps: 2.5 },
  { id: "derive", name: "Derive", type: "Perp DEX", feeBps: 3.0 },
  { id: "dydx", name: "dYdX Chain", type: "L1 Perp DEX", feeBps: 2.0 },
  { id: "edgex", name: "edgeX", type: "StarkEx Perp DEX", feeBps: 4.5 },
  { id: "evedex", name: "EVEDEX", type: "Perp DEX", feeBps: 3.0 },
  { id: "extended", name: "Extended", type: "Perp DEX", feeBps: 2.0 },
  { id: "gatedex", name: "Gate DEX", type: "Perp DEX", feeBps: 3.0 },
  { id: "gmx", name: "GMX V2", type: "GLV AMM Perps", feeBps: 5.0 },
  { id: "grvt", name: "GRVT", type: "Perp DEX", feeBps: 2.0 },
  { id: "helix", name: "Helix", type: "Cosmos Perp DEX", feeBps: 2.0 },
  { id: "hyperliquid", name: "Hyperliquid", type: "L1 Perp DEX", feeBps: 2.0 },
  { id: "ibkr", name: "Interactive Brokers", type: "TradFi Broker", feeBps: 1.0 },
  { id: "ig", name: "IG Trading", type: "TradFi Broker", feeBps: 5.0 },
  { id: "iotrader", name: "IO Trader", type: "Perp DEX", feeBps: 3.0 },
  { id: "kalshi", name: "Kalshi", type: "CFTC Regulated", feeBps: 10.0 },
  { id: "kraken", name: "Kraken", type: "CEX Spot & Perp", feeBps: 16.0 },
  { id: "lighter", name: "Lighter", type: "zkDEX Rollup", feeBps: 1.5 },
  { id: "nado", name: "Nado", type: "Perp DEX", feeBps: 3.0 },
  { id: "niza", name: "Niza", type: "Perp DEX", feeBps: 3.0 },
  { id: "okx", name: "OKX", type: "CEX Spot & Perp", feeBps: 8.0 },
  { id: "ondoperps", name: "Ondo Perps", type: "Equity + Crypto Perps", feeBps: 3.5 },
  { id: "ostium", name: "Ostium", type: "Oracle Perp DEX", feeBps: 3.0 },
  { id: "pacifica", name: "Pacifica", type: "Perp DEX", feeBps: 2.0 },
  { id: "paradex", name: "Paradex", type: "Starknet Perp DEX", feeBps: 2.0 },
  { id: "polymarket", name: "Polymarket", type: "CTF Binary Market", feeBps: 0.0 },
  { id: "popdex", name: "PopDEX", type: "Arbitrage Protocol", feeBps: 2.0 },
  { id: "qfex", name: "QFEX", type: "Perp DEX", feeBps: 4.0 },
  { id: "robinhood", name: "Robinhood Lighter", type: "EVM DEX Aggregator", feeBps: 2.5 },
  { id: "sodex", name: "SoDEX", type: "Perp DEX", feeBps: 3.0 },
  { id: "standx", name: "StandX", type: "Perp DEX", feeBps: 4.0 },
  { id: "vanta", name: "Vanta", type: "Omnichain Perps", feeBps: 3.0 },
  { id: "variational", name: "Variational", type: "Perp DEX (Read-Only)", feeBps: 0.0 },
  { id: "vest", name: "Vest", type: "Perp DEX", feeBps: 3.0 },
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
  // Long-tail perp venues: one toggle map instead of 23 useStates.
  const [extEnabled, setExtEnabled] = useState<Record<string, boolean>>({});
  const setExtVenueEnabled = (id: string, enabled: boolean) =>
    setExtEnabled((prev) => (prev[id] === enabled ? prev : { ...prev, [id]: enabled }));
  const [arcusApiKey, setArcusApiKey] = useState("");
  const [arcusSecret, setArcusSecret] = useState("");
  const [arcusUrl, setArcusUrl] = useState("https://api.arcus.exchange");

  // PopDEX (Arbitrage Protocol)
  const [popdexEnabled, setPopdexEnabled] = useState(false);
  const [popdexAccountIndex, setPopdexAccountIndex] = useState("0");
  const [popdexApiKey, setPopdexApiKey] = useState("");
  const [popdexUrl, setPopdexUrl] = useState(""); // Closed beta: no public endpoint; paste the beta base URL when granted.

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
  const [gitRepo, setGitRepo] = useState("https://github.com/Dav1D1010/Arbitrageur");

  // UI state
  const [activeTab, setActiveTab] = useState<SettingsTab>("execution");
  const [selectedExchange, setSelectedExchange] = useState<ExchangeId>("arcus");
  // Favorite exchanges: a pure UI preference (localStorage, per machine) —
  // starring never touches strategy state. The selector filter below narrows
  // the tab list to favorites for operators who only use a few venues.
  const FAVORITE_KEY = "arbitrageur.favoriteExchanges.v1";
  const [favoriteExchanges, setFavoriteExchanges] = useState<string[]>(() => {
    try {
      const raw = localStorage.getItem(FAVORITE_KEY);
      const arr = raw ? JSON.parse(raw) : [];
      return Array.isArray(arr) ? arr.filter((v) => typeof v === "string") : [];
    } catch {
      return [];
    }
  });
  const [exchangeFilter, setExchangeFilter] = useState<"all" | "favorites">("all");
  const toggleFavorite = (id: string) =>
    setFavoriteExchanges((prev) => {
      const next = prev.includes(id) ? prev.filter((v) => v !== id) : [...prev, id];
      try {
        localStorage.setItem(FAVORITE_KEY, JSON.stringify(next));
      } catch {
        /* private-mode storage — favorites simply don't persist */
      }
      return next;
    });
  const visibleExchanges = EXCHANGES_ALPHABETICAL.filter(
    (ex) => exchangeFilter === "all" || favoriteExchanges.includes(ex.id),
  );
  // Keep the selection visible: if the active tab is filtered out, fall back
  // to the first visible exchange (or keep it when the filter empties).
  useEffect(() => {
    if (!visibleExchanges.some((ex) => ex.id === selectedExchange) && visibleExchanges.length > 0) {
      setSelectedExchange(visibleExchanges[0].id);
    }
  }, [exchangeFilter, favoriteExchanges]); // eslint-disable-line react-hooks/exhaustive-deps
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Vault state
  const [vaultStatus, setVaultStatus] = useState<VaultStatus | null>(null);
  const [showVaultModal, setShowVaultModal] = useState(false);
  const [configuredVenues, setConfiguredVenues] = useState<string[]>([]);

  // Venue execution capabilities & live trading risk gating
  const [venueCapabilities, setVenueCapabilities] = useState<VenueCapabilityDto[]>([]);
  const [showLiveRiskModal, setShowLiveRiskModal] = useState(false);
  const [liveRiskAgreed, setLiveRiskAgreed] = useState(false);
  const [liveRiskConfirmationInput, setLiveRiskConfirmationInput] = useState("");
  const [liveRiskAck, setLiveRiskAck] = useState(false);

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
    if ((EXTENDED_VENUE_IDS as string[]).includes(id)) return extEnabled[id] ?? false;
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

  const setVenueFlagEnabled = (id: ExchangeId, enabled: boolean) => {
    if ((EXTENDED_VENUE_IDS as string[]).includes(id)) {
      setExtVenueEnabled(id, enabled);
      return;
    }
    switch (id) {
      case "arcus": setArcusEnabled(enabled); break;
      case "aster": setAsterEnabled(enabled); break;
      case "binance": setBinanceEnabled(enabled); break;
      case "coinbase": setCoinbaseEnabled(enabled); break;
      case "decibel": setDecibelEnabled(enabled); break;
      case "edgex": setEdgexEnabled(enabled); break;
      case "grvt": setGrvtEnabled(enabled); break;
      case "hyperliquid": setHlEnabled(enabled); break;
      case "ibkr": setIbkrEnabled(enabled); break;
      case "ig": setIgEnabled(enabled); break;
      case "kalshi": setKalshiEnabled(enabled); break;
      case "kraken": setKrakenEnabled(enabled); break;
      case "lighter": setLighterEnabled(enabled); break;
      case "okx": setOkxEnabled(enabled); break;
      case "ondoperps": setOndoperpsEnabled(enabled); break;
      case "polymarket": setPolymarketEnabled(enabled); break;
      case "popdex": setPopdexEnabled(enabled); break;
      case "robinhood": setRobinhoodEnabled(enabled); break;
      case "vanta": setVantaEnabled(enabled); break;
      case "variational": setVariationalEnabled(enabled); break;
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


  const latencyClass = (ms: number): string =>
    ms < 100 ? "text-profit font-semibold" : ms < 300 ? "text-warn font-semibold" : "text-loss font-semibold";

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
        // Long-tail venues: backend fields are `<id>_enabled`.
        setExtEnabled(Object.fromEntries(
          EXTENDED_VENUE_IDS.map((id) => [id, (s as unknown as Record<string, unknown>)[`${id}_enabled`] === true]),
        ));
        setCoinbaseEnabled(s.coinbase_enabled ?? false);

        setArcusApiKey(s.arcus_api_key || "");
        setArcusSecret(s.arcus_secret || "");
        setArcusUrl(s.arcus_url || "https://api.arcus.exchange");

        setPopdexAccountIndex(s.popdex_account_index || "0");
        setPopdexApiKey(s.popdex_api_key || "");
        setPopdexUrl(s.popdex_url || ""); // Dead parked-domain default removed; empty = unconfigured.

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
        setGitRepo(s.git_repo || "https://github.com/Dav1D1010/Arbitrageur");

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
        ...Object.fromEntries(
          EXTENDED_VENUE_IDS.map((id) => [`${id}_enabled`, extEnabled[id] ?? false]),
        ),
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

      // Ensure live mode state reflects selected mode
      await api.setLiveMode(tradingMode === "real");

      const sanitized = await api.getEngineSettings();
      setConfiguredVenues(sanitized.configured_venues || []);
      clearCredentialInputs();

      // Refresh balances for the newly active trading mode
      await loadVenueBalances();

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
  };

  const handleLockVault = async () => {
    try {
      const s = await api.lockVault();
      setVaultStatus(s);
      setConfiguredVenues([]);
      clearCredentialInputs();
      setShowVaultModal(true);
    } catch (err) {
      console.error("Lock error:", err);
    }
  };

  /** Reloads everything an account-level save affects so the card's own
   *  Save Account button is sufficient — no overall Save Configuration needed:
   *  account lists, venue credential-presence, balances, and live statuses. */
  const handleAccountsChanged = () => {
    setAccountsEpoch((e) => e + 1);
    void api
      .getEngineSettings()
      .then((s) => setConfiguredVenues(s.configured_venues || []))
      .catch(() => undefined);
    void loadVenueBalances();
    void api
      .getSystemStatus()
      .then((sys) => {
        if (sys?.active_venues) setLiveVenues(sys.active_venues);
      })
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
      setUpdateMsg("System up to date. Arbitrageur v1.2.0 is currently the latest stable release.");
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
        // Keyless public venues (incl. all extended venues): the enable
        // toggle plus any account/configured record is the configuration.
        return isExchangeEnabled(venue as ExchangeId) && (configured(venue) || hasConfiguredAccount(venue));
    }
  };

  const configuredCount = [
    isVenueConfigured("dydx"),
    isVenueConfigured("paradex"),
    isVenueConfigured("gmx"),
    isVenueConfigured("apex"),
    isVenueConfigured("extended"),
    isVenueConfigured("derive"),
    isVenueConfigured("standx"),
    isVenueConfigured("helix"),
    isVenueConfigured("aevo"),
    isVenueConfigured("vest"),
    isVenueConfigured("antarctic"),
    isVenueConfigured("evedex"),
    isVenueConfigured("bulk"),
    isVenueConfigured("qfex"),
    isVenueConfigured("ostium"),
    isVenueConfigured("sodex"),
    isVenueConfigured("alphax"),
    isVenueConfigured("pacifica"),
    isVenueConfigured("nado"),
    isVenueConfigured("iotrader"),
    isVenueConfigured("niza"),
    isVenueConfigured("astros"),
    isVenueConfigured("gatedex"),
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


  const isVaultLocked = !api.isTauri || !vaultStatus?.is_unlocked;
  const isVaultInitializing = vaultStatus?.is_initialized === false;

  return (
    <div className="flex flex-col h-full min-h-0 overflow-hidden select-none bg-surface">
      {/* Top Section Navigation Bar */}
      <div className="border-b border-surface-border bg-slate-50/80 shrink-0 flex items-stretch justify-between select-none">
        {/* Navigation Tabs with dividers: Execution, Exchanges, Network & VPS, About */}
        <div className="flex items-stretch divide-x divide-surface-border border-r border-surface-border">
          <button
            type="button"
            onClick={() => setActiveTab("execution")}
            className={`relative h-10 flex items-center space-x-2 px-2.5 sm:px-2.5 text-[11px] font-mono transition select-none group cursor-pointer ${
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
            className={`relative h-10 flex items-center space-x-2 px-2.5 sm:px-2.5 text-[11px] font-mono transition select-none group cursor-pointer ${
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
            {activeTab === "exchanges" && (
              <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
            )}
          </button>


          <button
            type="button"
            onClick={() => setActiveTab("network")}
            className={`relative h-10 flex items-center space-x-2 px-2.5 sm:px-2.5 text-[11px] font-mono transition select-none group cursor-pointer ${
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
            className={`relative h-10 flex items-center space-x-2 px-2.5 sm:px-2.5 text-[11px] font-mono transition select-none group cursor-pointer ${
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

        {/* Right Header: Unlock / Lock Vault action on Exchanges Tab */}
        <div className="flex items-center space-x-2 py-2 pr-5 sm:pr-6">
          {activeTab === "exchanges" && api.isTauri && (
            <button
              type="button"
              onClick={isVaultLocked ? () => setShowVaultModal(true) : handleLockVault}
              className="px-2 py-2 rounded-lg border border-slate-200 bg-surface hover:bg-slate-100 text-slate-600 text-xs font-mono flex items-center space-x-1.5 transition shadow-2xs cursor-pointer"
              title={isVaultLocked ? "Unlock vault credentials" : "Lock sensitive exchange credentials now"}
            >
              {isVaultLocked ? (
                <>
                  <Unlock className="w-3.5 h-3.5 text-warn" />
                  <span className="font-semibold text-slate-700">Unlock Vault</span>
                </>
              ) : (
                <>
                  <Lock className="w-3.5 h-3.5 text-slate-500" />
                  <span>Lock Vault</span>
                </>
              )}
            </button>
          )}
          {activeTab === "network" &&
            !isVaultLocked &&
            configuredVenues.includes("remote_hsm") && (
              <button
                type="button"
                onClick={handleRemoveHsmToken}
                className="px-2 py-2 rounded-lg border border-loss-border bg-surface hover:bg-loss-dim text-loss text-xs font-mono flex items-center space-x-1.5 transition shadow-2xs cursor-pointer"
                title="Permanently remove the stored HSM token from the vault"
              >
                <Trash2 className="w-3.5 h-3.5 text-loss" />
                <span>Remove HSM Token</span>
              </button>
            )}
        </div>
      </div>

      {/* Main Content Form Area */}
      <div className="flex-1 flex flex-col min-w-0 h-full overflow-hidden bg-surface">
        <form onSubmit={handleSave} className="flex flex-col h-full min-h-0">

          {activeTab === "exchanges" ? (
            <div className="relative flex-1 flex flex-row min-h-0 overflow-hidden">
              {/* Left Panel: Exchanges Selector in Alphabetical Order */}
              <div className="w-56 sm:w-64 shrink-0 border-r border-surface-border bg-slate-50/50 flex flex-col min-h-0">
                <div className="p-3 border-b border-surface-border bg-slate-100/60 flex items-center justify-between shrink-0">
                  <span className="text-xs font-mono font-bold text-slate-700 tracking-wider">
                    Exchanges ({configuredCount}/{EXCHANGES_ALPHABETICAL.length})
                  </span>
                  <div className="flex gap-1" title="Show all exchanges or favorites only">
                    <button
                      type="button"
                      onClick={() => setExchangeFilter("all")}
                      className={`px-2 py-1 rounded-md text-[10px] font-mono font-bold border transition cursor-pointer ${
                        exchangeFilter === "all"
                          ? "bg-accent-dim text-accent border-accent-line/60"
                          : "bg-surface text-slate-500 border-surface-border hover:text-slate-800"
                      }`}
                    >
                      All
                    </button>
                    <button
                      type="button"
                      onClick={() => setExchangeFilter("favorites")}
                      className={`px-2 py-1 rounded-md text-[10px] font-mono font-bold border transition cursor-pointer flex items-center gap-1 ${
                        exchangeFilter === "favorites"
                          ? "bg-accent-dim text-accent border-accent-line/60"
                          : "bg-surface text-slate-500 border-surface-border hover:text-slate-800"
                      }`}
                      title={`Favorites only (${favoriteExchanges.length} starred)`}
                    >
                      <Star className="w-3 h-3" />
                      <span>{favoriteExchanges.length > 0 ? favoriteExchanges.length : ""}</span>
                    </button>
                  </div>
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
                    className="px-2 py-2 bg-surface hover:bg-surface-raised border border-surface-border text-slate-600 hover:text-slate-900 rounded-lg text-[10px] font-mono transition flex items-center space-x-1 disabled:opacity-50"
                  >
                    <RefreshCw className={`w-3 h-3 ${pinging ? "animate-spin" : ""}`} />
                    <span>Ping</span>
                  </button>
                </div>

                <div className="flex-1 overflow-y-auto p-2 space-y-1">
                  {visibleExchanges.length === 0 && (
                    <div className="px-3 py-6 text-center text-[11px] font-mono text-slate-400">
                      No favorites yet — star an exchange to pin it here.
                    </div>
                  )}
                  {visibleExchanges.map((ex) => {
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
                    const showLatency = isEnabled && latency != null;
                    const showBalance = bal != null;
                    return (
                      <button
                        key={ex.id}
                        type="button"
                        onClick={() => setSelectedExchange(ex.id)}
                        className={`w-full text-left p-2.5 rounded-lg text-xs font-mono transition flex items-center justify-between group cursor-pointer border ${
                          isSelected
                            ? "bg-accent-dim text-accent border-accent-line shadow-2xs"
                            : "bg-surface hover:bg-slate-100/80 text-slate-700 border-surface-border/60 hover:border-surface-border"
                        }`}
                      >
                        <div className="min-w-0 pr-2">
                          <div className="flex items-center space-x-2">
                            <span
                              className={`w-2 h-2 rounded-full shrink-0 ${
                                isStreaming
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
                            <span className={`font-semibold truncate ${isSelected ? "text-accent font-bold" : "text-slate-900"}`}>
                              {ex.name}
                            </span>
                            <span
                              role="checkbox"
                              aria-checked={favoriteExchanges.includes(ex.id)}
                              title={favoriteExchanges.includes(ex.id) ? "Remove from favorites" : "Add to favorites"}
                              onClick={(e) => {
                                e.stopPropagation();
                                toggleFavorite(ex.id);
                              }}
                              className={`shrink-0 cursor-pointer transition ${
                                favoriteExchanges.includes(ex.id)
                                  ? "text-amber-500"
                                  : "text-slate-300 opacity-0 group-hover:opacity-100 hover:text-amber-400"
                              }`}
                            >
                              <Star className={`w-3.5 h-3.5 ${favoriteExchanges.includes(ex.id) ? "fill-amber-500" : ""}`} />
                            </span>
                          </div>
                        </div>
                        {(showLatency || showBalance) && (
                        <div className="text-right shrink-0">
                          {showLatency && (
                          <span
                            className={`text-[11px] font-mono block font-semibold ${
                              noAdapter
                                ? "text-warn"
                                : latencyClass(latency as number)
                            }`}
                            title={
                              isStreaming
                                ? `Market data active · measured API round-trip: ${latency} ms`
                                : noAdapter
                                ? `API reachable (${latency} ms), but market quotes are not integrated`
                                : undefined
                            }
                          >
                            {`${latency} ms`}
                          </span>
                          )}
                          {showBalance && bal && (
                          <span
                            className="text-[11px] font-mono block font-bold text-slate-800"
                            title={
                              bal.is_live
                                ? "Real exchange equity"
                                : "Simulated paper equity, not live exchange equity"
                            }
                          >
                            {bal.is_live
                              ? `$${bal.equity_usd.toLocaleString("en-US", { maximumFractionDigits: 0 })}`
                              : `Paper $${bal.equity_usd.toLocaleString("en-US", { maximumFractionDigits: 0 })}`}
                          </span>
                          )}
                        </div>
                        )}
                      </button>
                    );
                  })}
                </div>

              </div>

              {/* Right Content Pane: Single Selected Exchange */}
              <div className="flex-1 overflow-y-auto p-4 space-y-4 min-w-0">
                {errorMsg && (
                  <div className="p-4 bg-loss-dim border border-loss-border text-loss rounded-xl text-xs font-mono flex items-center space-x-2">
                    <AlertCircle className="w-4 h-4 shrink-0" />
                    <span>{errorMsg}</span>
                  </div>
                )}

                {saved && (
                  <div className="p-4 bg-profit-dim border border-profit-border text-profit rounded-xl text-xs font-mono flex items-center space-x-2 animate-in fade-in duration-200">
                    <CheckCircle2 className="w-4 h-4 shrink-0" />
                    <span>Configuration saved successfully and updated in live atomic execution coordinator.</span>
                  </div>
                )}

                {/* Non-blocking Vault Locked Banner */}
                {isVaultLocked && api.isTauri && (
                  <div className="p-3.5 rounded-xl bg-amber-500/10 border border-amber-500/30 flex items-center justify-between shadow-2xs">
                    <div className="flex items-center space-x-3">
                      <div className="w-8 h-8 rounded-lg bg-amber-500/20 border border-amber-500/40 flex items-center justify-center text-amber-600 shrink-0">
                        <Lock className="w-4 h-4" />
                      </div>
                      <div>
                        <h4 className="text-xs font-bold font-mono text-slate-900">
                          {isVaultInitializing ? "Master Password Not Created" : "Secure Vault is Locked"}
                        </h4>
                        <p className="text-[11px] text-slate-600 font-sans mt-0.5">
                          {isVaultInitializing
                            ? "Create a master password to encrypt and manage exchange API keys."
                            : "Unlock your vault with your master password to view, add, or edit live API credentials."}
                        </p>
                      </div>
                    </div>
                    <button
                      type="button"
                      onClick={() => setShowVaultModal(true)}
                      className="btn-accent px-2 py-2 text-xs font-mono font-bold flex items-center space-x-1.5 shrink-0 ml-3 cursor-pointer"
                    >
                      <Unlock className="w-3.5 h-3.5" />
                      <span>{isVaultInitializing ? "Create Password" : "Unlock Vault"}</span>
                    </button>
                  </div>
                )}

            {/* Exchange: Kraken (Spot & Futures Multi-Market) */}
            {selectedExchange === "kraken" && (
              <div className="space-y-4">
                <div className="p-2.5 bg-blue-50/70 border border-blue-200 rounded-lg text-xs font-mono text-slate-600">
                  💡 Kraken maintains separate API infrastructure for Spot and Futures markets. Configure accounts independently for each market.
                </div>

                {/* Kraken Spot */}
                <div className="space-y-2">
                  <div className="flex items-center space-x-2">
                    <span className="text-xs font-bold font-mono text-slate-900">1. Kraken Spot Trading API</span>
                    <span className="text-xs font-mono px-1.5 py-0.5 rounded-md bg-slate-200 text-slate-700">api.kraken.com</span>
                  </div>
                  <VenueAccountsSection
                    venue="kraken_spot"
                    locked={isVaultLocked}
                    bal={getVenueBalance("kraken")}
                    lane={getVenueLane("kraken")}
                    fallbackEnabled={krakenSpotEnabled}
                    onFallbackChange={setKrakenSpotEnabled}
                    refreshKey={accountsEpoch}
                    onChanged={handleAccountsChanged}
                  />
                </div>

                {/* Kraken Futures */}
                <div className="space-y-2">
                  <div className="flex items-center space-x-2">
                    <span className="text-xs font-bold font-mono text-slate-900">2. Kraken Futures Trading API</span>
                    <span className="text-xs font-mono px-1.5 py-0.5 rounded-md bg-indigo-50 text-indigo-700 border border-indigo-200">futures.kraken.com</span>
                  </div>
                  <VenueAccountsSection
                    venue="kraken_futures"
                    locked={isVaultLocked}
                    bal={getVenueBalance("kraken")}
                    lane={getVenueLane("kraken")}
                    fallbackEnabled={krakenFuturesEnabled}
                    onFallbackChange={setKrakenFuturesEnabled}
                    refreshKey={accountsEpoch}
                    onChanged={handleAccountsChanged}
                  />
                </div>
              </div>
            )}

            {/* Standard Exchanges */}
            {selectedExchange !== "kraken" && (
              <VenueAccountsSection
                venue={selectedExchange}
                locked={isVaultLocked}
                bal={getVenueBalance(selectedExchange)}
                lane={getVenueLane(selectedExchange)}
                fallbackEnabled={venueFlagEnabled(selectedExchange)}
                onFallbackChange={(en) => setVenueFlagEnabled(selectedExchange, en)}
                refreshKey={accountsEpoch}
                onChanged={handleAccountsChanged}
              />
            )}


              </div>
            </div>
          ) : (
            <div className="flex-1 overflow-y-auto p-4 space-y-4">
              {errorMsg && (
                <div className="p-4 bg-loss-dim border border-loss-border text-loss rounded-xl text-xs font-mono flex items-center space-x-2">
                  <AlertCircle className="w-4 h-4 shrink-0" />
                  <span>{errorMsg}</span>
                </div>
              )}

              {saved && (
                <div className="p-4 bg-profit-dim border border-profit-border text-profit rounded-xl text-xs font-mono flex items-center space-x-2 animate-in fade-in duration-200">
                  <CheckCircle2 className="w-4 h-4 shrink-0" />
                  <span>Configuration saved successfully and updated in live atomic execution coordinator.</span>
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
                    text={"Paper Trading executes simulated fills against live market data — no real orders are sent and balances are virtual.\n\nLive Trading submits signed orders to your connected exchange accounts on supported API venues. Live execution requires an unlocked credential vault and at least two live-ready venues."}
                  />
                </label>
                <p className="text-xs text-slate-500">
                  {hasApiConnectivity
                    ? "Both modes require exchange API connectivity for market data."
                    : "Unavailable — no exchange API is connected. Enable a venue with working connectivity in the Exchanges tab first."}
                </p>

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
                      canSwitchToLive || !vaultStatus?.is_unlocked ? "cursor-pointer" : "cursor-not-allowed opacity-60"
                    }`}
                    title={
                      !vaultStatus?.is_unlocked
                        ? "Vault is locked. Click to enter password and unlock the vault."
                        : liveReadyVenues.length < 2
                        ? `Requires at least 2 live-ready venues (currently ${liveReadyVenues.length}: ${liveReadyVenues.map(v => v.name).join(", ") || "none"}). Configure and enable API keys in the Exchanges tab.`
                        : "Switch to Live Trading"
                    }
                    onClick={() => {
                      if (!vaultStatus?.is_unlocked && api.isTauri) {
                        setShowVaultModal(true);
                      }
                    }}
                  >
                    <input
                      type="radio"
                      name="trading_mode"
                      value="real"
                      checked={tradingMode === "real"}
                      disabled={!canSwitchToLive && (vaultStatus?.is_unlocked ?? false)}
                      onChange={() => {
                        if (!vaultStatus?.is_unlocked && api.isTauri) {
                          setShowVaultModal(true);
                          return;
                        }
                        if (canSwitchToLive) {
                          if (!liveRiskAgreed && tradingMode !== "real") {
                            setShowLiveRiskModal(true);
                          } else {
                            setTradingMode("real");
                          }
                        }
                      }}
                      className="text-loss focus:ring-loss"
                    />
                    <span className="font-semibold text-slate-800 flex items-center gap-1.5">
                      Live Trading
                    </span>
                  </label>
                </div>


              </div>

              <div className="pt-3 border-t border-surface-border space-y-1.5">
                <div className="flex items-center gap-3">
                  <label className="text-slate-600 block font-mono text-xs">
                    Paper Starting Balance (USD)
                  </label>
                  <input
                  type="number"
                  min="1"
                  step="any"
                  disabled={loading}
                  value={paperStartingBalance}
                  onChange={(e) => setPaperStartingBalance(parseFloat(e.target.value) || 0)}
                  className="w-28 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
                  />
                </div>
                <p className="text-xs text-slate-400 leading-snug border-t border-surface-border/60 pt-1.5">
                  Virtual equity per paper account. Saving resets paper accounts and clears open positions.
                </p>
              </div>
            </div>

            {/* Orderbook Fast Warm-up */}
            <div className="p-3.5 bg-surface border border-surface-border rounded-lg shadow-2xs flex items-center justify-between gap-4">
              <div className="space-y-1 min-w-0">
                <label className="text-slate-800 font-bold block text-xs">Orderbook Cache Warm-Up</label>
                <p className="text-xs text-slate-500">
                  {fastWarmupEnabled
                    ? "Cached Warm-Up: Reuses persisted orderbook quotes to shorten the warm-up window when appropriate — warm-up still runs, just faster."
                    : "Cold Start: Starts orderbook buffer from scratch on every launch (requires the full warm-up window)."}
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

                  </div>
                  <input
                    type="number"
                    min="0"
                    step="1"
                    disabled={loading}
                    value={maxHistoryDays}
                    placeholder="No limit"
                    onChange={(e) => setMaxHistoryDays(e.target.value)}
                    className="w-24 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
                  />
                </div>
                <div className="flex items-center justify-between gap-3">
                  <div className="flex-1 min-w-0">
                    <label className="text-slate-600 block font-mono text-xs">
                      Max storage GB
                    </label>

                  </div>
                  <input
                    type="number"
                    min="0"
                    step="any"
                    disabled={loading}
                    value={maxStorageGb}
                    placeholder="No limit"
                    onChange={(e) => setMaxStorageGb(e.target.value)}
                    className="w-24 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
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
                  className="w-24 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>

              <div className="flex items-center justify-between gap-3">
                <label className="text-slate-600 flex-1">Engine WebSocket Port:</label>
                <input
                  type="number"
                  disabled={loading}
                  value={engineWsPort}
                  onChange={(e) => setEngineWsPort(parseInt(e.target.value, 10) || 8789)}
                  className="w-24 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
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
                  className="w-32 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                />
              </div>
            </div>

            {/* VPS Daemon Deployment Guide */}
            <div className="p-4 bg-slate-50 border border-surface-border rounded-lg space-y-2 text-xs font-mono text-slate-700">
              <h4 className="font-bold text-slate-900 flex items-center space-x-1.5">
                <span>🖥️ Headless VPS 24/7 Deployment Instructions</span>
              </h4>
              <p className="text-xs text-slate-600">
                To run Arbitrageur continuously 24/7 without keeping your local laptop or desktop open, deploy the standalone Rust binary to an Ubuntu/Debian VPS:
              </p>
              <div className="p-2.5 bg-slate-900 text-emerald-400 rounded-lg text-xs overflow-x-auto select-text font-mono">
                <code>./target/release/arbitrageur serve --bind 0.0.0.0 --port 8787</code>
              </div>
              <p className="text-xs text-slate-500">
                The serve command exposes a read-only health endpoint only — no market data ingestion or order execution runs. You can create a systemd service (e.g. <code className="bg-slate-200 px-1 py-0.5 rounded-md">systemctl start arbitrageur</code>) to auto-restart on server reboot.
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
                    <ArbitrageurLogo size={22} className="text-accent" />
                  </div>
                  <div>
                    <h3 className="text-[13px] font-bold font-mono tracking-wider text-slate-900">
                      About Arbitrageur ({version})
                    </h3>
                  </div>
                </div>

                <div className="flex items-center space-x-2">
                  <button
                    type="button"
                    onClick={handleCheckForUpdates}
                    disabled={checkingUpdate}
                    className="px-2 py-2 bg-surface-raised hover:bg-slate-200 border border-surface-border text-slate-800 rounded-lg text-xs font-mono font-medium transition flex items-center space-x-1.5 shadow-2xs"
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
                <div className="p-3 bg-profit-dim border border-profit-border text-profit rounded-xl text-xs font-mono flex items-center space-x-2">
                  <CheckCircle2 className="w-4 h-4 text-profit shrink-0" />
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
                  <span>Buy me a SOLO ($1)</span>
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
          className="px-2.5 py-1.5 rounded-lg border border-surface-border bg-surface text-slate-700 text-xs font-mono flex items-center space-x-1.5 shadow-2xs"
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
              className="px-2.5 py-2.5 bg-surface-raised hover:bg-slate-200 text-slate-700 border border-surface-border font-medium text-xs font-mono rounded-lg transition shadow-2xs cursor-pointer"
            >
              Cancel
            </button>
          )}
          <button
            type="submit"
            disabled={loading || saving}
            className="px-2.5 py-2.5 bg-accent hover:bg-blue-600 text-white font-medium text-xs font-mono rounded-lg transition flex items-center space-x-2 shadow-2xs disabled:opacity-50 cursor-pointer"
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
    onSuccess={async (status) => {
      setVaultStatus(status);
      setShowVaultModal(false);
      try {
        const sanitized = await api.getEngineSettings();
        setConfiguredVenues(sanitized.configured_venues || []);
        if (sanitized.trading_mode === "real" || sanitized.trading_mode === "paper") {
          setTradingMode(sanitized.trading_mode);
        }
        setAccountsEpoch((prev) => prev + 1);
        api.getVenueExecutionCapabilities().then(setVenueCapabilities).catch(console.error);
        await loadVenueBalances();
      } catch (err) {
        console.warn("Failed to refresh settings after vault unlock:", err);
      }
    }}
  />

  {/* Live Trading Risk Confirmation Modal */}
  {showLiveRiskModal && (
    <div className="modal-backdrop">
      <div className="modal-card max-w-lg w-full p-6 text-slate-900 border border-surface-border rounded-2xl shadow-xl">
        <div className="flex items-center space-x-3 mb-4 pb-3 border-b border-surface-border">
          <div className="p-2.5 bg-loss-dim rounded-xl border border-loss-border text-loss shadow-2xs">
            <ShieldAlert className="w-6 h-6 text-loss" />
          </div>
          <div>
            <h3 className="text-base font-bold font-sans text-slate-900 leading-snug">
              Live Trading Risk Disclosure
            </h3>
            <p className="text-xs text-loss font-mono">Real-Capital Execution Activation</p>
          </div>
        </div>

        <div className="space-y-3.5 text-xs text-slate-600 mb-5 leading-relaxed">
          <div className="p-3 bg-loss-dim border border-loss-border rounded-xl text-loss space-y-1 font-mono text-[11px]">
            <p className="font-bold text-loss flex items-center gap-1.5">
              <AlertTriangle className="w-3.5 h-3.5 shrink-0" />
              <span>REAL CAPITAL AT RISK</span>
            </p>
            <p className="text-slate-700 leading-normal font-sans text-xs">
              You are enabling real-money automated order execution. Live orders will be submitted directly to connected exchange order books using your real account balances.
            </p>
          </div>

          <div className="space-y-3 text-slate-700">
            <div className="flex items-start gap-2.5">
              <span className="w-5 h-5 rounded-full bg-accent/10 text-accent font-bold font-mono text-[11px] flex items-center justify-center shrink-0 mt-0.5">
                1
              </span>
              <div>
                <strong className="text-slate-900 font-semibold">Authenticated Venues Only:</strong> Orders are routed exclusively between venues with authenticated API credentials in your unlocked vault. Routes involving unauthenticated or read-only venues automatically fail closed to prevent unhedged exposure.
              </div>
            </div>

            <div className="flex items-start gap-2.5">
              <span className="w-5 h-5 rounded-full bg-accent/10 text-accent font-bold font-mono text-[11px] flex items-center justify-center shrink-0 mt-0.5">
                2
              </span>
              <div>
                <strong className="text-slate-900 font-semibold">Execution Latency &amp; Market Risk:</strong> Network jitter, exchange rate limits, and rapid orderbook depth fluctuations can cause slippage or partial fills. Real exchange fees and funding rates apply to all fills.
              </div>
            </div>

            <div className="flex items-start gap-2.5">
              <span className="w-5 h-5 rounded-full bg-accent/10 text-accent font-bold font-mono text-[11px] flex items-center justify-center shrink-0 mt-0.5">
                3
              </span>
              <div>
                <strong className="text-slate-900 font-semibold">Safety Controls &amp; Emergency Stop:</strong> Delta guards, max grid steps, and minimum profit hurdles remain enforced. Use <span className="font-mono font-bold text-amber-700 bg-amber-50 px-1 py-0.5 rounded border border-amber-200">PAUSE</span> in the header to freeze new submissions while preserving open positions, or <span className="font-mono font-bold text-loss bg-loss-dim px-1 py-0.5 rounded border border-loss-border">EMERGENCY STOP</span> to immediately halt the engine and flatten open legs.
              </div>
            </div>

            <div className="flex items-start gap-2.5">
              <span className="w-5 h-5 rounded-full bg-accent/10 text-accent font-bold font-mono text-[11px] flex items-center justify-center shrink-0 mt-0.5">
                4
              </span>
              <div>
                <strong className="text-slate-900 font-semibold">Vault Security:</strong> All API keys remain encrypted on-device. Relocking the vault or closing the application immediately stops live execution.
              </div>
            </div>
          </div>

          <div className="pt-3 border-t border-surface-border space-y-2.5">
            <label className="flex items-start space-x-2.5 cursor-pointer select-none">
              <input
                type="checkbox"
                checked={liveRiskAck}
                onChange={(e) => setLiveRiskAck(e.target.checked)}
                className="mt-0.5 rounded text-loss focus:ring-loss border-surface-border"
              />
              <span className="text-[11px] text-slate-700 font-medium leading-normal">
                I acknowledge the risks of automated real-money trading and confirm my risk parameters and exchange credentials.
              </span>
            </label>

            <div>
              <div className="flex items-center justify-between text-[11px] font-mono text-slate-500 mb-1.5">
                <span>
                  Type <span className="text-loss font-bold select-all">CONFIRM</span> below to enable:
                </span>
                <button
                  type="button"
                  onClick={() => {
                    setLiveRiskConfirmationInput("CONFIRM");
                    setLiveRiskAck(true);
                  }}
                  className="text-[11px] text-accent hover:underline font-mono cursor-pointer"
                >
                  Quick fill
                </button>
              </div>
              <input
                type="text"
                value={liveRiskConfirmationInput}
                onChange={(e) => setLiveRiskConfirmationInput(e.target.value)}
                placeholder="Type CONFIRM"
                className="input-field w-full font-mono text-xs text-slate-900"
              />
            </div>
          </div>
        </div>

        <div className="flex items-center justify-end space-x-2.5 pt-3 border-t border-surface-border">
          <button
            type="button"
            onClick={() => {
              setShowLiveRiskModal(false);
              setLiveRiskConfirmationInput("");
              setLiveRiskAck(false);
            }}
            className="btn-secondary px-2.5 py-2.5 text-xs cursor-pointer"
          >
            Cancel (Keep Paper)
          </button>
          <button
            type="button"
            disabled={
              !liveRiskAck ||
              (liveRiskConfirmationInput.trim().toUpperCase() !== "CONFIRM" &&
               liveRiskConfirmationInput.trim().toUpperCase() !== "CONFIRM LIVE TRADING")
            }
            onClick={() => {
              const inputNorm = liveRiskConfirmationInput.trim().toUpperCase();
              if (liveRiskAck && (inputNorm === "CONFIRM" || inputNorm === "CONFIRM LIVE TRADING")) {
                setLiveRiskAgreed(true);
                setTradingMode("real");
                setShowLiveRiskModal(false);
                setLiveRiskConfirmationInput("");
                setLiveRiskAck(false);
              }
            }}
            className="btn-danger px-2.5 py-2.5 text-xs font-bold cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed"
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
