// The GitHub workflows get read access only, and run actions at exact commits:
// a tag can be moved to other code, a commit cannot.
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { test } from "node:test";

const dir = new URL("../../../.github/workflows/", import.meta.url);
const workflows = readdirSync(dir)
  .filter((f) => /\.ya?ml$/.test(f))
  .map((f) => [f, readFileSync(new URL(f, dir), "utf8").replace(/\r/g, "")]);

test("every workflow limits its token to reading the repository", () => {
  assert.ok(workflows.length > 0);
  for (const [name, text] of workflows) {
    const block = text.match(/^permissions:\n((?:[ \t]+.*\n)+)/m);
    assert.ok(block, `${name}: no top-level permissions block`);
    assert.equal(block[1].trim(), "contents: read", name);
    assert.equal(text.match(/^\s*permissions\s*:/gm).length, 1, `${name}: a job overrides the permissions`);
  }
});

test("every action is pinned to a full commit, with its version noted", () => {
  let seen = 0;
  for (const [name, text] of workflows) {
    for (const line of text.split("\n").filter((l) => /^\s*(-\s+)?uses:/.test(l))) {
      seen += 1;
      assert.match(line, /uses:\s+[\w.-]+\/[\w./-]+@[0-9a-f]{40}\s+# v\d+\.\d+\.\d+$/, `${name}: ${line.trim()}`);
    }
  }
  assert.ok(seen >= 5, `only ${seen} actions found`);
});
