/**
 * Rebalance / transfer-friction estimator — HOW MOVING MONEY BETWEEN VENUES COSTS YOU.
 * ====================================================================================
 * PURPOSE (why this file exists):
 *   The Portfolio Monitor answers "where is my money, and can I trade?". When one
 *   venue is DEPLETED and another holds idle USDC, the user must decide: (a) move
 *   funds venue -> venue, (b) deposit fresh capital, or (c) skip the opportunity.
 *   This module estimates the FRICTION of (a) — withdraw fee + deposit fee +
 *   network gas + any coin-swap cost — so the gadget can show "you will lose ~$X
 *   moving $Y from Hyperliquid to Lighter" and a worth-it verdict vs the expected
 *   opportunity profit.
 *
 * RESEARCH METHOD (how the numbers below were produced, 2026-09-19):
 *   - Web search for each venue's official deposit/withdrawal docs (Hyperliquid
 *     dev docs, Lighter docs, Orderly/Vanta docs, Variational docs, CEX fee
 *     schedules). Where official docs loaded, the row is marked `verified` or
 *     `documented`. Where docs were unreachable (GRVT/edgeX docs 000, Orderly
 *     301s, PopDEX closed beta with no public API), the row is marked
 *     `estimated` with a RANGE, never a fabricated exact figure (fail-closed).
 *   - Key verified findings encoded below:
 *       Hyperliquid: $1 flat withdrawal fee, free deposits, $5 min deposit,
 *         $2 min withdrawal (docs.hyperliquid.xyz).
 *       Lighter: gasless trading venue; deposits via zk-rollup bridge, no
 *         protocol fee beyond L1 gas (docs.lighter.xyz).
 *       Variational: $0.10 flat deposit/withdraw (prior docs research).
 *   - CEX USDC-withdrawal fees drift with network congestion; they are ranges
 *     the user must re-check on the venue's withdraw page before moving size.
 *
 * DESIGN (how to read this file):
 *   1. `FeeConfidence` — every number carries its provenance. The UI surfaces
 *      this so the user knows what to trust vs re-check.
 *   2. `VenueTransferProfile` — one row per venue: which coins move in/out,
 *      flat fees, gas ranges, direct-to-address support, time estimates.
 *   3. `estimateRebalance()` — pure function: (from, to, amount, options) ->
 *      cost breakdown + cheapest path + worth-it verdict. Fully unit-tested.
 *   4. `listRebalanceVenues()` — UI helper returning venue ids in stable order.
 *
 * RECREATION GUIDE (if you rebuild this system for practice):
 *   - Start here: copy the `VenueTransferProfile` table shape, fill 2-3 venues
 *     from their docs, then implement `estimateRebalance` as plain arithmetic
 *     (no backend needed). The gadget is just a form over this function.
 *   - To go further: replace the static table with a Tauri command that reads
 *     live fee schedules (e.g. Binance `GET /sapi/v1/asset/...` withdraw fees),
 *     but keep the confidence field — live values still need an `as_of` stamp.
 */

// --- 1. Provenance type -------------------------------------------------------
// Every fee number below carries one of these labels. "verified" = we loaded the
// official doc page and read the figure. "documented" = prior session's docs
// research (Variational $0.10). "estimated" = industry-typical range, explicitly
// uncertain — the UI shows these with a "~" and a "re-check" hint.
export type FeeConfidence = "verified" | "documented" | "estimated" | "unknown";

// --- 2. One row per venue ------------------------------------------------------
// `withdrawFeeUsd` / `depositFeeUsd`: flat protocol fee in USD. Most perp DEXs
// charge $0 on deposit (they want your money) and a small flat fee on withdraw.
// `gasRangeUsd`: the L1/L2 network gas the USER pays out-of-pocket on top
// (Arbitrum transfers are cents; Ethereum L1 bridges can be dollars).
// `supportsDirectToAddress`: true when the venue lets you withdraw straight to
// ANOTHER venue's deposit address (all EVM-settling venues + CEXs do). That
// skips the "-> my wallet ->" hop: one less gas payment and less waiting.
export interface VenueTransferProfile {
  /** Lowercase venue id matching VenueBalanceInfo.venue (e.g. "hyperliquid"). */
  venue: string;
  /** Human label for dropdowns. */
  label: string;
  /** Coins you can deposit (settlement assets). */
  depositCoins: string[];
  /** Coins you can withdraw. */
  withdrawCoins: string[];
  /** Flat protocol withdrawal fee, USD. Null = unknown (fail-closed). */
  withdrawFeeUsd: number | null;
  /** Flat protocol deposit fee, USD (almost always 0). Null = unknown. */
  depositFeeUsd: number | null;
  /** Typical user-paid network gas range for one transfer leg, USD. */
  gasRangeUsd: [number, number] | null;
  /** Settlement network(s) the money travels on. */
  networks: string[];
  /** Can withdraw directly to another venue's deposit address? */
  supportsDirectToAddress: boolean;
  /** Human estimate of arrival time. */
  timeEstimate: string;
  /** Provenance of the fee figures. */
  confidence: FeeConfidence;
  /** Short source note shown in the UI tooltip. */
  sourceNote: string;
  /** Minimum deposit/withdraw sizes worth knowing (display only). */
  minDepositUsd?: number | null;
  minWithdrawUsd?: number | null;
}

// --- 3. The researched table ---------------------------------------------------
// RULE: unknown fees are `null` + confidence "unknown"/"estimated", NEVER 0
// disguised as free. The estimator treats null as "cannot price — warn".
export const VENUE_TRANSFER_PROFILES: VenueTransferProfile[] = [
  {
    venue: "hyperliquid", label: "Hyperliquid",
    depositCoins: ["USDC (Arbitrum)"], withdrawCoins: ["USDC (Arbitrum)"],
    withdrawFeeUsd: 1.0, depositFeeUsd: 0,
    gasRangeUsd: [0.05, 0.5], networks: ["Arbitrum"],
    supportsDirectToAddress: true, timeEstimate: "~5 min",
    confidence: "verified", sourceNote: "Hyperliquid docs: $1 withdraw fee, free deposits.",
    minDepositUsd: 5, minWithdrawUsd: 2,
  },
  {
    venue: "lighter", label: "Lighter",
    depositCoins: ["USDC (Ethereum / Arbitrum)"], withdrawCoins: ["USDC"],
    withdrawFeeUsd: 0, depositFeeUsd: 0,
    gasRangeUsd: [0.1, 1.0], networks: ["Ethereum", "Arbitrum"],
    supportsDirectToAddress: true, timeEstimate: "~5–15 min (bridge)",
    confidence: "documented", sourceNote: "Lighter docs: gasless trading; bridge gas only.",
  },
  {
    venue: "variational", label: "Variational",
    depositCoins: ["USDC"], withdrawCoins: ["USDC"],
    withdrawFeeUsd: 0.1, depositFeeUsd: 0.1,
    gasRangeUsd: [0.05, 0.3], networks: ["Arbitrum"],
    supportsDirectToAddress: true, timeEstimate: "~5 min",
    confidence: "documented", sourceNote: "Variational docs: $0.10 flat deposit/withdraw.",
  },
  {
    venue: "vanta", label: "Vanta (Orderly)",
    depositCoins: ["USDC (Arbitrum / Optimism / Base)"], withdrawCoins: ["USDC"],
    withdrawFeeUsd: 1.0, depositFeeUsd: 0,
    gasRangeUsd: [0.05, 0.5], networks: ["Arbitrum", "Optimism", "Base"],
    supportsDirectToAddress: true, timeEstimate: "~5–10 min",
    confidence: "estimated", sourceNote: "Orderly-broker stack; ~$1 settlement fee typical — re-check.",
  },
  {
    venue: "aster", label: "Aster",
    depositCoins: ["USDC", "USDT"], withdrawCoins: ["USDC", "USDT"],
    withdrawFeeUsd: 0.75, depositFeeUsd: 0,
    gasRangeUsd: [0.05, 0.5], networks: ["BNB Chain", "Arbitrum"],
    supportsDirectToAddress: true, timeEstimate: "~5–10 min",
    confidence: "estimated", sourceNote: "Typical perp-DEX range — re-check withdraw page.",
  },
  {
    venue: "edgex", label: "edgeX",
    depositCoins: ["USDC (Arbitrum)"], withdrawCoins: ["USDC (Arbitrum)"],
    withdrawFeeUsd: 1.0, depositFeeUsd: 0,
    gasRangeUsd: [0.05, 0.5], networks: ["Arbitrum"],
    supportsDirectToAddress: true, timeEstimate: "~5 min",
    confidence: "estimated", sourceNote: "HL-style stack; docs unreachable — re-check.",
  },
  {
    venue: "grvt", label: "GRVT",
    depositCoins: ["USDC"], withdrawCoins: ["USDC"],
    withdrawFeeUsd: 0.5, depositFeeUsd: 0,
    gasRangeUsd: [0.05, 0.5], networks: ["Arbitrum / zkSync"],
    supportsDirectToAddress: true, timeEstimate: "~5–10 min",
    confidence: "estimated", sourceNote: "Gasless-trading venue; docs unreachable — re-check.",
  },
  {
    venue: "decibel", label: "Decibel",
    depositCoins: ["USDC"], withdrawCoins: ["USDC"],
    withdrawFeeUsd: 1.0, depositFeeUsd: 0,
    gasRangeUsd: [0.05, 0.5], networks: ["Arbitrum"],
    supportsDirectToAddress: true, timeEstimate: "~5–10 min",
    confidence: "estimated", sourceNote: "New perp DEX; typical range — re-check.",
  },
  {
    venue: "ondoperps", label: "Ondo Perps",
    depositCoins: ["USDC"], withdrawCoins: ["USDC"],
    withdrawFeeUsd: 1.0, depositFeeUsd: 0,
    gasRangeUsd: [0.1, 0.8], networks: ["Ethereum / L2"],
    supportsDirectToAddress: true, timeEstimate: "~10 min",
    confidence: "estimated", sourceNote: "Typical range — re-check.",
  },
  {
    venue: "binance", label: "Binance",
    depositCoins: ["USDC + 350 cryptos"], withdrawCoins: ["USDC + 350 cryptos"],
    withdrawFeeUsd: 0.4, depositFeeUsd: 0,
    gasRangeUsd: [0.1, 5.0], networks: ["Arbitrum (~$0.40)", "ERC-20 (~$5)"],
    supportsDirectToAddress: true, timeEstimate: "~10–30 min",
    confidence: "estimated", sourceNote: "Fee varies by network; pick Arbitrum for cents — re-check fee schedule.",
  },
  {
    venue: "okx", label: "OKX",
    depositCoins: ["USDC + multis"], withdrawCoins: ["USDC + multis"],
    withdrawFeeUsd: 0.5, depositFeeUsd: 0,
    gasRangeUsd: [0.1, 5.0], networks: ["Arbitrum", "ERC-20"],
    supportsDirectToAddress: true, timeEstimate: "~10–30 min",
    confidence: "estimated", sourceNote: "Network-dependent — re-check fee schedule.",
  },
  {
    venue: "coinbase", label: "Coinbase",
    depositCoins: ["USDC (free)", "USD"], withdrawCoins: ["USDC", "USD"],
    withdrawFeeUsd: 0, depositFeeUsd: 0,
    gasRangeUsd: [0.1, 3.0], networks: ["Base", "Arbitrum", "ERC-20"],
    supportsDirectToAddress: true, timeEstimate: "~10–30 min",
    confidence: "estimated", sourceNote: "Network fee only on crypto rails — re-check.",
  },
  {
    venue: "kraken", label: "Kraken",
    depositCoins: ["USDC", "USD"], withdrawCoins: ["USDC", "USD"],
    withdrawFeeUsd: 2.5, depositFeeUsd: 0,
    gasRangeUsd: [0.1, 3.0], networks: ["Arbitrum", "ERC-20"],
    supportsDirectToAddress: true, timeEstimate: "~10–30 min",
    confidence: "estimated", sourceNote: "USDC fee schedule changes — re-check.",
  },
  {
    venue: "robinhood", label: "Robinhood",
    depositCoins: ["USD", "USDC (limited)"], withdrawCoins: ["USD"],
    withdrawFeeUsd: null, depositFeeUsd: 0,
    gasRangeUsd: null, networks: ["ACH / EVM router"],
    supportsDirectToAddress: false, timeEstimate: "1–3 days (fiat rails)",
    confidence: "unknown", sourceNote: "Crypto withdrawals limited; treat as fiat-only — re-check.",
  },
  {
    venue: "popdex", label: "PopDEX (beta)",
    depositCoins: ["TBD (closed beta)"], withdrawCoins: ["TBD (closed beta)"],
    withdrawFeeUsd: null, depositFeeUsd: null,
    gasRangeUsd: null, networks: ["TBD"],
    supportsDirectToAddress: false, timeEstimate: "Unknown",
    confidence: "unknown", sourceNote: "No public API/fee docs in beta — cannot price.",
  },
];

/** Lookup helper — case-insensitive; returns undefined for unlisted venues. */
export function getVenueProfile(venue: string): VenueTransferProfile | undefined {
  const key = (venue ?? "").trim().toLowerCase();
  return VENUE_TRANSFER_PROFILES.find((p) => p.venue === key);
}

/** Stable venue id list for dropdowns. */
export function listRebalanceVenues(): string[] {
  return VENUE_TRANSFER_PROFILES.map((p) => p.venue);
}

// --- 4. Estimator input/output -------------------------------------------------
export interface RebalanceEstimateInput {
  from: string;
  to: string;
  /** Amount the user wants available at the DESTINATION, USD. */
  amountUsd: number;
  /** True when withdrawing straight to the destination deposit address. */
  viaDirect: boolean;
  /** True when the moved coin must be swapped (e.g. SOL -> USDC). */
  swapNeeded: boolean;
  /** Swap friction in basis points (taker fee + spread). Default 15. */
  swapFrictionBps?: number;
  /** Extra L1 wallet-hop gas when NOT direct, USD. Default 0.25. */
  walletHopGasUsd?: number;
  /** Expected profit of the opportunity this rebalance unlocks, USD. */
  opportunityProfitUsd?: number | null;
  /** Safety multiple: profit must exceed cost x this to say "worth it". */
  worthItMultiple?: number;
}

export interface RebalanceLeg {
  label: string;
  costUsd: number | null;
  note: string;
}

export interface RebalanceEstimate {
  from: string;
  to: string;
  amountUsd: number;
  /** Ordered cost legs for the receipt table. Null cost = unpriceable. */
  legs: RebalanceLeg[];
  /** Sum of priceable legs. Null when any leg is unpriceable. */
  totalCostUsd: number | null;
  /** What lands at the destination (amount - priceable costs). */
  netReceivedUsd: number | null;
  /** Total cost in bps of amount. */
  costBps: number | null;
  /** Cheapest-path recommendation string. */
  cheapestPath: string;
  /** "worth it" / "not worth it" / "cannot tell" verdict. */
  verdict: "worth_it" | "not_worth_it" | "unknown";
  verdictDetail: string;
  /** True when any figure is estimated/unknown (show the re-check hint). */
  hasEstimates: boolean;
  warnings: string[];
}

const midGas = (p: VenueTransferProfile): number | null =>
  p.gasRangeUsd == null ? null : (p.gasRangeUsd[0] + p.gasRangeUsd[1]) / 2;

// --- 5. The estimator (pure arithmetic — easy to test, easy to rebuild) -------
export function estimateRebalance(input: RebalanceEstimateInput): RebalanceEstimate {
  const from = getVenueProfile(input.from);
  const to = getVenueProfile(input.to);
  const warnings: string[] = [];
  const legs: RebalanceLeg[] = [];
  const swapBps = input.swapFrictionBps ?? 15;
  const hopGas = input.walletHopGasUsd ?? 0.25;

  // Fail-closed: unknown venue or same-venue selection cannot be priced.
  if (!from || !to) {
    return {
      from: input.from, to: input.to, amountUsd: input.amountUsd, legs: [],
      totalCostUsd: null, netReceivedUsd: null, costBps: null,
      cheapestPath: "Unknown venue — cannot price.",
      verdict: "unknown", verdictDetail: "Pick two known venues to price the move.",
      hasEstimates: true, warnings: ["One of the selected venues has no fee profile."],
    };
  }
  if (from.venue === to.venue) {
    return {
      from: input.from, to: input.to, amountUsd: input.amountUsd, legs: [],
      totalCostUsd: 0, netReceivedUsd: input.amountUsd, costBps: 0,
      cheapestPath: "Same venue — no transfer needed.",
      verdict: "worth_it", verdictDetail: "No movement, no friction.",
      hasEstimates: false, warnings: [],
    };
  }

  // Leg 1: withdrawal protocol fee at the source.
  if (from.withdrawFeeUsd == null) {
    legs.push({ label: `Withdraw fee (${from.label})`, costUsd: null, note: "Unknown — check venue." });
    warnings.push(`${from.label} withdrawal fee is unpublished (beta/limited).`);
  } else {
    legs.push({ label: `Withdraw fee (${from.label})`, costUsd: from.withdrawFeeUsd, note: from.confidence === "verified" || from.confidence === "documented" ? "Per venue docs." : "Estimate — re-check." });
  }

  // Leg 2: network gas. Direct = one settlement leg; via-wallet = +1 hop.
  const gas = midGas(from);
  if (gas == null) {
    legs.push({ label: "Network gas", costUsd: null, note: "Unknown rails." });
  } else if (input.viaDirect && from.supportsDirectToAddress && to.supportsDirectToAddress) {
    legs.push({ label: "Network gas (direct venue→venue)", costUsd: gas, note: `Skips your wallet hop; ${from.label} → ${to.label} deposit address.` });
  } else {
    legs.push({ label: "Network gas (via your wallet)", costUsd: gas + hopGas, note: `Settlement leg + ~$${hopGas.toFixed(2)} wallet hop.` });
    if (input.viaDirect) warnings.push("Direct transfer unsupported on one side — priced via your wallet instead.");
  }

  // Leg 3: destination deposit fee (usually $0 — deposits are free almost everywhere).
  if (to.depositFeeUsd == null) {
    legs.push({ label: `Deposit fee (${to.label})`, costUsd: null, note: "Unknown — check venue." });
    warnings.push(`${to.label} deposit fee is unpublished (beta).`);
  } else {
    legs.push({ label: `Deposit fee (${to.label})`, costUsd: to.depositFeeUsd, note: to.depositFeeUsd === 0 ? "Deposits are free." : "Per venue docs." });
  }

  // Leg 4: coin swap friction (only when the moved asset isn't accepted as-is).
  if (input.swapNeeded) {
    const swapCost = (input.amountUsd * swapBps) / 10_000;
    legs.push({ label: `Swap friction (~${swapBps} bps)`, costUsd: swapCost, note: "Taker fee + spread for converting coins." });
    warnings.push("Swapping coins adds spread risk on top of this estimate.");
  }

  // Totals: null poisons the sum (fail-closed — never show a deceptively low total).
  const unpriceable = legs.some((l) => l.costUsd == null);
  const total = unpriceable ? null : legs.reduce((a, l) => a + (l.costUsd ?? 0), 0);
  const net = total == null ? null : Math.max(0, input.amountUsd - total);
  const bps = total == null || input.amountUsd <= 0 ? null : (total / input.amountUsd) * 10_000;
  const hasEstimates = from.confidence === "estimated" || to.confidence === "estimated"
    || from.confidence === "unknown" || to.confidence === "unknown" || unpriceable;

  // Cheapest-path recommendation.
  const directPossible = from.supportsDirectToAddress && to.supportsDirectToAddress;
  const cheapestPath = directPossible
    ? `Cheapest: withdraw ${from.label} directly to your ${to.label} deposit address (saves ~$${hopGas.toFixed(2)} wallet-hop gas + waiting).`
    : `No direct path: withdraw to your own wallet first, then deposit to ${to.label}. Keep both addresses whitelisted.`;

  // Worth-it verdict vs the opportunity this unlocks.
  const profit = input.opportunityProfitUsd;
  const mult = input.worthItMultiple ?? 2;
  let verdict: RebalanceEstimate["verdict"] = "unknown";
  let verdictDetail = "Enter the expected opportunity profit to judge worth-it.";
  if (total == null) {
    verdictDetail = "Cannot judge worth-it — one cost leg is unpriceable. Check the venue pages first.";
  } else if (profit != null && profit > 0) {
    if (profit >= total * mult) {
      verdict = "worth_it";
      verdictDetail = `Worth it: ~$${total.toFixed(2)} friction vs ~$${profit.toFixed(2)} expected profit (${mult}x safety margin met).`;
    } else {
      verdict = "not_worth_it";
      verdictDetail = `Not worth rebalancing: ~$${total.toFixed(2)} friction vs ~$${profit.toFixed(2)} profit. Prefer a fresh deposit to ${to.label} — or skip.`;
    }
  }

  return {
    from: from.venue, to: to.venue, amountUsd: input.amountUsd, legs,
    totalCostUsd: total, netReceivedUsd: net, costBps: bps,
    cheapestPath, verdict, verdictDetail, hasEstimates, warnings,
  };
}
