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
      className="fixed inset-0 z-50 bg-slate-900/50 flex items-center justify-center p-3 sm:p-6 select-none animate-in fade-in duration-150"
      onClick={(e) => {
        if (e.target === e.currentTarget) {
          onClose();
        }
      }}
    >
      <div className="bg-surface border border-surface-border rounded-xl w-[1240px] h-[820px] max-w-[96vw] max-h-[94vh] flex flex-col shadow-2xl overflow-hidden animate-in zoom-in-95 duration-150">
        {/* Modal Top Header Bar */}
        <div className="flex items-center justify-between px-6 py-3 border-b border-surface-border bg-surface shrink-0">
          <div className="flex items-center space-x-3">
            <div className="w-8 h-8 rounded-md bg-accent-dim border border-accent-line flex items-center justify-center text-accent">
              <SettingsIcon className="w-4 h-4" />
            </div>
            <div>
              <h2 className="text-sm font-bold font-mono tracking-wider text-slate-900 leading-tight">
                System Setting
              </h2>
            </div>
          </div>

          <div className="flex items-center space-x-2.5">
            <button
              type="button"
              onClick={onClose}
              className="p-1.5 rounded-md text-slate-400 hover:text-slate-700 hover:bg-surface-raised transition border border-transparent hover:border-surface-border cursor-pointer"
              title="Close Settings (ESC)"
            >
              <X className="w-4 h-4" />
            </button>
          </div>
        </div>

        {/* Modal Fixed Container */}
        <div className="flex-1 min-h-0 overflow-hidden">
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
