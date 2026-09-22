# Manual Strategy Parameter Guide

This guide explains how to set parameters without treating software defaults as profitable recommendations. Arbitrageur does not ship Conservative/Balanced/Aggressive trading presets yet because the repository does not contain a sufficiently broad, reviewed outcome corpus from which to derive them honestly.

Use Paper Trading, collect venue-specific history, review AutoTune's held-out results, and change one risk dimension at a time.

## General rules

1. **Start from executable friction, not gross spread.** Entry, exit, fees, slippage, funding, and quantity conversion all matter.
2. **Unknown means blocked.** Missing depth, stale books, missing funding, provisional session data, or insufficient history must not be replaced with zero.
3. **Size to the weaker leg.** Capacity is bounded by the shallower book and the smaller available margin pool.
4. **Use leverage last.** First establish stable fills, reconciliation, and drawdown at 1x–2x in paper mode.
5. **Do not infer live readiness from a backtest.** Venue order handling and restart reconciliation are separate release gates.
6. **Treat annualized carry as a comparison, not a forecast.** Funding can change or invert before costs are recovered.

## Simple Spread

### Target Profit (BPS)

The target is the minimum expected net capture after modeled round-trip friction:

```text
required gross BPS
  = entry fees
  + expected entry slippage
  + exit fees
  + expected exit crossing/slippage
  + target profit BPS
```

Raise the target when realized paper slippage or failed/partial fills exceed the model. Lower it only when a held-out sample still has positive expected net capture after costs.

### Sliding Window

- **Manual:** Use a horizon long enough to contain multiple independent divergence/reversion episodes. A 24-hour software default covers a full crypto day but is not universally optimal.
- **Auto:** Uses qualified pair history and OU half-life gates. When evidence is insufficient, the system must display and use the manual fallback.
- Avoid tuning the window to one recent profitable episode.

### Window Percentile

A higher percentile waits for rarer, wider deviations and usually lowers trade frequency. A lower percentile increases frequency and sensitivity to noise. Validate it together with the target and friction model rather than in isolation.

### Step size and ticker equity cap

The engine derives a base-unit step from venue lot sizes, price, `stepSizeMult`, and `maxStepUsd`, then caps total ticker exposure by `maxTickerEquityPct`.

- Reduce `maxStepUsd` before increasing the number of opportunities monitored.
- Keep the ticker equity cap small enough that one unresolved pair cannot dominate the portfolio.
- If either venue cannot fill one step inside the slippage/depth guards, the step is too large regardless of account equity.

### Execution mode

- **Single-Way:** One directional divergence at a time. This is the safer default while validating a pair.
- **Double-Way:** Requires independent evidence that both directional rows mean-revert after all costs. Do not enable merely because the historical mid spread crossed zero.

### Leverage

Effective leverage is the minimum of strategy target, account cap, venue capability, and global Risk Control cap. A lower setting increases required margin but improves liquidation distance. A configured target is not proof that the venue accepted the leverage update.

## Funding carry

### Trade size

Size is per leg. For perp/perp carry, each venue must cover its own initial margin. For spot/perp basis on one venue, required capital is approximately:

```text
spot notional + perpetual initial margin + fee/slippage reserve
```

The same free-margin balance must not be counted twice.

### Minimum APR

APR is a normalized comparison of the current funding gap. It is not a guaranteed realized return. The entry hurdle should cover:

- Both legs' entry and exit fees
- Measured entry price gap
- Expected exit friction
- Funding decay/inversion risk
- Capital lock-up and transfer constraints

Use expected USD and trailing outcome distributions alongside APR.

### Maximum slippage

This bounds each leg's depth-walk slippage at dispatch. A tighter value rejects more opportunities; a wider value permits more adverse execution. Increase it only after paper fill evidence shows that the additional fills improve net expectancy.

### Maximum entry spread

This bounds the absolute execution-price gap between legs. It protects against entering a funding trade whose basis move can overwhelm expected carry. It is independent of the funding-rate hurdle and must be enforced at dispatch.

### Basis controls

- Basis rows are long spot / short perpetual only.
- The spot and perpetual symbols remain distinct.
- Reverse basis requires verified borrow/short-spot support and is not implemented.
- Keep the basis floor disabled unless its sign and units are understood: positive means the perpetual is above spot.

### Holding horizon

Break-even time assumes the current gap persists. The decay-aware expected hold is preferable but remains a model. Exit when the strategy's validated condition fires; do not hold solely to recover sunk fees.

## Triangle

A triangle is executable only when all three exact markets exist on the same venue and the full quantity can be priced through all legs. Use the minimum net-profit hurdle after three fees and three depth walks. A venue that silently maps a cross-quoted symbol to a USD market must be rejected.

## Spot and prediction

Integrated spot-arbitrage and prediction execution are disabled. Their configurator fields are research/schema placeholders and must not be interpreted as active risk controls.

## AutoTune

AutoTune uses a chronological fit/validation split and applies only rows marked `READY`. `HELD` means the current manual value remains authoritative.

Before applying:

- Confirm the symbol and venue pair.
- Check observation count and date span.
- Read both fit and validation metrics.
- Reject recommendations based on one regime or thin depth.
- Apply selected rows only; do not change every parameter merely because one row is READY.
- Preserve the audit ID for later outcome review.

AutoTune does not yet learn from a complete join of quoted opportunities to realized paper fills/slippage. Treat recommendations as research assistance, not an autonomous mandate.

## Current software defaults

These values initialize missing settings; they are not universal recommendations:

| Field | Default |
|---|---:|
| Target profit | 1 BPS |
| Window percentile | 50 |
| Sliding window | Manual, 24 hours |
| Step-size multiplier | 10 |
| Maximum step | $2,500 |
| Maximum ticker equity | 20% |
| Target leverage | 5x |
| Futures max slippage | 15 BPS |
| Funding trade size | $25,000 per leg |
| Funding minimum APR | 8% |
| Funding max slippage | 15 BPS |
| Funding max entry spread | 50 BPS |
| Spot trade size | $1,000 |
| Spot max slippage | 5 BPS |
| Spot minimum net spread | 15 BPS |

Review and reduce these values for the actual account before any paper run. New live strategy entries are disabled regardless of these settings.

## Why fixed risk presets are still gated

A named preset creates an implied recommendation. To publish one responsibly, the project needs:

- A versioned source dataset
- Venue and asset-class coverage
- Held-out outcome metrics after fees/slippage
- Drawdown and unresolved-leg statistics
- Stability across sessions and regimes
- Documented sample thresholds and invalidation rules

Until that evidence exists, the honest cold-start behavior is a clearly labeled software default plus manual guidance, not fabricated risk tiers.
