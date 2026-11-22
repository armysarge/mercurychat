// Unit tests for src/transfers.ts (M4 Task 11): plain node (`node --test`), TypeScript by type stripping.
// Run from the repo root: node --test apps/desktop/scripts/transfers.test.mjs
import assert from "node:assert/strict";
import { test } from "node:test";
import { airtime, applyTransfer, emptyTransfers, isFinal, lists, loadTransfers } from "../src/transfers.ts";

const v = (id, over = {}) => ({
  id,
  direction: "out",
  peer: "SIM1",
  originalName: `${id}.txt`,
  savedName: null,
  size: 1000,
  wireSize: 600,
  state: "sending",
  bytesDone: 0,
  etaS: null,
  reason: null,
  offerExpiresS: null,
  createdUtc: "2026-11-22T10:00:00Z",
  ...over,
});

test("events_update_by_id", () => {
  const s = emptyTransfers();
  applyTransfer(s, v("a", { state: "waiting" }));
  applyTransfer(s, v("b", { direction: "in", state: "offered" }));
  applyTransfer(s, v("a", { state: "sending", bytesDone: 100 }));
  assert.equal(Object.keys(s.byId).length, 2);
  assert.equal(s.byId.a.state, "sending");
  assert.equal(s.byId.a.bytesDone, 100);
  assert.equal(s.byId.b.state, "offered");
});

test("progress_never_goes_backwards", () => {
  const s = emptyTransfers();
  applyTransfer(s, v("a", { bytesDone: 400 }));
  applyTransfer(s, v("a", { bytesDone: 200, etaS: 9 }));
  assert.equal(s.byId.a.bytesDone, 400, "a late, smaller progress report is not shown");
  assert.equal(s.byId.a.etaS, 9, "the rest of the update still applies");
});

test("final_states_are_final", () => {
  const s = emptyTransfers();
  applyTransfer(s, v("a", { state: "complete", bytesDone: 600 }));
  assert.equal(applyTransfer(s, v("a", { state: "sending", bytesDone: 300 })), false);
  assert.equal(s.byId.a.state, "complete");
  for (const st of ["complete", "rejected", "cancelled", "failed", "interrupted"]) assert.ok(isFinal(st), st);
  for (const st of ["offered", "waiting", "sending", "receiving", "verifying"]) assert.ok(!isFinal(st), st);
});

test("lists_split_newest_first", () => {
  const s = emptyTransfers();
  loadTransfers(s, [
    v("new-in", { direction: "in", state: "complete" }),
    v("out", { state: "sending" }),
    v("old-fail", { state: "failed" }),
    v("old-in", { direction: "in", state: "offered" }),
  ]);
  applyTransfer(s, v("newest", { state: "waiting" }));
  const l = lists(s);
  assert.deepEqual(l.incoming.map((x) => x.id), ["new-in", "old-in"]);
  assert.deepEqual(l.outgoing.map((x) => x.id), ["newest", "out"]);
  assert.deepEqual(l.ended.map((x) => x.id), ["old-fail"], "interrupted and failed in a list of their own");
});

test("airtime_in_words", () => {
  assert.deepEqual(airtime(42), { key: "files.airtime.seconds", n: 42 });
  assert.deepEqual(airtime(60), { key: "files.airtime.minutes", n: 1 });
  assert.deepEqual(airtime(719), { key: "files.airtime.minutes", n: 12 });
  assert.deepEqual(airtime(7200), { key: "files.airtime.hours", n: 2 });
});

test("helpers", async () => {
  const { formatSize, isPhotoName, reasonKey, offerAirtimeS } = await import("../src/transfers.ts");
  assert.equal(formatSize(812), "812 B");
  assert.equal(formatSize(4800), "4.7 KB");
  assert.equal(formatSize(1048576), "1.0 MB");
  assert.ok(isPhotoName("IMG_001.JPG") && isPhotoName("scan.png") && isPhotoName("a.webp") && !isPhotoName("notes.txt"));
  assert.equal(reasonKey("too large"), "files.reason.tooLarge");
  assert.equal(reasonKey("declined"), "files.reason.declined");
  assert.equal(reasonKey("hash_mismatch: declared ab got cd"), null, "an engine's own words are shown as they are");
  assert.equal(offerAirtimeS(1900, "BW500"), 110, "19 B/s nominal plus framing");
  assert.equal(offerAirtimeS(1150, "BW2300"), 11);
});
