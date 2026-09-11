import React from "react";
import {
  Layers,
  Percent,
  Target,
  FlaskConical,
  Coins,
} from "lucide-react";

export type ActiveTab =
  | "spatial"
  | "spot_arb"
  | "funding"
  | "prediction"
  | "statarb"
  | "portfolio_monitor";

export interface NavigationProps {
  activeTab: ActiveTab;
  onSelectTab: (tab: ActiveTab) => void;
  spatialCount?: number;
  spotCount?: number;
  fundingCount?: number;
  predictionCount?: number;
}

export const Navigation: React.FC<NavigationProps> = ({
  activeTab,
  onSelectTab,
  spatialCount = 0,
  spotCount = 0,
  fundingCount = 0,
  predictionCount = 0,
}) => {
  const tabs = [
    {
      id: "spatial" as ActiveTab,
      label: "Futures",
      subLabel: "Convergence",
      icon: Layers,
      badge: spatialCount > 0 ? spatialCount : undefined,
    },
    {
      id: "spot_arb" as ActiveTab,
      label: "Spot Arb",
      subLabel: "Flash",
      icon: Coins,
      badge: spotCount > 0 ? spotCount : undefined,
    },
    {
      id: "funding" as ActiveTab,
      label: "Funding",
      subLabel: "Radar",
      icon: Percent,
      badge: fundingCount > 0 ? fundingCount : undefined,
    },
    {
      id: "prediction" as ActiveTab,
      label: "Prediction",
      subLabel: "Parity",
      icon: Target,
      badge: predictionCount > 0 ? predictionCount : undefined,
    },
    {
      id: "statarb" as ActiveTab,
      label: "Stat-Arb",
      subLabel: "Lab",
      icon: FlaskConical,
    },
  ];

  return (
    <nav className="w-full h-full grid grid-cols-5 divide-x divide-surface-border select-none bg-surface">
      {tabs.map((t) => {
        const Icon = t.icon;
        const isActive = activeTab === t.id;
        return (
          <button
            key={t.id}
            onClick={() => onSelectTab(t.id)}
            className={`relative h-full flex items-center justify-center space-x-2 px-3 text-xs font-mono transition select-none group cursor-pointer ${
              isActive
                ? "text-accent font-bold bg-surface"
                : "text-slate-600 hover:text-slate-900 hover:bg-slate-50/50 font-medium"
            }`}
            title={`${t.label} ${t.subLabel}`.trim()}
          >
            <Icon
              className={`w-4 h-4 transition shrink-0 ${
                isActive ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
              }`}
            />
            <span className="truncate tracking-tight">
              <span>{t.label}</span>
              {t.subLabel && <span className="hidden xl:inline text-slate-400 font-normal"> {t.subLabel}</span>}
            </span>

            {/* Fixed tabular badge container to completely prevent number changes from shifting layout */}
            <span className="inline-flex items-center justify-center min-w-[18px] h-4 shrink-0">
              {t.badge !== undefined && (
                <span
                  className={`px-1.5 py-0.2 rounded text-[10px] font-bold font-mono min-w-[18px] text-center leading-tight transition ${
                    isActive
                      ? "bg-accent text-white shadow-2xs"
                      : "bg-accent-dim text-accent border border-accent-line/60"
                  }`}
                >
                  {t.badge}
                </span>
              )}
            </span>

            {/* CommBank-style active bottom underline bar */}
            {isActive && (
              <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
            )}
          </button>
        );
      })}
    </nav>
  );
};
