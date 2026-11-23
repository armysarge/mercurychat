// M5 Task 11b: the Operate screens after the restyle (the owner's design
// canvas, combined with every element the app already had), in the real
// window on the simulator (a debug build: no Mercury, no audio, no rig, no
// RF). Each view is measured at tablet portrait 768×1024, tablet landscape
// 1024×768 and desktop 1366×768 (responsive.mjs) and screenshotted.
//   O1 the idle station home: the dial, the bands, the spectrum, and the CQ
//      and Beacon cards, with no conversation open
//   O2 the header: the station pill (callsign and frequency) and the modem's
//      status pill naming where the modem is
//   O3 a session with SIM1: messenger bubbles (ours on the right), the
//      session header with the peer's avatar and state
//   O4 the QSO record beside the conversation (the session panel), still
//      holding everything it held
//   O5 no console errors
//   O6 no bar under the header (owner, 23 Nov): the station panel is closed
//      at first, the station pill opens it over the workspace (the layout
//      does not move) and Escape closes it
// Usage: node lab/windows/m5-operate-check.mjs <mercurychat.exe> <empty data dir> <png out dir> [port]
import { existsSync, mkdirSync, rmSync } from "node:fs";
import { firstUse, kill, launch, sleep, until } from "./cdp.mjs";
import { click, compose, pill, setInput } from "./m2-dom.mjs";
import { shoot } from "./responsive.mjs";

const [exe, dataDir, outDir, port = "9366"] = process.argv.slice(2);
if (!exe || !dataDir || !outDir) {
  console.error("usage: node m5-operate-check.mjs <mercurychat.exe> <empty data dir> <png out dir> [port]");
  process.exit(2);
}
const CALL = "ZS6OPS";
const rail = (name) => `[...document.querySelectorAll('.rail button')].find(b => b.getAttribute('aria-label') === ${JSON.stringify(name)})`;
const heardTab = "[...document.querySelectorAll('aside.sidebar [role=tab]')].find(b => b.textContent.trim().startsWith('Heard')).click()";
const sim1 = "document.querySelector('.heard .row[data-callsign=\"SIM1\"] button.pick')";
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
const fit = [];
let s;
let app;
const fits = async (view) => fit.push(...(await shoot(s, outDir, view)));

try {
  if (existsSync(dataDir)) rmSync(dataDir, { recursive: true, force: true });
  mkdirSync(outDir, { recursive: true });
  app = launch(exe, dataDir, port);
  s = await firstUse(port, "Use simulator", 40000);
  await s.eval(watchErrors);
  await s.eval(setInput('form.simulator input[name="callsign"]', CALL));
  await s.eval(click("Use simulator"));
  await until(s, `['ready','listening'].includes(${pill}) && !!document.querySelector('textarea.composer')`, "the workspace", 30000);

  // ---- O1: the idle station home.
  await until(s, "!!document.querySelector('.station-home')", "the station home", 10000);
  const home = await s.eval(`({
    dial: !!document.querySelector('.station-home .dial'),
    cq: !!document.querySelector('.station-home .cq-card button.send-cq'),
    beacon: !!document.querySelector('.station-home .beacon-card'),
    spectrum: !!document.querySelector('.station-home .spectrum'),
  })`);
  for (const [k, v] of Object.entries(home)) if (!v) throw new Error(`O1: the station home has no ${k}`);
  if (await s.eval("getComputedStyle(document.querySelector('section.strip')).display !== 'none'")) throw new Error("O6: the station panel is open at first");
  results.push(`PASS O1 station home: dial, bands "${await s.eval(text(".station-home .bands"))}", spectrum, CQ and Beacon cards`);

  // ---- O2: the header's station pill and status pill.
  const station = await s.eval(text("header .station-pill"));
  const status = await s.eval(text("header .state"));
  if (!station.includes(CALL)) throw new Error(`O2: the station pill says "${station}"`);
  if (!/Simulator/i.test(status)) throw new Error(`O2: the status pill does not say where the modem is: "${status}"`);
  results.push(`PASS O2 header: station "${station}"; status "${status}"`);
  await fits("operate-idle");

  // ---- O3: a session with SIM1, a message each way.
  await s.eval(heardTab);
  await until(s, sim1, "SIM1 in the Heard list", 20000);
  await s.eval(`${sim1}.click()`);
  await until(s, "!!document.querySelector('.peer-header .avatar')", "SIM1's conversation with its avatar");
  await s.eval(click("Call"));
  await until(s, `${pill} === 'connected'`, "connected to SIM1", 25000);
  await s.eval(compose("Good afternoon from the restyle check. 73"));
  await until(s, "document.querySelectorAll('.transcript [data-direction=in]').length >= 1", "SIM1's echo", 25000);
  const sides = await s.eval(`(() => {
    const out = document.querySelector('.transcript [data-direction=out]');
    const inn = document.querySelector('.transcript [data-direction=in]');
    const t = document.querySelector('.transcript').getBoundingClientRect();
    const mid = (e) => { const r = e.getBoundingClientRect(); return (r.left + r.right) / 2 - (t.left + t.right) / 2; };
    return { out: mid(out), in: mid(inn) };
  })()`);
  if (!(sides.out > 0 && sides.in < 0)) throw new Error(`O3: bubbles not on their sides: ${JSON.stringify(sides)}`);
  results.push(`PASS O3 session: ours right (+${Math.round(sides.out)} px), SIM1's left (${Math.round(sides.in)} px); header "${await s.eval(text(".peer-header .who"))}"`);
  await fits("operate-session");

  // ---- O4: the session panel with the QSO record.
  await s.eval("document.querySelector('button.toggle-session').click()");
  await until(s, "!!document.querySelector('.session-pane.open .qso-card, .session-pane .qso-card')", "the QSO record");
  await sleep(500);
  results.push(`PASS O4 session panel: "${(await s.eval(text(".session-pane"))).slice(0, 120)}…"`);
  await fits("operate-session-panel");

  // ---- O6: the station panel drops from the pill and covers, never pushes.
  // Escape closes the session drawer first (it is on top), then the panel.
  await s.eval("document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))");
  const top = "Math.round(document.querySelector('.body').getBoundingClientRect().top)";
  const before = await s.eval(top);
  await s.eval("document.querySelector('header .station-pill').click()");
  await until(s, "getComputedStyle(document.querySelector('section.strip')).display !== 'none'", "the station panel open");
  const after = await s.eval(top);
  if (after !== before) throw new Error(`O6: opening the panel moved the workspace ${before} -> ${after} px`);
  if ((await s.eval("document.querySelector('header .station-pill').getAttribute('aria-expanded')")) !== "true") throw new Error("O6: the pill does not say the panel is open");
  await fits("operate-panel");
  await s.eval("document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))");
  await until(s, "getComputedStyle(document.querySelector('section.strip')).display === 'none'", "Escape closing the panel");
  results.push(`PASS O6 station panel: closed at first; the pill opens it over the workspace (top stays ${before} px); Escape closes it`);

  results.push(...fit);
  const errs = (await s.eval("window.__uiErrors ?? []")) ?? [];
  if (errs.length) throw new Error(`O5 console errors: ${errs.join(" | ")}`);
  results.push("PASS O5 console: no errors");
} catch (e) {
  results.push(...fit);
  results.push(`FAIL ${e.message}`);
} finally {
  s?.close();
  kill(app);
}
for (const r of results) console.log(r);
process.exit(results.some((r) => r.startsWith("FAIL")) || results.length < 6 ? 1 : 0);
