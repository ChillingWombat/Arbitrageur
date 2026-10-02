# Orchestrator 3 Context

## Project
Arbitrageur Desktop Application UI/UX Standardization and Polish (R1-R8).

## Authoritative User Request
See `/mnt/Data/Projects/Arbitrageur/.agents/teamwork/ORIGINAL_REQUEST.md` (Follow-up — 2026-10-01T16:09:57Z).

## Requirements Overview
- **R1**: Prediction Market Ribbon & Input Cleanups (`PredictionHub.tsx`)
- **R2**: Strategy Monitor Header Alignment (`OpportunityRadar.tsx`, `SpotArbitrageBot.tsx`)
- **R3**: Button Set Outer Border Symmetric Padding (`BotLifecycleControls.tsx` and all strategy lifecycle button groups)
- **R4**: Dislocation Matrix & Signal Trend Gadget Cleanups (`LiveMatrix.tsx`, `SignalTrendGadget.tsx`)
- **R5**: Funding Rate Opportunity Scanner Dimensions (`FundingRadar.tsx`)
- **R6**: Options Section Header Ribbon (`OptionsArbitrage.tsx`)
- **R7**: Security Page Explanations & MFA Wording (`EngineSettings.tsx`)
- **R8**: Ticker Explorer Symbol Quote Display (`TickerExplorer.tsx`)

## Verification Targets
- `cd frontend && npm test`
- `npx tsc --noEmit`
