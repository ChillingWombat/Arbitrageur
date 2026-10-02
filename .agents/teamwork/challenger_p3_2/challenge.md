# Adversarial Challenge Report: R3, R4, R7

## Challenge Summary

**Overall risk assessment**: LOW

All implementations for requirements R3, R4, and R7 were empirically stress-tested through static source analysis, combinatorial state-space testing, and interactive DOM rendering harnesses in `frontend/src/components/ChallengerR3R4R7.test.ts`. Zero regressions, zero unhandled boundary conditions, and zero spec violations were found.

Verdict: **APPROVE**

---

## Challenges & Stress Tests

### Challenge 1: R3 — Outer Wrapper Symmetric Padding & Geometry
- **Assumption challenged**: The outer wrapper padding in `BotLifecycleControls.tsx` must be uniformly 4px on all 4 sides without residual asymmetric horizontal/vertical padding (`px-2 py-1`), and inner button spacing must remain uniform across all lifecycle states.
- **Attack scenario**:
  - Test if any residual `px-` or `py-` classes linger on the outer wrapper.
  - Test if button sizes or margins distort when state transitions occur (`idle` → `running` → `paused` → `reduce_only` → `stopping`).
  - Audit all 7 strategy surfaces (`PredictionHub`, `FundingRadar`, `OpportunityRadar`, `OptionsArbitrage`, `TriangularArbitrage`, `LiveMatrix`, `SpotArbitrageBot`) to ensure no outer wrapping container adds asymmetric padding overrides.
- **Empirical observation**:
  - Outer wrapper rendered class attribute is verbatim: `"flex items-center space-x-1.5 p-1 bg-surface border border-surface-border rounded-lg shadow-xs"`.
  - Padding class `p-1` applies 4px on top, bottom, left, and right symmetrically.
  - Inner button container is `<div className="flex items-center gap-1">`, maintaining 4px separation between adjacent buttons.
  - Inner buttons maintain uniform `px-2 py-2 rounded-lg border text-xs font-mono font-bold` across all phases.
  - All 7 strategy surfaces use standard `<BotLifecycleControls>` with zero custom padding overrides.
- **Result**: PASS

### Challenge 2: R3 — Bot Lifecycle State Availability & Fail-Closed Invariants
- **Assumption challenged**: Bot lifecycle state transitions and button availability must adhere to fail-closed safety constraints across all combinations of `phase`, `hasOpenPositions`, `disabled`, and `allowStartWithOpenPositions`.
- **Attack scenario**:
  - Test whether a disabled engine or missing configuration locks the operator out of emergency flattening or unwinding.
  - Test whether convergence bots can accidentally start new positions while unhedged open positions exist.
  - Test whether reduce-only mode properly suppresses new entry start buttons while preserving exit pathways.
- **Empirical observation**:
  - When `disabled=true` (entries disabled), `start` is blocked (`false`), but `reduceOnly` (when positions open) and `stop` (emergency exit) remain active (`true`). The operator cannot be trapped with open risk.
  - When `hasOpenPositions=true` and `phase="idle"`, convergence bots default to `start: false`, requiring explicit exit/flattening before re-arming unless `allowStartWithOpenPositions=true` (options bots).
  - When `phase="stopping"`, all action buttons lock to prevent concurrent command races during emergency flattening.
- **Result**: PASS

### Challenge 3: R4 — Dislocation Matrix Delta Badges (LiveMatrix)
- **Assumption challenged**: Matrix cell delta badges must render clean numerical delta strings (`Δ +` or `Δ -`) without any `TrendingUp` or `TrendingDown` icons or unicode arrow symbols (`↗`, `↘`, `↑`, `↓`), while correctly handling zero, negative zero, sub-decimal, and missing baseline values.
- **Attack scenario**:
  - Test whether `TrendingUp` or `TrendingDown` can still be imported or instantiated.
  - Test positive (`+5.23` → `Δ +5.2`), negative (`-4.89` → `Δ -4.9`), exact zero (`0` → `Δ 0.0`), and negative zero (`-0.0` → `Δ 0.0`).
  - Test missing economics (`costReady: false`), missing baseline (`baselineReady: false`), and empty sample count (`sampleCount: 0`) to confirm the badge is cleanly omitted rather than rendering `Δ NaN` or empty badges.
- **Empirical observation**:
  - `LiveMatrix.tsx` has zero imports and zero references to `TrendingUp` or `TrendingDown`.
  - Regex inspection confirms zero unicode arrow symbols in delta badge text.
  - Delta badge template `Δ {cell.deltaBps > 0 ? "+" : ""}{cell.deltaBps.toFixed(1)}` formats numbers deterministically with proper algebraic sign.
  - Missing baseline or cost conditions cleanly omit the delta badge container entirely.
- **Result**: PASS

### Challenge 4: R4 — SignalTrendGadget Footer Removal & Analytics Preservation
- **Assumption challenged**: The venue samples and baseline description footer line in `SignalTrendGadget.tsx` must be completely omitted, while the analytics row (`band μ`, `p90`, `max`, and hurdle %) must be preserved without disruption.
- **Attack scenario**:
  - Render `SignalTrendGadget` with active multi-venue series data. Verify no element contains `venue samples in window` or `baseline = `.
  - Render with single-venue series data. Verify `1 venue — pairwise spread needs ≥2 venues` is also removed.
  - Verify that analytics data (`band μ 3.1`, `88% of polls`) is present in DOM.
- **Empirical observation**:
  - Interactive DOM child-process test confirmed that neither `venue samples in window` nor `baseline = most-liquid venue` nor `1 venue — pairwise spread needs ≥2 venues` exists in DOM.
  - Analytics row renders cleanly: `band μ 3.1 · p90 4.5 · max 6.2 bps` and `≥2.5 bps hurdle 88% of polls`.
- **Result**: PASS

### Challenge 5: R7 — EngineSettings Security Model Box Styling & MFA Copy Cleanups
- **Assumption challenged**: Security Model box must adopt neutral slate styling (`bg-slate-50` with `text-slate-500` icon) matching the About page Legal Disclaimer box; MFA unlock explanation paragraph and authenticator app examples must be completely absent.
- **Attack scenario**:
  - Inspect Security tab DOM for old green styling (`text-emerald-600` or `bg-surface`).
  - Search DOM for old copy snippets (`MFA gates the unlock path`, `The TOTP secret is stored inside the encrypted vault`, `Google Authenticator, 1Password, Authy`).
  - Compare rendered classes between Security Model box and About page Legal Disclaimer box.
- **Empirical observation**:
  - Security Model box container has classes: `bg-slate-50 border border-surface-border rounded-lg p-3.5 shadow-2xs space-y-2`.
  - Icon is `<ShieldCheck className="w-3.5 h-3.5 text-slate-500" />` (zero emerald/green classes).
  - Matches About page Legal Disclaimer box styling (`p-4 bg-slate-50 border border-surface-border rounded-lg space-y-2 text-xs font-mono text-slate-600 leading-relaxed` with `<Scale className="w-3.5 h-3.5 text-slate-500" />`).
  - Verbatim check confirms `MFA gates the unlock path in this app...` is completely absent.
  - Verbatim check confirms `(Google Authenticator, 1Password, Authy)` is completely absent; clean label is `"Optionally require a 6-digit TOTP code at vault unlock"`.
- **Result**: PASS

---

## Stress Test Results Summary

| Scenario | Expected Behavior | Actual Behavior | Result |
|---|---|---|---|
| R3: Outer wrapper padding | `p-1` symmetric (4px) | `p-1` verified on wrapper div | PASS |
| R3: Inner button dimensions | `px-2 py-2 rounded-lg` across all phases | Verified across all 5 phases | PASS |
| R3: Bot state availability | Fail-closed invariants respected | Verified across 14 state permutations | PASS |
| R3: All 7 strategy surfaces | Consistent `BotLifecycleControls` usage | All 7 strategies verified without wrapper overrides | PASS |
| R4: LiveMatrix delta badge | Numerical `Δ +/-` without Trend arrow icons | Zero arrow icons/SVGs; clean numerical delta | PASS |
| R4: Missing delta / baseline | Delta badge suppressed | Badge cleanly omitted | PASS |
| R4: SignalTrendGadget footer | Venue samples/baseline text omitted | Omitted from static code and interactive DOM | PASS |
| R4: SignalTrendGadget analytics | Distribution stats row preserved | `band μ`, `p90`, `max`, hurdle % verified in DOM | PASS |
| R7: Security Model styling | `bg-slate-50` and `text-slate-500` | Matched About legal disclaimer exactly | PASS |
| R7: MFA unlock explanation | Paragraph completely absent | Zero matching text in source or DOM | PASS |
| R7: Authenticator app examples | Parenthesized app list removed | Clean copy verified in source and DOM | PASS |
| Verification: TypeScript check | Clean compilation | `npx tsc --noEmit` exited 0 with 0 errors | PASS |
| Verification: Frontend tests | All component tests pass | `npm test` exited 0 with all suites passing | PASS |

---

## Unchallenged Areas

- Backend dev RPC endpoints and rust crates: Out of scope for frontend UI/UX requirements R3, R4, R7.
- PredictionHub, OpportunityRadar, FundingRadar scanner layouts (R1, R2, R5, R6, R8): Assigned to parallel challenger `challenger_p3_1`.
