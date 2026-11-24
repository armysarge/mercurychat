// M5 Task 13: keyboard, names, contrast and scaling, in the real window (a
// debug build with the fake built-in bundle: no Mercury, no audio, no rig, no
// RF; the simulator for the chat). Views: the wizard (welcome, station), the
// chat (the station home, a session with SIM1), every Settings tab and the
// Logbook. On each view (a11y.mjs):
//   K  Tab reaches every visible control in DOM order; focus is never trapped
//   N  every control has an accessible name (Chromium's accessibility tree)
//   C  text contrast ≥ 4.5:1 (3:1 large), in the dark theme and the light one
// And at 150 % and 200 % zoom (the CSS viewport of a 1366×768 window divided
// by the zoom) on the chat, Settings and the Logbook:
//   Z  nothing overflows sideways, the station panel stays one row, and the
//      header's items (the window controls too) neither overlap nor leave the window
// Usage: node lab/windows/m5-a11y-check.mjs <mercurychat.exe> <empty data dir> <png out dir> [port]
import { existsSync, mkdirSync, rmSync } from "node:fs";
import { headerFits, lowContrast, panelOneRow, tabWalk, unnamed } from "./a11y.mjs";
import { fakeBundle } from "./fakebundle.mjs";
import { firstUse, kill, launch, sleep, until } from "./cdp.mjs";
import { click, compose, pill, setInput } from "./m2-dom.mjs";
import { shoot } from "./responsive.mjs";

const [exe, dataDir, outDir, port = "9380"] = process.argv.slice(2);
if (!exe || !dataDir || !outDir) {
  console.error("usage: node m5-a11y-check.mjs <mercurychat.exe> <empty data dir> <png out dir> [port]");
  process.exit(2);
}
const CALL = "ZS6KEY";
const ZOOMS = [
  ["zoom150", Math.round(1366 / 1.5), Math.round(768 / 1.5)],
  ["zoom200", Math.round(1366 / 2), Math.round(768 / 2)],
];
const TABS = ["station", "profiles", "modem", "rig", "audio", "beacon", "files", "notifications", "appearance", "about"];
const rail = (name) => `[...document.querySelectorAll('.rail button')].find(b => b.getAttribute('aria-label') === ${JSON.stringify(name)})`;

const results = [];
let s;
let app;

async function scheme(light) {
  await s.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: light ? "light" : "dark" }] });
  await sleep(300);
}

/** K, N and C (both themes) on the view now shown. */
async function audit(view) {
  const k = await tabWalk(s);
  results.push(k.found.length ? `FAIL K ${view}: ${k.found.join("; ")}` : `PASS K ${view}: Tab reached ${k.reached}/${k.n} in order`);
  const n = await unnamed(s);
  results.push(n.length ? `FAIL N ${view}: no name: ${n.join("; ")}` : `PASS N ${view}: every control named`);
  for (const light of [false, true]) {
    await scheme(light);
    const c = await lowContrast(s);
    const theme = light ? "light" : "dark";
    results.push(c.length ? `FAIL C ${view} ${theme}: ${c.join("; ")}` : `PASS C ${view} ${theme}: text contrast AA`);
  }
  await scheme(false);
}

/** Z on the view now shown: no sideways overflow at 150 % and 200 %, the panel one row. */
async function zoom(view) {
  results.push(...(await shoot(s, outDir, `${view}`, ZOOMS)).map((l) => l.replace(/^(PASS|FAIL) /, "$1 Z ")));
  await s.eval("document.querySelector('header .station-pill')?.getAttribute('aria-expanded') === 'true' || document.querySelector('header .station-pill')?.click()");
  for (const [name, w, h] of ZOOMS) {
    await s.send("Emulation.setDeviceMetricsOverride", { width: w, height: h, deviceScaleFactor: 1, mobile: false });
    await sleep(500);
    const row = await panelOneRow(s);
    results.push(row ? `FAIL Z ${view} ${name}: ${row}` : `PASS Z ${view} ${name}: the station panel is one row`);
    const head = await headerFits(s);
    results.push(head ? `FAIL Z ${view} ${name}: header: ${head}` : `PASS Z ${view} ${name}: the header's items all show, none overlapping`);
  }
  await s.send("Emulation.clearDeviceMetricsOverride");
  await s.eval("document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))");
  await sleep(200);
}

try {
  if (existsSync(dataDir)) rmSync(dataDir, { recursive: true, force: true });
  mkdirSync(outDir, { recursive: true });
  process.env.MC_BUNDLE_DIR = fakeBundle(exe, dataDir);
  app = launch(exe, dataDir, port);
  delete process.env.MC_BUNDLE_DIR;
  // firstUse leaves the wizard by its "Set up without the guide" button: the plain setup screen.
  s = await firstUse(port, "Use simulator", 40000);
  await audit("setup");

  // ---- the chat on the simulator.
  await s.eval(setInput('form.simulator input[name="callsign"]', CALL));
  await s.eval(click("Use simulator"));
  await until(s, `['ready','listening'].includes(${pill}) && !!document.querySelector('.station-home')`, "the station home", 30000);
  await audit("operate-home");
  await zoom("operate-home");

  await s.eval("[...document.querySelectorAll('aside.sidebar [role=tab]')].find(b => b.textContent.trim().startsWith('Heard')).click()");
  const sim1 = "document.querySelector('.heard .row[data-callsign=\"SIM1\"] button.pick')";
  await until(s, sim1, "SIM1 heard", 20000);
  await s.eval(`${sim1}.click()`);
  await until(s, "!!document.querySelector('.peer-header h1')", "SIM1's conversation");
  await s.eval(click("Call"));
  await until(s, `${pill} === 'connected'`, "connected to SIM1", 25000);
  await s.eval(compose("Keyboard and contrast check. 73"));
  await until(s, "document.querySelectorAll('.transcript [data-direction=in]').length >= 1", "SIM1's echo", 25000);
  await audit("operate-session");
  await zoom("operate-session");

  // ---- every Settings tab.
  await s.eval(`${rail("Settings")}.click()`);
  await until(s, "!!document.getElementById('tab-station')", "Settings");
  for (const tab of TABS) {
    await s.eval(`document.getElementById('tab-${tab}').click()`);
    await until(s, `!!document.getElementById('panel-${tab}')`, `the ${tab} tab`);
    await sleep(300);
    await audit(`settings-${tab}`);
  }
  await zoom("settings-about");

  // ---- the Logbook.
  await s.eval(`${rail("Logbook")}.click()`);
  await until(s, "!!document.querySelector('.export-bar')", "the Logbook");
  await sleep(300);
  await audit("logbook");
  await zoom("logbook");

  // ---- the wizard: Settings → Profiles → Run setup again, welcome and station.
  await s.eval(`${rail("Settings")}.click()`);
  await until(s, "!!document.getElementById('tab-profiles')", "Settings");
  await s.eval("document.getElementById('tab-profiles').click()");
  await until(s, "!!document.querySelector('button.run-setup')", "Run setup again");
  await s.eval("document.querySelector('button.run-setup').click()");
  await until(s, "document.querySelector('.wizard [data-step]')?.dataset.step === 'welcome'", "the wizard's welcome", 10000);
  await audit("wizard-welcome");
  await s.eval("document.querySelector('.wizard button.next').click()");
  await until(s, "document.querySelector('.wizard [data-step]')?.dataset.step === 'station'", "the wizard's station step", 10000);
  await audit("wizard-station");
} catch (e) {
  results.push(`FAIL ${e.message}`);
} finally {
  s?.close();
  kill(app);
}
for (const r of results) console.log(r);
const fails = results.filter((r) => r.startsWith("FAIL")).length;
console.log(`${results.length - fails} passed, ${fails} failed`);
process.exit(fails || results.length < 40 ? 1 : 0);
