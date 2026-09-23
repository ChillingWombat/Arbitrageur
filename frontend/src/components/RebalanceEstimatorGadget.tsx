/**
 * RebalanceEstimatorGadget — "SHOULD I MOVE MONEY, AND WHAT WILL IT COST?"
 * ========================================================================
 * USER STORY: the Capital Allocation bars above show venue A is DEPLETED while
 * venue B sits idle. The user picks (from, to, amount) and this gadget prints a
 * RECEIPT: withdraw fee + gas + deposit fee + optional swap friction = total
 * friction, what lands at the destination, the cheapest path (direct venue→venue
 * vs via own wallet), and a worth-it verdict against the opportunity profit.
 *
 * HOW IT WORKS (for rebuild practice):
 *   - All state is local useState (no backend/IPC — the fee table is static
 *     research in `services/rebalanceFees.ts`). The estimate is recomputed with
 *     useMemo on every keystroke: pure function, no effects, no polling.
 *   - `venueBalances` (optional prop) pre-fills the dropdowns with venues the
 *     user actually holds money on; otherwise all researched venues are listed.
 *   - Styling follows the app system: Inter (font-mono token), bg-surface cards,
 *     light-blue `bg-accent-dim` for selected/cheapest-path highlights, blue
 *     `bg-accent` for the primary action affordance. Numbers are tabular-nums.
 */
import React, { useMemo, useState } from "react";
import { ArrowRightLeft, Info } from "lucide-react";
import { EmptyState } from "./EmptyState";
import {
  VENUE_TRANSFER_PROFILES,
  estimateRebalance,
  getVenueProfile,
} from "../services/rebalanceFees";
import type { VenueBalanceInfo } from "../types";

interface Props {
  /** Live balances (optional) — used to offer funded venues first. */
  venueBalances?: VenueBalanceInfo[] | null;
}

export const RebalanceEstimatorGadget: React.FC<Props> = ({ venueBalances = null }) => {
  // --- Form state ------------------------------------------------------------
  // Defaults encode the most common rebalance: move $2,000 USDC onto the
  // cheapest verified-fee DEX rails (Hyperliquid -> Lighter), direct route.
  const funded = useMemo(
    () => (venueBalances ?? []).map((b) => (b.venue ?? "").toLowerCase()).filter(Boolean),
    [venueBalances],
  );
  const options = useMemo(() => {
    const all = VENUE_TRANSFER_PROFILES.map((p) => p.venue);
    // Funded venues first (relevant choices), then the rest alphabetically.
    const rest = all.filter((v) => !funded.includes(v)).sort();
    return [...funded, ...rest];
  }, [funded]);
  const [from, setFrom] = useState("hyperliquid");
  const [to, setTo] = useState("lighter");
  const [amount, setAmount] = useState("2000");
  const [viaDirect, setViaDirect] = useState(true);
  const [swapNeeded, setSwapNeeded] = useState(false);
  const [profit, setProfit] = useState("");

  // --- Pure estimate (recomputed each render; no network, no side effects) ---
  const result = useMemo(() => {
    const amountUsd = Number(amount) || 0;
    const opportunityProfitUsd = profit === "" ? null : Number(profit) || 0;
    return estimateRebalance({
      from, to, amountUsd, viaDirect, swapNeeded, opportunityProfitUsd,
    });
  }, [from, to, amount, viaDirect, swapNeeded, profit]);

  const fromProfile = getVenueProfile(from);
  const toProfile = getVenueProfile(to);
  const selCls =
    "h-9 w-full px-2.5 bg-surface border border-surface-border rounded-lg text-xs font-mono font-semibold text-slate-800 focus:outline-none focus:ring-1 focus:ring-accent cursor-pointer";
  const inputCls =
    "h-9 w-full px-2.5 bg-surface border border-surface-border rounded-lg text-xs font-mono font-semibold text-slate-900 tabular-nums focus:outline-none focus:ring-1 focus:ring-accent";
  const labelCls = "text-[11px] font-mono font-bold text-slate-500 leading-none";
  const routeBtnBase =
    "h-9 flex-1 px-2 rounded-lg text-xs font-mono font-bold border flex items-center justify-center transition cursor-pointer";
  const verdictCls =
    result.verdict === "worth_it"
      ? "bg-profit-dim/40 border-profit-border text-slate-900"
      : result.verdict === "not_worth_it"
        ? "bg-loss-dim/40 border-loss-border text-slate-900"
        : "bg-surface-raised border-surface-border text-slate-700";

  return (
    <div className="bg-surface rounded-xl border border-surface-border p-4 shadow-card space-y-3">
      {/* Header: what this gadget answers + why estimates exist. */}
      <div className="border-b border-surface-border pb-2.5">
        <h2 className="text-[13px] font-mono tracking-wider text-slate-900 font-bold flex items-center space-x-2">
          <ArrowRightLeft className="w-5 h-5 text-accent-strong" />
          <span>Rebalance Friction Estimator</span>
        </h2>
        <p className="text-xs text-slate-600 font-mono mt-1">
          Cost of moving funds venue → venue: withdraw + gas + deposit + swap.
          Fees marked ~ are researched estimates — re-check the venue withdraw page before moving size.
        </p>
      </div>

      {/* Form: from / to / amount / profit + route + swap toggles. All controls share h-9. */}
      <div className="grid grid-cols-2 items-end gap-2 md:grid-cols-5">
        <label className="flex min-w-0 flex-col gap-1.5">
          <span className={labelCls}>FROM</span>
          <select value={from} onChange={(e) => setFrom(e.target.value)} className={selCls} title="Source venue">
            {options.map((v) => (
              <option key={v} value={v}>{getVenueProfile(v)?.label ?? v}</option>
            ))}
          </select>
        </label>
        <label className="flex min-w-0 flex-col gap-1.5">
          <span className={labelCls}>TO</span>
          <select value={to} onChange={(e) => setTo(e.target.value)} className={selCls} title="Destination venue">
            {options.map((v) => (
              <option key={v} value={v}>{getVenueProfile(v)?.label ?? v}</option>
            ))}
          </select>
        </label>
        <label className="flex min-w-0 flex-col gap-1.5">
          <span className={labelCls}>AMOUNT (USD)</span>
          <input
            value={amount}
            onChange={(e) => setAmount(e.target.value.replace(/[^0-9.]/g, ""))}
            inputMode="decimal"
            className={inputCls}
          />
        </label>
        <label className="flex min-w-0 flex-col gap-1.5">
          <span className={labelCls}>OPPORTUNITY PROFIT (USD)</span>
          <input
            value={profit}
            onChange={(e) => setProfit(e.target.value.replace(/[^0-9.]/g, ""))}
            inputMode="decimal"
            placeholder="e.g. 50"
            className={inputCls}
          />
        </label>
        <div className="flex min-w-0 flex-col gap-1.5">
          <span className={labelCls}>ROUTE</span>
          <div className="flex gap-1.5">
            <button
              type="button"
              onClick={() => setViaDirect(true)}
              className={`${routeBtnBase} ${viaDirect ? "bg-accent-dim text-accent-strong border-accent-line/60" : "bg-surface text-slate-500 border-surface-border"}`}
              title="Withdraw straight to the destination deposit address"
            >
              Direct
            </button>
            <button
              type="button"
              onClick={() => setViaDirect(false)}
              className={`${routeBtnBase} ${!viaDirect ? "bg-accent-dim text-accent-strong border-accent-line/60" : "bg-surface text-slate-500 border-surface-border"}`}
              title="Withdraw to your wallet, then deposit"
            >
              Via Wallet
            </button>
          </div>
        </div>
      </div>

      <label className="flex items-center gap-2 text-xs font-mono text-slate-600 cursor-pointer">
        <input type="checkbox" checked={swapNeeded} onChange={(e) => setSwapNeeded(e.target.checked)} className="accent-[#2563eb]" />
        Coin swap needed (e.g. SOL → USDC, ~15 bps taker + spread)
      </label>

      {/* Coin rails: what each side accepts — prevents doomed moves. */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-2 text-[11px] font-mono">
        <div className="px-2.5 py-2 bg-surface-raised rounded-lg border border-surface-border">
          <span className="font-bold text-slate-700">{fromProfile?.label} rails: </span>
          <span className="text-slate-600">{fromProfile?.withdrawCoins.join(", ")} · {fromProfile?.networks.join(" / ")}</span>
        </div>
        <div className="px-2.5 py-2 bg-surface-raised rounded-lg border border-surface-border">
          <span className="font-bold text-slate-700">{toProfile?.label} rails: </span>
          <span className="text-slate-600">{toProfile?.depositCoins.join(", ")} · {toProfile?.timeEstimate}</span>
        </div>
      </div>

      {/* Receipt: the cost breakdown. Null total = fail-closed, not a lowball. */}
      {result.legs.length > 0 ? (
        <div className="space-y-2">
          <div className="rounded-lg border border-surface-border overflow-hidden">
            {result.legs.map((leg) => (
              <div key={leg.label} className="flex items-center justify-between gap-2 px-3 py-1.5 text-xs font-mono odd:bg-surface-raised/50">
                <span className="text-slate-600">{leg.label} <span className="text-muted">· {leg.note}</span></span>
                <span className="font-bold tabular-nums text-slate-900">
                  {leg.costUsd == null ? "—" : `$${leg.costUsd.toFixed(2)}`}
                </span>
              </div>
            ))}
            <div className="flex items-center justify-between gap-2 px-3 py-2 text-xs font-mono bg-surface-raised border-t border-surface-border">
              <span className="font-bold text-slate-900">Total friction {result.costBps != null && <span className="text-muted font-semibold">({result.costBps.toFixed(1)} bps)</span>}</span>
              <span className="font-bold tabular-nums text-slate-900">
                {result.totalCostUsd == null ? "— cannot price" : `$${result.totalCostUsd.toFixed(2)} → nets $${result.netReceivedUsd!.toFixed(2)}`}
              </span>
            </div>
          </div>

          {/* Cheapest path (light-blue = selected/recommended per app convention). */}
          <div className="px-3 py-2.5 rounded-lg border bg-accent-dim text-accent-strong border-accent-line/60 text-xs font-mono">
            {result.cheapestPath}
          </div>

          {/* Worth-it verdict — hidden when no profit entered (placeholder removed). */}
          {(result.verdict === "worth_it" || result.verdict === "not_worth_it" || result.totalCostUsd == null) && (
            <div className={`px-3 py-2.5 rounded-lg border text-xs font-mono ${verdictCls}`}>
              <span className="font-bold">{result.verdict === "worth_it" ? "Worth rebalancing. " : result.verdict === "not_worth_it" ? "Skip the rebalance. " : ""}</span>
              {result.verdictDetail}
            </div>
          )}

          {result.warnings.length > 0 && (
            <div className="flex items-start gap-2 text-[11px] font-mono text-warn-strong">
              <Info className="w-3.5 h-3.5 shrink-0 mt-0.5" />
              <span>{result.warnings.join(" ")}{result.hasEstimates && " Figures marked ~ are estimates — confirm on the venue withdraw page."}</span>
            </div>
          )}
        </div>
      ) : (
        <EmptyState title="Pick two venues to price the move." hint="Same venue needs no transfer." />
      )}
    </div>
  );
};
