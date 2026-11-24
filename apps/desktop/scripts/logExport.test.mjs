// Unit tests for src/logExport.ts (owner's Log4OM import, 24 Nov): the
// warning before an ADIF export counts the QSOs with neither band nor
// frequency (loggers refuse them), and the ADIF mode is checked as the core does.
// Run from the repo root: node --test apps/desktop/scripts/logExport.test.mjs
import assert from "node:assert/strict";
import { test } from "node:test";
import { adifModeOk, withoutBand } from "../src/logExport.ts";

const q = (state, band, freq) => ({ state, band, freq: freq === null ? null : { value: freq } });

test("counts_qsos_with_neither_band_nor_frequency", () => {
  const qsos = [q("final", "20m", 14074000), q("draft", null, null), q("final", null, null), q("final", "40m", null), q("final", null, 7100000)];
  assert.equal(withoutBand(qsos, false), 2);
  assert.equal(withoutBand(qsos, true), 1, "only the finalised ones when finalised only");
});

test("adif_mode_is_checked_like_the_core", () => {
  for (const ok of ["DYNAMIC", " mercury ", "DATA-1"]) assert.ok(adifModeOk(ok), ok);
  for (const bad of ["", "  ", "A B", "<EOR>", "MODE:7", "ÉTÉ", "ABCDEFGHIJKLMNOPQRSTU"]) assert.ok(!adifModeOk(bad), bad);
});
