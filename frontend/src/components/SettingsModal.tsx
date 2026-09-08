import React, { useEffect } from "react";
import { X, Settings as SettingsIcon } from "lucide-react";
import { EngineSettings } from "./EngineSettings";
import type { SystemStatusDto, EngineSettingsDto } from "../types";

interface SettingsModalProps {
  isOpen: boolean;
  onClose: () => void;
  systemStatus?: SystemStatusDto | null;
  onSettingsSaved?: (newSettings: EngineSettingsDto) => void;
}

export const SettingsModal: React.FC<SettingsModalProps> = ({
  isOpen,
  onClose,
  systemStatus,
  onSettingsSaved,
}) => {
  // Close on Escape key press
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
      }
    };
    if (isOpen) {
      window.addEventListener("keydown", handleKeyDown);
    }
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [isOpen, onClose]);

  if (!isOpen) return null;

  return (
    <div
      className="fixed inset-0 z-50 bg-slate-900/40 backdrop-blur-xs flex items-center justify-center p-3 sm:p-6 select-none animate-in fade-in duration-150"
      onClick={(e) => {
        if (e.target === e.currentTarget) {
          onClose();
        }
      }}
    >
      <div className="bg-surface border border-surface-border rounded-2xl w-full max-w-5xl max-h-[92vh] flex flex-col shadow-2xl overflow-hidden animate-in zoom-in-95 duration-150">
        {/* Modal Top Header Bar */}
        <div className="flex items-center justify-between px-6 py-3.5 border-b border-surface-border bg-surface shrink-0">
          <div className="flex items-center space-x-3">
            <div className="w-8 h-8 rounded-lg bg-accent-dim border border-accent-line flex items-center justify-center text-accent">
              <SettingsIcon className="w-4 h-4" />
            </div>
            <div>
              <h2 className="text-sm font-bold font-mono text-slate-900 leading-tight">
                System Engine Settings
              </h2>
              <p className="text-[11px] text-slate-500 font-mono">
                API credentials, encrypted vault, execution parameters & network configuration
              </p>
            </div>
          </div>

          <div className="flex items-center space-x-2.5">
            <span className="hidden sm:inline-block text-[10px] font-mono text-slate-400 bg-surface-raised px-2 py-0.5 rounded border border-surface-border">
              ESC to close
            </span>
            <button
              type="button"
              onClick={onClose}
              className="p-1.5 rounded-lg text-slate-400 hover:text-slate-700 hover:bg-surface-raised transition border border-transparent hover:border-surface-border"
              title="Close Settings (ESC)"
            >
              <X className="w-4 h-4" />
            </button>
          </div>
        </div>

        {/* Modal Scrollable Body */}
        <div className="flex-1 overflow-y-auto">
          <EngineSettings
            systemStatus={systemStatus}
            onClose={onClose}
            onSettingsSaved={onSettingsSaved}
          />
        </div>
      </div>
    </div>
  );
};
