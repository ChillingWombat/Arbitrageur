import React, { useState } from "react";
import { RefreshCw } from "lucide-react";
import type { VenueOrderBookDto, SpatialOpportunityDto } from "../types";

interface LiveMatrixProps {
  symbol: string;
  onSymbolChange: (symbol: string) => void;
  orderbooks: VenueOrderBookDto[];
  opportunities: SpatialOpportunityDto[];
  onRefresh: () => void;
  onExecuteTrade: (opp: SpatialOpportunityDto) => Promise<void>;
  loading: boolean;
}

export const LiveMatrix: React.FC<LiveMatrixProps> = ({
  symbol,
  onSymbolChange,
  orderbooks,
  opportunities,
  onRefresh,
  onExecuteTrade,
  loading,
}) => {
  const [orderQty, setOrderQty] = useState(0.5);
  const [minBps, setMinBps] = useState(5.0);
  const [executingId, setExecutingId] = useState<string | null>(null);
  const [lastExecuted, setLastExecuted] = useState<string | null>(null);

  const symbols = ["BTC-USDC", "ETH-USDC", "SOL-USDC"];

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
          <span className="text-xs font-mono uppercase text-slate-500">Target Asset:</span>
          <div className="flex items-center space-x-1.5 bg-surface-raised p-1 rounded-lg border border-surface-border">
            {symbols.map((s) => (
              <button
                key={s}
                onClick={() => onSymbolChange(s)}
                className={`px-3 py-1 text-xs font-mono rounded-md transition ${
                  symbol === s
                    ? "bg-accent text-white font-semibold shadow-xs"
                    : "text-slate-600 hover:text-slate-900"
                }`}
              >
                {s}
              </button>
            ))}
          </div>
        </div>

        <div className="flex items-center space-x-4">
          <div className="flex items-center space-x-2 text-xs font-mono">
            <span className="text-slate-500">Order Qty:</span>
            <input
              type="number"
              step="0.1"
              min="0.01"
              value={orderQty}
              onChange={(e) => setOrderQty(parseFloat(e.target.value) || 0.1)}
              className="w-20 px-2.5 py-1 bg-surface border border-surface-border rounded-md text-slate-900 font-mono text-xs focus:outline-none focus:ring-1 focus:ring-accent focus:border-accent"
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
            onClick={onRefresh}
            disabled={loading}
            className="flex items-center space-x-1.5 px-3 py-1.5 bg-surface hover:bg-surface-raised border border-surface-border rounded-lg text-xs font-mono text-slate-700 font-medium transition shadow-xs"
          >
            <RefreshCw className={`w-3.5 h-3.5 ${loading ? "animate-spin text-accent" : "text-slate-500"}`} />
            <span>Scan Orderbooks</span>
          </button>
        </div>
      </div>

      {/* 4-Venue Orderbook Comparison Grid */}
      <div>
        <h3 className="text-xs font-mono uppercase tracking-wider text-slate-500 mb-3 flex items-center justify-between">
          <span>Cross-Venue L2 Orderbook Depth & VWAP</span>
          <span className="text-slate-400 font-normal lowercase">calculated with benchmark size: {orderQty} {symbol.split("-")[0]}</span>
        </h3>
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
          {orderbooks.map((book) => {
            const spreadColor = book.spread_bps < 10 ? "text-profit" : "text-warn";
            return (
              <div
                key={book.venue_id}
                className="bg-surface border border-surface-border rounded-xl p-4 flex flex-col justify-between hover:border-slate-300 transition shadow-xs"
              >
                <div>
                  {/* Card Header */}
                  <div className="flex items-center justify-between border-b border-surface-border pb-2 mb-3">
                    <div>
                      <span className="font-mono text-xs font-bold text-slate-900">{book.venue_name}</span>
                      <div className="text-[10px] font-mono text-slate-500">
                        8h Funding: {(book.funding_rate_8h_pct).toFixed(4)}%
                      </div>
                    </div>
                    <div className="text-right">
                      <span className="text-[10px] font-mono text-slate-500">Spread:</span>{" "}
                      <span className={`font-mono text-xs font-semibold ${spreadColor}`}>
                        {book.spread_bps.toFixed(1)} BPS
                      </span>
                    </div>
                  </div>

                  {/* Asks (Red) */}
                  <div className="space-y-1 mb-2">
                    <div className="text-[10px] font-mono text-slate-400 flex justify-between px-1">
                      <span>ASK (SELL)</span>
                      <span>SIZE</span>
                    </div>
                    {book.asks.slice(0, 3).reverse().map(([price, size], idx) => {
                      const widthPct = Math.min(100, (size / (book.total_ask_depth || 1)) * 100);
                      return (
                        <div key={idx} className="relative flex justify-between text-xs font-mono px-2 py-0.5 rounded overflow-hidden">
                          <div
                            className="absolute inset-y-0 right-0 bg-loss-dim"
                            style={{ width: `${widthPct}%` }}
                          />
                          <span className="text-loss font-medium relative z-10">${price.toFixed(2)}</span>
                          <span className="text-slate-500 relative z-10">{size.toFixed(3)}</span>
                        </div>
                      );
                    })}
                  </div>

                  {/* Mid / VWAP Bar */}
                  <div className="py-1.5 my-1 px-2 rounded bg-surface-raised border border-surface-border text-center font-mono text-[11px] flex justify-between items-center">
                    <span className="text-slate-600">Mid: ${book.mid_price.toFixed(2)}</span>
                    <span className="text-accent font-medium text-[10px]">VWAP: ${book.vwap_ask.toFixed(2)}</span>
                  </div>

                  {/* Bids (Green) */}
                  <div className="space-y-1">
                    <div className="text-[10px] font-mono text-slate-400 flex justify-between px-1">
                      <span>BID (BUY)</span>
                      <span>SIZE</span>
                    </div>
                    {book.bids.slice(0, 3).map(([price, size], idx) => {
                      const widthPct = Math.min(100, (size / (book.total_bid_depth || 1)) * 100);
                      return (
                        <div key={idx} className="relative flex justify-between text-xs font-mono px-2 py-0.5 rounded overflow-hidden">
                          <div
                            className="absolute inset-y-0 right-0 bg-profit-dim"
                            style={{ width: `${widthPct}%` }}
                          />
                          <span className="text-profit font-medium relative z-10">${price.toFixed(2)}</span>
                          <span className="text-slate-500 relative z-10">{size.toFixed(3)}</span>
                        </div>
                      );
                    })}
                  </div>
                </div>

                {/* Best B/A Summary Footer */}
                <div className="mt-4 pt-2 border-t border-surface-border text-[10px] font-mono text-slate-500 flex justify-between">
                  <span>Best: ${book.best_bid.toFixed(2)}</span>
                  <span>${book.best_ask.toFixed(2)}</span>
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
                      <td className="py-3 px-3 text-loss font-medium">${opp.buy_price.toFixed(2)}</td>
                      <td className="py-3 px-3 text-profit font-medium">${opp.sell_price.toFixed(2)}</td>
                      <td className="py-3 px-3 text-slate-700">
                        {opp.gross_spread_bps.toFixed(1)} BPS (${opp.gross_spread.toFixed(2)})
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
                      <td className="py-3 px-3 font-bold text-profit">
                        +${opp.estimated_profit_usd.toFixed(2)}
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
