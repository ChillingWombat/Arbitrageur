import React, { useCallback, useMemo, useState } from "react";
import {
  Option,
  RefreshCw,
  AlertCircle,
  CheckCircle2,
  Clock3,
  ChevronRight,
  CircleHelp,
} from "lucide-react";
import { api } from "../services/api";
import type {
  OptionsParityOpportunityDto,
  OptionsParityExecutionParams,
  ExecutionLogEntry,
} from "../types";
import { EmptyState } from "./EmptyState";
import { ExecuteButton } from "./ExecuteButton";
import { ConfirmActionModal } from "./ConfirmActionModal";
import { DetailField, DetailSection } from "./DetailBits";
import { TickerLogMonitor } from "./TickerLogMonitor";

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
    universe: "European index & equity options",
    status: "pending",
    requires: "IBKR options (or Deribit multi-leg books)",
  },
  {
    id: "jelly_roll",
    logStrategyId: "options_jelly_roll",
    name: "Jelly Roll",
    tagline: "Calendar box — term-structure mispricing",
    description:
      "Two box spreads across different expiries — a calendar box. Arbitrage when the implied term structure of rates between expiries diverges from actual funding and carry.",
    family: "Rate Structures",
    universe: "Index & equity options across expiries",
    status: "pending",
    requires: "IBKR options chain depth across expiries",
  },
  {
    id: "convexity",
    logStrategyId: "options_convexity",
    name: "Convexity Boundary",
    tagline: "Butterfly/condor upper-bound violations",
    description:
      "A butterfly or condor's price can never exceed its maximum payoff bounds. A book quoting above the convexity upper bound is a locked-in arbitrage sell.",
    family: "Boundary & Convexity",
    universe: "Any listed option chain with a wing quote",
    status: "pending",
    requires: "multi-strike book snapshot (Deribit or IBKR)",
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

/** Substrategy id an opportunity belongs to — the parity scanner is the
 *  only emitter today, so `opar-*` ids (and anything without an explicit
 *  tag) map to `parity`. Future scanners tag their own id on the DTO. */
export const substrategyOfOpp = (_o: OptionsParityOpportunityDto): string => "parity";

/** Filter scan results by the rail selection ("all" passes everything). */
export const filterOptionsOpps = <T extends OptionsParityOpportunityDto>(
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
                        <span className="text-[9px] font-bold text-profit">LIVE</span>
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
}) => {
  const [selected, setSelected] = useState<string>("all");
  const [dirFilter, setDirFilter] = useState<"all" | "conversion" | "reversal">("all");
  const [opps, setOpps] = useState<OptionsParityOpportunityDto[]>([]);
  const [scanning, setScanning] = useState(false);
  const [scanned, setScanned] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [executing, setExecuting] = useState<string | null>(null);
  const [executed, setExecuted] = useState<Set<string>>(new Set());
  const [message, setMessage] = useState<string | null>(null);
  const [confirmOpp, setConfirmOpp] = useState<OptionsParityOpportunityDto | null>(null);

  const selDef = OPTIONS_SUBSTRATEGIES.find((s) => s.id === selected);
  const parityVisible = selected === "all" || selected === "parity";

  const filteredOpps = useMemo(
    () =>
      filterOptionsOpps(opps, selected).filter(
        (o) => dirFilter === "all" || o.direction === dirFilter,
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
  const executableCount = filteredOpps.filter((o) => o.is_executable).length;
  const bestEdge = filteredOpps.reduce(
    (m, o) => Math.max(m, o.net_edge_usd),
    -Infinity,
  );
  const realizedPnl = filteredLogs.reduce((acc, l) => acc + (l.net_profit || 0), 0);

  const scan = useCallback(async () => {
    setScanning(true);
    setError(null);
    setMessage(null);
    try {
      const settled = await Promise.allSettled(
        SCAN_CURRENCIES.map((currency) =>
          api.scanOptionsParity({
            currency,
            qty_base: SCAN_QTY_BASE,
            min_edge_bps: MIN_EDGE_BPS,
          }),
        ),
      );
      const res = settled.flatMap((s) => (s.status === "fulfilled" ? s.value : []));
      const rejected = settled.filter((s) => s.status === "rejected");
      if (rejected.length === settled.length) {
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
  }, []);

  const runExecute = useCallback(
    async (opp: OptionsParityOpportunityDto) => {
      setExecuting(opp.id);
      setError(null);
      setMessage(null);
      try {
        const params: OptionsParityExecutionParams = {
          id: opp.id,
          currency: opp.currency,
          qty_base: opp.qty_base_suggested,
          min_edge_bps: MIN_EDGE_BPS,
        };
        const res = await api.executeOptionsParity(params);
        setExecuted((s) => new Set(s).add(opp.id));
        setMessage(res.message);
        await onRefreshAll();
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setExecuting(null);
        setConfirmOpp(null);
      }
    },
    [onRefreshAll],
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
            read-only params left, primary action right. */}
        <div className="ribbon-container">
          <div className="flex flex-wrap items-center gap-2 text-xs font-mono">
            <span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1">
              Options Strategies
            </span>
            {parityVisible && (
              <>
                <div className="ribbon-pill" title="Net-edge hurdle for reported setups">
                  <span className="text-muted">Min Edge</span>
                  <span className="font-bold text-slate-900 tabular-nums">{MIN_EDGE_BPS}</span>
                  <span className="text-muted">BPS</span>
                </div>
              </>
            )}
            {isPaused && <span className="badge-status-warn">Engine Paused</span>}
            {isLive && (
              <span className="text-xs text-amber-400 border border-amber-500/40 rounded px-1.5 py-0.5 font-sans">
                LIVE — manual executes only
              </span>
            )}
          </div>
          <div className="flex items-center space-x-3 shrink-0">
            {parityVisible && (
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

        {/* Venue capability warning — parity scanning is the only live
            scanner today and it requires an options venue. */}
        {parityVisible && !optionsVenueAvailable && (
          <div className="p-2.5 rounded-lg border border-amber-500/40 bg-amber-500/10 text-amber-300 text-xs flex items-center gap-2">
            <AlertCircle className="w-4 h-4 shrink-0" />
            <span>
              No options-capable venue is enabled — parity scanning currently
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
                dirFilter === "all"
                  ? "No parity edge right now"
                  : `No ${dirFilter} edge right now`
              }
              hint="Synthetic forward ≈ funding-adjusted perp within fees on every quoted strike/expiry pair."
            />
          )}

          {filteredOpps.map((o) => (
            <OptionsParityCard
              key={o.id}
              opp={o}
              executing={executing === o.id}
              executed={executed.has(o.id)}
              disabled={!o.is_executable || isPaused || scanning}
              onExecute={() => setConfirmOpp(o)}
            />
          ))}
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
            ? {
                title: `Execute ${confirmOpp.direction} on Deribit?`,
                subtitle: isLive ? "Live funds at risk" : "Testnet dispatch",
                body: `This dispatches three bounded IOC orders on Deribit: ${confirmOpp.call_leg.action} ${confirmOpp.call_leg.instrument}, ${confirmOpp.put_leg.action} ${confirmOpp.put_leg.instrument}, ${confirmOpp.perp_leg.action} ${confirmOpp.perp_leg.instrument}. A partially-filled cycle leaves residual exposure that must be flattened manually.`,
                confirmLabel: "Execute",
              }
            : null
        }
        onConfirm={() => (confirmOpp ? runExecute(confirmOpp) : undefined)}
        onCancel={() => setConfirmOpp(null)}
        onError={(m) => setError(m)}
      />
    </div>
  );
};

export default OptionsArbitrage;
