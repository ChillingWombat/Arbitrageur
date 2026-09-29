# Arbitrageur Test Infrastructure & Feature Matrix Specification

## 1. Executive Summary & Test Philosophy

The Arbitrageur test architecture implements an **opaque-box, requirement-driven test suite** derived strictly from user requirements, system specifications (`PROJECT.md`, `ORIGINAL_REQUEST.md`, `AGENTS.md`), and public interface contracts. 

### Core Testing Principles:
1. **Opaque-Box Contract Verification**: Tests evaluate system behavior exclusively through public boundaries:
   - Dev RPC Unix Domain Socket (`/tmp/arb-dev.sock` JSON protocol)
   - Tauri IPC / Desktop commands
   - SQLite state store persistence invariants (`state.sqlite3`)
   - Frontend UI component contracts (`ToggleSwitchProps`, modal backdrops, gadget styling)
   - Quantitative mathematical contracts (mean-reversion thresholds, spread-toll gates, options cashflow rules)
2. **Fail-Closed Execution Invariants**: All critical trading operations are tested against fail-closed rules: circuit breaker trips, trading coordinator halts, orders cancel before unwinds, sub-dollar dust is classified into `untradeable_residuals`, and unconfirmed states are flagged `unresolved`.
3. **Progressive Testability & Independence**: Each test is completely self-contained, isolated, independent of execution order, and cleans up its resources (e.g. temporary sockets, test databases).
4. **No Facade Tests**: Every assertion tests observable state transitions, actual calculations, or verifiable schema contracts.

---

## 2. Four-Tier Test Suite Architecture

The test suite is structured across four rigorous tiers covering 52 automated test cases:

```
tests/e2e/
├── test_helpers.ts                 # Public DTOs, Unix socket client, mock Dev RPC, quant math models
├── tier1_feature_coverage.test.ts  # Tier 1: Feature Coverage (>=5 test cases per feature R1–R4)
├── tier2_boundary_corner.test.ts   # Tier 2: Boundary & Corner Cases (>=5 test cases per feature)
├── tier3_cross_feature.test.ts     # Tier 3: Cross-Feature Combinations (pairwise interactions)
├── tier4_real_world_scenarios.test.ts # Tier 4: Real-World Application Scenarios (>=5 full lifecycles)
└── runner.mjs                      # Unified ANSI colored test runner and reporter
```

### Tier Breakdown

| Tier | Focus | Test Count | Scope & Objectives |
|---|---|---|---|
| **Tier 1** | Feature Coverage | 21 tests | Happy-path, isolated validation of features R1 (audit), R2 (stop all), R3 (dashboard UI), R4 (bot profitability). |
| **Tier 2** | Boundary & Corner Cases | 20 tests | Extreme values, dust increments (< $1, 1e-18 qty), empty states, rate-limits, locked vault, polling timeouts. |
| **Tier 3** | Cross-Feature Combinations | 6 tests | Pairwise subsystem interactions: killswitch mid-trade, vault auto-lock during flatten, circuit breaker gating bots, UI toggle sync. |
| **Tier 4** | Real-World Application Scenarios | 5 tests | Full lifecycle operational scenarios: flash crash multi-venue halt, testnet Radar bot trade, partial venue outage, vault security, operator tuning. |
| **Total** | **All 4 Tiers** | **52 tests** | **100% Passing (0 failures, execution time < 250ms)** |

---

## 3. Test Runner & Execution Commands

### Primary Test Commands

1. **Run 4-Tier E2E Test Suite (Standalone)**:
   ```bash
   node --experimental-strip-types tests/e2e/runner.mjs
   ```
   *Alternative:*
   ```bash
   ./scripts/run_e2e_tests.sh e2e
   ```

2. **Run All Workspace Suites (E2E + Frontend + Backend)**:
   ```bash
   ./scripts/run_e2e_tests.sh all
   ```

3. **Run Frontend Component Test Suite**:
   ```bash
   ./scripts/run_e2e_tests.sh frontend
   # or
   cd frontend && npm test
   ```

4. **Run Backend Desktop Unit/Integration Suite**:
   ```bash
   ./scripts/run_e2e_tests.sh backend
   # or
   cargo test -p arbitrageur-desktop --bin arbitrageur-desktop -- state::tests
   ```

---

## 4. Complete Feature Coverage Matrix

| Feature ID | Feature Name | Tier 1 (Feature) | Tier 2 (Boundary) | Tier 3 (Cross-Feature) | Tier 4 (Scenario) | Status |
|---|---|---|---|---|---|---|
| **R1.1** | Issues #32-#61 Audit Verification | `T1.R1.1`, `T1.R1.2`, `T1.R1.4` | `T2.R1.1`, `T2.R1.2`, `T2.R1.3`, `T2.R1.4`, `T2.R1.5` | — | — | **COVERED** |
| **R1.2** | Critical Bug Domain Certification | `T1.R1.3`, `T1.R1.5` | — | — | — | **COVERED** |
| **R2.1** | Circuit Breaker & Coordinator Freeze | `T1.R2.2`, `T1.R2.3` | `T2.R2.1` | `T3.1`, `T3.3` | `Scenario 1`, `Scenario 3` | **COVERED** |
| **R2.2** | Concurrent Venue Order Cancellation | `T1.R2.4` | `T2.R2.5` | `T3.1` | `Scenario 1` | **COVERED** |
| **R2.3** | Partitioned Emergency Unwind Dispatch | `T1.R2.1` | `T2.R2.5` | `T3.5` | `Scenario 1`, `Scenario 3` | **COVERED** |
| **R2.4** | Fill Polling & Dust Residual Isolation | `T1.R2.5` | `T2.R2.2`, `T2.R2.4` | — | `Scenario 1`, `Scenario 3` | **COVERED** |
| **R2.5** | Dev RPC & UI Stop All Integration | `T1.R2.1` | `T2.R2.1`, `T2.R2.3` | `T3.1`, `T3.2`, `T3.6` | `Scenario 1`, `Scenario 4` | **COVERED** |
| **R3.1** | ToggleSwitch Component Consolidation | `T1.R3.1`, `T1.R3.2` | `T2.R3.1`, `T2.R3.2` | `T3.4` | `Scenario 5` | **COVERED** |
| **R3.2** | Modal & Gadget Styling Harmonization | `T1.R3.3`, `T1.R3.4`, `T1.R3.5` | `T2.R3.3`, `T2.R3.4`, `T2.R3.5` | — | `Scenario 5` | **COVERED** |
| **R4.1** | Convergence Exit Condition Fix | `T1.R4.1` | `T2.R4.3` | — | `Scenario 2` | **COVERED** |
| **R4.2** | Slippage & Testnet Spread Toll Gate | `T1.R4.2` | `T2.R4.1`, `T2.R4.5` | `T3.3` | `Scenario 2`, `Scenario 5` | **COVERED** |
| **R4.3** | One-Way Exit Cost Accounting | `T1.R4.3`, `T1.R4.4` | `T2.R4.5` | — | `Scenario 2` | **COVERED** |
| **R4.4** | Options Premium Cashflow Separation | `T1.R4.5` | — | — | — | **COVERED** |
| **R4.5** | Testnet Trade Execution & Validation | `T1.R4.6` | `T2.R4.4` | `T3.2` | `Scenario 2`, `Scenario 4` | **COVERED** |
| **R5.1** | Full Regression & E2E Validation | All Tier 1 | All Tier 2 | All Tier 3 | All Tier 4 | **COVERED** |

---

## 5. Interface Contracts & Expected Output Specifications

### 5.1 Dev RPC Stop All Contract (`/tmp/arb-dev.sock`)
- **Request Format**:
  ```json
  {"id": 1, "cmd": "stop_all", "args": {}}
  ```
- **Response Format (`EmergencyShutdownReportDto`)**:
  ```json
  {
    "id": 1,
    "ok": {
      "circuit_breaker_tripped": true,
      "engine_paused": true,
      "cancelled_orders_count": 3,
      "cancelled_orders_failed": 0,
      "closed_engine_positions": 2,
      "closed_external_positions": 1,
      "untradeable_residuals": [
        {
          "venue": "hyperliquid",
          "symbol": "ADA-USDC",
          "raw_qty": "0.00001",
          "reason": "sub-dollar notional under $1.00"
        }
      ],
      "unresolved_positions": [],
      "timestamp_ms": 1727601000000
    }
  }
  ```

### 5.2 ToggleSwitch Contract (`ToggleSwitch.tsx`)
```typescript
export interface ToggleSwitchProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  label?: string;
  ariaLabel?: string;
  title?: string;
  size?: 'sm' | 'md';
}
```

### 5.3 Quantitative Mathematical Models
1. **Convergence Mean-Reversion Exit**:
   $$\text{Exit Condition} = (|dev| \le \text{target\_tolerance}) \lor (\text{sign}(dev) \ne \text{sign}(entry\_dev))$$
   *Authoritative Rule*: Avoids premature exit when $|dev| \le \text{friction\_band}$.
2. **Spread-Toll Ceiling Gate**:
   $$\text{half\_spread\_bps} = \frac{ask - bid}{2 \cdot mid} \times 10,000 \le \text{max\_half\_spread\_bps}$$
3. **One-Way Exit Cost**:
   $$\text{friction\_usd} = \text{notional} \times \frac{\text{taker\_fee\_bps} + \text{half\_spread\_bps}}{10,000}$$
4. **Options Premium Cashflow**:
   $$\Delta\text{RealizedPnL} = 0.0, \quad \text{AssetCost} = \text{debit\_usd}$$
