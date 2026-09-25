# Options Arbitrage Strategies: From Classical Theory to Microstructure & Small-Capacity Edges

## 1. Executive Summary & Taxonomy Matrix

In quantitative finance, an **arbitrage** is strictly defined as an investment strategy that generates non-negative cash flows in all possible states of the world, with a strictly positive probability of a positive return, without requiring net initial capital (or earning a return strictly in excess of the risk-free rate on pledged capital).

While institutional market makers (Citadel Securities, Jane Street, Susquehanna, Optiver, IMC) aggressively arbitrage high-capacity, high-volume options on major indices (SPX, NDX, EuroStoxx), **structural market frictions prevent large institutions from capturing niche, low-capacity arbitrage opportunities**. These institutional barriers include:
1. **Capacity Constraints**: An opportunity with a capacity of \$5,000 to \$50,000 cannot move the needle for a multi-billion dollar hedge fund.
2. **Mandate & Regulatory Restrictions**: Strict risk limits on illiquid underlyings, hard-to-borrow fees, and non-standard deliverables.
3. **Execution Architecture**: Retail price improvement and payment for order flow (PFOF) allow retail orders to cross at the bid-ask midpoint in Complex Order Books (COBs), whereas institutional exchange-bound orders face maker-taker fee hurdles.

This document catalogs options arbitrage strategies across two broad spectra:
- **Classical / Mathematical Boundary Arbitrages** (pure pricing identities and convexity violations).
- **Microstructure & Small-Capacity Niche Arbitrages** (structural inefficiencies, corporate actions, loan fees, and operational quirks).

### 1.1 Strategy Comparison Matrix

| Strategy Category | Specific Strategy | Primary Driver | Capacity per Opportunity | Target Execution Style | Real-World Risk Factor |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **Mathematical** | **Conversion / Reversal** | Put-Call Parity violation | High (Indices) / Low (Small Cap) | Simultaneous 3-leg fill | Pin risk, early exercise |
| **Mathematical** | **Box Spread** | Synthetic bond rate $\ne$ Risk-free rate | High on European, Nil on American | Multi-leg COB limit order | American-style early assignment |
| **Mathematical** | **Jelly Roll** | Synthetic dividend/interest term-structure | Medium to Low | Multi-leg spread | Dividend adjustment risk |
| **Mathematical** | **Negative-Risk Butterfly / Condor** | Upper boundary violation ($Credit > Width$) | Tiny (\$5k – \$30k) | Patient passive limit in LEAPS | Early exercise on deep ITM short put |
| **Microstructure** | **Dividend Play / Assignment Arbitrage** | Retail non-exercise of deep ITM calls | Small to Medium (\$20k – \$200k) | Ex-dividend eve box/spread | Assignment lottery variance |
| **Microstructure** | **Hard-to-Borrow (HTB) Rebate Arb** | Skewed synthetic forward vs stock borrow cost | Small (\$10k – \$100k) | Reverse conversion + locate | Recall risk, borrow fee spike |
| **Microstructure** | **OCC Adjusted Contract Arbitrage** | Spin-off / Special dividend mispricing | Tiny (\$2k – \$20k) | Basket deliverable vs combo | Stale quotes, extreme illiquidity |
| **Microstructure** | **M&A / Cash Merger Pinning** | Lottery-ticket demand on closed deals | Small to Medium (\$10k – \$50k) | Out-of-the-money bear call spread | Deal termination / regulatory block |
| **Microstructure** | **Odd-Lot Tender Offer Option Hedge** | SEC Rule 13e-4 retail-only priority | Micro (\$1k – \$5k per account) | Stock purchase + synthetic short | Corporate tender terms revision |
| **Microstructure** | **4:00–5:30 PM Late Exercise Arb** | Post-market price moves vs expiry strikes | Variable | After-hours stock short + contrary notice | Broker cutoff times, margin call |
| **Microstructure** | **Crypto Option vs Perp Funding Arb** | Options forward basis vs Perp funding rate | Small to Medium (\$10k – \$250k) | Long synthetic forward + Short perp | Counterparty risk, funding spike |

---

## 2. Classical & Mathematical Options Arbitrage

### 2.1 Put-Call Parity Violations: Conversions & Reversals

Put-Call Parity defines the relationship between European calls, puts, and the underlying forward:
$$C(S, K, T) - P(S, K, T) = S_0 - K \cdot e^{-rT} - \text{PV}(D)$$

Where:
- $C$: Call premium
- $P$: Put premium
- $S_0$: Current spot price
- $K$: Strike price
- $r$: Risk-free interest rate
- $\text{PV}(D)$: Present value of expected dividends over horizon $T$

#### 1. The Conversion (Long Stock + Long Put + Short Call)
* **Execution**: Buy 100 shares of stock $S_0$, Buy 1 Put at strike $K$, Sell 1 Call at strike $K$.
* **Payoff at Expiration**: Exactly $K + D$.
* **Condition for Arbitrage**:
  $$S_0 - C + P < K \cdot e^{-rT} + \text{PV}(D)$$
* **Intuition**: If the Call is overpriced relative to the Put, selling the call and buying the put synthetic short against long stock locks in an annualized return higher than the risk-free rate.

#### 2. The Reverse Conversion / Reversal (Short Stock + Short Put + Long Call)
* **Execution**: Short 100 shares of stock $S_0$, Sell 1 Put at strike $K$, Buy 1 Call at strike $K$.
* **Payoff at Expiration**: Obligation to buy back stock at strike $K$.
* **Condition for Arbitrage**:
  $$C - P - S_0 > -K \cdot e^{-rT} - \text{PV}(D)$$
* **Intuition**: Synthetic long stock (Long Call + Short Put) financed by shorting physical stock. Yields borrowing at rates lower than the money market (or a negative cost of capital).

---

### 2.2 Box Spread Arbitrage (The 4-Leg Synthetic Zero-Coupon Bond)

A Box Spread is the combination of a **Bull Call Spread** and a **Bear Put Spread** across two strikes $K_1$ and $K_2$ ($K_2 > K_1$):
* **Leg 1**: Long Call at $K_1$
* **Leg 2**: Short Call at $K_2$
* **Leg 3**: Long Put at $K_2$
* **Leg 4**: Short Put at $K_1$

#### Mathematical Payoff
At expiration $T$, regardless of the underlying price $S_T$:
- If $S_T \le K_1$: Call spread pays \$0; Put spread pays $(K_2 - S_T) - (K_1 - S_T) = K_2 - K_1$.
- If $S_T \ge K_2$: Call spread pays $K_2 - K_1$; Put spread pays \$0.
- If $K_1 < S_T < K_2$: Call spread pays $S_T - K_1$; Put spread pays $K_2 - S_T$. Total payout: $(S_T - K_1) + (K_2 - S_T) = K_2 - K_1$.

The final cash flow is strictly deterministic:
$$\text{Payoff}(T) = K_2 - K_1$$

#### Arbitrage Rule
The theoretical fair price $B_0$ is:
$$B_0 = (K_2 - K_1) \cdot e^{-rT}$$
- If the market price $P_{\text{box}} < B_0$: **Buy the Box** (Lend money synthetically at a rate higher than Treasuries).
- If the market price $P_{\text{box}} > B_0$: **Sell the Box** (Borrow money synthetically at a rate cheaper than commercial broker margin).

> [!CAUTION]
> **The American Option Fatal Flaw**: Box spreads are only risk-free on **European-style, cash-settled index options** (e.g., SPX, NDX, RUT). Executing a box spread on American-style single-stock options (e.g., AAPL, TSLA) exposes the trader to catastrophic **early assignment risk** on the short in-the-money legs, creating unhedged stock positions and massive margin liquidation events (the infamous Reddit 1R0NYMAN collapse).

---

### 2.3 Jelly Roll Arbitrage (Calendar Box Spread)

A Jelly Roll trades the synthetic forward between two expiration cycles $T_1$ and $T_2$ ($T_2 > T_1$):
$$\text{Jelly Roll} = \left[ C(K, T_1) - P(K, T_1) \right] - \left[ C(K, T_2) - P(K, T_2) \right]$$

This is equivalent to a Long Synthetic Forward at $T_1$ combined with a Short Synthetic Forward at $T_2$.
* **Economic Value**: Captures the pure cost-of-carry (interest rate minus dividend yield) between two dates without holding physical shares:
  $$\text{Value} = K \left( e^{-r T_1} - e^{-r T_2} \right) + \left[ \text{PV}_{T_2}(D) - \text{PV}_{T_1}(D) \right]$$
* **Arbitrage Opportunity**: Discrepancies between the term structure of interest rates implied by the option rolls versus Fed Funds / SOFR futures, or anticipated dividend schedule announcements.

---

### 2.4 Convexity Violations & Butterfly Arbitrage

In any rational, arbitrage-free options surface, call prices must be a convex function of strike:
$$\frac{\partial^2 C}{\partial K^2} \ge 0$$

The price of a symmetric butterfly spread centered at strike $K_2$ with wings at $K_1$ and $K_3$ ($K_2 - K_1 = K_3 - K_2 = \Delta K$):
$$\text{Butterfly} = C(K_1) - 2C(K_2) + C(K_3)$$

Since the expiration payoff of a long butterfly is $\ge 0$ for all $S_T$, the entry price **must be strictly positive**:
$$\text{Cost} \ge 0$$
- **Negative Butterfly Arbitrage**: If market quotes yield $\text{Cost} < 0$ (a net credit to enter), the trader buys the wings, sells the body, collects upfront cash, and has a strictly non-negative payout profile at expiration.
- **Overpriced Butterfly Arbitrage**: If $\text{Cost} > \Delta K \cdot e^{-rT}$, selling the butterfly yields more than the maximum possible payout.

---

### 2.5 Negative-Risk / Super-Credit Iron Butterfly (The Screen-Shot Trade)

This is the exact strategy highlighted in the user's initial inquiry:
* **Leg 1**: Long Put at $K_1$
* **Leg 2**: Short Put at $K_2$
* **Leg 3**: Short Call at $K_2$
* **Leg 4**: Long Call at $K_3$
*(Where $K_2 - K_1 = K_3 - K_2 = W$)*

#### Payoff Profile:
$$\text{Max Liability at Expiration} = W \times 100$$
$$\text{Net Credit Received} = C_{\text{net}} = \text{Credit}_{\text{Put Spread}} + \text{Credit}_{\text{Call Spread}}$$

When market dislocations allow $C_{\text{net}} > W \times 100$:
$$\text{Minimum Expiration Profit} = C_{\text{net}} - W \times 100 > 0$$
$$\text{Maximum Expiration Profit} (S_T = K_2) = C_{\text{net}}$$

Furthermore, receiving $C_{\text{net}}$ upfront allows the trader to earn the risk-free rate ($r$) on collateral over duration $T$:
$$\text{Total Minimum Return} = (C_{\text{net}} - W \times 100) + C_{\text{net}} \left( e^{rT} - 1 \right)$$

---

## 3. High-Edge Microstructure & Small-Capacity Arbitrages

Small-capacity arbitrages persist because institutional algos ignore them, and market makers use automated quoting algorithms that fail to account for discrete, non-linear market events.

### 3.1 Dividend Play & Ex-Dividend Early Assignment Arbitrage

#### The Microstructure Mechanism
On the cum-dividend date (the trading day immediately preceding the ex-dividend date), holders of deep in-the-money (ITM) call options must make an optimal exercise decision.
An American call option on a dividend-paying stock should be exercised if:
$$P(K, T) < D - K \left( 1 - e^{-r \Delta t} \right)$$
*(Where $P$ is the corresponding put premium, $D$ is the cash dividend, and $\Delta t$ is the time remaining to expiry).*

If the put price is less than the dividend amount, holding the call is suboptimal because the stock price will drop by approximately $D$ overnight, destroying call value without yielding dividend cash.

#### The Market Inefficiency
Retail investors, unsophisticated fund managers, and overseas accounts holding ITM call options **routinely fail to exercise their calls on the cum-dividend date** due to ignorance, broker processing fees, or tax negligence.

#### The "Dividend Play" Arbitrage
Market makers and specialized prop desks exploit this via **open-interest pool dilution**:
1. On the day before ex-dividend, the trader buys and sells massive equal-quantity conversion spreads (e.g., Long Stock + Long Put + Short Call at deep ITM strikes).
2. The clearing house (OCC) assigns short call exercise notices **randomly across open interest**.
3. If total open interest is 100,000 contracts, and 80,000 contracts are exercised (because 20,000 retail contracts failed to exercise):
   - Every short contract has an assignment probability of $80\%$.
   - The trader exercises $100\%$ of their long positions.
   - For every unassigned short call ($20\%$), the trader collects the full cash dividend $D$ on the long stock, while their short call position remains alive.
   - The trader then sells the stock on the ex-date and closes the call, pocketing the dividend minus the small residual put value.

> [!NOTE]
> **Retail Application**: Retail traders can exploit smaller dividend anomalies by looking for ITM calls trading at a discount to intrinsic value minus dividend ($C < S - K - D$) on illiquid dividend payers, buying the calls, immediately exercising them before 5:30 PM ET, and shorting the underlying stock to lock in the dividend capture.

---

### 3.2 Hard-to-Borrow (HTB) & Negative Rebate Arbitrage

#### The Mechanism
When a stock is heavily shorted (e.g., meme stocks, biotechs undergoing FDA review, pre-merger targets), prime brokers charge high borrow fees (e.g., 50% to 300% annualized). This borrow fee rate $b$ is embedded into the options market through the cost of carry:
$$F = S_0 \cdot e^{(r - q - b)T}$$

If $b > r$, the implied forward $F$ is **substantially below the spot price $S_0$**. Consequently:
- In-the-money calls trade at a massive discount to parity ($C < S - K$).
- Puts trade at sky-high premiums (high implied volatility skew on the downside).

#### The Arbitrage Angles
1. **Existing Inventory Conversion**:
   If an investor already owns the stock in a cash account (or has access to cheap borrowing via retail lending programs at low rates), they can sell their stock synthetically at a massive premium by entering a **Reverse Conversion**:
   $$\text{Short Stock} + \text{Long Call} + \text{Short Put}$$
   Because the options market prices in an institutional borrow fee of 150%, the trader captures that 150% rate synthetically.
2. **Box Spread Discrepancies on HTB Equities**:
   Market participants often miscalculate the synthetic forward on non-standard expirations, allowing traders to sell boxes at yields far in excess of commercial financing rates.

---

### 3.3 Corporate Action & OCC Adjusted Contract Arbitrage

When a company executes a spin-off, reverse stock split, rights offering, or non-ordinary cash dividend, the OCC does not modify the historical strike prices. Instead, it issues an **Adjustment Memo** and updates the deliverable of existing contracts.

#### Example: Spin-Off Deliverable
Company `ABC` spins off subsidiary `XYZ` (1 share of `XYZ` for every 5 shares of `ABC`).
- Standard contract: Delivers 100 shares of `ABC`.
- Adjusted contract (`ABC1`): Delivers **100 shares of `ABC` + 20 shares of `XYZ`**.

#### The Arbitrage Opportunity
1. **Pricing Display Blindspots**: Retail broker interfaces (Webull, Robinhood, E*Trade) often display Greeks, implied volatilities, and underlying prices for adjusted options incorrectly (frequently computing Greeks using only parent stock `ABC` and ignoring `XYZ`).
2. **Panic Selling by Retail**: Retail option holders see strange ticker symbols (`ABC1`), illiquid quotes, and panic-sell at steep discounts to the combined intrinsic value:
   $$V_{\text{deliverable}} = 100 \cdot P_{\text{ABC}} + 20 \cdot P_{\text{XYZ}}$$
3. **Execution**: The trader computes the exact net asset value of the basket deliverable. When `ABC1` calls trade below $V_{\text{deliverable}} - K$:
   - Buy `ABC1` call option.
   - Immediately instruct broker to exercise the contract.
   - Sell 100 shares of `ABC` and 20 shares of `XYZ` in the open market.
   - Lock in pure cash spread minus transaction fees.

*Capacity*: \$2,000 to \$30,000 per event. Institutional funds cannot trade adjusted series due to liquidity mandates, leaving this field wide open for specialized small traders.

---

### 3.4 Cash Merger / M&A Target Option Arbitrage

#### The Mechanism
When an acquirer enters a **definitive all-cash acquisition agreement** to buy a target company at a fixed price $P_{\text{acq}}$ (e.g., \$50.00/share), the stock typically trades at a discount (e.g., \$48.50) reflecting closing risk and the time value of money.

#### Market Inefficiencies
1. **Lottery Ticket Bias**: Retail traders continue to purchase out-of-the-money calls at strikes above the acquisition price (e.g., \$55, \$60 calls) hoping for a topping bid or short squeeze, paying \$0.10 to \$0.40.
2. **Model Invalidation**: Standard Black-Scholes models assume continuous geometric Brownian motion with unbounded upside, leading automated quoting systems to overprice high-strike calls.

#### The Arbitrage / Low-Risk Structures
- **Bear Call Spreads Above Deal Price**: Sell \$52.50 Call / Buy \$55 Call. If the deal closes at \$50.00, or breaks downward, the entire credit is retained. The only risk is a rival takeover offer at a higher cash price.
- **Target Box Spread Financing**: If the acquisition is legally binding and backed by committed financing, building synthetic structures on the target locks in deal closure yields higher than short-term Treasury bills.

---

### 3.5 Odd-Lot Tender Offer Option Hedging

#### The Mechanism
Under SEC Rule 13e-4(f)(1)(ii), companies executing self-tender offers often include an **odd-lot priority provision**:
> Stockholders who own fewer than 100 shares ("odd-lot holders") and tender all of their shares are accepted in full, **exempt from proration**.

Large institutional holders who tender 500,000 shares might only get $30\%$ of their shares accepted if the offer is oversubscribed (proration risk). An odd-lot holder gets $100\%$ accepted.

#### The Arbitrage Execution
1. Buy 99 shares of the company at market price $P_{\text{market}}$ (e.g., \$38.00).
2. The tender offer price is $P_{\text{tender}}$ (e.g., \$44.00).
3. **The Option Hedge**: To eliminate market risk in the event the company cancels the tender offer or files for restructuring before closing, the trader buys a protective put at strike $K \ge \$38$ or sells an ATM synthetic forward.
4. Tender the 99 shares to the company, collect \$44.00 per share, and close the option hedge.
*Capacity*: Limited strictly to 99 shares per legal entity / brokerage account (\$1,000 – \$5,000 profit per deal).

---

### 3.6 Expiration 4:00 PM – 5:30 PM "Contra-Exercise" / Late Exercise Arbitrage

#### The Microstructure Friction
In US markets:
* **Option Trading Halts**: 4:00 PM Eastern Time.
* **Underlying Stock Trading**: Continues in the After-Hours session until 8:00 PM Eastern Time.
* **OCC Exercise Cutoff**: Option clearing brokers allow option holders until **5:30 PM Eastern Time** to submit "Contrary Exercise Advice" (CEA) notices to exercise out-of-the-money options or abandon in-the-money options.

#### The Arbitrage Mechanism
Suppose an earnings report or major FDA announcement is released at **4:05 PM ET** on option expiration Friday:
- Stock closes at 4:00 PM at **\$100.00**.
- The trader holds **\$102.50 Strike Call Options** (which expired worthless at the 4:00 PM market close and cost virtually \$0.01).
- At 4:10 PM, the stock surges in after-hours to **\$115.00**.

#### Execution:
1. **Short the stock in after-hours at \$115.00**.
2. Submit a Contrary Exercise Notice to your broker before 5:30 PM ET to exercise the \$102.50 Calls.
3. On Monday, the exercised calls deliver long shares at \$102.50, immediately covering the short stock established at \$115.00.
4. **Net Profit**: $\$115.00 - \$102.50 - \text{Option Premium} = \mathbf{+\$12.49}$ per share risk-free.

---

### 3.7 LEAPS Stale Quote & Super-Credit Limit Farming

On 2- to 3-year LEAPS (Long-Term Equity Anticipation Securities) on mid- and mega-cap equities:
* Open interest is low, and daily trading volume on non-round strikes is frequently zero.
* Market maker bots maintain quotes with massive bid-ask spreads (e.g., Bid \$1.00 / Ask \$6.00).
* Multi-leg pricing algorithms on retail brokerages (like Interactive Brokers, ThinkorSwim, Schwab) route complex orders to internalizers and Complex Order Books (COB).

#### The Edge
Traders post limit orders for multi-leg structures (Iron Condors, Butterflies, Conversions) inside the spread where the total credit mathematically exceeds the maximum payout.
* When institutional market participants roll institutional positions or rebalance delta via market sweeps, execution engines sweep the COB book and cross the trader's resting limit order.
* As demonstrated in the user's uploaded trade, patience yields fills where:
  $$\text{Credit} = \$10,052 > \text{Max Liability} = \$10,000$$

---

### 3.8 Retail Price Improvement & PFOF Midpoint Capture

Wholesalers (Citadel Securities, Virtu Financial, Two Sigma) pay retail brokerages for order flow under the legal agreement that they provide **Price Improvement** over the National Best Bid and Offer (NBBO).

```
   Public Exchange NBBO:   Bid: $4.00 (x50)  ──────────────  Ask: $5.00 (x50)
                                                    ▲
                                                    │
   Wholesaler Midpoint Fill:                  Price: $4.50
   (Retail Multi-Leg Order)
```

Because retail order flow is classified as non-toxic (uninformed), wholesalers are willing to fill combo orders at the exact mathematical midpoint.
* **The Arbitrage**: A trader identifies a wide bid-ask spread across two legs where:
  $$\text{Mid}(Leg_1) + \text{Mid}(Leg_2) > \text{Arbitrage Bound}$$
* By routing a limit order pegged to the combo midpoint via a retail broker, the retail trader achieves fills that an institutional participant on raw exchange feeds (who must pay maker/taker exchange fees of \$0.50 per contract) cannot match.

---

### 3.9 Crypto Option vs. Perpetual Swap Basis Arbitrage

On crypto exchanges (Deribit, Binance, OKX, Bybit, Paradigm):
- **Perpetual Swaps (Perps)** use an 8-hour funding rate mechanism to anchor perp prices to spot. During bull runs, annualized funding rates often exceed 50% to 100%.
- **Crypto Options** trade European-style options on BTC and ETH. The implied forward is defined by:
  $$F = S \cdot e^{(r - y)T} = K + e^{rT} (C - P)$$

#### The Arbitrage
When perpetual funding rates trade at a large premium to the option-implied forward yield:
1. **Short the Perpetual Swap** (to collect the 50%+ funding rate).
2. **Buy the Synthetic Forward** via options (Long Call + Short Put at strike $K$).
3. **Delta is perfectly neutralized**:
   $$\Delta_{\text{total}} = \Delta_{\text{perp}} (-1.0) + \Delta_{\text{options}} (+1.0) = 0$$
4. Capture the clean divergence between the floating perpetual funding yield and the fixed options implied carry rate.

> **Implemented (2026-09):** `frontend/src-tauri/src/options_parity.rs` +
> `crates/arbitrageur-exchanges/src/deribit.rs` ship this strategy on
> Deribit (BTC/ETH, testnet + live). The scanner pairs call/put quotes at
> identical strike+expiry, builds the synthetic forward `K + (C − P)·index`,
> and nets it against the perpetual's funding-adjusted price
> (`perp ± funding_8h × hours/8 × mark`). Conversion = sell call / buy put /
> buy perp when the synthetic is rich; Reversal = the mirror. Execution is
> a 3-leg single-venue cycle with the triangle path's discipline:
> persist-before-submit leg intents, bounded IOC orders (60 bps over VWAP),
> venue-truth settle polling, cancel + re-poll for still-working orders,
> and `imbalanced`/`unresolved` residual signals on any partial cycle.
> Fees use Deribit's real schedule (options: min(0.03% of underlying,
> 12.5% of premium); perp taker 0.05%). Premiums/fees are the only cash
> booked at entry — the convergence edge stays unrealized until expiry.
> Manual Execute is live-capable through `live_entry_gate`; autonomous
> (`auto`) entry is testnet-only.

---

## 4. Operational Requirements & Pitfalls

Executing low-capacity and structural options arbitrages requires strict operational controls:

### 4.1 Portfolio Margin (PM) vs. Reg-T Margin

```
                    ┌───────────────────────────────────────────────┐
                    │            Margin Account Types               │
                    └───────────────────────┬───────────────────────┘
                                            │
                     ┌──────────────────────┴──────────────────────┐
                     ▼                                             ▼
        [Reg-T Standard Margin]                        [Portfolio Margin (PM)]
     • Fixed rule-based margin                      • Risk-based TIMS / VaR model
     • Evaluates spreads independently              • Evaluates net portfolio stress (-15% to +15%)
     • Ties up massive capital on 4-leg boxes       • Margin matches true max economic loss
     • Unusable for multi-leg arbitrage             • Required for positive-carry option arb
```

Under Regulation T, a broker may calculate margin requirements for multi-leg structures using rigid rules that require 20% underlying equity value, completely destroying capital efficiency. Under **Portfolio Margin (PM)** (minimum equity requirement typically \$110,000 at US brokers), the broker stresses the portfolio across a 10-point valuation grid. A mathematically hedged box spread or iron butterfly requires margin equal only to the theoretical maximum loss.

### 4.2 Early Assignment Risk (American Style Options)

If an option is American style:
1. **Short Deep ITM Put**: If the risk-free rate is high, the put holder earns more interest by holding cash than holding the put's time value. They will exercise early.
2. **Short Deep ITM Call**: On the eve of an ex-dividend date, the call holder will exercise early to capture the dividend.
* **The Failure Mode**: The trader wakes up to find their short leg assigned, converting the spread into a leveraged long or short stock position, generating overnight margin borrowing fees of 8%–12% that destroy the arbitrage profit.

---

## 5. Screening & Detection Playbook for Small Traders

To systematically find these opportunities, a prop/retail trader should run the following automated scanners:

1. **LEAPS Combinatorial Credit Scanner**:
   Scan all option chains expiring in $> 1.5$ years for stocks with high implied volatility. Calculate:
   $$\text{Spread} = \text{Credit}_{\text{Bull Put}}(K_1, K_2) + \text{Credit}_{\text{Bear Call}}(K_2, K_3)$$
   Flag instances where $\text{Spread} \ge K_2 - K_1$.
2. **OCC Adjustment Memo Monitor**:
   Poll the OCC website (`theocc.com`) for new "Information Memos" regarding corporate actions, stock splits, and spin-offs. Calculate the deliverable basket fair value and set alerts for underpriced adjusted options.
3. **Ex-Dividend ITM Call Discount Scanner**:
   Identify stocks with ex-dividend dates in $T-1$ days. Filter for call options where:
   $$\text{Bid}(C) < (S_0 - K) - D$$
   Buy the discounted call and immediately submit an exercise notice while simultaneously shorting the stock.
4. **SEC 13e-4 Odd-Lot Tender Offer Tracker**:
   Parse SEC Edgar filings for SC 13E4/A submissions containing "odd lot priority" provisions. Compute the spread between current stock price and tender offer price, hedging via synthetic option forwards.
