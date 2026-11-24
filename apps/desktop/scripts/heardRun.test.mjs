// Unit tests for src/heardRun.ts (owner, 24 Nov): the Heard list starts empty
// at every app start; what was heard before stays on record for the rest of
// the app ("last heard ..."), and favourites stay under their own filter.
// Run from the repo root: node --test apps/desktop/scripts/heardRun.test.mjs
import assert from "node:assert/strict";
import { test } from "node:test";
import { heardThisRun } from "../src/heardRun.ts";

const START = Date.parse("2026-11-24T10:00:00Z");
const st = (callsign, last, favourite = false) => ({ callsign, lastHeardUtc: last, favourite });

test("only_stations_heard_since_the_start_are_listed", () => {
  const list = [st("NEW1", "2026-11-24T10:05:00Z"), st("OLD1", "2026-11-23T22:00:00Z"), st("FAV1", null, true)];
  assert.deepEqual(heardThisRun(list, START, "all").map((s) => s.callsign), ["NEW1"]);
});

test("favourites_keep_their_own_filter", () => {
  const list = [st("OLD1", "2026-11-23T22:00:00Z", true), st("FAV1", null, true), st("NEW1", "2026-11-24T10:05:00Z")];
  assert.deepEqual(heardThisRun(list, START, "favourite").map((s) => s.callsign), ["OLD1", "FAV1"]);
});
