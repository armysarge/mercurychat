// Localisation checks, run by scripts/check.sh (no test framework: plain node).
//
//  1. Every locale has exactly the keys of en.json (the source of truth), the
//     same {placeholders} and the same number of plural forms per message.
//  2. The three non-English files carry the machine-draft status marker.
//  3. No .vue template has bare text, and no user-visible attribute
//     (placeholder, title, aria-label, alt) or string literal in an
//     interpolation, outside t(). <script> and <style> are not scanned.
//  4. Every errors.* key the Rust contract can emit exists in en.json.
//  5. The keys Operate.vue uses resolve to French strings that differ from
//     English (the language switch visibly changes the Operate labels), and
//     the system-language mapping behaves.
//
// Run from apps/desktop: node scripts/check-i18n.mjs

import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { LOCALES, resolveLocale } from "../src/i18n/locale.ts";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const src = join(root, "src");
const problems = [];
const bad = (msg) => problems.push(msg);

// ---------------------------------------------------------------- locales

const DRAFT_STATUS = "machine draft — needs native review";
const files = {};
for (const l of LOCALES) files[l] = JSON.parse(readFileSync(join(src, "i18n", "locales", `${l}.json`), "utf8"));

function flatten(obj, prefix = "", out = {}) {
  for (const [k, v] of Object.entries(obj)) {
    const key = prefix ? `${prefix}.${k}` : k;
    if (v !== null && typeof v === "object") flatten(v, key, out);
    else if (typeof v === "string") out[key] = v;
    else bad(`${key}: a message must be a string`);
  }
  return out;
}

const flat = {};
for (const l of LOCALES) {
  const { _status, ...rest } = files[l];
  if (l === "en") {
    if (_status !== undefined) bad("en.json: the source of truth carries no _status");
  } else if (_status !== DRAFT_STATUS) {
    bad(`${l}.json: _status must be "${DRAFT_STATUS}"`);
  }
  flat[l] = flatten(rest);
}

const placeholders = (s) => [...new Set([...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]))].sort().join(",");
const forms = (s) => s.split("|").length;
for (const l of LOCALES) {
  if (l === "en") continue;
  for (const k of Object.keys(flat.en)) {
    if (!(k in flat[l])) {
      bad(`${l}.json: missing key ${k}`);
      continue;
    }
    if (placeholders(flat.en[k]) !== placeholders(flat[l][k])) {
      bad(`${l}.json: ${k} has placeholders {${placeholders(flat[l][k])}}, en has {${placeholders(flat.en[k])}}`);
    }
    if (forms(flat.en[k]) !== forms(flat[l][k])) bad(`${l}.json: ${k} has ${forms(flat[l][k])} plural forms, en has ${forms(flat.en[k])}`);
    if (flat[l][k].trim() === "") bad(`${l}.json: ${k} is empty`);
  }
  for (const k of Object.keys(flat[l])) if (!(k in flat.en)) bad(`${l}.json: ${k} is not in en.json`);
}

// ------------------------------------------------------------- templates

function walk(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
    e.isDirectory() ? (e.name === "bindings" ? [] : walk(join(dir, e.name))) : e.name.endsWith(".vue") ? [join(dir, e.name)] : [],
  );
}

/** The top-level <template> block's inner source, with its offset in the file. */
function templateOf(text) {
  const open = text.search(/^<template[^>]*>/m);
  if (open < 0) return "";
  const start = text.indexOf(">", open) + 1;
  const end = text.lastIndexOf("</template>");
  return text.slice(start, end);
}

/** Text a person reads in an allowed place: no letters, or a known literal. */
const ALLOWED_TOKENS = ["MercuryChat", "MCH/1", "UTC"];
const hasLetters = (s) => /\p{L}/u.test(s);
const USER_VISIBLE = new Set(["placeholder", "title", "aria-label", "alt"]);

/** String literals with letters in an expression, outside t('...') calls. */
function literalsOutsideT(expr) {
  const stripped = expr.replace(/\bt\(\s*(['"`])[^'"`]*\1/g, "t(");
  // Identifier-like literals (state names, keys, class names) are code, not words
  // a person reads: user text has a space, a capital or a non-ASCII letter.
  const prose = (s) => {
    const plain = s.replace(/\\./g, " ").trim();
    return hasLetters(plain) && /\s|^\p{Lu}|[^\x00-\x7f]/u.test(plain);
  };
  return [...stripped.matchAll(/(['"`])((?:\\.|(?!\1).)*)\1/g)].map((m) => m[2]).filter(prose);
}

function scanTemplate(file, tpl) {
  const rel = relative(root, file).replaceAll("\\", "/");
  let i = 0;
  const flag = (what, text) => bad(`${rel}: ${what} outside t(): ${JSON.stringify(text.trim().slice(0, 60))}`);

  const text = (s) => {
    // {{ expr }} parts: string literals with letters must be t() keys.
    for (const m of s.matchAll(/\{\{([\s\S]*?)\}\}/g)) for (const lit of literalsOutsideT(m[1])) flag("string literal in {{ }}", lit);
    let bare = s.replace(/\{\{[\s\S]*?\}\}/g, " ").replace(/&[a-z#0-9]+;/gi, " ");
    for (const token of ALLOWED_TOKENS) bare = bare.replaceAll(token, " ");
    if (hasLetters(bare.trim())) flag("bare text", bare);
  };

  while (i < tpl.length) {
    const lt = tpl.indexOf("<", i);
    if (lt < 0) {
      text(tpl.slice(i));
      break;
    }
    text(tpl.slice(i, lt));
    if (tpl.startsWith("<!--", lt)) {
      const end = tpl.indexOf("-->", lt);
      i = end < 0 ? tpl.length : end + 3;
      continue;
    }
    // Parse a tag, respecting quotes (an attribute may contain ">").
    let j = lt + 1;
    let quote = "";
    const attrStart = j;
    while (j < tpl.length) {
      const c = tpl[j];
      if (quote) {
        if (c === quote) quote = "";
      } else if (c === '"' || c === "'") quote = c;
      else if (c === ">") break;
      j++;
    }
    const tag = tpl.slice(attrStart, j);
    for (const m of tag.matchAll(/(?:^|\s)([:@#\w.\-]+)\s*=\s*("([^"]*)"|'([^']*)')/g)) {
      const name = m[1];
      const value = m[3] ?? m[4];
      const bound = name.startsWith(":") || name.startsWith("v-bind:");
      const plain = name.replace(/^(:|v-bind:)/, "");
      if (bound) {
        if (USER_VISIBLE.has(plain)) for (const lit of literalsOutsideT(value)) flag(`:${plain} literal`, lit);
        else if (/^(label|text|aria-\w+)$/.test(plain)) for (const lit of literalsOutsideT(value)) flag(`:${plain} literal`, lit);
      } else if (USER_VISIBLE.has(name) && hasLetters(value)) flag(`${name} attribute`, value);
    }
    i = j + 1;
  }
}

const vueFiles = walk(src);
for (const f of vueFiles) scanTemplate(f, templateOf(readFileSync(f, "utf8")));

// A self-test of the scanner, so a loosened regex cannot make it pass vacuously.
{
  const before = problems.length;
  scanTemplate(join(src, "Selftest.vue"), `<p title="Hello">Hi {{ x ? "No" : t('a') }}</p><b>Bare</b><input :placeholder="'Search'" />`);
  const found = problems.splice(before);
  if (found.length !== 5) bad(`scanner self-test: expected 5 findings, got ${found.length}: ${found.join(" | ")}`);
  scanTemplate(join(src, "Selftest.vue"), `<p :title="t('a')" aria-label="12">{{ n }} · <b class="mono">MercuryChat</b> <!-- Comment --> &gt; 1 · MCH/1</p>`);
  const clean = problems.splice(before);
  if (clean.length !== 0) bad(`scanner self-test: false positives: ${clean.join(" | ")}`);
}

// ------------------------------------------------------------ Rust errors

{
  const rs = readFileSync(join(root, "..", "..", "crates", "mc-core", "src", "app", "contract.rs"), "utf8");
  const keys = [...new Set([...rs.matchAll(/"(errors\.[A-Za-z0-9.]+)"/g)].map((m) => m[1]))];
  if (keys.length === 0) bad("contract.rs: no errors.* keys found");
  for (const k of keys) if (!(k in flat.en)) bad(`en.json: missing ${k}, which contract.rs emits`);
}

// ------------------------------------------------- Operate.vue in French

{
  const operate = readFileSync(join(src, "views", "Operate.vue"), "utf8");
  const used = [...new Set([...operate.matchAll(/\bt\(\s*['"`]([\w.]+)['"`]/g)].map((m) => m[1]))];
  if (used.length < 8) bad(`Operate.vue: only ${used.length} t() keys found; the check expects its labels to be keys`);
  // Words that are the same in French on purpose.
  const SAME = new Set(["operate.session", "operate.subtitle"]);
  let differing = 0;
  for (const k of used) {
    if (!(k in flat.en)) bad(`Operate.vue uses ${k}, which en.json lacks`);
    else if (!(k in flat.fr)) bad(`Operate.vue uses ${k}, which fr.json lacks`);
    else if (flat.fr[k] !== flat.en[k]) differing++;
    else if (!SAME.has(k)) bad(`fr.json: ${k} is identical to English ("${flat.en[k]}"); translate it or add it to the SAME list`);
  }
  if (differing < 8) bad(`only ${differing} Operate labels differ between French and English`);
  for (const k of ["operate.call", "operate.disconnect", "operate.abort"]) {
    if (!used.includes(k)) bad(`Operate.vue no longer uses ${k}`);
  }
}

// ------------------------------------------------- system-language mapping

{
  const cases = [
    [["system", ["fr-CA", "en-US"]], "fr"],
    [["system", ["pt-PT"]], "pt-BR"],
    [["system", ["de-DE", "es-MX"]], "es"],
    [["system", ["de-DE"]], "en"],
    [["system", []], "en"],
    [["fr", ["en-US"]], "fr"],
    [["xx", ["fr"]], "fr"],
  ];
  for (const [[setting, prefs], want] of cases) {
    const got = resolveLocale(setting, prefs);
    if (got !== want) bad(`resolveLocale(${setting}, ${JSON.stringify(prefs)}) = ${got}, want ${want}`);
  }
}

if (problems.length) {
  console.error(`check-i18n: ${problems.length} problem(s)`);
  for (const p of problems) console.error("  " + p);
  process.exit(1);
}
console.log(`check-i18n: ok (${Object.keys(flat.en).length} keys x ${LOCALES.length} locales, ${vueFiles.length} templates)`);
