import { test } from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { PriceRetriverPanel } from "./PriceRetriverPanel.tsx";

test("PriceRetriverPanel renders connection header and inputs", () => {
  const html = renderToStaticMarkup(React.createElement(PriceRetriverPanel));

  assert.match(html, /PriceRetriver 24\/7 VPS Data Ingestion Node/);
  assert.match(html, /Node REST &amp; WS URL/);
  assert.match(html, /API Authentication Token/);
  assert.match(html, /Connect/);
  assert.match(html, /DISCONNECTED/);
  assert.match(html, /VPS Setup Guide/);
});

test("PriceRetriverPanel renders node URL input with default or placeholder", () => {
  const html = renderToStaticMarkup(React.createElement(PriceRetriverPanel));

  assert.match(html, /placeholder="http:\/\/198\.51\.100\.24:8888 or http:\/\/localhost:8888"/);
  assert.match(html, /placeholder="Bearer secret token\.\.\."/);
});
