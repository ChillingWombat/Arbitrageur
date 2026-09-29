import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { OpportunityRadar } from "./OpportunityRadar.tsx";

test("OpportunityRadar: bottom dock container enforces fixed height across tabs without inner max-h-96", () => {
  const html = renderToStaticMarkup(
    React.createElement(OpportunityRadar, {
      executionLogs: [],
      isPaused: false,
    })
  );
  assert.match(html, /class="p-3\.5 h-96 overflow-y-auto"/);
  // Redundant inner wrappers removed
  assert.doesNotMatch(html, /<div class="max-h-96 overflow-y-auto">/);
});

test("OpportunityRadar renders standardized status badge immediately right of Strategy Status", () => {
  const html = renderToStaticMarkup(
    React.createElement(OpportunityRadar, {
      executionLogs: [],
      isPaused: false,
    })
  );
  assert.match(
    html,
    /Strategy Status<\/span>[\s\S]*?title="Strategy bot status: Stopped"[\s\S]*?>Stopped<\/span>/
  );
  const statusBadge = html.match(/title="Strategy bot status: [^"]*"[^>]*>[\s\S]*?<\/span>/);
  assert.ok(statusBadge);
  assert.doesNotMatch(statusBadge[0], /Engine/i);
});

test("OpportunityRadar status badge handles isPaused and never renders Engine Paused", () => {
  const htmlPaused = renderToStaticMarkup(
    React.createElement(OpportunityRadar, {
      executionLogs: [],
      isPaused: true,
    })
  );
  assert.match(htmlPaused, /Strategy Status/);
  assert.match(htmlPaused, /title="Strategy bot status: Stopped"/);
  const statusBadge = htmlPaused.match(/title="Strategy bot status: [^"]*"[^>]*>[\s\S]*?<\/span>/);
  assert.ok(statusBadge);
  assert.doesNotMatch(statusBadge[0], /Engine/i);
  assert.doesNotMatch(htmlPaused, /Engine Paused/);
  assert.doesNotMatch(htmlPaused, /badge-status-warn/);
});
