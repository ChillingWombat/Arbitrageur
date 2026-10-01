# Spec: Engine Lifecycle Disambiguation — Stopped vs. Paused State and Start vs. Resume Controls

## Problem Statement

Traders and risk operators experienced serious cognitive dissonance and operational ambiguity regarding engine control states in the desktop cockpit:

Following an emergency exit triggered via the "Stop All" button, resting orders are cancelled, and all open positions across all venues are unwound. However, the system previously reported this state as "Paused" rather than "Stopped". Specifically:
1. **Contradictory Header Action ("Resume Engine")**: The primary header control displayed `[ > Resume Engine ]`. A trader who has executed an emergency portfolio liquidation does not intend to "resume" an in-flight operation; they expect the engine to be cleanly **Stopped**, and the subsequent action to be **Start Engine** (or **Start**).
2. **Inaccurate Risk Status Badge in Positions Panel**: The bottom control panel of the Positions Panel rendered an amber `[ • Paused ]` badge beside net profit. Because positions had been entirely liquidated, labeling the engine as "Paused" implied that executions were merely suspended while positions were being held.
3. **Mismatched Trading Status in Portfolio Monitor**: The top Trading Status telemetry card displayed `[ Paused ]` with the descriptive subtitle `Engine paused`, falsely communicating that the engine was in a temporary pause mode rather than an intentional emergency stop.

---

## Solution

A rigorous architectural and UI disambiguation separating **deliberate pause** from **emergency stop**:

1. **First-Class Stopped State**:
   - The engine explicitly distinguishes `Stopped` (entered via Stop All, emergency killswitch, or explicit shutdown, where positions are unwound) from `Paused` (entered via Pause Engine, where positions are preserved).
   - This state is persisted durably in SQLite storage (`engine.stopped = "1"`), ensuring the stopped posture survives application crashes, daemon restarts, or service reboots.
2. **Directional Header Control ("Start Engine" vs. "Resume Engine")**:
   - When the engine is **Stopped**: The header action displays `<Play /> Start Engine`.
   - When the engine is **Paused**: The header action displays `<Play /> Resume Engine`.
   - When the engine is **Running**: The header action displays `<Pause /> Pause Engine`.
3. **Harmonized Badge & Telemetry Semantics**:
   - In the Positions Panel, a stopped engine renders a clean, neutral `[ • Stopped ]` badge styled consistently with stopped strategy bots (`bg-surface-raised border-surface-border text-slate-500`).
   - In Portfolio Monitor, the Trading Status verdict displays `[ Stopped ]` with the subtitle `Engine stopped` (or `Engine stopped (killswitch tripped)` when circuit breaker limits remain engaged).
4. **Atomic Start IPC & Dev RPC**:
   - Added `start_engine` IPC command and dev RPC endpoint that atomically clears both `is_stopped` and `is_paused` flags and persists the clean state to SQLite.

---

## User Stories

1. As an operator executing an emergency exit via "Stop All", I want the engine status to immediately transition to "Stopped" rather than "Paused", so that I have absolute clarity that trading has halted and risk is flat.
2. As a trader looking at the header controls after a Stop All event, I want the action button to display "Start Engine" instead of "Resume Engine", so that I understand restarting coordination is a fresh start rather than a continuation.
3. As a desk supervisor reviewing the header tooltip after Stop All, I want the tooltip to read "Start Engine: Start execution coordination...", so that the operational instructions match the button label.
4. As an operator who temporarily paused the engine using "Pause Engine", I want the header button to display "Resume Engine", so that I know my open positions are preserved and ready to resume.
5. As a trader inspecting the Positions Panel after an emergency shutdown, I want the status pill to display "[ • Stopped ]" in neutral slate text rather than amber "[ • Paused ]", so that the visual hierarchy aligns with flat exposure.
6. As a risk officer monitoring Portfolio Monitor after a killswitch event, I want the Trading Status card to display "Stopped" with subtext "Engine stopped", so that audit snapshots record an intentional shutdown.
7. As an operator who restarts the system service while the engine is stopped, I want the "Stopped" state to be restored from SQLite, so that a daemon reboot never silently converts a stopped engine into a paused or running engine.
8. As a developer automating desk tests via Dev RPC, I want the `system_status` command to return `"is_stopped": true`, so that test harnesses can programmatically verify emergency shutdown completion.
9. As an operator calling `start_engine` via Dev RPC or clicking "Start Engine" in the UI, I want both `is_stopped` and `is_paused` to atomically clear to `false`, so that the engine cleanly returns to active coordination.
10. As a risk engineer, I want `start_engine` to be strictly rejected if the circuit breaker remains tripped, ensuring safety interlocks cannot be bypassed without an explicit circuit-breaker reset.
11. As a trader who reset the circuit breaker in Portfolio Monitor after a killswitch event, I want the header button to remain "Start Engine" (now enabled) until I explicitly click it, preventing unexpected auto-start.
12. As an operator running on mobile or narrow desktop screens, I want the "Start Engine" button and icon to render with consistent padding without wrapping, preserving navigation layout integrity.
13. As an institutional compliance officer, I want the system to record an audit log whenever the engine transitions between Running, Paused, and Stopped, ensuring complete state traceability.
14. As an auditor running automated unit tests, I want the frontend test runner (`npm test`) to assert that `Header.test.ts` checks for "Start Engine" when `is_stopped: true`, preventing regressions.
15. As a backend developer running Rust test suites, I want `cargo test -p arbitrageur-desktop` to verify `start_engine_impl` and `set_engine_paused_impl` state transitions, ensuring 100% backend safety.
16. As a desktop user navigating between Strategy pages, I want all strategy bot status badges to display "Stopped" whenever the global engine is stopped, maintaining platform-wide coherence.
17. As an operator investigating transient network disconnections, I want WS feed lane supervisors to respect the stopped state, ensuring dead-lane reconnects do not trigger unwanted strategy entries.
18. As a live-money desk trader, I want identical Stopped vs. Paused guarantees to apply in live mode as in testnet mode, honoring the standing principle that live mode is the primary product path.

---

## Implementation Decisions

### Core State Invariants

- **`is_stopped`**: Indicates that the engine was brought to an intentional full stop (e.g. via Stop All, emergency killswitch, or explicit shutdown) where positions were closed or liquidated.
- **`is_paused`**: Indicates that execution dispatch is temporarily suspended.
- **Invariance Rule**: Any transition into `Stopped` automatically asserts `is_paused = true`. However, an unpause or start operation (`paused = false`) atomically clears both `is_paused = false` and `is_stopped = false`.

### State Transitions

| Action / Event | `is_stopped` | `is_paused` | Header Button | Positions Panel Badge | Trading Status Verdict |
| :--- | :---: | :---: | :---: | :---: | :---: |
| Normal Running | `false` | `false` | `Pause Engine` | `Healthy` / `Active` | `Healthy` |
| Pause Engine | `false` | `true` | `Resume Engine` | `Paused` | `Paused` |
| Stop All / Killswitch | `true` | `true` | `Start Engine` (disabled if tripped) | `Stopped` | `Stopped` |
| Reset Circuit Breaker | `true` | `true` | `Start Engine` (enabled) | `Stopped` | `Stopped` |
| Start Engine | `false` | `false` | `Pause Engine` | `Healthy` / `Active` | `Healthy` |

### Architectural Interfaces

1. **Storage Layer**:
   - SQLite key `engine.stopped` (`"1"` for stopped, `"0"` for cleared). Loaded on startup in the application state constructor.
2. **Application State**:
   - Shared atomic synchronization via `RwLock<bool>` on the engine state container.
3. **IPC & Dev RPC**:
   - `get_system_status` returns `is_stopped: bool`.
   - `start_engine` invokes `start_engine_impl` which delegates to unpausing and clearing the stopped flag.
4. **Presentation Layer**:
   - `Header`: Selects button icon and label between `Start Engine`, `Resume Engine`, and `Pause Engine`.
   - `PositionsPanel`: Surfaces `[ • Stopped ]` pill using neutral slate styling.
   - `PortfolioMonitor`: Surfaces `[ Stopped ]` in the Trading Status card and `Engine stopped` in the detail caption.

---

## Testing Decisions

### Guiding Principles

Tests verify observable external behavior, rendered DOM elements, IPC command contracts, and state invariants without relying on internal private structures:
- The text "Start Engine" must appear when `is_stopped` is true.
- The text "Resume Engine" must never appear when `is_stopped` is true.
- The badge "Stopped" must render in the Positions Panel and Portfolio Monitor when stopped.
- `start_engine` must atomically clear both flags in the backend.

### Test Seams

1. **Frontend Unit Seam (`node --test` via `npm test`)**:
   - `Header.test.ts`: Verifies `Start Engine` renders when `is_stopped: true` and `Resume Engine` renders when `is_paused: true && !is_stopped`.
   - `PositionsPanel.test.ts`: Verifies bottom control panel renders `Stopped` badge when `is_stopped: true`.
   - `PortfolioMonitor.test.ts`: Verifies Trading Status card renders `Stopped` verdict and `Engine stopped` subtext.
2. **Backend Seam (`cargo test -p arbitrageur-desktop`)**:
   - `commands::tests`: Verifies `start_engine_impl` and `set_engine_paused_impl` state mutations and persistence.
   - `execution::tests`: Verifies `execute_emergency_stop_all_impl` stamps `is_stopped = true` and `engine.stopped = "1"`.

### Prior Art

- `frontend/src/components/Header.test.ts`: Existing test harness for header pause/resume interactions.
- `frontend/src/components/PortfolioMonitor.test.ts`: Strategy filtering and allocation tests.
- `frontend/src-tauri/src/commands.rs`: `test_emergency_stop_all_deduplication_and_dust_adversarial`.

---

## Out of Scope

- Automated strategy restart upon `start_engine` (individual strategy bots strictly remain stopped per Requirement R1 and must be started individually on each strategy page).
- Hardware security module (HSM) session teardown on engine stop (credential vault session rules remain governed by the vault timeout policy).

---

## Further Notes

- All changes have been compiled, tested, and verified on the live desktop service (`arbitrageur-testnet.service`).
- All 606 backend tests and all 23 frontend test suites pass with 0 failures.
- Clippy and `cargo fmt` passed with zero warnings.
