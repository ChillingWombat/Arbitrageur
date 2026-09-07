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
    <nav className="flex items-center space-x-1 overflow-x-auto py-1 px-1.5 bg-white/[0.03] backdrop-blur-xl rounded-2xl border border-white/[0.08] select-none">
      {tabs.map((t) => {
        const Icon = t.icon;
        const isActive = activeTab === t.id;
        return (
          <button
            key={t.id}
            onClick={() => onSelectTab(t.id)}
            className={`flex items-center space-x-1.5 px-3 py-1.5 rounded-xl text-xs font-mono font-medium transition-all duration-200 whitespace-nowrap group shrink-0 ${
              isActive
                ? "bg-white/[0.08] text-white font-semibold shadow-[0_0_16px_rgba(129,140,248,0.3)] border border-white/[0.18] ring-1 ring-white/[0.06]"
                : "text-white/60 hover:text-white/95 hover:bg-white/[0.04] border border-transparent"
            }`}
            title={`${t.shortLabel} ${t.fullLabel}`.trim()}
          >
            <Icon
              className={`w-3.5 h-3.5 transition-colors shrink-0 ${
                isActive ? "text-accent-cyan" : "text-white/40 group-hover:text-white/70"
              }`}
            />
            <span>
              <span>{t.shortLabel}</span>
              {t.fullLabel && <span className="hidden 2xl:inline"> {t.fullLabel}</span>}
            </span>

            {t.badge !== undefined && (
              <span className="ml-1 px-1.5 py-0.2 rounded-full text-[10px] font-bold bg-accent-cyan/15 text-accent-cyan border border-accent-cyan/30 shadow-[0_0_8px_rgba(34,211,238,0.25)] shrink-0">
                {t.badge}
              </span>
            )}
          </button>
        );
      })}
    </nav>
  );
};
