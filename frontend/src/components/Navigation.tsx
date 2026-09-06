import React from "react";
import {
  Layers,
  Percent,
  Target,
  FlaskConical,
  ShieldCheck,
  PlaySquare,
  Settings,
} from "lucide-react";

export type ActiveTab =
  | "spatial"
  | "funding"
  | "prediction"
  | "statarb"
  | "risk"
  | "sim"
  | "settings";

interface NavigationProps {
  activeTab: ActiveTab;
  onSelectTab: (tab: ActiveTab) => void;
  spatialCount?: number;
  fundingCount?: number;
  predictionCount?: number;
}

export const Navigation: React.FC<NavigationProps> = ({
  activeTab,
  onSelectTab,
  spatialCount = 0,
  fundingCount = 0,
  predictionCount = 0,
}) => {
  const tabs = [
    {
      id: "spatial" as ActiveTab,
      label: "Spatial Matrix",
      icon: Layers,
      badge: spatialCount > 0 ? spatialCount : undefined,
    },
    {
      id: "funding" as ActiveTab,
      label: "Funding Radar",
      icon: Percent,
      badge: fundingCount > 0 ? fundingCount : undefined,
    },
    {
      id: "prediction" as ActiveTab,
      label: "Prediction Parity",
      icon: Target,
      badge: predictionCount > 0 ? predictionCount : undefined,
    },
    {
      id: "statarb" as ActiveTab,
      label: "Stat-Arb Lab",
      icon: FlaskConical,
    },
    {
      id: "risk" as ActiveTab,
      label: "DeltaGuard & Risk",
      icon: ShieldCheck,
    },
    {
      id: "sim" as ActiveTab,
      label: "Simulation Studio",
      icon: PlaySquare,
    },
    {
      id: "settings" as ActiveTab,
      label: "Engine Settings",
      icon: Settings,
    },
  ];

  return (
    <nav className="w-56 border-r border-surface-border bg-surface flex flex-col justify-between py-4 select-none">
      <div className="space-y-1 px-3">
        <div className="px-3 pb-2 text-[10px] font-mono uppercase tracking-widest text-slate-400 font-semibold">
          Modules
        </div>
        {tabs.map((t) => {
          const Icon = t.icon;
          const isActive = activeTab === t.id;
          return (
            <button
              key={t.id}
              onClick={() => onSelectTab(t.id)}
              className={`w-full flex items-center justify-between px-3 py-2 rounded-lg text-xs font-mono transition group text-left ${
                isActive
                  ? "bg-accent-dim text-accent border border-accent-line/60 font-medium"
                  : "text-slate-600 hover:text-slate-900 hover:bg-surface-raised border border-transparent"
              }`}
            >
              <div className="flex items-center space-x-2.5">
                <Icon
                  className={`w-3.5 h-3.5 transition ${
                    isActive ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
                  }`}
                />
                <span className={`font-medium ${isActive ? "text-accent" : "text-slate-700"}`}>
                  {t.label}
                </span>
              </div>

              {t.badge !== undefined && (
                <span className="px-1.5 py-0.5 rounded text-[10px] font-semibold bg-accent-dim text-accent border border-accent-line/60">
                  {t.badge}
                </span>
              )}
            </button>
          );
        })}
      </div>

      {/* Footer info */}
      <div className="px-4 py-3 border-t border-surface-border text-[11px] font-mono text-slate-500 space-y-1">
        <div className="flex justify-between">
          <span>Tauri IPC:</span>
          <span className="text-accent font-medium">Connected</span>
        </div>
        <div className="flex justify-between">
          <span>Engine Core:</span>
          <span className="text-profit font-medium">Rust Core</span>
        </div>
        <div className="flex justify-between">
          <span>Router:</span>
          <span className="text-slate-600 font-medium">Multi-Leg</span>
        </div>
      </div>
    </nav>
  );
};
