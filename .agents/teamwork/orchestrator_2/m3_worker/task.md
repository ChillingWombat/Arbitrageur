# Task: Milestone M3 — Standardized Status Badge & Dock Heights

Implement Requirement R4 (Standardized Status Badge component) & Requirement R3 (Fixed Dock Heights for FundingRadar and LiveMatrix):
- `frontend/src/components/BotLifecycleControls.tsx`:
  - Implement `StrategyBotStatus = "Active" | "Paused" | "Reducing" | "Stopped"`.
  - Implement `resolveBotStatus(phase?: BotPhase, isPaused?: boolean): StrategyBotStatus`.
  - Update `BotPhaseBadge` (and alias `StrategyStatusBadge`) to render exactly one of the 4 statuses:
    - Active: `bg-profit-dim text-profit border-profit-border` with emerald dot.
    - Paused: `bg-warn-dim text-warn-strong border-warn-border` with amber dot.
    - Reducing: `bg-warn-dim text-warn-strong border-warn-border` with pulsing amber dot.
    - Stopped: `bg-surface-raised text-slate-500 border-surface-border` with slate dot.
  - Remove "Engine" from any status label or tooltip.
  - Update `BotLifecycleControls.test.ts`.
- `frontend/src/components/FundingRadar.tsx`:
  - Lock dock tabs container to `className="p-3.5 h-96 overflow-y-auto"`.
  - Update strategy header to display `<BotPhaseBadge phase={bot.phase} isPaused={isPaused} />` immediately right of `Strategy Status`, before parameter tags.
- `frontend/src/components/LiveMatrix.tsx`:
  - Lock dock tabs container to `className="p-3.5 h-96 overflow-y-auto"` and remove redundant inner `max-h-96` wrappers.
  - Update strategy header to display `<BotPhaseBadge phase={bot.phase} isPaused={isPaused} />` immediately right of `Strategy Status`, before parameter tags.
- Verify tests: `cd frontend && npm test`.
