import React, { useState, useEffect, useMemo } from "react";
import { RefreshCw } from "lucide-react";
import type { VenueOrderBookDto, SpatialOpportunityDto, ExecutionLogEntry } from "../types";
import {
  PERP_TICKERS,
  formatCurrencyPrice,
  formatCurrencyPnl,
  formatSpreadPrice,
  getDefaultOrderQty,
} from "../types/tickers";

interface LiveMatrixProps {
  symbol: string;
  orderbooks: VenueOrderBookDto[];
  opportunities: SpatialOpportunityDto[];
  onRefresh: (qty?: number, minBps?: number) => void;
  onExecuteTrade: (opp: SpatialOpportunityDto) => Promise<void>;
  loading: boolean;
  executionLogs?: ExecutionLogEntry[];
}

export const LiveMatrix: React.FC<LiveMatrixProps> = ({
  symbol,
  orderbooks,
  opportunities,
  onRefresh,
  onExecuteTrade,
  loading,
  executionLogs = [],
}) => {
  const activeTicker = PERP_TICKERS.find((t) => t.symbol === symbol) || {
    symbol,
    base: symbol.split("-")[0],
    quote: symbol.split("-")[1] || "USDC",
    name: symbol,
    venues: ["HL", "LGT", "VNT"],
    basePrice: 100,
  };

  const [orderQty, setOrderQty] = useState(() => getDefaultOrderQty(activeTicker.basePrice));
  const [minBps, setMinBps] = useState(5.0);
  const [executingId, setExecutingId] = useState<string | null>(null);
  const [lastExecuted, setLastExecuted] = useState<string | null>(null);

  // Automatically sync benchmark order quantity when active asset changes
  useEffect(() => {
    setOrderQty(getDefaultOrderQty(activeTicker.basePrice));
  }, [symbol, activeTicker.basePrice]);

  // Derive execution stats for the currently selected ticker
  const tickerStats = useMemo(() => {
    let count = 0;
    let pnl = 0;
    const symKey = symbol.toUpperCase();
    const baseKey = symKey.split(/[-_/]/)[0];

    for (const log of executionLogs) {
      if (!log.symbol) continue;
      const logKey = log.symbol.toUpperCase();
      const logBase = logKey.split(/[-_/]/)[0];
      if (logKey === symKey || logBase === baseKey) {
        count += 1;
        pnl += log.net_profit || 0;
      }
    }
    return { count, pnl: Math.round(pnl * 100) / 100 };
  }, [executionLogs, symbol]);

  const handleExecute = async (opp: SpatialOpportunityDto) => {
    setExecutingId(opp.id);
    try {
      await onExecuteTrade(opp);
      setLastExecuted(opp.id);
      setTimeout(() => setLastExecuted(null), 3000);
    } finally {
      setExecutingId(null);
    }
  };

  return (
    <div className="p-6 space-y-6 overflow-y-auto h-full">
      {/* Top Bar: Controls */}
      <div className="flex flex-wrap items-center justify-between gap-4 bg-surface p-4 rounded-xl border border-surface-border shadow-xs">
        <div className="flex items-center space-x-3">
          <div className="flex items-center space-x-2">
            <span className="text-xs font-mono uppercase text-slate-500 font-semibold">Active:</span>
            <div className="text-xs font-mono font-bold text-slate-900 bg-surface-raised px-2.5 py-1.5 rounded-md border border-surface-border flex items-center space-x-2">
              <span>{symbol}</span>
              <span className="flex space-x-1">
                {activeTicker.venues.map((v) => (
                  <span key={v} className="text-[9px] px-1 py-0.2 rounded font-semibold bg-accent-dim text-accent border border-accent-line/40">
                    {v}
                  </span>
                ))}
              </span>
              <span
                className="text-[10px] px-1.5 py-0.2 rounded bg-surface border border-surface-border font-medium text-slate-700"
                title={`${tickerStats.count} strategy executions`}
              >
                {tickerStats.count} tx{tickerStats.count === 1 ? "" : "s"}
              </span>
              <span
                className={`text-[10px] px-1.5 py-0.2 rounded font-semibold ${
                  tickerStats.pnl > 0
                    ? "text-profit bg-profit-dim border border-profit-border"
                    : tickerStats.pnl < 0
                    ? "text-loss bg-loss-dim border border-loss-border"
                    : "text-slate-500 bg-surface border border-surface-border"
                }`}
                title={`Strategy Net PnL: ${formatCurrencyPnl(tickerStats.pnl)}`}
              >
                {formatCurrencyPnl(tickerStats.pnl)}
              </span>
            </div>
          </div>
        </div>

        <div className="flex items-center space-x-4">
          <div className="flex items-center space-x-2 text-xs font-mono">
            <span className="text-slate-500">Order Qty:</span>
            <input
              type="number"
              step={orderQty >= 1000 ? "100" : orderQty >= 10 ? "1" : orderQty >= 1 ? "0.1" : "0.001"}
              min="0.00000001"
              value={orderQty}
              onChange={(e) => setOrderQty(parseFloat(e.target.value) || 0.1)}
              className="w-28 px-2.5 py-1 bg-surface border border-surface-border rounded-md text-slate-900 font-mono text-xs focus:outline-none focus:ring-1 focus:ring-accent focus:border-accent"
            />
          </div>

          <div className="flex items-center space-x-2 text-xs font-mono">
            <span className="text-slate-500">Min Net BPS:</span>
            <input
              type="number"
              step="1"
              value={minBps}
              onChange={(e) => setMinBps(parseFloat(e.target.value) || 0)}
              className="w-16 px-2.5 py-1 bg-surface border border-surface-border rounded-md text-slate-900 font-mono text-xs focus:outline-none focus:ring-1 focus:ring-accent focus:border-accent"
            />
          </div>

          <button
            onClick={() => onRefresh(orderQty, minBps)}
            disabled={loading}
            className="flex items-center space-x-1.5 px-3 py-1.5 bg-surface hover:bg-surface-raised border border-surface-border rounded-lg text-xs font-mono text-slate-700 font-medium transition shadow-xs"
          >
            <RefreshCw className={`w-3.5 h-3.5 ${loading ? "animate-spin text-accent" : "text-slate-500"}`} />
            <span>Scan Orderbooks</span>
          </button>
        </div>
      </div>

      {/* 4-Venue Market Quotes Grid (Top of Book) */}
      <div>
        <div className="flex items-center justify-between mb-3">
          <h3 className="text-xs font-mono uppercase tracking-wider text-slate-500 font-semibold">
            Venue Market Quotes (Top of Book)
          </h3>
          <span className="text-[11px] font-mono text-slate-400">
            Target: {symbol}
          </span>
        </div>
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
          {orderbooks.map((book) => {
            const spreadColor = book.spread_bps < 10 ? "text-profit" : "text-warn";
            const topBidSize = book.bids.length > 0 ? book.bids[0][1] : 0;
            const topAskSize = book.asks.length > 0 ? book.asks[0][1] : 0;

            return (
              <div
                key={book.venue_id}
                className="bg-surface border border-surface-border rounded-xl p-4 flex flex-col justify-between hover:border-slate-300 transition shadow-xs"
              >
                {/* Header: Venue Name, Funding & Spread */}
                <div className="flex items-center justify-between border-b border-surface-border pb-2.5 mb-3">
                  <div>
                    <span className="font-mono text-xs font-bold text-slate-900">{book.venue_name}</span>
                    <div className="text-[10px] font-mono text-slate-500 mt-0.5">
                      8h Funding: {(book.funding_rate_8h_pct).toFixed(4)}%
                    </div>
                  </div>
                  <div className="text-right">
                    <span className="text-[10px] font-mono text-slate-400">Spread:</span>{" "}
                    <span className={`font-mono text-xs font-semibold ${spreadColor}`}>
                      {book.spread_bps.toFixed(1)} BPS
                    </span>
                  </div>
                </div>

                {/* Compact Best Bid / Best Ask Box */}
                <div className="grid grid-cols-2 gap-2 bg-surface-raised p-2.5 rounded-lg border border-surface-border mb-3">
                  {/* Bid (Buy) */}
                  <div>
                    <div className="text-[10px] font-mono text-slate-400 uppercase tracking-wide flex justify-between">
                      <span>Bid (Buy)</span>
                      <span className="text-slate-500">{topBidSize > 0 ? topBidSize.toFixed(2) : ""}</span>
                    </div>
                    <div className="text-sm font-mono font-bold text-profit mt-1">
                      {formatCurrencyPrice(book.best_bid)}
                    </div>
                  </div>

                  {/* Ask (Sell) */}
                  <div className="border-l border-surface-border pl-2.5">
                    <div className="text-[10px] font-mono text-slate-400 uppercase tracking-wide flex justify-between">
                      <span>Ask (Sell)</span>
                      <span className="text-slate-500">{topAskSize > 0 ? topAskSize.toFixed(2) : ""}</span>
                    </div>
                    <div className="text-sm font-mono font-bold text-loss mt-1">
                      {formatCurrencyPrice(book.best_ask)}
                    </div>
                  </div>
                </div>

                {/* Footer: Mid Price */}
                <div className="pt-2 border-t border-surface-border text-[11px] font-mono text-slate-500 flex justify-between items-center">
                  <span>Mid Price</span>
                  <span className="font-semibold text-slate-700">{formatCurrencyPrice(book.mid_price)}</span>
                </div>
              </div>
            );
          })}
        </div>
      </div>

      {/* Pairwise Arbitrage Opportunities */}
      <div className="bg-surface rounded-xl border border-surface-border p-5 shadow-xs">
        <div className="flex items-center justify-between mb-4">
          <div>
            <h3 className="text-sm font-bold font-mono text-slate-900 flex items-center space-x-2">
              <span>Arbitrage Dislocation Matrix</span>
              <span className="px-2 py-0.5 rounded-full text-[10px] bg-accent-dim text-accent border border-accent-line font-medium">
                {opportunities.length} Active
              </span>
            </h3>
            <p className="text-xs text-slate-500 mt-0.5">
              Net spreads calculated after venue taker fees and slippage verification.
            </p>
          </div>
        </div>

        {opportunities.length === 0 ? (
          <div className="text-center py-12 text-slate-400 font-mono text-xs border border-dashed border-surface-border rounded-lg">
            No spatial price discrepancies currently exceeding {minBps} BPS threshold.
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-left text-xs font-mono">
              <thead>
                <tr className="border-b border-surface-border text-slate-500 uppercase text-[10px] tracking-wider">
                  <th className="py-2.5 px-3">Route (Buy / Sell)</th>
                  <th className="py-2.5 px-3">Buy Price</th>
                  <th className="py-2.5 px-3">Sell Price</th>
                  <th className="py-2.5 px-3">Gross Spread</th>
                  <th className="py-2.5 px-3">Roundtrip Fees</th>
                  <th className="py-2.5 px-3">Net Spread</th>
                  <th className="py-2.5 px-3">Est. Profit</th>
                  <th className="py-2.5 px-3 text-right">Action</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-surface-border">
                {opportunities.map((opp) => {
                  const isExec = opp.is_executable;
                  const isLast = lastExecuted === opp.id;
                  return (
                    <tr
                      key={opp.id}
                      className={`hover:bg-surface-raised transition ${
                        isExec ? "bg-accent-dim/30" : ""
                      }`}
                    >
                      <td className="py-3 px-3 font-medium text-slate-900 flex items-center space-x-1.5">
                        <span className="text-slate-800">{opp.buy_venue}</span>
                        <span className="text-slate-400 text-xs">/</span>
                        <span className="text-slate-800">{opp.sell_venue}</span>
                      </td>
                      <td className="py-3 px-3 text-loss font-medium">{formatCurrencyPrice(opp.buy_price)}</td>
                      <td className="py-3 px-3 text-profit font-medium">{formatCurrencyPrice(opp.sell_price)}</td>
                      <td className="py-3 px-3 text-slate-700">
                        {opp.gross_spread_bps.toFixed(1)} BPS ({formatSpreadPrice(opp.gross_spread)})
                      </td>
                      <td className="py-3 px-3 text-slate-500">{opp.fee_bps.toFixed(1)} BPS</td>
                      <td className="py-3 px-3">
                        <span
                          className={`font-bold ${
                            opp.net_spread_bps >= minBps ? "text-profit" : "text-slate-500"
                          }`}
                        >
                          {opp.net_spread_bps.toFixed(2)} BPS
                        </span>
                      </td>
                      <td className={`py-3 px-3 font-bold ${opp.estimated_profit_usd >= 0 ? "text-profit" : "text-loss"}`}>
                        {opp.estimated_profit_usd >= 0
                          ? `+$${opp.estimated_profit_usd.toFixed(2)}`
                          : `-$${Math.abs(opp.estimated_profit_usd).toFixed(2)}`}
                      </td>
                      <td className="py-3 px-3 text-right">
                        {isLast ? (
                          <span className="inline-flex items-center px-2 py-0.5 rounded bg-profit-dim text-profit text-xs font-semibold">
                            Filled
                          </span>
                        ) : (
                          <button
                            onClick={() => handleExecute(opp)}
                            disabled={executingId === opp.id || !isExec}
                            className={`px-3 py-1.5 rounded-lg text-xs font-mono font-medium transition flex items-center ml-auto shadow-xs ${
                              isExec
                                ? "bg-accent hover:bg-accent-hover text-white"
                                : "bg-surface-raised text-slate-400 cursor-not-allowed border border-surface-border"
                            }`}
                          >
                            <span>{executingId === opp.id ? "Routing..." : "Execute"}</span>
                          </button>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </div>
  );
};
