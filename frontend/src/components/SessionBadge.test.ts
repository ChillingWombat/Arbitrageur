import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { SessionBadge } from "./SessionBadge.tsx";

test("badge renders the session label and the allow multiplier", () => {
  const html = renderToStaticMarkup(
    React.createElement(SessionBadge, { sessionId: "overlap", allow: 1, sizeMult: 1 }),
  );
  assert.match(html, /OVERLAP/);
  assert.match(html, /x1\.0/);
  assert.match(html, /bg-profit-dim/);
});

test("provisional badge flags neutral multipliers and renders the dot", () => {
  const html = renderToStaticMarkup(
    React.createElement(SessionBadge, { sessionId: "asia", provisional: true, allow: 0.5 }),
  );
  assert.match(html, /ASIA/);
  assert.match(html, /provisional/);
  assert.match(html, /bg-warn-dim/);
});

test("funding mask allow of zero renders the blocked style", () => {
  const html = renderToStaticMarkup(
    React.createElement(SessionBadge, { sessionId: "dead", allow: 0 }),
  );
  assert.match(html, /DEAD/);
  assert.match(html, /x0\.0/);
  assert.match(html, /bg-loss-dim/);
});

test("unknown session renders an em dash, never a fabricated label", () => {
  const html = renderToStaticMarkup(React.createElement(SessionBadge, { sessionId: null }));
  assert.match(html, /—/);
  assert.match(html, /session unknown/);
  const undefinedHtml = renderToStaticMarkup(
    React.createElement(SessionBadge, { sessionId: undefined, allow: NaN }),
  );
  assert.match(undefinedHtml, /—/);
  assert.match(undefinedHtml, /bg-surface-raised/);
});
