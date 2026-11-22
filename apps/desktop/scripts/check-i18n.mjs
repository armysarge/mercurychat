// Localisation checks, run by scripts/check.sh (no test framework: plain node).
//
//  1. Every locale has exactly the keys of en.json (the source of truth), the
//     same {placeholders} and the same number of plural forms per message.
//  2. The three non-English files carry the machine-draft status marker.
//  3. No .vue template has bare text, and no user-visible attribute
//     (placeholder, title, aria-label, alt) or string literal in an
//     interpolation, outside t(). <script> and <style> are not scanned.
//  4. Every errors.* key the Rust contract can emit, and every broadcast.*
//     reason key broadcast.rs names, exists in en.json.
//  5. Plural rules follow CLDR per locale (0 is singular in fr and pt-BR), and every
//     key used anywhere in src/ (t(), keypath, key-shaped literals, dynamic prefixes) is in en.json.
//  6. The keys Operate.vue uses resolve to French strings that differ from
//     English (the language switch visibly changes the Operate labels), and
//     the system-language mapping behaves.
//
// Run from apps/desktop: node scripts/check-i18n.mjs

import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import * as localeMod from "../src/i18n/locale.ts";
const { LOCALES, resolveLocale } = localeMod;
import { LOGO_KEYS, PRESETS, TOKEN_KEYS } from "../src/themes/presets.ts";

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
  // The reasons a broadcast outcome carries (NotConfirmedReason::key).
  const brs = readFileSync(join(root, "..", "..", "crates", "mc-core", "src", "app", "broadcast.rs"), "utf8");
  const reasons = [...new Set([...brs.matchAll(/"(broadcast\.[A-Za-z0-9.]+)"/g)].map((m) => m[1]))];
  if (reasons.length === 0) bad("broadcast.rs: no broadcast.* keys found");
  for (const k of reasons) if (!(k in flat.en)) bad(`en.json: missing ${k}, which broadcast.rs emits`);
}

// ------------------------------------------------------------ plural rules

{
  const rules = localeMod.pluralRules;
  const index = readFileSync(join(src, "i18n", "index.ts"), "utf8");
  if (!rules) bad("locale.ts: no pluralRules (vue-i18n's default picks the plural at 0, wrong for fr and pt-BR)");
  else if (!/createI18n\(\{[\s\S]*\bpluralRules\b[\s\S]*\}\)/.test(index)) bad("i18n/index.ts: createI18n does not set pluralRules");
  else {
    const { createI18n } = await import("vue-i18n");
    // "one | other" messages: what each locale shows for 0, 1 and 2 (CLDR).
    const want = { en: ["0 messages", "1 message", "2 messages"], es: ["0 mensajes", "1 mensaje", "2 mensajes"], fr: ["0 message", "1 message", "2 messages"], "pt-BR": ["0 mensagem", "1 mensagem", "2 mensagens"] };
    for (const l of LOCALES) {
      // The same options index.ts passes, with the real messages.
      const i18n = createI18n({ legacy: false, locale: l, fallbackLocale: "en", pluralRules: rules, messages: { [l]: files[l] } });
      const got = [0, 1, 2].map((n) => i18n.global.t("conversations.messageCount", n));
      if (JSON.stringify(got) !== JSON.stringify(want[l])) bad(`plural rule for ${l}: messageCount(0,1,2) = ${JSON.stringify(got)}, want ${JSON.stringify(want[l])}`);
    }
  }
}

// ------------------------------------------------- every key used exists

{
  /** Source files whose keys are checked: all .vue and .ts, except generated bindings. */
  const walkAll = (dir) =>
    readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
      e.isDirectory() ? (e.name === "bindings" ? [] : walkAll(join(dir, e.name))) : /\.(vue|ts)$/.test(e.name) ? [join(dir, e.name)] : [],
    );
  const namespaces = new Set(Object.keys(flat.en).map((k) => k.split(".")[0]));
  const binding = (file) => readFileSync(join(src, "bindings", file), "utf8").replace(/\/\*\*[\s\S]*?\*\//g, "");
  /** The members of a string-literal union type (`"cq" | "beacon"`). */
  const unionOf = (file) => [...binding(file).split("=").slice(1).join("=").matchAll(/"(\w+)"/g)].map((m) => m[1]);
  /** The `type` tags of a tagged union (`{ "type": "up" } | ...`). */
  const taggedOf = (file) => [...binding(file).matchAll(/"type": "(\w+)"/g)].map((m) => m[1]);
  /** The strings of a component's `const NAME = [...] as const` list. */
  const constOf = (file, name) => {
    const text = readFileSync(join(src, file), "utf8");
    const list = new RegExp(`const ${name} = \\[([^\\]]*)\\] as const`).exec(text)?.[1] ?? "";
    const values = [...list.matchAll(/"([\w-]+)"/g)].map((m) => m[1]);
    if (!values.length) throw new Error(`check-i18n: no \`const ${name} = [...] as const\` in ${file}`);
    return values;
  };
  /** Values the dynamic keys `${prefix}${x}` can take, from the app's own lists. */
  const dynamic = {
    "theme.name.": () => PRESETS.map((p) => p.id),
    "theme.token.": () => TOKEN_KEYS.filter((k) => k !== "scrim" && k !== "shadow" && !LOGO_KEYS.includes(k)),
    "theme.file.": () => ["tooLarge", "notJson", "notTheme", "version"],
    "modem.": () => [...readFileSync(join(src, "bindings", "ModemState.ts"), "utf8").matchAll(/"type": "(\w+)"/g)].map((m) => m[1]),
    // Task 10: the values come from the generated contract, so a new variant fails here until it has words.
    "heard.source.": () => unionOf("HeardSource.ts"),
    "heard.filter.": () => ["all", "favourite", ...unionOf("HeardSource.ts")],
    "beacon.channel.": () => taggedOf("ChannelState.ts"),
    "telemetry.link.": () => taggedOf("TelemetryLinkView.ts"),
    "strip.outcome.": () => taggedOf("BroadcastOutcome.ts"),
    "broadcast.notConfirmed.": () => unionOf("NotConfirmedReason.ts"),
    // Task 11: the lists the components iterate, read from their source.
    "about.link.": () => constOf("components/About.vue", "LINKS"),
    "about.credit.": () => constOf("components/About.vue", "CREDITS"),
    "stationInfo.": () => constOf("components/StationInfo.vue", "FIELDS"),
    "logbook.col.": () => constOf("views/Logbook.vue", "COLUMNS"),
    "transcript.sys.": () => unionOf("BroadcastKind.ts"),
    // Task 12b: notification kinds, and the message and modem states notify.ts names.
    "notifySettings.kind.": () => constOf("components/NotificationSettings.vue", "KINDS"),
    "notify.delivery.": () => ["uncertain", "failed"],
    "notify.fault.": () => ["faulted", "recovering"],
    // M3b: the rig's banners (from the contract) and the gate's reasons (rig.ts mirrors the core's).
    "banner.rig.": () => unionOf("RigBanner.ts"),
    "rig.refused.": () => constOf("rig.ts", "REFUSALS"),
    "settings.tabs.": () => constOf("views/Settings.vue", "TABS"),
    // M3b.1: Settings → Rig's dropdowns (rigForm.ts) and Settings → Audio's channels.
    "rigSettings.freqControlOpt.": () => constOf("rigForm.ts", "FREQ_CONTROLS"),
    "rigSettings.pttOpt.": () => constOf("rigForm.ts", "PTT_METHODS"),
    "rigSettings.pttLineOpt.": () => constOf("rigForm.ts", "PTT_LINES"),
    "rigSettings.linkOpt.": () => constOf("rigForm.ts", "LINKS"),
    "rigSettings.parityOpt.": () => constOf("rigForm.ts", "PARITIES"),
    "rigSettings.lineState.": () => constOf("rigForm.ts", "LINE_STATES"),
    "audioSettings.channelOpt.": () => constOf("components/AudioSettings.vue", "CHANNELS"),
    "rigSettings.step.": () => constOf("components/RigSettings.vue", "STEPS"),
    "freqList.purposeOpt.": () => constOf("components/FrequencyListEditor.vue", "PURPOSES"),
    // M3c: the QSY card's stages and notices, and the QSO card's peer-report fields.
    "qsy.stage.": () => constOf("components/QsyCard.vue", "STAGES"),
    "qsy.notify.": () => constOf("components/QsyCard.vue", "noticeKeys"),
    "qso.field.": () => constOf("components/QsoCard.vue", "PEER_FIELDS"),
    // M4: photo sizes and transfer states (the contract), the Files view's lists, and the reason codes transfers.ts maps.
    "files.photo.": () => unionOf("PhotoSize.ts"),
    "files.state.": () => unionOf("TransferState.ts"),
    "files.view.": () => constOf("views/Files.vue", "SECTIONS"),
    "files.reason.": () => {
      const t = readFileSync(join(src, "transfers.ts"), "utf8");
      const block = /const REASONS[^=]*=\s*\{([\s\S]*?)\n\};/.exec(t)?.[1] ?? "";
      return [...block.matchAll(/:\s*"(\w+)"/g)].map((m) => m[1]);
    },
    "transcript.status.": () => {
      const t = readFileSync(join(src, "components", "Transcript.vue"), "utf8");
      const block = /const STATUS[^=]*=\s*\{([\s\S]*?)\n\};/.exec(t)?.[1] ?? "";
      const fromTable = [...block.matchAll(/^\s*(\w+):/gm)].map((m) => m[1]);
      return [...new Set(["queued", "submitting", "awaiting_receipt", "delivered", "failed", "uncertain", "received", ...fromTable])];
    },
  };

  /** Problems in one source text: literal keys missing from en.json, unknown dynamic prefixes. */
  function keyProblems(name, text) {
    const out = [];
    const need = (key, how) => {
      if (!(key in flat.en)) out.push(`${name}: ${how} ${key} is not in en.json`);
    };
    for (const m of text.matchAll(/(?<![\w.$])\$?te?\(\s*(['"`])([\w.]+)\1/g)) need(m[2], "t()");
    for (const m of text.matchAll(/\bkeypath\s*=\s*"([\w.]+)"/g)) need(m[1], "keypath");
    // A string literal shaped like a key of a known area is a key (label: "nav.operate").
    // (Not an attribute value such as :role="banner.role", which is code.)
    for (const m of text.matchAll(/(?<!=)(['"])([a-z]\w*(?:\.\w+)+)\1/g)) if (namespaces.has(m[2].split(".")[0])) need(m[2], "key literal");
    for (const m of text.matchAll(/`([\w.]+\.)\$\{[^}]*\}`/g)) {
      const values = dynamic[m[1]];
      if (!values) out.push(`${name}: dynamic key \`${m[1]}\${...}\` has no list of values in check-i18n.mjs`);
      else for (const v of values()) need(m[1] + v, "dynamic key");
    }
    return out;
  }

  const before = problems.length;
  // Self-test: a crafted source with each kind of mistake must be caught, and a good one must not.
  const crafted = [
    `t('heard.emtpy')`,
    `<i18n-t keypath="nav.nope">`,
    `{ label: "nav.operat" }`,
    "t(`mystery.${x}`)",
    `const k = "modem.nonexistent"`,
  ].join("\n");
  const caught = keyProblems("selftest", crafted);
  if (caught.length !== 5) bad(`key scanner self-test: expected 5 findings, got ${caught.length}: ${caught.join(" | ")}`);
  const good = keyProblems("selftest", `t("nav.operate") te('errors.busy') {label: "nav.settings"} t(\`modem.\${s}\`) "./i18n" "a.b.c" 'x.y'`);
  if (good.length) bad(`key scanner self-test: false positives: ${good.join(" | ")}`);
  problems.length = before;

  for (const f of walkAll(src)) {
    if (f.endsWith("locale.ts")) continue;
    for (const p of keyProblems(relative(root, f).replaceAll("\\", "/"), readFileSync(f, "utf8"))) bad(p);
  }
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
