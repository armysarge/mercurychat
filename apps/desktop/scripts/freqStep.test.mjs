// Unit tests for src/components/freqStep.ts (M3b Task 10): plain node (`node --test`), TypeScript by type stripping.
// Run from the repo root: node --test apps/desktop/scripts/freqStep.test.mjs
import assert from "node:assert/strict";
import { test } from "node:test";
import { FREQ_MAX_HZ, formatDigits, parseFreqInput, stepDigit } from "../src/components/freqStep.ts";

test("step_digit_moves_one_place", () => {
  assert.equal(stepDigit(7_105_000, 3, 1), 7_106_000);
  assert.equal(stepDigit(7_105_000, 3, -1), 7_104_000);
  assert.equal(stepDigit(7_105_000, 0, 1), 7_105_001);
  assert.equal(stepDigit(7_105_000, 7, 1), 17_105_000);
  assert.equal(stepDigit(7_105_000, 6, -1), 6_105_000);
});

test("step_digit_clamps_and_never_wraps", () => {
  assert.equal(stepDigit(0, 0, -1), 0);
  assert.equal(stepDigit(500, 3, -1), 0);
  assert.equal(stepDigit(53_999_999, 6, 1), 54_000_000);
  assert.equal(FREQ_MAX_HZ, 54_000_000);
});

test("parse_freq_input_reads_mhz_khz_and_hz", () => {
  for (const [text, hz] of [
    ["7.105", 7_105_000],
    ["7105", 7_105_000],
    ["7105000", 7_105_000],
    ["7,105.000", 7_105_000],
    ["7.105.000", 7_105_000],
    [" 14.0905 ", 14_090_500],
    ["7,105", 7_105_000],
    ["3573.5", 3_573_500],
  ]) {
    assert.equal(parseFreqInput(text), hz, text);
  }
});

test("parse_freq_input_rejects_nonsense", () => {
  for (const text of ["", "   ", "abc", "-7", "7..1", "0", "7.1x", "99999999999"]) {
    assert.equal(parseFreqInput(text), null, JSON.stringify(text));
  }
});

test("format_digits", () => {
  assert.equal(formatDigits(7_105_000), "7.105.000");
  assert.equal(formatDigits(14_074_500.4), "14.074.500");
  assert.equal(formatDigits(null), "--.---.---");
  assert.equal(formatDigits(undefined), "--.---.---");
});

test("band_for_matches_the_core", async () => {
  const { bandFor } = await import("../src/components/freqStep.ts");
  assert.equal(bandFor(7_105_000), "40m");
  assert.equal(bandFor(14_000_000), "20m");
  assert.equal(bandFor(54_000_000), "6m");
  assert.equal(bandFor(6_000_000), null);
  assert.equal(bandFor(null), null);
});
