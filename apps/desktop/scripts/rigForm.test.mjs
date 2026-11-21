// Unit tests for src/rigForm.ts (M3b.1 Task 8): which boxes Settings → Rig
// shows for a configuration, and which PTT choices fit a Frequency control.
// Run from the repo root: node --test apps/desktop/scripts/rigForm.test.mjs
import assert from "node:assert/strict";
import { test } from "node:test";
import { defaultRigConfig, pttChoices, visibleBoxes } from "../src/rigForm.ts";

const cfg = (freqControl, method = "leaveMercury") => {
  const c = defaultRigConfig();
  c.freqControl = freqControl;
  c.ptt.method = method;
  return c;
};
const boxes = (c) => [...visibleBoxes(c)].sort();

test("visible_boxes_follow_frequency_control", () => {
  assert.deepEqual(boxes(cfg("managedRigctld")), ["cat"]);
  assert.deepEqual(boxes(cfg("externalRigctld")), ["external"]);
  assert.deepEqual(boxes(cfg("flrig")), ["flrig"]);
  assert.deepEqual(boxes(cfg("none")), []);
});

test("visible_boxes_follow_ptt", () => {
  assert.deepEqual(boxes(cfg("managedRigctld", "serialLine")), ["cat", "serialPtt"]);
  assert.deepEqual(boxes(cfg("none", "serialLine")), ["serialPtt"]);
  assert.deepEqual(boxes(cfg("flrig", "cm108")), ["cm108", "flrig"]);
  assert.deepEqual(boxes(cfg("none", "noPtt")), []);
});

test("cat_ptt_needs_a_rigctld", () => {
  assert.ok(!pttChoices("flrig").includes("cat"));
  assert.ok(!pttChoices("none").includes("cat"));
  assert.ok(pttChoices("managedRigctld").includes("cat"));
  assert.ok(pttChoices("externalRigctld").includes("cat"));
  for (const f of ["managedRigctld", "externalRigctld", "flrig", "none"]) {
    assert.ok(pttChoices(f).includes("leaveMercury"), f);
    assert.ok(pttChoices(f).includes("noPtt"), f);
  }
});

test("defaults_match_the_core", () => {
  const d = defaultRigConfig();
  assert.equal(d.freqControl, "none");
  assert.equal(d.ptt.method, "leaveMercury");
  assert.deepEqual([d.external.host, d.external.port], ["127.0.0.1", 4532]);
  assert.deepEqual([d.flrig.host, d.flrig.port], ["127.0.0.1", 12345]);
  assert.deepEqual([d.cat.baud, d.cat.dataBits, d.cat.stopBits, d.cat.waitMs], [9600, 8, 1, 0]);
  assert.deepEqual([d.readEveryS, d.offsetHz, d.ptt.cm108Gpio, d.dataMode], [2, 0, 3, "PKTUSB"]);
});
