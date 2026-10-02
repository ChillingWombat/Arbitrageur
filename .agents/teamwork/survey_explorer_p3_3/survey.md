# Technical Survey: R3, R4, R7 Exploration Report

**Surveyor**: `survey_explorer_p3_3` (Read-Only Exploration Agent)  
**Date**: 2026-10-01T16:18:00Z  
**Target Milestone**: Survey Phase 3 (R3, R4, R7)  
**Authoritative Request**: `.agents/teamwork/ORIGINAL_REQUEST.md` (Follow-up — 2026-10-01T16:09:57Z)

---

## 1. Executive Summary

This survey maps the complete technical scope, exact code locations, proposed edits, and verification paths for three requirements under Phase 3:
1. **R3. Button Set Outer Border Symmetric Padding**: Standardize the outer border wrapper in `BotLifecycleControls.tsx` to use symmetric padding (`p-1` replacing `px-2 py-1`), ensuring consistent 4px padding around inner buttons across all 7 strategy lifecycle control groups in the system.
2. **R4. Dislocation Matrix & Signal Trend Gadget Cleanups**:
   - In `LiveMatrix.tsx`: Remove `TrendingUp` and `TrendingDown` trend arrow icons (`↗` / `↘`) from matrix cell delta badges, and prune their unused imports from `lucide-react`.
   - In `SignalTrendGadget.tsx`: Remove the footer text line rendering venue sample count and baseline description (`... venue samples in window · ... recorded ... baseline = ...`), and prune the now-redundant `historyLabel` calculation.
3. **R7. Security Page Explanations & MFA Wording**:
   - In `EngineSettings.tsx`: Refine the Security Model explanatory card to use neutral grey styling (`bg-slate-50` matching the About page Legal Disclaimer) and a neutral slate icon (`text-slate-500` replacing `text-emerald-600`).
   - Remove the explanatory paragraph `MFA gates the unlock path in this app...`.
   - Remove the authenticator app examples `(Google Authenticator, 1Password, Authy)` from the TOTP description label so it reads: `"Optionally require a 6-digit TOTP code at vault unlock"`.

All existing frontend tests (`cd frontend && npm test`) pass cleanly (47 test files, 100% pass rate). TypeScript type checks (`npx tsc --noEmit`) pass with zero errors.

---

## 2. Requirement Details & Code Analysis

### R3. Button Set Outer Border Symmetric Padding

#### 2.1 File & Location
- **Primary File**: `frontend/src/components/BotLifecycleControls.tsx`
- **Lines**: 187–205

#### 2.2 Current Code (Asymmetric Padding)
```tsx
  return (
    <div className="flex items-center space-x-1.5 px-2 py-1 bg-surface border border-surface-border rounded-lg shadow-xs">
      <div className="flex items-center gap-1">
        {buttons.map((b) => (
          <button
            key={b.key}
            type="button"
            title={b.title}
            disabled={!b.enabled}
            onClick={b.onClick}
            className={`flex items-center space-x-1 px-2 py-2 rounded-lg border text-xs font-mono font-bold transition cursor-pointer disabled:opacity-35 disabled:cursor-not-allowed ${
              b.enabled ? b.activeClass : "text-muted border-surface-border bg-surface-raised/40"
            }`}
          >
            {b.icon}
            <span>{b.label}</span>
          </button>
        ))}
      </div>
```

#### 2.3 Observation & Rationale
- The outer wrapper `div` has `px-2 py-1` (8px horizontal padding, 4px vertical padding).
- The inner buttons each have symmetric `px-2 py-2` padding (8px all sides).
- Because the wrapper uses `px-2 py-1`, the gap between the outer container border and the outer edge of the button cluster is 8px horizontally and 4px vertically. This creates visual asymmetry.
- Updating `px-2 py-1` to `p-1` (4px all sides) results in equal 4px spacing between the container's outer border and the inner button group on top, bottom, left, and right.

#### 2.4 System-Wide Strategy Lifecycle Button Groups Audit
We audited all occurrences and consumers of `BotLifecycleControls` across the repository to determine whether any parent components wrap lifecycle buttons in custom outer borders:
1. `frontend/src/components/LiveMatrix.tsx` (line 1008): `<BotLifecycleControls ... />` rendered directly inside header flex ribbon (`space-x-2`). No surrounding border.
2. `frontend/src/components/OpportunityRadar.tsx` (line 2207): Rendered directly inside header flex ribbon (`space-x-2`). No surrounding border.
3. `frontend/src/components/FundingRadar.tsx` (line 1509): Rendered directly inside header flex ribbon (`space-x-2`). No surrounding border.
4. `frontend/src/components/SpotArbitrageBot.tsx` (line 312): Rendered directly inside header flex ribbon (`space-x-2`). No surrounding border.
5. `frontend/src/components/TriangularArbitrage.tsx` (line 400): Rendered directly inside header flex ribbon (`space-x-2`). No surrounding border.
6. `frontend/src/components/OptionsArbitrage.tsx` (line 977): Rendered directly inside header flex ribbon (`space-x-2`). No surrounding border.
7. `frontend/src/components/PredictionHub.tsx` (line 266): Rendered directly inside header flex ribbon (`gap-2`). No surrounding border.
8. `frontend/src/components/Header.tsx` (lines 90–146): Top bar controls are standalone action buttons (`Pause/Resume Engine` button and `Stop All` button), not a lifecycle button group.

**Conclusion for R3**: `BotLifecycleControls.tsx` is the sole source of truth for the lifecycle button group wrapper styling. Modifying line 188 of `BotLifecycleControls.tsx` uniformly normalizes symmetric padding across all strategy lifecycle button groups in the entire application.

#### 2.5 Proposed Diff
```diff
--- a/frontend/src/components/BotLifecycleControls.tsx
+++ b/frontend/src/components/BotLifecycleControls.tsx
@@ -185,7 +185,7 @@ export const BotLifecycleControls: React.FC<BotLifecycleControlsProps> = ({
   ];
 
   return (
-    <div className="flex items-center space-x-1.5 px-2 py-1 bg-surface border border-surface-border rounded-lg shadow-xs">
+    <div className="flex items-center space-x-1.5 p-1 bg-surface border border-surface-border rounded-lg shadow-xs">
       <div className="flex items-center gap-1">
         {buttons.map((b) => (
           <button
```

---

### R4. Dislocation Matrix & Signal Trend Gadget Cleanups

#### 4.1 Sub-item A: `LiveMatrix.tsx` Matrix Cell Delta Badge Trend Arrows

##### 4.1.1 File & Location
- **File**: `frontend/src/components/LiveMatrix.tsx`
- **Imports**: Lines 2–4
- **Delta Badge Render**: Lines 133–147

##### 4.1.2 Current Code
```tsx
// Lines 2-4:
import {
  TrendingUp,
  TrendingDown,
  Zap,
...
```
```tsx
// Lines 133-147:
        {/* Delta-vs-baseline badge only renders when both economics and a
            baseline exist — a bare "Δ —" placeholder is noise, not honesty. */}
        {cell.costReady !== false && cell.baselineReady !== false && cell.sampleCount !== 0 && (
          <div
            className={`flex items-center space-x-0.5 px-1 py-px rounded-lg text-[10px] font-mono whitespace-nowrap truncate max-w-full ${visuals.deltaBadgeBg}`}
          >
            {cell.deltaBps > 0 ? (
              <TrendingUp className="w-2 h-2 inline" />
            ) : cell.deltaBps < 0 ? (
              <TrendingDown className="w-2 h-2 inline" />
            ) : null}
            <span>
              Δ {cell.deltaBps > 0 ? "+" : ""}
              {cell.deltaBps.toFixed(1)}
            </span>
          </div>
        )}
```

##### 4.1.3 Observation & Rationale
- The cell renders `TrendingUp` (`↗`) when `cell.deltaBps > 0` and `TrendingDown` (`↘`) when `cell.deltaBps < 0`.
- At `text-[10px]` cell size, the trend icon clutters the compact cell badges.
- Removing `{cell.deltaBps > 0 ? (...) : cell.deltaBps < 0 ? (...) : null}` leaves clean numerical indicators: `<span>Δ {cell.deltaBps > 0 ? "+" : ""}{cell.deltaBps.toFixed(1)}</span>`.
- `visuals.deltaBadgeBg` retains the semantic background color (green for positive dislocation, red for negative).
- Checking all occurrences of `TrendingUp` and `TrendingDown` in `LiveMatrix.tsx` confirms they are only used in this cell component; pruning them from `lucide-react` imports prevents unused import warnings.

##### 4.1.4 Proposed Diff
```diff
--- a/frontend/src/components/LiveMatrix.tsx
+++ b/frontend/src/components/LiveMatrix.tsx
@@ -1,7 +1,5 @@
 import React, { useState, useEffect, useMemo, useCallback } from "react";
 import {
-  TrendingUp,
-  TrendingDown,
   Zap,
   History,
   Terminal,
@@ -134,11 +132,6 @@ export const MatrixCellView = React.memo(function MatrixCellView({
           <div
             className={`flex items-center space-x-0.5 px-1 py-px rounded-lg text-[10px] font-mono whitespace-nowrap truncate max-w-full ${visuals.deltaBadgeBg}`}
           >
-            {cell.deltaBps > 0 ? (
-              <TrendingUp className="w-2 h-2 inline" />
-            ) : cell.deltaBps < 0 ? (
-              <TrendingDown className="w-2 h-2 inline" />
-            ) : null}
             <span>
               Δ {cell.deltaBps > 0 ? "+" : ""}
               {cell.deltaBps.toFixed(1)}
```

---

#### 4.2 Sub-item B: `SignalTrendGadget.tsx` Footer Text Line

##### 4.2.1 File & Location
- **File**: `frontend/src/components/SignalTrendGadget.tsx`
- **Footer Text Render**: Lines 628–643
- **Unused Memo Dependency**: Lines 315–323 (`historyLabel`)

##### 4.2.2 Current Code
```tsx
// Lines 315-323:
  const historyLabel = useMemo(() => {
    if (!series?.earliest_ts_ms) return null;
    const hrs = (Date.now() - series.earliest_ts_ms) / 3_600_000;
    return hrs >= 48
      ? `${(hrs / 24).toFixed(1)}d history`
      : hrs >= 1
        ? `${hrs.toFixed(1)}h history`
        : `${Math.max(1, Math.round(hrs * 60))}m history`;
  }, [series?.earliest_ts_ms]);
```
```tsx
// Lines 628-643:
      {/* Footer: honesty indicators — how much real history backs this view */}
      {series && (
        <div className="flex items-center justify-between text-xs text-muted px-0.5">
          <span>
            {series.sample_count.toLocaleString()} venue samples in window
            {historyLabel ? ` · ${historyLabel} recorded` : ""}
          </span>
          <span>
            {series.venues.length === 1
              ? "1 venue — pairwise spread needs ≥2 venues"
              : series.mode === "leader"
                ? "baseline = most-liquid venue"
                : "baseline = volume-weighted consensus"}
          </span>
        </div>
      )}
      {analytics && analytics.band.sample_count > 0 && (
        <div className="flex items-center justify-between text-xs text-muted px-0.5 mt-0.5">
          <span title="Distribution of the per-poll max–min venue spread over this window">
            band μ {fmtBps(analytics.band.mean_bps ?? 0, false)} · p90{" "}
            {fmtBps(analytics.band.p90_bps ?? 0, false)} · max{" "}
            {fmtBps(analytics.band.max_bps ?? 0, false)} bps
          </span>
          <span title={`Share of polls where the band met the ${hurdleBps} bps hurdle`}>
            ≥{hurdleBps} bps hurdle{" "}
            {analytics.band.frac_at_or_above_hurdle != null
              ? `${(analytics.band.frac_at_or_above_hurdle * 100).toFixed(0)}% of polls`
              : "—"}
          </span>
        </div>
      )}
```

##### 4.2.3 Observation & Rationale
- The footer row `{series && (<div className="flex items-center justify-between text-xs text-muted px-0.5">...</div>)}` displays:
  - Left: `{series.sample_count.toLocaleString()} venue samples in window · {historyLabel} recorded`
  - Right: `1 venue — pairwise spread needs ≥2 venues` or `baseline = most-liquid venue` / `baseline = volume-weighted consensus`
- Requirement: "Remove the footer text line displaying venue samples and baseline description (`... venue samples in window · ... recorded ... baseline = ...`)."
- The second statistics line (`{analytics && analytics.band.sample_count > 0 && ...}`) below it displays spread analytics (`band μ ... p90 ... max ... bps` and `≥X bps hurdle ...`). This line does not contain venue samples or baseline descriptions and is preserved.
- Removing lines 628–643 leaves `historyLabel` unused (it was only referenced on line 633). To ensure clean code without compiler or linter warnings, `historyLabel` (lines 315–323) should also be pruned.

##### 4.2.4 Proposed Diff
```diff
--- a/frontend/src/components/SignalTrendGadget.tsx
+++ b/frontend/src/components/SignalTrendGadget.tsx
@@ -312,16 +312,6 @@ function SignalTrendGadgetInner({
     return map;
   }, [hoverIdx, series]);
 
-  const historyLabel = useMemo(() => {
-    if (!series?.earliest_ts_ms) return null;
-    const hrs = (Date.now() - series.earliest_ts_ms) / 3_600_000;
-    return hrs >= 48
-      ? `${(hrs / 24).toFixed(1)}d history`
-      : hrs >= 1
-        ? `${hrs.toFixed(1)}h history`
-        : `${Math.max(1, Math.round(hrs * 60))}m history`;
-  }, [series?.earliest_ts_ms]);
-
   const hasSeries = pointCount >= 2;
 
   // Session overlay: UTC regime bands behind the series, derived from wall
@@ -625,23 +615,7 @@ function SignalTrendGadgetInner({
           </svg>
         )}
       </div>
 
-      {/* Footer: honesty indicators — how much real history backs this view */}
-      {series && (
-        <div className="flex items-center justify-between text-xs text-muted px-0.5">
-          <span>
-            {series.sample_count.toLocaleString()} venue samples in window
-            {historyLabel ? ` · ${historyLabel} recorded` : ""}
-          </span>
-          <span>
-            {series.venues.length === 1
-              ? "1 venue — pairwise spread needs ≥2 venues"
-              : series.mode === "leader"
-                ? "baseline = most-liquid venue"
-                : "baseline = volume-weighted consensus"}
-          </span>
-        </div>
-      )}
       {analytics && analytics.band.sample_count > 0 && (
         <div className="flex items-center justify-between text-xs text-muted px-0.5 mt-0.5">
           <span title="Distribution of the per-poll max–min venue spread over this window">
```

---

### R7. Security Page Explanations & MFA Wording

#### 7.1 Scope & Target Component
- **File**: `frontend/src/components/EngineSettings.tsx`
- **Sub-tasks**:
  1. Security Model explanatory box styling and icon.
  2. Removal of the MFA unlock explanation paragraph.
  3. Removal of authenticator app examples from the TOTP description label.

#### 7.2 Sub-task 1: Security Model Explanatory Box
##### 7.2.1 Location
- **Lines**: 2402–2411 of `EngineSettings.tsx`
##### 7.2.2 Current Code
```tsx
            {/* Security declaration */}
            <div className="bg-surface border border-surface-border rounded-lg p-3.5 shadow-2xs space-y-2">
              <div className="flex items-center space-x-1.5 font-bold text-slate-800 text-xs font-mono">
                <ShieldCheck className="w-3.5 h-3.5 text-emerald-600" />
                <span>Security Model</span>
              </div>
              <p className="text-xs font-mono text-slate-600 leading-relaxed">
                No private keys or seed phrases are stored remotely or accessed by any third party; all credentials remain encrypted locally on your system using AES-256-GCM with Argon2id key derivation.
              </p>
            </div>
```
##### 7.2.3 Reference Comparison (Legal Disclaimer Box on About Tab, Lines 2489–2501)
```tsx
              {/* Legal & Financial Disclaimer */}
              <div className="p-4 bg-slate-50 border border-surface-border rounded-lg space-y-2 text-xs font-mono text-slate-600 leading-relaxed">
                <div className="flex items-center space-x-1.5 font-bold text-slate-800 text-xs">
                  <Scale className="w-3.5 h-3.5 text-slate-500" />
                  <span>Legal & Financial Risk Disclaimer</span>
                </div>
                <p>
                  This software is provided for research, simulation...
                </p>
              </div>
```
##### 7.2.4 Required Changes
- Change background: Replace `bg-surface` with `bg-slate-50`.
- Change icon style: Replace `text-emerald-600` on `ShieldCheck` with neutral slate `text-slate-500` (matching `Scale className="w-3.5 h-3.5 text-slate-500"`).

#### 7.3 Sub-task 2: Removal of MFA Unlock Explanation Paragraph
##### 7.3.1 Location
- **Lines**: 2397–2399 of `EngineSettings.tsx`
##### 7.3.2 Current Code
```tsx
              <p className="text-[10px] font-sans text-slate-500 leading-relaxed border-t border-surface-border pt-2">
                MFA gates the unlock path in this app — an operator needs the code, not just the password. The TOTP secret is stored inside the encrypted vault; it does not add encryption strength against theft of the vault file itself.
              </p>
```
##### 7.3.3 Required Changes
- Delete this `<p>` element completely.

#### 7.4 Sub-task 3: Removal of Authenticator App Examples from TOTP Description Label
##### 7.4.1 Location
- **Lines**: 2280–2285 of `EngineSettings.tsx`
##### 7.4.2 Current Code
```tsx
                    <p className="text-[11px] font-mono text-slate-500">
                      {vaultStatus?.mfa_enabled
                        ? "A 6-digit authenticator code is required to unlock the vault and rotate the password"
                        : "Optionally require a 6-digit TOTP code (Google Authenticator, 1Password, Authy) at vault unlock"}
                    </p>
```
##### 7.4.3 Required Changes
- Remove `(Google Authenticator, 1Password, Authy)` from line 2283.
- The new string is: `"Optionally require a 6-digit TOTP code at vault unlock"`.

#### 7.5 Proposed Diff for `EngineSettings.tsx`
```diff
--- a/frontend/src/components/EngineSettings.tsx
+++ b/frontend/src/components/EngineSettings.tsx
@@ -2280,7 +2280,7 @@ export const EngineSettings: React.FC<EngineSettingsProps> = ({
                     <p className="text-[11px] font-mono text-slate-500">
                       {vaultStatus?.mfa_enabled
                         ? "A 6-digit authenticator code is required to unlock the vault and rotate the password"
-                        : "Optionally require a 6-digit TOTP code (Google Authenticator, 1Password, Authy) at vault unlock"}
+                        : "Optionally require a 6-digit TOTP code at vault unlock"}
                     </p>
                   </div>
                 </div>
@@ -2394,15 +2394,11 @@ export const EngineSettings: React.FC<EngineSettingsProps> = ({
                   </button>
                 </div>
               )}
-
-              <p className="text-[10px] font-sans text-slate-500 leading-relaxed border-t border-surface-border pt-2">
-                MFA gates the unlock path in this app — an operator needs the code, not just the password. The TOTP secret is stored inside the encrypted vault; it does not add encryption strength against theft of the vault file itself.
-              </p>
             </div>
 
             {/* Security declaration */}
-            <div className="bg-surface border border-surface-border rounded-lg p-3.5 shadow-2xs space-y-2">
+            <div className="bg-slate-50 border border-surface-border rounded-lg p-3.5 shadow-2xs space-y-2">
               <div className="flex items-center space-x-1.5 font-bold text-slate-800 text-xs font-mono">
-                <ShieldCheck className="w-3.5 h-3.5 text-emerald-600" />
+                <ShieldCheck className="w-3.5 h-3.5 text-slate-500" />
                 <span>Security Model</span>
               </div>
               <p className="text-xs font-mono text-slate-600 leading-relaxed">
```

---

## 3. Test Coverage & Verification Analysis

### 3.1 Existing Tests Audit
1. `BotLifecycleControls.test.ts`:
   - Tests render of all 4 buttons (`Start`, `Pause`, `Reduce Only`, `Stop`).
   - Tests button states (`text-warn-strong`, disabled state).
   - Tests `resolveBotStatus` and `BotPhaseBadge` / `StrategyStatusBadge`.
   - **Impact**: Changing outer border padding from `px-2 py-1` to `p-1` does not break any existing assertions.
2. `SignalTrendGadget.test.ts`:
   - Tests initial render of header, window selector, and empty state message.
   - **Impact**: Removing the footer line does not break existing test assertions.
3. `LiveMatrix.test.ts` & `ChallengerR3R4R5.test.ts`:
   - Tests status badge alignment and locked dock height (`h-96`).
   - No tests assert on `TrendingUp` / `TrendingDown` in matrix cell delta badges.
   - **Impact**: Removing the trend icons does not break any existing tests.
4. `EngineSettings.test.ts`:
   - Tests sidebar exchange filtering and transaction history reset dialog.
   - Does not assert on MFA explanation paragraph or TOTP example apps.
   - **Impact**: Zero regressions on existing test suite.

### 3.2 Recommended Additional Tests
To guarantee the changes stay verified and regression-free, implementers/test-writers should add the following targeted tests:
1. **In `BotLifecycleControls.test.ts`**:
   ```ts
   test("BotLifecycleControls wrapper has symmetric padding p-1", () => {
     const html = renderToStaticMarkup(React.createElement(BotLifecycleControls, {
       phase: "idle",
       hasOpenPositions: false,
       onStart: () => {},
       onPause: () => {},
       onStop: () => {},
     }));
     assert.match(html, /class="[^"]*\bp-1\b[^"]*"/);
     assert.doesNotMatch(html, /px-2 py-1/);
   });
   ```
2. **In a new test or component test for `LiveMatrix.tsx`**:
   ```ts
   test("MatrixCellView renders delta badge without trend arrow SVG icons", () => {
     // Render cell with non-zero delta
     // Assert delta text "Δ +2.5" is rendered
     // Assert HTML does not contain TrendingUp or TrendingDown icons
   });
   ```
3. **In `SignalTrendGadget.test.ts`**:
   ```ts
   test("SignalTrendGadget omits footer line with venue samples and baseline description", () => {
     // Render with mock series
     // Assert html does not match /venue samples in window/
     // Assert html does not match /baseline = /
   });
   ```
4. **In `EngineSettings.test.ts`**:
   ```ts
   test("Security tab renders neutral Security Model box and omits MFA unlock explanation and app examples", () => {
     // Assert Security Model box has class "bg-slate-50" and icon has "text-slate-500"
     // Assert html does not contain "MFA gates the unlock path in this app"
     // Assert html does not contain "(Google Authenticator, 1Password, Authy)"
     // Assert html contains "Optionally require a 6-digit TOTP code at vault unlock"
   });
   ```

---

## 4. Summary Matrix of Required File Modifications

| Component | Target File | Lines | Modification | Risk |
|:---|:---|:---|:---|:---|
| **BotLifecycleControls** | `frontend/src/components/BotLifecycleControls.tsx` | 188 | Replace `px-2 py-1` with `p-1` | None |
| **LiveMatrix** | `frontend/src/components/LiveMatrix.tsx` | 2–4, 137–141 | Remove `TrendingUp` / `TrendingDown` from imports and cell delta badge JSX | None |
| **SignalTrendGadget** | `frontend/src/components/SignalTrendGadget.tsx` | 315–323, 628–643 | Remove footer line (`venue samples... baseline =...`) and unused `historyLabel` memo | None |
| **EngineSettings** | `frontend/src/components/EngineSettings.tsx` | 2283 | Remove `(Google Authenticator, 1Password, Authy)` from TOTP description | None |
| **EngineSettings** | `frontend/src/components/EngineSettings.tsx` | 2397–2399 | Remove paragraph `MFA gates the unlock path in this app...` | None |
| **EngineSettings** | `frontend/src/components/EngineSettings.tsx` | 2403–2405 | Change `bg-surface` to `bg-slate-50`; change `text-emerald-600` to `text-slate-500` | None |
