// Unit tests for src/broadcastList.ts: the latest general broadcasts on
// Operate (owner, 26 Nov). Run from the repo root:
// node --test apps/desktop/scripts/broadcastList.test.mjs
import assert from "node:assert/strict";
import { test } from "node:test";
import { LATEST_MAX, longUtc, shortUtc, snrText, upsertLatest } from "../src/broadcastList.ts";

const at = (minute, second = "00") => `2026-11-26T14:${String(minute).padStart(2, "0")}:${second}.000Z`;
const b = (id, atUtc, snr = null) => ({ id, callsign: "ZS6ABC", message: `m${id}`, atUtc, snr, mode: snr ? "DATAC3" : null });
const reading = (value) => ({ value, source: "modem monitor", atUtc: at(0), ageMs: null });
const ids = (list) => list.map((x) => x.id);

test("a_second_report_of_a_broadcast_replaces_its_row", () => {
  const list = upsertLatest(upsertLatest([], b(1, at(1))), b(1, at(1), reading(7.5)));
  assert.deepEqual(ids(list), [1]);
  assert.equal(list[0].snr.value, 7.5);
  assert.equal(list[0].mode, "DATAC3");
});

test("an_older_copy_does_not_take_the_snr_away", () => {
  // The list re-read at the same moment may answer after the S/N event.
  const list = upsertLatest([b(1, at(1), reading(7.5))], b(1, at(1)));
  assert.equal(list[0].snr.value, 7.5);
  assert.equal(list[0].mode, "DATAC3");
});

test("the_eleventh_drops_the_oldest", () => {
  let list = [];
  for (let n = 1; n <= LATEST_MAX + 1; n++) list = upsertLatest(list, b(n, at(n)));
  assert.deepEqual(ids(list), [11, 10, 9, 8, 7, 6, 5, 4, 3, 2]);
});

test("rows_sort_newest_first_whatever_order_they_arrive_in", () => {
  let list = [];
  for (const row of [b(2, at(2)), b(4, at(5)), b(1, at(1)), b(3, at(2))]) list = upsertLatest(list, row);
  assert.deepEqual(ids(list), [4, 3, 2, 1], "the same instant: the later id first");
});

test("the_list_given_is_not_changed", () => {
  const before = [b(1, at(1))];
  upsertLatest(before, b(2, at(2)));
  assert.deepEqual(ids(before), [1]);
});

test("snr_is_signed_and_a_dash_when_unknown", () => {
  assert.equal(snrText(reading(7.5)), "+7.5");
  assert.equal(snrText(reading(-2)), "−2.0");
  assert.equal(snrText(reading(0.5)), "+0.5");
  assert.equal(snrText(null), "—");
});

test("dates_are_utc_with_the_month_in_the_ui_language", () => {
  assert.equal(shortUtc("2026-11-26T14:32:08.000Z", "en"), "26 Nov 14:32");
  assert.equal(longUtc("2026-11-26T14:32:08.000Z", "en"), "26 Nov 2026, 14:32:08 UTC");
  assert.equal(shortUtc("2026-01-05T00:07:00.000Z", "fr"), "5 janv. 00:07");
});

test("a_time_that_cannot_be_read_is_a_dash", () => {
  assert.equal(shortUtc("", "en"), "—");
  assert.equal(longUtc("not a time", "en"), "—");
});
