# Quantitative Research: MoB Strategy Vaults, RWA Equity Carry & Productive Hold

This document captures the quantitative mechanics, architectural patterns, and risk-management specifications of the **MoB (Mobius)** cross-venue arbitrage and automated strategy vault platform, reverse-engineered and adapted for the **Arbitrager** high-frequency quantitative execution engine.

---

## 1. Executive Summary & Platform Architecture

MoB provides two core trading paradigms:
1. **Self-Directed Algorithmic Strategies**: The trader defines execution sizing, pair selection, and parameters; the engine automatically splits, executes, and routes orders across venues to capture cross-DEX dislocations.
2. **Automated Strategy Vaults**: Passive "deposit once" vaults where the protocol manages collateral, single-borrow credit facilities, spot tokenized stock purchases, perp short hedging, and funding accrual at net-zero delta.

```
                   ┌─────────────────────────────────────────┐
                   │             Trader / Vault              │
                   └────────────────────┬────────────────────┘
                                        │
                 ┌──────────────────────┴──────────────────────┐
                 ▼                                             ▼
   [Self-Directed Strategies]                     [Automated Strategy Vaults]
   • Delta-Neutral Perp/Perp                      • Leveraged RWA Carry Trade
     - Long Venue A (Hyperliquid)                   - USDG Collateral -> Borrow up to 5x
     - Short Venue B (RH Lighter)                   - Buy Stock Tokens (Robinhood Chain)
     - Margin Rebalancing Coordinator               - Short Perp (Lighter / Hyperliquid)
                                                  • Productive Hold
                                                    - Deposit Equities (NVDA, TSLA)
                                                    - Borrow USDG within LTV
                                                    - Carry Overlay on Borrowed Capital
```

---

## 2. Strategy 1: Self-Directed Delta-Neutral Perp / Perp

### 2.1 Core Thesis & Mechanics
Capture sustained funding rate differentials between two perpetual DEX venues without taking directional market risk ($\Delta \approx 0$).

- **Leg 1 (Long)**: Established on the DEX with the lower or more negative funding rate (e.g., Hyperliquid).
- **Leg 2 (Short)**: Established on the DEX with the higher positive funding rate (e.g., Robinhood Lighter).
- **Return Source**: Funding spread paid from the short leg minus funding paid (or plus funding received) on the long leg.

```
       ┌──────────────────┐                     ┌──────────────────┐
       │   Hyperliquid    │                     │    RH Lighter    │
       │      (LONG)      ├─────── Δ ≈ 0 ───────┤     (SHORT)      │
       └──────────────────┘                     └──────────────────┘
                 │                                       │
                 └────────────── Funding Spread ─────────┘
                                 (Net Yield)
```

### 2.2 Mathematical Model

Let:
- $F_{\text{long}}$: Hourly funding rate on Long venue (annualized as $\text{APR}_{\text{long}} = F_{\text{long}} \times 24 \times 365$).
- $F_{\text{short}}$: Hourly funding rate on Short venue (annualized as $\text{APR}_{\text{short}} = F_{\text{short}} \times 24 \times 365$).
- $V$: Notional position value per leg (USD).
- $c_{\text{taker}, 1}, c_{\text{taker}, 2}$: Round-trip taker fee rate on venues 1 and 2.
- $S_1, S_2$: Bid-ask spread friction upon entering and exiting each leg.

#### Gross Funding Yield:
$$\text{Yield}_{\text{gross}} = V \times (F_{\text{short}} - F_{\text{long}})$$

#### Net PnL over Holding Period $T$ (hours):
$$\Pi(T) = \sum_{t=1}^{T} V_t \cdot \left( F_{\text{short}, t} - F_{\text{long}, t} \right) - V_0 \cdot \left( c_{\text{taker}, 1} + c_{\text{taker}, 2} + \frac{S_1 + S_2}{2} \right)$$

#### Break-Even Duration ($T^*$):
$$T^* = \frac{c_{\text{taker}, 1} + c_{\text{taker}, 2} + \frac{S_1 + S_2}{2}}{\bar{F}_{\text{short}} - \bar{F}_{\text{long}}}$$

### 2.3 Critical Operational Challenge: Margin Drift & Asymmetric Depletion
Because the two legs are held on separate isolated exchanges, a strong directional trend will cause one leg to accrue unrealized profit ($+\text{uPnL}$) while the other leg incurs unrealized loss ($-\text{uPnL}$).
- If price rallies strongly: The Short leg incurs loss and approaches its liquidation price, while the Long leg accumulates excess equity on the other exchange.
- **Mitigation Strategy**: The Arbitrager engine tracks margin usage ratios:
  $$\text{MarginRatio}_v = \frac{\text{MaintenanceMargin}_v}{\text{Equity}_v}$$
  When $\text{MarginRatio}_v > \theta_{\text{rebalance}}$ (e.g. 70%), trigger an automated transfer or partial de-leveraging of both legs to reset margin buffers.

---

## 3. Strategy 2: Leveraged RWA Carry Trade (Automated Strategy Vault)

### 3.1 Overview & Specifications
- **Vault Type**: Automated Strategy Vault
- **Target APY**: Variable depending on equity funding and USDG borrow cost (typically 18%–35% net APY at $5\times$ leverage)
- **Max Leverage**: $5\times$
- **Deposit Asset**: USDG (Global Dollar stablecoin)
- **Target Equities**: Tokenized stock tokens (e.g., NVDA, TSLA, AAPL) on Robinhood Chain
- **Hedging DEX**: Lighter / Hyperliquid perpetual futures

### 3.2 Workflow (4-Step Lifecycle)
1. **01 Deposit**:
   - The user deposits USDG into the protocol's credit account as collateral.
2. **02 Borrow**:
   - The vault executes a single borrow of USDG against the collateral up to $5\times$ effective leverage.
   - **Crucial Architecture**: Single borrow, **no recursive looping** (avoids flash loan and circular liquidation cascades).
3. **03 Deploy**:
   - With the leveraged USDG pool ($5 \times C_{\text{USDG}}$), the vault buys spot tokenized stock tokens on **Robinhood Chain** (Arbitrum L2 EVM router).
   - Simultaneously executes an atomic perp **short** on **Lighter** or **Hyperliquid** for an identical notional size.
4. **04 Earn**:
   - Funding on the equity perp short accrues continuously to the credit account while net portfolio delta remains $\Delta \approx 0$.

### 3.3 Quantitative APY Formulation

Let:
- $C$: Initial USDG deposit.
- $L$: Leverage multiple ($L \in [1, 5]$, default $L = 5$).
- $B$: Borrowed USDG amount, where $B = C \cdot (L - 1)$.
- $R_{\text{funding}}$: Annualized funding rate received on the short equity perp position ($\text{APR}_{\text{funding}}$).
- $R_{\text{borrow}}$: Annualized borrowing interest rate charged for USDG ($\text{APR}_{\text{borrow}}$).
- $C_{\text{fees}}$: Annualized total fees (execution friction, router slippage, protocol performance fee).

#### Gross Position Notional:
$$N = C \cdot L$$

#### Annualized Net Return on Equity ($\text{ROE}$ / Net APY):
$$\text{APY}_{\text{net}} = \frac{N \cdot R_{\text{funding}} - B \cdot R_{\text{borrow}} - N \cdot C_{\text{fees}}}{C}$$

Substituting $N = C \cdot L$ and $B = C \cdot (L - 1)$:
$$\text{APY}_{\text{net}} = L \cdot R_{\text{funding}} - (L - 1) \cdot R_{\text{borrow}} - L \cdot C_{\text{fees}}$$

#### Numerical Example:
- Deposit: $C = \$10,000$ USDG
- Leverage: $L = 5\times \implies N = \$50,000$, Borrowed $B = \$40,000$
- Equity Perp Funding APR: $R_{\text{funding}} = 12.0\%$
- USDG Borrow APR: $R_{\text{borrow}} = 5.5\%$
- Fee drag: $C_{\text{fees}} = 0.5\%$
- Calculation:
  $$\text{APY}_{\text{net}} = 5 \times 12.0\% - 4 \times 5.5\% - 5 \times 0.5\% = 60.0\% - 22.0\% - 2.5\% = \mathbf{35.5\%}$$

The leverage magnifies the funding yield substantially as long as the funding rate exceeds the USDG borrow rate:
$$R_{\text{funding}} > \frac{L - 1}{L} \cdot R_{\text{borrow}} + C_{\text{fees}}$$

For $L = 5$, the hurdle rate is:
$$R_{\text{funding}} > 0.8 \cdot R_{\text{borrow}} + C_{\text{fees}}$$

---

## 4. Strategy 3: Productive Hold (Directional Collateral + Carry Overlay)

### 4.1 Overview & Specifications
- **Vault Type**: Automated Directional Collateral + Carry Overlay Vault
- **Target APY**: Market equity appreciation + 8%–18% additional carry overlay
- **Deposit Assets**: Tokenized US Equities (NVDA, TSLA, AAPL, MSFT)
- **Collateral Policy**: The investor retains **100% long exposure** to the underlying stock. The equity position is never sold or shorted against directly.

### 4.2 Workflow (4-Step Lifecycle)
1. **01 Deposit**:
   - Investor deposits existing tokenized stocks (e.g., $Q_{\text{stock}}$ shares of NVDA) into the Mobius cross-margin facility.
   - The investor stays 100% exposed to stock price appreciation: $\Delta_{\text{collateral}} = 1.0$.
2. **02 Borrow**:
   - The vault borrows USDG against the equity collateral within a safe Loan-To-Value ratio ($\text{LTV} \in [0.30, 0.50]$).
3. **03 Carry**:
   - The borrowed USDG is deployed into an independent, leveraged delta-neutral carry trade:
     - Buy spot tokenized asset on Robinhood Chain.
     - Short matching perp on supported DEX (Lighter / Hyperliquid).
     - Net delta of carry overlay: $\Delta_{\text{carry}} \approx 0$.
4. **04 Earn**:
   - Total Net Payoff combines full equity price appreciation plus stacked net carry yield minus borrow cost.

### 4.3 Quantitative Payoff Model

Let:
- $P_0$: Initial stock price at time of deposit.
- $P_t$: Stock price at time $t$.
- $Q$: Number of tokenized shares held.
- $V_0 = Q \cdot P_0$: Initial portfolio equity value.
- $\text{LTV}$: Loan-to-value ratio for borrowing USDG ($\text{LTV} = \frac{B_{\text{USDG}}}{V_t} \le \text{LTV}_{\text{max}}$).
- $L_{\text{carry}}$: Leverage applied to the borrowed carry trade ($L_{\text{carry}} \in [1, 3]$).
- $Y_{\text{carry}}$: Net funding yield generated by the carry overlay.
- $R_{\text{borrow}}$: USDG borrow rate.

#### Portfolio Delta:
$$\Delta_{\text{total}} = \Delta_{\text{collateral}} + \Delta_{\text{carry}} = 1.0 + 0.0 = 1.0$$
The user retains full unhedged exposure to the equity market.

#### Total Net Payoff Equation:
$$\Pi_{\text{total}}(t) = Q \cdot (P_t - P_0) + \left( \text{LTV} \cdot V_0 \right) \cdot \left[ L_{\text{carry}} \cdot Y_{\text{carry}} - R_{\text{borrow}} \right] \cdot \frac{t}{365}$$

$$\frac{\Pi_{\text{total}}(t)}{V_0} = \frac{P_t - P_0}{P_0} + \text{LTV} \cdot \left[ L_{\text{carry}} \cdot Y_{\text{carry}} - R_{\text{borrow}} \right] \cdot \frac{t}{365}$$

#### Comparative Advantage:
- **Traditional Equity Holding**: $\text{Return} = \frac{P_t - P_0}{P_0}$
- **Productive Hold**: $\text{Return} = \frac{P_t - P_0}{P_0} + \underbrace{\alpha_{\text{carry}}}_{\text{Non-dilutive cash flow yield}}$

---

## 5. Comprehensive Risk & Sensitivity Analysis

| Risk Vector | Description | Failure Mode | Engineered Mitigation |
| :--- | :--- | :--- | :--- |
| **Funding Inversion** | Perp funding rate flips negative (shorts pay longs). | Vault accumulates negative carry, eroding capital. | Automated Funding Circuit Breaker: If 7-day EMA of funding spread $< R_{\text{hurdle}}$, unwind the carry leg to cash. |
| **Market Closed Hours & Oracle Stale Price** | TradFi equities trade 09:30–16:00 EST. Crypto perps trade 24/7. | Weekend/night news events cause perp to trade at large premium/discount to closed spot. | Band-based slippage guard: Restrict large rebalancing during US market closed hours; rely on TWAP oracles with max staleness thresholds. |
| **Equity Price Crash (Productive Hold LTV)** | Tokenized stock crashes 30%+ in rapid succession. | Collateral value falls, pushing LTV towards liquidation threshold ($\text{LTV} \to 80\%$). | Dynamic De-leveraging: Pre-set stop-borrow limit. If LTV hits 60%, unwind a portion of the carry overlay and repay borrowed USDG. |
| **USDG De-Peg Risk** | USDG stablecoin trades below $0.98. | Dislocation between borrow debt denomination and collateral valuation. | Multi-stablecoin parity check against USDC/USDT before large borrows or settlements. |
| **Single-Venue Execution Latency** | One leg fills while the other leg rejects (e.g. rate limit). | Temporary directional exposure ($\Delta \neq 0$). | Atomic double-leg or IOC/FOK execution with immediate fallback unwind if second leg fails within 150ms. |

---

## 6. Implementation Roadmap for Arbitrager Engine

### Phase 1: Core Telemetry & Feeds (Complete / In Progress)
- [x] Hyperliquid Perpetual L1 API and WebSocket connectors.
- [x] Lighter zkDEX Rollup connector with orderbook synchronization.
- [x] Vanta (Orderly Network) and Robinhood EVM DEX router integration.
- [x] Real-time Dislocation Matrix with sliding window rolling baseline and fee deduction.

### Phase 2: RWA & Tokenized Equities Support
- [ ] Connect Pyth / Chainlink high-frequency tokenized stock oracle feeds (NVDA-USD, TSLA-USD, AAPL-USD).
- [ ] Monitor USDG borrow rates on Robinhood Chain lending pools (Aave/Compound fork or native credit pools).
- [ ] Compute real-time Net APY for Leveraged RWA Carry Trade ($5\times$).

### Phase 3: Vault Execution Automation
- [ ] Implement `CarryVaultCoordinator` in Rust (`crates/core/src/carry_vault.rs`):
  - Collateral deposit validation.
  - Borrow execution and spot buy on Robinhood Router.
  - Parallel perp short placement on Lighter.
  - Periodic automated funding harvest and margin health monitoring.
