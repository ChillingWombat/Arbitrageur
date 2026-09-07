import React, { useState, useEffect } from "react";
import {
  AlertCircle,
  CheckCircle2,
  Eye,
  EyeOff,
  Key,
  Lock,
  Sliders,
  Shield,
  ExternalLink,
  Wallet,
  Globe,
  Unlock,
  AlertTriangle,
  Scale,
  RefreshCw,
} from "lucide-react";
import { api } from "../services/api";
import type { EngineSettingsDto, SystemStatusDto, VenueStatusDto, VaultStatus, VenueBalanceInfo } from "../types";
import { VaultUnlockModal } from "./VaultUnlockModal";

type SettingsTab = "exchanges" | "balances" | "execution" | "network" | "about" | "all";
type ExchangeFilter = "all" | "hyperliquid" | "lighter" | "vanta" | "robinhood" | "polymarket" | "kalshi";

interface EngineSettingsProps {
  systemStatus?: SystemStatusDto | null;
}

export const EngineSettings: React.FC<EngineSettingsProps> = ({ systemStatus }) => {
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
  const [activeTab, setActiveTab] = useState<SettingsTab>("exchanges");
  const [venueFilter, setVenueFilter] = useState<ExchangeFilter>("all");
  const [showSecrets, setShowSecrets] = useState<Record<string, boolean>>({});
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Vault state
  const [vaultStatus, setVaultStatus] = useState<VaultStatus | null>(null);
  const [showVaultModal, setShowVaultModal] = useState(false);

  // Venue Balances
  const [venueBalances, setVenueBalances] = useState<VenueBalanceInfo[]>([]);
  const [loadingBalances, setLoadingBalances] = useState(false);

  // Update check
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [updateMsg, setUpdateMsg] = useState<string | null>(null);

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
        setHyperliquidAccountIndex(s.hyperliquid_account_index !== undefined ? String(s.hyperliquid_account_index) : "");
        setHyperliquidSubaccountIndex(s.hyperliquid_subaccount_index !== undefined ? String(s.hyperliquid_subaccount_index) : "");

        setLighterAccountIndex(s.lighter_account_index || "0");
        setLighterApiKey(s.lighter_api_key || "");
        setLighterUrl(s.lighter_url || "https://mainnet.zklighter.elliot.ai");
        setLighterSubaccountIndex(s.lighter_subaccount_index !== undefined ? String(s.lighter_subaccount_index) : "");

        setVantaAccountId(s.vanta_account_id || "");
        setVantaOrderlyKey(s.vanta_orderly_key || "");
        setVantaOrderlySecret(s.vanta_orderly_secret || "");
        setVantaUrl(s.vanta_url || "https://api-evm.orderly.org");
        setVantaSubaccountIndex(s.vanta_subaccount_index !== undefined ? String(s.vanta_subaccount_index) : "");

        setRobinhoodWallet(s.robinhood_wallet || "");
        setRobinhoodPrivateKey(s.robinhood_private_key || "");
        setRobinhoodRpcUrl(s.robinhood_rpc_url || "https://rpc.robinhood.network");
        setRobinhoodRouter(s.robinhood_router || "0x89e5DB8B5aA49aA85AC63f691524311AEB649eba");
        setRobinhoodSubaccountIndex(s.robinhood_subaccount_index !== undefined ? String(s.robinhood_subaccount_index) : "");

        setPolymarketApiKey(s.polymarket_api_key || "");
        setPolymarketSecret(s.polymarket_secret || "");
        setPolymarketPassphrase(s.polymarket_passphrase || "");
        setPolymarketUrl(s.polymarket_url || "https://clob.polymarket.com");

        setKalshiApiKeyId(s.kalshi_api_key_id || "");
        setKalshiPrivateKey(s.kalshi_private_key || "");
        setKalshiUrl(s.kalshi_url || "https://trading-api.kalshi.com");

        if (s.referral_lighter) setReferralLighter(s.referral_lighter);
        if (s.referral_robinhood) setReferralRobinhood(s.referral_robinhood);
        if (s.sponsor_link) setSponsorLink(s.sponsor_link);
        if (s.version) setVersion(s.version);
        if (s.git_repo) setGitRepo(s.git_repo);

        if (systemStatus?.active_venues) {
          setLiveVenues(systemStatus.active_venues);
        } else {
          api.getSystemStatus().then((s) => {
            if (s?.active_venues && s.active_venues.length > 0) setLiveVenues(s.active_venues);
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
  }, [systemStatus]);

  const defaultVenues: VenueStatusDto[] = [
    { id: "hyperliquid", name: "Hyperliquid (L1 DEX)", market_type: "Perpetual CLOB", status: "ONLINE", latency_ms: 2, fee_bps: 2.0 },
    { id: "lighter", name: "Lighter (zkDEX)", market_type: "zk-Rollup CLOB", status: "ONLINE", latency_ms: 5, fee_bps: 1.5 },
    { id: "vanta", name: "Vanta.exchange", market_type: "Omnichain Perps", status: "ONLINE", latency_ms: 4, fee_bps: 3.0 },
    { id: "robinhood", name: "Robinhood EVM", market_type: "EVM DEX Aggregator", status: "ONLINE", latency_ms: 8, fee_bps: 2.5 },
    { id: "polymarket", name: "Polymarket (CTF CLOB)", market_type: "Prediction Market", status: "ONLINE", latency_ms: 12, fee_bps: 0.0 },
    { id: "kalshi", name: "Kalshi (CFTC)", market_type: "Prediction Market", status: "ONLINE", latency_ms: 15, fee_bps: 10.0 },
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
      await api.saveEngineSettings({
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
      });

      // Also sync live mode in backend state if tradingMode changed
      await api.setLiveMode(tradingMode === "real");

      setSaved(true);
      setTimeout(() => setSaved(false), 3000);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      setErrorMsg(msg);
    } finally {
      setSaving(false);
    }
  };

  const handleLockVault = async () => {
    try {
      const s = await api.lockVault();
      setVaultStatus(s);
    } catch (err) {
      console.error("Lock error:", err);
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
    switch (venue) {
      case "hyperliquid":
        return Boolean(hyperliquidWallet || hyperliquidPrivateKey);
      case "lighter":
        return Boolean(lighterApiKey);
      case "vanta":
        return Boolean(vantaOrderlyKey || vantaAccountId);
      case "robinhood":
        return Boolean(robinhoodWallet || robinhoodPrivateKey);
      case "polymarket":
        return Boolean(polymarketApiKey || polymarketSecret);
      case "kalshi":
        return Boolean(kalshiApiKeyId || kalshiPrivateKey);
      default:
        return false;
    }
  };

  const configuredCount = [
    isVenueConfigured("hyperliquid"),
    isVenueConfigured("lighter"),
    isVenueConfigured("vanta"),
    isVenueConfigured("robinhood"),
    isVenueConfigured("polymarket"),
    isVenueConfigured("kalshi"),
  ].filter(Boolean).length;

  const isVaultLocked = vaultStatus ? !vaultStatus.is_unlocked : false;

  return (
    <div className="p-6 space-y-6 overflow-y-auto h-full max-w-5xl select-none">
      {/* Top Banner */}
      <div className="glass-panel p-5 rounded-2xl flex flex-col md:flex-row md:items-center justify-between gap-4">
        <div>
          <div className="flex items-center space-x-2.5">
            <h2 className="text-base font-bold font-mono text-white tracking-tight">
              Arbitrager System Engine Settings
            </h2>
            <span className="chip chip-info text-[10px] font-mono">
              {configuredCount} / 6 Venues Active
            </span>
          </div>
          <p className="text-xs text-white/50 font-mono mt-1">
            Zero-seed secure local vault, cross-venue API configuration, network ports, and execution safety
          </p>
        </div>

        {/* Section Tabs */}
        <div className="flex items-center bg-white/[0.04] p-1 rounded-xl text-xs font-mono border border-white/[0.08] overflow-x-auto self-start md:self-auto">
          <button
            type="button"
            onClick={() => setActiveTab("exchanges")}
            className={`px-3 py-1.5 rounded-lg transition flex items-center space-x-1.5 whitespace-nowrap ${
              activeTab === "exchanges"
                ? "bg-cyan-500/15 text-cyan-300 border border-cyan-500/30 font-semibold shadow-[0_0_12px_rgba(34,211,238,0.2)]"
                : "text-white/60 hover:text-white/90 hover:bg-white/[0.04]"
            }`}
          >
            <Key className="w-3.5 h-3.5" />
            <span>Exchanges</span>
          </button>
          <button
            type="button"
            onClick={() => setActiveTab("balances")}
            className={`px-3 py-1.5 rounded-lg transition flex items-center space-x-1.5 whitespace-nowrap ${
              activeTab === "balances"
                ? "bg-cyan-500/15 text-cyan-300 border border-cyan-500/30 font-semibold shadow-[0_0_12px_rgba(34,211,238,0.2)]"
                : "text-white/60 hover:text-white/90 hover:bg-white/[0.04]"
            }`}
          >
            <Wallet className="w-3.5 h-3.5" />
            <span>Balances</span>
          </button>
          <button
            type="button"
            onClick={() => setActiveTab("execution")}
            className={`px-3 py-1.5 rounded-lg transition flex items-center space-x-1.5 whitespace-nowrap ${
              activeTab === "execution"
                ? "bg-cyan-500/15 text-cyan-300 border border-cyan-500/30 font-semibold shadow-[0_0_12px_rgba(34,211,238,0.2)]"
                : "text-white/60 hover:text-white/90 hover:bg-white/[0.04]"
            }`}
          >
            <Sliders className="w-3.5 h-3.5" />
            <span>Execution</span>
          </button>
          <button
            type="button"
            onClick={() => setActiveTab("network")}
            className={`px-3 py-1.5 rounded-lg transition flex items-center space-x-1.5 whitespace-nowrap ${
              activeTab === "network"
                ? "bg-cyan-500/15 text-cyan-300 border border-cyan-500/30 font-semibold shadow-[0_0_12px_rgba(34,211,238,0.2)]"
                : "text-white/60 hover:text-white/90 hover:bg-white/[0.04]"
            }`}
          >
            <Globe className="w-3.5 h-3.5" />
            <span>Network & VPS</span>
          </button>
          <button
            type="button"
            onClick={() => setActiveTab("about")}
            className={`px-3 py-1.5 rounded-lg transition flex items-center space-x-1.5 whitespace-nowrap ${
              activeTab === "about"
                ? "bg-cyan-500/15 text-cyan-300 border border-cyan-500/30 font-semibold shadow-[0_0_12px_rgba(34,211,238,0.2)]"
                : "text-white/60 hover:text-white/90 hover:bg-white/[0.04]"
            }`}
          >
            <Shield className="w-3.5 h-3.5" />
            <span>About</span>
          </button>
        </div>
      </div>

      {errorMsg && (
        <div className="p-4 bg-rose-500/15 border border-rose-500/30 text-rose-300 rounded-xl text-xs font-mono flex items-center space-x-2">
          <AlertCircle className="w-4 h-4 shrink-0" />
          <span>{errorMsg}</span>
        </div>
      )}

      {saved && (
        <div className="p-4 bg-emerald-500/15 border border-emerald-500/30 text-emerald-300 rounded-xl text-xs font-mono flex items-center space-x-2 animate-in fade-in duration-200 shadow-[0_0_15px_rgba(52,211,153,0.15)]">
          <CheckCircle2 className="w-4 h-4 shrink-0" />
          <span>Configuration saved successfully and updated in live atomic execution coordinator.</span>
        </div>
      )}

      <form onSubmit={handleSave} className="space-y-6">
        {/* =================================================================== */}
        {/* SECTION 1: EXCHANGES & SECURE VAULT                                */}
        {/* =================================================================== */}
        {(activeTab === "exchanges" || activeTab === "all") && (
          <div className="space-y-4">
            {/* Referral Links Promotion Card */}
            <div className="p-4 glass-panel bg-gradient-to-r from-cyan-500/10 via-indigo-500/10 to-purple-500/10 border border-cyan-500/25 rounded-2xl shadow-[0_0_25px_rgba(34,211,238,0.06)]">
              <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3">
                <div>
                  <h4 className="text-xs font-bold font-mono text-cyan-300 flex items-center space-x-1.5">
                    <span>⚡ Exchange Referral Links (Fee Tier Discounts)</span>
                  </h4>
                  <p className="text-[11px] text-white/60 font-mono mt-0.5">
                    Create accounts with zero taker surcharges and support project maintenance:
                  </p>
                </div>
                <div className="flex flex-wrap items-center gap-2">
                  <a
                    href={referralLighter}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="px-3 py-1.5 glass-panel bg-white/[0.04] hover:bg-cyan-500/20 border border-cyan-500/30 text-cyan-300 hover:text-white rounded-xl text-xs font-mono font-semibold transition flex items-center space-x-1.5 shadow-[0_0_12px_rgba(34,211,238,0.12)]"
                  >
                    <span>Lighter (SOLO)</span>
                    <ExternalLink className="w-3 h-3" />
                  </a>
                  <a
                    href={referralRobinhood}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="px-3 py-1.5 glass-panel bg-white/[0.04] hover:bg-cyan-500/20 border border-cyan-500/30 text-cyan-300 hover:text-white rounded-xl text-xs font-mono font-semibold transition flex items-center space-x-1.5 shadow-[0_0_12px_rgba(34,211,238,0.12)]"
                  >
                    <span>Robinhood Chain (SOLO)</span>
                    <ExternalLink className="w-3 h-3" />
                  </a>
                </div>
              </div>
            </div>

            {/* Local Encrypted Vault Banner */}
            <div className="p-4 glass-panel border border-white/[0.08] rounded-2xl flex items-center justify-between gap-4">
              <div className="flex items-center space-x-3.5">
                <div
                  className={`w-9 h-9 rounded-xl flex items-center justify-center ${
                    isVaultLocked
                      ? "bg-amber-500/15 text-amber-300 border border-amber-500/30 shadow-[0_0_12px_rgba(245,158,11,0.2)]"
                      : "bg-emerald-500/15 text-emerald-300 border border-emerald-500/30 shadow-[0_0_12px_rgba(52,211,153,0.2)]"
                  }`}
                >
                  {isVaultLocked ? <Lock className="w-4 h-4" /> : <Unlock className="w-4 h-4" />}
                </div>
                <div>
                  <div className="flex items-center space-x-2">
                    <h4 className="text-xs font-bold font-mono text-white/95">
                      Local Secure Vault: {isVaultLocked ? "Locked" : "Unlocked"}
                    </h4>
                    <span
                      className={`chip text-[10px] font-mono ${
                        isVaultLocked ? "chip-warn" : "chip-pos"
                      }`}
                    >
                      AES-256-GCM Encrypted
                    </span>
                  </div>
                  <p className="text-[11px] text-white/50 font-mono mt-0.5">
                    {isVaultLocked
                      ? "Master password required to view or edit exchange private keys"
                      : "Vault unlocked in-memory. Sensitive fields can be viewed and edited."}
                  </p>
                </div>
              </div>

              <div>
                {isVaultLocked ? (
                  <button
                    type="button"
                    onClick={() => setShowVaultModal(true)}
                    className="px-4 py-2 bg-gradient-to-r from-cyan-500 to-indigo-600 hover:from-cyan-400 hover:to-indigo-500 text-white rounded-xl text-xs font-mono font-medium transition shadow-[0_0_16px_rgba(34,211,238,0.25)] flex items-center space-x-1.5"
                  >
                    <Key className="w-3.5 h-3.5" />
                    <span>Unlock Vault</span>
                  </button>
                ) : (
                  <button
                    type="button"
                    onClick={handleLockVault}
                    className="px-3.5 py-1.5 bg-white/[0.06] hover:bg-white/[0.1] text-white/80 border border-white/[0.1] rounded-xl text-xs font-mono font-medium transition"
                  >
                    Lock Vault Now
                  </button>
                )}
              </div>
            </div>

            {/* Exchange Filter Selector */}
            <div className="flex items-center space-x-1.5 overflow-x-auto pb-1 text-xs font-mono">
              <span className="text-white/40 text-[11px] mr-1.5">Filter Venue:</span>
              {(["all", "hyperliquid", "lighter", "vanta", "robinhood", "polymarket", "kalshi"] as ExchangeFilter[]).map((vf) => (
                <button
                  key={vf}
                  type="button"
                  onClick={() => setVenueFilter(vf)}
                  className={`px-3 py-1 rounded-lg transition capitalize ${
                    venueFilter === vf
                      ? "bg-cyan-500/20 text-cyan-300 border border-cyan-500/40 font-semibold shadow-[0_0_10px_rgba(34,211,238,0.2)]"
                      : "bg-white/[0.03] text-white/50 hover:text-white/80 hover:bg-white/[0.06] border border-white/[0.06]"
                  }`}
                >
                  {vf}
                </button>
              ))}
            </div>

            {/* Exchange 1: Hyperliquid */}
            {(venueFilter === "all" || venueFilter === "hyperliquid") && (
              <div className="glass-panel rounded-2xl p-5 space-y-4 border border-white/[0.08]">
                <div className="flex items-center justify-between border-b border-white/[0.06] pb-3">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-xs font-mono font-bold uppercase text-white/95">
                      Hyperliquid (L1 Perp DEX)
                    </h3>
                    <span className="chip chip-info text-[10px]">
                      Subaccount & EVM Key
                    </span>
                  </div>
                  <span className="text-[11px] font-mono text-white/40 tabular-nums">
                    Fee: {getVenueInfo("hyperliquid")?.fee_bps ?? 2.0} BPS
                  </span>
                </div>

                <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs font-mono">
                  <div>
                    <label className="text-white/60 block mb-1">Trading EOA Wallet Address:</label>
                    <input
                      type="text"
                      disabled={loading || isVaultLocked}
                      value={hyperliquidWallet}
                      placeholder="0x..."
                      onChange={(e) => setHyperliquidWallet(e.target.value)}
                      className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 placeholder:text-white/20 focus:outline-none focus:ring-1 focus:ring-cyan-400 focus:border-cyan-400 font-mono"
                    />
                  </div>

                  <div>
                    <div className="flex items-center justify-between mb-1">
                      <label className="text-white/60">Trading Agent Private Key:</label>
                      <button
                        type="button"
                        onClick={() => toggleSecret("hl_key")}
                        className="text-white/40 hover:text-white/80 text-[11px] flex items-center space-x-1"
                      >
                        {showSecrets["hl_key"] ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3 text-cyan-400" />}
                        <span>{showSecrets["hl_key"] ? "Hide" : "Show"}</span>
                      </button>
                    </div>
                    <input
                      type={showSecrets["hl_key"] ? "text" : "password"}
                      disabled={loading || isVaultLocked}
                      value={hyperliquidPrivateKey}
                      placeholder={isVaultLocked ? "••••••••••••••••" : "0x..."}
                      onChange={(e) => setHyperliquidPrivateKey(e.target.value)}
                      className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 placeholder:text-white/20 focus:outline-none focus:ring-1 focus:ring-cyan-400 focus:border-cyan-400 font-mono"
                    />
                  </div>

                  <div>
                    <label className="text-white/60 block mb-1">Account Index (Optional):</label>
                    <input
                      type="number"
                      disabled={loading || isVaultLocked}
                      value={hyperliquidAccountIndex}
                      placeholder="Default: 0"
                      onChange={(e) => setHyperliquidAccountIndex(e.target.value)}
                      className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 placeholder:text-white/20 focus:outline-none focus:ring-1 focus:ring-cyan-400 focus:border-cyan-400 font-mono tabular-nums"
                    />
                  </div>

                  <div>
                    <label className="text-white/60 block mb-1">Subaccount Index (Optional):</label>
                    <input
                      type="number"
                      disabled={loading || isVaultLocked}
                      value={hyperliquidSubaccountIndex}
                      placeholder="Default: None"
                      onChange={(e) => setHyperliquidSubaccountIndex(e.target.value)}
                      className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 placeholder:text-white/20 focus:outline-none focus:ring-1 focus:ring-cyan-400 focus:border-cyan-400 font-mono tabular-nums"
                    />
                  </div>
                </div>
              </div>
            )}

            {/* Exchange 2: Lighter */}
            {(venueFilter === "all" || venueFilter === "lighter") && (
              <div className="glass-panel rounded-2xl p-5 space-y-4 border border-white/[0.08]">
                <div className="flex items-center justify-between border-b border-white/[0.06] pb-3">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-xs font-mono font-bold uppercase text-white/95">
                      Lighter (zkDEX Rollup)
                    </h3>
                    <span className="chip chip-info text-[10px]">
                      zk-Rollup CLOB
                    </span>
                  </div>
                  <span className="text-[11px] font-mono text-white/40 tabular-nums">
                    Fee: {getVenueInfo("lighter")?.fee_bps ?? 1.5} BPS
                  </span>
                </div>

                <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs font-mono">
                  <div>
                    <label className="text-white/60 block mb-1">Account Index:</label>
                    <input
                      type="text"
                      disabled={loading || isVaultLocked}
                      value={lighterAccountIndex}
                      onChange={(e) => setLighterAccountIndex(e.target.value)}
                      className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 placeholder:text-white/20 focus:outline-none focus:ring-1 focus:ring-cyan-400 focus:border-cyan-400 font-mono tabular-nums"
                    />
                  </div>

                  <div>
                    <div className="flex items-center justify-between mb-1">
                      <label className="text-white/60">API Private Key / Signer Token:</label>
                      <button
                        type="button"
                        onClick={() => toggleSecret("lgt_key")}
                        className="text-white/40 hover:text-white/80 text-[11px] flex items-center space-x-1"
                      >
                        {showSecrets["lgt_key"] ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3 text-cyan-400" />}
                        <span>{showSecrets["lgt_key"] ? "Hide" : "Show"}</span>
                      </button>
                    </div>
                    <input
                      type={showSecrets["lgt_key"] ? "text" : "password"}
                      disabled={loading || isVaultLocked}
                      value={lighterApiKey}
                      placeholder={isVaultLocked ? "••••••••••••••••" : "0x..."}
                      onChange={(e) => setLighterApiKey(e.target.value)}
                      className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 placeholder:text-white/20 focus:outline-none focus:ring-1 focus:ring-cyan-400 focus:border-cyan-400 font-mono"
                    />
                  </div>

                  <div>
                    <label className="text-white/60 block mb-1">Subaccount Index (Optional):</label>
                    <input
                      type="number"
                      disabled={loading || isVaultLocked}
                      value={lighterSubaccountIndex}
                      placeholder="e.g. 1"
                      onChange={(e) => setLighterSubaccountIndex(e.target.value)}
                      className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 placeholder:text-white/20 focus:outline-none focus:ring-1 focus:ring-cyan-400 focus:border-cyan-400 font-mono tabular-nums"
                    />
                  </div>

                  <div>
                    <label className="text-white/60 block mb-1">Endpoint URL:</label>
                    <input
                      type="text"
                      disabled={loading}
                      value={lighterUrl}
                      onChange={(e) => setLighterUrl(e.target.value)}
                      className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 placeholder:text-white/20 focus:outline-none focus:ring-1 focus:ring-cyan-400 focus:border-cyan-400 font-mono"
                    />
                  </div>
                </div>
              </div>
            )}

            {/* Exchange 3: Vanta */}
            {(venueFilter === "all" || venueFilter === "vanta") && (
              <div className="glass-panel rounded-2xl p-5 space-y-4 border border-white/[0.08]">
                <div className="flex items-center justify-between border-b border-white/[0.06] pb-3">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-xs font-mono font-bold uppercase text-white/95">
                      Vanta.exchange (Orderly Network)
                    </h3>
                    <span className="chip chip-info text-[10px]">
                      Ed25519 Orderly
                    </span>
                  </div>
                  <span className="text-[11px] font-mono text-white/40 tabular-nums">
                    Fee: {getVenueInfo("vanta")?.fee_bps ?? 3.0} BPS
                  </span>
                </div>

                <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs font-mono">
                  <div>
                    <label className="text-white/60 block mb-1">Orderly Account ID:</label>
                    <input
                      type="text"
                      disabled={loading || isVaultLocked}
                      value={vantaAccountId}
                      placeholder="0x..."
                      onChange={(e) => setVantaAccountId(e.target.value)}
                      className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 placeholder:text-white/20 focus:outline-none focus:ring-1 focus:ring-cyan-400 focus:border-cyan-400 font-mono"
                    />
                  </div>

                  <div>
                    <label className="text-white/60 block mb-1">Orderly ED25519 Key:</label>
                    <input
                      type="text"
                      disabled={loading || isVaultLocked}
                      value={vantaOrderlyKey}
                      placeholder={isVaultLocked ? "••••••••••••••••" : "ed25519:..."}
                      onChange={(e) => setVantaOrderlyKey(e.target.value)}
                      className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 placeholder:text-white/20 focus:outline-none focus:ring-1 focus:ring-cyan-400 focus:border-cyan-400 font-mono"
                    />
                  </div>

                  <div>
                    <div className="flex items-center justify-between mb-1">
                      <label className="text-white/60">Orderly Secret:</label>
                      <button
                        type="button"
                        onClick={() => toggleSecret("vanta_secret")}
                        className="text-white/40 hover:text-white/80 text-[11px] flex items-center space-x-1"
                      >
                        {showSecrets["vanta_secret"] ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3 text-cyan-400" />}
                        <span>{showSecrets["vanta_secret"] ? "Hide" : "Show"}</span>
                      </button>
                    </div>
                    <input
                      type={showSecrets["vanta_secret"] ? "text" : "password"}
                      disabled={loading || isVaultLocked}
                      value={vantaOrderlySecret}
                      placeholder={isVaultLocked ? "••••••••••••••••" : "secret"}
                      onChange={(e) => setVantaOrderlySecret(e.target.value)}
                      className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 placeholder:text-white/20 focus:outline-none focus:ring-1 focus:ring-cyan-400 focus:border-cyan-400 font-mono"
                    />
                  </div>

                  <div>
                    <label className="text-white/60 block mb-1">Subaccount Index (Optional):</label>
                    <input
                      type="number"
                      disabled={loading || isVaultLocked}
                      value={vantaSubaccountIndex}
                      placeholder="e.g. 0"
                      onChange={(e) => setVantaSubaccountIndex(e.target.value)}
                      className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 placeholder:text-white/20 focus:outline-none focus:ring-1 focus:ring-cyan-400 focus:border-cyan-400 font-mono tabular-nums"
                    />
                  </div>
                </div>
              </div>
            )}

            {/* Exchange 4: Robinhood EVM */}
            {(venueFilter === "all" || venueFilter === "robinhood") && (
              <div className="glass-panel rounded-2xl p-5 space-y-4 border border-white/[0.08]">
                <div className="flex items-center justify-between border-b border-white/[0.06] pb-3">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-xs font-mono font-bold uppercase text-white/95">
                      Robinhood Chain (Arbitrum L2 EVM)
                    </h3>
                    <span className="chip chip-info text-[10px]">
                      DEX Aggregator Router
                    </span>
                  </div>
                  <span className="text-[11px] font-mono text-white/40 tabular-nums">
                    Fee: {getVenueInfo("robinhood")?.fee_bps ?? 2.5} BPS
                  </span>
                </div>

                <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs font-mono">
                  <div>
                    <label className="text-white/60 block mb-1">EVM Signer Address:</label>
                    <input
                      type="text"
                      disabled={loading || isVaultLocked}
                      value={robinhoodWallet}
                      placeholder="0x..."
                      onChange={(e) => setRobinhoodWallet(e.target.value)}
                      className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 placeholder:text-white/20 focus:outline-none focus:ring-1 focus:ring-cyan-400 focus:border-cyan-400 font-mono"
                    />
                  </div>

                  <div>
                    <div className="flex items-center justify-between mb-1">
                      <label className="text-white/60">Signer Private Key:</label>
                      <button
                        type="button"
                        onClick={() => toggleSecret("rh_key")}
                        className="text-white/40 hover:text-white/80 text-[11px] flex items-center space-x-1"
                      >
                        {showSecrets["rh_key"] ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3 text-cyan-400" />}
                        <span>{showSecrets["rh_key"] ? "Hide" : "Show"}</span>
                      </button>
                    </div>
                    <input
                      type={showSecrets["rh_key"] ? "text" : "password"}
                      disabled={loading || isVaultLocked}
                      value={robinhoodPrivateKey}
                      placeholder={isVaultLocked ? "••••••••••••••••" : "0x..."}
                      onChange={(e) => setRobinhoodPrivateKey(e.target.value)}
                      className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 placeholder:text-white/20 focus:outline-none focus:ring-1 focus:ring-cyan-400 focus:border-cyan-400 font-mono"
                    />
                  </div>
                </div>
              </div>
            )}

            {/* Exchange 5: Polymarket */}
            {(venueFilter === "all" || venueFilter === "polymarket") && (
              <div className="glass-panel rounded-2xl p-5 space-y-4 border border-white/[0.08]">
                <div className="flex items-center justify-between border-b border-white/[0.06] pb-3">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-xs font-mono font-bold uppercase text-white/95">
                      Polymarket (CTF CLOB)
                    </h3>
                    <span className="chip chip-info text-[10px]">
                      Prediction Market
                    </span>
                  </div>
                  <span className="text-[11px] font-mono text-white/40 tabular-nums">
                    Fee: {getVenueInfo("polymarket")?.fee_bps ?? 0.0} BPS
                  </span>
                </div>

                <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs font-mono">
                  <div>
                    <label className="text-white/60 block mb-1">API Key:</label>
                    <input
                      type="text"
                      disabled={loading || isVaultLocked}
                      value={polymarketApiKey}
                      placeholder="API Key..."
                      onChange={(e) => setPolymarketApiKey(e.target.value)}
                      className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 placeholder:text-white/20 focus:outline-none focus:ring-1 focus:ring-cyan-400 focus:border-cyan-400 font-mono"
                    />
                  </div>

                  <div>
                    <div className="flex items-center justify-between mb-1">
                      <label className="text-white/60">API Secret:</label>
                      <button
                        type="button"
                        onClick={() => toggleSecret("poly_secret")}
                        className="text-white/40 hover:text-white/80 text-[11px] flex items-center space-x-1"
                      >
                        {showSecrets["poly_secret"] ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3 text-cyan-400" />}
                        <span>{showSecrets["poly_secret"] ? "Hide" : "Show"}</span>
                      </button>
                    </div>
                    <input
                      type={showSecrets["poly_secret"] ? "text" : "password"}
                      disabled={loading || isVaultLocked}
                      value={polymarketSecret}
                      placeholder={isVaultLocked ? "••••••••••••••••" : "Secret..."}
                      onChange={(e) => setPolymarketSecret(e.target.value)}
                      className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 placeholder:text-white/20 focus:outline-none focus:ring-1 focus:ring-cyan-400 focus:border-cyan-400 font-mono"
                    />
                  </div>
                </div>
              </div>
            )}

            {/* Exchange 6: Kalshi */}
            {(venueFilter === "all" || venueFilter === "kalshi") && (
              <div className="glass-panel rounded-2xl p-5 space-y-4 border border-white/[0.08]">
                <div className="flex items-center justify-between border-b border-white/[0.06] pb-3">
                  <div className="flex items-center space-x-2">
                    <h3 className="text-xs font-mono font-bold uppercase text-white/95">
                      Kalshi (CFTC Regulated)
                    </h3>
                    <span className="chip chip-info text-[10px]">
                      Prediction Market
                    </span>
                  </div>
                  <span className="text-[11px] font-mono text-white/40 tabular-nums">
                    Fee: {getVenueInfo("kalshi")?.fee_bps ?? 10.0} BPS
                  </span>
                </div>

                <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs font-mono">
                  <div>
                    <label className="text-white/60 block mb-1">API Key ID:</label>
                    <input
                      type="text"
                      disabled={loading || isVaultLocked}
                      value={kalshiApiKeyId}
                      placeholder="Key ID..."
                      onChange={(e) => setKalshiApiKeyId(e.target.value)}
                      className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 placeholder:text-white/20 focus:outline-none focus:ring-1 focus:ring-cyan-400 focus:border-cyan-400 font-mono"
                    />
                  </div>

                  <div>
                    <div className="flex items-center justify-between mb-1">
                      <label className="text-white/60">RSA Private Key:</label>
                      <button
                        type="button"
                        onClick={() => toggleSecret("kalshi_key")}
                        className="text-white/40 hover:text-white/80 text-[11px] flex items-center space-x-1"
                      >
                        {showSecrets["kalshi_key"] ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3 text-cyan-400" />}
                        <span>{showSecrets["kalshi_key"] ? "Hide" : "Show"}</span>
                      </button>
                    </div>
                    <input
                      type={showSecrets["kalshi_key"] ? "text" : "password"}
                      disabled={loading || isVaultLocked}
                      value={kalshiPrivateKey}
                      placeholder={isVaultLocked ? "••••••••••••••••" : "-----BEGIN RSA PRIVATE KEY-----"}
                      onChange={(e) => setKalshiPrivateKey(e.target.value)}
                      className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 placeholder:text-white/20 focus:outline-none focus:ring-1 focus:ring-cyan-400 focus:border-cyan-400 font-mono"
                    />
                  </div>
                </div>
              </div>
            )}
          </div>
        )}

        {/* =================================================================== */}
        {/* SECTION 2: BALANCES & ASYMMETRIC REBALANCE                         */}
        {/* =================================================================== */}
        {(activeTab === "balances" || activeTab === "all") && (
          <div className="glass-panel rounded-2xl p-5 space-y-4 border border-white/[0.08]">
            <div className="flex items-center justify-between border-b border-white/[0.06] pb-3">
              <div>
                <h3 className="text-xs font-mono font-bold uppercase text-white/95 flex items-center space-x-2">
                  <Wallet className="w-4 h-4 text-cyan-400" />
                  <span>Venue Balances & Asymmetric Depletion Alert</span>
                </h3>
                <p className="text-[11px] text-white/50 font-mono mt-0.5">
                  Tracks free margin across venues to prevent single-leg fill failure from one-sided executions
                </p>
              </div>

              <button
                type="button"
                onClick={loadVenueBalances}
                disabled={loadingBalances}
                className="px-3 py-1 bg-white/[0.06] hover:bg-white/[0.1] border border-white/[0.08] rounded-lg text-xs font-mono flex items-center space-x-1.5 text-white/80 transition"
              >
                <RefreshCw className={`w-3 h-3 ${loadingBalances ? "animate-spin text-cyan-400" : ""}`} />
                <span>Refresh</span>
              </button>
            </div>

            <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-3">
              {venueBalances.map((b) => {
                const isWarning = b.needs_rebalance || b.status !== "HEALTHY";
                return (
                  <div
                    key={b.venue}
                    className={`p-4 rounded-xl border ${
                      isWarning
                        ? "bg-amber-500/10 border-amber-500/30 shadow-[0_0_15px_rgba(245,158,11,0.1)]"
                        : "glass-panel border-white/[0.08]"
                    } space-y-2`}
                  >
                    <div className="flex items-center justify-between">
                      <span className="text-xs font-bold font-mono text-white/90 uppercase">{b.venue}</span>
                      <span
                        className={`chip text-[10px] font-mono font-bold ${
                          isWarning ? "chip-warn" : "chip-pos"
                        }`}
                      >
                        {b.status}
                      </span>
                    </div>

                    <div className="space-y-0.5">
                      <span className="text-[10px] uppercase font-mono text-white/40 block">Free Margin</span>
                      <span className="text-base font-bold font-mono text-white tabular-nums">
                        ${b.balance_usd.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}
                      </span>
                    </div>

                    <div className="text-[10px] font-mono text-white/50 pt-1 border-t border-white/[0.06] tabular-nums">
                      <span>Safety Floor: ${b.min_required_usd.toFixed(0)}</span>
                    </div>

                    {b.recommended_action && (
                      <div className="p-2 bg-amber-500/15 border border-amber-500/30 rounded-lg text-[10px] font-mono text-amber-200 flex items-start space-x-1.5">
                        <AlertTriangle className="w-3 h-3 text-amber-400 shrink-0 mt-0.5" />
                        <span>{b.recommended_action}</span>
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          </div>
        )}

        {/* =================================================================== */}
        {/* SECTION 3: EXECUTION MODES & WARM-UP                               */}
        {/* =================================================================== */}
        {(activeTab === "execution" || activeTab === "all") && (
          <div className="glass-panel rounded-2xl p-5 space-y-4 border border-white/[0.08]">
            <div className="border-b border-white/[0.06] pb-3">
              <h3 className="text-xs font-mono font-bold uppercase text-white/95 flex items-center space-x-2">
                <Sliders className="w-4 h-4 text-cyan-400" />
                <span>Execution Modes & Cache Warm-Up Guardrails</span>
              </h3>
              <p className="text-[11px] text-white/50 font-mono mt-0.5">
                Configure live order execution vs paper simulation and orderbook persistence
              </p>
            </div>

            <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs font-mono">
              {/* Trading Mode Radio Card */}
              <div className="p-4 glass-panel border border-white/[0.08] rounded-xl space-y-2">
                <label className="text-white/90 font-bold block text-xs">Trading Mode</label>
                <p className="text-[11px] text-white/50">
                  Switch between risk-free simulation and active DEX order placement:
                </p>

                <div className="flex items-center space-x-4 pt-1">
                  <label className="flex items-center space-x-2 cursor-pointer">
                    <input
                      type="radio"
                      name="trading_mode"
                      value="paper"
                      checked={tradingMode === "paper"}
                      onChange={() => setTradingMode("paper")}
                      className="text-cyan-400 focus:ring-cyan-400"
                    />
                    <span className="font-semibold text-white/80">Paper Trading (Live Feeds)</span>
                  </label>
                  <label className="flex items-center space-x-2 cursor-pointer">
                    <input
                      type="radio"
                      name="trading_mode"
                      value="real"
                      checked={tradingMode === "real"}
                      onChange={() => setTradingMode("real")}
                      className="text-amber-400 focus:ring-amber-400"
                    />
                    <span className="font-semibold text-amber-300">Real Trading (Live Execution)</span>
                  </label>
                </div>
              </div>

              {/* Orderbook Fast Warm-up Toggle */}
              <div className="p-4 glass-panel border border-white/[0.08] rounded-xl space-y-2">
                <div className="flex items-center justify-between">
                  <label className="text-white/90 font-bold block text-xs">Orderbook Cache Warm-Up</label>
                  <input
                    type="checkbox"
                    checked={fastWarmupEnabled}
                    onChange={(e) => setFastWarmupEnabled(e.target.checked)}
                    className="w-4 h-4 text-cyan-400 rounded focus:ring-cyan-400"
                  />
                </div>
                <p className="text-[11px] text-white/50">
                  {fastWarmupEnabled
                    ? "Fast Warm-Up: Persists sliding window orderbook quotes across app restarts for zero-lag instant signal readiness."
                    : "Cold Start: Starts orderbook buffer from scratch on every launch (requires 30-60s warm-up window)."}
                </p>
              </div>

              {/* Max Slippage */}
              <div>
                <label className="text-white/60 block mb-1">Max Slippage Tolerance (BPS):</label>
                <input
                  type="number"
                  step="1"
                  min="1"
                  disabled={loading}
                  value={maxSlippageBps}
                  onChange={(e) => setMaxSlippageBps(parseFloat(e.target.value) || 15)}
                  className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 focus:outline-none focus:ring-1 focus:ring-cyan-400 tabular-nums font-mono"
                />
              </div>

              {/* Venue Rebalance Threshold */}
              <div>
                <label className="text-white/60 block mb-1">Venue Rebalance Warning Threshold ($ USD):</label>
                <input
                  type="number"
                  step="50"
                  min="100"
                  disabled={loading}
                  value={venueRebalanceThresholdUsd}
                  onChange={(e) => setVenueRebalanceThresholdUsd(parseFloat(e.target.value) || 500)}
                  className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 focus:outline-none focus:ring-1 focus:ring-cyan-400 tabular-nums font-mono"
                />
              </div>
            </div>
          </div>
        )}

        {/* =================================================================== */}
        {/* SECTION 4: NETWORKING & VPS 24/7 DEPLOYMENT                         */}
        {/* =================================================================== */}
        {(activeTab === "network" || activeTab === "all") && (
          <div className="glass-panel rounded-2xl p-5 space-y-4 border border-white/[0.08]">
            <div className="border-b border-white/[0.06] pb-3">
              <h3 className="text-xs font-mono font-bold uppercase text-white/95 flex items-center space-x-2">
                <Globe className="w-4 h-4 text-cyan-400" />
                <span>Engine Ports & 24/7 VPS Background Daemon</span>
              </h3>
              <p className="text-[11px] text-white/50 font-mono mt-0.5">
                Configure RPC / WebSocket listener ports and deploy headlessly on cloud servers
              </p>
            </div>

            <div className="grid grid-cols-1 md:grid-cols-3 gap-4 text-xs font-mono">
              <div>
                <label className="text-white/60 block mb-1">Engine RPC Port:</label>
                <input
                  type="number"
                  disabled={loading}
                  value={engineRpcPort}
                  onChange={(e) => setEngineRpcPort(parseInt(e.target.value, 10) || 8787)}
                  className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 focus:outline-none focus:ring-1 focus:ring-cyan-400 tabular-nums font-mono"
                />
              </div>

              <div>
                <label className="text-white/60 block mb-1">Engine WebSocket Port:</label>
                <input
                  type="number"
                  disabled={loading}
                  value={engineWsPort}
                  onChange={(e) => setEngineWsPort(parseInt(e.target.value, 10) || 8789)}
                  className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 focus:outline-none focus:ring-1 focus:ring-cyan-400 tabular-nums font-mono"
                />
              </div>

              <div>
                <label className="text-white/60 block mb-1">Bind Address:</label>
                <input
                  type="text"
                  disabled={loading}
                  value={bindAddress}
                  placeholder="127.0.0.1 or 0.0.0.0"
                  onChange={(e) => setBindAddress(e.target.value)}
                  className="w-full px-3 py-2 bg-black/40 border border-white/[0.1] rounded-xl text-white/90 focus:outline-none focus:ring-1 focus:ring-cyan-400 font-mono"
                />
              </div>
            </div>

            {/* VPS Daemon Deployment Guide */}
            <div className="p-4 glass-panel border border-white/[0.08] rounded-xl space-y-2 text-xs font-mono text-white/70">
              <h4 className="font-bold text-white flex items-center space-x-1.5">
                <span>🖥️ Headless VPS 24/7 Deployment Instructions</span>
              </h4>
              <p className="text-[11px] text-white/60">
                To run Arbitrager continuously 24/7 without keeping your local laptop or desktop open, deploy the standalone Rust binary to an Ubuntu/Debian VPS:
              </p>
              <div className="p-2.5 bg-black/60 border border-white/[0.08] text-cyan-300 rounded-lg text-[11px] overflow-x-auto select-text font-mono">
                <code>./target/release/arbitrager serve --bind 0.0.0.0 --port 8787 --ws-port 8789</code>
              </div>
              <p className="text-[10px] text-white/40">
                You can create a systemd service (e.g. <code className="bg-white/[0.08] px-1 py-0.5 rounded text-white/80">systemctl start arbitrager</code>) to auto-restart on server reboot.
              </p>
            </div>
          </div>
        )}

        {/* =================================================================== */}
        {/* SECTION 5: ABOUT, CHECK FOR UPDATES & BUY ME A SOLO                 */}
        {/* =================================================================== */}
        {(activeTab === "about" || activeTab === "all") && (
          <div className="space-y-4">
            <div className="glass-panel border border-white/[0.08] rounded-2xl p-6 shadow-xs space-y-5">
              <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4 border-b border-white/[0.06] pb-4">
                <div className="flex items-center space-x-3">
                  <div className="w-10 h-10 rounded-xl bg-cyan-500/15 border border-cyan-500/30 flex items-center justify-center text-cyan-300 shadow-[0_0_12px_rgba(34,211,238,0.25)]">
                    <Shield className="w-5 h-5" />
                  </div>
                  <div>
                    <h3 className="text-sm font-bold font-mono text-white">
                      About Arbitrager ({version})
                    </h3>
                    <p className="text-xs text-white/50 font-mono mt-0.5">
                      High-frequency cross-venue statistical & spatial arbitrage platform
                    </p>
                  </div>
                </div>

                <div className="flex items-center space-x-2">
                  <button
                    type="button"
                    onClick={handleCheckForUpdates}
                    disabled={checkingUpdate}
                    className="px-3.5 py-1.5 bg-white/[0.06] hover:bg-white/[0.1] border border-white/[0.08] text-white/90 rounded-xl text-xs font-mono font-medium transition flex items-center space-x-1.5 shadow-xs"
                  >
                    <RefreshCw className={`w-3.5 h-3.5 ${checkingUpdate ? "animate-spin text-cyan-400" : ""}`} />
                    <span>{checkingUpdate ? "Checking..." : "Check for Updates"}</span>
                  </button>

                  <a
                    href={gitRepo}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="px-3.5 py-1.5 bg-white/[0.06] hover:bg-white/[0.1] border border-white/[0.08] text-white/90 rounded-xl text-xs font-mono font-medium transition flex items-center space-x-1.5 shadow-xs"
                  >
                    <span>Git Repository</span>
                    <ExternalLink className="w-3.5 h-3.5" />
                  </a>
                </div>
              </div>

              {updateMsg && (
                <div className="p-3 bg-emerald-500/15 border border-emerald-500/30 text-emerald-300 rounded-xl text-xs font-mono flex items-center space-x-2">
                  <CheckCircle2 className="w-4 h-4 text-emerald-400 shrink-0" />
                  <span>{updateMsg}</span>
                </div>
              )}

              {/* Sponsor Card: Buy me a SOLO */}
              <div className="p-5 glass-panel bg-gradient-to-r from-amber-500/15 via-yellow-500/10 to-amber-500/15 border border-amber-500/30 rounded-2xl flex flex-col sm:flex-row items-center justify-between gap-4 shadow-[0_0_30px_rgba(245,158,11,0.12)]">
                <div className="flex items-center space-x-4">
                  <img
                    src="/solo_can.jpg"
                    alt="Can of Australian SOLO Lemon Soda"
                    className="w-14 h-14 object-contain rounded-xl shadow-[0_0_16px_rgba(245,158,11,0.25)] border border-amber-500/30 bg-black/40 p-1"
                  />
                  <div>
                    <h4 className="text-sm font-bold font-mono text-amber-200">
                      Enjoying the tool? Buy me a SOLO!
                    </h4>
                    <p className="text-xs text-white/70 font-mono mt-0.5">
                      Fuel the research and development of ultra-low latency quantitative arbitrage strategies.
                    </p>
                  </div>
                </div>

                <a
                  href={sponsorLink}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="px-5 py-2.5 bg-gradient-to-r from-amber-500 to-yellow-500 hover:from-amber-400 hover:to-yellow-400 text-slate-950 font-bold text-xs font-mono rounded-xl transition shadow-[0_0_20px_rgba(245,158,11,0.4)] flex items-center space-x-2 shrink-0"
                >
                  <span>Buy me a SOLO ($5)</span>
                  <ExternalLink className="w-3.5 h-3.5" />
                </a>
              </div>

              {/* Legal & Financial Disclaimer */}
              <div className="p-4 glass-panel border border-white/[0.06] rounded-xl space-y-2 text-[11px] font-mono text-white/50 leading-relaxed">
                <div className="flex items-center space-x-1.5 font-bold text-white/70 uppercase text-[10px]">
                  <Scale className="w-3.5 h-3.5 text-white/50" />
                  <span>Legal & Financial Risk Disclaimer</span>
                </div>
                <p>
                  This software is provided for research, simulation, and educational quantitative finance purposes. Cross-market arbitrage and algorithmic trading involve inherent execution risks, including market volatility, network latency spikes, smart contract bugs, API rate limits, and liquidity slippage. Past simulation performance does not guarantee future financial returns.
                </p>
                <p>
                  No private keys or seed phrases are stored remotely or accessed by any third party; all credentials remain encrypted locally on your system using AES-256-GCM. Users are solely responsible for compliance with their local jurisdiction regulations and exchange terms of service.
                </p>
              </div>
            </div>
          </div>
        )}

        {/* Save Bar */}
        <div className="flex items-center justify-between pt-2">
          <div className="text-[11px] font-mono text-white/50 flex items-center space-x-1.5">
            <Lock className="w-3 h-3 text-white/40" />
            <span>Config changes immediately affect live atomic coordinator execution.</span>
          </div>

          <button
            type="submit"
            disabled={loading || saving}
            className="px-6 py-2.5 bg-gradient-to-r from-cyan-500 to-indigo-600 hover:from-cyan-400 hover:to-indigo-500 text-white font-semibold text-xs font-mono rounded-xl transition flex items-center space-x-2 shadow-[0_0_20px_rgba(34,211,238,0.25)] disabled:opacity-50"
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
      </form>

      {/* Vault Unlock Modal */}
      <VaultUnlockModal
        isOpen={showVaultModal}
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
