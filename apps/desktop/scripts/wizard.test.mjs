// Unit tests for src/wizard.ts (M5 Task 9): the first-run wizard's step
// order, and Back, Next and Skip on the built-in and "My own Mercury" paths.
// Run from the repo root: node --test apps/desktop/scripts/wizard.test.mjs
import assert from "node:assert/strict";
import { test } from "node:test";
import { back, canSkip, next, skip, stepsFor, STEPS } from "../src/wizard.ts";

const state = (step, skipped = []) => ({ step, draftProfileId: null, done: false, skipped, station: null });

test("step_order", () => {
  assert.deepEqual(STEPS, ["welcome", "station", "modem", "audio", "rig", "operate", "tour", "done"]);
  assert.deepEqual(stepsFor("builtIn"), STEPS);
  assert.deepEqual(stepsFor(null), STEPS, "before the modem is chosen, every step is ahead");
});

test("my_own_mercury_skips_audio_and_rig", () => {
  // Its sound card and rig are set where that Mercury runs.
  assert.deepEqual(stepsFor("existing"), ["welcome", "station", "modem", "operate", "tour", "done"]);
  assert.equal(next("modem", "existing"), "operate");
  assert.equal(back("operate", "existing"), "modem");
  // A resumed state on a step the path does not have moves on to the next it has.
  assert.equal(next("audio", "existing"), "operate");
});

test("back_and_next_on_the_built_in_path", () => {
  const path = "builtIn";
  const walked = ["welcome"];
  while (walked.at(-1) !== "done") walked.push(next(walked.at(-1), path));
  assert.deepEqual(walked, STEPS);
  assert.equal(back("audio", path), "modem");
  assert.equal(back("welcome", path), "welcome", "nothing before the start");
  assert.equal(next("done", path), "done", "nothing after the end");
});

test("skip_moves_on_and_lists_the_step_once", () => {
  let s = skip(state("audio"), "builtIn");
  assert.equal(s.step, "rig");
  assert.deepEqual(s.skipped, ["audio"]);
  s = skip(state("audio", ["audio"]), "builtIn");
  assert.deepEqual(s.skipped, ["audio"], "once each");
});

test("welcome_modem_and_done_are_not_skipped", () => {
  // A station needs a modem; Welcome and Done have nothing to skip.
  for (const step of ["welcome", "modem", "done"]) {
    assert.equal(canSkip(step), false, step);
    assert.deepEqual(skip(state(step), "builtIn"), state(step), step);
  }
  for (const step of ["station", "audio", "rig", "operate", "tour"]) assert.equal(canSkip(step), true, step);
});
