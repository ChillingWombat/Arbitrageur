# Specification: PopDEX Toolkit Streamlining and Kraken Live-Only Environment Enforcement

## Problem Statement

Traders and operators configuring exchange accounts and executing strategies encounter two operational clarity and execution fidelity issues:
1. **Unnecessary PopDEX Setup Noise**: In the PopDEX Agent Setup Toolkit modal, external links to third-party faucets and verbose transaction rejection error explanations (-32000 deposit requirement) clutter the account onboarding flow.
2. **Ambiguous Kraken Testnet Semantics**: Kraken was previously registered as a testnet-capable venue pointing to a demo futures endpoint, but lacks full spot and unified demo infrastructure, creating inconsistent expectations in Testnet mode where real venue demo endpoints are expected. Per project standards ("Live mode is the product" and fail-closed venue admission), Kraken should operate strictly in Live mode alongside other mainnet-only venues (such as Coinbase and Aster) without pseudo-testnet exposure.

## Solution

1. **PopDEX Setup Ergonomics**: Clean up the PopDEX Agent Setup Toolkit by removing all references and hyperlinks to external faucets as well as the initial deposit error explanation under the primary account field.
2. **Strict Live-Only Classification for Kraken**: Fully decommission Kraken testnet adapter registration, remove Kraken and Kraken Futures from the testnet-capable venue registry, and ensure Kraken is only admitted, listed, and routed when the engine runs in Live mode.

## User Stories

1. As a trader using the PopDEX Agent Setup Toolkit, I want a clean network selection interface without external faucet links, so that my onboarding workflow is focused and distraction-free.
2. As a trader entering my PopDEX main wallet address, I want a streamlined input card without explanatory deposit error text, so that the setup wizard is compact and easy to scan.
3. As a testnet operator, I want Kraken excluded from the list of testnet-capable exchanges, so that the engine never attempts to route testnet orders to incomplete demo endpoints.
4. As a testnet operator, I want the settings sidebar to automatically hide Kraken when the active trading environment is Testnet, so that only fully supported testnet venues appear in the navigation.
5. As a live arbitrage trader, I want Kraken and Kraken Futures to remain fully accessible and routable in Live mode, so that live market data, balance tracking, and execution remain intact.
6. As a security-conscious operator, I want credential synchronization to only provision live adapter instances for Kraken, ensuring no testnet credentials or state are tracked.
7. As an automated strategy bot running in testnet, I want opportunity scanners to exclude Kraken from testnet route discovery, avoiding unfulfillable multi-leg route matching.
8. As a developer auditing venue capabilities, I want documentation and venue capability matrices to explicitly declare Kraken as live-only, matching other venues without public testnet parity.

## Implementation Decisions

- **PopDEX Modal UI Simplification**:
  - Removed network-level faucet link and callout banner next to the network switcher in the setup modal.
  - Removed deposit error explanation paragraph under the main account address input.
  - Purged unused external application and faucet URL constants from the accounts component module.
- **Engine Environment Resolution & Adapter Decoupling**:
  - Removed the testnet Kraken adapter field from the central engine state structure, ensuring no testnet adapter instance is instantiated, held, or cleared in memory.
  - Omitted Kraken from testnet venue admission in the environment-scoped venue resolution pipeline, ensuring `venues_for_env` never returns Kraken under Testnet mode.
  - Maintained complete live Kraken adapter initialization, credential management, and admission under Live mode.
- **Frontend Capability Registry Alignment**:
  - Removed Kraken venue identifiers and exchange codes from the testnet-capable venue sets in frontend type definitions.
  - Updated connected testnet venue code admission logic to reject Kraken codes in Testnet mode.
  - Preserved live multi-market configuration for Kraken Spot and Futures in settings and accounts views.
- **Documentation Alignment**:
  - Updated capabilities matrices and testnet sweep procedures to document Kraken as live-only without testnet simulation.

## Testing Decisions

- **Testing Principles**:
  - Test only observable external behavior: verify that the engine resolution returns empty testnet listings for Kraken, live listings when enabled, and that UI capability helper filters exclude Kraken in testnet mode.
  - Avoid coupling tests to internal state fields.
- **Tested Modules**:
  - Backend engine state environment resolution (`venues_for_env`).
  - Frontend venue capability filters (`venueHasTestnet`, `getEnabledConnectedVenueCodes`).
  - Settings exchange sidebar filtering in Testnet vs Live environments.
- **Prior Art**:
  - Pattern matches existing live-only venue tests for Coinbase, Aster, and Variational.

## Out of Scope

- Modifying live Kraken spot or futures execution logic, rate limiting, or order serialization.
- Modifying other testnet-capable venue adapters (Hyperliquid, Lighter, Binance, OKX, Alpaca, etc.).
- Changing PopDEX cryptographic agent key generation or transaction approval signing mechanics.

## Further Notes

- Kraken continues to be fully supported in Live mode across both Spot (`api.kraken.com`) and Futures (`futures.kraken.com`).
