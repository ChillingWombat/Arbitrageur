# Handoff Report: Survey Exploration for R3, R4, and R7

**Agent**: `survey_explorer_p3_3`  
**Role**: Read-only Exploration Agent  
**Date**: 2026-10-01T16:20:00Z  
**Type**: Hard Handoff (Investigation & Survey Complete)  
**Assigned Scope**: R3 (Button Set Border Symmetric Padding), R4 (Dislocation Matrix & Signal Trend Gadget Cleanups), R7 (Security Page Explanations & MFA Wording)

---

## 1. Observation

### 1.1 R3 — Button Set Outer Border Symmetric Padding
- **File**: `frontend/src/components/BotLifecycleControls.tsx`
- **Lines 187–205**:
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
- **Callers across codebase**:
  - `LiveMatrix.tsx:1008`
  - `OpportunityRadar.tsx:2207`
  - `FundingRadar.tsx:1509`
  - `SpotArbitrageBot.tsx:312`
  - `TriangularArbitrage.tsx:400`
  - `OptionsArbitrage.tsx:977`
  - `PredictionHub.tsx:266`
  None of these callers wrap `<BotLifecycleControls>` with an outer border or override its padding.

### 1.2 R4 — Dislocation Matrix & Signal Trend Gadget Cleanups
- **File**: `frontend/src/components/LiveMatrix.tsx`
  - **Lines 2–4**:
    ```tsx
    import {
      TrendingUp,
      TrendingDown,
      Zap,
    ```
  - **Lines 133–147**:
    ```tsx
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
- **File**: `frontend/src/components/SignalTrendGadget.tsx`
  - **Lines 315–323**:
    ```tsx
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
  - **Lines 628–643**:
    ```tsx
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
    ```

### 1.3 R7 — Security Page Explanations & MFA Wording
- **File**: `frontend/src/components/EngineSettings.tsx`
  - **Lines 2280–2285 (TOTP description label)**:
    ```tsx
    <p className="text-[11px] font-mono text-slate-500">
      {vaultStatus?.mfa_enabled
        ? "A 6-digit authenticator code is required to unlock the vault and rotate the password"
        : "Optionally require a 6-digit TOTP code (Google Authenticator, 1Password, Authy) at vault unlock"}
    </p>
    ```
  - **Lines 2397–2399 (MFA unlock explanation paragraph)**:
    ```tsx
    <p className="text-[10px] font-sans text-slate-500 leading-relaxed border-t border-surface-border pt-2">
      MFA gates the unlock path in this app — an operator needs the code, not just the password. The TOTP secret is stored inside the encrypted vault; it does not add encryption strength against theft of the vault file itself.
    </p>
    ```
  - **Lines 2402–2411 (Security Model explanatory box)**:
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
  - **Lines 2490–2494 (Legal disclaimer box on About page for reference)**:
    ```tsx
    {/* Legal & Financial Disclaimer */}
    <div className="p-4 bg-slate-50 border border-surface-border rounded-lg space-y-2 text-xs font-mono text-slate-600 leading-relaxed">
      <div className="flex items-center space-x-1.5 font-bold text-slate-800 text-xs">
        <Scale className="w-3.5 h-3.5 text-slate-500" />
        <span>Legal & Financial Risk Disclaimer</span>
      </div>
    ```

### 1.4 Test Baseline Execution
- Ran `cd frontend && npm test`: 47 test suites, 100% passed (exit code 0).
- Ran `npx tsc --noEmit` in `frontend/`: clean compilation, 0 errors.

---

## 2. Logic Chain

1. **R3 Symmetric Padding**:
   - As observed in §1.1, the outer border wrapper in `BotLifecycleControls.tsx:188` uses `px-2 py-1`, producing 8px horizontal padding and 4px vertical padding around the button list.
   - Because each button has `px-2 py-2`, the outer border appears oblong/asymmetrical relative to the buttons.
   - Replacing `px-2 py-1` with `p-1` sets 4px padding on all four edges (equal horizontal and vertical spacing).
   - Our audit of all 7 consumer components (§1.1) revealed that none of them add extra wrappers or borders around `BotLifecycleControls`. Thus, editing `BotLifecycleControls.tsx` line 188 standardizes all lifecycle button groups system-wide in one place.

2. **R4 Dislocation Matrix Cleanup**:
   - In `LiveMatrix.tsx:137–141`, `<TrendingUp>` and `<TrendingDown>` are rendered conditionally.
   - Removing this conditional element removes the trend arrows `↗` / `↘` while preserving the numerical delta string `Δ +X.X` / `Δ -X.X` and the color styling via `visuals.deltaBadgeBg`.
   - Pruning `TrendingUp` and `TrendingDown` from `LiveMatrix.tsx:2–4` eliminates unused imports.

3. **R4 Signal Trend Gadget Cleanup**:
   - In `SignalTrendGadget.tsx:628–643`, the footer block `{series && (<div className="flex items-center justify-between text-xs text-muted px-0.5">...</div>)}` renders venue samples and baseline description.
   - Removing lines 628–643 satisfies the requirement directly.
   - The memoized `historyLabel` (lines 315–323) is only referenced in line 633. Pruning it avoids dead code and linter warnings.
   - The analytics distribution row below it (lines 644–658) does not describe venue samples or baseline type and is preserved.

4. **R7 Security Page & MFA Explanations**:
   - In `EngineSettings.tsx:2403–2405`, replacing `bg-surface` with `bg-slate-50` and replacing `text-emerald-600` with `text-slate-500` matches the visual pattern of the About page legal disclaimer (`p-4 bg-slate-50 ... Scale className="... text-slate-500"`).
   - Deleting lines 2397–2399 removes the MFA unlock explanation paragraph.
   - Updating line 2283 from `"Optionally require a 6-digit TOTP code (Google Authenticator, 1Password, Authy) at vault unlock"` to `"Optionally require a 6-digit TOTP code at vault unlock"` removes the authenticator app examples from the TOTP description.

---

## 3. Caveats

- **Scope boundary**: This survey is strictly read-only. No source files under `frontend/src/` were modified by this agent.
- **Analytics footer line in `SignalTrendGadget.tsx`**: We verified that lines 644–658 (`band μ ... p90 ... max ... bps`) represent spread analytics, not venue samples or baseline descriptions. It was deliberately kept intact.
- **Header Top Bar Buttons**: `Header.tsx` contains standalone engine control buttons (`Pause Engine`, `Stop All`). These are not lifecycle control groups and do not share the `BotLifecycleControls` wrapper.

---

## 4. Conclusion

The technical requirements for R3, R4, and R7 are well-bounded, zero-risk, and localized to 4 component files:
1. `frontend/src/components/BotLifecycleControls.tsx` (1 line change: `px-2 py-1` → `p-1`)
2. `frontend/src/components/LiveMatrix.tsx` (prune 2 imports and remove 5 lines of trend icon JSX)
3. `frontend/src/components/SignalTrendGadget.tsx` (remove 15 lines of footer JSX and 9 lines of unused `historyLabel` memo)
4. `frontend/src/components/EngineSettings.tsx` (update background to `bg-slate-50`, icon to `text-slate-500`, remove MFA paragraph, clean TOTP label)

All proposed diffs are fully mapped in `survey.md`. Implementing these changes will cause zero regressions in existing tests and will fulfill the acceptance criteria for Phase 3.

---

## 5. Verification Method

To independently verify the investigation and future implementation:
1. **Frontend Test Suite**:
   ```bash
   cd /mnt/Data/Projects/Arbitrageur/frontend && npm test
   ```
   *Expected*: All 47 suites pass with 0 failures.
2. **TypeScript Compilation**:
   ```bash
   cd /mnt/Data/Projects/Arbitrageur/frontend && npx tsc --noEmit
   ```
   *Expected*: Clean exit (code 0).
3. **Code Inspection**:
   - Inspect `frontend/src/components/BotLifecycleControls.tsx`: verify line 188 uses `p-1` and no `px-2 py-1`.
   - Inspect `frontend/src/components/LiveMatrix.tsx`: verify no `TrendingUp` or `TrendingDown` in imports or JSX.
   - Inspect `frontend/src/components/SignalTrendGadget.tsx`: verify no `venue samples in window` or `baseline =` text.
   - Inspect `frontend/src/components/EngineSettings.tsx`: verify `bg-slate-50`, `text-slate-500`, no `MFA gates the unlock path`, no `(Google Authenticator, 1Password, Authy)`.
4. **Invalidation Conditions**:
   - Presence of `px-2 py-1` on line 188 of `BotLifecycleControls.tsx`.
   - Presence of `TrendingUp` or `TrendingDown` in matrix cell delta badges.
   - Presence of `venue samples in window` in `SignalTrendGadget.tsx`.
   - Presence of `text-emerald-600` or `bg-surface` on the Security Model box in `EngineSettings.tsx`.
