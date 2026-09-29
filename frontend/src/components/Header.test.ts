import nodeTest from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { Header } from "./Header.tsx";
import type { SystemStatusDto } from "../types";

const INTERACTIVE_CHILD = process.argv.includes("--interactive-child");
const test = INTERACTIVE_CHILD ? (((() => {}) as unknown) as typeof nodeTest) : nodeTest;

const mockStatus = (over: Partial<SystemStatusDto> = {}): SystemStatusDto => ({
  is_live_mode: false,
  environment: "testnet",
  is_paused: false,
  total_realized_pnl: 0,
  total_trades_count: 0,
  successful_trades_count: 0,
  circuit_breaker_status: "OK",
  circuit_breaker_tripped: false,
  current_drawdown_usd: 0,
  max_drawdown_usd: 1000,
  consecutive_failures: 0,
  active_venues: [],
  ...over,
});

test("Header pause/resume button is disabled with explanatory tooltip when circuit breaker is tripped", () => {
  const html = renderToStaticMarkup(
    React.createElement(Header, {
      status: mockStatus({ circuit_breaker_tripped: true }),
      isPaused: true,
      onTogglePause: () => {},
      onEmergencyKillswitch: () => {},
      refreshing: false,
      activeTab: "radar",
      onSelectTab: () => {},
    })
  );

  const trippedTooltip = "Trading halted by circuit breaker — reset in Portfolio Monitor before resuming engine";
  assert.ok(html.includes(trippedTooltip), "wrapper must contain tripped tooltip message");
  assert.match(html, /disabled=""/, "pause/resume button must be disabled when circuit breaker is tripped");
  assert.match(html, /cursor-not-allowed/, "button carries cursor-not-allowed styling");
  assert.match(html, /pointer-events-none/, "button has pointer-events-none so hover falls through to wrapper tooltip");
});

test("Header pause/resume button displays Resume tooltip and Resume text when engine is paused", () => {
  const html = renderToStaticMarkup(
    React.createElement(Header, {
      status: mockStatus({ circuit_breaker_tripped: false, is_paused: true }),
      isPaused: true,
      onTogglePause: () => {},
      onEmergencyKillswitch: () => {},
      refreshing: false,
      activeTab: "radar",
      onSelectTab: () => {},
    })
  );

  const resumeTooltip = "Resume Engine: Resume execution coordination. Automated bots remain stopped and must be started individually on each strategy page.";
  assert.ok(html.includes(resumeTooltip), "wrapper must contain resume tooltip message");
  assert.match(html, /Resume Engine/, "button text must be Resume Engine");
  assert.doesNotMatch(html, /disabled=""/, "button must not be disabled when engine is paused and breaker is clean");
});

test("Header pause/resume button displays Pause tooltip and Pause text when engine is running", () => {
  const html = renderToStaticMarkup(
    React.createElement(Header, {
      status: mockStatus({ circuit_breaker_tripped: false, is_paused: false }),
      isPaused: false,
      onTogglePause: () => {},
      onEmergencyKillswitch: () => {},
      refreshing: false,
      activeTab: "radar",
      onSelectTab: () => {},
    })
  );

  const pauseTooltip = "Pause Engine: Temporarily suspend executions. Open positions are preserved.";
  assert.ok(html.includes(pauseTooltip), "wrapper must contain pause tooltip message");
  assert.match(html, /Pause Engine/, "button text must be Pause Engine");
  assert.doesNotMatch(html, /disabled=""/, "button must not be disabled when engine is running and breaker is clean");
});

test("Header pause/resume button interactive toggling (interactive DOM)", async (t) => {
  await t.test("click toggles pause without starting bots", () => {
    const out = execFileSync(
      process.execPath,
      [__filename, "--interactive-child"],
      { encoding: "utf8", timeout: 30000 },
    );
    assert.match(out, /INTERACTIVE-OK/);
  });
});

// ── child-process harness (not executed under node:test) ────────────────────
declare const __filename: string;

async function runInteractiveChild() {
  const { Window } = await import("happy-dom");
  const { createRoot } = await import("react-dom/client");
  const { act } = await import("react");

  const window = new Window();
  const document = window.document;
  (globalThis as unknown as { window: unknown }).window = window;
  (globalThis as unknown as { document: unknown }).document = document;

  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container as unknown as HTMLElement);

  let toggledState: boolean | null = null;
  const onTogglePause = (paused: boolean) => {
    toggledState = paused;
  };

  // 1. Render in running state: click pauses engine
  await act(async () => {
    root.render(
      React.createElement(Header, {
        status: mockStatus({ circuit_breaker_tripped: false, is_paused: false }),
        isPaused: false,
        onTogglePause,
        onEmergencyKillswitch: () => {},
        refreshing: false,
        activeTab: "radar",
        onSelectTab: () => {},
      }),
    );
  });

  const pauseBtn = Array.from(container.querySelectorAll("button")).find((b) =>
    ((b as unknown as HTMLElement).textContent ?? "").includes("Pause Engine"),
  );
  if (!pauseBtn) throw new Error("Pause Engine button missing");
  await act(async () => {
    pauseBtn.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true }));
  });
  if (toggledState !== true) {
    throw new Error(`expected onTogglePause(true) when pausing, got ${String(toggledState)}`);
  }

  // 2. Render in paused state: click resumes engine (toggling pause to false, no bots started)
  toggledState = null;
  await act(async () => {
    root.render(
      React.createElement(Header, {
        status: mockStatus({ circuit_breaker_tripped: false, is_paused: true }),
        isPaused: true,
        onTogglePause,
        onEmergencyKillswitch: () => {},
        refreshing: false,
        activeTab: "radar",
        onSelectTab: () => {},
      }),
    );
  });

  const resumeBtn = Array.from(container.querySelectorAll("button")).find((b) =>
    ((b as unknown as HTMLElement).textContent ?? "").includes("Resume Engine"),
  );
  if (!resumeBtn) throw new Error("Resume Engine button missing");
  await act(async () => {
    resumeBtn.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true }));
  });
  if (toggledState !== false) {
    throw new Error(`expected onTogglePause(false) when resuming, got ${String(toggledState)}`);
  }

  // 3. Render when circuit breaker is tripped: button is disabled
  toggledState = null;
  await act(async () => {
    root.render(
      React.createElement(Header, {
        status: mockStatus({ circuit_breaker_tripped: true }),
        isPaused: true,
        onTogglePause,
        onEmergencyKillswitch: () => {},
        refreshing: false,
        activeTab: "radar",
        onSelectTab: () => {},
      }),
    );
  });

  const disabledBtn = container.querySelector("button[disabled]");
  if (!disabledBtn) throw new Error("button should have disabled attribute when tripped");

  await act(async () => root.unmount());
  process.stdout.write("INTERACTIVE-OK\n");
  process.exit(0);
}

if (INTERACTIVE_CHILD) {
  runInteractiveChild().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}
