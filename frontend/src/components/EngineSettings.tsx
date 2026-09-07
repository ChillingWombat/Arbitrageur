import React, { useState, useEffect } from "react";
import { AlertCircle, CheckCircle2, Eye, EyeOff, Key, Lock, Server, Sliders, Shield } from "lucide-react";
import { api } from "../services/api";
import { EngineSettingsDto } from "../types";

type SettingsTab = "all" | "exchanges" | "execution" | "hsm";
type ExchangeFilter = "all" | "hyperliquid" | "lighter" | "vanta" | "robinhood" | "polymarket" | "kalshi";

export const EngineSettings: React.FC = () => {
  // Remote HSM
  const [hsmUrl, setHsmUrl] = useState("http://127.0.0.1:8787/sign");
  const [hmacToken, setHmacToken] = useState("");

  // Guardrails
  const [maxSlippageBps, setMaxSlippageBps] = useState(15.0);
  const [minDepthRatio, setMinDepthRatio] = useState(3.0);

  // Hyperliquid
  const [hyperliquidWallet, setHyperliquidWallet] = useState("");
  const [hyperliquidPrivateKey, setHyperliquidPrivateKey] = useState("");
  const [hyperliquidUrl, setHyperliquidUrl] = useState("https://api.hyperliquid.xyz");

  // Lighter
  const [lighterAccountIndex, setLighterAccountIndex] = useState("0");
  const [lighterApiKey, setLighterApiKey] = useState("");
  const [lighterUrl, setLighterUrl] = useState("https://mainnet.zklighter.elliot.ai");

  // Vanta (Orderly)
  const [vantaAccountId, setVantaAccountId] = useState("");
  const [vantaOrderlyKey, setVantaOrderlyKey] = useState("");
  const [vantaOrderlySecret, setVantaOrderlySecret] = useState("");
  const [vantaUrl, setVantaUrl] = useState("https://api-evm.orderly.org");

  // Robinhood EVM
  const [robinhoodWallet, setRobinhoodWallet] = useState("");
  const [robinhoodPrivateKey, setRobinhoodPrivateKey] = useState("");
  const [robinhoodRpcUrl, setRobinhoodRpcUrl] = useState("https://rpc.robinhood.network");
  const [robinhoodRouter, setRobinhoodRouter] = useState("0x89e5DB8B5aA49aA85AC63f691524311AEB649eba");

  // Polymarket
  const [polymarketApiKey, setPolymarketApiKey] = useState("");
  const [polymarketSecret, setPolymarketSecret] = useState("");
  const [polymarketPassphrase, setPolymarketPassphrase] = useState("");
  const [polymarketUrl, setPolymarketUrl] = useState("https://clob.polymarket.com");

  // Kalshi
  const [kalshiApiKeyId, setKalshiApiKeyId] = useState("");
  const [kalshiPrivateKey, setKalshiPrivateKey] = useState("");
  const [kalshiUrl, setKalshiUrl] = useState("https://trading-api.kalshi.com");

  // UI state
  const [activeTab, setActiveTab] = useState<SettingsTab>("exchanges");
  const [venueFilter, setVenueFilter] = useState<ExchangeFilter>("all");
  const [showSecrets, setShowSecrets] = useState<Record<string, boolean>>({});
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const toggleSecret = (field: string) => {
    setShowSecrets((prev) => ({ ...prev, [field]: !prev[field] }));
  };

  useEffect(() => {
    const loadSettings = async () => {
      try {
        const s: EngineSettingsDto = await api.getEngineSettings();
        setHsmUrl(s.hsm_url || "http://127.0.0.1:8787/sign");
        setHmacToken(s.hmac_token || "");
        setMaxSlippageBps(s.max_slippage_bps ?? 15.0);
        setMinDepthRatio(s.min_depth_ratio ?? 3.0);

        setHyperliquidWallet(s.hyperliquid_wallet || "");
        setHyperliquidPrivateKey(s.hyperliquid_private_key || "");
        setHyperliquidUrl(s.hyperliquid_url || "https://api.hyperliquid.xyz");

        setLighterAccountIndex(s.lighter_account_index || "0");
        setLighterApiKey(s.lighter_api_key || "");
        setLighterUrl(s.lighter_url || "https://mainnet.zklighter.elliot.ai");

        setVantaAccountId(s.vanta_account_id || "");
        setVantaOrderlyKey(s.vanta_orderly_key || "");
        setVantaOrderlySecret(s.vanta_orderly_secret || "");
        setVantaUrl(s.vanta_url || "https://api-evm.orderly.org");

        setRobinhoodWallet(s.robinhood_wallet || "");
        setRobinhoodPrivateKey(s.robinhood_private_key || "");
        setRobinhoodRpcUrl(s.robinhood_rpc_url || "https://rpc.robinhood.network");
        setRobinhoodRouter(s.robinhood_router || "0x89e5DB8B5aA49aA85AC63f691524311AEB649eba");

        setPolymarketApiKey(s.polymarket_api_key || "");
        setPolymarketSecret(s.polymarket_secret || "");
        setPolymarketPassphrase(s.polymarket_passphrase || "");
        setPolymarketUrl(s.polymarket_url || "https://clob.polymarket.com");

        setKalshiApiKeyId(s.kalshi_api_key_id || "");
        setKalshiPrivateKey(s.kalshi_private_key || "");
        setKalshiUrl(s.kalshi_url || "https://trading-api.kalshi.com");
      } catch (err) {
        console.error("Failed to load engine settings:", err);
      } finally {
        setLoading(false);
      }
    };
    loadSettings();
  }, []);

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
        hyperliquid_wallet: hyperliquidWallet.trim(),
        hyperliquid_private_key: hyperliquidPrivateKey.trim(),
        hyperliquid_url: hyperliquidUrl.trim(),
        lighter_account_index: lighterAccountIndex.trim(),
        lighter_api_key: lighterApiKey.trim(),
        lighter_url: lighterUrl.trim(),
        vanta_account_id: vantaAccountId.trim(),
        vanta_orderly_key: vantaOrderlyKey.trim(),
        vanta_orderly_secret: vantaOrderlySecret.trim(),
        vanta_url: vantaUrl.trim(),
        robinhood_wallet: robinhoodWallet.trim(),
        robinhood_private_key: robinhoodPrivateKey.trim(),
        robinhood_rpc_url: robinhoodRpcUrl.trim(),
        robinhood_router: robinhoodRouter.trim(),
        polymarket_api_key: polymarketApiKey.trim(),
        polymarket_secret: polymarketSecret.trim(),
        polymarket_passphrase: polymarketPassphrase.trim(),
        polymarket_url: polymarketUrl.trim(),
        kalshi_api_key_id: kalshiApiKeyId.trim(),
        kalshi_private_key: kalshiPrivateKey.trim(),
        kalshi_url: kalshiUrl.trim(),
      });
      setSaved(true);
      setTimeout(() => setSaved(false), 3000);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      setErrorMsg(msg);
    } finally {
      setSaving(false);
    }
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

  return (
    <div className="p-6 space-y-6 overflow-y-auto h-full max-w-5xl">
      {/* Top Banner */}
      <div className="bg-surface p-5 rounded-xl border border-surface-border shadow-xs flex flex-col md:flex-row md:items-center justify-between gap-4">
        <div>
          <div className="flex items-center space-x-2">
            <h2 className="text-sm font-bold font-mono text-slate-900">
              Arbitrager Engine Settings & API Keys
            </h2>
            <span className="text-[10px] font-mono px-2 py-0.5 rounded-full bg-blue-50 text-blue-700 border border-blue-200">
              {configuredCount} / 6 Venues Configured
            </span>
          </div>
          <p className="text-xs text-slate-500 mt-1">
            Configure exchange API keys, order routing thresholds, and remote HSM signing endpoints.
          </p>
        </div>

        {/* Section Tabs */}
        <div className="flex items-center bg-slate-100 p-1 rounded-lg text-xs font-mono border border-slate-200 self-start md:self-auto">
          <button
            type="button"
            onClick={() => setActiveTab("exchanges")}
            className={`px-3 py-1.5 rounded-md transition flex items-center space-x-1.5 ${
              activeTab === "exchanges"
                ? "bg-white text-blue-600 font-semibold shadow-xs"
                : "text-slate-600 hover:text-slate-900"
            }`}
          >
            <Key className="w-3.5 h-3.5" />
            <span>Exchange APIs</span>
          </button>
          <button
            type="button"
            onClick={() => setActiveTab("execution")}
            className={`px-3 py-1.5 rounded-md transition flex items-center space-x-1.5 ${
              activeTab === "execution"
                ? "bg-white text-blue-600 font-semibold shadow-xs"
                : "text-slate-600 hover:text-slate-900"
            }`}
          >
            <Sliders className="w-3.5 h-3.5" />
            <span>Guardrails</span>
          </button>
          <button
            type="button"
            onClick={() => setActiveTab("hsm")}
            className={`px-3 py-1.5 rounded-md transition flex items-center space-x-1.5 ${
              activeTab === "hsm"
                ? "bg-white text-blue-600 font-semibold shadow-xs"
                : "text-slate-600 hover:text-slate-900"
            }`}
          >
            <Server className="w-3.5 h-3.5" />
            <span>Remote HSM</span>
          </button>
          <button
            type="button"
            onClick={() => setActiveTab("all")}
            className={`px-3 py-1.5 rounded-md transition ${
              activeTab === "all"
                ? "bg-white text-blue-600 font-semibold shadow-xs"
                : "text-slate-600 hover:text-slate-900"
            }`}
          >
            All
          </button>
        </div>
      </div>

      {errorMsg && (
        <div className="bg-loss-dim border border-loss-border text-loss px-4 py-3 rounded-xl text-xs font-mono flex items-center space-x-2">
          <AlertCircle className="w-4 h-4 flex-shrink-0" />
          <span>{errorMsg}</span>
        </div>
      )}

      {saved && (
        <div className="bg-emerald-50 border border-emerald-200 text-emerald-700 px-4 py-3 rounded-xl text-xs font-mono flex items-center space-x-2">
          <CheckCircle2 className="w-4 h-4 flex-shrink-0" />
          <span>Engine configuration successfully applied and persistent in runtime.</span>
        </div>
      )}

      <form onSubmit={handleSave} className="space-y-6">
        {/* =================================================================== */}
        {/* SECTION 1: EXCHANGE API CREDENTIALS                                */}
        {/* =================================================================== */}
        {(activeTab === "exchanges" || activeTab === "all") && (
          <div className="space-y-4">
            {/* Header with Sub-filters */}
            <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-2 pb-1 border-b border-surface-border">
              <div>
                <h3 className="text-xs font-mono font-semibold uppercase tracking-wide text-slate-900 flex items-center space-x-1.5">
                  <Key className="w-3.5 h-3.5 text-blue-600" />
                  <span>Exchange API Credentials & Private Keys</span>
                </h3>
                <p className="text-[11px] text-slate-500 mt-0.5">
                  Connect real trading accounts for atomic spatial arbitrage and funding rate capture.
                </p>
              </div>

              {/* Venue filter pills */}
              <div className="flex flex-wrap items-center gap-1 text-[11px] font-mono">
                {(["all", "hyperliquid", "lighter", "vanta", "robinhood", "polymarket", "kalshi"] as const).map(
                  (v) => (
                    <button
                      key={v}
                      type="button"
                      onClick={() => setVenueFilter(v)}
                      className={`px-2 py-0.5 rounded border transition ${
                        venueFilter === v
                          ? "bg-blue-50 border-blue-300 text-blue-700 font-medium"
                          : "bg-white border-slate-200 text-slate-600 hover:bg-slate-50"
                      }`}
                    >
                      {v === "all" ? "All Venues" : v.charAt(0).toUpperCase() + v.slice(1)}
                    </button>
                  )
                )}
              </div>
            </div>

            {/* Grid of Exchange Cards */}
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              {/* 1. HYPERLIQUID */}
              {(venueFilter === "all" || venueFilter === "hyperliquid") && (
                <div className="bg-surface border border-surface-border rounded-xl p-4 space-y-3 shadow-xs">
                  <div className="flex items-center justify-between border-b border-slate-100 pb-2">
                    <div className="flex items-center space-x-2">
                      <span className="w-2 h-2 rounded-full bg-blue-600"></span>
                      <h4 className="text-xs font-bold font-mono text-slate-900">Hyperliquid</h4>
                      <span className="text-[10px] font-mono px-1.5 py-0.2 rounded bg-slate-100 text-slate-600 border border-slate-200">
                        L1 DEX
                      </span>
                    </div>
                    <span
                      className={`text-[10px] font-mono px-2 py-0.5 rounded-full border ${
                        isVenueConfigured("hyperliquid")
                          ? "bg-emerald-50 border-emerald-200 text-emerald-700"
                          : "bg-slate-50 border-slate-200 text-slate-500"
                      }`}
                    >
                      {isVenueConfigured("hyperliquid") ? "Configured" : "Unset"}
                    </span>
                  </div>

                  <div className="space-y-2.5 text-xs font-mono">
                    <div>
                      <label className="text-slate-600 block mb-1">Main Wallet Address (0x...):</label>
                      <input
                        type="text"
                        disabled={loading}
                        value={hyperliquidWallet}
                        placeholder="0x0000000000000000000000000000000000000000"
                        onChange={(e) => setHyperliquidWallet(e.target.value)}
                        className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-xs"
                      />
                    </div>

                    <div>
                      <div className="flex items-center justify-between mb-1">
                        <label className="text-slate-600">API Agent Secret / Private Key:</label>
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
                        disabled={loading}
                        value={hyperliquidPrivateKey}
                        placeholder="0x..."
                        onChange={(e) => setHyperliquidPrivateKey(e.target.value)}
                        className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-xs font-mono"
                      />
                    </div>

                    <div>
                      <label className="text-slate-600 block mb-1">API Endpoint URL:</label>
                      <input
                        type="text"
                        disabled={loading}
                        value={hyperliquidUrl}
                        onChange={(e) => setHyperliquidUrl(e.target.value)}
                        className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-[11px] text-slate-500"
                      />
                    </div>
                  </div>
                </div>
              )}

              {/* 2. LIGHTER */}
              {(venueFilter === "all" || venueFilter === "lighter") && (
                <div className="bg-surface border border-surface-border rounded-xl p-4 space-y-3 shadow-xs">
                  <div className="flex items-center justify-between border-b border-slate-100 pb-2">
                    <div className="flex items-center space-x-2">
                      <span className="w-2 h-2 rounded-full bg-indigo-600"></span>
                      <h4 className="text-xs font-bold font-mono text-slate-900">Lighter</h4>
                      <span className="text-[10px] font-mono px-1.5 py-0.2 rounded bg-slate-100 text-slate-600 border border-slate-200">
                        zkDEX Rollup
                      </span>
                    </div>
                    <span
                      className={`text-[10px] font-mono px-2 py-0.5 rounded-full border ${
                        isVenueConfigured("lighter")
                          ? "bg-emerald-50 border-emerald-200 text-emerald-700"
                          : "bg-slate-50 border-slate-200 text-slate-500"
                      }`}
                    >
                      {isVenueConfigured("lighter") ? "Configured" : "Unset"}
                    </span>
                  </div>

                  <div className="space-y-2.5 text-xs font-mono">
                    <div>
                      <label className="text-slate-600 block mb-1">Account Index (integer):</label>
                      <input
                        type="text"
                        disabled={loading}
                        value={lighterAccountIndex}
                        placeholder="0"
                        onChange={(e) => setLighterAccountIndex(e.target.value)}
                        className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-xs"
                      />
                    </div>

                    <div>
                      <div className="flex items-center justify-between mb-1">
                        <label className="text-slate-600">Stark Private Key / API Key:</label>
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
                        disabled={loading}
                        value={lighterApiKey}
                        placeholder="Enter Stark private key"
                        onChange={(e) => setLighterApiKey(e.target.value)}
                        className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-xs font-mono"
                      />
                    </div>

                    <div>
                      <label className="text-slate-600 block mb-1">Base Endpoint URL:</label>
                      <input
                        type="text"
                        disabled={loading}
                        value={lighterUrl}
                        onChange={(e) => setLighterUrl(e.target.value)}
                        className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-[11px] text-slate-500"
                      />
                    </div>
                  </div>
                </div>
              )}

              {/* 3. VANTA.EXCHANGE (ORDERLY NETWORK) */}
              {(venueFilter === "all" || venueFilter === "vanta") && (
                <div className="bg-surface border border-surface-border rounded-xl p-4 space-y-3 shadow-xs">
                  <div className="flex items-center justify-between border-b border-slate-100 pb-2">
                    <div className="flex items-center space-x-2">
                      <span className="w-2 h-2 rounded-full bg-cyan-600"></span>
                      <h4 className="text-xs font-bold font-mono text-slate-900">Vanta.exchange</h4>
                      <span className="text-[10px] font-mono px-1.5 py-0.2 rounded bg-slate-100 text-slate-600 border border-slate-200">
                        Orderly EVM
                      </span>
                    </div>
                    <span
                      className={`text-[10px] font-mono px-2 py-0.5 rounded-full border ${
                        isVenueConfigured("vanta")
                          ? "bg-emerald-50 border-emerald-200 text-emerald-700"
                          : "bg-slate-50 border-slate-200 text-slate-500"
                      }`}
                    >
                      {isVenueConfigured("vanta") ? "Configured" : "Unset"}
                    </span>
                  </div>

                  <div className="space-y-2.5 text-xs font-mono">
                    <div className="grid grid-cols-2 gap-2">
                      <div>
                        <label className="text-slate-600 block mb-1">Account ID:</label>
                        <input
                          type="text"
                          disabled={loading}
                          value={vantaAccountId}
                          placeholder="vanta_account..."
                          onChange={(e) => setVantaAccountId(e.target.value)}
                          className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-xs"
                        />
                      </div>
                      <div>
                        <label className="text-slate-600 block mb-1">Orderly API Key:</label>
                        <input
                          type="text"
                          disabled={loading}
                          value={vantaOrderlyKey}
                          placeholder="ed25519:..."
                          onChange={(e) => setVantaOrderlyKey(e.target.value)}
                          className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-xs"
                        />
                      </div>
                    </div>

                    <div>
                      <div className="flex items-center justify-between mb-1">
                        <label className="text-slate-600">Orderly API Secret:</label>
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
                        disabled={loading}
                        value={vantaOrderlySecret}
                        placeholder="Enter Orderly private secret"
                        onChange={(e) => setVantaOrderlySecret(e.target.value)}
                        className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-xs font-mono"
                      />
                    </div>

                    <div>
                      <label className="text-slate-600 block mb-1">REST Base URL:</label>
                      <input
                        type="text"
                        disabled={loading}
                        value={vantaUrl}
                        onChange={(e) => setVantaUrl(e.target.value)}
                        className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-[11px] text-slate-500"
                      />
                    </div>
                  </div>
                </div>
              )}

              {/* 4. ROBINHOOD EVM */}
              {(venueFilter === "all" || venueFilter === "robinhood") && (
                <div className="bg-surface border border-surface-border rounded-xl p-4 space-y-3 shadow-xs">
                  <div className="flex items-center justify-between border-b border-slate-100 pb-2">
                    <div className="flex items-center space-x-2">
                      <span className="w-2 h-2 rounded-full bg-emerald-600"></span>
                      <h4 className="text-xs font-bold font-mono text-slate-900">Robinhood EVM</h4>
                      <span className="text-[10px] font-mono px-1.5 py-0.2 rounded bg-slate-100 text-slate-600 border border-slate-200">
                        Router & Bridge
                      </span>
                    </div>
                    <span
                      className={`text-[10px] font-mono px-2 py-0.5 rounded-full border ${
                        isVenueConfigured("robinhood")
                          ? "bg-emerald-50 border-emerald-200 text-emerald-700"
                          : "bg-slate-50 border-slate-200 text-slate-500"
                      }`}
                    >
                      {isVenueConfigured("robinhood") ? "Configured" : "Unset"}
                    </span>
                  </div>

                  <div className="space-y-2.5 text-xs font-mono">
                    <div>
                      <label className="text-slate-600 block mb-1">EVM Wallet Address:</label>
                      <input
                        type="text"
                        disabled={loading}
                        value={robinhoodWallet}
                        placeholder="0x..."
                        onChange={(e) => setRobinhoodWallet(e.target.value)}
                        className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-xs"
                      />
                    </div>

                    <div>
                      <div className="flex items-center justify-between mb-1">
                        <label className="text-slate-600">Wallet Private Key:</label>
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
                        disabled={loading}
                        value={robinhoodPrivateKey}
                        placeholder="0x..."
                        onChange={(e) => setRobinhoodPrivateKey(e.target.value)}
                        className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-xs font-mono"
                      />
                    </div>

                    <div className="grid grid-cols-2 gap-2">
                      <div>
                        <label className="text-slate-600 block mb-1">RPC URL:</label>
                        <input
                          type="text"
                          disabled={loading}
                          value={robinhoodRpcUrl}
                          onChange={(e) => setRobinhoodRpcUrl(e.target.value)}
                          className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-[11px] text-slate-500"
                        />
                      </div>
                      <div>
                        <label className="text-slate-600 block mb-1">Router Contract:</label>
                        <input
                          type="text"
                          disabled={loading}
                          value={robinhoodRouter}
                          onChange={(e) => setRobinhoodRouter(e.target.value)}
                          className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-[11px] text-slate-500 font-mono"
                        />
                      </div>
                    </div>
                  </div>
                </div>
              )}

              {/* 5. POLYMARKET */}
              {(venueFilter === "all" || venueFilter === "polymarket") && (
                <div className="bg-surface border border-surface-border rounded-xl p-4 space-y-3 shadow-xs">
                  <div className="flex items-center justify-between border-b border-slate-100 pb-2">
                    <div className="flex items-center space-x-2">
                      <span className="w-2 h-2 rounded-full bg-purple-600"></span>
                      <h4 className="text-xs font-bold font-mono text-slate-900">Polymarket</h4>
                      <span className="text-[10px] font-mono px-1.5 py-0.2 rounded bg-slate-100 text-slate-600 border border-slate-200">
                        CTF CLOB
                      </span>
                    </div>
                    <span
                      className={`text-[10px] font-mono px-2 py-0.5 rounded-full border ${
                        isVenueConfigured("polymarket")
                          ? "bg-emerald-50 border-emerald-200 text-emerald-700"
                          : "bg-slate-50 border-slate-200 text-slate-500"
                      }`}
                    >
                      {isVenueConfigured("polymarket") ? "Configured" : "Unset"}
                    </span>
                  </div>

                  <div className="space-y-2.5 text-xs font-mono">
                    <div>
                      <label className="text-slate-600 block mb-1">CLOB API Key:</label>
                      <input
                        type="text"
                        disabled={loading}
                        value={polymarketApiKey}
                        placeholder="Enter Polymarket CLOB API Key"
                        onChange={(e) => setPolymarketApiKey(e.target.value)}
                        className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-xs"
                      />
                    </div>

                    <div className="grid grid-cols-2 gap-2">
                      <div>
                        <div className="flex items-center justify-between mb-1">
                          <label className="text-slate-600">API Secret:</label>
                          <button
                            type="button"
                            onClick={() => toggleSecret("poly_secret")}
                            className="text-slate-400 hover:text-slate-600 text-[11px]"
                          >
                            {showSecrets["poly_secret"] ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3 text-blue-600" />}
                          </button>
                        </div>
                        <input
                          type={showSecrets["poly_secret"] ? "text" : "password"}
                          disabled={loading}
                          value={polymarketSecret}
                          placeholder="Secret"
                          onChange={(e) => setPolymarketSecret(e.target.value)}
                          className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-xs font-mono"
                        />
                      </div>

                      <div>
                        <div className="flex items-center justify-between mb-1">
                          <label className="text-slate-600">Passphrase:</label>
                          <button
                            type="button"
                            onClick={() => toggleSecret("poly_pass")}
                            className="text-slate-400 hover:text-slate-600 text-[11px]"
                          >
                            {showSecrets["poly_pass"] ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3 text-blue-600" />}
                          </button>
                        </div>
                        <input
                          type={showSecrets["poly_pass"] ? "text" : "password"}
                          disabled={loading}
                          value={polymarketPassphrase}
                          placeholder="Passphrase"
                          onChange={(e) => setPolymarketPassphrase(e.target.value)}
                          className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-xs font-mono"
                        />
                      </div>
                    </div>

                    <div>
                      <label className="text-slate-600 block mb-1">CLOB Base URL:</label>
                      <input
                        type="text"
                        disabled={loading}
                        value={polymarketUrl}
                        onChange={(e) => setPolymarketUrl(e.target.value)}
                        className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-[11px] text-slate-500"
                      />
                    </div>
                  </div>
                </div>
              )}

              {/* 6. KALSHI */}
              {(venueFilter === "all" || venueFilter === "kalshi") && (
                <div className="bg-surface border border-surface-border rounded-xl p-4 space-y-3 shadow-xs">
                  <div className="flex items-center justify-between border-b border-slate-100 pb-2">
                    <div className="flex items-center space-x-2">
                      <span className="w-2 h-2 rounded-full bg-amber-600"></span>
                      <h4 className="text-xs font-bold font-mono text-slate-900">Kalshi</h4>
                      <span className="text-[10px] font-mono px-1.5 py-0.2 rounded bg-slate-100 text-slate-600 border border-slate-200">
                        CFTC Binary
                      </span>
                    </div>
                    <span
                      className={`text-[10px] font-mono px-2 py-0.5 rounded-full border ${
                        isVenueConfigured("kalshi")
                          ? "bg-emerald-50 border-emerald-200 text-emerald-700"
                          : "bg-slate-50 border-slate-200 text-slate-500"
                      }`}
                    >
                      {isVenueConfigured("kalshi") ? "Configured" : "Unset"}
                    </span>
                  </div>

                  <div className="space-y-2.5 text-xs font-mono">
                    <div>
                      <label className="text-slate-600 block mb-1">API Key ID:</label>
                      <input
                        type="text"
                        disabled={loading}
                        value={kalshiApiKeyId}
                        placeholder="Enter Kalshi API Key ID"
                        onChange={(e) => setKalshiApiKeyId(e.target.value)}
                        className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-xs"
                      />
                    </div>

                    <div>
                      <div className="flex items-center justify-between mb-1">
                        <label className="text-slate-600">RSA Private Key / Secret:</label>
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
                        disabled={loading}
                        value={kalshiPrivateKey}
                        placeholder="Enter RSA private key or secret token"
                        onChange={(e) => setKalshiPrivateKey(e.target.value)}
                        className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-xs font-mono"
                      />
                    </div>

                    <div>
                      <label className="text-slate-600 block mb-1">Trading API URL:</label>
                      <input
                        type="text"
                        disabled={loading}
                        value={kalshiUrl}
                        onChange={(e) => setKalshiUrl(e.target.value)}
                        className="w-full px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent text-[11px] text-slate-500"
                      />
                    </div>
                  </div>
                </div>
              )}
            </div>

            {/* Security Notice */}
            <div className="p-3.5 bg-blue-50/50 border border-blue-100 rounded-xl text-xs font-mono text-slate-600 flex items-start space-x-2.5">
              <Shield className="w-4 h-4 text-blue-600 flex-shrink-0 mt-0.5" />
              <div>
                <span className="font-semibold text-slate-900">Zero-Key Local Leakage Architecture:</span>
                <p className="mt-0.5 text-[11px] text-slate-600 leading-relaxed">
                  Exchange credentials and private signing keys are held strictly in memory in the native Rust engine process or delegated to your designated remote HSM signer. Keys are never saved unencrypted to disk or sent to external telemetry servers.
                </p>
              </div>
            </div>
          </div>
        )}

        {/* =================================================================== */}
        {/* SECTION 2: EXECUTION GUARDRAILS & ROUTING                          */}
        {/* =================================================================== */}
        {(activeTab === "execution" || activeTab === "all") && (
          <div className="bg-surface border border-surface-border rounded-xl p-5 space-y-4 shadow-xs">
            <div className="border-b border-surface-border pb-3">
              <h3 className="text-xs font-mono font-semibold uppercase text-slate-900 flex items-center space-x-1.5">
                <Sliders className="w-3.5 h-3.5 text-blue-600" />
                <span>Smart Order Routing & Execution Guardrails</span>
              </h3>
              <p className="text-[11px] text-slate-500 mt-0.5">
                Protect atomic execution against market depth decay, adverse selection, and slippage.
              </p>
            </div>

            <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs font-mono">
              <div>
                <label className="text-slate-600 block mb-1">Maximum Slippage Tolerance (BPS):</label>
                <input
                  type="number"
                  step="1"
                  min="0.1"
                  max="500"
                  disabled={loading}
                  value={maxSlippageBps}
                  onChange={(e) => setMaxSlippageBps(parseFloat(e.target.value) || 15)}
                  className="w-full px-3 py-2 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
                />
                <span className="text-[11px] text-slate-400 mt-1 block">
                  Recommended: 10 - 25 BPS for perp DEXs
                </span>
              </div>

              <div>
                <label className="text-slate-600 block mb-1">Min Available Depth Ratio (Depth / Size):</label>
                <input
                  type="number"
                  step="0.5"
                  min="1.0"
                  disabled={loading}
                  value={minDepthRatio}
                  onChange={(e) => setMinDepthRatio(parseFloat(e.target.value) || 3)}
                  className="w-full px-3 py-2 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
                />
                <span className="text-[11px] text-slate-400 mt-1 block">
                  Ensures top-of-book depth is at least N× target order size
                </span>
              </div>
            </div>
          </div>
        )}

        {/* =================================================================== */}
        {/* SECTION 3: REMOTE HSM SIGNER                                       */}
        {/* =================================================================== */}
        {(activeTab === "hsm" || activeTab === "all") && (
          <div className="bg-surface border border-surface-border rounded-xl p-5 space-y-4 shadow-xs">
            <div className="border-b border-surface-border pb-3">
              <h3 className="text-xs font-mono font-semibold uppercase text-slate-900 flex items-center space-x-1.5">
                <Server className="w-3.5 h-3.5 text-blue-600" />
                <span>Remote HSM & Key Management (Zero-Key Local Storage)</span>
              </h3>
              <p className="text-[11px] text-slate-500 mt-0.5">
                Optional Cloudflare Worker or AWS KMS signer for enterprise air-gapped signing.
              </p>
            </div>

            <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs font-mono">
              <div>
                <label className="text-slate-600 block mb-1">Cloudflare Worker / Docker HSM URL:</label>
                <input
                  type="text"
                  disabled={loading}
                  value={hsmUrl}
                  onChange={(e) => setHsmUrl(e.target.value)}
                  className="w-full px-3 py-2 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>

              <div>
                <div className="flex items-center justify-between mb-1">
                  <label className="text-slate-600">HMAC Authentication Token:</label>
                  <button
                    type="button"
                    onClick={() => toggleSecret("hsm_token")}
                    className="text-slate-400 hover:text-slate-600 text-[11px] flex items-center space-x-1"
                  >
                    {showSecrets["hsm_token"] ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3 text-blue-600" />}
                    <span>{showSecrets["hsm_token"] ? "Hide" : "Show"}</span>
                  </button>
                </div>
                <input
                  type={showSecrets["hsm_token"] ? "text" : "password"}
                  disabled={loading}
                  value={hmacToken}
                  placeholder="Enter HMAC secret token"
                  onChange={(e) => setHmacToken(e.target.value)}
                  className="w-full px-3 py-2 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent font-mono"
                />
              </div>
            </div>
          </div>
        )}

        {/* Save Bar */}
        <div className="flex items-center justify-between pt-2">
          <div className="text-[11px] font-mono text-slate-500 flex items-center space-x-1.5">
            <Lock className="w-3 h-3 text-slate-400" />
            <span>Config changes immediately affect live atomic coordinator execution.</span>
          </div>

          <button
            type="submit"
            disabled={loading || saving}
            className="px-6 py-2.5 bg-accent hover:bg-accent-hover text-white font-medium text-xs font-mono rounded-lg transition flex items-center space-x-2 shadow-xs disabled:opacity-50"
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
    </div>
  );
};


