import React, { useState, useEffect } from "react";
import { AlertCircle } from "lucide-react";
import { api } from "../services/api";

export const EngineSettings: React.FC = () => {
  const [hsmUrl, setHsmUrl] = useState("http://127.0.0.1:8787/sign");
  const [hmacToken, setHmacToken] = useState("");
  const [maxSlippageBps, setMaxSlippageBps] = useState(15.0);
  const [minDepthRatio, setMinDepthRatio] = useState(3.0);
  const [robinhoodRouter, setRobinhoodRouter] = useState("0x89e5DB8B5aA49aA85AC63f691524311AEB649eba");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  useEffect(() => {
    const loadSettings = async () => {
      try {
        const s = await api.getEngineSettings();
        setHsmUrl(s.hsm_url);
        setHmacToken(s.hmac_token);
        setMaxSlippageBps(s.max_slippage_bps);
        setMinDepthRatio(s.min_depth_ratio);
        setRobinhoodRouter(s.robinhood_router);
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
        robinhood_router: robinhoodRouter,
      });
      setSaved(true);
      setTimeout(() => setSaved(false), 2500);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      setErrorMsg(msg);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="p-6 space-y-6 overflow-y-auto h-full max-w-4xl">
      <div className="bg-surface p-5 rounded-xl border border-surface-border shadow-xs">
        <h2 className="text-sm font-bold font-mono text-slate-900">
          Arbitrager Engine & Remote Signer Configuration
        </h2>
        <p className="text-xs text-slate-500 mt-1">
          Hardened key management, multi-chain router allowlists, and execution parameters.
        </p>
      </div>

      {errorMsg && (
        <div className="bg-loss-dim border border-loss-border text-loss px-4 py-3 rounded-xl text-xs font-mono flex items-center space-x-2">
          <AlertCircle className="w-4 h-4 flex-shrink-0" />
          <span>{errorMsg}</span>
        </div>
      )}

      <form onSubmit={handleSave} className="space-y-6">
        {/* Remote HSM Signer */}
        <div className="bg-surface border border-surface-border rounded-xl p-5 space-y-4 shadow-xs">
          <div className="border-b border-surface-border pb-3">
            <h3 className="text-xs font-mono font-semibold uppercase text-slate-900">
              Remote HSM & Key Management (Zero-Key Local Storage)
            </h3>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs font-mono">
            <div>
              <label className="text-slate-600 block mb-1">Cloudflare Worker / Docker HSM URL:</label>
              <input
                type="text"
                disabled={loading}
                value={hsmUrl}
                onChange={(e) => setHsmUrl(e.target.value)}
                className="w-full px-3 py-2 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent focus:border-accent"
              />
            </div>

            <div>
              <label className="text-slate-600 block mb-1">HMAC Authentication Token:</label>
              <input
                type="password"
                disabled={loading}
                value={hmacToken}
                placeholder="Enter HMAC secret token"
                onChange={(e) => setHmacToken(e.target.value)}
                className="w-full px-3 py-2 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent focus:border-accent"
              />
            </div>
          </div>
        </div>

        {/* Multi-Chain DEX Allowlists */}
        <div className="bg-surface border border-surface-border rounded-xl p-5 space-y-4 shadow-xs">
          <div className="border-b border-surface-border pb-3">
            <h3 className="text-xs font-mono font-semibold uppercase text-slate-900">
              DEX Router Contract Allowlists
            </h3>
          </div>

          <div className="space-y-3 text-xs font-mono">
            <div>
              <label className="text-slate-600 block mb-1">Robinhood EVM Router Contract:</label>
              <input
                type="text"
                disabled={loading}
                value={robinhoodRouter}
                onChange={(e) => setRobinhoodRouter(e.target.value)}
                className="w-full px-3 py-2 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent focus:border-accent font-mono text-xs"
              />
            </div>
          </div>
        </div>

        {/* Execution Guardrails */}
        <div className="bg-surface border border-surface-border rounded-xl p-5 space-y-4 shadow-xs">
          <div className="border-b border-surface-border pb-3">
            <h3 className="text-xs font-mono font-semibold uppercase text-slate-900">
              Smart Order Routing & Slippage Thresholds
            </h3>
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
                className="w-full px-3 py-2 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent focus:border-accent"
              />
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
                className="w-full px-3 py-2 bg-surface border border-surface-border rounded-md text-slate-900 focus:outline-none focus:ring-1 focus:ring-accent focus:border-accent"
              />
            </div>
          </div>
        </div>

        {/* Save Button */}
        <div className="flex justify-end">
          <button
            type="submit"
            disabled={loading || saving}
            className="px-5 py-2.5 bg-accent hover:bg-accent-hover text-white font-medium text-xs font-mono rounded-lg transition flex items-center shadow-xs disabled:opacity-50"
          >
            <span>{saved ? "Configuration Saved" : saving ? "Applying..." : "Save Settings"}</span>
          </button>
        </div>
      </form>
    </div>
  );
};

