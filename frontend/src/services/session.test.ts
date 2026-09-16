import test from "node:test";
import assert from "node:assert/strict";
import {
  sessionOf,
  isWeekendUtc,
  sessionLabel,
  sessionSegments,
} from "./session.ts";

const ts = (iso: string): number => Date.parse(iso);

test("sessionOf follows the UTC session boundaries", () => {
  assert.equal(sessionOf(ts("2026-01-15T00:00:00Z")), "asia");
  assert.equal(sessionOf(ts("2026-01-15T07:59:59Z")), "asia");
  assert.equal(sessionOf(ts("2026-01-15T08:00:00Z")), "europe");
  assert.equal(sessionOf(ts("2026-01-15T12:59:59Z")), "europe");
  assert.equal(sessionOf(ts("2026-01-15T13:00:00Z")), "overlap");
  assert.equal(sessionOf(ts("2026-01-15T16:59:59Z")), "overlap");
  assert.equal(sessionOf(ts("2026-01-15T17:00:00Z")), "us_late");
  assert.equal(sessionOf(ts("2026-01-15T21:59:59Z")), "us_late");
  assert.equal(sessionOf(ts("2026-01-15T22:00:00Z")), "dead");
  assert.equal(sessionOf(ts("2026-01-15T23:59:59Z")), "dead");
});

test("sessionOf fails closed on unusable timestamps", () => {
  assert.equal(sessionOf(NaN), "dead");
  assert.equal(sessionOf(Infinity), "dead");
});

test("isWeekendUtc matches Saturday and Sunday UTC", () => {
  assert.equal(isWeekendUtc(ts("2026-01-17T12:00:00Z")), true);
  assert.equal(isWeekendUtc(ts("2026-01-18T12:00:00Z")), true);
  assert.equal(isWeekendUtc(ts("2026-01-15T12:00:00Z")), false);
  assert.equal(isWeekendUtc(ts("2026-01-19T12:00:00Z")), false);
  assert.equal(isWeekendUtc(NaN), false);
});

test("sessionLabel renders every session and an em dash for unknown", () => {
  assert.equal(sessionLabel("overlap"), "OVERLAP");
  assert.equal(sessionLabel("us_late"), "US LATE");
  assert.equal(sessionLabel("bogus"), "—");
  assert.equal(sessionLabel(null), "—");
  assert.equal(sessionLabel(undefined), "—");
});

test("sessionSegments covers the window without gaps or overlap", () => {
  const t0 = ts("2026-01-15T12:30:00Z");
  const t1 = ts("2026-01-15T18:30:00Z");
  const segs = sessionSegments(t0, t1);
  assert.equal(segs[0].fromMs, t0);
  assert.equal(segs[segs.length - 1].toMs, t1);
  for (let i = 1; i < segs.length; i++) {
    assert.equal(segs[i].fromMs, segs[i - 1].toMs);
  }
  const sessions = segs.map((s) => s.session);
  assert.ok(sessions.includes("europe"));
  assert.ok(sessions.includes("overlap"));
  assert.ok(sessions.includes("us_late"));
  assert.deepEqual(sessionSegments(NaN, t1), []);
  assert.deepEqual(sessionSegments(t1, t0), []);
});
