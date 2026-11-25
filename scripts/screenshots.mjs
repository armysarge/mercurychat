// Screenshots of the real app for the repository and README (M3a Task 14;
// M5's manual generator calls it for every release, so no shot is taken by hand).
//
// It starts the built app on a fresh data folder, makes a simulator station with
// a neutral demo callsign (N0CALL; the simulator's own peer is SIM1), sets up
// what each shot needs through the window itself, and captures it over the
// WebView2 DevTools protocol. Nothing personal is shown: no owner callsign, no
// real endpoint, no lab path.
//
// Usage (Windows; the app built with its assets embedded,
// `cargo build -p mercurychat --features tauri/custom-protocol`):
//   node scripts/screenshots.mjs <mercurychat.exe> <out dir> [shots.json] [--only name,name]
//
// A shot is { name, view, theme, setup: [step...], clip? }:
//   view   home (the station home) | operate (a conversation) | logbook | settings
//   theme  a theme id from apps/desktop/src/themes/presets.ts (or "system")
//   setup  named steps (STEPS below), each done once per run, in order
//   side   the Operate sidebar tab: chats | heard
//   tab    a Settings tab id (station, rig, ...)
//   clip   a CSS selector: capture only that element (plus a margin)
//   scroll a CSS selector to scroll into view first
// Each PNG must be at most MAX_BYTES; a larger one fails the run.
import { mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { connect, firstUse, kill, launch, pages, sleep, until } from "../lab/windows/cdp.mjs";
import { click, compose, pill, setInput } from "../lab/windows/m2-dom.mjs";

const MAX_BYTES = 300 * 1024;
const WIDTH = 1500;
const HEIGHT = 920;
const CALL = "N0CALL";

/** The default shot list: what the README and the manual use. */
const SHOTS = [
  // The idle station home comes first: it is gone once a conversation is open.
  { name: "station-home", view: "home", theme: "midnight", setup: ["station", "heard", "beacons"], side: "heard" },
  { name: "operate-dark", view: "operate", theme: "midnight", setup: ["session", "chat", "panel"], side: "heard" },
  { name: "qso-card", view: "operate", theme: "midnight", setup: ["session", "qso"], clip: "section.qso-card" },
  { name: "operate-light", view: "operate", theme: "daylight", setup: ["session"], side: "chats" },
  { name: "settings-station", view: "settings", theme: "midnight", setup: ["stationInfo"], tab: "station" },
  { name: "logbook", view: "logbook", theme: "midnight", setup: ["disconnect"] },
  { name: "settings-rig", view: "settings", theme: "midnight", setup: ["disconnect", "rigTest"], tab: "rig", scroll: ".rig-settings fieldset.box:has(button.rig-test)" },
];

const args = process.argv.slice(2);
const only = (() => {
  const i = args.indexOf("--only");
  if (i < 0) return null;
  const list = args[i + 1]?.split(",") ?? [];
  args.splice(i, 2);
  return list;
})();
const [exe, outArg, listArg] = args;
if (!exe || !outArg) {
  console.error("usage: node scripts/screenshots.mjs <mercurychat.exe> <out dir> [shots.json] [--only name,name]");
  process.exit(2);
}
const out = resolve(outArg);
mkdirSync(out, { recursive: true });
const shots = (listArg ? JSON.parse(readFileSync(listArg, "utf8")) : SHOTS).filter((s) => !only || only.includes(s.name));

// The rail by its ids, not its labels (Operate is "Chats" there).
const rail = (id) => `document.querySelector('.rail [data-rail=${id}]')`;
const VIEW_RAIL = { home: "operate", operate: "chats", logbook: "logbook", settings: "settings" };
const sideTab = (id) => `document.getElementById(${JSON.stringify(`side-tab-${id}`)})?.click()`;
const sim1 = `.heard .row[data-callsign="SIM1"]`;

/** The named set-up steps. Each runs at most once per run. */
const STEPS = {
  async station(s) {
    await s.eval(setInput('form.simulator input[name="callsign"]', CALL));
    await s.eval(click("Use simulator"));
    await until(s, `['ready','listening'].includes(${pill}) && !!document.querySelector('.station-home')`, "the station home", 30000);
  },
  async heard(s) {
    await s.eval(sideTab("heard"));
    await until(s, `!!document.querySelector('${sim1} .grid')`, "SIM1 heard with its locator", 75000);
  },
  async beacons(s) {
    await until(s, "!!document.querySelector('.station-home input.beacon-toggle')", "the beacon card");
    if (!(await s.eval("document.querySelector('.station-home input.beacon-toggle').checked"))) await s.eval("document.querySelector('.station-home input.beacon-toggle').click()");
    await until(s, "!!document.querySelector('header .beacon-in')", "the beacon countdown", 15000);
  },
  async session(s) {
    if ((await s.eval(pill)) === "connected") return;
    await s.eval(`${rail("chats")}.click()`);
    await s.eval(sideTab("heard"));
    await until(s, `!!document.querySelector('${sim1} button.pick')`, "SIM1 in the Heard list", 75000);
    await s.eval(`document.querySelector('${sim1} button.pick').click()`);
    await until(s, "document.querySelector('.peer-header h1')?.textContent.trim() === 'SIM1'", "SIM1 open");
    if ((await s.eval(pill)) !== "connected") {
      await until(s, "!!document.querySelector('.peer-header button.primary:not([disabled])')", "the Call button");
      await s.eval("document.querySelector('.peer-header button.primary').click()");
    }
    await until(s, `${pill} === 'connected'`, "connected to SIM1", 30000);
  },
  async panel(s) {
    const toggle = "document.querySelector('.peer-header button.toggle-session')";
    if ((await s.eval(`${toggle}.getAttribute('aria-pressed')`)) !== "true") await s.eval(`${toggle}.click()`);
    await until(s, "!!document.querySelector('.session-pane .qso-card')", "the QSO record panel", 20000);
  },
  async chat(s) {
    for (const text of ["Good afternoon from N0CALL, testing MercuryChat on the simulator.", "Signal looks clean here. 73!"]) {
      await s.eval(compose(text));
      await until(s, `[...document.querySelectorAll('.transcript [data-id]')].some(e => e.dataset.state === 'delivered' && e.querySelector('.body').textContent === ${JSON.stringify(text)})`, "a delivered message", 20000);
    }
  },
  async qso(s) {
    await STEPS.panel(s);
    await until(s, "!!document.querySelector('section.qso-card input[name=rstSent]')", "the QSO card", 20000);
    for (const [name, value] of [["rstSent", "59"], ["rstRcvd", "57"], ["name", "Sam"], ["qth", "Simulator"]]) {
      await s.eval(setInput(`section.qso-card input[name=${name}]`, value));
    }
    await s.eval(setInput("section.qso-card textarea[name=notes]", "First contact on MercuryChat."));
    await s.eval("document.querySelector('section.qso-card button.qso-save').click()");
    await until(s, "/\\S/.test(document.querySelector('section.qso-card .saved')?.textContent ?? '')", "the record saved");
  },
  async stationInfo(s) {
    await s.eval(`${rail("settings")}.click()`);
    await until(s, "!!document.getElementById('tab-station')", "the Settings tabs");
    await s.eval("document.getElementById('tab-station').click()");
    await until(s, "!!document.querySelector('form.station-info')", "the station information form");
    const fields = { name: "Alex", grid: "FN31pr", qth: "Springfield", rig: "HF transceiver", power: "50 W", antenna: "End-fed half-wave", about: "Portable station, evenings and weekends" };
    for (const [k, v] of Object.entries(fields)) await s.eval(setInput(`form.station-info input[name=${k}]`, v));
    await s.eval("document.querySelector('form.station-info button[type=submit]').click()");
    await until(s, "/\\S/.test(document.querySelector('form.station-info .saved')?.textContent ?? '')", "station information saved");
    await until(s, "document.querySelectorAll('.macro-editor .macro').length >= 3", "the macros");
  },
  async rigTest(s) {
    await s.eval(`${rail("settings")}.click()`);
    await until(s, "!!document.getElementById('tab-rig')", "the Settings tabs");
    await s.eval("document.getElementById('tab-rig').click()");
    await until(s, "!!document.querySelector('.rig-settings button.rig-test')", "the rig link test");
    await s.eval("document.querySelector('.rig-settings button.rig-test').click()");
    await until(s, "document.querySelectorAll('.rig-test-steps li').length === 6", "the six test steps", 15000);
  },
  async disconnect(s) {
    if ((await s.eval(pill)) !== "connected") return;
    await s.eval(`${rail("chats")}.click()`);
    await s.eval("document.querySelector('.peer-header button.disconnect').click()");
    await until(s, `['ready','listening'].includes(${pill})`, "the session ended", 20000);
  },
};

async function setTheme(s, id) {
  await s.eval(`${rail("settings")}.click()`);
  await until(s, `!!document.querySelector('[data-theme-id=${JSON.stringify(id)}]')`, `theme ${id}`);
  await s.eval(`document.querySelector('[data-theme-id=${JSON.stringify(id)}]').click()`);
  await sleep(300);
}

const data = join(tmpdir(), `mc-shots-${process.pid}`);
rmSync(data, { recursive: true, force: true });
const port = "9371";
const app = launch(exe, data, port);
const done = new Set();
const results = [];
try {
  // A new install opens the setup wizard; this leaves it for the plain setup screen.
  const s = await firstUse(port, "Use simulator", 90000);
  await s.send("Emulation.setDeviceMetricsOverride", { width: WIDTH, height: HEIGHT, deviceScaleFactor: 1, mobile: false });
  for (const step of ["station"]) {
    await STEPS[step](s);
    done.add(step);
  }
  for (const shot of shots) {
    for (const step of shot.setup ?? []) {
      if (done.has(step)) continue;
      if (!STEPS[step]) throw new Error(`${shot.name}: no step "${step}"`);
      await STEPS[step](s);
      done.add(step);
    }
    if (shot.theme) await setTheme(s, shot.theme);
    await s.eval(`${rail(VIEW_RAIL[shot.view] ?? "chats")}.click()`);
    if (shot.side) await s.eval(sideTab(shot.side));
    if (shot.tab) await s.eval(`document.getElementById(${JSON.stringify(`tab-${shot.tab}`)}).click()`);
    await sleep(600);
    if (shot.scroll) await s.eval(`document.querySelector(${JSON.stringify(shot.scroll)})?.scrollIntoView({ block: 'start' })`);
    await sleep(300);
    let clip;
    if (shot.clip) {
      const r = await s.eval(`(() => { const e = document.querySelector(${JSON.stringify(shot.clip)}); e?.scrollIntoView({ block: 'nearest' }); const b = e?.getBoundingClientRect(); return b && { x: b.x, y: b.y, width: b.width, height: b.height }; })()`);
      if (!r) throw new Error(`${shot.name}: nothing matches ${shot.clip}`);
      const m = 12;
      clip = { x: Math.max(0, r.x - m), y: Math.max(0, r.y - m), width: r.width + 2 * m, height: r.height + 2 * m, scale: 1 };
    }
    const { data: png } = await s.send("Page.captureScreenshot", { format: "png", ...(clip ? { clip } : {}) });
    const file = join(out, `${shot.name}.png`);
    writeFileSync(file, Buffer.from(png, "base64"));
    const size = statSync(file).size;
    if (size > MAX_BYTES) throw new Error(`${shot.name}.png is ${Math.round(size / 1024)} KB, over ${MAX_BYTES / 1024} KB`);
    results.push(`OK ${shot.name}.png ${Math.round(size / 1024)} KB (${shot.view}, ${shot.theme})`);
  }
  s.close();
} catch (e) {
  results.push(`FAIL ${e.message.slice(0, 1200)}`);
  if (app.out) results.push(`APP OUTPUT ${app.out.slice(-1500)}`);
  // What the window was doing: no DevTools port, no page, or another screen.
  results.push(`APP ${app.exitCode === null ? "still running" : `exited ${app.exitCode}`}`);
  try {
    const found = await pages(port);
    results.push(`DEVTOOLS PAGES ${found.length}`);
    for (const t of found) {
      const c = await connect(t.webSocketDebuggerUrl);
      results.push(`PAGE ${t.url} TEXT ${String(await c.eval("document.body ? document.body.innerText : '(no body)'")).slice(0, 800)}`);
      c.close();
    }
  } catch (d) {
    results.push(`DIAG ${d.message}`);
  }
} finally {
  kill(app);
  await sleep(500);
  rmSync(data, { recursive: true, force: true });
}
console.log(results.join("\n"));
process.exit(results.some((r) => r.startsWith("FAIL")) ? 1 : 0);
