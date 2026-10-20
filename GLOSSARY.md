# Arbitrageur

Language used by the Arbitrageur Rust workspace and its desktop backend.

## Environments and identity

**TradingEnvironment**:
The venue endpoint, credential, and durable-state namespace in which orders run. Live uses production accounts; Testnet uses each venue's public demo account.
_Avoid_: paper mode, real mode, simulation mode

**CanonicalBase**:
The normalized underlying asset used to pair markets across venues, with quote suffixes and non-distinguishing wrappers removed. It preserves meaningful class markers such as spot versus perpetual.
_Avoid_: raw symbol, ticker, venue symbol

## Signals and positions

**SignalExecution**:
A durable record of one strategy-route execution, including its environment, status, expected edge, fees, and child legs.
_Avoid_: trade, order, signal log

**SignalLeg**:
One venue-account position or order inside a SignalExecution, with its side, symbol, intended quantity, fill truth, fees, and venue order identity.
_Avoid_: fill, order (when the durable leg is meant)

**TrackedPosition**:
The Spread Radar executor's record for an open convergence pair. It carries the entry frame, baseline, friction band, size, deadline, and watch state used to decide when to exit.
_Avoid_: position (unqualified), bot trade

**PassiveUnwind**:
A bounded reduce-only exit that first rests a limit order inside the spread, then falls back to a market or IOC exit after timeout or adverse conditions.
_Avoid_: limit exit, maker close

**EmergencyUnwind**:
An immediate market, reduce-only close of unmatched exposure after a multi-leg route cannot retain a matched position.
_Avoid_: passive exit, retry

## Quantitative controls

**LiveVolSurfaceCache**:
The process-wide cache of current forward prices and implied-volatility slices used by the desktop options paths.
_Avoid_: surface map, vol table

**SviSlice**:
An arbitrage-free stochastic-volatility-inspired parameterization of one option expiry's implied-variance curve.
_Avoid_: smile (unqualified), SVI curve

**ConformalRiskShield**:
The execution preflight that turns observed margin shortfall and slippage samples into a finite-sample risk bound used to allow, resize, or halt a route.
_Avoid_: risk guard (unqualified), generic CVaR

**HawkesFlowGate**:
An entry gate that uses cross-asset excitation, toxic burst intensity, and directional flow drift to penalize or reject a candidate route.
_Avoid_: flow check, toxicity score

**FastBasisHarvest**:
Early closure of a funding or basis carry when basis compression captures enough net edge before the settlement epoch.
_Avoid_: funding exit, early close
