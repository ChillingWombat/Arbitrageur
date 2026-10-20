# Consolidate conformal risk controls behind one execution seam

Status: accepted for the #146 quant-system integration.

## Context

`arbitrageur-execution` contains 30 separate `conformal_*` modules. Most are optimizer-themed clones that expose independent APIs but have no execution consumer. The real controls cover three distinct hazards:

- `ConformalRiskGuard` calibrates margin shortfall against observed shocks.
- `ConformalCvarShieldEngine` bounds recent execution-loss tails and produces an allow, throttle, or halt verdict.
- `L2SequencerLatencyGuard` and `QueuePriorityTracker` reject or withdraw orders when venue timing and queue conditions make the execution unsafe.

Leaving all 30 APIs exposed invites accidental wiring of an uncalibrated model and makes the execution gate impossible to audit.

## Decision

Add one `ExecutionRiskShield` seam in `arbitrageur-execution`. It composes the conformal margin guard, conformal CVaR shield, L2 sequencer guard, and queue-priority tracker, and returns one preflight verdict to desktop entry paths. The verdict carries whether dispatch is allowed, the size scale, and a concrete rejection reason.

Keep `conformal_risk_guard` and `conformal_cvar_shield` as separate model components behind the seam. Keep `l2_sequencer_guard` and `queue_priority` as their domain-specific inputs. Remove the 28 optimizer-clone conformal modules that do not correspond to an independent risk mechanism.

Calibration must use observed margin, slippage, or fill samples. Insufficient calibration data fails closed by holding entries or applying the shield's warm-up throttle. A missing route hazard input does not become an implicit pass.

Desktop entry paths call the seam before setting `SignalExecution` to dispatchable. Passive unwinds retain the queue tracker because the hazard applies while an order is resting. Diagnostics expose each component's bound and reason, not just a combined boolean.

## Alternatives considered

- Keep every conformal module selectable per strategy. Rejected: there is no evidence that the optimizer variants represent distinct execution risks, and a 30-way policy surface cannot be validated.
- Collapse everything into `ConformalRiskGuard`. Rejected: margin shortfall and realized execution-loss tails answer different questions and need different calibration samples.
- Apply a single static margin multiplier. Rejected: it ignores observed tail loss and queue or sequencer hazards.

## Consequences

The product gains one auditable preflight interface and one place to test fail-closed behavior. The cost is coupling four risk components into one seam and carrying their combined diagnostics through desktop DTOs. Their internal models stay separate, so a change to CVaR calibration does not alter margin-shortfall semantics.
