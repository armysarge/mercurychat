// Unit tests for src/quick.ts (M3c Task 12, B8): plain node (`node --test`), TypeScript by type stripping.
// Run from the repo root: node --test apps/desktop/scripts/quick.test.mjs
import assert from "node:assert/strict";
import { test } from "node:test";
import { PICK_ACTION, quickFirst } from "../src/quick.ts";

const m = (id, quick) => ({ id, name: id, body: `${id} text`, quick });

test("quick_macros_listed_first", () => {
  const list = [m("a", false), m("b", true), m("c", false), m("d", true)];
  const { quick, others } = quickFirst(list);
  assert.deepEqual(quick.map((x) => x.id), ["b", "d"], "the quick ones, in their own order");
  assert.deepEqual(others.map((x) => x.id), ["a", "c"], "then the rest, in theirs");
  assert.deepEqual(quickFirst([]), { quick: [], others: [] });
  assert.deepEqual(list.map((x) => x.id), ["a", "b", "c", "d"], "the list itself is untouched");
});

test("click_inserts_never_sends", () => {
  assert.equal(PICK_ACTION, "insert", "one click puts the text in the draft; Enter sends it");
});
