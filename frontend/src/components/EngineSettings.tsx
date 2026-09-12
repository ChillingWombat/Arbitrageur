import React, { useState, useEffect } from "react";
import {
  AlertCircle,
  CheckCircle2,
  Eye,
  EyeOff,
  Key,
  Lock,
  Sliders,
  ExternalLink,
  Wallet,
  Globe,
  AlertTriangle,
  Scale,
  RefreshCw,
  Info,
  Trash2,
} from "lucide-react";
import { api } from "../services/api";
import type { EngineSettingsDto, SystemStatusDto, VenueStatusDto, VaultStatus, VenueBalanceInfo } from "../types";
import { VaultUnlockModal } from "./VaultUnlockModal";
import { InfoTooltip } from "./InfoTooltip";
import { ArbitragerLogo } from "./ArbitragerLogo";

export type SettingsTab = "execution" | "exchanges" | "network" | "about";
export type ExchangeId =
  | "arcus"
  | "hyperliquid"
  | "kalshi"
  | "kraken"
  | "lighter"
  | "polymarket"
  | "popdex"
  | "robinhood"
  | "vanta";

export interface ExchangeMetadata {
  id: ExchangeId;
  name: string;
  type: string;
  feeBps: number;
}

export const EXCHANGES_ALPHABETICAL: ExchangeMetadata[] = [
  { id: "arcus", name: "Arcus", type: "CLOB DEX", feeBps: 1.5 },
  { id: "hyperliquid", name: "Hyperliquid", type: "L1 Perp DEX", feeBps: 2.0 },
  { id: "kalshi", name: "Kalshi", type: "CFTC Regulated", feeBps: 10.0 },
  { id: "kraken", name: "Kraken", type: "CEX Spot & Perp", feeBps: 16.0 },
  { id: "lighter", name: "Lighter", type: "zkDEX Rollup", feeBps: 1.5 },
  { id: "polymarket", name: "Polymarket", type: "CTF Binary Market", feeBps: 0.0 },
  { id: "popdex", name: "PopDEX", type: "Arbitrage Protocol", feeBps: 2.0 },
  { id: "robinhood", name: "Robinhood", type: "EVM DEX Aggregator", feeBps: 2.5 },
  { id: "vanta", name: "Vanta (Orderly)", type: "Omnichain Perps", feeBps: 3.0 },
];

interface ToggleSwitchProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  label?: string;
  size?: "sm" | "md";
}

const ToggleSwitch: React.FC<ToggleSwitchProps> = ({
  checked,
  onChange,
  disabled = false,
  label,
  size = "md",
}) => {
  const isSm = size === "sm";
  return (
    <div className="inline-flex items-center space-x-2">
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        disabled={disabled}
        onClick={(e) => {
          e.preventDefault();
          e.stopPropagation();
          onChange(!checked);
        }}
        className={`relative inline-flex items-center rounded-full transition-colors focus:outline-none focus:ring-2 focus:ring-emerald-500/30 ${
          isSm ? "w-8 h-[18px]" : "w-10 h-[22px]"
        } ${disabled ? "opacity-40 cursor-not-allowed" : "cursor-pointer"} ${
          checked ? "bg-emerald-500 border border-emerald-600 shadow-xs" : "bg-white border border-slate-300 shadow-xs"
        }`}
      >
        <span
          className={`inline-block rounded-full shadow-xs transform transition-transform ${
            checked ? "bg-white" : "bg-slate-400"
          } ${
            isSm
              ? checked
                ? "w-3.5 h-3.5 translate-x-4"
                : "w-3.5 h-3.5 translate-x-0.5"
              : checked
              ? "w-4 h-4 translate-x-5"
              : "w-4 h-4 translate-x-0.5"
          }`}
        />
      </button>
      {label && <span className="text-[11px] font-mono text-slate-700">{label}</span>}
    </div>
  );
};

interface EngineSettingsProps {
  systemStatus?: SystemStatusDto | null;
  onClose?: () => void;
  onSettingsSaved?: (newSettings: EngineSettingsDto) => void;
}

export const EngineSettings: React.FC<EngineSettingsProps> = ({ systemStatus, onClose, onSettingsSaved }) => {
  const [liveVenues, setLiveVenues] = useState<VenueStatusDto[]>([]);

  // Remote HSM
  const [hsmUrl, setHsmUrl] = useState("http://127.0.0.1:8787/sign");
  const [hmacToken, setHmacToken] = useState("");

  // Guardrails & Execution Modes
  const [maxSlippageBps, setMaxSlippageBps] = useState(15.0);
  const [minDepthRatio, setMinDepthRatio] = useState(3.0);
  const [tradingMode, setTradingMode] = useState<"paper" | "real">("paper");
  const [fastWarmupEnabled, setFastWarmupEnabled] = useState(true);
  const [venueRebalanceThresholdUsd, setVenueRebalanceThresholdUsd] = useState(500.0);

  // Networking & VPS
  const [engineRpcPort, setEngineRpcPort] = useState(8787);
  const [engineWsPort, setEngineWsPort] = useState(8789);
  const [bindAddress, setBindAddress] = useState("127.0.0.1");

  // Exchange Enablement Toggles
  const [hlEnabled, setHlEnabled] = useState(false);
  const [hlSpotEnabled, setHlSpotEnabled] = useState(false);
  const [lighterEnabled, setLighterEnabled] = useState(false);
  const [lighterSpotEnabled, setLighterSpotEnabled] = useState(false);
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
  const [vantaUrl, setVantaUrl] = useState("https://api-evm.orderly.org");
  const [vantaSubaccountIndex, setVantaSubaccountIndex] = useState<string>("");

  // Robinhood EVM
  const [robinhoodWallet, setRobinhoodWallet] = useState("");
  const [robinhoodPrivateKey, setRobinhoodPrivateKey] = useState("");
  const [robinhoodRpcUrl, setRobinhoodRpcUrl] = useState("https://rpc.robinhood.network");
  const [robinhoodRouter, setRobinhoodRouter] = useState("0x89e5DB8B5aA49aA85AC63f691524311AEB649eba");
  const [robinhoodSubaccountIndex, setRobinhoodSubaccountIndex] = useState<string>("");

  // Arcus CLOB DEX
  const [arcusEnabled, setArcusEnabled] = useState(false);
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
  const [showSecrets, setShowSecrets] = useState<Record<string, boolean>>({});
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Vault state
  const [vaultStatus, setVaultStatus] = useState<VaultStatus | null>(null);
  const [showVaultModal, setShowVaultModal] = useState(false);
  const [inlinePasscode, setInlinePasscode] = useState("");
  const [inlineUnlocking, setInlineUnlocking] = useState(false);
  const [inlineVaultError, setInlineVaultError] = useState<string | null>(null);
  const [configuredVenues, setConfiguredVenues] = useState<string[]>([]);

  const handleInlineUnlock = async (e?: React.FormEvent) => {
    if (e) e.preventDefault();
    if (!inlinePasscode) return;
    setInlineUnlocking(true);
    setInlineVaultError(null);
    try {
      let updatedStatus: VaultStatus;
      if (vaultStatus && !vaultStatus.is_initialized) {
        updatedStatus = await api.initializeVault(inlinePasscode);
      } else {
        updatedStatus = await api.unlockVault(inlinePasscode);
      }
      setVaultStatus(updatedStatus);
      setInlinePasscode("");
      const sanitized = await api.getEngineSettings();
      setConfiguredVenues(sanitized.configured_venues || []);
    } catch (err) {
      setInlineVaultError(err instanceof Error ? err.message : "Incorrect master passcode");
    } finally {
      setInlineUnlocking(false);
    }
  };

  // Venue Balances
  const [venueBalances, setVenueBalances] = useState<VenueBalanceInfo[]>([]);
  const [loadingBalances, setLoadingBalances] = useState(false);

  // Update check
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [updateMsg, setUpdateMsg] = useState<string | null>(null);

  const isExchangeEnabled = (id: ExchangeId): boolean => {
    switch (id) {
      case "arcus": return arcusEnabled;
      case "hyperliquid": return hlEnabled;
      case "kalshi": return kalshiEnabled;
      case "kraken": return krakenEnabled;
      case "lighter": return lighterEnabled;
      case "polymarket": return polymarketEnabled;
      case "popdex": return popdexEnabled;
      case "robinhood": return robinhoodEnabled;
      case "vanta": return vantaEnabled;
      default: return false;
    }
  };

  const getVenueBalance = (id: string): VenueBalanceInfo => {
    const found = venueBalances.find(
      (b) => b.venue.toLowerCase() === id.toLowerCase()
    );
    if (found) return found;
    return {
      venue: id,
      balance_usd: 0,
      equity_usd: 0,
      available_margin_usd: 0,
      margin_rate_pct: 0,
      min_required_usd: venueRebalanceThresholdUsd,
      needs_rebalance: false,
      status: "UNKNOWN",
      recommended_action: null,
    };
  };

  const toggleSecret = (field: string) => {
    setShowSecrets((prev) => ({ ...prev, [field]: !prev[field] }));
  };

  const loadVaultStatus = async () => {
    try {
      const status = await api.getVaultStatus();
      setVaultStatus(status);
    } catch (err) {
      console.warn("Could not fetch vault status:", err);
    }
  };

  const loadVenueBalances = async () => {
    setLoadingBalances(true);
    try {
      const data = await api.getVenueBalances();
      setVenueBalances(data);
    } catch (err) {
      console.error("Failed to load balances:", err);
    } finally {
      setLoadingBalances(false);
    }
  };

  useEffect(() => {
    const loadSettings = async () => {
      try {
        const s: EngineSettingsDto = await api.getEngineSettings();
        setConfiguredVenues(s.configured_venues || []);
        setHsmUrl(s.hsm_url || "http://127.0.0.1:8787/sign");
        setHmacToken(s.hmac_token || "");
        setMaxSlippageBps(s.max_slippage_bps ?? 15.0);
        setMinDepthRatio(s.min_depth_ratio ?? 3.0);

        setEngineRpcPort(s.engine_rpc_port ?? 8787);
        setEngineWsPort(s.engine_ws_port ?? 8789);
        setBindAddress(s.bind_address || "127.0.0.1");

        setTradingMode(s.trading_mode === "real" ? "real" : "paper");
        setFastWarmupEnabled(s.fast_warmup_enabled ?? true);
        setVenueRebalanceThresholdUsd(s.venue_rebalance_threshold_usd ?? 500.0);

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
        setVantaUrl(s.vanta_url || "https://api-evm.orderly.org");
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
        setHlSpotEnabled(s.hyperliquid_spot_enabled ?? false);
        setLighterEnabled(s.lighter_enabled ?? false);
        setLighterSpotEnabled(s.lighter_spot_enabled ?? false);
        setVantaEnabled(s.vanta_enabled ?? false);
        setRobinhoodEnabled(s.robinhood_enabled ?? false);
        setArcusEnabled(s.arcus_enabled ?? false);
        setPopdexEnabled(s.popdex_enabled ?? false);
        setKrakenEnabled(s.kraken_enabled ?? false);
        setKrakenSpotEnabled(s.kraken_spot_enabled ?? false);
        setKrakenFuturesEnabled(s.kraken_futures_enabled ?? false);
        setPolymarketEnabled(s.polymarket_enabled ?? false);
        setKalshiEnabled(s.kalshi_enabled ?? false);

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

  useEffect(() => {
    if (systemStatus?.active_venues && systemStatus.active_venues.length > 0) {
      setLiveVenues(systemStatus.active_venues);
    }
  }, [systemStatus]);

  const defaultVenues: VenueStatusDto[] = [
    { id: "hyperliquid", name: "Hyperliquid (L1 DEX)", market_type: "Perpetual CLOB", status: "OFFLINE", latency_ms: 0, fee_bps: 2.0 },
    { id: "lighter", name: "Lighter (zkDEX)", market_type: "zk-Rollup CLOB", status: "OFFLINE", latency_ms: 0, fee_bps: 1.5 },
    { id: "vanta", name: "Vanta.exchange", market_type: "Omnichain Perps", status: "OFFLINE", latency_ms: 0, fee_bps: 3.0 },
    { id: "arcus", name: "Arcus DEX", market_type: "Fast CLOB", status: "OFFLINE", latency_ms: 0, fee_bps: 1.5 },
    { id: "popdex", name: "PopDEX", market_type: "Lighter Hybrid", status: "OFFLINE", latency_ms: 0, fee_bps: 2.0 },
    { id: "robinhood", name: "Robinhood EVM", market_type: "EVM DEX Aggregator", status: "OFFLINE", latency_ms: 0, fee_bps: 2.5 },
    { id: "kraken", name: "Kraken", market_type: "Spot & Futures CLOB", status: "OFFLINE", latency_ms: 0, fee_bps: 16.0 },
    { id: "polymarket", name: "Polymarket (CTF CLOB)", market_type: "Prediction Market", status: "OFFLINE", latency_ms: 0, fee_bps: 0.0 },
    { id: "kalshi", name: "Kalshi (CFTC)", market_type: "Prediction Market", status: "OFFLINE", latency_ms: 0, fee_bps: 10.0 },
  ];
  const displayVenues = liveVenues.length > 0 ? liveVenues : defaultVenues;

  const getVenueInfo = (id: string) => {
    return displayVenues.find(
      (v) => v.id.toLowerCase() === id.toLowerCase() || v.name.toLowerCase().includes(id.toLowerCase())
    );
  };

  const handleSave = async (e: React.FormEvent) => {
    e.preventDefault();
    setErrorMsg(null);
    setSaving(true);
    try {
      const payload: EngineSettingsDto = {
        hsm_url: hsmUrl,
        hmac_token: hmacToken,
        max_slippage_bps: maxSlippageBps,
        min_depth_ratio: minDepthRatio,
        engine_rpc_port: engineRpcPort,
        engine_ws_port: engineWsPort,
        bind_address: bindAddress,
        trading_mode: tradingMode,
        fast_warmup_enabled: fastWarmupEnabled,
        venue_rebalance_threshold_usd: venueRebalanceThresholdUsd,
        configured_venues: configuredVenues,
        hyperliquid_enabled: hlEnabled,
        hyperliquid_spot_enabled: hlSpotEnabled,
        lighter_enabled: lighterEnabled,
        lighter_spot_enabled: lighterSpotEnabled,
        vanta_enabled: vantaEnabled,
        robinhood_enabled: robinhoodEnabled,
        arcus_enabled: arcusEnabled,
        popdex_enabled: popdexEnabled,
        kraken_enabled: krakenEnabled,
        kraken_spot_enabled: krakenSpotEnabled,
        kraken_futures_enabled: krakenFuturesEnabled,
        polymarket_enabled: polymarketEnabled,
        kalshi_enabled: kalshiEnabled,
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
    setInlinePasscode("");
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

  const vaultCredentialIds = (id: ExchangeId): string[] => {
    if (id === "kraken") {
      return ["kraken_spot", "kraken_futures"].filter((v) =>
        configuredVenues.includes(v)
      );
    }
    return configuredVenues.includes(id) ? [id] : [];
  };

  const handleRemoveCredentials = async () => {
    const ids = vaultCredentialIds(selectedExchange);
    if (ids.length === 0) return;
    const label =
      EXCHANGES_ALPHABETICAL.find((ex) => ex.id === selectedExchange)?.name ||
      selectedExchange;
    if (
      !window.confirm(
        `Remove encrypted credentials for ${label}? This cannot be undone.`
      )
    ) {
      return;
    }
    setErrorMsg(null);
    try {
      for (const id of ids) {
        await api.deleteExchangeCredentials(id);
      }
      const sanitized = await api.getEngineSettings();
      setConfiguredVenues(sanitized.configured_venues || []);
      clearCredentialInputs();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
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
        return hlEnabled && (configured("hyperliquid") || Boolean(hyperliquidWallet || hyperliquidPrivateKey));
      case "lighter":
        return lighterEnabled && (configured("lighter") || Boolean(lighterApiKey));
      case "vanta":
        return vantaEnabled && (configured("vanta") || Boolean(vantaOrderlyKey || vantaAccountId));
      case "arcus":
        return arcusEnabled && (configured("arcus") || Boolean(arcusApiKey));
      case "popdex":
        return popdexEnabled && (configured("popdex") || Boolean(popdexApiKey || (popdexAccountIndex && popdexAccountIndex !== "0")));
      case "robinhood":
        return robinhoodEnabled && (configured("robinhood") || Boolean(robinhoodWallet || robinhoodPrivateKey));
      case "kraken":
        return krakenEnabled && (
          configured("kraken_spot") || configured("kraken_futures") ||
          (krakenSpotEnabled && Boolean(krakenSpotApiKey)) ||
          (krakenFuturesEnabled && Boolean(krakenFuturesApiKey))
        );
      case "polymarket":
        return polymarketEnabled && (configured("polymarket") || Boolean(polymarketApiKey || polymarketSecret));
      case "kalshi":
        return kalshiEnabled && (configured("kalshi") || Boolean(kalshiApiKeyId || kalshiPrivateKey));
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

  const activeVenuesCount = [
    hlEnabled,
    lighterEnabled,
    vantaEnabled,
    arcusEnabled,
    popdexEnabled,
    robinhoodEnabled,
    krakenEnabled,
    polymarketEnabled,
    kalshiEnabled,
  ].filter(Boolean).length;

  const isVaultLocked = !api.isTauri || !vaultStatus?.is_unlocked;

  return (
    <div className="flex flex-col h-full min-h-0 overflow-hidden select-none bg-surface">
      {/* Top Section Navigation Bar */}
      <div className="border-b border-surface-border bg-slate-50/80 shrink-0 flex items-stretch justify-between select-none">
        {/* Navigation Tabs with dividers: Execution, Exchanges, Network & VPS, About */}
        <div className="flex items-stretch divide-x divide-surface-border border-r border-surface-border">
          <button
            type="button"
            onClick={() => setActiveTab("execution")}
            className={`relative h-11 flex items-center space-x-2 px-4 sm:px-5 text-xs font-mono transition select-none group cursor-pointer ${
              activeTab === "execution"
                ? "text-accent font-bold bg-surface"
                : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
            }`}
          >
            <Sliders
              className={`w-3.5 h-3.5 transition shrink-0 ${
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
            className={`relative h-11 flex items-center space-x-2 px-4 sm:px-5 text-xs font-mono transition select-none group cursor-pointer ${
              activeTab === "exchanges"
                ? "text-accent font-bold bg-surface"
                : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
            }`}
          >
            <Key
              className={`w-3.5 h-3.5 transition shrink-0 ${
                activeTab === "exchanges" ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
              }`}
            />
            <span>Exchanges</span>
            <span
              className={`px-1.5 py-0.2 rounded text-[10px] font-bold font-mono min-w-[16px] text-center leading-tight transition ${
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
            onClick={() => setActiveTab("network")}
            className={`relative h-11 flex items-center space-x-2 px-4 sm:px-5 text-xs font-mono transition select-none group cursor-pointer ${
              activeTab === "network"
                ? "text-accent font-bold bg-surface"
                : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
            }`}
          >
            <Globe
              className={`w-3.5 h-3.5 transition shrink-0 ${
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
            className={`relative h-11 flex items-center space-x-2 px-4 sm:px-5 text-xs font-mono transition select-none group cursor-pointer ${
              activeTab === "about"
                ? "text-accent font-bold bg-surface"
                : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
            }`}
          >
            <Info
              className={`w-3.5 h-3.5 transition shrink-0 ${
                activeTab === "about" ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
              }`}
            />
            <span>About</span>
            {activeTab === "about" && (
              <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
            )}
          </button>
        </div>

        {/* Right Header: Lock Vault / Remove Credentials actions when unlocked on Exchanges Tab */}
        <div className="flex items-center space-x-2 py-2 pr-5 sm:pr-6">
          {activeTab === "exchanges" && !isVaultLocked && (
            <>
              {vaultCredentialIds(selectedExchange).length > 0 && (
                <button
                  type="button"
                  onClick={handleRemoveCredentials}
                  className="px-2.5 py-1 rounded-lg border border-red-200 bg-surface hover:bg-red-50 text-red-600 text-[11px] font-mono flex items-center space-x-1.5 transition shadow-2xs cursor-pointer"
                  title="Permanently remove the selected venue's stored credentials from the vault"
                >
                  <Trash2 className="w-3.5 h-3.5 text-red-500" />
                  <span>Remove Credentials</span>
                </button>
              )}
              <button
                type="button"
                onClick={handleLockVault}
                className="px-2.5 py-1 rounded-lg border border-slate-200 bg-surface hover:bg-slate-100 text-slate-600 text-[11px] font-mono flex items-center space-x-1.5 transition shadow-2xs cursor-pointer"
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
                className="px-2.5 py-1 rounded-lg border border-red-200 bg-surface hover:bg-red-50 text-red-600 text-[11px] font-mono flex items-center space-x-1.5 transition shadow-2xs cursor-pointer"
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
              <h3 className="text-xs font-bold font-mono text-slate-900 flex items-center space-x-1.5">
                <span>
                  {activeTab === "execution" && "Execution Modes & Safety Guardrails"}
                  {activeTab === "exchanges" && "Exchange API Credentials & Account Margins"}
                  {activeTab === "network" && "Network Ports & HSM Signer Configuration"}
                  {activeTab === "about" && "About Arbitrager & Release Information"}
                </span>
                <InfoTooltip
                  text={
                    activeTab === "execution"
                      ? "Calibrate slippage tolerances, minimum depth ratio, and paper/live trading modes."
                      : activeTab === "exchanges"
                      ? "Configure exchange API credentials, inspect real-time equity & available margin, and manage secure vault."
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
                  <span className="text-[11px] font-mono font-bold uppercase text-slate-700 tracking-wider">
                    Exchanges ({EXCHANGES_ALPHABETICAL.length})
                  </span>
                  <span className="text-[10px] font-mono px-1.5 py-0.5 rounded bg-blue-50 text-blue-700 border border-blue-200 font-semibold">
                    Alphabetical (A-Z)
                  </span>
                </div>

                <div className="flex-1 overflow-y-auto p-2 space-y-1">
                  {EXCHANGES_ALPHABETICAL.map((ex) => {
                    const isSelected = selectedExchange === ex.id;
                    const isEnabled = isExchangeEnabled(ex.id);
                    const bal = getVenueBalance(ex.id);
                    return (
                      <button
                        key={ex.id}
                        type="button"
                        onClick={() => setSelectedExchange(ex.id)}
                        className={`w-full text-left p-2.5 rounded-xl text-xs font-mono transition flex items-center justify-between group cursor-pointer border ${
                          isSelected
                            ? "bg-accent text-white border-accent shadow-xs"
                            : "bg-surface hover:bg-slate-100/80 text-slate-700 border-surface-border/60 hover:border-surface-border"
                        }`}
                      >
                        <div className="min-w-0 pr-2">
                          <div className="flex items-center space-x-2">
                            <span
                              className={`w-2 h-2 rounded-full shrink-0 ${
                                isEnabled
                                  ? isSelected
                                    ? "bg-white ring-2 ring-white/30"
                                    : "bg-emerald-500"
                                  : isSelected
                                  ? "bg-white/40"
                                  : "bg-slate-300"
                              }`}
                            />
                            <span className={`font-semibold truncate ${isSelected ? "text-white font-bold" : "text-slate-900"}`}>
                              {ex.name}
                            </span>
                          </div>
                        </div>
                        <div className="text-right shrink-0">
                          <span
                            className={`text-[9px] font-mono uppercase block font-semibold ${
                              isSelected ? "text-blue-100" : isEnabled ? "text-emerald-700" : "text-slate-400"
                            }`}
                          >
                            {isEnabled ? "ACTIVE" : "OFF"}
                          </span>
                          <span className={`text-[11px] font-mono block font-bold ${isSelected ? "text-white" : "text-slate-800"}`}>
                            ${(bal.equity_usd ?? bal.balance_usd).toLocaleString("en-US", { maximumFractionDigits: 0 })}
                          </span>
                        </div>
                      </button>
                    );
                  })}
                </div>

                {/* Left Panel Summary */}
                <div className="p-3 border-t border-surface-border bg-slate-100/60 text-[10px] font-mono text-slate-500 flex items-center justify-between shrink-0">
                  <span>Active: <strong className="text-slate-800 font-bold">{activeVenuesCount}</strong>/9</span>
                  <span>Configured: <strong className="text-slate-800 font-bold">{configuredCount}</strong>/9</span>
                </div>
              </div>

              {/* Right Content Pane: Single Selected Exchange */}
              <div className="flex-1 overflow-y-auto p-6 space-y-5 min-w-0">
                {errorMsg && (
                  <div className="p-4 bg-red-50 border border-red-200 text-red-700 rounded-xl text-xs font-mono flex items-center space-x-2">
                    <AlertCircle className="w-4 h-4 shrink-0" />
                    <span>{errorMsg}</span>
                  </div>
                )}

                {saved && (
                  <div className="p-4 bg-emerald-50 border border-emerald-200 text-emerald-700 rounded-xl text-xs font-mono flex items-center space-x-2 animate-in fade-in duration-200">
                    <CheckCircle2 className="w-4 h-4 shrink-0" />
                    <span>Configuration saved successfully and updated in live atomic execution coordinator.</span>
                  </div>
                )}

                {/* Financial & Margin Metrics KPI Card for Selected Exchange */}
                {(() => {
                  const currentExMeta =
                    EXCHANGES_ALPHABETICAL.find((ex) => ex.id === selectedExchange) ||
                    EXCHANGES_ALPHABETICAL[0];
                  const currentBal = getVenueBalance(selectedExchange);
                  return (
                    <div className="bg-surface border border-surface-border rounded-2xl p-4.5 shadow-xs space-y-3.5">
                      <div className="flex items-center justify-between border-b border-surface-border pb-3">
                        <div className="flex items-center space-x-2">
                          <Wallet className="w-4 h-4 text-accent" />
                          <h4 className="text-xs font-mono font-bold uppercase text-slate-900 tracking-wide">
                            {currentExMeta.name} — Margin &amp; Collateral Account
                          </h4>
                          <span
                            className={`px-2 py-0.5 rounded text-[10px] font-mono font-bold ${
                              currentBal.status === "HEALTHY"
                                ? "bg-emerald-50 text-emerald-800 border border-emerald-300"
                                : currentBal.status === "LOW_MARGIN"
                                ? "bg-amber-50 text-amber-800 border border-amber-300"
                                : "bg-red-50 text-red-800 border border-red-300"
                            }`}
                          >
                            {currentBal.status}
                          </span>
                        </div>

                        <div className="flex items-center space-x-3">
                          <span className="text-[11px] font-mono text-slate-400">
                            Safety Floor: ${currentBal.min_required_usd.toLocaleString()}
                          </span>
                          <button
                            type="button"
                            onClick={loadVenueBalances}
                            disabled={loadingBalances}
                            className="px-2.5 py-1 bg-surface-raised border border-surface-border hover:bg-slate-200 rounded-lg text-xs font-mono flex items-center space-x-1.5 text-slate-700 transition cursor-pointer"
                            title="Sync collateral balances"
                          >
                            <RefreshCw className={`w-3 h-3 ${loadingBalances ? "animate-spin text-accent" : "text-slate-500"}`} />
                            <span className="text-[11px]">Sync</span>
                          </button>
                        </div>
                      </div>

                      <div className="grid grid-cols-1 sm:grid-cols-3 gap-3 font-mono">
                        {/* Metric 1: Equity */}
                        <div className="p-3.5 bg-slate-50/70 border border-surface-border rounded-xl space-y-1">
                          <div className="flex items-center justify-between text-slate-400">
                            <span className="text-[10px] uppercase font-bold tracking-wider">Equity</span>
                            <span className="text-[10px]">USD</span>
                          </div>
                          <div className="text-xl font-bold text-slate-900 tracking-tight">
                            ${(currentBal.equity_usd ?? currentBal.balance_usd).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}
                          </div>
                          <div className="text-[10px] text-slate-500">
                            Total account net value
                          </div>
                        </div>

                        {/* Metric 2: Available Margin */}
                        <div className="p-3.5 bg-slate-50/70 border border-surface-border rounded-xl space-y-1">
                          <div className="flex items-center justify-between text-slate-400">
                            <span className="text-[10px] uppercase font-bold tracking-wider">Available Margin</span>
                            <span className="text-[10px]">USD</span>
                          </div>
                          <div className="text-xl font-bold text-emerald-700 tracking-tight">
                            ${(currentBal.available_margin_usd ?? currentBal.balance_usd).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}
                          </div>
                          <div className="text-[10px] text-slate-500">
                            Free collateral for new orders
                          </div>
                        </div>

                        {/* Metric 3: Margin Rate */}
                        <div className="p-3.5 bg-slate-50/70 border border-surface-border rounded-xl space-y-1">
                          <div className="flex items-center justify-between text-slate-400">
                            <span className="text-[10px] uppercase font-bold tracking-wider">Margin Rate</span>
                            <span className="text-[10px]">Utilization</span>
                          </div>
                          <div className="text-xl font-bold text-slate-900 flex items-baseline space-x-1.5 tracking-tight">
                            <span>{(currentBal.margin_rate_pct ?? 0.0).toFixed(1)}%</span>
                            <span className="text-[11px] font-normal text-slate-400">utilized</span>
                          </div>
                          <div className="w-full bg-slate-200 h-1.5 rounded-full overflow-hidden mt-1.5">
                            <div
                              className={`h-full rounded-full transition-all duration-300 ${
                                (currentBal.margin_rate_pct ?? 0) > 80
                                  ? "bg-red-500"
                                  : (currentBal.margin_rate_pct ?? 0) > 50
                                  ? "bg-amber-500"
                                  : "bg-accent"
                              }`}
                              style={{ width: `${Math.min(100, Math.max(3, currentBal.margin_rate_pct ?? 0))}%` }}
                            />
                          </div>
                        </div>
                      </div>

                      {currentBal.recommended_action && (
                        <div className="p-2.5 bg-amber-50 border border-amber-300 rounded-xl text-xs font-mono text-amber-900 flex items-start space-x-2">
                          <AlertTriangle className="w-3.5 h-3.5 text-amber-700 shrink-0 mt-0.5" />
                          <span>{currentBal.recommended_action}</span>
                        </div>
                      )}
                    </div>
                  );
                })()}


            {/* Exchange: Arcus DEX */}
            {selectedExchange === "arcus" && (
              <div className="bg-surface border border-surface-border rounded-2xl p-5 space-y-4 shadow-xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-3">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-xs font-mono font-bold uppercase text-slate-900">
                      Arcus (CLOB DEX)
                    </h3>
                    <span className="px-2 py-0.5 rounded text-[10px] font-mono bg-blue-50 text-blue-700 border border-blue-200">
                      High-Throughput CLOB
                    </span>
                  </div>
                  <div className="flex items-center space-x-3">
                    <span className="text-[11px] font-mono text-slate-400">
                      Fee: {getVenueInfo("arcus")?.fee_bps ?? 1.5} BPS
                    </span>
                    <ToggleSwitch
                      checked={arcusEnabled}
                      onChange={setArcusEnabled}
                      label={arcusEnabled ? "Active" : "Disabled"}
                    />
                  </div>
                </div>

                {!arcusEnabled ? (
                  <div className="p-3.5 bg-surface-raised border border-surface-border rounded-xl text-center text-xs font-mono text-slate-500">
                    Arcus exchange is disabled. Toggle switch to enable and configure API credentials.
                  </div>
                ) : (
                  <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs font-mono">
                    <div>
                      <label className="text-slate-600 block mb-1">API Key:</label>
                      <input
                        type="text"
                        disabled={loading || isVaultLocked}
                        value={arcusApiKey}
                        placeholder={isVaultLocked ? "••••••••••••••••" : "arc_..."}
                        onChange={(e) => setArcusApiKey(e.target.value)}
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                      />
                    </div>

                    <div>
                      <div className="flex items-center justify-between mb-1">
                        <label className="text-slate-600">API Secret:</label>
                        <button
                          type="button"
                          onClick={() => toggleSecret("arcus_secret")}
                          className="text-slate-400 hover:text-slate-600 text-[11px] flex items-center space-x-1"
                        >
                          {showSecrets["arcus_secret"] ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3 text-blue-600" />}
                          <span>{showSecrets["arcus_secret"] ? "Hide" : "Show"}</span>
                        </button>
                      </div>
                      <input
                        type={showSecrets["arcus_secret"] ? "text" : "password"}
                        disabled={loading || isVaultLocked}
                        value={arcusSecret}
                        placeholder={isVaultLocked ? "••••••••••••••••" : "secret"}
                        onChange={(e) => setArcusSecret(e.target.value)}
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                      />
                    </div>

                    <div className="md:col-span-2">
                      <label className="text-slate-600 block mb-1">REST Endpoint:</label>
                      <input
                        type="text"
                        disabled={loading}
                        value={arcusUrl}
                        onChange={(e) => setArcusUrl(e.target.value)}
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                      />
                    </div>
                  </div>
                )}
              </div>
            )}



            {/* Exchange 1: Hyperliquid */}
            {selectedExchange === "hyperliquid" && (
              <div className="bg-surface border border-surface-border rounded-2xl p-5 space-y-4 shadow-xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-3">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-xs font-mono font-bold uppercase text-slate-900">
                      Hyperliquid (L1 Perp DEX)
                    </h3>
                    <span className="px-2 py-0.5 rounded text-[10px] font-mono bg-blue-50 text-blue-700 border border-blue-200">
                      Perp & Spot
                    </span>
                  </div>
                  <div className="flex items-center space-x-3">
                    <span className="text-[11px] font-mono text-slate-400">
                      Fee: {getVenueInfo("hyperliquid")?.fee_bps ?? 2.0} BPS
                    </span>
                    <ToggleSwitch
                      checked={hlEnabled}
                      onChange={setHlEnabled}
                      label={hlEnabled ? "Active" : "Disabled"}
                    />
                  </div>
                </div>

                {!hlEnabled ? (
                  <div className="p-3.5 bg-surface-raised border border-surface-border rounded-xl text-center text-xs font-mono text-slate-500">
                    Hyperliquid exchange is disabled. Toggle switch to enable and configure credentials.
                  </div>
                ) : (
                  <div className="space-y-3">
                    {/* Hyperliquid Multi-Market Option: Spot */}
                    <div className="p-3 bg-surface-raised border border-surface-border rounded-xl flex items-center justify-between">
                      <div>
                        <span className="text-xs font-bold font-mono text-slate-900 block">Hyperliquid Spot Market (HyperEVM / L1 Spot)</span>
                        <span className="text-[10px] text-slate-500 font-mono">Enables atomic spot leg routing alongside perp basis trades</span>
                      </div>
                      <ToggleSwitch
                        checked={hlSpotEnabled}
                        onChange={setHlSpotEnabled}
                        size="sm"
                        label={hlSpotEnabled ? "Spot Enabled" : "Spot Off"}
                      />
                    </div>

                    <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs font-mono">
                      <div>
                        <label className="text-slate-600 block mb-1">Trading EOA Wallet Address:</label>
                        <input
                          type="text"
                          disabled={loading || isVaultLocked}
                          value={hyperliquidWallet}
                          placeholder="0x..."
                          onChange={(e) => setHyperliquidWallet(e.target.value)}
                          className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                        />
                      </div>

                      <div>
                        <div className="flex items-center justify-between mb-1">
                          <label className="text-slate-600">Trading Agent Private Key:</label>
                          <button
                            type="button"
                            onClick={() => toggleSecret("hl_key")}
                            className="text-slate-400 hover:text-slate-600 text-[11px] flex items-center space-x-1"
                          >
                            {showSecrets["hl_key"] ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3 text-blue-600" />}
                            <span>{showSecrets["hl_key"] ? "Hide" : "Show"}</span>
                          </button>
                        </div>
                        <input
                          type={showSecrets["hl_key"] ? "text" : "password"}
                          disabled={loading || isVaultLocked}
                          value={hyperliquidPrivateKey}
                          placeholder={isVaultLocked ? "••••••••••••••••" : "0x..."}
                          onChange={(e) => setHyperliquidPrivateKey(e.target.value)}
                          className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                        />
                      </div>

                      <div>
                        <label className="text-slate-600 block mb-1">Account Index (Optional):</label>
                        <input
                          type="number"
                          disabled={loading || isVaultLocked}
                          value={hyperliquidAccountIndex}
                          placeholder="Default: 0"
                          onChange={(e) => setHyperliquidAccountIndex(e.target.value)}
                          className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                        />
                      </div>

                      <div>
                        <label className="text-slate-600 block mb-1">Subaccount Index (Optional):</label>
                        <input
                          type="number"
                          disabled={loading || isVaultLocked}
                          value={hyperliquidSubaccountIndex}
                          placeholder="Default: None"
                          onChange={(e) => setHyperliquidSubaccountIndex(e.target.value)}
                          className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                        />
                      </div>
                    </div>
                  </div>
                )}
              </div>
            )}



            {/* Exchange 7: Kalshi */}
            {selectedExchange === "kalshi" && (
              <div className="bg-surface border border-surface-border rounded-2xl p-5 space-y-4 shadow-xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-3">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-xs font-mono font-bold uppercase text-slate-900">
                      Kalshi (CFTC Binary)
                    </h3>
                    <span className="px-2 py-0.5 rounded text-[10px] font-mono bg-blue-50 text-blue-700 border border-blue-200">
                      Regulated Prediction
                    </span>
                  </div>
                  <div className="flex items-center space-x-3">
                    <span className="text-[11px] font-mono text-slate-400">
                      Fee: {getVenueInfo("kalshi")?.fee_bps ?? 10.0} BPS
                    </span>
                    <ToggleSwitch
                      checked={kalshiEnabled}
                      onChange={setKalshiEnabled}
                      label={kalshiEnabled ? "Active" : "Disabled"}
                    />
                  </div>
                </div>

                {!kalshiEnabled ? (
                  <div className="p-3.5 bg-surface-raised border border-surface-border rounded-xl text-center text-xs font-mono text-slate-500">
                    Kalshi is disabled. Toggle switch to enable and configure CFTC API credentials.
                  </div>
                ) : (
                  <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs font-mono">
                    <div>
                      <label className="text-slate-600 block mb-1">Kalshi API Key ID:</label>
                      <input
                        type="text"
                        disabled={loading || isVaultLocked}
                        value={kalshiApiKeyId}
                        placeholder="e.g. 550e8400-e29b-..."
                        onChange={(e) => setKalshiApiKeyId(e.target.value)}
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                      />
                    </div>

                    <div>
                      <div className="flex items-center justify-between mb-1">
                        <label className="text-slate-600">RSA Private Key:</label>
                        <button
                          type="button"
                          onClick={() => toggleSecret("kalshi_key")}
                          className="text-slate-400 hover:text-slate-600 text-[11px] flex items-center space-x-1"
                        >
                          {showSecrets["kalshi_key"] ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3 text-blue-600" />}
                          <span>{showSecrets["kalshi_key"] ? "Hide" : "Show"}</span>
                        </button>
                      </div>
                      <input
                        type={showSecrets["kalshi_key"] ? "text" : "password"}
                        disabled={loading || isVaultLocked}
                        value={kalshiPrivateKey}
                        placeholder={isVaultLocked ? "••••••••••••••••" : "-----BEGIN RSA PRIVATE KEY-----"}
                        onChange={(e) => setKalshiPrivateKey(e.target.value)}
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                      />
                    </div>

                    <div className="md:col-span-2">
                      <label className="text-slate-600 block mb-1">Endpoint URL:</label>
                      <input
                        type="text"
                        disabled={loading}
                        value={kalshiUrl}
                        onChange={(e) => setKalshiUrl(e.target.value)}
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                      />
                    </div>
                  </div>
                )}
              </div>
            )}


            {/* Exchange 5: Kraken (Spot & Futures Multi-Market) */}
            {selectedExchange === "kraken" && (
              <div className="bg-surface border border-surface-border rounded-2xl p-5 space-y-4 shadow-xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-3">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-xs font-mono font-bold uppercase text-slate-900">
                      Kraken (Spot & Futures CLOB)
                    </h3>
                    <span className="px-2 py-0.5 rounded text-[10px] font-mono bg-purple-50 text-purple-700 border border-purple-200">
                      Multi-Market (Spot & Futures)
                    </span>
                  </div>
                  <div className="flex items-center space-x-3">
                    <span className="text-[11px] font-mono text-slate-400">
                      Fee: {getVenueInfo("kraken")?.fee_bps ?? 16.0} BPS
                    </span>
                    <ToggleSwitch
                      checked={krakenEnabled}
                      onChange={setKrakenEnabled}
                      label={krakenEnabled ? "Active" : "Disabled"}
                    />
                  </div>
                </div>

                {!krakenEnabled ? (
                  <div className="p-3.5 bg-surface-raised border border-surface-border rounded-xl text-center text-xs font-mono text-slate-500">
                    Kraken exchange is disabled. Toggle switch to enable Spot & Futures API configuration.
                  </div>
                ) : (
                  <div className="space-y-4">
                    <div className="p-2.5 bg-blue-50/70 border border-blue-200 rounded-xl text-[11px] font-mono text-slate-600">
                      💡 Kraken maintains separate API infrastructure for Spot and Futures markets. Toggle each option independently to enter its respective credentials.
                    </div>

                    {/* Option A: Kraken Spot Market */}
                    <div className="p-4 bg-surface-raised border border-surface-border rounded-xl space-y-3">
                      <div className="flex items-center justify-between border-b border-surface-border/60 pb-2">
                        <div className="flex items-center space-x-2">
                          <span className="text-xs font-bold font-mono text-slate-900">1. Kraken Spot Trading API</span>
                          <span className="text-[10px] font-mono px-1.5 py-0.2 rounded bg-slate-200 text-slate-700">api.kraken.com</span>
                        </div>
                        <ToggleSwitch
                          checked={krakenSpotEnabled}
                          onChange={setKrakenSpotEnabled}
                          size="sm"
                          label={krakenSpotEnabled ? "Spot Enabled" : "Spot Disabled"}
                        />
                      </div>

                      {!krakenSpotEnabled ? (
                        <div className="text-[11px] text-slate-500 font-mono py-1">
                          Spot trading disabled for Kraken. Toggle on to configure Spot API Key & Secret.
                        </div>
                      ) : (
                        <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs font-mono pt-1">
                          <div>
                            <label className="text-slate-600 block mb-1">Kraken Spot API Key:</label>
                            <input
                              type="text"
                              disabled={loading || isVaultLocked}
                              value={krakenSpotApiKey}
                              placeholder="api_key_..."
                              onChange={(e) => setKrakenSpotApiKey(e.target.value)}
                              className="w-full px-3 py-2 bg-surface border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                            />
                          </div>

                          <div>
                            <div className="flex items-center justify-between mb-1">
                              <label className="text-slate-600">Kraken Spot Private/Secret Key:</label>
                              <button
                                type="button"
                                onClick={() => toggleSecret("kraken_spot_sec")}
                                className="text-slate-400 hover:text-slate-600 text-[11px] flex items-center space-x-1"
                              >
                                {showSecrets["kraken_spot_sec"] ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3 text-blue-600" />}
                                <span>{showSecrets["kraken_spot_sec"] ? "Hide" : "Show"}</span>
                              </button>
                            </div>
                            <input
                              type={showSecrets["kraken_spot_sec"] ? "text" : "password"}
                              disabled={loading || isVaultLocked}
                              value={krakenSpotSecret}
                              placeholder={isVaultLocked ? "••••••••••••••••" : "base64_secret..."}
                              onChange={(e) => setKrakenSpotSecret(e.target.value)}
                              className="w-full px-3 py-2 bg-surface border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                            />
                          </div>

                          <div className="md:col-span-2">
                            <label className="text-slate-600 block mb-1">Spot API Endpoint URL:</label>
                            <input
                              type="text"
                              disabled={loading}
                              value={krakenSpotUrl}
                              placeholder="https://api.kraken.com"
                              onChange={(e) => setKrakenSpotUrl(e.target.value)}
                              className="w-full px-3 py-2 bg-surface border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                            />
                          </div>
                        </div>
                      )}
                    </div>

                    {/* Option B: Kraken Futures Market */}
                    <div className="p-4 bg-surface-raised border border-surface-border rounded-xl space-y-3">
                      <div className="flex items-center justify-between border-b border-surface-border/60 pb-2">
                        <div className="flex items-center space-x-2">
                          <span className="text-xs font-bold font-mono text-slate-900">2. Kraken Futures Trading API</span>
                          <span className="text-[10px] font-mono px-1.5 py-0.2 rounded bg-indigo-50 text-indigo-700 border border-indigo-200">futures.kraken.com</span>
                        </div>
                        <ToggleSwitch
                          checked={krakenFuturesEnabled}
                          onChange={setKrakenFuturesEnabled}
                          size="sm"
                          label={krakenFuturesEnabled ? "Futures Enabled" : "Futures Disabled"}
                        />
                      </div>

                      {!krakenFuturesEnabled ? (
                        <div className="text-[11px] text-slate-500 font-mono py-1">
                          Futures trading disabled for Kraken. Toggle on to configure Futures API Key & Secret.
                        </div>
                      ) : (
                        <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs font-mono pt-1">
                          <div>
                            <label className="text-slate-600 block mb-1">Kraken Futures API Key:</label>
                            <input
                              type="text"
                              disabled={loading || isVaultLocked}
                              value={krakenFuturesApiKey}
                              placeholder="futures_key_..."
                              onChange={(e) => setKrakenFuturesApiKey(e.target.value)}
                              className="w-full px-3 py-2 bg-surface border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                            />
                          </div>

                          <div>
                            <div className="flex items-center justify-between mb-1">
                              <label className="text-slate-600">Kraken Futures Secret Key:</label>
                              <button
                                type="button"
                                onClick={() => toggleSecret("kraken_fut_sec")}
                                className="text-slate-400 hover:text-slate-600 text-[11px] flex items-center space-x-1"
                              >
                                {showSecrets["kraken_fut_sec"] ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3 text-blue-600" />}
                                <span>{showSecrets["kraken_fut_sec"] ? "Hide" : "Show"}</span>
                              </button>
                            </div>
                            <input
                              type={showSecrets["kraken_fut_sec"] ? "text" : "password"}
                              disabled={loading || isVaultLocked}
                              value={krakenFuturesSecret}
                              placeholder={isVaultLocked ? "••••••••••••••••" : "futures_secret..."}
                              onChange={(e) => setKrakenFuturesSecret(e.target.value)}
                              className="w-full px-3 py-2 bg-surface border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                            />
                          </div>

                          <div className="md:col-span-2">
                            <label className="text-slate-600 block mb-1">Futures API Endpoint URL:</label>
                            <input
                              type="text"
                              disabled={loading}
                              value={krakenFuturesUrl}
                              placeholder="https://futures.kraken.com"
                              onChange={(e) => setKrakenFuturesUrl(e.target.value)}
                              className="w-full px-3 py-2 bg-surface border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                            />
                          </div>
                        </div>
                      )}
                    </div>
                  </div>
                )}
              </div>
            )}



            {/* Exchange 2: Lighter */}
            {selectedExchange === "lighter" && (
              <div className="bg-surface border border-surface-border rounded-2xl p-5 space-y-4 shadow-xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-3">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-xs font-mono font-bold uppercase text-slate-900">
                      Lighter (zkDEX Rollup)
                    </h3>
                    <span className="px-2 py-0.5 rounded text-[10px] font-mono bg-blue-50 text-blue-700 border border-blue-200">
                      Perp & Spot CLOB
                    </span>
                  </div>
                  <div className="flex items-center space-x-3">
                    <span className="text-[11px] font-mono text-slate-400">
                      Fee: {getVenueInfo("lighter")?.fee_bps ?? 1.5} BPS
                    </span>
                    <ToggleSwitch
                      checked={lighterEnabled}
                      onChange={setLighterEnabled}
                      label={lighterEnabled ? "Active" : "Disabled"}
                    />
                  </div>
                </div>

                {!lighterEnabled ? (
                  <div className="p-3.5 bg-surface-raised border border-surface-border rounded-xl text-center text-xs font-mono text-slate-500">
                    Lighter exchange is disabled. Toggle switch to enable and configure credentials.
                  </div>
                ) : (
                  <div className="space-y-3">
                    {/* Lighter Multi-Market Option: Spot */}
                    <div className="p-3 bg-surface-raised border border-surface-border rounded-xl flex items-center justify-between">
                      <div>
                        <span className="text-xs font-bold font-mono text-slate-900 block">Lighter Spot Orderbook</span>
                        <span className="text-[10px] text-slate-500 font-mono">Enables atomic spot book market orders</span>
                      </div>
                      <ToggleSwitch
                        checked={lighterSpotEnabled}
                        onChange={setLighterSpotEnabled}
                        size="sm"
                        label={lighterSpotEnabled ? "Spot Enabled" : "Spot Off"}
                      />
                    </div>

                    <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs font-mono">
                      <div>
                        <label className="text-slate-600 block mb-1">Account Index:</label>
                        <input
                          type="text"
                          disabled={loading || isVaultLocked}
                          value={lighterAccountIndex}
                          onChange={(e) => setLighterAccountIndex(e.target.value)}
                          className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                        />
                      </div>

                      <div>
                        <div className="flex items-center justify-between mb-1">
                          <label className="text-slate-600">API Private Key / Signer Token:</label>
                          <button
                            type="button"
                            onClick={() => toggleSecret("lgt_key")}
                            className="text-slate-400 hover:text-slate-600 text-[11px] flex items-center space-x-1"
                          >
                            {showSecrets["lgt_key"] ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3 text-blue-600" />}
                            <span>{showSecrets["lgt_key"] ? "Hide" : "Show"}</span>
                          </button>
                        </div>
                        <input
                          type={showSecrets["lgt_key"] ? "text" : "password"}
                          disabled={loading || isVaultLocked}
                          value={lighterApiKey}
                          placeholder={isVaultLocked ? "••••••••••••••••" : "0x..."}
                          onChange={(e) => setLighterApiKey(e.target.value)}
                          className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                        />
                      </div>

                      <div>
                        <label className="text-slate-600 block mb-1">Subaccount Index (Optional):</label>
                        <input
                          type="number"
                          disabled={loading || isVaultLocked}
                          value={lighterSubaccountIndex}
                          placeholder="e.g. 1"
                          onChange={(e) => setLighterSubaccountIndex(e.target.value)}
                          className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                        />
                      </div>

                      <div>
                        <label className="text-slate-600 block mb-1">Endpoint URL:</label>
                        <input
                          type="text"
                          disabled={loading}
                          value={lighterUrl}
                          onChange={(e) => setLighterUrl(e.target.value)}
                          className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                        />
                      </div>
                    </div>
                  </div>
                )}
              </div>
            )}



            {/* Exchange 6: Polymarket */}
            {selectedExchange === "polymarket" && (
              <div className="bg-surface border border-surface-border rounded-2xl p-5 space-y-4 shadow-xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-3">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-xs font-mono font-bold uppercase text-slate-900">
                      Polymarket (CTF CLOB)
                    </h3>
                    <span className="px-2 py-0.5 rounded text-[10px] font-mono bg-blue-50 text-blue-700 border border-blue-200">
                      Prediction Market
                    </span>
                  </div>
                  <div className="flex items-center space-x-3">
                    <span className="text-[11px] font-mono text-slate-400">
                      Fee: {getVenueInfo("polymarket")?.fee_bps ?? 0.0} BPS
                    </span>
                    <ToggleSwitch
                      checked={polymarketEnabled}
                      onChange={setPolymarketEnabled}
                      label={polymarketEnabled ? "Active" : "Disabled"}
                    />
                  </div>
                </div>

                {!polymarketEnabled ? (
                  <div className="p-3.5 bg-surface-raised border border-surface-border rounded-xl text-center text-xs font-mono text-slate-500">
                    Polymarket is disabled. Toggle switch to enable and configure CTF credentials.
                  </div>
                ) : (
                  <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs font-mono">
                    <div>
                      <label className="text-slate-600 block mb-1">Polymarket API Key:</label>
                      <input
                        type="text"
                        disabled={loading || isVaultLocked}
                        value={polymarketApiKey}
                        placeholder="0x..."
                        onChange={(e) => setPolymarketApiKey(e.target.value)}
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                      />
                    </div>

                    <div>
                      <div className="flex items-center justify-between mb-1">
                        <label className="text-slate-600">Polymarket Secret:</label>
                        <button
                          type="button"
                          onClick={() => toggleSecret("pm_secret")}
                          className="text-slate-400 hover:text-slate-600 text-[11px] flex items-center space-x-1"
                        >
                          {showSecrets["pm_secret"] ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3 text-blue-600" />}
                          <span>{showSecrets["pm_secret"] ? "Hide" : "Show"}</span>
                        </button>
                      </div>
                      <input
                        type={showSecrets["pm_secret"] ? "text" : "password"}
                        disabled={loading || isVaultLocked}
                        value={polymarketSecret}
                        placeholder={isVaultLocked ? "••••••••••••••••" : "secret"}
                        onChange={(e) => setPolymarketSecret(e.target.value)}
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                      />
                    </div>

                    <div>
                      <div className="flex items-center justify-between mb-1">
                        <label className="text-slate-600">Polymarket Passphrase:</label>
                        <button
                          type="button"
                          onClick={() => toggleSecret("pm_pass")}
                          className="text-slate-400 hover:text-slate-600 text-[11px] flex items-center space-x-1"
                        >
                          {showSecrets["pm_pass"] ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3 text-blue-600" />}
                          <span>{showSecrets["pm_pass"] ? "Hide" : "Show"}</span>
                        </button>
                      </div>
                      <input
                        type={showSecrets["pm_pass"] ? "text" : "password"}
                        disabled={loading || isVaultLocked}
                        value={polymarketPassphrase}
                        placeholder={isVaultLocked ? "••••••••••••••••" : "passphrase"}
                        onChange={(e) => setPolymarketPassphrase(e.target.value)}
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                      />
                    </div>

                    <div>
                      <label className="text-slate-600 block mb-1">Endpoint URL:</label>
                      <input
                        type="text"
                        disabled={loading}
                        value={polymarketUrl}
                        onChange={(e) => setPolymarketUrl(e.target.value)}
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                      />
                    </div>
                  </div>
                )}
              </div>
            )}



            {/* Exchange: PopDEX */}
            {selectedExchange === "popdex" && (
              <div className="bg-surface border border-surface-border rounded-2xl p-5 space-y-4 shadow-xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-3">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-xs font-mono font-bold uppercase text-slate-900">
                      PopDEX (Arbitrage Protocol)
                    </h3>
                    <span className="px-2 py-0.5 rounded text-[10px] font-mono bg-blue-50 text-blue-700 border border-blue-200">
                      Lighter Multi-Leg Hybrid
                    </span>
                  </div>
                  <div className="flex items-center space-x-3">
                    <span className="text-[11px] font-mono text-slate-400">
                      Fee: {getVenueInfo("popdex")?.fee_bps ?? 2.0} BPS
                    </span>
                    <ToggleSwitch
                      checked={popdexEnabled}
                      onChange={setPopdexEnabled}
                      label={popdexEnabled ? "Active" : "Disabled"}
                    />
                  </div>
                </div>

                {!popdexEnabled ? (
                  <div className="p-3.5 bg-surface-raised border border-surface-border rounded-xl text-center text-xs font-mono text-slate-500">
                    PopDEX is disabled. Toggle switch to enable and configure API credentials.
                  </div>
                ) : (
                  <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs font-mono">
                    <div>
                      <label className="text-slate-600 block mb-1">Account Index / Address:</label>
                      <input
                        type="text"
                        disabled={loading || isVaultLocked}
                        value={popdexAccountIndex}
                        placeholder="0"
                        onChange={(e) => setPopdexAccountIndex(e.target.value)}
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                      />
                    </div>

                    <div>
                      <label className="text-slate-600 block mb-1">API Key:</label>
                      <input
                        type="text"
                        disabled={loading || isVaultLocked}
                        value={popdexApiKey}
                        placeholder={isVaultLocked ? "••••••••••••••••" : "pop_..."}
                        onChange={(e) => setPopdexApiKey(e.target.value)}
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                      />
                    </div>

                    <div className="md:col-span-2">
                      <label className="text-slate-600 block mb-1">REST Endpoint:</label>
                      <input
                        type="text"
                        disabled={loading}
                        value={popdexUrl}
                        onChange={(e) => setPopdexUrl(e.target.value)}
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                      />
                    </div>
                  </div>
                )}
              </div>
            )}



            {/* Exchange 4: Robinhood EVM */}
            {selectedExchange === "robinhood" && (
              <div className="bg-surface border border-surface-border rounded-2xl p-5 space-y-4 shadow-xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-3">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-xs font-mono font-bold uppercase text-slate-900">
                      Robinhood Chain (Arbitrum L2 EVM)
                    </h3>
                    <span className="px-2 py-0.5 rounded text-[10px] font-mono bg-blue-50 text-blue-700 border border-blue-200">
                      DEX Aggregator Router
                    </span>
                  </div>
                  <div className="flex items-center space-x-3">
                    <span className="text-[11px] font-mono text-slate-400">
                      Fee: {getVenueInfo("robinhood")?.fee_bps ?? 2.5} BPS
                    </span>
                    <ToggleSwitch
                      checked={robinhoodEnabled}
                      onChange={setRobinhoodEnabled}
                      label={robinhoodEnabled ? "Active" : "Disabled"}
                    />
                  </div>
                </div>

                {!robinhoodEnabled ? (
                  <div className="p-3.5 bg-surface-raised border border-surface-border rounded-xl text-center text-xs font-mono text-slate-500">
                    Robinhood EVM is disabled. Toggle switch to enable and configure signer credentials.
                  </div>
                ) : (
                  <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs font-mono">
                    <div>
                      <label className="text-slate-600 block mb-1">EVM Signer Address:</label>
                      <input
                        type="text"
                        disabled={loading || isVaultLocked}
                        value={robinhoodWallet}
                        placeholder="0x..."
                        onChange={(e) => setRobinhoodWallet(e.target.value)}
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                      />
                    </div>

                    <div>
                      <div className="flex items-center justify-between mb-1">
                        <label className="text-slate-600">Signer Private Key:</label>
                        <button
                          type="button"
                          onClick={() => toggleSecret("rh_key")}
                          className="text-slate-400 hover:text-slate-600 text-[11px] flex items-center space-x-1"
                        >
                          {showSecrets["rh_key"] ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3 text-blue-600" />}
                          <span>{showSecrets["rh_key"] ? "Hide" : "Show"}</span>
                        </button>
                      </div>
                      <input
                        type={showSecrets["rh_key"] ? "text" : "password"}
                        disabled={loading || isVaultLocked}
                        value={robinhoodPrivateKey}
                        placeholder={isVaultLocked ? "••••••••••••••••" : "0x..."}
                        onChange={(e) => setRobinhoodPrivateKey(e.target.value)}
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                      />
                    </div>

                    <div>
                      <label className="text-slate-600 block mb-1">RPC URL:</label>
                      <input
                        type="text"
                        disabled={loading}
                        value={robinhoodRpcUrl}
                        onChange={(e) => setRobinhoodRpcUrl(e.target.value)}
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                      />
                    </div>

                    <div>
                      <label className="text-slate-600 block mb-1">Router Address:</label>
                      <input
                        type="text"
                        disabled={loading}
                        value={robinhoodRouter}
                        onChange={(e) => setRobinhoodRouter(e.target.value)}
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                      />
                    </div>
                  </div>
                )}
              </div>
            )}



            {/* Exchange 3: Vanta */}
            {selectedExchange === "vanta" && (
              <div className="bg-surface border border-surface-border rounded-2xl p-5 space-y-4 shadow-xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-3">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-xs font-mono font-bold uppercase text-slate-900">
                      Vanta.exchange (Orderly Network)
                    </h3>
                    <span className="px-2 py-0.5 rounded text-[10px] font-mono bg-blue-50 text-blue-700 border border-blue-200">
                      Ed25519 Orderly
                    </span>
                  </div>
                  <div className="flex items-center space-x-3">
                    <span className="text-[11px] font-mono text-slate-400">
                      Fee: {getVenueInfo("vanta")?.fee_bps ?? 3.0} BPS
                    </span>
                    <ToggleSwitch
                      checked={vantaEnabled}
                      onChange={setVantaEnabled}
                      label={vantaEnabled ? "Active" : "Disabled"}
                    />
                  </div>
                </div>

                {!vantaEnabled ? (
                  <div className="p-3.5 bg-surface-raised border border-surface-border rounded-xl text-center text-xs font-mono text-slate-500">
                    Vanta exchange is disabled. Toggle switch to enable and configure Orderly credentials.
                  </div>
                ) : (
                  <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs font-mono">
                    <div>
                      <label className="text-slate-600 block mb-1">Orderly Account ID:</label>
                      <input
                        type="text"
                        disabled={loading || isVaultLocked}
                        value={vantaAccountId}
                        placeholder="0x..."
                        onChange={(e) => setVantaAccountId(e.target.value)}
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                      />
                    </div>

                    <div>
                      <label className="text-slate-600 block mb-1">Orderly ED25519 Key:</label>
                      <input
                        type="text"
                        disabled={loading || isVaultLocked}
                        value={vantaOrderlyKey}
                        placeholder={isVaultLocked ? "••••••••••••••••" : "ed25519:..."}
                        onChange={(e) => setVantaOrderlyKey(e.target.value)}
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                      />
                    </div>

                    <div>
                      <div className="flex items-center justify-between mb-1">
                        <label className="text-slate-600">Orderly Secret:</label>
                        <button
                          type="button"
                          onClick={() => toggleSecret("vanta_secret")}
                          className="text-slate-400 hover:text-slate-600 text-[11px] flex items-center space-x-1"
                        >
                          {showSecrets["vanta_secret"] ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3 text-blue-600" />}
                          <span>{showSecrets["vanta_secret"] ? "Hide" : "Show"}</span>
                        </button>
                      </div>
                      <input
                        type={showSecrets["vanta_secret"] ? "text" : "password"}
                        disabled={loading || isVaultLocked}
                        value={vantaOrderlySecret}
                        placeholder={isVaultLocked ? "••••••••••••••••" : "secret"}
                        onChange={(e) => setVantaOrderlySecret(e.target.value)}
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                      />
                    </div>

                    <div>
                      <label className="text-slate-600 block mb-1">Subaccount Index (Optional):</label>
                      <input
                        type="number"
                        disabled={loading || isVaultLocked}
                        value={vantaSubaccountIndex}
                        placeholder="e.g. 0"
                        onChange={(e) => setVantaSubaccountIndex(e.target.value)}
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                      />
                    </div>
                  </div>
                )}
              </div>
            )}



              </div>

              {/* Locked Vault Overlay: Completely Greys Out the Exchange Configuration */}
              {isVaultLocked && (
                <div className="absolute inset-0 z-40 bg-slate-900/40 backdrop-blur-xs flex items-center justify-center p-4 animate-in fade-in duration-150">
                  <div className="bg-white rounded-2xl p-6 shadow-2xl w-80 text-center space-y-4 border border-slate-200 animate-in zoom-in-95 duration-150">
                    <div className="w-12 h-12 rounded-2xl bg-blue-50 border border-blue-100 flex items-center justify-center mx-auto text-accent shadow-2xs">
                      <Lock className="w-5 h-5 text-accent" />
                    </div>
                    {api.isTauri ? (
                      <>
                        <div>
                          <h4 className="text-sm font-bold font-mono text-slate-900">Vault Locked</h4>
                          <p className="text-xs text-slate-500 font-sans mt-1">
                            Enter master passcode to unlock exchange API credentials
                          </p>
                        </div>

                        {inlineVaultError && (
                          <div className="p-2 bg-red-50 border border-red-200 rounded-lg text-[11px] font-mono text-red-600">
                            {inlineVaultError}
                          </div>
                        )}

                        <form onSubmit={(e) => { e.preventDefault(); handleInlineUnlock(e); }} className="space-y-3">
                          <input
                            type="password"
                            value={inlinePasscode}
                            onChange={(e) => setInlinePasscode(e.target.value)}
                            placeholder="Enter master passcode..."
                            autoFocus
                            className="w-full px-3 py-2 bg-slate-50 border border-slate-200 rounded-xl text-xs font-mono text-center focus:outline-none focus:ring-2 focus:ring-accent"
                          />
                          <button
                            type="button"
                            onClick={(e) => { e.preventDefault(); handleInlineUnlock(); }}
                            disabled={inlineUnlocking || !inlinePasscode}
                            className="w-full py-2 bg-accent hover:bg-blue-600 text-white font-semibold text-xs font-mono rounded-xl transition shadow-xs disabled:opacity-50 cursor-pointer"
                          >
                            {inlineUnlocking ? "Unlocking..." : "Unlock Vault"}
                          </button>
                        </form>
                      </>
                    ) : (
                      <div>
                        <h4 className="text-sm font-bold font-mono text-slate-900">Browser Demo Mode</h4>
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
            <div className="flex-1 overflow-y-auto p-6 space-y-6">
              {errorMsg && (
                <div className="p-4 bg-red-50 border border-red-200 text-red-700 rounded-xl text-xs font-mono flex items-center space-x-2">
                  <AlertCircle className="w-4 h-4 shrink-0" />
                  <span>{errorMsg}</span>
                </div>
              )}

              {saved && (
                <div className="p-4 bg-emerald-50 border border-emerald-200 text-emerald-700 rounded-xl text-xs font-mono flex items-center space-x-2 animate-in fade-in duration-200">
                  <CheckCircle2 className="w-4 h-4 shrink-0" />
                  <span>Configuration saved successfully and updated in live atomic execution coordinator.</span>
                </div>
              )}
        {/* =================================================================== */}
        {/* SECTION 3: EXECUTION MODES & WARM-UP                               */}
        {/* =================================================================== */}
        {activeTab === "execution" && (
          <div className="bg-surface border border-surface-border rounded-2xl p-5 space-y-4 shadow-xs">
            <div className="border-b border-surface-border pb-3">
              <h3 className="text-xs font-mono font-bold uppercase text-slate-900 flex items-center space-x-2">
                <Sliders className="w-4 h-4 text-accent" />
                <span>Execution Modes & Cache Warm-Up Guardrails</span>
              </h3>
              <p className="text-[11px] text-slate-500 font-mono mt-0.5">
                Configure live order execution vs paper simulation and orderbook persistence
              </p>
            </div>

            <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs font-mono">
              {/* Trading Mode Radio Card */}
              <div className="p-4 bg-surface-raised border border-surface-border rounded-xl space-y-2">
                <label className="text-slate-800 font-bold block text-xs">Trading Mode</label>
                <p className="text-[11px] text-slate-500">
                  Switch between paper simulation and active DEX order placement:
                </p>

                <div className="flex items-center space-x-3 pt-1">
                  <label className="flex items-center space-x-2 cursor-pointer">
                    <input
                      type="radio"
                      name="trading_mode"
                      value="paper"
                      checked={tradingMode === "paper"}
                      onChange={() => setTradingMode("paper")}
                      className="text-accent focus:ring-accent"
                    />
                    <span className="font-semibold text-slate-800">Paper Trading (Live Feeds)</span>
                  </label>
                  <label className="flex items-center space-x-2 cursor-not-allowed opacity-60">
                    <input
                      type="radio"
                      name="trading_mode"
                      value="real"
                      checked={tradingMode === "real"}
                      disabled
                      className="text-amber-600 focus:ring-amber-500"
                    />
                    <span className="font-semibold text-amber-800">Real Trading (Unavailable — authenticated execution not implemented)</span>
                  </label>
                </div>
              </div>

              {/* Orderbook Fast Warm-up Toggle */}
              <div className="p-4 bg-surface-raised border border-surface-border rounded-xl space-y-2">
                <div className="flex items-center justify-between">
                  <label className="text-slate-800 font-bold block text-xs">Orderbook Cache Warm-Up</label>
                  <input
                    type="checkbox"
                    checked={fastWarmupEnabled}
                    onChange={(e) => setFastWarmupEnabled(e.target.checked)}
                    className="w-4 h-4 text-accent rounded focus:ring-accent"
                  />
                </div>
                <p className="text-[11px] text-slate-500">
                  {fastWarmupEnabled
                    ? "Fast Warm-Up: Persists sliding window orderbook quotes across app restarts for zero-lag instant signal readiness."
                    : "Cold Start: Starts orderbook buffer from scratch on every launch (requires 30-60s warm-up window)."}
                </p>
              </div>

              {/* Max Slippage */}
              <div>
                <label className="text-slate-600 block mb-1">Max Slippage Tolerance (BPS):</label>
                <input
                  type="number"
                  step="1"
                  min="1"
                  disabled={loading}
                  value={maxSlippageBps}
                  onChange={(e) => setMaxSlippageBps(parseFloat(e.target.value) || 15)}
                  className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>

              {/* Venue Rebalance Threshold */}
              <div>
                <label className="text-slate-600 block mb-1">Venue Rebalance Warning Threshold ($ USD):</label>
                <input
                  type="number"
                  step="50"
                  min="100"
                  disabled={loading}
                  value={venueRebalanceThresholdUsd}
                  onChange={(e) => setVenueRebalanceThresholdUsd(parseFloat(e.target.value) || 500)}
                  className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>
            </div>
          </div>
        )}

        {/* =================================================================== */}
        {/* SECTION 4: NETWORKING & VPS 24/7 DEPLOYMENT                         */}
        {/* =================================================================== */}
        {activeTab === "network" && (
          <div className="bg-surface border border-surface-border rounded-2xl p-5 space-y-4 shadow-xs">
            <div className="border-b border-surface-border pb-3">
              <h3 className="text-xs font-mono font-bold uppercase text-slate-900 flex items-center space-x-2">
                <Globe className="w-4 h-4 text-accent" />
                <span>Engine Ports & 24/7 VPS Background Daemon</span>
              </h3>
              <p className="text-[11px] text-slate-500 font-mono mt-0.5">
                Configure RPC / WebSocket listener ports and deploy headlessly on cloud servers
              </p>
            </div>

            <div className="grid grid-cols-1 md:grid-cols-3 gap-4 text-xs font-mono">
              <div>
                <label className="text-slate-600 block mb-1">Engine RPC Port:</label>
                <input
                  type="number"
                  disabled={loading}
                  value={engineRpcPort}
                  onChange={(e) => setEngineRpcPort(parseInt(e.target.value, 10) || 8787)}
                  className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>

              <div>
                <label className="text-slate-600 block mb-1">Engine WebSocket Port:</label>
                <input
                  type="number"
                  disabled={loading}
                  value={engineWsPort}
                  onChange={(e) => setEngineWsPort(parseInt(e.target.value, 10) || 8789)}
                  className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>

              <div>
                <label className="text-slate-600 block mb-1">Bind Address:</label>
                <input
                  type="text"
                  disabled={loading}
                  value={bindAddress}
                  placeholder="127.0.0.1 or 0.0.0.0"
                  onChange={(e) => setBindAddress(e.target.value)}
                  className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-xl text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                />
              </div>
            </div>

            {/* VPS Daemon Deployment Guide */}
            <div className="p-4 bg-slate-50 border border-surface-border rounded-xl space-y-2 text-xs font-mono text-slate-700">
              <h4 className="font-bold text-slate-900 flex items-center space-x-1.5">
                <span>🖥️ Headless VPS 24/7 Deployment Instructions</span>
              </h4>
              <p className="text-[11px] text-slate-600">
                To run Arbitrager continuously 24/7 without keeping your local laptop or desktop open, deploy the standalone Rust binary to an Ubuntu/Debian VPS:
              </p>
              <div className="p-2.5 bg-slate-900 text-emerald-400 rounded-lg text-[11px] overflow-x-auto select-text font-mono">
                <code>./target/release/arbitrager serve --bind 0.0.0.0 --port 8787</code>
              </div>
              <p className="text-[10px] text-slate-500">
                The serve command exposes a read-only health endpoint only — no market data ingestion or order execution runs. You can create a systemd service (e.g. <code className="bg-slate-200 px-1 py-0.5 rounded">systemctl start arbitrager</code>) to auto-restart on server reboot.
              </p>
            </div>
          </div>
        )}

        {/* =================================================================== */}
        {/* SECTION 5: ABOUT, CHECK FOR UPDATES & BUY ME A SOLO                 */}
        {/* =================================================================== */}
        {activeTab === "about" && (
          <div className="space-y-4">
            <div className="bg-surface border border-surface-border rounded-2xl p-6 shadow-xs space-y-5">
              <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4 border-b border-surface-border pb-4">
                <div className="flex items-center space-x-3">
                  <div className="w-10 h-10 rounded-xl bg-accent-dim border border-accent-line flex items-center justify-center text-accent">
                    <ArbitragerLogo size={22} className="text-accent" />
                  </div>
                  <div>
                    <h3 className="text-sm font-bold font-mono text-slate-900">
                      About Arbitrager ({version})
                    </h3>
                  </div>
                </div>

                <div className="flex items-center space-x-2">
                  <button
                    type="button"
                    onClick={handleCheckForUpdates}
                    disabled={checkingUpdate}
                    className="px-3.5 py-1.5 bg-surface-raised hover:bg-slate-200 border border-surface-border text-slate-800 rounded-xl text-xs font-mono font-medium transition flex items-center space-x-1.5 shadow-xs"
                  >
                    <RefreshCw className={`w-3.5 h-3.5 ${checkingUpdate ? "animate-spin text-accent" : ""}`} />
                    <span>{checkingUpdate ? "Checking..." : "Check for Updates"}</span>
                  </button>

                  <a
                    href={gitRepo}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="px-3.5 py-1.5 bg-surface hover:bg-surface-raised border border-surface-border text-slate-800 rounded-xl text-xs font-mono font-medium transition flex items-center space-x-1.5 shadow-xs"
                  >
                    <span>Git Repository</span>
                    <ExternalLink className="w-3.5 h-3.5" />
                  </a>
                </div>
              </div>

              {updateMsg && (
                <div className="p-3 bg-emerald-50 border border-emerald-200 text-emerald-800 rounded-xl text-xs font-mono flex items-center space-x-2">
                  <CheckCircle2 className="w-4 h-4 text-emerald-600 shrink-0" />
                  <span>{updateMsg}</span>
                </div>
              )}

              {/* Sponsor Card: Buy me a SOLO */}
              <div className="p-5 bg-gradient-to-r from-amber-50/80 via-yellow-50/50 to-amber-50/80 border border-amber-200 rounded-2xl flex flex-col sm:flex-row items-center justify-between gap-4 shadow-xs">
                <div className="flex items-center space-x-4">
                  <img
                    src="/solo_can.jpg"
                    alt="Can of Australian SOLO Lemon Soda"
                    className="w-14 h-14 object-contain rounded-xl shadow-xs border border-amber-200 bg-white"
                  />
                  <div>
                    <h4 className="text-sm font-bold font-mono text-slate-900">
                      Enjoying the tool? Buy me a SOLO!
                    </h4>
                    <p className="text-xs text-slate-600 font-mono mt-0.5">
                      Fuel the research and development of ultra-low latency quantitative arbitrage strategies.
                    </p>
                  </div>
                </div>

                <a
                  href={sponsorLink}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="px-5 py-2.5 bg-amber-500 hover:bg-amber-600 text-white font-bold text-xs font-mono rounded-xl transition shadow-xs flex items-center space-x-2 shrink-0"
                >
                  <span>Buy me a SOLO ($5)</span>
                  <ExternalLink className="w-3.5 h-3.5" />
                </a>
              </div>

              {/* Legal & Financial Disclaimer */}
              <div className="p-4 bg-slate-50 border border-surface-border rounded-xl space-y-2 text-[11px] font-mono text-slate-600 leading-relaxed">
                <div className="flex items-center space-x-1.5 font-bold text-slate-800 uppercase text-[10px]">
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
          className="px-2.5 py-1.5 rounded-lg border border-surface-border bg-surface text-slate-700 text-[11px] font-mono flex items-center space-x-1.5 shadow-2xs"
          title={`Execution mode: ${tradingMode.toUpperCase()}`}
        >
          <span className="text-slate-400 font-normal">Mode:</span>
          <span className="font-bold text-accent uppercase">{tradingMode}</span>
        </div>

        <div className="flex items-center space-x-2.5">
          {onClose && (
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-2 bg-surface-raised hover:bg-slate-200 text-slate-700 border border-surface-border font-medium text-xs font-mono rounded-xl transition shadow-xs cursor-pointer"
            >
              Done
            </button>
          )}
          <button
            type="submit"
            disabled={loading || saving}
            className="px-5 py-2 bg-accent hover:bg-blue-600 text-white font-medium text-xs font-mono rounded-xl transition flex items-center space-x-2 shadow-xs disabled:opacity-50 cursor-pointer"
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
</div>
  );
};
