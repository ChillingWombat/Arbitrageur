import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { TriangularArbitrage } from "./TriangularArbitrage.tsx";

test("TriangularArbitrage renders standardized status badge immediately right of Strategy Status", () => {
  const html = renderToStaticMarkup(
    React.createElement(TriangularArbitrage, {
      executionLogs: [],
      isPaused: false,
      hasApiConnectivity: true,
      onRefreshAll: async () => {},
    })
  );
  assert.match(
    html,
    /Strategy Status<\/span>[\s\S]*?title="Strategy bot status: Stopped"[\s\S]*?>Stopped<\/span>/
  );
  assert.doesNotMatch(html, /Engine/i);
});

test("TriangularArbitrage omits trailing Engine Paused and respects isPaused", () => {
  const htmlPaused = renderToStaticMarkup(
    React.createElement(TriangularArbitrage, {
      executionLogs: [],
      isPaused: true,
      hasApiConnectivity: true,
      onRefreshAll: async () => {},
    })
  );
  assert.match(htmlPaused, /Strategy Status/);
  assert.doesNotMatch(htmlPaused, /Engine Paused/);
  assert.doesNotMatch(htmlPaused, /badge-status-warn/);
  assert.doesNotMatch(htmlPaused, /Engine/i);
});
