// Unit tests for src/themes/apply.ts: plain node (`node --test`), TypeScript by type stripping.
// Run from apps/desktop: node --test scripts/apply.test.mjs
import assert from "node:assert/strict";
import { test } from "node:test";
import { applyDensity, applyFontScale, applyTheme, clampFontScale, exportTheme, isColour, parseThemeImport } from "../src/themes/apply.ts";
import { PRESETS, TOKEN_KEYS, midnight, nightRed, resolvePreset } from "../src/themes/presets.ts";

/** A stand-in for document.documentElement: records what is set on it. */
function fakeRoot() {
  const props = new Map();
  const attrs = new Map();
  return {
    props,
    attrs,
    style: { setProperty: (k, v) => props.set(k, v), removeProperty: (k) => props.delete(k) },
    setAttribute: (k, v) => attrs.set(k, v),
  };
}

test("every token of a theme is set on the root as --name", () => {
  const root = fakeRoot();
  applyTheme(root, nightRed);
  for (const k of TOKEN_KEYS) assert.equal(root.props.get(`--${k}`), nightRed.tokens[k], k);
  assert.equal(root.props.get("color-scheme"), "dark");
});

test("a token that is not a colour never reaches the style", () => {
  const root = fakeRoot();
  applyTheme(root, { scheme: "dark", tokens: { ...midnight.tokens, accent: "red; } body { display:none", text: "url(http://x)", canvas: "#12" } });
  assert.equal(root.props.get("--accent"), midnight.tokens.accent, "falls back to the base value");
  assert.equal(root.props.get("--text"), midnight.tokens.text);
  assert.equal(root.props.get("--canvas"), midnight.tokens.canvas);
  for (const v of root.props.values()) assert.ok(!/[;{}()]/.test(v) || v === "dark", v);
});

test("isColour accepts hex only, and alpha only for overlays", () => {
  assert.ok(isColour("accent", "#abc"));
  assert.ok(isColour("accent", "#AABBCC"));
  assert.ok(!isColour("accent", "#aabbccdd"));
  assert.ok(isColour("scrim", "#000000aa"));
  for (const bad of ["red", "rgb(0,0,0)", "#ggg", "#12345", "url(x)", "#fff;", " #fff", "", 5, null]) assert.ok(!isColour("accent", bad), String(bad));
});

test("density and font scale apply at once, and font scale is clamped to 90-130", () => {
  const root = fakeRoot();
  applyDensity(root, "compact");
  assert.equal(root.attrs.get("data-density"), "compact");
  applyDensity(root, "whatever");
  assert.equal(root.attrs.get("data-density"), "normal");
  assert.equal(clampFontScale(50), 90);
  assert.equal(clampFontScale(500), 130);
  assert.equal(clampFontScale(Number.NaN), 100);
  assert.equal(clampFontScale(112.4), 112);
  applyFontScale(root, 150);
  assert.equal(root.props.get("--font-scale"), "1.3");
});

const doc = (over = {}) => JSON.stringify({ app: "mercurychat-theme", version: 1, name: "Mine", base: "ocean", scheme: "dark", tokens: { accent: "#ff00aa" }, ...over });
const base = (id) => resolvePreset(id, true);

test("export then import round-trips; unknown keys are ignored; missing tokens come from the base preset", () => {
  const custom = { id: "c1", name: "Mine", base: "ocean", scheme: "dark", tokens: { ...base("ocean").tokens, accent: "#ff00aa" } };
  const back = parseThemeImport(exportTheme(custom), base);
  assert.ok(back.ok, JSON.stringify(back));
  assert.deepEqual(back.theme.tokens, custom.tokens);
  const r = parseThemeImport(doc({ tokens: { accent: "#ff00aa", sparkle: "#123456", text: "#ffffff" }, extra: { a: 1 } }), base);
  assert.ok(r.ok);
  assert.equal(r.theme.tokens.accent, "#ff00aa");
  assert.equal(r.theme.tokens.text, "#ffffff");
  assert.equal(r.theme.tokens.canvas, base("ocean").tokens.canvas);
  assert.ok(!("sparkle" in r.theme.tokens));
  assert.deepEqual(Object.keys(r.theme.tokens).sort(), [...TOKEN_KEYS].sort());
});

test("a bad import is rejected with a reason", () => {
  const reason = (text) => {
    const r = parseThemeImport(text, base);
    assert.equal(r.ok, false, text.slice(0, 60));
    return r.reason;
  };
  assert.equal(reason("not json"), "notJson");
  assert.equal(reason("[1,2]"), "notTheme");
  assert.equal(reason(JSON.stringify({ version: 1, tokens: { accent: "#fff" } })), "notTheme");
  assert.equal(reason(doc({ version: 2 })), "version");
  assert.equal(reason(doc({ version: undefined })), "version");
  assert.equal(reason(doc({ tokens: { accent: "javascript:alert(1)" } })), "badColour");
  assert.equal(reason(doc({ tokens: { accent: "#fff; background: url(x)" } })), "badColour");
  assert.equal(reason(doc({ tokens: { accent: "rgb(1,2,3)" } })), "badColour");
  assert.equal(reason(doc({ tokens: "no" })), "notTheme");
  assert.equal(reason(" ".repeat(20000) + doc()), "tooLarge");
  const r = parseThemeImport(doc({ tokens: { accent: "red" } }), base);
  assert.equal(r.detail, "accent");
});

test("an unknown base or scheme falls back, and the name is cleaned", () => {
  const r = parseThemeImport(doc({ base: "nope", scheme: "plaid", name: "  Evil\u0000 name\n" + "x".repeat(80) }), base);
  assert.ok(r.ok);
  assert.equal(r.theme.base, "midnight");
  assert.equal(r.theme.scheme, "dark");
  assert.ok(r.theme.name.length <= 40 && !/[\u0000-\u001f]/.test(r.theme.name));
});

test("every preset applies cleanly", () => {
  for (const p of PRESETS) {
    const root = fakeRoot();
    applyTheme(root, p);
    assert.equal(root.props.size, TOKEN_KEYS.length + 1, p.id);
  }
});

test("saved appearance: bad parts fall back one by one, and the serialised form carries the resolved tokens", async () => {
  const { DEFAULT_SAVED, applySaved, parseSaved, serialiseSaved, themeFor } = await import("../src/themes/apply.ts");
  assert.deepEqual(parseSaved(null, base), DEFAULT_SAVED);
  assert.deepEqual(parseSaved("{{", base), DEFAULT_SAVED);
  const s = parseSaved(
    JSON.stringify({ theme: "night-red", density: "huge", fontScale: 400, custom: [{ id: "c-1", name: "A", base: "slate", tokens: { accent: "#ff00aa", text: "evil;" } }, { id: "Bad Id" }] }),
    base,
  );
  assert.equal(s.theme, "night-red");
  assert.equal(s.density, "normal");
  assert.equal(s.fontScale, 130);
  assert.equal(s.custom.length, 1);
  assert.equal(s.custom[0].tokens.accent, "#ff00aa");
  assert.equal(s.custom[0].tokens.text, base("slate").tokens.text, "an invalid colour is replaced from the base");
  assert.equal(themeFor({ theme: "system", custom: [] }, false).id, "daylight");
  assert.equal(themeFor({ theme: "system", custom: [] }, true).id, "midnight");
  assert.equal(themeFor({ theme: "c-1", custom: s.custom }, true).tokens.accent, "#ff00aa");
  const out = JSON.parse(serialiseSaved({ ...s, theme: "c-1" }, true));
  assert.equal(out.tokens.accent, "#ff00aa");
  assert.equal(out.scheme, "dark");
  const root = fakeRoot();
  applySaved(root, { ...s, theme: "night-red", density: "compact", fontScale: 120 }, true);
  assert.equal(root.props.get("--canvas"), nightRed.tokens.canvas);
  assert.equal(root.attrs.get("data-density"), "compact");
  assert.equal(root.props.get("--font-scale"), "1.2");
});

test("names: C1 controls are stripped and the 40-character cut is by code point, so Rust accepts the saved name", async () => {
  const { cleanName, serialiseSaved } = await import("../src/themes/apply.ts");
  assert.equal(cleanName("a\u0085b\u009fc"), "a b c");
  const cut = cleanName("x".repeat(39) + "😀😀");
  assert.equal(Array.from(cut).length, 40);
  assert.ok(cut.endsWith("😀"), "the emoji is whole, not a lone surrogate");
  assert.doesNotThrow(() => encodeURIComponent(cut), "no lone surrogate");
  const r = parseThemeImport(doc({ name: "y".repeat(39) + "😀😀\u0085" }), base);
  assert.ok(r.ok);
  assert.ok(!/\p{Cc}/u.test(r.theme.name) && Array.from(r.theme.name).length <= 40);
  assert.doesNotThrow(() => encodeURIComponent(r.theme.name));
  // Saved names are cleaned again on the way out.
  const out = JSON.parse(serialiseSaved({ theme: "system", custom: [{ id: "c-1", name: "z".repeat(39) + "😀😀\u0085", base: "midnight", scheme: "dark", tokens: midnight.tokens }], density: "normal", fontScale: 100 }, true));
  assert.ok(!/\p{Cc}/u.test(out.custom[0].name) && Array.from(out.custom[0].name).length <= 40);
});
