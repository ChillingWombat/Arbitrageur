import React from "react";
import {
  Layers,
  Percent,
  Target,
  FlaskConical,
  ShieldCheck,
  PlaySquare,
  TrendingUp,
  Settings,
  Coins,
} from "lucide-react";

export type ActiveTab =
  | "spatial"
  | "spot_arb"
  | "funding"
  | "prediction"
  | "statarb"
  | "risk"
  | "sim"
  | "performance"
  | "settings";

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
      shortLabel: "Futures",
      fullLabel: "Convergence",
      icon: Layers,
      badge: spatialCount > 0 ? spatialCount : undefined,
    },
    {
      id: "spot_arb" as ActiveTab,
      shortLabel: "Spot Arb",
      fullLabel: "Bot",
      icon: Coins,
      badge: spotCount > 0 ? spotCount : undefined,
    },
    {
      id: "funding" as ActiveTab,
      shortLabel: "Funding",
      fullLabel: "Radar",
      icon: Percent,
      badge: fundingCount > 0 ? fundingCount : undefined,
    },
    {
      id: "prediction" as ActiveTab,
      shortLabel: "Prediction",
      fullLabel: "Parity",
      icon: Target,
      badge: predictionCount > 0 ? predictionCount : undefined,
    },
    {
      id: "statarb" as ActiveTab,
      shortLabel: "Stat-Arb",
      fullLabel: "Lab",
      icon: FlaskConical,
    },
    {
      id: "risk" as ActiveTab,
      shortLabel: "DeltaGuard",
      fullLabel: "& Risk",
      icon: ShieldCheck,
    },
    {
      id: "sim" as ActiveTab,
      shortLabel: "Simulation",
      fullLabel: "Studio",
      icon: PlaySquare,
    },
    {
      id: "performance" as ActiveTab,
      shortLabel: "Performance",
      fullLabel: "",
      icon: TrendingUp,
    },
    {
      id: "settings" as ActiveTab,
      shortLabel: "Settings",
      fullLabel: "",
      icon: Settings,
    },
  ];

  return (
    <nav className="flex items-center space-x-1 overflow-x-auto py-1 px-1.5 bg-surface-raised/80 rounded-xl border border-surface-border select-none">
      {tabs.map((t) => {
        const Icon = t.icon;
        const isActive = activeTab === t.id;
        return (
          <button
            key={t.id}
            onClick={() => onSelectTab(t.id)}
            className={`flex items-center space-x-1.5 px-2.5 py-1 rounded-lg text-xs font-mono font-medium transition whitespace-nowrap group shrink-0 ${
              isActive
                ? "bg-surface text-accent font-semibold shadow-xs border border-accent-line/60"
                : "text-slate-600 hover:text-slate-900 hover:bg-surface/60 border border-transparent"
            }`}
            title={`${t.shortLabel} ${t.fullLabel}`.trim()}
          >
            <Icon
              className={`w-3.5 h-3.5 transition shrink-0 ${
                isActive ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
              }`}
            />
            <span>
              <span>{t.shortLabel}</span>
              {t.fullLabel && <span className="hidden 2xl:inline"> {t.fullLabel}</span>}
            </span>

            {t.badge !== undefined && (
              <span className="ml-0.5 px-1.5 py-0.2 rounded text-[10px] font-bold bg-accent-dim text-accent border border-accent-line/60 shrink-0">
                {t.badge}
              </span>
            )}
          </button>
        );
      })}
    </nav>
  );
};
