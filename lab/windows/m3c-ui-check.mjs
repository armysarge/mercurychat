// M3c Task 13 check of the real desktop window, driven over the WebView2
// DevTools protocol (simulator only: no RF, no audio device; the simulator
// runs a fake rigctld). SIM1 accepts any QSY with its first candidate and
// answers information requests (sim::serve_peer).
//   Q5 a macro marked "quick" in Settings is listed first in the composer's
//      menu under "Quick messages"; picking it inserts without sending
//   Q4 Settings → Station → Share ticks persist across a window reload
//   Q3 Settings → Beacon: bytes left moves as fields are ticked; Save keeps them;
//      Save is disabled exactly when the count is negative
//   Q1 with SIM1 on MCH/1 minor 1 the session menu is enabled and Request SNR
//      shows SIM1's report; with a minor-0 SIM1 (MC_SIM_PEER_MINOR0=1) the items
//      are disabled with "not supported by this station"
//   Q2 Propose QSY with two entries: SIM1 accepts, the QSY card stages through
//      to the move, the sim rig is retuned to the first entry, and the re-call
//      brings SIM1 back
//   and no console errors from the first screen to the end.
// Usage: node lab/windows/m3c-ui-check.mjs <mercurychat.exe> <empty data dir> [port]
// Build the app with its assets embedded first:
//   npm --prefix apps/desktop run build && cargo build -p mercurychat --features tauri/custom-protocol
import { existsSync, rmSync } from "node:fs";
import { connect, firstUse, kill, launch, page, sleep, until } from "./cdp.mjs";
import { click, pill, setInput } from "./m2-dom.mjs";

const [exe, dataDir, port = "9361"] = process.argv.slice(2);
if (!exe || !dataDir) {
  console.error("usage: node m3c-ui-check.mjs <mercurychat.exe> <empty data dir> [port]");
  process.exit(2);
}
const CALL = "ZS6SIM";
const ready = `(['ready','listening'].includes(${pill})) && document.querySelector('textarea.composer, .station-home')`;
const rail = (name) => `[...document.querySelectorAll('.rail button')].find(b => b.getAttribute('aria-label') === ${JSON.stringify(name)})`;
const invoke = (cmd) => `window.__TAURI_INTERNALS__.invoke('app_call', { cmd: ${JSON.stringify(cmd)} })`;
const settingsTab = (name) => `document.getElementById('tab-${name}').click()`;
const watchErrors = `(() => {
  if (window.__uiErrors) return;
  window.__uiErrors = [];
  const keep = (x) => window.__uiErrors.push(String(x?.stack ?? x?.message ?? x).slice(0, 300));
  const err = console.error.bind(console);
  console.error = (...a) => { keep(a.join(' ')); err(...a); };
  addEventListener('error', (e) => keep(e.error ?? e.message));
  addEventListener('unhandledrejection', (e) => keep(e.reason));
})()`;
const heardTab = "[...document.querySelectorAll('aside.sidebar [role=tab]')].find(b => b.textContent.trim().startsWith('Heard')).click()";
const sim1 = "document.querySelector('.heard .row[data-callsign=\"SIM1\"] button.pick')";

const results = [];
const children = [];
let s;

async function start(env = {}) {
  Object.assign(process.env, env);
  const child = launch(exe, dataDir, port);
  for (const k of Object.keys(env)) delete process.env[k];
  children.push(child);
  return child;
}

/** Operate, SIM1's conversation open (it is heard within a few seconds of attach). */
async function openSim1() {
  await s.eval(`${rail("Chats")}.click()`);
  await s.eval(heardTab);
  await until(s, sim1, "SIM1 in the Heard list", 20000);
  await s.eval(`${sim1}.click()`);
  await until(s, "!!document.querySelector('button.macro-toggle') && !document.querySelector('button.macro-toggle').disabled", "SIM1's conversation open");
}

async function errorsSoFar() {
  return (await s.eval("window.__uiErrors ?? []")) ?? [];
}

try {
  if (existsSync(dataDir)) rmSync(dataDir, { recursive: true, force: true });
  let app = await start();
  s = await firstUse(port, "Use simulator", 40000);
  await s.eval(watchErrors);
  await s.eval(setInput('form.simulator input[name="callsign"]', CALL));
  await s.eval(click("Use simulator"));
  await until(s, ready, "the workspace", 30000);

  // Station information the beacon and the share use.
  await s.eval(`${rail("Settings")}.click()`);
  await until(s, "!!document.getElementById('tab-station')", "Settings");
  await s.eval(settingsTab("station"));
  await until(s, "!!document.querySelector('form.station-info input[name=name]')", "Station information");
  await s.eval(setInput("form.station-info input[name=name]", "Sam"));
  await s.eval(setInput("form.station-info input[name=qth]", "Pretoria"));
  await s.eval(setInput("form.station-info input[name=email]", "op@example.org"));
  await s.eval("document.querySelector('form.station-info button[type=submit]').click()");
  await until(s, "/saved/i.test(document.querySelector('form.station-info .saved')?.textContent ?? '')", "station information saved");

  // ---- Q5. a quick macro is on top of the composer's menu
  await s.eval("document.querySelector('button.macro-add').click()");
  await until(s, "!!document.querySelector('form.macro-edit input[name=name]')", "the macro form");
  await s.eval(setInput("form.macro-edit input[name=name]", "TU quick"));
  await s.eval(setInput("form.macro-edit textarea[name=body]", "TU 73 <CALL>"));
  await s.eval("document.querySelector('form.macro-edit input[name=quick]').click()");
  await s.eval("document.querySelector('form.macro-edit button[type=submit]').click()");
  await until(s, "!document.querySelector('form.macro-edit')", "the macro saved");
  await openSim1();
  const before = await s.eval("document.querySelectorAll('.transcript [data-id]').length");
  await s.eval("document.querySelector('button.macro-toggle').click()");
  await until(s, "!!document.querySelector('.macro-menu')", "the macro menu");
  const menu = await s.eval(`({
    heading: document.querySelector('.macro-menu .macro-heading')?.textContent.trim() ?? '',
    first: document.querySelector('.macro-menu [role=menuitem]')?.textContent.trim() ?? '',
    quick: [...document.querySelectorAll('.macro-menu .macro-quick')].map(b => b.textContent.trim()),
    order: [...document.querySelectorAll('.macro-menu > *')].map(e => e.className || e.tagName).slice(0, 4),
  })`);
  if (menu.heading !== "Quick messages" || menu.first !== "TU quick" || menu.quick.join() !== "TU quick") throw new Error(`Q5 menu: ${JSON.stringify(menu)}`);
  await s.eval("document.querySelector('.macro-menu .macro-quick').click()");
  await until(s, "document.querySelector('textarea.composer').value === 'TU 73 SIM1'", "the quick message in the draft");
  await sleep(500);
  if ((await s.eval("document.querySelectorAll('.transcript [data-id]').length")) !== before) throw new Error("Q5: picking a quick message sent it");
  await s.eval("(() => { const t = document.querySelector('textarea.composer'); t.value = ''; t.dispatchEvent(new Event('input')); })()");
  results.push(`PASS Q5 quick messages: "${menu.heading}" heads the menu with "${menu.first}"; a click put "TU 73 SIM1" in the draft and sent nothing`);

  // ---- Q4. share ticks persist
  await s.eval(`${rail("Settings")}.click()`);
  await s.eval(settingsTab("station"));
  await until(s, "!!document.querySelector('section.share input[name=share-rig]')", "the share ticks");
  const defaults = await s.eval("[...document.querySelectorAll('section.share input[type=checkbox]')].filter(i => i.checked).map(i => i.name)");
  if (defaults.join() !== "share-name,share-qth,share-locator") throw new Error(`Q4 defaults: ${defaults}`);
  await s.eval("document.querySelector('section.share input[name=share-rig]').click()");
  await s.eval("document.querySelector('section.share input[name=share-qth]').click()");
  await s.eval("document.querySelector('section.share button.share-save').click()");
  await until(s, "/saved/i.test(document.querySelector('section.share .share-status')?.textContent ?? '')", "sharing saved");
  await s.eval("location.reload()");
  await sleep(1500);
  s.close();
  s = await page(port, (t) => t.includes("Share with other stations") || t.includes(CALL), "the window after a reload", 30000);
  await s.eval(watchErrors);
  await s.eval(`${rail("Settings")}.click()`);
  await s.eval(settingsTab("station"));
  await until(s, "!!document.querySelector('section.share input[name=share-rig]')?.checked", "the rig tick after the reload");
  const kept = await s.eval("[...document.querySelectorAll('section.share input[type=checkbox]')].filter(i => i.checked).map(i => i.name)");
  if (kept.join() !== "share-name,share-locator,share-rig") throw new Error(`Q4 after reload: ${kept}`);
  results.push(`PASS Q4 share: defaults ${defaults.join(" ")}; after ticking rig and unticking QTH, a reload shows ${kept.join(" ")}`);

  // ---- Q3. Settings → Beacon bytes left
  await s.eval(settingsTab("beacon"));
  await until(s, "document.querySelector('.beacon-settings .beacon-left')?.dataset.left !== undefined", "the bytes left");
  const left = async () => Number(await s.eval("document.querySelector('.beacon-settings .beacon-left').dataset.left"));
  const l0 = await left();
  await s.eval("document.querySelector('.beacon-settings input[name=beacon-name]').click()");
  await until(s, `Number(document.querySelector('.beacon-settings .beacon-left').dataset.left) === ${l0 - 6}`, `bytes left after Name ("Sam", its 2-byte TLV head and the TLV count): ${l0 - 6}`);
  await s.eval("document.querySelector('.beacon-settings input[name=beacon-qth]').click()");
  await until(s, `Number(document.querySelector('.beacon-settings .beacon-left').dataset.left) === ${l0 - 16}`, "bytes left after QTH");
  await s.eval(setInput(".beacon-settings input[name=beacon-status]", "QRV 40m"));
  await until(s, `Number(document.querySelector('.beacon-settings .beacon-left').dataset.left) === ${l0 - 25}`, "bytes left after the status");
  const l3 = await left();
  const saveRule = await s.eval("(() => { const n = Number(document.querySelector('.beacon-settings .beacon-left').dataset.left); return document.querySelector('.beacon-settings button.beacon-save').disabled === (n < 0); })()");
  if (!saveRule) throw new Error("Q3: Save's disabled state does not follow the count");
  await s.eval("document.querySelector('.beacon-settings button.beacon-save').click()");
  await until(s, "/saved/i.test(document.querySelector('.beacon-settings .ok')?.textContent ?? '')", "beacon content saved");
  const stored = await s.eval(`${invoke({ type: "beaconContent" })}`);
  if (!stored.name || !stored.qth || stored.status !== "QRV 40m" || stored.bytesLeft !== l3) throw new Error(`Q3 stored: ${JSON.stringify(stored)}`);
  results.push(`PASS Q3 beacon: bytes left ${l0} -> ${l0 - 6} (name) -> ${l0 - 16} (QTH) -> ${l3} (status); saved as kind 2 content; Save is disabled only below 0 (not reachable at DATAC3 with this station)`);

  // ---- Q1. the session menu with a minor-1 SIM1, and SIM1's SNR report
  await openSim1();
  await s.eval(click("Call"));
  await until(s, `${pill} === 'connected'`, "connected to SIM1", 25000);
  await until(s, "!!document.querySelector('.session-menu button.qsy-propose')", "the session menu");
  const enabled = await s.eval("['qsy-propose','info-snr','info-station','info-email'].map(c => [c, document.querySelector('.session-menu button.' + c).disabled])");
  if (enabled.some(([, d]) => d)) throw new Error(`Q1 minor 1: disabled ${JSON.stringify(enabled)}`);
  await s.eval("document.querySelector('.session-menu button.info-snr').click()");
  await until(s, "/SIM1 reports your signal at \\+6\\.0 dB/.test(document.querySelector('.session-menu .peer-snr')?.textContent ?? '')", "SIM1's SNR report");
  const report = await s.eval("document.querySelector('.session-menu .peer-snr').textContent.trim()");
  results.push(`PASS Q1 minor 1: Propose QSY, Request SNR, Request station info and Send my email are enabled; "${report}"`);

  // ---- Q2. Propose QSY with two entries
  const freqs = await s.eval(`${invoke({ type: "frequencies" })}`);
  if (freqs.entries.length < 2) throw new Error(`Q2: the frequency list has ${freqs.entries.length} entries`);
  const firstSession = await s.eval(`(${invoke({ type: "snapshot" })}).then(x => x.session.sessionId)`);
  await s.eval(`(() => {
    window.__qsyStages = [];
    const look = () => { const c = document.querySelector('.qsy-card'); const st = c?.dataset.stage ?? ''; if (st && window.__qsyStages.at(-1) !== st) window.__qsyStages.push(st); };
    new MutationObserver(look).observe(document.body, { subtree: true, childList: true, attributes: true, attributeFilter: ['data-stage'] });
  })()`);
  await s.eval("document.querySelector('.session-menu button.qsy-propose').click()");
  await until(s, "document.querySelectorAll('.qsy-propose input[name=qsy-entry]').length >= 2", "the entries to propose");
  await s.eval("(() => { const b = [...document.querySelectorAll('.qsy-propose input[name=qsy-entry]')]; b[0].click(); b[1].click(); })()");
  const picked = await s.eval("[...document.querySelectorAll('.qsy-propose input[name=qsy-entry]')].filter(i => i.checked).map(i => i.value)");
  const target = freqs.entries.find((e) => e.id === picked[0]);
  await s.eval("document.querySelector('.qsy-propose button.qsy-send').click()");
  // SIM1 accepts candidate 0; this station (the caller) confirms, disconnects, retunes and calls again.
  await until(s, `(${invoke({ type: "snapshot" })}).then(x => x.session && x.session.sessionId !== ${JSON.stringify(firstSession)})`, "the re-call's new session with SIM1", 60000);
  const after = await s.eval(`(${invoke({ type: "snapshot" })}).then(x => ({ freq: x.rig.frequency?.value ?? null, mode: x.rig.mode, qsy: x.qsy, peer: x.session.peer }))`);
  const stages = await s.eval("window.__qsyStages");
  if (!stages.includes("proposed") || !stages.some((x) => x === "agreed" || x === "moving")) throw new Error(`Q2 stages: ${JSON.stringify(stages)}`);
  if (after.freq !== target.freqHz || after.qsy !== null || after.peer !== "SIM1") throw new Error(`Q2 after: ${JSON.stringify(after)} (wanted ${target.freqHz})`);
  results.push(`PASS Q2 QSY: proposed ${picked.length} entries; the card went ${stages.join(" -> ")}; the sim rig is on ${target.freqHz} Hz (${target.name}) and SIM1 is back in a new session`);
  await s.eval(click("Hang up")).catch(() => {});
  const errs = await errorsSoFar();
  if (errs.length) throw new Error(`console errors: ${errs.join(" | ")}`);

  // ---- Q1. a minor-0 SIM1: the items are disabled, with the reason
  s.close();
  kill(app);
  await sleep(2000);
  app = await start({ MC_SIM_PEER_MINOR0: "1" });
  s = await page(port, (t) => t.includes(CALL), "the window with a minor-0 SIM1", 40000);
  await s.eval(watchErrors);
  await until(s, ready, "the workspace (minor 0)", 30000);
  await openSim1();
  await s.eval(click("Call"));
  await until(s, `${pill} === 'connected'`, "connected to the minor-0 SIM1", 25000);
  await until(s, "!!document.querySelector('.session-menu button.qsy-propose')", "the session menu (minor 0)");
  const off = await s.eval("['qsy-propose','info-snr','info-station','info-email'].map(c => { const b = document.querySelector('.session-menu button.' + c); return [c, b.disabled, b.title]; })");
  if (off.some(([, d, title]) => !d || title !== "not supported by this station")) throw new Error(`Q1 minor 0: ${JSON.stringify(off)}`);
  const caps = await s.eval(`(${invoke({ type: "snapshot" })}).then(x => x.session.caps)`);
  if (caps.includes("qsy/1") || caps.includes("info/1")) throw new Error(`Q1 minor 0 caps: ${caps}`);
  results.push(`PASS Q1 minor 0: with SIM1 on caps [${caps.join(", ")}] the four items are disabled, titled "not supported by this station"`);
  const errs0 = await errorsSoFar();
  if (errs0.length) throw new Error(`console errors (minor 0): ${errs0.join(" | ")}`);
  results.push("PASS console: no errors from the first screen to the end");
} catch (e) {
  results.push(`FAIL ${e.message.slice(0, 2000)}`);
} finally {
  for (const c of children) kill(c);
}
console.log(results.join("\n"));
process.exit(results.some((r) => r.startsWith("FAIL")) ? 1 : 0);
