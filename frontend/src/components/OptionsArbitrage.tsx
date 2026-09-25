import React, { useCallback, useState } from "react";
import { Option, RefreshCw, AlertCircle, CheckCircle2 } from "lucide-react";
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

interface OptionsArbitrageProps {
  executionLogs: ExecutionLogEntry[];
  onRefreshAll: () => Promise<void>;
  isPaused?: boolean;
  /** Live environment — manual Execute stays available, auto entry is testnet-only. */
  isLive?: boolean;
  /** Deribit is admitted (enabled + credentials) in the active environment. */
  deribitAvailable?: boolean;
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

export const OptionsArbitrage: React.FC<OptionsArbitrageProps> = ({
  onRefreshAll,
  isPaused = false,
  isLive = false,
  deribitAvailable = false,
}) => {
  const [currency, setCurrency] = useState<"BTC" | "ETH">("BTC");
  const [qtyBase, setQtyBase] = useState("0.1");
  const [minEdgeBps, setMinEdgeBps] = useState("5");
  const [opps, setOpps] = useState<OptionsParityOpportunityDto[]>([]);
  const [scanning, setScanning] = useState(false);
  const [scanned, setScanned] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [executing, setExecuting] = useState<string | null>(null);
  const [executed, setExecuted] = useState<Set<string>>(new Set());
  const [message, setMessage] = useState<string | null>(null);
  const [confirmOpp, setConfirmOpp] = useState<OptionsParityOpportunityDto | null>(null);

  const scan = useCallback(async () => {
    setScanning(true);
    setError(null);
    setMessage(null);
    try {
      const qty = parseFloat(qtyBase);
      const res = await api.scanOptionsParity({
        currency,
        qty_base: Number.isFinite(qty) && qty > 0 ? qty : 0.1,
        min_edge_bps: parseFloat(minEdgeBps) || 0,
      });
      setOpps(res);
      setScanned(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setOpps([]);
    } finally {
      setScanning(false);
    }
  }, [currency, qtyBase, minEdgeBps]);

  const runExecute = useCallback(
    async (opp: OptionsParityOpportunityDto) => {
      setExecuting(opp.id);
      setError(null);
      setMessage(null);
      try {
        const params: OptionsParityExecutionParams = {
          id: opp.id,
          currency: opp.currency,
          qty_base: parseFloat(qtyBase) || opp.qty_base_suggested,
          min_edge_bps: parseFloat(minEdgeBps) || 0,
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
    [qtyBase, minEdgeBps, onRefreshAll],
  );

  if (!deribitAvailable) {
    return (
      <div className="p-6">
        <EmptyState
          icon={Option}
          title="Deribit is not enabled"
          hint="Options parity runs on Deribit only. Enable Deribit in Engine Settings and add its Client ID + Secret under Exchange Accounts (test.deribit.com credentials for Testnet mode)."
        />
      </div>
    );
  }

  return (
    <div className="p-4 space-y-4">
      <div className="panel p-4">
        <div className="flex items-center gap-3 flex-wrap">
          <div className="flex items-center gap-2">
            <Option className="w-4 h-4 text-accent" />
            <span className="text-sm font-semibold">Options ↔ Perp Parity</span>
          </div>
          <select
            value={currency}
            onChange={(e) => setCurrency(e.target.value as "BTC" | "ETH")}
            className="px-2 py-1 bg-surface border border-surface-border rounded-md text-xs font-mono"
          >
            <option value="BTC">BTC</option>
            <option value="ETH">ETH</option>
          </select>
          <label className="text-xs text-slate-400">
            Qty ({currency})
            <input
              value={qtyBase}
              onChange={(e) => setQtyBase(e.target.value)}
              className="ml-1 w-20 px-2 py-1 bg-surface border border-surface-border rounded-md text-xs font-mono"
            />
          </label>
          <label className="text-xs text-slate-400">
            Min edge (bps)
            <input
              value={minEdgeBps}
              onChange={(e) => setMinEdgeBps(e.target.value)}
              className="ml-1 w-16 px-2 py-1 bg-surface border border-surface-border rounded-md text-xs font-mono"
            />
          </label>
          <button
            type="button"
            onClick={() => void scan()}
            disabled={scanning || isPaused}
            className="btn-primary inline-flex items-center gap-1.5"
          >
            <RefreshCw className={`w-3.5 h-3.5 ${scanning ? "animate-spin" : ""}`} />
            {scanning ? "Scanning…" : "Scan"}
          </button>
          {isLive && (
            <span className="text-xs text-amber-400 border border-amber-500/40 rounded px-1.5 py-0.5">
              LIVE — manual executes only
            </span>
          )}
        </div>
        <p className="mt-2 text-xs text-slate-500">
          Put-call parity: synthetic forward K + (C − P) vs the perpetual's
          funding-adjusted price. Conversion sells the rich synthetic (short
          call + long put + long perp); Reversal buys the cheap one. Edge is
          unrealized until expiry — premiums and fees are the only cash at
          entry.
        </p>
      </div>

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

      {scanned && opps.length === 0 && !error && (
        <EmptyState
          icon={Option}
          plain
          title="No parity edge right now"
          hint="Synthetic forward ≈ funding-adjusted perp within fees on every quoted strike/expiry pair."
        />
      )}

      {opps.map((o) => (
        <OptionsParityCard
          key={o.id}
          opp={o}
          executing={executing === o.id}
          executed={executed.has(o.id)}
          disabled={!o.is_executable || isPaused || scanning}
          onExecute={() => setConfirmOpp(o)}
        />
      ))}

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
