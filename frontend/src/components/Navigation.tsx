import React from "react";
import {
  Layers,
  Percent,
  Coins,
  Triangle,
  Target,
  FlaskConical,
} from "lucide-react";

export type ActiveTab =
  | "spatial"
  | "funding"
  | "spot_arb"
  | "triangle"
  | "prediction"
  | "statarb"
  | "portfolio_monitor";

export interface NavigationProps {
  activeTab: ActiveTab;
  onSelectTab: (tab: ActiveTab) => void;
  spatialCount?: number;
  fundingCount?: number;
  spotCount?: number;
  triangleCount?: number;
  predictionCount?: number;
}

export const Navigation: React.FC<NavigationProps> = ({
  activeTab,
  onSelectTab,
  spatialCount = 0,
  fundingCount = 0,
  spotCount = 0,
  triangleCount = 0,
  predictionCount = 0,
}) => {
  const tabs = [
    {
      id: "spatial" as ActiveTab,
      label: "Spread Range",
      icon: Layers,
      badge: spatialCount > 0 ? spatialCount : undefined,
    },
    {
      id: "funding" as ActiveTab,
      label: "Funding Rate",
      icon: Percent,
      badge: fundingCount > 0 ? fundingCount : undefined,
    },
    {
      id: "spot_arb" as ActiveTab,
      label: "Spot Trade",
      icon: Coins,
      badge: spotCount > 0 ? spotCount : undefined,
    },
    {
      id: "triangle" as ActiveTab,
      label: "Triangle",
      icon: Triangle,
      badge: triangleCount > 0 ? triangleCount : undefined,
    },
    {
      id: "prediction" as ActiveTab,
      label: "Prediction Market",
      icon: Target,
      badge: predictionCount > 0 ? predictionCount : undefined,
    },
    {
      id: "statarb" as ActiveTab,
      label: "Statistical",
      icon: FlaskConical,
    },
  ];

  return (
    <nav className="w-full h-full grid grid-cols-6 divide-x divide-surface-border select-none bg-surface">
      {tabs.map((t) => {
        const Icon = t.icon;
        const isActive = activeTab === t.id;
        return (
          <button
            key={t.id}
            onClick={() => onSelectTab(t.id)}
            className={`relative h-full flex items-center justify-center space-x-1.5 px-2 lg:px-2.5 text-[11px] xl:text-xs font-mono transition select-none group cursor-pointer ${
              isActive
                ? "text-accent font-bold bg-surface"
                : "text-slate-600 hover:text-slate-900 hover:bg-slate-50/50 font-medium"
            }`}
            title={t.label}
          >
            <Icon
              className={`w-3.5 h-3.5 xl:w-4 xl:h-4 transition shrink-0 ${
                isActive ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
              }`}
            />
            <span className="truncate tracking-tight font-mono">
              {t.label}
            </span>

            {/* Render badge only when present to give maximum room to labels */}
            {t.badge !== undefined && (
              <span className="inline-flex items-center justify-center min-w-[16px] h-4 shrink-0">
                <span
                  className={`px-1.5 py-0.2 rounded-sm text-[10px] font-bold font-mono min-w-[16px] text-center leading-tight transition ${
                    isActive
                      ? "bg-accent text-white shadow-2xs"
                      : "bg-accent-dim text-accent border border-accent-line/60"
                  }`}
                >
                  {t.badge}
                </span>
              </span>
            )}

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
