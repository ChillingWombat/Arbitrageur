import React, { useState, useEffect, useCallback, useMemo } from "react";
import {
  SlidersHorizontal,
  Filter,
  Clock,
  ArrowLeftRight,
  X,
  RotateCcw,
  AlertCircle,
} from "lucide-react";
import type {
  OptionsBotConfigDto,
  StrategyAccountRouteDto,
  AccountDto,
} from "../types";
import { defaultOptionsBotConfig } from "../services/optionsBot";
import { api } from "../services/api";
import { InfoTooltip } from "./InfoTooltip";
import { SaveButton, useSaveButton } from "./SaveButton";
import { StrategyRouteCard } from "./ExchangeAccounts";

export interface OptionsSubstrategyDef {
  id: string;
  name: string;
  logStrategyId: string;
  tagline: string;
  description: string;
}

export const LIVE_OPTIONS_SUBSTRATEGIES: OptionsSubstrategyDef[] = [
  {
    id: "parity",
    name: "Put–Call Parity",
    logStrategyId: "options_parity",
    tagline: "Conversion / reversal vs funding-adjusted forward",
    description:
      "Buys or sells synthetic forward (long call + short put at strike K) against the perpetual's funding-adjusted price.",
  },
  {
    id: "box_spread",
    name: "Box Spread",
    logStrategyId: "options_box_spread",
    tagline: "4-leg synthetic bond vs implied rate",
    description:
      "A 4-leg package (bull call spread + bear put spread across 2 strikes) paying a fixed amount at expiry — synthetic zero-coupon bond.",
  },
  {
    id: "jelly_roll",
    name: "Jelly Roll",
    logStrategyId: "options_jelly_roll",
    tagline: "Calendar box — term-structure mispricing",
    description:
      "Two box spreads across different expiries — arbitrage when implied rate term structure diverges from carry.",
  },
  {
    id: "convexity",
    name: "Convexity Boundary",
    logStrategyId: "options_convexity",
    tagline: "Butterfly upper-bound violations",
    description:
      "Butterfly upper-bound violations — sell overpriced butterflies when quoted above maximum payoff bounds.",
  },
];

type SectionId = "parameters" | "filters" | "cadence" | "routing";

interface SectionDef {
  id: SectionId;
  label: string;
  tooltip: string;
  icon: React.ComponentType<{ className?: string }>;
}

const SECTIONS: SectionDef[] = [
  {
    id: "parameters",
    label: "Bot Parameters",
    tooltip: "Entry sizing in base units and minimum net edge hurdle for option entries.",
    icon: SlidersHorizontal,
  },
  {
    id: "filters",
    label: "Contract Filters",
    tooltip: "Time-to-expiration gates and expected yield hurdle to exclude near-expiry pin risk and low-yielding maturities.",
    icon: Filter,
  },
  {
    id: "cadence",
    label: "Cadence & Limits",
    tooltip: "Chain scan polling interval and maximum concurrent open positions capacity.",
    icon: Clock,
  },
  {
    id: "routing",
    label: "Account Routing",
    tooltip: "Map exchange accounts to route this option substrategy's orders on Deribit and hedge venues.",
    icon: ArrowLeftRight,
  },
];

/** Standardized row container for config parameters matching TickerConfigModal */
interface SettingRowProps {
  label: string;
  description?: string;
  tooltip?: string;
  children: React.ReactNode;
}

const SettingRow: React.FC<SettingRowProps> = ({ label, description, tooltip, children }) => (
  <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-2xs space-y-2 transition hover:border-slate-300">
    <div className="flex items-center gap-3">
      <div className="flex items-center gap-1.5 min-w-0">
        <span className="text-xs font-semibold font-sans text-slate-800">{label}</span>
        {tooltip && <InfoTooltip text={tooltip} />}
      </div>
      <div className="shrink-0 flex items-center gap-2">{children}</div>
    </div>
    {description && (
      <p className="text-[11px] font-sans text-muted font-normal leading-normal border-t border-surface-border/60 pt-2">
        {description}
      </p>
    )}
  </div>
);

/** Standardized number input with unit suffix badge matching TickerConfigModal */
interface NumberInputProps {
  value: number | string;
  onChange: (val: string) => void;
  unit?: string;
  placeholder?: string;
  step?: string | number;
  min?: string | number;
  max?: string | number;
  disabled?: boolean;
  className?: string;
}

const NumberInput: React.FC<NumberInputProps> = ({
  value,
  onChange,
  unit,
  placeholder,
  step = "any",
  min,
  max,
  disabled = false,
  className = "w-32",
}) => (
  <div className="flex items-center gap-1.5 shrink-0">
    <input
      type="number"
      step={step}
      min={min}
      max={max}
      value={value}
      placeholder={placeholder}
      disabled={disabled}
      onChange={(e) => onChange(e.target.value)}
      className={`h-9 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-xs font-mono font-bold text-slate-900 tabular-nums focus:outline-none focus:ring-2 focus:ring-accent/20 focus:border-accent transition disabled:opacity-40 ${className}`}
    />
    {unit && (
      <span className="text-[11px] font-mono font-semibold text-muted select-none min-w-[24px]">
        {unit}
      </span>
    )}
  </div>
);

export interface OptionsConfigModalProps {
  isOpen?: boolean;
  onClose: () => void;
  initialSubstrategyId?: string;
  cfg?: OptionsBotConfigDto;
  onSave?: (cfg: OptionsBotConfigDto) => Promise<void> | void;
}

export const OptionsConfigModal: React.FC<OptionsConfigModalProps> = ({
  isOpen = true,
  onClose,
  initialSubstrategyId = "parity",
  cfg,
  onSave,
}) => {
  const [activeSubstrategyId, setActiveSubstrategyId] = useState<string>(() => {
    if (cfg?.substrategyId) return cfg.substrategyId;
    return initialSubstrategyId || "parity";
  });

  const [activeSection, setActiveSection] = useState<SectionId>("parameters");
  const [drafts, setDrafts] = useState<Record<string, OptionsBotConfigDto>>(() => {
    if (cfg?.substrategyId) {
      return { [cfg.substrategyId]: cfg };
    }
    return {};
  });
  const [_loadingConfig, setLoadingConfig] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Strategy Account Routing state
  const [routes, setRoutes] = useState<StrategyAccountRouteDto[]>([]);
  const [routeAccounts, setRouteAccounts] = useState<AccountDto[]>([]);
  const [routingError, setRoutingError] = useState<string | null>(null);

  const saveBtn = useSaveButton();

  // Selected substrategy definition
  const currentSubDef = useMemo(() => {
    return (
      LIVE_OPTIONS_SUBSTRATEGIES.find((s) => s.id === activeSubstrategyId) ??
      LIVE_OPTIONS_SUBSTRATEGIES[0]
    );
  }, [activeSubstrategyId]);

  // Current working draft for the active substrategy
  const currentDraft = useMemo(() => {
    return drafts[activeSubstrategyId] ?? defaultOptionsBotConfig(activeSubstrategyId);
  }, [drafts, activeSubstrategyId]);

  // Sync initialSubstrategyId or cfg if changed externally
  useEffect(() => {
    if (cfg?.substrategyId) {
      setActiveSubstrategyId(cfg.substrategyId);
      setDrafts((prev) => ({ ...prev, [cfg.substrategyId]: cfg }));
    } else if (initialSubstrategyId) {
      setActiveSubstrategyId(initialSubstrategyId);
    }
  }, [cfg, initialSubstrategyId]);

  // Load configuration for the active substrategy if not already in drafts
  useEffect(() => {
    if (!isOpen) return;
    if (drafts[activeSubstrategyId]) return;

    let cancelled = false;
    setLoadingConfig(true);
    api
      .getOptionsBotConfig(activeSubstrategyId)
      .then((loaded) => {
        if (cancelled) return;
        setDrafts((prev) => ({ ...prev, [activeSubstrategyId]: loaded }));
      })
      .catch((err) => {
        if (cancelled) return;
        console.warn(`Could not load options bot config for ${activeSubstrategyId}:`, err);
        const fallback = defaultOptionsBotConfig(activeSubstrategyId);
        setDrafts((prev) => ({ ...prev, [activeSubstrategyId]: fallback }));
      })
      .finally(() => {
        if (!cancelled) setLoadingConfig(false);
      });

    return () => {
      cancelled = true;
    };
  }, [isOpen, activeSubstrategyId, drafts]);

  // Account routing data loader
  const reloadRoutes = useCallback(async () => {
    try {
      const [r, a] = await Promise.all([api.listStrategyRoutes(), api.listAccounts()]);
      setRoutes(r);
      setRouteAccounts(a);
      setRoutingError(null);
    } catch (e) {
      setRoutingError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    if (isOpen && activeSection === "routing") {
      void reloadRoutes();
    }
  }, [isOpen, activeSection, reloadRoutes]);

  // Escape key handler
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
      }
    };
    if (isOpen) {
      window.addEventListener("keydown", handleKeyDown);
    }
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [isOpen, onClose]);

  if (!isOpen) return null;

  const setField = <K extends keyof OptionsBotConfigDto>(key: K, val: OptionsBotConfigDto[K]) => {
    setError(null);
    setDrafts((prev) => {
      const existing = prev[activeSubstrategyId] ?? defaultOptionsBotConfig(activeSubstrategyId);
      return {
        ...prev,
        [activeSubstrategyId]: { ...existing, [key]: val },
      };
    });
  };

  const handleResetDefaults = () => {
    setError(null);
    const existing = drafts[activeSubstrategyId] ?? defaultOptionsBotConfig(activeSubstrategyId);
    const defaults = defaultOptionsBotConfig(activeSubstrategyId);
    setDrafts((prev) => ({
      ...prev,
      [activeSubstrategyId]: {
        ...defaults,
        mode: existing.mode,
        enabled: existing.enabled,
      },
    }));
  };

  const handleSave = async () => {
    setError(null);
    const ok = await saveBtn.run(async () => {
      try {
        await api.setOptionsBotConfig(currentDraft);
        if (onSave) {
          await onSave(currentDraft);
        }
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        setError(msg);
        throw e;
      }
    });
    if (ok) {
      setTimeout(onClose, 400);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 bg-slate-900/50 backdrop-blur-sm flex items-center justify-center p-3 sm:p-6 select-none animate-in fade-in duration-150"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="w-[840px] max-w-[96vw] max-h-[90vh] bg-surface rounded-2xl border border-surface-border shadow-2xl flex flex-col overflow-hidden animate-in zoom-in-95 duration-150">
        {/* Top Header Bar */}
        <div className="flex flex-col border-b border-surface-border bg-surface shrink-0">
          <div className="flex items-center justify-between px-6 py-3.5 border-b border-surface-border/60">
            <div className="flex items-center space-x-3">
              <div className="w-8 h-8 rounded-lg bg-accent-dim border border-accent-line flex items-center justify-center text-accent-strong shadow-2xs">
                <SlidersHorizontal className="w-4 h-4" />
              </div>
              <div>
                <h2 className="text-sm font-bold font-sans tracking-tight text-slate-900 leading-tight">
                  Options Strategy Configurator
                </h2>
                <p className="text-[11px] font-sans text-muted">
                  Per-substrategy execution sizing, contract filters, cadence, and Deribit routing
                </p>
              </div>
            </div>

            <div className="flex items-center space-x-2">
              <button
                type="button"
                onClick={handleResetDefaults}
                className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface hover:bg-surface-raised text-slate-600 hover:text-slate-900 rounded-lg border border-surface-border transition cursor-pointer font-medium text-xs font-sans shadow-2xs"
                title="Reset current substrategy parameters to default"
              >
                <RotateCcw className="w-3.5 h-3.5 text-slate-500" />
                <span>Reset Defaults</span>
              </button>
              <button
                type="button"
                onClick={onClose}
                className="p-1.5 rounded-lg text-muted hover:text-slate-700 hover:bg-slate-100 transition border border-transparent hover:border-surface-border cursor-pointer"
                title="Close Configurator (ESC)"
              >
                <X className="w-4 h-4" />
              </button>
            </div>
          </div>

          {/* Substrategy Selector Tabs */}
          <div className="px-6 py-2 bg-slate-50/60 flex items-center justify-between gap-3 overflow-x-auto">
            <div className="flex items-center gap-1.5">
              <span className="text-[11px] font-sans font-bold uppercase tracking-wider text-muted mr-1">
                Strategy:
              </span>
              {LIVE_OPTIONS_SUBSTRATEGIES.map((sub) => {
                const isActive = activeSubstrategyId === sub.id;
                return (
                  <button
                    key={sub.id}
                    type="button"
                    onClick={() => setActiveSubstrategyId(sub.id)}
                    className={`px-3 py-1.5 rounded-lg text-xs font-sans transition cursor-pointer flex items-center gap-1.5 ${
                      isActive
                        ? "bg-accent text-white font-bold shadow-2xs border border-accent"
                        : "bg-surface hover:bg-slate-100 text-slate-700 border border-surface-border"
                    }`}
                  >
                    <span>{sub.name}</span>
                  </button>
                );
              })}
            </div>

            {/* Mode badge for active substrategy */}
            <div className="flex items-center gap-2 shrink-0">
              <span className="text-[11px] font-sans text-muted">Mode:</span>
              <span
                className={`text-[10px] font-mono font-bold uppercase px-2 py-0.5 rounded border ${
                  currentDraft.mode === "auto"
                    ? "bg-profit/10 border-profit/30 text-profit"
                    : currentDraft.mode === "paused"
                    ? "bg-amber-500/10 border-amber-500/30 text-amber-600"
                    : "bg-slate-100 border-slate-300 text-slate-600"
                }`}
              >
                {currentDraft.mode || "off"}
              </span>
            </div>
          </div>
        </div>

        {/* Modal Body: Left Navigation Rail + Right Content Area */}
        <div className="flex-1 flex min-h-0 overflow-hidden">
          {/* Left Navigation Rail (width 220px, bordered) */}
          <div className="w-[220px] shrink-0 border-r border-surface-border bg-slate-50/70 p-3 flex flex-col justify-between select-none overflow-y-auto">
            <div className="space-y-3">
              <div className="px-2 pt-1 flex items-center justify-between">
                <span className="text-[11px] font-sans font-bold tracking-wider uppercase text-muted">
                  Settings
                </span>
              </div>

              <div className="space-y-1">
                {SECTIONS.map((sec) => {
                  const Icon = sec.icon;
                  const isActive = activeSection === sec.id;
                  return (
                    <button
                      key={sec.id}
                      type="button"
                      onClick={() => setActiveSection(sec.id)}
                      className={`w-full flex items-center space-x-2.5 px-2.5 py-2.5 rounded-lg font-sans text-xs transition cursor-pointer text-left ${
                        isActive
                          ? "bg-accent text-white font-bold shadow-2xs border border-accent"
                          : "text-slate-600 hover:text-slate-900 bg-surface hover:bg-slate-100/80 border border-surface-border/60 hover:border-surface-border"
                      }`}
                    >
                      <Icon className={`w-3.5 h-3.5 shrink-0 ${isActive ? "text-white" : "text-muted"}`} />
                      <div className="min-w-0 flex-1 flex items-center">
                        <span className="truncate">{sec.label}</span>
                        <InfoTooltip
                          text={sec.tooltip}
                          iconClassName={isActive ? "text-blue-100 hover:text-white" : undefined}
                        />
                      </div>
                    </button>
                  );
                })}
              </div>
            </div>

            {/* Substrategy context note on the bottom of the rail */}
            <div className="p-2.5 rounded-lg bg-surface border border-surface-border/80 text-[11px] font-sans text-muted space-y-1">
              <span className="font-bold text-slate-800 block">{currentSubDef.name}</span>
              <p className="line-clamp-3 leading-snug">{currentSubDef.tagline}</p>
            </div>
          </div>

          {/* Right Main Content Panel */}
          <div className="flex-1 overflow-y-auto p-6 space-y-5 bg-surface">
            {/* Section 1: Bot Parameters */}
            {activeSection === "parameters" && (
              <div className="space-y-4">
                <div className="pb-1 border-b border-surface-border/60">
                  <h3 className="text-xs font-bold font-sans text-slate-800 tracking-tight">
                    Bot Parameters — {currentSubDef.name}
                  </h3>
                  <p className="text-[11px] font-sans text-muted mt-0.5">
                    Configure execution sizing and net edge profitability hurdle
                  </p>
                </div>

                <SettingRow
                  label="Entry Size"
                  description="Base currency contract size deployed per execution (e.g. 0.1 BTC or ETH). All combination legs scale by this base unit."
                  tooltip="Base unit entry size. Option legs and perpetual hedge legs are dimensioned using this multiplier."
                >
                  <NumberInput
                    value={currentDraft.qtyBase}
                    onChange={(v) => {
                      const n = parseFloat(v);
                      if (Number.isFinite(n)) setField("qtyBase", Math.max(0.01, Math.min(10.0, n)));
                    }}
                    unit="BTC/ETH"
                    step="0.05"
                    min="0.01"
                    max="10.0"
                  />
                </SettingRow>

                <SettingRow
                  label="Min Net Edge Hurdle"
                  description="Minimum net profit hurdle after fees in basis points (bps) required to trigger an entry."
                  tooltip="Hurdle required after factoring in Deribit maker/taker fees, delivery fees, and bid-ask spread friction."
                >
                  <NumberInput
                    value={currentDraft.minEdgeBps}
                    onChange={(v) => {
                      const n = parseFloat(v);
                      if (Number.isFinite(n)) setField("minEdgeBps", Math.max(0, Math.min(500, n)));
                    }}
                    unit="bps"
                    step="1"
                    min="0"
                    max="500"
                  />
                </SettingRow>
              </div>
            )}

            {/* Section 2: Contract Filters */}
            {activeSection === "filters" && (
              <div className="space-y-4">
                <div className="pb-1 border-b border-surface-border/60">
                  <h3 className="text-xs font-bold font-sans text-slate-800 tracking-tight">
                    Contract Filters — {currentSubDef.name}
                  </h3>
                  <p className="text-[11px] font-sans text-muted mt-0.5">
                    Filter out near-expiry gamma pin risk and illiquid far-dated expirations
                  </p>
                </div>

                {/* Expiry filter guardrail explanation */}
                <div className="p-3.5 bg-accent-dim/40 border border-accent-line/60 rounded-xl flex items-start gap-3">
                  <Filter className="w-4 h-4 text-accent-strong shrink-0 mt-0.5" />
                  <div className="text-xs font-sans text-slate-700 leading-relaxed space-y-1">
                    <p className="font-semibold text-slate-900">Why Filter by Expiration Horizon?</p>
                    <p className="text-[11px] text-muted">
                      Expiry filters safeguard against adverse execution hazards. Contracts expiring in under a few
                      hours suffer from heightened gamma instability, delivery fee drag, and wide bid-ask spreads that prevent
                      orderbook dislocations from converging smoothly. Conversely, contracts past 90 days suffer from low liquidity
                      and prolonged margin lockup. Constraining the scan horizon isolates high-liquidity, high-conviction trades.
                    </p>
                  </div>
                </div>

                <SettingRow
                  label="Min Hours to Expiry"
                  description="Skip option contracts expiring sooner than this cutoff to avoid pin risk and gamma whipsaws."
                  tooltip="Minimum remaining lifetime in hours. Prevents entering decaying contracts too close to settlement."
                >
                  <NumberInput
                    value={currentDraft.minHoursToExpiry}
                    onChange={(v) => {
                      const n = parseFloat(v);
                      if (Number.isFinite(n)) setField("minHoursToExpiry", Math.max(0, Math.min(720, n)));
                    }}
                    unit="hours"
                    step="1"
                    min="0"
                    max="720"
                  />
                </SettingRow>

                <SettingRow
                  label="Max Days to Expiry"
                  description="Skip option contracts expiring later than this horizon to avoid illiquid orderbooks and prolonged capital lockup."
                  tooltip="Maximum remaining maturity in days. Excludes far-dated expiries where carry cost dominates edge."
                >
                  <NumberInput
                    value={currentDraft.maxDaysToExpiry}
                    onChange={(v) => {
                      const n = parseFloat(v);
                      if (Number.isFinite(n)) setField("maxDaysToExpiry", Math.max(1, Math.min(365, n)));
                    }}
                    unit="days"
                    step="1"
                    min="1"
                    max="365"
                  />
                </SettingRow>

                <SettingRow
                  label="Min Expected Yield"
                  description="Minimum annualized expected yield hurdle (APR) required to trigger an entry. Opportunities yielding below this threshold are ignored."
                  tooltip="Annualized expected yield based on net edge and time to expiration. Filters out long-horizon opportunities where edge is eroded by carry cost."
                >
                  <NumberInput
                    value={currentDraft.minExpectedYieldPct ?? 10}
                    onChange={(v) => {
                      const n = parseFloat(v);
                      if (Number.isFinite(n)) setField("minExpectedYieldPct", Math.max(0, Math.min(1000, n)));
                    }}
                    unit="%"
                    step="0.5"
                    min="0"
                    max="1000"
                  />
                </SettingRow>
              </div>
            )}

            {/* Section 3: Cadence & Limits */}
            {activeSection === "cadence" && (
              <div className="space-y-4">
                <div className="pb-1 border-b border-surface-border/60">
                  <h3 className="text-xs font-bold font-sans text-slate-800 tracking-tight">
                    Cadence & Limits — {currentSubDef.name}
                  </h3>
                  <p className="text-[11px] font-sans text-muted mt-0.5">
                    Chain scan polling frequency and concurrent position limits
                  </p>
                </div>

                <SettingRow
                  label="Chain Scan Interval"
                  description="How frequently the background scanner queries Deribit option chains for arbitrage signals."
                  tooltip="Polling cycle in seconds. Lower values detect fleeting mispricings faster; higher values preserve API quota."
                >
                  <NumberInput
                    value={currentDraft.pollIntervalSecs}
                    onChange={(v) => {
                      const n = parseFloat(v);
                      if (Number.isFinite(n)) setField("pollIntervalSecs", Math.max(1, Math.min(60, n)));
                    }}
                    unit="s"
                    step="1"
                    min="1"
                    max="60"
                  />
                </SettingRow>

                <SettingRow
                  label="Max Open Positions"
                  description="Capacity limit: maximum concurrent positions allowed for this substrategy before halting entries."
                  tooltip="Maximum simultaneous positions managed by this bot. Protects margin and avoids position concentration."
                >
                  <NumberInput
                    value={currentDraft.maxOpenPositions}
                    onChange={(v) => {
                      const n = parseFloat(v);
                      if (Number.isFinite(n)) setField("maxOpenPositions", Math.max(1, Math.min(20, n)));
                    }}
                    unit="slots"
                    step="1"
                    min="1"
                    max="20"
                  />
                </SettingRow>
              </div>
            )}

            {/* Section 4: Account Routing */}
            {activeSection === "routing" && (
              <div className="space-y-4">
                <div className="pb-1 border-b border-surface-border/60">
                  <h3 className="text-xs font-bold font-sans text-slate-800 tracking-tight">
                    Account Routing — {currentSubDef.name}
                  </h3>
                  <p className="text-[11px] font-sans text-muted mt-0.5">
                    Map exchange accounts for Deribit options execution and delta hedging
                  </p>
                </div>

                <div className="p-3.5 bg-surface-raised border border-surface-border rounded-xl text-xs font-sans text-muted space-y-1">
                  <p className="font-semibold text-slate-800">Deribit Options Venue Routing</p>
                  <p className="text-[11px] leading-relaxed">
                    Options legs execute on your configured Deribit account. If this strategy engages in synthetic forward hedging
                    or basis arbitrage across exchanges, configure paired accounts to ensure legs route to active credentials.
                  </p>
                </div>

                {routingError && (
                  <div className="p-3 bg-loss-dim border border-loss-border rounded-lg text-xs font-mono text-loss flex items-center gap-2">
                    <AlertCircle className="w-4 h-4 shrink-0" />
                    <span>{routingError}</span>
                  </div>
                )}

                <StrategyRouteCard
                  strategy={{
                    id: currentSubDef.logStrategyId,
                    name: `${currentSubDef.name} Options`,
                    desc: `Routing for Deribit ${currentSubDef.name.toLowerCase()} executions`,
                  }}
                  routes={routes.filter(
                    (r) =>
                      r.strategyId === currentSubDef.logStrategyId ||
                      r.strategyId === currentSubDef.id ||
                      r.strategyId === "options"
                  )}
                  accounts={routeAccounts}
                  onChanged={() => void reloadRoutes()}
                />
              </div>
            )}
          </div>
        </div>

        {/* Modal Bottom Footer Actions */}
        <div className="px-6 py-3.5 border-t border-surface-border bg-surface flex items-center justify-between shrink-0 font-sans text-xs">
          <button
            type="button"
            onClick={handleResetDefaults}
            className="flex items-center space-x-1.5 px-2.5 py-2 bg-surface hover:bg-surface-raised text-slate-600 hover:text-slate-900 rounded-lg border border-surface-border transition cursor-pointer font-medium shadow-2xs"
            title="Reset current substrategy parameters to default"
          >
            <RotateCcw className="w-3.5 h-3.5 text-slate-500" />
            <span>Reset Defaults</span>
          </button>

          <div className="flex items-center space-x-3">
            {error && (
              <span role="alert" className="text-xs font-semibold text-loss max-w-64 text-right truncate" title={error}>
                {error}
              </span>
            )}
            <button
              type="button"
              onClick={onClose}
              className="px-3 py-2 bg-surface hover:bg-surface-raised text-slate-700 border border-surface-border rounded-lg font-medium transition cursor-pointer"
            >
              Cancel
            </button>

            <SaveButton state={saveBtn.state} onClick={() => void handleSave()} />
          </div>
        </div>
      </div>
    </div>
  );
};

export default OptionsConfigModal;
