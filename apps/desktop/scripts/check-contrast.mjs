// Theme contrast gate, run by scripts/check.sh (plain node, TypeScript by type stripping).
//  - every preset passes WCAG AA (AAA text for High Contrast) against its surfaces
//  - a deliberately bad fixture theme must FAIL the check, and auto-fix must repair it
//  - tokens.css (the pre-JS default) matches Midnight and Daylight exactly
//  - Night Red carries no blue or green light
// Run from apps/desktop: node scripts/check-contrast.mjs
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { autoFix, checkTheme, contrastRatio } from "../src/themes/contrast.ts";
import { PRESETS, TOKEN_KEYS, daylight, midnight, nightRed } from "../src/themes/presets.ts";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const problems = [];
const bad = (m) => problems.push(m);
const SURF = ["canvas", "surface", "raised", "bubble-in", "bubble-out"];

for (const p of PRESETS) {
  for (const k of TOKEN_KEYS) if (!/^#[0-9a-f]{3,8}$/i.test(p.tokens[k] ?? "")) bad(`${p.id}: token ${k} is missing or not a hex colour`);
  const fails = checkTheme(p);
  for (const f of fails) bad(`${p.id}: ${f.fg} on ${f.bg} is ${f.ratio.toFixed(2)}:1, needs ${f.min}:1`);
  const low = (fgs, bgs) => Math.min(...fgs.flatMap((fg) => bgs.map((bg) => contrastRatio(p.tokens[fg], p.tokens[bg])))).toFixed(2);
  console.log(
    `${fails.length ? "FAIL" : "ok  "} ${p.id.padEnd(20)} ${p.aaa ? "AAA" : "AA "} ` +
      `text ${low(["text"], SURF)} text-2 ${low(["text-2"], SURF)} accent ${low(["accent"], SURF.slice(0, 3))} ` +
      `status ${low(["success", "warning", "danger"], SURF.slice(0, 3))} badges ${low(["badge-beacon", "badge-cq", "badge-session"], SURF.slice(0, 3))} ` +
      `freq ${low(["freq-display"], ["surface", "raised"])} on-accent ${low(["accent-text"], ["accent"])} border-strong ${low(["border-strong"], SURF.slice(0, 3))} focus ${low(["focus"], SURF.slice(0, 3))}`,
  );
}

// The bad fixture: grey on grey, a faint control border, a focus ring that vanishes.
const fixture = { aaa: false, tokens: { ...midnight.tokens, text: "#777777", "text-2": "#666666", surface: "#888888", "border-strong": "#292929", focus: "#151d29" } };
const fixtureFails = checkTheme(fixture);
if (fixtureFails.length < 5) bad(`the bad fixture theme must fail the check (it produced ${fixtureFails.length} failures)`);
else console.log(`ok   bad fixture rejected (${fixtureFails.length} failures)`);
// A fixable one: the surfaces are fine, the foregrounds are not (what a user's custom accent can do).
const fixable = { aaa: false, tokens: { ...midnight.tokens, text: "#777777", "text-2": "#5a5a5a", accent: "#1a4a55", "border-strong": "#292929", focus: "#151d29" } };
if (checkTheme(fixable).length === 0) bad("the fixable fixture must fail before auto-fix");
const fixed = autoFix(fixable);
if (!fixed.passes) bad(`auto-fix could not repair the fixable fixture: ${JSON.stringify(checkTheme({ tokens: fixed.tokens }).map((f) => f.fg + "/" + f.bg))}`);
if (JSON.stringify(autoFix(fixable)) !== JSON.stringify(fixed)) bad("auto-fix is not deterministic");

// tokens.css is what the window shows before the theme code runs.
const css = readFileSync(join(root, "src", "tokens.css"), "utf8");
const block = (re) => {
  const m = css.match(re);
  if (!m) return {};
  return Object.fromEntries([...m[1].matchAll(/--([a-z0-9-]+):\s*(#[0-9a-fA-F]+)/g)].map((x) => [x[1], x[2].toLowerCase()]));
};
const dark = block(/:root\s*\{([^}]*)\}/);
const light = block(/prefers-color-scheme: light\)\s*\{\s*:root\s*\{([^}]*)\}/);
for (const [name, theme, got] of [["midnight", midnight, dark], ["daylight", daylight, light]]) {
  for (const k of TOKEN_KEYS) if (got[k] !== theme.tokens[k]) bad(`tokens.css ${name}: --${k} is ${got[k]}, the preset says ${theme.tokens[k]}`);
}

for (const [k, v] of Object.entries(nightRed.tokens)) {
  if (k === "scrim" || k === "shadow") continue;
  const g = parseInt(v.slice(3, 5), 16);
  const b = parseInt(v.slice(5, 7), 16);
  if (g > 0x95 || b > 0x84) bad(`night-red ${k} ${v} carries green or blue light (g ${g}, b ${b})`);
}

if (problems.length) {
  console.error("\ncheck-contrast FAILED:\n  " + problems.join("\n  "));
  process.exit(1);
}
console.log("check-contrast OK");
