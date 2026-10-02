## Problem Statement

In statistical arbitrage and spread convergence strategies, using static wall-clock timers (such as a blind 24-hour limit) to force position auto-exits creates severe structural distortions:

1. **Cutting Winning Trades (False Positive Exits):** If after several hours a spread has reverted 90% toward its baseline and the live model estimates a 96% probability of completing convergence, a rigid 24-hour clock timer forcibly unwinds the trade at market. This crosses the bid-ask spread and pays unnecessary taker fees to cut a position that is moments away from taking full profit.
2. **Holding Dead Risk in Structural Shifts (False Negative Holds):** If market conditions change rapidly after entry (e.g. protocol de-pegging, structural liquidity collapse, or persistent divergence) causing the estimated probability of reversion ($P_{win}$) to plunge from 95% down to 20%, an arbitrary 24-hour timer forces the system to hold the position for another 20+ hours. This ties up scarce margin collateral and exposes the account to adverse tail risk long after the quantitative thesis has broken.
3. **Misaligned Strategy Configurator Ergonomics:** Strategy configurators previously presented arbitrary duration fields ("Max hold before auto-exit") that misled operators into treating time as an independent alpha driver rather than conditioning exits on continuous statistical edge.

## Solution

Decouple auto-exits from arbitrary static time horizons and replace them with a dynamic **Probability-Driven Statistical Invalidation Exit Architecture**:

1. **State-Driven Multi-Gate Evaluation:** Position supervision is structured as an ordered state-machine evaluation:
   - **Target Convergence (Take Profit):** Spread reverts into the friction band around baseline ($|dev| \le \text{friction band}$). Target profit is captured; takes absolute precedence over all other exit gates.
   - **Stop Loss (Adverse Excursion):** Spread widens past the configured stop multiple on the adverse side ($|dev| \ge \text{stop\_mult} \times |entry\_dev|$). Loss is strictly bounded.
   - **Statistical Invalidation Decay (Model Breakdown):** Live estimated win/reversion probability $P_{win}(t)$ collapses below the invalidation threshold (default 50%). The position is unwound immediately as `ProbabilityDecayed` because the hypothesis no longer has positive statistical edge.
   - **Dead-Man Safety Ceiling (Fail-Safe Only):** An internal upper time ceiling (e.g., 7 days) serves purely as a dead-man switch against frozen feeds or zero-volatility book lockups, never as an active trade optimization knob.
2. **Ergonomic Strategy Configuration:** Static time limits are removed from strategy configurators. Instead, operators configure the quantitative invalidation threshold (`Exit invalidation P(win)`), defaulting to 50% ($0.50$).
3. **Honest Ledger Attribution:** Exits triggered by statistical invalidation are distinctly recorded on the audit log and mapped to `StoppedOut` in cycle performance history, ensuring telemetry truthfulness.

## User Stories

1. As a quantitative trader, I want my open spread convergence positions to stay open as long as reversion probability remains high, so that trades nearing full profit are not terminated by an arbitrary clock.
2. As a risk manager, I want positions to automatically exit when live estimated win probability falls below 50%, so that capital is not trapped in trades whose statistical edge has deteriorated.
3. As an operator configuring the Radar Bot, I want an "Exit invalidation P(win)" input in the Safety configuration section, so that I can set the exact probability threshold that triggers an invalidation exit.
4. As an operator configuring the Radar Bot, I want invalidation probability to default to 0.50 (50%), so that the system ships with a sensible, conservative risk boundary out of the box.
5. As a risk manager, I want the invalidation probability knob to be validated between 0.05 and 0.90, so that corrupt or unhinged configuration values cannot be saved.
6. As a quantitative developer, I want the convergence evaluation function to prioritize target profit over probability decay, so that positions already inside the friction band take profit even if wide-range volatility models report low confidence.
7. As an operator monitoring bot status, I want the bot's status JSON to expose the configured `invalidationPWin` value, so that external monitoring tools and dev RPC clients can inspect the active rule.
8. As a compliance auditor, I want invalidation exits to record the exact live probability and threshold in the audit reason, so that historical exits can be post-mortemed with complete mathematical transparency.
9. As a trader reviewing performance analytics, I want invalidation exits to be recorded with an honest status (`StoppedOut`) rather than masquerading as normal target completions (`Closed`), so that win-rate telemetry is never inflated.
10. As an automated executor, I want the reconciler to use durable wire client order IDs when unwinding invalidation exits, so that dropped gateway acknowledgements do not create stranded or duplicate legs.
11. As a futures spread trader, I want the background convergence supervisor to query the live opportunity monitor's calibrated $P_{win}$ and trigger invalidation exits when the model breaks, so that manual futures positions enjoy the same probability protection as automated bot positions.
12. As a trader using strategy configurators, I want static "Max hold before auto-exit" inputs removed from strategy forms, so that I am not misled into setting arbitrary holding durations.
13. As an operator running testnet validation, I want the release binary to run with custom protocol support and transient systemd user service management, so that live execution testing matches production packaging.
14. As an operator managing system credentials, I want the secure vault to maintain authenticated session state across bot cycles without stalling scheduled exit checks.
15. As a developer running regression tests, I want all unit tests in the strategy crate, desktop backend, and frontend component suites to pass with zero failures and zero linter warnings.

## Implementation Decisions

1. **AutoExecConfig & SpatialExitConfig Enhancements:**
   - Added `invalidation_p_win: f64` with default `0.50` to both `AutoExecConfig` and `SpatialExitConfig`.
   - Validated against range `0.05..=0.90` (rejecting NaN, infinite, and out-of-bound values fail-closed).
2. **Live Spread View & Exit Triggers:**
   - Extended `LiveSpreadView` to carry `p_win: Option<f64>`.
   - Added `ExitTrigger::ProbabilityDecayed` and `SpatialExitTrigger::ProbabilityDecayed` with `as_str()` serialization as `"probability_decayed"`.
3. **Evaluation Hierarchy:**
   - `evaluate_exit` and `evaluate_spatial_exit_with_prob` enforce strict precedence:
     1. Stale / unwatched grace check (`StaleUnwind`).
     2. Convergence into friction band (`Converged`).
     3. Adverse excursion stop band (`Stopped`).
     4. Statistical invalidation decay (`ProbabilityDecayed`).
     5. Dead-man hold ceiling (`TimedOut`).
4. **Execution Coordinator & Supervisor Plumb:**
   - In the Opportunity Executor tick loop, populated `p_win` from `MonitorRowDto` (`r.win_probability.or(r.reversion_probability).or(r.p_win)`).
   - In the futures convergence supervisor, resolved live $P_{win}$ from the active monitor snapshot by matching symbol and venue leg pairs.
   - Mapped `ProbabilityDecayed` exits to virtual ledger `CycleStatus::StoppedOut`.
5. **Frontend UI Plumb:**
   - Added `invalidationPWin?: number` to `RadarBotConfigDto` TypeScript interface.
   - Added `Exit invalidation P(win)` numeric input to `OpportunityRadar.tsx` in Section 4 (Safety), equipped with informative tooltip.
   - Retained safe default `invalidationPWin: 0.50` in `defaultRadarBotConfig`.

## Testing Decisions

- **Testing Principles:** Tests strictly evaluate external observable behavior (verdict enum variants, trigger strings, and rendered DOM attributes) rather than internal private variables.
- **Strategy Unit Tests (`arbitrageur-strategy`):**
   - `exit_on_probability_decay_below_floor`: Confirms that a spread outside the friction band with $P_{win} = 0.30$ triggers `ProbabilityDecayed`, while $P_{win} = 0.65$ holds, and $|dev| \le 4.0$ captures profit even if $P_{win}$ is degraded.
   - `spatial_exit::tests::exit_on_probability_decay_below_floor`: Confirms that `evaluate_spatial_exit_with_prob` yields `SpatialExitVerdict::Exit` with `SpatialExitTrigger::ProbabilityDecayed` when probability is 0.35, but holds when 0.75.
- **Desktop Integration Tests (`arbitrageur-desktop`):**
   - All 610 desktop unit and integration tests verified passing cleanly.
- **Frontend Verification (`npm test` & `npx tsc`):**
   - TypeScript compiles with 0 type errors.
   - All 52 frontend test suites (400+ test checks) pass with 0 failures.

## Out of Scope

- Rewriting the continuous Ornstein-Uhlenbeck stochastic calibration formulas.
- Changing exchange-level funding settlement windows (e.g. 8-hour funding intervals on perps).
- Introducing automated delta-hedging algorithms for options structures.

## Further Notes

- Verified on testnet: Active positions on Lighter and Hyperliquid survived dropped gateway acknowledgements and were seamlessly reconciled and settled via durable client order IDs.
- Deployed to the active systemd user service `arbitrageur-testnet.service` with release binary built under `--features custom-protocol`.
