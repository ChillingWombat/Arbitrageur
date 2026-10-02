import React, { useEffect, useMemo, useState } from "react";
import { Search, X, ChevronRight, Radar, AlertTriangle, CheckCircle2, Clock } from "lucide-react";
import { api } from "../services/api";
import { VenueLogo } from "./VenueLogo";
import { EmptyState } from "./EmptyState";
import { InfoTooltip } from "./InfoTooltip";
import { DetailField, DetailSection } from "./DetailBits";
import { TradedOpportunityDetailModal } from "./TradedOpportunitiesGadget";
import { statusChip } from "./AtomicTransactionsTable";
import type {
  TradedOpportunityDto,
  MonitorSignalEventDto,
  FundingOpportunityDto,
  ExecutionLogEntry,
} from "../types";

export type OpportunityStrategyKind = "futures" | "radar" | "funding" | "all";
export type OpportunityTradeStatusFilter = "all" | "traded" | "not_traded";

export interface UnifiedOpportunityItem {
  id: string;
  kind: "futures" | "funding";
  tradeStatus: "traded" | "not_traded";
  timestamp: number;
  symbol: string;
  venueBuy: string;
  venueSell: string;
  edgeDisplay?: string;
  edgeValue?: number | null;
  subEdgeDisplay?: string;
  winProbDisplay?: string;
  statusLabel: string;
  statusBadgeCls: string;
  reasonOrOutcome: string;
  realizedPnlUsd?: number | null;
  realizedPnlBps?: number | null;
  expectedNetUsd?: number | null;
  tradedDetail?: TradedOpportunityDto;
  monitorEventDetail?: MonitorSignalEventDto;
  fundingOppDetail?: FundingOpportunityDto;
  rawJson?: string | null;
}

export interface OpportunityHistoryGadgetProps {
  /** Strategy scope: "futures" (or "radar") for Spread Radar, "funding" for Funding Carry, or "all" */
  strategy?: OpportunityStrategyKind;
  /** Scoped symbol (empty for all tickers across the universe) */
  symbol?: string;
  /** Polling interval in ms */
  pollMs?: number;
  /** Scanned funding opportunities passed from FundingRadar */
  fundingOpps?: FundingOpportunityDto[];
  /** Alias for scanned funding opportunities */
  scannedOpps?: FundingOpportunityDto[];
  /** Execution log entries */
  executionLogs?: ExecutionLogEntry[];
  /** Traded opportunities pre-loaded or filtered by parent */
  tradedOpps?: TradedOpportunityDto[];
  /** Execution logs specific to radar / futures */
  radarLogs?: ExecutionLogEntry[];
}

const DEFAULT_POLL_MS = 5_000;
const PAGE_LIMIT = 500;

export const fmtTs = (ms: number): string => {
  if (!Number.isFinite(ms) || ms <= 0) return "—";
  const d = new Date(ms);
  const today = new Date();
  const sameDay = d.toDateString() === today.toDateString();
  const time = d.toLocaleTimeString(undefined, { hour12: false });
  return sameDay
    ? time
    : `${d.toLocaleDateString(undefined, { month: "short", day: "numeric" })} ${time}`;
};

export const pnlTone = (v: number | null | undefined): string => {
  if (v == null || !Number.isFinite(v) || Math.abs(v) < 0.005) {
    return "text-slate-600";
  }
  return v > 0 ? "text-profit" : "text-loss";
};

export interface ParsedEconomics {
  skipReason?: string | null;
  pWin?: number | null;
  expectedProfitBps?: number | null;
  expectedProfitUsd?: number | null;
  optimalSizeUsd?: number | null;
  breakevenSizeUsd?: number | null;
  deviationBps?: number | null;
  baselineBps?: number | null;
  currentEdgeBps?: number | null;
  expectedConvergeSecs?: number | null;
  sizeUsd?: number | null;
  frictionBandBps?: number | null;
  friction_bps?: number | null;
  fee_bps?: number | null;
  exit_est_bps?: number | null;
  execution_mode?: string | null;
  [key: string]: unknown;
}

export function parseEconomics(jsonStr: string | null | undefined): ParsedEconomics | null {
  if (!jsonStr) return null;
  try {
    return JSON.parse(jsonStr) as ParsedEconomics;
  } catch {
    return null;
  }
}

/** Detail modal for an untraded opportunity — surfaces the exact reason
 *  (bot skip, gate filter, execution blocked, scan prints) alongside model beliefs. */
export const UntradedOpportunityModal: React.FC<{
  item: UnifiedOpportunityItem;
  onClose: () => void;
}> = ({ item, onClose }) => {
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  const economics = parseEconomics(item.rawJson);
  const isSkipOrBlocked =
    item.statusLabel.includes("SKIP") ||
    item.statusLabel.includes("BLOCK") ||
    item.statusLabel.includes("FAIL");

  return (
    <div
      className="fixed inset-0 z-50 bg-slate-900/50 backdrop-blur-sm flex items-center justify-center p-3 sm:p-6 select-none"
      role="dialog"
      aria-modal="true"
      aria-label={`Untraded Opportunity Audit — ${item.symbol}`}
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="bg-surface border border-surface-border rounded-2xl w-[860px] max-w-[96vw] max-h-[90vh] flex flex-col shadow-2xl overflow-hidden">
        {/* Header */}
        <div className="flex items-center justify-between px-6 py-3.5 border-b border-surface-border bg-surface shrink-0">
          <div className="flex items-center gap-2 min-w-0 flex-wrap">
            <span className="px-2 py-0.5 rounded-md text-xs font-sans font-semibold bg-warn-dim text-warn-strong border border-warn-border">
              Not Traded
            </span>
            <span className="text-sm font-bold font-sans text-slate-900">{item.symbol}</span>
            <span className="text-xs font-sans text-slate-500">
              {item.kind === "funding" ? "Funding Carry" : "Spread Radar"}
            </span>
            <span className={`px-2 py-0.5 rounded-md text-[11px] font-mono font-bold border ${item.statusBadgeCls}`}>
              {item.statusLabel}
            </span>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="p-1.5 rounded-lg text-muted hover:text-slate-700 hover:bg-slate-100 transition cursor-pointer"
            title="Close details (ESC)"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        {/* Body */}
        <div className="flex-1 overflow-y-auto p-5 space-y-4 font-sans">
          {/* Reason Callout */}
          <div className="rounded-xl border border-warn-border bg-warn-dim/30 p-3.5 flex items-start gap-3">
            <AlertTriangle className="w-4 h-4 text-warn-strong shrink-0 mt-0.5" />
            <div className="space-y-1">
              <div className="text-xs font-bold text-warn-strong uppercase tracking-wider">
                {isSkipOrBlocked ? "Reason Not Traded / Blocked" : "Detection / Lifecycle Note"}
              </div>
              <div className="text-xs font-mono text-slate-800 break-words leading-relaxed">
                {economics?.skipReason || item.reasonOrOutcome || "Opportunity was evaluated or detected without dispatch."}
              </div>
            </div>
          </div>

          {/* Opportunity Overview */}
          <DetailSection title="Opportunity & Route">
            <DetailField label="Symbol" value={item.symbol} />
            <DetailField
              label="Route"
              value={
                <span className="inline-flex items-center gap-1.5">
                  <VenueLogo venueId={item.venueBuy} name={item.venueBuy} size={13} />
                  <span>{item.venueBuy}</span>
                  <span className="text-muted">{item.kind === "funding" ? "long ↔" : "→"}</span>
                  <VenueLogo venueId={item.venueSell} name={item.venueSell} size={13} />
                  <span>{item.venueSell}</span>
                  {item.kind === "funding" && <span className="text-muted">short</span>}
                </span>
              }
            />
            <DetailField label="Recorded Time" value={fmtTs(item.timestamp)} />
            <DetailField label="Decision Status" value={item.statusLabel} />
            {item.monitorEventDetail && (
              <DetailField
                label="Lifecycle Transition"
                value={`${item.monitorEventDetail.oldStatus ?? "idle"} → ${item.monitorEventDetail.newStatus}`}
              />
            )}
            {item.monitorEventDetail && (
              <DetailField label="Trigger Type" value={item.monitorEventDetail.trigger} />
            )}
          </DetailSection>

          {/* Model & Market Snapshot */}
          {item.kind === "futures" ? (
            <DetailSection title="Spread Radar Belief Snapshot">
              <DetailField
                label="Current Edge"
                value={item.edgeDisplay || "—"}
                tip="Gross spread edge vs. the rolling baseline at detection time, in basis points."
              />
              <DetailField
                label="Deviation"
                value={
                  economics?.deviationBps != null
                    ? `${economics.deviationBps >= 0 ? "+" : ""}${economics.deviationBps.toFixed(1)} bps`
                    : item.subEdgeDisplay || "—"
                }
                tip="Signed deviation of current spread from baseline — what the strategy trades on."
              />
              <DetailField
                label="Rolling Baseline"
                value={economics?.baselineBps != null ? `${economics.baselineBps.toFixed(1)} bps` : "—"}
                tip="Rolling baseline mean spread between the venue pair."
              />
              <DetailField
                label="Mean-Reversion P(Win)"
                value={item.winProbDisplay || (economics?.pWin != null ? `${(economics.pWin * 100).toFixed(0)}%` : "—")}
                tip="Estimated win probability under the mean-reversion model."
              />
              <DetailField
                label="Expected Net Profit"
                value={
                  economics?.expectedProfitUsd != null
                    ? `$${economics.expectedProfitUsd.toFixed(2)}`
                    : economics?.expectedProfitBps != null
                      ? `${economics.expectedProfitBps.toFixed(1)} bps`
                      : "—"
                }
              />
              <DetailField
                label="Target Size"
                value={economics?.sizeUsd != null ? `$${economics.sizeUsd.toFixed(0)}` : "—"}
              />
              <DetailField
                label="Optimal Capacity"
                value={economics?.optimalSizeUsd != null ? `$${economics.optimalSizeUsd.toLocaleString(undefined, { maximumFractionDigits: 0 })}` : "—"}
              />
              <DetailField
                label="Expected Convergence"
                value={
                  economics?.expectedConvergeSecs != null
                    ? `${Math.round(economics.expectedConvergeSecs)}s (~${(economics.expectedConvergeSecs / 60).toFixed(0)}m)`
                    : "—"
                }
              />
            </DetailSection>
          ) : (
            <DetailSection title="Funding Carry Snapshot">
              <DetailField
                label="Net APR"
                value={item.edgeDisplay || (item.fundingOppDetail?.net_apr_pct != null ? `${item.fundingOppDetail.net_apr_pct.toFixed(2)}%` : "—")}
              />
              <DetailField
                label="8h Net Rate"
                value={
                  item.fundingOppDetail?.net_rate_8h_pct != null
                    ? `${(item.fundingOppDetail.net_rate_8h_pct * 100).toFixed(3)}%`
                    : item.subEdgeDisplay || "—"
                }
              />
              <DetailField
                label="Round-Trip Fees"
                value={item.fundingOppDetail?.round_trip_fee_bps != null ? `${item.fundingOppDetail.round_trip_fee_bps.toFixed(1)} bps` : "—"}
              />
              <DetailField
                label="Breakeven Horizon"
                value={item.fundingOppDetail?.break_even_hours != null ? `${item.fundingOppDetail.break_even_hours.toFixed(1)}h` : "—"}
              />
              <DetailField
                label="Expected Return"
                value={item.fundingOppDetail?.expected_return_usd != null ? `$${item.fundingOppDetail.expected_return_usd.toFixed(2)}` : "—"}
              />
              <DetailField
                label="Persistence P(gap)"
                value={item.fundingOppDetail?.persistence_probability != null ? `${(item.fundingOppDetail.persistence_probability * 100).toFixed(0)}%` : "—"}
              />
            </DetailSection>
          )}

          {/* Raw JSON details if present */}
          {item.rawJson && (
            <details className="group rounded-xl border border-surface-border bg-surface-raised/30 p-3">
              <summary className="text-xs font-mono font-semibold text-slate-600 hover:text-slate-900 cursor-pointer select-none">
                Raw Point-in-Time Engine Belief Snapshot (JSON)
              </summary>
              <pre className="text-[11px] font-mono p-3 bg-slate-900 text-slate-100 rounded-lg overflow-x-auto mt-2.5 max-h-56">
                {(() => {
                  try {
                    return JSON.stringify(JSON.parse(item.rawJson), null, 2);
                  } catch {
                    return item.rawJson;
                  }
                })()}
              </pre>
            </details>
          )}
        </div>
      </div>
    </div>
  );
};

export const OpportunityHistoryGadget: React.FC<OpportunityHistoryGadgetProps> = ({
  strategy = "futures",
  symbol = "",
  pollMs = DEFAULT_POLL_MS,
  fundingOpps,
  scannedOpps,
  executionLogs,
  tradedOpps: propTradedOpps,
  radarLogs,
}) => {
  // Normalize strategy: "radar" is equivalent to "futures"
  const normStrategy: "futures" | "funding" | "all" =
    strategy === "radar" || strategy === "futures"
      ? "futures"
      : strategy === "funding"
        ? "funding"
        : "all";

  // Combine and normalize execution logs
  const allLogs = useMemo<ExecutionLogEntry[]>(
    () => [...(executionLogs ?? []), ...(radarLogs ?? [])],
    [executionLogs, radarLogs],
  );

  // Scanned funding opportunities passed by parent
  const activeFundingScans = useMemo<FundingOpportunityDto[]>(
    () => fundingOpps ?? scannedOpps ?? [],
    [fundingOpps, scannedOpps],
  );

  const [monitorEvents, setMonitorEvents] = useState<MonitorSignalEventDto[]>([]);
  const [fetchedTradedOpps, setFetchedTradedOpps] = useState<TradedOpportunityDto[] | null>(null);
  const [loaded, setLoaded] = useState(
    () => propTradedOpps != null || activeFundingScans.length > 0 || allLogs.length > 0,
  );
  const [error, setError] = useState<string | null>(null);
  const [partialError, setPartialError] = useState<string | null>(null);

  // Switch filter: "all" | "traded" | "not_traded"
  const [tradeFilter, setTradeFilter] = useState<OpportunityTradeStatusFilter>("all");
  const [search, setSearch] = useState("");
  const [venueFilter, setVenueFilter] = useState("all");
  const [actionableOnly, setActionableOnly] = useState(true);

  // Detail Modal selection
  const [selectedTraded, setSelectedTraded] = useState<TradedOpportunityDto | null>(null);
  const [selectedUntraded, setSelectedUntraded] = useState<UnifiedOpportunityItem | null>(null);

  // Polling data using Promise.allSettled for resilient partial degradation
  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      try {
        const promises: [Promise<MonitorSignalEventDto[]>, Promise<TradedOpportunityDto[]>] = [
          normStrategy === "funding"
            ? Promise.resolve([])
            : api.getMonitorSignalEvents(symbol.trim() ? symbol : undefined, PAGE_LIMIT),
          propTradedOpps
            ? Promise.resolve(propTradedOpps)
            : api.listTradedOpportunities(),
        ];

        const [evsRes, tradesRes] = await Promise.allSettled(promises);
        if (cancelled) return;

        let gotEvents = false;
        let gotTrades = false;

        if (evsRes.status === "fulfilled") {
          setMonitorEvents(evsRes.value);
          gotEvents = true;
        }
        if (tradesRes.status === "fulfilled") {
          setFetchedTradedOpps(tradesRes.value);
          gotTrades = true;
        }

        if (!gotEvents && !gotTrades && !propTradedOpps) {
          const reason =
            tradesRes.status === "rejected"
              ? String(tradesRes.reason)
              : evsRes.status === "rejected"
                ? String(evsRes.reason)
                : "Failed to load opportunity data";
          setError(reason);
        } else {
          setError(null);
          if (evsRes.status === "rejected" || tradesRes.status === "rejected") {
            setPartialError("Some historical signals or trade ledger rows could not be refreshed.");
          } else {
            setPartialError(null);
          }
        }
        setLoaded(true);
      } catch (e) {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : String(e));
        setLoaded(true);
      }
    };

    void load();
    const interval = setInterval(load, pollMs);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [normStrategy, symbol, pollMs, propTradedOpps]);

  // Unified items construction
  const unifiedItems = useMemo<UnifiedOpportunityItem[]>(() => {
    const items: UnifiedOpportunityItem[] = [];

    // --- 1. Process Traded Opportunities ---
    const allTrades = propTradedOpps ?? fetchedTradedOpps ?? [];
    const relevantTrades = allTrades.filter((o) => {
      const s = (o.strategyId || "").toLowerCase();
      if (normStrategy === "futures") {
        return s === "futures" || s === "radar";
      } else if (normStrategy === "funding") {
        return s === "funding" || s === "funding_rate";
      }
      return true;
    });

    for (const t of relevantTrades) {
      if (symbol.trim() && !t.symbol.toLowerCase().includes(symbol.trim().toLowerCase())) {
        continue;
      }
      const chip = statusChip(t.status);
      const isFutures = (t.strategyId || "").toLowerCase().includes("futures") || (t.strategyId || "").toLowerCase().includes("radar");

      const pnlUsd = t.realizedPnlUsd;
      const pnlBps = t.realizedPnlBps;
      let outcomeText = `${t.transactions.length} order(s)`;
      if (pnlUsd != null) {
        outcomeText = `${pnlUsd >= 0 ? "+" : "-"}$${Math.abs(pnlUsd).toFixed(2)}${pnlBps != null ? ` (${pnlBps >= 0 ? "+" : ""}${pnlBps.toFixed(1)} bps)` : ""}`;
      } else if (t.expectedNetUsd != null) {
        outcomeText = `Exp: $${t.expectedNetUsd.toFixed(2)}`;
      }

      items.push({
        id: `traded-${t.signalId}`,
        kind: isFutures ? "futures" : "funding",
        tradeStatus: "traded",
        timestamp: Date.parse(t.createdAt) || Date.now(),
        symbol: t.symbol,
        venueBuy: t.venueLong || "?",
        venueSell: t.venueShort || "?",
        edgeDisplay:
          t.entrySpreadBps != null
            ? `${t.entrySpreadBps >= 0 ? "+" : ""}${t.entrySpreadBps.toFixed(1)} bps`
            : t.expectedNetUsd != null
              ? `$${t.expectedNetUsd.toFixed(2)}`
              : "—",
        edgeValue: t.entrySpreadBps ?? t.expectedNetUsd,
        subEdgeDisplay:
          t.closeSpreadBps != null
            ? `Exit: ${t.closeSpreadBps >= 0 ? "+" : ""}${t.closeSpreadBps.toFixed(1)} bps`
            : undefined,
        winProbDisplay: undefined,
        statusLabel: chip.label.toUpperCase(),
        statusBadgeCls: chip.cls,
        reasonOrOutcome: outcomeText,
        realizedPnlUsd: pnlUsd,
        realizedPnlBps: pnlBps,
        expectedNetUsd: t.expectedNetUsd,
        tradedDetail: t,
        rawJson: t.note,
      });
    }

    // --- 2. Process Not Traded Opportunities (Spread Radar / Futures) ---
    if (normStrategy === "futures" || normStrategy === "all") {
      // 2a. Persisted monitor events & bot skips
      for (const ev of monitorEvents) {
        if (symbol.trim() && !ev.symbol.toLowerCase().includes(symbol.trim().toLowerCase())) {
          continue;
        }

        // If it was a bot_enter decision, it maps to a trade entry (in tradedOpps ledger)
        if (ev.trigger === "bot_decision" && ev.newStatus === "bot_enter") {
          continue;
        }

        const isSkip = ev.trigger === "bot_decision" && ev.newStatus === "bot_skip";
        const isActionableTrigger = ev.trigger === "transition" && (ev.newStatus === "triggered" || ev.newStatus === "ready");
        const isActionable = isSkip || isActionableTrigger;

        if (actionableOnly && !isActionable) {
          continue;
        }

        const eco = parseEconomics(ev.economicsJson);
        const skipReason = eco?.skipReason || (ev.trigger === "bot_decision" ? "Bot skipped entry" : null);

        let statusLabel = ev.newStatus.toUpperCase();
        let badgeCls = "bg-slate-100 text-slate-600 border-slate-200";

        if (isSkip) {
          statusLabel = "SKIPPED";
          badgeCls = "bg-warn-dim text-warn-strong border-warn-border";
        } else if (ev.newStatus === "ready") {
          statusLabel = "READY";
          badgeCls = "bg-profit-dim text-profit border-profit-border";
        } else if (ev.newStatus === "triggered") {
          statusLabel = "TRIGGERED";
          badgeCls = "bg-blue-50 text-blue-700 border-blue-200";
        }

        items.push({
          id: `mon-${ev.id}`,
          kind: "futures",
          tradeStatus: "not_traded",
          timestamp: ev.tsMs,
          symbol: ev.symbol,
          venueBuy: ev.buyVenue,
          venueSell: ev.sellVenue,
          edgeDisplay: ev.edgeBps != null ? `${ev.edgeBps >= 0 ? "+" : ""}${ev.edgeBps.toFixed(1)} bps` : "—",
          edgeValue: ev.edgeBps,
          subEdgeDisplay: ev.devBps != null ? `Dev: ${ev.devBps >= 0 ? "+" : ""}${ev.devBps.toFixed(1)} bps` : undefined,
          winProbDisplay: ev.pWin != null ? `${(ev.pWin * 100).toFixed(0)}%` : undefined,
          statusLabel,
          statusBadgeCls: badgeCls,
          reasonOrOutcome:
            skipReason ||
            (ev.trigger === "transition"
              ? `Signal transition: ${ev.oldStatus ?? "idle"} → ${ev.newStatus}`
              : `Heartbeat (${ev.newStatus})`),
          monitorEventDetail: ev,
          rawJson: ev.economicsJson,
        });
      }

      // 2b. Blocked or failed futures execution logs
      const failedFuturesLogs = allLogs.filter((l) => {
        const sid = (l.strategy_id || "").toLowerCase();
        const isFutures = sid === "radar" || sid === "futures" || (l.trade_type || "").toLowerCase().includes("spread");
        const isBlocked =
          l.success === false ||
          (l.note || "").toLowerCase().includes("blocked") ||
          (l.note || "").toLowerCase().includes("failed") ||
          (l.leg_a_status || "").toLowerCase().includes("fail") ||
          (l.leg_b_status || "").toLowerCase().includes("fail");
        return isFutures && isBlocked;
      });

      for (let idx = 0; idx < failedFuturesLogs.length; idx++) {
        const log = failedFuturesLogs[idx];
        if (symbol.trim() && !log.symbol.toLowerCase().includes(symbol.trim().toLowerCase())) {
          continue;
        }
        items.push({
          id: `futlog-${log.id || `${log.timestamp}-${log.symbol}-${idx}`}`,
          kind: "futures",
          tradeStatus: "not_traded",
          timestamp: Date.parse(log.timestamp) || Date.now(),
          symbol: log.symbol,
          venueBuy: log.venue_a || "?",
          venueSell: log.venue_b || "?",
          edgeDisplay: log.trigger_spread_bps != null ? `${log.trigger_spread_bps.toFixed(1)} bps` : "—",
          edgeValue: log.trigger_spread_bps,
          subEdgeDisplay: log.direction || undefined,
          statusLabel: "BLOCKED",
          statusBadgeCls: "bg-loss-dim text-loss border-loss-border",
          reasonOrOutcome: log.note || "Order execution failed or blocked",
          rawJson: JSON.stringify(log),
        });
      }
    }

    // --- 3. Process Not Traded Opportunities (Funding Rate) ---
    if (normStrategy === "funding" || normStrategy === "all") {
      // 3a. Blocked execution logs
      const fLogs = allLogs.filter((l) => {
        const sid = (l.strategy_id || "").toLowerCase();
        const isFunding = sid === "funding" || sid === "funding_rate" || (l.trade_type || "").toLowerCase().includes("funding");
        const isBlocked =
          l.success === false ||
          (l.note || "").toLowerCase().includes("blocked") ||
          (l.note || "").toLowerCase().includes("failed") ||
          (l.leg_a_status || "").toLowerCase().includes("fail") ||
          (l.leg_b_status || "").toLowerCase().includes("fail");
        return isFunding && isBlocked;
      });

      for (let idx = 0; idx < fLogs.length; idx++) {
        const log = fLogs[idx];
        if (symbol.trim() && !log.symbol.toLowerCase().includes(symbol.trim().toLowerCase())) {
          continue;
        }
        items.push({
          id: `fundlog-${log.id || `${log.timestamp}-${log.symbol}-${idx}`}`,
          kind: "funding",
          tradeStatus: "not_traded",
          timestamp: Date.parse(log.timestamp) || Date.now(),
          symbol: log.symbol,
          venueBuy: log.venue_a || "?",
          venueSell: log.venue_b || "?",
          edgeDisplay: log.trigger_spread_bps != null ? `${log.trigger_spread_bps.toFixed(1)} bps` : "—",
          edgeValue: log.trigger_spread_bps,
          subEdgeDisplay: log.direction || undefined,
          statusLabel: "BLOCKED",
          statusBadgeCls: "bg-loss-dim text-loss border-loss-border",
          reasonOrOutcome: log.note || "Execution blocked",
          rawJson: JSON.stringify(log),
        });
      }

      // 3b. Active scanned funding opportunities not yet traded
      for (const opp of activeFundingScans) {
        if (symbol.trim() && !opp.symbol.toLowerCase().includes(symbol.trim().toLowerCase())) {
          continue;
        }

        const isAboveHurdle = opp.net_apr_pct >= 8.0;
        if (actionableOnly && !isAboveHurdle) {
          continue;
        }

        items.push({
          id: `scanned-${opp.id}`,
          kind: "funding",
          tradeStatus: "not_traded",
          timestamp: Date.now(),
          symbol: opp.symbol,
          venueBuy: opp.long_venue,
          venueSell: opp.short_venue,
          edgeDisplay: `${opp.net_apr_pct >= 0 ? "+" : ""}${opp.net_apr_pct.toFixed(2)}% APR`,
          edgeValue: opp.net_apr_pct,
          subEdgeDisplay: `${(opp.net_rate_8h_pct * 100).toFixed(3)}%/8h`,
          winProbDisplay:
            opp.persistence_probability != null
              ? `${(opp.persistence_probability * 100).toFixed(0)}% P(persist)`
              : opp.backtest_win_rate != null
                ? `${(opp.backtest_win_rate * 100).toFixed(0)}% win`
                : undefined,
          statusLabel: isAboveHurdle ? "ACTIVE SCAN" : "BELOW HURDLE",
          statusBadgeCls: isAboveHurdle
            ? "bg-profit-dim text-profit border-profit-border"
            : "bg-slate-100 text-slate-600 border-slate-200",
          reasonOrOutcome:
            opp.expected_return_usd != null
              ? `Exp net: $${opp.expected_return_usd.toFixed(2)} (${opp.break_even_hours.toFixed(1)}h b/e)`
              : `Round-trip fee: ${opp.round_trip_fee_bps.toFixed(1)} bps`,
          fundingOppDetail: opp,
          rawJson: JSON.stringify(opp),
        });
      }
    }

    // Sort newest first
    items.sort((a, b) => b.timestamp - a.timestamp);
    return items;
  }, [
    propTradedOpps,
    fetchedTradedOpps,
    monitorEvents,
    allLogs,
    activeFundingScans,
    normStrategy,
    symbol,
    actionableOnly,
  ]);

  // Available venues for dropdown filter
  const availableVenues = useMemo(() => {
    const s = new Set<string>();
    for (const item of unifiedItems) {
      if (item.venueBuy && item.venueBuy !== "?") s.add(item.venueBuy.toLowerCase());
      if (item.venueSell && item.venueSell !== "?") s.add(item.venueSell.toLowerCase());
    }
    return Array.from(s).sort();
  }, [unifiedItems]);

  // Filter items by search query and venue dropdown FIRST
  const venueAndSearchFiltered = useMemo(() => {
    let rows = unifiedItems;

    // Venue filter
    if (venueFilter !== "all") {
      const v = venueFilter.toLowerCase();
      rows = rows.filter((r) => r.venueBuy.toLowerCase() === v || r.venueSell.toLowerCase() === v);
    }

    // Search query: symbol, venue, status, reason, or kind
    const q = search.trim().toLowerCase();
    if (q) {
      rows = rows.filter(
        (r) =>
          r.symbol.toLowerCase().includes(q) ||
          r.venueBuy.toLowerCase().includes(q) ||
          r.venueSell.toLowerCase().includes(q) ||
          r.statusLabel.toLowerCase().includes(q) ||
          r.reasonOrOutcome.toLowerCase().includes(q) ||
          r.kind.toLowerCase().includes(q),
      );
    }

    return rows;
  }, [unifiedItems, venueFilter, search]);

  // Aggregate counts for the switch buttons (reflects current search and venue filter scope)
  const counts = useMemo(() => {
    let traded = 0;
    let notTraded = 0;
    for (const item of venueAndSearchFiltered) {
      if (item.tradeStatus === "traded") traded++;
      else notTraded++;
    }
    return { all: venueAndSearchFiltered.length, traded, notTraded };
  }, [venueAndSearchFiltered]);

  // Filtered rows for the table based on the active switch button ("all" | "traded" | "not_traded")
  const filtered = useMemo(() => {
    if (tradeFilter === "traded") {
      return venueAndSearchFiltered.filter((r) => r.tradeStatus === "traded");
    } else if (tradeFilter === "not_traded") {
      return venueAndSearchFiltered.filter((r) => r.tradeStatus === "not_traded");
    }
    return venueAndSearchFiltered;
  }, [venueAndSearchFiltered, tradeFilter]);

  const hasActiveFilters = search.trim() !== "" || venueFilter !== "all" || tradeFilter !== "all";

  const handleRowClick = (item: UnifiedOpportunityItem) => {
    if (item.tradeStatus === "traded" && item.tradedDetail) {
      setSelectedTraded(item.tradedDetail);
    } else {
      setSelectedUntraded(item);
    }
  };

  return (
    <div className="font-sans space-y-2">
      {/* Sticky Gadget Toolbar */}
      <div className="sticky top-0 bg-surface z-20 pb-2 pt-0.5 border-b border-surface-border flex flex-wrap items-center justify-between gap-2">
        {/* Left: Switch Buttons: All / Traded / Not Traded */}
        <div className="flex items-center gap-2 flex-wrap">
          <div className="inline-flex items-center p-0.5 rounded-lg bg-surface-raised border border-surface-border text-xs shrink-0 select-none">
            <button
              type="button"
              onClick={() => setTradeFilter("all")}
              className={`px-3 py-1.5 rounded-md font-semibold transition cursor-pointer flex items-center gap-1.5 ${
                tradeFilter === "all"
                  ? "bg-surface text-slate-900 shadow-2xs border border-surface-border font-bold"
                  : "text-slate-600 hover:text-slate-900"
              }`}
            >
              <span>All</span>
              <span className="px-1.5 py-px rounded-full text-[10px] bg-slate-200/70 text-slate-700 font-mono">
                {counts.all}
              </span>
            </button>
            <button
              type="button"
              onClick={() => setTradeFilter("traded")}
              className={`px-3 py-1.5 rounded-md font-semibold transition cursor-pointer flex items-center gap-1.5 ${
                tradeFilter === "traded"
                  ? "bg-profit-dim text-profit shadow-2xs border border-profit-border font-bold"
                  : "text-slate-600 hover:text-slate-900"
              }`}
            >
              <CheckCircle2 className="w-3.5 h-3.5 text-profit" />
              <span>Traded</span>
              <span className="px-1.5 py-px rounded-full text-[10px] bg-profit-dim text-profit font-mono font-bold">
                {counts.traded}
              </span>
            </button>
            <button
              type="button"
              onClick={() => setTradeFilter("not_traded")}
              className={`px-3 py-1.5 rounded-md font-semibold transition cursor-pointer flex items-center gap-1.5 ${
                tradeFilter === "not_traded"
                  ? "bg-warn-dim text-warn-strong shadow-2xs border border-warn-border font-bold"
                  : "text-slate-600 hover:text-slate-900"
              }`}
            >
              <AlertTriangle className="w-3.5 h-3.5 text-warn-strong" />
              <span>Not Traded</span>
              <span className="px-1.5 py-px rounded-full text-[10px] bg-warn-dim text-warn-strong font-mono font-bold">
                {counts.notTraded}
              </span>
            </button>
          </div>

          {/* Actionable only toggle */}
          <button
            type="button"
            onClick={() => setActionableOnly((v) => !v)}
            className={`px-2.5 py-1.5 rounded-lg border text-[11px] font-semibold transition cursor-pointer shrink-0 ${
              actionableOnly
                ? "bg-accent-dim text-accent-strong border-accent-line"
                : "bg-surface text-slate-600 border-surface-border hover:bg-surface-raised"
            }`}
            title={
              normStrategy === "funding"
                ? "Actionable only shows active scans with net APR >= hurdle (8%), filtering out low-yield prints."
                : "Actionable only shows bot skips and triggered/ready transitions, filtering out routine minute heartbeats."
            }
          >
            Actionable only
          </button>
        </div>

        {/* Right: Search, Venue Filter, and Clear */}
        <div className="flex items-center gap-2 flex-1 min-w-[280px] justify-end flex-wrap">
          {/* Search Input */}
          <div className="relative flex-1 min-w-[140px] max-w-[240px]">
            <Search className="w-3.5 h-3.5 absolute left-2.5 top-1/2 -translate-y-1/2 text-muted" />
            <input
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Search symbol, venue, reason…"
              className="w-full pl-8 pr-6 py-1.5 text-xs bg-surface-raised border border-surface-border rounded-lg text-slate-900 placeholder:text-muted focus:outline-none focus:ring-2 focus:ring-accent/20 focus:border-accent transition font-mono"
            />
            {search && (
              <button
                type="button"
                onClick={() => setSearch("")}
                className="absolute right-2 top-1/2 -translate-y-1/2 text-slate-400 hover:text-slate-700"
                title="Clear search"
              >
                <X className="w-3 h-3" />
              </button>
            )}
          </div>

          {/* Venue Filter Dropdown */}
          <select
            value={venueFilter}
            onChange={(e) => setVenueFilter(e.target.value)}
            className="px-2 py-1.5 bg-surface border border-surface-border rounded-lg text-xs font-mono text-slate-800 focus:outline-none focus:ring-1 focus:ring-accent cursor-pointer"
            aria-label="Filter by venue"
          >
            <option value="all">All Venues</option>
            {availableVenues.map((v) => (
              <option key={v} value={v}>
                {v.toUpperCase()}
              </option>
            ))}
          </select>

          {/* Clear Filters */}
          {hasActiveFilters && (
            <button
              type="button"
              onClick={() => {
                setTradeFilter("all");
                setSearch("");
                setVenueFilter("all");
              }}
              className="px-2 py-1 rounded-md text-[11px] font-mono text-slate-500 hover:text-slate-800 hover:bg-surface-raised transition cursor-pointer"
            >
              Clear
            </button>
          )}

          <span className="inline-flex items-center text-[10px] text-muted whitespace-nowrap ml-1">
            durable audit log
            <InfoTooltip text="Unified opportunity ledger: executed trades, bot skips, blocked orders, and detected signals across all tickers." />
          </span>
        </div>
      </div>

      {/* Non-fatal partial load banner */}
      {partialError && (
        <div className="rounded-lg border border-warn-border bg-warn-dim/40 px-3 py-1.5 text-[11px] font-mono text-warn-strong flex items-center justify-between">
          <span>{partialError}</span>
          <button
            type="button"
            onClick={() => setPartialError(null)}
            className="text-warn-strong hover:opacity-70 ml-2"
          >
            <X className="w-3 h-3" />
          </button>
        </div>
      )}

      {/* Main Table / Body */}
      {!loaded ? (
        <div className="py-12 text-center text-xs text-muted flex items-center justify-center gap-2">
          <Clock className="w-4 h-4 animate-spin text-muted" />
          <span>Loading opportunity history…</span>
        </div>
      ) : error ? (
        <EmptyState
          icon={Radar}
          title="Opportunity history unavailable"
          hint={error}
        />
      ) : filtered.length === 0 ? (
        <EmptyState
          icon={Radar}
          plain
          title={
            search.trim() !== "" || venueFilter !== "all"
              ? "No opportunities match the current filters"
              : tradeFilter === "traded"
                ? "No traded opportunities yet"
                : tradeFilter === "not_traded"
                  ? "No untraded opportunities recorded"
                  : "No opportunities recorded yet"
          }
          hint={
            search.trim() !== "" || venueFilter !== "all"
              ? "Try widening the search or clearing the venue filter."
              : tradeFilter === "traded"
                ? "Once signals dispatch and execute, their atomic transactions and PnL will appear here."
                : tradeFilter === "not_traded"
                  ? "Monitor detections, bot skip decisions, and blocked orders will populate here automatically."
                  : "Monitor detections, bot skip decisions, and executed trades across all tickers will appear here."
          }
        />
      ) : (
        <div className="overflow-x-auto rounded-lg border border-surface-border">
          <table className="w-full text-xs font-mono tabular-nums text-left">
            <thead className="bg-surface-raised border-b border-surface-border text-slate-500 text-[10px] font-semibold uppercase tracking-wider sticky top-0 z-10 select-none">
              <tr>
                <th className="py-2.5 px-3">Time</th>
                <th className="py-2.5 px-3">Lifecycle / Status</th>
                <th className="py-2.5 px-3">Symbol</th>
                <th className="py-2.5 px-3">Route</th>
                <th className="py-2.5 px-3 text-right">
                  {normStrategy === "funding" ? "Net APR" : "Edge"}
                </th>
                <th className="py-2.5 px-3 text-right">
                  {normStrategy === "funding" ? "8h / Persist" : "Dev / P(Win)"}
                </th>
                <th className="py-2.5 px-3">Outcome / Reason</th>
                <th className="py-2.5 px-2 text-right">Details</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-surface-border">
              {filtered.map((item) => {
                const isTraded = item.tradeStatus === "traded";
                const pnlClass = isTraded
                  ? pnlTone(item.realizedPnlUsd)
                  : "text-slate-600";

                return (
                  <tr
                    key={item.id}
                    onClick={() => handleRowClick(item)}
                    className="hover:bg-surface-raised/70 transition cursor-pointer group"
                    title={
                      isTraded
                        ? "Click to view full order trail and realized PnL"
                        : "Click to view why this opportunity was not traded"
                    }
                  >
                    {/* 1. Time */}
                    <td className="py-2 px-3 text-slate-500 whitespace-nowrap text-[11px]">
                      {fmtTs(item.timestamp)}
                    </td>

                    {/* 2. Trade Status & Decision Badge */}
                    <td className="py-2 px-3 whitespace-nowrap">
                      <div className="inline-flex items-center gap-1.5">
                        <span
                          className={`px-1.5 py-px rounded text-[9px] font-bold uppercase tracking-wider ${
                            isTraded
                              ? "bg-profit-dim text-profit border border-profit-border"
                              : "bg-slate-100 text-slate-600 border border-slate-200"
                          }`}
                        >
                          {isTraded ? "TRADED" : "UNTRADED"}
                        </span>
                        <span
                          className={`px-2 py-0.5 rounded-full text-[10px] font-bold border ${item.statusBadgeCls}`}
                        >
                          {item.statusLabel}
                        </span>
                      </div>
                    </td>

                    {/* 3. Symbol */}
                    <td className="py-2 px-3 font-bold text-slate-900 whitespace-nowrap group-hover:underline underline-offset-2">
                      {item.symbol}
                    </td>

                    {/* 4. Route */}
                    <td className="py-2 px-3 whitespace-nowrap">
                      <span className="inline-flex items-center gap-1.5 text-slate-700">
                        <VenueLogo venueId={item.venueBuy} name={item.venueBuy} size={14} />
                        <span className="font-semibold">{item.venueBuy}</span>
                        <span className="text-muted text-[10px]">
                          {item.kind === "funding" ? "↔" : "→"}
                        </span>
                        <VenueLogo venueId={item.venueSell} name={item.venueSell} size={14} />
                        <span className="font-semibold">{item.venueSell}</span>
                      </span>
                    </td>

                    {/* 5. Edge / Net APR */}
                    <td className="py-2 px-3 text-right font-bold text-slate-800 whitespace-nowrap">
                      {item.edgeDisplay || "—"}
                    </td>

                    {/* 6. Sub Edge / Dev / Win% */}
                    <td className="py-2 px-3 text-right text-slate-600 whitespace-nowrap text-[11px]">
                      {item.subEdgeDisplay && <div>{item.subEdgeDisplay}</div>}
                      {item.winProbDisplay && (
                        <div className="text-slate-400 text-[10px]">{item.winProbDisplay}</div>
                      )}
                      {!item.subEdgeDisplay && !item.winProbDisplay && "—"}
                    </td>

                    {/* 7. Outcome / Reason */}
                    <td className="py-2 px-3 text-slate-700 text-xs max-w-md truncate">
                      {isTraded ? (
                        <span className={`font-semibold ${pnlClass}`}>
                          {item.reasonOrOutcome}
                        </span>
                      ) : (
                        <span className="text-slate-600 truncate block" title={item.reasonOrOutcome}>
                          {item.reasonOrOutcome}
                        </span>
                      )}
                    </td>

                    {/* 8. Action Arrow */}
                    <td className="py-2 px-2 text-right whitespace-nowrap">
                      <ChevronRight className="w-4 h-4 text-slate-400 group-hover:text-accent-strong transition inline-block" />
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      {/* Modals */}
      {selectedTraded && (
        <TradedOpportunityDetailModal
          opp={selectedTraded}
          onClose={() => setSelectedTraded(null)}
        />
      )}

      {selectedUntraded && (
        <UntradedOpportunityModal
          item={selectedUntraded}
          onClose={() => setSelectedUntraded(null)}
        />
      )}
    </div>
  );
};

export default OpportunityHistoryGadget;
