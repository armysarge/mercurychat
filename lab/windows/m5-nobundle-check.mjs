// M5 check of a build WITHOUT a built-in modem (a developer build, no
// `modem\` folder, no MC_BUNDLE_DIR): the built-in choice is not silently
// missing. It is shown, says why it cannot be used, and has no button that
// would try. No Mercury, no audio device, no rig, no RF.
//   N1 first-use: the built-in card is there, explains it is missing, offers no "Use"
//   N2 the wizard's Modem step does the same
//   N3 no console errors
// Usage: node lab/windows/m5-nobundle-check.mjs <mercurychat.exe> <empty data dir> [port]
import { existsSync, rmSync } from "node:fs";
import { firstUse, kill, launch, page, sleep, until } from "./cdp.mjs";
import { setInput } from "./m2-dom.mjs";

const [exe, dataDir, port = "9367"] = process.argv.slice(2);
if (!exe || !dataDir) {
  console.error("usage: node m5-nobundle-check.mjs <mercurychat.exe> <empty data dir> [port]");
  process.exit(2);
}
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

const results = [];
let s;
let app;

/** The built-in card under `scope`: there, explained, and without a Use button. */
async function missingCard(scope, id) {
  const sel = `${scope} .builtin-missing`;
  await until(s, `!!document.querySelector(${JSON.stringify(sel)})`, `${id}: the built-in card`, 15000);
  const said = await s.eval(text(sel));
  if (!said.includes("Built-in Mercury")) throw new Error(`${id}: the card does not name the built-in modem: "${said}"`);
  if (!/installer/i.test(said)) throw new Error(`${id}: the card does not say where the built-in modem comes from: "${said}"`);
  if (await s.eval(`!!document.querySelector(${JSON.stringify(scope + " form.builtin")})`)) {
    throw new Error(`${id}: the usable built-in form shows without a bundle`);
  }
  return said;
}

try {
  if (existsSync(dataDir)) rmSync(dataDir, { recursive: true, force: true });
  delete process.env.MC_BUNDLE_DIR;
  app = launch(exe, dataDir, port);
  s = await page(port, (t) => t.includes("Set up without the guide"), "the wizard", 40000);
  await s.eval(watchErrors);

  // ---- N2: the wizard's Modem step.
  const step = "document.querySelector('.wizard .body')?.dataset.step";
  await s.eval("document.querySelector('.wizard .nav .next')?.click()");
  await until(s, `${step} === 'station'`, "the Station step", 15000);
  await s.eval(setInput('.wizard input[name="callsign"]', "ZS6NOB"));
  await s.eval("document.querySelector('.wizard .nav .next').click()");
  await until(s, `${step} === 'modem'`, "the Modem step", 15000);
  const wiz = await missingCard(".wizard", "N2");
  results.push(`PASS N2 wizard Modem step: "${wiz}"`);

  // ---- N1: first-use without the guide.
  // A fresh install again: the guide offers "Set up without the guide" only at its start.
  const before = (await s.eval("window.__uiErrors ?? []")) ?? [];
  if (before.length) throw new Error(`N3 console errors in the wizard: ${before.join(" | ")}`);
  kill(app);
  await sleep(1500);
  rmSync(dataDir, { recursive: true, force: true });
  app = launch(exe, dataDir, port);
  s = await firstUse(port, "My own Mercury", 40000);
  await s.eval(watchErrors);
  const plain = await missingCard("", "N1");
  results.push(`PASS N1 first-use: "${plain}"`);

  const errs = (await s.eval("window.__uiErrors ?? []")) ?? [];
  if (errs.length) throw new Error(`N3 console errors: ${errs.join(" | ")}`);
  results.push("PASS N3 console: no errors");
} catch (e) {
  results.push(`FAIL ${e.message}`);
} finally {
  if (app) kill(app);
}
for (const r of results) console.log(r);
const failed = results.filter((r) => r.startsWith("FAIL")).length;
console.log(`${results.length - failed} passed, ${failed} failed`);
process.exit(failed ? 1 : 0);
