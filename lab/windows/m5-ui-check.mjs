// M5 Task 8 check of the real desktop window, driven over the WebView2
// DevTools protocol. The built-in modem is a FAKE bundle: fake-mercury as
// `mercury.exe` and fake-rigctld as `rigctld.exe`, with a manifest and
// BUILD.txt, given to a debug build through MC_BUNDLE_DIR. No Mercury, no
// audio device, no rig, no RF. Ports 18300/18100/20000/14532 (and the free
// set after them), away from any Mercury the operator runs.
//   B1 a built-in profile reaches Ready; the strip says "Choose a sound card"
//   B2 a taken port is named; "Use the free set" fixes it
//   B3 the built-in versions (BUILD.txt) appear next to each mention: the
//      modem choice, Settings → Rig, Settings → Modem, and About
//   B4 switching to a My own Mercury profile stops the built-in Mercury
//      (its control port closes)
//   B5 no console errors from the first screen to the end
//   B6 Settings → About lists the diagnostics file's entries before saving:
//      versions, settings and Mercury's log; no database (Task 11)
//   B7 Settings → About → Licences lists the components of THIRD-PARTY.md (Task 12)
// Usage: node lab/windows/m5-ui-check.mjs <mercurychat.exe> <empty data dir> [port]
// Build first (debug: MC_BUNDLE_DIR is read only by debug builds):
//   cargo build -p mercury-adapter --bins
//   npm --prefix apps/desktop run build && cargo build -p mercurychat --features tauri/custom-protocol
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { createServer, connect as tcp } from "node:net";
import { dirname, join } from "node:path";
import { firstUse, kill, launch, page, sleep, until } from "./cdp.mjs";
import { click, pill, setInput } from "./m2-dom.mjs";

const [exe, dataDir, port = "9363"] = process.argv.slice(2);
if (!exe || !dataDir) {
  console.error("usage: node m5-ui-check.mjs <mercurychat.exe> <empty data dir> [port]");
  process.exit(2);
}
const CALL = "ZS6BIN";
const MERCURY = "1.9.25";
const HAMLIB = "4.7.2";
const rail = (name) => `[...document.querySelectorAll('.rail button')].find(b => b.getAttribute('aria-label') === ${JSON.stringify(name)})`;
const text = (sel) => `(document.querySelector(${JSON.stringify(sel)})?.textContent ?? '').replace(/\\s+/g, ' ').trim()`;
const watchErrors = `(() => {
  if (window.__uiErrors) return;
  window.__uiErrors = [];
  const keep = (x) => window.__uiErrors.push(String(x?.stack ?? x?.message ?? x).slice(0, 300));
  const err = console.error.bind(console);
  console.error = (...a) => { keep(a.join(' ')); err(...a); };
  addEventListener('error', (e) => keep(e.error ?? e.message));
  addEventListener('unhandledrejection', (e) => keep(e.reason));
})()`;

/** The fake bundle beside the data dir: fake programs, manifest, BUILD.txt. */
function fakeBundle() {
  const bin = dirname(exe);
  const dir = `${dataDir}-bundle`;
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  let manifest = "";
  for (const [from, to] of [["fake-mercury.exe", "mercury.exe"], ["fake-rigctld.exe", "rigctld.exe"]]) {
    if (!existsSync(join(bin, from))) throw new Error(`${from} is not built: cargo build -p mercury-adapter --bins`);
    copyFileSync(join(bin, from), join(dir, to));
    manifest += `${createHash("sha256").update(readFileSync(join(dir, to))).digest("hex")}  ${to}\n`;
  }
  writeFileSync(join(dir, "MANIFEST.sha256"), manifest);
  writeFileSync(join(dir, "BUILD.txt"), `target=windows-x64\nhamlib_version=${HAMLIB}\nmercury_version=v${MERCURY}\n`);
  return dir;
}

const open = (p) =>
  new Promise((resolve) => {
    const c = tcp({ host: "127.0.0.1", port: p });
    c.once("connect", () => { c.destroy(); resolve(true); });
    c.once("error", () => resolve(false));
    setTimeout(() => { c.destroy(); resolve(false); }, 1500);
  });
const hold = (p) =>
  new Promise((resolve, reject) => {
    const srv = createServer(() => {});
    srv.once("error", reject);
    srv.listen(p, "127.0.0.1", () => resolve(srv));
  });

const results = [];
let s;
let app;

async function noErrors(when) {
  const errs = (await s.eval("window.__uiErrors ?? []")) ?? [];
  if (errs.length) throw new Error(`console errors (${when}): ${errs.join(" | ")}`);
}

async function settingsTab(name) {
  await s.eval(`${rail("Settings")}.click()`);
  await until(s, `!!document.getElementById('tab-${name}')`, "Settings");
  await s.eval(`document.getElementById('tab-${name}').click()`);
}

try {
  if (existsSync(dataDir)) rmSync(dataDir, { recursive: true, force: true });
  const bundle = fakeBundle();
  for (const p of [18300, 18301, 18100, 20000, 14532]) {
    if (await open(p)) throw new Error(`port ${p} is in use before the check: free it first`);
  }

  process.env.MC_BUNDLE_DIR = bundle;
  app = launch(exe, dataDir, port);
  delete process.env.MC_BUNDLE_DIR;
  s = await firstUse(port, "Built-in Mercury", 40000);
  await s.eval(watchErrors);

  // ---- B3 (1): the modem choice names the built-in version.
  await until(s, `${text("form.builtin h2")}.includes(${JSON.stringify(MERCURY)})`, "the built-in version on the choice", 10000);
  const choice = await s.eval(text("form.builtin h2"));

  // ---- B2: a taken port is named; "Use the free set" fixes it.
  const holder = await hold(18100);
  await s.eval(setInput('form.builtin input[name="callsign"]', CALL));
  await s.eval("document.querySelector('form.builtin details.advanced').open = true");
  for (const [n, v] of [["control", 18300], ["broadcast", 18100], ["ui", 20000], ["rigctld", 14532]]) {
    await s.eval(setInput(`form.builtin input[name="${n}"]`, String(v)));
  }
  await s.eval("document.querySelector('form.builtin button[type=submit]').click()");
  await until(s, "!!document.querySelector('form.builtin .port-taken')", "the taken port named", 10000);
  const taken = await s.eval(text("form.builtin .port-taken"));
  if (!taken.includes("18100")) throw new Error(`B2: the message does not name 18100: ${taken}`);
  await s.eval("document.querySelector('form.builtin button.use-free').click()");
  const ready = `['ready','listening'].includes(${pill})`;
  await until(s, ready, "the built-in modem ready", 40000);
  const control = await s.eval(
    "window.__TAURI_INTERNALS__.invoke('app_call', { cmd: { type: 'snapshot' } }).then(s => s.profiles.find(p => p.id === s.activeProfileId).modem.ports.control)",
  );
  holder.close();
  if (control === 18300) throw new Error("B2: the profile kept the taken set");
  results.push(`PASS B2 taken port: "${taken}"; the free set (control ${control}) started`);

  // ---- B1: Ready, and the strip asks for a sound card.
  await until(s, `${text(".strip .builtin-note")}.includes('Choose a sound card')`, "Choose a sound card", 15000);
  const note = await s.eval(text(".strip .builtin-note"));
  if (!(await open(control))) throw new Error(`B1: nothing listens on the built-in control port ${control}`);
  results.push(`PASS B1 built-in profile: ${await s.eval(pill)}; strip: "${note}"`);

  // ---- B3 (2–4): Settings → Rig, Settings → Modem, About.
  await settingsTab("rig");
  await until(s, `${text("#panel-rig .hamlib-builtin")}.includes(${JSON.stringify(HAMLIB)})`, "Hamlib's version in Settings → Rig");
  const rig = await s.eval(text("#panel-rig .hamlib-builtin"));
  await settingsTab("modem");
  await until(
    s,
    `${text("#panel-modem .mercury-version")}.includes(${JSON.stringify(MERCURY)}) && ${text("#panel-modem .hamlib-version")}.includes(${JSON.stringify(HAMLIB)}) && ${text("#panel-modem .integrity")}.length > 0`,
    "the versions in Settings → Modem",
    15000,
  );
  const modem = await s.eval(text("#panel-modem .modem-info"));
  await settingsTab("about");
  await until(s, `${text("#panel-about .builtin-versions")}.includes(${JSON.stringify(MERCURY)}) && ${text("#panel-about .builtin-versions")}.includes(${JSON.stringify(HAMLIB)})`, "the versions in About");
  const about = await s.eval(text("#panel-about .builtin-versions"));
  results.push(`PASS B3 versions: choice "${choice}"; rig "${rig}"; modem "${modem}"; about "${about}"`);

  // ---- B4: My own Mercury stops the built-in one.
  await settingsTab("profiles");
  await until(s, "!!document.querySelector('#panel-profiles form.existing')", "the My own Mercury form");
  await s.eval(setInput('#panel-profiles form.existing input[name="callsign"]', "ZS6OWN"));
  await s.eval(setInput('#panel-profiles form.existing input[name="controlPort"]', "18900"));
  await s.eval(setInput('#panel-profiles form.existing input[name="dataPort"]', "18901"));
  await s.eval(setInput('#panel-profiles form.existing input[name="broadcastPort"]', ""));
  await s.eval("document.querySelector('#panel-profiles form.existing button[type=submit]').click()");
  let closed = false;
  for (let i = 0; i < 40 && !closed; i++) {
    await sleep(250);
    closed = !(await open(control));
  }
  if (!closed) throw new Error(`B4: the built-in Mercury still listens on ${control}`);
  results.push(`PASS B4 my own Mercury: the built-in control port ${control} closed`);

  // ---- B6: the diagnostics file's preview (the save dialog is the system's).
  await settingsTab("about");
  await s.eval("document.querySelector('#panel-about button.diag-preview').click()");
  await until(s, "document.querySelectorAll('#panel-about .diag-entries [data-name]').length >= 3", "the diagnostics preview");
  const diag = await s.eval("[...document.querySelectorAll('#panel-about .diag-entries [data-name]')].map(e => e.dataset.name)");
  for (const want of ["versions.txt", "profile.json", "rigctld.log"]) {
    if (!diag.includes(want)) throw new Error(`B6: the preview lacks ${want}: ${diag.join(", ")}`);
  }
  if (diag.some((n) => /\.db$|files\//.test(n))) throw new Error(`B6: the preview lists data: ${diag.join(", ")}`);
  results.push(`PASS B6 diagnostics preview: ${diag.join(", ")}`);

  // ---- B7: About → Licences, the same list as THIRD-PARTY.md (M5 Task 12).
  await s.eval("document.querySelector('#panel-about button.licences-show').click()");
  await until(s, "document.querySelectorAll('#panel-about .licences [data-component]').length > 50", "the licence list");
  const lic = await s.eval(`(() => {
    const rows = [...document.querySelectorAll('#panel-about .licences [data-component]')];
    const has = (n) => rows.some((r) => r.dataset.component === n);
    return { n: rows.length, mercury: has('Mercury'), hamlib: has('Hamlib'), tauri: has('tauri'), vue: has('vue') };
  })()`);
  for (const k of ["mercury", "hamlib", "tauri", "vue"]) if (!lic[k]) throw new Error(`B7: the licence list has no ${k}`);
  results.push(`PASS B7 licences: ${lic.n} components, Mercury, Hamlib, tauri and vue among them`);

  await sleep(1000);
  await noErrors("the end");
  results.push("PASS B5 console: no errors from the first screen to the end");
} catch (e) {
  results.push(`FAIL ${e.message}`);
  if (app?.out) console.error(app.out.slice(-2000));
} finally {
  s?.close();
  kill(app);
}
for (const r of results) console.log(r);
process.exit(results.some((r) => r.startsWith("FAIL")) || results.length < 7 ? 1 : 0);
