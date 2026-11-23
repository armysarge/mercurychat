// M5 Task 9 check of the first-run wizard in the real desktop window, driven
// over the WebView2 DevTools protocol. The built-in modem is a FAKE bundle
// (fake-mercury as `mercury.exe`, fake-rigctld as `rigctld.exe`, with a
// manifest and BUILD.txt) given to a debug build through MC_BUNDLE_DIR. No
// Mercury, no audio device (the fake's first start is on "null"), no rig, no
// RF: "Key the radio for 3 s" is never pressed. Ports 18600/18250/20300/14732
// (or the free set after them), away from any Mercury the operator runs.
//   W1 a new install opens the wizard at Welcome: no strip, no Receive-only switch
//   W2 the built-in path to Done: the station is held in Receive only (Ready,
//      never Listening) until "How you operate" says Answering calls; a step
//      skipped on the way is listed in Settings
//   W3 the app killed on the Sound card step: the next start resumes there,
//      still Ready
//   W4 the Rig step lists Hamlib's models from the bundled rigctld -l, and its
//      read-only test connects to MercuryChat's own rigctld
//   W5 the Modem and Rig steps name the built-in Mercury and Hamlib versions
//   W6 every step fits 768 and 1024 px wide (responsive.mjs), screenshots kept
//   W7 no console errors from the first screen to the end
// Usage: node lab/windows/m5-wizard-check.mjs <mercurychat.exe> <empty data dir> <png out dir> [port]
// Build first (debug: MC_BUNDLE_DIR is read only by debug builds):
//   cargo build -p mercury-adapter --bins
//   npm --prefix apps/desktop run build && cargo build -p mercurychat --features tauri/custom-protocol
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { dirname, join } from "node:path";
import { kill, launch, page, sleep, until } from "./cdp.mjs";
import { pill, setInput } from "./m2-dom.mjs";
import { SIZES, shoot } from "./responsive.mjs";

const [exe, dataDir, outDir, port = "9365"] = process.argv.slice(2);
if (!exe || !dataDir || !outDir) {
  console.error("usage: node m5-wizard-check.mjs <mercurychat.exe> <empty data dir> <png out dir> [port]");
  process.exit(2);
}
const CALL = "ZS6WIZ";
const MERCURY = "1.9.25";
const HAMLIB = "4.7.2";
const TABLET = SIZES.slice(0, 2);
const step = "document.querySelector('.wizard .body')?.dataset.step";
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

const results = [];
const fit = [];
let s;
let app;
let bundle;

function start() {
  process.env.MC_BUNDLE_DIR = bundle;
  app = launch(exe, dataDir, port);
  delete process.env.MC_BUNDLE_DIR;
}
async function errorsSoFar(when) {
  const errs = await s.eval("window.__uiErrors ?? []");
  if (errs.length) throw new Error(`W7 console errors by ${when}: ${errs.join(" | ")}`);
}
const fits = async (view) => fit.push(...(await shoot(s, outDir, `wizard-${view}`, TABLET)));
async function press(sel) {
  await s.eval(`(() => { const b = document.querySelector(${JSON.stringify(sel)}); if (!b) throw new Error('no ' + ${JSON.stringify(sel)}); if (b.disabled) throw new Error('disabled ' + ${JSON.stringify(sel)}); b.click(); })()`);
}
const at = (name, ms = 15000) => until(s, `${step} === ${JSON.stringify(name)}`, `the ${name} step`, ms);
/** The modem state stays Ready (never Listening) for `ms`. */
async function heldReady(ms, what) {
  await until(s, `['ready','listening'].includes(${pill})`, `the modem attached (${what})`, 40000);
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const p = await s.eval(pill);
    if (p === "listening") throw new Error(`${what}: the station answers calls during the wizard`);
    await sleep(200);
  }
}

try {
  if (existsSync(dataDir)) rmSync(dataDir, { recursive: true, force: true });
  mkdirSync(outDir, { recursive: true });
  bundle = fakeBundle();
  start();

  // ---- W1: a new install opens the wizard.
  s = await page(port, (t) => t.includes("Welcome to MercuryChat"), "the wizard", 40000);
  await s.eval(watchErrors);
  await at("welcome");
  if (await s.eval("!!document.querySelector('section.strip, header .station-mode, .sidebar')")) {
    throw new Error("W1: the strip, sidebar or Receive-only switch shows during the wizard");
  }
  // The versions arrive once the bundle is hashed (off the actor).
  await until(s, `${text(".wizard .need")}.includes(${JSON.stringify(MERCURY)})`, `Welcome naming Mercury ${MERCURY}`);
  results.push(`PASS W1 first launch: the wizard at Welcome ("${await s.eval(text(".wizard .count"))}"), no strip or switch`);
  await fits("welcome");

  // ---- Station.
  await press(".wizard .nav .next");
  await at("station");
  await s.eval(setInput('.wizard input[name="callsign"]', CALL.toLowerCase()));
  await s.eval(setInput('.wizard input[name="name"]', "Wizard Test"));
  await s.eval(setInput('.wizard input[name="grid"]', "KG33xi"));
  await fits("station");
  await press(".wizard .nav .next");
  await at("modem");

  // ---- Modem (W5 part 1): built-in, the callsign from Station.
  const choice = await s.eval(text(".wizard form.builtin h2"));
  if (!choice.includes(MERCURY)) throw new Error(`W5: the modem step names "${choice}"`);
  if (await s.eval("!!document.querySelector('.wizard form.builtin input[name=callsign]')")) throw new Error("W2: the modem step asks the callsign again");
  if (await s.eval("!!document.querySelector('.wizard form.simulator')")) throw new Error("W2: the wizard offers the simulator");
  await fits("modem");
  await s.eval("document.querySelector('.wizard form.builtin details.advanced').open = true");
  for (const [n, v] of [["control", 18600], ["broadcast", 18250], ["ui", 20300], ["rigctld", 14732]]) {
    await s.eval(setInput(`.wizard form.builtin input[name="${n}"]`, String(v)));
  }
  await press(".wizard form.builtin button[type=submit]");
  await sleep(1500);
  if (await s.eval("!!document.querySelector('.wizard form.builtin button.use-free')")) await press(".wizard form.builtin button.use-free");
  await at("audio", 40000);
  await heldReady(3000, "after the modem step");

  // ---- W3: killed on the Sound card step; the next start resumes there.
  s.close();
  kill(app);
  await sleep(1500);
  start();
  s = await page(port, (t) => t.includes("Sound card") && t.includes("Step 4"), "the wizard after the restart", 40000);
  await s.eval(watchErrors);
  await at("audio");
  await heldReady(3000, "after the restart");
  results.push("PASS W3 restart on the Sound card step: resumed there, the station still Ready (Receive only)");

  // ---- Sound card: the forms and the explicit transmit test (not pressed).
  await until(s, "!!document.querySelector('.wizard .tx-test button.key-radio') && !!document.querySelector('.wizard .rx-check')", "the sound card step");
  await fits("audio");
  await press(".wizard .nav .next");
  await at("rig");

  // ---- W4 and W5 part 2: the rig step, our own rigctld on the dummy rig.
  await s.eval(`(() => { const sel = document.querySelector('.wizard select.rig-freq-control'); sel.value = 'managedRigctld'; sel.dispatchEvent(new Event('change')); })()`);
  await until(s, "document.querySelectorAll('#rig-models option').length >= 4", "Hamlib's models from the bundled rigctld", 20000);
  const models = await s.eval("document.querySelectorAll('#rig-models option').length");
  const hamlib = await s.eval(text(".wizard .hamlib-builtin"));
  if (!hamlib.includes(HAMLIB)) throw new Error(`W5: the rig step says "${hamlib}"`);
  results.push(`PASS W5 versions: modem step "${choice}"; rig step "${hamlib}"`);
  await s.eval(setInput(".wizard input.rig-model", "1"));
  if (await s.eval("!!document.querySelector('.wizard input.rig-serial-port')")) await s.eval(setInput(".wizard input.rig-serial-port", "COM10"));
  await fits("rig");
  await press(".wizard button.rig-test-own");
  await until(s, "!!document.querySelector('.wizard .rig-test-steps [data-step=connect]')", "the rig test's steps", 30000);
  const connect = await s.eval(text(".wizard .rig-test-steps [data-step=connect]"));
  if (!(await s.eval("document.querySelector('.wizard .rig-test-steps [data-step=connect]').classList.contains('ok')"))) {
    throw new Error(`W4: the read-only test did not connect: "${connect}"`);
  }
  results.push(`PASS W4 rig step: ${models} Hamlib models from the bundled rigctld; read-only test "${connect}"`);
  await heldReady(1000, "after the rig step");
  await press(".wizard .nav .next");
  await at("operate");

  // ---- How you operate: nothing chosen yet; Answering calls releases the hold.
  if (!(await s.eval("document.querySelector('.wizard .nav .next').disabled"))) throw new Error("W2: Next works before a choice");
  await fits("operate");
  await s.eval("document.querySelector('.wizard input.op-answering').click()");
  await press(".wizard .nav .next");
  await at("tour");
  await until(s, `${pill} === 'listening'`, "answering calls once chosen", 20000);

  // ---- First contact (skipped), Done, then Operate.
  await fits("tour");
  await press(".wizard .nav .skip");
  await at("done");
  const skippedNote = await s.eval(text(".wizard .body"));
  if (!skippedNote.includes("First contact")) throw new Error(`W2: Done does not list the skipped step: "${skippedNote}"`);
  await fits("done");
  await press(".wizard .nav .finish");
  await until(s, "!document.querySelector('.wizard') && !!document.querySelector('header .station-mode') && !!document.querySelector('section.strip')", "Operate after the wizard", 15000);
  await s.eval(`[...document.querySelectorAll('.rail button')].find(b => b.getAttribute('aria-label') === 'Settings').click()`);
  await until(s, "!!document.getElementById('tab-profiles')", "Settings");
  await s.eval("document.getElementById('tab-profiles').click()");
  await until(s, "!!document.querySelector('#panel-profiles .skipped-steps [data-step=tour]') && !!document.querySelector('#panel-profiles button.run-setup')", "the skipped step in Settings");
  results.push(`PASS W2 built-in path to Done: Ready (Receive only) until "How you operate", then Listening; the skipped step in Settings: "${await s.eval(text("#panel-profiles .skipped-steps"))}"`);

  results.push(...fit);
  await sleep(500);
  await errorsSoFar("the end");
  results.push("PASS W7 console: no errors from the first screen to the end");
} catch (e) {
  results.push(...fit);
  results.push(`FAIL ${e.message}`);
  if (app?.out) console.error(app.out.slice(-2000));
} finally {
  s?.close();
  kill(app);
}
for (const r of results) console.log(r);
process.exit(results.some((r) => r.startsWith("FAIL")) || results.length < 6 ? 1 : 0);
