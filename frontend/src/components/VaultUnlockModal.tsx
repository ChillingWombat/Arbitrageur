import React, { useState, useEffect } from "react";
import { Lock, Shield, Eye, EyeOff, KeyRound, AlertTriangle } from "lucide-react";
import { api } from "../services/api";
import type { VaultStatus } from "../types";

interface VaultUnlockModalProps {
  isOpen: boolean;
  vaultStatus: VaultStatus | null;
  /** "change" collects the current password plus a new one and rotates the
   *  vault + Stronghold snapshot atomically (backend `change_vault_password`). */
  mode?: "unlock" | "change";
  onClose?: () => void;
  onSuccess: (status: VaultStatus) => void;
  dismissible?: boolean;
}

export const VaultUnlockModal: React.FC<VaultUnlockModalProps> = ({
  isOpen,
  vaultStatus,
  mode = "unlock",
  onClose,
  onSuccess,
  dismissible = false,
}) => {
  const [password, setPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [currentPassword, setCurrentPassword] = useState("");
  const [totpCode, setTotpCode] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Reset inputs when modal opens
  useEffect(() => {
    if (isOpen) {
      setPassword("");
      setConfirmPassword("");
      setCurrentPassword("");
      setTotpCode("");
      setError(null);
      setShowPassword(false);
    }
  }, [isOpen]);

  // Support Escape key only if explicitly dismissible
  useEffect(() => {
    if (!dismissible) return;
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape" && isOpen && !loading) {
        onClose?.();
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [isOpen, loading, onClose, dismissible]);

  if (!isOpen) return null;

  const isInitializing = vaultStatus ? !vaultStatus.is_initialized : false;
  const isChanging = mode === "change";
  // The code field shows whenever MFA is enrolled — including a change-mode
  // rotation, which is gated by the same second factor. A backend "code
  // required" error keeps it visible for flag-drifted vaults.
  const mfaRequired =
    !isInitializing &&
    (vaultStatus?.mfa_enabled === true || /code/i.test(error ?? ""));

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);

    if ((isInitializing || isChanging) && Array.from(password).length < 6) {
      setError("Password must be at least 6 characters long.");
      return;
    }

    if ((isInitializing || isChanging) && password !== confirmPassword) {
      setError("Passwords do not match. Please re-enter.");
      return;
    }

    setLoading(true);
    try {
      let updatedStatus: VaultStatus;
      const code = totpCode.trim() || undefined;
      if (isChanging) {
        updatedStatus = await api.changeVaultPassword(currentPassword, password, code);
      } else if (isInitializing) {
        updatedStatus = await api.initializeVault(password);
      } else {
        updatedStatus = await api.unlockVault(password, code);
      }
      setPassword("");
      setConfirmPassword("");
      setCurrentPassword("");
      setTotpCode("");
      onSuccess(updatedStatus);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 bg-slate-950/80 backdrop-blur-3xl flex items-center justify-center p-4 select-none animate-in fade-in duration-200"
      style={{ backdropFilter: "blur(32px)", WebkitBackdropFilter: "blur(32px)" }}
      onClick={(e) => {
        if (dismissible && e.target === e.currentTarget && !loading) {
          onClose?.();
        }
      }}
    >
      <div className="bg-surface border border-surface-border rounded-2xl p-6 max-w-lg w-full shadow-2xl space-y-5 animate-in zoom-in-95 duration-150">
        <div className="flex items-start justify-between">
          <div className="flex items-center space-x-3">
            <div className="w-10 h-10 rounded-xl bg-accent-dim border border-accent-line flex items-center justify-center text-accent-strong shadow-2xs">
              {isInitializing ? <KeyRound className="w-5 h-5" /> : <Lock className="w-5 h-5" />}
            </div>
            <div>
              <h3 className="text-base font-bold font-sans tracking-tight text-slate-900 leading-snug">
                {isChanging
                  ? "Change Master Password"
                  : isInitializing
                    ? "Create Master Password"
                    : "Unlock Workspace"}
              </h3>
              <p className="text-xs text-slate-500 font-sans mt-0.5">
                {isChanging
                  ? "Rotates the vault key and re-encrypts the shared EVM wallet atomically"
                  : isInitializing
                    ? "Set a master password to encrypt and protect your credential vault"
                    : "Enter your master password to unlock trading and credentials"}
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
          {isChanging && (
            <div>
              <label className="block text-xs font-sans font-semibold text-slate-700 mb-1">
                Current Password
              </label>
              <input
                type={showPassword ? "text" : "password"}
                value={currentPassword}
                onChange={(e) => setCurrentPassword(e.target.value)}
                autoFocus
                placeholder="Enter current master password..."
                className="w-full bg-surface border border-surface-border rounded-lg px-3.5 py-2 text-xs font-mono text-slate-900 placeholder:text-muted focus:outline-none focus:ring-2 focus:ring-primary/15 focus:border-primary transition-all shadow-2xs"
              />
            </div>
          )}
          <div>
            <label className="block text-xs font-sans font-semibold text-slate-700 mb-1">
              {isChanging ? "New Password" : "Master Password"}
            </label>
            <div className="relative">
              <input
                type={showPassword ? "text" : "password"}
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                autoFocus={!isChanging}
                placeholder={
                  isChanging || isInitializing
                    ? "New master password (min 6 chars)..."
                    : "Enter master password..."
                }
                className="w-full bg-surface border border-surface-border rounded-lg px-3.5 py-2 text-xs font-mono text-slate-900 placeholder:text-muted focus:outline-none focus:ring-2 focus:ring-primary/15 focus:border-primary transition-all shadow-2xs pr-10"
              />
              <button
                type="button"
                onClick={() => setShowPassword(!showPassword)}
                className="absolute right-3 top-1/2 -translate-y-1/2 text-muted hover:text-slate-600 transition cursor-pointer"
              >
                {showPassword ? <EyeOff className="w-4 h-4" /> : <Eye className="w-4 h-4" />}
              </button>
            </div>
          </div>

          {(isInitializing || isChanging) && (
            <div>
              <label className="block text-xs font-sans font-semibold text-slate-700 mb-1">
                Confirm Password
              </label>
              <input
                type={showPassword ? "text" : "password"}
                value={confirmPassword}
                onChange={(e) => setConfirmPassword(e.target.value)}
                placeholder="Confirm master password..."
                className="w-full bg-surface border border-surface-border rounded-lg px-3.5 py-2 text-xs font-mono text-slate-900 placeholder:text-muted focus:outline-none focus:ring-2 focus:ring-primary/15 focus:border-primary transition-all shadow-2xs"
              />
              <p className="mt-1.5 text-[11px] text-slate-500 font-sans leading-relaxed">
                Use at least 6 characters. Letters, numbers, and symbols are supported. Remember this password — it cannot be recovered if lost.
              </p>
            </div>
          )}

          {mfaRequired && (
            <div>
              <label className="block text-xs font-sans font-semibold text-slate-700 mb-1">
                Authenticator Code
              </label>
              <input
                type="text"
                inputMode="numeric"
                autoComplete="one-time-code"
                maxLength={6}
                value={totpCode}
                onChange={(e) => setTotpCode(e.target.value.replace(/\D/g, "").slice(0, 6))}
                placeholder="6-digit code"
                className="w-full bg-surface border border-surface-border rounded-lg px-3.5 py-2 text-xs font-mono tracking-[0.3em] text-slate-900 placeholder:text-muted placeholder:tracking-normal focus:outline-none focus:ring-2 focus:ring-primary/15 focus:border-primary transition-all shadow-2xs"
              />
            </div>
          )}

          <div className="p-3 rounded-lg bg-blue-50/60 border border-blue-100 flex items-start space-x-2 text-xs text-slate-600">
            <Shield className="w-4 h-4 text-accent-strong shrink-0 mt-0.5" />
            <span className="font-sans leading-relaxed text-[11px]">
              <span className="block">Credentials are encrypted at rest in vault.enc with AES-256-GCM and Argon2id, and decrypted exclusively in-memory on your local device.</span>
              <span className="block mt-1">Operational state and history are stored locally in SQLite and protected by filesystem permissions, not by vault encryption.</span>
            </span>
          </div>

          <div className="pt-2 border-t border-surface-border">
            <button
              type="submit"
              disabled={
                loading ||
                password.length === 0 ||
                ((isInitializing || isChanging) && confirmPassword.length === 0) ||
                (isChanging && currentPassword.length === 0) ||
                (mfaRequired && totpCode.length !== 6)
              }
              className="w-full btn-primary px-2.5 py-2.5 text-xs font-semibold shadow-xs flex items-center justify-center space-x-1.5 cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {loading ? (
                <span className="animate-pulse">
                  {isChanging ? "Rotating keys..." : "Processing..."}
                </span>
              ) : (
                <span>
                  {isChanging
                    ? "Change Password"
                    : isInitializing
                      ? "Create Master Password & Enter"
                      : "Unlock Workspace"}
                </span>
              )}
            </button>
            {dismissible && onClose && (
              <button
                type="button"
                onClick={onClose}
                disabled={loading}
                className="w-full mt-2 btn-secondary px-2 py-2 text-xs font-mono cursor-pointer"
              >
                Cancel
              </button>
            )}
          </div>
        </form>
      </div>
    </div>
  );
};
