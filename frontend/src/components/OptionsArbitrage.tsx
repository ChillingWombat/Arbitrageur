import React, { useCallback, useEffect, useMemo, useState } from "react";
import {
  Option,
  RefreshCw,
  AlertCircle,
  CheckCircle2,
  Clock3,
  ChevronRight,
  CircleHelp,
  SlidersHorizontal,
} from "lucide-react";
import { api } from "../services/api";
import type {
  OptionsParityOpportunityDto,
  OptionsParityExecutionParams,
  OptionsPackageOpportunityDto,
  OptionsPackageExecutionParams,
  OptionsBotConfigDto,
  ExecutionLogEntry,
} from "../types";
import { defaultOptionsBotConfig } from "../services/optionsBot";
import { flattenResidualMessage } from "../services/flatten";
import { EmptyState } from "./EmptyState";
import { ExecuteButton } from "./ExecuteButton";
import { ConfirmActionModal } from "./ConfirmActionModal";
import { DetailField, DetailSection } from "./DetailBits";
import { TickerLogMonitor } from "./TickerLogMonitor";
import { BotPhaseBadge, BotLifecycleControls } from "./BotLifecycleControls";
import type { BotPhase } from "../hooks/useBotLifecycle";
import { SaveButton, useSaveButton } from "./SaveButton";

/**
 * Options strategy catalog — the taxonomy from
 * docs/OPTIONS_ARBITRAGE_STRATEGIES.md §1.1, scoped to what venue admission
 * can actually reach. `live` rows have a working scanner; `pending` rows are
 * the roadmap — each names the venue class that unblocks it (IBKR equity
 * options, or a second crypto options venue alongside Deribit).
 */
export interface OptionsSubstrategy {
  /** Selector id — also how opportunity/log filters key the substrategy. */
  id: string;
  /** strategy_id carried on execution logs for this substrategy. */
  logStrategyId: string;
  name: string;
  tagline: string;
  /** One-line explainer surfaced on the rail's help affordance. */
  description: string;
  family: string;
  /** Asset classes the strategy trades — drives the all-assets framing. */
  universe: string;
  status: "live" | "pending";
  /** What admission/integration unblocks the scanner. */
  requires?: string;
}

export const OPTIONS_SUBSTRATEGIES: OptionsSubstrategy[] = [
  {
    id: "parity",
    logStrategyId: "options_parity",
    name: "Put–Call Parity",
    tagline: "Conversion / reversal vs funding-adjusted forward",
    description:
      "Buys or sells the synthetic forward (long call + short put at strike K) against the perpetual's funding-adjusted price. Conversion shorts the rich synthetic; reversal buys the cheap one. Edge realizes at expiry — only premiums and fees are cash at entry.",
    family: "Parity & Convergence",
    universe: "Crypto perps (Deribit) — equities once IBKR options land",
    status: "live",
  },
  {
    id: "box_spread",
    logStrategyId: "options_box_spread",
    name: "Box Spread",
    tagline: "4-leg synthetic bond vs implied rate",
    description:
      "A four-leg package (bull call spread + bear put spread across two strikes) paying a fixed amount at expiry — a synthetic zero-coupon bond. Arbitrage when its implied interest rate diverges from the real funding/borrow rate on the same underlying.",
    family: "Rate Structures",
    universe: "Crypto options (Deribit) — equities once IBKR options land",
    status: "live",
  },
  {
    id: "jelly_roll",
    logStrategyId: "options_jelly_roll",
    name: "Jelly Roll",
    tagline: "Calendar box — term-structure mispricing",
    description:
      "Two box spreads across different expiries — a calendar box. Arbitrage when the implied term structure of rates between expiries diverges from actual funding and carry.",
    family: "Rate Structures",
    universe: "Crypto options across expiries (Deribit)",
    status: "live",
  },
  {
    id: "convexity",
    logStrategyId: "options_convexity",
    name: "Convexity Boundary",
    tagline: "Butterfly upper-bound violations",
    description:
      "A butterfly's price can never exceed its maximum payoff bounds. A book quoting above the convexity upper bound is a locked-in arbitrage sell — buy the wings, sell twice the body.",
    family: "Boundary & Convexity",
    universe: "Crypto option chains (Deribit)",
    status: "live",
  },
  {
    id: "dividend_play",
    logStrategyId: "options_dividend_play",
    name: "Dividend Assignment",
    tagline: "Ex-dividend eve ITM-call non-exercise",
    description:
      "Short ITM calls worth less than the upcoming dividend get exercised early by rational holders. On ex-dividend eve, a failure to exercise leaves capturable edge between the call price and the overnight drop in the underlying.",
    family: "Event & Microstructure",
    universe: "Dividend-paying equities",
    status: "pending",
    requires: "IBKR equity options + corporate-action calendar",
  },
  {
    id: "htb_rebate",
    logStrategyId: "options_htb_rebate",
    name: "HTB Rebate",
    tagline: "Reverse conversion vs stock-borrow cost",
    description:
      "Hard-to-borrow stock earns a borrow rebate for the lender. When a reverse conversion (short synthetic via options, long stock) prices below the borrow cost, the spread is arbitrage.",
    family: "Event & Microstructure",
    universe: "Hard-to-borrow equities",
    status: "pending",
    requires: "IBKR stock-borrow desk + options",
  },
  {
    id: "merger_pin",
    logStrategyId: "options_merger_pin",
    name: "M&A Pinning",
    tagline: "Bear-call spreads on closed cash mergers",
    description:
      "After a cash merger is announced, the target pins near deal price. Bear-call spreads struck above the deal price collect near-riskless premium when venues misprice the residual deal-break risk.",
    family: "Event & Microstructure",
    universe: "Merger-target equities",
    status: "pending",
    requires: "IBKR equity options + deal calendar",
  },
  {
    id: "occ_adjusted",
    logStrategyId: "options_occ_adjusted",
    name: "OCC Adjusted",
    tagline: "Spin-off deliverable vs combo mispricing",
    description:
      "Spin-offs and special dividends change an option's deliverable basket. Arbitrage when the adjusted contract's combo price diverges from the value of its deliverable parts.",
    family: "Event & Microstructure",
    universe: "Adjusted equity contracts",
    status: "pending",
    requires: "IBKR + OCC adjustment feed",
  },
  {
    id: "late_exercise",
    logStrategyId: "options_late_exercise",
    name: "Late-Exercise",
    tagline: "4:00–5:30 PM contra-exercise window",
    description:
      "Between the 4:00 PM close and the 5:30 PM exercise deadline, news can flip whether expiring options should be exercised. Holding the option into the window captures contra-exercise value the close price missed.",
    family: "Event & Microstructure",
    universe: "Equity options at expiry",
    status: "pending",
    requires: "IBKR exercise-desk access",
  },
  {
    id: "odd_lot",
    logStrategyId: "options_odd_lot",
    name: "Odd-Lot Tender",
    tagline: "SEC 13e-4 retail-priority hedges",
    description:
      "SEC 13e-4 tender offers give odd-lot holders (under 100 shares) proration priority. Buying 99 shares plus protective hedges captures a near-guaranteed tender fill when the hedge cost sits below the tender premium.",
    family: "Event & Microstructure",
    universe: "Tender-offer equities",
    status: "pending",
    requires: "IBKR equities + tender calendar",
  },
];

/** Fixed scan profile — there is no per-scan form; a scan always sweeps
    every reachable underlying at this size and edge hurdle. */
const SCAN_QTY_BASE = 0.1;
const MIN_EDGE_BPS = 5;
const SCAN_CURRENCIES = ["BTC", "ETH"] as const;

interface OptionsArbitrageProps {
  executionLogs: ExecutionLogEntry[];
  onRefreshAll: () => Promise<void>;
  isPaused?: boolean;
  isLive?: boolean;
  /** An options-capable venue (Deribit today) is admitted in this environment. */
  optionsVenueAvailable?: boolean;
  /** Net premium financing held inside open option packages — options-wide,
   *  so it surfaces here rather than on the system-wide panels. */
  premiumOutlayUsd?: number | null;
}

const fmtUsd = (v: number): string =>
  `${v < 0 ? "-" : ""}$${Math.abs(v).toLocaleString(undefined, {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  })}`;

const fmtPx = (v: number): string =>
  v.toLocaleString(undefined, {
    minimumFractionDigits: 2,
    maximumFractionDigits: v < 1 ? 6 : 2,
  });

const legLabel = (action: string, role: string): string =>
  `${action.toUpperCase()} ${role.toUpperCase()}`;

/** One scanned setup — parity opportunities carry no substrategy tag, so
 *  the scan results are a discriminated union; package DTOs tag their own
 *  `substrategy_id`. */
export type OptionsScannedOpp =
  | { kind: "parity"; p: OptionsParityOpportunityDto }
  | { kind: "package"; p: OptionsPackageOpportunityDto };

/** Substrategy id an opportunity belongs to. */
export const substrategyOfOpp = (o: OptionsScannedOpp): string =>
  o.kind === "parity" ? "parity" : o.p.substrategy_id;

/** Filter scan results by the rail selection ("all" passes everything). */
export const filterOptionsOpps = <T extends OptionsScannedOpp>(
  opps: T[],
  selected: string,
): T[] =>
  selected === "all" ? opps : opps.filter((o) => substrategyOfOpp(o) === selected);

/** Filter execution logs by substrategy. "all" keeps every options-strategy
 *  row (any catalogued `options_*` owner) — parity rows only under `parity`. */
export const filterOptionsLogs = (
  logs: ExecutionLogEntry[],
  selected: string,
): ExecutionLogEntry[] => {
  const ids = new Set(OPTIONS_SUBSTRATEGIES.map((s) => s.logStrategyId));
  const optionRows = logs.filter(
    (l) =>
      (l.strategy_id != null && ids.has(l.strategy_id)) ||
      (l.trade_type ?? "").toLowerCase().includes("options"),
  );
  if (selected === "all") return optionRows;
  const wanted = OPTIONS_SUBSTRATEGIES.find((s) => s.id === selected)?.logStrategyId;
  return wanted ? optionRows.filter((l) => l.strategy_id === wanted) : [];
};

interface OptionsParityCardProps {
  opp: OptionsParityOpportunityDto;
  executing: boolean;
  executed: boolean;
  disabled: boolean;
  onExecute: () => void;
}

/** One parity opportunity row — pure render, no IPC, so tests can drive it. */
export const OptionsParityCard: React.FC<OptionsParityCardProps> = ({
  opp: o,
  executing,
  executed,
  disabled,
  onExecute,
}) => (
  <div className="panel p-4">
    <div className="flex items-start justify-between gap-4 flex-wrap">
      <div>
        <div className="flex items-center gap-2">
          <span
            className={`text-xs font-bold uppercase px-1.5 py-0.5 rounded ${
              o.direction === "conversion"
                ? "bg-profit-dim text-profit"
                : "bg-accent/10 text-accent"
            }`}
          >
            {o.direction}
          </span>
          <span className="text-[10px] font-semibold uppercase tracking-wide px-1.5 py-0.5 rounded bg-surface-raised text-slate-500 border border-surface-border">
            Parity
          </span>
          <span className="font-mono text-sm">
            {o.currency} {o.strike.toLocaleString()} · exp {o.expiry_iso}
          </span>
        </div>
        <div className="mt-2 grid grid-cols-3 gap-x-6 gap-y-1 text-xs font-mono">
          {[o.call_leg, o.put_leg, o.perp_leg].map((l) => (
            <div key={l.role}>
              <span
                className={
                  l.action === "buy" ? "text-profit" : "text-loss"
                }
              >
                {legLabel(l.action, l.role)}
              </span>{" "}
              {l.instrument} @ {fmtPx(l.venue_price)}
              {l.role !== "perp" && (
                <span className="text-slate-500"> (≈{fmtUsd(l.usd_price)})</span>
              )}
            </div>
          ))}
        </div>
      </div>
      <DetailSection title="">
        <div className="grid grid-cols-2 gap-x-6">
          <DetailField label="Synthetic F" value={fmtUsd(o.synthetic_forward_usd)} />
          <DetailField label="Perp equiv" value={fmtUsd(o.perp_equiv_usd)} />
          <DetailField
            label="Funding 8h"
            value={`${(o.funding_8h * 100).toFixed(4)}%`}
          />
          <DetailField label="Funding drain" value={fmtUsd(o.funding_drain_usd)} />
          <DetailField label="Gross edge" value={fmtUsd(o.gross_edge_usd)} />
          <DetailField label="Est. fees" value={fmtUsd(o.est_fees_usd)} />
          <DetailField
            label="Net edge"
            value={
              <span className={o.net_edge_usd > 0 ? "text-profit" : "text-loss"}>
                {fmtUsd(o.net_edge_usd)} ({o.edge_bps.toFixed(1)} bps)
              </span>
            }
          />
        </div>
      </DetailSection>
      <div className="shrink-0 self-center">
        <ExecuteButton
          executing={executing}
          executed={executed}
          disabled={disabled}
          title={
            o.is_executable
              ? `Execute ${o.direction} on Deribit`
              : o.reason ?? "Not executable"
          }
          onClick={onExecute}
        />
      </div>
    </div>
  </div>
);

interface OptionsPackageCardProps {
  opp: OptionsPackageOpportunityDto;
  executing: boolean;
  executed: boolean;
  disabled: boolean;
  onExecute: () => void;
}

/** One package opportunity row (box / jelly roll / convexity butterfly) —
 *  pure render, no IPC. Legs print side + contract multiplier + exact
 *  instrument so the operator sees precisely what dispatches. */
export const OptionsPackageCard: React.FC<OptionsPackageCardProps> = ({
  opp: o,
  executing,
  executed,
  disabled,
  onExecute,
}) => {
  const expiry = o.expiry2_iso
    ? `exp ${o.expiry_iso} → ${o.expiry2_iso}`
    : `exp ${o.expiry_iso}`;
  return (
    <div className="panel p-4">
      <div className="flex items-start justify-between gap-4 flex-wrap">
        <div>
          <div className="flex items-center gap-2">
            <span className="text-xs font-bold uppercase px-1.5 py-0.5 rounded bg-accent/10 text-accent">
              {o.label}
            </span>
            <span className="font-mono text-sm">
              {o.currency} {o.strikes.map((k) => k.toLocaleString()).join("–")} · {expiry}
            </span>
          </div>
          <div className="mt-2 grid grid-cols-2 gap-x-6 gap-y-1 text-xs font-mono">
            {o.legs.map((l, i) => (
              <div key={`${l.instrument}-${i}`}>
                <span className={l.side === "buy" ? "text-profit" : "text-loss"}>
                  {l.side.toUpperCase()}
                  {l.qty_mult > 1 ? ` ×${l.qty_mult}` : ""}
                </span>{" "}
                {l.instrument} @ {fmtPx(l.venue_price)}
                <span className="text-slate-500"> (≈{fmtUsd(l.usd_price)})</span>
              </div>
            ))}
          </div>
        </div>
        <DetailSection title="">
          <div className="grid grid-cols-2 gap-x-6">
            {o.implied_rate_pct != null && (
              <DetailField label="Implied rate" value={`${o.implied_rate_pct.toFixed(2)}%`} />
            )}
            {o.benchmark_rate_pct != null && (
              <DetailField
                label="Benchmark"
                value={`${o.benchmark_rate_pct.toFixed(2)}%`}
              />
            )}
            <DetailField label="Gross edge" value={fmtUsd(o.gross_edge_usd)} />
            <DetailField label="Est. fees" value={fmtUsd(o.est_fees_usd)} />
            <DetailField
              label="Net edge"
              value={
                <span className={o.net_edge_usd > 0 ? "text-profit" : "text-loss"}>
                  {fmtUsd(o.net_edge_usd)} ({o.edge_bps.toFixed(1)} bps)
                </span>
              }
            />
          </div>
        </DetailSection>
        <div className="shrink-0 self-center">
          <ExecuteButton
            executing={executing}
            executed={executed}
            disabled={disabled}
            title={
              o.is_executable
                ? `Execute ${o.label} on Deribit`
                : o.reason ?? "Not executable"
            }
            onClick={onExecute}
          />
        </div>
      </div>
    </div>
  );
};

/** Left-rail substrategy selector — the Options-page analogue of the
 *  TickerExplorer sidebar: overall view plus one row per substrategy. */
const SubstrategyRail: React.FC<{
  selected: string;
  onSelect: (id: string) => void;
  oppCounts: Map<string, number>;
  totalOpps: number;
}> = ({ selected, onSelect, oppCounts, totalOpps }) => {
  const families = useMemo(() => {
    const order: string[] = [];
    const map = new Map<string, OptionsSubstrategy[]>();
    for (const s of OPTIONS_SUBSTRATEGIES) {
      if (!map.has(s.family)) {
        map.set(s.family, []);
        order.push(s.family);
      }
      map.get(s.family)!.push(s);
    }
    return order.map((f) => ({ family: f, items: map.get(f)! }));
  }, []);
  return (
    <aside className="w-64 shrink-0 border-r border-surface-border bg-surface flex flex-col min-h-0">
      <div className="px-3 pt-3 pb-2 border-b border-surface-border">
        <div className="flex items-center gap-1.5 text-[11px] font-bold font-mono tracking-wider text-slate-900 uppercase">
          <Option className="w-3.5 h-3.5 text-accent-strong" />
          <span>Substrategies</span>
        </div>
        <p className="mt-1 text-[11px] text-slate-500 leading-snug">
          All listed underlyings — crypto, equities and index — not perps only.
        </p>
      </div>
      <div className="flex-1 overflow-y-auto p-2 space-y-1">
        <button
          type="button"
          onClick={() => onSelect("all")}
          className={`w-full text-left px-2.5 py-2 rounded-lg border transition cursor-pointer ${
            selected === "all"
              ? "border-accent-line bg-accent-dim text-accent-strong"
              : "border-surface-border hover:bg-slate-50 text-slate-700"
          }`}
        >
          <div className="flex items-center justify-between gap-2">
            <span className="text-xs font-semibold">All Strategies</span>
            <span className="text-[10px] font-mono font-bold px-1.5 py-0.5 rounded bg-surface-raised text-slate-600 border border-surface-border tabular-nums">
              {totalOpps}
            </span>
          </div>
          <div className="text-[10px] text-slate-500 mt-0.5">
            Combined view — every scanner, monitor and log
          </div>
        </button>
        {families.map(({ family, items }) => (
          <div key={family} className="pt-1">
            <div className="px-2.5 pt-1 pb-0.5 text-[9px] font-bold uppercase tracking-widest text-slate-400">
              {family}
            </div>
            <div className="space-y-1.5">
              {items.map((s) => {
              const active = selected === s.id;
              const live = s.status === "live";
              const count = oppCounts.get(s.id) ?? 0;
              return (
                <button
                  type="button"
                  key={s.id}
                  onClick={() => onSelect(s.id)}
                  title={live ? s.tagline : `${s.tagline} — ${s.requires}`}
                  className={`w-full text-left px-2.5 py-1.5 rounded-lg border transition cursor-pointer ${
                    active
                      ? "border-accent-line bg-accent-dim"
                      : "border-surface-border hover:bg-slate-50"
                  }`}
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="flex items-center gap-1 min-w-0">
                      <span
                        className={`text-xs font-semibold truncate ${
                          active
                            ? "text-accent-strong"
                            : live
                              ? "text-slate-800"
                              : "text-slate-500"
                        }`}
                      >
                        {s.name}
                      </span>
                      <span
                        title={s.description}
                        className="inline-flex shrink-0 cursor-help text-slate-400 hover:text-slate-500"
                      >
                        <CircleHelp className="w-3 h-3" aria-hidden />
                      </span>
                    </span>
                    {live ? (
                      <span className="flex items-center gap-1 shrink-0">
                        {count > 0 && (
                          <span className="text-[10px] font-mono font-bold px-1 py-0.5 rounded bg-accent text-white tabular-nums">
                            {count}
                          </span>
                        )}
                        <span className="text-[9px] font-bold text-profit">READY</span>
                      </span>
                    ) : (
                      <Clock3 className="w-3 h-3 text-slate-400 shrink-0" />
                    )}
                  </div>
                  <div className="text-[10px] text-slate-400 truncate">{s.tagline}</div>
                </button>
              );
              })}
            </div>
          </div>
        ))}
      </div>
    </aside>
  );
};

export const OptionsArbitrage: React.FC<OptionsArbitrageProps> = ({
  executionLogs,
  onRefreshAll,
  isPaused = false,
  isLive = false,
  optionsVenueAvailable = false,
  premiumOutlayUsd = null,
}) => {
  const [selected, setSelected] = useState<string>("all");
  const [dirFilter, setDirFilter] = useState<"all" | "conversion" | "reversal">("all");
  const [opps, setOpps] = useState<OptionsScannedOpp[]>([]);
  const [scanning, setScanning] = useState(false);
  const [scanned, setScanned] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [executing, setExecuting] = useState<string | null>(null);
  const [executed, setExecuted] = useState<Set<string>>(new Set());
  const [message, setMessage] = useState<string | null>(null);
  const [confirmOpp, setConfirmOpp] = useState<OptionsScannedOpp | null>(null);
  const [botCfg, setBotCfg] = useState<OptionsBotConfigDto | null>(null);
  const [botError, setBotError] = useState<string | null>(null);
  const [openPositions, setOpenPositions] = useState(0);
  const [flattening, setFlattening] = useState(false);
  const [flattenError, setFlattenError] = useState<string | null>(null);
  const [showBotConfig, setShowBotConfig] = useState(false);

  const selDef = OPTIONS_SUBSTRATEGIES.find((s) => s.id === selected);
  /** Parity-specific chrome (conversion/reversal direction filter). */
  const parityVisible = selected === "all" || selected === "parity";
  /** Any live scanner is selected (or the combined "all" view) — gates the
   *  scan button, summary pills and the venue-capability warning. */
  const liveVisible = selected === "all" || selDef?.status === "live";
  /** A bot exists only for live scanners — the "All" view is monitor-only
   *  and pending substrategies have no executor to configure. */
  const botSubstrategy = selDef && selDef.status === "live" ? selDef : null;
  const env = isLive ? "live" : "testnet";

  // Poll the selected substrategy's persisted bot config + its open signal
  // count — the same sources the backend executor drains against.
  useEffect(() => {
    let cancelled = false;
    const sub = selDef && selDef.status === "live" ? selDef : null;
    if (!sub) {
      setBotCfg(null);
      setOpenPositions(0);
      return;
    }
    const load = async () => {
      try {
        const [cfg, sigs] = await Promise.all([
          api.getOptionsBotConfig(sub.id),
          api.listSignalExecutions(),
        ]);
        if (cancelled) return;
        setBotCfg(cfg);
        // One parity cycle persists three leg signals ({cycle}-legN) — count
        // distinct cycles, mirroring the executor's open-position book.
        setOpenPositions(
          new Set(
            sigs
              .filter(
                (s) =>
                  s.strategyId === sub.logStrategyId &&
                  s.environment.toLowerCase() === env &&
                  s.status !== "closed" &&
                  s.status !== "failed",
              )
              .map((s) => {
                const i = s.id.lastIndexOf("-leg");
                return i > 0 ? s.id.slice(0, i) : s.id;
              }),
          ).size,
        );
      } catch {
        // Config unreadable → controls stay on inert defaults (botCfg may be
        // null); the next poll retries. Never guess a mode.
      }
    };
    void load();
    const t = setInterval(load, 5000);
    return () => {
      cancelled = true;
      clearInterval(t);
    };
  }, [selDef, env]);

  /** Scan profile — a selected live substrategy scans at its bot config so
   *  manual Scan shows what the bot would see; "all" uses fixed defaults. */
  const scanQty = botSubstrategy && botCfg ? botCfg.qtyBase : SCAN_QTY_BASE;
  const scanEdge = botSubstrategy && botCfg ? botCfg.minEdgeBps : MIN_EDGE_BPS;

  const botPhase: BotPhase = flattening
    ? "stopping"
    : botCfg?.mode === "auto" && botCfg.enabled
      ? "running"
      : botCfg?.mode === "paused"
        ? "paused"
        : botCfg?.mode === "reduce_only"
          ? "reduce_only"
          : "idle";

  const handleBotMode = useCallback(
    async (mode: OptionsBotConfigDto["mode"]) => {
      if (!botSubstrategy) return;
      if (mode === "auto" && isLive) {
        // Backend refuses the write too — surface the reason immediately.
        setBotError(
          "Autonomous options entries are disabled in this environment — run a manual Execute.",
        );
        return;
      }
      const base = botCfg ?? defaultOptionsBotConfig(botSubstrategy.id);
      const next: OptionsBotConfigDto = {
        ...base,
        substrategyId: botSubstrategy.id,
        enabled: mode === "auto",
        mode,
      };
      setBotError(null);
      try {
        await api.setOptionsBotConfig(next);
        setBotCfg(next);
      } catch (e) {
        setBotError(e instanceof Error ? e.message : String(e));
      }
    },
    [botCfg, botSubstrategy, isLive],
  );

  const handleFlatten = useCallback(async () => {
    if (!botSubstrategy) return;
    setFlattening(true);
    setFlattenError(null);
    try {
      const res = await api.flattenTickerPositions(null, botSubstrategy.logStrategyId);
      const residual = flattenResidualMessage(res);
      if (residual !== null) setFlattenError(residual);
    } catch (e) {
      setFlattenError(e instanceof Error ? e.message : String(e));
    } finally {
      setFlattening(false);
    }
  }, [botSubstrategy]);

  const saveBotConfig = useCallback(async (cfg: OptionsBotConfigDto) => {
    await api.setOptionsBotConfig(cfg);
    setBotCfg(cfg);
  }, []);

  const filteredOpps = useMemo(
    () =>
      filterOptionsOpps(opps, selected).filter(
        (o) =>
          dirFilter === "all" || o.kind !== "parity" || o.p.direction === dirFilter,
      ),
    [opps, selected, dirFilter],
  );
  const oppCounts = useMemo(() => {
    const m = new Map<string, number>();
    for (const o of opps) {
      const id = substrategyOfOpp(o);
      m.set(id, (m.get(id) ?? 0) + 1);
    }
    return m;
  }, [opps]);
  const filteredLogs = useMemo(
    () => filterOptionsLogs(executionLogs, selected),
    [executionLogs, selected],
  );
  const executableCount = filteredOpps.filter((o) => o.p.is_executable).length;
  const bestEdge = filteredOpps.reduce(
    (m, o) => Math.max(m, o.p.net_edge_usd),
    -Infinity,
  );
  const realizedPnl = filteredLogs.reduce((acc, l) => acc + (l.net_profit || 0), 0);

  const scan = useCallback(async () => {
    setScanning(true);
    setError(null);
    setMessage(null);
    try {
      // Substrategies in scope: the selection's own scanner, or every live
      // scanner in the combined view — each swept over both underlyings.
      const subs = (
        selected === "all"
          ? OPTIONS_SUBSTRATEGIES.filter((s) => s.status === "live").map((s) => s.id)
          : selDef?.status === "live"
            ? [selDef.id]
            : []
      ) as string[];
      const jobs = subs.flatMap((sub) =>
        SCAN_CURRENCIES.map(async (currency): Promise<OptionsScannedOpp[]> => {
          if (sub === "parity") {
            const v = await api.scanOptionsParity({
              currency,
              qty_base: scanQty,
              min_edge_bps: scanEdge,
              min_hours_to_expiry: botCfg?.minHoursToExpiry,
              max_days_to_expiry: botCfg?.maxDaysToExpiry,
            });
            return v.map((p) => ({ kind: "parity" as const, p }));
          }
          const v = await api.scanOptionsPackages({
            substrategy_id: sub,
            currency,
            qty_base: scanQty,
            min_edge_bps: scanEdge,
            min_hours_to_expiry: botCfg?.minHoursToExpiry,
            max_days_to_expiry: botCfg?.maxDaysToExpiry,
          });
          return v.map((p) => ({ kind: "package" as const, p }));
        }),
      );
      const settled = await Promise.allSettled(jobs);
      const res = settled.flatMap((s) => (s.status === "fulfilled" ? s.value : []));
      const rejected = settled.filter((s) => s.status === "rejected");
      if (rejected.length === settled.length && settled.length > 0) {
        const reason = (rejected[0] as PromiseRejectedResult).reason;
        throw reason instanceof Error ? reason : new Error(String(reason));
      }
      if (rejected.length > 0) {
        setError(
          rejected
            .map((r) =>
              r.reason instanceof Error ? r.reason.message : String(r.reason),
            )
            .join("; "),
        );
      }
      setOpps(res);
      setScanned(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setOpps([]);
    } finally {
      setScanning(false);
    }
  }, [scanQty, scanEdge, botCfg, selected, selDef]);

  const runExecute = useCallback(
    async (opp: OptionsScannedOpp) => {
      setExecuting(opp.p.id);
      setError(null);
      setMessage(null);
      try {
        const res =
          opp.kind === "parity"
            ? await api.executeOptionsParity({
                id: opp.p.id,
                currency: opp.p.currency,
                qty_base: opp.p.qty_base_suggested,
                min_edge_bps: scanEdge,
              } satisfies OptionsParityExecutionParams)
            : await api.executeOptionsPackage({
                id: opp.p.id,
                substrategy_id: opp.p.substrategy_id,
                currency: opp.p.currency,
                qty_base: opp.p.qty_base_suggested,
                min_edge_bps: scanEdge,
              } satisfies OptionsPackageExecutionParams);
        setExecuted((s) => new Set(s).add(opp.p.id));
        setMessage(res.message);
        await onRefreshAll();
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setExecuting(null);
        setConfirmOpp(null);
      }
    },
    [onRefreshAll, scanEdge],
  );

  return (
    <div className="flex h-full overflow-hidden bg-background">
      <SubstrategyRail
        selected={selected}
        onSelect={setSelected}
        oppCounts={oppCounts}
        totalOpps={opps.length}
      />

      <div className="flex-1 overflow-y-auto p-4 pr-5 space-y-4 pb-24 min-w-0">
        {/* Strategy Status ribbon — mirrors the other strategy pages:
            read-only params + phase badge left, configurator, scan and the
            substrategy bot's lifecycle buttons right. The "All" view stays
            monitor-only — per-substrategy controls need a selection. */}
        <div className="ribbon-container">
          <div className="flex flex-wrap items-center gap-2 text-xs font-mono">
            <span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1">
              {selDef ? selDef.name : "Options Strategies"}
            </span>
            {botSubstrategy && <BotPhaseBadge phase={botPhase} />}
            {liveVisible && (
              <>
                <div className="ribbon-pill" title="Net-edge hurdle for reported setups">
                  <span className="text-muted">Min Edge</span>
                  <span className="font-bold text-slate-900 tabular-nums">{scanEdge}</span>
                  <span className="text-muted">BPS</span>
                </div>
                <div className="ribbon-pill" title="Scan/entry size in base units">
                  <span className="text-muted">Qty</span>
                  <span className="font-bold text-slate-900 tabular-nums">{scanQty}</span>
                </div>
                {botSubstrategy && botCfg && (
                  <>
                    <div className="ribbon-pill" title="Bot poll cadence">
                      <span className="text-muted">Poll</span>
                      <span className="font-bold text-slate-900 tabular-nums">
                        {botCfg.pollIntervalSecs}
                      </span>
                      <span className="text-muted">s</span>
                    </div>
                    <div className="ribbon-pill" title="Max simultaneous managed positions">
                      <span className="text-muted">Max Open</span>
                      <span className="font-bold text-slate-900 tabular-nums">
                        {botCfg.maxOpenPositions}
                      </span>
                    </div>
                  </>
                )}
              </>
            )}
            {premiumOutlayUsd != null && Math.abs(premiumOutlayUsd) > 0.005 && (
              <div
                className="ribbon-pill"
                title="Option premiums paid minus received at entry — financing held inside open packages, returned at close/expiry. Kept out of Net Profit."
              >
                <span className="text-muted">Premium Outlay</span>
                <span className="font-bold text-slate-900 tabular-nums">
                  {fmtUsd(premiumOutlayUsd)}
                </span>
              </div>
            )}
            {isPaused && <span className="badge-status-warn">Engine Paused</span>}
            {isLive && (
              <span className="text-xs text-amber-400 border border-amber-500/40 rounded px-1.5 py-0.5 font-sans">
                Manual executes only
              </span>
            )}
          </div>
          <div className="flex items-center space-x-3 shrink-0">
            {(botError || flattenError) && (
              <span
                className="text-xs font-mono text-loss max-w-72 truncate"
                title={botError ?? flattenError ?? ""}
              >
                {botError ?? flattenError}
              </span>
            )}
            {botSubstrategy && (
              <button
                type="button"
                onClick={() => setShowBotConfig(true)}
                className="btn-secondary !text-xs !py-2 !px-2 font-sans cursor-pointer gap-2"
                title={`Configure the ${botSubstrategy.name} bot`}
              >
                <SlidersHorizontal className="w-3.5 h-3.5 text-accent-strong" />
                <span>Configurator</span>
              </button>
            )}
            {liveVisible && (
              <button
                type="button"
                onClick={() => void scan()}
                disabled={scanning || isPaused || !optionsVenueAvailable}
                title={
                  !optionsVenueAvailable
                    ? "No options-capable venue is enabled in this environment"
                    : undefined
                }
                className="btn-primary inline-flex items-center gap-1.5 !text-xs !py-2 !px-2.5 font-sans"
              >
                <RefreshCw className={`w-3.5 h-3.5 ${scanning ? "animate-spin" : ""}`} />
                {scanning ? "Scanning…" : "Scan"}
              </button>
            )}
            {botSubstrategy && (
              <BotLifecycleControls
                phase={botPhase}
                hasOpenPositions={openPositions > 0}
                disabled={botCfg == null}
                allowStartWithOpenPositions
                onStart={() => void handleBotMode("auto")}
                onPause={() => void handleBotMode("paused")}
                onReduceOnly={() => void handleBotMode("reduce_only")}
                onStop={() => void handleFlatten()}
              />
            )}
          </div>
        </div>

        {/* Per-substrategy summary cards — filtered view of the monitor. */}
        <div className="grid grid-cols-2 sm:grid-cols-4 gap-4">
          <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
            <div className="text-xs font-medium text-muted font-mono">Scanned Setups</div>
            <div className="text-lg font-bold text-slate-900 font-mono tabular-nums mt-0.5">
              {filteredOpps.length}
            </div>
            <div className="text-xs text-slate-500 mt-0.5">
              {scanned ? "latest scan" : "no scan yet"}
            </div>
          </div>
          <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
            <div className="text-xs font-medium text-muted font-mono">Best Net Edge</div>
            <div
              className={`text-lg font-bold font-mono tabular-nums mt-0.5 ${
                bestEdge > 0 ? "text-profit" : "text-slate-900"
              }`}
            >
              {bestEdge === -Infinity ? "—" : fmtUsd(bestEdge)}
            </div>
            <div className="text-xs text-slate-500 mt-0.5">
              {filteredOpps.length > 0 ? "after fees + funding" : "no setups"}
            </div>
          </div>
          <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
            <div className="text-xs font-medium text-muted font-mono">Executable</div>
            <div className="text-lg font-bold text-slate-900 font-mono tabular-nums mt-0.5">
              {executableCount}
            </div>
            <div className="text-xs text-slate-500 mt-0.5">passes hurdle at scan size</div>
          </div>
          <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
            <div className="text-xs font-medium text-muted font-mono">Realized PnL</div>
            <div
              className={`text-lg font-bold font-mono tabular-nums mt-0.5 ${
                realizedPnl >= 0 ? "text-profit" : "text-loss"
              }`}
            >
              {filteredLogs.length > 0 ? fmtUsd(realizedPnl) : "—"}
            </div>
            <div className="text-xs text-slate-500 mt-0.5">
              {filteredLogs.length} execution log{filteredLogs.length === 1 ? "" : "s"}
            </div>
          </div>
        </div>

        {/* Venue capability warning — every live scanner quotes Deribit
            today; the warning names the missing capability honestly. */}
        {liveVisible && !optionsVenueAvailable && (
          <div className="p-2.5 rounded-lg border border-amber-500/40 bg-amber-500/10 text-amber-300 text-xs flex items-center gap-2">
            <AlertCircle className="w-4 h-4 shrink-0" />
            <span>
              No options-capable venue is enabled — options scanning currently
              requires Deribit. Enable it under Engine Settings → Exchanges and
              add Client ID + Secret in Exchange Accounts (test.deribit.com
              credentials in Testnet mode). IBKR and other options venues will
              slot into this page as they are wired.
            </span>
          </div>
        )}

        {/* Pending substrategy — honest placeholder panel instead of a
            scanner, so the selector never shows dead space. */}
        {selDef && selDef.status === "pending" && (
          <div className="panel p-4">
            <div className="flex items-center gap-2">
              <ChevronRight className="w-4 h-4 text-accent" />
              <span className="text-sm font-semibold">{selDef.name}</span>
              <span className="text-[10px] font-bold uppercase tracking-wide px-1.5 py-0.5 rounded bg-surface-raised text-slate-500 border border-surface-border">
                Pending
              </span>
            </div>
            <p className="mt-2 text-xs text-slate-500">{selDef.tagline}.</p>
            <p className="mt-1 text-xs text-slate-500">
              Universe: {selDef.universe}. Activates once {selDef.requires} is
              available — results, positions and logs for this substrategy will
              appear here.
            </p>
          </div>
        )}

        {error && (
          <div className="panel p-3 border-loss-border text-loss text-xs flex items-center gap-2">
            <AlertCircle className="w-4 h-4 shrink-0" />
            {error}
          </div>
        )}
        {message && (
          <div className="panel p-3 border-profit-border text-profit text-xs flex items-center gap-2">
            <CheckCircle2 className="w-4 h-4 shrink-0" />
            {message}
          </div>
        )}

        {/* Opportunities — filtered by substrategy and, within parity, by
            conversion/reversal direction. */}
        <div className="space-y-3">
          <div className="flex items-center justify-between gap-3 flex-wrap">
            <h2 className="text-[13px] font-bold font-mono tracking-wider text-slate-900">
              Opportunities
            </h2>
            {parityVisible && opps.length > 0 && (
              <div className="flex items-center bg-surface-raised p-1 rounded-lg border border-surface-border space-x-1">
                {(["all", "conversion", "reversal"] as const).map((d) => (
                  <button
                    key={d}
                    type="button"
                    onClick={() => setDirFilter(d)}
                    className={`px-2 py-1 rounded-lg text-[11px] font-mono font-medium transition cursor-pointer ${
                      dirFilter === d
                        ? "bg-accent text-white shadow-2xs"
                        : "text-slate-600 hover:text-slate-900"
                    }`}
                  >
                    {d === "all" ? "ALL" : d.toUpperCase()}
                  </button>
                ))}
              </div>
            )}
          </div>

          {scanned && filteredOpps.length === 0 && !error && (
            <EmptyState
              icon={Option}
              plain
              title={
                selDef && selected !== "all"
                  ? `No ${selDef.name.toLowerCase()} edge right now`
                  : dirFilter === "all"
                    ? "No options edge right now"
                    : `No ${dirFilter} edge right now`
              }
              hint={
                selDef && selected !== "all"
                  ? selDef.tagline
                  : "Every live scanner sits inside fees on the quoted chains."
              }
            />
          )}

          {filteredOpps.map((o) =>
            o.kind === "parity" ? (
              <OptionsParityCard
                key={o.p.id}
                opp={o.p}
                executing={executing === o.p.id}
                executed={executed.has(o.p.id)}
                disabled={!o.p.is_executable || isPaused || scanning}
                onExecute={() => setConfirmOpp(o)}
              />
            ) : (
              <OptionsPackageCard
                key={o.p.id}
                opp={o.p}
                executing={executing === o.p.id}
                executed={executed.has(o.p.id)}
                disabled={!o.p.is_executable || isPaused || scanning}
                onExecute={() => setConfirmOpp(o)}
              />
            ),
          )}
        </div>

        {/* Execution monitor — filtered by the same substrategy selection. */}
        <TickerLogMonitor
          symbol=""
          namespace="options"
          strategyName="Options Strategies"
          executionLogs={filteredLogs}
          matchAllSymbols
          isPaused={isPaused}
        />
      </div>

      <ConfirmActionModal
        spec={
          confirmOpp
            ? confirmOpp.kind === "parity"
              ? {
                  title: `Execute ${confirmOpp.p.direction} on Deribit?`,
                  subtitle: isLive ? "Real funds at risk" : "Venue demo endpoints — simulated funds",
                  body: `This dispatches three bounded IOC orders on Deribit: ${confirmOpp.p.call_leg.action} ${confirmOpp.p.call_leg.instrument}, ${confirmOpp.p.put_leg.action} ${confirmOpp.p.put_leg.instrument}, ${confirmOpp.p.perp_leg.action} ${confirmOpp.p.perp_leg.instrument}. A partially-filled cycle leaves residual exposure that must be flattened manually.`,
                  confirmLabel: "Execute",
                }
              : {
                  title: `Execute ${confirmOpp.p.label} on Deribit?`,
                  subtitle: isLive ? "Real funds at risk" : "Venue demo endpoints — simulated funds",
                  body: `This dispatches ${confirmOpp.p.legs.length} bounded IOC orders on Deribit: ${confirmOpp.p.legs
                    .map(
                      (l) =>
                        `${l.side} ${l.qty_mult > 1 ? `×${l.qty_mult} ` : ""}${l.instrument}`,
                    )
                    .join(", ")}. A partially-filled package leaves residual exposure that must be flattened manually.`,
                  confirmLabel: "Execute",
                }
            : null
        }
        onConfirm={() => (confirmOpp ? runExecute(confirmOpp) : undefined)}
        onCancel={() => setConfirmOpp(null)}
        onError={(m) => setError(m)}
      />

      {botSubstrategy && botCfg && showBotConfig && (
        <OptionsBotConfigModal
          cfg={botCfg}
          name={botSubstrategy.name}
          onSave={saveBotConfig}
          onClose={() => setShowBotConfig(false)}
        />
      )}
    </div>
  );
};

const BOT_FIELD_DEFS: {
  key: keyof OptionsBotConfigDto;
  label: string;
  tooltip: string;
  step?: string;
}[] = [
  {
    key: "qtyBase",
    label: "Entry size (base units)",
    tooltip: "Per-entry size in base units (BTC or ETH) — the manual Scan uses this size too.",
  },
  {
    key: "minEdgeBps",
    label: "Min net edge (bps)",
    tooltip: "Net-of-fees edge hurdle an entry must clear — the manual Scan reports at this hurdle too.",
    step: "0.5",
  },
  {
    key: "minHoursToExpiry",
    label: "Min hours to expiry",
    tooltip: "Skip setups expiring sooner — too little time for convergence.",
    step: "1",
  },
  {
    key: "maxDaysToExpiry",
    label: "Max days to expiry",
    tooltip: "Skip setups expiring later — carry cost dominates the edge.",
    step: "1",
  },
  {
    key: "pollIntervalSecs",
    label: "Poll interval (s)",
    tooltip: "How often the bot scans for entries.",
    step: "1",
  },
  {
    key: "maxOpenPositions",
    label: "Max open positions",
    tooltip: "Cap on simultaneously managed parity positions.",
    step: "1",
  },
];

const BOT_INPUT_CLS =
  "w-32 shrink-0 px-3 py-2 bg-surface border border-surface-border rounded-lg text-slate-900 font-bold text-xs focus:outline-none focus:ring-2 focus:ring-accent/20 focus:border-accent transition-all h-[38px] font-mono tabular-nums shadow-2xs";

/** Per-substrategy bot configurator — edits scan/entry profile and the
 *  bot's loop parameters. Mode is owned by the lifecycle buttons, never
 *  silently flipped here. */
export const OptionsBotConfigModal: React.FC<{
  cfg: OptionsBotConfigDto;
  name: string;
  onSave: (cfg: OptionsBotConfigDto) => Promise<void>;
  onClose: () => void;
}> = ({ cfg, name, onSave, onClose }) => {
  const [draft, setDraft] = useState<OptionsBotConfigDto>(cfg);
  const [error, setError] = useState<string | null>(null);
  const saveBtn = useSaveButton();

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  const set = <K extends keyof OptionsBotConfigDto>(k: K, v: OptionsBotConfigDto[K]) => {
    setError(null);
    setDraft((d) => ({ ...d, [k]: v }));
  };

  const handleResetDefaults = () => {
    setError(null);
    setDraft((d) => ({ ...defaultOptionsBotConfig(d.substrategyId), mode: d.mode }));
  };

  const handleSave = async () => {
    setError(null);
    const ok = await saveBtn.run(async () => {
      try {
        await onSave(draft);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        throw e; // lands the save button on "Failed!"
      }
    });
    if (ok) setTimeout(onClose, 400);
  };

  return (
    <div
      className="fixed inset-0 z-50 bg-slate-900/50 backdrop-blur-sm flex items-center justify-center p-3 sm:p-6 select-none animate-in fade-in duration-150"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="bg-surface border border-surface-border rounded-2xl w-[520px] max-w-[96vw] max-h-[92vh] flex flex-col shadow-2xl overflow-hidden animate-in zoom-in-95 duration-150">
        <div className="flex items-center justify-between px-6 py-3.5 border-b border-surface-border bg-surface shrink-0">
          <div className="flex items-center space-x-3">
            <div className="w-8 h-8 rounded-lg bg-accent-dim border border-accent-line flex items-center justify-center text-accent-strong shadow-2xs">
              <SlidersHorizontal className="w-4 h-4" />
            </div>
            <div>
              <h2 className="text-sm font-bold font-sans tracking-tight text-slate-900 leading-tight">
                {name} Bot Configurator
              </h2>
              <p className="text-[11px] text-muted font-sans">
                Scan and entry profile for this substrategy — Start/Pause are on the page ribbon
              </p>
            </div>
          </div>
        </div>

        <div className="flex-1 overflow-y-auto px-6 py-5 space-y-4">
          {BOT_FIELD_DEFS.map((f) => (
            <div key={f.key} className="flex items-center justify-between gap-4">
              <label
                className="text-xs text-slate-700 font-medium flex items-center font-sans"
                title={f.tooltip}
              >
                {f.label}
                <CircleHelp className="w-3 h-3 ml-1.5 text-slate-400" aria-hidden />
              </label>
              <input
                type="number"
                step={f.step ?? "any"}
                className={BOT_INPUT_CLS}
                value={Number(draft[f.key])}
                onChange={(e) => {
                  const n = Number(e.target.value);
                  if (Number.isFinite(n)) set(f.key, n as never);
                }}
              />
            </div>
          ))}
          {error && (
            <div className="p-2.5 rounded-lg border border-loss-border text-loss text-xs flex items-center gap-2">
              <AlertCircle className="w-4 h-4 shrink-0" />
              {error}
            </div>
          )}
        </div>

        <div className="flex items-center justify-between gap-3 px-6 py-3.5 border-t border-surface-border bg-surface shrink-0">
          <button
            type="button"
            onClick={handleResetDefaults}
            className="text-xs font-sans font-medium text-slate-500 hover:text-slate-700 cursor-pointer"
          >
            Reset defaults
          </button>
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={onClose}
              className="btn-secondary !text-xs !py-2 !px-3 font-sans cursor-pointer"
            >
              Cancel
            </button>
            <SaveButton state={saveBtn.state} onClick={handleSave} />
          </div>
        </div>
      </div>
    </div>
  );
};

export default OptionsArbitrage;
