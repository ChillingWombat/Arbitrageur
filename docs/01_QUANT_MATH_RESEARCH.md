# Quantitative Finance and Mathematical Foundations of Arbitrage

This research document details the rigorous mathematical, probabilistic, and microstructural foundations of the **Arbitrager** cross-market trading engine.

---

## 1. Axiomatic Foundations of Arbitrage

### 1.1 Law of One Price and No-Arbitrage Principle
In frictionless capital markets, two securities or portfolios with identical payoff profiles in all possible future states of nature must trade at the identical price:
$$\text{Payoff}(A, \omega) = \text{Payoff}(B, \omega) \quad \forall \, \omega \in \Omega \implies P_A(t) = P_B(t)$$

Under the **First Fundamental Theorem of Asset Pricing**, the absence of arbitrage is mathematically equivalent to the existence of at least one equivalent martingale measure (risk-neutral probability measure $\mathbb{Q}$) such that discounted asset prices are $\mathbb{Q}$-martingales:
$$S(t) = \mathbb{E}^\mathbb{Q} \left[ e^{-\int_t^T r(u)du} S(T) \;\Big|\; \mathcal{F}_t \right]$$

In real-world decentralized and electronic financial markets, structural frictions violate instantaneous parity:
1. **Network & Protocol Latency:** Discrete packet arrival times ($t_{\text{recv}, A} \neq t_{\text{recv}, B}$).
2. **Transaction Costs:** Taker/maker fee schedules ($f_A, f_B$), protocol gas costs ($G$), and slippage.
3. **Fragmented Liquidity & Capital Segmentation:** Disjoint collateral pools requiring distinct margin reserves.

---

## 2. Spatial Arbitrage (Pure Price Difference)

### 2.1 Gross vs. Net Spread in Basis Points (BPS)
Let Venue $A$ have orderbook bids $B_A$ and asks $A_A$, and Venue $B$ have bids $B_B$ and asks $A_B$.

An instantaneous spatial arbitrage exists in direction $A \to B$ (Buy at Venue $A$, Sell at Venue $B$) if:
$$\text{Gross Spread} = B_B - A_A > 0$$

Expressed in basis points (1 BPS = $10^{-4} = 0.01\%$):
$$\text{Spread}_{\text{BPS}} = \left( \frac{B_B - A_A}{A_A} \right) \times 10{,}000$$

Accounting for exchange taker fee ratios $f_A, f_B$ and execution slippage $\delta_A, \delta_B$:
$$\Pi_{\text{net}} = B_B (1 - f_B - \delta_B) - A_A (1 + f_A + \delta_A)$$

Arbitrage condition:
$$\Pi_{\text{net}} > 0 \iff \frac{B_B}{A_A} > \frac{1 + f_A + \delta_A}{1 - f_B - \delta_B}$$

Using the first-order Taylor expansion for small fees ($f_A, f_B \ll 1$):
$$\text{Spread}_{\text{BPS}} > (f_A + f_B + \delta_A + \delta_B) \times 10{,}000$$

### 2.2 Orderbook Depth Integration & VWAP Execution
Real orders consume finite market depth. Given an orderbook with ask levels $\{(p_i, q_i)\}_{i=1}^M$, the volume-weighted average price (VWAP) for executing target order size $Q$ is:
$$\text{VWAP}_{\text{buy}}(Q) = \frac{1}{Q} \sum_{i=1}^k p_i \cdot \min\left(q_i, Q - \sum_{j=1}^{i-1} q_j\right)$$
where $k = \min \left\{ m : \sum_{i=1}^m q_i \ge Q \right\}$.

The true expected slippage is:
$$\text{Slippage}(Q) = \frac{\text{VWAP}_{\text{buy}}(Q) - p_1}{p_1}$$

Our engine evaluates orderbooks by evaluating the continuous liquidity integral on both legs before dispatching orders.

---

## 3. Funding Rate & Cost-of-Carry Arbitrage

### 3.1 Perpetual Contract Pricing & Funding Mechanism
Unlike traditional futures which expire and converge to spot price at maturity $T$, perpetual contracts have no expiration. Price anchor to spot is enforced via a periodic cash settlement called the **Funding Rate** $F_t$.

The funding payment transferred from Longs to Shorts (or vice-versa) per unit position $Q$ over period $\Delta t$ is:
$$\text{Cashflow}_{\text{funding}} = - Q \times P_{\text{mark}} \times F_t$$
- If $F_t > 0$: Longs pay Shorts.
- If $F_t < 0$: Shorts pay Longs.

The funding rate is calculated from the premium index:
$$P_t = \frac{\max(0, \text{ImpactBid} - \text{Price}_{\text{index}}) - \max(0, \text{Price}_{\text{index}} - \text{ImpactAsk})}{\text{Price}_{\text{index}}}$$
$$F_t = \text{clamp}\left(P_t + \text{clamp}(r - P_t, -0.05\%, +0.05\%), -0.75\%, +0.75\%\right)$$

### 3.2 Cross-Exchange Funding Divergence (Perp-Perp Carry)
Consider two perpetual venues: Venue $A$ (e.g. Hyperliquid, continuous 1h funding rate $F_A$) and Venue $B$ (e.g. Lighter or Vanta, 8h funding rate $F_B$).

To compare rates across disparate intervals, all funding rates are normalized to continuous hourly rates:
$$r_{A,\text{hourly}} = \frac{F_A}{\Delta t_A}, \quad r_{B,\text{hourly}} = \frac{F_B}{\Delta t_B}$$

If $r_{A,\text{hourly}} > r_{B,\text{hourly}}$:
1. **Short Perp on Venue A** (receives $r_{A,\text{hourly}}$)
2. **Long Perp on Venue B** (pays $r_{B,\text{hourly}}$)
3. **Net Portfolio Delta:** $\Delta_{\text{net}} = Q_B - Q_A = 0$ (Delta Neutral)

**Net Hourly Carry Rate:**
$$\Delta r_{\text{hourly}} = r_{A,\text{hourly}} - r_{B,\text{hourly}} > 0$$

### 3.3 Annualized Compounded Yield (APY)
$$\text{APR}_{\text{net}} = \Delta r_{\text{hourly}} \times 24 \times 365 = 8{,}760 \times \Delta r_{\text{hourly}}$$
$$\text{APY}_{\text{net}} = \left( 1 + \Delta r_{\text{hourly}} \right)^{8760} - 1$$

### 3.4 Break-Even Holding Period
Entry and exit incur four trading fees (Open A, Open B, Close A, Close B):
$$\text{Total Fee BPS} = f_{A,\text{open}} + f_{B,\text{open}} + f_{A,\text{close}} + f_{B,\text{close}}$$
The minimum duration $T_{\text{breakeven}}$ (in hours) before the trade achieves net profitability is:
$$T_{\text{breakeven}} = \frac{\text{Total Fee Ratio}}{\Delta r_{\text{hourly}}}$$

Example:
- Total round-trip fees = 7 BPS = $0.0007$
- Hyperliquid hourly rate = $0.03\% = 0.0003$
- Lighter normalized hourly rate = $0.01\% / 8 = 0.0000125$
- $\Delta r = 0.0003 - 0.0000125 = 0.0002875$ / hour (APR = $251.8\%$)
- $T_{\text{breakeven}} = \frac{0.0007}{0.0002875} \approx 2.43\text{ hours}$
After only 2.43 hours, all transaction friction is amortized, and the trade generates pure risk-free yield.

---

## 4. Statistical Arbitrage & Mean-Reverting Processes

### 4.1 Cointegration: Engle-Granger Representation
Two non-stationary $I(1)$ price series $Y_t$ and $X_t$ are cointegrated $CI(1, 1)$ if there exists a linear combination $\beta$ such that the spread:
$$S_t = Y_t - \alpha - \beta X_t \sim I(0)$$
is stationary with constant mean and finite variance.

### 4.2 Online Dynamic Hedge Ratio: 2-Dimensional Kalman Filter
Static OLS fails in live trading because hedge ratios drift over time. We implement an online state-space recursive Kalman Filter:
- **State Transition Equation:**
  $$\begin{bmatrix} \alpha_t \\ \beta_t \end{bmatrix} = \begin{bmatrix} \alpha_{t-1} \\ \beta_{t-1} \end{bmatrix} + \mathbf{w}_t, \quad \mathbf{w}_t \sim \mathcal{N}(0, \mathbf{Q})$$
- **Measurement Equation:**
  $$Y_t = \begin{bmatrix} 1 & X_t \end{bmatrix} \begin{bmatrix} \alpha_t \\ \beta_t \end{bmatrix} + v_t, \quad v_t \sim \mathcal{N}(0, R)$$
- **Recursive State Update:**
  $$\hat{Y}_t = \alpha_{t|t-1} + \beta_{t|t-1} X_t$$
  $$e_t = Y_t - \hat{Y}_t \quad (\text{Innovation Residual})$$
  $$S_t = \mathbf{H}_t \mathbf{P}_{t|t-1} \mathbf{H}_t^T + R$$
  $$\mathbf{K}_t = \mathbf{P}_{t|t-1} \mathbf{H}_t^T S_t^{-1}$$
  $$\mathbf{\theta}_t = \mathbf{\theta}_{t|t-1} + \mathbf{K}_t e_t$$
  $$\mathbf{P}_t = (\mathbf{I} - \mathbf{K}_t \mathbf{H}_t) \mathbf{P}_{t|t-1}$$

### 4.3 Continuous Ornstein-Uhlenbeck (OU) Mean-Reverting Process
The spread $S_t$ satisfies the continuous stochastic differential equation:
$$dS_t = \theta (\mu - S_t) dt + \sigma dW_t$$
where:
- $\theta > 0$: Rate of mean reversion
- $\mu$: Long-term equilibrium mean
- $\sigma > 0$: Diffusion volatility
- $W_t$: Standard Brownian motion

**Discrete AR(1) Mapping:**
$$S_k = a S_{k-1} + b + \epsilon_k, \quad \epsilon_k \sim \mathcal{N}(0, \sigma_\epsilon^2)$$
$$a = e^{-\theta \Delta t} \implies \theta = -\frac{\ln a}{\Delta t}$$
$$\mu = \frac{b}{1 - a}$$
$$\sigma = \sigma_\epsilon \sqrt{\frac{2\theta}{1 - a^2}}$$

**Half-Life of Mean Reversion:**
$$t_{1/2} = \frac{\ln 2}{\theta}$$

### 4.4 Bertram (2010) Optimal Analytic Thresholds
Bertram formulated trade entry at spread level $a$ and exit at equilibrium $m = \mu$ to maximize the expected return per unit time under transaction cost $c$:
$$\max_d \frac{d - c}{\mathbb{E}[T(d)]}$$
Using the first-passage time distribution of the Ornstein-Uhlenbeck process:
$$\mathbb{E}[T(d)] = \frac{\pi}{\theta} \left( \frac{d}{\sigma/\sqrt{2\theta}} \right)^2 + \mathcal{O}(d^4)$$
The optimal entry distance $d^*$ satisfies the condition where the marginal trade return equals the trade rate acceleration.

---

## 5. Prediction Market Synthetic Arbitrage

### 5.1 Binary Arrow-Debreu Securities
A binary prediction market outcome $E$ pays out $\$1.00$ if event $E$ occurs, and $\$0.00$ otherwise:
$$\text{Payout}(E) = \mathbf{1}_{E}$$
$$\text{Payout}(\neg E) = \mathbf{1}_{\neg E}$$
Since outcomes are mutually exclusive and collectively exhaustive:
$$\mathbf{1}_E + \mathbf{1}_{\neg E} = 1.00 \implies P(E) + P(\neg E) = 1.00$$

### 5.2 Synthetic Cross-Market Parity (Polymarket vs Kalshi)
Let Venue $A$ (Polymarket) and Venue $B$ (Kalshi) trade contracts on the identical real-world event $E$:
- Strategy 1: Buy YES on Venue $A$ at $P_{A,\text{yes}}$, Buy NO on Venue $B$ at $P_{B,\text{no}}$.
  $$\text{Total Cost} = P_{A,\text{yes}} + P_{B,\text{no}}$$
  $$\text{Payout} = \mathbf{1}_E + \mathbf{1}_{\neg E} \equiv \$1.00$$
  $$\Pi_{\text{net}} = \$1.00 - (P_{A,\text{yes}} + P_{B,\text{no}}) - \text{Fees}$$
- Strategy 2: Buy YES on Venue $B$ at $P_{B,\text{yes}}$, Buy NO on Venue $A$ at $P_{A,\text{no}}$.
  $$\Pi_{\text{net}} = \$1.00 - (P_{B,\text{yes}} + P_{A,\text{no}}) - \text{Fees}$$

If either $\Pi_{\text{net}} > 0$, risk-free synthetic arbitrage is guaranteed at settlement regardless of which outcome occurs!

---

## 6. Risk Budgeting & Position Sizing

### 6.1 Continuous Kelly Criterion
For an arbitrage strategy with expected return $\mu$, borrowing rate $r$, and variance $\sigma^2$:
$$f^* = \frac{\mu - r}{\sigma^2}$$
To protect against model parameter uncertainty, fat-tailed slippage, and execution jitter, our system enforces **Half-Kelly**:
$$f_{\text{safe}} = 0.5 \times f^*$$

### 6.2 Liquidation Proximity Buffer
For a leveraged perpetual position with maintenance margin ratio $\text{MMR}$, the distance to liquidation is:
$$\text{Distance}_{\text{long}} = \frac{P_{\text{mark}} - P_{\text{liq}}}{P_{\text{mark}}} = \frac{\text{Equity} - \text{Initial Margin}}{\text{Notional}} + (1 - \text{MMR})$$
Trading is automatically gated if distance falls below the safe $15\%$ threshold.
