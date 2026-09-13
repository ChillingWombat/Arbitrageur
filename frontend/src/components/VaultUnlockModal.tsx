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

    if (isInitializing && Array.from(password).length < 12) {
      setError("Password must be at least 12 characters long.");
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
    <div className="fixed inset-0 z-50 bg-slate-900/40 backdrop-blur-xs flex items-center justify-center p-4 select-none">
      <div className="bg-surface border border-surface-border rounded-xl p-6 max-w-md w-full shadow-2xl space-y-5 animate-in fade-in zoom-in-95 duration-150">
        <div className="flex items-start justify-between">
          <div className="flex items-center space-x-3">
            <div className="w-10 h-10 rounded-lg bg-accent-dim border border-accent-line flex items-center justify-center text-accent">
              {isInitializing ? <KeyRound className="w-5 h-5" /> : <Lock className="w-5 h-5" />}
            </div>
            <div>
              <h3 className="text-base font-bold font-mono tracking-wider text-slate-900">
                {isInitializing ? "Create Password" : "Unlock Secure Vault"}
              </h3>
              <p className="text-xs text-slate-500 font-mono">
                {isInitializing
                  ? "Create a password to encrypt API credentials"
                  : "Enter your password to access sensitive exchange keys"}
              </p>
            </div>
          </div>
        </div>

        {error && (
          <div className="p-3 bg-loss-dim border border-loss-border rounded-lg flex items-center space-x-2 text-xs font-mono text-loss">
            <AlertTriangle className="w-4 h-4 shrink-0" />
            <span>{error}</span>
          </div>
        )}

        <form onSubmit={handleSubmit} className="space-y-4">
          <div>
            <label className="block text-[11px] font-mono font-semibold text-slate-600 mb-1">
              Password
            </label>
            <div className="relative">
              <input
                type={showPassword ? "text" : "password"}
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                autoFocus
                placeholder={isInitializing ? "Create password..." : "Enter password..."}
                className="w-full bg-surface-raised border border-surface-border rounded-lg px-3.5 py-2.5 text-xs font-mono text-slate-900 placeholder:text-slate-400 focus:outline-none focus:ring-2 focus:ring-accent/30 focus:border-accent transition pr-10"
              />
              <button
                type="button"
                onClick={() => setShowPassword(!showPassword)}
                className="absolute right-3 top-1/2 -translate-y-1/2 text-slate-400 hover:text-slate-600 transition"
              >
                {showPassword ? <EyeOff className="w-4 h-4" /> : <Eye className="w-4 h-4" />}
              </button>
            </div>
          </div>

          {isInitializing && (
            <div>
              <label className="block text-[11px] font-mono font-semibold text-slate-600 mb-1">
                Confirm Password
              </label>
              <input
                type={showPassword ? "text" : "password"}
                value={confirmPassword}
                onChange={(e) => setConfirmPassword(e.target.value)}
                placeholder="Confirm password..."
                className="w-full bg-surface-raised border border-surface-border rounded-lg px-3.5 py-2.5 text-xs font-mono text-slate-900 placeholder:text-slate-400 focus:outline-none focus:ring-2 focus:ring-accent/30 focus:border-accent transition"
              />
              <p className="mt-1.5 text-[10px] text-slate-500 font-mono leading-relaxed">
                Use at least 12 characters. Letters, numbers, and symbols are accepted. Longer passwords are safer.
              </p>
            </div>
          )}

          <div className="p-3 rounded-lg bg-blue-50/60 border border-blue-100 flex items-start space-x-2 text-[11px] text-slate-600">
            <Shield className="w-4 h-4 text-accent shrink-0 mt-0.5" />
            <span>
              All API keys are encrypted at rest locally (AES-256-GCM with Argon2id key derivation). Decrypted strictly in-memory during active sessions.
            </span>
          </div>

          <div className="flex justify-end space-x-2.5 pt-2">
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-2 bg-surface-raised hover:bg-slate-200 text-slate-700 rounded-lg text-xs font-mono font-medium transition"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={loading || password.length === 0}
              className="px-5 py-2 bg-accent hover:bg-blue-600 disabled:opacity-50 text-white rounded-lg text-xs font-mono font-medium transition shadow-xs flex items-center space-x-1.5"
            >
              {loading ? (
                <span className="animate-pulse">Processing...</span>
              ) : (
                <span>{isInitializing ? "Create & Unlock Vault" : "Unlock Vault"}</span>
              )}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
};
