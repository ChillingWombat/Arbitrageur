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
      desc: "Cross-Market Spreads",
    },
    {
      id: "funding" as ActiveTab,
      label: "Funding Radar",
      icon: Percent,
      badge: fundingCount > 0 ? fundingCount : undefined,
      desc: "Perpetual Carry & APY",
    },
    {
      id: "prediction" as ActiveTab,
      label: "Prediction Parity",
      icon: Target,
      badge: predictionCount > 0 ? predictionCount : undefined,
      desc: "Synthetic Parity",
    },
    {
      id: "statarb" as ActiveTab,
      label: "Stat-Arb Lab",
      icon: FlaskConical,
      desc: "Kalman Filter & OU",
    },
    {
      id: "risk" as ActiveTab,
      label: "DeltaGuard & Risk",
      icon: ShieldCheck,
      desc: "Exposure & Liquidation",
    },
    {
      id: "sim" as ActiveTab,
      label: "Simulation Studio",
      icon: PlaySquare,
      desc: "HFT Execution Paper",
    },
    {
      id: "settings" as ActiveTab,
      label: "Engine Settings",
      icon: Settings,
      desc: "Keys & Router Config",
    },
  ];

  return (
    <nav className="w-64 border-r border-surface-border bg-surface flex flex-col justify-between py-4 select-none">
      <div className="space-y-1 px-3">
        <div className="px-3 pb-2 text-[10px] font-mono uppercase tracking-widest text-slate-500">
          Trading Modules
        </div>
        {tabs.map((t) => {
          const Icon = t.icon;
          const isActive = activeTab === t.id;
          return (
            <button
              key={t.id}
              onClick={() => onSelectTab(t.id)}
              className={`w-full flex items-center justify-between px-3 py-2.5 rounded-lg text-xs font-mono transition group text-left ${
                isActive
                  ? "bg-accent/10 text-white border border-accent/30 shadow-sm"
                  : "text-slate-400 hover:text-white hover:bg-surface-raised border border-transparent"
              }`}
            >
              <div className="flex items-center space-x-3">
                <Icon
                  className={`w-4 h-4 transition ${
                    isActive ? "text-accent" : "text-slate-400 group-hover:text-slate-200"
                  }`}
                />
                <div>
                  <div className={`font-medium ${isActive ? "text-accent" : ""}`}>{t.label}</div>
                  <div className="text-[10px] text-slate-500">{t.desc}</div>
                </div>
              </div>

              {t.badge !== undefined && (
                <span className="px-2 py-0.5 rounded-full text-[10px] font-bold bg-accent/20 text-accent border border-accent/30">
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
          <span className="text-accent">Connected</span>
        </div>
        <div className="flex justify-between">
          <span>Engine Core:</span>
          <span className="text-profit">Sub-µs Rust</span>
        </div>
        <div className="flex justify-between">
          <span>Router:</span>
          <span className="text-slate-400">Atomic Multi-Leg</span>
        </div>
      </div>
    </nav>
  );
};
