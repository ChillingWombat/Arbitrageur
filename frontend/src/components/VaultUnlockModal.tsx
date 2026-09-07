import React, { useState } from "react";
import { Lock, Shield, Eye, EyeOff, KeyRound, AlertTriangle } from "lucide-react";
import { api } from "../services/api";
import type { VaultStatus } from "../types";

interface VaultUnlockModalProps {
  isOpen: boolean;
  vaultStatus: VaultStatus | null;
  onClose: () => void;
  onSuccess: (status: VaultStatus) => void;
}

export const VaultUnlockModal: React.FC<VaultUnlockModalProps> = ({
  isOpen,
  vaultStatus,
  onClose,
  onSuccess,
}) => {
  const [password, setPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!isOpen) return null;

  const isInitializing = vaultStatus ? !vaultStatus.is_initialized : false;

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);

    if (password.length < 6) {
      setError("Master password must be at least 6 characters long.");
      return;
    }

    if (isInitializing && password !== confirmPassword) {
      setError("Passwords do not match. Please re-enter.");
      return;
    }

    setLoading(true);
    try {
      let updatedStatus: VaultStatus;
      if (isInitializing) {
        updatedStatus = await api.initializeVault(password);
      } else {
        updatedStatus = await api.unlockVault(password);
      }
      setPassword("");
      setConfirmPassword("");
      onSuccess(updatedStatus);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 bg-black/80 backdrop-blur-md flex items-center justify-center p-4 select-none animate-in fade-in duration-200">
      <div className="glass-panel-raised border border-white/[0.12] rounded-2xl p-6 max-w-md w-full shadow-[0_0_50px_rgba(0,0,0,0.8)] space-y-5 animate-in zoom-in-95 duration-150">
        <div className="flex items-start justify-between">
          <div className="flex items-center space-x-3">
            <div className="w-10 h-10 rounded-xl bg-cyan-500/15 border border-cyan-500/30 flex items-center justify-center text-cyan-300 shadow-[0_0_15px_rgba(34,211,238,0.2)]">
              {isInitializing ? <KeyRound className="w-5 h-5" /> : <Lock className="w-5 h-5" />}
            </div>
            <div>
              <h3 className="text-base font-bold font-mono text-white">
                {isInitializing ? "Initialize Secure Vault" : "Unlock Secure Vault"}
              </h3>
              <p className="text-xs text-white/50 font-mono mt-0.5">
                {isInitializing
                  ? "Set master encryption password for API credentials"
                  : "Enter master password to access sensitive exchange keys"}
              </p>
            </div>
          </div>
        </div>

        {error && (
          <div className="p-3 bg-rose-500/15 border border-rose-500/30 rounded-xl flex items-center space-x-2 text-xs font-mono text-rose-300">
            <AlertTriangle className="w-4 h-4 shrink-0" />
            <span>{error}</span>
          </div>
        )}

        <form onSubmit={handleSubmit} className="space-y-4">
          <div>
            <label className="block text-[11px] uppercase font-mono font-semibold text-white/60 mb-1.5">
              Master Password
            </label>
            <div className="relative">
              <input
                type={showPassword ? "text" : "password"}
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                autoFocus
                placeholder="Enter master password..."
                className="w-full bg-black/50 border border-white/[0.1] rounded-xl px-3.5 py-2.5 text-xs font-mono text-white placeholder:text-white/30 focus:outline-none focus:ring-1 focus:ring-cyan-400 focus:border-cyan-400 transition pr-10"
              />
              <button
                type="button"
                onClick={() => setShowPassword(!showPassword)}
                className="absolute right-3 top-1/2 -translate-y-1/2 text-white/40 hover:text-white/80 transition"
              >
                {showPassword ? <EyeOff className="w-4 h-4" /> : <Eye className="w-4 h-4 text-cyan-400" />}
              </button>
            </div>
          </div>

          {isInitializing && (
            <div>
              <label className="block text-[11px] uppercase font-mono font-semibold text-white/60 mb-1.5">
                Confirm Master Password
              </label>
              <input
                type={showPassword ? "text" : "password"}
                value={confirmPassword}
                onChange={(e) => setConfirmPassword(e.target.value)}
                placeholder="Confirm master password..."
                className="w-full bg-black/50 border border-white/[0.1] rounded-xl px-3.5 py-2.5 text-xs font-mono text-white placeholder:text-white/30 focus:outline-none focus:ring-1 focus:ring-cyan-400 focus:border-cyan-400 transition"
              />
            </div>
          )}

          <div className="p-3 rounded-xl bg-cyan-500/10 border border-cyan-500/20 flex items-start space-x-2.5 text-[11px] text-white/70">
            <Shield className="w-4 h-4 text-cyan-300 shrink-0 mt-0.5" />
            <span className="leading-relaxed">
              All API keys are encrypted at rest locally (AES-256-GCM with PBKDF2 key derivation). Decrypted strictly in-memory during active sessions.
            </span>
          </div>

          <div className="flex justify-end space-x-2.5 pt-2">
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-2 bg-white/[0.06] hover:bg-white/[0.1] border border-white/[0.08] text-white/70 hover:text-white rounded-xl text-xs font-mono font-medium transition"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={loading || password.length === 0}
              className="px-5 py-2 bg-gradient-to-r from-cyan-500 to-indigo-600 hover:from-cyan-400 hover:to-indigo-500 disabled:opacity-50 text-white rounded-xl text-xs font-mono font-medium transition shadow-[0_0_16px_rgba(34,211,238,0.3)] flex items-center space-x-1.5"
            >
              {loading ? (
                <span className="animate-pulse">Processing...</span>
              ) : (
                <span>{isInitializing ? "Set & Unlock" : "Unlock Vault"}</span>
              )}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
};
