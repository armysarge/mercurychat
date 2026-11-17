// Colour-literal lint, run by scripts/check.sh: every colour goes through a
// theme token. A hex, rgb()/hsl()/oklch()/... or named colour in a .vue, .css,
// .html or .ts file fails, except in the theme files (tokens.css and
// themes/presets.ts) and the test fixtures. `var(--x)` and color-mix over
// tokens are fine; so are transparent, currentColor and inherit.
// Run from apps/desktop: node scripts/check-colours.mjs   (or with --self-test)
import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const ALLOWED = new Set(["src/tokens.css", "src/themes/presets.ts"]);
const NAMED =
  "aliceblue|antiquewhite|aqua|aquamarine|azure|beige|bisque|black|blanchedalmond|blue|blueviolet|brown|burlywood|cadetblue|chartreuse|chocolate|coral|cornflowerblue|cornsilk|crimson|cyan|darkblue|darkcyan|darkgoldenrod|darkgray|darkgrey|darkgreen|darkkhaki|darkmagenta|darkolivegreen|darkorange|darkorchid|darkred|darksalmon|darkseagreen|darkslateblue|darkslategray|darkslategrey|darkturquoise|darkviolet|deeppink|deepskyblue|dimgray|dimgrey|dodgerblue|firebrick|floralwhite|forestgreen|fuchsia|gainsboro|ghostwhite|gold|goldenrod|gray|grey|green|greenyellow|honeydew|hotpink|indianred|indigo|ivory|khaki|lavender|lavenderblush|lawngreen|lemonchiffon|lightblue|lightcoral|lightcyan|lightgoldenrodyellow|lightgray|lightgrey|lightgreen|lightpink|lightsalmon|lightseagreen|lightskyblue|lightslategray|lightslategrey|lightsteelblue|lightyellow|lime|limegreen|linen|magenta|maroon|mediumaquamarine|mediumblue|mediumorchid|mediumpurple|mediumseagreen|mediumslateblue|mediumspringgreen|mediumturquoise|mediumvioletred|midnightblue|mintcream|mistyrose|moccasin|navajowhite|navy|oldlace|olive|olivedrab|orange|orangered|orchid|palegoldenrod|palegreen|paleturquoise|palevioletred|papayawhip|peachpuff|peru|pink|plum|powderblue|purple|rebeccapurple|red|rosybrown|royalblue|saddlebrown|salmon|sandybrown|seagreen|seashell|sienna|silver|skyblue|slateblue|slategray|slategrey|snow|springgreen|steelblue|tan|teal|thistle|tomato|turquoise|violet|wheat|white|whitesmoke|yellow|yellowgreen";
const PROPS = "color|background|background-color|border|border-color|border-top|border-right|border-bottom|border-left|border-top-color|border-right-color|border-bottom-color|border-left-color|outline|outline-color|fill|stroke|box-shadow|text-shadow|caret-color|accent-color|text-decoration-color|column-rule";

const checks = [
  [/(?<![\w&-])#[0-9a-fA-F]{3,8}\b/, "hex colour"],
  [/\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch|color)\(/, "colour function"],
  [new RegExp(`\\b(?:${PROPS})\\s*[:=]\\s*["']?[^;}"']*?\\b(?:${NAMED})\\b`, "i"), "named colour"],
];

/** Problems in one file's text, as "line: what". */
export function scan(text) {
  const found = [];
  text.split(/\r?\n/).forEach((line, i) => {
    const code = line.replace(/\/\*.*?\*\//g, "").replace(/(^|\s)\/\/.*$/, "");
    // `color-mix(` over tokens is allowed: strip it before looking for colour functions.
    const stripped = code.replace(/color-mix\(/g, "mix(");
    for (const [re, what] of checks) if (re.test(stripped)) found.push(`${i + 1}: ${what}: ${line.trim().slice(0, 100)}`);
  });
  return found;
}

function* walk(dir) {
  for (const name of readdirSync(dir)) {
    if (name === "node_modules" || name === "dist" || name === "bindings" || name === "locales") continue;
    const p = join(dir, name);
    if (statSync(p).isDirectory()) yield* walk(p);
    else if (/\.(vue|css|html|ts)$/.test(name) && !name.endsWith(".d.ts")) yield p;
  }
}

if (process.argv.includes("--self-test")) {
  const mustFail = ["a { color: #fff }", "x { box-shadow: 0 1px rgb(0 0 0 / .3) }", "p { background: white }", 'fill="red"', "q { color: hsl(1 2% 3%) }"];
  const mustPass = ["a { color: var(--text) }", "b { background: color-mix(in srgb, var(--accent) 16%, var(--raised)) }", "c { border-color: transparent }", "<div id=\"app\">", "a { background: currentColor }"];
  for (const s of mustFail) if (!scan(s).length) { console.error(`lint missed: ${s}`); process.exit(1); }
  for (const s of mustPass) if (scan(s).length) { console.error(`lint flagged: ${s}`); process.exit(1); }
  console.log("check-colours self-test OK");
  process.exit(0);
}

const roots = [join(root, "src"), join(root, "index.html"), join(root, "splash.html")];
const problems = [];
for (const r of roots) {
  const files = statSync(r).isDirectory() ? [...walk(r)] : [r];
  for (const f of files) {
    const rel = relative(root, f).replaceAll("\\", "/");
    if (ALLOWED.has(rel)) continue;
    for (const p of scan(readFileSync(f, "utf8"))) problems.push(`${rel}:${p}`);
  }
}
if (problems.length) {
  console.error("check-colours FAILED: colours must be theme tokens (var(--...)), not literals:\n  " + problems.join("\n  "));
  process.exit(1);
}
console.log("check-colours OK");
