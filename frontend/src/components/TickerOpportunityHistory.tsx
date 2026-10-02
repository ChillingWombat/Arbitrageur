import React from "react";
import { OpportunityHistoryGadget } from "./OpportunityHistoryGadget";

export interface TickerOpportunityHistoryProps {
  /** Canonical symbol of the selected ticker (e.g. "BTC-USDC"), or empty for all tickers. */
  symbol?: string;
  /** Poll cadence. */
  pollMs?: number;
}

/**
 * Opportunity history gadget: covers all monitor detections alongside
 * executed trades and bot skips. Traded vs Not Traded switch controls
 * provide immediate visibility across the opportunity lifecycle.
 */
export const TickerOpportunityHistory: React.FC<TickerOpportunityHistoryProps> = ({
  symbol = "",
  pollMs,
}) => {
  return (
    <OpportunityHistoryGadget
      symbol={symbol}
      pollMs={pollMs}
      strategy="futures"
    />
  );
};

export default TickerOpportunityHistory;
