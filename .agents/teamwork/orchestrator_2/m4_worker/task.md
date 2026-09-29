# Task: Milestone M4 — Strategy Panels Integration & Options Ribbon Reorganization

Implement:
1. `OpportunityRadar.tsx`:
   - R3: Lock bottom dock container to `className="p-3.5 h-96 overflow-y-auto"`, remove redundant inner `max-h-96`.
   - R4: Standardized status badge: `<BotPhaseBadge phase={botPhase} isPaused={isPaused} />` immediately right of `Strategy Status`, before parameter tags.
   - R1: In `handleFlatten`, also set `handleBotMode("off")` so bot is stopped, not left running/paused. Register with `registerBotResetListener` from `useBotLifecycle` to reset `botCfg` mode to `"off"` on global reset.
2. `OptionsArbitrage.tsx`:
   - R5: Remove "Premium Outlay" parameter tag from the ribbon.
   - R5: Add 5th metric card for "Premium Outlay" in metrics grid alongside existing 4 cards (`grid-cols-2 sm:grid-cols-3 lg:grid-cols-5`). Displays formatted USD or "—" when null.
   - R5: Reorganize ribbon to single header row (`flex items-center justify-between gap-3 flex-nowrap`). Configurator, Scan, and BotLifecycleControls sit on same single header row as strategy title and status. Prune secondary internal loop parameters (`Poll`, `Max Open`) from ribbon.
   - R4: Display `<BotPhaseBadge phase={botPhase} isPaused={isPaused} />` immediately right of strategy title header, before parameter tags. Remove "Engine Paused" badge.
   - R1: In `handleFlatten`, also set `handleBotMode("off")`. Register with `registerBotResetListener`.
3. `TriangularArbitrage.tsx`:
   - R4: Display `<BotPhaseBadge phase={bot.phase} isPaused={isPaused} />` immediately right of `Strategy Status`. Remove trailing `{isPaused && <span className="badge-status-warn">Engine Paused</span>}`.
4. `PredictionHub.tsx`:
   - R4: Display `<BotPhaseBadge phase={bot.phase} isPaused={isPaused} />` immediately right of `Strategy Status`, before parameter tags.
5. Tests:
   - Add/update tests in `OpportunityRadar.test.ts`, `OptionsArbitrage.test.ts`, `TriangularArbitrage.test.ts`.
   - Verify: `cd frontend && npm test`.
